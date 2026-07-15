# Hai Agent 架构设计

> HetuShell 内建 AI Agent，独立模块，单仓库，Web 形态，绑定 Tab，跨分屏。
> 2026-07-15 初版，2026-07-15 修订（补充 Provider 抽象、上下文管理、安全强制、并发控制、错误重试、Phase 拆分）。

---

## 1. 核心差异化

| 能力 | 传统 CLI AI | Hai Agent |
|------|-------------|-----------|
| 本地文件操作 | ✓ | ✓ |
| 远程文件操作 | 需重新 SSH 认证 | **复用已有连接，SFTP 直通** |
| 远程命令执行 | `ssh host "cmd"` 单次 | **复用已有连接，PTY 实时输出** |
| 跨机器协调 | ✗ | **一个 Tab 内协调多个 Pane 的连接** |
| 终端上下文 | 粘贴输出 | **实时读 xterm buffer** |
| UI | 纯文本 | **Web 模态弹窗，工具调用可视化** |

---

## 2. 顶层架构

```
┌────────────────────  Frontend (WebView)  ──────────────────────────┐
│                                                                      │
│  ┌─ Tab X ────────────────────────────────────────────────────────┐ │
│  │                                                                  │ │
│  │  ┌─ Pane A (SSH → host-A) ──┐  ┌─ Pane B (local) ──────────┐  │ │
│  │  │  $ hai                    │  │                            │  │ │
│  │  │    Agent 绑定 Tab →       │  │                            │  │ │
│  │  │    可操作 Pane A + B      │  │                            │  │ │
│  │  └──────────────────────────┘  └────────────────────────────┘  │ │
│  │                                                                  │ │
│  │  ┌─ Agent Modal (叠加层) ─────────────────────────────────────┐ │ │
│  │  │  · 工具调用可视化（可折叠）                                  │ │ │
│  │  │  · 流式消息                                                 │ │ │
│  │  │  · 模式切换：Auto / Ask / Plan                              │ │ │
│  │  │  · Pane 选择器（跨 Pane 操作时）                            │ │ │
│  │  └────────────────────────────────────────────────────────────┘ │ │
│  └──────────────────────────────────────────────────────────────────┘ │
│                                                                      │
│  ┌─ AgentSessionManager ──────────────────────────────────────────┐  │
│  │  Map<TabId, AgentSession>                                       │  │
│  │  · 生命周期绑定 Tab                                             │  │
│  │  · Tab 切换 → 挂起                                              │  │
│  │  · Tab 关闭 → 销毁 tokio task                                    │  │
│  └────────────────────────────────────────────────────────────────┘  │
└───────────────────────┬─────────────────────────────────────────────┘
                        │ Tauri invoke + Channel
                        ▼
┌────────────────────  Rust Backend  ─────────────────────────────────┐
│                                                                      │
│  ┌─ AgentManager ─────────────────────────────────────────────────┐ │
│  │  HashMap<SessionId, AgentHandle>                                │ │
│  │  spawn(tabId, config) → SessionId                               │ │
│  │  send_message(sessionId, text) → ()                             │ │
│  │  approve_tool(sessionId, approve) → ()   // Ask 模式            │ │
│  │  answer_question(sessionId, choice) → ()  // Agent 提问        │ │
│  │  abort(sessionId) → ()                                          │ │
│  │                                                                 │ │
│  │  Session 绑定 Tab（非 Pane）：                                 │ │
│  │  · 同一 Tab 内第二个 Pane 执行 hai → 聚焦已有 Modal            │ │
│  │  · 不创建第二个 Session                                         │ │
│  └────────────────────────────────────────────────────────────────┘ │
│                                                                      │
│  ┌─ ReAct Loop (per-session tokio task) ──────────────────────────┐ │
│  │  // 中止检查点：每次 LLM 请求前 + 每次工具执行前               │ │
│  │  // 并发控制：Session 内消息串行处理，新消息排队                 │ │
│  │  loop {                                                          │ │
│  │    if aborted { break }                                          │ │
│  │    // 上下文窗口管理：超限时截断历史                             │ │
│  │    trim_history(&mut history, max_tokens)                        │ │
│  │    messages = [system_prompt, tools, history, user_msg]           │ │
│  │    response = provider.chat(messages, tools).await                │ │
│  │    match response {                                               │ │
│  │      TextChunk(t)     → channel.push(Message(t))                  │ │
│  │      TextDone         → channel.push(Done)                        │ │
│  │      ToolCall(name, args) →                                       │ │
│  │        if aborted { break }                                      │ │
│  │        match mode {                                               │ │
│  │          Auto → result = tool.execute(args).await                 │ │
│  │          Ask  → channel.push(AskApproval(name, args))             │ │
│  │                  Wait for user approval                           │ │
│  │                  result = tool.execute(args).await                │ │
│  │          Plan → channel.push(ProposedPlan) // 暂不执行            │ │
│  │        }                                                          │ │
│  │        history.push(tool_result)                                  │ │
│  │        continue  // 循环                                          │ │
│  │      Question(q, choices) →                                      │ │
│  │        channel.push(UserQuestion(q, choices))                    │ │
│  │        pause task                                                │ │
│  │        wait for answer_question(sessionId, choice)              │ │
│  │        history.push(user_choice_as_message)                      │ │
│  │        continue                                                   │ │
│  │    }                                                              │ │
│  │  }                                                                │ │
│  └────────────────────────────────────────────────────────────────┘ │
│                                                                      │
│  ┌─ Tool Registry ────────────────────────────────────────────────┐ │
│  │  所有工具接收 (tab_id, target_pane_id) 路由                      │ │
│  │                                                                  │ │
│  │  read_file     ───►  target pane 的连接 → 本地 fs / SFTP       │ │
│  │  write_file    ───►  target pane 的连接 → 本地 fs / SFTP       │ │
│  │  run_command   ───►  target pane 的连接 → PTY exec / SSH exec  │ │
│  │  list_dir      ───►  target pane 的连接 → 本地 / SFTP          │ │
│  │  search        ───►  target pane 的连接 → grep                 │ │
│  │  read_terminal ───►  读 target pane 的 xterm buffer             │ │
│  │  list_panes    ───►  返回 Tab 内所有 Pane 信息（连接、cwd）     │ │
│  └────────────────────────────────────────────────────────────────┘ │
│                                                                      │
│  ┌─ LLM Provider ─────────────────────────────────────────────────┐ │
│  │  trait LlmProvider（抽象层，Phase 1 就位）                      │ │
│  │  ├─ OpenAI-compatible 实现（DeepSeek/OpenAI）                   │ │
│  │  └─ Anthropic 实现（Phase 4 实装，接口已预留）                  │ │
│  │  · 负载均衡：同模型多 key → round-robin                        │ │
│  │  · 错误重试：429 自动切换 key，超时重试 N 次                    │ │
│  │  · 密钥：独立加密存储（app_data_dir/ai-keys）                    │ │
│  └────────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────────┘
```

---

## 3. 执行模式

```
┌─────────────────────────────────────────────────────┐
│  [Auto ▾]  [📎]  [🧠]  [⚙]              [⏸ 中止]  │
├─────────────────────────────────────────────────────┤
│                                                      │
│  下拉选项：                                          │
│  ● Auto   — 自主执行，不询问                         │
│  ○ Ask    — 每次工具调用前确认                       │
│  ○ Plan   — 先出计划，确认后才进入 Auto/Ask 执行     │
│                                                      │
└─────────────────────────────────────────────────────┘
```

| 模式 | 工具执行 | 适用场景 |
|------|----------|----------|
| **Auto** | 立即执行，实时显示过程 | 信任度高，快速迭代 |
| **Ask** | 每次弹出确认框，用户 approve/reject | 高风险操作，敏感环境 |
| **Plan** | 先输出执行计划（不调工具），用户确认后切换到 Auto/Ask | 复杂的多步骤任务 |

模式可在对话中动态切换。

---

## 4. 工具定义

### 4.1 系统提示词结构

```
你是 HetuShell 的 AI 助手，运行在用户终端环境中。

## 当前 Tab 的 Pane 列表
| # | 类型 | 主机 | 当前目录 | 操作系统 |
|---|------|------|----------|----------|
| 0 | SSH  | prod-server | /opt/app | Linux   |
| 1 | 本地 | localhost   | /home/wy/project | Linux |

默认在 Pane 0（你被调用的那个 Pane）上执行操作。
使用 list_panes 查看所有可用 Pane。
使用工具时指定 target_pane 来选择在哪个 Pane 上执行。

## 可用工具
{{tools_json_schema}}

## 项目记忆
{{project_memory}}

## 规则
- 文件操作默认限制在对应 Pane 的 cwd 子树内
- 危险命令（rm -rf, dd, shutdown 等）需要用户确认
```

### 4.2 工具清单

```rust
enum Tool {
    // 文件系统（通过 target_pane 的 conn_id 路由到本地/SFTP）
    ReadFile     { path: String, target_pane: usize },
    WriteFile    { path: String, content: String, target_pane: usize },
    ListDir      { path: String, target_pane: usize },
    Search       { pattern: String, path: String, target_pane: usize },
    FileStat     { path: String, target_pane: usize },

    // 命令执行
    RunCommand   { command: String, cwd: Option<String>, target_pane: usize },

    // 终端
    ReadTerminal { lines: Option<usize>, target_pane: usize },

    // Tab 环境
    ListPanes    {},   // 列出 Tab 内所有 Pane 及连接信息
    GetEnv       { target_pane: usize },  // cwd, host, user, os

    // 项目记忆
    ReadMemory   {},   // 读 .hetu/ai-memory.md
    WriteMemory  { content: String },
}
```

### 4.3 连接路由逻辑

```
target_pane → TabManager.getPane(tab_id, target_pane)
  ├── 本地 Pane → 本地文件系统 / PTY exec
  └── SSH Pane  → 通过 conn_id 走 SFTP / SSH exec channel
                   （复用已有连接，无需重建认证）
```

### 4.4 OS 感知

`GetEnv` 返回当前 Pane 的 OS 信息，Agent 据此调整命令语法：
- 本地 Linux → 标准 bash 命令
- 远程 Linux → 标准 bash 命令（通过 SSH）
- 未来扩展：远程 macOS / WSL

### 4.5 `read_terminal` 数据流

此工具特殊——xterm buffer 在前端，工具执行在 Rust 后端，需要前后端往返：

```
Rust: Tool::execute(ReadTerminal { target_pane, lines })
  → emit("ai_read_terminal", { session_id, pane_id, lines })
  → 前端监听事件，读取 term.buffer.active
  → 前端 invoke("ai_terminal_data", { session_id, text })
  → Rust 收到文本，作为 ToolResult 返回
```

实现要点：
- Rust 端用 `oneshot::channel` 等待前端回调，设超时（5s）
- 前端读取 buffer 时按行截取（`lines` 参数控制行数，默认 100 行）
- 读取的是**可见行**（viewport），非整个 scrollback（避免巨量文本）

### 4.6 工具输出截断策略

工具返回的内容可能极大（如 `read_file` 读取 5000 行代码），需要在返回给 LLM 前截断：

| 工具 | 截断策略 | 上限 |
|------|----------|------|
| `read_file` | 保留前 N 行 + `... (truncated M lines) ...` + 后 50 行 | 500 行 |
| `list_dir` | 按名称排序截取前 N 项 | 200 项 |
| `search` | 截取前 N 条匹配 | 100 条 |
| `run_command` | 保留 stdout/stderr 末尾 N 行（命令输出通常尾部最重要） | 300 行 |
| `read_terminal` | 按 `lines` 参数，默认 100 行 | 200 行 |

截断后 `ToolResult.truncated = true`，Agent 据此判断是否需要分段读取或缩小范围。

---

## 5. Provider 抽象层

### 5.1 LlmProvider trait

OpenAI 和 Anthropic 的 tool calling 格式差异巨大，Phase 1 必须定义 trait，避免后期重构 ReAct 循环：

| 维度 | OpenAI | Anthropic |
|------|--------|-----------|
| 系统提示词 | `messages[0]` role=system | 独立 `system` 字段 |
| 工具调用 | `tool_calls[]` + role=tool | `content[]` type=tool_use + tool_result |
| 流式格式 | `delta.tool_calls` | `content_block_delta` |
| 工具定义 | `tools[]` JSON Schema | `tools[]` input_schema（几乎相同） |

```rust
#[async_trait]
trait LlmProvider {
    /// 发送对话请求，流式返回事件
    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        system_prompt: &str,
        tx: &Channel<AgentEvent>,
    ) -> Result<LlmResponse>;

    /// 估算 token 数（用于上下文窗口管理）
    fn estimate_tokens(&self, messages: &[Message]) -> usize;
}

enum LlmResponse {
    Text(String),
    ToolCall { name: String, args: Value },
    Question { question: String, choices: Vec<UserChoice> },
    Done,
}
```

### 5.2 实装计划

- **Phase 1**：`OpenAiProvider` 实装（覆盖 DeepSeek/OpenAI/其他兼容服务）
- **Phase 4**：`AnthropicProvider` 实装（Claude native API）
- ReAct 循环只依赖 `LlmProvider` trait，切换 Provider 零改动

---

## 6. 上下文窗口管理

### 6.1 问题

Agent 多轮工具调用后，对话历史迅速膨胀。一个 `read_file` 可能就几万 token，几轮后即超出模型 `max_tokens` 限制。

### 6.2 策略

每次调 LLM 前检查 token 数，超限时执行截断：

```
trim_history(history, max_tokens):
  while estimate_tokens(history) > max_tokens * 0.8:
    1. 优先移除最早的 ToolCall + ToolResult 对（体积最大）
    2. 保留：系统提示词 + 首条用户消息 + 最近 N 轮对话
    3. 被移除的工具调用替换为摘要：
       "[tool: read_file('src/main.rs') — truncated, 500 lines]"
    4. 若仍超限，移除最早的普通对话消息
```

### 6.3 参数

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `context_window` | 模型 max_tokens 的 80% | 截断触发阈值 |
| `min_recent_turns` | 5 | 至少保留最近 5 轮对话 |
| `tool_output_summary` | `[tool: {name}({args}) — truncated]` | 被移除工具调用的占位 |

---

## 7. 错误处理与重试

### 7.1 LLM API 错误

| 错误类型 | 策略 |
|----------|------|
| 429 限流 | 自动切换到同模型的下一个 endpoint key（负载均衡核心价值），重试请求 |
| 5xx 服务端错误 | 等待 1s/2s/4s 指数退避，最多重试 3 次 |
| 网络超时 | 30s 超时，重试 2 次 |
| 401 密钥无效 | 不重试，直接报错 `Error` 事件，提示用户检查密钥 |
| 所有 endpoint 均失败 | `Error` 事件，Agent 停止 |

### 7.2 工具执行错误

工具执行失败不终止 Agent——将错误作为 `ToolResult::Error` 返回给 LLM，Agent 自行决定是否重试或换方案（ReAct 循环的天然优势）：

```
Tool::execute() → ToolResult::Error { message: "Permission denied: /root/.ssh" }
  → 追加到对话历史 → LLM 下一步可能：
    - 换路径重试
    - 告知用户需要权限
    - 放弃此方案
```

### 7.3 AgentEvent 补充

```rust
enum AgentEvent {
    // ... 已有事件 ...

    // 重试通知（可选展示）
    Retrying { reason: String, attempt: usize, max_attempts: usize },

    // 上下文截断通知（可选展示）
    ContextTrimmed { removed_tools: usize, removed_messages: usize },
}
```

---

## 8. 并发控制

### 8.1 Session 内消息串行

同一 Session 内，Agent 正在 ReAct 循环中时用户发送的新消息排队，不并发请求 LLM：

```
Session 状态机：
  Idle  → 收到消息 → Processing → LLM+工具循环 → Idle
  Processing → 收到消息 → 排队（Queue），当前轮完成后处理
  Processing → abort() → 设置 aborted 标志 → 循环检查点退出 → Idle
```

### 8.2 中止检查点

`abort()` 不强制中断，而是在 ReAct 循环的检查点退出：

| 检查点 | 说明 |
|--------|------|
| 每次 LLM 请求前 | 如果 `aborted == true`，不发送请求，直接退出 |
| 每次工具执行前 | 同上 |
| LLM 流式读取中 | `aborted` 时丢弃后续 chunk，退出循环 |

退出后推送 `AgentEvent::Aborted`，Session 回到 `Idle`，可接受新消息。

### 8.3 跨 Session 独立

不同 Tab 的 Session 完全独立，各自一个 tokio task，互不阻塞。

---

## 9. 流式协议（Frontend ↔ Backend）

Tauri Channel 推送结构化事件：

```rust
enum AgentEvent {
    // 助手消息（流式增量）
    Message { content: String, done: bool },

    // 思考过程（Claude thinking / o1，可折叠展示）
    Thinking { content: String },

    // 工具调用生命周期
    ToolStart  { tool: String, args: Value, target_pane: usize },
    ToolOutput { output: String },              // stdout / stderr 实时行
    ToolEnd    { result: ToolResult },          // 最终结果

    // Ask 模式专用
    AskApproval { tool: String, args: Value, target_pane: usize, reason: String },

    // Plan 模式专用
    ProposedPlan { steps: Vec<String>, summary: String },

    // Agent 向用户提问（任务中遇到歧义，需要用户抉择）
    UserQuestion { question: String, choices: Vec<UserChoice> },

    // 生命周期
    Error { message: String },
    Aborted,
    Done,
}

enum ToolResult {
    Success { output: String, truncated: bool },
    Error   { message: String },
    UserRejected,
}

struct UserChoice {
    label: String,       // "RAII封装"
    description: String, // "用 Drop trait 保证 unsafe 内存自动释放"
    action: String,      // 回传给 Agent 的标识，如 "raii"
}
```

### 9.1 Agent 提问流程

与 Ask 模式不同——Ask 是审批工具调用，提问是 Agent 在推理过程中遇到**需要用户决策的岔路口**。ReAct 循环中的处理：

```
ReAct loop:
  response = LLM.chat(messages, tools)
  match response {
    TextChunk  → channel.push(Message(chunk))
    ToolCall   → (按模式处理)
    Question   → channel.push(UserQuestion { ... })
                 pause tokio task
                 wait for frontend to call answer_question(session_id, choice)
                 将用户选择作为 user message 追加到对话历史
                 continue loop  // Agent 基于用户选择继续推理
  }
```

前端渲染为内联选择卡片：

```
┌─ ❓ 建议两种方案 ────────────────────────────────────────────┐
│                                                               │
│  ● A) 用 RAII 封装 unsafe 块，引入 Drop 保证释放               │
│     安全、符合 Rust 惯用法，改动范围小                          │
│                                                               │
│  ○ B) 切换到标准库的 alloc::Allocator trait                    │
│     更底层灵活，但需要较大重构                                  │
│                                                               │
│  [选 A]  [选 B]  [自由输入…]                                   │
└───────────────────────────────────────────────────────────────┘
```

Agent 可以在系统提示词中被指示：遇到方向性歧义时主动向用户提问，附带选项和说明。

---

## 10. UI 设计

### 10.1 Agent Modal 整体布局

默认尺寸与 himage 一致：`80vw × 80vh`，居中。支持 **最大化/还原** 切换（左上角按钮，与 himage 逻辑一致）。支持响应式。

**Modal 内部为多视图结构**——设置、角色选择等均在同一面板内切换，不弹出独立子对话框：

```
┌─ Hai ───────── [Chat] [设置] [⏸] [✕] ── [Auto ▾] ────────────┐
├─ 工具栏 ─────────────────────────────────────────────────────────┤
│ [通用助手 ▾]  [📎 附加选中]  [🧠 记忆]  [🔲 玻璃模式]          │
├─ 消息区 ─────────────────────────────────────────────────────────┤
│  ┌─── Chat ───────────────────────────────────────────────────┐  │
│  │                                                              │  │
│  │  👤 帮我检查当前目录下的代码有没有内存泄漏                   │  │
│  │                                                              │  │
│  │  🤖 让我先了解项目结构                                       │  │
│  │  ├→ list_dir(...) ── [Pane: 本地] ✓                        │  │
│  │  ├→ search(...)    ── [Pane: 本地] ✓                       │  │
│  │  ├→ read_file(...) ── [Pane: 本地] ✓                       │  │
│  │                                                              │  │
│  │  🤖 发现两处潜在问题：                                       │  │
│  │  1. allocator.rs:42 — unsafe 未配对 free                    │  │
│  │  2. cache.rs:18 — Arc 循环引用                              │  │
│  │  ─────────────────────────────────────                      │  │
│  │  ❓ 建议两种方案：                                           │  │
│  │  A) 用 RAII 封装 unsafe 块，引入 Drop 保证释放              │  │
│  │  B) 切换到标准库的 alloc::Allocator trait                   │  │
│  │                                                              │  │
│  │  [选 A — RAII封装]  [选 B — allocator]  [我自己改]          │  │
│  │                                                              │  │
│  └──────────────────────────────────────────────────────────────┘  │
├─ 输入区 ─────────────────────────────────────────────────────────┤
│ [Pane: 本地 ▲]  [输入消息____________________________]  [⏎]     │
├─ 状态栏 ─────────────────────────────────────────────────────────┤
│ ● DeepSeek V3  │  本轮 3 工具  │  Auto    │  🔲 Glass             │
└──────────────────────────────────────────────────────────────────┘
```

**视图切换：** 顶部标签栏 `[Chat] [设置] [角色]` 切换内容区：
- **Chat**：消息流 + 输入框（默认视图）
- **设置**：模型/密钥/负载均衡/执行策略/主题/玻璃模式（内嵌表单）
- **角色**：提示词模板浏览/选择/编辑

切换视图时 Agent 会话保持运行（后台继续或暂停），返回 Chat 视图无缝恢复。

**尺寸与生命周期：**

| 属性 | 行为 |
|------|------|
| 默认尺寸 | `80vw × 80vh`（与 himage 一致） |
| 最大化 | 左上角按钮切换，覆盖全视口 |
| 最小宽度 | `480px`（响应式下限，低于此宽度工具栏折叠） |
| Tab 切换 | Modal 保持打开但挂起到对应 Tab，切换回 Tab 时恢复 |
| Tab 关闭 | Modal 随 Tab 销毁，Agent 会话终止 |
| ESC | 关闭 Modal（不销毁会话，再次 `hai` 恢复） |

**响应式策略：**

```
> 900px          标准布局：工具栏展开，消息区全宽
480–900px        紧凑布局：工具栏折叠为图标，消息区缩减 padding
< 480px          不适用（终端窗口本身不建议低于此尺寸）
```

实现上复用 himage 的模态容器基类（`showImageViewer` 的弹窗逻辑），仅替换内容区域。

### 10.2 Ask 模式确认弹窗

```
┌─ ⚠ 工具调用确认 ────────────────────────────┐
│                                               │
│  Agent 想执行：                               │
│                                               │
│  🛠 run_command                               │
│     command: rm -rf node_modules              │
│     cwd:     /home/wy/project                 │
│     目标:    Pane: 本地                       │
│     原因:    清理依赖目录以重装               │
│                                               │
│  [批准 ✔]  [拒绝 ✘]  [编辑命令…]             │
└───────────────────────────────────────────────┘
```

### 10.3 Plan 模式

```
┌─ 📋 执行计划 ────────────────────────────────────┐
│                                                    │
│  1. list_dir     — 了解项目结构                    │
│  2. search       — 查找 unsafe 代码块              │
│  3. read_file    — 读取可疑文件                    │
│  4. run_command  — 运行测试                        │
│                                                    │
│  预计涉及 2 个 Pane（本地、host-A）                │
│                                                    │
│  [确认执行 ▸]  [修改计划]  [取消]                  │
└────────────────────────────────────────────────────┘
```

### 10.4 主题

HAI 面板默认**继承终端配色**，保持视觉一致。同时允许独立切换（比如终端暗色、面板亮色）。

| 属性 | 行为 |
|------|------|
| 默认主题 | 跟随终端活跃主题（`activeTheme()`） |
| 独立切换 | 在 Modal 工具栏或设置面板中选择亮/暗主题，仅作用于该面板 |
| 透明度/模糊 | 复用 himage 的 backdrop-filter 逻辑：毛玻璃 + 模糊度 + 磨砂（继承 `opacity` / `blur` / `blurAmount` / `frosted` / `frostStrength`） |
| 圆角 | 继承 `cornerRadius` |

实现上：AI Modal 与 himage 共享同一个弹窗容器基类/样式，主题切换通过 CSS 变量注入：

```css
.hai-modal {
  /* 默认继承终端 */
  --hai-bg: var(--xterm-bg);
  --hai-fg: var(--xterm-fg);
  --hai-accent: var(--xterm-blue);
  /* ... */
}

.hai-modal[data-theme="light"] {
  /* 独立亮色覆盖 */
  --hai-bg: #f5f5f5;
  --hai-fg: #1a1a1a;
  /* ... */
}
```

### 10.5 智能玻璃模式

独立于主题选择的**视觉模式**。高透明度 + 强模糊，终端内容透过后若隐若现，UI 组件如悬浮于玻璃之上。

```
┌─ 终端（Tab 背景）─────────────────────────────────────────────┐
│                                                                │
│  $ ls -la                  透过玻璃隐隐可见                     │
│  $ cargo build             模糊后的终端内容                     │
│                                                                │
│  ┌─ HAI 玻璃层 ────────────────────────────────────────────┐  │
│  │                                                          │  │
│  │  ┌─ 消息卡片 ──────────────────────────────────────┐   │  │
│  │  │  半透明底 + 微边框，悬浮感                       │   │  │
│  │  └─────────────────────────────────────────────────┘   │  │
│  │                                                          │  │
│  │  ┌─ 输入栏 ────────────────────────────────────────┐   │  │
│  │  │  半透明，底部固定                                 │   │  │
│  │  └─────────────────────────────────────────────────┘   │  │
│  └──────────────────────────────────────────────────────────┘  │
└────────────────────────────────────────────────────────────────┘
```

| 属性 | 常规模式 | 玻璃模式 |
|------|----------|----------|
| 面板背景 | 跟随终端主题 + 不透明度 | 极高透明度（~15%），强模糊（`blur(24px)`） |
| 消息卡片 | 与背景同色系 | 独立半透明卡片（`rgba` + 微边框 `1px solid`），悬浮感 |
| 工具栏/输入栏 | 与消息区一体 | 独立半透明条，附着底部/顶部 |
| 终端可见性 | 基本遮挡 | 透过面板可见终端内容（强模糊后不可读，仅视觉层次） |
| 切换方式 | — | 工具栏按钮或设置面板开关 |

**CSS 实现骨架：**

```css
.hai-modal[data-glass="true"] {
  /* 面板容器：极透 + 强模糊 */
  background: rgba(var(--hai-bg-rgb), 0.12);
  backdrop-filter: blur(24px) saturate(1.2);
  -webkit-backdrop-filter: blur(24px) saturate(1.2);

  /* 消息卡片：独立悬浮 */
  .hai-message {
    background: rgba(var(--hai-bg-rgb), 0.55);
    border: 1px solid rgba(var(--hai-fg-rgb), 0.08);
    border-radius: 8px;
    box-shadow: 0 1px 3px rgba(0,0,0,0.06);
  }

  /* 工具栏/输入栏：底部附着条 */
  .hai-toolbar,
  .hai-input-bar {
    background: rgba(var(--hai-bg-rgb), 0.65);
    backdrop-filter: blur(12px);
    border-color: rgba(var(--hai-fg-rgb), 0.06);
  }
}
```

颜色变量需提供 RGB 分量形式（`--hai-bg-rgb: 30, 30, 30`），用于 `rgba()` 组合透明度。

---

## 11. 命令设计

```
hai                     → 以默认角色（通用助手）启动，Auto 模式
hai [role]              → 以指定角色启动
hai --ask               → 以 Ask 模式启动
hai --ask [role]        → 指定角色 + Ask 模式
hai "这是消息"           → 直接带首条消息启动
```

Shell 脚本（`local.rs` 中注入，类似 hssh）：

```sh
#!/bin/sh
# hai — HetuShell AI Agent
OSC=1733
emit() { printf '\033]%s;%s\007' "$OSC" "$1"; }

role="general"
mode="auto"
message=""

while [ $# -gt 0 ]; do
  case "$1" in
    --ask)    mode="ask"; shift;;
    --plan)   mode="plan"; shift;;
    --role)   role="$2"; shift 2;;
    -h|--help) echo "hai [--ask|--plan] [--role <name>] [message]"; exit 0;;
    *)        message="$*"; break;;
  esac
done

tok=$(echo -n "${HSSH_TOKEN:-}" | base64 -w0)
role64=$(echo -n "$role" | base64 -w0)
mode64=$(echo -n "$mode" | base64 -w0)
msg64=$(echo -n "$message" | base64 -w0)
emit "tok=$tok;op=launch;role=$role64;mode=$mode64;msg=$msg64"
```

---

## 12. 配置文件

AI 配置独立于 HetuShell 主配置，存储于 `~/.config/hetushell/`（Tauri `app_data_dir`）：

```
~/.config/hetushell/
├── settings.json          ← HetuShell 主配置（字体/主题/布局等，不含 AI 配置）
├── profiles.json          ← SSH 连接项
└── ai-config.json         ← AI Agent 全部配置（模型/密钥/执行策略/UI 偏好）
```

单文件，不拆分密钥——`ai-config.json` 本身就在用户配置目录中，不进入项目仓库，无需额外的 `key_id` 间接引用。

AI 配置有**独立的设置面板**，不嵌入 HetuShell 主设置对话框。入口：
- Agent Modal 工具栏 **⚙ 设置** 按钮
- 未来可加菜单项（`帮助 → AI 设置`）

`ai-config.json` 内容：

```json
{
  "providers": {
    "openai": {
      "deepseek-v3": [
        { "url": "https://api.deepseek.com/v1", "key": "sk-xxx", "maxTokens": 8192, "temperature": 0.7 },
        { "url": "https://api.deepseek.com/v1", "key": "sk-yyy" }
      ],
      "glm-5.2": [
        { "url": "https://open.bigmodel.cn/api/paas/v4", "key": "xxx", "maxTokens": 8192 }
      ]
    },
    "anthropic": {
      "claude-4-sonnet": [
        { "key": "sk-ant-zzz", "maxTokens": 8192 }
      ]
    }
  },
  "default_model": "deepseek-v3",
  "default_provider": "openai",
  "execution": {
    "default_mode": "auto",
    "dangerous_commands": ["rm -rf", "dd", "shutdown", "reboot", "mkfs", ":(){ :|:& };:"],
    "always_ask_for": ["run_command"]
  },
  "roles": {
    "general":       { "model": "deepseek-v3", "provider": "openai" },
    "code-review":   { "model": "claude-4-sonnet", "provider": "anthropic" },
    "shell-expert":  { "model": "deepseek-v3", "provider": "openai" },
    "debugger":      { "model": "deepseek-v3", "provider": "openai" }
  },
  "extensions": {
    "mcp": [
      {
        "name": "github",
        "enabled": true,
        "command": "npx",
        "args": ["-y", "@modelcontextprotocol/server-github"],
        "env": { "GITHUB_TOKEN": "ghp_xxx" }
      }
    ]
  }
}
```

**结构说明：**

```
providers
  ├── openai                    ← Provider 类型（决定 API 协议格式）
  │   ├── deepseek-v3           ← 模型 ID（LLM 实际接收的 model 参数）
  │   │   ├── [0] { url, key }  ← 第一个 endpoint（round-robin 轮转）
  │   │   └── [1] { url, key }  ← 第二个 endpoint（同 URL 不同 key，或不同 URL 均可）
  │   └── glm-5.2
  │       └── [0] { url, key }
  └── anthropic
      └── claude-4-sonnet
          └── [0] { key }       ← anthropic 的 url 固定，可省略

roles（高级设置）
  ├── general       → { model, provider }  ← 不指定则用 default_model + default_provider
  ├── code-review   → { model, provider }  ← 推荐用 Claude（推理强）
  └── ...

extensions（高级设置）
  └── mcp[]          ← MCP Server 列表，stdio 协议
      └── { name, enabled, command, args, env }
```

**设计要点：**

| 规则 | 说明 |
|------|------|
| 同模型 ID 多个 entry | 即为负载均衡池，round-robin 轮转请求 |
| `maxTokens` / `temperature` | 第一个 entry 上定义即可，同模型共享；后续 entry 只需 `url` + `key` |
| `url` | openai 类型必填（各厂商 API 地址不同）；anthropic 类型可省略（固定 `api.anthropic.com`） |
| `default_model` + `default_provider` | 定位默认模型：`providers[default_provider][default_model]` |
| `roles.<role>.model/provider` | 角色绑定模型（高级设置），不指定则回退到 default |
| 429 限流 | 自动跳到数组中下一个 endpoint 重试，全部失败才报错 |
| 密钥安全 | 文件在 `~/.config/hetushell/` 下，不进入任何 git 仓库 |

### 角色绑定模型

每个角色可独立指定 `provider + model`，不填则用 `default_provider + default_model`。设置面板中给出建议：

| 角色 | 推荐模型类型 | 理由 |
|------|-------------|------|
| general（通用助手） | 通用能力强、性价比高 | DeepSeek V3 / GPT-4o |
| code-review（代码审查） | 推理深度强 | Claude Sonnet / DeepSeek R1 |
| shell-expert（运维） | 响应快、指令准确 | DeepSeek V3 / GPT-4o-mini |
| debugger（调试） | 长上下文 + 推理 | Claude Sonnet / GPT-4o |

用户可自由覆盖，建议仅在设置面板中作为 placeholder/提示文字展示，不强制。

### 扩展机制

支持 MCP（Model Context Protocol）Server 作为外部工具扩展：

```
Agent ReAct 循环
  ├── 内建工具（read_file, run_command, ...）
  └── MCP 工具（由 MCP Server 通过 stdio 提供）
      ├── MCP Server 启动（command + args + env）
      ├── tools/list → 注册到 Agent 工具表
      ├── tools/call → Agent 调用时转发
      └── Agent 生命周期结束 → Server 进程关闭
```

MCP Server 以子进程方式启动（stdio 通信），生命周期跟随 Agent Session：

| 阶段 | 行为 |
|------|------|
| Session 创建 | 遍历 `extensions.mcp[]`，`enabled: true` 的启动子进程，调用 `tools/list` 注册工具 |
| ReAct 循环 | MCP 工具与内建工具同等对待，LLM 可调用 |
| Session 销毁 | 关闭所有 MCP 子进程 |

MCP 工具在 `ToolStart`/`ToolOutput`/`ToolEnd` 事件中与内建工具统一展示，前端无需区分。

未来扩展点（预留，不在当前 Phase 范围内）：
- `extensions.custom_agents[]` — 自定义 Agent（独立提示词 + 工具子集 + 模型）

---

## 13. 文件结构

```
src/ai/
├── agent-modal.ts         ← AgentModal 组件（消息渲染 + 工具可视化 + 模式切换）
├── renderer.ts            ← Markdown 渲染器（代码高亮、Diff、图片、ANSI）
├── session.ts             ← AgentSession（前端状态管理，消息历史）
├── protocol.ts            ← AgentEvent / ToolResult / ExecutionMode 类型
├── config.ts              ← 模型配置 + key 管理 + 负载均衡设置
├── settings-panel.ts      ← AI 设置（内嵌于 Modal 视图，非独立对话框）
├── roles/                 ← 系统提示词模板
│   ├── general.md         ← 默认通用助手
│   ├── code-review.md     ← 代码审查
│   ├── shell-expert.md    ← Shell 命令行 / 运维
│   └── debugger.md        ← 调试分析
└── hai.ts                 ← OSC 1733 解析 + Session 入口

src-tauri/src/agent/
├── mod.rs                 ← AgentManager + Tauri commands
├── loop.rs                ← ReAct 循环（per-session tokio task）
├── tools.rs               ← Tool trait + 工具注册表 + 本地实现
├── tools_ssh.rs           ← SSH 远程工具实现（规划，后续实现）
├── mcp.rs                 ← MCP Server 管理（子进程 + stdio + tools/list|call）
├── provider.rs            ← LLM API 代理 + SSE → Channel 中继
├── provider_anthropic.rs  ← Anthropic API 适配
├── session.rs             ← Session 状态机
├── config.rs              ← ai-config.json 存取 + 角色模型绑定
└── types.rs               ← 共享类型（AgentEvent, ToolResult 等）

src-tauri/src/
└── local.rs               ← +hai shell 脚本

src/
└── main.ts                ← +Tab 生命周期管理（挂载/卸载 AgentSession）
└── pane.ts                ← +OSC 1733 handler
```

---

## 14. 内容渲染

LLM 返回的主体内容是 Markdown，在前端 WebView 中渲染为富文本。

### 14.1 渲染能力矩阵

| 内容类型 | 渲染方式 | 交互 |
|----------|----------|------|
| 正文 Markdown | 标准渲染（标题、列表、表格、引用、加粗、斜体、链接） | 链接可点击（浏览器打开） |
| 代码块 | 语法高亮（按语言标识） | 一键复制按钮、折叠/展开 |
| Diff 块 | 统一 diff 视图（+/- 着色），或并排 diff | 内联展示 |
| 终端输出 | ANSI 转义序列 → 彩色 HTML | 与终端一致的颜色呈现 |
| 静态图片 | `<img>` 内联渲染 | 点击放大（复用 himage 查看器） |
| SVG / Mermaid 图 | 内联渲染（Mermaid 客户端渲染） | 缩放/全屏 |
| 文件路径 | 自动识别并高亮 | 点击 → 在文件管理器面板打开 |
| 数学公式 | KaTeX 渲染（可选） | — |

### 14.2 渲染器选型

```
渲染栈：
  marked          ← Markdown → HTML（轻量，无额外依赖）
  highlight.js    ← 代码语法高亮（按语言自动检测）
  diff2html       ← Diff 统一视图（GitHub 风格）
  ansi-to-html    ← ANSI 转义序列 → 彩色 HTML
  mermaid         ← Mermaid 图表客户端渲染（可选，按需加载）
  KaTeX           ← 数学公式（可选，按需加载）
```

均为纯前端库，无新增 Tauri/系统依赖。全部 tree-shakable，不加载即不打包。

### 14.3 渲染分块策略

Agent 的 `Message` 事件是流式到达的，不能等全部到达再渲染（SSE 的 chunk 粒度可能是几个 token）。策略：

```
chunk1: "让我分析一下...\n\n```"     → 渲染 "让我分析一下..." + 开启代码块占位
chunk2: "rust\nfn main()"           → 代码块内累积
chunk3: " {\n    println!"           → 继续累积
chunk4: "(\"hello\");\n}\n```\n"    → 关闭代码块，触发语法高亮
```

实现上维护一个 **BlockParser** 状态机：
- `text` → Markdown 分段渲染
- `fence_pending` → 等待代码块语言标识 + 内容 + 闭合
- `diff_pending` → 等待 diff 块结束
- 块闭合时一次性生成带高亮的 HTML 替换占位元素

### 14.4 工具输出的多媒体化

Agent 调用工具后返回的结果也需要富展示：

| 工具 | 输出 | 渲染 |
|------|------|------|
| `list_dir` | 文件列表 JSON | 表格（名称、大小、时间），目录/文件图标区分 |
| `search` | grep 结果 | 匹配行 + 文件名 + 行号 + 高亮匹配词，点击跳转文件 |
| `read_file` | 文件内容 | 语法高亮代码块 + 文件名 + 行号 |
| `run_command` | stdout/stderr | ANSI 彩色终端输出 |
| `read_terminal` | 终端 buffer | ANSI 彩色终端输出（保留原汁原味） |

### 14.5 图片 / 图表工作流

LLM 可能通过两种方式产出图片：

1. **生成代码后本地渲染** — Agent 调用 `write_file("chart.py", code)` → `run_command("python chart.py")` → 生成 `output.png` → Agent 告知前端路径 → 前端用 himage 查看器展示
2. **返回 Mermaid/SVG 源码** → 前端直接客户端渲染（无需安装 Graphviz/PlantUML）

---

## 15. 分阶段实现路径

### Phase 1a — 骨架验证（最快打通端到端）

- [ ] `hai` 命令 + OSC 1733 + HSSH_TOKEN 校验
- [ ] Rust：添加 `reqwest`（HTTP/2 + SSE + rustls）
- [ ] `LlmProvider` trait 定义 + `OpenAiProvider` 实装
- [ ] `ai-config.json` / `ai-keys.json` 读写
- [ ] 基本 Modal（能显示流式消息，无工具）
- [ ] 单轮对话（无 ReAct，无工具调用）
- [ ] Tab 级 Session 绑定 + 生命周期

### Phase 1b — Agent 能力

- [ ] ReAct 循环 + tool calling
- [ ] 本地工具：read_file / write_file / list_dir / run_command
- [ ] 工具输出截断策略
- [ ] Auto 模式
- [ ] 中止机制（abort 检查点）
- [ ] 并发控制（消息串行排队）
- [ ] Markdown 流式渲染（marked + highlight.js）
- [ ] 错误处理与重试（429 切换 key、超时重试）

### Phase 1c — 配置完善

- [ ] AI 设置面板（内嵌于 Modal 视图）
- [ ] 负载均衡（round-robin + 429 自动切换）
- [ ] 上下文窗口管理（trim_history）
- [ ] 智能玻璃模式
- [ ] 主题独立切换

### Phase 2 — 跨 Pane + 模式

- [ ] list_panes 工具 + Pane 选择器 UI
- [ ] 跨 Pane 工具路由
- [ ] Ask 模式（工具确认）
- [ ] Plan 模式（先计划后执行）
- [ ] Agent 提问机制（UserQuestion + answer_question）
- [ ] read_terminal 工具（读 xterm buffer，前后端往返）

### Phase 3 — SSH 远程

- [ ] 工具通过 SSH conn_id 路由到远程
- [ ] SFTP 文件操作
- [ ] SSH exec 命令执行
- [ ] 远程 Pane 的 cwd/env 感知

### Phase 4 — 完善

- [ ] 项目记忆（.hetu/ai-memory.md）
- [ ] Anthropic provider 适配（`AnthropicProvider` 实装）
- [ ] 角色模板系统 + 角色绑定模型
- [ ] MCP Server 支持（子进程 + stdio + tools/list|call）
- [ ] 多模型运行时切换
- [ ] 历史对话持久化（session recovery）

---

## 16. 安全边界

### 16.1 强制性检查（Rust 层硬拦截）

| 检查项 | 实现 |
|--------|------|
| 文件操作路径限制 | `read_file` / `write_file` / `list_dir` 做 path canonicalization，限制在 target pane 的 cwd 子树内（可配置解除） |
| 危险命令拦截 | `run_command` 对 `dangerous_commands` 列表做 pattern match，命中则拒绝执行或强制走 Ask 模式 |
| HSSH_TOKEN 校验 | OSC 1733 载荷中的 token 必须与 pane 注入的一致 |
| API 密钥隔离 | 密钥在 `ai-config.json` 中，与 `settings.json` 分离，不进入 git |

### 16.2 提示性约束（系统提示词层面）

| 约束 | 说明 |
|------|------|
| 工具能力范围 | Agent 工具限定在当前 Tab 的 Pane 连接范围内 |
| `run_command` 可绕过文件限制 | Agent 可以 `cd / && cat /etc/passwd`，这是命令执行的灵活性需求——通过危险命令列表 + Ask 模式平衡，不做硬阻断 |
| 操作告知 | Agent 在执行重要操作前应在消息中说明意图 |
