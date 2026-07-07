---
name: xterm-transparent-bg-settings-perf
description: xterm.js 透明背景下 minimumContrastRatio 误判 + onSettingsChange 无条件设 theme/mcr 导致滑条卡顿的排查与修复
source: auto-skill
extracted_at: '2026-07-07T14:33:49.893Z'
---

# xterm.js 透明背景 + 设置热应用性能陷阱

## 场景一：浅色主题文字看不清

终端 `background` 设为 `#00000000`（透明，让 CSS 透明度/毛玻璃生效），同时启用 `minimumContrastRatio`（mcr）提亮细字。

### 根因

xterm.js 计算 mcr 时取 `background` 的 RGB 值作为对比基准。`#00000000` 的 RGB = (0,0,0) 即黑色：

- **深色主题**：实际背景深色 ≈ 黑色 → xterm 基准正确 → mcr 正常提亮暗色文字 ✅
- **浅色主题**：实际背景浅色（如 #fafafa），但 xterm 认为是黑色 → 浅色 ANSI 色（white/brightWhite）在"黑色"上对比度已经很高，不调暗 → 实际白底上几乎看不见 ❌

### 修复

MCR 值按主题基调 + 透明度动态决定：

```ts
// 初始化时
minimumContrastRatio: activeTheme().base === "dark"
  ? (s.opacity < 0.4 ? 1.1 : 1.6)  // 暗色：高透明度降至 1.1 避免白边
  : 1.1,                            // 亮色：1.1 微提亮（1.0 太虚，1.1 只调最暗色，安全）

// onSettingsChange 中也要同步（但要有条件，见场景二）
```

**亮色 1.1 安全性**：1.1 极温和，只对最暗的颜色做微提亮，不会误调浅色 ANSI。
原方案亮色 1.0（完全禁用）导致细字发虚，用户反馈 1.1 更好。

**暗色高透明度 1.1**：opacity < 0.4 时，透明背景下 xterm 以黑色为基准算对比度，
过高的 MCR（1.6）会在高透场景产生白边。降至 1.1 消除白边。

## 场景二：拖透明度滑条卡顿（开始快、越拖越卡）

### 根因

`onSettingsChange` 回调中**无条件**设置 `term.options.theme` 和 `term.options.minimumContrastRatio`。xterm 收到这两项变更后会**遍历整个 scrollback buffer（10000 行 × 列数）重新计算每个 cell 的对比度**——这是 O(scrollback) 的重操作。

- 拖透明度触发 `commit()` → `updateSettings()` → `onSettingsChange` → 全量重算
- "开始快、越拖越卡"：终端输出积累后 cell 数量增多，每次重算越来越慢

### 修复

用 `lastThemeId` / `lastThemeBase` / `lastMcr` 追踪，仅在主题或 MCR 真正变化时才设 theme + mcr：

```ts
let lastThemeId = "";
let lastThemeBase = "";
let lastMcr = 0;

onSettingsChange(() => {
  const theme = activeTheme();
  const themeChanged = theme.id !== lastThemeId;
  const baseChanged = theme.base !== lastThemeBase;
  // MCR 随主题和透明度变化（暗色高透明度 1.1，暗色正常 1.6，亮色 1.1）
  const mcr = theme.base === "dark"
    ? (s.opacity < 0.4 ? 1.1 : 1.6)
    : 1.1;
  const mcrChanged = mcr !== lastMcr;

  for (const tab of tabs.tabs) {
    for (const pane of tab.layout.panes()) {
      pane.term.options.fontSize = s.fontSize;  // 轻量，每次都设
      // 仅主题切换时才重设 theme
      if (themeChanged || baseChanged) {
        pane.term.options.theme = colors as never;
      }
      // 仅 MCR 实际变化时才重设（O(scrollback) 重算）
      if (themeChanged || baseChanged || mcrChanged) {
        pane.term.options.minimumContrastRatio = mcr as never;
      }
    }
  }
  lastThemeId = theme.id;
  lastThemeBase = theme.base;
  lastMcr = mcr;
});
```

### 关键认知

- `term.options.fontSize` 修改不触发全量重算，可安全地每次设
- `term.options.theme` 和 `term.options.minimumContrastRatio` 修改触发全量 cell 重算，**必须条件化**
- `term.options.fontFamily` 修改触发单元格重测 + refit，也较重，应仅在字体实际变化时设
- MCR 需要单独追踪（`lastMcr`）：透明度跨越 0.4 阈值时 MCR 值变化，需重设；
  但透明度在阈值同侧滑动时 MCR 不变，不应触发重算

## 场景三：滑条 debounce 的正确用法

### 陷阱

对滑条 `input` 事件做整体 debounce（包括视觉预览）会产生"慢一拍"感——数值标签更新了但画面没变。

### 正确模式

```ts
// input 事件：debounce 仅延迟昂贵的 commit（IPC 写盘 + onSettingsChange）
// 数值标签即时更新
let commitTimer: number | undefined;
const debouncedCommit = () => {
  window.clearTimeout(commitTimer);
  commitTimer = window.setTimeout(commit, 90);
};

input("opacity").addEventListener("input", () => {
  q<HTMLElement>(".opacity-val").textContent = `${...}%`;
  debouncedCommit();
});

// change 事件（松手）：已有通用绑定立即完整 commit
overlay.querySelectorAll("input, select").forEach((el) => {
  el.addEventListener("change", commit);
});
```

## 排查清单

1. 拖滑条卡顿 → 检查 `onSettingsChange` 回调里是否无条件设了 `term.options.theme` / `minimumContrastRatio`
2. 浅色主题文字浅 → 检查终端 background 是否为透明色 + mcr 是否启用
3. "开始快越拖越卡" → 典型的 O(scrollback) 累积效应，确认 xterm option 重设是否被不必要触发
4. 设置弹窗打开卡 → 大 `innerHTML` + `backdrop-filter` 同步合成，用 rAF 延后填充内容 + loading 骨架
