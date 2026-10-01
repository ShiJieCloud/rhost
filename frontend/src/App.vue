<script setup lang="ts">
import { onMounted, ref, watch } from 'vue'
import TitleBar from './components/TitleBar.vue'
import WbTitleBar from './components/wb/WbTitleBar.vue'
import HomeView from './views/HomeView.vue'
import Workbench from './views/Workbench.vue'
import NewConnectionModal from './components/NewConnectionModal.vue'
import SettingsModal from './components/SettingsModal.vue'
import AppToast from './components/AppToast.vue'
import { appView, restoreSessions } from './stores/session'

// 工作台挂载闩锁：一旦进入过工作台就永久保持挂载（v-show 保活终端状态），
// 即使关闭全部会话停留在空态也不卸载
const workbenchMounted = ref(false)
watch(appView, v => { if (v === 'workbench') workbenchMounted.value = true }, { immediate: true })

// 启动恢复：热启动（有未关闭会话）直接进入工作台并自动重连
onMounted(restoreSessions)
</script>

<template>
  <div class="app">
    <TitleBar v-if="appView === 'home'" />
    <WbTitleBar v-else />
    <HomeView v-show="appView === 'home'" />
    <Workbench v-if="workbenchMounted" v-show="appView === 'workbench'" />

    <NewConnectionModal />
    <SettingsModal />
    <AppToast />
  </div>
</template>
