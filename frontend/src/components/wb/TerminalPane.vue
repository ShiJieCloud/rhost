<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { toast } from '../../composables/useToast'
import { hosts, showNewConn } from '../../stores/hosts'
import {
  activeHost, activeSessionId, connected, openSession, reconnectTick, setSessionState,
} from '../../stores/session'
import type { Host, HostStatus } from '../../types'

/* TODO: 接入后端后由 IPC 二进制流 + xterm.js 替换 DOM 模拟 */
interface TermState {
  lines: string[]
  buffer: string
  enabled: boolean
  promptHtml: string
  history: string[]
  histIdx: number
}

const terms = reactive<Record<string, TermState>>({})
const current = ref<TermState | null>(null)
const termEl = ref<HTMLElement | null>(null)
const wrapEl = ref<HTMLElement | null>(null)
let token = 0
let resizeObs: ResizeObserver | null = null

function esc(s: unknown) {
  return String(s).replace(/[&<>"']/g, c =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c] as string)
}

const sleep = (ms: number) => new Promise(r => setTimeout(r, ms))

/** 粘底跟随：force=true 强制回底（新连接/用户输入）；否则仅当用户停在底部时跟随，翻阅历史时不打扰 */
const stickBottom = ref(true)

function isAtBottom() {
  const el = termEl.value
  if (!el) return true
  return el.scrollHeight - el.scrollTop - el.clientHeight < 40
}

function scrollToBottom(force = false) {
  nextTick(() => {
    const el = termEl.value
    if (!el) return
    if (!force && !stickBottom.value) return
    el.scrollTop = el.scrollHeight
    stickBottom.value = true
  })
}

function onScroll() {
  stickBottom.value = isAtBottom()
}

function ensureState(id: string): TermState {
  if (!terms[id]) {
    terms[id] = { lines: [], buffer: '', enabled: false, promptHtml: '', history: [], histIdx: 0 }
  }
  return terms[id]
}

function setPrompt(st: TermState, host: Host) {
  st.promptHtml =
    `<span class="c-green">${esc(host.user)}@${esc(host.id)}</span>` +
    `<span class="c-dim">:</span><span class="c-blue">~</span>` +
    `<span class="c-dim">$</span>`
}

async function connectSequence(host: Host, reconnect = false) {
  const my = ++token
  const st = ensureState(host.id)
  st.enabled = false
  st.buffer = ''
  st.lines = []
  setSessionState(host.id, reconnect ? 'reconnecting' : 'connecting')
  scrollToBottom(true)

  const lat = host.lat || 0

  if (host.status === 'offline') {
    st.lines.push(`<span class="c-green">$</span> <span class="c-white">ssh ${esc(host.user)}@${esc(host.ip)} -p ${host.port}</span>`)
    await sleep(260)
    if (my !== token) return
    st.lines.push(`<span class="c-dim">正在连接 ${esc(host.ip)}:${host.port} …</span>`)
    await sleep(900)
    if (my !== token) return
    st.lines.push(`<span class="c-red">✖ ssh: connect to host ${esc(host.ip)} port ${host.port}: Connection timed out</span>`)
    st.lines.push('<span class="c-dim">请检查网络连通性或主机电源状态。</span>')
    setSessionState(host.id, 'offline')
    return
  }

  const seq: { h: string; d: number }[] = [
    { h: `<span class="c-green">$</span> <span class="c-white">ssh ${esc(host.user)}@${esc(host.ip)} -p ${host.port}</span>`, d: 200 },
    { h: `<span class="c-dim">正在连接 ${esc(host.ip)}:${host.port} …</span>`, d: 320 },
    { h: `<span class="c-green">✔</span> TCP 握手完成  <span class="c-dim">延迟 ${lat}ms</span>`, d: 200 },
    { h: `<span class="c-dim">远端协议 OpenSSH_8.9p1 · 密钥交换 curve25519-sha256</span>`, d: 200 },
    { h: `<span class="c-dim">使用公钥 "~/.ssh/id_ed25519" 认证 …</span>`, d: 280 },
    { h: `<span class="c-green">✔</span> 认证成功，会话已加密 <span class="c-dim">(AES-256-GCM)</span>`, d: 220 },
    { h: '', d: 70 },
  ]

  for (const s of seq) {
    if (my !== token) return
    st.lines.push(s.h)
    scrollToBottom(true)
    await sleep(s.d)
  }

  const banner = [
    '  ______                    _                 ',
    ' /_  __/__  _________ ___  (_)__  ____  __  __',
    '  / / / _ \\/ ___/ __ `__ \\/ / _ \\/ __ \\/ / / /',
    ' / / /  __/ /  / / / / / / /  __/ / / / /_/ / ',
    '/_/  \\___/_/  /_/ /_/ /_/_/\\___/_/ /_/\\__,_/  ',
  ]
  banner.forEach(l => st.lines.push(`<span class="c-green c-bold">${esc(l)}</span>`))
  st.lines.push('')
  st.lines.push('<span class="c-dim">欢迎使用 Rhost · 输入 </span><span class="c-green">help</span><span class="c-dim"> 查看可用命令</span>')
  st.lines.push('')
  ;[
    ['<span class="c-cyan">系统</span>', esc(host.os)],
    ['<span class="c-cyan">内核</span>', '5.15.0-91-generic x86_64'],
    ['<span class="c-cyan">负载</span>', '0.08, 0.12, 0.09'],
    ['<span class="c-cyan">运行</span>', '42 days, 6 hours'],
    ['<span class="c-cyan">登录</span>', new Date().toLocaleString('zh-CN', { hour12: false })],
  ].forEach(([k, v]) => st.lines.push(`${k}<span class="c-dim"> · </span><span class="c-white">${v}</span>`))
  st.lines.push('')

  if (my !== token) return
  setPrompt(st, host)
  st.enabled = true
  setSessionState(host.id, 'online')
  scrollToBottom(true)
}

watch(activeSessionId, id => {
  if (!id) {
    current.value = null
    return
  }
  const st = ensureState(id)
  const isFirstVisit = st.lines.length === 0
  current.value = st
  const host = activeHost.value
  if (host && isFirstVisit) void connectSequence(host)
}, { immediate: true })

watch(reconnectTick, () => {
  const host = activeHost.value
  if (!host) return
  toast(`正在重新连接 ${host.id} …`, 'info')
  void connectSequence(host, true)
})

/* ---- 命令模拟 ---- */
function clearScreen() {
  const st = current.value
  if (st) st.lines = []
}

function disconnect() {
  const host = activeHost.value
  const st = current.value
  if (!host || !st) return
  st.lines.push('<span class="c-dim">logout</span>')
  st.lines.push(`<span class="c-dim">Connection to ${esc(host.ip)} closed.</span>`)
  st.enabled = false
  setSessionState(host.id, 'offline')
  toast('会话已断开', 'info')
}

function runCommand(raw: string) {
  const st = current.value!
  const host = activeHost.value!
  const cmd = raw.toLowerCase().replace(/\s+/g, ' ')

  const table = (rows: string[][]) =>
    rows.forEach(r => st.lines.push(r.join(' ')))

  switch (cmd) {
    case 'help':
      st.lines.push('<span class="c-cyan c-bold">可用命令</span>')
      ;[
        ['help', '显示此帮助'], ['ls', '列出目录内容'], ['pwd', '当前工作路径'],
        ['whoami', '当前登录用户'], ['date', '系统时间'], ['uptime', '运行时长与负载'],
        ['df -h', '磁盘使用情况'], ['free -h', '内存使用情况'], ['docker ps', '运行中的容器'],
        ['neofetch', '系统信息概览'], ['clear', '清空屏幕'], ['exit', '断开连接'],
      ].forEach(([c, d]) =>
        st.lines.push(`  <span class="c-green cmd-col">${esc(c)}</span><span class="c-dim">${esc(d)}</span>`))
      return
    case 'ls':
      st.lines.push('<span class="c-dim">total 32</span>')
      table([
        ['<span class="c-dim">drwxr-xr-x</span>  4 root root <span class="c-cyan"> 4096</span> <span class="c-dim">9月 30 09:12</span> <span class="c-blue">app</span>'],
        ['<span class="c-dim">drwxr-xr-x</span>  2 root root <span class="c-cyan"> 4096</span> <span class="c-dim">9月 25 11:30</span> <span class="c-blue">config</span>'],
        ['<span class="c-dim">-rwxr-xr-x</span>  1 root root <span class="c-cyan"> 2048</span> <span class="c-dim">9月 30 08:05</span> <span class="c-green">deploy.sh</span>'],
        ['<span class="c-dim">-rw-r--r--</span>  1 root root <span class="c-cyan"> 1024</span> <span class="c-dim">9月 18 10:22</span> <span class="c-white">README.md</span>'],
        ['<span class="c-dim">-rw-------</span>  1 root root <span class="c-cyan"> 1675</span> <span class="c-dim">9月 27 19:44</span> <span class="c-yellow">.env</span>'],
      ])
      return
    case 'pwd':
      st.lines.push('<span class="c-white">/root</span>')
      return
    case 'whoami':
      st.lines.push(`<span class="c-green">${esc(host.user)}</span>`)
      return
    case 'date':
      st.lines.push(`<span class="c-white">${new Date().toLocaleString('zh-CN', { hour12: false })}</span>`)
      return
    case 'uptime':
      st.lines.push('<span class="c-white"> 09:41:22 up 42 days,  6:31,  2 users,  load average: 0.08, 0.12, 0.09</span>')
      return
    case 'df -h':
      st.lines.push('<span class="c-dim">Filesystem      Size  Used Avail Use% Mounted on</span>')
      table([
        ['<span class="c-white">/dev/vda1</span>        <span class="c-cyan">48.5G</span>  <span class="c-cyan">20.5G</span>  <span class="c-cyan">25.7G</span>  <span class="c-yellow">42%</span> /'],
        ['<span class="c-white">/dev/vdb1</span>       <span class="c-cyan">200.0G</span>  <span class="c-cyan">88.2G</span> <span class="c-cyan">101.4G</span>  <span class="c-yellow">47%</span> /data'],
      ])
      return
    case 'free -h':
      st.lines.push('<span class="c-dim">              total   used   free  shared  buff/cache  available</span>')
      table([
        ['<span class="c-white">Mem:</span>           <span class="c-cyan">15Gi</span>  <span class="c-cyan">4.8Gi</span>  <span class="c-cyan">6.2Gi</span>  <span class="c-cyan">312Mi</span>   <span class="c-cyan">4.4Gi</span>     <span class="c-green">9.7Gi</span>'],
        ['<span class="c-white">Swap:</span>           <span class="c-cyan">2.0Gi</span>    <span class="c-cyan">0B</span>    <span class="c-cyan">2.0Gi</span>'],
      ])
      return
    case 'docker ps':
      st.lines.push('<span class="c-dim">CONTAINER ID   IMAGE              STATUS         PORTS                  NAMES</span>')
      table([
        ['<span class="c-white">a3f8c21d9e01</span>   nginx:1.25-alpine  <span class="c-green">Up 42 days</span>    0.0.0.0:80->80/tcp     <span class="c-cyan">web-frontend</span>'],
        ['<span class="c-white">7b2e5a10cc44</span>   redis:7.2-alpine   <span class="c-green">Up 42 days</span>    6379/tcp               <span class="c-cyan">cache-redis</span>'],
      ])
      return
    case 'neofetch': {
      const art = [
        '        .--.          ',
        '       |o_o |         ',
        '       |:_/ |         ',
        '      //   \\ \\        ',
        "     (|     | )       ",
        "    /'\\_   _/`\\       ",
        '    \\___)=(___/       ',
      ]
      const info = [
        `<span class="c-green c-bold">${esc(host.user)}@${esc(host.id)}</span>`,
        '<span class="c-dim">─────────────────────</span>',
        `<span class="c-cyan">OS</span>      ${esc(host.os)}`,
        '<span class="c-cyan">Kernel</span>  5.15.0-91-generic',
        '<span class="c-cyan">Uptime</span>  42 days, 6 hours',
        '<span class="c-cyan">Shell</span>   bash 5.1.16',
        '<span class="c-cyan">CPU</span>     Xeon E5-2680 v4 (8) @ 2.40GHz',
        '<span class="c-cyan">Memory</span>  4.8Gi / 15Gi (32%)',
      ]
      const n = Math.max(art.length, info.length)
      for (let i = 0; i < n; i++) {
        st.lines.push(`<span class="c-green">${esc(art[i] || '                      ')}</span>${info[i] || ''}`)
      }
      return
    }
    case 'clear':
      clearScreen()
      return
    case 'exit':
      disconnect()
      return
  }

  if (cmd === 'sudo su' || cmd === 'su') {
    st.lines.push('<span class="c-red">su: Authentication failure</span>')
    return
  }
  st.lines.push(`<span class="c-red">bash: ${esc(raw.split(' ')[0])}: command not found</span>`)
  st.lines.push('<span class="c-dim">输入 </span><span class="c-green">help</span><span class="c-dim"> 查看可用命令</span>')
}

/* ---- 键盘 ---- */
function onKey(e: KeyboardEvent) {
  if (showNewConn.value) return
  const t = e.target as HTMLElement | null
  if (t && (t.tagName === 'INPUT' || t.tagName === 'SELECT' || t.tagName === 'TEXTAREA')) return
  // ⌘/Ctrl+T：新建连接（全局）
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 't') {
    e.preventDefault()
    showNewConn.value = true
    return
  }
  const st = current.value
  if (!st || !st.enabled) return

  if (e.key === 'Enter') {
    e.preventDefault()
    const cmd = st.buffer
    st.lines.push(`${st.promptHtml} ${esc(cmd)}`)
    st.buffer = ''
    if (cmd.trim()) {
      st.history.push(cmd.trim())
      st.histIdx = st.history.length
    }
    runCommand(cmd.trim())
  } else if (e.key === 'Backspace') {
    e.preventDefault()
    st.buffer = st.buffer.slice(0, -1)
  } else if (e.key === 'Escape') {
    st.buffer = ''
  } else if (e.key === 'ArrowUp') {
    e.preventDefault()
    if (st.histIdx > 0) {
      st.histIdx--
      st.buffer = st.history[st.histIdx]
    }
  } else if (e.key === 'ArrowDown') {
    e.preventDefault()
    if (st.histIdx < st.history.length - 1) {
      st.histIdx++
      st.buffer = st.history[st.histIdx]
    } else {
      st.histIdx = st.history.length
      st.buffer = ''
    }
  } else if (e.key === 'l' && e.ctrlKey) {
    e.preventDefault()
    clearScreen()
  } else if (e.key === 'c' && e.ctrlKey) {
    e.preventDefault()
    st.lines.push(`${st.promptHtml} ${esc(st.buffer)}<span class="c-red">^C</span>`)
    st.buffer = ''
  } else if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
    e.preventDefault()
    st.buffer += e.key
  }
  // 用户敲键盘 = 回到提示符交互，强制回底
  scrollToBottom(true)
}

onMounted(() => {
  document.addEventListener('keydown', onKey)
  // 终端可视高度变化（Dock 展示/拖拽/窗口缩放）时保持滚动在底部
  resizeObs = new ResizeObserver(() => scrollToBottom())
  if (wrapEl.value) resizeObs.observe(wrapEl.value)
})
onUnmounted(() => {
  document.removeEventListener('keydown', onKey)
  resizeObs?.disconnect()
})

void connected

/* ---- 空态（关闭所有 Tab 后）：最近连接取主机列表前 3 台，在线优先 ---- */
const recentHosts = computed(() =>
  [...hosts.value]
    .sort((a, b) => (a.status === 'online' ? 0 : 1) - (b.status === 'online' ? 0 : 1))
    .slice(0, 3),
)

function dotClass(status: HostStatus) {
  return status === 'online' ? 'online' : status === 'offline' ? 'offline' : 'warn'
}
</script>

<template>
  <div ref="wrapEl" class="terminal-wrap">
    <!-- 工具栏（仅在终端活跃时显示） -->
    <div v-if="current" class="term-tools">
      <button title="滚动到顶部" @click="termEl && (termEl.scrollTop = 0)">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
          <line x1="12" y1="19" x2="12" y2="5"></line>
          <polyline points="5 12 12 5 19 12"></polyline>
        </svg>
      </button>
      <button title="滚动到底部" @click="scrollToBottom(true)">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
          <line x1="12" y1="5" x2="12" y2="19"></line>
          <polyline points="19 12 12 19 5 12"></polyline>
        </svg>
      </button>
      <button title="清屏" @click="clearScreen(); scrollToBottom(true)">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M3 6h18M8 6V4a1 1 0 0 1 1-1h6a1 1 0 0 1 1 1v2"></path>
          <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"></path>
        </svg>
      </button>
    </div>

    <!-- 终端（有会话时） -->
    <div v-if="current" ref="termEl" class="terminal" @scroll="onScroll">
      <div v-for="(line, i) in current.lines" :key="i" class="line" v-html="line"></div>
      <div class="line" :class="{ hidden: !current.enabled }">
        <span class="prompt" v-html="current.promptHtml"></span>
        <span>{{ current.buffer }}</span>
        <span class="cursor"></span>
      </div>
    </div>

    <!-- 空态（关闭所有 Tab 后） -->
    <div v-else class="tp-empty">
      <div class="icon-wrap">
        <div class="icon-box">
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
            <rect x="2.5" y="4" width="19" height="16" rx="3"/>
            <path d="M7 9.5L9.5 12L7 14.5"/>
            <path d="M13 14.5h4"/>
          </svg>
        </div>
      </div>
      <h1>没有活动的会话</h1>
      <p>连接到远程主机，或打开一个本地终端开始工作。</p>

      <div class="actions">
        <button class="btn primary" @click="showNewConn = true">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round">
            <path d="M12 5v14M5 12h14"/>
          </svg>
          新建连接
        </button>
        <button class="btn" @click="showNewConn = true">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">
            <path d="M7 9.5L9.5 12L7 14.5"/><path d="M13 14.5h4"/>
            <rect x="2.5" y="4" width="19" height="16" rx="3"/>
          </svg>
          本地终端
        </button>
      </div>

      <div v-if="recentHosts.length" class="recent">
        <div class="recent-label">可用主机</div>
        <div class="recent-list">
          <button v-for="h in recentHosts" :key="h.id" class="recent-item" @click="openSession(h.id)">
            <span class="dot" :class="dotClass(h.status)"></span>
            <span class="rname">{{ h.id }}</span>
            <span class="raddr">{{ h.user }}@{{ h.ip }}</span>
            <span class="rgo">
              <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M9 6l6 6-6 6"/>
              </svg>
            </span>
          </button>
        </div>
      </div>

      <div class="hints">
        <span><kbd>⌘</kbd><kbd>T</kbd> 新建连接</span>
        <span><kbd>⌘</kbd><kbd>K</kbd> 命令面板</span>
      </div>
    </div>
  </div>
</template>
