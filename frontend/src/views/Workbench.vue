<script setup lang="ts">
import WbSideBar from '../components/wb/WbSideBar.vue'
import WbTabs from '../components/wb/WbTabs.vue'
import TerminalPane from '../components/wb/TerminalPane.vue'
import DockPanel from '../components/wb/DockPanel.vue'
import Inspector from '../components/wb/Inspector.vue'
import StatusBar from '../components/wb/StatusBar.vue'
import { dockVisible, inspectorVisible, sidebarVisible } from '../stores/session'
</script>

<template>
  <div class="workbench-root">
    <div class="body">
      <WbSideBar v-show="sidebarVisible" />

      <main class="workspace">
        <WbTabs />

        <TerminalPane />

        <DockPanel v-show="dockVisible" />
      </main>

      <Inspector v-show="inspectorVisible" />
    </div>

    <StatusBar />
  </div>
</template>
