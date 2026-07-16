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
    // 如果有 pending 的 rAF，等它执行完
    if (this.pending) {
      requestAnimationFrame(() => this.finalize());
    } else {
      this.finalize();
    }
  }

  private finalize(): void {
    this.pending = false;
    this.rerender();
    // 最后一块也冻结
    this.highlightIfCode(this.activeEl);
    this.frozenCount++;
    this.activeEl = document.createElement("div");
    this.container.appendChild(this.activeEl);
    this.buffer = "";
  }

  private rerender(): void {
    if (!this.buffer) return;
    // marked 是静态 import，全局可用
    const html = (window as any).__marked_parse?.(this.buffer) ?? this.buffer;
    const temp = document.createElement("div");
    temp.innerHTML = html;
    const total = temp.children.length;
    if (total === 0) return;

    // 新完成的块：freeze
    const newFrozen = total - 1;
    while (this.frozenCount < newFrozen) {
      const el = temp.children[this.frozenCount].cloneNode(true) as HTMLElement;
      this.container.insertBefore(el, this.activeEl);
      this.highlightIfCode(el);
      this.frozenCount++;
    }
    // 防御：块数减少（marked 版本变化等）
    while (this.frozenCount > newFrozen) {
      const el = this.activeEl.previousElementSibling;
      if (el) this.container.removeChild(el);
      this.frozenCount--;
    }

    // 更新活跃块
    const lastChild = temp.children[total - 1];
    this.activeEl.innerHTML = lastChild.innerHTML;
    this.activeEl.className = lastChild.className;
  }

  private highlightIfCode(el: HTMLElement): void {
    el.querySelectorAll("pre code:not([data-hl])").forEach((code) => {
      const el = code as HTMLElement;
      // highlight.js 动态加载后可用
      const hljs = (window as any).__hljs;
      if (hljs) {
        hljs.highlightElement(el);
        el.dataset.hl = "done";
      }
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
