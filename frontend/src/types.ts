export type HostStatus = 'online' | 'idle' | 'offline' | 'warn'
export type HostColor = 'green' | 'blue' | 'cyan' | 'purple' | 'yellow' | 'red'
export type ViewStyle = 'list' | 'card' | 'grid' | 'compact'
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

export type TunnelType = 'local' | 'remote' | 'dynamic'

/** SSH 端口转发规则（主机配置 · 端口转发分组） */
export interface TunnelRule {
  id: string
  type: TunnelType
  name: string
  enabled: boolean
  /** 监听地址；local/dynamic 为本地侧，remote 为远端侧 */
  bindHost: string
  bindPort: number
  /** 目标地址；仅 local / remote（dynamic 为 SOCKS5 代理，无目标） */
  targetHost?: string
  targetPort?: number
}

/** 隧道运行状态（后端 0x0A 状态帧 TunnelStatus，camelCase，见 ssh/tunnel.rs） */
export interface TunnelStatus {
  id: string
  type: TunnelType
  state: 'starting' | 'active' | 'error' | 'stopped'
  bindHost: string
  bindPort: number
  /** 实际监听端口（next 顺延 / 远端分配时不同于 bindPort） */
  boundPort: number
  targetHost?: string
  targetPort?: number
  activeConnections: number
  /** 本地→远端方向累计字节 */
  bytesUp: number
  /** 远端→本地方向累计字节 */
  bytesDown: number
  error?: string
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
  /** SSH 端口转发规则列表（持久化在 StoredHost.extra.tunnels） */
  tunnels?: TunnelRule[]
  /** 登录密码（明文暂存，TODO: 迁移到系统钥匙串 tauri-plugin-keyring） */
  password?: string
  /** 公钥认证：私钥路径（存在时优先于 password，允许 ~ 前缀） */
  keyPath?: string
  /** 公钥认证：私钥口令（无口令为 undefined，会话内暂存） */
  keyPassphrase?: string
}

/**
 * 后端持久化的主机配置（与 src-tauri/src/store.rs 的 StoredHost 对应）。
 * 不含运行时状态字段（status/lat/cpu/mem/uptime），密码存系统钥匙串不入此结构。
 */
export interface StoredHost {
  id: string
  connType: string
  user: string
  ip: string
  port: number
  os: string
  color: string
  label: string
  tag: string
  group: string
  keyPath: string | null
  /** 该连接类型的扩展表单字段（代理、跳板机、端口转发等） */
  extra: Record<string, unknown>
  updatedAt: number
}
