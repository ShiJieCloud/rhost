<script setup lang="ts">
import { computed } from 'vue'
import { activeSession, metrics } from '../../stores/session'

// 五态：ok 已连接 / info 连接中 / warn 重连中 / err 已断开 / idle 未连接（新建会话，从未连接）
const statusText = () => {
  const s = activeSession.value
  if (!s) return { cls: 'idle', label: '未连接' }
  if (s.state === 'online') return { cls: 'ok', label: '已连接' }
  if (s.state === 'connecting') return { cls: 'info', label: '连接中…' }
  if (s.state === 'reconnecting') return { cls: 'warn', label: '重连中…' }
  if (s.state === 'idle') return { cls: 'idle', label: '未连接' }
  return { cls: 'err', label: '已断开' }
}

function fmtSpeed(v: number) {
  if (v >= 1024) return (v / 1024).toFixed(1) + ' MB'
  return v.toFixed(1) + ' KB'
}

const lat = computed(() => activeSession.value?.host.lat ?? null)

/* 延迟分档：<80ms 优 / <200ms 良 / ≥200ms 差 */
const latLevel = computed(() => {
  const v = lat.value
  if (v == null) return 'idle'
  if (v < 80) return 'ok'
  if (v < 200) return 'warn'
  return 'err'
})
</script>

<template>
  <footer class="statusbar">
    <span class="item" :class="statusText().cls">
      <span class="pulse"></span>
      {{ statusText().label }}
    </span>
    <span class="item">
      {{ activeSession ? `${activeSession.host.user}@${activeSession.host.ip}:${activeSession.host.port}` : '—' }}
    </span>
    <span class="item">ssh-ed25519</span>
    <span class="item">AES-256-GCM</span>
    <span class="spacer"></span>


    <span class="item">xterm-256color</span>
    <span class="item">UTF-8</span>

    <!-- 流量 + 延迟（附件设计：分段竖线分隔） -->
    <span class="seg"><span class="arrow-up">↑</span><span class="val">{{ fmtSpeed(metrics.tx) }}</span></span>
    <span class="seg"><span class="arrow-down">↓</span><span class="val">{{ fmtSpeed(metrics.rx) }}</span></span>
    <span class="seg"><span class="dot-rtt" :class="latLevel"></span><span class="val" :class="latLevel">{{ lat ? lat + ' ms' : '—' }}</span></span>
  </footer>
</template>
