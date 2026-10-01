<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import AppLogo from './AppLogo.vue'
import { toast } from '../composables/useToast'
import {
  DEFAULT_SETTINGS, savedSettings, saveSettings, showSettings,
  type AppSettings,
} from '../stores/settings'

/* =========================================================
   面板定义（数据驱动）
   ========================================================= */
type CtrlKind =
  | 'segmented' | 'switch' | 'select' | 'range' | 'text' | 'color'
  | 'keys' | 'buttons'

interface Opt { v: string; t: string }
interface RowDef {
  key?: keyof AppSettings
  title: string
  desc?: string
  keywords?: string
  kind?: CtrlKind
  options?: Opt[]
  min?: number
  max?: number
  step?: number
  unit?: string
  width?: number
  placeholder?: string
  actions?: { id: 'export' | 'import' | 'reset' | 'changelog' | 'checkUpdate'; label: string; danger?: boolean }[]
}
interface GroupDef {
  label: string
  rows: RowDef[]
  extra?: 'themeGrid' | 'envEditor' | 'preview'
}
interface PanelDef {
  id: string
  label: string
  glyph: string
  title: string
  sub: string
  groups: GroupDef[]
  about?: boolean
}

const ICONS: Record<string, string> = {
  appearance: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 3a9 9 0 000 18z" fill="currentColor" stroke="none"/></svg>',
  font: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="4 7 4 4 20 4 20 7"/><line x1="12" y1="4" x2="12" y2="20"/><line x1="9" y1="20" x2="15" y2="20"/></svg>',
  terminal: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M7 10l3 2-3 2M13 14h4"/></svg>',
  keymap: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="6" width="20" height="12" rx="2"/><path d="M6 10h.01M10 10h.01M14 10h.01M18 10h.01M7 14h10"/></svg>',
  advanced: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 00.3 1.9l.1.1a2 2 0 11-2.8 2.8l-.1-.1a1.7 1.7 0 00-1.9-.3 1.7 1.7 0 00-1 1.5V21a2 2 0 11-4 0v-.1a1.7 1.7 0 00-1.1-1.5 1.7 1.7 0 00-1.9.3l-.1.1a2 2 0 11-2.8-2.8l.1-.1a1.7 1.7 0 00.3-1.9 1.7 1.7 0 00-1.5-1H3a2 2 0 110-4h.1a1.7 1.7 0 001.5-1.1 1.7 1.7 0 00-.3-1.9l-.1-.1a2 2 0 112.8-2.8l.1.1a1.7 1.7 0 001.9.3h.1a1.7 1.7 0 001-1.5V3a2 2 0 114 0v.1a1.7 1.7 0 001 1.5 1.7 1.7 0 001.9-.3l.1-.1a2 2 0 112.8 2.8l-.1.1a1.7 1.7 0 00-.3 1.9v.1a1.7 1.7 0 001.5 1H21a2 2 0 110 4h-.1a1.7 1.7 0 00-1.5 1z"/></svg>',
  about: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><line x1="12" y1="11" x2="12" y2="16"/><line x1="12" y1="8" x2="12.01" y2="8"/></svg>',
}

const THEME_SWATCHES: Record<string, string[]> = {
  'one-dark': ['#282c34', '#61afef', '#98c379', '#e06c75', '#e5c07b'],
  dracula: ['#282a36', '#bd93f9', '#50fa7b', '#ff79c6', '#f1fa8c'],
  nord: ['#2e3440', '#88c0d0', '#a3be8c', '#bf616a', '#ebcb8b'],
  solarized: ['#002b36', '#268bd2', '#859900', '#dc322f', '#b58900'],
}
const THEME_LABELS: Record<string, string> = {
  'one-dark': 'One Dark',
  dracula: 'Dracula',
  nord: 'Nord',
  solarized: 'Solarized',
}
const ACCENTS = [
  { v: '#3ddc84', t: '绿' },
  { v: '#7aa2f7', t: '蓝' },
  { v: '#bb9af7', t: '紫' },
  { v: '#e5b567', t: '黄' },
  { v: '#f7768e', t: '红' },
  { v: '#4fd6e0', t: '青' },
]

const PANELS: PanelDef[] = [
  {
    id: 'appearance', label: '外观', glyph: 'appearance',
    title: '外观', sub: '调整配色、光标与窗口效果，更改会即时反映在应用中',
    groups: [
      {
        label: '主题', extra: 'themeGrid',
        rows: [
          { key: 'colorScheme', title: '配色方案', desc: '终端内容的 ANSI 调色板', keywords: '配色 主题 theme color scheme 颜色' },
        ],
      },
      {
        label: '界面',
        rows: [
          { key: 'uiTheme', title: '界面主题', desc: '跟随系统时随系统外观自动切换', kind: 'segmented', keywords: '界面 主题 明暗 深色 浅色 跟随系统',
            options: [{ v: 'dark', t: '深色' }, { v: 'light', t: '浅色' }, { v: 'auto', t: '跟随系统' }] },
          { key: 'accent', title: '强调色', desc: '用于按钮、选中态与焦点环', kind: 'color', keywords: '强调色 主题色 accent 颜色' },
        ],
      },
      {
        label: '窗口',
        rows: [
          { key: 'opacity', title: '背景不透明度', desc: '降低数值可获得毛玻璃效果', kind: 'range', min: 60, max: 100, unit: '%', keywords: '透明度 不透明 opacity 毛玻璃 模糊 blur' },
          { key: 'padding', title: '内容内边距', desc: '终端内容与窗口边缘的距离', kind: 'range', min: 0, max: 24, unit: 'px', keywords: '内边距 padding 边距' },
          { key: 'hideTitlebar', title: '隐藏标题栏', desc: '仅保留内容区域，视觉更沉浸', kind: 'switch', keywords: '标题栏 隐藏 沉浸 titlebar' },
          { key: 'tabPosition', title: '标签栏位置', kind: 'segmented', keywords: '标签栏 位置 tab 顶部 底部',
            options: [{ v: 'top', t: '顶部' }, { v: 'bottom', t: '底部' }] },
        ],
      },
      {
        label: '光标',
        rows: [
          { key: 'cursorStyle', title: '光标样式', kind: 'segmented', keywords: '光标 样式 cursor 方块 竖线 下划线',
            options: [{ v: 'block', t: '方块' }, { v: 'bar', t: '竖线' }, { v: 'underline', t: '下划线' }] },
          { key: 'cursorBlink', title: '光标闪烁', desc: '空闲 1 秒后开始闪烁', kind: 'switch', keywords: '光标 闪烁 blink cursor' },
        ],
      },
    ],
  },
  {
    id: 'font', label: '字体', glyph: 'font',
    title: '字体', sub: '选择终端渲染文本所使用的字形与排版参数',
    groups: [
      {
        label: '字形',
        rows: [
          { key: 'fontFamily', title: '等宽字体', desc: '需要系统已安装该字体', kind: 'select', width: 190, keywords: '字体 等宽 font family 字形',
            options: ['JetBrains Mono', 'SF Mono', 'Fira Code', 'Cascadia Code', 'Menlo', 'monospace'].map(v => ({ v, t: v === 'monospace' ? '系统等宽' : v })) },
          { key: 'fontSize', title: '字号', kind: 'range', min: 10, max: 20, unit: 'px', keywords: '字号 大小 font size' },
          { key: 'lineHeight', title: '行高', kind: 'range', min: 100, max: 200, unit: '%', keywords: '行高 行距 line height' },
          { key: 'fontWeight', title: '字重', kind: 'select', width: 120, keywords: '字重 粗细 weight',
            options: [{ v: '300', t: '细体' }, { v: '400', t: '常规' }, { v: '500', t: '中等' }, { v: '700', t: '粗体' }] },
          { key: 'ligatures', title: '编程连字', desc: '将 =>、!= 等符号合并显示', kind: 'switch', keywords: '连字 编程连字 ligature 合字' },
        ],
      },
      {
        label: '实时预览', extra: 'preview',
        rows: [],
      },
    ],
  },
  {
    id: 'terminal', label: '终端', glyph: 'terminal',
    title: '终端', sub: '配置 Shell 启动方式、会话行为与环境变量',
    groups: [
      {
        label: '启动',
        rows: [
          { key: 'shellPath', title: 'Shell 路径', desc: '留空则使用系统默认 Shell', kind: 'text', width: 230, keywords: 'shell 路径 程序' },
          { key: 'startDir', title: '启动目录', kind: 'text', width: 230, keywords: '启动目录 工作目录 cwd 路径' },
          { key: 'loginShell', title: '登录 Shell', desc: '以登录模式启动，加载完整环境变量', kind: 'switch', keywords: '登录 shell login 环境变量' },
          { key: 'startupCommand', title: '启动时执行', desc: '每次新建会话后自动运行，可留空', kind: 'text', width: 230, placeholder: '例如：source ~/.zshrc', keywords: '启动命令 执行 初始化 command' },
        ],
      },
      {
        label: '行为',
        rows: [
          { key: 'scrollback', title: '回滚缓冲区', desc: '可向上滚动的历史行数', kind: 'range', min: 1000, max: 100000, step: 1000, unit: ' 行', keywords: '回滚 缓冲 历史 行数 scrollback' },
          { key: 'trimOnCopy', title: '复制时去除行尾空格', kind: 'switch', keywords: '复制 行尾空格 修剪 trim copy' },
          { key: 'bell', title: '终端响铃', desc: '命令完成或出错时发出提示音', kind: 'switch', keywords: '响铃 提示音 bell 声音' },
          { key: 'pasteGuard', title: '粘贴保护', desc: '多行粘贴时弹窗确认，避免误执行', kind: 'switch', keywords: '粘贴 保护 确认 paste 安全' },
          { key: 'rightClick', title: '右键行为', kind: 'segmented', keywords: '右键 粘贴 菜单 right click 鼠标',
            options: [{ v: 'menu', t: '菜单' }, { v: 'paste', t: '粘贴' }] },
        ],
      },
      {
        label: '环境变量', extra: 'envEditor',
        rows: [],
      },
    ],
  },
  {
    id: 'keymap', label: '快捷键', glyph: 'keymap',
    title: '快捷键', sub: '点击任意组合即可重新录制，按 Esc 取消',
    groups: [
      {
        label: '会话',
        rows: [
          { key: 'key.newTab', title: '新建标签页', kind: 'keys', keywords: '新建 标签页 new tab 快捷键' },
          { key: 'key.closeTab', title: '关闭标签页', kind: 'keys', keywords: '关闭 标签页 close tab 快捷键' },
          { key: 'key.splitV', title: '垂直分屏', kind: 'keys', keywords: '分屏 垂直 split 快捷键' },
          { key: 'key.splitH', title: '水平分屏', kind: 'keys', keywords: '水平 分屏 split 快捷键' },
        ],
      },
      {
        label: '操作',
        rows: [
          { key: 'key.clear', title: '清空屏幕', kind: 'keys', keywords: '清空 屏幕 clear 快捷键' },
          { key: 'key.palette', title: '命令面板', kind: 'keys', keywords: '命令 面板 command palette 快捷键' },
          { key: 'key.find', title: '查找', kind: 'keys', keywords: '搜索 查找 find 快捷键' },
          { key: 'key.settings', title: '打开设置', kind: 'keys', keywords: '设置 偏好 preferences 快捷键' },
        ],
      },
    ],
  },
  {
    id: 'advanced', label: '高级', glyph: 'advanced',
    title: '高级', sub: '渲染、性能与配置文件管理',
    groups: [
      {
        label: '渲染',
        rows: [
          { key: 'gpuAccel', title: 'GPU 加速', desc: '关闭后使用软件渲染，可解决花屏问题', kind: 'switch', keywords: 'gpu 加速 硬件 渲染 acceleration 性能' },
          { key: 'renderer', title: '渲染后端', kind: 'select', width: 140, keywords: '渲染器 后端 renderer webgl webgpu',
            options: [{ v: 'auto', t: '自动' }, { v: 'webgl', t: 'WebGL' }, { v: 'webgpu', t: 'WebGPU' }, { v: 'canvas', t: 'Canvas' }] },
          { key: 'fps', title: '动画帧率', desc: '降低可减少电量消耗', kind: 'segmented', keywords: '刷新率 帧率 fps 动画 性能',
            options: [{ v: '30', t: '30' }, { v: '60', t: '60' }, { v: '120', t: '120' }] },
        ],
      },
      {
        label: '配置文件',
        rows: [
          { title: '导入 / 导出', desc: '将当前设置导出为 JSON，或从剪贴板恢复', kind: 'buttons', keywords: '导入 导出 配置 备份 config 文件',
            actions: [{ id: 'export', label: '导出' }, { id: 'import', label: '导入' }] },
          { title: '恢复默认设置', desc: '将所有选项还原为初始值，此操作不可撤销', kind: 'buttons', keywords: '重置 恢复 默认 reset 全部',
            actions: [{ id: 'reset', label: '恢复默认', danger: true }] },
        ],
      },
    ],
  },
  {
    id: 'about', label: '关于', glyph: 'about',
    title: '关于', sub: '版本信息与更新通道', about: true,
    groups: [
      {
        label: '更新',
        rows: [
          { key: 'autoUpdate', title: '自动检查更新', desc: '启动时在后台检查新版本', kind: 'switch', keywords: '自动更新 检查 update 升级' },
          { key: 'updateChannel', title: '更新通道', kind: 'select', width: 140, keywords: '更新通道 beta 稳定 stable channel',
            options: [{ v: 'stable', t: '稳定版' }, { v: 'beta', t: '测试版' }, { v: 'nightly', t: '每日构建' }] },
        ],
      },
    ],
  },
]

/* =========================================================
   状态
   ========================================================= */
const activePanelId = ref('appearance')
const query = ref('')
const searchEl = ref<HTMLInputElement | null>(null)
const modalEl = ref<HTMLElement | null>(null)

const draft = reactive(JSON.parse(JSON.stringify(savedSettings)) as AppSettings)

watch(showSettings, v => {
  if (!v) return
  Object.assign(draft, JSON.parse(JSON.stringify(savedSettings)))
  query.value = ''
  activePanelId.value = 'appearance'
  recordingKey.value = null
  nextTick(() => searchEl.value?.focus())
})

function close() {
  showSettings.value = false
}

const activePanel = computed(() => PANELS.find(p => p.id === activePanelId.value) ?? PANELS[0])

/* ---- 搜索 ---- */
function rowMatch(r: RowDef, q: string): boolean {
  return ((r.keywords ?? '') + ' ' + r.title + ' ' + (r.desc ?? '')).toLowerCase().includes(q)
}
const displayPanels = computed(() => {
  const q = query.value.trim().toLowerCase()
  if (!q) return [{ ...activePanel.value, searching: false, groups: activePanel.value.groups }]
  const out: (PanelDef & { searching: boolean; groups: GroupDef[] })[] = []
  for (const p of PANELS) {
    const groups = p.groups
      .map(g => ({ ...g, rows: g.rows.filter(r => rowMatch(r, q)) }))
      .filter(g => g.rows.length)
    if (groups.length) out.push({ ...p, searching: true, groups })
  }
  return out
})
const noResults = computed(() => !!query.value.trim() && !displayPanels.value.length)

/* ---- 脏检测 ---- */
function isDirty(key: keyof AppSettings): boolean {
  return JSON.stringify(draft[key]) !== JSON.stringify(savedSettings[key])
}
function groupDirty(g: GroupDef): boolean {
  return g.rows.some(r => r.key && isDirty(r.key)) ||
    (g.extra === 'envEditor' && g.rows.length === 0 && isDirty('env'))
}
function panelDirty(p: PanelDef): boolean {
  return p.groups.some(groupDirty)
}
const changedCount = computed(() => {
  let n = 0
  for (const p of PANELS) if (panelDirty(p)) n++
  return n
})

/* ---- 值写入 ---- */
function setVal(key: keyof AppSettings, v: unknown) {
  ;(draft as Record<string, unknown>)[key] = v
}
function strVal(key: keyof AppSettings | undefined): string {
  if (!key) return ''
  const v = draft[key]
  return typeof v === 'string' ? v : String(v ?? '')
}
function numVal(key: keyof AppSettings): number {
  return draft[key] as number
}
function boolVal(key: keyof AppSettings): boolean {
  return draft[key] as boolean
}
function resetOne(key: keyof AppSettings) {
  setVal(key, JSON.parse(JSON.stringify(savedSettings[key])))
  toast('已重置该项', 'info', 1500)
}

/* ---- 环境变量 ---- */
function addEnv() {
  draft.env.push({ key: '', value: '' })
  nextTick(() => {
    const inputs = modalEl.value?.querySelectorAll('.st-env-row:last-child input')
    ;(inputs?.[0] as HTMLElement | null)?.focus()
  })
}
function delEnv(i: number) {
  draft.env.splice(i, 1)
}
function setEnvKey(i: number, v: string) { draft.env[i]!.key = v }
function setEnvValue(i: number, v: string) { draft.env[i]!.value = v }

/* ---- 快捷键录制 ---- */
const recordingKey = ref<keyof AppSettings | null>(null)
const keyLabels = ['Control', 'Meta', 'Alt', 'Shift', 'Escape']
function splitKeys(s: string): string[] {
  return s.split('+').filter(Boolean)
}
function resetRecording() {
  recordingKey.value = null
}

/* ---- 全局键盘 ---- */
function onKey(e: KeyboardEvent) {
  if (!showSettings.value) return

  if (recordingKey.value) {
    e.preventDefault()
    const key = recordingKey.value
    if (e.key === 'Escape') {
      resetRecording()
      toast('已取消录制', 'info', 1500)
      return
    }
    if (keyLabels.includes(e.key)) return // 只按了修饰键，继续等待
    const parts: string[] = []
    if (e.metaKey) parts.push('⌘')
    if (e.ctrlKey) parts.push('Ctrl')
    if (e.altKey) parts.push('Alt')
    if (e.shiftKey) parts.push('Shift')
    let k = e.key
    if (k === ' ') k = 'Space'
    else if (k.length === 1) k = k.toUpperCase()
    parts.push(k)
    setVal(key, parts.join('+'))
    resetRecording()
    toast('快捷键已更新', 'ok', 1600)
    return
  }

  if (e.key === 'Escape') {
    e.preventDefault()
    if (document.activeElement === searchEl.value) {
      searchEl.value?.blur()
      return
    }
    close()
  }
}
onMounted(() => document.addEventListener('keydown', onKey))
onUnmounted(() => document.removeEventListener('keydown', onKey))

/* ---- 保存 / 放弃 / 恢复默认 ---- */
function save() {
  saveSettings(draft)
  toast('设置已保存', 'ok', 1800)
}
function discard() {
  Object.assign(draft, JSON.parse(JSON.stringify(savedSettings)))
  toast('已放弃未保存的更改', 'info', 1600)
}
function resetAll() {
  Object.assign(draft, JSON.parse(JSON.stringify(DEFAULT_SETTINGS)))
  toast('已恢复默认设置，保存后生效', 'info', 2000)
}

/* ---- 导入 / 导出 ---- */
async function exportCfg() {
  const data = JSON.stringify(draft, null, 2)
  try {
    await navigator.clipboard.writeText(data)
    toast('配置已复制到剪贴板', 'ok', 2000)
  } catch {
    toast('导出失败，请检查权限', 'err', 2200)
  }
}
function importCfg() {
  // TODO: 接入文件选择与配置校验
  toast('导入功能开发中', 'info')
}
function onAction(id: string) {
  if (id === 'export') exportCfg()
  else if (id === 'import') importCfg()
  else if (id === 'reset') resetAll()
  else if (id === 'checkUpdate') toast('当前已是最新版本', 'ok', 1800)
  else if (id === 'changelog') toast('更新日志开发中', 'info')
}

/* ---- 字体预览 ---- */
const previewStyle = computed(() => ({
  fontFamily: draft.fontFamily === 'monospace'
    ? 'ui-monospace, SFMono-Regular, Menlo, monospace'
    : `"${draft.fontFamily}", ui-monospace, monospace`,
  fontSize: draft.fontSize + 'px',
  lineHeight: (draft.lineHeight / 100).toFixed(2),
  fontWeight: draft.fontWeight,
}))
const previewAccent = computed(() =>
  ({ 'one-dark': '#61afef', dracula: '#bd93f9', nord: '#88c0d0', solarized: '#268bd2' })[draft.colorScheme] ?? 'var(--blue)')

/* ---- 控件辅助 ---- */
function segActive(key: keyof AppSettings, v: string): boolean {
  return strVal(key) === v
}
</script>

<template>
  <div class="mask" :class="{ show: showSettings }" @click.self="close">
    <div ref="modalEl" class="modal settings" role="dialog" aria-modal="true" aria-label="设置">
      <!-- 头部 -->
      <div class="modal-head">
        <div class="modal-icon">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
               stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="3"/>
            <path d="M19.4 15a1.7 1.7 0 00.3 1.9l.1.1a2 2 0 11-2.8 2.8l-.1-.1a1.7 1.7 0 00-1.9-.3 1.7 1.7 0 00-1 1.5V21a2 2 0 11-4 0v-.1a1.7 1.7 0 00-1.1-1.5 1.7 1.7 0 00-1.9.3l-.1.1a2 2 0 11-2.8-2.8l.1-.1a1.7 1.7 0 00.3-1.9 1.7 1.7 0 00-1.5-1H3a2 2 0 110-4h.1a1.7 1.7 0 001.5-1.1 1.7 1.7 0 00-.3-1.9l-.1-.1a2 2 0 112.8-2.8l.1.1a1.7 1.7 0 001.9.3h.1a1.7 1.7 0 001-1.5V3a2 2 0 114 0v.1a1.7 1.7 0 001 1.5 1.7 1.7 0 001.9-.3l.1-.1a2 2 0 112.8 2.8l-.1.1a1.7 1.7 0 00-.3 1.9v.1a1.7 1.7 0 001.5 1H21a2 2 0 110 4h-.1a1.7 1.7 0 00-1.5 1z"/>
          </svg>
        </div>
        <div class="modal-title">
          <h2>设置</h2>
          <p>偏好与全局配置 · 更改后点击保存生效</p>
        </div>
        <button class="modal-close" title="关闭 (Esc)" @click="close">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
               stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line></svg>
        </button>
      </div>

      <!-- 主体 -->
      <div class="st-body">
        <!-- 侧栏 -->
        <aside class="st-side">
          <div class="st-search">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
              <circle cx="11" cy="11" r="7"/><path d="M20 20l-3.5-3.5"/>
            </svg>
            <input
              ref="searchEl"
              v-model="query"
              type="text"
              placeholder="搜索设置…"
              autocomplete="off"
              spellcheck="false"
            >
            <button v-if="query" class="st-search-clear" title="清除" @click="query = ''">✕</button>
          </div>

          <nav class="st-nav">
            <button
              v-for="p in PANELS"
              :key="p.id"
              class="st-nav-item"
              :class="{ active: !query && activePanelId === p.id, dirty: panelDirty(p) }"
              @click="activePanelId = p.id; query = ''"
            >
              <span class="st-glyph" v-html="ICONS[p.glyph]"></span>
              <span>{{ p.label }}</span>
              <span class="st-dirty-dot"></span>
            </button>
          </nav>
        </aside>

        <!-- 内容 -->
        <main class="st-content">
          <section
            v-for="p in displayPanels"
            :key="p.id"
            class="st-panel"
          >
            <template v-if="!p.searching">
              <div class="st-panel-head">
                <div class="st-panel-title">{{ p.title }}</div>
                <div class="st-panel-sub">{{ p.sub }}</div>
              </div>

              <!-- 关于卡片 -->
              <div v-if="p.about" class="st-about">
                <div class="st-about-logo"><AppLogo /></div>
                <div>
                  <div class="st-about-name">Rhost</div>
                  <div class="st-about-ver">v0.1.0 · 开发版</div>
                  <div class="st-about-desc">轻量、快速的 SSH 终端工作台。<br>基于 Web 技术构建，支持 GPU 加速渲染。</div>
                </div>
              </div>
              <div v-if="p.about" class="st-link-row">
                <button class="btn" @click="onAction('changelog')">查看更新日志</button>
                <button class="btn" @click="onAction('checkUpdate')">检查更新</button>
              </div>
            </template>
            <div v-else class="st-search-label">{{ p.label }}</div>

            <div
              v-for="g in p.groups"
              :key="g.label"
              class="st-group"
            >
              <div class="st-group-label">{{ g.label }}</div>

              <div v-if="g.rows.length" class="st-rows">
                <div
                  v-for="r in g.rows"
                  :key="r.title"
                  class="st-row"
                  :class="{ modified: r.key && isDirty(r.key) }"
                >
                  <div class="st-row-main">
                    <div class="st-row-title">
                      {{ r.title }}
                      <button
                        v-if="r.key"
                        class="st-reset"
                        title="重置此项"
                        @click="resetOne(r.key)"
                      >↺</button>
                    </div>
                    <div v-if="r.desc" class="st-row-desc">{{ r.desc }}</div>
                  </div>

                  <div class="st-row-ctl">
                    <!-- 开关 -->
                    <label v-if="r.kind === 'switch'" class="st-switch">
                      <input
                        type="checkbox"
                        :checked="boolVal(r.key!)"
                        @change="setVal(r.key!, ($event.target as HTMLInputElement).checked)"
                      >
                    </label>

                    <!-- 分段 -->
                    <div v-else-if="r.kind === 'segmented'" class="segmented">
                      <button
                        v-for="op in r.options"
                        :key="op.v"
                        type="button"
                        :class="{ active: segActive(r.key!, op.v) }"
                        @click="setVal(r.key!, op.v)"
                      >{{ op.t }}</button>
                    </div>

                    <!-- 下拉 -->
                    <select
                      v-else-if="r.kind === 'select'"
                      class="st-select"
                      :style="{ width: (r.width ?? 150) + 'px' }"
                      :value="strVal(r.key)"
                      @change="setVal(r.key!, ($event.target as HTMLSelectElement).value)"
                    >
                      <option v-for="op in r.options" :key="op.v" :value="op.v">{{ op.t }}</option>
                    </select>

                    <!-- 滑块 -->
                    <template v-else-if="r.kind === 'range'">
                      <input
                        type="range"
                        :min="r.min"
                        :max="r.max"
                        :step="r.step ?? 1"
                        :value="numVal(r.key!)"
                        @input="setVal(r.key!, Number(($event.target as HTMLInputElement).value))"
                      >
                      <span class="st-val">{{ numVal(r.key!) }}{{ r.unit }}</span>
                    </template>

                    <!-- 文本 -->
                    <input
                      v-else-if="r.kind === 'text'"
                      class="st-input"
                      type="text"
                      :style="{ width: (r.width ?? 200) + 'px' }"
                      :value="strVal(r.key)"
                      :placeholder="r.placeholder"
                      spellcheck="false"
                      autocomplete="off"
                      @input="setVal(r.key!, ($event.target as HTMLInputElement).value)"
                    >

                    <!-- 强调色 -->
                    <div v-else-if="r.kind === 'color'" class="st-colors">
                      <button
                        v-for="c in ACCENTS"
                        :key="c.v"
                        type="button"
                        class="st-color-dot"
                        :class="{ active: strVal(r.key) === c.v }"
                        :style="{ '--c': c.v }"
                        :title="c.t"
                        @click="setVal(r.key!, c.v)"
                      ></button>
                    </div>

                    <!-- 快捷键 -->
                    <div
                      v-else-if="r.kind === 'keys'"
                      class="st-keys"
                      :class="{ recording: recordingKey === r.key }"
                      @click="recordingKey = r.key!"
                    >
                      <template v-if="recordingKey === r.key">
                        <kbd class="rec">按下组合键…</kbd>
                        <span class="st-keys-cancel" title="取消 (Esc)" @click.stop="resetRecording">✕</span>
                      </template>
                      <template v-else>
                        <kbd v-for="(k, i) in splitKeys(strVal(r.key))" :key="i">{{ k }}</kbd>
                      </template>
                    </div>

                    <!-- 按钮组 -->
                    <template v-else-if="r.kind === 'buttons'">
                      <button
                        v-for="a in r.actions"
                        :key="a.id"
                        class="btn"
                        :class="{ danger: a.danger }"
                        @click="onAction(a.id)"
                      >{{ a.label }}</button>
                    </template>
                  </div>
                </div>
              </div>

              <!-- 配色方案卡片 -->
              <div v-if="g.extra === 'themeGrid' && !p.searching" class="st-theme-grid">
                <button
                  v-for="(sw, name) in THEME_SWATCHES"
                  :key="name"
                  type="button"
                  class="st-theme-card"
                  :class="{ active: strVal('colorScheme') === name }"
                  @click="setVal('colorScheme', name)"
                >
                  <div class="st-theme-swatches">
                    <span v-for="c in sw" :key="c" :style="{ background: c }"></span>
                  </div>
                  <div class="st-theme-name">{{ THEME_LABELS[name] }}</div>
                </button>
              </div>

              <!-- 环境变量编辑 -->
              <template v-if="g.extra === 'envEditor' && !p.searching">
                <div class="st-env-list">
                  <div v-for="(e, i) in draft.env" :key="i" class="st-env-row">
                    <input class="st-input key" type="text" :value="e.key" placeholder="KEY"
                           spellcheck="false" autocomplete="off" @input="setEnvKey(i, ($event.target as HTMLInputElement).value)">
                    <input class="st-input" type="text" :value="e.value" placeholder="value"
                           spellcheck="false" autocomplete="off" @input="setEnvValue(i, ($event.target as HTMLInputElement).value)">
                    <button class="st-env-del" title="删除" @click="delEnv(i)">✕</button>
                  </div>
                </div>
                <button class="st-btn-ghost" @click="addEnv">＋ 添加变量</button>
              </template>

              <!-- 字体实时预览 -->
              <div v-if="g.extra === 'preview' && !p.searching" class="st-preview" :style="previewStyle">
                <div class="ln"><span class="u" :style="{ color: previewAccent }">user@dev</span>:<span class="u" :style="{ color: previewAccent }">~/projects</span>$ npm run build</div>
                <div class="ln gap"></div>
                <div class="ln"><span class="g">✔</span> 编译完成，用时 1.24s</div>
                <div class="ln"><span class="y">⚠</span> 2 个依赖已过时</div>
                <div class="ln c"># 连字测试: =&gt; != === -&gt; &lt;= ::</div>
              </div>
            </div>
          </section>

          <!-- 搜索空态 -->
          <div v-if="noResults" class="st-empty">
            <div class="st-empty-icon">⌕</div>
            <div class="st-empty-title">未找到匹配的设置项</div>
            <div class="st-empty-sub">试试更短的关键词，例如「字体」「光标」「主题」</div>
          </div>
        </main>
      </div>

      <!-- 底栏 -->
      <div class="modal-foot">
        <div class="st-foot-status" :class="{ dirty: changedCount > 0 }">
          <span class="ind"></span>
          {{ changedCount > 0 ? `${changedCount} 个分类有未保存的更改` : '所有更改已保存' }}
        </div>

        <div class="spacer"></div>

        <span class="kbd"><b>Esc 关闭</b></span>
        <button class="btn ghost" :disabled="changedCount === 0" @click="discard">放弃更改</button>
        <button class="btn primary" :disabled="changedCount === 0" @click="save">保存</button>
      </div>
    </div>
  </div>
</template>
