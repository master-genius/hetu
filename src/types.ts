/** 与 Rust 后端 serde(camelCase) 对应的共享类型 */

export interface Profile {
  id: string;
  name: string;
  host: string;
  port: number;
  user: string;
  auth: "key" | "password";
  keyPath?: string | null;
  /** 私钥内容（PEM），自存于 profiles.json，不依赖外部文件 */
  keyData?: string | null;
  source: "manual" | "ssh_config";
  /** 备注/标记，便于查找与展示 */
  note?: string | null;
  /** 保活间隔（秒） */
  keepalive?: number | null;
  /** 连接超时（秒） */
  timeout?: number | null;
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
  /** 上传遇同名文件时提示确认；false 表示直接覆盖 */
  confirmOverwrite: boolean;
  /** 记住最后的会话：下次启动自动重开并连接 */
  restoreSession: boolean;
}

/** 会话中一个标签页的可持久化描述（不含任何机密） */
export interface SessionTab {
  local: boolean;
  name: string;
  /** 来源连接项 id；无（临时/手输连接）则不恢复 */
  profileId?: string | null;
}

export interface ConnParams {
  name: string;
  host: string;
  port: number;
  user: string;
  auth: "key" | "password";
  password?: string;
  keyPath?: string;
  keyData?: string;
  passphrase?: string;
  keepalive?: number;
  timeout?: number;
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
