<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { writeText } from '@tauri-apps/plugin-clipboard-manager'
import { activeSession, rightPanelTab, sendInput } from '../../stores/session'
import {
  commandHistory, clearCommandHistory, removeCommand,
  type CommandHistoryItem,
} from '../../stores/commandHistory'
import { openSnippetEditor } from '../../stores/snippetDraft'
import { toast } from '../../composables/useToast'
import ContextMenu from './ContextMenu.vue'
import type { MenuItem } from './ContextMenu.vue'

/** 历史按会话隔离：只展示当前活动会话提交过的命令 */
const sid = computed(() => activeSession.value?.id ?? null)

const list = computed<CommandHistoryItem[]>(() => (sid.value ? commandHistory.value[sid.value] ?? [] : []))

const q = ref('')
const filtered = computed(() => {
  const kw = q.value.trim().toLowerCase()
  return kw ? list.value.filter(i => i.command.toLowerCase().includes(kw)) : list.value
})

/** 搜索命中分段：把命令切成「普通/命中」片段供模板渲染（不拼 HTML，天然防注入） */
function highlightSegments(text: string): Array<{ text: string; hit: boolean }> {
  const kw = q.value.trim().toLowerCase()
  if (!kw) return [{ text, hit: false }]
  const lower = text.toLowerCase()
  const out: Array<{ text: string; hit: boolean }> = []
  let from = 0
  while (from < text.length) {
    const at = lower.indexOf(kw, from)
    if (at === -1) {
      out.push({ text: text.slice(from), hit: false })
      break
    }
    if (at > from) out.push({ text: text.slice(from, at), hit: false })
    out.push({ text: text.slice(at, at + kw.length), hit: true })
    from = at + kw.length
  }
  return out
}

function fmtTime(ts: number) {
  const d = new Date(ts)
  const now = new Date()
  const hh = String(d.getHours()).padStart(2, '0')
  const mm = String(d.getMinutes()).padStart(2, '0')
  if (d.toDateString() === now.toDateString()) return `${hh}:${mm}`
  const mo = String(d.getMonth() + 1).padStart(2, '0')
  const dd = String(d.getDate()).padStart(2, '0')
  return `${mo}-${dd} ${hh}:${mm}`
}

function requireSession() {
  if (sid.value) return true
  toast('请先打开一个会话', 'warn', 2000)
  return false
}

/** 当前选中条目的 id：单击只选中，不向终端写入任何内容（避免误触） */
const selectedId = ref<number | null>(null)
watch(sid, () => {
  selectedId.value = null
  // 搜索词属于当前会话视图：切换会话时清空，避免旧关键词过滤新会话造成“无匹配”误解
  q.value = ''
})

function onSelect(item: CommandHistoryItem) {
  selectedId.value = item.id
}

/** 单击：插入到命令行（不回车） */
function onInsert(item: CommandHistoryItem) {
  if (!requireSession()) return
  void sendInput(sid.value!, item.command)
}

/** 双击：直接执行 */
function onRun(item: CommandHistoryItem) {
  if (!requireSession()) return
  void sendInput(sid.value!, item.command + '\r')
}

function onCopy(item: CommandHistoryItem) {
  writeText(item.command)
    .then(() => toast('已复制命令', 'ok', 1200))
    .catch(() => toast('复制失败', 'warn', 1600))
}

/** 收藏为命令片段：用命令首行预填名称，减少手动输入 */
function onSaveSnippet(item: CommandHistoryItem) {
  const firstLine = item.command.split('\n')[0]?.trim() ?? item.command
  const name = firstLine.length > 24 ? `${firstLine.slice(0, 24)}…` : firstLine
  openSnippetEditor({ name, command: item.command })
}

function onClear() {
  if (!sid.value || !list.value.length) return
  clearCommandHistory(sid.value)
  toast('已清空当前会话的历史命令', 'ok', 1600)
}

/* ---- 右键菜单 ---- */
const ICON = {
  run: 'M8 5l11 7-11 7V5z',
  insert: 'M4 6h16M4 12h10M4 18h7',
  copy: 'M9 9h10v10H9zM5 15V5h10',
  star: 'M12 4l2.4 4.9 5.4.8-3.9 3.8.9 5.4-4.8-2.5-4.8 2.5.9-5.4L4.2 9.7l5.4-.8z',
  trash: 'M4 7h16M9 7V5h6v2M7 7l1 13h8l1-13',
}

const menu = ref<{ x: number; y: number; item: CommandHistoryItem } | null>(null)
const menuVisible = computed({
  get: () => menu.value !== null,
  set: v => { if (!v) menu.value = null },
})
const menuX = computed(() => menu.value?.x ?? 0)
const menuY = computed(() => menu.value?.y ?? 0)
const menuItems = computed<MenuItem[]>(() => {
  const m = menu.value
  if (!m) return []
  return [
    { label: '在当前会话执行', icon: ICON.run, action: () => onRun(m.item) },
    { label: '插入到终端', icon: ICON.insert, action: () => onInsert(m.item) },
    { label: '复制命令', icon: ICON.copy, action: () => onCopy(m.item) },
    { label: '收藏为片段', icon: ICON.star, action: () => onSaveSnippet(m.item) },
    { divider: true },
    {
      label: '删除该条',
      icon: ICON.trash,
      action: () => { if (sid.value) removeCommand(sid.value, m.item.id) },
    },
  ]
})

function onContext(e: MouseEvent, item: CommandHistoryItem) {
  menu.value = { x: e.clientX, y: e.clientY, item }
}
</script>

<template>
  <aside class="history-panel">
    <!-- 清空按钮注入右侧面板头部（历史页签激活时） -->
    <Teleport v-if="rightPanelTab === 'history'" defer to="#rp-head-actions">
      <button class="hist-clear" title="清空当前会话历史" :disabled="!list.length" @click="onClear">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round">
          <path :d="ICON.trash" />
        </svg>
      </button>
    </Teleport>

    <div class="search">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
           stroke-linecap="round"><circle cx="11" cy="11" r="7"></circle>
        <line x1="21" y1="21" x2="16.7" y2="16.7"></line></svg>
      <input v-model="q" type="text" placeholder="搜索历史命令…" autocomplete="off"
             autocapitalize="off" autocorrect="off" spellcheck="false">
    </div>

    <div class="history-list">
      <div
        v-for="it in filtered"
        :key="it.id"
        class="history-item"
        :class="{ selected: selectedId === it.id }"
        :title="`${it.command}\n单击选中 · 双击执行 · 右键更多`"
        @click="onSelect(it)"
        @dblclick="onRun(it)"
        @contextmenu.prevent="onContext($event, it)"
      >
        <svg class="history-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor"
             stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="9 6 15 12 9 18" />
        </svg>
        <span class="history-cmd">
          <template v-for="(seg, si) in highlightSegments(it.command)" :key="si">
            <mark v-if="seg.hit" class="text-hit">{{ seg.text }}</mark>
            <template v-else>{{ seg.text }}</template>
          </template>
        </span>
        <span class="history-time">{{ fmtTime(it.timestamp) }}</span>
        <button class="history-act" title="收藏为命令片段" @click.stop="onSaveSnippet(it)">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
               stroke-linecap="round" stroke-linejoin="round">
            <path :d="ICON.star" />
          </svg>
        </button>
      </div>
      <div v-if="!filtered.length" class="history-empty">
        {{ !sid ? '打开一个会话后查看历史命令' : q.trim() ? '无匹配命令' : '暂无历史命令' }}
      </div>
    </div>

    <ContextMenu v-model:visible="menuVisible" :x="menuX" :y="menuY" :items="menuItems" />
  </aside>
</template>

<style scoped>
/* 右侧面板内的整页容器：撑满 panel-body，搜索框沿用全局 .search 的间距 */
.history-panel {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
/* 专注模式面板变宽时：搜索框与列表内容统一限宽 400px 居中，与监控面板布局一致 */
.history-panel > .search {
  width: calc(100% - 24px);
  max-width: 400px;
  margin-left: auto;
  margin-right: auto;
  box-sizing: border-box;
}
/* 头部操作区按钮（与 Inspector 的 ri-icon-btn 同尺寸） */
.hist-clear {
  width: 24px;
  height: 24px;
  display: grid;
  place-items: center;
  color: var(--muted-2);
  background: transparent;
  border: none;
  border-radius: 6px;
  cursor: default;
  transition: background var(--dur-fast) var(--ease), color var(--dur-fast) var(--ease);
}
.hist-clear:hover {
  color: var(--fg-strong);
  background: var(--hover);
}
.hist-clear:disabled {
  color: var(--fg-faint);
}
.hist-clear svg {
  width: 14px;
  height: 14px;
}
.history-list {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 0 12px 12px;
  /* 与 Inspector .ri-body 同构：宽面板下内容限宽居中，窄面板下自动铺满 */
  display: flex;
  flex-direction: column;
  align-items: center;
}
.history-item {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  max-width: 400px;
  box-sizing: border-box;
  flex-shrink: 0;
  padding: 5px 8px;
  border-radius: var(--radius-tab);
  cursor: default;
  transition: background var(--dur-fast) var(--ease);
}
.history-item:hover {
  background: var(--hover-overlay);
}
.history-item.selected {
  background: rgba(var(--accent-rgb), 0.1);
}
.history-item.selected .history-cmd {
  color: var(--fg-strong);
}
.history-icon {
  width: 11px;
  height: 11px;
  flex-shrink: 0;
  color: var(--fg-subtle);
}
.history-cmd {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  font-family: var(--mono);
  font-size: 11px;
  color: var(--text);
  white-space: nowrap;
  text-overflow: ellipsis;
}
.history-time {
  flex-shrink: 0;
  font-size: 9.5px;
  color: var(--fg-faint);
}
/* 行内收藏按钮：默认透明占位，hover 行时淡入，避免布局抖动 */
.history-act {
  flex-shrink: 0;
  width: 18px;
  height: 18px;
  display: grid;
  place-items: center;
  color: var(--fg-subtle);
  background: transparent;
  border: none;
  border-radius: 4px;
  opacity: 0;
  cursor: default;
  transition:
    opacity var(--dur-fast) var(--ease),
    color var(--dur-fast) var(--ease),
    background var(--dur-fast) var(--ease);
}
.history-item:hover .history-act {
  opacity: 1;
}
.history-act:hover {
  color: var(--accent);
  background: var(--hover);
}
.history-act svg {
  width: 12px;
  height: 12px;
}
.history-empty {
  width: 100%;
  max-width: 400px;
  box-sizing: border-box;
  flex-shrink: 0;
  padding: 18px 12px;
  font-size: 11.5px;
  color: var(--fg-faint);
  text-align: center;
}
</style>
