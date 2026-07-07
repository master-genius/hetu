---
name: xterm-v6-scrollbar-double-width
description: xterm.js v6 ships a VSCode-style custom scrollbar (.xterm-scrollable-element > .scrollbar) that overlays the native .xterm-viewport scrollbar, causing double/wide scrollbars — hide the NATIVE scrollbar and style the CUSTOM one (reversed from initial intuition).
source: auto-skill
extracted_at: '2026-07-08T00:00:00.000Z'
---

# xterm.js v6 双滚动条 / 滚动条过宽问题

## 症状

升级到 xterm.js v6 (`@xterm/xterm` ^6.0.0) 后，终端滚动条：
- 一直可见（即使内容未溢出）
- 宽度比以前大很多
- 原本是很细的一条、鼠标悬停时略宽

## 根因

xterm.js v6 在 `xterm.css` 中引入了 VSCode 风格的自定义滚动条组件：
`.xterm-scrollable-element > .scrollbar`（含 `.visible`/`.invisible`/`.fade` 等子类，
内部有 `.slider` 子元素）。

这个自定义滚动条 **叠加在** `.xterm-viewport` 的原生 `overflow-y: scroll` 滚动条之上，
两者同时显示导致滚动条看起来过宽。v5 及之前版本只有原生滚动条。

## ⚠️ 关键教训：方向反了会导致滚动条完全消失

**第一次尝试（错误）**：隐藏 xterm 自定义滚动条，样式化原生滚动条。
**结果**：滚动条完全不显示，设置开关也没用。

**根因分析**：xterm v6 的自定义滚动条组件（`.xterm-scrollable-element`）接管了
滚动条的显隐逻辑（通过 JS 添加/移除 `.visible`/`.invisible` 类）。当自定义滚动条
被 `display: none` 后，xterm 的 JS 仍然认为它在控制滚动条显示，但实际 DOM 已被
移除 → 滚动条彻底消失。原生 `::-webkit-scrollbar` 样式在 WebKitGTK + xterm v6 的
组合下不可靠（xterm 可能通过 JS 设置 inline style 覆盖）。

**正确做法**：隐藏 `.xterm-viewport` 的**原生**滚动条，保留并样式化 xterm v6 的
**自定义**滚动条。

## 排查路径

1. 检查 `node_modules/@xterm/xterm/css/xterm.css` 中是否有
   `.xterm-scrollable-element > .scrollbar` 相关规则 — 这是 v6 新增的
2. 确认 `.xterm-viewport` 的 `overflow-y: scroll` 设置
3. 对比 git 历史确认自定义滚动条不是项目自己引入的
4. 注意：xterm v6 的类型定义中有 `scrollbarSliderBackground` 等主题属性，
   但**没有** `verticalScrollbarSize` / `horizontalScrollbarSize` 配置项，
   所以无法通过 API 控制自定义滚动条宽度
5. **关键**：检查 xterm JS 源码中 `_visibilityController` / `ScrollbarVisibilityController`
   的逻辑——它会通过 JS 动态切换 `.visible`/`.invisible` 类名来控制显隐，
   纯 CSS `display:none` 会破坏这个逻辑链

## 修复方案（正确版）

隐藏原生滚动条，样式化 xterm v6 自定义滚动条：

```css
/* 隐藏 .xterm-viewport 的原生滚动条（避免与自定义滚动条双重显示） */
.xterm .xterm-viewport {
  background: transparent !important;
  scrollbar-width: none; /* Firefox */
}
.xterm .xterm-viewport::-webkit-scrollbar {
  width: 0;
  height: 0;
}

/* 样式化 xterm v6 自定义滚动条：默认 3px 极细，hover 加宽至 7px */
.xterm .xterm-scrollable-element > .scrollbar.vertical {
  width: 3px !important;
  transition: width 0.15s ease !important;
}
.xterm .xterm-scrollable-element > .scrollbar.vertical > .slider {
  background: color-mix(in srgb, var(--term-fg) 30%, transparent) !important;
  border-radius: 3px !important;
  width: 3px !important;
  left: 0 !important;
}
.xterm .xterm-scrollable-element > .scrollbar.vertical > .slider:hover {
  background: color-mix(in srgb, var(--term-fg) 50%, transparent) !important;
}
.xterm .xterm-scrollable-element:hover > .scrollbar.vertical {
  width: 7px !important;
}
.xterm .xterm-scrollable-element:hover > .scrollbar.vertical > .slider {
  width: 7px !important;
  border-radius: 7px !important;
}
```

如果项目有「显示/隐藏滚动条」的设置开关（通过 `data-scrollbar` 属性控制），
隐藏规则改为针对自定义滚动条：

```css
:root[data-scrollbar="0"] .xterm .xterm-scrollable-element > .scrollbar {
  display: none !important;
}
```

## 注意事项

- **方向至关重要**：隐藏自定义滚动条 → 滚动条消失；隐藏原生滚动条 → 正常工作
- `.scrollbar.vertical` 是 xterm v6 自定义滚动条的竖直方向选择器
- `.slider` 是滚动条内部的滑块子元素（可拖动部分）
- xterm v6 的 `AbstractScrollableElement` JS 会动态控制 `.visible`/`.invisible` 类，
  所以不能用 `display:none` 来隐藏自定义滚动条（除非完全不需要滚动条）
- 颜色使用 `color-mix()` 跟随主题前景色，避免硬编码
- `!important` 必要：xterm v6 通过 JS 设置 inline style，CSS 不加 `!important`
  会被覆盖
