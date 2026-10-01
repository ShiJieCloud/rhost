import { computed, reactive, ref } from 'vue'
import type { Host } from '../types'
import { hosts } from './hosts'

export type SessionState = 'idle' | 'connecting' | 'online' | 'reconnecting' | 'offline'
export type AppView = 'home' | 'workbench'

export interface Session {
  id: string
  host: Host
  state: SessionState
  startedAt: number
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
export const inspectorVisible = ref(true)
export const dockCollapsed = ref(false)
export const dockVisible = ref(true)
/** Dock 当前激活页签 */
export const dockTab = ref<'sftp' | 'log'>('sftp')

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

/* ---- 会话管理 ---- */

/* ---- 会话持久化（冷/热启动分离） ---- */
const SESSIONS_KEY = 'rhost.sessions'

function persistSessions() {
  try {
    localStorage.setItem(SESSIONS_KEY, JSON.stringify(sessions.value.map(s => s.id)))
  } catch { /* 隐私模式等场景忽略 */ }
}

/** 启动时恢复上次未关闭的会话；有则直接进入工作台并自动重连（热启动） */
export function restoreSessions() {
  let ids: string[] = []
  try {
    ids = JSON.parse(localStorage.getItem(SESSIONS_KEY) ?? '[]')
  } catch { /* 数据损坏按无会话处理 */ }
  const valid = ids.filter(id => hosts.value.some(h => h.id === id))
  if (!valid.length) return
  sessions.value = valid.map(id => ({
    id,
    host: hosts.value.find(h => h.id === id)!,
    state: 'connecting' as SessionState,
    startedAt: Date.now(),
  }))
  activeSessionId.value = valid[valid.length - 1]
  appView.value = 'workbench'
}

export function openSession(hostId: string) {
  const host = hosts.value.find(h => h.id === hostId)
  if (!host) return
  if (host.status === 'offline') return
  appView.value = 'workbench'
  activeSessionId.value = hostId
  if (!sessions.value.some(s => s.id === hostId)) {
    sessions.value.push({ id: hostId, host, state: 'connecting', startedAt: Date.now() })
  }
  persistSessions()
}

export function closeSession(id: string) {
  const idx = sessions.value.findIndex(s => s.id === id)
  if (idx === -1) return
  sessions.value.splice(idx, 1)
  if (activeSessionId.value === id) {
    const rest = sessions.value
    // 关闭最后一个 Tab 留在工作台空态，不回首页
    activeSessionId.value = rest.length ? rest[rest.length - 1].id : null
  }
  persistSessions()
}

export function setSessionState(id: string, state: SessionState) {
  const s = sessions.value.find(x => x.id === id)
  if (s) s.state = state
}

/* ---- 资源监控模拟（TODO: 接入后端后由事件流替换） ---- */
export interface ProcItem {
  pid: number
  cmd: string
  cpu: number
  mem: number
}

export const metrics = reactive({
  cpu: 0,
  mem: 0,
  disk: 0,
  tx: 0,
  rx: 0,
  procs: [] as ProcItem[],
})

const PROCS_BASE = [
  { pid: 1182, cmd: '/usr/sbin/sshd -D', cpu: 0.3, mem: 0.4 },
  { pid: 1284, cmd: 'nginx: worker process', cpu: 8.6, mem: 6.2 },
  { pid: 2103, cmd: 'postgres: writer process', cpu: 3.1, mem: 11.8 },
  { pid: 3450, cmd: 'redis-server *:6379', cpu: 5.7, mem: 4.3 },
  { pid: 5821, cmd: 'node /app/server.js', cpu: 14.2, mem: 9.6 },
  { pid: 7734, cmd: 'python3 monitor.py', cpu: 1.8, mem: 2.1 },
] as ProcItem[]

metrics.procs = PROCS_BASE.map(p => ({ ...p }))

let monitorTimer: ReturnType<typeof setInterval> | null = null

export function startMonitor() {
  if (monitorTimer) return
  monitorTimer = setInterval(() => {
    if (!connected.value) {
      metrics.cpu = 0
      metrics.mem = 0
      metrics.disk = 0
      metrics.tx = 0
      metrics.rx = 0
      return
    }
    metrics.cpu = Math.min(96, Math.max(6, metrics.cpu + (Math.random() - 0.5) * 14))
    metrics.mem = Math.min(92, Math.max(18, metrics.mem + (Math.random() - 0.5) * 5))
    metrics.disk = Math.min(88, Math.max(38, metrics.disk + (Math.random() - 0.5) * 0.8))
    metrics.tx = Math.max(0, metrics.tx + (Math.random() - 0.45) * 180)
    metrics.rx = Math.max(0, metrics.rx + (Math.random() - 0.45) * 420)
    metrics.procs.forEach(p => {
      p.cpu = Math.max(0.1, p.cpu + (Math.random() - 0.5) * 2.4)
      p.mem = Math.max(0.1, p.mem + (Math.random() - 0.5) * 0.7)
    })
  }, 2000)
}

export function barClass(v: number) {
  if (v >= 80) return 'bar-red'
  if (v >= 60) return 'bar-yellow'
  return 'bar-green'
}
