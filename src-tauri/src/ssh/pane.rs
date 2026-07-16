//! 终端 pane：在既有连接上开一个 PTY channel + shell。
//! 每个 pane 一个 tokio 任务独占 channel，通过 mpsc 接收输入/resize/关闭指令，
//! 输出以 Channel 事件推送给前端 xterm（点对点，无全局广播）。

use std::sync::Arc;

use base64::Engine;
use russh::ChannelMsg;
use serde::Serialize;
use tauri::{AppHandle, ipc::Channel};
use tokio::sync::mpsc;

use crate::error::{Error, Result};
use crate::ssh::conn::Connection;

/// 前端 per-pane 事件：通过 Tauri Channel 点对点推送，取代全局 app.emit 广播。
/// Channel 在 pane_open 命令调用时由前端创建并传入，天然绑定到特定 pane，
/// 无需 paneId 路由，重连时旧 Channel 自动失效（旧任务残余输出不会混入新 pane）。
#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PaneEvent {
    /// PTY 数据输出（base64 编码）
    Output { data: String },
    /// 远程 shell 退出状态码
    Exit { status: i32 },
    /// channel 关闭：exited=true 表示 shell 自行退出，false 表示连接断开
    Closed { exited: bool },
}

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
/// `on_event` 是 Tauri Channel，点对点推送 PaneEvent 到前端对应 pane。
pub async fn open(
    app: AppHandle,
    conn: Arc<Connection>,
    cols: u32,
    rows: u32,
    mut rx: mpsc::UnboundedReceiver<PaneCmd>,
    on_event: Channel<PaneEvent>,
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
                        let _ = on_event.send(PaneEvent::Output { data: b64(&data) });
                    }
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        let _ = on_event.send(PaneEvent::Output { data: b64(&data) });
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        exited = true;
                        let _ = on_event.send(PaneEvent::Exit { status: exit_status as i32 });
                    }
                    Some(_) => {}
                    None => {
                        // channel 结束：若非正常退出且连接已断，交给重连逻辑
                        if !exited && !conn.is_alive().await {
                            conn.clone().trigger_reconnect(app.clone());
                        }
                        let _ = on_event.send(PaneEvent::Closed { exited });
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
