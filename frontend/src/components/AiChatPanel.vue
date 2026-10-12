<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { activeSession, activeHost, activeSessionId, appView, rightPanelTab, rightPanelVisible, sendInput, sendInputAndCapture } from '../stores/session'
import { savedSettings } from '../stores/settings'
import { aiQuote } from '../stores/aiQuote'
import { toast } from '../composables/useToast'
import { matchHighRisk } from '../lib/aiRisk'
import { writeText } from '@tauri-apps/plugin-clipboard-manager'
import { isTauri } from '../lib/tauri'
import AppLogo from './AppLogo.vue'
import AiMarkdown from './AiMarkdown.vue'
import {
  aiChatStream,
  aiClearChat,
  aiLoadChat,
  aiSaveChat,
  aiStopChat,
  aiSummarize,
  listenAiEvents,
  type AiChatMessage,
  type AiScope,
  type AiToolProposal,
  type AiWireMessage,
} from '../lib/ai'
import type { UnlistenFn } from '@tauri-apps/api/event'

/** 触发压缩的未摘要字符数（中英混合粗估 1 token ≈ 2 chars，约 4k token） */
const SUMMARY_TRIGGER_CHARS = 8000
/** 摘要后保留的最近消息条数（不参与压缩，保证当前话题连续性） */
const SUMMARY_KEEP_RECENT = 4

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
/** 待编辑重发的 user 消息 id：填回输入框后不立即截断，真正发送时才截断该条及其后 */
const editFromId = ref<string | null>(null)
/** 摘要请求进行中（防重入） */
let summarizing = false
const inputRef = ref<HTMLInputElement | null>(null)
const scrollEl = ref<HTMLElement | null>(null)
let unlisten: UnlistenFn | null = null
/** 跨 scope 切换的版本号：使旧 scope 的迟到事件失效 */
let loadSeq = 0
/** 工具循环轮次：用户输入归零，每次提案裁决续跑 +1，超上限自动续跑终止 */
const toolRound = ref(0)
/** 单次用户输入允许的最大工具循环轮次（覆盖「提案 → 执行 → 汇总」常规深度） */
const MAX_TOOL_ROUNDS = 3

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
  }).catch(e => console.error('[AI] 会话保存失败:', e))
}

async function loadRecord(s: AiScope) {
  if (!isTauri) {
    messages.value = []
    summary.value = ''
    summaryUpTo.value = ''
    return
  }
  try {
    const rec = await aiLoadChat(s)
    messages.value = (rec?.messages ?? []).map(m =>
      m.status === 'streaming' ? { ...m, status: 'stopped' as const } : m,
    )
    summary.value = rec?.summary ?? ''
    summaryUpTo.value = rec?.summaryUpTo ?? ''
  } catch (e) {
    console.error('[AI] 会话加载失败:', e)
    messages.value = []
    summary.value = ''
    summaryUpTo.value = ''
  }
  scrollToBottom()
}

watch(scope, s => {
  loadSeq++
  streaming.value = false
  editFromId.value = null
  execPermission.value = 'manual' // 权限档随会话回落，杜绝「上次开的 full 忘了关」
  resetProposalState()
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
          reason: parsed?.reason,
          state: 'pending' as const,
        }
      })
      // 自动执行分流（权限档 M2.3 ③）：豁免表命中（任何档位，用户显式豁免优先）
      // 或 risky 档低危或 full 档 → 免弹窗直接执行；否则照旧入队弹窗
      const allow = sessionAllowlist.get(scope.value)
      for (const p of last.toolProposals) {
        if (p.state !== 'pending') continue
        const cmd = parseProposalArgs(p.args)?.command
        if (!cmd) continue
        if (allow?.has(cmd)) {
          autoApply(p, 'session')
        } else if (
          execPermission.value === 'full' ||
          (execPermission.value === 'risky' && !matchHighRisk(cmd, savedSettings.aiRiskKeywords.split('\n')))
        ) {
          autoApply(p, 'once')
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

/* ---------- 发送 ---------- */

function buildWireMessages(): AiWireMessage[] {
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
  // 会话上下文 system 靠近对话（注意力靠后），置于摘要之后
  const ctx = buildSessionContext()
  if (ctx) wired.unshift(ctx)
  if (summary.value) {
    wired.unshift({
      role: 'system',
      content: `以下是此前对话的摘要：\n${summary.value}\n\n请结合摘要与近期消息继续对话。`,
    })
  }
  return wired
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

/** 提案裁决结果 → tool 结果消息内容（模型据此了解裁决与执行情况） */
function toolResultOf(p: AiToolProposal): string {
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
 *  （全局对话无终端可写）；请求失败写入最后一条 assistant 消息 */
function runAiStream(seq: number) {
  aiChatStream(scope.value, buildWireMessages(), {
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
      if (p.state === 'pending' && !autoRunning.has(p)) {
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

/** 应用裁决：写状态并按需执行命令（once/session 均执行，session 另记入豁免表）。
 *  自动豁免路径在 onToolCalls 中直接调用（无弹窗） */
async function applyProposal(p: AiToolProposal, choice: 'once' | 'session' | 'reject') {
  const parsed = parseProposalArgs(p.args)
  if (choice === 'reject' || !parsed?.command) {
    // 拒绝 / 参数无法解析（不可执行）：按拒绝处理，模型得知后自行调整
    p.state = 'rejected'
  } else {
    const multiline = parsed.command.includes('\n')
    if (multiline && !savedSettings.aiMultilineExec) {
      p.state = 'accepted'
      p.error = '多行命令直接执行未开启（设置 → AI → 终端执行）'
    } else {
      // 弹窗即确认 UI（用户选择 = 人工批准），不再叠加 execConfirm 弹窗
      const { error, output } = await writeExec(parsed.command, multiline)
      p.state = 'accepted'
      p.allowedSession = choice === 'session'
      if (error) p.error = error
      if (output) p.execOutput = output  // M3.1：回传输出暂存，tool 结果组装时读取
      if (choice === 'session' && !error) {
        const set = sessionAllowlist.get(scope.value) ?? new Set<string>()
        set.add(parsed.command)
        sessionAllowlist.set(scope.value, set)
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

/** 授权弹窗键盘裁决：数字 1/2/3、Enter=仅本次、Esc=拒绝（capture 抢先，与 exec 弹窗互斥） */
function onAuthKey(e: KeyboardEvent) {
  if (!authorizing.value || execConfirm.value) return
  const map: Record<string, 'once' | 'session' | 'reject'> = {
    '1': 'once',
    '2': 'session',
    '3': 'reject',
    Enter: 'once',
    Escape: 'reject',
  }
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
          <!-- 思考过程（默认收起） -->
          <details v-if="m.reasoning" class="reasoning">
            <summary>思考过程</summary>
            <div class="reasoning-body">{{ m.reasoning }}</div>
          </details>
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
          <!-- 工具提案记录条（混合形态：决策在底部授权弹窗，流内仅留痕）：
               非交互，展示终态 + 来源 + 命令原文，回溯可审计 -->
          <div
            v-for="p in m.toolProposals"
            :key="p.id"
            class="proposal"
            :class="p.state"
          >
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

    <!-- 命令授权弹窗（混合形态·决策端）：覆盖输入框上方，视线必达；
         数字键/Enter/Esc 快捷裁决，未裁决不能继续输入 -->
    <div v-if="authorizing" class="auth-wrap" role="alertdialog" aria-modal="true">
      <div class="auth-title">⚠ 是否允许执行这个命令？</div>
      <p class="auth-sub">
        AI 请求在当前 SSH 会话执行以下命令<template v-if="authorizing.proposal.reason">：{{ authorizing.proposal.reason }}</template>
      </p>
      <pre class="auth-cmd">{{ proposalCommand(authorizing.proposal) }}</pre>
      <p v-if="proposalCommand(authorizing.proposal).includes('\n')" class="auth-warn">
        多行命令将整块提交执行（共 {{ proposalCommand(authorizing.proposal).split('\n').length }} 行），请逐行核对
      </p>
      <div class="auth-choices">
        <button type="button" class="auth-choice" @click="resolveCurrent('once')">
          <span class="auth-num">1</span>仅本次执行<span class="auth-key">↵</span>
        </button>
        <button type="button" class="auth-choice" @click="resolveCurrent('session')">
          <span class="auth-num">2</span>本会话允许此命令（后续相同命令自动执行）<span class="auth-key">2</span>
        </button>
        <button type="button" class="auth-choice" @click="resolveCurrent('reject')">
          <span class="auth-num">3</span>拒绝执行<span class="auth-key">Esc</span>
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

/* 思考过程（默认收起，› 指示展开方向） */
.reasoning {
  margin-bottom: 6px;
  font-size: 11px;
}
.reasoning summary {
  cursor: pointer;
  color: var(--muted-2);
  user-select: none;
  list-style: none;
  display: inline-flex;
  align-items: center;
  gap: 5px;
}
.reasoning summary::-webkit-details-marker {
  display: none;
}
.reasoning summary::after {
  content: '›';
  font-size: 12px;
  line-height: 1;
  transition: transform 0.15s;
}
.reasoning[open] summary::after {
  transform: rotate(90deg);
}
.reasoning[open] summary {
  margin-bottom: 4px;
}
.reasoning-body {
  color: var(--muted);
  border-left: 2px solid var(--border);
  padding-left: 8px;
  white-space: pre-wrap;
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
