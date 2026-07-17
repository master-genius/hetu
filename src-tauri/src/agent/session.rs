//! AgentSession — per-tab Agent 会话：消息历史 + ReAct 循环。
//! Session 内消息串行处理：正在请求 LLM 时新消息排队，abort 在检查点退出。
//! 每次请求前重新加载 ai-config.json，设置面板保存后即时生效。
//!
//! Phase 1c 新增：
//! - 上下文窗口管理（trim_history）
//! - 负载均衡（round-robin + 429 切 endpoint）
//! - 错误重试（5xx 指数退避 / 超时重试）
//!
//! ReAct 循环：chat_stream → 若有 tool_calls → 逐个执行 → 结果写回 history → 再调 LLM
//! 直到 LLM 不再请求工具调用（最终文本回复），推送 Done。

use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use tauri::ipc::Channel;
use tauri::AppHandle;
use tokio::sync::{mpsc, watch};

use crate::agent::config::{load_config, AiConfig, Endpoint};
use crate::agent::openai::OpenAiProvider;
use crate::agent::protocol::{emit, AgentEvent};
use crate::agent::provider::{LlmProvider, Message, StreamResult};
use crate::agent::tools;
use crate::error::Error;

/// Session 控制命令（从 Tauri command → session task）
pub enum SessionCmd {
    Message(String),
    Abort,
}

// ---------- 上下文窗口管理 ----------

/// 粗估 token 数：ceil(bytes / 3.5)（中英混合经验值）
fn estimate_tokens(messages: &[Message]) -> usize {
    let bytes: usize = messages
        .iter()
        .map(|m| m.content.len() + m.tool_calls.iter().map(|tc| tc.arguments.len() + tc.name.len()).sum::<usize>())
        .sum();
    (bytes + 2) / 3 // ≈ bytes / 3.5，用整数运算
}

/// 上下文截断：超限时优先移除最早的工具调用对，然后移除旧消息。
/// 保留首条用户消息 + 最近 min_recent_turns 轮对话。
fn trim_history(history: &mut Vec<Message>, max_tokens: usize, tx: &Channel<AgentEvent>) {
    let threshold = (max_tokens as f64 * 0.8) as usize;
    if estimate_tokens(history) <= threshold {
        return;
    }

    let mut removed_tools = 0usize;
    let mut removed_messages = 0usize;

    // 保留范围：首条消息 + 末尾 min_recent_turns*2 条（user+assistant 对）
    let min_recent_turns = 5;
    let keep_tail = min_recent_turns * 2;

    loop {
        if estimate_tokens(history) <= threshold || history.len() <= keep_tail + 1 {
            break;
        }

        // 从第 1 条（跳过首条 user）到 len-keep_tail 之间找最早的 assistant+tool 序列
        let search_end = history.len().saturating_sub(keep_tail);
        if search_end <= 1 {
            break;
        }

        // 优先移除 assistant(tool_calls) + tool(result) 对
        let mut found_tool = false;
        let mut i = 1;
        while i < search_end {
            if !history[i].tool_calls.is_empty() {
                // 移除 assistant 消息 + 紧随其后的所有 tool result
                let tool_count = history[i].tool_calls.len();
                let mut removed = 1; // assistant 自身
                history.remove(i);
                // 移除后续连续的 role=tool 消息
                while i < history.len() && history[i].role == "tool" {
                    history.remove(i);
                    removed += 1;
                }
                removed_tools += tool_count;
                removed_messages += removed;
                found_tool = true;
                break;
            }
            i += 1;
        }

        if !found_tool {
            // 没有工具调用了，移除最早的普通消息（第 1 条之后）
            if history.len() > keep_tail + 1 {
                history.remove(1);
                removed_messages += 1;
            } else {
                break;
            }
        }
    }

    if removed_tools > 0 || removed_messages > 0 {
        emit(
            tx,
            AgentEvent::ContextTrimmed {
                removed_tools,
                removed_messages,
            },
        );
    }
}

// ---------- 负载均衡 + 错误重试 ----------

/// 带重试的 chat_stream：429 切 endpoint，5xx 指数退避，超时重试。
async fn chat_with_retry(
    endpoints: &[Endpoint],
    model: &str,
    rr_counter: &AtomicUsize,
    messages: &[Message],
    system_prompt: &str,
    tool_defs: &[crate::agent::provider::ToolDef],
    tx: &Channel<AgentEvent>,
    abort: &watch::Receiver<bool>,
) -> Result<StreamResult, Error> {
    let endpoint_count = endpoints.len();
    let max_retries = 3;
    let max_endpoint_switches = endpoint_count; // 尝试所有 endpoint

    let mut attempt = 0usize;
    let mut endpoint_idx = rr_counter.fetch_add(1, Ordering::Relaxed) % endpoint_count;

    loop {
        if *abort.borrow() {
            return Err(Error::msg("已中止"));
        }

        let endpoint = &endpoints[endpoint_idx];
        let provider = OpenAiProvider::from_endpoint(endpoint, model)?;

        match provider.chat_stream(messages, system_prompt, tool_defs, tx, abort).await {
            Ok(result) => return Ok(result),
            Err(e) => {
                let err_str = e.to_string();

                // 401 密钥无效 — 不重试
                if err_str.contains("401") {
                    return Err(e);
                }

                // 429 限流 — 切换到下一个 endpoint
                if err_str.contains("429") {
                    if attempt < max_endpoint_switches {
                        attempt += 1;
                        endpoint_idx = (endpoint_idx + 1) % endpoint_count;
                        emit(
                            tx,
                            AgentEvent::Retrying {
                                reason: "429 限流，切换 API Key".into(),
                                attempt,
                                max_attempts: max_endpoint_switches,
                            },
                        );
                        continue;
                    }
                    return Err(e);
                }

                // 5xx 服务端错误 — 指数退避
                if err_str.contains("5") && (err_str.contains("LLM API 返回 5") || err_str.contains("502") || err_str.contains("503") || err_str.contains("500")) {
                    if attempt < max_retries {
                        attempt += 1;
                        let delay = 1u64 << (attempt - 1); // 1s, 2s, 4s
                        emit(
                            tx,
                            AgentEvent::Retrying {
                                reason: format!("服务端错误，{delay}s 后重试"),
                                attempt,
                                max_attempts: max_retries,
                            },
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                        continue;
                    }
                    return Err(e);
                }

                // 网络超时 / 连接错误 — 重试 2 次
                if err_str.contains("SSE 读取失败") || err_str.contains("LLM 请求失败") {
                    if attempt < 2 {
                        attempt += 1;
                        emit(
                            tx,
                            AgentEvent::Retrying {
                                reason: "网络错误，重试中".into(),
                                attempt,
                                max_attempts: 2,
                            },
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        continue;
                    }
                    return Err(e);
                }

                // 其他错误 — 不重试
                return Err(e);
            }
        }
    }
}

// ---------- 系统提示词 ----------

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

// ---------- Session 循环 ----------

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

    // round-robin 计数器（per-session，不同消息轮转 endpoint）
    let rr_counter = AtomicUsize::new(0);

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

                let endpoints: &[Endpoint] = match config.get_endpoints(provider_type, model) {
                    Ok(e) => e,
                    Err(e) => {
                        emit(&event_tx, AgentEvent::Error { message: e.to_string() });
                        emit(&event_tx, AgentEvent::Done);
                        continue;
                    }
                };

                let max_tokens = endpoints.first().map(|e| e.max_tokens).unwrap_or(8192);

                // 重置 abort 标志
                let _ = abort_tx.send(false);

                // ReAct 循环：chat → (tool calls?) → execute → chat → ...
                loop {
                    if *abort_rx.borrow() {
                        emit(&event_tx, AgentEvent::Aborted);
                        break;
                    }

                    // 上下文窗口管理：每次调 LLM 前截断
                    trim_history(&mut history, max_tokens as usize, &event_tx);

                    let result: StreamResult = match chat_with_retry(
                        endpoints,
                        model,
                        &rr_counter,
                        &history,
                        &system_prompt,
                        &tool_defs,
                        &event_tx,
                        &abort_rx,
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            let msg = e.to_string();
                            if msg == "已中止" {
                                emit(&event_tx, AgentEvent::Aborted);
                            } else {
                                emit(&event_tx, AgentEvent::Error { message: msg });
                            }
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
