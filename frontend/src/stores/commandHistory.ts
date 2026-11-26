/**
 * 历史命令（会话级、内存态）。
 *
 * 采集点在 session.ts 的写入函数：把用户敲入终端的原始字节拼成「回车提交的那一行」。
 * 不落盘，关闭应用即清空，避免敏感命令写入磁盘。
 */
import { ref, watch } from 'vue'
import { savedSettings } from './settings'

export interface CommandHistoryItem {
  /** 条目唯一 id（进程内自增；时间戳同毫秒会撞车，不能作为 key/删除依据） */
  id: number
  /** 回车提交时的命令行原文 */
  command: string
  /** 提交时间（毫秒时间戳） */
  timestamp: number
}

/** 进程内自增 id：跨会话唯一，避免同毫秒提交产生重复 key */
let nextId = 0

/** 单会话上限的允许区间：设置值被手改/旧版本缺失时兜底，避免 0 或超大值 */
const MIN_PER_SESSION = 10
const MAX_PER_SESSION_CAP = 500
const DEFAULT_PER_SESSION = 50

/** 当前生效的每会话上限：取设置项 commandHistoryLimit 并夹取到允许区间 */
function perSessionLimit(): number {
  const v = Number(savedSettings.commandHistoryLimit)
  if (!Number.isFinite(v)) return DEFAULT_PER_SESSION
  return Math.min(MAX_PER_SESSION_CAP, Math.max(MIN_PER_SESSION, Math.floor(v)))
}

/** 全部会话合计最大条数：多开标签时防止历史总量无限膨胀（列表全量渲染，需要硬上限） */
const MAX_TOTAL = 2000

/** 单条命令最大字符数：超长粘贴（大段 base64/文本）只保留前 N 字符，避免单条大文本拖慢内存与渲染 */
const MAX_COMMAND_CHARS = 4096

/** 会话 id → 历史命令（最新在前） */
export const commandHistory = ref<Record<string, CommandHistoryItem[]>>({})

/** 设置项变更后立即收敛现有列表，不必等下一次命令提交 */
watch(perSessionLimit, limit => {
  let changed = false
  const next: Record<string, CommandHistoryItem[]> = {}
  for (const [sessionId, list] of Object.entries(commandHistory.value)) {
    if (list.length > limit) {
      next[sessionId] = list.slice(0, limit)
      changed = true
    } else {
      next[sessionId] = list
    }
  }
  if (changed) commandHistory.value = trimTotal(next)
})

/** 每会话的行缓冲；reliable=false 表示该行被 ESC 序列/控制字符污染，
 *  光标位置已无法还原，回车时整行丢弃，避免把残片当成命令 */
const lineBuf = new Map<string, { text: string; reliable: boolean }>()

/** 采集写入终端的原始字节（键盘、粘贴、片段执行、AI 提案共用）。
 *  只处理可打印字符与退格；方向键回溯、Tab 补全等会重绘行的操作标记为不可靠。 */
export function recordTerminalInput(sessionId: string, data: string | Uint8Array) {
  const text = typeof data === 'string' ? data : new TextDecoder().decode(data)
  if (!text) return
  const state = lineBuf.get(sessionId) ?? { text: '', reliable: true }
  for (const ch of text) {
    const code = ch.codePointAt(0) ?? 0
    if (ch === '\r' || ch === '\n') {
      const command = state.text.trim()
      if (command && state.reliable) push(sessionId, command)
      state.text = ''
      state.reliable = true
    } else if (ch === '\x7f' || ch === '\b') {
      state.text = state.text.slice(0, -1)
    } else if (ch === '\x03' || ch === '\x15') {
      // Ctrl+C / Ctrl+U：丢弃当前行
      state.text = ''
      state.reliable = true
    } else if (ch === '\x1b' || code < 0x20) {
      // ESC 序列（方向键/Home/End）与其它控制字符：无法跟踪光标
      state.reliable = false
    } else {
      state.text += ch
    }
  }
  lineBuf.set(sessionId, state)
}

function push(sessionId: string, command: string) {
  const list = commandHistory.value[sessionId] ?? []
  const text = command.length > MAX_COMMAND_CHARS
    ? `${command.slice(0, MAX_COMMAND_CHARS)}…`
    : command
  const item: CommandHistoryItem = { id: ++nextId, command: text, timestamp: Date.now() }
  const next = { ...commandHistory.value, [sessionId]: [item, ...list].slice(0, perSessionLimit()) }
  commandHistory.value = trimTotal(next)
}

/** 总条数超限时按 id 从最旧开始淘汰；数组尾部即最旧，逐条删除直到达标 */
function trimTotal(history: Record<string, CommandHistoryItem[]>): Record<string, CommandHistoryItem[]> {
  let total = 0
  for (const list of Object.values(history)) total += list.length
  if (total <= MAX_TOTAL) return history
  const next = { ...history }
  while (total > MAX_TOTAL) {
    let oldestSid: string | null = null
    let oldestId = Number.POSITIVE_INFINITY
    for (const [sid, list] of Object.entries(next)) {
      const last = list[list.length - 1]
      if (last && last.id < oldestId) {
        oldestId = last.id
        oldestSid = sid
      }
    }
    if (!oldestSid) break
    const trimmed = next[oldestSid]!.slice(0, -1)
    if (trimmed.length) next[oldestSid] = trimmed
    else delete next[oldestSid]
    total--
  }
  return next
}

/** 删除单条（按自增 id，同毫秒多条也只会删中目标） */
export function removeCommand(sessionId: string, id: number) {
  const list = commandHistory.value[sessionId]
  if (!list) return
  commandHistory.value = {
    ...commandHistory.value,
    [sessionId]: list.filter(i => i.id !== id),
  }
}

/** 清空某会话历史；不传 sessionId 时清空全部 */
export function clearCommandHistory(sessionId?: string) {
  if (sessionId) {
    const next = { ...commandHistory.value }
    delete next[sessionId]
    commandHistory.value = next
    lineBuf.delete(sessionId)
  } else {
    commandHistory.value = {}
    lineBuf.clear()
  }
}
