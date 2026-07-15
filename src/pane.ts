/**
 * Pane：一个终端分屏单元 = xterm 实例 + 后端 PTY channel。
 * 负责：输入输出桥接、OSC7 cwd 追踪、悬停元信息、双击预览、繁忙探测。
 */

import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { api, b64decode, b64encode } from "./ipc";
import { installImeGuard } from "./imeGuard";
import { getSettings, activeTheme, fontStack, computeMcr } from "./settings";
import type { FileMeta } from "./types";

/** hssh 内建命令通过此 OSC 标识符通知前端（见 local.rs 注入的 hssh 脚本）。 */
const HSSH_OSC = 1729;
/** hexit 内建命令通过此 OSC 标识符通知前端直接退出（无确认）。 */
const HEXIT_OSC = 1730;
/** himage 内建命令通过此 OSC 标识符通知前端弹出图片查看器。 */
const HIMAGE_OSC = 1731;
/** hfile 内建命令通过此 OSC 标识符通知前端打开文件管理器面板。 */
const HFILE_OSC = 1732;

// FitAddon 在 scrollback>0 时预留滚动条宽度：overviewRuler?.width || 14。
// 初始化时传 overviewRuler: { width: 10 } 覆盖默认 14，减少 4px 预留。
// 必须在构造时设置——运行时改 overviewRuler 会触发 OverviewRulerRenderer 重建。

/** hssh 解析出的连接意图（tok 为来源校验令牌）：直连已保存连接项，或临时连接。
 *  feedPath/exitAfter 为自动化喂入字段，旧版 OSC 不携带时为 undefined/false（向后兼容）。 */
export type HsshSpec =
  | { tok: string; mode: "list" }
  | { tok: string; mode: "profile"; name: string; feedPath?: string; exitAfter?: boolean; quiet?: boolean; debug?: boolean }
  | { tok: string; mode: "adhoc"; host: string; user: string; port: string; password: string; identity: string; feedPath?: string; exitAfter?: boolean; quiet?: boolean; debug?: boolean };

/** hfile 命令解析结果（tok 为来源校验令牌） */
export type HfileSpec = {
  withShell: boolean;
  dir: string | null;
  remote: string | null;
};

/** base64 → UTF-8 字符串（hssh 各字段单独 base64，避免分隔符冲突）。失败返回空串。 */
function b64utf8(s: string): string {
  if (!s) return "";
  try {
    return new TextDecoder().decode(b64decode(s));
  } catch {
    return "";
  }
}

/** 解析 hssh OSC 载荷（`k=v;k=v`，值经 base64）。非法/未知一律返回 null。 */
export function parseHssh(data: string): HsshSpec | null {
  const f: Record<string, string> = {};
  for (const kv of data.split(";")) {
    const i = kv.indexOf("=");
    if (i > 0) f[kv.slice(0, i)] = kv.slice(i + 1);
  }
  const tok = b64utf8(f.tok);
  const feedPath = b64utf8(f.feed) || undefined;
  const exitAfter = b64utf8(f.exit) === "1";
  const quiet = b64utf8(f.quiet) === "1";
  const debug = b64utf8(f.debug) === "1";
  if (f.mode === "list") {
    return { tok, mode: "list" };
  }
  if (f.mode === "profile") {
    const name = b64utf8(f.name);
    return name ? { tok, mode: "profile", name, feedPath, exitAfter, quiet, debug } : null;
  }
  if (f.mode === "adhoc") {
    const host = b64utf8(f.host);
    if (!host) return null;
    return {
      tok,
      mode: "adhoc",
      host,
      user: b64utf8(f.user),
      port: b64utf8(f.port),
      password: b64utf8(f.pass),
      identity: b64utf8(f.ident),
      feedPath,
      exitAfter,
      quiet,
      debug,
    };
  }
  return null;
}

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
  /** 本地终端首开的起始目录（新标签/分屏继承源终端 cwd）；一次性消费，重开不再套用 */
  private initialCwd: string | null = null;
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
  /** 备用屏幕滚轮节流：rAF 合并帧内多次 wheel，避免触摸板高频 IPC 拥塞 PTY 输入队列 */
  private wheelRafId: number | null = null;
  /** 帧内待发送的累积行数（正=向下滚，负=向上滚），rAF 触发时一次性发出 */
  private wheelPending = 0;
  /** 像素余数累积：缓慢双指滚的亚行级 deltaY 累到下一帧，避免「吞距离」 */
  private wheelPixelCarry = 0;
  /** 远程应用是否启用了鼠标模式（DECSET 1000/1002/1003）。
   *  鼠标模式开时让 xterm core 编码并发送鼠标转义序列；关时由我们转 <C-E>/<C-Y>。 */
  mouseMode = false;
  /** 最近一次非空选中文本。TUI 应用（claude code 等）启用鼠标模式后，
   *  右键 mousedown 可能清除 xterm 选区，此时 getSelection() 返回空。
   *  缓存上次选中文本作为兜底，使右键"复制"和 Ctrl+Shift+C 仍可用。 */
  private lastSelection = "";
  /** hssh --debug 标志：进程退出时输出状态码提示。仅 hssh --prod --debug 场景为 true。 */
  debugExit = false;
  /** WebGL addon 引用，用于 dispose + 重建（Ctrl+Shift+R / onContextLoss） */
  private webglAddon: { dispose: () => void } | null = null;
  /** WebGL renderer 原型 patch 是否已应用（全局一次性，所有 pane 共享同一原型） */
  private static webglPatched = false;

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
  /** 内建 hssh 命令：仅本地终端触发，宿主据此按「点面板」路径打开连接 */
  onHssh: ((spec: HsshSpec, pane: Pane) => void) | null = null;
  /** 内建 hexit 命令：仅本地终端触发，宿主据此直接退出 HetuShell（跳过确认，仍保存会话） */
  onHexit: (() => void) | null = null;
  /** 内建 himage 命令：仅本地终端触发，paths 为图片绝对路径数组 */
  onHimage: ((paths: string[], withShell: boolean, pane: Pane) => void) | null = null;
  /** 内建 hfile 命令：仅本地终端触发，打开文件管理器面板 */
  onHfile: ((spec: HfileSpec, pane: Pane) => void) | null = null;
  /** hssh 来源校验令牌：随本地 shell 注入 $HSSH_TOKEN，OSC 载荷须回带一致值才受理，
   *  杜绝终端里被渲染的不可信内容伪造 hssh 序列诱导建连。每 pane 一枚随机值。 */
  private readonly hsshToken = crypto.randomUUID();

  constructor(id: string, connId: string, initialCwd?: string | null) {
    this.id = id;
    this.connId = connId;
    this.initialCwd = initialCwd ?? null;
    this.element = document.createElement("div");
    this.element.className = "pane";
    this.element.dataset.paneId = id;

    const s = getSettings();
    const theme = activeTheme();
    this.term = new Terminal({
      allowProposedApi: true,
      fontFamily: fontStack(),
      fontSize: s.fontSize,
      // 字重由字体名承载（如 "…Light"），终端统一 normal，避免把 Light 强加到 CJK
      fontWeight: "normal" as never,
      cursorBlink: true,
      cursorStyle: (s.cursorStyle === "bar" ? "bar" : "block") as never,
      cursorWidth: s.cursorStyle === "bar" ? 2 : undefined,
      scrollback: 12345,
      overviewRuler: { width: 10 },
      theme: (() => {
        const c: Record<string, string> = { ...theme.colors, background: "#00000000" };
        c.selectionBackground = "#8080806B";
        // 光标颜色：用户自定义优先（校验合法 hex），否则跟随主题（主题已含 cursor）
        const cc = s.cursorColor?.trim();
        if (cc && /^#[0-9a-fA-F]{6}$/.test(cc)) c.cursor = cc;
        return c as never;
      })(),
      allowTransparency: true,
      minimumContrastRatio: computeMcr(s, theme.base),
    });
    this.fit = new FitAddon();
    this.term.loadAddon(this.fit);
    // Web 链接：仅在按住 Ctrl 时用系统默认浏览器打开（普通单击不打开，避免误触）。
    // stopPropagation 掐断冒泡，防止远程 pane 的「Ctrl+单击下载文件」把 URL 当路径处理。
    this.term.loadAddon(
      new WebLinksAddon((e, uri) => {
        if (!e.ctrlKey) return;
        e.preventDefault();
        e.stopPropagation();
        void api.openExternal(uri).catch(() => {});
      }),
    );
    this.term.open(this.element);
    // WebKitGTK 下 CJK 输入法重复/残留修复（详见 imeGuard.ts 根因注释）
    installImeGuard(this.element);
    void this.tryWebgl();

    // hssh 内建命令（OSC 1729）：仅本地终端接受——安全上防止远程主机发同序列诱导客户端乱开连接。
    // 稳定优先：任何解析/回调异常都吞掉，且始终返回 true 避免原始转义序列回显污染终端。
    this.term.parser.registerOscHandler(HSSH_OSC, (data) => {
      try {
        // 双重门控：① 仅本地终端；② 载荷令牌须与本 pane 注入的 $HSSH_TOKEN 一致。
        // 不可信内容不知道该随机令牌 → 伪造序列一律被丢弃（失败关闭）。
        if (this.isLocal) {
          const spec = parseHssh(data);
          if (spec && spec.tok && spec.tok === this.hsshToken) this.onHssh?.(spec, this);
        }
      } catch {
        /* 忽略：绝不因辅助命令影响终端本身 */
      }
      return true;
    });

    // hexit 内建命令（OSC 1730）：仅本地终端接受，安全模型同 hssh。
    this.term.parser.registerOscHandler(HEXIT_OSC, (data) => {
      try {
        if (this.isLocal) {
          const f: Record<string, string> = {};
          for (const kv of data.split(";")) {
            const i = kv.indexOf("=");
            if (i > 0) f[kv.slice(0, i)] = kv.slice(i + 1);
          }
          const tok = b64utf8(f.tok ?? "");
          if (tok && tok === this.hsshToken) this.onHexit?.();
        }
      } catch {
        /* 忽略 */
      }
      return true;
    });

    // himage 内建命令（OSC 1731）：仅本地终端接受，安全模型同 hssh。
    this.term.parser.registerOscHandler(HIMAGE_OSC, (data) => {
      try {
        if (this.isLocal) {
          const f: Record<string, string> = {};
          for (const kv of data.split(";")) {
            const i = kv.indexOf("=");
            if (i > 0) f[kv.slice(0, i)] = kv.slice(i + 1);
          }
          const tok = b64utf8(f.tok ?? "");
          if (tok && tok === this.hsshToken) {
            const raw = b64utf8(f.paths ?? "");
            const paths = raw.split("\n").map((s) => s.trim()).filter(Boolean);
            if (paths.length > 0) this.onHimage?.(paths, (f.w ?? "0") === "1", this);
          }
        }
      } catch {
        /* 忽略 */
      }
      return true;
    });

    // hfile 内建命令（OSC 1732）：仅本地终端接受，安全模型同 hssh。
    this.term.parser.registerOscHandler(HFILE_OSC, (data) => {
      try {
        if (this.isLocal) {
          const f: Record<string, string> = {};
          for (const kv of data.split(";")) {
            const i = kv.indexOf("=");
            if (i > 0) f[kv.slice(0, i)] = kv.slice(i + 1);
          }
          const tok = b64utf8(f.tok ?? "");
          if (tok && tok === this.hsshToken) {
            this.onHfile?.(
              {
                withShell: (f.w ?? "0") === "1",
                dir: b64utf8(f.d ?? "") || null,
                remote: b64utf8(f.r ?? "") || null,
              },
              this,
            );
          }
        }
      } catch {
        /* 忽略 */
      }
      return true;
    });

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

    // 跟踪鼠标模式（DECSET ?1000/1002/1003 = 启用，DECRST = 禁用）：
    // 鼠标模式开时让 xterm core 编码鼠标转义序列（capture 阶段只 preventDefault 不
    // stopPropagation），关时由我们转 <C-E>/<C-Y>。返回 false 不阻止 xterm 默认处理。
    this.term.parser.registerCsiHandler({ prefix: "?", final: "h" }, (params) => {
      if (params[0] === 1000 || params[0] === 1002 || params[0] === 1003) {
        this.mouseMode = true;
      }
      return false;
    });
    this.term.parser.registerCsiHandler({ prefix: "?", final: "l" }, (params) => {
      if (params[0] === 1000 || params[0] === 1002 || params[0] === 1003) {
        this.mouseMode = false;
      }
      return false;
    });

    // 复制即选中（可配置）+ 缓存非空选区供右键/Ctrl+Shift+C 兜底
    this.term.onSelectionChange(() => {
      const sel = this.term.getSelection();
      if (sel) this.lastSelection = sel;
      if (sel && getSettings().copyOnSelect) void this.copyText(sel);
    });

    // mouseup 备份选区：鼠标模式下 onSelectionChange 可能不触发，
    // mouseup 时再检查一次，确保 lastSelection 捕获到 Shift+drag 选中的文本
    this.element.addEventListener("mouseup", () => {
      const sel = this.term.getSelection();
      if (sel) this.lastSelection = sel;
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
    // 无 ctrlHoverWord 时不 preventDefault：WebKitGTK 下 preventDefault(dragstart) 可能
    // 停止后续 mousemove 派发，导致 xterm.js 文本选择无法扩展（只能选中首字符）
    this.element.addEventListener("dragstart", (e) => {
      if (this.ctrlHoverWord && this.onCtrlDragStart) {
        this.onCtrlDragStart(this.ctrlHoverWord, e);
      }
    });

    this.element.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      // word 只取鼠标下的词，不用选中文本——选中文本是"要复制的内容"而非"要下载的文件"
      const word = this.wordUnderMouse(e);
      this.onContextMenu?.(e, word || null);
    });

    // 备用屏幕（vim/less/man/claude code/qwen code）下的滚轮处理：
    // 使用 capture 阶段：xterm v6 的 xterm-scrollable-element 会在 bubble 阶段
    // 调用 stopPropagation() 消费 wheel 事件，导致 bubble 监听器无法收到。
    //
    // 两种路径：
    // - 鼠标模式开（vim set mouse=a、claude code 等交互式 TUI）→ 只 preventDefault
    //   不 stopPropagation，让 xterm core 编码并发送鼠标转义序列给 PTY
    // - 鼠标模式关（less/man/普通 vim）→ preventDefault + stopPropagation，
    //   转为 <C-E>/<C-Y> 发送给 PTY
    //
    // 按键选择：发 <C-E>/<C-Y> 而非 ↑/↓。
    //  - vim 默认滚轮即绑定 <C-E>/<C-Y>（视口滚动，光标不动）；↑/↓ 在 normal 模式
    //    会让光标在文件里逐行移动、插入模式下更会跳行，与原生滚轮语义不符。
    //  - less/man 现代版本同样识别 <C-E>/<C-Y> 作为行级滚动，兼容不破坏。
    //
    // 触摸板（DOM_DELTA_PIXEL）高频事件用 rAF 合并到下一帧、并累积亚行级像素余数，
    // 避免一轮惯性滚动触发数十次 IPC 把 PTY 输入队列打满、并防止缓慢滚动吞距离。
    this.element.addEventListener(
      "wheel",
      (e) => {
        if (this.term.buffer.active.type !== "alternate") return;
        // 鼠标模式开：让 xterm core 编码鼠标转义序列，不干预事件传播
        if (this.mouseMode) return;
        // 鼠标模式关：阻止 xterm-scrollable-element 的默认滚动行为，转按键
        e.preventDefault();
        e.stopPropagation();

        let lines: number;
        switch (e.deltaMode) {
          case WheelEvent.DOM_DELTA_LINE:
            lines = Math.abs(e.deltaY);
            this.wheelPixelCarry = 0;
            break;
          case WheelEvent.DOM_DELTA_PAGE:
            lines = Math.abs(e.deltaY) * this.term.rows;
            this.wheelPixelCarry = 0;
            break;
          default: {
            // DOM_DELTA_PIXEL（触摸板双指滚动）：按单元格高度折算为行数，余数累积
            const ch = this.cellDims()?.height ?? 18;
            const px = Math.abs(e.deltaY) + this.wheelPixelCarry;
            lines = Math.floor(px / ch);
            this.wheelPixelCarry = px - lines * ch;
            break;
          }
        }
        if (lines <= 0) return;
        // 方向归一：向下滚（deltaY>0，文本上移、看到下方）→ +lines；向上滚 → -lines
        this.wheelPending += e.deltaY > 0 ? lines : -lines;

        if (this.wheelRafId !== null) return;
        this.wheelRafId = requestAnimationFrame(() => {
          this.wheelRafId = null;
          const total = this.wheelPending;
          this.wheelPending = 0;
          if (total === 0) return;
          // 单帧上限：超过一屏视为大 flicker，截到一屏避免 PTY 输入暴冲
          const clamped = Math.max(-this.term.rows, Math.min(total, this.term.rows));
          if (clamped === 0) return; // 终端未渲染/最小化时 rows=0，避免发空
          const key = clamped > 0 ? "\x05" : "\x19"; // <C-E> 下滚 / <C-Y> 上滚
          void api.paneInput(this.id, b64encode(key.repeat(Math.abs(clamped)))).catch(() => {});
        });
      },
      { capture: true, passive: false },
    );

    this.resizeObserver = new ResizeObserver(() => this.refit());
    this.resizeObserver.observe(this.element);
  }

  /**
   * 对 xterm.js WebGL addon 的渲染器原型做一次性 patch，修复 texture atlas
   * page merge 导致的同帧渲染不一致（xterm.js #4480）。
   *
   * Bug 时序：_updateModel 增量更新中途触发 page merge → 原地修改 glyph.texturePage
   * → 前半行用旧索引、后半行用新索引 → 顶点缓冲混合 → 乱码。
   * _requestClearModel 延迟到下一帧才生效，且原始 beginFrame() 从不重置该标志。
   *
   * Patch 1 — beginFrame：读取后重置 _requestClearModel，避免每帧全量重建。
   * Patch 2 — _updateModel：增量更新后检查 merge 是否触发，若是则立即全量重建，
   *           保证当前帧 model 与 atlas 状态一致。
   *
   * 所有 pane 共享同一原型，只需 patch 一次。
   */
  private patchWebglRenderer(renderer: any): void {
    if (Pane.webglPatched) return;
    const proto = Object.getPrototypeOf(renderer);
    const atlas = renderer._charAtlas;
    if (!atlas) return;

    // Patch 1: beginFrame — 读取后重置，原始代码从不重置导致每帧全量重建
    const atlasProto = Object.getPrototypeOf(atlas);
    atlasProto.beginFrame = function (this: any): boolean {
      const v = this._requestClearModel;
      this._requestClearModel = false;
      return v;
    };

    // Patch 2: _updateModel — 增量更新后若 merge 触发，立即全量重建
    const origUpdateModel = proto._updateModel;
    proto._updateModel = function (this: any, start: number, end: number): void {
      const isIncremental = start !== 0 || end !== this._terminal.rows - 1;
      origUpdateModel.call(this, start, end);
      if (isIncremental && this._charAtlas?._requestClearModel) {
        this._charAtlas._requestClearModel = false;
        this._clearModel(true);
        origUpdateModel.call(this, 0, this._terminal.rows - 1);
      }
    };

    Pane.webglPatched = true;
  }

  private async tryWebgl() {
    if (!getSettings().webgl) return;
    try {
      const { WebglAddon } = await import("@xterm/addon-webgl");
      const addon = new WebglAddon();
      // WebGL 上下文丢失（GPU 驱动重置等）时重建渲染器，否则字符纹理累积损坏导致乱码
      addon.onContextLoss(() => {
        addon.dispose();
        this.webglAddon = null;
        void this.tryWebgl();
      });
      this.term.loadAddon(addon);
      this.webglAddon = addon;
      // addon.activate 后 _renderer 才存在；下一帧取实例做原型 patch
      requestAnimationFrame(() => {
        const renderer = (addon as any)._renderer;
        if (renderer) this.patchWebglRenderer(renderer);
      });
    } catch {
      /* WebGL 不可用时回退 canvas 渲染 */
    }
  }

  /**
   * 重建 WebGL 渲染缓存（Ctrl+Shift+R）：dispose 当前 WebGL addon 并重新创建，
   * 获得全新的空 texture atlas + cache map，从现有 buffer 重建渲染。
   * TUI 应用不退出、对话记录不丢失，与 onContextLoss 走同一路径。
   */
  rebuildRenderer(): void {
    if (this.webglAddon) {
      this.webglAddon.dispose();
      this.webglAddon = null;
    }
    void this.tryWebgl();
    this.term.refresh(0, this.term.rows - 1);
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
      // 起始目录一次性消费：首开继承源终端 cwd，之后重开（切换连接等）回默认 home
      const startCwd = this.initialCwd;
      this.initialCwd = null;
      await api.paneOpenLocal(this.id, this.term.cols, this.term.rows, startCwd, this.hsshToken);
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
  async switchConnection(newConnId: string, preserve = false): Promise<void> {
    await api.paneClose(this.id).catch(() => {});
    this.connId = newConnId;
    this.cwd = null;
    this.homeDir = null;
    this.shellPid = null; // 旧主机的 shell PID 不能用于新连接的 /proc 读取
    this.statCache.clear();
    // preserve=true（SSH 退出回退本地）：不清屏，保留远程输出供用户查看
    if (!preserve) this.term.reset();
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
    const prevCols = this.term.cols, prevRows = this.term.rows;
    try {
      this.fit.fit();
    } catch {
      /* 布局未就绪 */
    }
    // 仅实际 resize 时才通知 PTY——冗余 SIGWINCH 会打扰 TUI 程序
    if (this.term.cols !== prevCols || this.term.rows !== prevRows) {
      void api.paneResize(this.id, this.term.cols, this.term.rows).catch(() => {});
    }
    // 始终刷新：DOM 移动或设置变更后 canvas 可能未重绘
    this.term.refresh(0, this.term.rows - 1);
  }

  focus() {
    this.term.focus();
  }

  /** 失去焦点：交给 xterm 原生 handleBlur（暂停光标闪烁 + 重绘选区色） */
  blur() {
    this.term.blur();
  }

  /** 当前选中文本；若无（如鼠标模式下右键清除了选区），回退到最近非空选区或 DOM 选区 */
  getSelectionText(): string {
    return this.term.getSelection() || this.lastSelection || document.getSelection()?.toString() || "";
  }

  async copyText(text: string) {
    if (!text) return;
    const tui = this.term.buffer.active.type === "alternate" && this.mouseMode;
    if (tui) {
      // TUI 模式（备用屏幕 + 鼠标模式）：避免 execCopy —— 其 DOM 操作（textarea 注入、
      // select、term.focus）会干扰 xterm 在鼠标模式下的交互状态，导致 TUI 应用重绘/重设。
      // 只用非侵入式通道：
      // ① navigator.clipboard.writeText — 大多数时候成功（WebKit → GTK → 系统剪贴板）
      // ② tauri 插件 writeText — arboard 直写系统剪贴板；与 pasteFromClipboard 的 readText
      //   同源（均走 arboard），确保读写一致，解决 navigator.clipboard 静默失效时的粘贴错位
      try {
        if (navigator.clipboard?.writeText) {
          await navigator.clipboard.writeText(text);
          return;
        }
      } catch {}
      try {
        const { writeText } = await import("@tauri-apps/plugin-clipboard-manager");
        await writeText(text);
      } catch (err) {
        console.error("[copy] TUI 写入剪贴板失败:", err);
      }
      return;
    }
    // 非 TUI 模式：原链路不变（navigator.clipboard → execCopy → tauri 插件）
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(text);
        return;
      }
    } catch (err) {
      console.warn("[copy] navigator.clipboard 不可用，回退 execCommand:", err);
    }
    if (this.execCopy(text)) return;
    try {
      const { writeText } = await import("@tauri-apps/plugin-clipboard-manager");
      await writeText(text);
    } catch (err) {
      console.error("[copy] 写入剪贴板失败（所有通道均失败）:", err);
    }
  }

  /**
   * 同步兜底复制：隐藏 textarea + document.execCommand('copy')。
   * 经 WebView 的 GTK 剪贴板，跨桌面通用、无需权限；须在用户手势内（快捷键/选中均满足）。
   * 复制后恢复原 DOM 选区并把焦点还给终端，避免打断输入。
   */
  private execCopy(text: string): boolean {
    try {
      const ta = document.createElement("textarea");
      ta.value = text;
      ta.setAttribute("readonly", "");
      ta.style.cssText = "position:fixed;top:-9999px;left:-9999px;opacity:0";
      document.body.appendChild(ta);
      const sel = document.getSelection();
      const prev = sel && sel.rangeCount > 0 ? sel.getRangeAt(0) : null;
      ta.select();
      const ok = document.execCommand("copy");
      ta.remove();
      if (prev && sel) {
        sel.removeAllRanges();
        sel.addRange(prev);
      }
      this.term.focus(); // execCommand 会夺走焦点，还给终端
      return ok;
    } catch {
      return false;
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
    // URL 交给 WebLinksAddon（Ctrl 打开浏览器），不当作可下载文件，避免光标/拖拽/点击冲突
    if (word && /^https?:\/\//i.test(word)) {
      this.clearCtrlHover();
      return;
    }
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
    if (this.wheelRafId !== null) {
      cancelAnimationFrame(this.wheelRafId);
      this.wheelRafId = null;
      this.wheelPending = 0;
    }
    this.resizeObserver.disconnect();
    void api.paneClose(this.id).catch(() => {});
    this.term.dispose();
    this.element.remove();
  }
}
