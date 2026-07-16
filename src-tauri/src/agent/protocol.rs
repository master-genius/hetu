//! AgentEvent — Agent → 前端的事件类型，通过 Tauri Channel 点对点推送。
//! Phase 1a 最小子集：消息流 + 错误 + 中止 + 完成。

use serde::Serialize;
use tauri::ipc::Channel;

/// Phase 1a 事件。后续 Phase 扩展 ToolStart/ToolOutput/ToolEnd/AskApproval 等。
#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AgentEvent {
    /// 助手消息（流式增量）。done=true 表示该条消息流结束。
    Message { content: String, done: bool },
    /// 错误（API 调用失败、配置错误等）。推送后 Session 回到 Idle。
    Error { message: String },
    /// 用户主动中止
    Aborted,
    /// 整个响应周期结束（LLM 回复完毕或错误后）。Session 回到 Idle，可接受新消息。
    Done,
}

/// 向 Channel 推送事件的便捷函数，忽略发送错误（前端可能已关闭）。
pub fn emit(tx: &Channel<AgentEvent>, event: AgentEvent) {
    let _ = tx.send(event);
}
