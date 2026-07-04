//! HetuShell Tauri 后端：应用状态、command 注册、窗口毛玻璃效果。

mod error;
mod local;
mod settings;
mod ssh;
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

/// 全局状态：连接注册表 + pane 注册表 + 设置 + 连接项
pub struct AppState {
    conns: Mutex<HashMap<String, Arc<Connection>>>,
    panes: Mutex<HashMap<String, PaneCtl>>,
    settings: Mutex<Settings>,
    /// 连接项存于独立文件 profiles.json，与 settings 分离
    profiles: Mutex<Vec<Profile>>,
}

type State<'a> = tauri::State<'a, AppState>;

async fn get_conn(state: &AppState, conn_id: &str) -> Result<Arc<Connection>> {
    state
        .conns
        .lock()
        .await
        .get(conn_id)
        .cloned()
        .ok_or_else(|| Error::msg("连接不存在或已关闭"))
}

// ---------- 设置 ----------

#[tauri::command]
async fn settings_get(state: State<'_>) -> Result<Settings> {
    Ok(state.settings.lock().await.clone())
}

#[tauri::command]
async fn settings_set(state: State<'_>, settings: Settings) -> Result<()> {
    // 连接项已独立到 profiles.json，settings 不再包含它们，无需特殊保护
    let auto = settings.auto_reconnect;
    settings::save(&settings)?;
    *state.settings.lock().await = settings;
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
    let mut profiles = state.profiles.lock().await;
    profiles.retain(|p| p.id != profile.id);
    profiles.push(profile);
    settings::save_profiles(&profiles)
}

#[tauri::command]
async fn profile_delete(state: State<'_>, id: String) -> Result<()> {
    let mut profiles = state.profiles.lock().await;
    profiles.retain(|p| p.id != id);
    settings::save_profiles(&profiles)
}

// ---------- 会话（独立文件 session.json）----------

#[tauri::command]
fn session_get() -> Vec<SessionTab> {
    settings::load_session()
}

#[tauri::command]
fn session_set(tabs: Vec<SessionTab>) -> Result<()> {
    settings::save_session(&tabs)
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
) -> Result<()> {
    let conn = get_conn(&state, &conn_id).await?;
    let (tx, rx) = mpsc::unbounded_channel();
    // 先登记 PaneCtl 再启动任务：既避免任务在插入前就 emit pane-closed 造成漏删，
    // 也在重连复用同一 pane_id 时先关闭旧 channel 任务，杜绝两个任务并发向同一 pane 推流。
    if let Some(old) = state
        .panes
        .lock()
        .await
        .insert(pane_id.clone(), PaneCtl { tx, conn_id })
    {
        let _ = old.tx.send(PaneCmd::Close);
    }
    if let Err(e) = ssh::pane::open(app, conn, pane_id.clone(), cols, rows, rx).await {
        state.panes.lock().await.remove(&pane_id); // 打开失败，回收占位
        return Err(e);
    }
    Ok(())
}

/// 打开本地终端 pane（不经 SSH，直接本机 PTY 跑用户 shell）。
/// conn_id 记为 "local"，输入/resize/关闭与 SSH pane 共用同一套 command。
#[tauri::command]
async fn pane_open_local(
    app: tauri::AppHandle,
    state: State<'_>,
    pane_id: String,
    cols: u32,
    rows: u32,
) -> Result<()> {
    let (tx, rx) = mpsc::unbounded_channel();
    // 同 pane_open：先登记占位（必要时关闭旧任务），再启动本地 PTY
    if let Some(old) = state.panes.lock().await.insert(
        pane_id.clone(),
        PaneCtl {
            tx,
            conn_id: "local".into(),
        },
    ) {
        let _ = old.tx.send(PaneCmd::Close);
    }
    if let Err(e) = local::open(app, pane_id.clone(), cols, rows, rx) {
        state.panes.lock().await.remove(&pane_id);
        return Err(e);
    }
    Ok(())
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
    ssh::sftp::download(&app, &conn, &remote_path, &local_path, &transfer_id).await
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
    ssh::sftp::upload(&app, &conn, &local_path, &remote_dir, &transfer_id).await
}

#[tauri::command]
async fn remote_home(state: State<'_>, conn_id: String) -> Result<String> {
    let conn = get_conn(&state, &conn_id).await?;
    ssh::sftp::home(&conn).await
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

// ---------- 应用入口 ----------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        // 记住并恢复窗口大小/位置
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .manage(AppState {
            conns: Mutex::new(HashMap::new()),
            panes: Mutex::new(HashMap::new()),
            settings: Mutex::new(settings::load()),
            profiles: Mutex::new(settings::load_profiles()),
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
            let _ = window;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            settings_get,
            settings_set,
            profiles_list,
            profile_save,
            profile_delete,
            session_get,
            session_set,
            ssh_connect,
            ssh_disconnect,
            pane_open,
            pane_open_local,
            pane_input,
            pane_resize,
            pane_close,
            sftp_stat,
            sftp_preview,
            sftp_download,
            sftp_upload,
            remote_home,
            local_list,
            local_home,
            read_key_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
