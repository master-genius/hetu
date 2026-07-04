/**
 * 文件资源管理器（右侧浮动面板内容），后端可插拔：
 * - local  ：列本机文件；行可拖到终端/远程面板上传，右键上传到终端目录。
 * - remote ：列某 SSH 连接的远端文件；行可拖到本地面板下载；接收本地条目拖入上传。
 * 每个实例保留自身目录状态（本地按标签页、远程按连接）。
 */

import { api } from "./ipc";
import { formatSize, showMenu, toast } from "./ui";
import type { LocalEntry } from "./types";

/** 拖拽数据的自定义 MIME（区分应用内拖拽与 OS 文件拖入） */
export const DND_MIME = "application/x-hetushell-paths";
/** 从终端/远程面板 Ctrl+拖拽远端文件 → 下载意图 */
export const DL_MIME = "application/x-hetushell-download";

/** 面板渲染所需的最小条目形状（本地/远端条目均满足） */
export interface FsEntry {
  name: string;
  path: string;
  isDir: boolean;
  size: number;
  mtime: number | null;
}

/** 数据源适配器：屏蔽本地与远端的差异，供 Explorer 复用同一套渲染/交互 */
export interface ExplorerBackend {
  kind: "local" | "remote";
  /** 远端连接 id（remote 专有，用于拖拽载荷与上传目标）；local 为 null */
  connId: string | null;
  /** 顶部标题：local 显示“本地”，remote 显示“连接名 · host” */
  label: string;
  list(dir: string): Promise<FsEntry[]>;
  home(): Promise<string>;
}

/** 本地数据源 */
export function localBackend(): ExplorerBackend {
  return {
    kind: "local",
    connId: null,
    label: "本地",
    list: (dir) => api.localList(dir) as Promise<LocalEntry[]>,
    home: () => api.localHome(),
  };
}

type ViewMode = "list" | "tiles";
const VIEW_KEY = "hetushell-explorer-view";

/** 按扩展名归类：类别 → 强调色（用于文件图标的折角与迷你标记，KDE Breeze 风格） */
const CATEGORY: Array<[RegExp, string]> = [
  [/\.(png|jpe?g|gif|webp|bmp|svg|ico|tiff?)$/i, "#e0a35a"], // 图片
  [/\.(mp4|mkv|avi|mov|webm|flv)$/i, "#c678dd"], // 视频
  [/\.(mp3|wav|flac|ogg|m4a|aac)$/i, "#56b6c2"], // 音频
  [/\.(zip|tar|gz|bz2|xz|zst|7z|rar|deb|rpm)$/i, "#d19a66"], // 压缩包
  [/\.(pdf)$/i, "#e06c75"], // PDF
  [/\.(docx?|xlsx?|pptx?|odt|ods)$/i, "#61afef"], // 文档
  [/\.(md|txt|log|rst)$/i, "#98c379"], // 文本
  [/\.(json|ya?ml|toml|ini|conf|cfg|env|xml)$/i, "#7f889b"], // 配置
  [/\.(sh|bash|zsh|fish|ps1|bat)$/i, "#98c379"], // 脚本
  [/\.(rs|ts|tsx|js|jsx|py|go|java|c|cpp|h|hpp|cs|rb|php|swift|kt|lua|vue|css|scss|html)$/i, "#61afef"], // 代码
  [/\.(key|pem|pub|crt|cer|p12)$/i, "#e5c07b"], // 密钥
];

/** 文件夹图标（圆角、双层，KDE Breeze 观感，用强调色） */
function folderSvg(): string {
  // fill 用 CSS 类而非 fill="var(--accent)" 属性——WebKitGTK 不解析属性里的 var()
  return `<svg viewBox="0 0 20 20" width="17" height="17" fill="none">
    <path class="folder-a" d="M2 5.5A1.5 1.5 0 013.5 4h4l1.6 1.8H16.5A1.5 1.5 0 0118 7.3V14a1.5 1.5 0 01-1.5 1.5h-13A1.5 1.5 0 012 14V5.5z"
      opacity="0.35"/>
    <path class="folder-a" d="M2 8.2A1.5 1.5 0 013.5 6.7h13A1.5 1.5 0 0118 8.2V14a1.5 1.5 0 01-1.5 1.5h-13A1.5 1.5 0 012 14V8.2z"
      opacity="0.85"/>
  </svg>`;
}

/** 文件图标（纸张 + 折角 + 类别色小标记） */
function fileSvg(color: string): string {
  return `<svg viewBox="0 0 20 20" width="17" height="17" fill="none">
    <path d="M5 2.5h6.5L16 7v10a1 1 0 01-1 1H5a1 1 0 01-1-1V3.5a1 1 0 011-1z"
      fill="currentColor" opacity="0.16"/>
    <path d="M5 2.5h6.5L16 7v10a1 1 0 01-1 1H5a1 1 0 01-1-1V3.5a1 1 0 011-1z"
      stroke="currentColor" stroke-opacity="0.45" stroke-width="0.9"/>
    <path d="M11.5 2.5V6a1 1 0 001 1H16" stroke="currentColor" stroke-opacity="0.45" stroke-width="0.9"/>
    <rect x="6" y="11" width="8" height="2.4" rx="1.2" fill="${color}"/>
  </svg>`;
}

/** 上一级目录，兼容 POSIX('/') 与 Windows('\\') 分隔符 */
function parentDir(path: string): string {
  const win = path.includes("\\") && !path.includes("/");
  const sep = win ? "\\" : "/";
  const trimmed = path.replace(/[/\\]+$/, "");
  const idx = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  if (idx < 0) return path;
  // Windows 盘符根 "C:" → 保留为 "C:\\"
  if (win && /^[A-Za-z]:$/.test(trimmed.slice(0, idx))) return trimmed.slice(0, idx) + "\\";
  return trimmed.slice(0, idx) || sep;
}

function iconFor(entry: FsEntry): string {
  if (entry.isDir) return folderSvg();
  for (const [re, color] of CATEGORY) {
    if (re.test(entry.name)) return fileSvg(color);
  }
  return fileSvg("#7f889b");
}

export class Explorer {
  readonly element: HTMLElement;
  readonly backend: ExplorerBackend;
  cwd = "";
  /** [local] 请求把本地路径上传到当前终端目录（由 main 装配） */
  onUploadRequest: ((paths: string[]) => void) | null = null;
  /**
   * [local] 请求把远端文件下载到本地某目录（终端/远程面板 Ctrl+拖入时触发）。
   * srcPaneId：拖拽发起终端的 pane id——remotePath 若为相对词需针对该 pane 解析，
   * 避免同一连接被分屏复用时错用其它 pane 的工作目录。
   */
  onDownloadRequest:
    | ((connId: string, remotePath: string, targetDir: string, srcPaneId?: string) => void)
    | null = null;
  /** [remote] 请求把本地文件上传到本连接的某远端目录（本地条目拖入时触发） */
  onUploadHere: ((localPaths: string[], remoteDir: string) => void) | null = null;
  private listEl: HTMLElement;
  private pathInput: HTMLInputElement;
  private titleEl: HTMLElement;
  private viewBtn: HTMLButtonElement;
  private view: ViewMode;
  private initialized = false;

  constructor(backend: ExplorerBackend) {
    this.backend = backend;
    this.view = (localStorage.getItem(VIEW_KEY) as ViewMode) || "list";
    this.element = document.createElement("div");
    this.element.className = `explorer explorer-${backend.kind}`;
    const hint =
      backend.kind === "local"
        ? "拖条目到终端/远程面板上传；从终端 Ctrl+拖文件到这里下载；右键更多操作"
        : "拖条目到本地面板下载；把本地条目拖到这里上传；右键更多操作";
    this.element.innerHTML = `
      <div class="ex-title"></div>
      <div class="ex-head">
        <button class="btn ex-up" title="上一级">↑</button>
        <input class="ex-path" spellcheck="false">
        <button class="btn ex-view" title="切换视图"></button>
        <button class="btn ex-refresh" title="刷新">⟳</button>
      </div>
      <div class="ex-list"></div>
      <div class="ex-hint">${hint}</div>`;
    this.listEl = this.element.querySelector(".ex-list") as HTMLElement;
    this.pathInput = this.element.querySelector(".ex-path") as HTMLInputElement;
    this.titleEl = this.element.querySelector(".ex-title") as HTMLElement;
    this.titleEl.textContent = backend.label;
    this.titleEl.title = backend.label;

    this.installDrop();

    this.viewBtn = this.element.querySelector(".ex-view") as HTMLButtonElement;
    this.element.querySelector(".ex-up")!.addEventListener("click", () => {
      void this.navigate(parentDir(this.cwd));
    });
    this.element.querySelector(".ex-refresh")!.addEventListener("click", () => void this.load());
    this.viewBtn.addEventListener("click", () => {
      this.view = this.view === "list" ? "tiles" : "list";
      localStorage.setItem(VIEW_KEY, this.view);
      this.applyView();
    });
    this.applyView();
    this.pathInput.addEventListener("keydown", (e) => {
      if (e.key === "Enter") void this.navigate(this.pathInput.value.trim());
    });
  }

  /** 面板级拖放：本地面板接收下载(DL_MIME)，远程面板接收上传(DND_MIME) */
  private installDrop() {
    const mime = this.backend.kind === "local" ? DL_MIME : DND_MIME;
    this.element.addEventListener("dragover", (e) => {
      if (e.dataTransfer?.types.includes(mime)) {
        e.preventDefault();
        e.dataTransfer.dropEffect = "copy";
      }
    });
    this.element.addEventListener("drop", (e) => {
      const raw = e.dataTransfer?.getData(mime);
      if (!raw) return;
      e.preventDefault();
      const row = (e.target as HTMLElement).closest(".ex-row") as HTMLElement | null;
      const targetDir = row?.dataset.dir || this.cwd;
      try {
        if (this.backend.kind === "local") {
          // 远端文件拖入本地面板 → 下载到该目录/当前目录
          const { connId, path, paneId } = JSON.parse(raw) as {
            connId: string;
            path: string;
            paneId?: string;
          };
          this.onDownloadRequest?.(connId, path, targetDir, paneId);
        } else {
          // 本地条目拖入远程面板 → 上传到该远端目录/当前目录
          const paths = JSON.parse(raw) as string[];
          if (Array.isArray(paths) && paths.length) this.onUploadHere?.(paths, targetDir);
        }
      } catch {
        /* 载荷异常，忽略 */
      }
    });
  }

  /** 列表 ⇄ 平铺（大图标 + 文件名）切换 */
  private applyView() {
    this.listEl.classList.toggle("tiles", this.view === "tiles");
    // 按钮展示“切换后”的目标视图图标
    this.viewBtn.textContent = this.view === "list" ? "▦" : "☰";
    this.viewBtn.title = this.view === "list" ? "切换为平铺视图" : "切换为列表视图";
  }

  /** 首次显示时定位目录：优先 initialDir（远程用终端实时 cwd），否则取 backend.home() */
  async init(initialDir?: string): Promise<void> {
    if (this.initialized) return;
    this.initialized = true;
    try {
      this.cwd = (initialDir && initialDir.trim()) || (await this.backend.home());
      await this.load();
    } catch (err) {
      this.initialized = false; // 首次列目录失败：允许下次打开重试，并提示而非静默空白
      toast(`无法打开${this.backend.kind === "remote" ? "远程" : ""}目录: ${err}`, true);
    }
  }

  async navigate(dir: string): Promise<void> {
    const prev = this.cwd;
    this.cwd = dir;
    try {
      await this.load();
    } catch (err) {
      this.cwd = prev;
      this.pathInput.value = prev;
      toast(`无法打开目录: ${err}`, true);
    }
  }

  async load(): Promise<void> {
    const entries = await this.backend.list(this.cwd);
    this.pathInput.value = this.cwd;
    this.listEl.textContent = "";
    for (const entry of entries) {
      this.listEl.appendChild(this.renderRow(entry));
    }
    if (entries.length === 0) {
      this.listEl.innerHTML = `<p class="hint" style="padding:12px">（空目录）</p>`;
    }
  }

  private renderRow(entry: FsEntry): HTMLElement {
    const row = document.createElement("div");
    row.className = "ex-row";
    row.draggable = true;
    const mtime = entry.mtime ? new Date(entry.mtime * 1000).toLocaleDateString() : "";
    row.innerHTML = `
      <span class="ex-icon">${iconFor(entry)}</span>
      <span class="ex-name"></span>
      <span class="ex-meta">${entry.isDir ? "" : formatSize(entry.size)}</span>
      <span class="ex-meta">${mtime}</span>`;
    row.querySelector(".ex-name")!.textContent = entry.name;
    row.title = entry.path;
    if (entry.isDir) row.dataset.dir = entry.path; // 拖入时作为目标目录

    row.addEventListener("dblclick", () => {
      if (entry.isDir) void this.navigate(entry.path);
    });
    row.addEventListener("dragstart", (e) => {
      if (this.backend.kind === "local") {
        // 本地条目：上传源（携带本地路径）
        e.dataTransfer?.setData(DND_MIME, JSON.stringify([entry.path]));
        e.dataTransfer?.setData("text/plain", entry.path);
      } else {
        // 远端条目：下载源（携带 连接 + 远端路径）
        e.dataTransfer?.setData(
          DL_MIME,
          JSON.stringify({ connId: this.backend.connId, path: entry.path }),
        );
      }
      if (e.dataTransfer) e.dataTransfer.effectAllowed = "copy";
      row.classList.add("dragging");
    });
    row.addEventListener("dragend", () => row.classList.remove("dragging"));
    row.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      e.stopPropagation();
      const items =
        this.backend.kind === "local"
          ? [
              {
                label: `上传 “${entry.name}” 到当前终端目录`,
                action: () => this.onUploadRequest?.([entry.path]),
              },
            ]
          : [
              {
                label: `下载 “${entry.name}” 到本地`,
                action: () =>
                  this.backend.connId &&
                  this.onDownloadRequest?.(this.backend.connId, entry.path, ""),
              },
            ];
      showMenu(e.clientX, e.clientY, [
        ...items,
        ...(entry.isDir
          ? [{ label: "进入目录", action: () => void this.navigate(entry.path) }]
          : []),
        { separator: true, label: "" },
        { label: "刷新", action: () => void this.load() },
      ]);
    });
    return row;
  }
}
