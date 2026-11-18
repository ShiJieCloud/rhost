/**
 * 应用锁全局状态（模块级单例，app-lock-design.md §6.4/§6.7）。
 *
 * - `appLocked` 为内存态：初值由 initAppLock() 在启动序列按
 *   「已启用 && 密码槽位存在」判定，启用后冷启动 splash 撤下即锁屏；
 * - 锁定只做 UI 闸门：遮罩渲染、夺走终端焦点、停 AI 流（AiChatPanel
 *   watcher 各自停止自己的 scope）、监控门控（Inspector/StatusBar watcher）；
 * - 不中断 SSH 连接、进行中的 SFTP 传输与端口转发。
 */
import { ref } from 'vue'
import { isTauri } from '../lib/tauri'
import { blurAllTerms } from '../lib/termRegistry'
import { lockStatus as ipcLockStatus, lockUnlockOrReset } from '../lib/lock'
import { persistSettings, savedSettings } from './settings'
import { toast } from '../composables/useToast'

export type LockReason = 'manual' | 'idle' | 'blur' | 'startup'

/** 应用当前是否处于锁屏态（遮罩、输入拦截、监控门控统一读此状态） */
export const appLocked = ref(false)

/** 锁屏原因（遮罩文案/埋点区分） */
export const appLockReason = ref<LockReason>('manual')

/** 锁屏瞬间的焦点元素，解锁后尽量还原 */
let activeBeforeLock: HTMLElement | null = null

/** 后端锁槽位状态（密码是否已设置 / lock.json 是否损坏） */
export const lockSlots = ref({ appPasswordSet: false, sessionPasswordSet: false })

/* ---- 失焦锁抑制 ----
 * 原生文件对话框（plugin-dialog 的 open/save）弹出期间主窗口会收到
 * onFocusChanged(false)，那是应用内模态交互而非「用户离开」，不能触发失焦锁；
 * 对话框关闭回焦时还要重置活动时刻，避免被 idle 补判误锁。
 * 用计数器支持嵌套调用，返回 release 函数，调用方 try/finally 释放。 */
let blurSuppressDepth = 0

export function suppressBlurLock(): () => void {
  blurSuppressDepth += 1
  let released = false
  return () => {
    if (released) return
    released = true
    blurSuppressDepth = Math.max(0, blurSuppressDepth - 1)
  }
}

export function isBlurLockSuppressed(): boolean {
  return blurSuppressDepth > 0
}

function stealFocus() {
  activeBeforeLock =
    document.activeElement instanceof HTMLElement ? document.activeElement : null
  try {
    ;(document.activeElement as HTMLElement | null)?.blur()
  } catch {
    // 部分元素 blur 抛异常可忽略
  }
  blurAllTerms()
}

function restoreFocus() {
  const el = activeBeforeLock
  activeBeforeLock = null
  if (el && el.isConnected) {
    try {
      el.focus()
      return
    } catch {
      // 落到无操作
    }
  }
}

/** 手动 / 闲置 / 失焦 / 启动锁定。幂等：已锁定时不重复触发。 */
export function lockApp(reason: LockReason = 'manual') {
  if (appLocked.value) return
  // 槽位守卫：运行中 lock.json 被删 / 损坏隔离后，密码槽位已消失，
  // 再锁就是无法解开的死态（设计 §7：删除 lock.json 关锁可接受）。
  // startup 路径在 initAppLock() 内已先确认槽位存在，不受此守卫影响。
  if (reason !== 'startup' && !lockSlots.value.appPasswordSet) return
  appLockReason.value = reason
  appLocked.value = true
  stealFocus()
  // AI 流由各 AiChatPanel 实例 watch(appLocked) 自行停止；
  // metrics/内存轮询由 Inspector/StatusBar 的门控 watcher 收敛。
}

/**
 * 启动序列调用（loadAppConfig 之后、splash 撤下之前）。
 *
 * - 启用且密码槽位存在 → 冷启动即锁定；
 * - 启用但槽位缺失（lock.json 被删/损坏）→ 不锁、toast 告警并把开关写回 false；
 * - lock_status IPC 失败 fail-closed：启用即锁，解锁请求会持续重试。
 */
export async function initAppLock(): Promise<void> {
  if (!isTauri) {
    appLocked.value = false
    return
  }
  try {
    const st = await ipcLockStatus()
    lockSlots.value = {
      appPasswordSet: st.appPasswordSet,
      sessionPasswordSet: st.sessionPasswordSet,
    }
    if (!savedSettings.appLockEnabled) {
      appLocked.value = false
      return
    }
    if (st.appPasswordSet) {
      lockApp('startup')
      return
    }
    // 开关开着但密码槽位没了：避免死态，回退开关
    savedSettings.appLockEnabled = false
    persistSettings()
    appLocked.value = false
    toast(
      st.fileCorrupted
        ? '锁数据已损坏，应用锁已关闭，请重新设置密码'
        : '应用锁密码缺失，应用锁已关闭，请重新设置密码',
      'warn',
    )
  } catch (e) {
    // fail-closed：状态查询失败时，只要配置启用就按锁定处理
    if (savedSettings.appLockEnabled) {
      lockApp('startup')
    } else {
      console.warn('lock_status 查询失败:', e)
    }
  }
}

/** 刷新槽位状态（设置密码 / 清除密码后调用） */
export async function refreshLockSlots(): Promise<void> {
  if (!isTauri) return
  try {
    const st = await ipcLockStatus()
    lockSlots.value = {
      appPasswordSet: st.appPasswordSet,
      sessionPasswordSet: st.sessionPasswordSet,
    }
  } catch (e) {
    console.warn('lock_status 刷新失败:', e)
  }
}

/** 解锁应用。成功 resolve；失败抛出 Error（message 为后端中文文案）供遮罩展示。 */
export async function unlockApp(password: string): Promise<void> {
  const outcome = await lockUnlockOrReset('app', password)
  if (outcome === 'slot_missing') {
    // 运行中 lock.json 被删 / 损坏：按设计 §7 视为已重置，解除锁定并关回开关
    await refreshLockSlots()
    if (savedSettings.appLockEnabled) {
      savedSettings.appLockEnabled = false
      persistSettings()
    }
    appLocked.value = false
    toast('未检测到应用锁密码文件，应用锁已关闭，请重新设置密码', 'warn', 3000)
    window.setTimeout(() => restoreFocus(), 500)
    return
  }
  appLocked.value = false
  // 焦点还原推迟到遮罩淡出结束后由 AppLockOverlay @after-leave 触发，
  // 避免与退出动画首帧竞争造成掉帧；500ms 兜底保证极端情况下也能还原。
  window.setTimeout(() => restoreFocus(), 500)
}

/** 遮罩退出动画结束时调用：把焦点还给锁屏前的元素 */
export function restoreFocusAfterUnlock() {
  restoreFocus()
}
