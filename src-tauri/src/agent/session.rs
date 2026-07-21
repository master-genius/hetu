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
use tauri::{AppHandle, Manager};
use tokio::sync::{mpsc, oneshot, watch};

use crate::agent::config::{load_config, AiConfig, Endpoint};
use crate::agent::openai::OpenAiProvider;
use crate::agent::protocol::{emit, AgentEvent, Attachment, HistoryEntry, HistoryIndex, PaneInfo, ToolResult, UserChoice};
use crate::agent::provider::{LlmProvider, Message, StreamResult};
use crate::agent::tools;
use crate::agent::SessionState;
use crate::error::Error;

/// Session 控制命令
pub enum SessionCmd {
    Message { text: String, attachments: Vec<Attachment> },
    #[allow(dead_code)]
    Abort,
    ClearHistory,
    /// 从指定 cwd 加载历史到当前 session（历史恢复/继续对话）
    LoadHistory(String),
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
    abort: &mut watch::Receiver<bool>,
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
                        tokio::select! {
                            _ = tokio::time::sleep(std::time::Duration::from_secs(delay)) => {}
                            _ = abort.changed() => return Err(Error::msg("已中止")),
                        }
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
                        tokio::select! {
                            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
                            _ = abort.changed() => return Err(Error::msg("已中止")),
                        }
                        continue;
                    }
                    return Err(e);
                }

                return Err(e);
            }
        }
    }
}

// ---------- 历史持久化 ----------

fn session_file_path(cwd: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{}/.hetu/ai-session.json", cwd.trim_end_matches('/')))
}

/// 从 {cwd}/.hetu/ai-session.json 加载历史。返回 (history, entries_for_frontend)
fn load_history(cwd: &str) -> (Vec<Message>, Vec<HistoryEntry>) {
    let path = session_file_path(cwd);
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(_) => return (Vec::new(), Vec::new()),
    };
    let history: Vec<Message> = match serde_json::from_str(&data) {
        Ok(h) => h,
        Err(_) => return (Vec::new(), Vec::new()),
    };
    let entries: Vec<HistoryEntry> = history
        .iter()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .filter(|m| !m.content.is_empty())
        .map(|m| HistoryEntry {
            role: m.role.clone(),
            content: m.content.clone(),
        })
        .collect();
    (history, entries)
}

/// 保存历史到 {cwd}/.hetu/ai-session.json
fn save_history(cwd: &str, history: &[Message]) {
    let path = session_file_path(cwd);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(data) = serde_json::to_string_pretty(history) {
        let _ = std::fs::write(&path, data);
    }
}

/// 删除历史文件
fn clear_history_file(cwd: &str) {
    let path = session_file_path(cwd);
    let _ = std::fs::remove_file(&path);
}

// ---------- 全局历史索引 ----------

fn index_file_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    use tauri::Manager;
    app.path().app_data_dir().ok().map(|d| d.join("ai-sessions.json"))
}

fn load_index(app: &AppHandle) -> Vec<HistoryIndex> {
    let path = match index_file_path(app) {
        Some(p) => p,
        None => return Vec::new(),
    };
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    serde_json::from_str(&data).unwrap_or_default()
}

fn save_index(app: &AppHandle, index: &[HistoryIndex]) {
    let path = match index_file_path(app) {
        Some(p) => p,
        None => return,
    };
    if let Ok(data) = serde_json::to_string_pretty(index) {
        let _ = std::fs::write(&path, data);
    }
}

/// 更新索引条目（同 cwd 覆盖）。在 save_history 后调用。
pub fn update_index_entry(app: &AppHandle, cwd: &str, preview: &str, role: &str, model: &str) {
    if cwd.is_empty() {
        return;
    }
    let mut index = load_index(app);
    index.retain(|e| e.cwd != cwd);
    index.push(HistoryIndex {
        cwd: cwd.to_string(),
        last_active: now_iso(),
        preview: preview.chars().take(100).collect(),
        role: role.to_string(),
        model: model.to_string(),
        dir_exists: true,
    });
    save_index(app, &index);
}

/// 列出全局历史索引，按 last_active 降序。pattern 模糊匹配 cwd。
pub fn list_history(app: &AppHandle, pattern: Option<&str>) -> Vec<HistoryIndex> {
    let mut index = load_index(app);
    if let Some(p) = pattern {
        if !p.is_empty() {
            index.retain(|e| e.cwd.contains(p));
        }
    }
    // 标记目录是否存在
    for entry in &mut index {
        entry.dir_exists = std::path::Path::new(&entry.cwd).exists();
    }
    index.sort_by(|a, b| b.last_active.cmp(&a.last_active));
    index
}

/// 删除指定目录的历史（session 文件 + 索引条目）
pub fn delete_history(app: &AppHandle, cwd: &str) {
    let session_path = session_file_path(cwd);
    let _ = std::fs::remove_file(&session_path);
    let mut index = load_index(app);
    index.retain(|e| e.cwd != cwd);
    save_index(app, &index);
}

/// 迁移历史到新目录（复制 session 文件 + 更新索引）
pub fn migrate_history(app: &AppHandle, old_cwd: &str, new_cwd: &str) {
    let old_path = session_file_path(old_cwd);
    let new_path = session_file_path(new_cwd);
    if let Some(parent) = new_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::copy(&old_path, &new_path);
    let mut index = load_index(app);
    if let Some(entry) = index.iter_mut().find(|e| e.cwd == old_cwd) {
        entry.cwd = new_cwd.to_string();
        entry.dir_exists = std::path::Path::new(new_cwd).exists();
    }
    save_index(app, &index);
}

fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = secs / 86400;
    let time = secs % 86400;
    let h = time / 3600;
    let m = (time % 3600) / 60;
    let s = time % 60;
    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
}

/// 将 Unix epoch 天数转换为 (year, month, day)。
/// 使用 Howard Hinnant 的 civil_from_days 算法，正确处理闰年。
fn days_to_ymd(days: u64) -> (u32, u32, u32) {
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y as u32, m as u32, d as u32)
}

// ---------- 系统提示词 ----------

/// 格式化 Pane 列表为 Markdown 表格
fn format_pane_table(panes: &[PaneInfo]) -> String {
    if panes.is_empty() {
        "| 0 | 本地 | localhost | (未知) | Linux |".into()
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
    }
}

/// 从 app_data_dir/roles/{role}.md 加载角色提示词正文，替换占位符。
/// 指定角色不存在时 fallback 到 general；均不存在时返回空串（session_loop 顶部
/// 已 ensure_defaults 写入 general，正常不会走到空串分支）。
fn build_system_prompt(app: &AppHandle, role: &str, cwd: &str, panes: &[PaneInfo]) -> String {
    let pane_table = format_pane_table(panes);

    let template = crate::agent::roles::load_content(app, role)
        .or_else(|| crate::agent::roles::load_content(app, "general"))
        .unwrap_or_default();

    template
        .replace("{cwd}", cwd)
        .replace("{pane_table}", &pane_table)
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
    // 加载持久化历史
    let (mut history, restored_entries) = load_history(&cwd);
    if !restored_entries.is_empty() {
        emit(&event_tx, AgentEvent::HistoryRestored { messages: restored_entries });
    }

    let tool_defs = tools::definitions();
    let mut wrr = WeightedRR::new();

    loop {
        match rx.recv().await {
            Some(SessionCmd::Message { text, attachments }) => {
                if text.trim().is_empty() && attachments.is_empty() {
                    continue;
                }

                history.push(Message::user_with_attachments(text, attachments));

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

                // Plan 模式状态：第一轮不传 tools，LLM 输出文字计划；确认后切换为 auto
                let mut plan_confirmed = mode != "plan";

                // ReAct 循环
                loop {
                    if *abort_rx.borrow() {
                        emit(&event_tx, AgentEvent::Aborted);
                        break;
                    }

                    trim_history(&mut history, max_tokens as usize, &event_tx);

                    let panes = state.panes.lock().await.clone();
                    let system_prompt = build_system_prompt(&app, &role, &cwd, &panes);

                    // Plan 模式未确认时，不传 tools（LLM 只能用文字描述计划）
                    let active_tools: &[crate::agent::provider::ToolDef] = if plan_confirmed { &tool_defs } else { &[] };

                    let result: StreamResult = match chat_with_retry(
                        endpoints,
                        model,
                        &mut wrr,
                        &history,
                        &system_prompt,
                        active_tools,
                        &event_tx,
                        &mut abort_rx,
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

                    let content_clone = result.content.clone();
                    history.push(Message::assistant_with_tool_calls(
                        result.content,
                        result.tool_calls.clone(),
                    ));

                    // Plan 模式：LLM 输出了文字计划（无 tool_calls），推送 ProposedPlan 等待确认
                    if !plan_confirmed && result.tool_calls.is_empty() {
                        emit(&event_tx, AgentEvent::ProposedPlan {
                            summary: content_clone,
                        });

                        let (approve_tx, approve_rx) = oneshot::channel();
                        *state.approve_tx.lock().await = Some(approve_tx);

                        let approved = tokio::select! {
                            r = approve_rx => r.unwrap_or(false),
                            _ = tokio::time::sleep(std::time::Duration::from_secs(config.execution.plan_confirm_timeout as u64)) => false,
                            _ = abort_rx.changed() => false,
                        };

                        if *abort_rx.borrow() {
                            emit(&event_tx, AgentEvent::Aborted);
                            break;
                        }

                        if approved {
                            plan_confirmed = true;
                            // 继续循环，下一轮传 tools，LLM 开始执行
                            continue;
                        } else {
                            // 用户拒绝计划，告知 LLM
                            history.push(Message::user("用户拒绝了该计划，请重新分析或调整方案。"));
                            continue;
                        }
                    }

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
                            let tool_result = handle_ask_user(&args, &state, &event_tx, &mut abort_rx, config.execution.ask_user_timeout).await;
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
                                &mut abort_rx,
                                config.execution.read_terminal_timeout,
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
                                _ = tokio::time::sleep(std::time::Duration::from_secs(config.execution.ask_approval_timeout as u64)) => false,
                                _ = abort_rx.changed() => false,
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

                        // 获取目标 pane 的 cwd + 连接
                        let panes = state.panes.lock().await;
                        let target_pane_info = panes.get(target_pane).cloned();
                        let target_cwd = target_pane_info
                            .as_ref()
                            .map(|p| p.cwd.clone())
                            .filter(|c| !c.is_empty())
                            .unwrap_or_else(|| cwd.clone());
                        drop(panes);

                        // 本地 pane → conn=None；SSH pane → 通过 conn_id 获取 Connection
                        let conn = if let Some(ref pi) = target_pane_info {
                            if pi.is_local || pi.conn_id.is_empty() {
                                None
                            } else {
                                match crate::AppState::get_conn(
                                    &app.state::<crate::AppState>(),
                                    &pi.conn_id,
                                ).await {
                                    Ok(c) => Some(c),
                                    Err(_) => None,
                                }
                            }
                        } else {
                            None
                        };

                        let command_timeout = config.execution.command_timeout;

                        // 用 select! 包裹工具执行，使其可被 abort 中断
                        let tool_result = tokio::select! {
                            r = tools::execute(&tc.name, &args, &target_cwd, conn.as_ref(), command_timeout, &event_tx) => r,
                            _ = abort_rx.changed() => {
                                emit(&event_tx, AgentEvent::Aborted);
                                abort_tools = true;
                                break;
                            }
                        };

                        emit(&event_tx, AgentEvent::ToolEnd { result: tool_result.clone() });
                        history.push(Message::tool_result(&tc.id, tool_result.to_llm_text()));
                    }

                    if abort_tools {
                        break;
                    }
                }

                emit(&event_tx, AgentEvent::Done);
                // 持久化历史 + 更新全局索引
                save_history(&cwd, &history);
                let preview = history.iter()
                    .find(|m| m.role == "user" && !m.content.is_empty())
                    .map(|m| m.content.clone())
                    .unwrap_or_default();
                update_index_entry(&app, &cwd, &preview, &role, model);
            }
            Some(SessionCmd::Abort) => {
                let _ = state.abort_tx.send(true);
            }
            Some(SessionCmd::ClearHistory) => {
                history.clear();
                clear_history_file(&cwd);
                emit(&event_tx, AgentEvent::HistoryCleared);
            }
            Some(SessionCmd::LoadHistory(history_cwd)) => {
                let (new_history, entries) = load_history(&history_cwd);
                history = new_history;
                if !entries.is_empty() {
                    emit(&event_tx, AgentEvent::HistoryRestored { messages: entries });
                }
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
    abort: &mut watch::Receiver<bool>,
    timeout_secs: u32,
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
        _ = tokio::time::sleep(std::time::Duration::from_secs(timeout_secs as u64)) => {
            ToolResult::Error { message: format!("提问超时（{timeout_secs}s）") }
        }
        _ = abort.changed() => {
            ToolResult::Error { message: "已中止".into() }
        }
    }
}

/// read_terminal 工具：推送 ReadTerminalRequest，等待前端 agent_terminal_data 回调
async fn handle_read_terminal(
    pane_id: &str,
    lines: usize,
    state: &SessionState,
    tx: &Channel<AgentEvent>,
    abort: &mut watch::Receiver<bool>,
    timeout_secs: u32,
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
        _ = tokio::time::sleep(std::time::Duration::from_secs(timeout_secs as u64)) => {
            ToolResult::Error { message: format!("读取终端超时（{timeout_secs}s）") }
        }
        _ = abort.changed() => {
            ToolResult::Error { message: "已中止".into() }
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
