<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import ContextMenu, { type MenuItem } from './ContextMenu.vue'
import {
  activeSessionId, closeSession, sessions,
  closeOtherSessions, closeSessionsToLeft, closeSessionsToRight,
  closeDisconnectedSessions, closeAllSessions,
  reconnectBackend, disconnectSession, setSessionAlias,
  duplicateSession, isDuplicating, openDock,
} from '../../stores/session'
import { showNewConn } from '../../stores/hosts'
import { toast } from '../../composables/useToast'
import type { Session } from '../../stores/session'

/* ---- 右键菜单状态 ---- */
interface MenuState { x: number; y: number; targetId: string | null }
const menu = ref<MenuState | null>(null)
const menuVisible = ref(false)

function closeMenu() { menu.value = null }

function dotCls(state: string) {
  return state === 'online' ? 'st-ok'
    : state === 'connecting' ? 'st-info'
    : state === 'reconnecting' ? 'st-warn'
    : state === 'idle' ? 'st-idle'
    : 'st-err'
}

/* ---- 标签右键菜单（targetId 非空） ---- */
const tabMenuItems = computed<MenuItem[]>(() => {
  const s = sessions.value.find(x => x.id === menu.value?.targetId)
  if (!s) return []
  const isBusy = s.state === 'connecting' || s.state === 'reconnecting'
  const idx = sessions.value.findIndex(x => x.id === s.id)
  const hasOther = sessions.value.length > 1
  const hasLeft = idx > 0
  const hasRight = idx < sessions.value.length - 1
  const hasOffline = sessions.value.some(x => x.state === 'offline')

  return [
    { label: '重命名标签', action: () => startRename(s.id) },
    {
      label: '复制会话',
      disabled: isBusy || isDuplicating(s.id),
      action: () => { void duplicateSession(s.id) },
    },
    {
      label: '重新连接',
      disabled: !['idle', 'offline', 'reconnecting'].includes(s.state),
      action: () => { activeSessionId.value = s.id; void reconnectBackend(s.id) },
    },
    {
      label: '断开连接',
      disabled: !['online', 'connecting', 'reconnecting'].includes(s.state),
      action: () => { void disconnectSession(s.id) },
    },
    { divider: true },
    { label: '关闭标签', action: () => closeSession(s.id) },
    { label: '关闭其他标签页', disabled: !hasOther, action: () => closeOtherSessions(s.id) },
    { label: '关闭左侧标签页', disabled: !hasLeft, action: () => closeSessionsToLeft(s.id) },
    { label: '关闭右侧标签页', disabled: !hasRight, action: () => closeSessionsToRight(s.id) },
    {
      label: '关闭已断开的标签',
      disabled: !hasOffline,
      action: () => { const n = closeDisconnectedSessions(); if (n > 0) toast(`已关闭 ${n} 个标签`) },
    },
    { label: '关闭所有标签页', action: () => closeAllSessions() },
    { divider: true },
    {
      label: '打开 SFTP 面板',
      disabled: s.state !== 'online',
      action: () => { activeSessionId.value = s.id; openDock('sftp') },
    },
  ]
})

/* ---- 空白区右键菜单（targetId 为 null） ---- */
const barMenuItems = computed<MenuItem[]>(() => [
  { label: '新建连接…', action: () => { showNewConn.value = true } },
  { divider: true },
  {
    label: '关闭已断开的标签',
    disabled: !sessions.value.some(s => s.state === 'offline'),
    action: () => { const n = closeDisconnectedSessions(); if (n > 0) toast(`已关闭 ${n} 个标签`) },
  },
  {
    label: '关闭全部标签',
    disabled: sessions.value.length === 0,
    action: () => closeAllSessions(),
  },
])

/* ---- 事件处理 ---- */
function onTabContext(e: MouseEvent, s: Session) {
  e.preventDefault()
  // 清除右键可能触发的文本选中：macOS 上 mousedown preventDefault 无法完全阻止
  // 右键启动选区（尤其是触控板双指轻点场景），在 contextmenu 时直接清空选区最可靠。
  window.getSelection()?.removeAllRanges()
  menu.value = { x: e.clientX, y: e.clientY, targetId: s.id }
  menuVisible.value = true
}

function onTabMouseDown(e: MouseEvent, s: Session) {
  if (e.button === 1) {
    e.preventDefault()
    closeSession(s.id)
  } else if (e.button === 2) {
    // 右键按下时阻止默认行为，避免浏览器在标签文本上启动文本选择；
    // contextmenu 事件不受影响，仍会正常弹出右键菜单。
    e.preventDefault()
  }
}

function onBarContext(e: MouseEvent) {
  const target = e.target as HTMLElement
  if (target.closest('.tab, .tab-arrow')) return
  e.preventDefault()
  menu.value = { x: e.clientX, y: e.clientY, targetId: null }
  menuVisible.value = true
}

/* ---- 横向滚动：派生状态 ---- */
const scrollEl = ref<HTMLDivElement | null>(null)
const canScroll = ref(false)
const atLeft = ref(true)
const atRight = ref(true)

const reducedMotion =
  typeof window !== 'undefined'
  && typeof window.matchMedia === 'function'
  && window.matchMedia('(prefers-reduced-motion: reduce)').matches

function update() {
  const el = scrollEl.value
  if (!el) return
  canScroll.value = el.scrollWidth - el.clientWidth > 1
  atLeft.value = el.scrollLeft <= 1
  atRight.value = el.scrollLeft + el.clientWidth >= el.scrollWidth - 1
}

// scroll 事件 rAF 节流：同一帧内多次滚动只重算一次
let rafId = 0
function scheduleUpdate() {
  if (rafId) return
  rafId = requestAnimationFrame(() => { rafId = 0; update() })
}

/* ---- 横向滚动：左键按住拖拽 ---- */
const DRAG_THRESHOLD = 5
let dragArmed = false    // mousedown 已按下、尚未判定
let dragMoved = false    // 位移超过阈值，已进入拖拽态
let dragStartX = 0
let dragStartLeft = 0
let dragTargetLeft = 0
let dragRaf = 0

function onDragStart(e: MouseEvent) {
  if (e.button !== 0) return
  const el = scrollEl.value
  if (!el) return
  // 关闭钮与重命名输入框上不启动拖拽，避免点关闭 / 编辑变成拖视窗
  const t = e.target as HTMLElement
  if (t.closest('.close, .rename-input')) return
  // mousedown 即 preventDefault：阻止文本选区启动；不影响后续 click 派发
  e.preventDefault()
  dragArmed = true
  dragMoved = false
  dragStartX = e.clientX
  dragStartLeft = el.scrollLeft
  window.addEventListener('mousemove', onDragMove)
  window.addEventListener('mouseup', onDragEnd)
}

function onDragMove(e: MouseEvent) {
  if (!dragArmed) return
  const dx = e.clientX - dragStartX
  if (!dragMoved) {
    if (Math.abs(dx) <= DRAG_THRESHOLD) return
    dragMoved = true
    scrollEl.value?.classList.add('dragging')
  }
  dragTargetLeft = dragStartLeft - dx
  // rAF 合帧：高频 mousemove 每帧最多写一次 scrollLeft
  if (dragRaf) return
  dragRaf = requestAnimationFrame(() => {
    dragRaf = 0
    if (scrollEl.value) scrollEl.value.scrollLeft = dragTargetLeft
  })
}

function onDragEnd() {
  dragArmed = false
  window.removeEventListener('mousemove', onDragMove)
  window.removeEventListener('mouseup', onDragEnd)
  if (dragRaf) { cancelAnimationFrame(dragRaf); dragRaf = 0 }
  if (dragMoved) {
    scrollEl.value?.classList.remove('dragging')
    // mouseup 后浏览器会按按下位置补发 click（会触发标签切换 / 关闭）；
    // 拖拽手势必须吞掉这次 click。document 捕获阶段一次性拦截。
    const swallow = (ev: Event) => {
      ev.stopPropagation()
      ev.preventDefault()
      document.removeEventListener('click', swallow, true)
    }
    document.addEventListener('click', swallow, true)
  }
}

/* ---- 横向滚动：普通纵向滚轮映射 ---- */
function onWheel(e: WheelEvent) {
  const el = scrollEl.value
  if (!el) return
  // Ctrl/Meta+滚轮是系统缩放手势，必须放行
  if (e.ctrlKey || e.metaKey) return
  // 触控板横向（或斜向以横向为主）交原生惯性处理
  if (Math.abs(e.deltaX) > Math.abs(e.deltaY)) return
  e.preventDefault()
  const raw = Math.abs(e.deltaY)
  let step: number
  if (e.deltaMode === 1) step = raw * 32        // 行模式
  else if (e.deltaMode === 2) step = raw * el.clientWidth // 页模式
  else step = Math.max(raw, 40)                // 像素模式：单步下限 40px
  el.scrollBy({ left: (e.deltaY > 0 ? 1 : -1) * step, behavior: 'auto' })
}

/* ---- 横向滚动：箭头按钮（单击翻 75% 视窗宽，长按持续滚动） ---- */
type Dir = -1 | 1
let holdTimer: number | undefined
let holdRaf: number | undefined
// 长按触发后置位：pointerup 后浏览器仍会补发 click，用来吞掉这次"翻页"
let didHold = false

function nudge(dir: Dir) {
  const el = scrollEl.value
  if (!el) return
  el.scrollBy({
    left: dir * el.clientWidth * 0.75,
    behavior: reducedMotion ? 'auto' : 'smooth',
  })
}

function onArrowClick(dir: Dir) {
  if (didHold) { didHold = false; return }
  nudge(dir)
}

function onArrowDown(e: PointerEvent, dir: Dir) {
  // 仅响应主键（左键），右键不启动长按（右键留给 contextmenu）
  if (e.button !== 0) return
  stopHold()
  holdTimer = window.setTimeout(() => {
    didHold = true
    const tick = () => {
      scrollEl.value?.scrollBy({ left: dir * 12, behavior: 'auto' })
      holdRaf = requestAnimationFrame(tick)
    }
    tick()
  }, 400)
}

function stopHold() {
  if (holdTimer !== undefined) { clearTimeout(holdTimer); holdTimer = undefined }
  if (holdRaf !== undefined) { cancelAnimationFrame(holdRaf); holdRaf = undefined }
}

/* ---- 横向滚动：激活标签自动滚入（最小位移原则） ---- */
const tabEls = new Map<string, HTMLElement>()
function setTabEl(id: string) {
  return (el: Element | { $el?: Element } | null) => {
    const node = (el instanceof Element ? el : el?.$el ?? null) as HTMLElement | null
    if (node) tabEls.set(id, node)
    else tabEls.delete(id)
  }
}

const PAD = 8
function ensureVisible(id: string | null, forceBehavior?: ScrollBehavior) {
  const el = scrollEl.value
  const tab = id ? tabEls.get(id) : null
  if (!el || !tab) return
  const { clientWidth, scrollLeft } = el
  const elLeft = tab.offsetLeft
  const elRight = elLeft + tab.offsetWidth
  // 已完整可见（含两侧 8px 留白）→ 不打扰，不滚动
  if (elLeft >= scrollLeft + PAD && elRight <= scrollLeft + clientWidth - PAD) return
  const behavior = forceBehavior ?? (reducedMotion ? 'auto' : 'smooth')
  if (elLeft < scrollLeft + PAD) {
    el.scrollTo({ left: elLeft - PAD, behavior })
  } else {
    el.scrollTo({ left: elRight - clientWidth + PAD, behavior })
  }
}

watch(activeSessionId, id => {
  // 激活/失活会即时重排标签宽度（90↔190，width 无过渡）；
  // 下一帧布局稳定后再按最终几何测量滚入，避免读到宽度变化中间态
  requestAnimationFrame(() => ensureVisible(id))
}, { flush: 'post' })
watch(() => sessions.value.length, () => void nextTick(update))

/* ---- 标签完整名 tooltip：仅文本被省略号截断时才提示 ---- */
function onNameEnter(s: Session, e: MouseEvent) {
  const nameEl = e.currentTarget as HTMLElement
  nameEl.title = nameEl.scrollWidth > nameEl.clientWidth ? (s.alias || s.host.id) : ''
}

/* ---- ResizeObserver / 监听注册与清理 ---- */
let ro: ResizeObserver | null = null

onMounted(() => {
  const el = scrollEl.value
  if (!el) return
  // 必须非 passive：onWheel 内需要 preventDefault
  el.addEventListener('wheel', onWheel, { passive: false })
  el.addEventListener('scroll', scheduleUpdate)
  el.addEventListener('mousedown', onDragStart)
  const track = el.firstElementChild
  if (typeof ResizeObserver !== 'undefined') {
    ro = new ResizeObserver(() => update())
    ro.observe(el)
    if (track) ro.observe(track)
  } else {
    window.addEventListener('resize', update)
  }
  update()
  // 初始定位（如 restoreSessions 已恢复标签）用瞬时滚动，不在启动时放动画
  ensureVisible(activeSessionId.value, 'auto')
})

onBeforeUnmount(() => {
  stopHold()
  if (rafId) cancelAnimationFrame(rafId)
  // 拖拽兜底清理：组件在拖拽中途卸载时移除 window 监听与 pending rAF
  window.removeEventListener('mousemove', onDragMove)
  window.removeEventListener('mouseup', onDragEnd)
  if (dragRaf) cancelAnimationFrame(dragRaf)
  const el = scrollEl.value
  if (el) {
    el.removeEventListener('wheel', onWheel)
    el.removeEventListener('scroll', scheduleUpdate)
    el.removeEventListener('mousedown', onDragStart)
  }
  ro?.disconnect()
  window.removeEventListener('resize', update)
})

/* ---- 重命名（inline 编辑） ---- */
const editingId = ref<string | null>(null)
const draft = ref('')
const skipSave = ref(false)
// 用函数 ref 而非字符串 ref：v-for 内的字符串 ref 会被 Vue 收集为数组，
// 导致 renameInput.value 是 [el] 而非 el，.focus()/.select() 失效。
const renameInputEl = ref<HTMLInputElement | null>(null)
function setRenameInput(el: Element | { $el?: Element } | null) {
  renameInputEl.value = (el instanceof Element ? el : el?.$el ?? null) as HTMLInputElement | null
}

function startRename(id: string) {
  const s = sessions.value.find(x => x.id === id)
  if (!s) return
  editingId.value = id
  // 预填当前显示的标签名（别名优先，空别名回落 host.id），而非仅 alias
  draft.value = s.alias || s.host.id
  skipSave.value = false
  void nextTick(() => {
    const el = renameInputEl.value
    if (!el) return
    el.focus()
    // select() 放在 rAF 中：nextTick 后浏览器可能仍在处理菜单关闭的
    // mousedown/click 事件链，立即 select 会被重置光标位置。rAF 确保
    // 在下一帧绘制前完成选中，避免被事件循环打断。
    requestAnimationFrame(() => el.select())
  })
}

function commitRename() {
  if (skipSave.value) { skipSave.value = false; editingId.value = null; return }
  const id = editingId.value
  if (id) {
    const el = renameInputEl.value
    const raw = el ? el.value : draft.value
    const trimmed = raw.trim()
    // 空输入不保存：别名保持原值不变，标签恢复原名。
    if (trimmed) setSessionAlias(id, trimmed)
  }
  editingId.value = null
}

function cancelRename() {
  skipSave.value = true
  renameInputEl.value?.blur()
}

function onRenameKeydown(e: KeyboardEvent) {
  if (e.key === 'Enter') {
    e.preventDefault()
    ;(e.target as HTMLInputElement).blur()
  } else if (e.key === 'Escape') {
    cancelRename()
  }
}

// 切换活动标签时丢弃未保存修改
watch(activeSessionId, () => {
  if (editingId.value && editingId.value !== activeSessionId.value) {
    cancelRename()
  }
})
</script>

<template>
  <div class="tabs-bar" @contextmenu="onBarContext">
    <button
      class="tab-arrow tab-arrow-left"
      :class="{ show: canScroll }"
      :aria-disabled="atLeft"
      title="向左滚动"
      @click="onArrowClick(-1)"
      @pointerdown="onArrowDown($event, -1)"
      @pointerup="stopHold"
      @pointerleave="stopHold"
      @pointercancel="stopHold"
    >‹</button>

    <div class="tabs-viewport">
      <div ref="scrollEl" class="tabs-scroll">
        <div class="tabs-track">
          <div
            v-for="s in sessions"
            :key="s.id"
            :ref="setTabEl(s.id)"
            class="tab"
            :class="{ active: activeSessionId === s.id }"
            @click="activeSessionId = s.id"
            @contextmenu="onTabContext($event, s)"
            @mousedown="onTabMouseDown($event, s)"
          >
            <span class="dot" :class="dotCls(s.state)"></span>
            <input
              v-if="editingId === s.id"
              :ref="setRenameInput"
              :value="draft"
              class="rename-input"
              maxlength="50"
              autocapitalize="off"
              autocorrect="off"
              spellcheck="false"
              @input="draft = ($event.target as HTMLInputElement).value"
              @keydown="onRenameKeydown"
              @blur="commitRename"
              @mousedown.stop
            >
            <span v-else class="name" @mouseenter="onNameEnter(s, $event)">{{ s.alias || s.host.id }}</span>
            <span class="close" @click.stop="closeSession(s.id)">×</span>
          </div>
        </div>
      </div>
    </div>

    <button
      class="tab-arrow tab-arrow-right"
      :class="{ show: canScroll }"
      :aria-disabled="atRight"
      title="向右滚动"
      @click="onArrowClick(1)"
      @pointerdown="onArrowDown($event, 1)"
      @pointerup="stopHold"
      @pointerleave="stopHold"
      @pointercancel="stopHold"
    >›</button>

    <ContextMenu
      v-if="menu"
      v-model:visible="menuVisible"
      :x="menu.x"
      :y="menu.y"
      :items="menu.targetId !== null ? tabMenuItems : barMenuItems"
      @update:visible="(v) => { if (!v) closeMenu() }"
    />
  </div>
</template>
