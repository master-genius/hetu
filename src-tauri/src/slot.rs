//! 多进程 slot 分配：每个 hetushell 进程启动时 acquire 一个 slot，
//! 持有对应锁文件的 flock 到进程退出，崩溃内核自动释放。
//! session 按 slot 分片持久化（session-<slot>.json），互不覆盖。

use std::fs::OpenOptions;

use crate::error::{Error, Result};
use crate::settings;

/// 分配最小可用 slot：从 0 开始遍历 `slots/<N>.lock`，`try_lock_exclusive`
/// 第一个成功即返回 `(slot, file)`。file 必须持有到进程退出（存入 AppState 防 drop），
/// 进程退出（含崩溃、kill -9）内核自动释放 flock，下次启动可重新 acquire。
///
/// 语义 A（最小可用 slot）：关掉 slot 0 后新实例会接力恢复 slot 0 的 session。
pub fn acquire_slot() -> Result<(usize, std::fs::File)> {
    let dir = settings::config_dir()?.join("slots");
    std::fs::create_dir_all(&dir)?;
    for slot in 0..u32::MAX {
        let path = dir.join(format!("{}.lock", slot));
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .open(&path)?;
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => return Ok((slot as usize, file)),
            Err(_) => continue, // 该 slot 被占用，试下一个
        }
    }
    Err(Error::msg("无法分配 slot（已耗尽 u32 空间）"))
}
