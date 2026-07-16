//! AgentSession — per-tab Agent 会话：消息历史 + 请求循环。
//! Session 内消息串行处理：正在请求 LLM 时新消息排队，abort 在检查点退出。

use std::sync::Arc;

use tauri::ipc::Channel;
use tokio::sync::{mpsc, watch};

use crate::agent::config::{AiConfig, Endpoint};
use crate::agent::openai::OpenAiProvider;
use crate::agent::protocol::{emit, AgentEvent};
use crate::agent::provider::{LlmProvider, Message};
use crate::error::Result;

/// Session 控制命令（从 Tauri command → session task）
pub enum SessionCmd {
    /// 用户发送消息
    Message(String),
    /// 中止当前 LLM 请求
    Abort,
}

/// 默认系统提示词（Phase 1a 简化版，后续从角色模板加载）
fn build_system_prompt() -> String {
    "你是 HetuShell 的 AI 助手，运行在用户终端环境中。\
     你可以帮助用户分析代码、执行命令、编辑文件。\
     请用简洁的中文回复，代码块使用正确的语言标识。"
        .into()
}

/// Session 循环：串行处理用户消息，每次调 LLM 后推送事件。
pub async fn session_loop(
    mut rx: mpsc::UnboundedReceiver<SessionCmd>,
    event_tx: Channel<AgentEvent>,
    config: AiConfig,
    role: String,
) {
    let mut history: Vec<Message> = Vec::new();
    let system_prompt = build_system_prompt();

    // abort 信号：watch channel，abort 命令置 true，LLM 请求中检查
    let (abort_tx, abort_rx) = watch::channel(false);

    loop {
        match rx.recv().await {
            Some(SessionCmd::Message(text)) => {
                if text.trim().is_empty() {
                    continue;
                }

                history.push(Message::user(text));

                // 解析角色 → provider → endpoint
                let (provider_type, model) = match config.resolve_model(&role) {
                    Ok(v) => v,
                    Err(e) => {
                        emit(&event_tx, AgentEvent::Error { message: e.to_string() });
                        emit(&event_tx, AgentEvent::Done);
                        continue;
                    }
                };

                let endpoint: &Endpoint = match config.get_endpoint(provider_type, model) {
                    Ok(e) => e,
                    Err(e) => {
                        emit(&event_tx, AgentEvent::Error { message: e.to_string() });
                        emit(&event_tx, AgentEvent::Done);
                        continue;
                    }
                };

                // Phase 1a: 只支持 openai 兼容 provider
                if provider_type != "openai" {
                    emit(
                        &event_tx,
                        AgentEvent::Error {
                            message: format!(
                                "Phase 1a 暂不支持 provider 类型 '{provider_type}'，仅支持 openai 兼容"
                            ),
                        },
                    );
                    emit(&event_tx, AgentEvent::Done);
                    continue;
                }

                let provider = match OpenAiProvider::from_endpoint(endpoint, model) {
                    Ok(p) => p,
                    Err(e) => {
                        emit(&event_tx, AgentEvent::Error { message: e.to_string() });
                        emit(&event_tx, AgentEvent::Done);
                        continue;
                    }
                };

                // 重置 abort 标志
                let _ = abort_tx.send(false);

                match provider
                    .chat_stream(&history, &system_prompt, &event_tx, &abort_rx)
                    .await
                {
                    Ok(full_content) => {
                        if !full_content.is_empty() {
                            history.push(Message::assistant(full_content));
                        }
                    }
                    Err(e) => {
                        emit(&event_tx, AgentEvent::Error { message: e.to_string() });
                    }
                }

                emit(&event_tx, AgentEvent::Done);
            }
            Some(SessionCmd::Abort) => {
                let _ = abort_tx.send(true);
            }
            None => {
                // 发送端 drop（agent_destroy）→ 退出
                break;
            }
        }
    }
}
