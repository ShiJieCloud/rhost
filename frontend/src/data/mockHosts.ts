import type { Host, HostColor } from '../types'

/** TODO: 接入后端后由 invoke('list_hosts') 替换 */
export const MOCK_HOSTS: Host[] = [
  {
    id: 'web-prod-01', user: 'root', ip: '192.168.1.101', port: 22,
    os: 'Ubuntu 22.04.3 LTS', color: 'green', label: 'WP',
    tag: '生产', status: 'online', lat: 12, cpu: '22%', mem: '31%',
    uptime: '42 天', group: '生产环境',
  },
  {
    id: 'db-master', user: 'root', ip: '192.168.1.102', port: 22,
    os: 'CentOS 7.9', color: 'blue', label: 'DB',
    tag: '数据库', status: 'online', lat: 18, cpu: '46%', mem: '68%',
    uptime: '128 天', group: '生产环境',
  },
  {
    id: 'cache-01', user: 'ubuntu', ip: '192.168.1.103', port: 22,
    os: 'Ubuntu 20.04.6 LTS', color: 'cyan', label: 'CA',
    tag: '缓存', status: 'online', lat: 9, cpu: '8%', mem: '24%',
    uptime: '12 天', group: '生产环境',
  },
  {
    id: 'staging-web', user: 'ubuntu', ip: '10.0.3.30', port: 22,
    os: 'Ubuntu 22.04.3 LTS', color: 'purple', label: 'ST',
    tag: '测试', status: 'online', lat: 23, cpu: '15%', mem: '42%',
    uptime: '6 天', group: '测试环境',
  },
  {
    id: 'test-env-02', user: 'deploy', ip: '10.0.3.21', port: 2222,
    os: 'Debian 12', color: 'yellow', label: 'TE',
    tag: '测试', status: 'offline', lat: null, cpu: '—', mem: '—',
    uptime: '—', group: '测试环境',
  },
  {
    id: 'backup-nas', user: 'admin', ip: '192.168.1.200', port: 22,
    os: 'Synology DSM 7.2', color: 'red', label: 'NA',
    tag: '存储', status: 'warn', lat: 64, cpu: '72%', mem: '81%',
    uptime: '256 天', group: '其他',
  },
]

export const COLOR_MAP: Record<HostColor, string> = {
  green: '#3ddc84',
  blue: '#7aa2f7',
  cyan: '#4fd6e0',
  purple: '#bb9af7',
  yellow: '#e5b567',
  red: '#f7768e',
}

export const STYLE_LABELS = {
  list: '列表视图',
  card: '卡片视图',
  grid: '网格视图',
  compact: '紧凑视图',
} as const

export const GROUP_OPTIONS = ['生产环境', '测试环境', '开发环境', '其他', '+ 新建分组…']
