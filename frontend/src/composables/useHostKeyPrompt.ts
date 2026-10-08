import { ref } from 'vue'

/**
 * 全局主机指纹确认弹窗（TOFU 两阶段确认，hostkey-verification-design.md §6.5）。
 *
 * 用法：
 *   const action = await confirmHostKey({ kind: 'unknown', host, port, algo, fingerprint })
 *   // 'save'   = 接受并保存（unknown，信任并写 known_hosts）
 *   // 'once'   = 仅本次连接（unknown，本次可信但不落盘）
 *   // 'update' = 更新指纹并重连（mismatch，仅鼠标点击可达）
 *   // 'reject' = 取消（unknown）/ 断开（mismatch）
 *
 * 模块级单例：HostKeyModal 组件绑定 visible/info，
 * 调用方通过 confirmHostKey() 拿 Promise，用户选择后 resolve。
 */

/** 弹窗形态：unknown = 首连指纹确认；mismatch = 密钥变更红色警告 */
export type HostKeyKind = 'unknown' | 'mismatch'

/** 用户动作：reject=取消/断开；once=仅本次连接；save=接受并保存；update=更新指纹并重连 */
export type HostKeyAction = 'reject' | 'once' | 'save' | 'update'

export interface HostKeyInfo {
  kind: HostKeyKind
  host: string
  port: number
  /** 主机密钥算法标准名（如 ssh-ed25519） */
  algo: string
  /** SHA256 指纹（SHA256:…），等宽字体展示、可复制 */
  fingerprint: string
  /** OpenSSH 格式完整公钥（`algorithm base64`，「查看完整公钥」展开区展示） */
  pubkey: string
}

const visible = ref(false)
const info = ref<HostKeyInfo | null>(null)

let resolver: ((value: HostKeyAction) => void) | null = null

/**
 * 解析主机密钥协议错误（错误前缀即协议，不得改为普通文案）：
 * `HOSTKEY_UNKNOWN: {algo}|{fingerprint}|{pubkey}` /
 * `HOSTKEY_MISMATCH: {algo}|{fingerprint}|{pubkey}`。
 * 非 HOSTKEY 错误返回 null。
 */
export function parseHostKeyError(
  msg: string,
): { kind: HostKeyKind; algo: string; fingerprint: string; pubkey: string } | null {
  const m = /^HOSTKEY_(UNKNOWN|MISMATCH):\s*([^|\s]+)\|([^|]+)\|(.+)/.exec(msg)
  if (!m) return null
  return {
    kind: m[1]!.toLowerCase() as HostKeyKind,
    algo: m[2]!,
    fingerprint: m[3]!,
    pubkey: m[4]!.trim(),
  }
}

/** 弹出指纹确认弹窗。已有弹窗未关闭时先取消上一个（resolve 'reject'） */
export function confirmHostKey(i: HostKeyInfo): Promise<HostKeyAction> {
  if (resolver) {
    resolver('reject')
    resolver = null
  }
  info.value = i
  visible.value = true
  return new Promise(resolve => {
    resolver = resolve
  })
}

/** 用户接受：persist=true 接受并保存；persist=false 仅本次连接（不落盘） */
export function acceptHostKey(persist: boolean) {
  visible.value = false
  const r = resolver
  resolver = null
  r?.(persist ? 'save' : 'once')
}

/** mismatch 专用「更新指纹并重连」（高危动作，仅鼠标点击可达，设计 §6.5） */
export function updateHostKey() {
  visible.value = false
  const r = resolver
  resolver = null
  r?.('update')
}

/** 用户拒绝/取消：ESC、遮罩点击、关闭按钮、mismatch 默认的「断开」都走这里 */
export function rejectHostKey() {
  visible.value = false
  const r = resolver
  resolver = null
  r?.('reject')
}

export const hostKeyPromptState = { visible, info }
