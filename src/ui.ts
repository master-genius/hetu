/** 通用 UI 组件：上下文菜单、确认框、tooltip、文件预览、传输进度 */

import type { FileMeta, Preview, TransferProgressEvent } from "./types";

// ---------- 上下文菜单 ----------

export interface MenuItem {
  label: string;
  action?: () => void;
  disabled?: boolean;
  separator?: boolean;
  danger?: boolean;
}

let menuEl: HTMLElement | null = null;

export function showMenu(x: number, y: number, items: MenuItem[]) {
  hideMenu();
  menuEl = document.createElement("div");
  menuEl.className = "ctx-menu";
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
      <div class="modal small">
        <h3></h3>
        <p class="modal-msg"></p>
        <div class="modal-actions">
          <button class="btn" data-act="cancel">取消</button>
          <button class="btn primary" data-act="ok">确定</button>
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

const IMAGE_MIME: Record<string, string> = {
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

// ---------- 传输进度 ----------

let transferBox: HTMLElement | null = null;
const transferRows = new Map<string, HTMLElement>();

export function updateTransfer(e: TransferProgressEvent) {
  if (!transferBox) {
    transferBox = document.createElement("div");
    transferBox.className = "transfer-box";
    document.body.appendChild(transferBox);
  }
  let row = transferRows.get(e.id);
  if (!row) {
    row = document.createElement("div");
    row.className = "transfer-row";
    row.innerHTML = `<span class="t-name"></span><div class="t-bar"><div class="t-fill"></div></div><span class="t-pct"></span>`;
    transferBox.appendChild(row);
    transferRows.set(e.id, row);
  }
  const pct = e.total > 0 ? Math.min(100, Math.round((e.done / e.total) * 100)) : 100;
  row.querySelector(".t-name")!.textContent = `${e.direction === "upload" ? "↑" : "↓"} ${e.name}`;
  (row.querySelector(".t-fill") as HTMLElement).style.width = `${pct}%`;
  row.querySelector(".t-pct")!.textContent = `${pct}%`;
  if (e.done >= e.total && e.total > 0) {
    const r = row;
    window.setTimeout(() => {
      r.remove();
      transferRows.delete(e.id);
      if (transferBox && transferRows.size === 0) {
        transferBox.remove();
        transferBox = null;
      }
    }, 1500);
  }
}
