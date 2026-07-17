//! ai-config.json 读写 — 模型/密钥/执行策略配置。
//! 文件位于 app_data_dir/ai-config.json，权限 0600，不进入 git。

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::error::{Error, Result};

/// 单个 API endpoint（URL + Key），同模型多个 endpoint 组成负载均衡池。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Endpoint {
    #[serde(default)]
    pub url: Option<String>,
    pub key: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// 核采样概率（0-1），与 temperature 互补，OpenAI 规定二者只传一个
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// 频率惩罚（-2.0 到 2.0），正值降低已出现 token 的重复概率
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    /// 存在惩罚（-2.0 到 2.0），正值鼓励引入新话题
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    /// 生成停止序列（最多 4 个），命中任一则停止生成
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    /// 随机种子，用于可复现输出（部分模型支持）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
}

fn default_max_tokens() -> u32 {
    8192
}
fn default_temperature() -> f32 {
    0.7
}

/// providers.type.model = Vec<Endpoint>
/// type: "openai" | "anthropic"
/// model: 模型 ID（LLM 实际接收的 model 参数）
pub type Providers = HashMap<String, HashMap<String, Vec<Endpoint>>>;

/// 角色绑定模型（不填则用 default_provider + default_model）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RoleBinding {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
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
    #[serde(default = "default_model")]
    pub default_model: String,
    #[serde(default = "default_provider")]
    pub default_provider: String,
    #[serde(default)]
    pub execution: ExecutionConfig,
    #[serde(default)]
    pub roles: HashMap<String, RoleBinding>,
}

fn default_model() -> String {
    "deepseek-v3".into()
}
fn default_provider() -> String {
    "openai".into()
}

impl Default for AiConfig {
    fn default() -> Self {
        AiConfig {
            providers: Providers::new(),
            default_model: default_model(),
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
    /// 根据 provider type + model ID 获取全部 endpoint（round-robin 池）
    pub fn get_endpoints(&self, provider: &str, model: &str) -> Result<&[Endpoint]> {
        self.providers
            .get(provider)
            .and_then(|m| m.get(model))
            .map(|v| v.as_slice())
            .ok_or_else(|| {
                Error::msg(format!(
                    "未找到模型配置: provider={provider}, model={model}。\n\
                     请编辑 ai-config.json 添加对应条目。"
                ))
            })
    }

    /// 便捷方法：获取第一个 endpoint（向后兼容）
    pub fn get_endpoint(&self, provider: &str, model: &str) -> Result<&Endpoint> {
        self.get_endpoints(provider, model)?
            .first()
            .ok_or_else(|| Error::msg(format!("provider={provider}, model={model} 的 endpoint 列表为空")))
    }

    /// 解析角色绑定：优先用 role 配置，回退到 default
    pub fn resolve_model(&self, role: &str) -> Result<(&str, &str)> {
        let binding = self.roles.get(role);
        let provider = binding
            .and_then(|b| b.provider.as_deref())
            .unwrap_or(&self.default_provider);
        let model = binding
            .and_then(|b| b.model.as_deref())
            .unwrap_or(&self.default_model);
        if provider.is_empty() || model.is_empty() {
            return Err(Error::msg(
                "未配置 default_provider / default_model。\n\
                 请编辑 ai-config.json 设置默认模型。",
            ));
        }
        Ok((provider, model))
    }
}

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
