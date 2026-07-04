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

  tabsByConn(connId: string): Tab[] {
    return this.tabs.filter((t) => t.connId === connId);
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
    await pane.open();
    pane.focus();
    return tab;
  }

  activate(tabId: string) {
    this.activeTabId = tabId;
    for (const t of this.tabs) {
      t.el.classList.toggle("active", t.id === tabId);
      t.layout.container.style.display = t.id === tabId ? "" : "none";
      if (t.banner) t.banner.style.display = t.id === tabId ? "" : "none";
    }
    const tab = this.active;
    if (tab && !this.content.contains(tab.layout.container)) {
      this.content.appendChild(tab.layout.container);
    }
    requestAnimationFrame(() => {
      const pane = this.activePane();
      pane?.refit();
      pane?.focus();
    });
    this.onLayoutChange?.();
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
    const pane = new Pane(crypto.randomUUID(), tab.connId);
    this.onPaneCreated?.(pane, tab);
    tab.layout.split(target, dir, pane);
    tab.activePaneId = pane.id;
    await pane.open();
    pane.focus();
  }

  /** 关闭一个 pane；若是最后一个则整标签页关闭 */
  async closePane(tab: Tab, pane: Pane): Promise<void> {
    const remaining = tab.layout.close(pane);
    if (!remaining) {
      await this.closeTab(tab, true);
      return;
    }
    if (tab.activePaneId === pane.id) {
      tab.activePaneId = tab.layout.panes()[0]?.id ?? "";
    }
    this.activePane(tab)?.focus();
  }

  /** tab 内是否有疑似运行中的程序（busy 启发式） */
  hasBusyPane(tab: Tab): boolean {
    return tab.layout.panes().some((p) => p.busy);
  }

  async closeTab(tab: Tab, force = false): Promise<void> {
    if (!force && this.hasBusyPane(tab)) {
      throw new Error("busy"); // 由调用方弹确认框
    }
    tab.layout.panes().forEach((p) => p.dispose());
    tab.el.remove();
    tab.banner?.remove();
    tab.layout.container.remove();
    this.tabs = this.tabs.filter((t) => t.id !== tab.id);
    // 若该连接不再被任何标签页使用，断开它（本地终端无连接可断）
    if (tab.connId !== "local" && !this.tabs.some((t) => t.connId === tab.connId)) {
      void api.sshDisconnect(tab.connId).catch(() => {});
    }
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
