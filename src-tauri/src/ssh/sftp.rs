//! SFTP 能力：悬停元信息(stat)、双击预览(限长读取)、右键下载、拖拽/点击上传。
//! SFTP 会话在连接上懒创建并缓存，连接重建后自动失效重建。

use std::sync::Arc;

use base64::Engine;
use russh_sftp::client::SftpSession;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::{Error, Result};
use crate::ssh::conn::Connection;

/// 获取（或懒创建）该连接的 SFTP 会话
pub async fn session(conn: &Arc<Connection>) -> Result<Arc<SftpSession>> {
    let mut guard = conn.sftp.lock().await;
    if let Some(s) = guard.as_ref() {
        return Ok(s.clone());
    }
    let channel = {
        let hguard = conn.handle.lock().await;
        let handle = hguard.as_ref().ok_or_else(|| Error::msg("连接未建立"))?;
        handle.channel_open_session().await?
    };
    channel.request_subsystem(true, "sftp").await?;
    let sftp = Arc::new(SftpSession::new(channel.into_stream()).await?);
    *guard = Some(sftp.clone());
    Ok(sftp)
}

/// SFTP 调用失败时使缓存会话失效，下次重建
pub async fn invalidate(conn: &Arc<Connection>) {
    *conn.sftp.lock().await = None;
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMeta {
    pub path: String,
    pub size: Option<u64>,
    pub is_dir: bool,
    pub is_link: bool,
    pub perms: String,
    pub mtime: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub owner: Option<String>,
    pub group: Option<String>,
}

/// mode → "rwxr-xr-x" 风格字符串
fn perms_string(mode: u32) -> String {
    let mut s = String::with_capacity(9);
    for shift in [6u32, 3, 0] {
        let bits = (mode >> shift) & 0o7;
        s.push(if bits & 0o4 != 0 { 'r' } else { '-' });
        s.push(if bits & 0o2 != 0 { 'w' } else { '-' });
        s.push(if bits & 0o1 != 0 { 'x' } else { '-' });
    }
    s
}

pub async fn stat(conn: &Arc<Connection>, path: &str) -> Result<FileMeta> {
    let sftp = session(conn).await?;
    let attrs = match sftp.metadata(path).await {
        Ok(a) => a,
        Err(e) => {
            // 会话可能已随断线失效；失效缓存后原样报错，前端静默处理
            invalidate(conn).await;
            return Err(e.into());
        }
    };
    let mode = attrs.permissions.unwrap_or(0);
    Ok(FileMeta {
        path: path.to_string(),
        size: attrs.size,
        is_dir: mode & 0o170000 == 0o040000,
        is_link: mode & 0o170000 == 0o120000,
        perms: perms_string(mode),
        mtime: attrs.mtime,
        uid: attrs.uid,
        gid: attrs.gid,
        owner: attrs.user.clone(),
        group: attrs.group.clone(),
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub data: String, // base64
    pub size: u64,
    pub truncated: bool,
    /// "text" | "image" | "binary"
    pub kind: String,
}

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico"];

/// 限长读取文件用于预览。图片按扩展名整读（受 max_bytes 上限保护）。
pub async fn preview(conn: &Arc<Connection>, path: &str, max_bytes: u64) -> Result<Preview> {
    let sftp = session(conn).await?;
    // 主体失败即失效缓存的 SFTP 会话，下次自动重建
    let result = async {
        let meta = sftp.metadata(path).await?;
        let size = meta.size.unwrap_or(0);

        let ext = path
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let is_image = IMAGE_EXTS.contains(&ext.as_str());

        let limit = if is_image {
            max_bytes.max(4 * 1024 * 1024) // 图片放宽到 4MB
        } else {
            max_bytes
        };
        if is_image && size > limit {
            return Err(Error::msg(format!("图片过大（{size} 字节），请右键下载后查看")));
        }

        let mut file = sftp.open(path).await?;
        let mut buf = Vec::with_capacity(limit.min(size.max(1)) as usize);
        let mut chunk = vec![0u8; 32 * 1024];
        while (buf.len() as u64) < limit {
            let n = file.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            let remain = (limit as usize).saturating_sub(buf.len());
            buf.extend_from_slice(&chunk[..n.min(remain)]);
        }

        let kind = if is_image {
            "image"
        } else if buf.iter().take(8192).any(|&b| b == 0) {
            "binary"
        } else {
            "text"
        };
        Ok(Preview {
            data: base64::engine::general_purpose::STANDARD.encode(&buf),
            size,
            truncated: (buf.len() as u64) < size,
            kind: kind.into(),
        })
    }
    .await;
    // 图片过大是业务校验错误，不代表会话失效——但统一失效也无害（下次自动重建）
    if result.is_err() {
        invalidate(conn).await;
    }
    result
}

/// 传输进度事件（上传/下载共用）
fn emit_progress(app: &AppHandle, id: &str, name: &str, done: u64, total: u64, dir: &str) {
    let _ = app.emit(
        "transfer-progress",
        serde_json::json!({
            "id": id, "name": name, "done": done, "total": total, "direction": dir,
        }),
    );
}

/// 单文件下载（追加进度到聚合计数）
async fn download_file(
    app: &AppHandle,
    sftp: &SftpSession,
    remote: &str,
    local: &str,
    transfer_id: &str,
    label: &str,
    done: &mut u64,
    total: u64,
) -> Result<()> {
    let mut src = sftp.open(remote).await?;
    let mut dst = tokio::fs::File::create(local).await?;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = src.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n]).await?;
        *done += n as u64;
        emit_progress(app, transfer_id, label, *done, total, "download");
    }
    dst.flush().await?;
    Ok(())
}

/// 单文件上传（追加进度到聚合计数）
async fn upload_file(
    app: &AppHandle,
    sftp: &SftpSession,
    local: &str,
    remote: &str,
    transfer_id: &str,
    label: &str,
    done: &mut u64,
    total: u64,
) -> Result<()> {
    let mut src = tokio::fs::File::open(local).await?;
    let mut dst = sftp.create(remote).await?;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = src.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n]).await?;
        *done += n as u64;
        emit_progress(app, transfer_id, label, *done, total, "upload");
    }
    dst.flush().await?;
    dst.shutdown().await?;
    Ok(())
}

fn is_dir_mode(mode: u32) -> bool {
    mode & 0o170000 == 0o040000
}

/// 目录条目名安全校验：拒绝 "."/".." 与含路径分隔符的名字。
/// 防止恶意/异常服务器通过 "../x" 让下载写到目标目录之外，或用 "." 造成无限递归。
fn is_safe_entry_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

/// 迭代遍历远端目录树 → (相对路径目录列表, (远端路径, 相对路径, 大小) 文件列表, 总字节)
async fn walk_remote(
    sftp: &SftpSession,
    base: &str,
) -> Result<(Vec<String>, Vec<(String, String, u64)>, u64)> {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    let mut total = 0u64;
    let mut stack = vec![String::new()]; // 相对路径栈
    while let Some(rel) = stack.pop() {
        let full = if rel.is_empty() {
            base.to_string()
        } else {
            format!("{base}/{rel}")
        };
        for entry in sftp.read_dir(&full).await? {
            let name = entry.file_name();
            if !is_safe_entry_name(&name) {
                continue; // 跳过 "."/".." 及含分隔符的异常名，防穿越/死循环
            }
            let child_rel = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            let attrs = entry.metadata();
            let mode = attrs.permissions.unwrap_or(0);
            if is_dir_mode(mode) {
                dirs.push(child_rel.clone());
                stack.push(child_rel);
            } else if mode & 0o170000 == 0o120000 {
                // 符号链接跳过，避免环
                continue;
            } else {
                let size = attrs.size.unwrap_or(0);
                total += size;
                files.push((format!("{base}/{child_rel}"), child_rel, size));
            }
        }
    }
    Ok((dirs, files, total))
}

/// 遍历本地目录树 → (相对路径目录列表, (本地路径, 相对路径) 文件列表, 总字节)
fn walk_local(base: &std::path::Path) -> Result<(Vec<String>, Vec<(String, String)>, u64)> {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    let mut total = 0u64;
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let meta = entry.metadata()?;
            let rel = path
                .strip_prefix(base)
                .map_err(|_| Error::msg("路径遍历异常"))?
                .to_string_lossy()
                .replace('\\', "/");
            if meta.is_symlink() {
                continue; // 符号链接跳过，避免环
            }
            if meta.is_dir() {
                dirs.push(rel);
                stack.push(path);
            } else {
                total += meta.len();
                files.push((path.to_string_lossy().into_owned(), rel));
            }
        }
    }
    Ok((dirs, files, total))
}

/// 下载（文件或整个目录，目录递归处理）。
/// remote 为目录时，local 视为目标父目录，在其下创建同名目录。
pub async fn download(
    app: &AppHandle,
    conn: &Arc<Connection>,
    remote: &str,
    local: &str,
    transfer_id: &str,
) -> Result<()> {
    let sftp = session(conn).await?;
    let name = remote.rsplit('/').next().unwrap_or(remote).to_string();
    // 主体失败即失效缓存的 SFTP 会话（会话可能已随连接中断而失效），下次自动重建
    let result = async {
        let meta = sftp.metadata(remote).await?;
        let mut done = 0u64;

        if !is_dir_mode(meta.permissions.unwrap_or(0)) {
            let total = meta.size.unwrap_or(0);
            download_file(app, &sftp, remote, local, transfer_id, &name, &mut done, total).await?;
            emit_progress(app, transfer_id, &name, done, done.max(total), "download");
            return Ok(());
        }

        // 目录：local 是父目录，先建目录骨架再逐文件下载
        let root = format!("{}/{}", local.trim_end_matches('/'), name);
        let (dirs, files, total) = walk_remote(&sftp, remote).await?;
        tokio::fs::create_dir_all(&root).await?;
        for d in &dirs {
            tokio::fs::create_dir_all(format!("{root}/{d}")).await?;
        }
        let count = files.len();
        for (i, (rpath, rel, _)) in files.iter().enumerate() {
            let label = format!("{name}/{rel} ({}/{count})", i + 1);
            download_file(app, &sftp, rpath, &format!("{root}/{rel}"), transfer_id, &label, &mut done, total).await?;
        }
        emit_progress(app, transfer_id, &name, done, done.max(total), "download");
        Ok(())
    }
    .await;
    if result.is_err() {
        invalidate(conn).await;
    }
    result
}

/// 上传（文件或整个目录，目录递归处理），返回远端根路径
pub async fn upload(
    app: &AppHandle,
    conn: &Arc<Connection>,
    local: &str,
    remote_dir: &str,
    transfer_id: &str,
) -> Result<String> {
    let local_path = std::path::Path::new(local);
    let name = local_path
        .file_name()
        .ok_or_else(|| Error::msg("无效的本地路径"))?
        .to_string_lossy()
        .into_owned();
    let remote_root = format!("{}/{}", remote_dir.trim_end_matches('/'), name);
    let sftp = session(conn).await?;
    // 主体失败即失效缓存的 SFTP 会话，下次自动重建
    let result = async {
        let meta = tokio::fs::metadata(local).await?;
        let mut done = 0u64;

        if !meta.is_dir() {
            let total = meta.len();
            upload_file(app, &sftp, local, &remote_root, transfer_id, &name, &mut done, total).await?;
            emit_progress(app, transfer_id, &name, done, total, "upload");
            return Ok(());
        }

        // 目录：先建远端目录骨架（已存在则忽略错误），再逐文件上传
        let (dirs, files, total) = walk_local(local_path)?;
        let _ = sftp.create_dir(&remote_root).await;
        for d in &dirs {
            let _ = sftp.create_dir(&format!("{remote_root}/{d}")).await;
        }
        let count = files.len();
        for (i, (lpath, rel)) in files.iter().enumerate() {
            let label = format!("{name}/{rel} ({}/{count})", i + 1);
            upload_file(app, &sftp, lpath, &format!("{remote_root}/{rel}"), transfer_id, &label, &mut done, total).await?;
        }
        emit_progress(app, transfer_id, &name, done, total, "upload");
        Ok(())
    }
    .await;
    match result {
        Ok(()) => Ok(remote_root),
        Err(e) => {
            invalidate(conn).await;
            Err(e)
        }
    }
}

/// 远端 home 目录（用于 cwd 兜底）
pub async fn home(conn: &Arc<Connection>) -> Result<String> {
    let sftp = session(conn).await?;
    match sftp.canonicalize(".").await {
        Ok(p) => Ok(p),
        Err(e) => {
            invalidate(conn).await;
            Err(e.into())
        }
    }
}
