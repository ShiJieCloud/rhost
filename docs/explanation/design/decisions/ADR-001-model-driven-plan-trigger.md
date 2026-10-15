# ADR-001: AI 规划触发权由用户显式开关改为模型自主提案

> status: 已采纳
>
> created: 2026-10-14 16:40:02
>
> author:

## 背景

rhost AI 聊天的 Plan-and-Act 能力（见 [ai-plan-and-act-design.md](../ai-plan-and-act-design.md)）首版采用「用户显式触发」：输入框 Toggle 开关与 `/plan ` 斜杠命令二合一，计划由独立 IPC `ai_plan_generate` 生成。用户必须先声明「我要规划」，模型不参与触发判定。

运行后暴露两类问题：

1. **触发门槛过高**：多文件、多服务、需「先诊断后修改」的复杂任务，用户通常不会主动打开开关，Plan-and-Act 长期处于「有着不用」状态；
2. **独立调用成本**：独立规划 IPC 需多发一次请求（含思考模式时单次实测 33~60s），且需单独维护规划上下文（`PLANNER_PROMPT`），与主对话流的复杂度判定割裂。

## 考虑的方案

1. 方案 A：维持用户显式触发（现状）
   - 做法：保留 Toggle + 斜杠命令双入口与独立 `ai_plan_generate` IPC。
   - 优点：触发确定、用户完全可控；实现已稳定。
   - 代价：复杂任务默认不规划；每次规划多一次数十秒的独立调用；两套生成路径（独立 IPC 与工具调用）各需适配。
2. 方案 B：模型自主提案 + 强制路径（采纳）
   - 做法：撤销独立开关，规划生成收敛为后端内置工具 `propose_plan`，在当轮对话流内完成「复杂度判定 + 输出计划」；默认由模型自主判定并提案，用户确认后执行；保留可见强制入口（输入区「+」菜单「规划模式」项，等价 `/plan ` 前缀）与「重新规划」；强制 / 重规划轮只下发该工具并携带 `tool_choice` 保证必然产出计划；下线 `ai_plan_generate` / `PLANNER_PROMPT` / `chat_stream_collect` 的规划用法。
   - 优点：复杂任务默认获得规划、零额外调用、零额外操作；生成、校验、卡片、失败处理收敛为单一路径；强制路径由服务端约束保证确定性。
   - 代价：复杂度判定交给模型，存在误判 / 漏判，需埋点观测并迭代判据；`tool_choice` 依赖供应商支持 forced tool use，不支持时需降级提示。

## 决策

采纳方案 B：规划触发权由「用户显式开关」改为「模型自主提案」，规划生成收敛为 `propose_plan` 单一路径，并保留用户可见的强制规划入口作为逃生口。设计细节见 [ai-plan-trigger-design.md](../ai-plan-trigger-design.md)。

## 影响

- 优点：复杂任务默认进入 Plan-and-Act；消除独立规划调用的数十秒额外延迟；生成路径、`applyPlanProposal` 校验、计划卡片、失败处理全部收敛为一份实现；模型漏判时用户仍可经「+」菜单「规划模式」强制触发，确定性由 `tool_choice` 保证。
- 缺点 / 代价：引入误判（简单任务被提案）与漏判（复杂任务未提案）两类风险，需以 `ai.plan.*` 本地 applog 埋点观测并迭代判据文案（设计文档 §6.9）；强制路径依赖供应商 forced tool use，不支持时降级为「未生成计划」提示并埋点 `ai.plan.force_failed`。
- 后果：
  - 推翻 [ai-plan-and-act-design.md](../ai-plan-and-act-design.md) 的既有非目标「不做任务复杂度自动检测与自动进入规划模式」与 §6.1「Toggle 开关 + 斜杠命令二合一」入口设计；该文档相关章节需同步指向本 ADR 与设计文档。
  - 代码侧下线 `ai_plan_generate` IPC、`PLANNER_PROMPT`、`chat_stream_collect`（规划用法），以及前端 `sendPlanned()` / `cancelPlanning()` / `planning` 状态与「取消规划」停止键；新增 `propose_plan` 工具、`ChatRequest.tool_choice`、`ai_chat_stream` 的 `planForced` 与 `disable_plan_proposal` 布尔入参。
  - 兼容性：AI 侧对外协议不变（仍为 `ai_chat_stream` 流式 + `ai-tool-calls` 扁平事件）；已保存的 `draft` / `executing` 计划结构不变，可继续确认与执行。
