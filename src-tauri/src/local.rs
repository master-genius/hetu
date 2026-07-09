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
  -h, --help                  显示此帮助

示例:
  hssh prod
  hssh root@10.0.0.9 -p 2222 -w 'secret'
  hssh --prod claude --exec "ls -la; df -h"
  hssh --prod claude --file deploy.sh --exit
  echo "uptime; free -m" | hsshprod claude --stdin --exit
EOF
}

[ $# -eq 0 ] && { usage; exit 1; }
case "$1" in -h|--help) usage; exit 0;; esac

# prod 模式预扫描：--prod 作为首参数时消费它，后续参数同常规解析。
# --prod 也可出现在选项中（hssh claude --prod --exec "ls"），option loop 会再设一次。
prod_mode=0
if [ "$1" = "--prod" ]; then
  prod_mode=1
  shift
  [ $# -eq 0 ] && { echo "hssh: --prod 需要连接项名称或主机" >&2; exit 1; }
fi

first="$1"
case "$first" in -*) usage; exit 1;; esac
shift

host=""; user=""; port=""; pass=""; ident=""; name=""; adhoc=0
feed=""; feed_file=""; feed_stdin=0; do_exit=0
case "$first" in
  *@*) user="${first%@*}"; host="${first#*@}"; adhoc=1;;
  *)   name="$first";;
esac

# 选项取值前先确认还有参数，避免「选项作为末位参数缺值」时 shift 2 触发死循环/报错。
need() { [ "$1" -ge 2 ] || { echo "hssh: 选项 $2 缺少参数" >&2; exit 1; }; }
while [ $# -gt 0 ]; do
  case "$1" in
    --prod)        prod_mode=1; shift;;
    -p|--port)     need $# "$1"; port="$2"; adhoc=1; shift 2;;
    -u|--user)     need $# "$1"; user="$2"; adhoc=1; shift 2;;
    -w|--password) need $# "$1"; pass="$2"; adhoc=1; shift 2;;
    -i|--identity) need $# "$1"; ident="$2"; adhoc=1; shift 2;;
    -e|--exec)     [ "$prod_mode" = 1 ] || { echo "hssh: --exec 需要 --prod 模式" >&2; exit 1; }; need $# "$1"; feed="$2"; shift 2;;
    -f|--file)     [ "$prod_mode" = 1 ] || { echo "hssh: --file 需要 --prod 模式" >&2; exit 1; }; need $# "$1"; feed_file="$2"; shift 2;;
    -s|--stdin)    [ "$prod_mode" = 1 ] || { echo "hssh: --stdin 需要 --prod 模式" >&2; exit 1; }; feed_stdin=1; shift;;
    -x|--exit)     [ "$prod_mode" = 1 ] || { echo "hssh: --exit 需要 --prod 模式" >&2; exit 1; }; do_exit=1; shift;;
    -h|--help)     usage; exit 0;;
    -*)            echo "hssh: 未知选项 $1" >&2; exit 1;;
    *)             host="$1"; adhoc=1; shift;;
  esac
done

# 裸名 + 任一连接选项 → 视为临时连接到该主机（而非连接项名），兑现 `hssh 主机 -p/-w …`。
if [ "$adhoc" = 1 ] && [ -n "$name" ] && [ -z "$host" ]; then
  host="$name"; name=""
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
if [ -n "$name" ]; then
  emit "v=1;tok=$tok;mode=profile;name=$(b64 "$name");feed=$feed_b64;exit=$exit_b64"
  printf '→ 正在打开连接项「%s」…\n' "$name"
else
  [ -z "$host" ] && { echo 'hssh: 缺少主机' >&2; exit 1; }
  user="${user:-$(id -un)}"
  emit "v=1;tok=$tok;mode=adhoc;host=$(b64 "$host");user=$(b64 "$user");port=$(b64 "$port");pass=$(b64 "$pass");ident=$(b64 "$ident");feed=$feed_b64;exit=$exit_b64"
  printf '→ 正在连接 %s@%s …\n' "$user" "$host"
fi
"#;

/// hsshprod 快捷别名：等价于 `hssh --prod`，仅转发参数。
const HSSHPROD_SCRIPT: &str = r#"#!/bin/sh
exec hssh --prod "$@"
"#;

/// 把 hssh / hsshprod 脚本落地到 bin 目录并置可执行位，返回该目录用于前置 PATH。
/// 内容一致则不重写（避免每次 spawn 写盘）；任何失败都返回 None（不影响终端启动）。
#[cfg(not(windows))]
fn install_hssh() -> Option<std::path::PathBuf> {
    let dir = crate::settings::bin_dir().ok()?;
    for (name, content) in [("hssh", HSSH_SCRIPT), ("hsshprod", HSSHPROD_SCRIPT)] {
        let path = dir.join(name);
        let need = std::fs::read_to_string(&path)
            .map(|c| c != content)
            .unwrap_or(true);
        if need {
            // hsshprod 安装失败不影响 hssh（hssh 是核心命令，hsshprod 只是别名）
            if name == "hsshprod" {
                let _ = std::fs::write(&path, content);
            } else {
                std::fs::write(&path, content).ok()?;
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
