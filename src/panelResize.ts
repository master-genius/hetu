/**
 * 文件面板尺寸可调（左=远程锚左边，右=本地锚右边）：
 * - 顶边拖拽调「高度」：底边固定，范围 [30%, 100%] × 最大高度；
 * - 内侧边拖拽调「宽度」：外侧边固定，范围 [80%, 100%] × 最大宽度；
 *   （本地面板锚右→拖左边；远程面板锚左→拖右边。外侧边与底边始终贴合，不脱离边缘。）
 * 默认尺寸即各自的最大值。调整后按「占最大值的比例」持久化到 localStorage，
 * 故窗口缩放后仍保持相对大小、且不越界。
 */

type Side = "left" | "right"; // left=远程(锚左)，right=本地(锚右)

const H_MIN = 0.3; // 高度下限 = 30% × 最大高度
const W_MIN = 0.8; // 宽度下限 = 80% × 最大宽度
const W_MAX_FRAC = 0.45; // 最大宽度 = 容器宽的 45%（与 CSS 默认 width:45% 一致）
const MARGIN = 4; // 面板四周留白（与 CSS 的 top/right/left/bottom:4px 对应）

interface Ratios {
  h: number;
  w: number;
}

/** 读取持久化比例并夹到合法范围；无记录时回默认（最大值，即 1） */
function load(key: string): Ratios {
  try {
    const r = JSON.parse(localStorage.getItem(key) ?? "") as Partial<Ratios>;
    const clamp = (v: unknown, lo: number) =>
      typeof v === "number" && Number.isFinite(v) ? Math.min(1, Math.max(lo, v)) : 1;
    return { h: clamp(r.h, H_MIN), w: clamp(r.w, W_MIN) };
  } catch {
    return { h: 1, w: 1 };
  }
}

export function initPanelResize(panel: HTMLElement, side: Side): void {
  const key = `hetushell-panel-${side}`;
  const ratios = load(key);
  // 定位基准（#content-wrap，position:relative）；退化时用 body 兜底
  const container = (): HTMLElement => (panel.offsetParent as HTMLElement) ?? document.body;
  const maxH = () => Math.max(1, container().clientHeight - MARGIN * 2);
  const maxW = () => Math.max(1, container().clientWidth * W_MAX_FRAC);

  /** 按当前容器尺寸把比例落成像素：解除顶边锚定→由底边(4px)+height 定位，底边固定 */
  const apply = () => {
    panel.style.top = "auto";
    panel.style.height = `${ratios.h * maxH()}px`;
    panel.style.width = `${ratios.w * maxW()}px`;
  };
  apply();

  const hHandle = document.createElement("div"); // 顶边：调高度
  hHandle.className = "panel-resize-h";
  const wHandle = document.createElement("div"); // 内侧边：调宽度
  wHandle.className = "panel-resize-w";
  panel.append(hHandle, wHandle);

  const startDrag = (e: PointerEvent, axis: "h" | "w") => {
    e.preventDefault();
    const handle = e.currentTarget as HTMLElement;
    handle.setPointerCapture(e.pointerId);
    document.body.style.userSelect = "none";
    const rect = container().getBoundingClientRect();

    const move = (ev: PointerEvent) => {
      if (axis === "h") {
        const mh = maxH();
        const bottomY = rect.bottom - MARGIN; // 底边固定
        const h = Math.min(mh, Math.max(H_MIN * mh, bottomY - ev.clientY));
        ratios.h = h / mh;
        panel.style.height = `${h}px`;
      } else {
        const mw = maxW();
        // 外侧边固定：右锚面板量到右边缘，左锚面板量到左边缘
        const raw =
          side === "right" ? rect.right - MARGIN - ev.clientX : ev.clientX - (rect.left + MARGIN);
        const w = Math.min(mw, Math.max(W_MIN * mw, raw));
        ratios.w = w / mw;
        panel.style.width = `${w}px`;
      }
    };
    const up = () => {
      handle.releasePointerCapture?.(e.pointerId);
      document.body.style.userSelect = "";
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      localStorage.setItem(key, JSON.stringify(ratios));
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  hHandle.addEventListener("pointerdown", (e) => startDrag(e, "h"));
  wHandle.addEventListener("pointerdown", (e) => startDrag(e, "w"));

  // 窗口尺寸变化：按持久比例重算实际像素，保持相对大小且不越界
  window.addEventListener("resize", apply);
}
