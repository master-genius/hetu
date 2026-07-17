//! 内建工具注册表 + 本地实现。
//! Phase 1b：read_file, write_file, list_dir, run_command, search, file_stat
//! Phase 3 将扩展为 SSH 远程工具（tools_ssh.rs）。

use std::sync::Arc;

use serde_json::{json, Value};
use tokio::process::Command;

use crate::agent::protocol::ToolResult;
use crate::agent::provider::{ToolDef, ToolFunction};
use crate::ssh::conn::Connection;

/// 返回所有内建工具的定义（传递给 LLM 的 function calling schema）。
/// 文件/命令类工具自动附带 target_pane 参数；特殊工具（ask_user/list_panes/read_terminal）
/// 由 session.rs 特殊处理，不在此返回。
pub fn definitions() -> Vec<ToolDef> {
    let mut defs = vec![
        ToolDef {
            def_type: "function".into(),
            function: ToolFunction {
                name: "read_file".into(),
                description: "读取文件内容。超过 500 行时截断（保留前 449 行 + 末尾 50 行）。".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "文件路径（绝对路径或相对于工作目录的相对路径）"
                        },
                        "target_pane": {
                            "type": "integer",
                            "description": "目标 Pane 索引（默认 0，使用 list_panes 查看）",
                            "default": 0
                        }
                    },
                    "required": ["path"]
                }),
            },
        },
        ToolDef {
            def_type: "function".into(),
            function: ToolFunction {
                name: "write_file".into(),
                description: "将内容写入文件（覆盖已存在的文件）。".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "文件路径" },
                        "content": { "type": "string", "description": "要写入的完整内容" },
                        "target_pane": { "type": "integer", "description": "目标 Pane 索引", "default": 0 }
                    },
                    "required": ["path", "content"]
                }),
            },
        },
        ToolDef {
            def_type: "function".into(),
            function: ToolFunction {
                name: "list_dir".into(),
                description: "列出目录内容（按名称排序）。超过 200 项时截断。".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "目录路径（默认为工作目录）" },
                        "target_pane": { "type": "integer", "description": "目标 Pane 索引", "default": 0 }
                    },
                    "required": []
                }),
            },
        },
        ToolDef {
            def_type: "function".into(),
            function: ToolFunction {
                name: "run_command".into(),
                description: "执行 shell 命令（非交互式，捕获 stdout/stderr/exit code）。输出超过 300 行时保留末尾 300 行。".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "要执行的命令" },
                        "cwd": { "type": "string", "description": "工作目录（默认为当前工作目录）" },
                        "target_pane": { "type": "integer", "description": "目标 Pane 索引", "default": 0 }
                    },
                    "required": ["command"]
                }),
            },
        },
        ToolDef {
            def_type: "function".into(),
            function: ToolFunction {
                name: "search".into(),
                description: "在文件中递归搜索文本（grep -rn）。返回匹配行（文件名:行号:内容）。超过 100 条匹配时截断。".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "搜索模式（正则表达式）" },
                        "path": { "type": "string", "description": "搜索路径（默认为工作目录）" },
                        "target_pane": { "type": "integer", "description": "目标 Pane 索引", "default": 0 }
                    },
                    "required": ["pattern"]
                }),
            },
        },
        ToolDef {
            def_type: "function".into(),
            function: ToolFunction {
                name: "file_stat".into(),
                description: "获取文件/目录的元信息（类型、大小、修改时间、权限）。".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "文件路径" },
                        "target_pane": { "type": "integer", "description": "目标 Pane 索引", "default": 0 }
                    },
                    "required": ["path"]
                }),
            },
        },
        // 特殊工具：session.rs 中特殊处理，不实际走 execute()
        ToolDef {
            def_type: "function".into(),
            function: ToolFunction {
                name: "list_panes".into(),
                description: "列出当前 Tab 内所有 Pane 及连接信息（类型、主机、当前目录、操作系统）。".into(),
                parameters: json!({ "type": "object", "properties": {} }),
            },
        },
        ToolDef {
            def_type: "function".into(),
            function: ToolFunction {
                name: "read_terminal".into(),
                description: "读取终端可见内容（xterm buffer viewport）。默认 100 行，最大 200 行。".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "lines": { "type": "integer", "description": "读取行数（默认 100）" },
                        "target_pane": { "type": "integer", "description": "目标 Pane 索引", "default": 0 }
                    },
                    "required": []
                }),
            },
        },
        ToolDef {
            def_type: "function".into(),
            function: ToolFunction {
                name: "ask_user".into(),
                description: "向用户提问（当遇到方向性歧义、需要用户决策时使用）。用户可选择提供的选项或自由输入。".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "question": { "type": "string", "description": "要问的问题" },
                        "choices": {
                            "type": "array",
                            "description": "可选项（可为空，让用户自由输入）",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "label": { "type": "string", "description": "选项标签" },
                                    "description": { "type": "string", "description": "选项说明" },
                                    "action": { "type": "string", "description": "内部标识" }
                                },
                                "required": ["label"]
                            }
                        }
                    },
                    "required": ["question"]
                }),
            },
        },
    ];
    let _ = &mut defs;
    defs
}

/// 执行工具调用。conn=None 走本地，conn=Some 走远程。
pub async fn execute(name: &str, args: &Value, cwd: &str, conn: Option<&Arc<Connection>>) -> ToolResult {
    match name {
        "read_file" => {
            match args.get("path").and_then(|v| v.as_str()) {
                Some(path) => {
                    let resolved = resolve_path(path, cwd);
                    match conn {
                        Some(c) => crate::agent::tools_ssh::read_file(c, &resolved).await,
                        None => read_file(&resolved).await,
                    }
                }
                None => err("缺少参数: path"),
            }
        }
        "write_file" => {
            let path = match args.get("path").and_then(|v| v.as_str()) {
                Some(p) => resolve_path(p, cwd),
                None => return err("缺少参数: path"),
            };
            let content = match args.get("content").and_then(|v| v.as_str()) {
                Some(c) => c,
                None => return err("缺少参数: content"),
            };
            match conn {
                Some(c) => crate::agent::tools_ssh::write_file(c, &path, content).await,
                None => write_file(&path, content).await,
            }
        }
        "list_dir" => {
            let path = match args.get("path").and_then(|v| v.as_str()) {
                Some(p) => resolve_path(p, cwd),
                None => cwd.to_string(),
            };
            match conn {
                Some(c) => crate::agent::tools_ssh::list_dir(c, &path).await,
                None => list_dir(&path).await,
            }
        }
        "run_command" => {
            let command = match args.get("command").and_then(|v| v.as_str()) {
                Some(c) => c,
                None => return err("缺少参数: command"),
            };
            let work_dir = args.get("cwd")
                .and_then(|v| v.as_str())
                .map(|p| resolve_path(p, cwd))
                .unwrap_or_else(|| cwd.to_string());
            match conn {
                Some(c) => crate::agent::tools_ssh::run_command(c, command, Some(&work_dir)).await,
                None => run_command(command, &work_dir).await,
            }
        }
        "search" => {
            let pattern = match args.get("pattern").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => return err("缺少参数: pattern"),
            };
            let path = match args.get("path").and_then(|v| v.as_str()) {
                Some(p) => resolve_path(p, cwd),
                None => cwd.to_string(),
            };
            match conn {
                Some(c) => crate::agent::tools_ssh::search(c, pattern, &path).await,
                None => search(pattern, &path).await,
            }
        }
        "file_stat" => {
            let path = match args.get("path").and_then(|v| v.as_str()) {
                Some(p) => resolve_path(p, cwd),
                None => return err("缺少参数: path"),
            };
            match conn {
                Some(c) => crate::agent::tools_ssh::file_stat(c, &path).await,
                None => file_stat(&path).await,
            }
        }
        _ => err(format!("未知工具: {name}")),
    }
}

// ---------- 工具实现 ----------

const READ_FILE_MAX_LINES: usize = 500;
const READ_FILE_TAIL_LINES: usize = 50;
const LIST_DIR_MAX_ITEMS: usize = 200;
const RUN_CMD_MAX_LINES: usize = 300;
const SEARCH_MAX_MATCHES: usize = 100;

async fn read_file(path: &str) -> ToolResult {
    let content = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(e) => return err(format!("读取文件失败 {path}: {e}")),
    };

    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    if total <= READ_FILE_MAX_LINES {
        success(content, false)
    } else {
        let head_n = READ_FILE_MAX_LINES - READ_FILE_TAIL_LINES - 1; // -1 for truncation line
        let head: Vec<&str> = lines.iter().take(head_n).copied().collect();
        let tail: Vec<&str> = lines.iter().skip(total - READ_FILE_TAIL_LINES).copied().collect();
        let skipped = total - head_n - READ_FILE_TAIL_LINES;
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

async fn write_file(path: &str, content: &str) -> ToolResult {
    if let Some(parent) = std::path::Path::new(path).parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }

    match tokio::fs::write(path, content).await {
        Ok(()) => success(format!("已写入 {path} ({} bytes)", content.len()), false),
        Err(e) => err(format!("写入文件失败 {path}: {e}")),
    }
}

async fn list_dir(path: &str) -> ToolResult {
    let mut entries = match tokio::fs::read_dir(path).await {
        Ok(rd) => rd,
        Err(e) => return err(format!("读取目录失败 {path}: {e}")),
    };

    let mut items: Vec<String> = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        let ft = entry.file_type().await.ok();
        let prefix = if ft.map(|t| t.is_dir()).unwrap_or(false) { "d" } else { "-" };
        items.push(format!("{prefix} {name}"));
    }

    items.sort();
    let total = items.len();
    let truncated = total > LIST_DIR_MAX_ITEMS;
    if truncated {
        items.truncate(LIST_DIR_MAX_ITEMS);
    }

    let output = format!(
        "{path} ({} entries{})\n{}",
        total,
        if truncated { ", showing first 200" } else { "" },
        items.join("\n"),
    );
    success(output, truncated)
}

async fn run_command(command: &str, work_dir: &str) -> ToolResult {
    let output = match Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(work_dir)
        .output()
        .await
    {
        Ok(out) => out,
        Err(e) => return err(format!("执行命令失败: {e}")),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let code = output.status.code().unwrap_or(-1);

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
    if total <= RUN_CMD_MAX_LINES {
        success(combined, false)
    } else {
        let tail: Vec<&str> = lines.iter().skip(total - RUN_CMD_MAX_LINES).copied().collect();
        success(
            format!("... (truncated {} lines) ...\n{}", total - RUN_CMD_MAX_LINES, tail.join("\n")),
            true,
        )
    }
}

async fn search(pattern: &str, path: &str) -> ToolResult {
    let out = match Command::new("grep")
        .arg("-rn")
        .arg("--color=never")
        .arg(pattern)
        .arg(&path)
        .current_dir("/")
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => return err(format!("执行搜索失败: {e}")),
    };

    let code = out.status.code().unwrap_or(0);
    let stderr = String::from_utf8_lossy(&out.stderr);
    // grep exit code 2 = error (bad regex, etc.)
    if code == 2 && !stderr.is_empty() {
        return err(format!("grep 错误: {stderr}"));
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    let total = lines.len();
    if total == 0 {
        return success("无匹配结果".to_string(), false);
    }

    if total <= SEARCH_MAX_MATCHES {
        success(stdout.to_string(), false)
    } else {
        let head: Vec<&str> = lines.iter().take(SEARCH_MAX_MATCHES).copied().collect();
        success(
            format!("{}\n... (truncated {} more matches) ...", head.join("\n"), total - SEARCH_MAX_MATCHES),
            true,
        )
    }
}

async fn file_stat(path: &str) -> ToolResult {
    let meta = match tokio::fs::metadata(path).await {
        Ok(m) => m,
        Err(e) => return err(format!("获取文件信息失败 {path}: {e}")),
    };

    let kind = if meta.is_dir() { "directory" }
        else if meta.is_file() { "file" }
        else { "symlink/other" };

    let size = meta.len();
    let modified = meta
        .modified()
        .map(|t| {
            t.elapsed()
                .map(|d| format!("{}s ago", d.as_secs()))
                .unwrap_or_else(|_| "unknown".into())
        })
        .unwrap_or_else(|_| "unknown".into());

    #[cfg(unix)]
    let perms = {
        use std::os::unix::fs::PermissionsExt;
        format!("{:o}", meta.permissions().mode() & 0o777)
    };
    #[cfg(not(unix))]
    let perms = "n/a".to_string();

    success(
        format!("path: {path}\ntype: {kind}\nsize: {size} bytes\nmodified: {modified}\npermissions: {perms}"),
        false,
    )
}

// ---------- 辅助函数 ----------

fn resolve_path(path: &str, cwd: &str) -> String {
    if path.starts_with('/') || path.starts_with('~') {
        expand_tilde(path)
    } else {
        format!("{}/{}", cwd.trim_end_matches('/'), path)
    }
}

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    path.to_string()
}

fn success(output: impl Into<String>, truncated: bool) -> ToolResult {
    ToolResult::Success { output: output.into(), truncated }
}

fn err(message: impl Into<String>) -> ToolResult {
    ToolResult::Error { message: message.into() }
}
