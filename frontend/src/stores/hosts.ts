import { computed, ref } from 'vue'
import type { Host, ViewStyle } from '../types'
import { MOCK_HOSTS, STYLE_LABELS } from '../data/mockHosts'

/** 模块级单例状态；后续接 Pinia 或 Tauri 后端时仅需替换此处 */
export const hosts = ref<Host[]>([...MOCK_HOSTS])
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

export const onlineCount = computed(
  () => hosts.value.filter(h => h.status === 'online').length,
)

export const styleLabel = computed(() => STYLE_LABELS[viewStyle.value])

export const filterInfo = computed(() =>
  filter.value.trim() ? `筛选出 ${filtered.value.length} 台主机` : '无筛选',
)

export function addHost(h: Host) {
  hosts.value.push(h)
}
