import { ref } from 'vue'
import type { ToastType } from '../types'

export interface ToastItem {
  id: number
  msg: string
  type: ToastType
  leaving: boolean
}

const toasts = ref<ToastItem[]>([])
let seq = 0

export function useToasts() {
  return toasts
}

/** 立即关闭某条通知（带退出动画）；到时长自动关闭也走这里。 */
export function dismissToast(id: number) {
  const item = toasts.value.find(t => t.id === id)
  if (!item || item.leaving) return
  item.leaving = true
  setTimeout(() => {
    toasts.value = toasts.value.filter(t => t.id !== id)
  }, 200)
}

export function toast(msg: string, type: ToastType = 'ok', dur = 2000) {
  const id = ++seq
  toasts.value.push({ id, msg, type, leaving: false })
  setTimeout(() => dismissToast(id), dur)
}
