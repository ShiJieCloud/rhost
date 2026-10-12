/**
 * AI 助手 IPC 封装（M1 / M2.3 协议层）。
 *
 * 后端协议：
 * - invoke('ai_chat_stream', ...) 发起流式对话，token 经事件增量推送；
 *   `enableTools: true` 时附加内置 propose_command 工具定义（后端内置，无需传定义）；
 * - 事件：`ai-token` / `ai-done` / `ai-error` / `ai-tool-calls`，均带 scope，前端需自行过滤；
 *   `ai-tool-calls` 为模型工具提案（后端不执行，裁决与工具循环由前端驱动）；
 * - 会话记录按 scope（'global' | SSH 会话 id）落盘到后端 ai/chats/ 目录。
 */
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

/** 对话归属：首页全局对话 / 终端会话对话（会话 id） */
export type AiScope = string

/** 工具调用记录（assistant 历史消息 / 提案事件共用形态） */
export interface AiToolCall {
  id: string
  type: 'function'
  function: { name: string; arguments: string }
}

/** 发给模型的消息（与 OpenAI 消息同构，后端透传）。
 *  tool 角色用于回传工具执行结果（content 为结果文本，tool_call_id 关联提案）；
 *  assistant 携带 tool_calls 时 content 为 null */
export interface AiWireMessage {
  role: 'user' | 'assistant' | 'system' | 'tool'
  content: string | null
  tool_calls?: AiToolCall[]
  tool_call_id?: string
}

/** 提案卡片持久化形态（随会话落盘，后端原样存取；与 wire 的 AiToolCall 区分） */
export interface AiToolProposal {
  id: string
  /** 工具名，M2 恒为 propose_command */
  name: string
  /** 原始 JSON 字符串（arguments 聚合结果，wire 回传时原样使用） */
  args: string
  /** 模型给出的一句话用途，供提案卡片展示 */
  reason?: string
  /** accepted 但写入失败时记录原因，wire 结果回传 error 让模型自行调整 */
  error?: string
  /** accepted 且经「本会话允许」豁免（会话级 allowlist 精确命中） */
  allowedSession?: boolean
  /** 执行输出回传（M3.1）：aiAutoReturnOutput=true 时捕获的命令输出，供 tool 结果组装 */
  execOutput?: string
  /** pending 等待裁决 / accepted 已处理 / rejected 已拒绝 / expired 用户忽略后失效 */
  state: 'pending' | 'accepted' | 'rejected' | 'expired'
}

/** 前端持久化的消息（比 wire 多 id/状态/思考过程） */
export interface AiChatMessage {
  id: string
  role: 'user' | 'assistant'
  content: string
  /** 模型思考过程（reasoning_content），独立存储以支持折叠可视化 */
  reasoning?: string
  /** 工具提案（M2.3）：assistant 消息携带的 propose_command 提案卡片状态 */
  toolProposals?: AiToolProposal[]
  /** 执行计划（M4 Plan-and-Act）：规划模式生成的计划卡片状态 */
  plan?: AiPlan
  /** streaming：流式生成中；done：完成；error：出错；stopped：用户中止 */
  status: 'streaming' | 'done' | 'error' | 'stopped'
  createdAt: number
}

/* ---- Plan-and-Act 执行计划（M4） ---- */

/** 计划步骤（卡片持久化形态，随会话落盘） */
export interface AiPlanStep {
  id: string
  /** 步骤简述（≤50 字符；draft 态可编辑） */
  title: string
  /** 步骤详情（draft 态可编辑） */
  detail: string
  /** pending 待执行 / running 执行中 / done 已完成 / failed 失败 / skipped 用户跳过 */
  state: 'pending' | 'running' | 'done' | 'failed' | 'skipped'
  /** 执行结果摘要（≤150 字符；failed 存失败原因；skipped 记「用户跳过」） */
  result?: string
}

/** 执行计划（挂在生成它的 assistant 消息上，随会话落盘） */
export interface AiPlan {
  /** 用户原始问题（剥离 /plan 前缀后） */
  goal: string
  steps: AiPlanStep[]
  /** 当前执行下标（executing 态有效） */
  currentIndex: number
  /** draft 待确认 / executing 执行中 / done 全部落定 / cancelled 已取消 */
  state: 'draft' | 'executing' | 'done' | 'cancelled'
}

/** 后端计划生成结果（serde camelCase） */
export interface AiPlanGenResult {
  steps: { title: string; detail: string }[]
}

/** 生成执行计划：非流式单次调用，模型输出经后端宽松解析为 3~8 步数组 */
export function aiPlanGenerate(scope: AiScope, question: string): Promise<AiPlanGenResult> {
  return invoke('ai_plan_generate', { scope, question })
}

/** 落盘的会话记录（后端原样存取） */
export interface AiChatRecord {
  version: number
  scope: AiScope
  title?: string
  updatedAt: number
  messages: AiChatMessage[]
  /** 滚动摘要：覆盖到 summaryUpTo 为止的全部历史（M1.5） */
  summary?: string
  /** 摘要覆盖的最后一条消息 id；此后消息仍原文进上下文 */
  summaryUpTo?: string
  /** 记忆提取覆盖的最后一条消息 id（M3.3）；此后消息待提取 */
  extractedUpTo?: string
}

/* ---- 事件类型 ---- */

export type AiTokenKind = 'content' | 'reasoning'
export interface AiTokenEvent {
  scope: AiScope
  kind: AiTokenKind
  delta: string
}
export interface AiDoneEvent {
  scope: AiScope
  reason: 'completed' | 'stopped'
}
export interface AiErrorEvent {
  scope: AiScope
  message: string
}
/** 工具提案事件项：后端 ToolCallOut 扁平形态（自定义事件载荷，非 OpenAI 嵌套结构） */
export interface AiToolCallOut {
  id: string
  name: string
  arguments: string
}
/** 工具提案事件：模型请求调用工具（arguments 为 JSON 字符串，解析在前端） */
export interface AiToolCallsEvent {
  scope: AiScope
  calls: AiToolCallOut[]
}

/* ---- IPC ---- */

/** 发起流式对话；结果通过事件返回，invoke 本身的 rejection 仅表示建连前失败。
 *  enableTools 附加内置工具定义（propose_command），供 M2.3 工具循环使用 */
export function aiChatStream(
  scope: AiScope,
  messages: AiWireMessage[],
  opts?: { enableTools?: boolean },
): Promise<void> {
  return invoke('ai_chat_stream', { scope, messages, enableTools: opts?.enableTools ?? false })
}

/** 中止指定 scope 的流式回复 */
export function aiStopChat(scope: AiScope): Promise<void> {
  return invoke('ai_stop_chat', { scope })
}

/** 只读工具执行结果（serde camelCase；exitCode 为 Option<i32> → null | number） */
export interface AiToolExecResult {
  /** stdout + stderr 合并文本，已截断至 8KB（超时时为部分片段） */
  output: string
  /** true 表示原始输出超出 8KB 已截断 */
  truncated: boolean
  /** 远端退出码：0=成功；n>0=命令业务失败；null=通道未给出 ExitStatus（含超时/通道级故障） */
  exitCode: number | null
  /** true 表示执行超时，output 含已收到的部分输出（可能非空） */
  timedOut: boolean
}

/** 经 SSH exec channel 执行只读工具（后端构造 shell 命令，前端只传 tool_name + args）。
 *  工具：read_file / list_directory / get_system_info，自动执行无需授权。 */
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

/** 测试连接结果（serde camelCase） */
export interface AiTestResult {
  latencyMs: number
  status: number
  modelCount: number
  models: string[]
}

/** 请求模型列表验证地址与 Key；apiKey 传空串时后端回退钥匙串/已保存值 */
export function aiTestConnection(baseUrl: string, apiKey: string): Promise<AiTestResult> {
  return invoke<AiTestResult>('ai_test_connection', {
    baseUrl,
    apiKey: apiKey || null,
  })
}

/** 保存 API Key 到系统钥匙串 */
export function aiSetApiKey(key: string): Promise<void> {
  return invoke('ai_set_api_key', { key })
}

/** 钥匙串中是否已保存 API Key */
export function aiHasApiKey(): Promise<boolean> {
  return invoke<boolean>('ai_has_api_key')
}

/** 覆盖保存一条会话记录 */
export function aiSaveChat(scope: AiScope, record: AiChatRecord): Promise<void> {
  return invoke('ai_save_chat', { scope, record })
}

/** 读取一条会话记录，不存在返回 null */
export async function aiLoadChat(scope: AiScope): Promise<AiChatRecord | null> {
  const v = await invoke<AiChatRecord | null>('ai_load_chat', { scope })
  return v ?? null
}

/** 删除一条会话记录 */
export function aiClearChat(scope: AiScope): Promise<void> {
  return invoke('ai_clear_chat', { scope })
}

/** 滚动摘要：把旧摘要与待压缩消息合并为一份新摘要（非流式，与对话事件流互不干扰） */
export function aiSummarize(messages: AiWireMessage[]): Promise<string> {
  return invoke<string>('ai_summarize', { messages })
}

/* ---- 技能/规则提示资产（M3.2：ai/skills/ 与 ai/rules/ 目录） ---- */

/** 一条技能/规则文件（serde camelCase；fileName 为稳定 key） */
export interface AiPromptAsset {
  fileName: string
  name: string
  description?: string
  content: string
  /** 单文件超 8KB 被截断 */
  truncated: boolean
  sizeBytes: number
}

export interface AiPromptAssets {
  skills: AiPromptAsset[]
  rules: AiPromptAsset[]
}

/** 列出技能/规则提示资产（目录由后端固定解析；目录/单文件失败降级为空，不报错） */
export function aiListPromptAssets(): Promise<AiPromptAssets> {
  return invoke<AiPromptAssets>('ai_list_prompt_assets')
}

/** 系统文件管理器打开技能/规则目录，返回目录绝对路径 */
export function aiRevealPromptDir(kind: 'skills' | 'rules'): Promise<string> {
  return invoke<string>('ai_reveal_prompt_dir', { kind })
}

/* ---- 跨会话记忆（M3.3：ai/memory/memory.json） ---- */

/** 单条记忆内容上限（字符）：与后端 MEMORY_CONTENT_MAX_CHARS 口径一致 */
export const AI_MEMORY_CONTENT_MAX_CHARS = 200

/** 一条跨会话记忆（serde camelCase；全局存储，sourceScope 仅展示来源） */
export interface AiMemoryItem {
  id: string
  content: string
  createdAt: number
  sourceScope: string
}

/** 读取全部记忆条目（文件不存在/解析失败时后端降级为空数组） */
export function aiListMemories(): Promise<AiMemoryItem[]> {
  return invoke<AiMemoryItem[]>('ai_list_memories')
}

/** 覆盖写入记忆条目（后端兜底：单条截断 200 字符、按设置上限淘汰最旧） */
export function aiSaveMemories(items: AiMemoryItem[]): Promise<void> {
  return invoke('ai_save_memories', { items })
}

/** 清空记忆（文件不存在视为成功） */
export function aiClearMemories(): Promise<void> {
  return invoke('ai_clear_memories')
}

/** 记忆提取：非流式调用，返回模型输出的 JSON 字符串（解析/校验/合并在前端） */
export function aiExtractMemories(messages: AiWireMessage[]): Promise<string> {
  return invoke<string>('ai_extract_memories', { messages })
}

/* ---- 事件订阅（一次性注册四个事件，组件卸载时 unlisten） ---- */

export interface AiEventHandlers {
  onToken: (e: AiTokenEvent) => void
  onDone: (e: AiDoneEvent) => void
  onError: (e: AiErrorEvent) => void
  /** M2.3 工具提案；未提供时事件被忽略（提案不落地） */
  onToolCalls?: (e: AiToolCallsEvent) => void
}

export async function listenAiEvents(h: AiEventHandlers): Promise<UnlistenFn> {
  const [u1, u2, u3, u4] = await Promise.all([
    listen<AiTokenEvent>('ai-token', e => h.onToken(e.payload)),
    listen<AiDoneEvent>('ai-done', e => h.onDone(e.payload)),
    listen<AiErrorEvent>('ai-error', e => h.onError(e.payload)),
    listen<AiToolCallsEvent>('ai-tool-calls', e => h.onToolCalls?.(e.payload)),
  ])
  return () => {
    u1()
    u2()
    u3()
    u4()
  }
}
