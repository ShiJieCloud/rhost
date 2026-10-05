<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { activeAlgo, activeMetrics, activeSession, activeTermSize, connected } from '../../stores/session'
import { savedSettings } from '../../stores/settings'

// 五态：ok 已连接 / info 连接中 / warn 重连中 / err 已断开 / idle 未连接（新建会话，从未连接）
const statusText = () => {
  const s = activeSession.value
  if (!s) return { cls: 'idle', label: '未连接' }
  if (s.state === 'online') return { cls: 'ok', label: '已连接' }
  if (s.state === 'connecting') return { cls: 'info', label: '连接中…' }
  if (s.state === 'reconnecting') return { cls: 'warn', label: '重连中…' }
  if (s.state === 'idle') return { cls: 'idle', label: '未连接' }
  return { cls: 'err', label: '已断开' }
}

/** 网络速率（bytes/s，0x06 真实采集）；离线/首帧未到为 0 */
const txBps = computed(() => (connected.value ? activeMetrics.value?.net.tx_rate ?? 0 : 0))
const rxBps = computed(() => (connected.value ? activeMetrics.value?.net.rx_rate ?? 0 : 0))

/** bytes/s 自适应为底栏紧凑文本：<1MB/s 显示 KB，否则 MB */
function fmtSpeed(v: number) {
  if (v >= 1024 * 1024) return (v / 1024 / 1024).toFixed(1) + ' MB'
  return (v / 1024).toFixed(1) + ' KB'
}

const lat = computed(() => activeSession.value?.host.lat ?? null)

/* ---- SSH 握手真实协商算法（0x09 Algo 帧，首帧未到各项为 —） ---- */
/** 加密算法名美化：去 @openssh.com 后缀，aesNNN-x 显示为 AES-NNN-X（与 OpenSSH 习惯一致） */
function fmtCipherName(name: string): string {
  const base = name.replace(/@openssh\.com$/, '')
  const m = base.match(/^aes(\d+)-(.+)$/)
  if (m) return `AES-${m[1]}-${m[2]!.toUpperCase()}`
  return base.toUpperCase()
}
const hostKeyLabel = computed(() => activeAlgo.value?.host_key || '—')
const cipherLabel = computed(() =>
  activeAlgo.value ? fmtCipherName(activeAlgo.value.cipher) : '—',
)
const termLabel = computed(() => activeAlgo.value?.term || '—')
const encLabel = computed(() => activeAlgo.value?.enc || '—')
/** 终端窗口尺寸：列×行（xterm fit 实时变化）；终端未挂载显示 — */
const sizeLabel = computed(() => {
  const z = activeTermSize.value
  return z ? `${z.cols}×${z.rows}` : '—'
})

/* 延迟分档：<80ms 优 / <200ms 良 / ≥200ms 差 */
const latLevel = computed(() => {
  const v = lat.value
  if (v == null) return 'idle'
  if (v < 80) return 'ok'
  if (v < 200) return 'warn'
  return 'err'
})

/* ---- 工具自身内存占用（后端 sysinfo 采集 Rhost 主进程 RSS） ---- */
interface AppMemory {
  usedBytes: number
  totalBytes: number
}

const SPARK_W = 40
const SPARK_H = 11
const SPARK_N = 24 // 折线可见点数
const HIST_MAX = 32 // 历史缓冲长度
const MB = 1024 * 1024

const memSnap = ref<AppMemory | null>(null)
const memHist = ref<number[]>([])
const sparkLine = ref('')
const sparkArea = ref('')

const memUsedMb = computed(() => (memSnap.value ? memSnap.value.usedBytes / MB : null))
const memPct = computed(() =>
  memSnap.value && memSnap.value.totalBytes > 0
    ? (memSnap.value.usedBytes / memSnap.value.totalBytes) * 100
    : null,
)

/* 单位自适应：<1GB 显示整数 MB（128 MB），>=1GB 显示 GB（1.5 GB / 16 GB） */
function fmtMem(bytes: number): { val: string; unit: string } {
  const mb = bytes / MB
  if (mb < 1024) return { val: String(Math.round(mb)), unit: 'MB' }
  const gb = mb / 1024
  const val = gb >= 10 || Number.isInteger(gb) ? String(Math.round(gb)) : gb.toFixed(1)
  return { val, unit: 'GB' }
}
const usedFmt = computed(() => (memSnap.value ? fmtMem(memSnap.value.usedBytes) : null))
const totalFmt = computed(() => (memSnap.value ? fmtMem(memSnap.value.totalBytes) : null))

/* 绝对用量与系统占比双条件升档；warn 阈值取自全局设置的内存告警阈值，
   err 为硬编码的危险线（≥1GB 或 ≥40%），兼顾大小内存机型 */
const memLevel = computed<'ok' | 'warn' | 'err' | 'idle'>(() => {
  const used = memUsedMb.value
  const pct = memPct.value
  if (used == null || pct == null) return 'idle'
  if (used >= 1024 || pct >= 40) return 'err'
  if (used >= savedSettings.memAlertMb || pct >= 20) return 'warn'
  return 'ok'
})
const memColor = computed(() => {
  if (memLevel.value === 'err') return 'var(--rhost-status-error)'
  if (memLevel.value === 'warn') return 'var(--rhost-status-warning)'
  return 'var(--rhost-status-success)'
})

function renderSpark(hist: number[]) {
  const win = hist.slice(-SPARK_N)
  if (win.length < 2) {
    sparkLine.value = ''
    sparkArea.value = ''
    return
  }
  // 按可见窗口 min/max 归一化（呈现相对波动，与总量无关）
  const min = Math.min(...win)
  const range = Math.max(Math.max(...win) - min, 1)
  const pts = win.map((v, i) => {
    const x = (i / (win.length - 1)) * (SPARK_W - 2) + 1
    const y = SPARK_H - 2 - ((v - min) / range) * (SPARK_H - 4)
    return x.toFixed(1) + ',' + y.toFixed(1)
  })
  sparkLine.value = pts.join(' ')
  sparkArea.value = `1,${SPARK_H - 1} ${sparkLine.value} ${SPARK_W - 1},${SPARK_H - 1}`
}

async function pollMem() {
  try {
    const r = await invoke<AppMemory>('get_app_memory')
    memSnap.value = r
    const usedMb = r.usedBytes / MB
    const h = memHist.value
    if (h.length === 0) {
      // 首帧用当前值填平历史，折线从平线起步而非空白
      for (let i = 0; i < HIST_MAX; i++) h.push(usedMb)
    } else {
      h.push(usedMb)
      if (h.length > HIST_MAX) h.shift()
    }
    memHist.value = [...h]
    renderSpark(memHist.value)
  } catch {
    // 后端不可用（如 web 预览环境）时保持上一次数值，不打扰主流程
  }
}

let memTimer: ReturnType<typeof setInterval> | null = null

/* 可见性：IntersectionObserver 覆盖 v-show 切回首页（display:none），
   visibilitychange 覆盖窗口最小化/切后台 */
const barEl = ref<HTMLElement | null>(null)
const barVisible = ref(true)
const docVisible = ref(typeof document === 'undefined' ? true : document.visibilityState === 'visible')
let io: IntersectionObserver | null = null
function onVisibility() {
  docVisible.value = document.visibilityState === 'visible'
}

/* 暂停开关开启时，仅「状态栏可见 且 文档可见」才采集 */
const pollingActive = computed(() =>
  !savedSettings.memPauseHidden || (barVisible.value && docVisible.value),
)

function startPolling() {
  stopPolling()
  void pollMem() // 恢复可见时立即补一帧，不等下一个周期
  memTimer = setInterval(() => void pollMem(), Math.max(1, savedSettings.memInterval) * 1000)
}
function stopPolling() {
  if (memTimer) {
    clearInterval(memTimer)
    memTimer = null
  }
}

watch(pollingActive, active => {
  if (active) startPolling()
  else stopPolling()
})
// 采集间隔在设置中调整后，用新周期重启（仅在轮询期间）
watch(() => savedSettings.memInterval, () => {
  if (pollingActive.value) startPolling()
})

onMounted(() => {
  document.addEventListener('visibilitychange', onVisibility)
  if (barEl.value && typeof IntersectionObserver !== 'undefined') {
    io = new IntersectionObserver(entries => {
      barVisible.value = entries[0]?.isIntersecting ?? true
    })
    io.observe(barEl.value)
  }
  if (pollingActive.value) startPolling()
})
onUnmounted(() => {
  stopPolling()
  document.removeEventListener('visibilitychange', onVisibility)
  io?.disconnect()
})
</script>

<template>
  <footer ref="barEl" class="statusbar">
    <span class="item" :class="statusText().cls">
      <span class="pulse"></span>
      {{ statusText().label }}
    </span>
    <span class="item">
      {{ activeSession ? `${activeSession.host.user}@${activeSession.host.ip}:${activeSession.host.port}` : '—' }}
    </span>
    <span class="item">{{ hostKeyLabel }}</span>
    <span class="item">{{ cipherLabel }}</span>
    <span class="spacer"></span>


    <span class="item">{{ termLabel }}</span>
    <span class="item">{{ encLabel }}</span>
    <span class="item">{{ sizeLabel }}</span>

    <!-- 流量 + 延迟（附件设计：分段竖线分隔） -->
    <span class="seg"><span class="arrow-up">↑</span><span class="val">{{ fmtSpeed(txBps) }}</span></span>
    <span class="seg"><span class="arrow-down">↓</span><span class="val">{{ fmtSpeed(rxBps) }}</span></span>
    <span class="seg"><span class="dot-rtt" :class="latLevel"></span><span class="val" :class="latLevel">{{ lat ? lat + ' ms' : '—' }}</span></span>

    <!-- 工具自身内存占用：迷你趋势图 + 当前/总量 -->
    <span class="seg mem-seg">
      <svg class="mem-spark" :width="SPARK_W" :height="SPARK_H" :viewBox="`0 0 ${SPARK_W} ${SPARK_H}`">
        <polygon class="mem-area" :points="sparkArea" :fill="memColor"></polygon>
        <polyline class="mem-line" :points="sparkLine" fill="none"
                  :stroke="memColor" stroke-width="1.2"
                  stroke-linejoin="round" stroke-linecap="round"></polyline>
      </svg>
      <span class="mem-num">
        <template v-if="usedFmt">
          <span class="mem-val" :style="{ color: memColor }">{{ usedFmt.val }}</span><span class="mem-unit">{{ usedFmt.unit }}</span>
        </template>
        <span v-else class="mem-val">--</span>
        <span class="mem-slash">/</span>
        <template v-if="totalFmt">
          <span class="mem-total">{{ totalFmt.val }}</span><span class="mem-unit mem-unit-total">{{ totalFmt.unit }}</span>
        </template>
        <span v-else class="mem-total">--</span>
      </span>
    </span>
  </footer>
</template>
