/** AgentModal — Hai Agent 前端面板。
 *
 * Modal 生命周期：
 * - hai 命令 → OSC 1733 → show()：若已有 session 则恢复 hidden Modal，否则新建。
 * - ESC → 中止当前对话；Alt+H → 隐藏面板（session 保留）。
 * - Tab 关闭 → destroy()，invoke agent_destroy。
 *
 * 视图切换：[Chat] / [设置] 标签，切换时会话保持运行。
 */

import { Channel } from "@tauri-apps/api/core";
import { api } from "../ipc";
import type { AgentEvent, HaiSpec } from "./protocol";
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

// ---------- 类型 ----------

interface EndpointConfig {
  url?: string;
  key: string;
  max_tokens: number;
  temperature: number;
  top_p?: number;
  frequency_penalty?: number;
  presence_penalty?: number;
  stop?: string[];
  seed?: number;
}

interface AiConfig {
  providers: Record<string, Record<string, EndpointConfig[]>>;
  default_model: string;
  default_provider: string;
  execution?: {
    default_mode?: string;
    dangerous_commands?: string[];
    always_ask_for?: string[];
  };
  roles?: Record<string, { model?: string; provider?: string }>;
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

  private tabId: string;
  private role: string;
  private mode: string;

  private channel: Channel<AgentEvent> | null = null;
  private spawned = false;
  private processing = false;
  private currentRenderer: StreamingMarkdown | null = null;
  private turns: ChatTurn[] = [];
  private configLoaded = false;

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
          <div class="hai-tabs">
            <button class="hai-tab active" data-view="chat">Chat</button>
            <button class="hai-tab" data-view="settings">设置</button>
          </div>
          <div class="hai-header-tools">
            <span class="hai-mode-badge">Auto</span>
            <button class="btn hai-btn-close" title="隐藏 (Alt+H)">隐藏</button>
          </div>
        </div>
        <div class="hai-view hai-view-chat active">
          <div class="hai-messages"></div>
          <div class="hai-input-bar">
            <textarea class="hai-input" rows="1" placeholder="输入消息… (Enter 发送, Shift+Enter 换行)"></textarea>
            <button class="btn hai-btn-send" title="发送">发送</button>
            <button class="btn hai-btn-abort hidden" title="中止 (ESC)">⏸ 中止</button>
          </div>
          <div class="hai-status-bar">
            <span class="hai-status">就绪</span>
          </div>
        </div>
        <div class="hai-view hai-view-settings">
          <div class="hai-settings-body">
            <div class="hai-settings-section">
              <h4>默认模型</h4>
              <div class="hai-form-row">
                <label>Provider</label>
                <select class="hai-cfg-provider">
                  <option value="openai">openai</option>
                  <option value="anthropic">anthropic</option>
                </select>
              </div>
              <div class="hai-form-row">
                <label>模型 ID</label>
                <input type="text" class="hai-cfg-model" placeholder="deepseek-v3" />
              </div>
            </div>
            <div class="hai-settings-section">
              <h4>API 端点</h4>
              <div class="hai-form-row">
                <label>URL</label>
                <input type="text" class="hai-cfg-url" placeholder="https://api.deepseek.com/v1" />
              </div>
              <div class="hai-form-row">
                <label>API Key</label>
                <input type="password" class="hai-cfg-key" placeholder="sk-..." />
              </div>
            </div>
            <div class="hai-settings-section">
              <h4>生成参数</h4>
              <div class="hai-form-row">
                <label>Max Tokens</label>
                <input type="number" class="hai-cfg-max-tokens" value="8192" min="1" />
              </div>
              <div class="hai-form-row">
                <label>Temperature</label>
                <input type="number" class="hai-cfg-temperature" value="0.7" min="0" max="2" step="0.1" />
              </div>
              <div class="hai-form-row">
                <label>Top P</label>
                <input type="number" class="hai-cfg-top-p" min="0" max="1" step="0.1" placeholder="留空使用默认" />
              </div>
              <div class="hai-form-row">
                <label>Frequency Penalty</label>
                <input type="number" class="hai-cfg-freq-penalty" min="-2" max="2" step="0.1" placeholder="留空使用默认" />
              </div>
              <div class="hai-form-row">
                <label>Presence Penalty</label>
                <input type="number" class="hai-cfg-pres-penalty" min="-2" max="2" step="0.1" placeholder="留空使用默认" />
              </div>
              <div class="hai-form-row">
                <label>Stop（逗号分隔）</label>
                <input type="text" class="hai-cfg-stop" placeholder="留空使用默认" />
              </div>
              <div class="hai-form-row">
                <label>Seed</label>
                <input type="number" class="hai-cfg-seed" placeholder="留空使用默认" />
              </div>
            </div>
          </div>
          <div class="hai-settings-actions">
            <button class="btn hai-btn-save-settings">保存</button>
            <span class="hai-settings-hint"></span>
          </div>
        </div>
      </div>`;

    this.chatView = this.overlay.querySelector(".hai-view-chat")!;
    this.settingsView = this.overlay.querySelector(".hai-view-settings")!;
    this.messagesEl = this.overlay.querySelector(".hai-messages")!;
    this.inputEl = this.overlay.querySelector(".hai-input")!;
    this.sendBtn = this.overlay.querySelector(".hai-btn-send")!;
    this.abortBtn = this.overlay.querySelector(".hai-btn-abort")!;
    this.statusEl = this.overlay.querySelector(".hai-status")!;
    this.closeBtn = this.overlay.querySelector(".hai-btn-close")!;

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

    // 点击 overlay 空白处关闭
    this.overlay.addEventListener("mousedown", (e) => {
      if (e.target === this.overlay) this.hide();
    });

    this.closeBtn.addEventListener("click", () => this.hide());
    this.sendBtn.addEventListener("click", () => this.send());
    this.abortBtn.addEventListener("click", () => this.abort());

    // 标签切换
    this.overlay.querySelectorAll<HTMLElement>(".hai-tab").forEach((tab) => {
      tab.addEventListener("click", () => {
        const view = tab.dataset.view;
        if (view) this.switchView(view as "chat" | "settings");
      });
    });

    // 保存设置
    this.overlay.querySelector(".hai-btn-save-settings")!.addEventListener("click", () => {
      void this.saveSettings();
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
      this.inputEl.style.height = Math.min(this.inputEl.scrollHeight, 150) + "px";
    });
  }

  // ---------- 视图切换 ----------

  private switchView(view: "chat" | "settings"): void {
    this.chatView.classList.toggle("active", view === "chat");
    this.settingsView.classList.toggle("active", view === "settings");
    this.overlay.querySelectorAll(".hai-tab").forEach((tab) => {
      (tab as HTMLElement).classList.toggle("active", tab.dataset.view === view);
    });
    if (view === "settings" && !this.configLoaded) {
      void this.loadSettings();
    }
    if (view === "chat") {
      this.inputEl.focus();
    }
  }

  // ---------- 设置面板 ----------

  private async loadSettings(): Promise<void> {
    const hint = this.overlay.querySelector(".hai-settings-hint") as HTMLElement;
    hint.textContent = "加载中…";
    try {
      const config = await api.agentLoadConfig() as AiConfig;
      this.populateSettingsForm(config);
      this.configLoaded = true;
      hint.textContent = "";
    } catch (err: any) {
      hint.textContent = `加载失败: ${err?.message ?? err}`;
    }
  }

  private populateSettingsForm(config: AiConfig): void {
    const provider = config.default_provider || "openai";
    const model = config.default_model || "";
    const endpoints = config.providers?.[provider]?.[model];
    const ep = endpoints?.[0] ?? ({} as EndpointConfig);

    (this.overlay.querySelector(".hai-cfg-provider") as HTMLSelectElement).value = provider;
    (this.overlay.querySelector(".hai-cfg-model") as HTMLInputElement).value = model;
    (this.overlay.querySelector(".hai-cfg-url") as HTMLInputElement).value = ep.url ?? "";
    (this.overlay.querySelector(".hai-cfg-key") as HTMLInputElement).value = ep.key ?? "";
    (this.overlay.querySelector(".hai-cfg-max-tokens") as HTMLInputElement).value =
      String(ep.max_tokens ?? 8192);
    (this.overlay.querySelector(".hai-cfg-temperature") as HTMLInputElement).value =
      String(ep.temperature ?? 0.7);
    (this.overlay.querySelector(".hai-cfg-top-p") as HTMLInputElement).value =
      ep.top_p != null ? String(ep.top_p) : "";
    (this.overlay.querySelector(".hai-cfg-freq-penalty") as HTMLInputElement).value =
      ep.frequency_penalty != null ? String(ep.frequency_penalty) : "";
    (this.overlay.querySelector(".hai-cfg-pres-penalty") as HTMLInputElement).value =
      ep.presence_penalty != null ? String(ep.presence_penalty) : "";
    (this.overlay.querySelector(".hai-cfg-stop") as HTMLInputElement).value =
      ep.stop?.join(", ") ?? "";
    (this.overlay.querySelector(".hai-cfg-seed") as HTMLInputElement).value =
      ep.seed != null ? String(ep.seed) : "";
  }

  private async saveSettings(): Promise<void> {
    const hint = this.overlay.querySelector(".hai-settings-hint") as HTMLElement;
    hint.textContent = "保存中…";

    const provider = (this.overlay.querySelector(".hai-cfg-provider") as HTMLSelectElement).value;
    const model = (this.overlay.querySelector(".hai-cfg-model") as HTMLInputElement).value.trim();
    const url = (this.overlay.querySelector(".hai-cfg-url") as HTMLInputElement).value.trim();
    const key = (this.overlay.querySelector(".hai-cfg-key") as HTMLInputElement).value.trim();
    const maxTokens = parseInt(
      (this.overlay.querySelector(".hai-cfg-max-tokens") as HTMLInputElement).value || "8192",
      10,
    );
    const temperature = parseFloat(
      (this.overlay.querySelector(".hai-cfg-temperature") as HTMLInputElement).value || "0.7",
    );
    const topPStr = (this.overlay.querySelector(".hai-cfg-top-p") as HTMLInputElement).value.trim();
    const freqPenaltyStr = (this.overlay.querySelector(".hai-cfg-freq-penalty") as HTMLInputElement).value.trim();
    const presPenaltyStr = (this.overlay.querySelector(".hai-cfg-pres-penalty") as HTMLInputElement).value.trim();
    const stopStr = (this.overlay.querySelector(".hai-cfg-stop") as HTMLInputElement).value.trim();
    const seedStr = (this.overlay.querySelector(".hai-cfg-seed") as HTMLInputElement).value.trim();

    if (!model) {
      hint.textContent = "模型 ID 不能为空";
      return;
    }
    if (!key) {
      hint.textContent = "API Key 不能为空";
      return;
    }

    // 加载现有配置（保留 roles、execution 等），仅修改 providers + default
    let config: AiConfig;
    try {
      config = await api.agentLoadConfig() as AiConfig;
    } catch {
      config = { providers: {}, default_model: "", default_provider: "" };
    }

    config.default_provider = provider;
    config.default_model = model;
    if (!config.providers) config.providers = {};
    if (!config.providers[provider]) config.providers[provider] = {};

    const endpoint: EndpointConfig = {
      url: url || undefined,
      key,
      max_tokens: maxTokens || 8192,
      temperature: Number.isFinite(temperature) ? temperature : 0.7,
    };
    if (topPStr) endpoint.top_p = parseFloat(topPStr);
    if (freqPenaltyStr) endpoint.frequency_penalty = parseFloat(freqPenaltyStr);
    if (presPenaltyStr) endpoint.presence_penalty = parseFloat(presPenaltyStr);
    if (stopStr) {
      endpoint.stop = stopStr.split(",").map((s) => s.trim()).filter(Boolean);
    }
    if (seedStr) endpoint.seed = parseInt(seedStr, 10);

    config.providers[provider][model] = [endpoint];

    try {
      await api.agentSaveConfig(config);
      hint.textContent = "已保存";
      // 下次对话时新配置生效（session_loop 每次请求前重新加载 config）
      setTimeout(() => {
        if (hint.textContent === "已保存") hint.textContent = "";
      }, 3000);
    } catch (err: any) {
      hint.textContent = `保存失败: ${err?.message ?? err}`;
    }
  }

  // ---------- 显示 / 隐藏 / 销毁 ----------

  /** 显示 Modal。若首次则创建 Channel + invoke agent_spawn。 */
  async show(spec: HaiSpec): Promise<void> {
    this.role = spec.role || "general";
    this.mode = spec.mode || "auto";

    const modeBadge = this.overlay.querySelector(".hai-mode-badge")!;
    modeBadge.textContent = this.mode.charAt(0).toUpperCase() + this.mode.slice(1);

    document.body.appendChild(this.overlay);
    this.overlay.style.display = "flex";
    window.addEventListener("keydown", this.onKey, true);
    this.inputEl.focus();

    // 首次：创建 Channel + spawn session
    if (!this.spawned) {
      this.spawned = true;
      this.channel = new Channel<AgentEvent>();
      this.channel.onmessage = (event) => this.onEvent(event);

      this.setStatus("连接中…");

      try {
        await api.agentSpawn(this.tabId, this.mode, this.role, spec.msg || null, this.channel);
      } catch (err: any) {
        this.setStatus("错误");
        this.appendError(String(err?.message ?? err));
      }

      // 若有初始消息，在 UI 中展示
      if (spec.msg) {
        this.appendUserMessage(spec.msg);
      }
    }
  }

  /** 隐藏 Modal（Alt+H / 隐藏按钮）。Session 保留，监听移除。 */
  hide(): void {
    this.overlay.style.display = "none";
    window.removeEventListener("keydown", this.onKey, true);
    if (this.overlay.parentElement) {
      this.overlay.parentElement.removeChild(this.overlay);
    }
  }

  /** 销毁 Session（Tab 关闭时调用）。 */
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
    // 空内容 + done + 无活跃 renderer → LLM 返回空响应，不创建空气泡
    if (!content && done && !this.currentRenderer) return;

    // 首次 chunk → 创建 assistant 消息气泡 + StreamingMarkdown
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
      this.setProcessing(false);
      this.setStatus("就绪");
    }

    this.scrollToBottom();
  }

  // ---------- DOM 构建 ----------

  private appendUserMessage(text: string): void {
    const el = document.createElement("div");
    el.className = "hai-msg hai-msg-user";
    el.textContent = text;
    this.messagesEl.appendChild(el);
    this.turns.push({ role: "user", el });
    this.scrollToBottom();
  }

  private appendAssistantBubble(): { el: HTMLElement; renderer: StreamingMarkdown } {
    const el = document.createElement("div");
    el.className = "hai-msg hai-msg-assistant";
    const contentEl = document.createElement("div");
    contentEl.className = "hai-msg-content";
    el.appendChild(contentEl);
    this.messagesEl.appendChild(el);

    const renderer = new StreamingMarkdown(contentEl);
    this.turns.push({ role: "assistant", el, renderer });
    return { el, renderer };
  }

  private appendError(message: string): void {
    const el = document.createElement("div");
    el.className = "hai-msg hai-msg-error";
    el.textContent = `⚠ ${message}`;
    this.messagesEl.appendChild(el);
    this.scrollToBottom();
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

  /** 懒加载渲染依赖（在 show 前调用）。 */
  static preload(): void {
    ensureMarked().catch(() => {});
    ensureHljs().catch(() => {});
  }
}
