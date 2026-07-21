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
use serde_json::Value;
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
    /// HTTP 请求超时（秒）：覆盖从发送请求到收到完整响应的最大等待。
    /// 仅作用于初始响应；SSE 流式阶段由 stream_chunk_timeout 控制。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_timeout: Option<u32>,
    /// SSE 流式读取时，两个 chunk 之间的最大间隔（秒）。
    /// 超过则判定连接挂起，中止当前请求并触发重试。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_chunk_timeout: Option<u32>,
}

fn default_max_tokens() -> u32 {
    8192
}
fn default_temperature() -> f32 {
    0.7
}

/// 默认 HTTP 请求超时（秒）
pub const DEFAULT_REQUEST_TIMEOUT: u64 = 120;
/// 默认 SSE chunk 间隔超时（秒）
pub const DEFAULT_STREAM_CHUNK_TIMEOUT: u64 = 60;
/// 默认 run_command 超时（秒）
pub const DEFAULT_COMMAND_TIMEOUT: u32 = 120;

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
    /// 轮转权重（加权轮转已实现）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<u32>,
    /// 生成参数（OpenAI body 标准字段）
    #[serde(default)]
    pub options: GenOptions,
    /// 自定义 HTTP 头（如 OpenRouter 的 HTTP-Referer/X-Title，或平台特定头）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
    /// 非标准 body 参数透传（response_format/logit_bias/n 等，直接合并进 request body）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<HashMap<String, Value>>,
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
    /// run_command 工具的执行超时（秒）。超时后强制终止命令并返回错误。
    /// 防止 LLM 调用 tail -f 等不退出命令导致 session 永久阻塞。
    #[serde(default = "default_command_timeout")]
    pub command_timeout: u32,
    /// Ask 模式工具调用确认超时（秒）。
    #[serde(default = "default_ask_approval_timeout")]
    pub ask_approval_timeout: u32,
    /// Plan 模式计划确认超时（秒）。
    #[serde(default = "default_plan_confirm_timeout")]
    pub plan_confirm_timeout: u32,
    /// ask_user 工具提问超时（秒）。
    #[serde(default = "default_ask_user_timeout")]
    pub ask_user_timeout: u32,
    /// read_terminal 工具读取终端 buffer 超时（秒）。
    /// 前端繁忙时回调可能延迟，默认给充足时间。
    #[serde(default = "default_read_terminal_timeout")]
    pub read_terminal_timeout: u32,
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
fn default_command_timeout() -> u32 {
    DEFAULT_COMMAND_TIMEOUT
}
fn default_ask_approval_timeout() -> u32 {
    300
}
fn default_plan_confirm_timeout() -> u32 {
    600
}
fn default_ask_user_timeout() -> u32 {
    300
}
fn default_read_terminal_timeout() -> u32 {
    10
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
    /// 默认角色 ID（用户选择的角色系统提示词）。hai 命令未指定 role 时使用。
    #[serde(default)]
    pub default_role: String,
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
                command_timeout: default_command_timeout(),
                ask_approval_timeout: default_ask_approval_timeout(),
                plan_confirm_timeout: default_plan_confirm_timeout(),
                ask_user_timeout: default_ask_user_timeout(),
                read_terminal_timeout: default_read_terminal_timeout(),
            },
            roles: HashMap::new(),
            default_role: String::new(),
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
