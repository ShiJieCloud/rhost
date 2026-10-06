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
  /** 启动动画最短展示时长（ms，200~5000，消费侧夹取）；呼吸动画不受影响，重启生效 */
  splashDurationMs: number
  /** 启动动画背景：true 透明悬浮桌面（需 macOSPrivateApi）/ false 深色底 + LOGO；重启生效 */
  splashTransparent: boolean
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
  /** 网络异常断开时自动重连（远端主动 exit、手动断开不触发；退避 1s 起指数增长，封顶 30s） */
  autoReconnect: boolean
  /** 自动重连最大尝试次数，0 表示不限次数 */
  autoReconnectMaxAttempts: number
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
  /* ---- 日志 ---- */
  /** 日志采集：关闭后不再采集和显示日志 */
  logCollect: boolean
  /** 日志级别：低于该级别的日志不会被采集（debug/info/warn/error） */
  logLevel: string
  /** 内存中保留的日志行数上限，超出后丢弃最旧的行 */
  logMaxLines: number
  /** 持久化到磁盘：关闭后日志仅存在于内存，退出即丢失 */
  logPersist: boolean
  /** 日志存储路径：会话日志文件的存放目录 */
  logStoragePath: string
  /** 日志文件名模板：${date} 为按切割策略格式化的日期，如 rhost_app_20260102.log */
  logNaming: string
  /** 日志切割策略：daily 按天 / weekly 按周 / monthly 按月 / none 不切割 */
  logRotate: string
  /** 保留的日志文件数上限，超出后删除最旧的文件；0 表示不限制 */
  logMaxFiles: number
  /** 日志文件保留天数，超过后按时间清理旧文件 */
  logRetentionDays: number
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
  splashDurationMs: 400,
  splashTransparent: true,

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
  autoReconnect: true,
  autoReconnectMaxAttempts: 5,
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

  logCollect: true,
  logLevel: 'info',
  logMaxLines: 5000,
  logPersist: true,
  logStoragePath: '',
  logNaming: 'rhost_app_${date}.log',
  logRotate: 'daily',
  logMaxFiles: 100,
  logRetentionDays: 30,
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

/** 强调色应用到全局（view-card 等处 var(--accent) 生效）。
 *  同时注入 --accent-rgb（"r,g,b"）供 rgba() 派生半透明色（如日志搜索高亮），
 *  避免依赖 color-mix（老版本 WKWebView 不支持）。 */
export function applyAccent(color: string) {
  const root = document.documentElement.style
  root.setProperty('--accent', color)
  const rgb = hexToRgbTriplet(color)
  if (rgb) root.setProperty('--accent-rgb', rgb)
}

/** #rgb / #rrggbb → "r,g,b"；非法输入返回 null */
function hexToRgbTriplet(hex: string): string | null {
  const m = /^#?([0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/.exec(hex.trim())
  if (!m) return null
  let h = m[1]
  if (h.length === 3) h = h.split('').map(c => c + c).join('')
  const n = parseInt(h, 16)
  return `${(n >> 16) & 255},${(n >> 8) & 255},${n & 255}`
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
