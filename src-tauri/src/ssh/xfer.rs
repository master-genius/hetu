//! hsshx 专用文件搬运（--get / --put / --cp）。
//!
//! 为什么不与 `super::sftp` 共用传输入口：`download`/`upload`/`copy_remote` 的签名要求
//! `&AppHandle`（进度经 `transfer-progress` 事件广播给前端），独立 bin 没有 tauri 运行时，
//! 无法构造 AppHandle。故本模块只复用 sftp 的 pub 原语（`session`/`invalidate`/`stat`），
//! 搬运循环独立实现。walk / 条目判定的函数形状与 `super::sftp` 刻意对齐，便于将来收敛。
//!
//! 与 GUI 路径的四处有意差异：
//! - 取消：SIGINT 置位 `Cancel`，循环每块前检查（GUI 用 TransferCtl 的 watch 通道，gate 私有）
//! - 无 SFTP→exec 回退：子系统不可用直接报错，不重复 GUI 的 base64-over-exec 兜底
//! - 目标已存在默认拒绝（GUI 有前端确认框兜底，CLI 多为脚本无人值守，静默覆盖更危险）
//! - 覆盖既有目标走「临时名 → rename」，因此取消/失败时既有文件逐字节无损。
//!   GUI 的 download/upload 是直接 `create(最终名)`（O_TRUNC 先清空），取消时既有内容
//!   已经没了；目录回滚更用 remove_dir_all 连用户既有文件一起删。本模块修正了这一点，
//!   **GUI 侧的同源风险仍在**（按约定 sftp.rs 不改动）。

use std::io::IsTerminal;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use russh::ChannelMsg;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::FileAttributes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::conn::Connection;
use super::sftp;
use crate::error::{Error, Result};

/// 单次读写的块大小：与 GUI 路径一致，256KB 比 64KB 少约 4 倍 SFTP 往返。
const CHUNK: usize = 256 * 1024;

/// 元数据操作超时。sftp.rs 的同名常量为私有，此处独立取值保持一致语义：
/// 防止连接半断开（TCP 未检测到）时 stat/mkdir 永久挂起。
const META_TIMEOUT: Duration = Duration::from_secs(10);

/// 取消后清理半成品/回滚目录树的限时上限：网络停滞正是用户按 Ctrl-C 的典型场景，
/// 不能让用户再等一次 TCP 超时才能退出。
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

// ---------- 取消信号 ----------

/// SIGINT 取消标志。克隆共享同一个原子位：主线程挂信号处理器置位，搬运循环只读。
#[derive(Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn trigger(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    fn check(&self) -> Result<()> {
        if self.cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// 覆盖写临时名用的短标识：进程号 + 进程内递增序号，足以区分并发传输。
/// GUI 侧用 transfer_id 前 8 位做同样的事（见 super::sftp::copy_file_between）。
static PART_SEQ: AtomicU32 = AtomicU32::new(0);

fn transfer_tag() -> String {
    let n = PART_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{:x}-{:x}", std::process::id(), n)
}

// ---------- 进度输出 ----------

/// 进度输出器：**只写 stderr**。stdout 纯净是 hsshx 面向脚本/AI 的既有契约
/// （exec 模式下 stdout 是远程命令的输出，混入进度会让调用方无法解析）。
///
/// TTY 下整个任务只占一行：`desc` 与字节数/百分比/速率一起原地重绘，任务结束才换行——
/// 目录含上千文件时不会刷出上千行。非 TTY（重定向到日志/管道）完全静默，
/// 只在每个对象开始时打一行名字，便于事后核对传了哪些。
///
/// `total`/`done` 都是**整个任务**的聚合量（目录 = 整棵树），速率基准取自任务起点，
/// 因此不会在每个文件边界处跳回 0。
struct Progress {
    tty: bool,
    desc: String,
    total: u64,
    start: Instant,
    /// 刷新节流：慢链路每 256KB 回调一次会刷屏，限制到 ~10 次/秒
    last_render: Instant,
    /// 是否真的画出过进度帧：小文件一帧都没有，收尾就不该凭空换一行
    painted: bool,
}

impl Progress {
    fn new(total: u64) -> Self {
        let now = Instant::now();
        Self {
            tty: std::io::stderr().is_terminal(),
            desc: String::new(),
            total,
            start: now,
            last_render: now,
            painted: false,
        }
    }

    /// 标注当前正在搬运的对象（单文件名，或目录内 `路径 (第i/N个)`）。
    /// 不重置速率基准——那是任务级的量。
    fn label(&mut self, desc: &str) {
        self.desc = desc.to_string();
        if !self.tty {
            eprintln!("{desc}");
        }
    }

    /// 每写完一块调用一次；`done` 为聚合已传字节。
    fn tick(&mut self, done: u64) {
        if !self.tty || self.last_render.elapsed().as_millis() < 100 {
            return;
        }
        self.last_render = Instant::now();
        self.painted = true;
        let secs = self.start.elapsed().as_secs_f64().max(0.001);
        let pct = if self.total > 0 {
            (done as f64 * 100.0 / self.total as f64).min(100.0)
        } else {
            0.0
        };
        eprint!(
            "\r\x1b[2K{}  {}/{}  {pct:.1}%  {}/s    ",
            self.desc,
            human(done),
            human(self.total),
            human((done as f64 / secs) as u64),
        );
    }

    /// 任务收尾：TTY 下结束那一行进度（一帧都没画过则不产生任何输出）。
    /// 幂等——收尾后置位，保证 Drop 不会重复换行。
    fn finish(&mut self) {
        if self.tty && self.painted {
            eprintln!();
            self.painted = false;
        }
    }
}

impl Drop for Progress {
    /// 错误/取消路径靠它来收尾：进度器在 `?` 或提前 `return` 时离开作用域，
    /// 若该行还在原地刷新状态就必须换行，否则紧随其后的错误信息会拼在进度后面。
    /// 成功路径已显式 `finish()` 并把 painted 置位，故此处不会重复输出。
    fn drop(&mut self) {
        self.finish();
    }
}

/// 字节数转可读串（二进制单位，与终端习惯一致）。
fn human(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    format!("{v:.1} {}", UNITS[u])
}

// ---------- 路径与条目判定 ----------

fn is_dir_mode(mode: u32) -> bool {
    mode & 0o170000 == 0o040000
}

/// 目录条目名安全校验：拒绝 "."/".." 与含分隔符的名字。
/// 防止异常/恶意服务器用 "../x" 让下载写到目标目录之外，或用 "." 造成无限递归。
fn is_safe_entry_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

/// 远端路径 basename（POSIX 语义，不做本地路径分隔解释）。
fn base_name(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

/// `~/x` 展开为本地 home。只用于本地侧参数——远端路径交给远端 shell/SSHD 解释。
fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

// ---------- 目录树遍历 ----------

/// 树遍历中的单个文件：（源路径, 相对根目录的路径, 字节数）
type TreeFile = (String, String, u64);

/// 目录树遍历结果：（相对目录列表, 文件列表, 全部文件的总字节数）
type Tree = (Vec<String>, Vec<TreeFile>, u64);

/// 遍历本地目录树。逐目录检查取消，使大目录树也能即时中止。
fn walk_local(base: &Path, cancel: &Cancel) -> Result<Tree> {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    let mut total = 0u64;
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        cancel.check()?;
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
                let size = meta.len();
                total += size;
                files.push((path.to_string_lossy().into_owned(), rel, size));
            }
        }
    }
    Ok((dirs, files, total))
}

/// 遍历远端目录树。每次 read_dir 前检查取消——GUI 侧用 preempt 包住整个枚举使其可中断
/// （super::sftp::download），此处用同样的循环内检查达到同一语义：远端大目录枚举期间
/// Ctrl-C 也能即时生效，而不是等枚举跑完才有反应。
async fn walk_remote(sftp: &SftpSession, base: &str, cancel: &Cancel) -> Result<Tree> {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    let mut total = 0u64;
    let mut stack = vec![String::new()];
    while let Some(rel) = stack.pop() {
        cancel.check()?;
        let full = if rel.is_empty() {
            base.to_string()
        } else {
            format!("{base}/{rel}")
        };
        for entry in sftp.read_dir(&full).await? {
            let name = entry.file_name();
            if !is_safe_entry_name(&name) {
                continue;
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
                continue; // 符号链接跳过，避免环
            } else {
                let size = attrs.size.unwrap_or(0);
                total += size;
                files.push((format!("{base}/{child_rel}"), child_rel, size));
            }
        }
    }
    Ok((dirs, files, total))
}

// ---------- 权限保留（尽力而为，失败静默） ----------

/// 远端 mode → 本地文件权限。
#[cfg(unix)]
async fn preserve_to_local(sftp: &SftpSession, remote: &str, local: &str) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(Ok(m)) = tokio::time::timeout(META_TIMEOUT, sftp.metadata(remote)).await else {
        return;
    };
    let mode = m.permissions.map(|p| p & 0o777).unwrap_or(0);
    if mode != 0 {
        let _ = tokio::fs::set_permissions(local, std::fs::Permissions::from_mode(mode)).await;
    }
}

#[cfg(not(unix))]
async fn preserve_to_local(_sftp: &SftpSession, _remote: &str, _local: &str) {}

/// 本地 mode → 远端文件权限。
/// 必须用 `FileAttributes::empty()`：其 Default 是「dummy attrs」（size=0 等全字段填充），
/// 带进 SETSTAT 会把远端文件截断为 0 大小——同 super::sftp 的回归测试 dummy_default_would_truncate_file。
#[cfg(unix)]
async fn preserve_to_remote(sftp: &SftpSession, local: &str, remote: &str) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(m) = std::fs::metadata(local) else { return };
    let mode = m.permissions().mode() & 0o777;
    if mode == 0 {
        return;
    }
    let attrs = FileAttributes {
        permissions: Some(mode),
        ..FileAttributes::empty()
    };
    let _ = tokio::time::timeout(META_TIMEOUT, sftp.set_metadata(remote, attrs)).await;
}

#[cfg(not(unix))]
async fn preserve_to_remote(_sftp: &SftpSession, _local: &str, _remote: &str) {}

// ---------- exec 通道（仅 --cp 的同机快路径用） ----------

/// 在连接上执行命令，返回（退出码，stderr）。退出码 None 表示服务器未上报。
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

/// POSIX shell 单引号转义：任意字节序列安全嵌入命令行（' → '\''）。
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

// ---------- 存在性判定与覆盖保护 ----------

/// 远端目标的类型：None=不存在，Some(true)=目录，Some(false)=普通文件。
/// metadata 失败按「不存在」处理——父目录尚不存在时必然失败，而这正是允许写入的正常情况。
async fn remote_kind(sftp: &SftpSession, path: &str) -> Option<bool> {
    match tokio::time::timeout(META_TIMEOUT, sftp.metadata(path)).await {
        Ok(Ok(m)) => Some(is_dir_mode(m.permissions.unwrap_or(0))),
        _ => None,
    }
}

/// 本地路径是否为目录（不存在时为 false）。跟随符号链接，与 create_dir_all 的判据一致。
fn local_is_dir(path: &str) -> bool {
    Path::new(path).is_dir()
}

/// 已存在的目标若与本次要写入的类型不符（文件↔目录），提前报可读错误。
/// 不加这层判定的话，要等到 rename 阶段才炸出底层 errno（EISDIR/ENOTDIR），
/// 用户看不出是「目标是个目录」。
fn guard_kind(exists: bool, target: &str, want_dir: bool, is_dir: bool) -> Result<()> {
    if exists && want_dir != is_dir {
        let (a, b) = if want_dir {
            ("目录", "文件")
        } else {
            ("文件", "目录")
        };
        return Err(Error::msg(format!(
            "目标已存在且是{b}，无法替换为{a}: {target}"
        )));
    }
    Ok(())
}

fn local_exists(path: &str) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// 目标已存在且未授权覆盖 → 报错。CLI 没有确认框，宁可失败也不静默截断既有文件。
fn guard(force: bool, exists: bool, target: &str) -> Result<()> {
    if exists && !force {
        return Err(Error::msg(format!(
            "目标已存在: {target}（覆盖请加 --force）"
        )));
    }
    Ok(())
}

/// 建远端目录：仅在确认不存在时创建并报真错；已存在（--force 覆盖目录）则跳过，
/// 避免把「目录本就存在」误报成失败。
async fn ensure_remote_dir(sftp: &SftpSession, path: &str, absent: bool) -> Result<()> {
    if absent {
        sftp.create_dir(path.to_string()).await?;
    }
    Ok(())
}

// ---------- 下载 ----------

/// 取消后丢弃本地半成品。本地删除不受远端网络影响，无需限时。
async fn discard_local(dst: tokio::fs::File, path: &str) {
    drop(dst);
    let _ = tokio::fs::remove_file(path).await;
}

/// 限时执行清理：超时即放弃回滚——留半成品优于让清理路径挂死等 TCP 超时。
/// 网络停滞正是用户按 Ctrl-C 的典型场景，清理本身不能再成为新的阻塞点。
/// 清理序列固定为「shutdown → drop → remove_file」：SFTP 文件要先关闭数据才落盘，
/// 也才谈得上删除（与 super::sftp 的 upload_file/copy_file_between 同一写法）。
async fn cleanup_bounded(fut: impl std::future::Future<Output = ()>) {
    let _ = tokio::time::timeout(CLEANUP_TIMEOUT, fut).await;
}

/// 远端单文件 → 本地文件。
///
/// `overwrite=false`（目标不存在）：直接写最终名，取消/失败时删掉它——本次新建，
/// 删除不丢任何既有数据，行为与逐字节直写等价（零额外开销）。
/// `overwrite=true`（目标已存在且已获 --force 授权）：先写 `{最终名}.{tag}.part`，
/// 全部写完才删旧目标并 rename 覆盖。取消/失败只删临时名，**既有文件逐字节无损**
/// （与 copy_file 既有的临时名策略同构）。
/// 反例是原先的 `File::create`：O_TRUNC 在写第一字节之前就把既有文件清成 0 长度，
/// 此时无论取消时删不删，用户数据都已丢失。
async fn pull_file(
    sftp: &SftpSession,
    remote: &str,
    local: &str,
    overwrite: bool,
    cancel: &Cancel,
    prog: &mut Progress,
    done: &mut u64,
) -> Result<()> {
    let tmp = overwrite.then(|| format!("{local}.{}.part", transfer_tag()));
    let write_to = tmp.as_deref().unwrap_or(local);

    let mut src = sftp.open(remote).await?;
    let mut dst = tokio::fs::File::create(write_to).await?;
    let mut buf = vec![0u8; CHUNK];
    loop {
        // 取消必须在返回前清掉半成品：CLI 的核心卖点之一就是 Ctrl-C 不留脏文件
        if let Err(e) = cancel.check() {
            discard_local(dst, write_to).await;
            return Err(e);
        }
        let n = match src.read(&mut buf).await {
            Ok(n) => n,
            Err(e) => {
                discard_local(dst, write_to).await;
                return Err(e.into());
            }
        };
        if n == 0 {
            break;
        }
        if let Err(e) = dst.write_all(&buf[..n]).await {
            discard_local(dst, write_to).await;
            return Err(e.into());
        }
        *done += n as u64;
        prog.tick(*done);
    }
    dst.flush().await?;
    drop(dst);
    // 权限设在临时名上：rename 是同 inode 改名，权限随文件一起落到最终名
    preserve_to_local(sftp, remote, write_to).await;
    match tmp {
        None => Ok(()),
        Some(tmp) => commit_local(&tmp, local).await,
    }
}

/// 覆盖本地既有文件：删旧目标再 rename（与 copy_file 的远端序列一致）。
/// rename 失败时**保留临时名**——数据已完整写在里面，删它才是真的丢数据。
async fn commit_local(tmp: &str, final_path: &str) -> Result<()> {
    let _ = tokio::fs::remove_file(final_path).await;
    tokio::fs::rename(tmp, final_path).await.map_err(|e| {
        Error::msg(format!(
            "覆盖 {final_path} 失败: {e}；已完整传输的数据保留在 {tmp}，请手工移回"
        ))
    })
}

/// 覆盖远端既有文件：同上。SFTP RENAME 对已存在目标可能失败，故先删再改名。
async fn commit_remote(sftp: &SftpSession, tmp: &str, final_path: &str) -> Result<()> {
    let _ = sftp.remove_file(final_path).await;
    sftp.rename(tmp.to_string(), final_path.to_string())
        .await
        .map_err(|e| {
            Error::msg(format!(
                "覆盖 {final_path} 失败: {e}；已完整传输的数据保留在 {tmp}，请手工移回"
            ))
        })
}

/// 下载（文件或整棵目录）。
/// - remote 为文件：local 若是已存在目录 → 落到 `<local>/<basename>`，否则视为完整目标路径。
/// - remote 为目录：local 视为父目录，在其下创建同名目录。
pub async fn get(
    conn: &Arc<Connection>,
    remote: &str,
    local: &str,
    force: bool,
    cancel: &Cancel,
) -> Result<()> {
    let local = expand_tilde(local);
    let sftp = sftp::session(conn).await?;
    let name = base_name(remote).to_string();
    let result = async {
        let meta = sftp.metadata(remote).await?;
        let mut done = 0u64;

        if !is_dir_mode(meta.permissions.unwrap_or(0)) {
            let target = if local_is_dir(&local) {
                format!("{}/{}", local.trim_end_matches('/'), name)
            } else {
                local.clone()
            };
            let target_existed = local_exists(&target);
            guard(force, target_existed, &target)?;
            guard_kind(target_existed, &target, false, local_is_dir(&target))?;
            let total = meta.size.unwrap_or(0);
            let mut prog = Progress::new(total);
            prog.label(&format!("{name} → {target}"));
            // 目标已存在（--force 授权）→ 走临时名，取消/失败绝不触碰既有内容
            pull_file(&sftp, remote, &target, target_existed, cancel, &mut prog, &mut done).await?;
            prog.finish();
            return Ok(());
        }

        // 目录：local 是父目录，先建骨架再逐文件下载
        let root = format!("{}/{}", local.trim_end_matches('/'), name);
        let root_existed = local_exists(&root);
        guard(force, root_existed, &root)?;
        guard_kind(root_existed, &root, true, local_is_dir(&root))?;
        let (dirs, files, total) = walk_remote(&sftp, remote, cancel).await?;
        let count = files.len();
        tokio::fs::create_dir_all(&root).await?;
        for d in &dirs {
            tokio::fs::create_dir_all(format!("{root}/{d}")).await?;
        }
        let mut prog = Progress::new(total);
        for (i, (rpath, rel, _)) in files.iter().enumerate() {
            prog.label(&format!("{name}/{rel} ({}/{count})", i + 1));
            // 覆盖目录（--force）时按文件决策：本地 stat 无网络成本，逐文件查存在性，
            // 只有确实已存在的文件才走临时名，新增文件直写最终名（省一次 rename）。
            let target = format!("{root}/{rel}");
            let overwrite = root_existed && local_exists(&target);
            if let Err(e) = pull_file(
                &sftp,
                rpath,
                &target,
                overwrite,
                cancel,
                &mut prog,
                &mut done,
            )
            .await
            {
                // 取消：root 是本次 mkdir 出来的 → 整树删掉不丢任何既有数据；
                // root 本就存在（--force）→ 什么都不删。此时每个文件的半成品都是临时名，
                // 已由 pull_file 自行清理，既有内容从未被触碰；批量删除既有目录树
                // 才是不可逆的丢数据。
                if matches!(e, Error::Cancelled) && !root_existed {
                    let _ = tokio::time::timeout(CLEANUP_TIMEOUT, tokio::fs::remove_dir_all(&root)).await;
                }
                return Err(e);
            }
        }
        prog.finish();
        Ok(())
    }
    .await;
    // 取消是用户主动行为，会话仍健康；只有真实故障才失效缓存 session 待重建
    if result.is_err() && !matches!(result, Err(Error::Cancelled)) {
        sftp::invalidate(conn).await;
    }
    result
}

// ---------- 上传 ----------

/// 本地单文件 → 远端文件。取消语义与 pull_file 完全对称：
/// `overwrite=false` 直写最终名并清理；`overwrite=true` 写临时名，全部写完才替换既有文件。
async fn push_file(
    sftp: &SftpSession,
    local: &str,
    remote: &str,
    overwrite: bool,
    cancel: &Cancel,
    prog: &mut Progress,
    done: &mut u64,
) -> Result<()> {
    let tmp = overwrite.then(|| format!("{remote}.{}.part", transfer_tag()));
    let write_to = tmp.as_deref().unwrap_or(remote);

    let mut src = tokio::fs::File::open(local).await?;
    let mut dst = sftp.create(write_to).await?;
    let mut buf = vec![0u8; CHUNK];
    loop {
        // 取消/失败必须清掉正在写的目标；写临时名时既有远端文件根本未被触碰
        if let Err(e) = cancel.check() {
            cleanup_bounded(async {
                let _ = dst.shutdown().await;
                drop(dst);
                let _ = sftp.remove_file(write_to).await;
            })
            .await;
            return Err(e);
        }
        let n = match src.read(&mut buf).await {
            Ok(n) => n,
            Err(e) => {
                cleanup_bounded(async {
                    let _ = dst.shutdown().await;
                    drop(dst);
                    let _ = sftp.remove_file(write_to).await;
                })
                .await;
                return Err(e.into());
            }
        };
        if n == 0 {
            break;
        }
        if let Err(e) = dst.write_all(&buf[..n]).await {
            cleanup_bounded(async {
                let _ = dst.shutdown().await;
                drop(dst);
                let _ = sftp.remove_file(write_to).await;
            })
            .await;
            return Err(e.into());
        }
        *done += n as u64;
        prog.tick(*done);
    }
    dst.flush().await?;
    dst.shutdown().await?;
    drop(dst);
    // 权限设在临时名上，rename 是同 inode 改名，权限随文件落到最终名
    preserve_to_remote(sftp, local, write_to).await;
    match tmp {
        None => Ok(()),
        Some(tmp) => commit_remote(sftp, &tmp, remote).await,
    }
}

/// 上传（文件或整棵目录）到远端目录 `remote_dir` 下，返回远端根路径。
pub async fn put(
    conn: &Arc<Connection>,
    local: &str,
    remote_dir: &str,
    force: bool,
    cancel: &Cancel,
) -> Result<String> {
    let local = expand_tilde(local);
    let local_path = Path::new(&local);
    let name = local_path
        .file_name()
        .ok_or_else(|| Error::msg("无效的本地路径"))?
        .to_string_lossy()
        .into_owned();
    let remote_root = format!("{}/{}", remote_dir.trim_end_matches('/'), name);
    let sftp = sftp::session(conn).await?;
    let result = async {
        // 落点父目录必须已存在：不会自动创建父级，缺失时底层只会炸出
        // 「SFTP 错误: No such file」，用户看不出是目录没建，故显式探测。
        match remote_kind(&sftp, remote_dir).await {
            Some(true) => {}
            Some(false) => {
                return Err(Error::msg(format!("远端路径不是目录: {remote_dir}")));
            }
            None => {
                return Err(Error::msg(format!(
                    "远端目录不存在: {remote_dir}（先创建它，或用 --ls 确认路径）"
                )));
            }
        }
        let meta = tokio::fs::metadata(local_path).await?;
        let kind = remote_kind(&sftp, &remote_root).await;
        let root_existed = kind.is_some();
        guard(force, root_existed, &remote_root)?;
        guard_kind(root_existed, &remote_root, meta.is_dir(), kind == Some(true))?;
        let mut done = 0u64;

        if !meta.is_dir() {
            let total = meta.len();
            let mut prog = Progress::new(total);
            prog.label(&format!("{name} → {remote_root}"));
            push_file(&sftp, &local, &remote_root, root_existed, cancel, &mut prog, &mut done).await?;
            prog.finish();
            return Ok(());
        }

        // 目录：先建远端骨架（子目录已存在则忽略），再逐文件上传
        let (dirs, files, total) = walk_local(local_path, cancel)?;
        let count = files.len();
        ensure_remote_dir(&sftp, &remote_root, !root_existed).await?;
        for d in &dirs {
            cancel.check()?;
            let _ = sftp.create_dir(format!("{remote_root}/{d}")).await;
        }
        // 覆盖目录（--force）时按文件决策是否走临时名：目标树先整体枚举一次
        // （read_dir 批量返回，一次往返取回大量条目），只有远端确实已存在的文件
        // 才付出 remove+rename 两次额外往返；新增文件直写最终名，零额外开销。
        // 相比按根目录存在与否一刀切，大目录覆盖场景可省约 2×文件数 次 SFTP 往返。
        let existing: std::collections::HashSet<String> = if root_existed {
            let (_, efiles, _) = walk_remote(&sftp, &remote_root, cancel).await?;
            efiles.into_iter().map(|(_, rel, _)| rel).collect()
        } else {
            std::collections::HashSet::new()
        };
        let mut prog = Progress::new(total);
        for (i, (lpath, rel, _)) in files.iter().enumerate() {
            prog.label(&format!("{name}/{rel} ({}/{count})", i + 1));
            if let Err(e) = push_file(
                &sftp,
                lpath,
                &format!("{remote_root}/{rel}"),
                existing.contains(rel.as_str()),
                cancel,
                &mut prog,
                &mut done,
            )
            .await
            {
                // 取消回滚的前提是「整棵树都属于本次新建」。root 本就存在时（--force 覆盖）
                // 树里可能有用户的既有内容，rollback 里的 remove_dir 会连带删掉空目录、
                // 已完成的文件则是本次覆盖写入的——删它等于删用户数据，故此处什么都不做。
                // 每个文件的半成品都是临时名，已由 push_file 自行清理。
                if matches!(e, Error::Cancelled) && !root_existed {
                    rollback_remote(&sftp, &remote_root, &dirs, &files).await;
                }
                return Err(e);
            }
        }
        prog.finish();
        Ok(())
    }
    .await;
    if result.is_err() && !matches!(result, Err(Error::Cancelled)) {
        sftp::invalidate(conn).await;
    }
    result.map(|()| remote_root)
}

/// 取消后回滚远端目录树：先删文件，再逆序删目录，最后删根。整体限时。
async fn rollback_remote(
    sftp: &SftpSession,
    root: &str,
    dirs: &[String],
    files: &[TreeFile],
) {
    let _ = tokio::time::timeout(
        CLEANUP_TIMEOUT,
        async {
            for (_, rel, _) in files {
                let _ = sftp.remove_file(format!("{root}/{rel}")).await;
            }
            for d in dirs.iter().rev() {
                let _ = sftp.remove_dir(format!("{root}/{d}")).await;
            }
            let _ = sftp.remove_dir(root.to_string()).await;
        },
    )
    .await;
}

// ---------- 远程 → 远程复制 ----------

/// src 连接读 → dst 连接写，不落本地盘。
/// 先写 `.part` 临时名、写完再 rename：即使目标与源是同一物理文件（不同连接名指向
/// 同一台服务器），也绝不会发生「先截断后读空」丢数据。
async fn copy_file(
    src_sftp: &SftpSession,
    dst_sftp: &SftpSession,
    src: &str,
    dst: &str,
    cancel: &Cancel,
    prog: &mut Progress,
    done: &mut u64,
) -> Result<()> {
    let mut sf = src_sftp.open(src).await?;
    // 临时名带进程唯一标识：两个传输进程指向同一目标时不会互相踩同一个 .part
    let tmp = format!("{dst}.{}.part", transfer_tag());
    let mut df = dst_sftp.create(&tmp).await?;
    let mut buf = vec![0u8; CHUNK];
    loop {
        if let Err(e) = cancel.check() {
            cleanup_bounded(async {
                let _ = df.shutdown().await;
                drop(df);
                let _ = dst_sftp.remove_file(tmp.as_str()).await;
            })
            .await;
            return Err(e);
        }
        let n = match sf.read(&mut buf).await {
            Ok(n) => n,
            Err(e) => {
                cleanup_bounded(async {
                    let _ = df.shutdown().await;
                    drop(df);
                    let _ = dst_sftp.remove_file(tmp.as_str()).await;
                })
                .await;
                return Err(e.into());
            }
        };
        if n == 0 {
            break;
        }
        if let Err(e) = df.write_all(&buf[..n]).await {
            cleanup_bounded(async {
                let _ = df.shutdown().await;
                drop(df);
                let _ = dst_sftp.remove_file(tmp.as_str()).await;
            })
            .await;
            return Err(e.into());
        }
        *done += n as u64;
        prog.tick(*done);
    }
    df.flush().await?;
    df.shutdown().await?;
    drop(df);
    commit_remote(dst_sftp, &tmp, dst).await
}

/// 远程 → 远程复制（文件或目录）。同一服务器优先 `cp -a`（零中转带宽），
/// 否则经本客户端流式中转，不落本地盘。
/// 目录复制被取消时**不回滚**：目标树可能与源存在未检出的别名重叠，
/// 批量删除是全功能中风险最高的操作——保留半成品优于任何误删源数据的可能
/// （与 super::sftp::copy_remote 同一取舍）。
pub async fn cp(
    src_conn: &Arc<Connection>,
    dst_conn: &Arc<Connection>,
    src_path: &str,
    dst_dir: &str,
    force: bool,
    cancel: &Cancel,
) -> Result<String> {
    let src_sftp = sftp::session(src_conn).await?;
    let dst_sftp = sftp::session(dst_conn).await?;
    let result = async {
        // 规范化两端路径（消除符号链接别名），数据安全判定以规范路径为准
        let canon_src = src_sftp.canonicalize(src_path).await?;
        let canon_dst = dst_sftp.canonicalize(dst_dir).await?;
        let name = base_name(&canon_src).to_string();
        if name.is_empty() {
            return Err(Error::msg("不支持复制根目录"));
        }
        let dst_root = format!("{}/{}", canon_dst.trim_end_matches('/'), name);
        let meta = src_sftp.metadata(&canon_src).await?;
        let is_dir = is_dir_mode(meta.permissions.unwrap_or(0));

        // 同一服务器判定：同连接必然同机；不同连接按 host:port 兜底
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
        let exists = {
            let kind = remote_kind(&dst_sftp, &dst_root).await;
            let exists = kind.is_some();
            guard(force, exists, &dst_root)?;
            guard_kind(exists, &dst_root, is_dir, kind == Some(true))?;
            exists
        };

        // 快路径：同一连接 → 服务器内 cp -a，不占任何上行/下行带宽
        if same_conn {
            let cmd = format!("cp -a -- {} {}/", sh_quote(&canon_src), sh_quote(&canon_dst));
            match exec_status(src_conn, &cmd).await {
                Ok((Some(0), _)) => return Ok(dst_root),
                // cp 不可执行/不存在（126/127，如 Windows sshd）或未上报退出码 → 回退流式
                Ok((Some(126), _)) | Ok((Some(127), _)) | Ok((None, _)) => {}
                Ok((Some(c), err)) => {
                    return Err(Error::msg(format!(
                        "服务器内复制失败（退出码 {c}）: {}",
                        err.trim()
                    )))
                }
                // exec 通道打不开属真实故障，流式路径同样依赖该连接，直接上报
                Err(e) => return Err(e),
            }
        }

        let mut done = 0u64;
        if !is_dir {
            let total = meta.size.unwrap_or(0);
            let mut prog = Progress::new(total);
            prog.label(&format!("{name} → {dst_root}"));
            copy_file(&src_sftp, &dst_sftp, &canon_src, &dst_root, cancel, &mut prog, &mut done).await?;
            prog.finish();
            return Ok(dst_root);
        }
        // 目录：只遍历一次，(dirs, files, total) 同时供骨架创建与进度分母使用
        let (dirs, files, total) = walk_remote(&src_sftp, &canon_src, cancel).await?;
        let count = files.len();
        ensure_remote_dir(&dst_sftp, &dst_root, !exists).await?;
        for d in &dirs {
            cancel.check()?;
            let _ = dst_sftp.create_dir(format!("{dst_root}/{d}")).await;
        }
        let mut prog = Progress::new(total);
        for (i, (spath, rel, _)) in files.iter().enumerate() {
            prog.label(&format!("{name}/{rel} ({}/{count})", i + 1));
            copy_file(&src_sftp, &dst_sftp, spath, &format!("{dst_root}/{rel}"), cancel, &mut prog, &mut done).await?;
        }
        prog.finish();
        Ok(dst_root)
    }
    .await;
    if result.is_err() && !matches!(result, Err(Error::Cancelled)) {
        sftp::invalidate(src_conn).await;
        sftp::invalidate(dst_conn).await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SETSTAT 只带权限位：若某天回归成 `..Default::default()`，size=0 会被写进
    /// 远端属性，把刚上传完的文件截断为 0 大小。
    #[test]
    fn attrs_only_carry_permissions() {
        let attrs = FileAttributes {
            permissions: Some(0o755),
            ..FileAttributes::empty()
        };
        assert_eq!(attrs.permissions, Some(0o755));
        assert_eq!(attrs.size, None, "SETSTAT 不得携带 size，否则远端文件被截断");
    }

    /// exec 快路径的命令注入安全：路径必须整体处于单引号字面内，
    /// 分号/命令替换/换行都不得逃逸成 shell 语法。
    #[test]
    fn sh_quote_contains_shell_metacharacters() {
        assert_eq!(sh_quote("/tmp/it's.txt"), "'/tmp/it'\\''s.txt'");
        assert_eq!(sh_quote("/tmp/x; rm -rf /"), "'/tmp/x; rm -rf /'");
        assert_eq!(sh_quote("/tmp/$(whoami)`id`"), "'/tmp/$(whoami)`id`'");
        assert_eq!(sh_quote("/tmp/a\nb"), "'/tmp/a\nb'");
    }

    /// 同机 cp 命令构造：源与目标各自独立加引号，元字符路径不能破坏命令边界。
    /// 判据是「把两段引号字面整体删掉后，只剩命令骨架」——即用户可控内容没有一处
    /// 泄漏到未加引号的位置。不依赖引号奇偶（`'\''` 转义会让奇偶切分失效）。
    #[test]
    fn cp_command_quotes_both_ends() {
        let qdst = sh_quote("/b/dir");
        let skeleton = "cp -a --  /";

        let qsrc = sh_quote("/a/x; touch pwned");
        let cmd = format!("cp -a -- {qsrc} {qdst}/");
        assert_eq!(cmd.replace(&qsrc, "").replace(&qdst, ""), skeleton);

        // 含单引号的路径同样不得逃逸：转义后仍是一段完整字面字符串
        let tricky = "/a/it's; rm -rf /";
        let qsrc = sh_quote(tricky);
        assert_eq!(qsrc, "'/a/it'\\''s; rm -rf /'");
        let cmd = format!("cp -a -- {qsrc} {qdst}/");
        assert_eq!(cmd.replace(&qsrc, "").replace(&qdst, ""), skeleton);
    }

    /// 恶意服务器条目名必须被拦下，否则下载可写到目标目录之外。
    #[test]
    fn unsafe_entry_names_rejected() {
        assert!(!is_safe_entry_name(".."));
        assert!(!is_safe_entry_name("."));
        assert!(!is_safe_entry_name("../escape"));
        assert!(!is_safe_entry_name("a/b"));
        assert!(!is_safe_entry_name(""));
        assert!(is_safe_entry_name(".hidden"));
        assert!(is_safe_entry_name("正常文件名"));
    }

    #[test]
    fn base_name_is_posix_semantic() {
        assert_eq!(base_name("/var/log/a.log"), "a.log");
        assert_eq!(base_name("/data/dir/"), "dir");
        assert_eq!(base_name("/"), "");
        // 反斜杠不是 POSIX 分隔符，不得被当作目录边界
        assert_eq!(base_name("/tmp/we\\ird"), "we\\ird");
    }

    /// 覆盖判据：存在 + 未 force 才拒绝，其余组合一律放行。
    #[test]
    fn guard_only_blocks_unforced_overwrite() {
        assert!(guard(false, true, "/t").is_err());
        assert!(guard(true, true, "/t").is_ok());
        assert!(guard(false, false, "/t").is_ok());
    }

    /// 类型不符必须提前拦下（文件↔目录不可互相替换），类型一致或目标不存在都放行。
    /// 不拦的话要等 commit_* 的 rename 才炸出底层 errno。
    #[test]
    fn guard_kind_rejects_only_type_mismatch() {
        assert!(guard_kind(false, "/t", false, false).is_ok(), "目标不存在应放行");
        assert!(guard_kind(true, "/t", false, false).is_ok(), "文件覆盖文件应放行");
        assert!(guard_kind(true, "/t", true, true).is_ok(), "目录合并目录应放行");
        assert!(guard_kind(true, "/t", false, true).is_err(), "不得用文件替换目录");
        assert!(guard_kind(true, "/t", true, false).is_err(), "不得用目录替换文件");
    }

    /// 临时名每次调用必须唯一：两个传输指向同一目标时不能踩同一个 .part。
    #[test]
    fn transfer_tag_is_unique_per_call() {
        assert_ne!(transfer_tag(), transfer_tag());
    }

    /// 覆盖写的临时名与最终名同目录、同前缀，且绝不等于最终名本身
    /// （跨目录 rename 在 SFTP 上可能不被支持，同目录才保证 rename 是原子的）。
    #[test]
    fn part_name_stays_in_target_directory() {
        let dir = "/srv/data";
        let final_path = format!("{dir}/ckpt.bin");
        let tmp = format!("{dir}/ckpt.bin.{}.part", transfer_tag());
        assert!(tmp.starts_with(&final_path));
        assert!(tmp.ends_with(".part"));
        assert_ne!(tmp, final_path);
        assert_eq!(tmp.rsplit('/').next().unwrap().split('.').next().unwrap(), "ckpt");
    }

    /// local_is_dir 对不存在的路径必须返回 false（不能把「不存在」误判成「是文件」）。
    #[test]
    fn local_is_dir_handles_absent_path() {
        assert!(!local_is_dir("/nonexistent-path-for-hsshx-test"));
        assert!(local_is_dir(env!("CARGO_MANIFEST_DIR")));
    }

    #[test]
    fn human_formats_units() {
        assert_eq!(human(0), "0 B");
        assert_eq!(human(1023), "1023 B");
        assert_eq!(human(1024), "1.0 KiB");
        assert_eq!(human(5 * 1024 * 1024), "5.0 MiB");
    }
}
