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
              { text: '文档规范', link: '/explanation/design/docs-spec' },
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
