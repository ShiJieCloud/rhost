# AI Plan-and-Act（显式推理链编排）设计方案

> description: 规划模式：Planner 生成可编辑计划，确认后按步骤独立 ReAct 循环执行，状态全程可视
>
> created: 2026-10-12 18:05:08
>
> updated: 2026-10-12 18:21:10
>
> author:

---

## 1. 设计目标与硬约束

> 范围：本文覆盖工作台会话的「规划模式」完整链路——计划生成（Planner）、计划卡片预览/编辑/确认、每步独立 ReAct 循环执行（Act）、步骤状态机与卡片可视化、失败处理与手动重规划（Replan）、全部落定后的总结（finishPlan）。
> 边界：不覆盖任务复杂度自动检测、步骤间依赖图与并行执行、自动重规划、global 纯推理计划（无工具场景）。
> 关联代码：`src-tauri/src/ai.rs`、`frontend/src/lib/ai.ts`、`frontend/src/components/AiChatPanel.vue`

**核心定位**：计划 = 给 ReAct 的**目标清单与约束**，不是不可修改的死脚本——draft 态步骤文本可编辑，确认后作为约束注入（禁止擅自新增计划外动作）；同时计划也是执行的**编排骨架**——每步独立 ReAct 循环推进，步骤状态全程可视化。

| 目标 | 量化口径 |
|---|---|
| 计划可审可改 | 计划必须经用户显式确认后才执行；draft 态步骤文本可编辑，无自动放行路径 |
| 步骤规模受控 | 单计划 3~8 步；步骤标题 ≤50 字符；步骤结果摘要 ≤150 字符 |
| 注入有界 | 计划上下文注入 system 消息总预算 2KB UTF-8，超出丢弃最旧步骤摘要 |
| 步骤隔离 | 每步独立 ReAct 循环，工具轮次复用 MAX_TOOL_ROUNDS=6，步间计数互不影响 |
| 低开销 | 计划生成为 1 次非流式调用；生成/解析失败降级为普通 ReAct 对话，用户输入不丢失 |

必须遵守的既有项目约束：见 §2。

## 2. 必须遵守的既有项目约束

- 后端只定义工具协议、不执行工具；工具循环（裁决 → 执行 → 续跑）由前端驱动（`ai.rs` `run_stream` 单轮透传，前端 `settleProposalRound` 推进）；
- 写操作（`propose_command` / `edit_file` / `write_file`）授权语义不受计划影响：权限档（manual/risky/full）、会话级豁免表、授权弹窗在执行阶段照常生效；
- AI 会话中止仅切断模型循环与 SSE 流，不回滚已执行的终端命令与文件修改，已落定状态保留；
- `ai-tool-calls` 事件载荷为扁平结构 `{ id, name, arguments }`，前端按扁平读取；
- 文件编辑工具 64KB 内容上限、系统关键文件黑名单、写入前自动备份的约束不变；
- 隐私边界：计划内容与步骤结果作为上下文发往模型服务商，与用户显式发送文本同口径；终端内容仍以用户显式选中发送为唯一入口；
- 会话切换时经 `ai_cancel_execs` 取消在飞 exec 任务，提案状态经 `resetProposalState` 收敛——计划执行态必须接入同一收敛路径；
- 消息持久化 `persist()` 将 streaming 收敛为 stopped，计划执行态崩溃恢复遵循同一收敛语义。

## 3. 现状盘点与差距

| 位置 | 现状 | 差距 |
|---|---|---|
| `src-tauri/src/ai.rs` | `run_stream` 单轮流式透传工具提案；`ai_summarize` 提供非流式单次调用先例 | 无计划生成 IPC，无 Planner prompt 与 JSON 解析 |
| `frontend/src/components/AiChatPanel.vue` | ReAct 循环（`toolRound` / `MAX_TOOL_ROUNDS=6`）、工具路由、授权弹窗、推理可视化齐备 | 无规划入口、无计划状态机与执行驱动、无步骤卡片 UI、无计划上下文注入 |
| `frontend/src/lib/ai.ts` | `aiChatStream` / `aiExecTool` / `aiSummarize` 等 IPC 封装完整 | 无 `aiPlanGenerate` 封装与计划类型定义 |
| 消息模型 `AiChatMessage` | 含 `toolProposals`、`reasoning` 等字段，随 `persist()` 落盘 | 无 `plan` 字段承载计划状态 |

结论：本设计补齐五块——后端计划生成 IPC（含宽松 JSON 解析）、前端规划入口（开关 + 斜杠命令）、计划状态机与逐步执行驱动、计划卡片（编辑/状态可视化/失败操作）、计划上下文注入。

## 4. 目标与非目标

### 目标

1. 两种入口任选：输入框 Toggle 开关（消息级，提交后自动复位）与斜杠命令 `/plan `，斜杠命令优先级更高；
2. Planner 生成 3~8 步计划，以卡片展示，用户可预览、编辑步骤文本、确认或取消；
3. 确认后逐步执行：每步注入计划上下文并独立跑一轮 ReAct 循环（复用现有工具路由与授权链路），步骤状态（pending/running/done/failed/skipped）在卡片上实时更新；
4. 步骤失败时用户可选择：重试、跳过、终止、重新规划（Replan 生成剩余步骤后再次确认）；
5. 全部步骤落定后自动收尾：模型基于各步结果摘要给出总结回复（finishPlan）；
6. 计划状态随会话落盘，应用重启后恢复展示，中断步骤收敛为 failed。

### 非目标

- 不做任务复杂度自动检测与自动进入规划模式（规划入口永远由用户显式触发）；
- 不做自动重规划（失败后的重规划必须经用户显式触发并再次确认）；
- 不做步骤间依赖图与并行执行（MVP 仅线性顺序执行）；
- 不支持 global scope（无终端无工具，计划无执行载体）。

## 5. 总体架构

```text
入口检测（优先级：/plan 命令 > Toggle 开关 > 默认 ReAct）
   │
   ▼
ai_plan_generate（后端非流式：Planner prompt + 用户问题 → JSON 步骤数组）
   │ 失败 → toast + 自动按普通 ReAct 发送（用户输入不丢失）
   ▼
计划卡片（plan.state=draft）：步骤文本可编辑 → [确认执行] [取消]
   │ 用户确认（以编辑后文本生成快照并落盘）
   ▼
plan.state=executing ──► 步骤 i 循环（step: pending→running）
   │   ├─ toolRound 归零（每步独立计数）
   │   ├─ buildWireMessages 注入计划 system 消息
   │   │   （目标 + 步骤进展 + 当前步 detail + 约束条款）
   │   ├─ runAiStream（现有 ReAct：工具路由/授权弹窗/权限档/豁免表）
   │   └─ 该步落定：assistant done 且无未裁决提案（复用 settleProposalRound 推进链）
   │
   ├─ 步骤完成：提取结果摘要（≤150 字符）→ step=done → 下一步
   ├─ 步骤失败/中止/轮次超限：step=failed → 卡片操作 [重试] [跳过] [终止] [重新规划]
   │     └─ 重新规划：进展+失败原因 → ai_plan_generate → 替换剩余步骤 → draft 再确认
   ▼
全部步骤落定 → plan.state=done → finishPlan 总结回复
   （注入各步摘要，enableTools=false，纯文本一轮收尾）
```

与既有模块的关系：Planner 是独立非流式调用（复用 `ai_summarize` 的请求链路）；Act 阶段完全复用现有 ReAct 循环（`runAiStream` / `onToolCalls` / `settleProposalRound` / 授权弹窗），不新增工具、不改授权语义，增量仅为计划上下文注入与步骤推进编排。

## 6. 详细设计

### 6.1 入口（二合一）

| 入口 | 形态 | 状态粒度 |
|---|---|---|
| Toggle 开关 | 输入框左下「Plan」开关 | 消息级：仅作用于本次提交，发送完成自动复位关闭；切 scope 自动关闭 |
| 斜杠命令 | 消息以 `/plan ` 开头（`/plan` 后须有空格） | 单条消息：检测到前缀即剥离前缀、按规划模式处理，忽略开关状态 |

- **优先级**：斜杠命令 > Toggle 开关 > 默认（ReAct）；斜杠命令在开关关闭时也生效；
- 仅工作台 scope 可用：global scope 下开关置灰（hover 提示「全局对话无终端可执行计划」），`/plan ` 前缀按普通文本发送；
- 发送后进入「规划中…」loading（输入区禁用 + 可取消按钮），期间调 `ai_plan_generate`；取消走 `aiStopChat` 同源中止（`AiState.cancels` 同 scope 键）。

### 6.2 后端 IPC：`ai_plan_generate`（`src-tauri/src/ai.rs`）

```rust
/// 计划生成结果（serde camelCase）
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanGenResult {
    /// 步骤列表（3~8 步，已校验）
    steps: Vec<PlanStepOut>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanStepOut {
    title: String,   // 步骤简述（≤50 字符，超出截断）
    detail: String,  // 步骤详情（做什么、验证什么）
}

#[tauri::command]
pub async fn ai_plan_generate(
    scope: String,
    question: String,
    state: State<'_, AiState>,
) -> Result<PlanGenResult, String>
```

- **Planner prompt**（system）：要求模型将用户目标拆解为 3~8 个可执行步骤，每步给出 `title`（≤50 字符）与 `detail`；严格输出 JSON 数组，禁止任何额外文本。prompt 声明执行环境为「SSH 远程服务器，可用只读工具 read_file / list_directory / get_system_info 与经用户授权的写操作」，使步骤落到工具可及的范围；
- **请求方式**：非流式单次 POST（复用 `ai_summarize` 的请求链路，messages = system(Planner) + user(question)），不携带工具定义；
- **宽松解析** `parse_plan_json`：模型未严格遵守时，截取响应中首个 `[` 至末个 `]` 的子串再 `serde_json::from_str`；解析后校验——数组长度 ∈ [3, 8]（<3 无需规划，>8 注入膨胀且长程规划质量下降）、每步 `title` 非空（空则剔除该步，剔除后不足 3 步整体 Err）；
- **取消**：令牌登记进 `AiState.cancels`（与流式同一 scope 键），中止/切 scope 时取消在飞规划请求；
- **失败**：Err 透传前端，前端 toast「计划生成失败，已按普通模式发送」并自动走既有 ReAct 路径。

### 6.3 前端数据结构与状态机（`frontend/src/lib/ai.ts` + `AiChatPanel.vue`）

```ts
export interface AiPlanStep {
  id: string
  title: string   // draft 态可编辑
  detail: string  // draft 态可编辑
  state: 'pending' | 'running' | 'done' | 'failed' | 'skipped'
  /** 执行结果摘要（≤150 字符；failed 时存失败原因；skipped 记「用户跳过」） */
  result?: string
}

export interface AiPlan {
  /** 用户原始问题（剥离 /plan 前缀后） */
  goal: string
  steps: AiPlanStep[]
  /** 当前执行下标（executing 态有效） */
  currentIndex: number
  state: 'draft' | 'executing' | 'done' | 'cancelled'
}

// AiChatMessage 新增字段
plan?: AiPlan
```

- 计划挂在生成它的那条 assistant 消息上，随 `persist()` 落盘；重载后恢复展示；
- 崩溃收敛：重载时 `state === 'executing'` 的计划，其 `running` 步骤转为 `failed`（结果记「应用中断」），plan 保持 `executing` 等用户决策（重试/跳过/终止）——与 streaming→stopped 的收敛语义一致；
- draft 态重载后仍可编辑/确认；
- 发送新消息前若存在 `draft` 或 `executing` 计划：提示「有计划进行中，请先处理」并拦截（与 `hasPendingProposal` 拦截同一交互语言），避免多计划并行。

### 6.4 执行驱动

```text
confirmPlan()
  以编辑后文本生成快照落盘 → plan.state=executing, currentIndex=0 → runPlanStep(0)

runPlanStep(i)
  step[i].state=running；toolRound.value=0（每步独立计数）
  → 追加 assistant 流式消息 → runAiStream(seq)
    （buildWireMessages 注入计划 system 消息，见 §6.5）
  → 该步落定判定：assistant status='done' 且本条无未裁决提案、不再续跑
    （挂接点：finishStreaming 无提案路径 + settleProposalRound 续跑链末端）
  → step[i].result = 该步 assistant content 前 150 字符 → state=done
  → i+1 < steps.length ? runPlanStep(i+1) : finishPlan()

finishPlan()
  plan.state=done → 追加总结 assistant 消息并发起一轮请求：
  注入「全部步骤已落定 + 各步结果摘要」，enableTools=false，纯文本收尾

failPlanStep(i, reason)
  step[i].state=failed, result=reason → 卡片呈现操作按钮
  [重试]     → step 重置 pending（result 清空）→ runPlanStep(i)
  [跳过]     → step.state=skipped（result 记「用户跳过」）→ runPlanStep(i+1) 或 finishPlan()
  [终止]     → plan.state=cancelled，剩余步骤保持 pending
  [重新规划] → replan(i)
```

- **失败触发路径**：assistant status='error'、用户中止、工具轮次超上限（该步 toast 后转 failed，原因「工具轮次上限」）；
- **重试**不清除该步已产生的 assistant 消息（历史留痕），新起一条；
- **跳过**与**终止**是显式用户决策，均落盘；
- 中止按钮（既有 streaming 中止）：执行中点击 → `aiStopChat` 现状不变，当前步转 `failed`（原因「用户中止」），plan 保持 `executing` 等用户决策，不回滚已执行操作（既有中止语义）。

### 6.5 计划上下文注入（`buildWireMessages` 扩展）

执行步骤 i 时，在既有 system 序列（规则/技能/记忆/摘要）之后追加一条计划 system 消息：

```text
【执行计划】（第 {i+1}/{n} 步，用户已确认，须严格遵守）
目标：{plan.goal}
步骤进展：
1. [已完成] {title} — {result 摘要}
2. [当前步] {title}：{detail}
3. [待执行] {title}
约束：专注完成当前步骤；禁止擅自新增不在计划内的动作；确需偏离时先向用户说明并获得同意；本步完成后用一句话总结本步结果。
```

- **预算**：该条消息 ≤2KB UTF-8。构造顺序：目标（≤100 字符截断）→ 步骤标题（每步 ≤50 字符）→ 当前步 detail → 已完成步骤 result（每条 ≤150 字符）；超出预算时从最旧的已完成步骤摘要开始丢弃（标注 `[早期步骤摘要省略]`）；
- 计划文本取自落盘的 `plan` 快照（确认时冻结），卡片展示与注入同源；
- 技能/规则/记忆的注入顺序与 16KB 预算不变，计划消息独立预算、互不挤占；
- finishPlan 一轮注入「全部步骤结果摘要」（同 2KB 预算），不带工具定义。

### 6.6 Replan（重规划）

```text
replan(failedIndex)
  输入：goal + 各步状态清单（done 的含 result 摘要）+ 失败步 detail 与失败原因
  → ai_plan_generate（同一 IPC，question 为拼装后的重规划描述）
  → 新步骤数组替换 plan.steps[failedIndex..]
    （已完成步骤保留原样；失败步与后续全部作废）
  → plan.state=draft, currentIndex=failedIndex → 用户再次确认后执行
```

- Replan 同样必须经用户确认，不做自动重规划（避免模型在失败循环里自行放大操作面）；
- 重规划次数无硬上限（用户每次显式触发，天然限速）。

### 6.7 计划卡片视觉

- 文档流卡片（与授权弹窗同一视觉语言）：挂在产生它的 assistant 消息位置，宽度同消息区；
- 结构：卡片头部（徽标 + 目标行 `🎯 {goal}`）→ 步骤列表（序号 + 状态图标 + title + detail；draft 态为可编辑输入框）→ 操作按钮行；
- 状态图标：pending `○` 灰、running 蓝色脉冲圆点（复用「思考中…」脉冲动画）、done `✓` 绿、failed `✗` 红、skipped `⊘` 黄；状态随执行驱动实时更新；
- 徽标：`计划中`（蓝，draft）/ `执行中`（蓝脉冲，executing）/ `已完成`（绿，done）/ `已取消`（灰，cancelled）；
- 编辑交互：draft 态 title/detail 为 `<input>` / `<textarea>`，确认时以输入框当前值生成快照（与「commitRename 读 DOM 值保证一致性」同一教训）；
- 按钮按态呈现：draft → `[确认执行] [取消]`；executing 且有 failed 步 → `[重试] [跳过] [终止] [重新规划]`；executing 正常推进 → 无按钮（中止走消息区既有中止按钮）；done/cancelled → 无按钮纯展示；
- 步骤行可展开：done/failed/skipped 步骤点击展开 result 全文（摘要超过一行的场景）；
- 执行期间的工具提案卡片、推理可视化、授权弹窗、消息操作按钮全部照常；重新生成整组截断到对应 user 消息时，计划一并作废（`plan.state=cancelled`）。

### 6.8 魔数取值

| 魔数 | 取值 | 理由 |
|---|---|---|
| 步骤数 | 3~8 | <3 步普通 ReAct 即可，规划是负担；>8 步注入膨胀且模型长程规划质量显著下降 |
| 步骤标题 | ≤50 字符 | 卡片单行展示；Planner prompt 约束 + 后端截断双闸 |
| 步骤结果摘要 | ≤150 字符 | 8 步 × 150 = 1.2KB，加目标与标题后总量 <2KB 预算 |
| 计划注入预算 | 2KB UTF-8 | 够表达进展与约束，不挤占对话上下文与技能/规则 16KB 预算 |
| 每步工具轮次 | 6（复用 MAX_TOOL_ROUNDS） | 步骤是独立子任务，计数独立；不新增常量 |
| 规划生成超时 | 复用非流式请求链路默认 | `ai_summarize` 同链路已验证，不引入新超时参数 |

## 7. 安全与降级

- **授权语义不变**：计划只决定「做什么」，不放大「能做什么」——写操作仍按权限档与豁免表处理，弹窗照常；模型不遵守约束（擅自新增动作）时，最终防线是既有授权链路（manual/risky 档弹窗拦截，用户拒绝即纠正）；
- **隐私边界**：goal、步骤 detail、结果摘要随对话上下文发往模型服务商，与用户显式发送文本同口径；计划内容不写入应用日志；终端内容仍仅以用户显式选中发送为入口；
- **生成失败降级**：`ai_plan_generate` 失败或 JSON 解析失败 → toast + 自动按普通 ReAct 发送，用户输入不丢失；
- **会话切换**：watch scope 时，执行中计划 → `plan.state=cancelled`、`running` 步 → `failed`；在飞规划请求经 `AiState.cancels` 取消；在飞 exec 经既有 `aiCancelExecs` 取消；
- **会话断开**：步骤执行所需 `aiExecTool` / `writeExec` 走既有降级（提案 rejected + error），该步转 failed 等用户决策，不自动重试；
- **崩溃恢复**：重载时 executing 计划的 running 步收敛为 failed（§6.3），不产生孤儿执行；
- **降级矩阵**：
  - 模型输出非 JSON → 宽松解析（§6.2）→ 仍失败则 Err → 降级普通 ReAct；
  - 步数越界（<3 或 >8）→ Err → 降级普通 ReAct；
  - global scope 携带 `/plan ` 前缀 → 按普通文本发送（不触发规划）；
  - 步骤执行中模型纯文本作答（无工具提案）→ 该步正常落定（done），结果摘要为其回复前 150 字符；
  - 全部步骤跳过/无一步成功 → finishPlan 照常总结，模型据各步结果如实作答。

## 8. 测试与验证

- **后端单测**（`ai.rs` `#[cfg(test)]`）：
  1. `parse_plan_json`：严格 JSON 数组 → 正常解析；含前缀/后缀解释文本 → 宽松截取成功；
  2. 步数校验：2 步 → Err；9 步 → Err；空 title 步剔除后 ≥3 → 正常、<3 → Err；
  3. title 超 50 字符截断；
- **前端验证**：
  1. 入口优先级：`/plan ` 前缀（开关关）→ 规划；开关开（无前缀）→ 规划；两者皆无 → 普通；global scope 前缀不生效；发送后 Toggle 自动复位；
  2. 状态机：confirm → 逐步推进 → done；failed → 重试/跳过/终止/重规划各分支；draft/executing 计划拦截下一条发送；
  3. `buildWireMessages` 计划注入：预算 2KB 截断顺序（先丢最旧摘要）；快照与编辑后文本一致；
  4. 每步 toolRound 归零，超限步转 failed；
  5. 崩溃收敛：构造 executing+running 落盘记录 → 重载后步转 failed；
  6. 会话切换：executing 计划切 scope → cancelled + 在飞 exec 取消；
- **手工验证清单**：
  1. `/plan 部署 nginx，修改配置并重启` → 计划卡片 → 编辑某步文本 → 确认 → 逐步执行，状态图标实时更新，注入与编辑后一致；
  2. Toggle 开关发送 → 规划；再发一条（开关已复位）→ 普通 ReAct；
  3. 某步写操作触发授权弹窗（manual 档）→ 裁决后该步继续，计划推进不受干扰；
  4. 制造失败步 → 失败操作按钮各分支行为正确；重新规划 → 新剩余步骤替换并再次确认；
  5. 执行中中止 → 当前步 failed、计划保持 executing；
  6. 全部完成 → finishPlan 总结回复（无工具提案）；
  7. 应用重载 → 中断计划收敛展示；draft 计划恢复可编辑/确认；
  8. 计划生成失败（断网/非法 baseUrl）→ toast + 自动普通模式发送。

## 9. 未决问题

- **自动 Replan**：失败 N 次后自动重规划可解放双手，但放大模型自主操作面；待 MVP 使用反馈后再议；
- **计划模板**：常见运维场景（日志排查/服务部署）的预置计划模板库，可减少规划调用；MVP 不做；
- **步骤间变量传递**：如前一步产出的文件路径供后一步引用，当前靠结果摘要自然语言传递，结构化变量是后续增强；
- **global scope 纯推理计划**：无工具场景的计划（如写作大纲、方案对比）有独立价值，但不复用本执行链路，另行设计。
