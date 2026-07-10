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
  /** 毛玻璃模糊程度（px） */
  blurAmount: number;
  /** 背景虚化：终端整体背景虚化/光晕/玻璃效果的总开关（默认开）。
   *  关闭后透明度仍作用于底色，但复杂背景效果停用，弹窗模糊不受影响。 */
  bgBlur: boolean;
  /** 磨砂质感：独立于毛玻璃的表面颗粒层（同色系噪点） */
  frosted: boolean;
  /** 磨砂程度 0–100（映射颗粒透明度） */
  frostStrength: number;
  /** 圆角级别 */
  cornerRadius: "square" | "xs" | "sm" | "md" | "lg";
  /** 标签页平分横向宽度 */
  tabBarFill: boolean;
  /** 标签页字体（空 = 同主字体） */
  tabFontFamily: string;
  /** 标签页字号（0/不填 = 默认 12，不跟随终端字号） */
  tabFontSize: number;
  /** 显示终端滚动条 */
  showScrollbar: boolean;
  /** 新建标签页行为："local" 直接本地终端 | "dialog" 弹出连接选择 */
  newTabMode: "local" | "dialog";
  autoReconnect: boolean;
  copyOnSelect: boolean;
  /** 上传遇同名文件时提示确认；false 表示直接覆盖 */
  confirmOverwrite: boolean;
  /** 默认下载目录（空 = 自动，系统 Downloads） */
  downloadDir: string;
  /** 每次下载都询问保存位置 */
  askDownloadLocation: boolean;
  /** 追踪远程工作目录（注入隐形 PID 标记 + /proc 读实时 cwd） */
  trackRemoteCwd: boolean;
  /** 记住最后的会话：下次启动自动重开并连接 */
  restoreSession: boolean;
  /** 窗口还原尺寸（屏幕占比百分比 35-90，默认 78） */
  restoreSize: number;
  /** 图片预览单张上限（MB），范围 32–512，默认 128 */
  maxImageMb: number;
  /** 光标样式："block" | "bar" */
  cursorStyle: "block" | "bar";
  /** 光标颜色（#rrggbb）；null 表示跟随主题 */
  cursorColor: string | null;
  /** 自定义快捷键：动作 → 组合键（仅存覆盖项） */
  keybindings: Record<string, string>;
}

/** 分屏布局快照：结构 + 比例 + 每个 leaf 的连接来源，随会话持久化。
 *  local/profileId/name 为可选字段：旧版 session.json 的 leaf 不携带这些字段，
 *  恢复时回退到 tab 级信息（向后兼容）。 */
export type SessionLayout =
  | { type: "leaf"; local?: boolean; profileId?: string | null; name?: string }
  | { type: "split"; dir: "row" | "col"; ratio: number; a: SessionLayout; b: SessionLayout };

/** 会话中一个标签页的可持久化描述（不含任何机密） */
export interface SessionTab {
  local: boolean;
  name: string;
  /** 来源连接项 id；无（临时/手输连接）则不恢复 */
  profileId?: string | null;
  /** 分屏结构快照；缺省/叶子 = 单 pane，不切分 */
  layout?: SessionLayout | null;
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

export interface RemoteEntry {
  name: string;
  path: string;
  isDir: boolean;
  isLink: boolean;
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
