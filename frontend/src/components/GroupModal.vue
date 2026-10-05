<script setup lang="ts">
import { nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { toast } from '../composables/useToast'
import { COLOR_MAP } from '../data/mockHosts'
import {
  addGroup,
  allGroups,
  closeGroupModal,
  editingGroup,
  showGroupModal,
  updateGroup,
} from '../stores/groups'
import type { HostColor } from '../types'

/** 色板使用项目 COLOR_MAP 的 6 个颜色键，与主机配色体系一致 */
const COLORS = Object.keys(COLOR_MAP) as HostColor[]

const name = ref('')
const picked = ref<HostColor>('green')
const nameInvalid = ref(false)
const dupError = ref('')
const shaking = ref(false)
const inputEl = ref<HTMLInputElement | null>(null)

watch(showGroupModal, v => {
  if (!v) return
  name.value = editingGroup.value?.name ?? ''
  // 新建时默认色按现有分组数轮转，避免同色扎堆
  picked.value = editingGroup.value?.color ?? COLORS[allGroups.value.length % COLORS.length]!
  nameInvalid.value = false
  dupError.value = ''
  nextTick(() => setTimeout(() => inputEl.value?.focus(), 180))
})

function onInput() {
  nameInvalid.value = false
  dupError.value = ''
}

function pick(c: HostColor) {
  picked.value = c
}

function shake() {
  shaking.value = false
  requestAnimationFrame(() => {
    shaking.value = true
  })
  setTimeout(() => (shaking.value = false), 400)
}

function save() {
  const n = name.value.trim()
  if (!n) {
    nameInvalid.value = true
    dupError.value = ''
    shake()
    inputEl.value?.focus()
    return
  }
  const editing = editingGroup.value
  const ok = editing
    ? updateGroup(editing.name, n, picked.value)
    : addGroup(n, picked.value)
  if (!ok) {
    dupError.value = `分组「${n}」已存在`
    nameInvalid.value = true
    shake()
    inputEl.value?.focus()
    return
  }
  toast(editing ? `分组「${n}」已更新` : `已创建分组「${n}」`, 'ok', 2200)
  closeGroupModal()
}

function onKey(e: KeyboardEvent) {
  if (!showGroupModal.value) return
  if (e.key === 'Escape') {
    e.preventDefault()
    closeGroupModal()
  } else if (e.key === 'Enter') {
    e.preventDefault()
    save()
  }
}

onMounted(() => document.addEventListener('keydown', onKey))
onUnmounted(() => document.removeEventListener('keydown', onKey))
</script>

<template>
  <div class="mask" :class="{ show: showGroupModal }" @click.self="closeGroupModal">
    <div class="modal gm-modal" role="dialog" aria-modal="true" aria-labelledby="gmTitle">
      <!-- 头部 -->
      <div class="modal-head">
        <div class="modal-icon">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
               stroke-linecap="round" stroke-linejoin="round">
            <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
          </svg>
        </div>
        <div class="modal-title">
          <h2 id="gmTitle">{{ editingGroup ? '编辑分组' : '新建分组' }}</h2>
          <p>{{ editingGroup ? `重命名或更换「${editingGroup.name}」的标识颜色` : '按环境或用途归类主机' }}</p>
        </div>
        <button class="modal-close" title="关闭" @click="closeGroupModal">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
               stroke-linecap="round">
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </button>
      </div>

      <!-- 表单 -->
      <div class="gm-body">
        <div
          class="field"
          :class="{ 'has-error': nameInvalid, 'gm-shake': shaking }"
        >
          <label>分组名称<span class="req">*</span></label>
          <input
            ref="inputEl"
            v-model="name"
            type="text"
            :class="{ invalid: nameInvalid }"
            placeholder="例如：生产环境"
            maxlength="20"
            spellcheck="false"
            autocomplete="off"
            autocapitalize="off"
            autocorrect="off"
            @input="onInput"
          />
          <span class="error-text">{{ dupError || '请输入分组名称' }}</span>
        </div>

        <div class="field">
          <label>标识颜色</label>
          <div class="gm-colors">
            <button
              v-for="c in COLORS"
              :key="c"
              type="button"
              class="gm-swatch"
              :class="{ active: picked === c }"
              :style="{ background: COLOR_MAP[c], color: COLOR_MAP[c] }"
              :title="c"
              @click="pick(c)"
            ></button>
          </div>
        </div>
      </div>

      <!-- 底部按钮 -->
      <div class="modal-foot">
        <span class="gm-hint"><kbd>Enter</kbd> 保存 · <kbd>Esc</kbd> 取消</span>
        <span class="spacer"></span>
        <button type="button" class="btn ghost" @click="closeGroupModal">取消</button>
        <button type="button" class="btn primary" @click="save">确定</button>
      </div>
    </div>
  </div>
</template>
