---
name: xterm-tui-right-click-copy
description: TUI apps (claude code/vim) enable mouse mode → right-click mousedown clears xterm selection → copy disabled; fix with selection caching + always-enable button + buffer-type gating for download/preview
source: auto-skill
extracted_at: '2026-07-08T10:45:14.650Z'
---

# xterm.js TUI 应用右键复制失效 + 下载菜单误弹

## 症状

在远程 SSH 连接中运行 claude code / qwen code / vim 等交互式 TUI 应用时：
1. 选中文本后右键，"复制"按钮灰显不可点击
2. 选中文本后右键，弹出"下载"菜单项（选中的文本被当作文件路径）

## 根因分析

### 问题1：复制按钮灰显

TUI 应用启用 DECSET 鼠标模式（1000/1002/1003）。xterm.js 在鼠标模式下将鼠标事件
编码为转义序列发给 PTY，而非进行文本选择。右键 `mousedown` 事件会清除 xterm 当前选区，
导致右键菜单弹出时 `term.getSelection()` 返回空字符串 → `disabled: !sel` 为 true。

即使用户按住 Shift 绕过鼠标模式选中了文本，右键 mousedown 仍可能清除选区。

### 问题2：下载菜单误弹

原始代码将选中文本混入了文件路径参数：
```ts
// 错误：选中文本被当作文件路径
const word = this.term.getSelection().trim() || this.wordUnderMouse(e);
```
选中文本是"要复制的内容"，不是"要下载的文件"。两者必须分离。

## 修复方案（三层）

### 第一层：分离 word 和 selection

```ts
// pane.ts contextmenu 事件
this.element.addEventListener("contextmenu", (e) => {
  e.preventDefault();
  // word 只取鼠标下的词，不用选中文本
  const word = this.wordUnderMouse(e);
  this.onContextMenu?.(e, word || null);
});
```

### 第二层：缓存选区 + 始终启用复制按钮

```ts
// pane.ts — 缓存非空选区
private lastSelection = "";

this.term.onSelectionChange(() => {
  const sel = this.term.getSelection();
  if (sel) this.lastSelection = sel;  // 仅非空时更新，空选区不覆盖
  if (sel && getSettings().copyOnSelect) void this.copyText(sel);
});

// mouseup 备份：鼠标模式下 onSelectionChange 可能不触发
this.element.addEventListener("mouseup", () => {
  const sel = this.term.getSelection();
  if (sel) this.lastSelection = sel;
});

// 三重兜底获取选区
getSelectionText(): string {
  return this.term.getSelection()
    || this.lastSelection
    || document.getSelection()?.toString()
    || "";
}
```

```ts
// main.ts — 始终启用复制按钮（去掉 disabled）
{ label: "复制", action: () => void pane.copyText(pane.getSelectionText()) },
// copyText 内部 if (!text) return 保证空文本静默返回
```

### 第三层：buffer 类型门控下载/预览

```ts
// main.ts — 仅普通 shell 输出才提供下载/预览
const normalBuf = pane.term.buffer.active.type === "normal";
const fileWord = normalBuf ? word : null;
// 备用屏幕（vim/less/claude code）下 fileWord = null → 下载/预览禁用
```

## 关键认知

| buffer.active.type | 含义 | 下载/预览 | 复制 |
|---|---|---|---|
| `"normal"` | 普通 shell 输出 | ✅ wordUnderMouse 可能为文件路径 | ✅ 始终可用 |
| `"alternate"` | TUI 全屏应用（vim/less/man/claude code） | ❌ 界面文字不是文件路径 | ✅ 始终可用 |

- `wordUnderMouse` 在备用屏幕下也能读到 TUI 渲染的文字，所以必须用 buffer 类型门控
- `onSelectionChange` 在鼠标模式下可能不触发，需要 `mouseup` 备份
- `clearSelection()` 触发 `onSelectionChange` 时 `sel=""` 为 falsy，不会覆盖 `lastSelection` 缓存
- `copyOnSelect` 行为不受影响：`if (sel && getSettings().copyOnSelect)` 逻辑未变

## 原有功能验证

- `cd /tmp` → `ls` → 鼠标停在文件名上右键 → `wordUnderMouse` 取到文件名 → `normalBuf=true` → "下载"可用 ✅
- 普通终端选中文本右键 → "复制"可点击（即使 mousedown 清了选区，lastSelection 兜底）✅
- vim/claude code 中右键 → 不出现"下载"菜单项（alternate buffer）✅
