import { defineConfig } from 'vitepress'

// 内容根目录为 docs/（本配置位于 docs/.vitepress/），首页对应 docs/index.md → 路由 /
// nav / sidebar 的 link 一律不带 .md 扩展名；新增文档后在此同步登记
// 部署基路径：本地默认 /；CI 部署到 GitHub Pages 项目页（<user>.github.io/<repo>/）时
// 由 DOCS_BASE 注入 '/<repo>/'，无需维护两份配置。head 中的静态资源 href 不会被
// VitePress 自动补 base，需手动拼接（themeConfig.logo 由框架处理）
const siteBase = process.env.DOCS_BASE ?? '/'

export default defineConfig({
  lang: 'zh-CN',
  title: 'Rhost',
  description: '基于 Tauri 2 + Rust + Vue 3 的跨平台 SSH 远程主机管理器',
  base: siteBase,
  cleanUrls: true,
  lastUpdated: true,

  // 浏览器标签栏图标：取自桌面端应用图标（frontend/public/favicon.svg 的副本，方形底
  // 座在 16px 下比导航栏的细线 logo 更醒目），与 themeConfig.logo 是两套图形
  head: [['link', { rel: 'icon', type: 'image/svg+xml', href: `${siteBase}favicon.svg` }]],

  // 文档中的源码引用（../frontend/、../src-tauri/ 下的 .vue/.rs/.ts 等）在
  // GitHub/编辑器中有效，但位于 VitePress 内容根之外，不纳入站内死链检查
  ignoreDeadLinks: [
    /(?:^|[/\\])(frontend|src-tauri)(?:[/\\]|$)/,
    /\.(vue|rs|ts|js|mts|json|toml|css)(?:[#?].*)?$/,
  ],

  themeConfig: {
    // 站点 LOGO：取自前端 AppLogo（终端窗口 + 命令提示符），
    // 配色沿用 QuickConnectView 中 .qc-mark 的 --green:#3ddc84，在浅/深模式下均可见
    logo: '/logo.svg',

    nav: [
      { text: '快速开始', link: '/getting-started/', activeMatch: '/getting-started/' },
      {
        text: '操作指南',
        activeMatch: '/guides/',
        items: [
          { text: '安装', link: '/guides/install' },
          { text: '快速连接', link: '/guides/quick-connect' },
          { text: '主机与密钥管理', link: '/guides/host-management' },
          { text: 'SSH 终端', link: '/guides/ssh-terminal' },
          { text: 'SFTP 文件传输', link: '/guides/sftp' },
          { text: '配置导入导出', link: '/guides/config-import-export' },
          {
            text: 'AI 助手',
            items: [
              { text: 'AI 助手使用指南', link: '/guides/ai-assistant' },
              { text: 'AI 规划模式', link: '/guides/ai-plan-mode' },
              { text: 'AI 终端执行与授权', link: '/guides/ai-terminal-exec' },
              { text: 'AI 技能、规则与记忆配置', link: '/guides/ai-skills-rules-memory' },
            ],
          },
          { text: '开发指南', link: '/guides/development' },
          { text: '贡献指南', link: '/guides/contributing' },
          { text: 'CI/CD 流水线', link: '/guides/ci-cd' },
          { text: '触发发版流水线', link: '/guides/release-trigger' },
          { text: '应用日志排查手册', link: '/guides/troubleshooting/applog-troubleshooting' },
          { text: 'Vim 中文编码问题', link: '/guides/troubleshooting/vim-utf8-locale' },
        ],
      },
      {
        text: '参考',
        activeMatch: '/reference/',
        items: [
          { text: '全局设置', link: '/reference/global-settings' },
          { text: '发版 Release notes 模板', link: '/reference/release-notes-template' },
        ],
      },
      {
        text: '解释',
        activeMatch: '/explanation/',
        items: [
          { text: '架构与通信协议', link: '/explanation/architecture' },
          { text: '主机指标采集', link: '/explanation/design/metrics-design' },
          { text: '应用日志', link: '/explanation/design/applog-design' },
          { text: '配置导入导出', link: '/explanation/design/config-import-export-design' },
          { text: '启动动画', link: '/explanation/design/splash-design' },
          { text: '标签栏右键菜单', link: '/explanation/design/tab-context-menu-design' },
          { text: '标签栏右键菜单开发计划', link: '/explanation/design/tab-context-menu-dev-plan' },
          { text: '标签栏横向滚动', link: '/explanation/design/tab-scroll-design' },
          { text: 'SSH 端口转发', link: '/explanation/design/tunnel-design' },
          { text: 'SSH 端口转发开发计划', link: '/explanation/design/tunnel-dev-plan' },
          { text: 'SSH 主机密钥校验', link: '/explanation/design/hostkey-verification-design' },
          { text: 'CI 三平台打包与发布', link: '/explanation/design/ci-release-design' },
          { text: 'AI 消息操作按钮', link: '/explanation/design/ai-message-actions-design' },
          { text: 'AI 终端命令执行', link: '/explanation/design/ai-terminal-exec-design' },
          { text: 'AI 技能/规则配置', link: '/explanation/design/ai-skills-rules-design' },
          { text: 'AI 跨会话记忆', link: '/explanation/design/ai-memory-design' },
          { text: 'AI ReAct 循环', link: '/explanation/design/ai-react-loop-design' },
          { text: 'AI Plan-and-Act 推理链编排', link: '/explanation/design/ai-plan-and-act-design' },
          { text: 'AI 规划触发机制（模型自主提案）', link: '/explanation/design/ai-plan-trigger-design' },
          { text: 'AI 计划执行提前收敛', link: '/explanation/design/ai-plan-early-convergence-design' },
          { text: 'AI 多模态输入（图片与文本文件附件）', link: '/explanation/design/ai-multimodal-design' },
          { text: 'AI 消息渲染方案 v2', link: '/explanation/design/ai-message-render-v2-design' },
          { text: 'AI 会话大纲', link: '/explanation/design/ai-chat-outline-design' },
          { text: '文档规范', link: '/explanation/design/docs-spec' },
        ],
      },
    ],

    sidebar: [
      {
        text: '教程',
        items: [
          { text: '项目概览', link: '/tutorial/overview' },
          { text: '快速开始', link: '/getting-started/' },
        ],
      },
      {
        text: '操作指南',
        items: [
          { text: '安装', link: '/guides/install' },
          { text: '快速连接', link: '/guides/quick-connect' },
          { text: '主机与密钥管理', link: '/guides/host-management' },
          { text: 'SSH 终端', link: '/guides/ssh-terminal' },
          { text: 'SFTP 文件传输', link: '/guides/sftp' },
          { text: '配置导入导出', link: '/guides/config-import-export' },
          {
            text: 'AI 助手',
            items: [
              { text: 'AI 助手使用指南', link: '/guides/ai-assistant' },
              { text: 'AI 规划模式', link: '/guides/ai-plan-mode' },
              { text: 'AI 终端执行与授权', link: '/guides/ai-terminal-exec' },
              { text: 'AI 技能、规则与记忆配置', link: '/guides/ai-skills-rules-memory' },
            ],
          },
          {
            text: '故障排查',
            items: [
              { text: '应用日志排查手册', link: '/guides/troubleshooting/applog-troubleshooting' },
              { text: 'Vim 中文编码问题', link: '/guides/troubleshooting/vim-utf8-locale' },
            ],
          },
        ],
      },
      {
        text: '开发者',
        items: [
          { text: '开发指南', link: '/guides/development' },
          { text: '贡献指南', link: '/guides/contributing' },
          { text: 'CI/CD 流水线', link: '/guides/ci-cd' },
          { text: '触发发版流水线', link: '/guides/release-trigger' },
          { text: '架构与通信协议', link: '/explanation/architecture' },
        ],
      },
      {
        text: '参考',
        items: [
          { text: '全局设置', link: '/reference/global-settings' },
          { text: '发版 Release notes 模板', link: '/reference/release-notes-template' },
        ],
      },
      {
        text: 'FAQ',
        items: [
          { text: '常见问题', link: '/faq/' },
        ],
      },
      {
        text: '解释',
        items: [
          { text: '架构与通信协议', link: '/explanation/architecture' },
          {
            text: '设计方案',
            items: [
              { text: '主机指标采集', link: '/explanation/design/metrics-design' },
              { text: '应用日志', link: '/explanation/design/applog-design' },
              { text: '配置导入导出', link: '/explanation/design/config-import-export-design' },
              { text: '启动动画', link: '/explanation/design/splash-design' },
              { text: '标签栏右键菜单', link: '/explanation/design/tab-context-menu-design' },
              { text: '标签栏右键菜单开发计划', link: '/explanation/design/tab-context-menu-dev-plan' },
              { text: '标签栏横向滚动', link: '/explanation/design/tab-scroll-design' },
              { text: 'SSH 端口转发', link: '/explanation/design/tunnel-design' },
              { text: 'SSH 端口转发开发计划', link: '/explanation/design/tunnel-dev-plan' },
              { text: 'SSH 主机密钥校验', link: '/explanation/design/hostkey-verification-design' },
              { text: 'CI 三平台打包与发布', link: '/explanation/design/ci-release-design' },
              { text: 'AI 消息操作按钮', link: '/explanation/design/ai-message-actions-design' },
              { text: 'AI 终端命令执行', link: '/explanation/design/ai-terminal-exec-design' },
              { text: 'AI 技能/规则配置', link: '/explanation/design/ai-skills-rules-design' },
              { text: 'AI 跨会话记忆', link: '/explanation/design/ai-memory-design' },
              { text: 'AI ReAct 循环', link: '/explanation/design/ai-react-loop-design' },
              { text: 'AI Plan-and-Act 推理链编排', link: '/explanation/design/ai-plan-and-act-design' },
              { text: 'AI 规划触发机制（模型自主提案）', link: '/explanation/design/ai-plan-trigger-design' },
              { text: 'AI 计划执行提前收敛', link: '/explanation/design/ai-plan-early-convergence-design' },
              { text: 'AI 多模态输入（图片与文本文件附件）', link: '/explanation/design/ai-multimodal-design' },
              { text: 'AI 消息渲染方案 v2', link: '/explanation/design/ai-message-render-v2-design' },
              { text: 'AI 会话大纲', link: '/explanation/design/ai-chat-outline-design' },
              { text: '文档规范', link: '/explanation/design/docs-spec' },
              { text: 'ADR-001 规划触发权改为模型自主提案', link: '/explanation/design/decisions/ADR-001-model-driven-plan-trigger' },
            ],
          },
        ],
      },
    ],

    outline: { label: '本页目录', level: [2, 4] },
    docFooter: { prev: '上一页', next: '下一页' },
    lastUpdatedText: '最后更新',
    darkModeSwitchLabel: '外观',
    sidebarMenuLabel: '菜单',
    returnToTopLabel: '返回顶部',

    search: {
      provider: 'local',
      options: {
        translations: {
          button: { buttonText: '搜索文档', buttonAriaLabel: '搜索文档' },
          modal: {
            noResultsText: '没有找到相关结果',
            resetButtonTitle: '清除查询条件',
            footer: { selectText: '选择', navigateText: '切换' },
          },
        },
      },
    },
  },
})
