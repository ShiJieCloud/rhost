# AI ReAct 循环设计方案

> description: 新增 read_file/list_directory/get_system_info 三个只读工具，经 SSH exec channel 执行并自动回传结果，闭合「观察→推理→行动」循环
>
> created: 2026-10-12 14:52:37
>
> updated: 2026-10-12 15:31:00
>
> author:

---

## 1. 设计目标与硬约束

> 范围：在既有 `propose_command` 工具提案通道之上，新增只读观察工具，使模型能在提案命令前主动读取远程文件、目录与系统信息，形成 ReAct（Reasoning + Acting）闭环。只读工具经 SSH exec channel 执行（非 PTY），自动返回输出，无需用户授权。
> 边界：不覆盖 Plan-and-Act / 多步推理链显式编排；不覆盖工具输出的流式推送（exec 通道一次性返回）；不覆盖自定义工具定义（后端内置常量）；不覆盖首页全局对话（只读工具需 SSH 会话目标）。
> 关联代码：`src-tauri/src/ai.rs`、`src-tauri/src/ssh/session.rs`、`src-tauri/src/ssh/manager.rs`、`frontend/src/components/AiChatPanel.vue`、`frontend/src/lib/ai.ts`

| 目标 | 量化口径 |
|---|---|
| 观察覆盖 | 模型可读文件内容（≤8KB）、列目录、取系统快照，覆盖 ≥80% 常见排障前置探索场景 |
| 零授权开销 | 只读工具 100% 自动执行，无弹窗、无确认交互 |
| 输出有界 | 单工具输出 ≤8KB UTF-8 字节，超出截断标注；一轮 ReAct 总输出 ≤32KB（4 工具 × 8KB） |
| 执行低延迟 | exec channel 命令 ≤10s 超时，不阻塞对话主流程超过 10s |
| 轮次可控 | 单次用户输入最多 6 轮工具循环（覆盖 3~4 次只读探索 + 2~3 次命令执行），超出终止续跑 |
| 终端零污染 | 只读工具 0 字节写入 PTY，用户终端不出现探索命令刷屏 |

必须遵守的既有项目约束：

- 后端 AI 模块保持无会话状态：`ai.rs` 命令均为一次性调用，不缓存会话级数据；
- 工具定义后端内置：`builtin_tools()` 返回 JSON 常量，前端不传工具 schema（`ai-terminal-exec-design.md` §6.2 既有裁决）；
- `enableTools` 启用条件：工作台会话 scope（`scope !== 'global'`），首页对话不附加工具；
- 终端内容不会以任何自动形式离开本机：只读工具是**模型主动发起**的 exec channel 查询，不是终端输出自动读取；PTY 字节流零暴露；
- `exec_collect` 已有并发闸门（`METRIC_EXEC_GATE`）与超时机制，复用不另建基础设施；
- `SessionManager` 按 `session_id` 管理 `Arc<SshSession>` 句柄，exec channel 与 PTY 共享同一 SSH 连接。

## 2. 必须遵守的既有项目约束

- **exec channel 已存在**：`ssh/session.rs` `exec_collect(script_body, timeout)` 已用于 MOTD 采集与指标采集，包含单引号转义、`channel_open_session` + `ch.exec` + `ChannelMsg::Data/ExtendedData` 捕获、UTF-8 容错、`METRIC_EXEC_GATE` 并发闸门、超时 drop 关闭通道。ReAct 只读工具复用此方法，但需升级签名以捕获 `ChannelMsg::ExitStatus`（当前在 `_ => {}` 中被丢弃），详见 §6.2.1；
- **SessionManager 查找模式**：`write`/`resize` 等方法均采用 `sessions.read().await → get(session_id).cloned() → 调用 SshSession 方法` 模式，exec collect 沿用同一模式；
- **工具循环前端驱动**：后端 `finish_stream` 发 `ai-tool-calls` 事件后即返回，不挂起等待结果；前端持久化提案、执行、收尾、续跑全链路已稳定（`pumpProposals` / `settleProposalRound` / `continueToolRound`），只读工具接入同一循环；
- **AiToolProposal 已有多工具承载能力**：`name` 字段区分工具，`args` 字段存原始 JSON，`execOutput` 字段存执行输出，`state` 字段标记裁决状态——只读工具复用此结构，`name` 非 `propose_command` 即为只读；
- **shell 命令构造在后端**：只读工具的 shell 命令由后端根据 `tool_name` + `args` JSON 构造，前端不拼命令（安全边界 + 单一来源）；
- **权限档与豁免表仅作用于 propose_command**：只读工具不经授权弹窗，不受 `execPermission` / `sessionAllowlist` 影响。

## 3. 现状盘点与差距

| 位置 | 现状 | 差距 |
|---|---|---|
| `ai.rs` `builtin_tools()` | 仅 `propose_command` 一个工具 | 需新增 `read_file` / `list_directory` / `get_system_info` |
| `AiChatPanel.vue` `onToolCalls` | 解析 `command` 参数 → 授权/豁免/自动执行分流 | 需按 `name` 路由：只读工具走自动执行路径 |
| `AiChatPanel.vue` `pumpProposals` | pending 提案全部入队弹窗 | 需过滤只读工具（已在 `autoRunning` 中，不入队） |
| `AiChatPanel.vue` `toolResultOf` | 仅 `propose_command` 的结构化 JSON 格式 | 需为只读工具返回原始输出文本 |
| `ai.rs` | 无 exec channel 执行入口 | 需新增 `ai_exec_tool` IPC 命令 |
| `ssh/manager.rs` | `write`/`resize` 转发 PTY 操作 | 需新增 `exec_collect` 转发方法 |
| `MAX_TOOL_ROUNDS` | 3（覆盖 propose → execute → summarize） | 需提升至 6（覆盖只读探索 + 命令执行） |
| 提案卡片 UI | 授权弹窗 + 留痕卡片（带状态徽标） | 只读工具需轻量观察卡片（无授权交互） |

结论：本设计补齐三块——后端只读工具定义与 exec channel IPC、前端只读工具自动执行路径与观察卡片、轮次上限调整。

## 4. 目标与非目标

### 目标

1. 三个只读工具定义：`read_file(path)` / `list_directory(path)` / `get_system_info()`，经 `builtin_tools()` 与 `propose_command` 一并下发；
2. 后端 IPC：`ai_exec_tool(session_id, tool_name, args)` → `SessionManager.exec_collect()` → exec channel 执行，10s 超时，8KB 输出截断，返回 `ToolExecResult`；
3. 前端自动执行：`onToolCalls` 中只读工具直接调用 `ai_exec_tool`，结果写入 `execOutput`，不经授权弹窗；
4. 轻量观察卡片：只读工具的执行记录以折叠式卡片展示（工具名 + 参数 + 输出预览），与命令提案卡片视觉区分；
5. `MAX_TOOL_ROUNDS` 从 3 提升至 6；
6. `toolResultOf` 扩展：只读工具返回原始输出文本，`propose_command` 保持既有结构化 JSON。

### 非目标

- 不做 Plan-and-Act 或显式推理链编排（模型自主决定工具调用顺序）；
- 不做工具输出的流式推送（exec channel 一次性返回，非 SSE 流）；
- 不做自定义工具定义接口（工具表后端内置常量，新增工具改后端一处）；
- 不做只读工具的授权开关或白名单（只读即安全，无条件自动执行）；
- 不做 read_file 的二进制检测/编码转换（UTF-8 容错即可，二进制文件输出乱码由模型自行判断）；
- 不做工具调用的费用/频率限制（由 `MAX_TOOL_ROUNDS` 与 exec channel 闸门兜底）。

## 5. 总体架构

```text
┌─ ai_chat_stream（后端，既有不变）─────────────────────────────┐
│  ChatRequest { tools: builtin_tools() }  ← 4 个工具定义一并下发  │
│  SSE 解析 → ai-tool-calls 事件 { calls: [{id, name, args}] }    │
│  ai-done(completed)                                            │
└──────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─ AiChatPanel onToolCalls（前端，路由分流）─────────────────────┐
│  遍历 calls，为每个创建 AiToolProposal { name, args, state }    │
│                                                                 │
│  ├─ name === 'propose_command'  → 既有授权/豁免/自动执行路径    │
│  │   （manual → 弹窗 / risky → 低危自动 / full → 全自动）       │
│  │   执行：writeExec → PTY write + optional capture             │
│  │                                                              │
│  └─ name ∈ {read_file, list_directory, get_system_info}        │
│      → autoExecReadonly(p)  ← 新增路径                           │
│        ├─ aiExecTool(sessionId, name, args)  ← 新增 IPC          │
│        │    └─ SessionManager.exec_collect()  ← 复用既有方法     │
│        │         └─ SSH exec channel: /bin/sh -c '...'          │
│        ├─ p.execOutput = result.output                          │
│        └─ settleProposalRound()  ← 复用既有收尾                 │
└──────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─ settleProposalRound → pumpProposals → continueToolRound ─────┐
│  全部提案落定（accepted/rejected/expired）且无 autoRunning     │
│  → toolRound++ (≤6) → 追加 assistant 消息 → runAiStream 续跑  │
│  buildWireMessages: assistant toolProposals 展开为              │
│    [{role:'assistant', tool_calls:[...]},                      │
│     {role:'tool', tool_call_id, content: toolResultOf(p)}]     │
└──────────────────────────────────────────────────────────────┘
```

关键取舍——**只读工具经 exec channel 而非 PTY 执行**：PTY 是用户的可见终端，只读探索命令（`cat` / `ls` / `uname`）刷屏会干扰用户正在进行的操作；exec channel 是独立非交互通道，输出直接回传前端，不写入 PTY。两者共享同一 SSH 连接，不新建会话。`exec_collect` 已有并发闸门与超时机制，复用零新增基础设施。

关键取舍——**shell 命令构造在后端**：前端只传 `tool_name` + 原始 args JSON，后端根据 `tool_name` 查表构造 shell 命令并做单引号转义。理由：(1) 安全——前端不参与命令拼装，无注入面；(2) 单一来源——工具定义与执行逻辑同在后端，新增工具改一处；(3) 前端无需了解每个工具的 shell 映射。

## 6. 详细设计

### 6.1 工具定义（`builtin_tools()` 扩展）

`builtin_tools()` 返回数组从 1 个工具扩展为 4 个：

```json
[
  {
    "type": "function",
    "function": {
      "name": "read_file",
      "description": "读取远程服务器上的文件内容。适用于查看配置文件、日志片段、脚本源码等。单次最多返回 8KB 文本，超出截断。路径不存在或无权限时返回错误信息。",
      "parameters": {
        "type": "object",
        "properties": {
          "path": { "type": "string", "description": "文件的绝对路径，如 /etc/nginx/nginx.conf" }
        },
        "required": ["path"]
      }
    }
  },
  {
    "type": "function",
    "function": {
      "name": "list_directory",
      "description": "列出远程服务器上指定目录的文件和子目录，包含名称、类型、大小、权限和修改时间。适用于探索目录结构、查找配置文件位置。",
      "parameters": {
        "type": "object",
        "properties": {
          "path": { "type": "string", "description": "目录的绝对路径，如 /etc/nginx" }
        },
        "required": ["path"]
      }
    }
  },
  {
    "type": "function",
    "function": {
      "name": "get_system_info",
      "description": "获取远程服务器的系统信息快照：操作系统、内核版本、CPU 核数、内存、磁盘使用率、运行时间。无需参数。适用于了解环境后进行诊断。",
      "parameters": { "type": "object", "properties": {} }
    }
  },
  {
    "type": "function",
    "function": {
      "name": "propose_command",
      "description": "向用户当前活动的终端提案一条命令。命令会完整展示给用户，经用户批准后写入终端并执行。仅在需要执行命令、操作远程服务器或验证假设时调用；不要用此工具读取文件或获取信息（使用 read_file / list_directory / get_system_info）。",
      "parameters": {
        "type": "object",
        "properties": {
          "command": { "type": "string", "description": "要写入终端的命令原文，必须是用户可读可审的完整命令" },
          "reason": { "type": "string", "description": "一句话说明执行这条命令的目的" }
        },
        "required": ["command"]
      }
    }
  }
]
```

`propose_command` 的 description 追加引导语「不要用此工具读取文件或获取信息（使用 read_file / list_directory / get_system_info）」，让模型优先选择只读工具观察而非盲目提案命令。

### 6.2 后端 IPC：`ai_exec_tool`（`src-tauri/src/ai.rs`）

#### 6.2.1 前置改动：`exec_collect` 签名升级

`ssh/session.rs` 的 `exec_collect` 当前返回 `Result<String, SshError>`——仅返回 output，丢弃了 `ChannelMsg::ExitStatus`（远端命令退出码在 `_ => {}` catch-all 中被忽略），且超时时用 `time::timeout` 包裹整个 future，future 被 drop 导致已收到的部分 stdout/stderr 全部丢失。ReAct 只读工具需要真实退出码区分「命令正常执行」与「命令业务失败」（如文件不存在 exit 1、权限拒绝 exit 13），且超时时应保留已收到的部分输出供模型参考，因此升级签名：

```rust
/// exec channel 执行结果（升级后，含退出码 + 超时标记）
pub struct ExecOutput {
    /// stdout + stderr 合并文本（超时时为已收到的部分片段）
    pub output: String,
    /// 远端命令退出码：Some(0)=成功；Some(n)=命令业务失败；None=通道未给出 ExitStatus（含超时）
    pub exit_code: Option<i32>,
    /// true 表示执行超时，output 为部分输出（可能非空）
    pub timed_out: bool,
}

/// exec_collect 签名从 Result<String> 变为 Result<ExecOutput>
pub async fn exec_collect(
    &self,
    script_body: &str,
    timeout: Duration,
) -> Result<ExecOutput, SshError> {
    // ... 既有 channel_open_session + ch.exec 不变 ...
    let mut out = Vec::new();
    let mut exit_code = None;
    let mut timed_out = false;
    // deadline-based 超时：不再用 time::timeout 包裹整个 future
    // 而是在 ch.wait() 循环内逐次竞速，超时时 break 并保留已收到的部分 output
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() { timed_out = true; break; }
        match tokio::time::timeout(remaining, ch.wait()).await {
            Err(_) => { timed_out = true; break; }  // 单次等待超时
            Ok(None) => break,                      // 通道关闭
            Ok(Some(msg)) => match msg {
                ChannelMsg::Data { data } => out.extend_from_slice(&data),
                ChannelMsg::ExtendedData { data, .. } => out.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => {
                    exit_code = Some(exit_status as i32);
                }
                ChannelMsg::Close => break,
                _ => {}
            }
        }
    }
    Ok(ExecOutput {
        output: String::from_utf8_lossy(&out).into_owned(),
        exit_code,
        timed_out,
    })
}
```

**关键变化**：超时不再返回 `Err(SshError::Channel(...))`，而是返回 `Ok(ExecOutput { output: partial, exit_code: None, timed_out: true })`。已收到的 stdout/stderr 片段保留在 `output` 中，模型至少能看到部分输出而非完全空白。通道级错误（`channel_open_session` 失败等）仍返回 `Err`，此时无任何输出可保留。

**既有调用方适配**（两处，改动量极小）：

| 调用方 | 位置 | 改动 |
|---|---|---|
| 指标采集 | `metrics/collector.rs:179` | `.exec_collect(...).await` → `.exec_collect(...).await.map(|o| o.output)` |
| 会话清理 | `session.rs:1550`（`rm -f` 清理临时脚本） | 同上，取 `.output` 或直接忽略返回值 |

两处调用方不关心退出码与超时标记（指标采集的脚本内部有 `2>/dev/null` 容错；清理命令是尽力而为），仅加 `.output` 访问器即可。

#### 6.2.2 `SessionManager` 转发方法

```rust
/// 经 exec channel 执行只读命令（转发 SshSession.exec_collect）
pub async fn exec_collect(
    &self, session_id: &str, script: &str, timeout: Duration,
) -> Result<ExecOutput, SshError> {
    let session = {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).cloned()
    };
    match session {
        Some(s) => s.exec_collect(script, timeout).await,
        None => Err(SshError::NotFound),
    }
}
```

#### 6.2.3 `ai_exec_tool` 命令

```rust
/// 只读工具执行结果（前端 camelCase）
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolExecResult {
    /// stdout + stderr 合并文本，已截断至 8KB（超时时为部分片段）
    output: String,
    /// true 表示原始输出超出 8KB 已截断
    truncated: bool,
    /// 远端命令退出码：
    ///   Some(0) = 成功；Some(n>0) = 命令业务失败（文件不存在/权限拒绝等，错误信息在 output 中）；
    ///   None = 通道未给出 ExitStatus（含超时/通道级故障）
    exit_code: Option<i32>,
    /// true 表示执行超时，output 含已收到的部分输出（可能非空）
    timed_out: bool,
}

/// 工具名 → shell 命令构造（后端单点，前端不参与拼命令）
fn build_tool_command(tool_name: &str, args: &str) -> Result<String, String> {
    let v: serde_json::Value = serde_json::from_str(args)
        .map_err(|e| format!("工具参数解析失败: {e}"))?;
    match tool_name {
        "read_file" => {
            let path = v.get("path").and_then(|v| v.as_str())
                .ok_or("read_file 缺少 path 参数")?;
            Ok(format!("head -c 8192 {}", shell_quote(path)))
        }
        "list_directory" => {
            let path = v.get("path").and_then(|v| v.as_str())
                .ok_or("list_directory 缺少 path 参数")?;
            Ok(format!("ls -la --time-style=long-iso {}", shell_quote(path)))
        }
        "get_system_info" => {
            // compound 命令：分段输出，每段以 == 标题 == 开头
            Ok(concat!(
                "echo '== OS =='; ",
                "(cat /etc/os-release 2>/dev/null || uname -s) | head -5; ",
                "echo '== KERNEL =='; uname -r; ",
                "echo '== CPU =='; nproc 2>/dev/null; ",
                "echo '== MEMORY =='; free -h 2>/dev/null || echo 'N/A'; ",
                "echo '== DISK =='; df -h / 2>/dev/null; ",
                "echo '== UPTIME =='; uptime 2>/dev/null"
            ).to_string())
        }
        _ => Err(format!("未知工具: {tool_name}"))
    }
}

/// 单引号 shell 转义：' → '\''  （与 exec_collect 的转义口径一致）
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// 执行只读工具：经 SessionManager 查找会话，exec_collect 执行 shell 命令
#[tauri::command]
pub async fn ai_exec_tool(
    session_id: String,
    tool_name: String,
    args: String,
    state: State<'_, crate::ssh::manager::SessionManager>,
) -> Result<ToolExecResult, String> {
    let cmd = build_tool_command(&tool_name, &args)?;
    let session = {
        let sessions = state.sessions.read().await;
        sessions.get(&session_id).cloned()
    };
    let session = session.ok_or("SSH 会话不存在或已断开")?;

    match session.exec_collect(&cmd, std::time::Duration::from_secs(10)).await {
        // exec_collect 返回 Ok：含正常完成 + 超时（timed_out=true 时 output 为部分片段）
        Ok(exec) => {
            let (truncated, capped) = cap_output(&exec.output);
            Ok(ToolExecResult {
                output: capped,
                truncated,
                exit_code: exec.exit_code,
                timed_out: exec.timed_out,
            })
        }
        // exec_collect 返回 Err = 通道级故障（channel_open_session 失败等），无任何输出
        Err(_) => Ok(ToolExecResult {
            output: String::new(),
            truncated: false,
            exit_code: None,
            timed_out: false,
        })
    }
}

/// 输出截断至 8KB UTF-8 字节（与 PROMPT_ASSET_MAX_BYTES 同口径）
fn cap_output(s: &str) -> (bool, String) {
    const MAX: usize = 8 * 1024;
    let bytes = s.as_bytes();
    if bytes.len() <= MAX { return (false, s.to_string()); }
    // 在 UTF-8 边界截断，避免拆分多字节字符
    let mut end = MAX;
    while end > 0 && !s.is_char_boundary(end) { end -= 1; }
    (true, s[..end].to_string())
}
```

**退出码 + 超时标记语义四态**：

| `exit_code` | `timed_out` | 含义 | 前端处理 | 模型可见 |
|---|---|---|---|---|
| `Some(0)` | `false` | 命令成功执行 | `accepted` + output 展示 | 原始输出文本 |
| `Some(n>0)` | `false` | 命令业务失败（文件不存在/权限拒绝等） | `accepted` + output 展示（含 stderr） | 原始输出文本（模型自行判断错误） |
| `None` | `true` | 执行超时，output 为部分片段 | `accepted` + output 展示（部分输出） | 部分输出文本 + `[执行超时，输出可能不完整]` |
| `None` | `false` | 通道级故障（会话断开/通道打开失败） | `rejected` + error 文案 | `{"error":"执行失败（会话断开）"}` |

关键区分：**命令业务失败与超时都不是通道故障**——`head` 报「No such file」时 exit_code=`Some(1)`、output 含 stderr 文本，前端标记 `accepted`（观察动作完成了）；超时时 `exit_code=None` + `timed_out=true`，output 含已收到的部分片段，前端也标记 `accepted`（部分观察仍有价值，模型据片段自行调整策略）。只有 `exec_collect` 返回 `Err`（通道无法建立等）才标记 `rejected`。

#### 6.2.4 后端 exec 任务取消（会话切换防堆积）

用户快速切换会话时，`autoExecReadonly` 的 IPC 已派发，后端 exec 任务仍在运行（最长 10s），前端仅丢弃结果——大量切换会在后端堆积无用 exec 任务。为此引入 scope 级取消令牌，会话切换时主动取消旧会话的在飞 exec 任务。

**`AiState` 扩展**（复用既有 State，新增 exec 取消表）：

```rust
#[derive(Default)]
pub struct AiState {
    /// 既有：scope → 对话流取消令牌
    cancels: StdMutex<HashMap<String, CancellationToken>>,
    /// 新增：session_id → 在飞 exec 取消令牌列表
    /// （同一会话可能并行多个只读工具，用 Vec 收集）
    exec_cancels: StdMutex<HashMap<String, Vec<CancellationToken>>>,
}

impl AiState {
    /// 登记新 exec 令牌，返回克隆供 select! 使用
    fn register_exec(&self, session_id: &str) -> CancellationToken {
        let token = CancellationToken::new();
        let mut map = self.exec_cancels.lock().unwrap();
        map.entry(session_id.to_string()).or_default().push(token.clone());
        token
    }

    /// 取消并清除指定会话的全部在飞 exec 令牌
    fn cancel_execs(&self, session_id: &str) {
        let mut map = self.exec_cancels.lock().unwrap();
        if let Some(tokens) = map.remove(session_id) {
            for t in tokens { t.cancel(); }
        }
    }

    /// exec 完成后注销令牌（正常/超时/取消均调用，避免列表膨胀）
    fn unregister_exec(&self, session_id: &str, token: &CancellationToken) {
        let mut map = self.exec_cancels.lock().unwrap();
        if let Some(tokens) = map.get_mut(session_id) {
            tokens.retain(|t| t != token);
            if tokens.is_empty() { map.remove(session_id); }
        }
    }
}
```

**`ai_exec_tool` 集成取消**：

```rust
#[tauri::command]
pub async fn ai_exec_tool(
    session_id: String,
    tool_name: String,
    args: String,
    state: State<'_, SessionManager>,
    ai_state: State<'_, AiState>,  // 新增：取消令牌管理
) -> Result<ToolExecResult, String> {
    let cmd = build_tool_command(&tool_name, &args)?;
    let session = { /* ... 既有查找逻辑 ... */ };
    let session = session.ok_or("SSH 会话不存在或已断开")?;

    let token = ai_state.register_exec(&session_id);
    let exec_fut = session.exec_collect(&cmd, std::time::Duration::from_secs(10));
    // exec_collect 与取消令牌竞速：会话切换时前端调 ai_cancel_execs → token.cancel()
    let result = tokio::select! {
        _ = token.cancelled() => {
            // 会话已切换，exec 结果不再需要
            Ok(ToolExecResult { output: String::new(), truncated: false, exit_code: None, timed_out: false })
        }
        r = exec_fut => {
            match r {
                Ok(exec) => {
                    let (truncated, capped) = cap_output(&exec.output);
                    Ok(ToolExecResult { output: capped, truncated, exit_code: exec.exit_code, timed_out: exec.timed_out })
                }
                Err(_) => Ok(ToolExecResult { output: String::new(), truncated: false, exit_code: None, timed_out: false })
            }
        }
    };
    ai_state.unregister_exec(&session_id, &token);
    result
}

/// 取消指定会话的全部在飞 exec 任务（会话切换时前端调用）
#[tauri::command]
pub fn ai_cancel_execs(session_id: String, ai_state: State<'_, AiState>) {
    ai_state.cancel_execs(&session_id);
}
```

**`exec_collect` 不需改动**：`tokio::select!` 在 `ai_exec_tool` 层竞速，token 被取消时 `exec_fut` 被 drop，其内部的 `exec_collect` future 被 drop → `ch.wait()` future 被 drop → `METRIC_EXEC_GATE` permit 被 drop → 通道关闭。与超时机制（§6.2.1 deadline-based 循环内 `tokio::time::timeout`）的 drop 语义一致。取消路径返回 `timed_out: false`（区别于超时的 `true`），前端据 `scope.value !== atScope` 丢弃结果。

**魔数与取值理由**：

| 常量 | 值 | 理由 |
|---|---|---|
| 输出上限 | 8KB | 与 `PROMPT_ASSET_MAX_BYTES` 同口径；8KB ≈ 2k~4k tokens，足够展示一个配置文件或目录列表，不撑爆上下文窗口 |
| 执行超时 | 10s | 远端只读命令（cat/ls/uname）正常 <100ms；10s 覆盖慢主机与磁盘 IO 抖动，不阻塞对话太久 |
| `head -c 8192` | read_file 的 shell 侧截断 | 防止读 1GB 日志时 `cat` 全量输出再在 Rust 侧截断——shell 侧截断更省内存 |

### 6.3 前端工具路由（`AiChatPanel.vue`）

**`onToolCalls` 扩展**：按 `name` 分流，只读工具走自动执行路径。

```ts
onToolCalls: e => {
  if (e.scope !== scope.value || !streaming.value) return
  const last = messages.value[messages.value.length - 1]
  if (!last || last.role !== 'assistant') return

  last.toolProposals = e.calls.map(c => ({
    id: c.id,
    name: c.name,   // read_file / list_directory / get_system_info / propose_command
    args: c.arguments ?? '',
    reason: c.name === 'propose_command' ? parseProposalArgs(c.arguments)?.reason : undefined,
    state: 'pending' as const,
  }))

  const allow = sessionAllowlist.get(scope.value)
  for (const p of last.toolProposals) {
    if (p.state !== 'pending') continue

    if (p.name === 'propose_command') {
      // 既有授权/豁免/自动执行分流（不变）
      const cmd = parseProposalArgs(p.args)?.command
      if (!cmd) continue
      if (allow?.has(cmd)) { autoApply(p, 'session') }
      else if (execPermission.value === 'full' ||
               (execPermission.value === 'risky' &&
                !matchHighRisk(cmd, savedSettings.aiRiskKeywords.split('\n')))) {
        autoApply(p, 'once')
      }
    } else {
      // 只读工具：无条件自动执行（不经授权弹窗）
      autoExecReadonly(p)
    }
  }
  persist()
}
```

**`autoExecReadonly`**（新增，与 `autoApply` 同构）：

```ts
/** 只读工具自动执行：经 exec channel 执行，结果写入 execOutput，
 *  完成后统一交 settleProposalRound 收尾推进（与 autoApply 同路径） */
function autoExecReadonly(p: AiToolProposal) {
  const atScope = scope.value
  const sid = activeSessionId.value
  if (!sid || !activeSession.value?.backendId || activeSession.value?.state !== 'online') {
    p.state = 'rejected'
    p.error = '无可用会话或会话已断开'
    settleProposalRound()
    return
  }
  autoRunning.add(p)  // 竞态防护：pumpProposals 不入队执行中的只读提案
  aiExecTool(sid, p.name, p.args).then(result => {
    if (scope.value !== atScope) return
    p.execOutput = result.output
    if (result.truncated) {
      p.execOutput += '\n[输出超 8KB 已截断]'
    }
    if (result.timedOut) {
      // 超时：output 含已收到的部分片段，标记 accepted（部分观察仍有价值）
      p.state = 'accepted'
      p.execOutput += p.execOutput
        ? '\n[执行超时，输出可能不完整]'
        : '[执行超时，无输出]'
    } else if (result.exitCode === null) {
      // 通道级故障（会话断开/通道打开失败）：无输出，标记 rejected
      p.state = 'rejected'
      p.error = '执行失败（会话断开）'
    } else {
      // Some(0) 成功 | Some(n>0) 命令业务失败（文件不存在等）
      // 两者都标记 accepted：观察动作已完成，输出中的 stderr 供模型自行理解
      p.state = 'accepted'
    }
  }).catch(e => {
    if (scope.value !== atScope) return
    p.state = 'rejected'
    p.error = typeof e === 'string' ? e : (e as Error)?.message ?? String(e)
  }).finally(() => {
    autoRunning.delete(p)
    if (scope.value === atScope) settleProposalRound()
  })
}
```

**会话切换取消**（`watch(scope, ...)` 扩展）：

既有 `watch(scope, ...)` 在会话切换时执行 `loadSeq++` / `resetProposalState()` / `loadRecord(s)`。新增：切换时取消旧会话的在飞 exec 任务，防止后端堆积无用任务。

```ts
watch(scope, (s, oldS) => {
  loadSeq++
  streaming.value = false
  editFromId.value = null
  execPermission.value = 'manual'
  resetProposalState()
  // 新增：取消旧会话的在飞 exec 任务（旧 scope 非 global 时才需取消）
  if (oldS && oldS !== 'global') {
    void aiCancelExecs(oldS).catch(() => {}) // 失败静默，取消是尽力而为
  }
  void loadRecord(s)
})
```

`oldS` 即旧 scope 值（`'global'` 或旧会话 id），与 `ai_cancel_execs` 的 `session_id` 参数一致——`ai_exec_tool` 在 `ai_cancel_execs` 中按 `session_id` 查找取消令牌（§6.2.4）。

**`pumpProposals` 过滤**：只入队 `propose_command` 的 pending 提案。

```ts
function pumpProposals() {
  if (streaming.value) return
  const last = [...messages.value].reverse().find(m => m.role === 'assistant')
  if (last?.toolProposals) {
    for (const p of last.toolProposals) {
      // 只读工具已在 autoRunning 中（autoExecReadonly 同步 add），不会进入此分支；
      // 额外 name 防御：万一 autoExecReadonly 未触发（如 onToolCalls 异常）
      // 也不把只读提案放入授权弹窗队列
      if (p.state === 'pending' && p.name === 'propose_command' && !autoRunning.has(p)) {
        proposalQueue.value.push({ msgId: last.id, proposal: p })
      }
    }
  }
  // ... 后续不变（队列弹出 / awaitingContinue / continueToolRound）
}
```

**`toolResultOf` 扩展**：只读工具返回原始输出。

```ts
function toolResultOf(p: AiToolProposal): string {
  // 只读工具：直接返回输出文本（模型据此理解远程环境）
  if (p.name !== 'propose_command') {
    if (p.error) return JSON.stringify({ error: p.error })
    return p.execOutput ?? '(无输出)'
  }
  // propose_command：既有结构化 JSON 格式（不变）
  if (p.state === 'accepted') { ... }
  if (p.state === 'rejected') return JSON.stringify({ accepted: false, rejected: true })
  return JSON.stringify({ accepted: false, expired: true })
}
```

### 6.4 前端 API 封装（`lib/ai.ts`）

```ts
/** 只读工具执行结果（serde camelCase；exitCode 为 Option<i32> → null | number） */
export interface AiToolExecResult {
  /** stdout + stderr 合并文本，已截断至 8KB（超时时为部分片段） */
  output: string
  /** true 表示原始输出超出 8KB 已截断 */
  truncated: boolean
  /** 远端退出码：0=成功；n>0=命令业务失败（错误信息在 output 中）；null=通道未给出 ExitStatus（含超时/通道级故障） */
  exitCode: number | null
  /** true 表示执行超时，output 含已收到的部分输出（可能非空） */
  timedOut: boolean
}

/** 经 SSH exec channel 执行只读工具（后端构造 shell 命令，前端只传 tool_name + args） */
export function aiExecTool(
  sessionId: string,
  toolName: string,
  args: string,
): Promise<AiToolExecResult> {
  return invoke<AiToolExecResult>('ai_exec_tool', { sessionId, toolName, args })
}

/** 取消指定会话的全部在飞 exec 任务（会话切换时调用，防后端堆积） */
export function aiCancelExecs(sessionId: string): Promise<void> {
  return invoke<void>('ai_cancel_execs', { sessionId })
}
```

`AiToolProposal` 类型无变更——`name` 字段已支持多工具，`execOutput` 字段已支持输出存储。

### 6.5 观察卡片视觉设计

只读工具的提案卡片与 `propose_command` 卡片视觉区分：

```vue
<!-- 只读工具观察卡片（轻量、无授权交互、可折叠输出） -->
<div
  v-for="p in m.toolProposals"
  :key="p.id"
  class="proposal"
  :class="[p.state, { 'readonly-tool': p.name !== 'propose_command' }]"
>
  <!-- 既有 propose_command 卡片结构不变 -->
  <template v-if="p.name === 'propose_command'">
    <!-- ... 命令提案卡片（授权/拒绝/执行失败 状态徽标） ... -->
  </template>
  <!-- 只读工具观察卡片 -->
  <template v-else>
    <div class="proposal-head">
      <span class="proposal-tag readonly-tag">{{ readonlyLabel(p.name) }}</span>
      <span class="proposal-args">{{ readonlyArgs(p) }}</span>
      <span class="proposal-state">
        {{ p.error ? '执行失败' : p.execOutput ? '已观察' : '执行中…' }}
      </span>
    </div>
    <div v-if="p.error" class="proposal-error">{{ p.error }}</div>
    <details v-if="p.execOutput" class="readonly-output">
      <summary>输出（{{ p.execOutput.length }} 字符）</summary>
      <pre class="proposal-cmd">{{ p.execOutput }}</pre>
    </details>
  </template>
</div>
```

```ts
function readonlyLabel(name: string): string {
  return { read_file: '读取文件', list_directory: '列出目录', get_system_info: '系统信息' }[name] ?? name
}
function readonlyArgs(p: AiToolProposal): string {
  const parsed = parseProposalArgs(p.args)
  if (p.name === 'get_system_info') return ''
  return parsed?.path ?? p.args
}
```

CSS 差异：`.readonly-tool` 使用蓝色系边框/标签（区别于 `propose_command` 的黄色 pending / 红色 rejected），输出区用 `<details>` 折叠（默认收起，点击展开），避免长输出撑爆消息流。

### 6.6 MAX_TOOL_ROUNDS 调整

```ts
/** 单次用户输入允许的最大工具循环轮次。
 *  ReAct 场景：3~4 次只读探索 + 2~3 次命令执行 = 6 轮上限 */
const MAX_TOOL_ROUNDS = 6
```

取值理由：典型 ReAct 流程为「get_system_info → read_file → list_directory → propose_command → read_file(验证) → propose_command(修复)」约 5~6 轮；3 轮仅够 propose → execute → summarize，不足以支撑探索阶段。6 轮覆盖绝大多数排障场景，超出时 toast 提示用户重新提问。

## 7. 安全与降级

- **shell 注入防护**：`build_tool_command` 对 `read_file` / `list_directory` 的 `path` 参数做单引号转义（`shell_quote`，`'` → `'\''`），与 `exec_collect` 的转义口径一致；`get_system_info` 无外部参数，命令为硬编码常量；
- **工具名白名单**：`build_tool_command` 的 `match` 仅认 `read_file` / `list_directory` / `get_system_info`，未知 `tool_name` 返回错误——前端无法通过伪造工具名执行任意命令；
- **exec channel 权限边界**：只读命令经 `/bin/sh -c` 执行，权限与 SSH 登录用户一致（与 PTY 同级），不提权、不降权；
- **输出有界**：shell 侧 `head -c 8192` + Rust 侧 `cap_output` 双闸，防大文件/长目录列表撑爆上下文窗口；
- **超时防护**：`exec_collect` 10s deadline-based 循环内超时，超时时 break 并返回已收到的部分 output（标记 `timed_out=true`），不丢弃片段；通道 drop 关闭，远端 sh 收 SIGHUP 退出，无残留进程；
- **并发闸门**：`METRIC_EXEC_GATE` 限制同时在飞的 exec 通道数，保护本机 fd 与远端 sshd `MaxSessions`；
- **会话校验**：`autoExecReadonly` 执行前校验 `activeSession.backendId` 与 `state === 'online'`，断开时直接 reject 并收尾；
- **会话切换取消**：`watch(scope, ...)` 切换时调用 `ai_cancel_execs(oldSessionId)`，`tokio::select!` 在 `ai_exec_tool` 层竞速取消令牌与 `exec_collect`——令牌取消时 `exec_fut` 被 drop，通道关闭、`METRIC_EXEC_GATE` permit 释放，不堆积无用任务。取消失败静默（`catch(() => {})`），不影响会话切换主流程；
- **隐私边界**：只读工具的输出作为 `tool` 消息回传模型服务商，与用户手动复制粘贴文件内容到对话的隐私口径一致——不是终端自动读取，是模型主动发起的 exec 查询；输出不入应用日志；
- **降级矩阵**：
  - 会话不存在/已断开 → `exec_collect` 返回 `Err(SshError::NotFound)` → `exit_code=null, timed_out=false`，前端记为 `rejected` + error 文案，模型据此调整策略；
  - 命令超时 → `exec_collect` 返回 `Ok(ExecOutput { output: partial, exit_code: None, timed_out: true })` → 前端标记 `accepted`（部分观察仍有价值），output 展示已收到片段 + `[执行超时，输出可能不完整]` 标注，模型据部分输出调整策略；
  - 会话切换取消 → `ai_cancel_execs` 触发 `token.cancel()` → `tokio::select!` 返回 `exit_code=null, timed_out=false` → 前端 `scope.value !== atScope` 丢弃结果（结果本身不会写入 proposal，因为 IPC 的 `.then()` 回调先于 scope 检查退出）；
  - 文件不存在/无权限 → `head` / `ls` 退出码非零（`Some(1)`），stderr 并入 output，前端标记 `accepted`（观察动作完成），模型据 output 中的错误文本自行调整；
  - `ai_exec_tool` IPC 失败（`build_tool_command` 报错等） → 前端 catch → `rejected` + error，`settleProposalRound` 正常推进；
  - exec channel 不被服务端支持（极少见） → `exec_collect` 返回 `Err(SshError::Channel)`，`exit_code=null, timed_out=false`，前端标记 `rejected` + error。

## 8. 测试与验证

- **后端单测**（`ai.rs` `#[cfg(test)]`）：
  1. `builtin_tools_shape`：扩展为 4 个工具，验证 name 分别为 `read_file` / `list_directory` / `get_system_info` / `propose_command`；
  2. `build_tool_command`：各工具名 + 合法 args → 正确 shell 命令；非法 args / 未知工具名 → Err；
  3. `shell_quote`：普通路径 → 单引号包裹；含单引号路径 → `'\''` 转义；
  4. `cap_output`：≤8KB 不截断；>8KB 在 UTF-8 边界截断并标记 truncated；
  5. `exec_collect` 退出码捕获：`ChannelMsg::ExitStatus` → `Some(code)`；通道无 ExitStatus → `None`（需要 SSH 会话 mock 或集成测试）；
  6. `exec_collect` 超时部分输出保留：deadline 到达前已收到 `ChannelMsg::Data` 片段 → 超时后返回 `Ok(ExecOutput { output: partial, exit_code: None, timed_out: true })`，output 非空（需要 mock channel 逐帧投递 + 延迟）。
- **前端验证**：
  1. `onToolCalls` 混合工具路由：同一轮含 read_file + propose_command，只读工具进 `autoExecReadonly`、命令工具进授权队列；
  2. `toolResultOf`：只读工具返回原始 output；propose_command 返回结构化 JSON；
  3. `pumpProposals` 过滤：只读提案（autoRunning 中）不入授权队列；
  4. `settleProposalRound` 混合收尾：只读 + 命令提案全部落定后才续跑。
  5. 会话切换取消：`watch(scope, ...)` 触发 `aiCancelExecs(oldS)`，旧会话在飞 exec 被取消，前端 `.then()` 回调因 `scope !== atScope` 丢弃结果。
  6. 超时部分输出：`aiExecTool` 返回 `timedOut=true` + 非空 output → `autoExecReadonly` 标记 `accepted`，`execOutput` 含部分片段 + `[执行超时，输出可能不完整]` 标注；`timedOut=false` + `exitCode=null` → 标记 `rejected` + error。
- **手工验证清单**：
  1. 工作台提问「nginx 为什么 502」→ 模型先调 `get_system_info` 观察环境 → 调 `read_file` 读 nginx.conf → 调 `list_directory` 看 conf.d → 调 `propose_command` 测试后端 → 给出结论；
  2. 只读工具卡片显示蓝色标签 + 折叠输出，不出现授权弹窗；
  3. 命令提案仍走黄色 pending → 授权弹窗流程；
  4. 构造 6 轮工具调用验证上限终止 + toast 提示；
  5. 会话断开后只读工具 → 卡片标「执行失败」+ error（`exitCode=null, timedOut=false` 路径）；
  6. `read_file` 指向不存在文件 → 卡片标「已观察」+ output 显示 `head: cannot open ... No such file`（`exitCode=Some(1)` 路径，命令业务失败但观察完成）；
  7. 首页（global scope）不出现只读工具调用（`enableTools: false`）；
  8. 切换 scope 后遗留的只读提案 autoRunning 被正确清理（`resetProposalState` + `autoRunning.clear`）；
  9. 快速切换会话：只读工具执行中切到另一会话 → 旧会话 exec 任务被取消（后端不堆积），切回后对话正常续跑；
  10. 超时部分输出：构造慢命令（如 `sleep 15`）→ 10s 超时后卡片标「已观察」+ output 含 `[执行超时，输出可能不完整]`（`exitCode=null, timedOut=true` 路径），模型据部分输出续跑。

## 9. 未决问题

- **工具集扩展**：当前三个只读工具覆盖文件/目录/系统信息，后续可能需要 `check_port`（端口连通性）、`check_process`（进程状态）、`read_log`（日志尾部跟踪）等。新增工具只需在 `builtin_tools()` 加定义 + `build_tool_command` 加 match 分支，架构无需变动。
- **MAX_TOOL_ROUNDS 动态化**：当前固定 6 轮，复杂排障可能不够。后续可考虑按只读/写操作分别计数（只读 5 轮 + 写 3 轮），或提供用户可调设置。MVP 先固定 6。
- **read_file 二进制检测**：当前 UTF-8 容错，二进制文件输出乱码。若实际使用中模型被乱码误导，可加 NUL 字节检测返回「二进制文件，无法显示」提示。
- **输出格式化**：`list_directory` 的 `ls -la` 输出为纯文本，模型可读但不够结构化。后续可考虑解析为 JSON（`ls --json` 或后端解析），提升模型理解精度。MVP 先用纯文本（兼容性最好）。
- **工具调用费用控制**：每个只读工具发起一次 exec channel + 一次模型续跑，6 轮最多 12 次 API 调用。若模型服务商按量计费，可能需要加频率限制或费用预估提示。MVP 不做。
