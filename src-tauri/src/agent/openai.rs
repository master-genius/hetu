//! OpenAI 兼容 Provider — 支持 DeepSeek/OpenAI/GLM 等兼容 /v1/chat/completions 的服务。
//! SSE 流式解析：按行读取 data: {...}，提取 delta.content 和 delta.tool_calls。
//! tool_calls 按 index 累积 arguments 片段，流结束时合并为完整 ToolCall 列表。

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;
use tauri::ipc::Channel;
use tokio::sync::watch;

use crate::agent::protocol::{emit, AgentEvent};
use crate::agent::provider::{LlmProvider, Message, StreamResult, ToolCall, ToolDef};
use crate::error::{Error, Result};

/// 全局复用 reqwest::Client（连接池共享，避免每次请求新建）
fn shared_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| reqwest::Client::new())
}

pub struct OpenAiProvider {
    url: String,
    key: String,
    model: String,
    max_tokens: u32,
    temperature: f32,
    top_p: Option<f32>,
    frequency_penalty: Option<f32>,
    presence_penalty: Option<f32>,
    stop: Option<Vec<String>>,
    seed: Option<u64>,
    headers: Vec<(String, String)>,
    extra_body: Vec<(String, serde_json::Value)>,
    /// HTTP 请求超时（秒）
    request_timeout: u64,
    /// SSE chunk 间隔超时（秒）
    stream_chunk_timeout: u64,
}

impl OpenAiProvider {
    pub fn new(url: String, key: String, model: String, max_tokens: u32, temperature: f32) -> Self {
        Self {
            url, key, model, max_tokens, temperature,
            top_p: None, frequency_penalty: None, presence_penalty: None,
            stop: None, seed: None,
            headers: Vec::new(), extra_body: Vec::new(),
            request_timeout: crate::agent::config::DEFAULT_REQUEST_TIMEOUT,
            stream_chunk_timeout: crate::agent::config::DEFAULT_STREAM_CHUNK_TIMEOUT,
        }
    }
}

// ---------- 请求序列化 ----------

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    stream: bool,
    max_tokens: u32,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frequency_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<&'a ToolDef>>,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    /// 纯文本时为 string，多模态时为 content parts 数组
    content: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ChatToolCall<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
}

#[derive(Serialize)]
struct ChatToolCall<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    call_type: &'a str,
    function: ChatToolFunction<'a>,
}

#[derive(Serialize)]
struct ChatToolFunction<'a> {
    name: &'a str,
    arguments: &'a str,
}

fn to_chat_message(msg: &Message) -> ChatMessage<'_> {
    let content: serde_json::Value = if !msg.attachments.is_empty() {
        // 多模态：构建 content parts 数组
        let mut parts: Vec<serde_json::Value> = Vec::new();
        if !msg.content.is_empty() {
            parts.push(serde_json::json!({ "type": "text", "text": msg.content }));
        }
        for att in &msg.attachments {
            let data_url = format!("data:{};base64,{}", att.mime_type, att.data);
            parts.push(serde_json::json!({
                "type": "image_url",
                "image_url": { "url": data_url, "detail": "auto" }
            }));
        }
        serde_json::Value::Array(parts)
    } else if msg.content.is_empty() && !msg.tool_calls.is_empty() {
        // 纯工具调用（无文本）：content 为 null
        serde_json::Value::Null
    } else {
        // 纯文本
        serde_json::Value::String(msg.content.clone())
    };

    let tool_calls = if msg.tool_calls.is_empty() {
        None
    } else {
        Some(msg.tool_calls.iter().map(|tc| ChatToolCall {
            id: &tc.id,
            call_type: "function",
            function: ChatToolFunction {
                name: &tc.name,
                arguments: &tc.arguments,
            },
        }).collect())
    };
    let tool_call_id = if msg.tool_call_id.is_empty() {
        None
    } else {
        Some(msg.tool_call_id.as_str())
    };
    ChatMessage { role: &msg.role, content, tool_calls, tool_call_id }
}

// ---------- SSE 响应解析 ----------

#[derive(Deserialize)]
struct StreamChunk {
    choices: Vec<StreamChoice>,
}

#[derive(Deserialize)]
struct StreamChoice {
    #[serde(default)]
    delta: StreamDelta,
    #[serde(default)]
    #[allow(dead_code)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct StreamDelta {
    content: Option<String>,
    tool_calls: Vec<StreamToolCall>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct StreamToolCall {
    index: usize,
    id: Option<String>,
    function: StreamToolFunction,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct StreamToolFunction {
    name: Option<String>,
    arguments: Option<String>,
}

/// 流式 tool_calls 累积器
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments: String,
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn chat_stream(
        &self,
        messages: &[Message],
        system_prompt: &str,
        tools: &[ToolDef],
        tx: &Channel<AgentEvent>,
        abort: &mut watch::Receiver<bool>,
    ) -> Result<StreamResult> {
        // 构建请求消息：system prompt + history
        let mut api_messages = vec![ChatMessage {
            role: "system",
            content: serde_json::Value::String(system_prompt.to_string()),
            tool_calls: None,
            tool_call_id: None,
        }];
        for msg in messages {
            api_messages.push(to_chat_message(msg));
        }

        let tool_refs: Vec<&ToolDef> = tools.iter().collect();

        let body = ChatRequest {
            model: &self.model,
            messages: api_messages,
            stream: true,
            max_tokens: self.max_tokens,
            temperature: self.temperature,
            top_p: self.top_p,
            frequency_penalty: self.frequency_penalty,
            presence_penalty: self.presence_penalty,
            stop: self.stop.clone(),
            seed: self.seed,
            tools: if tool_refs.is_empty() { None } else { Some(tool_refs) },
        };

        // 合并 extra_body（非标准 OpenAI body 参数透传）
        let mut body_val = serde_json::to_value(&body)
            .map_err(|e| Error::msg(format!("序列化请求失败: {e}")))?;
        if !self.extra_body.is_empty() {
            if let Some(obj) = body_val.as_object_mut() {
                for (k, v) in &self.extra_body {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }

        let endpoint = format!("{}/chat/completions", self.url.trim_end_matches('/'));

        let client = shared_client();
        let mut req = client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {}", self.key))
            .header("Accept", "text/event-stream");

        // 自定义 HTTP 头
        for (k, v) in &self.headers {
            req = req.header(k, v);
        }

        let resp = req
            .json(&body_val)
            .timeout(std::time::Duration::from_secs(self.request_timeout))
            .send()
            .await
            .map_err(|e| Error::msg(format!("LLM 请求失败: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::msg(format!(
                "LLM API 返回 {status}: {}",
                text.chars().take(500).collect::<String>()
            )));
        }

        let mut stream = resp.bytes_stream();
        let mut line_buf = String::new();
        let mut full_content = String::new();
        let mut tool_acc: HashMap<usize, ToolCallAccumulator> = HashMap::new();

        let chunk_timeout = std::time::Duration::from_secs(self.stream_chunk_timeout);

        loop {
            // abort 检查 + 等待下一个 chunk（带超时），三者竞争
            tokio::select! {
                // abort 信号：立即中止
                _ = abort.changed() => {
                    emit(tx, AgentEvent::Aborted);
                    return Ok(StreamResult {
                        content: full_content,
                        tool_calls: Vec::new(),
                    });
                }
                // chunk 间隔超时：判定连接挂起
                _ = tokio::time::sleep(chunk_timeout) => {
                    return Err(Error::msg(format!(
                        "SSE 流式读取超时（{}s 无数据），可能连接已挂起",
                        self.stream_chunk_timeout
                    )));
                }
                // 正常收到 chunk
                chunk_result = stream.next() => {
                    let chunk_result = match chunk_result {
                        Some(r) => r,
                        None => break, // 流结束
                    };

                    let chunk = chunk_result.map_err(|e| Error::msg(format!("SSE 读取失败: {e}")))?;
                    line_buf.push_str(&String::from_utf8_lossy(&chunk));
                }
            }

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
                    emit(tx, AgentEvent::Message { content: String::new(), done: true });
                    let tool_calls = finalize_tool_calls(&mut tool_acc);
                    return Ok(StreamResult { content: full_content, tool_calls });
                }

                match serde_json::from_str::<StreamChunk>(data) {
                    Ok(chunk) => {
                        for choice in &chunk.choices {
                            // 文本内容
                            if let Some(content) = &choice.delta.content {
                                if !content.is_empty() {
                                    full_content.push_str(content);
                                    emit(tx, AgentEvent::Message {
                                        content: content.clone(),
                                        done: false,
                                    });
                                }
                            }
                            // 工具调用（流式 fragments）
                            for tc in &choice.delta.tool_calls {
                                let acc = tool_acc.entry(tc.index).or_insert(ToolCallAccumulator {
                                    id: String::new(),
                                    name: String::new(),
                                    arguments: String::new(),
                                });
                                if let Some(id) = &tc.id {
                                    acc.id = id.clone();
                                }
                                if let Some(name) = &tc.function.name {
                                    acc.name = name.clone();
                                }
                                if let Some(args) = &tc.function.arguments {
                                    acc.arguments.push_str(args);
                                }
                            }
                        }
                    }
                    Err(_) => { /* 跳过无法解析的行（心跳、注释等） */ }
                }
            }
        }

        // 流自然结束（可能没有 [DONE]，某些实现直接关闭流）
        emit(tx, AgentEvent::Message { content: String::new(), done: true });
        let tool_calls = finalize_tool_calls(&mut tool_acc);
        Ok(StreamResult { content: full_content, tool_calls })
    }
}

/// 将累积器转为排序后的 ToolCall 列表
fn finalize_tool_calls(acc: &mut HashMap<usize, ToolCallAccumulator>) -> Vec<ToolCall> {
    let mut items: Vec<(usize, ToolCallAccumulator)> = acc.drain().collect();
    items.sort_by_key(|(i, _)| *i);
    items.into_iter().map(|(_, a)| ToolCall {
        id: a.id,
        name: a.name,
        arguments: a.arguments,
    }).collect()
}

impl OpenAiProvider {
    /// 从 endpoint 配置构建 provider。
    /// model_id: 实际发给 API 的 model 参数（由 config.get_model_id 解析）。
    pub fn from_endpoint(endpoint: &crate::agent::config::Endpoint, model_id: &str) -> Result<Self> {
        let url = endpoint
            .url
            .as_deref()
            .ok_or_else(|| Error::msg("OpenAI 兼容 provider 需要配置 url"))?;
        let opts = &endpoint.options;
        let mut provider = OpenAiProvider::new(
            url.to_string(),
            endpoint.key.clone(),
            model_id.to_string(),
            opts.max_tokens,
            opts.temperature,
        );
        provider.top_p = opts.top_p;
        provider.frequency_penalty = opts.frequency_penalty;
        provider.presence_penalty = opts.presence_penalty;
        provider.stop = opts.stop.clone();
        provider.seed = opts.seed;
        provider.request_timeout = opts.request_timeout
            .map(|s| s as u64)
            .unwrap_or(crate::agent::config::DEFAULT_REQUEST_TIMEOUT);
        provider.stream_chunk_timeout = opts.stream_chunk_timeout
            .map(|s| s as u64)
            .unwrap_or(crate::agent::config::DEFAULT_STREAM_CHUNK_TIMEOUT);
        if let Some(ref h) = endpoint.headers {
            provider.headers = h.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        }
        if let Some(ref b) = endpoint.body {
            provider.extra_body = b.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        }
        Ok(provider)
    }
}

/// 将缓冲区按行分割：返回 (完整行列表, 剩余不完整部分)。
fn split_lines(buf: &str) -> (Vec<String>, String) {
    let mut complete = Vec::new();
    let mut remaining = String::new();
    for line in buf.split_inclusive('\n') {
        if line.ends_with('\n') {
            complete.push(line.trim_end_matches('\n').to_string());
        } else {
            remaining = line.to_string();
        }
    }
    (complete, remaining)
}
