<script setup lang="ts">
import { ref } from 'vue'
import { open } from '@tauri-apps/plugin-dialog'
import { toast } from '../composables/useToast'
import { filter, showNewConn } from '../stores/hosts'
import {
  importHosts,
  isEncryptedConfig,
  readImportFile,
  reloadAllAfterImport,
} from '../stores/appConfig'
import { promptPassword } from '../composables/usePasswordPrompt'
import { isTauri } from '../lib/tauri'
import { suppressBlurLock } from '../stores/appLock'

/** 从 JSON 配置文件导入主机（仅合并主机+分组，不动本地设置/密钥/界面状态） */
const importing = ref(false)
async function onImportHosts() {
  if (!isTauri) {
    toast('请在桌面端使用「导入主机」', 'info', 2000)
    return
  }
  // 原生文件对话框会让主窗口收到假失焦事件，抑制失焦锁直到对话框关闭
  const releaseBlur = suppressBlurLock()
  let selected: string | string[] | null
  try {
    selected = await open({
      multiple: false,
      title: '选择要导入的配置文件',
      filters: [{ name: 'JSON 配置文件', extensions: ['json'] }],
    })
  } finally {
    releaseBlur()
  }
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
