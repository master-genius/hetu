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

/// 连接配置。密钥与密码为并列属性（可同时保存，0600 保护）；
/// 连接时按 auth 指定的主方式先尝试，失败后自动回退另一种凭据。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub user: String,
    /// "key" | "password"（主认证方式；连接时先尝试此方式，失败后回退另一种）
    #[serde(default = "default_auth")]
    pub auth: String,
    #[serde(default)]
    pub key_path: Option<String>,
    /// 私钥内容（PEM 文本），自存于 profiles.json，不依赖外部文件
    #[serde(default)]
    pub key_data: Option<String>,
    /// 密码（持久化，0600 保护；密钥认证失败时自动回退使用）
    #[serde(default)]
    pub password: Option<String>,
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
fn default_max_image_mb() -> u16 {
    128
}
fn default_cursor_style() -> String {
    "block".to_string()
}
fn default_scrollback() -> u32 {
    12345
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
    /// 背景不透明度 0.0 ~ 1.0（仅作用于终端底色）
    pub opacity: f64,
    /// 磨砂质感：终端背景的表面颗粒层（同色系噪点，不采样任何背景）
    pub frosted: bool,
    /// 磨砂程度 0–100（映射颗粒透明度）
    pub frost_strength: u16,
    /// 覆盖层材质：面板/弹窗/菜单/弹出层的材质（终端主体不参与，见 styles.css）
    #[serde(default)]
    pub overlay_material: OverlayMaterial,
    /// 覆盖层模糊半径（px，0–60），仅 overlay_material = "blur" 时生效
    #[serde(default = "default_overlay_blur")]
    pub overlay_blur: f64,
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
    /// 终端最大回滚行数（范围 5000–100000，默认 12345；非法值由前端回退默认）
    #[serde(default = "default_scrollback")]
    pub scrollback: u32,
    /// WebGL 硬件加速渲染（默认关闭；开启后走 WebGL，软渲染/驱动异常时可回退 Canvas）
    pub webgl: bool,
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
    /// 图片预览单张上限（MB），范围 32–512，默认 128
    #[serde(default = "default_max_image_mb")]
    pub max_image_mb: u16,
    /// 光标样式："block" | "bar"
    #[serde(default = "default_cursor_style")]
    pub cursor_style: String,
    /// 光标颜色（#rrggbb）；None 表示跟随主题
    #[serde(default)]
    pub cursor_color: Option<String>,
    /// 自定义快捷键：动作 → 组合键（仅存与默认不同的覆盖项）
    #[serde(default)]
    pub keybindings: std::collections::HashMap<String, String>,
    /// 是否启用最小对比度提亮（MCR）
    #[serde(default = "default_mcr_enabled")]
    pub mcr_enabled: bool,
    /// MCR 最大值（1.1–2.0，默认 1.6），暗色高不透明度段使用
    #[serde(default = "default_mcr_max")]
    pub mcr_max: f64,
    /// 本地终端 shell：空/"default"/"默认" 表示自动推断；否则为命令名或路径
    #[serde(default)]
    pub shell: String,
}

fn default_mcr_enabled() -> bool {
    true
}
fn default_mcr_max() -> f64 {
    1.6
}
/// 覆盖层材质。用枚举而非 String：非法值在**反序列化阶段**就收敛为 None，
/// 而不是散落在读路径/写路径/前端三处各归一一次（那正是漂移的来源）。
/// 序列化仍是 "none"/"blur"/"liquid" 三个字符串，与前端 union 一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", try_from = "String", into = "String")]
pub enum OverlayMaterial {
    None,
    Blur,
    Liquid,
}

impl Default for OverlayMaterial {
    fn default() -> Self {
        Self::Liquid
    }
}

impl std::fmt::Display for OverlayMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::None => "none",
            Self::Blur => "blur",
            Self::Liquid => "liquid",
        })
    }
}

impl From<OverlayMaterial> for String {
    fn from(m: OverlayMaterial) -> Self {
        m.to_string()
    }
}

impl TryFrom<String> for OverlayMaterial {
    type Error = std::convert::Infallible;
    /// 非法值不报错，一律降级为 None（可读性最高的一档）。
    /// 用 Infallible 是为了让损坏的 settings.json 不至于整份反序列化失败、
    /// 连带丢掉用户的其它设置。
    fn try_from(s: String) -> std::result::Result<Self, Self::Error> {
        Ok(match s.as_str() {
            "blur" => Self::Blur,
            "liquid" => Self::Liquid,
            _ => Self::None,
        })
    }
}

fn default_overlay_blur() -> f64 {
    24.0
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
            frosted: true,
            frost_strength: 20,
            overlay_material: OverlayMaterial::Liquid,
            overlay_blur: 24.0,
            corner_radius: "md".into(),
            tab_bar_fill: true,
            tab_font_family: String::new(),
            tab_font_size: 12,
            show_scrollbar: true,
            scrollback: 12345,
            webgl: false,
            new_tab_mode: "local".into(),
            auto_reconnect: true,
            copy_on_select: true,
            confirm_overwrite: false,
            download_dir: String::new(),
            ask_download_location: false,
            track_remote_cwd: true,
            restore_session: true,
            restore_size: 78,
            max_image_mb: 128,
            cursor_style: "block".into(),
            cursor_color: None,
            keybindings: std::collections::HashMap::new(),
            mcr_enabled: true,
            mcr_max: 1.6,
            shell: String::new(),
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
    /// 本地终端首个 pane 的起始工作目录，恢复时传给 shell 启动
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// hssh park 模式的连接项 ID：本地终端 + hssh 进入的 SSH 连接，恢复时自动重连
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hssh_profile: Option<String>,
    /// hssh 自动恢复连续失败次数；成功连接后归零，超过阈值（12）则放弃重试
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hssh_retries: Option<u32>,
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

/// hsshx 活跃连接的租约目录（跨进程可见）。
///
/// 为什么需要：hsshx 是独立二进制，它的连接不进 Tauri 进程的 AppState.conns，GUI 侧
/// 看不见，也就无法被 `hssh listconn` 列出或被终止。此目录把「GUI 连接」与「CLI 连接」
/// 两个宇宙拼成一张视图。pub 是因为 hsshx bin 只能通过库 crate 访问路径。
pub fn conns_dir() -> Result<PathBuf> {
    let dir = config_dir()?.join("conns");
    std::fs::create_dir_all(&dir)?;
    // 目录 0700（同 bin_dir，不能用 set_owner_only——那是 0600，会抹掉目录 x 权限
    // 导致后续 open 全部失败）；目录里是本人连接的主机/用户清单，不给其他用户读
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(dir)
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
    let Ok(p) = settings_path() else {
        return Settings::default();
    };
    // 读原始 JSON 与 Settings 一次到位：迁移判据要看字段是否**出现过**，
    // 而反序列化后「缺失字段」与「默认值」无法区分，这是唯一需要原始 JSON 的地方。
    let Some(raw) = read_json_value(&p) else {
        return Settings::default();
    };
    let mut s = serde_json::from_value::<Settings>(raw.clone()).unwrap_or_default();

    if migrate(&mut s, &raw) {
        // 迁移是一次性动作（升级首启），结果必须落盘：否则旧字段永远留在文件里，
        // 每次 load 重新生效，把用户此后显式选择的材质压回去。
        // 锁内重新读原始 JSON 再判一次——闭包拿到的是最新文件，若已被别的进程
        // 升级过则 migrate 返回 false，不会覆盖。
        let _ = update_locked::<Settings, _>(&p, |v| {
            if let Some(locked) = read_json_value(&p) {
                migrate(v, &locked);
            }
            Ok(())
        });
    }
    // 材质非法值已在反序列化阶段由 OverlayMaterial 的 TryFrom 收敛，这里只剩
    // 半径需要钳制（它不是枚举，无法在类型层表达区间）。
    s.overlay_blur = clamp_overlay_blur(s.overlay_blur);
    s
}

/// 读 settings.json 为原始 JSON。损坏时按既有策略备份为 .bak 并返回 None。
fn read_json_value(path: &std::path::Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text) {
        Ok(v) => Some(v),
        Err(_) => {
            let _ = std::fs::rename(path, path.with_extension("json.bak"));
            None
        }
    }
}

/// 旧配置迁移。返回 true 表示做了改动，调用方据此落盘。
fn migrate(s: &mut Settings, raw: &serde_json::Value) -> bool {
    // 用 `|` 而非 `||`：两个迁移互不依赖，都必须执行
    migrate_legacy_blur(s, raw) | migrate_font_weight(s)
}

/// 3.4.0 → 3.5.0：终端侧「背景虚化」整条链路删除（终端背景是窗口外桌面，
/// backdrop-filter 采样不到，开关从未生效），改为覆盖层材质 overlay_material。
///
/// 判据全部来自原始 JSON，规则只有两条：
/// - **已有新字段 → 视为已迁移，绝不覆写**。否则只要文件里同时残留旧字段
///   （手改、从备份还原、旧版本文件被复制回来），用户此后显式选择的材质就会被
///   每次 load 压回旧映射值，且界面上看不出任何异常。
/// - 旧「毛玻璃虚化」开关即新「毛玻璃」材质；缺 blur 键时退到 bgBlur
///   （两者都是「虚化」开关），都没有才沿用旧默认「开」。旧 blurAmount 钳到 [0,60]
///   （它标定的是终端级模糊，直接拿来会把覆盖层糊成纯色）。
fn migrate_legacy_blur(s: &mut Settings, raw: &serde_json::Value) -> bool {
    if raw.get("overlayMaterial").is_some() || raw.get("overlayBlur").is_some() {
        return false;
    }
    let blur = raw.get("blur").and_then(|v| v.as_bool());
    let bg_blur = raw.get("bgBlur").and_then(|v| v.as_bool());
    let amount = raw.get("blurAmount").and_then(|v| v.as_f64());
    if blur.is_none() && bg_blur.is_none() && amount.is_none() {
        return false; // 全新配置，无旧字段可迁
    }
    s.overlay_material = if blur.or(bg_blur).unwrap_or(true) {
        OverlayMaterial::Blur
    } else {
        OverlayMaterial::None
    };
    if let Some(v) = amount.filter(|v| v.is_finite()) {
        s.overlay_blur = clamp_overlay_blur(v);
    }
    true
}

/// 覆盖层模糊半径合法范围 [0, 60]，非法回退默认 24
pub(crate) fn clamp_overlay_blur(v: f64) -> f64 {
    if !v.is_finite() {
        return default_overlay_blur();
    }
    v.clamp(0.0, 60.0)
}

/// 兼容旧配置：1.0.0 以 font_weight="300" 存储全局字重，据此唯一识别旧配置
/// （新版本一律存 "normal"，故不会误伤新用户主动选择的字体）。仅当旧配置的等宽/CJK
/// 仍是 1.0.0 默认（未定制）时，升级到「字重并入名字」的新 Light 默认；随后归一化字重。
/// 返回 true 表示做了改动。
fn migrate_font_weight(s: &mut Settings) -> bool {
    if s.font_weight != "300" {
        return false; // 新配置（weight=normal）不动，避免反复覆盖用户选择
    }
    if s.font_family == "JetBrains Mono NL" && s.cjk_font_family == "Noto Sans SC" {
        let d = Settings::default();
        s.font_family = d.font_family;
        s.cjk_font_family = d.cjk_font_family;
    }
    // 终端不再全局施加字重，字重由字体名承载；归一化后此分支不再匹配
    s.font_weight = "normal".into();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 旧配置迁移：blur=true → overlay_material="blur"，blurAmount 钳到 [0,60]
    #[test]
    fn clamp_overlay_blur_bounds() {
        assert_eq!(clamp_overlay_blur(82.0), 60.0);
        assert_eq!(clamp_overlay_blur(24.0), 24.0);
        assert_eq!(clamp_overlay_blur(-5.0), 0.0);
        assert_eq!(clamp_overlay_blur(f64::NAN), default_overlay_blur());
        assert_eq!(clamp_overlay_blur(f64::INFINITY), default_overlay_blur());
    }

    /// 默认值：新装用户拿到液态玻璃 + 24px
    #[test]
    fn defaults_use_liquid_material() {
        let d = Settings::default();
        assert_eq!(d.overlay_material, OverlayMaterial::Liquid);
        assert_eq!(d.overlay_blur, 24.0);
        // 终端侧只保留不透明度 + 磨砂
        assert_eq!(d.opacity, 0.85);
        assert!(d.frosted);
    }

    /// 新结构反序列化：缺失 overlay_* 时回落到默认（serde default）
    #[test]
    fn missing_overlay_fields_fall_back_to_default() {
        let json = r#"{"fontFamily":"X","cjkFontFamily":"Y","fontSize":16,
            "fontWeight":"normal","theme":"dark","customThemes":[],
            "opacity":0.8,"frosted":true,"frostStrength":30,
            "cornerRadius":"md","tabBarFill":true,"tabFontFamily":"",
            "tabFontSize":12,"showScrollbar":true,"webgl":false,
            "newTabMode":"local","autoReconnect":true,"copyOnSelect":true,
            "confirmOverwrite":false,"downloadDir":"","askDownloadLocation":false,
            "trackRemoteCwd":true,"restoreSession":true}"#;
        let s: Settings = serde_json::from_str(json).expect("应能反序列化缺失新字段的配置");
        assert_eq!(s.overlay_material, OverlayMaterial::Liquid);
        assert_eq!(s.overlay_blur, 24.0);
    }

    /// 迁移决策（纯函数，无环境依赖）：三条规则一次覆盖
    #[test]
    fn legacy_blur_migration_rules() {
        // blur=true → "blur"，blurAmount 超上限被钳住
        let mut s = Settings::default();
        assert!(migrate_legacy_blur(&mut s, &json(r#"{"blur":true,"blurAmount":82.0}"#)));
        assert_eq!(s.overlay_material, OverlayMaterial::Blur);
        assert_eq!(s.overlay_blur, 60.0);

        // blur=false（用户显式关掉毛玻璃）→ "none"，合法的 blurAmount 保留
        let mut s = Settings::default();
        assert!(migrate_legacy_blur(&mut s, &json(r#"{"blur":false,"blurAmount":30.0}"#)));
        assert_eq!(s.overlay_material, OverlayMaterial::None);
        assert_eq!(s.overlay_blur, 30.0);

        // blurAmount 缺失 / 非法（类型错、null）→ 保留默认
        for raw in [
            r#"{"blur":true}"#,
            r#"{"blur":true,"blurAmount":"oops"}"#,
            r#"{"blur":true,"blurAmount":null}"#,
        ] {
            let mut s = Settings::default();
            assert!(migrate_legacy_blur(&mut s, &json(raw)));
            assert_eq!(s.overlay_blur, default_overlay_blur(), "输入={raw}");
        }
    }

    /// 缺 blur 键时以 bgBlur 为准：不能把 bgBlur 只当「存在标记」而丢掉它的布尔值。
    /// 否则这份配置里唯一的反证信号被忽略，材质会被静默置成 blur。
    #[test]
    fn legacy_bg_blur_decides_when_blur_absent() {
        let mut s = Settings::default();
        assert!(migrate_legacy_blur(&mut s, &json(r#"{"blurAmount":30.0,"bgBlur":false}"#)));
        assert_eq!(s.overlay_material, OverlayMaterial::None, "bgBlur=false 是唯一的反证信号，不应被忽略");

        // 两个开关都没有 → 无法判断用户意图，沿用旧默认「开」
        let mut s = Settings::default();
        assert!(migrate_legacy_blur(&mut s, &json(r#"{"blurAmount":30.0}"#)));
        assert_eq!(s.overlay_material, OverlayMaterial::Blur);
    }

    /// 【核心回归】旧字段与新字段并存时，新字段必须胜出。
    /// 路径真实可达：从备份还原、手改文件补回旧字段、旧版本文件被复制回来。
    /// 若判据只看「有没有旧字段」，用户此后显式选择的材质会被每次 load 压回旧映射值，
    /// 且界面上看不出任何异常 —— 这是本重构最隐蔽的一条缺陷。
    #[test]
    fn new_field_always_beats_legacy_fields() {
        for raw in [
            r#"{"blur":true,"blurAmount":82.0,"overlayMaterial":"liquid","overlayBlur":12.0}"#,
            r#"{"bgBlur":false,"overlayMaterial":"none"}"#,
            r#"{"blur":false,"overlayBlur":40.0}"#,
        ] {
            let mut s = Settings::default();
            s.overlay_material = OverlayMaterial::Liquid.into();
            s.overlay_blur = 12.0;
            assert!(!migrate_legacy_blur(&mut s, &json(raw)), "含新字段的配置不得被旧字段覆写：{raw}");
            assert_eq!(s.overlay_material, OverlayMaterial::Liquid, "新字段值必须原样保留：{raw}");
            assert_eq!(s.overlay_blur, 12.0, "新字段值必须原样保留：{raw}");
        }
    }

    /// 全新配置（无旧字段）不触发迁移，也就不产生写盘
    #[test]
    fn fresh_config_needs_no_migration() {
        let mut s = Settings::default();
        assert!(!migrate_legacy_blur(&mut s, &json(r#"{"opacity":0.8}"#)));
    }

    /// 端到端：旧文件 → load() 迁移并**落盘**（D-1 的关键：不落盘则旧字段永不消失、
    /// 每次 load 重放迁移，会把用户此后显式设置的新材质一路压回去）。
    ///
    /// 本用例改 XDG_CONFIG_HOME（进程级全局态），与并行用例有竞争风险；
    /// 故模块内其余用例一律是纯函数/serde，不读配置目录。
    #[test]
    fn legacy_file_migrates_and_persists() {
        let tmp = std::env::temp_dir().join(format!("hetu-mig-{}", std::process::id()));
        let dir = tmp.join("hetushell");
        std::fs::create_dir_all(&dir).expect("建临时配置目录");
        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{"opacity":0.83,"blur":true,"blurAmount":82.0,
            "frosted":true,"frostStrength":64}"#).expect("写旧配置");

        let prev = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", &tmp);
        let s = load();
        let after = std::fs::read_to_string(&path).unwrap_or_default();
        let s2 = load();
        match prev {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }

        // 终端侧观感原样保留（升级不得动用户已有设置）
        assert_eq!(s.opacity, 0.83);
        assert!(s.frosted);
        assert_eq!(s.frost_strength, 64);
        // 迁移结果落在合法域
        assert_eq!(s.overlay_material, OverlayMaterial::Blur);
        assert_eq!(s.overlay_blur, 60.0);
        // 关键：旧字段已被写盘消除，第二次 load 读到的是新结构。
        // 注意不能用 contains("\"blur\"") 判断 —— 新字段 overlayBlur 也含该子串。
        // 按 JSON 键存在性判断才准确。
        let after_json: serde_json::Value = serde_json::from_str(&after).expect("落盘内容应是合法 JSON");
        for legacy_key in ["blur", "blurAmount", "bgBlur"] {
            assert!(after_json.get(legacy_key).is_none(), "迁移后文件不应再含旧字段 {legacy_key}，实际={after}");
        }
        assert_eq!(after_json.get("overlayMaterial").and_then(|v| v.as_str()), Some("blur"),
                   "迁移后文件应含新字段，实际={after}");
        assert_eq!(s2.overlay_material, s.overlay_material);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 材质类型层归一：非法值在**反序列化阶段**收敛为 None，且不连带丢失其它设置。
    /// 这是把归一从「三处各写一遍」收敛到「类型本身」的验证点。
    #[test]
    fn overlay_material_rejects_unknown_values() {
        // 合法值原样通过，且三个字符串与前端 union 一致
        for (raw, want) in [
            (r#""blur""#, OverlayMaterial::Blur),
            (r#""liquid""#, OverlayMaterial::Liquid),
            (r#""none""#, OverlayMaterial::None),
        ] {
            let m: OverlayMaterial = serde_json::from_str(raw).expect("合法值应能反序列化");
            assert_eq!(m, want, "输入={raw}");
        }
        // 手改配置 / 大小写差异 / 空串 / 多余空格，一律降级为 None
        for bad in [r#""glass""#, r#""Blur""#, r#""LIQUID""#, r#""""#, r#""liquid ""#] {
            let m: OverlayMaterial = serde_json::from_str(bad).expect("非法值不应导致反序列化失败");
            assert_eq!(m, OverlayMaterial::None, "输入={bad}");
        }
        // 序列化回字符串（决定 settings.json 里存什么）
        assert_eq!(serde_json::to_string(&OverlayMaterial::Liquid).unwrap(), r#""liquid""#);
        assert_eq!(serde_json::to_string(&OverlayMaterial::None).unwrap(), r#""none""#);
    }

    /// 非法材质不得拖垮整份配置的反序列化：用户其它设置必须保住。
    /// 这是用 Infallible + try_from（而非让 serde 报错）的全部理由。
    #[test]
    fn bad_material_does_not_drop_other_settings() {
        let raw = r#"{"opacity":0.71,"frostStrength":42,"overlayMaterial":"glass"}"#;
        let s: Settings = serde_json::from_str(raw).expect("非法材质不应导致整份配置解析失败");
        assert_eq!(s.opacity, 0.71);
        assert_eq!(s.frost_strength, 42);
        assert_eq!(s.overlay_material, OverlayMaterial::None);
    }

    /// 测试辅助：JSON 字面量 → Value（迁移判据看的是原始 JSON，不是反序列化结果）
    fn json(raw: &str) -> serde_json::Value {
        serde_json::from_str(raw).expect("测试 JSON 应合法")
    }
}
