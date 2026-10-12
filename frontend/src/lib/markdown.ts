/**
 * AI 消息 Markdown 渲染。
 *
 * - markdown-it：`html: false` 关闭传入 HTML（全部转义，配合 v-html 防 XSS），
 *   `linkify: true` 自动识别裸链接，遵循 CommonMark（breaks 关闭）；
 * - highlight.js：core + 按需注册常用语言，控制打包体积；
 * - fence 块自定义渲染：语言标签 + 复制按钮的头部包装，
 *   按钮点击由 AiMarkdown 组件事件委托处理（原文取自相邻 pre 的 textContent）。
 */
import MarkdownIt from 'markdown-it'
import hljs from 'highlight.js/lib/core'
import bash from 'highlight.js/lib/languages/bash'
import json from 'highlight.js/lib/languages/json'
import yaml from 'highlight.js/lib/languages/yaml'
import rust from 'highlight.js/lib/languages/rust'
import python from 'highlight.js/lib/languages/python'
import javascript from 'highlight.js/lib/languages/javascript'
import typescript from 'highlight.js/lib/languages/typescript'
import go from 'highlight.js/lib/languages/go'
import c from 'highlight.js/lib/languages/c'
import cpp from 'highlight.js/lib/languages/cpp'
import java from 'highlight.js/lib/languages/java'
import sql from 'highlight.js/lib/languages/sql'
import ini from 'highlight.js/lib/languages/ini'
import diff from 'highlight.js/lib/languages/diff'
import dockerfile from 'highlight.js/lib/languages/dockerfile'
import nginx from 'highlight.js/lib/languages/nginx'
import xml from 'highlight.js/lib/languages/xml'
import css from 'highlight.js/lib/languages/css'
import powershell from 'highlight.js/lib/languages/powershell'
import lua from 'highlight.js/lib/languages/lua'
import http from 'highlight.js/lib/languages/http'

hljs.registerLanguage('bash', bash)
hljs.registerLanguage('json', json)
hljs.registerLanguage('yaml', yaml)
hljs.registerLanguage('rust', rust)
hljs.registerLanguage('python', python)
hljs.registerLanguage('javascript', javascript)
hljs.registerLanguage('typescript', typescript)
hljs.registerLanguage('go', go)
hljs.registerLanguage('c', c)
hljs.registerLanguage('cpp', cpp)
hljs.registerLanguage('java', java)
hljs.registerLanguage('sql', sql)
hljs.registerLanguage('ini', ini) // 覆盖 toml/conf 等键值配置
hljs.registerLanguage('diff', diff)
hljs.registerLanguage('dockerfile', dockerfile)
hljs.registerLanguage('nginx', nginx)
hljs.registerLanguage('xml', xml) // 覆盖 html/svg
hljs.registerLanguage('css', css)
hljs.registerLanguage('powershell', powershell)
hljs.registerLanguage('lua', lua)
hljs.registerLanguage('http', http)
// 常见别名归一到已注册语言
hljs.registerAliases(['sh', 'shell', 'zsh'], { languageName: 'bash' })
hljs.registerAliases(['toml', 'conf'], { languageName: 'ini' })
hljs.registerAliases(['html', 'svg'], { languageName: 'xml' })
hljs.registerAliases(['yml'], { languageName: 'yaml' })
hljs.registerAliases(['js', 'jsx', 'mjs'], { languageName: 'javascript' })
hljs.registerAliases(['ts', 'tsx'], { languageName: 'typescript' })
hljs.registerAliases(['py'], { languageName: 'python' })
hljs.registerAliases(['golang'], { languageName: 'go' })
hljs.registerAliases(['c++'], { languageName: 'cpp' })
hljs.registerAliases(['docker'], { languageName: 'dockerfile' })

const md = new MarkdownIt({ html: false, linkify: true, breaks: false })

// 外链新窗口打开（WebView 导航受 Tauri 管控）
const defaultLinkOpen =
  md.renderer.rules.link_open ??
  ((tokens, idx, options, _env, self) => self.renderToken(tokens, idx, options))
md.renderer.rules.link_open = (tokens, idx, options, env, self) => {
  tokens[idx].attrSet('target', '_blank')
  tokens[idx].attrSet('rel', 'noreferrer')
  return defaultLinkOpen(tokens, idx, options, env, self)
}

/** fence 块：语言标签 + 复制按钮头部 + 行号 gutter + 高亮代码（行号与代码同行高，横向滚动时 gutter 固定） */
md.renderer.rules.fence = (tokens, idx) => {
  const token = tokens[idx]
  const lang = (token.info || '').trim().split(/\s+/)[0]?.toLowerCase() ?? ''
  let highlighted = ''
  if (lang && hljs.getLanguage(lang)) {
    try {
      highlighted = hljs.highlight(token.content, { language: lang, ignoreIllegals: true }).value
    } catch {
      highlighted = '' // 高亮异常回退纯文本
    }
  }
  if (!highlighted) highlighted = md.utils.escapeHtml(token.content)
  const label = lang || 'text'
  // 行号列独立成 <pre>，与代码同 font/line-height 对齐；末尾空行不计数
  const lineCount = Math.max(1, token.content.replace(/\n$/, '').split('\n').length)
  const lineNos = Array.from({ length: lineCount }, (_, i) => i + 1).join('\n')
  return (
    '<div class="md-code">' +
    '<div class="md-code-head">' +
    `<span class="md-code-lang">${md.utils.escapeHtml(label)}</span>` +
    // 终端操作按钮由 AiMarkdown 按宿主 terminalReady 状态用 CSS 控制显隐
    '<div class="md-code-actions">' +
    '<button type="button" class="md-code-copy">复制</button>' +
    '<button type="button" class="md-code-insert">插入终端</button>' +
    '<button type="button" class="md-code-run">直接执行</button>' +
    '</div>' +
    '</div>' +
    '<div class="md-code-wrap">' +
    `<pre class="md-code-lines" aria-hidden="true">${lineNos}</pre>` +
    `<pre><code class="hljs">${highlighted}</code></pre>` +
    '</div>' +
    '</div>'
  )
}

/** 渲染 markdown 为 HTML（XSS 安全：传入 HTML 一律转义） */
export function renderMarkdown(src: string): string {
  return md.render(src)
}
