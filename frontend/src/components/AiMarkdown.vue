<script setup lang="ts">
/**
 * AI 消息 Markdown 正文：computed 缓存渲染结果，流式期间仅变化中的消息重算。
 * 代码块按钮为事件委托，原文取自相邻 pre 的 textContent（fence 渲染时不内联原文）。
 * 终端操作按钮（插入/直接执行）由宿主 terminalReady 控制显隐，意图经 emit 上报，
 * PTY 写入与会话定位由宿主 AiChatPanel 执行（ai-terminal-exec-design.md §6.1）。
 */
import { computed } from 'vue'
import { writeText } from '@tauri-apps/plugin-clipboard-manager'
import { renderMarkdown } from '../lib/markdown'
import { toast } from '../composables/useToast'

const props = defineProps<{ content: string; terminalReady?: boolean }>()

const emit = defineEmits<{ terminal: [{ code: string; run: boolean }] }>()

const html = computed(() => renderMarkdown(props.content))

async function onClick(e: MouseEvent) {
  const target = e.target as HTMLElement | null
  const btn = target?.closest?.('.md-code-copy, .md-code-insert, .md-code-run')
  if (!btn) return
  // 仅取代码列（排除行号 gutter 列）；fence 内容天然以 \n 结尾，
  // 必须去掉尾部换行，否则「插入终端」会带上回车导致立即执行
  const code = (btn.closest('.md-code')?.querySelector('pre:not(.md-code-lines)')?.textContent ?? '').replace(/[\r\n]+$/, '')
  if (!code) return
  if (btn.classList.contains('md-code-copy')) {
    try {
      await writeText(code)
      toast('代码已复制', 'ok', 1200)
    } catch (err) {
      console.error('[clipboard] writeText failed', err)
      toast('复制失败', 'err', 1500)
    }
    return
  }
  // 插入终端（不回车）/ 直接执行（确认后写入 + 回车）
  emit('terminal', { code, run: btn.classList.contains('md-code-run') })
}
</script>

<template>
  <!-- markdown-it html:false 已转义传入 HTML，v-html 安全 -->
  <div class="md" :class="{ 'term-ready': terminalReady }" v-html="html" @click="onClick"></div>
</template>

<style scoped>
.md {
  font-size: 12px;
  line-height: 1.6;
  color: var(--text);
  word-break: break-word;
  /* 覆盖父级 .bubble 的 pre-wrap，换行交给 markdown 处理 */
  white-space: normal;
}
.md :deep(p) {
  margin: 0 0 8px;
}
.md :deep(p:last-child) {
  margin-bottom: 0;
}
.md :deep(h1),
.md :deep(h2),
.md :deep(h3),
.md :deep(h4) {
  margin: 12px 0 6px;
  font-size: 12.5px;
  font-weight: 600;
}
.md :deep(h1:first-child),
.md :deep(h2:first-child),
.md :deep(h3:first-child) {
  margin-top: 0;
}
.md :deep(ul),
.md :deep(ol) {
  margin: 0 0 8px;
  padding-left: 18px;
}
.md :deep(li) {
  margin: 2px 0;
}
.md :deep(blockquote) {
  margin: 0 0 8px;
  padding: 2px 10px;
  border-left: 2px solid var(--border);
  color: var(--muted);
}
.md :deep(hr) {
  border: none;
  border-top: 1px solid var(--border);
  margin: 10px 0;
}
.md :deep(a) {
  color: var(--green);
  text-decoration: none;
}
.md :deep(a:hover) {
  text-decoration: underline;
}
/* 行内代码 */
.md :deep(code) {
  font-family: var(--mono);
  font-size: 11px;
  background: var(--panel-2);
  border-radius: 4px;
  padding: 1px 5px;
}
.md :deep(table) {
  display: block;
  overflow-x: auto;
  border-collapse: collapse;
  margin: 0 0 8px;
  font-size: 11.5px;
}
.md :deep(th),
.md :deep(td) {
  border: 1px solid var(--border);
  padding: 4px 8px;
  text-align: left;
}
.md :deep(th) {
  background: var(--panel-2);
}

/* 代码块 */
.md :deep(.md-code) {
  margin: 0 0 8px;
  border: 1px solid var(--border);
  border-radius: 8px;
  overflow: hidden;
  background: #0d1319;
}
.md :deep(.md-code-head) {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 3px 10px;
  background: var(--panel-2);
  border-bottom: 1px solid var(--border);
  font-size: 10.5px;
  color: var(--muted-2);
}
.md :deep(.md-code-lang) {
  font-family: var(--mono);
  text-transform: lowercase;
}
.md :deep(.md-code-actions) {
  display: flex;
  align-items: center;
  gap: 2px;
}
.md :deep(.md-code-copy),
.md :deep(.md-code-insert),
.md :deep(.md-code-run) {
  border: none;
  background: transparent;
  color: var(--muted-2);
  font-size: 10.5px;
  cursor: pointer;
  padding: 2px 4px;
  border-radius: 4px;
  transition: color 0.15s, background 0.15s;
}
.md :deep(.md-code-copy:hover),
.md :deep(.md-code-insert:hover),
.md :deep(.md-code-run:hover) {
  color: var(--green);
  background: rgba(61, 220, 132, 0.08);
}
/* 终端操作按钮：仅宿主声明 terminalReady（workbench + 在线会话 + 非 streaming）时可见 */
.md :deep(.md-code-insert),
.md :deep(.md-code-run) {
  display: none;
}
.md.term-ready :deep(.md-code-insert),
.md.term-ready :deep(.md-code-run) {
  display: inline-block;
}
.md :deep(.md-code-wrap) {
  display: flex;
  align-items: stretch;
}
/* 行号 gutter：与代码列同 font/line-height，仅代码列横向滚动，行号固定 */
.md :deep(.md-code-lines) {
  flex-shrink: 0;
  overflow-x: hidden;
  user-select: none;
  text-align: right;
  padding: 10px 8px 10px 12px;
  border-right: 1px solid var(--border);
  color: #3d4a5c;
  font-family: var(--mono);
  font-size: 11px;
  line-height: 1.55;
}
.md :deep(pre) {
  margin: 0;
  padding: 10px 12px;
  overflow-x: auto;
}
.md :deep(.md-code-wrap pre:not(.md-code-lines)) {
  flex: 1;
  min-width: 0;
  padding-left: 8px;
}
.md :deep(pre code) {
  display: block;
  background: transparent;
  padding: 0;
  font-size: 11px;
  line-height: 1.55;
  font-family: var(--mono);
}

/* highlight.js token 配色（One Dark 系，适配暗色主题） */
.md :deep(.hljs-keyword),
.md :deep(.hljs-selector-tag),
.md :deep(.hljs-literal),
.md :deep(.hljs-section) {
  color: #c678dd;
}
.md :deep(.hljs-string),
.md :deep(.hljs-regexp),
.md :deep(.hljs-addition) {
  color: #98c379;
}
.md :deep(.hljs-number),
.md :deep(.hljs-symbol),
.md :deep(.hljs-bullet),
.md :deep(.hljs-deletion) {
  color: #d19a66;
}
.md :deep(.hljs-comment),
.md :deep(.hljs-quote) {
  color: #5f6b7d;
  font-style: italic;
}
.md :deep(.hljs-title),
.md :deep(.hljs-function),
.md :deep(.hljs-name) {
  color: #61afef;
}
.md :deep(.hljs-type),
.md :deep(.hljs-built_in),
.md :deep(.hljs-class .hljs-title) {
  color: #e5c07b;
}
.md :deep(.hljs-attr),
.md :deep(.hljs-attribute),
.md :deep(.hljs-variable),
.md :deep(.hljs-template-variable) {
  color: #e06c75;
}
.md :deep(.hljs-meta),
.md :deep(.hljs-link) {
  color: #56b6c2;
}
.md :deep(.hljs-emphasis) {
  font-style: italic;
}
.md :deep(.hljs-strong) {
  font-weight: 600;
}
</style>
