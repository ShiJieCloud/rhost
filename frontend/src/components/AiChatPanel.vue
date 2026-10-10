<script setup lang="ts">
import { nextTick, ref, watch } from 'vue'
import { activeSession, appView, rightPanelTab, rightPanelVisible } from '../stores/session'

interface ChatMessage {
  role: 'user' | 'ai'
  text: string
}

/** 每个会话单独记录；首页用特殊 key 'global' */
const sessionKey = () => (appView.value === 'home' ? 'global' : activeSession.value?.id ?? 'global')

/** 极简内存存储；后续可迁移到 localStorage / IndexedDB */
const historyMap = new Map<string, ChatMessage[]>()
const currentMessages = ref<ChatMessage[]>([])
const draft = ref('')
const inputRef = ref<HTMLInputElement | null>(null)
const scrollEl = ref<HTMLElement | null>(null)

function syncMessages() {
  const key = sessionKey()
  currentMessages.value = historyMap.get(key) ?? []
}

// 切换会话/页面时同步
watch([activeSession, appView], syncMessages, { immediate: true })

// 右面板切换到 AI 页签并展开时自动聚焦
watch(
  [rightPanelVisible, rightPanelTab],
  ([visible, tab]) => {
    if (visible && tab === 'ai') {
      nextTick(() => inputRef.value?.focus())
    }
  },
  { flush: 'post' },
)

function scrollToBottom() {
  nextTick(() => {
    if (scrollEl.value) scrollEl.value.scrollTop = scrollEl.value.scrollHeight
  })
}

function send() {
  const text = draft.value.trim()
  if (!text) return
  const msgs = historyMap.get(sessionKey()) ?? []
  msgs.push({ role: 'user', text })
  msgs.push({ role: 'ai', text: `已收到：${text}\n（AI 接入尚未完成，这是占位回复）` })
  historyMap.set(sessionKey(), msgs)
  draft.value = ''
  syncMessages()
  scrollToBottom()
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault()
    send()
  }
}

/** 快速提问快捷词 */
const quickHints = [
  'nginx 502 怎么排查？',
  '解释这条日志的含义',
  '生成一个 systemd service 文件',
]
function insertHint(h: string) {
  draft.value = h
  nextTick(() => inputRef.value?.focus())
}

/** 将代码块插入当前终端（仅工作台可用） */
function insertToTerminal(code: string) {
  // TODO: 通过全局事件或 store action 向 TerminalPane 写入 stdin
  console.log('[AI] insert to terminal:', code)
}

/** 清空当前会话/全局聊天记录 */
function clearChat() {
  const key = sessionKey()
  historyMap.delete(key)
  syncMessages()
}
</script>

<template>
  <div class="ai-panel">
    <!-- 操作按钮注入 RightPanel 头部 -->
    <Teleport v-if="rightPanelTab === 'ai'" to="#rp-head-actions">
      <button class="ai-action-btn" title="清空对话" :disabled="currentMessages.length === 0" @click="clearChat">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
             stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="3 6 5 6 21 6" />
          <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
        </svg>
      </button>
    </Teleport>
    <!-- 消息列表 -->
    <div ref="scrollEl" class="chat-scroll">
      <template v-if="currentMessages.length === 0">
        <div class="empty">
          <div class="empty-title">AI 助手</div>
          <div class="empty-sub">
            {{ appView === 'home' ? '全局问答 · 不绑定任何会话' : '已自动关联当前会话上下文' }}
          </div>
          <div class="hints">
            <button v-for="h in quickHints" :key="h" class="hint-pill" @click="insertHint(h)">
              {{ h }}
            </button>
          </div>
        </div>
      </template>

      <template v-else>
        <div v-for="(m, i) in currentMessages" :key="i" class="bubble" :class="m.role">
          <div class="bubble-label">
            <template v-if="m.role === 'ai'">AI ·</template>
          </div>
          <div class="bubble-body">{{ m.text }}</div>
          <div v-if="m.role === 'ai' && appView === 'workbench'" class="bubble-action">
            <button class="insert-btn" @click="insertToTerminal(m.text)">⇥ 插入终端</button>
          </div>
        </div>
      </template>
    </div>

    <!-- 输入区 -->
    <div class="chat-input">
      <input
        ref="inputRef"
        v-model="draft"
        class="input"
        type="text"
        :placeholder="appView === 'home' ? '询问 AI…' : '询问 AI（携带当前会话上下文）…'"
        @keydown="onKeydown"
      />
      <button class="send-btn" title="发送 (Enter)" @click="send">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
             stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <line x1="22" y1="2" x2="11" y2="13" />
          <polygon points="22 2 15 22 11 13 2 9 22 2" />
        </svg>
      </button>
    </div>
  </div>
</template>

<style scoped>
.ai-panel {
  display: flex;
  flex-direction: column;
  width: 100%;
  height: 100%;
  overflow: hidden;
}

/* 消息列表 */
.chat-scroll {
  flex: 1;
  overflow-y: auto;
  overflow-x: hidden;
  padding: 10px 12px;
  display: flex;
  flex-direction: column;
  gap: 10px;
  /* 专注模式下面板很宽：内容限宽居中，保持阅读舒适 */
  width: 100%;
  max-width: 760px;
  margin: 0 auto;
  box-sizing: border-box;
}
.chat-scroll::-webkit-scrollbar { width: 6px; }
.chat-scroll::-webkit-scrollbar-thumb {
  background: #1c2836;
  border-radius: 3px;
}

/* 空态 */
.empty {
  margin-top: auto;
  margin-bottom: auto;
  text-align: center;
}
.empty-title {
  font-size: 14px;
  font-weight: 600;
  color: var(--text);
  margin-bottom: 4px;
}
.empty-sub {
  font-size: 11.5px;
  color: var(--muted-2);
  margin-bottom: 14px;
}
.hints {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
}
.hint-pill {
  border: 1px solid var(--border);
  border-radius: 999px;
  padding: 4px 12px;
  font-size: 11.5px;
  color: var(--muted);
  background: transparent;
  cursor: pointer;
  transition: background 0.15s, border-color 0.15s, color 0.15s;
}
.hint-pill:hover {
  background: var(--panel-2);
  border-color: var(--green);
  color: var(--text);
}

/* 气泡 */
.bubble {
  max-width: 94%;
  border-radius: 10px;
  padding: 8px 10px;
  font-size: 12px;
  line-height: 1.55;
  white-space: pre-wrap;
}
.bubble.user {
  align-self: flex-end;
  background: var(--panel-2);
  color: var(--text);
}
.bubble.ai {
  align-self: flex-start;
  background: transparent;
  border: 1px solid var(--border);
  color: var(--text);
}
.bubble-label {
  font-size: 10.5px;
  font-weight: 600;
  color: var(--green);
  margin-bottom: 2px;
}
.bubble-action {
  margin-top: 6px;
}
.insert-btn {
  border: 1px solid var(--green);
  border-radius: 6px;
  color: var(--green);
  background: rgba(61, 220, 132, 0.08);
  padding: 2px 8px;
  font-size: 11px;
  cursor: pointer;
  transition: background 0.15s;
}
.insert-btn:hover {
  background: rgba(61, 220, 132, 0.18);
}

/* 输入区 */
.chat-input {
  flex-shrink: 0;
  padding: 8px 10px;
  border-top: 1px solid var(--border);
  display: flex;
  align-items: center;
  gap: 6px;
  background: var(--panel);
  /* 与消息列表同宽居中 */
  width: 100%;
  max-width: 760px;
  margin: 0 auto;
  box-sizing: border-box;
}
.input {
  flex: 1;
  min-width: 0;
  background: var(--panel-2);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 6px 10px;
  font-size: 12px;
  color: var(--text);
  outline: none;
  transition: border-color 0.15s;
}
.input::placeholder {
  color: var(--muted-2);
}
.input:focus {
  border-color: var(--green);
}
.send-btn {
  width: 28px;
  height: 28px;
  border-radius: 8px;
  display: grid;
  place-items: center;
  background: var(--green);
  border: none;
  color: #0b1118;
  cursor: pointer;
  transition: opacity 0.15s;
  flex-shrink: 0;
}
.send-btn:hover {
  opacity: 0.85;
}
.send-btn:active {
  transform: scale(0.94);
}

/* 头部操作按钮（Teleport 到 RightPanel） */
.ai-action-btn {
  width: 26px;
  height: 26px;
  border-radius: 6px;
  display: grid;
  place-items: center;
  background: transparent;
  border: none;
  color: var(--muted-2);
  cursor: pointer;
  transition: background 0.15s, color 0.15s;
}
.ai-action-btn:hover:not(:disabled) {
  background: var(--panel-2);
  color: var(--text);
}
.ai-action-btn:disabled {
  opacity: 0.35;
  cursor: default;
}
</style>
