/** 通用 UI 组件：上下文菜单、确认框、tooltip、文件预览、自定义下拉 */

import type { FileMeta, Preview } from "./types";

// ---------- 自定义下拉（替代原生 select，避免 webkit2gtk 白底弹出列表不跟随主题）----------

export interface CustomSelect {
  el: HTMLElement;
  getValue(): string;
  setValue(v: string): void;
  /** 动态替换选项（如系统字体异步加载完成后注入），保留当前值 */
  setOptions(opts: CSOption[]): void;
}

/** 下拉选项：可选项或分隔线（用于「自带默认 —— 系统字体」分组） */
export type CSOption = { value: string; label: string } | { separator: true };

export function customSelect(
  options: CSOption[],
  initial: string,
  onChange?: (value: string) => void,
  menuMaxHeight?: number,
): CustomSelect {
  let value = initial;
  let opts = options;
  const el = document.createElement("button");
  el.type = "button";
  el.className = "cs";
  const labelSpan = document.createElement("span");
  labelSpan.className = "cs-label";
  const caret = document.createElement("span");
  caret.className = "cs-caret";
  caret.textContent = "▾";
  el.append(labelSpan, caret);

  const isOpt = (o: CSOption): o is { value: string; label: string } => !("separator" in o);
  const labelOf = (v: string) => {
    const found = opts.find((o): o is { value: string; label: string } => isOpt(o) && o.value === v);
    return found ? found.label : v;
  };
  const render = () => (labelSpan.textContent = labelOf(value));
  render();

  el.addEventListener("click", (e) => {
    e.stopPropagation();
    // 复用上下文菜单样式呈现选项列表，定位在下拉框正下方
    const rect = el.getBoundingClientRect();
    showMenu(
      rect.left,
      rect.bottom + 2,
      opts.map((o) =>
        isOpt(o)
          ? {
              label: (o.value === value ? "✓ " : "  ") + o.label,
              action: () => {
                value = o.value;
                render();
                onChange?.(value);
              },
            }
          : { separator: true, label: "" },
      ),
      rect.width,
      menuMaxHeight,
    );
  });

  return {
    el,
    getValue: () => value,
    setValue: (v: string) => {
      value = v;
      render();
    },
    setOptions: (next: CSOption[]) => {
      opts = next;
      render();
    },
  };
}

// ---------- 上下文菜单 ----------

export interface MenuItem {
  label: string;
  action?: () => void;
  disabled?: boolean;
  separator?: boolean;
  danger?: boolean;
}

let menuEl: HTMLElement | null = null;

export function showMenu(
  x: number,
  y: number,
  items: MenuItem[],
  minWidth?: number,
  maxHeight?: number,
) {
  hideMenu();
  menuEl = document.createElement("div");
  menuEl.className = "ctx-menu";
  if (minWidth) menuEl.style.minWidth = `${minWidth}px`;
  // 超长列表（如系统字体）限高并滚动，避免菜单溢出屏幕导致顶部项（内置默认）被推出可视区
  if (maxHeight) {
    menuEl.style.maxHeight = `${maxHeight}px`;
    menuEl.style.overflowY = "auto";
  }
  for (const item of items) {
    if (item.separator) {
      const sep = document.createElement("div");
      sep.className = "ctx-sep";
      menuEl.appendChild(sep);
      continue;
    }
    const btn = document.createElement("button");
    btn.className = "ctx-item" + (item.danger ? " danger" : "");
    btn.textContent = item.label;
    btn.disabled = !!item.disabled;
    btn.addEventListener("click", () => {
      hideMenu();
      item.action?.();
    });
    menuEl.appendChild(btn);
  }
  document.body.appendChild(menuEl);
  const rect = menuEl.getBoundingClientRect();
  menuEl.style.left = `${Math.min(x, window.innerWidth - rect.width - 8)}px`;
  menuEl.style.top = `${Math.min(y, window.innerHeight - rect.height - 8)}px`;
  window.setTimeout(() => {
    window.addEventListener("mousedown", onOutside, { once: true });
  }, 0);
}

function onOutside(e: MouseEvent) {
  if (menuEl && !menuEl.contains(e.target as Node)) hideMenu();
}

export function hideMenu() {
  menuEl?.remove();
  menuEl = null;
}

// ---------- 确认对话框 ----------

export function confirmDialog(title: string, message: string): Promise<boolean> {
  return new Promise((resolve) => {
    const overlay = document.createElement("div");
    overlay.className = "modal-overlay";
    overlay.innerHTML = `
      <div class="modal small confirm">
        <h3></h3>
        <p class="modal-msg"></p>
        <div class="modal-actions center">
          <button class="btn primary" data-act="ok">确定</button>
          <button class="btn" data-act="cancel">取消</button>
        </div>
      </div>`;
    overlay.querySelector("h3")!.textContent = title;
    overlay.querySelector(".modal-msg")!.textContent = message;
    const done = (v: boolean) => {
      overlay.remove();
      resolve(v);
    };
    overlay.querySelector('[data-act="ok"]')!.addEventListener("click", () => done(true));
    overlay.querySelector('[data-act="cancel"]')!.addEventListener("click", () => done(false));
    overlay.addEventListener("mousedown", (e) => {
      if (e.target === overlay) done(false);
    });
    document.body.appendChild(overlay);
    (overlay.querySelector('[data-act="ok"]') as HTMLElement).focus();
  });
}

/** 同名文件覆盖确认。返回本次选择；带 all 表示对后续同名文件沿用该选择。 */
export type OverwriteChoice = "overwrite" | "skip";
export interface OverwriteDecision {
  choice: OverwriteChoice;
  all: boolean;
}

export function confirmOverwriteDialog(name: string, dir: string): Promise<OverwriteDecision> {
  return new Promise((resolve) => {
    const overlay = document.createElement("div");
    overlay.className = "modal-overlay";
    overlay.innerHTML = `
      <div class="modal small">
        <h3>目标已存在同名文件</h3>
        <p class="modal-msg"></p>
        <label class="check overwrite-all"><input type="checkbox"> 对后续同名文件都如此</label>
        <div class="modal-actions">
          <button class="btn" data-act="skip">跳过</button>
          <button class="btn primary" data-act="overwrite">覆盖</button>
        </div>
      </div>`;
    overlay.querySelector(".modal-msg")!.textContent = `“${name}” 在 ${dir} 中已存在。`;
    const allInput = overlay.querySelector(".overwrite-all input") as HTMLInputElement;
    const done = (choice: OverwriteChoice) => {
      overlay.remove();
      resolve({ choice, all: allInput.checked });
    };
    overlay.querySelector('[data-act="overwrite"]')!.addEventListener("click", () => done("overwrite"));
    overlay.querySelector('[data-act="skip"]')!.addEventListener("click", () => done("skip"));
    // 背景点击 = 跳过（保守，不误覆盖）
    overlay.addEventListener("mousedown", (e) => {
      if (e.target === overlay) done("skip");
    });
    document.body.appendChild(overlay);
    (overlay.querySelector('[data-act="overwrite"]') as HTMLElement).focus();
  });
}

export function toast(message: string, isError = false) {
  const el = document.createElement("div");
  el.className = "toast" + (isError ? " error" : "");
  el.textContent = message;
  document.body.appendChild(el);
  window.setTimeout(() => el.classList.add("show"), 10);
  window.setTimeout(() => {
    el.classList.remove("show");
    window.setTimeout(() => el.remove(), 300);
  }, isError ? 5000 : 2500);
}

// ---------- 悬停元信息 tooltip ----------

let tipEl: HTMLElement | null = null;

export function formatSize(n: number | null): string {
  if (n == null) return "-";
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v >= 100 ? 0 : 1)} ${units[i]}`;
}

export function showFileTooltip(meta: FileMeta | null, x: number, y: number) {
  if (!meta) {
    tipEl?.remove();
    tipEl = null;
    return;
  }
  if (!tipEl) {
    tipEl = document.createElement("div");
    tipEl.className = "file-tooltip";
    document.body.appendChild(tipEl);
  }
  const kind = meta.isDir ? "目录" : meta.isLink ? "链接" : "文件";
  const mtime = meta.mtime ? new Date(meta.mtime * 1000).toLocaleString() : "-";
  const owner = meta.owner ?? (meta.uid != null ? String(meta.uid) : "-");
  const group = meta.group ?? (meta.gid != null ? String(meta.gid) : "-");
  tipEl.innerHTML = `
    <div class="tip-path"></div>
    <div class="tip-grid">
      <span>类型</span><b>${kind}</b>
      <span>大小</span><b>${meta.isDir ? "-" : formatSize(meta.size)}</b>
      <span>权限</span><b>${meta.perms}</b>
      <span>属主</span><b></b>
      <span>修改时间</span><b>${mtime}</b>
    </div>`;
  tipEl.querySelector(".tip-path")!.textContent = meta.path;
  tipEl.querySelectorAll(".tip-grid b")[3]!.textContent = `${owner}:${group}`;
  const rect = tipEl.getBoundingClientRect();
  tipEl.style.left = `${Math.min(x + 14, window.innerWidth - rect.width - 8)}px`;
  tipEl.style.top = `${Math.min(y + 18, window.innerHeight - rect.height - 8)}px`;
}

// ---------- 文件预览 ----------

export const IMAGE_MIME: Record<string, string> = {
  png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif",
  webp: "image/webp", bmp: "image/bmp", svg: "image/svg+xml", ico: "image/x-icon",
};

export function showPreview(path: string, preview: Preview) {
  const overlay = document.createElement("div");
  overlay.className = "modal-overlay";
  const modal = document.createElement("div");
  modal.className = "modal preview";
  const header = document.createElement("div");
  header.className = "preview-header";
  const title = document.createElement("span");
  title.className = "preview-title";
  title.textContent = path + (preview.truncated ? `（预览前 ${formatSize(atob(preview.data).length)} / 共 ${formatSize(preview.size)}）` : "");
  const closeBtn = document.createElement("button");
  closeBtn.className = "btn";
  closeBtn.textContent = "关闭";
  header.append(title, closeBtn);
  modal.appendChild(header);

  const body = document.createElement("div");
  body.className = "preview-body";
  if (preview.kind === "image") {
    const ext = path.split(".").pop()?.toLowerCase() ?? "png";
    const img = document.createElement("img");
    img.src = `data:${IMAGE_MIME[ext] ?? "image/png"};base64,${preview.data}`;
    body.appendChild(img);
  } else if (preview.kind === "binary") {
    body.innerHTML = `<p class="hint">二进制文件，无法预览文本内容。可右键选择下载。</p>`;
  } else {
    const pre = document.createElement("pre");
    pre.textContent = new TextDecoder("utf-8", { fatal: false }).decode(
      Uint8Array.from(atob(preview.data), (c) => c.charCodeAt(0)),
    );
    body.appendChild(pre);
  }
  modal.appendChild(body);
  overlay.appendChild(modal);
  const close = () => overlay.remove();
  closeBtn.addEventListener("click", close);
  overlay.addEventListener("mousedown", (e) => {
    if (e.target === overlay) close();
  });
  window.addEventListener("keydown", function esc(e) {
    if (e.key === "Escape") {
      close();
      window.removeEventListener("keydown", esc);
    }
  });
  document.body.appendChild(overlay);
}

/**
 * 图片查看器（文件面板右键「预览」）：滚轮/按钮缩放、左右旋转、拖拽平移，
 * 双击在「适应窗口 ⇄ 100%」间切换。src 由调用方异步提供（远端需先取回/命中缓存），
 * 弹窗先出「加载中」，避免慢网络下看似无响应。
 */
export function showImageViewer(name: string, load: Promise<string>): void {
  const overlay = document.createElement("div");
  overlay.className = "modal-overlay";
  overlay.innerHTML = `
    <div class="modal preview image-viewer">
      <div class="preview-header">
        <span class="preview-title"></span>
        <div class="iv-tools">
          <button class="btn" data-act="out" title="缩小">−</button>
          <span class="iv-pct">--</span>
          <button class="btn" data-act="in" title="放大">＋</button>
          <button class="btn" data-act="fit" title="适应窗口">适应</button>
          <button class="btn" data-act="bg" title="切换底色：主题 → 棋盘格 → 白 → 黑（查看透明图）">底色</button>
          <button class="btn" data-act="ccw" title="向左旋转 90°">↺</button>
          <button class="btn" data-act="cw" title="向右旋转 90°">↻</button>
          <button class="btn" data-act="close">关闭</button>
        </div>
      </div>
      <div class="preview-body"><p class="hint" style="padding:24px">加载中…</p></div>
    </div>`;
  overlay.querySelector(".preview-title")!.textContent = name;
  const body = overlay.querySelector(".preview-body") as HTMLElement;
  const pct = overlay.querySelector(".iv-pct") as HTMLElement;
  const on = (act: string, fn: () => void) =>
    overlay.querySelector(`[data-act="${act}"]`)!.addEventListener("click", fn);

  let img: HTMLImageElement | null = null;
  let scale = 1;
  let rot = 0; // 仅 90° 步进
  let tx = 0;
  let ty = 0;

  // 底色循环：透明图在主题底上可能看不清 → 棋盘格（专业软件惯例）/ 纯白 / 纯黑
  const BG_MODES = ["", "iv-bg-checker", "iv-bg-white", "iv-bg-black"] as const;
  let bgIdx = 0;
  const applyBg = () => {
    if (!img) return;
    for (const c of BG_MODES) if (c) img.classList.remove(c);
    const cls = BG_MODES[bgIdx];
    if (cls) img.classList.add(cls);
  };

  // translate 在最左（屏幕坐标系平移），旋转/缩放围绕图片中心，拖拽手感与方向无关
  const apply = () => {
    if (!img) return;
    img.style.transform = `translate(${tx}px, ${ty}px) rotate(${rot}deg) scale(${scale})`;
    pct.textContent = `${Math.round(scale * 100)}%`;
  };
  /** 适应窗口的缩放比（旋转 90/270° 时宽高互换）；小图不放大（上限 100%） */
  const fitScale = () => {
    if (!img || !img.naturalWidth || !img.naturalHeight) return 1;
    const swap = rot % 180 !== 0;
    const w = swap ? img.naturalHeight : img.naturalWidth;
    const h = swap ? img.naturalWidth : img.naturalHeight;
    return Math.min((body.clientWidth - 32) / w, (body.clientHeight - 32) / h, 1) || 1;
  };
  const fit = () => {
    scale = fitScale();
    tx = ty = 0;
    apply();
  };
  const zoom = (factor: number) => {
    scale = Math.min(20, Math.max(0.05, scale * factor));
    apply();
  };
  const rotate = (deg: number) => {
    rot = (rot + deg + 360) % 360;
    fit(); // 旋转后宽高互换，重新适应最不易「转丢」
  };

  const close = () => {
    overlay.remove();
    window.removeEventListener("keydown", onKey);
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") close();
  };
  window.addEventListener("keydown", onKey);
  overlay.addEventListener("mousedown", (e) => {
    if (e.target === overlay) close();
  });
  on("close", close);
  on("in", () => zoom(1.25));
  on("out", () => zoom(0.8));
  on("fit", fit);
  on("bg", () => {
    bgIdx = (bgIdx + 1) % BG_MODES.length;
    applyBg();
  });
  on("ccw", () => rotate(-90));
  on("cw", () => rotate(90));

  body.addEventListener(
    "wheel",
    (e) => {
      e.preventDefault();
      zoom(e.deltaY < 0 ? 1.12 : 1 / 1.12);
    },
    { passive: false },
  );
  body.addEventListener("mousedown", (e) => {
    if (!img || e.button !== 0) return;
    e.preventDefault();
    const sx = e.clientX - tx;
    const sy = e.clientY - ty;
    img.classList.add("panning");
    const move = (ev: MouseEvent) => {
      tx = ev.clientX - sx;
      ty = ev.clientY - sy;
      apply();
    };
    const up = () => {
      img?.classList.remove("panning");
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  });
  body.addEventListener("dblclick", () => {
    if (!img) return;
    const f = fitScale();
    scale = Math.abs(scale - f) < 0.001 ? 1 : f; // 适应 ⇄ 100%
    tx = ty = 0;
    apply();
  });

  const showHint = (text: string) => {
    const p = document.createElement("p");
    p.className = "hint";
    p.style.padding = "24px";
    p.textContent = text;
    body.replaceChildren(p);
  };
  load
    .then((src) => {
      const el = document.createElement("img");
      el.draggable = false; // 禁掉原生拖拽幽灵图，让位给平移
      el.onload = () => {
        img = el;
        body.replaceChildren(el);
        applyBg(); // 加载完成前用户可能已切过底色
        fit();
      };
      el.onerror = () => showHint("图片解码失败，格式可能不受支持");
      el.src = src;
    })
    .catch((err) => showHint(`加载失败: ${err}`));

  document.body.appendChild(overlay);
}

// 传输进度面板已迁出到 transfers.ts（列表化、含暂停/取消/删除与速度展示）。
