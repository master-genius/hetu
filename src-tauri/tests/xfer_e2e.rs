//! 端到端验证：hsshx 的文件搬运语义（src/ssh/xfer.rs）。
//!
//! 前置条件：本地已运行隔离 sshd（127.0.0.1:2222，key 认证，~/.ssh/agi），
//! 与 tests/sftp_perms_e2e.rs 同一环境。
//!
//! 运行：cargo test --test xfer_e2e -- --ignored --nocapture
//!
//! 覆盖的是「单元测试看不见、真机已手工验证过」的四类行为：字节完整性、目录递归与
//! 权限保留、覆盖保护、取消清理。同机 --cp 是否真走服务器内 cp -a，以 mtime 是否保留
//! 为判据（流式路径不写 mtime，cp -a 会）。
//!
//! 注意：用例一律先上传再取得远端路径，不假设本地与 sshd 共享文件系统——隔离 sshd
//! 若跑在容器里，直接把本地路径当远端路径会失效。

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use hetushell_lib::error::Error;
use hetushell_lib::ssh::conn::{establish, ConnParams, Connection};
use hetushell_lib::ssh::sftp;
use hetushell_lib::ssh::xfer::{self, Cancel};

const ROOT: &str = "/tmp/hsshx_xfer_e2e";

fn test_params() -> ConnParams {
    ConnParams {
        name: "e2e".into(),
        host: "127.0.0.1".into(),
        port: 2222,
        user: std::env::var("USER").unwrap_or_else(|_| "wy".into()),
        auth: "key".into(),
        password: None,
        key_path: Some("~/.ssh/agi".into()),
        key_data: None,
        passphrase: None,
        keepalive: None,
        timeout: Some(10),
    }
}

/// 与 hsshx 的 attach() 同一组装方式：CLI 侧连接不启用自动重连。
async fn connect() -> Arc<Connection> {
    let params = test_params();
    let handle = establish(&params)
        .await
        .expect("连接临时 sshd 失败（127.0.0.1:2222 未运行？）");
    let conn = Arc::new(Connection::new("e2e".into(), params, false));
    *conn.handle.lock().await = Some(handle);
    conn
}

/// 确保远端根目录存在——被测的 put() 只创建自己的落点，不负责建父目录。
async fn ensure_root(conn: &Arc<Connection>) {
    let sess = sftp::session(conn).await.expect("SFTP 会话建立失败");
    let _ = sess.create_dir(ROOT.to_string()).await;
}

/// 本地临时工作区（每次独立，用例之间零共享状态）。
struct Scratch {
    dir: std::path::PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "hsshx_xfer_e2e_{tag}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn path(&self, name: &str) -> std::path::PathBuf {
        self.dir.join(name)
    }

    /// 写入内容并设定权限，返回落点路径。
    fn write(&self, name: &str, bytes: &[u8], mode: u32) -> std::path::PathBuf {
        let p = self.path(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, bytes).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
        p
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn remote_has(conn: &Arc<Connection>, path: &str) -> bool {
    let sess = sftp::session(conn).await.unwrap();
    sess.metadata(path).await.is_ok()
}

/// 尽力删除远端对象（文件优先，失败再当目录删）。
async fn cleanup(conn: &Arc<Connection>, path: &str) {
    let sess = sftp::session(conn).await.unwrap();
    let _ = sess.remove_file(path.to_string()).await;
    let _ = sess.remove_dir(path.to_string()).await;
}

/// 读回远端文件内容用于断言（走既有 pub 原语，不经被测代码）。
async fn read_remote(conn: &Arc<Connection>, path: &str) -> Vec<u8> {
    sftp::read_file_bytes(conn, path, 8 * 1024 * 1024)
        .await
        .unwrap_or_else(|e| panic!("读取远端 {path} 失败: {e}"))
}

#[tokio::test]
#[ignore = "需要本地 127.0.0.1:2222 隔离 sshd"]
async fn put_then_get_roundtrip_preserves_bytes() {
    let local = Scratch::new("roundtrip");
    // 4MB 非均匀内容：跨 256KB 分块边界，能抓到任何偏移/截断错误
    let payload: Vec<u8> = (0..4 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    let src = local.write("payload.bin", &payload, 0o644);

    let conn = connect().await;
    ensure_root(&conn).await;
    let cancel = Cancel::new();

    let uploaded = xfer::put(&conn, src.to_str().unwrap(), ROOT, false, &cancel)
        .await
        .expect("上传失败");
    assert_eq!(uploaded, format!("{ROOT}/payload.bin"));

    let back = local.path("payload.copy.bin");
    xfer::get(&conn, &uploaded, back.to_str().unwrap(), false, &cancel)
        .await
        .expect("下载失败");

    let got = std::fs::read(&back).unwrap();
    assert_eq!(got.len(), payload.len(), "往返后字节数变化");
    assert_eq!(got, payload, "往返后内容不一致");

    cleanup(&conn, &uploaded).await;
    println!("PASS: 4MB 往返逐字节一致");
}

#[tokio::test]
#[ignore = "需要本地 127.0.0.1:2222 隔离 sshd"]
async fn put_directory_recurses_and_preserves_permissions() {
    let local = Scratch::new("tree");
    local.write("tree/a.txt", b"alpha\n", 0o644);
    local.write("tree/sub/deep/n.txt", b"nested\n", 0o600);
    local.write("tree/run.sh", b"#!/bin/sh\necho ok\n", 0o755);
    // 空格名与隐藏文件必须一起递归到
    local.write("tree/sub/.dot", b"hidden\n", 0o644);
    local.write("tree/sp ace.bin", b"x", 0o644);

    let conn = connect().await;
    ensure_root(&conn).await;
    let cancel = Cancel::new();
    let root = xfer::put(&conn, local.path("tree").to_str().unwrap(), ROOT, false, &cancel)
        .await
        .expect("目录上传失败");

    let rels = [
        "a.txt",
        "sub/deep/n.txt",
        "run.sh",
        "sub/.dot",
        "sp ace.bin",
    ];
    for rel in rels {
        assert!(
            remote_has(&conn, &format!("{root}/{rel}")).await,
            "远端缺少 {rel}：递归遍历漏了文件"
        );
    }
    let sess = sftp::session(&conn).await.unwrap();
    let sh = sess.metadata(format!("{root}/run.sh")).await.unwrap();
    assert_eq!(
        sh.permissions.unwrap_or(0) & 0o777,
        0o755,
        "可执行位必须在上传后保留"
    );

    // 目录下载回来必须与源树逐字节一致（含权限位）
    let back = local.path("mirror");
    std::fs::create_dir_all(&back).unwrap();
    xfer::get(&conn, &root, back.to_str().unwrap(), false, &cancel)
        .await
        .expect("目录下载失败");
    assert_dir_eq(&local.path("tree"), &back.join("tree"));

    // 远端清理：叶子先删，目录逆序删
    for rel in ["sub/deep/n.txt", "sub/.dot", "a.txt", "run.sh", "sp ace.bin"] {
        let _ = sess.remove_file(format!("{root}/{rel}")).await;
    }
    let _ = sess.remove_dir(format!("{root}/sub/deep")).await;
    let _ = sess.remove_dir(format!("{root}/sub")).await;
    cleanup(&conn, &root).await;
    println!("PASS: 目录递归 + 权限保留 + 下载镜像一致");
}

#[tokio::test]
#[ignore = "需要本地 127.0.0.1:2222 隔离 sshd"]
async fn overwrite_is_refused_without_force_and_leaves_target_intact() {
    let local = Scratch::new("overwrite");
    let conn = connect().await;
    ensure_root(&conn).await;
    let cancel = Cancel::new();
    // 落点名取自本地 basename，故全程用同一个文件名，才能反复打到同一个远端目标
    let remote = format!("{ROOT}/guarded.bin");
    cleanup(&conn, &remote).await;
    let name = local.path("guarded.bin");
    std::fs::write(&name, b"ORIGINAL").unwrap();

    xfer::put(&conn, name.to_str().unwrap(), ROOT, false, &cancel)
        .await
        .expect("首次上传应成功");
    assert_eq!(read_remote(&conn, &remote).await, b"ORIGINAL");

    // 目标已存在且未加 --force：必须拒绝，且拒绝发生在写第一字节之前
    std::fs::write(&name, b"REPLACED-BY-MISTAKE").unwrap();
    let err = xfer::put(&conn, name.to_str().unwrap(), ROOT, false, &cancel)
        .await
        .expect_err("目标已存在时必须拒绝");
    assert!(
        err.to_string().contains("--force"),
        "错误应提示 --force，实际: {err}"
    );
    assert_eq!(
        read_remote(&conn, &remote).await,
        b"ORIGINAL",
        "被拒绝的覆盖绝不能改动目标"
    );

    // --force：必须真正替换（若只是跳过，内容仍是 ORIGINAL，断言会失败）
    xfer::put(&conn, name.to_str().unwrap(), ROOT, true, &cancel)
        .await
        .expect("--force 应允许覆盖");
    assert_eq!(
        read_remote(&conn, &remote).await,
        b"REPLACED-BY-MISTAKE",
        "--force 必须真正替换内容"
    );

    cleanup(&conn, &remote).await;
    println!("PASS: 覆盖保护拦截且未破坏原文件，--force 放行并真替换");
}

#[tokio::test]
#[ignore = "需要本地 127.0.0.1:2222 隔离 sshd"]
async fn cancel_removes_partial_file_when_target_is_new() {
    let local = Scratch::new("cancel");
    let name = local.path("big.bin");
    std::fs::write(&name, vec![0x5A_u8; 3 * 1024 * 1024]).unwrap();

    let conn = connect().await;
    ensure_root(&conn).await;
    let live = Cancel::new();
    let remote = format!("{ROOT}/big.bin");
    // 准备一个确定存在的远端文件，供下载方向的取消测试使用
    xfer::put(&conn, name.to_str().unwrap(), ROOT, true, &live)
        .await
        .expect("准备用上传失败");

    // 预置取消位：循环在搬完第 0 块前就退出，但写入目标已经 create——
    // 这正是「半成品必须被删掉」那段清理逻辑的靶心。
    let cancel = Cancel::new();
    cancel.trigger();

    let err = xfer::get(&conn, &remote, local.path("down.bin").to_str().unwrap(), false, &cancel)
        .await
        .expect_err("已置位取消时应中止");
    assert!(matches!(err, Error::Cancelled), "应报告取消，实际: {err}");
    assert!(
        !local.path("down.bin").exists(),
        "目标本不存在时取消，不得留下半成品"
    );

    cleanup(&conn, &remote).await;
    let err = xfer::put(&conn, name.to_str().unwrap(), ROOT, false, &cancel)
        .await
        .expect_err("已置位取消时应中止");
    assert!(matches!(err, Error::Cancelled));
    assert!(
        !remote_has(&conn, &remote).await,
        "上传取消后远端不得留下截断文件"
    );

    // 判别力对照：同一路径不取消时必须正常落盘——
    // 否则上一条断言可能只是因为压根传不上去而「假通过」。
    xfer::put(&conn, name.to_str().unwrap(), ROOT, false, &live)
        .await
        .expect("对照组：未取消应成功");
    assert!(remote_has(&conn, &remote).await, "对照组失败：正常上传也没落盘");

    cleanup(&conn, &remote).await;
    println!("PASS: 新目标取消 → 半成品被清除，且对照组证明传输本身可用");
}

/// 回归锁（分析确认的数据丢失路径）：**覆盖既有目标时取消，既有内容必须逐字节无损**。
/// 旧实现用 File::create / sftp.create 直接打开最终名，O_TRUNC 在写第一字节前就清空了
/// 用户文件——此时取消无论删不删，数据都已丢。现在覆盖走「临时名 → rename」，
/// 取消只删临时名，既有文件从未被打开。
#[tokio::test]
#[ignore = "需要本地 127.0.0.1:2222 隔离 sshd"]
async fn cancel_during_forced_overwrite_leaves_existing_content_intact() {
    let local = Scratch::new("cancel_overwrite");
    let conn = connect().await;
    ensure_root(&conn).await;
    let live = Cancel::new();
    // 预置取消：任何搬运一开工就中止
    let cancel = Cancel::new();
    cancel.trigger();

    // ---- 下载方向：本地目标已存在且有内容 ----
    let keeper = local.path("keep.bin");
    std::fs::write(&keeper, b"PRECIOUS-LOCAL-DATA").unwrap();
    let remote = format!("{ROOT}/keep.bin");
    std::fs::write(&keeper, b"OTHER-CONTENT-FOR-SOURCE").unwrap();
    xfer::put(&conn, keeper.to_str().unwrap(), ROOT, true, &live)
        .await
        .expect("准备远端源失败");
    std::fs::write(&keeper, b"PRECIOUS-LOCAL-DATA").unwrap();

    let err = xfer::get(&conn, &remote, keeper.to_str().unwrap(), true, &cancel)
        .await
        .expect_err("--force 下载覆盖已存在文件时取消应中止");
    assert!(matches!(err, Error::Cancelled));
    assert_eq!(
        std::fs::read(&keeper).unwrap(),
        b"PRECIOUS-LOCAL-DATA",
        "取消不得损坏既有本地文件（旧实现在此处已把它截成 0 长度）"
    );

    // ---- 上传方向：远端目标已存在且有内容 ----
    let up = local.path("up.bin");
    std::fs::write(&up, vec![7_u8; 64 * 1024]).unwrap();
    xfer::put(&conn, up.to_str().unwrap(), ROOT, true, &live)
        .await
        .expect("准备远端目标失败");
    let remote_target = format!("{ROOT}/up.bin");
    let before = read_remote(&conn, &remote_target).await;
    std::fs::write(&up, vec![8_u8; 128 * 1024]).unwrap();

    let err = xfer::put(&conn, up.to_str().unwrap(), ROOT, true, &cancel)
        .await
        .expect_err("--force 上传覆盖已存在文件时取消应中止");
    assert!(matches!(err, Error::Cancelled));
    assert_eq!(
        read_remote(&conn, &remote_target).await,
        before,
        "取消不得损坏既有远端文件"
    );

    // 且不得留下任何 .part 垃圾
    let sess = sftp::session(&conn).await.unwrap();
    for entry in sess.read_dir(ROOT.to_string()).await.unwrap() {
        assert!(
            !entry.file_name().contains(".part"),
            "取消后远端残留临时名 {}",
            entry.file_name()
        );
    }

    cleanup(&conn, &remote).await;
    cleanup(&conn, &remote_target).await;
    println!("PASS: 覆盖场景取消，既有内容双向无损且无临时名残留");
}

/// 回归锁：`--force` 下载目录到**本地已存在目录**时取消，该目录里与本次下载无关的
/// 既有文件必须仍在。旧实现的回滚是 `remove_dir_all(root)`，会把整棵树（含用户既有
/// 内容）一并删掉。
#[tokio::test]
#[ignore = "需要本地 127.0.0.1:2222 隔离 sshd"]
async fn cancel_during_forced_directory_get_keeps_unrelated_existing_files() {
    let local = Scratch::new("dir_cancel");
    let conn = connect().await;
    ensure_root(&conn).await;
    let live = Cancel::new();
    let sess = sftp::session(&conn).await.unwrap();

    // 远端源目录：put 的落点名取自本地 basename，故本地文件就叫 in.txt
    let src_dir = format!("{ROOT}/dsrc");
    let _ = sess.create_dir(src_dir.clone()).await;
    let seed = local.path("in.txt");
    std::fs::write(&seed, vec![9_u8; 128 * 1024]).unwrap();
    xfer::put(&conn, seed.to_str().unwrap(), &src_dir, false, &live)
        .await
        .expect("准备远端源目录内容失败");

    // 本地已存在同名目录，里面有一个本次下载绝不涉及的文件
    let parent = local.path("mirror");
    let root = parent.join("dsrc");
    std::fs::create_dir_all(&root).unwrap();
    let unrelated = root.join("unrelated.txt");
    std::fs::write(&unrelated, b"MUST-SURVIVE").unwrap();

    let cancel = Cancel::new();
    cancel.trigger();
    let err = xfer::get(&conn, &src_dir, parent.to_str().unwrap(), true, &cancel)
        .await
        .expect_err("--force 目录下载取消应中止");
    assert!(matches!(err, Error::Cancelled));

    assert!(root.exists(), "既有目录不得因取消而被整体删除");
    assert!(
        unrelated.exists(),
        "与本次下载无关的既有文件被删掉了——旧的 remove_dir_all 回滚正是这个缺陷"
    );
    assert_eq!(
        std::fs::read(&unrelated).unwrap(),
        b"MUST-SURVIVE",
        "既有文件内容必须原样保留"
    );

    let _ = sess.remove_file(format!("{src_dir}/in.txt")).await;
    cleanup(&conn, &src_dir).await;
    println!("PASS: --force 目录下载取消后，无关既有文件完好（未整树被删）");
}

#[tokio::test]
#[ignore = "需要本地 127.0.0.1:2222 隔离 sshd"]
async fn cp_on_same_connection_keeps_mtime_proving_server_side_copy() {
    let local = Scratch::new("cp");
    // 固定 mtime 作为指纹：cp -a 会带过去，流式路径不会写 mtime
    let src = local.write("stamp.bin", b"payload", 0o644);
    set_mtime(&src, 1_700_000_000);

    let conn = connect().await;
    ensure_root(&conn).await;
    let cancel = Cancel::new();
    let uploaded = xfer::put(&conn, src.to_str().unwrap(), ROOT, false, &cancel)
        .await
        .expect("上传失败");
    // 源文件的 mtime 由 SFTP 上传决定，这里改用远端实测 mtime 作对照基准
    let sess = sftp::session(&conn).await.unwrap();
    let src_mtime = sess.metadata(&uploaded).await.unwrap().mtime;

    let dst_dir = format!("{ROOT}/cpdir");
    let _ = sess.create_dir(dst_dir.clone()).await;
    let copied = xfer::cp(&conn, &conn, &uploaded, &dst_dir, false, &cancel)
        .await
        .expect("同机复制失败");
    assert_eq!(copied, format!("{dst_dir}/stamp.bin"));

    let dst_mtime = sess.metadata(&copied).await.unwrap().mtime;
    assert_eq!(
        dst_mtime, src_mtime,
        "mtime 未随复制保留（{src_mtime:?} → {dst_mtime:?}），说明走的是流式中转而非 cp -a"
    );

    cleanup(&conn, &copied).await;
    cleanup(&conn, &dst_dir).await;
    cleanup(&conn, &uploaded).await;
    println!("PASS: 同机 cp 走服务器内 cp -a（mtime 一致）");
}

/// 设置文件 mtime（atime/mtime 同值），不引入 filetime 依赖。
fn set_mtime(path: &Path, secs: u64) {
    let c = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    let times = [
        libc::timespec {
            tv_sec: secs as libc::c_long,
            tv_nsec: 0,
        },
        libc::timespec {
            tv_sec: secs as libc::c_long,
            tv_nsec: 0,
        },
    ];
    let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), times.as_ptr(), 0) };
    assert_eq!(rc, 0, "设置 mtime 失败");
}

/// 两棵目录树逐字节一致（相对路径集合 + 内容 + 权限位）。
fn assert_dir_eq(a: &Path, b: &Path) {
    let mut stack = vec![a.to_path_buf()];
    let mut seen = 0usize;
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let rel = path.strip_prefix(a).unwrap();
            let twin = b.join(rel);
            assert!(twin.exists(), "镜像缺少 {}", rel.display());
            if path.is_dir() {
                assert!(twin.is_dir(), "{} 应为目录", rel.display());
                stack.push(path);
            } else {
                assert_eq!(
                    std::fs::read(&path).unwrap(),
                    std::fs::read(&twin).unwrap(),
                    "{} 内容不一致",
                    rel.display()
                );
                let want = path.metadata().unwrap().permissions().mode() & 0o777;
                let got = twin.metadata().unwrap().permissions().mode() & 0o777;
                assert_eq!(want, got, "{} 权限不一致", rel.display());
                seen += 1;
            }
        }
    }
    assert!(seen >= 5, "镜像比对的样本过少（{seen}），用例可能没真跑到");
}
