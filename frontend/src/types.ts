export type HostStatus = 'online' | 'idle' | 'offline' | 'warn'
export type HostColor = 'green' | 'blue' | 'cyan' | 'purple' | 'yellow' | 'red'
export type ViewStyle = 'list' | 'card' | 'grid' | 'compact'
export type AuthType = 'key' | 'password' | 'agent'
export type ToastType = 'ok' | 'info' | 'warn' | 'err'

export type KeyType = 'ED25519' | 'RSA' | 'ECDSA'

export interface SshKey {
  id: string
  name: string
  type: KeyType
  bits: string
  comment: string
  fingerprint: string
  publicKey: string
  /** 私钥本地路径；仅导入公钥时为 null */
  privatePath: string | null
  passphrase: boolean
  /** ISO 时间字符串 */
  created: string
  /** ISO 时间字符串；从未使用为 null */
  lastUsed: string | null
  hosts: string[]
  tags: string[]
}

export interface Host {
  id: string
  user: string
  ip: string
  port: number
  os: string
  color: HostColor
  label: string
  tag: string
  status: HostStatus
  lat: number | null
  cpu: string
  mem: string
  uptime: string
  group: string
}
