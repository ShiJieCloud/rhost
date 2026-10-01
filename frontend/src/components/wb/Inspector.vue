<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { toast } from '../../composables/useToast'
import {
  activeHost, activeSession, barClass, connected, metrics, openDock, reconnectTick,
} from '../../stores/session'

const uptime = ref('00:00:00')
let timer: ReturnType<typeof setInterval> | null = null

const loginTime = computed(() => {
  const s = activeSession.value
  if (!s) return '—'
  return new Date(s.startedAt).toLocaleTimeString('zh-CN', { hour12: false })
})

function pad(n: number) {
  return String(n).padStart(2, '0')
}

onMounted(() => {
  timer = setInterval(() => {
    const s = activeSession.value
    if (s && s.state === 'online') {
      const t = Math.floor((Date.now() - s.startedAt) / 1000)
      uptime.value = `${pad(Math.floor(t / 3600))}:${pad(Math.floor(t / 60) % 60)}:${pad(t % 60)}`
    }
  }, 1000)
})
onUnmounted(() => {
  if (timer) clearInterval(timer)
})

function onReconnect() {
  if (!activeSession.value) { toast('没有活动会话', 'warn'); return }
  reconnectTick.value++
}
function onDisconnect() {
  if (!connected.value) { toast('当前没有活动会话', 'warn'); return }
  toast('已请求断开连接，请在终端执行 exit', 'info', 2400)
}

/* 会话五态：ok 已连接 / info 连接中 / warn 重连中 / err 已断开 / idle 未连接 */
const stateLabel = computed(() => {
  const s = activeSession.value
  if (!s) return '未连接'
  if (s.state === 'online') return '已连接'
  if (s.state === 'connecting') return '连接中…'
  if (s.state === 'reconnecting') return '重连中…'
  if (s.state === 'idle') return '未连接'
  return '已断开'
})
const stateCls = computed(() => {
  const s = activeSession.value
  if (!s) return 'idle'
  if (s.state === 'online') return 'ok'
  if (s.state === 'connecting') return 'info'
  if (s.state === 'reconnecting') return 'warn'
  if (s.state === 'idle') return 'idle'
  return 'err'
})
</script>

<template>
  <aside class="inspector">
    <div class="ins-title">概览</div>
    <div class="card" :style="{ '--accent': connected ? '#3ddc84' : '#f7768e' }">
      <div class="conn-top">
        <div class="conn-ico">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"
               stroke-linecap="round" stroke-linejoin="round">
            <rect x="2" y="3" width="20" height="6" rx="2"></rect>
            <rect x="2" y="11" width="20" height="6" rx="2"></rect>
            <line x1="6" y1="6" x2="6.01" y2="6"></line>
            <line x1="6" y1="14" x2="6.01" y2="14"></line>
          </svg>
        </div>
        <div style="min-width:0">
          <div class="conn-name">{{ activeSession?.host.id ?? '—' }}</div>
          <div class="conn-addr">
            {{ activeSession ? `${activeSession.host.user}@${activeSession.host.ip}:${activeSession.host.port}` : '' }}
          </div>
        </div>
      </div>

      <div class="kv">
        <span class="k">状态</span>
        <span class="v" :class="stateCls">{{ stateLabel }}</span>
      </div>
      <div class="kv"><span class="k">延迟</span><span class="v">{{ activeSession?.host.lat ? activeSession.host.lat + ' ms' : '—' }}</span></div>
      <div class="kv"><span class="k">会话时长</span><span class="v">{{ uptime }}</span></div>
    </div>

    <div class="ins-title">系统信息</div>
    <div class="card">
      <div class="sysinfo">
        <template v-if="activeHost">
          <span class="k">主机名</span><span class="v ok">{{ activeHost.id }}</span>
          <span class="k">系统</span><span class="v">{{ activeHost.os }}</span>
          <span class="k">内核</span><span class="v">5.15.0-91-generic</span>
          <span class="k">架构</span><span class="v">x86_64</span>
          <span class="k">运行时长</span><span class="v">42 天 6 小时</span>
          <span class="k">负载均值</span><span class="v">0.08 / 0.12 / 0.09</span>
          <span class="k">时区</span><span class="v">Asia/Shanghai (CST)</span>
        </template>
        <template v-else>
          <span class="k">状态</span><span class="v">未连接</span>
        </template>
      </div>
    </div>

    <div class="ins-title">资源监控</div>
    <div class="card" style="--accent:#4fd6e0">
      <div class="ins-kpi-grid">
        <div class="ins-kpi">
          <div class="k">CPU</div>
          <div class="v">{{ connected ? metrics.cpu.toFixed(0) : '—' }}<small>%</small></div>
          <div class="bar"><i :class="barClass(metrics.cpu)" :style="{ width: (connected ? metrics.cpu : 0) + '%' }"></i></div>
        </div>
        <div class="ins-kpi">
          <div class="k">内存</div>
          <div class="v">{{ connected ? metrics.mem.toFixed(0) : '—' }}<small>%</small></div>
          <div class="bar"><i class="bar-blue" :style="{ width: (connected ? metrics.mem : 0) + '%' }"></i></div>
        </div>
        <div class="ins-kpi">
          <div class="k">磁盘 /</div>
          <div class="v">{{ connected ? metrics.disk.toFixed(0) : '—' }}<small>%</small></div>
          <div class="bar"><i class="bar-yellow" :style="{ width: (connected ? metrics.disk : 0) + '%' }"></i></div>
        </div>
        <div class="ins-kpi">
          <div class="k">网络</div>
          <div class="v">{{ connected ? ((metrics.tx + metrics.rx) / 6).toFixed(0) : '—' }}<small>KB/s</small></div>
          <div class="bar"><i class="bar-cyan" :style="{ width: (connected ? Math.min(100, (metrics.tx + metrics.rx) / 6) : 0) + '%' }"></i></div>
        </div>
      </div>

      <div class="ins-divider-wrap"><div class="ins-divider"></div></div>

      <div class="ins-proc-head"><span>进程</span><span style="text-align:right">CPU</span><span style="text-align:right">MEM</span></div>
      <div class="ins-proc-list">
        <div v-for="p in metrics.procs" :key="p.pid" class="ins-proc">
          <span class="cmd"><span class="pid">{{ p.pid }}</span>{{ p.cmd }}</span>
          <span class="num" style="color:var(--yellow)">{{ p.cpu.toFixed(1) }}</span>
          <span class="num">{{ p.mem.toFixed(1) }}</span>
        </div>
      </div>

      <div class="ins-divider-wrap"><div class="ins-divider"></div></div>

      <div class="net-row">
        <span class="k">
          <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor"
               stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
            <line x1="12" y1="19" x2="12" y2="5"></line>
            <polyline points="5 12 12 5 19 12"></polyline>
          </svg>
          上行
        </span>
        <span class="v">{{ metrics.tx.toFixed(1) }} KB/s</span>
      </div>
      <div class="net-row">
        <span class="k">
          <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor"
               stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
            <line x1="12" y1="5" x2="12" y2="19"></line>
            <polyline points="19 12 12 19 5 12"></polyline>
          </svg>
          下行
        </span>
        <span class="v">{{ metrics.rx.toFixed(1) }} KB/s</span>
      </div>
    </div>

    <div class="ins-title">会话信息</div>
    <div class="card" style="--accent:#7aa2f7">
      <div class="kv"><span class="k">终端</span><span class="v">xterm-256color</span></div>
      <div class="kv"><span class="k">编码</span><span class="v">UTF-8</span></div>
      <div class="kv"><span class="k">认证方式</span><span class="v">ssh-ed25519</span></div>
      <div class="kv"><span class="k">加密算法</span><span class="v">AES-256-GCM</span></div>
      <div class="kv"><span class="k">密钥交换</span><span class="v">curve25519-sha256</span></div>
      <div class="kv"><span class="k">登录时间</span><span class="v">{{ loginTime }}</span></div>
    </div>

    <div class="ins-title">快捷操作</div>
    <div class="actions">
      <button class="action" @click="onReconnect">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round">
          <polyline points="1 4 1 10 7 10"></polyline>
          <path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10"></path>
        </svg>
        重新连接
      </button>
      <button class="action" @click="openDock(); toast('已打开 SFTP 文件管理', 'info', 1600)">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round">
          <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
        </svg>
        打开 SFTP 文件管理
      </button>
      <button class="action" @click="toast('正在打开端口转发配置…', 'info')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round">
          <path d="M4 12h16M4 12l4-4M4 12l4 4M20 12l-4-4M20 12l-4 4"></path>
        </svg>
        端口转发
      </button>
      <button class="action" @click="toast('正在打开主机配置编辑页…', 'info')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round">
          <path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"></path>
          <path d="M18.5 2.5a2.12 2.12 0 0 1 3 3L12 15l-4 1 1-4z"></path>
        </svg>
        编辑主机配置
      </button>
      <button class="action danger" @click="onDisconnect">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round">
          <path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"></path>
          <polyline points="16 17 21 12 16 7"></polyline>
          <line x1="21" y1="12" x2="9" y2="12"></line>
        </svg>
        断开连接
      </button>
    </div>
  </aside>
</template>

<style scoped>
.ins-divider-wrap{margin:10px 0}
.ins-divider{height:1px;background:var(--border-soft)}
.ins-kpi-grid{display:grid;grid-template-columns:1fr 1fr;gap:8px}
.ins-kpi{
  background:#0c121a;border:1px solid var(--border);
  border-radius:8px;padding:8px 10px;min-width:0;
}
.ins-kpi .k{
  font-size:9px;letter-spacing:1px;text-transform:uppercase;
  color:var(--muted-2);white-space:nowrap;
}
.ins-kpi .v{
  font-size:16px;font-weight:600;color:#fff;
  margin:4px 0 7px;display:flex;align-items:baseline;gap:3px;
}
.ins-kpi .v small{font-size:9px;color:var(--muted);font-weight:400}
.ins-kpi .bar{height:3px;border-radius:2px;background:#161f2a;overflow:hidden}
.ins-kpi .bar i{display:block;height:100%;border-radius:2px;transition:width .7s cubic-bezier(.4,0,.2,1)}
.ins-proc-head,.ins-proc{
  display:grid;grid-template-columns:1fr 40px 40px;gap:6px;
  font-size:10px;padding:4px 0;align-items:center;
}
.ins-proc-head{
  color:var(--muted-2);font-size:9px;
  text-transform:uppercase;letter-spacing:.5px;
}
.ins-proc{border-radius:5px}
.ins-proc:hover{background:var(--hover)}
.ins-proc .cmd{
  color:var(--text);white-space:nowrap;overflow:hidden;text-overflow:ellipsis;
  display:flex;gap:6px;min-width:0;
}
.ins-proc .cmd .pid{color:var(--muted-2);flex-shrink:0}
.ins-proc .num{text-align:right;color:var(--cyan)}
.ins-proc-list{margin:2px 0 2px}
</style>
