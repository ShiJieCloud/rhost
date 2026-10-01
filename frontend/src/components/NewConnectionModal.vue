<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { toast } from '../composables/useToast'
import { addHost, showNewConn } from '../stores/hosts'
import { COLOR_MAP, GROUP_OPTIONS } from '../data/mockHosts'
import type { HostColor } from '../types'

/* =========================================================
   类型与表单定义（数据驱动）
   ========================================================= */
type ConnType = 'ssh' | 'local' | 'serial' | 'docker' | 'telnet'
type Values = Record<string, string>

interface FieldOption { v: string; t: string }
interface FieldDef {
  label: string
  name: string
  span: 1 | 2
  required?: boolean
  type?: 'text' | 'number' | 'password' | 'select' | 'segmented' | 'color' | 'textarea'
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
}

const TYPES: { v: ConnType; t: string; icon: string; sub: string }[] = [
  { v: 'ssh', t: 'SSH', icon: 'ssh', sub: 'SSH · 安全远程主机连接' },
  { v: 'local', t: '本地', icon: 'local', sub: '本地 Shell 会话' },
  { v: 'serial', t: '串口', icon: 'serial', sub: '串口设备连接' },
  { v: 'docker', t: 'Docker', icon: 'docker', sub: 'Docker 容器终端' },
  { v: 'telnet', t: 'Telnet', icon: 'telnet', sub: 'Telnet 网络设备连接' },
]

const GROUP_OPTS: FieldOption[] = GROUP_OPTIONS.map(g => ({ v: g, t: g }))

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
          { label: '颜色标签', name: 'color', span: 2, type: 'color', value: 'green' },
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
        desc: '通过 SSH 隧道映射端口。每行一条，格式：端口:目标主机:目标端口。',
        summary: v => {
          const p: string[] = []
          if (v.localForward) p.push('本地')
          if (v.remoteForward) p.push('远程')
          if (v.dynamicForward) p.push('动态')
          return p.join(' · ')
        },
        fields: [
          { label: '本地转发 (-L)', name: 'localForward', span: 2, type: 'textarea',
            placeholder: '8080:localhost:80\n3306:db.internal:3306\n每行一条：本地端口:目标主机:目标端口' },
          { label: '远程转发 (-R)', name: 'remoteForward', span: 2, type: 'textarea',
            placeholder: '9090:localhost:3000\n每行一条：远程端口:目标主机:目标端口' },
          { label: '动态转发 (-D)', name: 'dynamicForward', span: 2, placeholder: '1080',
            hint: '本地 SOCKS5 代理端口' },
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
              { v: 'gbk', t: 'GBK' },
              { v: 'latin1', t: 'Latin-1' },
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
          { label: '颜色标签', name: 'color', span: 1, type: 'color', value: 'blue' },
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
          { label: '颜色标签', name: 'color', span: 2, type: 'color', value: 'yellow' },
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
          { label: '颜色标签', name: 'color', span: 2, type: 'color', value: 'cyan' },
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
          { label: '颜色标签', name: 'color', span: 2, type: 'color', value: 'purple' },
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
const formData = reactive({} as Record<ConnType, Values>)
const fieldErrors = reactive<Record<string, boolean>>({})
const errorCards = reactive<Record<string, boolean>>({})
const pwShown = reactive<Record<string, boolean>>({})
const testing = ref(false)
const saving = ref(false)
const testResult = ref<{ kind: 'ok' | 'err' | 'loading'; text: string } | null>(null)
const modalEl = ref<HTMLElement | null>(null)

const COLOR_KEYS = Object.keys(COLOR_MAP) as HostColor[]

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
watch(showNewConn, v => {
  if (!v) return
  TYPES.forEach(t => initType(t.v))
  Object.keys(fieldErrors).forEach(k => delete fieldErrors[k])
  Object.keys(errorCards).forEach(k => delete errorCards[k])
  Object.keys(pwShown).forEach(k => delete pwShown[k])
  currentType.value = 'ssh'
  currentCardId.value = FORMS.ssh.cards[0].id
  testResult.value = null
  testing.value = false
  saving.value = false
  nextTick(() => setTimeout(() => {
    const el = modalEl.value?.querySelector('.content-pane input:not([type=hidden])') as HTMLElement | null
    el?.focus()
  }, 180))
})

function close() {
  showNewConn.value = false
}

function onKey(e: KeyboardEvent) {
  if (!showNewConn.value) return
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
  if (t === currentType.value) return
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
  delete errorCards[`${currentType.value}.${currentCardId.value}`]
  testResult.value = null
}

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
   侧栏摘要与状态点
   ========================================================= */
function summaryOf(c: CardDef) {
  try {
    return c.summary(values.value) || ''
  } catch {
    return ''
  }
}

function cardStatus(cardId: string): 'error' | 'done' | 'partial' | 'empty' {
  if (errorCards[`${currentType.value}.${cardId}`]) return 'error'
  const c = cards.value.find(x => x.id === cardId)
  if (!c) return 'empty'
  const req = c.fields.filter(f => f.required && isVisible(f))
  if (!req.length) return 'done'
  const filled = req.filter(f => (values.value[f.name] ?? '').trim()).length
  if (filled === 0) return 'empty'
  return filled < req.length ? 'partial' : 'done'
}

/* =========================================================
   校验
   ========================================================= */
function validate(): boolean {
  Object.keys(fieldErrors).forEach(k => delete fieldErrors[k])
  Object.keys(errorCards).forEach(k => delete errorCards[k])

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
        errorCards[`${currentType.value}.${c.id}`] = true
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

  testing.value = true
  testResult.value = { kind: 'loading', text: '正在连接…' }

  // TODO: 接入后端后替换为 invoke('test_connection', { type, ...values })
  await new Promise(r => setTimeout(r, 900 + Math.random() * 700))

  const v = values.value
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
  // TODO: 接入后端后替换为 invoke('save_connection', { type, ...v })
  await new Promise(r => setTimeout(r, 500))

  if (currentType.value === 'ssh') {
    const label =
      v.name.replace(/[^a-zA-Z0-9一-龥]/g, '').slice(0, 2).toUpperCase() || 'SS'
    addHost({
      id: v.name,
      user: v.user,
      ip: v.host,
      port: parseInt(v.port, 10) || 22,
      os: 'Linux (未知发行版)',
      color: (v.color || 'green') as HostColor,
      label,
      tag: '新建',
      status: 'idle',
      lat: null,
      cpu: '—',
      mem: '—',
      uptime: '—',
      group: v.group || GROUP_OPTIONS[0],
    })
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
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
               stroke-linecap="round" stroke-linejoin="round">
            <line x1="12" y1="5" x2="12" y2="19"></line>
            <line x1="5" y1="12" x2="19" y2="12"></line>
          </svg>
        </div>
        <div class="modal-title">
          <h2 id="newConnTitle">新建连接</h2>
          <p>{{ typeSub }}</p>
        </div>
        <button class="modal-close" title="关闭" @click="close">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
               stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line></svg>
        </button>
      </div>

      <!-- 类型选择条 -->
      <div class="type-bar">
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
            :data-status="cardStatus(c.id)"
            @click="selectCard(c.id)"
          >
            <div class="snav-icon" v-html="ICONS[c.icon] || ICONS.info"></div>
            <div class="snav-body">
              <div class="snav-title">{{ c.title }}</div>
              <div class="snav-summary">{{ summaryOf(c) }}</div>
            </div>
            <div class="snav-badge"></div>
          </div>
        </nav>

        <section class="content-pane">
          <div class="pane-inner" :key="`${currentType}:${currentCardId}`">
            <div class="pane-header">
              <div class="pane-title">
                <span class="dot"></span>
                {{ card.title }}
              </div>
              <div v-if="card.desc" class="pane-desc">{{ card.desc }}</div>
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

                <div v-else-if="f.type === 'color'" class="color-row">
                  <button
                    v-for="c in COLOR_KEYS"
                    :key="c"
                    type="button"
                    class="color-dot"
                    :class="{ active: values[f.name] === c }"
                    :style="{ '--c': COLOR_MAP[c] }"
                    :title="c"
                    @click="setField(f.name, c)"
                  ></button>
                </div>

                <textarea
                  v-else-if="f.type === 'textarea'"
                  :id="`f-${f.name}`"
                  :value="values[f.name]"
                  :placeholder="f.placeholder"
                  :class="{ invalid: hasError(f.name) }"
                  @input="setField(f.name, ($event.target as HTMLTextAreaElement).value)"
                ></textarea>

                <input
                  v-else
                  :id="`f-${f.name}`"
                  :type="f.type === 'number' ? 'number' : 'text'"
                  :value="values[f.name]"
                  :placeholder="f.placeholder"
                  :class="{ invalid: hasError(f.name) }"
                  autocomplete="off"
                  @input="setField(f.name, ($event.target as HTMLInputElement).value)"
                >

                <span v-if="f.hint" class="desc">{{ f.hint }}</span>
                <span class="error-text">{{ errorText(f) }}</span>
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
          {{ saving ? '保存中…' : '保存并连接' }}
        </button>
      </div>
    </div>
  </div>
</template>
