<script setup lang="ts">
import { ref } from 'vue'
import { open } from '@tauri-apps/plugin-dialog'
import { toast } from '../composables/useToast'
import { filter, showNewConn, viewStyle } from '../stores/hosts'
import {
  importHosts,
  isEncryptedConfig,
  readImportFile,
  reloadAllAfterImport,
} from '../stores/appConfig'
import { promptPassword } from '../composables/usePasswordPrompt'
import { isTauri } from '../lib/tauri'
import type { ViewStyle } from '../types'

const VIEWS: { style: ViewStyle; title: string }[] = [
  { style: 'list', title: '列表视图' },
  { style: 'card', title: '卡片视图' },
  { style: 'grid', title: '网格视图' },
  { style: 'compact', title: '紧凑视图' },
]

const VIEW_SVGS: Record<ViewStyle, string> = {
  list: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="8" y1="6" x2="21" y2="6"/><line x1="8" y1="12" x2="21" y2="12"/><line x1="8" y1="18" x2="21" y2="18"/><line x1="3" y1="6" x2="3.01" y2="6"/><line x1="3" y1="12" x2="3.01" y2="12"/><line x1="3" y1="18" x2="3.01" y2="18"/></svg>',
  card: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/></svg>',
  grid: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="7" rx="1.5"/><rect x="3" y="14" width="18" height="7" rx="1.5"/></svg>',
  compact: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="3" y1="6" x2="21" y2="6"/><line x1="3" y1="10" x2="21" y2="10"/><line x1="3" y1="14" x2="21" y2="14"/><line x1="3" y1="18" x2="21" y2="18"/></svg>',
}

function onSwitch(style: ViewStyle) {
  viewStyle.value = style
  toast(
    `已切换到${{ list: '列表视图', card: '卡片视图', grid: '网格视图', compact: '紧凑视图' }[style]}`,
    'info',
    1400,
  )
}

/** 从 JSON 配置文件导入主机（仅合并主机+分组，不动本地设置/密钥/界面状态） */
const importing = ref(false)
async function onImportHosts() {
  if (!isTauri) {
    toast('请在桌面端使用「导入主机」', 'info', 2000)
    return
  }
  const selected = await open({
    multiple: false,
    title: '选择要导入的配置文件',
    filters: [{ name: 'JSON 配置文件', extensions: ['json'] }],
  })
  if (!selected || typeof selected !== 'string') return
  importing.value = true
  try {
    const file = await readImportFile(selected)
    let password: string | undefined
    if (isEncryptedConfig(file.text)) {
      const pw = await promptPassword(
        '请输入导出时设置的密码',
        '导入配置',
        '输入密码',
      )
      if (pw === null) return
      password = pw
    }
    const s = await importHosts(file.text, file.fileName, password)
    await reloadAllAfterImport()
    toast(
      `导入完成：新增主机 ${s.connectionsAdded} 台，重名改名 ${s.connectionsRenamed} 台，新增分组 ${s.groupsAdded} 个`,
      'ok',
      4000,
    )
  } catch (e) {
    toast(`导入失败：${String(e)}`, 'err', 5000)
  } finally {
    importing.value = false
  }
}
</script>

<template>
  <div>
    <div class="toolbar">
      <div class="search-box">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round"><circle cx="11" cy="11" r="7"></circle>
          <line x1="21" y1="21" x2="16.7" y2="16.7"></line></svg>
        <input v-model="filter" type="text" placeholder="搜索主机、IP 或用户…" autocomplete="off">
      </div>

      <div class="view-switch">
        <button
          v-for="v in VIEWS"
          :key="v.style"
          class="vs-btn"
          :class="{ active: viewStyle === v.style }"
          :title="v.title"
          @click="onSwitch(v.style)"
        >
          <span v-html="VIEW_SVGS[v.style]"></span>
        </button>
      </div>

      <div class="toolbar-actions">
        <button class="btn primary" @click="showNewConn = true">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.3"
               stroke-linecap="round"><line x1="12" y1="5" x2="12" y2="19"></line>
            <line x1="5" y1="12" x2="19" y2="12"></line></svg>
          <span>新建连接</span>
        </button>
        <button class="btn" :disabled="importing" @click="onImportHosts">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
               stroke-linecap="round" stroke-linejoin="round">
            <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path>
            <polyline points="7 10 12 15 17 10"></polyline>
            <line x1="12" y1="15" x2="12" y2="3"></line>
          </svg>
          <span>{{ importing ? '导入中…' : '导入主机' }}</span>
        </button>
      </div>
    </div>
  </div>
</template>
