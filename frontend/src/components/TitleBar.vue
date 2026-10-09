<script setup lang="ts">
import { appWindow, detectPlatform } from '../lib/tauri'

// macOS 由原生 Overlay 红绿灯渲染（左侧安全区）；Windows/Linux 前端自绘右侧窗口按钮
const showWinCtrl = detectPlatform() !== 'macos'

async function onClose() {
  ;(await appWindow())?.close()
}
async function onMinimize() {
  ;(await appWindow())?.minimize()
}
async function onMaximize() {
  const w = await appWindow()
  await w?.toggleMaximize()
}
</script>

<template>
  <header class="titlebar" data-tauri-drag-region>
    <h1 data-tauri-drag-region>欢迎访问 <em>Rhost</em></h1>
    <!-- Windows/Linux：前端自绘窗口控制按钮（macOS 由原生 Overlay 红绿灯渲染） -->
    <div v-if="showWinCtrl" class="winctl">
      <button class="wc-btn" title="最小化" @click="onMinimize">
        <svg viewBox="0 0 10 10"><path d="M0 5h10"/></svg>
      </button>
      <button class="wc-btn" title="最大化" @click="onMaximize">
        <svg viewBox="0 0 10 10"><rect x=".5" y=".5" width="9" height="9" fill="none"/></svg>
      </button>
      <button class="wc-btn wc-close" title="关闭" @click="onClose">
        <svg viewBox="0 0 10 10"><path d="M0 0l10 10M10 0L0 10"/></svg>
      </button>
    </div>
  </header>
</template>
