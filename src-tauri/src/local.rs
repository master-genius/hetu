//! 本地终端：不建立 SSH 连接时，直接在本机 PTY 中运行用户 shell（bash 等）。
//! 复用与 SSH pane 相同的 PaneCmd 通道与事件协议，前端无差别渲染。

use base64::Engine;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Serialize;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

use crate::error::{Error, Result};
use crate::ssh::pane::PaneCmd;

// ---------- 本地文件系统（文件管理器面板用） ----------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: Option<u64>,
}

/// 列出本地目录（目录在前，按名称排序）
pub fn list_dir(dir: &str) -> Result<Vec<LocalEntry>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let Ok(meta) = entry.metadata() else { continue };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        entries.push(LocalEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            path: entry.path().to_string_lossy().into_owned(),
            is_dir: meta.is_dir(),
            size: meta.len(),
            mtime,
        });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

pub fn home_dir() -> String {
    dirs::home_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".into())
}

/// 本地 shell 的实时工作目录：读 /proc/<pid>/cwd 符号链接（与远端 remote_cwd 同理）。
/// 仅 Linux 有 /proc；其它平台不支持（前端会回退到 home）。
#[cfg(target_os = "linux")]
pub fn cwd(pid: u32) -> Result<String> {
    let target = std::fs::read_link(format!("/proc/{pid}/cwd"))?;
    Ok(target.to_string_lossy().into_owned())
}

#[cfg(not(target_os = "linux"))]
pub fn cwd(_pid: u32) -> Result<String> {
    Err(Error::msg("当前平台不支持读取本地终端工作目录"))
}

/// 本地终端标签页信息：实时工作目录 + 前台进程名（供标签标题展示 `目录:进程`）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabInfo {
    /// 实时工作目录（绝对路径）
    pub cwd: String,
    /// 前台进程名：idle 时为 shell 名（bash/zsh…），运行程序时为程序名（vim/node…）
    pub process: String,
}

/// 读取本地终端标签页信息：cwd（/proc/<pid>/cwd）+ 前台进程名。
/// 前台进程：读 /proc/<pid>/stat 的 tpgid（该 tty 的前台进程组），再取组 leader 的 comm；
/// idle 时 tpgid 指向 shell 自身 → "bash"/"zsh"，运行程序时 → 程序名。取不到则回退 shell 自身 comm。
#[cfg(target_os = "linux")]
pub fn tab_info(pid: u32) -> Result<TabInfo> {
    let cwd = std::fs::read_link(format!("/proc/{pid}/cwd"))?
        .to_string_lossy()
        .into_owned();
    let process = fg_process_name(pid).unwrap_or_else(|| shell_comm(pid));
    Ok(TabInfo { cwd, process })
}

/// shell 自身进程名（前台进程探测失败时的兜底）
#[cfg(target_os = "linux")]
fn shell_comm(pid: u32) -> String {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// pid 所在 tty 的前台进程组 leader 的进程名。
#[cfg(target_os = "linux")]
fn fg_process_name(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // comm 字段（第 2 个）可能含空格与括号，跳到最后一个 ')' 之后再按空白切分，
    // 剩余首字段为 state，依次为 ppid pgrp session tty_nr tpgid…，故 tpgid 位于索引 5。
    let rest = stat.get(stat.rfind(')')? + 1..)?;
    let tpgid: i32 = rest.split_whitespace().nth(5)?.parse().ok()?;
    if tpgid <= 0 {
        return None;
    }
    let comm = std::fs::read_to_string(format!("/proc/{tpgid}/comm")).ok()?;
    let comm = comm.trim();
    (!comm.is_empty()).then(|| comm.to_string())
}

#[cfg(not(target_os = "linux"))]
pub fn tab_info(_pid: u32) -> Result<TabInfo> {
    Err(Error::msg("当前平台不支持读取本地终端标签页信息"))
}

// ---------- 本地 PTY ----------

fn size(cols: u32, rows: u32) -> PtySize {
    // 钳制到合法范围：0 或 >u16 都会让 curses 应用错乱，避免 `as u16` 直接回绕
    PtySize {
        rows: rows.clamp(1, u16::MAX as u32) as u16,
        cols: cols.clamp(1, u16::MAX as u32) as u16,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// 某可执行文件是否能在 PATH 中找到
#[cfg(windows)]
fn on_path(exe: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| dir.join(exe).is_file())
}

/// 本机默认 shell：
/// - Windows：优先 PowerShell 7（pwsh.exe），否则系统自带 Windows PowerShell（powershell.exe）
/// - macOS / Linux：系统默认 shell（$SHELL），兜底 /bin/bash
#[cfg(windows)]
fn default_shell() -> String {
    if on_path("pwsh.exe") {
        "pwsh.exe".into()
    } else {
        "powershell.exe".into()
    }
}

#[cfg(not(windows))]
fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into())
}

/// hssh 辅助命令脚本（POSIX sh）。运行时写入 config/hetushell/bin 并前置到本地 shell 的 PATH，
/// 让用户在 HetuShell 本地终端里用 `hssh <名称>` / `hssh 用户@主机` 快速打开自实现 SSH 连接
/// （只有自实现 SSH 才有远程文件面板/右键下载）。脚本仅发出 OSC 1729 转义序列通知前端，
/// 由前端按「点连接面板」的同一路径建连；脚本本身不接触任何机密逻辑。仅本地终端注入。
#[cfg(not(windows))]
const HSSH_SCRIPT: &str = r#"#!/bin/sh
# HetuShell 内建命令：在应用内打开一个自实现 SSH 连接（带远程文件面板）。
# 通过 OSC 1729 通知宿主前端，等价于在连接面板点击。仅在 HetuShell 本地终端内有效。
OSC=1729
emit() { printf '\033]%s;%s\007' "$OSC" "$1"; }
b64() { printf '%s' "$1" | base64 | tr -d '\n'; }

usage() {
  cat <<'EOF'
用法:
  hssh <连接项名称>            直接打开已保存的连接项（密钥认证直连；密码认证会弹窗要密码）
  hssh 用户@主机 [选项]        临时连接（缺密码且无密钥时弹窗询问）
  hssh 主机 -w 密码 [选项]     临时连接裸主机（裸名不带选项时按连接项名解析）
  hssh --prod <连接项> [选项]  prod 模式：连接后自动执行命令并可选退出
  hsshprod <连接项> [选项]     hssh --prod 的快捷别名

连接选项:
  -p, --port <端口>           端口（默认 22）
  -u, --user <用户名>         用户名（也可写成 用户@主机）
  -w, --password <密码>       密码（明文，注意会进 shell 历史）
  -i, --identity <私钥文件>   私钥文件路径

prod 模式选项（仅在 --prod 下生效）:
  -e, --exec <命令>           连接后自动执行命令（多条用分号分隔）
  -f, --file <脚本文件>       连接后自动执行文件中的命令
  -s, --stdin                 从标准输入读取命令并执行
  -x, --exit                  命令执行完后退出，退出码跟随最后一条命令
  --quiet                     静默模式：不输出连接提示和退出状态码
  --debug                     详细模式：输出连接提示和退出状态码（与 --quiet 相反）
  -l, --list                  列出已保存的连接项（名称/主机/备注）
  -h, --help                  显示此帮助

示例:
  hssh myserver
  hssh root@10.0.0.9 -p 2222 -w 'secret'
  hssh --prod claude --exec "ls -la; df -h"
  hssh --prod claude --file deploy.sh --exit
  echo "uptime; free -m" | hsshprod claude --stdin --exit
  hssh -l
EOF
}

[ $# -eq 0 ] && { usage; exit 1; }

# 预扫描 --prod：可出现在任意位置，prod 选项（-e/-f/-s/-x）依赖此标志
prod_mode=0
for _a in "$@"; do
  case "$_a" in --prod) prod_mode=1;; esac
done

# 单遍参数解析：连接信息（连接项名或 user@host）可在任意位置出现
host=""; user=""; port=""; pass=""; ident=""; name=""; adhoc=0
feed=""; feed_file=""; feed_stdin=0; do_exit=0; quiet=0; debug=0; list_mode=0
_first_pos=1

need() { [ "$1" -ge 2 ] || { echo "hssh: 选项 $2 缺少参数" >&2; exit 1; }; }
while [ $# -gt 0 ]; do
  case "$1" in
    -h|--help)     usage; exit 0;;
    --prod)        shift;;
    -p|--port)     need $# "$1"; port="$2"; adhoc=1; shift 2;;
    -u|--user)     need $# "$1"; user="$2"; adhoc=1; shift 2;;
    -w|--password) need $# "$1"; pass="$2"; adhoc=1; shift 2;;
    -i|--identity) need $# "$1"; ident="$2"; adhoc=1; shift 2;;
    -e|--exec)     [ "$prod_mode" = 1 ] || { echo "hssh: --exec 需要 --prod 模式" >&2; exit 1; }; need $# "$1"; feed="$2"; shift 2;;
    -f|--file)     [ "$prod_mode" = 1 ] || { echo "hssh: --file 需要 --prod 模式" >&2; exit 1; }; need $# "$1"; feed_file="$2"; shift 2;;
    -s|--stdin)    [ "$prod_mode" = 1 ] || { echo "hssh: --stdin 需要 --prod 模式" >&2; exit 1; }; feed_stdin=1; shift;;
    -x|--exit)     [ "$prod_mode" = 1 ] || { echo "hssh: --exit 需要 --prod 模式" >&2; exit 1; }; do_exit=1; shift;;
    --quiet)       quiet=1; shift;;
    --debug)       debug=1; shift;;
    -l|--list)     list_mode=1; shift;;
    -*)            echo "hssh: 未知选项 $1" >&2; exit 1;;
    *)             if [ "$_first_pos" = 1 ]; then
                     case "$1" in
                       *@*) user="${1%@*}"; host="${1#*@}"; adhoc=1;;
                       *)   name="$1";;
                     esac
                     _first_pos=0
                   else
                     host="$1"; adhoc=1
                   fi
                   shift;;
  esac
done

# 裸名 + 任一连接选项 → 视为临时连接到该主机（而非连接项名），兑现 `hssh 主机 -p/-w …`。
if [ "$adhoc" = 1 ] && [ -n "$name" ] && [ -z "$host" ]; then
  host="$name"; name=""
fi

# -l/--list：列出已保存的连接项（名称/主机/备注），由前端读取 profiles 后格式化输出。
# emit OSC 后阻塞等待前端信号（换行符），确保列表内容在 shell prompt 之前写入终端。
# 时序：emit → 前端 term.write(列表) → 前端 paneInput(\n) → read 返回 → exit → shell prompt。
if [ "$list_mode" = 1 ]; then
  tok=$(b64 "${HSSH_TOKEN:-}")
  emit "v=1;tok=$tok;mode=list"
  read _hssh_list_done
  exit 0
fi

# 必须有连接目标
if [ -z "$name" ] && [ -z "$host" ]; then
  echo "hssh: 需要连接项名称或主机" >&2; exit 1
fi

# 自动化喂入（仅 prod 模式）：收集命令到临时文件，路径随 OSC 传给前端，连接成功后由前端读回喂入。
# 用完由后端 read_feed_file 删除，原文件不受影响。
feed_path=""
if [ "$prod_mode" = 1 ]; then
  if [ -n "$feed_file" ]; then
    [ -f "$feed_file" ] || { echo "hssh: 文件不存在: $feed_file" >&2; exit 1; }
    fsize=$(wc -c < "$feed_file" 2>/dev/null || echo 0)
    [ "$fsize" -gt 1048576 ] && { echo "hssh: 脚本文件过大（上限 1MB）" >&2; exit 1; }
    feed_path=$(mktemp "${TMPDIR:-/tmp}/hssh_feed.XXXXXX") || { echo "hssh: 创建临时文件失败" >&2; exit 1; }
    cat "$feed_file" > "$feed_path"
  elif [ -n "$feed" ]; then
    feed_path=$(mktemp "${TMPDIR:-/tmp}/hssh_feed.XXXXXX") || { echo "hssh: 创建临时文件失败" >&2; exit 1; }
    printf '%s\n' "$feed" > "$feed_path"
  elif [ "$feed_stdin" = 1 ]; then
    feed_path=$(mktemp "${TMPDIR:-/tmp}/hssh_feed.XXXXXX") || { echo "hssh: 创建临时文件失败" >&2; exit 1; }
    cat > "$feed_path"
  fi
fi

# 能力令牌：仅由本地 shell 环境中的 $HSSH_TOKEN 提供，供前端校验来源，
# 防止终端里被渲染的不可信内容（cat 恶意文件、ls 恶意文件名等）伪造本序列诱导建连。
tok=$(b64 "${HSSH_TOKEN:-}")
feed_b64=""; [ -n "$feed_path" ] && feed_b64=$(b64 "$feed_path")
exit_b64=""; [ "$do_exit" = 1 ] && exit_b64=$(b64 "1")
quiet_b64=""; [ "$quiet" = 1 ] && [ "$debug" = 0 ] && quiet_b64=$(b64 "1")
debug_b64=""; [ "$debug" = 1 ] && debug_b64=$(b64 "1")
if [ -n "$name" ]; then
  emit "v=1;tok=$tok;mode=profile;name=$(b64 "$name");feed=$feed_b64;exit=$exit_b64;quiet=$quiet_b64;debug=$debug_b64"
  # --quiet 静默连接提示；--debug 覆盖 --quiet 强制输出
  [ "$quiet" = 1 ] && [ "$debug" = 0 ] || printf '→ 正在打开连接项「%s」…\n' "$name"
else
  [ -z "$host" ] && { echo 'hssh: 缺少主机' >&2; exit 1; }
  user="${user:-$(id -un)}"
  emit "v=1;tok=$tok;mode=adhoc;host=$(b64 "$host");user=$(b64 "$user");port=$(b64 "$port");pass=$(b64 "$pass");ident=$(b64 "$ident");feed=$feed_b64;exit=$exit_b64;quiet=$quiet_b64;debug=$debug_b64"
  [ "$quiet" = 1 ] && [ "$debug" = 0 ] || printf '→ 正在连接 %s@%s …\n' "$user" "$host"
fi
"#;

/// hsshprod 快捷别名：等价于 `hssh --prod --quiet`，静默连接 + 自动执行命令。
const HSSHPROD_SCRIPT: &str = r#"#!/bin/sh
exec hssh --prod --quiet "$@"
"#;

/// hexit 退出命令：仅本地终端有效，发出 OSC 1730 通知前端直接退出（无确认弹窗）。
/// 复用 $HSSH_TOKEN 来源校验，防止终端中被渲染的不可信内容伪造退出序列。
#[cfg(not(windows))]
const HEXIT_SCRIPT: &str = r#"#!/bin/sh
# HetuShell 内建命令：直接退出当前 HetuShell 窗口（无确认弹窗）。
# 通过 OSC 1730 通知宿主前端，仅本地终端有效。
OSC=1730
emit() { printf '\033]%s;%s\007' "$OSC" "$1"; }
b64() { printf '%s' "$1" | base64 | tr -d '\n'; }
emit "tok=$(b64 "${HSSH_TOKEN:-}")"
"#;

/// himage 图片查看命令：仅本地终端有效，发出 OSC 1731 携带图片路径列表。
/// 参数可以是图片文件或目录（目录自动扫描图片按名排序，上限 300 张）。
/// 复用 $HSSH_TOKEN 来源校验。
#[cfg(not(windows))]
const HIMAGE_SCRIPT: &str = r#"#!/bin/sh
# HetuShell 内建命令：在终端内弹出图片查看器。
# 用法: himage [选项] <图片路径|目录> [<图片路径|目录> ...]
#       himage -h, --help
# -w/--with-shell: 弹窗跟随当前终端分屏大小定位
# 目录参数自动展开为目录下的图片文件（按名排序），上限 300 张。
OSC=1731
emit() { printf '\033]%s;%s\007' "$OSC" "$1"; }
b64() { printf '%s' "$1" | base64 | tr -d '\n'; }

usage() {
  cat <<'EOF'
用法: himage [选项] <图片路径|目录> [<图片路径|目录> ...]
选项:
  -w, --with-shell    弹窗跟随当前终端分屏大小定位
  -h, --help          显示此帮助
参数可以是图片文件或目录（目录自动扫描图片按名排序，上限 300 张）。
支持格式: png jpg jpeg gif webp bmp svg ico
EOF
}

# 无参数 → 显示帮助
[ $# -eq 0 ] && { usage; exit 0; }

_with_shell=0
_paths=""
_count=0
add_path() {
  _paths="${_paths}${1}
"
  _count=$((_count + 1))
}

for _arg in "$@"; do
  case "$_arg" in
    -h|--help) usage; exit 0 ;;
    -w|--with-shell) _with_shell=1; continue ;;
  esac
  # 转绝对路径
  _abs=$(readlink -f -- "$_arg" 2>/dev/null) || _abs="$_arg"
  if [ -d "$_abs" ]; then
    # 扫描目录下的图片（find + while read 安全处理含空格文件名）
    # 管道子 shell 中变量修改不可见，用临时文件传递
    _tmpfile=$(mktemp 2>/dev/null) || _tmpfile="/tmp/himage_$$_tmp"
    _find_expr=""
    for _ext in png jpg jpeg gif webp bmp svg ico; do
      _find_expr="${_find_expr:+$_find_expr -o }-iname \"*.$_ext\""
    done
    eval "find \"\$_abs\" -maxdepth 1 -type f \( $_find_expr \)" 2>/dev/null | sort > "$_tmpfile"
    while IFS= read -r _f; do
      [ -z "$_f" ] && continue
      [ $_count -ge 300 ] && break
      add_path "$_f"
    done < "$_tmpfile"
    rm -f "$_tmpfile" 2>/dev/null
  elif [ -f "$_abs" ]; then
    [ $_count -ge 300 ] && break
    add_path "$_abs"
  fi
done

if [ $_count -eq 0 ]; then
  echo "himage: 未找到图片文件" >&2
  exit 1
fi

# 构造 OSC 载荷：tok=<b64>;paths=<b64>;w=<0|1>
_payload="tok=$(b64 "${HSSH_TOKEN:-}");paths=$(printf '%s' "$_paths" | base64 | tr -d '\n');w=$_with_shell"
emit "$_payload"
"#;

/// hfile 文件面板命令：仅本地终端有效，发出 OSC 1732 通知前端打开文件管理器面板。
/// - 不带选项 → 切换本地面板（等同点击图标）；-d 指定初始目录
/// - -w/--with-shell → 在当前终端分屏上创建浮动覆盖层（非独占，ESC 关闭）
/// - -r/--remote <连接名> → 打开远程面板（连接须已激活）；-d 指定远程目录
/// 复用 $HSSH_TOKEN 来源校验。
#[cfg(not(windows))]
const HFILE_SCRIPT: &str = r#"#!/bin/sh
# HetuShell 内建命令：在终端内打开文件管理器面板。
# 用法: hfile [选项] [-d <目录>]
#       hfile -r <连接名> [选项] [-d <远程目录>]
#       hfile -h, --help
# -w/--with-shell: 浮动覆盖层跟随当前终端分屏大小定位
# -d/--dir <路径>: 指定打开目录（默认：本地→终端cwd，远程→主目录）
# -r/--remote <连接名>: 打开远程文件面板（连接须已在 HetuShell 中激活）
OSC=1732
emit() { printf '\033]%s;%s\007' "$OSC" "$1"; }
b64() { printf '%s' "$1" | base64 | tr -d '\n'; }

usage() {
  cat <<'EOF'
用法: hfile [选项] [-d <目录>]
       hfile -r <连接名> [选项] [-d <远程目录>]

选项:
  -w, --with-shell    浮动覆盖层跟随当前终端分屏大小定位
  -d, --dir <路径>    指定打开目录（默认：本地→终端cwd，远程→主目录）
  -r, --remote <名称> 打开远程文件面板（连接须已激活）
  -h, --help          显示此帮助

不带 -r 时打开本地文件面板；-w 在当前终端上创建浮动覆盖层而非切换侧边面板。
EOF
}

[ $# -eq 0 ] && { emit "tok=$(b64 "${HSSH_TOKEN:-}");w=0;d=;r="; exit 0; }

_with_shell=0
_dir=""
_remote=""

while [ $# -gt 0 ]; do
  case "$1" in
    -h|--help)        usage; exit 0;;
    -w|--with-shell)  _with_shell=1; shift;;
    -d|--dir)         [ $# -ge 2 ] || { echo "hfile: 选项 $1 缺少参数" >&2; exit 1; }; _dir="$2"; shift 2;;
    -r|--remote)      [ $# -ge 2 ] || { echo "hfile: 选项 $1 缺少参数" >&2; exit 1; }; _remote="$2"; shift 2;;
    -*)               echo "hfile: 未知选项 $1" >&2; exit 1;;
    *)                echo "hfile: 多余参数 $1" >&2; exit 1;;
  esac
done

_payload="tok=$(b64 "${HSSH_TOKEN:-}");w=$_with_shell;d=$(b64 "$_dir");r=$(b64 "$_remote")"
emit "$_payload"
"#;

/// 把 hssh / hsshprod 脚本落地到 bin 目录并置可执行位，返回该目录用于前置 PATH。
/// 内容一致则不重写（避免每次 spawn 写盘）；任何失败都返回 None（不影响终端启动）。
#[cfg(not(windows))]
fn install_hssh() -> Option<std::path::PathBuf> {
    let dir = crate::settings::bin_dir().ok()?;
    for (name, content) in [("hssh", HSSH_SCRIPT), ("hsshprod", HSSHPROD_SCRIPT), ("hexit", HEXIT_SCRIPT), ("himage", HIMAGE_SCRIPT), ("hfile", HFILE_SCRIPT)] {
        let path = dir.join(name);
        let need = std::fs::read_to_string(&path)
            .map(|c| c != content)
            .unwrap_or(true);
        if need {
            // hssh 是核心命令，写入失败则整体放弃；其余辅助命令失败不影响 hssh
            if name == "hssh" {
                std::fs::write(&path, content).ok()?;
            } else {
                let _ = std::fs::write(&path, content);
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
        }
    }
    Some(dir)
}

/// 打开本地 PTY pane。控制线程处理输入/resize/关闭，读线程转发输出。
/// 返回本地 shell 的进程号（用于 /proc/<pid>/cwd 读实时工作目录）；取不到则 None。
pub fn open(
    app: AppHandle,
    pane_id: String,
    cols: u32,
    rows: u32,
    initial_cwd: Option<String>,
    hssh_token: String,
    mut rx: mpsc::UnboundedReceiver<PaneCmd>,
) -> Result<Option<u32>> {
    let pty = native_pty_system();
    let pair = pty
        .openpty(size(cols, rows))
        .map_err(|e| Error::msg(format!("创建本地 PTY 失败: {e}")))?;

    let shell = default_shell();
    let mut cmd = CommandBuilder::new(&shell);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    // 能力令牌注入本地 shell：hssh 读取 $HSSH_TOKEN 并回填到 OSC，前端据此校验来源，
    // 防止终端里被渲染的不可信内容（cat 恶意文件、ls 恶意文件名等）伪造序列诱导建连。
    cmd.env("HSSH_TOKEN", &hssh_token);
    // 注入内建 hssh 命令：把脚本所在 bin 目录「追加」到本地 shell PATH 末尾（仅 POSIX；
    // 远程连接不注入）。追加而非前置：系统同名可执行优先，杜绝 bin 被写入后劫持 sudo/ssh 等。
    // 安装失败则跳过——用户至多得到 command not found，绝不影响终端启动。
    #[cfg(not(windows))]
    if let Some(bin) = install_hssh() {
        let existing = std::env::var("PATH").unwrap_or_default();
        let bin = bin.to_string_lossy().into_owned();
        cmd.env(
            "PATH",
            if existing.is_empty() {
                bin
            } else {
                format!("{existing}:{bin}")
            },
        );
    }
    // 起始工作目录：优先用调用方传入的目录（新标签/分屏继承源终端 cwd），
    // 仅当它确为可用目录时采用；否则（未传或切换失败/目录不存在）回退用户主目录。
    let start_dir = initial_cwd
        .filter(|d| !d.is_empty() && std::path::Path::new(d).is_dir())
        .map(std::path::PathBuf::from)
        .or_else(dirs::home_dir);
    if let Some(dir) = start_dir {
        cmd.cwd(dir);
    }
    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| Error::msg(format!("启动本地 shell({shell}) 失败: {e}")))?;
    drop(pair.slave);
    // 在 child 被移入控制线程前取其 PID（供 /proc/<pid>/cwd 读实时工作目录）
    let pid = child.process_id();

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| Error::msg(format!("PTY reader 失败: {e}")))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| Error::msg(format!("PTY writer 失败: {e}")))?;
    let master = pair.master;

    // 主动关闭标志：区分「用户主动关闭/切换连接」与「shell 自己退出」。
    // 前者不应让前端把 pane 当作退出而关闭（否则切换连接会误关当前终端）。
    let deliberate = Arc::new(AtomicBool::new(false));

    // 读线程：PTY 输出 → 前端。EOF 时通知前端；exited 仅在 shell 自行退出时为 true。
    let app_out = app.clone();
    let pane_out = pane_id.clone();
    let deliberate_r = deliberate.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let _ = app_out.emit(
                        "pane-output",
                        serde_json::json!({
                            "paneId": pane_out,
                            "data": base64::engine::general_purpose::STANDARD.encode(&buf[..n]),
                        }),
                    );
                }
            }
        }
        let exited = !deliberate_r.load(Ordering::SeqCst);
        let _ = app_out.emit(
            "pane-closed",
            serde_json::json!({ "paneId": pane_out, "exited": exited }),
        );
    });

    // 控制线程：持有 master 与 child，串行处理指令（blocking_recv 不能在 async 上下文调用）
    std::thread::spawn(move || {
        loop {
            match rx.blocking_recv() {
                Some(PaneCmd::Data(d)) => {
                    if writer.write_all(&d).is_err() {
                        break;
                    }
                }
                Some(PaneCmd::Resize { cols, rows }) => {
                    let _ = master.resize(size(cols, rows));
                }
                // 主动关闭/发送端 drop：标记为非退出，随后让读线程 EOF 时据此上报
                Some(PaneCmd::Close) | None => {
                    deliberate.store(true, Ordering::SeqCst);
                    break;
                }
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        // master 随线程结束 drop，读线程随之 EOF 退出
    });

    Ok(pid)
}
