<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import AppLogo from '../../components/AppLogo.vue'
import { toast } from '../../composables/useToast'
import { promptPassword } from '../../composables/usePasswordPrompt'
import { addHost, hosts } from '../../stores/hosts'
import { openSession } from '../../stores/session'
import { isTauri } from '../../lib/tauri'
import { onConfigLoad, persistSectionNow } from '../../stores/appConfig'
import type { Host } from '../../types'

/* ================= 命令解析 ================= */
interface ParsedCmd {
  user: string
  host: string
  port: string
  key: string
}

const OPTS_WITH_VALUE = new Set(
  ['-o', '-c', '-m', '-F', '-J', '-b', '-e', '-E', '-I', '-L', '-R', '-D', '-w', '-Q', '-S', '-W'],
)

function tokenize(str: string): string[] {
  const out: string[] = []
  let cur = ''
  let quote: string | null = null
  for (let i = 0; i < str.length; i++) {
    const ch = str[i]!
    if (quote) {
      if (ch === quote) quote = null
      else cur += ch
    } else if (ch === '"' || ch === "'") {
      quote = ch
    } else if (ch === ' ' || ch === '\t') {
      if (cur) {
        out.push(cur)
        cur = ''
      }
    } else {
      cur += ch
    }
  }
  if (cur) out.push(cur)
  return out
}

function parseCommand(raw: string): ParsedCmd | null {
  const tokens = tokenize(raw.trim())
  if (!tokens.length) return null

  let i = tokens[0]!.toLowerCase() === 'ssh' ? 1 : 0
  let user = ''
  let port = ''
  let key = ''
  const positional: string[] = []

  for (; i < tokens.length; i++) {
    const t = tokens[i]!
    if (t === '--') {
      positional.push(...tokens.slice(i + 1))
      break
    }
    if (t === '-p' || t === '--port') {
      port = tokens[++i] || ''
      continue
    }
    if (t === '-i' || t === '--identity-file') {
      key = tokens[++i] || ''
      continue
    }
    if (t === '-l') {
      user = tokens[++i] || ''
      continue
    }
    if (/^-p\d+$/.test(t)) {
      port = t.slice(2)
      continue
    }
    if (/^-i\S+$/.test(t)) {
      key = t.slice(2)
      continue
    }
    if (/^-l\S+$/.test(t)) {
      user = t.slice(2)
      continue
    }
    if (OPTS_WITH_VALUE.has(t)) {
      i++
      continue
    }
    if (t.startsWith('-')) continue
    positional.push(t)
  }

  const spec = positional[0] || ''
  let host = ''
  if (spec) {
    const at = spec.lastIndexOf('@')
    if (at > -1) {
      if (!user) user = spec.slice(0, at)
      host = spec.slice(at + 1)
    } else {
      host = spec
    }
  }
  if (!host) return null

  const p = parseInt(port, 10)
  if (port && (!Number.isInteger(p) || p < 1 || p > 65535)) return null

  return { user, host, port: port || '22', key }
}

/* ================= 历史记录（持久化于后端 quick_connect_history 节，磁盘格式 {host,timestamp}） ================= */
interface HistoryItem {
  cmd: string
  time: number
}

/** 磁盘条数上限（与后端 MAX_QUICK_HISTORY_ENTRIES 对齐）；列表仅展示最近 HISTORY_SHOW 条 */
const HISTORY_MAX = 50
const HISTORY_SHOW = 6

const history = ref<HistoryItem[]>([])

onConfigLoad(snap => {
  if (!Array.isArray(snap.quickConnectHistory)) return
  history.value = snap.quickConnectHistory
    .map(it => {
      const o = (it ?? {}) as Record<string, unknown>
      return { cmd: String(o.host ?? ''), time: Number(o.timestamp ?? 0) }
    })
    .filter(it => it.cmd)
})

/** UI 模型 {cmd,time} → 磁盘模型 {host,timestamp} */
function saveHistory() {
  const disk = history.value.map(it => ({ host: it.cmd, timestamp: it.time }))
  persistSectionNow('quick_connect_history', disk).catch(e =>
    console.error('快速连接历史写穿失败:', e),
  )
}

function pushHistory(cmd: string) {
  history.value = history.value.filter(it => it.cmd !== cmd)
  history.value.unshift({ cmd, time: Date.now() })
  history.value = history.value.slice(0, HISTORY_MAX)
  saveHistory()
}

function removeHistory(cmd: string) {
  history.value = history.value.filter(it => it.cmd !== cmd)
  saveHistory()
}

function clearAllHistory() {
  history.value = []
  saveHistory()
  historyVisible.value = false
}

function relTime(ts: number): string {
  const diff = Date.now() - ts
  const m = Math.floor(diff / 60000)
  if (m < 1) return '刚刚'
  if (m < 60) return `${m} 分钟前`
  const h = Math.floor(m / 60)
  if (h < 24) return `${h} 小时前`
  const d = Math.floor(h / 24)
  if (d < 30) return `${d} 天前`
  return new Date(ts).toLocaleDateString('zh-CN', { month: 'numeric', day: 'numeric' })
}

/* ================= 组件状态 ================= */
const cmdInput = ref('')
const inputEl = ref<HTMLInputElement | null>(null)
const historyVisible = ref(false)
const historyIndex = ref(-1)
const invalid = ref(false)
const shaking = ref(false)
/** 正在测试连接（门禁：失败不跳转） */
const testing = ref(false)

const parsed = computed<ParsedCmd | null>(() => {
  if (!cmdInput.value.trim()) return null
  return parseCommand(cmdInput.value)
})

const canGo = computed(() => parsed.value !== null)

const visibleHistory = computed(() => history.value.slice(0, HISTORY_SHOW))

/* ================= 交互 ================= */
function onInput() {
  invalid.value = false
  if (cmdInput.value.trim()) {
    historyVisible.value = false
  } else if (history.value.length) {
    historyVisible.value = true
  }
  historyIndex.value = -1
}

function onFocus() {
  if (!cmdInput.value.trim() && history.value.length) {
    historyVisible.value = true
  }
}

function applyHistory(cmd: string, focus = true) {
  cmdInput.value = cmd
  historyVisible.value = false
  historyIndex.value = -1
  invalid.value = false
  if (focus) {
    inputEl.value?.focus()
    requestAnimationFrame(() => {
      const el = inputEl.value
      if (el) el.setSelectionRange(el.value.length, el.value.length)
    })
  }
}

function connect(raw?: string) {
  const cmd = (raw ?? cmdInput.value).trim()
  if (!cmd) return
  if (testing.value) return
  const p = parseCommand(cmd)
  if (!p) {
    invalid.value = true
    shaking.value = false
    requestAnimationFrame(() => {
      shaking.value = true
    })
    setTimeout(() => (shaking.value = false), 450)
    inputEl.value?.focus()
    return
  }

  pushHistory(cmd)
  historyVisible.value = false

  const port = parseInt(p.port, 10)
  const user = p.user || 'root'
  // 在 hosts 中匹配：ip+port 相同且（命令未指定用户 或 用户也相同）即视为已配置
  let target = hosts.value.find(
    h => h.ip === p.host && h.port === port && (!p.user || h.user === p.user),
  )

  // 未找到则自动创建主机：一键连接 = 输入即建连，无需先去主机列表手动新建
  if (!target) {
    const id = `${user}@${p.host}${port !== 22 ? ':' + port : ''}`
    const label =
      id.replace(/[^a-zA-Z0-9一-龥]/g, '').slice(0, 2).toUpperCase() || 'SS'
    target = {
      id,
      user,
      ip: p.host,
      port,
      os: 'Linux (未知发行版)',
      color: 'green',
      label,
      tag: '新建',
      status: 'idle',
      lat: null,
      cpu: '—',
      mem: '—',
      uptime: '—',
      group: '开发环境',
    } satisfies Host
    void addHost(target)
  }

  if (target.status === 'offline') {
    toast(`主机 ${target.id} 处于离线状态，无法连接`, 'err', 2400)
    return
  }

  // -i 指定了私钥 → 密钥认证：路径已在命令中给出，不弹窗；
  // 仅当后端判定私钥加密（KEY_ENCRYPTED 标记）时才弹口令框
  if (p.key) {
    void runKeyConnect(target, p.key)
    return
  }

  // 门禁：先测试连接，失败不跳转工作台；非 Tauri 环境无后端，跳过测试直接进入
  if (isTauri) {
    void runTestAndConnect(target)
  } else {
    openSession(target.id)
  }
}

/** 密钥认证：先试无口令加载；私钥加密时弹口令框重试，口令错误可反复重试 */
async function runKeyConnect(host: Host, keyPath: string) {
  // 非 Tauri 环境无后端，直接 mock 进入
  if (!isTauri) {
    openSession(host.id)
    return
  }
  testing.value = true
  host.keyPath = keyPath
  let passphrase = ''
  try {
    for (;;) {
      try {
        await invoke<{ latencyMs: number }>('test_ssh_connection', {
          payload: {
            host: host.ip,
            port: host.port,
            username: host.user,
            keyPath,
            passphrase,
            cols: 0,
            rows: 0,
          },
        })
        // 测试通过：口令暂存到主机（重连免输），打开会话
        host.keyPassphrase = passphrase || undefined
        openSession(host.id)
        return
      } catch (e) {
        const msg = String(e)
        if (msg.startsWith('KEY_ENCRYPTED')) {
          // 私钥已加密（或上一轮口令错误）：弹口令框重试
          // 口令是私钥的本地解密口令（passphrase），与服务器登录密码无关
          const pw = await promptPassword(
            `密钥 ${keyPath} 已加密，输入口令解锁：`,
            '输入私钥口令',
            '输入私钥口令（passphrase）',
          )
          if (pw == null) return // 用户取消
          passphrase = pw
          continue
        }
        // 其它失败：报错留在一键连接页，不跳转
        toast(`连接失败：${msg}`, 'err', 3000)
        return
      }
    }
  } finally {
    testing.value = false
  }
}

/** 测试连接 → 成功才跳转，失败留在当前页 */
async function runTestAndConnect(host: Host) {
  testing.value = true
  // 解析密码：主机已存则用之，否则弹专用密码框询问（Tauri 下 window.prompt 不可用）
  const password = host.password ?? await promptPassword(`输入 ${host.user}@${host.ip} 的登录密码：`)
  if (password == null) {
    testing.value = false
    return // 用户取消
  }
  try {
    await invoke<{ latencyMs: number }>('test_ssh_connection', {
      payload: {
        host: host.ip,
        port: host.port,
        username: host.user,
        password,
        cols: 0,
        rows: 0,
      },
    })
  } catch (e) {
    // 测试失败：报错并留在一键连接页，不跳转
    toast(`连接失败：${String(e)}`, 'err', 3000)
    return
  } finally {
    testing.value = false
  }
  // 测试通过：保存密码（若之前是弹窗输入的）并打开会话
  if (!host.password) {
    host.password = password
    // 回写系统钥匙串，下次启动免询问
    if (isTauri) {
      invoke('save_connection_password', { id: host.id, password }).catch(e =>
        console.error('save_connection_password 失败:', e),
      )
    }
  }
  openSession(host.id)
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Enter') {
    e.preventDefault()
    connect()
    return
  }
  if (e.key === 'ArrowUp') {
    if (!history.value.length) return
    e.preventDefault()
    historyIndex.value = Math.min(historyIndex.value + 1, history.value.length - 1)
    cmdInput.value = history.value[historyIndex.value]!.cmd
    historyVisible.value = false
    return
  }
  if (e.key === 'ArrowDown') {
    if (!history.value.length) return
    e.preventDefault()
    historyIndex.value = Math.max(historyIndex.value - 1, -1)
    cmdInput.value = historyIndex.value === -1 ? '' : history.value[historyIndex.value]!.cmd
    if (!cmdInput.value.trim() && history.value.length) historyVisible.value = true
    return
  }
  if (e.key === 'Escape') {
    if (historyVisible.value) {
      e.preventDefault()
      historyVisible.value = false
      return
    }
    e.preventDefault()
    cmdInput.value = ''
    historyIndex.value = -1
    invalid.value = false
  }
}

function onGlobalClick(e: MouseEvent) {
  const target = e.target as HTMLElement
  if (!target.closest('.qc-input-wrap')) historyVisible.value = false
}

const SAMPLES = [
  'ssh root@10.0.1.25',
  'ssh -p 2222 deploy@172.16.8.14',
  'ssh -i ~/.ssh/id_ed25519 ubuntu@192.168.1.50',
]

function applySample(s: string) {
  applyHistory(s)
}

onMounted(() => {
  document.addEventListener('mousedown', onGlobalClick)
  if (window.innerWidth > 640) {
    requestAnimationFrame(() => inputEl.value?.focus())
  }
})

onUnmounted(() => {
  document.removeEventListener('mousedown', onGlobalClick)
})
</script>

<template>
  <div class="qc-page">
    <div class="qc-shell">
      <div class="qc-mark">
        <AppLogo />
      </div>

      <h1 class="qc-title">一键连接</h1>
      <p class="qc-sub">输入 SSH 命令，按 Enter 立即建立连接</p>

      <div class="qc-input-wrap">
        <div
          class="qc-input-box"
          :class="{ invalid, shake: shaking }"
        >
          <span class="qc-sigil">$</span>
          <input
            ref="inputEl"
            v-model="cmdInput"
            type="text"
            spellcheck="false"
            autocomplete="off"
            autocapitalize="off"
            autocorrect="off"
            placeholder="ssh root@10.0.1.25 -p 22"
            aria-label="SSH 命令"
            @input="onInput"
            @focus="onFocus"
            @keydown="onKeydown"
          />
          <button
            type="button"
            class="qc-go"
            :class="{ loading: testing }"
            :title="testing ? '正在测试连接…' : '连接 (Enter)'"
            aria-label="连接"
            :disabled="!canGo || testing"
            @click="connect()"
          >
            <svg v-if="!testing" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.7"
                 stroke-linecap="round" stroke-linejoin="round">
              <path d="M5 12h13" />
              <path d="m12 5 7 7-7 7" />
            </svg>
            <span v-else class="qc-spin" aria-hidden="true"></span>
          </button>
        </div>

        <!-- 历史浮层 -->
        <section v-if="historyVisible && visibleHistory.length" class="qc-history">
          <div class="qc-history-head">
            <span class="qc-history-title">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
                   stroke-linecap="round" stroke-linejoin="round">
                <circle cx="12" cy="12" r="9" />
                <path d="M12 7v5l3 2" />
              </svg>
              最近命令
            </span>
            <button type="button" class="qc-history-clear" @click="clearAllHistory">清空</button>
          </div>
          <div class="qc-history-list">
            <div
              v-for="item in visibleHistory"
              :key="item.cmd + item.time"
              class="qc-history-item"
              :title="item.cmd"
              @click="applyHistory(item.cmd)"
              @dblclick="connect(item.cmd)"
            >
              <span class="qc-history-cmd">{{ item.cmd }}</span>
              <span class="qc-history-time">{{ relTime(item.time) }}</span>
              <span class="qc-history-actions">
                <button
                  type="button"
                  class="qc-h-action run"
                  title="立即连接"
                  aria-label="立即连接"
                  @click.stop="connect(item.cmd)"
                >
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6"
                       stroke-linecap="round" stroke-linejoin="round">
                    <path d="M5 12h13" />
                    <path d="m12 5 7 7-7 7" />
                  </svg>
                </button>
                <button
                  type="button"
                  class="qc-h-action del"
                  title="删除这条记录"
                  aria-label="删除"
                  @click.stop="removeHistory(item.cmd)"
                >
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6"
                       stroke-linecap="round">
                    <path d="M6 6l12 12M18 6L6 18" />
                  </svg>
                </button>
              </span>
            </div>
          </div>
        </section>
      </div>

      <!-- 解析预览 -->
      <div class="qc-preview" aria-live="polite">
        <span v-if="!cmdInput.trim()" class="qc-preview-text">例如 ssh root@10.0.1.25 -p 22</span>
        <span v-else-if="!parsed" class="qc-chip err">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
               stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="10" />
            <line x1="12" y1="8" x2="12" y2="12" />
            <line x1="12" y1="16" x2="12.01" y2="16" />
          </svg>
          <span class="v">SSH 命令解析失败，请检查命令格式</span>
        </span>
        <template v-else>
          <span class="qc-chip">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                 stroke-linecap="round" stroke-linejoin="round">
              <path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2" />
              <circle cx="12" cy="7" r="4" />
            </svg>
            <span class="k">用户</span>
            <span class="v">{{ parsed.user || 'root' }}</span>
          </span>
          <span class="qc-chip host">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                 stroke-linecap="round" stroke-linejoin="round">
              <rect x="2.5" y="3.5" width="19" height="13" rx="2" />
              <path d="M8 21h8" />
              <path d="M12 17v4" />
            </svg>
            <span class="k">主机</span>
            <span class="v">{{ parsed.host }}</span>
          </span>
          <span class="qc-chip">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                 stroke-linecap="round" stroke-linejoin="round">
              <path d="M9 2v6M15 2v6M6 8h12v3a6 6 0 0 1-12 0z" />
              <path d="M12 17v5" />
            </svg>
            <span class="k">端口</span>
            <span class="v">{{ parsed.port }}</span>
          </span>
          <span v-if="parsed.key" class="qc-chip">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                 stroke-linecap="round" stroke-linejoin="round">
              <circle cx="7.5" cy="15.5" r="5.5" />
              <path d="m21 2-9.6 9.6" />
              <path d="m15.5 7.5 3 3L22 7l-3-3" />
            </svg>
            <span class="k">密钥</span>
            <span class="v">{{ parsed.key }}</span>
          </span>
        </template>
      </div>

      <!-- 样例 -->
      <div class="qc-samples-label">示例命令</div>
      <div class="qc-samples">
        <button
          v-for="s in SAMPLES"
          :key="s"
          type="button"
          class="qc-sample"
          @click="applySample(s)"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
               stroke-linecap="round" stroke-linejoin="round">
            <rect x="2.5" y="3.5" width="19" height="13" rx="2" />
            <path d="M8 21h8" />
            <path d="M12 17v4" />
          </svg>
          <span>{{ s }}</span>
        </button>
      </div>

      <div class="qc-keys">
        <span><kbd>Enter</kbd> 连接</span>
        <span><kbd>↑</kbd><kbd>↓</kbd> 历史命令</span>
        <span><kbd>Esc</kbd> 清空</span>
      </div>
    </div>
  </div>
</template>
