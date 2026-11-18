/**
 * 命令片段编辑器草稿通道。
 *
 * 各入口（片段面板新建/编辑、终端右键「保存为片段」）通过 openSnippetEditor
 * 打开 App 层居中模态框并预填内容。面板可能尚未挂载，展开左侧片段页的
 * 副作用必须在此完成。静态 UI 阶段：保存不接线，草稿仅用于展示。
 *
 * 模态框有编辑 / 预览两种模式：点击列表行进入预览（只读完整命令），
 * 新建或右键保存进入编辑。
 */
import { ref } from 'vue'
import { effectiveVars, varsFromCommand, type SnippetVar } from '../lib/snippetVars'
import { leftPanelTab, sidebarVisible } from './session'

export interface SnippetDraft {
  /** 已存在片段的主键；新建时为空 */
  id?: string
  name: string
  group: string
  command: string
  description: string
  /** 变量声明，随命令内容自动增删 */
  vars: SnippetVar[]
}

export type SnippetEditorMode = 'edit' | 'preview'

/** 当前编辑草稿；null 表示模态框关闭 */
export const snippetDraft = ref<SnippetDraft | null>(null)

/** 当前模态框模式 */
export const snippetMode = ref<SnippetEditorMode>('edit')

function revealPanel() {
  leftPanelTab.value = 'snippets'
  sidebarVisible.value = true
}

/** 打开片段编辑器：可预填部分字段（如终端选中文本 → command） */
export function openSnippetEditor(partial?: Partial<SnippetDraft>) {
  const command = partial?.command ?? ''
  snippetDraft.value = {
    name: '',
    group: '',
    command: '',
    description: '',
    ...partial,
    vars: partial?.vars ? partial.vars.map(v => ({ ...v, options: [...v.options] })) : varsFromCommand(command),
  }
  snippetMode.value = 'edit'
  revealPanel()
}

/** 打开片段预览：只读展示完整命令与变量 */
export function openSnippetPreview(s: Omit<SnippetDraft, 'vars'> & { vars?: SnippetVar[] }) {
  snippetDraft.value = {
    id: s.id,
    name: s.name,
    group: s.group,
    command: s.command,
    description: s.description,
    vars: effectiveVars(s),
  }
  snippetMode.value = 'preview'
  revealPanel()
}

/** 从预览切回编辑，沿用当前内容 */
export function editSnippetDraft() {
  snippetMode.value = 'edit'
}

export function closeSnippetEditor() {
  snippetDraft.value = null
  snippetMode.value = 'edit'
}
