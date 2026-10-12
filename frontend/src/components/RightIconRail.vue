<script setup lang="ts">
import { computed, watch } from 'vue'
import { appView, rightPanelTab, rightPanelVisible } from '../stores/session'
import { savedSettings } from '../stores/settings'

const isHome = computed(() => appView.value === 'home')
const aiEnabled = computed(() => savedSettings.aiEnabled)

/** AI 关闭时若正停留在 AI 页签，折叠面板并回退到监控页签 */
watch(aiEnabled, on => {
  if (!on && rightPanelTab.value === 'ai') {
    rightPanelTab.value = 'monitor'
    rightPanelVisible.value = false
  }
})

/** 点击图标：面板折叠→展开并切到对应页签；当前页签激活中→折叠；否则仅切换页签 */
function onIcon(tab: 'monitor' | 'ai') {
  if (rightPanelVisible.value && rightPanelTab.value === tab) {
    rightPanelVisible.value = false
  } else {
    rightPanelTab.value = tab
    rightPanelVisible.value = true
  }
}
</script>

<template>
  <aside class="icon-rail">
    <button
      class="rail-btn"
      :class="{ on: rightPanelVisible && rightPanelTab === 'monitor' }"
      :title="isHome ? '历史会话' : '监控'"
      @click="onIcon('monitor')"
    >
      <!-- 首页：历史会话（时钟）；工作台：监控（波形） -->
      <svg v-if="isHome" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
           stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <circle cx="12" cy="12" r="9" />
        <polyline points="12 7 12 12 15.5 14" />
      </svg>
      <svg v-else width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
           stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M22 12h-4l-3 9L9 3l-3 9H2" />
      </svg>
    </button>
    <button
      v-if="aiEnabled"
      class="rail-btn"
      :class="{ on: rightPanelVisible && rightPanelTab === 'ai' }"
      title="AI 助手 (⌘I)"
      @click="onIcon('ai')"
    >
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
           stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M12 3l1.9 5.6L19.5 10l-5.6 1.9L12 17.5l-1.9-5.6L4.5 10l5.6-1.4z" />
        <path d="M19 15l.9 2.6 2.6.9-2.6.9-.9 2.6-.9-2.6-2.6-.9 2.6-.9z" />
      </svg>
    </button>
  </aside>
</template>

<style scoped>
.icon-rail {
  width: 44px;
  flex-shrink: 0;
  background: var(--panel);
  border-left: 1px solid var(--border);
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
  cursor: pointer;
  transition: background 0.15s, color 0.15s, border-color 0.15s;
}
.rail-btn:hover {
  background: var(--panel-2);
  border-color: var(--border);
  color: var(--text);
}
.rail-btn:active {
  transform: scale(0.94);
}
.rail-btn.on {
  color: var(--green);
  background: rgba(61, 220, 132, 0.1);
  border-color: rgba(61, 220, 132, 0.35);
}

/* 与工作台响应式一致：窄屏隐藏右侧栏整体（面板在同断点隐藏，见 RightPanel） */
@media (max-width: 1180px) {
  .icon-rail {
    display: none;
  }
}
</style>
