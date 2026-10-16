//! SSH 连接建立、认证与断线重连。
//!
//! 认证策略：先尝试用户指定的主方式（密钥或密码），失败后自动回退到另一种
//! （若提供了对应凭据）。最多尝试 2 次，不会像 openssh 那样把 agent 里所有
//! 密钥挨个试——从根本上避免触发服务端 MaxAuthTries 限制。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use russh::client::{self, AuthResult};
use russh::keys::{HashAlg, PrivateKeyWithHashAlg};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

use crate::error::{Error, Result};
use crate::settings::known_hosts_path;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnParams {
    /// 展示名（前端标签页标题使用，后端仅透传保存）
    #[allow(dead_code)]
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    /// "key" | "password"
    pub auth: String,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub key_path: Option<String>,
    /// 私钥内容（PEM 文本）；优先于 key_path，自包含不依赖外部文件路径
    #[serde(default)]
    pub key_data: Option<String>,
    #[serde(default)]
    pub passphrase: Option<String>,
    /// 保活间隔（秒），None 用默认 15s
    #[serde(default)]
    pub keepalive: Option<u64>,
    /// 连接超时（秒），None 用默认 20s
    #[serde(default)]
    pub timeout: Option<u64>,
    /// 发起方标记（`hssh --by <tag>` / `hsshx --by <tag>`）：让 listconn 能区分这条连接
    /// 是谁起的。None = GUI 面板点击或手输参数，按「user」展示。
    #[serde(default)]
    pub by: Option<String>,
    /// 发起该连接的本地 pane id（仅 hssh 建连路径有值）：killconn 授权判据之一——
    /// 同一终端里发起的连接，其发起者有权自行终止。
    #[serde(default)]
    pub origin_pane: Option<String>,
}

/// `by` 发起方标记的取值校验（唯一实现点：hsshx CLI 与后端 `ssh_connect` 共用）。
///
/// 为什么必须校验：tag 会被 `hssh listconn` 原样输出到宿主终端与 AI 上下文，含 ESC/CR
/// 即构成转义序列注入；另外前导 `-` 会让 `--by --force` 把 `--force` 吞成 tag，
/// 使传输静默失去覆盖保护。
///
/// 返回 std 的 `Result<_, String>` 而非 crate 别名 `Result`：调用方（CLI 参数解析、
/// Tauri command）各自决定如何把错误字符串包成自己的错误类型。
pub fn valid_tag(raw: &str) -> std::result::Result<String, String> {
    if raw.is_empty() {
        return Err("不能为空".to_string());
    }
    if raw.len() > 32 {
        return Err(format!("过长（{} 字节，上限 32）", raw.len()));
    }
    if raw.starts_with('-') {
        return Err(format!("不能以 - 开头（会吞掉后续选项）: {raw:?}"));
    }
    if !raw
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | '/' | ':' | '+'))
    {
        return Err(format!(
            "含非法字符（仅允许字母数字与 - _ . @ / : +）: {raw:?}"
        ));
    }
    Ok(raw.to_string())
}

/// 一条 SSH 连接。分屏/标签内的每个 pane 在此连接上开独立 channel，复用 TCP。
pub struct Connection {
    pub id: String,
    pub params: ConnParams,
    pub handle: Mutex<Option<client::Handle<ClientHandler>>>,
    pub sftp: Mutex<Option<Arc<russh_sftp::client::SftpSession>>>,
    reconnecting: AtomicBool,
    pub auto_reconnect: AtomicBool,
    /// 建连时刻，供 listconn 报告存活时长
    pub created_at: std::time::SystemTime,
}

/// `listconn` 的连接视图。**安全约束**：字段全部白名单枚举，绝不从 ConnParams 整体序列化——
/// ConnParams 含 password/keyData/passphrase，一旦入视图就会经响应文件落到 /tmp 供任意本地
/// 进程读取。新增字段必须逐项显式判断是否为机密。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnInfo {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    /// 发起方标记；None = GUI 面板点击或手输参数（消费侧显示为「user」）
    pub by: Option<String>,
    /// 发起该连接的 pane id（仅 hssh 路径有值）
    pub origin_pane: Option<String>,
    /// 该连接上正在使用的 pane id 列表
    pub panes: Vec<String>,
    pub alive: bool,
    pub reconnecting: bool,
    pub auto_reconnect: bool,
    pub uptime_secs: u64,
    pub transfers: usize,
}

/// russh 客户端回调：TOFU（首次信任）模型校验主机指纹。
/// 指纹库存于应用配置目录 known_hosts.json，键为 "host:port"。
pub struct ClientHandler {
    host: String,
    port: u16,
}

impl ClientHandler {
    pub fn new(host: String, port: u16) -> Self {
        Self { host, port }
    }

    /// 读取指纹库。文件不存在 → 空库（Ok）；文件损坏/无法解析 → Err（视为致命，
    /// 由调用方拒绝连接，绝不静默清空重新信任）。
    fn load_hosts_db() -> std::result::Result<HashMap<String, String>, ()> {
        let path = match known_hosts_path() {
            Ok(p) => p,
            Err(_) => return Ok(HashMap::new()),
        };
        match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).map_err(|_| ()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(_) => Err(()),
        }
    }

    /// 原子写入（临时文件 + rename），避免崩溃写坏文件。
    fn save_hosts_db(db: &HashMap<String, String>) {
        if let (Ok(path), Ok(json)) = (known_hosts_path(), serde_json::to_string_pretty(db)) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                crate::settings::set_owner_only(&tmp); // 与其它配置文件一致：0600
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }
}

/// 线程级串行化 known_hosts 的「读-判-写」，避免同进程并发连接互相覆盖已固定指纹。
/// （flock 是进程级锁，同进程内多线程不互斥，故仍需此 Mutex；跨进程互斥由文件锁提供）
static HOSTS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKey,
    ) -> std::result::Result<bool, Self::Error> {
        let key = format!("{}:{}", self.host, self.port);
        let fp = server_public_key.fingerprint(HashAlg::Sha256).to_string();
        // 线程级锁：防同进程多 SSH 并发（flock 进程级，同进程内不互斥）
        let _guard = HOSTS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let path = match known_hosts_path() {
            Ok(p) => p,
            Err(_) => return Ok(false),
        };
        // 跨进程文件锁：防多进程并发连接新主机时互相覆盖已固定指纹
        let lock_path = path.with_extension("json.lock");
        let lock_file = match std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&lock_path)
        {
            Ok(f) => f,
            Err(_) => return Ok(false),
        };
        if fs2::FileExt::lock_exclusive(&lock_file).is_err() {
            return Ok(false);
        }
        let result = (|| {
            let mut db = match Self::load_hosts_db() {
                Ok(db) => db,
                // 指纹库损坏：安全起见拒绝连接，而非清空后重新信任（否则等于放行 MITM）
                Err(()) => return Ok(false),
            };
            match db.get(&key) {
                Some(known) if *known == fp => Ok(true),
                Some(_) => Ok(false), // 指纹变化，拒绝连接（防中间人）
                None => {
                    db.insert(key, fp);
                    Self::save_hosts_db(&db);
                    Ok(true) // 首次信任
                }
            }
        })();
        let _ = fs2::FileExt::unlock(&lock_file);
        result
    }
}

fn ensure_auth(result: AuthResult) -> Result<()> {
    match result {
        AuthResult::Success => Ok(()),
        AuthResult::Failure { .. } => Err(Error::msg(
            "认证失败：请检查用户名、密码或私钥是否正确",
        )),
    }
}

/// 尝试密钥认证。返回 Some(()) 表示成功，None 表示失败或无可用密钥。
/// 失败时不返回错误（调用方负责回退到密码或报告最终错误），以免密钥解码/加载
/// 等问题（如密钥格式错误、passphrase 缺失）被误报为"认证失败"。
async fn try_key_auth(
    handle: &mut client::Handle<ClientHandler>,
    params: &ConnParams,
) -> Option<()> {
    let passphrase = params.passphrase.as_deref().filter(|s| !s.is_empty());
    let key = match params.key_data.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(pem) => russh::keys::decode_secret_key(pem, passphrase).ok()?,
        None => {
            let path = params.key_path.as_deref()?;
            russh::keys::load_secret_key(expand_tilde(path), passphrase).ok()?
        }
    };
    let hash = handle.best_supported_rsa_hash().await.ok()?.flatten();
    let auth_key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
    match handle.authenticate_publickey(&params.user, auth_key).await.ok()? {
        AuthResult::Success => Some(()),
        AuthResult::Failure { .. } => None,
    }
}

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

/// 建立连接并按指定方式认证（主方式失败自动回退）。
pub async fn establish(params: &ConnParams) -> Result<client::Handle<ClientHandler>> {
    // 超时覆盖「连接 + 认证」全过程：仅包 connect 会让服务器在认证阶段卡住时无限挂起，
    // 进而卡死重连循环。默认 20s。
    let timeout = Duration::from_secs(params.timeout.unwrap_or(20).max(1));
    match tokio::time::timeout(timeout, establish_inner(params)).await {
        Err(_) => Err(Error::msg(format!("连接超时（超过 {} 秒）", timeout.as_secs()))),
        Ok(r) => r,
    }
}

/// 实际的连接 + 认证流程（由 establish 统一施加超时）。
async fn establish_inner(params: &ConnParams) -> Result<client::Handle<ClientHandler>> {
    let config = Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(params.keepalive.unwrap_or(15).max(1))),
        ..Default::default()
    });
    let mut handle = client::connect(
        config,
        (params.host.as_str(), params.port),
        ClientHandler::new(params.host.clone(), params.port),
    )
    .await
    .map_err(|e| match e {
        russh::Error::UnknownKey => {
            Error::msg("服务器主机指纹与上次记录不一致，已拒绝连接（可能存在中间人攻击）。若确认服务器已重装，请在设置目录删除 known_hosts.json 中对应条目。")
        }
        other => Error::Ssh(other),
    })?;

    match params.auth.as_str() {
        "password" => {
            // 主方法：密码；备选：密钥
            match params.password.as_deref() {
                Some(pw) => match handle.authenticate_password(&params.user, pw).await? {
                    AuthResult::Success => {}
                    AuthResult::Failure { .. } => {
                        // 密码失败 → 尝试密钥回退（若提供了私钥）
                        try_key_auth(&mut handle, params)
                            .await
                            .ok_or_else(|| {
                                Error::msg("认证失败：请检查用户名、密码或私钥是否正确")
                            })?;
                    }
                },
                // 未提供密码 → 直接尝试密钥回退
                None => {
                    try_key_auth(&mut handle, params)
                        .await
                        .ok_or_else(|| {
                            Error::msg("认证失败：请检查用户名、密码或私钥是否正确")
                        })?;
                }
            }
        }
        _ => {
            // 主方法：密钥；备选：密码
            match try_key_auth(&mut handle, params).await {
                Some(()) => {}
                None => {
                    // 密钥失败或无密钥 → 尝试密码回退
                    let pw = params.password.as_deref().ok_or_else(|| {
                        Error::msg("认证失败：请检查用户名、密码或私钥是否正确")
                    })?;
                    ensure_auth(handle.authenticate_password(&params.user, pw).await?)?;
                }
            }
        }
    }
    Ok(handle)
}

impl Connection {
    pub fn new(id: String, params: ConnParams, auto_reconnect: bool) -> Self {
        Self {
            id,
            params,
            handle: Mutex::new(None),
            sftp: Mutex::new(None),
            reconnecting: AtomicBool::new(false),
            auto_reconnect: AtomicBool::new(auto_reconnect),
            created_at: std::time::SystemTime::now(),
        }
    }

    /// 是否正处于重连退避中（trigger_reconnect 任务存活期间为 true）
    pub fn is_reconnecting(&self) -> bool {
        self.reconnecting.load(Ordering::SeqCst)
    }

    /// 构造脱敏视图。`alive` 由调用方异步查（`is_alive`）后传入，保持本函数纯同步可测。
    /// 逐字段显式取值，绝不 `serde` 整个 params——params 含 password/keyData/passphrase。
    pub fn view(&self, alive: bool, panes: Vec<String>, transfers: usize) -> ConnInfo {
        ConnInfo {
            id: self.id.clone(),
            name: self.params.name.clone(),
            host: self.params.host.clone(),
            port: self.params.port,
            user: self.params.user.clone(),
            by: self.params.by.clone(),
            origin_pane: self.params.origin_pane.clone(),
            panes,
            alive,
            reconnecting: self.is_reconnecting(),
            auto_reconnect: self.auto_reconnect.load(Ordering::SeqCst),
            uptime_secs: self
                .created_at
                .elapsed()
                .map(|d| d.as_secs())
                .unwrap_or_default(),
            transfers,
        }
    }

    /// 连接是否仍然活着
    pub async fn is_alive(&self) -> bool {
        match self.handle.lock().await.as_ref() {
            Some(h) => !h.is_closed(),
            None => false,
        }
    }

    /// 触发断线重连（幂等：并发触发只会跑一个重连任务）。
    /// 指数退避 1s→30s，成功后广播 conn-state=connected，前端负责重建各 pane 的 shell。
    pub fn trigger_reconnect(self: Arc<Self>, app: AppHandle) {
        if self.reconnecting.swap(true, Ordering::SeqCst) {
            return;
        }
        tokio::spawn(async move {
            let mut delay = 1u64;
            loop {
                if !self.auto_reconnect.load(Ordering::SeqCst) {
                    let _ = app.emit(
                        "conn-state",
                        serde_json::json!({ "connId": self.id, "state": "closed" }),
                    );
                    break;
                }
                let _ = app.emit(
                    "conn-state",
                    serde_json::json!({ "connId": self.id, "state": "reconnecting" }),
                );
                match establish(&self.params).await {
                    Ok(h) => {
                        // establish 可能耗时数秒，期间用户可能已主动断开（ssh_disconnect
                        // 置 auto_reconnect=false 并 take() 掉 handle）。为杜绝「检查通过后、
                        // 写入 handle 前被 disconnect 插入」而泄漏幽灵连接，必须在**持有 handle
                        // 锁的临界区内**完成「判 auto_reconnect + 写入」，与 disconnect 的
                        // handle.lock().take() 互斥。
                        let mut guard = self.handle.lock().await;
                        if !self.auto_reconnect.load(Ordering::SeqCst) {
                            drop(guard);
                            let _ = h
                                .disconnect(russh::Disconnect::ByApplication, "bye", "zh")
                                .await;
                            break;
                        }
                        *guard = Some(h);
                        drop(guard);
                        *self.sftp.lock().await = None; // 旧 SFTP 会话随连接失效
                        let _ = app.emit(
                            "conn-state",
                            serde_json::json!({ "connId": self.id, "state": "connected" }),
                        );
                        break;
                    }
                    Err(e) => {
                        let _ = app.emit(
                            "conn-state",
                            serde_json::json!({
                                "connId": self.id,
                                "state": "waiting",
                                "error": e.to_string(),
                                "retryIn": delay
                            }),
                        );
                        tokio::time::sleep(Duration::from_secs(delay)).await;
                        delay = (delay * 2).min(30);
                    }
                }
            }
            self.reconnecting.store(false, Ordering::SeqCst);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params_with_secrets() -> ConnParams {
        ConnParams {
            name: "web".into(),
            host: "10.0.0.9".into(),
            port: 2222,
            user: "root".into(),
            auth: "password".into(),
            password: Some("SECRETPW".into()),
            key_path: Some("/secret/path/id_ed25519".into()),
            key_data: Some("-----BEGIN KEY-----SECRETPEM-----END KEY-----".into()),
            passphrase: Some("SECRETPHRASE".into()),
            keepalive: None,
            timeout: None,
            by: None,
            origin_pane: None,
        }
    }

    /// 回归锁：`conns_list` 的视图经响应文件落到 /tmp 供任意本地进程读取，
    /// 因此凭据字段必须一个都不出现在序列化结果里。改 ConnInfo 时若不小心
    /// 整体序列化 params，本测试即刻失败。
    #[test]
    fn conn_view_never_carries_credentials() {
        let mut params = params_with_secrets();
        params.by = Some("qwen".into());
        params.origin_pane = Some("pane-1".into());
        let conn = Connection::new("conn-1".into(), params, true);

        let view = conn.view(
            true,
            vec!["pane-a".into(), "pane-b".into()],
            2,
        );
        let json = serde_json::to_string(&view).expect("ConnInfo 应可序列化");

        for secret in ["SECRETPW", "SECRETPHRASE", "SECRETPEM", "id_ed25519"] {
            assert!(!json.contains(secret), "视图泄漏凭据 {secret}: {json}");
        }
        // 该出的字段要出全：发起方标记与归属 pane 是 killconn 授权判据的依据
        assert_eq!(view.by.as_deref(), Some("qwen"));
        assert_eq!(view.origin_pane.as_deref(), Some("pane-1"));
        assert_eq!(view.panes, vec!["pane-a", "pane-b"]);
        assert_eq!(view.transfers, 2);
        assert!(view.alive && view.auto_reconnect && !view.reconnecting);
    }

    /// 回归锁：旧版前端/旧版 hssh 脚本发的 payload 不含新字段，必须照旧建连成功。
    /// 少了 serde(default) 会让所有既有连接因反序列化失败而全线报「缺少字段」。
    #[test]
    fn legacy_params_payload_without_origin_still_deserializes() {
        let legacy = r#"{"name":"web","host":"10.0.0.9","port":22,"user":"root","auth":"key"}"#;
        let params: ConnParams = serde_json::from_str(legacy).expect("旧 payload 必须仍可解析");
        assert_eq!(params.by, None, "缺省即 user 发起");
        assert_eq!(params.origin_pane, None);
        assert_eq!(params.keepalive, None);
    }

    /// 驼峰线格式：前端 types.ts 与 hssh listconn 的 JSON 都按 camelCase 消费，
    /// 键名退化成 snake_case 会让前端静默读到 undefined（by 永远显示 user）。
    /// 断言写成「必须含 camelCase 且不含 snake_case」——用 || 等于没验。
    #[test]
    fn origin_fields_use_camel_case_on_the_wire() {
        let mut params = params_with_secrets();
        params.by = Some("qwen".into());
        params.origin_pane = Some("pane-1".into());
        let conn = Connection::new("c".into(), params, true);
        let json = serde_json::to_string(&conn.view(true, vec![], 0)).unwrap();
        assert!(json.contains("\"originPane\":\"pane-1\""), "实际: {json}");
        assert!(!json.contains("origin_pane"), "snake_case 泄漏进线格式: {json}");
    }

    /// `is_reconnecting` 反映 trigger_reconnect 的幂等占位标志（视图据此区分
    /// 「正在重连」与「已放弃」两种断开态）。
    #[test]
    fn reconnecting_flag_starts_false() {
        let conn = Connection::new("c".into(), params_with_secrets(), false);
        assert!(!conn.is_reconnecting());
        assert!(!conn.auto_reconnect.load(Ordering::SeqCst));
    }
}
