<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted } from 'vue'
import Inspector from './wb/Inspector.vue'
import AiChatPanel from './AiChatPanel.vue'
import { appView, activeSession, rightPanelMaximized, rightPanelTab, rightPanelVisible, rightPanelWidth } from '../stores/session'
import { savedSettings } from '../stores/settings'

/** 头部标题：AI 固定；monitor 页签在首页显示「历史会话」，工作台显示「监控」 */
const panelTitle = computed(() => {
  if (rightPanelTab.value === 'ai') return 'AI 助手'
  return appView.value === 'home' ? '历史会话' : '监控'
})

/** AI 对话 scope 标识：工作台显示当前主机名（别名优先回落 host.id），其余为全局 */
const scopeLabel = computed(() => {
  if (appView.value === 'workbench' && activeSession.value) {
    return activeSession.value.alias || activeSession.value.host.id
  }
  return '全局'
})

const railRef = ref<HTMLElement | null>(null)
const resizing = ref(false)

const MIN_W = 280

function onMouseDown(e: MouseEvent) {
  if (!railRef.value) return
  resizing.value = true
  const startX = e.clientX
  const startWidth = rightPanelWidth.value

  const onMove = (ev: MouseEvent) => {
    // 面板在右侧，向左拖宽，向右拖窄；上限为「窗口宽 - 主区最小宽(480) - 图标栏(44)」
    const delta = startX - ev.clientX
    const maxW = Math.max(MIN_W, window.innerWidth - 480 - 44)
    rightPanelWidth.value = Math.min(maxW, Math.max(MIN_W, startWidth + delta))
  }
  const onUp = () => {
    resizing.value = false
    document.removeEventListener('mousemove', onMove)
    document.removeEventListener('mouseup', onUp)
  }
  document.addEventListener('mousemove', onMove)
  document.addEventListener('mouseup', onUp)
}

function collapse() {
  rightPanelVisible.value = false
}

function toggleMaximize() {
  rightPanelMaximized.value = !rightPanelMaximized.value
}

/** ⌘I / Ctrl+I：折叠↔展开并切到 AI 页签；已在 AI 展开态再按则折叠；AI 功能关闭时停用 */
function onKey(e: KeyboardEvent) {
  if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === 'i') {
    if (!savedSettings.aiEnabled) return
    e.preventDefault()
    if (rightPanelVisible.value && rightPanelTab.value === 'ai') {
      rightPanelVisible.value = false
    } else {
      rightPanelTab.value = 'ai'
      rightPanelVisible.value = true
    }
  }
}

onMounted(() => document.addEventListener('keydown', onKey))
onUnmounted(() => document.removeEventListener('keydown', onKey))
</script>

<template>
  <div
    v-show="rightPanelVisible"
    class="right-panel"
    :class="{ maximized: rightPanelMaximized }"
    :style="rightPanelMaximized ? undefined : { width: rightPanelWidth + 'px' }"
  >
    <!-- 拖拽把手（专注模式下禁用；双击快速进入/退出专注） -->
    <div
      v-show="!rightPanelMaximized"
      ref="railRef"
      class="resize-handle"
      :class="{ resizing }"
      title="拖拽调宽 · 双击专注模式"
      @mousedown.prevent="onMouseDown"
      @dblclick.prevent="toggleMaximize"
    />

    <!-- 头部：当前面板标题 + 各面板 Teleport 注入的操作按钮 + 专注/折叠按钮 -->
    <div class="panel-head">
      <div class="panel-title">
        <span class="title-text">{{ panelTitle }}</span>
        <span v-if="rightPanelTab === 'ai'" class="scope-badge" :title="`对话归属：${scopeLabel}`">{{ scopeLabel }}</span>
      </div>
      <div id="rp-head-actions" class="panel-actions" />
      <button
        class="close-btn"
        :class="{ on: rightPanelMaximized }"
        :title="rightPanelMaximized ? '退出专注模式' : '专注模式（面板占满主区）'"
        @click="toggleMaximize"
      >
        <svg v-if="rightPanelMaximized" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor"
             stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M8 3v3a2 2 0 0 1-2 2H3" />
          <path d="M21 8h-3a2 2 0 0 1-2-2V3" />
          <path d="M3 16h3a2 2 0 0 1 2 2v3" />
          <path d="M16 21v-3a2 2 0 0 1 2-2h3" />
        </svg>
        <svg v-else width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor"
             stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M8 3H5a2 2 0 0 0-2 2v3" />
          <path d="M21 8V5a2 2 0 0 0-2-2h-3" />
          <path d="M3 16v3a2 2 0 0 0 2 2h3" />
          <path d="M16 21h3a2 2 0 0 0 2-2v-3" />
        </svg>
      </button>
      <button class="close-btn" title="折叠 (⌘I)" @click="collapse">
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor"
             stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M9 18l6-6-6-6" />
        </svg>
      </button>
    </div>

    <!-- 面板内容 -->
    <div class="panel-body">
      <Inspector v-show="rightPanelTab === 'monitor'" />
      <AiChatPanel v-show="rightPanelTab === 'ai'" />
    </div>
  </div>
</template>

<style scoped>
.right-panel {
  position: relative;
  flex-shrink: 0;
  background: var(--panel);
  border-left: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  min-width: 280px;
  transition: none; /* 拖拽时不加宽度过渡，避免延迟 */
}
/* 专注模式：占满主区（主区内容由 App 层隐藏），宽度不再受限 */
.right-panel.maximized {
  flex: 1;
  min-width: 0;
}

/* 拖拽把手：与 Dock 面板 dock-resizer 同风格（隐形热区 + 中央小把手，hover 变绿加长） */
.resize-handle {
  position: absolute;
  left: -4px;
  top: 0;
  bottom: 0;
  width: 9px;
  cursor: col-resize;
  z-index: 10;
}
.resize-handle::after {
  content: '';
  position: absolute;
  left: 50%;
  top: 50%;
  transform: translate(-50%, -50%);
  width: 2px;
  height: 38px;
  border-radius: 2px;
  background: #1f2b39;
  transition: background 0.15s, height 0.15s;
}
.resize-handle:hover::after,
.resize-handle.resizing::after {
  background: var(--green);
  height: 56px;
}

/* 页签头 */
.panel-head {
  height: 40px;
  padding: 0 8px 0 12px;
  border-bottom: 1px solid var(--border-soft);
  display: flex;
  align-items: center;
  background: var(--panel);
  flex-shrink: 0;
}
.panel-title {
  font-size: 12.5px;
  font-weight: 600;
  color: var(--text);
  /* 窄面板时标题区可收缩（收缩只发生在 scope 徽标上），保住右侧按钮不换行不溢出 */
  flex-shrink: 1;
  /* 标题靠左，后续所有按钮（操作区 + 折叠）整体靠右 */
  margin-right: auto;
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}
.title-text {
  flex-shrink: 0;
}
/* AI 对话归属标识：全局 / 当前主机名。
 * 标题与右侧按钮宽度固定，徽标是唯一弹性元素：宽度 = min(完整名字, 可用空间)，
 * 面板加宽跟随展开、收窄截断（ellipsis），极限压缩靠 min-width: 0 */
.scope-badge {
  flex-shrink: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 10.5px;
  font-weight: 500;
  color: var(--green);
  background: rgba(61, 220, 132, 0.1);
  border: 1px solid rgba(61, 220, 132, 0.25);
  border-radius: 999px;
  padding: 1px 8px;
}
/* 各面板通过 Teleport 注入的操作按钮容器 */
.panel-actions {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
}
.panel-actions:empty {
  display: none;
}
.close-btn {
  width: 26px;
  height: 26px;
  border-radius: 6px;
  display: grid;
  place-items: center;
  background: transparent;
  border: none;
  color: var(--muted-2);
  cursor: pointer;
  transition: background 0.15s, color 0.15s;
  flex-shrink: 0;
  margin-left: 4px;
}
.close-btn:hover {
  background: var(--panel-2);
  color: var(--text);
}
.close-btn.on {
  color: var(--green);
  background: rgba(61, 220, 132, 0.1);
}

/* 面板内容 */
.panel-body {
  flex: 1;
  overflow: hidden;
  display: flex;
  flex-direction: column;
}

/* 窄屏隐藏整个面板（与工作台响应式一致） */
@media (max-width: 1180px) {
  .right-panel {
    display: none;
  }
}
</style>
