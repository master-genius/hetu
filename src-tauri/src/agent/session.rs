//! AgentSession — per-tab Agent 会话：消息历史 + ReAct 循环。
//! Session 内消息串行处理：正在请求 LLM 时新消息排队，abort 在检查点退出。
//! 每次请求前重新加载 ai-config.json，设置面板保存后即时生效。
//!
//! ReAct 循环：chat_stream → 若有 tool_calls → 逐个执行 → 结果写回 history → 再调 LLM
//! 直到 LLM 不再请求工具调用（最终文本回复），推送 Done。

use serde_json::Value;
use tauri::ipc::Channel;
use tauri::AppHandle;
use tokio::sync::{mpsc, watch};

use crate::agent::config::{load_config, AiConfig, Endpoint};
use crate::agent::openai::OpenAiProvider;
use crate::agent::protocol::{emit, AgentEvent};
use crate::agent::provider::{LlmProvider, Message, StreamResult};
use crate::agent::tools;

/// Session 控制命令（从 Tauri command → session task）
pub enum SessionCmd {
    Message(String),
    Abort,
}

/// 构建系统提示词（Phase 1b：含工具说明 + 工作目录）
fn build_system_prompt(cwd: &str) -> String {
    format!(
        "你是 HetuShell 的 AI 助手，运行在用户终端环境中。\n\
         工作目录: {cwd}\n\n\
         可用工具：\n\
         - read_file: 读取文件内容（超过 500 行截断）\n\
         - write_file: 写入文件\n\
         - list_dir: 列出目录内容\n\
         - run_command: 执行 shell 命令（非交互式）\n\
         - search: 在文件中递归搜索文本\n\
         - file_stat: 获取文件元信息\n\n\
         工作方式：\n\
         1. 理解用户需求\n\
         2. 使用工具收集信息（读文件、执行命令等）\n\
         3. 基于工具结果分析问题\n\
         4. 给出结论或继续使用工具\n\n\
         规则：\n\
         - 执行命令前说明你的意图\n\
         - 工具输出可能被截断，需要时分段读取\n\
         - 如果某个操作失败，尝试不同方案\n\
         - 用简洁的中文回复，代码块使用正确的语言标识"
    )
}

/// Session 循环：串行处理用户消息，ReAct 循环执行工具调用。
pub async fn session_loop(
    mut rx: mpsc::UnboundedReceiver<SessionCmd>,
    event_tx: Channel<AgentEvent>,
    app: AppHandle,
    role: String,
    cwd: String,
) {
    let mut history: Vec<Message> = Vec::new();
    let system_prompt = build_system_prompt(&cwd);
    let tool_defs = tools::definitions();

    let (abort_tx, abort_rx) = watch::channel(false);

    loop {
        match rx.recv().await {
            Some(SessionCmd::Message(text)) => {
                if text.trim().is_empty() {
                    continue;
                }

                history.push(Message::user(text));

                // 每次请求前重新加载 config（设置面板保存后即时生效）
                let config: AiConfig = match load_config(&app) {
                    Ok(c) => c,
                    Err(e) => {
                        emit(&event_tx, AgentEvent::Error { message: e.to_string() });
                        emit(&event_tx, AgentEvent::Done);
                        continue;
                    }
                };

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

                if provider_type != "openai" {
                    emit(
                        &event_tx,
                        AgentEvent::Error {
                            message: format!(
                                "暂不支持 provider 类型 '{provider_type}'，仅支持 openai 兼容"
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

                // ReAct 循环：chat → (tool calls?) → execute → chat → ...
                loop {
                    if *abort_rx.borrow() {
                        emit(&event_tx, AgentEvent::Aborted);
                        break;
                    }

                    let result: StreamResult = match provider
                        .chat_stream(&history, &system_prompt, &tool_defs, &event_tx, &abort_rx)
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            emit(&event_tx, AgentEvent::Error { message: e.to_string() });
                            break;
                        }
                    };

                    // 将 assistant 回复（含可能的 tool_calls）写入历史
                    history.push(Message::assistant_with_tool_calls(
                        result.content,
                        result.tool_calls.clone(),
                    ));

                    if result.tool_calls.is_empty() {
                        // 无工具调用 → 最终回复，轮次结束
                        break;
                    }

                    // 逐个执行工具调用
                    for tc in &result.tool_calls {
                        if *abort_rx.borrow() {
                            emit(&event_tx, AgentEvent::Aborted);
                            break;
                        }

                        let args: Value =
                            serde_json::from_str(&tc.arguments).unwrap_or(Value::Null);

                        emit(
                            &event_tx,
                            AgentEvent::ToolStart {
                                tool: tc.name.clone(),
                                args: args.clone(),
                                target_pane: 0,
                            },
                        );

                        let tool_result = tools::execute(&tc.name, &args, &cwd).await;

                        emit(
                            &event_tx,
                            AgentEvent::ToolEnd {
                                result: tool_result.clone(),
                            },
                        );

                        // 工具结果写入历史（role=tool，tool_call_id 关联）
                        history.push(Message::tool_result(&tc.id, tool_result.to_llm_text()));
                    }

                    // 循环回到 chat_stream → LLM 看到工具结果后继续推理
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
