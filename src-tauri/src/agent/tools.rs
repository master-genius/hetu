//! 内建工具注册表 + 本地实现。
//! Phase 1b：read_file, write_file, list_dir, run_command, search, file_stat
//! Phase 3 将扩展为 SSH 远程工具（tools_ssh.rs）。

use serde_json::{json, Value};
use tokio::process::Command;

use crate::agent::protocol::ToolResult;
use crate::agent::provider::{ToolDef, ToolFunction};

/// 返回所有内建工具的定义（传递给 LLM 的 function calling schema）
pub fn definitions() -> Vec<ToolDef> {
    vec![
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
                        "content": { "type": "string", "description": "要写入的完整内容" }
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
                        "path": { "type": "string", "description": "目录路径（默认为工作目录）" }
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
                        "cwd": { "type": "string", "description": "工作目录（默认为当前工作目录）" }
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
                        "path": { "type": "string", "description": "搜索路径（默认为工作目录）" }
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
                        "path": { "type": "string", "description": "文件路径" }
                    },
                    "required": ["path"]
                }),
            },
        },
    ]
}

/// 执行工具调用。返回 ToolResult。
pub async fn execute(name: &str, args: &Value, cwd: &str) -> ToolResult {
    match name {
        "read_file" => read_file(args, cwd).await,
        "write_file" => write_file(args, cwd).await,
        "list_dir" => list_dir(args, cwd).await,
        "run_command" => run_command(args, cwd).await,
        "search" => search(args, cwd).await,
        "file_stat" => file_stat(args, cwd).await,
        _ => ToolResult::Error {
            message: format!("未知工具: {name}"),
        },
    }
}

// ---------- 工具实现 ----------

const READ_FILE_MAX_LINES: usize = 500;
const READ_FILE_TAIL_LINES: usize = 50;
const LIST_DIR_MAX_ITEMS: usize = 200;
const RUN_CMD_MAX_LINES: usize = 300;
const SEARCH_MAX_MATCHES: usize = 100;

async fn read_file(args: &Value, cwd: &str) -> ToolResult {
    let path = match args.get("path").and_then(|v| v.as_str()) {
        Some(p) => resolve_path(p, cwd),
        None => return err("缺少参数: path"),
    };

    let content = match tokio::fs::read_to_string(&path).await {
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

async fn write_file(args: &Value, cwd: &str) -> ToolResult {
    let path = match args.get("path").and_then(|v| v.as_str()) {
        Some(p) => resolve_path(p, cwd),
        None => return err("缺少参数: path"),
    };
    let content = match args.get("content").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return err("缺少参数: content"),
    };

    if let Some(parent) = std::path::Path::new(&path).parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }

    match tokio::fs::write(&path, content).await {
        Ok(()) => success(format!("已写入 {path} ({} bytes)", content.len()), false),
        Err(e) => err(format!("写入文件失败 {path}: {e}")),
    }
}

async fn list_dir(args: &Value, cwd: &str) -> ToolResult {
    let path = match args.get("path").and_then(|v| v.as_str()) {
        Some(p) => resolve_path(p, cwd),
        None => cwd.to_string(),
    };

    let mut entries = match tokio::fs::read_dir(&path).await {
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

async fn run_command(args: &Value, cwd: &str) -> ToolResult {
    let command = match args.get("command").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return err("缺少参数: command"),
    };
    let work_dir = args
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|p| resolve_path(p, cwd))
        .unwrap_or_else(|| cwd.to_string());

    let output = match Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(&work_dir)
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

async fn search(args: &Value, cwd: &str) -> ToolResult {
    let pattern = match args.get("pattern").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return err("缺少参数: pattern"),
    };
    let path = match args.get("path").and_then(|v| v.as_str()) {
        Some(p) => resolve_path(p, cwd),
        None => cwd.to_string(),
    };

    let out = match Command::new("grep")
        .arg("-rn")
        .arg("--color=never")
        .arg(pattern)
        .arg(&path)
        .current_dir(cwd)
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
        return success("无匹配结果".into(), false);
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

async fn file_stat(args: &Value, cwd: &str) -> ToolResult {
    let path = match args.get("path").and_then(|v| v.as_str()) {
        Some(p) => resolve_path(p, cwd),
        None => return err("缺少参数: path"),
    };

    let meta = match tokio::fs::metadata(&path).await {
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
