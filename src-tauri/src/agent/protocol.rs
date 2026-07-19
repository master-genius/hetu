//! AgentEvent — Agent → 前端的事件类型，通过 Tauri Channel 点对点推送。
//! Phase 1b：消息流 + 工具调用生命周期 + 错误 + 中止 + 完成。
//! Phase 1c：重试通知 + 上下文截断通知。
//! Phase 2：Ask/Plan 模式 + AskUser 提问 + read_terminal + list_panes。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::ipc::Channel;

/// Agent → 前端的事件。通过 Tauri Channel 推送。
#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AgentEvent {
    /// 助手消息（流式增量）。done=true 表示该条消息的文本流结束（不代表整个轮次结束）。
    Message { content: String, done: bool },

    /// 工具调用开始
    ToolStart {
        tool: String,
        args: Value,
        target_pane: usize,
    },

    /// 工具实时输出（如 run_command 的 stdout/stderr 行，Phase 1b 预留）
    #[allow(dead_code)]
    ToolOutput { output: String },

    /// 工具调用结束
    ToolEnd { result: ToolResult },

    /// Ask 模式：请求用户确认工具调用
    AskApproval {
        tool: String,
        args: Value,
        target_pane: usize,
        reason: String,
    },

    /// Plan 模式：提出执行计划
    ProposedPlan {
        steps: Vec<PlanStep>,
        summary: String,
    },

    /// Agent 向用户提问（AskUser 工具触发）
    UserQuestion {
        question: String,
        choices: Vec<UserChoice>,
    },

    /// 读取终端 buffer 请求（read_terminal 工具触发）
    ReadTerminalRequest {
        request_id: String,
        pane_id: String,
        lines: usize,
    },

    /// 错误（API 调用失败、配置错误等）。推送后 Session 回到 Idle。
    Error { message: String },

    /// 用户主动中止
    Aborted,

    /// 整个响应周期结束（LLM 最终回复完毕或错误后）。Session 回到 Idle，可接受新消息。
    Done,

    /// 重试通知（429 切换 endpoint / 5xx 退避 / 超时重试）
    Retrying { reason: String, attempt: usize, max_attempts: usize },

    /// 上下文截断通知（trim_history 移除了旧工具输出 / 旧消息）
    ContextTrimmed { removed_tools: usize, removed_messages: usize },

    /// 历史恢复（agent_spawn 时加载了持久化历史，前端渲染旧消息）
    HistoryRestored { messages: Vec<HistoryEntry> },

    /// 历史清除（用户点击"清除"按钮）
    HistoryCleared,
}

/// 历史消息条目（前端渲染用，不含 tool_calls）
#[derive(Serialize, Clone)]
pub struct HistoryEntry {
    pub role: String,
    pub content: String,
}

/// 工具执行结果
#[derive(Serialize, Clone)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ToolResult {
    Success { output: String, truncated: bool },
    Error { message: String },
    UserRejected,
}

impl ToolResult {
    /// 转换为 LLM 可读的文本（写入对话历史的 tool 消息）
    pub fn to_llm_text(&self) -> String {
        match self {
            ToolResult::Success { output, .. } => output.clone(),
            ToolResult::Error { message } => format!("Error: {message}"),
            ToolResult::UserRejected => "用户拒绝了此操作".into(),
        }
    }
}

/// 计划步骤（Plan 模式）
#[derive(Serialize, Clone)]
pub struct PlanStep {
    pub tool: String,
    pub args: Value,
    pub target_pane: usize,
}

/// 用户选择项（AskUser 工具）
#[derive(Serialize, Clone, Deserialize)]
pub struct UserChoice {
    pub label: String,
    pub description: String,
    pub action: String,
}

/// Pane 信息（list_panes 工具 + agent_update_panes 命令）
#[derive(Serialize, Clone, Deserialize)]
pub struct PaneInfo {
    pub id: String,
    pub is_local: bool,
    pub conn_id: String,
    pub host: String,
    pub cwd: String,
    pub os: String,
}

/// 全局历史索引条目（app_data_dir/ai-sessions.json）
#[derive(Serialize, Clone, Deserialize)]
pub struct HistoryIndex {
    pub cwd: String,
    pub last_active: String,
    pub preview: String,
    pub role: String,
    pub model: String,
    /// 目录是否存在（前端用于显示"迁移"按钮）
    pub dir_exists: bool,
}

/// 向 Channel 推送事件，忽略发送错误（前端可能已关闭）。
pub fn emit(tx: &Channel<AgentEvent>, event: AgentEvent) {
    let _ = tx.send(event);
}
