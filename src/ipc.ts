/** 后端 command 的类型化封装 */

import { invoke, Channel } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ConnParams,
  ConnStateEvent,
  FileMeta,
  LocalEntry,
  RemoteEntry,
  PaneEvent,
  Preview,
  Profile,
  SessionTab,
  Settings,
  TransferProgressEvent,
} from "./types";

export const api = {
  settingsGet: () => invoke<Settings>("settings_get"),
  settingsSet: (settings: Settings) => invoke<void>("settings_set", { settings }),

  profilesList: () => invoke<Profile[]>("profiles_list"),
  profileSave: (profile: Profile) => invoke<void>("profile_save", { profile }),
  profileDelete: (id: string) => invoke<void>("profile_delete", { id }),

  sessionAcquire: () => invoke<number>("session_acquire"),
  sessionRelease: () => invoke<void>("session_release"),
  sessionGet: () => invoke<SessionTab[]>("session_get"),
  sessionSet: (tabs: SessionTab[]) => invoke<void>("session_set", { tabs }),

  sshConnect: (params: ConnParams) => invoke<string>("ssh_connect", { params }),
  sshDisconnect: (connId: string) => invoke<void>("ssh_disconnect", { connId }),
  /** 查询连接是否有进行中的传输（gcConnections 判断是否可安全断开） */
  connHasTransfers: (connId: string) => invoke<boolean>("conn_has_transfers", { connId }),

  paneOpen: (connId: string, paneId: string, cols: number, rows: number, onEvent: Channel<PaneEvent>) =>
    invoke<void>("pane_open", { connId, paneId, cols, rows, onEvent }),
  paneOpenLocal: (paneId: string, cols: number, rows: number, cwd: string | null, hsshToken: string, onEvent: Channel<PaneEvent>) =>
    invoke<void>("pane_open_local", { paneId, cols, rows, cwd: cwd ?? null, hsshToken, onEvent }),
  paneInput: (paneId: string, data: string) => invoke<void>("pane_input", { paneId, data }),
  paneResize: (paneId: string, cols: number, rows: number) =>
    invoke<void>("pane_resize", { paneId, cols, rows }),
  paneClose: (paneId: string) => invoke<void>("pane_close", { paneId }),

  sftpStat: (connId: string, path: string) => invoke<FileMeta>("sftp_stat", { connId, path }),
  sftpPreview: (connId: string, path: string, maxBytes: number) =>
    invoke<Preview>("sftp_preview", { connId, path, maxBytes }),
  /** 图片整读预览：本地直接读文件；远端经磁盘缓存（connId 传 "local" 表示本机） */
  imagePreview: (connId: string, path: string) =>
    invoke<{ data: string; size: number }>("image_preview", { connId, path }),
  sftpDownload: (connId: string, remotePath: string, localPath: string, transferId: string) =>
    invoke<void>("sftp_download", { connId, remotePath, localPath, transferId }),
  sftpUpload: (connId: string, localPath: string, remoteDir: string, transferId: string) =>
    invoke<string>("sftp_upload", { connId, localPath, remoteDir, transferId }),
  /** 远程→远程复制：同连接走服务器内 cp，跨连接经客户端流式中转；返回目标端根路径 */
  sftpCopyRemote: (srcConnId: string, srcPath: string, dstConnId: string, dstDir: string, transferId: string) =>
    invoke<string>("sftp_copy_remote", { srcConnId, srcPath, dstConnId, dstDir, transferId }),
  transferPause: (transferId: string) => invoke<void>("transfer_pause", { transferId }),
  transferResume: (transferId: string) => invoke<void>("transfer_resume", { transferId }),
  transferCancel: (transferId: string) => invoke<void>("transfer_cancel", { transferId }),
  remoteHome: (connId: string) => invoke<string>("remote_home", { connId }),
  remoteList: (connId: string, path: string) =>
    invoke<RemoteEntry[]>("sftp_list", { connId, path }),
  remoteCwd: (connId: string, pid: number) => invoke<string>("remote_cwd", { connId, pid }),
  defaultDownloadDir: () => invoke<string>("default_download_dir"),

  listFonts: () => invoke<string[]>("list_fonts"),
  localList: (dir: string) => invoke<LocalEntry[]>("local_list", { dir }),
  localHome: () => invoke<string>("local_home"),
  /** 本地终端 pane 的实时工作目录（经其 shell 的 /proc/<pid>/cwd） */
  localCwd: (paneId: string) => invoke<string>("local_cwd", { paneId }),
  /** 本地终端标签页信息：工作目录 + 前台进程名（标签标题 `目录:进程`） */
  localTabInfo: (paneId: string) =>
    invoke<{ cwd: string; process: string }>("local_tab_info", { paneId }),
  readKeyFile: (path: string) => invoke<string>("read_key_file", { path }),
  /** 读取 hssh 自动化喂入临时文件（读完即删） */
  readFeedFile: (path: string) => invoke<string>("read_feed_file", { path }),
  /** 用系统默认浏览器打开外部 http/https 链接 */
  openExternal: (url: string) => invoke<void>("open_external", { url }),
  /** 从最大化还原窗口尺寸（后端直接获取屏幕尺寸 + 设置窗口） */
  restoreWindowSize: () => invoke<void>("restore_window_size"),

  // ---------- Agent ----------

  agentSpawn: (tabId: string, mode: string, role: string, initialMessage: string | null, onEvent: Channel<any>) =>
    invoke<void>("agent_spawn", { tabId, mode, role, initialMessage, onEvent }),
  agentSendMessage: (tabId: string, message: string) =>
    invoke<void>("agent_send_message", { tabId, message }),
  agentAbort: (tabId: string) =>
    invoke<void>("agent_abort", { tabId }),
  agentDestroy: (tabId: string) =>
    invoke<void>("agent_destroy", { tabId }),
};

export const events = {
  onConnState: (fn: (e: ConnStateEvent) => void): Promise<UnlistenFn> =>
    listen<ConnStateEvent>("conn-state", (e) => fn(e.payload)),
  onTransferProgress: (fn: (e: TransferProgressEvent) => void): Promise<UnlistenFn> =>
    listen<TransferProgressEvent>("transfer-progress", (e) => fn(e.payload)),
};

/** base64 → Uint8Array（终端输出解码） */
export function b64decode(s: string): Uint8Array {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** Uint8Array/string → base64（终端输入编码，兼容多字节字符） */
export function b64encode(s: string): string {
  const bytes = new TextEncoder().encode(s);
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin);
}
