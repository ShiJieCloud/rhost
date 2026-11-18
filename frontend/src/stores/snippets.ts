import { ref, watch } from 'vue'
import { isTauri } from '../lib/tauri'
import { normalizeVars, type SnippetVar } from '../lib/snippetVars'
import { onConfigLoad, schedulePersist } from './appConfig'

/**
 * 命令片段状态。
 * 模块级单例；Tauri 下由后端 app_config.json snippets 节持久化，
 * 浏览器 dev 模式降级为内存演示数据。
 */

export interface Snippet {
  id: string
  name: string
  group: string
  command: string
  description: string
  /** 变量声明；undefined 表示老数据，运行期按命令自动识别 */
  vars?: SnippetVar[]
}

/** 浏览器 dev 模式的演示数据；Tauri 生产环境初始为空（由快照 hydrate） */
function seedSnippets(): Snippet[] {
  return [
    { id: 's_seed_1', name: '查看磁盘使用', group: '系统排查', command: 'df -h', description: '' },
    { id: 's_seed_2', name: '查看内存占用', group: '系统排查', command: 'free -h', description: '' },
    { id: 's_seed_3', name: '查看监听端口', group: '系统排查', command: 'ss -tulpn | grep LISTEN', description: '' },
    { id: 's_seed_4', name: '系统负载快照', group: '系统排查', command: 'uptime && top -bn1 | head -20', description: '' },
    { id: 's_seed_5', name: '查看运行中容器', group: 'Docker', command: "docker ps --format 'table {{.Names}}\\t{{.Status}}'", description: '' },
    { id: 's_seed_6', name: '清理无用镜像', group: 'Docker', command: 'docker image prune -af', description: '' },
    { id: 's_seed_7', name: '查看容器日志', group: 'Docker', command: 'docker logs -f --tail 200 {{container}}', description: '' },
    { id: 's_seed_8', name: '测试配置语法', group: 'Nginx', command: 'nginx -t', description: '' },
    { id: 's_seed_9', name: '重载配置', group: 'Nginx', command: 'nginx -s reload', description: '' },
    { id: 's_seed_10', name: '查看错误日志', group: 'Nginx', command: 'tail -f /var/log/nginx/error.log', description: '' },
    { id: 's_seed_11', name: '部署后健康检查', group: '自定义', command: 'curl -fsS http://127.0.0.1:8080/health', description: '' },
  ]
}

/** 快照元素合法性（后端 schema 已保证 id/name/command，前端再兜底） */
function isValidSnippet(v: unknown): v is Snippet {
  if (!v || typeof v !== 'object') return false
  const o = v as Record<string, unknown>
  return typeof o.id === 'string' && typeof o.name === 'string' && typeof o.command === 'string'
}

export const snippets = ref<Snippet[]>(isTauri ? [] : seedSnippets())

/** hydrate 当次赋值不回写 */
let hydrated = !isTauri

onConfigLoad(snap => {
  if (!isTauri) return
  const list = Array.isArray(snap.snippets) ? snap.snippets.filter(isValidSnippet) : []
  hydrated = false
  snippets.value = list.map(s => ({
    id: s.id,
    name: s.name,
    group: typeof s.group === 'string' ? s.group : '',
    command: s.command,
    description: typeof s.description === 'string' ? s.description : '',
    vars: normalizeVars(s.vars),
  }))
  hydrated = true
})

watch(
  snippets,
  v => {
    if (!isTauri || !hydrated) return
    schedulePersist('snippets', v)
  },
  { deep: true, flush: 'sync' },
)

/* ==================== CRUD ==================== */

let seq = 0
function uid(): string {
  return 's_' + Date.now().toString(36) + '_' + (++seq).toString(36)
}

export type SnippetInput = Omit<Snippet, 'id'>

export function addSnippet(input: SnippetInput): Snippet {
  const item: Snippet = { ...input, id: uid() }
  snippets.value.unshift(item)
  return item
}

export function updateSnippet(id: string, input: SnippetInput): boolean {
  const idx = snippets.value.findIndex(s => s.id === id)
  if (idx === -1) return false
  snippets.value[idx] = { ...input, id }
  return true
}

export function removeSnippet(id: string) {
  const idx = snippets.value.findIndex(s => s.id === id)
  if (idx !== -1) snippets.value.splice(idx, 1)
}

export function findSnippet(id: string | null): Snippet | null {
  if (!id) return null
  return snippets.value.find(s => s.id === id) ?? null
}
