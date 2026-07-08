---
name: xterm-webgl-context-loss-garbled-text
description: Terminal garbled text after prolonged use — base64/UTF-8 pipeline is clean; root cause is WebGL addon context loss causing accumulated texture atlas corruption on Linux/WebKitGTK
source: auto-skill
extracted_at: '2026-07-08T10:45:14.650Z'
---

# 终端使用一段时间后乱码 — WebGL 上下文丢失

## 症状

终端使用一段时间后出现乱码：
- 某些字符乱了（部分乱码）
- 偶尔全部乱了
- `reset` 命令能修复，但进入某些工作模式后很麻烦

## 排查过程：base64/UTF-8 管线全链路验证

终端数据传输全链路（前端 → 后端 → 前端）逐层排查，**确认管线完全干净**：

| 环节 | 数据类型 | UTF-8 转换? | 风险 |
|------|---------|------------|------|
| PTY/SSH read | `&[u8]` 原始字节 | 否 | 无 |
| Rust base64 encode | `&[u8]` → String(ASCII) | 否 | 无 |
| Tauri event (JSON) | String(base64, 纯 ASCII) | 否 | 无 |
| JS b64decode | String → Uint8Array | 否 | 无 |
| xterm.write | Uint8Array | xterm 内部处理 | 无（xterm 有 UTF-8 缓冲） |
| xterm.onData | string | 是（已完整） | 无 |
| JS b64encode | TextEncoder → UTF-8 bytes → base64 | 是（完整序列） | 无 |
| Rust base64 decode | String → Vec<u8> | 否 | 无 |
| PTY/SSH write | `&[u8]` | 否 | 无 |

**关键结论**：全链路中终端数据始终以原始字节传输，没有 `String::from_utf8_lossy`、
`to_string()` 或任何 UTF-8 字符串转换作用于终端 I/O 数据。Base64 对每个 read chunk
独立编码，不存在跨 chunk 状态依赖。xterm.js 的 `write(Uint8Array)` 内部有 UTF-8 和
转义序列的状态机缓冲，能正确处理跨 `write()` 调用的不完整序列。

**排查清单（确认无问题）**：
- `b64encode`：`TextEncoder().encode()` 产生完整多字节序列 ✅
- `b64decode`：返回 `Uint8Array`（原始字节），不做 UTF-8 解码 ✅
- Rust PTY read：`buf[..n]` 原始字节切片，`base64::encode` 直接编码 ✅
- Rust SSH channel：`ChannelMsg::Data { data }` 的 `CryptoVec` 直接编码 ✅
- `String::from_utf8_lossy` 仅用于文件名和字体列表，不涉及终端 I/O ✅

## 真正根因：WebGL 渲染器累积性损坏

xterm.js WebGL addon（`@xterm/addon-webgl`）在 Linux WebKitGTK 上存在已知的
累积性渲染损坏问题：

- GPU 驱动重置 / WebGL 上下文丢失时，字符纹理 atlas 逐渐错位
- 表现为字符乱码（部分或全部）
- `reset()` 触发完整重绘，清空渲染状态从而修复——与"使用一段时间后出现"高度吻合

## 修复方案

注册 `onContextLoss` 事件，上下文丢失时销毁并重建 WebGL 渲染器：

```ts
private async tryWebgl() {
  try {
    const { WebglAddon } = await import("@xterm/addon-webgl");
    const addon = new WebglAddon();
    // WebGL 上下文丢失（GPU 驱动重置等）时重建渲染器，
    // 否则字符纹理累积损坏导致乱码
    addon.onContextLoss(() => {
      addon.dispose();
      void this.tryWebgl();  // 异步重建
    });
    this.term.loadAddon(addon);
  } catch {
    /* WebGL 不可用时回退 canvas 渲染 */
  }
}
```

## 关键认知

- 乱码出现后 `reset` 能修复 → 大概率是渲染层问题，不是数据层问题
- base64 管线是"字节级透明"的：每个环节都不做 UTF-8 字符串转换，不可能截断多字节字符
- xterm.js `write(Uint8Array)` 内部有 UTF-8 状态机，跨 write 调用能正确缓冲不完整序列
- WebGL `onContextLoss` 是标准 WebGL API，在 GPU 驱动重置/资源回收时触发
- 重建是安全的：pane 已 disposed 时 `loadAddon` 抛异常被 `catch` 吞掉，不会无限递归
- 如果 WebGL 不是原因：在 `pane.write()` 中加日志记录字节十六进制值，对比乱码前后数据

## 排查方法论

遇到终端乱码，按以下顺序排查：

1. **`reset` 能否修复？** → 能 = 渲染/状态层问题；不能 = 数据层问题
2. **验证 base64 管线**：确认全链路无 `String::from_utf8_lossy` / `to_string()` 作用于 I/O 数据
3. **检查 WebGL**：临时注释掉 `tryWebgl()`，强制 canvas 渲染，观察乱码是否消失
4. **检查转义序列注入**：如 `injectCwdTracker` 的 OSC 序列终止符是否正确发出
5. **加日志**：在 `write()` 中记录字节十六进制，确认是数据损坏还是渲染损坏
