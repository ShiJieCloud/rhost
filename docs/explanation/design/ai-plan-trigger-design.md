# AI 规划触发机制设计方案（模型自主提案 + 强制路径）

> description: 规划生成收敛为 propose_plan 单一路径；撤销独立 Plan 开关，默认由模型自主提案，强制路径经 tool_choice 保证
>
> created: 2026-10-14 15:58:18
>
> updated: 2026-10-14 18:21:04
>
> author:

---

## 1. 设计目标与硬约束

> 范围：本文覆盖 Plan-and-Act 的**触发机制改造**——撤销独立 Plan Toggle 开关，规划生成收敛为**单一路径**（后端内置工具 `propose_plan`，在当轮对话流内完成）；默认路径由模型自主判定任务复杂度并提案，强制路径（输入区「+」菜单「规划模式」项 / `/plan ` 前缀）与重规划通过「只下发该工具 + `tool_choice` 强制调用」实现。用户确认后复用既有逐步执行链路，简单任务不提案直接回答。
> 边界：不覆盖计划执行驱动（`confirmPlan` / `runPlanStep` / `failPlanStep` / `finishPlan`）、授权与权限档、消息渲染方案，这些**全部复用既有实现**；Replan 链路改造为「注入各步进展 + 对话轮内强制工具调用」，不再依赖独立 IPC；不覆盖 global scope（无工具即无提案）。
> 关联代码：`src-tauri/src/ai.rs`、`frontend/src/components/AiChatPanel.vue`、`frontend/src/lib/ai.ts`
> 修订关系：本文修订 [ai-plan-and-act-design.md](./ai-plan-and-act-design.md) 的 §4 非目标（「不做任务复杂度自动检测」）与 §6.1 入口（「Toggle 开关 + 斜杠命令二合一」）；触发权归属的决策记录见 [ADR-001](./decisions/ADR-001-model-driven-plan-trigger.md)。执行驱动、计划卡片视觉、注入预算、崩溃收敛等其余部分不变。

| 目标 | 量化口径 |
|---|---|
| 生成路径唯一 | 规划只由 `propose_plan` 工具产生（对话轮内）；下线 `ai_plan_generate` 独立 IPC、`PLANNER_PROMPT` 与 `chat_stream_collect`，前端不再有独立规划阶段 |
| 可见入口收敛 | UI 无独立模式开关按钮；强制规划入口为输入区「+」菜单的「规划模式」项（等价 `/plan ` 前缀命令）；默认路径不做前缀识别，规划提案由模型在对话轮内产生 |
| 强制路径确定性 | 强制 / 重规划轮**只下发 `propose_plan` 单一工具**并携带 `tool_choice` 强制调用，由服务端约束保证必然产出计划；供应商不支持 forced tool use 时降级为「未生成计划」提示并埋点 |
| 零额外调用 | 三种场景均复用当轮流式请求，不新增模型调用（现状独立 Planner 调用单次实测 33~60s，随 `ai_plan_generate` 下线一并消除） |
| 判定可用 | 单步/单文件/纯咨询任务不提案；≥3 步、跨文件或跨服务、需「先诊断后修改」的任务提案 |
| 确认不可绕过 | 计划必须经用户显式确认后才执行，无自动放行路径（既有硬约束不变） |
| 提案规模受控 | 步骤数 3~8、步骤标题 ≤50 字符，由工具 JSON Schema 与前端二次校验双闸约束 |
| 失败无害 | 工具参数解析失败、步数越界、模型未提案 → 100% 退回普通 ReAct，用户输入不丢失 |
| 判定可观测 | 提案 / 采纳 / 拒绝 / 强制 / 冲突 / 步失败等事件全部落本地 applog，可按 `event_id` 聚合出误判率与漏判候选率；字段仅枚举与数值，不含用户文本 |

必须遵守的既有项目约束：

- 后端只定义工具协议、不执行工具；工具循环（裁决 → 执行 → 续跑）由前端驱动（`ai.rs` `run_stream` 单轮透传，前端 `settleProposalRound` 推进）；
- 写操作授权语义不受计划影响：权限档（manual/risky/full）、会话级豁免表、授权弹窗在执行阶段照常生效；
- 计划必须经用户显式确认后才执行；
- AI 会话中止仅切断模型循环与 SSE 流，不回滚已执行的终端命令与文件修改；
- `ai-tool-calls` 事件载荷为扁平结构 `{ id, name, arguments }`，前端按扁平读取；
- 计划上下文注入 system 消息 ≤2KB UTF-8；技能/规则 16KB 预算与注入顺序不变，两者互不挤占；
- 消息渲染采用统一方案：每个回复组仅「执行过程折叠区 + 最终结果区」两个固定区域，末轮正文才晋升结果区；
- global scope 不附加工具（`enableTools` 仅在 `scope !== 'global'` 时为 true）。

## 2. 必须遵守的既有项目约束

- **工具定义后端内置**：`builtin_tools()` 返回 JSON 常量，前端不传工具 schema；新增工具只改后端一处（`ai-react-loop-design.md` §6.2 既有裁决）；
- **前端不拼命令/不解析工具语义**：`name` 为唯一路由键，参数以原始 JSON 字符串承载（`AiToolProposal.args`）；
- **提案状态机**：`AiToolProposal.state ∈ {pending, accepted, rejected, expired}`，`pending` 表示未裁决；`hasPendingProposal()` 对**任意** pending 提案返回 true，是发送拦截与续跑挂起的判据（`AiChatPanel.vue`）；
- **续跑触发条件**：`pumpProposals()` 仅在「无 pending 提案、无授权弹窗队列、无在飞 `autoRunning`」且 `awaitingContinue === true` 时推进轮次；`awaitingContinue` 由 `settleProposalRound()` 置位；
- **授权弹窗入队白名单**：`pumpProposals()` 只把写操作类工具（`propose_command` / `edit_file` / `write_file` / `sftp_upload_file` / `sftp_download_file`）的 pending 提案放入弹窗队列，其余工具不入队；
- **计划面板与授权弹窗互斥展示**：授权弹窗出现时计划悬浮面板让位隐藏（既有）；
- **计划随会话落盘**：`plan` 挂在生成它的 assistant 消息上，`persist()` 落盘；崩溃重载时 `executing` 计划的 `running` 步收敛为 `failed`；
- **同一时间只允许一个活动计划**：已有 `draft` / `executing` 计划时拦截新消息发送（toast「有计划进行中」）。

## 3. 现状盘点与差距

| 位置 | 现状 | 差距 |
|---|---|---|
| `AiChatPanel.vue` 发送入口 | `planMode` Toggle（消息级，发送后复位）+ `PLAN_CMD_RE = /^\/plan\s+/` 前缀检测；`usePlan` 分流到 `sendPlanned()` 调独立 IPC 生成计划 | 需删除 `planMode` 与 `sendPlanned()`；判据改为仅前缀命中，命中后转入**对话轮内强制工具调用**（`planForced` 入参 + system 注入），前缀来源为「+」菜单项 |
| `AiChatPanel.vue` 输入工具行 | `+` 附件按钮（`pickFiles()`）、Plan 开关按钮、规划期「取消规划」停止键分支 | `+` 按钮改为下拉菜单（上传附件 / 规划模式）；Plan 开关按钮删除，能力由菜单项承接；`planning` ref 与「取消规划」停止键**一并下线**（规划不再有独立阶段，由 `streaming` 与停止生成覆盖） |
| `AiChatPanel.vue` 工具路由 | `onToolCalls` 按 `name` 二分流：`propose_command` 走授权/豁免/自动执行，只读工具走 `autoExecReadonly` | 需新增第三分流：`propose_plan` → 建计划卡片（draft），不入授权弹窗队列 |
| `AiChatPanel.vue` 计划执行驱动 | `confirmPlan` / `runPlanStep` / `maybeAdvancePlan` / `completePlanStep` / `failPlanStep` / `finishPlan` / `retry|skip|abort` / `replan` 完整 | 无差距，直接复用 |
| `AiChatPanel.vue` 计划面板 | `activePlanMsg` / `planCollapsed` / `planHeadTitle` / `planBadgeText` / `planProgressText` / `acknowledgePlan` 完整 | 按钮语义需调整（draft 态「取消」改为「不用计划，直接执行」） |
| `src-tauri/src/ai.rs` | `builtin_tools()` 含 8 个工具（只读 3 + 写操作 3 + SFTP 2）；`ai_plan_generate` 独立规划 IPC + `PLANNER_PROMPT` + `chat_stream_collect` | 需新增 `propose_plan`（共 9 个）；`ChatRequest` 新增 `tool_choice`，`ai_chat_stream` 新增 tools 裁剪入参；**下线** `ai_plan_generate` / `PLANNER_PROMPT` / `chat_stream_collect` |
| `lib/ai.ts` | `aiPlanGenerate()` 封装 + `AiPlan` / `AiPlanStep` 类型完整 | **下线** `aiPlanGenerate()` 与 `AiPlanGenResult`；`aiChatStream` 新增 `planForced` 入参；`AiPlan` / `AiPlanStep` 保留 |

结论：本设计补齐四块——后端 `propose_plan` 工具定义与 `tool_choice` 支持、前端工具路由第三分流与计划卡片决策语义、输入区入口改造（`+` 按钮下拉化、Plan Toggle 与 `planMode` 移除）、规划生成路径收敛（下线 `ai_plan_generate` 独立 IPC，强制与重规划改走对话轮内强制工具调用）。

## 4. 目标与非目标

### 目标

1. 改造输入区入口：`+` 按钮改为下拉菜单（上传附件 / 规划模式），删除独立的 Plan Toggle 按钮与 `planMode` 模式状态；「规划模式」菜单项等价 `/plan ` 前缀命令（补前缀并聚焦），与手写 `/plan ` 共同构成强制规划入口；
2. 后端 `builtin_tools()` 新增 `propose_plan` 工具，参数为 `steps: [{title, detail}]`，经 JSON Schema 约束步数 3~8、标题 ≤50 字符；
3. 工具描述与 system 注入共同定义复杂度判据，引导模型「复杂才提案、简单直接回答」；
4. 前端 `onToolCalls` 新增 `propose_plan` 分流：解析参数 → 建 `AiPlan`（`state = draft`）→ 挂在当前 assistant 消息 → 悬浮计划卡片展示，等待用户决策；
5. `propose_plan` 提案以 `pending` 状态天然挂起续跑（无需新增守卫），用户决策后分流：确认 → 执行；不用计划 → 拒绝并续跑普通 ReAct；
6. 计划执行、失败处理、Replan、崩溃收敛、落盘全部复用既有实现；
7. 强制规划路径（「规划模式」菜单项 / `/plan ` 前缀）与 Replan 路径统一改走**对话轮内强制工具调用**：请求只下发 `propose_plan` 单一工具并携带 `tool_choice` 强制调用，产物与默认路径完全同构——同一 `AiPlan` 结构、同一计划卡片、同一确认 / 执行 / 失败处理 / Replan 链路；`ai_plan_generate` 独立 IPC 及其 `PLANNER_PROMPT` / `chat_stream_collect` 规划用法一并下线；未产出计划时降级为提示并埋点，用户输入不丢失。

### 非目标

- **不做独立的复杂度分类模型调用**：判定由主对话轮的模型自行完成，避免为每条消息增加一次前置调用（含思考模式数十秒延迟）；
- **不做自动执行计划**：确认仍是执行唯一入口，模型无权自行跳过确认；
- **不做多计划并行**：沿用「同一时间一个活动计划」拦截；
- **不改计划执行驱动与授权链路**：`confirmPlan` / `runPlanStep` / 权限档 / 豁免表 / 授权弹窗零改动；
- **不新增独立规划 IPC**：规划生成只有 `propose_plan` 一条路径，`ai_plan_generate` / `PLANNER_PROMPT` / `chat_stream_collect`（规划用法）全部下线（见 §6.6）；
- **不支持 global scope 提案**：无工具即无 `propose_plan`，与既有非目标一致；
- **不保留独立规划开关按钮**：规划能力不再以独立 Toggle 呈现，仅作为「+」菜单项（及等价的 `/plan ` 前缀命令）；不在输入区另加提示气泡或占位文案。

## 5. 总体架构

```text
用户消息（可见入口：+ 菜单「规划模式」项 / 直接写 /plan 前缀）
   ├─ 命中 `/plan ` 前缀 → 剥离前缀 → 强制规划路径（§6.1）
   │      → ai_chat_stream(planForced = true)：只下发 propose_plan + tool_choice 强制调用
   └─ 无前缀 → 常规路径
          → ai_chat_stream(planForced = false)：下发全部工具（含 propose_plan），模型自主判定
   │
   ▼
ai_chat_stream（builtin_tools 按 planForced 裁剪后随请求下发）
   │  流式正文 → 前端「执行过程」区
   │  模型决策：
   │    ├─ 任务简单 → 直接文本作答（无工具调用）→ 既有单轮晋升结果区，流程结束
   │    └─ 任务复杂 / 强制轮 → tool_calls 含 propose_plan
   ▼
onToolCalls 三分流（AiChatPanel.vue）
   ├─ propose_command / edit_file / write_file / sftp_* → 既有授权/豁免/自动执行路径（不变）
   ├─ read_file / list_directory / get_system_info → autoExecReadonly（不变）
   └─ propose_plan → 解析 steps → 校验（3~8 步、title 非空且 ≤50 字符）
        ├─ 与写操作类工具同轮（模型幻觉，仅常规路径可能）→ 作废计划（卡片仍可见步骤摘要与作废原因），
        │     写操作提案照常进授权弹窗；两者 tool 结果回传 → 续跑
        ├─ 校验失败 → 提案 rejected，tool 结果回传「计划参数无效，请直接执行」→ 续跑
        └─ 校验通过 → 建 AiPlan { goal, steps, state: draft, currentIndex: 0 }
                      提案置 pending → 悬浮计划卡片展示，等待用户决策
   ▼
用户在计划卡片决策
   ├─ [确认执行] → 提案置 accepted → confirmPlan() → plan.state=executing → runPlanStep(0)
   │                  （既有：每步独立 ReAct、计划 system 注入、失败操作、finishPlan 总结）
   └─ [不用计划，直接执行] → 提案置 rejected，plan.state=cancelled + acknowledged=true（面板直接消失）
                              toolResultOf 返回 {"accepted":false,"note":"用户未采纳计划，请直接执行任务"}
                              → settleProposalRound() → pumpProposals() → 续跑普通 ReAct
   ▼（某步失败后点 [重新规划]）
失败步 [重新规划] → 拼装「各步进展 + 失败步 detail 与原因」→ ai_chat_stream(planForced = true)
                    只下发 propose_plan + tool_choice → 新 draft 计划替换失败步及后续 → 再次确认
```

关键取舍——**用工具调用承载「复杂度判定 + 计划生成」，而非独立 IPC**：独立规划调用需先发一次额外请求（实测 33~60s）才能得到计划，且必须由用户预先声明「我要规划」；改由 `propose_plan` 工具承载后，判定与生成合并进当轮对话流，模型在一次响应内同时完成「判断复杂度」与「输出计划 JSON」，前端零额外延迟，用户零额外操作。

关键取舍——**提案天然挂起，不引入新的状态机**：`propose_plan` 提案以 `pending` 存入 `toolProposals`，而 `hasPendingProposal()`（发送拦截、续跑挂起）与 `pumpProposals()`（入队白名单）均按「任意 pending」工作——`propose_plan` 不在授权弹窗白名单内故不入队，其 pending 状态使 `awaitingContinue` 无法置位故不续跑。既有机制天然完成挂起，无需新增标志位。

## 6. 详细设计

### 6.1 输入区入口改造（+ 下拉菜单 + 强制规划路径）

**移除项**：

| 位置 | 移除项 | 说明 |
|---|---|---|
| `AiChatPanel.vue` 状态 | `planMode` ref、`planning` ref | Toggle 的模式载体与独立规划阶段标记；前缀判定即触发条件，规划不再有独立阶段（改由 `streaming` 与既有「停止生成」覆盖） |
| `AiChatPanel.vue` 发送逻辑 | `planMode.value = false` 发送后复位、`usePlan` 中的 `\|\| planMode.value` 分支 | `usePlan` 判据改为仅 `PLAN_CMD_RE.test(input)` |
| `AiChatPanel.vue` 模板 | Plan Toggle 按钮（`v-if="appView === 'workbench'"`）、「取消规划」停止键分支 | 独立开关与独立规划中断路径删除；能力分别由「+」菜单项与既有「停止生成」承接 |
| `AiChatPanel.vue` 函数 | `sendPlanned()`、`cancelPlanning()`、`replan()` 内的 `aiPlanGenerate()` 调用 | 强制规划与重规划统一改走 `ai_chat_stream(planForced = true)` |
| `AiChatPanel.vue` 样式 | `.plan-toggle` 三段规则 | 随按钮删除 |

**新增项（「+」下拉菜单）**：

| 位置 | 新增项 | 说明 |
|---|---|---|
| 模板 | `+` 按钮外层 `add-wrap` + `add-menu`（「上传附件」「规划模式」两项） | 交互复用权限档下拉既有模式：上弹定位、`mousedown` 点击外部关闭、`Escape` 关闭、`onUnmounted` 注销监听 |
| 状态/逻辑 | `addOpen`、`addWrapRef`、`onAddDocMouseDown`、`onAddKeydown` | 照 `.perm-menu` 实现 |
| 函数 | `pickFilesFromMenu()` | 关闭菜单后调用既有 `pickFiles()` |
| 函数 | `applyPlanCommand()` | 补 `/plan ` 前缀（已带前缀则不重复）→ 聚焦输入框、光标置末尾 |
| 计算属性 | `planArmed`（`draft.startsWith('/plan')`） | 菜单项激活态（蓝色 + ✓），与发送判据一致 |
| 发送逻辑 | `ai_chat_stream` 新增 `planForced` 布尔入参 | 命中前缀或重规划时传 `true`，后端据此只下发 `propose_plan` + `tool_choice` |
| 样式 | `.add-wrap` / `.add-menu` / `.add-item` / `.add-check` | 视觉对齐 `.perm-menu` / `.perm-item` |

**保留项（强制规划路径的触发判定）**：

| 位置 | 保留项 | 说明 |
|---|---|---|
| 发送逻辑 | `PLAN_CMD_RE = /^\/plan\s+/`、`input.replace(PLAN_CMD_RE, '')` 剥离、空内容守卫 | 剥离后文本作为计划目标（消息内容不变，仅工具集与 `tool_choice` 改变） |

**强制规划路径的语义**：

- 触发方式（两者等价，最终都表现为输入框带 `/plan ` 前缀）：`+` 菜单点「规划模式」→ `applyPlanCommand()` 补前缀；用户直接手写 `/plan `；此外某步失败点「重新规划」也进入该路径；
- 行为：剥离前缀（重规划则拼装进展描述）后走 `ai_chat_stream(planForced = true)`——**不依赖模型是否愿意调用 `propose_plan`**：请求只携带 `propose_plan` 单一工具 + `tool_choice = {"type":"function","function":{"name":"propose_plan"}}`，由服务端约束保证必然产出计划（确定性）；
- 产物同构：返回的 `propose_plan` 工具调用经与默认路径**完全相同**的 `onToolCalls` 分流与 `applyPlanProposal` 校验，建同一 `AiPlan(state=draft)` 计划卡片；确认 / 执行 / 失败处理 / Replan 复用同一链路；
- 失败降级：供应商不支持 forced tool use 或未产出合法计划时，发 `ai.plan.force_failed` 埋点并提示「未能生成计划，请重试或用普通方式描述任务」，**不回退独立 IPC**（该 IPC 已下线），用户输入经既有消息重发路径保留；
- 边界：单独 `/plan`（无内容）被既有空内容守卫拦下、不发请求；菜单在 `configHint` / `streaming` 期间因 `+` 按钮禁用而不可打开，global scope 的「规划模式」项置灰；
- 无冲突：强制轮只下发 `propose_plan` 一个工具，模型无其他工具可调用，故「计划 + 执行」同轮冲突在该路径下不可能发生（该冲突仅存在于常规路径）。

**其余保留不变**：发送前的「有计划进行中，请先处理」拦截、失效 pending 提案兜底转 `expired`、`toolRound.value = 0` 归零。

### 6.2 复杂度判定口径

判定由模型完成，判据以「工具描述 + system 注入」双处表述，不经用户侧开关（「+」菜单「规划模式」属强制路径，经 `tool_choice` 强制产出计划，不参与复杂度判定）。

**应提案（复杂任务）**：

- 目标需要 ≥3 个可独立验证的步骤才能完成；
- 涉及多个文件 / 多个服务 / 多条命令，且存在先后依赖；
- 需要「先诊断、后修改、再验证」的闭环（观察 → 判断 → 变更 → 确认）；
- 用户目标含多个并列动作（「并」「然后」「同时」「顺便」）。

**不应提案（简单任务）**：

- 单条命令、单次文件读取、单次目录列举、单次系统信息查询即可完成；
- 纯知识问答、纯咨询（不涉及远程操作）；
- 一次工具调用即可给出结论。

**互斥要求（三层防线）**：计划必须在行动之前确定，故 `propose_plan` 必须独占本轮。

1. **提示词层（主防线）**：工具 `description` 声明「调用本工具时本轮必须独占，禁止同时输出任何其他工具调用」；system 注入追加同义硬约束——「一旦决定调用 `propose_plan`，本轮禁止输出任何其他工具调用；若你已确定要立即执行某个动作（执行命令、修改文件），就不要调用 `propose_plan`，直接输出该动作」。两层叠加降低模型幻觉概率；
2. **前端冲突检测层（次防线）**：`onToolCalls` 按提案类型判定是否构成冲突。**只读工具（`read_file` / `list_directory` / `get_system_info`）并存不算冲突**——只读不改变服务器状态，其结果可作为计划的输入，照常自动执行，并存的计划提案照常建卡片；仅当与**写操作类**（`propose_command` / `edit_file` / `write_file` / `sftp_upload_file` / `sftp_download_file`）并存时判为冲突（判定逻辑见 §6.4）；
3. **冲突处理层（兜底）**：构成冲突时**作废计划、保留执行动作**——把被牺牲的对象从「模型想执行的动作」换成「计划提案」，使模型原本的写操作意图仍以提案卡片 + 授权弹窗形式呈现给用户裁决，不丢信息（§7）。

### 6.3 后端工具定义（`builtin_tools()` 扩展）

在既有 8 个工具后追加第 9 个：

```json
{
  "type": "function",
  "function": {
    "name": "propose_plan",
    "description": "当任务复杂时需要先规划再执行：目标需要 3 个以上可独立验证的步骤、涉及多个文件或多个服务且有先后依赖、或需要先诊断后修改再验证时，调用本工具提出执行计划，由用户确认后再逐步执行。简单任务（单条命令、单次读文件、单次查询、纯咨询）不要调用本工具，直接作答。【硬约束】调用本工具时本轮必须独占，禁止同时输出任何其他工具调用；若你已确定要立即执行某个动作（如执行命令、修改文件），就不要调用本工具，直接输出该动作。",
    "parameters": {
      "type": "object",
      "properties": {
        "steps": {
          "type": "array",
          "minItems": 3,
          "maxItems": 8,
          "description": "按执行顺序排列的计划步骤",
          "items": {
            "type": "object",
            "properties": {
              "title": { "type": "string", "description": "一句话描述该步骤要做什么，不超过 50 字符，须自描述、完整动作，作为 todo 展示给用户" },
              "detail": { "type": "string", "description": "执行参考：具体做什么、如何验证结果（不展示给用户，仅供执行时参考）" }
            },
            "required": ["title", "detail"]
          }
        }
      },
      "required": ["steps"]
    }
  }
}
```

设计要点：

- **`goal` 不由模型传**：计划目标取该轮 user 消息文本（前端从消息链取），避免模型改写用户原话导致卡片与注入文本失真；
- **步数与标题由 Schema 约束**：`minItems` / `maxItems` 给出模型侧硬约束，前端再按 §6.4 二次校验并截断；
- **描述即判据**：判据写进 `description`，使模型无需额外 system 段落即可遵循；system 侧仅在需要强调时补充（见 §6.4 备选）；
- 强制与重规划轮由 `planForced` 入参控制：为 `true` 时后端只返回 `propose_plan` 单一工具并附 `tool_choice`（§6.6）；
- `ai_plan_generate` / `PLANNER_PROMPT` / `chat_stream_collect`（规划用法）随本次改造下线，不再服务任何规划场景。

### 6.4 前端工具路由（`onToolCalls` 三分流）

```ts
onToolCalls: e => {
  if (e.scope !== scope.value || !streaming.value) return
  const last = messages.value[messages.value.length - 1]
  if (!last || last.role !== 'assistant') return

  last.toolProposals = e.calls.map(c => ({
    id: c.id,
    name: c.name,
    args: c.arguments ?? '',
    reason: c.name === 'propose_command' ? parseProposalArgs(c.arguments)?.reason : undefined,
    state: 'pending' as const,
  }))

  // 冲突检测：同一轮出现 propose_plan 与写操作类工具 → 判定为模型幻觉（§7 互斥兜底）。
  // 与只读工具并存不算冲突（只读不改变服务器状态，其结果可作为计划输入）
  const planProposal = last.toolProposals.find(p => p.name === 'propose_plan')
  const planConflict = !!planProposal &&
    last.toolProposals.some(p => WRITE_TOOLS.includes(p.name))

  for (const p of last.toolProposals) {
    if (p.state !== 'pending') continue
    if (p.name === 'propose_plan') {
      // 冲突时作废计划，把「执行动作」留给用户裁决；无冲突才建计划卡片
      if (planConflict) rejectPlanProposal(p, '本轮同时出现执行动作，规划已作废')
      else applyPlanProposal(last, p)
    } else if (p.name === 'propose_command') {
      // 既有授权/豁免/自动执行分流（不变）——冲突时照常入队弹窗，用户可见可裁决
    } else if (WRITE_TOOLS.includes(p.name)) {
      // 既有写操作工具分流（edit_file / write_file / sftp_*，不变）
    } else {
      autoExecReadonly(p) // 只读工具（不变）
    }
  }
  persist()
}
```

**`applyPlanProposal`**（新增）：

```ts
/** 计划提案：解析 steps → 校验 → 建 draft 计划；校验失败直接拒绝并交续跑兜底 */
function applyPlanProposal(msg: AiChatMessage, p: AiToolProposal) {
  const parsed = parseProposalArgs(p.args ?? '')
  const rawSteps = Array.isArray(parsed?.steps) ? parsed.steps : []
  const steps = rawSteps
    .map((s: any) => ({
      title: String(s?.title ?? '').trim(),
      detail: String(s?.detail ?? '').trim(),
    }))
    .filter(s => s.title)
    .slice(0, PLAN_MAX_STEPS)   // 8
    .map(s => ({
      id: uid(),
      title: s.title.slice(0, PLAN_TITLE_MAX_CHARS), // 50
      detail: s.detail,
      state: 'pending' as const,
    }))

  if (steps.length < PLAN_MIN_STEPS) { // 3
    p.state = 'rejected'
    p.error = '计划步骤不足，已按普通模式继续'
    settleProposalRound()
    return
  }

  const lastUser = [...messages.value].reverse().find(m => m.role === 'user')
  msg.plan = {
    goal: (lastUser?.content || '').trim() || '（含附件）',
    steps,
    currentIndex: 0,
    state: 'draft',
  }
  // p 保持 pending：计划卡片是决策 UI，用户决策后转 accepted / rejected
  scrollToBottom()
}
```

- **`goal` 取值**：最近一条 user 消息的 `content`（可能含 `<terminal-output>` 与 `<file>` 块）；为空（纯图片附件）时记「（含附件）」；
- **校验双闸**：Schema 约束模型侧，前端再校验步数下限、过滤空 title、截断超长 title（与工具 JSON Schema 同口径：`PLAN_MIN_STEPS = 3`、`PLAN_MAX_STEPS = 8`、`PLAN_TITLE_MAX_CHARS = 50`）；
- **不入授权弹窗队列**：`pumpProposals()` 的入队白名单不含 `propose_plan`，无需改动该函数；
- **挂起续跑**：提案保持 `pending` → `hasPendingProposal()` 为 true（发送拦截生效、`maybeAdvancePlan` 不推进）→ `awaitingContinue` 不会被置位 → 引擎静默等待用户决策。

**`rejectPlanProposal`**（新增，冲突兜底路径）：

```ts
/** 计划提案作废：不建计划卡片，提案置 rejected 并保留可读原因；
 *  同轮写操作提案不受影响，继续走既有授权链路 */
function rejectPlanProposal(p: AiToolProposal, reason: string) {
  p.state = 'rejected'
  p.error = reason
  // 与写操作提案的 tool 结果一并回传，由 settleProposalRound 统一推进续跑
}
```

- **可见性**：作废的 `propose_plan` 提案**不会被静默丢弃**，仍按提案卡片渲染——展示模型提案的步骤摘要（title 列表）与状态「已作废 · 本轮同时出现执行动作」，使「模型原本想怎么规划」「这次为什么没有计划」对用户可见；
- **执行动作不丢失**：同轮的 `propose_command` / `edit_file` / `write_file` / `sftp_*` 提案保持 `pending`，照常进入授权弹窗，用户可看到命令 / 变更原文并逐条裁决；这避免「静默丢弃模型想执行的动作」造成的信息丢失与困惑；
- **回传与续跑**：两类提案的 tool 结果一并回传（计划侧 `{"accepted":false,"note":"本轮同时输出计划与执行动作，计划已作废；已按你的工具调用继续"}`），`settleProposalRound()` 在写操作全部裁决落定后置 `awaitingContinue` → 续跑一轮，模型在收到明确约束反馈后重新决策（通常改为纯计划或纯执行）。

**计划卡片决策（`AiChatPanel.vue` 计划面板按钮语义调整）**：

| 计划态 | 按钮 | 行为 |
|---|---|---|
| `draft` | `[确认执行]` | 提案 `p.state = 'accepted'` → `confirmPlan(planMsg)`（既有：`plan.state = executing` → `runPlanStep(currentIndex)`） |
| `draft` | `[不用计划，直接执行]` | 提案 `p.state = 'rejected'`，`plan.state = 'cancelled'` 并直接置 `plan.acknowledged = true`（面板直接消失，不再弹二次确认），置 `planBannedInRound = true` → `settleProposalRound()` 触发续跑（该回合禁用再提案，见 §6.8） |
| `executing` 且有 failed 步 | `[重试] [跳过] [终止] [重新规划]` | 既有不变 |
| `done`（提前收敛）/ `cancelled`（执行中终止） | `[确认]` | 既有不变（`acknowledgePlan` 置 `acknowledged`，面板消失；普通 done 自动消失无按钮） |

- **「不用计划」的 tool 结果**：`toolResultOf()` 为 `propose_plan` 返回强化后的拒绝文案（见 §6.8 层 1），使模型在续跑轮以普通 ReAct 完成用户目标；该结果并非唯一防线——同时有机制层禁止再提案（§6.8 层 2），避免循环弹窗；
- **语义变更说明**：既有 draft 态「取消」按钮（`cancelPlan`，纯展示不续跑）被「不用计划，直接执行」取代——用户取消计划的通常动机是「这任务不值得规划，直接做」，若仅终止则用户需重新描述任务，体验断裂。确认与拒绝都要落盘。

### 6.5 执行衔接（复用既有）

- 用户确认 → `confirmPlan(planMsg)` → `plan.state = executing` → `runPlanStep(planMsg, 0)`：每步新起 assistant 流式消息、`toolRound` 归零、`planRunRef` 指向该步，`buildPlanSystem()` 注入「目标 + 进展 + 当前步 detail + 约束」（2KB 预算，逻辑不变）；
- 步骤落定判定 `maybeAdvancePlan` / `completePlanStep` / `failPlanStep` 不变；
- 全部落定 → `finishPlan` 总结轮（`enableTools=false`，纯文本收尾）不变；
- 步骤内写操作仍走授权弹窗、权限档、豁免表；会话断开时该步转 `failed` 等用户决策；
- 承载计划的 assistant 消息即 `propose_plan` 那一轮消息（无正文或仅一句说明），与既有「Planner 占位消息」等价；确认前结果区为空，执行过程区保留该轮工具卡片与正文。

### 6.6 三种生成场景与统一路径

| 场景 | 使用路径 | 工具下发与 `tool_choice` | 理由 |
|---|---|---|---|
| 首次提案（默认） | `propose_plan` 工具（当轮对话流） | 全量工具，无 `tool_choice` | 判定与生成合并，零额外调用 |
| 首次提案（用户强制） | 「+」菜单「规划模式」项 / `/plan ` 前缀 → `ai_chat_stream(planForced = true)` | 只下发 `propose_plan`，`tool_choice` 强制调用 | 用户意图直接生效（确定性），不经模型判定；见 §6.1 |
| 失败后重新规划 | failed 步点「重新规划」→ 拼装进展描述为 user 消息 → `ai_chat_stream(planForced = true)` | 只下发 `propose_plan`，`tool_choice` 强制调用 | 长上下文（各步进展 + 失败原因）随消息注入，工具调用只产出 JSON，无需独立 IPC |

三种场景共用同一工具协议、同一 `applyPlanProposal` 校验与同一计划卡片，差异仅在「工具集裁剪 + `tool_choice`」。重规划结果仍替换失败步及后续步骤、`plan.state` 回 `draft` 再次确认——后续确认 / 执行 / 失败处理链路零改动。

`ai_plan_generate` / `PLANNER_PROMPT` / `chat_stream_collect`（规划用法）随之全部下线，规划生成只剩 `propose_plan` 一条路径。

### 6.7 魔数取值

| 魔数 | 取值 | 理由 |
|---|---|---|
| 步骤数 | 3~8 | 沿用既有：<3 步普通 ReAct 即可；>8 步注入膨胀且长程规划质量下降。同时写入 Schema `minItems`/`maxItems` 与前端校验 |
| 步骤标题 | ≤50 字符 | 卡片单行展示；Schema 描述 + 前端截断双闸 |
| 步骤结果摘要 | ≤150 字符 | 沿用既有：8 步 × 150 = 1.2KB，加目标与标题后 < 2KB 预算 |
| 计划注入预算 | 2KB UTF-8 | 沿用既有，不挤占技能/规则 16KB 预算 |
| 每步工具轮次 | 6（`MAX_TOOL_ROUNDS`） | 沿用既有，每步独立计数 |
| 强制轮工具集 | `["propose_plan"]`（单元素） | 强制 / 重规划轮 `planForced = true` 时只下发该工具，从源头消除同轮冲突与循环提案 |
| `tool_choice` 取值 | `{"type":"function","function":{"name":"propose_plan"}}` | 服务端约束模型必然调用该工具；供应商不支持时按 §7 降级提示并埋点 `ai.plan.force_failed` |

### 6.8 循环提案防护（拒绝后的禁止再提案）

**风险链路**：用户点「不用计划，直接执行」→ tool 结果回传 → 模型续跑 ReAct → 模型仍判定任务复杂 → 再次调用 `propose_plan` → 再度弹出计划卡片 → 用户再拒 → 循环弹窗。仅靠 tool 结果里的自然语言约束不足以杜绝这类幻觉循环，需三层防护。

**层 1 · 提示层（弱约束）**：`toolResultOf()` 对 `propose_plan` 的 rejected 结果强化为：

```json
{"accepted":false,"rejected":true,"note":"用户已拒绝本次规划提案。禁止再次调用 propose_plan，请直接使用可用工具完成任务，不要再提案。"}
```

**层 2 · 机制层（强约束，物理移除工具）**：前端维护回合级标记，后端据此在组装 tools 时剔除 `propose_plan`。

| 时机 | 动作 |
|---|---|
| 用户点「不用计划，直接执行」 | `planBannedInRound.value = true` |
| 每次发起 `ai_chat_stream` | 传布尔入参 `disablePlanProposal = planBannedInRound.value` |
| 本次任务回合结束（流结束且不再续跑） | `planBannedInRound.value = false`（恢复可提案） |

- 后端新增布尔入参 `disable_plan_proposal`（默认 `false`，沿用既有 `enable_tools` 的传参模式）；为 `true` 时 `builtin_tools()` 不返回 `propose_plan` 条目（与 §6.1 的 `planForced` 共用「按布尔入参裁剪工具列表」同一实现）；
- **该回合模型上下文中不存在此工具，物理上无法调用**——比「拦截已发出的调用」更强；且不违反「后端只定义工具协议、不执行工具」的分层（后端只做组装裁剪，工具循环仍由前端驱动）；
- 标记为**回合级**而非单请求级：若只在下一轮请求发出后清除，多轮工具循环的后续轮次仍可再提案，因此需覆盖到整回合结束再复位。

**层 3 · 前端防御性兜底**：若标记生效期间仍收到 `propose_plan`（层 2 生效时理论不应发生，用于兜住传参遗漏等缺陷），`onToolCalls` 直接 `rejectPlanProposal(p, '本轮已禁用计划提案')`，不建卡片，tool 结果回传层 1 文案，避免产生第二张卡片。

**范围界定**：标记仅由「用户显式拒绝」触发。「与写操作工具同轮」的计划作废场景只应用层 1 提示强化，不设标记——该场景模型并非被用户拒绝，避免过度限制其后续合理提案。

### 6.9 判定质量埋点与调优闭环

判定质量（误判 / 漏判）此前无观测手段，`description` 里的复杂度判据只能凭主观调整。本节定义埋点，使判据迭代有数据依据。

**埋点落地**：复用既有 applog（见 [applog-design.md](./applog-design.md)），不新增存储与上报通道。

- `event_id` 沿用既有 `domain.action[.subaction]` 格式，新增 `ai.plan.*` 一组；后端 `src-tauri/src/applog/events.rs` 注册常量，前端 `frontend/src/stores/applog.ts` 同步同一份表，并**同步更新 `applog-design.md` 的事件清单**（该文档「集中定义」小节要求三处一致）；
- 事件产生点位于前端裁决处（`AiChatPanel.vue`），经既有「前端日志上报后端」通道进入 Hub，单一时钟、单一 `seq`；落盘受既有 `logPersist` 设置控制；
- 聚合方式沿用既有做法：`jq 'select(.event_id=="ai.plan.refused")'` 即得拒绝次数。

**事件清单**（字段仅枚举 / 数值 / 布尔）：

| event_id | 触发时机 | 字段 |
|---|---|---|
| `ai.plan.proposed` | 模型提案建卡成功 | `step_count`、`tool_round`、`has_write_in_round`（同轮是否含写操作工具） |
| `ai.plan.invalid` | `steps` 校验失败 | `fail_reason`（`parse` / `steps_missing` / `too_few`）、`raw_step_count` |
| `ai.plan.confirmed` | 用户点「确认执行」 | `step_count`、`latency_ms`（建卡 → 点击） |
| `ai.plan.refused` | 用户点「不用计划，直接执行」 | `step_count`、`latency_ms`（建卡 → 点击） |
| `ai.plan.forced` | 强制规划路径触发（「+」菜单「规划模式」项 / 手写 `/plan ` / 「重新规划」） | `via`（`menu` / `prefix` / `replan`）、`task_had_proposal`（同一任务此前是否已提案） |
| `ai.plan.force_failed` | 强制 / 重规划轮未产出合法计划（`tool_choice` 未生效或步骤校验失败） | `reason`（`no_tool_call` / `parse` / `too_few`） |
| `ai.plan.conflict_dropped` | 与写操作工具同轮、计划作废 | `other_tools`（工具名枚举数组） |
| `ai.plan.reproposal_blocked` | 禁令生效期仍收到提案（§6.8 层 3 命中） | 无 |
| `ai.plan.step_failed` | 计划步骤失败 | `step_index`、`fail_reason`（`aborted` / `exec_error` / `timeout` / `disconnect` / `round_limit`） |
| `ai.plan.replanned` | 用户点「重新规划」 | `step_index` |
| `ai.plan.aborted` | 用户点「终止」 | `step_index` |
| `ai.plan.finished` | 总结轮完成 | `step_count`、`done_count`、`failed_count`、`duration_ms` |
| `ai.plan.concluded` | 执行阶段提前收敛（详见 [ai-plan-early-convergence-design.md](./ai-plan-early-convergence-design.md) §6.7） | `outcome`（`resolved` / `blocked`）、`step_index`、`step_count`、`skipped_count`、`duration_ms` |

**指标口径**：

| 指标 | 计算 | 反映 |
|---|---|---|
| 误判率 | `refused` / `proposed` | 判据过宽（简单任务也被提案） |
| 误判确信度 | `refused.latency_ms` 中位数 | 越短越像「一眼假」的误判 |
| 漏判候选率 | `forced` 中 `task_had_proposal = false` 的占比 | 判据过窄（复杂任务未提案，用户被迫手动强制） |
| 冲突率 | (`conflict_dropped` + `reproposal_blocked`) / `proposed` | 提示词独占约束是否被模型遵守 |
| 计划健康度 | `step_failed` 数 / 总步数；`replanned`、`aborted` 率 | 计划粒度是否恰当（过粗 / 过细） |
| 提前收敛构成 | `concluded` 按 `outcome` 拆分（`resolved` / `blocked`） | `blocked` 属合法降级；`resolved` 中 `step_index` 偏小、`skipped_count` 偏大的比例才是「偷懒式收敛」的嫌疑 |

**调优闭环（阈值触发动作）**：

| 观测信号 | 建议阈值 | 调整动作 |
|---|---|---|
| 误判率 | > 20% | 收紧判据：抬高「应提案」门槛（如要求 ≥4 步或明确跨服务） |
| 漏判候选率 | > 10% | 放宽判据：把命中场景类型补入 `description` 的「应提案」列表 |
| 冲突率 | > 5% | 强化 `description` 独占约束措辞，或调整工具在列表中的位置 |
| 计划健康度（步失败率） | > 20% | 调整 `description` 对 `detail` 的引导（要求可验证的单步动作） |

**隐私边界（硬约束）**：

- 字段**只含枚举、数值、布尔**——不含用户消息文本、计划 `goal` / 步骤 `title` / `detail`、命令原文、文件路径、主机地址；
- 复用 applog 既有安全规则（永不记录密码、私钥、PTY 数据流、剪贴板、文件内容）；
- **仅本地落盘**，不做任何远端上报，不新增网络通道。

## 7. 安全与降级

- **授权语义不变**：计划只决定「做什么」，不放大「能做什么」——写操作仍按权限档与豁免表处理，弹窗照常；模型不遵守计划约束时，最终防线是既有授权链路；
- **确认不可绕过**：`propose_plan` 提案无自动接受路径；`pumpProposals()` 入队白名单不含该工具，任何权限档都只能经由卡片上的人工点击进入执行态；
- **入口收敛后的越权面**：删除显式入口不改变工具集，模型无法在 global scope 提案（`enableTools=false`），也无法借 `propose_plan` 触发任何远端执行（该工具只产出计划文本）；
- **互斥兜底（三级防线）**：模型一旦出现「计划 + 执行」混合输出，按**保留执行动作、作废计划**处理，两边的信息都不丢——
  1. 提示词层：工具 `description` 与 system 双处声明 `propose_plan` 必须独占本轮（§6.2）；
  2. 前端冲突检测：仅当与**写操作类**工具并存时判为冲突，与只读工具并存放行；
  3. 冲突处理：计划提案置 `rejected` + error「本轮同时出现执行动作，规划已作废」，且**该提案卡片仍渲染**（步骤摘要 + 废弃原因可见）；写操作提案**保持 `pending`** 照常进入授权弹窗，由用户看到命令 / 变更原文并逐条裁决；两者 tool 结果一并回传后续跑一轮，模型在明确约束反馈下重新决策；
- **隐私边界**：计划目标、步骤 detail、结果摘要随对话上下文发往模型服务商，与用户显式发送文本同口径；计划内容不写入应用日志；终端内容仍仅以用户显式选中发送为入口；
- **降级矩阵**：

| 场景 | 行为 |
|---|---|
| 模型未提案（判为简单） | 直接文本作答，既有单轮晋升结果区，无计划卡片 |
| `steps` 参数解析失败 / 非数组 | 提案 `rejected` + error，`toolResultOf` 回传「计划参数无效，请直接执行」，续跑普通 ReAct |
| 有效步骤数 <3（过滤空 title 后） | 同上，退回普通 ReAct，用户输入不丢失 |
| 有效步骤数 >8 | 前端截断至 8 步（Schema 已约束，此处兜底） |
| `propose_plan` 与写操作工具同轮（模型幻觉，仅常规路径） | 计划提案 `rejected`（卡片可见步骤摘要与作废原因），写操作提案保持 `pending` 进入授权弹窗；两者 tool 结果一并回传后续跑，用户可照常裁决执行 |
| 用户点「不用计划」 | `plan.state=cancelled` + 提案 `rejected` → 续跑，模型直接执行任务 |
| 用户点「确认执行」 | 进入既有执行链路，失败处理/重试/跳过/终止/重规划全部可用 |
| 模型误判为简单（漏判） | 无计划卡片，退回普通 ReAct；用户可用「+」菜单「规划模式」或 `/plan ` 强制规划（不依赖模型配合） |
| 强制 / 重规划轮未产出合法计划（`tool_choice` 未生效或校验失败） | 发 `ai.plan.force_failed` 埋点，提示「未能生成计划，请重试或用普通方式描述任务」；**不回退独立 IPC**（已下线），用户输入保留待重发 |
| 用户点「不用计划」后模型再次想提案 | 该回合 `propose_plan` 未下发（§6.8 层 2），模型无该工具可调；若仍出现则前端再次拒绝（层 3），不产生第二张卡片 |
| 会话断开 | 既有降级不变：该步转 `failed` 等用户决策，不自动重试 |
| 崩溃重载 | 既有收敛不变：`executing` 计划的 `running` 步转 `failed`；`draft` 计划恢复可确认 |
| 切换 scope | 既有收敛不变：`executing` 计划转 `cancelled`，`running` 步转 `failed`，在飞 exec 经 `ai_cancel_execs` 取消 |

## 8. 测试与验证

- **后端单测**（`ai.rs` `#[cfg(test)]`）：
  1. `builtin_tools_shape`：工具数扩展为 9，验证新增 `propose_plan` 且 `parameters` 含 `steps` 数组约束（`minItems=3` / `maxItems=8` / `required: [title, detail]`）；
  2. `parse_plan_json` 相关用例随 `ai_plan_generate` 下线一并移除；新增 `planForced` 裁剪用例：`planForced=true` 时 `tools` 为单元素 `[propose_plan]` 且 `tool_choice` 指向该工具，`planForced=false` 时返回全量工具且无 `tool_choice`；
- **前端验证**：
  1. 输入区入口：工具行无独立 Plan 开关按钮；`+` 按钮点击弹出「上传附件 / 规划模式」菜单（点击外部与 ESC 可关闭）；点「规划模式」→ 输入框补 `/plan ` 前缀、聚焦且光标至末尾、菜单项显示激活态；普通消息无前缀识别；`/plan 部署 nginx 并验证` 发送 → 剥离前缀 → `ai_chat_stream` 入参 `planForced=true`（校验入参）→ 后端只下发 `propose_plan` 且 `tool_choice` 指向该工具 → 生成 `draft` 计划卡片；单独 `/plan` → 被空内容守卫拦下不发送；无独立「取消规划」按钮，由既有「停止生成」中断；
  2. 路由三分流：同一轮 `propose_plan` + 只读工具混合 → 不算冲突（只读正常执行 + 计划卡片照常生成）；`propose_plan` + `propose_command` 混合 → 判为冲突（计划提案 rejected 且卡片可见作废原因、命令提案保持 pending 进入授权弹窗、两者 tool 结果回传后续跑）；
  3. 简单任务：模型无工具调用 → 单轮直接作答，无计划面板；
  4. 计划提案：`draft` 计划面板出现（标题为 `goal`、进度「N 步 · 待确认」），步骤无空 title；
  5. 挂起：`draft` 计划存在时发送新消息被拦截（toast「有计划进行中」）；
  6. 确认：点「确认执行」→ 计划转 `executing`、步骤状态图标实时更新、计划 system 注入与快照一致；
  7. 拒绝与防循环：点「不用计划，直接执行」→ 计划转 `cancelled`、提案转 `rejected`、`planBannedInRound` 置位 → 续跑请求带 `disablePlanProposal: true`（校验入参）→ 该回合后端下发的工具列表无 `propose_plan` → 只产生一条普通 ReAct 回复，无第二张计划卡片；回合结束后标记复位，下个回合的复杂任务可正常提案；
  8. 参数异常：构造 `<3` 步或非法 JSON 的 `steps` → 提案 rejected + 续跑，无计划面板；
  9. 崩溃收敛：构造 `draft` / `executing` + `running` 落盘记录 → 重载后分别可确认、转 `failed`；
  10. 切换 scope：`executing` 计划切走 → `cancelled`，在飞 exec 被取消；
  11. 埋点：走一遍提案 → 确认 / 拒绝 / 强制 / 强制失败 / 冲突作废 / 步失败路径，校验 `ai.plan.*` 事件按 §6.9 字段落盘（applog 面板可按 `event_id` 筛出），且事件中不含消息正文、计划标题、命令原文。
- **手工验证清单**：
  1. 工作台输入「查看一下磁盘空间」→ 模型直接回答或调只读工具，无计划卡片（简单任务不提案）；
  2. 工作台输入「部署 nginx，修改配置并重启，然后验证」→ 出现计划卡片（3~8 步）→ 编辑某步 title → 确认 → 逐步执行，状态图标实时更新；
  3. 同一条复杂任务上点「不用计划，直接执行」→ 计划转已取消，模型随即按普通 ReAct 完成并给出结论；
  4. 某步写操作触发授权弹窗（manual 档）→ 裁决后该步继续，计划推进不受干扰；
  5. 制造失败步 → 重试 / 跳过 / 终止 / 重新规划各分支行为正确（重规划走 `ai_chat_stream(planForced = true)` + `tool_choice`）；
  6. 全部完成 → 总结轮输出，无工具提案；
  7. 首页（global scope）输入复杂任务 → 只回答不提案（无工具）。

## 9. 未决问题

- **漏判 / 误判补救路径（已决）**：用户侧强制规划入口为「+」菜单「规划模式」项（等价 `/plan ` 前缀，无独立开关按钮）。模型误判复杂任务为简单时，用户可主动强制规划；用户本想规划而模型未提案时，有明确可见的入口可用，不会出现「命令无效」的困惑。分工：默认路径 = 模型判定（零额外调用、零操作），强制路径 = 用户触发（确定性、不依赖模型意愿）。剩余待观测的是漏判率本身，用于后续调优判据文案；
- **`/plan ` 前缀的可发现性**：可见入口已收敛为「+」菜单项，直接手写前缀属高级用法，不做额外 UI 提示；若后续需在文档（`docs/guides/`）中列出等价命令，按需补充；
- **埋点落盘范围（已决）**：§6.9 定义本地 applog 埋点（可按 `event_id` 聚合、可导出），**仅本地落盘**，不做远端上报、不做面板图表；跨版本趋势分析待后续按需评估，且不引入任何含用户文本的字段；
- **强制路径的供应商兼容**：`tool_choice` 为 OpenAI 兼容协议字段，若某供应商不支持 forced tool use，按 §7 降级为提示 + `ai.plan.force_failed` 埋点，**不回退独立 IPC**（已下线）；后续如需提高兼容性，可在服务端做一次性重试；
- **判据可配置**：复杂度判据当前硬编码在 `description`。若不同用户群体（如 SRE 与普通开发者）对「复杂」的预期差异大，可能需要设置项调节；MVP 不做；
- **ADR 归档（已归档）**：本设计推翻了 [ai-plan-and-act-design.md](./ai-plan-and-act-design.md) 的既有非目标（「不做任务复杂度自动检测与自动进入规划模式」）与入口设计，属跨模块长期决策，已按 [docs-spec.md](./docs-spec.md) §8 归档为 [ADR-001](./decisions/ADR-001-model-driven-plan-trigger.md)（规划触发权由用户显式开关改为模型自主提案）。
