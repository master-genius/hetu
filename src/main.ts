/** HetuShell 前端入口：装配标签页、事件路由、上传下载、快捷键 */

import "@xterm/xterm/css/xterm.css";
import "./fonts.css";
import "./styles.css";

import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, events } from "./ipc";
import { loadSettings, getSettings, onSettingsChange, activeTheme, fontStack } from "./settings";
import { TabManager, type Tab } from "./tabs";
import { Pane, type HsshSpec } from "./pane";
import {
  DND_MIME,
  DL_MIME,
  Explorer,
  VIEWABLE_IMG,
  localBackend,
  type ExplorerBackend,
} from "./explorer";
import { type Action, matchAction, resolveBindings } from "./keybindings";
import { showAboutDialog, showConnectDialog, showSettingsDialog } from "./dialogs";
import { feedPane } from "./feed";
import { initPanelResize } from "./panelResize";
import {
  confirmDialog, confirmOverwriteDialog, formatSize, IMAGE_MIME, showFileTooltip, showHimageViewer, showMenu, showPreview,
  toast,
} from "./ui";
import {
  beginTransfer, completeTransfer, failTransfer, initTransfers, updateTransfer,
} from "./transfers";
import type { LayoutNode } from "./layout";
import type { ConnParams, Profile, SessionLayout } from "./types";

const PREVIEW_MAX_BYTES = 512 * 1024;

async function bootstrap() {
  await loadSettings();
  // 多实例 slot 分配：每个 hetushell 进程拿独立 slot，session 按 slot 分片持久化。
  // 失败不阻塞启动（退化为本进程不持久化，不影响其它实例）。
  await api.sessionAcquire().catch(() => {});

  // 等内置字体就绪再创建终端，避免 xterm 用回退字体测量出错误的单元格宽度。
  // 覆盖默认字体名（含「字重并入名字」别名），确保首帧用正确字体测量单元格。
  await Promise.allSettled([
    document.fonts.load('normal 14px "JetBrains Mono NL"'),
    document.fonts.load('bold 14px "JetBrains Mono NL"'),
    document.fonts.load('normal 14px "Noto Sans CJK SC"'),
    document.fonts.load('bold 14px "Noto Sans CJK SC"'),
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
    remoteExplorers.delete(connId);
    refreshPanels();
  };

  const connectAndOpenTab = async (
    params: ConnParams,
    profileId: string | null = null,
    order?: number,
  ): Promise<Tab> => {
    const connId = await api.sshConnect(params);
    recordConn(connId, params, profileId);
    return tabs.createTab(connId, params, order);
  };

  /** 本地终端：无 SSH 连接，connId 固定为 "local" */
  const LOCAL_PARAMS: ConnParams = {
    name: "本地终端",
    host: "local",
    port: 0,
    user: "",
    auth: "key",
  };
  const openLocalTab = async (order?: number, cwd?: string | null): Promise<Tab> =>
    tabs.createTab("local", LOCAL_PARAMS, order, cwd);

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
  // 连接对话框回调只关心成功与否，不需要 Tab 返回值 → 适配为 Promise<void>
  const connectAndOpenTabVoid = async (params: ConnParams, profileId?: string | null) => {
    await connectAndOpenTab(params, profileId ?? null);
  };
  const openLocalTabVoid = async () => {
    await openLocalTab();
  };

  /** 已保存连接项 → 连接参数（与手动连接的 paramsFromForm 等价；密码/口令从不入库，故不含）。 */
  const profileToParams = (p: Profile): ConnParams => ({
    name: p.name,
    host: p.host,
    port: p.port,
    user: p.user,
    auth: p.auth,
    keyPath: p.keyPath ?? undefined,
    keyData: p.keyData ?? undefined,
    keepalive: p.keepalive ?? undefined,
    timeout: p.timeout ?? undefined,
  });

  /**
   * 内建 hssh 命令入口（仅本地终端经 OSC 触发）。就地替换当前 pane 的连接（与右键
   * 「切换连接」/连接图标弹窗同路径，走 switchPaneConnection，不新建标签页）：
   * - profile：密钥认证直连；密码认证不存密码 → 弹预填窗要密码。
   * - adhoc：给了密码/密钥则直连；都没给 → 弹预填窗询问。
   * 稳定优先：任何异常仅 toast，不影响既有终端；pane 已关闭则降级提示不做任何操作。
   */
  const handleHssh = async (spec: HsshSpec, pane: Pane) => {
    try {
      const found = tabs.findPane(pane.id);
      if (!found) {
        toast("hssh：当前终端已关闭", true);
        return;
      }
      const { tab } = found;
      // 替换当前 pane 的连接（复用已验证的 switchPaneConnection 路径，不新建标签）
      const onConnect = async (params: ConnParams, profileId?: string | null) => {
        const connId = await api.sshConnect(params);
        recordConn(connId, params, profileId ?? null);
        try {
          await tabs.switchPaneConnection(tab, pane, connId, params.name);
        } catch (err) {
          // pane 已关闭或切换失败：回收刚建立的连接，避免泄漏
          void api.sshDisconnect(connId).catch(() => {});
          throw err;
        }
        // hssh --debug：进程退出时在终端输出状态码提示
        pane.debugExit = !!spec.debug;
        // 自动化喂入：连接成功后读取临时文件，喂入命令到新 pane
        if (spec.feedPath) {
          try {
            const content = await api.readFeedFile(spec.feedPath);
            void feedPane(pane, content, spec.exitAfter).catch((e) =>
              toast(`hssh：喂入命令失败: ${e}`, true),
            );
          } catch (err) {
            toast(`hssh：读取喂入文件失败: ${err}`, true);
          }
        }
      };
      const onLocal = async () => {
        await tabs.switchPaneConnection(tab, pane, "local", "本地终端");
      };

      if (spec.mode === "profile") {
        const profiles = await api.profilesList().catch(() => [] as Profile[]);
        const p = profiles.find((x) => x.name === spec.name);
        if (!p) {
          toast(`hssh：未找到连接项「${spec.name}」`, true);
          return;
        }
        if (p.auth === "password") {
          showConnectDialog(onConnect, onLocal, { kind: "profile", profile: p });
        } else {
          await onConnect(profileToParams(p), p.id);
        }
        return;
      }
      // adhoc（临时连接）
      const port = parseInt(spec.port, 10) || 22;
      const user = spec.user || "root";
      if (spec.password) {
        await onConnect(
          { name: spec.host, host: spec.host, port, user, auth: "password", password: spec.password },
          null,
        );
      } else if (spec.identity) {
        await onConnect(
          { name: spec.host, host: spec.host, port, user, auth: "key", keyPath: spec.identity },
          null,
        );
      } else {
        showConnectDialog(onConnect, onLocal, {
          kind: "adhoc",
          host: spec.host,
          user,
          port,
        });
      }
    } catch (err) {
      toast(String(err), true);
    }
  };

  tabs.onNewTabRequest = async () => {
    if (getSettings().newTabMode === "dialog") {
      showConnectDialog(connectAndOpenTabVoid, openLocalTabVoid);
    } else {
      // 直接新建本地终端：继承当前聚焦本地终端的实时 cwd（远程/取不到为 null → 后端回退 home）
      const cwd = (await tabs.activePane()?.resolveLocalCwd()) ?? null;
      void openLocalTab(undefined, cwd);
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

  // himage：在终端内弹出图片查看器（多图可切换、缩放/旋转/底色复用现有组件）
  const handleHimage = (paths: string[], withShell: boolean, pane: Pane) => {
    const items = paths.map((p) => {
      const ext = p.split(".").pop()?.toLowerCase() ?? "png";
      const mime = IMAGE_MIME[ext] ?? "image/png";
      const name = p.split("/").pop() ?? p;
      return {
        name,
        load: () => api.imagePreview("local", p).then((r) => `data:${mime};base64,${r.data}`),
      };
    });
    const anchor = withShell ? pane.element.getBoundingClientRect() : null;
    showHimageViewer(items, anchor);
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
      const switched = tab.activePaneId !== pane.id;
      tab.activePaneId = pane.id;
      if (switched && tab === tabs.active) {
        refreshPanels();
        // 多 pane 分屏：标签标题跟随活动 pane
        if (pane.isLocal) {
          // 本地 pane：异步取 `目录:进程`，轮询会持续刷新
          void api.localTabInfo(pane.id).then((info) => {
            if (info) tabs.updateActivePaneTitle(tab, `${tabs.dirLabel(info.cwd)}:${info.process}`);
          }).catch(() => {});
        } else {
          const info = connMeta.get(pane.connId);
          if (info) tabs.updateActivePaneTitle(tab, info.name);
        }
      }
    };
    // 终端聚焦时的按键先经全局快捷键分发（命中则不透传给 shell）
    pane.onAppKey = (e) => dispatchShortcut(e, pane);
    // 内建 hssh 命令（仅本地终端，Pane 内已按 isLocal 门控）
    pane.onHssh = (spec, p) => void handleHssh(spec, p);
    // 内建 hexit 命令：跳过确认，保存会话后直接 destroy（会话下次启动恢复）
    pane.onHexit = () => void performHexit();
    // 内建 himage 命令：弹出图片查看器
    pane.onHimage = (paths, withShell, p) => handleHimage(paths, withShell, p);
    pane.onTooltip = showFileTooltip;
    // Ctrl+单击文件/目录 → 下载（默认 Downloads / 每次询问，按设置）
    pane.onCtrlClick = (path) => void downloadFile(pane, path);
    // Ctrl+拖拽文件/目录 → 携带下载意图，拖到文件管理器则下载到其目录
    pane.onCtrlDragStart = (word, e) => {
      // 携带原始词 + 源 pane id：落点由 downloadFile 针对源 pane 异步解析为绝对路径，
      // 保证同一连接分屏复用时仍用发起终端的工作目录。
      e.dataTransfer?.setData(
        DL_MIME,
        JSON.stringify({ connId: pane.connId, paneId: pane.id, path: word }),
      );
      if (e.dataTransfer) e.dataTransfer.effectAllowed = "copy";
    };
    pane.onPreview = async (word) => {
      if (pane.isLocal) return; // 本地终端无 SFTP，双击不预览
      try {
        const path = await pane.resolveRemotePath(word);
        if (!path) return;
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
      const remote = !pane.isLocal;
      // 备用屏幕 = TUI 全屏应用（vim/less/man/claude code）：界面上的词是应用渲染的
      // 文字，不是 shell 输出中的文件路径，下载/预览无意义。仅普通 shell 输出才提供。
      const normalBuf = pane.term.buffer.active.type === "normal";
      const fileWord = normalBuf ? word : null;
      // TUI 模式（备用屏幕 + 鼠标模式开）下右键复制需要 Shift+drag 选中
      const tuiMode = !normalBuf && pane.mouseMode;
      showMenu(e.clientX, e.clientY, [
        {
          label: "复制",
          action: () => {
            const sel = pane.getSelectionText();
            if (sel) {
              void pane.copyText(sel);
            } else if (tuiMode) {
              toast("在 TUI 应用中需按住 Shift 拖动选中文本，再右键复制", true);
            }
          },
        },
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
              // 预览仅对图片提供（识别非图片则不展示，文本等无意义预览一律省去）
              ...(fileWord && VIEWABLE_IMG.test(fileWord)
                ? [
                    {
                      label: `预览 “${truncate(fileWord)}”`,
                      action: () => void pane.onPreview?.(fileWord),
                    },
                  ]
                : []),
              {
                label: fileWord ? `下载 “${truncate(fileWord)}”` : "下载",
                disabled: !fileWord,
                action: () => fileWord && void downloadFile(pane, fileWord),
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
    if (found?.pane.debugExit) {
      found.pane.term.write(`\r\n\x1b[90m[退出，状态码 ${e.status}]\x1b[0m\r\n`);
      found.pane.debugExit = false;
    }
  });

  void events.onPaneClosed((e) => {
    const found = tabs.findPane(e.paneId);
    if (!found) return;
    if (e.exited) {
      // 远程 shell 正常退出（用户 exit / hsshprod --exit）→ 退回本地终端，不关闭 pane。
      // preserve=true 保留远程输出不清屏，用户可查看命令执行结果。
      if (!found.pane.isLocal) {
        found.pane.switchConnection("local", true).catch(() => {});
      } else {
        // 本地终端退出 → 收起该分屏
        void tabs.closePane(found.tab, found.pane);
      }
    }
    // 连接断开（exited=false）→ 保留等待重连
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
  initTransfers(document.getElementById("btn-transfers")!);

  // 预热：空闲时提前加载 WebGL 渲染器分块，消除首次新建标签页时动态 import 的一次性卡顿。
  // 失败无妨（无 WebGL 时 pane 自会回退 canvas 渲染）。
  const preloadWebgl = () => void import("@xterm/addon-webgl").catch(() => {});
  const ric = (window as unknown as { requestIdleCallback?: (cb: () => void) => void }).requestIdleCallback;
  if (ric) ric(preloadWebgl);
  else window.setTimeout(preloadWebgl, 1500);

  // 本地终端标签标题 `目录:进程`：注入 home（用于 `~` 显示）并定时轮询刷新。
  api.localHome().then((h) => (tabs.localHomeDir = h)).catch(() => {});
  const pollLocalTitles = () => void tabs.refreshLocalTabTitles();
  pollLocalTitles();
  window.setInterval(pollLocalTitles, 1500);

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
    const basename = (p: string) => p.replace(/[/\\]+$/, "").split(/[/\\]/).pop() ?? p;
    // 立即为每个文件创建传输行（占位），让用户马上看到面板反馈
    const entries = paths.map((p) => ({ path: p, name: basename(p), tid: crypto.randomUUID() }));
    for (const e of entries) beginTransfer(e.tid, e.name, "upload", "解析目标目录…");
    // dirOverride（拖到某目录名上）最精确；否则用 pane 的已知 cwd，再退到 home。
    let dir: string | null;
    let guessed = false;
    if (dirOverride) {
      dir = dirOverride;
    } else {
      const u = await target.uploadDir();
      dir = u.dir;
      guessed = u.guessed;
    }
    if (!dir) {
      for (const e of entries) failTransfer(e.tid, "尚未获取远端目录，请稍候重试");
      return;
    }
    // cwd 未由 OSC7 上报时，上传落到 home 目录——明确告知，避免用户以为传到了当前目录
    if (guessed) {
      const ok = await confirmDialog(
        "确认上传目录",
        `未能获取该终端的当前工作目录（shell 未上报 OSC7）。\n将上传到用户主目录：${dir}\n\n继续吗？`,
      );
      if (!ok) {
        for (const e of entries) failTransfer(e.tid, "用户取消");
        return;
      }
    }
    // 同名文件处理：默认直接覆盖；开启「提示确认」后按项询问，可选「后续都如此」。
    const confirmOverwrite = getSettings().confirmOverwrite;
    let bulk: "overwrite" | "skip" | null = null; // 记住「全部」选择

    for (const e of entries) {
      const { path: p, name, tid } = e;
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
            failTransfer(tid, "已跳过同名文件");
            continue;
          }
        }
      }
      // 传输的成败/进度均由右下角传输面板呈现，不再用 toast，避免刷屏；
      // dir 作为行名悬停提示，让用户知道传到了哪里
      try {
        await api.sftpUpload(target.connId, p, dir, tid);
        completeTransfer(tid);
      } catch (err) {
        failTransfer(tid, String(err));
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
    // 拖放期间 xterm 可能创建选区（如从文本区拖到同一终端），释放后残留背景条
    for (const tab of tabs.tabs) for (const p of tab.layout.panes()) p.term.clearSelection();
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

  // Ctrl+拖拽远端文件 → 拖到「本地终端」pane → 下载到该终端实时 cwd。
  // 远端文件拖拽携带 DL_MIME，落点按终端类型分流：
  // - 本地终端 → 下载到该 shell 的 /proc cwd（拿不到则回退 home）；
  // - 远程终端 → 复制到该终端所在目录（同连接走服务器内 cp，跨连接经客户端流式中转）。
  // 与拖到本地文件面板的下载互不影响。
  let dlHlPane: HTMLElement | null = null;
  const clearDlHighlight = () => {
    dlHlPane?.classList.remove("dl-drop-target");
    dlHlPane = null;
  };
  const paneUnder = (x: number, y: number): Pane | null => {
    const el = document.elementFromPoint(x, y)?.closest(".pane") as HTMLElement | null;
    return (el?.dataset.paneId && tabs.findPane(el.dataset.paneId)?.pane) || null;
  };
  content.addEventListener("dragover", (e) => {
    if (!e.dataTransfer?.types.includes(DL_MIME)) return;
    const pane = paneUnder(e.clientX, e.clientY);
    if (!pane) {
      clearDlHighlight();
      return; // 非终端区域不接收，保留默认（禁止落下）
    }
    e.preventDefault();
    e.dataTransfer.dropEffect = "copy";
    if (dlHlPane !== pane.element) {
      clearDlHighlight();
      dlHlPane = pane.element;
      dlHlPane.classList.add("dl-drop-target");
    }
  });
  /** 远端文件拖入远程终端：复制到该终端的实时 cwd */
  const copyToRemotePane = async (src: Pane, srcSpec: string, target: Pane) => {
    try {
      const srcPath = await src.resolveRemotePath(srcSpec);
      if (!srcPath) {
        toast("无法解析远端路径（未知当前目录）", true);
        return;
      }
      const { dir, guessed } = await target.currentDir();
      if (!dir) {
        toast("尚未获取目标终端的远端目录，请稍候重试", true);
        return;
      }
      // cwd 未由 OSC7/proc 获得时落到 home 猜测——与上传拖放同款确认，避免静默复制错位置
      if (guessed) {
        const ok = await confirmDialog(
          "确认复制目录",
          `未能获取目标终端的当前工作目录。\n将复制到用户主目录：${dir}\n\n继续吗？`,
        );
        if (!ok) return;
      }
      const name = srcPath.replace(/\/+$/, "").split("/").pop() ?? srcPath;
      const tid = crypto.randomUUID();
      beginTransfer(tid, name, "upload", dir);
      try {
        const dest = await api.sftpCopyRemote(src.connId, srcPath, target.connId, dir, tid);
        completeTransfer(tid);
        toast(`已复制到 ${dest}`);
      } catch (err) {
        failTransfer(tid, String(err));
      }
    } catch (err) {
      // 路径解析 / cwd 获取等传输前错误仍用 toast 提示
      toast(`复制失败: ${err}`, true);
    }
  };
  // 拖拽操作结束（无论落在哪、是否成功落下）统一清理两套高亮：
  // 修复「拖回源终端/自己面板后，目标虚线残留不消失」——那些路径没有 drop 事件，
  // 但源元素在应用内，dragend 必然触发并冒泡到 document。
  document.addEventListener("dragend", () => {
    clearDlHighlight();
    clearDropIndicators();
  });
  content.addEventListener("drop", (e) => {
    const raw = e.dataTransfer?.getData(DL_MIME);
    if (!raw) return;
    const target = paneUnder(e.clientX, e.clientY);
    clearDlHighlight();
    if (!target) return;
    e.preventDefault();
    let payload: { connId: string; paneId?: string; path: string };
    try {
      payload = JSON.parse(raw) as { connId: string; paneId?: string; path: string };
    } catch {
      return; // 载荷异常，忽略
    }
    // 优先用拖拽发起的源 pane 解析相对路径，回退到该连接任一 pane
    const src = (payload.paneId && tabs.findPane(payload.paneId)?.pane) || tabs.panesByConn(payload.connId)[0]?.pane;
    if (!src) return;
    if (target.isLocal) {
      // 本地终端 → 下载（原有行为不变）
      void (async () => {
        const dir = (await target.resolveLocalCwd()) ?? (await api.localHome().catch(() => ""));
        if (!dir) {
          toast("无法确定本地终端目录", true);
          return;
        }
        void downloadFile(src, payload.path, dir);
      })();
    } else {
      // 远程终端 → 复制到其所在目录
      void copyToRemotePane(src, payload.path, target);
    }
  });

  // ---------- 下载 ----------

  /** 解析默认下载目录：设置里指定的优先，否则系统 Downloads */
  const resolveDownloadDir = async (): Promise<string> => {
    const s = getSettings().downloadDir.trim();
    if (s) return s;
    return api.defaultDownloadDir().catch(() => "");
  };

  /**
   * 下载远端文件/目录。remoteSpec 可为相对词或绝对路径——相对词按实时 cwd 解析，
   * 因此 shell 未上报 OSC7 时也能命中用户 `cd` 后的真实目录（经 /proc/PID）。
   * - targetDir 指定（拖到文件管理器某目录）→ 直接下载到该目录。
   * - 否则按设置：勾选"每次询问"→ 弹保存对话框；否则默认下载目录（Downloads / 自定义）。
   */
  const downloadFile = async (pane: Pane, remoteSpec: string, targetDir?: string) => {
    try {
      const remotePath = await pane.resolveRemotePath(remoteSpec);
      if (!remotePath) {
        toast("无法解析远端路径（未知当前目录）", true);
        return;
      }
      const meta = await api.sftpStat(pane.connId, remotePath);
      const name = remotePath.replace(/\/+$/, "").split("/").pop() ?? remotePath;
      let local: string | null = null;

      if (targetDir) {
        // 拖到文件管理器：目录用父目录（递归为同名目录），文件用父目录 + 文件名
        local = meta.isDir ? targetDir : `${targetDir.replace(/\/+$/, "")}/${name}`;
      } else if (getSettings().askDownloadLocation) {
        const dialog = await import("@tauri-apps/plugin-dialog");
        local = meta.isDir
          ? ((await dialog.open({ directory: true, title: `选择 “${name}” 的保存位置` })) as string | null)
          : await dialog.save({ defaultPath: name, title: "保存到本地" });
      } else {
        const dir = await resolveDownloadDir();
        if (!dir) {
          toast("未能确定下载目录，请在设置中指定", true);
          return;
        }
        local = meta.isDir ? dir : `${dir.replace(/\/+$/, "")}/${name}`;
      }
      if (!local) return;
      // 传输进度与成败由右下角面板呈现（可暂停/取消/删除）；
      // 落地完整路径作为行名悬停提示，让用户能找到文件。
      // 完成时另发一条 toast：小文件瞬间完成，仅靠面板行不够醒目
      const dest = meta.isDir ? `${local.replace(/\/+$/, "")}/${name}` : local;
      const tid = crypto.randomUUID();
      beginTransfer(tid, name, "download", dest);
      try {
        await api.sftpDownload(pane.connId, remotePath, local, tid);
        completeTransfer(tid);
        toast(`已下载到 ${dest}`);
      } catch (err) {
        failTransfer(tid, String(err));
      }
    } catch (err) {
      // 路径解析 / stat 等传输前的错误仍用 toast 提示
      toast(`下载失败: ${err}`, true);
    }
  };

  // ---------- 工具栏 ----------

  // ---------- 窗口控制（自定义标题栏） ----------

  const win = getCurrentWindow();
  // 最大化状态同步到 html data 属性，CSS 据此切换圆角（最大化时圆角变直角填满窗口，
  // 避免透明窗口四角露出桌面小孔）。
  const syncMaximized = async () => {
    document.documentElement.dataset.maximized = (await win.isMaximized()) ? "1" : "0";
  };
  await syncMaximized();
  void win.onResized(() => void syncMaximized());
  document.getElementById("btn-about")!.addEventListener("click", showAboutDialog);
  document.getElementById("btn-min")!.addEventListener("click", () => void win.minimize());
  document.getElementById("btn-max")!.addEventListener("click", async () => {
    const wasMaximized = await win.isMaximized();
    await win.toggleMaximize();
    if (wasMaximized) {
      // 从最大化还原：调用后端命令直接获取屏幕尺寸并设置窗口，
      // 避免 WebKitGTK 下 window.screen/currentMonitor 返回异常值。
      // 加 100ms 延迟让 window-state 插件先恢复，再覆盖为目标值。
      await new Promise((r) => setTimeout(r, 100));
      await api.restoreWindowSize().catch(() => {});
    }
  });
  // 关闭按钮触发 onCloseRequested，确认对话框与 session flush 统一在那里处理
  document.getElementById("btn-close")!.addEventListener("click", () => void win.close());

  document.getElementById("btn-newtab")!.addEventListener("click", () => tabs.onNewTabRequest?.());

  // 顶部连接图标：默认针对当前聚焦的终端（就地打开/切换连接）；无终端时才新建标签页
  document.getElementById("btn-connect")!.addEventListener("click", () => {
    const tab = tabs.active;
    const pane = tabs.activePane();
    if (tab && pane) connectInPane(tab, pane);
    else showConnectDialog(connectAndOpenTabVoid, openLocalTabVoid);
  });

  // ---------- 本地文件管理器面板（右侧浮动 45%，每标签页独立实例） ----------

  const explorerPanel = document.getElementById("explorer-panel")!;
  const explorerBtn = document.getElementById("btn-explorer") as HTMLButtonElement;
  // 本地文件图标激活态：面板打开时高亮（与远程图标 updateRemoteBtn 对齐）
  const updateExplorerBtn = () =>
    explorerBtn.classList.toggle("active", !!tabs.active?.explorerOpen);
  initPanelResize(explorerPanel, "right");
  /** revealDir=true（用户显式点击打开）时，定位到聚焦本地终端的实时 cwd；
   *  被动同步不打断用户正在浏览的目录（与远程面板同款策略） */
  const syncExplorerPanel = (revealDir = false) => {
    const tab = tabs.active;
    updateExplorerBtn();
    // 用 .open 类做滑入/滑出动画（面板常驻 DOM，不用 display 切换以免动画失效）
    if (!tab || !tab.explorerOpen) {
      explorerPanel.classList.remove("open");
      return;
    }
    if (!tab.explorer) {
      tab.explorer = new Explorer(localBackend());
      // 面板标题栏 ✕：关闭本地文件面板（等价于顶部本地文件图标再点一次）
      tab.explorer.onClose = () => {
        tab.explorerOpen = false;
        syncExplorerPanel();
      };
      tab.explorer.onUploadRequest = (paths) => void uploadFiles(paths);
      tab.explorer.onDownloadRequest = (connId, path, targetDir, srcPaneId) => {
        // 优先用拖拽发起的源 pane 解析相对路径，回退到该连接任一 pane
        const src = (srcPaneId && tabs.findPane(srcPaneId)?.pane) || tabs.panesByConn(connId)[0]?.pane;
        if (src) void downloadFile(src, path, targetDir);
        else toast("该连接已无终端，无法下载", true); // 不再静默：用户能知道为什么没反应
      };
    }
    // 仅替换已挂载的 explorer 子节点，保留常驻的尺寸把手（.panel-resize-*）；
    // 不能用 textContent="" 整体清空（会连把手一起删，令面板尺寸调节失效）。
    const mounted = explorerPanel.querySelector(":scope > .explorer");
    if (mounted !== tab.explorer.element) {
      mounted?.remove();
      explorerPanel.appendChild(tab.explorer.element);
    }
    explorerPanel.classList.add("open");
    // 聚焦本地终端时用其实时 cwd（/proc）定位；聚焦远程终端则回退 home
    const inst = tab.explorer;
    const pane = tabs.activePane();
    if (pane?.isLocal) {
      void pane
        .resolveLocalCwd()
        .catch(() => null)
        .then((dir) =>
          revealDir ? void inst.reveal(dir ?? undefined) : void inst.init(dir ?? undefined),
        );
    } else void inst.init();
  };

  // ---------- 远程文件管理器面板（左侧浮动 45%，每连接独立实例） ----------

  const remotePanel = document.getElementById("remote-panel")!;
  initPanelResize(remotePanel, "left");
  const remoteBtn = document.getElementById("btn-remote") as HTMLButtonElement;
  const remoteExplorers = new Map<string, Explorer>();

  /** 构造某连接的远程数据源；初始目录由 syncRemotePanel 用实时 cwd 注入 */
  const remoteBackend = (connId: string): ExplorerBackend => {
    const info = connMeta.get(connId);
    return {
      kind: "remote",
      connId,
      label: info ? `${info.name} · ${info.host}` : "远程",
      list: (dir) => api.remoteList(connId, dir),
      home: () => api.remoteHome(connId),
    };
  };

  /** 当前聚焦终端所属的活跃 SSH 连接 id；本地终端返回 null */
  const focusedConnId = (): string | null => {
    const pane = tabs.activePane();
    return pane && !pane.isLocal ? pane.connId : null;
  };

  /** revealDir=true（用户显式点击打开）时，定位到聚焦终端的实时 cwd；
   *  被动同步（切焦点/切标签）不打断用户正在浏览的目录 */
  const syncRemotePanel = (revealDir = false) => {
    const tab = tabs.active;
    const connId = focusedConnId();
    if (!tab || !tab.remoteOpen || !connId) {
      remotePanel.classList.remove("open");
      return;
    }
    let ex = remoteExplorers.get(connId);
    if (!ex) {
      ex = new Explorer(remoteBackend(connId));
      // 面板标题栏 ✕：关闭当前标签页的远程文件面板（等价于顶部远程图标再点一次）
      ex.onClose = () => {
        if (tabs.active) tabs.active.remoteOpen = false;
        syncRemotePanel();
        updateRemoteBtn();
      };
      ex.onDownloadRequest = (cid, path, targetDir) => {
        const found = tabs.panesByConn(cid)[0];
        if (found) void downloadFile(found.pane, path, targetDir || undefined);
        else toast("该连接已无终端，无法下载", true); // 不再静默：用户能知道为什么没反应
      };
      ex.onUploadHere = (localPaths, remoteDir) => {
        const found = tabs.panesByConn(connId)[0];
        if (!found) {
          toast("连接已断开", true);
          return;
        }
        const inst = ex!;
        void uploadFiles(localPaths, found.pane, remoteDir).then(() => inst.load().catch(() => {}));
      };
      remoteExplorers.set(connId, ex);
    }
    // 同 syncExplorerPanel：只替换已挂载的 explorer，保留尺寸把手
    const mounted = remotePanel.querySelector(":scope > .explorer");
    if (mounted !== ex.element) {
      mounted?.remove();
      remotePanel.appendChild(ex.element);
    }
    remotePanel.classList.add("open");
    // 初始目录取聚焦终端的实时 cwd（/proc）；已初始化则不再改动用户当前浏览目录。
    // connId 即 focusedConnId()，故聚焦 pane 必在此连接上；回退到该连接任一 pane。
    const inst = ex;
    const active = tabs.activePane();
    const pane = (active && active.connId === connId ? active : tabs.panesByConn(connId)[0]?.pane) || undefined;
    if (pane) {
      void pane.currentDir().then(({ dir }) =>
        // 显式打开 → 跳到聚焦终端当前目录（终端 cd 之后再开面板要看到新目录）；
        // 被动同步 → 仅首次初始化定位，不改动用户当前浏览目录
        revealDir ? void inst.reveal(dir ?? undefined) : void inst.init(dir ?? undefined),
      );
    } else void inst.init();
  };

  /** 远程按钮点亮/禁用：仅当聚焦终端有活跃 SSH 连接时可用（点亮态用强调色） */
  const updateRemoteBtn = () => {
    const on = !!focusedConnId();
    remoteBtn.classList.toggle("disabled", !on);
    remoteBtn.classList.toggle("active", on && !!tabs.active?.remoteOpen);
  };

  /** 统一刷新左右两个文件面板与远程按钮态 */
  const refreshPanels = () => {
    syncExplorerPanel();
    syncRemotePanel();
    updateRemoteBtn();
  };
  // 会话快照：遍历布局树逐 leaf 记录连接来源（不含机密），防抖写入 session.json。
  // 恢复期间 restoring=true 时跳过，避免在标签页尚未全部重建时把会话写成半截而丢失。
  let restoring = false;
  /** 布局树 → 可持久化快照：每个 leaf 记录自己的连接来源，split 只记结构与比例 */
  const snapLayout = (n: LayoutNode): SessionLayout => {
    if (n.type === "leaf") {
      const info = connMeta.get(n.pane.connId);
      return {
        type: "leaf",
        local: !info || info.local,
        name: info?.name ?? "本地终端",
        profileId: info?.profileId ?? null,
      };
    }
    return {
      type: "split",
      dir: n.dir,
      ratio: Math.round(n.ratio * 1000) / 1000,
      a: snapLayout(n.a),
      b: snapLayout(n.b),
    };
  };
  const snapshotSession = (): Promise<void> => {
    // 未开启「记住最后的会话」时不写盘：既避免无谓写入，也不覆盖上次保存的会话
    if (restoring || !getSettings().restoreSession) return Promise.resolve();
    const snap = tabs.tabs.map((tab) => {
      const first = tab.layout.panes()[0];
      const info = first ? connMeta.get(first.connId) : undefined;
      // 分屏结构仅在确有切分时记录（单 pane 省略，保持 session.json 精简）
      const layout = tab.layout.root.type === "split" ? snapLayout(tab.layout.root) : undefined;
      if (!info || info.local) return { local: true, name: info?.name ?? "本地终端", layout };
      return { local: false, name: info.name, profileId: info.profileId ?? null, layout };
    });
    return api.sessionSet(snap).catch(() => {});
  };
  let saveTimer: number | undefined;
  const scheduleSaveSession = () => {
    window.clearTimeout(saveTimer);
    saveTimer = window.setTimeout(snapshotSession, 500);
  };

  tabs.onLayoutChange = () => {
    refreshPanels();
    scheduleSaveSession();
  };

  // 关闭流程：确认 → flush session → destroy。btn-close（win.close）与系统关闭
  // （Alt+F4 / 任务栏右键）都走 onCloseRequested，统一由 performClose 处理。
  // closing flag 防重入：快速重复触发或 confirm 期间再次关闭时只执行一次，
  // 用户取消时重置以便下次再触发。
  let closing = false;
  const performClose = async (): Promise<void> => {
    if (closing) return;
    closing = true;
    const busy = tabs.tabs.some((t) => tabs.hasBusyPane(t));
    const ok = await confirmDialog(
      "退出 HetuShell",
      busy ? "有标签页中可能有程序正在运行，确定退出吗？" : "确定要退出 HetuShell 吗？",
    );
    if (!ok) { closing = false; return; } // 用户取消，窗口保持
    // flush 待保存的 session：取消 debounce 定时器，立即写一次并等其落盘
    if (saveTimer !== undefined) {
      window.clearTimeout(saveTimer);
      saveTimer = undefined;
    }
    await snapshotSession();
    await api.sessionRelease().catch(() => {});
    // destroy 失败时重置 closing，否则后续关闭请求全被 closing flag 跳过、窗口永久卡死
    await win.destroy().catch(() => { closing = false; });
  };

  // hexit：跳过确认，保存会话后直接 destroy（会话下次启动恢复）。
  // 复用 closing flag 防重入——若 performClose 已在进行中则不再触发。
  const performHexit = async (): Promise<void> => {
    if (closing) return;
    closing = true;
    if (saveTimer !== undefined) {
      window.clearTimeout(saveTimer);
      saveTimer = undefined;
    }
    await snapshotSession();
    await api.sessionRelease().catch(() => {});
    await win.destroy().catch(() => { closing = false; });
  };

  // 窗口关闭：onCloseRequested 拦截系统关闭与 btn-close 的 win.close()。
  // preventDefault 后用 setTimeout(0) 把 performClose 移出 listen 回调上下文——
  // snapshotSession/sessionRelease/destroy 均为 IPC invoke，在 listen 回调的
  // await 链内串行调用会阻塞，导致 destroy 永不执行，窗口关不掉。
  await win.onCloseRequested((event) => {
    event.preventDefault();
    setTimeout(() => void performClose(), 0);
  });

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
    syncExplorerPanel(tab.explorerOpen); // 打开时定位到聚焦终端当前目录
  });

  remoteBtn.addEventListener("click", () => {
    const tab = tabs.active;
    if (!tab) return;
    // 无活跃 SSH 连接（本地终端聚焦）时按钮为禁用态，点击无效
    if (!focusedConnId()) {
      toast("当前终端未连接远程主机", true);
      return;
    }
    tab.remoteOpen = !tab.remoteOpen;
    syncRemotePanel(tab.remoteOpen); // 打开时定位到聚焦终端当前目录
    updateRemoteBtn();
  });

  document.getElementById("btn-upload")!.addEventListener("click", () => void uploadViaDialog());
  document.getElementById("btn-split-h")!.addEventListener("click", () => void tabs.splitActive("row"));
  document.getElementById("btn-split-v")!.addEventListener("click", () => void tabs.splitActive("col"));
  document.getElementById("btn-settings")!.addEventListener("click", () => showSettingsDialog());

  // ---------- 设置热应用到所有终端 ----------

  // 追踪上次应用的主题 id + 基调：拖透明度/模糊时 onSettingsChange 仍会触发，
  // 但主题没变就不重设 theme/minimumContrastRatio——xterm 设这两项会重算所有
  // cell 的对比度（O(scrollback)），是拖滑条卡顿的根因。
  let lastThemeId = "";
  let lastThemeBase = "";
  let lastMcr = 0;
  let lastCursorStyle = "";
  let lastCursorColor = "";

  onSettingsChange(() => {
    const s = getSettings();
    const theme = activeTheme();
    const themeChanged = theme.id !== lastThemeId;
    const baseChanged = theme.base !== lastThemeBase;
    // 暗色：高不透明(>=0.85) 1.61，中(>=0.6) 1.57，中透明(>=0.4) 1.3，高透明 1.1 避免白边
    const mcr = theme.base === "dark"
      ? (s.opacity < 0.4 ? 1.1 : s.opacity < 0.6 ? 1.3 : s.opacity < 0.85 ? 1.57 : 1.61)
      : 1.1;
    const mcrChanged = mcr !== lastMcr;
    // 主题/字号/透明度立即生效（这些不依赖字体加载）
    for (const tab of tabs.tabs) {
      for (const pane of tab.layout.panes()) {
        pane.term.options.fontSize = s.fontSize;
        pane.term.options.fontWeight = "normal" as never;
        // 光标样式：非法值回退 block
        const cStyle = s.cursorStyle === "bar" ? "bar" : "block";
        if (cStyle !== lastCursorStyle) {
          pane.term.options.cursorStyle = cStyle as never;
          pane.term.options.cursorWidth = cStyle === "bar" ? 2 : undefined;
        }
        // 光标颜色随 theme 一起重设（cursor 字段在 theme 内）
        const cursorColorChanged = (s.cursorColor ?? "") !== lastCursorColor;
        if (themeChanged || baseChanged || cursorColorChanged) {
          const colors: Record<string, string> = { ...theme.colors, background: "#00000000" };
          colors.selectionBackground = "#8080806B";
          const cc = s.cursorColor?.trim();
          if (cc && /^#[0-9a-fA-F]{6}$/.test(cc)) colors.cursor = cc;
          pane.term.options.theme = colors as never;
        }
        // 仅 MCR 实际变化时才重设（O(scrollback) 重算）
        if (themeChanged || baseChanged || mcrChanged) {
          pane.term.options.minimumContrastRatio = mcr as never;
        }
      }
    }
    lastThemeId = theme.id;
    lastThemeBase = theme.base;
    lastMcr = mcr;
    lastCursorStyle = s.cursorStyle;
    lastCursorColor = s.cursorColor ?? "";
    // 字体单独处理：先确保所选字重（含各自 bold）就绪，再设 fontFamily 触发 xterm
    // 以正确字体重测单元格。否则切到未加载的字体（如 Light）时会用回退字体测量，首帧偏细/错位。
    const px = s.fontSize;
    void Promise.allSettled([
      document.fonts.load(`normal ${px}px "${s.fontFamily}"`),
      document.fonts.load(`bold ${px}px "${s.fontFamily}"`),
      document.fonts.load(`normal ${px}px "${s.cjkFontFamily}"`),
      document.fonts.load(`bold ${px}px "${s.cjkFontFamily}"`),
    ]).then(() => {
      for (const tab of tabs.tabs) {
        for (const pane of tab.layout.panes()) {
          pane.term.options.fontFamily = fontStack();
          pane.refit();
        }
      }
    });
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
      // 多 pane 分屏：标签标题跟随新活动 pane
      if (best.isLocal) {
        void api.localTabInfo(best.id).then((info) => {
          if (info) tabs.updateActivePaneTitle(tab, `${tabs.dirLabel(info.cwd)}:${info.process}`);
        }).catch(() => {});
      } else {
        const info = connMeta.get(best.connId);
        if (info) tabs.updateActivePaneTitle(tab, info.name);
      }
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
      case "resizeLeft": if (tab && p) tab.layout.adjustDivider(p, "left"); break;
      case "resizeRight": if (tab && p) tab.layout.adjustDivider(p, "right"); break;
      case "resizeUp": if (tab && p) tab.layout.adjustDivider(p, "up"); break;
      case "resizeDown": if (tab && p) tab.layout.adjustDivider(p, "down"); break;
      case "copy": {
        const sel = p?.getSelectionText();
        if (p && sel) void p.copyText(sel);
        break;
      }
      case "paste": void p?.pasteFromClipboard(); break;
    }
  };

  /** 命中快捷键则执行并阻止默认；返回是否命中。终端聚焦时由 pane.onAppKey 调用，
   *  其它场合由 window 兜底。 */
  const dispatchShortcut = (e: KeyboardEvent, pane?: Pane): boolean => {
    // 去重：同一 KeyboardEvent 可能既经 pane.onAppKey（终端聚焦）又冒泡到 window 兜底，
    // 两条路径共享同一事件对象，打标记确保一次按键只执行一次动作（修复分屏被创建两次）。
    if ((e as { __hetuHandled?: boolean }).__hetuHandled) return false;
    // 忽略操作系统按键自动重复：一次长按不应连开多个标签页/多次切分/多次切换
    // （修复「快捷键容易连续出现 2 个标签页」——按住 Ctrl+Shift+T 稍久即触发多次 keydown）
    if (e.repeat) return false;
    // 有弹窗打开时不响应全局快捷键（避免在对话框后面误切分/新建/关闭）
    if (document.querySelector(".modal-overlay")) return false;
    const action = matchAction(e, resolveBindings(getSettings().keybindings));
    if (!action) return false;
    (e as { __hetuHandled?: boolean }).__hetuHandled = true;
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
  /**
   * 按布局快照重放分屏结构，并为每个 leaf 恢复其各自的连接。
   *
   * 外层已按 tab 级信息创建了首个 pane（本地或 SSH 已连接）。rebuild 递归 split
   * 创建新 pane（继承被切分 pane 的连接），随后对每个 leaf 检查其保存的连接信息：
   * - 与当前 connId 一致 → 无需切换
   * - 本地终端 → switchConnection("local")
   * - SSH 连接 → 建立连接后 switchPaneConnection
   *
   * 向后兼容：旧版 session.json 的 leaf 无 local/profileId 字段 → 回退到 tab 级信息，行为同旧版。
   */
  const applySessionLayout = async (tab: Tab, snap?: SessionLayout | null) => {
    /** 布局树中的 leaf 节点类型（从 discriminated union 中提取） */
    type LeafNode = Extract<SessionLayout, { type: "leaf" }>;
    /** 收集布局树中所有 leaf 的连接信息（按前序遍历，与 panes() 顺序一致） */
    const collectLeaves = (node: SessionLayout | null | undefined): LeafNode[] => {
      if (!node || node.type !== "split") return [];
      const out: LeafNode[] = [];
      const walk = (n: SessionLayout) => {
        if (n.type === "leaf") out.push(n);
        else { walk(n.a); walk(n.b); }
      };
      walk(node);
      return out;
    };

    const rebuild = async (node: SessionLayout, pane: Pane): Promise<void> => {
      if (node?.type !== "split" || (node.dir !== "row" && node.dir !== "col")) return;
      const ratio = typeof node.ratio === "number" ? node.ratio : 0.5;
      const created = await tabs.splitPane(tab, pane, node.dir, ratio).catch(() => null);
      if (!created) return;
      await rebuild(node.a, pane);
      await rebuild(node.b, created);
    };

    if (snap?.type === "split") await rebuild(snap, tab.layout.panes()[0]);

    // 分屏结构重建后，逐 leaf 校正连接：splitPane 继承了源 pane 的 connId，
    // 需按快照中各 leaf 自己的连接信息独立切换。
    const leaves = collectLeaves(snap);
    const panes = tab.layout.panes();
    const profiles = leaves.some((l) => !l.local && l.profileId)
      ? await api.profilesList().catch(() => [] as Profile[])
      : [];

    for (let i = 0; i < leaves.length && i < panes.length; i++) {
      const leaf = leaves[i];
      const pane = panes[i];
      // 向后兼容：旧版 leaf 无 local 字段 → 跳过（继承 tab 级连接，行为同旧版）
      if (leaf.local === undefined) break;

      if (leaf.local) {
        if (!pane.isLocal) {
          await tabs.switchPaneConnection(tab, pane, "local", leaf.name ?? "本地终端");
        }
        continue;
      }

      // SSH leaf：需要 profileId 且连接项仍存在
      if (!leaf.profileId) continue;
      const p = profiles.find((pr) => pr.id === leaf.profileId);
      if (!p) continue;
      const hasKey = !!(p.keyData || p.keyPath);
      if (p.auth !== "key" || !hasKey) continue;

      // 已在同一连接上（外层 connectAndOpenTab 已为此 pane 建立连接）→ 无需切换
      const info = connMeta.get(pane.connId);
      if (info && !info.local && info.profileId === leaf.profileId) continue;

      // 建立新 SSH 连接并切换（最多重试 3 次，全部失败则该 pane 回退本地终端）
      const connParams = {
        name: p.name, host: p.host, port: p.port, user: p.user,
        auth: p.auth, keyPath: p.keyPath ?? undefined, keyData: p.keyData ?? undefined,
        keepalive: p.keepalive ?? undefined, timeout: p.timeout ?? undefined,
      };
      let connId: string | null = null;
      for (let attempt = 1; attempt <= 3; attempt++) {
        try {
          connId = await api.sshConnect(connParams);
          break;
        } catch (err) {
          if (attempt < 3) {
            await new Promise((r) => setTimeout(r, 1000 * attempt));
            continue;
          }
          toast(`恢复分屏连接「${leaf.name}」失败（重试 3 次）：${err}`, true);
        }
      }
      if (!connId) {
        if (!pane.isLocal) {
          await tabs.switchPaneConnection(tab, pane, "local", "本地终端").catch(() => {});
        }
        continue;
      }
      recordConn(connId, { name: p.name, host: p.host, port: p.port, user: p.user, auth: p.auth } as ConnParams, p.id);
      try {
        await tabs.switchPaneConnection(tab, pane, connId, p.name);
      } catch (err) {
        void api.sshDisconnect(connId).catch(() => {});
        connMeta.delete(connId);
        toast(`恢复分屏连接「${leaf.name}」失败：${err}`, true);
      }
    }

    // 重放过程中 activePaneId 落在最后创建的 pane 上，恢复完成后焦点回到首 pane
    const first = tab.layout.panes()[0];
    if (first) tab.activePaneId = first.id;
  };

  const session = getSettings().restoreSession ? await api.sessionGet().catch(() => []) : [];
  if (session.length === 0) {
    await openLocalTab();
    snapshotSession();
  } else {
    restoring = true;
    // 防御损坏/异常膨胀的 session.json：布局树深度已有 MAX_SPLIT_DEPTH 防护，
    // 顶层标签数量也要设限，避免启动时无界并发拉起标签与 PTY
    const capped = session.slice(0, 20);
    const profiles = capped.some((s) => !s.local && s.profileId)
      ? await api.profilesList().catch(() => [] as Awaited<ReturnType<typeof api.profilesList>>)
      : [];
    // 本地终端立即打开（秒开）；SSH 连接**并行在后台建立、不阻塞界面**——
    // 任何一个连接慢/挂起都不会卡死整个应用，其它标签页照常可用。
    const pending: Promise<unknown>[] = [];
    // order = 会话中的原始序号：后台并行连接完成顺序不定，最后按 order 归位
    capped.forEach((st, order) => {
      if (st.local) {
        pending.push(openLocalTab(order).then((tab) => applySessionLayout(tab, st.layout)));
        return;
      }
      if (!st.profileId) return; // 临时/手输连接，未保存为连接项 → 不恢复
      const p = profiles.find((pr) => pr.id === st.profileId);
      if (!p) return; // 连接项已删除
      const hasKey = !!(p.keyData || p.keyPath);
      if (p.auth !== "key" || !hasKey) {
        toast(`「${p.name}」未保存凭据，跳过自动连接`);
        return;
      }
      pending.push(
        connectAndOpenTab(
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
          order,
        )
          .then((tab) => applySessionLayout(tab, st.layout))
          .catch((err) => toast(`自动连接「${st.name}」失败：${err}`, true)),
      );
    });
    // 不 await：界面已可交互；后台连接全部结束后按保存顺序归位、解除 restoring 并落盘一次
    void Promise.allSettled(pending).then(() => {
      restoring = false;
      if (tabs.tabs.length === 0) void openLocalTab();
      tabs.reorderRestored();
      snapshotSession();
    });
  }
}

function truncate(s: string, n = 24): string {
  return s.length > n ? s.slice(0, n) + "…" : s;
}

void bootstrap();
