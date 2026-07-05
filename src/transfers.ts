/**
 * 传输面板：右下角浮动的上传/下载列表。
 *
 * 每行展示 名称 / 进度条 / 已传·总大小·百分比 / 实时速度，右侧按状态给出动作：
 *   - 进行中(active) / 已暂停(paused)：[暂停·继续] [取消]
 *   - 终态(done · failed · cancelled)：[删除]（点击即删，无提示）
 *
 * 面板标题栏的 × 仅**隐藏**面板，不影响任何传输；只要列表非空，标题栏（设置右侧）
 * 的下载图标会亮起，点击重新展开。列表清空后图标随之隐藏。
 *
 * 暂停/继续/取消是真实的 IO 控制，经 transfer_pause/resume/cancel 命令下达到后端；
 * 大小与速度则由前端从 transfer-progress 事件的字节增量本地计算。
 */

import { api } from "./ipc";
import type { TransferProgressEvent } from "./types";
import { formatSize } from "./ui";

type TState = "active" | "paused" | "done" | "failed" | "cancelled";

interface Row {
  id: string;
  name: string;
  direction: "upload" | "download";
  el: HTMLElement;
  state: TState;
  done: number;
  total: number;
  /** 速度计算：上一次采样的字节数与时间戳，speed 为 EMA 平滑后的 B/s */
  lastDone: number;
  lastTs: number;
  speed: number;
  /** 用户已请求取消：用于在传输 Promise reject 时区分「取消」与「失败」 */
  cancelling: boolean;
  detail?: string;
}

const ICON = {
  pause: `<svg viewBox="0 0 16 16" width="13" height="13"><path fill="currentColor" d="M4.5 3h2.2v10H4.5V3zm4.8 0h2.2v10H9.3V3z"/></svg>`,
  play: `<svg viewBox="0 0 16 16" width="13" height="13"><path fill="currentColor" d="M5 3.2l7 4.8-7 4.8V3.2z"/></svg>`,
  cancel: `<svg viewBox="0 0 16 16" width="13" height="13"><path stroke="currentColor" stroke-width="1.6" stroke-linecap="round" d="M4 4l8 8M12 4l-8 8"/></svg>`,
  trash: `<svg viewBox="0 0 16 16" width="13" height="13"><path fill="currentColor" d="M6 2h4l.6 1H14v1.4H2V3h3.4L6 2zm-2.4 3.4h8.8l-.7 8A1.3 1.3 0 0110.4 14.6H5.6a1.3 1.3 0 01-1.3-1.2l-.7-8zM6.4 7v5.2h1.1V7H6.4zm2.1 0v5.2h1.1V7H8.5z"/></svg>`,
  close: `<svg viewBox="0 0 12 12" width="12" height="12"><path stroke="currentColor" stroke-width="1.3" stroke-linecap="round" d="M2.5 2.5l7 7M9.5 2.5l-7 7"/></svg>`,
};

let panel: HTMLElement | null = null;
let listEl: HTMLElement | null = null;
let reopenBtn: HTMLElement | null = null;
/** 用户是否已主动关闭面板：为真时新传输不自动弹出，只点亮标题栏图标 */
let userHidden = false;
const rows = new Map<string, Row>();

/** 装配：绑定标题栏「重新打开」图标（设置图标右侧的 #btn-transfers） */
export function initTransfers(btn: HTMLElement): void {
  reopenBtn = btn;
  btn.addEventListener("click", () => showPanel());
  refreshVisibility();
}

/** 确保面板 DOM 存在（不改变可见性），返回列表容器 */
function ensurePanel(): HTMLElement {
  if (!panel) {
    panel = document.createElement("div");
    panel.className = "transfer-panel";
    panel.innerHTML = `
      <div class="tp-head">
        <span class="tp-title">传输</span>
        <button class="tp-close" title="隐藏（可从顶部下载图标重新打开）">${ICON.close}</button>
      </div>
      <div class="tp-list"></div>`;
    panel.querySelector(".tp-close")!.addEventListener("click", () => hidePanel());
    document.body.appendChild(panel);
    listEl = panel.querySelector(".tp-list");
  }
  return listEl!;
}

/** 从标题栏图标重新展开 */
function showPanel(): void {
  userHidden = false;
  ensurePanel();
  refreshVisibility();
}

/** 仅隐藏面板，不触碰任何传输；列表非空时点亮标题栏图标 */
function hidePanel(): void {
  userHidden = true;
  refreshVisibility();
}

/**
 * 同步面板与标题栏图标的可见性：
 * 列表空 → 移除面板、隐藏图标；
 * 非空 → 面板按 userHidden 显隐，隐藏时点亮标题栏图标。
 */
function refreshVisibility(): void {
  const has = rows.size > 0;
  if (!has) {
    if (panel) {
      panel.remove();
      panel = null;
      listEl = null;
    }
    userHidden = false;
    reopenBtn?.classList.add("hidden");
    return;
  }
  if (panel) panel.style.display = userHidden ? "none" : "flex";
  reopenBtn?.classList.toggle("hidden", !userHidden);
}

/** 开始一次传输：建立行并显示面板 */
export function beginTransfer(id: string, name: string, direction: "upload" | "download"): void {
  if (rows.has(id)) return;
  const list = ensurePanel();
  const el = document.createElement("div");
  el.className = "transfer-row";
  el.dataset.state = "active";
  el.innerHTML = `
    <div class="t-top">
      <span class="t-name"></span>
      <span class="t-actions">
        <button class="t-act t-pause" title="暂停">${ICON.pause}</button>
        <button class="t-act t-cancel" title="取消">${ICON.cancel}</button>
        <button class="t-act t-del" title="删除">${ICON.trash}</button>
      </span>
    </div>
    <div class="t-bar"><div class="t-fill"></div></div>
    <div class="t-meta"><span class="t-prog"></span><span class="t-speed"></span></div>`;
  const row: Row = {
    id, name, direction, el, state: "active",
    done: 0, total: 0, lastDone: 0, lastTs: Date.now(), speed: 0, cancelling: false,
  };
  el.querySelector(".t-name")!.textContent = `${direction === "upload" ? "↑" : "↓"} ${name}`;
  (el.querySelector(".t-name") as HTMLElement).title = name;
  el.querySelector(".t-pause")!.addEventListener("click", () => togglePause(row));
  el.querySelector(".t-cancel")!.addEventListener("click", () => requestCancel(row));
  el.querySelector(".t-del")!.addEventListener("click", () => removeRow(row));
  rows.set(id, row);
  list.appendChild(el);
  render(row);
  refreshVisibility();
}

/** 进度事件：更新字节与速度（EMA 平滑，采样间隔 ≥200ms 才刷新速度） */
export function updateTransfer(e: TransferProgressEvent): void {
  let row = rows.get(e.id);
  if (!row) {
    // 防御：极少数情况下进度先于 beginTransfer 到达
    beginTransfer(e.id, e.name, e.direction);
    row = rows.get(e.id)!;
  }
  row.done = e.done;
  row.total = e.total;
  if (row.state === "active") {
    const now = Date.now();
    const dt = (now - row.lastTs) / 1000;
    if (dt >= 0.2) {
      const inst = Math.max(0, (row.done - row.lastDone) / dt);
      row.speed = row.speed === 0 ? inst : row.speed * 0.6 + inst * 0.4;
      row.lastDone = row.done;
      row.lastTs = now;
    }
  }
  render(row);
}

/** 传输成功（由调用方在命令 resolve 后调用） */
export function completeTransfer(id: string): void {
  const row = rows.get(id);
  if (!row) return;
  setState(row, "done");
}

/** 传输结束于异常：已请求取消 → cancelled，否则 → failed */
export function failTransfer(id: string, detail?: string): void {
  const row = rows.get(id);
  if (!row) return;
  row.detail = detail;
  setState(row, row.cancelling ? "cancelled" : "failed");
}

function togglePause(row: Row): void {
  if (row.state === "active") {
    void api.transferPause(row.id);
    setState(row, "paused");
  } else if (row.state === "paused") {
    void api.transferResume(row.id);
    row.lastTs = Date.now();
    row.lastDone = row.done;
    setState(row, "active");
  }
}

function requestCancel(row: Row): void {
  if (row.state !== "active" && row.state !== "paused") return;
  row.cancelling = true;
  void api.transferCancel(row.id);
  // 终态由传输 Promise 的 reject 经 failTransfer 落定，这里只反馈「取消中」
  row.el.querySelector(".t-speed")!.textContent = "取消中…";
}

function removeRow(row: Row): void {
  row.el.remove();
  rows.delete(row.id);
  refreshVisibility();
}

function setState(row: Row, state: TState): void {
  row.state = state;
  row.el.dataset.state = state;
  render(row);
}

function render(row: Row): void {
  const pct = row.total > 0 ? Math.min(100, Math.round((row.done / row.total) * 100)) : row.state === "active" || row.state === "paused" ? 0 : 100;
  (row.el.querySelector(".t-fill") as HTMLElement).style.width = `${pct}%`;
  const prog = row.total > 0
    ? `${formatSize(row.done)} / ${formatSize(row.total)} · ${pct}%`
    : formatSize(row.done);
  row.el.querySelector(".t-prog")!.textContent = prog;
  // 暂停按钮图标随状态切换：进行中显示「暂停」，已暂停显示「继续」
  const pauseBtn = row.el.querySelector(".t-pause") as HTMLElement;
  if (row.state === "paused") {
    pauseBtn.innerHTML = ICON.play;
    pauseBtn.title = "继续";
  } else {
    pauseBtn.innerHTML = ICON.pause;
    pauseBtn.title = "暂停";
  }
  const speedEl = row.el.querySelector(".t-speed") as HTMLElement;
  switch (row.state) {
    case "active":
      speedEl.textContent = row.speed > 0 ? `${formatSize(Math.round(row.speed))}/s` : "…";
      break;
    case "paused":
      speedEl.textContent = "已暂停";
      break;
    case "done":
      speedEl.textContent = "已完成";
      break;
    case "failed":
      speedEl.textContent = "失败";
      speedEl.title = row.detail ?? "";
      break;
    case "cancelled":
      speedEl.textContent = "已取消";
      break;
  }
}
