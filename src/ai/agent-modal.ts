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
import { toast } from "../ui";
import type { AgentEvent, HaiSpec, HistoryEntry, PaneInfo, ToolResult, UserChoice } from "./protocol";
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
  };
  roles?: Record<string, { model?: string }>;
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
  private glassBtn: HTMLElement;
  private themeBtn: HTMLElement;
  private clearBtn: HTMLElement;
  private historyView: HTMLElement;

  private tabId: string;
  private role: string;
  private mode: string;
  private glassMode = false;
  private themeMode: "auto" | "light" | "dark" = "auto";
  private currentCwd = "";

  /** 外部设置的终端读取回调（main.ts 注入，用于 read_terminal 工具） */
  onReadTerminal: ((paneId: string, lines: number) => Promise<string>) | null = null;

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
            <button class="hai-tab" data-view="history">历史</button>
          </div>
          <div class="hai-header-tools">
            <button class="btn hai-btn-glass" title="玻璃模式">🔲</button>
            <button class="btn hai-btn-theme" title="主题切换">🌓</button>
            <button class="btn hai-btn-clear" title="清除对话历史">🗑</button>
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
                <label>模型分组 ID</label>
                <input type="text" class="hai-cfg-model" placeholder="deepseek-v3" />
              </div>
              <div class="hai-form-row">
                <label>显示名称</label>
                <input type="text" class="hai-cfg-showname" placeholder="DeepSeek V3（留空则用分组 ID）" />
              </div>
            </div>
            <div class="hai-settings-section">
              <h4>API 端点</h4>
              <div class="hai-form-row">
                <label>URL</label>
                <input type="text" class="hai-cfg-url" placeholder="https://api.deepseek.com/v1" />
              </div>
              <div class="hai-form-row">
                <label>API Model ID</label>
                <input type="text" class="hai-cfg-endpoint-id" placeholder="留空则用分组 ID" />
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
              <div class="hai-form-row">
                <label>请求超时(秒)</label>
                <input type="number" class="hai-cfg-request-timeout" placeholder="留空=120" min="5" />
              </div>
              <div class="hai-form-row">
                <label>流式超时(秒)</label>
                <input type="number" class="hai-cfg-stream-timeout" placeholder="留空=60" min="5" />
              </div>
            </div>
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
        <div class="hai-view hai-view-history">
          <div class="hai-history-list"></div>
        </div>
      </div>`;

    this.chatView = this.overlay.querySelector(".hai-view-chat")!;
    this.settingsView = this.overlay.querySelector(".hai-view-settings")!;
    this.historyView = this.overlay.querySelector(".hai-view-history")!;
    this.messagesEl = this.overlay.querySelector(".hai-messages")!;
    this.inputEl = this.overlay.querySelector(".hai-input")!;
    this.sendBtn = this.overlay.querySelector(".hai-btn-send")!;
    this.abortBtn = this.overlay.querySelector(".hai-btn-abort")!;
    this.statusEl = this.overlay.querySelector(".hai-status")!;
    this.closeBtn = this.overlay.querySelector(".hai-btn-close")!;
    this.glassBtn = this.overlay.querySelector(".hai-btn-glass")!;
    this.themeBtn = this.overlay.querySelector(".hai-btn-theme")!;
    this.clearBtn = this.overlay.querySelector(".hai-btn-clear")!;

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
      if (confirm("确定清除对话历史？")) {
        api.agentClearHistory(this.tabId).catch(() => {});
      }
    });
    this.sendBtn.addEventListener("click", () => this.send());
    this.abortBtn.addEventListener("click", () => this.abort());

    // 标签切换
    this.overlay.querySelectorAll<HTMLElement>(".hai-tab").forEach((tab) => {
      tab.addEventListener("click", () => {
        const view = tab.dataset.view;
        if (view) this.switchView(view as "chat" | "settings" | "history");
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

  private switchView(view: "chat" | "settings" | "history"): void {
    this.chatView.classList.toggle("active", view === "chat");
    this.settingsView.classList.toggle("active", view === "settings");
    this.historyView.classList.toggle("active", view === "history");
    this.overlay.querySelectorAll<HTMLElement>(".hai-tab").forEach((tab) => {
      tab.classList.toggle("active", tab.dataset.view === view);
    });
    if (view === "settings" && !this.configLoaded) {
      void this.loadSettings();
    }
    if (view === "history") {
      void this.loadHistoryList();
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
    const providerCfg = config.providers?.[provider];
    const model = providerCfg?.default_model || "";
    const group = providerCfg?.models?.[model];
    const ep = group?.endpoints?.[0] ?? ({} as EndpointConfig);
    const opts = ep.options ?? ({} as GenOptions);

    (this.overlay.querySelector(".hai-cfg-provider") as HTMLSelectElement).value = provider;
    (this.overlay.querySelector(".hai-cfg-model") as HTMLInputElement).value = model;
    (this.overlay.querySelector(".hai-cfg-showname") as HTMLInputElement).value = group?.show_name ?? "";
    (this.overlay.querySelector(".hai-cfg-url") as HTMLInputElement).value = ep.url ?? "";
    (this.overlay.querySelector(".hai-cfg-endpoint-id") as HTMLInputElement).value = ep.id ?? "";
    (this.overlay.querySelector(".hai-cfg-key") as HTMLInputElement).value = ep.key ?? "";
    (this.overlay.querySelector(".hai-cfg-max-tokens") as HTMLInputElement).value =
      String(opts.max_tokens ?? 8192);
    (this.overlay.querySelector(".hai-cfg-temperature") as HTMLInputElement).value =
      String(opts.temperature ?? 0.7);
    (this.overlay.querySelector(".hai-cfg-top-p") as HTMLInputElement).value =
      opts.top_p != null ? String(opts.top_p) : "";
    (this.overlay.querySelector(".hai-cfg-freq-penalty") as HTMLInputElement).value =
      opts.frequency_penalty != null ? String(opts.frequency_penalty) : "";
    (this.overlay.querySelector(".hai-cfg-pres-penalty") as HTMLInputElement).value =
      opts.presence_penalty != null ? String(opts.presence_penalty) : "";
    (this.overlay.querySelector(".hai-cfg-stop") as HTMLInputElement).value =
      opts.stop?.join(", ") ?? "";
    (this.overlay.querySelector(".hai-cfg-seed") as HTMLInputElement).value =
      opts.seed != null ? String(opts.seed) : "";
    (this.overlay.querySelector(".hai-cfg-request-timeout") as HTMLInputElement).value =
      opts.request_timeout != null ? String(opts.request_timeout) : "";
    (this.overlay.querySelector(".hai-cfg-stream-timeout") as HTMLInputElement).value =
      opts.stream_chunk_timeout != null ? String(opts.stream_chunk_timeout) : "";
    const exec = config.execution ?? {};
    (this.overlay.querySelector(".hai-cfg-command-timeout") as HTMLInputElement).value =
      exec.command_timeout != null ? String(exec.command_timeout) : "";
  }

  private async saveSettings(): Promise<void> {
    const hint = this.overlay.querySelector(".hai-settings-hint") as HTMLElement;
    hint.textContent = "保存中…";

    const provider = (this.overlay.querySelector(".hai-cfg-provider") as HTMLSelectElement).value;
    const model = (this.overlay.querySelector(".hai-cfg-model") as HTMLInputElement).value.trim();
    const showName = (this.overlay.querySelector(".hai-cfg-showname") as HTMLInputElement).value.trim();
    const url = (this.overlay.querySelector(".hai-cfg-url") as HTMLInputElement).value.trim();
    const endpointId = (this.overlay.querySelector(".hai-cfg-endpoint-id") as HTMLInputElement).value.trim();
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
      hint.textContent = "模型分组 ID 不能为空";
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
      config = { providers: {}, default_provider: "openai" };
    }

    config.default_provider = provider;
    if (!config.providers) config.providers = {};
    if (!config.providers[provider]) config.providers[provider] = { default_model: "", models: {} };
    config.providers[provider].default_model = model;

    // 构建 options
    const options: GenOptions = {
      max_tokens: maxTokens || 8192,
      temperature: Number.isFinite(temperature) ? temperature : 0.7,
    };
    if (topPStr) options.top_p = parseFloat(topPStr);
    if (freqPenaltyStr) options.frequency_penalty = parseFloat(freqPenaltyStr);
    if (presPenaltyStr) options.presence_penalty = parseFloat(presPenaltyStr);
    if (stopStr) options.stop = stopStr.split(",").map((s) => s.trim()).filter(Boolean);
    if (seedStr) options.seed = parseInt(seedStr, 10);

    const reqTimeoutStr = (this.overlay.querySelector(".hai-cfg-request-timeout") as HTMLInputElement).value.trim();
    const streamTimeoutStr = (this.overlay.querySelector(".hai-cfg-stream-timeout") as HTMLInputElement).value.trim();
    if (reqTimeoutStr) options.request_timeout = parseInt(reqTimeoutStr, 10);
    if (streamTimeoutStr) options.stream_chunk_timeout = parseInt(streamTimeoutStr, 10);

    // 命令超时写入 execution
    const cmdTimeoutStr = (this.overlay.querySelector(".hai-cfg-command-timeout") as HTMLInputElement).value.trim();
    if (cmdTimeoutStr) {
      if (!config.execution) config.execution = {};
      config.execution.command_timeout = parseInt(cmdTimeoutStr, 10);
    }

    // 构建 endpoint
    const endpoint: EndpointConfig = {
      key,
      options,
    };
    if (url) endpoint.url = url;
    if (endpointId) endpoint.id = endpointId;

    // 构建 model group
    const group: ModelGroup = {
      endpoints: [endpoint],
    };
    if (showName) group.show_name = showName;

    config.providers[provider].models[model] = group;

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

  /** 显示 Modal。若首次则创建 Channel + invoke agent_spawn。
   *  withShell=true 时为浮动覆盖层模式，跟随 pane 定位。 */
  async show(spec: HaiSpec, cwd: string, panes: PaneInfo[], paneRect?: DOMRect): Promise<void> {
    this.role = spec.role || "general";
    this.mode = spec.mode || "auto";
    this.currentCwd = cwd;

    const modeBadge = this.overlay.querySelector(".hai-mode-badge")!;
    modeBadge.textContent = this.mode.charAt(0).toUpperCase() + this.mode.slice(1);

    // 浮动覆盖层模式：跟随 pane 定位，紧凑尺寸
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
    this.inputEl.focus();

    // 首次：创建 Channel + spawn session
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
      // Message done 仅表示该条消息的文本流结束，不代表整个轮次结束。
      // 可能 LLM 还要调工具（ReAct 循环）。processing 由 Done 事件关闭。
      this.currentRenderer?.done();
      this.currentRenderer = null;
      this.setStatus("执行中…");
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

    // 配置相关错误 → 引导用户去设置
    if (message.includes("未配置") || message.includes("未找到模型配置") || message.includes("ai-config.json")) {
      el.innerHTML = "";
      const text = document.createElement("div");
      text.textContent = `⚠ ${message}`;
      el.appendChild(text);
      const hint = document.createElement("div");
      hint.className = "hai-error-hint";
      hint.innerHTML = `请点击右上角 <strong>⚙ 设置</strong> 配置模型和 API Key`;
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
    // 若有活跃的消息 renderer，先冻结（文本阶段结束，工具阶段开始）
    if (this.currentRenderer) {
      this.currentRenderer.done();
      this.currentRenderer = null;
    }

    const el = document.createElement("div");
    el.className = "hai-tool-call";
    el.innerHTML = `
      <div class="hai-tool-header">
        <span class="hai-tool-icon">🔧</span>
        <span class="hai-tool-name">${tool}</span>
        <span class="hai-tool-status">执行中…</span>
      </div>
      <div class="hai-tool-args"></div>
      <div class="hai-tool-output hidden"></div>`;

    const argsEl = el.querySelector(".hai-tool-args")!;
    argsEl.textContent = formatArgs(args);

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
      // userRejected
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
      <div class="hai-plan-header">📋 执行计划</div>
      <div class="hai-plan-summary"></div>
      <div class="hai-plan-actions">
        <button class="btn hai-plan-approve">确认执行 ✔</button>
        <button class="btn hai-plan-reject">拒绝 ✘</button>
      </div>`;

    // summary 是 Markdown 文本，用 marked 渲染
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
      <div class="hai-ask-header">⚠ 工具调用确认</div>
      <div class="hai-ask-tool">${tool}</div>
      <div class="hai-ask-args"></div>
      <div class="hai-ask-reason">${reason}</div>
      <div class="hai-ask-actions">
        <button class="btn hai-ask-approve">批准 ✔</button>
        <button class="btn hai-ask-reject">拒绝 ✘</button>
      </div>`;

    el.querySelector(".hai-ask-args")!.textContent = formatArgs(args);

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
      <div class="hai-uq-header">❓ ${question}</div>
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

    // 自由输入
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

  // ---------- 历史恢复 / 清除 / 浏览 ----------

  /** 加载全局历史索引列表 */
  private async loadHistoryList(): Promise<void> {
    const listEl = this.historyView.querySelector(".hai-history-list") as HTMLElement;
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

  /** 创建历史卡片 */
  private createHistoryCard(entry: any): HTMLElement {
    const el = document.createElement("div");
    el.className = "hai-history-card";
    const dirExists = entry.dirExists;
    el.innerHTML = `
      <div class="hai-history-cwd">${entry.cwd}</div>
      <div class="hai-history-meta">${entry.lastActive} · ${entry.role} · ${entry.model}</div>
      <div class="hai-history-preview">${entry.preview || "(无预览)"}</div>
      <div class="hai-history-actions">
        <button class="btn hai-hist-continue">继续</button>
        ${dirExists ? "" : '<button class="btn hai-hist-migrate">迁移</button>'}
        <button class="btn hai-hist-delete">删除</button>
      </div>`;

    el.querySelector(".hai-hist-continue")!.addEventListener("click", () => {
      this.switchView("chat");
      this.setStatus(`恢复 ${entry.cwd} 的历史…`);
      // 通过后端加载该目录的历史（如果当前 cwd 不同，提示用户切换目录）
      if (entry.cwd !== this.currentCwd) {
        this.setStatus(`请在 ${entry.cwd} 目录下运行 hai 继续对话`);
        toast(`历史属于 ${entry.cwd}，请切换到该目录后运行 hai`);
      }
    });

    el.querySelector(".hai-hist-delete")!.addEventListener("click", async () => {
      if (confirm(`确定删除 ${entry.cwd} 的历史？`)) {
        try {
          await api.agentDeleteHistory(entry.cwd);
          el.remove();
        } catch (e: any) {
          toast(`删除失败: ${e?.message || e}`);
        }
      }
    });

    const migrateBtn = el.querySelector(".hai-hist-migrate");
    if (migrateBtn) {
      migrateBtn.addEventListener("click", async () => {
        const newCwd = prompt(`迁移 ${entry.cwd} 的历史到新目录：`, entry.cwd);
        if (newCwd && newCwd !== entry.cwd) {
          try {
            await api.agentMigrateHistory(entry.cwd, newCwd);
            toast("迁移成功");
            void this.loadHistoryList();
          } catch (e: any) {
            toast(`迁移失败: ${e?.message || e}`);
          }
        }
      });
    }

    return el;
  }

  private onHistoryRestored(messages: HistoryEntry[]): void {
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
    const labels: Record<string, string> = { auto: "🌓", light: "☀", dark: "🌙" };
    this.themeBtn.textContent = labels[this.themeMode] ?? "🌓";
  }

  /** 懒加载渲染依赖（在 show 前调用）。 */
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
