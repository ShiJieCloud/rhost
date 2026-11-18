<script setup lang="ts">
import { appWindow, detectPlatform } from '../../lib/tauri'
import AppLogo from '../AppLogo.vue'
import AppLockButton from '../AppLockButton.vue'
import { showNewConn } from '../../stores/hosts'
import { showSettings } from '../../stores/settings'
import { toast } from '../../composables/useToast'
import {
  appView, dockVisible, openDock, rightPanelVisible, searchTick, sidebarVisible, toggleDock,
} from '../../stores/session'

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

function onNew() {
  showNewConn.value = true
}

function goHome() {
  appView.value = 'home'
}

function onSearch() {
  searchTick.value++
}

function onSftp() {
  openDock('sftp')
  toast('已打开 SFTP 文件管理', 'info', 1600)
}

function onPaletteClick() {
  // TODO: 接入全局命令面板
  toast('命令面板开发中', 'info')
}

function onNotify() {
  toast('打开通知中心', 'info')
}

function onSettings() {
  showSettings.value = true
}
</script>

<template>
  <header class="titlebar wb" data-tauri-drag-region>
    <!-- 品牌（点击返回首页；不参与窗口拖拽以接收点击）。
         macOS 下左侧安全区由 CSS 预留，避免与原生 Overlay 红绿灯重叠。 -->
    <div class="logo" title="返回首页" @click="goHome">
      <AppLogo />
      <span>Rhost</span>
    </div>

    <!-- 新建 -->
    <button class="tb-btn primary" title="新建连接" @click="onNew">
      <svg viewBox="0 0 24 24"><path d="M12 5v14M5 12h14"/></svg>
      <span class="btn-text">新建</span>
    </button>

    <!-- 搜索 -->
    <button class="tb-btn tb-icon" title="搜索日志 (⌘F)" @click="onSearch">
      <svg viewBox="0 0 24 24"><circle cx="11" cy="11" r="7"/><path d="M20 20l-3.5-3.5"/></svg>
    </button>

    <!-- SFTP -->
    <button class="tb-btn tb-icon" title="打开 SFTP" @click="onSftp">
      <svg viewBox="0 0 24 24"><path d="M4 7a2 2 0 012-2h3l2 2h7a2 2 0 012 2v8a2 2 0 01-2 2H6a2 2 0 01-2-2V7z"/></svg>
    </button>

    <div class="tb-spacer"></div>

    <!-- 命令面板入口 -->
    <button class="tb-cmdk" title="执行命令…" @click="onPaletteClick">
      <svg viewBox="0 0 24 24"><path d="M9 6l6 6-6 6"/></svg>
      <span class="text">执行命令…</span>
      <span class="kbd"><kbd>⌘</kbd><kbd>K</kbd></span>
    </button>

    <!-- 视图切换组 -->
    <div class="tb-group">
      <button
        class="tb-toggle"
        :class="{ active: sidebarVisible }"
        title="显示 / 隐藏左侧栏"
        @click="sidebarVisible = !sidebarVisible"
      >
        <svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M9 4v16"/></svg>
      </button>
      <button
        class="tb-toggle"
        :class="{ active: dockVisible }"
        title="显示 / 隐藏底部面板"
        @click="toggleDock"
      >
        <svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 15h18"/></svg>
      </button>
      <button
        class="tb-toggle"
        :class="{ active: rightPanelVisible }"
        title="显示 / 隐藏右侧面板"
        @click="rightPanelVisible = !rightPanelVisible"
      >
        <svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M15 4v16"/></svg>
      </button>
    </div>

    <div class="title-actions">
      <!-- 通知 -->
      <button class="icon-btn title-icon-btn" title="通知" @click="onNotify">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 8A6 6 0 006 8c0 7-3 9-3 9h18s-3-2-3-9"/><path d="M13.7 21a2 2 0 01-3.4 0"/></svg>
      </button>

      <!-- 应用锁（与首页一致） -->
      <AppLockButton class="title-lock-btn" />

      <!-- 设置（与首页共用同一 class 与图标，保证尺寸/位置一致） -->
      <button class="icon-btn title-settings-btn" title="设置" @click="onSettings">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.6a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>
      </button>
    </div>

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
