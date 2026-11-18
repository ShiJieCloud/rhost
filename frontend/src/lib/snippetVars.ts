import { writeText } from '@tauri-apps/plugin-clipboard-manager'
import { isTauri } from './tauri'

/**
 * 命令片段变量：解析、渲染与运行期辅助。
 *
 * 仅支持 `{{name}}` 一种占位符写法，允许两侧空格。变量名以字母或下划线
 * 开头，后接字母、数字、下划线，该规则排除了 Docker / Go 模板的
 * `{{.Names}}` 与 `{{range .Items}}`，避免把格式化模板误识别为片段变量。
 *
 * 声明列表（SnippetVar[]）是渲染的白名单：只有声明中存在的变量会被替换，
 * 其余占位符原样保留。用户可以在编辑器里删除误识别的变量来表达「不要替换」。
 */
const VAR_RE = /\{\{\s*([A-Za-z_][A-Za-z0-9_]*)\s*\}\}/g

const VAR_NAME_RE = /^[A-Za-z_][A-Za-z0-9_]*$/

/** 片段变量的声明与展示元数据 */
export interface SnippetVar {
  /** 占位符名，对应命令中的 `{{name}}`，创建后不可改 */
  name: string
  /** 填写面板中的显示名，默认同 name */
  label: string
  defaultValue: string
  /** 为 true 时未填写则禁止调用 */
  required: boolean
  kind: 'text' | 'select'
  /** kind === 'select' 时的候选值 */
  options: string[]
  /** 密码式输入，且不参与值记忆 */
  secret: boolean
}

export function defaultVar(name: string): SnippetVar {
  return {
    name,
    label: name,
    defaultValue: '',
    required: false,
    kind: 'text',
    options: [],
    secret: false,
  }
}

/** 占位符展示文本，如 `{{container}}` */
export function varToken(name: string): string {
  return `{{${name}}}`
}

/** 提取命令中出现的变量名，按首次出现顺序去重 */
export function extractVars(cmd: string): string[] {
  const seen = new Set<string>()
  for (const m of cmd.matchAll(VAR_RE)) {
    const key = m[1]
    if (key) seen.add(key)
  }
  return [...seen]
}

/** 按占位符首次出现顺序生成默认声明 */
export function varsFromCommand(cmd: string): SnippetVar[] {
  return extractVars(cmd).map(defaultVar)
}

/**
 * 规范化外部数据（持久化快照 / 草稿）中的变量声明。
 * 非数组返回 undefined，表示该片段从未声明过变量，运行期可自动识别兜底。
 */
export function normalizeVars(raw: unknown): SnippetVar[] | undefined {
  if (!Array.isArray(raw)) return undefined
  const out: SnippetVar[] = []
  const seen = new Set<string>()
  for (const item of raw) {
    if (!item || typeof item !== 'object') continue
    const o = item as Record<string, unknown>
    const name = typeof o.name === 'string' ? o.name : ''
    if (!VAR_NAME_RE.test(name) || seen.has(name)) continue
    seen.add(name)
    out.push({
      name,
      label: typeof o.label === 'string' && o.label ? o.label : name,
      defaultValue: typeof o.defaultValue === 'string' ? o.defaultValue : '',
      required: o.required === true,
      kind: o.kind === 'select' ? 'select' : 'text',
      options: Array.isArray(o.options)
        ? o.options.filter((x): x is string => typeof x === 'string')
        : [],
      secret: o.secret === true,
    })
  }
  return out
}

/**
 * 片段的生效声明：显式声明优先；从未声明过（老数据）才按命令自动识别。
 * 空数组表示用户已明确「没有变量」，不会自动补回。
 */
export function effectiveVars(s: { command: string; vars?: SnippetVar[] }): SnippetVar[] {
  return s.vars ?? varsFromCommand(s.command)
}

export interface CommandPart {
  text: string
  /** 命中变量时为其名称，用于高亮渲染 */
  var?: string
}

/** 按变量切分命令，供预览时高亮变量片段 */
export function splitCommand(cmd: string): CommandPart[] {
  const parts: CommandPart[] = []
  let last = 0
  for (const m of cmd.matchAll(VAR_RE)) {
    const start = m.index ?? 0
    if (start > last) parts.push({ text: cmd.slice(last, start) })
    parts.push({ text: m[0], var: m[1] })
    last = start + m[0].length
  }
  if (last < cmd.length) parts.push({ text: cmd.slice(last) })
  return parts
}

/**
 * 渲染最终命令。只替换声明中存在的变量；未声明或已删除的占位符、
 * 以及声明了但留空的变量，均原样保留。值按原样插入，需要引号时由用户自行填写。
 */
export function renderCommand(
  cmd: string,
  declared: readonly SnippetVar[],
  values: Record<string, string>,
): string {
  const byName = new Map(declared.map(v => [v.name, v]))
  return cmd.replace(VAR_RE, (raw, key: string) => {
    const def = byName.get(key)
    if (!def) return raw
    const value = values[key]
    if (!value || !value.trim()) return raw
    return value
  })
}

/* ==================== 剪贴板 ==================== */

/** 复制文本到剪贴板：Tauri 环境走插件，浏览器环境走 Web API */
export async function copyToClipboard(text: string): Promise<boolean> {
  try {
    if (isTauri) {
      await writeText(text)
    } else {
      await navigator.clipboard.writeText(text)
    }
    return true
  } catch (err) {
    console.error('[clipboard] writeText failed', err)
    return false
  }
}
