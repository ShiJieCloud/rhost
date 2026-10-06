<script setup lang="ts">
import { onMounted, onUnmounted } from 'vue'
import {
  acceptConfirm,
  confirmState,
  rejectConfirm,
} from '../composables/useConfirm'

const { visible, message, title, confirmText, cancelText, danger } = confirmState

function onKey(e: KeyboardEvent) {
  if (!visible.value) return
  if (e.key === 'Escape') {
    e.preventDefault()
    rejectConfirm()
  } else if (e.key === 'Enter') {
    e.preventDefault()
    acceptConfirm()
  }
}

onMounted(() => document.addEventListener('keydown', onKey))
onUnmounted(() => document.removeEventListener('keydown', onKey))
</script>

<template>
  <div class="mask" :class="{ show: visible }" @click.self="rejectConfirm">
    <div class="modal cf-modal" role="alertdialog" aria-modal="true" aria-labelledby="cfTitle">
      <div class="modal-head">
        <div class="modal-icon">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
               stroke-linecap="round" stroke-linejoin="round">
            <path d="M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
            <line x1="12" y1="9" x2="12" y2="13" />
            <line x1="12" y1="17" x2="12.01" y2="17" />
          </svg>
        </div>
        <div class="modal-title">
          <h2 id="cfTitle">{{ title }}</h2>
          <p>{{ message }}</p>
        </div>
        <button class="modal-close" title="关闭" @click="rejectConfirm">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
               stroke-linecap="round">
            <line x1="18" y1="6" x2="6" y2="18" />
            <line x1="6" y1="6" x2="18" y2="18" />
          </svg>
        </button>
      </div>

      <div class="modal-foot cf-foot">
        <span class="cf-hint"><kbd>Enter</kbd> 确定 · <kbd>Esc</kbd> 取消</span>
        <span class="spacer"></span>
        <button type="button" class="btn ghost" @click="rejectConfirm">{{ cancelText }}</button>
        <button type="button" class="btn" :class="danger ? 'danger' : 'primary'" @click="acceptConfirm">
          {{ confirmText }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 全局 .modal 固定 820×640，确认弹窗内容少，按内容自适应 */
.cf-modal {
  width: 420px;
  height: auto;
  max-height: calc(100vh - 60px);
}
.cf-modal .modal-title p {
  white-space: pre-line;
  line-height: 1.6;
}
.cf-foot {
  padding: 14px 18px 16px;
}
.cf-hint {
  color: var(--text-3);
  font-size: 12px;
}
</style>
