//! 本地终端：不建立 SSH 连接时，直接在本机 PTY 中运行用户 shell（bash 等）。
//! 复用与 SSH pane 相同的 PaneCmd 通道与事件协议，前端无差别渲染。

use base64::Engine;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Serialize;
use std::io::{Read, Write};
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

use crate::error::{Error, Result};
use crate::ssh::pane::PaneCmd;

// ---------- 本地文件系统（文件管理器面板用） ----------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: Option<u64>,
}

/// 列出本地目录（目录在前，按名称排序）
pub fn list_dir(dir: &str) -> Result<Vec<LocalEntry>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let Ok(meta) = entry.metadata() else { continue };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        entries.push(LocalEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            path: entry.path().to_string_lossy().into_owned(),
            is_dir: meta.is_dir(),
            size: meta.len(),
            mtime,
        });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

pub fn home_dir() -> String {
    dirs::home_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".into())
}

// ---------- 本地 PTY ----------

fn size(cols: u32, rows: u32) -> PtySize {
    PtySize {
        rows: rows as u16,
        cols: cols as u16,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// 打开本地 PTY pane。控制线程处理输入/resize/关闭，读线程转发输出。
pub fn open(
    app: AppHandle,
    pane_id: String,
    cols: u32,
    rows: u32,
    mut rx: mpsc::UnboundedReceiver<PaneCmd>,
) -> Result<()> {
    let pty = native_pty_system();
    let pair = pty
        .openpty(size(cols, rows))
        .map_err(|e| Error::msg(format!("创建本地 PTY 失败: {e}")))?;

    let shell = if cfg!(windows) {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into())
    };
    let mut cmd = CommandBuilder::new(&shell);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    if let Some(home) = dirs::home_dir() {
        cmd.cwd(home);
    }
    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| Error::msg(format!("启动本地 shell({shell}) 失败: {e}")))?;
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| Error::msg(format!("PTY reader 失败: {e}")))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| Error::msg(format!("PTY writer 失败: {e}")))?;
    let master = pair.master;

    // 读线程：PTY 输出 → 前端。EOF（shell 退出或 master 关闭）时通知前端收起 pane。
    let app_out = app.clone();
    let pane_out = pane_id.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let _ = app_out.emit(
                        "pane-output",
                        serde_json::json!({
                            "paneId": pane_out,
                            "data": base64::engine::general_purpose::STANDARD.encode(&buf[..n]),
                        }),
                    );
                }
            }
        }
        let _ = app_out.emit(
            "pane-closed",
            serde_json::json!({ "paneId": pane_out, "exited": true }),
        );
    });

    // 控制线程：持有 master 与 child，串行处理指令（blocking_recv 不能在 async 上下文调用）
    std::thread::spawn(move || {
        loop {
            match rx.blocking_recv() {
                Some(PaneCmd::Data(d)) => {
                    if writer.write_all(&d).is_err() {
                        break;
                    }
                }
                Some(PaneCmd::Resize { cols, rows }) => {
                    let _ = master.resize(size(cols, rows));
                }
                Some(PaneCmd::Close) | None => break,
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        // master 随线程结束 drop，读线程随之 EOF 退出
    });

    Ok(())
}
