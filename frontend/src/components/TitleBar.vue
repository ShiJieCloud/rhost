<script setup lang="ts">
import { appWindow } from '../lib/tauri'

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
    <div class="traffic">
      <i class="t-red" title="关闭" @click="onClose"></i>
      <i class="t-yellow" title="最小化" @click="onMinimize"></i>
      <i class="t-green" title="最大化" @click="onMaximize"></i>
    </div>
    <h1 data-tauri-drag-region>欢迎访问 <em>Rhost</em></h1>
  </header>
</template>
