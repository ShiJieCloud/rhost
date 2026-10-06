import { defineConfig } from 'vitepress'

// 内容根目录为 docs/（本配置位于 docs/.vitepress/），首页对应 docs/index.md → 路由 /
// nav / sidebar 的 link 一律不带 .md 扩展名；新增文档后在此同步登记
// base 本地默认根路径；部署到 GitHub Pages 项目页（<user>.github.io/<repo>/）时
// 由 CI 通过 DOCS_BASE 注入 '/<repo>/'，无需为本地/CI 维护两份配置
export default defineConfig({
  lang: 'zh-CN',
  title: 'Rhost',
  description: '基于 Tauri 2 + Rust + Vue 3 的跨平台 SSH 远程主机管理器',
  base: process.env.DOCS_BASE ?? '/',
  cleanUrls: true,
  lastUpdated: true,

  // 文档中的源码引用（../frontend/、../src-tauri/ 下的 .vue/.rs/.ts 等）在
  // GitHub/编辑器中有效，但位于 VitePress 内容根之外，不纳入站内死链检查
  ignoreDeadLinks: [
    /(?:^|[/\\])(frontend|src-tauri)(?:[/\\]|$)/,
    /\.(vue|rs|ts|js|mts|json|toml|css)(?:[#?].*)?$/,
  ],

  themeConfig: {
    nav: [
      { text: '指南', link: '/development', activeMatch: '/development' },
      { text: '架构', link: '/architecture', activeMatch: '/architecture' },
      {
        text: '设计',
        activeMatch: '/(metrics-design|applog-design|config-import-export|splash-design|explanation)/',
        items: [
          { text: '主机指标采集', link: '/metrics-design' },
          { text: '应用日志', link: '/applog-design' },
          { text: '配置导入导出', link: '/config-import-export' },
          { text: '启动动画', link: '/splash-design' },
          { text: '文档规范', link: '/explanation/design/docs-spec' },
        ],
      },
      {
        text: '排障',
        activeMatch: '/(applog-troubleshooting|vim-utf8-locale)/',
        items: [
          { text: '应用日志排查手册', link: '/applog-troubleshooting' },
          { text: 'Vim 中文编码问题', link: '/vim-utf8-locale' },
        ],
      },
    ],

    sidebar: [
      {
        text: '开发与架构',
        items: [
          { text: '开发指南', link: '/development' },
          { text: '架构与通信协议', link: '/architecture' },
        ],
      },
      {
        text: '设计方案',
        items: [
          { text: '主机指标采集', link: '/metrics-design' },
          { text: '应用日志', link: '/applog-design' },
          { text: '配置导入导出', link: '/config-import-export' },
          { text: '启动动画', link: '/splash-design' },
          { text: '文档规范', link: '/explanation/design/docs-spec' },
        ],
      },
      {
        text: '故障排查',
        items: [
          { text: '应用日志排查手册', link: '/applog-troubleshooting' },
          { text: 'Vim 中文编码问题', link: '/vim-utf8-locale' },
        ],
      },
    ],

    outline: { label: '本页目录' },
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
