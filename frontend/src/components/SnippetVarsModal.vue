<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { applySnippetCommand, closeSnippetRun, snippetRun } from '../stores/snippetRun'
import { renderCommand, varToken } from '../lib/snippetVars'

const vars = computed(() => snippetRun.value?.vars ?? [])

const values = reactive<Record<string, string>>({})

/** 首个输入框，打开面板时自动聚焦 */
const firstField = ref<HTMLElement | null>(null)
function setFirst(el: unknown) {
  if (el) firstField.value = el as HTMLElement
}

/** 打开新目标时重置并预填默认值 */
watch(snippetRun, async run => {
  for (const k of Object.keys(values)) delete values[k]
  firstField.value = null
  if (!run) return
  for (const v of run.vars) {
    values[v.name] = v.defaultValue
  }
  await nextTick()
  ;(firstField.value as HTMLElement | null)?.focus()
})

const rendered = computed(() => {
  const run = snippetRun.value
  if (!run) return ''
  return renderCommand(run.command, run.vars, values)
})

/** 只拦截必填项，非必填留空时占位符原样保留 */
const missing = computed(() => vars.value.filter(v => v.required && !(values[v.name] ?? '').trim()))
const canConfirm = computed(() => missing.value.length === 0)

async function onConfirm() {
  const run = snippetRun.value
  if (!run || !canConfirm.value) return
  const command = renderCommand(run.command, run.vars, values)
  closeSnippetRun()
  await applySnippetCommand(command)
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape' && snippetRun.value) closeSnippetRun()
}

onMounted(() => document.addEventListener('keydown', onKeydown))
onUnmounted(() => document.removeEventListener('keydown', onKeydown))
</script>

<template>
  <div v-if="snippetRun" class="mask show" @click.self="closeSnippetRun">
    <div class="modal vars-modal" role="dialog" aria-modal="true">
      <div class="modal-head">
        <div class="modal-icon">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
               stroke-linecap="round" stroke-linejoin="round">
            <polyline points="4 17 10 11 4 5" />
            <line x1="12" y1="19" x2="20" y2="19" />
          </svg>
        </div>
        <div class="modal-title">
          <h2>填写变量</h2>
          <p>{{ snippetRun.name }} · 共 {{ vars.length }} 个变量</p>
        </div>
        <button class="modal-close" title="关闭" @click="closeSnippetRun">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
               stroke-linecap="round">
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </button>
      </div>

      <div class="vars-body">
        <div v-for="(v, i) in vars" :key="v.name" class="field">
          <label>
            {{ v.label || v.name }}
            <span v-if="v.required" class="req">*</span>
            <span v-if="v.label && v.label !== v.name" class="var-token" v-text="varToken(v.name)"></span>
          </label>

          <select
            v-if="v.kind === 'select'"
            v-model="values[v.name]"
            :ref="i === 0 ? setFirst : undefined"
          >
            <option value="">请选择…</option>
            <option v-for="o in v.options" :key="o" :value="o">{{ o }}</option>
          </select>

          <input
            v-else
            :ref="i === 0 ? setFirst : undefined"
            v-model="values[v.name]"
            :type="v.secret ? 'password' : 'text'"
            :placeholder="v.defaultValue ? `默认：${v.defaultValue}` : `填写 ${v.name}`"
            spellcheck="false"
            autocomplete="off"
            @keydown.enter.prevent="onConfirm"
          />
        </div>

        <p v-if="missing.length" class="vars-hint">
          还有必填变量未填写：{{ missing.map(v => v.label || v.name).join('、') }}
        </p>

        <div class="field">
          <label>最终命令</label>
          <pre class="vars-preview">{{ rendered }}</pre>
        </div>
      </div>

      <div class="modal-foot">
        <span class="km-hint"><kbd>Esc</kbd> 取消</span>
        <span class="spacer"></span>
        <button type="button" class="btn ghost" @click="closeSnippetRun">取消</button>
        <button type="button" class="btn primary" :disabled="!canConfirm" @click="onConfirm">
          执行
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.vars-modal {
  width: 460px;
  max-width: calc(100vw - 48px);
  height: auto;
  max-height: min(640px, calc(100vh - 60px));
}
.vars-body {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  gap: 14px;
  min-height: 0;
  padding: 16px 18px;
  overflow-y: auto;
}
.var-token {
  font-family: var(--mono);
  font-size: 10px;
  color: var(--muted-2);
}
.vars-hint {
  margin: 0;
  font-size: 11px;
  line-height: 1.5;
  color: var(--danger);
}
.vars-preview {
  margin: 0;
  padding: 8px 10px;
  overflow-x: auto;
  font-family: var(--mono);
  font-size: 11.5px;
  line-height: 1.65;
  color: var(--muted-2);
  white-space: pre-wrap;
  word-break: break-all;
  background: var(--panel-2);
  border: 1px solid var(--border);
  border-radius: var(--radius-control);
}
</style>
