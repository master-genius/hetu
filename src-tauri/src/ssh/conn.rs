//! SSH 连接建立、认证与断线重连。
//!
//! 关键设计：认证时只提交用户明确指定的**单一**凭据（一把私钥或一个密码），
//! 不像 openssh 那样把 agent 里所有密钥挨个试一遍——从根本上避免触发服务端
//! MaxAuthTries 重试次数限制。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use russh::client::{self, AuthResult};
use russh::keys::{HashAlg, PrivateKeyWithHashAlg};
use serde::Deserialize;
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
    #[serde(default)]
    pub passphrase: Option<String>,
}

/// 一条 SSH 连接。分屏/标签内的每个 pane 在此连接上开独立 channel，复用 TCP。
pub struct Connection {
    pub id: String,
    pub params: ConnParams,
    pub handle: Mutex<Option<client::Handle<ClientHandler>>>,
    pub sftp: Mutex<Option<Arc<russh_sftp::client::SftpSession>>>,
    reconnecting: AtomicBool,
    pub auto_reconnect: AtomicBool,
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

    fn hosts_db() -> HashMap<String, String> {
        known_hosts_path()
            .ok()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save_hosts_db(db: &HashMap<String, String>) {
        if let (Ok(path), Ok(json)) = (known_hosts_path(), serde_json::to_string_pretty(db)) {
            let _ = std::fs::write(path, json);
        }
    }
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKey,
    ) -> std::result::Result<bool, Self::Error> {
        let key = format!("{}:{}", self.host, self.port);
        let fp = server_public_key.fingerprint(HashAlg::Sha256).to_string();
        let mut db = Self::hosts_db();
        match db.get(&key) {
            Some(known) if *known == fp => Ok(true),
            Some(_) => Ok(false), // 指纹变化，拒绝连接（防中间人）
            None => {
                db.insert(key, fp);
                Self::save_hosts_db(&db);
                Ok(true)
            }
        }
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

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

/// 建立连接并按指定方式认证。只尝试一次、只用一份凭据。
pub async fn establish(params: &ConnParams) -> Result<client::Handle<ClientHandler>> {
    let config = Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(15)),
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
            let pw = params
                .password
                .as_deref()
                .ok_or_else(|| Error::msg("未提供密码"))?;
            ensure_auth(handle.authenticate_password(&params.user, pw).await?)?;
        }
        _ => {
            let path = params
                .key_path
                .as_deref()
                .ok_or_else(|| Error::msg("未指定私钥文件"))?;
            let key = russh::keys::load_secret_key(
                expand_tilde(path),
                params.passphrase.as_deref().filter(|s| !s.is_empty()),
            )?;
            // RSA 密钥协商服务端支持的最优签名哈希（rsa-sha2-512/256）
            let hash = handle.best_supported_rsa_hash().await?.flatten();
            let auth_key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
            ensure_auth(handle.authenticate_publickey(&params.user, auth_key).await?)?;
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
                        *self.handle.lock().await = Some(h);
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
