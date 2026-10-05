<script setup lang="ts">
import { computed, ref } from 'vue'
import { hosts, onlineCount } from '../../stores/hosts'
import { activeSession, openSession } from '../../stores/session'

const q = ref('')

const groups = computed(() => {
  const kw = q.value.trim().toLowerCase()
  const map = new Map<string, typeof hosts.value>()
  hosts.value.forEach(h => {
    if (kw && !(h.id.toLowerCase().includes(kw) || h.ip.includes(kw) || h.user.toLowerCase().includes(kw) || h.group.toLowerCase().includes(kw))) {
      return
    }
    const list = map.get(h.group) ?? []
    list.push(h)
    map.set(h.group, list)
  })
  return [...map.entries()].map(([name, list]) => ({ name, list }))
})
</script>

<template>
  <aside class="sidebar" :class="{ hide: false }">
    <div class="search">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
           stroke-linecap="round"><circle cx="11" cy="11" r="7"></circle>
        <line x1="21" y1="21" x2="16.7" y2="16.7"></line></svg>
      <input v-model="q" type="text" placeholder="筛选主机…" autocomplete="off">
    </div>

    <div class="host-list">
      <template v-for="g in groups" :key="g.name">
        <div class="group-label">{{ g.name }}</div>
        <div
          v-for="h in g.list"
          :key="h.id"
          class="host"
          :class="{ active: activeSession?.host.id === h.id }"
          @click="openSession(h.id)"
        >
          <span class="dot" :class="h.status === 'offline' ? 'offline' : h.status === 'warn' ? 'warn' : 'online'"></span>
          <div class="host-info">
            <div class="host-name">{{ h.id }}</div>
            <div class="host-meta">{{ h.user }}@{{ h.ip }}:{{ h.port }}</div>
          </div>
        </div>
      </template>
    </div>

    <div class="sidebar-foot">
      <span class="dot online" style="animation:none"></span>
      <span>{{ hosts.length }} 台主机 · {{ onlineCount }} 台在线</span>
    </div>
  </aside>
</template>
