/** 后端 command 的类型化封装 */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ConnParams,
  ConnStateEvent,
  FileMeta,
  LocalEntry,
  PaneOutputEvent,
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

  sessionGet: () => invoke<SessionTab[]>("session_get"),
  sessionSet: (tabs: SessionTab[]) => invoke<void>("session_set", { tabs }),

  sshConnect: (params: ConnParams) => invoke<string>("ssh_connect", { params }),
  sshDisconnect: (connId: string) => invoke<void>("ssh_disconnect", { connId }),

  paneOpen: (connId: string, paneId: string, cols: number, rows: number) =>
    invoke<void>("pane_open", { connId, paneId, cols, rows }),
  paneOpenLocal: (paneId: string, cols: number, rows: number) =>
    invoke<void>("pane_open_local", { paneId, cols, rows }),
  paneInput: (paneId: string, data: string) => invoke<void>("pane_input", { paneId, data }),
  paneResize: (paneId: string, cols: number, rows: number) =>
    invoke<void>("pane_resize", { paneId, cols, rows }),
  paneClose: (paneId: string) => invoke<void>("pane_close", { paneId }),

  sftpStat: (connId: string, path: string) => invoke<FileMeta>("sftp_stat", { connId, path }),
  sftpPreview: (connId: string, path: string, maxBytes: number) =>
    invoke<Preview>("sftp_preview", { connId, path, maxBytes }),
  sftpDownload: (connId: string, remotePath: string, localPath: string, transferId: string) =>
    invoke<void>("sftp_download", { connId, remotePath, localPath, transferId }),
  sftpUpload: (connId: string, localPath: string, remoteDir: string, transferId: string) =>
    invoke<string>("sftp_upload", { connId, localPath, remoteDir, transferId }),
  remoteHome: (connId: string) => invoke<string>("remote_home", { connId }),

  localList: (dir: string) => invoke<LocalEntry[]>("local_list", { dir }),
  localHome: () => invoke<string>("local_home"),
};

export const events = {
  onPaneOutput: (fn: (e: PaneOutputEvent) => void): Promise<UnlistenFn> =>
    listen<PaneOutputEvent>("pane-output", (e) => fn(e.payload)),
  onPaneExit: (fn: (e: { paneId: string; status: number }) => void): Promise<UnlistenFn> =>
    listen("pane-exit", (e) => fn(e.payload as { paneId: string; status: number })),
  onPaneClosed: (fn: (e: { paneId: string; exited: boolean }) => void): Promise<UnlistenFn> =>
    listen("pane-closed", (e) => fn(e.payload as { paneId: string; exited: boolean })),
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
