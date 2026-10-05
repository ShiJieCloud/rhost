import { reactive, ref } from 'vue'

/** 设置弹窗显隐（模块级单例，同 showNewConn 模式） */
export const showSettings = ref(false)

export interface EnvVar {
  key: string
  value: string
}

export interface AppSettings {
  /* ---- 外观 ---- */
  uiTheme: string
  accent: string
  opacity: number
  /* ---- 字体 ---- */
  fontFamily: string
  fontSize: number
  /** 行高百分比（100~200），消费侧 /100 转倍数 */
  lineHeight: number
  /** 字重，持久化为字符串（'300'/'400'/'500'/'700'），消费侧 Number() 转换 */
  fontWeight: string
  /* ---- 终端 ---- */
  /** 登录后为无颜色的远端 shell 注入彩色提示符（PS1 由远端生成，透传不变） */
  colorPrompt: boolean
  /** 连接成功后采集服务器状态绘制 Rhost MOTD；开启同时抑制 sshd 原生 MOTD/Last login */
  motd: boolean
  /** 启用自定义 MOTD LOGO：开启用 motdLogo 文本替换内置 LOGO，关闭显示内置（内容保留） */
  motdLogoOn: boolean
  /** 自定义 MOTD ASCII LOGO（多行文本）；仅 motdLogoOn 开启时生效 */
  motdLogo: string
  scrollback: number
  trimOnCopy: boolean
  pasteGuard: boolean
  rightClick: string
  /** 终端光标样式：block 方块 / bar 竖线 / underline 下划线 */
  cursorStyle: string
  /** 终端光标空闲 1 秒后闪烁 */
  cursorBlink: boolean
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
  /* ---- 监控 ---- */
  /** 工具内存采集间隔（秒），驱动底部状态栏内存指标轮询 */
  memInterval: number
  /** 远端主机动态指标（CPU/内存/网络/磁盘/进程/GPU）采集间隔（秒），1~10，后端再夹取 */
  metricsInterval: number
  /** 工作台不可见（回首页/窗口最小化）时暂停内存采集 */
  memPauseHidden: boolean
  /** 内存告警阈值（MB）：主进程 RSS 持续超过该值时状态栏告警 */
  memAlertMb: number
  /* ---- SFTP 文件传输 ---- */
  /** 传输分块大小（KB）：上传/下载单次读写长度，越大吞吐越高、进度粒度越粗 */
  sftpChunkKb: number
  /** 断点续传：同名文件未传完时从已有偏移续传；关闭后一律全量重传 */
  sftpResume: boolean
  /** 断点校验方式：size 文件大小 / sizeMtime 大小+mtime / sha256 哈希（逻辑待实现） */
  sftpResumeCheck: string
  /** 文件覆盖策略：skip 跳过 / overwrite 直接覆盖 / newer 仅源文件更新时覆盖 / ask 每次询问（逻辑待实现） */
  sftpOverwritePolicy: string
  /** 上传临时文件机制：先写临时文件、完成后原子重命名（逻辑待实现） */
  sftpUploadTemp: boolean
  /** 保留文件元数据：传输时同步修改时间与权限属性（逻辑待实现） */
  sftpPreserveMeta: boolean
  /** 全局最大并发传输任务数（跨所有主机） */
  sftpGlobalConcurrency: number
  /** 单主机最大并发传输任务数（受单 SFTP 通道约束，建议保持 1） */
  sftpHostConcurrency: number
  /** 全局带宽限速 KB/s，0 不限速 */
  sftpGlobalRateKb: number
  /** 单任务带宽限速 KB/s，0 不限速 */
  sftpTaskRateKb: number
  /** 可恢复错误的自动重试最大次数 */
  sftpRetryCount: number
  /** 重试前等待毫秒 */
  sftpRetryIntervalMs: number
  /** SFTP 子通道空闲超时秒数，超时自动关闭（SSH 终端不受影响），0 不自动关闭 */
  sftpIdleTimeoutSec: number
  /** 传输完成后对整文件做 SHA256 完整性校验 */
  sftpVerifyHash: boolean
  /** 上传黑名单 glob，一行一条，命中则跳过不创建任务 */
  sftpBlacklist: string
  /** 文件树显示以 . 开头的隐藏文件（本地/远端两侧均生效） */
  sftpShowHidden: boolean
  /* ---- 关于 ---- */
  autoUpdate: boolean
  updateChannel: string
}

export const DEFAULT_SETTINGS: AppSettings = {
  uiTheme: 'dark',
  accent: '#3ddc84',
  opacity: 96,

  fontFamily: 'JetBrains Mono',
  fontSize: 13,
  lineHeight: 145,
  fontWeight: '400',

  colorPrompt: true,
  motd: true,
  motdLogoOn: false,
  motdLogo: '',
  scrollback: 10000,
  trimOnCopy: true,
  pasteGuard: false,
  rightClick: 'menu',
  cursorStyle: 'bar',
  cursorBlink: true,
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

  memInterval: 2,
  metricsInterval: 3,
  memPauseHidden: true,
  memAlertMb: 300,

  sftpChunkKb: 64,
  sftpResume: true,
  sftpResumeCheck: 'size',
  sftpOverwritePolicy: 'newer',
  sftpUploadTemp: true,
  sftpPreserveMeta: false,
  sftpGlobalConcurrency: 3,
  sftpHostConcurrency: 1,
  sftpGlobalRateKb: 0,
  sftpTaskRateKb: 0,
  sftpRetryCount: 3,
  sftpRetryIntervalMs: 1000,
  sftpIdleTimeoutSec: 300,
  sftpVerifyHash: false,
  sftpBlacklist: '.DS_Store\nThumbs.db',
  sftpShowHidden: false,

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

/** 等宽字体回退链：首选字体未安装时逐级回退，最终落到系统通用等宽 */
const MONO_FALLBACK = 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace'

/**
 * 设置中的字体名 → 可直接用于 CSS / xterm fontFamily 的字体栈。
 * 'monospace' 表示系统等宽（不前置具名字体）；其余值作为首选字体加引号，
 * 防止含空格/数字的字体名被解析成多个族名。设置面板预览与终端共用此函数。
 */
export function resolveTerminalFontFamily(name: string): string {
  return name === 'monospace' ? MONO_FALLBACK : `"${name}", ${MONO_FALLBACK}`
}
