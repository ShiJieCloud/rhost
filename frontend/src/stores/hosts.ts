import { computed, ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { Host, StoredHost, ViewStyle } from '../types'
import { MOCK_HOSTS, STYLE_LABELS } from '../data/mockHosts'
import { isTauri } from '../lib/tauri'

/** 模块级单例状态；数据由后端 JSON 文件持久化，密码存系统钥匙串 */
export const hosts = ref<Host[]>([])
export const viewStyle = ref<ViewStyle>('list')
export const filter = ref('')
export const showNewConn = ref(false)

export const filtered = computed(() => {
  const q = filter.value.trim().toLowerCase()
  if (!q) return hosts.value
  return hosts.value.filter(
    h =>
      h.id.toLowerCase().includes(q) ||
      h.ip.includes(q) ||
      h.user.toLowerCase().includes(q) ||
      h.os.toLowerCase().includes(q) ||
      h.tag.includes(q) ||
      h.group.includes(q),
  )
})

export const styleLabel = computed(() => STYLE_LABELS[viewStyle.value])

export const filterInfo = computed(() =>
  filter.value.trim() ? `筛选出 ${filtered.value.length} 台主机` : '无筛选',
)

/* =========================================================
 *  前后端数据转换
 * ========================================================= */

/** 后端 StoredHost → 前端 Host（补齐运行时状态默认值） */
function storedToHost(s: StoredHost): Host {
  return {
    id: s.id,
    user: s.user,
    ip: s.ip,
    port: s.port,
    os: s.os,
    color: s.color as Host['color'],
    label: s.label,
    tag: s.tag,
    status: 'idle',
    lat: null,
    cpu: '—',
    mem: '—',
    uptime: '—',
    group: s.group,
    keyPath: s.keyPath ?? undefined,
    // password 不入 JSON，连接时从钥匙串按需读取
  }
}

/** 前端 Host → 后端 StoredHost（剔除运行时状态与密码） */
function hostToStored(h: Host, connType: string): StoredHost {
  return {
    id: h.id,
    connType,
    user: h.user,
    ip: h.ip,
    port: h.port,
    os: h.os,
    color: h.color,
    label: h.label,
    tag: h.tag,
    group: h.group,
    keyPath: h.keyPath ?? null,
    extra: {},
    updatedAt: Date.now(),
  }
}

/* =========================================================
 *  持久化操作（Tauri 环境走后端，浏览器环境走内存 + MOCK）
 * ========================================================= */

/** 应用启动时调用：从后端 JSON 文件加载全部主机配置（密码不预加载，按需从钥匙串读取） */
export async function loadHosts() {
  if (!isTauri) {
    hosts.value = [...MOCK_HOSTS]
    return
  }
  try {
    const list = await invoke<StoredHost[]>('load_connections')
    hosts.value = list.map(storedToHost)
  } catch (e) {
    console.error('loadHosts 失败:', e)
    hosts.value = []
  }
}

export async function addHost(h: Host, connType = 'ssh') {
  hosts.value.push(h)
  if (!isTauri) return
  try {
    await invoke('save_connection', { host: hostToStored(h, connType) })
  } catch (e) {
    console.error('save_connection 失败:', e)
  }
}

/** 正在编辑的主机；非 null 时新建连接弹窗以编辑态打开 */
export const editingHost = ref<Host | null>(null)

export function openEdit(h: Host) {
  editingHost.value = h
  showNewConn.value = true
}

/** 按 id 局部更新主机，返回更新后的新对象（未找到返回 null）。
 *  若 patch 中包含 id 且与原 id 不同（改名），会先删除后端旧 id 的配置与钥匙串条目。 */
export async function updateHost(
  id: string,
  patch: Partial<Host>,
  connType = 'ssh',
): Promise<Host | null> {
  const idx = hosts.value.findIndex(h => h.id === id)
  if (idx === -1) return null
  const updated = { ...hosts.value[idx]!, ...patch }
  hosts.value[idx] = updated
  if (!isTauri) return updated
  try {
    // 改名：先清理后端旧 id 的配置与钥匙串条目，再保存新 id
    if (patch.id && patch.id !== id) {
      await invoke('delete_connection', { id }).catch(() => {})
    }
    await invoke('save_connection', { host: hostToStored(updated, connType) })
  } catch (e) {
    console.error('save_connection 失败:', e)
  }
  return updated
}

export async function removeHost(id: string) {
  const idx = hosts.value.findIndex(h => h.id === id)
  if (idx !== -1) hosts.value.splice(idx, 1)
  if (!isTauri) return
  try {
    await invoke('delete_connection', { id })
  } catch (e) {
    console.error('delete_connection 失败:', e)
  }
}
