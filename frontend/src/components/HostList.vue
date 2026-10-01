<script setup lang="ts">
import { onUnmounted, ref } from 'vue'
import { toast } from '../composables/useToast'
import { filtered, openEdit, removeHost, viewStyle } from '../stores/hosts'
import { closeSession, openSession } from '../stores/session'
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

/* ---- 编辑 ---- */
function onEdit(e: MouseEvent, h: Host) {
  e.stopPropagation()
  clearConfirm()
  openEdit(h)
}

/* ---- 删除：两步内联确认（首次点击进入 2.6s 待确认态，再次点击执行） ---- */
const confirmingId = ref<string | null>(null)
let confirmTimer: ReturnType<typeof setTimeout> | null = null

function clearConfirm() {
  confirmingId.value = null
  if (confirmTimer) {
    clearTimeout(confirmTimer)
    confirmTimer = null
  }
}

function onDeleteClick(e: MouseEvent, h: Host) {
  e.stopPropagation()
  if (confirmingId.value === h.id) {
    clearConfirm()
    removeHost(h.id)
    closeSession(h.id) // 若工作台中已打开该会话，一并关闭
    toast(`已删除连接「${h.id}」`, 'ok', 2200)
    return
  }
  confirmingId.value = h.id
  if (confirmTimer) clearTimeout(confirmTimer)
  confirmTimer = setTimeout(() => {
    confirmingId.value = null
    confirmTimer = null
  }, 2600)
}

onUnmounted(clearConfirm)
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
          <div class="proj-actions" @mouseleave="clearConfirm">
            <button type="button" class="proj-act" title="编辑连接" @click="onEdit($event, h)">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                   stroke-linecap="round" stroke-linejoin="round">
                <path d="M17 3a2.83 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"></path>
              </svg>
            </button>
            <button
              type="button"
              class="proj-act del"
              :class="{ armed: confirmingId === h.id }"
              :title="confirmingId === h.id ? '再次点击确认删除' : '删除连接'"
              @click="onDeleteClick($event, h)"
            >
              <svg v-if="confirmingId !== h.id" viewBox="0 0 24 24" fill="none"
                   stroke="currentColor" stroke-width="2" stroke-linecap="round"
                   stroke-linejoin="round">
                <polyline points="3 6 5 6 21 6"></polyline>
                <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"></path>
                <path d="M10 11v6M14 11v6"></path>
                <path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2"></path>
              </svg>
              <span v-else class="del-confirm">确认删除</span>
            </button>
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
            <span class="proj-actions" @mouseleave="clearConfirm">
              <button type="button" class="proj-act" title="编辑连接" @click="onEdit($event, h)">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                     stroke-linecap="round" stroke-linejoin="round">
                  <path d="M17 3a2.83 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"></path>
                </svg>
              </button>
              <button
                type="button"
                class="proj-act del"
                :class="{ armed: confirmingId === h.id }"
                :title="confirmingId === h.id ? '再次点击确认删除' : '删除连接'"
                @click="onDeleteClick($event, h)"
              >
                <svg v-if="confirmingId !== h.id" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="2" stroke-linecap="round"
                     stroke-linejoin="round">
                  <polyline points="3 6 5 6 21 6"></polyline>
                  <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"></path>
                  <path d="M10 11v6M14 11v6"></path>
                  <path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2"></path>
                </svg>
                <span v-else class="del-confirm">确认删除</span>
              </button>
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
          <span class="proj-actions" @mouseleave="clearConfirm">
            <button type="button" class="proj-act" title="编辑连接" @click="onEdit($event, h)">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                   stroke-linecap="round" stroke-linejoin="round">
                <path d="M17 3a2.83 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"></path>
              </svg>
            </button>
            <button
              type="button"
              class="proj-act del"
              :class="{ armed: confirmingId === h.id }"
              :title="confirmingId === h.id ? '再次点击确认删除' : '删除连接'"
              @click="onDeleteClick($event, h)"
            >
              <svg v-if="confirmingId !== h.id" viewBox="0 0 24 24" fill="none"
                   stroke="currentColor" stroke-width="2" stroke-linecap="round"
                   stroke-linejoin="round">
                <polyline points="3 6 5 6 21 6"></polyline>
                <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"></path>
                <path d="M10 11v6M14 11v6"></path>
                <path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2"></path>
              </svg>
              <span v-else class="del-confirm">确认删除</span>
            </button>
          </span>
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
          <span class="proj-actions" @mouseleave="clearConfirm">
            <button type="button" class="proj-act" title="编辑连接" @click="onEdit($event, h)">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                   stroke-linecap="round" stroke-linejoin="round">
                <path d="M17 3a2.83 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"></path>
              </svg>
            </button>
            <button
              type="button"
              class="proj-act del"
              :class="{ armed: confirmingId === h.id }"
              :title="confirmingId === h.id ? '再次点击确认删除' : '删除连接'"
              @click="onDeleteClick($event, h)"
            >
              <svg v-if="confirmingId !== h.id" viewBox="0 0 24 24" fill="none"
                   stroke="currentColor" stroke-width="2" stroke-linecap="round"
                   stroke-linejoin="round">
                <polyline points="3 6 5 6 21 6"></polyline>
                <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"></path>
                <path d="M10 11v6M14 11v6"></path>
                <path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2"></path>
              </svg>
              <span v-else class="del-confirm">确认删除</span>
            </button>
          </span>
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
