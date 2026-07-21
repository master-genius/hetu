//! 角色管理系统 — 用户自定义系统提示词 + 分类管理。
//!
//! 每个角色一个 `.md` 文件，YAML frontmatter 存元数据，正文为提示词模板。
//! 文件名即角色 ID。文件存储于 app_data_dir/roles/。
//!
//! frontmatter 格式：
//! ```markdown
//! ---
//! name: "通用助手"
//! category: "默认"
//! description: "通用任务助手"
//! ---
//! 你是 HetuShell 的 AI 助手...
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

use crate::error::{Error, Result};

/// 全部分类（固定列表，与前端 ROLE_CATEGORIES 对应）
#[allow(dead_code)]
pub const CATEGORIES: &[&str] = &[
    "默认", "编程", "办公", "游戏", "创意", "设计", "教育", "玄学", "生活",
];

/// 角色元数据（列表用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleMeta {
    pub id: String,
    pub name: String,
    pub category: String,
    pub description: String,
}

/// 角色完整信息（编辑用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleFull {
    pub id: String,
    pub name: String,
    pub category: String,
    pub description: String,
    pub content: String,
}

/// frontmatter 解析结果
struct FrontMatter {
    fields: HashMap<String, String>,
    body: String,
}

/// 解析 frontmatter。无 frontmatter 时返回整个内容作为 body。
fn parse_frontmatter(raw: &str) -> FrontMatter {
    let mut fields = HashMap::new();

    let body = if let Some(rest) = raw.strip_prefix("---\n").or_else(|| raw.strip_prefix("---\r\n")) {
        // 找闭合 ---
        if let Some(end) = rest.find("\n---\n").or_else(|| rest.find("\n---\r\n")) {
            let fm_text = &rest[..end];
            let after = &rest[end..];
            // 跳过 "\n---\n" 或 "\n---\r\n"
            let body_start = after.find('\n').map(|i| i + 1).unwrap_or(after.len());
            let body = &after[body_start..];

            for line in fm_text.lines() {
                if let Some((k, v)) = line.split_once(':') {
                    let key = k.trim().to_string();
                    let val = v.trim().trim_matches('"').trim_matches('\'').to_string();
                    fields.insert(key, val);
                }
            }
            body.to_string()
        } else {
            raw.to_string()
        }
    } else {
        raw.to_string()
    };

    FrontMatter { fields, body }
}

/// 序列化为带 frontmatter 的完整文件内容
fn serialize_frontmatter(name: &str, category: &str, description: &str, content: &str) -> String {
    format!(
        "---\nname: \"{name}\"\ncategory: \"{category}\"\ndescription: \"{description}\"\n---\n{content}"
    )
}

/// roles 目录路径
fn roles_dir(app: &AppHandle) -> Result<PathBuf> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| Error::msg(format!("获取配置目录失败: {e}")))?
        .join("roles"))
}

/// 列出所有角色。文件名（去 .md）为 id。
pub fn list_roles(app: &AppHandle) -> Result<Vec<RoleMeta>> {
    let dir = roles_dir(app)?;
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut roles = Vec::new();
    let entries = std::fs::read_dir(&dir)
        .map_err(|e| Error::msg(format!("读取 roles 目录失败: {e}")))?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            continue;
        }

        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let fm = parse_frontmatter(&raw);

        roles.push(RoleMeta {
            id,
            name: fm.fields.get("name").cloned().unwrap_or_default(),
            category: fm.fields.get("category").cloned().unwrap_or_else(|| "默认".into()),
            description: fm.fields.get("description").cloned().unwrap_or_default(),
        });
    }

    // 按分类排序，同分类按 name
    roles.sort_by(|a, b| {
        a.category
            .cmp(&b.category)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(roles)
}

/// 获取角色完整内容
pub fn get_role(app: &AppHandle, id: &str) -> Result<RoleFull> {
    let path = roles_dir(app)?.join(format!("{id}.md"));
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| Error::msg(format!("读取角色 {id} 失败: {e}")))?;
    let fm = parse_frontmatter(&raw);

    Ok(RoleFull {
        id: id.to_string(),
        name: fm.fields.get("name").cloned().unwrap_or_default(),
        category: fm.fields.get("category").cloned().unwrap_or_else(|| "默认".into()),
        description: fm.fields.get("description").cloned().unwrap_or_default(),
        content: fm.body,
    })
}

/// 仅加载角色提示词正文（供 build_system_prompt 用）
pub fn load_content(app: &AppHandle, id: &str) -> Option<String> {
    let dir = roles_dir(app).ok()?;
    let path = dir.join(format!("{id}.md"));
    let raw = std::fs::read_to_string(&path).ok()?;
    Some(parse_frontmatter(&raw).body)
}

/// 保存角色（新建或覆盖）。同时确保目录存在。
pub fn save_role(
    app: &AppHandle,
    id: &str,
    name: &str,
    category: &str,
    description: &str,
    content: &str,
) -> Result<()> {
    let dir = roles_dir(app)?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| Error::msg(format!("创建 roles 目录失败: {e}")))?;

    let path = dir.join(format!("{id}.md"));
    let data = serialize_frontmatter(name, category, description, content);
    std::fs::write(&path, data)
        .map_err(|e| Error::msg(format!("写入角色 {id} 失败: {e}")))?;
    Ok(())
}

/// 删除角色文件
pub fn delete_role(app: &AppHandle, id: &str) -> Result<()> {
    let path = roles_dir(app)?.join(format!("{id}.md"));
    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|e| Error::msg(format!("删除角色 {id} 失败: {e}")))?;
    }
    Ok(())
}

/// 首次运行时写入默认角色模板
pub fn ensure_defaults(app: &AppHandle) {
    let dir = match roles_dir(app) {
        Ok(d) => d,
        Err(_) => return,
    };
    if dir.exists() {
        return;
    }
    let _ = std::fs::create_dir_all(&dir);

    // general（默认分类）
    let _ = save_role(app, "general", "通用助手", "默认", "通用任务助手", DEFAULT_GENERAL_PROMPT);
    // code（编程分类）
    let _ = save_role(app, "code", "代码助手", "编程", "代码理解、编写和审查", DEFAULT_CODE_PROMPT);
    // debug（编程分类）
    let _ = save_role(app, "debug", "调试助手", "编程", "问题诊断和修复", DEFAULT_DEBUG_PROMPT);
}

// ---------- 默认提示词模板 ----------

const DEFAULT_GENERAL_PROMPT: &str = r#"你是 HetuShell 的 AI 助手，运行在用户终端环境中。

## 当前 Tab 的 Pane 列表
| # | 类型 | 主机 | 当前目录 | 操作系统 |
|---|------|------|----------|----------|
{pane_table}

默认在 Pane 0（你被调用的那个 Pane）上执行操作。
使用 list_panes 查看所有可用 Pane。
使用工具时指定 target_pane 来选择在哪个 Pane 上执行。

可用工具：
- read_file: 读取文件内容（超过 500 行截断）
- write_file: 写入文件
- list_dir: 列出目录内容
- run_command: 执行 shell 命令（非交互式）
- search: 在文件中递归搜索文本
- file_stat: 获取文件元信息
- list_panes: 列出 Tab 内所有 Pane
- read_terminal: 读取终端可见内容
- ask_user: 向用户提问（方向性歧义时使用）

工作方式：
1. 理解用户需求
2. 使用工具收集信息（读文件、执行命令等）
3. 基于工具结果分析问题
4. 给出结论或继续使用工具

规则：
- 执行命令前说明你的意图
- 工具输出可能被截断，需要时分段读取
- 如果某个操作失败，尝试不同方案
- 用简洁的中文回复，代码块使用正确的语言标识"#;

const DEFAULT_CODE_PROMPT: &str = r#"你是 HetuShell 的代码助手，专注于代码理解、编写和审查。

## 当前 Tab 的 Pane 列表
| # | 类型 | 主机 | 当前目录 | 操作系统 |
|---|------|------|----------|----------|
{pane_table}

## 工作目录
{cwd}

## 可用工具
- read_file / write_file / list_dir / search / file_stat: 文件操作
- run_command: 执行编译/测试/lint 等命令
- list_panes / read_terminal / ask_user: 终端交互

## 工作方式
1. 先读相关代码理解上下文
2. 分析问题，给出修改方案
3. 修改后运行测试/编译验证
4. 用简洁的中文解释，代码块使用正确的语言标识

## 规则
- 修改前先阅读现有代码风格
- 遵循项目的命名规范和架构模式
- 不擅自添加未要求的依赖或抽象
- 错误先承认再修复"#;

const DEFAULT_DEBUG_PROMPT: &str = r#"你是 HetuShell 的调试助手，专注于问题诊断和修复。

## 当前 Tab 的 Pane 列表
| # | 类型 | 主机 | 当前目录 | 操作系统 |
|---|------|------|----------|----------|
{pane_table}

## 工作目录
{cwd}

## 可用工具
- read_file / search: 查看源码和日志
- run_command: 执行诊断命令（strace/ltrace/gdb/日志查看等）
- list_panes / read_terminal: 查看终端输出
- ask_user: 确认复现步骤和环境

## 工作方式
1. 复现问题，确认根因
2. 定位到具体代码行
3. 提出最小修复方案
4. 验证修复有效

## 规则
- 先分析根因再改代码，不猜测
- 用简洁的中文报告：现象→根因→修复
- 提供可验证的测试方法"#;
