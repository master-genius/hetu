//! hsshx — HetuShell SSH exec CLI
//! 复用 hetushell_lib SSH 栈，按 profiles.json 连接项执行远程命令。
//! 纯净输出：仅远程 stdout/stderr，退出码跟随远程命令。

use std::io::{IsTerminal, Read, Write};
use std::process::ExitCode;

use russh::{ChannelMsg, Disconnect};

use hetushell_lib::settings::load_profiles;
use hetushell_lib::ssh::conn::{establish, ConnParams};

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();

    if args.len() == 2 && (args[1] == "-l" || args[1] == "--list") {
        return list_profiles();
    }

    if args.len() < 2 {
        usage();
        return ExitCode::from(2);
    }

    let name = &args[1];
    let profile = match load_profiles().into_iter().find(|p| &p.name == name) {
        Some(p) => p,
        None => {
            eprintln!("hsshx: 未找到连接项「{name}」");
            return ExitCode::from(2);
        }
    };

    let command = if args.len() >= 3 {
        args[2..].join(" ")
    } else if !std::io::stdin().is_terminal() {
        let mut buf = String::new();
        match std::io::stdin().read_to_string(&mut buf) {
            Ok(_) => buf,
            Err(_) => {
                eprintln!("hsshx: 读取 stdin 失败");
                return ExitCode::from(2);
            }
        }
    } else {
        usage();
        return ExitCode::from(2);
    };

    if command.trim().is_empty() {
        return ExitCode::SUCCESS;
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

    let mut channel = match handle.channel_open_session().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("hsshx: {e}");
            return ExitCode::from(255);
        }
    };

    if let Err(e) = channel.exec(true, command.as_str()).await {
        eprintln!("hsshx: {e}");
        return ExitCode::from(255);
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

    let _ = handle.disconnect(Disconnect::ByApplication, "bye", "zh").await;

    ExitCode::from(code)
}

fn usage() {
    eprintln!("用法: hsshx <连接项> <命令>  |  hsshx <连接项>  |  hsshx -l");
    eprintln!("  无命令且 stdin 有数据时自动从 stdin 读取命令");
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
