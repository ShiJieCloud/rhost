<script setup lang="ts">
/** 首页未上线视图的统一占位骨架；功能实现后在 config/homeNav.ts 中替换 component */
defineProps<{
  title: string
  desc?: string
}>()
</script>

<template>
  <div class="home-placeholder">
    <div class="hp-icon">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6"
           stroke-linecap="round" stroke-linejoin="round">
        <rect x="3" y="3" width="18" height="18" rx="3"></rect>
        <line x1="8" y1="12" x2="16" y2="12"></line>
      </svg>
    </div>
    <p class="hp-title">{{ title }}</p>
    <p class="hp-desc">{{ desc || '功能开发中，敬请期待' }}</p>
  </div>
</template>
