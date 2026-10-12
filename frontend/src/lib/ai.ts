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
  /** streaming：流式生成中；done：完成；error：出错；stopped：用户中止 */
  status: 'streaming' | 'done' | 'error' | 'stopped'
  createdAt: number
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
