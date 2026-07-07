---
name: xterm-v6-scrollbar-double-width
description: xterm.js v6 ships a VSCode-style custom scrollbar (.xterm-scrollable-element > .scrollbar) that overlays the native .xterm-viewport scrollbar, causing double/wide scrollbars — hide the custom one and style the native scrollbar instead.
source: auto-skill
extracted_at: '2026-07-07T15:16:43.058Z'
---

# xterm.js v6 双滚动条 / 滚动条过宽问题

## 症状

升级到 xterm.js v6 (`@xterm/xterm` ^6.0.0) 后，终端滚动条：
- 一直可见（即使内容未溢出）
- 宽度比以前大很多
- 原本是很细的一条、鼠标悬停时略宽

## 根因

xterm.js v6 在 `xterm.css` 中引入了 VSCode 风格的自定义滚动条组件：
`.xterm-scrollable-element > .scrollbar`（含 `.visible`/`.invisible`/`.fade` 等子类）。

这个自定义滚动条 **叠加在** `.xterm-viewport` 的原生 `overflow-y: scroll` 滚动条之上，
两者同时显示导致滚动条看起来过宽。v5 及之前版本只有原生滚动条。

`.xterm-viewport` 本身设置了 `overflow-y: scroll`（不是 `auto`），所以即使内容
未溢出也会始终显示滚动条轨道。

## 排查路径

1. 检查 `node_modules/@xterm/xterm/css/xterm.css` 中是否有
   `.xterm-scrollable-element > .scrollbar` 相关规则 — 这是 v6 新增的
2. 确认 `.xterm-viewport` 的 `overflow-y: scroll` 设置
3. 对比 git 历史确认自定义滚动条不是项目自己引入的
4. 注意：xterm v6 的类型定义中有 `scrollbarSliderBackground` 等主题属性，
   但**没有** `verticalScrollbarSize` / `horizontalScrollbarSize` 配置项，
   所以无法通过 API 控制自定义滚动条宽度

## 修复方案

隐藏 xterm 自定义滚动条，改用 CSS 样式化的原生滚动条：

```css
/* 隐藏 xterm v6 自带的 VSCode 风格自定义滚动条 */
.xterm .xterm-scrollable-element > .scrollbar {
  display: none !important;
}

/* 原生滚动条：默认极细，悬停时加宽 */
.xterm .xterm-viewport {
  scrollbar-width: thin;
  scrollbar-color: color-mix(in srgb, var(--term-fg) 25%, transparent) transparent;
}
.xterm .xterm-viewport::-webkit-scrollbar {
  width: 3px;
  height: 3px;
}
.xterm .xterm-viewport::-webkit-scrollbar-track {
  background: transparent;
}
.xterm .xterm-viewport::-webkit-scrollbar-thumb {
  background: color-mix(in srgb, var(--term-fg) 25%, transparent);
  border-radius: 3px;
}
.xterm .xterm-viewport::-webkit-scrollbar-thumb:hover {
  background: color-mix(in srgb, var(--term-fg) 45%, transparent);
}
/* 悬停时加宽方便鼠标操作 */
.xterm .xterm-viewport:hover::-webkit-scrollbar {
  width: 7px;
}
.xterm .xterm-viewport:hover::-webkit-scrollbar-thumb {
  border-radius: 7px;
}
```

如果项目有「显示/隐藏滚动条」的设置开关（通过 `data-scrollbar` 属性控制），
保持对 `:root[data-scrollbar="0"]` 的隐藏规则即可：

```css
:root[data-scrollbar="0"] .xterm-viewport {
  scrollbar-width: none;
}
:root[data-scrollbar="0"] .xterm-viewport::-webkit-scrollbar {
  width: 0;
  height: 0;
}
```

## 注意事项

- `display: none !important` 对 `.xterm-scrollable-element > .scrollbar` 是安全的，
  不影响滚动功能（滚轮/键盘滚动由 `.xterm-viewport` 的 `overflow-y` 处理）
- `scrollbar-width: thin` 是 Firefox 的原生细滚动条，WebKitGTK（Tauri Linux）
  用 `::-webkit-scrollbar` 规则
- 颜色使用 `color-mix()` 跟随主题前景色，避免硬编码
