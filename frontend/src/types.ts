export type HostStatus = 'online' | 'idle' | 'offline' | 'warn'
export type HostColor = 'green' | 'blue' | 'cyan' | 'purple' | 'yellow' | 'red'
export type ViewStyle = 'list' | 'card' | 'grid' | 'compact'
export type AuthType = 'key' | 'password' | 'agent'
export type ToastType = 'ok' | 'info' | 'warn' | 'err'

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
