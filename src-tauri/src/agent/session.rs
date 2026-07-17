//! AgentSession — per-tab Agent 会话：消息历史 + ReAct 循环。
//!
//! Phase 2 新增：
//! - Ask 模式：工具执行前等待用户确认
//! - Plan 模式：LLM 先出计划（文字），用户确认后执行（Phase 2 简化为 Ask 模式变体）
//! - ask_user 工具：Agent 向用户提问
//! - read_terminal 工具：读 xterm buffer（前后端往返）
//! - list_panes 工具：列出 Tab 内所有 Pane
//! - target_pane 路由：根据 pane 类型选择本地/远程执行（Phase 3 实装远程，Phase 2 仅本地）

use serde_json::Value;
use tauri::ipc::Channel;
use tauri::AppHandle;
use tokio::sync::{mpsc, oneshot, watch};

use crate::agent::config::{load_config, AiConfig, Endpoint};
use crate::agent::openai::OpenAiProvider;
use crate::agent::protocol::{emit, AgentEvent, PaneInfo, ToolResult, UserChoice};
use crate::agent::provider::{LlmProvider, Message, StreamResult};
use crate::agent::tools;
use crate::agent::SessionState;
use crate::error::Error;

/// Session 控制命令
pub enum SessionCmd {
    Message(String),
    Abort,
}

// ---------- 上下文窗口管理 ----------

fn estimate_tokens(messages: &[Message]) -> usize {
    let bytes: usize = messages
        .iter()
        .map(|m| {
            m.content.len()
                + m.tool_calls.iter().map(|tc| tc.arguments.len() + tc.name.len()).sum::<usize>()
        })
        .sum();
    (bytes + 2) / 3
}

fn trim_history(history: &mut Vec<Message>, max_tokens: usize, tx: &Channel<AgentEvent>) {
    let threshold = (max_tokens as f64 * 0.8) as usize;
    if estimate_tokens(history) <= threshold {
        return;
    }

    let mut removed_tools = 0usize;
    let mut removed_messages = 0usize;
    let min_recent_turns = 5;
    let keep_tail = min_recent_turns * 2;

    loop {
        if estimate_tokens(history) <= threshold || history.len() <= keep_tail + 1 {
            break;
        }

        let search_end = history.len().saturating_sub(keep_tail);
        if search_end <= 1 {
            break;
        }

        let mut found_tool = false;
        let mut i = 1;
        while i < search_end {
            if !history[i].tool_calls.is_empty() {
                let tool_count = history[i].tool_calls.len();
                let mut removed = 1;
                history.remove(i);
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

// ---------- 加权轮转 ----------

/// 加权 round-robin：weight=N 的 endpoint 连续用 N 次，然后轮转到下一个。
/// weight=0 的 endpoint 跳过（禁用）。默认 weight=1。
struct WeightedRR {
    idx: usize,
    remaining: u32,
}

impl WeightedRR {
    fn new() -> Self {
        Self { idx: 0, remaining: 0 }
    }

    /// 选出下一个 endpoint 索引。连续用 weight 次后轮转。
    fn next(&mut self, endpoints: &[Endpoint]) -> usize {
        let len = endpoints.len();
        if len == 0 {
            return 0;
        }
        // 当前 endpoint 还有剩余配额 → 继续用它
        if self.remaining > 0 {
            self.remaining -= 1;
            return self.idx;
        }
        // 配额用完（或初始），找下一个 weight>0 的 endpoint
        for i in 0..len {
            let idx = (self.idx + 1 + i) % len;
            let w = endpoints[idx].weight.unwrap_or(1);
            if w > 0 {
                self.idx = idx;
                self.remaining = w - 1; // 本次用掉 1 次
                return idx;
            }
        }
        // 全部 weight=0 → fallback 到第一个
        self.idx = 0;
        self.remaining = 0;
        0
    }

    /// 强制轮转到下一个（429 限流时调用）
    fn force_rotate(&mut self, endpoints: &[Endpoint]) {
        self.remaining = 0;
        // next() 会自动找下一个 weight>0 的
        self.next(endpoints);
    }
}

// ---------- 负载均衡 + 错误重试 ----------

async fn chat_with_retry(
    endpoints: &[Endpoint],
    model: &str,
    wrr: &mut WeightedRR,
    messages: &[Message],
    system_prompt: &str,
    tool_defs: &[crate::agent::provider::ToolDef],
    tx: &Channel<AgentEvent>,
    abort: &watch::Receiver<bool>,
) -> Result<StreamResult, Error> {
    let endpoint_count = endpoints.len();
    let max_retries = 3;
    let max_endpoint_switches = endpoint_count;

    let mut attempt = 0usize;
    let mut endpoint_idx = wrr.next(endpoints);

    loop {
        if *abort.borrow() {
            return Err(Error::msg("已中止"));
        }

        let endpoint = &endpoints[endpoint_idx];
        let model_id = AiConfig::get_model_id(endpoint, model);
        let provider = OpenAiProvider::from_endpoint(endpoint, model_id)?;

        match provider.chat_stream(messages, system_prompt, tool_defs, tx, abort).await {
            Ok(result) => return Ok(result),
            Err(e) => {
                let err_str = e.to_string();

                if err_str.contains("401") {
                    return Err(e);
                }

                if err_str.contains("429") {
                    if attempt < max_endpoint_switches {
                        attempt += 1;
                        wrr.force_rotate(endpoints);
                        endpoint_idx = wrr.idx;
                        emit(tx, AgentEvent::Retrying {
                            reason: "429 限流，切换 API Key".into(),
                            attempt,
                            max_attempts: max_endpoint_switches,
                        });
                        continue;
                    }
                    return Err(e);
                }

                if err_str.contains("LLM API 返回 5")
                    || err_str.contains("502") || err_str.contains("503") || err_str.contains("500")
                {
                    if attempt < max_retries {
                        attempt += 1;
                        let delay = 1u64 << (attempt - 1);
                        emit(tx, AgentEvent::Retrying {
                            reason: format!("服务端错误，{delay}s 后重试"),
                            attempt,
                            max_attempts: max_retries,
                        });
                        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                        continue;
                    }
                    return Err(e);
                }

                if err_str.contains("SSE 读取失败") || err_str.contains("LLM 请求失败") {
                    if attempt < 2 {
                        attempt += 1;
                        emit(tx, AgentEvent::Retrying {
                            reason: "网络错误，重试中".into(),
                            attempt,
                            max_attempts: 2,
                        });
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        continue;
                    }
                    return Err(e);
                }

                return Err(e);
            }
        }
    }
}

// ---------- 系统提示词 ----------

fn build_system_prompt(cwd: &str, panes: &[PaneInfo]) -> String {
    let pane_table = if panes.is_empty() {
        format!("| 0 | 本地 | localhost | {} | Linux |", cwd)
    } else {
        panes
            .iter()
            .enumerate()
            .map(|(i, p)| {
                format!(
                    "| {} | {} | {} | {} | {} |",
                    i,
                    if p.is_local { "本地" } else { "SSH" },
                    p.host,
                    p.cwd,
                    p.os,
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "你是 HetuShell 的 AI 助手，运行在用户终端环境中。\n\n\
         ## 当前 Tab 的 Pane 列表\n\
         | # | 类型 | 主机 | 当前目录 | 操作系统 |\n\
         |---|------|------|----------|----------|\n\
         {pane_table}\n\n\
         默认在 Pane 0（你被调用的那个 Pane）上执行操作。\n\
         使用 list_panes 查看所有可用 Pane。\n\
         使用工具时指定 target_pane 来选择在哪个 Pane 上执行。\n\n\
         可用工具：\n\
         - read_file: 读取文件内容（超过 500 行截断）\n\
         - write_file: 写入文件\n\
         - list_dir: 列出目录内容\n\
         - run_command: 执行 shell 命令（非交互式）\n\
         - search: 在文件中递归搜索文本\n\
         - file_stat: 获取文件元信息\n\
         - list_panes: 列出 Tab 内所有 Pane\n\
         - read_terminal: 读取终端可见内容\n\
         - ask_user: 向用户提问（方向性歧义时使用）\n\n\
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

pub async fn session_loop(
    mut rx: mpsc::UnboundedReceiver<SessionCmd>,
    event_tx: Channel<AgentEvent>,
    app: AppHandle,
    role: String,
    cwd: String,
    mode: String,
    state: SessionState,
    mut abort_rx: watch::Receiver<bool>,
) {
    let mut history: Vec<Message> = Vec::new();
    let tool_defs = tools::definitions();
    let mut wrr = WeightedRR::new();

    loop {
        match rx.recv().await {
            Some(SessionCmd::Message(text)) => {
                if text.trim().is_empty() {
                    continue;
                }

                history.push(Message::user(text));

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
                    emit(&event_tx, AgentEvent::Error {
                        message: format!("暂不支持 provider 类型 '{provider_type}'，仅支持 openai 兼容"),
                    });
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

                let max_tokens = endpoints.first().map(|e| e.options.max_tokens).unwrap_or(8192);

                let _ = state.abort_tx.send(false);

                // ReAct 循环
                loop {
                    if *abort_rx.borrow() {
                        emit(&event_tx, AgentEvent::Aborted);
                        break;
                    }

                    trim_history(&mut history, max_tokens as usize, &event_tx);

                    let panes = state.panes.lock().await.clone();
                    let system_prompt = build_system_prompt(&cwd, &panes);

                    let result: StreamResult = match chat_with_retry(
                        endpoints,
                        model,
                        &mut wrr,
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

                    history.push(Message::assistant_with_tool_calls(
                        result.content,
                        result.tool_calls.clone(),
                    ));

                    if result.tool_calls.is_empty() {
                        break;
                    }

                    // 处理工具调用
                    let mut abort_tools = false;
                    for tc in &result.tool_calls {
                        if *abort_rx.borrow() {
                            emit(&event_tx, AgentEvent::Aborted);
                            abort_tools = true;
                            break;
                        }

                        let args: Value =
                            serde_json::from_str(&tc.arguments).unwrap_or(Value::Null);

                        // --- 特殊工具 ---

                        if tc.name == "ask_user" {
                            let tool_result = handle_ask_user(&args, &state, &event_tx, &abort_rx).await;
                            history.push(Message::tool_result(&tc.id, tool_result.to_llm_text()));
                            emit(&event_tx, AgentEvent::ToolEnd { result: tool_result });
                            continue;
                        }

                        if tc.name == "list_panes" {
                            let panes = state.panes.lock().await;
                            let output = format_panes(&panes);
                            let result = ToolResult::Success { output, truncated: false };
                            emit(&event_tx, AgentEvent::ToolStart {
                                tool: tc.name.clone(),
                                args: args.clone(),
                                target_pane: 0,
                            });
                            emit(&event_tx, AgentEvent::ToolEnd { result: result.clone() });
                            history.push(Message::tool_result(&tc.id, result.to_llm_text()));
                            continue;
                        }

                        if tc.name == "read_terminal" {
                            let target_pane = args.get("target_pane").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                            let lines = args.get("lines").and_then(|v| v.as_u64()).unwrap_or(100) as usize;
                            let panes = state.panes.lock().await;
                            let pane_id = panes.get(target_pane).map(|p| p.id.clone()).unwrap_or_default();
                            drop(panes);

                            emit(&event_tx, AgentEvent::ToolStart {
                                tool: tc.name.clone(),
                                args: args.clone(),
                                target_pane,
                            });

                            let tool_result = handle_read_terminal(
                                &pane_id,
                                lines,
                                &state,
                                &event_tx,
                                &abort_rx,
                            )
                            .await;

                            emit(&event_tx, AgentEvent::ToolEnd { result: tool_result.clone() });
                            history.push(Message::tool_result(&tc.id, tool_result.to_llm_text()));
                            continue;
                        }

                        // --- 普通工具 ---

                        let target_pane = args.get("target_pane").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

                        emit(&event_tx, AgentEvent::ToolStart {
                            tool: tc.name.clone(),
                            args: args.clone(),
                            target_pane,
                        });

                        // Ask 模式 / 危险命令：等待用户确认
                        if mode == "ask" || is_dangerous(&tc.name, &args, &config) {
                            let reason = format!("执行 {} on Pane {}", tc.name, target_pane);
                            emit(&event_tx, AgentEvent::AskApproval {
                                tool: tc.name.clone(),
                                args: args.clone(),
                                target_pane,
                                reason,
                            });

                            let (approve_tx, approve_rx) = oneshot::channel();
                            *state.approve_tx.lock().await = Some(approve_tx);

                            let approved = tokio::select! {
                                r = approve_rx => r.unwrap_or(false),
                                _ = tokio::time::sleep(std::time::Duration::from_secs(300)) => false,
                            };

                            if *abort_rx.borrow() {
                                emit(&event_tx, AgentEvent::Aborted);
                                abort_tools = true;
                                break;
                            }

                            if !approved {
                                let result = ToolResult::UserRejected;
                                emit(&event_tx, AgentEvent::ToolEnd { result: result.clone() });
                                history.push(Message::tool_result(&tc.id, result.to_llm_text()));
                                continue;
                            }
                        }

                        // 获取目标 pane 的 cwd
                        let panes = state.panes.lock().await;
                        let target_cwd = panes.get(target_pane)
                            .map(|p| p.cwd.clone())
                            .filter(|c| !c.is_empty())
                            .unwrap_or_else(|| cwd.clone());
                        drop(panes);

                        let tool_result = tools::execute(&tc.name, &args, &target_cwd).await;

                        emit(&event_tx, AgentEvent::ToolEnd { result: tool_result.clone() });
                        history.push(Message::tool_result(&tc.id, tool_result.to_llm_text()));
                    }

                    if abort_tools {
                        break;
                    }
                }

                emit(&event_tx, AgentEvent::Done);
            }
            Some(SessionCmd::Abort) => {
                let _ = state.abort_tx.send(true);
            }
            None => break,
        }
    }
}

// ---------- 特殊工具处理 ----------

/// ask_user 工具：推送 UserQuestion 事件，等待前端 answer_question
async fn handle_ask_user(
    args: &Value,
    state: &SessionState,
    tx: &Channel<AgentEvent>,
    abort: &watch::Receiver<bool>,
) -> ToolResult {
    let question = args.get("question").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let choices: Vec<UserChoice> = args
        .get("choices")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|c| UserChoice {
                    label: c.get("label").and_then(|v| v.as_str()).unwrap_or("").into(),
                    description: c.get("description").and_then(|v| v.as_str()).unwrap_or("").into(),
                    action: c.get("action").and_then(|v| v.as_str()).unwrap_or("").into(),
                })
                .collect()
        })
        .unwrap_or_default();

    emit(tx, AgentEvent::ToolStart {
        tool: "ask_user".into(),
        args: args.clone(),
        target_pane: 0,
    });

    emit(tx, AgentEvent::UserQuestion { question, choices });

    let (answer_tx, answer_rx) = oneshot::channel();
    *state.answer_tx.lock().await = Some(answer_tx);

    tokio::select! {
        r = answer_rx => {
            match r {
                Ok(answer) => ToolResult::Success { output: answer, truncated: false },
                Err(_) => ToolResult::Error { message: "回答通道关闭".into() },
            }
        }
        _ = tokio::time::sleep(std::time::Duration::from_secs(300)) => {
            ToolResult::Error { message: "提问超时（5分钟）".into() }
        }
    }
}

/// read_terminal 工具：推送 ReadTerminalRequest，等待前端 agent_terminal_data 回调
async fn handle_read_terminal(
    pane_id: &str,
    lines: usize,
    state: &SessionState,
    tx: &Channel<AgentEvent>,
    _abort: &watch::Receiver<bool>,
) -> ToolResult {
    let request_id = format!(
        "rt_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );

    emit(tx, AgentEvent::ReadTerminalRequest {
        request_id,
        pane_id: pane_id.to_string(),
        lines,
    });

    let (term_tx, term_rx) = oneshot::channel();
    *state.terminal_tx.lock().await = Some(term_tx);

    tokio::select! {
        r = term_rx => {
            match r {
                Ok(data) => ToolResult::Success { output: data, truncated: false },
                Err(_) => ToolResult::Error { message: "终端数据通道关闭".into() },
            }
        }
        _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
            ToolResult::Error { message: "读取终端超时（5s）".into() }
        }
    }
}

// ---------- 辅助函数 ----------

fn is_dangerous(tool_name: &str, args: &Value, config: &AiConfig) -> bool {
    if config.execution.always_ask_for.iter().any(|t| t == tool_name) {
        return true;
    }
    if tool_name == "run_command" {
        if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
            return config.execution.dangerous_commands.iter().any(|dc| cmd.contains(dc));
        }
    }
    false
}

fn format_panes(panes: &[PaneInfo]) -> String {
    if panes.is_empty() {
        return "无 Pane 信息".into();
    }
    panes
        .iter()
        .enumerate()
        .map(|(i, p)| {
            format!(
                "Pane {}: {} | {} | {} | {}",
                i,
                if p.is_local { "本地" } else { "SSH" },
                p.host,
                p.cwd,
                p.os,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
