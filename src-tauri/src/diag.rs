//! hsshx 自身诊断的输出通道。
//!
//! 默认写 stderr：GUI 进程从不登记 fd，不带 `--machine` 的 hsshx 也不登记，
//! 这两条路径的输出与引入本模块之前**逐字节相同**。
//!
//! `--machine` 登记调用方用 `3>...` 打开的 fd 3，此后 hsshx 自己的错误、传输进度、
//! `--put/--cp` 的落点路径全部只出现在 fd 3，stderr 就只剩**远端命令自己的** stderr。
//! 一次调用因此能拿到三路互不污染的流：stdout=远端 stdout、stderr=远端 stderr、
//! fd 3=工具自身的话。此前两路挤在 stderr 里，调用方无从判断某一行是谁说的。

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicI32, Ordering};

/// 诊断写往哪个 fd；-1 = stderr。进程启动时由 `--machine` 登记一次，之后只读。
static DIAG_FD: AtomicI32 = AtomicI32::new(-1);

/// 诊断当前的落点。`None` = stderr（默认，GUI 与不带 `--machine` 的 CLI 全靠它零变化）。
pub fn target() -> Option<i32> {
    match DIAG_FD.load(Ordering::Relaxed) {
        v if v >= 0 => Some(v),
        _ => None,
    }
}

/// 登记诊断 fd。重复登记保留第一次的值：fd 归调用方的 shell 所有，本进程绝不关闭它。
#[cfg(unix)]
pub fn set(fd: i32) {
    let _ = DIAG_FD.compare_exchange(-1, fd, Ordering::Relaxed, Ordering::Relaxed);
}

/// 探 fd 是否**可写**。只看「已打开」不够：`3<文件` 这种只读 fd 也算打开，
/// 而本模块写诊断时忽略写错误（写不进也不能让操作失败），只读 fd 会让诊断静默全丢——
/// 那正是 `--machine` 要消灭的假承诺。
#[cfg(unix)]
fn fd_is_writable(fd: i32) -> bool {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    flags != -1 && (flags & libc::O_ACCMODE) != libc::O_RDONLY
}

/// exec 时 fd 3 是否可写：3 = 可写，-1 = 不可用，-9 = 尚未采集。
///
/// 必须在**建 tokio 运行时之前**固化（见 `capture_machine_fd`）。实测原因：
/// 调用方用 `3>&-` 关掉 fd 3 后，运行时自己会把 3 号占去当内部 fd，此时再探
/// `fcntl(3)` 依然为真，诊断就被静默写进了运行时内部——假承诺。
static FD3_AT_EXEC: AtomicI32 = AtomicI32::new(-9);

/// 固化 fd 3 的可用状态。`hsshx` 的 `main` 在交给异步运行时之前第一件事就调它。
pub fn capture_machine_fd() {
    #[cfg(unix)]
    let ok = fd_is_writable(3);
    #[cfg(not(unix))]
    let ok = false;
    FD3_AT_EXEC.store(if ok { 3 } else { -1 }, Ordering::Relaxed);
}

/// `--machine` 的落地：把诊断通道接到调用方用 `3>...` 打开的 fd 3 上。
///
/// 失败一律返回 Err 让调用方硬报错，**不回落 stderr**：调用方指定 `--machine` 就是在声明
/// 「我要三条互不污染的流」，此时把 hsshx 的话仍写进 stderr，会让它把工具自身的错误当成
/// 远端程序的输出——那比没有这个开关更糟。
pub fn adopt() -> Result<(), &'static str> {
    match FD3_AT_EXEC.load(Ordering::Relaxed) {
        3 => {
            set(3);
            Ok(())
        }
        // 未采集说明入口没先固化状态（新代码路径漏调 capture_machine_fd），
        // 与其写进一个来路不明的 fd，不如直接报错。
        -9 => Err("--machine 内部错误：fd 3 状态未采集"),
        _ => Err("--machine 需要调用方以可写方式打开 fd 3（用法示例：hsshx web \"ls\" 3>/tmp/hsshx.diag）"),
    }
}

/// 诊断通道是不是终端：进度帧的原地刷新（`\r\x1b[2K`）只对终端有意义，
/// 重定向到文件/管道时要退化成「每对象一行名字」，否则日志里全是控制字符。
pub fn is_tty() -> bool {
    #[cfg(unix)]
    if let Some(fd) = target() {
        return unsafe { libc::isatty(fd) } == 1;
    }
    std::io::stderr().is_terminal()
}

/// 写一条诊断。`nl = false` 给进度帧（自带 `\r`，不再补换行）。
pub fn emit(nl: bool, args: std::fmt::Arguments<'_>) {
    #[cfg(unix)]
    if let Some(fd) = target() {
        use std::os::unix::io::FromRawFd;
        // ManuallyDrop：fd 3 属于调用方，不能被 File 的 Drop 关掉。
        let mut out = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(fd) });
        let _ = if nl {
            writeln!(out, "{args}")
        } else {
            write!(out, "{args}")
        };
        let _ = out.flush();
        return;
    }
    let mut out = std::io::stderr().lock();
    let _ = if nl {
        writeln!(out, "{args}")
    } else {
        write!(out, "{args}")
    };
    let _ = out.flush();
}

/// 结束当前诊断行（进度收尾用的裸换行）。
pub fn newline() {
    emit(true, format_args!(""));
}

/// hsshx 自身诊断的一行：默认 stderr，`--machine` 下走登记的那个 fd。
#[macro_export]
macro_rules! dprintln {
    ($($arg:tt)*) => { $crate::diag::emit(true, format_args!($($arg)*)) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;

    /// 每个用例独占目录：cargo test 并行时共用同一路径会让写入互相截断。
    fn tmp(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("hsshx-diag-tests-{tag}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("f")
    }

    /// 诊断通道的状态机：未登记→stderr、首个 set 胜出、adopt 的三条判据。
    ///
    /// 刻意合并成**一个**用例：这两段都在改同一个进程级 `DIAG_FD`，拆成两个并行跑时
    /// `set(7)` 可能正好插在另一用例的 `store(-1)` 与断言之间 → 与产品无关的假失败。
    /// 结束时还原成 -1，否则同进程后续测试的 `dprintln!` 会被导向一个来路不明的 fd。
    #[cfg(unix)]
    #[test]
    fn diag_channel_state_machine() {
        // 未登记时诊断必须是 stderr——这是「GUI 行为逐字节不变」的前提
        DIAG_FD.store(-1, Ordering::Relaxed);
        assert!(target().is_none(), "未登记时必须是 stderr");

        // 重复登记保留第一个：fd 归调用方，不允许中途改道
        set(7);
        set(9);
        assert_eq!(target(), Some(7), "重复登记不得改写已登记的 fd");

        // adopt：未采集 / fd 3 不可用都必须拒绝，且拒绝时不得留下登记
        DIAG_FD.store(-1, Ordering::Relaxed);
        FD3_AT_EXEC.store(-9, Ordering::Relaxed);
        assert!(adopt().is_err(), "未采集必须报错");
        assert_eq!(target(), None, "报错路径不得留下登记");

        FD3_AT_EXEC.store(-1, Ordering::Relaxed);
        assert!(adopt().is_err(), "fd 3 不可用必须报错，绝不静默回落 stderr");
        assert_eq!(target(), None, "报错路径不得留下登记");

        FD3_AT_EXEC.store(3, Ordering::Relaxed);
        assert!(adopt().is_ok(), "采集到可写 fd 3 时应登记成功");
        assert_eq!(target(), Some(3));

        DIAG_FD.store(-1, Ordering::Relaxed);
        FD3_AT_EXEC.store(-9, Ordering::Relaxed);
    }

    /// 只读 fd 必须判为不可写：`3<文件` 也算「已打开」，但诊断写不进去会静默全丢，
    /// 而 `--machine` 的整个价值就在于「不会再有假承诺」。
    #[cfg(unix)]
    #[test]
    fn writability_probe_rejects_read_only_fd() {
        let p = tmp("writability");
        std::fs::write(&p, "x").unwrap();

        let ro = std::fs::File::open(&p).unwrap();
        assert!(!fd_is_writable(ro.as_raw_fd()), "只读 fd 必须判假");

        let wr = std::fs::File::create(&p).unwrap();
        assert!(fd_is_writable(wr.as_raw_fd()), "写 fd 必须判真");

        let rw = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&p)
            .unwrap();
        assert!(fd_is_writable(rw.as_raw_fd()), "读写 fd 必须判真");

        // 巨大的 fd 号必然未打开：fcntl 返错时不得误判为可写
        assert!(!fd_is_writable(1 << 20), "未打开的 fd 必须判假");
    }
}
