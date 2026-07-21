/**
 * AgentModal — HetuShell 内置 AI Agent 的前端 UI。
 *
 * 设计参考 AI Studio 的融合式输入容器 + 侧边抽屉模式。
 *
 * Modal 生命周期：
 * - hai 命令 → OSC 1733 → show()：若已有 session 则恢复 hidden Modal，否则新建。
 * - ESC → 中止当前对话；Alt+H → 隐藏面板（session 保留）。
 * - Tab 关闭 → destroy()，invoke agent_destroy。
 */

import { Channel } from "@tauri-apps/api/core";
import { api } from "../ipc";
import { toast } from "../ui";
import type { AgentEvent, HaiSpec, HistoryEntry, PaneInfo, RoleMeta, RoleFull, ToolResult, UserChoice } from "./protocol";
import { ROLE_CATEGORIES } from "./protocol";
import { StreamingMarkdown } from "./renderer";

// ---------- marked + highlight.js 懒初始化 ----------

let markedReady = false;

async function ensureMarked(): Promise<void> {
  if (markedReady) return;
  const { marked } = await import("marked");
  (window as any).__marked_parse = (src: string) =>
    marked.parse(src, { async: false, breaks: true }) as string;
  markedReady = true;
}

let hljsReady = false;

async function ensureHljs(): Promise<void> {
  if (hljsReady) return;
  const hljs = (await import("highlight.js")).default;
  const langs: Record<string, any> = {
    rust: await import("highlight.js/lib/languages/rust"),
    python: await import("highlight.js/lib/languages/python"),
    javascript: await import("highlight.js/lib/languages/javascript"),
    typescript: await import("highlight.js/lib/languages/typescript"),
    go: await import("highlight.js/lib/languages/go"),
    java: await import("highlight.js/lib/languages/java"),
    c: await import("highlight.js/lib/languages/c"),
    cpp: await import("highlight.js/lib/languages/cpp"),
    shell: await import("highlight.js/lib/languages/bash"),
    bash: await import("highlight.js/lib/languages/bash"),
    yaml: await import("highlight.js/lib/languages/yaml"),
    json: await import("highlight.js/lib/languages/json"),
    toml: await import("highlight.js/lib/languages/ini"),
    sql: await import("highlight.js/lib/languages/sql"),
    xml: await import("highlight.js/lib/languages/xml"),
    html: await import("highlight.js/lib/languages/xml"),
    ini: await import("highlight.js/lib/languages/ini"),
    diff: await import("highlight.js/lib/languages/diff"),
    markdown: await import("highlight.js/lib/languages/markdown"),
  };
  for (const [name, mod] of Object.entries(langs)) {
    hljs.registerLanguage(name, mod.default);
  }
  (window as any).__hljs = hljs;
  hljsReady = true;
}

// ---------- SVG 图标 ----------

const ICONS = {
  send: `<svg width="18" height="18" viewBox="0 0 24 24" fill="none"><path d="M12 19V5M12 5l-6 6M12 5l6 6" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>`,
  abort: `<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="6" width="12" height="12" rx="2"/></svg>`,
  close: `<svg width="16" height="16" viewBox="0 0 24 24" fill="none"><path d="M6 6l12 12M18 6L6 18" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>`,
  settings: `<svg width="16" height="16" viewBox="0 0 24 24" fill="none"><path d="M12 15a3 3 0 100-6 3 3 0 000 6z" stroke="currentColor" stroke-width="2"/><path d="M19.4 15a1.65 1.65 0 00.33 1.82l.06.06a2 2 0 11-2.83 2.83l-.06-.06a1.65 1.65 0 00-1.82-.33 1.65 1.65 0 00-1 1.51V21a2 2 0 11-4 0v-.09a1.65 1.65 0 00-1-1.51 1.65 1.65 0 00-1.82.33l-.06.06a2 2 0 11-2.83-2.83l.06-.06A1.65 1.65 0 005 15a1.65 1.65 0 00-1.51-1H3a2 2 0 110-4h.09A1.65 1.65 0 005 9.4a1.65 1.65 0 00-.33-1.82l-.06-.06a2 2 0 112.83-2.83l.06.06a1.65 1.65 0 001.82.33H9a1.65 1.65 0 001-1.51V3a2 2 0 114 0v.09a1.65 1.65 0 001 1.51 1.65 1.65 0 001.82-.33l.06-.06a2 2 0 112.83 2.83l-.06.06a1.65 1.65 0 00-.33 1.82V9a1.65 1.65 0 001.51 1H21a2 2 0 110 4h-.09a1.65 1.65 0 00-1.51 1z" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/></svg>`,
  history: `<svg width="16" height="16" viewBox="0 0 24 24" fill="none"><path d="M3 12a9 9 0 109-9 9.75 9.75 0 00-6.74 2.74L3 8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/><path d="M3 4v4h4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/><path d="M12 7v5l3 2" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>`,
  glass: `<svg width="16" height="16" viewBox="0 0 24 24" fill="none"><rect x="3" y="3" width="18" height="18" rx="2" stroke="currentColor" stroke-width="1.5"/><path d="M3 9h18M9 3v18" stroke="currentColor" stroke-width="1.5"/></svg>`,
  theme: `<svg width="16" height="16" viewBox="0 0 24 24" fill="none"><circle cx="12" cy="12" r="9" stroke="currentColor" stroke-width="1.5"/><path d="M12 3a9 9 0 000 18z" fill="currentColor"/></svg>`,
  clear: `<svg width="16" height="16" viewBox="0 0 24 24" fill="none"><path d="M3 6h18M8 6V4a2 2 0 012-2h4a2 2 0 012 2v2m3 0v14a2 2 0 01-2 2H7a2 2 0 01-2-2V6h14z" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>`,
  plus: `<svg width="14" height="14" viewBox="0 0 24 24" fill="none"><path d="M12 5v14M5 12h14" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>`,
  trash: `<svg width="14" height="14" viewBox="0 0 24 24" fill="none"><path d="M3 6h18M8 6V4a2 2 0 012-2h4a2 2 0 012 2v2m3 0v14a2 2 0 01-2 2H7a2 2 0 01-2-2V6h14z" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>`,
  chevron: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none"><path d="M6 9l6 6 6-6" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>`,
  check: `<svg width="14" height="14" viewBox="0 0 24 24" fill="none"><path d="M20 6L9 17l-5-5" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>`,
  role: `<svg width="16" height="16" viewBox="0 0 24 24" fill="none"><circle cx="12" cy="8" r="4" stroke="currentColor" stroke-width="1.5"/><path d="M4 21c0-4 4-6 8-6s8 2 8 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>`,
  star: `<svg width="14" height="14" viewBox="0 0 24 24" fill="none"><path d="M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2z" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/></svg>`,
};

// ---------- 类型 ----------

interface GenOptions {
  max_tokens?: number;
  temperature?: number;
  top_p?: number;
  frequency_penalty?: number;
  presence_penalty?: number;
  stop?: string[];
  seed?: number;
  request_timeout?: number;
  stream_chunk_timeout?: number;
}

interface EndpointConfig {
  id?: string;
  url?: string;
  key: string;
  weight?: number;
  options?: GenOptions;
}

interface ModelGroup {
  show_name?: string;
  endpoints: EndpointConfig[];
}

interface ProviderConfig {
  default_model: string;
  models: Record<string, ModelGroup>;
}

interface AiConfig {
  providers: Record<string, ProviderConfig>;
  default_provider: string;
  execution?: {
    default_mode?: string;
    dangerous_commands?: string[];
    always_ask_for?: string[];
    command_timeout?: number;
    ask_approval_timeout?: number;
    plan_confirm_timeout?: number;
    ask_user_timeout?: number;
    read_terminal_timeout?: number;
  };
  roles?: Record<string, { model?: string }>;
  default_role?: string;
}

// ---------- AgentModal ----------

interface ChatTurn {
  role: "user" | "assistant";
  el: HTMLElement;
  renderer?: StreamingMarkdown;
}

export class AgentModal {
  private overlay: HTMLElement;
  private messagesEl: HTMLElement;
  private inputEl: HTMLTextAreaElement;
  private sendBtn: HTMLElement;
  private abortBtn: HTMLElement;
  private statusEl: HTMLElement;
  private closeBtn: HTMLElement;
  private chatView: HTMLElement;
  private settingsView: HTMLElement;
  private rolesView: HTMLElement;
  private glassBtn: HTMLElement;
  private themeBtn: HTMLElement;
  private clearBtn: HTMLElement;
  private settingsBtn: HTMLElement;
  private rolesBtn: HTMLElement;
  private historyBtn: HTMLElement;
  private historyDrawer: HTMLElement;

  private tabId: string;
  private role: string;
  private mode: string;
  private glassMode = false;
  private themeMode: "auto" | "light" | "dark" = "auto";
  private currentCwd = "";

  /** 设置面板缓存的配置 */
  private config: AiConfig | null = null;
  private configLoaded = false;

  /** 角色管理状态 */
  private rolesLoaded = false;
  private editingRoleId: string | null = null;
  private defaultRoleId: string = "";

  /** 外部设置的终端读取回调（main.ts 注入，用于 read_terminal 工具） */
  onReadTerminal: ((paneId: string, lines: number) => Promise<string>) | null = null;

  private channel: Channel<AgentEvent> | null = null;
  private spawned = false;
  private processing = false;
  private currentRenderer: StreamingMarkdown | null = null;
  private turns: ChatTurn[] = [];

  private onKey: (e: KeyboardEvent) => void;

  constructor(tabId: string) {
    this.tabId = tabId;
    this.role = "general";
    this.mode = "auto";

    this.overlay = document.createElement("div");
    this.overlay.className = "modal-overlay hai-overlay";
    this.overlay.innerHTML = `
      <div class="modal hai-modal">
        <div class="hai-header">
          <div class="hai-header-left">
            <button class="hai-icon-btn hai-btn-history" title="历史对话">${ICONS.history}</button>
            <button class="hai-icon-btn hai-btn-roles" title="角色管理">${ICONS.role}</button>
            <span class="hai-mode-badge">Auto</span>
          </div>
          <div class="hai-header-right">
            <button class="hai-icon-btn hai-btn-settings" title="设置">${ICONS.settings}</button>
            <button class="hai-icon-btn hai-btn-glass" title="玻璃模式">${ICONS.glass}</button>
            <button class="hai-icon-btn hai-btn-theme" title="主题切换">${ICONS.theme}</button>
            <button class="hai-icon-btn hai-btn-clear" title="清除对话历史">${ICONS.clear}</button>
            <button class="hai-icon-btn hai-btn-close" title="隐藏 (Alt+H)">${ICONS.close}</button>
          </div>
        </div>
        <div class="hai-body">
          <div class="hai-view hai-view-chat active">
            <div class="hai-messages">
              <div class="hai-empty-state">
                <div class="hai-empty-title">AI 助手已就绪</div>
                <div class="hai-empty-desc">输入问题，或试试：</div>
                <div class="hai-empty-suggestions">
                  <button class="hai-suggestion-chip">分析当前目录结构</button>
                  <button class="hai-suggestion-chip">查看 git 状态</button>
                  <button class="hai-suggestion-chip">读取文件并解释</button>
                </div>
              </div>
            </div>
            <button class="hai-scroll-btn hidden" title="滚动到底部">${ICONS.chevron}</button>
            <div class="hai-model-bar">
              <select class="hai-model-select" title="切换模型"></select>
            </div>
            <div class="hai-input-container">
              <textarea class="hai-input" rows="1" placeholder="输入消息…  (Enter 发送 · Shift+Enter 换行)"></textarea>
              <button class="hai-send-btn" title="发送 (Enter)">${ICONS.send}</button>
              <button class="hai-abort-btn hidden" title="中止 (ESC)">${ICONS.abort}</button>
            </div>
            <div class="hai-status-bar">
              <span class="hai-status">就绪</span>
            </div>
          </div>
          <div class="hai-view hai-view-settings">
            <div class="hai-settings-body">
              <div class="hai-settings-top">
                <div class="hai-form-row">
                  <label>默认 Provider</label>
                  <select class="hai-cfg-provider">
                    <option value="openai">openai</option>
                    <option value="anthropic">anthropic</option>
                  </select>
                </div>
              </div>
              <div class="hai-model-list"></div>
              <button class="hai-btn-add-model">${ICONS.plus} 新增模型</button>
              <div class="hai-settings-section">
                <h4>执行策略</h4>
                <div class="hai-form-row">
                  <label>命令超时(秒)</label>
                  <input type="number" class="hai-cfg-command-timeout" placeholder="留空=120" min="5" />
                </div>
              </div>
            </div>
            <div class="hai-settings-actions">
              <button class="btn hai-btn-save-settings">保存</button>
              <span class="hai-settings-hint"></span>
            </div>
          </div>
          <div class="hai-view hai-view-roles">
            <div class="hai-roles-body">
              <div class="hai-roles-list">
                <button class="hai-btn-new-role">${ICONS.plus} 新建角色</button>
                <div class="hai-roles-groups"></div>
              </div>
              <div class="hai-role-editor">
                <div class="hai-form-row">
                  <label>名称</label>
                  <input type="text" class="hai-role-name" placeholder="角色名称" />
                </div>
                <div class="hai-form-row">
                  <label>分类</label>
                  <select class="hai-role-category"></select>
                </div>
                <div class="hai-form-row">
                  <label>描述</label>
                  <input type="text" class="hai-role-desc" placeholder="简短描述" />
                </div>
                <div class="hai-form-row hai-form-row-content">
                  <label>提示词</label>
                  <textarea class="hai-role-content" rows="12" placeholder="系统提示词内容...&#10;可用占位符: {cwd} {pane_table}"></textarea>
                </div>
                <div class="hai-role-actions">
                  <button class="btn hai-btn-role-save">保存</button>
                  <button class="btn hai-btn-role-default">设为默认</button>
                  <button class="btn hai-btn-role-delete">删除</button>
                </div>
                <span class="hai-role-hint"></span>
              </div>
            </div>
          </div>
        </div>
        <div class="hai-history-drawer hidden">
          <div class="hai-history-drawer-header">
            <span>历史对话</span>
            <button class="hai-icon-btn hai-history-drawer-close" title="关闭">${ICONS.close}</button>
          </div>
          <div class="hai-history-list"></div>
        </div>
      </div>`;

    this.chatView = this.overlay.querySelector(".hai-view-chat")!;
    this.settingsView = this.overlay.querySelector(".hai-view-settings")!;
    this.historyDrawer = this.overlay.querySelector(".hai-history-drawer")!;
    this.messagesEl = this.overlay.querySelector(".hai-messages")!;
    this.inputEl = this.overlay.querySelector(".hai-input")!;
    this.sendBtn = this.overlay.querySelector(".hai-send-btn")!;
    this.abortBtn = this.overlay.querySelector(".hai-abort-btn")!;
    this.statusEl = this.overlay.querySelector(".hai-status")!;
    this.closeBtn = this.overlay.querySelector(".hai-btn-close")!;
    this.glassBtn = this.overlay.querySelector(".hai-btn-glass")!;
    this.themeBtn = this.overlay.querySelector(".hai-btn-theme")!;
    this.clearBtn = this.overlay.querySelector(".hai-btn-clear")!;
    this.settingsBtn = this.overlay.querySelector(".hai-btn-settings")!;
    this.rolesBtn = this.overlay.querySelector(".hai-btn-roles")!;
    this.historyBtn = this.overlay.querySelector(".hai-btn-history")!;
    this.rolesView = this.overlay.querySelector(".hai-view-roles")!;

    // 从 localStorage 恢复偏好
    this.glassMode = localStorage.getItem("hai-glass") === "true";
    const savedTheme = localStorage.getItem("hai-theme") as "auto" | "light" | "dark" | null;
    if (savedTheme) this.themeMode = savedTheme;
    this.applyGlass();
    this.applyTheme();

    // ESC：中止对话（capture phase，先于 xterm 拦截）
    // Alt+H：隐藏面板（仅在面板显示时监听）
    this.onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        this.abort();
      } else if (e.altKey && (e.key === "h" || e.key === "H")) {
        e.preventDefault();
        e.stopPropagation();
        this.hide();
      }
    };

    this.closeBtn.addEventListener("click", () => this.hide());
    this.glassBtn.addEventListener("click", () => this.toggleGlass());
    this.themeBtn.addEventListener("click", () => this.cycleTheme());
    this.clearBtn.addEventListener("click", () => {
      this.showConfirm("确定清除对话历史？", () => {
        api.agentClearHistory(this.tabId).catch(() => {});
      });
    });
    this.settingsBtn.addEventListener("click", () => this.switchView("settings"));
    this.rolesBtn.addEventListener("click", () => this.switchView("roles"));
    this.historyBtn.addEventListener("click", () => this.toggleHistoryDrawer());
    this.overlay.querySelector(".hai-history-drawer-close")!.addEventListener("click", () => this.toggleHistoryDrawer(false));

    // 角色管理
    this.overlay.querySelector(".hai-btn-new-role")!.addEventListener("click", () => this.newRole());
    this.overlay.querySelector(".hai-btn-role-save")!.addEventListener("click", () => void this.saveRole());
    this.overlay.querySelector(".hai-btn-role-default")!.addEventListener("click", () => void this.setRoleDefault());
    this.overlay.querySelector(".hai-btn-role-delete")!.addEventListener("click", () => void this.deleteRole());

    this.sendBtn.addEventListener("click", () => this.send());
    this.abortBtn.addEventListener("click", () => this.abort());

    // 保存设置
    this.overlay.querySelector(".hai-btn-save-settings")!.addEventListener("click", () => {
      void this.saveSettings();
    });

    // 新增模型
    this.overlay.querySelector(".hai-btn-add-model")!.addEventListener("click", () => {
      void this.addModel();
    });

    // 输入框：Enter 发送，Shift+Enter 换行
    this.inputEl.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        this.send();
      }
    });

    // 自动调整 textarea 高度
    this.inputEl.addEventListener("input", () => {
      this.inputEl.style.height = "auto";
      this.inputEl.style.height = Math.min(this.inputEl.scrollHeight, 200) + "px";
    });

    // 模型选择器：切换后直接保存配置
    const modelSelect = this.overlay.querySelector(".hai-model-select") as HTMLSelectElement;
    modelSelect.addEventListener("change", () => {
      const value = modelSelect.value;
      const slashIdx = value.indexOf("/");
      if (slashIdx < 0) return;
      const provider = value.slice(0, slashIdx);
      const model = value.slice(slashIdx + 1);
      void this.switchModel(provider, model);
    });

    // scroll-to-bottom 按钮
    const scrollBtn = this.overlay.querySelector(".hai-scroll-btn") as HTMLElement;
    scrollBtn.addEventListener("click", () => this.scrollToBottom());
    this.messagesEl.addEventListener("scroll", () => {
      const atBottom = this.messagesEl.scrollHeight - this.messagesEl.scrollTop - this.messagesEl.clientHeight < 80;
      scrollBtn.classList.toggle("hidden", atBottom);
    });

    // 空状态建议芯片
    this.overlay.querySelectorAll<HTMLElement>(".hai-suggestion-chip").forEach((chip) => {
      chip.addEventListener("click", () => {
        this.inputEl.value = chip.textContent || "";
        this.inputEl.focus();
        this.inputEl.style.height = "auto";
        this.inputEl.style.height = Math.min(this.inputEl.scrollHeight, 200) + "px";
      });
    });
  }

  // ---------- 视图切换 ----------

  private switchView(view: "chat" | "settings" | "roles"): void {
    this.chatView.classList.toggle("active", view === "chat");
    this.settingsView.classList.toggle("active", view === "settings");
    this.rolesView.classList.toggle("active", view === "roles");
    this.settingsBtn.classList.toggle("active", view === "settings");
    this.rolesBtn.classList.toggle("active", view === "roles");
    if (view === "settings" && !this.configLoaded) {
      void this.loadSettings();
    }
    if (view === "roles" && !this.rolesLoaded) {
      void this.loadRoles();
    }
    if (view === "chat") {
      this.inputEl.focus();
    }
  }

  // ---------- 角色管理 ----------

  private async loadRoles(): Promise<void> {
    // 初始化分类下拉
    const catSelect = this.overlay.querySelector(".hai-role-category") as HTMLSelectElement;
    if (catSelect.children.length === 0) {
      for (const cat of ROLE_CATEGORIES) {
        const opt = document.createElement("option");
        opt.value = cat;
        opt.textContent = cat;
        catSelect.appendChild(opt);
      }
    }

    // 读取默认角色
    try {
      const config = await api.agentLoadConfig() as AiConfig;
      this.defaultRoleId = config.default_role || "";
    } catch { /* ignore */ }

    // 加载角色列表
    try {
      const roles = await api.agentListRoles() as RoleMeta[];
      this.renderRolesList(roles);
      this.rolesLoaded = true;
    } catch (err: any) {
      this.setRoleHint(String(err?.message ?? err), true);
    }
  }

  private renderRolesList(roles: RoleMeta[]): void {
    const container = this.overlay.querySelector(".hai-roles-groups")!;
    container.innerHTML = "";

    // 按分类分组
    const groups = new Map<string, RoleMeta[]>();
    for (const role of roles) {
      const cat = role.category || "默认";
      if (!groups.has(cat)) groups.set(cat, []);
      groups.get(cat)!.push(role);
    }

    // 按 ROLE_CATEGORIES 顺序渲染，未列出的分类放最后
    const orderedCats = [...ROLE_CATEGORIES];
    for (const [cat] of groups) {
      if (!orderedCats.includes(cat)) orderedCats.push(cat);
    }

    for (const cat of orderedCats) {
      const items = groups.get(cat);
      if (!items || items.length === 0) continue;

      const groupEl = document.createElement("div");
      groupEl.className = "hai-role-group";
      groupEl.innerHTML = `<div class="hai-role-group-title">${cat}</div>`;
      const listEl = document.createElement("div");
      listEl.className = "hai-role-group-list";

      for (const role of items) {
        const itemEl = document.createElement("div");
        itemEl.className = "hai-role-item";
        if (role.id === this.editingRoleId) itemEl.classList.add("selected");
        itemEl.dataset.roleId = role.id;

        const isDefault = role.id === this.defaultRoleId;
        itemEl.innerHTML = `
          <span class="hai-role-item-name">${this.escapeHtml(role.name || role.id)}</span>
          ${isDefault ? `<span class="hai-role-default-mark" title="默认角色">${ICONS.star}</span>` : ""}
          ${role.description ? `<span class="hai-role-item-desc">${this.escapeHtml(role.description)}</span>` : ""}
        `;
        itemEl.addEventListener("click", () => void this.selectRole(role.id));
        listEl.appendChild(itemEl);
      }
      groupEl.appendChild(listEl);
      container.appendChild(groupEl);
    }
  }

  private async selectRole(id: string): Promise<void> {
    try {
      const role = await api.agentGetRole(id) as RoleFull;
      this.editingRoleId = id;
      (this.overlay.querySelector(".hai-role-name") as HTMLInputElement).value = role.name;
      (this.overlay.querySelector(".hai-role-category") as HTMLSelectElement).value = role.category || "默认";
      (this.overlay.querySelector(".hai-role-desc") as HTMLInputElement).value = role.description;
      (this.overlay.querySelector(".hai-role-content") as HTMLTextAreaElement).value = role.content;

      // 高亮选中项
      this.overlay.querySelectorAll(".hai-role-item").forEach((el) => {
        el.classList.toggle("selected", (el as HTMLElement).dataset.roleId === id);
      });
      this.setRoleHint("");
    } catch (err: any) {
      this.setRoleHint(String(err?.message ?? err), true);
    }
  }

  private newRole(): void {
    this.editingRoleId = null;
    (this.overlay.querySelector(".hai-role-name") as HTMLInputElement).value = "";
    (this.overlay.querySelector(".hai-role-category") as HTMLSelectElement).value = "默认";
    (this.overlay.querySelector(".hai-role-desc") as HTMLInputElement).value = "";
    (this.overlay.querySelector(".hai-role-content") as HTMLTextAreaElement).value = "";
    this.overlay.querySelectorAll(".hai-role-item").forEach((el) => el.classList.remove("selected"));
    this.setRoleHint("新建角色：填写名称和提示词后保存");
    (this.overlay.querySelector(".hai-role-name") as HTMLInputElement).focus();
  }

  private async saveRole(): Promise<void> {
    const name = (this.overlay.querySelector(".hai-role-name") as HTMLInputElement).value.trim();
    const category = (this.overlay.querySelector(".hai-role-category") as HTMLSelectElement).value;
    const description = (this.overlay.querySelector(".hai-role-desc") as HTMLInputElement).value.trim();
    const content = (this.overlay.querySelector(".hai-role-content") as HTMLTextAreaElement).value;

    if (!name) {
      this.setRoleHint("请填写角色名称", true);
      return;
    }
    if (!content.trim()) {
      this.setRoleHint("请填写提示词内容", true);
      return;
    }

    // 从名称生成 ID（编辑已有角色时保持原 ID）
    const id = this.editingRoleId || this.slugify(name);

    try {
      await api.agentSaveRole(id, name, category, description, content);
      this.editingRoleId = id;
      this.setRoleHint("已保存");
      // 刷新列表
      const roles = await api.agentListRoles() as RoleMeta[];
      this.renderRolesList(roles);
    } catch (err: any) {
      this.setRoleHint(String(err?.message ?? err), true);
    }
  }

  private async deleteRole(): Promise<void> {
    if (!this.editingRoleId) {
      this.setRoleHint("未选择角色", true);
      return;
    }
    const roleName = (this.overlay.querySelector(".hai-role-name") as HTMLInputElement).value || this.editingRoleId;
    this.showConfirm(`确定删除角色「${roleName}」？`, async () => {
      try {
        await api.agentDeleteRole(this.editingRoleId!);
        // 若删除的是默认角色，清除默认设置
        if (this.editingRoleId === this.defaultRoleId) {
          await api.agentSetDefaultRole(null);
          this.defaultRoleId = "";
        }
        this.newRole();
        const roles = await api.agentListRoles() as RoleMeta[];
        this.renderRolesList(roles);
        this.setRoleHint("已删除");
      } catch (err: any) {
        this.setRoleHint(String(err?.message ?? err), true);
      }
    });
  }

  private async setRoleDefault(): Promise<void> {
    if (!this.editingRoleId) {
      this.setRoleHint("未选择角色", true);
      return;
    }
    try {
      await api.agentSetDefaultRole(this.editingRoleId);
      this.defaultRoleId = this.editingRoleId;
      // 刷新列表显示星标
      const roles = await api.agentListRoles() as RoleMeta[];
      this.renderRolesList(roles);
      this.setRoleHint(`已将「${(this.overlay.querySelector(".hai-role-name") as HTMLInputElement).value}」设为默认角色`);
    } catch (err: any) {
      this.setRoleHint(String(err?.message ?? err), true);
    }
  }

  private setRoleHint(msg: string, isError = false): void {
    const hint = this.overlay.querySelector(".hai-role-hint") as HTMLElement;
    hint.textContent = msg;
    hint.classList.toggle("error", isError);
  }

  private slugify(name: string): string {
    return name.trim()
      .replace(/\s+/g, "-")
      .replace(/[\/\\:*?"<>|]/g, "")
      .toLowerCase();
  }

  private escapeHtml(s: string): string {
    return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
  }

  /** 角色选择器（hai 命令未指定 role 且无默认角色时弹出） */
  private showRolePicker(): Promise<string | null> {
    return new Promise((resolve) => {
      const picker = document.createElement("div");
      picker.className = "hai-role-picker-overlay";
      picker.innerHTML = `
        <div class="hai-role-picker">
          <div class="hai-role-picker-title">选择角色</div>
          <div class="hai-role-picker-body"></div>
          <div class="hai-role-picker-actions">
            <button class="btn hai-role-picker-skip">跳过（用通用助手）</button>
          </div>
        </div>`;
      document.body.appendChild(picker);

      const cleanup = () => picker.remove();

      picker.querySelector(".hai-role-picker-skip")!.addEventListener("click", () => {
        cleanup();
        resolve("general");
      });

      // 加载角色列表
      void (async () => {
        try {
          const roles = await api.agentListRoles() as RoleMeta[];
          const body = picker.querySelector(".hai-role-picker-body")!;
          const groups = new Map<string, RoleMeta[]>();
          for (const role of roles) {
            const cat = role.category || "默认";
            if (!groups.has(cat)) groups.set(cat, []);
            groups.get(cat)!.push(role);
          }
          const orderedCats = [...ROLE_CATEGORIES];
          for (const [cat] of groups) {
            if (!orderedCats.includes(cat)) orderedCats.push(cat);
          }
          for (const cat of orderedCats) {
            const items = groups.get(cat);
            if (!items || items.length === 0) continue;
            const groupEl = document.createElement("div");
            groupEl.className = "hai-role-picker-group";
            groupEl.innerHTML = `<div class="hai-role-picker-group-title">${cat}</div>`;
            for (const role of items) {
              const itemEl = document.createElement("button");
              itemEl.className = "hai-role-picker-item";
              itemEl.innerHTML = `
                <span class="hai-role-picker-name">${this.escapeHtml(role.name || role.id)}</span>
                ${role.description ? `<span class="hai-role-picker-desc">${this.escapeHtml(role.description)}</span>` : ""}`;
              itemEl.addEventListener("click", () => {
                cleanup();
                resolve(role.id);
              });
              groupEl.appendChild(itemEl);
            }
            body.appendChild(groupEl);
          }
        } catch {
          cleanup();
          resolve("general");
        }
      })();
    });
  }

  // ---------- 模型选择器 ----------

  private async loadModelSelector(): Promise<void> {
    const select = this.overlay.querySelector(".hai-model-select") as HTMLSelectElement;
    try {
      const config = await api.agentLoadConfig() as AiConfig;
      select.innerHTML = "";
      let hasSelection = false;
      for (const [providerName, providerCfg] of Object.entries(config.providers || {})) {
        for (const [modelKey, group] of Object.entries(providerCfg.models || {})) {
          const opt = document.createElement("option");
          opt.value = `${providerName}/${modelKey}`;
          opt.textContent = group.show_name || modelKey;
          if (providerName === config.default_provider && modelKey === providerCfg.default_model) {
            opt.selected = true;
            hasSelection = true;
          }
          select.appendChild(opt);
        }
      }
      if (!hasSelection && select.options.length > 0) {
        select.options[0].selected = true;
      }
    } catch {
      // 配置未加载，选择器保持空
    }
  }

  private async switchModel(provider: string, model: string): Promise<void> {
    try {
      const config = await api.agentLoadConfig() as AiConfig;
      config.default_provider = provider;
      if (config.providers[provider]) {
        config.providers[provider].default_model = model;
      }
      await api.agentSaveConfig(config);
      this.setStatus(`已切换到 ${config.providers[provider]?.models[model]?.show_name || model}`);
      setTimeout(() => {
        if (this.statusEl.textContent?.startsWith("已切换到")) this.setStatus("就绪");
      }, 2000);
    } catch (e: any) {
      toast(`切换模型失败: ${e?.message || e}`);
    }
  }

  // ---------- 设置面板 ----------

  private async loadSettings(): Promise<void> {
    const hint = this.overlay.querySelector(".hai-settings-hint") as HTMLElement;
    hint.textContent = "加载中…";
    try {
      this.config = await api.agentLoadConfig() as AiConfig;
      this.renderModelList();
      this.populateExecutionFields();
      this.configLoaded = true;
      hint.textContent = "";
    } catch (err: any) {
      hint.textContent = `加载失败: ${err?.message ?? err}`;
    }
  }

  private populateExecutionFields(): void {
    const exec = this.config?.execution ?? {};
    const cmdTimeoutEl = this.overlay.querySelector(".hai-cfg-command-timeout") as HTMLInputElement;
    cmdTimeoutEl.value = exec.command_timeout != null ? String(exec.command_timeout) : "";

    const providerEl = this.overlay.querySelector(".hai-cfg-provider") as HTMLSelectElement;
    providerEl.value = this.config?.default_provider || "openai";
  }

  /** 渲染模型卡片列表 */
  private renderModelList(): void {
    const listEl = this.overlay.querySelector(".hai-model-list") as HTMLElement;
    listEl.innerHTML = "";

    if (!this.config?.providers) return;

    for (const [providerName, providerCfg] of Object.entries(this.config.providers)) {
      for (const [modelKey, group] of Object.entries(providerCfg.models)) {
        const isDefault = providerName === this.config!.default_provider &&
                          modelKey === providerCfg.default_model;
        listEl.appendChild(
          this.createModelCard(providerName, modelKey, group, isDefault),
        );
      }
    }
  }

  /** 创建模型卡片 */
  private createModelCard(
    provider: string,
    model: string,
    group: ModelGroup,
    isDefault: boolean,
  ): HTMLElement {
    const card = document.createElement("div");
    card.className = "hai-model-card";
    card.dataset.provider = provider;
    card.dataset.model = model;

    const showName = group.show_name || "";
    const epCount = group.endpoints.length;

    card.innerHTML = `
      <div class="hai-model-card-header">
        <div class="hai-model-info">
          <span class="hai-model-key">${model}</span>
          ${showName ? `<span class="hai-model-sep">·</span><span class="hai-model-name">${showName}</span>` : ""}
        </div>
        <div class="hai-model-actions">
          ${isDefault ? '<span class="hai-default-badge">默认</span>' : `<button class="hai-icon-btn hai-model-set-default" title="设为默认">${ICONS.check}</button>`}
          <span class="hai-ep-count">${epCount} endpoint${epCount !== 1 ? "s" : ""}</span>
          <button class="hai-icon-btn hai-model-expand" title="展开/折叠">${ICONS.chevron}</button>
        </div>
      </div>
      <div class="hai-model-card-body hidden">
        <div class="hai-form-row">
          <label>显示名称</label>
          <input type="text" class="hai-cfg-showname" value="${showName}" placeholder="留空则用模型 ID" />
        </div>
        <div class="hai-endpoint-list"></div>
        <button class="hai-btn-add-endpoint">${ICONS.plus} 新增 Endpoint</button>
        <div class="hai-model-card-footer">
          ${!isDefault ? `<button class="btn hai-model-set-default-btn">设为默认模型</button>` : ""}
          <button class="btn hai-model-delete-btn">${ICONS.trash} 删除模型</button>
        </div>
      </div>`;

    // 展开/折叠
    const expandBtn = card.querySelector(".hai-model-expand")!;
    expandBtn.addEventListener("click", () => {
      card.querySelector(".hai-model-card-body")!.classList.toggle("hidden");
      expandBtn.classList.toggle("expanded");
    });

    // 设为默认
    const setDefaultBtn = card.querySelector(".hai-model-set-default, .hai-model-set-default-btn");
    if (setDefaultBtn) {
      setDefaultBtn.addEventListener("click", () => {
        this.setDefaultModel(provider, model);
      });
    }

    // 删除模型
    card.querySelector(".hai-model-delete-btn")!.addEventListener("click", () => {
      this.showConfirm(`确定删除模型 ${model}？`, () => this.deleteModel(provider, model));
    });

    // 渲染 endpoints
    const epListEl = card.querySelector(".hai-endpoint-list")!;
    for (let i = 0; i < group.endpoints.length; i++) {
      epListEl.appendChild(this.createEndpointCard(provider, model, i, group.endpoints[i]));
    }

    // 新增 endpoint
    card.querySelector(".hai-btn-add-endpoint")!.addEventListener("click", () => {
      this.addEndpoint(provider, model);
    });

    return card;
  }

  /** 创建 endpoint 编辑卡片 */
  private createEndpointCard(
    provider: string,
    model: string,
    idx: number,
    ep: EndpointConfig,
  ): HTMLElement {
    const card = document.createElement("div");
    card.className = "hai-endpoint-card";
    card.dataset.idx = String(idx);

    const opts = ep.options ?? {};

    card.innerHTML = `
      <div class="hai-endpoint-card-header">
        <span class="hai-endpoint-label">Endpoint ${idx + 1}</span>
        <button class="hai-icon-btn hai-endpoint-expand" title="展开/折叠">${ICONS.chevron}</button>
      </div>
      <div class="hai-endpoint-card-body">
        <div class="hai-form-row">
          <label>URL</label>
          <input type="text" class="hai-cfg-url" value="${ep.url ?? ""}" placeholder="https://api.deepseek.com/v1" />
        </div>
        <div class="hai-form-row">
          <label>API Model ID</label>
          <input type="text" class="hai-cfg-endpoint-id" value="${ep.id ?? ""}" placeholder="留空则用模型 ID" />
        </div>
        <div class="hai-form-row">
          <label>API Key</label>
          <input type="password" class="hai-cfg-key" value="${ep.key}" placeholder="sk-..." />
        </div>
        <div class="hai-form-row">
          <label>权重</label>
          <input type="number" class="hai-cfg-weight" value="${ep.weight ?? 1}" min="1" />
        </div>
        <details class="hai-advanced-params">
          <summary>高级参数</summary>
          <div class="hai-form-row">
            <label>Max Tokens</label>
            <input type="number" class="hai-cfg-max-tokens" value="${opts.max_tokens ?? 8192}" min="1" />
          </div>
          <div class="hai-form-row">
            <label>Temperature</label>
            <input type="number" class="hai-cfg-temperature" value="${opts.temperature ?? 0.7}" min="0" max="2" step="0.1" />
          </div>
          <div class="hai-form-row">
            <label>Top P</label>
            <input type="number" class="hai-cfg-top-p" value="${opts.top_p ?? ""}" min="0" max="1" step="0.1" placeholder="留空使用默认" />
          </div>
          <div class="hai-form-row">
            <label>Frequency Penalty</label>
            <input type="number" class="hai-cfg-freq-penalty" value="${opts.frequency_penalty ?? ""}" min="-2" max="2" step="0.1" placeholder="留空使用默认" />
          </div>
          <div class="hai-form-row">
            <label>Presence Penalty</label>
            <input type="number" class="hai-cfg-pres-penalty" value="${opts.presence_penalty ?? ""}" min="-2" max="2" step="0.1" placeholder="留空使用默认" />
          </div>
          <div class="hai-form-row">
            <label>Stop（逗号分隔）</label>
            <input type="text" class="hai-cfg-stop" value="${opts.stop?.join(", ") ?? ""}" placeholder="留空使用默认" />
          </div>
          <div class="hai-form-row">
            <label>Seed</label>
            <input type="number" class="hai-cfg-seed" value="${opts.seed ?? ""}" placeholder="留空使用默认" />
          </div>
          <div class="hai-form-row">
            <label>请求超时(秒)</label>
            <input type="number" class="hai-cfg-request-timeout" value="${opts.request_timeout ?? ""}" placeholder="留空=120" min="5" />
          </div>
          <div class="hai-form-row">
            <label>流式超时(秒)</label>
            <input type="number" class="hai-cfg-stream-timeout" value="${opts.stream_chunk_timeout ?? ""}" placeholder="留空=60" min="5" />
          </div>
        </details>
        <div class="hai-endpoint-card-footer">
          <button class="btn hai-endpoint-delete-btn">${ICONS.trash} 删除 Endpoint</button>
        </div>
      </div>`;

    // 展开/折叠
    card.querySelector(".hai-endpoint-expand")!.addEventListener("click", () => {
      card.querySelector(".hai-endpoint-card-body")!.classList.toggle("hidden");
    });

    // 删除
    card.querySelector(".hai-endpoint-delete-btn")!.addEventListener("click", () => {
      this.showConfirm(`确定删除 Endpoint ${idx + 1}？`, () => this.deleteEndpoint(provider, model, idx));
    });

    return card;
  }

  /** 新增模型 */
  private async addModel(): Promise<void> {
    const provider = (this.overlay.querySelector(".hai-cfg-provider") as HTMLSelectElement).value;
    this.showPrompt("输入模型 ID", "deepseek-v3", (modelKey) => {
      if (!this.config) this.config = { providers: {}, default_provider: provider };
      if (!this.config.providers[provider]) {
        this.config.providers[provider] = { default_model: "", models: {} };
      }
      if (this.config.providers[provider].models[modelKey]) {
        toast("模型已存在");
        return;
      }
      this.config.providers[provider].models[modelKey] = {
        endpoints: [{ key: "", options: { max_tokens: 8192, temperature: 0.7 } }],
      };
      if (!this.config.providers[provider].default_model) {
        this.config.providers[provider].default_model = modelKey;
      }
      this.renderModelList();
    });
  }

  /** 新增 endpoint */
  private addEndpoint(provider: string, model: string): void {
    if (!this.config) return;
    const group = this.config.providers?.[provider]?.models?.[model];
    if (!group) return;

    group.endpoints.push({
      key: "",
      options: { max_tokens: 8192, temperature: 0.7 },
    });

    this.renderModelList();
    // 重新展开刚操作的卡片
    const card = this.overlay.querySelector(`.hai-model-card[data-provider="${provider}"][data-model="${model}"]`);
    if (card) {
      card.querySelector(".hai-model-card-body")!.classList.remove("hidden");
    }
  }

  /** 删除模型 */
  private deleteModel(provider: string, model: string): void {
    if (!this.config) return;
    const providerCfg = this.config.providers?.[provider];
    if (!providerCfg) return;

    delete providerCfg.models[model];
    if (providerCfg.default_model === model) {
      const remaining = Object.keys(providerCfg.models);
      providerCfg.default_model = remaining[0] ?? "";
    }

    this.renderModelList();
  }

  /** 删除 endpoint */
  private deleteEndpoint(provider: string, model: string, idx: number): void {
    if (!this.config) return;
    const group = this.config.providers?.[provider]?.models?.[model];
    if (!group) return;

    group.endpoints.splice(idx, 1);

    this.renderModelList();
    // 重新展开卡片
    const card = this.overlay.querySelector(`.hai-model-card[data-provider="${provider}"][data-model="${model}"]`);
    if (card) {
      card.querySelector(".hai-model-card-body")!.classList.remove("hidden");
    }
  }

  /** 设为默认模型 */
  private setDefaultModel(provider: string, model: string): void {
    if (!this.config) return;
    this.config.default_provider = provider;
    if (!this.config.providers[provider]) return;
    this.config.providers[provider].default_model = model;

    // 更新 provider 下拉
    (this.overlay.querySelector(".hai-cfg-provider") as HTMLSelectElement).value = provider;

    this.renderModelList();
  }

  /** 从 DOM 收集配置并保存 */
  private async saveSettings(): Promise<void> {
    const hint = this.overlay.querySelector(".hai-settings-hint") as HTMLElement;
    hint.textContent = "保存中…";

    const provider = (this.overlay.querySelector(".hai-cfg-provider") as HTMLSelectElement).value;

    // 从 DOM 重建配置
    const config: AiConfig = {
      providers: {},
      default_provider: provider,
      execution: this.config?.execution ?? {},
      roles: this.config?.roles ?? {},
    };

    // 遍历所有模型卡片
    const modelCards = this.overlay.querySelectorAll<HTMLElement>(".hai-model-card");
    for (const card of modelCards) {
      const p = card.dataset.provider!;
      const m = card.dataset.model!;

      if (!config.providers[p]) {
        config.providers[p] = { default_model: "", models: {} };
      }
      // 检查 DOM 中该卡片是否有默认徽章
      const isDefault = card.querySelector(".hai-default-badge") !== null;
      if (isDefault) {
        config.default_provider = p;
        config.providers[p].default_model = m;
      }

      const showName = (card.querySelector(".hai-cfg-showname") as HTMLInputElement).value.trim();
      const group: ModelGroup = { endpoints: [] };
      if (showName) group.show_name = showName;

      // 遍历 endpoint 卡片
      const epCards = card.querySelectorAll<HTMLElement>(".hai-endpoint-card");
      for (const epCard of epCards) {
        const url = (epCard.querySelector(".hai-cfg-url") as HTMLInputElement).value.trim();
        const endpointId = (epCard.querySelector(".hai-cfg-endpoint-id") as HTMLInputElement).value.trim();
        const key = (epCard.querySelector(".hai-cfg-key") as HTMLInputElement).value.trim();
        const weight = parseInt((epCard.querySelector(".hai-cfg-weight") as HTMLInputElement).value || "1", 10);
        const maxTokens = parseInt((epCard.querySelector(".hai-cfg-max-tokens") as HTMLInputElement).value || "8192", 10);
        const temperature = parseFloat((epCard.querySelector(".hai-cfg-temperature") as HTMLInputElement).value || "0.7");
        const topPStr = (epCard.querySelector(".hai-cfg-top-p") as HTMLInputElement).value.trim();
        const freqPenaltyStr = (epCard.querySelector(".hai-cfg-freq-penalty") as HTMLInputElement).value.trim();
        const presPenaltyStr = (epCard.querySelector(".hai-cfg-pres-penalty") as HTMLInputElement).value.trim();
        const stopStr = (epCard.querySelector(".hai-cfg-stop") as HTMLInputElement).value.trim();
        const seedStr = (epCard.querySelector(".hai-cfg-seed") as HTMLInputElement).value.trim();
        const reqTimeoutStr = (epCard.querySelector(".hai-cfg-request-timeout") as HTMLInputElement).value.trim();
        const streamTimeoutStr = (epCard.querySelector(".hai-cfg-stream-timeout") as HTMLInputElement).value.trim();

        if (!key) {
          hint.textContent = `${p}/${m} 的 Endpoint 缺少 API Key`;
          return;
        }

        const options: GenOptions = {
          max_tokens: maxTokens || 8192,
          temperature: Number.isFinite(temperature) ? temperature : 0.7,
        };
        if (topPStr) options.top_p = parseFloat(topPStr);
        if (freqPenaltyStr) options.frequency_penalty = parseFloat(freqPenaltyStr);
        if (presPenaltyStr) options.presence_penalty = parseFloat(presPenaltyStr);
        if (stopStr) options.stop = stopStr.split(",").map((s) => s.trim()).filter(Boolean);
        if (seedStr) options.seed = parseInt(seedStr, 10);
        if (reqTimeoutStr) options.request_timeout = parseInt(reqTimeoutStr, 10);
        if (streamTimeoutStr) options.stream_chunk_timeout = parseInt(streamTimeoutStr, 10);

        const endpoint: EndpointConfig = { key, options };
        if (url) endpoint.url = url;
        if (endpointId) endpoint.id = endpointId;
        if (weight && weight > 0) endpoint.weight = weight;

        group.endpoints.push(endpoint);
      }

      config.providers[p].models[m] = group;
    }

    // 从 default_model 反查：如果没有明确的默认，取第一个
    for (const pc of Object.values(config.providers)) {
      if (!pc.default_model) {
        const first = Object.keys(pc.models)[0];
        if (first) pc.default_model = first;
      }
    }

    // 执行策略
    const cmdTimeoutStr = (this.overlay.querySelector(".hai-cfg-command-timeout") as HTMLInputElement).value.trim();
    if (cmdTimeoutStr) {
      if (!config.execution) config.execution = {};
      config.execution.command_timeout = parseInt(cmdTimeoutStr, 10);
    }

    this.config = config;

    try {
      await api.agentSaveConfig(config);
      hint.textContent = "已保存";
      setTimeout(() => {
        if (hint.textContent === "已保存") hint.textContent = "";
      }, 3000);
    } catch (err: any) {
      hint.textContent = `保存失败: ${err?.message ?? err}`;
    }
  }

  // ---------- 历史抽屉 ----------

  private toggleHistoryDrawer(force?: boolean): void {
    const show = force ?? this.historyDrawer.classList.contains("hidden");
    this.historyDrawer.classList.toggle("hidden", !show);
    if (show) {
      void this.loadHistoryList();
    }
  }

  private async loadHistoryList(): Promise<void> {
    const listEl = this.historyDrawer.querySelector(".hai-history-list") as HTMLElement;
    listEl.innerHTML = '<div class="hai-history-loading">加载中…</div>';
    try {
      const entries = await api.agentListHistory();
      if (entries.length === 0) {
        listEl.innerHTML = '<div class="hai-history-empty">暂无历史对话</div>';
        return;
      }
      listEl.innerHTML = "";
      for (const entry of entries) {
        listEl.appendChild(this.createHistoryCard(entry));
      }
    } catch (e: any) {
      listEl.innerHTML = `<div class="hai-history-empty">加载失败: ${e?.message || e}</div>`;
    }
  }

  private createHistoryCard(entry: any): HTMLElement {
    const el = document.createElement("div");
    el.className = "hai-history-card";
    const dirExists = entry.dirExists;
    const isCurrent = entry.cwd === this.currentCwd;
    el.innerHTML = `
      <div class="hai-history-cwd">${entry.cwd}${isCurrent ? ' <span class="hai-history-current">当前</span>' : ""}</div>
      <div class="hai-history-meta">${entry.lastActive} · ${entry.role} · ${entry.model}</div>
      <div class="hai-history-preview">${entry.preview || "(无预览)"}</div>
      <div class="hai-history-actions">
        <button class="btn hai-hist-continue" ${!dirExists ? "disabled" : ""}>继续</button>
        ${!dirExists ? '<button class="btn hai-hist-migrate">迁移</button>' : ""}
        <button class="btn hai-hist-delete">删除</button>
      </div>`;

    el.querySelector(".hai-hist-continue")!.addEventListener("click", async () => {
      this.historyDrawer.classList.add("hidden");
      this.switchView("chat");
      this.setStatus("恢复历史中…");
      try {
        await api.agentLoadHistory(this.tabId, entry.cwd);
        this.setStatus("已恢复历史对话");
        setTimeout(() => {
          if (this.statusEl.textContent === "已恢复历史对话") this.setStatus("就绪");
        }, 2000);
      } catch (e: any) {
        this.setStatus("恢复失败");
        toast(`恢复历史失败: ${e?.message || e}`);
      }
    });

    el.querySelector(".hai-hist-delete")!.addEventListener("click", () => {
      this.showConfirm(`确定删除 ${entry.cwd} 的历史？`, async () => {
        try {
          await api.agentDeleteHistory(entry.cwd);
          el.remove();
        } catch (e: any) {
          toast(`删除失败: ${e?.message || e}`);
        }
      });
    });

    const migrateBtn = el.querySelector(".hai-hist-migrate");
    if (migrateBtn) {
      migrateBtn.addEventListener("click", () => {
        this.showPrompt("迁移历史到新目录", entry.cwd, async (newCwd) => {
          if (newCwd !== entry.cwd) {
            try {
              await api.agentMigrateHistory(entry.cwd, newCwd);
              toast("迁移成功");
              void this.loadHistoryList();
            } catch (e: any) {
              toast(`迁移失败: ${e?.message || e}`);
            }
          }
        });
      });
    }

    return el;
  }

  // ---------- 显示 / 隐藏 / 销毁 ----------

  async show(spec: HaiSpec, cwd: string, panes: PaneInfo[], paneRect?: DOMRect): Promise<void> {
    // role 解析优先级：OSC 指定 → config.default_role → 弹角色选择器
    let role = spec.role;
    if (!role) {
      try {
        const config = await api.agentLoadConfig() as AiConfig;
        this.defaultRoleId = config.default_role || "";
        role = this.defaultRoleId;
      } catch { /* ignore */ }
    }
    if (!role) {
      role = (await this.showRolePicker()) || "general";
    }
    this.role = role;
    this.mode = spec.mode || "auto";
    this.currentCwd = cwd;

    const modeBadge = this.overlay.querySelector(".hai-mode-badge")!;
    modeBadge.textContent = this.mode.charAt(0).toUpperCase() + this.mode.slice(1);

    // 浮动覆盖层模式：跟随 pane 定位
    if (spec.w && paneRect) {
      this.overlay.classList.add("hai-floating");
      const modal = this.overlay.querySelector(".hai-modal") as HTMLElement;
      modal.style.left = `${paneRect.left}px`;
      modal.style.top = `${paneRect.top}px`;
      modal.style.width = `${paneRect.width}px`;
      modal.style.height = `${paneRect.height}px`;
    } else {
      this.overlay.classList.remove("hai-floating");
      const modal = this.overlay.querySelector(".hai-modal") as HTMLElement;
      modal.style.left = "";
      modal.style.top = "";
      modal.style.width = "";
      modal.style.height = "";
    }

    document.body.appendChild(this.overlay);
    this.overlay.style.display = "flex";
    window.addEventListener("keydown", this.onKey, true);

    // 确保渲染依赖已加载（避免首条消息渲染为纯文本）
    await ensureMarked();
    await ensureHljs();

    // 加载模型选择器（非阻塞，不阻止面板显示）
    void this.loadModelSelector();

    this.inputEl.focus();

    if (!this.spawned) {
      this.spawned = true;
      this.channel = new Channel<AgentEvent>();
      this.channel.onmessage = (event) => this.onEvent(event);

      this.setStatus("连接中…");

      try {
        await api.agentSpawn(this.tabId, this.mode, this.role, spec.msg || null, cwd, panes, this.channel);
      } catch (err: any) {
        this.setStatus("错误");
        this.appendError(String(err?.message ?? err));
      }

      if (spec.msg) {
        this.appendUserMessage(spec.msg);
      }
    }
  }

  hide(): void {
    this.overlay.style.display = "none";
    window.removeEventListener("keydown", this.onKey, true);
    if (this.overlay.parentElement) {
      this.overlay.parentElement.removeChild(this.overlay);
    }
  }

  async destroy(): Promise<void> {
    window.removeEventListener("keydown", this.onKey, true);
    if (this.overlay.parentElement) {
      this.overlay.parentElement.removeChild(this.overlay);
    }
    try {
      await api.agentDestroy(this.tabId);
    } catch {
      /* session 可能已不存在 */
    }
  }

  // ---------- 消息发送 ----------

  private send(): void {
    const text = this.inputEl.value.trim();
    if (!text || this.processing) return;
    this.inputEl.value = "";
    this.inputEl.style.height = "auto";

    this.appendUserMessage(text);
    api.agentSendMessage(this.tabId, text).catch((err) => {
      this.appendError(String(err?.message ?? err));
    });
  }

  private abort(): void {
    api.agentAbort(this.tabId).catch(() => {});
  }

  // ---------- 事件处理 ----------

  private onEvent(event: AgentEvent): void {
    switch (event.type) {
      case "message":
        this.onMessage(event.content, event.done);
        break;
      case "toolStart":
        this.onToolStart(event.tool, event.args);
        break;
      case "toolOutput":
        this.onToolOutput(event.output);
        break;
      case "toolEnd":
        this.onToolEnd(event.result);
        break;
      case "askApproval":
        this.onAskApproval(event.tool, event.args, event.reason);
        break;
      case "userQuestion":
        this.onUserQuestion(event.question, event.choices);
        break;
      case "readTerminalRequest":
        void this.onReadTerminalRequest(event.requestId, event.paneId, event.lines);
        break;
      case "proposedPlan":
        this.onProposedPlan(event.summary);
        break;
      case "retrying":
        this.setStatus(`重试中 (${event.attempt}/${event.maxAttempts}): ${event.reason}`);
        break;
      case "contextTrimmed":
        this.setStatus(`上下文截断: 移除 ${event.removedTools} 工具 + ${event.removedMessages} 消息`);
        break;
      case "historyRestored":
        this.onHistoryRestored(event.messages);
        break;
      case "historyCleared":
        this.onHistoryCleared();
        break;
      case "error":
        this.appendError(event.message);
        this.setProcessing(false);
        break;
      case "aborted":
        if (this.currentRenderer) {
          this.currentRenderer.done();
          this.currentRenderer = null;
        }
        this.setProcessing(false);
        this.setStatus("已中止");
        break;
      case "done":
        if (this.currentRenderer) {
          this.currentRenderer.done();
          this.currentRenderer = null;
        }
        this.setProcessing(false);
        this.setStatus("就绪");
        break;
    }
  }

  private onMessage(content: string, done: boolean): void {
    if (!content && done && !this.currentRenderer) return;

    if (!this.currentRenderer) {
      const { renderer } = this.appendAssistantBubble();
      this.currentRenderer = renderer;
      this.setProcessing(true);
      this.setStatus("思考中…");
    }

    if (content) {
      this.currentRenderer!.push(content);
    }

    if (done) {
      this.currentRenderer?.done();
      this.currentRenderer = null;
      this.setStatus("执行中…");
    }

    this.scrollToBottom();
  }

  // ---------- DOM 构建 ----------

  private appendUserMessage(text: string): void {
    this.hideEmptyState();
    const el = document.createElement("div");
    el.className = "hai-msg hai-msg-user";
    el.textContent = text;
    this.messagesEl.appendChild(el);
    this.turns.push({ role: "user", el });
    this.scrollToBottom();
  }

  private appendAssistantBubble(): { el: HTMLElement; renderer: StreamingMarkdown } {
    this.hideEmptyState();
    const el = document.createElement("div");
    el.className = "hai-msg hai-msg-assistant";

    // 模型标识
    const modelSelect = this.overlay.querySelector(".hai-model-select") as HTMLSelectElement;
    const modelName = modelSelect?.selectedOptions[0]?.text;
    if (modelName) {
      const badge = document.createElement("div");
      badge.className = "hai-msg-model-badge";
      badge.textContent = modelName;
      el.appendChild(badge);
    }

    const contentEl = document.createElement("div");
    contentEl.className = "hai-msg-content";
    el.appendChild(contentEl);

    // 复制按钮
    const copyBtn = document.createElement("button");
    copyBtn.className = "hai-msg-copy";
    copyBtn.innerHTML = `${ICONS.check}`;
    copyBtn.title = "复制";
    copyBtn.addEventListener("click", () => {
      const text = contentEl.textContent || "";
      navigator.clipboard.writeText(text).then(() => {
        copyBtn.classList.add("copied");
        setTimeout(() => copyBtn.classList.remove("copied"), 2000);
      }).catch(() => {});
    });
    el.appendChild(copyBtn);

    this.messagesEl.appendChild(el);

    const renderer = new StreamingMarkdown(contentEl);
    this.turns.push({ role: "assistant", el, renderer });
    return { el, renderer };
  }

  private hideEmptyState(): void {
    const empty = this.messagesEl.querySelector(".hai-empty-state");
    if (empty) empty.remove();
  }

  private appendError(message: string): void {
    const el = document.createElement("div");
    el.className = "hai-msg hai-msg-error";

    if (message.includes("未配置") || message.includes("未找到模型配置") || message.includes("ai-config.json")) {
      el.innerHTML = "";
      const text = document.createElement("div");
      text.textContent = `⚠ ${message}`;
      el.appendChild(text);
      const hint = document.createElement("div");
      hint.className = "hai-error-hint";
      const settingsBtn = document.createElement("button");
      settingsBtn.className = "hai-error-settings-btn";
      settingsBtn.textContent = "打开设置 →";
      settingsBtn.addEventListener("click", () => this.switchView("settings"));
      hint.appendChild(settingsBtn);
      el.appendChild(hint);
    } else {
      el.textContent = `⚠ ${message}`;
    }

    this.messagesEl.appendChild(el);
    this.scrollToBottom();
  }

  // ---------- 工具调用渲染 ----------

  private currentToolEl: HTMLElement | null = null;

  private onToolStart(tool: string, args: any): void {
    if (this.currentRenderer) {
      this.currentRenderer.done();
      this.currentRenderer = null;
    }

    const el = document.createElement("div");
    el.className = "hai-tool-call";
    el.innerHTML = `
      <div class="hai-tool-header">
        <span class="hai-tool-name">${tool}</span>
        <span class="hai-tool-status">执行中…</span>
      </div>
      <div class="hai-tool-args"></div>
      <div class="hai-tool-output hidden"></div>`;

    el.querySelector(".hai-tool-args")!.textContent = formatArgs(args);

    // header 点击折叠/展开
    el.querySelector(".hai-tool-header")!.addEventListener("click", () => {
      el.classList.toggle("hai-tool-collapsed");
    });

    this.messagesEl.appendChild(el);
    this.currentToolEl = el;
    this.setStatus("工具执行中…");
    this.scrollToBottom();
  }

  private onToolOutput(output: string): void {
    if (!this.currentToolEl) return;
    const outEl = this.currentToolEl.querySelector(".hai-tool-output")!;
    outEl.classList.remove("hidden");
    outEl.textContent += output;
    this.scrollToBottom();
  }

  private onToolEnd(result: ToolResult): void {
    if (!this.currentToolEl) return;

    const statusEl = this.currentToolEl.querySelector(".hai-tool-status")!;
    const outEl = this.currentToolEl.querySelector(".hai-tool-output")!;

    if (result.status === "success") {
      statusEl.textContent = "✓";
      statusEl.classList.add("hai-tool-ok");
      if (result.output) {
        outEl.classList.remove("hidden");
        outEl.textContent = result.output;
      }
      if (result.truncated) {
        const trunc = document.createElement("div");
        trunc.className = "hai-tool-truncated";
        trunc.textContent = "（输出已截断）";
        this.currentToolEl.appendChild(trunc);
      }
    } else if (result.status === "error") {
      statusEl.textContent = "✘";
      statusEl.classList.add("hai-tool-fail");
      outEl.classList.remove("hidden");
      outEl.textContent = result.message;
    } else {
      statusEl.textContent = "⊘";
      statusEl.classList.add("hai-tool-fail");
    }

    this.currentToolEl.classList.add("hai-tool-done");
    this.currentToolEl = null;
    this.setStatus("思考中…");
    this.scrollToBottom();
  }

  // ---------- Plan 确认 / Ask 确认 / AskUser 提问 / read_terminal ----------

  private onProposedPlan(summary: string): void {
    if (this.currentRenderer) {
      this.currentRenderer.done();
      this.currentRenderer = null;
    }

    const el = document.createElement("div");
    el.className = "hai-plan-approval";
    el.innerHTML = `
      <div class="hai-plan-header">执行计划</div>
      <div class="hai-plan-summary"></div>
      <div class="hai-plan-actions">
        <button class="btn hai-plan-approve">确认执行</button>
        <button class="btn hai-plan-reject">拒绝</button>
      </div>`;

    const summaryEl = el.querySelector(".hai-plan-summary")!;
    const w = window as any;
    if (w.__marked_parse) {
      summaryEl.innerHTML = w.__marked_parse(summary);
    } else {
      summaryEl.textContent = summary;
    }

    el.querySelector(".hai-plan-approve")!.addEventListener("click", () => {
      el.remove();
      api.agentApproveTool(this.tabId, true).catch(() => {});
    });
    el.querySelector(".hai-plan-reject")!.addEventListener("click", () => {
      el.remove();
      api.agentApproveTool(this.tabId, false).catch(() => {});
    });

    this.messagesEl.appendChild(el);
    this.scrollToBottom();
  }

  private onAskApproval(tool: string, args: any, reason: string): void {
    if (this.currentRenderer) {
      this.currentRenderer.done();
      this.currentRenderer = null;
    }

    const el = document.createElement("div");
    el.className = "hai-ask-approval";
    el.innerHTML = `
      <div class="hai-ask-header">工具调用确认</div>
      <div class="hai-ask-tool">${tool}</div>
      <div class="hai-ask-args"></div>
      <div class="hai-ask-reason"></div>
      <div class="hai-ask-actions">
        <button class="btn hai-ask-approve">批准</button>
        <button class="btn hai-ask-reject">拒绝</button>
      </div>`;

    el.querySelector(".hai-ask-args")!.textContent = formatArgs(args);
    el.querySelector(".hai-ask-reason")!.textContent = reason;

    el.querySelector(".hai-ask-approve")!.addEventListener("click", () => {
      el.remove();
      api.agentApproveTool(this.tabId, true).catch(() => {});
    });
    el.querySelector(".hai-ask-reject")!.addEventListener("click", () => {
      el.remove();
      api.agentApproveTool(this.tabId, false).catch(() => {});
    });

    this.messagesEl.appendChild(el);
    this.scrollToBottom();
  }

  private onUserQuestion(question: string, choices: UserChoice[]): void {
    if (this.currentRenderer) {
      this.currentRenderer.done();
      this.currentRenderer = null;
    }

    const el = document.createElement("div");
    el.className = "hai-user-question";
    el.innerHTML = `
      <div class="hai-uq-header">${question}</div>
      <div class="hai-uq-choices"></div>
      <div class="hai-uq-free">
        <input type="text" class="hai-uq-input" placeholder="自由输入…" />
        <button class="btn hai-uq-send">发送</button>
      </div>`;

    const choicesEl = el.querySelector(".hai-uq-choices")!;
    for (const choice of choices) {
      const btn = document.createElement("button");
      btn.className = "btn hai-uq-choice";
      btn.innerHTML = `<div class="hai-uq-label">${choice.label}</div>${choice.description ? `<div class="hai-uq-desc">${choice.description}</div>` : ""}`;
      btn.addEventListener("click", () => {
        el.remove();
        api.agentAnswerQuestion(this.tabId, choice.action || choice.label).catch(() => {});
      });
      choicesEl.appendChild(btn);
    }

    const input = el.querySelector(".hai-uq-input") as HTMLInputElement;
    const sendFree = () => {
      const val = input.value.trim();
      if (!val) return;
      el.remove();
      api.agentAnswerQuestion(this.tabId, val).catch(() => {});
    };
    el.querySelector(".hai-uq-send")!.addEventListener("click", sendFree);
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") { e.preventDefault(); sendFree(); }
    });

    this.messagesEl.appendChild(el);
    this.scrollToBottom();
  }

  private async onReadTerminalRequest(requestId: string, paneId: string, lines: number): Promise<void> {
    if (!this.onReadTerminal) {
      api.agentTerminalData(this.tabId, requestId, "").catch(() => {});
      return;
    }
    try {
      const data = await this.onReadTerminal(paneId, lines);
      api.agentTerminalData(this.tabId, requestId, data).catch(() => {});
    } catch {
      api.agentTerminalData(this.tabId, requestId, "").catch(() => {});
    }
  }

  // ---------- UI 辅助 ----------

  private setProcessing(processing: boolean): void {
    this.processing = processing;
    this.sendBtn.classList.toggle("hidden", processing);
    this.abortBtn.classList.toggle("hidden", !processing);
  }

  private setStatus(text: string): void {
    this.statusEl.textContent = text;
  }

  private scrollToBottom(): void {
    requestAnimationFrame(() => {
      this.messagesEl.scrollTop = this.messagesEl.scrollHeight;
    });
  }

  // ---------- 自定义对话框（替换 prompt/confirm） ----------

  private showConfirm(message: string, onConfirm: () => void): HTMLElement {
    const overlay = document.createElement("div");
    overlay.className = "hai-dialog-overlay";
    overlay.innerHTML = `
      <div class="hai-dialog">
        <div class="hai-dialog-msg">${message}</div>
        <div class="hai-dialog-actions">
          <button class="btn hai-dialog-cancel">取消</button>
          <button class="btn hai-dialog-ok">确定</button>
        </div>
      </div>`;
    overlay.querySelector(".hai-dialog-ok")!.addEventListener("click", () => { overlay.remove(); onConfirm(); });
    overlay.querySelector(".hai-dialog-cancel")!.addEventListener("click", () => overlay.remove());
    overlay.addEventListener("click", (e) => { if (e.target === overlay) overlay.remove(); });
    this.overlay.querySelector(".hai-modal")!.appendChild(overlay);
    return overlay;
  }

  private showPrompt(label: string, defaultValue: string, onConfirm: (value: string) => void): HTMLElement {
    const overlay = document.createElement("div");
    overlay.className = "hai-dialog-overlay";
    overlay.innerHTML = `
      <div class="hai-dialog">
        <div class="hai-dialog-label">${label}</div>
        <input type="text" class="hai-dialog-input" value="${defaultValue}" />
        <div class="hai-dialog-actions">
          <button class="btn hai-dialog-cancel">取消</button>
          <button class="btn hai-dialog-ok">确定</button>
        </div>
      </div>`;
    const input = overlay.querySelector(".hai-dialog-input") as HTMLInputElement;
    const confirmDialog = () => { const v = input.value.trim(); if (v) { overlay.remove(); onConfirm(v); } };
    overlay.querySelector(".hai-dialog-ok")!.addEventListener("click", confirmDialog);
    overlay.querySelector(".hai-dialog-cancel")!.addEventListener("click", () => overlay.remove());
    input.addEventListener("keydown", (e) => { if (e.key === "Enter") confirmDialog(); });
    overlay.addEventListener("click", (e) => { if (e.target === overlay) overlay.remove(); });
    this.overlay.querySelector(".hai-modal")!.appendChild(overlay);
    requestAnimationFrame(() => input.focus());
    return overlay;
  }

  // ---------- 历史恢复 / 清除 ----------

  private onHistoryRestored(messages: HistoryEntry[]): void {
    // 清空旧消息（用于 LoadHistory 场景）
    this.messagesEl.innerHTML = "";
    this.turns = [];
    this.currentRenderer = null;
    this.currentToolEl = null;

    for (const msg of messages) {
      if (msg.role === "user") {
        this.appendUserMessage(msg.content);
      } else if (msg.role === "assistant") {
        const { renderer } = this.appendAssistantBubble();
        renderer.push(msg.content);
        renderer.done();
      }
    }
    this.scrollToBottom();
  }

  private onHistoryCleared(): void {
    this.messagesEl.innerHTML = "";
    this.turns = [];
    this.currentRenderer = null;
    this.currentToolEl = null;
    this.setProcessing(false);
    this.setStatus("就绪");
  }

  // ---------- 玻璃模式 / 主题 ----------

  private toggleGlass(): void {
    this.glassMode = !this.glassMode;
    localStorage.setItem("hai-glass", String(this.glassMode));
    this.applyGlass();
  }

  private applyGlass(): void {
    const modal = this.overlay.querySelector(".hai-modal") as HTMLElement;
    modal.dataset.glass = String(this.glassMode);
    this.glassBtn.classList.toggle("active", this.glassMode);
  }

  private cycleTheme(): void {
    const order: ("auto" | "light" | "dark")[] = ["auto", "light", "dark"];
    const idx = order.indexOf(this.themeMode);
    this.themeMode = order[(idx + 1) % order.length];
    localStorage.setItem("hai-theme", this.themeMode);
    this.applyTheme();
  }

  private applyTheme(): void {
    const modal = this.overlay.querySelector(".hai-modal") as HTMLElement;
    modal.dataset.theme = this.themeMode;
    this.themeBtn.classList.toggle("active", this.themeMode !== "auto");
  }

  static preload(): void {
    ensureMarked().catch(() => {});
    ensureHljs().catch(() => {});
  }
}

// ---------- 辅助函数 ----------

function formatArgs(args: any): string {
  if (!args || typeof args !== "object") return String(args ?? "");
  return JSON.stringify(args, null, 2);
}
