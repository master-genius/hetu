//! hsshx — HetuShell SSH exec CLI
//! 复用 hetushell_lib SSH 栈，按 profiles.json 连接项执行远程命令。
//!
//! 三种模式：
//! - exec 模式：hsshx <连接项> <命令...>  → 执行命令，纯净输出，退出码跟随远程命令
//! - file 模式：hsshx <连接项> -f <文件> → 从文件读取命令执行
//! - 交互模式：hsshx <连接项>            → 进入远程 shell，退出即回本地 shell
//!
//! stdin 管道有数据时自动读取为命令（exec 模式），与无命令+终端场景区分。

use std::io::{IsTerminal, Read, Write};
use std::process::ExitCode;

use russh::{ChannelMsg, Disconnect};
use tokio::sync::mpsc;

use hetushell_lib::settings::load_profiles;
use hetushell_lib::ssh::conn::{establish, ConnParams};

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();

    if args.len() == 2 && (args[1] == "-l" || args[1] == "--list") {
        return list_profiles();
    }

    if args.len() >= 2 && (args[1] == "-h" || args[1] == "--help") {
        usage();
        return ExitCode::SUCCESS;
    }

    if args.len() < 2 {
        usage();
        return ExitCode::from(2);
    }

    // 参数解析：连接项名 + 可选 -f 文件 + 可选命令参数
    let mut name: Option<String> = None;
    let mut file_path: Option<String> = None;
    let mut cmd_parts: Vec<String> = Vec::new();
    let mut parsing_flags = true;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                usage();
                return ExitCode::SUCCESS;
            }
            "-f" | "--file" if parsing_flags => {
                if i + 1 >= args.len() {
                    eprintln!("hsshx: 选项 {} 缺少参数", args[i]);
                    return ExitCode::from(2);
                }
                file_path = Some(args[i + 1].clone());
                i += 2;
            }
            _ => {
                if name.is_none() {
                    name = Some(args[i].clone());
                } else {
                    parsing_flags = false;
                    cmd_parts.push(args[i].clone());
                }
                i += 1;
            }
        }
    }

    let name = match name {
        Some(n) => n,
        None => {
            usage();
            return ExitCode::from(2);
        }
    };

    let profile = match load_profiles().into_iter().find(|p| p.name == name) {
        Some(p) => p,
        None => {
            eprintln!("hsshx: 未找到连接项「{name}」");
            return ExitCode::from(2);
        }
    };

    // 确定命令来源：file > 位置参数 > stdin 管道 > 交互模式
    let command = if let Some(path) = file_path {
        match std::fs::read_to_string(&path) {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("hsshx: 读取文件失败: {e}");
                return ExitCode::from(2);
            }
        }
    } else if !cmd_parts.is_empty() {
        Some(cmd_parts.join(" "))
    } else if !std::io::stdin().is_terminal() {
        let mut buf = String::new();
        match std::io::stdin().read_to_string(&mut buf) {
            Ok(_) => Some(buf),
            Err(_) => {
                eprintln!("hsshx: 读取 stdin 失败");
                return ExitCode::from(2);
            }
        }
    } else {
        None
    };

    if let Some(ref cmd) = command {
        if cmd.trim().is_empty() {
            return ExitCode::SUCCESS;
        }
    }

    let params = ConnParams {
        name: profile.name.clone(),
        host: profile.host.clone(),
        port: profile.port,
        user: profile.user.clone(),
        auth: profile.auth.clone(),
        password: None,
        key_path: profile.key_path.clone(),
        key_data: profile.key_data.clone(),
        passphrase: None,
        keepalive: profile.keepalive,
        timeout: profile.timeout,
    };

    let handle = match establish(&params).await {
        Ok(h) => h,
        Err(e) => {
            eprintln!("hsshx: {e}");
            return ExitCode::from(255);
        }
    };

    let code = match command {
        Some(cmd) => exec_mode(&handle, cmd).await,
        None => interactive_mode(&handle).await,
    };

    let _ = handle
        .disconnect(Disconnect::ByApplication, "bye", "zh")
        .await;

    ExitCode::from(code)
}

/// exec 模式：非交互执行命令，纯净输出，退出码跟随远程命令。
async fn exec_mode(
    handle: &russh::client::Handle<hetushell_lib::ssh::conn::ClientHandler>,
    command: String,
) -> u8 {
    let mut channel = match handle.channel_open_session().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("hsshx: {e}");
            return 255;
        }
    };

    if let Err(e) = channel.exec(true, command.as_str()).await {
        eprintln!("hsshx: {e}");
        return 255;
    }

    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    let mut code: u8 = 0;

    while let Some(msg) = channel.wait().await {
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
            eprintln!("hsshx: 无法设置终端 raw 模式: {e}");
            return 255;
        }
    };

    let channel = match handle.channel_open_session().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("hsshx: {e}");
            restore_termios(stdin_fd, &orig_termios);
            return 255;
        }
    };

    if let Err(e) = channel
        .request_pty(false, "xterm-256color", cols, rows, 0, 0, &[])
        .await
    {
        eprintln!("hsshx: 请求 PTY 失败: {e}");
        restore_termios(stdin_fd, &orig_termios);
        return 255;
    }

    if let Err(e) = channel.request_shell(false).await {
        eprintln!("hsshx: 请求 shell 失败: {e}");
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

    restore_termios(stdin_fd, &orig_termios);
    code
}

#[cfg(not(unix))]
async fn interactive_mode(
    _handle: &russh::client::Handle<hetushell_lib::ssh::conn::ClientHandler>,
) -> u8 {
    eprintln!("hsshx: 交互模式仅支持 Unix 系统");
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
    eprintln!("用法:");
    eprintln!("  hsshx <连接项>                     交互模式（进入远程 shell，退出即回本地）");
    eprintln!("  hsshx <连接项> <命令...>           执行命令并退出");
    eprintln!("  hsshx <连接项> -f <脚本文件>       从文件读取命令执行");
    eprintln!("  hsshx <连接项> | command           从 stdin 管道读取命令执行");
    eprintln!("  hsshx -l                           列出已保存的连接项");
    eprintln!("  hsshx -h                           显示此帮助");
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
