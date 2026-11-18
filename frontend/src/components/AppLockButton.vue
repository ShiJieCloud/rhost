<script setup lang="ts">
/**
 * 应用锁手动上锁按钮（状态栏最左 / 首页标题栏共用）。
 *
 * 仅在应用锁已启用且密码槽位存在时渲染；未锁为低透明度开锁图标，
 * 已锁为强调色闭合锁（此时全屏遮罩已盖住按钮，态切换在解锁瞬间可见）。
 */
import { computed } from 'vue'
import LockIcon from './LockIcon.vue'
import { appLocked, lockApp, lockSlots } from '../stores/appLock'
import { savedSettings } from '../stores/settings'

/** 启用开关打开且密码已设置，锁入口才存在 */
const available = computed(
  () => savedSettings.appLockEnabled && lockSlots.value.appPasswordSet,
)

function onClick() {
  if (!appLocked.value) lockApp('manual')
}
</script>

<template>
  <button
    v-if="available"
    class="app-lock-btn"
    :class="{ locked: appLocked }"
    :title="appLocked ? '应用已锁定' : '锁定应用（⌘/Ctrl+Shift+L）'"
    :aria-pressed="appLocked"
    @click="onClick"
  >
    <LockIcon :locked="appLocked" />
  </button>
</template>

<style scoped>
.app-lock-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  background: transparent;
  border: none;
  padding: 0 4px;
  cursor: pointer;
  color: var(--text);
  font-size: 13px;
  line-height: 1;
  /* 未锁态降低视觉噪音，hover 全显 */
  opacity: 0.6;
}
.app-lock-btn:hover {
  opacity: 1;
}
.app-lock-btn.locked {
  color: var(--accent);
  opacity: 1;
  cursor: default;
}
</style>
