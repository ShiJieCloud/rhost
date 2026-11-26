<script setup lang="ts">
import { computed, onUnmounted, ref } from 'vue'
import { openSnippetEditor, openSnippetPreview } from '../../stores/snippetDraft'
import { triggerSnippetAction } from '../../stores/snippetRun'
import { removeSnippet, snippets, type Snippet } from '../../stores/snippets'
import { effectiveVars } from '../../lib/snippetVars'
import { toast } from '../../composables/useToast'

/* ================= 搜索筛选 ================= */
const q = ref('')

const filtered = computed(() => {
  const kw = q.value.trim().toLowerCase()
  if (!kw) return snippets.value
  return snippets.value.filter(s =>
    s.name.toLowerCase().includes(kw) ||
    s.group.toLowerCase().includes(kw) ||
    s.command.toLowerCase().includes(kw) ||
    s.description.toLowerCase().includes(kw),
  )
})

/** 按分组聚合，保留首次出现顺序；空分组名归入「未分组」 */
const groups = computed(() => {
  const map = new Map<string, Snippet[]>()
  filtered.value.forEach(s => {
    const name = s.group.trim() || '未分组'
    const list = map.get(name) ?? []
    list.push(s)
    map.set(name, list)
  })
  return [...map.entries()].map(([name, list]) => ({ name, list }))
})

/* ================= 分组折叠 ================= */
const collapsed = ref<Set<string>>(new Set())

function toggleGroup(name: string) {
  const next = new Set(collapsed.value)
  next.has(name) ? next.delete(name) : next.add(name)
  collapsed.value = next
}

/** 搜索时强制展开，避免命中结果被折叠隐藏 */
const isCollapsed = (name: string) => !q.value.trim() && collapsed.value.has(name)

/* ================= 操作 ================= */
function startCreate() {
  openSnippetEditor()
}

function onAction(s: Snippet) {
  triggerSnippetAction({ name: s.name, command: s.command, vars: s.vars })
}

const hasVars = (s: Snippet) => effectiveVars(s).length > 0

/* ================= 删除：两步内联确认（与主机/分组列表一致） ================= */
const confirmingId = ref<string | null>(null)
let confirmTimer: ReturnType<typeof setTimeout> | null = null

function clearConfirm() {
  confirmingId.value = null
  if (confirmTimer) {
    clearTimeout(confirmTimer)
    confirmTimer = null
  }
}

function onDeleteClick(e: MouseEvent, s: Snippet) {
  e.stopPropagation()
  if (confirmingId.value === s.id) {
    clearConfirm()
    removeSnippet(s.id)
    toast(`已删除片段「${s.name}」`, 'ok', 2000)
    return
  }
  confirmingId.value = s.id
  if (confirmTimer) clearTimeout(confirmTimer)
  confirmTimer = setTimeout(() => {
    confirmingId.value = null
    confirmTimer = null
  }, 2600)
}

onUnmounted(clearConfirm)
</script>

<template>
  <!-- 顶部工具栏：与主机 / 分组页共用全局 toolbar 体系 -->
  <div class="toolbar">
    <div class="search-box">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
           stroke-linecap="round" stroke-linejoin="round">
        <circle cx="11" cy="11" r="7"></circle>
        <line x1="21" y1="21" x2="16.7" y2="16.7"></line>
      </svg>
      <input v-model="q" type="text" spellcheck="false" autocapitalize="off" autocorrect="off" placeholder="搜索名称、命令或分组…" />
    </div>
    <div class="toolbar-actions">
      <button type="button" class="btn primary" @click="startCreate">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4"
             stroke-linecap="round">
          <path d="M12 5v14M5 12h14" />
        </svg>
        新建片段
      </button>
    </div>
  </div>

  <div class="divider"></div>

  <div class="list-meta">
    共 <b>{{ filtered.length }}</b> 个片段
    <span class="sep">·</span>
    <span><b>{{ groups.length }}</b> 个分组</span>
  </div>

  <div class="gp-list">
    <div v-for="g in groups" :key="g.name" class="snip-group">
      <button type="button" class="snip-group-head" @click="toggleGroup(g.name)">
        <svg class="chev" :class="{ open: !isCollapsed(g.name) }" viewBox="0 0 16 16"
             fill="none" stroke="currentColor" stroke-width="1.7"
             stroke-linecap="round" stroke-linejoin="round">
          <path d="M6 4l4 4-4 4" />
        </svg>
        <span class="snip-group-name">{{ g.name }}</span>
        <span class="snip-group-count">{{ g.list.length }}</span>
      </button>

      <div v-show="!isCollapsed(g.name)">
        <div
          v-for="s in g.list"
          :key="s.id"
          class="gp-row snip-row"
          @click="openSnippetPreview(s)"
        >
          <div class="gp-icon">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                 stroke-linecap="round" stroke-linejoin="round">
              <polyline points="4 17 10 11 4 5" />
              <line x1="12" y1="19" x2="20" y2="19" />
            </svg>
          </div>

          <div class="gp-main">
            <div class="gp-name">
              <span class="snip-name-text">{{ s.name }}</span>
              <span v-if="hasVars(s)" class="snip-var-mark" title="包含变量占位符">{{ '{ }' }}</span>
            </div>
            <div class="snip-cmd">{{ s.command }}</div>
          </div>

          <span class="proj-actions" @mouseleave="clearConfirm">
            <button type="button" class="proj-act" title="在当前会话执行"
                    @click.stop="onAction(s)">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                   stroke-linecap="round" stroke-linejoin="round">
                <polygon points="6 4 20 12 6 20 6 4" />
              </svg>
            </button>
            <button
              type="button"
              class="proj-act del"
              :class="{ armed: confirmingId === s.id }"
              :title="confirmingId === s.id ? '再次点击确认删除' : '删除片段'"
              @click="onDeleteClick($event, s)"
            >
              <svg v-if="confirmingId !== s.id" viewBox="0 0 24 24" fill="none"
                   stroke="currentColor" stroke-width="2" stroke-linecap="round"
                   stroke-linejoin="round">
                <polyline points="3 6 5 6 21 6"></polyline>
                <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"></path>
                <path d="M10 11v6M14 11v6"></path>
                <path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2"></path>
              </svg>
              <span v-else class="del-confirm">确认删除</span>
            </button>
          </span>
        </div>
      </div>
    </div>

    <!-- 空状态 -->
    <div v-if="!filtered.length" class="empty-state">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"
           stroke-linecap="round" stroke-linejoin="round">
        <polyline points="4 17 10 11 4 5" />
        <line x1="12" y1="19" x2="20" y2="19" />
      </svg>
      <p>{{ q.trim() ? '没有找到匹配的命令片段' : '还没有命令片段，点击右上角新建' }}</p>
    </div>
  </div>
</template>

<style scoped>
/* 分组头：沿用列表的紧凑节奏，与主机 / 分组页对齐 */
.snip-group-head {
  width: 100%;
  display: flex;
  align-items: center;
  gap: 7px;
  padding: 7px 10px;
  font-family: inherit;
  font-size: 12px;
  font-weight: 500;
  color: var(--muted-2);
  text-align: left;
  background: transparent;
  border: none;
  border-radius: var(--radius-control);
  cursor: default;
  transition:
    background var(--dur-fast) var(--ease),
    color var(--dur-fast) var(--ease);
}
.snip-group-head:hover {
  color: var(--fg-strong);
  background: var(--hover-overlay);
}
.chev {
  width: 12px;
  height: 12px;
  flex-shrink: 0;
  transition: transform var(--dur-fast) var(--ease);
}
.chev.open {
  transform: rotate(90deg);
}
.snip-group-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.snip-group-count {
  font-size: 10.5px;
  font-weight: 400;
  color: var(--fg-subtle);
}

/* 片段行：命令占副标题位置，与分组行的「N 台主机」同层级 */
.snip-row .gp-main {
  min-width: 0;
}
.gp-name {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}
.snip-name-text {
  min-width: 0;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.snip-var-mark {
  flex-shrink: 0;
  font-family: var(--mono);
  font-size: 9.5px;
  color: var(--accent);
}
.snip-cmd {
  margin-top: 2px;
  overflow: hidden;
  font-family: var(--mono);
  font-size: 11px;
  color: var(--muted-2);
  white-space: nowrap;
  text-overflow: ellipsis;
}
</style>
