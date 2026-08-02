//! 端到端验证：SFTP 上传后文件大小与权限。
//!
//! 前置条件：本地已运行隔离 sshd（127.0.0.1:2222，key 认证，~/.ssh/agi）：
//!   见项目历史：权限保留曾用 `..Default::default()` 构造 SETSTAT attrs，
//!   FileAttributes 的 Default 是 dummy attrs（size=Some(0)），SETSTAT 会把
//!   远端文件截断为 0 大小。本测试验证修复后（empty()）不截断，并复现
//!   错误写法（Default）确实截断——证明测试有判别力。
//!
//! 运行：cargo test --test sftp_perms_e2e -- --ignored --nocapture

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use hetushell_lib::ssh::conn::{establish, ConnParams, Connection};
use hetushell_lib::ssh::sftp;
use russh_sftp::protocol::FileAttributes;
use tokio::io::AsyncWriteExt;

const CONTENT: [u8; 4096] = [0xAB; 4096];

fn test_params() -> ConnParams {
    ConnParams {
        name: "e2e".into(),
        host: "127.0.0.1".into(),
        port: 2222,
        user: "wy".into(),
        auth: "key".into(),
        password: None,
        key_path: Some("~/.ssh/agi".into()),
        key_data: None,
        passphrase: None,
        keepalive: None,
        timeout: Some(10),
    }
}

async fn connect() -> Arc<Connection> {
    let params = test_params();
    let handle = establish(&params)
        .await
        .expect("连接临时 sshd 失败（127.0.0.1:2222 未运行？）");
    let conn = Arc::new(Connection::new("e2e".into(), params, false));
    *conn.handle.lock().await = Some(handle);
    conn
}

/// 模拟 upload_file 的收尾：create + write + flush + shutdown + set_metadata。
/// 返回 set_metadata 的结果（错误写法会因 chown root 被拒而失败，属预期）。
async fn upload_with(
    sftp_sess: &russh_sftp::client::SftpSession,
    remote: &str,
    attrs: FileAttributes,
) -> std::result::Result<(), russh_sftp::client::error::Error> {
    let mut dst = sftp_sess.create(remote).await.expect("create 失败");
    dst.write_all(&CONTENT).await.expect("write 失败");
    dst.flush().await.expect("flush 失败");
    dst.shutdown().await.expect("shutdown 失败");
    sftp_sess.set_metadata(remote, attrs).await
}

#[tokio::test]
#[ignore = "需要本地 127.0.0.1:2222 隔离 sshd"]
async fn upload_preserves_size_and_permissions() {
    let conn = connect().await;
    let sftp_sess = sftp::session(&conn).await.expect("SFTP 会话建立失败");
    let remote = "/tmp/e2e_upload_fixed.bin";
    let _ = sftp_sess.remove_file(remote).await;

    // 修复后的 attrs：只带 permissions，绝不携带 size/uid/gid/mtime
    let attrs = FileAttributes {
        permissions: Some(0o755),
        ..FileAttributes::empty()
    };
    upload_with(&sftp_sess, remote, attrs)
        .await
        .expect("修复后 set_metadata 必须成功");

    let meta = sftp_sess.metadata(remote).await.expect("stat 失败");
    assert_eq!(
        meta.size,
        Some(CONTENT.len() as u64),
        "修复后远端文件大小必须等于本地内容大小，不能被截断"
    );
    assert_eq!(
        meta.permissions.unwrap_or(0) & 0o777,
        0o755,
        "修复后远端文件权限必须保留 0o755"
    );

    let _ = sftp_sess.remove_file(remote).await;
    println!("PASS: 修复后上传 size={} perm=0o755", CONTENT.len());
}

#[tokio::test]
#[ignore = "需要本地 127.0.0.1:2222 隔离 sshd"]
async fn dummy_default_truncates_remote_file() {
    let conn = connect().await;
    let sftp_sess = sftp::session(&conn).await.expect("SFTP 会话建立失败");
    let remote = "/tmp/e2e_upload_bad.bin";
    let _ = sftp_sess.remove_file(remote).await;

    // 复现 bug：错误写法 `..Default::default()` → SETSTAT 携带 size=0 + uid/gid=0。
    // OpenSSH SETSTAT 语义：chown(0) 失败（普通用户）→ 整个 SETSTAT 报错；
    // chown(0) 成功（root 账户）→ 继续执行 truncate(0) → 文件被截断为 0 大小。
    // 无论哪种环境，dummy Default 都产生有害行为——断言二选一。
    let attrs = FileAttributes {
        permissions: Some(0o755),
        ..Default::default()
    };
    let setstat_result = upload_with(&sftp_sess, remote, attrs).await;

    let meta = sftp_sess.metadata(remote).await.expect("stat 失败");
    let size_after = meta.size;
    assert!(
        setstat_result.is_err() || size_after == Some(0),
        "判别力验证：dummy Default 必须有害（普通用户→SETSTAT 失败 chown root；root→截断为 0），\
         实际 setstat Ok 且 size={size_after:?}"
    );
    println!(
        "PASS: 错误写法（Default）setstat_err={} size={:?}——证实 Default 有害",
        setstat_result.is_err(),
        size_after
    );

    let _ = sftp_sess.remove_file(remote).await;
}
