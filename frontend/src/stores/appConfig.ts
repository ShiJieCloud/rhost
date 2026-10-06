/**
 * 应用配置底座：后端 app_config.json 的唯一前端通道（localStorage 已下线）。
 *
 * 职责：
 * - 启动时 `loadAppConfig()` 拉取全节快照（后端文件为唯一真相源）；
 * - 各业务 store 经 `onConfigLoad` 注册 hydrate 回调，把快照节灌入自身响应式状态；
 * - 写穿：`schedulePersist`（300ms 按节防抖，适合 watch 自动保存）与
 *   `persistSectionNow`（显式保存后立即落盘）；
 * - 导入/重置后 `reloadAllAfterImport` 重新拉取并 hydrate 全部 store + 主机列表。
 *
 * logs 节不在此写穿：日志配置走既有 `set_log_config`（内存热更新 + 落盘一体）。
 */
import { invoke } from '@tauri-apps/api/core'
import { isTauri } from '../lib/tauri'
import { loadHosts } from './hosts'

/** 可经 set_app_config_section 写穿的节（logs 走专用 IPC，不在此列） */
export type ConfigSection =
  | 'settings'
  | 'keys'
  | 'groups'
  | 'ui_state'
  | 'quick_connect_history'

/** 后端 AppConfigSnapshot（serde camelCase；各节为原始 JSON，由各 store 自行解释） */
export interface AppConfigSnapshot {
  logs: Record<string, unknown>
  settings: Record<string, unknown>
  keys: unknown[]
  groups: unknown[]
  uiState: Record<string, unknown>
  quickConnectHistory: unknown[]
  /** 导入文件大小上限（后端权威，UI 预检取此值） */
  maxImportFileBytes: number
  /** 日志存储路径留空时的平台默认目录（设置面板输入框 placeholder） */
  defaultLogDir: string
}

/** 导入结果摘要（与后端 ImportSummary camelCase 对齐） */
export interface ImportSummary {
  connectionsAdded: number
  connectionsRenamed: number
  keysAdded: number
  keysRenamed: number
  groupsAdded: number
  settingsChanged: number
}

/** read_import_file 返回内容 */
export interface ImportFileContent {
  text: string
  bytes: number
  fileName: string
}

/** 最近一次成功加载的快照（未加载前为 null；非 Tauri 环境为浏览器降级空快照） */
let snapshot: AppConfigSnapshot | null = null

export function getSnapshot(): AppConfigSnapshot | null {
  return snapshot
}

/** 浏览器 dev 模式的降级快照（各节空值，store 各自回退内置默认/MOCK） */
function browserFallbackSnapshot(): AppConfigSnapshot {
  return {
    logs: {},
    settings: {},
    keys: [],
    groups: [],
    uiState: {},
    quickConnectHistory: [],
    maxImportFileBytes: 5 * 1024 * 1024,
    defaultLogDir: '',
  }
}

/* ---- hydrate 注册表（避免本模块反向依赖业务 store，杜绝循环依赖） ---- */

type Hydrater = (snap: AppConfigSnapshot) => void
const hydraters: Hydrater[] = []

/** 业务 store 注册「快照到达/刷新」回调（模块顶层调用即可，幂等由注册方保证） */
export function onConfigLoad(fn: Hydrater) {
  hydraters.push(fn)
}

function applyToStores(snap: AppConfigSnapshot) {
  for (const fn of hydraters) {
    try {
      fn(snap)
    } catch (e) {
      // 单个 store hydrate 失败不影响其他 store
      console.error('配置 hydrate 失败:', e)
    }
  }
}

/** 启动时调用：拉取全节快照并 hydrate 所有已注册 store */
export async function loadAppConfig(): Promise<AppConfigSnapshot> {
  if (!isTauri) {
    snapshot = browserFallbackSnapshot()
    applyToStores(snapshot)
    return snapshot
  }
  const snap = await invoke<AppConfigSnapshot>('load_app_config')
  snapshot = snap
  applyToStores(snap)
  return snap
}

/** 导入/恢复后刷新：重新拉取配置并 hydrate，再重载主机列表（调用方负责随后的重启提示） */
export async function reloadAllAfterImport(): Promise<void> {
  await loadAppConfig()
  await loadHosts()
}

/* ---- 写穿（300ms 防抖 / 立即） ---- */

const DEBOUNCE_MS = 300
const timers = new Map<ConfigSection, ReturnType<typeof setTimeout>>()

/** 按节防抖写穿；同节连续变更合并为一次 IPC */
export function schedulePersist(section: ConfigSection, value: unknown) {
  if (!isTauri) return
  const old = timers.get(section)
  if (old) clearTimeout(old)
  timers.set(
    section,
    setTimeout(() => {
      timers.delete(section)
      void persistSectionNow(section, value)
    }, DEBOUNCE_MS),
  )
}

/** 立即写穿单节（schema 校验失败后端拒绝且零写入，错误返回供 UI 提示） */
export async function persistSectionNow(
  section: ConfigSection,
  value: unknown,
): Promise<void> {
  if (!isTauri) return
  // 待发的防抖写以本次显式写为准：取消挂起定时器，避免旧值后写覆盖
  const old = timers.get(section)
  if (old) {
    clearTimeout(old)
    timers.delete(section)
  }
  await invoke('set_app_config_section', { section, value })
}

/**
 * 局部合并写 ui_state 节。
 * ui_state 由多个消费方共享（homeView / layout / sessions / logWrap），
 * 各方只提交自己的键；这里以快照镜像为底做 merge，避免整节覆盖互相丢失。
 * 300ms 防抖合并同一波连续变更。
 */
export function patchUiState(patch: Record<string, unknown>) {
  if (!isTauri || !snapshot) return
  snapshot.uiState = { ...snapshot.uiState, ...patch }
  schedulePersist('ui_state', snapshot.uiState)
}

/* ---- 导入 / 导出 / 重置 IPC 封装（UI 层 K 阶段消费） ---- */

/**
 * 检测文件内容是否为加密 envelope（顶层 meta.encrypted === true）。
 * 解析失败按明文处理——后续 IPC 会返回权威的格式错误。
 */
export function isEncryptedConfig(text: string): boolean {
  try {
    const doc = JSON.parse(text) as unknown
    return (
      !!doc &&
      typeof doc === 'object' &&
      (doc as Record<string, unknown>).meta != null &&
      ((doc as Record<string, unknown>).meta as Record<string, unknown>).encrypted === true
    )
  } catch {
    return false
  }
}

export async function readImportFile(path: string): Promise<ImportFileContent> {
  return invoke<ImportFileContent>('read_import_file', { path })
}

export async function importConfig(
  payload: string,
  source: string,
  password?: string,
): Promise<ImportSummary> {
  return invoke<ImportSummary>('import_config', { payload, source, password: password ?? null })
}

export async function importHosts(
  payload: string,
  source: string,
  password?: string,
): Promise<ImportSummary> {
  return invoke<ImportSummary>('import_hosts', { payload, source, password: password ?? null })
}

export async function exportConfig(
  path: string,
  scope: 'full' | 'hosts' | 'ui',
  includeUi: boolean,
  includeHistory: boolean,
  password?: string,
): Promise<string> {
  return invoke<string>('export_config', {
    path,
    scope,
    includeUi,
    includeHistory,
    password: password ?? null,
  })
}

export async function resetSettingsConfig(): Promise<void> {
  await invoke('reset_settings_config')
}
