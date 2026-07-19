//! LlmProvider trait — 抽象不同 LLM 后端的流式对话接口。
//! Phase 1b 支持 tool calling。新 provider 通过实现此 trait 扩展。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tokio::sync::watch;

use crate::agent::protocol::AgentEvent;
use crate::error::Result;

/// 对话历史中的单条消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool_call_id: String,
}

/// LLM 发起的工具调用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// 传递给 LLM 的工具定义（OpenAI function calling 格式）
#[derive(Debug, Clone, Serialize)]
pub struct ToolDef {
    #[serde(rename = "type")]
    pub def_type: String,
    pub function: ToolFunction,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// chat_stream 的返回结果
pub struct StreamResult {
    /// LLM 回复的文本内容
    pub content: String,
    /// LLM 请求调用的工具（可能为空）
    pub tool_calls: Vec<ToolCall>,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: String::new(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: String::new(),
        }
    }

    pub fn assistant_with_tool_calls(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
            tool_calls,
            tool_call_id: String::new(),
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".into(),
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: tool_call_id.into(),
        }
    }
}

/// LLM Provider 抽象层。
/// chat_stream 将文本增量通过 Channel 推送给前端，
/// 返回 StreamResult 包含完整回复文本 + 工具调用列表。
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn chat_stream(
        &self,
        messages: &[Message],
        system_prompt: &str,
        tools: &[ToolDef],
        tx: &Channel<AgentEvent>,
        abort: &mut watch::Receiver<bool>,
    ) -> Result<StreamResult>;
}
