# AI 计划执行提前收敛设计方案

> description: 计划执行中剩余步骤已无必要时（目标达成 resolved / 前提被证伪 blocked），模型调用 conclude_plan 提前收敛并进入总结轮
>
> created: 2026-10-14 18:12:50
>
> updated: 2026-10-14 18:21:04
>
> author:

## 1. 设计目标与硬约束

> 范围：本文覆盖「计划执行阶段的提前收敛」——执行某一步后，若已完成步骤的结果已足以回答用户原始目标，则不再执行剩余步骤，直接进入总结轮产出答案。
>
> 边界：不覆盖规划触发与计划生成（见 [ai-plan-trigger-design.md](./ai-plan-trigger-design.md)）、计划状态机与逐步执行驱动（见 [ai-plan-and-act-design.md](./ai-plan-and-act-design.md) §6.4）、自动重规划、步骤间依赖图与并行执行。
>
> 关联代码：`src-tauri/src/ai.rs`、`src-tauri/src/applog/events.rs`、`frontend/src/lib/ai.ts`、`frontend/src/stores/applog.ts`、`frontend/src/components/AiChatPanel.vue`
>
> 修订关系：本文是 [ai-plan-and-act-design.md](./ai-plan-and-act-design.md) §6.4「执行驱动」的增量设计，不推翻其任何既有决策；对 [ai-plan-trigger-design.md](./ai-plan-trigger-design.md) §6.9 的 `ai.plan.*` 事件清单为追加（新增 `ai.plan.concluded`）。

| 目标 | 量化口径 |
|---|---|
| 可提前收敛 | 第 i 步（i < n）完成后判定「剩余步骤已无必要」时，剩余 n-i 步不再执行；计划状态机仍走 `executing → done` |
| 收敛原因可区分 | 每次收敛必须声明 `outcome`：`resolved`（目标已达成 / 信息已足）或 `blocked`（目标前提被证伪、剩余步骤不可行）；两者在跳过原因文案与埋点中可区分 |
| 判定零额外请求 | 不新增任何 LLM 调用；判定由执行步内的模型通过工具调用自主完成（与 ADR-001 的判定取向一致） |
| 剩余步骤全程可见 | 收敛后剩余 `pending` 步一律置 `skipped`，复用既有 ⊘ 图标与橙色样式，用户可一眼看出哪些步骤未执行 |
| 收敛结果由用户确认 | 收敛计划转 `done` 后活动规划面板保留展示各步最终状态（✓/⊘/✗）与「确认」按钮，用户确认后面板才消失；普通 `done` 自动消失行为不变 |
| 不新增步骤状态 | 复用既有 `pending` / `running` / `done` / `failed` / `skipped` 五态，卡片渲染逻辑零改动 |
| 工具面收敛 | `conclude_plan` 仅在计划执行步下发；常规 ReAct 轮与总结轮不下发；`propose_plan` 从计划执行步剔除 |
| 可观测 | 新增埋点 `ai.plan.concluded`，字段仅含枚举 / 数值 / 布尔，不含消息正文、计划标题与命令原文 |

## 2. 必须遵守的既有项目约束

- **判定权交模型**（[ADR-001](./decisions/ADR-001-model-driven-plan-trigger.md)）：复杂度判定、规划触发均由模型在对话轮内自主完成，不引入独立的分类调用；提前收敛的判定沿用同一取向，不使用额外请求；
- **总结轮是唯一答案出口**（[ai-plan-and-act-design.md](./ai-plan-and-act-design.md) §6.4）：最终答案由 `finishPlan()` 发起的总结轮产出；提前收敛只改变「进入总结轮的时机」，不新增第二条答案产出路径；
- **工具调用独占**（[ai-plan-trigger-design.md](./ai-plan-trigger-design.md) §6.2）：判定类工具必须本轮独占，禁止与写操作工具同轮出现，以 `description` 硬约束 + 前端冲突作废双保险；
- **步骤规模与注入预算**（同上 §6.8）：单计划 3~8 步、注入预算 2KB；本设计不改变这两项取值；
- **事件三处一致**：后端 `src-tauri/src/applog/events.rs` 常量表、`docs/explanation/design/applog-design.md` 事件清单、前端 `frontend/src/stores/applog.ts` 常量表必须同步新增 `ai.plan.concluded`；
- **埋点隐私**：事件字段仅含枚举 / 布尔 / 数值，不写入任何用户文本、计划内容或命令原文，仅本地落盘、无网络上报；
- **前端上报白名单**：`report_app_log` 已放行 `ai.` 前缀（见 applog-design.md §7.2）。

## 3. 现状盘点与差距

| 位置 | 现状 | 差距 |
|---|---|---|
| `ai.rs::select_tools` | 两档裁剪开关：`plan_forced`（仅下发 `propose_plan`）、`disable_plan_proposal`（全量剔除 `propose_plan`） | 缺「计划执行步」档位；执行步与常规轮工具集同构，`propose_plan` 仍在执行步下发（模型可产生嵌套计划卡片） |
| `ai.rs::builtin_tools` | 9 个工具（3 只读 + 3 文件编辑 + 2 SFTP + `propose_command` + `propose_plan`） | 缺提前收敛的信号工具 |
| `ai.rs::ai_chat_stream` | 入参 `plan_forced` / `disable_plan_proposal` | 缺 `plan_step_active` 入参 |
| `AiChatPanel.runAiStream` | `enableTools = scope !== 'global' && !planSummaryRef` | 无「当前处于计划执行步」的判定 |
| `AiChatPanel.completePlanStep` | 无条件推进：`i+1 < n` 则 `runPlanStep(i+1)`，否则 `finishPlan()` | 无收敛分支，无从「提前进入总结轮」 |
| `AiChatPanel.onToolCalls` | 已按 `propose_plan` / 写操作 / 只读三分流 | 无 `conclude_plan` 分流 |
| 步骤状态机与计划卡片 | `skipped` 状态、⊘ 图标、橙色样式均已实现（用户手动「跳过」路径在用，`step.result` 承载跳过原因） | 复用即可，状态与视觉无需改动 |
| `AiChatPanel.activePlanMsg` | 活动计划面板仅展示 `draft` / `executing` / `cancelled`；`done` 自动消失（由变更文件列表面板接替） | 收敛计划转 `done` 后需保留面板展示各步最终状态，等用户确认收尾 |
| 埋点 `ai.plan.*` | 12 个事件 | 缺 `ai.plan.concluded` |

**结论**：改动集中在「新增 1 个工具 + 1 个裁剪档位 + 1 个推进分支 + 1 处面板保留分支」，状态机、卡片渲染、总结轮链路全部复用，属低风险增量。

## 4. 目标与非目标

### 目标

1. 信息获取 / 诊断类目标：任一执行步完成后若结果已足以回答原始目标，剩余步骤不执行，直接进入总结轮；
2. 收敛决策零额外请求，由模型在既有执行步的 ReAct 轮内通过 `conclude_plan` 工具调用给出；
3. 收敛后剩余步骤状态可见（`skipped` + 跳过原因），用户不会误以为「已全部执行」；
4. `conclude_plan` 严格限定在计划执行步，普通对话与总结轮不受污染；
5. 事件可观测：`ai.plan.concluded` 记录收敛发生的步序与跳过步数，用于评估误判。

### 非目标

- **不做「继续执行剩余步骤」逃生口按钮**（本轮决策）：收敛后用户可通过自然语言继续要求执行，不新增专用按钮；
- **不做每步独立判定请求**：与 ADR-001「零额外分类调用」冲突，且每步增加一次请求延迟；
- **不按目标类型做硬阻断**：不在服务端或前端按「信息型 / 动作型」分类拦截收敛——该分类无法客观判定，且动作型目标同样存在合法收敛场景（前提被证伪）；收敛合法性由 `description` 判据 + `outcome` 埋点数据约束，不引入机制阻断；
- 不做步骤重排、局部回滚、自动重规划；
- 不改变步骤数量（3~8）、注入预算（2KB）、单步工具轮次（6）等既有取值。

## 5. 总体架构

```text
第 i 步执行（runPlanStep）
        │
        ▼
  执行步 ReAct 轮（工具集：全量 − propose_plan + conclude_plan）
        │
        ├── 模型继续调工具 ──► 常规提案链路（授权 / 自动执行）──► 循环
        │
        └── 模型调用 conclude_plan（本轮独占）
                 │
                 ▼
        onToolCalls 分流：标记该步 concluded=true，提案置 accepted
                 │
                 ▼
        步落定（maybeAdvancePlan → completePlanStep）
                 │
        step.concluded == true ?
          ├── 否 ──► i+1 < n ? runPlanStep(i+1) : finishPlan()
          └── 是 ──► 剩余 pending 步置 skipped（原因按 outcome 区分）
                    + ai.plan.concluded 埋点（含 outcome）──► finishPlan()（总结轮，复用唯一答案出口）
```

## 6. 详细设计

### 6.1 工具定义：`conclude_plan`（`src-tauri/src/ai.rs::builtin_tools`）

```json
{
  "type": "function",
  "function": {
    "name": "conclude_plan",
    "description": "当【继续执行剩余步骤已无必要】时调用本工具，调用后剩余步骤不再执行，直接进入结果归纳。仅在以下两种情形调用：① outcome=resolved——原始目标（信息获取、诊断、解释类）已被已完成步骤的结果完整回答；② outcome=blocked——动作型目标（修复、安装、部署、重启等）的前提已被证伪或不成立（例如要求修复 nginx 但发现机器上未安装 nginx），后续步骤已不可行，此时应收敛并给出阻塞点与建议，而不是继续执行无效步骤。禁止在以下情况调用：① 目标仍然可达且剩余步骤仍然有效，仅因当前步骤困难、报错或耗时而想收尾；② 剩余步骤是用户显式要求且仍可执行的动作。调用后请直接输出结论正文，不要在同一轮调用其他工具。",
    "parameters": {
      "type": "object",
      "properties": {
        "outcome": {
          "type": "string",
          "enum": ["resolved", "blocked"],
          "description": "resolved：目标已达成或信息已足以回答；blocked：目标前提不成立、剩余步骤不可行"
        },
        "reason": {
          "type": "string",
          "description": "resolved 时说明已获得哪些关键信息；blocked 时说明阻塞原因与建议的下一步"
        }
      },
      "required": ["outcome", "reason"]
    }
  }
}
```

设计要点：

- **只读语义、无副作用**：工具本身不产生远端操作，前端分流时自动接受（不进授权弹窗、不进会话豁免表）；
- **`outcome` + `reason` 双必填**：`outcome` 先强制二分「已达成」与「已阻塞」，避免把「做不下去」当成「做完了」；`reason` 强制显式论证（`resolved` 说明关键信息、`blocked` 说明阻塞点与建议），是最低成本的滥用抑制；
- **独占约束写进 `description`**：与 `propose_plan` 同一策略——提示层约束 + 前端冲突作废双保险；
- **禁止条款只针对「偷懒式收敛」**：不按目标类型一刀切禁止（动作型目标在前提被证伪时同样应收敛），只禁止「目标仍可达、剩余步骤仍有效却因困难或耗时收尾」，取值与理由见 §6.8。

### 6.2 工具集裁剪：第三档位 `plan_step_active`（`ai.rs::select_tools`）

```text
select_tools(plan_forced, plan_step_active, disable_plan_proposal)
  ├─ plan_forced            → [propose_plan]                       （强制 / 重规划轮，仅 1 个）
  ├─ plan_step_active       → 全量 − propose_plan（保留 conclude_plan）（计划执行步，9 个）
  ├─ disable_plan_proposal  → 全量 − propose_plan − conclude_plan   （拒绝计划后的回合，8 个）
  └─ 其余（常规 ReAct 轮）    → 全量 − conclude_plan                  （现状，9 个）
```

> 两开关同真（`plan_step_active` ∧ `disable_plan_proposal`）时：`propose_plan` 剔除、`conclude_plan` 保留——执行步语义优先。

- `plan_forced` 优先级最高（强制轮只有一个工具，天然不可能出现收敛信号）；
- 执行步剔除 `propose_plan`：修复 §3 指出的现状缺口——执行过程中模型不应再提新计划；
- 常规轮不下发 `conclude_plan`：普通对话没有「剩余步骤」概念，下发会诱导无效调用；
- `ai_chat_stream` 新增入参 `plan_step_active: Option<bool>`，同时写入 `ai-tool-calls` / 流事件 payload，便于前端与排查对齐。

### 6.3 前端下发条件（`AiChatPanel.vue::runAiStream`）

```text
plan_step_active = planRunRef.value !== null && !planSummaryRef.value
```

- `planRunRef` 非空 = 当前正处某一步的执行链（既有状态，见 `runPlanStep`）；
- 排除总结轮（`planSummaryRef` 非空）：总结轮不下发任何工具，`conclude_plan` 也不应例外；
- 该标记与既有 `enableTools`（`scope !== 'global'`）取与关系，global 会话天然不带工具。

### 6.4 前端分流：`onToolCalls` 新增 `conclude_plan` 分支

```text
for (p of toolProposals):
  ├─ name === 'propose_plan'  → 现状：建卡 / 冲突作废 / 禁令兜底
  ├─ name === 'conclude_plan'
  │     ├─ 与写操作工具同轮（WRITE_TOOLS 任一）→ 作废 conclude（state=rejected，保留执行动作）
  │     ├─ 同轮已出现 propose_plan            → 作废 conclude（计划优先级更高）
  │     └─ 否则 → 给当前步打标记 concluded=true 并记录 outcome（resolved / blocked）；p.state='accepted'
  ├─ WRITE_TOOLS              → 现状：豁免表 / 权限档 / 弹窗入队
  └─ 其余（只读）              → 现状：自动执行
```

- 标记落在步骤上（`planMsg.plan.steps[stepIndex].concluded = true`，同时记 `concludeOutcome = outcome`），由 §6.5 消费；
- `outcome` 缺失或非 `resolved` / `blocked` 时按 `resolved` 处理，保证参数异常不阻断收敛链路（不额外写 `p.error`）；
- `toolResultOf` 对 `conclude_plan` 走既有只读工具通用分支（`p.state === 'accepted'` 且无输出）返回成功信号 `{"success":true,"note":"工具执行成功，无标准输出"}`，无需特判；
- 作废原因写入 `p.error`，卡片按既有 rejected 样式展示（可读留痕）。

### 6.5 推进分支：`completePlanStep` 提前收敛（`AiChatPanel.vue`）

```ts
function completePlanStep(planMsg, i) {
  const plan = planMsg.plan
  const step = plan.steps[i]
  step.state = 'done'
  step.result = <该步最终回复前 150 字符>
  persist()

  // 提前收敛：本步标记 concluded → 剩余待执行步骤转 skipped 后直接进入总结轮
  if (step.concluded) {
    for (const s of plan.steps.slice(i + 1)) {
      if (s.state !== 'pending') continue
      s.state = 'skipped'
      s.result = step.concludeOutcome === 'blocked' ? '后续步骤不可行，已提前结束' : '已获得答案，无需继续'
    }
    plan.converged = true                 // 面板保留标记（见 §6.5 面板行为）
    report('info', AI_PLAN_CONCLUDED, '计划提前收敛', {
      step_index: i + 1,
      step_count: plan.steps.length,
      skipped_count: plan.steps.filter(s => s.state === 'skipped').length,
      outcome: step.concludeOutcome ?? 'resolved',
      duration_ms: Date.now() - planMsg.createdAt,
    })
    persist()
    finishPlan(planMsg)
    return
  }

  if (i + 1 < plan.steps.length) runPlanStep(planMsg, i + 1)
  else finishPlan(planMsg)
}
```

- `finishPlan()` 复用既有实现：`plan.state = 'done'`、`planSummaryRef` 置位、发起不带工具的总结轮——**答案出口唯一性不变**；
- 跳过原因写入 `step.result`（按 `outcome` 取「已获得答案，无需继续」/「后续步骤不可行，已提前结束」），与用户手动「跳过」写 `'用户跳过'` 同构，注入逻辑零改动；
- 只对 `pending` 步置 `skipped`：已 `failed` 的步骤保留失败态（用户可能需要看到它，也可走重试）；
- 收敛后计划卡片呈现：已完成步 ✓、跳过步 ⊘（橙色），用户在卡片上一眼可辨。

**面板行为（收敛 vs 普通完成）**：

- 普通计划转 `done` 后活动计划面板自动消失（既有行为不变），由变更文件列表面板接替；
- **收敛计划转 `done` 后活动计划面板保留**：新增 `plan.converged` 标记，`activePlanMsg` 判定扩展为
  `done && converged && !acknowledged`；面板继续展示各步最终状态（✓ 已完成 / ⊘ 已跳过 / ✗ 失败），
  徽标显示「已收敛」、进度显示 `done/N · 已完成（K 步跳过）`，并提供「确认」按钮——用户点击后
  `acknowledgePlan` 置 `acknowledged` 面板才消失（与 `cancelled` 收尾同一路径）；
- 步骤状态与结果**只在规划面板呈现**，消息对话窗口的渲染方案不变（执行过程折叠区 + 结果区）；
- 普通 `done`（无 `converged`）不进 `activePlanMsg`，不影响既有自动消失行为。

### 6.6 上下文注入（仅执行步约束尾巴追加一句）

[ai-plan-and-act-design.md](./ai-plan-and-act-design.md) §6.5 的注入按 `done` / `skipped` / `failed` / 未执行分别打标签，`skipped` 步注入为 `[跳过] {title} — {result}`。收敛置 `skipped` 后，总结轮上下文自动包含「第 k..n 步未执行及其原因（已获得答案 / 后续步骤不可行）」，总结正文不会假装全部完成，**无需为收敛单独改注入代码**；`blocked` 收敛时，阻塞原因与建议由 `conclude_plan` 的 `reason` 与步 `result` 承载，总结轮据此输出「阻塞点 + 建议下一步」而非假装任务已完结。

唯一的注入侧改动：执行步 system 的约束尾巴追加一句收敛指引——「若原始目标已被当前结果完整回答，或后续步骤的前提已不成立（如目标所需组件不存在），调用 `conclude_plan` 提前收敛，剩余步骤将标记跳过并直接进入结果归纳」。现有约束中的「只执行当前步骤、未到后续步骤不要提前执行」容易被模型理解为「不得提前收尾」，该句用于消歧；不新增独立注入段，不占额外预算（尾部单句，仍在 2KB 预算内）。

### 6.7 埋点：`ai.plan.concluded`

| event_id | 触发时机 | 字段 |
|---|---|---|
| `ai.plan.concluded` | 执行步标记收敛、剩余步骤转 `skipped` 后 | `outcome`（`resolved` / `blocked`）、`step_index`、`step_count`、`skipped_count`、`duration_ms`（建卡 → 收敛） |

- **`outcome` 是模型显式声明的枚举，可客观聚合**：`blocked` 属合法降级（剩余步骤确实不可行）；`resolved` 且 `step_index` 偏小、`skipped_count` 偏大时，「偷懒式过早收敛」嫌疑最高——两类分开统计才能准确评估收敛质量；
- **不含 `goal_type`**：信息型 / 动作型由模型自述、无客观判定依据，写入埋点只会产生误导性数据，故不新增该字段；
- 字段仅含枚举 / 数值，满足 applog 隐私约束（§2）；
- 同步三处：`events.rs` 常量、`applog-design.md` 事件清单、`stores/applog.ts`。

### 6.8 魔数取值

| 魔数 | 取值 | 理由 |
|---|---|---|
| 收敛工具集档位 | 执行步 = 全量 − `propose_plan` + `conclude_plan` | 执行步既不该提新计划，也必须能提前收尾；两处裁剪共用同一 `select_tools` 实现 |
| `conclude_plan` 参数 | `outcome`（枚举 `resolved` / `blocked`）+ `reason`（必填，无长度上限） | `outcome` 强制二分「已达成」与「已阻塞」，是评估偷懒式收敛的关键维度；`reason` 强制显式论证（blocked 时承载阻塞点与建议）；两者均不落卡片，`reason` 不进埋点 |
| `outcome` 取值 | `resolved` / `blocked`（缺失或非法按 `resolved` 处理） | 覆盖「目标达成」与「前提被证伪」两种合法收敛；缺省取值保证参数异常时不阻断收敛链路 |
| 跳过原因文案 | `已获得答案，无需继续`（resolved）/ `后续步骤不可行，已提前结束`（blocked） | 与用户手动跳过的原因（`用户跳过`）用词可区分；两类收敛在卡片与注入中也彼此可辨 |
| 剩余步骤处理范围 | 仅 `pending` 步 | 已 `failed` 步保留失败态供用户决策，避免掩盖失败信息 |
| 收敛后状态 | 计划 `executing → done` | 复用既有终态，不新增 `concluded` 计划态；仅新增 `plan.converged` 布尔标记用于面板保留判定（持久化字段，随会话落盘） |

## 7. 安全与降级

| 场景 | 处理 |
|---|---|
| 模型过早收敛（目标实际未被回答） | 剩余步骤以 ⊘ `skipped` + `已获得答案，无需继续` 明示未执行；用户可在随后消息中继续要求执行，普通对话轮仍能调工具完成任务（非目标项：不提供专用逃生口按钮） |
| 模型把「做不下去」当成「做完了」（`resolved` 误标） | 跳过原因文案显示为「已获得答案，无需继续」，与 blocked 文案可辨；靠 `outcome` 分布 + 人工抽测识别；用户可在随后消息中指出未完成并要求继续（§9 未决 1） |
| 模型「偷懒式」收敛（目标仍可达却收尾） | 唯一防线是 `description` 的禁止条款；`ai.plan.concluded` 中 `outcome=resolved` 且 `step_index` 偏小的占比用于评估是否需升级为机制约束（§9 未决 2） |
| 该收敛（`blocked`）却继续跑无效步骤 | 无机制强制；属性能损失而非正确性问题（无效命令仍走授权链路由用户裁决），靠 §6.1 判据调优改善 |
| 模型与写操作工具同轮调用 `conclude_plan` | 前端作废收敛、保留写操作，与 `propose_plan` 冲突策略一致（判定类提案让位于实际动作） |
| 供应商不支持工具调用 / 模型不调用 `conclude_plan` | 静默退化为现状：全部步骤执行完毕后进入总结轮，无功能损失、无报错 |
| 权限档影响 | `conclude_plan` 只读无副作用：full / manual / risky 三档一律自动接受，不进授权弹窗、不进会话豁免表 |
| 会话在收敛步中途断开 | 既有降级不变：该步转 `failed` 等用户决策，不会因收敛逻辑改变断线语义 |

## 8. 测试与验证

- **单元测试**（`ai.rs`）：
  1. `select_tools` 三档位断言：`plan_step_active=true` 时结果含 `conclude_plan`、不含 `propose_plan`，且总数为 9（原 9 减 1 加 1）；
  2. `plan_forced` 与 `plan_step_active` 同时为真时，仍只下发 `[propose_plan]`（强制优先）；
  3. `conclude_plan` schema 断言：`required` 为 `["outcome", "reason"]`，`outcome` 为 `string` 且 `enum = ["resolved", "blocked"]`。
- **手工验证清单**：
  1. 信息型复杂任务（如「排查磁盘为什么被占满但先别清理」）→ 若第 2 步已得出结论，剩余步骤应显示 ⊘ 跳过，随后直接出现总结答案；规划面板保留展示各步最终状态与「确认」按钮，点击后面板消失；
  2. 动作型任务、前提成立（如「修复 nginx 配置错误并重启服务」，机器已装 nginx）→ **不得收敛**，应跑完全部步骤；
  3. 动作型任务、前提被证伪（如「修复 nginx」但机器上未安装 nginx）→ 应以 `outcome=blocked` 收敛，剩余步骤显示 ⊘，总结输出「阻塞点 + 建议（是否安装 / 换方案）」；
  4. 常规对话轮：普通提问不得出现 `conclude_plan` 工具（可通过后端日志确认下发工具集）；
  5. 计划执行步中模型不得再产出 `propose_plan` 卡片（§6.2 剔除生效）；
  6. 收敛路径埋点 `ai.plan.concluded` 字段与 §6.7 一致，且日志中不含 `reason` 原文、步骤标题与命令原文。
- **回归**：`cargo test --lib`、`vue-tsc -b`、`vite build` 全绿；既有 12 个 `ai.plan.*` 事件行为不变。

## 9. 未决问题

1. **「继续执行剩余步骤」逃生口**：本轮明确不做（§4 非目标）。若上线后 `ai.plan.concluded` 显示过早收敛频繁，再评估增加按钮（把 `skipped` 恢复 `pending` 后 `runPlanStep`），复用既有 `retryPlanStep` 逻辑即可；
2. **`blocked` 判定的可靠性**：模型可能在本可继续（例如换用替代方案）时误判为 `blocked` 并收敛。当前仅靠 `description` 判据 + `outcome` 埋点观测；若数据表明误判频繁，可在收敛后的总结轮显式增加「是否继续 / 换方案」引导，或恢复重新规划入口（收敛后计划已转 `done`、失败四按钮不再出现，用户当前需手写 `/plan ` 手动重规划）；
3. **过早收敛的客观观测手段不足**：无法自动关联「用户随后追问同一目标」这一最强信号，当前只能依赖 `step_index` 分布与人工抽测；
4. **跳过原因是否在卡片上以 tooltip 展示**：当前 `step.result` 仅用于注入与重规划拼装，卡片只显示 ⊘ 与标题；若用户反馈「不知为何跳过」，再补 `title` 属性提示（改动极小）。
