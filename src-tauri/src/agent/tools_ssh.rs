//! SSH 远程工具实现。
//! 复用已有 Connection 上的 SFTP session（懒初始化）和 exec channel（独立于 PTY）。
//!
//! 所有函数接收 Arc<Connection>，与 tools.rs 的本地实现一一对应。

use std::sync::Arc;

use russh::ChannelMsg;
use tauri::ipc::Channel;

use crate::agent::protocol::{emit, AgentEvent, ToolResult};
use crate::error::{Error, Result};
use crate::ssh::conn::Connection;
use crate::ssh::sftp;

// ---------- 远程工具实现 ----------

/// read_file: 通过 SFTP 读取文件，截断逻辑同本地版
pub async fn read_file(conn: &Arc<Connection>, path: &str) -> ToolResult {
    const MAX_LINES: usize = 500;
    const TAIL_LINES: usize = 50;

    // 先尝试 canonicalize 解析路径（处理 ~ 等）
    let resolved = match sftp::session(conn).await {
        Ok(sftp_sess) => {
            match sftp_sess.canonicalize(path).await {
                Ok(p) => p,
                Err(_) => path.to_string(),
            }
        }
        Err(e) => return err(format!("SFTP 连接失败: {e}")),
    };

    // 用 SFTP 读文件
    let bytes = match sftp::read_file_bytes(conn, &resolved, 10 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => return err(format!("读取文件失败 {path}: {e}")),
    };

    let content = match String::from_utf8_lossy(&bytes) {
        std::borrow::Cow::Borrowed(s) => s.to_string(),
        std::borrow::Cow::Owned(s) => s,
    };

    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    if total <= MAX_LINES {
        success(content, false)
    } else {
        let head_n = MAX_LINES - TAIL_LINES - 1;
        let head: Vec<&str> = lines.iter().take(head_n).copied().collect();
        let tail: Vec<&str> = lines.iter().skip(total - TAIL_LINES).copied().collect();
        let skipped = total - head_n - TAIL_LINES;
        success(
            format!(
                "{}\n... (truncated {} lines) ...\n{}",
                head.join("\n"),
                skipped,
                tail.join("\n"),
            ),
            true,
        )
    }
}

/// write_file: 通过 SFTP 写入文件
pub async fn write_file(conn: &Arc<Connection>, path: &str, content: &str) -> ToolResult {
    let sftp_sess = match sftp::session(conn).await {
        Ok(s) => s,
        Err(e) => return err(format!("SFTP 连接失败: {e}")),
    };

    // 确保父目录存在
    if let Some(parent) = std::path::Path::new(path).parent() {
        let parent_str = parent.to_string_lossy();
        if !parent_str.is_empty() {
            let _ = sftp_sess.create_dir(parent_str.as_ref()).await;
        }
    }

    match sftp_sess.create(path).await {
        Ok(mut file) => {
            use tokio::io::AsyncWriteExt;
            match file.write_all(content.as_bytes()).await {
                Ok(()) => {
                    let _ = file.flush().await;
                    let _ = file.shutdown().await;
                    success(format!("已写入 {path} ({} bytes)", content.len()), false)
                }
                Err(e) => err(format!("写入文件失败 {path}: {e}")),
            }
        }
        Err(e) => err(format!("创建文件失败 {path}: {e}")),
    }
}

/// list_dir: 通过 SFTP 列出目录
pub async fn list_dir(conn: &Arc<Connection>, path: &str) -> ToolResult {
    const MAX_ITEMS: usize = 200;

    let entries = match sftp::list(conn, path).await {
        Ok(e) => e,
        Err(e) => return err(format!("读取目录失败 {path}: {e}")),
    };

    let total = entries.len();
    let truncated = total > MAX_ITEMS;

    let items: Vec<String> = entries
        .iter()
        .take(MAX_ITEMS)
        .map(|e| {
            let prefix = if e.is_dir { "d" } else { "-" };
            format!("{} {}", prefix, e.name)
        })
        .collect();

    let output = format!(
        "{path} ({} entries{})\n{}",
        total,
        if truncated { ", showing first 200" } else { "" },
        items.join("\n"),
    );

    success(output, truncated)
}

/// run_command: 通过 SSH exec channel 执行命令（独立于用户 PTY）
pub async fn run_command(conn: &Arc<Connection>, command: &str, cwd: Option<&str>, timeout_secs: u32, tx: &Channel<AgentEvent>) -> ToolResult {
    const MAX_LINES: usize = 300;

    // 如果有 cwd，用 cd 包裹命令
    let full_cmd = match cwd {
        Some(dir) if !dir.is_empty() => format!("cd {} && {}", sh_quote(dir), command),
        _ => command.to_string(),
    };

    match exec_channel(conn, &full_cmd, timeout_secs, tx).await {
        Ok((stdout, stderr, code)) => {
            let mut combined = String::new();
            if !stdout.is_empty() {
                combined.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !combined.is_empty() {
                    combined.push('\n');
                }
                combined.push_str("[stderr]\n");
                combined.push_str(&stderr);
            }
            combined.push_str(&format!("\n[exit code: {code}]"));

            let lines: Vec<&str> = combined.lines().collect();
            let total = lines.len();
            if total <= MAX_LINES {
                success(combined, false)
            } else {
                let tail: Vec<&str> = lines.iter().skip(total - MAX_LINES).copied().collect();
                success(
                    format!("... (truncated {} lines) ...\n{}", total - MAX_LINES, tail.join("\n")),
                    true,
                )
            }
        }
        Err(e) => err(format!("执行命令失败: {e}")),
    }
}

/// search: 通过 SSH exec 执行 grep -rn
pub async fn search(conn: &Arc<Connection>, pattern: &str, path: &str, tx: &Channel<AgentEvent>) -> ToolResult {
    const MAX_MATCHES: usize = 100;

    let cmd = format!("grep -rn --color=never -- {} {}", sh_quote(pattern), sh_quote(path));
    // search 默认用 command_timeout（通过调用方传入），这里用合理默认值 60s
    match exec_channel(conn, &cmd, 60, tx).await {
        Ok((stdout, stderr, code)) => {
            // grep exit code 1 = no matches (not error), 2 = error
            if code == 2 && !stderr.is_empty() {
                return err(format!("grep 错误: {stderr}"));
            }

            let lines: Vec<&str> = stdout.lines().collect();
            let total = lines.len();
            if total == 0 {
                return success("无匹配结果".to_string(), false);
            }

            if total <= MAX_MATCHES {
                success(stdout, false)
            } else {
                let head: Vec<&str> = lines.iter().take(MAX_MATCHES).copied().collect();
                success(
                    format!("{}\n... (truncated {} more matches) ...", head.join("\n"), total - MAX_MATCHES),
                    true,
                )
            }
        }
        Err(e) => err(format!("执行搜索失败: {e}")),
    }
}

/// file_stat: 通过 SFTP 获取文件元信息
pub async fn file_stat(conn: &Arc<Connection>, path: &str) -> ToolResult {
    match sftp::stat(conn, path).await {
        Ok(meta) => {
            let kind = if meta.is_dir { "directory" }
                else if meta.is_link { "symlink" }
                else { "file" };

            let output = format!(
                "path: {}\ntype: {}\nsize: {} bytes\nperms: {}{}",
                meta.path,
                kind,
                meta.size.unwrap_or(0),
                if meta.perms.is_empty() { "".to_string() } else { format!("{} ", meta.perms) },
                "",
            );

            success(output, false)
        }
        Err(e) => err(format!("获取文件信息失败 {path}: {e}")),
    }
}

// ---------- 辅助函数 ----------

/// 在连接上执行命令，返回 (stdout, stderr, exit_code)
/// timeout_secs 控制命令执行的最大时间，超时后强制终止。
async fn exec_channel(conn: &Arc<Connection>, cmd: &str, timeout_secs: u32, tx: &Channel<AgentEvent>) -> Result<(String, String, i32)> {
    let timeout_dur = std::time::Duration::from_secs(timeout_secs as u64);

    let mut channel = {
        let guard = conn.handle.lock().await;
        let handle = guard.as_ref().ok_or_else(|| Error::msg("连接未建立"))?;
        handle.channel_open_session().await?
    };
    channel.exec(true, cmd).await?;

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut code = -1i32;

    let wait_result = tokio::time::timeout(timeout_dur, async {
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { ref data } => {
                    let text = String::from_utf8_lossy(data);
                    emit(tx, AgentEvent::ToolOutput { output: text.into_owned() });
                    stdout.extend_from_slice(data);
                }
                ChannelMsg::ExtendedData { ref data, .. } => {
                    let text = String::from_utf8_lossy(data);
                    emit(tx, AgentEvent::ToolOutput { output: format!("[stderr] {}", text) });
                    stderr.extend_from_slice(data);
                }
                ChannelMsg::ExitStatus { exit_status } => code = exit_status as i32,
                _ => {}
            }
        }
    })
    .await;

    match wait_result {
        Ok(()) => Ok((
            String::from_utf8_lossy(&stdout).into_owned(),
            String::from_utf8_lossy(&stderr).into_owned(),
            code,
        )),
        Err(_) => Err(Error::msg(format!(
            "SSH 命令执行超时（{timeout_secs}s），可能是不退出命令（如 tail -f）"
        ))),
    }
}

/// POSIX shell 单引号转义
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn success(output: impl Into<String>, truncated: bool) -> ToolResult {
    ToolResult::Success { output: output.into(), truncated }
}

fn err(message: impl Into<String>) -> ToolResult {
    ToolResult::Error { message: message.into() }
}
