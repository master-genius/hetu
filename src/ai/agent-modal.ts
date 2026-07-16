/** AgentModal — Hai Agent 前端面板。
 *
 * Modal 生命周期：
 * - hai 命令 → OSC 1733 → show()：若已有 session 则恢复 hidden Modal，否则新建。
 * - ESC → hide（display:none），session 保留。
 * - Tab 关闭 → destroy()，invoke agent_destroy。
 *
 * Phase 1a：消息区 + 输入框 + 流式 Markdown 渲染 + 中止。
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
  // 按需注册常用语言（禁止全量导入，体积 1MB+）
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

  private tabId: string;
  private role: string;
  private mode: string;

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
          <span class="hai-title">🤖 Hai Agent</span>
          <div class="hai-header-tools">
            <span class="hai-mode-badge">Auto</span>
            <button class="btn hai-btn-close" title="关闭 (ESC)">关闭</button>
          </div>
        </div>
        <div class="hai-messages"></div>
        <div class="hai-input-bar">
          <textarea class="hai-input" rows="1" placeholder="输入消息… (Enter 发送, Shift+Enter 换行)"></textarea>
          <button class="btn hai-btn-send" title="发送">发送</button>
          <button class="btn hai-btn-abort hidden" title="中止">⏸ 中止</button>
        </div>
        <div class="hai-status-bar">
          <span class="hai-status">就绪</span>
        </div>
      </div>`;

    this.messagesEl = this.overlay.querySelector(".hai-messages")!;
    this.inputEl = this.overlay.querySelector(".hai-input")!;
    this.sendBtn = this.overlay.querySelector(".hai-btn-send")!;
    this.abortBtn = this.overlay.querySelector(".hai-btn-abort")!;
    this.statusEl = this.overlay.querySelector(".hai-status")!;
    this.closeBtn = this.overlay.querySelector(".hai-btn-close")!;

    // ESC 关闭（capture phase，先于 xterm 拦截）
    this.onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
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

  /** 隐藏 Modal（ESC / 关闭按钮）。Session 保留。 */
  hide(): void {
    this.overlay.style.display = "none";
    window.removeEventListener("keydown", this.onKey, true);
    // 从 DOM 移除但保留引用，再次 show() 时重新挂载
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
