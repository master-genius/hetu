# hfile — 终端模式文件管理器设计文档

> 状态：设计阶段，仅本地实现优先

## 定位

在终端中输入 `hfile` 即可在当前 pane 上方渲染一个 TUI 风格的文件管理器覆盖层。
利用 HetuShell 的 WebView 本质，以 HTML/CSS 渲染完整 UI（图标、预览、图片），
同时保持终端操作的键盘交互习惯（vim 风格）。

## 与现有架构的关系

```
hfile 脚本（POSIX sh，随 hssh 一起安装到 bin/）
  → OSC 1731;tok=...;path=<base64 cwd>
  → 前端 OSC handler（pane.ts）
  → FileManager 覆盖层组件（src/filemanager.ts）
     ├── 文件列表：复用 local_list 后端
     ├── 文件操作：复用 local API + 文件系统
     ├── 图片预览：复用 image_preview（base64 → <img>，WebView 原生渲染）
     ├── 文本预览：复用 sftp_preview 逻辑或直接读文件
     └── 键盘导航：vim 风格快捷键
  → q/ESC 关闭覆盖层，焦点归还 pane
```

## 交互设计

### 布局

```
┌──────────────────────────────────────────┐
│ 📁 ..                                     │  ← 上级目录
│ 📁 src/              4096  Jul 9 14:30   │
│ 📄 main.ts          12580  Jul 9 14:28   │  ← 高亮选中行
│ 🖼️ logo.png          8192  Jul 8 10:00   │
│ 📄 README.md         4520  Jul 7 09:15   │
│                                          │
├──────────────────────────────────────────┤
│ 预览区（选中文件实时展示）                  │
│  1  import { Terminal } from ...          │
│  2  import { FitAddon } from ...          │
│  3  ...                                   │
└──────────────────────────────────────────┘
```

### 快捷键

| 键 | 功能 |
|----|------|
| ↑/k | 上移 |
| ↓/j | 下移 |
| Enter/l | 进入目录 / 打开文件 |
 | Backspace/h | 返回上级目录 |
| Space | 切换预览 |
| d | 删除（确认） |
| r | 重命名 |
| y | 复制（标记） |
| p | 粘贴 |
| m | 移动（标记 + 粘贴时移动而非复制） |
| u | 上传到当前目录（从系统选文件） |
| / | 搜索过滤 |
| g | 跳到顶部 |
| G | 跳到底部 |
| q/ESC | 退出，焦点归还终端 |

### 图片预览

WebView 原生能力：`image_preview` 返回 base64 → `<img src="data:image/png;base64,...">`
直接在预览区渲染。支持 PNG/JPG/GIF/WEBP/BMP/SVG。

## 技术要点

### 1. 覆盖层渲染

- 覆盖层是 `position: absolute` 的 HTML div，覆盖在 xterm canvas 之上
- 不走 PTY，纯 IPC 通信：文件列表/操作直接调后端 command
- 关闭时 `display: none`，不销毁 xterm 实例

### 2. 安全

- hfile 脚本复用 `$HSSH_TOKEN` 令牌校验（与 hssh 同机制）
- OSC 1731 仅在本地终端受理（与 OSC 1729 同安全策略）
- 文件操作经现有后端 command，已有路径校验

### 3. 性能

- 大目录分页/虚拟列表（>500 条时只渲染视口内条目）
- 图片预览缓存（复用 cache.rs 已有机制）
- 目录列表缓存 + 手动刷新（r 键）

### 4. 本地优先，远程后行

- 一期仅支持 `connId === "local"`（local_list / local API）
- 二期扩展远程：复用 sftp_list / sftp_upload / sftp_download / sftp_copy_remote
- 远程图片预览复用 cache.rs 的磁盘缓存机制

## 实现清单（一期：本地）

- [ ] `hfile` 脚本（OSC 1731 + token）
- [ ] `src/filemanager.ts` 覆盖层组件
- [ ] pane.ts OSC 1731 handler
- [ ] 文件列表渲染 + vim 导航
- [ ] 文本/图片预览
- [ ] 基础文件操作（删除/重命名/复制/粘贴）
- [ ] 上传（系统文件选择对话框）
- [ ] 集成到 pane.ts 生命周期（打开/关闭/resize 联动）
