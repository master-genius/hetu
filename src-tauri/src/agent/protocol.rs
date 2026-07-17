//! AgentEvent — Agent → 前端的事件类型，通过 Tauri Channel 点对点推送。
//! Phase 1b：消息流 + 工具调用生命周期 + 错误 + 中止 + 完成。
//! Phase 1c：重试通知 + 上下文截断通知。

use serde::Serialize;
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
}

/// 工具执行结果
#[derive(Serialize, Clone)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ToolResult {
    Success { output: String, truncated: bool },
    Error { message: String },
}

impl ToolResult {
    /// 转换为 LLM 可读的文本（写入对话历史的 tool 消息）
    pub fn to_llm_text(&self) -> String {
        match self {
            ToolResult::Success { output, .. } => output.clone(),
            ToolResult::Error { message } => format!("Error: {message}"),
        }
    }
}

/// 向 Channel 推送事件，忽略发送错误（前端可能已关闭）。
pub fn emit(tx: &Channel<AgentEvent>, event: AgentEvent) {
    let _ = tx.send(event);
}
