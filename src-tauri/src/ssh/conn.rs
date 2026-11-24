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
    /// 用户偏好：是否允许断线自动重连。**纯偏好**，不参与任何状态判定
    /// （`settings_set` 会批量覆写它，若让它承载状态语义就会让僵尸连接复活）。
    pub auto_reconnect: AtomicBool,
    /// 连接的权威状态机（见 `ConnState` 与本文件顶部的转移表）。
    /// 取代旧的两个独立布尔位 `disconnect_pending` / 隐式表成员资格：
    /// 「已放弃但还不能拆」现在是一个**显式状态** `Draining`，而不是挂在某个位上的注释。
    pub state: Mutex<ConnState>,
    /// 建连时刻，供 listconn 报告存活时长
    pub created_at: std::time::SystemTime,
}

/// 连接状态。**这是唯一的权威状态**。
///
/// 四个状态都不带数据：判定所需的一切（阻碍量、尝试次数、错误信息）都由**事件**传入。
/// 曾经让状态携带 `attempts` / `obstacles` / `reason`，结果是「写入一份、判定又用另一份」，
/// 出现「先丢失信息再去反推」的错位。现在状态只回答「在哪个阶段」，其余由事件带来。
///
/// 转移表：
/// ```text
/// Alive        --ReconnectStarted--> Reconnecting       （由退避任务在入口写，唯一写入点）
/// Alive        --放弃且无阻碍------> Terminated
/// Alive        --放弃但有阻碍------> Draining
/// Reconnecting --AttemptSucceeded--> Alive              （与尝试次数无关）
/// Reconnecting --AttemptFailed 未到顶--> Reconnecting
/// Reconnecting --AttemptFailed 到顶 --> Draining
/// Reconnecting --放弃---------------> Draining
/// Draining     --阻碍清空-----------> Terminated
/// Draining     --Reacquired---------> Alive
/// Terminated   --任何---------------> Terminated        （无副作用）
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    /// 已建连且 TCP 存活。
    Alive,
    /// 断线退避中。由退避任务写入。
    Reconnecting,
    /// 已放弃（或被判定终止），但尚有阻碍不能立刻拆：该连接上还有进行中的传输，
    /// 或后端 pane 表里还有引用。阻碍清空时转入 `Terminated`。
    Draining,
    /// 已拆除：已出 conns 表，无后台任务，无 handle。终态。
    Terminated,
}

impl ConnState {
    /// 是否处于终态（不可再转移）。
    pub fn is_terminal(&self) -> bool {
        matches!(self, ConnState::Terminated)
    }
}

/// 阻碍拆除的剩余量。两个计数是**唯一**的拆除判据。
///
/// 为什么必须是计数而不是布尔：批量上传/下载是逐项串行、每项独立 transfer_id，
/// 两项之间存在「本条已摘除、下条未注册」的空档。用「当前有无活跃传输」这个布尔
/// 判据会命中空档而误拆；用计数则在空档期仍为 >0（整批未结束），不会误判。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Obstacles {
    pub transfers: usize,
    pub panes: usize,
}

impl Obstacles {
    pub fn is_clear(&self) -> bool {
        self.transfers == 0 && self.panes == 0
    }
}

/// 状态机事件：外部输入的唯一入口。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnEvent {
    /// 退避任务启动：把 Alive 推进到 Reconnecting 并广播一次。
    /// 由 `trigger_reconnect` 在任务入口发出 —— 它是 `Reconnecting` 的唯一写入者。
    ReconnectStarted,
    /// 一次尝试成功。**不带次数**：成功转移与次数无关，这样「第 MAX 次成功却被
    /// 上限否决」在类型层面就不可能表达。
    AttemptSucceeded,
    /// 一次尝试失败。带上重试间隔与错误原文，使 `EmitWaiting` 无需占位/回调补齐。
    AttemptFailed {
        attempts: u32,
        retry_in: u64,
        error: String,
    },
    /// 放弃请求（用户/前端不再需要这条连接）。
    AbandonRequested { obstacles: Obstacles },
    /// 阻碍量变化（传输注册/摘除、pane 打开/关闭）。
    ObstaclesChanged { obstacles: Obstacles },
    /// 连接重新被引用（`pane_open` 成功）。
    Reacquired,
}

/// 状态转移的**纯函数**结果：新状态 + 需要执行的动作。
///
/// 动作与状态分离，是为了让转移本身可穷举单测（不需要真实网络/AppHandle），
/// 同时保证「该发事件的地方不会漏发」—— 动作随转移一起产出，漏发即编译不过。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// 广播 conn-state=connected
    EmitConnected,
    /// 广播 conn-state=reconnecting
    EmitReconnecting,
    /// 广播 conn-state=waiting
    EmitWaiting {
        error: String,
        retry_in: u64,
        attempts: u32,
    },
    /// 广播 conn-state=closed（尝试次数用尽）
    EmitExhausted { attempts: u32 },
    /// 广播 conn-state=closed（用户关闭自动重连 / 主动放弃）
    EmitClosed,
    /// 拆除：出表、断开 handle、清理 pane
    Teardown,
}

impl ConnState {
    /// 纯状态转移。**本函数是全部状态语义的唯一实现点**。
    ///
    /// 返回 `(新状态, 动作列表)`。非法/乱序事件原样返回、不产生动作（容错优先：
    /// 真实系统的时序竞争会让事件乱序到达，忽略比崩溃好）。
    pub fn apply(self, event: &ConnEvent) -> (Self, Vec<Effect>) {
        use ConnState::*;
        match (self, event) {
            // ---------- 终态吸收一切 ----------
            (Terminated, _) => (Terminated, vec![]),

            // ---------- Alive ----------
            (Alive, ConnEvent::ReconnectStarted) => (Reconnecting, vec![Effect::EmitReconnecting]),
            (Alive, ConnEvent::AbandonRequested { obstacles }) => {
                if obstacles.is_clear() {
                    (Terminated, vec![Effect::Teardown])
                } else {
                    (Draining, vec![])
                }
            }
            // Alive 下的其余事件：重连结果属于时序错乱（连接已被别处恢复），
            // 已有的 handle 有效，不能被过期结果覆盖；Reacquired/阻碍变化无需转移。
            (Alive, _) => (Alive, vec![]),

            // ---------- Reconnecting ----------
            // 成功恒定回到 Alive：与 attempts 无关，上限只作用于失败路径。
            (Reconnecting, ConnEvent::AttemptSucceeded) => (Alive, vec![Effect::EmitConnected]),
            (Reconnecting, ConnEvent::AttemptFailed { attempts, retry_in, error }) => {
                if *attempts >= MAX_RECONNECT_ATTEMPTS {
                    (
                        Draining,
                        vec![Effect::EmitExhausted { attempts: *attempts }],
                    )
                } else {
                    (
                        Reconnecting,
                        vec![Effect::EmitWaiting {
                            error: error.clone(),
                            retry_in: *retry_in,
                            attempts: *attempts,
                        }],
                    )
                }
            }
            // 放弃 → 进 Draining，并告知前端终态。
            (Reconnecting, ConnEvent::AbandonRequested { .. }) => {
                (Draining, vec![Effect::EmitClosed])
            }
            // 重连期间重复的 ReconnectStarted / 阻碍变化：不改变状态
            (Reconnecting, _) => (Reconnecting, vec![]),

            // ---------- Draining ----------
            // 阻碍清空 → 真正拆除。Draining 唯一的正常出口。
            (Draining, ConnEvent::ObstaclesChanged { obstacles }) => {
                if obstacles.is_clear() {
                    (Terminated, vec![Effect::Teardown])
                } else {
                    (Draining, vec![])
                }
            }
            // 重新被 pane 引用 → 撤销放弃。让 Draining 不是单向的
            // （用户关掉文件面板又打开、hssh 按名复用都可恢复）。
            (Draining, ConnEvent::Reacquired) => (Alive, vec![]),
            // 退避任务在 Draining 期间仍可能回连成功（任务尚未观察到放弃）。
            // 接受它并回到 Alive —— 把一条活着且仍被引用的连接丢掉更糟。
            (Draining, ConnEvent::AttemptSucceeded) => (Alive, vec![Effect::EmitConnected]),
            (Draining, _) => (Draining, vec![]),
        }
    }
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
        // 判定必须挪到 blocking 池：本回调由 russh 的会话任务在 tokio worker 线程上调用，
        // 而下面的临界区是线程锁 + 跨进程 flock（**无超时**）+ 同步文件读写。留在原地时，
        // 多连接并发 + 另一实例持锁就会把 worker 线程占死，整个执行器随之停摆——外部表现
        // 正是「应用假死、只能重启」。挪走后 worker 不再被阻塞，判定逻辑与结果逐字节不变；
        // 线程 panic 按失败关闭处理（unwrap_or(false) 即拒绝连接）。
        Ok(tokio::task::spawn_blocking(move || verify_and_pin(&key, &fp))
            .await
            .unwrap_or(false))
    }
}

/// 校验并（首次）钉住主机指纹，返回是否放行。
///
/// **阻塞函数**：内含线程锁、跨进程 flock（无超时）与同步文件 I/O，只允许在
/// spawn_blocking 线程上调用。任何异常路径一律返回 false（失败关闭）——指纹库损坏时
/// 尤其不能「清空后重新信任」，那等于放行中间人。
fn verify_and_pin(key: &str, fp: &str) -> bool {
    // 线程级锁：防同进程多 SSH 并发（flock 进程级，同进程内不互斥）
    let _guard = HOSTS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = match known_hosts_path() {
        Ok(p) => p,
        Err(_) => return false,
    };
    // 跨进程文件锁：防多进程并发连接新主机时互相覆盖已固定指纹
    let lock_path = path.with_extension("json.lock");
    let lock_file = match std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&lock_path)
    {
        Ok(f) => f,
        Err(_) => return false,
    };
    if fs2::FileExt::lock_exclusive(&lock_file).is_err() {
        return false;
    }
    let result = (|| {
        let mut db = match ClientHandler::load_hosts_db() {
            Ok(db) => db,
            // 指纹库损坏：安全起见拒绝连接，而非清空后重新信任（否则等于放行 MITM）
            Err(()) => return false,
        };
        match db.get(key) {
            Some(known) if known == fp => true,
            Some(_) => false, // 指纹变化，拒绝连接（防中间人）
            None => {
                db.insert(key.to_string(), fp.to_string());
                ClientHandler::save_hosts_db(&db);
                true // 首次信任
            }
        }
    })();
    let _ = fs2::FileExt::unlock(&lock_file);
    result
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

/// 重连任务存活期间的 RAII 守卫：Drop 时复位 `Connection::reconnecting`。
///
/// 为什么必须是 RAII 而非循环末尾一句 store：`reconnecting` 是 `trigger_reconnect` 的
/// 幂等占位标志（入口 `swap(true)` 后若已是 true 即直接返回）。若任务因 panic 或被取消
/// 而没走到末尾的 store，该标志永久为 true，此后**所有**重连触发都在入口处直接返回——
/// 这条连接再也无法重连，pane 永远停在「等待重连」，只能重启应用。
/// 正常 break、panic、取消三种出口，Drop 都会执行。
struct ReconnectGuard(Arc<Connection>);

impl Drop for ReconnectGuard {
    fn drop(&mut self) {
        self.0.reconnecting.store(false, Ordering::SeqCst);
        // 任务退出时若状态仍停在 `Reconnecting`，把它推到 `Draining`：
        // 那个状态意味着「退避任务存在」，任务没了就必须有人接手收尾，
        // 否则它会永久滞留（不出表、不重连、sweep 也不管它 —— sweep 只扫 Draining）。
        //
        // 正常路径下状态早已被推进（成功→Alive、到顶/放弃→Draining），这里是空操作。
        // Drop 不能 await，用 try_lock 尽力而为：抢不到锁说明另一条路径正持有并处理
        // 状态，那次处理必然会把 Reconnecting 带出去，不必也不该在这里重试。
        if let Ok(mut guard) = self.0.state.try_lock() {
            if matches!(*guard, ConnState::Reconnecting) {
                *guard = ConnState::Draining;
            }
        }
    }
}

/// 断线重连的尝试次数上限。
///
/// 退避曲线 1s→2s→4s→…→30s 封顶，累计到第 100 次约需 **48 分钟**（前 5 次 31s，
/// 之后每次 30s）。取值意图：
/// · 常见的短暂断网、机器休眠、Wi-Fi 漫游、sshd 重启，都在这个窗口内自愈；
/// · 长期不可达（服务器下线、地址变了）不会再无限刷 `waiting` 事件，48 分钟后停在
///   `closed` 出口，由用户决定重连还是关掉分屏。
/// 刻意不用「无限 + 只靠用户关自动重连」：那会留下一条永远每 30s 醒一次的后台任务。
const MAX_RECONNECT_ATTEMPTS: u32 = 100;

impl Connection {
    pub fn new(id: String, params: ConnParams, auto_reconnect: bool) -> Self {
        Self {
            id,
            params,
            handle: Mutex::new(None),
            sftp: Mutex::new(None),
            reconnecting: AtomicBool::new(false),
            auto_reconnect: AtomicBool::new(auto_reconnect),
            // 构造即 Alive：`ssh_connect` 只有建连成功才构造并入表（失败路径不构造）。
            // CLI 的 `hsshx attach` 同构。因此不存在可观测的 Connecting 状态。
            state: Mutex::new(ConnState::Alive),
            created_at: std::time::SystemTime::now(),
        }
    }

    /// 是否正处于重连退避中（`trigger_reconnect` 任务存活期间为 true）。
    ///
    /// **仅表示「退避任务是否在跑」**，不再由状态机同步。曾经的错误设计是让它
    /// 兼职「展示用状态」，由 `apply_event` 依据 `ConnState` 反写；结果一旦状态机
    /// 先被推进到 `Reconnecting`，这里就变成 true，`trigger_reconnect` 的入口幂等
    /// 检查随即 `swap(true)` 拿到旧值 true 直接返回 —— **重连任务根本不启动**，
    /// 且守卫未构造、标志永久为 true，该连接再也无法重连。
    /// 展示层要判「是否重连中」请用 `state()`。
    pub fn is_reconnecting(&self) -> bool {
        self.reconnecting.load(Ordering::SeqCst)
    }

    /// 当前状态快照（克隆，不持锁跨越 await）。
    pub async fn state(&self) -> ConnState {
        self.state.lock().await.clone()
    }

    /// 状态机唯一入口：取状态 → 纯转移 → 写回。返回转移产生的动作。
    ///
    /// 把「读-算-写」收在一把锁里，杜绝两个并发事件各自读到旧状态再双写。
    /// **不触碰 `reconnecting` 原子位**：那是任务互斥量，与状态是两个正交维度。
    pub async fn apply_event(&self, event: ConnEvent) -> Vec<Effect> {
        let mut guard = self.state.lock().await;
        let (next, effects) = guard.clone().apply(&event);
        *guard = next;
        effects
    }

    /// 以当前阻碍量发一次 `ObstaclesChanged`。
    ///
    /// 阻碍量由调用方（持 `AppState` 的 lib.rs）按权威表算好后传入，本层不反查全局表 ——
    /// 避免「状态机去查外部表」造成锁序倒置与判据不同步。
    pub async fn notify_obstacles(&self, obstacles: Obstacles) -> Vec<Effect> {
        self.apply_event(ConnEvent::ObstaclesChanged { obstacles }).await
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
    ///
    /// **循环只负责「驱动」**：每次迭代把事件喂给状态机（`apply_event`），再由
    /// `run_effects` 执行返回的动作。它自己不做任何状态判断 —— 判断全在
    /// `ConnState::apply` 这个纯函数里，可穷举单测。
    pub fn trigger_reconnect(self: Arc<Self>, app: AppHandle) {
        if self.reconnecting.swap(true, Ordering::SeqCst) {
            return;
        }
        tokio::spawn(async move {
            // 任何退出路径（正常 break、panic、任务被取消）都复位占位标志
            let _guard = ReconnectGuard(self.clone());
            // 由任务（而非 pane.rs）把状态推进到 Reconnecting，并广播一次。
            // 这是 `Reconnecting` 的唯一写入点，避免「谁都能改状态」的错位。
            let effects = self.apply_event(ConnEvent::ReconnectStarted).await;
            run_effects(&self, &app, effects).await;

            let mut delay = 1u64;
            let mut attempts: u32 = 0;
            loop {
                // 入口：状态已非 Reconnecting（被放弃/已恢复/已拆除）就停
                if !matches!(self.state().await, ConnState::Reconnecting) {
                    break;
                }
                // 用户关闭自动重连 → 按放弃处理。这里拿不到 AppState（算不出阻碍量），
// 故传空阻碍：无 pane/传输时立即拆除；有的话进 Draining，由 lib.rs 的
// sweep_draining 周期结算（阻碍量的权威在 AppState 侧，不在本层反查）。
                if !self.auto_reconnect.load(Ordering::SeqCst) {
                    let effects = self
                        .apply_event(ConnEvent::AbandonRequested {
                            obstacles: Obstacles::default(),
                        })
                        .await;
                    run_effects(&self, &app, effects).await;
                    break;
                }

                attempts += 1;
                match establish(&self.params).await {
                    Ok(h) => {
                        // establish 可能耗时数秒，期间用户可能已放弃该连接。
                        // 必须在**持有 handle 锁的临界区内**完成「判状态 + 写入」，
                        // 与 teardown 的 handle.take() 互斥，否则会泄漏幽灵连接。
                        let mut guard = self.handle.lock().await;
                        // 只判是否已被放弃 —— **不看 attempts**。本轮成功不该被上限否决
                        // （第 MAX 次恰好成功时，那是最后一个能救回来的连接）。
                        let abandoned = matches!(
                            self.state().await,
                            ConnState::Draining | ConnState::Terminated
                        );
                        if abandoned {
                            drop(guard);
                            let _ = h
                                .disconnect(russh::Disconnect::ByApplication, "bye", "zh")
                                .await;
                            break;
                        }
                        *guard = Some(h);
                        drop(guard);
                        *self.sftp.lock().await = None; // 旧 SFTP 会话随连接失效
                        let effects = self.apply_event(ConnEvent::AttemptSucceeded).await;
                        run_effects(&self, &app, effects).await;
                        break;
                    }
                    Err(e) => {
                        let effects = self
                            .apply_event(ConnEvent::AttemptFailed {
                                attempts,
                                retry_in: delay,
                                error: e.to_string(),
                            })
                            .await;
                        for eff in effects {
                            // waiting 额外带上限值，便于前端显示「N/M」；其余动作
                            // （EmitExhausted / Teardown）照原样执行。
                            if let Effect::EmitWaiting { error, retry_in, attempts } = &eff {
                                let _ = app.emit(
                                    "conn-state",
                                    serde_json::json!({
                                        "connId": self.id,
                                        "state": "waiting",
                                        "error": error,
                                        "retryIn": retry_in,
                                        "attempts": attempts,
                                        "maxAttempts": MAX_RECONNECT_ATTEMPTS,
                                    }),
                                );
                            } else {
                                run_effect(&self, &app, eff).await;
                            }
                        }
                        // 达上限时状态机已转 Draining，下一轮入口即 break
                        if attempts >= MAX_RECONNECT_ATTEMPTS {
                            continue;
                        }
                        tokio::time::sleep(Duration::from_secs(delay)).await;
                        delay = (delay * 2).min(30);
                    }
                }
            }
        });
    }
}

/// 执行状态机产出的动作。
///
/// 「动作随转移一起产出」保证**该发的事件不会漏发**：只要转移表写了 `EmitClosed`，
/// 这里就必然发；不像旧实现靠在循环体里手写 emit，漏写一处就静默。
async fn run_effects(conn: &Arc<Connection>, app: &AppHandle, effects: Vec<Effect>) {
    for eff in effects {
        run_effect(conn, app, eff).await;
    }
}

/// 执行单个动作。
async fn run_effect(conn: &Arc<Connection>, app: &AppHandle, eff: Effect) {
    match eff {
        Effect::EmitConnected => {
            let _ = app.emit(
                "conn-state",
                serde_json::json!({ "connId": conn.id, "state": "connected" }),
            );
        }
        Effect::EmitReconnecting => {
            let _ = app.emit(
                "conn-state",
                serde_json::json!({ "connId": conn.id, "state": "reconnecting" }),
            );
        }
        // 调用方（退避循环）会展开自己的版本以补上 maxAttempts，通常不走这里。
        Effect::EmitWaiting { error, retry_in, attempts } => {
            let _ = app.emit(
                "conn-state",
                serde_json::json!({
                    "connId": conn.id,
                    "state": "waiting",
                    "error": error,
                    "retryIn": retry_in,
                    "attempts": attempts,
                    "maxAttempts": MAX_RECONNECT_ATTEMPTS,
                }),
            );
        }
        // 次数用尽：`auto_reconnect` 仍为 true，前端不能显示「自动重连已禁用」。
        Effect::EmitExhausted { attempts } => {
            let _ = app.emit(
                "conn-state",
                serde_json::json!({
                    "connId": conn.id,
                    "state": "closed",
                    "retryExhausted": true,
                    "attempts": attempts,
                }),
            );
        }
        Effect::EmitClosed => {
            let _ = app.emit(
                "conn-state",
                serde_json::json!({
                    "connId": conn.id,
                    "state": "closed",
                    "retryExhausted": false,
                }),
            );
        }
        // 重连任务内的 Teardown：只断开 handle，**不做出表/清 pane** ——
        // 那是 lib.rs 的职责（它持有 AppState），本层拿不到。
        Effect::Teardown => {
            if let Some(h) = conn.handle.lock().await.take() {
                let _ = h
                    .disconnect(russh::Disconnect::ByApplication, "bye", "zh")
                    .await;
            }
        }
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

    /// 回归锁：`reconnecting` 的复位必须由 Drop 保证，而非依赖函数走到末尾。
    ///
    /// 这是「只能重启应用」最隐蔽的一条路径：该标志是 `trigger_reconnect` 的幂等占位，
    /// 若任务 panic / 被取消而漏复位，此后**所有**触发都在入口 `swap` 掉直接返回，
    /// 这条连接永久失去自愈能力。用 panic 构造「没走到末尾」的场景来验证 Drop 兜住了。
    #[test]
    fn reconnect_flag_is_reset_even_when_task_panics() {
        let conn = Arc::new(Connection::new("c".into(), params_with_secrets(), true));
        conn.reconnecting.store(true, Ordering::SeqCst);

        let c = conn.clone();
        // catch_unwind 捕获 panic，模拟任务异常退出（Drop 仍会执行）
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = ReconnectGuard(c);
            panic!("模拟重连任务内部 panic");
        }));
        assert!(r.is_err(), "前置不成立：panic 未发生，测不到 Drop 路径");
        assert!(
            !conn.is_reconnecting(),
            "panic 后标志仍为 true：该连接将永久无法重连"
        );
    }

    /// 回归锁：正常作用域退出同样复位（RAII 覆盖所有出口，不只 panic 路径）。
    #[test]
    fn reconnect_flag_is_reset_on_normal_drop() {
        let conn = Arc::new(Connection::new("c".into(), params_with_secrets(), true));
        conn.reconnecting.store(true, Ordering::SeqCst);
        {
            let _guard = ReconnectGuard(conn.clone());
            assert!(conn.is_reconnecting(), "守卫存活期间标志应保持占位");
        }
        assert!(!conn.is_reconnecting(), "守卫 drop 后必须复位");
    }

    /// 回归锁：任务异常退出时，状态不得滞留 `Reconnecting`。
    ///
    /// `Reconnecting` 的语义是「退避任务存在」。任务没了就必须有人接手，否则该连接
    /// 永久停在「既不重连、也不出表」，而且 `sweep_draining` 只扫 `Draining`，
    /// 不会发现它 —— 这是一个会一直挂到重启应用的僵死态。
    /// `ReconnectGuard::drop` 负责把它推进到 `Draining`，交给周期 sweep 收尾。
    #[test]
    fn state_must_not_linger_in_reconnecting_after_task_exit() {
        let conn = Arc::new(Connection::new("c".into(), params_with_secrets(), true));
        conn.reconnecting.store(true, Ordering::SeqCst);
        *conn.state.try_lock().unwrap() = ConnState::Reconnecting;

        let c = conn.clone();
        // panic 模拟任务异常退出：正常路径下状态早被推进，只有异常退出会留在这里
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = ReconnectGuard(c);
            panic!("模拟重连任务内部 panic");
        }));
        assert!(r.is_err(), "前置不成立：panic 未发生");

        let st = *conn.state.try_lock().unwrap();
        assert_eq!(
            st,
            ConnState::Draining,
            "任务异常退出后状态必须推进到 Draining（sweep 才会接手），实际 {st:?}"
        );
        assert!(!conn.is_reconnecting(), "互斥位同样必须复位");
    }

    /// 守卫不得覆盖已推进的状态：正常退出时状态应保持原样。
    /// （例如成功 → Alive、到顶 → Draining，都不该被 Drop 改回去）
    #[test]
    fn guard_does_not_clobber_already_advanced_state() {
        for settled in [ConnState::Alive, ConnState::Draining, ConnState::Terminated] {
            let conn = Arc::new(Connection::new("c".into(), params_with_secrets(), true));
            conn.reconnecting.store(true, Ordering::SeqCst);
            *conn.state.try_lock().unwrap() = settled;

            {
                let _guard = ReconnectGuard(conn.clone());
            }
            let st = *conn.state.try_lock().unwrap();
            assert_eq!(st, settled, "Drop 不得把 {settled:?} 改掉");
        }
    }

    // ==========================================================================
    // 状态机（ConnState::apply）穷举测试
    //
    // 这是本次重构的核心保障：状态语义**全部**收敛在纯函数 `apply` 里，
    // 因此可以用穷举 + 不变量断言来锁死，而不必模拟真实网络。
    // ==========================================================================

    /// 所有状态 × 所有事件的笛卡尔积，断言不变量恒成立。
    ///
    /// 这是「不出现修复 bug 引出更多 bug」的结构性保障：不需要为每个缺陷写一条
    /// 针对性用例，只要不变量对**全部组合**成立，任何新增转移都不可能偷偷破坏它们。
    #[test]
    fn invariants_hold_for_every_state_and_event() {
        let clear = Obstacles::default();
        let busy_transfer = Obstacles { transfers: 1, panes: 0 };
        let busy_pane = Obstacles { panes: 2, transfers: 0 };

        let states = vec![
            ConnState::Alive,
            ConnState::Reconnecting,
            ConnState::Draining,
            ConnState::Terminated,
        ];
        let events = vec![
            ConnEvent::ReconnectStarted,
            ConnEvent::AttemptSucceeded,
            ConnEvent::AttemptFailed { attempts: 1, retry_in: 1, error: "e".into() },
            ConnEvent::AttemptFailed {
                attempts: MAX_RECONNECT_ATTEMPTS,
                retry_in: 30,
                error: "e".into(),
            },
            ConnEvent::AbandonRequested { obstacles: clear },
            ConnEvent::AbandonRequested { obstacles: busy_transfer },
            ConnEvent::ObstaclesChanged { obstacles: clear },
            ConnEvent::ObstaclesChanged { obstacles: busy_pane },
            ConnEvent::Reacquired,
        ];

        for s in &states {
            for e in &events {
                let (next, effects) = s.clone().apply(e);

                // INV-A：Terminated 是吸收态，且永不产生副作用
                if s.is_terminal() {
                    assert_eq!(&next, s, "Terminated 必须吸收 {e:?}");
                    assert!(effects.is_empty(), "Terminated 不得产生动作 {e:?}");
                }

                // INV-B：任何转移都不得产生重复的 Teardown（重复拆会二次断开）
                let teardowns = effects.iter().filter(|x| matches!(x, Effect::Teardown)).count();
                assert!(teardowns <= 1, "一次转移最多一个 Teardown: {s:?} + {e:?}");

                // INV-C：只有「放弃且无障碍」或「阻碍清空」才能拆。
                // 这条锁死了「批量传输中途被误拆」那个回归：有阻碍时绝不允许 Teardown。
                if teardowns == 1 {
                    let has_obstacle = match e {
                        ConnEvent::AbandonRequested { obstacles }
                        | ConnEvent::ObstaclesChanged { obstacles } => !obstacles.is_clear(),
                        _ => false,
                    };
                    assert!(
                        !has_obstacle,
                        "有阻碍却发生拆除：{s:?} + {e:?} —— 会中断传输或留僵尸 pane"
                    );
                    assert_eq!(next, ConnState::Terminated, "拆除后必须进 Terminated");
                }

                // INV-D：进入 Reconnecting 必须伴随 reconnecting 广播（前端要知道在重试）
                if matches!(next, ConnState::Reconnecting) && !matches!(s, ConnState::Reconnecting) {
                    assert!(
                        effects.iter().any(|x| matches!(x, Effect::EmitReconnecting)),
                        "进入 Reconnecting 未广播: {s:?} + {e:?}"
                    );
                }

                // INV-E：进入 Terminated 若来自 Draining/Abandon，不能静默 —— 要么拆，
                // 要么已由 closed 事件告知过用户。
                if matches!(next, ConnState::Terminated) && !s.is_terminal() {
                    assert!(!effects.is_empty(), "转入 Terminated 不得静默: {s:?} + {e:?}");
                }
            }
        }
    }

    /// 回归锁：次数上限**不得**否决一次成功重连。
    ///
    /// 这是曾经真实存在的缺陷：第 MAX 次尝试恰好成功时，若判据带次数，会把
    /// 刚建好的连接当作「已到上限」当场丢弃 —— 熬了 48 分钟最后一次连上却失败。
    /// 状态机里 `AttemptSucceeded` 的转移目标是**恒定** Alive，事件里根本不带次数。
    #[test]
    fn success_is_never_vetoed_by_attempt_cap() {
        let (next, effects) = ConnState::Reconnecting.apply(&ConnEvent::AttemptSucceeded);
        assert_eq!(next, ConnState::Alive, "成功必须回到 Alive");
        assert!(
            effects.iter().any(|e| matches!(e, Effect::EmitConnected)),
            "重连成功必须广播 connected"
        );
    }

    /// 回归锁：到达上限必须转入 Draining 并广播**次数用尽**的终态。
    ///
    /// 曾经的缺陷：达上限时就地 `break` 跳出循环，绕过了 closed 广播 ——
    /// 前端永远收不到终态，死连接不被回收，`retryExhausted` 成死字段。
    #[test]
    fn exhaustion_must_emit_closed_with_flag() {
        let (next, effects) = ConnState::Reconnecting.apply(&ConnEvent::AttemptFailed {
            attempts: MAX_RECONNECT_ATTEMPTS,
            retry_in: 30,
            error: "boom".into(),
        });
        assert_eq!(next, ConnState::Draining, "达上限应转入 Draining");
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::EmitExhausted { attempts } if *attempts == MAX_RECONNECT_ATTEMPTS)),
            "达上限必须广播 closed 且带次数，实际 {effects:?}"
        );
    }

    /// 回归锁：未达上限的失败必须继续重连并广播 waiting。
    #[test]
    fn failure_below_cap_keeps_retrying() {
        let (next, effects) = ConnState::Reconnecting.apply(&ConnEvent::AttemptFailed {
            attempts: 4,
            retry_in: 8,
            error: "boom".into(),
        });
        assert_eq!(next, ConnState::Reconnecting);
        let waited = effects.iter().any(|e| {
            matches!(e, Effect::EmitWaiting { retry_in, .. } if *retry_in == 8)
        });
        assert!(waited, "未达上限应广播带真实 retryIn 的 waiting，实际 {effects:?}");
    }

    /// 回归锁：批量传输期间不得拆除。
    ///
    /// 这是那个让批量上传在第二个文件全部失败的回归：用「当前无活跃传输」这个
    /// 布尔判据会命中两项之间的空档。用**计数**则不会 —— 只在归零时转移。
    #[test]
    fn batch_transfer_never_triggers_premature_teardown() {
        // 放弃时还有 3 个传输在排队 → 进 Draining，不拆
        let (s1, e1) = ConnState::Alive.apply(&ConnEvent::AbandonRequested {
            obstacles: Obstacles { transfers: 3, panes: 0 },
        });
        assert_eq!(s1, ConnState::Draining, "有传输应进 Draining");
        assert!(!e1.iter().any(|e| matches!(e, Effect::Teardown)), "不得拆除");

        // 传完第 1 个（3→2）→ 仍不拆
        let (s2, e2) = s1.apply(&ConnEvent::ObstaclesChanged {
            obstacles: Obstacles { transfers: 2, panes: 0 },
        });
        assert_eq!(s2, ConnState::Draining);
        assert!(!e2.iter().any(|e| matches!(e, Effect::Teardown)), "剩 2 个时不得拆");

        // 传完第 2 个（2→1）→ 仍不拆
        let (s3, e3) = s2.apply(&ConnEvent::ObstaclesChanged {
            obstacles: Obstacles { transfers: 1, panes: 0 },
        });
        assert!(!e3.iter().any(|e| matches!(e, Effect::Teardown)), "剩 1 个时不得拆");

        // 全部传完（1→0）→ 拆
        let (s4, e4) = s3.apply(&ConnEvent::ObstaclesChanged {
            obstacles: Obstacles::default(),
        });
        assert_eq!(s4, ConnState::Terminated, "归零后应拆除");
        assert!(e4.iter().any(|e| matches!(e, Effect::Teardown)), "归零必须拆");
    }

    /// 回归锁：Draining 不是单向的 —— 重新被引用可撤销放弃。
    /// （hfile 关面板后又打开、hssh 按名复用同一 connId 都依赖这一点）
    #[test]
    fn draining_can_be_revoked_by_reacquire() {
        let (draining, _) = ConnState::Alive.apply(&ConnEvent::AbandonRequested {
            obstacles: Obstacles { transfers: 1, panes: 0 },
        });
        let (back, effects) = draining.apply(&ConnEvent::Reacquired);
        assert_eq!(back, ConnState::Alive, "重新被引用应回到 Alive");
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Teardown)),
            "撤销放弃时不得拆除"
        );
    }

    /// 回归锁：`ReconnectStarted` 是进入 Reconnecting 的唯一入口且只广播一次。
    ///
    /// 这条锁的是那个 P0 回归的**状态侧**：曾经让 `pane.rs` 先发一个状态事件、
    /// 再由 `trigger_reconnect` 启动任务，结果状态事件顺带写了任务互斥位，
    /// 使任务启动被幂等检查挡掉。现在状态推进只由任务自己完成。
    #[test]
    fn reconnect_started_is_the_only_entry() {
        let (next, effects) = ConnState::Alive.apply(&ConnEvent::ReconnectStarted);
        assert_eq!(next, ConnState::Reconnecting);
        assert_eq!(
            effects.iter().filter(|e| matches!(e, Effect::EmitReconnecting)).count(),
            1,
            "进入重连必须且只广播一次"
        );
        // 重复启动不得重复广播
        let (again, effects2) = next.apply(&ConnEvent::ReconnectStarted);
        assert_eq!(again, ConnState::Reconnecting);
        assert!(effects2.is_empty(), "重复启动不应重复广播，实际 {effects2:?}");
    }

    /// 回归锁：上限必须是一个「够久但不无限」的值。
    ///
    /// 累计耗时 = 前 5 次 (1+2+4+8+16) + 剩余 95 次 × 30 ≈ 31 + 2850 秒 ≈ 48 分钟。
    /// 太小（一次 Wi-Fi 漫游就放弃），太大等于无限（每 30s 永久唤醒后台任务）。
    #[test]
    fn reconnect_attempt_cap_is_finite_and_reasonable() {
        assert!(MAX_RECONNECT_ATTEMPTS >= 10, "上限过小：常见网络抖动会直接放弃");
        assert!(MAX_RECONNECT_ATTEMPTS <= 1000, "上限过大：退避任务实际近似无限");
        // 按退避曲线推算总时长，锁住「约 48 分钟」这个量级
        let mut delay = 1u64;
        let mut total = 0u64;
        for _ in 0..MAX_RECONNECT_ATTEMPTS {
            total += delay;
            delay = (delay * 2).min(30);
        }
        assert!(
            (1200..=7200).contains(&total),
            "累计退避时长 {total}s 超出半小时~2 小时区间，与设计意图不符"
        );
    }

    /// 新连接从 Alive 起步（`ssh_connect` 只有在建连成功后才构造并入表）。
    /// 若初值不是 Alive，首次断线事件会被错误吸收。
    #[test]
    fn new_connection_starts_alive() {
        let conn = Connection::new("c".into(), params_with_secrets(), true);
        assert!(conn.auto_reconnect.load(Ordering::SeqCst));
        assert!(!conn.is_reconnecting(), "新建连接不应处于重连态");
    }
}
