//! SFTP 能力：悬停元信息(stat)、双击预览(限长读取)、右键下载、拖拽/点击上传。
//! SFTP 会话在连接上懒创建并缓存，连接重建后自动失效重建。

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use russh::ChannelMsg;
use russh_sftp::client::SftpSession;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;

use crate::error::{Error, Result};
use crate::ssh::conn::Connection;

/// SFTP 单次操作超时：防止连接半断开（TCP 未检测到）时操作永久挂起
const SFTP_TIMEOUT: Duration = Duration::from_secs(10);

/// SFTP 操作包装宏：带超时 + 失败重试一次。
/// 首次失败（错误或超时）→ invalidate session → 重建 → 重试一次。
/// $body 须返回 Result<T, E>（E: Into<Error>）。
macro_rules! sftp_with_retry {
    ($conn:expr, $sftp:ident, $body:expr) => {{
        let $sftp = session($conn).await?;
        match tokio::time::timeout(SFTP_TIMEOUT, async { $body }).await {
            Ok(Ok(v)) => v,
            _ => {
                // 失败或超时：invalidate + 重建 session + 重试一次
                invalidate($conn).await;
                let $sftp = session($conn).await?;
                match tokio::time::timeout(SFTP_TIMEOUT, async { $body }).await {
                    Ok(Ok(v)) => v,
                    Ok(Err(e)) => return Err(e.into()),
                    Err(_) => return Err(Error::msg("SFTP 操作超时（超过 10 秒）")),
                }
            }
        }
    }};
}

// ---------- 传输控制（暂停/继续/取消）----------

pub const T_RUNNING: u8 = 0;
pub const T_PAUSED: u8 = 1;
pub const T_CANCELLED: u8 = 2;

/// 单次传输的控制句柄：存于 AppState.transfers（按 transfer_id 索引），
/// 由 transfer_pause/resume/cancel 命令改写状态，传输循环在每块前过 `gate` 响应。
/// 用 watch 通道而非 Notify，天然避免「检查—唤醒」之间的丢失唤醒。
pub struct TransferCtl {
    /// 关联的连接 id：用于查询连接是否有活跃传输，ssh_disconnect 时保护
    conn_id: String,
    tx: watch::Sender<u8>,
}

impl TransferCtl {
    pub fn new(conn_id: String) -> Self {
        Self {
            conn_id,
            tx: watch::channel(T_RUNNING).0,
        }
    }

    pub fn conn_id(&self) -> &str {
        &self.conn_id
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

    /// 取消信号：状态变为「已取消」时 resolve（用于 select! 抢占阻塞中的网络读写，
    /// 使取消无需等当前 64KB 读/写完成或 TCP 超时即可立即中止）。
    async fn cancelled(&self) {
        let mut rx = self.tx.subscribe();
        loop {
            if *rx.borrow_and_update() == T_CANCELLED {
                return;
            }
            if rx.changed().await.is_err() {
                return; // 发送端被丢弃，视同取消
            }
        }
    }

    /// 让一段远端操作可被取消抢占：取消时立即返回 Err(Cancelled)，不等操作完成。
    /// 用于数据块读写之外的慢阶段（目录树枚举、骨架创建），使取消全程即时。
    async fn preempt<T>(&self, fut: impl std::future::Future<Output = Result<T>>) -> Result<T> {
        tokio::select! {
            biased;
            _ = self.cancelled() => Err(Error::Cancelled),
            r = fut => r,
        }
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

/// 获取（或懒创建）该连接的 SFTP 会话
pub async fn session(conn: &Arc<Connection>) -> Result<Arc<SftpSession>> {
    let mut guard = conn.sftp.lock().await;
    if let Some(s) = guard.as_ref() {
        // 连接已断时主动失效缓存 session，避免用失效 session 操作后才失败
        if !conn.is_alive().await {
            *guard = None;
        } else {
            return Ok(s.clone());
        }
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
    let attrs = sftp_with_retry!(conn, sftp, {
        sftp.metadata(path).await
    });
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
    let rd = sftp_with_retry!(conn, sftp, {
        sftp.read_dir(path).await
    });
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

/// 整读远端文件（图片预览缓存用）。超过 limit 直接报错，避免大文件驻留内存。
pub async fn read_file_bytes(conn: &Arc<Connection>, path: &str, limit: u64) -> Result<Vec<u8>> {
    let sftp = session(conn).await?;
    let result = async {
        let meta = sftp.metadata(path).await?;
        let size = meta.size.unwrap_or(0);
        if size > limit {
            return Err(Error::msg(format!("文件过大（{size} 字节）")));
        }
        let mut file = sftp.open(path).await?;
        let mut buf = Vec::with_capacity(size as usize);
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let n = file.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            // stat 与读取之间文件可能被写大——按实际读取量二次设限
            if buf.len() as u64 > limit {
                return Err(Error::msg("文件在读取过程中超过大小上限"));
            }
        }
        Ok(buf)
    }
    .await;
    // 与 preview 一致：失败统一失效缓存会话（业务性失败也失效无害，下次自动重建）
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
    // 256KB 块：减少 SFTP read 请求往返次数（64KB→256KB 约减 4 倍往返）
    let mut buf = vec![0u8; 256 * 1024];
    // 进度节流：每 1MB 发一次，避免高频 IPC 拖慢传输（100MB 文件 1600 次→100 次）
    let mut last_emit = 0u64;
    const PROGRESS_INTERVAL: u64 = 1024 * 1024;
    loop {
        // 每块前过闸门：暂停则挂起，取消则中止并清理半成品文件
        if let Err(e) = ctl.gate().await {
            drop(dst);
            let _ = tokio::fs::remove_file(local).await;
            return Err(e);
        }
        // 抢占式取消：读阻塞在慢/停滞的网络上时，取消无需等这次读完成即可中止
        let n = tokio::select! {
            biased;
            _ = ctl.cancelled() => {
                drop(dst);
                let _ = tokio::fs::remove_file(local).await;
                return Err(Error::Cancelled);
            }
            r = src.read(&mut buf) => r?,
        };
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n]).await?;
        *done += n as u64;
        if *done >= last_emit + PROGRESS_INTERVAL {
            emit_progress(app, transfer_id, label, *done, total, "download");
            last_emit = *done;
        }
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
    // 256KB 块 + 进度节流（与 download_file 一致）
    let mut buf = vec![0u8; 256 * 1024];
    let mut last_emit = 0u64;
    const PROGRESS_INTERVAL: u64 = 1024 * 1024;
    loop {
        // 每块前过闸门：暂停则挂起，取消则中止并清理半成品远端文件
        if let Err(e) = ctl.gate().await {
            // 清理是远端操作，限时执行：网络停滞时不让取消路径自身挂起
            cleanup_bounded(async {
                let _ = dst.shutdown().await;
                drop(dst);
                let _ = sftp.remove_file(remote).await;
            })
            .await;
            return Err(e);
        }
        let n = src.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        // 抢占式取消：写阻塞在慢/停滞的远端网络上时，取消无需等这次写完成即可中止
        tokio::select! {
            biased;
            _ = ctl.cancelled() => {
                cleanup_bounded(async {
                    let _ = dst.shutdown().await;
                    drop(dst);
                    let _ = sftp.remove_file(remote).await;
                })
                .await;
                return Err(Error::Cancelled);
            }
            r = dst.write_all(&buf[..n]) => r?,
        }
        *done += n as u64;
        if *done >= last_emit + PROGRESS_INTERVAL {
            emit_progress(app, transfer_id, label, *done, total, "upload");
            last_emit = *done;
        }
    }
    dst.flush().await?;
    dst.shutdown().await?;
    Ok(())
}

fn is_dir_mode(mode: u32) -> bool {
    mode & 0o170000 == 0o040000
}

/// 取消后的远端清理：网络可能已停滞（这正是用户点取消的典型场景），限时 best-effort，
/// 超时放弃回滚——留半成品优于让取消路径挂死等 TCP 超时。
async fn cleanup_bounded(fut: impl std::future::Future<Output = ()>) {
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), fut).await;
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
        let meta = ctl.preempt(async { Ok(sftp.metadata(remote).await?) }).await?;
        let mut done = 0u64;

        if !is_dir_mode(meta.permissions.unwrap_or(0)) {
            let total = meta.size.unwrap_or(0);
            download_file(app, &sftp, ctl, remote, local, transfer_id, &name, &mut done, total).await?;
            emit_progress(app, transfer_id, &name, done, done.max(total), "download");
            return Ok(());
        }

        // 目录：local 是父目录，先建目录骨架再逐文件下载。
        // 枚举整棵远端树可能很慢（大目录/停滞网络），允许取消抢占
        let root = format!("{}/{}", local.trim_end_matches('/'), name);
        let (dirs, files, total) = ctl.preempt(walk_remote(&sftp, remote)).await?;
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

        // 目录：先建远端目录骨架（已存在则忽略错误），再逐文件上传。
        // 骨架创建是逐目录的远端调用，允许取消抢占（不回滚：目录可能本就存在）
        let (dirs, files, total) = walk_local(local_path)?;
        ctl.preempt(async {
            let _ = sftp.create_dir(&remote_root).await;
            for d in &dirs {
                let _ = sftp.create_dir(&format!("{remote_root}/{d}")).await;
            }
            Ok(())
        })
        .await?;
        let count = files.len();
        for (i, (lpath, rel)) in files.iter().enumerate() {
            let label = format!("{name}/{rel} ({}/{count})", i + 1);
            match upload_file(app, &sftp, ctl, lpath, &format!("{remote_root}/{rel}"), transfer_id, &label, &mut done, total).await {
                Ok(()) => {}
                // 取消：best-effort 回滚远端已建的树——先删文件，再逆序删目录，最后删根。
                // 回滚是 O(N) 次串行远端调用，整体限时：网络停滞/大目录时不让取消挂死
                Err(Error::Cancelled) => {
                    cleanup_bounded(async {
                        for (_, r) in &files {
                            let _ = sftp.remove_file(format!("{remote_root}/{r}")).await;
                        }
                        for d in dirs.iter().rev() {
                            let _ = sftp.remove_dir(format!("{remote_root}/{d}")).await;
                        }
                        let _ = sftp.remove_dir(remote_root.clone()).await;
                    })
                    .await;
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
    Ok(sftp_with_retry!(conn, sftp, {
        sftp.canonicalize(format!("/proc/{pid}/cwd")).await
    }))
}

/// 远端 home 目录（用于 cwd 兜底）
pub async fn home(conn: &Arc<Connection>) -> Result<String> {
    Ok(sftp_with_retry!(conn, sftp, {
        sftp.canonicalize(".").await
    }))
}

// ---------- 远程 → 远程复制（面板条目拖到远程终端）----------

/// POSIX shell 单引号转义：任意字节序列安全嵌入命令行（' → '\''）
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// 在连接上执行一条命令，返回 (退出码, stderr)。退出码 None 表示服务器未上报。
/// 抢占取消由调用方用 `ctl.preempt` 包裹：future 被 drop 时 channel 随之关闭，
/// 服务端（OpenSSH）会终止会话子进程，属 best-effort 中止。
async fn exec_status(conn: &Arc<Connection>, cmd: &str) -> Result<(Option<u32>, String)> {
    let mut channel = {
        let guard = conn.handle.lock().await;
        let handle = guard.as_ref().ok_or_else(|| Error::msg("连接未建立"))?;
        handle.channel_open_session().await?
    };
    channel.exec(true, cmd).await?;
    let mut code = None;
    let mut stderr = Vec::new();
    while let Some(msg) = channel.wait().await {
        match msg {
            ChannelMsg::ExtendedData { data, .. } => stderr.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => code = Some(exit_status),
            _ => {}
        }
    }
    Ok((code, String::from_utf8_lossy(&stderr).into_owned()))
}

/// 同连接快路径：服务器内 `cp -a`，零下行/上行带宽。
/// Ok(true)=已复制；Ok(false)=快路径不可用（无 cp / exec 失败），调用方回退流式；
/// Err=真实失败（权限/空间等，流式多半也会失败，直接上报）或已取消。
async fn try_server_side_cp(
    conn: &Arc<Connection>,
    ctl: &TransferCtl,
    src: &str,
    dst_dir: &str,
) -> Result<bool> {
    let cmd = format!("cp -a -- {} {}/", sh_quote(src), sh_quote(dst_dir));
    let (code, stderr) = match ctl.preempt(exec_status(conn, &cmd)).await {
        Ok(r) => r,
        Err(Error::Cancelled) => return Err(Error::Cancelled),
        Err(_) => return Ok(false), // exec 通道打开/请求失败 → 回退流式
    };
    match code {
        Some(0) => Ok(true),
        // 126/127（cp 不可执行/不存在，如 Windows sshd）或未上报退出码 → 回退流式
        Some(126) | Some(127) | None => Ok(false),
        Some(c) => Err(Error::msg(format!(
            "服务器内复制失败（退出码 {c}）: {}",
            stderr.trim()
        ))),
    }
}

/// 跨会话单文件流式复制：src 连接读 → dst 连接写，不落本地盘。
/// 先写临时名、全部写完再 rename 覆盖——即使目标与源是同一物理文件
/// （不同连接名指向同一服务器的未检出别名），也绝不会发生「先截断后读空」的数据丢失。
#[allow(clippy::too_many_arguments)]
async fn copy_file_between(
    app: &AppHandle,
    src_sftp: &SftpSession,
    dst_sftp: &SftpSession,
    ctl: &TransferCtl,
    src: &str,
    dst: &str,
    transfer_id: &str,
    label: &str,
    done: &mut u64,
    total: u64,
) -> Result<()> {
    let mut sf = src_sftp.open(src).await?;
    // 临时名带 transfer_id 前 8 位，避免并发复制到同一目标时互相踩踏
    let tmp = format!("{dst}.{}.part", &transfer_id[..8.min(transfer_id.len())]);
    let mut df = dst_sftp.create(&tmp).await?;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        // 每块前过闸门（暂停/取消），取消时限时清理目标端临时文件
        if let Err(e) = ctl.gate().await {
            cleanup_bounded(async {
                let _ = df.shutdown().await;
                drop(df);
                let _ = dst_sftp.remove_file(&tmp).await;
            })
            .await;
            return Err(e);
        }
        // 读/写均可被取消抢占（网络停滞时不等当前块完成）
        let n = tokio::select! {
            biased;
            _ = ctl.cancelled() => {
                cleanup_bounded(async {
                    let _ = df.shutdown().await;
                    drop(df);
                    let _ = dst_sftp.remove_file(&tmp).await;
                })
                .await;
                return Err(Error::Cancelled);
            }
            r = sf.read(&mut buf) => r?,
        };
        if n == 0 {
            break;
        }
        tokio::select! {
            biased;
            _ = ctl.cancelled() => {
                cleanup_bounded(async {
                    let _ = df.shutdown().await;
                    drop(df);
                    let _ = dst_sftp.remove_file(&tmp).await;
                })
                .await;
                return Err(Error::Cancelled);
            }
            r = df.write_all(&buf[..n]) => r?,
        }
        *done += n as u64;
        emit_progress(app, transfer_id, label, *done, total, "upload");
    }
    df.flush().await?;
    df.shutdown().await?;
    drop(df);
    // SFTP RENAME 在目标已存在时会失败：先删旧目标（不存在则忽略）再改名。
    // 源已完整读入临时文件，即便目标恰为源本身（别名），此序列也只是原地等价替换。
    let _ = dst_sftp.remove_file(dst).await;
    dst_sftp.rename(&tmp, dst).await?;
    Ok(())
}

/// 远程 → 远程复制（文件或整个目录）：
/// - 同一连接：优先服务器内 `cp -a`（零中转带宽），cp 不可用时回退流式；
/// - 跨连接：经客户端流式中转（src 读 → dst 写），不落本地盘。
/// 数据安全：两端路径先 canonicalize 消除符号链接别名；同一服务器
/// （同连接，或不同连接但 host:port 相同）上拒绝「复制到自身」与「目录复制进自己的子树」。
/// 目录复制被取消时**不回滚**已复制部分：目标树可能与源存在未检出的别名重叠，
/// 批量删除是全功能中风险最高的操作——保留半成品优于任何误删源数据的可能。
pub async fn copy_remote(
    app: &AppHandle,
    src_conn: &Arc<Connection>,
    dst_conn: &Arc<Connection>,
    ctl: &TransferCtl,
    src_path: &str,
    dst_dir: &str,
    transfer_id: &str,
) -> Result<String> {
    let src_sftp = session(src_conn).await?;
    let dst_sftp = session(dst_conn).await?;
    let result = async {
        // 规范化两端路径（消除 symlink 别名），数据安全判定以规范路径为准
        let canon_src = ctl
            .preempt(async { Ok(src_sftp.canonicalize(src_path).await?) })
            .await?;
        let canon_dst = ctl
            .preempt(async { Ok(dst_sftp.canonicalize(dst_dir).await?) })
            .await?;
        let name = canon_src
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            return Err(Error::msg("不支持复制根目录"));
        }
        let dst_root = format!("{}/{}", canon_dst.trim_end_matches('/'), name);
        let meta = ctl
            .preempt(async { Ok(src_sftp.metadata(&canon_src).await?) })
            .await?;
        let is_dir = is_dir_mode(meta.permissions.unwrap_or(0));

        // 同一服务器判定：同连接必然同服务器；不同连接按 host:port 相同兜底
        let same_conn = src_conn.id == dst_conn.id;
        let same_server = same_conn
            || (src_conn.params.host == dst_conn.params.host
                && src_conn.params.port == dst_conn.params.port);
        if same_server {
            if dst_root == canon_src {
                return Err(Error::msg("目标与源相同，无需复制"));
            }
            if is_dir && (canon_dst == canon_src || canon_dst.starts_with(&format!("{canon_src}/"))) {
                return Err(Error::msg("不能把目录复制到它自己的子目录中"));
            }
        }

        // 快路径：同一连接 → 服务器内 cp（仅同连接才 100% 确定两个路径在同一台机器上）
        if same_conn {
            match try_server_side_cp(src_conn, ctl, &canon_src, &canon_dst).await? {
                true => {
                    let total = if is_dir { 0 } else { meta.size.unwrap_or(0) };
                    emit_progress(app, transfer_id, &name, total, total, "upload");
                    return Ok(dst_root);
                }
                false => {} // cp 不可用，落回流式
            }
        }

        // 流式中转
        let mut done = 0u64;
        if !is_dir {
            let total = meta.size.unwrap_or(0);
            copy_file_between(app, &src_sftp, &dst_sftp, ctl, &canon_src, &dst_root, transfer_id, &name, &mut done, total).await?;
            emit_progress(app, transfer_id, &name, done, done.max(total), "upload");
            return Ok(dst_root);
        }
        // 目录：走一遍源树 → 目标端建骨架 → 逐文件复制（骨架已存在则忽略，与上传一致）
        let (dirs, files, total) = ctl.preempt(walk_remote(&src_sftp, &canon_src)).await?;
        ctl.preempt(async {
            let _ = dst_sftp.create_dir(&dst_root).await;
            for d in &dirs {
                let _ = dst_sftp.create_dir(&format!("{dst_root}/{d}")).await;
            }
            Ok(())
        })
        .await?;
        let count = files.len();
        for (i, (spath, rel, _)) in files.iter().enumerate() {
            let label = format!("{name}/{rel} ({}/{count})", i + 1);
            copy_file_between(app, &src_sftp, &dst_sftp, ctl, spath, &format!("{dst_root}/{rel}"), transfer_id, &label, &mut done, total).await?;
        }
        emit_progress(app, transfer_id, &name, done, total, "upload");
        Ok(dst_root)
    }
    .await;
    // 取消是主动行为不失效会话；真实故障时两端会话都可能已坏，均失效待重建
    if matches!(&result, Err(e) if !matches!(e, Error::Cancelled)) {
        invalidate(src_conn).await;
        invalidate(dst_conn).await;
    }
    result
}
