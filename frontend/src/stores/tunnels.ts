/**
 * 端口转发运行态 store（与后端 ssh/tunnel.rs、ipc.rs tunnel_start/stop 对应）
 *
 * 数据分两层：
 * - tunnelSnapshots：后端 0x0A 状态帧的全量快照（唯一事实源），按前端会话 id 隔离；
 * - desiredRules：用户「期望运行」的规则 id 集合（内存态，不持久化），Dock 页签
 *   开关的显示依据、断线重连后 restoreOnReconnect 的恢复依据。每次快照帧到达
 *   先用后端 Active 状态校正（syncDesiredFromSnapshot），防止期望集与后端真实
 *   监听漂移（开关显示关闭但后端仍在监听）。
 */
import { reactive, shallowRef, triggerRef } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { Session } from './session'
import { sessions } from './session'
import type { TunnelRule, TunnelStatus } from '../types'
import { savedSettings } from './settings'
import { toast } from '../composables/useToast'
import { isTauri } from '../lib/tauri'

/** 每会话最新隧道状态快照（0x0A 帧整帧替换；关闭会话时清理，断线作废） */
export const tunnelSnapshots = shallowRef(new Map<string, TunnelStatus[]>())

/** 每会话期望运行的规则 id 集合（reactive Map 嵌套 Set 保证开关响应式） */
export const desiredRules = reactive(new Map<string, Set<string>>())

function addDesired(sessionId: string, ruleId: string) {
  let set = desiredRules.get(sessionId)
  if (!set) {
    set = new Set()
    desiredRules.set(sessionId, set)
  }
  set.add(ruleId)
}

function removeDesired(sessionId: string, ruleId: string) {
  desiredRules.get(sessionId)?.delete(ruleId)
}

/** 页签开关显示依据：id 在期望集合内 = 已请求启动（或后端正在运行） */
export function isDesired(sessionId: string, ruleId: string): boolean {
  return desiredRules.get(sessionId)?.has(ruleId) ?? false
}

/** 后端错误前缀即协议（见 ipc.rs tunnel_start 注释）；幂等信号按成功处理 */
const TUNNEL_RUNNING = 'TUNNEL_RUNNING:'

/** 启动一条规则：加入期望集 → invoke tunnel_start。失败保留期望集（用户意图），
 *  仅 toast 提示；TUNNEL_RUNNING: 为幂等成功信号（后端已在运行/重试中），按成功处理。 */
export async function startTunnel(session: Session, rule: TunnelRule): Promise<boolean> {
  if (!isTauri) {
    toast('[dev] 浏览器模式无 Tauri 后端', 'err')
    return false
  }
  if (!session.backendId) return false
  addDesired(session.id, rule.id)
  try {
    // name/enabled 为配置态字段，按契约在 invoke 前剥离，引擎不消费
    await invoke('tunnel_start', {
      sessionId: session.backendId,
      rule: {
        id: rule.id,
        type: rule.type,
        bindHost: rule.bindHost,
        bindPort: rule.bindPort,
        targetHost: rule.targetHost,
        targetPort: rule.targetPort,
      },
      portConflict: savedSettings.tunnelPortConflict,
      dnsResolve: savedSettings.tunnelDnsResolve,
      retryCount: savedSettings.tunnelRetryCount,
    })
    return true
  } catch (e) {
    // 失败不移出期望集：保留用户意图，重连/配置修复后可恢复
    const msg = String(e)
    if (!msg.startsWith(TUNNEL_RUNNING)) {
      toast(`端口转发启动失败：${msg}`, 'err', 4000)
      return false
    }
    return true
  }
}

/** 停止一条规则：移出期望集 → invoke tunnel_stop（后端幂等：规则不存在视为成功） */
export async function stopTunnel(sessionId: string, ruleId: string): Promise<void> {
  removeDesired(sessionId, ruleId)
  if (!isTauri) {
    toast('[dev] 浏览器模式无 Tauri 后端', 'err')
    return
  }
  const backendId = sessions.value.find(x => x.id === sessionId)?.backendId
  if (!backendId) return
  try {
    await invoke('tunnel_stop', { sessionId: backendId, ruleId })
  } catch (e) {
    toast(`端口转发停止失败：${String(e)}`, 'err', 4000)
  }
}

/** Dock 页签开关的直达函数：on → startTunnel；off → stopTunnel */
export async function toggleDesired(sessionId: string, ruleId: string, on: boolean) {
  const s = sessions.value.find(x => x.id === sessionId)
  if (!s) return
  if (!on) {
    await stopTunnel(sessionId, ruleId)
    return
  }
  const rule = s.host.tunnels?.find(r => r.id === ruleId)
  if (!rule) return
  await startTunnel(s, rule)
}

/** 首次连接进入 online 后调用（session.ts connectBackend）：清掉旧快照与期望集
 *  （新后端会话无任何监听），tunnelAutoStart 开启时批量启动 enabled=true 规则。
 *  单条失败（含 TUNNEL_LIMIT/端口冲突）只 toast 并继续后续规则，不回滚连接状态。 */
export async function startSavedOnConnect(session: Session) {
  tunnelSnapshots.value.delete(session.id)
  desiredRules.delete(session.id)
  triggerRef(tunnelSnapshots)
  if (!savedSettings.tunnelAutoStart || !session.host.tunnels?.length) return
  for (const rule of session.host.tunnels.filter(r => r.enabled)) {
    await startTunnel(session, rule)
  }
}

/** 重连成功进入 online 后调用（自动/手动重连统一经 connectBackend）：
 *  快照整体作废（旧后端会话监听已随断线全部消失）；tunnelReconnect 开启时按
 *  期望集批量重启，否则清空期望集避免开关假亮（后端实际未监听）。
 *  重启失败保留期望集（用户意图），但快照会反映错误状态（开关不亮）。 */
export async function restoreOnReconnect(session: Session) {
  tunnelSnapshots.value.delete(session.id)
  triggerRef(tunnelSnapshots)
  const ids = desiredRules.get(session.id)
  if (!savedSettings.tunnelReconnect || !ids?.size) {
    desiredRules.delete(session.id)
    return
  }
  const rules = (session.host.tunnels ?? []).filter(r => ids.has(r.id))
  for (const rule of rules) {
    await startTunnel(session, rule)
  }
}

/** 每次收到 0x0A 快照帧都执行：后端 Active 状态同步进期望集（幂等校正）。
 *  Stopped/Error 不动期望集——停止是用户意图，Error 留给用户确认后重开。 */
export function syncDesiredFromSnapshot(sessionId: string, statuses: TunnelStatus[]) {
  for (const st of statuses) {
    if (st.state === 'active') addDesired(sessionId, st.id)
  }
}

/** session.ts 0x0A 分支处理：先校正期望集，再整帧替换快照 */
export function handleTunnelFrame(sessionId: string, payload: Uint8Array) {
  try {
    const o = JSON.parse(new TextDecoder().decode(payload)) as { tunnels?: unknown }
    const statuses = Array.isArray(o.tunnels) ? (o.tunnels as TunnelStatus[]) : []
    syncDesiredFromSnapshot(sessionId, statuses)
    tunnelSnapshots.value.set(sessionId, statuses)
    triggerRef(tunnelSnapshots)
  } catch {
    // 单帧解析失败不影响终端主流程
  }
}

/** 会话移除：清理快照与期望集（session.ts closeSession 调用） */
export function clearSessionTunnels(sessionId: string) {
  tunnelSnapshots.value.delete(sessionId)
  desiredRules.delete(sessionId)
  triggerRef(tunnelSnapshots)
}
