<script setup lang="ts">
import { useToasts } from '../composables/useToast'

const TOAST_ICON: Record<string, string> = {
  ok: '<polyline points="20 6 9 17 4 12"></polyline>',
  info: '<circle cx="12" cy="12" r="10"></circle><line x1="12" y1="16" x2="12" y2="12"></line><line x1="12" y1="8" x2="12.01" y2="8"></line>',
  warn: '<path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"></path><line x1="12" y1="9" x2="12" y2="13"></line><line x1="12" y1="17" x2="12.01" y2="17"></line>',
  err: '<circle cx="12" cy="12" r="10"></circle><line x1="15" y1="9" x2="9" y2="15"></line><line x1="9" y1="9" x2="15" y2="15"></line>',
}

const toasts = useToasts()
</script>

<template>
  <div class="toast-wrap">
    <div
      v-for="t in toasts"
      :key="t.id"
      class="toast"
      :class="[t.type, { out: t.leaving }]"
    >
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor"
           stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
           v-html="TOAST_ICON[t.type] || TOAST_ICON.ok"></svg>
      <span>{{ t.msg }}</span>
    </div>
  </div>
</template>
