/**
 * Pane：一个终端分屏单元 = xterm 实例 + 后端 PTY channel。
 * 负责：输入输出桥接、OSC7 cwd 追踪、悬停元信息、双击预览、繁忙探测。
 */

import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { api, b64decode, b64encode } from "./ipc";
import { installImeGuard } from "./imeGuard";
import { getSettings, activeTheme, fontStack } from "./settings";
import type { FileMeta } from "./types";

export interface WordRange {
  word: string;
  row: number; // 视口内行号
  startCol: number;
  endCol: number; // 含
}

/** 从终端某行文本中提取 col 处的“词”（路径/文件名候选）及其列区间 */
function wordRangeAt(line: string, col: number): { word: string; start: number; end: number } | null {
  if (col >= line.length) return null;
  const isSep = (ch: string) => /[\s"'`()[\]{}<>|;:*?=]/.test(ch);
  if (isSep(line[col] ?? " ")) return null;
  let start = col;
  let end = col;
  while (start > 0 && !isSep(line[start - 1])) start--;
  while (end < line.length - 1 && !isSep(line[end + 1])) end++;
  // 去掉行尾标点（ls -F 的 / 保留，目录判断需要）
  let word = line.slice(start, end + 1);
  const trimmed = word.replace(/[,.:;]+$/, "");
  end -= word.length - trimmed.length;
  word = trimmed;
  return word ? { word, start, end } : null;
}

export class Pane {
  readonly id: string;
  /** 所属连接 id（可就地切换：断开旧 channel、指向新连接、重开 shell） */
  connId: string;
  readonly term: Terminal;
  readonly element: HTMLElement;
  cwd: string | null = null;
  homeDir: string | null = null;
  /** 远端 shell 的 PID（连接时经隐形 OSC 标记捕获），用于 /proc 读实时 cwd */
  shellPid: number | null = null;
  lastOutputAt = 0;
  private fit: FitAddon;
  private resizeObserver: ResizeObserver;
  private hoverTimer: number | undefined;
  private statCache = new Map<string, { meta: FileMeta | null; at: number }>();
  private disposed = false;
  /** 当前 Ctrl 悬停命中的词（原始相对/绝对词，下载时再异步解析为绝对路径） */
  private ctrlHoverWord: string | null = null;

  onFocus: (() => void) | null = null;
  /** 全局快捷键分发：返回 true 表示已处理（不透传给 shell） */
  onAppKey: ((e: KeyboardEvent) => boolean) | null = null;
  onPreview: ((path: string) => void) | null = null;
  onTooltip: ((meta: FileMeta | null, x: number, y: number) => void) | null = null;
  onContextMenu: ((e: MouseEvent, word: string | null) => void) | null = null;
  /** Ctrl+单击终端中的文件/目录（下载） */
  onCtrlClick: ((path: string) => void) | null = null;
  /** Ctrl+拖拽终端中的文件/目录（拖到文件管理器下载） */
  onCtrlDragStart: ((path: string, e: DragEvent) => void) | null = null;

  constructor(id: string, connId: string) {
    this.id = id;
    this.connId = connId;
    this.element = document.createElement("div");
    this.element.className = "pane";
    this.element.dataset.paneId = id;

    const s = getSettings();
    this.term = new Terminal({
      allowProposedApi: true,
      fontFamily: fontStack(),
      fontSize: s.fontSize,
      // 字重由字体名承载（如 "…Light"），终端统一 normal，避免把 Light 强加到 CJK
      fontWeight: "normal" as never,
      cursorBlink: true,
      scrollback: 10000,
      theme: { ...activeTheme().colors, background: "#00000000" } as never,
      allowTransparency: true,
    });
    this.fit = new FitAddon();
    this.term.loadAddon(this.fit);
    this.term.loadAddon(new WebLinksAddon());
    this.term.open(this.element);
    // WebKitGTK 下 CJK 输入法重复/残留修复（详见 imeGuard.ts 根因注释）
    installImeGuard(this.element);
    void this.tryWebgl();

    // 输入 → 后端 PTY
    this.term.onData((data) => {
      void api.paneInput(this.id, b64encode(data)).catch(() => {});
    });

    // OSC 7：shell 上报 cwd（file://host/path）
    this.term.parser.registerOscHandler(7, (data) => {
      try {
        const url = new URL(data);
        if (url.protocol === "file:") this.cwd = decodeURIComponent(url.pathname);
      } catch {
        /* 非标准载荷，忽略 */
      }
      return true;
    });

    // OSC 5379（私有）：连接时注入的隐形标记，捕获远端 shell PID，用于 /proc 读实时 cwd
    this.term.parser.registerOscHandler(5379, (data) => {
      const pid = parseInt(data, 10);
      if (Number.isFinite(pid) && pid > 0) this.shellPid = pid;
      return true;
    });

    // 复制即选中（可配置）
    this.term.onSelectionChange(() => {
      const sel = this.term.getSelection();
      if (sel && getSettings().copyOnSelect) void this.copyText(sel);
    });

    // 快捷键拦截：命中全局快捷键则返回 false，阻止按键透传给远端 shell（也防止终端聚焦
    // 时窗口监听重复触发——快捷键统一由这里在终端聚焦时处理，窗口监听仅兜底焦点在外）。
    this.term.attachCustomKeyEventHandler((e) => {
      if (e.type !== "keydown") return true;
      return this.onAppKey?.(e) ? false : true;
    });

    this.element.addEventListener("mousedown", () => this.onFocus?.());

    // 双击 → 预览（xterm 已按词选中）
    this.element.addEventListener("dblclick", () => {
      window.setTimeout(() => {
        const sel = this.term.getSelection().trim();
        if (sel && !sel.includes("\n")) this.onPreview?.(sel);
      }, 10);
    });

    // 悬停元信息（防抖 400ms）+ Ctrl 悬停链接态
    this.element.addEventListener("mousemove", (e) => {
      window.clearTimeout(this.hoverTimer);
      this.onTooltip?.(null, e.clientX, e.clientY);
      this.updateCtrlHover(e);
      this.hoverTimer = window.setTimeout(() => void this.hoverStat(e), 400);
    });
    this.element.addEventListener("mouseleave", () => {
      window.clearTimeout(this.hoverTimer);
      this.onTooltip?.(null, 0, 0);
      this.clearCtrlHover();
    });

    // Ctrl+单击文件/目录 → 下载（若未拖拽）
    this.element.addEventListener("click", (e) => {
      if (e.ctrlKey && this.ctrlHoverWord) {
        e.preventDefault();
        this.onCtrlClick?.(this.ctrlHoverWord);
      }
    });
    // Ctrl+拖拽文件/目录 → 拖到文件管理器下载到对应目录
    this.element.addEventListener("dragstart", (e) => {
      if (this.ctrlHoverWord && this.onCtrlDragStart) {
        this.onCtrlDragStart(this.ctrlHoverWord, e);
      } else {
        e.preventDefault();
      }
    });

    this.element.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      const word = this.term.getSelection().trim() || this.wordUnderMouse(e);
      this.onContextMenu?.(e, word || null);
    });

    this.resizeObserver = new ResizeObserver(() => this.refit());
    this.resizeObserver.observe(this.element);
  }

  private async tryWebgl() {
    try {
      const { WebglAddon } = await import("@xterm/addon-webgl");
      this.term.loadAddon(new WebglAddon());
    } catch {
      /* WebGL 不可用时回退 canvas 渲染 */
    }
  }

  /** 本地终端 pane（无 SSH/SFTP 能力） */
  get isLocal(): boolean {
    return this.connId === "local";
  }

  /** 打开（或断线后重开）后端 PTY channel */
  async open(): Promise<void> {
    // 首帧 fit 放到下一帧：新建标签时不在点击处理里同步强制重排（消除「新建标签有点卡」）。
    // PTY 先以当前 cols/rows 启动，下一帧 fit 后经 paneResize 校正（SIGWINCH），观感无损；
    // 重连场景终端已保留上次尺寸，cols/rows 本就正确，延后一帧亦无副作用。
    requestAnimationFrame(() => this.refit());
    if (this.isLocal) {
      await api.paneOpenLocal(this.id, this.term.cols, this.term.rows);
      return;
    }
    // 每次（重）打开远端 shell 都是新进程：PID 变、cwd 回到 home，需重新捕获追踪
    this.shellPid = null;
    this.cwd = null;
    await api.paneOpen(this.connId, this.id, this.term.cols, this.term.rows);
    if (!this.homeDir) {
      // 仅记录 home 作为兜底；不写入 cwd——cwd 只反映真实上报的工作目录，
      // 以便上传等操作能区分“已知目录”与“home 猜测”，避免静默传错位置。
      api.remoteHome(this.connId).then((h) => {
        this.homeDir = h;
      }).catch(() => {});
    }
    this.injectCwdTracker();
  }

  /**
   * 注入一次隐形标记，让远端 shell 用 OSC 5379 上报自己的 PID（任何 shell 都有 $$）。
   * printf 直接输出控制序列（不显示为文本），并用光标上移+清行擦掉命令行回显，尽量无痕。
   * 之后需要 cwd 时按需 realpath /proc/PID/cwd（见 uploadDir）。
   */
  private injectCwdTracker() {
    if (this.isLocal || this.shellPid || !getSettings().trackRemoteCwd) return;
    // 组装发送给 shell 的命令文本（这些是"敲入"的字符；printf 再把 \033 转成 ESC 字节）。
    // 用常量拼接避免反斜杠数目算错。printf 输出：OSC(上报 PID) + 光标上移+清行(擦掉回显)。
    const ESC = "\\033"; // 4 个字符 \0 3 3 → shell → printf → ESC
    const cmd =
      " printf '" +
      ESC + "]5379;%s" + // OSC 私有码 + PID 占位
      ESC + "\\\\" + //   ESC + \\ → ST（ESC \），结束 OSC
      ESC + "[1A" + //    光标上移一行（到命令回显行）
      ESC + "[2K" + //    清除整行（擦掉回显的命令）
      "\\r" + //          回车到行首
      "' \"$$\"\n"; //    传入 $$（shell 自身 PID）并回车执行
    // 稍等 shell 就绪再发，使自清除相对首个提示符生效
    window.setTimeout(() => {
      if (!this.disposed && !this.shellPid) {
        void api.paneInput(this.id, b64encode(cmd)).catch(() => {});
      }
    }, 450);
  }

  /**
   * 就地切换到另一条连接：关闭旧 channel、清空主机相关状态、指向新连接并重开 shell。
   * newConnId 需已建立（SSH）或为 "local"。
   */
  async switchConnection(newConnId: string): Promise<void> {
    await api.paneClose(this.id).catch(() => {});
    this.connId = newConnId;
    this.cwd = null;
    this.homeDir = null;
    this.shellPid = null; // 旧主机的 shell PID 不能用于新连接的 /proc 读取
    this.statCache.clear();
    this.term.reset();
    await this.open();
    this.focus();
  }

  /** cwd 是否由 shell（OSC7）真实上报，而非 home 兜底猜测 */
  get cwdKnown(): boolean {
    return this.cwd !== null;
  }

  /**
   * 当前工作目录，按可靠性优先：
   * 1) OSC7 上报的 cwd；2) 经 shell PID 从 /proc 读到的实时 cwd；3) home 兜底。
   * guessed=true 表示落到了 home 兜底（并非 shell 真实所在目录）。
   */
  async currentDir(): Promise<{ dir: string | null; guessed: boolean }> {
    if (this.cwd) return { dir: this.cwd, guessed: false };
    if (this.shellPid) {
      const cwd = await api.remoteCwd(this.connId, this.shellPid).catch(() => null);
      if (cwd) return { dir: cwd, guessed: false };
    }
    return { dir: this.homeDir, guessed: true };
  }

  /** 上传目标目录（语义同 currentDir，保留原调用名） */
  async uploadDir(): Promise<{ dir: string | null; guessed: boolean }> {
    return this.currentDir();
  }

  /**
   * 本地终端的实时工作目录（拖拽下载落点用）：
   * 优先 OSC7 上报的 cwd，否则经后端 /proc/<本地 shell pid>/cwd 读取；均失败返回 null。
   * 非本地 pane 返回 null。
   */
  async resolveLocalCwd(): Promise<string | null> {
    if (!this.isLocal) return null;
    if (this.cwd) return this.cwd;
    return api.localCwd(this.id).catch(() => null);
  }

  /**
   * 相对词 → 远端绝对路径（异步）。相对名基于实时 cwd（OSC7 → /proc → home），
   * 因此即便 shell 未上报 OSC7，只要拿到远端 PID 也能拼出用户 `cd` 后的真实路径。
   * 已是绝对路径（/ 或 ~/）则原样返回，故传入已解析的绝对路径亦幂等安全。
   */
  async resolveRemotePath(word: string): Promise<string | null> {
    if (!word || word.includes("\n")) return null;
    if (word.startsWith("/")) return word;
    if (word.startsWith("~/")) {
      return this.homeDir ? `${this.homeDir}/${word.slice(2)}` : null;
    }
    const { dir } = await this.currentDir();
    if (!dir) return null;
    return `${dir.replace(/\/$/, "")}/${word.replace(/^\.\//, "")}`;
  }

  write(dataB64: string) {
    this.lastOutputAt = Date.now();
    this.term.write(b64decode(dataB64));
  }

  /** 繁忙启发式：最近 1.2s 内仍有输出，视为有程序在运行 */
  get busy(): boolean {
    return Date.now() - this.lastOutputAt < 1200;
  }

  refit() {
    if (this.disposed || !this.element.isConnected) return;
    try {
      this.fit.fit();
      void api.paneResize(this.id, this.term.cols, this.term.rows).catch(() => {});
    } catch {
      /* 布局未就绪 */
    }
  }

  focus() {
    this.term.focus();
  }

  async copyText(text: string) {
    try {
      const { writeText } = await import("@tauri-apps/plugin-clipboard-manager");
      await writeText(text);
    } catch {
      await navigator.clipboard.writeText(text).catch(() => {});
    }
  }

  async pasteFromClipboard() {
    let text = "";
    try {
      const { readText } = await import("@tauri-apps/plugin-clipboard-manager");
      text = await readText();
    } catch {
      text = await navigator.clipboard.readText().catch(() => "");
    }
    if (text) this.term.paste(text);
  }

  /** 相对词 → 远端绝对路径（基于 OSC7 cwd 或 home 兜底） */
  resolvePath(word: string): string | null {
    if (!word || word.includes("\n")) return null;
    if (word.startsWith("/")) return word;
    if (word.startsWith("~/")) {
      return this.homeDir ? `${this.homeDir}/${word.slice(2)}` : null;
    }
    const base = this.cwd ?? this.homeDir;
    if (!base) return null;
    return `${base.replace(/\/$/, "")}/${word.replace(/^\.\//, "")}`;
  }

  /** xterm 渲染单元格尺寸（内部 API，主流终端封装均如此取用） */
  private cellDims(): { width: number; height: number } | null {
    const core = (this.term as unknown as { _core: any })._core;
    const dims = core?._renderService?.dimensions?.css?.cell;
    return dims?.width && dims?.height ? dims : null;
  }

  /** 屏幕坐标 → 终端单元格 */
  private cellAt(clientX: number, clientY: number): { col: number; row: number } | null {
    const dims = this.cellDims();
    const rect = this.element.querySelector(".xterm-screen")?.getBoundingClientRect();
    if (!dims || !rect) return null;
    const col = Math.floor((clientX - rect.left) / dims.width);
    const row = Math.floor((clientY - rect.top) / dims.height);
    if (col < 0 || row < 0 || col >= this.term.cols || row >= this.term.rows) return null;
    return { col, row };
  }

  /** 屏幕坐标 → 该处的词及列区间 */
  wordRangeAtPoint(clientX: number, clientY: number): WordRange | null {
    const cell = this.cellAt(clientX, clientY);
    if (!cell) return null;
    const buf = this.term.buffer.active;
    const line = buf.getLine(buf.viewportY + cell.row)?.translateToString(true);
    if (!line) return null;
    const r = wordRangeAt(line, cell.col);
    return r ? { word: r.word, row: cell.row, startCol: r.start, endCol: r.end } : null;
  }

  /** 鼠标位置 → 该处的词 */
  wordUnderMouse(e: MouseEvent): string | null {
    return this.wordRangeAtPoint(e.clientX, e.clientY)?.word ?? null;
  }

  /** Ctrl 悬停在文件/目录词上时：手形光标 + 可拖拽（仅远程 pane 有意义） */
  private updateCtrlHover(e: MouseEvent) {
    if (this.isLocal || !e.ctrlKey) {
      this.clearCtrlHover();
      return;
    }
    const word = this.wordUnderMouse(e);
    // 样式判定用同步 resolvePath 尽力而为；实际下载路径在点击/拖放时再异步解析。
    const w = word ? word.replace(/\/$/, "") : null;
    const linkable = !!(w && this.resolvePath(w));
    this.ctrlHoverWord = linkable ? w : null;
    this.element.classList.toggle("ctrl-link", linkable);
    this.element.draggable = linkable;
  }

  private clearCtrlHover() {
    if (this.ctrlHoverWord === null && !this.element.draggable) return;
    this.ctrlHoverWord = null;
    this.element.classList.remove("ctrl-link");
    this.element.draggable = false;
  }

  /** stat（带 5s 缓存），失败返回 null */
  async statPath(path: string): Promise<FileMeta | null> {
    const cached = this.statCache.get(path);
    if (cached && Date.now() - cached.at < 5000) return cached.meta;
    const meta = await api.sftpStat(this.connId, path).catch(() => null);
    this.statCache.set(path, { meta, at: Date.now() });
    if (this.statCache.size > 200) this.statCache.clear();
    return meta;
  }

  // ---------- 拖放目标高亮（拖到输出中的目录名上 → 选中态） ----------

  private dropHl: HTMLElement | null = null;

  showDropHighlight(range: WordRange) {
    const dims = this.cellDims();
    const screen = this.element.querySelector(".xterm-screen")?.getBoundingClientRect();
    const host = this.element.getBoundingClientRect();
    if (!dims || !screen) return;
    if (!this.dropHl) {
      this.dropHl = document.createElement("div");
      this.dropHl.className = "drop-target-hl";
      this.element.appendChild(this.dropHl);
    }
    const s = this.dropHl.style;
    s.left = `${screen.left - host.left + range.startCol * dims.width - 2}px`;
    s.top = `${screen.top - host.top + range.row * dims.height - 1}px`;
    s.width = `${(range.endCol - range.startCol + 1) * dims.width + 4}px`;
    s.height = `${dims.height + 2}px`;
    s.display = "block";
  }

  clearDropHighlight() {
    if (this.dropHl) this.dropHl.style.display = "none";
  }

  /** 悬停 stat：词能解析为远端路径且存在 → 展示元信息 tooltip */
  private async hoverStat(e: MouseEvent) {
    if (this.isLocal) return;
    const word = this.wordUnderMouse(e);
    if (!word || word.length > 512) return;
    const path = this.resolvePath(word);
    if (!path) return;
    const meta = await this.statPath(path);
    if (meta) this.onTooltip?.(meta, e.clientX, e.clientY);
  }

  dispose() {
    if (this.disposed) return;
    this.disposed = true;
    this.resizeObserver.disconnect();
    void api.paneClose(this.id).catch(() => {});
    this.term.dispose();
    this.element.remove();
  }
}
