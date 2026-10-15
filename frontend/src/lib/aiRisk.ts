/**
 * AI 终端命令高危判定原语（ai-terminal-exec-design.md §6.1.4）。
 *
 * 两层规则：
 * - 内置正则规则集：复杂高危模式（flag 组合、管道执行远程脚本等），随版本演进、
 *   不可编辑——它是安全底线，不提供关闭开关（嫌误报的用户可直接切 never 档）；
 * - 自定义关键词：用户补充自己关心的命令片段，纯子串匹配（大小写敏感，与 shell
 *   语义一致：bash 不执行 RM）。刻意不做正则——降低使用门槛，复杂模式交给内置层。
 *
 * 定位：把确认精力集中在大概率危险的操作上，不是安全边界；漏报由兜底层接住
 * （终端可见 + 多行整体提交缓冲 + Ctrl+C 可中断）。
 * 两条路径共享本判定：代码块直接执行（全局设置）与未来 agent 执行（M2.3 会话级权限）。
 */

export interface RiskHit {
  /** 命中来源：内置规则中文名 / 自定义关键词原文（供确认弹窗警示展示） */
  rule: string
}

/**
 * 内置高危规则：`command` 类按「命令词」精确匹配（避免引号内字符串误报），
 * `pattern` 类按整行正则匹配（表达参数组合、管道、重定向等语义）
 */
type RiskRule = { name: string; command: string } | { name: string; pattern: RegExp }

/** 前缀词：真正的命令词在其之后 */
const COMMAND_PREFIXES = new Set([
  'sudo', 'doas', 'env', 'nohup', 'time', 'nice', 'ionice', 'setsid', 'command', 'builtin', 'exec', 'stdbuf',
])

/** 剥离引号内文本（保留引号本身与结构字符），使字符串字面量不参与命令判定 */
function stripQuoted(line: string): string {
  let out = ''
  let quote: string | null = null
  for (let i = 0; i < line.length; i++) {
    const ch = line[i]
    if (quote) {
      if (ch === '\\' && quote === '"' && i + 1 < line.length) { out += '  '; i++; continue }
      if (ch === quote) { quote = null; out += ch; continue }
      out += ' '
      continue
    }
    if (ch === '"' || ch === "'") quote = ch
    out += ch
  }
  return out
}

/** 逐命令段取命令词：按 ; & | 分段，跳过变量赋值、选项与前缀词 */
function commandWords(line: string): string[] {
  return stripQuoted(line)
    .split(/[;&|]+/)
    .map(seg =>
      seg
        .trim()
        .split(/\s+/)
        .find(t => !!t && !/^[A-Za-z_][A-Za-z0-9_]*=/.test(t) && !t.startsWith('-') && !COMMAND_PREFIXES.has(t)),
    )
    .filter((t): t is string => !!t)
}

/** 内置高危规则集（随版本演进维护） */
export const BUILTIN_RISK_RULES: RiskRule[] = [
  { name: '删除文件', command: 'rm' },
  { name: '磁盘格式化/写入', pattern: /\bmkfs\.|\bdd\b[^\n]*\bof=/ },
  { name: '系统关机/重启', pattern: /\b(?:shutdown|reboot|poweroff|halt|init\s+[06])\b/ },
  { name: '递归放权', pattern: /chmod\s+-R\s+777/ },
  { name: '清空系统文件', pattern: />\s*\/etc\/|>\s*\/dev\/(?:sd|nvme|vd)/ },
  { name: '管道执行远程脚本', pattern: /(?:curl|wget)[^\n|]*\|\s*(?:ba|z)?sh\b/ },
  { name: '强制覆盖远端历史', pattern: /git\s+push\s+[^\n]*--force/ },
]

/** 判定单条命令（多行命令时逐行匹配，任一行命中即整段高危） */
export function matchHighRisk(code: string, customKeywords: string[]): RiskHit | null {
  for (const line of code.split('\n')) {
    const words = commandWords(line)
    for (const r of BUILTIN_RISK_RULES) {
      if ('command' in r) {
        if (words.includes(r.command)) return { rule: r.name }
      } else if (r.pattern.test(line)) {
        return { rule: r.name }
      }
    }
    for (const raw of customKeywords) {
      const kw = raw.trim()
      // 空行与 # 注释行跳过
      if (!kw || kw.startsWith('#')) continue
      if (line.includes(kw)) return { rule: kw }
    }
  }
  return null
}
