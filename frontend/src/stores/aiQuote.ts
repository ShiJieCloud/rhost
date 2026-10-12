/**
 * 「添加到 AI 对话」引用通道。
 *
 * 终端（wb 层 TerminalPane）向 AI 面板（App 层）传递选中文本：
 * TerminalPane 右键菜单调用 addToAiChat()；AiChatPanel watch 此状态把内容
 * 填充为输入框上方的引用块。一次性事件语义：消费后立即置空。
 */
import { ref } from 'vue'
import { rightPanelTab, rightPanelVisible } from './session'

/** 待添加到 AI 对话的终端选中文本；null 表示无 */
export const aiQuote = ref<{ text: string } | null>(null)

/** 终端右键菜单入口：记录选中文本并切到 AI 面板（面板可能尚未挂载，副作用必须在此完成） */
export function addToAiChat(text: string) {
  const t = text.trim()
  if (!t) return
  aiQuote.value = { text: t }
  rightPanelTab.value = 'ai'
  rightPanelVisible.value = true
}
