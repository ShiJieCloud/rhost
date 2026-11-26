<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { invoke, Channel } from '@tauri-apps/api/core'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import type { DragDropEvent } from '@tauri-apps/api/webview'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { activeHost, activeSession, activeSftpCwd, closeDock, connected, dockCollapsed, dockHeight, DOCK_DEFAULT_HEIGHT, dockTab, dockVisible, searchTick, sftpLocalRatio, sftpQueueCollapsed, sftpQueueHeight } from '../../stores/session'
import { persistSettings, savedSettings } from '../../stores/settings'
import { appLocked } from '../../stores/appLock'
import { toast } from '../../composables/useToast'
import { confirmDialog } from '../../composables/useConfirm'
import TunnelPane from './TunnelPane.vue'
import { isTauri } from '../../lib/tauri'
import { getSnapshot, onConfigLoad, patchUiState } from '../../stores/appConfig'
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

/** SFTP 操作冻结条件（app-lock-design.md §6.5）：应用锁（遮罩兜底）或激活会话被锁。
 *  进行中的传输不中断，但新建传输/文件管理/任务控制一律禁止。 */
const sftpLocked = computed(() => appLocked.value)

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
const menu = ref<{ x: number; y: number; side: Side; targets: FsEntry[]; pane?: boolean } | null>(null)

/* 最后交互的一侧：快捷键作用域（点击/右键/导航时更新） */
const lastSide = ref<Side>('local')

const localSelected = ref<Set<string>>(new Set())
const remoteSelected = ref<Set<string>>(new Set())
const localSort = ref<SortState>({ key: 'name', dir: 1 })
const remoteSort = ref<SortState>({ key: 'name', dir: 1 })

/* ---- 目录导航历史：返回 / 前进 ---- */
interface NavHistory { stack: string[]; idx: number }
const localHist = ref<NavHistory>({ stack: [], idx: -1 })
const remoteHist = ref<NavHistory>({ stack: [], idx: -1 })
function histOf(side: Side) {
  return side === 'local' ? localHist : remoteHist
}
/** 记录一次成功加载；重复路径（刷新）不产生新历史 */
function navRecord(side: Side, path: string) {
  const h = histOf(side).value
  if (h.stack[h.idx] === path) return
  h.stack = [...h.stack.slice(0, h.idx + 1), path]
  h.idx = h.stack.length - 1
}
function canGoBack(side: Side) {
  return histOf(side).value.idx > 0
}
function canGoForward(side: Side) {
  const h = histOf(side).value
  return h.idx >= 0 && h.idx < h.stack.length - 1
}
async function goBack(side: Side) {
  const h = histOf(side).value
  if (h.idx <= 0) return
  if (side === 'remote' && sftpLocked.value) return
  const target = h.stack[h.idx - 1]!
  const ok = side === 'local' ? await loadLocal(target, false, false) : await loadRemote(target, false, false)
  if (!ok) return
  h.idx -= 1
  if (side === 'remote') syncShellCwd(target)
}
async function goForward(side: Side) {
  const h = histOf(side).value
  if (h.idx >= h.stack.length - 1) return
  if (side === 'remote' && sftpLocked.value) return
  const target = h.stack[h.idx + 1]!
  const ok = side === 'local' ? await loadLocal(target, false, false) : await loadRemote(target, false, false)
  if (!ok) return
  h.idx += 1
  if (side === 'remote') syncShellCwd(target)
}

/* 本地目录真实数据：由后端 list_local_dir 加载；localPath 始终对应已加载成功的目录 */
const localRaw = ref<FsEntry[]>([])
const localLoading = ref(false)

async function loadLocal(path?: string, quiet = false, recordHistory = true): Promise<boolean> {
  if (!isTauri) return true // 浏览器 dev 模式无本地 FS，保持空列表
  localLoading.value = true
  try {
    const res = await invoke<DirListing>('list_local_dir', { path: path ?? null })
    localRaw.value = res.entries.map(toFsEntry)
    localPath.value = res.path
    localSelected.value = new Set()
    localFilter.value = ''
    if (recordHistory) navRecord('local', res.path)
    return true
  } catch (e) {
    if (!quiet) toast(String(e), 'warn', 2200)
    return false
  } finally {
    localLoading.value = false
  }
}

/* 远端目录真实数据：由后端 sftp_list_dir 经会话 SFTP 子系统加载。
   remotePath 始终对应已加载成功的目录；请求失败时停留在原目录。 */
const remoteRaw = ref<FsEntry[]>([])
const remoteLoading = ref(false)
let remoteReqSeq = 0 // 竞态守卫：快速切目录/切会话时丢弃过期响应

async function loadRemote(path?: string, quiet = false, recordHistory = true): Promise<boolean> {
  const backendId = activeSession.value?.backendId
  if (!isTauri || !backendId) return false // 浏览器 dev 模式 / 未连接：保持空列表
  if (sftpLocked.value) return false // 锁定期间不发起远端浏览，保留锁定前列表
  const req = ++remoteReqSeq
  remoteLoading.value = true
  try {
    const res = await invoke<DirListing>('sftp_list_dir', {
      sessionId: backendId,
      path: path ?? null,
    })
    // 过期响应（用户已继续导航或切换/断开会话）直接丢弃，避免覆盖新视图
    if (req !== remoteReqSeq || activeSession.value?.backendId !== backendId) return true
    remoteRaw.value = res.entries.map(toFsEntry)
    remotePath.value = res.path
    remoteSelected.value = new Set()
    remoteFilter.value = ''
    if (recordHistory) navRecord('remote', res.path)
    return true
  } catch (e) {
    if (req !== remoteReqSeq) return true
    if (!quiet) toast(String(e), 'warn', 2600)
    return false
  } finally {
    if (req === remoteReqSeq) remoteLoading.value = false
  }
}

/* ---- 路径栏编辑：单击进入输入态，回车跳转，Esc 取消 ---- */
const pathEditing = ref<Side | null>(null)
const pathDraft = ref('')

function focusPathInput(side: Side, select: boolean) {
  void nextTick(() => {
    const el = document.querySelector<HTMLInputElement>(`.pane.${side} .pane-path-input`)
    el?.focus()
    if (select) el?.select()
  })
}

/** 绝对路径校验：Unix 以 / 开头；Windows 本地允许盘符路径 */
function isAbsolutePath(p: string, side: Side) {
  if (p.startsWith('/')) return true
  return side === 'local' && /^[A-Za-z]:[\\/]/.test(p)
}

function startPathEdit(side: Side) {
  if (side === 'remote' && (!connected.value || sftpLocked.value)) return
  pathEditing.value = side
  // 有筛选时保持筛选词，否则预填当前路径；进入编辑即全选便于替换
  const filter = side === 'local' ? localFilter.value : remoteFilter.value
  pathDraft.value = filter || (side === 'local' ? localPath.value : remotePath.value)
  focusPathInput(side, true)
}

/** 路径栏空白处单击进入编辑；面包屑与输入框自身的点击不触发 */
function onPathClick(side: Side, e: MouseEvent) {
  if (pathEditing.value) return
  const el = e.target as HTMLElement
  if (el.closest('.crumb') || el.closest('.pane-path-input')) return
  startPathEdit(side)
}

async function commitPathEdit(side: Side) {
  // Esc / Enter 会先清空编辑态，届时 blur 回调直接忽略
  if (pathEditing.value !== side) return
  const target = pathDraft.value.trim()
  // 空值、相对路径与后端拒绝统一提示，不暴露后端原始措辞
  if (!target || !isAbsolutePath(target, side)) {
    toast(`无法打开目录：${target || '（空）'}`, 'warn', 2200)
    return
  }
  pathEditing.value = null
  const ok = side === 'local' ? await loadLocal(target, true) : await loadRemote(target, true)
  if (!ok) {
    toast(`无法打开目录：${target}`, 'warn', 2400)
    // 保留输入并回到编辑态，便于修改后重试
    pathEditing.value = side
    focusPathInput(side, false)
  }
}

function onPathKey(e: KeyboardEvent, side: Side) {
  if (e.key === 'Enter') {
    e.preventDefault()
    // 含 "/" 视为目录路径并跳转；否则按筛选处理（关闭编辑但保留筛选）
    if (pathDraft.value.includes('/')) void commitPathEdit(side)
    else pathEditing.value = null
  } else if (e.key === 'Escape') {
    e.preventDefault()
    pathEditing.value = null
  }
}

/** 下发会话级 SFTP 空闲超时：仅控制 SFTP 子通道回收，不影响 SSH 终端会话。
 *  设置变更/切换会话时同步，后端据此在长时间无操作且无传输时关闭缓存通道。 */function applySftpIdleTimeout() {
  const backendId = activeSession.value?.backendId
  if (!isTauri || !backendId) return
  const secs = Math.max(0, savedSettings.sftpIdleTimeoutSec | 0)
  void invoke('sftp_set_idle_timeout', { sessionId: backendId, secs }).catch(() => {})
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
    if (sftpLocked.value) throw new Error('会话已锁定，请先解锁')
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
/* 状态文案 */
function crumbs(path: string) {
  const parts = path.split('/').filter(Boolean)
  // 根目录整体显示为一个可点击的 “/”
  if (!parts.length) return [{ label: '/', path: '/' }]
  // 根节点只用于分隔：不显示文字，避免出现开头的 “//home”
  const out: { label: string; path: string }[] = [{ label: '', path: '/' }]
  let acc = ''
  parts.forEach(p => { acc += '/' + p; out.push({ label: p, path: acc }) })
  return out
}

/** 按全局设置过滤隐藏文件（. 开头）；开启时原样返回 */
function visibleEntries(list: FsEntry[]): FsEntry[] {
  return savedSettings.sftpShowHidden ? list : list.filter(e => !e.name.startsWith('.'))
}
/* ---- 目录内筛选：路径栏输入不含 "/" 时按名称实时过滤 ---- */
const localFilter = ref('')
const remoteFilter = ref('')

/** 仅在用户实际输入时更新筛选，进入编辑时预填路径不触发清空 */
function onPathInput(side: Side) {
  const f = pathDraft.value.includes('/') ? '' : pathDraft.value
  if (side === 'local') localFilter.value = f
  else remoteFilter.value = f
}
function filterEntries(list: FsEntry[], q: string) {
  const s = q.trim().toLowerCase()
  return s ? list.filter(e => e.name.toLowerCase().includes(s)) : list
}
/** 把文件名按当前筛选词切段，命中段用于高亮 */
function nameSegments(name: string, side: Side) {
  const q = (side === 'local' ? localFilter.value : remoteFilter.value).trim()
  if (!q) return [{ text: name, hit: false }]
  const lower = name.toLowerCase()
  const needle = q.toLowerCase()
  const out: { text: string; hit: boolean }[] = []
  let i = 0
  while (i < name.length) {
    const j = lower.indexOf(needle, i)
    if (j < 0) {
      out.push({ text: name.slice(i), hit: false })
      break
    }
    if (j > i) out.push({ text: name.slice(i, j), hit: false })
    out.push({ text: name.slice(j, j + needle.length), hit: true })
    i = j + needle.length
  }
  return out
}
const localEntries = computed(() => filterEntries(sortEntries(visibleEntries(localRaw.value), localSort.value), localFilter.value))
const remoteEntries = computed(() => filterEntries(sortEntries(visibleEntries(remoteRaw.value), remoteSort.value), remoteFilter.value))

/** 轻量 glob 匹配：支持 `*`（任意字符）与 `?`（单字符），其余正则元字符转义。
 *  用于上传黑名单过滤，不引入 minimatch 依赖。 */
function globMatch(name: string, pattern: string): boolean {
  const escaped = pattern.replace(/[.+^${}()|[\]\\]/g, '\\$&') // 转义除 * ? 外的正则元字符
  const re = '^' + escaped.replace(/\*/g, '.*').replace(/\?/g, '.') + '$'
  return new RegExp(re).test(name)
}
/** 文件名是否命中上传黑名单（一行一条 glob，空行忽略） */
function isBlacklisted(name: string): boolean {
  const list = (savedSettings.sftpBlacklist || '')
    .split(/\r?\n/).map(s => s.trim()).filter(Boolean)
  return list.some(p => globMatch(name, p))
}

/* ---- 传输队列：pending → running → success/error/cancelled，按并发配额调度
   状态色规范：pending #606266 · running #409EFF · paused #E6A23C · error #F56C6C · success 项目绿 · cancelled 灰 ---- */
type TransferStatus = 'pending' | 'running' | 'paused' | 'success' | 'error' | 'cancelled'
interface Transfer {
  id: number; name: string; dir: 'up' | 'down'
  size: number; pct: number; status: TransferStatus
  speed: number // 展示速率 B/s（真实传输由后端进度事件提供）
  /** 首次开始传输的墙钟时间戳（ms），用于完成事件 elapsed_ms；暂停时间计入 */
  startAt: number
  error?: string // 失败详情（查看错误弹窗展示）
  localPath?: string   // 本地绝对路径
  remotePath?: string  // 远端绝对路径
  /** 跨侧剪切粘贴：传输成功后要删除的源文件绝对路径（文件夹任务为源目录） */
  cutSrc?: string
  /** 任务所属会话 backendId：用于按主机并发计数与断线后归属判断 */
  backendId?: string
  /** 按覆盖策略跳过（目标已存在且无需覆盖），未发生实际传输 */
  skipped?: boolean
  /** 字节已传完、正在做 SHA256 比对（收尾前的过渡态） */
  verifying?: boolean
  /** ask 策略下用户已确认覆盖：重试/重跑该任务时固定按 overwrite 执行 */
  forceOverwrite?: boolean
  /** 任务类型：缺省为文件；dir 表示文件夹任务的汇总行（本身不参与传输） */
  kind?: 'dir'
  /** 文件夹任务包含的文件数（扫描完成后填充） */
  fileCount?: number
  /** 文件夹任务是否展开子文件列表 */
  expanded?: boolean
  /** 文件夹任务正在递归扫描源目录 */
  scanning?: boolean
  /** 子文件任务所属的文件夹任务 id；有值时不在队列顶层展示 */
  groupId?: number
}
const transfers = ref<Transfer[]>([])
let seq = 0

/** 队列顶层项（文件夹任务与其子文件任务只展示汇总行） */
const queueItems = computed(() => transfers.value.filter(t => t.groupId === undefined))
/** 子文件任务按所属文件夹任务归档（缓存，避免每次渲染重复扫描全队列） */
const membersByGroup = computed(() => {
  const map = new Map<number, Transfer[]>()
  for (const t of transfers.value) {
    if (t.groupId === undefined) continue
    const arr = map.get(t.groupId)
    if (arr) arr.push(t)
    else map.set(t.groupId, [t])
  }
  return map
})

/** 后端 SFTP 进度事件载荷 */
interface TransferProgress {
  taskId: number
  transferred: number
  total: number
  speed: number
  done: boolean
  /** 收尾包专用：按覆盖策略跳过（目标已存在） */
  skipped: boolean
  /** 字节已传完、正在做 SHA256 比对 */
  verifying: boolean
}

const runningCount = computed(() => queueItems.value.filter(t => t.status === 'running').length)
const pendingCount = computed(() => queueItems.value.filter(t => t.status === 'pending').length)
const pausedCount = computed(() => queueItems.value.filter(t => t.status === 'paused').length)
const successCount = computed(() => queueItems.value.filter(t => t.status === 'success').length)
const cancelledCount = computed(() => queueItems.value.filter(t => t.status === 'cancelled').length)

/* 筛选：全部 / 上传 / 下载 / 已完成（仅顶层任务，文件夹子文件归组展示） */
const queueFilter = ref<'all' | 'up' | 'down' | 'done'>('all')
const filteredTransfers = computed(() => {
  const f = queueFilter.value
  return queueItems.value.filter(t =>
    f === 'all' ? true
    : f === 'done' ? t.status === 'success'
    : t.dir === f,
  )
})

/* 队列跟随：默认贴底展示最新任务；用户上滚查看历史时暂停跟随，
   新任务改为在右下角提示，避免打断阅读 */
const queueListEl = ref<HTMLElement | null>(null)
const queueFollow = ref(true)
const queueNewCount = ref(0)
const QUEUE_STICK_GAP = 24
function queueAtBottom() {
  const el = queueListEl.value
  return !!el && el.scrollHeight - el.scrollTop - el.clientHeight < QUEUE_STICK_GAP
}
function scrollQueueToLatest() {
  const el = queueListEl.value
  if (el) el.scrollTop = el.scrollHeight
  queueFollow.value = true
  queueNewCount.value = 0
}
function onQueueScroll() {
  queueFollow.value = queueAtBottom()
  if (queueFollow.value) queueNewCount.value = 0
}
watch(() => queueItems.value.length, (n, o) => {
  if (n > o) {
    if (queueFollow.value) nextTick(scrollQueueToLatest)
    else queueNewCount.value += n - o
  } else if (n < o && queueNewCount.value) {
    queueNewCount.value = Math.min(queueNewCount.value, n)
  }
})
/* 展开队列时若处于跟随状态，回到最新位置 */
watch(sftpQueueCollapsed, collapsed => {
  if (!collapsed && queueFollow.value) nextTick(scrollQueueToLatest)
})

/* 队列面板：head 28 + tabs 28；折叠/高度状态存 ui_state（session store），
   跨重启保留；首个任务入队时自动展开，折叠态头部以 queueSummary 展示进度摘要 */
const queueListMaxHeight = computed(() =>
  sftpQueueCollapsed.value ? '0px' : Math.max(0, sftpQueueHeight.value - 56) + 'px')
function toggleQueueCollapse() {
  sftpQueueCollapsed.value = !sftpQueueCollapsed.value
  // 拖拽收起到 28 后再展开：恢复默认高度，避免列表区高度为负
  if (!sftpQueueCollapsed.value && sftpQueueHeight.value < 120) sftpQueueHeight.value = 180
}
/** 点击头部空白区（非按钮）切换折叠，提升折叠态可发现性 */
function onQueueHeadClick(e: MouseEvent) {
  if ((e.target as HTMLElement).closest('button')) return
  toggleQueueCollapse()
}

/** 折叠态头部摘要：优先进行中任务（方向/名称/百分比/速率），
 *  否则按 等待 → 暂停 → 失败 → 完成 汇总；空闲留空。
 *  子文件任务运行中时，以所属文件夹任务的名称聚合展示 */
const queueSummary = computed(() => {
  const list = queueItems.value
  if (!list.length) return ''
  const r = list.find(t => t.status === 'running')
  if (r) {
    const arrow = r.dir === 'up' ? '↑' : '↓'
    const speed = r.speed > 0 ? ' · ' + fmtSpeed(r.speed) : ''
    return `${arrow} ${r.name} ${Math.floor(Math.min(100, r.pct))}%${speed}`
  }
  const pending = list.filter(t => t.status === 'pending').length
  if (pending) return `等待中 ${pending} 项`
  const paused = list.filter(t => t.status === 'paused').length
  if (paused) return `已暂停 ${paused} 项`
  const failed = list.filter(t => t.status === 'error').length
  if (failed) return `${failed} 项失败`
  const done = list.filter(t => t.status === 'success').length
  if (done) return `已完成 ${done} 项`
  const cancelled = list.filter(t => t.status === 'cancelled').length
  return cancelled ? `已取消 ${cancelled} 项` : ''
})

function fmtSpeed(bps: number) {
  if (!bps || bps <= 0) return '0 B/s'
  return fmtSize(bps) + '/s'
}
/* 队列内展示名 */
function taskName(t: Transfer) {
  return t.name
}
/* 文件夹任务的子文件任务（按 groupId 归组，保持入队顺序） */
function groupMembers(g: Transfer) {
  return membersByGroup.value.get(g.id) || []
}
/* 文件夹任务已完成文件数 */
function doneChildren(t: Transfer) {
  return groupMembers(t).filter(c => c.status === 'success').length
}
/* 子项清单：与顶层任务共用 Transfer 结构 */
function visibleChildren(t: Transfer) {
  return groupMembers(t)
}
/* 子项状态文案：进度/速率/SHA-256 各自成列，此处只表达结果；
   完成态由进度 100% 与绿色校验结果表达，不再重复文案 */
function childStateText(c: Transfer) {
  // 覆盖策略跳过：未发生实际传输，先于 success 判断，否则会落到空文案
  if (c.skipped) return '已跳过'
  if (c.status === 'success') return ''
  if (c.status === 'running') return c.verifying ? '校验中' : '传输中'
  if (c.status === 'paused') return '已暂停'
  if (c.status === 'pending') return '等待中'
  if (c.status === 'error') return c.error || '失败'
  return '已取消'
}
/* 子项进度百分比 */
function childPctText(c: Transfer) {
  // 跳过子项未传任何字节，不显示 0% 以免误读为未开始
  if (c.skipped) return '—'
  return Math.floor(Math.min(100, Math.max(0, c.pct))) + '%'
}
/* 依据子文件任务汇总文件夹任务的进度 / 速度 / 状态；
   状态跃迁时触发一次完成/失败提示与收尾动作 */
function syncGroup(g: Transfer) {
  const members = groupMembers(g)
  if (!members.length) return
  const prev = g.status
  const total = members.reduce((s, c) => s + (c.size || 0), 0)
  const done = members.reduce((s, c) => s + (c.size || 0) * Math.min(100, Math.max(0, c.pct)) / 100, 0)
  const avgPct = members.reduce((s, c) => s + Math.min(100, Math.max(0, c.pct)), 0) / members.length
  g.size = total
  g.pct = total > 0 ? (done / total) * 100 : avgPct
  g.speed = members.reduce((s, c) => s + (c.status === 'running' ? (c.speed || 0) : 0), 0)
  if (members.some(c => c.status === 'running')) g.status = 'running'
  else if (members.some(c => c.status === 'paused')) g.status = 'paused'
  else if (members.some(c => c.status === 'pending')) g.status = 'pending'
  else if (members.some(c => c.status === 'error')) g.status = 'error'
  else if (members.every(c => c.status === 'cancelled')) g.status = 'cancelled'
  else g.status = 'success'
  if (g.status === 'success') g.pct = 100
  if (prev === g.status) return
  if (g.status === 'success') onFolderDone(g)
  else if (g.status === 'error') {
    const failed = members.filter(c => c.status === 'error').length
    toast(`文件夹${g.dir === 'up' ? '上传' : '下载'}有 ${failed} 个文件失败：${g.name}`, 'warn', 2600)
  }
}
/* 子任务状态变化后回写所属文件夹任务 */
function touchGroup(t: Transfer) {
  if (!t.groupId) return
  const g = transfers.value.find(x => x.id === t.groupId)
  if (g) syncGroup(g)
}
/* 文件夹传输完成：提示、剪切源删除、对侧目录刷新 */
function onFolderDone(g: Transfer) {
  const dirText = g.dir === 'up' ? '上传' : '下载'
  toast(`文件夹${dirText}完成：${g.name}`, 'ok', 2200)
  reportLog('info', SFTP_TRANSFER_COMPLETE, `文件夹${dirText}完成`, {
    name: g.name, size: g.size, files: g.fileCount ?? 0,
  })
  if (g.cutSrc) {
    // 安全兜底：仅当扫描到的全部文件都成功才删除剪切源，
    // 避免用户移除队列子项后误删未传输的源文件
    const okCount = groupMembers(g).filter(c => c.status === 'success').length
    if (okCount !== (g.fileCount ?? 0)) {
      reportLog('warn', SFTP_MANAGE_REMOVE, '剪切源未删除：存在未成功传输的文件', {
        name: g.name, ok: okCount, total: g.fileCount ?? 0,
      })
    } else if (g.dir === 'up') {
      void invoke('local_remove', { path: g.cutSrc })
        .catch(e => reportLog('error', SFTP_MANAGE_REMOVE, '剪切源删除失败', { path: g.cutSrc, err: String(e) }))
    } else {
      void invoke('sftp_remove', { sessionId: g.backendId || '', path: g.cutSrc })
        .catch(e => reportLog('error', SFTP_MANAGE_REMOVE, '剪切源删除失败', { path: g.cutSrc, err: String(e) }))
    }
  }
  if (g.dir === 'up') void loadRemote(remotePath.value)
  else void loadLocal(localPath.value)
}
/* 展开 / 收起文件夹任务的子文件列表 */
function toggleTaskExpand(t: Transfer) {
  t.expanded = !t.expanded
}
/* 已传输字节数 */
function doneBytes(t: Transfer) {
  return Math.min(t.size, Math.max(0, t.size * Math.min(100, Math.max(0, t.pct)) / 100))
}
/* 任务目标路径：上传=远端目标，下载=本地目标 */
function targetPath(t: Transfer) {
  return (t.dir === 'up' ? t.remotePath : t.localPath) || ''
}
/* 进度百分比文案：按覆盖策略跳过时未传输任何字节，不显示 100% */
function pctText(t: Transfer) {
  if (t.skipped) return '—'
  return Math.floor(Math.min(100, Math.max(0, t.pct))) + '%'
}
/* 瞬时速率：仅运行中 */
function speedText(t: Transfer) {
  return t.status === 'running' && t.speed > 0 ? fmtSpeed(t.speed) : ''
}
/* 文件夹任务的已完成/总数：属于进度信息，紧跟百分比展示，不参与结果区靠右 */
function folderCountText(t: Transfer) {
  if (t.kind !== 'dir' || t.scanning) return ''
  return `${doneChildren(t)}/${t.fileCount ?? 0}`
}
/* 剩余时间文案：按剩余字节与当前速度估算；无法估算或不足 1 秒时返回空 */
function etaText(remainBytes: number, speed: number) {
  if (!(speed > 0)) return ''
  const sec = remainBytes / speed
  if (!isFinite(sec) || sec < 1) return ''
  if (sec < 60) return '剩 ' + Math.ceil(sec) + ' 秒'
  if (sec < 3600) return '剩 ' + Math.ceil(sec / 60) + ' 分'
  return '剩 ' + (sec / 3600).toFixed(1) + ' 时'
}
/* 进度备注：剩余时间 / 校验中 / 状态 / 错误 */
function noteText(t: Transfer) {
  // 跳过状态已由任务名后的标签表达，备注区留空避免重复
  if (t.skipped) return ''
  // 文件夹任务：完成计数已独立展示；剩余时间按整组剩余字节/聚合速度估算
  if (t.kind === 'dir') {
    if (t.scanning) return '正在扫描文件夹…'
    if (t.status === 'error') {
      const members = groupMembers(t)
      const failed = members.filter(c => c.status === 'error').length
      return failed ? `失败 ${failed}/${t.fileCount ?? members.length}` : (t.error || '传输失败')
    }
    if (t.status === 'running') return etaText(t.size - doneBytes(t), t.speed)
    return ''
  }
  if (t.status === 'running') {
    if (t.verifying || (t.pct >= 99.9 && savedSettings.sftpVerifyHash)) return '校验中'
    return etaText(t.size - doneBytes(t), t.speed)
  }
  if (t.status === 'success') return ''
  if (t.status === 'paused') return '已暂停'
  if (t.status === 'pending') return '等待中'
  if (t.status === 'error') return t.error || '传输失败'
  return ''
}
function startQueueDrag(e: MouseEvent) {
  e.preventDefault()
  const startY = e.clientY
  const startH = sftpQueueHeight.value
  const sftpEl = (e.target as HTMLElement).closest('.sftp') as HTMLElement | null
  /* 最大高度需为顶栏、路径栏与文件列表保留可视空间：
     直接用整块高度会让队列拖到顶部后继续增高，内容下溢被底部状态栏裁掉 */
  const MIN_ABOVE = 160
  const maxH = sftpEl ? Math.max(120, sftpEl.offsetHeight - MIN_ABOVE) : 500
  document.body.style.userSelect = 'none'
  document.body.style.cursor = 'ns-resize'

  const onMove = (ev: MouseEvent) => {
    const delta = ev.clientY - startY
    const h = startH - delta
    if (h <= 40) {
      sftpQueueCollapsed.value = true
      sftpQueueHeight.value = 28
    } else {
      sftpQueueCollapsed.value = false
      sftpQueueHeight.value = Math.min(maxH, h)
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
  // 会话锁/应用锁：禁止新建传输（进行中的不中断，见拦截矩阵）
  if (sftpLocked.value) {
    toast('会话已锁定，请先解锁', 'warn')
    return
  }
  // 上传黑名单：命中 glob 规则的文件不入队（仅上传，过滤本地 .DS_Store 等垃圾文件）
  if (dir === 'up' && isBlacklisted(name)) {
    toast(`已跳过黑名单文件：${name}`, 'warn', 1800)
    reportLog('info', SFTP_TRANSFER_ENQUEUE, '上传命中黑名单，已跳过', { name })
    return
  }
  /* 仅在队列从空闲进入活动（首个任务）时自动展开；
     活动期间用户手动折叠后，后续入队保持折叠，进度看头部摘要 */
  const wasIdle = transfers.value.length === 0
  transfers.value.push({
    id: ++seq, name, dir, size, pct: 0, status: 'pending', speed: 0, startAt: 0,
    localPath: extra.localPath,
    remotePath: extra.remotePath,
    cutSrc: extra.cutSrc,
    backendId: activeSession.value?.backendId || '',
  })
  if (wasIdle) sftpQueueCollapsed.value = false
  reportLog('info', SFTP_TRANSFER_ENQUEUE, `${dir === 'up' ? '上传' : '下载'}加入队列`, {
    name, size, dir, from: extra.localPath, to: extra.remotePath,
  })
  schedulePump()
}

/** 调度入口：合并同一微任务批次内的多次触发，避免并发完成事件重入调度。 */
let pumpScheduled = false
function schedulePump() {
  if (pumpScheduled) return
  pumpScheduled = true
  queueMicrotask(() => {
    pumpScheduled = false
    pumpQueue()
  })
}

/** 按 sftpGlobalConcurrency / sftpHostConcurrency 配额，同步循环启动所有可跑的 pending 任务。
 *  实际传输在 runTask 中异步执行；runTask 首句同步置 running，防止同一任务被重复派发。 */
function pumpQueue() {
  if (sftpLocked.value) return // 锁定期间排队任务不启动，解锁后由 watcher 续跑
  const globalLimit = Math.max(1, savedSettings.sftpGlobalConcurrency | 0)
  const hostLimit = Math.max(1, savedSettings.sftpHostConcurrency | 0)
  // eslint-disable-next-line no-constant-condition
  while (true) {
    // 文件夹汇总行不参与传输；子文件任务按普通文件任务占用并发配额
    const running = transfers.value.filter(t => t.status === 'running' && t.kind !== 'dir')
    if (running.length >= globalLimit) break
    const hostCount = new Map<string, number>()
    for (const r of running) hostCount.set(r.backendId || '', (hostCount.get(r.backendId || '') || 0) + 1)
    const next = transfers.value.find(
      x => x.status === 'pending' && x.kind !== 'dir' && (hostCount.get(x.backendId || '') || 0) < hostLimit,
    )
    if (!next) break
    void runTask(next)
  }
}

/** 单任务限速分配：全局限速均分到当前并发数，再与单任务上限取小（0 表示不限）。 */
function computeTaskRateKb(runningBefore: number): number {
  const globalKb = savedSettings.sftpGlobalRateKb | 0
  const taskKb = savedSettings.sftpTaskRateKb | 0
  if (globalKb <= 0) return taskKb
  const share = Math.floor(globalKb / Math.max(1, runningBefore + 1))
  return taskKb <= 0 ? share : Math.min(taskKb, share)
}

/** 是否仍有并发配额（供恢复任务前检查，避免超过上限） */
function hasCapacity(backendId: string): boolean {
  const globalLimit = Math.max(1, savedSettings.sftpGlobalConcurrency | 0)
  const hostLimit = Math.max(1, savedSettings.sftpHostConcurrency | 0)
  const running = transfers.value.filter(t => t.status === 'running' && t.kind !== 'dir')
  if (running.length >= globalLimit) return false
  return running.filter(t => (t.backendId || '') === backendId).length < hostLimit
}

/** 启动单个传输任务：真实 SFTP invoke，进度由后端 Channel 流式推送。 */
async function runTask(t: Transfer) {
  const taskBackendId = t.backendId || activeSession.value?.backendId || ''
  const runningBefore = transfers.value.filter(x => x.status === 'running').length
  const taskRateKb = computeTaskRateKb(runningBefore)
  t.status = 'running'
  t.verifying = false // 新一轮传输/重试从传输阶段重新开始
  if (t.startAt === 0) t.startAt = Date.now() // 仅首次开始计时；暂停后续传不重置
  touchGroup(t)
  // pct > 0 为暂停/重试后的恢复，不再重复"开始"提示
  if (t.pct > 0) {
    reportLog('debug', SFTP_TRANSFER_RESUME, `${t.dir === 'up' ? '上传' : '下载'}继续`, {
      name: t.name, pct: Math.floor(t.pct),
    })
  } else {
    reportLog(t.groupId ? 'debug' : 'info', SFTP_TRANSFER_START, `${t.dir === 'up' ? '上传' : '下载'}开始`, {
      name: t.name, size: t.size, resume_from: 0,
    })
    // 文件夹子文件不逐条弹开始提示，避免大量文件刷屏
    if (!t.groupId) toast(`开始${t.dir === 'up' ? '上传' : '下载'}：${taskName(t)}`, 'info', 1700)
  }

  const failTask = (msg: string) => {
    t.speed = 0
    t.error = msg
    const dirText = t.dir === 'up' ? '上传' : '下载'
    t.status = 'error'
    reportLog('error', SFTP_TRANSFER_FAILED, `${dirText}失败`, {
      name: t.name, err: msg, transferred: Math.round(t.size * t.pct / 100), total: t.size,
    })
    if (!t.groupId) toast(`${dirText}失败：${taskName(t)}`, 'warn', 2200)
    touchGroup(t)
    schedulePump()
  }
  const finishTask = () => {
    t.status = 'success'
    // 覆盖策略跳过：目标已存在且无需覆盖。未发生实际传输，进度保持 0、不做校验；
    // 同时不得删除剪切源文件，也不刷新对侧目录
    if (t.skipped) {
      t.pct = 0
      t.speed = 0
      const dirText = t.dir === 'up' ? '上传' : '下载'
      reportLog('info', SFTP_TRANSFER_COMPLETE, `${dirText}已跳过（目标已存在）`, {
        name: t.name, size: t.size, skipped: true,
      })
      if (!t.groupId) toast(`已跳过：${taskName(t)}`, 'info', 1800)
      touchGroup(t)
      schedulePump()
      return
    }
    t.pct = 100
    // 清零前捕获最后一帧速率（后端按滑动窗口计算的瞬时速率）
    const finalSpeedBps = t.speed
    t.speed = 0
    reportLog(t.groupId ? 'debug' : 'info', SFTP_TRANSFER_COMPLETE, `${t.dir === 'up' ? '上传' : '下载'}完成`, {
      name: t.name, size: t.size,
      elapsed_ms: t.startAt ? Date.now() - t.startAt : 0,
      speed_bps: finalSpeedBps, verify: savedSettings.sftpVerifyHash ? 'pass' : 'none',
    })
    if (!t.groupId) toast(`${t.dir === 'up' ? '上传' : '下载'}完成：${taskName(t)}`, 'ok', 2000)
    // 跨侧剪切粘贴：传输成功后删除源文件（失败仅记日志，目标已完整）
    if (t.cutSrc) {
      if (t.dir === 'up') {
        void invoke('local_remove', { path: t.cutSrc })
          .catch(e => reportLog('error', SFTP_MANAGE_REMOVE, '剪切源删除失败', { path: t.cutSrc, err: String(e) }))
      } else {
        void invoke('sftp_remove', { sessionId: taskBackendId, path: t.cutSrc })
          .catch(e => reportLog('error', SFTP_MANAGE_REMOVE, '剪切源删除失败', { path: t.cutSrc, err: String(e) }))
      }
    }
    // 传输完成自动刷新对侧目录，免去手动点刷新：上传→远端，下载→本地。
    // 文件夹子文件由汇总行在整组完成时统一刷新，避免每个文件都刷新一次。
    if (!t.groupId) {
      if (t.dir === 'up') void loadRemote(remotePath.value)
      else void loadLocal(localPath.value)
    }
    touchGroup(t)
    schedulePump()
  }

  if (!taskBackendId) { failTask('未建立 SSH 连接'); return }
  if (!t.localPath || !t.remotePath) { failTask('传输路径缺失'); return }
  const cmd = t.dir === 'up' ? 'sftp_upload' : 'sftp_download'
  // 每个任务独立 Channel：后端流式推送进度，前端 onmessage 按 taskId 更新对应任务
  const channel = new Channel<TransferProgress>()
  channel.onmessage = (p) => {
    if (p.total > 0) t.size = p.total
    // 跳过包不带实际传输字节：保持 finishTask 置的 100%，避免迟到包把进度拉回 0
    if (p.skipped) t.skipped = true
    else t.pct = p.total > 0 ? (p.transferred / p.total) * 100 : (p.done ? 100 : t.pct)
    // 字节已传完、正在比对哈希：清空速率并让状态标签切到"校验中"
    if (p.verifying) {
      t.verifying = true
      t.speed = 0
    } else {
      t.speed = p.speed
    }
    touchGroup(t)
  }
  // 单次传输选项：从全局设置组装（camelCase 与后端 SftpTransferOptions 对齐）；
  // ask 策略下用户确认覆盖后，该任务后续重跑固定按 overwrite 执行
  const options = {
    overwritePolicy: t.forceOverwrite ? 'overwrite' : savedSettings.sftpOverwritePolicy,
    preserveMeta: savedSettings.sftpPreserveMeta,
    resumeCheck: savedSettings.sftpResumeCheck,
    taskRateKb,
    verifyHash: savedSettings.sftpVerifyHash,
  }
  const args = t.dir === 'up'
    ? { sessionId: taskBackendId, taskId: t.id, localPath: t.localPath, remotePath: t.remotePath, chunkKb: savedSettings.sftpChunkKb, resume: savedSettings.sftpResume, options, channel }
    : { sessionId: taskBackendId, taskId: t.id, remotePath: t.remotePath, localPath: t.localPath, chunkKb: savedSettings.sftpChunkKb, resume: savedSettings.sftpResume, options, channel }
  try {
    await invoke(cmd, args)
    finishTask()
  } catch (e) {
    const msg = String(e)
    if (msg.includes('NEED_CONFIRM')) {
      // ask 策略：目标已存在，由用户决定覆盖或跳过（确认后该任务固定按覆盖重跑）
      const dirText = t.dir === 'up' ? '上传' : '下载'
      const ok = await confirmDialog(
        `目标位置已存在同名文件，是否覆盖？\n${taskName(t)}`,
        `${dirText}文件已存在`,
        { confirmText: '覆盖', cancelText: '跳过' },
      )
      if (!transfers.value.some(x => x.id === t.id)) return
      if (ok) {
        t.forceOverwrite = true
        t.pct = 0
        t.skipped = false
        void runTask(t)
      } else {
        t.skipped = true
        finishTask()
      }
      return
    }
    // 用户主动取消：保留队列记录并标记"已取消"，不弹失败 toast
    if (msg.includes('已取消') || msg.includes('Cancelled')) {
      if ((t.status as TransferStatus) !== 'cancelled') {
        t.status = 'cancelled'
        t.speed = 0
        t.error = undefined
        reportLog('warn', SFTP_TRANSFER_CANCEL, `${t.dir === 'up' ? '上传' : '下载'}已取消`, {
          name: t.name, transferred: Math.round(t.size * t.pct / 100), total: t.size,
        })
      }
      touchGroup(t)
      schedulePump()
    } else {
      failTask(msg)
    }
  }
}

/** 取消传输：运行中/已暂停通知后端取消（状态由 runTask 的 catch 统一落为"已取消"），
 *  尚未启动的直接标记；队列保留记录，可查看进度位置后再移除。
 *  文件夹任务级联取消全部子文件；扫描阶段取消则中止扫描。 */
function cancelTask(t: Transfer) {
  if (sftpLocked.value) return
  if (t.kind === 'dir') {
    const members = groupMembers(t)
    members.forEach(m => cancelTask(m))
    if (!members.length) {
      t.status = 'cancelled'
      t.scanning = false
      t.speed = 0
    } else {
      syncGroup(t)
    }
    return
  }
  // 已失败的任务取消时直接落为已取消
  if (t.status === 'error') {
    t.status = 'cancelled'
    t.speed = 0
    touchGroup(t)
    schedulePump()
    return
  }
  if (t.status === 'running' || t.status === 'paused') {
    const backendId = t.backendId || activeSession.value?.backendId
    if (backendId) {
      void invoke('sftp_transfer_cancel', { sessionId: backendId, taskId: t.id }).catch(() => {})
    }
    return
  }
  if (t.status !== 'pending') return
  t.status = 'cancelled'
  t.speed = 0
  reportLog('warn', SFTP_TRANSFER_CANCEL, `${t.dir === 'up' ? '上传' : '下载'}已取消`, {
    name: t.name, pct: Math.floor(t.pct),
  })
  touchGroup(t)
  schedulePump()
}

/** 移除任务记录：进行中的任务先通知后端取消，再从队列删除并启动下一个 */
function removeTask(t: Transfer) {
  if (sftpLocked.value) return
  // 文件夹任务：级联移除全部子文件任务后删除汇总行（扫描中则中止扫描）
  if (t.kind === 'dir') {
    groupMembers(t).forEach(m => removeTask(m))
    transfers.value = transfers.value.filter(x => x.id !== t.id)
    return
  }
  // 进行中或已暂停的任务都要通知后端取消：后端在取消路径关闭句柄并删除 part
  const active = t.status === 'running' || t.status === 'paused'
  if (active) {
    const backendId = t.backendId || activeSession.value?.backendId
    if (backendId) {
      void invoke('sftp_transfer_cancel', { sessionId: backendId, taskId: t.id }).catch(() => {})
    }
  }
  transfers.value = transfers.value.filter(x => x.id !== t.id)
  reportLog('warn', SFTP_TRANSFER_CANCEL, `${t.dir === 'up' ? '上传' : '下载'}${active ? '已取消' : '记录移除'}`, {
    name: t.name, transferred: Math.round(t.size * t.pct / 100), total: t.size,
  })
  touchGroup(t)
  if (active) schedulePump()
}

/** 暂停任务：通知后端暂停开关，invoke 仍挂起；running→paused 释放一个并发槽位。
 *  文件夹任务级联暂停全部子文件。 */
function pauseTask(t: Transfer) {
  if (sftpLocked.value) return
  if (t.kind === 'dir') {
    const members = groupMembers(t)
    // 扫描 / 建目录阶段尚无子任务：直接挂起汇总行，扫描完成后子任务以暂停态入队
    if (!members.length) {
      if (t.scanning && (t.status === 'pending' || t.status === 'running')) t.status = 'paused'
      return
    }
    members.forEach(m => pauseTask(m))
    syncGroup(t)
    return
  }
  const backendId = activeSession.value?.backendId
  if (t.status === 'running') {
    if (backendId) {
      void invoke('sftp_transfer_pause', { sessionId: backendId, taskId: t.id, paused: true }).catch(() => {})
    }
    t.status = 'paused'
    t.speed = 0
    reportLog('warn', SFTP_TRANSFER_PAUSE, '已暂停', { name: t.name, pct: Math.floor(t.pct) })
    touchGroup(t)
    schedulePump() // 释放槽位，启动下一个排队任务
  } else if (t.status === 'pending') {
    t.status = 'paused'
    reportLog('warn', SFTP_TRANSFER_PAUSE, '排队任务已挂起', { name: t.name, pct: 0 })
    touchGroup(t)
  }
}
/** 继续任务：通知后端恢复并直接回到 running（invoke 仍在）。
 *  quiet 用于组级继续：并发配额不足时静默跳过，避免逐个文件弹提示 */
function resumeTask(t: Transfer, quiet = false) {
  if (sftpLocked.value) return
  if (t.kind === 'dir') {
    if (t.status !== 'paused') return
    const members = groupMembers(t)
    // 扫描阶段挂起的汇总行：恢复为排队，等待扫描完成后再入队文件任务
    if (!members.length && t.scanning) { t.status = 'pending'; return }
    members.filter(m => m.status === 'paused').forEach(m => resumeTask(m, true))
    syncGroup(t)
    return
  }
  if (t.status !== 'paused') return
  const bid = t.backendId || activeSession.value?.backendId || ''
  // 恢复前校验并发配额，避免暂停期间补位的任务导致超过上限
  if (!hasCapacity(bid)) {
    if (!quiet) toast('并发任务已达上限，请先暂停其他任务', 'warn', 2000)
    return
  }
  if (bid) {
    void invoke('sftp_transfer_pause', { sessionId: bid, taskId: t.id, paused: false }).catch(() => {})
  }
  t.status = 'running'
  // 与暂停（WARN）配对的显式用户动作，INFO 保证默认级别下可见
  reportLog('info', SFTP_TRANSFER_RESUME, `${t.dir === 'up' ? '上传' : '下载'}继续`, {
    name: t.name, pct: Math.floor(t.pct),
  })
  touchGroup(t)
}
/** 失败/已取消任务重试：进度归零、清错误后重新入队。
 *  文件夹任务仅重跑失败/已取消的文件，已成功的自动跳过。 */
function retryTask(t: Transfer) {
  if (sftpLocked.value) return
  if (t.kind === 'dir') {
    if (t.status !== 'error' && t.status !== 'cancelled') return
    const members = groupMembers(t)
    // 扫描/建目录阶段失败：重新准备整个文件夹
    if (!members.length) { retryFolderScan(t); return }
    members.forEach(m => {
      if (m.status === 'error' || m.status === 'cancelled') retryTask(m)
    })
    syncGroup(t)
    return
  }
  if (t.status !== 'error' && t.status !== 'cancelled') return
  t.pct = 0
  t.speed = 0
  t.error = undefined
  t.status = 'pending'
  reportLog('info', SFTP_TRANSFER_ENQUEUE, `${t.dir === 'up' ? '上传' : '下载'}重新加入队列`, {
    name: t.name, size: t.size, dir: t.dir, from: t.localPath, to: t.remotePath,
  })
  touchGroup(t)
  schedulePump()
}

// 解锁后续跑冻结期间滞留的排队任务（进行中的任务从未中断，无需恢复动作）
watch(sftpLocked, locked => {
  // 锁定瞬间关闭已打开的右键菜单：菜单 Teleport 到 body 且 z-index:9999，
  // 会越过两级遮罩残留可点
  if (locked) menu.value = null
  else schedulePump()
})
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
  transfers.value.forEach(t => { if (t.status === 'paused') resumeTask(t, true) })
}

/** 清空终态记录（已完成/已取消；进行中、排队、暂停、失败保留）。
 *  文件夹任务连同其子文件任务一并清理 */
function clearDoneTasks() {
  const deadGroups = new Set(
    transfers.value
      .filter(t => t.kind === 'dir' && (t.status === 'success' || t.status === 'cancelled'))
      .map(t => t.id),
  )
  transfers.value = transfers.value.filter(t => {
    // 子文件任务跟随所属文件夹任务：仅当整组被清理时一并移除，
    // 避免失败组内已成功文件被清空导致进度与重试统计失真
    if (t.groupId !== undefined) return !deadGroups.has(t.groupId)
    return t.status !== 'success' && t.status !== 'cancelled'
  })
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

/** 传输任务状态标签文案（队列任务名后展示） */
const TASK_TAG_TEXT: Record<TransferStatus, string> = {
  pending: '排队中',
  running: '传输中',
  paused: '已暂停',
  error: '失败',
  success: '已完成',
  cancelled: '已取消',
}

/** 任务状态标签：覆盖策略跳过的任务虽为 success，但语义上应显示"已跳过"；
 *  字节传完但还在比对哈希时显示"校验中"；完成态（文件与目录）不再重复"完成"，
 *  由进度 100%、校验结果与文件计数表达 */
/* 目录任务失败口径：有成功项时为「部分失败」，全部失败时为「失败」；
   两者沿用相同的红色标签，仅文案区分 */
function folderFailText(t: Transfer): string {
  const members = groupMembers(t)
  const failed = members.filter(c => c.status === 'error').length
  const ok = members.filter(c => c.status === 'success').length
  return failed > 0 && ok > 0 ? '部分失败' : '失败'
}
function taskStatusText(t: Transfer): string {
  if (t.kind === 'dir' && t.scanning) return '扫描中'
  if (t.skipped) return '已跳过'
  if (t.kind === 'dir' && t.status === 'error') return folderFailText(t)
  if (t.status === 'success') return ''
  if (t.status === 'running' && t.verifying) return '校验中'
  return TASK_TAG_TEXT[t.status]
}

/** 将一个选中条目入队：文件直接传输，目录递归展开为文件级任务并归入文件夹汇总行。
 *  targetDir 为对侧目标目录覆盖（拖拽落到目录行时使用），缺省为对侧当前目录 */
function enqueueEntry(dir: 'up' | 'down', e: FsEntry, targetDir?: string) {
  const localAbs = joinPath(localPath.value, e.name)
  const remoteAbs = joinPath(remotePath.value, e.name)
  const srcRoot = dir === 'up' ? localAbs : remoteAbs
  const dstBase = targetDir ?? (dir === 'up' ? remotePath.value : localPath.value)
  const dstRoot = joinPath(dstBase, e.name)
  if (e.type === 'dir') {
    startFolderTask(dir, e.name, srcRoot, dstRoot)
    return
  }
  startTask(dir, e.name, e.size, {
    localPath: dir === 'up' ? localAbs : dstRoot,
    remotePath: dir === 'up' ? dstRoot : remoteAbs,
  })
}

/** 文件夹传输上限：防御异常目录导致内存/队列失控 */
const FOLDER_FILE_LIMIT = 10_000
const FOLDER_BYTES_LIMIT = 100 * 1024 ** 3

/** 文件夹传输入队：先建汇总行，再异步扫描源目录、创建目标目录、
 *  把每个文件作为独立任务入队（支持单文件级的暂停/继续/重试/取消） */
function startFolderTask(
  dir: 'up' | 'down', name: string, srcRoot: string, dstRoot: string, cutSrc?: string,
) {
  if (sftpLocked.value) {
    toast('会话已锁定，请先解锁', 'warn')
    return
  }
  const backendId = activeSession.value?.backendId || ''
  if (!backendId) { toast('请先建立 SSH 连接', 'warn'); return }
  const wasIdle = transfers.value.length === 0
  const g: Transfer = {
    id: ++seq, name, dir, kind: 'dir', size: 0, pct: 0, status: 'pending',
    speed: 0, startAt: 0,
    localPath: dir === 'up' ? srcRoot : dstRoot,
    remotePath: dir === 'up' ? dstRoot : srcRoot,
    backendId, expanded: true, scanning: true, fileCount: 0, cutSrc,
  }
  transfers.value.push(g)
  if (wasIdle) sftpQueueCollapsed.value = false
  reportLog('info', SFTP_TRANSFER_ENQUEUE, `${dir === 'up' ? '上传' : '下载'}文件夹加入队列`, {
    name, dir, from: srcRoot, to: dstRoot,
  })
  void prepareFolder(g)
}

/** 文件夹任务的源 / 目标根目录（localPath/remotePath 在 up/down 下语义相反） */
function folderRoots(g: Transfer) {
  return g.dir === 'up'
    ? { src: g.localPath || '', dst: g.remotePath || '' }
    : { src: g.remotePath || '', dst: g.localPath || '' }
}

/** 重新准备（扫描 + 建目录）准备阶段失败的文件夹任务 */
function retryFolderScan(g: Transfer) {
  if (g.scanning) return
  g.scanning = true
  g.status = 'pending'
  g.pct = 0
  g.speed = 0
  g.error = undefined
  void prepareFolder(g)
}

/**
 * 文件夹任务准备流程：迭代遍历源目录 → 收集文件与子目录 →
 * 在目标侧创建目录 → 为每个文件创建独立子任务并入队。
 * 默认不跟随符号链接；文件数 / 总大小超限时中止整组。
 */
async function prepareFolder(g: Transfer) {
  const { src, dst } = folderRoots(g)
  const alive = () => transfers.value.some(x => x.id === g.id) && g.status !== 'cancelled'
  try {
    if (!src) throw new Error('传输路径缺失')
    const files: { rel: string; size: number }[] = []
    const dirs: string[] = []
    const queue: { src: string; rel: string }[] = [{ src, rel: '' }]
    const visited = new Set<string>()
    let total = 0
    while (queue.length) {
      if (!alive()) return
      if (sftpLocked.value) throw new Error('会话已锁定，文件夹传输已中止')
      const cur = queue.shift()!
      // 循环检测：同一真实路径只扫描一次（软链默认跳过，此处为额外兜底）
      if (visited.has(cur.src)) continue
      visited.add(cur.src)
      const res = g.dir === 'up'
        ? await invoke<DirListing>('list_local_dir', { path: cur.src })
        : await invoke<DirListing>('sftp_list_dir', { sessionId: g.backendId, path: cur.src })
      for (const ent of res.entries) {
        if (ent.isSymlink) continue // 默认不跟随符号链接
        const rel = cur.rel ? `${cur.rel}/${ent.name}` : ent.name
        if (ent.isDir) {
          dirs.push(rel)
          queue.push({ src: joinPath(cur.src, ent.name), rel })
          continue
        }
        if (g.dir === 'up' && isBlacklisted(ent.name)) continue
        files.push({ rel, size: ent.size })
        total += ent.size
        if (files.length > FOLDER_FILE_LIMIT) {
          throw new Error(`文件数超过 ${FOLDER_FILE_LIMIT} 上限，已中止`)
        }
        if (total > FOLDER_BYTES_LIMIT) {
          throw new Error('文件夹总大小超过 100 GB 上限，已中止')
        }
      }
    }
    if (!alive()) return

    // 目标目录：根目录 + 全部子目录，父目录先创建
    const ordered = ['', ...dirs].sort((a, b) => a.split('/').length - b.split('/').length)
    for (const rel of ordered) {
      if (!alive()) return
      if (sftpLocked.value) throw new Error('会话已锁定，文件夹传输已中止')
      const p = rel ? joinPath(dst, rel) : dst
      try {
        if (g.dir === 'up') await invoke('sftp_mkdir', { sessionId: g.backendId, path: p })
        else await invoke('mkdir', { path: p })
      } catch (e) {
        // 目标目录已存在属正常情况（SFTP 无法区分存在与权限错误，
        // 统一放行，真正失败会在文件传输阶段以明确错误暴露）
        const msg = String(e)
        if (g.dir === 'down' && !/exist|已存在/i.test(msg)) throw e
      }
    }
    if (!alive()) return

    g.fileCount = files.length
    g.size = total
    g.scanning = false
    // 文件过多时默认收起，避免一次性渲染大量子项行
    g.expanded = files.length <= 200
    const paused = g.status === 'paused' // 扫描期间被暂停：新任务直接置为已暂停
    for (const f of files) {
      transfers.value.push({
        id: ++seq,
        name: f.rel,
        dir: g.dir,
        size: f.size,
        pct: 0,
        speed: 0,
        startAt: 0,
        status: paused ? 'paused' : 'pending',
        localPath: g.dir === 'up' ? joinPath(src, f.rel) : joinPath(dst, f.rel),
        remotePath: g.dir === 'up' ? joinPath(dst, f.rel) : joinPath(src, f.rel),
        backendId: g.backendId,
        groupId: g.id,
      })
    }
    if (!files.length) {
      // 仅空目录：创建完目录即完成
      g.status = 'success'
      g.pct = 100
      onFolderDone(g)
      return
    }
    syncGroup(g)
    schedulePump()
  } catch (e) {
    if (!alive()) return
    g.scanning = false
    g.status = 'error'
    g.error = String((e as Error)?.message ?? e)
    reportLog('error', SFTP_TRANSFER_FAILED, '文件夹传输准备失败', { name: g.name, err: g.error })
    toast(`文件夹${g.dir === 'up' ? '上传' : '下载'}失败：${g.name}`, 'warn', 2600)
  }
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
    /* 跨侧：本质下载/上传，入现有传输队列；目录递归展开为文件级任务 */
    const dir: 'up' | 'down' = side === 'remote' ? 'up' : 'down'
    for (const j of jobs) {
      const localAbs = dir === 'up' ? j.src : j.dst
      const remoteAbs = dir === 'up' ? j.dst : j.src
      if (j.isDir) {
        startFolderTask(dir, j.name, j.src, j.dst, cb.mode === 'cut' ? j.src : undefined)
        continue
      }
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
  if (sftpLocked.value) return
  lastSide.value = side
  const sel = sideSel(side)
  // 右键未选中项 → 仅选该项；右键已选中项 → 保持多选
  if (!sel.value.has(entry.name)) sel.value = new Set([entry.name])
  menu.value = { x: e.clientX, y: e.clientY, side, targets: selectedEntries(side) }
}
function onBlankContext(e: MouseEvent, side: Side) {
  if (sftpLocked.value) return
  lastSide.value = side
  sideSel(side).value = new Set()
  menu.value = { x: e.clientX, y: e.clientY, side, targets: [] }
}
/** 列表横向滚动时同步表头偏移，避免表头参与滚动区导致可滚范围异常 */
function onListScroll(e: Event) {
  const list = e.currentTarget as HTMLElement
  const header = list.closest('.pane')?.querySelector<HTMLElement>('.list-header')
  if (header) header.style.transform = `translateX(${-list.scrollLeft}px)`
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
  if (!m) return []
  return m.pane ? buildPaneMenu(m.side) : buildMenu(m.side, m.targets)
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
  back: 'M15 18l-6-6 6-6',
  forward: 'M9 6l6 6-6 6',
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
      { divider: true },
      {
        label: '显示隐藏文件',
        checked: savedSettings.sftpShowHidden,
        action: () => {
          savedSettings.sftpShowHidden = !savedSettings.sftpShowHidden
          persistSettings()
        },
      },
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
    icon: ICON.del, shortcut: 'Del',
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

/** 每栏「更多」菜单：导航 → 传输 → 编辑 → 新建 → 显示隐藏文件 */
function buildPaneMenu(side: Side): MenuItem[] {
  const entries = selectedEntries(side)
  const single = entries.length === 1 ? entries[0]! : null
  const remoteBlocked = side === 'remote' && (!connected.value || sftpLocked.value)
  return [
    {
      label: '返回', icon: ICON.back,
      disabled: !canGoBack(side),
      action: () => void goBack(side),
    },
    {
      label: '前进', icon: ICON.forward,
      disabled: !canGoForward(side),
      action: () => void goForward(side),
    },
    { divider: true },
    {
      label: '下载',
      icon: ICON.down,
      disabled: side !== 'remote' || !entries.length || !connected.value || sftpLocked.value,
      action: () => onDownload(),
    },
    {
      label: '上传',
      icon: ICON.up,
      disabled: side !== 'local' || !entries.length || !connected.value || sftpLocked.value,
      action: () => onUpload(),
    },
    { divider: true },
    {
      label: entries.length > 1 ? `删除 ${entries.length} 项` : '删除',
      icon: ICON.del,
      disabled: !entries.length || remoteBlocked,
      action: () => void deleteTargets(side, entries),
    },
    {
      label: '重命名',
      icon: ICON.rename,
      disabled: !single || remoteBlocked,
      action: () => { if (single) startEdit(side, 'rename', single) },
    },
    { divider: true },
    {
      label: '新建文件',
      icon: ICON.mkfile,
      disabled: remoteBlocked,
      action: () => startEdit(side, 'mkfile'),
    },
    {
      label: '新建文件夹',
      icon: ICON.mkdir,
      disabled: remoteBlocked,
      action: () => startEdit(side, 'mkdir'),
    },
    { divider: true },
    {
      label: '显示隐藏文件',
      checked: savedSettings.sftpShowHidden,
      action: () => {
        savedSettings.sftpShowHidden = !savedSettings.sftpShowHidden
        persistSettings()
      },
    },
  ]
}

/** 打开每栏工具行的「更多」菜单（锚定按钮左下角） */
function openPaneMenu(side: Side, e: MouseEvent) {
  const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
  menu.value = { x: Math.round(r.left), y: Math.round(r.bottom + 4), side, targets: [], pane: true }
}

/* 行交互：单击选择（Ctrl/⌘ 多选）、双击目录进入、双击文件传输 */
function onRowClick(side: Side, entry: FsEntry, e: MouseEvent) {
  if (pointerMoved) return // 释放动作补发的 click：不改写选区
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
  if (side === 'remote' && sftpLocked.value) return // 锁定期：禁导航/禁双击下载
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

/* ---- 跨侧拖拽传输（指针事件自绘，三端行为一致）----
   本地行拖到远端列表 = 上传，远端行拖到本地列表 = 下载；
   拖拽项为当前选中集合（拖未选中的行则先选中该行）；
   落点缺省为对侧当前目录，拖到对侧目录行上则直接传入该目录。 */
const dragSide = ref<Side | null>(null)
/** 当前高亮的落点侧（恒为拖拽源的对侧）与落点目录 */
const dropSide = ref<Side | null>(null)
const dropDir = ref<string | null>(null)

/** 已按下但未越过阈值的待定拖拽 */
let pendingDrag: { side: Side; entry: FsEntry; x: number; y: number } | null = null
/** 拖拽中指针位置与跟手信息条文案 */
const dragPos = reactive({ x: 0, y: 0 })
const dragLabel = ref('')
const DRAG_THRESHOLD = 5
/** 行内可起文件拖拽的热区：图标、文件名、链接徽标、各列文字。
 *  其余位置（列内空白、名称列空白）都视为空白，交给框选。 */
const ROW_DRAG_HOT = '.row-icon, .row-text, .link-badge, .cell-text'

/** 本轮按下（pointerdown → click）是否发生过框选或拖拽。
 *  释放时浏览器会补发一次 click，用它把这次 click 从选中逻辑中排除，
 *  避免刚框选完就被清空、拖拽后选区被改写；下一次 pointerdown 复位。 */
let pointerMoved = false

/* ---- 空白区域框选：在列表空白或行的非名称区域按下拖动，选中覆盖的文件 ---- */
const marquee = ref<{
  side: Side
  x1: number
  y1: number
  x2: number
  y2: number
  additive: boolean
  moved: boolean
} | null>(null)
let marqueeBase: Set<string> = new Set()

/** 点击空白（列表空白或行内非文字区域）取消该侧选中 */
function onPaneClick(side: Side, e: MouseEvent) {
  if (pointerMoved) return // 释放动作补发的 click：不清空刚框选/刚拖拽的选区
  const t = e.target as HTMLElement
  if (t.closest('input') || t.closest(ROW_DRAG_HOT)) return
  lastSide.value = side
  sideSel(side).value = new Set()
}

function onPanePointerDown(e: PointerEvent, side: Side) {
  if (e.button !== 0) return
  pointerMoved = false // 新一轮交互开始
  const el = e.currentTarget as HTMLElement
  const t = e.target as HTMLElement
  // 只有文字/图标热区参与行拖拽与点选；列内空白与名称列空白按空白处理，
  // 这样列表被文件铺满时仍能在任意列的空白处起框选
  if (t.closest('input') || t.closest(ROW_DRAG_HOT)) return
  // 排除非 overlay 滚动条区域，避免拖滚动条时误触发框选
  const rect = el.getBoundingClientRect()
  if (e.clientX > rect.left + el.clientWidth || e.clientY > rect.top + el.clientHeight) return
  lastSide.value = side
  marqueeBase = new Set(sideSel(side).value)
  marquee.value = {
    side,
    x1: e.clientX,
    y1: e.clientY,
    x2: e.clientX,
    y2: e.clientY,
    additive: e.ctrlKey || e.metaKey || e.shiftKey,
    moved: false,
  }
  window.addEventListener('pointermove', onMarqueeMove)
  window.addEventListener('pointerup', onMarqueeUp)
  window.addEventListener('pointercancel', onMarqueeUp)
}

function marqueeBox() {
  const m = marquee.value
  if (!m) return { left: 0, top: 0, width: 0, height: 0 }
  let left = Math.min(m.x1, m.x2)
  let top = Math.min(m.y1, m.y2)
  let right = Math.max(m.x1, m.x2)
  let bottom = Math.max(m.y1, m.y2)
  // 夹取到列表可视区：矩形不覆盖表头/工具栏，也不会选中被滚动裁掉的行
  const list = document.querySelector<HTMLElement>(`.pane.${m.side} .pane-list`)
  if (list) {
    const r = list.getBoundingClientRect()
    left = Math.max(left, r.left)
    top = Math.max(top, r.top)
    right = Math.min(right, r.right)
    bottom = Math.min(bottom, r.bottom)
  }
  return { left, top, width: Math.max(0, right - left), height: Math.max(0, bottom - top) }
}

/** 按矩形与行元素的实际矩形求交，实时刷新该侧选择集 */
function applyMarquee() {
  const m = marquee.value
  if (!m) return
  const list = document.querySelector<HTMLElement>(`.pane.${m.side} .pane-list`)
  if (!list) return
  const box = marqueeBox()
  const right = box.left + box.width
  const bottom = box.top + box.height
  const hit = new Set<string>(m.additive ? marqueeBase : [])
  for (const row of list.querySelectorAll<HTMLElement>('.row')) {
    const b = row.getBoundingClientRect()
    if (b.left < right && b.right > box.left && b.top < bottom && b.bottom > box.top) {
      const name = row.dataset.name
      if (name) hit.add(name)
    }
  }
  sideSel(m.side).value = hit
}

function onMarqueeMove(e: PointerEvent) {
  const m = marquee.value
  if (!m) return
  const moved = m.moved || Math.abs(e.clientX - m.x1) + Math.abs(e.clientY - m.y1) > 3
  marquee.value = { ...m, x2: e.clientX, y2: e.clientY, moved }
  if (moved) {
    pointerMoved = true
    applyMarquee()
  }
}

function onMarqueeUp() {
  marquee.value = null
  window.removeEventListener('pointermove', onMarqueeMove)
  window.removeEventListener('pointerup', onMarqueeUp)
  window.removeEventListener('pointercancel', onMarqueeUp)
}

/* 边缘自动滚动：指针停靠在列表上下边缘时持续滚动，便于拖到视野外的目录 */
let autoScrollList: HTMLElement | null = null
let autoScrollRaf = 0
const AUTO_SCROLL_EDGE = 28
const AUTO_SCROLL_MAX = 12

function scheduleAutoScroll() {
  if (!autoScrollRaf && dragSide.value) autoScrollRaf = requestAnimationFrame(autoScrollStep)
}

function autoScrollStep() {
  autoScrollRaf = 0
  if (!dragSide.value) { autoScrollList = null; return }
  const list = autoScrollList
  if (list) {
    const r = list.getBoundingClientRect()
    let dy = 0
    if (dragPos.y < r.top + AUTO_SCROLL_EDGE) {
      dy = -Math.ceil(((r.top + AUTO_SCROLL_EDGE - dragPos.y) / AUTO_SCROLL_EDGE) * AUTO_SCROLL_MAX)
    } else if (dragPos.y > r.bottom - AUTO_SCROLL_EDGE) {
      dy = Math.ceil(((dragPos.y - (r.bottom - AUTO_SCROLL_EDGE)) / AUTO_SCROLL_EDGE) * AUTO_SCROLL_MAX)
    }
    if (dy) {
      const before = list.scrollTop
      list.scrollTop += dy
      // 内容滚动后指针下的行可能变化，重算落点
      if (list.scrollTop !== before) updateDropTarget(dragPos.x, dragPos.y)
    }
  }
  scheduleAutoScroll()
}

function stopAutoScroll() {
  if (autoScrollRaf) cancelAnimationFrame(autoScrollRaf)
  autoScrollRaf = 0
  autoScrollList = null
}

function clearDropTarget() {
  dropSide.value = null
  dropDir.value = null
}

function onRowPointerDown(side: Side, entry: FsEntry, e: PointerEvent) {
  if (e.button !== 0 || sftpLocked.value) return
  pointerMoved = false // 新一轮交互开始
  const t = e.target as HTMLElement
  if (t.closest('input')) return // 行内重命名输入框不触发拖拽
  // 只有按在图标/文件名/列内文字上才起拖；列内空白留给框选
  if (!t.closest(ROW_DRAG_HOT)) return
  pendingDrag = { side, entry, x: e.clientX, y: e.clientY }
}

function onRowPointerMove(e: PointerEvent) {
  if (pendingDrag) {
    const dx = e.clientX - pendingDrag.x
    const dy = e.clientY - pendingDrag.y
    if (dx * dx + dy * dy < DRAG_THRESHOLD * DRAG_THRESHOLD) return
    startInternalDrag(e)
  }
  if (!dragSide.value) return
  dragPos.x = e.clientX
  dragPos.y = e.clientY
  updateDropTarget(e.clientX, e.clientY)
}

function onRowPointerUp() {
  if (pendingDrag) { pendingDrag = null; return } // 未越阈值：仍按普通点击处理
  finishInternalDrag(true)
}

function onRowPointerCancel() {
  pendingDrag = null
  if (dragSide.value) finishInternalDrag(false)
}

function onDragKeydown(e: KeyboardEvent) {
  if (e.key !== 'Escape' || !dragSide.value) return
  e.preventDefault()
  finishInternalDrag(false)
}

function onDragBlur() {
  if (dragSide.value || pendingDrag) finishInternalDrag(false)
}

function startInternalDrag(e: PointerEvent) {
  const p = pendingDrag
  if (!p) return
  pendingDrag = null
  const sel = sideSel(p.side)
  if (!sel.value.has(p.entry.name)) sel.value = new Set([p.entry.name])
  pointerMoved = true // 拖拽已启动：松手补发的 click 不改写选区
  dragSide.value = p.side
  dragLabel.value = sel.value.size > 1 ? `${p.entry.name} 等 ${sel.value.size} 项` : p.entry.name
  dragPos.x = e.clientX
  dragPos.y = e.clientY
  document.body.classList.add('row-dragging')
  clearDropTarget()
  scheduleAutoScroll()
}

/** 指针命中：对侧列表内为落点，命中目录行则传入该目录 */
function updateDropTarget(x: number, y: number) {
  const src = dragSide.value
  if (!src) return
  const el = document.elementFromPoint(x, y) as HTMLElement | null
  const list = (el?.closest('.pane-list') as HTMLElement | null) ?? null
  autoScrollList = list
  const pane = el?.closest('.pane.local, .pane.remote') as HTMLElement | null
  const side: Side | null = pane?.classList.contains('remote') ? 'remote'
    : pane?.classList.contains('local') ? 'local' : null
  if (!el || !list || !side || side === src
      || (side === 'remote' && !connected.value)) {
    if (dropSide.value) clearDropTarget()
    return
  }
  const name = (el.closest('.row.dir') as HTMLElement | null)?.dataset.name
  dropSide.value = side
  dropDir.value = name ? joinPath(sidePath(side), name) : sidePath(side)
}

function finishInternalDrag(commit: boolean) {
  const src = dragSide.value
  pendingDrag = null
  dragSide.value = null
  stopAutoScroll()
  dragPos.x = 0
  dragPos.y = 0
  document.body.classList.remove('row-dragging')
  if (!src) { clearDropTarget(); return }
  const target = commit ? dropSide.value : null
  const targetDir = dropDir.value
  clearDropTarget()
  if (!target || !targetDir) return
  if (target === 'remote' && (!connected.value || sftpLocked.value)) {
    toast(sftpLocked.value ? '会话已锁定，请先解锁' : '请先建立 SSH 连接', 'warn')
    return
  }
  const names = new Set(sideSel(src).value)
  const entries = sideRaw(src).value.filter(x => names.has(x.name))
  if (!entries.length) return
  entries.forEach(en => enqueueEntry(src === 'local' ? 'up' : 'down', en, targetDir))
  sideSel(src).value = new Set()
}

/* ---- 外部文件拖入（原生拖放，三端统一）：拖到远端栏即上传到落点目录 ----
   原生事件只给坐标与本地绝对路径，落点靠命中测试确定。 */
let unlistenExternalDrop: UnlistenFn | null = null

/** 物理坐标命中远端栏：返回落点目录（目录行则进入该目录），否则 null。
 *  坐标单位在平台间不一致（macOS WKWebView 为逻辑点，Windows 为物理像素），
 *  先按逻辑点命中，未落在远端栏时再按 devicePixelRatio 换算兜底。 */
function externalDropTargetAt(pos: { x: number; y: number }): string | null {
  if (!connected.value || sftpLocked.value) return null
  const pick = (x: number, y: number) => document.elementFromPoint(x, y) as HTMLElement | null
  let el = pick(pos.x, pos.y)
  const dpr = window.devicePixelRatio || 1
  if (dpr !== 1 && (!el || !el.closest('.pane.remote'))) {
    const scaled = pick(pos.x / dpr, pos.y / dpr)
    if (scaled?.closest('.pane.remote')) el = scaled
  }
  if (!el || !el.closest('.pane.remote')) return null
  const name = (el.closest('.row.dir') as HTMLElement | null)?.dataset.name
  return name ? joinPath(remotePath.value, name) : remotePath.value
}

function onExternalDragEvent(ev: DragDropEvent) {
  if (ev.type === 'leave') { clearDropTarget(); return }
  if (ev.type === 'enter' || ev.type === 'over') {
    const dir = externalDropTargetAt(ev.position)
    if (dir) {
      dropSide.value = 'remote'
      dropDir.value = dir
    } else if (dropSide.value) {
      clearDropTarget()
    }
    return
  }
  const targetDir = externalDropTargetAt(ev.position)
  clearDropTarget()
  if (targetDir) void uploadExternalPaths(ev.paths, targetDir)
}

/** 外部路径入队上传：文件夹递归展开，文件按本地 stat 大小建单文件任务 */
async function uploadExternalPaths(paths: string[], targetDir: string) {
  if (!connected.value) { toast('请先建立 SSH 连接', 'warn'); return }
  if (sftpLocked.value) { toast('会话已锁定，请先解锁', 'warn'); return }
  let queued = 0
  for (const p of paths) {
    const name = p.split(/[\\/]/).filter(Boolean).pop() || p
    try {
      const info = await invoke<{ isDir: boolean; size: number }>('local_stat', { path: p })
      const dst = joinPath(targetDir, name)
      const before = transfers.value.length
      if (info.isDir) startFolderTask('up', name, p, dst)
      else startTask('up', name, info.size, { localPath: p, remotePath: dst })
      // 命中上传黑名单等情况不会入队，按队列增量计数
      if (transfers.value.length > before) queued++
    } catch (e) {
      toast(`无法读取 ${name}：${String(e)}`, 'warn', 3000)
    }
  }
  if (queued) toast(`已加入上传队列：${queued} 项`, 'ok', 1800)
}

function goUp(side: 'local' | 'remote') {
  if (side === 'local') void loadLocal(parentPath(localPath.value))
  else {
    if (sftpLocked.value) return
    const next = parentPath(remotePath.value)
    void loadRemote(next)
    syncShellCwd(next)
  }
}
function gotoCrumb(side: 'local' | 'remote', path: string) {
  if (side === 'local') void loadLocal(path)
  else {
    if (sftpLocked.value) return
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
  remoteHist.value = { stack: [], idx: -1 } // 历史属于单个会话，切换/断开即重置
  applySftpIdleTimeout() // 新会话继承当前空闲超时设置
  if (id) void loadRemote()
})

/* 设置页修改空闲超时后即时下发给当前会话（后端按新值判定下次回收） */
watch(() => savedSettings.sftpIdleTimeoutSec, () => applySftpIdleTimeout())

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
function switchTab(tab: 'sftp' | 'log' | 'tunnel') {
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
// 日志自动换行（纯视图偏好，默认开启，随 ui_state 节持久化）；关闭时长行单行横向滚动。
// 本组件晚于启动快照挂载，初始值直接读快照；onConfigLoad 处理导入重载后的再 hydrate。
const logWrap = ref(getSnapshot()?.uiState.logWrap !== false)
let wrapHydrated = true
onConfigLoad(snap => {
  wrapHydrated = false
  logWrap.value = snap.uiState.logWrap !== false
  wrapHydrated = true
})
watch(
  logWrap,
  v => {
    if (!wrapHydrated) return
    patchUiState({ logWrap: v })
  },
  { flush: 'sync' },
)
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
  // Tab 栏真实 class 为 .tabs-bar；此前查 .tabs 恒为空，导致上限退化为整个 workspace 高度
  const tabsEl = ws.querySelector('.tabs-bar')
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
  // 会话在线时同步一次空闲超时（设置可能在上次下发后已变更）
  applySftpIdleTimeout()

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
  // 自绘拖拽（跨栏传输）
  document.addEventListener('pointermove', onRowPointerMove)
  document.addEventListener('pointerup', onRowPointerUp)
  document.addEventListener('pointercancel', onRowPointerCancel)
  document.addEventListener('keydown', onDragKeydown)
  window.addEventListener('blur', onDragBlur)

  // 原生文件拖入（外部拖拽上传，三端统一启用）
  if (isTauri) {
    void getCurrentWebview().onDragDropEvent((event) => onExternalDragEvent(event.payload)).then((fn) => {
      unlistenExternalDrop = fn
    })
  }
})
onUnmounted(() => {
  document.removeEventListener('mousemove', onMove)
  document.removeEventListener('mouseup', onUp)
  document.removeEventListener('keydown', onFindShortcut)
  document.removeEventListener('keydown', onSftpShortcut)
  document.removeEventListener('pointermove', onRowPointerMove)
  document.removeEventListener('pointerup', onRowPointerUp)
  document.removeEventListener('pointercancel', onRowPointerCancel)
  document.removeEventListener('keydown', onDragKeydown)
  window.removeEventListener('blur', onDragBlur)
  stopAutoScroll()
  unlistenExternalDrop?.()
  unlistenExternalDrop = null
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

      <button class="dock-tab" :class="{ active: activeTab === 'tunnel' }" title="端口转发"
              @click="switchTab('tunnel')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round">
          <circle cx="5" cy="12" r="2.5"></circle>
          <circle cx="19" cy="12" r="2.5"></circle>
          <path d="M7.5 12h9"></path>
        </svg>
        端口转发
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
            <button class="sftp-btn" title="刷新本地与远端目录" :disabled="sftpLocked" @click="refreshAll">
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
                <div class="pane-path" :class="{ editing: pathEditing === 'local' }"
                     :title="pathEditing === 'local' ? '不含 / 为筛选，含 / 为目录路径 · Esc 取消' : '单击筛选或输入路径'"
                     @click="onPathClick('local', $event)">
                  <input v-if="pathEditing === 'local'" v-model="pathDraft" class="pane-path-input"
                         spellcheck="false" autocomplete="off" placeholder="筛选"
                         @input="onPathInput('local')"
                         @keydown="onPathKey($event, 'local')"
                         @blur="pathEditing = null" />
                  <span v-else-if="localFilter" class="path-filter-text">{{ localFilter }}</span>
                  <template v-else>
                    <template v-for="(c, i) in crumbs(localPath)" :key="c.path">
                      <span v-if="i" class="sep">/</span>
                      <span v-if="c.label" class="crumb" @click="gotoCrumb('local', c.path)">{{ c.label }}</span>
                    </template>
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
                <button class="pane-btn transfer-btn" :class="{ ready: localSel.length }"
                        :title="localSel.length ? `上传选中的 ${localSel.length} 个文件到远程` : '先在列表中选择文件'"
                        :disabled="!localSel.length || sftpLocked"
                        @click="onUpload">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                       stroke-linecap="round" stroke-linejoin="round">
                    <path d="M12 19V5M5 12l7-7 7 7"/>
                  </svg>
                </button>
                <button class="pane-btn" title="更多操作" @click="openPaneMenu('local', $event)">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4"
                       stroke-linecap="round" stroke-linejoin="round">
                    <path d="M5 12h.01M12 12h.01M19 12h.01"/>
                  </svg>
                </button>
              </div>
              <div class="list-header-wrap">
                <div class="list-header">
                  <div class="lh-name" :class="{ sorted: localSort.key==='name' }" @click="changeSort('local','name')">名称</div>
                  <div class="lh-size" :class="{ sorted: localSort.key==='size' }" @click="changeSort('local','size')">大小</div>
                  <div class="lh-time" :class="{ sorted: localSort.key==='mtime' }" @click="changeSort('local','mtime')">修改时间</div>
                  <div class="lh-perm">权限</div>
                  <div class="lh-owner">用户/组</div>
                </div>
              </div>
              <div class="pane-list-wrap" :class="{ 'drop-active': dropSide === 'local' }">
                <div class="pane-list" @contextmenu="onBlankContext($event, 'local')"
                     @scroll="onListScroll" @pointerdown="onPanePointerDown($event, 'local')"
                     @click="onPaneClick('local', $event)">
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
                    :class="{ dir: e.type==='dir', selected: localSelected.has(e.name),
                              dragging: dragSide === 'local' && localSelected.has(e.name),
                              'drop-target': dropSide === 'local' && e.type === 'dir'
                                && dropDir === joinPath(localPath, e.name) }"
                    :data-name="e.name"
                    @click="onRowClick('local', e, $event)"
                    @dblclick="onRowDblClick('local', e)"
                    @contextmenu.stop="onRowContext($event, 'local', e)"
                    @pointerdown="onRowPointerDown('local', e, $event)"
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
                      <span v-else class="row-text">
                        <template v-for="(seg, i) in nameSegments(e.name, 'local')" :key="i">
                          <mark v-if="seg.hit" class="text-hit">{{ seg.text }}</mark>
                          <template v-else>{{ seg.text }}</template>
                        </template>
                      </span>
                      <svg v-if="e.isSymlink" class="link-badge" viewBox="0 0 16 16" title="符号链接">
                        <path d="M7 2H4a3 3 0 0 0 0 6h2" stroke="currentColor" stroke-width="1.8" fill="none" stroke-linecap="round"/>
                        <path d="M9 14h3a3 3 0 0 0 0-6h-2" stroke="currentColor" stroke-width="1.8" fill="none" stroke-linecap="round"/>
                      </svg>
                    </div>
                    <div class="row-size"><span class="cell-text">{{ e.type==='dir' ? '—' : fmtSize(e.size) }}</span></div>
                    <div class="row-time"><span class="cell-text">{{ e.mtime }}</span></div>
                    <div class="row-perm"><span class="cell-text">{{ e.perm }}</span></div>
                    <div class="row-owner"><span class="cell-text">{{ e.owner }}</span></div>
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
                <div class="pane-path" :class="{ editing: pathEditing === 'remote' }"
                     :title="pathEditing === 'remote' ? '不含 / 为筛选，含 / 为目录路径 · Esc 取消' : '单击筛选或输入路径'"
                     @click="onPathClick('remote', $event)">
                  <input v-if="pathEditing === 'remote'" v-model="pathDraft" class="pane-path-input"
                         spellcheck="false" autocomplete="off" placeholder="筛选"
                         @input="onPathInput('remote')"
                         @keydown="onPathKey($event, 'remote')"
                         @blur="pathEditing = null" />
                  <span v-else-if="!connected" class="crumb dim">未连接</span>
                  <span v-else-if="remoteFilter" class="path-filter-text">{{ remoteFilter }}</span>
                  <template v-else>
                    <template v-for="(c, i) in crumbs(remotePath)" :key="c.path">
                      <span v-if="i" class="sep">/</span>
                      <span v-if="c.label" class="crumb" @click="gotoCrumb('remote', c.path)">{{ c.label }}</span>
                    </template>
                  </template>
                </div>
                <button class="pane-btn" title="上一级" :disabled="!connected || sftpLocked" @click="goUp('remote')">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                       stroke-linecap="round" stroke-linejoin="round">
                    <path d="M19 12H5M12 19l-7-7 7-7"/>
                  </svg>
                </button>
                <button class="pane-btn" title="刷新" :disabled="!connected || remoteLoading || sftpLocked"
                        @click="loadRemote(remotePath)">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                       stroke-linecap="round" stroke-linejoin="round">
                    <path d="M21 12a9 9 0 11-3-6.7L21 8"/><path d="M21 3v5h-5"/>
                  </svg>
                </button>
                <button class="pane-btn" title="新建文件夹" :disabled="!connected || sftpLocked" @click="startEdit('remote', 'mkdir')">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                       stroke-linecap="round" stroke-linejoin="round">
                    <path d="M4 7a2 2 0 012-2h3l2 2h7a2 2 0 012 2v8a2 2 0 01-2 2H6a2 2 0 01-2-2V7z"/>
                    <path d="M12 11v5M9.5 13.5h5" stroke-width="2.2"/>
                  </svg>
                </button>
                <button class="pane-btn transfer-btn" :class="{ ready: remoteSel.length }"
                        :title="remoteSel.length ? `下载选中的 ${remoteSel.length} 个文件到本地` : '先在列表中选择文件'"
                        :disabled="!remoteSel.length || sftpLocked"
                        @click="onDownload">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                       stroke-linecap="round" stroke-linejoin="round">
                    <path d="M12 5v14M5 12l7 7 7-7"/>
                  </svg>
                </button>
                <button class="pane-btn" title="更多操作" :disabled="!connected || sftpLocked"
                        @click="openPaneMenu('remote', $event)">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4"
                       stroke-linecap="round" stroke-linejoin="round">
                    <path d="M5 12h.01M12 12h.01M19 12h.01"/>
                  </svg>
                </button>
              </div>
              <div class="list-header-wrap">
                <div class="list-header">
                  <div class="lh-name" :class="{ sorted: remoteSort.key==='name' }" @click="changeSort('remote','name')">名称</div>
                  <div class="lh-size" :class="{ sorted: remoteSort.key==='size' }" @click="changeSort('remote','size')">大小</div>
                  <div class="lh-time" :class="{ sorted: remoteSort.key==='mtime' }" @click="changeSort('remote','mtime')">修改时间</div>
                  <div class="lh-perm">权限</div>
                  <div class="lh-owner">用户/组</div>
                </div>
              </div>
              <div class="pane-list-wrap" :class="{ 'drop-active': dropSide === 'remote' }">
                <div class="pane-list" @contextmenu="onBlankContext($event, 'remote')"
                     @scroll="onListScroll" @pointerdown="onPanePointerDown($event, 'remote')"
                     @click="onPaneClick('remote', $event)">
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
                    :class="{ dir: e.type==='dir', selected: remoteSelected.has(e.name),
                              dragging: dragSide === 'remote' && remoteSelected.has(e.name),
                              'drop-target': dropSide === 'remote' && e.type === 'dir'
                                && dropDir === joinPath(remotePath, e.name) }"
                    :data-name="e.name"
                    @click="onRowClick('remote', e, $event)"
                    @dblclick="onRowDblClick('remote', e)"
                    @contextmenu.stop="onRowContext($event, 'remote', e)"
                    @pointerdown="onRowPointerDown('remote', e, $event)"
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
                      <span v-else class="row-text">
                        <template v-for="(seg, i) in nameSegments(e.name, 'remote')" :key="i">
                          <mark v-if="seg.hit" class="text-hit">{{ seg.text }}</mark>
                          <template v-else>{{ seg.text }}</template>
                        </template>
                      </span>
                      <svg v-if="e.isSymlink" class="link-badge" viewBox="0 0 16 16" title="符号链接">
                        <path d="M7 2H4a3 3 0 0 0 0 6h2" stroke="currentColor" stroke-width="1.8" fill="none" stroke-linecap="round"/>
                        <path d="M9 14h3a3 3 0 0 0 0-6h-2" stroke="currentColor" stroke-width="1.8" fill="none" stroke-linecap="round"/>
                      </svg>
                    </div>
                    <div class="row-size"><span class="cell-text">{{ e.type==='dir' ? '—' : fmtSize(e.size) }}</span></div>
                    <div class="row-time"><span class="cell-text">{{ e.mtime }}</span></div>
                    <div class="row-perm"><span class="cell-text">{{ e.perm }}</span></div>
                    <div class="row-owner"><span class="cell-text">{{ e.owner }}</span></div>
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
          <div class="sftp-tasks" :class="{ collapsed: sftpQueueCollapsed }"
               :style="{ height: sftpQueueCollapsed ? '28px' : sftpQueueHeight + 'px' }">
            <div class="queue-resizer" @mousedown.prevent="startQueueDrag"></div>
            <!-- 头部：指示灯 + 标题 + 总数（+折叠态进度摘要）；右侧 全部暂停 / 全部继续 / 清空 / 折叠。
                 点击头部空白区可折叠 / 展开 -->
            <div class="queue-head" :title="sftpQueueCollapsed ? '点击展开传输队列' : '点击收起传输队列'"
                 @click="onQueueHeadClick">
              <span class="queue-led" :class="{ idle: !runningCount && !pendingCount }"></span>
              <span class="queue-title">传输队列</span>
              <span class="queue-badge">{{ queueItems.length }}</span>
              <span v-if="sftpQueueCollapsed && queueSummary" class="queue-summary"
                    :title="queueSummary">{{ queueSummary }}</span>
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
              <button class="queue-clear" title="清空已完成/已取消记录"
                      :disabled="!successCount && !cancelledCount" @click="clearDoneTasks">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                     stroke-linecap="round" stroke-linejoin="round">
                  <path d="M4 7h16M9 7V4h6v3M6.5 7l.8 13h9.4l.8-13M10 11v5M14 11v5"/>
                </svg>
              </button>
              <button class="queue-fold"
                      :title="sftpQueueCollapsed ? '展开传输队列' : '收起传输队列'"
                      @click="toggleQueueCollapse">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                     stroke-linecap="round" stroke-linejoin="round">
                  <path v-if="sftpQueueCollapsed" d="M6 15l6-6 6 6"/>
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
            <div ref="queueListEl" class="queue-list" :style="{ maxHeight: queueListMaxHeight }"
                 @scroll="onQueueScroll">
              <div v-if="!queueItems.length" class="queue-empty">
                暂无传输任务
              </div>
              <div v-for="t in filteredTransfers" :key="t.id"
                   class="sftp-task"
                   :class="[t.dir === 'up' ? 'upload' : 'download', t.status,
                            { folder: t.kind === 'dir', expanded: t.kind === 'dir' && t.expanded }]">
                <div class="task-row">
                  <span class="task-main">
                  <!-- 文件夹用目录图标；文件用上传/下载方向图标 -->
                  <span class="task-ico">
                    <svg v-if="t.kind === 'dir'" viewBox="0 0 16 16" fill="none" stroke="currentColor"
                         stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                      <path d="M2 4.4a1.4 1.4 0 0 1 1.4-1.4h2.7l1.4 1.6h5.1A1.4 1.4 0 0 1 14 6v5.6a1.4 1.4 0 0 1-1.4 1.4H3.4A1.4 1.4 0 0 1 2 11.6z"/>
                    </svg>
                    <svg v-else viewBox="0 0 16 16" fill="none" stroke="currentColor"
                         stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                      <path d="M4 1.9h4.6l3.4 3.4v8.1a.9.9 0 0 1-.9.9H4a.9.9 0 0 1-.9-.9V2.8a.9.9 0 0 1 .9-.9z"/>
                      <path d="M8.4 1.9v3.6h3.6"/>
                    </svg>
                  </span>
                  <!-- 文件名 / 方向 · 大小 · 目标路径 -->
                  <span class="task-info">
                    <span class="task-name" :title="t.name">
                      <!-- 文件夹任务可展开子文件列表 -->
                      <button v-if="t.kind === 'dir' && visibleChildren(t).length" class="task-twist"
                              :title="t.expanded ? '收起文件列表' : '展开文件列表'"
                              @click.stop="toggleTaskExpand(t)">
                        <svg viewBox="0 0 10 10" fill="none" stroke="currentColor" stroke-width="1.6"
                             stroke-linecap="round" stroke-linejoin="round">
                          <path d="M3.6 1.8 7 5l-3.4 3.2"/>
                        </svg>
                      </button>
                      <span class="task-name-text">{{ taskName(t) }}</span>
                      <span v-if="taskStatusText(t)" class="file-tag"
                            :class="[t.status, { verifying: t.status === 'running' && t.verifying }]">{{ taskStatusText(t) }}</span>
                    </span>
                    <span class="task-sub">
                      <span class="dir-tag" :class="t.dir">{{ t.dir === 'up' ? '↑ 上传' : '↓ 下载' }}</span>
                      <span class="sep">·</span>
                      <span class="size">{{ fmtSize(t.size) }}</span>
                      <span class="sep">·</span>
                      <span class="path" :title="targetPath(t)">{{ targetPath(t) }}</span>
                    </span>
                  </span>
                  <!-- 传输进度：进度条 + 百分比/速率/剩余 -->
                  <span class="task-progress">
                    <span class="track"><i class="fill" :style="{ width: Math.min(100, t.pct) + '%' }"></i></span>
                    <span class="bar-meta">
                      <span class="pct">{{ pctText(t) }}</span>
                      <span v-if="folderCountText(t)" class="bar-count">{{ folderCountText(t) }}</span>
                      <span v-if="speedText(t)" class="speed">{{ speedText(t) }}</span>
                      <span v-if="noteText(t)" class="bar-note"
                            :class="{ verifying: t.status === 'running' && t.verifying }"
                            :title="noteText(t)">{{ noteText(t) }}</span>
                    </span>
                  </span>
                  </span>
                  <!-- 操作（图标保持不变，按状态显隐） -->
                  <span class="acts">
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
                    <!-- 重试：失败/已取消 -->
                    <button v-if="t.status === 'error' || t.status === 'cancelled'" class="task-abtn" title="重试"
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
                    <!-- 取消：进行中/排队中/已暂停（保留"已取消"记录） -->
                    <button v-if="t.status === 'pending' || t.status === 'running' || t.status === 'paused'"
                            class="task-x" title="取消传输"
                            @click="cancelTask(t)">
                      <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8"
                           stroke-linecap="round">
                        <path d="M4.3 4.3l7.4 7.4M11.7 4.3l-7.4 7.4"/>
                      </svg>
                    </button>
                    <!-- 移除记录：已完成/已取消/失败 -->
                    <button v-else class="task-x" title="移除记录" @click="removeTask(t)">
                      <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"
                           stroke-linecap="round" stroke-linejoin="round">
                        <path d="M3 4.4h10"/><path d="M6.4 4.4V3.2h3.2v1.2"/>
                        <path d="M4.4 4.4l.6 8a1 1 0 0 0 1 .9h4a1 1 0 0 0 1-.9l.6-8"/>
                      </svg>
                    </button>
                  </span>
                </div>
                <!-- 文件夹任务：展开后的文件清单（收起时不渲染子项行，避免大量 DOM） -->
                <div v-if="t.kind === 'dir' && t.expanded && visibleChildren(t).length" class="task-children">
                  <!-- 子项列：文件图标 | 文件名 | 大小 | 进度(条+百分比) | SHA-256 | 操作 | 状态 -->
                  <div v-for="c in visibleChildren(t)" :key="c.id" class="tc-item"
                       :class="[c.status, { verifying: c.status === 'running' && c.verifying, skipped: c.skipped }]">
                    <span class="tc-main">
                    <span class="tc-ico">
                      <svg viewBox="0 0 16 16" fill="none" stroke="currentColor"
                           stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                        <path d="M4 1.9h4.6l3.4 3.4v8.1a.9.9 0 0 1-.9.9H4a.9.9 0 0 1-.9-.9V2.8a.9.9 0 0 1 .9-.9z"/>
                        <path d="M8.4 1.9v3.6h3.6"/>
                      </svg>
                    </span>
                    <span class="tc-file">
                      <span class="tc-name" :title="c.name">{{ c.name }}</span>
                      <span class="tc-size">{{ fmtSize(c.size || 0) }}</span>
                    </span>
                    <span class="tc-bar" :title="c.status === 'running' && c.speed ? fmtSpeed(c.speed) : ''">
                      <i :style="{ width: Math.min(100, c.pct) + '%' }"></i>
                    </span>
                    <span class="tc-pct">{{ childPctText(c) }}</span>
                    <!-- 结果列：展示传输状态 / 错误信息 -->
                    <span class="tc-result" :class="{ verifying: c.status === 'running' && c.verifying }"
                          :title="childStateText(c)">
                      <span class="tc-state">{{ childStateText(c) }}</span>
                    </span>
                    </span>
                    <!-- 单文件级操作：暂停/继续/重试/打开/取消/移除，常态显示 -->
                    <span class="tc-acts">
                      <button v-if="c.status === 'success'" class="tc-act"
                              title="打开目录" @click.stop="openTaskDir(c)">
                        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"
                             stroke-linecap="round" stroke-linejoin="round">
                          <path d="M2.3 4.6a1 1 0 0 1 1-1h2.5l1.3 1.6h5.6a1 1 0 0 1 1 1v5.8a1 1 0 0 1-1 1H3.3a1 1 0 0 1-1-1z"/>
                        </svg>
                      </button>
                      <button v-if="c.status === 'pending' || c.status === 'running'" class="tc-act"
                              title="暂停" @click.stop="pauseTask(c)">
                        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor"
                             stroke-width="1.8" stroke-linecap="round">
                          <path d="M6 3.4v9.2"/><path d="M10 3.4v9.2"/>
                        </svg>
                      </button>
                      <button v-if="c.status === 'paused'" class="tc-act"
                              title="继续" @click.stop="resumeTask(c)">
                        <svg viewBox="0 0 16 16" fill="currentColor">
                          <path d="M5.6 3.4 12.1 8l-6.5 4.6z"/>
                        </svg>
                      </button>
                      <button v-if="c.status === 'error' || c.status === 'cancelled'" class="tc-act"
                              title="重试" @click.stop="retryTask(c)">
                        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6"
                             stroke-linecap="round" stroke-linejoin="round">
                          <path d="M13.1 8a5.1 5.1 0 1 1-1.6-3.7"/><path d="M13.1 2.7v3.1H10"/>
                        </svg>
                      </button>
                      <button v-if="c.status === 'pending' || c.status === 'running' || c.status === 'paused'"
                              class="tc-act tc-act-x" title="取消" @click.stop="cancelTask(c)">
                        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8"
                             stroke-linecap="round">
                          <path d="M4.3 4.3l7.4 7.4M11.7 4.3l-7.4 7.4"/>
                        </svg>
                      </button>
                      <button v-else class="tc-act tc-act-x"
                              title="移除记录" @click.stop="removeTask(c)">
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
              <!-- 上滚查看历史时，新任务在此提示；点击回到最新 -->
              <button v-if="queueNewCount && !sftpQueueCollapsed" class="queue-latest"
                      @click="scrollQueueToLatest">
                {{ queueNewCount }} 个新任务
                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8"
                     stroke-linecap="round" stroke-linejoin="round">
                  <path d="M8 3.4v9.2M4.4 8.9 8 12.5l3.6-3.6"/>
                </svg>
              </button>
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
            autocomplete="off"
            autocapitalize="off"
            autocorrect="off"
            spellcheck="false"
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
          <!-- 非活动 tab 卸载行 DOM：debug 级别下传输会持续刷日志，
               常驻数千行每批全量 diff 会占满主线程，导致界面完全无响应 -->
          <template v-if="activeTab === 'log'">
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
            <span class="log-msg" data-selectable>
              <template v-for="(seg, i) in highlightParts(l.msg)" :key="i">
                <mark v-if="seg.hit" class="log-hit"
                      :class="{ current: l.id === currentMatchId }">{{ seg.text }}</mark>
                <template v-else>{{ seg.text }}</template>
              </template>
            </span>
          </div>
          </template>
          </template>
        </div>
      </div>

      <!-- 端口转发（规则启停与运行态观测；规则编辑在主机配置面板） -->
      <div class="dock-pane" :class="{ show: activeTab === 'tunnel' }">
        <TunnelPane />
      </div>
    </div>

    <!-- 右键菜单（Teleport 到 body，定位与键盘导航由组件内部完成） -->
    <ContextMenu v-model:visible="menuVisible" :x="menuX" :y="menuY" :items="menuItems" />

    <!-- 拖拽跟随光标的信息条（Teleport 到 body，避免被 dock 边框裁剪） -->
    <Teleport to="body">
      <div v-if="dragSide" class="row-drag-ghost"
           :style="{ left: dragPos.x + 14 + 'px', top: dragPos.y + 12 + 'px' }">
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path d="M14 3H6a2 2 0 00-2 2v14a2 2 0 002 2h12a2 2 0 002-2V9z"/>
          <path d="M14 3v6h6"/>
        </svg>
        <span>{{ dragLabel }}</span>
      </div>
    </Teleport>
  </section>
</template>
