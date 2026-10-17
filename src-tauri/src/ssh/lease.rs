//! hsshx 活跃连接租约：跨进程可见的「谁还活着」登记表。
//!
//! 为什么需要：hsshx 是独立二进制，它的连接不进 Tauri 进程的 AppState.conns，
//! GUI 侧完全看不见 → `hssh listconn` 只能看到半个宇宙，也杀不到 CLI 那半边。
//!
//! 存活判定复用 `slot.rs` 已验证的同一条内核语义：租约持有 `<id>.lock` 的 flock 到进程
//! 退出，崩溃/`kill -9` 由内核自动释放，因此不需要心跳、也不会残留脏账。
//!
//! 顺序契约（不可调换）：**先抢锁，再写 sidecar**。反过来会留下「json 已存在但还没拿锁」
//! 的窗口，扫描方 try_lock 成功即误判为僵尸并删除，把刚启动的连接抹掉。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::settings;

/// 一条租约的内容。绝不含凭据（密码/私钥/口令），只有定位与归属所需的字段。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lease {
    /// 租约 id（uuid）。与连接 id 同源，killconn 用它定位
    pub id: String,
    /// 持有进程 pid。终止 CLI 连接即对该 pid 发信号
    pub pid: u32,
    /// "gui" | "interactive" | "exec"（仅 `hsshx --track`）| "get" | "put" | "cp"
    pub kind: String,
    pub profile: String,
    pub host: String,
    #[serde(default)]
    pub port: u16,
    pub user: String,
    /// 发起方标记（`--by`）。None = 人在终端手敲
    #[serde(default)]
    pub by: Option<String>,
    /// 持有这条租约的进程所在 pane（来自注入的 `$HSSH_PANE`）。
    /// 「本 pane 发起 → 免确认直杀」这条判据要用它；`serde(default)` 让 3.3.3 写下的
    /// 旧租约（无此字段）照样能读出来，只是退化成「必须 --by + --force」。
    #[serde(default)]
    pub pane: Option<String>,
    /// 操作摘要，如 `--put ./a → /srv/in`。已经过 sanitize 净化
    #[serde(default)]
    pub detail: String,
    pub started_unix: u64,
}

/// 标记归一的唯一实现点：None/空串都按「user」。
/// 展示、授权判定、命令行 `--by` 的比对全部走这里——否则「省略 --by」和「目标是用户发起」
/// 会被当成两件事，规则就写歪了。
pub fn label_of(tag: Option<&str>) -> &str {
    match tag {
        Some(t) if !t.is_empty() => t,
        _ => "user",
    }
}

/// 发起方标记的规范化显示：None/空串都按「user」呈现（用户在 GUI 或终端直接发起）。
pub fn by_label(by: &Option<String>) -> &str {
    label_of(by.as_deref())
}

/// 授权判定的输入。`by/origin_pane/claim` 都是 `Option<&str>` 且语义相近，用位置参数
/// 一旦错位编译器不报错（review 时就发生过同类错），故收敛成具名结构。
pub struct KillCtx<'a> {
    /// 目标记录的发起方标记（GUI：`ConnParams.by`；CLI：`Lease.by`）
    pub by: &'a Option<String>,
    /// 目标记录的发起 pane（GUI：`origin_pane`；CLI：`Lease.pane`）
    pub origin_pane: Option<&'a str>,
    /// 当前挂着该连接的 pane 列表。GUI 才有（`ConnInfo.panes`）；CLI 传空切片。
    /// 面板点击建的连接没有「发起 pane」（它在新建的 tab 里），只能靠这条判据认领。
    pub panes: &'a [String],
    /// 发起本次请求的 pane
    pub req_pane: Option<&'a str>,
    /// 调用方声明的发起标记（`--by`）；None 即按 user 归一
    pub claim: Option<&'a str>,
    /// 调用方是否显式按下 `--force`
    pub force: bool,
}

/// 终止授权判定（唯一实现点：listconn 的「可否终止」与 killconn 的执行共用同一规则）。
///
/// 规则（开发者拍板）两条，顺序即优先级：
/// ① **本 pane 的** → 直接可终止，不需要任何确认。「本 pane」取两种归属之一：
///    - 目标记录的发起 pane 就是请求方（hssh 在该 pane 发起，park 后仍成立）；
///    - 该连接当前正挂在这个 pane 里（面板点击建的连接没有发起 pane，靠这条认领）。
/// ② 其余一律 `--force` **且** `--by` 与目标记录的标记一致——`--by` 省略即按「user」比对，
///    所以单给 `--force` 就能收到用户发起的连接，收 AI/脚本的则要连标记一起给。
///
/// 刻意不做 tab 作用域，也不再给「打了非 user 标记就免确认」这条捷径：前者要把 tab 拓扑
/// 搬进判定链路（跨进程传递，本身是不稳定源），后者等于用可猜测的 tag 换掉显式确认。
/// 破坏性动作的门槛只留「这条终端是不是持有它」和「有没有按下 --force」这两个事实。
pub fn kill_allowed(c: &KillCtx<'_>) -> bool {
    if let Some(r) = c.req_pane {
        if c.origin_pane == Some(r) || c.panes.iter().any(|p| p == r) {
            return true;
        }
    }
    c.force && label_of(c.claim) == label_of(c.by.as_deref())
}

/// 不可终止时该补什么参数——由 `kill_allowed` 的同一条规则推出，别让调用方自己猜文案。
/// 只在 ① 不成立时使用（同 pane 的行本来就可直杀，不带这个提示）。
/// tag 来自命令行，进终端/错误文案前必须净化（与全仓「用户可控字段过 sanitize」同一口径）。
pub fn kill_hint(by: &Option<String>) -> String {
    let tag = by_label(by);
    if tag == "user" {
        "--force".to_string()
    } else {
        format!("--by {} --force", sanitize(tag))
    }
}

/// 对 CLI 持有者发 SIGINT —— 等价于用户在它终端里按 Ctrl-C。
///
/// 为什么用 SIGINT 而非 SIGTERM：hsshx 的传输只在 SIGINT 上走 `watch_cancel` → 块边界退出
/// → 删远端半成品 → 回滚未存在的目录树（退出码 130）。SIGTERM 没装处理器，会直接终止进程、
/// 把半成品留在远端。**绝不能用 SIGKILL**：那会跳过租约文件的自我清理（只能等下次 scan 回收）。
pub fn signal_int(pid: u32) -> Result<()> {
    let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGINT) };
    if rc == 0 {
        Ok(())
    } else {
        let e = std::io::Error::last_os_error();
        // ESRCH：进程已死但锁还没被 scan 回收（正常竞态），报可读文案而不是裸 errno
        Err(Error::msg(if e.raw_os_error() == Some(libc::ESRCH) {
            format!("进程 {pid} 已不存在（连接已随其退出而终止）")
        } else {
            format!("向进程 {pid} 发信号失败: {e}")
        }))
    }
}

/// detail 的净化：控制字符一律替换为 U+25A1 并限长 200 字符。
///
/// 这不是装饰性校验：detail 会被 `hssh listconn` 原样 cat 到宿主终端，而它可以来自
/// `hsshx web --get "$(printf 'a\033[2J')"` 这样的路径——不去控制字符就是转义序列注入入口。
pub fn sanitize(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(200));
    for ch in raw.chars() {
        if out.chars().count() >= 200 {
            out.push('…');
            break;
        }
        // C0 控制字符 + DEL 全部剔除；其余（含中文/空格）原样保留
        out.push(if (ch as u32) < 0x20 || ch == '\x7f' { '\u{25a1}' } else { ch });
    }
    out
}

/// 文件名安全校验：id 只允许 uuid 形态，杜绝路径穿越（`../../etc/passwd` 当 id 传入时
/// 会在配置目录之外创建文件）。
fn guard_id(id: &str) -> Result<()> {
    let ok = id.len() == 36
        && id
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == '-')
        && id.matches('-').count() == 4;
    if ok {
        Ok(())
    } else {
        Err(Error::msg(format!("非法租约 id: {id:?}")))
    }
}

fn dir_of(dir: &Path) -> Result<PathBuf> {
    // 只建目录，不设权限：0700 由 settings::conns_dir 单点负责。
    // 这里绝不能用 set_owner_only——它是 0600，给目录抹掉 x 后本模块后续 open 全部失败。
    std::fs::create_dir_all(dir)?;
    Ok(dir.to_path_buf())
}

fn json_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

fn lock_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.lock"))
}

/// 已登记的租约。持有此结构即持有 flock；`drop`（含提前 return 的栈展开）自动释放并清理
/// 文件，因此正常退出与 panic 都不会留脏账。真正的 kill -9 由内核回收锁，扫描时清理。
pub struct Holder {
    dir: PathBuf,
    id: String,
    /// 锁文件句柄：只为保活而存在，字段本身不被读取
    _lock: std::fs::File,
}

/// 登记一条租约（生产路径：目录取 config/conns）。
pub fn acquire(lease: &mut Lease) -> Result<Holder> {
    let dir = settings::conns_dir()?;
    acquire_in(&dir, lease)
}

/// 登记一条租约（指定目录，便于单测隔离，不污染用户真实配置目录）。
pub fn acquire_in(dir: &Path, lease: &mut Lease) -> Result<Holder> {
    // id 先校验再动磁盘：非法 id 不该留下任何目录/文件痕迹
    guard_id(&lease.id)?;
    let dir = dir_of(dir)?;
    lease.pid = std::process::id();
    lease.started_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    lease.detail = sanitize(&lease.detail);

    // 1) 先抢锁（顺序见模块头注释）
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(lock_path(&dir, &lease.id))?;
    fs2::FileExt::try_lock_exclusive(&lock)
        .map_err(|_| Error::msg(format!("租约 id 撞车（{id} 的锁已被占用）", id = lease.id)))?;

    // 2) 再原子写 sidecar：临时文件 + rename，扫描方永远读不到半截 JSON。
    //    写失败必须连锁文件一起清掉——否则空 .lock 会永久留在目录里（扫描只遍历 json，
    //    永远不会回收它）。
    if let Err(e) = write_sidecar(&dir, lease) {
        drop(lock);
        let _ = std::fs::remove_file(lock_path(&dir, &lease.id));
        return Err(e);
    }

    Ok(Holder { dir, id: lease.id.clone(), _lock: lock })
}

/// 原子写 sidecar（0600 + rename）。失败时自行清理临时文件。
fn write_sidecar(dir: &Path, lease: &Lease) -> Result<()> {
    let tmp = dir.join(format!("{}.json.part", lease.id));
    let target = json_path(dir, &lease.id);
    let r = (|| -> Result<()> {
        let json = serde_json::to_string(lease)
            .map_err(|e| Error::msg(format!("序列化失败: {e}")))?;
        std::fs::write(&tmp, json)?;
        settings::set_owner_only(&tmp); // 0600：与其余配置文件一致
        std::fs::rename(&tmp, &target)?;
        Ok(())
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(json_path(&self.dir, &self.id));
        // 锁随 _lock 关闭释放；文件本身删掉，避免目录里堆死锁文件
        let _ = std::fs::remove_file(lock_path(&self.dir, &self.id));
    }
}

/// 列出存活租约，顺带回收僵尸（持有者已崩溃/被 kill -9 的条目）。
pub fn scan() -> Result<Vec<Lease>> {
    scan_in(&settings::conns_dir()?)
}

/// 列出指定目录下的存活租约。
///
/// 判活方式：对 sidecar 的同名 `.lock` 试加排他锁。**成功 = 没人持有 = 原持有者已消失**
/// （内核在进程死亡时释放 flock）→ 删除该条目；失败 = 仍在持有 → 保留。
/// 试锁句柄一出作用域即关闭，锁随之释放，不会误长期占用存活租约。
pub fn scan_in(dir: &Path) -> Result<Vec<Lease>> {
    let mut out = Vec::new();
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
            // 目录不存在 = 从来没人跑过 hsshx，空视图即可，不是错误
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(Error::msg(format!("读取租约目录失败: {e}"))),
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        // 文件名来自磁盘（可能被手工篡改），仍走 guard_id 再拼路径，防穿越
        if guard_id(id).is_err() {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        let lp = lock_path(dir, id);
        let held = match std::fs::OpenOptions::new()
            .create(false)
            .write(true)
            .open(&lp)
        {
            Ok(f) => match fs2::FileExt::try_lock_exclusive(&f) {
                Ok(()) => false, // 拿到了锁 → 原持有者已死
                Err(_) => true,  // 拿不到 → 还活着
            },
            // 锁文件缺失（例如被手工删）：无法证明存活，按僵尸回收
            Err(_) => false,
        };
        if !held {
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(&lp);
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(s) => match serde_json::from_str::<Lease>(&s) {
                Ok(lease) => out.push(lease),
                Err(_) => { let _ = std::fs::remove_file(&path); }
            },
            Err(_) => {}
        }
    }
    out.sort_by(|a, b| a.started_unix.cmp(&b.started_unix));
    Ok(out)
}

/// 短 id（listconn 表格展示用，也是 killconn 可接受的前缀形式）
pub fn short_id(id: &str) -> &str {
    match id.get(..8) {
        Some(s) => s,
        None => id,
    }
}

/// 在给定租约集合里定位目标：完整 id 优先，否则按唯一前缀匹配。纯函数便于单测。
///
/// 歧义与缺失都报错而不是「取第一个」——杀错连接比杀不掉更糟。
///
/// 策略唯一实现点是 [`resolve_id_in`]（完整 id 优先，否则唯一前缀）：CLI 租约与 GUI 连接
/// 必须用同一套解析，否则 listconn 表里都是 8 位短 id，却只有一半边能按短 id 被终止。
pub fn resolve_id(all: &[Lease], want: &str) -> std::result::Result<Lease, String> {
    let ids: Vec<String> = all.iter().map(|l| l.id.clone()).collect();
    let hit = resolve_id_in(&ids, want)?;
    all.iter()
        .find(|l| l.id == hit)
        .cloned()
        .ok_or_else(|| format!("内部错误：解析出的 id {hit} 不在集合内"))
}

/// 从一组 id 里按「完整匹配优先，否则唯一前缀」定位一个。
/// 返回命中的完整 id；缺失或有歧义都报错（绝不取第一个）。
pub fn resolve_id_in(ids: &[String], want: &str) -> std::result::Result<String, String> {
    if want.is_empty() {
        return Err("目标 id 为空".to_string());
    }
    let exact: Vec<&String> = ids.iter().filter(|i| **i == want).collect();
    let picked: Vec<&String> = if exact.len() == 1 {
        exact
    } else {
        ids.iter().filter(|i| i.starts_with(want)).collect()
    };
    match picked.len() {
        0 => Err(format!(
            "没有 id 为 {want} 的活跃连接（listconn 只列存活连接，已结束的自然杀不到）"
        )),
        1 => Ok(picked[0].clone()),
        n => Err(format!("前缀 {want} 匹配到 {n} 条连接，请给出更长的 id")),
    }
}

/// 定位可终止目标。**只接受 `scan()` 的结果**：残留条目（持有者已死、文件未回收）的 pid
/// 可能已被系统复用，对它发信号会误杀无关进程；而 flock 由内核随进程死亡释放，故「仍在
/// scan 列表里」严格等价于「该 pid 就是当时登记的那个持有者」。
pub fn find_kill_target(id: &str) -> Result<Lease> {
    let all = scan()?;
    resolve_id(&all, id).map_err(Error::msg)
}

/// 单条租约是否仍被持有：用与 `scan` 同一条判据（试加排他锁），但**不做任何回收**。
///
/// killconn 发信号后的确认走这个，而不是每轮全量 `scan()`：整体扫描会顺带回收其他进程
/// 的僵尸条目，让一个「等对方退出」的轮询带上写副作用，也做了多余的事。
/// 判据与 scan 一致：拿得到锁=没人持有=已退出；拿不到=仍在持有；锁文件打不开=已消失。
pub fn still_alive(id: &str) -> bool {
    let Ok(dir) = settings::conns_dir() else { return false };
    still_alive_in(&dir, id)
}

/// 指定目录版（与 `scan_in`/`acquire_in` 同构：可注入目录才能离线单测，不碰真实配置）
pub fn still_alive_in(dir: &Path, id: &str) -> bool {
    if guard_id(id).is_err() {
        return false; // 非法 id 不去拼路径（防穿越）
    }
    match std::fs::OpenOptions::new().write(true).open(lock_path(dir, id)) {
        Ok(f) => fs2::FileExt::try_lock_exclusive(&f).is_err(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个用例独占一份目录：cargo test 并行执行，共用目录会让 A 用例的回收
    /// 撞上 B 用例的登记，flaky 且假阳。
    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hssh-lease-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn sample(id: &str) -> Lease {
        Lease {
            id: id.into(),
            pid: 0,
            kind: "put".into(),
            profile: "web".into(),
            host: "10.0.0.9".into(),
            port: 22,
            user: "root".into(),
            by: Some("qwen".into()),
            pane: Some("pane-A".into()),
            detail: "--put ./a → /srv/in".into(),
            started_unix: 0,
        }
    }

    fn uuidish(seed: u8) -> String {
        // 固定形态的 36 字符 id（满足 guard_id），避免随机导致断言不稳定
        format!(
            "00000000-0000-4000-8000-{:012x}",
            u128::from(seed)
        )
    }

    #[test]
    fn acquire_then_scan_sees_live_lease() {
        let dir = temp_dir("live");
        let mut lease = sample(&uuidish(1));
        let _h = acquire_in(&dir, &mut lease).expect("登记失败");
        let got = scan_in(&dir).expect("扫描失败");
        assert_eq!(got.len(), 1, "持锁期间必须被判定为存活");
        assert_eq!(got[0].id, lease.id);
        assert_eq!(got[0].by.as_deref(), Some("qwen"), "归属标记必须随租约带出");
        assert_eq!(got[0].pid, std::process::id(), "pid 由 acquire 强制写为自身");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 回归锁：drop 释放锁后，扫描必须把它当僵尸回收（连带删掉两个文件），
    /// 这条锁住的是「崩溃不需要人工清账」这个核心承诺。
    #[test]
    fn released_lease_is_reaped_and_files_gone() {
        let dir = temp_dir("reap");
        let mut lease = sample(&uuidish(2));
        let id = lease.id.clone();
        {
            let _h = acquire_in(&dir, &mut lease).unwrap();
        } // Holder 已 drop：自己就删了文件
        assert!(scan_in(&dir).unwrap().is_empty(), "已释放的租约不得出现在视图里");
        assert!(!json_path(&dir, &id).exists(), "json 必须被清理");
        assert!(!lock_path(&dir, &id).exists(), "lock 必须被清理");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 模拟真实崩溃：sidecar 存在、锁文件存在，但没人持有锁（内核已在进程死亡时释放）。
    /// 用「自己抢锁后立刻释放」复现这一磁盘状态，验证回收分支真的被执行到。
    #[test]
    fn crash_residue_without_lock_holder_is_reaped() {
        let dir = temp_dir("crash");
        let id = uuidish(3);
        dir_of(&dir).unwrap();
        std::fs::write(json_path(&dir, &id), serde_json::to_string(&sample(&id)).unwrap()).unwrap();
        // 建锁文件但不持有：等价于持有者已崩溃后的残留
        let f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(lock_path(&dir, &id))
            .unwrap();
        fs2::FileExt::lock_exclusive(&f).unwrap();
        fs2::FileExt::unlock(&f).unwrap();
        drop(f);

        assert!(scan_in(&dir).unwrap().is_empty(), "无持有者的残留条目必须被回收");
        assert!(!json_path(&dir, &id).exists(), "回收必须删掉 sidecar");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_sidecar_is_dropped_not_fatal() {
        let dir = temp_dir("corrupt");
        let id = uuidish(4);
        dir_of(&dir).unwrap();
        std::fs::write(json_path(&dir, &id), "{ not json").unwrap();
        let f = std::fs::OpenOptions::new().create(true).write(true).open(lock_path(&dir, &id)).unwrap();
        fs2::FileExt::lock_exclusive(&f).unwrap(); // 假装还活着，走解析分支
        let got = scan_in(&dir).unwrap();
        assert!(got.is_empty(), "坏 JSON 必须被跳过而不是让整次扫描失败");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// id 校验：uuid 之外的形态一律拒绝，且不落任何文件——否则 `id=../../x` 会把
    /// 文件写到配置目录之外。
    #[test]
    fn path_traversal_id_is_rejected() {
        let dir = temp_dir("traverse");
        for bad in ["../../etc/passwd", "short", "", &"z".repeat(36)] {
            let mut lease = sample(bad);
            assert!(acquire_in(&dir, &mut lease).is_err(), "非法 id 必须拒绝: {bad:?}");
        }
        assert!(!dir.join("..").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// detail 净化：ESC 序列必须被打断，否则 listconn 输出即终端注入。
    #[test]
    fn sanitize_breaks_escape_sequences() {
        let dirty = format!("a\ts\u{1b}[2Jb\r\n{}", "x".repeat(300));
        let clean = sanitize(&dirty);
        assert!(!clean.contains('\u{1b}'), "ESC 必须被剔除");
        assert!(!clean.contains('\r') && !clean.contains('\n'), "CR/LF 必须被剔除");
        assert!(!clean.contains('\t'), "TAB 必须被剔除");
        assert!(clean.chars().count() <= 201, "必须限长，实际 {}", clean.chars().count());
        // 正常中文/空格路径不受影响（回归：净化不得破坏可读性）
        assert_eq!(sanitize("--put ./a → /srv/in"), "--put ./a → /srv/in");
    }

    /// 顺序契约：sidecar 出现时锁必须已被持有。这条锁住模块头里写死的先后顺序——
    /// 调换后本测试会以「try_lock 成功」失败。
    #[test]
    fn sidecar_never_appears_before_lock_is_held() {
        let dir = temp_dir("order");
        let mut lease = sample(&uuidish(5));
        let _h = acquire_in(&dir, &mut lease).unwrap();
        let f = std::fs::OpenOptions::new()
            .write(true)
            .open(lock_path(&dir, &lease.id))
            .unwrap();
        assert!(
            fs2::FileExt::try_lock_exclusive(&f).is_err(),
            "json 已存在却拿得到锁 = 登记顺序被写反了"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// still_alive 必须与 scan 用同一条判据，否则 killconn 的「等对方退出」会和
    /// listconn 的「谁活着」互相矛盾。三个方向都测：持有中为真、释放后为假、非法 id 为假。
    #[test]
    fn still_alive_matches_scan_liveness() {
        let dir = temp_dir("alive");
        let mut lease = sample(&uuidish(8));
        let id = lease.id.clone();
        let h = acquire_in(&dir, &mut lease).unwrap();

        assert!(still_alive_in(&dir, &id), "持锁期间必须判活（否则 killconn 会误报已终止）");
        assert_eq!(scan_in(&dir).unwrap().iter().any(|l| l.id == id), true, "与 scan 判据不一致");

        drop(h);
        assert!(!still_alive_in(&dir, &id), "释放后必须判死");
        assert!(!still_alive_in(&dir, "../../etc/passwd"), "穿越 id 必须判死且不碰路径");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// id 解析是 CLI 租约与 GUI 连接共用的唯一策略，而「歧义绝不取第一个」是防误杀的
    /// 最后防线——杀错连接比杀不掉更糟，所以这条必须锁住。
    #[test]
    fn id_resolution_is_exact_then_unique_prefix() {
        let ids: Vec<String> = ["aabbccdd-1111", "aazz-2222", "ffee-3333"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        // 完整 id 命中
        assert_eq!(resolve_id_in(&ids, "ffee-3333").unwrap(), "ffee-3333");
        // 唯一前缀命中（listconn 表里显示的就是短 id）
        assert_eq!(resolve_id_in(&ids, "ffee").unwrap(), "ffee-3333");
        // 歧义前缀：报错而不是取第一个
        let amb = resolve_id_in(&ids, "aa").unwrap_err();
        assert!(amb.contains("2 条") && amb.contains("更长的 id"), "歧义文案异常: {amb}");
        // 完整匹配优先于前缀：若某 id 恰是另一个的前缀，必须选完全相等的那个
        let nested: Vec<String> = ["aa", "aabbcc"].iter().map(|s| s.to_string()).collect();
        assert_eq!(resolve_id_in(&nested, "aa").unwrap(), "aa");
        // 缺失与空值都是错误（空值不得退化成「匹配全部」）
        assert!(resolve_id_in(&ids, "zz").unwrap_err().contains("没有"));
        assert!(resolve_id_in(&ids, "").unwrap_err().contains("为空"));
    }

    #[test]
    fn empty_dir_scan_is_ok_not_error() {
        let dir = std::env::temp_dir().join("hssh-lease-test-absent-does-not-exist");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(scan_in(&dir).unwrap().is_empty(), "目录不存在应为空视图");
    }

    /// 跨进程验证整套设计的承重墙：**持有者死亡 → 内核释放 flock → 扫描判僵尸并回收**。
    ///
    /// 前面 8 条都在同进程内用 try_lock 模拟，证明不了内核的跨进程语义。这里用外部
    /// `flock(1)` 真起一个子进程持锁，走两个相反断言：活着时必须判活、被 kill -9 后必须
    /// 判僵尸并删文件。两个断言互为对照——若测试空转（子进程没抢到锁），第一条就会失败。
    ///
    /// 依赖 util-linux 的 flock（Debian/Ubuntu 与 GitHub runner 均自带）；缺失即失败，
    /// 绝不静默跳过冒充通过。
    #[test]
    fn cross_process_holder_death_is_detected_and_reaped() {
        use std::process::{Command, Stdio};

        let dir = temp_dir("xproc");
        dir_of(&dir).unwrap();
        let id = uuidish(7);
        let lp = lock_path(&dir, &id);
        // 顺序遵守 acquire 的契约：**json 出现时 lock 必须已存在**。所以先建 lock、等外部
        // 真持有、最后才写 json。反着写会让 scan 把「json 有 lock 无」正确判成僵尸并删掉——
        // 那是被测代码的正确行为，测试却会把它误报成缺陷（本用例第一版就踩了这个）。
        std::fs::write(&lp, b"").expect("预建 lock 文件失败");

        // 持有者必须单进程。`flock -x file -c "sleep 20"` 会 fork，子进程继承锁 fd（flock
        // 不能给锁 fd 设 CLOEXEC，否则 exec 后锁就丢了），kill 父进程锁并不释放——已实测。
        // 用 exec 让 sh 原地变成 sleep，锁始终挂在同一个 pid 上，kill 才真的释放。
        let holder = format!("exec 9>{}; flock -x 9; exec sleep 10", lp.display());
        let mut child = Command::new("sh")
            .args(["-c", &holder])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("环境缺 sh/flock(1)：宁可失败也不静默跳过冒充通过");

        // 前置自检：确认外部进程真拿到了锁才搭现场，否则「判活」断言是在空转。
        let mut held = false;
        for _ in 0..60 {
            if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&lp) {
                let got = fs2::FileExt::try_lock_exclusive(&f);
                if got.is_ok() {
                    let _ = fs2::FileExt::unlock(&f);
                }
                held = got.is_err();
                drop(f);
                if held {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(held, "前置不成立：外部进程未持有锁，此时判活断言无从验证");

        // 现场就绪，现在才写 sidecar
        std::fs::write(
            json_path(&dir, &id),
            serde_json::to_string(&sample(&id)).unwrap(),
        )
        .unwrap();
        let got = scan_in(&dir).unwrap();
        assert_eq!(got.len(), 1, "外部进程持锁期间必须判活");
        assert_eq!(got[0].id, id, "判活结果必须是本用例的条目");

        // kill -9：不给任何清理机会，纯靠内核随进程销毁释放锁
        child.kill().expect("kill 失败");
        let _ = child.wait();
        let mut reaped = false;
        for _ in 0..60 {
            if scan_in(&dir).unwrap().iter().all(|l| l.id != id) {
                reaped = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(reaped, "持有者被 SIGKILL 后 flock 未随进程释放 → 租约会永久说谎");
        assert!(!json_path(&dir, &id).exists(), "回收必须删掉 sidecar");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
