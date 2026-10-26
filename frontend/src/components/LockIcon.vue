<script setup lang="ts">
/**
 * 锁状态图标（两态）：locked=true 闭合锁，false 开锁图标。
 * currentColor 着色，尺寸随父级 font-size（1em）。
 */
defineProps<{ locked: boolean }>()
</script>

<template>
  <svg
    v-if="locked"
    class="lock-icon"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.8"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <rect width="18" height="11" x="3" y="11" rx="2" ry="2" />
    <path d="M7 11V7a5 5 0 0 1 10 0v4" />
  </svg>
  <svg
    v-else
    class="lock-icon"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.8"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <rect width="18" height="11" x="3" y="11" rx="2" ry="2" />
    <path d="M7 11V7a5 5 0 0 1 9.9-1" />
  </svg>
</template>

<style scoped>
.lock-icon {
  width: 1em;
  height: 1em;
  display: block;
}
</style>
