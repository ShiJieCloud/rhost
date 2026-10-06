import { ref } from 'vue'

/**
 * 全局确认弹窗：替代 window.confirm（Tauri WKWebView 下默认不可用）。
 *
 * 用法：
 *   const ok = await confirmDialog('是否立即重启？', '重启应用', {
 *     confirmText: '立即重启',
 *     cancelText: '稍后',
 *   })
 *   // 用户确认 → true；取消/关闭/Esc → false
 *
 * 模块级单例：ConfirmModal 组件绑定 visible/message/title 等状态，
 * 调用方通过 confirmDialog() 拿 Promise，accept/reject 时 resolve。
 */
export interface ConfirmOptions {
  confirmText?: string
  cancelText?: string
  /** 确认按钮是否使用危险（红色）样式 */
  danger?: boolean
}

const visible = ref(false)
const title = ref('请确认')
const message = ref('')
const confirmText = ref('确定')
const cancelText = ref('取消')
const danger = ref(false)

let resolver: ((value: boolean) => void) | null = null

export function confirmDialog(
  msg: string,
  t = '请确认',
  opts: ConfirmOptions = {},
): Promise<boolean> {
  // 已有弹窗未关闭：先取消上一个（返回 false），再开新的
  if (resolver) {
    resolver(false)
    resolver = null
  }
  message.value = msg
  title.value = t
  confirmText.value = opts.confirmText ?? '确定'
  cancelText.value = opts.cancelText ?? '取消'
  danger.value = opts.danger ?? false
  visible.value = true
  return new Promise(resolve => {
    resolver = resolve
  })
}

export function acceptConfirm() {
  visible.value = false
  const r = resolver
  resolver = null
  r?.(true)
}

export function rejectConfirm() {
  visible.value = false
  const r = resolver
  resolver = null
  r?.(false)
}

export const confirmState = {
  visible,
  title,
  message,
  confirmText,
  cancelText,
  danger,
}
