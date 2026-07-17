//! HetuShell Tauri 后端：应用状态、command 注册、窗口毛玻璃效果。

mod cache;
mod error;
mod local;
mod settings;
mod slot;
mod ssh;
mod sshcfg;

mod agent;

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

/// 用系统默认浏览器打开外部链接（终端里 Ctrl+单击 URL 触发）。
/// 仅放行 http/https，杜绝把任意字符串当命令参数注入到系统 opener。
#[tauri::command]
fn open_external(url: String) -> Result<()> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err(Error::msg("仅支持打开 http/https 链接"));
    }
    #[cfg(target_os = "linux")]
    {
        // xdg-open 是 freedesktop 标准分发器（GNOME/KDE/XFCE 等各桌面均会路由到默认浏览器），
        // 再以 gio open 兜底，覆盖个别缺 xdg-utils 但装了 glib 的环境。
        let attempts: [(&str, &[&str]); 2] = [("xdg-open", &[]), ("gio", &["open"])];
        let mut last = String::from("无可用打开器");
        for (bin, pre) in attempts {
            match std::process::Command::new(bin).args(pre).arg(&url).spawn() {
                Ok(_) => return Ok(()),
                Err(e) => last = format!("{bin}: {e}"),
            }
        }
        return Err(Error::msg(format!("打开链接失败（xdg-open/gio 均不可用）: {last}")));
    }
    #[cfg(not(target_os = "linux"))]
    {
        #[cfg(target_os = "macos")]
        let mut cmd = {
            let mut c = std::process::Command::new("open");
            c.arg(&url);
            c
        };
        #[cfg(target_os = "windows")]
        let mut cmd = {
            // start 是 cmd 内建命令；空标题占位 "" 避免带引号的 URL 被当成窗口标题
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", "start", "", &url]);
            c
        };
        cmd.spawn().map_err(|e| Error::msg(format!("打开链接失败: {e}")))?;
        Ok(())
    }
}

// ---------- 应用入口 ----------

/// 从最大化还原：在后端直接获取屏幕尺寸并设置窗口大小，避免前端 IPC 竞态。
/// 按设置中的 restore_size 百分比计算，兜底 960×550。
#[tauri::command]
async fn restore_window_size(app: tauri::AppHandle) -> Result<()> {
    let window = app.get_webview_window("main").ok_or_else(|| Error::msg("主窗口不存在"))?;
    let settings = settings::load();
    let pct = (settings.restore_size.max(50).min(90)) as f64 / 100.0;

    let (mut lw, mut lh) = (960.0_f64, 550.0_f64);
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
            settings_mtime: Mutex::new(
                settings::settings_path().ok().and_then(|p| std::fs::metadata(&p).ok().and_then(|m| m.modified().ok()))
            ),
            profiles: Mutex::new(settings::load_profiles()),
            transfers: Mutex::new(HashMap::new()),
            slot: Mutex::new(None),
        })
        .manage(agent::AgentManager::new())
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
            open_external,
            restore_window_size,
            agent::agent_spawn,
            agent::agent_send_message,
            agent::agent_abort,
            agent::agent_destroy,
            agent::agent_load_config,
            agent::agent_save_config,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
