<script setup lang="ts">
import { computed, onMounted, onUnmounted, watch } from 'vue'
import {
  closeSnippetEditor,
  editSnippetDraft,
  snippetDraft,
  snippetMode,
} from '../stores/snippetDraft'
import { triggerSnippetAction } from '../stores/snippetRun'
import { addSnippet, updateSnippet } from '../stores/snippets'
import {
  copyToClipboard,
  defaultVar,
  extractVars,
  splitCommand,
  varToken,
} from '../lib/snippetVars'
import { toast } from '../composables/useToast'

const isPreview = computed(() => snippetMode.value === 'preview')
const vars = computed(() => snippetDraft.value?.vars ?? [])
/** 命令切分结果，预览时高亮变量占位符 */
const commandParts = computed(() => (snippetDraft.value ? splitCommand(snippetDraft.value.command) : []))

/**
 * 命令变化时同步变量声明：命令里新出现的占位符追加默认声明，已删除的占位符
 * 连同声明移除。用户手动移除的声明不会因为命令其他位置的变化而被补回。
 */
watch(
  () => snippetDraft.value?.command,
  (next, prev) => {
    const d = snippetDraft.value
    // 模态框刚打开时 prev 为 undefined，此时以草稿自带的声明为准
    if (!d || prev === undefined || next === undefined) return
    const nextNames = extractVars(next)
    const prevNames = new Set(extractVars(prev))
    const nextSet = new Set(nextNames)
    d.vars = d.vars.filter(v => nextSet.has(v.name))
    for (const name of nextNames) {
      if (!prevNames.has(name) && !d.vars.some(v => v.name === name)) {
        d.vars.push(defaultVar(name))
      }
    }
  },
)

/** 移除声明：该占位符在渲染时原样保留 */
function removeVar(name: string) {
  const d = snippetDraft.value
  if (!d) return
  d.vars = d.vars.filter(v => v.name !== name)
}

/** 保存：新建或更新已有片段，持久化由 snippets store 的 watch 写穿 */
function onSave() {
  const d = snippetDraft.value
  if (!d) return
  const name = d.name.trim()
  if (!name) {
    toast('请填写片段名称', 'err', 1800)
    return
  }
  if (!d.command.trim()) {
    toast('请填写命令内容', 'err', 1800)
    return
  }
  const input = {
    name,
    group: d.group.trim(),
    command: d.command,
    description: d.description.trim(),
    vars: d.vars.map(v => ({ ...v, options: [...v.options] })),
  }
  if (d.id) updateSnippet(d.id, input)
  else addSnippet(input)
  closeSnippetEditor()
  toast(d.id ? '片段已更新' : '片段已保存', 'ok', 1600)
}

/** 删除统一收在列表行内，预览弹窗不再提供入口 */

function onCopy() {
  const cmd = snippetDraft.value?.command
  if (!cmd) return
  void copyToClipboard(cmd).then(ok => {
    toast(ok ? '已复制命令' : '复制失败，请检查剪贴板权限', ok ? 'ok' : 'err', 1800)
  })
}

function onAction() {
  const d = snippetDraft.value
  if (!d) return
  const varDefs = d.vars.map(v => ({ ...v, options: [...v.options] }))
  closeSnippetEditor()
  triggerSnippetAction({ name: d.name, command: d.command, vars: varDefs })
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape' && snippetDraft.value) closeSnippetEditor()
}

onMounted(() => document.addEventListener('keydown', onKeydown))
onUnmounted(() => document.removeEventListener('keydown', onKeydown))
</script>

<template>
  <div
    v-if="snippetDraft"
    class="mask show"
    @click.self="closeSnippetEditor"
  >
    <div class="modal snippet-modal" role="dialog" aria-modal="true">
      <div class="modal-head">
        <div class="modal-icon">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
               stroke-linecap="round" stroke-linejoin="round">
            <polyline points="4 17 10 11 4 5" />
            <line x1="12" y1="19" x2="20" y2="19" />
          </svg>
        </div>
        <div class="modal-title">
          <h2>{{ isPreview ? '片段预览' : '命令片段' }}</h2>
          <p>{{ isPreview ? '查看完整命令与变量，可直接复制或调用' : '保存常用命令，在终端中快速调用' }}</p>
        </div>
        <button class="modal-close" title="关闭" @click="closeSnippetEditor">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
               stroke-linecap="round">
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </button>
      </div>

      <!-- 预览：只读展示完整命令，变量高亮 -->
      <div v-if="isPreview" class="snippet-body preview-body">
        <div class="preview-meta">
          <span class="preview-name">{{ snippetDraft.name || '未命名片段' }}</span>
          <span v-if="snippetDraft.group" class="preview-group">{{ snippetDraft.group }}</span>
        </div>

        <div class="field">
          <label>命令</label>
          <pre class="preview-command"><template
            v-for="(p, i) in commandParts"
            :key="i"
          ><span :class="{ 'preview-var': p.var }">{{ p.text }}</span></template></pre>
        </div>

        <div v-if="vars.length" class="preview-vars">
          <span class="preview-vars-label">变量</span>
          <span
            v-for="v in vars"
            :key="v.name"
            class="preview-var-chip"
            :title="v.defaultValue ? `默认值：${v.defaultValue}` : v.label"
          >{{ v.name }}</span>
        </div>

        <div v-if="snippetDraft.description" class="field">
          <label>说明</label>
          <p class="preview-desc">{{ snippetDraft.description }}</p>
        </div>
      </div>

      <!-- 编辑：表单 -->
      <div v-else class="snippet-body">
        <div class="field">
          <label>名称<span class="req">*</span></label>
          <input v-model="snippetDraft.name" type="text"
                 placeholder="例如：跟踪服务日志" spellcheck="false" />
        </div>

        <div class="field">
          <label>分组（可选）</label>
          <input v-model="snippetDraft.group" type="text"
                 placeholder="例如：服务、系统、容器" spellcheck="false" />
        </div>

        <div class="field">
          <label>命令<span class="req">*</span></label>
          <textarea
            v-model="snippetDraft.command"
            class="snippet-command-input"
            spellcheck="false"
            placeholder="journalctl -u nginx -f"
          ></textarea>
          <span class="desc">支持多行命令与 <code v-pre>{{name}}</code> 变量占位符，变量会自动识别。</span>
        </div>

        <div v-if="snippetDraft.vars.length" class="field">
          <label>变量<span class="var-count">{{ snippetDraft.vars.length }}</span></label>
          <div class="var-list">
            <div class="var-row">
              <span class="var-col-head">变量名</span>
              <span class="var-col-head">显示名</span>
              <span class="var-col-head">默认值</span>
              <span></span>
            </div>
            <div v-for="v in snippetDraft.vars" :key="v.name" class="var-row var-row-data">
              <span class="var-name" v-text="varToken(v.name)"></span>
              <input v-model="v.label" type="text" placeholder="例如：容器名" spellcheck="false" />
              <input
                v-model="v.defaultValue"
                type="text"
                placeholder="默认值（可空）"
                spellcheck="false"
              />
              <button
                type="button"
                class="var-del"
                title="移除后该占位符将原样保留"
                @click="removeVar(v.name)"
              >
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                     stroke-linecap="round" stroke-linejoin="round">
                  <polyline points="3 6 5 6 21 6"></polyline>
                  <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"></path>
                  <path d="M10 11v6M14 11v6"></path>
                  <path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2"></path>
                </svg>
              </button>
            </div>
          </div>
          <span class="desc">显示名支持中文；移除的变量不会参与替换，对应的 <code v-pre>{{name}}</code> 会原样保留。</span>
        </div>

        <div class="field">
          <label>说明（可选）</label>
          <input v-model="snippetDraft.description" type="text"
                 placeholder="一句话描述这个片段的用途" spellcheck="false" />
        </div>
      </div>

      <div class="modal-foot">
        <span class="km-hint"><kbd>Esc</kbd> 关闭</span>
        <span class="spacer"></span>

        <template v-if="isPreview">
          <button type="button" class="btn ghost" @click="editSnippetDraft">编辑</button>
          <button type="button" class="btn ghost" @click="onCopy">复制</button>
          <button type="button" class="btn primary" @click="onAction">执行</button>
        </template>

        <template v-else>
          <button type="button" class="btn ghost" @click="closeSnippetEditor">取消</button>
          <button type="button" class="btn primary" @click="onSave">保存片段</button>
        </template>
      </div>
    </div>
  </div>
</template>

<style scoped>
.snippet-modal {
  width: 480px;
  max-width: calc(100vw - 48px);
  height: auto;
  max-height: min(640px, calc(100vh - 60px));
}
/* 删除按钮：低饱和文字色，仅在悬停时提亮 */
.btn.ghost.danger {
  color: var(--danger);
}
.snippet-body {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  gap: 14px;
  min-height: 0;
  padding: 16px 18px;
  overflow-y: auto;
}
.snippet-command-input {
  min-height: 96px;
  font-family: var(--mono);
  font-size: 12px;
  line-height: 1.7;
  resize: vertical;
}

/* 变量声明编辑：一行一个变量，列对齐 */
.var-count {
  padding: 0 5px;
  font-size: 9.5px;
  line-height: 15px;
  color: var(--muted-2);
  border: 1px solid var(--border);
  border-radius: var(--radius-control);
}
.var-list {
  overflow: hidden;
  background: var(--panel-2);
  border: 1px solid var(--border);
  border-radius: var(--radius-control);
}
.var-row {
  display: grid;
  grid-template-columns: 104px minmax(0, 1fr) minmax(0, 1fr) 28px;
  align-items: center;
  gap: 8px;
  padding: 5px 8px;
}
.var-row + .var-row {
  border-top: 1px solid var(--border);
}
.var-col-head {
  font-size: 10px;
  color: var(--fg-faint);
  letter-spacing: 0.3px;
}
.var-row-data {
  transition: background var(--dur-fast) var(--ease);
}
.var-row-data:hover {
  background: var(--hover);
}
.var-name {
  overflow: hidden;
  font-family: var(--mono);
  font-size: 11px;
  color: var(--accent);
  white-space: nowrap;
  text-overflow: ellipsis;
}
.var-row-data input[type="text"] {
  width: 100%;
  height: 26px;
  padding: 0 8px;
  font-size: 11.5px;
}
.var-del {
  display: grid;
  place-items: center;
  width: 28px;
  height: 28px;
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
.var-del svg {
  width: 14px;
  height: 14px;
}
.var-del:hover {
  color: var(--danger);
  background: var(--hover);
}

/* 预览态 */
.preview-body {
  display: flex;
  flex-direction: column;
}
.preview-meta {
  display: flex;
  align-items: center;
  gap: 8px;
}
.preview-name {
  font-size: 14px;
  font-weight: 500;
  color: var(--fg-strong);
}
.preview-group {
  padding: 0 6px;
  font-size: 10.5px;
  line-height: 17px;
  color: var(--muted-2);
  border: 1px solid var(--border);
  border-radius: var(--radius-control);
}
.preview-command {
  margin: 0;
  padding: 10px 12px;
  overflow-x: auto;
  font-family: var(--mono);
  font-size: 12px;
  line-height: 1.7;
  color: var(--text);
  white-space: pre-wrap;
  word-break: break-all;
  background: var(--panel-2);
  border: 1px solid var(--border);
  border-radius: var(--radius-control);
}
.preview-var {
  color: var(--accent);
  font-weight: 500;
}
.preview-vars {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
}
.preview-vars-label {
  font-size: 11px;
  color: var(--muted-2);
}
.preview-var-chip {
  padding: 0 6px;
  font-family: var(--mono);
  font-size: 10.5px;
  line-height: 17px;
  color: var(--accent);
  background: var(--panel-2);
  border: 1px solid var(--border);
  border-radius: var(--radius-control);
}
.preview-desc {
  margin: 0;
  font-size: 12px;
  line-height: 1.6;
  color: var(--muted-2);
}
</style>
