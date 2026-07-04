/** HetuShell 前端入口：装配标签页、事件路由、上传下载、快捷键 */

import "@xterm/xterm/css/xterm.css";
import "./fonts.css";
import "./styles.css";

import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, events } from "./ipc";
import { loadSettings, getSettings, onSettingsChange, activeTheme, fontStack } from "./settings";
import { TabManager, type Tab } from "./tabs";
import { Pane } from "./pane";
import { DND_MIME, Explorer } from "./explorer";
import { type Action, matchAction, resolveBindings } from "./keybindings";
import { showConnectDialog, showSettingsDialog } from "./dialogs";
import {
  confirmDialog, confirmOverwriteDialog, formatSize, showFileTooltip, showMenu, showPreview,
  toast, updateTransfer,
} from "./ui";
import type { ConnParams } from "./types";

const PREVIEW_MAX_BYTES = 512 * 1024;

async function bootstrap() {
  await loadSettings();

  // 等内置字体就绪再创建终端，避免 xterm 用回退字体测量出错误的单元格宽度
  await Promise.allSettled([
    document.fonts.load('300 14px "JetBrains Mono NL"'),
    document.fonts.load('400 14px "JetBrains Mono NL"'),
    document.fonts.load('300 14px "Noto Sans SC"'),
  ]);

  const tabBar = document.getElementById("tab-bar")!;
  const content = document.getElementById("content")!;
  const tabs = new TabManager(tabBar, content);

  // ---------- 连接与标签页 ----------

  // 连接元信息（供「标识连接」浮层与会话恢复用），键为 connId。不含任何机密。
  // profileId 记录连接来源的连接项；临时/手输连接为 null，不参与会话恢复。
  interface ConnInfo {
    local: boolean;
    name: string;
    host: string;
    profileId: string | null;
  }
  const connMeta = new Map<string, ConnInfo>();
  connMeta.set("local", { local: true, name: "本地终端", host: "local", profileId: null });

  const recordConn = (connId: string, params: ConnParams, profileId: string | null) => {
    connMeta.set(connId, {
      local: false,
      name: params.name,
      host: params.host,
      profileId,
    });
  };
  // 连接被回收（断开）时同步清除其元信息，避免 connMeta 随连接次数无界增长
  tabs.onConnClosed = (connId) => {
    connMeta.delete(connId);
  };

  const connectAndOpenTab = async (params: ConnParams, profileId: string | null = null) => {
    const connId = await api.sshConnect(params);
    recordConn(connId, params, profileId);
    await tabs.createTab(connId, params);
  };

  /** 本地终端：无 SSH 连接，connId 固定为 "local" */
  const LOCAL_PARAMS: ConnParams = {
    name: "本地终端",
    host: "local",
    port: 0,
    user: "",
    auth: "key",
  };
  const openLocalTab = async () => {
    await tabs.createTab("local", LOCAL_PARAMS);
  };

  /**
   * 在指定 pane 内打开/切换连接（右键菜单与顶部连接图标共用）：
   * 弹出连接选择，选 SSH 则建连后就地切换该 pane，选“本地终端”则切回本地。
   */
  const connectInPane = (tab: Tab, pane: Pane) => {
    showConnectDialog(
      async (params, profileId) => {
        const connId = await api.sshConnect(params);
        recordConn(connId, params, profileId ?? null);
        await tabs.switchPaneConnection(tab, pane, connId, params.name);
      },
      async () => {
        await tabs.switchPaneConnection(tab, pane, "local", "本地终端");
      },
    );
  };

  // “+” 行为由设置决定：默认直接开本地终端；连接图标始终弹连接选择
  tabs.onNewTabRequest = () => {
    if (getSettings().newTabMode === "dialog") {
      showConnectDialog(connectAndOpenTab, openLocalTab);
    } else {
      void openLocalTab();
    }
  };

  tabs.onTabContextMenu = (e, tab) => {
    showMenu(e.clientX, e.clientY, [
      {
        label: "关闭标签页",
        danger: true,
        action: () => void requestCloseTab(tab),
      },
    ]);
  };

  const requestCloseTab = async (tab: Tab) => {
    if (tabs.hasBusyPane(tab)) {
      const ok = await confirmDialog(
        "关闭标签页",
        `“${tab.title}”中可能有程序正在运行，确定要关闭吗？`,
      );
      if (!ok) return;
    }
    await tabs.closeTab(tab);
  };

  // ---------- Pane 事件装配 ----------

  tabs.onPaneCreated = (pane, tab) => {
    pane.onFocus = () => {
      tab.activePaneId = pane.id;
    };
    // 终端聚焦时的按键先经全局快捷键分发（命中则不透传给 shell）
    pane.onAppKey = (e) => dispatchShortcut(e, pane);
    pane.onTooltip = showFileTooltip;
    pane.onPreview = async (path) => {
      if (pane.isLocal) return; // 本地终端无 SFTP，双击不预览
      try {
        const meta = await api.sftpStat(pane.connId, path);
        if (meta.isDir) return; // 目录不预览
        if ((meta.size ?? 0) > 20 * 1024 * 1024) {
          toast(`文件过大（${formatSize(meta.size)}），请右键下载`, true);
          return;
        }
        const preview = await api.sftpPreview(pane.connId, path, PREVIEW_MAX_BYTES);
        showPreview(path, preview);
      } catch {
        /* 词不是文件路径，静默 */
      }
    };
    pane.onContextMenu = (e, word) => {
      const path = word ? pane.resolvePath(word) : null;
      const sel = pane.term.getSelection();
      // 本地终端没有 SFTP：预览/下载/上传不可用（避免调用 SSH-only 命令报误导性错误）
      const remote = !pane.isLocal;
      showMenu(e.clientX, e.clientY, [
        { label: "复制", disabled: !sel, action: () => void pane.copyText(sel) },
        { label: "粘贴", action: () => void pane.pasteFromClipboard() },
        { separator: true, label: "" },
        // 已连接 → 切换连接；本地终端 → 打开连接。均作用于本 pane。
        {
          label: remote ? "切换连接…" : "打开连接…",
          action: () => connectInPane(tab, pane),
        },
        { separator: true, label: "" },
        ...(remote
          ? [
              {
                label: word ? `预览 “${truncate(word)}”` : "预览",
                disabled: !path,
                action: () => path && pane.onPreview?.(path),
              },
              {
                label: word ? `下载 “${truncate(word)}”` : "下载",
                disabled: !path,
                action: () => path && void downloadFile(pane, path),
              },
              { label: "上传文件到当前目录", action: () => void uploadViaDialog() },
              { separator: true, label: "" },
            ]
          : []),
        { label: "向右切分 (Ctrl+Alt+R)", action: () => void tabs.splitActive("row", pane) },
        { label: "向下切分 (Ctrl+Alt+D)", action: () => void tabs.splitActive("col", pane) },
        {
          label: "关闭此分屏",
          danger: true,
          action: () => void requestClosePane(tab, pane),
        },
        { separator: true, label: "" },
        { label: "清屏", action: () => pane.term.clear() },
      ]);
    };
  };

  const requestClosePane = async (tab: Tab, pane: Pane) => {
    if (pane.busy) {
      const ok = await confirmDialog("关闭分屏", "该分屏中可能有程序正在运行，确定关闭吗？");
      if (!ok) return;
    }
    await tabs.closePane(tab, pane);
  };

  // ---------- 后端事件路由 ----------

  void events.onPaneOutput((e) => {
    tabs.findPane(e.paneId)?.pane.write(e.data);
  });

  void events.onPaneExit((e) => {
    const found = tabs.findPane(e.paneId);
    found?.pane.term.write(`\r\n\x1b[90m[进程已退出，状态码 ${e.status}]\x1b[0m\r\n`);
  });

  void events.onPaneClosed((e) => {
    const found = tabs.findPane(e.paneId);
    if (!found) return;
    // 正常退出（用户 exit）→ 收起该分屏；连接断开则保留等待重连
    if (e.exited) void tabs.closePane(found.tab, found.pane);
  });

  void events.onConnState((e) => {
    // 按 pane 自身 connId 定位受影响的分屏（而非 tab.connId）：多 pane 标签页里被就地
    // 切换连接的 pane 才能正确收到自己连接的重连事件，且不会误重开同标签页其它连接的 pane。
    const affected = tabs.panesByConn(e.connId);
    if (affected.length === 0) return;
    const affectedTabs = new Set(affected.map((a) => a.tab));
    switch (e.state) {
      case "reconnecting":
        for (const tab of affectedTabs) tabs.setBanner(tab, "连接已断开，正在重连…");
        break;
      case "waiting":
        for (const tab of affectedTabs)
          tabs.setBanner(tab, `重连失败（${e.error ?? "未知错误"}），${e.retryIn}s 后重试…`);
        break;
      case "connected":
        for (const tab of affectedTabs) tabs.setBanner(tab, null);
        for (const { pane } of affected) {
          pane.term.write("\r\n\x1b[32m[连接已恢复]\x1b[0m\r\n");
          void pane.open().catch(() => {});
        }
        break;
      case "closed":
        for (const tab of affectedTabs) tabs.setBanner(tab, "连接已关闭（自动重连已禁用）");
        break;
    }
  });

  void events.onTransferProgress(updateTransfer);

  // ---------- 上传：工具栏按钮 + 拖拽 ----------

  const uploadFiles = async (paths: string[], pane?: Pane | null, dirOverride?: string | null) => {
    const target = pane ?? tabs.activePane();
    if (!target) {
      toast("没有活动的连接", true);
      return;
    }
    if (target.isLocal) {
      toast("本地终端无需上传，直接在本机操作文件即可", true);
      return;
    }
    // dirOverride（拖到某目录名上）最精确；否则用 pane 的已知 cwd，再退到 home。
    let dir: string | null;
    let guessed = false;
    if (dirOverride) {
      dir = dirOverride;
    } else {
      const u = target.uploadDir();
      dir = u.dir;
      guessed = u.guessed;
    }
    if (!dir) {
      toast("尚未获取远端目录，请稍候重试", true);
      return;
    }
    // cwd 未由 OSC7 上报时，上传落到 home 目录——明确告知，避免用户以为传到了当前目录
    if (guessed) {
      const ok = await confirmDialog(
        "确认上传目录",
        `未能获取该终端的当前工作目录（shell 未上报 OSC7）。\n将上传到用户主目录：${dir}\n\n继续吗？`,
      );
      if (!ok) return;
    }
    // 同名文件处理：默认直接覆盖；开启「提示确认」后按项询问，可选「后续都如此」。
    const confirmOverwrite = getSettings().confirmOverwrite;
    let bulk: "overwrite" | "skip" | null = null; // 记住「全部」选择

    const basename = (p: string) => p.replace(/[/\\]+$/, "").split(/[/\\]/).pop() ?? p;

    for (const p of paths) {
      const name = basename(p);
      if (confirmOverwrite && bulk !== "overwrite") {
        // 探测远端是否已有同名项（stat 成功即存在）
        const exists = await api.sftpStat(target.connId, `${dir}/${name}`).then(() => true).catch(() => false);
        if (exists) {
          let decision: "overwrite" | "skip" | null = bulk;
          if (!decision) {
            const d = await confirmOverwriteDialog(name, dir);
            if (d.all) bulk = d.choice;
            decision = d.choice;
          }
          if (decision === "skip") {
            toast(`已跳过 ${name}`);
            continue;
          }
        }
      }
      try {
        await api.sftpUpload(target.connId, p, dir, crypto.randomUUID());
        toast(`已上传到 ${dir}`);
      } catch (err) {
        toast(`上传失败: ${err}`, true);
      }
    }
  };

  const uploadViaDialog = async () => {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({ multiple: true, title: "选择要上传的文件" });
    if (!picked) return;
    await uploadFiles(Array.isArray(picked) ? picked : [picked]);
  };

  // ---------- 拖放落点识别 ----------
  // 拖到终端空白/文件词上 → 上传到该 pane 的当前目录；
  // 拖到输出中的**目录名**上 → 该词高亮为选中态，上传到该目录。

  let dropTarget: { pane: Pane; dir: string | null } | null = null;
  let lastDetect = 0;
  let detectSeq = 0;

  const clearDropIndicators = () => {
    for (const tab of tabs.tabs) for (const p of tab.layout.panes()) p.clearDropHighlight();
    dropTarget = null;
  };

  const updateDropIndicator = (x: number, y: number) => {
    const el = document.elementFromPoint(x, y)?.closest(".pane") as HTMLElement | null;
    const found = el?.dataset.paneId ? tabs.findPane(el.dataset.paneId) : null;
    const pane = found?.pane ?? tabs.activePane();
    if (!pane || pane.isLocal) {
      clearDropIndicators();
      return;
    }
    if (dropTarget?.pane !== pane) dropTarget?.pane.clearDropHighlight();
    dropTarget = { pane, dir: dropTarget?.pane === pane ? dropTarget.dir : null };

    const now = Date.now();
    if (now - lastDetect < 120) return; // 节流：stat 是网络往返
    lastDetect = now;
    const seq = ++detectSeq;

    const range = pane.wordRangeAtPoint(x, y);
    if (!range) {
      pane.clearDropHighlight();
      dropTarget = { pane, dir: null };
      return;
    }
    const path = pane.resolvePath(range.word.replace(/\/$/, ""));
    if (!path) {
      pane.clearDropHighlight();
      dropTarget = { pane, dir: null };
      return;
    }
    void pane.statPath(path).then((meta) => {
      if (seq !== detectSeq) return; // 指针已移走，结果过期
      if (meta?.isDir) {
        pane.showDropHighlight(range);
        dropTarget = { pane, dir: path };
      } else {
        pane.clearDropHighlight();
        dropTarget = { pane, dir: null };
      }
    });
  };

  const performDrop = (paths: string[]) => {
    const t = dropTarget;
    clearDropIndicators();
    void uploadFiles(paths, t?.pane, t?.dir);
  };

  // OS 文件拖入（Tauri 原生事件，物理坐标 → 逻辑坐标）
  void getCurrentWebview().onDragDropEvent((event) => {
    const overlay = document.getElementById("drop-overlay")!;
    if (event.payload.type === "over") {
      overlay.style.display = "flex";
      const { x, y } = event.payload.position;
      updateDropIndicator(x / window.devicePixelRatio, y / window.devicePixelRatio);
    } else if (event.payload.type === "drop") {
      overlay.style.display = "none";
      const { x, y } = event.payload.position;
      updateDropIndicator(x / window.devicePixelRatio, y / window.devicePixelRatio);
      performDrop(event.payload.paths);
    } else {
      overlay.style.display = "none";
      clearDropIndicators();
    }
  });

  // 应用内拖拽（文件管理器条目 → 终端，HTML5 DnD）
  content.addEventListener("dragover", (e) => {
    if (!e.dataTransfer?.types.includes(DND_MIME)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "copy";
    updateDropIndicator(e.clientX, e.clientY);
  });
  content.addEventListener("drop", (e) => {
    const raw = e.dataTransfer?.getData(DND_MIME);
    if (!raw) return;
    e.preventDefault();
    try {
      performDrop(JSON.parse(raw) as string[]);
    } catch {
      clearDropIndicators();
    }
  });
  content.addEventListener("dragleave", (e) => {
    if (e.target === content) clearDropIndicators();
  });

  // ---------- 下载 ----------

  const downloadFile = async (pane: Pane, remotePath: string) => {
    try {
      const meta = await api.sftpStat(pane.connId, remotePath);
      const name = remotePath.replace(/\/+$/, "").split("/").pop() ?? remotePath;
      const dialog = await import("@tauri-apps/plugin-dialog");
      let local: string | null;
      if (meta.isDir) {
        // 目录：选择本地父目录，递归下载为其中的同名目录
        local = await dialog.open({ directory: true, title: `选择 “${name}” 的保存位置` }) as string | null;
      } else {
        local = await dialog.save({ defaultPath: name, title: "保存到本地" });
      }
      if (!local) return;
      await api.sftpDownload(pane.connId, remotePath, local, crypto.randomUUID());
      toast(`已下载到 ${meta.isDir ? `${local}/${name}` : local}`);
    } catch (err) {
      toast(`下载失败: ${err}`, true);
    }
  };

  // ---------- 工具栏 ----------

  // ---------- 窗口控制（自定义标题栏） ----------

  const win = getCurrentWindow();
  document.getElementById("btn-min")!.addEventListener("click", () => void win.minimize());
  document.getElementById("btn-max")!.addEventListener("click", () => void win.toggleMaximize());
  document.getElementById("btn-close")!.addEventListener("click", async () => {
    const busy = tabs.tabs.some((t) => tabs.hasBusyPane(t));
    const ok = await confirmDialog(
      "退出 HetuShell",
      busy ? "有标签页中可能有程序正在运行，确定退出吗？" : "确定要退出 HetuShell 吗？",
    );
    if (!ok) return;
    await win.close();
  });

  document.getElementById("btn-newtab")!.addEventListener("click", () => tabs.onNewTabRequest?.());

  // 顶部连接图标：默认针对当前聚焦的终端（就地打开/切换连接）；无终端时才新建标签页
  document.getElementById("btn-connect")!.addEventListener("click", () => {
    const tab = tabs.active;
    const pane = tabs.activePane();
    if (tab && pane) connectInPane(tab, pane);
    else showConnectDialog(connectAndOpenTab, openLocalTab);
  });

  // ---------- 本地文件管理器面板（右侧浮动 45%，每标签页独立实例） ----------

  const explorerPanel = document.getElementById("explorer-panel")!;
  const syncExplorerPanel = () => {
    const tab = tabs.active;
    // 用 .open 类做滑入/滑出动画（面板常驻 DOM，不用 display 切换以免动画失效）
    if (!tab || !tab.explorerOpen) {
      explorerPanel.classList.remove("open");
      return;
    }
    if (!tab.explorer) {
      tab.explorer = new Explorer();
      tab.explorer.onUploadRequest = (paths) => void uploadFiles(paths);
    }
    if (explorerPanel.firstChild !== tab.explorer.element) {
      explorerPanel.textContent = "";
      explorerPanel.appendChild(tab.explorer.element);
    }
    explorerPanel.classList.add("open");
    void tab.explorer.init();
  };
  // 会话快照：每个标签页取其首个 pane 的连接来源（连接项 id，不含机密），防抖写入 session.json。
  // 恢复期间 restoring=true 时跳过，避免在标签页尚未全部重建时把会话写成半截而丢失。
  let restoring = false;
  const snapshotSession = () => {
    // 未开启「记住最后的会话」时不写盘：既避免无谓写入，也不覆盖上次保存的会话
    if (restoring || !getSettings().restoreSession) return;
    const snap = tabs.tabs.map((tab) => {
      const first = tab.layout.panes()[0];
      const info = first ? connMeta.get(first.connId) : undefined;
      if (!info || info.local) return { local: true, name: info?.name ?? "本地终端" };
      return { local: false, name: info.name, profileId: info.profileId ?? null };
    });
    void api.sessionSet(snap).catch(() => {});
  };
  let saveTimer: number | undefined;
  const scheduleSaveSession = () => {
    window.clearTimeout(saveTimer);
    saveTimer = window.setTimeout(snapshotSession, 500);
  };

  tabs.onLayoutChange = () => {
    syncExplorerPanel();
    scheduleSaveSession();
  };

  // 标识连接：当前标签页每个终端中央浮层显示 连接名 + 地址，5 秒后淡出
  const identifyPanes = () => {
    const tab = tabs.active;
    if (!tab) return;
    for (const pane of tab.layout.panes()) {
      const meta =
        connMeta.get(pane.connId) ??
        (pane.isLocal ? { name: "本地终端", host: "local" } : { name: "连接", host: "" });
      pane.element.querySelector(".pane-identify")?.remove();
      const el = document.createElement("div");
      el.className = "pane-identify";
      el.innerHTML = `<b></b><span></span>`;
      el.querySelector("b")!.textContent = meta.name;
      el.querySelector("span")!.textContent = meta.host;
      pane.element.appendChild(el);
      requestAnimationFrame(() => el.classList.add("show"));
      window.setTimeout(() => {
        el.classList.remove("show");
        window.setTimeout(() => el.remove(), 300);
      }, 5000);
    }
  };
  document.getElementById("btn-identify")!.addEventListener("click", identifyPanes);

  document.getElementById("btn-explorer")!.addEventListener("click", () => {
    const tab = tabs.active;
    if (!tab) return;
    tab.explorerOpen = !tab.explorerOpen;
    syncExplorerPanel();
  });

  document.getElementById("btn-upload")!.addEventListener("click", () => void uploadViaDialog());
  document.getElementById("btn-split-h")!.addEventListener("click", () => void tabs.splitActive("row"));
  document.getElementById("btn-split-v")!.addEventListener("click", () => void tabs.splitActive("col"));
  document.getElementById("btn-settings")!.addEventListener("click", () => showSettingsDialog());

  // ---------- 设置热应用到所有终端 ----------

  onSettingsChange(() => {
    const s = getSettings();
    const colors = { ...activeTheme().colors, background: "#00000000" };
    for (const tab of tabs.tabs) {
      for (const pane of tab.layout.panes()) {
        pane.term.options.fontFamily = fontStack();
        pane.term.options.fontSize = s.fontSize;
        pane.term.options.fontWeight = s.fontWeight as never;
        pane.term.options.theme = colors as never;
        pane.refit();
      }
    }
  });

  // ---------- 分屏焦点导航（Alt+方向键）----------

  /** 按几何方位把焦点移到最近的相邻分屏 */
  const focusNeighbor = (dir: "left" | "right" | "up" | "down") => {
    const tab = tabs.active;
    const cur = tabs.activePane();
    if (!tab || !cur) return;
    const panes = tab.layout.panes();
    if (panes.length < 2) return;
    const r = cur.element.getBoundingClientRect();
    const cx = r.left + r.width / 2;
    const cy = r.top + r.height / 2;
    let best: Pane | null = null;
    let bestScore = Infinity;
    for (const p of panes) {
      if (p === cur) continue;
      const pr = p.element.getBoundingClientRect();
      const dx = pr.left + pr.width / 2 - cx;
      const dy = pr.top + pr.height / 2 - cy;
      let valid = false;
      let primary = 0;
      let offset = 0;
      switch (dir) {
        case "left": valid = dx < -1; primary = -dx; offset = Math.abs(dy); break;
        case "right": valid = dx > 1; primary = dx; offset = Math.abs(dy); break;
        case "up": valid = dy < -1; primary = -dy; offset = Math.abs(dx); break;
        case "down": valid = dy > 1; primary = dy; offset = Math.abs(dx); break;
      }
      if (!valid) continue;
      // 方向距离为主，垂直/水平偏移为辅，选最贴合目标方位的分屏
      const score = primary + offset * 2;
      if (score < bestScore) {
        bestScore = score;
        best = p;
      }
    }
    if (best) {
      tab.activePaneId = best.id;
      best.focus();
    }
  };

  // ---------- 快捷键（统一分发）----------

  const runAction = (action: Action, pane?: Pane) => {
    const tab = tabs.active;
    const p = pane ?? tabs.activePane();
    switch (action) {
      case "newTab": tabs.onNewTabRequest?.(); break;
      case "splitRight": if (tab && p) void tabs.splitActive("row", p); break;
      case "splitDown": if (tab && p) void tabs.splitActive("col", p); break;
      case "closePane": if (tab && p) void requestClosePane(tab, p); break;
      case "cycleTab": case "nextTab": tabs.switchTabBy(1); break;
      case "prevTab": tabs.switchTabBy(-1); break;
      case "focusLeft": focusNeighbor("left"); break;
      case "focusRight": focusNeighbor("right"); break;
      case "focusUp": focusNeighbor("up"); break;
      case "focusDown": focusNeighbor("down"); break;
      case "copy": {
        const sel = p?.term.getSelection();
        if (p && sel) void p.copyText(sel);
        break;
      }
      case "paste": void p?.pasteFromClipboard(); break;
    }
  };

  /** 命中快捷键则执行并阻止默认；返回是否命中。终端聚焦时由 pane.onAppKey 调用，
   *  其它场合由 window 兜底。 */
  const dispatchShortcut = (e: KeyboardEvent, pane?: Pane): boolean => {
    // 有弹窗打开时不响应全局快捷键（避免在对话框后面误切分/新建/关闭）
    if (document.querySelector(".modal-overlay")) return false;
    const action = matchAction(e, resolveBindings(getSettings().keybindings));
    if (!action) return false;
    e.preventDefault();
    runAction(action, pane);
    return true;
  };

  window.addEventListener("keydown", (e) => {
    const el = document.activeElement as HTMLElement | null;
    // 终端聚焦：已由 pane.onAppKey 处理，避免重复触发
    if (el?.closest?.(".pane")) return;
    // 表单/弹窗聚焦：保留原生行为（如 Tab 在字段间移动），不触发全局快捷键
    if (el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.closest(".modal"))) return;
    dispatchShortcut(e);
  });

  // 启动：若开启「记住最后的会话」则恢复上次的标签页并自动连接；否则开一个本地终端。
  // 远程连接只恢复来自已保存连接项（含密钥）的会话；临时/手输、密码认证（未存密码）的一律跳过。
  const session = getSettings().restoreSession ? await api.sessionGet().catch(() => []) : [];
  if (session.length > 0) {
    restoring = true;
    const profiles = session.some((s) => !s.local && s.profileId)
      ? await api.profilesList().catch(() => [] as Awaited<ReturnType<typeof api.profilesList>>)
      : [];
    for (const st of session) {
      try {
        if (st.local) {
          await openLocalTab();
          continue;
        }
        if (!st.profileId) continue; // 临时/手输连接，未保存为连接项 → 不恢复
        const p = profiles.find((pr) => pr.id === st.profileId);
        if (!p) continue; // 连接项已删除
        // 密码认证不存密码、密钥认证缺密钥内容/路径 → 无法静默连接，跳过
        const hasKey = !!(p.keyData || p.keyPath);
        if (p.auth !== "key" || !hasKey) {
          toast(`「${p.name}」未保存凭据，跳过自动连接`);
          continue;
        }
        await connectAndOpenTab(
          {
            name: p.name,
            host: p.host,
            port: p.port,
            user: p.user,
            auth: p.auth,
            keyPath: p.keyPath ?? undefined,
            keyData: p.keyData ?? undefined,
            keepalive: p.keepalive ?? undefined,
            timeout: p.timeout ?? undefined,
          },
          p.id,
        );
      } catch (err) {
        toast(`自动连接「${st.name}」失败：${err}`, true);
      }
    }
    restoring = false;
  }
  if (tabs.tabs.length === 0) await openLocalTab(); // 无可恢复项时的兜底
  snapshotSession(); // 恢复完成后落一次盘，反映真实的当前会话
}

function truncate(s: string, n = 24): string {
  return s.length > n ? s.slice(0, n) + "…" : s;
}

void bootstrap();
