//! hsshx — HetuShell SSH exec + 文件传输 CLI
//! 复用 hetushell_lib 的 SSH/SFTP 栈，按 profiles.json 连接项执行远程命令与文件搬运。
//!
//! 命令类（三种）：
//! - exec 模式：hsshx <连接项> <命令...>  → 执行命令，纯净输出，退出码跟随远程命令
//! - file 模式：hsshx <连接项> -f <文件> → 从文件读取命令执行
//! - 交互模式：hsshx <连接项>            → 进入远程 shell，退出即回本地 shell
//!
//! 文件类（五种，搬运循环见 ssh::xfer）：
//! - hsshx <连接项> --get <远端> [本地]      下载文件或目录
//! - hsshx <连接项> --put <本地> <远端目录>  上传文件或目录
//! - hsshx <连接项> --cp <远端源> <目标>     远端复制（目标带「连接项:」则跨机）
//! - hsshx <连接项> --ls [远端目录]          列远端目录
//! - hsshx <连接项> --stat <远端路径>        远端元数据 JSON
//!
//! stdin 管道有数据时自动读取为命令（exec 模式），与无命令+终端场景区分。
//! stdout 只放结果数据，进度与错误一律走 stderr——脚本可用 2>/dev/null 静音。

use std::io::{IsTerminal, Read, Write};
use std::process::ExitCode;
use std::sync::Arc;

use russh::{ChannelMsg, Disconnect};
use tokio::sync::mpsc;
use uuid::Uuid;

use hetushell_lib::error::Error;
use hetushell_lib::settings::{load_profiles, Profile};
use hetushell_lib::ssh::conn::{establish, valid_tag, ConnParams, Connection};
use hetushell_lib::ssh::lease::{self, Holder};
use hetushell_lib::ssh::sftp;
use hetushell_lib::ssh::xfer::{self, Cancel};
use hetushell_lib::{diag, dprintln};

/// 同步入口：唯一的作用是在**任何异步运行时建立之前**固化 fd 3 的可用状态。
/// `#[tokio::main]` 展开后会先建运行时再跑函数体，而运行时会占用低位 fd——
/// 实测后果是调用方用 `3>&-` 关掉 fd 3 时，`fcntl(3)` 仍探到「已打开」（那是运行时自己的），
/// `--machine` 于是把诊断静默写进运行时内部，等于给了个假承诺。
fn main() -> ExitCode {
    diag::capture_machine_fd();
    real_main()
}

#[tokio::main]
async fn real_main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();

    if args.len() == 2 && (args[1] == "-l" || args[1] == "--list") {
        return list_profiles();
    }

    // -h 可在任意位置（沿用原行为：解析中途遇到即打印帮助并成功退出）
    if args.iter().skip(1).any(|a| a == "-h" || a == "--help") {
        usage();
        return ExitCode::SUCCESS;
    }

    if args.iter().skip(1).any(|a| a == "--doc" || a == "--docs") {
        print_doc();
        return ExitCode::SUCCESS;
    }

    // 免连接的本地子命令：只读写租约目录，不开 SSH、不查 profiles。
    // 放在连接项解析之前，否则 listconn/killconn 会被当成连接项名去找配置。
    if args.get(1).map(String::as_str) == Some("listconn") {
        return list_connections(&args[2..]);
    }
    if args.get(1).map(String::as_str) == Some("killconn") {
        return kill_connection(&args[2..]);
    }

    if args.len() < 2 {
        usage();
        return ExitCode::from(2);
    }

    let spec = match parse_args(&args) {
        Ok(s) => s,
        Err(e) => {
            dprintln!("hsshx: {e}");
            usage();
            return ExitCode::from(2);
        }
    };

    // `--machine` 的接入点必须排在任何诊断输出之前——之后所有 `dprintln!` 才谈得上改道。
    // 这里刻意用 eprintln!：登记失败（fd 3 没打开／非 Unix）意味着第三条流并不存在，
    // 只能报在 stderr；此时若也写进 fd 3 就是往没打开的 fd 上写，等于把错误弄丢。
    if spec.machine {
        if let Err(e) = diag::adopt() {
            eprintln!("hsshx: {e}");
            return ExitCode::from(2);
        }
    }

    let profile = match find_profile(&spec.name) {
        Some(p) => p,
        None => {
            dprintln!("hsshx: 未找到连接项「{}」", spec.name);
            return ExitCode::from(2);
        }
    };

    match spec.mode {
        Mode::Exec { command } => run_exec(profile, spec.by, command).await,
        Mode::Job(job) => run_job(job, spec.force, spec.mkdir, spec.by, profile).await,
    }
}

/// 一次调用的解析结果。
#[derive(Debug)]
struct Spec {
    name: String,
    force: bool,
    /// 发起方标记（`--by <tag>`）。None = 人在终端手敲，listconn 按「user」展示。
    by: Option<String>,
    /// `-p/--mkdir`：`--put` 的远端目录缺失时逐级创建。默认 false = 沿用「必须已存在」。
    mkdir: bool,
    /// `--machine`：把 hsshx 自身的诊断改投调用方打开的 fd 3，于是 stdout/stderr 只装
    /// 远端命令的两条流。默认 false = 诊断照旧走 stderr（人的手动用法逐字节不变）。
    machine: bool,
    mode: Mode,
}

#[derive(Debug)]
enum Mode {
    /// 原有路径：命令来自 `-f` 文件 / 位置参数 / stdin 管道；None = 交互模式
    Exec { command: Option<String> },
    /// 文件搬运与远端探查
    Job(Job),
}

#[derive(Debug)]
enum Job {
    Get { remote: String, local: String },
    Put { local: String, remote_dir: String },
    Cp { src: String, dst: String },
    Ls { path: Option<String> },
    Stat { path: String },
}

fn is_job_flag(a: &str) -> bool {
    matches!(a, "--get" | "--put" | "--cp" | "--ls" | "--stat")
}

/// hsshx 自有选项。出现在操作数位置时**不能**当路径取走——
/// 否则 `--get --force /x` 会把 `--force` 静默当成远端路径发给服务器。
fn is_hsshx_option(a: &str) -> bool {
    is_job_flag(a)
        || matches!(
            a,
            "--force"
                | "--by"
                | "-p"
                | "--mkdir"
                | "-f"
                | "--file"
                | "-n"
                | "--name"
                | "--machine"
        )
}

/// `--by <tag>` 取值：复用后端唯一实现点 `ssh::conn::valid_tag`，在此只加 CLI 前缀。
/// 两处各写一份规则必然漂移——后端 ssh_connect 也要校验（前端可传任意值）。
fn cli_tag(raw: &str) -> Result<String, String> {
    valid_tag(raw).map_err(|e| format!("选项 --by {e}"))
}

/// 解析 `<连接项>` 及其后的参数。
///
/// 兼容性规则：
/// - 第一个未被选项消费的参数恒为连接项名（与原来一致，未知 `-x` 仍视作名字）；
/// - 所有 hsshx 自有选项（`-f`/`--force`/传输类）只在 `flags_open` 时识别——即远程命令
///   的第一个位置参数一旦出现，其后全部按原样交给远程命令，不会被选项解析吞掉；
/// - `-f/--file` 在连接项名之后仍然可用（原 `parsing_flags` 语义：置 false 只发生在收集
///   到第二个位置参数时，故 `hsshx web -f s.sh` 成立）；
/// - 传输类选项要求已见过连接项名，命中后按该操作的元数取走操作数，不再拼进远程命令。
fn parse_args(args: &[String]) -> Result<Spec, String> {
    let mut name: Option<String> = None;
    let mut file_path: Option<String> = None;
    let mut cmd_parts: Vec<String> = Vec::new();
    let mut job: Option<Job> = None;
    let mut force = false;
    let mut by: Option<String> = None;
    let mut mkdir = false;
    let mut machine = false;
    let mut flags_open = true;

    let mut i = 1;
    while i < args.len() {
        let arg = args[i].as_str();

        if flags_open && arg == "--force" {
            force = true;
            i += 1;
            continue;
        }

        // --machine：hsshx 自身的话改投调用方打开的 fd 3，于是 stdout/stderr 只剩远端两条流。
        // 纯开关、不取值，故也刻意不关 flags_open：`hsshx web --machine --ls /tmp` 仍要认得 --ls。
        if flags_open && arg == "--machine" {
            machine = true;
            i += 1;
            continue;
        }

        if flags_open && arg == "--by" {
            let Some(v) = args.get(i + 1) else {
                return Err(format!("选项 {arg} 缺少参数"));
            };
            by = Some(cli_tag(v)?);
            i += 2;
            continue;
        }

        if flags_open && (arg == "-p" || arg == "--mkdir") {
            mkdir = true;
            i += 1;
            continue;
        }

        // -n/--name：显式指定连接项，与 hssh 的 -n 同一取向——给自动化一条不受位置参数语义
        // 影响的精确入口。刻意不关 flags_open：`hsshx -n cy2 --put a b` 仍要认得传输选项；
        // 而之后的裸参数因 name 已是 Some 会自然落进远程命令，正合 `hsshx -n cy2 ls -la`。
        if flags_open && (arg == "-n" || arg == "--name") {
            let Some(v) = args.get(i + 1) else {
                return Err(format!("选项 {arg} 缺少参数"));
            };
            if let Some(cur) = &name {
                return Err(format!("连接项已由位置参数给出「{cur}」，与 -n {v} 冲突（二选一）"));
            }
            name = Some(v.to_string());
            i += 2;
            continue;
        }

        if flags_open && name.is_some() && is_job_flag(arg) {
            i += 1;
            job = Some(take_job(arg, args, &mut i)?);
            continue;
        }

        if job.is_some() {
            return Err(format!("文件传输模式不接受多余参数「{arg}」"));
        }

        if flags_open && (arg == "-f" || arg == "--file") {
            if i + 1 >= args.len() {
                return Err(format!("选项 {arg} 缺少参数"));
            }
            file_path = Some(args[i + 1].clone());
            i += 2;
            continue;
        }

        match &mut name {
            None => name = Some(arg.to_string()),
            Some(_) => {
                flags_open = false;
                cmd_parts.push(arg.to_string());
            }
        }
        i += 1;
    }

    let name = name.ok_or("缺少连接项名（hsshx -l 查看已保存的连接项）")?;
    // 必须在 resolve_command 之前判定：后者会读 stdin，误用时报错不该先卡在读输入上
    if force && job.is_none() {
        return Err("--force 仅对 --get/--put/--cp 有意义（当前没有传输操作）".to_string());
    }
    // 同上：-p 只对 --put 有意义。--get 的落点是本地路径（由调用方自己决定），
    // --ls/--stat 是只读探查，静默收下这个开关会让用户以为目录被建好了。
    if mkdir && !matches!(job, Some(Job::Put { .. })) {
        return Err("-p 仅对 --put 有意义（当前不是 --put 操作）".to_string());
    }
    let mode = match job {
        Some(j) => Mode::Job(j),
        None => Mode::Exec {
            command: resolve_command(file_path, cmd_parts)?,
        },
    };
    Ok(Spec {
        name,
        force,
        by,
        mkdir,
        machine,
        mode,
    })
}

/// 从 `i` 处按 `flag` 的元数取走操作数。可选操作数有默认值：
/// `--get` 的落点默认当前目录，`--ls` 的路径默认远端 home。
fn take_job(flag: &str, args: &[String], i: &mut usize) -> Result<Job, String> {
    let mut take = || match args.get(*i) {
        Some(a) if is_hsshx_option(a) => None,
        Some(a) => {
            *i += 1;
            Some(a.clone())
        }
        None => None,
    };
    Ok(match flag {
        "--get" => Job::Get {
            remote: take().ok_or("--get 缺少远端路径")?,
            local: take().unwrap_or_else(|| ".".to_string()),
        },
        "--put" => Job::Put {
            local: take().ok_or("--put 缺少本地路径")?,
            remote_dir: take().ok_or("--put 缺少远端目录")?,
        },
        "--cp" => Job::Cp {
            src: take().ok_or("--cp 缺少源路径")?,
            dst: take().ok_or("--cp 缺少目标路径")?,
        },
        "--ls" => Job::Ls { path: take() },
        "--stat" => Job::Stat {
            path: take().ok_or("--stat 缺少路径")?,
        },
        other => return Err(format!("未知选项 {other}")),
    })
}

/// 命令来源优先级：`-f` 文件 > 位置参数 > stdin 管道 > 交互模式（None）。
fn resolve_command(file_path: Option<String>, cmd_parts: Vec<String>) -> Result<Option<String>, String> {
    if let Some(path) = file_path {
        return std::fs::read_to_string(&path)
            .map(Some)
            .map_err(|e| format!("读取文件失败: {e}"));
    }
    if !cmd_parts.is_empty() {
        return Ok(Some(cmd_parts.join(" ")));
    }
    if !std::io::stdin().is_terminal() {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|_| "读取 stdin 失败".to_string())?;
        return Ok(Some(buf));
    }
    Ok(None)
}

fn find_profile(name: &str) -> Option<Profile> {
    load_profiles().into_iter().find(|p| p.name == name)
}

fn conn_params(profile: &Profile, by: Option<String>) -> ConnParams {
    ConnParams {
        name: profile.name.clone(),
        host: profile.host.clone(),
        port: profile.port,
        user: profile.user.clone(),
        auth: profile.auth.clone(),
        password: profile.password.clone(),
        key_path: profile.key_path.clone(),
        key_data: profile.key_data.clone(),
        passphrase: None,
        keepalive: profile.keepalive,
        timeout: profile.timeout,
        by,
        // hsshx 是独立进程，没有 pane 归属概念：它的终止授权凭据是租约里的 pid（见 ssh::lease）
        origin_pane: None,
    }
}

/// 原有执行路径：exec 模式或交互模式，退出码跟随远程命令。
async fn run_exec(profile: Profile, by: Option<String>, command: Option<String>) -> ExitCode {
    if let Some(cmd) = &command {
        if cmd.trim().is_empty() {
            return ExitCode::SUCCESS;
        }
    }

    let params = conn_params(&profile, by.clone());
    let handle = match establish(&params).await {
        Ok(h) => h,
        Err(e) => {
            dprintln!("hsshx: {e}");
            return ExitCode::from(255);
        }
    };

    // 只给交互模式登记租约：exec 是一次性命令、秒级完成，登记只会带来锁文件高频创删
    // 与残留竞态，却没有「去看/去杀」的对象。
    let _lease = command
        .is_none()
        .then(|| register_lease(&profile, by, "interactive", String::new()))
        .flatten();

    let code = match command {
        Some(cmd) => exec_mode(&handle, cmd).await,
        None => interactive_mode(&handle).await,
    };

    let _ = handle
        .disconnect(Disconnect::ByApplication, "bye", "zh")
        .await;

    ExitCode::from(code)
}

/// `hsshx listconn [--json]`：只列 **CLI 侧**活跃连接（读租约目录），免连接、纯本地。
///
/// 与 `hssh listconn` 的差别必须说清：后者由 GUI 应答，能同时看到面板建的连接；本命令
/// 读不到别的进程的内存，只覆盖 hsshx 自己登记的租约。好处是不依赖宿主在跑、stdout 直接
/// 可被脚本与 AI 捕获。
fn list_connections(rest: &[String]) -> ExitCode {
    let leases = match lease::scan() {
        Ok(v) => v,
        Err(e) => {
            dprintln!("hsshx: {e}");
            return ExitCode::from(1);
        }
    };
    // 本进程的 pane 归属：与 GUI 侧共用 lease::kill_allowed 的同一条「本 pane 直杀」判据，
    // 不在这里复制规则；读不到 $HSSH_PANE 时为 None，判据①自然不成立。
    let own = self_pane();
    if rest.iter().any(|a| a == "--json") {
        // 每行附带 killable：授权规则只在 lease::kill_allowed 一处，不让调用方自己复制
        let rows: Vec<serde_json::Value> = leases
            .iter()
            .map(|l| {
                let mut v = serde_json::to_value(l).unwrap_or(serde_json::Value::Null);
                if let Some(o) = v.as_object_mut() {
                    o.insert(
                        "killable".into(),
                        serde_json::json!(lease::kill_allowed(&lease::KillCtx {
                            by: &l.by,
                            origin_pane: l.pane.as_deref(),
                            panes: &[],
                            req_pane: own.as_deref(),
                            claim: None,
                            force: false,
                        })),
                    );
                }
                v
            })
            .collect();
        match serde_json::to_string_pretty(&rows) {
            Ok(s) => {
                println!("{s}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                dprintln!("hsshx: 序列化失败: {e}");
                ExitCode::from(1)
            }
        }
    } else {
        if leases.is_empty() {
            println!("（没有由 hsshx 发起的活跃连接；GUI 面板建的连接请用 hssh listconn 查看）");
            return ExitCode::SUCCESS;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        println!(
            "{}  {}  {}  {}  {}  {}",
            pad("发起", 10),
            pad("连接项", 10),
            pad("主机", 22),
            pad("用户", 8),
            pad("ID", 8),
            "状态 / 详情"
        );
        for l in &leases {
            // 全部字段过 sanitize：它们源自 profiles.json 与命令行，含 ESC 会注入宿主终端
            let can = lease::kill_allowed(&lease::KillCtx {
                by: &l.by,
                origin_pane: l.pane.as_deref(),
                panes: &[],
                req_pane: own.as_deref(),
                claim: None,
                force: false,
            });
            let mut tail = format!("{} {}m", l.kind, now.saturating_sub(l.started_unix) / 60);
            if !l.detail.is_empty() {
                tail.push_str(&format!("  {}", lease::sanitize(&l.detail)));
            }
            if !can {
                tail.push_str(&format!("  [需 {}]", lease::kill_hint(&l.by)));
            }
            println!(
                "{}  {}  {}  {}  {}  {}",
                pad(lease::by_label(&l.by), 10),
                pad(&lease::sanitize(&l.profile), 10),
                pad(&format!("{}:{}", lease::sanitize(&l.host), l.port), 22),
                pad(&lease::sanitize(&l.user), 8),
                pad(lease::short_id(&l.id), 8),
                tail,
            );
        }
        ExitCode::SUCCESS
    }
}

/// `hsshx killconn <id>`：终止一条由 hsshx 发起的传输（发 SIGINT，等价于用户按 Ctrl-C）。
///
/// 三道约束：
/// 1. 目标必须来自 `lease::scan()`（锁仍被持有 = 那个 pid 就是当时登记人），否则残留条目里
///    被复用的 pid 会让信号打到无关进程上。
/// 2. 授权：`lease::kill_allowed` 只放行打了 `--by` 的租约。CLI 侧没有 pane 归属概念，
///    所以「本 pane 发起」这条判据在这里恒不成立——用户手敲的会话不得被随手杀。
/// 3. 发 SIGINT 而非 SIGTERM/KILL：交互式有 termios 恢复处理、传输式有 Cancel+回滚，
///    两条路径都能干净收尾并自删租约。
///
/// 本进程是一次性 CLI（无并发任务），故用同步 thread::sleep 做有界等待，不引入 async。
fn kill_connection(rest: &[String]) -> ExitCode {
    // 与 GUI 侧同一条规则，所以这里也要收集 --by（声明目标的发起标记）与 --force；
    // 选项可出现在 id 前后，第一个非选项参数才是 id。
    let mut target: Option<&str> = None;
    let mut claim: Option<String> = None;
    let mut force = false;
    let mut idx = 0;
    while idx < rest.len() {
        let a = rest[idx].as_str();
        match a {
            "--force" => force = true,
            "--by" => match rest.get(idx + 1) {
                Some(v) => {
                    match cli_tag(v) {
                        Ok(t) => claim = Some(t),
                        Err(e) => {
                            dprintln!("hsshx: {e}");
                            return ExitCode::from(2);
                        }
                    }
                    idx += 1; // --by 的值不再当 id 看
                }
                None => {
                    dprintln!("hsshx: 选项 --by 缺少参数");
                    return ExitCode::from(2);
                }
            },
            _ if a.starts_with('-') => {
                dprintln!("hsshx: killconn 收到未知选项 {a}");
                return ExitCode::from(2);
            }
            _ => {
                if target.is_some() {
                    dprintln!("hsshx: killconn 只接受一个目标 id，多余参数 {a}");
                    return ExitCode::from(2);
                }
                target = Some(a);
            }
        }
        idx += 1;
    }
    let Some(target) = target else {
        dprintln!("hsshx: killconn 需要目标 id（hsshx listconn 查看）");
        return ExitCode::from(2);
    };
    let l = match lease::find_kill_target(target) {
        Ok(v) => v,
        Err(e) => {
            dprintln!("hsshx: {e}");
            return ExitCode::from(1);
        }
    };
    // 同一条规则（lease::kill_allowed）：本 pane 持有免确认，其余要 --force 且 --by 与目标标记一致
    let own = self_pane();
    if !lease::kill_allowed(&lease::KillCtx {
        by: &l.by,
        origin_pane: l.pane.as_deref(),
        panes: &[],
        req_pane: own.as_deref(),
        claim: claim.as_deref(),
        force,
    }) {
        dprintln!(
            "hsshx: 拒绝终止 {}（{}）：它不是本终端发起的连接。确认要强杀请补 {}",
            lease::short_id(&l.id),
            l.profile,
            lease::kill_hint(&l.by)
        );
        return ExitCode::from(1);
    }
    if let Err(e) = lease::signal_int(l.pid) {
        dprintln!("hsshx: {e}");
        return ExitCode::from(1);
    }
    dprintln!(
        "hsshx: 已向 {}（{} pid {}）发 SIGINT",
        lease::short_id(&l.id),
        l.kind,
        l.pid
    );
    // 有界确认：对方退出前租约不会消失。等不到不等于失败（回滚可能卡在已停滞的网络上），
    // 所以文案要区分「已终止」与「已发信号但仍在收尾」。
    // 用 still_alive 而非每轮全量 scan()：scan 会顺带回收别的僵尸条目，让等待用的只读轮询
    // 带上写副作用，也做了多余的事。
    for _ in 0..100 {
        if !lease::still_alive(&l.id) {
            println!("已终止 {}", lease::short_id(&l.id));
            return ExitCode::SUCCESS;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    dprintln!(
        "hsshx: 租约仍在（{}）——对方可能在等网络超时做回滚清理，可用 listconn 再确认",
        lease::short_id(&l.id)
    );
    ExitCode::from(1)
}

fn pad(v: &str, n: usize) -> String {
    let t = v.trim_end();
    let len = t.chars().count();
    if len >= n {
        t.to_string()
    } else {
        format!("{t}{}", " ".repeat(n - len))
    }
}

/// 本 hsshx 进程所在的 HetuShell pane（后端注入的 `$HSSH_PANE`）。
/// 不在 HetuShell 本地终端里时为 None —— 此时「本 pane 直杀」这条判据自动不成立，
/// 只剩 `--by` + `--force` 一条路，不存在「读不到就当放行」的放宽。
fn self_pane() -> Option<String> {
    std::env::var("HSSH_PANE").ok().filter(|v| !v.is_empty())
}

/// 登记跨进程租约，让 GUI 侧的 `hssh listconn` 能看见并终止这条 CLI 连接。
///
/// **登记失败绝不阻断本次操作**：只 warn 到 stderr 并返回 None。租约是可观测性附加层，
/// 传输成败不该取决于它（与 `install_hssh` 的「安装失败即跳过」同一取向）。
/// 返回的 `Holder` 靠调用方持有到函数结束，drop 时自动释放 flock 并清理文件。
fn register_lease(
    profile: &Profile,
    by: Option<String>,
    kind: &str,
    detail: String,
) -> Option<Holder> {
    let mut lease = lease::Lease {
        id: Uuid::new_v4().to_string(),
        pid: 0,
        kind: kind.to_string(),
        profile: profile.name.clone(),
        host: profile.host.clone(),
        port: profile.port,
        user: profile.user.clone(),
        by,
        pane: self_pane(),
        detail,
        started_unix: 0,
    };
    match lease::acquire(&mut lease) {
        Ok(h) => Some(h),
        Err(e) => {
            dprintln!("hsshx: 租约登记失败（不影响本次操作）: {e}");
            None
        }
    }
}

/// 传输类操作的租约摘要（`--ls/--stat` 是瞬时探查，按约定不登记租约）。
fn job_lease_kind(job: &Job) -> Option<(&'static str, String)> {
    match job {
        Job::Get { remote, local } => Some(("get", format!("--get {remote} → {local}"))),
        Job::Put { local, remote_dir } => Some(("put", format!("--put {local} → {remote_dir}"))),
        Job::Cp { src, dst } => Some(("cp", format!("--cp {src} → {dst}"))),
        Job::Ls { .. } | Job::Stat { .. } => None,
    }
}

/// 文件搬运路径：组装 `Arc<Connection>`（`ssh::sftp` 的 pub 原语按连接缓存懒创建的
/// SFTP 会话，故必须走 Connection 而非裸 handle），再分发到具体操作。
async fn run_job(
    job: Job,
    force: bool,
    mkdir: bool,
    by: Option<String>,
    profile: Profile,
) -> ExitCode {
    let cancel = Cancel::new();
    watch_cancel(&cancel);

    let src_conn = match attach(conn_params(&profile, by.clone())).await {
        Ok(c) => c,
        Err(e) => {
            dprintln!("hsshx: {e}");
            return ExitCode::from(255);
        }
    };

    // 传输进行中才可被 listconn 看见并终止（Ctrl-C 走的是既有的 Cancel + 回滚路径）
    let _lease = job_lease_kind(&job)
        .and_then(|(kind, detail)| register_lease(&profile, by.clone(), kind, detail));

    // 跨机 --cp 的第二条连接在此建立、留在外层：这样无论成功还是失败，
    // 结束时都能对两条连接统一优雅 disconnect（此前它在外层块的分支里被 drop，
    // 只有 TCP 关闭、没有 SSH 层的告别）。同机（目标无前缀）时为 None，复用源连接。
    let profiles = load_profiles();
    let dst_conn: Option<Arc<Connection>> = match &job {
        Job::Cp { dst, .. } => match split_dst(dst, &profiles).0 {
            None => None,
            Some(n) => match find_profile(&n) {
                None => {
                    dprintln!("hsshx: 未找到连接项「{n}」");
                    close(&src_conn).await;
                    return ExitCode::from(2);
                }
                Some(p) => match attach(conn_params(&p, by.clone())).await {
                    Ok(c) => Some(c),
                    Err(e) => {
                        dprintln!("hsshx: {e}");
                        close(&src_conn).await;
                        return ExitCode::from(255);
                    }
                },
            },
        },
        _ => None,
    };

    let outcome: Result<(), Error> = async {
        match &job {
            Job::Get { remote, local } => {
                xfer::get(&src_conn, remote, local, force, &cancel).await
            }
            Job::Put { local, remote_dir } => {
                // -p 在 put 之前逐级建目录：put 自身的探测因此通过，不需要改它的签名
                // （put 有 14 处调用点，含 tests/xfer_e2e.rs 的 13 处）。
                if mkdir {
                    xfer::mkdir_p(&src_conn, remote_dir).await?;
                }
                let root = xfer::put(&src_conn, local, remote_dir, force, &cancel).await?;
                // 结果路径走 stderr：stdout 纯净是 hsshx 面向脚本/AI 的既有契约
                dprintln!("→ {root}");
                Ok(())
            }
            Job::Cp { src, dst } => {
                let (_, dst_path) = split_dst(dst, &profiles);
                let dst_conn = dst_conn.as_ref().unwrap_or(&src_conn);
                let root = xfer::cp(&src_conn, dst_conn, src, dst_path, force, &cancel).await?;
                dprintln!("→ {root}");
                Ok(())
            }
            Job::Ls { path } => {
                let text = list_dir(&src_conn, path.as_deref()).await?;
                print!("{text}");
                Ok(())
            }
            Job::Stat { path } => {
                let meta = sftp::stat(&src_conn, path).await?;
                let json = serde_json::to_string_pretty(&meta)
                    .map_err(|e| Error::msg(format!("序列化失败: {e}")))?;
                println!("{json}");
                Ok(())
            }
        }
    }
    .await;

    close(&src_conn).await;
    if let Some(conn) = &dst_conn {
        close(conn).await;
    }

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            dprintln!("hsshx: {e}");
            // 130 = 128+SIGINT，Unix 惯例：让脚本能把「人为中断」和「传输失败」分开处理
            match e {
                Error::Cancelled => ExitCode::from(130),
                _ => ExitCode::from(1),
            }
        }
    }
}

/// `--cp` 目标解析：`<连接项>:<路径>` → 跨机复制；无前缀 → 目标即源连接（同机 `cp -a`）。
/// 只在冒号前缀命中已保存连接项时才拆分，避免把 `/tmp/a:b` 这类含冒号的真实路径切坏。
/// 连接项表由调用方一次载入并传入——避免每个目标都重读一遍 profiles.json。
fn split_dst<'a>(dst: &'a str, profiles: &[Profile]) -> (Option<String>, &'a str) {
    match dst.split_once(':') {
        Some((head, rest)) if !head.is_empty() && profiles.iter().any(|p| p.name == head) => {
            (Some(head.to_string()), rest)
        }
        _ => (None, dst),
    }
}

/// 把已认证的 handle 装进 `Connection`。CLI 不做断线重连（`trigger_reconnect` 需要
/// AppHandle 且会起后台任务，与一次性命令的语义相悖），故 auto_reconnect=false。
async fn attach(params: ConnParams) -> Result<Arc<Connection>, Error> {
    let handle = establish(&params).await?;
    let conn = Arc::new(Connection::new(Uuid::new_v4().to_string(), params, false));
    *conn.handle.lock().await = Some(handle);
    Ok(conn)
}

async fn close(conn: &Arc<Connection>) {
    if let Some(handle) = conn.handle.lock().await.as_ref() {
        let _ = handle.disconnect(Disconnect::ByApplication, "bye", "zh").await;
    }
}

/// Ctrl-C → 置位取消标志，搬运循环在块边界退出并清理半成品。
/// 第二次 Ctrl-C 直接退出：清理本身可能卡在已停滞的网络上（那正是用户按 Ctrl-C 的
/// 典型场景），不该剥夺用户不再等待的权利。
fn watch_cancel(cancel: &Cancel) {
    let cancel = cancel.clone();
    tokio::spawn(async move {
        loop {
            if tokio::signal::ctrl_c().await.is_err() {
                break;
            }
            if cancel.cancelled() {
                std::process::exit(130);
            }
            cancel.trigger();
        }
    });
}

/// 列远端目录（`--ls`）。输出走 stdout——和 exec 模式一样，这里的数据本身就是结果。
/// 完整元数据（权限/属主/mtime）请用 `--stat`，故此处只给定位所需的「大小 + 名字」。
async fn list_dir(conn: &Arc<Connection>, path: Option<&str>) -> Result<String, Error> {
    let dir = match path {
        Some(p) => p.to_string(),
        None => sftp::home(conn).await?,
    };
    let entries = sftp::list(conn, &dir).await?;
    let mut out = String::new();
    for e in &entries {
        let suffix = if e.is_dir {
            "/"
        } else if e.is_link {
            "@"
        } else {
            ""
        };
        out.push_str(&format!("{:>12} {}{suffix}\n", e.size, e.name));
    }
    Ok(out)
}

/// exec 模式：非交互执行命令，纯净输出，退出码跟随远程命令。
/// stdin 管道数据自动转发到远端 channel（Unix 管道哲学），读完发 EOF；
/// 终端 stdin 不转发，直接发 EOF 防止读 stdin 的命令挂死。
async fn exec_mode(
    handle: &russh::client::Handle<hetushell_lib::ssh::conn::ClientHandler>,
    command: String,
) -> u8 {
    let channel = match handle.channel_open_session().await {
        Ok(c) => c,
        Err(e) => {
            dprintln!("hsshx: {e}");
            return 255;
        }
    };

    // 通过 login shell 执行，确保 /etc/profile、~/.bash_profile、~/.profile 被加载，
    // 否则 conda/nvm/自定义 PATH 等环境变量在非交互非登录 shell 中不可用。
    let quoted = format!("'{}'", command.replace('\'', "'\\''"));
    let full_command = format!("bash --login -c {quoted}");

    if let Err(e) = channel.exec(true, full_command.as_str()).await {
        dprintln!("hsshx: {e}");
        return 255;
    }

    // stdin 管道转发：管道数据写入 channel，读完发 EOF。
    // 终端 stdin 不转发（exec 模式非交互），直接发 EOF 防止读 stdin 的命令挂死。
    let (mut read_half, write_half) = channel.split();
    let (write_tx, mut write_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    if !std::io::stdin().is_terminal() {
        tokio::task::spawn_blocking(move || {
            let mut stdin = std::io::stdin().lock();
            let mut buf = [0u8; 4096];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if write_tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            // write_tx 在此 drop → write_rx 收到 None → 写 task 发 EOF
        });
    } else {
        drop(write_tx);
    }

    // 写 task：stdin 数据 → channel，stdin 读完 → EOF
    tokio::spawn(async move {
        while let Some(data) = write_rx.recv().await {
            if write_half.data_bytes(data).await.is_err() {
                break;
            }
        }
        let _ = write_half.eof().await;
    });

    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    let mut code: u8 = 0;

    while let Some(msg) = read_half.wait().await {
        match msg {
            ChannelMsg::Data { ref data } => {
                let _ = stdout.write_all(data);
                let _ = stdout.flush();
            }
            ChannelMsg::ExtendedData { ref data, .. } => {
                let _ = stderr.write_all(data);
                let _ = stderr.flush();
            }
            ChannelMsg::ExitStatus { exit_status } => {
                code = (exit_status & 0xFF) as u8;
            }
            _ => {}
        }
    }

    code
}

/// 交互模式：请求 PTY + shell，双向 relay I/O，支持终端 resize。
/// 退出时恢复终端属性，退出码跟随远程 shell。
#[cfg(unix)]
async fn interactive_mode(
    handle: &russh::client::Handle<hetushell_lib::ssh::conn::ClientHandler>,
) -> u8 {
    use std::os::fd::AsRawFd;

    let stdin_fd = std::io::stdin().as_raw_fd();
    let (cols, rows) = term_size();

    let orig_termios = match enable_raw_mode(stdin_fd) {
        Ok(t) => t,
        Err(e) => {
            dprintln!("hsshx: 无法设置终端 raw 模式: {e}");
            return 255;
        }
    };

    let channel = match handle.channel_open_session().await {
        Ok(c) => c,
        Err(e) => {
            dprintln!("hsshx: {e}");
            restore_termios(stdin_fd, &orig_termios);
            return 255;
        }
    };

    if let Err(e) = channel
        .request_pty(false, "xterm-256color", cols, rows, 0, 0, &[])
        .await
    {
        dprintln!("hsshx: 请求 PTY 失败: {e}");
        restore_termios(stdin_fd, &orig_termios);
        return 255;
    }

    if let Err(e) = channel.request_shell(false).await {
        dprintln!("hsshx: 请求 shell 失败: {e}");
        restore_termios(stdin_fd, &orig_termios);
        return 255;
    }

    let (mut read_half, write_half) = channel.split();

    enum WriteCmd {
        Data(Vec<u8>),
        Resize(u32, u32),
    }
    let (write_tx, mut write_rx) = mpsc::unbounded_channel::<WriteCmd>();

    // stdin 读取（阻塞线程，raw mode 下逐字节读）
    let write_tx_stdin = write_tx.clone();
    tokio::task::spawn_blocking(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if write_tx_stdin
                        .send(WriteCmd::Data(buf[..n].to_vec()))
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // SIGWINCH → window_change
    let write_tx_sig = write_tx.clone();
    tokio::spawn(async move {
        let mut sig = match tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::window_change(),
        ) {
            Ok(s) => s,
            Err(_) => return,
        };
        while sig.recv().await.is_some() {
            let (c, r) = term_size();
            let _ = write_tx_sig.send(WriteCmd::Resize(c, r));
        }
    });

    // 写 task：独占 write_half
    tokio::spawn(async move {
        while let Some(cmd) = write_rx.recv().await {
            match cmd {
                WriteCmd::Data(d) => {
                    if write_half.data_bytes(d).await.is_err() {
                        break;
                    }
                }
                WriteCmd::Resize(c, r) => {
                    let _ = write_half.window_change(c, r, 0, 0).await;
                }
            }
        }
    });

    // 读 task（主线程）：channel → stdout
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    let mut code: u8 = 0;

    // 外部 SIGINT（hsshx killconn 或用户手动 kill -INT）必须先恢复 termios 再退出：
    // enable_raw_mode 关掉了 ISIG，键盘 Ctrl-C 不会变成信号（它照常发给远端 shell），
    // 所以这个分支只接外部信号。而默认动作终止进程会把本地终端留在 raw 状态
    // （回显与行缓冲全丢），等于毁掉发起者的终端——那是本模块的真实缺陷，不只是 killconn 的前置。
    let mut sig = std::pin::pin!(tokio::signal::ctrl_c());
    let mut interrupted = false;
    loop {
        let msg = tokio::select! {
            m = read_half.wait() => m,
            _ = &mut sig => {
                interrupted = true;
                None
            }
        };
        let Some(msg) = msg else { break };
        match msg {
            ChannelMsg::Data { ref data } => {
                let _ = stdout.write_all(data);
                let _ = stdout.flush();
            }
            ChannelMsg::ExtendedData { ref data, .. } => {
                let _ = stderr.write_all(data);
                let _ = stderr.flush();
            }
            ChannelMsg::ExitStatus { exit_status } => {
                code = (exit_status & 0xFF) as u8;
            }
            _ => {}
        }
    }
    // 外部 SIGINT（hssh killconn 或用户手动 kill -INT）：raw 模式关掉了 ISIG，键盘 Ctrl-C
    // 不会变成信号（照常发给远端 shell），所以这个分支只接外部信号。
    // 恢复 termios 后必须直接结束进程，不能回到正常的 disconnect + 运行时 Drop 路径：
    // stdin 读取在 spawn_blocking 线程里阻塞等本地终端输入，Drop 会等它，而被杀的会话
    // 不会再有输入 —— 实测会让"已终止"的会话滞留 90s 以上，killconn 因此确认不到终止。
    // 代价：跳过 Holder 的 Drop，租约文件留待下次 scan 回收（flock 随进程退出释放，
    // 所以 still_alive/listconn 的判活始终是对的）。
    // 正常退出（远端 shell 自己 exit）仍走原路径，行为与改动前逐字节一致。
    if interrupted {
        restore_termios(stdin_fd, &orig_termios);
        // 130 = 128+SIGINT，Unix 惯例：让上层脚本把「被信号终止」与远端退出码区分开
        std::process::exit(130);
    }

    restore_termios(stdin_fd, &orig_termios);
    code
}

#[cfg(not(unix))]
async fn interactive_mode(
    _handle: &russh::client::Handle<hetushell_lib::ssh::conn::ClientHandler>,
) -> u8 {
    dprintln!("hsshx: 交互模式仅支持 Unix 系统");
    255
}

#[cfg(unix)]
fn term_size() -> (u32, u32) {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) };
    if ok == 0 && ws.ws_col > 0 && ws.ws_row > 0 {
        (ws.ws_col as u32, ws.ws_row as u32)
    } else {
        (80, 24)
    }
}

#[cfg(unix)]
fn enable_raw_mode(fd: i32) -> std::io::Result<libc::termios> {
    let mut orig: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut orig) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut raw = orig;
    raw.c_lflag &= !(libc::ECHO | libc::ICANON | libc::ISIG | libc::IEXTEN);
    raw.c_iflag &= !(libc::BRKINT | libc::ICRNL | libc::INPCK | libc::ISTRIP | libc::IXON);
    raw.c_oflag &= !libc::OPOST;
    raw.c_cflag |= libc::CS8;
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(orig)
}

#[cfg(unix)]
fn restore_termios(fd: i32, orig: &libc::termios) {
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, orig) };
}

fn usage() {
    dprintln!("用法:");
    dprintln!("  hsshx <连接项>                     交互模式（进入远程 shell，退出即回本地）");
    dprintln!("  hsshx <连接项> <命令...>           执行命令并退出");
    dprintln!("  hsshx <连接项> -f <脚本文件>       从文件读取命令执行");
    dprintln!("  hsshx <连接项> | command           从 stdin 管道读取命令执行");
    dprintln!("  hsshx <连接项> --get <远端> [本地]  下载文件或目录（本地默认当前目录）");
    dprintln!("  hsshx <连接项> --put <本地> <远端目录>  上传文件或目录");
    dprintln!("  hsshx <连接项> --cp <远端源> <目标>  远端复制（目标写「连接项:目录」则跨机）");
    dprintln!("  hsshx <连接项> --ls [远端目录]     列远端目录（默认 home）");
    dprintln!("  hsshx <连接项> --stat <远端路径>   取远端元数据（JSON）");
    dprintln!("  hsshx <连接项> ... --force         允许覆盖已存在的目标");
    dprintln!("  hsshx <连接项> ... --by <标记>     标注发起方（AI/脚本自命名，供 listconn 区分归属）");
    dprintln!("  hsshx <连接项> ... --machine        自身诊断改投 fd 3：stdout/stderr 只剩远端两条流");
    dprintln!("                                     （需调用方开 fd 3，如 3>/tmp/hsshx.diag；未开则报错）");
    dprintln!("  hsshx <连接项> --put <本地> <目录> -p  远端目录缺失时逐级自动创建（--mkdir 同义）");
    dprintln!("  hsshx listconn [--json]            列出活跃的 hsshx 连接（免连接，只读租约）");
    dprintln!("  hsshx killconn <id>                终止一条带 --by 标记的 hsshx 连接");
    dprintln!("  hsshx -l                           列出已保存的连接项");
    dprintln!("  hsshx -h                           显示简要帮助");
    dprintln!("  hsshx --doc                        显示完整文档（含示例，适合人与 AI 参考）");
}

/// 完整文档：结构化说明 + 基本/进阶示例。输出到 stdout，可直接管道给
/// 人阅读或 AI 参考（如 `hsshx --doc | llm`）。
fn print_doc() {
    println!("hsshx — HetuShell SSH 命令行工具（SSH exec + 文件传输 CLI）");
    println!("====================================================");
    println!();
    println!("概述");
    println!("----");
    println!("复用 HetuShell 的 SSH/SFTP 栈（russh + russh-sftp），按连接项（profiles.json）");
    println!("执行远程命令与文件搬运。与交互式终端不同，hsshx 面向脚本化/自动化场景：");
    println!("非交互执行、纯净输出、退出码可判别。认证信息（私钥/口令）来自已保存的连接项，");
    println!("命令行不出现任何凭据。");
    println!();
    println!("运行模式");
    println!("--------");
    println!("A. 命令类（连接项后不带传输选项）");
    println!("  1. exec 模式（默认）: hsshx <连接项> <命令...>");
    println!("     非交互执行命令，stdout/stderr 分离输出，退出码跟随远程命令（0-255）。");
    println!("  2. file 模式: hsshx <连接项> -f <脚本文件>");
    println!("     从本地文件读取命令内容后按 exec 模式执行（适合多行脚本）。");
    println!("  3. 交互模式: hsshx <连接项>");
    println!("     请求远程 PTY + shell，完整交互能力（sudo 密码、apt 确认、vi 等）。");
    println!("     退出即返回本地 shell。");
    println!();
    println!("B. 文件类（连接项后带 --get/--put/--cp/--ls/--stat）");
    println!("  4. 下载: hsshx <连接项> --get <远端路径> [本地落点]");
    println!("  5. 上传: hsshx <连接项> --put <本地路径> <远端目录>");
    println!("  6. 远端复制: hsshx <连接项> --cp <远端源> <目标>");
    println!("  7. 列目录: hsshx <连接项> --ls [远端目录]");
    println!("  8. 取元数据: hsshx <连接项> --stat <远端路径>");
    println!();
    println!("C. 免连接的本地子命令（不读 profiles、不开 SSH，只操作租约目录）");
    println!("  9.  看连接: hsshx listconn [--json]");
    println!("      只列 hsshx 自己发起的活跃连接（交互会话与进行中的传输）。");
    println!("      GUI 面板/终端 hssh 建的连接不在此列——它们在另一个进程的记忆体里，");
    println!("      要用 `hssh listconn` 看（那份是合并视图，需 HetuShell 正在运行）。");
    println!("  10. 终止连接: hsshx killconn <id>");
    println!("      id 取 listconn 的短 ID（唯一前缀即可）。只对带 --by 标记的连接放行——");
    println!("      用户手敲的会话不会被脚本随手杀。发 SIGINT，对方按既有路径取消回滚并自删租约。");
    println!();
    println!("命令来源优先级");
    println!("--------------");
    println!("-f 文件 > 位置参数 > stdin 管道 > 交互模式");
    println!();
    println!("参数说明");
    println!("--------");
    println!("  <连接项>             profiles.json 中的连接项名称（hsshx -l 查看）");
    println!("  <命令...>            要执行的命令（多参数自动空格连接）");
    println!("  -f, --file <文件>    从文件读取命令内容");
    println!("  --get <远端> [本地]  下载。本地省略或指向已存在目录时，按远端同名落盘");
    println!("  --put <本地> <目录>  上传到远端目录，落点为 <目录>/<本地basename>");
    println!("                     远端目录默认必须已存在；缺失时加 -p 逐级自动创建");
    println!("  --cp <源> <目标>     目标不带前缀=同机复制；写成「连接项:目录」=跨机复制");
    println!("  --ls [目录]          列目录（远端 home 缺省）；输出为「大小 名字」，目录尾随 /");
    println!("  --stat <路径>        输出 JSON：size/isDir/isLink/perms/mtime/uid/gid/owner/group");
    println!("  --force              允许覆盖已存在的目标（仅对 --get/--put/--cp 有效）");
    println!("  -p, --mkdir          --put 的远端目录逐级自动创建（仅对 --put 有效，不给 --get/--cp 建目标）");
    println!("  -l, --list           列出已保存的连接项");
    println!("  -h, --help           显示简要帮助");
    println!("  --doc                显示完整文档（本内容）");
    println!();
    println!("选项识别边界");
    println!("------------");
    println!("hsshx 自有选项只在「远程命令的第一个位置参数出现之前」识别。命令一旦开始");
    println!("收集，其后全部原样交给远端，不会被解析吞掉：");
    println!("  hsshx web uptime -f all   → 远端收到 \"uptime -f all\"");
    println!();
    println!("文件传输语义");
    println!("------------");
    println!("- 目录递归：--get/--put 对目录整树处理；符号链接一律跳过（避免环），");
    println!("  因此链接目标不会被搬运，远端也不会重建链接。");
    println!("- 权限保留：尽力保留 0o777 位（上传保本地 mode，下载保远端 mode），");
    println!("  属主/属组/mtime 不保留（需 root，跨机无意义）。");
    println!("- 覆盖保护：目标已存在时默认**拒绝**并退出码 1，须显式 --force。");
    println!("  这是刻意与 GUI 不同：GUI 有确认框，CLI 多为脚本无人值守，静默截断更危险。");
    println!("  判定发生在任何写入之前，拒绝不会破坏既有内容。");
    println!("- 覆盖写入是原子替换：--force 覆盖既有文件时先写 `<目标名>.<标识>.part`，");
    println!("  全部传完才删旧目标并 rename 过去。所以取消、断连、写失败都**不会损坏既有");
    println!("  文件**——直写最终名的话 O_TRUNC 会在传输出错前就把用户文件清空。");
    println!("  替换阶段万一 rename 失败，保留 .part 并把其路径写进错误信息，绝不静默丢数据。");
    println!("- 目标类型不符（要用文件替换目录，或反过来）在任何传输之前就报错。");
    println!("- Ctrl-C 取消：数据块边界、远端目录枚举、建目录骨架处都可中止，退出码 130。");
    println!("  清理原则是「只删本次写入的临时名，绝不批量删除既有内容」：");
    println!("  目标目录本次新建 → 整树回滚；目标目录本就存在（--force 覆盖）→ 一个文件都不删。");
    println!("  既有目录覆盖场景下，本次新建但尚未写入任何文件的**空目录骨架会留在原处**——这是");
    println!("  「绝不批量删除既有内容」的必然代价，手工删除即可。");
    println!("  第二次 Ctrl-C 立即退出（清理可能卡在停滞网络上）。进程被 SIGKILL 强杀、或清理被");
    println!("  强行中止时会留下 `<名>.<标识>.part` 临时文件；下次传输不受影响，可手工清理。");
    println!("- 跨机 --cp：数据在两个 SSH 连接间流式中转，**不落本地盘**；目标先写");
    println!("  <名>.<标识>.part 再 rename，故中断/同名别名都不会截断已有数据。");
    println!("- 同机 --cp（同一连接项）：走服务器内 cp -a，零中转带宽且保留 mtime/权限。");
    println!("  不同连接项即使 host 相同、端口不同，也按跨机处理（保守，不误判同机）。");
    println!("- 恶意服务器防护：下载时拒绝 \".\"/\"..\"/含路径分隔符的条目名，写到不了目标目录外。");
    println!();
    println!("输出通道约定");
    println!("--------------");
    println!("  stdout = 结果数据，保持纯净（exec 的远程输出、--ls 列表、--stat 的 JSON）");
    println!("  stderr = 进度条、错误信息、以及 --put/--cp 成功后打印的远端落点路径");
    println!("进度仅在 stderr 是终端时绘制（整任务占一行原地刷新）；重定向到文件/管道时");
    println!("完全静默，只保留每个对象一行的名字。脚本要静音用 2>/dev/null。");
    println!();
    println!("三条流分离（--machine，给 AI/脚本）");
    println!("--------------------------------");
    println!("默认约定里 stderr 同时装了两件事：远端程序的 stderr，以及 hsshx 自己的话");
    println!("（连不上、认证失败、进度、落点路径）。调用方拿到一行 stderr 时无从判断是谁说的，");
    println!("而 rc=255 既可能是远端命令返回 255、也可能是压根没连上——只能猜。");
    println!();
    println!("加 --machine 并把 fd 3 打开，hsshx 自己的话就整体改投 fd 3，三条流互不污染：");
    println!("  stdout = 远端程序的 stdout（结果数据）");
    println!("  stderr = 远端程序的 stderr（不掺任何 hsshx 自身信息）");
    println!("  fd 3   = hsshx 自己的诊断（错误/进度/落点路径）");
    println!();
    println!("  hsshx web --machine \"make\" 3>/tmp/hsshx.diag 2>/tmp/remote.err");
    println!("  hsshx web --machine --put ./build srv:/opt 3>/tmp/hsshx.diag");
    println!();
    println!("判读规则：fd 3 有内容 = hsshx 自身说过话（失败或进度）；fd 3 空而 rc!=0 = 远端");
    println!("命令自己的退出码。不加 --machine 时一切照旧（诊断仍在 stderr），人的手动用法不变。");
    println!("fd 3 未打开时 --machine 直接报错退出 2，不会静默回落到 stderr——那会让调用方");
    println!("误以为已经拿到三条纯净流。仅支持 Unix。");
    println!();
    println!("stdin 管道输入");
    println!("--------------");
    println!("exec 模式下，stdin 管道数据自动转发到远程命令（Unix 管道哲学），");
    println!("读完发送 EOF。典型用法：");
    println!();
    println!("  echo \"data\" | hsshx server \"cat\"");
    println!("  echo \"password\" | hsshx server \"sudo -S apt update\"");
    println!("  yes | hsshx server \"apt install pkg\"");
    println!();
    println!("注意：sudo/apt 等交互命令在 exec 模式（无 PTY）下无法直接交互，");
    println!("应使用其非交互参数（sudo -S、apt -y、yes |），或改用交互模式。");
    println!("文件类模式不读 stdin。");
    println!();
    println!("退出码");
    println!("------");
    println!("  0     成功（命令类：远程退出码为 0）");
    println!("  1-255 跟随远程命令退出码（命令类）");
    println!("  1     文件传输失败（含目标已存在未加 --force、远端路径不存在、权限不足）");
    println!("  130   文件传输被 Ctrl-C 取消（本次写入的临时名已清理，既有内容未受影响）");
    println!("  2     参数错误 / 连接项未找到");
    println!("  255   SSH 连接或 channel 建立失败");
    println!();
    println!("示例");
    println!("----");
    println!("  # 基本");
    println!("  hsshx web \"uptime\"");
    println!("  hsshx web \"df -h && free -m\"");
    println!("  hsshx -l");
    println!();
    println!("  # 文件脚本");
    println!("  hsshx web -f deploy.sh");
    println!();
    println!("  # 管道（sudo/自动化）");
    println!("  echo \"password\" | hsshx web \"sudo -S systemctl restart nginx\"");
    println!("  yes | hsshx web \"apt install -y git\"");
    println!("  curl -s https://example.com/install.sh | hsshx web \"bash\"");
    println!();
    println!("  # 上传/下载（目录自动递归）");
    println!("  hsshx web --put ./build server:/opt/app      # → stderr 打印 /opt/app/build");
    println!("  hsshx web --get /var/log/app.log ./logs      # 落 ./logs/app.log");
    println!("  hsshx web --get /var/log/app.log /tmp/a.log  # 指定目标文件名");
    println!("  hsshx web --put ./site server:/srv           # 整棵目录树上传");
    println!("  hsshx web --put ./f server:/srv --force      # 覆盖已存在的目标");
    println!();
    println!("  # 远端之间搬运（不经过本地磁盘）");
    println!("  hsshx web --cp /data/ckpt gpu:/models        # 跨连接：客户端流式中转");
    println!("  hsshx web --cp /data/ckpt /backup            # 同连接：服务器内 cp -a");
    println!();
    println!("  # 探查（脚本/AI 友好）");
    println!("  hsshx web --ls /var/log");
    println!("  hsshx web --stat /var/log/app.log | jq .size");
    println!("  hsshx web --ls /tmp 2>/dev/null | head      # 只要数据，丢弃进度");
    println!();
    println!("已知限制");
    println!("--------");
    println!("- 文件类模式只走 SFTP subsystem；服务器禁用 sftp 子系统时报错，");
    println!("  不会自动回退成 base64-over-exec（GUI 传输有该兜底，CLI 暂未复用）。");
    println!("- 无断点续传：中断后需整文件重传（--force 覆盖）。");
    println!("- 目录复制（--cp 目录）被取消时不回滚已复制部分：目标树与源可能存在");
    println!("  未检出的别名重叠，批量删除的风险高于留半成品。");
    println!("- 不做增量/校验和比对（非 rsync）：整树按字节搬运，是否覆盖由 --force 决定。");
    println!("- 目录上传/下载是**合并**语义：只增改同名内容，不删除目标侧多出来的文件");
    println!("  （不等价于 rsync --delete）。");
    println!("- 覆盖既有大文件时，传输期间临时名与旧文件并存，磁盘峰值约 2×；");
    println!("  这是「既有文件不会被传输出错损坏」的代价，空间紧张时可能 ENOSPC。");
    println!("- 目录的权限位不保留（下载目录用本地 umask，上传目录用远端默认），");
    println!("  只有文件保留 0o777 位——与 GUI 行为一致。");
    println!();
    println!("安全提示");
    println!("--------");
    println!("- 密码等敏感信息通过 stdin 管道传入（sudo -S），切勿作为命令行参数");
    println!("  （ps 可被其他用户看到）。");
    println!("- 连接项密码/口令不会持久化，仅支持密钥认证（keyData/keyPath）。");
    println!("- 主机指纹为 TOFU（首次信任）：指纹变更即拒绝连接，防中间人。");
}

fn list_profiles() -> ExitCode {
    let profiles = load_profiles();
    if profiles.is_empty() {
        println!("（无连接项）");
        return ExitCode::SUCCESS;
    }
    for p in &profiles {
        println!("{:<20} {:<6} {}@{}:{}", p.name, p.auth, p.user, p.host, p.port);
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 解析器是纯函数：不建连接、不碰 stdin（涉及 stdin 的用例必须自带命令或 -f，
    /// 否则 resolve_command 会去读管道并阻塞）。argv[0] 按惯例是程序名，解析从 1 开始。
    fn parse(argv: &[&str]) -> Result<Spec, String> {
        parse_args(&argv.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    /// 每个用例独占一份脚本文件。共用同一路径会 flaky：cargo test 并行执行时，
    /// 另一用例的 std::fs::write（O_TRUNC）可能正撞在本用例读取的瞬间，读到空内容。
    fn temp_script(tag: &str) -> String {
        let dir = std::env::temp_dir().join(format!("hsshx-parse-tests-{tag}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.sh");
        std::fs::write(&path, "uptime\n").unwrap();
        path.to_string_lossy().into_owned()
    }

    fn exec_command(spec: &Spec) -> Option<&str> {
        match &spec.mode {
            Mode::Exec { command } => command.as_deref(),
            _ => None,
        }
    }

    /// `--by <tag>` 在连接项名之后、命令之前被识别，且不影响命令收集。
    #[test]
    fn by_tag_is_captured_without_disturbing_the_command() {
        let spec = parse(&["hsshx", "web", "--by", "qwen", "uptime"]).unwrap();
        assert_eq!(spec.name, "web");
        assert_eq!(spec.by.as_deref(), Some("qwen"));
        assert_eq!(exec_command(&spec), Some("uptime"));
    }

    /// 回归锁：不带 `--by` 时 by 必须为 None（listconn 据此显示 user），
    /// 且命令解析与带 tag 时完全一致——新选项不得动到既有路径。
    #[test]
    fn absent_by_tag_stays_none_and_command_is_untouched() {
        let spec = parse(&["hsshx", "web", "uptime", "-f", "all"]).unwrap();
        assert_eq!(spec.by, None, "缺省即 user 发起，不能凭空补 tag");
        assert_eq!(exec_command(&spec), Some("uptime -f all"));
    }

    /// `--machine` 的三条解析锁：默认关、显式开、开了不影响命令收集。
    /// 默认关这条最关键——它锁住「人的手动用法逐字节不变」的前提。
    #[test]
    fn machine_flag_defaults_off_and_captures_on() {
        assert!(
            !parse(&["hsshx", "web", "uptime"]).unwrap().machine,
            "不带 --machine 必须走 stderr（既有行为）"
        );
        let spec = parse(&["hsshx", "web", "--machine", "uptime"]).unwrap();
        assert!(spec.machine, "--machine 必须被识别");
        assert_eq!(spec.name, "web");
        assert_eq!(exec_command(&spec), Some("uptime"), "开关不得吞掉远端命令");
    }

    /// `--machine` 是纯开关，刻意不关 flags_open：`hsshx web --machine --ls /tmp` 里
    /// `--ls` 仍要认得。反过来，命令开始收集后的 `--machine` 属于远端，不得被吃掉。
    #[test]
    fn machine_keeps_flags_open_and_stays_unparsed_after_command() {
        match parse(&["hsshx", "web", "--machine", "--ls", "/tmp"]).unwrap().mode {
            Mode::Job(Job::Ls { path }) => assert_eq!(path.as_deref(), Some("/tmp")),
            other => panic!("--machine 之后的 --ls 仍应是传输模式，实际: {other:?}"),
        }
        let spec = parse(&["hsshx", "web", "grep", "--machine", "x"]).unwrap();
        assert!(
            !spec.machine,
            "命令里的 --machine 属远端参数，不得被 CLI 吃掉"
        );
        assert_eq!(exec_command(&spec), Some("grep --machine x"));
    }

    /// `--machine` 已登记进 is_hsshx_option，否则 `--get --machine /x` 会把开关
    /// 静默当成远端路径发给服务器（与 --force/--by 同一失效模式）。
    #[test]
    fn machine_is_never_taken_as_a_transfer_operand() {
        match parse(&["hsshx", "web", "--machine", "--get", "/r/a.log"])
            .unwrap()
            .mode
        {
            Mode::Job(Job::Get { remote, local }) => {
                assert_eq!(remote, "/r/a.log");
                let _ = local;
            }
            other => panic!("--machine 不得被 take_job 吞成操作数，实际: {other:?}"),
        }
    }

    /// tag 会被 `hssh listconn` 原样 cat 到宿主终端，含 ESC/CR 即转义序列注入入口，
    /// 必须在 CLI 边界拒绝——这条锁住的是 valid_tag 的白名单本身。
    #[test]
    fn by_tag_rejects_terminal_injection_payloads() {
        for bad in ["qwen\u{1b}[2J", "a\nb", "a b", "--force", "中文", "", &"x".repeat(33)] {
            let err = parse(&["hsshx", "web", "--by", bad, "uptime"])
                .err()
                .unwrap_or_else(|| panic!("非法 tag {bad:?} 必须被拒绝"));
            assert!(err.contains("--by"), "报错应指向 --by，实际: {err}");
        }
    }

    /// `--by` 缺值必须直接报错，不能把下一个 token（可能是远端命令）当 tag 吞掉。
    #[test]
    fn by_tag_without_value_is_an_error() {
        let err = parse(&["hsshx", "web", "--by"]).err().expect("--by 缺值必须报错");
        assert!(err.contains("缺少参数"), "实际: {err}");
    }

    /// 传输模式下 tag 同样可用：`--by` 已登记进 is_hsshx_option，
    /// 否则会被 take_job 当成远端路径静默发给服务器。
    #[test]
    fn by_tag_coexists_with_transfer_jobs() {
        match parse(&["hsshx", "web", "--by", "ci", "--get", "/r/a.log"]).unwrap().mode {
            Mode::Job(Job::Get { remote, local }) => {
                assert_eq!(remote, "/r/a.log");
                assert_eq!(local, ".");
            }
            _ => panic!("应解析为 Get 任务"),
        }
    }

    /// 白名单字符集要放行真实会用的写法（工具名带斜杠/冒号/点/加号）。
    #[test]
    fn by_tag_accepts_common_tool_names() {
        for ok in ["qwen-code", "claude.code", "ci:runner", "ha1/+@:_"] {
            let spec = parse(&["hsshx", "web", "--by", ok, "uptime"]).unwrap();
            assert_eq!(spec.by.as_deref(), Some(ok), "{ok} 应被放行");
        }
    }

    /// `-p` 放在操作数之后仍要生效：`take_job` 取操作数不会关闭 flags_open，
    /// 所以 `--put ./a /srv -p` 是合法写法（与 --force 的既有用法同形）。
    #[test]
    fn mkdir_flag_after_operands_is_still_parsed() {
        let spec = parse(&["hsshx", "web", "--put", "./a", "/srv/in", "-p"]).unwrap();
        assert!(spec.mkdir, "--put 后置 -p 必须生效");
        match spec.mode {
            Mode::Job(Job::Put { ref local, ref remote_dir }) => {
                assert_eq!(local, "./a");
                assert_eq!(remote_dir, "/srv/in");
            }
            ref m => panic!("应为 Put 任务，实际 {m:?}"),
        }
    }

    /// 长写法等价，且 -p 在 job 之前也能识别。
    #[test]
    fn mkdir_long_form_and_leading_position() {
        let s = parse(&["hsshx", "web", "--mkdir", "--put", "./a", "/srv"]).unwrap();
        assert!(s.mkdir);
        assert!(matches!(s.mode, Mode::Job(Job::Put { .. })));
    }

    /// 回归锁：不带 -p 时 mkdir 必须为 false —— 否则「远端目录必须已存在」的既有保护
    /// 会在所有人身上失效，静默往远端建目录。
    #[test]
    fn mkdir_defaults_off_and_put_untouched() {
        let s = parse(&["hsshx", "web", "--put", "./a", "/srv"]).unwrap();
        assert!(!s.mkdir, "缺省不得自动建目录");
        let e = parse(&["hsshx", "web", "uptime"]).unwrap();
        assert!(!e.mkdir);
    }

    /// `-p` 只对 --put 有意义：给 --get/--cp 静默收下会让人以为目标目录被建好了。
    #[test]
    fn mkdir_rejected_for_other_jobs() {
        for argv in [
            &["hsshx", "web", "--get", "/r/a.log", "-p"][..],
            &["hsshx", "web", "--cp", "/a", "/b", "-p"][..],
            &["hsshx", "web", "--ls", "/tmp", "-p"][..],
            &["hsshx", "web", "uptime", "-p"][..],
        ] {
            // 命令位之后的 -p 原样交给远端（uptime -p），那不是「误用的开关」
            let err = match parse(argv) {
                Ok(s) if s.mkdir => panic!("-p 不该对 {argv:?} 生效"),
                Ok(s) => {
                    // exec 路径：-p 落在命令里，mkdir 保持 false 即可
                    assert!(!s.mkdir);
                    continue;
                }
                Err(e) => e,
            };
            assert!(err.contains("-p"), "报错应指向 -p，实际: {err}");
        }
    }

    /// `-p` 必须登记进 is_hsshx_option：否则 `--put -p /srv` 会把 `-p` 当本地路径取走，
    /// 真正危险的后果在 `--get --force -p x` 这类组合里——选项被静默当路径发给服务器。
    #[test]
    fn mkdir_option_is_never_taken_as_operand() {
        assert!(is_hsshx_option("-p"));
        assert!(is_hsshx_option("--mkdir"));
        // --put 要求两个操作数，-p 占位时必须报错而不是把 -p 当路径
        let err = parse(&["hsshx", "web", "--put", "-p", "/srv"]).err().unwrap();
        assert!(err.contains("--put"), "实际: {err}");
    }

    /// `-p` 与 `--by` 可共存：AI 发起的自动建目录上传，两个元信息都要留下。
    #[test]
    fn mkdir_coexists_with_by_tag() {
        let s = parse(&["hsshx", "web", "--by", "qwen", "--put", "./a", "/srv/in", "-p"]).unwrap();
        assert_eq!(s.by.as_deref(), Some("qwen"));
        assert!(s.mkdir);
    }

    /// 回归锁：`-f` 在连接项名**之后**必须仍被识别。
    /// 曾因把守卫写成 `name.is_none()` 而失效，导致 `-f` 掉进命令参数，
    /// 远端收到 `bash --login -c '-f'` 并报 `bash: - : invalid option`。
    #[test]
    fn file_option_after_name_is_still_parsed() {
        let script = temp_script("after-name");
        let spec = parse(&["hsshx", "web", "-f", &script]).unwrap();
        assert_eq!(spec.name, "web");
        assert_eq!(exec_command(&spec), Some("uptime\n"), "-f 文件内容应成为命令");
    }

    /// `-f` 也可出现在连接项名之前（原 parsing_flags 允许的位置）。
    #[test]
    fn file_option_before_name_works_too() {
        let script = temp_script("before-name");
        let spec = parse(&["hsshx", "-f", &script, "web"]).unwrap();
        assert_eq!(spec.name, "web");
        assert_eq!(exec_command(&spec), Some("uptime\n"));
    }

    #[test]
    fn positional_after_command_is_not_option_parsed() {
        // 命令参数一旦开始收集，后续内容原样交给远程命令
        let spec = parse(&["hsshx", "web", "uptime", "-f", "all"]).unwrap();
        assert_eq!(exec_command(&spec), Some("uptime -f all"));
        assert!(!spec.force);
    }

    #[test]
    fn job_flags_take_their_operands() {
        match parse(&["hsshx", "web", "--get", "/r/a.log"]).unwrap().mode {
            Mode::Job(Job::Get { remote, local }) => {
                assert_eq!(remote, "/r/a.log");
                assert_eq!(local, ".", "--get 缺省落点应为当前目录");
            }
            _ => panic!("应为 Get"),
        }
        let spec = parse(&["hsshx", "web", "--put", "./a.sh", "/tmp", "--force"]).unwrap();
        assert!(spec.force, "--force 可在操作数之后");
        match spec.mode {
            Mode::Job(Job::Put { local, remote_dir }) => {
                assert_eq!(local, "./a.sh");
                assert_eq!(remote_dir, "/tmp");
            }
            _ => panic!("应为 Put"),
        }
        match parse(&["hsshx", "web", "--ls"]).unwrap().mode {
            Mode::Job(Job::Ls { path }) => assert!(path.is_none(), "--ls 缺省列 home"),
            _ => panic!("应为 Ls"),
        }
        match parse(&["hsshx", "web", "--cp", "/a", "other:/b"]).unwrap().mode {
            Mode::Job(Job::Cp { src, dst }) => {
                assert_eq!(src, "/a");
                assert_eq!(dst, "other:/b", "跨机前缀留给执行期按连接项表判定");
            }
            _ => panic!("应为 Cp"),
        }
    }

    /// 传输模式不得吞掉远程命令里的同类字符串。
    #[test]
    fn job_flag_inside_command_is_not_swallowed() {
        let spec = parse(&["hsshx", "web", "echo --put /a /b"]).unwrap();
        assert_eq!(exec_command(&spec), Some("echo --put /a /b"));
    }

    #[test]
    fn missing_or_excess_operands_are_errors() {
        assert!(parse(&["hsshx", "web", "--put", "only"]).is_err(), "--put 需要两个操作数");
        assert!(parse(&["hsshx", "web", "--stat"]).is_err(), "--stat 需要路径");
        assert!(parse(&["hsshx", "web", "--get"]).is_err(), "--get 需要远端路径");
        assert!(parse(&["hsshx", "web", "-f"]).is_err(), "-f 缺参数");
        assert!(parse(&["hsshx", "web", "--ls", "a", "b"]).is_err(), "传输模式不接受多余参数");
        assert!(parse(&["hsshx"]).is_err(), "缺连接项名");
    }

    /// --force 脱离传输操作属于误用，且必须在读 stdin 之前就报错（故此处不会阻塞）。
    #[test]
    fn force_without_job_is_rejected() {
        let err = parse(&["hsshx", "--force", "web"]).unwrap_err();
        assert!(err.contains("--force"), "实际错误: {err}");
    }

    /// 回归锁：hsshx 自有选项绝不能被当成操作数取走。
    /// 曾发生 `--get --force /x` 把字符串 "--force" 当作远端路径发给服务器（静默传错目标）。
    #[test]
    fn option_is_never_taken_as_operand() {
        assert!(
            parse(&["hsshx", "web", "--get", "--force", "/x"]).is_err(),
            "--force 不得被当成远端路径"
        );
        assert!(
            parse(&["hsshx", "web", "--put", "./a", "--force"]).is_err(),
            "--force 不得被当成远端目录"
        );
        assert!(
            parse(&["hsshx", "web", "--stat", "--ls"]).is_err(),
            "--ls 不得被当成 --stat 的路径"
        );
        // 以 -- 开头的真实路径仍要能用（它不等于任何自有选项）
        match parse(&["hsshx", "web", "--get", "./--weird-name"]).unwrap().mode {
            Mode::Job(Job::Get { remote, .. }) => assert_eq!(remote, "./--weird-name"),
            _ => panic!("应解析为 Get"),
        }
    }

    /// `--cp` 目标拆分：只有冒号前缀命中已保存连接项才算跨机，
    /// 否则整串按路径处理（含冒号的真实文件名不被切坏）。
    #[test]
    fn cp_dst_split_only_on_known_profile() {
        let profiles = [profile_named("gpu"), profile_named("web")];

        let (conn, path) = split_dst("gpu:/models", &profiles);
        assert_eq!(conn.as_deref(), Some("gpu"));
        assert_eq!(path, "/models");

        let (conn, path) = split_dst("/plain/path", &profiles);
        assert!(conn.is_none());
        assert_eq!(path, "/plain/path");

        let (conn, path) = split_dst("/tmp/log:2026", &profiles);
        assert!(conn.is_none(), "未知前缀不得被当成连接项名");
        assert_eq!(path, "/tmp/log:2026");

        // 空前缀（":/x"）不拆分，避免把空串当连接项名去查
        let (conn, path) = split_dst(":/x", &profiles);
        assert!(conn.is_none());
        assert_eq!(path, ":/x");
    }

    fn profile_named(name: &str) -> Profile {
        serde_json::from_str(&format!(
            r#"{{"id":"{name}","name":"{name}","host":"h","port":22,"user":"u","auth":"key","source":"manual"}}"#
        ))
        .unwrap_or_else(|e| panic!("构造 Profile 失败: {e}"))
    }
}
