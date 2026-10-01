import { computed, ref, watch } from 'vue'
import type { Host, HostColor } from '../types'
import { hosts } from './hosts'

/** 分组定义；主机的归属仍记录在 Host.group 字段上，此处只维护分组的元信息 */
export interface GroupDef {
  name: string
  color: HostColor
}

export interface GroupStat {
  def: GroupDef
  hosts: Host[]
  total: number
  online: number
  warn: number
  offline: number
  /** 在线主机的平均延迟；无在线主机时为 null */
  avgLat: number | null
}

/** 模块级单例状态；后续接 Pinia 或 Tauri 后端时仅需替换此处 */
const STORE_KEY = 'rhost.groups'
/** 兜底分组：删除分组时其下主机移入该分组，且该分组不可删除 */
export const FALLBACK_GROUP = '其他'

const COLOR_SEQ: HostColor[] = ['green', 'blue', 'cyan', 'purple', 'yellow', 'red']

const DEFAULT_GROUPS: GroupDef[] = [
  { name: '生产环境', color: 'green' },
  { name: '测试环境', color: 'purple' },
  { name: '开发环境', color: 'blue' },
  { name: FALLBACK_GROUP, color: 'yellow' },
]

function pickColor(i: number): HostColor {
  return COLOR_SEQ[i % COLOR_SEQ.length]!
}

function restore(): GroupDef[] {
  try {
    const raw = localStorage.getItem(STORE_KEY)
    if (!raw) return [...DEFAULT_GROUPS]
    const parsed = JSON.parse(raw)
    if (!Array.isArray(parsed)) return [...DEFAULT_GROUPS]
    const list = parsed.filter(
      (g): g is GroupDef => !!g && typeof g.name === 'string' && typeof g.color === 'string',
    )
    return list.length ? list : [...DEFAULT_GROUPS]
  } catch {
    return [...DEFAULT_GROUPS]
  }
}

export const groups = ref<GroupDef[]>(restore())

watch(
  groups,
  v => {
    try {
      localStorage.setItem(STORE_KEY, JSON.stringify(v))
    } catch { /* 持久化失败不影响内存态 */ }
  },
  { deep: true },
)

/** 主机中出现过但未登记的分组（如编辑主机时填写了新分组名），渲染时自动合并 */
const unregistered = computed<GroupDef[]>(() => {
  const registered = new Set(groups.value.map(g => g.name))
  const names = new Set<string>()
  hosts.value.forEach(h => {
    if (h.group && !registered.has(h.group)) names.add(h.group)
  })
  return [...names].map((name, i) => ({ name, color: pickColor(groups.value.length + i) }))
})

/** 渲染用全量分组列表 */
export const allGroups = computed<GroupDef[]>(() => [...groups.value, ...unregistered.value])

/** 各分组统计（主机列表、状态计数、平均延迟） */
export const groupStats = computed<GroupStat[]>(() =>
  allGroups.value.map(def => {
    const list = hosts.value.filter(h => h.group === def.name)
    const onlineList = list.filter(h => h.status === 'online')
    const withLat = onlineList.filter(h => h.lat !== null)
    return {
      def,
      hosts: list,
      total: list.length,
      online: onlineList.length,
      warn: list.filter(h => h.status === 'warn').length,
      offline: list.filter(h => h.status === 'offline').length,
      avgLat: withLat.length
        ? Math.round(withLat.reduce((s, h) => s + (h.lat ?? 0), 0) / withLat.length)
        : null,
    }
  }),
)

function nameTaken(name: string): boolean {
  return allGroups.value.some(g => g.name === name)
}

/** 新建/编辑分组弹窗状态；非 null 的 editingGroup 表示编辑态 */
export const showGroupModal = ref(false)
export const editingGroup = ref<GroupDef | null>(null)

export function openGroupModal(g?: GroupDef) {
  editingGroup.value = g ?? null
  showGroupModal.value = true
}

export function closeGroupModal() {
  showGroupModal.value = false
  editingGroup.value = null
}

/** 新建分组；名称为空或重名时返回 false */
export function addGroup(name: string, color?: HostColor): boolean {
  const n = name.trim()
  if (!n || nameTaken(n)) return false
  groups.value.push({ name: n, color: color ?? pickColor(groups.value.length) })
  return true
}

/**
 * 更新分组（重命名 + 换色），同步其下所有主机的 group 字段。
 * 名称未变时仅更新颜色；重名时返回 false。
 */
export function updateGroup(oldName: string, newName: string, color?: HostColor): boolean {
  const n = newName.trim()
  if (!n) return false
  if (n !== oldName && nameTaken(n)) return false
  const def = groups.value.find(g => g.name === oldName)
  if (def) {
    def.name = n
    if (color) def.color = color
  } else {
    // 未登记分组（由主机引入）被编辑：登记新名称
    groups.value.push({ name: n, color: color ?? pickColor(groups.value.length) })
  }
  if (n !== oldName) {
    hosts.value.forEach(h => {
      if (h.group === oldName) h.group = n
    })
  }
  return true
}

/** 删除分组；其下主机移入兜底分组。返回移走的主机数 */
export function removeGroup(name: string): number {
  const idx = groups.value.findIndex(g => g.name === name)
  if (idx !== -1) groups.value.splice(idx, 1)
  let moved = 0
  hosts.value.forEach(h => {
    if (h.group === name) {
      h.group = FALLBACK_GROUP
      moved++
    }
  })
  // 兜底分组不存在时补登记，保证移入主机有归属
  if (moved && !groups.value.some(g => g.name === FALLBACK_GROUP)) {
    groups.value.push({ name: FALLBACK_GROUP, color: pickColor(groups.value.length) })
  }
  return moved
}
