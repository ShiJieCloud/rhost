<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { toast } from '../composables/useToast'
import { confirmDialog, confirmState } from '../composables/useConfirm'
import { copyText } from '../stores/keys'
import { addHost, editingHost, showNewConn, updateHost } from '../stores/hosts'
import { rehostSession } from '../stores/session'
import { GROUP_OPTIONS } from '../data/mockHosts'
import { isTauri } from '../lib/tauri'
import type { HostColor, TunnelRule, TunnelType } from '../types'

/* =========================================================
   类型与表单定义（数据驱动）
   ========================================================= */
type ConnType = 'ssh' | 'sftp' | 'local' | 'serial' | 'docker' | 'telnet'
type Values = Record<string, string>

interface FieldOption { v: string; t: string }
interface FieldDef {
  label: string
  name: string
  span: 1 | 2
  required?: boolean
  type?: 'text' | 'number' | 'password' | 'select' | 'segmented' | 'tags' | 'textarea' | 'tunnels'
  value?: string
  placeholder?: string
  hint?: string
  options?: FieldOption[]
  showWhen?: (v: Values) => boolean
}
interface CardDef {
  id: string
  title: string
  icon: string
  desc?: string
  summary: (v: Values) => string
  fields: FieldDef[]
}

const ICONS: Record<string, string> = {
  info: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M8 10h8M8 14h5"/></svg>',
  lock: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="4" y="11" width="16" height="10" rx="2"/><path d="M8 11V7a4 4 0 018 0v4"/></svg>',
  globe: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a15 15 0 010 18 15 15 0 010-18z"/></svg>',
  tunnel: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 12h6l2-6 2 12 2-6h4"/></svg>',
  term: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M7 10l3 2-3 2M13 14h4"/></svg>',
  chip: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="4" y="4" width="16" height="16" rx="2"/><path d="M9 9h6v6H9z"/></svg>',
  shell: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 17l6-6-6-6M12 19h8"/></svg>',
  env: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 2l3 6 6 1-4 4 1 6-6-3-6 3 1-6-4-4 6-1z"/></svg>',
  ssh: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="4" width="20" height="16" rx="2"/><path d="M7 9l3 3-3 3M13 15h4"/></svg>',
  local: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/></svg>',
  serial: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 2v6M15 2v6M6 8h12v4a6 6 0 01-6 6 6 6 0 01-6-6V8zM12 18v4"/></svg>',
  docker: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 12h18v3a5 5 0 01-5 5H8a5 5 0 01-5-5v-3z"/><path d="M7 12V9h3v3M12 12V9h3v3M12 8V5h3v3"/></svg>',
  telnet: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a15 15 0 010 18 15 15 0 010-18z"/></svg>',
  folder: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>',
  sftp: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7a2 2 0 0 1 2-2h3.5l2 2H19a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><path d="M12 9.5v5M10 12.5l2 2 2-2"/></svg>',
}

const TYPES: { v: ConnType; t: string; icon: string; sub: string }[] = [
  { v: 'ssh', t: 'SSH', icon: 'ssh', sub: 'SSH · 安全远程主机连接' },
  { v: 'sftp', t: 'SFTP', icon: 'sftp', sub: 'SFTP · 安全文件传输' },
  { v: 'local', t: '本地', icon: 'local', sub: '本地 Shell 会话' },
  { v: 'serial', t: '串口', icon: 'serial', sub: '串口设备连接' },
  { v: 'docker', t: 'Docker', icon: 'docker', sub: 'Docker 容器终端' },
  { v: 'telnet', t: 'Telnet', icon: 'telnet', sub: 'Telnet 网络设备连接' },
]

const GROUP_OPTS: FieldOption[] = GROUP_OPTIONS.map(g => ({ v: g, t: g }))

/* =========================================================
   端口转发（隧道规则）
   ========================================================= */
const TNL_META: Record<TunnelType, { label: string; flag: string }> = {
  local: { label: '本地转发', flag: '-L' },
  remote: { label: '远程转发', flag: '-R' },
  dynamic: { label: '动态转发', flag: '-D' },
}
const TNL_UI: Record<TunnelType, { bind: string; target: string; desc: string }> = {
  local: {
    bind: '绑定地址（本地）',
    target: '目标地址（远端）',
    desc: '在本地监听端口，把收到的连接通过 SSH 通道转发到服务器侧可达的目标地址。',
  },
  remote: {
    bind: '绑定地址（远端）',
    target: '目标地址（本地）',
    desc: '在服务器侧监听端口，把收到的连接通过 SSH 通道转发回本地可达的目标地址。',
  },
  dynamic: {
    bind: '监听地址（本地）',
    target: '',
    desc: '在本地开启 SOCKS5 代理端口，应用经此代理访问远端网络。',
  },
}
/** 当前主机的隧道规则（保存时写入 Host.tunnels） */
const tunnelRules = ref<TunnelRule[]>([])

const FORMS: Record<ConnType, { cards: CardDef[] }> = {
  ssh: {
    cards: [
      {
        id: 'basic', title: '基本信息', icon: 'info',
        desc: '设置连接的显示名称、分组、目标主机和颜色标签。',
        summary: v => [v.name, v.host && (v.host + (v.port && v.port !== '22' ? ':' + v.port : ''))]
          .filter(Boolean).join(' · '),
        fields: [
          { label: '名称', name: 'name', span: 1, placeholder: '留空自动生成' },
          { label: '分组', name: 'group', span: 1, type: 'select', value: GROUP_OPTIONS[0], options: GROUP_OPTS },
          { label: '主机地址', name: 'host', span: 2, required: true, placeholder: '192.168.1.10 或 example.com' },
          { label: '端口', name: 'port', span: 1, required: true, value: '22', type: 'number' },
          { label: '用户名', name: 'user', span: 1, required: true, placeholder: 'root' },
          { label: '标签', name: 'tags', span: 2, type: 'tags',
            placeholder: '输入标签后回车，如：生产、数据库' },
        ],
      },
      {
        id: 'auth', title: '认证方式', icon: 'lock',
        desc: '选择登录凭据类型。密码与私钥口令将加密保存到系统钥匙串。',
        summary: v => v.auth === 'password' ? '密码'
          : v.auth === 'key' ? (v.keyfile ? '公钥 · ' + v.keyfile : '公钥')
          : v.auth === 'agent' ? 'SSH Agent' : '',
        fields: [
          { label: '认证方式', name: 'auth', span: 2, type: 'segmented', value: 'key',
            options: [
              { v: 'key', t: '公钥' },
              { v: 'password', t: '密码' },
              { v: 'agent', t: 'SSH Agent' },
            ] },
          { label: '私钥文件', name: 'keyfile', span: 2, value: '~/.ssh/id_ed25519',
            hint: '支持 RSA / ECDSA / Ed25519 格式的私钥',
            showWhen: v => v.auth === 'key' },
          { label: '密码', name: 'password', span: 2, type: 'password',
            placeholder: '输入登录密码', showWhen: v => v.auth === 'password' },
        ],
      },
      {
        id: 'network', title: '网络与代理', icon: 'globe',
        desc: '配置代理、跳板机与连接保活。跳板机支持多级串联。',
        summary: v => {
          const p: string[] = []
          if (v.proxyType && v.proxyType !== 'none') p.push('代理')
          if (v.jumpHost) p.push('跳板机')
          if (v.keepalive && v.keepalive !== '0') p.push('保活 ' + v.keepalive + 's')
          return p.join(' · ')
        },
        fields: [
          { label: '代理类型', name: 'proxyType', span: 1, type: 'select', value: 'none',
            options: [
              { v: 'none', t: '不使用代理' },
              { v: 'http', t: 'HTTP 代理' },
              { v: 'socks5', t: 'SOCKS5' },
            ] },
          { label: '代理地址', name: 'proxyHost', span: 1, placeholder: '127.0.0.1:7890',
            showWhen: v => !!v.proxyType && v.proxyType !== 'none' },
          { label: '跳板机', name: 'jumpHost', span: 2, type: 'textarea',
            placeholder: 'user@jump.example.com:22\n支持多级，每行一台，从上到下依次连接',
            hint: '多条跳板机按顺序串联' },
          { label: '保活间隔（秒）', name: 'keepalive', span: 1, value: '60', type: 'number', hint: '0 = 禁用' },
          { label: '连接超时（秒）', name: 'timeout', span: 1, value: '15', type: 'number' },
          { label: '压缩', name: 'compression', span: 1, type: 'select', value: 'off',
            options: [{ v: 'off', t: '关闭' }, { v: 'on', t: '开启' }] },
          { label: '主机密钥验证', name: 'hostKeyCheck', span: 1, type: 'select', value: 'ask',
            options: [
              { v: 'ask', t: '每次询问' },
              { v: 'accept', t: '自动接受' },
              { v: 'strict', t: '严格拒绝未知' },
            ] },
        ],
      },
      {
        id: 'forward', title: '端口转发', icon: 'tunnel',
        desc: '通过 SSH 隧道映射端口，多条规则按列表顺序在连接建立后依次生效。',
        summary: () => {
          const p: string[] = []
          const count = (t: TunnelType) => tunnelRules.value.filter(x => x.type === t).length
          if (count('local')) p.push(`本地 ×${count('local')}`)
          if (count('remote')) p.push(`远程 ×${count('remote')}`)
          if (count('dynamic')) p.push(`动态 ×${count('dynamic')}`)
          return p.join(' · ')
        },
        fields: [
          { label: '转发规则', name: 'tunnels', span: 2, type: 'tunnels' },
        ],
      },
      {
        id: 'terminal', title: '终端与高级', icon: 'term',
        desc: '终端仿真类型、字符编码、X11 转发与登录后自动执行命令。',
        summary: v => {
          const p: string[] = []
          if (v.term) p.push(v.term)
          if (v.encoding) p.push(v.encoding.toUpperCase())
          if (v.env) p.push('环境变量')
          if (v.command) p.push('启动命令')
          return p.join(' · ')
        },
        fields: [
          { label: '终端类型 (TERM)', name: 'term', span: 1, type: 'select', value: 'xterm-256color',
            options: [
              { v: 'xterm-256color', t: 'xterm-256color' },
              { v: 'xterm', t: 'xterm' },
              { v: 'screen-256color', t: 'screen-256color' },
              { v: 'vt100', t: 'vt100' },
            ] },
          { label: '字符编码', name: 'encoding', span: 1, type: 'select', value: 'utf-8',
            options: [
              { v: 'utf-8', t: 'UTF-8' },
            ] },
          { label: 'X11 转发', name: 'x11', span: 1, type: 'select', value: 'off',
            options: [{ v: 'off', t: '关闭' }, { v: 'on', t: '开启 (-X)' }] },
          { label: '登录后执行', name: 'command', span: 1, placeholder: '如：cd /var/log' },
          { label: '环境变量', name: 'env', span: 2, type: 'textarea',
            placeholder: 'LANG=en_US.UTF-8\nEDITOR=vim\n每行一条 KEY=VALUE' },
        ],
      },
    ],
  },

  sftp: {
    cards: [
      {
        id: 'basic', title: '基本信息', icon: 'info',
        desc: '设置 SFTP 连接的显示名称、目标主机与颜色标签。',
        summary: v => [v.name, v.host && (v.host + (v.port && v.port !== '22' ? ':' + v.port : ''))]
          .filter(Boolean).join(' · '),
        fields: [
          { label: '名称', name: 'name', span: 1, placeholder: '留空自动生成' },
          { label: '分组', name: 'group', span: 1, type: 'select', value: GROUP_OPTIONS[0], options: GROUP_OPTS },
          { label: '主机地址', name: 'host', span: 2, required: true, placeholder: '192.168.1.10 或 example.com' },
          { label: '端口', name: 'port', span: 1, required: true, value: '22', type: 'number' },
          { label: '用户名', name: 'user', span: 1, required: true, placeholder: 'root' },
          { label: '标签', name: 'tags', span: 2, type: 'tags',
            placeholder: '输入标签后回车，如：生产、文件服务器' },
        ],
      },
      {
        id: 'auth', title: '认证方式', icon: 'lock',
        desc: '选择登录凭据类型。密码与私钥口令将加密保存到系统钥匙串。',
        summary: v => v.auth === 'password' ? '密码'
          : v.auth === 'key' ? (v.keyfile ? '公钥 · ' + v.keyfile : '公钥')
          : v.auth === 'agent' ? 'SSH Agent' : '',
        fields: [
          { label: '认证方式', name: 'auth', span: 2, type: 'segmented', value: 'key',
            options: [
              { v: 'key', t: '公钥' },
              { v: 'password', t: '密码' },
              { v: 'agent', t: 'SSH Agent' },
            ] },
          { label: '私钥文件', name: 'keyfile', span: 2, value: '~/.ssh/id_ed25519',
            hint: '支持 RSA / ECDSA / Ed25519 格式的私钥',
            showWhen: v => v.auth === 'key' },
          { label: '密码', name: 'password', span: 2, type: 'password',
            placeholder: '输入登录密码', showWhen: v => v.auth === 'password' },
        ],
      },
      {
        id: 'transfer', title: '目录与传输', icon: 'folder',
        desc: '连接后默认打开的本地 / 远程目录，以及文件传输策略。',
        summary: v => {
          const p: string[] = []
          if (v.remotePath) p.push(v.remotePath)
          if (v.transferMode === 'binary') p.push('二进制')
          else if (v.transferMode === 'ascii') p.push('文本')
          if (v.resume === 'on') p.push('断点续传')
          return p.join(' · ')
        },
        fields: [
          { label: '默认远程目录', name: 'remotePath', span: 1, placeholder: '/var/www 或 /home/user' },
          { label: '默认本地目录', name: 'localPath', span: 1, placeholder: '~/Downloads' },
          { label: '传输模式', name: 'transferMode', span: 1, type: 'select', value: 'auto',
            options: [
              { v: 'auto', t: '自动识别' },
              { v: 'binary', t: '二进制' },
              { v: 'ascii', t: '文本' },
            ] },
          { label: '传输并发数', name: 'concurrency', span: 1, type: 'select', value: '4',
            options: ['1', '2', '4', '8'].map(n => ({ v: n, t: n + ' 线程' })) },
          { label: '保留权限', name: 'keepPerms', span: 1, type: 'select', value: 'on',
            options: [{ v: 'on', t: '开启' }, { v: 'off', t: '关闭' }] },
          { label: '保留时间戳', name: 'keepTimes', span: 1, type: 'select', value: 'on',
            options: [{ v: 'on', t: '开启' }, { v: 'off', t: '关闭' }] },
          { label: '断点续传', name: 'resume', span: 1, type: 'select', value: 'on',
            options: [{ v: 'on', t: '开启' }, { v: 'off', t: '关闭' }] },
          { label: '同名文件', name: 'onExists', span: 1, type: 'select', value: 'ask',
            options: [
              { v: 'ask', t: '每次询问' },
              { v: 'overwrite', t: '直接覆盖' },
              { v: 'rename', t: '自动重命名' },
              { v: 'skip', t: '跳过' },
            ] },
        ],
      },
      {
        id: 'network', title: '网络与代理', icon: 'globe',
        desc: '配置代理、跳板机与连接保活。跳板机支持多级串联。',
        summary: v => {
          const p: string[] = []
          if (v.proxyType && v.proxyType !== 'none') p.push('代理')
          if (v.jumpHost) p.push('跳板机')
          if (v.keepalive && v.keepalive !== '0') p.push('保活 ' + v.keepalive + 's')
          return p.join(' · ')
        },
        fields: [
          { label: '代理类型', name: 'proxyType', span: 1, type: 'select', value: 'none',
            options: [
              { v: 'none', t: '不使用代理' },
              { v: 'http', t: 'HTTP 代理' },
              { v: 'socks5', t: 'SOCKS5' },
            ] },
          { label: '代理地址', name: 'proxyHost', span: 1, placeholder: '127.0.0.1:7890',
            showWhen: v => !!v.proxyType && v.proxyType !== 'none' },
          { label: '跳板机', name: 'jumpHost', span: 2, type: 'textarea',
            placeholder: 'user@jump.example.com:22\n支持多级，每行一台，从上到下依次连接',
            hint: '多条跳板机按顺序串联' },
          { label: '保活间隔（秒）', name: 'keepalive', span: 1, value: '60', type: 'number', hint: '0 = 禁用' },
          { label: '连接超时（秒）', name: 'timeout', span: 1, value: '15', type: 'number' },
          { label: '压缩传输', name: 'compression', span: 1, type: 'select', value: 'off',
            options: [{ v: 'off', t: '关闭' }, { v: 'on', t: '开启' }] },
          { label: '主机密钥验证', name: 'hostKeyCheck', span: 1, type: 'select', value: 'ask',
            options: [
              { v: 'ask', t: '每次询问' },
              { v: 'accept', t: '自动接受' },
              { v: 'strict', t: '严格拒绝未知' },
            ] },
        ],
      },
    ],
  },

  local: {
    cards: [
      {
        id: 'basic', title: '基本信息', icon: 'info',
        desc: '本地 Shell 会话的基本设置。',
        summary: v => [v.name, v.shell].filter(Boolean).join(' · '),
        fields: [
          { label: '名称', name: 'name', span: 1, placeholder: '可选' },
          { label: '分组', name: 'group', span: 1, type: 'select', value: GROUP_OPTIONS[2], options: GROUP_OPTS },
          { label: 'Shell', name: 'shell', span: 1, required: true, type: 'select', value: '/bin/zsh',
            options: [
              { v: '/bin/zsh', t: 'Zsh' },
              { v: '/bin/bash', t: 'Bash' },
              { v: '/bin/fish', t: 'Fish' },
              { v: 'powershell.exe', t: 'PowerShell' },
              { v: 'cmd.exe', t: 'CMD' },
              { v: 'wsl.exe', t: 'WSL' },
            ] },
          { label: '启动参数', name: 'args', span: 1, placeholder: '-l' },
          { label: '工作目录', name: 'cwd', span: 1, placeholder: '留空使用默认目录' },
          { label: '标签', name: 'tags', span: 1, type: 'tags',
            placeholder: '输入后回车' },
        ],
      },
      {
        id: 'env', title: '环境与启动', icon: 'env',
        desc: '进入 Shell 时注入的环境变量和自动执行的命令。',
        summary: v => [v.env && '环境变量', v.command && '启动命令'].filter(Boolean).join(' · '),
        fields: [
          { label: '环境变量', name: 'env', span: 2, type: 'textarea', placeholder: 'KEY=VALUE\n每行一条' },
          { label: '启动命令', name: 'command', span: 2, placeholder: '进入后自动执行，如：git status' },
        ],
      },
    ],
  },

  serial: {
    cards: [
      {
        id: 'basic', title: '基本信息', icon: 'info',
        desc: '串口设备的识别与显示设置。',
        summary: v => [v.name, v.device].filter(Boolean).join(' · '),
        fields: [
          { label: '名称', name: 'name', span: 1, placeholder: '可选' },
          { label: '分组', name: 'group', span: 1, type: 'select', value: GROUP_OPTIONS[0], options: GROUP_OPTS },
          { label: '串口设备', name: 'device', span: 2, required: true,
            placeholder: '/dev/ttyUSB0 或 COM3',
            hint: 'Linux/macOS：/dev/tty*；Windows：COMx' },
          { label: '标签', name: 'tags', span: 2, type: 'tags',
            placeholder: '输入标签后回车，如：交换机、现场设备' },
        ],
      },
      {
        id: 'params', title: '串口参数', icon: 'chip',
        desc: '波特率、数据位、停止位、校验位与流控设置。',
        summary: v => [v.baud && v.baud + ' baud',
          v.databits && `${v.databits}${v.parity === 'even' ? 'E' : v.parity === 'odd' ? 'O' : 'N'}${v.stopbits}`]
          .filter(Boolean).join(' · '),
        fields: [
          { label: '波特率', name: 'baud', span: 1, required: true, type: 'select', value: '115200',
            options: ['9600', '19200', '38400', '57600', '115200', '230400', '460800', '921600']
              .map(v => ({ v, t: v })) },
          { label: '数据位', name: 'databits', span: 1, type: 'select', value: '8',
            options: ['5', '6', '7', '8'].map(v => ({ v, t: v })) },
          { label: '停止位', name: 'stopbits', span: 1, type: 'select', value: '1',
            options: [{ v: '1', t: '1' }, { v: '1.5', t: '1.5' }, { v: '2', t: '2' }] },
          { label: '校验位', name: 'parity', span: 1, type: 'select', value: 'none',
            options: [{ v: 'none', t: '无' }, { v: 'even', t: '偶' }, { v: 'odd', t: '奇' }] },
          { label: '流控', name: 'flow', span: 1, type: 'select', value: 'none',
            options: [
              { v: 'none', t: '无' },
              { v: 'rtscts', t: 'RTS/CTS' },
              { v: 'xonxoff', t: 'XON/XOFF' },
            ] },
          { label: '本地回显', name: 'echo', span: 1, type: 'select', value: 'off',
            options: [{ v: 'on', t: '开启' }, { v: 'off', t: '关闭' }] },
        ],
      },
      {
        id: 'terminal', title: '终端设置', icon: 'term',
        desc: '接收数据的编码与行结束符。',
        summary: v => [v.encoding && v.encoding.toUpperCase(), v.eol && 'EOL: ' + v.eol.toUpperCase()]
          .filter(Boolean).join(' · '),
        fields: [
          { label: '接收编码', name: 'encoding', span: 1, type: 'select', value: 'utf-8',
            options: [
              { v: 'utf-8', t: 'UTF-8' },
              { v: 'gbk', t: 'GBK' },
              { v: 'hex', t: '十六进制' },
            ] },
          { label: '行结束符', name: 'eol', span: 1, type: 'select', value: 'lf',
            options: [
              { v: 'lf', t: 'LF (\\n)' },
              { v: 'crlf', t: 'CRLF (\\r\\n)' },
              { v: 'cr', t: 'CR (\\r)' },
            ] },
        ],
      },
    ],
  },

  docker: {
    cards: [
      {
        id: 'basic', title: '基本信息', icon: 'info',
        desc: '选择要进入的容器并设置显示名称。',
        summary: v => [v.name, v.container].filter(Boolean).join(' · '),
        fields: [
          { label: '名称', name: 'name', span: 1, placeholder: '可选' },
          { label: '分组', name: 'group', span: 1, type: 'select', value: GROUP_OPTIONS[0], options: GROUP_OPTS },
          { label: '容器', name: 'container', span: 2, required: true,
            placeholder: '容器名或 ID，如 nginx-prod',
            hint: '支持模糊匹配' },
          { label: '标签', name: 'tags', span: 2, type: 'tags',
            placeholder: '输入标签后回车，如：容器、nginx' },
        ],
      },
      {
        id: 'conn', title: '连接设置', icon: 'shell',
        desc: 'Docker Host、容器内 Shell 及启动参数。',
        summary: v => [v.shell, v.host && '远程 Docker'].filter(Boolean).join(' · '),
        fields: [
          { label: 'Docker Host', name: 'host', span: 2, placeholder: 'unix:///var/run/docker.sock' },
          { label: 'Shell', name: 'shell', span: 1, type: 'select', value: '/bin/sh',
            options: [
              { v: '/bin/sh', t: '/bin/sh' },
              { v: '/bin/bash', t: '/bin/bash' },
              { v: '/bin/ash', t: '/bin/ash' },
            ] },
          { label: '工作目录', name: 'cwd', span: 1, placeholder: '容器内工作目录' },
          { label: '启动命令', name: 'command', span: 2, placeholder: '留空进入交互式 Shell' },
        ],
      },
    ],
  },

  telnet: {
    cards: [
      {
        id: 'basic', title: '基本信息', icon: 'info',
        desc: 'Telnet 主机地址与端口。',
        summary: v => [v.name, v.host && (v.host + (v.port ? ':' + v.port : ''))]
          .filter(Boolean).join(' · '),
        fields: [
          { label: '名称', name: 'name', span: 1, placeholder: '可选' },
          { label: '分组', name: 'group', span: 1, type: 'select', value: GROUP_OPTIONS[0], options: GROUP_OPTS },
          { label: '主机地址', name: 'host', span: 1, required: true, placeholder: '192.168.1.1' },
          { label: '端口', name: 'port', span: 1, required: true, value: '23', type: 'number' },
          { label: '标签', name: 'tags', span: 2, type: 'tags',
            placeholder: '输入标签后回车，如：路由器、网络设备' },
        ],
      },
      {
        id: 'terminal', title: '终端设置', icon: 'term',
        desc: 'Telnet 终端仿真类型与行结束符。',
        summary: v => [v.term, v.eol && 'EOL: ' + v.eol.toUpperCase()].filter(Boolean).join(' · '),
        fields: [
          { label: '终端类型', name: 'term', span: 1, type: 'select', value: 'vt100',
            options: [
              { v: 'vt100', t: 'vt100' },
              { v: 'xterm', t: 'xterm' },
              { v: 'ansi', t: 'ansi' },
            ] },
          { label: '行结束符', name: 'eol', span: 1, type: 'select', value: 'crlf',
            options: [
              { v: 'crlf', t: 'CRLF (\\r\\n)' },
              { v: 'lf', t: 'LF (\\n)' },
              { v: 'cr', t: 'CR (\\r)' },
            ] },
          { label: '登录后执行', name: 'command', span: 2, placeholder: '如：enable' },
        ],
      },
    ],
  },
}

/* =========================================================
   状态
   ========================================================= */
const currentType = ref<ConnType>('ssh')
const currentCardId = ref('basic')
/** 编辑态：正在编辑的主机原 id；null 表示新建 */
const editingId = ref<string | null>(null)
const isEdit = computed(() => !!editingId.value)
const formData = reactive({} as Record<ConnType, Values>)
const fieldErrors = reactive<Record<string, boolean>>({})
const pwShown = reactive<Record<string, boolean>>({})
const testing = ref(false)
const saving = ref(false)
const testResult = ref<{ kind: 'ok' | 'err' | 'loading'; text: string } | null>(null)
const modalEl = ref<HTMLElement | null>(null)

const cards = computed(() => FORMS[currentType.value].cards)
const card = computed(() => cards.value.find(c => c.id === currentCardId.value) ?? cards.value[0])
const values = computed(() => formData[currentType.value])
const typeSub = computed(() => TYPES.find(t => t.v === currentType.value)?.sub ?? '')

function initType(t: ConnType) {
  const data: Values = {}
  FORMS[t].cards.forEach(c => c.fields.forEach(f => { data[f.name] = f.value ?? '' }))
  formData[t] = data
}
TYPES.forEach(t => initType(t.v))

/* =========================================================
   打开 / 关闭 / 快捷键
   ========================================================= */
watch(showNewConn, async v => {
  if (!v) return
  const h = editingHost.value
  TYPES.forEach(t => initType(t.v))
  Object.keys(fieldErrors).forEach(k => delete fieldErrors[k])
  Object.keys(pwShown).forEach(k => delete pwShown[k])
  tagDraft.value = ''
  testResult.value = null
  testing.value = false
  saving.value = false
  if (h) {
    // 编辑态：当前持久化的主机只有 SSH / SFTP 两种
    const type: ConnType = h.os === 'SFTP 主机' ? 'sftp' : 'ssh'
    currentType.value = type
    currentCardId.value = FORMS[type].cards[0].id
    const d = formData[type]
    d.name = h.id
    d.user = h.user
    d.host = h.ip
    d.port = String(h.port)
    d.group = GROUP_OPTIONS.includes(h.group) ? h.group : GROUP_OPTIONS[0]
    // 新建时的兜底标记不回填为用户标签
    d.tags = h.tag === '新建' || h.tag === 'SFTP' ? '' : h.tag
    // 回填认证方式与密码：优先内存缓存，否则按需从钥匙串读取
    if (h.password) {
      d.auth = 'password'
      d.password = h.password
    } else if (isTauri) {
      try {
        const pwd = await invoke<string | null>('get_connection_password', { id: h.id })
        if (pwd) {
          d.auth = 'password'
          d.password = pwd
          h.password = pwd // 缓存到内存，避免重复读取
        }
      } catch (e) {
        console.error('回填密码失败:', e)
      }
    }
    editingId.value = h.id
    // 回填端口转发规则（拷贝一份，取消编辑时不污染原数据）
    tunnelRules.value = (h.tunnels ?? []).map(t => ({ ...t }))
  } else {
    currentType.value = 'ssh'
    currentCardId.value = FORMS.ssh.cards[0].id
    editingId.value = null
    tunnelRules.value = []
  }
  nextTick(() => setTimeout(() => {
    const el = modalEl.value?.querySelector('.content-pane input:not([type=hidden])') as HTMLElement | null
    el?.focus()
  }, 180))
})

function close() {
  showNewConn.value = false
  editingHost.value = null
  editingId.value = null
}

function onKey(e: KeyboardEvent) {
  if (!showNewConn.value) return
  // 全局确认弹窗（如删除规则确认）打开时，按键交给 ConfirmModal 处理
  if (confirmState.visible.value) return
  if (tnlEditorOpen.value) {
    // 隧道编辑子弹窗打开时：Esc 只关子弹窗，主弹窗快捷键全部忽略
    if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      closeTnlEditor()
    }
    return
  }
  if (e.key === 'Escape') {
    e.preventDefault()
    close()
  } else if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
    e.preventDefault()
    save()
  } else if (e.key.toLowerCase() === 't' && (e.metaKey || e.ctrlKey)) {
    e.preventDefault()
    testConnection()
  }
}

onMounted(() => document.addEventListener('keydown', onKey))
onUnmounted(() => document.removeEventListener('keydown', onKey))

/* =========================================================
   切换与输入
   ========================================================= */
function selectType(t: ConnType) {
  if (isEdit.value || t === currentType.value) return
  currentType.value = t
  currentCardId.value = FORMS[t].cards[0].id
  testResult.value = null
}

function selectCard(id: string) {
  if (id !== currentCardId.value) currentCardId.value = id
}

function setField(name: string, val: string) {
  values.value[name] = val
  delete fieldErrors[`${currentType.value}.${name}`]
  testResult.value = null
}

/* ---- 标签（chips）输入：值以逗号分隔存入 Values ---- */
const tagDraft = ref('')

function tagsOf(name: string): string[] {
  return (values.value[name] || '')
    .split(',').map(s => s.trim()).filter(Boolean)
}
function writeTags(name: string, list: string[]) {
  values.value[name] = list.join(',')
  testResult.value = null
}
function commitTag(name: string) {
  const parts = tagDraft.value.split(/[,，]/).map(s => s.trim()).filter(Boolean)
  if (parts.length) {
    const list = tagsOf(name)
    parts.forEach(t => { if (!list.includes(t)) list.push(t) })
    writeTags(name, list)
  }
  tagDraft.value = ''
}
function removeTag(name: string, t: string) {
  writeTags(name, tagsOf(name).filter(x => x !== t))
}
function onTagKeydown(name: string, e: KeyboardEvent) {
  if (e.key === 'Enter' || e.key === ',' || e.key === '，') {
    if (e.metaKey || e.ctrlKey) return // 让 ⌘↵ 走保存
    e.preventDefault()
    commitTag(name)
  } else if (e.key === 'Backspace' && !tagDraft.value) {
    const list = tagsOf(name)
    if (list.length) writeTags(name, list.slice(0, -1))
  }
}
function focusTagControl(e: Event) {
  (e.currentTarget as HTMLElement).querySelector('input')?.focus()
}
watch([currentType, currentCardId], () => { tagDraft.value = '' })

/* =========================================================
   端口转发：列表操作与规则编辑子弹窗
   ========================================================= */
const tnlEditorOpen = ref(false)
const tnlEditingId = ref<string | null>(null)
const tnlModalEl = ref<HTMLElement | null>(null)
const tnlErrors = reactive<Record<string, boolean>>({})
const tnlForm = reactive({
  name: '',
  type: 'local' as TunnelType,
  bindHost: '127.0.0.1',
  bindPort: '80',
  targetHost: '',
  targetPort: '80',
})

function tnlUid(): string {
  return 't' + Date.now().toString(36) + Math.random().toString(36).slice(2, 6)
}

function defaultTnlName(type: TunnelType, port: number): string {
  return `${TNL_META[type].label} ${port}`
}

function openTnlEditor(rule?: TunnelRule) {
  tnlEditingId.value = rule ? rule.id : null
  tnlForm.name = rule?.name ?? ''
  tnlForm.type = rule?.type ?? 'local'
  tnlForm.bindHost = rule?.bindHost ?? '127.0.0.1'
  tnlForm.bindPort = rule ? String(rule.bindPort) : '80'
  tnlForm.targetHost = rule?.targetHost ?? ''
  tnlForm.targetPort = rule ? String(rule.targetPort ?? '80') : '80'
  Object.keys(tnlErrors).forEach(k => delete tnlErrors[k])
  tnlEditorOpen.value = true
  nextTick(() => setTimeout(() => {
    ;(tnlModalEl.value?.querySelector('input') as HTMLElement | null)?.focus()
  }, 60))
}

function closeTnlEditor() {
  tnlEditorOpen.value = false
  tnlEditingId.value = null
}

function validTnlPort(raw: string): number | null {
  const n = Number(raw)
  return raw && Number.isInteger(n) && n >= 1 && n <= 65535 ? n : null
}

/** 端口步进（自定义上下按钮），clamp 到 1–65535 */
function stepTnlPort(key: 'bindPort' | 'targetPort', delta: number) {
  const cur = parseInt(tnlForm[key], 10) || 0
  tnlForm[key] = String(Math.min(65535, Math.max(1, cur + delta)))
  tnlErrors[key] = false
}

function saveTnlRule() {
  Object.keys(tnlErrors).forEach(k => delete tnlErrors[k])
  // 两个端口规则一致：留空默认 80，填了则校验 1–65535
  const rawBindPort = tnlForm.bindPort.trim()
  const bindPort = rawBindPort ? validTnlPort(rawBindPort) : 80
  if (!bindPort) tnlErrors.bindPort = true
  let targetPort: number | null = null
  if (tnlForm.type !== 'dynamic') {
    if (!tnlForm.targetHost.trim()) tnlErrors.targetHost = true
    const rawTargetPort = tnlForm.targetPort.trim()
    targetPort = rawTargetPort ? validTnlPort(rawTargetPort) : 80
    if (!targetPort) tnlErrors.targetPort = true
  }
  if (Object.keys(tnlErrors).length) return

  const payload: TunnelRule = {
    id: tnlEditingId.value ?? tnlUid(),
    type: tnlForm.type,
    name: tnlForm.name.trim() || defaultTnlName(tnlForm.type, bindPort!),
    enabled: true,
    bindHost: tnlForm.bindHost.trim() || '127.0.0.1',
    bindPort: bindPort!,
  }
  if (tnlForm.type !== 'dynamic') {
    payload.targetHost = tnlForm.targetHost.trim()
    payload.targetPort = targetPort!
  }

  if (tnlEditingId.value) {
    const idx = tunnelRules.value.findIndex(t => t.id === tnlEditingId.value)
    if (idx !== -1) {
      payload.enabled = tunnelRules.value[idx]!.enabled // 编辑保留原启用状态
      tunnelRules.value[idx] = payload
    }
  } else {
    tunnelRules.value.push(payload)
  }
  closeTnlEditor()
}

async function removeTnlRule(rule: TunnelRule) {
  const ok = await confirmDialog(`确定删除转发规则「${rule.name}」？`, '删除规则', {
    danger: true, confirmText: '删除',
  })
  if (!ok) return
  tunnelRules.value = tunnelRules.value.filter(t => t.id !== rule.id)
  toast(`已删除转发规则「${rule.name}」`, 'info', 2000)
}

function toggleTnlRule(rule: TunnelRule, ev: Event) {
  rule.enabled = (ev.target as HTMLInputElement).checked
}

/** 卡片 mousedown 时动态决定是否可拖拽（交互控件上不启动拖拽） */
function onTnlCardMousedown(e: MouseEvent) {
  const card = e.currentTarget as HTMLElement
  card.draggable = !(e.target as HTMLElement).closest('button, input, label')
}

function onTnlDragKey(id: string, e: KeyboardEvent) {
  if (e.key === 'ArrowUp') {
    e.preventDefault()
    moveTnlRule(id, -1)
  } else if (e.key === 'ArrowDown') {
    e.preventDefault()
    moveTnlRule(id, 1)
  }
}

/** 「N / M 已启用」计数 */
const tnlEnabledCount = computed(() => tunnelRules.value.filter(t => t.enabled).length)

/* ---- 排序：拖拽 + 手柄聚焦后 ↑ / ↓ ---- */
const tnlDraggingId = ref<string | null>(null)

function moveTnlRule(id: string, dir: -1 | 1) {
  const list = tunnelRules.value
  const idx = list.findIndex(t => t.id === id)
  const next = idx + dir
  if (idx === -1 || next < 0 || next >= list.length) return
  ;[list[idx], list[next]] = [list[next]!, list[idx]!]
}

function onTnlDragStart(rule: TunnelRule) {
  tnlDraggingId.value = rule.id
}
function onTnlDragOver(e: DragEvent, rule: TunnelRule) {
  e.preventDefault()
  if (e.dataTransfer) e.dataTransfer.dropEffect = 'move'
  const from = tunnelRules.value.findIndex(t => t.id === tnlDraggingId.value)
  const to = tunnelRules.value.findIndex(t => t.id === rule.id)
  if (from === -1 || to === -1 || from === to) return
  const [moved] = tunnelRules.value.splice(from, 1)
  tunnelRules.value.splice(to, 0, moved!)
}
function onTnlDragEnd() {
  tnlDraggingId.value = null
}

/* ---- 等效命令预览 ---- */
interface CmdToken { cls: string; text: string }

/** 子弹窗内等效命令（随表单输入实时更新；目标端口留空时以弱化样式展示默认值 80） */
const tnlCmd = computed<CmdToken[]>(() => {
  const bindHost = tnlForm.bindHost.trim()
  const bindPort = tnlForm.bindPort.trim()
  const targetHost = tnlForm.targetHost.trim()
  const targetPort = tnlForm.targetPort.trim()
  const seg = (host: string, port: string, hostFb: string, portFb: string): CmdToken[] => [
    { cls: 'c-arg', text: host || hostFb },
    { cls: 'c-sep', text: ':' },
    { cls: port ? 'c-arg' : 'c-ph', text: port || portFb },
  ]
  const tokens: CmdToken[] = [
    { cls: 'c-prompt', text: '$' },
    { cls: 'c-cmd', text: 'ssh' },
    { cls: 'c-flag', text: '-N' },
  ]
  if (tnlForm.type === 'dynamic') {
    tokens.push({ cls: 'c-flag', text: '-D' }, ...seg(bindHost, bindPort, '127.0.0.1', '80'))
  } else {
    tokens.push(
      { cls: 'c-flag', text: tnlForm.type === 'local' ? '-L' : '-R' },
      ...seg(bindHost, bindPort, '127.0.0.1', '80'),
      { cls: 'c-sep', text: ':' },
      ...seg(targetHost, targetPort, 'localhost', '80'),
    )
  }
  tokens.push({ cls: 'c-host', text: 'user@host' })
  return tokens
})

const tnlCmdText = computed(() => {
  const bindHost = tnlForm.bindHost.trim() || '127.0.0.1'
  const bindPort = tnlForm.bindPort.trim() || '80'
  let cmd = 'ssh -N'
  if (tnlForm.type === 'dynamic') {
    cmd += ` -D ${bindHost}:${bindPort}`
  } else {
    cmd += ` ${tnlForm.type === 'local' ? '-L' : '-R'} ` +
      `${bindHost}:${bindPort}:${tnlForm.targetHost.trim() || 'localhost'}:${tnlForm.targetPort.trim() || '80'}`
  }
  return cmd + ' user@host'
})

async function copyTnlCmd() {
  if (await copyText(tnlCmdText.value)) toast('命令已复制', 'ok', 1600)
}

/** 列表底部命令预览：多行续行符格式，仅包含启用的规则 */
const tnlPreview = computed<CmdToken[][]>(() => {
  const active = tunnelRules.value.filter(t => t.enabled)
  const user = (values.value.user || '').trim()
  const host = (values.value.host || '').trim()
  const target = user && host ? `${user}@${host}` : 'user@example.com'
  if (!active.length) {
    return [[{ cls: 'c-cmd', text: 'ssh' }, { cls: 'c-host', text: target }]]
  }
  const lines: CmdToken[][] = [
    [
      { cls: 'c-cmd', text: 'ssh' },
      { cls: 'c-flag', text: '-N' },
      { cls: 'c-sep', text: '\\' },
    ],
  ]
  active.forEach(t => {
    const arg = t.type === 'dynamic'
      ? `${t.bindHost}:${t.bindPort}`
      : `${t.bindHost}:${t.bindPort}:${t.targetHost}:${t.targetPort}`
    lines.push([
      { cls: 'c-flag', text: TNL_META[t.type].flag },
      { cls: 'c-arg', text: arg },
      { cls: 'c-sep', text: '\\' },
    ])
  })
  lines.push([{ cls: 'c-host', text: target }])
  return lines
})

function isVisible(f: FieldDef) {
  return !f.showWhen || f.showWhen(values.value)
}

function hasError(name: string) {
  return !!fieldErrors[`${currentType.value}.${name}`]
}

function errorText(f: FieldDef) {
  return f.type === 'number' ? '需为 1–65535 的整数' : '此项为必填'
}

/* =========================================================
   侧栏摘要
   ========================================================= */
function summaryOf(c: CardDef) {
  try {
    return c.summary(values.value) || ''
  } catch {
    return ''
  }
}

/* =========================================================
   校验
   ========================================================= */
function validate(): boolean {
  // 提交标签输入框中尚未确认的草稿，避免漏存
  if (tagDraft.value) {
    const tagField = card.value.fields.find(f => f.type === 'tags')
    if (tagField) commitTag(tagField.name)
  }
  Object.keys(fieldErrors).forEach(k => delete fieldErrors[k])

  let firstBadCard = ''
  let firstBadField = ''

  for (const c of cards.value) {
    for (const f of c.fields) {
      if (!f.required || !isVisible(f)) continue
      const raw = (values.value[f.name] ?? '').trim()
      let bad = !raw
      if (!bad && f.type === 'number') {
        const n = Number(raw)
        if (!Number.isInteger(n) || n < 1 || n > 65535) bad = true
      }
      if (bad) {
        fieldErrors[`${currentType.value}.${f.name}`] = true
        if (!firstBadCard) {
          firstBadCard = c.id
          firstBadField = f.name
        }
      }
    }
  }

  if (firstBadCard) {
    currentCardId.value = firstBadCard
    nextTick(() => {
      const el = modalEl.value?.querySelector(`[data-field="${firstBadField}"]`)
      el?.scrollIntoView({ behavior: 'smooth', block: 'center' })
      ;(el?.querySelector('input,select,textarea') as HTMLElement | null)?.focus()
    })
    return false
  }
  return true
}

/* =========================================================
   测试连接
   ========================================================= */
async function testConnection() {
  if (testing.value) return
  if (!validate()) {
    toast('请先完善必填项', 'warn')
    return
  }

  const v = values.value
  // 仅 SSH/SFTP 的密码认证走真实后端测试；其余类型/认证方式暂用 mock
  const canRealTest =
    isTauri &&
    (currentType.value === 'ssh' || currentType.value === 'sftp') &&
    v.auth === 'password'

  testing.value = true
  testResult.value = { kind: 'loading', text: '正在连接…' }

  if (canRealTest) {
    const port = parseInt(v.port, 10) || 22
    const target = `${v.user}@${v.host}:${port}`
    try {
      const res = await invoke<{ latencyMs: number }>('test_ssh_connection', {
        payload: {
          host: v.host,
          port,
          username: v.user,
          password: v.password,
          cols: 0,
          rows: 0,
        },
      })
      testResult.value = { kind: 'ok', text: `连接成功 · ${target} · ${res.latencyMs}ms` }
      toast(`测试成功 · ${target} · ${res.latencyMs}ms`, 'ok', 2200)
    } catch (e) {
      testResult.value = { kind: 'err', text: `连接失败：${String(e)}` }
      toast(`连接失败：${String(e)}`, 'err', 2800)
    } finally {
      testing.value = false
    }
    return
  }

  // ---- mock 兜底：非 Tauri 环境 / 非密码认证 / 其他连接类型 ----
  await new Promise(r => setTimeout(r, 900 + Math.random() * 700))
  const target = (v.host || v.device || v.container || v.shell || '').trim()
  const fail = /(bad|fail)/i.test(target) || /^0\./.test(target) || /^255\./.test(target) || /^999/.test(target)
  const lat = 8 + Math.floor(Math.random() * 40)
  testing.value = false
  if (fail) {
    testResult.value = { kind: 'err', text: `连接失败：无法访问 ${target || '目标'}（超时）` }
    toast(`无法连接到 ${target || '目标'}`, 'err', 2800)
  } else {
    testResult.value = { kind: 'ok', text: `连接成功 · ${target || '本地'}${v.port ? ':' + v.port : ''} · ${lat}ms` }
    toast(`测试成功 · ${target || '本地'} · ${lat}ms`, 'ok', 2200)
  }
}

/* =========================================================
   保存
   ========================================================= */
function autoName(v: Values): string {
  switch (currentType.value) {
    case 'ssh':
    case 'sftp':
    case 'telnet':
      return `${v.user ? v.user + '@' : ''}${v.host}`
    case 'serial':
      return v.device
    case 'docker':
      return v.container
    default:
      return v.shell || '本地终端'
  }
}

async function save() {
  if (saving.value) return
  if (tnlEditorOpen.value) return // 隧道编辑子弹窗打开时不触发主保存
  if (!validate()) {
    toast('请先完善必填项', 'warn')
    return
  }

  const v = { ...values.value }
  if (v.group === '+ 新建分组…') {
    toast('分组创建功能开发中', 'warn')
    return
  }
  if (!v.name?.trim()) v.name = autoName(v)

  saving.value = true

  const label =
    v.name.replace(/[^a-zA-Z0-9一-龥]/g, '').slice(0, 2).toUpperCase() ||
    (currentType.value === 'sftp' ? 'SF' : 'SS')
  const tagList = (v.tags || '').split(',').map(t => t.trim()).filter(Boolean)
  const tagVal = tagList.length ? tagList.join(',') : (currentType.value === 'sftp' ? 'SFTP' : '新建')
  const connType = currentType.value
  const hasPassword = v.auth === 'password' && !!v.password
  const tunnelsVal = tunnelRules.value.length ? tunnelRules.value.map(t => ({ ...t })) : undefined

  /** 保存密码到系统钥匙串（Tauri 环境）；浏览器 dev 模式跳过。
   *  改名场景下旧 id 的钥匙串条目已由 updateHost 内部清理。 */
  async function persistPassword(hostId: string) {
    if (!isTauri || !hasPassword) return
    await invoke('save_connection_password', { id: hostId, password: v.password }).catch(
      e => console.error('save_connection_password 失败:', e),
    )
  }

  // 编辑态：局部更新并同步已打开的会话，不新建、不连接
  if (editingId.value) {
    const oldId = editingId.value
    const updated = await updateHost(oldId, {
      id: v.name,
      user: v.user,
      ip: v.host,
      port: parseInt(v.port, 10) || 22,
      label,
      tag: tagVal,
      group: v.group || GROUP_OPTIONS[0],
      keyPath: v.auth === 'key' ? v.keyfile : undefined,
      tunnels: connType === 'ssh' ? tunnelsVal : undefined,
      // 密码仅暂存内存（hostToStored 不会写入 JSON），供本次会话连接直接使用；
      // 同时异步写入系统钥匙串，供下次启动读取
      password: v.auth === 'password' ? v.password : undefined,
    }, connType)
    if (updated) {
      rehostSession(oldId, updated)
      await persistPassword(updated.id)
    }
    saving.value = false
    close()
    toast(`已保存连接「${v.name}」的修改`, 'ok', 2400)
    return
  }

  if (currentType.value === 'ssh' || currentType.value === 'sftp') {
    const isSftp = currentType.value === 'sftp'
    const newHost = {
      id: v.name,
      user: v.user,
      ip: v.host,
      port: parseInt(v.port, 10) || 22,
      os: isSftp ? 'SFTP 主机' : 'Linux (未知发行版)',
      color: (isSftp ? 'cyan' : 'green') as HostColor,
      label,
      tag: tagVal,
      status: 'idle' as const,
      lat: null,
      cpu: '—',
      mem: '—',
      uptime: '—',
      group: v.group || GROUP_OPTIONS[0],
      keyPath: v.auth === 'key' ? v.keyfile : undefined,
      tunnels: isSftp ? undefined : tunnelsVal,
      // 密码仅暂存内存（不入 JSON），首次连接直接使用免弹框；钥匙串持久化由 persistPassword 完成
      password: v.auth === 'password' ? v.password : undefined,
    }
    await addHost(newHost, connType)
    await persistPassword(newHost.id)
  }

  saving.value = false
  close()
  toast(`已创建连接「${v.name}」`, 'ok', 2400)
}
</script>

<template>
  <div class="mask" :class="{ show: showNewConn }" @click.self="close">
    <div ref="modalEl" class="modal" role="dialog" aria-modal="true" aria-labelledby="newConnTitle">
      <!-- 头部 -->
      <div class="modal-head">
        <div class="modal-icon">
          <svg v-if="isEdit" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
               stroke-linecap="round" stroke-linejoin="round">
            <path d="M17 3a2.83 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"></path>
          </svg>
          <svg v-else viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
               stroke-linecap="round" stroke-linejoin="round">
            <line x1="12" y1="5" x2="12" y2="19"></line>
            <line x1="5" y1="12" x2="19" y2="12"></line>
          </svg>
        </div>
        <div class="modal-title">
          <h2 id="newConnTitle">{{ isEdit ? '编辑连接' : '新建连接' }}</h2>
          <p>{{ typeSub }}</p>
        </div>
        <button class="modal-close" title="关闭" @click="close">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
               stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line></svg>
        </button>
      </div>

      <!-- 类型选择条（编辑态锁定为原类型） -->
      <div class="type-bar" :class="{ locked: isEdit }">
        <div
          v-for="t in TYPES"
          :key="t.v"
          class="type-tab"
          :class="{ active: currentType === t.v }"
          @click="selectType(t.v)"
        >
          <span class="ti" v-html="ICONS[t.icon]"></span>
          <span>{{ t.t }}</span>
        </div>
      </div>

      <!-- 主内容：侧栏导航 + 表单 -->
      <div class="modal-content">
        <nav class="side-nav">
          <div
            v-for="c in cards"
            :key="c.id"
            class="snav-item"
            :class="{ active: currentCardId === c.id }"
            @click="selectCard(c.id)"
          >
            <div class="snav-icon" v-html="ICONS[c.icon] || ICONS.info"></div>
            <div class="snav-body">
              <div class="snav-title">{{ c.title }}</div>
              <div class="snav-summary">{{ summaryOf(c) }}</div>
            </div>
          </div>
        </nav>

        <section class="content-pane">
          <div class="pane-inner" :key="`${currentType}:${currentCardId}`">
            <div class="st-panel-head">
              <div class="st-panel-title">{{ card.title }}</div>
              <div v-if="card.desc" class="st-panel-sub">{{ card.desc }}</div>
            </div>

            <div class="form-grid">
              <div
                v-for="f in card.fields"
                v-show="isVisible(f)"
                :key="f.name"
                class="field"
                :class="{ half: f.span === 1, 'has-error': hasError(f.name) }"
                :data-field="f.name"
              >
                <label :for="`f-${f.name}`">
                  {{ f.label }}
                  <span v-if="f.required" class="req">*</span>
                </label>

                <select
                  v-if="f.type === 'select'"
                  :id="`f-${f.name}`"
                  :value="values[f.name]"
                  :class="{ invalid: hasError(f.name) }"
                  @change="setField(f.name, ($event.target as HTMLSelectElement).value)"
                >
                  <option v-for="op in f.options" :key="op.v" :value="op.v">{{ op.t }}</option>
                </select>

                <div v-else-if="f.type === 'segmented'" class="segmented">
                  <button
                    v-for="op in f.options"
                    :key="op.v"
                    type="button"
                    :class="{ active: values[f.name] === op.v }"
                    @click="setField(f.name, op.v)"
                  >
                    {{ op.t }}
                  </button>
                </div>

                <div v-else-if="f.type === 'password'" class="pw-wrap">
                  <input
                    :id="`f-${f.name}`"
                    :type="pwShown[f.name] ? 'text' : 'password'"
                    :value="values[f.name]"
                    :placeholder="f.placeholder"
                    :class="{ invalid: hasError(f.name) }"
                    autocomplete="new-password"
                    autocapitalize="off"
                    autocorrect="off"
                    spellcheck="false"
                    @input="setField(f.name, ($event.target as HTMLInputElement).value)"
                  >
                  <button
                    type="button"
                    class="pw-toggle"
                    :class="{ on: pwShown[f.name] }"
                    title="显示 / 隐藏"
                    @click="pwShown[f.name] = !pwShown[f.name]"
                  >
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                         stroke-linecap="round" stroke-linejoin="round">
                      <path d="M1 12s4-7 11-7 11 7 11 7-4 7-11 7S1 12 1 12z"/>
                      <circle cx="12" cy="12" r="3"/>
                    </svg>
                  </button>
                </div>

                <div v-else-if="f.type === 'tags'" class="tags-input" @click="focusTagControl">
                  <span v-for="t in tagsOf(f.name)" :key="t" class="tag-chip">
                    {{ t }}
                    <button
                      type="button"
                      class="tag-chip-x"
                      title="移除标签"
                      @click.stop="removeTag(f.name, t)"
                    >
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6"
                           stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
                    </button>
                  </span>
                  <input
                    type="text"
                    :value="tagDraft"
                    :placeholder="tagsOf(f.name).length ? '' : f.placeholder"
                    autocomplete="off"
                    autocapitalize="off"
                    autocorrect="off"
                    spellcheck="false"
                    @input="tagDraft = ($event.target as HTMLInputElement).value"
                    @keydown="onTagKeydown(f.name, $event)"
                    @blur="commitTag(f.name)"
                  >
                </div>

                <textarea
                  v-else-if="f.type === 'textarea'"
                  :id="`f-${f.name}`"
                  :value="values[f.name]"
                  :placeholder="f.placeholder"
                  :class="{ invalid: hasError(f.name) }"
                  autocapitalize="off"
                  autocorrect="off"
                  spellcheck="false"
                  @input="setField(f.name, ($event.target as HTMLTextAreaElement).value)"
                ></textarea>

                <!-- 端口转发：规则列表 -->
                <div v-else-if="f.type === 'tunnels'" class="tnl-field">
                  <div class="tnl-head">
                    <span class="tnl-count">{{ tnlEnabledCount }} / {{ tunnelRules.length }} 已启用</span>
                    <button type="button" class="btn tnl-add" @click="openTnlEditor()">+ 新建</button>
                  </div>

                  <div v-if="!tunnelRules.length" class="tnl-empty">
                    <strong>暂无转发规则</strong>
                    点击右上角「新建」添加第一条端口映射规则
                  </div>
                  <div v-else class="tnl-list">
                    <div
                      v-for="rule in tunnelRules"
                      :key="rule.id"
                      class="tnl-card"
                      :class="[`t-${rule.type}`, { disabled: !rule.enabled, dragging: tnlDraggingId === rule.id }]"
                      draggable="false"
                      @mousedown="onTnlCardMousedown($event)"
                      @dragstart="onTnlDragStart(rule)"
                      @dragover="onTnlDragOver($event, rule)"
                      @dragend="onTnlDragEnd"
                    >
                      <div
                        class="tnl-drag" tabindex="0"
                        title="拖拽调整顺序（聚焦后按 ↑ / ↓ 亦可）"
                        @keydown="onTnlDragKey(rule.id, $event)"
                      >
                        <svg width="9" height="14" viewBox="0 0 10 16" fill="currentColor" aria-hidden="true">
                          <circle cx="2" cy="3" r="1.3"/><circle cx="8" cy="3" r="1.3"/>
                          <circle cx="2" cy="8" r="1.3"/><circle cx="8" cy="8" r="1.3"/>
                          <circle cx="2" cy="13" r="1.3"/><circle cx="8" cy="13" r="1.3"/>
                        </svg>
                      </div>

                      <div class="tnl-body">
                        <div class="tnl-top">
                          <span class="tnl-badge">{{ TNL_META[rule.type].label }} {{ TNL_META[rule.type].flag }}</span>
                          <span class="tnl-name">{{ rule.name }}</span>
                        </div>
                        <div class="tnl-route">
                          <span class="tnl-ep">
                            <i class="tnl-ep-role">{{ rule.type === 'remote' ? '远端' : '本地' }}</i>
                            <code>{{ rule.bindHost }}:{{ rule.bindPort }}</code>
                          </span>
                          <template v-if="rule.type !== 'dynamic'">
                            <svg class="tnl-arrow" width="11" height="11" viewBox="0 0 24 24" fill="none"
                                 stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                              <path d="M5 12h14"/><path d="m12 5 7 7-7 7"/>
                            </svg>
                            <span class="tnl-ep">
                              <i class="tnl-ep-role">{{ rule.type === 'remote' ? '本地' : '远端' }}</i>
                              <code>{{ rule.targetHost }}:{{ rule.targetPort }}</code>
                            </span>
                          </template>
                          <span v-else class="tnl-ep"><i class="tnl-ep-role">SOCKS5</i></span>
                        </div>
                      </div>

                      <div class="tnl-actions">
                        <label class="st-switch" :title="rule.enabled ? '点击禁用' : '点击启用'">
                          <input type="checkbox" :checked="rule.enabled" @change="toggleTnlRule(rule, $event)" />
                        </label>
                        <button type="button" class="tnl-icon-btn" title="编辑" @click="openTnlEditor(rule)">
                          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                               stroke-linecap="round" stroke-linejoin="round">
                            <path d="M12 20h9"/><path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z"/>
                          </svg>
                        </button>
                        <button type="button" class="tnl-icon-btn danger" title="删除" @click="removeTnlRule(rule)">
                          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                               stroke-linecap="round" stroke-linejoin="round">
                            <path d="M3 6h18"/><path d="M8 6V4h8v2"/><path d="M19 6l-1 14H6L5 6"/>
                          </svg>
                        </button>
                      </div>
                    </div>
                  </div>

                  <div v-if="tunnelRules.length" class="tnl-hint">
                    拖拽左侧 <b>⠿</b> 手柄调整顺序，或聚焦后按 <kbd>↑</kbd> <kbd>↓</kbd> 移动
                  </div>
                </div>

                <input
                  v-else
                  :id="`f-${f.name}`"
                  :type="f.type === 'number' ? 'number' : 'text'"
                  :value="values[f.name]"
                  :placeholder="f.placeholder"
                  :class="{ invalid: hasError(f.name) }"
                  autocomplete="off"
                  autocapitalize="off"
                  autocorrect="off"
                  spellcheck="false"
                  @input="setField(f.name, ($event.target as HTMLInputElement).value)"
                >

                <span v-if="f.hint" class="desc">{{ f.hint }}</span>
                <span class="error-text">{{ errorText(f) }}</span>
              </div>

              <!-- 命令预览：从转发规则实时派生，跟随转发规则渲染，无需字段定义 -->
              <div v-if="card.id === 'forward'" class="field">
                <label>命令预览</label>
                <div class="tnl-preview">
                  <div class="tnl-preview-body">
                    <span v-for="(line, li) in tnlPreview" :key="li" class="ln"><template v-for="(tk, ti) in line" :key="ti"><span :class="tk.cls">{{ tk.text }}</span>{{ ' ' }}</template></span>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </section>
      </div>

      <!-- 底部 -->
      <div class="modal-foot">
        <button class="btn test" :class="{ loading: testing }" @click="testConnection">
          <span v-if="testing" class="spin"></span>
          <svg v-else viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
               stroke-linecap="round" stroke-linejoin="round">
            <polyline points="22 12 18 12 15 21 9 3 6 12 2 12"></polyline>
          </svg>
          {{ testing ? '测试中…' : '测试连接' }}
        </button>

        <span v-if="testResult" class="test-status" :class="testResult.kind">
          <span v-if="testResult.kind === 'loading'" class="spin"></span>
          <svg v-else-if="testResult.kind === 'ok'" viewBox="0 0 24 24" fill="none"
               stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="20 6 9 17 4 12"></polyline>
          </svg>
          <svg v-else viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4"
               stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="10"></circle>
            <line x1="12" y1="8" x2="12" y2="13"></line>
            <line x1="12" y1="16" x2="12.01" y2="16"></line>
          </svg>
          {{ testResult.text }}
        </span>

        <div class="spacer"></div>

        <span class="kbd">
          <b>Esc</b>
          <b>⌘T 测试</b>
          <b>⌘↵ 保存</b>
        </span>

        <button class="btn ghost" @click="close">取消</button>

        <button class="btn primary" :class="{ loading: saving }" @click="save">
          <span v-if="saving" class="spin"></span>
          <svg v-else viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
               stroke-linecap="round" stroke-linejoin="round">
            <path d="M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2z"></path>
            <polyline points="17 21 17 13 7 13 7 21"></polyline>
            <polyline points="7 3 7 8 15 8"></polyline>
          </svg>
          {{ saving ? '保存中…' : (isEdit ? '保存修改' : '保存并连接') }}
        </button>
      </div>
    </div>

    <!-- 隧道规则编辑子弹窗 -->
    <div v-if="tnlEditorOpen" class="tnl-mask" @click.self="closeTnlEditor">
      <div ref="tnlModalEl" class="tnl-modal" role="dialog" aria-modal="true" @keydown.enter.exact.prevent="saveTnlRule">
        <div class="tnl-modal-head">
          <span>{{ tnlEditingId ? '编辑转发规则' : '新建转发规则' }}</span>
          <button type="button" class="modal-close" title="关闭" @click="closeTnlEditor">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
                 stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
          </button>
        </div>

        <div class="tnl-modal-body">
          <div class="field">
            <label for="tnl-name">名称</label>
            <input
              id="tnl-name" v-model="tnlForm.name" type="text"
              placeholder="留空自动生成，如：本地转发 3306" maxlength="40"
              autocomplete="off" autocapitalize="off" autocorrect="off" spellcheck="false"
            >
          </div>

          <div class="field">
            <label>转发类型</label>
            <div class="segmented">
              <button
                v-for="(m, t) in TNL_META" :key="t" type="button"
                :class="{ active: tnlForm.type === t }"
                @click="tnlForm.type = t as TunnelType"
              >
                {{ m.label }} {{ m.flag }}
              </button>
            </div>
          </div>

          <div class="tnl-grid2">
            <div class="field">
              <label for="tnl-bindHost">{{ TNL_UI[tnlForm.type].bind }}</label>
              <input
                id="tnl-bindHost" v-model="tnlForm.bindHost" type="text"
                placeholder="127.0.0.1"
                autocomplete="off" autocapitalize="off" autocorrect="off" spellcheck="false"
              >
            </div>
            <div class="field">
              <label for="tnl-bindPort">端口</label>
              <div class="tnl-spin">
                <input
                  id="tnl-bindPort" :value="tnlForm.bindPort" type="number"
                  min="1" max="65535"
                  :class="{ invalid: tnlErrors.bindPort }"
                  @input="tnlForm.bindPort = ($event.target as HTMLInputElement).value; tnlErrors.bindPort = false"
                >
                <div class="tnl-spin-btns">
                  <button type="button" tabindex="-1" aria-label="增加" @click="stepTnlPort('bindPort', 1)">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M6 14l6-6 6 6"/></svg>
                  </button>
                  <button type="button" tabindex="-1" aria-label="减少" @click="stepTnlPort('bindPort', -1)">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M6 10l6 6 6-6"/></svg>
                  </button>
                </div>
              </div>
            </div>
          </div>

          <div v-show="tnlForm.type !== 'dynamic'" class="tnl-grid2">
            <div class="field">
              <label for="tnl-targetHost">{{ TNL_UI[tnlForm.type].target }}</label>
              <input
                id="tnl-targetHost" v-model="tnlForm.targetHost" type="text"
                placeholder="10.0.0.1"
                :class="{ invalid: tnlErrors.targetHost }"
                autocomplete="off" autocapitalize="off" autocorrect="off" spellcheck="false"
                @input="tnlErrors.targetHost = false"
              >
            </div>
            <div class="field">
              <label for="tnl-targetPort">端口</label>
              <div class="tnl-spin">
                <input
                  id="tnl-targetPort" :value="tnlForm.targetPort" type="number"
                  min="1" max="65535"
                  :class="{ invalid: tnlErrors.targetPort }"
                  @input="tnlForm.targetPort = ($event.target as HTMLInputElement).value; tnlErrors.targetPort = false"
                >
                <div class="tnl-spin-btns">
                  <button type="button" tabindex="-1" aria-label="增加" @click="stepTnlPort('targetPort', 1)">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M6 14l6-6 6 6"/></svg>
                  </button>
                  <button type="button" tabindex="-1" aria-label="减少" @click="stepTnlPort('targetPort', -1)">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M6 10l6 6 6-6"/></svg>
                  </button>
                </div>
              </div>
            </div>
          </div>

          <div class="tnl-type-hint">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"
                 stroke-linecap="round" stroke-linejoin="round">
              <circle cx="12" cy="12" r="10"/><path d="M12 16v-4"/><path d="M12 8h.01"/>
            </svg>
            <span>{{ TNL_UI[tnlForm.type].desc }}</span>
          </div>

          <div class="field">
            <label>等效命令</label>
            <div class="tnl-cmd">
              <code class="tnl-cmd-body"><template v-for="(tk, i) in tnlCmd" :key="i"><span :class="tk.cls">{{ tk.text }}</span>{{ ' ' }}</template></code>
              <button type="button" class="tnl-copy" title="复制命令" @click="copyTnlCmd">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                     stroke-linecap="round" stroke-linejoin="round">
                  <rect x="9" y="9" width="13" height="13" rx="2" ry="2"/>
                  <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>
                </svg>
              </button>
            </div>
          </div>
        </div>

        <div class="tnl-modal-foot">
          <button type="button" class="btn ghost" @click="closeTnlEditor">取消</button>
          <button type="button" class="btn primary" @click="saveTnlRule">保存</button>
        </div>
      </div>
    </div>
  </div>
</template>
