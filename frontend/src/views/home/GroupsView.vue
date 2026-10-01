<script setup lang="ts">
import { computed, onUnmounted, ref } from 'vue'
import { toast } from '../../composables/useToast'
import { COLOR_MAP } from '../../data/mockHosts'
import {
  FALLBACK_GROUP,
  groupStats,
  openGroupModal,
  removeGroup,
  type GroupStat,
} from '../../stores/groups'
import { setHomeView } from '../../stores/homeView'
import { filter, hosts } from '../../stores/hosts'

/* ================= 搜索筛选 ================= */
const q = ref('')

const visible = computed<GroupStat[]>(() => {
  const k = q.value.trim().toLowerCase()
  if (!k) return groupStats.value
  return groupStats.value.filter(
    s =>
      s.def.name.toLowerCase().includes(k) ||
      s.hosts.some(h => h.id.toLowerCase().includes(k)),
  )
})

const filterInfo = computed(() =>
  q.value.trim() ? `筛选出 ${visible.value.length} 个分组` : '无筛选',
)

const onlineTotal = computed(
  () => hosts.value.filter(h => h.status === 'online').length,
)

/* ================= 跳转主机视图 ================= */
function openGroup(s: GroupStat) {
  filter.value = s.def.name
  setHomeView('hosts')
}

/* ================= 新建 / 编辑（弹窗） ================= */
function startCreate() {
  openGroupModal()
}

function startRename(e: MouseEvent, s: GroupStat) {
  e.stopPropagation()
  clearConfirm()
  openGroupModal(s.def)
}

/* ================= 删除：两步内联确认（与主机列表一致） ================= */
const confirmingName = ref<string | null>(null)
let confirmTimer: ReturnType<typeof setTimeout> | null = null

function clearConfirm() {
  confirmingName.value = null
  if (confirmTimer) {
    clearTimeout(confirmTimer)
    confirmTimer = null
  }
}

function onDeleteClick(e: MouseEvent, s: GroupStat) {
  e.stopPropagation()
  if (s.def.name === FALLBACK_GROUP) return
  if (confirmingName.value === s.def.name) {
    clearConfirm()
    const moved = removeGroup(s.def.name)
    toast(
      moved
        ? `已删除分组「${s.def.name}」，${moved} 台主机移入「${FALLBACK_GROUP}」`
        : `已删除分组「${s.def.name}」`,
      'ok',
      2400,
    )
    return
  }
  confirmingName.value = s.def.name
  if (confirmTimer) clearTimeout(confirmTimer)
  confirmTimer = setTimeout(() => {
    confirmingName.value = null
    confirmTimer = null
  }, 2600)
}

onUnmounted(clearConfirm)

/* ================= 展示辅助 ================= */
function accent(s: GroupStat) {
  return { '--accent': COLOR_MAP[s.def.color] }
}
</script>

<template>
  <!-- 顶部工具栏：复用全局 toolbar / search-box / btn 体系 -->
  <div class="toolbar">
    <div class="search-box">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
           stroke-linecap="round" stroke-linejoin="round">
        <circle cx="11" cy="11" r="7"></circle>
        <line x1="21" y1="21" x2="16.7" y2="16.7"></line>
      </svg>
      <input v-model="q" type="text" spellcheck="false" placeholder="搜索分组或主机名…" />
    </div>
    <div class="toolbar-actions">
      <button type="button" class="btn primary" @click="startCreate">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4"
             stroke-linecap="round">
          <path d="M12 5v14M5 12h14" />
        </svg>
        新建分组
      </button>
    </div>
  </div>

  <div class="divider"></div>

  <div class="list-meta">
    共 <b>{{ groupStats.length }}</b> 个分组
    <span class="sep">·</span>
    <span>覆盖 <b>{{ hosts.length }}</b> 台主机（在线 <b>{{ onlineTotal }}</b>）</span>
    <span class="sep">·</span>
    <span>{{ filterInfo }}</span>
  </div>

  <div class="gp-list">
    <template v-if="visible.length">
      <div
        v-for="s in visible"
        :key="s.def.name"
        class="gp-row"
        :style="accent(s)"
        @click="openGroup(s)"
      >
        <div class="gp-icon" :class="'icon-' + s.def.color">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
               stroke-linecap="round" stroke-linejoin="round">
            <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
          </svg>
        </div>

        <div class="gp-main">
          <div class="gp-name">{{ s.def.name }}</div>
          <div class="gp-count">
            {{ s.total }} 台主机
            <template v-if="s.total">
              · 在线 <span class="gp-state ok">{{ s.online }}</span>
              · 告警 <span class="gp-state warn">{{ s.warn }}</span>
              · 离线 <span class="gp-state off">{{ s.offline }}</span>
            </template>
          </div>
        </div>

        <span class="proj-actions gp-actions" @mouseleave="clearConfirm">
          <button type="button" class="proj-act" title="查看分组主机" @click.stop="openGroup(s)">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4"
                 stroke-linecap="round" stroke-linejoin="round">
              <path d="M5 12h14" />
              <path d="m12 5 7 7-7 7" />
            </svg>
          </button>
          <button type="button" class="proj-act" title="编辑分组" @click="startRename($event, s)">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                 stroke-linecap="round" stroke-linejoin="round">
              <path d="M17 3a2.83 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"></path>
            </svg>
          </button>
          <button
            type="button"
            class="proj-act del"
            :class="{ armed: confirmingName === s.def.name }"
            :title="
              s.def.name === FALLBACK_GROUP
                ? '兜底分组不可删除'
                : confirmingName === s.def.name
                  ? '再次点击确认删除'
                  : '删除分组（主机移入「' + FALLBACK_GROUP + '」）'
            "
            :disabled="s.def.name === FALLBACK_GROUP"
            @click="onDeleteClick($event, s)"
          >
            <svg v-if="confirmingName !== s.def.name" viewBox="0 0 24 24" fill="none"
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

    <!-- 空状态 -->
    <div v-else class="empty-state">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"
           stroke-linecap="round" stroke-linejoin="round">
        <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
      </svg>
      <p>没有找到匹配的分组</p>
    </div>
  </div>
</template>
