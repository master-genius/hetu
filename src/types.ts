/** 与 Rust 后端 serde(camelCase) 对应的共享类型 */

export interface Profile {
  id: string;
  name: string;
  host: string;
  port: number;
  user: string;
  auth: "key" | "password";
  keyPath?: string | null;
  source: "manual" | "ssh_config";
}

export interface ThemeDef {
  id: string;
  name: string;
  base: "dark" | "light";
  colors: Record<string, string>;
}

export interface Settings {
  fontFamily: string;
  cjkFontFamily: string;
  fontSize: number;
  fontWeight: string;
  theme: string;
  /** 标题栏颜色；null 表示跟随主题 */
  titlebarColor: string | null;
  customThemes: ThemeDef[];
  opacity: number;
  blur: boolean;
  /** 新建标签页行为："local" 直接本地终端 | "dialog" 弹出连接选择 */
  newTabMode: "local" | "dialog";
  autoReconnect: boolean;
  copyOnSelect: boolean;
  profiles: Profile[];
}

export interface ConnParams {
  name: string;
  host: string;
  port: number;
  user: string;
  auth: "key" | "password";
  password?: string;
  keyPath?: string;
  passphrase?: string;
}

export interface FileMeta {
  path: string;
  size: number | null;
  isDir: boolean;
  isLink: boolean;
  perms: string;
  mtime: number | null;
  uid: number | null;
  gid: number | null;
  owner: string | null;
  group: string | null;
}

export interface Preview {
  data: string; // base64
  size: number;
  truncated: boolean;
  kind: "text" | "image" | "binary";
}

export interface PaneOutputEvent {
  paneId: string;
  data: string;
}

export interface ConnStateEvent {
  connId: string;
  state: "connected" | "reconnecting" | "waiting" | "closed";
  error?: string;
  retryIn?: number;
}

export interface LocalEntry {
  name: string;
  path: string;
  isDir: boolean;
  size: number;
  mtime: number | null;
}

export interface TransferProgressEvent {
  id: string;
  name: string;
  done: number;
  total: number;
  direction: "upload" | "download";
}
