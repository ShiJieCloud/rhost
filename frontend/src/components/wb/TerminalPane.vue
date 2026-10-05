<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { WebLinksAddon } from '@xterm/addon-web-links'
import { openUrl } from '@tauri-apps/plugin-opener'
import { readText, writeText } from '@tauri-apps/plugin-clipboard-manager'
import '@xterm/xterm/css/xterm.css'
import AppLogo from '../AppLogo.vue'
import ContextMenu, { type MenuItem } from './ContextMenu.vue'
import { toast } from '../../composables/useToast'
import { hosts, showNewConn } from '../../stores/hosts'
import { savedSettings } from '../../stores/settings'
import {
  activeSessionId, attachTerminal, detachTerminal, openSession, reconnectBackend, reconnectTick, resizeTerminal,
  sendInput, sessions,
} from '../../stores/session'
import type { Session, SessionState } from '../../stores/session'
import type { HostStatus } from '../../types'

/* ============================================================
 * xterm.js 真实终端：每会话一个 Terminal 实例
 * 数据流：后端 Channel 帧 → attachTerminal 注册的 sink → term.write
 *         term.onData（键盘原始字节）→ sendInput → write_terminal
 * ============================================================ */

interface TermInst {
  term: Terminal
  fit: FitAddon
  disposables: Array<{ dispose(): void }>
}

/** 会话 id → xterm 实例（非响应式，避免深度代理开销） */
const termInsts = new Map<string, TermInst>()
/** 会话 id → 挂载容器元素 */
const termEls = new Map<string, HTMLElement>()

const wrapEl = ref<HTMLElement | null>(null)
let resizeObs: ResizeObserver | null = null

/** 终端可见高度下限（px）：低于此值 fit 会算出非法 rows（行高约 17px + 8px 呼吸边） */
const MIN_FIT_HEIGHT = 30
/** 悬浮工具条所需最小高度（top 12 + 工具条 32 = 44）：低于此值时工具条若照常
 *  绝对定位，会溢出 0 高容器落在 Dock 头部上，拦截 Dock 的关闭/收起按钮 */
const TERM_TOOLS_MIN_HEIGHT = 44
/** 广播条所需最小高度（bottom 12 + bar 约 46 = 58）：同上，空间不足时溢出
 *  会横跨在 Dock 头部上 */
const BCAST_MIN_HEIGHT = 58
/** 工具条与广播条垂直共存所需高度：两者占位之和；低于此值且广播开启时
 *  垂直区间交叠（广播条会盖住工具条），工具条应让位给广播模式 */
const FLOAT_COEXIST_MIN_HEIGHT = TERM_TOOLS_MIN_HEIGHT + BCAST_MIN_HEIGHT
/** 当前容器是否有足够空间显示悬浮工具条/广播条（Dock 上拖覆盖终端时为 false） */
const termSpaceEnough = ref(true)
const bcastSpaceEnough = ref(true)
/** 当前容器高度是否够工具条与广播条垂直共存 */
const floatCoexistEnough = ref(true)

/** 安全 fit：Dock 上拖覆盖终端时容器高度不足一行，跳过以避免 xterm resize 到
 *  rows<=0；行列保持原值，拉回可见高度后由 ResizeObserver 补 fit 并经 onResize 同步远端 */
function safeFit(id: string) {
  if (wrapEl.value && wrapEl.value.clientHeight < MIN_FIT_HEIGHT) return
  termInsts.get(id)?.fit.fit()
}

/* ---- 粘贴保护（pasteGuard）----
 * 终端粘贴的换行 = 立即回车执行，剪贴板可被网页"投毒"（显示与实际内容不符、
 * 夹带伪造的 bracketed-paste 结束标记 \x1b[201~ 提前逃逸、藏 ESC 控制序列）。
 * 拦截策略见 mountTerminal 内 onPaste；此处是检测规则与确认弹窗状态。 */

/** 待确认的粘贴内容；pasteGuardSession 为触发拦截的会话 id */
const pasteGuardOpen = ref(false)
const pasteGuardText = ref('')
let pasteGuardSession = ''

/** 危险控制字符：C0 中除 \t \n \r 外全部 + DEL（\x1b 转义序列、\x03 中断、\x7f 等） */
// eslint-disable-next-line no-control-regex
const PASTE_CTRL_RE = /[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/

/** 是否需要拦截确认：多行（\n 与 \r 都按"回车=立即执行"对待）或含危险控制字符。
 *  弹窗副标题为统一文案，不区分具体原因——内容本身会在预览区完整展示。 */
function pasteNeedsGuard(text: string): boolean {
  return /\r\n|\r|\n/.test(text) || PASTE_CTRL_RE.test(text)
}

/** 预览文本：控制字符转 U+2400 控制图片（␛␃…）便于肉眼识别，\t \n 保持原样 */
function visibleCtrl(s: string): string {
  // eslint-disable-next-line no-control-regex
  return s.replace(/[\x00-\x1f\x7f]/g, m => {
    const c = m.charCodeAt(0)
    if (c === 0x09 || c === 0x0a) return m
    return String.fromCharCode(0x2400 + c)
  })
}

/** 弹窗预览：最多 8 行，超出折叠 */
const PASTE_PREVIEW_LINES = 8
const pastePreview = computed(() => {
  const lines = visibleCtrl(pasteGuardText.value).split('\n')
  return lines.length <= PASTE_PREVIEW_LINES
    ? lines
    : [...lines.slice(0, PASTE_PREVIEW_LINES), `… 共 ${lines.length} 行`]
})

/** 确认粘贴：经 term.paste() 发送——与浏览器 paste 事件走同一 xterm 内部
 *  路径（\n→\r 归一化 + bracketed paste 包裹），远端 shell 体验与直接粘贴一致 */
function confirmPasteGuard() {
  pasteGuardOpen.value = false
  const text = pasteGuardText.value
  const inst = termInsts.get(pasteGuardSession)
  pasteGuardText.value = ''
  if (!text || !inst) return
  inst.term.paste(text)
  inst.term.focus()
}

/** 取消：丢弃暂存内容，不向远端发送任何字节 */
function cancelPasteGuard() {
  pasteGuardOpen.value = false
  pasteGuardText.value = ''
}

/** 弹窗打开期间接管 Esc（取消）/ Enter（确认），capture + stopPropagation
 *  防止按键穿透到底下终端被远端 shell 收到 */
function onPasteGuardKey(e: KeyboardEvent) {
  if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); cancelPasteGuard() }
  else if (e.key === 'Enter') { e.preventDefault(); e.stopPropagation(); confirmPasteGuard() }
}
watch(pasteGuardOpen, open => {
  if (open) window.addEventListener('keydown', onPasteGuardKey, true)
  else window.removeEventListener('keydown', onPasteGuardKey, true)
})

/** 统一粘贴入口（快捷键事件 / 右键 paste 模式 / 菜单"粘贴"项共用）：
 *  开启保护且内容有风险时转确认弹窗，否则直接经 term.paste() 发送 */
function pasteWithGuard(id: string, text: string) {
  if (!text) return
  if (savedSettings.pasteGuard && pasteNeedsGuard(text)) {
    pasteGuardText.value = text
    pasteGuardSession = id
    pasteGuardOpen.value = true
    return
  }
  termInsts.get(id)?.term.paste(text)
}

/** 取选区文本（按设置修剪行尾空白，与 copy 事件路径同规则） */
function selectionText(term: Terminal): string {
  const raw = term.getSelection()
  return savedSettings.trimOnCopy ? raw.replace(/[ \t]+$/gm, '') : raw
}

/* ---- 终端右键菜单（复用 SFTP 的 ContextMenu 组件）---- */

const termMenuOpen = ref(false)
const termMenuPos = ref({ x: 0, y: 0 })
const termMenuItems = ref<MenuItem[]>([])

const ICON_COPY = 'M9 9h10a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H11a2 2 0 0 1-2-2V9zM5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1'
const ICON_PASTE = 'M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2M9 2h6a1 1 0 0 1 1 1v2a1 1 0 0 1-1 1H9a1 1 0 0 1-1-1V3a1 1 0 0 1 1-1z'
const ICON_CLEAR = 'M18 6L6 18M6 6l12 12'
const ICON_SELECT = 'M8 3H5a2 2 0 0 0-2 2v3m18 0V5a2 2 0 0 0-2-2h-3m0 18h3a2 2 0 0 0 2-2v-3M3 16v3a2 2 0 0 0 2 2h3'

function copySelection(id: string) {
  const inst = termInsts.get(id)
  const text = inst ? selectionText(inst.term) : ''
  if (!text) return
  writeText(text).catch(err => {
    console.error('[clipboard] writeText failed', err)
    toast('复制失败：剪贴板不可用', 'warn')
  })
}

function pasteFromClipboard(id: string) {
  readText().then(text => {
    if (text) pasteWithGuard(id, text)
  }).catch(err => {
    console.error('[clipboard] readText failed', err)
    toast('无法读取剪贴板', 'warn')
  })
}

/** 打开终端右键菜单（每次重建动作项，复制项随选区有无禁用） */
function openTermMenu(id: string, x: number, y: number) {
  const inst = termInsts.get(id)
  if (!inst) return
  termMenuItems.value = [
    { label: '复制', icon: ICON_COPY, shortcut: '⌘C', disabled: !inst.term.hasSelection(), action: () => copySelection(id) },
    { label: '粘贴', icon: ICON_PASTE, shortcut: '⌘V', action: () => pasteFromClipboard(id) },
    { divider: true },
    { label: '清屏', icon: ICON_CLEAR, action: () => inst.term.clear() },
    { label: '全选', icon: ICON_SELECT, shortcut: '⌘A', action: () => inst.term.selectAll() },
  ]
  termMenuPos.value = { x, y }
  termMenuOpen.value = true
}

/** xterm 主题：与项目配色（--status-ok 等设计令牌）对齐 */
const XTERM_THEME = {
  background: '#00000000', // 透明，透出 .terminal 的背景渐变
  foreground: '#e6edf3',
  cursor: '#3fb950',
  cursorAccent: '#0d1117',
  selectionBackground: 'rgba(88, 166, 255, .28)',
  black: '#484f58',
  red: '#ff7b72',
  green: '#3fb950',
  yellow: '#f0883e',
  blue: '#58a6ff',
  magenta: '#bc8cff',
  cyan: '#39c5cf',
  white: '#b1bac4',
  brightBlack: '#6e7681',
  brightRed: '#ffa198',
  brightGreen: '#56d364',
  brightYellow: '#e3b341',
  brightBlue: '#79c0ff',
  brightMagenta: '#d2a8ff',
  brightCyan: '#56d4dd',
  brightWhite: '#f0f6fc',
}

/** v-for 容器 ref 收集 */
function setTermEl(id: string, el: Element | null) {
  if (el instanceof HTMLElement) {
    termEls.set(id, el)
    // 模板容器就绪即补挂载：覆盖 watch(activeSessionId) 之外的时机
    // （恢复会话 / 首次进入工作台时 watch 已错过赋值，容器渲染才是可靠锚点）
    if (id === activeSessionId.value) mountTerminal(id)
  } else {
    termEls.delete(id)
  }
}

/** 为会话创建 xterm 实例并发起后端连接（幂等） */
function mountTerminal(id: string) {
  if (termInsts.has(id)) return
  const el = termEls.get(id)
  if (!el) return

  const term = new Terminal({
    cursorBlink: true,
    cursorStyle: 'bar',
    fontSize: 12.5,
    fontFamily: "'JetBrains Mono', 'SF Mono', Menlo, Consolas, monospace",
    lineHeight: 1.4,
    // 回滚缓冲行数读设置（xterm 构造参数，仅对新建终端生效）；clamp 非负兜底
    scrollback: Math.max(0, Math.trunc(savedSettings.scrollback) || 0),
    allowTransparency: true,
    theme: XTERM_THEME,
  })
  const fit = new FitAddon()
  term.loadAddon(fit)
  // 自动识别终端文本里的 http(s) 链接（hover 下划线，点击激活）。
  // handler 必须走 Tauri opener 调系统浏览器——插件默认的 window.open
  // 在 Tauri WebView 内不可靠；插件正则仅匹配 http(s) 且经 URL 校验。
  term.loadAddon(new WebLinksAddon((_ev, uri) => {
    openUrl(uri).catch((err: unknown) => {
      console.error('[opener] openUrl failed for', uri, err)
      toast(`无法打开链接：${err instanceof Error ? err.message : String(err)}`, 'warn')
    })
  }))
  term.open(el)
  // open 后根元素必然存在，提前取出供监听注册/清理使用
  const screenEl: HTMLElement = term.element!

  // 键盘原始字节 → 后端 PTY（先绑定再 fit，fit 触发的尺寸事件也能被捕获）
  term.onData(data => sendInput(id, data))
  // xterm 尺寸变化（fit 计算出新行列）→ window-change 同步远端 PTY
  // 订阅挂在 term 上，term.dispose() 时自动回收，无监听泄漏
  term.onResize(({ cols, rows }) => resizeTerminal(id, cols, rows))

  // 复制时去除行尾空格：在 capture 阶段接管 copy 事件，先于 xterm 内部 bubble
  // 阶段的 copy handler（它写入未修剪的选区文本）。快捷键 Ctrl/⌘+C 与右键原生
  // 菜单的“复制”都会触发本事件；无选区或设置关闭时放行，不影响无选区 Ctrl+C
  // 向远端发送 ETX 中断信号。
  const onCopy = (ev: ClipboardEvent): void => {
    if (!savedSettings.trimOnCopy || !term.hasSelection()) return
    const trimmed = term.getSelection().replace(/[ \t]+$/gm, '')
    ev.stopPropagation()
    ev.preventDefault()
    ev.clipboardData?.setData('text/plain', trimmed)
  }
  screenEl.addEventListener('copy', onCopy, true)

  // 粘贴保护：capture 阶段先于 xterm 内部 paste handler（bubble 阶段）检查剪贴板
  // 原文，发现多行（回车会被 shell 立即执行）或危险控制字符（可伪造 bracketed
  // paste 结束标记、篡改终端状态）时弹窗确认；确认后经 term.paste() 走 xterm
  // 正常 bracketed-paste 流程发送，取消则不向远端发送任何字节。
  const onPaste = (ev: ClipboardEvent): void => {
    const text = ev.clipboardData?.getData('text/plain')
    if (!text) return
    ev.preventDefault()
    ev.stopPropagation()
    // 统一入口：保护开启且有风险 → 确认弹窗；否则 term.paste() 直接发送
    //（与 xterm 内部 paste handler 同一内部路径，bracketed-paste 语义不变）
    pasteWithGuard(id, text)
  }
  screenEl.addEventListener('paste', onPaste, true)

  // 右键行为（设置 rightClick）：menu 模式弹自定义菜单（阻止原生菜单）；
  // paste 模式为 PuTTY/X11 惯例——右键直接粘贴剪贴板内容
  const onCtxMenu = (ev: MouseEvent): void => {
    ev.preventDefault()
    ev.stopPropagation()
    if (savedSettings.rightClick !== 'menu') {
      pasteFromClipboard(id)
      return
    }
    openTermMenu(id, ev.clientX, ev.clientY)
  }
  screenEl.addEventListener('contextmenu', onCtxMenu, true)

  termInsts.set(id, {
    term, fit,
    disposables: [
      { dispose: () => screenEl.removeEventListener('copy', onCopy, true) },
      { dispose: () => screenEl.removeEventListener('paste', onPaste, true) },
      { dispose: () => screenEl.removeEventListener('contextmenu', onCtxMenu, true) },
    ],
  })
  safeFit(id)

  // 注册数据回调（sink）并触发后端 SSH 连接；connect 时带上当前终端尺寸
  attachTerminal(id, bytes => term.write(bytes), term.cols, term.rows)
  term.focus()
}

/** 销毁已关闭会话的 xterm 实例（后端断开在 closeSession 中处理） */
watch(
  () => sessions.value.map(s => s.id),
  ids => {
    for (const [id, inst] of termInsts) {
      if (!ids.includes(id)) {
        // 先移除自定义监听（copy capture），再 dispose xterm
        for (const d of inst.disposables) d.dispose()
        inst.term.dispose()
        termInsts.delete(id)
        termEls.delete(id)
      }
    }
  },
)

watch(activeSessionId, async id => {
  if (!id) return
  await nextTick()
  mountTerminal(id)
  // 切回已存在的 Tab：容器从 display:none 恢复，需要重新计算尺寸并聚焦。
  // Dock 覆盖终端（容器高度不足一行）时 safeFit 自动跳过，拉回后补 fit
  safeFit(id)
  termInsts.get(id)?.term.focus()
})

watch(reconnectTick, () => {
  const id = activeSessionId.value
  if (!id) return
  const inst = termInsts.get(id)
  if (inst) inst.term.reset()
  const s = sessions.value.find(x => x.id === id)
  toast(`正在重新连接 ${s?.host.id ?? ''} …`, 'info')
  void reconnectBackend(id)
})

/* ---- 工具栏 ---- */
function activeTerm(): Terminal | null {
  const id = activeSessionId.value
  return id ? (termInsts.get(id)?.term ?? null) : null
}

function termScrollTop() {
  activeTerm()?.scrollToTop()
}

function termScrollBottom() {
  activeTerm()?.scrollToBottom()
}

function clearScreen() {
  activeTerm()?.clear()
}

/* ---- 广播命令（一次发送到多个会话） ---- */
const bcastOpen = ref(false)
const bcastCmd = ref('')
const bcastPopOpen = ref(false)
const selectedIds = ref<string[]>([])
const bcastInputEl = ref<HTMLInputElement | null>(null)
const bcastHistory: string[] = []
let bcastHistIdx = 0

/** 可作为广播目标的在线会话 */
const onlineTargets = computed(() => sessions.value.filter(s => s.state === 'online'))

/** 选中且仍然在线的会话（自动过滤已断开/已关闭的选择） */
const effectiveSelected = computed<Session[]>(() =>
  selectedIds.value
    .map(id => sessions.value.find(s => s.id === id))
    .filter((s): s is Session => !!s && s.state === 'online'),
)

const targetLabel = computed(() => {
  const total = onlineTargets.value.length
  const n = effectiveSelected.value.length
  if (!total) return '无在线会话'
  if (n === total) return `全部在线 · ${n}`
  if (n === 0) return '未选择会话'
  return `已选 ${n} / ${total} 台`
})

function toggleBroadcast() {
  if (!bcastOpen.value) {
    if (!onlineTargets.value.length) {
      toast('当前没有可广播的在线会话', 'warn')
      return
    }
    bcastOpen.value = true
    // 打开时若选择已失效（如上次的会话全部关闭），默认全选在线会话
    if (!effectiveSelected.value.length) selectedIds.value = onlineTargets.value.map(s => s.id)
    nextTick(() => bcastInputEl.value?.focus())
  } else {
    bcastOpen.value = false
    bcastPopOpen.value = false
  }
}

/* 广播条开/关都会改变 .term-instance 的 bottom（style.css 中 bcast-on 的避让高度），
   但终端可视区高度变化不会触发 wrapEl 的 ResizeObserver（包裹层尺寸没变），必须手动
   fit：xterm 重算行列 → onResize → window-change → 远端重绘提示符，
   保证 shell 提示符始终露在广播条上方而不被遮挡。 */
watch(bcastOpen, async () => {
  await nextTick()
  const id = activeSessionId.value
  if (id) safeFit(id)
})

function toggleTarget(id: string) {
  const i = selectedIds.value.indexOf(id)
  if (i === -1) selectedIds.value.push(id)
  else selectedIds.value.splice(i, 1)
}

function selectAllOnline() {
  selectedIds.value = onlineTargets.value.map(s => s.id)
}

/** 广播 = 把命令文本 + 回车写入每个目标会话的 PTY，远端 shell 自行执行 */
function broadcastSend() {
  const raw = bcastCmd.value.trim()
  if (!raw) return
  const targets = effectiveSelected.value
  if (!targets.length) {
    toast('请选择至少一个在线会话', 'warn')
    return
  }
  for (const s of targets) sendInput(s.id, raw + '\r')
  if (bcastHistory[0] !== raw) {
    bcastHistory.unshift(raw)
    if (bcastHistory.length > 30) bcastHistory.pop()
  }
  bcastHistIdx = bcastHistory.length
  bcastCmd.value = ''
  toast(`已向 ${targets.length} 个会话广播命令`, 'ok')
}

function onBcastKey(e: KeyboardEvent) {
  e.stopPropagation()
  if (e.key === 'Enter') {
    e.preventDefault()
    broadcastSend()
  } else if (e.key === 'ArrowUp') {
    e.preventDefault()
    if (bcastHistIdx > 0) {
      bcastHistIdx--
      bcastCmd.value = bcastHistory[bcastHistIdx]
    }
  } else if (e.key === 'ArrowDown') {
    e.preventDefault()
    if (bcastHistIdx < bcastHistory.length - 1) {
      bcastHistIdx++
      bcastCmd.value = bcastHistory[bcastHistIdx]
    } else {
      bcastHistIdx = bcastHistory.length
      bcastCmd.value = ''
    }
  } else if (e.key === 'Escape') {
    e.preventDefault()
    if (bcastPopOpen.value) bcastPopOpen.value = false
    else if (bcastCmd.value) bcastCmd.value = ''
    else bcastOpen.value = false
  }
}

function stateDotCls(state: SessionState) {
  return state === 'online' ? 'st-ok'
    : state === 'connecting' ? 'st-info'
    : state === 'reconnecting' ? 'st-warn'
    : state === 'idle' ? 'st-idle'
    : 'st-err'
}

function stateLabel(state: SessionState) {
  return state === 'online' ? '在线'
    : state === 'connecting' ? '连接中'
    : state === 'reconnecting' ? '重连中'
    : state === 'idle' ? '空闲'
    : '已断开'
}

/* ---- 全局快捷键（输入按键本身由 xterm 处理，这里只管命令类快捷键） ---- */
function onKey(e: KeyboardEvent) {
  // xterm 聚焦时 e.target 是其内部辅助 textarea，不视为表单输入，快捷键照常生效
  const t = e.target as HTMLElement | null
  const inForm =
    t &&
    (t.tagName === 'INPUT' ||
      t.tagName === 'SELECT' ||
      (t.tagName === 'TEXTAREA' && !t.classList.contains('xterm-helper-textarea')))
  if (inForm) return

  // ⌘/Ctrl+T：新建连接（全局）
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 't') {
    if (showNewConn.value) return
    e.preventDefault()
    showNewConn.value = true
    return
  }
  // ⌘/Ctrl+Shift+B：切换广播命令条
  if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === 'b') {
    e.preventDefault()
    toggleBroadcast()
  }
}

onMounted(() => {
  document.addEventListener('keydown', onKey)
  // 终端可视尺寸变化（Dock 拖拽/窗口缩放）时重新 fit，并按剩余空间
  // 决定悬浮工具条/广播条是否显示（0 高容器中绝对定位子元素会溢出拦截 Dock 按钮）
  resizeObs = new ResizeObserver(() => {
    const h = wrapEl.value?.clientHeight ?? 0
    termSpaceEnough.value = h >= TERM_TOOLS_MIN_HEIGHT
    bcastSpaceEnough.value = h >= BCAST_MIN_HEIGHT
    floatCoexistEnough.value = h >= FLOAT_COEXIST_MIN_HEIGHT
    const id = activeSessionId.value
    if (id) safeFit(id)
  })
  // 先按当前真实高度同步一次：覆盖态冷启动时避免首帧悬浮元素闪现
  const initH = wrapEl.value?.clientHeight ?? 0
  termSpaceEnough.value = initH >= TERM_TOOLS_MIN_HEIGHT
  bcastSpaceEnough.value = initH >= BCAST_MIN_HEIGHT
  floatCoexistEnough.value = initH >= FLOAT_COEXIST_MIN_HEIGHT
  if (wrapEl.value) resizeObs.observe(wrapEl.value)
})
onUnmounted(() => {
  document.removeEventListener('keydown', onKey)
  // 粘贴保护弹窗若开着，一并摘除其 Esc/Enter 接管监听
  window.removeEventListener('keydown', onPasteGuardKey, true)
  resizeObs?.disconnect()
  // 组件重建（HMR 等）时销毁全部 xterm 并注销 sink，
  // 避免旧 sink 闭包持有 detached xterm、数据写入不可见实例
  for (const [id, inst] of termInsts) {
    inst.disposables.forEach(d => d.dispose())
    inst.term.dispose()
    detachTerminal(id)
  }
  termInsts.clear()
  termEls.clear()
})

/* ---- 空态（关闭所有 Tab 后）：最近连接取主机列表前 3 台，在线优先 ---- */
const hasSessions = computed(() => sessions.value.length > 0)

const recentHosts = computed(() =>
  [...hosts.value]
    .sort((a, b) => (a.status === 'online' ? 0 : 1) - (b.status === 'online' ? 0 : 1))
    .slice(0, 3),
)

function dotClass(status: HostStatus) {
  return status === 'online' ? 'online' : status === 'offline' ? 'offline' : 'warn'
}
</script>

<template>
  <div ref="wrapEl" class="terminal-wrap">
    <!-- 工具栏：广播开启且高度不足以垂直共存时让位隐藏（Esc 关广播后恢复） -->
    <div v-show="hasSessions && termSpaceEnough && (!bcastOpen || floatCoexistEnough)"
         class="term-tools" :class="{ pinned: bcastOpen }">
      <button title="滚动到顶部" @click="termScrollTop">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
          <line x1="12" y1="19" x2="12" y2="5"></line>
          <polyline points="5 12 12 5 19 12"></polyline>
        </svg>
      </button>
      <button title="滚动到底部" @click="termScrollBottom">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
          <line x1="12" y1="5" x2="12" y2="19"></line>
          <polyline points="19 12 12 19 5 12"></polyline>
        </svg>
      </button>
      <button title="清屏" @click="clearScreen">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M3 6h18M8 6V4a1 1 0 0 1 1-1h6a1 1 0 0 1 1 1v2"></path>
          <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"></path>
        </svg>
      </button>
      <button
        class="bcast-tool"
        :class="{ on: bcastOpen }"
        title="广播命令到多个会话 (⌘⇧B)"
        @click="toggleBroadcast"
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="5.5" cy="12" r="2.2"></circle>
          <circle cx="18.5" cy="6" r="2.2"></circle>
          <circle cx="18.5" cy="18" r="2.2"></circle>
          <path d="M7.6 10.9 16.4 7.1"></path>
          <path d="M7.6 13.1l8.8 3.8"></path>
        </svg>
      </button>
    </div>

    <!-- 终端：每会话一个 xterm 容器，v-show 保持实例存活 -->
    <div v-show="hasSessions" class="terminal" :class="{ 'bcast-on': bcastOpen }">
      <div
        v-for="s in sessions"
        :key="s.id"
        v-show="s.id === activeSessionId"
        :ref="el => setTermEl(s.id, el as Element | null)"
        class="term-instance"
      ></div>
    </div>

    <!-- 广播命令条：一次发送命令到多个会话（空间不足时随 Dock 拖拽自动隐藏） -->
    <div v-if="hasSessions && bcastOpen && bcastSpaceEnough" class="bcast">
      <div v-if="bcastPopOpen" class="bcast-scrim" @click="bcastPopOpen = false"></div>

      <div v-if="bcastPopOpen" class="bcast-pop">
        <div class="bp-head">
          目标会话
          <span>{{ effectiveSelected.length }} / {{ onlineTargets.length }}</span>
        </div>
        <div class="bp-list">
          <button
            v-for="s in sessions"
            :key="s.id"
            class="bp-item"
            :class="{ disabled: s.state !== 'online' }"
            :disabled="s.state !== 'online'"
            @click="toggleTarget(s.id)"
          >
            <span class="bp-check" :class="{ on: selectedIds.includes(s.id) }">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"></polyline></svg>
            </span>
            <span class="dot" :class="stateDotCls(s.state)"></span>
            <span class="bp-name">{{ s.host.id }}</span>
            <span class="bp-addr">{{ s.host.user }}@{{ s.host.ip }}</span>
            <span class="bp-state">{{ stateLabel(s.state) }}</span>
          </button>
          <div v-if="!sessions.length" class="bp-empty">暂无可选会话</div>
        </div>
        <div class="bp-foot">
          <button @click="selectAllOnline">全选在线</button>
          <button @click="selectedIds = []">清空</button>
        </div>
      </div>

      <div class="bcast-bar">
        <span class="bcast-ico" title="广播模式">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="5.5" cy="12" r="2.2"></circle>
            <circle cx="18.5" cy="6" r="2.2"></circle>
            <circle cx="18.5" cy="18" r="2.2"></circle>
            <path d="M7.6 10.9 16.4 7.1"></path>
            <path d="M7.6 13.1l8.8 3.8"></path>
          </svg>
        </span>

        <button class="bcast-targets" :class="{ none: effectiveSelected.length === 0 }" @click.stop="bcastPopOpen = !bcastPopOpen">
          <span>{{ targetLabel }}</span>
          <svg class="chev" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"></polyline></svg>
        </button>

        <input
          ref="bcastInputEl"
          v-model="bcastCmd"
          class="bcast-input"
          type="text"
          spellcheck="false"
          autocomplete="off"
          placeholder="输入命令，按 Enter 广播到所选会话（↑/↓ 切换历史，Esc 关闭）"
          @keydown="onBcastKey"
        >

        <button
          class="bcast-send"
          :disabled="!bcastCmd.trim() || effectiveSelected.length === 0"
          @click="broadcastSend"
        >
          发送
          <kbd>↵</kbd>
        </button>
      </div>
    </div>

    <!-- 空态（关闭所有 Tab 后） -->
    <div v-if="!hasSessions" class="tp-empty">
      <div class="icon-wrap">
        <div class="icon-box">
          <AppLogo />
        </div>
      </div>
      <h1>没有活动的会话</h1>
      <p>连接到远程主机，或打开一个本地终端开始工作。</p>

      <div class="actions">
        <button class="btn primary" @click="showNewConn = true">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round">
            <path d="M12 5v14M5 12h14"/>
          </svg>
          新建连接
        </button>
        <button class="btn" @click="showNewConn = true">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">
            <path d="M7 9.5L9.5 12L7 14.5"/><path d="M13 14.5h4"/>
            <rect x="2.5" y="4" width="19" height="16" rx="3"/>
          </svg>
          本地终端
        </button>
      </div>

      <div v-if="recentHosts.length" class="recent">
        <div class="recent-label">可用主机</div>
        <div class="recent-list">
          <button v-for="h in recentHosts" :key="h.id" class="recent-item" @click="openSession(h.id)">
            <span class="dot" :class="dotClass(h.status)"></span>
            <span class="rname">{{ h.id }}</span>
            <span class="raddr">{{ h.user }}@{{ h.ip }}</span>
            <span class="rgo">
              <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M9 6l6 6-6 6"/>
              </svg>
            </span>
          </button>
        </div>
      </div>

      <div class="hints">
        <span><kbd>⌘</kbd><kbd>T</kbd> 新建连接</span>
        <span><kbd>⌘</kbd><kbd>K</kbd> 命令面板</span>
      </div>
    </div>

    <!-- 终端右键菜单：复用 SFTP 上下文菜单组件（定位/边界夹取/键盘导航内置） -->
    <ContextMenu v-model:visible="termMenuOpen" :x="termMenuPos.x" :y="termMenuPos.y" :items="termMenuItems" />

    <!-- ============ 粘贴保护确认弹窗 ============ -->
    <div v-if="pasteGuardOpen" class="mask show" @click.self="cancelPasteGuard">
      <div class="modal tp-paste-modal" role="alertdialog" aria-modal="true">
        <div class="modal-head">
          <div class="modal-icon tp-paste-icon">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                 stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"></path>
              <path d="M9 12l2 2 4-4"></path>
            </svg>
          </div>
          <div class="modal-title">
            <h2>粘贴保护</h2>
            <p>粘贴前请确认，内容将发送至远端执行</p>
          </div>
        </div>

        <div class="tp-paste-body">
          <div class="tp-paste-label">内容预览（控制字符已转义显示）</div>
          <pre class="tp-paste-preview">{{ pastePreview.join('\n') }}</pre>
        </div>

        <div class="modal-foot">
          <span class="spacer"></span>
          <button type="button" class="btn ghost" @click="cancelPasteGuard">取消（Esc）</button>
          <button type="button" class="btn primary" @click="confirmPasteGuard">确认粘贴（↵）</button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 粘贴保护弹窗：复用全局 .mask/.modal/.btn，仅补本组件专属尺寸与预览样式。
   height:auto 覆盖全局 .modal 的固定 640px——本弹窗内容自适应，避免大片空白；
   head/body/foot 间距统一压缩，区域间不留大空隙 */
.tp-paste-modal{width:440px;max-width:calc(100vw - 48px);height:auto}
.tp-paste-modal .modal-head{padding:12px 18px}
.tp-paste-modal .modal-foot{padding:10px 18px}
.tp-paste-icon{color:#f0883e}
.tp-paste-body{padding:10px 18px 12px}
.tp-paste-label{font-size:12px;color:var(--text-dim,#8b949e);margin-bottom:6px}
.tp-paste-preview{
  margin:0;max-height:200px;overflow:auto;white-space:pre-wrap;word-break:break-all;
  background:rgba(110,118,129,.12);border:1px solid rgba(110,118,129,.25);
  border-radius:8px;padding:10px 12px;font-size:12px;line-height:1.55;
  font-family:'JetBrains Mono','SF Mono',Menlo,Consolas,monospace;color:#e6edf3;
}
</style>
