/** AgentEvent — 与 Rust agent/protocol.rs 对应的前端类型。
 *  通过 Tauri Channel 点对点推送。 */

export type AgentEvent =
  | { type: "message"; content: string; done: boolean }
  | { type: "toolStart"; tool: string; args: any; targetPane: number }
  | { type: "toolOutput"; output: string }
  | { type: "toolEnd"; result: ToolResult }
  | { type: "askApproval"; tool: string; args: any; targetPane: number; reason: string }
  | { type: "proposedPlan"; steps: PlanStep[]; summary: string }
  | { type: "userQuestion"; question: string; choices: UserChoice[] }
  | { type: "readTerminalRequest"; requestId: string; paneId: string; lines: number }
  | { type: "error"; message: string }
  | { type: "aborted" }
  | { type: "done" }
  | { type: "retrying"; reason: string; attempt: number; maxAttempts: number }
  | { type: "contextTrimmed"; removedTools: number; removedMessages: number }
  | { type: "historyRestored"; messages: HistoryEntry[] }
  | { type: "historyCleared" };

/** 历史消息条目 */
export interface HistoryEntry {
  role: string;
  content: string;
}

/** 全局历史索引条目 */
export interface HistoryIndex {
  cwd: string;
  lastActive: string;
  preview: string;
  role: string;
  model: string;
  dirExists: boolean;
}

/** 工具执行结果 */
export type ToolResult =
  | { status: "success"; output: string; truncated: boolean }
  | { status: "error"; message: string }
  | { status: "userRejected" };

/** 计划步骤 */
export interface PlanStep {
  tool: string;
  args: any;
  targetPane: number;
}

/** 用户选择项 */
export interface UserChoice {
  label: string;
  description: string;
  action: string;
}

/** Pane 信息 */
export interface PaneInfo {
  id: string;
  isLocal: boolean;
  connId: string;
  host: string;
  cwd: string;
  os: string;
}

/** OSC 1733 载荷解析结果 */
export interface HaiSpec {
  tok: string;
  op: string;
  role: string;
  mode: string;
  msg: string;
  /** 浮动覆盖层模式（-w） */
  w: boolean;
}
