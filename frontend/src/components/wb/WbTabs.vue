<script setup lang="ts">
import { ref } from 'vue'
import { hosts } from '../../stores/hosts'
import { activeSessionId, closeSession, openSession, sessions } from '../../stores/session'

const quickOpen = ref(false)

function dotCls(state: string) {
  return state === 'online' ? 'st-ok'
    : state === 'connecting' ? 'st-info'
    : state === 'reconnecting' ? 'st-warn'
    : state === 'idle' ? 'st-idle'
    : 'st-err'
}

function onQuick(h: { id: string }) {
  quickOpen.value = false
  openSession(h.id)
}
</script>

<template>
  <div class="tabs">
    <button class="tab-add" title="快速连接" @click.stop="quickOpen = !quickOpen">+</button>

    <div class="quick-menu" :class="{ show: quickOpen }">
      <div class="qm-head">快速连接</div>
      <button
        v-for="h in hosts.slice(0, 7)"
        :key="h.id"
        class="qm-item"
        @click="onQuick(h)"
      >
        <span class="dot" :class="h.status === 'offline' ? 'offline' : h.status === 'warn' ? 'warn' : 'online'"></span>
        <span>{{ h.id }}</span>
        <span class="m">{{ h.ip }}</span>
      </button>
    </div>

    <div
      v-for="s in sessions"
      :key="s.id"
      class="tab"
      :class="{ active: activeSessionId === s.id }"
      @click="activeSessionId = s.id"
    >
      <span class="dot" :class="dotCls(s.state)"></span>
      <span class="name">{{ s.host.id }}</span>
      <span class="close" @click.stop="closeSession(s.id)">×</span>
    </div>
  </div>
</template>
