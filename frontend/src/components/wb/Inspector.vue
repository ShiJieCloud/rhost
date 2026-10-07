<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { toast } from '../../composables/useToast'
import {
  METRICS_HEARTBEAT_MS, activeHost, activeHostInfo, activeMetrics, activeNetHist,
  activeReconnectAttempt, activeSession,
  connected, disconnectSession, inspectorVisible, metricsHeartbeat, openDock, reconnectTick,
  startMetrics, stopMetrics,
} from '../../stores/session'
import { savedSettings } from '../../stores/settings'

/* ---------- 会话时长计时 ---------- */
const uptime = ref('00:00:00')
let timer: ReturnType<typeof setInterval> | null = null

const loginTime = computed(() => {
  const s = activeSession.value
  if (!s) return '—'
  return new Date(s.startedAt).toLocaleTimeString('zh-CN', { hour12: false })
})

function pad(n: number) {
  return String(n).padStart(2, '0')
}

onMounted(() => {
  timer = setInterval(() => {
    const s = activeSession.value
    if (s && s.state === 'online') {
      const t = Math.floor((Date.now() - s.startedAt) / 1000)
      uptime.value = `${pad(Math.floor(t / 3600))}:${pad(Math.floor(t / 60) % 60)}:${pad(t % 60)}`
    }
  }, 1000)
})
onUnmounted(() => {
  if (timer) clearInterval(timer)
})

/* ---------- 连接状态五态 ---------- */
const stateLabel = computed(() => {
  const s = activeSession.value
  if (!s) return '未连接'
  if (s.state === 'online') return '已连接'
  if (s.state === 'connecting') return '连接中…'
  if (s.state === 'reconnecting') {
    return activeReconnectAttempt.value > 0
      ? `重连中 · 第 ${activeReconnectAttempt.value} 次`
      : '重连中…'
  }
  if (s.state === 'idle') return '未连接'
  return '已断开'
})
const stateCls = computed(() => {
  const s = activeSession.value
  if (!s) return 'idle'
  if (s.state === 'online') return 'ok'
  if (s.state === 'connecting') return 'info'
  if (s.state === 'reconnecting') return 'warn'
  if (s.state === 'idle') return 'idle'
  return 'err'
})
/** 状态圆点对应的语义色变量 */
const stateDotVar = computed(() => {
  switch (stateCls.value) {
    case 'ok': return 'var(--rhost-status-success)'
    case 'info': return 'var(--rhost-status-processing)'
    case 'warn': return 'var(--rhost-status-warning)'
    case 'err': return 'var(--rhost-status-error)'
    default: return 'var(--rhost-status-idle)'
  }
})

/* ---------- 主机地址（带复制） ---------- */
const hostLabel = computed(() => activeSession.value?.host.id ?? '—')
const hostIpPort = computed(() => {
  const s = activeSession.value
  return s ? `${s.host.ip}:${s.host.port}` : ''
})
/** 当前连接登录账号（本地配置，SSH 认证保证即实际账号，无需远端采集） */
const currentUser = computed(() => activeSession.value?.host.user || '—')
const authMethod = computed(() => {
  const s = activeSession.value
  if (!s) return '—'
  return s.host.keyPath ? '公钥认证' : '密码认证'
})
const latencyText = computed(() => {
  const lat = activeSession.value?.host.lat
  return lat ? `${lat} ms` : '—'
})

function copyAddr() {
  if (!hostIpPort.value) return
  navigator.clipboard?.writeText(hostIpPort.value).then(() => {
    toast('地址已复制到剪贴板', 'ok', 1400)
  }).catch(() => {
    toast('复制失败', 'err', 1200)
  })
}

/* ---------- 折叠区块状态 ---------- */
const collapsed = reactive({
  sysinfo: false,
  resources: false,
  gpu: false,
  network: false,
  disk: false,
  proc: false,
})
type SectionKey = keyof typeof collapsed
function toggle(key: SectionKey) {
  collapsed[key] = !collapsed[key]
}

/* ---------- 资源监控：CPU / 内存 / Swap ---------- */
/** 逻辑核数取真实 nproc（0x05），未到达前不显示具体数字 */
const cpuCoresText = computed(() =>
  activeHostInfo.value && activeHostInfo.value.cores > 0
    ? `${activeHostInfo.value.cores} 核`
    : '核数 N/A',
)
/** 当前在线会话的最新一帧真实指标；离线返回 null（进度条归零） */
const liveMetrics = computed(() => (connected.value ? activeMetrics.value : null))
const cpuPct = computed(() => liveMetrics.value?.cpu.util ?? 0)
/** 1/5/15 分钟负载文本（两位小数，运行队列长度非百分比）；未采集到不显示 */
const loadText = computed(() => {
  const l = liveMetrics.value?.cpu.load
  return l ? l.map(v => v.toFixed(2)).join(' / ') : null
})
const memPct = computed(() => {
  const x = liveMetrics.value?.mem
  if (!x || x.total <= 0) return 0
  return (x.used / x.total) * 100
})
/** 内存用量摘要：字节自适应单位；首帧未到/离线时占位 */
const memSummary = computed(() => {
  const x = liveMetrics.value?.mem
  if (!x || x.total <= 0) return '—'
  return `${fmtBytes(x.used)} / ${fmtBytes(x.total)} · ${memPct.value.toFixed(0)}%`
})
/** 无 swap（swap_total=0）时整个 Swap 区块隐藏，而非显示 0% */
const swapVisible = computed(() => (liveMetrics.value?.mem.swap_total ?? 0) > 0)
const swapPct = computed(() => {
  const x = liveMetrics.value?.mem
  if (!x || x.swap_total <= 0) return 0
  return (x.swap_used / x.swap_total) * 100
})
const swapSummary = computed(() => {
  const x = liveMetrics.value?.mem
  if (!x || x.swap_total <= 0) return '—'
  return `${fmtBytes(x.swap_used)} / ${fmtBytes(x.swap_total)} · ${swapPct.value.toFixed(0)}%`
})

/** 字节自适应单位：<1GB 显示整数 MB，>=1GB 保留一位小数（≥10GB 取整） */
function fmtBytes(bytes: number) {
  const mb = bytes / (1024 * 1024)
  if (mb < 1024) return `${Math.round(mb)} MB`
  const gb = mb / 1024
  return `${gb >= 10 || Number.isInteger(gb) ? Math.round(gb) : gb.toFixed(1)} GB`
}

function meterClass(pct: number) {
  if (pct >= 85) return 'crit'
  if (pct >= 65) return 'warn'
  return ''
}

/* ---------- GPU：显存/温度格式化 ---------- */
/** 显存占用百分比（后端显存为字节口径） */
function gpuMemPct(g: { mem_used: number; mem_total: number }) {
  if (!g.mem_total) return 0
  return Math.min(100, (g.mem_used / g.mem_total) * 100)
}
/** 温度语义色：≥85°C 红、≥72°C 黄 */
function gpuTempClass(t: number) {
  if (t >= 85) return 'crit'
  if (t >= 72) return 'warn'
  return ''
}

/* ---------- 网络：真实速率（0x06）+ 按会话隔离的 sparkline 历史 ---------- */
/** 活动会话当前网络指标；离线/首帧未到为 null（速率显示 0） */
const liveNet = computed(() => (connected.value ? activeMetrics.value?.net ?? null : null))
const txRate = computed(() => liveNet.value?.tx_rate ?? 0)
const rxRate = computed(() => liveNet.value?.rx_rate ?? 0)

/** bytes/s 自适应速率文本：<1MB/s 用 KB（一位小数），否则 MB（两位小数） */
function fmtRate(bps: number) {
  if (bps < 1024 * 1024) return `${(bps / 1024).toFixed(1)} KB/s`
  return `${(bps / 1024 / 1024).toFixed(2)} MB/s`
}

function sparkPoints(arr: number[]) {
  const W = 100
  const H = 24
  const max = Math.max(...arr) || 1
  return arr
    .map((v, i) => {
      const x = (i / (arr.length - 1)) * W
      const y = H - 2 - (v / max) * (H - 5)
      return `${x.toFixed(1)},${y.toFixed(1)}`
    })
    .join(' ')
}

/* ---------- 磁盘（0x06 Metrics.disks；df 低频夹带，非夹带轮沿用缓存） ---------- */
const liveDisks = computed(() => (connected.value ? activeMetrics.value?.disks ?? [] : []))

/* ---------- 进程（0x06 Metrics.procs；Top 5 每轮随帧更新） ---------- */
const liveProcs = computed(() =>
  connected.value ? activeMetrics.value?.procs ?? null : null,
)

/* ---------- GPU（0x06 Metrics.gpus；无 GPU 主机为空数组，区块整体隐藏） ---------- */
const liveGpus = computed(() =>
  connected.value ? activeMetrics.value?.gpus ?? [] : [],
)

/* ---------- 系统信息（0x05 HostInfo；未到达前发行版回退主机配置） ---------- */
const NA = 'N/A'
const sysinfo = computed(() => {
  const h = activeHostInfo.value
  return {
    distro: h?.distro || activeHost.value?.os || NA,
    kernel: h?.kernel || NA,
    arch: h?.arch || NA,
    cpuModel: h?.cpu_model || NA,
    uptime: h?.uptime || NA,
    timezone: h?.timezone || NA,
    ip: h?.ip && h.ip !== NA ? h.ip : NA,
  }
})

/** 网络区块标题：默认路由网卡名（0x05）；未采集到则不带后缀 */
const netTitle = computed(() =>
  activeHostInfo.value?.iface ? `网络 · ${activeHostInfo.value.iface}` : '网络',
)

/* ---------- 自动刷新开关（默认开启） ---------- */
const autoRefresh = ref(true)
function toggleAutoRefresh() {
  autoRefresh.value = !autoRefresh.value
  toast(autoRefresh.value ? '已开启自动刷新' : '已关闭自动刷新', 'info', 1200)
}

/* ---------- 动态指标采集门控（0x06） ----------
 * 仅当「活动会话在线 + Inspector 显示 + 面板在视口 + 页面可见 + 自动刷新」
 * 全部满足时才让后端采集；任一条件翻转即停，后端 9s 心跳 TTL 兜底。 */
const rootEl = ref<HTMLElement | null>(null)
const panelVisible = ref(true)
const docVisible = ref(typeof document === 'undefined' ? true : document.visibilityState === 'visible')

const metricsTarget = computed<{ id: string; iface: string | null; intervalMs: number } | null>(() => {
  const s = activeSession.value
  if (!s || s.state !== 'online' || !s.backendId) return null
  if (!autoRefresh.value || !inspectorVisible.value || !panelVisible.value || !docVisible.value) {
    return null
  }
  // iface 取自 0x05：首帧若晚到，target 变化会触发一次幂等重启切到默认网卡
  // intervalMs 取自全局设置：改采集间隔时 target 变化，幂等重启 collector 即时生效
  const intervalSec = Math.min(10, Math.max(1, Math.round(savedSettings.metricsInterval) || 3))
  return { id: s.backendId, iface: activeHostInfo.value?.iface || null, intervalMs: intervalSec * 1000 }
})

let runningBackendId: string | null = null
let runningIface: string | null = null
let runningIntervalMs: number | null = null
let hbTimer: ReturnType<typeof setInterval> | null = null

/** 按目标（后端会话 id + 网卡 + 采集间隔）做状态机式收敛：相同目标不重复启停 */
function reconcileMetrics(target: { id: string; iface: string | null; intervalMs: number } | null) {
  if (
    target
    && runningBackendId === target.id
    && runningIface === target.iface
    && runningIntervalMs === target.intervalMs
  ) return
  if (runningBackendId) {
    void stopMetrics(runningBackendId)
    runningBackendId = null
    runningIface = null
    runningIntervalMs = null
  }
  if (hbTimer) {
    clearInterval(hbTimer)
    hbTimer = null
  }
  if (target) {
    void startMetrics(target.id, target.iface, target.intervalMs)
    runningBackendId = target.id
    runningIface = target.iface
    runningIntervalMs = target.intervalMs
    hbTimer = setInterval(() => {
      if (runningBackendId) metricsHeartbeat(runningBackendId)
    }, METRICS_HEARTBEAT_MS)
  }
}

watch(metricsTarget, target => reconcileMetrics(target), { immediate: true })

function onDocVisibility() {
  docVisible.value = document.visibilityState === 'visible'
}
let panelObserver: IntersectionObserver | null = null
onMounted(() => {
  document.addEventListener('visibilitychange', onDocVisibility)
  // Inspector 以 v-show 常驻：display:none 时 isIntersecting 为 false
  if (rootEl.value && typeof IntersectionObserver !== 'undefined') {
    panelObserver = new IntersectionObserver(entries => {
      panelVisible.value = entries[0]?.isIntersecting ?? true
    })
    panelObserver.observe(rootEl.value)
  }
})
onUnmounted(() => {
  reconcileMetrics(null)
  document.removeEventListener('visibilitychange', onDocVisibility)
  panelObserver?.disconnect()
})

/* ---------- 折叠侧栏 ---------- */
function onCollapse() {
  inspectorVisible.value = false
}

/* ---------- 快捷操作（保留现有） ---------- */
function onReconnect() {
  if (!activeSession.value) {
    toast('没有活动会话', 'warn')
    return
  }
  reconnectTick.value++
}
async function onDisconnect() {
  const s = activeSession.value
  // 在线断开 SSH；重连等待中则取消自动重连（同一入口）
  if (!s || (s.state !== 'online' && s.state !== 'reconnecting')) {
    toast('当前没有活动会话', 'warn')
    return
  }
  const wasWaiting = s.state === 'reconnecting'
  if (await disconnectSession(s.id)) {
    toast(wasWaiting ? '已取消自动重连并断开' : '已断开连接', 'info', 1600)
  }
}
</script>

<template>
  <aside ref="rootEl" class="inspector ri-root">
    <!-- 头部：左侧标题 + 右侧操作图标（重连/SFTP/转发/编辑/断开/折叠） -->
    <header class="ri-head">
      <div class="ri-title">主机概览</div>
      <div class="ri-head-actions">

        <button class="ri-icon-btn" title="重新连接" @click="onReconnect">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
               stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M21 12a9 9 0 1 1-2.64-6.36" />
            <path d="M21 3v6h-6" />
          </svg>
        </button>

        <!-- 自动刷新开关 -->
        <button class="ri-icon-btn ri-toggle" :class="{ on: autoRefresh }"
                title="自动刷新"
                @click="toggleAutoRefresh">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/>
            <path d="M21 3v5h-5"/>
            <path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/>
            <path d="M8 16H3v5"/>
          </svg>
        </button>

        <button class="ri-icon-btn" title="打开 SFTP 文件管理"
                @click="openDock(); toast('已打开 SFTP 文件管理', 'info', 1600)">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
               stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
          </svg>
        </button>
        <button class="ri-icon-btn danger" title="断开连接" @click="onDisconnect">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M18.36 6.64a9 9 0 1 1-12.73 0"/>
            <line x1="12" y1="2" x2="12" y2="12"/>
          </svg>
        </button>
        <button class="ri-icon-btn" title="折叠侧栏" @click="onCollapse">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
               stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M9 18l6-6-6-6" />
          </svg>
        </button>
      </div>
    </header>

    <!-- 滚动内容 -->
    <div class="ri-body">
      <!-- 连接状态：扁平信息网格 -->
      <div class="ri-conn">
        <div class="ri-conn-grid">
          <div class="ri-cell">
            <div class="ri-label">状态</div>
            <div class="ri-value" :class="`st-${stateCls}`">
              <span class="ri-pulse" :style="{ background: stateDotVar }" />
              {{ stateLabel }}
            </div>
          </div>
          <div class="ri-cell">
            <div class="ri-label">时长</div>
            <div class="ri-value mono">{{ uptime }}</div>
          </div>
          <div class="ri-cell full">
            <div class="ri-label">主机</div>
            <div class="ri-value ri-host">{{ hostLabel }}</div>
          </div>
          <div class="ri-cell full">
            <div class="ri-label">地址</div>
            <div class="ri-value addr">
              <span class="addr-text">{{ hostIpPort || '—' }}</span>
              <button v-if="hostIpPort" class="ri-copy" title="复制地址" @click="copyAddr">
                <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                     stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                  <rect x="9" y="9" width="13" height="13" rx="2" />
                  <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
                </svg>
              </button>
            </div>
          </div>
          <div class="ri-cell">
            <div class="ri-label">用户</div>
            <div class="ri-value mono">{{ currentUser }}</div>
          </div>
          <div class="ri-cell">
            <div class="ri-label">认证</div>
            <div class="ri-value mono">{{ authMethod }}</div>
          </div>
          <div class="ri-cell">
            <div class="ri-label">RTT</div>
            <div class="ri-value mono">{{ latencyText }}</div>
          </div>
        </div>
      </div>

      <!-- 系统信息 -->
      <section class="ri-section" :class="{ collapsed: collapsed.sysinfo }">
        <button class="ri-sec-head" @click="toggle('sysinfo')">
          <span>系统信息</span>
          <svg class="chev" width="11" height="11" viewBox="0 0 24 24" fill="none"
               stroke="currentColor" stroke-width="2.4" stroke-linecap="round"
               stroke-linejoin="round">
            <path d="M6 9l6 6 6-6" />
          </svg>
        </button>
        <div class="ri-sec-body">
          <div class="ri-kv"><span class="k">发行版</span><span class="v" :title="sysinfo.distro">{{ sysinfo.distro }}</span></div>
          <div class="ri-kv"><span class="k">内核</span><span class="v mono" :title="sysinfo.kernel">{{ sysinfo.kernel }}</span></div>
          <div class="ri-kv"><span class="k">架构</span><span class="v mono">{{ sysinfo.arch }}</span></div>
          <div class="ri-kv"><span class="k">CPU 型号</span><span class="v mono" :title="sysinfo.cpuModel">{{ sysinfo.cpuModel }}</span></div>
          <div class="ri-kv"><span class="k">运行时长</span><span class="v mono">{{ sysinfo.uptime }}</span></div>
          <div class="ri-kv"><span class="k">时区</span><span class="v mono">{{ sysinfo.timezone }}</span></div>
          <div class="ri-kv"><span class="k">服务器 IP</span><span class="v mono" :title="sysinfo.ip">{{ sysinfo.ip }}</span></div>
          <div class="ri-kv"><span class="k">登录时间</span><span class="v mono">{{ loginTime }}</span></div>
        </div>
      </section>

      <!-- 资源监控 -->
      <section class="ri-section" :class="{ collapsed: collapsed.resources }">
        <button class="ri-sec-head" @click="toggle('resources')">
          <span>资源监控</span>
          <svg class="chev" width="11" height="11" viewBox="0 0 24 24" fill="none"
               stroke="currentColor" stroke-width="2.4" stroke-linecap="round"
               stroke-linejoin="round">
            <path d="M6 9l6 6 6-6" />
          </svg>
        </button>
        <div class="ri-sec-body">
          <div class="ri-res">
            <div class="ri-res-top">
              <span class="name">CPU</span>
              <span class="val mono">
                <b>{{ cpuPct.toFixed(0) }}%</b> · {{ cpuCoresText }}
              </span>
            </div>
            <div class="ri-meter"><i :class="meterClass(cpuPct)" :style="{ width: cpuPct + '%' }" /></div>
            <div v-if="loadText" class="ri-res-sub mono">负载 {{ loadText }}</div>
          </div>

          <div class="ri-res">
            <div class="ri-res-top">
              <span class="name">内存</span>
              <span class="val mono">
                <b>{{ memSummary }}</b>
              </span>
            </div>
            <div class="ri-meter"><i :class="meterClass(memPct)" :style="{ width: memPct + '%' }" /></div>
          </div>

          <!-- 无 swap 的主机（swap_total=0）整块隐藏 -->
          <div v-if="swapVisible" class="ri-res">
            <div class="ri-res-top">
              <span class="name">Swap</span>
              <span class="val mono">{{ swapSummary }}</span>
            </div>
            <div class="ri-meter"><i class="swap" :style="{ width: swapPct + '%' }" /></div>
          </div>
        </div>
      </section>

      <!-- GPU（无 GPU 主机整块隐藏） -->
      <section v-if="liveGpus.length" class="ri-section" :class="{ collapsed: collapsed.gpu }">
        <button class="ri-sec-head" @click="toggle('gpu')">
          <span>GPU · {{ liveGpus.length }} 卡</span>
          <svg class="chev" width="11" height="11" viewBox="0 0 24 24" fill="none"
               stroke="currentColor" stroke-width="2.4" stroke-linecap="round"
               stroke-linejoin="round">
            <path d="M6 9l6 6 6-6" />
          </svg>
        </button>
        <div class="ri-sec-body">
          <div v-for="g in liveGpus" :key="g.index" class="ri-gpu">
            <div class="ri-gpu-name" :title="g.name">
              <em class="ri-gpu-tag">{{ g.index }}</em>
              <span class="ri-gpu-model">{{ g.name }}</span>
            </div>

            <div class="ri-res">
              <div class="ri-res-top">
                <span class="name">利用率</span>
                <span class="val">
                  <b>{{ Math.round(g.util) }}%</b>
                  <span class="ri-gpu-aux">
                    · <span :class="gpuTempClass(g.temp)">{{ Math.round(g.temp) }}°C</span>
                    <template v-if="g.power_limit > 0">
                      · {{ Math.round(g.power) }}/{{ g.power_limit }}W
                    </template>
                  </span>
                </span>
              </div>
              <div class="ri-meter"><i :class="meterClass(g.util)" :style="{ width: g.util + '%' }" /></div>
            </div>

            <div class="ri-res">
              <div class="ri-res-top">
                <span class="name">显存</span>
                <span class="val">
                  {{ fmtBytes(g.mem_used) }} / {{ fmtBytes(g.mem_total) }} · {{ gpuMemPct(g).toFixed(0) }}%
                </span>
              </div>
              <div class="ri-meter"><i class="vram" :style="{ width: gpuMemPct(g) + '%' }" /></div>
            </div>
          </div>
        </div>
      </section>

      <!-- 网络 -->
      <section class="ri-section" :class="{ collapsed: collapsed.network }">
        <button class="ri-sec-head" @click="toggle('network')">
          <span>{{ netTitle }}</span>
          <svg class="chev" width="11" height="11" viewBox="0 0 24 24" fill="none"
               stroke="currentColor" stroke-width="2.4" stroke-linecap="round"
               stroke-linejoin="round">
            <path d="M6 9l6 6 6-6" />
          </svg>
        </button>
        <div class="ri-sec-body">
          <div class="ri-net-row">
            <div class="ri-net-label">
              <span class="arrow up">▲</span><span>上传</span>
            </div>
            <span class="ri-net-val">{{ fmtRate(txRate) }}</span>
            <svg class="ri-spark" viewBox="0 0 100 24" preserveAspectRatio="none">
              <polyline :points="sparkPoints(activeNetHist.up)" stroke="var(--purple)" />
            </svg>
          </div>
          <div class="ri-net-row">
            <div class="ri-net-label">
              <span class="arrow down">▼</span><span>下载</span>
            </div>
            <span class="ri-net-val">{{ fmtRate(rxRate) }}</span>
            <svg class="ri-spark" viewBox="0 0 100 24" preserveAspectRatio="none">
              <polyline :points="sparkPoints(activeNetHist.down)" stroke="var(--cyan)" />
            </svg>
          </div>
        </div>
      </section>

      <!-- 磁盘 -->
      <section class="ri-section" :class="{ collapsed: collapsed.disk }">
        <button class="ri-sec-head" @click="toggle('disk')">
          <span>磁盘</span>
          <svg class="chev" width="11" height="11" viewBox="0 0 24 24" fill="none"
               stroke="currentColor" stroke-width="2.4" stroke-linecap="round"
               stroke-linejoin="round">
            <path d="M6 9l6 6 6-6" />
          </svg>
        </button>
        <div class="ri-sec-body">
          <div v-if="liveDisks.length === 0" class="ri-disk-empty">等待采集…</div>
          <div v-for="d in liveDisks" :key="d.mount" class="ri-disk">
            <div class="ri-disk-top">
              <span class="mount mono">{{ d.mount }}</span>
              <span class="size mono">{{ fmtBytes(d.used) }} / {{ fmtBytes(d.total) }} · {{ d.pct }}%</span>
            </div>
            <div class="ri-meter">
              <i :class="meterClass(d.pct)" :style="{ width: d.pct + '%' }" />
            </div>
          </div>
        </div>
      </section>

      <!-- 进程 -->
      <section class="ri-section" :class="{ collapsed: collapsed.proc }">
        <button class="ri-sec-head" @click="toggle('proc')">
          <span>进程 · 共 {{ liveProcs ? liveProcs.total : 0 }} 个</span>
          <svg class="chev" width="11" height="11" viewBox="0 0 24 24" fill="none"
               stroke="currentColor" stroke-width="2.4" stroke-linecap="round"
               stroke-linejoin="round">
            <path d="M6 9l6 6 6-6" />
          </svg>
        </button>
        <div class="ri-sec-body">
          <div class="ri-proc-head">
            <span>进程</span><span>CPU</span><span>内存</span>
          </div>
          <div v-if="!liveProcs || liveProcs.top.length === 0" class="ri-disk-empty">等待采集…</div>
          <div v-for="p in liveProcs?.top ?? []" :key="p.pid" class="ri-proc-row">
            <span class="pname mono">
              {{ p.name }}<em>#{{ p.pid }}</em>
            </span>
            <span class="num" :class="{ 'cpu-hi': p.cpu >= 10, 'cpu-crit': p.cpu >= 20 }">
              {{ p.cpu.toFixed(1) }}
            </span>
            <span class="num mem">{{ fmtBytes(p.rss) }}</span>
          </div>
        </div>
      </section>
    </div>
  </aside>
</template>

<style scoped>
/* 根容器：覆盖全局 .inspector 的 padding/overflow，改为 flex 列布局 */
.inspector.ri-root {
  display: flex;
  flex-direction: column;
  padding: 0;
  overflow: hidden;
}

/* ============ 头部 ============ */
.ri-head {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 40px;
  padding: 0 14px;
  border-bottom: 1px solid var(--border-soft);
  background: var(--panel);
  flex: 0 0 auto;
}
.ri-title {
  display: flex;
  align-items: center;
  gap: 9px;
  font-size: 12.5px;
  font-weight: 600;
  color: var(--text);
  letter-spacing: 0.02em;
}
.ri-title::before {
  content: '';
  width: 3px;
  height: 14px;
  border-radius: 2px;
  background: var(--green);
  flex: 0 0 auto;
}
.ri-head-actions {
  margin-left: auto;
  display: flex;
  gap: 2px;
}
.ri-icon-btn {
  width: 28px;
  height: 28px;
  border-radius: 6px;
  display: grid;
  place-items: center;
  background: transparent;
  border: 1px solid transparent;
  color: var(--muted-2);
  cursor: pointer;
  transition: background 0.15s, color 0.15s, border-color 0.15s;
}
.ri-icon-btn:hover {
  background: var(--panel-2);
  border-color: var(--border);
  color: var(--text);
}
.ri-icon-btn:active {
  transform: scale(0.94);
}
.ri-icon-btn.danger:hover {
  background: rgba(247, 118, 142, 0.12);
  border-color: rgba(247, 118, 142, 0.4);
  color: var(--red);
}

/* 自动刷新开关：开启=品牌绿+持续旋转，关闭=与普通按钮一致 */
.ri-toggle.on {
  color: var(--green);
}
.ri-toggle.on svg {
  transform-origin: 50% 50%;
  animation: ri-spin 3s linear infinite;
}
@keyframes ri-spin {
  to {
    transform: rotate(360deg);
  }
}
/* ============ 滚动内容 ============ */
.ri-body {
  flex: 1;
  overflow-y: auto;
  overflow-x: hidden;
  padding: 0 12px 12px;
}
.ri-body::-webkit-scrollbar {
  width: 8px;
}
.ri-body::-webkit-scrollbar-thumb {
  background: #1c2836;
  border-radius: 4px;
  border: 2px solid var(--panel);
}
.ri-body::-webkit-scrollbar-thumb:hover {
  background: #2a3a4d;
}

/* ============ 连接状态卡片 ============ */
.ri-conn {
  margin: 14px 0 2px;
  padding: 14px;
  border-radius: 10px;
  border: 1px solid var(--border);
  background: var(--panel-2);
}
.ri-conn-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 13px 16px;
}
.ri-cell {
  min-width: 0;
}
.ri-cell.full {
  grid-column: 1 / -1;
}
.ri-label {
  font-size: 9.5px;
  font-weight: 700;
  letter-spacing: 0.11em;
  color: var(--muted-2);
  margin-bottom: 4px;
  text-transform: uppercase;
}
.ri-value {
  font-family: var(--mono);
  font-size: 12.5px;
  color: var(--text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.ri-value.mono {
  font-family: var(--mono);
}
.ri-value.ri-host {
  font-size: 13.5px;
  font-weight: 600;
  color: #fff;
}
.ri-value.st-ok {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--rhost-status-success);
  font-weight: 600;
}
.ri-value.st-info {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--rhost-status-processing);
  font-weight: 600;
}
.ri-value.st-warn {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--rhost-status-warning);
  font-weight: 600;
}
.ri-value.st-err {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--rhost-status-error);
  font-weight: 600;
}
.ri-value.st-idle {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--rhost-status-idle);
  font-weight: 600;
}
.ri-pulse {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  flex: 0 0 auto;
  box-shadow: 0 0 0 0 currentColor;
  animation: ri-pulse 2s infinite;
}
@keyframes ri-pulse {
  0% {
    box-shadow: 0 0 0 0 rgba(61, 220, 132, 0.5);
  }
  70% {
    box-shadow: 0 0 0 7px rgba(61, 220, 132, 0);
  }
  100% {
    box-shadow: 0 0 0 0 rgba(61, 220, 132, 0);
  }
}
.ri-value.addr {
  display: flex;
  align-items: center;
  gap: 2px;
}
.addr-text {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.ri-copy {
  background: none;
  border: 0;
  color: var(--muted-2);
  cursor: pointer;
  padding: 2px 5px;
  font-size: 11px;
  line-height: 1;
  border-radius: 4px;
  display: grid;
  place-items: center;
  flex-shrink: 0;
  transition: background 0.15s, color 0.15s;
}
.ri-copy:hover {
  color: var(--green);
  background: rgba(61, 220, 132, 0.12);
}

/* ============ 折叠区块 ============ */
.ri-section {
  border-top: 1px solid var(--border-soft);
}
.ri-section:first-of-type {
  border-top: 0;
}
.ri-sec-head {
  width: 100%;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 11px 2px 9px;
  background: none;
  border: 0;
  cursor: pointer;
  color: var(--muted-2);
  font-family: var(--sans, inherit);
  font-size: 10.5px;
  font-weight: 700;
  letter-spacing: 0.09em;
}
.ri-sec-head:hover {
  color: var(--muted);
}
.ri-sec-head .chev {
  margin-left: auto;
  transition: transform 0.18s ease;
  opacity: 0.6;
}
.ri-section.collapsed .chev {
  transform: rotate(-90deg);
}
.ri-section.collapsed .ri-sec-body {
  display: none;
}
.ri-sec-body {
  padding: 0 2px 14px;
}

/* 键值行 */
.ri-kv {
  display: flex;
  align-items: baseline;
  gap: 10px;
  padding: 3.5px 0;
  font-size: 12px;
}
.ri-kv .k {
  flex: 0 0 auto;
  color: var(--muted);
}
.ri-kv .v {
  margin-left: auto;
  text-align: right;
  color: var(--text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
  max-width: 66%;
}
.ri-kv .v.mono {
  font-family: var(--mono);
  font-size: 11.5px;
}

/* ============ 资源监控进度条 ============ */
.ri-res + .ri-res {
  margin-top: 13px;
}
.ri-res-top {
  display: flex;
  align-items: baseline;
  gap: 8px;
  font-size: 11.5px;
  margin-bottom: 6px;
}
.ri-res-top .name {
  color: var(--muted);
}
.ri-res-top .val {
  margin-left: auto;
  font-family: var(--mono);
  font-size: 11.5px;
  color: var(--text);
}
.ri-meter {
  height: 6px;
  border-radius: 3px;
  background: #1a222c;
  overflow: hidden;
}
.ri-meter > i {
  display: block;
  height: 100%;
  border-radius: 3px;
  background: var(--green);
  transition: width 0.5s ease, background 0.3s;
}
.ri-meter > i.warn {
  background: var(--yellow);
}
.ri-meter > i.crit {
  background: var(--red);
}
.ri-meter > i.swap {
  background: var(--purple);
}
.ri-meter > i.vram {
  background: var(--cyan);
}
/* 进度条下方的单行辅助信息（如 CPU 的 1/5/15 分钟负载） */
.ri-res-sub {
  margin-top: 5px;
  font-size: 11px;
  line-height: 1.4;
  white-space: nowrap;
  color: var(--muted);
}

/* ============ GPU ============ */
.ri-gpu + .ri-gpu {
  margin-top: 14px;
  padding-top: 13px;
  border-top: 1px solid var(--border-soft);
}
.ri-gpu-name {
  display: flex;
  align-items: center;
  gap: 7px;
  margin-bottom: 10px;
  min-width: 0;
}
.ri-gpu-tag {
  flex: 0 0 auto;
  font-style: normal;
  font-family: var(--mono);
  font-size: 9.5px;
  font-weight: 700;
  letter-spacing: 0.04em;
  color: var(--cyan);
  background: rgba(79, 214, 224, 0.1);
  border: 1px solid rgba(79, 214, 224, 0.25);
  border-radius: 4px;
  padding: 1px 5px;
}
.ri-gpu-model {
  font-family: var(--mono);
  font-size: 11.5px;
  color: var(--text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.ri-gpu-aux {
  color: var(--muted);
  font-weight: 400;
}
.ri-gpu-aux .warn {
  color: var(--yellow);
}
.ri-gpu-aux .crit {
  color: var(--red);
}

/* ============ 网络 ============ */
.ri-net-row {
  display: flex;
  align-items: center;
  gap: 10px;
}
.ri-net-row + .ri-net-row {
  margin-top: 9px;
}
.ri-net-label {
  flex: 0 0 74px;
  display: flex;
  align-items: center;
  gap: 5px;
  font-size: 11.5px;
  color: var(--muted);
}
.ri-net-label .arrow {
  font-size: 11px;
}
.ri-net-label .arrow.up {
  color: var(--purple);
}
.ri-net-label .arrow.down {
  color: var(--cyan);
}
.ri-net-val {
  font-family: var(--mono);
  font-size: 11.5px;
  color: var(--text);
  white-space: nowrap;
}
.ri-spark {
  flex: 1 1 auto;
  height: 24px;
  min-width: 0;
  overflow: visible;
}
.ri-spark polyline {
  fill: none;
  stroke-width: 1.4;
  stroke-linecap: round;
  stroke-linejoin: round;
  vector-effect: non-scaling-stroke;
}

/* ============ 磁盘 ============ */
.ri-disk + .ri-disk {
  margin-top: 12px;
}
.ri-disk-empty {
  font-size: 11.5px;
  color: var(--muted);
  padding: 2px 0;
}
.ri-disk-top {
  display: flex;
  align-items: baseline;
  gap: 8px;
  font-size: 11.5px;
  margin-bottom: 5px;
}
.ri-disk-top .mount {
  min-width: 0;
  font-family: var(--mono);
  color: var(--text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.ri-disk-top .size {
  margin-left: auto;
  flex-shrink: 0;
  font-family: var(--mono);
  font-size: 11px;
  color: var(--muted);
  white-space: nowrap;
}

/* ============ 进程表 ============ */
.ri-proc-head,
.ri-proc-row {
  display: grid;
  grid-template-columns: 1fr 48px 52px;
  gap: 8px;
  align-items: center;
  font-size: 11.5px;
  padding: 4px 0;
}
.ri-proc-head {
  color: var(--muted-2);
  font-size: 10px;
  letter-spacing: 0.06em;
  border-bottom: 1px solid var(--border-soft);
  padding-bottom: 5px;
  margin-bottom: 2px;
}
.ri-proc-head span:not(:first-child),
.ri-proc-row span:not(:first-child) {
  text-align: right;
  font-family: var(--mono);
}
.ri-proc-row .pname {
  font-family: var(--mono);
  color: var(--text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.ri-proc-row .pname em {
  margin-left: 5px;
  color: var(--muted-2);
  font-style: normal;
}
.ri-proc-row .num.cpu-hi {
  color: var(--yellow);
}
.ri-proc-row .num.cpu-crit {
  color: var(--red);
}
.ri-proc-row .num.mem {
  color: var(--muted);
}

</style>
