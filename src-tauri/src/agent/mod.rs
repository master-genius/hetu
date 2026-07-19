//! AgentManager — 管理 per-tab Agent Session 生命周期。
//! Session 以 tabId 为 key，前端 invoke 时传入。
//!
//! Phase 2 新增：
//! - SessionState（panes + 交互 channel：approve/answer/terminal）
//! - agent_approve_tool / agent_answer_question / agent_terminal_data / agent_update_panes

mod config;
mod openai;
mod provider;
mod protocol;
mod session;
mod tools;
mod tools_ssh;

use std::collections::HashMap;
use std::sync::Arc;

use tauri::ipc::Channel;
use tokio::sync::{mpsc, watch, Mutex};
use tokio::sync::oneshot;

use crate::agent::protocol::{AgentEvent, PaneInfo};
use crate::agent::session::{session_loop, SessionCmd};
use crate::error::Result;

/// Session 运行时状态（在 session_loop 和 Tauri commands 之间共享）
#[derive(Clone)]
pub struct SessionState {
    /// Tab 内 pane 列表（前端通过 agent_update_panes 同步）
    pub panes: Arc<Mutex<Vec<PaneInfo>>>,
    /// Ask 模式：用户确认结果（true=批准 / false=拒绝）
    pub approve_tx: Arc<Mutex<Option<oneshot::Sender<bool>>>>,
    /// AskUser 工具：用户回答
    pub answer_tx: Arc<Mutex<Option<oneshot::Sender<String>>>>,
    /// read_terminal 工具：前端回调的终端文本
    pub terminal_tx: Arc<Mutex<Option<oneshot::Sender<String>>>>,
    /// abort 信号（agent_abort 直接设 true，不需要走 mpsc）
    pub abort_tx: watch::Sender<bool>,
}

impl SessionState {
    fn new() -> Self {
        let (abort_tx, _) = watch::channel(false);
        Self {
            panes: Arc::new(Mutex::new(Vec::new())),
            approve_tx: Arc::new(Mutex::new(None)),
            answer_tx: Arc::new(Mutex::new(None)),
            terminal_tx: Arc::new(Mutex::new(None)),
            abort_tx,
        }
    }
}

struct SessionHandle {
    tx: mpsc::UnboundedSender<SessionCmd>,
    state: SessionState,
}

pub struct AgentManager {
    sessions: Mutex<HashMap<String, SessionHandle>>,
}

impl AgentManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }
}

/// 创建 Agent Session。若同 tabId 已有 session，先销毁旧的。
#[tauri::command]
pub async fn agent_spawn(
    app: tauri::AppHandle,
    state: tauri::State<'_, AgentManager>,
    tab_id: String,
    mode: String,
    role: String,
    initial_message: Option<String>,
    cwd: String,
    panes: Vec<PaneInfo>,
    on_event: Channel<AgentEvent>,
) -> Result<()> {
    let mut sessions = state.sessions.lock().await;

    if sessions.remove(&tab_id).is_some() {
        // 旧 session 的 tx 被 drop，task 退出
    }

    let (tx, rx) = mpsc::unbounded_channel();
    let session_state = SessionState::new();

    // 初始化 pane 列表
    *session_state.panes.lock().await = panes;

    // 首次运行时生成默认角色模板
    session::ensure_default_roles(&app);

    let abort_rx = session_state.abort_tx.subscribe();

    sessions.insert(
        tab_id.clone(),
        SessionHandle {
            tx: tx.clone(),
            state: session_state.clone(),
        },
    );

    tokio::spawn(session_loop(
        rx,
        on_event,
        app.clone(),
        role,
        cwd,
        mode,
        session_state,
        abort_rx,
    ));

    drop(sessions);

    if let Some(msg) = initial_message {
        if !msg.is_empty() {
            let sessions = state.sessions.lock().await;
            if let Some(handle) = sessions.get(&tab_id) {
                let _ = handle.tx.send(SessionCmd::Message(msg));
            }
        }
    }

    Ok(())
}

/// 向已有 session 发送消息
#[tauri::command]
pub async fn agent_send_message(
    state: tauri::State<'_, AgentManager>,
    tab_id: String,
    message: String,
) -> Result<()> {
    let sessions = state.sessions.lock().await;
    match sessions.get(&tab_id) {
        Some(handle) => {
            handle
                .tx
                .send(SessionCmd::Message(message))
                .map_err(|_| crate::error::Error::msg("Agent session 已关闭"))?;
        }
        None => {
            return Err(crate::error::Error::msg("Agent session 不存在"));
        }
    }
    Ok(())
}

/// 中止当前 LLM 请求 / 工具执行 / 交互等待
#[tauri::command]
pub async fn agent_abort(
    state: tauri::State<'_, AgentManager>,
    tab_id: String,
) -> Result<()> {
    let sessions = state.sessions.lock().await;
    if let Some(handle) = sessions.get(&tab_id) {
        // 直接设 watch，不走 mpsc——中止需要立即生效，即使 session 在等待 oneshot
        let _ = handle.state.abort_tx.send(true);
    }
    Ok(())
}

/// 销毁 session（Tab 关闭时调用）
#[tauri::command]
pub async fn agent_destroy(
    state: tauri::State<'_, AgentManager>,
    tab_id: String,
) -> Result<()> {
    state.sessions.lock().await.remove(&tab_id);
    Ok(())
}

/// Ask 模式：用户确认工具调用
#[tauri::command]
pub async fn agent_approve_tool(
    state: tauri::State<'_, AgentManager>,
    tab_id: String,
    approved: bool,
) -> Result<()> {
    let sessions = state.sessions.lock().await;
    if let Some(handle) = sessions.get(&tab_id) {
        if let Some(tx) = handle.state.approve_tx.lock().await.take() {
            let _ = tx.send(approved);
        }
    }
    Ok(())
}

/// AskUser 工具：用户回答
#[tauri::command]
pub async fn agent_answer_question(
    state: tauri::State<'_, AgentManager>,
    tab_id: String,
    answer: String,
) -> Result<()> {
    let sessions = state.sessions.lock().await;
    if let Some(handle) = sessions.get(&tab_id) {
        if let Some(tx) = handle.state.answer_tx.lock().await.take() {
            let _ = tx.send(answer);
        }
    }
    Ok(())
}

/// read_terminal 工具：前端回调终端 buffer 文本
#[tauri::command]
pub async fn agent_terminal_data(
    state: tauri::State<'_, AgentManager>,
    tab_id: String,
    _request_id: String,
    data: String,
) -> Result<()> {
    let sessions = state.sessions.lock().await;
    if let Some(handle) = sessions.get(&tab_id) {
        if let Some(tx) = handle.state.terminal_tx.lock().await.take() {
            let _ = tx.send(data);
        }
    }
    Ok(())
}

/// 前端同步 Tab 内 pane 列表（方案 B：不改 pane.ts，由 Agent Modal 主动收集）
#[tauri::command]
pub async fn agent_update_panes(
    state: tauri::State<'_, AgentManager>,
    tab_id: String,
    panes: Vec<PaneInfo>,
) -> Result<()> {
    let sessions = state.sessions.lock().await;
    if let Some(handle) = sessions.get(&tab_id) {
        *handle.state.panes.lock().await = panes;
    }
    Ok(())
}

/// 读取 ai-config.json
#[tauri::command]
pub async fn agent_load_config(app: tauri::AppHandle) -> Result<config::AiConfig> {
    config::load_config(&app)
}

/// 保存 ai-config.json
#[tauri::command]
pub async fn agent_save_config(app: tauri::AppHandle, config: config::AiConfig) -> Result<()> {
    config::save_config(&app, &config)
}

/// 清除对话历史（删除持久化文件 + 清空内存历史）
#[tauri::command]
pub async fn agent_clear_history(
    state: tauri::State<'_, AgentManager>,
    tab_id: String,
) -> Result<()> {
    let sessions = state.sessions.lock().await;
    if let Some(handle) = sessions.get(&tab_id) {
        let _ = handle.tx.send(session::SessionCmd::ClearHistory);
    }
    Ok(())
}
