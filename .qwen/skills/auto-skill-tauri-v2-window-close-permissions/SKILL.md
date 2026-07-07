---
name: tauri-v2-window-close-permissions
description: Tauri v2 窗口关闭/destroy 权限缺失排查 — onCloseRequested + preventDefault + destroy 链路中的静默失败
source: auto-skill
extracted_at: '2026-07-07T12:02:21.105Z'
---

# Tauri v2 窗口关闭权限排查

## 场景

Tauri v2 应用中，用户点击关闭按钮后弹确认框，确认后窗口仍不关闭；再次点击则连确认框都不弹出（完全无反应）。

## 根因模式

`onCloseRequested` 回调中 `event.preventDefault()` + 手动 `win.destroy()` 是标准模式，但：

1. **`core:window:allow-destroy` 不在 default 权限中**。`core:window` 的 default permission 只含只读查询类命令（get-all-windows、scale-factor、is-maximized 等），`allow-close` 和 `allow-destroy` 需要单独声明。

2. **`destroy()` 调用 IPC `plugin:window|destroy`**，权限不足时抛异常。若调用方用 `void` 或无 catch 吞掉 Promise reject，则 destroy 静默失败。

3. **防重入 flag 卡死**：典型模式是 `let closing = false; if (closing) return; closing = true;` — destroy 失败后 `closing` 永远保持 `true`，后续所有关闭请求被跳过。

## 排查步骤

1. 检查 `src-tauri/capabilities/*.json` 中是否有 `core:window:allow-destroy`
2. 查看 `src-tauri/gen/schemas/acl-manifests.json` 确认权限名（`allow-destroy` 对应 command `destroy`）
3. 检查 JS 端 `onCloseRequested` 的实现：`@tauri-apps/api/window.js` 中 `onCloseRequested` 在 handler 返回后，若 `!isPreventDefault()` 会自动调 `this.destroy()`

## 修复清单

```
// capabilities/default.json — 添加：
"core:window:allow-destroy"

// 前端 destroy 调用加 catch + 重置 flag：
await win.destroy().catch(() => { closing = false; });
```

## 注意事项

- `allow-close`（emit closeRequested 事件）和 `allow-destroy`（强制关闭窗口）是两个独立权限
- Tauri v2 的 ACL 权限系统比 v1 严格得多，许多在 v1 中无需声明的操作在 v2 中需要显式授权
- `list_fonts` 等同步 `#[tauri::command]`（非 async）在 v2 中默认在主线程执行，会阻塞 UI
