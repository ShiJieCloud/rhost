<script setup lang="ts">
import { nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { passwordPromptState, submitPassword, cancelPassword } from '../composables/usePasswordPrompt'

const { visible, message, title, placeholder } = passwordPromptState
const value = ref('')
const shown = ref(false)
const inputEl = ref<HTMLInputElement | null>(null)

watch(visible, v => {
  if (!v) return
  value.value = ''
  shown.value = false
  nextTick(() => setTimeout(() => inputEl.value?.focus(), 120))
})

function onSubmit() {
  submitPassword(value.value)
}

function onKey(e: KeyboardEvent) {
  if (!visible.value) return
  if (e.key === 'Escape') {
    e.preventDefault()
    cancelPassword()
  } else if (e.key === 'Enter') {
    e.preventDefault()
    onSubmit()
  }
}

onMounted(() => document.addEventListener('keydown', onKey))
onUnmounted(() => document.removeEventListener('keydown', onKey))
</script>

<template>
  <div class="mask" :class="{ show: visible }" @click.self="cancelPassword">
    <div class="modal pw-modal" role="dialog" aria-modal="true" aria-labelledby="pwTitle">
      <div class="modal-head">
        <div class="modal-icon">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
               stroke-linecap="round" stroke-linejoin="round">
            <rect x="4" y="11" width="16" height="10" rx="2" />
            <path d="M8 11V7a4 4 0 0 1 8 0v4" />
          </svg>
        </div>
        <div class="modal-title">
          <h2 id="pwTitle">{{ title }}</h2>
          <p>{{ message }}</p>
        </div>
        <button class="modal-close" title="关闭" @click="cancelPassword">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
               stroke-linecap="round">
            <line x1="18" y1="6" x2="6" y2="18" />
            <line x1="6" y1="6" x2="18" y2="18" />
          </svg>
        </button>
      </div>

      <div class="pw-body">
        <div class="pw-wrap">
          <input
            ref="inputEl"
            v-model="value"
            :type="shown ? 'text' : 'password'"
            :placeholder="placeholder"
            autocomplete="new-password"
            autocapitalize="off"
            autocorrect="off"
            spellcheck="false"
            @keydown.enter.prevent="onSubmit"
          />
          <button
            type="button"
            class="pw-toggle"
            :class="{ on: shown }"
            :title="shown ? '隐藏密码' : '显示密码'"
            @click="shown = !shown"
          >
            <svg v-if="shown" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                 stroke-linecap="round" stroke-linejoin="round">
              <path d="M1 12s4-7 11-7 11 7 11 7-4 7-11 7S1 12 1 12z" />
              <circle cx="12" cy="12" r="3" />
            </svg>
            <svg v-else viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                 stroke-linecap="round" stroke-linejoin="round">
              <path d="M17.94 17.94A10.94 10.94 0 0 1 12 20c-7 0-11-8-11-8a18.5 18.5 0 0 1 5.06-5.94" />
              <path d="M9.9 4.24A10.94 10.94 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19" />
              <path d="M14.12 14.12A3 3 0 1 1 9.88 9.88" />
              <line x1="2" y1="2" x2="22" y2="22" />
            </svg>
          </button>
        </div>
      </div>

      <div class="modal-foot">
        <span class="pw-hint"><kbd>Enter</kbd> 确定 · <kbd>Esc</kbd> 取消</span>
        <span class="spacer"></span>
        <button type="button" class="btn ghost" @click="cancelPassword">取消</button>
        <button type="button" class="btn primary" @click="onSubmit">确定</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 全局 .modal 固定 820×640，密码弹窗内容少，按内容自适应 */
.pw-modal {
  width: 400px;
  height: auto;
  max-height: calc(100vh - 60px);
}
/* 提示信息（user@host）可能较长，允许换行 */
.pw-modal .modal-title p {
  word-break: break-all;
  line-height: 1.5;
}
.pw-body {
  padding: 16px 18px;
}
.pw-wrap {
  position: relative;
  display: flex;
  align-items: center;
}
.pw-wrap input {
  width: 100%;
  height: 40px;
  padding: 0 42px 0 12px;
  border-radius: 8px;
  border: 1px solid var(--border);
  background: var(--input-bg);
  color: var(--text);
  font-size: 14px;
  outline: none;
  transition: border-color .15s, box-shadow .15s;
}
.pw-wrap input:focus {
  border-color: var(--green);
  box-shadow: 0 0 0 3px rgba(61, 220, 132, .18);
}
.pw-toggle {
  position: absolute;
  right: 6px;
  width: 30px;
  height: 30px;
  border: none;
  background: transparent;
  color: var(--text-3);
  cursor: pointer;
  display: grid;
  place-items: center;
  border-radius: 6px;
}
.pw-toggle:hover { color: var(--text); background: var(--hover-bg) }
.pw-toggle svg { width: 18px; height: 18px }
.pw-hint { color: var(--text-3); font-size: 12px }
</style>
