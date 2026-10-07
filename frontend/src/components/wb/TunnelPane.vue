<script setup lang="ts">
import { computed } from 'vue'
import type { TunnelRule, TunnelStatus } from '../../types'
import { activeSession } from '../../stores/session'
import { openEdit } from '../../stores/hosts'
import { isDesired, startTunnel, stopTunnel, tunnelSnapshots } from '../../stores/tunnels'
import { toast } from '../../composables/useToast'
import { isTauri } from '../../lib/tauri'

/* 当前会话的端口转发规则（配置源：主机 extra.tunnels）与运行态快照（0x0A 帧） */
const session = activeSession
const rules = computed(() => session.value?.host.tunnels ?? [])
const snapshots = tunnelSnapshots
const statusOf = (ruleId: string): TunnelStatus | undefined =>
  (session.value && snapshots.value.get(session.value.id)?.find(s => s.id === ruleId)) || undefined

/* 按类型分组展示（与设计稿一致：-L / -R / -D 三组，空组隐藏） */
const TYPE_LABELS: Record<TunnelRule['type'], string> = {
  local: '本地转发 (-L)',
  remote: '远程转发 (-R)',
  dynamic: '动态转发 SOCKS5 (-D)',
}
const groups = computed(() => {
  const out: { type: TunnelRule['type']; label: string; rules: TunnelRule[] }[] = []
  for (const type of ['local', 'remote', 'dynamic'] as const) {
    const rs = rules.value.filter(r => r.type === type)
    if (rs.length) out.push({ type, label: TYPE_LABELS[type], rules: rs })
  }
  return out
})

/* 实际监听端口（next 顺延 / 远端分配时不同于规则 bindPort，以快照为准） */
function bindPortOf(rule: TunnelRule): number {
  return statusOf(rule.id)?.boundPort || rule.bindPort
}
function endpointText(rule: TunnelRule): string {
  const p = bindPortOf(rule)
  if (rule.type === 'dynamic') return `socks5://${rule.bindHost}:${p}`
  const target = rule.targetHost && rule.targetPort ? `${rule.targetHost}:${rule.targetPort}` : '?'
  return `${rule.bindHost}:${p} → ${target}`
}

/* 状态点与状态文案 */
function dotClass(rule: TunnelRule): string {
  const st = statusOf(rule.id)
  if (st) return `dot-${st.state}`
  // 无快照条目的回退：仅在线且期望运行才是「启动中」过渡态；
  // 断线/重连中一律灰点（后端监听已随会话消失，无东西在启动）
  const sid = session.value?.id ?? ''
  if (session.value?.state === 'online' && isDesired(sid, rule.id)) return 'dot-starting'
  return 'dot-stopped'
}
function stateText(rule: TunnelRule): string {
  const st = statusOf(rule.id)
  if (st) {
    if (st.state === 'active') return `活动 ${st.activeConnections}`
    if (st.state === 'starting') return '启动中…'
    if (st.state === 'error') return st.error || '启动失败'
    return '已停止'
  }
  const sid = session.value?.id ?? ''
  const desired = isDesired(sid, rule.id)
  if (session.value?.state === 'online') return desired ? '启动中…' : '未启动'
  // 会话非在线：期望运行的规则显示「已断开」（保留用户意图，重连后自动恢复），
  // 未期望的显示「未启动」——区分「网络断了」与「用户关的」
  return desired ? '已断开' : '未启动'
}

/* 流量自适应格式（B/KB/MB/GB） */
function fmtBytes(n: number): string {
  if (n < 1024) return `${Math.round(n)} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  if (n < 1024 * 1024 * 1024) return `${(n / 1048576).toFixed(1)} MB`
  return `${(n / 1073741824).toFixed(2)} GB`
}
const trafficText = (st: TunnelStatus): string => `↑${fmtBytes(st.bytesUp)} ↓${fmtBytes(st.bytesDown)}`

/* 开关：desired 驱动显示；start 失败（含非 Tauri dev）由 store toast 并回滚勾选态。
 * 停止语义 tooltip：不再接受新连接，已有连接最多等待 3s 完成传输 */
/* 开关显示依据：后端快照优先，无快照时回退到期望集 */
function switchChecked(rule: TunnelRule): boolean {
  const st = statusOf(rule.id)
  if (st) return st.state === 'active' || st.state === 'starting'
  return isDesired(session.value?.id ?? '', rule.id)
}

async function onToggle(rule: TunnelRule, ev: Event) {
  const el = ev.target as HTMLInputElement
  const on = el.checked
  const s = session.value
  if (!s) { el.checked = !on; return }
  if (on) {
    const ok = await startTunnel(s, rule)
    if (!ok) el.checked = false
  } else {
    await stopTunnel(s.id, rule.id)
  }
}
function switchTitle(rule: TunnelRule): string {
  return isDesired(session.value?.id ?? '', rule.id)
    ? '停止：不再接受新连接，已有连接最多等待 3s 完成传输'
    : '启动该规则'
}

/* 批量启停：逐条串行（avoid 并发 invoke 打爆端口探测），单条失败不中断 */
async function startAll() {
  const s = session.value
  if (!s) return
  for (const r of rules.value) await startTunnel(s, r)
}
async function stopAll() {
  const s = session.value
  if (!s) return
  for (const r of rules.value) await stopTunnel(s.id, r.id)
}

/* 空态引导：跳转主机配置面板（编辑态弹窗，端口转发卡片在其中） */
function gotoRuleEditor() {
  const h = session.value?.host
  if (!h) return
  if (!isTauri) {
    toast('[dev] 浏览器模式无 Tauri 后端', 'err')
    return
  }
  openEdit(h)
}
</script>

<template>
  <div class="tnl-pane">
    <template v-if="session">
      <div v-if="rules.length" class="tnl-bar">
        <span class="tnl-title">端口转发</span>
        <span class="tnl-count">{{ rules.length }} 条规则</span>
        <span class="spacer"></span>
        <button class="tnl-bar-btn" @click="startAll">全部启动</button>
        <button class="tnl-bar-btn" @click="stopAll">全部停止</button>
      </div>

      <div v-if="!rules.length" class="tnl-empty">
        <p>当前主机未配置端口转发规则</p>
        <button class="tnl-empty-btn" @click="gotoRuleEditor">去主机配置面板添加端口转发规则</button>
      </div>

      <div v-else class="tnl-list">
        <section v-for="g in groups" :key="g.type" class="tnl-group">
          <div class="tnl-group-label">{{ g.label }}</div>
          <div v-for="rule in g.rules" :key="rule.id" class="tnl-row"
               :title="statusOf(rule.id)?.state === 'error' ? statusOf(rule.id)?.error : undefined">
            <span class="tnl-dot" :class="dotClass(rule)"></span>
            <span class="tnl-name" :title="rule.name">{{ rule.name || endpointText(rule) }}</span>
            <span class="tnl-ep">{{ endpointText(rule) }}</span>
            <span v-if="statusOf(rule.id)?.state === 'active' && (statusOf(rule.id)!.bytesUp || statusOf(rule.id)!.bytesDown)"
                  class="tnl-traffic">{{ trafficText(statusOf(rule.id)!) }}</span>
            <span class="tnl-state"
                  :class="{ err: statusOf(rule.id)?.state === 'error', act: statusOf(rule.id)?.state === 'active' }">
              {{ stateText(rule) }}
            </span>
            <label class="st-switch" :title="switchTitle(rule)">
              <input type="checkbox" :checked="switchChecked(rule)" @change="onToggle(rule, $event)">
            </label>
          </div>
        </section>
      </div>
    </template>

    <div v-else class="tnl-empty">
      <p>无活动会话</p>
    </div>
  </div>
</template>

<style scoped>
.tnl-pane {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  overflow: hidden;
}
.tnl-bar {
  display: flex;
  align-items: center;
  gap: 10px;
  flex: none;
  padding: 8px 12px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.06);
}
.tnl-title { font-size: 12px; font-weight: 600; color: var(--text, #dbe4f0); }
.tnl-count { font-size: 11px; color: var(--dim, #7d8aa0); }
.spacer { flex: 1; }
.tnl-bar-btn {
  flex: none;
  padding: 3px 10px;
  font-size: 11px;
  border-radius: 6px;
  border: 1px solid rgba(255, 255, 255, 0.12);
  background: transparent;
  color: var(--text, #dbe4f0);
  cursor: pointer;
}
.tnl-bar-btn:hover { background: rgba(255, 255, 255, 0.07); }

.tnl-list {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 8px 12px 12px;
}
.tnl-group { margin-bottom: 10px; }
.tnl-group-label {
  font-size: 11px;
  color: var(--dim, #7d8aa0);
  margin: 6px 0 4px;
}
.tnl-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 5px 8px;
  border-radius: 7px;
  min-width: 0;
}
.tnl-row:hover { background: rgba(255, 255, 255, 0.045); }
.tnl-dot {
  flex: none;
  width: 8px;
  height: 8px;
  border-radius: 50%;
}
.dot-active { background: var(--green, #3ddc84); }
.dot-starting { background: #e8c34a; animation: tnl-pulse 1.1s ease-in-out infinite; }
.dot-error { background: #ef5b58; }
.dot-stopped { background: #55606f; }
@keyframes tnl-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.35; }
}
.tnl-name {
  flex: none;
  max-width: 180px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 12px;
  color: var(--text, #dbe4f0);
}
.tnl-ep {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 11px;
  font-family: var(--mono, monospace);
  color: var(--dim, #7d8aa0);
}
.tnl-traffic {
  flex: none;
  font-size: 11px;
  font-family: var(--mono, monospace);
  color: var(--dim, #7d8aa0);
}
.tnl-state {
  flex: none;
  max-width: 300px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 11px;
  color: var(--dim, #7d8aa0);
}
.tnl-state.act { color: var(--green, #3ddc84); }
.tnl-state.err { color: #ef5b58; }
.tnl-row .st-switch { margin-left: 4px; }

.tnl-empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 12px;
  color: var(--dim, #7d8aa0);
  font-size: 12px;
}
.tnl-empty-btn {
  padding: 5px 14px;
  font-size: 12px;
  border-radius: 7px;
  border: 1px solid rgba(255, 255, 255, 0.14);
  background: transparent;
  color: var(--text, #dbe4f0);
  cursor: pointer;
}
.tnl-empty-btn:hover { background: rgba(255, 255, 255, 0.07); }
</style>
