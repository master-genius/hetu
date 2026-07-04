//! 应用设置与连接配置的持久化（JSON 文件，位于系统配置目录）。

use serde::{Deserialize, Serialize};
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
    /// 圆角级别："square" | "xs" | "sm" | "md" | "lg"
    pub corner_radius: String,
    /// 标签页平分横向宽度
    pub tab_bar_fill: bool,
    /// 标签页字体（空 = 同主字体）
    pub tab_font_family: String,
    /// 标签页字号（0 = 自动，终端字号 - 2）
    pub tab_font_size: u16,
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
    /// 记住最后的会话：下次启动自动重开并连接（默认关闭）
    pub restore_session: bool,
    /// 自定义快捷键：动作 → 组合键（仅存与默认不同的覆盖项）
    #[serde(default)]
    pub keybindings: std::collections::HashMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            // 字重并入字体名（如 "…Light"），不再全局强制字重；内置等宽/CJK 默认均用 Light。
            font_family: "JetBrains Mono NL Light".into(),
            cjk_font_family: "Noto Sans CJK SC Light".into(),
            font_size: 14,
            // 保留字段仅为兼容；终端一律用 normal，字重由所选字体名承载
            font_weight: "normal".into(),
            // 默认：暗色主题 + 半透明毛玻璃背景
            theme: "dark".into(),
            titlebar_color: None,
            custom_themes: Vec::new(),
            opacity: 0.85,
            blur: true,
            blur_amount: 30.0,
            corner_radius: "md".into(),
            tab_bar_fill: true,
            tab_font_family: String::new(),
            tab_font_size: 0,
            new_tab_mode: "local".into(),
            auto_reconnect: true,
            copy_on_select: true,
            confirm_overwrite: false,
            download_dir: String::new(),
            ask_download_location: false,
            track_remote_cwd: true,
            restore_session: false,
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
}

fn config_dir() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .ok_or_else(|| Error::msg("无法定位系统配置目录"))?
        .join("hetushell");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn settings_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("settings.json"))
}

fn profiles_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("profiles.json"))
}

fn session_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("session.json"))
}

/// 已知主机指纹存储路径（TOFU 模型）
pub fn known_hosts_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("known_hosts.json"))
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

// ---------- 连接项（独立文件 profiles.json）----------

pub fn load_profiles() -> Vec<Profile> {
    match profiles_path() {
        Ok(p) => read_json_or_default(&p),
        Err(_) => Vec::new(),
    }
}

pub fn save_profiles(profiles: &[Profile]) -> Result<()> {
    write_json(&profiles_path()?, &profiles.to_vec())
}

// ---------- 会话（独立文件 session.json）----------

pub fn load_session() -> Vec<SessionTab> {
    match session_path() {
        Ok(p) => read_json_or_default(&p),
        Err(_) => Vec::new(),
    }
}

pub fn save_session(tabs: &[SessionTab]) -> Result<()> {
    write_json(&session_path()?, &tabs.to_vec())
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

/// 兼容旧配置：1.0.0 的字体默认三元组（等宽/CJK 未定制、且用全局字重 300）
/// 升级到「字重并入名字」的新 Light 默认，使旧用户也能得到内置 Light 字体并在名字上体现。
fn migrate(s: &mut Settings) {
    if s.font_family == "JetBrains Mono NL"
        && s.cjk_font_family == "Noto Sans SC"
        && (s.font_weight == "300" || s.font_weight == "normal")
    {
        let d = Settings::default();
        s.font_family = d.font_family;
        s.cjk_font_family = d.cjk_font_family;
    }
    // 终端不再全局施加字重，统一归一化为 normal（字重由字体名承载）
    s.font_weight = "normal".into();
}

pub fn save(settings: &Settings) -> Result<()> {
    // 与 profiles/session 一致：原子写入 + 0600，避免半写导致 load() 判损后重置全部偏好
    write_json(&settings_path()?, settings)
}
