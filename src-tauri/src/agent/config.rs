//! ai-config.json 读写 — 模型/密钥/执行策略配置。
//! 文件位于 app_data_dir/ai-config.json，权限 0600，不进入 git。
//!
//! 配置格式（2026-07-17 定稿）：
//! ```json
//! {
//!   "providers": {
//!     "openai": {
//!       "default_model": "glm-5.2",
//!       "models": {
//!         "glm-5.2": {
//!           "show_name": "GLM 5.2",
//!           "endpoints": [
//!             { "id": "Zhipu/GLM-5.2", "url": "...", "key": "...", "weight": 5, "options": {} }
//!           ]
//!         }
//!       }
//!     }
//!   },
//!   "default_provider": "openai",
//!   "roles": { "code-review": { "model": "glm-5.2" } }
//! }
//! ```
//!
//! 简化格式兼容：endpoints 直接为数组时，show_name 默认 = key，endpoint.id 默认 = key。

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::error::{Error, Result};

// ---------- 生成参数 ----------

/// 生成参数（从 endpoint 级别覆盖，未设置时用默认值）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GenOptions {
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
}

fn default_max_tokens() -> u32 {
    8192
}
fn default_temperature() -> f32 {
    0.7
}

// ---------- Endpoint ----------

/// 单个 API endpoint。同一 model group 内多个 endpoint 组成负载均衡池。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Endpoint {
    /// 实际发给 API 的 model 参数。未设置时用 group key。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub key: String,
    /// 轮转权重（暂不实现，结构预留）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<u32>,
    /// 生成参数
    #[serde(default)]
    pub options: GenOptions,
}

// ---------- Model Group ----------

/// model 分组：同一逻辑模型的多个 endpoint。
/// 简化格式兼容：Vec<Endpoint> 直接反序列化为 { endpoints: Vec<Endpoint> }
#[derive(Debug, Clone, Serialize)]
pub struct ModelGroup {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_name: Option<String>,
    pub endpoints: Vec<Endpoint>,
}

// 简化格式兼容：数组 → { endpoints: array }
impl<'de> Deserialize<'de> for ModelGroup {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum GroupOrArray {
            Group {
                #[serde(default, skip_serializing_if = "Option::is_none")]
                show_name: Option<String>,
                endpoints: Vec<Endpoint>,
            },
            Array(Vec<Endpoint>),
        }

        match GroupOrArray::deserialize(deserializer)? {
            GroupOrArray::Group { show_name, endpoints } => Ok(ModelGroup { show_name, endpoints }),
            GroupOrArray::Array(arr) => Ok(ModelGroup { show_name: None, endpoints: arr }),
        }
    }
}

// ---------- Provider ----------

/// 单个 provider 类型（openai / anthropic）下的配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(default)]
    pub default_model: String,
    pub models: HashMap<String, ModelGroup>,
}

// ---------- 顶层配置 ----------

pub type Providers = HashMap<String, ProviderConfig>;

/// 角色绑定（只指定 model group ID，provider 从 default_provider 取）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RoleBinding {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionConfig {
    #[serde(default = "default_mode")]
    pub default_mode: String,
    #[serde(default = "default_dangerous_commands")]
    pub dangerous_commands: Vec<String>,
    #[serde(default = "default_always_ask_for")]
    pub always_ask_for: Vec<String>,
}

fn default_mode() -> String {
    "auto".into()
}
fn default_dangerous_commands() -> Vec<String> {
    vec![
        "rm".into(),
        "dd".into(),
        "mkfs".into(),
        "shutdown".into(),
        "reboot".into(),
    ]
}
fn default_always_ask_for() -> Vec<String> {
    vec!["run_command".into()]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    #[serde(default)]
    pub providers: Providers,
    #[serde(default = "default_provider")]
    pub default_provider: String,
    #[serde(default)]
    pub execution: ExecutionConfig,
    #[serde(default)]
    pub roles: HashMap<String, RoleBinding>,
}

fn default_provider() -> String {
    "openai".into()
}

impl Default for AiConfig {
    fn default() -> Self {
        AiConfig {
            providers: Providers::new(),
            default_provider: default_provider(),
            execution: ExecutionConfig {
                default_mode: default_mode(),
                dangerous_commands: default_dangerous_commands(),
                always_ask_for: default_always_ask_for(),
            },
            roles: HashMap::new(),
        }
    }
}

impl AiConfig {
    /// 获取全部 endpoint（round-robin 池）
    pub fn get_endpoints(&self, provider: &str, model: &str) -> Result<&[Endpoint]> {
        self.providers
            .get(provider)
            .and_then(|p| p.models.get(model))
            .map(|g| g.endpoints.as_slice())
            .ok_or_else(|| {
                Error::msg(format!(
                    "未找到模型配置: provider={provider}, model={model}。\n\
                     请编辑 ai-config.json 添加对应条目。"
                ))
            })
    }

    /// 解析角色绑定 → (provider, model_group_key)
    pub fn resolve_model(&self, role: &str) -> Result<(&str, &str)> {
        let binding = self.roles.get(role);
        let model = binding
            .and_then(|b| b.model.as_deref())
            .or_else(|| {
                self.providers
                    .get(&self.default_provider)
                    .map(|p| p.default_model.as_str())
            })
            .unwrap_or("");

        if model.is_empty() {
            return Err(Error::msg(
                "未配置默认模型。\n\
                 请编辑 ai-config.json，在 providers.openai 下设置 default_model。",
            ));
        }

        let provider = &self.default_provider;
        if provider.is_empty() {
            return Err(Error::msg("未配置 default_provider。"));
        }
        Ok((provider, model))
    }

    /// 获取 endpoint 的实际 API model ID（endpoint.id 或 fallback 到 group key）
    pub fn get_model_id<'a>(endpoint: &'a Endpoint, group_key: &'a str) -> &'a str {
        endpoint.id.as_deref().unwrap_or(group_key)
    }
}

// ---------- 文件读写 ----------

pub fn config_path(app: &AppHandle) -> Result<PathBuf> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| Error::msg(format!("获取配置目录失败: {e}")))?
        .join("ai-config.json"))
}

pub fn load_config(app: &AppHandle) -> Result<AiConfig> {
    let path = config_path(app)?;
    if !path.exists() {
        let template = AiConfig::default();
        save_config(app, &template)?;
        return Ok(template);
    }
    let data = fs::read_to_string(&path)
        .map_err(|e| Error::msg(format!("读取 ai-config.json 失败: {e}")))?;
    if data.trim().is_empty() {
        return Ok(AiConfig::default());
    }
    serde_json::from_str(&data).map_err(|e| Error::msg(format!("解析 ai-config.json 失败: {e}")))
}

pub fn save_config(app: &AppHandle, config: &AiConfig) -> Result<()> {
    let path = config_path(app)?;
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let data = serde_json::to_string_pretty(config)
        .map_err(|e| Error::msg(format!("序列化配置失败: {e}")))?;
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| Error::msg(format!("创建 ai-config.json 失败: {e}")))?
        .write_all(data.as_bytes())
        .map_err(|e| Error::msg(format!("写入 ai-config.json 失败: {e}")))?;
    Ok(())
}
