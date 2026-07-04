/**
 * 本地文件资源管理器（右侧浮动面板内容）。
 * 每个标签页持有独立实例——目录位置等状态随标签页保留。
 * 条目可拖拽到左侧终端上传，或右键选择上传。
 */

import { api } from "./ipc";
import { formatSize, showMenu, toast } from "./ui";
import type { LocalEntry } from "./types";

/** 拖拽数据的自定义 MIME（区分应用内拖拽与 OS 文件拖入） */
export const DND_MIME = "application/x-superssh-paths";

type ViewMode = "list" | "tiles";
const VIEW_KEY = "superssh-explorer-view";

/** 按扩展名归类的类型图标（展示在名称前） */
const ICON_MAP: Array<[RegExp, string]> = [
  [/\.(png|jpe?g|gif|webp|bmp|svg|ico|tiff?)$/i, "🖼️"],
  [/\.(mp4|mkv|avi|mov|webm|flv)$/i, "🎬"],
  [/\.(mp3|wav|flac|ogg|m4a|aac)$/i, "🎵"],
  [/\.(zip|tar|gz|bz2|xz|zst|7z|rar|deb|rpm)$/i, "📦"],
  [/\.(pdf)$/i, "📕"],
  [/\.(docx?|xlsx?|pptx?|odt|ods)$/i, "📘"],
  [/\.(md|txt|log|rst)$/i, "📝"],
  [/\.(json|ya?ml|toml|ini|conf|cfg|env|xml)$/i, "⚙️"],
  [/\.(sh|bash|zsh|fish|ps1|bat)$/i, "📜"],
  [/\.(rs|ts|tsx|js|jsx|py|go|java|c|cpp|h|hpp|cs|rb|php|swift|kt|lua|vue|css|scss|html)$/i, "💻"],
  [/\.(key|pem|pub|crt|cer|p12)$/i, "🔑"],
];

function iconFor(entry: LocalEntry): string {
  if (entry.isDir) return "📁";
  for (const [re, icon] of ICON_MAP) {
    if (re.test(entry.name)) return icon;
  }
  return "📄";
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
      const parent = this.cwd.replace(/\/+$/, "").split("/").slice(0, -1).join("/") || "/";
      void this.navigate(parent);
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
