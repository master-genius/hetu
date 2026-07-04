/**
 * 本地文件资源管理器（右侧浮动面板内容）。
 * 每个标签页持有独立实例——目录位置等状态随标签页保留。
 * 条目可拖拽到左侧终端上传，或右键选择上传。
 */

import { api } from "./ipc";
import { formatSize, showMenu, toast } from "./ui";
import type { LocalEntry } from "./types";

/** 拖拽数据的自定义 MIME（区分应用内拖拽与 OS 文件拖入） */
export const DND_MIME = "application/x-hetushell-paths";

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
  return `<svg viewBox="0 0 20 20" width="17" height="17" fill="none">
    <path d="M2 5.5A1.5 1.5 0 013.5 4h4l1.6 1.8H16.5A1.5 1.5 0 0118 7.3V14a1.5 1.5 0 01-1.5 1.5h-13A1.5 1.5 0 012 14V5.5z"
      fill="var(--accent)" opacity="0.35"/>
    <path d="M2 8.2A1.5 1.5 0 013.5 6.7h13A1.5 1.5 0 0118 8.2V14a1.5 1.5 0 01-1.5 1.5h-13A1.5 1.5 0 012 14V8.2z"
      fill="var(--accent)" opacity="0.85"/>
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

function iconFor(entry: LocalEntry): string {
  if (entry.isDir) return folderSvg();
  for (const [re, color] of CATEGORY) {
    if (re.test(entry.name)) return fileSvg(color);
  }
  return fileSvg("#7f889b");
}

export class Explorer {
  readonly element: HTMLElement;
  cwd = "";
  /** 请求把本地路径上传到当前终端目录（由 main 装配） */
  onUploadRequest: ((paths: string[]) => void) | null = null;
  private listEl: HTMLElement;
  private pathInput: HTMLInputElement;
  private viewBtn: HTMLButtonElement;
  private view: ViewMode;
  private initialized = false;

  constructor() {
    this.view = (localStorage.getItem(VIEW_KEY) as ViewMode) || "list";
    this.element = document.createElement("div");
    this.element.className = "explorer";
    this.element.innerHTML = `
      <div class="ex-head">
        <button class="btn ex-up" title="上一级">↑</button>
        <input class="ex-path" spellcheck="false">
        <button class="btn ex-view" title="切换视图"></button>
        <button class="btn ex-refresh" title="刷新">⟳</button>
      </div>
      <div class="ex-list"></div>
      <div class="ex-hint">拖动条目到左侧终端即可上传；右键更多操作</div>`;
    this.listEl = this.element.querySelector(".ex-list") as HTMLElement;
    this.pathInput = this.element.querySelector(".ex-path") as HTMLInputElement;
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

  /** 列表 ⇄ 平铺（大图标 + 文件名）切换 */
  private applyView() {
    this.listEl.classList.toggle("tiles", this.view === "tiles");
    // 按钮展示“切换后”的目标视图图标
    this.viewBtn.textContent = this.view === "list" ? "▦" : "☰";
    this.viewBtn.title = this.view === "list" ? "切换为平铺视图" : "切换为列表视图";
  }

  /** 首次显示时定位到本机 home */
  async init(): Promise<void> {
    if (this.initialized) return;
    this.initialized = true;
    this.cwd = await api.localHome();
    await this.load();
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
    const entries = await api.localList(this.cwd);
    this.pathInput.value = this.cwd;
    this.listEl.textContent = "";
    for (const entry of entries) {
      this.listEl.appendChild(this.renderRow(entry));
    }
    if (entries.length === 0) {
      this.listEl.innerHTML = `<p class="hint" style="padding:12px">（空目录）</p>`;
    }
  }

  private renderRow(entry: LocalEntry): HTMLElement {
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

    row.addEventListener("dblclick", () => {
      if (entry.isDir) void this.navigate(entry.path);
    });
    row.addEventListener("dragstart", (e) => {
      e.dataTransfer?.setData(DND_MIME, JSON.stringify([entry.path]));
      e.dataTransfer?.setData("text/plain", entry.path);
      if (e.dataTransfer) e.dataTransfer.effectAllowed = "copy";
      row.classList.add("dragging");
    });
    row.addEventListener("dragend", () => row.classList.remove("dragging"));
    row.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      e.stopPropagation();
      showMenu(e.clientX, e.clientY, [
        {
          label: `上传 “${entry.name}” 到当前终端目录`,
          action: () => this.onUploadRequest?.([entry.path]),
        },
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
