<script setup lang="ts">
/**
 * 应用级锁屏遮罩（app-lock-design.md §6.4）。
 *
 * position:fixed 覆盖整个窗口（z-index:1000，高于所有弹窗 60 / toast 90），
 * 不透明深色底，盖住标题栏以下全部网页内容；挂载即夺焦，只有密码框可交互。
 */
import { computed } from 'vue'
import AppLogo from './AppLogo.vue'
import LockIcon from './LockIcon.vue'
import LockPasswordForm from './LockPasswordForm.vue'
import { appLockReason, appLocked, unlockApp, restoreFocusAfterUnlock } from '../stores/appLock'

const hint = computed(() =>
  appLockReason.value === 'startup'
    ? '输入应用锁密码以继续'
    : appLockReason.value === 'idle'
      ? '闲置超时，应用已锁定'
      : appLockReason.value === 'blur'
        ? '窗口失焦，应用已锁定'
        : '应用已锁定',
)
</script>

<template>
  <Transition name="app-lock" @after-leave="restoreFocusAfterUnlock">
    <div v-if="appLocked" class="app-lock-overlay">
      <div class="lock-card">
        <div class="logo-row">
          <AppLogo class="app-logo" />
          <LockIcon :locked="true" class="badge" />
        </div>
        <div class="lock-title">Rhost 已锁定</div>
        <div class="lock-hint">{{ hint }}</div>
        <LockPasswordForm :submit="unlockApp" />
      </div>
    </div>
  </Transition>
</template>

<style scoped>
.app-lock-overlay {
  position: fixed;
  inset: 0;
  z-index: 1000;
  /* 不透明：禁止毛玻璃，避免透出终端文字 */
  background: var(--bg);
  display: flex;
  align-items: center;
  justify-content: center;
  will-change: opacity;
}
.lock-card {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
  will-change: opacity, transform;
}

/* ---- 解锁过渡（app-lock-design.md §6.4） ----
   分层节奏：背景层整体淡出 280ms；卡片更快（220ms）上浮轻微放大，
   营造「验证通过 → 揭开」的手感。入场 200ms 反向收敛。
   曲线统一用 ease-out（越接近终点越慢），避免机械线性感。 */
.app-lock-enter-active {
  transition: opacity 0.2s ease-out;
}
.app-lock-enter-active .lock-card {
  transition:
    opacity 0.2s ease-out,
    transform 0.2s cubic-bezier(0.22, 1, 0.36, 1);
}
.app-lock-leave-active {
  transition: opacity 0.28s ease-out;
}
.app-lock-leave-active .lock-card {
  transition:
    opacity 0.2s ease-out,
    transform 0.28s cubic-bezier(0.22, 1, 0.36, 1);
}
.app-lock-enter-from,
.app-lock-leave-to {
  opacity: 0;
}
.app-lock-enter-from .lock-card {
  opacity: 0;
  transform: translateY(10px) scale(0.97);
}
.app-lock-leave-to .lock-card {
  opacity: 0;
  transform: translateY(-8px) scale(1.03);
}

@media (prefers-reduced-motion: reduce) {
  .app-lock-enter-active,
  .app-lock-enter-active .lock-card,
  .app-lock-leave-active,
  .app-lock-leave-active .lock-card {
    transition-duration: 0.01ms;
  }
}
.logo-row {
  position: relative;
  width: 52px;
  height: 52px;
  color: var(--accent);
  margin-bottom: 6px;
}
.app-logo {
  width: 52px;
  height: 52px;
  opacity: 0.9;
}
.badge {
  position: absolute;
  right: -6px;
  bottom: -4px;
  font-size: 20px;
  color: var(--accent);
  background: var(--bg);
  border-radius: 50%;
  padding: 2px;
}
.lock-title {
  font-size: 15px;
  font-weight: 600;
  color: var(--text);
}
.lock-hint {
  font-size: 12px;
  color: var(--muted);
  margin-bottom: 6px;
}
</style>
