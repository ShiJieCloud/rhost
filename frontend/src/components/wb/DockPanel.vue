<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { activeHost, closeDock, connected, dockCollapsed, dockTab, searchTick } from '../../stores/session'
import { toast } from '../../composables/useToast'

/* ---- SFTP 文件系统（树形，可导航） ---- */
interface FsEntry { name: string; type: 'dir' | 'file'; size: number; mtime: string }
type FsTree = Record<string, FsEntry[]>

const LOCAL_FS: FsTree = {
  '/': [
    { name: 'Users', type: 'dir', size: 0, mtime: '2026-08-01 00:00' },
    { name: 'tmp', type: 'dir', size: 0, mtime: '2026-09-20 10:00' },
  ],
  '/Users': [
    { name: 'dev', type: 'dir', size: 0, mtime: '2026-09-20 10:00' },
    { name: 'shared', type: 'dir', size: 0, mtime: '2026-08-15 10:00' },
  ],
  '/Users/dev': [
    { name: 'Desktop', type: 'dir', size: 0, mtime: '2026-09-30 18:20' },
    { name: 'Documents', type: 'dir', size: 0, mtime: '2026-09-28 11:00' },
    { name: 'Downloads', type: 'dir', size: 0, mtime: '2026-10-01 09:45' },
    { name: 'Projects', type: 'dir', size: 0, mtime: '2026-09-30 22:15' },
    { name: '.zshrc', type: 'file', size: 3921, mtime: '2026-09-15 08:00' },
    { name: '.gitconfig', type: 'file', size: 386, mtime: '2026-08-10 14:30' },
  ],
  '/Users/dev/Downloads': [
    { name: 'report.pdf', type: 'file', size: 2202009, mtime: '2026-10-01 09:40' },
    { name: 'data.csv', type: 'file', size: 131072, mtime: '2026-09-30 20:15' },
    { name: 'archive.zip', type: 'file', size: 52428800, mtime: '2026-09-29 16:30' },
    { name: 'screenshot.png', type: 'file', size: 1048576, mtime: '2026-09-28 11:00' },
    { name: 'notes.md', type: 'file', size: 8192, mtime: '2026-09-27 15:22' },
  ],
  '/Users/dev/Desktop': [
    { name: 'todo.md', type: 'file', size: 2048, mtime: '2026-10-01 08:00' },
    { name: 'work', type: 'dir', size: 0, mtime: '2026-09-30 18:20' },
  ],
  '/Users/dev/Documents': [
    { name: 'bookmarks.html', type: 'file', size: 18432, mtime: '2026-09-15 10:00' },
    { name: 'resume.pdf', type: 'file', size: 262144, mtime: '2026-08-20 09:00' },
  ],
  '/Users/dev/Projects': [
    { name: 'rhost', type: 'dir', size: 0, mtime: '2026-09-30 22:15' },
    { name: 'website', type: 'dir', size: 0, mtime: '2026-09-25 14:00' },
  ],
  '/Users/dev/Projects/rhost': [
    { name: 'src', type: 'dir', size: 0, mtime: '2026-09-30 22:15' },
    { name: 'package.json', type: 'file', size: 1280, mtime: '2026-09-28 20:00' },
    { name: 'README.md', type: 'file', size: 3072, mtime: '2026-09-26 10:00' },
  ],
  '/tmp': [
    { name: 'build.log', type: 'file', size: 524288, mtime: '2026-10-01 10:00' },
    { name: 'cache', type: 'dir', size: 0, mtime: '2026-09-30 22:00' },
  ],
}

const REMOTE_FS: FsTree = {
  '/': [
    { name: 'var', type: 'dir', size: 0, mtime: '2026-09-20 08:00' },
    { name: 'home', type: 'dir', size: 0, mtime: '2026-09-20 08:00' },
    { name: 'etc', type: 'dir', size: 0, mtime: '2026-09-20 08:00' },
  ],
  '/var': [
    { name: 'www', type: 'dir', size: 0, mtime: '2026-09-25 11:30' },
    { name: 'log', type: 'dir', size: 0, mtime: '2026-10-01 09:40' },
  ],
  '/var/www': [
    { name: 'app', type: 'dir', size: 0, mtime: '2026-09-25 11:30' },
    { name: 'config', type: 'dir', size: 0, mtime: '2026-09-25 11:30' },
    { name: 'logs', type: 'dir', size: 0, mtime: '2026-09-30 09:40' },
    { name: 'static', type: 'dir', size: 0, mtime: '2026-09-22 16:02' },
    { name: 'deploy.sh', type: 'file', size: 2048, mtime: '2026-09-30 08:12' },
    { name: 'README.md', type: 'file', size: 1024, mtime: '2026-09-18 10:22' },
    { name: '.env', type: 'file', size: 1600, mtime: '2026-09-27 19:44' },
  ],
  '/var/www/app': [
    { name: 'index.js', type: 'file', size: 12300, mtime: '2026-09-25 11:30' },
    { name: 'package.json', type: 'file', size: 1280, mtime: '2026-09-25 11:30' },
    { name: 'node_modules', type: 'dir', size: 0, mtime: '2026-09-25 11:30' },
  ],
  '/var/www/logs': [
    { name: 'access.log', type: 'file', size: 2516582, mtime: '2026-10-01 10:15' },
    { name: 'error.log', type: 'file', size: 131072, mtime: '2026-10-01 10:14' },
  ],
  '/home': [
    { name: 'deploy', type: 'dir', size: 0, mtime: '2026-09-20 10:00' },
  ],
  '/home/deploy': [
    { name: '.bashrc', type: 'file', size: 3106, mtime: '2026-08-15 10:00' },
    { name: 'scripts', type: 'dir', size: 0, mtime: '2026-09-28 14:20' },
  ],
}

type SortKey = 'name' | 'size' | 'mtime'
interface SortState { key: SortKey; dir: 1 | -1 }

const localPath = ref('/Users/dev')
const remotePath = ref('/var/www')
const localSelected = ref<Set<string>>(new Set())
const remoteSelected = ref<Set<string>>(new Set())
const localSort = ref<SortState>({ key: 'name', dir: 1 })
const remoteSort = ref<SortState>({ key: 'name', dir: 1 })

function listLocal(p: string): FsEntry[] { return LOCAL_FS[p] || [] }
function listRemote(p: string): FsEntry[] { return REMOTE_FS[p] || [] }

function joinPath(dir: string, name: string) {
  if (dir === '/') return '/' + name
  return dir + '/' + name
}
function parentPath(path: string) {
  if (path === '/') return '/'
  const i = path.lastIndexOf('/')
  if (i === 0) return '/'
  return path.slice(0, i)
}
function sortEntries(entries: FsEntry[], sort: SortState): FsEntry[] {
  return [...entries].sort((a, b) => {
    if (a.type !== b.type) return a.type === 'dir' ? -1 : 1
    let r = 0
    if (sort.key === 'name') r = a.name.localeCompare(b.name)
    if (sort.key === 'size') r = a.size - b.size
    if (sort.key === 'mtime') r = a.mtime.localeCompare(b.mtime)
    return r * sort.dir
  })
}
function fmtSize(bytes: number) {
  if (!bytes) return '—'
  if (bytes < 1024) return bytes + ' B'
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB'
  if (bytes < 1024 * 1024 * 1024) return (bytes / 1024 / 1024).toFixed(1) + ' MB'
  return (bytes / 1024 / 1024 / 1024).toFixed(2) + ' GB'
}
function crumbs(path: string) {
  const parts = path.split('/').filter(Boolean)
  const out: { label: string; path: string }[] = [{ label: '/', path: '/' }]
  let acc = ''
  parts.forEach(p => { acc += '/' + p; out.push({ label: p, path: acc }) })
  return out
}

const localEntries = computed(() => sortEntries(listLocal(localPath.value), localSort.value))
// 远程目录绑定活动会话：无会话时不显示残留数据
const remoteEntries = computed(() =>
  connected.value ? sortEntries(listRemote(remotePath.value), remoteSort.value) : [],
)

interface Transfer { id: number; name: string; dir: 'up' | 'down'; size: number; pct: number; done: boolean }
const transfers = ref<Transfer[]>([])
let seq = 0
let timers: ReturnType<typeof setInterval>[] = []

onUnmounted(() => timers.forEach(clearInterval))

/* TODO: 接入后端后替换为 invoke('sftp_upload'/'sftp_download') */
function startTask(dir: 'up' | 'down', name: string, size: number) {
  const id = ++seq
  transfers.value.push({ id, name, dir, size, pct: 0, done: false })
  pushLog('INFO', `SFTP ${dir === 'up' ? '上传' : '下载'}开始：${name}`)
  toast(`开始${dir === 'up' ? '上传' : '下载'}：${name}`, 'info', 1700)

  const timer = setInterval(() => {
    const t = transfers.value.find(x => x.id === id)
    if (!t) return
    t.pct += Math.random() * 16 + 7
    if (t.pct >= 100) {
      t.pct = 100
      t.done = true
      clearInterval(timer)
      pushLog('INFO', `SFTP ${dir === 'up' ? '上传' : '下载'}完成：${name} · 校验通过`)
      toast(`${dir === 'up' ? '上传' : '下载'}完成：${name}`, 'ok', 2000)
      setTimeout(() => {
        transfers.value = transfers.value.filter(x => x.id !== id)
      }, 3200)
    }
  }, 240)
  timers.push(timer)
}

function selectedFiles(side: 'local' | 'remote'): FsEntry[] {
  const path = side === 'local' ? localPath.value : remotePath.value
  const list = side === 'local' ? listLocal(path) : listRemote(path)
  const sel = side === 'local' ? localSelected.value : remoteSelected.value
  return list.filter(e => sel.has(e.name) && e.type === 'file')
}

function onUpload() {
  if (!activeHost.value) { toast('请先建立 SSH 连接', 'warn'); return }
  const files = selectedFiles('local')
  if (!files.length) { toast('请先在左侧选择文件', 'warn'); return }
  files.forEach(f => startTask('up', f.name, f.size))
  localSelected.value = new Set()
}
function onDownload() {
  if (!activeHost.value) { toast('请先建立 SSH 连接', 'warn'); return }
  const files = selectedFiles('remote')
  if (!files.length) { toast('请先在右侧选择文件', 'warn'); return }
  files.forEach(f => startTask('down', f.name, f.size))
  remoteSelected.value = new Set()
}

/* 行交互：单击选择（Ctrl/⌘ 多选）、双击目录进入、双击文件传输 */
function onRowClick(side: 'local' | 'remote', entry: FsEntry, e: MouseEvent) {
  const sel = side === 'local' ? localSelected : remoteSelected
  if (e.ctrlKey || e.metaKey) {
    const next = new Set(sel.value)
    next.has(entry.name) ? next.delete(entry.name) : next.add(entry.name)
    sel.value = next
  } else {
    sel.value = new Set([entry.name])
  }
}
function onRowDblClick(side: 'local' | 'remote', entry: FsEntry) {
  const sel = side === 'local' ? localSelected : remoteSelected
  if (entry.type === 'dir') {
    if (side === 'local') localPath.value = joinPath(localPath.value, entry.name)
    else remotePath.value = joinPath(remotePath.value, entry.name)
    sel.value = new Set()
  } else {
    sel.value = new Set([entry.name])
    if (side === 'local') onUpload()
    else onDownload()
  }
}

function goUp(side: 'local' | 'remote') {
  if (side === 'local') { localPath.value = parentPath(localPath.value); localSelected.value = new Set() }
  else { remotePath.value = parentPath(remotePath.value); remoteSelected.value = new Set() }
}
function gotoCrumb(side: 'local' | 'remote', path: string) {
  if (side === 'local') { localPath.value = path; localSelected.value = new Set() }
  else { remotePath.value = path; remoteSelected.value = new Set() }
}
function changeSort(side: 'local' | 'remote', key: SortKey) {
  const s = side === 'local' ? localSort : remoteSort
  if (s.value.key === key) s.value.dir = (s.value.dir * -1) as 1 | -1
  else s.value = { key, dir: 1 }
}

/* ---- Dock tab 切换（状态在 store，供顶部工具栏等外部入口同步） ---- */
const activeTab = dockTab
function switchTab(tab: 'sftp' | 'log') {
  activeTab.value = tab
  expandDock() // 折叠状态下点击 tab 时一并展开
}

/* ---- SSH 运行日志（纯界面 Mock；接入后端后替换为 SSH 事件监听） ---- */
type LogLevel = 'INFO' | 'WARN' | 'ERROR' | 'DEBUG'
interface LogEntry { id: number; time: string; level: LogLevel; msg: string }

let logSeq = 0
function nowTime() {
  return new Date().toLocaleTimeString('zh-CN', { hour12: false })
}
function pushLog(level: LogLevel, msg: string) {
  logs.value.push({ id: ++logSeq, time: nowTime(), level, msg })
  // 限制最大条数，避免长时间运行后无限增长
  if (logs.value.length > 500) logs.value.splice(0, logs.value.length - 500)
}

const LOG_SEED: LogEntry[] = [
  { id: 1, time: '2023-09-30 09:41:02', level: 'INFO', msg: 'SSH 客户端初始化：russh 0.46 · 加密算法集 default' },
  { id: 2, time: '2023-09-30 09:41:02', level: 'INFO', msg: '正在连接 192.168.1.10:22（超时 10s）…' },
  { id: 3, time: '2023-09-30 09:41:02', level: 'INFO', msg: 'TCP 连接已建立，RTT 23ms' },
  { id: 4, time: '2023-09-30 09:41:02', level: 'INFO', msg: '远程 Banner：SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13' },
  { id: 5, time: '2023-09-30 09:41:02', level: 'DEBUG', msg: '本地 Banner：SSH-2.0-rhost_0.1.0' },
  { id: 6, time: '2023-09-30 09:41:03', level: 'INFO', msg: '密钥交换完成：curve25519-sha256 · ssh-ed25519 · chacha20-poly1305' },
  { id: 7, time: '2023-09-30 09:41:03', level: 'DEBUG', msg: '主机密钥指纹 SHA256:k2X9…Q7nE 与 known_hosts 记录匹配' },
  { id: 8, time: '2023-09-30 09:41:03', level: 'INFO', msg: '公钥认证成功：~/.ssh/id_ed25519（用户 deploy）' },
  { id: 9, time: '2023-09-30 09:41:03', level: 'INFO', msg: 'SSH 会话已建立 · 通道 #0（session）' },
  { id: 10, time: '2023-09-30 09:41:04', level: 'INFO', msg: '请求伪终端：xterm-256color 120×30 · LANG=zh_CN.UTF-8' },
  { id: 11, time: '2023-09-30 09:41:04', level: 'INFO', msg: 'SFTP 子系统已启动 · 通道 #1（subsystem: sftp）' },
  { id: 12, time: '2023-09-30 09:41:05', level: 'DEBUG', msg: 'keepalive@openssh.com 心跳间隔 30s' },
  { id: 13, time: '2023-09-30 09:41:35', level: 'DEBUG', msg: '发送 keepalive 心跳包 seq=1，RTT 21ms' },
  { id: 14, time: '2023-09-30 09:42:02', level: 'INFO', msg: 'SFTP 上传开始：deploy.sh → /var/www/deploy.sh（2.0 KB）' },
  { id: 15, time: '2023-09-30 09:42:03', level: 'WARN', msg: '远程文件已存在，执行覆盖并保留权限 0755' },
  { id: 16, time: '2023-09-30 09:42:03', level: 'INFO', msg: 'SFTP 上传完成：deploy.sh · 780 KB/s · 耗时 0.8s' },
  { id: 17, time: '2023-09-30 09:43:10', level: 'WARN', msg: 'keepalive 响应延迟 1840ms，疑似网络抖动' },
  { id: 18, time: '2023-09-30 09:43:41', level: 'ERROR', msg: '端口转发建立失败：L :3306 → 127.0.0.1:3306 · Connection refused' },
  { id: 19, time: '2023-09-30 09:43:41', level: 'INFO', msg: '转发规则已回滚，10s 后自动重试' },
  { id: 20, time: '2023-09-30 09:43:51', level: 'INFO', msg: '端口转发已建立：L :3306 → 127.0.0.1:3306 · 通道 #2' },
]
logSeq = LOG_SEED.length
const logs = ref<LogEntry[]>([...LOG_SEED])

const LEVEL_FILTERS: Array<{ key: LogLevel | 'ALL'; label: string }> = [
  { key: 'ALL', label: '全部' },
  { key: 'INFO', label: 'INFO' },
  { key: 'WARN', label: 'WARN' },
  { key: 'ERROR', label: 'ERROR' },
  { key: 'DEBUG', label: 'DEBUG' },
]
const levelFilter = ref<LogLevel | 'ALL'>('ALL')
const filteredLogs = computed(() =>
  // 始终返回新数组：ALL 时也 slice()，否则原地 push 不改变引用，watch 不会触发
  levelFilter.value === 'ALL'
    ? logs.value.slice()
    : logs.value.filter(l => l.level === levelFilter.value),
)
const levelCounts = computed(() => {
  const c: Record<LogLevel, number> = { INFO: 0, WARN: 0, ERROR: 0, DEBUG: 0 }
  for (const l of logs.value) c[l.level]++
  return c
})

const autoScroll = ref(true)
const logListEl = ref<HTMLElement | null>(null)

const STICK_GAP = 24 // 距底部小于该值视为"贴底"
function isAtBottom() {
  const el = logListEl.value
  return !!el && el.scrollHeight - el.scrollTop - el.clientHeight < STICK_GAP
}
function scrollLogToBottom() {
  const el = logListEl.value
  if (el) el.scrollTop = el.scrollHeight
}
// 手动上滚查看历史 → 自动暂停跟随；滚回底部 → 自动恢复跟随
function onLogScroll() {
  autoScroll.value = isAtBottom()
}
// 点击按钮：跟随中 → 暂停跟随；暂停中 → 立即回到底部并恢复跟随
function toggleAutoScroll() {
  if (autoScroll.value) {
    autoScroll.value = false
  } else {
    autoScroll.value = true
    scrollLogToBottom()
  }
}
watch(filteredLogs, async () => {
  // 搜索状态下由匹配跳转控制滚动位置，不强制贴底
  if (activeTab.value === 'log' && autoScroll.value && !searchActive.value) {
    await nextTick()
    scrollLogToBottom()
  }
})
watch(activeTab, async (tab) => {
  if (tab === 'log') {
    await nextTick()
    // 切回日志 tab 时按当前跟随状态决定位置
    if (autoScroll.value) scrollLogToBottom()
  }
})
function clearLogs() {
  logs.value = []
}

/* ---- 日志搜索（关键字高亮 + 上/下匹配跳转，可与级别筛选叠加） ---- */
const searchVisible = ref(false)
const keyword = ref('')
const caseSensitive = ref(false)
const currentMatchIdx = ref(0)
const searchInputEl = ref<HTMLInputElement | null>(null)

const searchActive = computed(() => searchVisible.value && keyword.value.length > 0)

function matchText(msg: string) {
  const kw = keyword.value
  if (!kw) return false
  return caseSensitive.value ? msg.includes(kw) : msg.toLowerCase().includes(kw.toLowerCase())
}

// 当前级别筛选结果内的全部匹配行 id
const matchIds = computed(() =>
  searchActive.value ? filteredLogs.value.filter(l => matchText(l.msg)).map(l => l.id) : [],
)
const currentMatchId = computed(() => matchIds.value[currentMatchIdx.value] ?? null)

async function openSearch() {
  searchVisible.value = true
  await nextTick()
  searchInputEl.value?.focus()
  searchInputEl.value?.select()
}
function closeSearch() {
  searchVisible.value = false
}
// Ctrl/⌘+F 全局唤起搜索（自动切到日志 tab 并展开面板）
function revealSearch() {
  activeTab.value = 'log'
  expandDock()
  openSearch()
}
function onFindShortcut(e: KeyboardEvent) {
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'f') {
    e.preventDefault()
    revealSearch()
  }
}
// 标题栏搜索按钮通过 searchTick 触发
watch(searchTick, revealSearch)
function gotoMatch(dir: 1 | -1) {
  const n = matchIds.value.length
  if (!n) return
  currentMatchIdx.value = (currentMatchIdx.value + dir + n) % n
}

// 关键字 / 大小写变化：回到第一个匹配；匹配数减少时收敛索引
watch([keyword, caseSensitive], () => { currentMatchIdx.value = 0 })
watch(matchIds, (ids) => {
  if (currentMatchIdx.value > ids.length - 1) {
    currentMatchIdx.value = Math.max(0, ids.length - 1)
  }
})
// 当前匹配行滚入可视区域
watch([currentMatchId, searchVisible], async () => {
  if (!searchActive.value) return
  await nextTick()
  logListEl.value
    ?.querySelector('.log-row.match-current')
    ?.scrollIntoView({ block: 'nearest' })
})

// 将消息文本按匹配位置拆分为片段（不用 v-html，避免日志内容 XSS）
function highlightParts(msg: string): Array<{ text: string; hit: boolean }> {
  const kw = keyword.value
  if (!searchActive.value || !kw) return [{ text: msg, hit: false }]
  const hay = caseSensitive.value ? msg : msg.toLowerCase()
  const needle = caseSensitive.value ? kw : kw.toLowerCase()
  const parts: Array<{ text: string; hit: boolean }> = []
  let i = 0
  let p = hay.indexOf(needle)
  while (p !== -1) {
    if (p > i) parts.push({ text: msg.slice(i, p), hit: false })
    parts.push({ text: msg.slice(p, p + kw.length), hit: true })
    i = p + kw.length
    p = hay.indexOf(needle, i)
  }
  if (i < msg.length) parts.push({ text: msg.slice(i), hit: false })
  return parts
}

/* Mock：模拟 SSH 通道运行期事件（接入后端后删除） */
let mockEventIdx = 0
let heartbeatSeq = 1
const MOCK_EVENTS: Array<() => [LogLevel, string]> = [
  () => ['DEBUG', `发送 keepalive 心跳包 seq=${++heartbeatSeq}，RTT ${18 + Math.floor(Math.random() * 26)}ms`],
  () => ['DEBUG', `通道流量窗口调整：+2097152 字节（通道 #0）`],
  () => ['INFO', `SFTP 读取目录 /var/www/logs（14 项，耗时 ${12 + Math.floor(Math.random() * 36)}ms）`],
  () => ['DEBUG', `加密通道吞吐 ↑ ${(4 + Math.random() * 18).toFixed(1)} KB/s ↓ ${(10 + Math.random() * 52).toFixed(1)} KB/s`],
  () => ['INFO', `端口转发 :3306 当前活动连接 ${1 + Math.floor(Math.random() * 3)} 条`],
]
timers.push(setInterval(() => {
  const [level, msg] = MOCK_EVENTS[mockEventIdx++ % MOCK_EVENTS.length]()
  pushLog(level, msg)
}, 4000))

/* resizer 拖拽 */
const dockEl = ref<HTMLElement | null>(null)
const isResizing = ref(false)
let dragging = false
let pendingY = 0
let rafId = 0

const HEAD_H = 37        // 标题栏高度（与 CSS .dock.collapsed 保持一致），拖拽下限
const MIN_H = 120        // 视为"可用"的最小高度，低于此展开时自动恢复
const COLLAPSE_GAP = 48  // 触底后继续下拉该距离 → 自动折叠，并记住可用高度
const EXPAND_GAP = 24    // 折叠态上拉超过该距离 → 展开；折叠/展开在同一次拖拽内可反复切换
let lastGoodHeight = 286 // 最近一次可用高度，展开时恢复（与 CSS 默认高度一致）

function startDrag(e: MouseEvent) {
  dragging = true
  isResizing.value = true
  pendingY = e.clientY
  if (dockCollapsed.value) {
    // 折叠态：先把内联高度预置为上次可用高度（collapsed class 仍强制 37px），展开瞬间无缝衔接
    if (dockEl.value) dockEl.value.style.height = lastGoodHeight + 'px'
  } else {
    const cur = dockEl.value?.offsetHeight ?? 0
    if (cur >= MIN_H) lastGoodHeight = cur
  }
  document.body.style.userSelect = 'none'
  document.body.style.cursor = 'ns-resize'
}

function endDrag() {
  dragging = false
  isResizing.value = false
  if (rafId) {
    cancelAnimationFrame(rafId)
    rafId = 0
  }
  document.body.style.userSelect = ''
  document.body.style.cursor = ''
}

function applyHeight() {
  rafId = 0
  // 上限动态计算：保底终端区 140px（title 46 + tabs 37 + statusbar 27）
  const h = window.innerHeight - pendingY - 27
  const max = window.innerHeight - 46 - 37 - 27 - 140
  const el = dockEl.value

  if (!dockCollapsed.value) {
    // 已到下限（保留标题栏）仍继续下拉超过阈值 → 自动折叠。
    // 注意：不结束拖拽手势，反向拉回超过展开阈值可在同一次拖拽内立即重新展开
    if (h <= HEAD_H - COLLAPSE_GAP) {
      if (el) el.style.height = lastGoodHeight + 'px'
      dockCollapsed.value = true
      return
    }
    const clamped = Math.min(Math.max(h, HEAD_H), max)
    if (el) el.style.height = clamped + 'px'
    if (clamped >= MIN_H) lastGoodHeight = clamped
    return
  }

  // 折叠态：上拉超过阈值 → 展开，高度继续实时跟随鼠标
  if (h >= HEAD_H + EXPAND_GAP) {
    dockCollapsed.value = false
    const clamped = Math.min(Math.max(h, HEAD_H), max)
    if (el) el.style.height = clamped + 'px'
    if (clamped >= MIN_H) lastGoodHeight = clamped
  }
}

function onMove(e: MouseEvent) {
  if (!dragging) return
  pendingY = e.clientY
  if (!rafId) rafId = requestAnimationFrame(applyHeight)
}
function onUp() {
  if (!dragging) return
  endDrag()
  applyHeight() // 收尾：应用最后一次 pendingY
}

/* 展开 / 折叠 */
function expandDock() {
  if (!dockCollapsed.value) return
  const el = dockEl.value
  const h = el ? parseInt(el.style.height, 10) : NaN
  if (el && (!Number.isFinite(h) || h < MIN_H)) el.style.height = lastGoodHeight + 'px'
  dockCollapsed.value = false
}

function toggleDock() {
  if (dockCollapsed.value) expandDock()
  else dockCollapsed.value = true
}
onMounted(() => {
  document.addEventListener('mousemove', onMove)
  document.addEventListener('mouseup', onUp)
  document.addEventListener('keydown', onFindShortcut)
})
onUnmounted(() => {
  document.removeEventListener('mousemove', onMove)
  document.removeEventListener('mouseup', onUp)
  document.removeEventListener('keydown', onFindShortcut)
})
</script>

<template>
  <section ref="dockEl" class="dock" :class="{ collapsed: dockCollapsed, resizing: isResizing }">
    <div class="dock-resizer" title="拖拽调整高度（下拉到底折叠，折叠时上拉展开）· 双击折叠 / 展开"
         @mousedown.prevent="startDrag" @dblclick="toggleDock"></div>

    <div class="dock-head">

      <button class="dock-tab" :class="{ active: activeTab === 'log' }" title="日志"
              @click="switchTab('log')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round">
          <polyline points="4 17 10 11 4 5"></polyline>
          <line x1="12" y1="19" x2="20" y2="19"></line>
        </svg>
        日志
      </button>

      <button class="dock-tab" :class="{ active: activeTab === 'sftp' }" title="文件管理"
              @click="switchTab('sftp')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round">
          <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
        </svg>
        SFTP
      </button>

      <div class="spacer"></div>

      <button class="dock-toggle" title="收起 / 展开" @click="toggleDock">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
             stroke-linecap="round" stroke-linejoin="round">
          <polyline points="6 9 12 15 18 9"></polyline>
        </svg>
      </button>

      <button class="dock-toggle" title="关闭面板" @click="closeDock">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
             stroke-linecap="round" stroke-linejoin="round">
          <line x1="18" y1="6" x2="6" y2="18"></line>
          <line x1="6" y1="6" x2="18" y2="18"></line>
        </svg>
      </button>
    </div>

    <div class="dock-body">
      <!-- SFTP -->
      <div class="dock-pane" :class="{ show: activeTab === 'sftp' }">
        <div class="sftp">
          <!-- 顶栏 -->
          <div class="sftp-topbar">
            <div class="sftp-title">
              <span class="led" :class="{ off: !connected }"></span>
              SFTP · <span class="dock-badge" :class="{ off: !connected }">{{ connected ? activeHost?.id : '未连接' }}</span>
            </div>
            <div class="sep"></div>
            <button class="sftp-btn" :disabled="!connected"
                    @click="toast('已刷新文件列表', 'info', 1500)">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                   stroke-linecap="round" stroke-linejoin="round">
                <path d="M21 12a9 9 0 11-3-6.7L21 8"/><path d="M21 3v5h-5"/>
              </svg>
              刷新
            </button>
            <div class="spacer"></div>
            <span class="sftp-hint">拖拽文件到对侧即可传输</span>
          </div>

          <!-- 双栏主体 -->
          <div class="sftp-main">
            <!-- 本地 -->
            <div class="pane local">
              <div class="pane-header">
                <span class="pane-tag">本地</span>
                <div class="pane-path">
                  <template v-for="(c, i) in crumbs(localPath)" :key="c.path">
                    <span v-if="i" class="sep">/</span>
                    <span class="crumb" @click="gotoCrumb('local', c.path)">{{ c.label }}</span>
                  </template>
                </div>
                <button class="pane-btn" title="上一级" @click="goUp('local')">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                       stroke-linecap="round" stroke-linejoin="round">
                    <path d="M19 12H5M12 19l-7-7 7-7"/>
                  </svg>
                </button>
                <button class="pane-btn" title="刷新" @click="toast('已刷新', 'info', 1200)">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                       stroke-linecap="round" stroke-linejoin="round">
                    <path d="M21 12a9 9 0 11-3-6.7L21 8"/><path d="M21 3v5h-5"/>
                  </svg>
                </button>
              </div>
              <div class="pane-list">
                <div v-if="!localEntries.length" class="pane-empty">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"
                       stroke-linecap="round" stroke-linejoin="round">
                    <path d="M4 7a2 2 0 012-2h3l2 2h7a2 2 0 012 2v8a2 2 0 01-2 2H6a2 2 0 01-2-2V7z"/>
                  </svg>
                  <div>此文件夹为空</div>
                </div>
                <template v-else>
                  <div class="list-header">
                    <div class="lh-name" :class="{ sorted: localSort.key==='name' }" @click="changeSort('local','name')">名称</div>
                    <div class="lh-size" :class="{ sorted: localSort.key==='size' }" @click="changeSort('local','size')">大小</div>
                    <div class="lh-time" :class="{ sorted: localSort.key==='mtime' }" @click="changeSort('local','mtime')">修改时间</div>
                  </div>
                  <div
                    v-for="e in localEntries"
                    :key="e.name"
                    class="row"
                    :class="{ dir: e.type==='dir', selected: localSelected.has(e.name) }"
                    @click="onRowClick('local', e, $event)"
                    @dblclick="onRowDblClick('local', e)"
                  >
                    <div class="row-name">
                      <svg class="row-icon" viewBox="0 0 24 24">
                        <path v-if="e.type==='dir'" d="M4 7a2 2 0 012-2h3l2 2h7a2 2 0 012 2v8a2 2 0 01-2 2H6a2 2 0 01-2-2V7z"/>
                        <template v-else>
                          <path d="M14 3H6a2 2 0 00-2 2v14a2 2 0 002 2h12a2 2 0 002-2V9z"/>
                          <path d="M14 3v6h6"/>
                        </template>
                      </svg>
                      <span class="row-text">{{ e.name }}</span>
                    </div>
                    <div class="row-size">{{ e.type==='dir' ? '—' : fmtSize(e.size) }}</div>
                    <div class="row-time">{{ e.mtime }}</div>
                  </div>
                </template>
              </div>
            </div>

            <!-- 传输按钮列 -->
            <div class="transfer-col">
              <button class="transfer-btn" :disabled="!connected || !localSelected.size"
                      title="上传选中文件到远程" @click="onUpload">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
                     stroke-linecap="round" stroke-linejoin="round">
                  <path d="M5 12h14M13 5l7 7-7 7"/>
                </svg>
              </button>
              <button class="transfer-btn" :disabled="!connected || !remoteSelected.size"
                      title="下载选中文件到本地" @click="onDownload">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
                     stroke-linecap="round" stroke-linejoin="round">
                  <path d="M19 12H5M11 19l-7-7 7-7"/>
                </svg>
              </button>
            </div>

            <!-- 远程 -->
            <div class="pane remote">
              <div class="pane-header">
                <span class="pane-tag">远程</span>
                <div class="pane-path">
                  <span v-if="!connected" class="crumb dim">未连接</span>
                  <template v-else v-for="(c, i) in crumbs(remotePath)" :key="c.path">
                    <span v-if="i" class="sep">/</span>
                    <span class="crumb" @click="gotoCrumb('remote', c.path)">{{ c.label }}</span>
                  </template>
                </div>
                <button class="pane-btn" title="上一级" :disabled="!connected" @click="goUp('remote')">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                       stroke-linecap="round" stroke-linejoin="round">
                    <path d="M19 12H5M12 19l-7-7 7-7"/>
                  </svg>
                </button>
                <button class="pane-btn" title="刷新" :disabled="!connected" @click="toast('已刷新', 'info', 1200)">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                       stroke-linecap="round" stroke-linejoin="round">
                    <path d="M21 12a9 9 0 11-3-6.7L21 8"/><path d="M21 3v5h-5"/>
                  </svg>
                </button>
              </div>
              <div class="pane-list">
                <div v-if="!remoteEntries.length" class="pane-empty">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"
                       stroke-linecap="round" stroke-linejoin="round">
                    <path d="M4 7a2 2 0 012-2h3l2 2h7a2 2 0 012 2v8a2 2 0 01-2 2H6a2 2 0 01-2-2V7z"/>
                  </svg>
                  <div>{{ connected ? '此文件夹为空' : '未连接到远程主机' }}</div>
                </div>
                <template v-else>
                  <div class="list-header">
                    <div class="lh-name" :class="{ sorted: remoteSort.key==='name' }" @click="changeSort('remote','name')">名称</div>
                    <div class="lh-size" :class="{ sorted: remoteSort.key==='size' }" @click="changeSort('remote','size')">大小</div>
                    <div class="lh-time" :class="{ sorted: remoteSort.key==='mtime' }" @click="changeSort('remote','mtime')">修改时间</div>
                  </div>
                  <div
                    v-for="e in remoteEntries"
                    :key="e.name"
                    class="row"
                    :class="{ dir: e.type==='dir', selected: remoteSelected.has(e.name) }"
                    @click="onRowClick('remote', e, $event)"
                    @dblclick="onRowDblClick('remote', e)"
                  >
                    <div class="row-name">
                      <svg class="row-icon" viewBox="0 0 24 24">
                        <path v-if="e.type==='dir'" d="M4 7a2 2 0 012-2h3l2 2h7a2 2 0 012 2v8a2 2 0 01-2 2H6a2 2 0 01-2-2V7z"/>
                        <template v-else>
                          <path d="M14 3H6a2 2 0 00-2 2v14a2 2 0 002 2h12a2 2 0 002-2V9z"/>
                          <path d="M14 3v6h6"/>
                        </template>
                      </svg>
                      <span class="row-text">{{ e.name }}</span>
                    </div>
                    <div class="row-size">{{ e.type==='dir' ? '—' : fmtSize(e.size) }}</div>
                    <div class="row-time">{{ e.mtime }}</div>
                  </div>
                </template>
              </div>
            </div>
          </div>

          <!-- 传输任务 -->
          <div v-if="transfers.length" class="sftp-tasks">
            <div
              v-for="t in transfers"
              :key="t.id"
              class="sftp-task"
              :class="[t.dir==='up' ? 'upload' : 'download', { done: t.done }]"
            >
              <span class="icon">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                     stroke-linecap="round" stroke-linejoin="round">
                  <path v-if="t.dir==='up'" d="M12 19V5M5 12l7-7 7 7"/>
                  <path v-else d="M12 5v14M5 12l7 7 7-7"/>
                </svg>
              </span>
              <span class="name">{{ t.name }}</span>
              <span class="dir-tag">{{ t.dir==='up' ? '本地 → 远程' : '远程 → 本地' }}</span>
              <span class="bar"><span class="bar-fill" :style="{ width: t.pct + '%' }"></span></span>
              <span class="pct">{{ t.done ? '✓' : Math.floor(t.pct) + '%' }}</span>
            </div>
          </div>

          <!-- 状态栏 -->
          <div class="sftp-footer">
            <span>
              本地 <b>{{ localEntries.filter(e=>e.type==='dir').length }} 目录 / {{ localEntries.filter(e=>e.type==='file').length }} 文件</b>
              ·
              远程 <b>{{ remoteEntries.filter(e=>e.type==='dir').length }} 目录 / {{ remoteEntries.filter(e=>e.type==='file').length }} 文件</b>
            </span>
            <span class="stat">
              <span v-if="localSelected.size">本地选中 {{ localSelected.size }}</span>
              <span v-if="localSelected.size && remoteSelected.size"> · </span>
              <span v-if="remoteSelected.size">远程选中 {{ remoteSelected.size }}</span>
            </span>
          </div>
        </div>
      </div>

      <!-- SSH 运行日志 -->
      <div class="dock-pane" :class="{ show: activeTab === 'log' }">
        <div class="log-bar">
          <button
            v-for="f in LEVEL_FILTERS"
            :key="f.key"
            class="log-filter"
            :class="{ active: levelFilter === f.key }"
            @click="levelFilter = f.key"
          >
            {{ f.label }}
            <span class="cnt">{{ f.key === 'ALL' ? logs.length : levelCounts[f.key] }}</span>
          </button>

          <span class="spacer"></span>

          <button class="mini-ico" :class="{ active: searchVisible }" title="搜索日志 (Ctrl/⌘+F)"
                  @click="openSearch">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                 stroke-linecap="round" stroke-linejoin="round">
              <circle cx="11" cy="11" r="7"></circle>
              <line x1="21" y1="21" x2="16.65" y2="16.65"></line>
            </svg>
          </button>
          <button class="mini-ico" :class="{ active: autoScroll }"
                  :title="autoScroll ? '自动跟随最新日志（点击暂停）' : '已暂停跟随（点击回到底部）'"
                  @click="toggleAutoScroll">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                 stroke-linecap="round" stroke-linejoin="round">
              <polyline points="7 13 12 18 17 13"></polyline>
              <polyline points="7 6 12 11 17 6"></polyline>
            </svg>
          </button>
          <button class="mini-ico" title="清空日志" @click="clearLogs">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                 stroke-linecap="round" stroke-linejoin="round">
              <polyline points="3 6 5 6 21 6"></polyline>
              <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"></path>
              <line x1="10" y1="11" x2="10" y2="17"></line>
              <line x1="14" y1="11" x2="14" y2="17"></line>
            </svg>
          </button>
        </div>

        <div v-show="searchVisible" class="log-search">
          <svg class="ls-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
               stroke-linecap="round" stroke-linejoin="round">
            <circle cx="11" cy="11" r="7"></circle>
            <line x1="21" y1="21" x2="16.65" y2="16.65"></line>
          </svg>
          <input
            ref="searchInputEl"
            v-model="keyword"
            class="ls-input"
            :class="{ nomatch: searchActive && !matchIds.length }"
            type="text"
            placeholder="搜索日志内容…"
            @keydown.enter.exact.prevent="gotoMatch(1)"
            @keydown.shift.enter.prevent="gotoMatch(-1)"
            @keydown.esc.prevent="closeSearch"
          >
          <span class="ls-count" :class="{ zero: searchActive && !matchIds.length }">
            <template v-if="searchActive">
              {{ matchIds.length ? currentMatchIdx + 1 : 0 }} / {{ matchIds.length }}
            </template>
          </span>
          <button class="ls-btn" :class="{ on: caseSensitive }" title="区分大小写"
                  @click="caseSensitive = !caseSensitive">Aa</button>
          <button class="ls-btn" title="上一个匹配 (Shift+Enter)" @click="gotoMatch(-1)">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
                 stroke-linecap="round" stroke-linejoin="round">
              <polyline points="18 15 12 9 6 15"></polyline>
            </svg>
          </button>
          <button class="ls-btn" title="下一个匹配 (Enter)" @click="gotoMatch(1)">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
                 stroke-linecap="round" stroke-linejoin="round">
              <polyline points="6 9 12 15 18 9"></polyline>
            </svg>
          </button>
          <button class="ls-btn" title="关闭 (Esc)" @click="closeSearch">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
                 stroke-linecap="round" stroke-linejoin="round">
              <line x1="18" y1="6" x2="6" y2="18"></line>
              <line x1="6" y1="6" x2="18" y2="18"></line>
            </svg>
          </button>
        </div>

        <div ref="logListEl" class="log-list" @scroll="onLogScroll">
          <div v-if="!filteredLogs.length" class="log-empty">暂无日志</div>
          <div
            v-for="l in filteredLogs"
            :key="l.id"
            class="log-row"
            :class="[l.level, {
              'match-current': searchActive && l.id === currentMatchId,
            }]"
          >
            <span class="log-time">{{ l.time }}</span>
            <span class="log-level">{{ l.level }}</span>
            <span class="log-msg">
              <template v-for="(seg, i) in highlightParts(l.msg)" :key="i">
                <mark v-if="seg.hit" class="log-hit"
                      :class="{ current: l.id === currentMatchId }">{{ seg.text }}</mark>
                <template v-else>{{ seg.text }}</template>
              </template>
            </span>
          </div>
        </div>
      </div>
    </div>
  </section>
</template>
