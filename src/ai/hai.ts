/** hai — OSC 1733 解析 + Agent Modal 入口。
 *  与 hssh/himage/hfile 同安全模型：仅本地终端 + HSSH_TOKEN 校验。
 *  OSC 常量 HAI_OSC 定义在 pane.ts（与 hssh/hexit/himage/hfile 一致）。 */

import type { HaiSpec } from "./protocol";

/** base64 → UTF-8 字符串（与 pane.ts b64utf8 同实现）。 */
function b64utf8(s: string): string {
  if (!s) return "";
  try {
    const bin = atob(s);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    return new TextDecoder().decode(bytes);
  } catch {
    return "";
  }
}

/** 解析 hai OSC 1733 载荷（`k=v;k=v`，值经 base64）。非法返回 null。 */
export function parseHai(data: string): HaiSpec | null {
  const f: Record<string, string> = {};
  for (const kv of data.split(";")) {
    const i = kv.indexOf("=");
    if (i > 0) f[kv.slice(0, i)] = kv.slice(i + 1);
  }
  const tok = b64utf8(f.tok ?? "");
  if (!tok) return null;
  return {
    tok,
    op: b64utf8(f.op ?? ""),
    role: b64utf8(f.role ?? "") || "general",
    mode: b64utf8(f.mode ?? "") || "auto",
    msg: b64utf8(f.msg ?? ""),
    w: f.w === "1",
  };
}
