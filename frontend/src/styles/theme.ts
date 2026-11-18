/*
 * 主题应用层：把设置中的 uiTheme（dark / light / auto）落到 <html data-theme>，
 * 并对外暴露当前解析结果，供终端等无法走 CSS 变量的场景消费。
 * 变量定义见 ./tokens.css。
 */
import { ref } from 'vue'

export type ResolvedTheme = 'dark' | 'light'

/** 当前实际生效的主题（auto 已按系统外观解析） */
export const resolvedTheme = ref<ResolvedTheme>('dark')

/** 当前强调色（applyAccent 写入，供 xterm 等非 CSS 消费方跟随） */
export const accentColor = ref('#3ddc84')

const darkMedia = window.matchMedia('(prefers-color-scheme: dark)')

/** 用户偏好；auto 时随系统外观变化重新解析 */
let preference = 'dark'

function resolve(pref: string): ResolvedTheme {
  if (pref === 'light') return 'light'
  if (pref === 'auto') return darkMedia.matches ? 'dark' : 'light'
  return 'dark'
}

function setTheme(theme: ResolvedTheme) {
  resolvedTheme.value = theme
  document.documentElement.dataset.theme = theme
}

/** 应用主题偏好；非法值回退深色 */
export function applyTheme(pref: string) {
  preference = pref === 'light' || pref === 'auto' ? pref : 'dark'
  setTheme(resolve(preference))
}

darkMedia.addEventListener('change', () => {
  if (preference === 'auto') setTheme(resolve('auto'))
})

/* ---- 强调色：品牌绿 + 浅色主题加深变体 ---- */

interface Hsl {
  h: number
  s: number
  l: number
}

function hexToHsl(hex: string): Hsl | null {
  const m = /^#?([0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/.exec(hex.trim())
  if (!m) return null
  let h = m[1]
  if (h.length === 3) h = h.split('').map(c => c + c).join('')
  const n = parseInt(h, 16)
  const r = ((n >> 16) & 255) / 255
  const g = ((n >> 8) & 255) / 255
  const b = (n & 255) / 255
  const max = Math.max(r, g, b)
  const min = Math.min(r, g, b)
  const l = (max + min) / 2
  if (max === min) return { h: 0, s: 0, l: l * 100 }
  const d = max - min
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min)
  let hue: number
  if (max === r) hue = ((g - b) / d + (g < b ? 6 : 0)) / 6
  else if (max === g) hue = ((b - r) / d + 2) / 6
  else hue = ((r - g) / d + 4) / 6
  return { h: hue * 360, s: s * 100, l: l * 100 }
}

function hslToHex({ h, s, l }: Hsl): string {
  const sn = s / 100
  const ln = l / 100
  const c = (1 - Math.abs(2 * ln - 1)) * sn
  const x = c * (1 - Math.abs(((h / 60) % 2) - 1))
  const m = ln - c / 2
  const [r1, g1, b1] =
    h < 60 ? [c, x, 0]
    : h < 120 ? [x, c, 0]
    : h < 180 ? [0, c, x]
    : h < 240 ? [0, x, c]
    : h < 300 ? [x, 0, c]
    : [c, 0, x]
  const to = (v: number) => Math.round((v + m) * 255).toString(16).padStart(2, '0')
  return `#${to(r1)}${to(g1)}${to(b1)}`
}

function rgbTriplet(hex: string): string | null {
  const m = /^#?([0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/.exec(hex.trim())
  if (!m) return null
  let h = m[1]
  if (h.length === 3) h = h.split('').map(c => c + c).join('')
  const n = parseInt(h, 16)
  return `${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}`
}

/**
 * 应用强调色到全局。
 * 同时写入浅色主题需要的加深变体与半透明派生用的 rgb 三元组，
 * 避免在运行时依赖 color-mix（旧版 WKWebView 不支持）。
 */
export function applyAccent(color: string) {
  accentColor.value = color
  const root = document.documentElement.style
  root.setProperty('--accent-raw', color)
  const rawRgb = rgbTriplet(color)
  if (rawRgb) root.setProperty('--accent-rgb-raw', rawRgb)

  const hsl = hexToHsl(color)
  if (!hsl) return
  const strong = hslToHex({ ...hsl, l: Math.max(18, hsl.l * 0.42) })
  const strongRgb = rgbTriplet(strong)
  root.setProperty('--accent-strong', strong)
  if (strongRgb) root.setProperty('--accent-rgb-strong', strongRgb)
  root.setProperty('--accent-hover-raw', hslToHex({ ...hsl, l: Math.min(92, hsl.l + 8) }))
  root.setProperty('--accent-hover-strong', hslToHex({ ...hsl, l: Math.max(14, hsl.l * 0.34) }))
}

/* ---- xterm 配色（跟随明暗主题） ---- */

export interface TerminalPalette {
  background: string
  foreground: string
  cursor: string
  cursorAccent: string
  selectionBackground: string
  black: string
  red: string
  green: string
  yellow: string
  blue: string
  magenta: string
  cyan: string
  white: string
  brightBlack: string
  brightRed: string
  brightGreen: string
  brightYellow: string
  brightBlue: string
  brightMagenta: string
  brightCyan: string
  brightWhite: string
}

const DARK_PALETTE: TerminalPalette = {
  // 与 --bg-app 一致：xterm 需要具体色值，透明值会被其 viewport 默认黑底覆盖
  background: '#191a1c',
  foreground: '#d1d3d9',
  cursor: '#3ddc84',
  cursorAccent: '#191a1c',
  selectionBackground: 'rgba(61, 220, 132, .26)',
  black: '#484f58',
  red: '#f57e84',
  green: '#6db083',
  yellow: '#d59637',
  blue: '#71a1fe',
  magenta: '#bb9af7',
  cyan: '#4fd6e0',
  white: '#b1bac4',
  brightBlack: '#73767c',
  brightRed: '#ffa198',
  brightGreen: '#56d364',
  brightYellow: '#e3b341',
  brightBlue: '#92b7ff',
  brightMagenta: '#d2a8ff',
  brightCyan: '#56d4dd',
  brightWhite: '#dfe1e5',
}

const LIGHT_PALETTE: TerminalPalette = {
  background: '#ffffff',
  foreground: '#1f2328',
  cursor: '#0f8a4a',
  cursorAccent: '#ffffff',
  selectionBackground: 'rgba(15, 138, 74, .22)',
  black: '#24292f',
  red: '#c54e58',
  green: '#338555',
  yellow: '#a56906',
  blue: '#2f5eb9',
  magenta: '#8250df',
  cyan: '#1b7c83',
  white: '#6e7781',
  brightBlack: '#8c959f',
  brightRed: '#a40e26',
  brightGreen: '#1a7f37',
  brightYellow: '#b35900',
  brightBlue: '#218bff',
  brightMagenta: '#a475f9',
  brightCyan: '#3192aa',
  brightWhite: '#57606a',
}

export function terminalPalette(): TerminalPalette {
  const light = resolvedTheme.value === 'light'
  const base = light ? LIGHT_PALETTE : DARK_PALETTE
  // 光标与选中态跟随强调色；浅色主题下按与 --accent-strong 相同的规则加深，
  // 保证浅底对比度。ANSI 16 色保持语义色不跟随（避免 green 变成任意品牌色）。
  const hsl = hexToHsl(accentColor.value)
  const accent = light && hsl
    ? hslToHex({ ...hsl, l: Math.max(18, hsl.l * 0.42) })
    : accentColor.value
  const rgb = rgbTriplet(accent) ?? (light ? '15, 138, 74' : '61, 220, 132')
  return {
    ...base,
    cursor: accent,
    selectionBackground: `rgba(${rgb}, ${light ? '.22' : '.26'})`,
  }
}
