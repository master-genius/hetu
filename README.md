# HetuShell 河图终端

一款**创新的现代终端**：本质是「终端 +（AI、工具链等）」的融合体验，SSH 是它天然内建的能力。
远程连接采用**自实现的 SSH 协议栈**（纯 Rust `russh`，不调用系统 `ssh` 命令），配合 Tauri WebUI，
把传统终端与现代交互（复制粘贴、悬停元信息、双击预览、拖拽上传、文件管理、分屏）融为一体。

> 定位：终端是内核，SSH 是天然支持的第一等能力，AI 辅助与工具链集成是后续演进方向。

## 特性

### 连接
- **自研 SSH 客户端**：基于 `russh`，认证时只提交用户指定的**单一凭据**（一把私钥或一个密码），不会像 openssh 那样把 agent 里的密钥挨个尝试——从根本上避免服务端 `MaxAuthTries` 重试次数限制导致的失败。
- 支持私钥（含口令保护）与密码认证；密码永不落盘。私钥可**直接粘贴内容**或**从文件导入**，导入后内容存入应用自己的 `profiles.json`，不依赖你的密钥文件路径。
- **导入 `~/.ssh/config`**：自动解析 Host / HostName / Port / User / IdentityFile（支持 `Include`），在新建连接对话框中直接可选。
- 主机指纹 TOFU 校验（首次信任，指纹变化即拒绝，防中间人），存于配置目录 `known_hosts.json`。
- **断线自动重连（默认开启）**：指数退避重试，恢复后自动重建所有分屏的 shell。

### 终端体验
- **自定义标题栏**：无系统边框，左侧为操作图标（上传/分屏/设置），右侧为最小化/最大化/关闭，下方是标签栏；标题栏空白区可拖动窗口。
- **本地终端**：启动即打开本机 shell，无需连接；新建标签页对话框最上方固定"本地终端"入口。默认 shell 按平台选择：Linux/macOS 用系统默认（`$SHELL`），**Windows 用 PowerShell**（优先 PowerShell 7 `pwsh`，否则系统自带 `powershell.exe`）。
- 多标签页；新建标签页时选择连接项。**标签页不显示关闭按钮**（防误触），右键菜单关闭；检测到疑似运行中的程序时弹出确认。
- **分屏**（水平/垂直，可拖拽调整比例），分屏**复用当前连接**（同一 TCP 连接上开新 channel）。
- 现代复制粘贴：选中即复制（可关）、Ctrl+Shift+C/V、右键菜单。
- **双击文件名 → 预览**（文本/图片，SFTP 限长读取）。
- **悬停文件名 → 元信息浮窗**（类型/大小/权限/属主/修改时间）。
- **右键文件名 → 下载**（保存对话框 + 进度条）。
- **拖拽文件进窗口 → 上传到当前工作目录**（落在哪个分屏就传到它的 cwd）；工具栏也有上传按钮。
- **智能拖放落点**：拖到终端输出里的**目录名**上时该词高亮为选中态，释放即上传到该目录；拖到空白或文件词上则上传到当前目录。
- **本地文件管理器**：工具栏文件夹图标打开右侧浮动面板（45% 宽），**每个标签页独立实例**、目录状态各自保留；条目可拖到终端上传，或右键上传。
- **文件夹上传/下载**：目录自动递归遍历（跳过符号链接），聚合进度按字节展示。
- **TUI 滚轮支持**：已针对 vim/less/man 等进入备用屏幕的全屏 TUI 程序实现滚轮接管——鼠标滚轮或触摸板双指滚动自动转为 `<C-E>`/`<C-Y>` 视口滚动键发往应用程序（vim 默认滚轮即此绑定，光标不动；less/man 同样识别）。触摸板高频事件以 rAF 合并、亚行级像素余数累积，避免输入队列拥塞与缓慢滚动吞距离。若应用自行开启鼠标模式（如 vim `set mouse=a`），xterm.js 会先行拦截，滚轮走原生鼠标转义序列，本逻辑不介入。
- **新建标签页行为可配置**："+" 默认直接开本地终端，可在设置改为弹出连接选择；工具栏连接图标始终弹出连接选择。

### 内建命令

本地终端随包附带以下命令（安装到用户配置目录 `bin/` 下，追加到 PATH 末尾，不影响系统同名命令）：

- **`hssh <连接项名称>`** / **`hssh 用户@主机 [选项]`** — 在应用内打开自实现 SSH 连接（带远程文件面板），等价于在连接面板点击。支持 `--prod` 模式（连接后自动执行命令并可选退出）、`--file`/`--stdin` 批量喂入、`hsshprod` 快捷别名。
- **`hexit`** — 直接退出当前 HetuShell 窗口（跳过确认弹窗，仍保存会话快照，下次启动恢复）。不影响其他 HetuShell 实例。
- **`himage <图片路径|目录> [...]`** — 在终端内弹出图片查看器。
  - 图片文件直接打开；目录自动扫描图片（按名排序，上限 300 张）；多参数混合支持。
  - 80% 屏幕居中模态弹窗，毛玻璃透明背景，支持最大化/还原。
  - 缩放（滚轮/按钮）、旋转（90° 步进）、拖拽平移、适应窗口/100% 双击切换。
  - **底色自动检测**：透明 PNG/WebP/GIF/ICO/SVG 自动默认棋盘格底色，普通图片无底色；可手动切换主题/棋盘格/白/黑。
  - 多图左右切换（◀▶ 按钮 / ← → 键盘），标题栏显示索引（如 3/300）。
  - **缩略图侧边栏**：▤ 按钮切换，默认不显示，IntersectionObserver 懒加载。
  - ESC 关闭（多图时弹确认，单图直接关）。

> 以上命令仅本地终端有效，通过 OSC 转义序列 + 随机令牌校验与前端通信，终端中被渲染的不可信内容无法伪造。

### 外观
- **字体随应用内置分发**（woff2 内嵌，无需系统安装）：`JetBrains Mono NL`（Light/Regular/Bold + 斜体）+ `Noto Sans SC` 可变字体（CJK，100–900 全字重）；两者均为 SIL OFL 协议。主字体/CJK 字体/字号/字重均可改为任意本机字体。
- **内置 33 套主流主题**：暗色 22（One Dark、Dracula、Nord、Gruvbox、Solarized、Tokyo Night、Catppuccin Mocha、Monokai、GitHub Dark、Ayu、Rosé Pine、Rosé Pine Moon、Kanagawa、Everforest、Night Owl、Palenight、Tomorrow Night、Oceanic Next、Snazzy、Cobalt2、Zenburn…）+ 亮色 11（Solarized Light、GitHub Light、Catppuccin Latte、Gruvbox Light、Rosé Pine Dawn、One Light…），按名称排序；可**基于任一主题新建、编辑、删除自定义主题**（逐色编辑），全部持久化。
- **标题栏颜色默认跟随主题**，也可在设置中单独取色。
- **背景透明度**可调 + **毛玻璃虚化**：macOS（vibrancy）/ Windows（acrylic）原生模糊；应用内另有 backdrop 层，即使很透明也能看清终端内容。Linux 下窗口级模糊取决于合成器（KDE 可为透明窗口配置模糊规则）。
- **字体锐化（MCR）自适应**：暗色主题下按透明度四档自动调整 minimumContrastRatio——高不透明(>0.83) 1.61、中(≥0.6) 1.57、中透明(≥0.4) 1.3、高透明(<0.4) 1.1——在保持文字清晰的同时避免高透明度下暗色 ANSI 出现白边。亮色主题固定 1.1。
- **光标样式可配置**：方块（默认）或竖线（2px），光标颜色可自定义或跟随主题，设置面板支持重置。
- **图片预览上限可配置**：单张图片预览大小上限 32–512MB（默认 128MB），在设置面板中调节。

## 构建与打包

### 方式一：GitHub Actions 云端打包（推荐，本地无需 Rust）

仓库已内置 `.github/workflows/release.yml`。推送到 GitHub 后打一个 tag 即可，
CI 会在 Linux / Windows / macOS(Intel+Apple Silicon) 四个环境自动构建，
并把 deb/rpm/AppImage/msi/exe/dmg 全部上传到 Release 草稿：

```bash
git tag v0.1.0
git push origin v0.1.0
```

也可在 GitHub 页面 Actions → release → Run workflow 手动触发。CI 会在四个环境并行构建：
Ubuntu（deb/rpm/AppImage）、Windows（msi/nsis exe，自动携带 WebView2 引导安装）、
macOS Apple Silicon 与 macOS Intel（各出 dmg/app）。Windows/macOS 安装包**只能在对应系统上构建**
（桌面包不支持交叉编译），这正是用 CI 的原因。

### 方式二：本地构建（需要 Rust 环境）

```bash
# 1. Rust 工具链（一次性）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 2. Linux 另需系统库（Debian/Ubuntu 示例；Windows 需 VS Build Tools，macOS 需 Xcode CLT）
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
  librsvg2-dev build-essential pkg-config libssl-dev

# 3. 构建
npm install
npx tauri dev      # 开发运行（热更新）
npx tauri build    # 打包，产物在 src-tauri/target/release/bundle/
```

注意：桌面安装包不支持交叉编译——Windows 包必须在 Windows 上构建、macOS 包必须在 macOS 上构建，这正是方式一的价值。

## 故障排查

### AppImage 无法运行 / 提示二进制不完整

AppImage 的启动器依赖 **FUSE 2（libfuse.so.2）**；较新的发行版（Ubuntu 22.04+、Fedora 等）默认只装
FUSE 3，导致 AppImage 启动失败，报错常被显示为"二进制文件不完整/无法执行"之类。三种解法任选其一：

```bash
# A. 免 FUSE 直接解压运行（最省事）
./HetuShell_0.1.0_amd64.AppImage --appimage-extract-and-run

# B. 安装 FUSE 2
sudo apt install libfuse2        # Debian/Ubuntu
sudo dnf install fuse-libs       # Fedora

# C. 直接用 deb / rpm（不依赖 FUSE，推荐）
sudo dpkg -i HetuShell_0.1.0_amd64.deb
```

若怀疑文件在传输中被截断，可校验（构建时会随产物给出 sha256）：
`sha256sum HetuShell_0.1.0_amd64.AppImage`。AppImage 是有效文件的话，前 12 字节为
`7f454c46 02010100 41490200`（ELF 头 + AppImage type-2 魔数 `AI\x02`）。

### 界面渲染异常（花屏 / 空白 / 卡顿）

Linux 上 HetuShell 使用系统自带的 **WebKitGTK**（Tauri/WRY 的机制，见下）。个别显卡/驱动组合下
可尝试禁用合成或 DMA-BUF 渲染：

```bash
WEBKIT_DISABLE_DMABUF_RENDERER=1 ./HetuShell        # 常见花屏修复
WEBKIT_DISABLE_COMPOSITING_MODE=1 ./HetuShell
```

### 关于「自选 WebView 内核」

Tauri（底层 WRY）刻意复用**操作系统自带的 WebView**——Linux 用 WebKitGTK、Windows 用 WebView2、
macOS 用 WKWebView——以换取几 MB 的极小体积。因此 Linux 上**不能像 Electron 那样内置 Chromium**；
可控的只是所用 WebKitGTK 的版本（升级系统的 `webkit2gtk-4.1` 即可获得更新的内核）。若确实需要
统一的 Chromium 内核，只能改用 Electron/CEF 技术栈重构，会牺牲体积优势。Servo/Verso 作为 WRY 的
实验性后端仍不成熟，暂不适合生产。

配置保存在每个用户各自的配置目录 `hetushell/` 下（Linux: `~/.config/hetushell/`，
macOS: `~/Library/Application Support/hetushell/`，Windows: `%APPDATA%\hetushell\`），按用途分文件：
- `settings.json` — 界面与行为偏好
- `profiles.json` — 连接项（名称、主机、认证方式、私钥内容、备注等；**不含密码/口令**，文件权限 0600）
- `session.json` — 最后的会话（仅记录标签页来源的连接项 id，据此从 profiles.json 取回参数恢复；临时/手输或未保存凭据的连接不恢复）
- `known_hosts.json` — 主机指纹（TOFU）

窗口大小/位置由 `tauri-plugin-window-state` 自动记忆并在下次启动恢复。不同系统用户互不影响。

## 当前工作目录（cwd）追踪说明

上传/预览/下载解析相对路径依赖远端 shell 通过 **OSC 7** 上报 cwd；未上报时回退到远端 home。
建议在远端 shell 配置中加入（bash 示例）：

```bash
PROMPT_COMMAND='printf "\e]7;file://%s%s\e\\" "$HOSTNAME" "$PWD"'
```

zsh 用户可用 `precmd` 钩子；较新的发行版（及 fish）通常已默认上报。

## 快捷键

| 快捷键 | 功能 |
|---|---|
| Ctrl+Shift+T | 新建标签页 |
| 顶部「标识连接」图标 | 各终端中央浮层显示连接名与地址（5 秒） |
| Ctrl+Alt+R | 向右切分（当前终端变为左右两个） |
| Ctrl+Alt+D | 向下切分（当前终端变为上下两个） |
| Alt+方向键 | 在分屏之间切换焦点 |
| Ctrl+Shift+W | 关闭当前分屏 |
| Ctrl+Shift+C / V | 复制 / 粘贴 |

在任一终端上**右键**可「打开/切换连接」：已连接的终端选择新目标即就地切换，本地终端则连接到所选主机；顶部连接图标同样作用于当前聚焦的终端。

分屏支持嵌套（子终端可继续切分，最多 5 级）；分屏内 `exit` 或右键关闭后，相邻终端自动占据其空间。

## 架构

```
src/            前端（TypeScript + xterm.js，无重框架）
  main.ts       装配：事件路由、上传下载、快捷键
  pane.ts       终端分屏单元：xterm ↔ PTY 桥接、OSC7、悬停/双击/右键
  layout.ts     分屏二叉树布局
  tabs.ts       标签页管理
  dialogs.ts    连接/设置对话框
  themes.ts     主题系统
src-tauri/src/  后端（Rust）
  ssh/conn.rs   连接建立、单凭据认证、TOFU 指纹、自动重连
  ssh/pane.rs   PTY channel 事件循环
  ssh/sftp.rs   stat/预览/上传/下载
  sshcfg.rs     ~/.ssh/config 解析导入
  settings.rs   设置与连接项持久化
```
