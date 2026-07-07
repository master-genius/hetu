/**
 * 标签页管理。每个 Tab 持有一条 SSH 连接 + 一个分屏布局。
 * 标签不显示关闭按钮（防误触）；右键菜单关闭；有运行中程序时需确认。
 */

import { Explorer } from "./explorer";
import { api } from "./ipc";
import { toast } from "./ui";
import { Layout } from "./layout";
import { Pane } from "./pane";
import type { ConnParams } from "./types";

/** 测量文本像素宽度用的共享 canvas 上下文（懒建） */
let measureCtx: CanvasRenderingContext2D | null = null;

/**
 * 把 full 截断到不超过 availPx（用给定 font 度量）。放得下则原样返回；
 * 放不下则二分求最长前缀 + 结尾 ".."，保证整体不超出可用宽度。
 */
function fitLabel(full: string, availPx: number, font: string): string {
  if (availPx <= 0) return full; // 尚未布局：交给后续 relabel/poll 再算
  measureCtx ??= document.createElement("canvas").getContext("2d");
  const ctx = measureCtx;
  if (!ctx) return full;
  ctx.font = font;
  if (ctx.measureText(full).width <= availPx) return full;
  const ellW = ctx.measureText("..").width;
  let lo = 0;
  let hi = full.length;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (ctx.measureText(full.slice(0, mid)).width + ellW <= availPx) lo = mid;
    else hi = mid - 1;
  }
  return full.slice(0, lo).trimEnd() + "..";
}

export interface Tab {
  id: string;
  title: string;
  connId: string;
  connParams: ConnParams;
  layout: Layout;
  activePaneId: string;
  el: HTMLElement; // 标签头元素
  labelEl: HTMLElement; // 标签内的文字 span
  /** 标签完整标题（未截断）：本地终端为 `目录:进程`，其余为连接名；用作 tooltip 与截断源串 */
  fullTitle: string;
  banner: HTMLElement | null; // 重连提示条
  /** 本地文件管理器：每标签页独立实例，目录状态各自保留（懒创建） */
  explorer: Explorer | null;
  explorerOpen: boolean;
  /** 远程文件管理器面板是否打开（实例按连接缓存于 main.ts，非本标签页私有） */
  remoteOpen: boolean;
  /** 会话恢复时的原始序号（用于后台并行连接完成后按保存顺序归位） */
  order?: number;
}

export class TabManager {
  tabs: Tab[] = [];
  activeTabId: string | null = null;
  /** 本地终端 home 目录（由 main.ts 注入）：cwd 等于它时标题目录段显示为 `~` */
  localHomeDir: string | null = null;
  private tabBar: HTMLElement;
  private content: HTMLElement;
  private relabelScheduled = false;
  /** 拖拽换位：当前被拖动的 tab id（dragstart 置位，dragend 清除） */
  private draggedTabId: string | null = null;

  onTabContextMenu: ((e: MouseEvent, tab: Tab) => void) | null = null;
  onNewTabRequest: (() => void) | null = null;
  /** pane 创建后由 main.ts 挂载事件回调 */
  onPaneCreated: ((pane: Pane, tab: Tab) => void) | null = null;
  onLayoutChange: (() => void) | null = null;

  constructor(tabBar: HTMLElement, content: HTMLElement) {
    this.tabBar = tabBar;
    this.content = content;
    const addBtn = document.createElement("button");
    addBtn.className = "tab-add";
    addBtn.title = "新建标签页";
    addBtn.textContent = "+";
    addBtn.addEventListener("click", () => this.onNewTabRequest?.());
    this.tabBar.appendChild(addBtn);
    // 窗口尺寸变化会改变每个标签的可用宽度 → 重新按新宽度截断（rAF 去抖）
    window.addEventListener("resize", () => this.scheduleRelabel());
  }

  /** 按当前可用宽度把 full 截断后写入标签，并把完整串作为 tooltip */
  private applyLabel(tab: Tab): void {
    const el = tab.el;
    const cs = getComputedStyle(el);
    const avail = el.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight);
    const font = `${cs.fontSize} ${cs.fontFamily}`;
    tab.labelEl.textContent = fitLabel(tab.fullTitle, avail, font);
    el.title = tab.fullTitle; // 悬停展示完整信息
  }

  /** 设置标签完整标题并立即按可用宽度截断展示 */
  setLabel(tab: Tab, full: string): void {
    tab.fullTitle = full;
    this.applyLabel(tab);
  }

  /** 重新截断所有标签（宽度可能因窗口缩放/标签数量/平分模式改变） */
  relabelAll(): void {
    for (const t of this.tabs) this.applyLabel(t);
  }

  /** rAF 去抖的 relabelAll（连续 resize 每帧至多一次） */
  private scheduleRelabel(): void {
    if (this.relabelScheduled) return;
    this.relabelScheduled = true;
    requestAnimationFrame(() => {
      this.relabelScheduled = false;
      this.relabelAll();
    });
  }

  /** 单 pane 且为本地终端的 pane（多 pane 分屏不动态改名，沿用静态名） */
  private localSinglePane(tab: Tab): Pane | null {
    const panes = tab.layout.panes();
    return panes.length === 1 && panes[0].isLocal ? panes[0] : null;
  }

  /** cwd → 标题目录段：home 显示 `~`，根显示 `/`，否则末级目录名 */
  private dirLabel(cwd: string): string {
    if (this.localHomeDir && cwd === this.localHomeDir) return "~";
    const trimmed = cwd.replace(/\/+$/, "");
    if (!trimmed) return "/";
    return trimmed.slice(trimmed.lastIndexOf("/") + 1) || "/";
  }

  /**
   * 刷新所有本地终端标签的标题为 `目录:进程`（由 main.ts 定时轮询调用）。
   * 逐个读后端 /proc 信息；失败则保留原标题，不打断其它标签。
   */
  async refreshLocalTabTitles(): Promise<void> {
    await Promise.all(
      this.tabs.map(async (tab) => {
        const pane = this.localSinglePane(tab);
        if (!pane) return;
        const info = await api.localTabInfo(pane.id).catch(() => null);
        if (!info) return;
        const title = `${this.dirLabel(info.cwd)}:${info.process}`;
        if (title !== tab.fullTitle) this.setLabel(tab, title);
      }),
    );
  }

  get active(): Tab | null {
    return this.tabs.find((t) => t.id === this.activeTabId) ?? null;
  }

  activePane(tab = this.active): Pane | null {
    if (!tab) return null;
    return tab.layout.panes().find((p) => p.id === tab.activePaneId) ?? tab.layout.panes()[0] ?? null;
  }

  findPane(paneId: string): { tab: Tab; pane: Pane } | null {
    for (const tab of this.tabs) {
      const pane = tab.layout.panes().find((p) => p.id === paneId);
      if (pane) return { tab, pane };
    }
    return null;
  }

  /** 所有连到某连接的 pane（跨标签页、跨分屏）。重连按 pane 自身 connId 定位，
   *  而非 tab.connId——后者在多 pane 标签页里被就地切换后会失真。 */
  panesByConn(connId: string): Array<{ tab: Tab; pane: Pane }> {
    const out: Array<{ tab: Tab; pane: Pane }> = [];
    for (const tab of this.tabs) {
      for (const pane of tab.layout.panes()) {
        if (pane.connId === connId) out.push({ tab, pane });
      }
    }
    return out;
  }

  /** 创建标签页：先建首个 pane，再渲染。order 用于会话恢复后按保存顺序归位。 */
  async createTab(
    connId: string,
    params: ConnParams,
    order?: number,
    initialCwd?: string | null,
  ): Promise<Tab> {
    const paneId = crypto.randomUUID();
    const pane = new Pane(paneId, connId, initialCwd);
    const layout = new Layout(pane);
    // 拖动分割线改比例也要触发会话快照（结构变化由 splitPane/closePane 触发）
    layout.onChange = () => this.onLayoutChange?.();

    const el = document.createElement("div");
    el.className = "tab";
    const label = document.createElement("span");
    label.className = "tab-label";
    label.textContent = params.name;
    el.appendChild(label);

    const tab: Tab = {
      id: crypto.randomUUID(),
      title: params.name,
      connId,
      connParams: params,
      layout,
      activePaneId: paneId,
      el,
      labelEl: label,
      fullTitle: params.name,
      banner: null,
      explorer: null,
      explorerOpen: false,
      remoteOpen: false,
      order,
    };

    el.draggable = true;
    el.addEventListener("click", () => this.activate(tab.id));
    el.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      this.onTabContextMenu?.(e, tab);
    });
    // 拖拽换位：dragstart 记 id；dragover 按光标 X 判定插左/插右并显指示线；
    // drop 完成数组与 DOM 重排；dragend/dragleave 清理指示类。
    el.addEventListener("dragstart", (e) => {
      this.draggedTabId = tab.id;
      el.classList.add("dragging");
      if (e.dataTransfer) {
        e.dataTransfer.effectAllowed = "move";
        e.dataTransfer.setData("text/plain", tab.id);
      }
    });
    el.addEventListener("dragover", (e) => {
      if (!this.draggedTabId || this.draggedTabId === tab.id) return;
      e.preventDefault();
      if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
      const r = el.getBoundingClientRect();
      const isLeft = e.clientX < r.left + r.width / 2;
      el.classList.toggle("drop-left", isLeft);
      el.classList.toggle("drop-right", !isLeft);
    });
    el.addEventListener("dragleave", () => {
      el.classList.remove("drop-left", "drop-right");
    });
    el.addEventListener("drop", (e) => {
      e.preventDefault();
      el.classList.remove("drop-left", "drop-right");
      if (!this.draggedTabId || this.draggedTabId === tab.id) return;
      const r = el.getBoundingClientRect();
      const isLeft = e.clientX < r.left + r.width / 2;
      this.reorderTab(this.draggedTabId, tab.id, isLeft);
    });
    el.addEventListener("dragend", () => {
      this.draggedTabId = null;
      for (const t of this.tabs) t.el.classList.remove("dragging", "drop-left", "drop-right");
    });

    this.tabBar.insertBefore(el, this.tabBar.querySelector(".tab-add"));
    this.tabs.push(tab);
    this.onPaneCreated?.(pane, tab);
    layout.render();
    this.activate(tab.id);
    try {
      await pane.open();
    } catch (err) {
      // 打开失败：回收该标签页与其连接，避免留下无法操作的僵尸标签页与泄漏的后端连接
      pane.dispose();
      el.remove();
      tab.banner?.remove();
      layout.container.remove();
      this.tabs = this.tabs.filter((t) => t.id !== tab.id);
      if (this.activeTabId === tab.id) {
        this.activeTabId = this.tabs[this.tabs.length - 1]?.id ?? null;
        if (this.active) this.activate(this.active.id);
        else this.onLayoutChange?.();
      }
      this.gcConnections([connId]);
      throw err;
    }
    pane.focus();
    return tab;
  }

  activate(tabId: string) {
    this.activeTabId = tabId;
    for (const t of this.tabs) {
      const active = t.id === tabId;
      t.el.classList.toggle("active", active);
      // 用 visibility 隐藏而非 display:none：非活动标签的终端**保持原尺寸、画布不塌缩**，
      // 切回时无需从 0×0 重新布局/重绘，彻底消除「大字闪烁」。所有容器常驻 DOM、绝对定位
      // 叠放，切换只是切可见性——瞬时、零重绘。
      t.layout.container.classList.toggle("tab-hidden", !active);
      if (!this.content.contains(t.layout.container)) {
        this.content.appendChild(t.layout.container);
      }
      if (t.banner) t.banner.style.display = active ? "" : "none";
    }
    // 仅一个标签页时隐藏标签栏（新建标签页由工具栏「+」或快捷键触发）
    this.tabBar.classList.toggle("single", this.tabs.length < 2);
    requestAnimationFrame(() => {
      const pane = this.activePane();
      pane?.refit();
      pane?.focus();
      // 标签数量/平分模式变化会改变每个标签可用宽度，按新宽度重截断
      this.relabelAll();
    });
    this.onLayoutChange?.();
  }

  /** 会话恢复完成后，按各标签页保存的 order 归位（后台并行连接会打乱到达顺序） */
  reorderRestored(): void {
    if (this.tabs.every((t) => t.order === undefined)) return;
    this.tabs.sort((a, b) => (a.order ?? Number.MAX_SAFE_INTEGER) - (b.order ?? Number.MAX_SAFE_INTEGER));
    const addBtn = this.tabBar.querySelector(".tab-add");
    for (const t of this.tabs) this.tabBar.insertBefore(t.el, addBtn);
    for (const t of this.tabs) t.order = undefined; // 归位后清除，避免影响后续
  }

  /** 拖拽换位：把 srcId 标签移到 targetId 左侧或右侧，DOM 与数组同步，并触发会话快照 */
  private reorderTab(srcId: string, targetId: string, beforeTarget: boolean): void {
    if (srcId === targetId) return;
    const srcIdx = this.tabs.findIndex((t) => t.id === srcId);
    if (srcIdx < 0) return;
    const [moved] = this.tabs.splice(srcIdx, 1);
    const tgtIdx = this.tabs.findIndex((t) => t.id === targetId);
    if (tgtIdx < 0) {
      // 目标已不在：放回原位，保守不破坏现有顺序
      this.tabs.splice(srcIdx, 0, moved);
      return;
    }
    const insertAt = beforeTarget ? tgtIdx : tgtIdx + 1;
    this.tabs.splice(insertAt, 0, moved);
    // DOM 顺序对齐数组（标签数量小，全量重排最稳，与 reorderRestored 一致）
    const addBtn = this.tabBar.querySelector(".tab-add");
    for (const t of this.tabs) this.tabBar.insertBefore(t.el, addBtn);
    this.onLayoutChange?.(); // 触发会话快照
  }

  /** 相对当前标签页切换（delta=+1 下一个 / -1 上一个，循环） */
  switchTabBy(delta: number): void {
    if (this.tabs.length < 2) return;
    const idx = this.tabs.findIndex((t) => t.id === this.activeTabId);
    if (idx < 0) return;
    const next = (idx + delta + this.tabs.length) % this.tabs.length;
    this.activate(this.tabs[next].id);
  }

  /** 允许的最大切分层级（参考 Konsole，防止无限嵌套） */
  static readonly MAX_SPLIT_DEPTH = 5;

  /**
   * 切分指定 tab 中的某个 pane（交互切分与会话恢复共用）。
   * 超出最大层级返回 null；ratio 供会话恢复还原分割比例。
   */
  async splitPane(tab: Tab, target: Pane, dir: "row" | "col", ratio?: number): Promise<Pane | null> {
    if (tab.layout.depthOf(target) > TabManager.MAX_SPLIT_DEPTH) return null;
    // 分屏继承被切分终端的实时 cwd（本地终端）：resolveLocalCwd 对远程/取不到返回 null，
    // 后端据此回退 home（见 local::open）
    const initialCwd = await target.resolveLocalCwd();
    // 复用被切分 pane 的连接（可能已就地切换过，未必等于 tab.connId）
    const pane = new Pane(crypto.randomUUID(), target.connId, initialCwd);
    this.onPaneCreated?.(pane, tab);
    tab.layout.split(target, dir, pane, ratio);
    tab.activePaneId = pane.id;
    try {
      await pane.open();
    } catch (err) {
      // 打开失败（如连接正处于重连等待）：从布局回收，不留无 shell 的死分屏
      tab.layout.close(pane);
      tab.activePaneId = target.id;
      this.onLayoutChange?.();
      throw err;
    }
    this.onLayoutChange?.(); // 分屏结构变化 → 面板同步 + 会话快照
    return pane;
  }

  /** 分屏：复用当前标签页的连接。target 缺省为活动 pane。 */
  async splitActive(dir: "row" | "col", target?: Pane): Promise<void> {
    const tab = this.active;
    target = target ?? this.activePane(tab ?? undefined) ?? undefined;
    if (!tab || !target) return;
    let pane: Pane | null;
    try {
      pane = await this.splitPane(tab, target, dir);
    } catch (err) {
      toast(`切分失败: ${err}`, true);
      return;
    }
    if (!pane) {
      toast(`已达到最大切分层级（${TabManager.MAX_SPLIT_DEPTH} 级）`, true);
      return;
    }
    pane.focus();
  }

  /** 就地把某 pane 切换到另一条连接（含本地终端），并回收旧连接、更新标签标题 */
  async switchPaneConnection(
    tab: Tab,
    pane: Pane,
    newConnId: string,
    name: string,
  ): Promise<void> {
    const oldConn = pane.connId;
    if (oldConn === newConnId) {
      pane.focus();
      return;
    }
    await pane.switchConnection(newConnId);
    // 单 pane 标签：标题跟随新连接；多 pane 分屏不改名，避免误导
    if (tab.layout.panes().length === 1) {
      tab.title = name;
      this.setLabel(tab, name); // 切到本地终端后由轮询接管为 `目录:进程`
      tab.connId = newConnId;
    }
    this.gcConnections([oldConn]);
    this.onLayoutChange?.();
  }

  /** 某连接是否仍被任一 pane 使用 */
  connInUse(connId: string): boolean {
    return this.tabs.some((t) => t.layout.panes().some((p) => p.connId === connId));
  }

  /** 某连接被回收（断开）后的通知，供上层清理连接元信息缓存 */
  onConnClosed: ((connId: string) => void) | null = null;

  /** 回收不再被任何 pane 引用的连接（本地终端无需断开） */
  gcConnections(candidates: string[]): void {
    for (const cid of new Set(candidates)) {
      if (cid !== "local" && !this.connInUse(cid)) {
        void api.sshDisconnect(cid).catch(() => {});
        this.onConnClosed?.(cid);
      }
    }
  }

  /** 关闭一个 pane；若是最后一个则整标签页关闭 */
  async closePane(tab: Tab, pane: Pane): Promise<void> {
    const oldConn = pane.connId;
    const remaining = tab.layout.close(pane);
    if (!remaining) {
      await this.closeTab(tab);
      return;
    }
    if (tab.activePaneId === pane.id) {
      tab.activePaneId = tab.layout.panes()[0]?.id ?? "";
    }
    this.gcConnections([oldConn]);
    this.activePane(tab)?.focus();
    this.onLayoutChange?.(); // 分屏结构变化 → 面板同步 + 会话快照
  }

  /** tab 内是否有疑似运行中的程序（busy 启发式） */
  hasBusyPane(tab: Tab): boolean {
    return tab.layout.panes().some((p) => p.busy);
  }

  /** 关闭标签页。忙碌确认由调用方（main.ts requestCloseTab / requestClosePane）负责。 */
  async closeTab(tab: Tab): Promise<void> {
    // 收集该标签页各 pane 使用的连接（可能因就地切换而多于一个）
    const conns = tab.layout.panes().map((p) => p.connId);
    tab.layout.panes().forEach((p) => p.dispose());
    tab.el.remove();
    tab.banner?.remove();
    tab.layout.container.remove();
    this.tabs = this.tabs.filter((t) => t.id !== tab.id);
    // 断开不再被任何 pane 引用的连接
    this.gcConnections(conns);
    this.tabBar.classList.toggle("single", this.tabs.length < 2);
    if (this.activeTabId === tab.id) {
      const next = this.tabs[this.tabs.length - 1];
      if (next) this.activate(next.id);
      else {
        this.activeTabId = null;
        this.onLayoutChange?.();
        this.onNewTabRequest?.();
      }
    } else {
      // 关闭非活动标签（标签头右键）不走 activate，也要触发会话快照，否则
      // session.json 残留已关闭的标签，下次启动被错误恢复
      this.onLayoutChange?.();
    }
  }

  setBanner(tab: Tab, text: string | null) {
    if (!text) {
      tab.banner?.remove();
      tab.banner = null;
      return;
    }
    if (!tab.banner) {
      tab.banner = document.createElement("div");
      tab.banner.className = "reconnect-banner";
      this.content.parentElement?.insertBefore(tab.banner, this.content);
    }
    tab.banner.textContent = text;
    tab.banner.style.display = tab.id === this.activeTabId ? "" : "none";
  }
}
