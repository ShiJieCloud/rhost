<script setup lang="ts">
/**
 * 锁遮罩共用的密码输入条：回车提交、错误文案、冷却倒计时禁用。
 * 提交逻辑由父级以 submit prop 注入（应用锁 / 会话锁走不同槽位）。
 */
import { nextTick, onMounted, onUnmounted, ref, watch } from 'vue'

const props = defineProps<{
  submit: (password: string) => Promise<void>
  /** 输入框占位文案 */
  placeholder?: string
}>()

const emit = defineEmits<{ success: [] }>()

const pwd = ref('')
const error = ref('')
const busy = ref(false)
const cooldownLeft = ref(0)
const inputEl = ref<HTMLInputElement | null>(null)
const formEl = ref<HTMLFormElement | null>(null)
let cdTimer: number | undefined

watch(error, v => {
  if (!v) return
  // 不重建 DOM（保住输入焦点）：移除类→强制 reflow→加回，重播 shake 动画
  const el = formEl.value
  if (!el) return
  el.classList.remove('shake-once')
  void el.offsetWidth
  el.classList.add('shake-once')
})

/**
 * 等到「下一帧真正绘制完成」再 resolve。
 * 单 RAF 的回调运行在浏览器绘制之前；双 RAF 时第二帧回调已能保证
 * 上一帧的样式变更（spinner / 文案）合成上屏。
 */
function paintCommitted(): Promise<void> {
  return new Promise(resolve => {
    requestAnimationFrame(() => {
      requestAnimationFrame(() => resolve())
    })
  })
}

async function onSubmit() {
  if (busy.value || cooldownLeft.value > 0) return
  if (!pwd.value) return
  busy.value = true
  error.value = ''
  // 关键时序：Vue 的 DOM 更新是微任务批处理，若立刻 await IPC，WebView 会把
  // 「验证中…」首帧绘制推迟到 PBKDF2（约 150~250ms）返回前后，造成「点了没反应」。
  // 依次等：Vue 写完 DOM → 强制提交布局 → 两帧后确认反馈已合成上屏，再发起验证。
  await nextTick()
  void formEl.value?.offsetHeight
  await paintCommitted()
  try {
    await props.submit(pwd.value)
    pwd.value = ''
    emit('success')
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
    // 后端冷却文案「请在 N 秒后重试」→ 本地倒计时禁用输入
    const m = /(\d+)\s*秒后重试/.exec(error.value)
    if (m) startCooldown(Number(m[1]))
  } finally {
    busy.value = false
  }
}

function startCooldown(secs: number) {
  cooldownLeft.value = secs
  if (cdTimer !== undefined) window.clearInterval(cdTimer)
  cdTimer = window.setInterval(() => {
    cooldownLeft.value -= 1
    if (cooldownLeft.value <= 0) {
      window.clearInterval(cdTimer)
      cdTimer = undefined
      error.value = ''
    }
  }, 1000)
}

defineExpose({ focus: () => inputEl.value?.focus() })

onMounted(() => inputEl.value?.focus())
onUnmounted(() => {
  if (cdTimer !== undefined) window.clearInterval(cdTimer)
})
</script>

<template>
  <form ref="formEl" class="lock-form" @submit.prevent="onSubmit">
    <input
      ref="inputEl"
      v-model="pwd"
      type="password"
      class="lock-input"
      :placeholder="cooldownLeft > 0 ? `请在 ${cooldownLeft} 秒后重试` : (placeholder ?? '输入密码解锁')"
      :disabled="busy || cooldownLeft > 0"
      autocomplete="current-password"
      autocapitalize="off"
      autocorrect="off"
      spellcheck="false"
    />
    <button
      type="submit"
      class="lock-submit"
      :class="{ busy }"
      :disabled="busy || cooldownLeft > 0 || !pwd"
    >
      <span v-if="busy" class="spinner" aria-hidden="true"></span>
      {{ busy ? '验证中…' : '解锁' }}
    </button>
    <p v-if="error" class="lock-error">{{ error }}</p>
  </form>
</template>

<style scoped>
.lock-form {
  display: flex;
  flex-direction: column;
  gap: 10px;
  width: 260px;
}
/* 验证失败横向抖动：位移克制（±7px），时长 360ms */
.lock-form.shake-once {
  animation: lock-shake 0.36s cubic-bezier(0.36, 0.07, 0.19, 0.97);
}
@keyframes lock-shake {
  10%, 90% { transform: translateX(-1px); }
  20%, 80% { transform: translateX(2px); }
  30%, 50%, 70% { transform: translateX(-4px); }
  40%, 60% { transform: translateX(4px); }
}
.lock-input {
  width: 100%;
  box-sizing: border-box;
  height: 38px;
  padding: 0 12px;
  border-radius: 9px;
  border: 1px solid var(--border);
  background: var(--panel-2);
  color: var(--text);
  font-size: 13px;
  outline: none;
}
.lock-input:focus {
  border-color: var(--accent);
}
.lock-input:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}
.lock-submit {
  height: 36px;
  border-radius: 9px;
  border: none;
  background: var(--accent);
  color: #04140b;
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 7px;
  /* 高度固定避免 spinner 出现时文案抖动；状态切换有即时反馈感 */
  transition: opacity 0.15s ease, transform 0.08s ease;
}
/* 按压瞬间的物理反馈（鼠标按下 / 键盘回车激活均触发），零 JS 延迟 */
.lock-submit:active:not(:disabled) {
  transform: scale(0.96);
}
.lock-submit:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
/* 验证进行中：保持强调色不暗淡，只换光标 + spinner，弱化等待感 */
.lock-submit.busy {
  opacity: 0.92;
  cursor: progress;
}
.spinner {
  width: 12px;
  height: 12px;
  border-radius: 50%;
  border: 2px solid currentColor;
  border-top-color: transparent;
  animation: lock-spin 0.6s linear infinite;
}
@keyframes lock-spin {
  to { transform: rotate(360deg); }
}
.lock-error {
  margin: 0;
  font-size: 12px;
  color: #ff6b6b;
  text-align: center;
}
</style>
