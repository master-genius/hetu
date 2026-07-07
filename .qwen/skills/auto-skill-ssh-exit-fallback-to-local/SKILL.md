---
name: ssh-exit-fallback-to-local
description: When an SSH shell exits normally (user types exit), fallback to a local terminal pane instead of closing the pane — only local terminal exit closes the pane/split.
source: auto-skill
extracted_at: '2026-07-08T00:00:00.000Z'
---

# SSH exit 退回本地终端而非关闭 pane

## 需求

用户在 SSH 连接的 shell 中输入 `exit` 后，不应关闭该分屏 pane，
而应退回本地终端。只有本地终端 exit 才触发 pane/tab 的关闭逻辑。

## 事件流分析

后端（Rust）在 shell 进程结束时发出两个事件：

1. `pane-exit` — shell 进程退出，携带 `paneId` 和退出状态码 `status`
2. `pane-closed` — channel 完全关闭，携带 `paneId` 和 `exited` 布尔值

`exited` 的语义：
- `true`：shell 正常退出（用户主动 `exit`，或远程 `ChannelMsg::ExitStatus`）
- `false`：连接断开（网络故障等），需保留 pane 等待重连

后端区分逻辑（`ssh/pane.rs`）：
- `ChannelMsg::ExitStatus` → `exited = true`
- channel 结束（`None`）且 `!exited && !conn.is_alive()` → 触发重连
- 本地终端（`local.rs`）：`deliberate` 标志区分用户主动关闭 vs shell 自行退出

## 修复方案

在前端 `onPaneClosed` 事件处理中，根据 `pane.isLocal` 分流：

```typescript
void events.onPaneClosed((e) => {
  const found = tabs.findPane(e.paneId);
  if (!found) return;
  if (e.exited) {
    // 远程 shell 正常退出 → 退回本地终端，不关闭 pane
    if (!found.pane.isLocal) {
      found.pane.switchConnection("local").catch(() => {});
    } else {
      // 本地终端退出 → 收起该分屏
      void tabs.closePane(found.tab, found.pane);
    }
  }
  // exited=false（连接断开）→ 保留等待重连
});
```

## 关键点

- `pane.switchConnection("local")` 会：关闭当前后端 channel → 设 `connId = "local"` →
  清空 cwd/shellPid/statCache → `term.reset()` → `pane.open()` 开新本地 PTY → `focus()`
- `onPaneExit`（退出码提示）保持不变，仍在终端写入 `[进程已退出，状态码 N]`
- 连接断开（`exited=false`）的重连逻辑不受影响
- 如果 pane 是 tab 中最后一个 pane 且本地终端也 exit，`closePane` 会自动关闭整个 tab

## 防循环机制（代码审查确认）

`switchConnection` 调用 `api.paneClose()` 会关闭旧的后端 channel，可能触发
`pane-closed` 事件。两种后端 pane 类型各有防循环策略，确保不会形成
"switchConnection → paneClose → pane-closed → 又触发 switchConnection" 的循环：

### SSH pane (`ssh/pane.rs`)

`PaneCmd::Close` 分支只做 `channel.eof()` + `channel.close()` + break，
**不发送 `pane-closed` 事件**。只有自然退出（`None` 分支）才发送。

### 本地 PTY (`local.rs`)

`PaneCmd::Close` 设置 `deliberate = true`，reader EOF 时
`exited = !deliberate = false`，发送 `pane-closed`（`exited = false`）。
前端 `onPaneClosed` 收到 `exited = false` → 跳过处理。

### 审查建议（非阻塞）

- `switchConnection("local").catch(() => {})` 静默吞没错误。如果本地 PTY 打开失败，
  pane 会处于空白无 shell 状态。建议在 catch 中向终端写入错误提示。
- `switchConnection` 方法开头建议加 `disposed` 状态检查。
