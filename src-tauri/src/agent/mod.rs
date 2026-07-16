//! AgentManager — 管理 per-tab Agent Session 生命周期。
//! Session 以 tabId 为 key，前端 invoke 时传入。

mod config;
mod openai;
mod provider;
mod protocol;
mod session;

use std::collections::HashMap;
use std::sync::Arc;

use tauri::ipc::Channel;
use tokio::sync::{mpsc, Mutex};

use crate::agent::config::load_config;
use crate::agent::protocol::AgentEvent;
use crate::agent::session::{session_loop, SessionCmd};
use crate::error::Result;

pub struct AgentManager {
    sessions: Mutex<HashMap<String, SessionHandle>>,
}

struct SessionHandle {
    tx: mpsc::UnboundedSender<SessionCmd>,
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
    on_event: Channel<AgentEvent>,
) -> Result<()> {
    let mut sessions = state.sessions.lock().await;

    // 同 tabId 已有 session → 销毁（drop 旧 tx → task 退出）
    if sessions.remove(&tab_id).is_some() {
        // 旧 session 的 tx 被 drop，task 退出
    }

    let (tx, rx) = mpsc::unbounded_channel();

    sessions.insert(
        tab_id.clone(),
        SessionHandle { tx: tx.clone() },
    );

    // 加载配置
    let config = match load_config(&app) {
        Ok(c) => c,
        Err(e) => {
            // 配置加载失败：通知前端
            let _ = on_event.send(AgentEvent::Error {
                message: e.to_string(),
            });
            let _ = on_event.send(AgentEvent::Done);
            // 清理刚插入的 session
            sessions.remove(&tab_id);
            return Ok(());
        }
    };

    // 启动 session 循环
    tokio::spawn(session_loop(rx, on_event, config, role));

    drop(sessions); // 释放锁

    // 若有初始消息，自动发送
    if let Some(msg) = initial_message {
        if !msg.is_empty() {
            // 重新获取锁发送消息
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

/// 中止当前 LLM 请求
#[tauri::command]
pub async fn agent_abort(
    state: tauri::State<'_, AgentManager>,
    tab_id: String,
) -> Result<()> {
    let sessions = state.sessions.lock().await;
    if let Some(handle) = sessions.get(&tab_id) {
        let _ = handle.tx.send(SessionCmd::Abort);
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
