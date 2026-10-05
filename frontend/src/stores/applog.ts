/**
 * 应用日志（App Log）前端 store：模块级单例。
 *
 * 数据源：后端 Hub 经 subscribe_app_logs 推送（replay + 增量），
 * 面板显示 = 磁盘文件内容（同一结构化模型，前端仅做纯文本格式化，§5.2）。
 *
 * - `logs` 为响应式数组，DockPanel 日志 tab 直接绑定；
 * - 行数上限读 savedSettings.logMaxLines（截头保留最新）；
 * - 前端事件经 report() 上报后端统一打 seq/ts（fire-and-forget）；
 * - 非 Tauri 环境（浏览器 dev）降级 console，不拉取订阅。
 */
import { ref } from 'vue'
import { invoke, Channel } from '@tauri-apps/api/core'
import { isTauri } from '../lib/tauri'
import { savedSettings } from './settings'

/** 面板行级别（与现有 DockPanel .log-row 样式类对齐） */
export type LogLevel = 'INFO' | 'WARN' | 'ERROR' | 'DEBUG'

/** 面板行模型（渲染友好：time 已格式化、msg 含 kv 文本） */
export interface PanelLogEntry {
  /** 全局唯一 seq（v-for key） */
  id: number
  /** 显示时间：HH:MM:SS（当天）/ MM-DD HH:MM:SS（跨天） */
  time: string
  level: LogLevel
  /** 消息 + kv 纯文本（[target] [sid] 前缀已拼入） */
  msg: string
}

/** 后端推送的结构化条目（与文件 JSONL 同构） */
interface AppLogEntry {
  seq: number
  ts: string
  level: string
  target: string
  event_id: string
  sid?: string
  msg: string
  kv?: Record<string, unknown>
}

interface LogBatch {
  entries: AppLogEntry[]
  lost_because?: string
  from_seq?: number
}

/* ---- web.* 域事件常量（与后端 applog/events.rs 同步） ---- */
export const WEB_IPC_ERROR = 'web.ipc.error'
export const WEB_UNHANDLED_ERROR = 'web.unhandled_error'
/** SFTP 传输事件（前端队列侧上报） */
export const SFTP_TRANSFER_ENQUEUE = 'sftp.transfer.enqueue'
export const SFTP_TRANSFER_START = 'sftp.transfer.start'
export const SFTP_TRANSFER_COMPLETE = 'sftp.transfer.complete'
export const SFTP_TRANSFER_FAILED = 'sftp.transfer.failed'
export const SFTP_TRANSFER_CANCEL = 'sftp.transfer.cancel'
export const SFTP_TRANSFER_PAUSE = 'sftp.transfer.pause'
export const SFTP_TRANSFER_RESUME = 'sftp.transfer.resume'
export const SFTP_MANAGE_REMOVE = 'sftp.manage.remove'

/** 面板日志数组（模块级单例） */
export const logs = ref<PanelLogEntry[]>([])

/** 最后一条 seq（断点续传游标） */
let lastSeq = 0
let subscribed = false

/** 级别小写 → 面板级别 */
function toPanelLevel(level: string): LogLevel {
  switch (level) {
    case 'error': return 'ERROR'
    case 'warn': return 'WARN'
    case 'debug': return 'DEBUG'
    default: return 'INFO'
  }
}

/** ISO 8601 ts → 显示时间（当天 HH:MM:SS / 跨天 MM-DD HH:MM:SS） */
function formatTime(ts: string): string {
  const d = new Date(ts)
  if (isNaN(d.getTime())) return ts
  const pad = (n: number) => String(n).padStart(2, '0')
  const hms = `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
  const now = new Date()
  const sameDay = d.getFullYear() === now.getFullYear()
    && d.getMonth() === now.getMonth() && d.getDate() === now.getDate()
  return sameDay ? hms : `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${hms}`
}

/**
 * 字节语义的 kv 键：面板显示时转为人类可读单位（B/KB/MB/GB）。
 * 注意：仅面板显示层转换，JSONL 文件内仍为原始字节数，便于 jq 精确过滤与计算。
 */
const BYTE_KEYS = new Set(['size', 'total', 'transferred', 'resume_from', 'bytes', 'written'])
/** 速度键：kv 值单位为 B/s，显示时转可读速率并把键名简化为 speed */
const SPEED_BPS_KEYS = new Set(['speed_bps'])
/** 百分比键：0-100 数值，显示追加 % */
const PERCENT_KEYS = new Set(['pct'])

/** 字节数 → 紧凑可读文本（无空格，避免触发 kv 引号规则），如 524684643 → 500.37MB */
function humanizeBytes(n: number): string {
  if (!Number.isFinite(n)) return String(n)
  if (n < 1024) return `${Math.round(n)}B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let v = n / 1024
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  // 大数少给小数位，避免噪声：≥100 取整，≥10 一位，其余两位
  const digits = v >= 100 ? 0 : v >= 10 ? 1 : 2
  return `${v.toFixed(digits)}${units[i]}`
}

/** kv 对象 → `key=value` 空格分隔文本（含空格/特殊字符加引号） */
function formatKv(kv?: Record<string, unknown>): string {
  if (!kv) return ''
  return Object.entries(kv)
    .map(([k, v]) => {
      let displayKey = k
      let s: string
      if (typeof v === 'number' && Number.isFinite(v)) {
        if (BYTE_KEYS.has(k)) {
          s = humanizeBytes(v)
        } else if (SPEED_BPS_KEYS.has(k)) {
          displayKey = 'speed'
          s = `${humanizeBytes(v)}/s`
        } else if (PERCENT_KEYS.has(k)) {
          s = `${Math.round(v)}%`
        } else {
          s = String(v)
        }
      } else {
        s = String(v)
      }
      return /[\s"]/.test(s) ? `${displayKey}="${s.replace(/"/g, '\\"')}"` : `${displayKey}=${s}`
    })
    .join(' ')
}

/** 结构化条目 → 面板行（§5.2 纯文本格式） */
function toPanelEntry(e: AppLogEntry): PanelLogEntry {
  const kvText = formatKv(e.kv)
  const parts = [`[${e.target}]`]
  if (e.sid) parts.push(`[${e.sid}]`)
  parts.push(e.msg)
  if (kvText) parts.push(kvText)
  let msg = parts.join(' ')
  // 面板单条上限 2KB
  if (msg.length > 2048) msg = msg.slice(0, 2048) + '…[truncated]'
  return { id: e.seq, time: formatTime(e.ts), level: toPanelLevel(e.level), msg }
}

/** 截头保留最新 logMaxLines 行 */
function trim() {
  const max = savedSettings.logMaxLines || 5000
  if (logs.value.length > max) logs.value.splice(0, logs.value.length - max)
}

/** 订阅后端日志流（Workbench 挂载时调用一次，幂等） */
export function subscribe() {
  if (!isTauri || subscribed) return
  subscribed = true
  const channel = new Channel<LogBatch>()
  channel.onmessage = (batch) => {
    if (batch.lost_because) {
      // 客户端游标已被逐出缓冲：提示部分更早日志丢失
      logs.value.push({
        id: (batch.from_seq ?? lastSeq) - 1,
        time: '',
        level: 'WARN',
        msg: `⚠ 部分更早日志已从内存缓冲区丢弃（lost_because=${batch.lost_because}）`,
      })
    }
    for (const e of batch.entries) {
      logs.value.push(toPanelEntry(e))
      lastSeq = Math.max(lastSeq, e.seq)
    }
    trim()
  }
  invoke('subscribe_app_logs', { channel, sinceId: lastSeq > 0 ? lastSeq : null })
    .catch((e) => console.warn('subscribe_app_logs 失败:', e))
}

/** 清空日志（面板「清空日志」按钮）：后端清内存缓冲 + 前端清显示 */
export async function clear() {
  logs.value = []
  if (!isTauri) return
  try {
    await invoke('clear_app_log_buffer')
  } catch { /* 忽略 */ }
}

/** 前端事件上报（fire-and-forget；level 小写；eventId 必须属允许域） */
export function report(level: 'debug' | 'info' | 'warn' | 'error', eventId: string, msg: string, kv?: Record<string, unknown>) {
  if (!isTauri) {
    console.log(`[${level}] [${eventId}] ${msg}`, kv ?? '')
    return
  }
  invoke('report_app_log', { input: { level, eventId, msg, kv: kv ?? null } })
    .catch(() => { /* 上报失败静默：避免日志路径再产日志 */ })
}

/** 打开日志目录（系统文件管理器，当前实例实际写入目录） */
export async function revealLogDir() {
  if (!isTauri) return
  try {
    await invoke('reveal_log_dir')
  } catch { /* 忽略 */ }
}

/**
 * 打开设置面板配置的日志存储路径：空串回退当前生效目录；
 * 目录不存在时后端自动创建（便于确认未重启生效的目标落点）。
 * 返回实际打开的绝对路径；失败抛出错误供调用方提示。
 */
export async function revealLogStorageDir(path: string): Promise<string> {
  return invoke<string>('reveal_log_storage_dir', { path })
}

let errorReportingInit = false

/** 全局错误上报：window error + unhandledrejection → web.unhandled_error。
 *  应用挂载时调用一次（幂等）；堆栈截断 500 字符防超长。 */
export function initGlobalErrorReporting() {
  if (errorReportingInit) return
  errorReportingInit = true
  const cut = (s: string) => (s.length > 500 ? s.slice(0, 500) + '…' : s)
  window.addEventListener('error', (e) => {
    report('error', WEB_UNHANDLED_ERROR, e.message || '未知脚本错误', {
      source: e.filename ? `${e.filename}:${e.lineno}:${e.colno}` : '',
      stack: e.error?.stack ? cut(e.error.stack) : '',
    })
  })
  window.addEventListener('unhandledrejection', (e) => {
    const reason = e.reason
    const msg = reason instanceof Error ? reason.message : String(reason)
    report('error', WEB_UNHANDLED_ERROR, `未处理的 Promise 拒绝: ${msg}`, {
      stack: reason instanceof Error && reason.stack ? cut(reason.stack) : '',
    })
  })
}

/** 同步日志配置到后端（saveSettings 后调用） */
export function syncLogConfig() {
  if (!isTauri) return
  const s = savedSettings
  invoke('set_log_config', {
    config: {
      collect: s.logCollect,
      level: s.logLevel,
      maxLines: s.logMaxLines,
      persist: s.logPersist,
      storagePath: s.logStoragePath,
      rotate: s.logRotate,
      maxFiles: s.logMaxFiles,
      retentionDays: s.logRetentionDays,
    },
  }).catch((e) => console.warn('set_log_config 失败:', e))
}
