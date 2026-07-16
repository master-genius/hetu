//! LlmProvider trait — 抽象不同 LLM 后端的流式对话接口。
//! Phase 1a 只有 OpenAiProvider 实装，Phase 4 加 AnthropicProvider。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;

use crate::agent::protocol::AgentEvent;
use crate::error::Result;

/// 对话历史中的单条消息。Phase 1a 只有 user/assistant 两种角色。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
        }
    }
}

/// LLM Provider 抽象层。
/// chat_stream 将文本增量通过 Channel 推送给前端，
/// 返回 Ok(content) 包含完整回复文本（用于写入对话历史）。
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn chat_stream(
        &self,
        messages: &[Message],
        system_prompt: &str,
        tx: &Channel<AgentEvent>,
        abort: &tokio::sync::watch::Receiver<bool>,
    ) -> Result<String>;
}
