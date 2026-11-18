/**
 * 命令片段的调用通道（执行）。
 *
 * 落点优先级：当前激活且在线会话的 PTY → 剪贴板降级。
 * 含变量的片段先弹填充面板，确认后走同一落点。
 */
import { ref } from 'vue'
import { toast } from '../composables/useToast'
import { copyToClipboard, effectiveVars, type SnippetVar } from '../lib/snippetVars'
import { activeSessionId, sendInput, sessions } from './session'

export interface SnippetTarget {
  name: string
  command: string
  /** 变量声明；缺省时按命令自动识别 */
  vars?: SnippetVar[]
}

/** 进入填充面板时变量的声明已确定 */
export type ResolvedTarget = SnippetTarget & { vars: SnippetVar[] }

/** 非空表示变量填充面板已打开 */
export const snippetRun = ref<ResolvedTarget | null>(null)

export function openSnippetRun(target: ResolvedTarget) {
  snippetRun.value = target
}

export function closeSnippetRun() {
  snippetRun.value = null
}

/** 写入当前激活会话；不可用时返回 false 由调用方降级 */
function writeToActiveTerminal(command: string): boolean {
  const id = activeSessionId.value
  if (!id) return false
  const s = sessions.value.find(x => x.id === id)
  if (!s || !s.backendId || s.state !== 'online') return false
  const multiline = command.includes('\n')
  // 执行多行片段时包成复合命令组：shell 把它当作一个语法单元整体解析，
  // 中间不再逐条打印提示符。组在当前 shell 内执行，cd、变量赋值等状态照常生效。
  const text = multiline
    ? `{\n${command.replace(/\r\n?/g, '\n').replace(/\s+$/, '')}\n}`
    : command
  // 多行裸写时中间的 \n 会被行规程当作回车逐行提交，故用 bracketed paste 整体提交，
  // 避免前一行未结束就把后续行送进它的输入、或续行符被提前拼接
  const payload = multiline ? `\x1b[200~${text}\x1b[201~` : text
  void sendInput(id, `${payload}\r`)
  return true
}

/** 命令落点：优先写入终端，无可用会话时复制到剪贴板并提示 */
export async function applySnippetCommand(command: string): Promise<void> {
  if (writeToActiveTerminal(command)) return
  const ok = await copyToClipboard(command)
  toast(
    ok ? '未连接会话，命令已复制到剪贴板' : '复制失败，请检查剪贴板权限',
    ok ? 'info' : 'err',
    2200,
  )
}

/** 调用片段：有变量先填值，无变量直接落点 */
export function triggerSnippetAction(target: SnippetTarget) {
  const resolved: ResolvedTarget = { ...target, vars: effectiveVars(target) }
  if (resolved.vars.length) {
    openSnippetRun(resolved)
    return
  }
  void applySnippetCommand(resolved.command)
}
