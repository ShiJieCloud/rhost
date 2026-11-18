<script setup lang="ts">
import WbSideBar from '../components/wb/WbSideBar.vue'
import SnippetsPanel from '../components/wb/SnippetsPanel.vue'
import LeftIconRail from '../components/LeftIconRail.vue'
import WbTabs from '../components/wb/WbTabs.vue'
import TerminalPane from '../components/wb/TerminalPane.vue'
import DockPanel from '../components/wb/DockPanel.vue'
import { dockVisible, leftPanelTab, sidebarVisible } from '../stores/session'
</script>

<template>
  <div class="workbench-root">
    <div class="body">
      <LeftIconRail />
      <WbSideBar v-show="sidebarVisible && leftPanelTab === 'hosts'" />
      <SnippetsPanel v-show="sidebarVisible && leftPanelTab === 'snippets'" />

      <main class="workspace">
        <WbTabs />

        <TerminalPane />

        <DockPanel v-show="dockVisible" />
      </main>
    </div>
  </div>
</template>
