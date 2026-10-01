import { ref, watch } from 'vue'
import type { KeyType, SshKey } from '../types'

/**
 * SSH 密钥管理状态。
 * 模块级单例；后续接 Pinia 或 Tauri 后端（ssh-keygen / 读取 ~/.ssh）时仅需替换此文件。
 */

const STORE_KEY = 'rhost.sshKeys'

function daysAgo(n: number): string {
  return new Date(Date.now() - n * 86400000).toISOString()
}

/** 首次启动的演示数据；用户产生数据后以 localStorage 为准 */
function seedKeys(): SshKey[] {
  return [
    {
      id: 'k_seed_prod',
      name: '生产环境部署',
      type: 'ED25519',
      bits: '256',
      comment: 'deploy@prod-web-01',
      fingerprint: 'SHA256:9xK2mVq7LpR4tN8wYzA3bC6dE1fG5hJ0kM2nP4qS7vU',
      publicKey:
        'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILkQv3mR7pX2sT8nW1yZ4bC6dE9fG2hJ5kM8nP0qS3vU',
      privatePath: '~/.ssh/id_ed25519_prod',
      passphrase: true,
      created: daysAgo(148),
      lastUsed: daysAgo(0.02),
      hosts: ['prod-web-01', 'prod-web-02', 'prod-db-01'],
      tags: ['生产', '部署'],
    },
    {
      id: 'k_seed_github',
      name: 'GitHub 个人账号',
      type: 'ED25519',
      bits: '256',
      comment: 'wang@MacBook-Pro',
      fingerprint: 'SHA256:Qm3Tt8VxPz1Lc5Nb7Rk2Wd9Yh4Jf6Sg0Ae3Uo5In8Mx',
      publicKey:
        'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGh8Lp2Qw9Rz4Tn6Vb1Yc3Xd5Ze7Af9Gj2Kl4Mo6Pq8',
      privatePath: '~/.ssh/id_ed25519',
      passphrase: false,
      created: daysAgo(420),
      lastUsed: daysAgo(3),
      hosts: ['github.com'],
      tags: ['Git'],
    },
    {
      id: 'k_seed_legacy',
      name: '旧版跳板机',
      type: 'RSA',
      bits: '4096',
      comment: 'legacy-bastion',
      fingerprint: 'SHA256:Zx7Cv2Bn9Mk4Lp6Qw1Er3Ty5Ui7Oa9Sd0Fg2Hj4Kl6',
      publicKey:
        'ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAACAQDf3kL9mN2pQ7rT1vW4xY6zA8bC0dE5fG7hJ9kL1mN3oP5qR7sT9uV1wX3yZ5aB7cD9eF1gH3iJ5kL7mN9oP',
      privatePath: '~/.ssh/id_rsa_legacy',
      passphrase: true,
      created: daysAgo(690),
      lastUsed: daysAgo(46),
      hosts: ['bastion.legacy.internal'],
      tags: ['内网', '跳板机'],
    },
    {
      id: 'k_seed_cloud',
      name: '云主机集群',
      type: 'ECDSA',
      bits: '521',
      comment: 'ops@cloud-cluster',
      fingerprint: 'SHA256:Wq4Rt7Yu2Io5Pa8Sd1Fg3Hj6Kl9Zx0Cv2Bn4Mq7Lp1',
      publicKey:
        'ecdsa-sha2-nistp521 AAAAE2VjZHNhLXNoYTItbmlzdHA1MjEAAAAIbmlzdHA1MjEAAACFBAHk2Lp4Qw7Rz9Tn1Vb3Yc5Xd8Ze0Af2Gj4Kl6Mo8Pq',
      privatePath: '~/.ssh/id_ecdsa_cloud',
      passphrase: false,
      created: daysAgo(75),
      lastUsed: daysAgo(1),
      hosts: ['cloud-node-01', 'cloud-node-02', 'cloud-node-03', 'cloud-node-04', 'cloud-node-05'],
      tags: ['云', '运维'],
    },
  ]
}

function loadKeys(): SshKey[] {
  try {
    const raw = localStorage.getItem(STORE_KEY)
    if (raw) {
      const parsed = JSON.parse(raw)
      if (Array.isArray(parsed)) return parsed as SshKey[]
    }
  } catch { /* 隐私模式或数据损坏时回退种子数据 */ }
  return seedKeys()
}

export const keys = ref<SshKey[]>(loadKeys())

watch(keys, v => {
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify(v))
  } catch { /* 持久化失败不影响操作 */ }
}, { deep: true })

/* ==================== CRUD ==================== */

let seq = 0
function uid(): string {
  return 'k_' + Date.now().toString(36) + '_' + (++seq).toString(36)
}

export function addKey(k: Omit<SshKey, 'id'>): SshKey {
  const item: SshKey = { ...k, id: uid() }
  keys.value.unshift(item)
  return item
}

export function removeKey(id: string) {
  const idx = keys.value.findIndex(k => k.id === id)
  if (idx !== -1) keys.value.splice(idx, 1)
}

export function findKey(id: string | null): SshKey | null {
  if (!id) return null
  return keys.value.find(k => k.id === id) ?? null
}

/* ==================== 弹窗状态 ==================== */

export type KeyModalKind = '' | 'generate' | 'import' | 'detail'

export const keyModal = ref<KeyModalKind>('')
export const detailKeyId = ref<string | null>(null)
/** 待删除确认的密钥；非 null 时删除确认弹窗覆盖在最上层 */
export const deletingKeyId = ref<string | null>(null)

export function openGenerate() {
  keyModal.value = 'generate'
}

export function openImport() {
  keyModal.value = 'import'
}

export function openDetail(id: string) {
  detailKeyId.value = id
  keyModal.value = 'detail'
}

export function askDelete(id: string) {
  deletingKeyId.value = id
}

export function closeKeyModal() {
  keyModal.value = ''
  detailKeyId.value = null
}

export function cancelDelete() {
  deletingKeyId.value = null
}

/** 确认删除：关闭删除弹窗；若详情弹窗正在展示同一密钥则一并关闭 */
export function confirmDelete(): SshKey | null {
  const target = findKey(deletingKeyId.value)
  if (!target) {
    deletingKeyId.value = null
    return null
  }
  removeKey(target.id)
  if (detailKeyId.value === target.id) {
    keyModal.value = ''
    detailKeyId.value = null
  }
  deletingKeyId.value = null
  return target
}

/* ==================== 密钥生成辅助（前端模拟） ==================== */

export const KEY_BITS_MAP: Record<KeyType, string[]> = {
  ED25519: [],
  RSA: ['2048', '3072', '4096'],
  ECDSA: ['256', '384', '521'],
}

export const KEY_DEFAULT_BITS: Record<KeyType, string> = {
  ED25519: '256',
  RSA: '4096',
  ECDSA: '521',
}

export function keySlug(name: string): string {
  return (name || 'key').replace(/[^\w一-龥-]+/g, '_').slice(0, 24) || 'key'
}

function randToken(len: number): string {
  const chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'
  let s = ''
  for (let i = 0; i < len; i++) s += chars[Math.floor(Math.random() * chars.length)]!
  return s
}

export function randomFingerprint(): string {
  return 'SHA256:' + randToken(43)
}

export function randomPublicKey(type: KeyType): string {
  const prefix =
    type === 'ED25519' ? 'ssh-ed25519'
      : type === 'RSA' ? 'ssh-rsa'
        : 'ecdsa-sha2-nistp521'
  return `${prefix} ${randToken(type === 'RSA' ? 172 : 68)}`
}

/** 从粘贴文本粗略识别密钥类型 */
export function detectKeyType(text: string): KeyType {
  if (/ed25519/i.test(text)) return 'ED25519'
  if (/ecdsa/i.test(text)) return 'ECDSA'
  return 'RSA'
}

/* ==================== 展示辅助 ==================== */

export function formatKeyDate(iso: string | null): string {
  if (!iso) return '—'
  const d = new Date(iso)
  const p = (v: number) => String(v).padStart(2, '0')
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`
}

export function relKeyTime(iso: string | null): string {
  if (!iso) return '从未使用'
  const diff = Date.now() - new Date(iso).getTime()
  const m = Math.floor(diff / 60000)
  if (m < 1) return '刚刚'
  if (m < 60) return `${m} 分钟前`
  const h = Math.floor(m / 60)
  if (h < 24) return `${h} 小时前`
  const d = Math.floor(h / 24)
  if (d < 30) return `${d} 天前`
  return formatKeyDate(iso)
}

export function shortFingerprint(fp: string): string {
  return fp.length > 28 ? fp.slice(0, 28) + '…' : fp
}

/** 逗号/中文逗号/空白分隔的字符串解析为列表 */
export function parseListInput(str: string): string[] {
  return str
    .split(/[,，\s]+/)
    .map(s => s.trim())
    .filter(Boolean)
}

/** 复制文本到剪贴板，兼容非安全上下文 */
export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text)
      return true
    }
    const ta = document.createElement('textarea')
    ta.value = text
    ta.style.position = 'fixed'
    ta.style.opacity = '0'
    document.body.appendChild(ta)
    ta.select()
    document.execCommand('copy')
    ta.remove()
    return true
  } catch {
    return false
  }
}
