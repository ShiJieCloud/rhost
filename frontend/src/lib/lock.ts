/**
 * 应用锁 / 会话锁后端 IPC 封装（docs/explanation/design/app-lock-design.md §6.2）。
 *
 * 前端任何时候都拿不到 salt/hash，验证只能经 lock_unlock 走后端限速通道。
 */
import { invoke } from '@tauri-apps/api/core'

export type LockKind = 'app' | 'session'

export interface LockStatus {
  appPasswordSet: boolean
  sessionPasswordSet: boolean
  /** 读取时是否发生了 lock.json 损坏隔离（前端用于告警文案） */
  fileCorrupted: boolean
}

/** 查询两级锁密码槽位是否已设置（只返回布尔） */
export function lockStatus(): Promise<LockStatus> {
  return invoke<LockStatus>('lock_status')
}

/** 设置 / 修改密码；首次设置 oldPassword 传 null，修改时必须传正确旧密码 */
export function lockSetPassword(
  kind: LockKind,
  password: string,
  oldPassword: string | null,
): Promise<void> {
  return invoke('lock_set_password', { kind, password, oldPassword })
}

/** 解锁验证。错误文案由后端给（含剩余次数 / 冷却秒数），前端原样展示。 */
export function lockUnlock(kind: LockKind, password: string): Promise<void> {
  return invoke('lock_unlock', { kind, password })
}

/**
 * 解锁验证 + 「凭据槽位已消失」复核（设计 §7：删除 lock.json 关锁可接受）。
 *
 * - 验证成功 → 'ok'
 * - 验证失败但对应槽位已不存在（运行中 lock.json 被删 / 损坏隔离）→
 *   'slot_missing'：调用方按「凭据已被本机重置」解除锁定，避免锁屏死态；
 *   槽位仍存在（普通密码错误 / 冷却）→ 透传后端错误
 */
export async function lockUnlockOrReset(
  kind: LockKind,
  password: string,
): Promise<'ok' | 'slot_missing'> {
  try {
    await lockUnlock(kind, password)
    return 'ok'
  } catch (e) {
    const st = await lockStatus().catch(() => null)
    if (st) {
      const missing = kind === 'app' ? !st.appPasswordSet : !st.sessionPasswordSet
      if (missing) return 'slot_missing'
    }
    throw e
  }
}

/** 清除密码槽位（关闭锁），清除前必须验证当前密码 */
export function lockClearPassword(kind: LockKind, password: string): Promise<void> {
  return invoke('lock_clear_password', { kind, password })
}
