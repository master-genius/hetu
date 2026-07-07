//! 应用设置与连接配置的持久化（JSON 文件，位于系统配置目录）。

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::path::PathBuf;

use crate::error::{Error, Result};

/// 自定义主题：基于某个内置主题（base），覆盖部分颜色。
/// colors 直接透传给 xterm.js 的 ITheme，后端不关心具体键。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeDef {
    pub id: String,
    pub name: String,
    /// "dark" | "light"，决定 UI 底色基调
    pub base: String,
    pub colors: serde_json::Map<String, serde_json::Value>,
}

/// 连接配置。密码永不持久化，仅在连接时由前端传入。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub user: String,
    /// "key" | "password"
    #[serde(default = "default_auth")]
    pub auth: String,
    #[serde(default)]
    pub key_path: Option<String>,
    /// 私钥内容（PEM 文本），自存于 profiles.json，不依赖外部文件
    #[serde(default)]
    pub key_data: Option<String>,
    /// "manual" | "ssh_config"
    #[serde(default = "default_source")]
    pub source: String,
    /// 备注/标记，便于查找与展示
    #[serde(default)]
    pub note: Option<String>,
    /// 保活间隔（秒）
    #[serde(default)]
    pub keepalive: Option<u64>,
    /// 连接超时（秒）
    #[serde(default)]
    pub timeout: Option<u64>,
}

fn default_port() -> u16 {
    22
}
fn default_auth() -> String {
    "key".into()
}
fn default_source() -> String {
    "manual".into()
}
fn default_restore_size() -> u16 {
    78
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub font_family: String,
    pub cjk_font_family: String,
    pub font_size: u16,
    /// CSS font-weight，Light = "300"
    pub font_weight: String,
    /// 当前主题 id（内置主题或自定义主题 id）
    pub theme: String,
    /// 标题栏颜色；None 表示跟随主题背景色
    pub titlebar_color: Option<String>,
    pub custom_themes: Vec<ThemeDef>,
    /// 背景不透明度 0.0 ~ 1.0
    pub opacity: f64,
    /// 毛玻璃虚化
    pub blur: bool,
    /// 毛玻璃模糊程度（px）
    pub blur_amount: f64,
    /// 背景虚化：终端整体背景虚化/光晕/玻璃效果总开关（默认开）。
    /// 关闭后透明度仍作用于底色，复杂背景效果停用，弹窗模糊不受影响。
    pub bg_blur: bool,
    /// 磨砂质感：独立于毛玻璃的表面颗粒层（同色系噪点）
    pub frosted: bool,
    /// 磨砂程度 0–100（映射颗粒透明度）
    pub frost_strength: u16,
    /// 圆角级别："square" | "xs" | "sm" | "md" | "lg"
    pub corner_radius: String,
    /// 标签页平分横向宽度
    pub tab_bar_fill: bool,
    /// 标签页字体（空 = 同主字体）
    pub tab_font_family: String,
    /// 标签页字号（0/不填 = 默认 12，不跟随终端字号）
    pub tab_font_size: u16,
    /// 显示终端滚动条（默认显示）
    pub show_scrollbar: bool,
    /// 新建标签页行为："local"（直接本地终端）| "dialog"（弹出连接选择）
    pub new_tab_mode: String,
    /// 断线自动重连（默认开启）
    pub auto_reconnect: bool,
    pub copy_on_select: bool,
    /// 上传遇同名文件时提示确认；默认 false（直接覆盖）
    pub confirm_overwrite: bool,
    /// 默认下载目录（空 = 自动，系统 Downloads）
    pub download_dir: String,
    /// 每次下载都询问保存位置
    pub ask_download_location: bool,
    /// 追踪远程工作目录（连接时注入隐形 PID 标记，用 /proc 读实时 cwd）
    pub track_remote_cwd: bool,
    /// 记住最后的会话：下次启动自动重开并连接（默认开启，多实例下按 slot 分片恢复）
    pub restore_session: bool,
    /// 窗口还原尺寸（屏幕占比百分比 35-90，默认 78）
    #[serde(default = "default_restore_size")]
    pub restore_size: u16,
    /// 自定义快捷键：动作 → 组合键（仅存与默认不同的覆盖项）
    #[serde(default)]
    pub keybindings: std::collections::HashMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            // 字重并入字体名，不再全局强制字重；内置等宽/CJK 默认均用 Regular（常规），
            // 配合各自内置的真 Bold 字形，避免 xterm(canvas) 渲染下 Light 过细、CJK 伪粗。
            font_family: "JetBrains Mono NL".into(),
            cjk_font_family: "Noto Sans CJK SC".into(),
            font_size: 16,
            // 保留字段仅为兼容；终端一律用 normal，字重由所选字体名承载
            font_weight: "normal".into(),
            // 默认：暗色主题 + 半透明毛玻璃背景
            theme: "dark".into(),
            titlebar_color: None,
            custom_themes: Vec::new(),
            opacity: 0.85,
            blur: true,
            blur_amount: 30.0,
            bg_blur: true,
            frosted: true,
            frost_strength: 20,
            corner_radius: "md".into(),
            tab_bar_fill: true,
            tab_font_family: String::new(),
            tab_font_size: 12,
            show_scrollbar: true,
            new_tab_mode: "local".into(),
            auto_reconnect: true,
            copy_on_select: true,
            confirm_overwrite: false,
            download_dir: String::new(),
            ask_download_location: false,
            track_remote_cwd: true,
            restore_session: true,
            restore_size: 78,
            keybindings: std::collections::HashMap::new(),
        }
    }
}

/// 会话中一个标签页的可持久化描述（不含任何机密）。
/// 远程连接只记录来源连接项 id，恢复时据此从 profiles.json 取回完整参数（含密钥）；
/// 临时/手输、或未保存为连接项的连接 profile_id 为 None，不予恢复。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTab {
    /// 是否本地终端
    pub local: bool,
    pub name: String,
    #[serde(default)]
    pub profile_id: Option<String>,
    /// 分屏结构快照（结构+比例的递归 JSON）。后端不解释其形状，原样透传给前端重放；
    /// 缺省 = 单 pane。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<serde_json::Value>,
}

pub(crate) fn config_dir() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .ok_or_else(|| Error::msg("无法定位系统配置目录"))?
        .join("hetushell");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub(crate) fn settings_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("settings.json"))
}

pub(crate) fn profiles_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("profiles.json"))
}

/// 多实例分片：每个进程的 session 存于 session-<slot>.json，互不覆盖
pub(crate) fn session_path(slot: usize) -> Result<PathBuf> {
    Ok(config_dir()?.join(format!("session-{}.json", slot)))
}

/// 旧版扁平 session.json 路径（仅迁移到分片格式时使用）
fn legacy_session_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("session.json"))
}

/// 已知主机指纹存储路径（TOFU 模型）
pub fn known_hosts_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("known_hosts.json"))
}

/// 随应用注入本地 shell 的辅助命令目录（config/hetushell/bin），如 hssh。
/// 仅本地终端会把它追加到 PATH 末尾；远程连接不注入。目录置 0700，避免他人写入植入可执行。
pub fn bin_dir() -> Result<PathBuf> {
    let dir = config_dir()?.join("bin");
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(dir)
}

/// 读取 JSON 文件为某类型；文件缺失或损坏返回默认值（损坏时备份原文件）。
fn read_json_or_default<T: serde::de::DeserializeOwned + Default>(path: &PathBuf) -> T {
    let content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return T::default(),
    };
    match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => {
            let _ = std::fs::rename(path, path.with_extension("json.bak"));
            T::default()
        }
    }
}

/// 仅属主可读写（0600）。私钥等机密以文件形式存于配置目录，用权限而非加密保护。
#[cfg(unix)]
pub fn set_owner_only(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}
#[cfg(not(unix))]
pub fn set_owner_only(_path: &std::path::Path) {}

fn write_json<T: Serialize>(path: &PathBuf, value: &T) -> Result<()> {
    let json =
        serde_json::to_string_pretty(value).map_err(|e| Error::msg(format!("序列化失败: {e}")))?;
    // 原子写入：先写临时文件并设 0600，再 rename，避免半写损坏与短暂的宽松权限窗口
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    set_owner_only(&tmp);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// 跨进程文件锁保护的读-改-写事务：flock 独占锁 → 读 → 改 → 写 → 释放。
/// 防多进程并发修改共享配置（settings/profiles/known_hosts）导致 lost update。
/// 原子写入已防"半写"，但 read-modify-write 仍需文件锁串行化，否则后写覆盖先写。
pub(crate) fn update_locked<T, F>(path: &PathBuf, f: F) -> Result<()>
where
    T: DeserializeOwned + Default + Serialize,
    F: FnOnce(&mut T) -> Result<()>,
{
    let lock_path = path.with_extension("json.lock");
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&lock_path)?;
    fs2::FileExt::lock_exclusive(&lock_file)?;
    let result = (|| {
        let mut v: T = read_json_or_default(path);
        f(&mut v)?;
        write_json(path, &v)
    })();
    fs2::FileExt::unlock(&lock_file)?;
    result
}

/// 同 update_locked，但返回修改后的值，供调用方同步内存缓存。
pub(crate) fn update_locked_return<T, F>(path: &PathBuf, f: F) -> Result<T>
where
    T: DeserializeOwned + Default + Serialize,
    F: FnOnce(&mut T) -> Result<()>,
{
    let lock_path = path.with_extension("json.lock");
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&lock_path)?;
    fs2::FileExt::lock_exclusive(&lock_file)?;
    let result = (|| {
        let mut v: T = read_json_or_default(path);
        f(&mut v)?;
        write_json(path, &v)?;
        Ok(v)
    })();
    fs2::FileExt::unlock(&lock_file)?;
    result
}

// ---------- 连接项（独立文件 profiles.json）----------

pub fn load_profiles() -> Vec<Profile> {
    match profiles_path() {
        Ok(p) => read_json_or_default(&p),
        Err(_) => Vec::new(),
    }
}

// ---------- 会话（按 slot 分片：session-<slot>.json）----------

pub fn load_session(slot: usize) -> Vec<SessionTab> {
    match session_path(slot) {
        Ok(p) => {
            // 首次升级到多实例分片：slot 0 且 session-0.json 不存在时，
            // 从旧版扁平 session.json 迁移内容，原文件改名 .bak 保留
            if slot == 0 && !p.exists() {
                if let Ok(legacy) = legacy_session_path() {
                    if legacy.exists() {
                        if let Ok(content) = std::fs::read_to_string(&legacy) {
                            if let Ok(tabs) = serde_json::from_str::<Vec<SessionTab>>(&content) {
                                let _ = write_json(&p, &tabs);
                                let _ = std::fs::rename(&legacy, legacy.with_extension("json.bak"));
                                return tabs;
                            }
                        }
                    }
                }
            }
            read_json_or_default(&p)
        }
        Err(_) => Vec::new(),
    }
}

pub fn save_session(slot: usize, tabs: &[SessionTab]) -> Result<()> {
    write_json(&session_path(slot)?, &tabs.to_vec())
}

pub fn load() -> Settings {
    // 与 profiles/session 共用损坏恢复策略（解析失败备份为 .bak 后回默认值）
    let mut s = match settings_path() {
        Ok(p) => read_json_or_default(&p),
        Err(_) => Settings::default(),
    };
    migrate(&mut s);
    s
}

/// 兼容旧配置：1.0.0 以 font_weight="300" 存储全局字重，据此唯一识别旧配置
/// （新版本一律存 "normal"，故不会误伤新用户主动选择的字体）。仅当旧配置的等宽/CJK
/// 仍是 1.0.0 默认（未定制）时，升级到「字重并入名字」的新 Light 默认；随后归一化字重。
fn migrate(s: &mut Settings) {
    if s.font_weight != "300" {
        return; // 新配置（weight=normal）不动，避免反复覆盖用户选择
    }
    if s.font_family == "JetBrains Mono NL" && s.cjk_font_family == "Noto Sans SC" {
        let d = Settings::default();
        s.font_family = d.font_family;
        s.cjk_font_family = d.cjk_font_family;
    }
    // 终端不再全局施加字重，字重由字体名承载；归一化后此分支不再匹配
    s.font_weight = "normal".into();
}
