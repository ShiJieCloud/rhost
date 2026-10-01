<script setup lang="ts">
import { toast } from '../composables/useToast'
import { filtered, viewStyle } from '../stores/hosts'
import { openSession } from '../stores/session'
import { COLOR_MAP } from '../data/mockHosts'
import type { Host } from '../types'

function statusInfo(h: Host) {
  if (h.status === 'idle') return { cls: 'idle', text: '未连接' }
  if (h.status === 'offline') return { cls: 'offline', text: '离线' }
  if (h.status === 'warn') return { cls: 'warn', text: '告警' }
  return { cls: '', text: '在线' }
}

function onConnect(h: Host) {
  if (h.status === 'offline') {
    toast(`主机 ${h.id} 处于离线状态，无法连接`, 'err', 2400)
  } else {
    openSession(h.id)
  }
}

function accent(h: Host) {
  return { '--accent': COLOR_MAP[h.color] }
}
</script>

<template>
  <div class="project-list" :class="'view-' + viewStyle">
    <template v-if="filtered.length">
      <!-- 列表视图 -->
      <template v-if="viewStyle === 'list'">
        <div
          v-for="h in filtered"
          :key="h.id"
          class="project-item"
          @click="onConnect(h)"
        >
          <div class="proj-icon" :class="'icon-' + h.color">{{ h.label }}</div>
          <div class="proj-info">
            <div class="proj-name">{{ h.id }}</div>
            <div class="proj-path">
              <span>{{ h.user }}@{{ h.ip }}:{{ h.port }}</span>
              <span class="sep">·</span>
              <span>{{ h.os }}</span>
            </div>
          </div>
          <div class="proj-status" :class="statusInfo(h).cls">
            <span class="sdot"></span>{{ statusInfo(h).text }}
          </div>
          <div class="proj-arrow">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
                 stroke-linecap="round" stroke-linejoin="round">
              <polyline points="9 18 15 12 9 6"></polyline>
            </svg>
          </div>
        </div>
      </template>

      <!-- 卡片视图 -->
      <template v-else-if="viewStyle === 'card'">
        <div
          v-for="h in filtered"
          :key="h.id"
          class="project-item"
          :style="accent(h)"
          @click="onConnect(h)"
        >
          <div class="proj-head">
            <div class="proj-icon" :class="'icon-' + h.color">{{ h.label }}</div>
            <div class="proj-title">
              <div class="proj-name">{{ h.id }}</div>
              <div class="proj-tagline">{{ h.os }}</div>
            </div>
            <div class="proj-status" :class="statusInfo(h).cls">
              <span class="sdot"></span>{{ statusInfo(h).text }}
            </div>
          </div>

          <div class="proj-divider"></div>

          <div class="proj-kv"><span class="k">地址</span><span class="v">{{ h.ip }}:{{ h.port }}</span></div>
          <div class="proj-kv"><span class="k">用户</span><span class="v">{{ h.user }}</span></div>
          <div class="proj-kv"><span class="k">延迟</span><span class="v">{{ h.lat ? h.lat + ' ms' : '—' }}</span></div>
          <div class="proj-kv"><span class="k">运行</span><span class="v">{{ h.uptime }}</span></div>

          <div class="proj-foot">
            <span class="tag">{{ h.tag }}</span>
            <span class="tag">{{ h.group }}</span>
            <span class="spacer"></span>
            <span class="go">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4"
                   stroke-linecap="round" stroke-linejoin="round">
                <polyline points="9 18 15 12 9 6"></polyline>
              </svg>
            </span>
          </div>
        </div>
      </template>

      <!-- 网格视图 -->
      <template v-else-if="viewStyle === 'grid'">
        <div
          v-for="h in filtered"
          :key="h.id"
          class="project-item"
          @click="onConnect(h)"
        >
          <span class="proj-status" :class="statusInfo(h).cls"></span>
          <div class="proj-icon" :class="'icon-' + h.color">{{ h.label }}</div>
          <div class="proj-name">{{ h.id }}</div>
          <div class="proj-tagline">{{ h.ip }}</div>
        </div>
      </template>

      <!-- 紧凑视图 -->
      <template v-else>
        <div
          v-for="h in filtered"
          :key="h.id"
          class="project-item"
          @click="onConnect(h)"
        >
          <div class="proj-icon" :class="'icon-' + h.color">{{ h.label }}</div>
          <div class="proj-name">{{ h.id }}</div>
          <div class="proj-addr">{{ h.user }}@{{ h.ip }}:{{ h.port }}</div>
          <div class="proj-os">{{ h.os }}</div>
          <span class="proj-tag">{{ h.tag }}</span>
          <div class="proj-status" :class="statusInfo(h).cls">
            <span class="sdot"></span>{{ statusInfo(h).text }}
          </div>
        </div>
      </template>
    </template>

    <!-- 空状态 -->
    <div v-else class="empty-state">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"
           stroke-linecap="round" stroke-linejoin="round">
        <circle cx="11" cy="11" r="7"></circle>
        <line x1="21" y1="21" x2="16.7" y2="16.7"></line>
      </svg>
      <p>没有找到匹配的主机</p>
    </div>
  </div>
</template>
