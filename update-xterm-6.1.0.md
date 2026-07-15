# xterm.js 6.0.0 → 6.1.0 升级说明

> 基准：6.1.0-beta.289（2026-07-15 最新 beta）
> 配套 addon：fit 0.12.0-beta.289 / web-links 0.13.0-beta.289 / webgl 0.19.0（无 beta）

---

## 迁移清单

### 唯一需要改的代码

`src/pane.ts:196`：

```ts
// 6.0.0
new Terminal({ overviewRuler: { width: 10 } })

// 6.1.0
new Terminal({ scrollbar: { width: 10 } })
```

- `overviewRuler` 从 `ITerminalOptions` 顶层移除，归入 `ITerminalOptions.scrollbar: IScrollbarOptions`
- `scrollbar.width` 同时控制 OverviewRuler 渲染 + FitAddon 预留宽度
- FitAddon `proposeDimensions()` 同步改为读 `options.scrollbar?.width ?? 14`
- 10px 覆盖逻辑无变化，默认仍为 14px

### 无需处理

- `customGlyphs` 已移除——HetuShell 未使用
- addon API 无变化（fit / web-links / webgl 类型定义无差异）
- `cursorBlink` 语义变更（5 分钟空闲后停止闪烁）——HetuShell 未设置该选项

---

## 🔴 API 不兼容变化

| 变更项 | 6.0.0 | 6.1.0 |
|--------|-------|-------|
| `overviewRuler` 迁移 | `ITerminalOptions.overviewRuler` | 删除；归入 `ITerminalOptions.scrollbar.overviewRuler` |
| `IOverviewRulerOptions.width` | 独立字段 | 移至 `IScrollbarOptions.width` |
| `customGlyphs` | `ITerminalOptions.customGlyphs?: boolean` | 彻底移除 |

---

## 🟢 新增选项

### `scrollbar: IScrollbarOptions`

```ts
interface IScrollbarOptions {
  showScrollbar?: boolean;   // 默认 true
  showArrows?: boolean;      // 默认 false
  width?: number;            // 默认 14，同时启用 OverviewRuler
  overviewRuler?: IOverviewRulerOptions;
}
```

### `vtExtensions: IVtExtensions`

```ts
interface IVtExtensions {
  kittyKeyboard?: boolean;           // kitty keyboard protocol
  kittySgrBoldFaintControl?: boolean;// SGR 221/222
  win32InputMode?: boolean;          // DECSET 9001
  colorSchemeQuery?: boolean;        // 色彩方案查询
}
```

### 其他

| 选项 | 说明 |
|------|------|
| `blinkIntervalDuration` | 光标闪烁间隔（ms），0=禁用 |
| `mouseEventsRequireAlt` | 需按住 Alt 才发送鼠标事件 |
| `quirks.allowSetCursorBlink` | 阻止程序通过 DECSET 12 改光标闪烁 |
| `showCursorImmediately` | 终端创建即显光标，不等聚焦 |

---

## 🟢 新增公开 API

| 属性/方法 | 说明 |
|-----------|------|
| `terminal.dimensions: IRenderDimensions` | 维度（CSS/device 像素），替代 `_core._renderService.dimensions` |
| `terminal.screenElement: HTMLElement` | canvas 渲染层 DOM 元素 |
| `terminal.onDimensionsChange` | 维度变化事件 |
| `terminal.parser.registerApcHandler()` | APC 序列处理器（Kitty graphics 等） |
| `BufferCell.attributesEquals(other)` | 比较单元格属性 |

---

## 内部变化（FitAddon）

- 不再依赖 `core._renderService` 私有 API，改用 `terminal.dimensions` 和 `terminal.options`
- 新增 `scrollbar?.showScrollbar` 检查：`false` 时滚动条宽度按 0 计
- 移除 fit 时的 `_renderService.clear()` 强制重绘
