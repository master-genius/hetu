# 待办能力（next-upgrade）

批次收口后明确**不放在当前版本**的能力，连同已核实的代码边界与最小实现口径记录在此，避免重复讨论与重复踩坑。

所有「现状」条目都按代码核实过（附文件:行），不是按计划记忆写的。

---

## 1. tab 级可编程入口（关 tab / 标签页寻址）

**现状**

- 控制面只有四种模式：`listconn` / `killconn` / `dump` / `open`（`src/pane.ts:52-56`），**没有任何 tab 级动作**。
- `closeTab` 的触发者仅两个：tab 右键菜单（`src/main.ts:304` → `requestCloseTab:555`）、最后一个分屏被关闭时的隐式收尾（`src/tabs.ts:479`）。键位定义里搜不到关 tab 的快捷键。
- `ConnInfo`（`src-tauri/src/ssh/conn.rs:103-120`）**不含 tab id**；但 `origin_pane` 是有值的——`hssh --new` 建连时就把它记成请求方 pane（`src/main.ts:417`），而 `panesByConn(connId)`（`src/tabs.ts:190`）能从连接反查到 pane 及其所属 tab。

**残余缺口**

AI 用 `hssh --new` 开的 tab，连接被 `killconn` 收掉后会留下一个**空的本地终端 tab**。AI 关不掉它，人只能用右键菜单关（没有快捷键）。

**已裁定**：不加 `--close` 开关；tab 来源标记与标题 `[AI]` 前缀一并暂缓（2026-10-18）。归属信息已经在 `--by` 里公开印在 listconn 表上，为「关一个空 tab」新增 CLI 开关或 tab 状态属过度设计。

**若将来要做，最小口径**

1. 寻址直接用 `connId`（`--new` 的回程文案已给出），**不需要新增 Tab 字段**。
2. 授权复用 `lease::kill_allowed` 的 `origin_pane` 语义，不另起判据点。
3. 必须先定「忙碌 tab」的确认语义：`requestCloseTab` 在 `hasBusyPane` 时弹 `confirmDialog`（`src/main.ts:557-561`），而 AI 调用无人应答弹窗，会白等满脚本侧 10s 轮询——这与 `--new` 里「需交互输入凭据直接拒绝」是同一类坑（`src/main.ts:412-416`）。二选一：**跳过确认**（风险：连正在跑程序的 tab 一起杀）或**尊重 busy 并返回非零码**。这是本项唯一的真实障碍，不是代码量。

---

## 2. `dump` 跨 pane 读取（AI 观察它打开的那个 tab 的输出）

**现状**

- `screenText` 是纯函数（`pane.term.buffer.active` + `getLine(i).translateToString(true)`，`src/main.ts:332-339`），入参就是一个 `Pane`。
- `handleHctl` 传进去的是**发起控制面的那个 pane**（`src/main.ts:424`），所以 `dump` 只读得到本终端；脚本帮助也这么写：「导出**本终端**屏幕文本」（`src-tauri/src/local.rs:255`）。
- 技术上限比现状高得多：`findPane` / `panesByConn` 已经能定位任意 pane，因此「读别的终端」不缺能力，缺的是**寻址参数**与**授权**。

**最小口径（三层，缺一不可）**

1. **寻址**：`dump --conn <id>`，按 `panesByConn` 定位。一个连接可能挂在多个 pane 上，需定义取哪个（建议：唯一命中才读，多命中要求显式指定 pane，避免静默选一个）。
2. **授权**：按 `origin_pane` 收窄，与 `kill_allowed` 同口径。否则任何本地终端都能读别人 tab 的屏幕——屏幕上可能有口令提示、私钥 passphrase、交互应答，直接违背最小信息泄漏原则。
3. **时机**：需要等待语义才有实用价值。当前 dump 是快照，AI 只能盲轮询。最小形态 `--until <串> --timeout N`；注意超时上限必须**短于脚本侧 10s 轮询**，否则脚本先超时、拿不到结论（同 `write_reply` 首行状态位那套约束）。

**未决**：这项值不值得做，取决于「人也要围观的交互式 tab」有多常用。纯无头执行已由 `hsshx`（三条流分离 + `--track` + 取消传播）覆盖，本项不是它替代品。

---

## 3. 已知边界（不是待办，是别重复踩）

- **`hssh -l` / `--list` / `list` 在 agent 环境里会等满超时。** 它走 **1729**（`emit` + `read _hssh_list_done` park，`src-tauri/src/local.rs:396-401`），而 `listconn` / `killconn` / `dump` 走 **1734 + 响应文件**（`hctl_call`）。agent 的嵌套 pty 收不到 1729 的屏幕回写，于是三种写法全部 rc=124 等满（实测三者输出 md5 全等，`-l` 同样超时，故这是既有信道限制而非别名缺陷）。AI 要列连接项请直接用 `hsshx -l`（读 profiles.json，不经 GUI）。若要统一，就把 `mode=list` 也接进 1734 通道。
- **`listconn` 的可见性是全局的，tab/pane 只收窄 `killconn` 的授权**：`hsshx listconn` 读租约目录，跨 tab 一律可见；看不见的只是「别的发起方」（GUI/`hssh` 建的连接活在 GUI 进程内存），要看合并视图用 `hssh listconn`。
- **连接终止后回落本地终端是产品定的行为**（`src/main.ts:640-656`），不是遗留缺陷，不要因为「看起来该关」就改掉。
