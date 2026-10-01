<script setup lang="ts">
import { appWindow } from '../../lib/tauri'
import AppLogo from '../AppLogo.vue'
import { showNewConn } from '../../stores/hosts'
import { showSettings } from '../../stores/settings'
import { toast } from '../../composables/useToast'
import {
  appView, dockVisible, inspectorVisible, openDock, searchTick, sidebarVisible, toggleDock,
} from '../../stores/session'

async function onClose() {
  ;(await appWindow())?.close()
}
async function onMinimize() {
  ;(await appWindow())?.minimize()
}
async function onMaximize() {
  ;(await appWindow())?.toggleMaximize()
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
    <!-- macOS 交通灯 -->
    <div class="traffic">
      <i class="t-red" title="关闭" @click="onClose"></i>
      <i class="t-yellow" title="最小化" @click="onMinimize"></i>
      <i class="t-green" title="最大化" @click="onMaximize"></i>
    </div>

    <!-- 品牌（点击返回首页；不参与窗口拖拽以接收点击） -->
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
    <button class="tb-btn" title="搜索日志 (⌘F)" @click="onSearch">
      <svg viewBox="0 0 24 24"><circle cx="11" cy="11" r="7"/><path d="M20 20l-3.5-3.5"/></svg>
    </button>

    <!-- SFTP -->
    <button class="tb-btn" title="打开 SFTP" @click="onSftp">
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
        :class="{ active: inspectorVisible }"
        title="显示 / 隐藏右侧栏"
        @click="inspectorVisible = !inspectorVisible"
      >
        <svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M15 4v16"/></svg>
      </button>
    </div>

    <!-- 通知 -->
    <button class="tb-btn" title="通知" @click="onNotify">
      <svg viewBox="0 0 24 24"><path d="M18 8A6 6 0 006 8c0 7-3 9-3 9h18s-3-2-3-9"/><path d="M13.7 21a2 2 0 01-3.4 0"/></svg>
      <span class="badge" style="display:none"></span>
    </button>

    <!-- 设置 -->
    <button class="tb-btn" title="设置" @click="onSettings">
      <svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 00.3 1.9l.1.1a2 2 0 11-2.8 2.8l-.1-.1a1.7 1.7 0 00-1.9-.3 1.7 1.7 0 00-1 1.5V21a2 2 0 11-4 0v-.1a1.7 1.7 0 00-1.1-1.5 1.7 1.7 0 00-1.9.3l-.1.1a2 2 0 11-2.8-2.8l.1-.1a1.7 1.7 0 00.3-1.9 1.7 1.7 0 00-1.5-1H3a2 2 0 110-4h.1a1.7 1.7 0 001.5-1.1 1.7 1.7 0 00-.3-1.9l-.1-.1a2 2 0 112.8-2.8l.1.1a1.7 1.7 0 001.9.3h.1a1.7 1.7 0 001-1.5V3a2 2 0 114 0v.1a1.7 1.7 0 001 1.5 1.7 1.7 0 001.9-.3l.1-.1a2 2 0 112.8 2.8l-.1.1a1.7 1.7 0 00-.3 1.9v.1a1.7 1.7 0 001.5 1H21a2 2 0 110 4h-.1a1.7 1.7 0 00-1.5 1z"/></svg>
    </button>
  </header>
</template>
