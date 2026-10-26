<script setup lang="ts">
/**
 * 单会话锁遮罩（app-lock-design.md §6.5）。
 *
 * absolute 覆盖终端区（不盖标签栏与状态栏），不透明；仅在所属会话锁定且
 * 被渲染（激活终端）时出现。切到锁定标签时自动聚焦密码框。
 */
import { computed, onMounted, ref, watch } from 'vue'
import LockIcon from '../LockIcon.vue'
import LockPasswordForm from '../LockPasswordForm.vue'
import { activeSessionId, sessions, unlockSession, sessionLockFocusReq } from '../../stores/session'

const props = defineProps<{ sessionId: string }>()

const session = computed(
  () => sessions.value.find(s => s.id === props.sessionId) ?? null,
)
const locked = computed(() => session.value?.locked === true)
const isActive = computed(() => activeSessionId.value === props.sessionId)

const formRef = ref<InstanceType<typeof LockPasswordForm> | null>(null)

function submit(pwd: string) {
  return unlockSession(props.sessionId, pwd)
}

onMounted(() => {
  if (locked.value && isActive.value) formRef.value?.focus()
})
watch(
  () => [locked.value, isActive.value],
  ([l, a]) => {
    if (l && a) formRef.value?.focus()
  },
)
// 已锁标签图标点击（标签可能已是激活态，上面的 watch 不触发）
watch(sessionLockFocusReq, req => {
  if (req.id === props.sessionId && locked.value && isActive.value) formRef.value?.focus()
})
</script>

<template>
  <Transition name="session-lock">
    <div v-if="locked" class="session-lock-overlay">
      <div class="session-lock-card">
        <LockIcon :locked="true" class="session-lock-icon" />
        <div class="session-lock-title">此会话已锁定</div>
        <div class="session-lock-sub">输入密码解锁终端</div>
        <LockPasswordForm ref="formRef" :submit="submit" />
      </div>
    </div>
  </Transition>
</template>

<style scoped>
.session-lock-overlay {
  position: absolute;
  inset: 0;
  z-index: 20;
  /* 不透明：遮住终端历史输出，防旁观 */
  background: var(--bg);
  display: flex;
  align-items: center;
  justify-content: center;
  will-change: opacity;
}
.session-lock-card {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  will-change: opacity, transform;
}

/* ---- 解锁过渡：与应用锁同款语言，局部遮罩节奏更短（220/160ms） ---- */
.session-lock-enter-active {
  transition: opacity 0.16s ease-out;
}
.session-lock-enter-active .session-lock-card {
  transition:
    opacity 0.16s ease-out,
    transform 0.16s cubic-bezier(0.22, 1, 0.36, 1);
}
.session-lock-leave-active {
  transition: opacity 0.22s ease-out;
}
.session-lock-leave-active .session-lock-card {
  transition:
    opacity 0.16s ease-out,
    transform 0.22s cubic-bezier(0.22, 1, 0.36, 1);
}
.session-lock-enter-from,
.session-lock-leave-to {
  opacity: 0;
}
.session-lock-enter-from .session-lock-card {
  opacity: 0;
  transform: translateY(8px) scale(0.97);
}
.session-lock-leave-to .session-lock-card {
  opacity: 0;
  transform: translateY(-6px) scale(1.03);
}

@media (prefers-reduced-motion: reduce) {
  .session-lock-enter-active,
  .session-lock-enter-active .session-lock-card,
  .session-lock-leave-active,
  .session-lock-leave-active .session-lock-card {
    transition-duration: 0.01ms;
  }
}
.session-lock-icon {
  font-size: 30px;
  color: var(--accent);
  margin-bottom: 4px;
}
.session-lock-title {
  font-size: 14px;
  font-weight: 600;
  color: var(--text);
}
.session-lock-sub {
  font-size: 12px;
  color: var(--muted);
  margin-bottom: 4px;
}
</style>
