//! HetuShell Tauri 后端：应用状态、command 注册、窗口毛玻璃效果。

pub mod error;
pub mod settings;
pub mod ssh;
mod cache;
mod local;
mod slot;
mod sshcfg;

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use base64::Engine;
use tauri::Manager;
use tokio::sync::{mpsc, Mutex};

use error::{Error, Result};
use settings::{Profile, SessionTab, Settings};
use ssh::conn::{ConnParams, Connection};
use ssh::pane::{PaneCmd, PaneCtl};

/// 全局状态：连接注册表 + pane 注册表 + 设置 + 连接项 + 传输控制表
pub struct AppState {
    conns: Mutex<HashMap<String, Arc<Connection>>>,
    panes: Mutex<HashMap<String, PaneCtl>>,
    settings: Mutex<Settings>,
    /// settings.json 上次加载时的 mtime；settings_get 对比它判断是否被其他实例改过
    settings_mtime: Mutex<Option<std::time::SystemTime>>,
    /// 连接项存于独立文件 profiles.json，与 settings 分离
    profiles: Mutex<Vec<Profile>>,
    /// 进行中的传输：transfer_id → 控制句柄（暂停/继续/取消），传输结束即移除
    transfers: Mutex<HashMap<String, Arc<ssh::sftp::TransferCtl>>>,
    /// 多实例 slot：持有对应 slots/<N>.lock 的 flock 到进程退出。
    /// None = 未分配（session_acquire 前）；Some((slot, file)) = 已分配。
    /// file drop 即释放锁，进程退出/崩溃内核自动回收。
    slot: Mutex<Option<(usize, std::fs::File)>>,
}

type State<'a> = tauri::State<'a, AppState>;

impl AppState {
    /// 获取连接（Agent 模块通过 app.state() 访问）
    pub async fn get_conn(&self, conn_id: &str) -> Result<Arc<Connection>> {
        self.conns
            .lock()
            .await
            .get(conn_id)
            .cloned()
            .ok_or_else(|| Error::msg("连接不存在或已关闭"))
    }
}

async fn get_conn(state: &AppState, conn_id: &str) -> Result<Arc<Connection>> {
    state.get_conn(conn_id).await
}

// ---------- 设置 ----------

#[tauri::command]
async fn settings_get(state: State<'_>) -> Result<Settings> {
    // 多实例共享同一份 settings.json：对比文件 mtime，若被其他实例改过则重新加载
    let stale = {
        let mtime = state.settings_mtime.lock().await;
        match settings::settings_path().ok().and_then(|p| std::fs::metadata(&p).ok().and_then(|m| m.modified().ok())) {
            Some(current) => mtime.map_or(true, |last| current != last),
            None => false,
        }
    };
    if stale {
        let s = settings::load();
        let m = settings::settings_path().ok().and_then(|p| std::fs::metadata(&p).ok().and_then(|m| m.modified().ok()));
        *state.settings.lock().await = s.clone();
        *state.settings_mtime.lock().await = m;
        return Ok(s);
    }
    Ok(state.settings.lock().await.clone())
}

#[tauri::command]
async fn settings_set(state: State<'_>, settings: Settings) -> Result<()> {
    // 连接项已独立到 profiles.json，settings 不再包含它们，无需特殊保护
    let auto = settings.auto_reconnect;
    // 跨进程文件锁保护写入，防多进程同时改设置互相覆盖
    let s = settings.clone();
    settings::update_locked::<Settings, _>(&settings::settings_path()?, |v| {
        *v = s;
        Ok(())
    })?;
    *state.settings.lock().await = settings;
    // 记录本次写入后的 mtime，避免下次 settings_get 误判为被其他实例改过
    *state.settings_mtime.lock().await =
        settings::settings_path().ok().and_then(|p| std::fs::metadata(&p).ok().and_then(|m| m.modified().ok()));
    // 自动重连开关改动后同步到所有存活连接（否则仅对新建连接生效）
    for conn in state.conns.lock().await.values() {
        conn.auto_reconnect.store(auto, Ordering::SeqCst);
    }
    Ok(())
}

// ---------- 连接配置（独立文件 profiles.json）----------

/// 手动保存的 profile + ~/.ssh/config 导入的 profile 合并列表
#[tauri::command]
async fn profiles_list(state: State<'_>) -> Result<Vec<Profile>> {
    let mut list = state.profiles.lock().await.clone();
    list.extend(sshcfg::import());
    Ok(list)
}

#[tauri::command]
async fn profile_save(state: State<'_>, profile: Profile) -> Result<()> {
    // 跨进程文件锁保护的读-改-写：从磁盘读最新值再改再写，防多进程并发增删互覆盖
    let updated = settings::update_locked_return::<Vec<Profile>, _>(
        &settings::profiles_path()?,
        |profiles| {
            profiles.retain(|p| p.id != profile.id);
            profiles.push(profile);
            Ok(())
        },
    )?;
    *state.profiles.lock().await = updated;
    Ok(())
}

#[tauri::command]
async fn profile_delete(state: State<'_>, id: String) -> Result<()> {
    let updated = settings::update_locked_return::<Vec<Profile>, _>(
        &settings::profiles_path()?,
        |profiles| {
            profiles.retain(|p| p.id != id);
            Ok(())
        },
    )?;
    *state.profiles.lock().await = updated;
    Ok(())
}

// ---------- 会话（按 slot 分片：session-<slot>.json）----------

#[tauri::command]
async fn session_acquire(state: State<'_>) -> Result<usize> {
    let mut guard = state.slot.lock().await;
    if let Some((slot, _)) = *guard {
        // 已分配：返回现有 slot，防前端 bug 重复调用导致旧锁 fd 被覆盖释放
        return Ok(slot);
    }
    let (slot, file) = slot::acquire_slot()?;
    *guard = Some((slot, file));
    Ok(slot)
}

#[tauri::command]
async fn session_release(state: State<'_>) -> Result<()> {
    // 显式释放（进程退出时内核也会自动释放 flock，此处供前端关闭流程调用）
    *state.slot.lock().await = None;
    Ok(())
}

#[tauri::command]
async fn session_get(state: State<'_>) -> Result<Vec<SessionTab>> {
    let guard = state.slot.lock().await;
    let slot = guard
        .as_ref()
        .ok_or_else(|| Error::msg("slot 未分配"))?
        .0;
    Ok(settings::load_session(slot))
}

#[tauri::command]
async fn session_set(tabs: Vec<SessionTab>, state: State<'_>) -> Result<()> {
    let guard = state.slot.lock().await;
    let slot = guard
        .as_ref()
        .ok_or_else(|| Error::msg("slot 未分配"))?
        .0;
    settings::save_session(slot, &tabs)
}

// ---------- 连接生命周期 ----------

#[tauri::command]
async fn ssh_connect(state: State<'_>, params: ConnParams) -> Result<String> {
    let handle = ssh::conn::establish(&params).await?;
    let conn_id = uuid::Uuid::new_v4().to_string();
    let auto = state.settings.lock().await.auto_reconnect;
    let conn = Arc::new(Connection::new(conn_id.clone(), params, auto));
    *conn.handle.lock().await = Some(handle);
    state.conns.lock().await.insert(conn_id.clone(), conn);
    Ok(conn_id)
}

#[tauri::command]
async fn ssh_disconnect(state: State<'_>, conn_id: String) -> Result<()> {
    // 有活跃传输时拒绝断开，避免中断下载/上传
    let has_transfers = state
        .transfers
        .lock()
        .await
        .values()
        .any(|t| t.conn_id() == conn_id);
    if has_transfers {
        return Err(Error::msg("该连接有进行中的传输，请等待完成或取消后再断开"));
    }
    if let Some(conn) = state.conns.lock().await.remove(&conn_id) {
        // 主动断开：关闭自动重连，优雅发送 disconnect
        conn.auto_reconnect.store(false, Ordering::SeqCst);
        if let Some(handle) = conn.handle.lock().await.take() {
            let _ = handle
                .disconnect(russh::Disconnect::ByApplication, "bye", "zh")
                .await;
        }
    }
    // 清理该连接下所有 pane
    state
        .panes
        .lock()
        .await
        .retain(|_, ctl| ctl.conn_id != conn_id);
    Ok(())
}

/// 查询某连接是否有进行中的传输（供前端 gcConnections 判断是否可安全断开）
#[tauri::command]
async fn conn_has_transfers(state: State<'_>, conn_id: String) -> Result<bool> {
    Ok(state
        .transfers
        .lock()
        .await
        .values()
        .any(|t| t.conn_id() == conn_id))
}

// ---------- Pane（PTY channel）----------

/// pane_id 由前端生成并持有；断线重连后前端用同一 id 重开 pane。
#[tauri::command]
async fn pane_open(
    app: tauri::AppHandle,
    state: State<'_>,
    conn_id: String,
    pane_id: String,
    cols: u32,
    rows: u32,
    on_event: tauri::ipc::Channel<ssh::pane::PaneEvent>,
) -> Result<()> {
    let conn = get_conn(&state, &conn_id).await?;
    let (tx, rx) = mpsc::unbounded_channel();
    // 先登记 PaneCtl 再启动任务：既避免任务在插入前就 emit pane-closed 造成漏删，
    // 也在重连复用同一 pane_id 时先关闭旧 channel 任务，杜绝两个任务并发向同一 pane 推流。
    if let Some(old) = state
        .panes
        .lock()
        .await
        .insert(
            pane_id.clone(),
            PaneCtl {
                tx,
                conn_id,
                local_pid: None,
                cwd: tokio::sync::Mutex::new(None),
            },
        )
    {
        let _ = old.tx.send(PaneCmd::Close);
    }
    if let Err(e) = ssh::pane::open(app, conn, cols, rows, rx, on_event).await {
        state.panes.lock().await.remove(&pane_id); // 打开失败，回收占位
        return Err(e);
    }
    Ok(())
}

/// 打开本地终端 pane（不经 SSH，直接本机 PTY 跑用户 shell）。
/// conn_id 记为 "local"，输入/resize/关闭与 SSH pane 共用同一套 command。
#[tauri::command]
async fn pane_open_local(
    state: State<'_>,
    pane_id: String,
    cols: u32,
    rows: u32,
    cwd: Option<String>,
    hssh_token: String,
    on_event: tauri::ipc::Channel<ssh::pane::PaneEvent>,
) -> Result<()> {
    let (tx, rx) = mpsc::unbounded_channel();
    // 同 pane_open：先登记占位（必要时关闭旧任务），再启动本地 PTY
    if let Some(old) = state.panes.lock().await.insert(
        pane_id.clone(),
        PaneCtl {
            tx,
            conn_id: "local".into(),
            local_pid: None,
            cwd: tokio::sync::Mutex::new(None),
        },
    ) {
        let _ = old.tx.send(PaneCmd::Close);
    }
    match local::open(cols, rows, cwd, hssh_token, state.settings.lock().await.shell.clone(), rx, on_event) {
        // 启动成功后回填 shell PID，供 local_cwd 读实时工作目录
        Ok(pid) => {
            if let Some(ctl) = state.panes.lock().await.get_mut(&pane_id) {
                ctl.local_pid = pid;
            }
            Ok(())
        }
        Err(e) => {
            state.panes.lock().await.remove(&pane_id);
            Err(e)
        }
    }
}

/// 读取本地终端 pane 的实时工作目录（经其 shell 的 /proc/<pid>/cwd）。
/// 用于「Ctrl+拖远端文件到本地终端」时确定下载落点。
#[tauri::command]
async fn local_cwd(state: State<'_>, pane_id: String) -> Result<String> {
    let pid = {
        let panes = state.panes.lock().await;
        panes
            .get(&pane_id)
            .and_then(|c| c.local_pid)
            .ok_or_else(|| Error::msg("本地终端未就绪"))?
    };
    local::cwd(pid)
}

/// 本地终端标签页信息（工作目录 + 前台进程名），供标签标题展示 `目录:进程`。
#[tauri::command]
async fn local_tab_info(state: State<'_>, pane_id: String) -> Result<local::TabInfo> {
    let pid = {
        let panes = state.panes.lock().await;
        panes
            .get(&pane_id)
            .and_then(|c| c.local_pid)
            .ok_or_else(|| Error::msg("本地终端未就绪"))?
    };
    local::tab_info(pid)
}

#[tauri::command]
async fn pane_input(state: State<'_>, pane_id: String, data: String) -> Result<()> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| Error::msg("输入数据编码错误"))?;
    let panes = state.panes.lock().await;
    let ctl = panes.get(&pane_id).ok_or_else(|| Error::msg("pane 不存在"))?;
    ctl.tx
        .send(PaneCmd::Data(bytes))
        .map_err(|_| Error::msg("pane 已关闭"))
}

#[tauri::command]
async fn pane_resize(state: State<'_>, pane_id: String, cols: u32, rows: u32) -> Result<()> {
    let panes = state.panes.lock().await;
    if let Some(ctl) = panes.get(&pane_id) {
        let _ = ctl.tx.send(PaneCmd::Resize { cols, rows });
    }
    Ok(())
}

#[tauri::command]
async fn pane_close(state: State<'_>, pane_id: String) -> Result<()> {
    if let Some(ctl) = state.panes.lock().await.remove(&pane_id) {
        let _ = ctl.tx.send(PaneCmd::Close);
    }
    Ok(())
}

/// 远程 pane 的 cwd 同步（OSC 7 → 前端 → 后端）
#[tauri::command]
async fn pane_set_cwd(state: State<'_>, pane_id: String, cwd: String) -> Result<()> {
    let panes = state.panes.lock().await;
    if let Some(ctl) = panes.get(&pane_id) {
        *ctl.cwd.lock().await = Some(cwd);
    }
    Ok(())
}

// ---------- SFTP ----------

#[tauri::command]
async fn sftp_stat(state: State<'_>, conn_id: String, path: String) -> Result<ssh::sftp::FileMeta> {
    let conn = get_conn(&state, &conn_id).await?;
    ssh::sftp::stat(&conn, &path).await
}

#[tauri::command]
async fn sftp_preview(
    state: State<'_>,
    conn_id: String,
    path: String,
    max_bytes: u64,
) -> Result<ssh::sftp::Preview> {
    let conn = get_conn(&state, &conn_id).await?;
    ssh::sftp::preview(&conn, &path, max_bytes).await
}

/// 图片整读预览（文件面板右键 / himage 命令）：本地直接读文件；远端经磁盘缓存。
/// 单张上限由设置 max_image_mb 决定（默认 128MB，范围 32–512，非法值回退 64MB）。
#[tauri::command]
async fn image_preview(state: State<'_>, conn_id: String, path: String) -> Result<cache::ImageData> {
    let max_bytes = cache::resolve_max_bytes(state.settings.lock().await.max_image_mb);
    if conn_id == "local" {
        // 同步文件读取放到阻塞线程池，不占用异步执行器
        tokio::task::spawn_blocking(move || cache::local_image(&path, max_bytes))
            .await
            .map_err(|e| Error::msg(format!("预览任务失败: {e}")))?
    } else {
        let conn = get_conn(&state, &conn_id).await?;
        cache::remote_image(&conn, &path, max_bytes).await
    }
}

/// 注册一次传输的控制句柄，返回它；传输结束务必调用 unregister_transfer 清理。
async fn register_transfer(state: &AppState, id: &str, conn_id: &str) -> Arc<ssh::sftp::TransferCtl> {
    let ctl = Arc::new(ssh::sftp::TransferCtl::new(conn_id.to_string()));
    state.transfers.lock().await.insert(id.to_string(), ctl.clone());
    ctl
}

async fn unregister_transfer(state: &AppState, id: &str) {
    state.transfers.lock().await.remove(id);
}

#[tauri::command]
async fn sftp_download(
    app: tauri::AppHandle,
    state: State<'_>,
    conn_id: String,
    remote_path: String,
    local_path: String,
    transfer_id: String,
) -> Result<()> {
    let conn = get_conn(&state, &conn_id).await?;
    let ctl = register_transfer(&state, &transfer_id, &conn_id).await;
    let r = ssh::sftp::download(&app, &conn, &ctl, &remote_path, &local_path, &transfer_id).await;
    unregister_transfer(&state, &transfer_id).await;
    r
}

#[tauri::command]
async fn sftp_upload(
    app: tauri::AppHandle,
    state: State<'_>,
    conn_id: String,
    local_path: String,
    remote_dir: String,
    transfer_id: String,
) -> Result<String> {
    let conn = get_conn(&state, &conn_id).await?;
    let ctl = register_transfer(&state, &transfer_id, &conn_id).await;
    let r = ssh::sftp::upload(&app, &conn, &ctl, &local_path, &remote_dir, &transfer_id).await;
    unregister_transfer(&state, &transfer_id).await;
    r
}

/// 远程 → 远程复制（面板条目拖到远程终端）：同连接走服务器内 cp 快路径，
/// 否则经客户端流式中转。返回目标端根路径。
#[tauri::command]
async fn sftp_copy_remote(
    app: tauri::AppHandle,
    state: State<'_>,
    src_conn_id: String,
    src_path: String,
    dst_conn_id: String,
    dst_dir: String,
    transfer_id: String,
) -> Result<String> {
    let src = get_conn(&state, &src_conn_id).await?;
    let dst = get_conn(&state, &dst_conn_id).await?;
    let ctl = register_transfer(&state, &transfer_id, &src_conn_id).await;
    let r = ssh::sftp::copy_remote(&app, &src, &dst, &ctl, &src_path, &dst_dir, &transfer_id).await;
    unregister_transfer(&state, &transfer_id).await;
    r
}

/// 对指定传输的控制句柄执行一个操作；句柄已移除（传输结束）时静默忽略，前端幂等。
async fn with_transfer(state: &AppState, id: &str, f: impl FnOnce(&ssh::sftp::TransferCtl)) {
    if let Some(ctl) = state.transfers.lock().await.get(id) {
        f(ctl);
    }
}

/// 暂停 / 继续 / 取消一个进行中的传输（按 transfer_id 定位控制句柄）。
#[tauri::command]
async fn transfer_pause(state: State<'_>, transfer_id: String) -> Result<()> {
    with_transfer(&state, &transfer_id, |c| c.pause()).await;
    Ok(())
}

#[tauri::command]
async fn transfer_resume(state: State<'_>, transfer_id: String) -> Result<()> {
    with_transfer(&state, &transfer_id, |c| c.resume()).await;
    Ok(())
}

#[tauri::command]
async fn transfer_cancel(state: State<'_>, transfer_id: String) -> Result<()> {
    with_transfer(&state, &transfer_id, |c| c.cancel()).await;
    Ok(())
}

#[tauri::command]
async fn sftp_list(
    state: State<'_>,
    conn_id: String,
    path: String,
) -> Result<Vec<ssh::sftp::RemoteEntry>> {
    let conn = get_conn(&state, &conn_id).await?;
    ssh::sftp::list(&conn, &path).await
}

#[tauri::command]
async fn remote_home(state: State<'_>, conn_id: String) -> Result<String> {
    let conn = get_conn(&state, &conn_id).await?;
    ssh::sftp::home(&conn).await
}

/// 通过 /proc/<pid>/cwd 读取远端 shell 实时工作目录
#[tauri::command]
async fn remote_cwd(state: State<'_>, conn_id: String, pid: u32) -> Result<String> {
    let conn = get_conn(&state, &conn_id).await?;
    ssh::sftp::proc_cwd(&conn, pid).await
}

/// 系统默认下载目录（Linux 尊重 XDG，macOS/Windows 为各自 Downloads），兜底 ~/Downloads
#[tauri::command]
fn default_download_dir() -> String {
    dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 系统已安装字体族列表（去重、排序）。Linux/macOS 走 fontconfig 的 `fc-list`；
/// Windows 用 PowerShell 枚举。取不到时返回空表，前端仍展示内置默认字体分组。
#[tauri::command]
fn list_fonts() -> Vec<String> {
    use std::collections::BTreeSet;
    let mut set: BTreeSet<String> = BTreeSet::new();

    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(out) = std::process::Command::new("fc-list").args([":", "family"]).output() {
            if out.status.success() {
                for line in String::from_utf8_lossy(&out.stdout).lines() {
                    // 每行形如 "Fam1,Fam2"（多语言别名）；逐一去空白收集
                    for fam in line.split(',') {
                        let f = fam.trim();
                        if !f.is_empty() {
                            set.insert(f.to_string());
                        }
                    }
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        let script = "[void][System.Reflection.Assembly]::LoadWithPartialName('System.Drawing');\
            (New-Object System.Drawing.Text.InstalledFontCollection).Families|ForEach-Object{$_.Name}";
        if let Ok(out) = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .output()
        {
            if out.status.success() {
                for line in String::from_utf8_lossy(&out.stdout).lines() {
                    let f = line.trim();
                    if !f.is_empty() {
                        set.insert(f.to_string());
                    }
                }
            }
        }
    }

    set.into_iter().collect()
}

// ---------- 本地文件系统（文件管理器面板） ----------

#[tauri::command]
fn local_list(dir: String) -> Result<Vec<local::LocalEntry>> {
    local::list_dir(&dir)
}

#[tauri::command]
fn local_home() -> String {
    local::home_dir()
}

/// 读取用户选择的私钥文件内容（填入连接对话框的密钥文本框，随后自存到 profiles.json）。
/// 限制 256KB，避免误选大文件。
#[tauri::command]
fn read_key_file(path: String) -> Result<String> {
    let meta = std::fs::metadata(&path)?;
    if meta.len() > 256 * 1024 {
        return Err(Error::msg("文件过大，看起来不是私钥文件"));
    }
    Ok(std::fs::read_to_string(&path)?)
}

/// 读取 hssh --exec/--file/--stdin 写入的临时喂入文件，读完即删。
/// 限制 1MB；无论读取成功与否都尝试删除临时文件，避免残留。
#[tauri::command]
fn read_feed_file(path: String) -> Result<String> {
    let meta = std::fs::metadata(&path)?;
    if meta.len() > 1024 * 1024 {
        let _ = std::fs::remove_file(&path);
        return Err(Error::msg("喂入文件过大（上限 1MB）"));
    }
    let result = std::fs::read_to_string(&path);
    let _ = std::fs::remove_file(&path);
    Ok(result?)
}

/// 以 base64 读取任意文件内容（供 Agent 图片上传等场景使用）。
/// 限制 max_bytes（默认建议 10MB），返回 data: URL 格式的字符串。
#[tauri::command]
fn read_file_base64(path: String, max_bytes: u64) -> Result<String> {
    let meta = std::fs::metadata(&path)?;
    if meta.len() > max_bytes {
        return Err(Error::msg(format!(
            "文件过大（{}MB，上限 {}MB）",
            meta.len() / 1024 / 1024,
            max_bytes / 1024 / 1024
        )));
    }
    let data = std::fs::read(&path)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&data))
}

/// 用系统默认浏览器打开外部链接（终端里 Ctrl+单击 URL 触发）。
/// 仅放行 http/https，杜绝把任意字符串当命令参数注入到系统 opener。
#[tauri::command]
fn open_external(url: String) -> Result<()> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err(Error::msg("仅支持打开 http/https 链接"));
    }
    open_with_system(&url)
}

/// 用系统默认应用打开本地文件（双击终端中的文件路径触发）。
/// 经下载缓存后打开的远程文件也走此命令。
fn open_with_system(path: &str) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        // xdg-open 是 freedesktop 标准分发器（GNOME/KDE/XFCE 等各桌面均会路由到默认应用），
        // 再以 gio open 兜底，覆盖个别缺 xdg-utils 但装了 glib 的环境。
        let attempts: [(&str, &[&str]); 2] = [("xdg-open", &[]), ("gio", &["open"])];
        let mut last = String::from("无可用打开器");
        for (bin, pre) in attempts {
            match std::process::Command::new(bin).args(pre).arg(path).spawn() {
                Ok(_) => return Ok(()),
                Err(e) => last = format!("{bin}: {e}"),
            }
        }
        return Err(Error::msg(format!("打开失败（xdg-open/gio 均不可用）: {last}")));
    }
    #[cfg(not(target_os = "linux"))]
    {
        #[cfg(target_os = "macos")]
        let mut cmd = {
            let mut c = std::process::Command::new("open");
            c.arg(path);
            c
        };
        #[cfg(target_os = "windows")]
        let mut cmd = {
            // start 是 cmd 内建命令；空标题占位 "" 避免带引号的路径被当成窗口标题
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", "start", "", path]);
            c
        };
        cmd.spawn().map_err(|e| Error::msg(format!("打开失败: {e}")))?;
        Ok(())
    }
}

/// 用系统默认应用打开本地文件路径。
#[tauri::command]
fn open_path(path: String) -> Result<()> {
    if !std::path::Path::new(&path).exists() {
        return Err(Error::msg("文件不存在"));
    }
    open_with_system(&path)
}

/// 返回连接专属缓存目录（/tmp/hetushell_cache/<conn_id>），自动创建。
#[tauri::command]
async fn cache_dir(conn_id: String) -> Result<String> {
    let dir = std::env::temp_dir().join("hetushell_cache").join(&conn_id);
    tokio::fs::create_dir_all(&dir).await?;
    Ok(dir.to_string_lossy().into_owned())
}

// ---------- 应用入口 ----------

/// 从最大化还原：在后端直接获取屏幕尺寸并设置窗口大小，避免前端 IPC 竞态。
/// 按设置中的 restore_size 百分比计算，兜底 1280×820（与窗口初始配置一致）。
#[tauri::command]
async fn restore_window_size(app: tauri::AppHandle) -> Result<()> {
    let window = app.get_webview_window("main").ok_or_else(|| Error::msg("主窗口不存在"))?;
    let settings = settings::load();
    let pct = (settings.restore_size.max(50).min(90)) as f64 / 100.0;

    let (mut lw, mut lh) = (1280.0_f64, 820.0_f64);
    if let Ok(Some(monitor)) = window.current_monitor() {
        let sf = monitor.scale_factor();
        let size = monitor.size();
        let mw = size.width as f64 / sf;
        let mh = size.height as f64 / sf;
        if mw >= 700.0 && mh >= 500.0 {
            lw = mw;
            lh = mh;
        }
    }

    let w = (lw * pct).round() as u32;
    let h = (lh * pct).round() as u32;
    window
        .set_size(tauri::LogicalSize::new(w, h))
        .map_err(|e| Error::msg(format!("设置窗口大小失败: {e}")))?;
    Ok(())
}

/// 自研窗口状态：保存/恢复窗口尺寸、位置与最大化状态（替代 tauri-plugin-window-state）。
///
/// 为什么自研：插件 restore 把 set_size→maximize→show 在窗口尚未映射时排队，窗口管理器把
/// 「最大化前几何」记录为排队中的中间值，拖拽标题栏（WM 标准行为=取消最大化并还原到该
/// 几何）时窗口塌缩成错乱尺寸。自研恢复在窗口几何稳定后再 maximize，「最大化前几何」即为
/// 稳定后的合法尺寸——「启动恢复最大化」体验与拖拽不塌缩两者兼得。
mod window_state {
    use serde_json::Value;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Arc;
    use tauri::{Manager, PhysicalPosition, PhysicalSize, WebviewWindow};

    const FILE: &str = ".window-state.json";

    /// 与其他持久化配置同目录（~/.config/hetushell/）：config_dir() 自带 create_dir_all，
    /// 全新安装也能落盘。此前用 app_config_dir()（~/.config/dev.hetushell.app/）时该目录
    /// 已无任何代码创建，写盘静默 ENOENT，导致新装系统「最大化后关闭仍不恢复」必现。
    fn state_path() -> Option<PathBuf> {
        crate::settings::config_dir().ok().map(|d| d.join(FILE))
    }

    /// 按设置中的 restore_size 百分比算出的物理尺寸；拿不到显示器信息时兜底 1280×820
    /// （与窗口初始配置一致）。
    fn size_by_settings(window: &WebviewWindow) -> (u32, u32) {
        if let Ok(Some(monitor)) = window.current_monitor() {
            let s = monitor.size();
            if s.width > 0 && s.height > 0 {
                let pct = crate::settings::load().restore_size.clamp(50, 90) as f64 / 100.0;
                return (
                    (s.width as f64 * pct).round() as u32,
                    (s.height as f64 * pct).round() as u32,
                );
            }
        }
        (1280, 820)
    }

    /// 恢复窗口尺寸/位置；返回 true 表示需要恢复最大化（由调用方延迟探测执行）。
    /// 无历史状态（全新安装）时按 restore_size 设定稳定尺寸并同样尝试最大化——先给 WM
    /// 一个合法的「最大化前几何」，新装首次拖拽标题栏也不会塌缩。
    /// 尺寸超过所有显示器并集边界（跨会话缩放错位）时回退为 restore_size 百分比；
    /// 位置仅在目标点位于某个显示器内时恢复（换显示器布局后不把窗口甩出屏幕）。
    pub fn restore(window: &WebviewWindow) -> bool {
        let saved = state_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.get("main").cloned());
        let Some(main) = saved else {
            let (w, h) = size_by_settings(window);
            let _ = window.set_size(PhysicalSize::new(w, h));
            return true;
        };

        let should_maximize = main.get("maximized").and_then(Value::as_bool) == Some(true);

        let (mut w, mut h) = (1280_u32, 820_u32);
        if let (Some(wv), Some(hv)) = (
            main.get("width").and_then(Value::as_u64),
            main.get("height").and_then(Value::as_u64),
        ) {
            if within_union(window, wv, hv) {
                w = wv.min(u32::MAX as u64) as u32;
                h = hv.min(u32::MAX as u64) as u32;
            } else {
                (w, h) = size_by_settings(window);
            }
        }
        let _ = window.set_size(PhysicalSize::new(w, h));

        if let (Some(x), Some(y)) = (
            main.get("x").and_then(Value::as_i64),
            main.get("y").and_then(Value::as_i64),
        ) {
            if let Ok(monitors) = window.available_monitors() {
                let inside = monitors.iter().any(|m| {
                    let p = m.position();
                    let s = m.size();
                    x >= p.x as i64
                        && x < (p.x + s.width as i32) as i64
                        && y >= p.y as i64
                        && y < (p.y + s.height as i32) as i64
                });
                if inside {
                    let _ = window.set_position(PhysicalPosition::new(x as i32, y as i32));
                }
            }
        }

        should_maximize
    }

    /// 窗口尺寸 (w,h) 是否不超过所有显示器并集边界（允许合法的多屏跨屏布局）。
    /// 拿不到显示器信息时保守放行（不修改）。
    fn within_union(window: &WebviewWindow, w: u64, h: u64) -> bool {
        let Ok(monitors) = window.available_monitors() else {
            return true;
        };
        if monitors.is_empty() {
            return true;
        }
        let mut min_x = i32::MAX;
        let mut min_y = i32::MAX;
        let mut max_x = i32::MIN;
        let mut max_y = i32::MIN;
        for m in monitors {
            let p = m.position();
            let s = m.size();
            min_x = min_x.min(p.x);
            min_y = min_y.min(p.y);
            max_x = max_x.max(p.x + s.width as i32);
            max_y = max_y.max(p.y + s.height as i32);
        }
        let union_w = (max_x - min_x) as u64;
        let union_h = (max_y - min_y) as u64;
        union_w > 0 && w <= union_w && h <= union_h
    }

    /// 延迟恢复最大化：等窗口几何稳定（Resized 静默 300ms）后 maximize，
    /// 让 WM 把「最大化前几何」记录为稳定后的合法尺寸。2s 兜底覆盖
    /// 「恢复尺寸 == 初始尺寸、无 Resized 事件」的情况。done 保证只探测一轮。
    pub fn restore_maximize(window: &WebviewWindow) {
        let done = Arc::new(AtomicBool::new(false));
        let gen = Arc::new(AtomicU32::new(0));
        let w = window.clone();
        let w1 = w.clone();
        let done1 = done.clone();
        let gen1 = gen.clone();
        w.on_window_event(move |event| {
            if !matches!(event, tauri::WindowEvent::Resized(_)) {
                return;
            }
            // 已恢复最大化后不再处理后续 resize（窗口生命周期内零开销）
            if done1.load(Ordering::SeqCst) {
                return;
            }
            let g = gen1.fetch_add(1, Ordering::SeqCst) + 1;
            let w = w1.clone();
            let gen = gen1.clone();
            let done = done1.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                if gen.load(Ordering::SeqCst) != g {
                    return; // 期间尺寸继续变化，等待下一轮
                }
                if done.swap(true, Ordering::SeqCst) {
                    return;
                }
                probe_maximize(w).await;
            });
        });
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
            if done.swap(true, Ordering::SeqCst) {
                return;
            }
            probe_maximize(w).await;
        });
    }

    /// 下发 maximize 并回读校验窗口管理器是否真的接受了它，未接受则退避重试。
    /// Wayland 下 maximize 是需 compositor 确认的异步请求（tao 还要经 channel 转主线程、
    /// 拆成 3 个 idle 步骤执行），一次被吞即永久失败且无痕迹。成功即停——正常路径只多
    /// 一次原子读；全部失败则保持已设定的还原尺寸，不比不探测更差。
    async fn probe_maximize(window: WebviewWindow) {
        const BACKOFF: [u64; 4] = [0, 200, 400, 800];
        for delay in BACKOFF {
            if delay > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }
            let _ = window.maximize();
            // 请求跨线程投递 + compositor 往返，留一拍再回读
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
            if window.is_maximized().unwrap_or(false) {
                return;
            }
        }
        eprintln!("[window_state] maximize 请求未被窗口管理器接受，保持还原尺寸");
    }

    /// 保存窗口状态（应用退出时）。最大化时不覆盖尺寸/位置（保留上次普通尺寸，
    /// 还原后回到该尺寸），只更新 maximized=true。
    pub fn save(app: &tauri::AppHandle) {
        let Some(path) = state_path() else {
            return;
        };
        let Some(window) = app.get_webview_window("main") else {
            return;
        };
        let Ok(maximized) = window.is_maximized() else {
            return;
        };

        let mut v: Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| Value::Object(Default::default()));
        if v.get("main").is_none() {
            v["main"] = Value::Object(Default::default());
        }
        if let Some(main) = v.get_mut("main") {
            main["maximized"] = Value::Bool(maximized);
            if !maximized {
                if let (Ok(size), Ok(pos)) = (window.inner_size(), window.outer_position()) {
                    main["width"] = Value::from(size.width);
                    main["height"] = Value::from(size.height);
                    main["x"] = Value::from(pos.x);
                    main["y"] = Value::from(pos.y);
                }
            }
        }
        // 写盘失败必须可见：原先的 `let _ =` 把 ENOENT（父目录不存在）静默吞掉，
        // 导致新装系统上「最大化存不下来」完全无从排查。目录现由 state_path →
        // config_dir() 保证存在，剩下的都是真实异常（权限/只读/磁盘满）。
        if let Ok(json) = serde_json::to_string_pretty(&v) {
            if let Err(e) = std::fs::write(&path, json) {
                eprintln!("[window_state] 状态写入失败 {}: {e}", path.display());
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// 回归锁：状态文件必须落在「与其余配置同目录、且写前一定已存在」的位置。
        /// v3.2.5 曾写往 app_config_dir()（已无任何代码创建该目录）并用 `let _ =` 吞掉
        /// ENOENT，导致全新安装上窗口状态永远存不下来——新装系统「最大化后关闭仍不恢复」。
        #[test]
        fn state_path_is_writable_on_fresh_install() {
            let path = state_path().expect("无法定位配置目录");
            let dir = path.parent().expect("状态文件必须有父目录");
            assert_eq!(
                dir,
                crate::settings::config_dir().unwrap().as_path(),
                "必须与其他持久化配置同目录（该目录由 config_dir 自建）"
            );
            assert!(dir.exists(), "写盘前父目录必须存在，否则 write 静默 ENOENT");
        }
    }
}

/// 渲染进程内存信息：RSS + WebKit 自杀阈值（系统内存一半，字节）。
/// WebProcess 的 PPid 是本 UI 进程，据此精确匹配当前实例（多实例并存时互不干扰）。
/// 前端定期采样，RSS 接近阈值时提示用户，防窗口假死。
#[derive(serde::Serialize)]
struct WebProcessMem {
    rss: u64,
    /// WebKit 内存压力机制的自杀阈值（≈ 系统内存一半）
    threshold: u64,
}

#[tauri::command]
fn webprocess_rss() -> WebProcessMem {
    let self_pid = std::process::id();
    let mut max_rss = 0u64;
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().parse::<u32>().is_err() {
                continue;
            }
            let Ok(status) = std::fs::read_to_string(entry.path().join("status")) else {
                continue;
            };
            let mut is_web = false;
            let mut is_child = false;
            let mut rss_kb: Option<u64> = None;
            for line in status.lines() {
                if let Some(v) = line.strip_prefix("Name:") {
                    if v.trim().starts_with("WebKitWebProcess") {
                        is_web = true;
                    }
                } else if let Some(v) = line.strip_prefix("PPid:") {
                    if v.trim().parse::<u32>().ok() == Some(self_pid) {
                        is_child = true;
                    }
                } else if let Some(v) = line.strip_prefix("VmRSS:") {
                    rss_kb = v.trim().trim_end_matches("kB").trim().parse::<u64>().ok();
                }
            }
            if is_web && is_child {
                if let Some(kb) = rss_kb {
                    max_rss = max_rss.max(kb.saturating_mul(1024));
                }
            }
        }
    }
    // WebKit 内存压力阈值 ≈ 系统总内存的一半（WebKitGTK Linux MemoryPressureHandler 默认）
    let threshold = system_memory_kb().saturating_mul(1024) / 2;
    WebProcessMem { rss: max_rss, threshold }
}

/// 读取系统物理内存（MemTotal，单位 kB），失败返回 0。
fn system_memory_kb() -> u64 {
    if let Ok(s) = std::fs::read_to_string("/proc/meminfo") {
        for line in s.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                if let Ok(kb) = rest.trim().trim_end_matches("kB").trim().parse::<u64>() {
                    return kb;
                }
            }
        }
    }
    0
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(AppState {
            conns: Mutex::new(HashMap::new()),
            panes: Mutex::new(HashMap::new()),
            settings: Mutex::new(settings::load()),
            settings_mtime: Mutex::new(
                settings::settings_path().ok().and_then(|p| std::fs::metadata(&p).ok().and_then(|m| m.modified().ok()))
            ),
            profiles: Mutex::new(settings::load_profiles()),
            transfers: Mutex::new(HashMap::new()),
            slot: Mutex::new(None),
        })
        .setup(|app| {
            let window = app.get_webview_window("main").expect("main window");
            // 原生毛玻璃：macOS vibrancy / Windows acrylic；Linux 依赖合成器(KDE 等)对
            // 透明窗口的模糊规则，应用内另有 CSS backdrop 层保证可读性。
            #[cfg(target_os = "macos")]
            let _ = window_vibrancy::apply_vibrancy(
                &window,
                window_vibrancy::NSVisualEffectMaterial::HudWindow,
                None,
                None,
            );
            #[cfg(target_os = "windows")]
            let _ = window_vibrancy::apply_acrylic(&window, Some((18, 18, 18, 120)));

            // 恢复窗口尺寸/位置（自研，尺寸钳制到显示器并集内）；需要最大化时（上次退出为
            // 最大化，或全新安装无历史状态）在窗口几何稳定后探测恢复最大化——避免 WM 把
            // 「最大化前几何」记为恢复序列中间值导致拖拽标题栏塌缩，并回读校验请求是否被接受。
            // 无系统/WM 特定代码，仅 tauri 跨平台 API + serde_json。
            if window_state::restore(&window) {
                window_state::restore_maximize(&window);
            }
            // 关闭请求时保存窗口状态（此时窗口一定可用，能读到 is_maximized 等状态）；
            // 用户取消关闭（prevent_close）也无害——下次启动恢复当前状态。
            let app_handle = app.handle().clone();
            window.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { .. } = event {
                    window_state::save(&app_handle);
                }
            });
            let _ = window;
            // WebProcess 崩溃自动恢复：渲染进程（WebKitWebProcess）承载全部前端 JS，
            // 长时间运行可能被 WebKit 内存压力机制杀死，导致窗口全透明假死。
            // 连接 web-process-terminated 信号（v2_20 起替代已弃用的 web-process-crashed），
            // 崩溃后自动 reload 恢复界面。
            #[cfg(target_os = "linux")]
            {
                use webkit2gtk::WebViewExt;
                if let Some(webview_window) = app.get_webview_window("main") {
                    let webview: &tauri::Webview = webview_window.as_ref();
                    let _ = webview.with_webview(|wv| {
                        let inner = wv.inner(); // webkit2gtk::WebView（引用计数句柄）
                        inner.connect_web_process_terminated(move |wv, _reason| {
                            // 延迟一帧执行 reload，避免在信号分发中直接重入
                            let wv = wv.clone();
                            glib::timeout_add_local_once(
                                std::time::Duration::from_millis(100),
                                move || {
                                    wv.reload();
                                },
                            );
                        });
                    });
                }
            }
            // 预览缓存清理：应用内后台任务（启动 30s 后首次、之后每 30 分钟），
            // 随应用退出自动结束。文件操作走 spawn_blocking，不占用异步执行器。
            tauri::async_runtime::spawn(async {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                loop {
                    let _ = tokio::task::spawn_blocking(cache::sweep).await;
                    tokio::time::sleep(std::time::Duration::from_secs(30 * 60)).await;
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            settings_get,
            settings_set,
            profiles_list,
            profile_save,
            profile_delete,
            session_acquire,
            session_release,
            session_get,
            session_set,
            ssh_connect,
            ssh_disconnect,
            conn_has_transfers,
            pane_open,
            pane_open_local,
            pane_input,
            pane_resize,
            pane_close,
            pane_set_cwd,
            sftp_stat,
            sftp_list,
            sftp_preview,
            image_preview,
            sftp_download,
            sftp_upload,
            transfer_pause,
            transfer_resume,
            transfer_cancel,
            sftp_copy_remote,
            remote_home,
            remote_cwd,
            default_download_dir,
            list_fonts,
            local_list,
            local_home,
            local_cwd,
            local_tab_info,
            read_key_file,
            read_feed_file,
            read_file_base64,
            open_external,
            open_path,
            cache_dir,
            restore_window_size,
            webprocess_rss,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // 兜底：CloseRequested 未触发时（如 SIGTERM/进程被杀前的优雅退出）尝试保存。
            // 窗口可能已销毁，save 内部 get_webview_window 返回 None 时安全跳过。
            if let tauri::RunEvent::ExitRequested { .. } = event {
                window_state::save(app);
            }
        });
}
