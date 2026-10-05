<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { invoke, Channel } from '@tauri-apps/api/core'
import { activeHost, activeSession, activeSftpCwd, closeDock, connected, dockCollapsed, dockHeight, DOCK_DEFAULT_HEIGHT, dockTab, dockVisible, searchTick, sftpLocalRatio } from '../../stores/session'
import { savedSettings } from '../../stores/settings'
import { toast } from '../../composables/useToast'
import { isTauri } from '../../lib/tauri'
import ContextMenu from './ContextMenu.vue'
import type { MenuItem } from './ContextMenu.vue'
import {
  logs, subscribe as subscribeAppLogs, clear as clearAppLogs, revealLogDir,
  report as reportLog,
  SFTP_TRANSFER_ENQUEUE, SFTP_TRANSFER_START, SFTP_TRANSFER_COMPLETE,
  SFTP_TRANSFER_FAILED, SFTP_TRANSFER_CANCEL, SFTP_TRANSFER_PAUSE,
  SFTP_TRANSFER_RESUME, SFTP_MANAGE_REMOVE,
} from '../../stores/applog'
import type { LogLevel } from '../../stores/applog'

type Side = 'local' | 'remote'

/* ---- SFTP 文件系统（本地/远程均为后端真实数据） ---- */
interface FsEntry {
  name: string
  type: 'dir' | 'file'
  size: number
  mtime: string
  perm: string      // e.g. "rwxr-xr-x"
  owner: string     // e.g. "root/root"
  isSymlink: boolean
}

/* 后端 list_local_dir / sftp_list_dir 返回结构（camelCase，两侧同构） */
interface DirListing {
  path: string
  entries: Array<{
    name: string
    isDir: boolean
    isSymlink: boolean
    size: number
    mtime: string
    perm: string
    owner: string
  }>
}

type SortKey = 'name' | 'size' | 'mtime'
interface SortState { key: SortKey; dir: 1 | -1 }

const localPath = ref('/')
const remotePath = ref('/')

/* ---- 行内编辑（新建文件夹/新建文件/重命名统一一个状态，不弹窗） ---- */
type EditKind = 'mkdir' | 'mkfile' | 'rename'
interface EditState { side: Side; kind: EditKind; origName?: string }
const edit = ref<EditState | null>(null)
const editName = ref('')
const editInput = ref<HTMLInputElement | null>(null)

/** 开始编辑：新建类名空；重命名回填原名并全选 */
function startEdit(side: Side, kind: EditKind, target?: FsEntry) {
  if (kind === 'rename' && target) {
    edit.value = { side, kind, origName: target.name }
    editName.value = target.name
  } else {
    edit.value = { side, kind }
    editName.value = ''
  }
  nextTick(() => {
    editInput.value?.focus()
    if (kind === 'rename') editInput.value?.select()
  })
}

/** 确认编辑：校验名称/重名 → 调后端 → 刷新；空名/未改名直接收起 */
async function confirmEdit() {
  const st = edit.value
  if (!st) return
  const name = editName.value.trim()
  edit.value = null
  if (!name) return
  if (st.kind === 'rename' && name === st.origName) return
  if (name.includes('/')) {
    toast('名称不能包含 /', 'warn')
    return
  }
  if (name === '.' || name === '..') {
    toast('名称不能为 . 或 ..', 'warn')
    return
  }
  if (st.kind !== 'rename' && entryExists(st.side, name)) {
    toast('已存在同名项', 'warn')
    return
  }
  const base = sidePath(st.side)
  try {
    if (st.kind === 'rename') {
      const oldPath = joinPath(base, st.origName!)
      const newPath = joinPath(base, name)
      await invokeSide(st.side, 'rename', { oldPath, newPath })
    } else if (st.kind === 'mkdir') {
      await invokeSide(st.side, 'mkdir', { path: joinPath(base, name) })
    } else {
      await invokeSide(st.side, 'create_file', { path: joinPath(base, name) })
    }
    void refreshSide(st.side)
  } catch (e) {
    const verb = st.kind === 'rename' ? '重命名' : st.kind === 'mkdir' ? '新建文件夹' : '新建文件'
    toast(`${verb}失败：${(e as Error).message}`, 'warn')
  }
}

/** Esc 取消（edit 先置空，随后的 blur 即 no-op） */
function cancelEdit() {
  edit.value = null
}

/* ---- 文件剪贴板（复制/剪切；记录复制瞬间的源目录绝对路径与元数据，允许随后导航） ---- */
const clipboard = ref<{
  srcSide: Side
  base: string
  mode: 'copy' | 'cut'
  items: { name: string; isDir: boolean; size: number }[]
} | null>(null)

/* ---- 右键菜单状态：事件入口一次性写全坐标与目标，菜单项由 computed 生成 ---- */
const menu = ref<{ x: number; y: number; side: Side; targets: FsEntry[] } | null>(null)

/* 最后交互的一侧：快捷键作用域（点击/右键/导航时更新） */
const lastSide = ref<Side>('local')

const localSelected = ref<Set<string>>(new Set())
const remoteSelected = ref<Set<string>>(new Set())
const localSort = ref<SortState>({ key: 'name', dir: 1 })
const remoteSort = ref<SortState>({ key: 'name', dir: 1 })

/* 本地目录真实数据：由后端 list_local_dir 加载；localPath 始终对应已加载成功的目录 */
const localRaw = ref<FsEntry[]>([])
const localLoading = ref(false)

async function loadLocal(path?: string) {
  if (!isTauri) return // 浏览器 dev 模式无本地 FS，保持空列表
  localLoading.value = true
  try {
    const res = await invoke<DirListing>('list_local_dir', { path: path ?? null })
    localRaw.value = res.entries.map(toFsEntry)
    localPath.value = res.path
    localSelected.value = new Set()
  } catch (e) {
    toast(String(e), 'warn', 2200)
  } finally {
    localLoading.value = false
  }
}

/* 远端目录真实数据：由后端 sftp_list_dir 经会话 SFTP 子系统加载。
   remotePath 始终对应已加载成功的目录；请求失败时停留在原目录。 */
const remoteRaw = ref<FsEntry[]>([])
const remoteLoading = ref(false)
let remoteReqSeq = 0 // 竞态守卫：快速切目录/切会话时丢弃过期响应

async function loadRemote(path?: string) {
  const backendId = activeSession.value?.backendId
  if (!isTauri || !backendId) return // 浏览器 dev 模式 / 未连接：保持空列表
  const req = ++remoteReqSeq
  remoteLoading.value = true
  try {
    const res = await invoke<DirListing>('sftp_list_dir', {
      sessionId: backendId,
      path: path ?? null,
    })
    // 过期响应（用户已继续导航或切换/断开会话）直接丢弃，避免覆盖新视图
    if (req !== remoteReqSeq || activeSession.value?.backendId !== backendId) return
    remoteRaw.value = res.entries.map(toFsEntry)
    remotePath.value = res.path
    remoteSelected.value = new Set()
  } catch (e) {
    if (req !== remoteReqSeq) return
    toast(String(e), 'warn', 2600)
  } finally {
    if (req === remoteReqSeq) remoteLoading.value = false
  }
}

/** 后端目录项 → 前端通用 FsEntry（本地/远程共用） */
function toFsEntry(e: DirListing['entries'][number]): FsEntry {
  return {
    name: e.name,
    type: e.isDir ? 'dir' : 'file',
    size: e.size,
    mtime: e.mtime,
    perm: e.perm,
    owner: e.owner,
    isSymlink: e.isSymlink,
  }
}

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

/* ---- 按侧（local/remote）统一访问路径/原始列表/选择集/刷新 ---- */
function sidePath(side: Side) { return side === 'local' ? localPath.value : remotePath.value }
function sideRaw(side: Side) { return side === 'local' ? localRaw : remoteRaw }
function sideSel(side: Side) { return side === 'local' ? localSelected : remoteSelected }
function refreshSide(side: Side) {
  return side === 'local' ? loadLocal(localPath.value) : loadRemote(remotePath.value)
}
function entryExists(side: Side, name: string) {
  return sideRaw(side).value.some(e => e.name === name)
}

/**
 * 在目标侧为重名项分配一个不存在的候选名：
 *   a.txt → a (1).txt → a (2).txt；a.tar.gz → a.tar (1).gz
 *   无扩展名或纯点开头（.bashrc）→ name (1)
 * `used` 为本批粘贴已占用的名字（尚未落盘，列表里查不到）。
 */
function nextFreeName(side: Side, name: string, used: Set<string>) {
  const dot = name.lastIndexOf('.')
  // dot > 0：".bashrc" 的点在首位，按无扩展名处理
  const stem = dot > 0 ? name.slice(0, dot) : name
  const ext = dot > 0 ? name.slice(dot) : ''
  let n = 1
  for (;;) {
    const candidate = `${stem} (${n})${ext}`
    if (!used.has(candidate) && !entryExists(side, candidate)) return candidate
    n++
  }
}

/**
 * 统一派发按侧区分的后端命令：远程命令自动带 sessionId。
 * kind → 本地/远程命令名：
 *   mkdir       mkdir / sftp_mkdir
 *   create_file local_create_file / sftp_create_file
 *   rename      local_rename / sftp_rename
 *   remove      local_remove / sftp_remove
 *   copy        local_copy / sftp_copy
 */
async function invokeSide(
  side: Side,
  kind: 'mkdir' | 'create_file' | 'rename' | 'remove' | 'copy',
  args: Record<string, string>,
) {
  if (side === 'local') {
    const cmd = ({ mkdir: 'mkdir', create_file: 'local_create_file', rename: 'local_rename', remove: 'local_remove', copy: 'local_copy' } as const)[kind]
    await invoke(cmd, args)
  } else {
    const cmd = ({ mkdir: 'sftp_mkdir', create_file: 'sftp_create_file', rename: 'sftp_rename', remove: 'sftp_remove', copy: 'sftp_copy' } as const)[kind]
    const backendId = activeSession.value?.backendId
    if (!backendId) throw new Error('未连接')
    await invoke(cmd, { sessionId: backendId, ...args })
  }
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
  if (bytes < 1024) return Math.round(bytes) + ' B'
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB'
  if (bytes < 1024 * 1024 * 1024) return (bytes / 1024 / 1024).toFixed(1) + ' MB'
  return (bytes / 1024 / 1024 / 1024).toFixed(2) + ' GB'
}
/* 两个字节数按同一单位成对格式化（已传输 / 总大小），单位取两者中较大者对应量级，
   避免浮点长小数与 "470 B / 1.0 KB" 这类单位错位：→ "0.5 KB / 1.0 KB" */
function fmtSizePair(a: number, b: number) {
  const max = Math.max(0, a, b)
  if (max < 1024) return `${Math.round(a)} B / ${Math.round(b)} B`
  if (max < 1024 * 1024) return `${(a / 1024).toFixed(1)} KB / ${(b / 1024).toFixed(1)} KB`
  if (max < 1024 * 1024 * 1024) return `${(a / 1048576).toFixed(1)} MB / ${(b / 1048576).toFixed(1)} MB`
  return `${(a / 1073741824).toFixed(2)} GB / ${(b / 1073741824).toFixed(2)} GB`
}
function crumbs(path: string) {
  const parts = path.split('/').filter(Boolean)
  const out: { label: string; path: string }[] = [{ label: '/', path: '/' }]
  let acc = ''
  parts.forEach(p => { acc += '/' + p; out.push({ label: p, path: acc }) })
  return out
}

/** 按全局设置过滤隐藏文件（. 开头）；开启时原样返回 */
function visibleEntries(list: FsEntry[]): FsEntry[] {
  return savedSettings.sftpShowHidden ? list : list.filter(e => !e.name.startsWith('.'))
}
const localEntries = computed(() => sortEntries(visibleEntries(localRaw.value), localSort.value))
const remoteEntries = computed(() => sortEntries(visibleEntries(remoteRaw.value), remoteSort.value))

/* ---- 传输队列：pending → running → success，同一时刻仅一个 running（串行）
   状态色规范：pending #606266 · running #409EFF · paused #E6A23C · error #F56C6C · success 项目绿 ---- */
type TransferStatus = 'pending' | 'running' | 'paused' | 'success' | 'error'
interface Transfer {
  id: number; name: string; dir: 'up' | 'down'
  size: number; pct: number; status: TransferStatus
  speed: number // 展示速率 B/s（真实传输由后端进度事件提供）
  /** 首次开始传输的墙钟时间戳（ms），用于完成事件 elapsed_ms；暂停时间计入 */
  startAt: number
  error?: string // 失败详情（查看错误弹窗展示）
  localPath?: string   // 本地绝对路径
  remotePath?: string  // 远端绝对路径
  /** 跨侧剪切粘贴：传输成功后要删除的源文件绝对路径 */
  cutSrc?: string
}
const transfers = ref<Transfer[]>([])
let seq = 0

/** 后端 SFTP 进度事件载荷 */
interface TransferProgress {
  taskId: number
  transferred: number
  total: number
  speed: number
  done: boolean
}

const runningCount = computed(() => transfers.value.filter(t => t.status === 'running').length)
const pendingCount = computed(() => transfers.value.filter(t => t.status === 'pending').length)
const pausedCount = computed(() => transfers.value.filter(t => t.status === 'paused').length)
const successCount = computed(() => transfers.value.filter(t => t.status === 'success').length)

/* 筛选：全部 / 上传 / 下载 / 已完成 */
const queueFilter = ref<'all' | 'up' | 'down' | 'done'>('all')
const filteredTransfers = computed(() => {
  const f = queueFilter.value
  return transfers.value.filter(t =>
    f === 'all' ? true
    : f === 'done' ? t.status === 'success'
    : t.dir === f,
  )
})

/* 队列面板：head 28 + tabs 28；折叠时只留 head */
const queueCollapsed = ref(false)
const queueHeight = ref(180)
const queueListMaxHeight = computed(() =>
  queueCollapsed.value ? '0px' : Math.max(0, queueHeight.value - 56) + 'px')
function toggleQueueCollapse() {
  queueCollapsed.value = !queueCollapsed.value
  // 拖拽收起到 28 后再展开：恢复默认高度，避免列表区高度为负
  if (!queueCollapsed.value && queueHeight.value < 120) queueHeight.value = 180
}

function fmtSpeed(bps: number) {
  if (!bps || bps <= 0) return '0 B/s'
  return fmtSize(bps) + '/s'
}
/* 状态文案 */
function statusText(t: Transfer) {
  return ({ pending: '等待中', running: '传输中', paused: '已暂停', success: '已完成', error: '失败' } as const)[t.status]
}
/* 队列内展示名 */
function taskName(t: Transfer) {
  return t.name
}
/* 已传输字节数 */
function doneBytes(t: Transfer) {
  return Math.min(t.size, Math.max(0, t.size * Math.min(100, Math.max(0, t.pct)) / 100))
}
/* 字节信息：已传 / 总大小 */
function bytesText(t: Transfer) {
  return fmtSizePair(doneBytes(t), t.size)
}
/* 速率：仅传输中 */
function rateText(t: Transfer) {
  return t.status === 'running' && t.speed > 0 ? fmtSpeed(t.speed) : ''
}
/* 备注：传输中显剩余时间，完成显校验通过，失败显错误，其余留空 */
function noteText(t: Transfer) {
  if (t.status === 'running' && t.speed > 0 && t.pct < 100) {
    const sec = (t.size - doneBytes(t)) / t.speed
    if (isFinite(sec) && sec >= 1) {
      if (sec < 60) return '剩 ' + Math.ceil(sec) + ' 秒'
      if (sec < 3600) return '剩 ' + Math.ceil(sec / 60) + ' 分'
      return '剩 ' + (sec / 3600).toFixed(1) + ' 时'
    }
  }
  if (t.status === 'success') return '校验通过'
  if (t.status === 'error') return t.error || '传输失败'
  return ''
}
/* 右侧状态文案 */
function stateText(t: Transfer) {
  return statusText(t)
}

function startQueueDrag(e: MouseEvent) {
  e.preventDefault()
  const startY = e.clientY
  const startH = queueHeight.value
  const sftpEl = (e.target as HTMLElement).closest('.sftp') as HTMLElement | null
  const maxH = sftpEl ? sftpEl.offsetHeight - 1 : 500
  document.body.style.userSelect = 'none'
  document.body.style.cursor = 'ns-resize'

  const onMove = (ev: MouseEvent) => {
    const delta = ev.clientY - startY
    const h = startH - delta
    if (h <= 40) {
      queueCollapsed.value = true
      queueHeight.value = 28
    } else {
      queueCollapsed.value = false
      queueHeight.value = Math.min(maxH, h)
    }
  }
  const onUp = () => {
    document.body.style.userSelect = ''
    document.body.style.cursor = ''
    window.removeEventListener('mousemove', onMove)
    window.removeEventListener('mouseup', onUp)
  }
  window.addEventListener('mousemove', onMove)
  window.addEventListener('mouseup', onUp)
}

function startTask(
  dir: 'up' | 'down', name: string, size: number,
  extra: { localPath: string; remotePath: string; cutSrc?: string },
) {
  transfers.value.push({
    id: ++seq, name, dir, size, pct: 0, status: 'pending', speed: 0, startAt: 0,
    localPath: extra.localPath,
    remotePath: extra.remotePath,
    cutSrc: extra.cutSrc,
  })
  queueCollapsed.value = false
  reportLog('info', SFTP_TRANSFER_ENQUEUE, `${dir === 'up' ? '上传' : '下载'}加入队列`, {
    name, size, dir, from: extra.localPath, to: extra.remotePath,
  })
  pumpQueue()
}

/** 空闲时取下一个排队任务启动（完成/失败回调中再次调用形成串行链）。
 *  单文件走真实 SFTP invoke，进度由后端 Channel 流式推送。 */
async function pumpQueue() {
  if (transfers.value.some(t => t.status === 'running')) return
  const t = transfers.value.find(x => x.status === 'pending')
  if (!t) return
  t.status = 'running'
  if (t.startAt === 0) t.startAt = Date.now() // 仅首次开始计时；暂停后续传不重置
  // pct > 0 为暂停/重试后的恢复，不再重复"开始"提示
  if (t.pct > 0) {
    reportLog('debug', SFTP_TRANSFER_RESUME, `${t.dir === 'up' ? '上传' : '下载'}继续`, {
      name: t.name, pct: Math.floor(t.pct),
    })
  } else {
    reportLog('info', SFTP_TRANSFER_START, `${t.dir === 'up' ? '上传' : '下载'}开始`, {
      name: t.name, size: t.size, resume_from: 0,
    })
    toast(`开始${t.dir === 'up' ? '上传' : '下载'}：${taskName(t)}`, 'info', 1700)
  }

  const failTask = (msg: string) => {
    t.status = 'error'
    t.speed = 0
    t.error = msg
    reportLog('error', SFTP_TRANSFER_FAILED, `${t.dir === 'up' ? '上传' : '下载'}失败`, {
      name: t.name, err: msg, transferred: Math.round(t.size * t.pct / 100), total: t.size,
    })
    toast(`${t.dir === 'up' ? '上传' : '下载'}失败：${taskName(t)}`, 'warn', 2200)
    void pumpQueue()
  }
  const finishTask = () => {
    t.pct = 100
    t.status = 'success'
    // 清零前捕获最后一帧速率（后端按滑动窗口计算的瞬时速率）
    const finalSpeedBps = t.speed
    t.speed = 0
    reportLog('info', SFTP_TRANSFER_COMPLETE, `${t.dir === 'up' ? '上传' : '下载'}完成`, {
      name: t.name, size: t.size,
      elapsed_ms: t.startAt ? Date.now() - t.startAt : 0,
      speed_bps: finalSpeedBps, verify: 'pass',
    })
    toast(`${t.dir === 'up' ? '上传' : '下载'}完成：${taskName(t)}`, 'ok', 2000)
    // 跨侧剪切粘贴：传输成功后删除源文件（失败仅记日志，目标已完整）
    if (t.cutSrc) {
      if (t.dir === 'up') {
        void invoke('local_remove', { path: t.cutSrc })
          .catch(e => reportLog('error', SFTP_MANAGE_REMOVE, '剪切源删除失败', { path: t.cutSrc, err: String(e) }))
      } else {
        void invoke('sftp_remove', { sessionId: backendId, path: t.cutSrc })
          .catch(e => reportLog('error', SFTP_MANAGE_REMOVE, '剪切源删除失败', { path: t.cutSrc, err: String(e) }))
      }
    }
    // 传输完成自动刷新对侧目录，免去手动点刷新：上传→远端，下载→本地
    if (t.dir === 'up') void loadRemote(remotePath.value)
    else void loadLocal(localPath.value)
    void pumpQueue()
  }

  const backendId = activeSession.value?.backendId
  if (!backendId) { failTask('未建立 SSH 连接'); return }
  if (!t.localPath || !t.remotePath) { failTask('传输路径缺失'); return }
  const cmd = t.dir === 'up' ? 'sftp_upload' : 'sftp_download'
  // 每个任务独立 Channel：后端流式推送进度，前端 onmessage 按 taskId 更新对应任务
  const channel = new Channel<TransferProgress>()
  channel.onmessage = (p) => {
    if (p.total > 0) t.size = p.total
    t.pct = p.total > 0 ? (p.transferred / p.total) * 100 : (p.done ? 100 : t.pct)
    t.speed = p.speed
  }
  const args = t.dir === 'up'
    ? { sessionId: backendId, taskId: t.id, localPath: t.localPath, remotePath: t.remotePath, chunkKb: savedSettings.sftpChunkKb, resume: savedSettings.sftpResume, channel }
    : { sessionId: backendId, taskId: t.id, remotePath: t.remotePath, localPath: t.localPath, chunkKb: savedSettings.sftpChunkKb, resume: savedSettings.sftpResume, channel }
  try {
    await invoke(cmd, args)
    finishTask()
  } catch (e) {
    const msg = String(e)
    // 用户主动取消：静默移除，不弹失败 toast
    if (msg.includes('已取消') || msg.includes('Cancelled')) {
      t.status = 'error'
      t.speed = 0
      t.error = '已取消'
      transfers.value = transfers.value.filter(x => x.id !== t.id)
      reportLog('warn', SFTP_TRANSFER_CANCEL, `${t.dir === 'up' ? '上传' : '下载'}已取消`, {
        name: t.name, transferred: Math.round(t.size * t.pct / 100), total: t.size,
      })
      void pumpQueue()
    } else {
      failTask(msg)
    }
  }
}

/** 取消/移除任务：进行中的任务先通知后端取消，再出队启动下一个 */
function removeTask(t: Transfer) {
  if (t.status === 'running') {
    const backendId = activeSession.value?.backendId
    if (backendId) {
      void invoke('sftp_transfer_cancel', { sessionId: backendId, taskId: t.id }).catch(() => {})
    }
  }
  transfers.value = transfers.value.filter(x => x.id !== t.id)
  reportLog('warn', SFTP_TRANSFER_CANCEL, `${t.dir === 'up' ? '上传' : '下载'}${t.status === 'success' ? '记录移除' : '已取消'}`, {
    name: t.name, transferred: Math.round(t.size * t.pct / 100), total: t.size,
  })
  if (t.status === 'running') void pumpQueue()
}

/** 暂停任务：通知后端暂停开关，invoke 仍挂起（保留串行槽位避免并发） */
function pauseTask(t: Transfer) {
  const backendId = activeSession.value?.backendId
  if (t.status === 'running') {
    if (backendId) {
      void invoke('sftp_transfer_pause', { sessionId: backendId, taskId: t.id, paused: true }).catch(() => {})
    }
    t.status = 'paused'
    t.speed = 0
    reportLog('warn', SFTP_TRANSFER_PAUSE, '已暂停', { name: t.name, pct: Math.floor(t.pct) })
  } else if (t.status === 'pending') {
    t.status = 'paused'
    reportLog('warn', SFTP_TRANSFER_PAUSE, '排队任务已挂起', { name: t.name, pct: 0 })
  }
}
/** 继续任务：通知后端恢复并直接回到 running（invoke 仍在） */
function resumeTask(t: Transfer) {
  if (t.status !== 'paused') return
  const backendId = activeSession.value?.backendId
  if (backendId) {
    void invoke('sftp_transfer_pause', { sessionId: backendId, taskId: t.id, paused: false }).catch(() => {})
  }
  t.status = 'running'
  // 与暂停（WARN）配对的显式用户动作，INFO 保证默认级别下可见
  reportLog('info', SFTP_TRANSFER_RESUME, `${t.dir === 'up' ? '上传' : '下载'}继续`, {
    name: t.name, pct: Math.floor(t.pct),
  })
}
/** 失败任务重试：进度归零、清错误后重新入队 */
function retryTask(t: Transfer) {
  if (t.status !== 'error') return
  t.pct = 0
  t.speed = 0
  t.error = undefined
  t.status = 'pending'
  reportLog('info', SFTP_TRANSFER_ENQUEUE, `${t.dir === 'up' ? '上传' : '下载'}重新加入队列`, {
    name: t.name, size: t.size, dir: t.dir, from: t.localPath, to: t.remotePath,
  })
  void pumpQueue()
}
/** 打开目录：上传完成 → 远程目录；下载完成 → 本地目录 */
function openTaskDir(t: Transfer) {
  if (t.status !== 'success') return
  const side = t.dir === 'up' ? '远程' : '本地'
  toast(`打开${side}目录：${t.name}`, 'info', 2000)
}
/** 头部批量操作：全部暂停 / 全部继续（独立按钮） */
function pauseAllTasks() {
  transfers.value.forEach(t => { if (t.status === 'running' || t.status === 'pending') pauseTask(t) })
}
function resumeAllTasks() {
  transfers.value.forEach(t => { if (t.status === 'paused') resumeTask(t) })
}

/** 清空已完成任务（进行中/排队中保留） */
function clearDoneTasks() {
  transfers.value = transfers.value.filter(t => t.status !== 'success')
}

/** 当前侧选中的条目 */
function selectedEntries(side: 'local' | 'remote'): FsEntry[] {
  const list = side === 'local' ? localRaw.value : remoteRaw.value
  const sel = side === 'local' ? localSelected.value : remoteSelected.value
  return list.filter(e => sel.has(e.name))
}

/** 各侧选中条目数（文件与目录），供 header 传输按钮状态/角标显示 */
const localSel = computed(() => selectedEntries('local'))
const remoteSel = computed(() => selectedEntries('remote'))

/** 将一个选中条目入队：仅支持单文件传输（目录递归传输暂未实现），
 *  拼出本地与远端绝对路径后入队。 */
function enqueueEntry(dir: 'up' | 'down', e: FsEntry) {
  if (e.type === 'dir') {
    toast('暂不支持目录传输，请选择文件', 'warn')
    return
  }
  startTask(dir, e.name, e.size, {
    localPath: joinPath(localPath.value, e.name),
    remotePath: joinPath(remotePath.value, e.name),
  })
}

function onUpload() {
  if (!connected.value) { toast('请先建立 SSH 连接', 'warn'); return }
  const entries = selectedEntries('local')
  if (!entries.length) { toast('请先在左侧选择文件', 'warn'); return }
  entries.forEach(e => enqueueEntry('up', e))
  localSelected.value = new Set()
}
function onDownload() {
  if (!connected.value) { toast('请先建立 SSH 连接', 'warn'); return }
  const entries = selectedEntries('remote')
  if (!entries.length) { toast('请先在右侧选择文件', 'warn'); return }
  entries.forEach(e => enqueueEntry('down', e))
  remoteSelected.value = new Set()
}

/* ---- 文件管理操作（右键菜单与快捷键共用同一套业务函数） ---- */

/** 删除目标：不进回收站、不做二次确认，逐项直接删除 → 刷新；软链只删链接本身（后端保证） */
async function deleteTargets(side: Side, targets: FsEntry[]) {
  if (!targets.length) return
  try {
    for (const t of targets) {
      await invokeSide(side, 'remove', { path: joinPath(sidePath(side), t.name) })
    }
    sideSel(side).value = new Set()
    reportLog('info', SFTP_MANAGE_REMOVE, `删除 ${targets.length} 项`, {
      side: side === 'local' ? '本地' : '远程', count: targets.length,
    })
    toast(`已删除 ${targets.length} 项`, 'ok', 1800)
    void refreshSide(side)
  } catch (e) {
    toast(`删除失败：${(e as Error).message}`, 'warn')
    void refreshSide(side)
  }
}

/** 复制/剪切当前选择：记录复制瞬间源目录绝对路径与元数据，之后导航仍可粘贴 */
function copySelection(side: Side, mode: 'copy' | 'cut') {
  const entries = selectedEntries(side)
  if (!entries.length) return
  clipboard.value = {
    srcSide: side,
    base: sidePath(side),
    mode,
    items: entries.map(e => ({ name: e.name, isDir: e.type === 'dir', size: e.size })),
  }
  toast(mode === 'copy' ? `已复制 ${entries.length} 项` : `已剪切 ${entries.length} 项`, 'info')
}

/**
 * 粘贴：同侧 copy → 递归复制、cut → 重命名；跨侧 → 入传输队列
 * （跨侧 cut 传输成功后自动删除源文件）。目标重名不覆盖、不询问，
 * 自动分配 "原名 (n)" 的新名字。
 */
async function pasteTargets(side: Side) {
  const cb = clipboard.value
  if (!cb) return
  const dstDir = sidePath(side)
  interface Job { src: string; dst: string; name: string; isDir: boolean; size: number }
  const jobs: Job[] = []
  for (const it of cb.items) {
    const src = joinPath(cb.base, it.name)
    const dst = joinPath(dstDir, it.name)
    if (src === dst) continue
    jobs.push({ src, dst, name: it.name, isDir: it.isDir, size: it.size })
  }
  if (!jobs.length) { toast('源与目标位置相同', 'info'); return }

  /* 重名冲突：自动重命名（a.txt → a (1).txt），不覆盖任何已有项。
     used 记录本批已占用的新名字，防止多个 job 拿到同一候选 */
  const used = new Set<string>()
  for (const j of jobs) {
    if (used.has(j.name) || entryExists(side, j.name)) {
      const free = nextFreeName(side, j.name, used)
      j.name = free
      j.dst = joinPath(dstDir, free)
    }
    used.add(j.name)
  }

  if (cb.srcSide === side) {
    /* 同侧：纯远程/纯本地操作 */
    try {
      for (const j of jobs) {
        if (cb.mode === 'cut') await invokeSide(side, 'rename', { oldPath: j.src, newPath: j.dst })
        else await invokeSide(side, 'copy', { src: j.src, dst: j.dst })
      }
      toast(cb.mode === 'cut' ? '已移动' : '已粘贴', 'ok')
      if (cb.mode === 'cut') clipboard.value = null
      void refreshSide(side)
    } catch (e) {
      toast(`粘贴失败：${(e as Error).message}`, 'warn')
      void refreshSide(side)
    }
  } else {
    /* 跨侧：本质下载/上传，入现有串行传输队列 */
    const dir: 'up' | 'down' = side === 'remote' ? 'up' : 'down'
    for (const j of jobs) {
      if (j.isDir) { toast('暂不支持目录传输，请选择文件', 'warn'); continue }
      const localAbs = dir === 'up' ? j.src : j.dst
      const remoteAbs = dir === 'up' ? j.dst : j.src
      startTask(dir, j.name, j.size, {
        localPath: localAbs,
        remotePath: remoteAbs,
        cutSrc: cb.mode === 'cut' ? j.src : undefined,
      })
    }
    if (cb.mode === 'cut') clipboard.value = null
  }
}

/** 复制选中项完整路径到系统剪贴板（多项换行分隔） */
async function copyPaths(side: Side, targets: FsEntry[]) {
  if (!targets.length) return
  const text = targets.map(t => joinPath(sidePath(side), t.name)).join('\n')
  await writeClipboardText(text)
  toast('路径已复制', 'ok')
}

/** 写系统剪贴板：优先 Clipboard API，失败降级 textarea + execCommand */
async function writeClipboardText(text: string) {
  try {
    await navigator.clipboard.writeText(text)
  } catch {
    const ta = document.createElement('textarea')
    ta.value = text
    ta.style.position = 'fixed'
    ta.style.opacity = '0'
    document.body.appendChild(ta)
    ta.select()
    document.execCommand('copy')
    document.body.removeChild(ta)
  }
}

/** 在终端打开：远程 → 应用内终端 cd 该目录；本地 → 系统终端打开该目录。
 *  传文件时定位到其所在目录。 */
function openInTerminal(side: Side, target?: FsEntry) {
  const p = target && target.type === 'dir'
    ? joinPath(sidePath(side), target.name)
    : sidePath(side)
  if (side === 'remote') {
    syncShellCwd(p)
    toast('已在终端打开', 'info', 1500)
  } else {
    invoke('local_open_terminal', { path: p })
      .then(() => toast('已在终端打开', 'info', 1500))
      .catch(e => toast(`打开终端失败：${e}`, 'warn'))
  }
}

/** 全选当前侧可见条目 */
function selectAll(side: Side) {
  sideSel(side).value = new Set(visibleEntries(sideRaw(side).value).map(e => e.name))
}

/* ---- 右键事件：源头一次性写全坐标/侧/目标 ---- */
function onRowContext(e: MouseEvent, side: Side, entry: FsEntry) {
  lastSide.value = side
  const sel = sideSel(side)
  // 右键未选中项 → 仅选该项；右键已选中项 → 保持多选
  if (!sel.value.has(entry.name)) sel.value = new Set([entry.name])
  menu.value = { x: e.clientX, y: e.clientY, side, targets: selectedEntries(side) }
}
function onBlankContext(e: MouseEvent, side: Side) {
  lastSide.value = side
  sideSel(side).value = new Set()
  menu.value = { x: e.clientX, y: e.clientY, side, targets: [] }
}

/* v-model 代理：ContextMenu 关闭即清空菜单状态 */
const menuVisible = computed({
  get: () => menu.value !== null,
  set: v => { if (!v) menu.value = null },
})
const menuX = computed(() => menu.value?.x ?? 0)
const menuY = computed(() => menu.value?.y ?? 0)
const menuItems = computed<MenuItem[]>(() => {
  const m = menu.value
  return m ? buildMenu(m.side, m.targets) : []
})

/* 图标 path d（24x24） */
const ICON = {
  mkdir: 'M4 7a2 2 0 012-2h3l2 2h7a2 2 0 012 2v8a2 2 0 01-2 2H6a2 2 0 01-2-2V7z M12 11v5M9.5 13.5h5',
  mkfile: 'M14 3H6a2 2 0 00-2 2v14a2 2 0 002 2h12a2 2 0 002-2V9z M14 3v6h6 M12 13v4M10 15h4',
  paste: 'M9 4h6a2 2 0 012 2h2a1 1 0 011 1v13a1 1 0 01-1 1H6a1 1 0 01-1-1V7a1 1 0 011-1h2a2 2 0 012-2z M9 4a2 2 0 012-2h2a2 2 0 012 2 M12 11v5M9.5 13.5h5',
  copy: 'M9 9h10v10H9z M5 15H4V4h11v1',
  cut: 'M8.5 7.5 19 18 M8.5 16.5 19 6 M6 6a2 2 0 100 4 2 2 0 000-4z M6 14a2 2 0 100 4 2 2 0 000-4z',
  rename: 'M4 20h4L19 9l-4-4L4 16v4z M14 6l4 4',
  del: 'M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13M10 11v5M14 11v5',
  path: 'M7 8h11M7 12h11M7 16h7 M4 7h.01M4 11h.01M4 15h.01',
  term: 'M4 5h16v14H4z M8 9l3 3-3 3 M13 15h3',
  all: 'M4 4h16v16H4z M9 9h6v6H9z',
  enter: 'M9 6l6 6-6 6 M5 12h10',
  file: 'M14 3H6a2 2 0 00-2 2v14a2 2 0 002 2h12a2 2 0 002-2V9z M14 3v6h6',
  up: 'M12 19V5M5 12l7-7 7 7',
  down: 'M12 5v14M5 12l7 7 7-7',
  refresh: 'M21 12a9 9 0 11-3-6.7L21 8 M21 3v5h-5',
}

/** 按场景构造菜单模型（空白/单项/多选） */
function buildMenu(side: Side, targets: FsEntry[]): MenuItem[] {
  if (!targets.length) {
    return [
      { label: '新建文件夹', icon: ICON.mkdir, action: () => startEdit(side, 'mkdir') },
      { label: '新建文件', icon: ICON.mkfile, action: () => startEdit(side, 'mkfile') },
      { divider: true },
      { label: '粘贴', icon: ICON.paste, shortcut: '⌘V', disabled: !clipboard.value, action: () => void pasteTargets(side) },
      { divider: true },
      { label: '全选', icon: ICON.all, shortcut: '⌘A', action: () => selectAll(side) },
      { label: '刷新', icon: ICON.refresh, action: () => void refreshSide(side) },
    ]
  }

  const single = targets.length === 1 ? targets[0]! : null
  const onlyFiles = targets.every(t => t.type !== 'dir')
  const items: MenuItem[] = []

  if (single) {
    items.push({
      label: single.type === 'dir' ? '进入目录' : '打开',
      icon: single.type === 'dir' ? ICON.enter : ICON.file,
      action: () => onRowDblClick(side, single),
    })
    items.push({ divider: true })
  }

  if (onlyFiles) {
    items.push({
      label: side === 'local' ? '上传到远程' : '下载到本地',
      icon: side === 'local' ? ICON.up : ICON.down,
      disabled: !connected.value,
      action: () => {
        sideSel(side).value = new Set(targets.map(t => t.name))
        if (side === 'local') onUpload()
        else onDownload()
      },
    })
    items.push({ divider: true })
  }

  items.push({ label: '复制', icon: ICON.copy, shortcut: '⌘C', action: () => copySelection(side, 'copy') })
  items.push({ label: '剪切', icon: ICON.cut, shortcut: '⌘X', action: () => copySelection(side, 'cut') })
  if (single) {
    items.push({ label: '重命名', icon: ICON.rename, shortcut: 'F2', action: () => startEdit(side, 'rename', single) })
  }
  items.push({
    label: targets.length > 1 ? `删除 ${targets.length} 项` : '删除',
    icon: ICON.del, shortcut: 'Del', danger: true,
    action: () => void deleteTargets(side, targets),
  })
  items.push({ divider: true })
  items.push({ label: '复制路径', icon: ICON.path, action: () => void copyPaths(side, targets) })
  /* 本地/远程菜单一致：单项时可在终端打开（文件 → 所在目录） */
  if (single) {
    items.push({
      label: single.type === 'dir' ? '在终端打开' : '在终端打开所在目录',
      icon: ICON.term,
      action: () => openInTerminal(side, single),
    })
  }
  return items
}

/* 行交互：单击选择（Ctrl/⌘ 多选）、双击目录进入、双击文件传输 */
function onRowClick(side: Side, entry: FsEntry, e: MouseEvent) {
  lastSide.value = side
  const sel = side === 'local' ? localSelected : remoteSelected
  if (e.ctrlKey || e.metaKey) {
    const next = new Set(sel.value)
    next.has(entry.name) ? next.delete(entry.name) : next.add(entry.name)
    sel.value = next
  } else {
    sel.value = new Set([entry.name])
  }
}
/** SFTP → Shell：让远端交互式终端 `cd` 到指定目录，保持两侧 CWD 一致 */
function syncShellCwd(path: string) {
  const backendId = activeSession.value?.backendId
  if (!backendId) return
  void invoke('sftp_sync_cwd', { sessionId: backendId, path }).catch(() => {})
}

function onRowDblClick(side: Side, entry: FsEntry) {
  lastSide.value = side
  const sel = side === 'local' ? localSelected : remoteSelected
  if (entry.type === 'dir') {
    sel.value = new Set()
    if (side === 'local') {
      void loadLocal(joinPath(localPath.value, entry.name))
    } else {
      const next = joinPath(remotePath.value, entry.name)
      void loadRemote(next)
      syncShellCwd(next)
    }
  } else {
    sel.value = new Set([entry.name])
    if (side === 'local') onUpload()
    else onDownload()
  }
}

function goUp(side: 'local' | 'remote') {
  if (side === 'local') void loadLocal(parentPath(localPath.value))
  else {
    const next = parentPath(remotePath.value)
    void loadRemote(next)
    syncShellCwd(next)
  }
}
function gotoCrumb(side: 'local' | 'remote', path: string) {
  if (side === 'local') void loadLocal(path)
  else {
    void loadRemote(path)
    syncShellCwd(path)
  }
}
function changeSort(side: 'local' | 'remote', key: SortKey) {
  const s = side === 'local' ? localSort : remoteSort
  if (s.value.key === key) s.value.dir = (s.value.dir * -1) as 1 | -1
  else s.value = { key, dir: 1 }
}

/* 顶栏刷新：本地与远端（若已连接）同时重载当前目录 */
function refreshAll() {
  void loadLocal(localPath.value)
  void loadRemote(remotePath.value)
}

/* 远端目录跟随活动后端会话：建立连接 → 加载登录默认目录；
   断开（backendId 置空）或切换会话 → 清空残留，避免把上一台主机的文件展示给新会话 */
watch(() => activeSession.value?.backendId, (id) => {
  remoteRaw.value = []
  remotePath.value = '/'
  remoteSelected.value = new Set()
  remoteLoading.value = false
  remoteReqSeq++ // 使在飞的旧会话请求全部过期
  if (id) void loadRemote()
})

/* Shell → SFTP：终端执行 cd 后后端经 0x08 帧上报新 CWD，文件树同步跳转。
   仅当上报路径与当前远端目录不同时才刷新，避免 SFTP 导航触发的回环。 */
watch(activeSftpCwd, (cwd) => {
  if (!cwd || cwd === remotePath.value) return
  remotePath.value = cwd
  remoteSelected.value = new Set()
  void loadRemote(cwd)
})

/* ---- Dock tab 切换（状态在 store，供顶部工具栏等外部入口同步） ---- */
const activeTab = dockTab
function switchTab(tab: 'sftp' | 'log') {
  activeTab.value = tab
  expandDock() // 折叠状态下点击 tab 时一并展开
}

/* ---- 应用日志（数据源：stores/applog，后端 Hub 订阅推送；面板显示 = 磁盘文件内容） ----
   日志行模型 PanelLogEntry 由 store 提供（id=seq 全局唯一、time 已格式化、msg 含 kv 文本）。
   SFTP 队列事件经 report() 上报后端统一打 seq/ts，再由订阅回推显示，不经本地直插。 */

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
// 日志自动换行（纯视图偏好，默认开启，本地持久化）；关闭时长行单行横向滚动
const logWrap = ref(localStorage.getItem('rhost.logWrap') !== '0')
watch(logWrap, v => localStorage.setItem('rhost.logWrap', v ? '1' : '0'))
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
  // 清空后端内存缓冲 + 前端显示（磁盘文件不受影响）
  void clearAppLogs()
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

/* resizer 拖拽 */
const dockEl = ref<HTMLElement | null>(null)
const isResizing = ref(false)
let dragging = false
let pendingY = 0
let rafId = 0

/* ---- SFTP 双栏分割拖拽（本地/远程宽度占比，持久化在 store.sftpLocalRatio） ---- */
const isSplitting = ref(false)
let splitting = false
let splitStartX = 0
let splitStartRatio = 0.5
let splitUsableW = 0   // sftp-main 宽度 - 中间传输列宽度
let pendingX = 0
let splitRaf = 0

function startSplitDrag(e: MouseEvent) {
  e.preventDefault()
  const mainEl = (e.target as HTMLElement).closest('.sftp-main') as HTMLElement | null
  if (!mainEl) return
  splitting = true
  isSplitting.value = true
  splitStartX = e.clientX
  pendingX = e.clientX // 初始化，防止单击时 onUp 收尾用残留值计算 dx
  splitStartRatio = sftpLocalRatio.value
  splitUsableW = mainEl.getBoundingClientRect().width
  document.body.style.userSelect = 'none'
  document.body.style.cursor = 'col-resize'
}

/** 双击恢复均分 */
function onSplitDblClick(e: MouseEvent) {
  e.preventDefault()
  sftpLocalRatio.value = 0.5
}

function applySplit() {
  splitRaf = 0
  if (!splitUsableW) return
  const dx = pendingX - splitStartX
  sftpLocalRatio.value = Math.min(0.8, Math.max(0.2, splitStartRatio + dx / splitUsableW))
}

function endSplitDrag() {
  splitting = false
  isSplitting.value = false
  if (splitRaf) { cancelAnimationFrame(splitRaf); splitRaf = 0 }
  if (!dragging) {
    document.body.style.userSelect = ''
    document.body.style.cursor = ''
  }
}

const HEAD_H = 40        // 标题栏高度（与 CSS .dock-head / .dock.collapsed 保持一致），拖拽下限
const MIN_H = 120        // 视为"可用"的最小高度，低于此展开时自动恢复
const COLLAPSE_GAP = 48  // 触底后继续下拉该距离 → 自动折叠，并记住可用高度
const EXPAND_GAP = 24    // 折叠态上拉超过该距离 → 展开；折叠/展开在同一次拖拽内可反复切换
// 可用高度统一存于全局 dockHeight（折叠态下保留，展开/重启无缝恢复）

/** 实时几何：以 workspace / tabs 的真实矩形为基准，避免硬编码
 *  标题栏/Tab/状态栏高度常量（边框、macOS 安全区造成 1~2px 偏差即出现顶缝） */
function geometry() {
  const el = dockEl.value
  const ws = el?.parentElement
  if (!el || !(ws instanceof HTMLElement)) return null
  const wsRect = ws.getBoundingClientRect()
  const tabsEl = ws.querySelector(':scope > .tabs')
  const tabsBottom = tabsEl?.getBoundingClientRect().bottom ?? wsRect.top
  return { wsBottom: wsRect.bottom, tabsBottom }
}

function startDrag(e: MouseEvent) {
  dragging = true
  isResizing.value = true
  pendingY = e.clientY
  if (!dockCollapsed.value) {
    // 从真实渲染高度同步（内联绑定值通常即此值，保险一次）
    const cur = dockEl.value?.offsetHeight ?? 0
    if (cur >= MIN_H) dockHeight.value = cur
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
  if (!splitting) {
    document.body.style.userSelect = ''
    document.body.style.cursor = ''
  }
}

function applyHeight() {
  rafId = 0
  const g = geometry()
  if (!g) return
  // 期望高度 = workspace 底边 - 鼠标Y；上限 = workspace 底边 - Tab 栏真实底边
  const h = g.wsBottom - pendingY
  const max = g.wsBottom - g.tabsBottom

  if (!dockCollapsed.value) {
    // 已到下限（保留标题栏）仍继续下拉超过阈值 → 自动折叠。
    // 注意：不结束拖拽手势，反向拉回超过展开阈值可在同一次拖拽内立即重新展开
    if (h <= HEAD_H - COLLAPSE_GAP) {
      dockCollapsed.value = true // dockHeight 保留，展开无缝恢复
      return
    }
    const clamped = Math.min(Math.max(h, HEAD_H), max)
    dockHeight.value = clamped
    return
  }

  // 折叠态：上拉超过阈值 → 展开，高度继续实时跟随鼠标
  if (h >= HEAD_H + EXPAND_GAP) {
    dockCollapsed.value = false
    dockHeight.value = Math.min(Math.max(h, HEAD_H), max)
  }
}

function onMove(e: MouseEvent) {
  if (splitting) {
    pendingX = e.clientX
    if (!splitRaf) splitRaf = requestAnimationFrame(applySplit)
  }
  if (!dragging) return
  pendingY = e.clientY
  if (!rafId) rafId = requestAnimationFrame(applyHeight)
}
function onUp() {
  if (splitting) {
    endSplitDrag()
    applySplit() // 收尾：应用最后一次 pendingX
  }
  if (!dragging) return
  endDrag()
  applyHeight() // 收尾：应用最后一次 pendingY
}

/* 展开 / 折叠 */
function expandDock() {
  if (!dockCollapsed.value) return
  if (dockHeight.value < MIN_H) dockHeight.value = DOCK_DEFAULT_HEIGHT
  dockCollapsed.value = false
}

function toggleDock() {
  if (dockCollapsed.value) expandDock()
  else dockCollapsed.value = true
}
/* ---- SFTP 快捷键：作用于最后交互的一侧；右键菜单打开/输入框聚焦时不介入 ---- */
function onSftpShortcut(e: KeyboardEvent) {
  if (!dockVisible.value || activeTab.value !== 'sftp') return
  if (menu.value) return // 菜单有自己的键盘行为（Esc/方向/Enter）
  const el = e.target as HTMLElement | null
  if (el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA')) return
  const side = lastSide.value
  const meta = e.metaKey || e.ctrlKey
  const key = e.key.toLowerCase()

  if (meta && key === 'c') {
    e.preventDefault()
    copySelection(side, 'copy')
  } else if (meta && key === 'x') {
    e.preventDefault()
    copySelection(side, 'cut')
  } else if (meta && key === 'v') {
    e.preventDefault()
    void pasteTargets(side)
  } else if (meta && key === 'a') {
    e.preventDefault()
    selectAll(side)
  } else if (e.key === 'F2') {
    const t = selectedEntries(side)
    if (t.length === 1) startEdit(side, 'rename', t[0])
  } else if (e.key === 'Delete' || e.key === 'Backspace') {
    const t = selectedEntries(side)
    if (t.length) void deleteTargets(side, t)
  } else if (e.key === 'Enter') {
    const t = selectedEntries(side)
    if (t.length === 1) onRowDblClick(side, t[0]!)
  }
}

onMounted(() => {
  // 订阅后端应用日志流（replay + 增量；幂等，多订阅者广播）
  subscribeAppLogs()
  // 首次打开加载本地目录（不传路径 → 后端解析为用户主目录）
  void loadLocal()
  // 组件重挂时活动会话可能早已在线（watch 不会补发），主动加载一次远端默认目录
  if (activeSession.value?.backendId) void loadRemote()

  // 视口适配：上次保存的高度可能超出当前窗口（换显示器/全屏化），
  // 以 workspace / tabs 真实矩形夹取，上限即 Tab 下沿
  const g = geometry()
  if (g) {
    const maxH = g.wsBottom - g.tabsBottom
    if (maxH >= HEAD_H) dockHeight.value = Math.min(dockHeight.value, maxH)
    if (dockHeight.value < HEAD_H) dockHeight.value = DOCK_DEFAULT_HEIGHT
  }

  document.addEventListener('mousemove', onMove)
  document.addEventListener('mouseup', onUp)
  document.addEventListener('keydown', onFindShortcut)
  document.addEventListener('keydown', onSftpShortcut)
})
onUnmounted(() => {
  document.removeEventListener('mousemove', onMove)
  document.removeEventListener('mouseup', onUp)
  document.removeEventListener('keydown', onFindShortcut)
  document.removeEventListener('keydown', onSftpShortcut)
})
</script>

<template>
  <section ref="dockEl" class="dock" :class="{ collapsed: dockCollapsed, resizing: isResizing }"
           :style="{ height: dockHeight + 'px' }">
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
            <div class="spacer"></div>
            <button class="sftp-btn" title="刷新本地与远端目录" @click="refreshAll">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                   stroke-linecap="round" stroke-linejoin="round">
                <path d="M21 12a9 9 0 11-3-6.7L21 8"/><path d="M21 3v5h-5"/>
              </svg>
              刷新
            </button>
          </div>

          <!-- 双栏主体 -->
          <div class="sftp-main" :class="{ splitting: isSplitting }"
               :style="{ '--lr': sftpLocalRatio }">
            <!-- 本地 -->
            <div class="pane local"
                 :style="{ flex: `0 0 calc(100% * ${sftpLocalRatio})` }">
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
                <button class="pane-btn" title="刷新" :disabled="localLoading" @click="loadLocal(localPath)">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                       stroke-linecap="round" stroke-linejoin="round">
                    <path d="M21 12a9 9 0 11-3-6.7L21 8"/><path d="M21 3v5h-5"/>
                  </svg>
                </button>
                <button class="pane-btn" title="新建文件夹" @click="startEdit('local', 'mkdir')">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                       stroke-linecap="round" stroke-linejoin="round">
                    <path d="M4 7a2 2 0 012-2h3l2 2h7a2 2 0 012 2v8a2 2 0 01-2 2H6a2 2 0 01-2-2V7z"/>
                    <path d="M12 11v5M9.5 13.5h5" stroke-width="2.2"/>
                  </svg>
                </button>
                <!-- 导航区 | 传输区 -->
                <span class="sep"></span>
                <button class="pane-btn transfer-btn" :class="{ ready: localSel.length }"
                        :title="localSel.length ? `上传选中的 ${localSel.length} 个文件到远程` : '先在列表中选择文件'"
                        :disabled="!localSel.length"
                        @click="onUpload">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                       stroke-linecap="round" stroke-linejoin="round">
                    <path d="M12 19V5M5 12l7-7 7 7"/>
                  </svg>
                </button>
              </div>
              <div class="list-header">
                <div class="lh-name" :class="{ sorted: localSort.key==='name' }" @click="changeSort('local','name')">名称</div>
                <div class="lh-size" :class="{ sorted: localSort.key==='size' }" @click="changeSort('local','size')">大小</div>
                <div class="lh-time" :class="{ sorted: localSort.key==='mtime' }" @click="changeSort('local','mtime')">修改时间</div>
                <div class="lh-perm">权限</div>
                <div class="lh-owner">用户/组</div>
              </div>
              <div class="pane-list" @contextmenu="onBlankContext($event, 'local')">
                <div v-if="!localEntries.length && !(edit && edit.side==='local' && edit.kind!=='rename')" class="pane-empty">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"
                       stroke-linecap="round" stroke-linejoin="round">
                    <path d="M4 7a2 2 0 012-2h3l2 2h7a2 2 0 012 2v8a2 2 0 01-2 2H6a2 2 0 01-2-2V7z"/>
                  </svg>
                  <div>{{ localLoading ? '加载中…' : '此文件夹为空' }}</div>
                </div>
                <template v-else>
                  <div
                    v-for="e in localEntries"
                    :key="e.name"
                    class="row"
                    :class="{ dir: e.type==='dir', selected: localSelected.has(e.name) }"
                    @click="onRowClick('local', e, $event)"
                    @dblclick="onRowDblClick('local', e)"
                    @contextmenu.stop="onRowContext($event, 'local', e)"
                  >
                    <div class="row-name">
                      <svg class="row-icon" viewBox="0 0 24 24">
                        <path v-if="e.type==='dir'" d="M4 7a2 2 0 012-2h3l2 2h7a2 2 0 012 2v8a2 2 0 01-2 2H6a2 2 0 01-2-2V7z"/>
                        <template v-else>
                          <path d="M14 3H6a2 2 0 00-2 2v14a2 2 0 002 2h12a2 2 0 002-2V9z"/>
                          <path d="M14 3v6h6"/>
                        </template>
                      </svg>
                      <!-- 重命名：原位行内编辑；其它情况纯文本 -->
                      <input v-if="edit && edit.side==='local' && edit.kind==='rename' && edit.origName===e.name"
                             ref="editInput" v-model="editName" class="rename-input"
                             @keydown.enter.prevent="confirmEdit"
                             @keydown.esc.prevent="cancelEdit"
                             @blur="confirmEdit" />
                      <span v-else class="row-text">{{ e.name }}</span>
                      <svg v-if="e.isSymlink" class="link-badge" viewBox="0 0 16 16" title="符号链接">
                        <path d="M7 2H4a3 3 0 0 0 0 6h2" stroke="currentColor" stroke-width="1.8" fill="none" stroke-linecap="round"/>
                        <path d="M9 14h3a3 3 0 0 0 0-6h-2" stroke="currentColor" stroke-width="1.8" fill="none" stroke-linecap="round"/>
                      </svg>
                    </div>
                    <div class="row-size">{{ e.type==='dir' ? '—' : fmtSize(e.size) }}</div>
                    <div class="row-time">{{ e.mtime }}</div>
                    <div class="row-perm">{{ e.perm }}</div>
                    <div class="row-owner">{{ e.owner }}</div>
                  </div>
                  <!-- 新建文件夹/文件编辑行（列表底部，不隐藏已有文件） -->
                  <div v-if="edit && edit.side==='local' && (edit.kind==='mkdir' || edit.kind==='mkfile')"
                       class="row mkdir-row">
                    <div class="row-name">
                      <svg class="row-icon" viewBox="0 0 24 24">
                        <path v-if="edit.kind==='mkdir'" d="M4 7a2 2 0 012-2h3l2 2h7a2 2 0 012 2v8a2 2 0 01-2 2H6a2 2 0 01-2-2V7z"/>
                        <template v-else>
                          <path d="M14 3H6a2 2 0 00-2 2v14a2 2 0 002 2h12a2 2 0 002-2V9z"/>
                          <path d="M14 3v6h6"/>
                        </template>
                      </svg>
                      <input ref="editInput" v-model="editName" class="mkdir-input"
                             :placeholder="edit.kind==='mkdir' ? '输入文件夹名，回车确认' : '输入文件名，回车确认'"
                             @keydown.enter.prevent="confirmEdit"
                             @keydown.esc.prevent="cancelEdit"
                             @blur="confirmEdit" />
                    </div>
                  </div>
                </template>
              </div>
              <div class="pane-stats">
                <b>{{ localEntries.filter(e=>e.type==='dir').length }}</b> 目录 /
                <b>{{ localEntries.filter(e=>e.type==='file').length }}</b> 文件
                <template v-if="localSelected.size"> · 选中 {{ localSelected.size }}</template>
              </div>
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
                <button class="pane-btn" title="刷新" :disabled="!connected || remoteLoading"
                        @click="loadRemote(remotePath)">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                       stroke-linecap="round" stroke-linejoin="round">
                    <path d="M21 12a9 9 0 11-3-6.7L21 8"/><path d="M21 3v5h-5"/>
                  </svg>
                </button>
                <button class="pane-btn" title="新建文件夹" :disabled="!connected" @click="startEdit('remote', 'mkdir')">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                       stroke-linecap="round" stroke-linejoin="round">
                    <path d="M4 7a2 2 0 012-2h3l2 2h7a2 2 0 012 2v8a2 2 0 01-2 2H6a2 2 0 01-2-2V7z"/>
                    <path d="M12 11v5M9.5 13.5h5" stroke-width="2.2"/>
                  </svg>
                </button>
                <!-- 导航区 | 传输区 -->
                <span class="sep"></span>
                <button class="pane-btn transfer-btn" :class="{ ready: remoteSel.length }"
                        :title="remoteSel.length ? `下载选中的 ${remoteSel.length} 个文件到本地` : '先在列表中选择文件'"
                        :disabled="!remoteSel.length"
                        @click="onDownload">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                       stroke-linecap="round" stroke-linejoin="round">
                    <path d="M12 5v14M5 12l7 7 7-7"/>
                  </svg>
                </button>
              </div>
              <div class="list-header">
                <div class="lh-name" :class="{ sorted: remoteSort.key==='name' }" @click="changeSort('remote','name')">名称</div>
                <div class="lh-size" :class="{ sorted: remoteSort.key==='size' }" @click="changeSort('remote','size')">大小</div>
                <div class="lh-time" :class="{ sorted: remoteSort.key==='mtime' }" @click="changeSort('remote','mtime')">修改时间</div>
                <div class="lh-perm">权限</div>
                <div class="lh-owner">用户/组</div>
              </div>
              <div class="pane-list" @contextmenu="onBlankContext($event, 'remote')">
                <div v-if="!remoteEntries.length && !(edit && edit.side==='remote' && edit.kind!=='rename')" class="pane-empty">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"
                       stroke-linecap="round" stroke-linejoin="round">
                    <path d="M4 7a2 2 0 012-2h3l2 2h7a2 2 0 012 2v8a2 2 0 01-2 2H6a2 2 0 01-2-2V7z"/>
                  </svg>
                  <div>{{ !connected ? '未连接到远程主机' : remoteLoading ? '加载中…' : '此文件夹为空' }}</div>
                </div>
                <template v-else>
                  <div
                    v-for="e in remoteEntries"
                    :key="e.name"
                    class="row"
                    :class="{ dir: e.type==='dir', selected: remoteSelected.has(e.name) }"
                    @click="onRowClick('remote', e, $event)"
                    @dblclick="onRowDblClick('remote', e)"
                    @contextmenu.stop="onRowContext($event, 'remote', e)"
                  >
                    <div class="row-name">
                      <svg class="row-icon" viewBox="0 0 24 24">
                        <path v-if="e.type==='dir'" d="M4 7a2 2 0 012-2h3l2 2h7a2 2 0 012 2v8a2 2 0 01-2 2H6a2 2 0 01-2-2V7z"/>
                        <template v-else>
                          <path d="M14 3H6a2 2 0 00-2 2v14a2 2 0 002 2h12a2 2 0 002-2V9z"/>
                          <path d="M14 3v6h6"/>
                        </template>
                      </svg>
                      <input v-if="edit && edit.side==='remote' && edit.kind==='rename' && edit.origName===e.name"
                             ref="editInput" v-model="editName" class="rename-input"
                             @keydown.enter.prevent="confirmEdit"
                             @keydown.esc.prevent="cancelEdit"
                             @blur="confirmEdit" />
                      <span v-else class="row-text">{{ e.name }}</span>
                      <svg v-if="e.isSymlink" class="link-badge" viewBox="0 0 16 16" title="符号链接">
                        <path d="M7 2H4a3 3 0 0 0 0 6h2" stroke="currentColor" stroke-width="1.8" fill="none" stroke-linecap="round"/>
                        <path d="M9 14h3a3 3 0 0 0 0-6h-2" stroke="currentColor" stroke-width="1.8" fill="none" stroke-linecap="round"/>
                      </svg>
                    </div>
                    <div class="row-size">{{ e.type==='dir' ? '—' : fmtSize(e.size) }}</div>
                    <div class="row-time">{{ e.mtime }}</div>
                    <div class="row-perm">{{ e.perm }}</div>
                    <div class="row-owner">{{ e.owner }}</div>
                  </div>
                  <!-- 新建文件夹/文件编辑行（列表底部，不隐藏已有文件） -->
                  <div v-if="edit && edit.side==='remote' && (edit.kind==='mkdir' || edit.kind==='mkfile')"
                       class="row mkdir-row">
                    <div class="row-name">
                      <svg class="row-icon" viewBox="0 0 24 24">
                        <path v-if="edit.kind==='mkdir'" d="M4 7a2 2 0 012-2h3l2 2h7a2 2 0 012 2v8a2 2 0 01-2 2H6a2 2 0 01-2-2V7z"/>
                        <template v-else>
                          <path d="M14 3H6a2 2 0 00-2 2v14a2 2 0 002 2h12a2 2 0 002-2V9z"/>
                          <path d="M14 3v6h6"/>
                        </template>
                      </svg>
                      <input ref="editInput" v-model="editName" class="mkdir-input"
                             :placeholder="edit.kind==='mkdir' ? '输入文件夹名，回车确认' : '输入文件名，回车确认'"
                             @keydown.enter.prevent="confirmEdit"
                             @keydown.esc.prevent="cancelEdit"
                             @blur="confirmEdit" />
                    </div>
                  </div>
                </template>
              </div>
              <div class="pane-stats">
                <b>{{ remoteEntries.filter(e=>e.type==='dir').length }}</b> 目录 /
                <b>{{ remoteEntries.filter(e=>e.type==='file').length }}</b> 文件
                <template v-if="remoteSelected.size"> · 选中 {{ remoteSelected.size }}</template>
              </div>
            </div>

            <!-- 分割线：绝对定位，不占用 flex 空间，hover/拖拽高亮，双击均分 -->
            <div class="pane-divider" :class="{ dragging: isSplitting }"
                 title="拖拽调整宽度，双击恢复均分"
                 @mousedown="startSplitDrag"
                 @dblclick="onSplitDblClick">
              <div class="pane-divider-grip"></div>
            </div>
          </div>

          <!-- 传输队列 -->
          <div class="sftp-tasks" :class="{ collapsed: queueCollapsed }"
               :style="{ height: queueCollapsed ? '28px' : queueHeight + 'px' }">
            <div class="queue-resizer" @mousedown.prevent="startQueueDrag"></div>
            <!-- 头部：指示灯 + 标题 + 总数；右侧 全部暂停 / 全部继续 / 清空 / 折叠 -->
            <div class="queue-head">
              <span class="queue-led" :class="{ idle: !runningCount && !pendingCount }"></span>
              <span class="queue-title">传输队列</span>
              <span class="queue-badge">{{ transfers.length }}</span>
              <span class="spacer"></span>
              <button class="queue-act" title="全部暂停"
                      :disabled="!runningCount && !pendingCount" @click="pauseAllTasks">
                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor"
                     stroke-width="1.8" stroke-linecap="round">
                  <path d="M6 3.4v9.2"/><path d="M10 3.4v9.2"/>
                </svg>
              </button>
              <button class="queue-act" title="全部继续"
                      :disabled="!pausedCount" @click="resumeAllTasks">
                <svg viewBox="0 0 16 16" fill="currentColor">
                  <path d="M5.6 3.4 12.1 8l-6.5 4.6z"/>
                </svg>
              </button>
              <span class="queue-sep"></span>
              <button class="queue-clear" title="清空已完成"
                      :disabled="!successCount" @click="clearDoneTasks">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                     stroke-linecap="round" stroke-linejoin="round">
                  <path d="M4 7h16M9 7V4h6v3M6.5 7l.8 13h9.4l.8-13M10 11v5M14 11v5"/>
                </svg>
              </button>
              <button class="queue-fold"
                      :title="queueCollapsed ? '展开传输队列' : '收起传输队列'"
                      @click="toggleQueueCollapse">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                     stroke-linecap="round" stroke-linejoin="round">
                  <path v-if="queueCollapsed" d="M6 15l6-6 6 6"/>
                  <path v-else d="M6 9l6 6 6-6"/>
                </svg>
              </button>
            </div>
            <!-- 筛选标签 -->
            <div class="queue-tabs">
              <button v-for="f in ([
                { k: 'all', label: '全部' },
                { k: 'up', label: '上传' },
                { k: 'down', label: '下载' },
                { k: 'done', label: '已完成' },
              ] as const)" :key="f.k" class="q-tab"
                      :class="{ active: queueFilter === f.k }"
                      @click="queueFilter = f.k">{{ f.label }}</button>
            </div>
            <!-- 任务列表 -->
            <div class="queue-list" :style="{ maxHeight: queueListMaxHeight }">
              <div v-if="!transfers.length" class="queue-empty">暂无传输任务</div>
              <div v-for="t in filteredTransfers" :key="t.id"
                   class="sftp-task"
                   :class="[t.dir === 'up' ? 'upload' : 'download', t.status]">
                <div class="task-row">
                  <span class="task-twist-space"></span>
                  <!-- 方向图标 -->
                  <span class="task-ico">
                    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2"
                         stroke-linecap="round" stroke-linejoin="round">
                      <path v-if="t.dir==='up'" d="M8 12.6V3.4M3.9 7.5L8 3.4l4.1 4.1"/>
                      <path v-else d="M8 3.4v9.2M3.9 8.5L8 12.6l4.1-4.1"/>
                    </svg>
                  </span>
                  <!-- 主体：名称 / 进度条 / 字节·速率·备注 -->
                  <span class="task-body">
                    <span class="task-name">{{ taskName(t) }}</span>
                    <span class="track"><i class="fill" :style="{ width: Math.min(100, t.pct) + '%' }"></i></span>
                    <span class="task-sub">
                      <span class="bytes">{{ bytesText(t) }}</span>
                      <span v-if="rateText(t)" class="rate">{{ rateText(t) }}</span>
                      <span v-if="noteText(t)" class="note">{{ noteText(t) }}</span>
                    </span>
                  </span>
                  <!-- 状态 + 操作（图标保持不变，按状态显隐） -->
                  <span class="acts">
                    <span class="task-state">{{ stateText(t) }}</span>
                    <!-- 暂停：等待 / 传输中 -->
                    <button v-if="t.status === 'pending' || t.status === 'running'"
                            class="task-abtn" title="暂停" @click="pauseTask(t)">
                      <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8"
                           stroke-linecap="round">
                        <path d="M6 3.4v9.2"/><path d="M10 3.4v9.2"/>
                      </svg>
                    </button>
                    <!-- 继续：已暂停 -->
                    <button v-if="t.status === 'paused'" class="task-abtn" title="继续"
                            @click="resumeTask(t)">
                      <svg viewBox="0 0 16 16" fill="currentColor">
                        <path d="M5.6 3.4 12.1 8l-6.5 4.6z"/>
                      </svg>
                    </button>
                    <!-- 重试：失败 -->
                    <button v-if="t.status === 'error'" class="task-abtn" title="重试"
                            @click="retryTask(t)">
                      <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6"
                           stroke-linecap="round" stroke-linejoin="round">
                        <path d="M13.1 8a5.1 5.1 0 1 1-1.6-3.7"/><path d="M13.1 2.7v3.1H10"/>
                      </svg>
                    </button>
                    <!-- 打开目录：完成（上传→远程 / 下载→本地） -->
                    <button v-if="t.status === 'success'" class="task-abtn" title="打开目录"
                            @click="openTaskDir(t)">
                      <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"
                           stroke-linecap="round" stroke-linejoin="round">
                        <path d="M2.3 4.6a1 1 0 0 1 1-1h2.5l1.3 1.6h5.6a1 1 0 0 1 1 1v5.8a1 1 0 0 1-1 1H3.3a1 1 0 0 1-1-1z"/>
                      </svg>
                    </button>
                    <!-- 取消：未完成任务 -->
                    <button v-if="t.status !== 'success'" class="task-x" title="取消传输"
                            @click="removeTask(t)">
                      <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8"
                           stroke-linecap="round">
                        <path d="M4.3 4.3l7.4 7.4M11.7 4.3l-7.4 7.4"/>
                      </svg>
                    </button>
                    <!-- 清除：已完成记录 -->
                    <button v-else class="task-x" title="清除记录" @click="removeTask(t)">
                      <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"
                           stroke-linecap="round" stroke-linejoin="round">
                        <path d="M3 4.4h10"/><path d="M6.4 4.4V3.2h3.2v1.2"/>
                        <path d="M4.4 4.4l.6 8a1 1 0 0 0 1 .9h4a1 1 0 0 0 1-.9l.6-8"/>
                      </svg>
                    </button>
                  </span>
                </div>
              </div>
            </div>
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
          <button class="mini-ico" :class="{ active: logWrap }"
                  :title="logWrap ? '自动换行（点击关闭，长行横向滚动）' : '自动换行已关闭（点击开启）'"
                  @click="logWrap = !logWrap">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                 stroke-linecap="round" stroke-linejoin="round">
              <line x1="3" y1="7" x2="21" y2="7"></line>
              <path d="M3 12h12.5a2.5 2.5 0 0 1 0 5H11"></path>
              <polyline points="8.5 14 6 17 8.5 20"></polyline>
              <line x1="3" y1="21" x2="14" y2="21"></line>
            </svg>
          </button>
          <button class="mini-ico" title="打开日志目录" @click="revealLogDir">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                 stroke-linecap="round" stroke-linejoin="round">
              <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
            </svg>
          </button>
          <button class="mini-ico" title="清空日志（仅清内存缓冲，磁盘文件保留）" @click="clearLogs">
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

        <div ref="logListEl" class="log-list" :class="{ nowrap: !logWrap }" @scroll="onLogScroll">
          <div v-if="!savedSettings.logCollect" class="log-empty">日志采集已关闭（设置 → 日志 → 日志采集）</div>
          <div v-else-if="!filteredLogs.length" class="log-empty">暂无日志</div>
          <template v-else>
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
          </template>
        </div>
      </div>
    </div>

    <!-- 右键菜单（Teleport 到 body，定位与键盘导航由组件内部完成） -->
    <ContextMenu v-model:visible="menuVisible" :x="menuX" :y="menuY" :items="menuItems" />
  </section>
</template>
