import { ref } from 'vue'

/**
 * 全局密码弹窗：替代 window.prompt（Tauri WKWebView 下默认不可用）。
 *
 * 用法：
 *   const pw = await promptPassword('输入 root@1.2.3.4 的登录密码：')
 *   // 用户取消 → null；空密码 → ''
 *
 * 模块级单例：PasswordModal 组件绑定 visible/message/title，
 * 调用方通过 prompt() 拿 Promise，submit/cancel 时 resolve。
 */
const visible = ref(false)
const title = ref('请输入密码')
const message = ref('')
/** 输入框占位符：密码场景默认"输入登录密码"，私钥口令场景由调用方覆盖 */
const placeholder = ref('输入登录密码')

let resolver: ((value: string | null) => void) | null = null

export function promptPassword(
  msg: string,
  t = '请输入密码',
  ph = '输入登录密码',
): Promise<string | null> {
  // 已有弹窗未关闭：先取消上一个（返回 null），再开新的
  if (resolver) {
    resolver(null)
    resolver = null
  }
  message.value = msg
  title.value = t
  placeholder.value = ph
  visible.value = true
  return new Promise(resolve => {
    resolver = resolve
  })
}

export function submitPassword(value: string) {
  visible.value = false
  const r = resolver
  resolver = null
  r?.(value)
}

export function cancelPassword() {
  visible.value = false
  const r = resolver
  resolver = null
  r?.(null)
}

export const passwordPromptState = { visible, title, message, placeholder }
