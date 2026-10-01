<script setup lang="ts">
import AppLogo from './AppLogo.vue'
import { showSettings } from '../stores/settings'
import { homeView, setHomeView } from '../stores/homeView'
import { HOME_NAV, type HomeNavItem } from '../config/homeNav'

const SETTINGS_SVG =
  '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.6a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>'

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
        <div class="app-name">Rhost</div>
        <div class="app-ver">0.1.0 · 开发版</div>
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

    <div class="sidebar-foot">
      <span class="sidebar-stat" title="SSH 服务就绪 · 本地模式">
        <span class="dot"></span>
        SSH 就绪
      </span>
      <button class="icon-btn" title="偏好设置" @click="showSettings = true">
        <span v-html="SETTINGS_SVG"></span>
      </button>
    </div>
  </aside>
</template>
