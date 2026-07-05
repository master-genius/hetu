//! 终端 pane：在既有连接上开一个 PTY channel + shell。
//! 每个 pane 一个 tokio 任务独占 channel，通过 mpsc 接收输入/resize/关闭指令，
//! 输出以 base64 事件推送给前端 xterm。

use std::sync::Arc;

use base64::Engine;
use russh::ChannelMsg;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

use crate::error::{Error, Result};
use crate::ssh::conn::Connection;

pub enum PaneCmd {
    Data(Vec<u8>),
    Resize { cols: u32, rows: u32 },
    Close,
}

pub struct PaneCtl {
    pub tx: mpsc::UnboundedSender<PaneCmd>,
    pub conn_id: String,
    /// 本地 PTY shell 的进程号（仅本地 pane 有；SSH pane 为 None）。
    /// 用于经 /proc/<pid>/cwd 读本地终端的实时工作目录（拖拽下载落点）。
    pub local_pid: Option<u32>,
}

fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// 打开 pane：请求 PTY(xterm-256color) + shell，随后进入事件循环。
/// 返回后 channel 已就绪；循环任务在后台运行。
pub async fn open(
    app: AppHandle,
    conn: Arc<Connection>,
    pane_id: String,
    cols: u32,
    rows: u32,
    mut rx: mpsc::UnboundedReceiver<PaneCmd>,
) -> Result<()> {
    let mut channel = {
        let guard = conn.handle.lock().await;
        let handle = guard.as_ref().ok_or_else(|| Error::msg("连接未建立"))?;
        handle.channel_open_session().await?
    };
    channel
        .request_pty(false, "xterm-256color", cols, rows, 0, 0, &[])
        .await?;
    channel.request_shell(false).await?;

    tokio::spawn(async move {
        let mut exited = false;
        loop {
            tokio::select! {
                msg = channel.wait() => match msg {
                    Some(ChannelMsg::Data { data }) => {
                        let _ = app.emit("pane-output", serde_json::json!({
                            "paneId": pane_id, "data": b64(&data),
                        }));
                    }
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        let _ = app.emit("pane-output", serde_json::json!({
                            "paneId": pane_id, "data": b64(&data),
                        }));
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        exited = true;
                        let _ = app.emit("pane-exit", serde_json::json!({
                            "paneId": pane_id, "status": exit_status,
                        }));
                    }
                    Some(_) => {}
                    None => {
                        // channel 结束：若非正常退出且连接已断，交给重连逻辑
                        if !exited && !conn.is_alive().await {
                            conn.clone().trigger_reconnect(app.clone());
                        }
                        let _ = app.emit("pane-closed", serde_json::json!({
                            "paneId": pane_id, "exited": exited,
                        }));
                        break;
                    }
                },
                cmd = rx.recv() => match cmd {
                    Some(PaneCmd::Data(d)) => {
                        if channel.data(&d[..]).await.is_err() {
                            break;
                        }
                    }
                    Some(PaneCmd::Resize { cols, rows }) => {
                        let _ = channel.window_change(cols, rows, 0, 0).await;
                    }
                    Some(PaneCmd::Close) | None => {
                        let _ = channel.eof().await;
                        let _ = channel.close().await;
                        break;
                    }
                },
            }
        }
    });
    Ok(())
}
