/** AgentEvent — 与 Rust agent/protocol.rs 对应的前端类型。
 *  通过 Tauri Channel 点对点推送。 */

export type AgentEvent =
  | { type: "message"; content: string; done: boolean }
  | { type: "error"; message: string }
  | { type: "aborted" }
  | { type: "done" };

/** OSC 1733 载荷解析结果 */
export interface HaiSpec {
  tok: string;
  op: string;
  role: string;
  mode: string;
  msg: string;
}
