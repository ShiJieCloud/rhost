<script setup lang="ts">
import { computed } from 'vue'
import AppLogo from './AppLogo.vue'
import { APP_NAME, APP_VERSION } from '../lib/appMeta'
import { homeView, setHomeView } from '../stores/homeView'
import { showSettings } from '../stores/settings'
import { appView, sessions } from '../stores/session'
import { HOME_NAV, type HomeNavItem } from '../config/homeNav'

/** 后台活跃会话数：首页据此提示连接仍保持，并提供返回工作台入口 */
const activeCount = computed(() =>
  sessions.value.filter(s => s.state === 'online' || s.state === 'connecting' || s.state === 'reconnecting').length,
)

function backToWorkbench() {
  appView.value = 'workbench'
}

function onNav(item: HomeNavItem) {
  if (!item.enabled) return
  setHomeView(item.id)
}
</script>

<template>
  <aside class="sidebar">
    <div class="app-info">
      <div class="app-icon">
        <AppLogo />
      </div>
      <div class="app-meta">
        <div class="app-name">{{ APP_NAME }}</div>
        <div class="app-ver">{{ APP_VERSION }}</div>
      </div>
    </div>

    <nav class="nav">
      <div class="group-label">连接</div>
      <div
        v-for="item in HOME_NAV"
        :key="item.id"
        class="nav-item"
        :class="{ active: homeView === item.id, disabled: !item.enabled }"
        @click="onNav(item)"
      >
        <span class="ico" v-html="item.icon"></span>
        <span class="label">{{ item.label }}</span>
        <span v-if="item.enabled && item.badge?.()" class="badge">{{ item.badge!() }}</span>
      </div>
    </nav>

    <div class="sidebar-bottom">
      <button
        v-if="activeCount"
        class="session-entry"
        :title="`${activeCount} 个会话进行中，点击返回工作台`"
        @click="backToWorkbench"
      >
        <span class="dot" />
        <span class="session-entry-text">{{ activeCount }} 个会话进行中</span>
      </button>
      <button class="icon-btn" title="偏好设置" @click="showSettings = true">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.6a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>
      </button>
    </div>
  </aside>
</template>
