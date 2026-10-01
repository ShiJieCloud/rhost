import { ref, watch } from 'vue'

/** 首页主区可切换的视图；新增页面时在此扩展联合类型 */
export type HomeViewId = 'quick-connect' | 'hosts' | 'groups' | 'keys' | 'logs'

/** 模块级单例状态；后续接 Pinia 或 Tauri 后端时仅需替换此处 */
const VIEW_KEY = 'rhost.homeView'
const VIEW_IDS: HomeViewId[] = ['quick-connect', 'hosts', 'groups', 'keys', 'logs']

function restoreView(): HomeViewId {
  try {
    const saved = localStorage.getItem(VIEW_KEY) as HomeViewId | null
    if (saved && VIEW_IDS.includes(saved)) return saved
  } catch { /* 隐私模式等场景按默认页处理 */ }
  return 'quick-connect'
}

export const homeView = ref<HomeViewId>(restoreView())

export function setHomeView(id: HomeViewId) {
  homeView.value = id
}

watch(homeView, v => {
  try {
    localStorage.setItem(VIEW_KEY, v)
  } catch { /* 持久化失败不影响切换 */ }
})
