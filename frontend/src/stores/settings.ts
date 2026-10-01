import { reactive, ref } from 'vue'

/** 设置弹窗显隐（模块级单例，同 showNewConn 模式） */
export const showSettings = ref(false)

export interface EnvVar {
  key: string
  value: string
}

export interface AppSettings {
  /* ---- 外观 ---- */
  colorScheme: string
  uiTheme: string
  accent: string
  opacity: number
  padding: number
  hideTitlebar: boolean
  tabPosition: string
  cursorStyle: string
  cursorBlink: boolean
  /* ---- 字体 ---- */
  fontFamily: string
  fontSize: number
  lineHeight: number
  fontWeight: string
  ligatures: boolean
  /* ---- 终端 ---- */
  shellPath: string
  startDir: string
  loginShell: boolean
  startupCommand: string
  scrollback: number
  trimOnCopy: boolean
  bell: boolean
  pasteGuard: boolean
  rightClick: string
  env: EnvVar[]
  /* ---- 快捷键 ---- */
  'key.newTab': string
  'key.closeTab': string
  'key.splitV': string
  'key.splitH': string
  'key.clear': string
  'key.palette': string
  'key.find': string
  'key.settings': string
  /* ---- 高级 ---- */
  gpuAccel: boolean
  renderer: string
  fps: string
  /* ---- 关于 ---- */
  autoUpdate: boolean
  updateChannel: string
}

export const DEFAULT_SETTINGS: AppSettings = {
  colorScheme: 'one-dark',
  uiTheme: 'dark',
  accent: '#3ddc84',
  opacity: 96,
  padding: 10,
  hideTitlebar: false,
  tabPosition: 'top',
  cursorStyle: 'bar',
  cursorBlink: true,

  fontFamily: 'JetBrains Mono',
  fontSize: 13,
  lineHeight: 145,
  fontWeight: '400',
  ligatures: true,

  shellPath: '/bin/zsh',
  startDir: '~',
  loginShell: true,
  startupCommand: '',
  scrollback: 10000,
  trimOnCopy: true,
  bell: false,
  pasteGuard: true,
  rightClick: 'menu',
  env: [
    { key: 'EDITOR', value: 'nvim' },
    { key: 'LANG', value: 'zh_CN.UTF-8' },
  ],

  'key.newTab': '⌘+T',
  'key.closeTab': '⌘+W',
  'key.splitV': '⌘+D',
  'key.splitH': '⌘+E',
  'key.clear': '⌘+L',
  'key.palette': '⌘+K',
  'key.find': '⌘+F',
  'key.settings': '⌘+,',

  gpuAccel: true,
  renderer: 'auto',
  fps: '60',

  autoUpdate: true,
  updateChannel: 'stable',
}

const SETTINGS_KEY = 'rhost.settings'

function load(): AppSettings {
  try {
    const raw = JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? '{}') as Partial<AppSettings>
    return { ...DEFAULT_SETTINGS, ...raw }
  } catch {
    return { ...DEFAULT_SETTINGS }
  }
}

/** 已保存的设置（持久化） */
export const savedSettings = reactive<AppSettings>(load())

export function persistSettings() {
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(savedSettings))
  } catch { /* 隐私模式等场景忽略 */ }
}

/** 强调色应用到全局（view-card 等处 var(--accent) 生效） */
export function applyAccent(color: string) {
  document.documentElement.style.setProperty('--accent', color)
}
applyAccent(savedSettings.accent)

export function saveSettings(draft: AppSettings) {
  const next = JSON.parse(JSON.stringify(draft)) as AppSettings
  Object.assign(savedSettings, next)
  persistSettings()
  applyAccent(savedSettings.accent)
}
