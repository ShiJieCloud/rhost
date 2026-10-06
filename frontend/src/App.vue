<script setup lang="ts">
import { onMounted, ref, watch } from 'vue'
import TitleBar from './components/TitleBar.vue'
import WbTitleBar from './components/wb/WbTitleBar.vue'
import HomeView from './views/HomeView.vue'
import Workbench from './views/Workbench.vue'
import NewConnectionModal from './components/NewConnectionModal.vue'
import SettingsModal from './components/SettingsModal.vue'
import GroupModal from './components/GroupModal.vue'
import KeysModal from './components/KeysModal.vue'
import PasswordModal from './components/PasswordModal.vue'
import ConfirmModal from './components/ConfirmModal.vue'
import AppToast from './components/AppToast.vue'
import { appView, restoreSessions } from './stores/session'
import { loadHosts } from './stores/hosts'
import { savedSettings } from './stores/settings'
import { loadAppConfig } from './stores/appConfig'
import { initGlobalErrorReporting, syncLogConfig } from './stores/applog'

// 工作台挂载闩锁：一旦进入过工作台就永久保持挂载（v-show 保活终端状态），
// 即使关闭全部会话停留在空态也不卸载
const workbenchMounted = ref(false)
watch(appView, v => { if (v === 'workbench') workbenchMounted.value = true }, { immediate: true })

// Splash 最短展示时长（ms）：设置项 splashDurationMs（200~5000，消费侧夹取防手改配置文件越界），
// 生产构建过快时补齐，避免启动画面一闪而过；呼吸动画不受影响。
// 注意：必须在 await loadAppConfig() hydrate 之后再读取——savedSettings 在此之前是内置默认值，
// 模块同步阶段求值会导致用户设置永远不生效（曾恒为 400ms）。
const SPLASH_MS_MIN = 200
const SPLASH_MS_MAX = 5000
const SPLASH_MS_DEFAULT = 400
function clampSplashMs(v: unknown): number {
  return Math.min(SPLASH_MS_MAX, Math.max(SPLASH_MS_MIN, Number(v) || SPLASH_MS_DEFAULT))
}

// 启动流程：先注册全局错误上报；然后从后端拉取全节配置快照（localStorage 已下线，
// app_config.json 为唯一真相源）并 hydrate 各 store → 下发日志配置（热生效项立即一致；
// logStoragePath 下次重启生效）→ 加载主机配置 → 恢复上次未关闭的会话（热启动）。
// 交接时机：最短展示与启动任务二者取晚完成后撤掉 splash（docs/splash-design.md §5）；
// 其余启动任务挂起不阻塞交接，3s 后强制放行（业务容错自理，见 §6）——
// 该兜底只截断任务等待，不截断用户配置的最短展示时长
onMounted(async () => {
  initGlobalErrorReporting()
  try {
    await loadAppConfig()
  } catch (e) {
    console.error('loadAppConfig 失败，使用内置默认值:', e)
  }
  // hydrate 完成后启动最短展示计时（本地 IPC 仅数十 ms，对计时起点影响可忽略）
  const minDelay = new Promise(r => setTimeout(r, clampSplashMs(savedSettings.splashDurationMs)))
  const restBoot = (async () => {
    syncLogConfig()
    await loadHosts()
    restoreSessions()
  })()
  await Promise.all([
    minDelay,
    Promise.race([
      restBoot,
      new Promise(r => setTimeout(r, 3000)),
    ]),
  ])
  ;(window as any).__hideSplash?.()
})
</script>

<template>
  <div class="app">
    <TitleBar v-if="appView === 'home'" />
    <WbTitleBar v-else />
    <HomeView v-show="appView === 'home'" />
    <Workbench v-if="workbenchMounted" v-show="appView === 'workbench'" />

    <NewConnectionModal />
    <SettingsModal />
    <GroupModal />
    <KeysModal />
    <PasswordModal />
    <ConfirmModal />
    <AppToast />
  </div>
</template>
