/** StreamingMarkdown — 流式 Markdown 渲染器（冻结块方案）。
 *
 * 每次 push() 对完整 buffer 执行 marked.parse()，按 top-level 块级元素分割。
 * 除最后一块外全部冻结（clone 到正式 DOM，不再触碰），只有最后一块动态更新。
 *
 * 参见设计文档 14.3 节：流式渲染——冻结块方案。
 */

export class StreamingMarkdown {
  private container: HTMLElement;
  private activeEl: HTMLElement;
  private frozenCount = 0;
  private buffer = "";
  private pending = false;

  constructor(container: HTMLElement) {
    this.container = container;
    this.activeEl = document.createElement("div");
    this.container.appendChild(this.activeEl);
  }

  push(text: string): void {
    this.buffer += text;
    if (!this.pending) {
      this.pending = true;
      requestAnimationFrame(() => {
        this.pending = false;
        this.rerender();
      });
    }
  }

  done(): void {
    if (this.pending) {
      requestAnimationFrame(() => this.finalize());
    } else {
      this.finalize();
    }
  }

  private finalize(): void {
    this.pending = false;
    this.rerender();
    this.highlightIfCode(this.activeEl);
    this.addCopyButtons(this.activeEl);
    this.frozenCount++;
    this.activeEl = document.createElement("div");
    this.container.appendChild(this.activeEl);
    this.buffer = "";
  }

  private rerender(): void {
    if (!this.buffer) return;
    const parseFn = (window as any).__marked_parse;
    if (!parseFn) {
      this.activeEl.textContent = this.buffer;
      return;
    }
    let html: string;
    try {
      html = parseFn(this.buffer);
    } catch {
      this.activeEl.textContent = this.buffer;
      return;
    }
    const temp = document.createElement("div");
    temp.innerHTML = html;
    const total = temp.children.length;
    if (total === 0) {
      this.activeEl.textContent = this.buffer;
      return;
    }

    const newFrozen = total - 1;
    while (this.frozenCount < newFrozen) {
      const el = temp.children[this.frozenCount].cloneNode(true) as HTMLElement;
      this.container.insertBefore(el, this.activeEl);
      this.highlightIfCode(el);
      this.addCopyButtons(el);
      this.frozenCount++;
    }
    while (this.frozenCount > newFrozen) {
      const el = this.activeEl.previousElementSibling;
      if (el) this.container.removeChild(el);
      this.frozenCount--;
    }

    const lastChild = temp.children[total - 1];
    this.activeEl.innerHTML = lastChild.innerHTML;
    this.activeEl.className = lastChild.className;
  }

  private highlightIfCode(el: HTMLElement): void {
    el.querySelectorAll("pre code:not([data-hl])").forEach((code) => {
      const el = code as HTMLElement;
      const hljs = (window as any).__hljs;
      if (hljs) {
        hljs.highlightElement(el);
        el.dataset.hl = "done";
      }
    });
  }

  private addCopyButtons(el: HTMLElement): void {
    el.querySelectorAll("pre").forEach((pre) => {
      if (pre.querySelector(".hai-code-copy")) return;
      const btn = document.createElement("button");
      btn.className = "hai-code-copy";
      btn.textContent = "Copy";
      btn.addEventListener("click", () => {
        const code = pre.querySelector("code");
        if (code) {
          navigator.clipboard.writeText(code.textContent || "").then(() => {
            btn.textContent = "Copied!";
            setTimeout(() => { btn.textContent = "Copy"; }, 2000);
          }).catch(() => {});
        }
      });
      pre.appendChild(btn);
    });
  }

  dispose(): void {
    this.container.innerHTML = "";
    this.frozenCount = 0;
    this.buffer = "";
    this.activeEl = document.createElement("div");
    this.container.appendChild(this.activeEl);
  }
}
