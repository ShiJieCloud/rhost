<script setup lang="ts">
import { leftPanelTab, sidebarVisible } from '../stores/session'

/** 与右侧 rail 一致：当前 tab 已展开则折叠，否则切 tab 并展开 */
function onIcon(tab: 'hosts' | 'snippets') {
  if (sidebarVisible.value && leftPanelTab.value === tab) {
    sidebarVisible.value = false
  } else {
    leftPanelTab.value = tab
    sidebarVisible.value = true
  }
}
</script>

<template>
  <aside class="left-rail">
    <button
      class="rail-btn"
      :class="{ on: sidebarVisible && leftPanelTab === 'hosts' }"
      title="主机列表"
      @click="onIcon('hosts')"
    >
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
           stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <rect x="3" y="4" width="18" height="7" rx="1.5" />
        <rect x="3" y="13" width="18" height="7" rx="1.5" />
        <path d="M7 7.5h.01M7 16.5h.01" />
      </svg>
    </button>
    <button
      class="rail-btn"
      :class="{ on: sidebarVisible && leftPanelTab === 'snippets' }"
      title="命令片段"
      @click="onIcon('snippets')"
    >
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
           stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <polyline points="4 17 10 11 4 5" />
        <line x1="12" y1="19" x2="20" y2="19" />
      </svg>
    </button>
  </aside>
</template>

<style scoped>
.left-rail {
  width: 44px;
  flex-shrink: 0;
  background: var(--panel);
  border-right: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  align-items: center;
  padding-top: 10px;
  gap: 6px;
  user-select: none;
}
.rail-btn {
  width: 32px;
  height: 32px;
  border-radius: 8px;
  display: grid;
  place-items: center;
  background: transparent;
  border: 1px solid transparent;
  color: var(--muted-2);
  cursor: default;
  transition: background 0.15s, color 0.15s, border-color 0.15s;
}
.rail-btn:hover {
  background: var(--hover);
  color: var(--fg-strong);
}
.rail-btn:active {
  transform: scale(0.94);
}
.rail-btn.on {
  color: var(--on-accent);
  background: var(--accent);
  border-color: var(--accent);
}
</style>
