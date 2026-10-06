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
import AppToast from './components/AppToast.vue'
import { appView, restoreSessions } from './stores/session'
import { loadHosts } from './stores/hosts'
import { savedSettings } from './stores/settings'
import { initGlobalErrorReporting, syncLogConfig } from './stores/applog'

// 工作台挂载闩锁：一旦进入过工作台就永久保持挂载（v-show 保活终端状态），
// 即使关闭全部会话停留在空态也不卸载
const workbenchMounted = ref(false)
watch(appView, v => { if (v === 'workbench') workbenchMounted.value = true }, { immediate: true })

// Splash 最短展示时长（ms）：设置项 splashDurationMs（200~5000，消费侧夹取防手改 localStorage），
// 生产构建过快时补齐，避免启动画面一闪而过；呼吸动画不受影响
const MIN_SPLASH_MS = Math.min(5000, Math.max(200, Number(savedSettings.splashDurationMs) || 400))

// 启动流程：先注册全局错误上报，再把 localStorage 日志配置同步给后端
// （热生效项立即一致；logStoragePath 写入后端配置文件，下次重启生效），
// 然后从后端加载主机配置，最后恢复上次未关闭的会话（热启动）。
// 交接时机：最短展示与启动任务二者取晚完成后撤掉 splash（docs/splash-design.md §5）；
// boot 挂起不阻塞交接，3s 后强制放行（业务容错自理，见 §6）
onMounted(async () => {
  const minDelay = new Promise(r => setTimeout(r, MIN_SPLASH_MS))
  const boot = (async () => {
    initGlobalErrorReporting()
    syncLogConfig()
    await loadHosts()
    restoreSessions()
  })()
  await Promise.race([
    Promise.all([minDelay, boot]),
    new Promise(r => setTimeout(r, 3000)),
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
    <AppToast />
  </div>
</template>
