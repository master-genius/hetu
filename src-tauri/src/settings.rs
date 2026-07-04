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
    /// "manual" | "ssh_config"
    #[serde(default = "default_source")]
    pub source: String,
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
    pub profiles: Vec<Profile>,
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
            profiles: Vec::new(),
        }
    }
}

fn config_dir() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .ok_or_else(|| Error::msg("无法定位系统配置目录"))?
        .join("superssh");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn settings_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("settings.json"))
}

/// 已知主机指纹存储路径（TOFU 模型）
pub fn known_hosts_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("known_hosts.json"))
}

pub fn load() -> Settings {
    let path = match settings_path() {
        Ok(p) => p,
        Err(_) => return Settings::default(),
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(settings: &Settings) -> Result<()> {
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| Error::msg(format!("序列化设置失败: {e}")))?;
    std::fs::write(settings_path()?, json)?;
    Ok(())
}
