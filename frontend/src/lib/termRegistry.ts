/**
 * xterm 实例模块级注册表。
 *
 * TerminalPane 是组件实例级 Map（每实例一份），而 AI 面板（App 层）需要读取
 * 终端缓冲组装会话上下文——跨组件访问经此注册表中转。TerminalPane 在 xterm
 * mount 时注册、销毁时注销；read 侧只依赖本模块，不依赖组件实例。
 */
import type { Terminal } from '@xterm/xterm'

const terms = new Map<string, Terminal>()

export function registerTerm(sessionId: string, term: Terminal) {
  terms.set(sessionId, term)
}

export function unregisterTerm(sessionId: string) {
  terms.delete(sessionId)
}

/** 默认采集行数：约一屏，覆盖报错现场又不至于撑爆上下文 */
const DEFAULT_LINES = 30
/** 采集字符上限：防止全屏 TUI 残留/超长单行刷爆请求体（约 2k token） */
const MAX_CHARS = 4000

/**
 * 读取指定会话终端缓冲的最近 N 行（xterm buffer 已是解析后文本，无 ANSI 码）。
 *
 * 当前 UI 无消费方——显式引用路径走 xterm getSelection；本函数为 M2.3 Agent
 * 工具（Observation 兜底读屏）预留。规则：
 * - alternate buffer（vim/htop 等全屏 TUI）画面无对话价值，返回空串；
 * - 每行 trimEnd，去掉顶部连续空行，总字符超限时从顶部继续丢弃；
 * - 终端输出可能含敏感数据（密钥/token），调用方仅将其发给用户自己配置的模型服务。
 */
export function getRecentOutput(sessionId: string, maxLines: number = DEFAULT_LINES): string {
  const term = terms.get(sessionId)
  const buf = term?.buffer.active
  if (!buf || buf.type === 'alternate') return ''

  const lines: string[] = []
  let chars = 0
  const end = buf.length
  for (let i = end - 1; i >= 0 && lines.length < maxLines; i--) {
    const text = (buf.getLine(i)?.translateToString(true) ?? '').trimEnd()
    if (chars + text.length > MAX_CHARS) break
    lines.unshift(text)
    chars += text.length + 1
  }
  // 丢弃顶部连续空行（滚动缓冲末尾常见），保留中间空行
  while (lines.length > 0 && lines[0] === '') lines.shift()
  return lines.join('\n')
}
