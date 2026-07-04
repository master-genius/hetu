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
    /// 新建标签页行为："local"（直接本地终端）| "dialog"（弹出连接选择）
    pub new_tab_mode: String,
    /// 断线自动重连（默认开启）
    pub auto_reconnect: bool,
    pub copy_on_select: bool,
    /// 上传遇同名文件时提示确认；默认 false（直接覆盖）
    pub confirm_overwrite: bool,
    /// 记住最后的会话：下次启动自动重开并连接（默认关闭）
    pub restore_session: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            // 字体随应用内置分发（woff2 内嵌前端），无需系统安装；用户可改为任意本机字体
            font_family: "JetBrains Mono NL".into(),
            cjk_font_family: "Noto Sans SC".into(),
            font_size: 14,
            font_weight: "300".into(),
            // 默认：暗色主题 + 半透明毛玻璃背景
            theme: "dark".into(),
            titlebar_color: None,
            custom_themes: Vec::new(),
            opacity: 0.85,
            blur: true,
            new_tab_mode: "local".into(),
            auto_reconnect: true,
            copy_on_select: true,
            confirm_overwrite: false,
            restore_session: false,
        }
    }
}

/// 会话中一个标签页的可持久化描述（不含任何密钥/密码等机密）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTab {
    /// 是否本地终端
    pub local: bool,
    pub name: String,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub user: Option<String>,
    /// "key" | "password"
    #[serde(default)]
    pub auth: Option<String>,
    #[serde(default)]
    pub key_path: Option<String>,
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

fn write_json<T: Serialize>(path: &PathBuf, value: &T) -> Result<()> {
    let json =
        serde_json::to_string_pretty(value).map_err(|e| Error::msg(format!("序列化失败: {e}")))?;
    // 原子写入
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
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
    let path = match settings_path() {
        Ok(p) => p,
        Err(_) => return Settings::default(),
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return Settings::default(), // 文件不存在等：首次运行，用默认值
    };
    match serde_json::from_str(&content) {
        Ok(s) => s,
        Err(_) => {
            // 文件存在但无法解析（手工误编辑/版本不兼容/损坏）。
            // 绝不静默用默认值覆盖——先把原文件备份，避免下次 save 永久毁掉用户的
            // 连接项与自定义主题；用户可从 .bak 手动恢复。
            let _ = std::fs::rename(&path, path.with_extension("json.bak"));
            Settings::default()
        }
    }
}

pub fn save(settings: &Settings) -> Result<()> {
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| Error::msg(format!("序列化设置失败: {e}")))?;
    std::fs::write(settings_path()?, json)?;
    Ok(())
}
