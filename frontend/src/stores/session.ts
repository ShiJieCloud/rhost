import { computed, reactive, ref, shallowRef, triggerRef, watch } from 'vue'
import { Channel, invoke } from '@tauri-apps/api/core'
import type { Host } from '../types'
import { hosts } from './hosts'
import { savedSettings } from './settings'
import { isTauri } from '../lib/tauri'
import { getSnapshot, onConfigLoad, patchUiState } from './appConfig'
import { promptPassword } from '../composables/usePasswordPrompt'

export type SessionState = 'idle' | 'connecting' | 'online' | 'reconnecting' | 'offline'
export type AppView = 'home' | 'workbench'

export interface Session {
  /** 前端本地会话 id（uuid，非主机 id；同一主机复用同一会话） */
  id: string
  host: Host
  state: SessionState
  startedAt: number
  /** 后端 SSH 会话 id（connect_ssh 返回的 uuid）；未连接/已断开为 undefined */
  backendId?: string
  /** 断线/失败原因（仅离线/重连浮层展示，绝不写入终端缓冲以保留断线前画面）；在线为 '' */
  disconnectReason: string
  /** 下次自动重连尝试的 epoch 毫秒（浮层倒计时用）；无等待为 null */
  retryAt: number | null
  /** 建连成功序号：每次成功自增，TerminalPane 监听后 term.reset() 清空旧画面，
   *  让全新 PTY 从干净 shell 提示符开始 */
  resetSeq: number
  /** 标签自定义别名；空字符串表示无别名，UI 回落展示 host.id */
  alias: string
}

export const appView = ref<AppView>('home')
export const sessions = ref<Session[]>([])
export const activeSessionId = ref<string | null>(null)

export const activeSession = computed(
  () => sessions.value.find(s => s.id === activeSessionId.value) ?? null,
)
export const activeHost = computed(() => activeSession.value?.host ?? null)
export const connected = computed(() => activeSession.value?.state === 'online')

/* ---- Dock / 面板 UI 状态 ---- */
export const sidebarVisible = ref(true)

/** Dock 展开高度（px）；折叠态下该值保留，展开时无缝恢复 */
export const dockHeight = ref(286)
export const DOCK_DEFAULT_HEIGHT = 286

/* ---- 工作区布局持久化：app_config.json ui_state 节（全局使用习惯，不绑会话） ---- */

const num = (v: unknown) => (typeof v === 'number' && Number.isFinite(v) ? v : null)
const bool = (v: unknown, dflt: boolean) => (typeof v === 'boolean' ? v : dflt)

export const inspectorVisible = ref(true)
export const dockCollapsed = ref(false)
export const dockVisible = ref(true)
/** Dock 当前激活页签 */
export const dockTab = ref<'sftp' | 'log'>('sftp')
/** SFTP 双栏左侧（本地）宽度占比；夹取 0.2~0.8 */
export const sftpLocalRatio = ref(0.5)
/** SFTP 传输队列展开高度（px）；折叠态下保留，展开无缝恢复 */
export const sftpQueueHeight = ref(180)
/** SFTP 传输队列折叠状态；初始折叠，用户调整后跨重启保留 */
export const sftpQueueCollapsed = ref(true)

/** hydrate 当次的批量赋值不回写（watch flush sync 同步拦截） */
let layoutHydrated = false

onConfigLoad(snap => {
  const ui = snap.uiState
  layoutHydrated = false
  const savedDockH = num(ui.dockHeight)
  if (savedDockH !== null) dockHeight.value = savedDockH // 视口级 clamp 由 DockPanel 挂载时完成
  inspectorVisible.value = bool(ui.inspectorVisible, true)
  dockCollapsed.value = bool(ui.dockCollapsed, false)
  dockVisible.value = bool(ui.dockVisible, true)
  dockTab.value = ui.dockTab === 'log' || ui.dockTab === 'sftp' ? ui.dockTab : 'sftp'
  const savedRatio = num(ui.sftpLocalRatio)
  sftpLocalRatio.value =
    savedRatio !== null ? Math.min(0.8, Math.max(0.2, savedRatio)) : 0.5
  // 队列高度只做异常值防呆夹取（28 为拖拽折叠残留值，展开时组件恢复默认）；
  // 视口级适配由 DockPanel 布局自然约束
  const savedQueueH = num(ui.sftpQueueHeight)
  sftpQueueHeight.value =
    savedQueueH !== null ? Math.min(720, Math.max(28, savedQueueH)) : 180
  sftpQueueCollapsed.value = bool(ui.sftpQueueCollapsed, true)
  layoutHydrated = true
})

/** 状态变化经 ui_state 合并写落盘（patchUiState 内部 300ms 防抖合并连续变更） */
watch(
  [inspectorVisible, dockVisible, dockCollapsed, dockTab, dockHeight, sftpLocalRatio,
   sftpQueueHeight, sftpQueueCollapsed],
  () => {
    if (!isTauri || !layoutHydrated) return
    patchUiState({
      inspectorVisible: inspectorVisible.value,
      dockVisible: dockVisible.value,
      dockCollapsed: dockCollapsed.value,
      dockTab: dockTab.value,
      dockHeight: dockHeight.value,
      sftpLocalRatio: sftpLocalRatio.value,
      sftpQueueHeight: sftpQueueHeight.value,
      sftpQueueCollapsed: sftpQueueCollapsed.value,
    })
  },
  { flush: 'sync' },
)

/** 打开底部面板（取消折叠并显示，可指定页签） */
export function openDock(tab?: 'sftp' | 'log') {
  dockCollapsed.value = false
  dockVisible.value = true
  if (tab) dockTab.value = tab
}

export function toggleDock() {
  dockVisible.value = !dockVisible.value
}

/** 关闭底部面板（隐藏，可通过顶部工具栏重新打开） */
export function closeDock() {
  dockVisible.value = false
}

/** TerminalPane 监听此计数触发重连 */
export const reconnectTick = ref(0)

/** DockPanel 监听此计数唤起日志搜索 */
export const searchTick = ref(0)

/* ============================================================
 * 二进制帧协议（与后端 ssh/frame.rs 对应）
 * 1 字节类型 + 4 字节大端长度 + payload
 * ============================================================ */
const FRAME_DATA = 0x01 // PTY 原始输出
const FRAME_EXIT = 0x02 // 会话结束（payload 为可选原因文本）
const FRAME_ERROR = 0x03 // 会话内部错误
const FRAME_MOTD = 0x04 // Rhost MOTD 指令数组（JSON，先于一切 PTY 数据到达）
const FRAME_HOST_INFO = 0x05 // 主机静态信息（JSON HostInfo，连接时一次，先于 PTY 数据）
const FRAME_METRICS = 0x06 // 主机动态指标（JSON Metrics，周期推送）
const FRAME_RTT = 0x07     // 链路 RTT（JSON {"ms": 23}，建连即测 + 30s 周期）
const FRAME_CWD = 0x08     // Shell 当前工作目录（绝对路径文本，PTY cd 后后端上报）
const FRAME_ALGO = 0x09    // SSH 协商算法（JSON AlgoInfo，建连时一次，先于 PTY 数据）

/** Exit 帧 payload 解析结果：reason 为展示文本，lost=true 表示连接意外丢失（可自动重连） */
interface ExitInfo {
  reason: string
  lost: boolean
}

/** 解析 0x02 Exit 帧。新后端 payload 为 JSON {"reason","lost"}；
 *  兼容旧版纯文本协议：空文本（Eof/Close）视为连接丢失，非空文本（退出状态/信号）视为正常退出 */
function parseExit(payload: Uint8Array): ExitInfo {
  const text = textDecoder.decode(payload)
  if (text.startsWith('{')) {
    try {
      const o = JSON.parse(text) as { reason?: unknown; lost?: unknown }
      return {
        reason: typeof o.reason === 'string' ? o.reason : '',
        lost: o.lost === true,
      }
    } catch {
      // 损坏 JSON 按文本规则回退
    }
  }
  return { reason: text, lost: text === '' }
}

/** 主机静态信息（与后端 metrics::HostInfo 字段对应，snake_case 保紧凑） */
export interface HostInfoData {
  os: string
  distro: string
  kernel: string
  arch: string
  cpu_model: string
  cores: number
  timezone: string
  uptime: string
  iface: string
  ip: string
  procfs: boolean
}

/** 每会话静态主机信息（连接到达后存入，关闭会话时清理；断线保留最后一帧） */
const hostInfoMap = reactive(new Map<string, HostInfoData>())
export const activeHostInfo = computed<HostInfoData | null>(
  () => (activeSessionId.value && hostInfoMap.get(activeSessionId.value)) || null,
)

/** SSH 握手真实协商算法（与后端 Algo 帧 JSON 对应，snake_case） */
export interface AlgoInfoData {
  /** 服务器主机密钥算法标准名（如 ssh-ed25519） */
  host_key: string
  /** 对称加密算法标准名（如 aes256-gcm@openssh.com） */
  cipher: string
  /** PTY 终端类型（我方申请值，如 xterm-256color） */
  term: string
  /** 终端字符编码（xterm.js 恒为 UTF-8） */
  enc: string
}

/** 每会话协商算法（建连时一帧；独立 Map 存放，避免污染连接持久化字段） */
const algoMap = reactive(new Map<string, AlgoInfoData>())
export const activeAlgo = computed<AlgoInfoData | null>(
  () => (activeSessionId.value && algoMap.get(activeSessionId.value)) || null,
)

/** 活动会话当前终端尺寸（列×行，xterm fit 实时更新；未挂载终端为 null） */
export const activeTermSize = computed<{ cols: number; rows: number } | null>(
  () =>
    (activeSessionId.value && termSizeMap.get(activeSessionId.value)) || null,
)

/** CPU 动态值（与后端 CpuMetricsPayload 对应） */
export interface CpuMetricsData {
  /** 总体使用率 0~100 */
  util: number
  /** 1/5/15 分钟平均负载（运行队列长度，非百分比）；无法采集为 null */
  load: [number, number, number] | null
}

/** 内存与 Swap 动态值（字节，与后端 MemMetrics 对应） */
export interface MemMetricsData {
  total: number
  available: number
  used: number
  /** Swap 总量；为 0 表示主机无 swap，前端隐藏 Swap 区块 */
  swap_total: number
  swap_free: number
  swap_used: number
}

/** 网络动态值（与后端 NetMetricsPayload 对应；速率 bytes/s，计数为累计值） */
export interface NetMetricsData {
  /** 接收速率 bytes/s */
  rx_rate: number
  /** 发送速率 bytes/s */
  tx_rate: number
  /** 累计接收字节 */
  rx_bytes: number
  /** 累计发送字节 */
  tx_bytes: number
}

/** 单个挂载点磁盘用量（字节，与后端 DiskMetrics 对应） */
export interface DiskMetricsData {
  /** 挂载点 */
  mount: string
  /** 总容量 */
  total: number
  /** 已用 */
  used: number
  /** 可用 */
  available: number
  /** 使用率 0~100 */
  pct: number
}

/** 单个 Top 进程（与后端 ProcItemPayload 对应） */
export interface ProcMetricsData {
  pid: number
  /** 进程名 comm（已去括号） */
  name: string
  /** 区间 CPU 占用 %（单核封顶 100，多核可超） */
  cpu: number
  /** 常驻内存字节（RSS） */
  rss: number
}

/** 进程区：全量进程数 + CPU Top 5 */
export interface ProcsMetricsData {
  /** 主机当前进程总数 */
  total: number
  top: ProcMetricsData[]
}

/** 主机动态指标（0x06 帧；cpu.util/load + mem/swap + net + disks + procs + gpus） */
export interface MetricsData {
  /** 后端采集序号，从 1 递增 */
  seq: number
  cpu: CpuMetricsData
  mem: MemMetricsData
  net: NetMetricsData
  /** 真实挂载点磁盘用量（后端低频夹带，非夹带轮沿用上一次结果） */
  disks: DiskMetricsData[]
  /** 进程总数与 CPU Top 5 */
  procs: ProcsMetricsData
  /** GPU 卡列表（无 GPU 主机为空数组，侧栏区块隐藏） */
  gpus: GpuMetricsData[]
}

/** 每会话最新一帧动态指标。shallowRef：每帧整体替换 value 字段，
 *  Map 按前端会话 id 隔离；关闭会话时清理，断线保留最后一帧 */
const metricsMap = shallowRef(new Map<string, MetricsData>())
/** 每会话 Shell 当前工作目录（由 0x08 帧推送，SFTP 面板据此同步） */
const sftpCwdMap = shallowRef(new Map<string, string>())
export const activeMetrics = computed<MetricsData | null>(
  () => (activeSessionId.value && metricsMap.value.get(activeSessionId.value)) || null,
)
/** 活动会话的 Shell CWD（由 0x08 帧驱动），SFTP 面板 watch 它实现 Shell→SFTP 同步 */
export const activeSftpCwd = computed<string | null>(
  () => (activeSessionId.value && sftpCwdMap.value.get(activeSessionId.value)) || null,
)

/** sparkline 采样点数（与旧右侧栏网络曲线一致） */
const NET_SPARK_LEN = 28
/** 每会话网络速率历史（bytes/s），随 0x06 帧推进；关闭会话时清理，断线冻结在最后画面 */
const netHistMap = shallowRef(new Map<string, { up: number[]; down: number[] }>())
const EMPTY_NET_HIST = {
  up: new Array(NET_SPARK_LEN).fill(0) as number[],
  down: new Array(NET_SPARK_LEN).fill(0) as number[],
}
export const activeNetHist = computed<{ up: number[]; down: number[] }>(() => {
  const id = activeSessionId.value
  return (id && netHistMap.value.get(id)) || EMPTY_NET_HIST
})

const textEncoder = new TextEncoder()
const textDecoder = new TextDecoder()

/* ============================================================
 * MOTD 前端渲染：后端只下发结构化指令 [{t,text,cls}]，
 * 配色取 xterm 主题同色值（与 TerminalPane 主题一致）。
 * 反引号 `...` 段切换链接高亮色，反引号本身不显示。
 * ============================================================ */
interface TermCmd {
  t: string
  text?: string
  cls?: string
}
/** cls → ANSI SGR 前景色（truecolor，对齐 TerminalPane 主题） */
const MOTD_SGR: Record<string, string> = {
  brand: '38;2;57;197;207', // #39c5cf cyan
  green: '38;2;63;185;80', // #3fb950
  warn: '38;2;210;153;34', // #d29922
  err: '38;2;248;81;73', // #f85149
  cmd: '38;2;230;237;243', // #e6edf3
  white: '38;2;230;237;243',
  author: '1;38;2;242;193;78', // #f2c14e 金色 + 加粗，重点标识作者行
}
const MOTD_SGR_DIM = '38;2;110;118;129' // #6e7681
const MOTD_SGR_LINK = '38;2;79;214;224' // #4fd6e0

/** 把 Motd 帧 payload 渲染为带 ANSI 配色的终端字节（首尾各留一空行） */
function renderMotd(payload: Uint8Array): Uint8Array {
  let cmds: TermCmd[]
  try {
    cmds = JSON.parse(textDecoder.decode(payload)) as TermCmd[]
  } catch {
    return new Uint8Array(0)
  }
  if (!Array.isArray(cmds)) return new Uint8Array(0)

  let out = '\r\n'
  for (const cmd of cmds) {
    if (cmd.t !== 'print') continue
    const sgr = MOTD_SGR[cmd.cls ?? ''] ?? MOTD_SGR_DIM
    out += `\x1b[${sgr}m`
    const segs = (cmd.text ?? '').split('`')
    segs.forEach((seg, i) => {
      if (i % 2 === 1) {
        out += `\x1b[${MOTD_SGR_LINK}m${seg}\x1b[${sgr}m`
      } else {
        out += seg
      }
    })
    out += '\x1b[0m\r\n'
  }
  out += '\r\n'
  return textEncoder.encode(out)
}

/** 终端写入回调：TerminalPane 为每个会话注册，数据最终进 xterm.write */
type TermSink = (data: Uint8Array) => void
const termSinks = new Map<string, TermSink>()
/** sink 注册前到达的帧先排队，注册后补发（避免丢掉连接初期的欢迎输出） */
const pendingFrames = new Map<string, Uint8Array[]>()
/** 每个会话的 PTY 尺寸（重连时复用） */
const termSizes = new Map<string, { cols: number; rows: number }>()
/** 同上的响应式副本：供状态栏实时显示当前终端列×行（xterm fit 即更新） */
const termSizeMap = reactive(new Map<string, { cols: number; rows: number }>())

function deliver(id: string, payload: Uint8Array) {
  const sink = termSinks.get(id)
  if (sink) {
    sink(payload)
    return
  }
  const q = pendingFrames.get(id) ?? []
  q.push(payload)
  pendingFrames.set(id, q)
}

/** Tauri Channel 收到的 Vec<u8> 可能表现为 number[] / ArrayBuffer / Uint8Array，统一转换 */
function toU8(raw: unknown): Uint8Array {
  if (raw instanceof Uint8Array) return raw
  if (raw instanceof ArrayBuffer) return new Uint8Array(raw)
  return new Uint8Array(raw as number[])
}

/** 解析一帧并分发：DATA → xterm；EXIT/ERROR → 状态流转 + 终端提示 */
function handleFrame(id: string, frame: Uint8Array) {
  if (frame.length < 5) return
  const type = frame[0]!
  const len = ((frame[1]! << 24) | (frame[2]! << 16) | (frame[3]! << 8) | frame[4]!) >>> 0
  const payload = frame.subarray(5, Math.min(5 + len, frame.length))

  if (type === FRAME_DATA) {
    deliver(id, payload)
    return
  }
  if (type === FRAME_MOTD) {
    // MOTD 首帧：本地渲染后写入终端（此时 PTY 数据尚未到达，严格先于 PS1）
    const bytes = renderMotd(payload)
    if (bytes.length) deliver(id, bytes)
    return
  }
  if (type === FRAME_HOST_INFO) {
    // 主机静态信息：存入按会话隔离的 map，供右侧栏 Inspector 读取
    try {
      const info = JSON.parse(textDecoder.decode(payload)) as HostInfoData
      hostInfoMap.set(id, info)
    } catch {
      // 单帧解析失败不影响终端主流程
    }
    return
  }
  if (type === FRAME_ALGO) {
    // 协商算法：状态栏显示真实主机密钥/加密/终端类型/编码
    try {
      const a = JSON.parse(textDecoder.decode(payload)) as AlgoInfoData
      algoMap.set(id, a)
    } catch {
      // 单帧解析失败不影响终端主流程
    }
    return
  }
  if (type === FRAME_METRICS) {
    // 动态指标：按会话隔离存入最新一帧（CPU/内存/网络/...）
    try {
      const m = JSON.parse(textDecoder.decode(payload)) as MetricsData
      metricsMap.value.set(id, m)
      triggerRef(metricsMap)
      // 同步推进该会话的网络速率 sparkline（bytes/s；旧后端无 net 字段时跳过）
      if (m.net) {
        let hist = netHistMap.value.get(id)
        if (!hist) {
          hist = {
            up: new Array(NET_SPARK_LEN).fill(0),
            down: new Array(NET_SPARK_LEN).fill(0),
          }
          netHistMap.value.set(id, hist)
        }
        hist.up.push(m.net.tx_rate)
        hist.up.shift()
        hist.down.push(m.net.rx_rate)
        hist.down.shift()
        triggerRef(netHistMap)
      }
    } catch {
      // 单帧解析失败不影响终端主流程
    }
    return
  }
  if (type === FRAME_RTT) {
    // 链路 RTT：直写 host.lat 运行时字段。不走 updateHost——lat 不入持久化，
    // 避免 30s 周期触发一次 save_connection 写盘；s.host 是 hosts 数组项引用，
    // 改属性即响应式生效（状态栏/Inspector 消费同一字段）
    try {
      const { ms } = JSON.parse(textDecoder.decode(payload)) as { ms: number }
      const s = sessions.value.find(x => x.id === id)
      if (s && typeof ms === 'number') s.host.lat = Math.round(ms)
    } catch {
      // 单帧解析失败不影响终端主流程
    }
    return
  }
  if (type === FRAME_CWD) {
    // Shell CWD 变更：存入按会话隔离的 map，SFTP 面板 watch activeSftpCwd 后同步目录
    const path = textDecoder.decode(payload)
    if (path) {
      sftpCwdMap.value.set(id, path)
      triggerRef(sftpCwdMap)
    }
    return
  }
  if (type === FRAME_EXIT) {
    const s = sessions.value.find(x => x.id === id)
    if (s) s.backendId = undefined
    // 重连途中旧会话的 EXIT：静默收尾，不写关闭提示、不覆盖 reconnecting 状态
    if (s?.state === 'reconnecting') return
    // 用户手动断开后在途残留的 EXIT：忽略，避免重复提示与误触发自动重连
    if (manualClosed.has(id)) return
    const { reason, lost } = parseExit(payload)
    // 不向终端写任何字节：保留断线瞬间的屏幕快照（含 Vim alt buffer 画面），
    // 断线信息只进浮层字段
    if (s) s.retryAt = null
    setSessionState(id, 'offline')
    if (s) {
      s.disconnectReason = lost
        ? '连接已中断（远端无响应）'
        : (reason || '远端会话已结束')
    }
    // 仅连接意外丢失（无远端退出状态）时自动重连；用户主动 exit 不重连
    if (lost) scheduleAutoReconnect(id)
    return
  }
  if (type === FRAME_ERROR) {
    const msg = textDecoder.decode(payload)
    setSessionState(id, 'offline')
    const s = sessions.value.find(x => x.id === id)
    if (s) {
      s.retryAt = null
      s.disconnectReason = `连接错误：${msg}`
    }
    if (!manualClosed.has(id)) scheduleAutoReconnect(id)
  }
}

/** 解析登录密码：优先从系统钥匙串读取，缺失时弹专用密码框询问，
 *  用户输入后回写钥匙串（下次免询问）。Tauri 下 window.prompt 不可用。 */
async function resolvePassword(host: Host): Promise<string> {
  // 本次会话内已缓存的密码直接用（避免重连反复查钥匙串）
  if (host.password) return host.password

  // Tauri 环境：尝试从系统钥匙串读取
  if (isTauri) {
    try {
      const stored = await invoke<string | null>('get_connection_password', { id: host.id })
      if (stored) {
        host.password = stored
        return stored
      }
    } catch (e) {
      console.warn('get_connection_password 失败，回退到弹框询问:', e)
    }
  }

  // 钥匙串无密码 → 弹框询问
  const pw = await promptPassword(`输入 ${host.user}@${host.ip} 的登录密码：`)
  if (pw != null && pw !== '') {
    host.password = pw
    // 回写钥匙串，下次连接免询问
    if (isTauri) {
      invoke('save_connection_password', { id: host.id, password: pw }).catch(e =>
        console.error('save_connection_password 失败:', e),
      )
    }
  }
  return pw ?? ''
}

/**
 * 彩色提示符初始化（完全符合透传架构，前端零参与）。
 *
 * 后端经独立 exec 通道把脚本静默写入远端 /tmp 唯一临时文件，PTY 开启后
 * 立即经写队列注入 source 命令（tty 缓冲暂存，shell 就绪即执行）；
 * 初始化期间的 PTY 输出被后端 hold 住，直到脚本末尾输出不可见 OSC marker
 * 才放行——回显行、ANSI 清行、着色 PS1 在同一渲染帧到达 xterm，
 * 用户看到的第一个画面就是最终态，无任何闪烁/切换。
 * /tmp 不可写或 3s 未见 marker 时后端自动降级为直通。
 *
 * - 提示符仍完全由远端 shell 的 PS1/PROMPT 生成，Rhost 不构造、不拼接、不解析输出流；
 * - 远端已有彩色提示符（oh-my-zsh、p10k 等）时脚本自动跳过，不覆盖用户配置；
 * - 行首空格配合常见 HISTCONTROL=ignorespace 配置使其不进 bash 历史；
 * - fish 非 POSIX 语法不兼容（会显示一行错误），可在全局设置关闭彩色提示符。
 */
interface ConnectResult {
  sessionId: string
}

/** 建立后端 SSH 连接：创建独立 Channel，invoke connect_ssh，帧流进 xterm */
async function connectBackend(s: Session, cols: number, rows: number) {
  if (!isTauri) {
    deliver(s.id, textEncoder.encode('\x1b[33m[dev] 浏览器模式无 Tauri 后端，无法建立 SSH 连接\x1b[0m\r\n'))
    setSessionState(s.id, 'offline')
    return
  }
  setSessionState(s.id, s.state === 'reconnecting' ? 'reconnecting' : 'connecting')
  // 连接代次 +1：本次 invoke 在途期间若用户断开/再次重连，代次会失配
  const gen = (connectGen.get(s.id) ?? 0) + 1
  connectGen.set(s.id, gen)

  // 每会话独立 Channel：二进制帧直推，不经过 Emitter/JSON。
  // 代次门：旧连接的 Channel 不会被自动关闭，快速连续重连/取消后旧通道帧
  // 仍可能到达——非当前代次一律丢弃，防止两条 PTY 流混写同一终端、
  // 或孤儿输出污染断线快照
  const channel = new Channel<number[] | ArrayBuffer | Uint8Array>()
  channel.onmessage = raw => {
    if (gen !== connectGen.get(s.id)) return
    handleFrame(s.id, toU8(raw))
  }

  try {
    // 密钥认证：keyPath 存在时跳过密码询问，凭据由一键连接门禁阶段收集并暂存在 host 上
    const keyPath = s.host.keyPath
    const res = await invoke<ConnectResult>('connect_ssh', {
      payload: {
        host: s.host.ip,
        port: s.host.port,
        username: s.host.user,
        password: keyPath ? undefined : await resolvePassword(s.host),
        keyPath,
        passphrase: keyPath ? s.host.keyPassphrase : undefined,
        cols,
        rows,
        // 连接成功后由后端采集服务器状态并绘制 Rhost MOTD 欢迎面板；
        // 开启时后端同时走 exec 路径抑制 sshd 原生 MOTD/Last login
        motd: savedSettings.motd,
        // 自定义 MOTD ASCII LOGO：开关关闭（或文本为空）传空串，后端显示内置 LOGO
        motdLogo: savedSettings.motdLogoOn ? savedSettings.motdLogo : '',
        // 彩色提示符：后端在 PTY 开启后自动注入并 hold 初始化输出至脚本完成，
        // 前端首帧即着色 PS1 的最终画面，无需任何时序编排
        colorPrompt: savedSettings.colorPrompt,
        // 环境变量：随 init 脚本 export 注入远端交互 shell（绕过 sshd AcceptEnv 限制）
        env: savedSettings.env.map(e => [e.key, e.value]),
      },
      channel,
    })
    // 代次失配：等待期间用户已断开或发起了更新的连接，本次结果作废。
    // 后端会话已建立但前端不再需要——主动断开这个孤儿，避免后台泄漏连接
    if (gen !== connectGen.get(s.id)) {
      void invoke('disconnect_session', { sessionId: res.sessionId }).catch(() => {})
      return
    }
    s.backendId = res.sessionId
    // 建连成功：清除自动重连计数与手动断开标记（首次连接时本就为空，幂等）
    reconnectAttemptMap.delete(s.id)
    manualClosed.delete(s.id)
    // 全新 PTY：丢弃旧连接残帧、清断线信息，并通知 TerminalPane reset 终端——
    // 断线前的屏幕快照（含 Vim alt buffer）在此刻才被清掉，进入干净 shell 提示符
    pendingFrames.delete(s.id)
    s.disconnectReason = ''
    s.retryAt = null
    s.resetSeq++
    setSessionState(s.id, 'online')
  } catch (e) {
    // 代次失配：失败已无关当前状态（用户已断开或更新的连接在进行），静默丢弃
    if (gen !== connectGen.get(s.id)) return
    // 错误信息只进浮层字段，不写入终端缓冲（保留断线前画面）
    s.disconnectReason = `连接失败：${String(e)}`
    s.retryAt = null
    setSessionState(s.id, 'offline')
    // 处于自动重连流程中（attemptMap 有记录）：失败后按退避策略续试；
    // 初次连接/手动重连失败不自动续试，等待用户操作
    if (reconnectAttemptMap.has(s.id)) scheduleAutoReconnect(s.id)
  }
}

/**
 * TerminalPane 在 xterm 实例就绪后调用：
 * 注册写入回调、记录 PTY 尺寸，并在会话尚未连接时发起后端连接
 */
export function attachTerminal(id: string, sink: TermSink, cols: number, rows: number) {
  termSinks.set(id, sink)
  termSizes.set(id, { cols, rows })
  termSizeMap.set(id, { cols, rows })
  const pending = pendingFrames.get(id)
  if (pending?.length) {
    pending.forEach(sink)
    pendingFrames.delete(id)
  }
  const s = sessions.value.find(x => x.id === id)
  if (s && !s.backendId && s.state !== 'online') void connectBackend(s, cols, rows)
}

/** 注销终端数据回调（组件卸载时调用，防止 sink 闭包持有已销毁的 xterm） */
export function detachTerminal(id: string) {
  termSinks.delete(id)
}

/** 前端键盘原始字节写入 PTY（低频控制消息走 invoke/JSON 无妨）。
 *  非 online（断线/重连等待/连接中）一律静默丢弃——输入冻结，
 *  既不下发也不本地回显，终端停留在断线瞬间的静态快照 */
export function sendInput(id: string, data: string | Uint8Array) {
  const s = sessions.value.find(x => x.id === id)
  if (!s?.backendId || !isTauri || s.state !== 'online') return
  const bytes = typeof data === 'string' ? textEncoder.encode(data) : data
  // Tauri 序列化 Vec<u8> 需要普通数组（Uint8Array 会被 JSON 序列化成对象）
  void invoke('write_terminal', { sessionId: s.backendId, data: Array.from(bytes) })
}

/* ============================================================
 * PTY resize：xterm fit → onResize → resize_terminal
 *
 * 时序原则（关键）：
 * 1. leading 立即发送 —— xterm 已同步按新尺寸 reflow，远端必须立刻
 *    收到 window-change 并重绘，否则会出现"新容器 + 旧全屏画面"的错乱；
 * 2. 50ms trailing 合并拖拽期间的高频变化，松手最后一次必达；
 * 3. trailing 期间只要发生过变化（dirty），最终无条件再发一次：
 *    快速"关→开"会让 xterm 多次 reflow，即使最终尺寸 == 初始尺寸，
 *    也要强制远端重绘一次来修复屏幕。
 * ============================================================ */
const RESIZE_TRAIL_MS = 50
/** trailing 定时器（存在即说明处于一次 resize 窗口内） */
const resizeTimers = new Map<string, ReturnType<typeof setTimeout>>()
/** trailing 期间最新待发尺寸 */
const pendingResize = new Map<string, { cols: number; rows: number }>()
/** 上次真正发给后端的尺寸（leading 立即发，保证 xterm 与远端同步） */
const lastSentResize = new Map<string, { cols: number; rows: number }>()

function doResize(id: string, backendId: string, cols: number, rows: number) {
  lastSentResize.set(id, { cols, rows })
  void invoke('resize_terminal', { sessionId: backendId, cols, rows })
}

export function resizeTerminal(id: string, cols: number, rows: number) {
  if (!Number.isInteger(cols) || !Number.isInteger(rows) || cols <= 0 || rows <= 0) return
  // 先更新响应式尺寸：状态栏显示与是否连上后端无关，xterm fit 了就是这个尺寸。
  // termSizes 同步更新——断线期间顶部状态条占位会触发 fit 缩小行数，
  // 重连建连必须按最新尺寸申请 PTY，不能沿用挂载时的旧尺寸
  termSizeMap.set(id, { cols, rows })
  termSizes.set(id, { cols, rows })

  const s = sessions.value.find(x => x.id === id)
  // 后端未连上（backendId 缺失）时无需发送：建连本身就会带上当前尺寸
  if (!s?.backendId || !isTauri) return
  // 与 xterm 当前已发尺寸相同：像素抖动也会触发 fit，但结果没变
  const last = lastSentResize.get(id)
  if (last && last.cols === cols && last.rows === rows && !resizeTimers.has(id)) return

  pendingResize.set(id, { cols, rows })

  // leading：本窗口第一次变化立即发送，远端与 xterm 同步重绘
  if (!resizeTimers.has(id)) {
    doResize(id, s.backendId, cols, rows)
  }

  // trailing：合并后续变化；dirty（有 pending）即最终无条件发送
  const oldTimer = resizeTimers.get(id)
  if (oldTimer) clearTimeout(oldTimer)
  const timer = setTimeout(() => {
    resizeTimers.delete(id)
    const cur = sessions.value.find(x => x.id === id)
    const size = pendingResize.get(id)
    pendingResize.delete(id)
    if (cur?.backendId && size) doResize(id, cur.backendId, size.cols, size.rows)
  }, RESIZE_TRAIL_MS)
  resizeTimers.set(id, timer)
}

/* ============================================================
 * 断线自动重连（指数退避状态机）
 *
 * 单一闸门原则：只有「已建立连接后意外断开」（Exit 帧 lost=true / Error 帧，
 * 或自动重连尝试本身失败）才会调度。用户手动断开、关闭标签、远端主动 exit、
 * 初次连接失败、手动重连失败均不触发。
 *
 * 互斥：每会话最多一个等待定时器（reconnectTimers），重复入口直接忽略；
 * reconnectAttemptMap 的存在同时表示「该会话处于自动重连流程」，
 * connectBackend 失败时据此判断是否续试。
 * ============================================================ */
/** 首次重连等待 1s，逐次翻倍，封顶 30s */
const RECONNECT_BASE_MS = 1000
const RECONNECT_MAX_MS = 30_000

/** 等待中的重连定时器（存在即互斥，拒绝重复调度） */
const reconnectTimers = new Map<string, ReturnType<typeof setTimeout>>()
/** 每会话自动重连尝试次数（响应式，供状态栏/侧栏显示"第 N 次"） */
const reconnectAttemptMap = reactive(new Map<string, number>())
/** 用户手动断开的会话集合：抑制断开指令发出后在途残留的 EXIT 帧重新触发重连 */
const manualClosed = new Set<string>()
/** 连接代次令牌：每次发起/断开连接 +1。在途 invoke 返回后比对代次，
 *  不匹配说明等待期间用户已断开或发起了更新的连接——成功则关掉孤儿后端会话，
 *  失败则不碰状态，防止"已取消的连接延迟返回把状态打回 online"竞态 */
const connectGen = new Map<string, number>()

/** 活动会话当前自动重连尝试次数（0 = 未在自动重连） */
export const activeReconnectAttempt = computed(
  () => (activeSessionId.value && reconnectAttemptMap.get(activeSessionId.value)) || 0,
)

function clearReconnectTimer(id: string) {
  const t = reconnectTimers.get(id)
  if (t) {
    clearTimeout(t)
    reconnectTimers.delete(id)
  }
}

/** 放弃自动重连并清理全部痕迹；markManual 同时登记手动旗标（手动断开场景） */
function cancelAutoReconnect(id: string, markManual = false) {
  clearReconnectTimer(id)
  reconnectAttemptMap.delete(id)
  if (markManual) manualClosed.add(id)
}

/** 指数退避：1s、2s、4s…封顶 30s */
function reconnectDelayMs(attempt: number) {
  return Math.min(RECONNECT_MAX_MS, RECONNECT_BASE_MS * 2 ** (attempt - 1))
}
/** 对外暴露退避时长：重连弹窗的倒计时进度条按该时长做一次收缩动画 */
export function autoReconnectDelayMs(attempt: number) {
  return reconnectDelayMs(attempt)
}

/** 意外断线统一入口：按设置与退避策略调度下一次重连。可安全重复调用（互斥去重） */
function scheduleAutoReconnect(id: string) {
  if (!savedSettings.autoReconnect) return
  if (manualClosed.has(id)) return
  const s = sessions.value.find(x => x.id === id)
  if (!s || s.state === 'online') return
  if (reconnectTimers.has(id)) return // 等待中不重复调度

  const attempt = (reconnectAttemptMap.get(id) ?? 0) + 1
  const max = Math.max(0, Math.round(savedSettings.autoReconnectMaxAttempts) || 0)
  if (max > 0 && attempt > max) {
    reconnectAttemptMap.delete(id)
    // 超限只更新浮层文案，终端快照保持不动
    s.retryAt = null
    s.disconnectReason = `自动重连已停止（连续 ${max} 次失败），可手动重连`
    setSessionState(id, 'offline')
    return
  }

  reconnectAttemptMap.set(id, attempt)
  s.retryAt = Date.now() + reconnectDelayMs(attempt)
  if (!s.disconnectReason) s.disconnectReason = '连接已中断'
  setSessionState(id, 'reconnecting')

  const timer = setTimeout(() => {
    reconnectTimers.delete(id)
    const cur = sessions.value.find(x => x.id === id)
    // 等待期间被用户接管（手动断开/重连/关标签）：放弃本次
    if (!cur || cur.state !== 'reconnecting' || manualClosed.has(id)) return
    // 等待期间用户关闭了自动重连开关：回到已断开态，不再续试
    if (!savedSettings.autoReconnect) {
      reconnectAttemptMap.delete(id)
      cur.retryAt = null
      cur.disconnectReason = '连接已中断（自动重连已关闭）'
      setSessionState(id, 'offline')
      return
    }
    cur.retryAt = null // 尝试进行中，浮层转"正在重连"
    void runAutoReconnect(id)
  }, reconnectDelayMs(attempt))
  reconnectTimers.set(id, timer)
}

/** 执行一次自动重连（复用已记录的 PTY 尺寸；失败后续试由 connectBackend catch 驱动） */
async function runAutoReconnect(id: string) {
  const s = sessions.value.find(x => x.id === id)
  if (!s || manualClosed.has(id)) return
  const size = termSizes.get(id) ?? { cols: 120, rows: 32 }
  await connectBackend(s, size.cols, size.rows)
}

/** 重连：先断开后端会话，再按已记录的 PTY 尺寸重新连接。
 *  终端缓冲不在此时清理——断线快照保留到新 PTY 建连成功（resetSeq 驱动 reset） */
export async function reconnectBackend(id: string) {
  const s = sessions.value.find(x => x.id === id)
  if (!s) return
  // 用户手动接管：取消等待中的自动重连（立即执行本次）、清零计数与手动旗标
  clearReconnectTimer(id)
  reconnectAttemptMap.delete(id)
  manualClosed.delete(id)
  // 先置 reconnecting：旧会话断开产生的 EXIT 帧据此静默丢弃，不向终端写关闭提示；
  // 同时清掉旧连接断开期间排队的帧，避免新终端 attach 时补发旧输出
  s.retryAt = null
  s.disconnectReason = ''
  setSessionState(id, 'reconnecting')
  pendingFrames.delete(id)
  if (s.backendId && isTauri) {
    await invoke('disconnect_session', { sessionId: s.backendId }).catch(() => {})
    s.backendId = undefined
  }
  const size = termSizes.get(id) ?? { cols: 120, rows: 32 }
  void connectBackend(s, size.cols, size.rows)
}

/* ---- 会话管理 ---- */

/* ---- 会话持久化（冷/热启动分离）：SessionEntry[] 存 ui_state.sessions，恢复时用持久化 sessionId ---- */

function persistSessions() {
  if (!isTauri) return
  patchUiState({
    sessions: sessions.value.map(s => ({
      sessionId: s.id,
      hostId: s.host.id,
      alias: s.alias,
    })),
  })
}

/** 启动时恢复上次未关闭的会话；有则直接进入工作台并自动重连（热启动）。
 *  持久化格式为 SessionEntry[] = [{ sessionId, hostId, alias }]；
 *  旧 string[] 格式读取失败直接清空（不迁移，会话为临时状态）。
 *  不再按 hostId 去重——同一主机的多个会话逐条恢复。 */
export function restoreSessions() {
  const raw = getSnapshot()?.uiState.sessions
  interface StoredEntry { sessionId: string; hostId: string; alias: string }
  const entries: StoredEntry[] = []
  if (Array.isArray(raw)) {
    for (const x of raw) {
      if (x && typeof x === 'object' && 'sessionId' in x && 'hostId' in x) {
        const o = x as Record<string, unknown>
        entries.push({
          sessionId: typeof o.sessionId === 'string' ? o.sessionId : '',
          hostId: typeof o.hostId === 'string' ? o.hostId : '',
          alias: typeof o.alias === 'string' ? o.alias : '',
        })
      }
    }
  }
  const valid = entries.filter(e => e.sessionId && e.hostId && hosts.value.some(h => h.id === e.hostId))
  if (!valid.length) return
  // 懒连接：仅活跃会话（最后打开的）立即建连，其余恢复为 idle（灰点"空闲"），切 tab 时再连
  const restored: Session[] = valid.map((e, i) => ({
    id: e.sessionId,
    host: hosts.value.find(h => h.id === e.hostId)!,
    state: (i === valid.length - 1 ? 'connecting' : 'idle') as SessionState,
    startedAt: Date.now(),
    disconnectReason: '',
    retryAt: null,
    resetSeq: 0,
    alias: e.alias,
  }))
  const activeId = restored[restored.length - 1]!.id
  sessions.value = restored
  activeSessionId.value = activeId
  appView.value = 'workbench'
}

export function openSession(hostId: string) {
  const host = hosts.value.find(h => h.id === hostId)
  if (!host) return
  if (host.status === 'offline') return
  appView.value = 'workbench'
  // 同一主机复用已打开的会话，不重复建连
  const existing = sessions.value.find(s => s.host.id === hostId)
  if (existing) {
    activeSessionId.value = existing.id
    return
  }
  const id = crypto.randomUUID()
  sessions.value.push({
    id, host, state: 'connecting', startedAt: Date.now(),
    disconnectReason: '', retryAt: null, resetSeq: 0, alias: '',
  })
  activeSessionId.value = id
  persistSessions()
}

/** 主机被编辑（可能改名/替换对象）后，同步更新已打开会话的主机引用 */
export function rehostSession(oldHostId: string, host: Host) {
  const s = sessions.value.find(x => x.host.id === oldHostId)
  if (!s) return
  s.host = host
  persistSessions()
}

export function closeSession(id: string) {
  const idx = sessions.value.findIndex(s => s.id === id)
  if (idx === -1) return
  const s = sessions.value[idx]!
  // 关闭标签 = 停止自动重连 + 断开后端会话（触发取消令牌，后台任务退出）
  cancelAutoReconnect(id)
  manualClosed.delete(id)
  if (s.backendId && isTauri) {
    void invoke('disconnect_session', { sessionId: s.backendId }).catch(() => {})
  }
  termSinks.delete(id)
  pendingFrames.delete(id)
  termSizes.delete(id)
  connectGen.delete(id)
  hostInfoMap.delete(id)
  algoMap.delete(id)
  termSizeMap.delete(id)
  metricsMap.value.delete(id)
  netHistMap.value.delete(id)
  const timer = resizeTimers.get(id)
  if (timer) { clearTimeout(timer); resizeTimers.delete(id) }
  pendingResize.delete(id)
  lastSentResize.delete(id)
  sessions.value.splice(idx, 1)
  if (activeSessionId.value === id) {
    const rest = sessions.value
    // 关闭最后一个 Tab 留在工作台空态，不回首页
    activeSessionId.value = rest.length ? rest[rest.length - 1]!.id : null
  }
  persistSessions()
}

/* ---- 批量关闭（全部复用 closeSession，禁止另写清理路径） ---- */

/** 关闭除 id 外的全部会话；完成后激活 id */
export function closeOtherSessions(id: string) {
  const targets = sessions.value.filter(s => s.id !== id).map(s => s.id)
  targets.forEach(closeSession)
  activeSessionId.value = id
}

/** 关闭 id 左侧（按当前标签顺序）全部会话 */
export function closeSessionsToLeft(id: string) {
  const idx = sessions.value.findIndex(s => s.id === id)
  if (idx <= 0) return
  sessions.value.slice(0, idx).map(s => s.id).forEach(closeSession)
}

/** 关闭 id 右侧全部会话 */
export function closeSessionsToRight(id: string) {
  const idx = sessions.value.findIndex(s => s.id === id)
  if (idx < 0) return
  sessions.value.slice(idx + 1).map(s => s.id).forEach(closeSession)
}

/** 关闭全部 offline 标签；返回实际关闭数量（toast 用）。
 *  idle 态不属于"已断开"，不会被清理。 */
export function closeDisconnectedSessions(): number {
  const targets = sessions.value.filter(s => s.state === 'offline').map(s => s.id)
  targets.forEach(closeSession)
  return targets.length
}

/** 关闭全部标签，进入工作台空态 */
export function closeAllSessions() {
  sessions.value.map(s => s.id).forEach(closeSession)
}

/** 设置标签别名；空字符串 "" 表示清除别名，UI 回落展示 host.id。 */
export function setSessionAlias(sessionId: string, alias: string) {
  const item = sessions.value.find(s => s.id === sessionId)
  if (!item) return
  item.alias = alias
  persistSessions()
}

/* ---- 复制会话 ---- */

/** 进行中的复制会话源会话 id 集合（前端互斥，防止快速连点并发创建） */
const duplicating = new Set<string>()
const DUPLICATE_TIMEOUT_MS = 8000

/** 查询某源会话是否有进行中的复制请求（菜单项 disabled 判断用） */
export function isDuplicating(sourceSessionId: string): boolean {
  return duplicating.has(sourceSessionId)
}

/** 复制会话：基于源会话 id，创建全新独立会话，返回新 sessionId。
 *  新会话立即以 connecting 态加入 sessions；终端组件挂载后 attachTerminal
 *  自动发起 connectBackend，成功转 online，失败转 offline。
 *  并发保护：duplicating 集合互斥；8s setTimeout 兜底防止 IPC 异常导致
 *  finally 不执行、菜单项永久置灰。 */
export async function duplicateSession(sourceSessionId: string): Promise<string | null> {
  if (duplicating.has(sourceSessionId)) return null
  duplicating.add(sourceSessionId)
  const timer = setTimeout(() => duplicating.delete(sourceSessionId), DUPLICATE_TIMEOUT_MS)
  try {
    const source = sessions.value.find(s => s.id === sourceSessionId)
    if (!source) return null
    const newId = crypto.randomUUID()
    sessions.value.push({
      id: newId,
      host: source.host,
      state: 'connecting',
      startedAt: Date.now(),
      disconnectReason: '',
      retryAt: null,
      resetSeq: 0,
      // 继承源会话别名：源有别名则复制别名（与源同名，用户可自行重命名区分）；
      // 源无别名则 alias 为空，UI 回落展示 host.id（与源一致）。
      alias: source.alias,
    })
    activeSessionId.value = newId
    appView.value = 'workbench'
    persistSessions()
    return newId
  } finally {
    clearTimeout(timer)
    duplicating.delete(sourceSessionId)
  }
}

export function setSessionState(id: string, state: SessionState) {
  const s = sessions.value.find(x => x.id === id)
  if (s) s.state = state
}

/**
 * 断开连接：仅断开后端 SSH 会话，保留标签页、终端画面与输入冻结态（区别于 closeSession 关标签）。
 * 同时是自动重连的手动取消入口：等待退避期间（无 backendId）调用也生效。
 * 主动 disconnect 后后端不会再推 EXIT 帧（取消令牌直接终止转发循环），
 * 因此本地同步置 offline；不向终端写入任何字节，断线画面作为静态快照保留，
 * 登记手动旗标抑制在途残留 EXIT，之后可通过重连（reconnectBackend）恢复。
 * 返回 true 表示会话存在且已处理（已在线或处于重连等待）。
 */
export async function disconnectSession(id: string): Promise<boolean> {
  const s = sessions.value.find(x => x.id === id)
  if (!s) return false
  // 登记手动旗标并取消自动重连（含等待中的定时器），任何残留 EXIT 都不会再触发重连
  cancelAutoReconnect(id, true)
  // 代次 +1：在途 connect_ssh 即使随后成功/失败也按孤儿处理
  connectGen.set(id, (connectGen.get(id) ?? 0) + 1)
  if (s.backendId && isTauri) {
    const backendId = s.backendId
    s.backendId = undefined // 立即置空：断开的转发循环残留的 EXIT 帧不会重复处理
    try {
      await invoke('disconnect_session', { sessionId: backendId })
    } catch (e) {
      console.warn('disconnect_session 失败:', e)
    }
  }
  s.retryAt = null
  s.disconnectReason = '你已断开连接'
  setSessionState(id, 'offline')
  return true
}

/* ---- GPU 指标（0x06 帧 gpus[]；字段与后端 GpuItemPayload 对应，snake_case） ---- */
/** 单卡 GPU 指标（nvidia-smi --query-gpu；无 GPU 主机 gpus 为空数组，侧栏区块自动隐藏） */
export interface GpuMetricsData {
  /** 卡序号（nvidia-smi index） */
  index: number
  /** 型号名 */
  name: string
  /** 核心利用率 % */
  util: number
  /** 显存已用（字节） */
  mem_used: number
  /** 显存总量（字节） */
  mem_total: number
  /** 核心温度 °C */
  temp: number
  /** 当前功耗 W */
  power: number
  /** 功耗上限 W（0 表示不支持/未知，UI 隐藏功耗） */
  power_limit: number
}

/* ---- 右侧栏动态指标采集（0x06）：可见性门控启停 + 心跳续约 ---- */

/** 心跳间隔；后端 9s TTL，5s 续约留足余量 */
export const METRICS_HEARTBEAT_MS = 5000

/** 当前主机指标采集间隔（毫秒）：读全局设置，夹取 1~10s，与后端 clamp 保持一致 */
function metricsIntervalMs(): number {
  const sec = Math.min(10, Math.max(1, Math.round(savedSettings.metricsInterval) || 3))
  return sec * 1000
}

/** 启动指定后端会话的周期采集（幂等，后端重复 start 会先停旧任务）。
 *  iface 为默认路由网卡（0x05 已采集），网络计数优先取它；缺省后端汇总非 lo 网卡。
 *  intervalMs 由调用方（Inspector 门控）传入以便设置变更时热重启。 */
export async function startMetrics(
  backendId: string,
  iface?: string | null,
  intervalMs?: number,
) {
  if (!isTauri) return
  try {
    await invoke('start_metrics', {
      sessionId: backendId,
      intervalMs: intervalMs ?? metricsIntervalMs(),
      iface: iface || null,
    })
  } catch (e) {
    console.warn('start_metrics 失败:', e)
  }
}

/** 停止周期采集（Inspector 隐藏/切换会话时即时释放 exec 通道） */
export async function stopMetrics(backendId: string) {
  if (!isTauri) return
  try {
    await invoke('stop_metrics', { sessionId: backendId })
  } catch (e) {
    console.warn('stop_metrics 失败:', e)
  }
}

/** 心跳续约，维持后端采集任务存活 */
export function metricsHeartbeat(backendId: string) {
  if (!isTauri) return
  void invoke('metrics_heartbeat', { sessionId: backendId }).catch(() => {})
}
