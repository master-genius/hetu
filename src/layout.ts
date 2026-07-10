/**
 * 分屏布局：二叉树结构，leaf = Pane，split = 横/竖二分 + 可拖拽比例。
 * 分屏复用当前连接（新 Pane 在同一 connId 上开 channel）。
 */

import { Pane } from "./pane";

export type SplitDir = "row" | "col";

export type LayoutNode =
  | { type: "leaf"; pane: Pane }
  | { type: "split"; dir: SplitDir; ratio: number; a: LayoutNode; b: LayoutNode };

export class Layout {
  root: LayoutNode;
  readonly container: HTMLElement;
  /** 结构/比例变化通知（拖动分割线结束时触发，供上层持久化会话） */
  onChange: (() => void) | null = null;

  constructor(firstPane: Pane) {
    this.root = { type: "leaf", pane: firstPane };
    this.container = document.createElement("div");
    this.container.className = "layout-root";
  }

  /** 某 pane 的嵌套层级（根 pane 为 1，每切分一次 +1） */
  depthOf(target: Pane): number {
    const walk = (n: LayoutNode, d: number): number | null => {
      if (n.type === "leaf") return n.pane === target ? d : null;
      return walk(n.a, d + 1) ?? walk(n.b, d + 1);
    };
    return walk(this.root, 1) ?? 1;
  }

  panes(): Pane[] {
    const out: Pane[] = [];
    const walk = (n: LayoutNode) => {
      if (n.type === "leaf") out.push(n.pane);
      else {
        walk(n.a);
        walk(n.b);
      }
    };
    walk(this.root);
    return out;
  }

  /** 将 target pane 分裂为两半，新 pane 放在后半。ratio 供会话恢复还原比例。 */
  split(target: Pane, dir: SplitDir, newPane: Pane, ratio = 0.5): void {
    const r = Math.min(0.9, Math.max(0.1, ratio));
    const replace = (n: LayoutNode): LayoutNode => {
      if (n.type === "leaf") {
        if (n.pane === target) {
          return {
            type: "split",
            dir,
            ratio: r,
            a: { type: "leaf", pane: target },
            b: { type: "leaf", pane: newPane },
          };
        }
        return n;
      }
      return { ...n, a: replace(n.a), b: replace(n.b) };
    };
    this.root = replace(this.root);
    this.render();
  }

  /** 关闭一个 pane，兄弟节点上提。若是最后一个 pane 返回 false（由调用方关标签页） */
  close(target: Pane): boolean {
    if (this.root.type === "leaf") return this.root.pane !== target ? true : false;
    const prune = (n: LayoutNode): LayoutNode => {
      if (n.type === "leaf") return n;
      if (n.a.type === "leaf" && n.a.pane === target) return prune(n.b);
      if (n.b.type === "leaf" && n.b.pane === target) return prune(n.a);
      return { ...n, a: prune(n.a), b: prune(n.b) };
    };
    this.root = prune(this.root);
    target.dispose();
    this.render();
    return true;
  }

  /**
   * 快捷键调整分割线比例。
   * 从 root 向下遍历到 target pane 的路径，找第一个方向匹配的 split 节点：
   * - row split + resizeLeft/Right → 调整水平分割线
   * - col split + resizeUp/Down → 调整垂直分割线
   * target 在 a 侧：resizeLeft/Up → ratio 减小；resizeRight/Down → ratio 增大
   * target 在 b 侧：resizeLeft/Up → ratio 增大；resizeRight/Down → ratio 减小
   * 找到则调整并 render，找不到（无对应方向分割线）则静默忽略。
   */
  adjustDivider(target: Pane, dir: "left" | "right" | "up" | "down", step = 0.05): void {
    const wantRow = dir === "left" || dir === "right";
    const increase = dir === "right" || dir === "down";

    const find = (n: LayoutNode): LayoutNode | null => {
      if (n.type === "leaf") return null;
      if (n.dir === (wantRow ? "row" : "col")) {
        const inA = this.contains(n.a, target);
        const inB = this.contains(n.b, target);
        if (inA || inB) return n;
      }
      return find(n.a) ?? find(n.b);
    };

    const splitNode = find(this.root);
    if (!splitNode || splitNode.type === "leaf") return;

    const inA = this.contains(splitNode.a, target);
    // target 在 a 侧：increase → ratio 增大（a 变大）；在 b 侧：increase → ratio 减小（b 变大）
    const delta = inA ? (increase ? step : -step) : (increase ? -step : step);
    splitNode.ratio = Math.min(0.9, Math.max(0.1, splitNode.ratio + delta));
    this.render();
    this.onChange?.();
  }

  private contains(node: LayoutNode, target: Pane): boolean {
    if (node.type === "leaf") return node.pane === target;
    return this.contains(node.a, target) || this.contains(node.b, target);
  }

  /** 重建 DOM。pane 元素被移动而非重建，xterm 状态保留。 */
  render(): void {
    this.container.textContent = "";
    this.container.appendChild(this.renderNode(this.root));
    // DOM 稳定后统一 refit
    requestAnimationFrame(() => this.panes().forEach((p) => p.refit()));
  }

  private renderNode(node: LayoutNode): HTMLElement {
    if (node.type === "leaf") return node.pane.element;

    const box = document.createElement("div");
    box.className = `split split-${node.dir}`;
    const first = document.createElement("div");
    first.className = "split-cell";
    first.style.flex = `${node.ratio} 1 0`;
    first.appendChild(this.renderNode(node.a));

    const divider = document.createElement("div");
    divider.className = "split-divider";
    this.attachDrag(divider, node, box);

    const second = document.createElement("div");
    second.className = "split-cell";
    second.style.flex = `${1 - node.ratio} 1 0`;
    second.appendChild(this.renderNode(node.b));

    box.append(first, divider, second);
    return box;
  }

  private attachDrag(divider: HTMLElement, node: Extract<LayoutNode, { type: "split" }>, box: HTMLElement) {
    divider.addEventListener("mousedown", (down) => {
      down.preventDefault();
      const rect = box.getBoundingClientRect();
      const move = (e: MouseEvent) => {
        const frac =
          node.dir === "row"
            ? (e.clientX - rect.left) / rect.width
            : (e.clientY - rect.top) / rect.height;
        node.ratio = Math.min(0.9, Math.max(0.1, frac));
        const cells = box.querySelectorAll(":scope > .split-cell");
        (cells[0] as HTMLElement).style.flex = `${node.ratio} 1 0`;
        (cells[1] as HTMLElement).style.flex = `${1 - node.ratio} 1 0`;
        this.panes().forEach((p) => p.refit());
      };
      const up = () => {
        window.removeEventListener("mousemove", move);
        window.removeEventListener("mouseup", up);
        this.onChange?.(); // 拖动结束：比例已定，通知上层持久化
      };
      window.addEventListener("mousemove", move);
      window.addEventListener("mouseup", up);
    });
  }
}
