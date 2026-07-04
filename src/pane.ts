/**
 * Pane：一个终端分屏单元 = xterm 实例 + 后端 PTY channel。
 * 负责：输入输出桥接、OSC7 cwd 追踪、悬停元信息、双击预览、繁忙探测。
 */

import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { api, b64decode, b64encode } from "./ipc";
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
  lastOutputAt = 0;
  private fit: FitAddon;
  private resizeObserver: ResizeObserver;
  private hoverTimer: number | undefined;
  private statCache = new Map<string, { meta: FileMeta | null; at: number }>();
  private disposed = false;

  onFocus: (() => void) | null = null;
  /** 请求以本 pane 为目标切分（row=左右两个，col=上下两个） */
  onSplitRequest: ((dir: "row" | "col") => void) | null = null;
  /** 请求把焦点移到相邻分屏（Alt+方向键） */
  onFocusNeighbor: ((dir: "left" | "right" | "up" | "down") => void) | null = null;
  onPreview: ((path: string) => void) | null = null;
  onTooltip: ((meta: FileMeta | null, x: number, y: number) => void) | null = null;
  onContextMenu: ((e: MouseEvent, word: string | null) => void) | null = null;

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
      fontWeight: s.fontWeight as never,
      cursorBlink: true,
      scrollback: 10000,
      theme: { ...activeTheme().colors, background: "#00000000" } as never,
      allowTransparency: true,
    });
    this.fit = new FitAddon();
    this.term.loadAddon(this.fit);
    this.term.loadAddon(new WebLinksAddon());
    this.term.open(this.element);
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

    // 复制即选中（可配置）
    this.term.onSelectionChange(() => {
      const sel = this.term.getSelection();
      if (sel && getSettings().copyOnSelect) void this.copyText(sel);
    });

    // 快捷键拦截（返回 false 阻止按键透传给远端 shell）：
    // Ctrl+Shift+C/V 复制粘贴；Ctrl+Alt+R 向右切分、Ctrl+Alt+D 向下切分
    this.term.attachCustomKeyEventHandler((e) => {
      if (e.type !== "keydown") return true;
      if (e.ctrlKey && e.shiftKey && !e.altKey) {
        if (e.code === "KeyC") {
          const sel = this.term.getSelection();
          if (sel) void this.copyText(sel);
          return false;
        }
        if (e.code === "KeyV") {
          void this.pasteFromClipboard();
          return false;
        }
      }
      if (e.ctrlKey && e.altKey && !e.shiftKey) {
        if (e.code === "KeyR") {
          this.onSplitRequest?.("row");
          return false;
        }
        if (e.code === "KeyD") {
          this.onSplitRequest?.("col");
          return false;
        }
      }
      // Alt+方向键：切换分屏焦点（拦截，避免透传成 shell 的按词移动）
      if (e.altKey && !e.ctrlKey && !e.shiftKey) {
        const map: Record<string, "left" | "right" | "up" | "down"> = {
          ArrowLeft: "left",
          ArrowRight: "right",
          ArrowUp: "up",
          ArrowDown: "down",
        };
        const dir = map[e.code];
        if (dir) {
          this.onFocusNeighbor?.(dir);
          return false;
        }
      }
      return true;
    });

    this.element.addEventListener("mousedown", () => this.onFocus?.());

    // 双击 → 预览（xterm 已按词选中）
    this.element.addEventListener("dblclick", () => {
      window.setTimeout(() => {
        const sel = this.term.getSelection().trim();
        if (sel && !sel.includes("\n")) {
          const path = this.resolvePath(sel);
          if (path) this.onPreview?.(path);
        }
      }, 10);
    });

    // 悬停元信息（防抖 400ms）
    this.element.addEventListener("mousemove", (e) => {
      window.clearTimeout(this.hoverTimer);
      this.onTooltip?.(null, e.clientX, e.clientY);
      this.hoverTimer = window.setTimeout(() => void this.hoverStat(e), 400);
    });
    this.element.addEventListener("mouseleave", () => {
      window.clearTimeout(this.hoverTimer);
      this.onTooltip?.(null, 0, 0);
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
    this.refit();
    if (this.isLocal) {
      await api.paneOpenLocal(this.id, this.term.cols, this.term.rows);
      return;
    }
    await api.paneOpen(this.connId, this.id, this.term.cols, this.term.rows);
    if (!this.homeDir) {
      // 仅记录 home 作为兜底；不写入 cwd——cwd 只反映 OSC7 上报的真实工作目录，
      // 以便上传等操作能区分“已知目录”与“home 猜测”，避免静默传错位置。
      api.remoteHome(this.connId).then((h) => {
        this.homeDir = h;
      }).catch(() => {});
    }
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
    this.statCache.clear();
    this.term.reset();
    await this.open();
    this.focus();
  }

  /** cwd 是否由 shell（OSC7）真实上报，而非 home 兜底猜测 */
  get cwdKnown(): boolean {
    return this.cwd !== null;
  }

  /** 上传/写入操作的目标目录：已知 cwd 优先，否则 home 兜底并标记为 guessed */
  uploadDir(): { dir: string | null; guessed: boolean } {
    if (this.cwd) return { dir: this.cwd, guessed: false };
    return { dir: this.homeDir, guessed: true };
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
