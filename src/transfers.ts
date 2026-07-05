/**
 * 传输面板：右下角浮动的上传/下载列表。
 *
 * 每行展示 名称 / 进度条 / 已传·总大小·百分比 / 实时速度，右侧按状态给出动作：
 *   - 进行中(active) / 已暂停(paused)：[暂停·继续] [取消]
 *   - 终态(done · failed · cancelled)：[删除]（点击即删，无提示）
 * 行名悬停显示完整目标路径（下载落地位置 / 上传目标目录）。
 *
 * 面板标题栏的 × 仅**隐藏**面板，不影响任何传输；只要列表非空，标题栏（设置右侧）
 * 的下载图标会亮起，点击重新展开。列表清空后图标随之隐藏。用户主动隐藏的偏好会保留，
 * 后续新传输只点亮图标、不再自动弹出。
 *
 * 暂停/继续/取消是真实的 IO 控制，经 transfer_pause/resume/cancel 命令下达到后端；
 * 大小与速度则由前端从 transfer-progress 事件的字节增量本地计算。
 */

import { api } from "./ipc";
import type { TransferProgressEvent } from "./types";
import { formatSize } from "./ui";

type TState = "active" | "paused" | "done" | "failed" | "cancelled";

/** 后端取消错误的序列化文案（error.rs Error::Cancelled），用于权威区分取消/失败 */
const CANCELLED_MARK = "传输已取消";

interface Row {
  id: string;
  el: HTMLElement;
  // 缓存子元素引用，避免每个进度事件都重新 querySelector（下载可达数千事件/秒）
  fill: HTMLElement;
  prog: HTMLElement;
  speed: HTMLElement;
  pauseBtn: HTMLElement;
  state: TState;
  done: number;
  total: number;
  /** 速度计算：上一次采样的字节数与时间戳，speedBps 为 EMA 平滑后的 B/s */
  lastDone: number;
  lastTs: number;
  speedBps: number;
  /** 用户已请求取消：用于渲染「取消中…」并作为取消意图记录 */
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
/** 用户是否已主动隐藏面板：为真时新传输不自动弹出，只点亮标题栏图标（偏好持久保留） */
let userHidden = false;
const rows = new Map<string, Row>();

const isTerminal = (s: TState): boolean => s === "done" || s === "failed" || s === "cancelled";

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
 * 列表空 → 移除面板、隐藏图标（保留 userHidden 偏好）；
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
    reopenBtn?.classList.add("hidden");
    return;
  }
  if (panel) panel.style.display = userHidden ? "none" : "flex";
  reopenBtn?.classList.toggle("hidden", !userHidden);
}

/**
 * 开始一次传输：建立行。
 * @param dest 完整目标路径（下载落地位置 / 上传目标目录），作为行名悬停提示
 */
export function beginTransfer(
  id: string,
  name: string,
  direction: "upload" | "download",
  dest?: string,
): void {
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
  const nameEl = el.querySelector(".t-name") as HTMLElement;
  nameEl.textContent = `${direction === "upload" ? "↑" : "↓"} ${name}`;
  nameEl.title = dest ?? name;
  const row: Row = {
    id, el, state: "active",
    fill: el.querySelector(".t-fill") as HTMLElement,
    prog: el.querySelector(".t-prog") as HTMLElement,
    speed: el.querySelector(".t-speed") as HTMLElement,
    pauseBtn: el.querySelector(".t-pause") as HTMLElement,
    done: 0, total: 0, lastDone: 0, lastTs: Date.now(), speedBps: 0, cancelling: false,
  };
  row.pauseBtn.addEventListener("click", () => togglePause(row));
  el.querySelector(".t-cancel")!.addEventListener("click", () => requestCancel(row));
  el.querySelector(".t-del")!.addEventListener("click", () => removeRow(row));
  rows.set(id, row);
  list.prepend(el); // 最新的传输在最前面
  render(row);
  refreshVisibility();
}

/**
 * 进度事件：更新字节与速度（EMA 平滑，采样间隔 ≥200ms 才刷新速度）。
 * 行不存在则忽略——beginTransfer 必在调用方同步先行，未知 id 只可能是已删除行的
 * 滞后事件，绝不重建（否则会复活永远停在 active 的僵尸行）。
 */
export function updateTransfer(e: TransferProgressEvent): void {
  const row = rows.get(e.id);
  if (!row) return;
  row.done = e.done;
  row.total = e.total;
  if (row.state === "active") {
    const now = Date.now();
    const dt = (now - row.lastTs) / 1000;
    if (dt >= 0.2) {
      const inst = Math.max(0, (row.done - row.lastDone) / dt);
      row.speedBps = row.speedBps === 0 ? inst : row.speedBps * 0.6 + inst * 0.4;
      row.lastDone = row.done;
      row.lastTs = now;
    }
  }
  render(row);
}

/** 传输成功（由调用方在命令 resolve 后调用） */
export function completeTransfer(id: string): void {
  const row = rows.get(id);
  if (row) setState(row, "done");
}

/**
 * 传输结束于异常：按后端权威错误判定——文案含取消标记 → cancelled，否则 → failed。
 * 不再仅凭前端 cancelling 标志，避免「刚点取消却真实失败」被误标为已取消而吞掉原因。
 */
export function failTransfer(id: string, detail?: string): void {
  const row = rows.get(id);
  if (!row) return;
  row.detail = detail;
  const cancelled = detail?.includes(CANCELLED_MARK) ?? false;
  setState(row, cancelled ? "cancelled" : "failed");
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
  // 作为状态渲染，之后任何进度事件的 render 都会持续显示「取消中…」，不会被速度覆盖
  render(row);
}

function removeRow(row: Row): void {
  row.el.remove();
  rows.delete(row.id);
  refreshVisibility();
}

function setState(row: Row, state: TState): void {
  row.state = state;
  row.el.dataset.state = state;
  // 暂停按钮图标仅随状态切换更新（不在每个进度事件里重写 innerHTML）
  if (state === "paused") {
    row.pauseBtn.innerHTML = ICON.play;
    row.pauseBtn.title = "继续";
  } else if (state === "active") {
    row.pauseBtn.innerHTML = ICON.pause;
    row.pauseBtn.title = "暂停";
  }
  render(row);
}

function render(row: Row): void {
  // total=0（空文件/空目录）按 100% 处理：字节维度即已完成
  const pct = row.total > 0 ? Math.min(100, Math.round((row.done / row.total) * 100)) : 100;
  row.fill.style.width = `${pct}%`;

  // 失败时用较宽的进度行展示错误原因（截断 + 悬停全文），比小字提示更醒目
  if (row.state === "failed") {
    row.prog.textContent = row.detail || "传输失败";
    row.prog.title = row.detail ?? "";
    row.speed.textContent = "失败";
    return;
  }
  row.prog.title = "";
  row.prog.textContent = row.total > 0
    ? `${formatSize(row.done)} / ${formatSize(row.total)} · ${pct}%`
    : formatSize(row.done);

  // 取消中优先于速度/状态文案，且作为渲染状态存在，不会被进度事件覆盖
  if (row.cancelling && !isTerminal(row.state)) {
    row.speed.textContent = "取消中…";
    return;
  }
  switch (row.state) {
    case "active":
      row.speed.textContent = row.speedBps > 0 ? `${formatSize(Math.round(row.speedBps))}/s` : "…";
      break;
    case "paused":
      row.speed.textContent = "已暂停";
      break;
    case "done":
      row.speed.textContent = "已完成";
      break;
    case "cancelled":
      row.speed.textContent = "已取消";
      break;
  }
}
