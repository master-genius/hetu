//! ~/.ssh/config 导入：只读解析，提取可直连的 Host 条目为 Profile。
//! 支持 Include 指令（含 glob 与 ~ 展开）；跳过含通配符的 Host 模式。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::settings::Profile;

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

/// 单个 Host 块解析出的键值（键统一小写）
type HostBlock = HashMap<String, String>;

fn read_config_file(path: &Path, depth: u8, out: &mut Vec<(String, HostBlock)>) {
    // 防御 Include 循环
    if depth > 4 {
        return;
    }
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };

    // 当前 Host 块：别名列表 + 共享设置。"Host web db" 会为 web、db 各生成一条。
    let mut current: Option<(Vec<String>, HostBlock)> = None;
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // 支持 "Key Value"、"Key=Value" 与 "Key = Value"（'=' 两侧可有空格）三种写法。
        // 先在首个分隔符处切分，再从值侧剥掉可能残留的 '=' 与空白，避免 "= value" 粘连。
        let (key, value) = match line.split_once(|c: char| c == ' ' || c == '\t' || c == '=') {
            Some((k, v)) => (
                k.trim().to_ascii_lowercase(),
                v.trim().trim_start_matches('=').trim().trim_matches('"'),
            ),
            None => continue,
        };
        if value.is_empty() {
            continue;
        }

        match key.as_str() {
            "include" => {
                for pattern in value.split_whitespace() {
                    let expanded = expand_tilde(pattern);
                    // 相对路径基于 ~/.ssh
                    let full = if expanded.is_relative() {
                        dirs::home_dir()
                            .map(|h| h.join(".ssh").join(&expanded))
                            .unwrap_or(expanded)
                    } else {
                        expanded
                    };
                    if let Some(s) = full.to_str() {
                        if let Ok(paths) = glob::glob(s) {
                            for p in paths.flatten() {
                                read_config_file(&p, depth + 1, out);
                            }
                        }
                    }
                }
            }
            "host" => {
                flush_block(&mut current, out);
                // Host 可跟多个模式，逐个生成条目；含通配符/否定的跳过
                let aliases: Vec<String> = value
                    .split_whitespace()
                    .filter(|a| !a.contains('*') && !a.contains('?') && !a.starts_with('!'))
                    .map(|a| a.to_string())
                    .collect();
                if !aliases.is_empty() {
                    current = Some((aliases, HostBlock::new()));
                }
            }
            "match" => {
                // Match 块无法静态解析，终止当前 Host 块
                flush_block(&mut current, out);
            }
            _ => {
                if let Some((_, block)) = current.as_mut() {
                    block.entry(key).or_insert_with(|| value.to_string());
                }
            }
        }
    }
    flush_block(&mut current, out);
}

/// 把当前 Host 块按别名展开为多条 (alias, block) 推入结果
fn flush_block(current: &mut Option<(Vec<String>, HostBlock)>, out: &mut Vec<(String, HostBlock)>) {
    if let Some((aliases, block)) = current.take() {
        for alias in aliases {
            out.push((alias, block.clone()));
        }
    }
}

/// 导入 ~/.ssh/config，转换为只读 Profile 列表（id 前缀 "sshcfg:"）
pub fn import() -> Vec<Profile> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let mut blocks = Vec::new();
    read_config_file(&home.join(".ssh").join("config"), 0, &mut blocks);

    blocks
        .into_iter()
        .map(|(alias, block)| {
            let host = block
                .get("hostname")
                .cloned()
                .unwrap_or_else(|| alias.clone());
            let port = block
                .get("port")
                .and_then(|p| p.parse().ok())
                .unwrap_or(22);
            let user = block
                .get("user")
                .cloned()
                .unwrap_or_else(|| std::env::var("USER").unwrap_or_else(|_| "root".into()));
            let key_path = block.get("identityfile").map(|p| {
                expand_tilde(p.split_whitespace().next().unwrap_or(p))
                    .to_string_lossy()
                    .into_owned()
            });
            Profile {
                id: format!("sshcfg:{alias}"),
                name: alias,
                host,
                port,
                user,
                auth: if key_path.is_some() {
                    "key".into()
                } else {
                    "password".into()
                },
                key_path,
                key_data: None,
                source: "ssh_config".into(),
                note: None,
                keepalive: None,
                timeout: None,
            }
        })
        .collect()
}
