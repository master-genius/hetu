//! 本地终端：不建立 SSH 连接时，直接在本机 PTY 中运行用户 shell（bash 等）。
//! 复用与 SSH pane 相同的 PaneCmd 通道与事件协议，前端无差别渲染。

use base64::Engine;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Serialize;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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

/// 本地 shell 的实时工作目录：读 /proc/<pid>/cwd 符号链接（与远端 remote_cwd 同理）。
/// 仅 Linux 有 /proc；其它平台不支持（前端会回退到 home）。
#[cfg(target_os = "linux")]
pub fn cwd(pid: u32) -> Result<String> {
    let target = std::fs::read_link(format!("/proc/{pid}/cwd"))?;
    Ok(target.to_string_lossy().into_owned())
}

#[cfg(not(target_os = "linux"))]
pub fn cwd(_pid: u32) -> Result<String> {
    Err(Error::msg("当前平台不支持读取本地终端工作目录"))
}

/// 本地终端标签页信息：实时工作目录 + 前台进程名（供标签标题展示 `目录:进程`）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabInfo {
    /// 实时工作目录（绝对路径）
    pub cwd: String,
    /// 前台进程名：idle 时为 shell 名（bash/zsh…），运行程序时为程序名（vim/node…）
    pub process: String,
}

/// 读取本地终端标签页信息：cwd（/proc/<pid>/cwd）+ 前台进程名。
/// 前台进程：读 /proc/<pid>/stat 的 tpgid（该 tty 的前台进程组），再取组 leader 的 comm；
/// idle 时 tpgid 指向 shell 自身 → "bash"/"zsh"，运行程序时 → 程序名。取不到则回退 shell 自身 comm。
#[cfg(target_os = "linux")]
pub fn tab_info(pid: u32) -> Result<TabInfo> {
    let cwd = std::fs::read_link(format!("/proc/{pid}/cwd"))?
        .to_string_lossy()
        .into_owned();
    let process = fg_process_name(pid).unwrap_or_else(|| shell_comm(pid));
    Ok(TabInfo { cwd, process })
}

/// shell 自身进程名（前台进程探测失败时的兜底）
#[cfg(target_os = "linux")]
fn shell_comm(pid: u32) -> String {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// pid 所在 tty 的前台进程组 leader 的进程名。
#[cfg(target_os = "linux")]
fn fg_process_name(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // comm 字段（第 2 个）可能含空格与括号，跳到最后一个 ')' 之后再按空白切分，
    // 剩余首字段为 state，依次为 ppid pgrp session tty_nr tpgid…，故 tpgid 位于索引 5。
    let rest = stat.get(stat.rfind(')')? + 1..)?;
    let tpgid: i32 = rest.split_whitespace().nth(5)?.parse().ok()?;
    if tpgid <= 0 {
        return None;
    }
    let comm = std::fs::read_to_string(format!("/proc/{tpgid}/comm")).ok()?;
    let comm = comm.trim();
    (!comm.is_empty()).then(|| comm.to_string())
}

#[cfg(not(target_os = "linux"))]
pub fn tab_info(_pid: u32) -> Result<TabInfo> {
    Err(Error::msg("当前平台不支持读取本地终端标签页信息"))
}

// ---------- 本地 PTY ----------

fn size(cols: u32, rows: u32) -> PtySize {
    // 钳制到合法范围：0 或 >u16 都会让 curses 应用错乱，避免 `as u16` 直接回绕
    PtySize {
        rows: rows.clamp(1, u16::MAX as u32) as u16,
        cols: cols.clamp(1, u16::MAX as u32) as u16,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// 某可执行文件是否能在 PATH 中找到
#[cfg(windows)]
fn on_path(exe: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| dir.join(exe).is_file())
}

/// 本机默认 shell：
/// - Windows：优先 PowerShell 7（pwsh.exe），否则系统自带 Windows PowerShell（powershell.exe）
/// - macOS / Linux：系统默认 shell（$SHELL），兜底 /bin/bash
#[cfg(windows)]
fn default_shell() -> String {
    if on_path("pwsh.exe") {
        "pwsh.exe".into()
    } else {
        "powershell.exe".into()
    }
}

#[cfg(not(windows))]
fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into())
}

/// 打开本地 PTY pane。控制线程处理输入/resize/关闭，读线程转发输出。
/// 返回本地 shell 的进程号（用于 /proc/<pid>/cwd 读实时工作目录）；取不到则 None。
pub fn open(
    app: AppHandle,
    pane_id: String,
    cols: u32,
    rows: u32,
    mut rx: mpsc::UnboundedReceiver<PaneCmd>,
) -> Result<Option<u32>> {
    let pty = native_pty_system();
    let pair = pty
        .openpty(size(cols, rows))
        .map_err(|e| Error::msg(format!("创建本地 PTY 失败: {e}")))?;

    let shell = default_shell();
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
    // 在 child 被移入控制线程前取其 PID（供 /proc/<pid>/cwd 读实时工作目录）
    let pid = child.process_id();

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| Error::msg(format!("PTY reader 失败: {e}")))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| Error::msg(format!("PTY writer 失败: {e}")))?;
    let master = pair.master;

    // 主动关闭标志：区分「用户主动关闭/切换连接」与「shell 自己退出」。
    // 前者不应让前端把 pane 当作退出而关闭（否则切换连接会误关当前终端）。
    let deliberate = Arc::new(AtomicBool::new(false));

    // 读线程：PTY 输出 → 前端。EOF 时通知前端；exited 仅在 shell 自行退出时为 true。
    let app_out = app.clone();
    let pane_out = pane_id.clone();
    let deliberate_r = deliberate.clone();
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
        let exited = !deliberate_r.load(Ordering::SeqCst);
        let _ = app_out.emit(
            "pane-closed",
            serde_json::json!({ "paneId": pane_out, "exited": exited }),
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
                // 主动关闭/发送端 drop：标记为非退出，随后让读线程 EOF 时据此上报
                Some(PaneCmd::Close) | None => {
                    deliberate.store(true, Ordering::SeqCst);
                    break;
                }
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        // master 随线程结束 drop，读线程随之 EOF 退出
    });

    Ok(pid)
}
