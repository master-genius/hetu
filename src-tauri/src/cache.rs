//! 图片预览缓存：远端图片整读后落盘到系统缓存目录（XDG cache），
//! 以 (host, port, 路径) 哈希 + size + mtime 为键——远端文件变化即自然失效为新键，
//! 旧条目由 sweep 回收。
//!
//! 清理策略：应用内 tokio 后台任务定期调用 sweep()（lib.rs setup 中启动），
//! 先删过期条目，再按最旧优先压到总量限额。不用独立子进程：清理本身是毫秒级
//! 文件操作，进程内任务随应用生命周期启停，无孤儿进程、无 IPC、跨平台零差异。

use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
use serde::Serialize;

use crate::error::{Error, Result};
use crate::ssh::conn::Connection;

/// 单张图片上限：base64 后整体驻留内存并跨 IPC 传输，必须设硬上限
const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;
/// 缓存总量上限：超出后按最旧优先删除
const MAX_CACHE_BYTES: u64 = 200 * 1024 * 1024;
/// 条目最大寿命：7 天未再访问即清除
const MAX_AGE_SECS: u64 = 7 * 24 * 3600;
/// 半写残留（.part）超过 1 小时视为死条目（正常写入毫秒级完成）
const PART_STALE_SECS: u64 = 3600;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageData {
    pub data: String, // base64
    pub size: u64,
}

fn preview_dir() -> Result<PathBuf> {
    let dir = dirs::cache_dir()
        .ok_or_else(|| Error::msg("无法定位系统缓存目录"))?
        .join("hetushell")
        .join("preview");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn too_big(size: u64) -> Error {
    Error::msg(format!(
        "图片过大（{} MB，上限 {} MB），请下载后在本地查看",
        size / (1024 * 1024),
        MAX_IMAGE_BYTES / (1024 * 1024)
    ))
}

fn encode(bytes: Vec<u8>) -> ImageData {
    ImageData {
        size: bytes.len() as u64,
        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
    }
}

/// 本地图片：直接整读，无需缓存
pub fn local_image(path: &str) -> Result<ImageData> {
    let meta = std::fs::metadata(path)?;
    if meta.len() > MAX_IMAGE_BYTES {
        return Err(too_big(meta.len()));
    }
    Ok(encode(std::fs::read(path)?))
}

/// 远端图片：查磁盘缓存（键含 size+mtime，命中即免网络往返），未命中整读远端并落盘
pub async fn remote_image(conn: &Arc<Connection>, path: &str) -> Result<ImageData> {
    let meta = crate::ssh::sftp::stat(conn, path).await?;
    if meta.is_dir {
        return Err(Error::msg("目录不可作为图片预览"));
    }
    let size = meta.size.unwrap_or(0);
    if size > MAX_IMAGE_BYTES {
        return Err(too_big(size));
    }

    // DefaultHasher 在同一 Rust 版本内确定；跨版本变化的代价只是缓存失效重取，可接受
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&conn.params.host, conn.params.port, path).hash(&mut h);
        h.finish()
    };
    let file = preview_dir()?.join(format!(
        "{key:016x}-{size}-{}.img",
        meta.mtime.unwrap_or(0)
    ));
    if let Ok(bytes) = std::fs::read(&file) {
        if bytes.len() as u64 == size {
            return Ok(encode(bytes));
        }
    }

    let bytes = crate::ssh::sftp::read_file_bytes(conn, path, MAX_IMAGE_BYTES).await?;
    // 写缓存失败（磁盘满/权限）只损失缓存，不影响本次预览；temp+rename 防半写被命中
    let tmp = file.with_extension("part");
    if std::fs::write(&tmp, &bytes)
        .and_then(|_| std::fs::rename(&tmp, &file))
        .is_err()
    {
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(encode(bytes))
}

/// 清理缓存：删过期条目与死 .part，超出总量限额时按最旧优先删除。
/// 由 lib.rs 启动的应用内定时任务周期调用；全程容错，绝不 panic。
pub fn sweep() {
    let Ok(dir) = preview_dir() else { return };
    let Ok(rd) = std::fs::read_dir(&dir) else { return };
    let now = std::time::SystemTime::now();
    let age_of = |t: std::time::SystemTime| now.duration_since(t).map(|d| d.as_secs()).unwrap_or(0);

    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        let Ok(m) = e.metadata() else { continue };
        if !m.is_file() {
            continue;
        }
        let mtime = m.modified().unwrap_or(now);
        let stale_part =
            p.extension().is_some_and(|x| x == "part") && age_of(mtime) > PART_STALE_SECS;
        if age_of(mtime) > MAX_AGE_SECS || stale_part {
            let _ = std::fs::remove_file(&p);
            continue;
        }
        files.push((p, m.len(), mtime));
    }

    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= MAX_CACHE_BYTES {
        return;
    }
    files.sort_by_key(|f| f.2); // 最旧在前
    for (p, len, _) in files {
        if total <= MAX_CACHE_BYTES {
            break;
        }
        if std::fs::remove_file(&p).is_ok() {
            total -= len;
        }
    }
}
