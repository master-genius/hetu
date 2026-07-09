/**
 * 自动化喂入模块：连接建立后向 pane 逐行喂入命令文本。
 *
 * 设计原理：
 * - 复用现有 pane_input IPC（前端 → PTY），不引入新通信通道
 * - 通过 pane-output 事件探测 shell 就绪（首次输出 = prompt 已显示）
 * - exit $? 使 shell 以最后一条命令的退出码退出，pane-exit 事件据此正确传递
 *
 * 与 hssh --exec/--file/--stdin/--exit 的协作链路：
 *   hssh 脚本 → 命令写入临时文件 → OSC 1729 携带路径 → 前端建连
 *   → read_feed_file 读回内容 → feedPane 喂入 PTY → exit $? 保留退出码
 */

import { api, events, b64encode } from "./ipc";
import type { Pane } from "./pane";

/**
 * 等待 pane 首次输出（shell 就绪信号）。
 * 超时后强制继续——shell 可能已就绪但未产生输出（极罕见）。
 */
function waitForShellReady(paneId: string, timeoutMs = 5000): Promise<void> {
  return new Promise((resolve) => {
    let unlisten: (() => void) | null = null;
    let done = false;
    const finish = () => {
      if (done) return;
      done = true;
      unlisten?.();
      resolve();
    };
    events.onPaneOutput((e) => {
      if (e.paneId === paneId) finish();
    }).then((fn) => {
      // 如果 timeout 已先触发（done=true），立即注销刚注册的监听器，杜绝泄漏
      if (done) fn();
      else unlisten = fn;
    });
    setTimeout(finish, timeoutMs);
  });
}

/**
 * 向已连接的 pane 喂入命令文本。
 *
 * @param pane      目标 pane（已通过 switchConnection 建立连接）
 * @param content   命令文本（\n 分隔，来自 hssh 临时文件）
 * @param exitAfter 为 true 时追加 `exit $?`，使 shell 以最后一条命令的退出码退出；
 *                  pane-exit 事件携带正确退出码，终端显示 `[进程已退出，状态码 N]`，
 *                  随后 pane-closed(exited=true) 触发现有逻辑退回本地终端。
 */
export async function feedPane(
  pane: Pane,
  content: string,
  exitAfter?: boolean,
): Promise<void> {
  await waitForShellReady(pane.id);
  // PTY 行结束符为 \r（回车），统一换行
  let feed = content.replace(/\r\n/g, "\n").replace(/\n/g, "\r");
  if (!feed.endsWith("\r")) feed += "\r";
  await api.paneInput(pane.id, b64encode(feed));
  if (exitAfter) {
    // mpsc channel 保序：上一条 paneInput 已入队，exit $? 必在所有命令之后被 shell 处理
    await api.paneInput(pane.id, b64encode("exit $?\r"));
  }
}
