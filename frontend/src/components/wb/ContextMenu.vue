<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from 'vue'

/** 菜单项：divider=true 时渲染为分隔符行（不绑定 click，与动作项显式分支） */
export interface MenuItem {
  label?: string
  icon?: string // 内联 SVG 的 path d（24x24 viewBox）
  shortcut?: string
  disabled?: boolean
  danger?: boolean
  divider?: boolean
  action?: () => void
}

/* v-model:visible 双向契约：父级只通过 visible 控制显隐，关闭一律 emit 更新 */
const visible = defineModel<boolean>('visible', { default: false })
const props = defineProps<{ x: number; y: number; items: MenuItem[] }>()

const MENU_W = 184
const ITEM_H = 28
const DIV_H = 9
const PAD_Y = 4

/* 菜单高度预估（与 CSS 固定行高一致 + 上下边框 2px），用于视口边缘检测 */
const estH = computed(() =>
  props.items.reduce((h, i) => h + (i.divider ? DIV_H : ITEM_H), PAD_Y * 2 + 2),
)

/* 边缘夹取后的实际渲染坐标；四周预留 GAP，且 max-height 兜底可滚动 */
const pos = ref({ left: 0, top: 0 })
const GAP = 4
function clampPos() {
  const vh = window.innerHeight
  const vw = window.innerWidth
  let left = props.x
  let top = props.y
  if (left + MENU_W > vw - GAP) left = Math.max(GAP, vw - MENU_W - GAP)
  if (top + estH.value > vh - GAP) top = Math.max(GAP, vh - estH.value - GAP)
  pos.value = { left, top }
}

/* 键盘导航 */
const activeIdx = ref(-1)
const actionIdx = computed(() =>
  props.items.map((i, idx) => (!i.divider && !i.disabled ? idx : -1)).filter(i => i >= 0),
)

watch(visible, (v) => {
  if (v) {
    clampPos()
    activeIdx.value = -1
  }
}, { immediate: true })
/* 菜单开着时再次右键会重建 items（新数组引用），重夹取位置让菜单跟随新坐标 */
watch(() => props.items, () => {
  activeIdx.value = -1
  if (visible.value) clampPos()
})

function pick(idx: number) {
  const item = props.items[idx]
  if (!item || item.disabled || item.divider) return
  visible.value = false
  item.action?.()
}

function onKeydown(e: KeyboardEvent) {
  if (!visible.value) return
  const isModifierCombo = e.ctrlKey || e.metaKey || e.altKey
  if (e.key === 'Escape') {
    visible.value = false
    e.stopPropagation()
    return
  }
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    e.preventDefault()
    const n = actionIdx.value.length
    if (!n) { e.stopPropagation(); return }
    let p = actionIdx.value.indexOf(activeIdx.value)
    p = e.key === 'ArrowDown' ? (p + 1) % n : (p - 1 + n) % n
    activeIdx.value = actionIdx.value[p]!
    e.stopPropagation()
    return
  }
  if (e.key === 'Enter') {
    if (activeIdx.value >= 0) pick(activeIdx.value)
    e.stopPropagation()
    return
  }
  // 非导航键：修饰键组合（Ctrl/Cmd/Alt+key）放行到终端（复制粘贴等）；
  // 普通单字符 stopPropagation，阻止穿透到 xterm textarea / PTY
  if (!isModifierCombo) {
    e.stopPropagation()
  }
}

/* 菜单外点击 / 外部容器滚动 / 窗口尺寸变化 → 关闭。mousedown 用捕获阶段，
   早于其它元素的 click 处理，避免误触列表行。菜单自身内部滚动不关闭 */
function onDocMouseDown(e: MouseEvent) {
  const el = menuEl.value
  if (el && !el.contains(e.target as Node)) visible.value = false
}
function onScrollClose(e: Event) {
  if (menuEl.value?.contains(e.target as Node)) return
  if (visible.value) visible.value = false
}
function onClose() { if (visible.value) visible.value = false }

const menuEl = ref<HTMLElement | null>(null)

function addListeners() {
  document.addEventListener('keydown', onKeydown, true)
  document.addEventListener('mousedown', onDocMouseDown, true)
  document.addEventListener('scroll', onScrollClose, true)
  window.addEventListener('resize', onClose)
}
function removeListeners() {
  document.removeEventListener('keydown', onKeydown, true)
  document.removeEventListener('mousedown', onDocMouseDown, true)
  document.removeEventListener('scroll', onScrollClose, true)
  window.removeEventListener('resize', onClose)
}

watch(visible, (v) => {
  if (v) addListeners()
  else removeListeners()
}, { immediate: true })

/* 兜底：组件卸载时强制移除所有监听器。
 * 正常关闭流程中 visible→false 的 watch 回调会异步执行 removeListeners，
 * 但菜单关闭会同时触发父级 v-if 卸载组件，若卸载先于 watch 刷新发生，
 * 监听器会泄漏并持续拦截终端按键（Enter 等 stopPropagation）。
 * onUnmounted 确保无论时序如何，监听器必被清理。 */
onUnmounted(removeListeners)
</script>

<template>
  <Teleport to="body">
    <div v-if="visible" ref="menuEl" class="ctx-menu"
         :style="{ left: pos.left + 'px', top: pos.top + 'px', width: MENU_W + 'px' }">
      <template v-for="(item, idx) in items" :key="idx">
        <div v-if="item.divider" class="ctx-sep"></div>
        <div v-else class="ctx-item"
             :class="{ disabled: item.disabled, danger: item.danger, active: activeIdx === idx }"
             @click="pick(idx)" @mouseenter="activeIdx = idx">
          <svg class="ctx-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor"
               stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
            <path v-if="item.icon" :d="item.icon"/>
          </svg>
          <span class="ctx-label">{{ item.label }}</span>
          <span v-if="item.shortcut" class="ctx-shortcut">{{ item.shortcut }}</span>
        </div>
      </template>
    </div>
  </Teleport>
</template>

<style scoped>
.ctx-menu{
  position:fixed;z-index:9999;
  padding:4px 0;
  max-height:calc(100vh - 8px);
  overflow-y:auto;
  background:rgba(13,19,28,.97);
  border:1px solid var(--border);
  border-radius:7px;
  box-shadow:0 10px 34px rgba(0,0,0,.52),0 0 0 1px rgba(255,255,255,.02);
  backdrop-filter:blur(8px);
  user-select:none;
}
.ctx-item{
  display:flex;align-items:center;gap:8px;
  height:28px;padding:0 10px;margin:0 4px;
  border-radius:5px;
  font-size:12px;color:var(--text);
  cursor:pointer;white-space:nowrap;
}
.ctx-item.active{background:var(--hover)}
.ctx-item.disabled{color:var(--muted-2);cursor:default}
.ctx-item.danger{color:var(--red)}
.ctx-icon{width:14px;height:14px;flex-shrink:0;color:var(--muted)}
.ctx-item.danger .ctx-icon{color:var(--red)}
.ctx-label{flex:1;overflow:hidden;text-overflow:ellipsis}
.ctx-shortcut{font-size:11px;color:var(--muted-2);margin-left:10px}
.ctx-sep{height:1px;margin:4px 10px;background:var(--border)}
</style>
