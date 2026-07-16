//! OpenAI 兼容 Provider — 支持 DeepSeek/OpenAI/GLM 等兼容 /v1/chat/completions 的服务。
//! SSE 流式解析：按行读取 data: {...}，提取 delta.content 推送给前端。

use std::collections::VecDeque;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tokio::sync::watch;

use crate::agent::protocol::{emit, AgentEvent};
use crate::agent::provider::{LlmProvider, Message};
use crate::error::{Error, Result};

pub struct OpenAiProvider {
    url: String,
    key: String,
    model: String,
    max_tokens: u32,
    temperature: f32,
}

impl OpenAiProvider {
    pub fn new(url: String, key: String, model: String, max_tokens: u32, temperature: f32) -> Self {
        Self {
            url,
            key,
            model,
            max_tokens,
            temperature,
        }
    }
}

/// OpenAI chat/completions 请求体
#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    stream: bool,
    max_tokens: u32,
    temperature: f32,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

/// SSE 行解析用的部分 JSON 结构
#[derive(Deserialize)]
struct StreamChunk {
    choices: Vec<StreamChoice>,
}

#[derive(Deserialize)]
struct StreamChoice {
    #[serde(default)]
    delta: StreamDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn chat_stream(
        &self,
        messages: &[Message],
        system_prompt: &str,
        tx: &Channel<AgentEvent>,
        abort: &watch::Receiver<bool>,
    ) -> Result<String> {
        // 构建请求消息：system prompt + history
        let mut api_messages = vec![ChatMessage {
            role: "system",
            content: system_prompt,
        }];
        for msg in messages {
            api_messages.push(ChatMessage {
                role: &msg.role,
                content: &msg.content,
            });
        }

        let body = ChatRequest {
            model: &self.model,
            messages: api_messages,
            stream: true,
            max_tokens: self.max_tokens,
            temperature: self.temperature,
        };

        let endpoint = format!("{}/chat/completions", self.url.trim_end_matches('/'));

        let client = reqwest::Client::new();
        let resp = client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {}", self.key))
            .header("Accept", "text/event-stream")
            .json(&body)
            .timeout(std::time::Duration::from_secs(120))
            .send()
            .await
            .map_err(|e| Error::msg(format!("LLM 请求失败: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            // 401 密钥无效、429 限流等
            return Err(Error::msg(format!(
                "LLM API 返回 {status}: {}",
                text.chars().take(500).collect::<String>()
            )));
        }

        // SSE 流式解析：reqwest bytes_stream → 按行分割 → 解析 data: 行
        let mut stream = resp.bytes_stream();
        let mut line_buf = String::new(); // 跨 chunk 的不完整行缓冲
        let mut full_content = String::new(); // 累积完整回复（用于返回 session 写入 history）

        while let Some(chunk_result) = stream.next().await {
            // abort 检查点
            if *abort.borrow() {
                emit(tx, AgentEvent::Aborted);
                return Ok(full_content);
            }

            let chunk = chunk_result.map_err(|e| Error::msg(format!("SSE 读取失败: {e}")))?;
            line_buf.push_str(&String::from_utf8_lossy(&chunk));

            // 按行处理：先分割出完整行，保留最后不完整行
            let (complete_lines, remaining) = split_lines(&line_buf);
            line_buf = remaining;

            for line in complete_lines {
                let line = line.trim();
                if line.is_empty() || line.starts_with(':') {
                    continue;
                }
                if !line.starts_with("data: ") {
                    continue;
                }
                let data = &line[6..];

                if data.trim() == "[DONE]" {
                    // 流结束
                    emit(
                        tx,
                        AgentEvent::Message {
                            content: String::new(),
                            done: true,
                        },
                    );
                    return Ok(full_content);
                }

                // 解析 JSON
                match serde_json::from_str::<StreamChunk>(data) {
                    Ok(chunk) => {
                        for choice in &chunk.choices {
                            if let Some(content) = &choice.delta.content {
                                if !content.is_empty() {
                                    full_content.push_str(content);
                                    emit(
                                        tx,
                                        AgentEvent::Message {
                                            content: content.clone(),
                                            done: false,
                                        },
                                    );
                                }
                            }
                            // finish_reason=stop 在 [DONE] 之前可能出现，不单独处理
                        }
                    }
                    Err(_) => {
                        // 跳过无法解析的行（心跳、注释等）
                    }
                }
            }
        }

        // 流自然结束（可能没有 [DONE]，某些实现直接关闭流）
        emit(
            tx,
            AgentEvent::Message {
                content: String::new(),
                done: true,
            },
        );

        Ok(full_content)
    }
}

impl OpenAiProvider {
    /// 从 endpoint 配置构建 provider
    pub fn from_endpoint(
        endpoint: &crate::agent::config::Endpoint,
        model: &str,
    ) -> Result<Self> {
        let url = endpoint
            .url
            .as_deref()
            .ok_or_else(|| Error::msg("OpenAI 兼容 provider 需要配置 url"))?;
        Ok(OpenAiProvider::new(
            url.to_string(),
            endpoint.key.clone(),
            model.to_string(),
            endpoint.max_tokens,
            endpoint.temperature,
        ))
    }
}
