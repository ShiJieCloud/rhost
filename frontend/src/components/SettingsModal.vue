<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import AppLogo from './AppLogo.vue'
import { toast } from '../composables/useToast'
import {
  DEFAULT_SETTINGS, resolveTerminalFontFamily, savedSettings, saveSettings, showSettings,
  type AppSettings,
} from '../stores/settings'

/* =========================================================
   面板定义（数据驱动）
   ========================================================= */
type CtrlKind =
  | 'segmented' | 'switch' | 'select' | 'range' | 'text' | 'color'
  | 'keys' | 'buttons' | 'numberUnit' | 'number' | 'textarea'

interface Opt { v: string; t: string }
interface RowDef {
  key?: keyof AppSettings
  title: string
  desc?: string
  keywords?: string
  kind?: CtrlKind
  /** 选项：数组为静态；函数为动态（如字体安装态标记），渲染时调用并建立响应式依赖 */
  options?: Opt[] | (() => Opt[])
  min?: number
  max?: number
  step?: number
  /** select 值以 Number 转换后写入 draft（默认字符串） */
  num?: boolean
  /** 依赖的开关设置项：该开关为 false 时本控件置灰禁用 */
  disabledKey?: keyof AppSettings
  unit?: string
  width?: number
  placeholder?: string
  actions?: { id: 'export' | 'import' | 'reset' | 'changelog' | 'checkUpdate'; label: string; danger?: boolean }[]
}
interface GroupDef {
  label: string
  rows: RowDef[]
  extra?: 'envEditor' | 'preview'
}
interface PanelDef {
  id: string
  label: string
  glyph: string
  title: string
  sub: string
  groups: GroupDef[]
  about?: boolean
}

const ICONS: Record<string, string> = {
  appearance: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 3a9 9 0 000 18z" fill="currentColor" stroke="none"/></svg>',
  terminal: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M7 10l3 2-3 2M13 14h4"/></svg>',
  keymap: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="6" width="20" height="12" rx="2"/><path d="M6 10h.01M10 10h.01M14 10h.01M18 10h.01M7 14h10"/></svg>',
  advanced: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 00.3 1.9l.1.1a2 2 0 11-2.8 2.8l-.1-.1a1.7 1.7 0 00-1.9-.3 1.7 1.7 0 00-1 1.5V21a2 2 0 11-4 0v-.1a1.7 1.7 0 00-1.1-1.5 1.7 1.7 0 00-1.9.3l-.1.1a2 2 0 11-2.8-2.8l.1-.1a1.7 1.7 0 00.3-1.9 1.7 1.7 0 00-1.5-1H3a2 2 0 110-4h.1a1.7 1.7 0 001.5-1.1 1.7 1.7 0 00-.3-1.9l-.1-.1a2 2 0 112.8-2.8l.1.1a1.7 1.7 0 001.9.3h.1a1.7 1.7 0 001-1.5V3a2 2 0 114 0v.1a1.7 1.7 0 001 1.5 1.7 1.7 0 001.9-.3l.1-.1a2 2 0 112.8 2.8l-.1.1a1.7 1.7 0 00-.3 1.9v.1a1.7 1.7 0 001.5 1H21a2 2 0 110 4h-.1a1.7 1.7 0 00-1.5 1z"/></svg>',
  monitor: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 12 7 12 10 5 14 19 17 12 21 12"/></svg>',
  sftp: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 12h-6l-2 3h-4l-2-3H2"/><path d="M5.45 5.11L2 12v6a2 2 0 002 2h16a2 2 0 002-2v-6l-3.45-6.89A2 2 0 0016.76 4H7.24a2 2 0 00-1.79 1.11z"/><path d="M12 8v6"/><path d="M9.5 11.5L12 14l2.5-2.5"/></svg>',
  about: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><line x1="12" y1="11" x2="12" y2="16"/><line x1="12" y1="8" x2="12.01" y2="8"/></svg>',
}

const ACCENTS = [
  { v: '#3ddc84', t: '绿' },
  { v: '#7aa2f7', t: '蓝' },
  { v: '#bb9af7', t: '紫' },
  { v: '#e5b567', t: '黄' },
  { v: '#f7768e', t: '红' },
  { v: '#4fd6e0', t: '青' },
]

/** 可选等宽字体（monospace 之外的具名字体，安装态需检测） */
const FONT_CHOICES = ['JetBrains Mono', 'SF Mono', 'Fira Code', 'Cascadia Code', 'Menlo'] as const

/* ---- 字体安装态检测 ----
 * WebView 内 canvas 测量 / document.fonts.check 对本地字体判定不可靠
 * （实测 WKWebView 将系统自带的 Menlo 误报为未安装），改由后端经系统
 * 字体框架（macOS CoreText）权威判定，前端仅缓存结果驱动下拉标记。 */
/** 字体名 → 是否已安装；面板打开时刷新，期间新装字体重开面板即更新 */
const fontAvail = reactive<Record<string, boolean>>({})
async function refreshFontAvail() {
  try {
    const res = await invoke<Record<string, boolean>>('check_fonts', { names: [...FONT_CHOICES] })
    for (const name of FONT_CHOICES) fontAvail[name] = res[name] === true
  } catch (err) {
    console.error('[fonts] check_fonts failed', err)
  }
}
void refreshFontAvail()

const PANELS: PanelDef[] = [
  {
    id: 'appearance', label: '外观', glyph: 'appearance',
    title: '外观', sub: '调整主题与显示效果，更改会即时反映在应用中',
    groups: [
      {
        label: '界面',
        rows: [
          { key: 'uiTheme', title: '界面主题', desc: '跟随系统时随系统外观自动切换', kind: 'segmented', keywords: '界面 主题 明暗 深色 浅色 跟随系统',
            options: [{ v: 'dark', t: '深色' }, { v: 'light', t: '浅色' }, { v: 'auto', t: '跟随系统' }] },
          { key: 'accent', title: '强调色', desc: '用于按钮、选中态与焦点环', kind: 'color', keywords: '强调色 主题色 accent 颜色' },
          { key: 'opacity', title: '背景不透明度', desc: '降低数值可获得毛玻璃效果', kind: 'range', min: 60, max: 100, unit: '%', keywords: '透明度 不透明 opacity 毛玻璃 模糊 blur' },
        ],
      },
    ],
  },
  {
    id: 'terminal', label: '终端', glyph: 'terminal',
    title: '终端', sub: '配置 Shell 启动方式、字体、光标、会话行为与环境变量',
    groups: [
      {
        label: '字体',
        rows: [
          { key: 'fontFamily', title: '等宽字体', desc: '需要系统已安装该字体，未安装时自动回退到系统等宽', kind: 'select', width: 190, keywords: '字体 等宽 font family 字形 安装',
            options: () => [...FONT_CHOICES, 'monospace'].map(v => ({
              v,
              t: v === 'monospace' ? '系统等宽' : `${v}${fontAvail[v] === false ? '（未安装）' : ''}`,
            })) },
          { key: 'fontSize', title: '字号', kind: 'range', min: 10, max: 20, unit: 'px', keywords: '字号 大小 font size' },
          { key: 'lineHeight', title: '行高', kind: 'range', min: 100, max: 200, unit: '%', keywords: '行高 行距 line height' },
          { key: 'fontWeight', title: '字重', kind: 'select', width: 120, keywords: '字重 粗细 weight',
            options: [{ v: '300', t: '细体' }, { v: '400', t: '常规' }, { v: '500', t: '中等' }, { v: '700', t: '粗体' }] },
        ],
      },
      {
        label: '字体预览', extra: 'preview',
        rows: [],
      },
      {
        label: '启动',
        rows: [
          { key: 'colorPrompt', title: '彩色提示符', desc: '登录后为无颜色的远端 shell 配置彩色 PS1 与 ls/grep 颜色；远端已有彩色配置（oh-my-zsh 等）时自动跳过，不覆盖用户设置', kind: 'switch', keywords: '彩色 提示符 颜色 prompt ps1 注入 高亮' },
          { key: 'motd', title: '登录欢迎面板', desc: '连接成功后采集服务器负载、内存、磁盘、IP 等状态，在终端绘制 Rhost MOTD 欢迎横幅；开启时自动屏蔽 sshd 原生 MOTD 与 Last login 避免重复', kind: 'switch', keywords: 'motd 欢迎 面板 横幅 banner 登录 系统状态 负载 内存 磁盘 屏蔽 抑制 原生 last login' },
        ],
      },
      {
        label: '行为',
        rows: [
          { key: 'scrollback', title: '回滚缓冲区', desc: '可向上滚动的历史行数', kind: 'range', min: 1000, max: 100000, step: 1000, unit: ' 行', keywords: '回滚 缓冲 历史 行数 scrollback' },
          { key: 'trimOnCopy', title: '复制时去除行尾空格', kind: 'switch', keywords: '复制 行尾空格 修剪 trim copy' },
          { key: 'pasteGuard', title: '粘贴保护', desc: '多行粘贴时弹窗确认，避免误执行', kind: 'switch', keywords: '粘贴 保护 确认 paste 安全' },
          { key: 'rightClick', title: '右键行为', desc: '终端内右键：弹出操作菜单，或直接粘贴剪贴板', kind: 'segmented', keywords: '右键 粘贴 菜单 right click 鼠标',
            options: [{ v: 'menu', t: '菜单' }, { v: 'paste', t: '粘贴' }] },
        ],
      },
      {
        label: '光标',
        rows: [
          { key: 'cursorStyle', title: '光标样式', kind: 'segmented', keywords: '光标 样式 cursor 方块 竖线 下划线',
            options: [{ v: 'block', t: '方块' }, { v: 'bar', t: '竖线' }, { v: 'underline', t: '下划线' }] },
          { key: 'cursorBlink', title: '光标闪烁', desc: '空闲 1 秒后开始闪烁', kind: 'switch', keywords: '光标 闪烁 blink cursor' },
        ],
      },
      {
        label: '环境变量', extra: 'envEditor',
        rows: [],
      },
    ],
  },
  {
    id: 'sftp', label: 'SFTP', glyph: 'sftp',
    title: 'SFTP 文件传输', sub: '配置远端文件浏览与上传/下载行为',
    groups: [
      {
        label: '传输',
        rows: [
          { key: 'sftpChunkKb', title: '传输块大小', desc: '单次读写的数据量；大块吞吐更高，小块进度更细腻、弱网下重试成本更低', kind: 'select', num: true, width: 150, keywords: 'sftp 传输 块 分块 缓冲 chunk 大小 吞吐 速度',
            options: [32, 64, 128, 256, 512, 1024].map(v => ({ v: String(v), t: v >= 1024 ? '1 MB' : `${v} KB${v === 64 ? '（默认）' : ''}` })) },
          { key: 'sftpOverwritePolicy', title: '文件覆盖策略', desc: '上传或下载时，如果目标位置已存在同名文件，执行对应的处理规则', kind: 'select', width: 230, keywords: 'sftp 覆盖 策略 同名 冲突 overwrite skip 跳过 询问 更新 mtime',
            options: [
              { v: 'newer', t: '仅源文件更新时覆盖（默认）' },
              { v: 'skip', t: '不覆盖，跳过文件' },
              { v: 'overwrite', t: '直接覆盖全部' },
              { v: 'ask', t: '每次冲突询问' },
            ] },
          { key: 'sftpUploadTemp', title: '上传临时文件机制', desc: '上传先写入临时文件，传输完成后原子重命名，避免远端产生损坏文件', kind: 'switch', keywords: 'sftp 上传 临时文件 原子 重命名 temp atomic rename 损坏' },
          { key: 'sftpPreserveMeta', title: '保留文件元数据', desc: '传输文件时同步保留文件修改时间与权限属性，备份场景推荐开启；受服务器账号权限限制', kind: 'switch', keywords: 'sftp 元数据 保留 修改时间 权限 preserve mtime chmod 备份' },
        ],
      },
      {
        label: '断点续传',
        rows: [
          { key: 'sftpResume', title: '启用断点续传', desc: '传输中断后再次发起任务，可以从已传输完成位置继续传输，无需从头重传。关闭后所有文件每次都完整从头传输', kind: 'switch', keywords: 'sftp 断点续传 续传 resume 中断 重传 偏移' },
          { key: 'sftpResumeCheck', title: '断点校验方式', desc: '续传前校验已存在的半截文件，确认未被改动后从偏移位置续传；仅判断能否续传，与传输完成后的完整性校验相互独立', kind: 'select', width: 200, disabledKey: 'sftpResume', keywords: 'sftp 断点 校验 大小 mtime sha256 哈希 续传 半截',
            options: [
              { v: 'size', t: '文件大小（推荐）' },
              { v: 'sizeMtime', t: '文件大小 + mtime' },
              { v: 'sha256', t: 'SHA256 哈希' },
            ] },
        ],
      },
      {
        label: '并发与限速',
        rows: [
          { key: 'sftpGlobalConcurrency', title: '全局最大并发传输任务', desc: '整个客户端所有主机同时运行的上传与下载任务总数上限，超出上限的任务进入排队', kind: 'number', width: 100, min: 1, max: 20, step: 1, unit: ' 个', keywords: 'sftp 全局 并发 任务 上传 下载 排队 上限' },
          { key: 'sftpHostConcurrency', title: '单主机最大并发传输任务', desc: '同一台服务器同时运行的上传、下载任务上限，防止单台服务器并发过高导致连接卡顿', kind: 'number', width: 100, min: 1, max: 10, step: 1, unit: ' 个', keywords: 'sftp 单主机 服务器 并发 任务 卡顿 上限' },
          { key: 'sftpGlobalRateKb', title: '全局带宽限速', desc: '全部传输任务合计的总带宽上限，填写 0 代表不限制带宽', kind: 'number', width: 130, min: 0, step: 64, unit: ' KB/s', keywords: 'sftp 全局 带宽 限速 速率 限流 总带宽 KB' },
          { key: 'sftpTaskRateKb', title: '单任务带宽限速', desc: '单个文件传输任务的最大带宽上限，填写 0 代表不限制带宽', kind: 'number', width: 130, min: 0, step: 64, unit: ' KB/s', keywords: 'sftp 单任务 带宽 限速 速率 限流 KB' },
          { key: 'sftpRetryCount', title: '失败重试次数', desc: '遇到网络抖动、临时超时等可恢复错误时自动重试的最大次数，达到次数后任务标记失败', kind: 'number', width: 100, min: 0, max: 20, step: 1, unit: ' 次', keywords: 'sftp 失败 重试 次数 网络抖动 超时 错误' },
          { key: 'sftpRetryIntervalMs', title: '重试间隔', desc: '单次传输失败后等待指定毫秒再发起下一次重试，避免短时间密集请求冲击服务器', kind: 'number', width: 130, min: 0, step: 100, unit: ' ms', keywords: 'sftp 重试 间隔 等待 毫秒 退避 backoff' },
        ],
      },
      {
        label: '安全与完整性校验',
        rows: [
          { key: 'sftpIdleTimeoutSec', title: 'SFTP 会话空闲超时', desc: 'SFTP 子通道长时间没有文件操作时自动关闭释放资源，不会断开 SSH 终端会话，后续传输会自动重建通道；填 0 表示不自动关闭', kind: 'number', width: 130, min: 0, step: 10, unit: ' 秒', keywords: 'sftp 会话 空闲 超时 自动关闭 释放 通道 重建' },
          { key: 'sftpVerifyHash', title: '传输完成后完整性 Hash 校验', desc: '文件完整传输结束后，对整个文件计算 SHA256 哈希做完整性校验，校验失败标记任务异常；开启会增加 CPU 与 IO 开销', kind: 'switch', keywords: 'sftp 完整性 hash sha256 校验 哈希 完成 异常 cpu io' },
          { key: 'sftpBlacklist', title: '文件黑名单 glob 过滤列表', desc: '上传文件时，文件名匹配黑名单规则将自动跳过，不创建上传任务，一行一条 glob 表达式', kind: 'textarea', placeholder: '例如：*.tmp', keywords: 'sftp 黑名单 过滤 glob 跳过 上传 DS_Store Thumbs.db 排除 ignore' },
        ],
      },
      {
        label: '浏览',
        rows: [
          { key: 'sftpShowHidden', title: '显示隐藏文件', desc: '在本地与远端文件树中显示以 . 开头的文件', kind: 'switch', keywords: 'sftp 隐藏文件 点文件 hidden dotfile 显示' },
        ],
      },
    ],
  },
  {
    id: 'keymap', label: '快捷键', glyph: 'keymap',
    title: '快捷键', sub: '点击任意组合即可重新录制，按 Esc 取消',
    groups: [
      {
        label: '会话',
        rows: [
          { key: 'key.newTab', title: '新建标签页', kind: 'keys', keywords: '新建 标签页 new tab 快捷键' },
          { key: 'key.closeTab', title: '关闭标签页', kind: 'keys', keywords: '关闭 标签页 close tab 快捷键' },
          { key: 'key.splitV', title: '垂直分屏', kind: 'keys', keywords: '分屏 垂直 split 快捷键' },
          { key: 'key.splitH', title: '水平分屏', kind: 'keys', keywords: '水平 分屏 split 快捷键' },
        ],
      },
      {
        label: '操作',
        rows: [
          { key: 'key.clear', title: '清空屏幕', kind: 'keys', keywords: '清空 屏幕 clear 快捷键' },
          { key: 'key.palette', title: '命令面板', kind: 'keys', keywords: '命令 面板 command palette 快捷键' },
          { key: 'key.find', title: '查找', kind: 'keys', keywords: '搜索 查找 find 快捷键' },
          { key: 'key.settings', title: '打开设置', kind: 'keys', keywords: '设置 偏好 preferences 快捷键' },
        ],
      },
    ],
  },
  {
    id: 'monitor', label: '监控', glyph: 'monitor',
    title: '监控', sub: '客户端内存指标的采集频率、后台策略与告警阈值',
    groups: [
      {
        label: '性能与采集',
        rows: [
          { key: 'memInterval', title: '内存采集间隔', desc: '采集 Rhost 主进程驻留内存 (RSS) 用于底部状态栏监控；间隔越短折线越实时，IPC 开销越高', kind: 'select', num: true, width: 150, keywords: '监控 内存 采集 间隔 频率 interval 轮询 秒 性能',
            options: [1, 2, 3, 5, 10, 30, 60].map(v => ({ v: String(v), t: `${v} 秒${v === 2 ? '（默认）' : ''}` })) },
          { key: 'metricsInterval', title: '主机指标采集间隔', desc: '采集远端服务器的 CPU/内存/网络/磁盘/进程/GPU 并刷新右侧监控面板；间隔越短越实时，对远端与本机开销越高。允许 1~10 秒', kind: 'select', num: true, width: 150, keywords: '监控 主机 服务器 指标 cpu 内存 网络 磁盘 进程 gpu 采集 间隔 频率 轮询 秒 性能 ssh',
            options: [1, 2, 3, 5, 10].map(v => ({ v: String(v), t: `${v} 秒${v === 3 ? '（默认）' : ''}` })) },
          { key: 'memPauseHidden', title: '不可见时暂停采集', desc: '回到首页或窗口最小化时停止内存采集以降低后台开销；关闭后无论面板是否可见均持续采集', kind: 'switch', keywords: '暂停 后台 隐藏 最小化 不可见 pause 采集 cpu 开销' },
        ],
      },
      {
        label: '内存告警',
        rows: [
          { key: 'memAlertMb', title: '内存告警阈值', desc: '主进程常驻内存持续超过该值时，状态栏内存指标以告警色提示；此为监控告警阈值，不是硬性内存上限。允许范围 50 MB ~ 8 GB', kind: 'numberUnit', keywords: '内存 告警 阈值 上限 提醒 rss 占用 MB GB 预警' },
        ],
      },
    ],
  },
  {
    id: 'advanced', label: '高级', glyph: 'advanced',
    title: '高级', sub: '渲染、性能与配置文件管理',
    groups: [
      {
        label: '渲染',
        rows: [
          { key: 'gpuAccel', title: 'GPU 加速', desc: '关闭后使用软件渲染，可解决花屏问题', kind: 'switch', keywords: 'gpu 加速 硬件 渲染 acceleration 性能' },
          { key: 'renderer', title: '渲染后端', kind: 'select', width: 140, keywords: '渲染器 后端 renderer webgl webgpu',
            options: [{ v: 'auto', t: '自动' }, { v: 'webgl', t: 'WebGL' }, { v: 'webgpu', t: 'WebGPU' }, { v: 'canvas', t: 'Canvas' }] },
          { key: 'fps', title: '动画帧率', desc: '降低可减少电量消耗', kind: 'segmented', keywords: '刷新率 帧率 fps 动画 性能',
            options: [{ v: '30', t: '30' }, { v: '60', t: '60' }, { v: '120', t: '120' }] },
        ],
      },
      {
        label: '配置文件',
        rows: [
          { title: '导入 / 导出', desc: '将当前设置导出为 JSON，或从剪贴板恢复', kind: 'buttons', keywords: '导入 导出 配置 备份 config 文件',
            actions: [{ id: 'export', label: '导出' }, { id: 'import', label: '导入' }] },
          { title: '恢复默认设置', desc: '将所有选项还原为初始值，此操作不可撤销', kind: 'buttons', keywords: '重置 恢复 默认 reset 全部',
            actions: [{ id: 'reset', label: '恢复默认', danger: true }] },
        ],
      },
    ],
  },
  {
    id: 'about', label: '关于', glyph: 'about',
    title: '关于', sub: '版本信息与更新通道', about: true,
    groups: [
      {
        label: '更新',
        rows: [
          { key: 'autoUpdate', title: '自动检查更新', desc: '启动时在后台检查新版本', kind: 'switch', keywords: '自动更新 检查 update 升级' },
          { key: 'updateChannel', title: '更新通道', kind: 'select', width: 140, keywords: '更新通道 beta 稳定 stable channel',
            options: [{ v: 'stable', t: '稳定版' }, { v: 'beta', t: '测试版' }, { v: 'nightly', t: '每日构建' }] },
        ],
      },
    ],
  },
]

/* =========================================================
   状态
   ========================================================= */
const activePanelId = ref('appearance')
const query = ref('')
const searchEl = ref<HTMLInputElement | null>(null)
const modalEl = ref<HTMLElement | null>(null)

const draft = reactive(JSON.parse(JSON.stringify(savedSettings)) as AppSettings)

watch(showSettings, v => {
  if (!v) return
  Object.assign(draft, JSON.parse(JSON.stringify(savedSettings)))
  query.value = ''
  activePanelId.value = 'appearance'
  recordingKey.value = null
  alertUnit.value = 'MB'
  syncAlertRaw()
  // 刷新字体安装态：面板打开期间新装/卸载字体重开面板即反映
  void refreshFontAvail()
  nextTick(() => searchEl.value?.focus())
})

function close() {
  showSettings.value = false
}

const activePanel = computed(() => PANELS.find(p => p.id === activePanelId.value) ?? PANELS[0])

/* ---- 搜索 ---- */
function rowMatch(r: RowDef, q: string): boolean {
  return ((r.keywords ?? '') + ' ' + r.title + ' ' + (r.desc ?? '')).toLowerCase().includes(q)
}
const displayPanels = computed(() => {
  const q = query.value.trim().toLowerCase()
  if (!q) return [{ ...activePanel.value, searching: false, groups: activePanel.value.groups }]
  const out: (PanelDef & { searching: boolean; groups: GroupDef[] })[] = []
  for (const p of PANELS) {
    const groups = p.groups
      .map(g => ({ ...g, rows: g.rows.filter(r => rowMatch(r, q)) }))
      .filter(g => g.rows.length)
    if (groups.length) out.push({ ...p, searching: true, groups })
  }
  return out
})
const noResults = computed(() => !!query.value.trim() && !displayPanels.value.length)

/* ---- 脏检测 ---- */
function isDirty(key: keyof AppSettings): boolean {
  return JSON.stringify(draft[key]) !== JSON.stringify(savedSettings[key])
}
function groupDirty(g: GroupDef): boolean {
  return g.rows.some(r => r.key && isDirty(r.key)) ||
    (g.extra === 'envEditor' && g.rows.length === 0 && isDirty('env'))
}
function panelDirty(p: PanelDef): boolean {
  return p.groups.some(groupDirty)
}
const changedCount = computed(() => {
  let n = 0
  for (const p of PANELS) if (panelDirty(p)) n++
  return n
})

/* ---- 值写入 ---- */
function setVal(key: keyof AppSettings, v: unknown) {
  ;(draft as Record<string, unknown>)[key] = v
}
/** 数字输入：仅接受有限数字；失焦时收敛到 [min, max] */
function onNumInput(key: keyof AppSettings, e: Event) {
  const v = Number((e.target as HTMLInputElement).value)
  if (Number.isFinite(v)) setVal(key, v)
}
function onNumBlur(r: RowDef) {
  if (!r.key) return
  const v = Number(numVal(r.key)) || 0
  const clamped = Math.min(r.max ?? Number.MAX_SAFE_INTEGER, Math.max(r.min ?? 0, v))
  if (clamped !== v) setVal(r.key, clamped)
}
function strVal(key: keyof AppSettings | undefined): string {
  if (!key) return ''
  const v = draft[key]
  return typeof v === 'string' ? v : String(v ?? '')
}
function numVal(key: keyof AppSettings): number {
  return draft[key] as number
}
function boolVal(key: keyof AppSettings): boolean {
  return draft[key] as boolean
}
function resetOne(key: keyof AppSettings) {
  setVal(key, JSON.parse(JSON.stringify(savedSettings[key])))
  if (key === 'memAlertMb') syncAlertRaw()
  toast('已重置该项', 'info', 1500)
}

/* ---- 监控：采集间隔提示 ---- */
function ctlHint(r: RowDef): string {
  if (r.key === 'memInterval' || r.key === 'metricsInterval') {
    const n = numVal(r.key)
    const perMin = 60 / n
    const rate = perMin >= 10 ? String(Math.round(perMin)) : perMin.toFixed(1).replace(/\.0$/, '')
    return `每 ${n} 秒采集一次 · 约 ${rate} 次/分钟`
  }
  return ''
}

/* ---- 监控：内存告警阈值（数值 + MB/GB 单位） ---- */
const ALERT_MIN_MB = 50
const ALERT_MAX_MB = 8192
const alertUnit = ref<'MB' | 'GB'>('MB')
const alertRaw = ref(String(numVal('memAlertMb')))

/** 按当前单位把 draft 中的 MB 值格式化为输入框文本 */
function fmtByUnit(mb: number, unit: 'MB' | 'GB'): string {
  if (unit === 'GB') {
    const s = (Math.round((mb / 1024) * 100) / 100).toFixed(2).replace(/0+$/, '').replace(/\.$/, '')
    return s === '' ? '0' : s
  }
  return String(Math.round(mb))
}
function syncAlertRaw() {
  alertRaw.value = fmtByUnit(numVal('memAlertMb'), alertUnit.value)
}
/** 原始输入换算 MB；空值/非正数返回 NaN */
function alertToMb(raw: string): number {
  const v = Number(raw.trim())
  if (!Number.isFinite(v) || v <= 0) return NaN
  return alertUnit.value === 'GB' ? v * 1024 : v
}
const alertError = computed(() => {
  const raw = alertRaw.value.trim()
  if (raw === '') return '请输入内存告警阈值'
  const v = Number(raw)
  if (!Number.isFinite(v)) return '请输入有效数字'
  const mb = alertToMb(raw)
  if (Math.round(mb) < ALERT_MIN_MB) return `最小 ${ALERT_MIN_MB} MB：低于此值容易频繁告警`
  if (Math.round(mb) > ALERT_MAX_MB) return '最大 8 GB：超过此值告警基本不会触发，请确认单位'
  return ''
})
const alertStep = computed(() => (alertUnit.value === 'GB' ? '0.1' : '1'))

function onAlertInput(e: Event) {
  const raw = (e.target as HTMLInputElement).value
  alertRaw.value = raw
  const mb = alertToMb(raw)
  if (Number.isFinite(mb)) setVal('memAlertMb', Math.round(mb))
}
function onAlertBlur() {
  const raw = alertRaw.value.trim()
  if (raw === '') { syncAlertRaw(); return }
  const v = Number(raw)
  if (!Number.isFinite(v)) { syncAlertRaw(); return }
  // 失焦收敛到合法区间，再按单位回显
  const clamped = Math.min(ALERT_MAX_MB, Math.max(ALERT_MIN_MB, alertToMb(raw)))
  setVal('memAlertMb', Math.round(clamped))
  syncAlertRaw()
}
function onAlertUnitChange(e: Event) {
  const unit = (e.target as HTMLSelectElement).value as 'MB' | 'GB'
  const mb = alertToMb(alertRaw.value)
  alertUnit.value = unit
  // 切换单位时按原语义换算，避免阈值被静默改变；非法输入则按 draft 值重显
  alertRaw.value = Number.isFinite(mb) ? fmtByUnit(mb, unit) : fmtByUnit(numVal('memAlertMb'), unit)
}

/* ---- 环境变量 ---- */
function addEnv() {
  draft.env.push({ key: '', value: '' })
  nextTick(() => {
    const inputs = modalEl.value?.querySelectorAll('.st-env-row:last-child input')
    ;(inputs?.[0] as HTMLElement | null)?.focus()
  })
}
function delEnv(i: number) {
  draft.env.splice(i, 1)
}
function setEnvKey(i: number, v: string) { draft.env[i]!.key = v }
function setEnvValue(i: number, v: string) { draft.env[i]!.value = v }

/* ---- 快捷键录制 ---- */
const recordingKey = ref<keyof AppSettings | null>(null)
const keyLabels = ['Control', 'Meta', 'Alt', 'Shift', 'Escape']
function splitKeys(s: string): string[] {
  return s.split('+').filter(Boolean)
}
function resetRecording() {
  recordingKey.value = null
}

/* ---- 全局键盘 ---- */
function onKey(e: KeyboardEvent) {
  if (!showSettings.value) return

  if (recordingKey.value) {
    e.preventDefault()
    const key = recordingKey.value
    if (e.key === 'Escape') {
      resetRecording()
      toast('已取消录制', 'info', 1500)
      return
    }
    if (keyLabels.includes(e.key)) return // 只按了修饰键，继续等待
    const parts: string[] = []
    if (e.metaKey) parts.push('⌘')
    if (e.ctrlKey) parts.push('Ctrl')
    if (e.altKey) parts.push('Alt')
    if (e.shiftKey) parts.push('Shift')
    let k = e.key
    if (k === ' ') k = 'Space'
    else if (k.length === 1) k = k.toUpperCase()
    parts.push(k)
    setVal(key, parts.join('+'))
    resetRecording()
    toast('快捷键已更新', 'ok', 1600)
    return
  }

  if (e.key === 'Escape') {
    e.preventDefault()
    if (document.activeElement === searchEl.value) {
      searchEl.value?.blur()
      return
    }
    close()
  }
}
onMounted(() => document.addEventListener('keydown', onKey))
onUnmounted(() => document.removeEventListener('keydown', onKey))

/* ---- 保存 / 放弃 / 恢复默认 ---- */
function save() {
  saveSettings(draft)
  toast('设置已保存', 'ok', 1800)
}
function discard() {
  Object.assign(draft, JSON.parse(JSON.stringify(savedSettings)))
  toast('已放弃未保存的更改', 'info', 1600)
}
function resetAll() {
  Object.assign(draft, JSON.parse(JSON.stringify(DEFAULT_SETTINGS)))
  toast('已恢复默认设置，保存后生效', 'info', 2000)
}

/* ---- 导入 / 导出 ---- */
async function exportCfg() {
  const data = JSON.stringify(draft, null, 2)
  try {
    await navigator.clipboard.writeText(data)
    toast('配置已复制到剪贴板', 'ok', 2000)
  } catch {
    toast('导出失败，请检查权限', 'err', 2200)
  }
}
function importCfg() {
  // TODO: 接入文件选择与配置校验
  toast('导入功能开发中', 'info')
}
function onAction(id: string) {
  if (id === 'export') exportCfg()
  else if (id === 'import') importCfg()
  else if (id === 'reset') resetAll()
  else if (id === 'checkUpdate') toast('当前已是最新版本', 'ok', 1800)
  else if (id === 'changelog') toast('更新日志开发中', 'info')
}

/* ---- 字体预览 ---- */
const previewStyle = computed(() => ({
  fontFamily: resolveTerminalFontFamily(draft.fontFamily),
  fontSize: draft.fontSize + 'px',
  lineHeight: (draft.lineHeight / 100).toFixed(2),
  fontWeight: draft.fontWeight,
}))
const previewAccent = 'var(--blue)'

/* ---- 控件辅助 ---- */
function segActive(key: keyof AppSettings, v: string): boolean {
  return strVal(key) === v
}
/** 统一解析静态/动态选项（动态选项函数内读 reactive 缓存，变更驱动视图刷新） */
function optionList(r: RowDef): Opt[] {
  return typeof r.options === 'function' ? r.options() : r.options ?? []
}
</script>

<template>
  <div class="mask" :class="{ show: showSettings }" @click.self="close">
    <div ref="modalEl" class="modal settings" role="dialog" aria-modal="true" aria-label="设置">
      <!-- 头部 -->
      <div class="modal-head">
        <div class="modal-icon">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
               stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="3"/>
            <path d="M19.4 15a1.7 1.7 0 00.3 1.9l.1.1a2 2 0 11-2.8 2.8l-.1-.1a1.7 1.7 0 00-1.9-.3 1.7 1.7 0 00-1 1.5V21a2 2 0 11-4 0v-.1a1.7 1.7 0 00-1.1-1.5 1.7 1.7 0 00-1.9.3l-.1.1a2 2 0 11-2.8-2.8l.1-.1a1.7 1.7 0 00.3-1.9 1.7 1.7 0 00-1.5-1H3a2 2 0 110-4h.1a1.7 1.7 0 001.5-1.1 1.7 1.7 0 00-.3-1.9l-.1-.1a2 2 0 112.8-2.8l.1.1a1.7 1.7 0 001.9.3h.1a1.7 1.7 0 001-1.5V3a2 2 0 114 0v.1a1.7 1.7 0 001 1.5 1.7 1.7 0 001.9-.3l.1-.1a2 2 0 112.8 2.8l-.1.1a1.7 1.7 0 00-.3 1.9v.1a1.7 1.7 0 001.5 1H21a2 2 0 110 4h-.1a1.7 1.7 0 00-1.5 1z"/>
          </svg>
        </div>
        <div class="modal-title">
          <h2>设置</h2>
          <p>偏好与全局配置 · 更改后点击保存生效</p>
        </div>
        <button class="modal-close" title="关闭 (Esc)" @click="close">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
               stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line></svg>
        </button>
      </div>

      <!-- 主体 -->
      <div class="st-body">
        <!-- 侧栏 -->
        <aside class="st-side">
          <div class="st-search">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
              <circle cx="11" cy="11" r="7"/><path d="M20 20l-3.5-3.5"/>
            </svg>
            <input
              ref="searchEl"
              v-model="query"
              type="text"
              placeholder="搜索设置…"
              autocomplete="off"
              spellcheck="false"
            >
            <button v-if="query" class="st-search-clear" title="清除" @click="query = ''">✕</button>
          </div>

          <nav class="st-nav">
            <button
              v-for="p in PANELS"
              :key="p.id"
              class="st-nav-item"
              :class="{ active: !query && activePanelId === p.id, dirty: panelDirty(p) }"
              @click="activePanelId = p.id; query = ''"
            >
              <span class="st-glyph" v-html="ICONS[p.glyph]"></span>
              <span>{{ p.label }}</span>
              <span class="st-dirty-dot"></span>
            </button>
          </nav>
        </aside>

        <!-- 内容 -->
        <main class="st-content">
          <section
            v-for="p in displayPanels"
            :key="p.id"
            class="st-panel"
          >
            <template v-if="!p.searching">
              <div class="st-panel-head">
                <div class="st-panel-title">{{ p.title }}</div>
                <div class="st-panel-sub">{{ p.sub }}</div>
              </div>

              <!-- 关于卡片 -->
              <div v-if="p.about" class="st-about">
                <div class="st-about-logo"><AppLogo /></div>
                <div>
                  <div class="st-about-name">Rhost</div>
                  <div class="st-about-ver">v0.1.0 · 开发版</div>
                  <div class="st-about-desc">轻量、快速的 SSH 终端工作台。<br>基于 Web 技术构建，支持 GPU 加速渲染。</div>
                </div>
              </div>
              <div v-if="p.about" class="st-link-row">
                <button class="btn" @click="onAction('changelog')">查看更新日志</button>
                <button class="btn" @click="onAction('checkUpdate')">检查更新</button>
              </div>
            </template>
            <div v-else class="st-search-label">{{ p.label }}</div>

            <div
              v-for="g in p.groups"
              :key="g.label"
              class="st-group"
            >
              <div class="st-group-label">{{ g.label }}</div>

              <div v-if="g.rows.length" class="st-rows">
                <div
                  v-for="r in g.rows"
                  :key="r.title"
                  class="st-row"
                  :class="{ modified: r.key && isDirty(r.key), stack: r.kind === 'textarea' }"
                >
                  <div class="st-row-main">
                    <div class="st-row-title">
                      {{ r.title }}
                      <button
                        v-if="r.key"
                        class="st-reset"
                        title="重置此项"
                        @click="resetOne(r.key)"
                      >↺</button>
                    </div>
                    <div v-if="r.desc" class="st-row-desc">{{ r.desc }}</div>
                  </div>

                  <div class="st-row-ctl">
                    <!-- 开关 -->
                    <label v-if="r.kind === 'switch'" class="st-switch">
                      <input
                        type="checkbox"
                        :checked="boolVal(r.key!)"
                        @change="setVal(r.key!, ($event.target as HTMLInputElement).checked)"
                      >
                    </label>

                    <!-- 分段 -->
                    <div v-else-if="r.kind === 'segmented'" class="segmented">
                      <button
                        v-for="op in optionList(r)"
                        :key="op.v"
                        type="button"
                        :class="{ active: segActive(r.key!, op.v) }"
                        @click="setVal(r.key!, op.v)"
                      >{{ op.t }}</button>
                    </div>

                    <!-- 下拉 -->
                    <select
                      v-else-if="r.kind === 'select'"
                      class="st-select"
                      :style="{ width: (r.width ?? 150) + 'px' }"
                      :value="strVal(r.key)"
                      :disabled="!!r.disabledKey && !boolVal(r.disabledKey)"
                      @change="setVal(r.key!, r.num ? Number(($event.target as HTMLSelectElement).value) : ($event.target as HTMLSelectElement).value)"
                    >
                      <option v-for="op in optionList(r)" :key="op.v" :value="op.v">{{ op.t }}</option>
                    </select>

                    <!-- 数值 + 单位（内存告警阈值） -->
                    <div v-else-if="r.kind === 'numberUnit'" class="st-num-unit">
                      <input
                        class="st-input"
                        :class="{ invalid: alertError }"
                        type="number"
                        inputmode="decimal"
                        :min="r.min"
                        :max="r.max"
                        :step="alertStep"
                        :value="alertRaw"
                        spellcheck="false"
                        autocomplete="off"
                        @input="onAlertInput"
                        @blur="onAlertBlur"
                      >
                      <select
                        class="st-select st-unit-select"
                        :value="alertUnit"
                        @change="onAlertUnitChange"
                      >
                        <option value="MB">MB</option>
                        <option value="GB">GB</option>
                      </select>
                      <span v-if="alertError" class="st-ctl-err">{{ alertError }}</span>
                    </div>

                    <!-- 滑块 -->
                    <template v-else-if="r.kind === 'range'">
                      <input
                        type="range"
                        :min="r.min"
                        :max="r.max"
                        :step="r.step ?? 1"
                        :value="numVal(r.key!)"
                        @input="setVal(r.key!, Number(($event.target as HTMLInputElement).value))"
                      >
                      <span class="st-val">{{ numVal(r.key!) }}{{ r.unit }}</span>
                    </template>

                    <!-- 文本 -->
                    <input
                      v-else-if="r.kind === 'text'"
                      class="st-input"
                      type="text"
                      :style="{ width: (r.width ?? 200) + 'px' }"
                      :value="strVal(r.key)"
                      :placeholder="r.placeholder"
                      spellcheck="false"
                      autocomplete="off"
                      @input="setVal(r.key!, ($event.target as HTMLInputElement).value)"
                    >

                    <!-- 数字 + 单位后缀 -->
                    <div v-else-if="r.kind === 'number'" class="st-num-unit">
                      <input
                        class="st-input"
                        type="number"
                        inputmode="numeric"
                        :style="{ width: (r.width ?? 120) + 'px' }"
                        :min="r.min"
                        :max="r.max"
                        :step="r.step ?? 1"
                        :value="numVal(r.key!)"
                        spellcheck="false"
                        autocomplete="off"
                        @input="onNumInput(r.key!, $event)"
                        @blur="onNumBlur(r)"
                      >
                      <span v-if="r.unit" class="st-unit-label">{{ r.unit.trim() }}</span>
                    </div>

                    <!-- 多行文本（glob 列表等，一行一条） -->
                    <textarea
                      v-else-if="r.kind === 'textarea'"
                      class="st-input st-textarea"
                      :style="{ width: '100%' }"
                      rows="4"
                      :value="strVal(r.key)"
                      :placeholder="r.placeholder"
                      spellcheck="false"
                      autocomplete="off"
                      @input="setVal(r.key!, ($event.target as HTMLTextAreaElement).value)"
                    ></textarea>

                    <!-- 强调色 -->
                    <div v-else-if="r.kind === 'color'" class="st-colors">
                      <button
                        v-for="c in ACCENTS"
                        :key="c.v"
                        type="button"
                        class="st-color-dot"
                        :class="{ active: strVal(r.key) === c.v }"
                        :style="{ '--c': c.v }"
                        :title="c.t"
                        @click="setVal(r.key!, c.v)"
                      ></button>
                    </div>

                    <!-- 快捷键 -->
                    <div
                      v-else-if="r.kind === 'keys'"
                      class="st-keys"
                      :class="{ recording: recordingKey === r.key }"
                      @click="recordingKey = r.key!"
                    >
                      <template v-if="recordingKey === r.key">
                        <kbd class="rec">按下组合键…</kbd>
                        <span class="st-keys-cancel" title="取消 (Esc)" @click.stop="resetRecording">✕</span>
                      </template>
                      <template v-else>
                        <kbd v-for="(k, i) in splitKeys(strVal(r.key))" :key="i">{{ k }}</kbd>
                      </template>
                    </div>

                    <!-- 按钮组 -->
                    <template v-else-if="r.kind === 'buttons'">
                      <button
                        v-for="a in r.actions"
                        :key="a.id"
                        class="btn"
                        :class="{ danger: a.danger }"
                        @click="onAction(a.id)"
                      >{{ a.label }}</button>
                    </template>

                    <!-- 控件下方辅助提示（独占一行右对齐） -->
                    <span v-if="ctlHint(r)" class="st-ctl-hint">{{ ctlHint(r) }}</span>
                    <span
                      v-if="r.key === 'memPauseHidden' && !boolVal(r.key)"
                      class="st-ctl-hint warn"
                    >关闭后窗口最小化或回到首页时仍持续采集，可能增加后台开销</span>
                  </div>
                </div>
              </div>

              <!-- 环境变量编辑 -->
              <template v-if="g.extra === 'envEditor' && !p.searching">
                <div class="st-env-list">
                  <div v-for="(e, i) in draft.env" :key="i" class="st-env-row">
                    <input class="st-input key" type="text" :value="e.key" placeholder="KEY"
                           spellcheck="false" autocomplete="off" @input="setEnvKey(i, ($event.target as HTMLInputElement).value)">
                    <input class="st-input" type="text" :value="e.value" placeholder="value"
                           spellcheck="false" autocomplete="off" @input="setEnvValue(i, ($event.target as HTMLInputElement).value)">
                    <button class="st-env-del" title="删除" @click="delEnv(i)">✕</button>
                  </div>
                </div>
                <button class="st-btn-ghost" @click="addEnv">＋ 添加变量</button>
              </template>

              <!-- 字体实时预览 -->
              <div v-if="g.extra === 'preview' && !p.searching" class="st-preview" :style="previewStyle">
                <div class="ln"><span class="u" :style="{ color: previewAccent }">root@localhost</span>:<span class="u" :style="{ color: previewAccent }">~/projects</span>$ npm run build</div>
                <div class="ln gap"></div>
                <div class="ln"><span class="g">✔</span> 编译完成，用时 1.24s</div>
                <div class="ln"><span class="y">⚠</span> 2 个依赖已过时</div>
                <div class="ln c"># 连字测试: =&gt; != === -&gt; &lt;= ::</div>
              </div>
            </div>
          </section>

          <!-- 搜索空态 -->
          <div v-if="noResults" class="st-empty">
            <div class="st-empty-icon">⌕</div>
            <div class="st-empty-title">未找到匹配的设置项</div>
            <div class="st-empty-sub">试试更短的关键词，例如「字体」「光标」「主题」</div>
          </div>
        </main>
      </div>

      <!-- 底栏 -->
      <div class="modal-foot">
        <div class="st-foot-status" :class="{ dirty: changedCount > 0 }">
          <span class="ind"></span>
          {{ changedCount > 0 ? `${changedCount} 个分类有未保存的更改` : '所有更改已保存' }}
        </div>

        <div class="spacer"></div>

        <span class="kbd"><b>Esc 关闭</b></span>
        <button class="btn ghost" :disabled="changedCount === 0" @click="discard">放弃更改</button>
        <button class="btn primary" :disabled="changedCount === 0" @click="save">保存</button>
      </div>
    </div>
  </div>
</template>
