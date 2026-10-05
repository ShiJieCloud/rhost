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
import { initGlobalErrorReporting, syncLogConfig } from './stores/applog'

// 工作台挂载闩锁：一旦进入过工作台就永久保持挂载（v-show 保活终端状态），
// 即使关闭全部会话停留在空态也不卸载
const workbenchMounted = ref(false)
watch(appView, v => { if (v === 'workbench') workbenchMounted.value = true }, { immediate: true })

// 启动流程：先注册全局错误上报，再把 localStorage 日志配置同步给后端
// （热生效项立即一致；logStoragePath 写入后端配置文件，下次重启生效），
// 然后从后端加载主机配置，最后恢复上次未关闭的会话（热启动）
onMounted(async () => {
  initGlobalErrorReporting()
  syncLogConfig()
  await loadHosts()
  restoreSessions()
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
