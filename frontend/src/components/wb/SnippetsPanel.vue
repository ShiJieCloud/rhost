<script setup lang="ts">
import { computed, ref } from 'vue'
import { openSnippetEditor, openSnippetPreview } from '../../stores/snippetDraft'
import { triggerSnippetAction } from '../../stores/snippetRun'
import { snippets, type Snippet } from '../../stores/snippets'
import { effectiveVars } from '../../lib/snippetVars'

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

/** 折叠的分组名；按设计稿默认折叠低频分组 */
const collapsed = ref<Set<string>>(new Set(['Nginx', '自定义']))

function toggleGroup(name: string) {
  const next = new Set(collapsed.value)
  next.has(name) ? next.delete(name) : next.add(name)
  collapsed.value = next
}

/** 搜索时强制展开，避免命中结果被折叠隐藏 */
const isCollapsed = (name: string) => !q.value.trim() && collapsed.value.has(name)

/** 调用片段：含变量时先弹填充面板，否则直接写入终端 */
function onAction(s: Snippet) {
  triggerSnippetAction({ name: s.name, command: s.command, vars: s.vars })
}

/** 是否含变量，用于在行内标注 */
const hasVars = (s: Snippet) => effectiveVars(s).length > 0
</script>

<template>
  <aside class="sidebar wb-sidebar snippets-panel">
    <div class="snippets-head">
      <span class="snippets-title">命令片段</span>
      <span class="snippets-badge">{{ filtered.length }}</span>
      <span class="spacer"></span>
      <button class="icon-btn" title="新建片段" @click="openSnippetEditor()">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round">
          <line x1="12" y1="5" x2="12" y2="19" />
          <line x1="5" y1="12" x2="19" y2="12" />
        </svg>
      </button>
    </div>

    <div class="search">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
           stroke-linecap="round"><circle cx="11" cy="11" r="7"></circle>
        <line x1="21" y1="21" x2="16.7" y2="16.7"></line></svg>
      <input v-model="q" type="text" placeholder="搜索名称、命令或标签…" autocomplete="off"
             autocapitalize="off" autocorrect="off" spellcheck="false">
      <kbd>⌘K</kbd>
    </div>

    <div class="snippet-list">
      <div v-for="g in groups" :key="g.name" class="snippet-group">
        <button class="snippet-group-head" @click="toggleGroup(g.name)">
          <svg class="chev" :class="{ open: !isCollapsed(g.name) }"
               viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.7"
               stroke-linecap="round" stroke-linejoin="round">
            <path d="M6 4l4 4-4 4" />
          </svg>
          <span class="snippet-group-name">{{ g.name }}</span>
          <span class="snippet-group-count">{{ g.list.length }}</span>
        </button>

        <div v-if="!isCollapsed(g.name)" class="snippet-group-body">
          <div
            v-for="s in g.list"
            :key="s.id"
            class="snippet"
            @click="openSnippetPreview(s)"
          >
            <div class="snippet-main">
              <div class="snippet-name">
                <span class="snippet-name-text">{{ s.name }}</span>
                <span v-if="hasVars(s)" class="snippet-var-mark" title="包含变量占位符">{{ '{ }' }}</span>
              </div>
              <div class="snippet-cmd">{{ s.command }}</div>
            </div>
            <div class="snippet-actions">
              <button class="snippet-act" title="在当前会话执行" @click.stop="onAction(s)">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                     stroke-linecap="round" stroke-linejoin="round">
                  <polygon points="6 4 20 12 6 20 6 4" />
                </svg>
              </button>
            </div>
          </div>
        </div>
      </div>
      <div v-if="!filtered.length" class="snippet-empty">无匹配片段</div>
    </div>

  </aside>
</template>

<style scoped>
.snippets-head {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 0 2px 8px 9px;
}
.snippets-head .spacer {
  flex: 1;
}
.snippets-title {
  font-size: 12.5px;
  font-weight: 500;
  color: var(--fg-strong);
}
.snippets-badge {
  padding: 0 6px;
  font-size: 10px;
  line-height: 15px;
  color: var(--muted-2);
  background: var(--panel-2);
  border-radius: 999px;
}

/* 搜索框右侧快捷键提示 */
.snippets-panel .search input {
  padding-right: 34px;
}
.snippets-panel .search kbd {
  position: absolute;
  top: 50%;
  right: 7px;
  padding: 0 4px;
  font-family: var(--mono);
  font-size: 9.5px;
  line-height: 16px;
  color: var(--fg-faint);
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: var(--radius-control);
  pointer-events: none;
  transform: translateY(-50%);
}

.snippet-list {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding-bottom: 8px;
}

/* 分组头 */
.snippet-group-head {
  width: 100%;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 8px;
  background: transparent;
  border: none;
  border-radius: var(--radius-control);
  font-family: inherit;
  font-size: 11.5px;
  font-weight: 500;
  letter-spacing: 0.03em;
  color: var(--muted-2);
  text-align: left;
  cursor: default;
  transition:
    background var(--dur-fast) var(--ease),
    color var(--dur-fast) var(--ease);
}
.snippet-group-head:hover {
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
.snippet-group-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.snippet-group-count {
  font-size: 10px;
  font-weight: 400;
  color: var(--fg-subtle);
}
.snippet-group-body {
  padding-left: 4px;
}

/* 单条片段 */
.snippet {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 8px;
  border: 1px solid transparent;
  border-radius: var(--radius-tab);
  cursor: default;
  transition:
    background var(--dur-fast) var(--ease),
    border-color var(--dur-fast) var(--ease);
}
.snippet:hover {
  background: var(--hover-overlay);
}
.snippet-main {
  flex: 1;
  min-width: 0;
}
.snippet-name {
  display: flex;
  align-items: center;
  gap: 5px;
  overflow: hidden;
  font-size: 12.5px;
  color: var(--text);
  white-space: nowrap;
}
.snippet-name-text {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
}
/* 含变量占位符的标记 */
.snippet-var-mark {
  flex-shrink: 0;
  font-family: var(--mono);
  font-size: 9px;
  color: var(--accent);
}
.snippet:hover .snippet-name {
  color: var(--fg-strong);
}
.snippet-cmd {
  margin-top: 1px;
  overflow: hidden;
  font-family: var(--mono);
  font-size: 10.5px;
  color: var(--muted-2);
  white-space: nowrap;
  text-overflow: ellipsis;
}
/* 操作按钮常驻占位，仅控制透明度，避免悬浮时行内文字重排抖动 */
.snippet-actions {
  display: flex;
  flex: 0 0 auto;
  gap: 4px;
  opacity: 0;
  transition: opacity var(--dur-fast) var(--ease);
}
.snippet:hover .snippet-actions {
  opacity: 1;
}
.snippet-act {
  display: grid;
  place-items: center;
  width: 24px;
  height: 24px;
  padding: 0;
  color: var(--muted-2);
  background: transparent;
  border: none;
  border-radius: var(--radius-control);
  cursor: default;
  transition:
    background var(--dur-fast) var(--ease),
    color var(--dur-fast) var(--ease);
}
.snippet-act svg {
  width: 14px;
  height: 14px;
}
.snippet-act:hover {
  color: var(--fg-strong);
  background: var(--hover);
}

.snippet-empty {
  padding: 24px 0;
  font-size: 12px;
  color: var(--muted-2);
  text-align: center;
}

</style>
