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
   * 沿 root → target 路径收集 split 节点，取第一个方向匹配的（最外层 = 最左/最上分割线）：
   * - row split + resizeLeft/Right → 调整垂直分割线
   * - col split + resizeUp/Down → 调整水平分割线
   * 方向语义固定：Left/Up → ratio 减小（a 侧变窄）；Right/Down → ratio 增大（a 侧变宽）。
   * 找不到（无对应方向分割线）则静默忽略。
   */
  adjustDivider(target: Pane, dir: "left" | "right" | "up" | "down", step = 0.015): void {
    const wantRow = dir === "left" || dir === "right";
    const increase = dir === "right" || dir === "down";

    // 沿 root → target 路径收集 split 节点，取第一个方向匹配的（最外层优先）
    const path: LayoutNode[] = [];
    const trace = (n: LayoutNode): boolean => {
      if (n.type === "leaf") return n.pane === target;
      path.push(n);
      const found = trace(n.a) || trace(n.b);
      if (!found) path.pop();
      return found;
    };
    trace(this.root);

    const splitNode = path.find((n) => n.type === "split" && n.dir === (wantRow ? "row" : "col"));
    if (!splitNode || splitNode.type !== "split") return;

    splitNode.ratio = Math.min(0.9, Math.max(0.1, splitNode.ratio + (increase ? step : -step)));
    this.render();
    this.onChange?.();
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
      const isCol = node.dir === "col";
      const containerRect = this.container.getBoundingClientRect();

      const guide = document.createElement("div");
      guide.className = `snap-guide ${isCol ? "snap-guide-h" : "snap-guide-v"}`;
      this.container.appendChild(guide);

      // 拖拽期间 DOM 结构不变，一次查询即可
      const peers = Array.from(this.container.querySelectorAll(".split-divider"))
        .filter((d): d is HTMLElement => d !== divider)
        .filter((d) => {
          const p = d.parentElement;
          return p && (isCol ? p.classList.contains("split-col") : p.classList.contains("split-row"));
        });

      const SNAP_PX = 6;

      const move = (e: MouseEvent) => {
        let frac = isCol
          ? (e.clientY - rect.top) / rect.height
          : (e.clientX - rect.left) / rect.width;

        // 查找最近的对齐目标（同方向 + 垂直范围重叠）
        const curPx = isCol ? e.clientY : e.clientX;
        let snapPx: number | null = null;
        let bestDist = SNAP_PX;

        for (const d of peers) {
          const dRect = d.getBoundingClientRect();
          if (isCol) {
            if (dRect.right < rect.left || dRect.left > rect.right) continue;
          } else {
            if (dRect.bottom < rect.top || dRect.top > rect.bottom) continue;
          }
          const dCenter = isCol ? dRect.top + dRect.height / 2 : dRect.left + dRect.width / 2;
          const dist = Math.abs(dCenter - curPx);
          if (dist <= bestDist) {
            bestDist = dist;
            snapPx = dCenter;
          }
        }

        if (snapPx !== null) {
          const snapFrac = isCol
            ? (snapPx - rect.top) / rect.height
            : (snapPx - rect.left) / rect.width;
          if (snapFrac >= 0.1 && snapFrac <= 0.9) {
            frac = snapFrac;
            if (isCol) guide.style.top = `${snapPx - containerRect.top}px`;
            else guide.style.left = `${snapPx - containerRect.left}px`;
            guide.classList.add("snap-guide-on");
          } else {
            guide.classList.remove("snap-guide-on");
          }
        } else {
          guide.classList.remove("snap-guide-on");
        }

        frac = Math.min(0.9, Math.max(0.1, frac));
        node.ratio = frac;
        const cells = box.querySelectorAll(":scope > .split-cell");
        (cells[0] as HTMLElement).style.flex = `${frac} 1 0`;
        (cells[1] as HTMLElement).style.flex = `${1 - frac} 1 0`;
        // 不显式 refit：仅更新 flex（廉价），由各 pane 的 ResizeObserver 异步触发 refit。
        // 浏览器每帧批量回调，只通知尺寸真正变化的 pane（含嵌套子分屏），避免全量同步 reflow。
      };
      const up = () => {
        window.removeEventListener("mousemove", move);
        window.removeEventListener("mouseup", up);
        guide.remove();
        this.onChange?.();
      };
      window.addEventListener("mousemove", move);
      window.addEventListener("mouseup", up);
    });
  }
}
