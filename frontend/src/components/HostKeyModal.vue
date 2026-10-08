<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import {
  hostKeyPromptState,
  acceptHostKey,
  rejectHostKey,
  updateHostKey,
} from '../composables/useHostKeyPrompt'
import { copyText } from '../stores/keys'

const { visible, info } = hostKeyPromptState

const isMismatch = computed(() => info.value?.kind === 'mismatch')

// 「查看完整公钥」展开状态；每次打开弹窗重置为收起
const showPubkey = ref(false)
watch(visible, (v) => {
  if (v) showPubkey.value = false
})
function togglePubkey() {
  showPubkey.value = !showPubkey.value
}

// 复制指纹：成功后短暂显示「已复制」反馈；失败静默——指纹块本身可手动选中
const copied = ref(false)
let copiedTimer: ReturnType<typeof setTimeout> | null = null
async function copyFp() {
  if (!info.value) return
  const ok = await copyText(info.value.fingerprint)
  if (!ok) return
  copied.value = true
  if (copiedTimer) clearTimeout(copiedTimer)
  copiedTimer = setTimeout(() => {
    copied.value = false
  }, 1600)
}
onUnmounted(() => {
  if (copiedTimer) clearTimeout(copiedTimer)
})

function onKey(e: KeyboardEvent) {
  if (!visible.value) return
  if (e.key === 'Escape') {
    // ESC 一律 = 取消（拒绝信任、不落盘、连接保持失败态）
    e.preventDefault()
    rejectHostKey()
  } else if (e.key === 'Enter') {
    e.preventDefault()
    // Enter 仅触发默认动作：unknown = 接受并保存（主按钮）；
    // mismatch = 断开（默认焦点语义）——高危的「更新指纹并重连」
    // 禁止成为回车默认动作，只能鼠标点击（设计 §6.5 硬性要求）
    if (isMismatch.value) rejectHostKey()
    else acceptHostKey(true)
  }
}

onMounted(() => document.addEventListener('keydown', onKey))
onUnmounted(() => document.removeEventListener('keydown', onKey))
</script>

<template>
  <div class="mask" :class="{ show: visible }" @click.self="rejectHostKey">
    <div class="modal hk-modal" role="dialog" aria-modal="true" aria-labelledby="hkTitle">
      <div class="modal-head">
        <div class="modal-icon" :class="{ danger: isMismatch }">
          <!-- unknown：指纹/锁；mismatch：警告三角 -->
          <svg v-if="!isMismatch" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
               stroke-linecap="round" stroke-linejoin="round">
            <rect x="4" y="11" width="16" height="10" rx="2" />
            <path d="M8 11V7a4 4 0 0 1 8 0v4" />
          </svg>
          <svg v-else viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
               stroke-linecap="round" stroke-linejoin="round">
            <path d="M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
            <line x1="12" y1="9" x2="12" y2="13" />
            <line x1="12" y1="17" x2="12.01" y2="17" />
          </svg>
        </div>
        <div class="modal-title">
          <h2 id="hkTitle">{{ isMismatch ? '主机密钥已变更！' : '无法验证主机真实性' }}</h2>
          <p v-if="isMismatch" class="hk-warn-text">
            主机密钥与上次记录不一致。可能是服务器重装或密钥轮换，<strong>也可能存在中间人攻击</strong>。
          </p>
        </div>
        <button class="modal-close" title="关闭" @click="rejectHostKey">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
               stroke-linecap="round">
            <line x1="18" y1="6" x2="6" y2="18" />
            <line x1="6" y1="6" x2="18" y2="18" />
          </svg>
        </button>
      </div>

      <div class="hk-body">
        <!-- unknown 引导：解释为何需确认，主机与 known_hosts 路径用 chip 突出 -->
        <p v-if="!isMismatch" class="hk-lead">
          无法确认主机 <code>{{ info?.host }}</code> 的真实性。该主机尚未记录在
          <code>~/.ssh/known_hosts</code> 中，请核对下方指纹是否与服务器管理员提供的一致。
        </p>

        <div class="hk-kv">
          <div class="hk-row">
            <span class="hk-label">主机</span>
            <span class="hk-value">{{ info?.host }}</span>
          </div>
          <div class="hk-row">
            <span class="hk-label">地址</span>
            <span class="hk-value">{{ info?.host }}:{{ info?.port }}</span>
          </div>
          <div class="hk-row">
            <span class="hk-label">密钥类型</span>
            <span class="hk-value">{{ info?.algo }}</span>
          </div>
          <div class="hk-row">
            <span class="hk-label">指纹</span>
            <!-- 等宽字体 + 可选中复制：用户可与服务器侧指纹逐字比对 -->
            <span class="hk-fp">{{ info?.fingerprint }}</span>
            <button type="button" class="hk-copy" :class="{ done: copied }" @click="copyFp">
              {{ copied ? '已复制' : '复制' }}
            </button>
          </div>
        </div>

        <!-- 完整公钥展开区：可选中复制，与服务器侧 `ssh-keyscan` 输出核对 -->
        <button type="button" class="hk-toggle" @click="togglePubkey">
          <svg class="hk-caret" :class="{ open: showPubkey }" viewBox="0 0 24 24" fill="none"
               stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="9 18 15 12 9 6" />
          </svg>
          {{ showPubkey ? '收起完整公钥' : '查看完整公钥' }}
        </button>
        <pre v-if="showPubkey" class="hk-pubkey" spellcheck="false">{{ info?.pubkey }}</pre>

        <p class="hk-hint" :class="{ danger: isMismatch }">
          <template v-if="isMismatch">⚠ 若你近期未重装或更换过该服务器，请勿更新指纹。</template>
          <template v-else>⚠ 指纹不一致时请勿继续，可能存在中间人攻击风险。</template>
        </p>
      </div>

      <div class="modal-foot">
        <span class="hk-kbd-hint">
          <template v-if="isMismatch"><kbd>Enter</kbd> 断开 · <kbd>Esc</kbd> 取消</template>
          <template v-else><kbd>Enter</kbd> 接受并保存 · <kbd>Esc</kbd> 取消</template>
        </span>
        <span class="spacer"></span>
        <template v-if="isMismatch">
          <!-- 默认动作（安全）：断开；危险动作（红色样式）仅鼠标点击可达 -->
          <button type="button" class="btn primary" @click="rejectHostKey">断开</button>
          <button type="button" class="btn danger" @click="updateHostKey">更新指纹并重连</button>
        </template>
        <template v-else>
          <button type="button" class="btn ghost" @click="rejectHostKey">取消</button>
          <!-- 仅本次连接：本次会话可信但不写 known_hosts，下次连接重新确认 -->
          <button type="button" class="btn" @click="acceptHostKey(false)">仅本次连接</button>
          <button type="button" class="btn primary" @click="acceptHostKey(true)">接受并保存</button>
        </template>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 全局 .modal 固定 820×640，本弹窗内容少，按内容自适应 */
.hk-modal {
  width: 480px;
  height: auto;
  max-height: calc(100vh - 60px);
}
/* mismatch 时头部图标与标题转红色警示 */
.hk-modal .modal-icon.danger {
  color: var(--rhost-status-error);
}
.hk-modal .modal-title p {
  word-break: break-all;
  line-height: 1.5;
}
.hk-warn-text {
  color: var(--rhost-status-error);
}
.hk-warn-text strong {
  font-weight: 700;
}
.hk-body {
  padding: 14px 18px 4px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}
/* 引导段落：unknown 态解释为何需确认；主机名与 known_hosts 路径用 chip 突出 */
.hk-lead {
  margin: 0;
  font-size: 13px;
  line-height: 1.7;
  color: var(--text);
}
.hk-lead code {
  font-family: var(--mono);
  font-size: 12px;
  color: var(--text);
  background: var(--hover);
  border: 1px solid var(--border);
  padding: 1px 6px;
  border-radius: 4px;
  word-break: break-all;
}
/* 键值信息面板：深色底 + 行间虚线分隔，供逐字核对 */
.hk-kv {
  background: var(--panel-2);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 0 12px;
}
.hk-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 9px 0;
  border-bottom: 1px dashed var(--border-soft);
}
.hk-row:last-child {
  border-bottom: none;
}
.hk-label {
  flex: 0 0 4.5em;
  color: var(--muted);
  font-size: 12px;
}
.hk-value {
  flex: 1;
  min-width: 0;
  font-size: 12.5px;
  word-break: break-all;
}
.hk-fp {
  flex: 1;
  min-width: 0;
  font-size: 12.5px;
  word-break: break-all;
  /* 指纹必须可选中复制，供用户与服务器侧比对 */
  user-select: text;
  -webkit-user-select: text;
  cursor: text;
}
/* 复制按钮：小尺寸次级样式，成功后转绿色「已复制」 */
.hk-copy {
  flex: none;
  font: inherit;
  font-size: 11px;
  padding: 2px 9px;
  border-radius: 6px;
  cursor: pointer;
  color: var(--muted);
  background: var(--hover);
  border: 1px solid var(--border);
  transition: color 0.15s, background 0.15s, border-color 0.15s;
}
.hk-copy:hover {
  color: var(--text);
  border-color: var(--muted-2);
}
.hk-copy.done {
  color: var(--rhost-status-success);
  border-color: rgba(74, 222, 128, 0.45);
}
/* 完整公钥展开开关：低调次级按钮，左置小箭头随展开旋转 */
.hk-toggle {
  align-self: flex-start;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font: inherit;
  font-size: 12px;
  padding: 3px 8px;
  border-radius: 6px;
  cursor: pointer;
  color: var(--muted);
  background: transparent;
  border: 1px solid transparent;
  transition: color 0.15s, background 0.15s, border-color 0.15s;
}
.hk-toggle:hover {
  color: var(--text);
  background: var(--hover);
  border-color: var(--border);
}
.hk-caret {
  width: 14px;
  height: 14px;
  transition: transform 0.15s;
}
.hk-caret.open {
  transform: rotate(90deg);
}
/* 完整公钥块：等宽 + 自动换行 + 可选中，供与 ssh-keyscan 输出比对 */
.hk-pubkey {
  margin: 0;
  padding: 10px 12px;
  font-family: var(--mono);
  font-size: 11.5px;
  line-height: 1.6;
  color: var(--text);
  background: var(--panel-2);
  border: 1px solid var(--border);
  border-radius: 8px;
  white-space: pre-wrap;
  word-break: break-all;
  user-select: text;
  -webkit-user-select: text;
}
.hk-hint {
  margin: 0;
  color: var(--rhost-status-warning);
  font-size: 12px;
  line-height: 1.5;
}
.hk-hint.danger {
  color: var(--rhost-status-error);
}
.hk-kbd-hint {
  color: var(--muted);
  font-size: 12px;
}
</style>
