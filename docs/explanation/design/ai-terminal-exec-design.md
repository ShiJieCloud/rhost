# AI 终端命令执行（M2）设计方案

> description: AI 向终端写入命令的完整设计——代码块级插入/直接执行与后端 tool calling 提案通道，复用 write_terminal，裁决目标会话与多行安全规则
>
> created: 2026-10-11 21:00:40
>
> updated: 2026-10-12 02:30:00
>
> author:

---

## 1. 设计目标与硬约束

> 范围：M2 阶段「AI → 终端」写入通道的两条路径——用户手动路径（代码块级「插入终端 / 直接执行」按钮）与模型主动路径（tool calling 提案工具 `propose_command`，经用户确认后写入）。两条路径汇入同一 PTY 写入原语。
> 边界：不覆盖终端输出回传模型（与隐私边界冲突，未来单独设计）；不覆盖命令片段库；不覆盖完整 React / Plan-and-Act 循环（本文仅铺设 tool 协议层）；不覆盖 SFTP 与端口转发。
> 关联代码：`frontend/src/components/AiMarkdown.vue`、`frontend/src/components/AiChatPanel.vue`、`frontend/src/lib/ai.ts`、`frontend/src/stores/session.ts`、`frontend/src/stores/settings.ts`、`src-tauri/src/ai.rs`、`src-tauri/src/ipc.rs`

| 目标 | 量化口径 |
|---|---|
| 零新增 IPC（手动路径） | 代码块按钮复用既有 `write_terminal` 命令，后端 0 改动 |
| 低延迟 | 点击确认 → `write_terminal` invoke 发起 < 50ms（本地 IPC，不含远端执行） |
| 多行防护 | `aiMultilineExec=false`（默认）时，含换行的「直接执行」100% 拦截 |
| 确认覆盖 | 「直接执行」与工具提案 100% 有确认交互，无静默写入 |
| 隐私零回传 | 终端输出 0 字节自动发送给模型 |

必须遵守的既有项目约束：

- **终端内容不会以任何自动形式离开本机**，AI 仅接收用户显式选中并主动发送的终端内容，无静默自动读取逻辑（用户决策，M2.1 已删除自动注入）；
- AI 对话按视图 scope 隔离，工作台会话绑定当前 SSH 会话，首页为全局对话（无终端目标）；
- `AiMarkdown` 保持纯展示组件：终端操作仅 emit 意图，PTY 写入与会话定位由宿主 `AiChatPanel` 执行（`ai-message-actions-design.md` §6.4）；
- 图标统一 24 viewBox / stroke currentColor / 实际渲染 14px（`ai-message-actions-design.md` §6.4）。

## 2. 必须遵守的既有项目约束

- `write_terminal(session_id, data: Vec<u8>)`（`src-tauri/src/ipc.rs`）是唯一 PTY 写入原语，键盘输入同源；**禁止**绕过会话池另开 SSH exec channel——绕过 PTY 等于绕过用户可见性；
- `AiWireMessage` 与 OpenAI 消息同构、后端透传（`frontend/src/lib/ai.ts` 注释即契约）；嵌套字段名跟随 OpenAI 原名（`tool_calls`、`tool_call_id`），不因 Tauri 参数 camelCase 惯例改变；
- 错误即协议：返回字符串错误带前缀（先例 `KEY_ENCRYPTED:`、`TUNNEL_*`），前端据此分流，文案前缀不可随意改动；
- 设置项集中在 `frontend/src/stores/settings.ts`，默认值显式声明；
- Toast 类型 `'ok' | 'info' | 'warn' | 'err'`（`frontend/src/composables/useToast`）；
- 不可信来源的大段写入需确认（先例：`TerminalPane.vue` bracketed-paste 守卫弹窗）。

## 3. 现状盘点与差距

| 位置 | 现状 | 差距 |
|---|---|---|
| `src-tauri/src/ipc.rs` `write_terminal` | 键盘原始字节写入通道已稳定 | 无，直接复用 |
| `AiMarkdown.vue` `md-code-head` | 仅语言标签 + 复制按钮（事件委托，原文取 `pre:not(.md-code-lines)`） | 缺插入终端 / 直接执行按钮与 emit 契约 |
| `AiChatPanel.vue` | 无任何终端写入接线 | 缺目标会话定位、多行校验、确认交互 |
| `src-tauri/src/ai.rs` `ChatRequest` | 无 `tools` 字段；SSE 解析仅认 `delta.content` / `delta.reasoning_content` | 缺 tools 参数、`delta.tool_calls` 流式聚合、tool 结果回传 |
| `AiChatMessage`（`lib/ai.ts`） | `id/role/content/reasoning/status/createdAt` | 缺工具提案字段（含确认状态）的持久化 |
| `stores/settings.ts` | AI 模块已有启用/接入/技能/规则/记忆分组 | 缺 `aiMultilineExec` 开关 |

结论：本设计补齐三块——前端代码块按钮链路（后端零改动）、后端 tool calling 协议层、工具提案确认 UI 与消息模型扩展。

## 4. 目标与非目标

### 目标

1. 代码块级按钮（仅 workbench、非 streaming）：插入终端（写 PTY 不回车）、直接执行（写 PTY + 回车）；
2. 后端 tool calling 协议：`tools` 参数透传、`delta.tool_calls` 流式聚合、`finish_reason=tool_calls` 处理、`role:"tool"` 消息回传后再请求；
3. 首个工具 `propose_command`：模型提案 → 前端确认卡片 → 用户裁决（接受写入 / 拒绝）→ 结果回传模型继续对话；
4. 设置项 `aiMultilineExec`（默认 `false`）；
5. 裁决目标会话取值、多行命令规则、工具循环驱动方（本文 §6）。

### 非目标

- 不做终端输出自动采集或回传模型（见 §9 未决问题）；
- 不做 SSH exec channel 直接执行；
- 不做命令风险分级 / 白名单 / 审批流（M2 一律确认制）;
- 不做片段库、定时执行、批量执行；
- 不做工具执行结果的流式输出。

## 5. 总体架构

```text
┌─────────────── AiChatPanel（宿主，持有 scope/会话状态）───────────────┐
│                                                                      │
│  路径 A：用户手动（M2.2，前端为主，后端零改动）                        │
│  AiMarkdown(md-code-head 按钮)                                       │
│      └─ emit('terminal', {code, run}) ──► execInTerminal(code, run)  │
│             ├─ 目标会话解析（当前活动会话）                            │
│             ├─ 多行校验（aiMultilineExec）                            │
│             ├─ 直接执行：确认弹窗（命令原文）                          │
│             └─ invoke('write_terminal')  ◄── 复用既有命令             │
│                                                                      │
│  路径 B：模型提案（M2.3，后端协议 + 前端驱动循环）                      │
│  ai_chat_stream(tools=[propose_command])                             │
│      └─ ai-tool 事件 {callId, name, args} ► 提案卡片（pending）       │
│            ├─ 接受 ► execInTerminal(同路径 A 原语与规则)              │
│            │         └─ messages 追加 tool 结果 ► 再次 ai_chat_stream │
│            └─ 拒绝 ──┘（结果 {'accepted':false} → 模型继续）          │
└──────────────────────────────────────────────────────────────────────┘
```

关键取舍——**工具循环由前端驱动，后端保持无会话状态**：后端本轮流结束（`finish_reason=tool_calls`）即返回，不挂起等待裁决；前端持久化提案、用户裁决后把 tool 结果并入 `messages` 再次发起 `ai_chat_stream`。理由：前端已拥有全部会话状态与落盘能力（M1 分层），后端挂起循环需处理 scope 切换 / 超时 / 应用重载丢失等一堆状态问题，前端驱动天然免费获得提案持久化与竞态防护。

## 6. 详细设计

### 6.1 代码块级操作（M2.2，前端为主）

**按钮与 emit 契约**（`AiMarkdown.vue`）：

- `md-code-head` 右侧追加两个文字按钮：插入终端（`.md-code-insert`）、直接执行（`.md-code-run`），与复制按钮并列常显（复制按钮为既有文字按钮风格，三者同款式）；
- 复制按钮的事件委托模式扩展：`onClick` 中识别 `.md-code-copy` / `.md-code-insert` / `.md-code-run`，代码文本取 `pre:not(.md-code-lines)` 的 `textContent`（与复制同源，天然排除行号列）；
- 组件新增 `props.terminalReady?: boolean`（宿主传入，AiMarkdown 不依赖 store）；新增 `emit('terminal', { code: string, run: boolean })`；
- 可见性规则：`terminalReady === false` 时两个按钮不可见且不可交互——实现为容器类名（`.md.term-ready`）驱动的 CSS 显隐，原因是按钮由 markdown-it fence 渲染、经 v-html 注入，无法响应式增删 DOM；可见性口径：首页、无在线会话（`state !== 'online'`）、streaming 期间均不可见，不出现不可用入口。

**宿主执行**（`AiChatPanel.vue`，`onTerminalIntent` → `execInTerminal` 路径）：

1. 目标会话解析：`appView === 'workbench' && activeSession` 才可用；**裁决：目标会话 = 点击时的当前活动会话**，不做消息产生时的快照。理由：工作台 AI 对话本就绑定当前会话（scope 一致性）；会话重连后 `backendId` 变化使快照必然失效；向用户「当前看不见的终端」写入违背预期。无可用会话时 toast `'err'`「无可用会话，无法写入终端」；
2. 编码与写入：复用 `stores/session.ts` 的 `sendInput(id, text)`——该封装已完成 TextEncoder 编码、`backendId` 解析、online 状态守卫与 `write_terminal` invoke（键盘输入同源），前端不直接触碰 IPC；写入前校验 `state === 'online'`，断开时 toast `'err'`（`sendInput` 对非 online 静默丢弃，故宿主必须先行校验）；
3. 插入终端（`run=false`）：写入原文，**不追加回车**，用户可先编辑；成功 toast「已插入终端」。多行文本裸写时中间 `\n` 即回车、会被 shell 逐行执行，故**含换行时用 bracketed paste 包装**（`\x1b[200~` + 文本 + `\x1b[201~`）：支持 2004 模式的 shell（bash 4.4+ / zsh / fish 等主流）整块进编辑缓冲区，等用户回车才执行；不支持的 shell 显示转义序列字面量，同样不会执行；
4. 直接执行（`run=true`）：
   - 多行先决：`aiMultilineExec === false` → toast `'warn'`「多行命令直接执行未开启，可在设置 → AI → 终端执行中开启」并中止；
   - 确认模式（`aiExecConfirmMode`，默认 `risky`）：`always` 每次弹确认窗（命令原文完整可读、Esc 取消 / Enter 确认）；`risky` 仅命中高危规则时弹窗且附红色警示（「检测到高危命令（规则名）」），其余直接执行；`never` 点击即执行——按钮点击本身已是意图确认，命令原文就在眼前；
   - 高危判定原语 `lib/aiRisk.ts` `matchHighRisk(code, customKeywords)`：内置正则规则集（递归强制删除 / 磁盘写入 / 关机重启 / 管道执行远程脚本等，随版本演进、不可编辑、不提供关闭开关——安全底线，嫌误报可切 `never`）+ 自定义关键词（`aiRiskKeywords` 一行一条，**纯子串匹配**、大小写敏感，刻意不做正则以降低使用门槛，复杂模式交给内置层）；多行命令逐行匹配，任一行命中即整段高危；命中来源（内置规则中文名 / 关键词原文）供弹窗警示展示。规则集定位是「把确认精力集中在大概率危险的操作上」，不是安全边界；
   - 写入方式：单行裸写 + `\n`；多行以 bracketed paste 包装整块写入 + 单个 `\n` 提交。**裁决：多行整体执行而非逐行裸写**——逐行裸写边写边执行，用户失去整体反悔窗口，且不完整复合命令会闪现 PS2 续行符中间态；整体提交与插入终端路径行为统一（插入 = 进缓冲不回车；执行 = 进缓冲 + 回车）。实现细节：`\x1b[201~` 结束标记必须在 `\n` 之前，否则回车被当作字面换行插入缓冲区而非提交；
5. 确认弹窗支持 Esc 取消 / Enter 确认（对齐 `TerminalPane.vue` paste guard 先例），点遮罩不关闭语义由 Esc 承担；确认时再次校验会话在线，失败 toast `'err'`「会话已断开，命令未执行」。

### 6.2 tool calling 协议层（M2.3，后端为主）

**请求扩展**（`src-tauri/src/ai.rs`，① 已实现）：

```rust
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [AiMessage],
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<serde_json::Value>,   // 新增；None 时请求体与 M1 完全一致
}
```

**裁决：工具定义后端内置，前端不传**。`ai_chat_stream` 增加 `enable_tools: Option<bool>` 参数（不新增 IPC 命令），true 时附加后端常量 `builtin_tools()`——工具定义是协议而非前端状态，内置后端避免前端硬编码 schema、避免 IPC 传大 JSON；后续新增工具只改后端一处。

**M2 工具表**（仅一个，最小参数面）：

| 字段 | 值 |
|---|---|
| 名称 | `propose_command` |
| 参数 | `command: string`（必填，待写入终端的命令原文）、`reason: string`（可选，一句话用途，供提案卡片展示） |
| 语义 | 提案命令，等待用户裁决；模型不假设命令已执行 |

**SSE 解析扩展**（`handle_sse_line` 状态机，① 已实现）：

- `StreamAcc` 累积器：`delta.tool_calls[]` 按 `index` 聚合（`acc_tool_call` 独立纯函数，可单测）——`id`、`function.name` 首片赋值，`function.arguments` 分片字符串拼接（OpenAI 兼容层普遍分片发送 arguments）；`choices[0].finish_reason` 记录进 acc；
- **裁决：流终止统一由 `[DONE]` / 对端关闭判定**，`finish_reason` 仅作信息记录、不触发提前结束——个别实现 finish_reason 先到而流还有残余帧，提前终止会丢尾部；流自然结束时 `finish_stream` 宽松判断（有累积分片即发事件，不强依赖 finish_reason），聚合结果按 index 排序经新事件推送：

```text
ai-tool-calls 事件  { scope, calls: [{ id, name, arguments }, ...] }  // 一轮一条，calls 为聚合后的完整提案
ai-done 事件  { scope, reason: "completed" }                          // reason 枚举不扩展，语义不变
```

- 提案走独立事件而非 `ai-token`，避免污染正文 token 流；`AiTokenKind` 不扩展；
- **裁决：一轮一条事件而非逐 call 一条**——前端需按轮次持久化 assistant 消息的 toolCalls，整批到达与持久化粒度一致，也避免 done 事件语义分叉。

**`AiMessage` 扩展**（嵌套字段用 OpenAI 原名，`#[serde(default)]` 保证旧请求兼容，① 已实现）：

```rust
pub(crate) struct AiMessage {
    pub role: String,
    #[serde(default)]
    pub content: Option<String>,                           // assistant tool_calls 轮次可为 null（协议规范形态）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,                 // assistant 消息携带
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,                      // role:"tool" 消息携带
}
// ToolCall { id, kind: "function"(rename type), function: { name, arguments } }
```

**前端模型扩展**（`lib/ai.ts`，② 已实现）：

```ts
// ① wire 形态（与 OpenAI 协议同构，透传后端）：
// AiWireMessage 扩展 role 'tool' / content: string | null / tool_calls? / tool_call_id?
// ② 持久化形态（随会话落盘，后端原样存取无需迁移；命名为 AiToolProposal 以区分 wire 类型）：
export interface AiToolProposal {
  id: string            // 模型返回的 call id
  name: string          // 工具名，M2 恒为 propose_command
  args: string          // 原始 JSON 字符串（arguments 聚合结果，wire 回传原样使用）
  reason?: string       // 模型给出的一句话用途，供提案卡片展示
  error?: string        // accepted 但写入失败时记录原因，wire 结果回传 error 让模型自行调整
  state: 'pending' | 'accepted' | 'rejected' | 'expired'
  // expired（② 实现中新增的第四态）：用户忽略 pending 卡片直接发送新消息时置为失效，
  // 卡片置灰不可操作；wire 仍须回传 tool 结果 {"accepted":false,"expired":true}
  // （协议要求每个 tool_call 都有应答，缺失会被上游 400）
}
// AiChatMessage 增加可选字段 toolProposals?: AiToolProposal[]
```

**前端驱动循环**（`AiChatPanel.vue`，② 已实现；授权交互为**混合形态**——底部授权弹窗管决策，流内记录条留痕）：

1. 收到 `ai-tool-calls` 事件：解析 `calls`（arguments JSON → command/reason），当前流式 assistant 消息追加 `toolProposals`（`state: 'pending'`）并先 `persist()` 防崩溃丢失；随后 `ai-done(completed)` 正常收尾本轮 UI（status/摘要）；
2. **提案消化调度器 `pumpProposals`**（`finishStreaming` 后触发）：pending 提案按到达顺序入队并逐个弹出**授权弹窗**（悬浮于输入框上方，视线必达）；弹窗展示命令原文 + 用途 + 三个编号选项（`1 仅本次执行 ↵ / 2 本会话允许 / 3 拒绝 Esc`），数字键/Enter/Esc 快捷裁决，多提案显示队列余量；**未裁决时 send 被拦截**（模态强制力，消掉 expired 浪费）；
3. 裁决（`applyProposal` 统一应用，两分支都先更新 `state` 并 `persist()` 防崩溃丢裁决）：
   - **仅本次 / 本会话允许**：授权弹窗本身已是确认 UI（选择 = 人工批准），**不叠加 §6.1 execConfirm 弹窗**；复用同一 `writeExec` 原语（目标会话 = 当前活动会话、多行仍受 `aiMultilineExec` 先决拦截、bracketed paste 规则一致）；写入成功 → 结果 `{"accepted":true}`；多行被拦 / 写入失败 → 记录条标红 `error` 且结果 `{"accepted":false,"error":"..."}`；「本会话允许」另将命令**精确匹配**记入会话级豁免表（内存 Map、按 scope 隔离、重启即失效；前缀白名单为后续扩展点），后续提案命中豁免表在 `onToolCalls` 阶段直接自动执行（无弹窗，记录条标注「已执行 · 会话允许」）；
   - **拒绝**（含 arguments 解析失败，不可执行按拒绝处理）：结果 `{"accepted":false,"rejected":true}`；
4. **流内记录条**（弱化后的 proposal 卡片，非交互）：assistant 消息正文之后的单条记录（终态徽标：等待授权 / 已执行 / 已执行 · 会话允许 / 执行失败 / 已拒绝 / 已失效 + 命令原文），裁决后永久留痕，回溯可审计；
5. 全部提案裁决完成（`awaitingContinue`）→ pump 推进轮次（`continueToolRound`）追加新的 assistant 流式消息再发起请求——`buildWireMessages` 扩展映射：assistant 提案消息展开为 `tool_calls` + 逐 call 的 `role:"tool"` 结果消息（`toolResultOf` 按状态构造），协议序列按序还原；回复组视觉合并（连续 assistant 消息共享一个 Logo 头，操作按钮仅组尾显示）；
6. 轮次上限：**单次用户输入最多 3 轮工具循环**（`MAX_TOOL_ROUNDS = 3`：覆盖「提案 → 执行 → 汇总」常规深度，超出视为模型异常，防请求失控）。超限时终止自动续跑并 toast 提示，用户可重新提问；
7. **`enableTools` 启用条件：工作台会话 scope（`scope !== 'global'`）**——全局对话无终端可写，不附加工具定义；续跑请求与首轮一致；scope 切换时 `resetProposalState` 清队列 + 遗留 pending 置为失效。

> **授权形态裁决**：对比过「纯流内卡片」（可见性依赖滚动位置、无键盘路径、二元选项）与「纯底部弹窗」（裁决后无痕、回溯弱）后采用混合形态——弹窗赢在决策时刻（必达 + 快捷键 + 会话允许提效项），记录条赢在决策之后（审计与叙事连续）。

**降级**：模型服务不支持 `tools`（HTTP 4xx）时，错误经既有 `ai-error` 路径提示；用户可继续纯对话（工具提案对该服务静默不可用）。自动去 tools 重发的优化留待真实服务兼容性问题出现后再做（避免为未知错误形态预设启发式）。

### 6.3 设置项

| 设置项 | 类型 / 默认值 | 位置 | 文案 |
|---|---|---|---|
| `aiMultilineExec` | `boolean` / `false` | 设置 → AI 模块 | 「允许多行命令直接执行」；描述：关闭时，含多行的命令仅可插入终端人工编辑 |
| `aiExecConfirmMode` | `select` / `'risky'` | 设置 → AI 模块 | 「直接执行确认」：每次确认 / 高危命令确认 / 不确认 |
| `aiRiskKeywords` | `string`（换行分隔）/ `''` | 设置 → AI 模块 | 「自定义高危关键词」：命令包含即视为高危（纯子串匹配，与内置规则叠加生效）；UI 为环境变量式动态行编辑（逐条输入框 + 删除 + 「+ 添加关键词」虚线按钮，`SettingsModal` 新增 `list` kind），存储仍是换行分隔 string，判定逻辑零改动 |

> **agent 权限档（M2.3 ③，已实施）**：会话级 `execPermission: 'manual' | 'risky' | 'full'`（输入框下拉，仅工作台 scope），与手动执行的全局设置相互独立——信任模型不同，但共用 `matchHighRisk` 判定原语。语义：`manual`=全部进授权弹窗（默认，行为与未加档位前完全一致）；`risky`=低危自动执行（`applyProposal 'once'` 路径）、高危仍弹窗；`full`=全自动。分流入口在 `onToolCalls`：豁免表命中（任何档位，用户显式豁免优先于高危判定）→ `risky` 且非高危 / `full` → 自动执行，否则照旧入队弹窗；弹窗组件、裁决、写入路径全部原样复用。会话级内存态、不持久化，切 scope 自动回落 `manual`（杜绝「上次开的 full 忘了关」）；放权态下拉黄色警示。

## 7. 安全与降级

- **隐私**：终端输出零回传；`write_terminal` 的字节流不入日志（既有约束）；工具提案仅包含模型生成的命令文本，不含任何终端内容；
- **写入即键盘**：命令原文不做 shell 转义或改写（转义会改变语义），确认弹窗展示完整原文，由用户裁决；
- **多行风险**：直接执行默认拦截（`aiMultilineExec`）+ 确认模式（默认 `risky`，内置规则 + 自定义关键词命中即确认）；多行直接执行与插入终端均走 bracketed paste（见 §6.1），整体进编辑缓冲区，不产生「边写边执行」的失控窗口；用户切至 `never` 后风险自担（与浏览器关闭下载确认同理）；
- **已修正的误判**：初版 §7 曾认为「插入终端不回车即不执行」对多行成立——实际多行裸写时中间 `\n` 即回车，会逐行执行，已改为 bracketed paste 包装（2026-10-11 实测发现）；多行直接执行同步由逐行裸写改为整体提交。
- **失败降级**：写入失败 toast 不重试；工具循环请求失败走既有 `ai-error` 通道，提案保持 `pending` 可再次手动裁决或重新生成；
- **竞态**：streaming 期间按钮与提案操作不可用（foot 隐藏沿用）；`loadSeq` 守卫沿用，scope 切换后旧裁决不落地（裁决函数内校验消息仍存在且 `state === 'pending'`）。

## 8. 测试与验证

- 单测（后端 `ai.rs`）：
  1. `delta.tool_calls` 跨 chunk 分片聚合（含多调用并行 `index` 交错）；
  2. `finish_reason=tool_calls` 与 `[DONE]` 双终止路径；
  3. `AiMessage` 含 `tool_calls` / `tool_call_id` 的 serde 往返与旧格式兼容（无新字段时请求体不变）。
- 单测（前端，落地形态取决于 vitest 引入，见 §9）：多行判定、目标会话解析、`buildWireMessages` 含 tool 消息的映射顺序。
- 手工验证清单：
  1. workbench 代码块 hover 出现两个按钮，首页不出现；streaming 期间不可见；
  2. 插入终端不回车；单行直接执行确认后执行；多行默认拦截且提示开关位置；开关开启后确认框带多行警示；
  3. 会话断开后写入 → `err` toast，不崩溃；
  4. `propose_command` 全链路：提案卡片 → 接受 → 命令写入 → 模型收到 `{"accepted":true}` 继续回复；拒绝分支同理；
  5. 应用重载后提案卡片按落盘状态还原，`pending` 仍可裁决；
  6. 构造连续提案验证 3 轮上限终止；
  7. 对不支持 tools 的服务端验证自动降级为纯对话。

### M3.1 补充：执行结果捕获与会话清理

- **执行结果自动回传**（`aiAutoReturnOutput`，默认关闭）：提案执行时 `sendInputAndCapture` 劫持 TermSink 订阅帧，提示符正则检测（bash/sh/zsh/fish 常见 PS1）+ 30s 超时，截断命令回显与提示符行后作为 tool 结果回传；超时附加提示。关闭时保持零回传（M2.1 隐私边界）。
- **会话断开脚本清理**：SSH 初始化脚本 `/tmp/.ri-<uuid8>` 的 `source` 自删仅在脚本成功执行后生效，会话在 source 前断开导致残留。`SshSession` 记录 `init_script_path`，`shutdown()` 时 spawn 异步任务经 exec 通道发送 `rm -f` 清理；失败记录 `ssh.session.init_script_cleanup_failed` 日志（尽力而为，不阻塞 shutdown）。

## 9. 未决问题

- 终端输出回传模型：已落地为 M3.1 开关方案（`aiAutoReturnOutput`，默认关闭）。开启后提案执行捕获输出作为 tool 结果回传，实现自主多步执行；关闭时保持零回传（隐私边界）。实现：`sendInputAndCapture` 劫持 TermSink 订阅帧，提示符正则检测 + 30s 超时，截断命令回显与提示符行。与原「显式选中发送」路径（终端右键 → AI 引用块）并行，用户按需选择颗粒度；
- 降级判定口径：HTTP 4xx 错误体匹配「不支持 tools」的具体关键词集需在实现时按主流兼容服务（vLLM / Ollama / DeepSeek 等）实测确定；
- `propose_command` 是否扩展 `working_dir` 等参数：M2 保持最小单参数，待真实使用反馈后再议；
- 前端测试框架（vitest）未引入，§8 前端单测的落地形态待定。
