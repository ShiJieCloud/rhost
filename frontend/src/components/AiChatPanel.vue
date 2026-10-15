<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { activeSession, activeHost, activeSessionId, appView, rightPanelTab, rightPanelVisible, sendInput, sendInputAndCapture, sessions } from '../stores/session'
import { savedSettings } from '../stores/settings'
import { aiQuote } from '../stores/aiQuote'
import { toast } from '../composables/useToast'
import { matchHighRisk } from '../lib/aiRisk'
import { writeText } from '@tauri-apps/plugin-clipboard-manager'
import { isTauri } from '../lib/tauri'
import { Channel, invoke } from '@tauri-apps/api/core'
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
  type AiAttachment,
  type AiChatMessage,
  type AiContentPart,
  type AiImageAttachment,
  type AiMemoryItem,
  type AiPlan,
  type AiPromptAsset,
  type AiPromptAssets,
  type AiScope,
  type AiTextAttachment,
  type AiToolExecResult,
  type AiToolProposal,
  type AiWireMessage,
} from '../lib/ai'
import {
  report,
  AI_PLAN_PROPOSED,
  AI_PLAN_INVALID,
  AI_PLAN_CONFIRMED,
  AI_PLAN_REFUSED,
  AI_PLAN_FORCED,
  AI_PLAN_FORCE_FAILED,
  AI_PLAN_CONFLICT_DROPPED,
  AI_PLAN_REPROPOSAL_BLOCKED,
  AI_PLAN_STEP_FAILED,
  AI_PLAN_REPLANNED,
  AI_PLAN_ABORTED,
  AI_PLAN_FINISHED,
  AI_PLAN_CONCLUDED,
} from '../stores/applog'
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
/** 待发送的附件（内存态，多模态输入：图片 + 文本/代码文件）：发送后随消息落盘并清空 */
const attachments = ref<AiAttachment[]>([])
/** 隐藏的文件选择 input（点击 + 触发） */
const fileInputEl = ref<HTMLInputElement | null>(null)
/** 输入区聚焦/拖拽悬停高亮（仅视觉反馈） */
const composerFocused = ref(false)
const dragActive = ref(false)
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
/** 执行过程展开/收起手动状态：回复组首消息 id → true(展开)/false(收起)；未在 map 中则跟随自动行为 */
const traceManual = ref<Map<string, boolean>>(new Map())
/** 每轮思考过程展开/收起手动状态：消息 id → true/false；未在 map 中则跟随自动行为
 *  （自动：reasoning 流式中且该轮正文未到达 → 展开+脉冲；正文到达 → 收起） */
const reasoningManual = ref<Map<string, boolean>>(new Map())
const inputRef = ref<HTMLTextAreaElement | null>(null)
const scrollEl = ref<HTMLElement | null>(null)
let unlisten: UnlistenFn | null = null
/** 跨 scope 切换的版本号：使旧 scope 的迟到事件失效 */
let loadSeq = 0
/** 工具循环轮次：用户输入归零，每次提案裁决续跑 +1，超上限自动续跑终止 */
const toolRound = ref(0)
/** 单次用户输入允许的最大工具循环轮次。
 *  ReAct 场景：3~4 次只读探索 + 2~3 次命令执行 = 6 轮上限 */
const MAX_TOOL_ROUNDS = 6

/* ---------- Plan-and-Act 规划模式（M4） ---------- */

/** 当前正在执行的计划步：所属计划消息 + 步下标。
 *  续跑链期间保持，直到该步落定（完成/失败）才清除 */
const planRunRef = ref<{ msgId: string; stepIndex: number } | null>(null)
/** finishPlan 总结轮所属计划消息 id（该轮注入全部步骤摘要且不带工具） */
const planSummaryRef = ref<string | null>(null)
/** 重规划上下文（重规划轮 propose_plan 的落点）：目标计划消息 + 失败步下标 + 进展描述。
 *  非 null 表示本轮的 propose_plan 用于替换既有计划的失败步及后续，而非新建卡片 */
const replanCtx = ref<{ msgId: string; failedIndex: number; progress: string } | null>(null)
/** 本回合禁用计划提案（用户已拒绝本次规划，§6.8 层 2）：随 ai_chat_stream 入参下发，
 *  后端据此物理移除 propose_plan；回合结束（流结束且不再续跑）自动复位 */
const planBannedInRound = ref(false)
/** 强制规划入口来源（埋点 via）：'menu'（+ 菜单「规划模式」）/'prefix'（手写 /plan） */
let forcedVia: 'menu' | 'prefix' = 'prefix'
/** 强制规划轮进行中（planForced=true 的请求）：用于「未产出合法计划」兜底提示与埋点 */
let forcedRound = false
/** 强制规划轮是否已产出 propose_plan（未见则流结束时记 ai.plan.force_failed） */
let forcedProposalSeen = false
/** 斜杠命令：消息以 "/plan " 开头强制走规划路径（等价 + 菜单「规划模式」项） */
const PLAN_CMD_RE = /^\/plan\s+/

/** 写操作类工具（与 propose_plan 同轮并存构成冲突；授权弹窗入队白名单同源） */
const WRITE_TOOLS = ['propose_command', 'edit_file', 'write_file', 'sftp_upload_file', 'sftp_download_file']
/** 计划步骤数上下限与标题长度（与后端 propose_plan JSON Schema 同口径） */
const PLAN_MIN_STEPS = 3
const PLAN_MAX_STEPS = 8
const PLAN_TITLE_MAX_CHARS = 50

/* ---------- 任务耗时（回复组维度） ---------- */

/** 每秒自增的节拍：让「任务耗时」在流式期间实时刷新（落定后改用持久化的 elapsedMs） */
const nowTick = ref(0)
let tickTimer: number | undefined
watch(
  () => streaming.value,
  busy => {
    if (tickTimer !== undefined) {
      window.clearInterval(tickTimer)
      tickTimer = undefined
    }
    if (!busy) return
    tickTimer = window.setInterval(() => {
      nowTick.value++
    }, 1000)
  },
)
onUnmounted(() => {
  if (tickTimer !== undefined) window.clearInterval(tickTimer)
})

/** 毫秒 → 耗时文案：省略高位零段、最少保留秒，如 `21m 27s` / `1h 2m 3s` */
function fmtDuration(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000))
  const h = Math.floor(total / 3600)
  const m = Math.floor((total % 3600) / 60)
  const s = total % 60
  const parts: string[] = []
  if (h) parts.push(`${h}h`)
  if (h || m) parts.push(`${m}m`)
  parts.push(`${s}s`)
  return parts.join(' ')
}

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

/* 权限档自定义下拉：原生 select 无法渲染图标与两行描述，改为按钮 + 上弹菜单 */
type PermLevel = 'manual' | 'risky' | 'full'
/** 权限档元数据：manual=盾（全防护）、risky=盾+勾（低危放行）、full=盾+斜杠（撤销防护） */
const PERM_LEVELS: { value: PermLevel; label: string; desc: string }[] = [
  { value: 'manual', label: '手动审批', desc: '所有命令执行前都需人工确认' },
  { value: 'risky', label: '低危自动', desc: '低危命令自动执行，高危仍需确认' },
  { value: 'full', label: '完全访问', desc: '全部命令自动执行，不再弹窗确认' },
]
const permOpen = ref(false)
const permWrapRef = ref<HTMLElement | null>(null)
const currentPermLabel = computed(
  () => PERM_LEVELS.find(l => l.value === execPermission.value)?.label ?? '',
)

function pickPerm(v: PermLevel) {
  execPermission.value = v
  permOpen.value = false
}
function onPermDocMouseDown(e: MouseEvent) {
  const el = permWrapRef.value
  if (el && !el.contains(e.target as Node)) permOpen.value = false
}
function onPermKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') permOpen.value = false
}
watch(permOpen, (v) => {
  if (v) {
    document.addEventListener('mousedown', onPermDocMouseDown, true)
    document.addEventListener('keydown', onPermKeydown, true)
  } else {
    document.removeEventListener('mousedown', onPermDocMouseDown, true)
    document.removeEventListener('keydown', onPermKeydown, true)
  }
})

/* ---------- 输入区「+」下拉菜单：上传附件 / 规划模式 ---------- */
const addOpen = ref(false)
const addWrapRef = ref<HTMLElement | null>(null)
function onAddDocMouseDown(e: MouseEvent) {
  const el = addWrapRef.value
  if (el && !el.contains(e.target as Node)) addOpen.value = false
}
function onAddKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') addOpen.value = false
}
watch(addOpen, (v) => {
  if (v) {
    document.addEventListener('mousedown', onAddDocMouseDown, true)
    document.addEventListener('keydown', onAddKeydown, true)
  } else {
    document.removeEventListener('mousedown', onAddDocMouseDown, true)
    document.removeEventListener('keydown', onAddKeydown, true)
  }
})
onUnmounted(() => {
  document.removeEventListener('mousedown', onPermDocMouseDown, true)
  document.removeEventListener('keydown', onPermKeydown, true)
  document.removeEventListener('mousedown', onAddDocMouseDown, true)
  document.removeEventListener('keydown', onAddKeydown, true)
})

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

/* ---------- 回复组（一次用户输入触发的连续 assistant 消息） ---------- */

/** 执行过程区轮次：一条 assistant 消息 = 一轮（ReAct 轮 / 计划步骤轮 / 续跑轮 / 总结轮）。
 *  轮次内容固定三段：思考过程（独立折叠）→ 规划轨迹/正文 → 工具卡片；
 *  结果确认前（promoted=false）本轮正文渲染在轮次槽位内，确认后仅末轮正文晋升至结果区 */
interface AiRound {
  key: string
  /** 消息代理引用（思考折叠状态按消息 id 存取） */
  m: AiChatMessage
  /** 本轮正文已晋升至结果区（不再在轮次内渲染，避免同文两处同显） */
  promoted: boolean
  /** Plan 执行步下标（planner/总结轮无此字段；旧数据无 → 无步骤分隔条） */
  stepIndex?: number
  /** 是否该步骤的首个轮次（渲染步骤分隔条） */
  stepStart: boolean
}

/** 组末条下标：从 start 起连续 assistant 消息的最后一条 */
function groupEnd(start: number): number {
  let i = start
  while (i + 1 < messages.value.length && messages.value[i + 1].role === 'assistant') i++
  return i
}

/** 组首条下标：从 end 起向前找连续 assistant 消息的第一条 */
function groupStartOf(end: number): number {
  let s = end
  while (s - 1 >= 0 && messages.value[s - 1].role === 'assistant') s--
  return s
}

/** 是否回复组首条（assistant 且前一条非 assistant）：仅组首渲染整组 */
function isGroupLead(i: number): boolean {
  const m = messages.value[i]
  if (!m || m.role !== 'assistant') return false
  const prev = messages.value[i - 1]
  return !prev || prev.role !== 'assistant'
}

/** 组末条状态：驱动 assistant 气泡（error/streaming）与打字动画 */
function groupStatus(start: number): AiChatMessage['status'] {
  return messages.value[groupEnd(start)]?.status ?? 'done'
}

/** 组任务是否活跃：任一条流式中、存在未落定（pending）的工具提案、或组内计划执行中。
 *  覆盖续跑轮间 / 授权等待 / 工具执行 / 计划步间（含失败步待决策）等「末条非 streaming」
 *  的任务空窗，避免执行过程区在多段流串行期间反复折叠展开（闪烁） */
function groupActive(start: number): boolean {
  const end = groupEnd(start)
  for (let i = start; i <= end; i++) {
    const m = messages.value[i]
    if (m.status === 'streaming') return true
    if (m.toolProposals?.some(p => p.state === 'pending')) return true
  }
  return groupPlan(start)?.state === 'executing'
}

/** 结果是否已确认：组落定（非活跃）即确认；Plan 总结轮流式中首轮 token 即确认
 *  （planSummaryRef 指向该组组首，总结正文直接流入结果区，无晋升搬迁）。
 *  未确认时结果区恒空，组内一切消息（含流式中的当前轮）只渲染在执行过程区 */
function groupResultConfirmed(start: number): boolean {
  if (!groupActive(start)) return true
  return !!planSummaryRef.value && planSummaryRef.value === messages.value[start]?.id
}

/** 最终结果：组末条正文（仅结果确认后可见） */
function groupResult(start: number): string {
  return messages.value[groupEnd(start)]?.content.trim() ?? ''
}

/** 组内挂载的计划（取最新一条） */
function groupPlan(start: number): AiPlan | undefined {
  for (let i = groupEnd(start); i >= start; i--) {
    const p = messages.value[i].plan
    if (p) return p
  }
  return undefined
}

/** 结果区文本：仅结果确认后产生——取 AI 最终回复（末轮正文）；无则空（结果区恒空规则）。
 *  计划各步状态与结果只在规划面板呈现，不在消息回复区回退渲染步骤列表 */
function groupResultText(start: number): string {
  if (!groupResultConfirmed(start)) return ''
  return groupResult(start)
}

/** 本轮任务耗时（ms）：运行中实时计算（依赖 nowTick 节拍），落定后用持久化 elapsedMs */
function groupElapsedMs(start: number): number {
  void nowTick.value // 建立响应式依赖：流式期间每秒重算
  const last = messages.value[groupEnd(start)]
  const user = messages.value[start - 1]
  if (!last) return 0
  if (last.status === 'streaming') return Date.now() - (user?.createdAt ?? last.createdAt)
  return last.elapsedMs ?? (user ? last.createdAt - user.createdAt : 0)
}

/** 执行过程区轮次：按消息顺序逐轮推导（含流式中的当前轮）。
 *  末轮正文在结果确认后标记 promoted——由结果区渲染，轮次内不再重复显示 */
function groupRounds(start: number): AiRound[] {
  const end = groupEnd(start)
  const confirmed = groupResultConfirmed(start)
  const rounds: AiRound[] = []
  let prevStep: number | undefined
  for (let i = start; i <= end; i++) {
    const m = messages.value[i]
    rounds.push({
      key: m.id,
      m,
      promoted: i === end && confirmed && !!m.content.trim(),
      stepIndex: m.stepIndex,
      stepStart: m.stepIndex !== undefined && m.stepIndex !== prevStep,
    })
    prevStep = m.stepIndex
  }
  return rounds
}

/** 执行过程区是否有可见内容（决定任务耗时行是否可点击展开） */
function hasTrace(start: number): boolean {
  return groupRounds(start).some(
    r =>
      !!r.m.reasoning?.trim() ||
      !!r.m.trace ||
      !!r.m.toolProposals?.length ||
      (!!r.m.content.trim() && !r.promoted),
  )
}

/** 执行过程是否展开：手动优先；自动 = 任务活跃且结果区尚未出现内容。
 *  Plan 总结轮 content 开始到达 → 结果区出现 → 收起（提前于任务落定）；
 *  其余一切落定场景（含中止/失败）在活跃结束瞬间收起 */
function isTraceOpen(start: number): boolean {
  const lead = messages.value[start]
  if (!lead) return false
  const manual = traceManual.value.get(lead.id)
  if (manual !== undefined) return manual
  return groupActive(start) && !groupResultText(start)
}

/** 用户手动切换执行过程展开/收起 */
function toggleTrace(start: number) {
  const lead = messages.value[start]
  if (!lead) return
  traceManual.value.set(lead.id, !isTraceOpen(start))
  resampleAnchors() // 折叠区高度变化 → 大纲锚点重采样
}

/** 该轮思考是否处于「思考中」自动态：reasoning 流式中且该轮正文未到达 */
function reasoningThinking(m: AiChatMessage): boolean {
  return !!(m.status === 'streaming' && m.reasoning?.trim() && !m.content.trim())
}

/** 每轮思考过程是否展开：手动优先；自动 = 思考中（展开+脉冲），正文到达即收起 */
function isReasoningOpen(m: AiChatMessage): boolean {
  const manual = reasoningManual.value.get(m.id)
  if (manual !== undefined) return manual
  return reasoningThinking(m)
}

/** 用户手动切换某轮思考过程展开/收起（进入手动模式，不再受自动逻辑覆盖） */
function toggleReasoning(m: AiChatMessage) {
  reasoningManual.value.set(m.id, !isReasoningOpen(m))
  resampleAnchors() // 思考条目高度变化 → 大纲锚点重采样
}

/* ---------- 持久化 ---------- */

/** 会话标题：取首条 user 消息文本前 20 字符；纯附件消息（无文本）取「图片」或文件名 */
function chatTitle(msgs: AiChatMessage[]): string | undefined {
  const first = msgs.find(m => m.role === 'user')
  if (!first) return undefined
  return (
    first.content.slice(0, 20) ||
    (first.images?.length
      ? '图片'
      : first.files?.length
        ? first.files[0].name.slice(0, 20)
        : undefined)
  )
}

/** 落盘：streaming 态转为 stopped（应用崩溃/重载后的中间态收敛） */
function persist() {
  if (!isTauri) return
  const cleaned = messages.value.map(m =>
    m.status === 'streaming' ? { ...m, status: 'stopped' as const } : m,
  )
  const title = chatTitle(cleaned)
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
    traceManual.value.clear()
    reasoningManual.value.clear()
    return
  }
  try {
    const rec = await aiLoadChat(s)
    messages.value = (rec?.messages ?? []).map(m =>
      m.status === 'streaming' ? { ...m, status: 'stopped' as const } : m,
    )
    // 计划崩溃收敛：executing 计划的 running 步中断后收敛为 failed（记「应用中断」），
    // plan 保持 executing 等用户决策（重试/跳过/终止）——与 streaming→stopped 同语义
    for (const m of messages.value) {
      if (m.plan?.state === 'executing') {
        m.plan.steps.forEach(st => {
          if (st.state === 'running') {
            st.state = 'failed'
            st.result = '应用中断'
          }
        })
      }
    }
    summary.value = rec?.summary ?? ''
    summaryUpTo.value = rec?.summaryUpTo ?? ''
    extractedUpTo.value = rec?.extractedUpTo ?? ''
  } catch (e) {
    console.error('[AI] 会话加载失败:', e)
    messages.value = []
    summary.value = ''
    summaryUpTo.value = ''
    extractedUpTo.value = ''
    traceManual.value.clear()
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
  // 旧 scope 执行中计划收敛：executing → cancelled、running 步 → failed，
  // 在 loadRecord 替换消息前落盘到旧 scope（persist 内部读 scope.value 已是新值，此处手动存）
  if (oldS && isTauri) {
    let dirty = false
    for (const m of messages.value) {
      if (m.plan?.state === 'executing') {
        m.plan.state = 'cancelled'
        m.plan.steps.forEach(st => {
          if (st.state === 'running') {
            st.state = 'failed'
            st.result = '会话切换'
          }
        })
        dirty = true
      }
    }
    if (dirty) {
      const cleaned = messages.value.map(m =>
        m.status === 'streaming' ? { ...m, status: 'stopped' as const } : m,
      )
      void aiSaveChat(oldS, {
        version: 1,
        scope: oldS,
        title: chatTitle(cleaned),
        updatedAt: Date.now(),
        messages: cleaned,
        summary: summary.value || undefined,
        summaryUpTo: summaryUpTo.value || undefined,
        extractedUpTo: extractedUpTo.value || undefined,
      }).catch(() => {}) // 失败静默：收敛是尽力而为
    }
  }
  // 计划相关内存态随 scope 清理
  planRunRef.value = null
  planSummaryRef.value = null
  replanCtx.value = null
  planBannedInRound.value = false
  // 输入区上下文 chip 随 scope 清理（终端引用 + 图片附件）
  quote.value = null
  attachments.value = []
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
      followScroll()
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
      stampElapsed()
      // 计划步执行中请求失败 → 该步转 failed，等用户决策
      const run = planRunRef.value
      if (run) {
        planRunRef.value = null
        const planMsg = messages.value.find(m => m.id === run.msgId)
        if (planMsg?.plan) failPlanStep(planMsg, run.stepIndex, `请求失败：${e.message}`)
      }
      planSummaryRef.value = null
      persist()
      // 错误落定产生部分结果（非计划场景）→ 晋升滚动（守卫内）
      if (groupResultText(groupStartOf(messages.value.length - 1))) scrollToResult()
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
      // - propose_command / edit_file / write_file / sftp_upload_file / sftp_download_file：
      //   走授权路径（权限档决定自动/弹窗）
      // - 只读工具（read_file / list_directory / get_system_info）：自动执行，无需授权
      const allow = sessionAllowlist.get(scope.value)
      // 冲突检测：同一轮出现「判定类提案」（propose_plan / conclude_plan）与写操作类工具
      // → 模型幻觉（设计 §6.2 互斥兜底）：判定让位于实际动作，避免「先规划/先收尾」被
      // 同时发生的执行动作覆盖。与只读工具并存不算冲突（只读不改变服务器状态）。
      const writeInRound = last.toolProposals.filter(p => WRITE_TOOLS.includes(p.name))
      const planProposal = last.toolProposals.find(p => p.name === 'propose_plan')
      const concludeProposal = last.toolProposals.find(p => p.name === 'conclude_plan')
      const planConflict = !!planProposal && writeInRound.length > 0
      if (planConflict) {
        report('warn', AI_PLAN_CONFLICT_DROPPED, '计划提案与执行动作同轮，计划作废', {
          other_tools: writeInRound.map(p => p.name),
        })
      }
      // 提前收敛提案：同轮出现写操作或计划提案时作废（计划优先级更高，判定不能叠加）
      const concludeConflict =
        !!concludeProposal && (writeInRound.length > 0 || !!planProposal)
      for (const p of last.toolProposals) {
        if (p.state !== 'pending') continue
        if (p.name === 'propose_plan') {
          forcedProposalSeen = true
          // 层 3 兜底：禁令生效期仍收到提案（层 2 传参遗漏等缺陷）→ 直接作废不建卡片
          if (planBannedInRound.value) {
            report('warn', AI_PLAN_REPROPOSAL_BLOCKED, '禁令生效期仍收到计划提案，已作废')
            rejectPlanProposal(p, '本轮已禁用计划提案')
          } else if (planConflict) {
            rejectPlanProposal(p, '本轮同时出现执行动作，规划已作废')
          } else {
            applyPlanProposal(last, p)
          }
          continue
        }
        if (p.name === 'conclude_plan') {
          const parsed = parseProposalArgs(p.args)
          if (concludeConflict) {
            p.state = 'rejected'
            p.error = '本轮同时出现执行动作或计划提案，收敛已作废'
            continue
          }
          // 标记当前执行步：completePlanStep 据此把剩余 pending 步转 skipped 并直接进总结轮
          const run = planRunRef.value
          const planMsg = run ? messages.value.find(m => m.id === run.msgId) : undefined
          const step = run ? planMsg?.plan?.steps[run.stepIndex] : undefined
          const outcome = parsed?.outcome === 'blocked' ? 'blocked' : 'resolved'
          if (step) {
            step.concluded = true
            step.concludeOutcome = outcome
          }
          p.state = 'accepted'
          p.reason = parsed?.reason
          continue
        }
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

/** 写入本轮回复组总耗时（组末条落定时刻 - 组前 user 消息发送时刻）。
 *  每次流结束时覆盖组末条；工具循环/计划逐步续跑会追加新末条，最终末条持有整组耗时 */
function stampElapsed() {
  const msgs = messages.value
  const end = msgs.length - 1
  if (end < 1 || msgs[end].role !== 'assistant') return
  let start = end
  while (start - 1 >= 0 && msgs[start - 1].role === 'assistant') start--
  const user = msgs[start - 1]
  if (!user || user.role !== 'user') return
  msgs[end].elapsedMs = Date.now() - user.createdAt
}

function finishStreaming(status: 'done' | 'stopped') {
  const start = groupStartOf(messages.value.length - 1)
  const before = groupResultText(start)
  const last = messages.value[messages.value.length - 1]
  if (last && last.role === 'assistant') last.status = status
  streaming.value = false
  stampElapsed()
  persist()
  scrollToBottom()
  // 提案调度：pending 入队弹窗；无 pending 且本轮已裁决完 → 自动续跑
  pumpProposals()
  // 计划步落定判定（pumpProposals 之后：有提案则续跑链推进，无提案才可能落定）
  maybeAdvancePlan(status)
  // finishPlan 总结轮无工具无续跑，流结束即清除标记。
  // maybeAdvancePlan 刚启动总结轮时（finishPlan 已置标记并发起请求）streaming=true，
  // 不得在同一次调用里清除——否则 runAiStream 异步续上后读到 null，
  // 总结轮会带着工具与空计划上下文发出（表象：总结轮继续执行工具、重复干活）
  if (planSummaryRef.value && !streaming.value) planSummaryRef.value = null
  // 强制规划轮收尾：未见提案（tool_choice 未生效 / 供应商不支持 / 模型未按约束调用）→
  // 提示用户在无计划路径下重试，同时记 ai.plan.force_failed 供判据调优
  if (forcedRound && !forcedProposalSeen) {
    report('warn', AI_PLAN_FORCE_FAILED, '强制规划轮未产出计划提案', { reason: 'no_tool_call' })
    toast('未能生成计划，请重试或用普通方式描述任务', 'warn', 3200)
  }
  // 回合级复位：禁令与强制标记仅在「本回合不再续跑」时解除
  // （用户拒绝计划后的续跑链延续同一回合禁令，continueToolRound 使 streaming 重为 true）
  if (!streaming.value) {
    planBannedInRound.value = false
    forcedRound = false
    forcedProposalSeen = false
  }
  // 晋升滚动：落定产生了此前不存在的最终结果（ReAct/单轮确认晋升、中止部分结果）→
  // 守卫内平滑滚动补偿上移；工具轮末尾续跑链已同步推进（结果区回空）不会误触
  if (!before && groupResultText(start)) scrollToResult()
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

/** 把文本/代码附件内联为 `<file name="…">…</file>` 块（置于用户文本之后）。
 *  文件名去除引号/尖括号，避免破坏块标记；无文件时原样返回 */
function withFileBlocks(content: string, files?: AiTextAttachment[]): string {
  if (!files?.length) return content
  const blocks = files
    .map(f => `<file name="${f.name.replace(/["<>]/g, '')}">\n${f.text}\n</file>`)
    .join('\n\n')
  return content.trim() ? `${content}\n\n${blocks}` : blocks
}

/** 组装 user 消息内容：文本/代码附件内联为 `<file>` 块；含图片时返回 content parts
 *  （文本在前、图片在后），否则返回字符串。对话 wire 装配与规划生成共用同一口径 */
function buildUserContent(
  text: string,
  images?: AiImageAttachment[],
  files?: AiTextAttachment[],
): string | AiContentPart[] {
  const body = withFileBlocks(text, files)
  if (!images?.length) return body
  const parts: AiContentPart[] = []
  if (body.trim()) parts.push({ type: 'text', text: body })
  for (const img of images) parts.push({ type: 'image_url', image_url: { url: img.dataUrl } })
  return parts
}

function buildWireMessages(assets?: AiPromptAssets, memories?: AiMemoryItem[]): AiWireMessage[] {
  // 仅把已成形的消息发给模型；空回复/错误回复不进上下文（带提案的空正文 assistant 除外）
  const usable = messages.value.filter(
    m =>
      m.status !== 'streaming' &&
      (m.content.trim() || m.toolProposals?.length || m.images?.length || m.files?.length),
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
    // 含附件 user 消息：文本文件内联为 <file> 块；有图片时装配为 content parts
    // （文本在前、图片在后；纯文本文件消息仍保持字符串 content，不升级为数组）
    if (m.role === 'user' && (m.images?.length || m.files?.length)) {
      wired.push({ role: 'user', content: buildUserContent(m.content, m.images, m.files) })
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
  // 计划上下文（M4）：执行步/总结轮时置于最末（当前任务的强约束，注意力最近）
  const planSys = buildPlanSystem()
  if (planSys) systems.push(planSys)
  return [...systems, ...wired]
}

/* ---------- Plan-and-Act 计划上下文注入 ---------- */

/** 计划注入总预算：2KB UTF-8（够表达 8 步目标清单与进展，不挤占技能/规则 16KB 预算） */
const PLAN_BUDGET_BYTES = 2 * 1024
/** 目标行截断（字符） */
const PLAN_GOAL_MAX_CHARS = 100
/** 步骤结果摘要截断（字符）：8 步 × 150 = 1.2KB，加目标与标题后总量 <2KB 预算 */
const PLAN_RESULT_MAX_CHARS = 150

/** 组装计划 system 消息：执行步轮注入「目标+进展+当前步+约束」，finishPlan 总结轮注入
 *  「全部步骤结果摘要」。超 2KB 预算时从最旧的已完成步骤摘要开始丢弃并标注。
 *  计划文本取自落盘的 plan 快照（确认时冻结），卡片展示与注入同源 */
function buildPlanSystem(): AiWireMessage | null {
  const enc = new TextEncoder()
  // finishPlan 总结轮：全部步骤结果摘要，无工具、纯文本收尾
  if (planSummaryRef.value) {
    const plan = messages.value.find(m => m.id === planSummaryRef.value)?.plan
    if (!plan) return null
    const head = `【执行计划总结】（用户确认的计划已全部落定）\n目标：${plan.goal.slice(0, PLAN_GOAL_MAX_CHARS)}\n各步骤结果：\n`
    const tail = `\n请基于以上执行过程给出完整总结。`
    const parts: string[] = []
    plan.steps.forEach((s, i) => {
      const label =
        s.state === 'done' ? '完成' : s.state === 'skipped' ? '跳过' : s.state === 'failed' ? '失败' : '未执行'
      parts.push(`${i + 1}. [${label}] ${s.title} — ${s.result ?? ''}`)
    })
    let omitted = 0
    while (parts.length > 1 && enc.encode(head + parts.join('\n') + tail).length > PLAN_BUDGET_BYTES) {
      parts.shift()
      omitted++
    }
    const content =
      head + (omitted ? `[早期 ${omitted} 步摘要省略]\n` : '') + parts.join('\n') + tail
    return { role: 'system', content }
  }
  // 执行步轮：目标 + 步骤进展 + 当前步 detail + 约束条款
  const run = planRunRef.value
  if (!run) return null
  const plan = messages.value.find(m => m.id === run.msgId)?.plan
  if (!plan) return null
  const i = run.stepIndex
  const n = plan.steps.length
  const step = plan.steps[i]
  if (!step) return null
  const head = `【执行计划】（第 ${i + 1}/${n} 步，用户已确认，须严格遵守）\n目标：${plan.goal.slice(0, PLAN_GOAL_MAX_CHARS)}\n步骤进展：\n`
  const tail =
    `\n约束：专注完成当前步骤；禁止擅自新增不在计划内的动作；确需偏离时先向用户说明并获得同意；文件创建与修改必须使用 write_file/edit_file 工具，禁止通过 shell 重定向或 heredoc 写文件；本步完成后用一句话总结本步结果；只执行当前步骤对应的动作，未到后续步骤不要提前执行；若本步目标已达成或已被先前步骤/本轮工具结果覆盖，直接输出结论文本结束本轮，禁止重复执行同一动作；若原始目标已被当前结果完整回答，或后续步骤的前提已不成立（如目标所需组件不存在），调用 conclude_plan 提前收敛，剩余步骤将标记跳过并直接进入结果归纳。`
  const parts: string[] = []
  plan.steps.forEach((s, idx) => {
    if (idx < i) parts.push(`${idx + 1}. [已完成] ${s.title} — ${s.result ?? ''}`)
    else if (idx === i) parts.push(`${idx + 1}. [当前步] ${s.title}：${s.detail}`)
    else parts.push(`${idx + 1}. [待执行] ${s.title}`)
  })
  // 从最旧已完成步骤摘要丢弃（不得丢当前步与待执行步）
  let omitted = 0
  while (omitted < i && enc.encode(head + parts.join('\n') + tail).length > PLAN_BUDGET_BYTES) {
    parts.shift()
    omitted++
  }
  const content = head + (omitted ? `[早期 ${omitted} 步摘要省略]\n` : '') + parts.join('\n') + tail
  return { role: 'system', content }
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

/** 解析提案 arguments（JSON 字符串）；解析失败返回 null（卡片禁用执行并显示原文）。
 *  steps 仅 propose_plan 使用（计划步骤数组） */
function parseProposalArgs(args: string): {
  command?: string
  reason?: string
  outcome?: string
  steps?: { title?: string; detail?: string }[]
} | null {
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
      sftp_upload_file: '上传文件',
      sftp_download_file: '下载文件',
    }[name] ?? name
  )
}

/** SFTP 传输提案的路径对（上传：本地→远端；下载：远端→本地） */
function sftpPaths(p: AiToolProposal): { local: string; remote: string } | null {
  try {
    const v = JSON.parse(p.args)
    const local = typeof v?.local_path === 'string' ? v.local_path : ''
    const remote = typeof v?.remote_path === 'string' ? v.remote_path : ''
    return local && remote ? { local, remote } : null
  } catch {
    return null
  }
}

/** 字节数人性化显示（KB/MB，一位小数） */
function humanBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / 1024 / 1024).toFixed(1)} MB`
}

/** AI 执行 SFTP 传输的体积上限：大文件长耗时易阻塞对话链路，引导模型走终端命令 */
const SFTP_AI_MAX_BYTES = 64 * 1024 * 1024

/** 下载落盘路径黑名单：本机敏感目录不作为 AI 传输目标（前缀匹配，~ 已按家目录展开判断） */
const SFTP_AI_LOCAL_DENY = ['/.ssh/', '/.gnupg/', '/.config/gcloud/', '/Library/Keychains/']

/** SFTP 传输提案执行：复用 SFTP 面板同款 IPC 命令（task 机制 + Channel 进度），
 *  进度实时写回 p.progress 驱动卡片进度条；预检失败直接落 rejected */
async function runSftpTransfer(p: AiToolProposal) {
  const sid = activeSession.value?.backendId
  if (!sid || activeSession.value?.state !== 'online') {
    p.state = 'rejected'
    p.error = '无可用会话或会话已断开'
    return
  }
  const paths = sftpPaths(p)
  if (!paths) {
    p.state = 'rejected'
    p.error = '参数缺少 local_path / remote_path'
    return
  }
  const isUpload = p.name === 'sftp_upload_file'
  // 本地敏感目录黑名单（仅下载写盘方向需要；上传只读本地）。
  // 后端会展开 ~，此处对原始路径与家目录形式均做前缀匹配
  if (!isUpload && SFTP_AI_LOCAL_DENY.some(d => paths.local.includes(d))) {
    p.state = 'rejected'
    p.error = '下载目标位于本机敏感目录，已拒绝'
    return
  }
  if (!isTauri) {
    p.state = 'rejected'
    p.error = '非桌面环境无法执行 SFTP 传输'
    return
  }
  const taskId = Date.now()
  // 预检通过：进入执行态（卡片显示传输中，进度条实时刷新）
  p.state = 'accepted'
  const channel = new Channel<{ transferred: number; total: number; speed: number; done: boolean }>()
  channel.onmessage = m => {
    p.progress = m.done ? { ...m, speed: 0 } : m
  }
  try {
    await invoke(isUpload ? 'sftp_upload' : 'sftp_download', {
      sessionId: sid,
      taskId,
      localPath: paths.local,
      remotePath: paths.remote,
      chunkKb: null,
      resume: false,
      // 体积上限由后端在打开目标文件写入前预检拒绝：既避免残留文件，也保证错误文案准确
      maxBytes: SFTP_AI_MAX_BYTES,
      channel,
    })
    p.state = 'accepted'
    p.execOutput = isUpload
      ? `已上传 ${humanBytes(p.progress?.transferred ?? 0)} → ${paths.remote}`
      : `已下载 ${humanBytes(p.progress?.transferred ?? 0)} → ${paths.local}`
  } catch (e) {
    p.state = 'rejected'
    p.error = typeof e === 'string' ? e : (e as Error)?.message ?? String(e)
  }
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
  // 计划提案：仅回传决策结果（不涉及远端执行）。rejected 强化文案（§6.8 层 1）：
  // 明确禁止再提案，引导模型在续跑轮改用普通工具完成任务
  if (p.name === 'propose_plan') {
    if (p.state === 'rejected') {
      return JSON.stringify({
        accepted: false,
        rejected: true,
        note: '用户已拒绝本次规划提案。禁止再次调用 propose_plan，请直接使用可用工具完成任务，不要再提案。',
      })
    }
    if (p.state === 'accepted') return JSON.stringify({ accepted: true })
    return JSON.stringify({ accepted: false, pending: true, note: '计划提案等待用户确认。' })
  }
  // 只读工具：直接返回输出文本（模型据此理解远程环境）
  if (p.name !== 'propose_command') {
    if (p.error) return JSON.stringify({ error: p.error })
    if (p.execOutput?.trim()) return p.execOutput
    // 成功落定但无标准输出（write_file/edit_file 静默成功最典型）：
    // 显式给成功信号，避免模型因「无输出」误判未生效而重复执行同一动作
    if (p.state === 'accepted') {
      return JSON.stringify({ success: true, note: '工具执行成功，无标准输出' })
    }
    return '(无输出)'
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
  if ((!input && !attachments.value.length) || streaming.value) return

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
  // 存在 draft/executing 计划：拦截发送（确认、取消或执行完毕后才可继续），避免多计划并行
  const activePlan = messages.value.find(
    m => m.plan && (m.plan.state === 'draft' || m.plan.state === 'executing'),
  )
  if (activePlan) {
    toast('有计划进行中，请先确认、取消或等待完成', 'warn', 2400)
    return
  }
  // 兜底：会话切换/载入遗留的失效转换（正常路径已由 resetProposalState 处理）
  for (const m of messages.value) {
    m.toolProposals?.forEach(p => {
      if (p.state === 'pending') p.state = 'expired'
    })
  }
  toolRound.value = 0

  // 规划模式检测（仅工作台 scope）：/plan 前缀或 + 菜单「规划模式」项均落到同一判据，
  // 命中即走强制规划路径（planForced=true）
  const usePlan = scope.value !== 'global' && PLAN_CMD_RE.test(input)
  // 附件快照（发送即清空）；按类型拆分：图片走 content parts，文本文件内联为 <file> 块
  const atts = attachments.value.slice()
  const imgs = atts.filter(a => a.kind === 'image')
  const files = atts.filter(a => a.kind === 'text')

  // 终端引用块并入本次 user 消息（引用在前，问题在后；模型对标记内内容按上下文对待）
  let text = usePlan ? input.replace(PLAN_CMD_RE, '').trim() : input
  if (!text && !atts.length) return // "/plan" 后无实际内容，且无附件
  if (quote.value) {
    text = `<terminal-output>\n${quote.value.text}\n</terminal-output>\n\n${text}`
    quote.value = null
  }

  const seq = ++loadSeq
  messages.value.push({
    id: uid(),
    role: 'user',
    content: text,
    images: imgs.length ? imgs : undefined,
    files: files.length ? files : undefined,
    status: 'done',
    createdAt: Date.now(),
  })
  streaming.value = true
  scrolledUp.value = false // 用户主动发送：恢复底部跟随
  draft.value = ''
  attachments.value = []
  scrollToBottom()
  persist() // 先落盘用户消息，防止崩溃丢失

  if (!isTauri) {
    // 浏览器 dev 降级：占位回复
    messages.value.push({
      id: uid(),
      role: 'assistant',
      content: '',
      status: 'streaming',
      createdAt: Date.now(),
    })
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

  if (usePlan) {
    // 强制规划路径（+ 菜单「规划模式」/ 手写 /plan）：本轮只下发 propose_plan + tool_choice，
    // 由服务端约束保证产出计划；产物经同一 onToolCalls 分流建卡（设计 §6.1 / §6.6）
    report('info', AI_PLAN_FORCED, '强制规划路径触发', {
      via: forcedVia,
      task_had_proposal: messages.value.some(
        m => !!m.plan || !!m.toolProposals?.some(p => p.name === 'propose_plan'),
      ),
    })
  }
  messages.value.push({
    id: uid(),
    role: 'assistant',
    content: '',
    status: 'streaming',
    createdAt: Date.now(),
  })
  scrollToBottom()
  persist()
  runAiStream(seq, usePlan)
}

/* ---------- 计划提案分流（propose_plan，设计 §6.4） ---------- */

/** 计划提案步骤摘要（卡片展示用；参数解析失败返回空数组） */
function planProposalSteps(p: AiToolProposal): { title: string; detail: string }[] {
  const parsed = parseProposalArgs(p.args ?? '')
  const raw = Array.isArray(parsed?.steps) ? parsed.steps : []
  return raw
    .map(s => ({ title: String(s?.title ?? '').trim(), detail: String(s?.detail ?? '').trim() }))
    .filter(s => s.title)
}

/** 计划提案建卡：解析 steps → 双闸校验（步数下限 / 空 title 过滤 / 超长截断，与工具 JSON
 *  Schema 同口径）→ 建 draft 计划卡片（p 保持 pending 挂起续跑）；校验失败直接拒绝并交续跑兜底。
 *  重规划上下文存在时替换既有计划的失败步及后续，而非新建卡片 */
function applyPlanProposal(msg: AiChatMessage, p: AiToolProposal) {
  const raw = planProposalSteps(p)
  const steps = raw
    .slice(0, PLAN_MAX_STEPS)
    .map(s => ({
      id: uid(),
      title: s.title.slice(0, PLAN_TITLE_MAX_CHARS),
      detail: s.detail,
      state: 'pending' as const,
    }))

  const fail = (reason: 'parse' | 'steps_missing' | 'too_few') => {
    p.state = 'rejected'
    p.error = '计划步骤不足，已按普通模式继续'
    report('warn', AI_PLAN_INVALID, '计划提案步骤校验失败', {
      fail_reason: reason,
      raw_step_count: raw.length,
    })
    if (forcedRound) {
      report('warn', AI_PLAN_FORCE_FAILED, '强制规划轮未产出合法计划', {
        reason: reason === 'parse' ? 'parse' : 'too_few',
      })
      toast('未能生成计划，请重试或用普通方式描述任务', 'warn', 3200)
    }
    settleProposalRound()
  }
  if (parseProposalArgs(p.args ?? '') === null) return fail('parse')
  if (!raw.length) return fail('steps_missing')
  if (steps.length < PLAN_MIN_STEPS) return fail('too_few')

  // 重规划：替换失败步及后续，plan 回 draft 再次确认（确认 / 执行 / 失败处理链路零改动）
  const ctx = replanCtx.value
  if (ctx) {
    const planMsg = messages.value.find(m => m.id === ctx.msgId)
    if (planMsg?.plan) {
      planMsg.plan.steps = [...planMsg.plan.steps.slice(0, ctx.failedIndex), ...steps]
      planMsg.plan.currentIndex = ctx.failedIndex
      planMsg.plan.state = 'draft'
      // 决策提案迁移到计划承载消息，保持「计划卡片 ↔ 决策提案」一一对应
      msg.toolProposals = (msg.toolProposals ?? []).filter(x => x !== p)
      planMsg.toolProposals = [...(planMsg.toolProposals ?? []), p]
      replanCtx.value = null
      report('info', AI_PLAN_PROPOSED, '重规划产新计划', {
        step_count: steps.length,
        tool_round: toolRound.value,
        has_write_in_round: false,
      })
      persist()
      scrollToBottom()
      return
    }
    replanCtx.value = null
  }

  const lastUser = [...messages.value].reverse().find(m => m.role === 'user')
  msg.plan = {
    goal: (lastUser?.content || '').trim() || '（含附件）',
    steps,
    currentIndex: 0,
    state: 'draft',
  }
  report('info', AI_PLAN_PROPOSED, '计划提案建卡', {
    step_count: steps.length,
    tool_round: toolRound.value,
    has_write_in_round: !!msg.toolProposals?.some(x => WRITE_TOOLS.includes(x.name)),
  })
  scrollToBottom()
}

/** 计划提案作废（同轮冲突 / 禁令兜底路径）：不建计划卡片，提案置 rejected 保留可读原因；
 *  同轮写操作提案不受影响，继续走既有授权链路；tool 结果一并回传由 settleProposalRound 推进续跑 */
function rejectPlanProposal(p: AiToolProposal, reason: string) {
  p.state = 'rejected'
  p.error = reason
}

/** 计划中首个失败步下标（无则 -1）：失败操作按钮的作用目标（同时只会有一个 failed 步） */
function planFailedIndex(plan: AiPlan): number {
  return plan.steps.findIndex(s => s.state === 'failed')
}

/** 活动计划：展示在输入框上方的可折叠面板。
 *  draft/executing 始终展示；done（提前收敛）保留展示各步最终状态直至用户确认；
 *  普通 done 自动消失（无需确认），由变更文件列表面板接替；
 *  cancelled 在用户确认收尾（acknowledged）前仍悬浮展示。 */
const activePlanMsg = computed<AiChatMessage | null>(
  () =>
    [...messages.value]
      .reverse()
      .find(m => m.plan && (
        m.plan.state === 'draft' ||
        m.plan.state === 'executing' ||
        (m.plan.state === 'done' && !!m.plan.converged && !m.plan.acknowledged) ||
        (m.plan.state === 'cancelled' && !m.plan.acknowledged)
      )) ?? null,
)
/** 面板折叠态（内存态）：新计划出现自动展开，用户手动切换后保持 */
const planCollapsed = ref(false)
watch(
  () => activePlanMsg.value?.id,
  () => {
    planCollapsed.value = false
  },
)
/** 面板标题：折叠 + 执行中时展示当前正在执行的步骤（随步骤更新），其余展示目标 */
const planHeadTitle = computed(() => {
  const plan = activePlanMsg.value?.plan
  if (!plan) return ''
  if (planCollapsed.value && plan.state === 'executing') {
    const step = plan.steps[Math.min(plan.currentIndex, plan.steps.length - 1)]
    if (step) return step.title
  }
  return plan.goal
})
/** 面板徽标文本：draft 计划中 / executing 执行中 / done 已收敛|已完成 / cancelled 已取消 */
const planBadgeText = computed(() => {
  const plan = activePlanMsg.value?.plan
  const s = plan?.state
  return s === 'draft' ? '计划中'
    : s === 'executing' ? '执行中'
    : s === 'done' ? (plan?.converged ? '已收敛' : '已完成')
    : '已取消'
})
/** 面板进度文本：draft 待确认 / executing 第 X/N 步 / done 已完成（含跳过步数）/ cancelled 已取消 */
const planProgressText = computed(() => {
  const plan = activePlanMsg.value?.plan
  if (!plan) return ''
  const n = plan.steps.length
  if (plan.state === 'draft') return `${n} 步 · 待确认`
  if (plan.state === 'executing') return `第 ${Math.min(plan.currentIndex + 1, n)}/${n} 步`
  if (plan.state === 'done') {
    const doneCount = plan.steps.filter(s => s.state === 'done').length
    const skippedCount = plan.steps.filter(s => s.state === 'skipped').length
    return skippedCount > 0
      ? `${doneCount}/${n} · 已完成（${skippedCount} 步跳过）`
      : `${n}/${n} · 已完成`
  }
  return '已取消'
})
/** 用户确认收尾：done/cancelled 后标记 acknowledged，悬浮面板消失（不进消息流） */
function acknowledgePlan(planMsg: AiChatMessage) {
  const plan = planMsg.plan
  if (!plan || (plan.state !== 'done' && plan.state !== 'cancelled')) return
  plan.acknowledged = true
  persist()
}

/* ---- 计划完成后的变更文件列表面板：接替计划面板，用户确认后消失 ---- */

/** 单文件变更统计（同一路径多次编辑累加） */
interface PlanFileChange {
  path: string
  added: number
  removed: number
}

/** 解析已接受 propose_command 命令中的常见文件写入模式（尽力识别）：
 *  heredoc、`>`/`>>` 重定向、tee、sed -i、touch、cp/mv 目标。
 *  能确定行数的给出行数（heredoc 正文行数、echo/printf 的 \n 转义个数），否则按 0 计仅展示路径 */
function extractShellWriteFiles(cmd: string): Array<{ path: string; added: number }> {
  const out: Array<{ path: string; added: number }> = []
  const push = (path: string | undefined, added: number) => {
    if (!path || path.startsWith('-') || path.startsWith('/dev/')) return
    out.push({ path, added })
  }
  // heredoc 正文整体剥离，行数留给同段的 tee/重定向目标关联
  const bodies: number[] = []
  const stripped = cmd.replace(
    /<<-?\s*['"]?(\w[\w-]*)['"]?[^\n]*\n([\s\S]*?)\n[ \t]*\1(?=\n|$)/g,
    (_m, _tag: string, body: string) => {
      bodies.push(body ? body.split('\n').length : 0)
      return ' \x00 '
    },
  )
  for (const seg of stripped.split(/&&|\|\||[;\n|]/)) {
    const added = seg.includes('\x00') ? (bodies.shift() ?? 0) : 0
    // sed -i 's/…/…/' /path：目标为末位参数（sed 脚本中可能含 >，需先于重定向判定）
    if (/\bsed\s+(?:-\S+\s+)*-i(?:\.\S*)?\s/.test(seg)) {
      push(seg.trim().split(/\s+/).pop()?.replace(/^['"]|['"]$/g, ''), 0)
      continue
    }
    let m = seg.match(/\btee\s+(?:-\S+\s+)*(["']?)([^\s;&|'"<>]+)\1/)
    if (m) {
      push(m[2], added)
      continue
    }
    // 输出重定向（排除 2>、&> 等文件描述符重定向）
    m = seg.match(/(?:^|[^\d&>])>>?\s*(["']?)([^\s;&|'"<>]+)\1/)
    if (m) {
      let n = added
      if (!n) {
        const em = seg.match(/\b(?:echo|printf)\s+(?:-\S+\s+)*(['"])((?:\\.|(?!\1)[\s\S])*)\1/)
        n = em ? em[2].split(/\\n/).length : 0
      }
      push(m[2], n)
      continue
    }
    if (/\btouch\s/.test(seg)) {
      for (const t of seg.trim().split(/\s+/).slice(1)) push(t.replace(/^['"]|['"]$/g, ''), 0)
      continue
    }
    m = seg.match(/\b(?:cp|mv)\s+(?:-\S+\s+)*(["']?)([^\s;&|'"<>]+)\1\s+(["']?)([^\s;&|'"<>]+)\3\s*$/)
    if (m) push(m[4], 0)
  }
  return out
}

/** 收集本次计划任务（计划消息所在回复组）内的文件变更，按路径聚合 +n/-n：
 *  1) 已接受的 edit_file/write_file 提案（行数精确）
 *  2) 已接受的 propose_command 中识别出的写文件模式（尽力识别，行数不确定时为 0）
 *  执行失败（error）或被拒绝的提案不计入 */
function collectPlanFiles(m: AiChatMessage): PlanFileChange[] {
  const idx = messages.value.indexOf(m)
  if (idx < 0) return []
  const end = groupEnd(idx)
  const byPath = new Map<string, PlanFileChange>()
  const merge = (path: string, added: number, removed: number) => {
    const cur = byPath.get(path)
    if (cur) {
      cur.added += added
      cur.removed += removed
    } else {
      byPath.set(path, { path, added, removed })
    }
  }
  for (let i = idx; i <= end; i++) {
    for (const p of messages.value[i].toolProposals ?? []) {
      if (p.state !== 'accepted' || p.error) continue
      if (p.name === 'edit_file' || p.name === 'write_file') {
        let path = ''
        try {
          const v = JSON.parse(p.args)
          path = typeof v?.path === 'string' ? v.path : ''
        } catch {
          continue
        }
        if (!path) continue
        merge(
          path,
          countLines(fileEditNew(p)),
          p.name === 'edit_file' ? countLines(fileEditOld(p)) : 0,
        )
      } else if (p.name === 'propose_command') {
        try {
          const v = JSON.parse(p.args)
          const cmd = typeof v?.command === 'string' ? v.command : ''
          if (!cmd) continue
          for (const f of extractShellWriteFiles(cmd)) merge(f.path, f.added, 0)
        } catch {
          continue
        }
      }
    }
  }
  return [...byPath.values()]
}

/** 变更文件面板目标消息：done + 有变更 + 未确认（取最新一条） */
const planFilesMsg = computed<AiChatMessage | null>(
  () =>
    [...messages.value]
      .reverse()
      .find(
        m =>
          m.plan &&
          m.plan.state === 'done' &&
          !m.plan.filesAcknowledged &&
          collectPlanFiles(m).length > 0,
      ) ?? null,
)
/** 变更文件列表（模板直接使用，避免重复聚合） */
const planFiles = computed<PlanFileChange[]>(
  () => (planFilesMsg.value ? collectPlanFiles(planFilesMsg.value) : []),
)
/** 面板折叠态（内存态）：新面板出现自动展开 */
const planFilesCollapsed = ref(false)
watch(
  () => planFilesMsg.value?.id,
  () => {
    planFilesCollapsed.value = false
  },
)
/** 用户确认变更文件列表：标记后面板消失 */
function acknowledgePlanFiles(msg: AiChatMessage) {
  if (!msg.plan) return
  msg.plan.filesAcknowledged = true
  persist()
}

/* ---------- Plan-and-Act 执行驱动（M4，设计文档 §6.4） ---------- */

/** 确认执行：draft 编辑直接落在 plan.steps 上（v-model 绑源数据无中间态），
 *  确认即冻结快照（卡片切换只读），随后从 currentIndex 启动逐步执行。
 *  同时把承载决策的 propose_plan 提案置 accepted（挂起解除，tool 结果回传模型） */
function confirmPlan(planMsg: AiChatMessage) {
  const plan = planMsg.plan
  if (!plan || plan.state !== 'draft') return
  const p = planMsg.toolProposals?.find(x => x.name === 'propose_plan' && x.state === 'pending')
  if (p) p.state = 'accepted'
  plan.state = 'executing'
  report('info', AI_PLAN_CONFIRMED, '用户确认执行计划', {
    step_count: plan.steps.length,
    latency_ms: Date.now() - planMsg.createdAt,
  })
  persist()
  scrollToBottom()
  runPlanStep(planMsg, plan.currentIndex)
}

/** 「不用计划，直接执行」：作废计划提案 + 计划置 cancelled + 本回合禁用再提案（§6.8 层 2），
 *  随后续跑一轮——模型收到强化后的拒绝文案，在无 propose_plan 的工具集下直接完成任务 */
function refusePlan(planMsg: AiChatMessage) {
  const plan = planMsg.plan
  if (!plan || plan.state !== 'draft') return
  const p = planMsg.toolProposals?.find(x => x.name === 'propose_plan' && x.state !== 'expired')
  if (p) {
    p.state = 'rejected'
    p.error = '用户选择不用计划，直接执行'
  }
  plan.state = 'cancelled'
  // draft 态拒绝属明确意图，面板直接消失，不再要求二次确认（执行中终止 abortPlan 仍保留确认）
  plan.acknowledged = true
  planBannedInRound.value = true
  report('info', AI_PLAN_REFUSED, '用户拒绝计划，改直接执行', {
    step_count: plan.steps.length,
    latency_ms: Date.now() - planMsg.createdAt,
  })
  persist()
  settleProposalRound()
}

/** 启动步骤 i：step 转 running、工具轮次归零（每步独立计数），
 *  追加 assistant 流式消息发起请求；buildPlanSystem 依 planRunRef 注入该步上下文 */
function runPlanStep(planMsg: AiChatMessage, i: number) {
  const plan = planMsg.plan
  const step = plan?.steps[i]
  if (!plan || !step) return
  step.state = 'running'
  step.result = undefined
  plan.currentIndex = i
  toolRound.value = 0
  planRunRef.value = { msgId: planMsg.id, stepIndex: i }
  messages.value.push({
    id: uid(),
    role: 'assistant',
    content: '',
    status: 'streaming',
    createdAt: Date.now(),
    stepIndex: i,
  })
  streaming.value = true
  scrollToBottom()
  persist()
  runAiStream(++loadSeq)
}

/** 步骤落定判定（finishStreaming 中 pumpProposals 之后调用）：
 *  - 用户中止 → 当前步 failed「用户中止」，plan 保持 executing 等决策（不回滚已执行操作）
 *  - 正常结束且续跑链已尽（无弹窗/无执行中/不再续跑）→ 该步 done，推进下一步或收尾 */
function maybeAdvancePlan(status: 'done' | 'stopped') {
  const run = planRunRef.value
  if (!run) return
  const planMsg = messages.value.find(m => m.id === run.msgId)
  if (!planMsg?.plan) {
    planRunRef.value = null
    return
  }
  if (status === 'stopped') {
    planRunRef.value = null
    failPlanStep(planMsg, run.stepIndex, '用户中止')
    return
  }
  // 提案弹窗中 / 只读工具执行中 / 已起续跑轮：步骤未落定，等后续回调推进
  if (streaming.value || hasPendingProposal() || autoRunning.size > 0) return
  planRunRef.value = null
  completePlanStep(planMsg, run.stepIndex)
}

/** 步骤完成：取该步最终 assistant 回复前 150 字符为结果摘要，推进下一步或 finishPlan。
 *  提前收敛：本步执行中模型调用过 conclude_plan（step.concluded）时，剩余 pending 步
 *  一律置 skipped（按 outcome 区分原因）并直接 finishPlan，不再执行后续步骤 */
function completePlanStep(planMsg: AiChatMessage, i: number) {
  const plan = planMsg.plan
  const step = plan?.steps[i]
  if (!plan || !step) return
  const last = messages.value[messages.value.length - 1]
  const content = last?.role === 'assistant' ? last.content.trim() : ''
  step.state = 'done'
  step.result = (content || '（无文本输出）').slice(0, PLAN_RESULT_MAX_CHARS)
  if (step.concluded) {
    const skippedReason =
      step.concludeOutcome === 'blocked' ? '后续步骤不可行，已提前结束' : '已获得答案，无需继续'
    for (const s of plan.steps) {
      if (s.state !== 'pending') continue
      s.state = 'skipped'
      s.result = skippedReason
    }
    // 收敛标记：计划转 done 后规划面板不自动消失，保留展示各步最终状态等待用户确认
    plan.converged = true
    report('info', AI_PLAN_CONCLUDED, '计划提前收敛', {
      outcome: step.concludeOutcome ?? 'resolved',
      step_index: i + 1,
      step_count: plan.steps.length,
      skipped_count: plan.steps.filter(s => s.state === 'skipped').length,
      duration_ms: Date.now() - planMsg.createdAt,
    })
    persist()
    finishPlan(planMsg)
    return
  }
  persist()
  if (i + 1 < plan.steps.length) runPlanStep(planMsg, i + 1)
  else finishPlan(planMsg)
}

/** 步骤失败：转 failed 记原因，plan 保持 executing，卡片呈现失败操作按钮等用户决策 */
function failPlanStep(planMsg: AiChatMessage, i: number, reason: string) {
  const plan = planMsg.plan
  const step = plan?.steps[i]
  if (!plan || !step) return
  step.state = 'failed'
  step.result = reason.slice(0, PLAN_RESULT_MAX_CHARS)
  report('warn', AI_PLAN_STEP_FAILED, '计划步骤执行失败', {
    step_index: i + 1,
    fail_reason: 'aborted',
  })
  persist()
  scrollToBottom()
}

/** 全部步骤落定：plan 转 done，发起总结轮（注入全部步骤摘要、不带工具，纯文本收尾）。
 *  总结轮关闭思考：各步结果已注入上下文，纯文本归纳无需推理 */
function finishPlan(planMsg: AiChatMessage) {
  const plan = planMsg.plan
  if (!plan) return
  plan.state = 'done'
  planSummaryRef.value = planMsg.id
  report('info', AI_PLAN_FINISHED, '计划执行完成', {
    step_count: plan.steps.length,
    done_count: plan.steps.filter(s => s.state === 'done').length,
    failed_count: plan.steps.filter(s => s.state === 'failed').length,
    duration_ms: Date.now() - planMsg.createdAt,
  })
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
  runAiStream(++loadSeq)
}

/** 重试失败步：重置为 pending（result 清空）后重新启动；已产生的 assistant 消息留痕 */
function retryPlanStep(planMsg: AiChatMessage, i: number) {
  const plan = planMsg.plan
  const step = plan?.steps[i]
  if (!plan || plan.state !== 'executing' || !step || step.state !== 'failed') return
  step.state = 'pending'
  step.result = undefined
  persist()
  runPlanStep(planMsg, i)
}

/** 跳过失败步：记「用户跳过」，推进下一步或收尾（显式用户决策，落盘） */
function skipPlanStep(planMsg: AiChatMessage, i: number) {
  const plan = planMsg.plan
  const step = plan?.steps[i]
  if (!plan || plan.state !== 'executing' || !step || step.state !== 'failed') return
  step.state = 'skipped'
  step.result = '用户跳过'
  persist()
  if (i + 1 < plan.steps.length) runPlanStep(planMsg, i + 1)
  else finishPlan(planMsg)
}

/** 终止计划：plan 转 cancelled，剩余步骤保持 pending（显式用户决策，落盘） */
function abortPlan(planMsg: AiChatMessage) {
  const plan = planMsg.plan
  if (!plan || plan.state !== 'executing') return
  plan.state = 'cancelled'
  planRunRef.value = null
  report('info', AI_PLAN_ABORTED, '用户终止计划', {
    step_index: plan.currentIndex + 1,
  })
  persist()
}

/** 重新规划：拼装「目标 + 各步进展 + 失败步详情与原因」走强制规划轮（planForced=true），
 *  产出的 propose_plan 由 replanCtx 落到计划失败步，替换该步及后续（已完成步骤保留），
 *  计划回 draft 再次经用户确认 */
function replan(planMsg: AiChatMessage, failedIndex: number) {
  const plan = planMsg.plan
  const failed = plan?.steps[failedIndex]
  if (!plan || !failed) return
  const lines = [
    `原始目标：${plan.goal}`,
    '',
    '已确认计划的执行进展：',
    ...plan.steps.map((s, idx) => {
      const label =
        s.state === 'done' ? '已完成' : s.state === 'skipped' ? '已跳过' : s.state === 'failed' ? '失败' : '未执行'
      return `${idx + 1}. [${label}] ${s.title}${s.result ? ` — ${s.result}` : ''}`
    }),
    '',
    `第 ${failedIndex + 1} 步「${failed.title}」执行失败：${failed.detail}`,
    `失败原因：${failed.result ?? '未知'}`,
    '',
    '请基于以上进展，重新规划从该步开始的剩余步骤（已完成步骤不要重复）。',
  ]
  // 重规划上下文：本轮 propose_plan 落到该计划的失败步（替换而非新建）；
  // 进展描述经 runAiStream 作为瞬时 user 消息注入（不进对话记录）
  replanCtx.value = { msgId: planMsg.id, failedIndex, progress: lines.join('\n') }
  // 重规划轮等价强制规划路径（via=replan），与用户动作埋点分别记录
  report('info', AI_PLAN_FORCED, '强制规划轮触发', { via: 'replan', task_had_proposal: true })
  report('info', AI_PLAN_REPLANNED, '用户触发重新规划', { step_index: failedIndex + 1 })
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
  runAiStream(seq, true)
}

/** 发起流式请求（send 与提案裁决续跑共用）。工具提案仅在工作台会话下启用
 *  （全局对话无终端可写）；请求失败写入最后一条 assistant 消息。
 *  技能/规则资产与记忆每轮重新拉取：无缓存实时读，下一轮即最新。
 *  planForced=true 为强制 / 重规划轮：后端只下发 propose_plan 并置 tool_choice。 */
async function runAiStream(seq: number, planForced = false) {
  let assets: AiPromptAssets | undefined
  let memories: AiMemoryItem[] | undefined
  try {
    ;[assets, memories] = await Promise.all([aiListPromptAssets(), aiListMemories()])
  } catch (e) {
    // 资产/记忆加载失败降级为空注入，对话主流程不受影响
    console.warn('[AI] 技能/规则/记忆加载失败（按空注入）:', e)
  }
  if (seq !== loadSeq) return // 拉取期间切换了 scope，丢弃本次发送
  // 强制标记随本次流建立（续跑轮由 continueToolRound 传入默认 false 自动复位）
  forcedRound = planForced
  forcedProposalSeen = false
  const wire = buildWireMessages(assets, memories)
  // 重规划轮：把「目标 + 各步进展 + 失败原因」作为瞬时 user 消息注入（不落盘、不进对话记录）
  if (planForced && replanCtx.value?.progress) {
    wire.push({ role: 'user', content: replanCtx.value.progress })
  }
  aiChatStream(scope.value, wire, {
    // finishPlan 总结轮不带工具：纯文本收尾，避免总结阶段再起提案循环
    enableTools: scope.value !== 'global' && !planSummaryRef.value,
    planForced,
    // 计划执行步：后端剔除 propose_plan（执行中不再提新计划）并追加 conclude_plan（提前收敛判定）
    planStepActive: planRunRef.value !== null && !planSummaryRef.value,
    disablePlanProposal: planBannedInRound.value,
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

/** 提案裁决后的自动续跑：追加新的 assistant 流式消息再发起请求。
 *  计划步执行中的续跑轮继承当前步下标（过程区内归入同一步骤分隔条之下）。 */
function continueToolRound() {
  const seq = ++loadSeq
  messages.value.push({
    id: uid(),
    role: 'assistant',
    content: '',
    status: 'streaming',
    createdAt: Date.now(),
    stepIndex: planRunRef.value?.stepIndex,
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
      // 写操作（propose_command / edit_file / write_file / sftp_*_file）需弹窗授权
      if (
        p.state === 'pending' &&
        ['propose_command', 'edit_file', 'write_file', 'sftp_upload_file', 'sftp_download_file'].includes(p.name) &&
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
  // 计划步执行中会话断开（提案因会话问题 rejected）：该步直接转 failed 等用户决策，
  // 不再续跑让模型在断开的会话上空转（设计 §7 降级：不自动重试）
  const run = planRunRef.value
  if (run) {
    const planMsg = messages.value.find(m => m.id === run.msgId)
    const last = [...messages.value].reverse().find(m => m.role === 'assistant')
    const disconnected = last?.toolProposals?.some(
      p => p.state === 'rejected' && p.error?.includes('会话'),
    )
    if (planMsg?.plan && disconnected) {
      planRunRef.value = null
      failPlanStep(planMsg, run.stepIndex, '会话已断开')
      return
    }
  }
  toolRound.value++
  if (toolRound.value > MAX_TOOL_ROUNDS) {
    toast(`已达到工具循环轮次上限（${MAX_TOOL_ROUNDS}），如需继续请重新提问`, 'warn', 3200)
    // 计划步执行中超限：该步转 failed「工具轮次上限」，等用户决策
    const run = planRunRef.value
    if (run) {
      planRunRef.value = null
      const planMsg = messages.value.find(m => m.id === run.msgId)
      if (planMsg?.plan) failPlanStep(planMsg, run.stepIndex, '工具轮次上限')
    }
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
  } else if (p.name === 'sftp_upload_file' || p.name === 'sftp_download_file') {
    // SFTP 传输：复用 SFTP 面板 IPC（task + Channel 进度），不写终端
    await runSftpTransfer(p)
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
  if (summaryUpTo.value) {
    const upIdx = messages.value.findIndex(m => m.id === summaryUpTo.value)
    if (upIdx < 0 || cutIndex <= upIdx) {
      summary.value = ''
      summaryUpTo.value = ''
    }
  }
  // 计划随消息截断一并作废：清理指向已移除消息的计划内存态引用
  // （设计 §6.7：重新生成/删除/编辑重发截断到 user 消息时，计划作废）
  if (planRunRef.value && !messages.value.some(m => m.id === planRunRef.value!.msgId)) {
    planRunRef.value = null
  }
  if (planSummaryRef.value && !messages.value.some(m => m.id === planSummaryRef.value)) {
    planSummaryRef.value = null
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

/** 编辑重发：仅填回输入框并聚焦，截断推迟到真正发送时；附件一并回填保持一致 */
function startEdit(m: AiChatMessage) {
  if (streaming.value) return
  editFromId.value = m.id
  draft.value = m.content
  attachments.value = [
    ...(m.images ?? []).map(img => ({ ...img, kind: 'image' as const })),
    ...(m.files ?? []),
  ]
  nextTick(() => inputRef.value?.focus())
}

/** 删除 user 消息及其整个 assistant 回复组（连续 assistant 消息），不弹确认框 */
function deleteMessage(m: AiChatMessage) {
  if (streaming.value) return
  const idx = messages.value.indexOf(m)
  if (idx < 0) return
  const count =
    messages.value[idx + 1]?.role === 'assistant' ? groupEnd(idx + 1) - idx + 1 : 1
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
  traceManual.value.clear()
  reasoningManual.value.clear()
  // 计划内存态一并清理（卡片随消息清空）
  planRunRef.value = null
  planSummaryRef.value = null
  replanCtx.value = null
  planBannedInRound.value = false
  // 输入区上下文 chip 一并清理
  quote.value = null
  attachments.value = []
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

// 文本域自增高：草稿变化（输入/清空/填回）后重算高度
watch(draft, () => nextTick(autoGrow))

/** 用户上翻守卫阈值（px）：消息区滚动位置距底部超过该值即视为上翻中 */
const STICK_THRESHOLD_PX = 80
/** 用户上翻中（流式 token / 晋升滚动共用的同一口径），由 scroll 事件维护 */
const scrolledUp = ref(false)

function onChatScroll() {
  const el = scrollEl.value
  if (!el) return
  scrolledUp.value = el.scrollHeight - el.scrollTop - el.clientHeight > STICK_THRESHOLD_PX
  scheduleSpy() // 会话大纲 scroll-spy：rAF 节流，仅导轨显示期间生效
}

function scrollToBottom() {
  nextTick(() => {
    if (scrollEl.value) scrollEl.value.scrollTop = scrollEl.value.scrollHeight
  })
}

/** 任务驱动滚动（流式 token 逐帧跟随）：用户上翻查看历史时不劫持视口 */
function followScroll() {
  if (scrolledUp.value) return
  scrollToBottom()
}

/** 晋升滚动：最终结果首次出现时平滑滚动至底部（结果区顶部）补偿上移；
 *  用户上翻期间禁止，保持其当前阅读位置 */
function scrollToResult() {
  if (scrolledUp.value) return
  nextTick(() =>
    scrollEl.value?.scrollTo({ top: scrollEl.value.scrollHeight, behavior: 'smooth' }),
  )
}

/* ---------- 会话大纲（刻度导轨）：消息列表的只读派生视图，无持久化、无手动开关 ---------- */

/** 大纲条目：每条 user 消息（回复组锚点）派生一条 */
interface OutlineEntry {
  /** 锚点消息 id = user 消息 id */
  id: string
  /** 派生标题（已扁平化、长度兜底） */
  title: string
  /** 组执行状态（悬浮标签内状态点用色） */
  state: 'waiting' | 'running' | 'done' | 'error' | 'stopped'
}

/** 内联 `<file name="…">…</file>` 附件块匹配（与多模态发送装配格式对应） */
const FILE_BLOCK_RE = /<file name="[^"]*">[\s\S]*?<\/file>/g

/** 标题派生：剔除附件块 → 空白折叠 → 空串回退（图片/首个文件名/空消息）→ 截断 60 字符 */
function outlineTitle(m: AiChatMessage): string {
  let t = m.content.replace(FILE_BLOCK_RE, '').replace(/\s+/g, ' ').trim()
  if (!t) {
    if (m.images?.length) t = '「图片」'
    else if (m.files?.length) t = m.files[0].name
    else t = '「（空消息）」'
  }
  return t.slice(0, 60)
}

/** 大纲条目派生：与渲染层 v2 同一分组口径（user 消息 + 紧随的连续 assistant 回复组） */
const outlineEntries = computed<OutlineEntry[]>(() => {
  const msgs = messages.value
  const out: OutlineEntry[] = []
  for (let i = 0; i < msgs.length; i++) {
    const m = msgs[i]
    if (m.role !== 'user') continue
    let state: OutlineEntry['state'] = 'waiting'
    if (msgs[i + 1]?.role === 'assistant') {
      const end = groupEnd(i + 1)
      let anyStreaming = false
      for (let j = i + 1; j <= end; j++) {
        if (msgs[j].status === 'streaming') {
          anyStreaming = true
          break
        }
      }
      const last = msgs[end]
      if (anyStreaming) state = 'running'
      else if (last.status === 'error') state = 'error'
      else if (last.status === 'stopped') state = 'stopped'
      else state = 'done'
    }
    out.push({ id: m.id, title: outlineTitle(m), state })
  }
  return out
})

/** 导轨显示所需的面板最小宽度（px）：760 消息区 + 两侧各 34px（22 导轨 + 12 间距）；
 *  左侧 margin 放不下导轨时整体隐藏，避免遮挡消息正文 */
const RAIL_MIN_WIDTH_PX = 828
/** 面板宽度是否达到导轨显示门槛（ResizeObserver 维护，初值 true 避免宽屏初始闪烁） */
const railWidthOk = ref(true)
/** 导轨显示门槛：对话轮数 > 3（第 4 条 user 消息出现）且面板够宽才渲染，否则恒隐藏 */
const showRail = computed(() => outlineEntries.value.length > 3 && railWidthOk.value)

/** 当前激活刻度（scroll-spy 推导的最近条目 id） */
const activeOutlineId = ref('')
/** 悬浮标签：hover/focus 刻度时显示（fixed 定位，导轨右缘 + 14px，与刻度垂直对中） */
const outlineTip = ref<{ x: number; y: number; entry: OutlineEntry } | null>(null)
/** 导轨内部滚动容器 */
const railEl = ref<HTMLElement | null>(null)
/** 导轨淡出遮罩状态：可滚 / 贴顶 / 贴底 */
const railCanScroll = ref(false)
const railAtTop = ref(true)
const railAtBottom = ref(true)

/** 激活条目下标（-1 = 无激活） */
const activeOutlineIndex = computed(() =>
  outlineEntries.value.findIndex(e => e.id === activeOutlineId.value),
)
/** 已读（past）刻度集合：激活条目之前的全部条目 */
const pastOutlineIds = computed(() => {
  const s = new Set<string>()
  for (let i = 0; i < activeOutlineIndex.value; i++) s.add(outlineEntries.value[i].id)
  return s
})

/** 锚点位置缓存（性能关键）：offsetTop 只在布局变化时改变，滚动本身不改变——
 *  采样时机一次性读取缓存为升序数组，帧回调零布局读取、仅二分查找 */
let anchorTops: { id: string; top: number }[] = []

/** 采样锚点位置：以消息区内容坐标系（rect 推导，与定位基准无关）记录各锚点 top */
function sampleAnchors() {
  const el = scrollEl.value
  if (!el) {
    anchorTops = []
    return
  }
  const scrollRect = el.getBoundingClientRect()
  const arr: { id: string; top: number }[] = []
  for (const e of outlineEntries.value) {
    const node = el.querySelector(`[data-mid="${e.id}"]`) as HTMLElement | null
    if (!node) continue // 锚点缺失（极端时序）：跳过该条
    const r = node.getBoundingClientRect()
    arr.push({ id: e.id, top: r.top - scrollRect.top + el.scrollTop })
  }
  arr.sort((a, b) => a.top - b.top)
  anchorTops = arr
}

/** 当前刻度保持可见：active 变化后校正导轨内部滚动（上下各留 6px 余量） */
function ensureActiveTickVisible() {
  const rail = railEl.value
  const idx = activeOutlineIndex.value
  if (!rail || idx < 0) return
  const tick = rail.children[idx] as HTMLElement | undefined
  if (!tick) return
  const M = 6
  const railRect = rail.getBoundingClientRect()
  const tr = tick.getBoundingClientRect()
  const y = tr.top - railRect.top + rail.scrollTop
  if (y < rail.scrollTop + M) rail.scrollTop = y - M
  else if (y + tr.height > rail.scrollTop + rail.clientHeight - M)
    rail.scrollTop = y + tr.height - rail.clientHeight + M
}

function setActiveOutline(id: string) {
  if (activeOutlineId.value === id) return
  activeOutlineId.value = id
  ensureActiveTickVisible()
}

/** scroll-spy 激活计算：激活线 = 可视高度 × 0.34（偏视口上 1/3，符合阅读视线）；
 *  取最后一个 top <= scrollTop + 激活线 的条目为 active；贴底 40px 内强制末条 active */
function updateActiveOutline() {
  const el = scrollEl.value
  if (!el || !anchorTops.length) return
  if (el.scrollTop + el.clientHeight >= el.scrollHeight - 40) {
    setActiveOutline(anchorTops[anchorTops.length - 1].id)
    return
  }
  const line = el.scrollTop + el.clientHeight * 0.34
  let lo = 0
  let hi = anchorTops.length - 1
  let ans = -1
  while (lo <= hi) {
    const mid = (lo + hi) >> 1
    if (anchorTops[mid].top <= line) {
      ans = mid
      lo = mid + 1
    } else {
      hi = mid - 1
    }
  }
  setActiveOutline(anchorTops[ans >= 0 ? ans : 0].id)
}

/** rAF 节流：滚动事件只置位，帧回调内统一计算 */
let spyRaf = 0
function scheduleSpy() {
  if (!showRail.value || spyRaf) return
  spyRaf = requestAnimationFrame(() => {
    spyRaf = 0
    updateActiveOutline()
  })
}

/** 导轨淡出遮罩状态同步（随导轨滚动、采样、条目变化） */
function syncRailFades() {
  const rail = railEl.value
  if (!rail) return
  railCanScroll.value = rail.scrollHeight > rail.clientHeight + 1
  railAtTop.value = rail.scrollTop <= 1
  railAtBottom.value = rail.scrollTop + rail.clientHeight >= rail.scrollHeight - 1
}

function onRailScroll() {
  syncRailFades()
  outlineTip.value = null // 导轨内部滚动时立即收起标签，避免错位
}

/** 重采样 + 激活重算 + 遮罩同步（DOM 变化后下一帧执行） */
function resampleAnchors() {
  nextTick(() => {
    if (!showRail.value) return
    sampleAnchors()
    updateActiveOutline()
    syncRailFades()
  })
}

/** 点击刻度：平滑滚动定位到对应 user 消息（锚点缺失静默不滚动），并伴随 1.1s 闪烁反馈 */
function scrollToOutline(e: OutlineEntry) {
  const el = scrollEl.value
  const node = el?.querySelector(`[data-mid="${e.id}"]`) as HTMLElement | null
  if (!node) return
  node.scrollIntoView({ behavior: 'smooth', block: 'start' })
  node.classList.remove('flash')
  void node.offsetWidth // 强制重启动画
  node.classList.add('flash')
  window.setTimeout(() => node.classList.remove('flash'), 1150)
}

function showOutlineTip(e: OutlineEntry, ev: MouseEvent | FocusEvent) {
  const tick = ev.currentTarget as HTMLElement
  const shell = tick.closest('.rail-shell') as HTMLElement | null
  if (!shell) return
  const sr = shell.getBoundingClientRect()
  const tr = tick.getBoundingClientRect()
  outlineTip.value = { x: sr.right + 14, y: tr.top + tr.height / 2, entry: e }
}

function hideOutlineTip() {
  outlineTip.value = null
}

/** 重采样时机：导轨挂载（含第 4 轮出现/切换 scope 恢复）即首次采样，不等首次滚动 */
watch(showRail, v => {
  if (v) {
    resampleAnchors()
  } else {
    activeOutlineId.value = ''
    anchorTops = []
    outlineTip.value = null
  }
})
/** 重采样时机：大纲结构变化（锚点增删、编辑重发、删除整组、清空） */
watch(() => outlineEntries.value.map(e => e.id).join(','), resampleAnchors)
/** 重采样时机：任务落定瞬间折叠区自动收起改变组总高度（流式 token 追加不触发） */
watch(
  () => streaming.value,
  (busy, prev) => {
    if (prev && !busy) resampleAnchors()
  },
)
/** 切换 scope：导轨内部滚动位置与高亮随会话重置（条目 watcher 负责重采样） */
watch(scope, () => {
  activeOutlineId.value = ''
  anchorTops = []
  outlineTip.value = null
  if (railEl.value) railEl.value.scrollTop = 0
})

/** 重采样时机：chat-wrap 宽度变化（消息区换行高度重排 / 左侧 margin 变化；token 追加不触发） */
const chatWrapEl = ref<HTMLElement | null>(null)
let chatResizeObs: ResizeObserver | null = null
let lastChatWidth = 0
onMounted(() => {
  if (chatWrapEl.value) {
    lastChatWidth = chatWrapEl.value.clientWidth
    railWidthOk.value = lastChatWidth >= RAIL_MIN_WIDTH_PX
    chatResizeObs = new ResizeObserver(() => {
      const w = chatWrapEl.value?.clientWidth ?? 0
      railWidthOk.value = w >= RAIL_MIN_WIDTH_PX
      if (w === lastChatWidth) return
      lastChatWidth = w
      resampleAnchors()
    })
    chatResizeObs.observe(chatWrapEl.value)
  }
  window.addEventListener('resize', resampleAnchors)
})
onUnmounted(() => {
  chatResizeObs?.disconnect()
  window.removeEventListener('resize', resampleAnchors)
  if (spyRaf) cancelAnimationFrame(spyRaf)
})

function onKeydown(e: KeyboardEvent) {
  // 组合输入（中文/日文输入法）期间回车不发送，避免选词误发
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
    e.preventDefault()
    send()
  }
}

/* ---------- 附件（多模态输入）：图片 + 文本/代码文件，粘贴 / 拖拽 / 选择三入口 ---------- */

/** 允许的图片 MIME（主流模型 vision 支持的范围；gif 取首帧） */
const IMAGE_ALLOWED_TYPES = ['image/png', 'image/jpeg', 'image/webp', 'image/gif']
/** 单图原始大小上限：防超大文件读入内存（正常截图/照片远小于此） */
const IMAGE_MAX_RAW_BYTES = 8 * 1024 * 1024
/** 长边上限：超出按比例缩放（视觉模型推荐上限，降低 token 与传输体积） */
const IMAGE_MAX_EDGE = 1568
/** 单图压缩后上限 */
const IMAGE_MAX_ENCODED_BYTES = 2 * 1024 * 1024
/** 单条消息图片数量上限 */
const IMAGE_MAX_COUNT = 4
/** 单条消息附件合计上限 */
const IMAGE_TOTAL_MAX_BYTES = 6 * 1024 * 1024

/** 允许的文本/代码文件扩展名（小写含点，如 '.ts'） */
const FILE_ALLOWED_EXTS = [
  // 文本 / 文档
  '.txt', '.md', '.markdown', '.log', '.csv', '.tsv', '.rst', '.adoc', '.tex',
  // 配置 / 数据
  '.json', '.json5', '.yaml', '.yml', '.toml', '.ini', '.conf', '.cfg', '.env',
  '.properties', '.xml', '.plist', '.editorconfig',
  // 代码
  '.py', '.js', '.mjs', '.cjs', '.ts', '.tsx', '.jsx', '.vue', '.svelte',
  '.rs', '.go', '.java', '.kt', '.kts', '.c', '.h', '.cc', '.cpp', '.hpp', '.cs',
  '.rb', '.php', '.swift', '.scala', '.sh', '.bash', '.zsh', '.fish', '.bat', '.ps1',
  '.sql', '.graphql', '.gql', '.proto', '.lua', '.pl', '.pm', '.r', '.dart', '.ex', '.exs',
  '.html', '.htm', '.css', '.scss', '.sass', '.less', '.styl', '.vim',
  '.gradle', '.cmake', '.tf', '.tfvars', '.hcl', '.diff', '.patch', '.mk',
]
/** 无扩展名的常见文本/配置文件名（整名小写匹配） */
const FILE_ALLOWED_NAMES = ['dockerfile', 'makefile', 'rakefile', 'gemfile', 'procfile', 'brewfile']
/** 单文件原始大小上限：内容内联进上下文，过大推高 token 成本 */
const FILE_MAX_RAW_BYTES = 1024 * 1024
/** 单条消息文本/代码文件数量上限 */
const FILE_MAX_COUNT = 4
/** 二进制守卫检查的字符数（只查头部，兼顾性能与准确性） */
const FILE_BINARY_SCAN_CHARS = 8192
/** `<input accept>` 值：图片 MIME + 文本/代码扩展名（由白名单派生，避免手工维护） */
const FILE_ACCEPT = [...IMAGE_ALLOWED_TYPES, ...FILE_ALLOWED_EXTS].join(',')

/** 多行文本域最大增高像素（约 6 行） */
const COMPOSER_MAX_HEIGHT = 132

/** data URL 的近似字节数（去掉头部后按 base64 4 字符 3 字节换算） */
function dataUrlBytes(dataUrl: string): number {
  const i = dataUrl.indexOf(',')
  if (i < 0) return 0
  return Math.floor((dataUrl.length - i - 1) * 3 / 4)
}

function readFileAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader()
    r.onload = () => resolve(String(r.result))
    r.onerror = () => reject(new Error('读取失败'))
    r.readAsDataURL(file)
  })
}

/** 采集单个图片文件：白名单 → 原始大小 → 解码 → 缩放/重编码 → 体积与数量校验 → 入列 */
async function addImageFile(file: File, name?: string) {
  if (!IMAGE_ALLOWED_TYPES.includes(file.type)) {
    toast('不支持的图片格式（仅 png / jpeg / webp / gif）', 'warn', 2600)
    return
  }
  if (file.size > IMAGE_MAX_RAW_BYTES) {
    toast('图片过大（原始超过 8MB）', 'warn', 2600)
    return
  }
  if (attachments.value.filter(a => a.kind === 'image').length >= IMAGE_MAX_COUNT) {
    toast(`最多添加 ${IMAGE_MAX_COUNT} 张图片`, 'warn', 2600)
    return
  }
  try {
    const bmp = await createImageBitmap(file)
    const longEdge = Math.max(bmp.width, bmp.height)
    let dataUrl: string
    let w = bmp.width
    let h = bmp.height
    if (longEdge > IMAGE_MAX_EDGE) {
      // 缩放重编码：保持原 MIME（PNG 保文字清晰；GIF 取首帧转 PNG）
      const scale = IMAGE_MAX_EDGE / longEdge
      w = Math.max(1, Math.round(bmp.width * scale))
      h = Math.max(1, Math.round(bmp.height * scale))
      const canvas = document.createElement('canvas')
      canvas.width = w
      canvas.height = h
      const ctx = canvas.getContext('2d')
      if (!ctx) throw new Error('canvas 不可用')
      ctx.drawImage(bmp, 0, 0, w, h)
      const mime = file.type === 'image/gif' ? 'image/png' : file.type
      dataUrl = canvas.toDataURL(mime, mime === 'image/jpeg' ? 0.85 : undefined)
    } else {
      dataUrl = await readFileAsDataUrl(file)
    }
    bmp.close()
    const sizeBytes = dataUrlBytes(dataUrl)
    if (sizeBytes > IMAGE_MAX_ENCODED_BYTES) {
      toast('图片过大（压缩后超过 2MB）', 'warn', 2600)
      return
    }
    const total = attachments.value.reduce((n, a) => n + a.sizeBytes, 0)
    if (total + sizeBytes > IMAGE_TOTAL_MAX_BYTES) {
      toast('附件合计超过 6MB', 'warn', 2600)
      return
    }
    attachments.value.push({
      id: uid(),
      kind: 'image',
      dataUrl,
      name: name || file.name || '粘贴的图片',
      sizeBytes,
      width: w,
      height: h,
    })
  } catch {
    toast('图片解码失败', 'warn', 2600)
  }
}

/** 取文件小写扩展名（含点）；无扩展名返回空串 */
function fileExt(name: string): string {
  const i = name.lastIndexOf('.')
  return i >= 0 ? name.slice(i).toLowerCase() : ''
}

/** 是否属于白名单文本/代码文件（按扩展名或整名判定） */
function isAllowedTextFile(file: File): boolean {
  const lower = file.name.toLowerCase()
  if (FILE_ALLOWED_NAMES.includes(lower)) return true
  return FILE_ALLOWED_EXTS.includes(fileExt(lower))
}

/** 内容二进制守卫：含 NUL 字节视为二进制，拒绝（防改扩展名绕过白名单） */
function looksBinary(text: string): boolean {
  return text.slice(0, FILE_BINARY_SCAN_CHARS).includes('\u0000')
}

/** 采集单个文本/代码文件：白名单 → 原始大小 → UTF-8 读取 → 二进制守卫 → 入列 */
async function addTextFile(file: File) {
  if (!isAllowedTextFile(file)) {
    toast(`不支持的文本/代码文件：${file.name}`, 'warn', 3000)
    return
  }
  if (file.size > FILE_MAX_RAW_BYTES) {
    toast(`文件过大（${file.name} 超过 1MB）`, 'warn', 3000)
    return
  }
  if (attachments.value.filter(a => a.kind === 'text').length >= FILE_MAX_COUNT) {
    toast(`最多添加 ${FILE_MAX_COUNT} 个文件`, 'warn', 2600)
    return
  }
  try {
    const text = await file.text()
    if (looksBinary(text)) {
      toast(`文件疑似二进制，已拒绝：${file.name}`, 'warn', 3000)
      return
    }
    attachments.value.push({
      id: uid(),
      kind: 'text',
      name: file.name || '未命名文件',
      sizeBytes: file.size,
      text,
    })
  } catch {
    toast(`文件读取失败：${file.name}`, 'warn', 3000)
  }
}

/** 按类型分流：图片走压缩链路，白名单文本/代码文件走文本读入链路 */
async function addFiles(files: File[]) {
  for (const f of files) {
    if (f.type.startsWith('image/')) await addImageFile(f)
    else await addTextFile(f)
  }
}

function removeAttachment(id: string) {
  attachments.value = attachments.value.filter(a => a.id !== id)
}

/** 入口一：选择文件（点击 + 触发隐藏 input） */
function pickFiles() {
  fileInputEl.value?.click()
}

/** 「+」菜单项：上传附件（关闭菜单后打开文件选择） */
function pickFilesFromMenu() {
  addOpen.value = false
  pickFiles()
}

/** 「规划模式」菜单项激活态（与发送时强制规划的判据同源：PLAN_CMD_RE） */
const planArmed = computed(() => PLAN_CMD_RE.test(draft.value))

/** 「+」菜单项：规划模式（等价 /plan 命令：补前缀并聚焦，交由既有强制规划路径处理） */
function applyPlanCommand() {
  addOpen.value = false
  forcedVia = 'menu'
  if (!planArmed.value) draft.value = `/plan ${draft.value}`
  nextTick(() => {
    const el = inputRef.value
    if (!el) return
    el.focus()
    el.setSelectionRange(el.value.length, el.value.length)
  })
}

function onFilePicked(e: Event) {
  const input = e.target as HTMLInputElement
  const files = Array.from(input.files ?? [])
  input.value = '' // 清空以允许重复选择同一文件
  void addFiles(files)
}

/** 入口二：粘贴（剪贴板截图/图片）；命中图片时阻止二进制粘入文本域 */
function onPaste(e: ClipboardEvent) {
  const items = e.clipboardData?.items
  if (!items) return
  const files: File[] = []
  for (const it of items) {
    if (it.kind === 'file' && it.type.startsWith('image/')) {
      const f = it.getAsFile()
      if (f) files.push(f)
    }
  }
  if (files.length) {
    e.preventDefault()
    void addFiles(files)
  }
}

/** 入口三：拖拽（dragover 进出需计数，避免子元素间抖动导致高亮闪烁） */
let dragDepth = 0
function onDragEnter(e: DragEvent) {
  if (!e.dataTransfer?.types?.includes('Files')) return
  dragDepth++
  dragActive.value = true
}
function onDragLeave() {
  dragDepth = Math.max(0, dragDepth - 1)
  if (dragDepth === 0) dragActive.value = false
}
function onDragOver(e: DragEvent) {
  if (e.dataTransfer?.types?.includes('Files')) e.preventDefault()
}
function onDrop(e: DragEvent) {
  dragDepth = 0
  dragActive.value = false
  // 只收图片与白名单文本/代码文件，其余静默忽略（拖拽常混入无关文件）
  const files = Array.from(e.dataTransfer?.files ?? []).filter(
    f => f.type.startsWith('image/') || isAllowedTextFile(f),
  )
  if (files.length) void addFiles(files)
}

/** 多行文本域自增高（1~6 行，超出内部滚动） */
function autoGrow() {
  const el = inputRef.value
  if (!el) return
  el.style.height = 'auto'
  el.style.height = `${Math.min(el.scrollHeight, COMPOSER_MAX_HEIGHT)}px`
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

    <!-- 消息列表：chat-wrap 全宽容器（刻度导轨定位基准），chat-scroll 760px 限宽居中（原布局） -->
    <div ref="chatWrapEl" class="chat-wrap">
      <div ref="scrollEl" class="chat-scroll" @scroll="onChatScroll">
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
        <template v-for="(m, i) in messages" :key="m.id">
          <!-- 用户消息（data-mid 为会话大纲定位锚点） -->
          <div v-if="m.role === 'user'" class="bubble user" :data-mid="m.id">
            <!-- 图片附件（多模态输入）：缩略图行 -->
            <div v-if="m.images?.length" class="bubble-images">
              <img
                v-for="img in m.images"
                :key="img.id"
                :src="img.dataUrl"
                :alt="img.name"
                :title="img.name"
                class="bubble-thumb"
              />
            </div>
            <!-- 文本/代码文件附件：文件名 chip 行 -->
            <div v-if="m.files?.length" class="bubble-files">
              <span v-for="f in m.files" :key="f.id" class="bubble-file" :title="f.name">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                     stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
                  <polyline points="14 2 14 8 20 8" />
                </svg>
                <span class="bubble-file-name">{{ f.name }}</span>
              </span>
            </div>
            <div v-if="m.content" class="bubble-body">{{ m.content }}</div>
            <div v-show="!streaming" class="bubble-foot">
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
          </div>

          <!-- AI 回复组：同一问题触发的连续 assistant 消息（工具循环中间轮 / 计划各步 /
               收尾总结）合并渲染为一个回复——整组仅组首渲染，执行过程统一收进可折叠区域 -->
          <div
            v-else-if="isGroupLead(i)"
            class="bubble assistant"
            :class="groupStatus(i)"
          >
            <!-- AI 消息头：项目 logo + Agent 标识 -->
            <div class="msg-head">
              <span class="agent-logo">
                <AppLogo />
              </span>
              <span class="agent-name">Agent</span>
            </div>
            <!-- 任务耗时行：整组耗时；点击展开/收起执行过程（无过程则不可点） -->
            <div
              class="task-meta"
              :class="{ clickable: hasTrace(i) }"
              @click="hasTrace(i) && toggleTrace(i)"
            >
              <svg class="task-clock" width="11" height="11" viewBox="0 0 24 24" fill="none"
                   stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <circle cx="12" cy="12" r="9" />
                <polyline points="12 7 12 12 15 14" />
              </svg>
              <span class="task-elapsed">任务耗时 {{ fmtDuration(groupElapsedMs(i)) }}</span>
              <span v-if="groupActive(i)" class="task-running">执行中…</span>
              <span v-if="hasTrace(i)" class="trace-arrow" :class="{ open: isTraceOpen(i) }">›</span>
            </div>
            <!-- 执行过程折叠区（v2）：按轮次分组（一条 assistant 消息 = 一轮），
                 结果确认前一切轮次内容（含流式中的当前轮正文）只渲染在此区内 -->
            <div v-show="isTraceOpen(i)" class="trace">
              <template v-for="r in groupRounds(i)" :key="r.key">
                <!-- Plan 步骤分隔条：标识该轮所属执行步骤 -->
                <div v-if="r.stepStart" class="round-sep">
                  <span class="round-sep-num">步骤 {{ (r.stepIndex ?? 0) + 1 }}/{{ groupPlan(i)?.steps.length }}</span>
                  <span class="round-sep-title">{{ groupPlan(i)?.steps[r.stepIndex ?? 0]?.title }}</span>
                </div>
                <!-- 思考过程（每轮独立折叠）：自动 = 思考中展开+脉冲、正文到达收起；手动点击优先 -->
                <div v-if="r.m.reasoning" class="trace-block">
                  <div class="trace-label reasoning-toggle" @click="toggleReasoning(r.m)">
                    <span>思考过程</span>
                    <span v-if="reasoningThinking(r.m)" class="think-pulse">思考中…</span>
                    <span class="trace-arrow" :class="{ open: isReasoningOpen(r.m) }">›</span>
                  </div>
                  <div v-show="isReasoningOpen(r.m)" class="trace-body">
                    <AiMarkdown :content="r.m.reasoning ?? ''" />
                  </div>
                </div>
                <div v-if="r.m.trace" class="trace-block">
                  <div class="trace-label">规划</div>
                  <pre class="trace-pre">{{ r.m.trace }}</pre>
                </div>
                <!-- 轮次正文：结果确认后末轮正文晋升至结果区，此处不再渲染（防同文两处同显） -->
                <div v-if="r.m.content.trim() && !r.promoted" class="trace-block">
                  <div class="trace-body">
                    <AiMarkdown :content="r.m.content" />
                  </div>
                </div>
                <div v-if="r.m.toolProposals?.length" class="trace-block">
                  <!-- 工具提案记录条：
                       - propose_command：命令提案卡片（授权/拒绝/执行状态徽标）
                       - edit_file / write_file：文件编辑卡片（授权/执行状态徽标 + 变更预览）
                       - propose_plan：计划提案卡片（步骤列表只读留痕，决策由计划面板承载）
                       - 只读工具：轻量观察卡片（无授权交互，可折叠输出） -->
                  <div
                    v-for="p in r.m.toolProposals"
                    :key="p.id"
                    class="proposal"
                    :class="[p.state, { 'readonly-tool': !['propose_command','edit_file','write_file','sftp_upload_file','sftp_download_file','propose_plan','conclude_plan'].includes(p.name) }]"
                  >
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
                    <!-- 计划提案：步骤编号列表（决策由计划面板承载，此处只读留痕） -->
                    <template v-else-if="p.name === 'propose_plan'">
                      <div class="proposal-head">
                        <span class="proposal-tag">计划提案</span>
                        <span class="proposal-state">
                          {{ p.state === 'pending'
                            ? '等待确认'
                            : p.state === 'accepted' ? '已确认' : p.state === 'rejected' ? '未采用' : '已失效' }}
                        </span>
                      </div>
                      <ol class="plan-proposal-steps">
                        <li v-for="(s, si) in planProposalSteps(p)" :key="si">{{ s.title }}</li>
                      </ol>
                      <div v-if="p.error" class="proposal-error">{{ p.error }}</div>
                    </template>
                    <!-- 提前收敛提案：剩余步骤已无必要，本步收尾并直接进入结果归纳 -->
                    <template v-else-if="p.name === 'conclude_plan'">
                      <div class="proposal-head">
                        <span class="proposal-tag">
                          {{ parseProposalArgs(p.args)?.outcome === 'blocked' ? '任务阻塞' : '提前收敛' }}
                        </span>
                        <span class="proposal-state">
                          {{ p.state === 'pending'
                            ? '处理中'
                            : p.state === 'accepted' ? '已收敛' : p.state === 'rejected' ? '未采用' : '已失效' }}
                        </span>
                      </div>
                      <div v-if="p.reason" class="proposal-reason">{{ p.reason }}</div>
                      <div v-if="p.error" class="proposal-error">{{ p.error }}</div>
                    </template>
                    <template v-else-if="p.name === 'sftp_upload_file' || p.name === 'sftp_download_file'">
                      <div class="proposal-head">
                        <span class="proposal-tag">{{ p.name === 'sftp_upload_file' ? '⬆ 上传文件' : '⬇ 下载文件' }}</span>
                        <span class="proposal-state">
                          {{ p.state === 'pending'
                            ? '等待授权'
                            : p.state === 'accepted'
                              ? p.error ? '执行失败' : !p.execOutput ? '传输中…' : '已传输'
                              : p.state === 'rejected' ? '已拒绝' : '已失效' }}
                        </span>
                      </div>
                      <div v-if="sftpPaths(p)" class="file-edit-row sftp-paths">
                        <code class="sftp-local">{{ sftpPaths(p)!.local }}</code>
                        <span class="sftp-arrow">→</span>
                        <code class="sftp-remote">{{ sftpPaths(p)!.remote }}</code>
                      </div>
                      <!-- 传输进度条（执行中实时更新；total 未知时不显示） -->
                      <div v-if="p.progress?.total" class="sftp-progress">
                        <div class="sftp-progress-bar">
                          <div
                            class="sftp-progress-fill"
                            :style="{ width: Math.min(100, Math.round((p.progress.transferred / p.progress.total) * 100)) + '%' }"
                          ></div>
                        </div>
                        <span class="sftp-progress-text">
                          {{ humanBytes(p.progress.transferred) }} / {{ humanBytes(p.progress.total) }}
                          <template v-if="p.progress.speed"> · {{ humanBytes(p.progress.speed) }}/s</template>
                        </span>
                      </div>
                      <div v-if="p.error" class="proposal-error">{{ p.error }}</div>
                    </template>
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
                </div>
              </template>
            </div>
            <!-- 结果分隔线：与执行过程区隔开，凸显最终结果（结果确认前恒不出现） -->
            <div v-if="groupResultText(i)" class="result-divider"></div>
            <!-- 最终结果：AI 最终回复（确认晋升 / 总结轮直接流入）；无则计划落定后本地步骤总结 -->
            <AiMarkdown
              v-if="groupResultText(i)"
              :content="groupResultText(i)"
              :terminal-ready="terminalReady"
              @terminal="onTerminalIntent"
            />
            <!-- 计划不进消息流：done/cancelled 由输入框上方悬浮面板展示至用户确认收尾 -->
            <!-- 消息底栏：streaming 期间整体隐藏；hover 淡入。整组仅此一处 -->
            <div v-show="!streaming" class="bubble-foot">
              <button v-if="groupResultText(i)" class="foot-btn" title="复制全文" @click="copyText(groupResultText(i))">
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                     stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                  <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
                  <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
                </svg>
              </button>
              <button
                v-if="groupStatus(i) === 'done' || groupStatus(i) === 'stopped' || groupStatus(i) === 'error'"
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
              <span v-if="groupStatus(i) === 'stopped'" class="stopped-tag">已中止</span>
            </div>
          </div>
        </template>
      </template>
      </div>

      <!-- 会话大纲刻度导轨：轮数 > 3 自动出现（≤3 轮恒不渲染），纯派生无手动开关 -->
      <div
        v-if="showRail"
        class="rail-shell"
        :class="{ 'can-scroll': railCanScroll, 'at-top': railAtTop, 'at-bottom': railAtBottom }"
      >
        <div class="rail-fade top"></div>
        <div ref="railEl" class="rail" @scroll="onRailScroll">
          <button
            v-for="e in outlineEntries"
            :key="e.id"
            type="button"
            class="tick"
            :class="{ past: pastOutlineIds.has(e.id), active: e.id === activeOutlineId }"
            :aria-label="e.title"
            @click="scrollToOutline(e)"
            @mouseenter="showOutlineTip(e, $event)"
            @mouseleave="hideOutlineTip"
            @focus="showOutlineTip(e, $event)"
            @blur="hideOutlineTip"
          ></button>
        </div>
        <div class="rail-fade bottom"></div>
      </div>
    </div>

    <!-- 大纲悬浮标签：fixed 定位 teleport 到 body（刻度无文字，标签是标题的唯一载体） -->
    <Teleport to="body">
      <div
        v-if="outlineTip"
        class="rail-tip"
        :style="{ left: outlineTip.x + 'px', top: outlineTip.y + 'px' }"
      >
        <span class="rail-tip-dot" :class="outlineTip.entry.state"></span>
        <span class="rail-tip-text">{{ outlineTip.entry.title }}</span>
      </div>
    </Teleport>

    <!-- 活动计划面板（M4 Plan-and-Act）：固定在输入框上方，执行进展始终可见；
         可折叠展开；授权弹窗出现时让位隐藏（授权优先，裁决完自动恢复） -->
    <div
      v-if="activePlanMsg?.plan && !authorizing"
      class="plan-panel"
      :class="[activePlanMsg.plan.state, { collapsed: planCollapsed }]"
    >
      <div class="plan-panel-head" @click="planCollapsed = !planCollapsed">
        <span class="plan-badge">
          {{ planBadgeText }}
        </span>
        <span class="plan-goal" :title="planHeadTitle">🎯 {{ planHeadTitle }}</span>
        <span class="plan-progress">
          {{ planProgressText }}
        </span>
        <span class="plan-collapse-arrow" :class="{ open: !planCollapsed }">›</span>
      </div>
      <div v-show="!planCollapsed" class="plan-panel-body">
        <ol class="plan-steps">
          <li
            v-for="(s, si) in activePlanMsg.plan.steps"
            :key="s.id"
            class="plan-step"
            :class="s.state"
          >
            <span class="plan-step-icon">
              <span v-if="s.state === 'running'" class="plan-dot"></span>
              <template v-else>
                {{ s.state === 'done' ? '✓' : s.state === 'failed' ? '✗' : s.state === 'skipped' ? '⊘' : '○' }}
              </template>
            </span>
            <div class="plan-step-body">
              <!-- draft 态：title 可编辑（一句话 todo，v-model 直绑源数据，确认即冻结快照） -->
              <template v-if="activePlanMsg.plan.state === 'draft'">
                <input
                  v-model="s.title"
                  class="plan-step-title-input"
                  type="text"
                  maxlength="50"
                  placeholder="步骤简述"
                />
              </template>
              <template v-else>
                <span class="plan-step-title">{{ si + 1 }}. {{ s.title }}</span>
              </template>
            </div>
          </li>
        </ol>
        <!-- 操作按钮行：draft → 确认执行 / 不用计划直接执行；executing 且有 failed 步 → 失败四按钮 -->
        <div v-if="activePlanMsg.plan.state === 'draft'" class="plan-actions">
          <button
            type="button"
            class="plan-btn primary"
            @click="confirmPlan(activePlanMsg)"
          >
            确认执行
          </button>
          <button type="button" class="plan-btn" @click="refusePlan(activePlanMsg)">
            不用计划，直接执行
          </button>
        </div>
        <div
          v-else-if="activePlanMsg.plan.state === 'executing' && planFailedIndex(activePlanMsg.plan) >= 0"
          class="plan-actions"
        >
          <button
            type="button"
            class="plan-btn primary"
            @click="retryPlanStep(activePlanMsg, planFailedIndex(activePlanMsg.plan))"
          >
            重试
          </button>
          <button
            type="button"
            class="plan-btn"
            @click="skipPlanStep(activePlanMsg, planFailedIndex(activePlanMsg.plan))"
          >
            跳过
          </button>
          <button
            type="button"
            class="plan-btn danger"
            @click="abortPlan(activePlanMsg)"
          >
            终止
          </button>
          <button
            type="button"
            class="plan-btn"
            @click="replan(activePlanMsg, planFailedIndex(activePlanMsg.plan))"
          >
            重新规划
          </button>
        </div>
        <!-- done（提前收敛）→ 确认收尾：面板保留展示各步最终状态（✓/⊘/✗），用户确认后消失；
             cancelled → 确认收尾：面板消失（普通 done 自动消失，由变更文件列表面板接替） -->
        <div
          v-else-if="activePlanMsg.plan.state === 'done' || activePlanMsg.plan.state === 'cancelled'"
          class="plan-actions"
        >
          <button
            type="button"
            class="plan-btn primary"
            @click="acknowledgePlan(activePlanMsg)"
          >
            确认
          </button>
        </div>
      </div>
    </div>

    <!-- 计划完成后的变更文件列表面板（接替计划面板）：左侧文件名，右侧 +n/-n；
         可折叠，用户确认后消失；授权弹窗出现时让位隐藏 -->
    <div
      v-if="planFilesMsg && !authorizing"
      class="plan-panel done files-panel"
      :class="{ collapsed: planFilesCollapsed }"
    >
      <div class="plan-panel-head" @click="planFilesCollapsed = !planFilesCollapsed">
        <span class="plan-badge">已完成</span>
        <span class="plan-goal">变更文件</span>
        <span class="plan-progress">{{ planFiles.length }} 个文件</span>
        <span class="plan-collapse-arrow" :class="{ open: !planFilesCollapsed }">›</span>
      </div>
      <div v-show="!planFilesCollapsed" class="plan-panel-body">
        <ul class="files-list">
          <li v-for="f in planFiles" :key="f.path" class="files-row">
            <span class="files-path" :title="f.path">{{ f.path }}</span>
            <span class="files-diff">
              <span v-if="f.added > 0" class="diff-add">+{{ f.added }}</span>
              <span v-if="f.removed > 0" class="diff-del">-{{ f.removed }}</span>
            </span>
          </li>
        </ul>
        <div class="plan-actions">
          <button
            type="button"
            class="plan-btn primary"
            @click="acknowledgePlanFiles(planFilesMsg)"
          >
            确认
          </button>
        </div>
      </div>
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
        ⚠ {{ authorizing.proposal.name === 'propose_command'
          ? '是否允许执行这个命令？'
          : authorizing.proposal.name === 'sftp_upload_file' || authorizing.proposal.name === 'sftp_download_file'
            ? '是否允许传输这个文件？'
            : '是否允许修改这个文件？' }}
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
      <!-- sftp_upload_file / sftp_download_file：路径对展示 -->
      <template v-else-if="authorizing.proposal.name === 'sftp_upload_file' || authorizing.proposal.name === 'sftp_download_file'">
        <p class="auth-sub">
          AI 请求{{ authorizing.proposal.name === 'sftp_upload_file' ? '将本机文件上传到' : '下载远端文件到本机' }}当前 SSH 会话对应服务器
        </p>
        <p class="auth-sub file-edit-line" v-if="sftpPaths(authorizing.proposal)">
          <code class="sftp-local">{{ sftpPaths(authorizing.proposal)!.local }}</code>
          <span class="sftp-arrow">→</span>
          <code class="sftp-remote">{{ sftpPaths(authorizing.proposal)!.remote }}</code>
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

    <!-- 输入区（composer）：大圆角容器；顶部上下文 chip 行 + 多行文本域 + 底部工具行。
         粘贴 / 拖拽附件在此容器内捕获 -->
    <div
      class="composer"
      :class="{ focus: composerFocused, dragging: dragActive }"
      @paste="onPaste"
      @dragenter="onDragEnter"
      @dragover="onDragOver"
      @dragleave="onDragLeave"
      @drop.prevent="onDrop"
    >
      <!-- 上下文 chip 行：终端引用 + 附件（图片缩略图 / 文件图标） -->
      <div v-if="quote || attachments.length" class="ctx-strip">
        <div v-if="quote" class="ctx-chip quote-chip">
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor"
               stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="4 17 10 11 4 5" />
            <line x1="12" y1="19" x2="20" y2="19" />
          </svg>
          <span class="ctx-label">终端引用 · {{ quote.lines }} 行</span>
          <span class="ctx-preview">{{ quote.text.split('\n')[0] }}</span>
          <button class="ctx-remove" title="移除引用" @click="quote = null">✕</button>
        </div>
        <div
          v-for="a in attachments"
          :key="a.id"
          class="ctx-chip"
          :class="a.kind === 'text' ? 'file-chip' : 'img-chip'"
          :title="a.name"
        >
          <img v-if="a.kind === 'image'" class="ctx-thumb" :src="a.dataUrl" :alt="a.name" />
          <svg v-else width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor"
               stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
            <polyline points="14 2 14 8 20 8" />
          </svg>
          <span class="ctx-preview">{{ a.name }}</span>
          <button class="ctx-remove" :title="a.kind === 'text' ? '移除文件' : '移除图片'" @click="removeAttachment(a.id)">✕</button>
        </div>
      </div>

      <!-- 多行文本域：Enter 发送、Shift+Enter 换行；1~6 行自增高后内部滚动 -->
      <textarea
        ref="inputRef"
        v-model="draft"
        class="composer-input"
        rows="1"
        :disabled="!!configHint"
        :placeholder="configHint || (appView === 'home' ? '询问 AI…' : '询问 AI（携带当前会话上下文）…')"
        @keydown="onKeydown"
        @focus="composerFocused = true"
        @blur="composerFocused = false"
      ></textarea>

      <!-- 底部工具行：左侧 + 更多（上传附件 / 规划模式）/ 权限档，右侧发送（绿色圆形）/ 停止 -->
      <div class="composer-bar">
        <div ref="addWrapRef" class="add-wrap">
          <button
            type="button"
            class="icon-btn"
            title="更多"
            :disabled="!!configHint || streaming"
            @click="addOpen = !addOpen"
          >
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <line x1="12" y1="5" x2="12" y2="19" />
              <line x1="5" y1="12" x2="19" y2="12" />
            </svg>
          </button>
          <div v-if="addOpen" class="add-menu">
            <button type="button" class="add-item" @click="pickFilesFromMenu">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                   stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
                <polyline points="17 8 12 3 7 8" />
                <line x1="12" y1="3" x2="12" y2="15" />
              </svg>
              <span class="add-item-label">上传附件</span>
            </button>
            <button
              type="button"
              class="add-item"
              :class="{ active: planArmed }"
              :disabled="scope === 'global'"
              :title="scope === 'global' ? '全局对话无终端可执行计划' : '规划模式：先生成计划，确认后执行'"
              @click="applyPlanCommand"
            >
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                   stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M9 11l3 3L22 4" />
                <path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11" />
              </svg>
              <span class="add-item-label">规划模式</span>
              <span v-if="planArmed" class="add-check">✓</span>
            </button>
          </div>
        </div>
        <!-- agent 执行权限档（仅工作台 scope）：自定义下拉，每档带图标 + 标签/描述两行；
             manual 弱化默认，risky/full 变色提醒放权态 -->
        <div
          v-if="appView === 'workbench'"
          ref="permWrapRef"
          class="perm-wrap"
          :class="{ elevated: execPermission !== 'manual' }"
        >
          <button
            type="button"
            class="perm-trigger"
            title="命令执行授权档：手动审批 / 低危自动 / 完全访问"
            @click="permOpen = !permOpen"
          >
            <svg
              width="12"
              height="12"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <path
                d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"
              />
              <path v-if="execPermission === 'risky'" d="m9 12 2 2 4-4" />
              <path v-else-if="execPermission === 'full'" d="m4 4 16 16" />
            </svg>
            <span>{{ currentPermLabel }}</span>
            <svg
              class="perm-caret"
              width="10"
              height="10"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <path d="m18 15-6-6-6 6" />
            </svg>
          </button>
          <div v-if="permOpen" class="perm-menu">
            <button
              v-for="lv in PERM_LEVELS"
              :key="lv.value"
              type="button"
              class="perm-item"
              :class="{ active: execPermission === lv.value }"
              @click="pickPerm(lv.value)"
            >
              <svg
                width="14"
                height="14"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
              >
                <path
                  d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"
                />
                <path v-if="lv.value === 'risky'" d="m9 12 2 2 4-4" />
                <path v-else-if="lv.value === 'full'" d="m4 4 16 16" />
              </svg>
              <span class="perm-item-text">
                <span class="perm-item-label">{{ lv.label }}</span>
                <span class="perm-item-desc">{{ lv.desc }}</span>
              </span>
            </button>
          </div>
        </div>
        <div class="bar-spacer"></div>
        <button v-if="!streaming" class="send-btn" title="发送 (Enter)" @click="send()">
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor"
               stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
            <line x1="12" y1="19" x2="12" y2="5" />
            <polyline points="5 12 12 5 19 12" />
          </svg>
        </button>
        <button v-else class="stop-btn" title="停止生成" @click="stop">
          <span class="stop-square"></span>
        </button>
        <!-- 隐藏文件选择 input（点击 + 触发）：图片 MIME + 文本/代码扩展名 -->
        <input
          ref="fileInputEl"
          type="file"
          :accept="FILE_ACCEPT"
          multiple
          class="file-hidden"
          @change="onFilePicked"
        />
      </div>
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

/* 消息列表包裹：全宽容器，仅作刻度导轨定位基准（消息区 760px 限宽居中在 .chat-scroll 上） */
.chat-wrap {
  flex: 1;
  min-height: 0;
  width: 100%;
  position: relative;
  display: flex;
}
/* 消息列表（布局同会话大纲引入前：760px 限宽居中、padding 12px；导轨不占消息区空间） */
.chat-scroll {
  flex: 1;
  min-width: 0;
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

/* ---------- 会话大纲刻度导轨 ---------- */
/* 导轨容器：置于消息区（760px 居中）左侧 margin 空隙，右缘距消息区左缘 12px；
   414px = 760/2 + 22 导轨宽 + 12 间距；面板过窄（margin 不足）时由宽度阈值整体隐藏 */
.rail-shell {
  position: absolute;
  left: calc(50% - 414px);
  top: 50%;
  transform: translateY(-50%);
  width: 22px;
  max-height: 215px; /* 10×14 行高 + 9×7 间距 + 12 上下内边距 = 最多 10 个刻度可见 */
  display: flex;
  animation: rail-in 0.16s ease-out;
}
@keyframes rail-in {
  from { opacity: 0; }
  to { opacity: 1; }
}
.rail {
  overflow-y: auto;
  scrollbar-width: none;
  max-height: inherit;
  width: 100%;
  display: flex;
  flex-direction: column;
  gap: 7px;
  padding: 6px 0;
}
.rail::-webkit-scrollbar { display: none; }
/* 上下淡出遮罩：可滚且未贴顶/贴底时提示还能继续滚（与面板背景同色渐变） */
.rail-fade {
  position: absolute;
  left: 0;
  right: 0;
  height: 16px;
  pointer-events: none;
  opacity: 0;
  transition: opacity 0.15s;
  z-index: 1;
}
.rail-fade.top { top: 0; background: linear-gradient(180deg, var(--panel), transparent); }
.rail-fade.bottom { bottom: 0; background: linear-gradient(0deg, var(--panel), transparent); }
.rail-shell.can-scroll:not(.at-top) .rail-fade.top { opacity: 1; }
.rail-shell.can-scroll:not(.at-bottom) .rail-fade.bottom { opacity: 1; }
/* 刻度：行高 14px 扩大 hover/focus 热区（横线自身仅 2~3px，热区不足难以命中） */
.tick {
  height: 14px;
  width: 100%;
  flex: none;
  border: none;
  background: transparent;
  padding: 0;
  cursor: pointer;
  display: flex;
  align-items: center;
}
.tick::before {
  content: '';
  display: block;
  width: 16px;
  height: 2px;
  border-radius: 2px;
  background: var(--border);
  transition:
    width 0.26s cubic-bezier(0.4, 0, 0.2, 1),
    height 0.26s cubic-bezier(0.4, 0, 0.2, 1),
    background 0.26s cubic-bezier(0.4, 0, 0.2, 1);
}
.tick.past::before { background: var(--muted-2); }
.tick:hover::before,
.tick:focus-visible::before {
  width: 20px;
  background: var(--muted);
}
.tick.active::before {
  width: 22px;
  height: 3px;
  background: rgb(var(--accent-rgb));
}
.tick.active:hover::before,
.tick.active:focus-visible::before {
  width: 22px;
  background: rgb(var(--accent-rgb));
}
/* 大纲悬浮标签（teleport 到 body）：状态点 + 标题，单行省略 */
.rail-tip {
  position: fixed;
  transform: translateY(-50%);
  z-index: 1000;
  display: flex;
  align-items: center;
  gap: 7px;
  max-width: 240px;
  padding: 6px 10px;
  background: var(--panel-2);
  border: 1px solid var(--border);
  border-radius: 10px;
  box-shadow: 0 10px 28px rgba(0, 0, 0, 0.35);
  pointer-events: none;
  animation: rail-tip-in 0.16s ease-out;
}
@keyframes rail-tip-in {
  from { opacity: 0; transform: translate(-5px, -50%); }
  to { opacity: 1; transform: translate(0, -50%); }
}
.rail-tip-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  flex: none;
}
.rail-tip-dot.waiting { background: var(--rhost-status-idle); }
.rail-tip-dot.running {
  background: var(--rhost-status-processing);
  animation: reasoning-pulse 1.2s ease-in-out infinite;
}
.rail-tip-dot.done { background: var(--rhost-status-success); }
.rail-tip-dot.error { background: var(--rhost-status-error); }
.rail-tip-dot.stopped { background: var(--rhost-status-warning); }
.rail-tip-text {
  font-size: 12.5px;
  color: var(--text);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
/* 大纲定位：锚点距消息区顶 40px 呼吸位；点击刻度后目标 bubble 闪烁反馈 */
.chat-scroll .bubble.user { scroll-margin-top: 40px; }
.bubble.user.flash .bubble-body,
.bubble.user.flash .bubble-images,
.bubble.user.flash .bubble-files {
  border-radius: 10px;
  animation: outline-flash 1.1s ease-out;
}
@keyframes outline-flash {
  0% { box-shadow: 0 0 0 4px rgba(var(--accent-rgb), 0.2); }
  100% { box-shadow: 0 0 0 8px rgba(var(--accent-rgb), 0); }
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
/* 用户消息图片附件缩略图行 */
.bubble-images {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  justify-content: flex-end;
  margin-bottom: 6px;
}
/* 用户消息文本/代码文件附件行 */
.bubble-files {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  justify-content: flex-end;
  margin-bottom: 6px;
}
.bubble-file {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  max-width: 240px;
  padding: 3px 8px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--panel-2);
  color: var(--muted);
  font-size: 11px;
  font-family: var(--mono);
}
.bubble-file svg {
  flex-shrink: 0;
  color: var(--green);
}
.bubble-file-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.bubble-thumb {
  width: 84px;
  height: 84px;
  border-radius: 8px;
  object-fit: cover;
  border: 1px solid var(--border);
  cursor: zoom-in;
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

/* 计划提案卡片：步骤编号列表（决策在计划面板完成，此处仅留痕） */
.plan-proposal-steps {
  margin: 2px 0 0;
  padding-left: 20px;
  color: var(--text);
  font-size: 11.5px;
  line-height: 1.7;
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
/* SFTP 传输：路径对 + 进度条 */
.sftp-paths {
  flex-wrap: wrap;
  font-size: 11.5px;
}
.sftp-paths code,
.file-edit-line code {
  font-family: var(--mono);
  color: var(--muted);
  word-break: break-all;
}
.sftp-local {
  color: var(--muted);
}
.sftp-remote {
  color: var(--text);
}
.sftp-arrow {
  color: var(--accent);
  flex: none;
}
.sftp-progress {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 5px;
}
.sftp-progress-bar {
  flex: 1;
  height: 4px;
  border-radius: 2px;
  background: var(--border);
  overflow: hidden;
}
.sftp-progress-fill {
  height: 100%;
  border-radius: 2px;
  background: var(--accent);
  transition: width 0.2s ease;
}
.sftp-progress-text {
  flex: none;
  font-size: 10.5px;
  color: var(--muted);
  font-family: var(--mono);
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

/* ---- 执行计划（M4 Plan-and-Act）：悬浮面板 + 步骤列表 ---- */
.plan-badge {
  flex-shrink: 0;
  font-weight: 600;
  color: #569cd6;
}
.plan-goal {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text);
}
.plan-steps {
  margin: 0;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.plan-step {
  display: flex;
  gap: 8px;
  font-size: 11.5px;
  line-height: 1.45;
}
.plan-step-icon {
  flex-shrink: 0;
  width: 14px;
  text-align: center;
  color: var(--muted);
  user-select: none;
}
.plan-step.done .plan-step-icon {
  color: var(--green);
}
.plan-step.failed .plan-step-icon {
  color: #e57a7a;
}
.plan-step.skipped .plan-step-icon {
  color: #e5a13a;
}
/* running 步：蓝色脉冲圆点（与「思考中…」同一脉冲动画） */
.plan-dot {
  display: inline-block;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #569cd6;
  animation: reasoning-pulse 1.2s ease-in-out infinite;
}
.plan-step-body {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.plan-step-title {
  color: var(--text);
  font-weight: 500;
}
.plan-step.running .plan-step-title {
  color: #569cd6;
}
/* draft 态编辑输入框 */
.plan-step-title-input {
  width: 100%;
  box-sizing: border-box;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 4px 8px;
  font-size: 11.5px;
  color: var(--text);
  outline: none;
  font-family: inherit;
}
.plan-step-title-input:focus {
  border-color: #569cd6;
}
/* 操作按钮行（规划面板 / 文件变更面板共用）：按钮统一靠右对齐 */
.plan-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  flex-wrap: wrap;
}
.plan-btn {
  padding: 4px 12px;
  border-radius: 6px;
  border: 1px solid var(--border);
  background: var(--panel);
  color: var(--text);
  font-size: 11.5px;
  cursor: pointer;
  transition: opacity 0.15s;
}
.plan-btn:hover:not(:disabled) {
  opacity: 0.85;
}
.plan-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.plan-btn.primary {
  background: #569cd6;
  border-color: #569cd6;
  color: #0b1118;
  font-weight: 600;
}
.plan-btn.danger {
  border-color: rgba(229, 90, 90, 0.5);
  color: #e57a7a;
}

/* ---- 活动计划面板：固定输入框上方，可折叠，授权弹窗出现时让位（v-if 隐藏） ---- */
.plan-panel {
  flex-shrink: 0;
  /* 与输入框同宽居中；上与消息区、下与输入框各留 8px 空白（下方由 composer 上 margin 叠加） */
  width: calc(100% - 20px);
  max-width: 740px;
  box-sizing: border-box;
  margin: 8px auto 0;
  border: 1px solid rgba(86, 156, 214, 0.4);
  border-radius: 10px;
  background: var(--panel);
  box-shadow: 0 -2px 12px rgba(0, 0, 0, 0.15);
  overflow: hidden;
}
.plan-panel-head {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  font-size: 11px;
  cursor: pointer;
  user-select: none;
}
.plan-panel-head .plan-badge {
  animation: reasoning-pulse 1.2s ease-in-out infinite;
}
.plan-panel.draft .plan-badge,
.plan-panel.done .plan-badge,
.plan-panel.cancelled .plan-badge {
  animation: none;
}
.plan-panel.done .plan-badge {
  color: var(--green);
}
.plan-panel.cancelled .plan-badge {
  color: var(--muted);
}
.plan-progress {
  margin-left: auto;
  flex-shrink: 0;
  color: var(--muted);
}
.plan-collapse-arrow {
  flex-shrink: 0;
  color: var(--muted-2);
  font-size: 14px;
  line-height: 1;
  transform: rotate(0deg); /* 折叠态箭头向右 */
  transition: transform 0.15s;
}
.plan-collapse-arrow.open {
  transform: rotate(90deg); /* 展开态箭头向下 */
}
.plan-panel-body {
  padding: 2px 12px 10px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

/* ---- 变更文件列表面板：左侧路径（超长省略），右侧 +n/-n ---- */
.files-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.files-row {
  display: flex;
  align-items: center;
  gap: 10px;
  font-size: 12px;
}
.files-path {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-family: var(--mono);
  color: var(--text);
}
.files-diff {
  flex: none;
  display: flex;
  gap: 6px;
  font-size: 11px;
  font-family: var(--mono);
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

@keyframes reasoning-pulse {
  0%, 100% { opacity: 0.4; }
  50% { opacity: 1; }
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

/* 输入区（composer）：大圆角容器，含上下文 chip 行 + 多行文本域 + 底部工具行 */
.composer {
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 8px 10px 6px;
  margin: 8px auto;
  border: 1px solid var(--border);
  border-radius: 14px;
  background: var(--panel-2);
  transition: border-color 0.15s, box-shadow 0.15s;
  /* 与消息列表同宽居中 */
  width: calc(100% - 20px);
  max-width: 740px;
  box-sizing: border-box;
}
.composer.focus {
  border-color: var(--green);
}
/* 拖拽悬停：高亮提示可放置图片 */
.composer.dragging {
  border-color: var(--green);
  box-shadow: 0 0 0 2px rgba(61, 220, 132, 0.25);
}

/* 上下文 chip 行：终端引用 + 图片附件（有则显示，可换行） */
.ctx-strip {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}
.ctx-chip {
  display: flex;
  align-items: center;
  gap: 6px;
  max-width: 240px;
  padding: 3px 6px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--panel);
  color: var(--muted-2);
  font-size: 11px;
}
.ctx-chip svg {
  color: var(--green);
  flex-shrink: 0;
}
.ctx-label {
  flex-shrink: 0;
  color: var(--green);
}
.ctx-preview {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 10.5px;
}
.quote-chip .ctx-preview,
.file-chip .ctx-preview {
  font-family: var(--mono);
}
.ctx-thumb {
  width: 22px;
  height: 22px;
  border-radius: 4px;
  object-fit: cover;
  flex-shrink: 0;
}
.ctx-remove {
  flex-shrink: 0;
  border: none;
  background: transparent;
  color: var(--muted-2);
  cursor: pointer;
  font-size: 11px;
  padding: 1px 4px;
  border-radius: 4px;
}
.ctx-remove:hover {
  color: var(--text);
  background: rgba(255, 255, 255, 0.06);
}

/* 多行文本域：无边框融入 composer，1~6 行自增高后内部滚动 */
.composer-input {
  width: 100%;
  box-sizing: border-box;
  resize: none;
  border: none;
  background: transparent;
  padding: 2px 2px;
  font-size: 12px;
  line-height: 1.45;
  font-family: inherit;
  color: var(--text);
  outline: none;
  max-height: 132px;
  overflow-y: auto;
}
.composer-input::placeholder {
  color: var(--muted-2);
}
.composer-input:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

/* 底部工具行 */
.composer-bar {
  display: flex;
  align-items: center;
  gap: 6px;
}
.bar-spacer {
  flex: 1;
}
/* ---- 回复组头部：任务耗时行 + 执行过程折叠区 ---- */
/* 任务耗时行：整组耗时，可点击展开执行过程 */
.task-meta {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 8px;
  font-size: 11px;
  color: var(--muted);
  user-select: none;
}
.task-meta.clickable {
  cursor: pointer;
}
.task-meta.clickable:hover {
  color: var(--text);
}
.task-clock {
  flex-shrink: 0;
  opacity: 0.7;
}
.task-elapsed {
  font-family: var(--mono, ui-monospace, SFMono-Regular, Menlo, monospace);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}
.task-running {
  color: var(--accent);
  animation: reasoning-pulse 1.2s ease-in-out infinite;
}
.trace-arrow {
  font-size: 13px;
  line-height: 1;
  color: var(--muted-2);
  transition: transform 0.15s;
}
.trace-arrow.open {
  transform: rotate(90deg);
}
/* 执行过程折叠区：左侧竖线串联思考/规划/工具记录 */
.trace {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin: 0 0 8px;
  padding-left: 9px;
  border-left: 2px solid var(--border);
}
.trace-block {
  display: flex;
  flex-direction: column;
  gap: 3px;
  font-size: 11px;
}
.trace-label {
  font-size: 10.5px;
  font-weight: 600;
  color: var(--muted-2);
  letter-spacing: 0.02em;
}
.trace-body {
  color: var(--muted);
}
/* Plan 步骤分隔条：执行过程区内标识该组轮次所属的执行步骤 */
.round-sep {
  display: flex;
  align-items: baseline;
  gap: 6px;
  margin: 2px 0;
  font-size: 10.5px;
  color: var(--muted-2);
}
.round-sep-num {
  flex-shrink: 0;
  font-weight: 600;
  color: var(--muted);
}
.round-sep-title {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
/* 每轮思考过程折叠标签：可点击切换（手动优先于自动展开/收起） */
.reasoning-toggle {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  cursor: pointer;
  user-select: none;
}
.reasoning-toggle:hover {
  color: var(--text);
}
/* 思考中脉冲标签（该轮 reasoning 流式且正文未到达时的自动展开态标识） */
.think-pulse {
  color: var(--accent);
  animation: reasoning-pulse 1.2s ease-in-out infinite;
}
.trace-pre {
  margin: 0;
  padding: 6px 8px;
  border-radius: 6px;
  background: var(--panel-2);
  color: var(--muted);
  font-family: var(--mono);
  font-size: 11px;
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-word;
  max-height: 260px;
  overflow: auto;
}
/* 结果分隔线：隔开执行过程与最终结果 */
.result-divider {
  height: 1px;
  background: var(--border);
  margin: 0 0 8px;
}
/* 附件（+）图标按钮 */
.icon-btn {
  flex: none;
  width: 26px;
  height: 26px;
  border-radius: 7px;
  display: grid;
  place-items: center;
  background: transparent;
  border: none;
  color: var(--muted);
  cursor: pointer;
  transition: background 0.15s, color 0.15s;
}
.icon-btn:hover:not(:disabled) {
  background: rgba(255, 255, 255, 0.06);
  color: var(--text);
}
.icon-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
/* 输入区「+」下拉：触发按钮复用 .icon-btn，菜单上弹（与权限档菜单同风格） */
.add-wrap {
  flex: none;
  position: relative;
  display: flex;
  align-items: center;
}
.add-menu {
  position: absolute;
  bottom: calc(100% + 6px);
  left: 0;
  min-width: 168px;
  width: max-content;
  background: var(--panel-2);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 4px;
  display: flex;
  flex-direction: column;
  gap: 2px;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.35);
  z-index: 1000;
}
.add-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 8px;
  border: none;
  border-radius: 8px;
  background: transparent;
  color: var(--text);
  font-size: 12px;
  cursor: pointer;
  text-align: left;
  white-space: nowrap;
}
.add-item:hover:not(:disabled) {
  background: var(--hover);
}
.add-item:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.add-item svg {
  flex: none;
  color: var(--muted);
}
.add-item.active,
.add-item.active svg {
  color: #569cd6;
}
.add-check {
  margin-left: auto;
  color: #569cd6;
}
/* 隐藏的文件选择 input */
.file-hidden {
  display: none;
}
/* agent 执行权限档下拉：触发按钮弱化融入输入区，放权档黄色警示；菜单上弹 */
.perm-wrap {
  flex: none;
  position: relative;
  display: flex;
  align-items: center;
  color: var(--muted);
}
.perm-wrap.elevated {
  color: var(--yellow);
}
.perm-trigger {
  display: flex;
  align-items: center;
  gap: 4px;
  background: var(--panel-2);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 4px 8px;
  font-size: 11px;
  color: inherit;
  outline: none;
  cursor: pointer;
}
.perm-wrap.elevated .perm-trigger {
  border-color: var(--yellow);
}
.perm-caret {
  margin-left: 2px;
  opacity: 0.6;
}
.perm-menu {
  position: absolute;
  bottom: calc(100% + 6px);
  left: 0;
  min-width: 210px;
  width: max-content;
  background: var(--panel-2);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 4px;
  display: flex;
  flex-direction: column;
  gap: 2px;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.35);
  z-index: 1000;
}
.perm-item {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  padding: 6px 8px;
  border: none;
  border-radius: 8px;
  background: transparent;
  color: var(--text);
  cursor: pointer;
  text-align: left;
}
.perm-item:hover {
  background: var(--hover);
}
.perm-item.active {
  color: var(--yellow);
}
.perm-item svg {
  flex: none;
  margin-top: 1px;
}
.perm-item-text {
  display: flex;
  flex-direction: column;
  gap: 1px;
}
.perm-item-label {
  font-size: 12px;
  line-height: 1.3;
}
.perm-item-desc {
  font-size: 11px;
  line-height: 1.3;
  color: var(--muted);
  white-space: nowrap;
}
.send-btn {
  width: 28px;
  height: 28px;
  border-radius: 50%;
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
