# hfile — 终端模式文件管理器设计文档

> 状态：已实现

## 定位

在终端中输入 `hfile` 即可打开文件管理器面板，复用现有 Explorer 组件（与工具栏图标打开的侧边面板相同）。
支持本地与远程（已激活的 SSH 连接），支持浮动覆盖层模式（`-w`）跟随终端分屏定位。

## 与现有架构的关系

```
hfile 脚本（POSIX sh，随 hssh 一起安装到 bin/）
  → OSC 1732;tok=...;w=<0|1>;d=<b64>;r=<b64>
  → 前端 OSC handler（pane.ts，isLocal + hsshToken 双重门控）
  → main.ts handleHfile()
     ├── 无 -w：切换现有侧边面板（syncExplorerPanel / syncRemotePanel）
     └── -w：创建浮动覆盖层 div，挂载 Explorer 实例（非独占，ESC 关闭）
```

## 用法

```
hfile                              切换本地文件面板（等同点击图标）
hfile -d <路径>                    打开本地面板并跳转到指定目录
hfile -w                           在当前终端上创建浮动覆盖层
hfile -w -d <路径>                 浮动覆盖层 + 指定目录
hfile -r <连接名>                  打开远程文件面板（连接须已激活）
hfile -r <连接名> -d <远程目录>    远程面板 + 指定远程目录
hfile -r <连接名> -w               远程浮动覆盖层
hfile -h, --help                   显示帮助
```

## 设计要点

### 1. OSC 1732（hfile 专用通道）

- 与 hssh(1729)/hexit(1730)/himage(1731) 同安全模型：`isLocal` 门控 + `hsshToken` 校验
- 载荷字段：`tok`(令牌) / `w`(浮动模式) / `d`(目录) / `r`(远程连接名)
- 不干扰任何现有 OSC handler

### 2. 非 -w 模式：复用现有面板逻辑

- 本地：`tab.explorerOpen = !tab.explorerOpen; syncExplorerPanel(revealDir)`
- 远程：`tab.remoteOpen = true; syncRemotePanel(true, overrideConnId)`
- `syncRemotePanel` 新增可选 `overrideConnId` 参数，不影响现有调用者（默认 `undefined` → `focusedConnId()`）
- `-d` 指定目录时：用 `syncExplorerPanel(false)` 打开面板（被动同步），然后 `inst.reveal(spec.dir)` 跳转
  - `init()` 的 `if (this.initialized) return` 守卫保证幂等，无竞态

### 3. -w 模式：浮动覆盖层

- 取 `pane.element.getBoundingClientRect()` 定位，`position: fixed` 直接挂 `document.body`
- 创建独立 Explorer 实例，装配上传/下载回调（与侧边面板同款逻辑）
- ESC 关闭（capture-phase `window` listener，防止 xterm 截获），关闭时 `removeEventListener`
- 无 overlay 遮罩层，不阻止底层交互；非独占，可多开
- 无最大化/最小化（与 himage -w 一致的紧凑模式）

### 4. 远程模式

- `hfile -r <连接名>`：通过 `findConnByName(name)` 遍历 `connMeta` 查找已激活的连接
- 未找到 → toast 提示，不尝试自动连接
- 远程 backend 构造与 `syncRemotePanel` 中的 `remoteBackend(connId)` 相同

### 5. -d 目录指定

- 本地：`pane.resolveLocalCwd()` → `inst.reveal(dir)` 跳转
- 远程：`api.remoteHome(connId)` → `inst.reveal(dir)` 跳转
- `init()` 幂等守卫确保 `syncExplorerPanel` 异步 resolveLocalCwd 与同步 `reveal(spec.dir)` 无竞态

## 涉及文件

| 文件 | 改动 |
|------|------|
| `src-tauri/src/local.rs` | HSSH_SCRIPT 增加 `-l/--list`；新增 HFILE_SCRIPT；install_hssh 增加 hfile 落地 |
| `src/pane.ts` | HsshSpec 加 `mode: "list"`；新增 HfileSpec、HFILE_OSC=1732、onHfile 回调 |
| `src/main.ts` | handleHssh list 分支；handleHfile；findConnByName；syncRemotePanel 可选 connId |
| `src/styles.css` | `.hfile-overlay` 浮动覆盖层样式 |
