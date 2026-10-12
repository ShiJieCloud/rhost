<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { activeSession, activeHost, activeSessionId, appView, rightPanelTab, rightPanelVisible, sendInput, sendInputAndCapture, sessions } from '../stores/session'
import { savedSettings } from '../stores/settings'
import { aiQuote } from '../stores/aiQuote'
import { toast } from '../composables/useToast'
import { matchHighRisk } from '../lib/aiRisk'
import { writeText } from '@tauri-apps/plugin-clipboard-manager'
import { isTauri } from '../lib/tauri'
import AppLogo from './AppLogo.vue'
import AiMarkdown from './AiMarkdown.vue'
import {
  AI_MEMORY_CONTENT_MAX_CHARS,
  aiChatStream,
  aiClearChat,
  aiExtractMemories,
  aiListMemories,
  aiListPromptAssets,
  aiLoadChat,
  aiSaveChat,
  aiCancelExecs,
  aiExecTool,
  aiSaveMemories,
  aiStopChat,
  aiSummarize,
  listenAiEvents,
  type AiChatMessage,
  type AiMemoryItem,
  type AiPromptAsset,
  type AiPromptAssets,
  type AiScope,
  type AiToolExecResult,
  type AiToolProposal,
  type AiWireMessage,
} from '../lib/ai'
import type { UnlistenFn } from '@tauri-apps/api/event'

/** 触发压缩的未摘要字符数（中英混合粗估 1 token ≈ 2 chars，约 4k token） */
const SUMMARY_TRIGGER_CHARS = 8000
/** 摘要后保留的最近消息条数（不参与压缩，保证当前话题连续性） */
const SUMMARY_KEEP_RECENT = 4
/** 触发记忆提取的未提取字符数（约 2k tokens，避免短对话每轮都提取） */
const EXTRACT_TRIGGER_CHARS = 4000

/** 对话归属：首页 'global'，终端为当前 SSH 会话 id */
const scope = computed<AiScope>(() =>
  appView.value === 'home' ? 'global' : activeSession.value?.id ?? 'global',
)

const messages = ref<AiChatMessage[]>([])
const draft = ref('')
const streaming = ref(false)
/** 终端选中文本引用块（右键「添加到 AI 对话」产生），随下次发送并入 user 消息 */
const quote = ref<{ text: string; lines: number } | null>(null)
/** 滚动摘要：覆盖 summaryUpTo 及之前的全部历史，组装上下文时以 system 注入 */
const summary = ref('')
const summaryUpTo = ref('')
/** 记忆提取：覆盖 extractedUpTo 及之前的全部历史（已提取部分不再重复提取） */
const extractedUpTo = ref('')
/** 待编辑重发的 user 消息 id：填回输入框后不立即截断，真正发送时才截断该条及其后 */
const editFromId = ref<string | null>(null)
/** 摘要请求进行中（防重入） */
let summarizing = false
/** 记忆提取请求进行中（防重入） */
let extracting = false
/** reasoning 手动展开/收起状态：msg id → true(展开) / false(收起)；未在 map 中则跟随自动行为 */
const reasoningManual = ref<Map<string, boolean>>(new Map())
const inputRef = ref<HTMLInputElement | null>(null)
const scrollEl = ref<HTMLElement | null>(null)
let unlisten: UnlistenFn | null = null
/** 跨 scope 切换的版本号：使旧 scope 的迟到事件失效 */
let loadSeq = 0
/** 工具循环轮次：用户输入归零，每次提案裁决续跑 +1，超上限自动续跑终止 */
const toolRound = ref(0)
/** 单次用户输入允许的最大工具循环轮次。
 *  ReAct 场景：3~4 次只读探索 + 2~3 次命令执行 = 6 轮上限 */
const MAX_TOOL_ROUNDS = 6

function uid(): string {
  return crypto.randomUUID()
}

/* ---------- 授权交互（混合形态：底部弹窗决策 + 流内记录留痕） ---------- */

/** 会话级命令豁免表（内存、命令精确匹配、重启即失效）：scope id → 已允许命令集合。
 *  前缀白名单为后续扩展点，MVP 先收窄为精确匹配以控制豁免面 */
const sessionAllowlist = new Map<string, Set<string>>()

/** agent 执行权限档（会话级内存态、不持久化）：manual=全部弹窗（默认）、
 *  risky=低危自动执行高危弹窗、full=全自动。切 scope 回落 manual（watch scope） */
const execPermission = ref<'manual' | 'risky' | 'full'>('manual')

/** 正在自动执行的提案（竞态防护）：applyProposal 为异步，ai-tool-calls 与
 *  ai-done 几乎同时到达，pumpProposals 若只看 state==='pending' 会把执行中的
 *  提案误入队弹窗（表象：豁免命中仍弹窗 + 命令重复执行） */
const autoRunning = new Set<AiToolProposal>()

/** 待裁决提案队列（按到达顺序逐个弹窗） */
const proposalQueue = ref<{ msgId: string; proposal: AiToolProposal }[]>([])
/** 当前弹窗中的裁决项 */
const authorizing = ref<{ msgId: string; proposal: AiToolProposal } | null>(null)
/** 本轮提案全部裁决完毕、等待续跑（由 pumpProposals 消化，避免多处触发续跑） */
const awaitingContinue = ref(false)

/** 是否存在未裁决提案（任一消息的 pending 提案或弹窗/队列中） */
function hasPendingProposal(): boolean {
  if (authorizing.value || proposalQueue.value.length) return true
  return messages.value.some(m => m.toolProposals?.some(p => p.state === 'pending'))
}

/** 是否为回复组尾（下一条非 assistant 或已是末条）：组 = 一次逻辑回复，
 *  复制/重新生成等操作按钮只出现在组尾——重新生成本就截断到对应 user 消息重跑整组 */
function isGroupTail(i: number): boolean {
  const next = messages.value[i + 1]
  return !next || next.role !== 'assistant'
}

/* ---------- 推理可视化 ---------- */

/** reasoning 是否正在流式接收（思考阶段：有 reasoning、无 content、正在 streaming） */
function isReasoningStreaming(m: { status: string; reasoning?: string; content: string }): boolean {
  return m.status === 'streaming' && !!m.reasoning && !m.content
}

/** reasoning 是否应展开：手动操作优先，否则思考阶段自动展开、content 到达后自动收起 */
function isReasoningOpen(m: { id: string; status: string; reasoning?: string; content: string }): boolean {
  const manual = reasoningManual.value.get(m.id)
  if (manual !== undefined) return manual
  return isReasoningStreaming(m)
}

/** 用户手动切换 reasoning 展开/收起 */
function toggleReasoning(id: string) {
  const msg = messages.value.find(m => m.id === id)
  if (!msg) return
  reasoningManual.value.set(id, !isReasoningOpen(msg))
}

/* ---------- 持久化 ---------- */

/** 落盘：streaming 态转为 stopped（应用崩溃/重载后的中间态收敛） */
function persist() {
  if (!isTauri) return
  const cleaned = messages.value.map(m =>
    m.status === 'streaming' ? { ...m, status: 'stopped' as const } : m,
  )
  const title = cleaned.find(m => m.role === 'user')?.content.slice(0, 20)
  void aiSaveChat(scope.value, {
    version: 1,
    scope: scope.value,
    title,
    updatedAt: Date.now(),
    messages: cleaned,
    summary: summary.value || undefined,
    summaryUpTo: summaryUpTo.value || undefined,
    extractedUpTo: extractedUpTo.value || undefined,
  }).catch(e => console.error('[AI] 会话保存失败:', e))
}

async function loadRecord(s: AiScope) {
  if (!isTauri) {
    messages.value = []
    summary.value = ''
    summaryUpTo.value = ''
    extractedUpTo.value = ''
    reasoningManual.value.clear()
    return
  }
  try {
    const rec = await aiLoadChat(s)
    messages.value = (rec?.messages ?? []).map(m =>
      m.status === 'streaming' ? { ...m, status: 'stopped' as const } : m,
    )
    summary.value = rec?.summary ?? ''
    summaryUpTo.value = rec?.summaryUpTo ?? ''
    extractedUpTo.value = rec?.extractedUpTo ?? ''
  } catch (e) {
    console.error('[AI] 会话加载失败:', e)
    messages.value = []
    summary.value = ''
    summaryUpTo.value = ''
    extractedUpTo.value = ''
    reasoningManual.value.clear()
  }
  scrollToBottom()
}

watch(scope, (s, oldS) => {
  loadSeq++
  streaming.value = false
  editFromId.value = null
  execPermission.value = 'manual' // 权限档随会话回落，杜绝「上次开的 full 忘了关」
  resetProposalState()
  // 取消旧会话在飞的只读工具 exec 任务，防止后端堆积（旧 scope 非 global 时才需取消）
  // 注意：ai_cancel_execs 需要后端 session_id（backendId），不是前端 scope
  if (oldS && oldS !== 'global') {
    const oldBackendId = sessions.value.find(s => s.id === oldS)?.backendId
    if (oldBackendId) void aiCancelExecs(oldBackendId).catch(() => {}) // 失败静默，取消是尽力而为
  }
  void loadRecord(s)
})

/* ---------- 事件 ---------- */

onMounted(async () => {
  if (!isTauri) return
  unlisten = await listenAiEvents({
    onToken: e => {
      if (e.scope !== scope.value || !streaming.value) return
      const last = messages.value[messages.value.length - 1]
      if (!last || last.role !== 'assistant') return
      if (e.kind === 'reasoning') {
        last.reasoning = (last.reasoning ?? '') + e.delta
      } else {
        last.content += e.delta
      }
      scrollToBottom()
    },
    onDone: e => {
      if (e.scope !== scope.value) return
      finishStreaming(e.reason === 'stopped' ? 'stopped' : 'done')
    },
    onError: e => {
      if (e.scope !== scope.value) return
      const last = messages.value[messages.value.length - 1]
      if (last && last.role === 'assistant') {
        last.status = 'error'
        if (!last.content) last.content = `请求失败：${e.message}`
      }
      streaming.value = false
      persist()
    },
    onToolCalls: e => {
      // 工具提案（M2.3）：写入当前流式 assistant 消息并先落盘防崩溃丢失；
      // 消息收尾（status/persist/摘要）由随后的 ai-done 统一处理。
      // 事件载荷为后端扁平形态 { id, name, arguments }，非 OpenAI 嵌套结构
      if (e.scope !== scope.value || !streaming.value) return
      const last = messages.value[messages.value.length - 1]
      if (!last || last.role !== 'assistant') return
      last.toolProposals = e.calls.map(c => {
        const args = c.arguments ?? ''
        const parsed = parseProposalArgs(args)
        return {
          id: c.id,
          name: c.name,
          args,
          reason: c.name === 'propose_command' ? parsed?.reason : undefined,
          state: 'pending' as const,
        }
      })
      // 按工具名路由分流：
      // - propose_command / edit_file / write_file：走授权路径（权限档决定自动/弹窗）
      // - 只读工具（read_file / list_directory / get_system_info）：自动执行，无需授权
      const allow = sessionAllowlist.get(scope.value)
      const WRITE_TOOLS = ['propose_command', 'edit_file', 'write_file']
      for (const p of last.toolProposals) {
        if (p.state !== 'pending') continue
        if (WRITE_TOOLS.includes(p.name)) {
          // 写操作：豁免表仅对 propose_command 精确匹配命令生效，文件编辑不进豁免表
          if (p.name === 'propose_command') {
            const cmd = parseProposalArgs(p.args)?.command
            if (!cmd) continue
            if (allow?.has(cmd)) {
              autoApply(p, 'session')
              continue
            }
          }
          if (execPermission.value === 'full') {
            // full 档：写操作全自动放行
            autoApply(p, 'once')
          }
          // manual / risky 档：写操作留 pending，由 pumpProposals 入队弹窗
        } else {
          // 只读工具：无条件自动执行（不经授权弹窗）
          autoExecReadonly(p)
        }
      }
      persist()
    },
  })
  void loadRecord(scope.value)
})

onUnmounted(() => unlisten?.())

function finishStreaming(status: 'done' | 'stopped') {
  const last = messages.value[messages.value.length - 1]
  if (last && last.role === 'assistant') last.status = status
  streaming.value = false
  persist()
  scrollToBottom()
  // 提案调度：pending 入队弹窗；无 pending 且本轮已裁决完 → 自动续跑
  pumpProposals()
  void maybeSummarize()
  // 记忆提取仅在正常完成后触发（中止/错误轮次内容不完整，不提取）
  if (status === 'done') void maybeExtractMemories()
}

/* ---------- 滚动摘要（M1.5） ---------- */

/**
 * 对话完成后后台检查：未摘要部分超阈值时，把除最近 KEEP 条外的消息
 * 连同旧摘要发给模型压缩。失败静默（下次完成后再试），不影响对话。
 */
async function maybeSummarize() {
  if (!isTauri || summarizing || streaming.value) return
  const startIdx = summaryUpTo.value
    ? messages.value.findIndex(m => m.id === summaryUpTo.value) + 1
    : 0
  const pending = messages.value
    .slice(Math.max(startIdx, 0))
    .filter(m => m.status !== 'streaming' && m.status !== 'error' && m.content.trim())
  const chars = pending.reduce((n, m) => n + m.content.length, 0)
  if (chars < SUMMARY_TRIGGER_CHARS) return

  const compress = pending.slice(0, Math.max(0, pending.length - SUMMARY_KEEP_RECENT))
  if (compress.length === 0) return

  summarizing = true
  const seq = loadSeq // 期间切换 scope 则丢弃结果
  try {
    const input: AiWireMessage[] = []
    if (summary.value) {
      input.push({ role: 'user', content: `【旧摘要】\n${summary.value}` })
    }
    input.push(...compress.map(m => ({ role: m.role, content: m.content })))
    const next = await aiSummarize(input)
    if (seq !== loadSeq) return
    summary.value = next
    summaryUpTo.value = compress[compress.length - 1].id
    persist()
    console.info('[AI] 滚动摘要已更新，覆盖至', summaryUpTo.value)
  } catch (e) {
    console.warn('[AI] 摘要生成失败（不影响对话）:', e)
  } finally {
    summarizing = false
  }
}

/* ---------- 跨会话记忆提取（M3.3） ---------- */

/**
 * 对话完成后后台检查：未提取部分超阈值时，把新对话连同已有记忆发给模型
 * 提取跨会话事实，合并去重后落盘。失败静默（不更新 extractedUpTo，
 * 下次完成后再试），不影响对话。已摘要部分不再重复提取。
 */
async function maybeExtractMemories() {
  if (!isTauri || extracting || streaming.value) return
  if (!savedSettings.aiMemoryEnabled) return
  const floorIdx = summaryUpTo.value
    ? messages.value.findIndex(m => m.id === summaryUpTo.value) + 1
    : 0
  const startIdx = extractedUpTo.value
    ? messages.value.findIndex(m => m.id === extractedUpTo.value) + 1
    : 0
  const pending = messages.value
    .slice(Math.max(floorIdx, startIdx, 0))
    .filter(m => m.status !== 'streaming' && m.status !== 'error' && m.content.trim())
  const chars = pending.reduce((n, m) => n + m.content.length, 0)
  if (chars < EXTRACT_TRIGGER_CHARS) return

  extracting = true
  const seq = loadSeq // 期间切换 scope 则丢弃结果
  try {
    const existing = await aiListMemories()
    const input: AiWireMessage[] = []
    if (existing.length) {
      input.push({
        role: 'user',
        content: `【已有记忆】\n${JSON.stringify(existing.map(e => e.content))}`,
      })
    }
    input.push(...pending.map(m => ({ role: m.role, content: m.content })))
    const raw = await aiExtractMemories(input)
    if (seq !== loadSeq) return
    const merged = mergeMemories(existing, parseExtracted(raw))
    await aiSaveMemories(merged)
    extractedUpTo.value = pending[pending.length - 1].id
    persist()
    console.info('[AI] 记忆提取完成，当前共', merged.length, '条')
  } catch (e) {
    console.warn('[AI] 记忆提取失败（不影响对话）:', e)
  } finally {
    extracting = false
  }
}

/** 解析模型提取输出：宽容剥离 markdown 围栏后 JSON.parse，逐项校验 content，
 *  单条截断至 200 字符；解析失败返回空数组（跳过本轮提取） */
function parseExtracted(raw: string): string[] {
  const stripped = raw.trim().replace(/^```(?:json)?\s*|\s*```$/g, '')
  let arr: unknown
  try {
    arr = JSON.parse(stripped)
  } catch {
    return []
  }
  if (!Array.isArray(arr)) return []
  const out: string[] = []
  for (const item of arr) {
    const c = (item as { content?: unknown })?.content
    if (typeof c === 'string' && c.trim()) {
      // Array.from 按码点切分，避免截断代理对（与后端 chars 口径一致）
      out.push(Array.from(c.trim()).slice(0, AI_MEMORY_CONTENT_MAX_CHARS).join(''))
    }
  }
  return out
}

/** 合并提取结果与已有记忆：content 精确匹配去重（保留旧条目 id/时间），
 *  新条目按提取输出顺序追加在后；总数超设置上限时从头部淘汰最旧 */
function mergeMemories(existing: AiMemoryItem[], extracted: string[]): AiMemoryItem[] {
  const seen = new Set(existing.map(e => e.content))
  const now = Date.now()
  const added = extracted
    .filter(c => !seen.has(c))
    .map((c, i): AiMemoryItem => ({
      id: crypto.randomUUID(),
      content: c,
      createdAt: now + i,
      sourceScope: scope.value,
    }))
  const merged = [...existing, ...added].sort((a, b) => a.createdAt - b.createdAt)
  const limit = savedSettings.aiMemoryLimit
  return merged.length > limit ? merged.slice(merged.length - limit) : merged
}

/* ---------- 发送 ---------- */

function buildWireMessages(assets?: AiPromptAssets, memories?: AiMemoryItem[]): AiWireMessage[] {
  // 仅把已成形的消息发给模型；空回复/错误回复不进上下文（带提案的空正文 assistant 除外）
  const usable = messages.value.filter(
    m => m.status !== 'streaming' && (m.content.trim() || m.toolProposals?.length),
  )
  // 摘要覆盖点之后的消息原文携带；覆盖点之前的以 system 摘要注入
  const idx = summaryUpTo.value ? usable.findIndex(m => m.id === summaryUpTo.value) : -1
  const rest = idx >= 0 ? usable.slice(idx + 1) : usable
  const wired: AiWireMessage[] = []
  for (const m of rest) {
    // 工具轮次：assistant 提案消息展开为 tool_calls + 对应 tool 结果
    // （OpenAI 协议要求每个 tool_call 都有应答，pending/expired 同样补齐，见 toolResultOf）
    if (m.role === 'assistant' && m.toolProposals?.length) {
      wired.push({
        role: 'assistant',
        content: m.content || null,
        tool_calls: m.toolProposals.map(p => ({
          id: p.id,
          type: 'function' as const,
          function: { name: p.name, arguments: p.args },
        })),
      })
      for (const p of m.toolProposals) {
        wired.push({ role: 'tool', tool_call_id: p.id, content: toolResultOf(p) })
      }
      continue
    }
    wired.push({ role: m.role, content: m.content })
  }
  // system 前缀按「规则 → 技能 → 记忆 → 摘要 → 会话上下文」顺序装配
  // （注意力靠后原则：越稳定、越全局的约定越靠前；合并为单条，兼容多 system 支持不一的上游）
  const systems: AiWireMessage[] = []
  const rulesSys = buildAssetSystem(
    '规则', 'rules', savedSettings.aiRulesEnabled, savedSettings.aiRulesDisabled, assets?.rules ?? [],
  )
  if (rulesSys) systems.push(rulesSys)
  const skillsSys = buildAssetSystem(
    '技能', 'skills', savedSettings.aiSkillsEnabled, savedSettings.aiSkillsDisabled, assets?.skills ?? [],
  )
  if (skillsSys) systems.push(skillsSys)
  const memSys = buildMemorySystem(savedSettings.aiMemoryEnabled, memories ?? [])
  if (memSys) systems.push(memSys)
  if (summary.value) {
    systems.push({
      role: 'system',
      content: `以下是此前对话的摘要：\n${summary.value}\n\n请结合摘要与近期消息继续对话。`,
    })
  }
  // 会话上下文 system 靠近对话（注意力靠后），置于最末
  const ctx = buildSessionContext()
  if (ctx) systems.push(ctx)
  return [...systems, ...wired]
}

/** 单类（规则/技能）合计注入预算：16KB，按 UTF-8 字节计量（与后端单文件 8KB 口径一致；
 *  不用 string.length——UTF-16 code unit 对中文计量偏差约 3 倍） */
const ASSET_BUDGET_BYTES = 16 * 1024

/** 组装一类提示资产（规则/技能）的合并 system 消息；总开关关闭或无可用文件返回 null。
 *  按禁用名单过滤，截断文件标题标注；超合计预算丢弃靠后文件并在末尾标注 */
function buildAssetSystem(
  kindLabel: '规则' | '技能',
  dirName: 'rules' | 'skills',
  enabled: boolean,
  disabled: string[],
  assets: AiPromptAsset[],
): AiWireMessage | null {
  if (!enabled) return null
  const files = assets.filter(a => !disabled.includes(a.fileName))
  if (files.length === 0) return null
  const header = kindLabel === '规则'
    ? '# 用户自定义规则（遵循以下约定响应）'
    : '# 用户自定义技能（参考以下能力说明响应）'
  const enc = new TextEncoder()
  const parts: string[] = []
  let used = 0
  let dropped = 0
  for (const a of files) {
    const block = `## ${a.name}（${dirName}/${a.fileName}${a.truncated ? '，内容已截断' : ''}）\n\n${a.content}`
    const bytes = enc.encode(block).length
    if (used + bytes > ASSET_BUDGET_BYTES) {
      dropped++
      continue
    }
    used += bytes
    parts.push(block)
  }
  if (parts.length === 0) return null
  let content = `${header}\n\n${parts.join('\n\n')}`
  if (dropped > 0) content += `\n\n（其余 ${dropped} 个文件因总量超限未注入）`
  return { role: 'system', content }
}

/** 记忆注入合计预算：8KB UTF-8 字节（与技能/规则单类 16KB 同口径计量；
 *  100 条 × 200 字符最坏约 20KB，8KB 约可注入 40 条，覆盖绝大多数场景） */
const MEMORY_BUDGET_BYTES = 8 * 1024

/** 组装跨会话记忆 system 消息；总开关关闭或无记忆返回 null。
 *  记忆按 createdAt 升序存储，预算不足时从最旧开始丢弃，末尾标注 */
function buildMemorySystem(enabled: boolean, memories: AiMemoryItem[]): AiWireMessage | null {
  if (!enabled || memories.length === 0) return null
  const header = '# 跨会话记忆（以下是你与用户的历史交互中记住的事实）'
  const enc = new TextEncoder()
  const kept: string[] = []
  let used = 0
  let dropped = 0
  for (let i = memories.length - 1; i >= 0; i--) {
    const line = `- ${memories[i].content}`
    const bytes = enc.encode(line).length
    if (used + bytes > MEMORY_BUDGET_BYTES) {
      dropped++
      continue
    }
    used += bytes
    kept.unshift(line)
  }
  if (kept.length === 0) return null
  let content = `${header}\n\n${kept.join('\n')}`
  if (dropped > 0) content += `\n\n（其余 ${dropped} 条记忆因总量超限未注入）`
  return { role: 'system', content }
}

/* ---------- 工具提案裁决（M2.3 前端循环） ---------- */

/** 解析提案 arguments（JSON 字符串）；解析失败返回 null（卡片禁用执行并显示原文） */
function parseProposalArgs(args: string): { command?: string; reason?: string } | null {
  try {
    const v = JSON.parse(args)
    return typeof v === 'object' && v !== null ? v : null
  } catch {
    return null
  }
}

/** 卡片展示的命令原文：参数可解析取 command，否则显示原始 arguments 便于人工辨认 */
function proposalCommand(p: AiToolProposal): string {
  return parseProposalArgs(p.args)?.command ?? p.args
}

/** 只读工具中文标签 */
function readonlyLabel(name: string): string {
  return (
    {
      read_file: '读取文件',
      list_directory: '列出目录',
      get_system_info: '系统信息',
      edit_file: '编辑文件',
      write_file: '写入文件',
    }[name] ?? name
  )
}

/** 工具参数展示（get_system_info 无参数，其余取 path） */
function readonlyArgs(p: AiToolProposal): string {
  if (p.name === 'get_system_info') return ''
  try {
    const v = JSON.parse(p.args)
    return typeof v?.path === 'string' ? v.path : p.args
  } catch {
    return p.args
  }
}

/** edit_file 的 old_text 预览 */
function fileEditOld(p: AiToolProposal): string {
  try {
    const v = JSON.parse(p.args)
    return typeof v?.old_text === 'string' ? v.old_text : ''
  } catch {
    return ''
  }
}

/** edit_file 的 new_text 或 write_file 的 content 预览 */
function fileEditNew(p: AiToolProposal): string {
  try {
    const v = JSON.parse(p.args)
    if (p.name === 'edit_file') return typeof v?.new_text === 'string' ? v.new_text : ''
    return typeof v?.content === 'string' ? v.content : ''
  } catch {
    return ''
  }
}

/** 计算文本行数（空文本算 0 行） */
function countLines(text: string): number {
  if (!text) return 0
  return text.split('\n').length
}

/** 提案裁决结果 → tool 结果消息内容（模型据此了解裁决与执行情况）。
 *  只读工具直接返回原始输出文本，模型据此理解远程环境；
 *  propose_command 保持既有结构化 JSON 格式。 */
function toolResultOf(p: AiToolProposal): string {
  // 只读工具：直接返回输出文本（模型据此理解远程环境）
  if (p.name !== 'propose_command') {
    if (p.error) return JSON.stringify({ error: p.error })
    return p.execOutput ?? '(无输出)'
  }
  // propose_command：既有结构化 JSON 格式
  if (p.state === 'accepted') {
    if (p.error) return JSON.stringify({ accepted: false, error: p.error })
    const out = p.execOutput ?? ''
    return out
      ? JSON.stringify({ accepted: true, output: out })
      : JSON.stringify({ accepted: true })
  }
  if (p.state === 'rejected') return JSON.stringify({ accepted: false, rejected: true })
  return JSON.stringify({ accepted: false, expired: true })
}

/** 组装 SSH 会话上下文 system 消息；全局对话（home）或无会话时返回 null */
function buildSessionContext(): AiWireMessage | null {
  if (appView.value !== 'workbench') return null
  const host = activeHost.value
  const sid = activeSessionId.value
  if (!host || !sid) return null

  const parts = [
    '当前 SSH 会话上下文（回答时以此为背景，用户消息可能省略主机信息）：',
    `- 主机：${host.id}（${host.user}@${host.ip}:${host.port}）`,
  ]
  return { role: 'system', content: parts.join('\n') }
}

function send(override?: string) {
  const input = (override ?? draft.value).trim()
  if (!input || streaming.value) return

  // 编辑重发：截断发生在真正发送时（填回时不截断，用户可能取消编辑）
  if (editFromId.value) {
    const cut = messages.value.findIndex(m => m.id === editFromId.value)
    editFromId.value = null
    if (cut >= 0) {
      messages.value = messages.value.slice(0, cut)
      rollbackSummaryAfterCut(cut)
    }
  }

  // 存在未裁决提案：拦截发送（弹窗覆盖输入框，用户须先完成授权——强制力来自模态）
  if (hasPendingProposal()) {
    toast('有命令提案等待确认，请先完成授权', 'warn', 2400)
    return
  }
  // 兜底：会话切换/载入遗留的失效转换（正常路径已由 resetProposalState 处理）
  for (const m of messages.value) {
    m.toolProposals?.forEach(p => {
      if (p.state === 'pending') p.state = 'expired'
    })
  }
  toolRound.value = 0

  // 终端引用块并入本次 user 消息（引用在前，问题在后；模型对标记内内容按上下文对待）
  let text = input
  if (quote.value) {
    text = `<terminal-output>\n${quote.value.text}\n</terminal-output>\n\n${input}`
    quote.value = null
  }

  const seq = ++loadSeq
  messages.value.push({
    id: uid(),
    role: 'user',
    content: text,
    status: 'done',
    createdAt: Date.now(),
  })
  messages.value.push({
    id: uid(),
    role: 'assistant',
    content: '',
    status: 'streaming',
    createdAt: Date.now(),
  })
  streaming.value = true
  draft.value = ''
  scrollToBottom()
  persist() // 先落盘用户消息，防止崩溃丢失

  if (!isTauri) {
    // 浏览器 dev 降级：占位回复
    window.setTimeout(() => {
      if (seq !== loadSeq) return
      const last = messages.value[messages.value.length - 1]
      if (last) {
        last.content = `已收到：${text}\n（浏览器预览模式，未连接模型服务）`
        last.status = 'done'
      }
      streaming.value = false
    }, 400)
    return
  }

  runAiStream(seq)
}

/** 发起流式请求（send 与提案裁决续跑共用）。工具提案仅在工作台会话下启用
 *  （全局对话无终端可写）；请求失败写入最后一条 assistant 消息。
 *  技能/规则资产与记忆每轮重新拉取：无缓存实时读，下一轮即最新 */
async function runAiStream(seq: number) {
  let assets: AiPromptAssets | undefined
  let memories: AiMemoryItem[] | undefined
  try {
    ;[assets, memories] = await Promise.all([aiListPromptAssets(), aiListMemories()])
  } catch (e) {
    // 资产/记忆加载失败降级为空注入，对话主流程不受影响
    console.warn('[AI] 技能/规则/记忆加载失败（按空注入）:', e)
  }
  if (seq !== loadSeq) return // 拉取期间切换了 scope，丢弃本次发送
  aiChatStream(scope.value, buildWireMessages(assets, memories), {
    enableTools: scope.value !== 'global',
  }).catch(e => {
    if (seq !== loadSeq) return
    const last = messages.value[messages.value.length - 1]
    if (last && last.role === 'assistant') {
      last.status = 'error'
      last.content = `请求失败：${typeof e === 'string' ? e : (e as Error)?.message ?? e}`
    }
    persist()
  })
}

/** 提案裁决后的自动续跑：追加新的 assistant 流式消息再发起请求 */
function continueToolRound() {
  const seq = ++loadSeq
  messages.value.push({
    id: uid(),
    role: 'assistant',
    content: '',
    status: 'streaming',
    createdAt: Date.now(),
  })
  streaming.value = true
  scrollToBottom()
  persist()
  runAiStream(seq)
}

/** 提案消化调度器：streaming 结束后调用（finishStreaming / 裁决完 / 自动执行完）。
 *  ① 最新 assistant 消息的 pending 提案入队并弹出首项（执行中的 autoRunning 提案
 *  不入队，由完成回调推进）；② 无 pending 且 awaitingContinue → 推进轮次并续跑。
 *  幂等：awaitingContinue 消费即清 */
function pumpProposals() {
  if (streaming.value) return
  const last = [...messages.value].reverse().find(m => m.role === 'assistant')
  if (last?.toolProposals) {
    for (const p of last.toolProposals) {
      // 只读工具已在 autoRunning 中（autoExecReadonly 同步 add），不会进入此分支；
      // 写操作（propose_command / edit_file / write_file）需弹窗授权
      if (
        p.state === 'pending' &&
        ['propose_command', 'edit_file', 'write_file'].includes(p.name) &&
        !autoRunning.has(p)
      ) {
        proposalQueue.value.push({ msgId: last.id, proposal: p })
      }
    }
  }
  if (proposalQueue.value.length) {
    authorizing.value = proposalQueue.value[0]
    return
  }
  if (!awaitingContinue.value) return
  awaitingContinue.value = false
  toolRound.value++
  if (toolRound.value > MAX_TOOL_ROUNDS) {
    toast(`已达到工具循环轮次上限（${MAX_TOOL_ROUNDS}），如需继续请重新提问`, 'warn', 3200)
    return
  }
  continueToolRound()
}

/** 本轮收尾（各裁决/自动执行完成路径统一调用）：仅当最后一条 assistant 消息
 *  的提案全部落定（无 pending 且无执行中）才标记待续跑，交 pumpProposals 推进 */
function settleProposalRound() {
  const last = [...messages.value].reverse().find(m => m.role === 'assistant')
  const unsettled = last?.toolProposals?.some(p => p.state === 'pending' || autoRunning.has(p))
  if (unsettled) {
    pumpProposals()
    return
  }
  awaitingContinue.value = true
  pumpProposals()
}

/** 自动执行入口（豁免表命中 / 权限档放行）：标记执行中防 pump 误入队，
 *  完成后统一收尾推进。捕获 scope 防执行期间切会话错乱 */
function autoApply(p: AiToolProposal, choice: 'once' | 'session') {
  const atScope = scope.value
  autoRunning.add(p)
  void applyProposal(p, choice).finally(() => {
    autoRunning.delete(p)
    if (scope.value === atScope) settleProposalRound()
  })
}

/** 只读工具自动执行：经 SSH exec channel 执行，结果写入 execOutput，
 *  完成后统一交 settleProposalRound 收尾推进（与 autoApply 同路径）。
 *  状态分流：
 *  - timedOut=true（超时，含部分输出）→ accepted（部分观察仍有价值）
 *  - exitCode=null 且 timedOut=false（通道级故障）→ rejected
 *  - exitCode=Some(n)（成功/命令业务失败）→ accepted（观察动作完成） */
function autoExecReadonly(p: AiToolProposal) {
  const atScope = scope.value
  // 用 backendId（后端 SessionManager 的 key），不是前端 activeSessionId
  const sid = activeSession.value?.backendId
  if (!sid || activeSession.value?.state !== 'online') {
    p.state = 'rejected'
    p.error = '无可用会话或会话已断开'
    settleProposalRound()
    return
  }
  autoRunning.add(p) // 竞态防护：pumpProposals 不入队执行中的只读提案
  aiExecTool(sid, p.name, p.args)
    .then((result: AiToolExecResult) => {
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
    })
    .catch(e => {
      if (scope.value !== atScope) return
      p.state = 'rejected'
      p.error = typeof e === 'string' ? e : (e as Error)?.message ?? String(e)
    })
    .finally(() => {
      autoRunning.delete(p)
      if (scope.value === atScope) settleProposalRound()
    })
}

/** 应用裁决：写状态并按需执行（once/session 均执行，session 另记入豁免表）。
 *  执行方式按工具名分流：
 *  - propose_command → writeExec（PTY 写入用户终端）
 *  - edit_file / write_file → aiExecTool（exec channel，不污染终端）
 *  自动豁免路径在 onToolCalls 中直接调用（无弹窗） */
async function applyProposal(p: AiToolProposal, choice: 'once' | 'session' | 'reject') {
  if (choice === 'reject') {
    p.state = 'rejected'
    persist()
    scrollToBottom()
    return
  }
  if (p.name === 'propose_command') {
    const parsed = parseProposalArgs(p.args)
    if (!parsed?.command) {
      p.state = 'rejected'
    } else {
      const multiline = parsed.command.includes('\n')
      if (multiline && !savedSettings.aiMultilineExec) {
        p.state = 'accepted'
        p.error = '多行命令直接执行未开启（设置 → AI → 终端执行）'
      } else {
        const { error, output } = await writeExec(parsed.command, multiline)
        p.state = 'accepted'
        p.allowedSession = choice === 'session'
        if (error) p.error = error
        if (output) p.execOutput = output
        if (choice === 'session' && !error) {
          const set = sessionAllowlist.get(scope.value) ?? new Set<string>()
          set.add(parsed.command)
          sessionAllowlist.set(scope.value, set)
        }
      }
    }
  } else {
    // edit_file / write_file：经 exec channel 执行，不写入 PTY
    const sid = activeSession.value?.backendId
    if (!sid || activeSession.value?.state !== 'online') {
      p.state = 'rejected'
      p.error = '无可用会话或会话已断开'
    } else {
      try {
        const result = await aiExecTool(sid, p.name, p.args)
        p.execOutput = result.output
        if (result.truncated) p.execOutput += '\n[输出超 8KB 已截断]'
        if (result.timedOut) {
          p.state = 'accepted'
          p.execOutput += p.execOutput ? '\n[执行超时，输出可能不完整]' : '[执行超时，无输出]'
        } else if (result.exitCode === null) {
          p.state = 'rejected'
          p.error = '执行失败（会话断开）'
        } else if (result.exitCode === 0) {
          p.state = 'accepted'
        } else {
          // exitCode>0：old_text 未匹配、权限不足等业务失败
          p.state = 'accepted'
          if (!p.execOutput) p.execOutput = `命令返回非零退出码 ${result.exitCode}`
        }
      } catch (e) {
        p.state = 'rejected'
        p.error = typeof e === 'string' ? e : (e as Error)?.message ?? String(e)
      }
    }
  }
  persist()
  scrollToBottom()
}

/** 弹窗裁决当前项：应用后统一交 settleProposalRound 收尾（等剩余提案落定才续跑） */
async function resolveCurrent(choice: 'once' | 'session' | 'reject') {
  const cur = authorizing.value
  if (!cur) return
  authorizing.value = null
  proposalQueue.value = proposalQueue.value.filter(x => x.proposal !== cur.proposal)
  await applyProposal(cur.proposal, choice)
  settleProposalRound()
}

/** 会话切换：清裁决状态，遗留 pending 提案置为失效（卡片置灰，wire 回传 expired） */
function resetProposalState() {
  authorizing.value = null
  proposalQueue.value = []
  awaitingContinue.value = false
  autoRunning.clear()
  for (const m of messages.value) {
    m.toolProposals?.forEach(p => {
      if (p.state === 'pending') p.state = 'expired'
    })
  }
}

/* ---------- 消息操作（复制 / 重新生成 / 编辑重发 / 删除） ---------- */

/**
 * 截断/删除后按保守规则回滚滚动摘要：被删消息落在摘要覆盖范围内
 * （覆盖点已不存在，或覆盖点位于被删区间之后）则整体清空。
 * 理由：摘要由被删消息合并而来，删除后无法重建，宁可回退为原文携带。
 */
function rollbackSummaryAfterCut(cutIndex: number) {
  if (!summaryUpTo.value) return
  const upIdx = messages.value.findIndex(m => m.id === summaryUpTo.value)
  if (upIdx < 0 || cutIndex <= upIdx) {
    summary.value = ''
    summaryUpTo.value = ''
  }
}

async function copyText(text: string) {
  try {
    await writeText(text)
    toast('已复制', 'ok', 1500)
  } catch (e) {
    console.error('[clipboard] writeText failed', e)
    toast('复制失败：剪贴板不可用', 'err')
  }
}

/** 重新生成：定位该回复对应的 user 消息，截断其后全部消息后重新发起 */
function regenerate(m: AiChatMessage) {
  if (streaming.value) return
  const idx = messages.value.indexOf(m)
  if (idx < 0) return
  let userIdx = -1
  for (let i = idx - 1; i >= 0; i--) {
    if (messages.value[i].role === 'user') {
      userIdx = i
      break
    }
  }
  if (userIdx < 0) return
  const userContent = messages.value[userIdx].content
  messages.value = messages.value.slice(0, userIdx)
  rollbackSummaryAfterCut(userIdx)
  editFromId.value = null // 避免随后的 send 再次按编辑点截断
  send(userContent)
}

/** 编辑重发：仅填回输入框并聚焦，截断推迟到真正发送时 */
function startEdit(m: AiChatMessage) {
  if (streaming.value) return
  editFromId.value = m.id
  draft.value = m.content
  nextTick(() => inputRef.value?.focus())
}

/** 删除 user 消息及紧随其后的 assistant 回复，不弹确认框 */
function deleteMessage(m: AiChatMessage) {
  if (streaming.value) return
  const idx = messages.value.indexOf(m)
  if (idx < 0) return
  const count = messages.value[idx + 1]?.role === 'assistant' ? 2 : 1
  messages.value.splice(idx, count)
  rollbackSummaryAfterCut(idx)
  persist()
}

/* ---------- 代码块终端执行（M2.2，复用 sendInput → write_terminal） ---------- */

/** 直接执行确认弹窗状态：null 关闭；multiline/lines 供警示文案；risk 为命中的高危规则（无则 null） */
const execConfirm = ref<{ code: string; multiline: boolean; lines: number; risk: string | null } | null>(null)

/** 代码块终端按钮可用性：workbench + 在线会话 + 非 streaming（AiMarkdown 据此显隐按钮） */
const terminalReady = computed(
  () =>
    appView.value === 'workbench' &&
    !streaming.value &&
    !!activeSession.value?.backendId &&
    activeSession.value?.state === 'online',
)

/** AiMarkdown emit 的终端操作意图：目标会话 = 点击时的当前活动会话（设计 §6.1 裁决） */
function onTerminalIntent(payload: { code: string; run: boolean }) {
  const s = activeSession.value
  if (appView.value !== 'workbench' || !s?.backendId || s.state !== 'online') {
    toast('无可用会话，无法写入终端', 'err')
    return
  }
  if (!payload.run) {
    // 插入终端：不追加回车，用户可先编辑。
    // 多行文本裸写时中间 \n 即回车、会被 shell 逐行执行，故用 bracketed paste
    // 包装（\x1b[200~/[201~）：支持 2004 模式的 shell（bash 4.4+/zsh/fish 等主流）
    // 会把整块内容插入编辑缓冲区，等用户回车才执行；不支持的 shell 显示转义序列
    // 字面量，同样不会执行
    const text = payload.code
    sendInput(s.id, text.includes('\n') ? `\x1b[200~${text}\x1b[201~` : text)
    toast('已插入终端', 'ok', 1500)
    return
  }
  const trimmed = payload.code.replace(/\n$/, '')
  const multiline = trimmed.includes('\n')
  if (multiline && !savedSettings.aiMultilineExec) {
    toast('多行命令直接执行未开启，可在设置 → AI → 终端执行中开启', 'warn', 2800)
    return
  }
  // 确认模式三态：never 点击即执行（按钮点击本身已是意图确认）；
  // always 一律弹窗；risky 仅命中高危规则（内置 + 自定义关键词）时弹窗
  const mode = savedSettings.aiExecConfirmMode
  if (mode === 'never') {
    writeExec(trimmed, multiline)
    return
  }
  const hit = matchHighRisk(trimmed, savedSettings.aiRiskKeywords.split('\n'))
  if (mode === 'risky' && !hit) {
    writeExec(trimmed, multiline)
    return
  }
  // 存 trimmed：确认后统一追加单个 \n，避免原文本尾随换行造成多余空提示符行
  execConfirm.value = { code: trimmed, multiline, lines: trimmed.split('\n').length, risk: hit?.rule ?? null }
}

/** 按行数选择写入方式：多行 bracketed paste 整体提交 / 单行裸写 + 回车。
 *  返回错误说明（null 成功）；代码块确认路径 toast 展示，提案路径记入卡片 */
/** 命令写入执行：弹窗确认与提案裁决共用。aiAutoReturnOutput=true 时等待输出返回 */
async function writeExec(code: string, multiline: boolean): Promise<{ error: string | null; output?: string }> {
  const s = activeSession.value
  if (!s?.backendId || s.state !== 'online') return { error: '会话已断开，命令未执行' }
  const payload = multiline ? `\x1b[200~${code}\x1b[201~\n` : `${code}\n`
  try {
    if (savedSettings.aiAutoReturnOutput) {
      const { output, timedOut } = await sendInputAndCapture(s.id, payload)
      if (timedOut) return { error: null, output: `${output}\n[超时 30s，输出可能不完整]` }
      return { error: null, output }
    }
    await sendInput(s.id, payload)
  } catch (e) {
    return { error: `写入终端失败：${typeof e === 'string' ? e : (e as Error)?.message ?? e}` }
  }
  toast('命令已发送到终端', 'ok', 1500)
  return { error: null }
}

function confirmExec() {
  const c = execConfirm.value
  execConfirm.value = null
  if (!c) return
  void writeExec(c.code, c.multiline).then(({ error }) => {
    if (error) toast(error, 'err')
  })
}

function cancelExec() {
  execConfirm.value = null
}

function onExecKey(e: KeyboardEvent) {
  if (authorizing.value) return // 授权弹窗打开期间让位（互斥）
  if (e.key === 'Escape') {
    e.preventDefault()
    e.stopPropagation()
    cancelExec()
  } else if (e.key === 'Enter') {
    e.preventDefault()
    e.stopPropagation()
    confirmExec()
  }
}

watch(execConfirm, open => {
  if (open) window.addEventListener('keydown', onExecKey, true)
  else window.removeEventListener('keydown', onExecKey, true)
})

onUnmounted(() => window.removeEventListener('keydown', onExecKey))

/** 授权弹窗键盘裁决：数字键、Enter=仅本次、Esc=拒绝（capture 抢先，与 exec 弹窗互斥）。
 *  propose_command：1=仅本次 2=本会话允许 3=拒绝
 *  edit_file/write_file：1=仅本次 2=拒绝（无会话豁免） */
function onAuthKey(e: KeyboardEvent) {
  if (!authorizing.value || execConfirm.value) return
  const isCmd = authorizing.value.proposal.name === 'propose_command'
  const map: Record<string, 'once' | 'session' | 'reject'> = isCmd
    ? { '1': 'once', '2': 'session', '3': 'reject', Enter: 'once', Escape: 'reject' }
    : { '1': 'once', '2': 'reject', Enter: 'once', Escape: 'reject' }
  const choice = map[e.key]
  if (!choice) return
  e.preventDefault()
  e.stopPropagation()
  void resolveCurrent(choice)
}
watch(authorizing, open => {
  if (open) window.addEventListener('keydown', onAuthKey, true)
  else window.removeEventListener('keydown', onAuthKey, true)
})
onUnmounted(() => window.removeEventListener('keydown', onAuthKey))

function stop() {
  if (!streaming.value) return
  void aiStopChat(scope.value)
}

function clearChat() {
  messages.value = []
  streaming.value = false
  editFromId.value = null
  summary.value = ''
  summaryUpTo.value = ''
  reasoningManual.value.clear()
  if (isTauri) void aiClearChat(scope.value).catch(e => console.error('[AI] 清空失败:', e))
}

/* ---------- UI 辅助 ---------- */

// 右面板切到 AI 并展开时自动聚焦
watch(
  [rightPanelVisible, rightPanelTab],
  ([visible, tab]) => {
    if (visible && tab === 'ai') nextTick(() => inputRef.value?.focus())
  },
  { flush: 'post' },
)

// 终端右键「添加到 AI 对话」：一次性消费引用事件
watch(aiQuote, q => {
  if (!q) return
  quote.value = { text: q.text, lines: q.text.split('\n').length }
  aiQuote.value = null
  nextTick(() => inputRef.value?.focus())
})

function scrollToBottom() {
  nextTick(() => {
    if (scrollEl.value) scrollEl.value.scrollTop = scrollEl.value.scrollHeight
  })
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault()
    send()
  }
}

const quickHints = ['nginx 502 怎么排查？', '解释这条日志的含义', '生成一个 systemd service 文件']
function insertHint(h: string) {
  draft.value = h
  nextTick(() => inputRef.value?.focus())
}

/** AI 未启用/未配置时的引导文案 */
const configHint = computed(() => {
  if (!savedSettings.aiEnabled) return 'AI 助手未启用，请先在设置中开启'
  if (!savedSettings.aiBaseUrl || !savedSettings.aiModel) return '尚未配置模型，请先在设置中填写接口地址与模型'
  return ''
})
</script>

<template>
  <div class="ai-panel">
    <!-- 操作按钮注入 RightPanel 头部 -->
    <Teleport v-if="rightPanelTab === 'ai'" to="#rp-head-actions">
      <button class="ai-action-btn" title="清空对话" :disabled="messages.length === 0" @click="clearChat">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
             stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="3 6 5 6 21 6" />
          <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
        </svg>
      </button>
    </Teleport>

    <!-- 消息列表 -->
    <div ref="scrollEl" class="chat-scroll">
      <template v-if="messages.length === 0">
        <div class="empty">
          <div class="empty-title">AI 助手</div>
          <div class="empty-sub">
            {{ appView === 'home' ? '全局问答 · 不绑定任何会话' : '已自动关联当前会话上下文' }}
          </div>
          <div v-if="configHint" class="config-hint">{{ configHint }}</div>
          <div v-else class="hints">
            <button v-for="h in quickHints" :key="h" class="hint-pill" @click="insertHint(h)">
              {{ h }}
            </button>
          </div>
        </div>
      </template>

      <template v-else>
        <!-- 回复组：同一问题触发的连续 assistant 消息（工具循环中间轮 + 最终回答）
             视觉合并为一个连续回复——仅首条显示 logo 头，组内间距收紧 -->
        <div
          v-for="(m, i) in messages"
          :key="m.id"
          class="bubble"
          :class="[m.role, m.status, { grouped: i > 0 && m.role === 'assistant' && messages[i - 1].role === 'assistant' }]"
        >
          <!-- AI 消息头：项目 logo + Agent 标识 -->
          <div v-if="m.role === 'assistant'" class="msg-head">
            <span class="agent-logo">
              <AppLogo />
            </span>
            <span class="agent-name">Agent</span>
          </div>
          <!-- 思考过程：流式时自动展开，content 到达后自动收起；用户可手动切换 -->
          <div v-if="m.reasoning" class="reasoning" :class="{ open: isReasoningOpen(m) }">
            <div class="reasoning-toggle" @click="toggleReasoning(m.id)">
              <span class="reasoning-arrow">›</span>
              <span class="reasoning-label">思考过程</span>
              <span v-if="isReasoningStreaming(m)" class="reasoning-thinking">思考中…</span>
            </div>
            <div v-show="isReasoningOpen(m)" class="reasoning-body">
              <AiMarkdown :content="m.reasoning" />
            </div>
          </div>
          <!-- AI 正文 markdown 渲染；用户消息保持纯文本 -->
          <AiMarkdown
            v-if="m.role === 'assistant' && m.content"
            :content="m.content"
            :terminal-ready="terminalReady"
            @terminal="onTerminalIntent"
          />
          <div v-else-if="m.content" class="bubble-body">{{ m.content }}</div>
          <div v-else-if="m.status === 'streaming'" class="typing">
            <span></span><span></span><span></span>
          </div>
          <!-- 工具提案记录条：
               - propose_command：命令提案卡片（授权/拒绝/执行状态徽标）
               - edit_file / write_file：文件编辑卡片（授权/执行状态徽标 + 变更预览）
               - 只读工具：轻量观察卡片（无授权交互，可折叠输出） -->
          <div
            v-for="p in m.toolProposals"
            :key="p.id"
            class="proposal"
            :class="[p.state, { 'readonly-tool': !['propose_command','edit_file','write_file'].includes(p.name) }]"
          >
            <!-- propose_command 命令提案卡片 -->
            <template v-if="p.name === 'propose_command'">
              <div class="proposal-head">
                <span class="proposal-tag">命令执行</span>
                <span class="proposal-state">
                  {{ p.state === 'pending'
                    ? '等待授权'
                    : p.state === 'accepted'
                      ? p.error ? '执行失败' : p.allowedSession ? '已执行 · 会话允许' : '已执行'
                      : p.state === 'rejected' ? '已拒绝' : '已失效' }}
                </span>
              </div>
              <div v-if="p.reason" class="proposal-reason">{{ p.reason }}</div>
              <div v-if="p.error" class="proposal-error">{{ p.error }}</div>
              <pre class="proposal-cmd">{{ proposalCommand(p) }}</pre>
            </template>
            <!-- edit_file / write_file 文件编辑卡片：两行布局，与只读工具卡片统一 -->
            <template v-else-if="p.name === 'edit_file' || p.name === 'write_file'">
              <div class="proposal-head">
                <span class="proposal-tag">{{ readonlyLabel(p.name) }}</span>
                <span class="proposal-state">
                  {{ p.state === 'pending'
                    ? '等待授权'
                    : p.state === 'accepted'
                      ? p.error ? '执行失败' : '已执行'
                      : p.state === 'rejected' ? '已拒绝' : '已失效' }}
                </span>
              </div>
              <div class="file-edit-row">
                <span class="proposal-args">{{ readonlyArgs(p) }}</span>
                <span class="line-diff">
                  <span class="diff-add">+{{ countLines(fileEditNew(p)) }}</span>
                  <template v-if="p.name === 'edit_file'">
                    <span class="diff-del">-{{ countLines(fileEditOld(p)) }}</span>
                  </template>
                </span>
              </div>
              <div v-if="p.error" class="proposal-error">{{ p.error }}</div>
              <details v-if="p.execOutput" class="readonly-output">
                <summary>输出（{{ p.execOutput.length }} 字符）</summary>
                <pre class="proposal-cmd">{{ p.execOutput }}</pre>
              </details>
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
          <!-- 消息底栏：streaming 期间整体隐藏；hover 淡入。
               回复组内仅最后一条显示（组 = 一次逻辑回复，重新生成语义即重跑整组） -->
          <div v-if="m.role === 'user'" v-show="!streaming" class="bubble-foot">
            <button class="foot-btn" title="编辑重发" @click="startEdit(m)">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                   stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M17 3a2.828 2.828 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5L17 3z" />
              </svg>
            </button>
            <button class="foot-btn" title="复制" @click="copyText(m.content)">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                   stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
                <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
              </svg>
            </button>
            <button class="foot-btn danger" title="删除" @click="deleteMessage(m)">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                   stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <polyline points="3 6 5 6 21 6" />
                <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
              </svg>
            </button>
          </div>
          <div v-if="m.role === 'assistant'" v-show="!streaming && isGroupTail(i)" class="bubble-foot">
            <button v-if="m.content" class="foot-btn" title="复制全文" @click="copyText(m.content)">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                   stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
                <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
              </svg>
            </button>
            <button
              v-if="m.status === 'done' || m.status === 'stopped' || m.status === 'error'"
              class="foot-btn"
              title="重新生成"
              @click="regenerate(m)"
            >
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                   stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <polyline points="23 4 23 10 17 10" />
                <polyline points="1 20 1 14 7 14" />
                <path d="M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15" />
              </svg>
            </button>
            <span v-if="m.status === 'stopped'" class="stopped-tag">已中止</span>
          </div>
        </div>
      </template>
    </div>

    <!-- 终端引用块（右键「添加到 AI 对话」产生） -->
    <div v-if="quote" class="quote-bar">
      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor"
           stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <polyline points="4 17 10 11 4 5" />
        <line x1="12" y1="19" x2="20" y2="19" />
      </svg>
      <span class="quote-label">终端引用 · {{ quote.lines }} 行</span>
      <span class="quote-preview">{{ quote.text.split('\n')[0] }}</span>
      <button class="quote-remove" title="移除引用" @click="quote = null">✕</button>
    </div>

    <!-- 直接执行确认弹窗：写入即键盘，用户核对命令原文（对齐 paste guard 先例） -->
    <div v-if="execConfirm" class="exec-mask" @click.self="cancelExec">
      <div class="exec-modal" role="alertdialog" aria-modal="true">
        <div class="exec-title">直接执行命令</div>
        <p class="exec-sub">
          命令将写入当前终端并回车执行{{ execConfirm.multiline ? `，共 ${execConfirm.lines} 行，将整块提交执行` : '' }}
        </p>
        <p v-if="execConfirm.risk" class="exec-warn">检测到高危命令（{{ execConfirm.risk }}），请核对后执行</p>
        <p v-else-if="execConfirm.multiline" class="exec-warn">多行命令将整块写入后一并执行，请逐行核对</p>
        <pre class="exec-preview">{{ execConfirm.code }}</pre>
        <div class="exec-foot">
          <button type="button" class="exec-btn" @click="cancelExec">取消（Esc）</button>
          <button type="button" class="exec-btn primary" @click="confirmExec">执行（↵）</button>
        </div>
      </div>
    </div>

    <!-- 授权弹窗（命令 / 文件编辑）：覆盖输入框上方，视线必达；
         数字键/Enter/Esc 快捷裁决，未裁决不能继续输入 -->
    <div v-if="authorizing" class="auth-wrap" role="alertdialog" aria-modal="true">
      <div class="auth-title">
        ⚠ {{ authorizing.proposal.name === 'propose_command' ? '是否允许执行这个命令？' : '是否允许修改这个文件？' }}
      </div>
      <!-- propose_command：显示命令 -->
      <template v-if="authorizing.proposal.name === 'propose_command'">
        <p class="auth-sub">
          AI 请求在当前 SSH 会话执行以下命令<template v-if="authorizing.proposal.reason">：{{ authorizing.proposal.reason }}</template>
        </p>
        <pre class="auth-cmd">{{ proposalCommand(authorizing.proposal) }}</pre>
        <p v-if="proposalCommand(authorizing.proposal).includes('\n')" class="auth-warn">
          多行命令将整块提交执行（共 {{ proposalCommand(authorizing.proposal).split('\n').length }} 行），请逐行核对
        </p>
      </template>
      <!-- edit_file / write_file：单行展示路径 + 行数变化 -->
      <template v-else-if="authorizing.proposal.name === 'edit_file' || authorizing.proposal.name === 'write_file'">
        <p class="auth-sub file-edit-line">
          <span class="file-edit-path">
            {{ authorizing.proposal.name === 'edit_file' ? '编辑' : '写入' }}
            <code>{{ readonlyArgs(authorizing.proposal) }}</code>
          </span>
          <span class="line-diff">
            <span class="diff-add">+{{ countLines(fileEditNew(authorizing.proposal)) }}</span>
            <template v-if="authorizing.proposal.name === 'edit_file'">
              <span class="diff-del">-{{ countLines(fileEditOld(authorizing.proposal)) }}</span>
            </template>
          </span>
        </p>
      </template>
      <div class="auth-choices">
        <button type="button" class="auth-choice" @click="resolveCurrent('once')">
          <span class="auth-num">1</span>仅本次执行<span class="auth-key">↵</span>
        </button>
        <button
          v-if="authorizing.proposal.name === 'propose_command'"
          type="button" class="auth-choice" @click="resolveCurrent('session')">
          <span class="auth-num">2</span>本会话允许此命令（后续相同命令自动执行）<span class="auth-key">2</span>
        </button>
        <button type="button" class="auth-choice" @click="resolveCurrent('reject')">
          <span class="auth-num">{{ authorizing.proposal.name === 'propose_command' ? '3' : '2' }}</span>拒绝执行<span class="auth-key">Esc</span>
        </button>
      </div>
      <div v-if="proposalQueue.length > 1" class="auth-queue">
        还有 {{ proposalQueue.length - 1 }} 个提案待确认
      </div>
    </div>

    <!-- 输入区 -->
    <div class="chat-input">
      <!-- agent 执行权限档（仅工作台 scope）：manual 弱化默认，risky/full 变色提醒放权态 -->
      <select
        v-if="appView === 'workbench'"
        v-model="execPermission"
        class="perm-select"
        :class="{ elevated: execPermission !== 'manual' }"
        title="命令执行授权档：每次确认 / 低危自动 / 全自动"
      >
        <option value="manual">手动确认</option>
        <option value="risky">低危自动</option>
        <option value="full">全自动</option>
      </select>
      <input
        ref="inputRef"
        v-model="draft"
        class="input"
        type="text"
        :disabled="!!configHint"
        :placeholder="configHint || (appView === 'home' ? '询问 AI…' : '询问 AI（携带当前会话上下文）…')"
        @keydown="onKeydown"
      />
      <button v-if="!streaming" class="send-btn" title="发送 (Enter)" @click="send()">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
             stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <line x1="22" y1="2" x2="11" y2="13" />
          <polygon points="22 2 15 22 11 13 2 9 22 2" />
        </svg>
      </button>
      <button v-else class="stop-btn" title="停止生成" @click="stop">
        <span class="stop-square"></span>
      </button>
    </div>
  </div>
</template>

<style scoped>
.ai-panel {
  position: relative; /* 授权弹窗（auth-wrap absolute）的定位基准 */
  display: flex;
  flex-direction: column;
  width: 100%;
  height: 100%;
  overflow: hidden;
}

/* 消息列表 */
.chat-scroll {
  flex: 1;
  overflow-y: auto;
  overflow-x: hidden;
  padding: 10px 12px;
  display: flex;
  flex-direction: column;
  gap: 14px;
  /* 专注模式下面板很宽：内容限宽居中，保持阅读舒适 */
  width: 100%;
  max-width: 760px;
  margin: 0 auto;
  box-sizing: border-box;
}
.chat-scroll::-webkit-scrollbar { width: 6px; }
.chat-scroll::-webkit-scrollbar-thumb {
  background: #1c2836;
  border-radius: 3px;
}

/* 空态 */
.empty {
  margin-top: auto;
  margin-bottom: auto;
  text-align: center;
}
.empty-title {
  font-size: 14px;
  font-weight: 600;
  color: var(--text);
  margin-bottom: 4px;
}
.empty-sub {
  font-size: 11.5px;
  color: var(--muted-2);
  margin-bottom: 14px;
}
.config-hint {
  font-size: 11.5px;
  color: #e5a13a;
  border: 1px solid rgba(229, 161, 58, 0.35);
  background: rgba(229, 161, 58, 0.08);
  border-radius: 8px;
  padding: 8px 12px;
  display: inline-block;
  max-width: 90%;
}
.hints {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
}
.hint-pill {
  border: 1px solid var(--border);
  border-radius: 999px;
  padding: 4px 12px;
  font-size: 11.5px;
  color: var(--muted);
  background: transparent;
  cursor: pointer;
  transition: background 0.15s, border-color 0.15s, color 0.15s;
}
.hint-pill:hover {
  background: var(--panel-2);
  border-color: var(--green);
  color: var(--text);
}

/* 消息：用户保留气泡，AI 无气泡平铺 */
.bubble {
  font-size: 12px;
  line-height: 1.55;
  white-space: pre-wrap;
  word-break: break-word;
}
.bubble.user {
  align-self: flex-end;
  max-width: 94%;
}
/* 气泡视觉仅作用于内容层，操作按钮位于气泡外部下方 */
.bubble.user .bubble-body {
  border-radius: 10px;
  padding: 8px 10px;
  background: var(--panel-2);
  color: var(--text);
}
/* user 消息右对齐，按钮行跟随气泡右缘 */
.bubble.user .bubble-foot {
  justify-content: flex-end;
}
.bubble.assistant {
  align-self: stretch;
  background: transparent;
  color: var(--text);
  padding-top: 2px;
}
.bubble.assistant.error .md {
  color: #e57a7a;
}
/* 回复组内的后续消息（工具循环中间轮）：无独立 logo 头，间距收紧为连续回复
   （flex gap 14px - 8px = 视觉 6px，与提案卡片的内部间距一致） */
.bubble.assistant.grouped {
  margin-top: -8px;
  padding-top: 0;
}
.bubble.assistant.grouped .msg-head {
  display: none;
}
/* ---------- 命令授权弹窗（输入框上方，占据布局不遮挡消息） ---------- */
.auth-wrap {
  flex-shrink: 0;
  /* 与输入框同宽居中，作为文档流区块推挤消息区而非覆盖 */
  width: 100%;
  max-width: 760px;
  box-sizing: border-box;
  margin: 0 auto 6px;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 10px;
  box-shadow: 0 -2px 12px rgba(0, 0, 0, 0.2);
  padding: 12px 14px;
}
.auth-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--text);
}
.auth-sub {
  margin: 6px 0 0;
  font-size: 12px;
  color: var(--muted);
}
.auth-cmd {
  margin: 8px 0;
  padding: 8px 10px;
  background: #0d1219;
  border: 1px solid var(--border);
  border-radius: 6px;
  font-family: var(--mono);
  font-size: 11.5px;
  line-height: 1.5;
  color: var(--text);
  white-space: pre-wrap;
  word-break: break-all;
  max-height: 160px;
  overflow: auto;
}
.auth-warn {
  margin: -4px 0 8px;
  font-size: 11px;
  color: var(--yellow);
}
.auth-choices {
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin-top: 8px;
}
.auth-choice {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 7px 9px;
  background: transparent;
  border: 1px solid transparent;
  border-radius: 6px;
  font-size: 12px;
  color: var(--text);
  text-align: left;
  cursor: pointer;
}
.auth-choice:hover {
  background: var(--hover);
  border-color: var(--border);
}
.auth-num {
  flex: none;
  width: 18px;
  height: 18px;
  display: grid;
  place-items: center;
  background: var(--hover);
  border-radius: 4px;
  font-size: 11px;
  color: var(--muted);
}
.auth-key {
  margin-left: auto;
  font-size: 10px;
  color: var(--muted-2);
}
.auth-queue {
  margin-top: 8px;
  font-size: 11px;
  color: var(--muted);
}

/* 工具提案卡片（M2.3） */
.proposal {
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 8px 10px;
  margin-top: 6px;
  max-width: 94%;
  background: var(--panel-2);
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.proposal.pending {
  border-color: rgba(229, 161, 58, 0.4);
}
.proposal.rejected,
.proposal.expired {
  opacity: 0.55;
}
.proposal-head {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 11px;
}
.proposal-tag {
  font-weight: 600;
  color: var(--text);
}
.proposal-state {
  margin-left: auto;
  color: var(--muted);
}
.proposal.pending .proposal-state {
  color: #e5a13a;
}
.proposal.rejected .proposal-state {
  color: #e57a7a;
}
.proposal-reason {
  font-size: 11.5px;
  color: var(--muted);
  line-height: 1.45;
}
.proposal-error {
  font-size: 11.5px;
  color: #e57a7a;
  line-height: 1.45;
}
.proposal-cmd {
  margin: 0;
  padding: 7px 9px;
  border-radius: 7px;
  background: #0d1219;
  border: 1px solid var(--border);
  font-family: var(--mono);
  font-size: 11.5px;
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-all;
  color: var(--text);
}

/* 只读工具观察卡片：蓝色系边框/标签，区别于命令提案的黄色 pending */
.proposal.readonly-tool {
  border-color: rgba(86, 156, 214, 0.4);
}
.proposal.readonly-tool .readonly-tag {
  color: #569cd6;
}
.proposal.readonly-tool .proposal-args {
  font-family: var(--mono);
  color: var(--muted);
  font-size: 11px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 220px;
}
.readonly-output {
  font-size: 11.5px;
}
.readonly-output summary {
  cursor: pointer;
  color: var(--muted);
  user-select: none;
}
.readonly-output summary:hover {
  color: var(--text);
}
.readonly-output[open] summary {
  margin-bottom: 6px;
}
/* 文件编辑行数差异展示：+新增行 / -删除行 */
.line-diff {
  display: inline-flex;
  gap: 8px;
  font-family: var(--mono);
  font-size: 11px;
  font-weight: 600;
  margin-left: auto;
  flex-shrink: 0;
}
.diff-add {
  color: var(--accent, #3ddc84);
}
.diff-del {
  color: var(--red, #f48771);
}
/* 卡片中文件编辑第二行：左侧路径，右侧行数变化 */
.file-edit-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 4px;
}
.file-edit-row .proposal-args {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-family: var(--mono);
  color: var(--muted);
  font-size: 11.5px;
}
/* 弹窗中文件编辑单行布局 */
.file-edit-line {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}
.file-edit-path {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* AI 消息头：logo + Agent */
.msg-head {
  display: flex;
  align-items: center;
  gap: 7px;
  margin-bottom: 6px;
}
.agent-logo {
  width: 18px;
  height: 18px;
  border-radius: 5px;
  background: var(--green);
  display: grid;
  place-items: center;
  flex-shrink: 0;
}
/* AppLogo 由父级 CSS 控制尺寸，currentColor 着色 */
.agent-logo svg {
  width: 11px;
  height: 11px;
  color: #0b1118;
}
.agent-name {
  font-size: 12px;
  font-weight: 600;
  color: var(--text);
}

/* 思考过程（流式时自动展开，content 到达后自动收起；› 指示展开方向） */
.reasoning {
  margin-bottom: 6px;
  font-size: 11px;
}
.reasoning-toggle {
  cursor: pointer;
  color: var(--muted-2);
  user-select: none;
  display: inline-flex;
  align-items: center;
  gap: 5px;
}
.reasoning-arrow {
  font-size: 12px;
  line-height: 1;
  transition: transform 0.15s;
}
.reasoning.open .reasoning-arrow {
  transform: rotate(90deg);
}
.reasoning.open .reasoning-toggle {
  margin-bottom: 4px;
}
.reasoning-thinking {
  color: var(--accent);
  animation: reasoning-pulse 1.2s ease-in-out infinite;
}
@keyframes reasoning-pulse {
  0%, 100% { opacity: 0.4; }
  50% { opacity: 1; }
}
.reasoning-body {
  color: var(--muted);
  border-left: 2px solid var(--border);
  padding-left: 8px;
}

/* 流式等待动画 */
.typing {
  display: inline-flex;
  gap: 4px;
  padding: 4px 2px;
}
.typing span {
  width: 5px;
  height: 5px;
  border-radius: 50%;
  background: var(--muted-2);
  animation: typing-blink 1.2s infinite ease-in-out;
}
.typing span:nth-child(2) { animation-delay: 0.2s; }
.typing span:nth-child(3) { animation-delay: 0.4s; }
@keyframes typing-blink {
  0%, 60%, 100% { opacity: 0.25; }
  30% { opacity: 1; }
}

.bubble-foot {
  margin-top: 6px;
  display: flex;
  align-items: center;
  gap: 6px;
  /* hover 淡入：默认隐藏，降低视觉噪音 */
  opacity: 0;
  transition: opacity 0.15s;
}
.bubble:hover .bubble-foot {
  opacity: 1;
}
.foot-btn {
  width: 22px;
  height: 22px;
  border-radius: 5px;
  display: grid;
  place-items: center;
  background: transparent;
  border: none;
  color: var(--muted-2);
  cursor: pointer;
  transition: background 0.15s, color 0.15s;
}
.foot-btn:hover {
  color: var(--text);
  background: var(--panel-2);
}
.foot-btn.danger:hover {
  color: #e57a7a;
  background: rgba(229, 90, 90, 0.12);
}
.stopped-tag {
  font-size: 10.5px;
  color: var(--muted-2);
  margin-left: 4px;
}

/* 终端引用块 */
.quote-bar {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 6px 10px 0;
  padding: 4px 8px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--panel-2);
  color: var(--muted-2);
  font-size: 11px;
}
.quote-bar svg {
  color: var(--green);
  flex-shrink: 0;
}
.quote-label {
  flex-shrink: 0;
  color: var(--green);
}
.quote-preview {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-family: var(--mono);
  font-size: 10.5px;
}
.quote-remove {
  flex-shrink: 0;
  border: none;
  background: transparent;
  color: var(--muted-2);
  cursor: pointer;
  font-size: 11px;
  padding: 1px 4px;
  border-radius: 4px;
}
.quote-remove:hover {
  color: var(--text);
  background: rgba(255, 255, 255, 0.06);
}

/* 输入区 */
.chat-input {
  flex-shrink: 0;
  padding: 8px 10px;
  border-top: 1px solid var(--border);
  display: flex;
  align-items: center;
  gap: 6px;
  background: var(--panel);
  /* 与消息列表同宽居中 */
  width: 100%;
  max-width: 760px;
  margin: 0 auto;
  box-sizing: border-box;
}
.input {
  flex: 1;
  min-width: 0;
  background: var(--panel-2);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 6px 10px;
  font-size: 12px;
  color: var(--text);
  outline: none;
  transition: border-color 0.15s;
}
/* agent 执行权限档下拉：manual 弱化融入输入区，放权档黄色警示 */
.perm-select {
  flex: none;
  background: var(--panel-2);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 6px 4px;
  font-size: 11px;
  color: var(--muted);
  outline: none;
  cursor: pointer;
}
.perm-select.elevated {
  color: var(--yellow);
  border-color: var(--yellow);
}
.input::placeholder {
  color: var(--muted-2);
}
.input:focus {
  border-color: var(--green);
}
.input:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}
.send-btn {
  width: 28px;
  height: 28px;
  border-radius: 8px;
  display: grid;
  place-items: center;
  background: var(--green);
  border: none;
  color: #0b1118;
  cursor: pointer;
  transition: opacity 0.15s;
  flex-shrink: 0;
}
.send-btn:hover {
  opacity: 0.85;
}
.send-btn:active {
  transform: scale(0.94);
}
.stop-btn {
  width: 28px;
  height: 28px;
  border-radius: 8px;
  display: grid;
  place-items: center;
  background: rgba(229, 90, 90, 0.15);
  border: 1px solid rgba(229, 90, 90, 0.5);
  cursor: pointer;
  flex-shrink: 0;
}
.stop-square {
  width: 10px;
  height: 10px;
  border-radius: 2px;
  background: #e57a7a;
}

/* 头部操作按钮（Teleport 到 RightPanel） */
.ai-action-btn {
  width: 26px;
  height: 26px;
  border-radius: 6px;
  display: grid;
  place-items: center;
  background: transparent;
  border: none;
  color: var(--muted-2);
  cursor: pointer;
  transition: background 0.15s, color 0.15s;
}
.ai-action-btn:hover:not(:disabled) {
  background: var(--panel-2);
  color: var(--text);
}
.ai-action-btn:disabled {
  opacity: 0.35;
  cursor: default;
}

/* 直接执行确认弹窗 */
.exec-mask {
  position: fixed;
  inset: 0;
  z-index: 1000;
  background: rgba(0, 0, 0, 0.45);
  display: grid;
  place-items: center;
}
.exec-modal {
  width: 400px;
  max-width: 90%;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 14px;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.4);
}
.exec-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--text);
}
.exec-sub {
  font-size: 11px;
  color: var(--muted-2);
  margin: 4px 0 0;
}
.exec-warn {
  font-size: 11px;
  color: #e5a13a;
  margin: 4px 0 0;
}
.exec-preview {
  max-height: 200px;
  overflow: auto;
  background: #0d1319;
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 8px 10px;
  font-family: var(--mono);
  font-size: 11px;
  line-height: 1.55;
  color: var(--text);
  white-space: pre-wrap;
  word-break: break-all;
  margin: 10px 0 12px;
}
.exec-foot {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}
.exec-btn {
  border: 1px solid var(--border);
  background: transparent;
  color: var(--muted);
  border-radius: 6px;
  padding: 4px 12px;
  font-size: 11.5px;
  cursor: pointer;
  transition: background 0.15s, color 0.15s;
}
.exec-btn:hover {
  color: var(--text);
  background: var(--panel-2);
}
.exec-btn.primary {
  background: var(--green);
  border-color: var(--green);
  color: #0b1118;
}
.exec-btn.primary:hover {
  opacity: 0.85;
  color: #0b1118;
}
</style>
