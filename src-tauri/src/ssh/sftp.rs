//! SFTP 能力：悬停元信息(stat)、双击预览(限长读取)、右键下载、拖拽/点击上传。
//! SFTP 会话在连接上懒创建并缓存，连接重建后自动失效重建。

use std::sync::Arc;

use base64::Engine;
use russh_sftp::client::SftpSession;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;

use crate::error::{Error, Result};
use crate::ssh::conn::Connection;

// ---------- 传输控制（暂停/继续/取消）----------

pub const T_RUNNING: u8 = 0;
pub const T_PAUSED: u8 = 1;
pub const T_CANCELLED: u8 = 2;

/// 单次传输的控制句柄：存于 AppState.transfers（按 transfer_id 索引），
/// 由 transfer_pause/resume/cancel 命令改写状态，传输循环在每块前过 `gate` 响应。
/// 用 watch 通道而非 Notify，天然避免「检查—唤醒」之间的丢失唤醒。
pub struct TransferCtl {
    tx: watch::Sender<u8>,
}

impl TransferCtl {
    pub fn new() -> Self {
        Self {
            tx: watch::channel(T_RUNNING).0,
        }
    }

    /// 状态跃迁：allow_from 指定的当前态才可切到 to（None 表示除 to 外的任意态皆可）。
    fn transition(&self, allow_from: Option<u8>, to: u8) {
        self.tx.send_if_modified(|s| {
            let ok = allow_from.map_or(*s != to, |f| *s == f);
            if ok {
                *s = to;
            }
            ok
        });
    }

    /// 暂停（仅运行态生效，不覆盖已取消）
    pub fn pause(&self) {
        self.transition(Some(T_RUNNING), T_PAUSED);
    }

    /// 继续（仅暂停态生效）
    pub fn resume(&self) {
        self.transition(Some(T_PAUSED), T_RUNNING);
    }

    /// 取消（终态，不可逆；任意非取消态皆可切入）
    pub fn cancel(&self) {
        self.transition(None, T_CANCELLED);
    }

    /// 传输循环闸门：运行→立即放行；暂停→挂起等待；取消→返回错误中止本次传输。
    async fn gate(&self) -> Result<()> {
        // 快路径：运行态零开销放行（绝大多数块走这里）
        match *self.tx.borrow() {
            T_CANCELLED => return Err(Error::Cancelled),
            T_RUNNING => return Ok(()),
            _ => {}
        }
        // 慢路径：已暂停，订阅后等待恢复或取消
        let mut rx = self.tx.subscribe();
        loop {
            match *rx.borrow_and_update() {
                T_CANCELLED => return Err(Error::Cancelled),
                T_RUNNING => return Ok(()),
                _ => {}
            }
            if rx.changed().await.is_err() {
                return Err(Error::Cancelled); // 发送端被丢弃，视同取消
            }
        }
    }
}

impl Default for TransferCtl {
    fn default() -> Self {
        Self::new()
    }
}

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
pub struct RemoteEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_link: bool,
    pub size: u64,
    pub mtime: Option<u32>,
}

/// 列出远端目录条目（read_dir 迭代器已自动跳过 . 与 ..）。
/// 排序：目录在前，随后按名称不区分大小写升序，观感与本地面板一致。
pub async fn list(conn: &Arc<Connection>, path: &str) -> Result<Vec<RemoteEntry>> {
    let sftp = session(conn).await?;
    let rd = match sftp.read_dir(path).await {
        Ok(rd) => rd,
        Err(e) => {
            invalidate(conn).await;
            return Err(e.into());
        }
    };
    let mut out: Vec<RemoteEntry> = rd
        .map(|entry| {
            let attrs = entry.metadata();
            let mode = attrs.permissions.unwrap_or(0);
            RemoteEntry {
                name: entry.file_name(),
                path: entry.path(),
                is_dir: attrs.is_dir(),
                is_link: mode & 0o170000 == 0o120000,
                size: attrs.size.unwrap_or(0),
                mtime: attrs.mtime,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
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
    ctl: &TransferCtl,
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
        // 每块前过闸门：暂停则挂起，取消则中止并清理半成品文件
        if let Err(e) = ctl.gate().await {
            drop(dst);
            let _ = tokio::fs::remove_file(local).await;
            return Err(e);
        }
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
    ctl: &TransferCtl,
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
        // 每块前过闸门：暂停则挂起，取消则中止并清理半成品远端文件
        if let Err(e) = ctl.gate().await {
            let _ = dst.shutdown().await;
            drop(dst);
            let _ = sftp.remove_file(remote).await;
            return Err(e);
        }
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
    ctl: &TransferCtl,
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
            download_file(app, &sftp, ctl, remote, local, transfer_id, &name, &mut done, total).await?;
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
            match download_file(app, &sftp, ctl, rpath, &format!("{root}/{rel}"), transfer_id, &label, &mut done, total).await {
                Ok(()) => {}
                // 取消：回滚整棵已下载的目录树，不留半成品
                Err(Error::Cancelled) => {
                    let _ = tokio::fs::remove_dir_all(&root).await;
                    return Err(Error::Cancelled);
                }
                Err(e) => return Err(e),
            }
        }
        emit_progress(app, transfer_id, &name, done, done.max(total), "download");
        Ok(())
    }
    .await;
    // 取消是用户主动行为，会话仍健康——只在真实故障时才失效缓存重建
    if matches!(&result, Err(e) if !matches!(e, Error::Cancelled)) {
        invalidate(conn).await;
    }
    result
}

/// 上传（文件或整个目录，目录递归处理），返回远端根路径
pub async fn upload(
    app: &AppHandle,
    conn: &Arc<Connection>,
    ctl: &TransferCtl,
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
            upload_file(app, &sftp, ctl, local, &remote_root, transfer_id, &name, &mut done, total).await?;
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
            match upload_file(app, &sftp, ctl, lpath, &format!("{remote_root}/{rel}"), transfer_id, &label, &mut done, total).await {
                Ok(()) => {}
                // 取消：best-effort 回滚远端已建的树——先删文件，再逆序删目录，最后删根
                Err(Error::Cancelled) => {
                    for (_, r) in &files {
                        let _ = sftp.remove_file(format!("{remote_root}/{r}")).await;
                    }
                    for d in dirs.iter().rev() {
                        let _ = sftp.remove_dir(format!("{remote_root}/{d}")).await;
                    }
                    let _ = sftp.remove_dir(remote_root.clone()).await;
                    return Err(Error::Cancelled);
                }
                Err(e) => return Err(e),
            }
        }
        emit_progress(app, transfer_id, &name, done, total, "upload");
        Ok(())
    }
    .await;
    match result {
        Ok(()) => Ok(remote_root),
        Err(e) => {
            // 取消不使会话失效（会话仍健康），仅真实故障才失效重建
            if !matches!(e, Error::Cancelled) {
                invalidate(conn).await;
            }
            Err(e)
        }
    }
}

/// 通过 /proc/<pid>/cwd 解析远端 shell 的实时工作目录（Linux）。
/// realpath 解析该符号链接即得 shell 当前 cwd，无需 OSC7 或持续上报。
pub async fn proc_cwd(conn: &Arc<Connection>, pid: u32) -> Result<String> {
    let sftp = session(conn).await?;
    match sftp.canonicalize(format!("/proc/{pid}/cwd")).await {
        Ok(p) => Ok(p),
        Err(e) => {
            invalidate(conn).await;
            Err(e.into())
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
