//! 终端 pane：在既有连接上开一个 PTY channel + shell。
//! 每个 pane 一个 tokio 任务独占 channel，通过 mpsc 接收输入/resize/关闭指令，
//! 输出以 Channel 事件推送给前端 xterm（点对点，无全局广播）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use russh::ChannelMsg;
use serde::Serialize;
use tauri::{AppHandle, ipc::Channel};
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;

use crate::error::{Error, Result};
use crate::ssh::conn::Connection;

/// 前端 per-pane 事件：通过 Tauri Channel 点对点推送，取代全局 app.emit 广播。
/// Channel 在 pane_open 命令调用时由前端创建并传入，天然绑定到特定 pane，
/// 无需 paneId 路由，重连时旧 Channel 自动失效（旧任务残余输出不会混入新 pane）。
#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PaneEvent {
    /// PTY 数据输出（base64 编码）
    Output { data: String },
    /// 远程 shell 退出状态码
    Exit { status: i32 },
    /// channel 关闭：exited=true 表示 shell 自行退出，false 表示连接断开。
    /// resumable=true 表示「连接仍然活着，只是这个 channel 被关了」，上层重开 channel
    /// 即可恢复 —— 与连接级重连是两条不同的恢复路径，必须让前端分得清。
    Closed {
        exited: bool,
        resumable: bool,
    },
}

pub enum PaneCmd {
    Data(Vec<u8>),
    Resize { cols: u32, rows: u32 },
    Close,
}

pub struct PaneCtl {
    pub tx: mpsc::UnboundedSender<PaneCmd>,
    pub conn_id: String,
    /// 本地 PTY shell 的进程号（仅本地 pane 有；SSH pane 为 None）。
    /// 用于经 /proc/<pid>/cwd 读本地终端的实时工作目录（拖拽下载落点）。
    pub local_pid: Option<u32>,
    /// 远程 pane 的 cwd（前端 OSC 7 同步）。本地 pane 由 /proc/<pid>/cwd 直接读。
    pub cwd: tokio::sync::Mutex<Option<String>>,
}

fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// channel 关闭后应有的动作。判据顺序即语义优先级，不可调换。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseOutcome {
    /// shell 自行退出 / 我们主动关闭：不做任何恢复
    Nothing,
    /// 连接也断了：走连接级重连
    Reconnect,
    /// 连接仍活着、仅该 channel 被关：原地重开 channel
    ResumeChannel,
}

/// 决定关闭后做什么。**纯函数**，顺序与组合由单测锁住。
///
/// 顺序不可调换的原因：
/// - `deliberate`（主动关闭）必须最先判。用户关分屏 / 切换连接时连接往往还活着，
///   若让后续分支先命中，前端会为刚关掉的 pane 重开会话，把切换连接时新建的通道顶掉。
/// - `exited`（shell 自行退出）优先于重连：正常 `exit` 后重连会让用户退出不掉。
fn close_outcome(exited: bool, deliberate: bool, conn_alive: bool) -> CloseOutcome {
    if deliberate || exited {
        return CloseOutcome::Nothing;
    }
    if conn_alive {
        CloseOutcome::ResumeChannel
    } else {
        CloseOutcome::Reconnect
    }
}

/// 建 channel 与协商 PTY 阶段的单步超时。TCP 已断/半开时，这几步都会无限期等待服务器
/// 回应，而 `open` 是 `pane_open` 这个 IPC 命令的实现体 —— 一挂住前端 await 就永不来，
/// 界面永久停在「打开中」。给每个网络往返一个有界上限，超时即如实报错让前端走失败路径。
const OPEN_STEP_TIMEOUT: Duration = Duration::from_secs(20);

/// 打开 pane：请求 PTY(xterm-256color) + shell，随后进入事件循环。
/// 返回后 channel 已就绪；循环任务在后台运行。
/// `on_event` 是 Tauri Channel，点对点推送 PaneEvent 到前端对应 pane。
pub async fn open(
    app: AppHandle,
    conn: Arc<Connection>,
    cols: u32,
    rows: u32,
    mut rx: mpsc::UnboundedReceiver<PaneCmd>,
    on_event: Channel<PaneEvent>,
) -> Result<()> {
    let channel = {
        let guard = conn.handle.lock().await;
        let handle = guard.as_ref().ok_or_else(|| Error::msg("连接未建立"))?;
        timeout(OPEN_STEP_TIMEOUT, handle.channel_open_session())
            .await
            .map_err(|_| Error::msg("打开 shell 超时：服务器未确认 channel"))??
    };
    // PTY / shell 请求同样限时：服务器不回应时不能把调用方永久挂住。
    // want_reply 保持 false（与改动前一致）：本端不等 RequestSuccess，超时只兜住
    // 「消息发不出去」（会话已断、发送队列满）这一类真实挂起。
    timeout(
        OPEN_STEP_TIMEOUT,
        channel.request_pty(false, "xterm-256color", cols, rows, 0, 0, &[]),
    )
    .await
    .map_err(|_| Error::msg("打开 shell 超时：PTY 请求未能发出"))??;
    timeout(OPEN_STEP_TIMEOUT, channel.request_shell(false))
        .await
        .map_err(|_| Error::msg("打开 shell 超时：shell 请求未能发出"))??;

    // 读/写分离：ChannelWriteHalf 的方法都是 &self，move 到写 task 不需要锁；
    // ChannelReadHalf 的 wait() 需要 &mut self，独占读 task。
    // 这样写 task 的 data().await 阻塞（远程窗口满）时，读 task 仍可 poll wait()，
    // 不会出现 Ctrl+C 等 cat/grep 大输出结束后才生效的问题。
    let (mut read_half, write_half) = channel.split();
    let (done_tx, mut done_rx) = oneshot::channel::<()>();

    // 主动关闭标记：用户关 pane / 切换连接 / 连接回收都会发 PaneCmd::Close。
    // 这类关闭必须先于「resumable」判定短路——否则连接还活着的正常收尾会被误判成
    // 「服务器把 channel 关了」，前端随即为刚关掉的 pane 重开会话，把切换连接时新建的
    // 通道顶掉（本地 PTY 线程同构，用 deliberate 区分主动关闭与自行退出）。
    let deliberate = Arc::new(AtomicBool::new(false));
    let deliberate_r = deliberate.clone();

    // 读 task：独占 read_half
    tokio::spawn(async move {
        let mut exited = false;
        loop {
            match read_half.wait().await {
                Some(ChannelMsg::Data { data }) => {
                    let _ = on_event.send(PaneEvent::Output { data: b64(&data) });
                }
                Some(ChannelMsg::ExtendedData { data, .. }) => {
                    let _ = on_event.send(PaneEvent::Output { data: b64(&data) });
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    exited = true;
                    let _ = on_event.send(PaneEvent::Exit { status: exit_status as i32 });
                }
                Some(_) => {}
                None => {
                    // channel 已关闭。三条路径必须分清，判据顺序不可调换：
                    // ⓪ 主动关闭（用户关 pane / 切换连接 / 重连替换旧任务）→ 不做任何恢复。
                    //    必须放在最前：此时连接往往还活着，否则会被误判成 ②，
                    //    前端会为刚关掉的 pane 重开会话，把切换连接时新建的通道顶掉。
                    // ① 连接也断了 → 触发连接级重连（原有行为，成功后由前端重建 pane）。
                    // ② 连接还活着、只是这个 channel 被关（服务器 MaxSessions 清理、sshd 侧
                    //    主动 CHANNEL_CLOSE、ExitStatus 报文丢失）→ 不需要重连，前端用同一
                    //    connId 重开 channel 即可，故打上 resumable 标记。
                    // 改动前只有 ① 有动作；② 会静默停在「终端不动、无任何提示」，用户唯一
                    // 出路是关标签页 —— 这正是必须消灭的假死态。
                    // 判据本体抽成纯函数 close_outcome，由单测锁住顺序与组合。
                    let alive = conn.is_alive().await;
                    let outcome = close_outcome(exited, deliberate_r.load(Ordering::SeqCst), alive);
                    if outcome == CloseOutcome::Reconnect {
                        // 只启动退避任务。状态推进到 Reconnecting 由任务自己在入口完成
                        // （`ReconnectStarted`）—— 不能在这里先推进状态：那会写入
                        // `reconnecting` 原子位，使 trigger_reconnect 的幂等检查立即返回，
                        // 任务根本不启动，连接就此永久失去自愈能力。
                        conn.clone().trigger_reconnect(app.clone());
                    }
                    let _ = on_event.send(PaneEvent::Closed {
                        exited,
                        resumable: outcome == CloseOutcome::ResumeChannel,
                    });
                    break;
                }
            }
        }
        let _ = done_tx.send(());
    });

    // 写 task：独占 write_half（方法均为 &self，无需 &mut）
    tokio::spawn(async move {
        loop {
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    Some(PaneCmd::Data(d)) => {
                        if write_half.data(&d[..]).await.is_err() {
                            break;
                        }
                    }
                    Some(PaneCmd::Resize { cols, rows }) => {
                        let _ = write_half.window_change(cols, rows, 0, 0).await;
                    }
                    Some(PaneCmd::Close) => {
                        // 主动关闭（用户关分屏 / 切换连接 / 重连复用同一 pane_id 时替换旧任务）：
                        // 先置标记再关通道，读任务收到 None 时据此跳过恢复判定。
                        // 顺序不能反——close() 会立刻让读任务拿到 None，晚一步就来不及了。
                        deliberate.store(true, Ordering::SeqCst);
                        let _ = write_half.eof().await;
                        let _ = write_half.close().await;
                        break;
                    }
                    None => {
                        // 发送端被 drop 而**没有**发 Close：`teardown_conn` 用
                        // `panes.retain(...)` 直接移除 PaneCtl，走的正是这条路径。
                        // 那是「连接被放弃/拆除」，与主动关闭语义相反 ——
                        // 绝不能置 deliberate，否则前端会把本该结束的 pane 当成已收尾。
                        // 这里只做通道清理，不需要额外标记。
                        let _ = write_half.eof().await;
                        let _ = write_half.close().await;
                        break;
                    }
                },
                // 读任务先结束（channel 已断）→ 写侧随之收尾，这不是主动关闭，不置标记
                _ = &mut done_rx => break,
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归锁：建 channel / 协商 PTY 的每一步都必须落在有界时限内。
    ///
    /// 背景：`open` 是 `pane_open` 这个 IPC 命令的实现体。TCP 半开时服务器既不确认
    /// channel 也不回 PTY 应答，若这几步没有超时，前端 `await` 就永远不返回，界面
    /// 永久停在「打开中」——只能重启应用。改动前后行为差异无法用单测直接观测（要真
    /// 断网），故在此锁住「时限存在且量级正确」这一不变量：它一旦被删或改成极大值，
    /// 本测试立刻失败。
    #[test]
    fn open_steps_are_bounded() {
        let t = OPEN_STEP_TIMEOUT;
        assert!(t.as_secs() > 0, "必须有非零时限：零时长会让正常建连也被判超时");
        assert!(
            t.as_secs() <= 60,
            "上限过大等于没有超时（实测 {t:?}）：IPC 调用方会被挂住"
        );
    }

    /// 回归锁：Closed 事件必须携带 resumable，且它与 exited 互斥。
    /// 前端靠 resumable 区分「重连」与「原地重开 channel」两条恢复路径；
    /// 缺了它，channel 被服务器关掉时前端只能静默不动（曾经的假死态）。
    #[test]
    fn closed_event_carries_resumable_flag() {
        let ev = PaneEvent::Closed { exited: false, resumable: true };
        let json = serde_json::to_string(&ev).expect("PaneEvent 应可序列化");
        assert!(json.contains("resumable"), "前端读不到该字段就无法区分恢复路径: {json}");
        // 线格式必须是 camelCase 同级字段（tag 已由 serde 展开为 type）
        assert!(!json.contains("resumable_"));
    }

    /// 回归锁：主动关闭必须先于一切恢复判定命中。
    ///
    /// 现场：用户切换连接 / 关分屏时，连接往往还活着。若 `deliberate` 判晚了，就会落到
    /// ResumeChannel，前端随即为刚关掉的 pane 重开会话，把切换连接时新建的通道顶掉 ——
    /// 表现为「切换连接后终端闪断或错乱」。这条断言直接锁死优先级顺序。
    #[test]
    fn deliberate_close_never_resumes() {
        // 连接活着也不能重开
        assert_eq!(close_outcome(false, true, true), CloseOutcome::Nothing);
        // 连接死了也不该重连（是我们要关的）
        assert_eq!(close_outcome(false, true, false), CloseOutcome::Nothing);
        // 即便 exited 与 deliberate 同时成立，仍是 Nothing
        assert_eq!(close_outcome(true, true, true), CloseOutcome::Nothing);
    }

    /// 回归锁：shell 自行退出（exited=true）不得触发重连或重开，
    /// 否则用户 `exit` 之后终端会自己弹回来。
    #[test]
    fn natural_exit_never_recovers() {
        assert_eq!(close_outcome(true, false, true), CloseOutcome::Nothing);
        assert_eq!(close_outcome(true, false, false), CloseOutcome::Nothing);
    }

    /// 回归锁：非主动、连接仍活 → 原地重开 channel（本轮修掉的假死态）。
    /// 非主动、连接已断 → 连接级重连（改动前的唯一行为，必须保持）。
    #[test]
    fn passive_close_picks_the_right_recovery() {
        assert_eq!(close_outcome(false, false, true), CloseOutcome::ResumeChannel);
        assert_eq!(close_outcome(false, false, false), CloseOutcome::Reconnect);
    }
}
