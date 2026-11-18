<script setup lang="ts">
/**
 * 锁密码管理编辑器（设置 → 安全；app-lock-design.md §6.3）。
 *
 * 折叠态显示密码槽位状态 + 「设置 / 修改 / 清除」按钮；展开后行内表单：
 * - 首次设置：新密码 + 确认密码（oldPassword 传 null）
 * - 修改：旧密码 + 新密码 + 确认
 * - 清除：走全局密码弹窗验证后调 lock_clear_password
 * 成功后 refreshLockSlots() 同步槽位，标签锁 / 应用锁入口即时出现。
 */
import { computed, ref } from 'vue'
import { lockSetPassword, lockClearPassword, type LockKind } from '../lib/lock'
import { lockSlots, refreshLockSlots } from '../stores/appLock'
import { toast } from '../composables/useToast'
import { promptPassword } from '../composables/usePasswordPrompt'
import { confirmDialog } from '../composables/useConfirm'
import { isTauri } from '../lib/tauri'

const props = defineProps<{ kind: LockKind }>()

/** 密码槽位清除成功后发出：父级负责收回依赖该槽位的开关并解除受影响的会话锁，
 *  避免「槽位已消失但 locked 仍在」的永久死锁 */
const emit = defineEmits<{ cleared: [] }>()

const passwordSet = computed(() =>
  props.kind === 'app' ? lockSlots.value.appPasswordSet : lockSlots.value.sessionPasswordSet,
)
const kindLabel = computed(() => (props.kind === 'app' ? '应用锁' : '会话锁'))

const PWD_MIN = 6
const PWD_MAX = 128

type Mode = 'idle' | 'set' | 'change'
const mode = ref<Mode>('idle')
const busy = ref(false)
const error = ref('')

const oldPwd = ref('')
const newPwd = ref('')
const confirmPwd = ref('')

function reset() {
  mode.value = 'idle'
  oldPwd.value = ''
  newPwd.value = ''
  confirmPwd.value = ''
  error.value = ''
}

function validate(): boolean {
  if (newPwd.value.length < PWD_MIN || newPwd.value.length > PWD_MAX) {
    error.value = `密码长度需为 ${PWD_MIN}~${PWD_MAX} 个字符`
    return false
  }
  if (newPwd.value !== confirmPwd.value) {
    error.value = '两次输入的新密码不一致'
    return false
  }
  if (mode.value === 'change' && !oldPwd.value) {
    error.value = '请输入当前密码'
    return false
  }
  return true
}

async function submit() {
  error.value = ''
  if (!validate()) return
  busy.value = true
  try {
    await lockSetPassword(
      props.kind,
      newPwd.value,
      mode.value === 'change' ? oldPwd.value : null,
    )
    await refreshLockSlots()
    toast(`${kindLabel.value}密码已${mode.value === 'change' ? '修改' : '设置'}`, 'ok', 2000)
    reset()
  } catch (e) {
    error.value = typeof e === 'string' ? e : (e as Error)?.message ?? String(e)
  } finally {
    busy.value = false
  }
}

async function clearPwd() {
  const ok = await confirmDialog(
    `清除后${kindLabel.value}将不再受密码保护，需重新设置密码后才能再次使用。确认清除？`,
    `清除${kindLabel.value}密码`,
    { confirmText: '去验证密码', danger: true },
  )
  if (!ok) return
  const pwd = await promptPassword(
    `清除${kindLabel.value}密码需要验证当前密码`,
    '验证密码',
    '输入当前密码',
  )
  if (pwd == null) return
  busy.value = true
  try {
    await lockClearPassword(props.kind, pwd)
    await refreshLockSlots()
    toast(`${kindLabel.value}密码已清除`, 'ok', 2000)
    emit('cleared')
  } catch (e) {
    toast(typeof e === 'string' ? e : (e as Error)?.message ?? String(e), 'warn', 3000)
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <div class="lock-pwd-editor">
    <!-- 折叠态：状态 + 操作按钮 -->
    <div v-if="mode === 'idle'" class="lpe-head">
      <span class="lpe-status" :class="{ set: passwordSet }">
        {{ passwordSet ? '已设置密码' : '尚未设置密码' }}
      </span>
      <template v-if="isTauri">
        <button class="btn" @click="mode = passwordSet ? 'change' : 'set'">
          {{ passwordSet ? '修改密码' : '设置密码' }}
        </button>
        <button v-if="passwordSet" class="btn" :disabled="busy" @click="clearPwd">清除密码</button>
      </template>
    </div>

    <!-- 行内表单 -->
    <div v-else class="lpe-form">
      <input
        v-if="mode === 'change'"
        v-model="oldPwd"
        class="st-input"
        type="password"
        placeholder="当前密码"
        autocomplete="current-password"
        spellcheck="false"
      >
      <input
        v-model="newPwd"
        class="st-input"
        type="password"
        :placeholder="`新密码（${PWD_MIN}~${PWD_MAX} 个字符）`"
        autocomplete="new-password"
        spellcheck="false"
      >
      <input
        v-model="confirmPwd"
        class="st-input"
        type="password"
        placeholder="确认新密码"
        autocomplete="new-password"
        spellcheck="false"
        @keydown.enter.prevent="submit"
      >
      <div v-if="error" class="lpe-error">{{ error }}</div>
      <div class="lpe-actions">
        <button class="btn" :disabled="busy" @click="submit">{{ busy ? '提交中…' : '确定' }}</button>
        <button class="st-btn-ghost" :disabled="busy" @click="reset">取消</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.lpe-head {
  display: flex;
  align-items: center;
  gap: 10px;
}
.lpe-status {
  font-size: 12px;
  color: var(--muted);
}
.lpe-status.set {
  color: var(--rhost-status-success, var(--green));
}
.lpe-form {
  display: flex;
  flex-direction: column;
  gap: 8px;
  max-width: 320px;
}
.lpe-error {
  font-size: 12px;
  color: var(--red, #e5534b);
}
.lpe-actions {
  display: flex;
  gap: 8px;
}
</style>
