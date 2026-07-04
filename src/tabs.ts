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

export interface Tab {
  id: string;
  title: string;
  connId: string;
  connParams: ConnParams;
  layout: Layout;
  activePaneId: string;
  el: HTMLElement; // 标签头元素
  banner: HTMLElement | null; // 重连提示条
  /** 本地文件管理器：每标签页独立实例，目录状态各自保留（懒创建） */
  explorer: Explorer | null;
  explorerOpen: boolean;
}

export class TabManager {
  tabs: Tab[] = [];
  activeTabId: string | null = null;
  private tabBar: HTMLElement;
  private content: HTMLElement;

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

  /** 创建标签页：先建首个 pane，再渲染 */
  async createTab(connId: string, params: ConnParams): Promise<Tab> {
    const paneId = crypto.randomUUID();
    const pane = new Pane(paneId, connId);
    const layout = new Layout(pane);

    const el = document.createElement("div");
    el.className = "tab";
    const label = document.createElement("span");
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
      banner: null,
      explorer: null,
      explorerOpen: false,
    };

    el.addEventListener("click", () => this.activate(tab.id));
    el.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      this.onTabContextMenu?.(e, tab);
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
      t.layout.container.style.display = active ? "" : "none";
      if (active && !this.content.contains(t.layout.container)) {
        this.content.appendChild(t.layout.container);
      }
      if (t.banner) t.banner.style.display = active ? "" : "none";
    }
    // 同步 refit（强制布局），让终端在本帧就以正确尺寸绘制，避免切回标签时先以旧尺寸
    // 绘一帧造成的「大字闪烁」；随后再 rAF 聚焦。
    const pane = this.activePane();
    pane?.refit();
    requestAnimationFrame(() => {
      pane?.refit();
      pane?.focus();
    });
    this.onLayoutChange?.();
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

  /** 分屏：复用当前标签页的连接。target 缺省为活动 pane。 */
  async splitActive(dir: "row" | "col", target?: Pane): Promise<void> {
    const tab = this.active;
    target = target ?? this.activePane(tab ?? undefined) ?? undefined;
    if (!tab || !target) return;
    if (tab.layout.depthOf(target) > TabManager.MAX_SPLIT_DEPTH) {
      toast(`已达到最大切分层级（${TabManager.MAX_SPLIT_DEPTH} 级）`, true);
      return;
    }
    // 复用被切分 pane 的连接（可能已就地切换过，未必等于 tab.connId）
    const pane = new Pane(crypto.randomUUID(), target.connId);
    this.onPaneCreated?.(pane, tab);
    tab.layout.split(target, dir, pane);
    tab.activePaneId = pane.id;
    await pane.open();
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
      const label = tab.el.querySelector("span");
      if (label) label.textContent = name;
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
    if (this.activeTabId === tab.id) {
      const next = this.tabs[this.tabs.length - 1];
      if (next) this.activate(next.id);
      else {
        this.activeTabId = null;
        this.onLayoutChange?.();
        this.onNewTabRequest?.();
      }
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
