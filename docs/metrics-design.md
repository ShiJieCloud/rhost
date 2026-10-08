# 右侧栏主机指标采集设计方案

> 状态：P0–P4 已全部落地（2026-10-03）；100 会话压测、断连清理、Darwin 式无 procfs 降级均已 e2e/单测验证
> 日期：2026-10-03
> 范围：工作台右侧栏（Inspector）全部远程主机指标的采集、传输、渲染与生命周期管理
> 关联代码：`src-tauri/src/ssh/{session,frame,manager}.rs`、`src-tauri/src/motd/mod.rs`、`frontend/src/components/wb/Inspector.vue`、`frontend/src/stores/session.ts`

---

## 1. 设计目标与硬约束

| 目标 | 量化口径 |
|---|---|
| 高性能 | 无 GPU 时单轮远端执行 P95 < 50ms；帧 payload < 4KB；本地解析 < 1ms |
| 低消耗 | **只有「当前活动 Tab + Inspector 可见」的会话才采集**；侧栏隐藏/窗口最小化时零 SSH 流量；Rhost 自身 CPU 增量 < 1%/活动会话 |
| 安全 | 零命令拼接、零远端落盘、零 sudo、零常驻进程；argv 不泄露；断连零残留 |
| 高并发 | 100+ 会话不雪崩：每会话单飞 + 全局信号量闸门 + 启动错峰 + watch 丢帧合并 |

必须遵守的既有项目约束：

- russh 原生 tokio 异步，**禁止** `spawn_blocking` 包装 SSH 读写；
- 每会话独立 `CancellationToken`，关闭/异常必须能取消全部后台任务；
- 终端流走每会话独立的 Tauri `Channel<Vec<u8>>` 自定义二进制帧（`1 字节类型 + 4 字节大端长度 + payload`），禁止用 Emitter/Event 传流；
- **严禁**解析/修改/拦截 PTY 字节流；指标采集必须走独立 exec 子通道，与终端通道物理隔离；
- 采集失败/超时/命令缺失一律静默降级，绝不影响 SSH 主连接（MOTD 采集已确立此原则）；
- SessionManager 使用 `Arc<RwLock<HashMap<session_id, Arc<SshSession>>>>`，uuid v4 生成会话 ID。

## 2. 指标分层与数据源

按变化频率分三层，决定采集节奏：

| 层 | 区块 | 频率 | 数据源（均为普通用户可读，Linux/procfs） |
|---|---|---|---|
| 静态 | 系统信息 | 连接时 **1 次** | `/etc/os-release`（PRETTY_NAME）、`uname -srm`、`nproc`、`/proc/cpuinfo`（model name 去重）、`/proc/uptime`、`date +%z %Z`、默认路由 iface/IP（MOTD 已有）、GPU 能力探测 |
| 动态 | CPU/内存/Swap、GPU、网络、进程 | 2–3s | `/proc/stat`（两次差值）、`/proc/loadavg`、`/proc/meminfo`（MemTotal/**MemAvailable**/SwapTotal/SwapFree）、`/proc/net/dev`（字节差值算速率）、遍历 `/proc/[0-9]*/stat`、`nvidia-smi --query-gpu`（AMD 回退 `rocm-smi`） |
| 低频 | 磁盘 | 30s（每 10 轮夹带一次） | `df -Pk`（POSIX、KB 精确值），过滤 tmpfs/devtmpfs/伪 fs，保留 overlay |

口径要点：

- **内存占用**用 `MemTotal - MemAvailable`（可回收 cache 不算占用），比 `free` 的 used 列更准确；需要精确字节值而非 `free -h` 的人类可读值。
- **CPU 使用率**必须两次采样差值：`1 - Δidle / Δ(total)`，单次快照无意义。
- **进程 CPU%** 用 `/proc/pid/stat` 的 utime+stime（字段 14/15）**两次采样差值 / 实际间隔** 计算。`ps aux` 的 CPU% 是进程生命周期均值，不可用；busybox `ps -o/--sort` 支持残缺，直接读 /proc 最稳（Alpine/嵌入式通用）。
- **进程名**取 `/proc/pid/stat` 第 2 字段 comm（内核给的可执行名），用「最后一个 `)`」切分（comm 可含空格/括号，是经典解析坑）；**不读 cmdline**——见安全章节。RSS = 字段 24 × page size。
- **网络速率** = `/proc/net/dev` 收发字节差 / 两次采样墙钟差。
- **磁盘**用 `df -Pk`（POSIX 标准 `-P` 单行记录、`-k` 输出 1K 块精确值）；同设备多挂载点去重；overlay 必须保留（容器根分区）。
- **无 swap**（SwapTotal=0）时前端隐藏 Swap 进度条，而非显示 0%。

### 2.1 指标对照表（相对当前 Inspector 实现）

下表为接入真实采集后侧栏呈现的完整指标清单；「现状」列标注该指标在当前 Inspector 中的来源（界面缺失 / mock 或硬编码）。

| 区块 | 指标 | 数据源 / 采集口径 | 当前 Inspector 现状 |
|---|---|---|---|
| 系统信息 | 发行版 | `/etc/os-release` PRETTY_NAME | 取 `host.os` 配置值 |
| 系统信息 | 内核 / 架构 | `uname -srm` | 硬编码 |
| 系统信息 | 系统运行时长 | `uptime` 解析 | 硬编码「63 天 4 小时」 |
| 系统信息 | 时区 | `date +%z %Z` | 硬编码 |
| 系统信息 | CPU 型号（如 Intel Xeon Platinum 8469） | `/proc/cpuinfo` model name 去重 | 界面缺失 |
| 系统信息 | 服务器 IP | `ip route get 1.1.1.1`，回退 `hostname -I`（MOTD 已采） | 界面缺失 |
| 连接信息 | 当前用户（本连接登录账号） | **本地 `host.user`，无需远端采集**（SSH 认证即实际账号），与「认证」成组 | 原拟采 utmp 登录用户数，已弃用：容器无 `/var/run/utmp` 恒为 0、且在 PTY 前采集永远少算自己 |
| 资源监控 | CPU 利用率 / 核数 | `/proc/stat` 两次差值 / `nproc` | 随机数 + 硬编码 32 核 |
| 资源监控 | 1/5/15 分钟负载 | `/proc/loadavg` | 仅一个硬编码的 1 分钟值 |
| 资源监控 | 内存 used/total/% | `/proc/meminfo`，口径 `MemTotal - MemAvailable`，精确字节 | mock（used 列口径） |
| 资源监控 | Swap used/total/% | SwapTotal/SwapFree；SwapTotal=0 时隐藏 | 硬编码 128 MB / 2 GB |
| 网络 | 真实网卡名 + IPv4 | 默认路由 dev/src 字段 | 标题写死 eth0 |
| 网络 | 上传/下载速率 | `/proc/net/dev` 字节差值 ÷ 真实采样间隔（sparkline 保留） | 随机数 |
| 网络 | 累计收发字节 rxBytes/txBytes | `/proc/net/dev` | 界面缺失 |
| 网络 | 丢包/错误累计计数 | `/proc/net/dev` drop/error 列 | 显示无法采集的链路「丢包率 0.02%」 |
| 磁盘 | 全挂载点列表 + 可用容量 Avail | `df -Pk` 过滤 tmpfs/devtmpfs 等伪 fs，保留 overlay | 4 条写死数据 |
| 磁盘 | 文件系统类型 fs（ext4/xfs/overlay） | `df -Pk` + 挂载枚举 | 界面缺失 |
| 进程 | 进程总数 | `/proc/[0-9]*` 计数 | 显示 mock 数组长度 |
| 进程 | PID / 命令名 / CPU% | 遍历 /proc，utime+stime 两次差值，按 CPU 排序取 Top N；命令名只取内核 comm（不读 cmdline 防泄露密钥） | mock 数组 |
| 进程 | 进程 RSS 实际字节 | `/proc/pid/stat` 字段 24 × page size | mem 列为含义模糊的 mock 百分比 |
| GPU | 厂商能力探测 gpuVendor | 静态阶段探测一次（nvidia；后续 AMD rocm-smi） | 界面缺失 |
| GPU | 利用率 / 显存 / 温度 / 功耗 | `nvidia-smi --query-gpu`，字段与现有 `GpuItem` 对齐；无 GPU 自动隐藏 | 已建 UI，mock 数据 |

本期明确不新增：

- **链路丢包率 / RTT 持续探测**：主机自身采不到「到本机链路」的丢包率；RTT 沿用建连时单次值（持续刷新需走 SSH keepalive global request 计时，列入后续增强）。
- **GPU 进程列表**（哪张卡上跑着哪些 PID）：nvidia-smi 可取但解析重、敏感度高，列为二期可选项。

## 3. 总体架构

```
前端                          后端(Rust)                      远端
─────                         ──────────                      ────
Inspector 可见且活动
  │ invoke start_metrics(sid, {intervalMs})
  │ 每 5s heartbeat ──────────► Collector::start
  │                            ┌─ tokio interval(+jitter)
  │                            │  ① 单飞检查(在飞则跳过本轮)
  │                            │  ② Semaphore 全局取牌
  │                            │  ③ 开 1 个 exec 通道 ──────► sh 执行打包脚本(≤3s 超时)
  │                            │  ④ marker 分段文本解析       cat /proc/* ...
  │                            │  ⑤ 与 prev 快照做差值 ◄────── 输出
  │                            │  ⑥ serde_json → Frame 0x06
  │                            ▼
  │            watch::Sender（只保留最新值，自动合帧）
  │                            │ forward task
  │  Channel 二进制帧 ◄─────────┘ （背压时 await，watch 已合并中间帧）
  │ shallowRef 整体替换 → 渲染
侧栏隐藏/断开/autoRefresh 关
  └ invoke stop_metrics 或心跳超时(9s) → cancel collector，远端 sh 随 channel close 收 SIGHUP 退出
```

**明确不采用「远端常驻采样脚本 + sleep 循环」方案**：会在远端留下用户 `ps` 可见的常驻进程、依赖 sh 兼容、断连 kill 清理复杂、安全审计敏感。按需 exec 单次退出，sshd 自动回收（每轮一个 channel 的开销在毫秒级，可测），是低消耗与安全的更优解。

## 4. 后端设计

### 4.1 保留 SSH handle（前置改造）

当前 `SshSession::connect` 末尾 `drop(handle)`（`src-tauri/src/ssh/session.rs`），PTY 之外的 exec 能力随之释放。改造：

- PTY 打开前 `handle.clone()`（russh Handle 内部为 Arc，clone 廉价）；
- 以 `Arc<tokio::sync::Mutex<client::Handle<ClientHandler>>>` 存入 `SshSession`；
- 新增方法：

```rust
/// 在独立 exec 子通道执行采集脚本，返回 stdout+stderr 合并文本。
/// 与 PTY 通道互不影响；调用方负责超时与取消。
pub async fn exec_collect(&self, script: &str, timeout: Duration)
    -> Result<String, SshError>
```

内部 `lock().await.channel_open_session()` + `exec("/bin/sh -c …")` + drain。每会话动态阶段通道占用恒为 PTY×1 + exec×1 = 2，远低于 sshd `MaxSessions=10`。Mutex 仅串行化「开通道+下发」的短临界区，drain 在通道对象上进行，不长期持锁。

### 4.2 采集脚本：单通道打包 + marker 分段

一轮采集只开 **1 个 exec**。脚本为代码内置常量（`metrics/script.rs`），远端只做 cat/遍历，解析全部在 Rust 侧（可单测、远端 CPU 最小）：

```sh
/bin/sh -c '
echo @@STAT@@;  head -n1 /proc/stat
echo @@LOAD@@;  cat /proc/loadavg
echo @@MEM@@;   cat /proc/meminfo
echo @@NET@@;   cat /proc/net/dev
echo @@PROC@@;  for p in /proc/[0-9]*; do read -r a b rest < "$p/stat" 2>/dev/null && echo "$p $b $rest"; done
echo @@GPU@@;   nvidia-smi --query-gpu=index,name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw,power.limit --format=csv,noheader,nounits 2>/dev/null
echo @@DISK@@;  df -Pk 2>/dev/null
echo @@END@@'
```

- POSIX sh + `/proc/[0-9]*` glob，busybox/Alpine 通用；每条命令体外层统一 `/bin/sh -c`，兼容 fish/tcsh 用户 shell（沿用 MOTD 既有做法）。
- `@@DISK@@` 段按轮次计数夹带（脚本接受参数控制是否跑 df），默认不跑。
- `nvidia-smi` 启动开销 100–300ms 是主要成本：静态阶段探测一次，无 GPU/无命令的主机**后续轮次不带 GPU 段**。
- 任意一段失败只导致该段缺失（marker 缺失即该区块 N/A），其余照常；总超时 3s 覆盖慢主机。
- 单引号转义复用 MOTD 的 `body.replace('\'', "'\\''")` 包装。

### 4.3 MetricsCollector 状态机（每会话一个）

```rust
struct MetricsCollector {
    cancel: CancellationToken,
    handle: Arc<Mutex<client::Handle<ClientHandler>>>,
    frame_tx: mpsc::Sender<Vec<u8>>,       // clone 会话帧队列，直接投递完整 0x05/0x06 帧
    prev: Mutex<Option<PrevSample>>,       // CPU ticks、各 iface 字节、pid→(utime+stime, ts)
    interval: Duration,
    last_heartbeat: AtomicInstant,
    disk_tick: AtomicU32,
}
```

- **单飞**：采集在同一个 loop task 内串行，上一轮未返回不发下一轮；慢轮天然跳过，不积压。
- **错峰**：start 时 `sleep(random 0–300ms)`，避免大量会话同时重连形成波峰。
- **超时**：每轮 `tokio::time::timeout(3s)`；超时即弃本轮并主动 `channel.close()`。
- **真实间隔**：速率 = 差值 / 两次采样墙钟差，前端后台 Tab 节流也不影响正确性。
- **合帧**：结果经 `tokio::sync::watch` 交给 forward task，只在 `changed()` 时发帧；IPC 阻塞期间的中间快照自动丢弃——指标是「最新值语义」，永不需要排队。
- **回收**：会话 disconnect → `cancel.cancel()` → collector 与所有 wait 立即终止，`prev` 随 drop 释放，无全局泄漏表。

### 4.4 三重生命周期保险

1. 显式 `stop_metrics`（侧栏隐藏 / autoRefresh 关 / 断开）；
2. **心跳续约**：前端每 5s 发 heartbeat，后端 9s 未收到自动停（防 WebView 刷新/崩溃后空转）；
3. collector 全部 `select!` 在会话 `CancellationToken` 上，断连必停。

全局兜底闸门：`once_cell` 全局 `Semaphore::new(24)` 限制同时在飞的 exec，配合前端门控（正常只有 1 个活动会话在采），属极端情况下的保险丝。

连续 exec 失败 3 轮时，该会话自动降级为 10s 慢轮询并继续重试，避免在故障主机上高频空转。

### 4.5 帧协议扩展（`ssh/frame.rs`）

新增两个类型，沿用 `[u8 type][u32 BE len][payload]`：

- `HostInfo = 0x05`：连接时 1 帧，在 MOTD 首帧之后、PTY Data 之前发出（mpsc FIFO 保序）；
- `Metrics = 0x06`：周期帧。payload 均为紧凑 JSON（传数值，不传格式化字符串）：

```jsonc
// 0x05 HostInfo
{
  "os": "Linux",
  "distro": "Ubuntu 22.04.4 LTS",
  "kernel": "5.15.0-113-generic",
  "arch": "x86_64",
  "cpuModel": "Intel Xeon Platinum 8469",
  "cores": 32,
  "timezone": "UTC+00:00",
  "iface": "eth0",
  "ip": "10.20.3.41",
  "procfs": true,
  "gpuVendor": "nvidia"          // "none" 或字段缺省时前端隐藏 GPU 区块
}
```

```jsonc
// 0x06 Metrics
{
  "seq": 42,
  "ts": 1759490000123,
  "cpu": { "util": 38.2, "load": [0.42, 0.38, 0.31] },
  "mem": { "total": 8372371456, "avail": 5012345600,
           "swapTotal": 2147483648, "swapFree": 2013265920 },
  "net": { "rxRate": 4812345, "txRate": 1258304, "rxBytes": 0, "txBytes": 0 },
  "disks": [
    { "mount": "/", "fs": "ext4", "total": 83783942144, "used": 30491234304, "pct": 36 }
  ],
  "procs": {
    "total": 187,
    "top": [ { "pid": 1204, "name": "nginx", "cpu": 12.4, "rss": 32546816 } ]
  },
  "gpus": [
    { "index": 0, "util": 34, "memUsed": 8413184, "memTotal": 25769803776,
      "temp": 56, "power": 182, "powerLimit": 450 }
  ]
}
```

Tauri 命令（注册于 `lib.rs` 的 `invoke_handler`，经 SessionManager 按 sid 取 `Arc<SshSession>`，复制 Arc 后释放读锁，不持锁 await）：

| 命令 | 作用 |
|---|---|
| `start_metrics(session_id, opts)` | 启动/重置该会话 collector（幂等），opts 含 intervalMs |
| `stop_metrics(session_id)` | 停止并释放 collector |
| `metrics_heartbeat(session_id)` | 续约 last_heartbeat |

## 5. 前端设计

### 5.1 Store 按会话隔离（修现状缺陷）

现有 `metrics` 是全局单例 reactive，**多 Tab 切换会串数据**。改为：

```ts
const hostInfoMap = new Map<string, HostInfo>()              // 静态，缓存到断连
const metricsMap = shallowRef(new Map<string, Metrics>())    // 动态快照，整体替换
const activeMetrics = computed(() => metricsMap.value.get(activeSessionId.value))
```

- `shallowRef` 整体替换快照，避免每 2s 对数百个字段建响应式代理；
- sparkline 维持定长 28 点环形数组；
- 切 Tab 先显示该会话上次快照（不闪 N/A），断线冻结最后一帧；
- 帧解析在现有 Channel 二进制帧分发处增加 `0x05 / 0x06` case；
- 现有 `GpuItem` mock 结构字段已与 0x06 的 `gpus[]` 对齐，接通时仅替换数据源。

### 5.2 可见性门控（零消耗的关键）

以下条件全部满足才 start，任一变化即重新评估：

- `activeSession.state === 'online'`；
- `inspectorVisible === true`（侧栏折叠不采）；
- Inspector 滚动容器在视口内（IntersectionObserver，沿用状态栏内存监控的成熟模式）；
- `document.visibilityState === 'visible'`（最小化/切别的 App 暂停，恢复时立即补采一帧）；
- `autoRefresh === true`。

**后台 Tab 的会话永不采集**——这是高并发下消耗不随 Tab 数线性增长的根本。轮询间隔进全局设置（沿用 settings.ts 数据驱动面板模式），默认 3s，范围 1–10s。

## 6. 安全设计（清单）

1. **零注入面**：脚本是固定常量，SessionConfig 任何字段（host/username/备注）都不拼接进命令；`$USER` 等变量一律在远端展开。
2. **零新增攻击面**：复用已认证加密 SSH 通道，不开新端口、不存新凭据、不需要 sudo；所有数据源（procfs、df、nvidia-smi）均为普通用户只读。
3. **零远端写入/残留**：脚本经 exec argv 传入（不像 `.ri-xxx` 需要落 /tmp），无临时文件；单次执行退出，sshd 回收；断连 channel close → SIGHUP/SIGPIPE，远端零残留（需 e2e 验证 `pgrep` 为空）。
4. **防 argv 泄密**：进程名只取内核 comm，不读 `/proc/pid/cmdline`（`mysql -pxxx` 一类密钥不会出现在侧栏）。
5. **渲染安全**：Vue <code v-pre>{{ }}</code> 插值自动转义，远端字符串（发行版名/挂载点/进程名）一律不使用 v-html。
6. **信息出口**：指标只含系统统计；日志仅记录采集计数与耗时，不打 payload；错误不回显远端原始敏感内容。
7. **自限反冲**：单飞 + 信号量 + 3s 超时 + 心跳 + CancellationToken 五重边界，慢主机/异常 sshd 不会拖垮 Rhost 或耗尽 fd。

## 7. 降级兼容矩阵

| 场景 | 行为 |
|---|---|
| 无 /proc（macOS/BSD 主机） | HostInfo 标 `procfs:false`，动态区块显示「暂不支持」（二期可补 sysctl/ps 通道） |
| busybox/Alpine | glob + read + head/cat 全可用；无 nvidia-smi 则 gpus=[]，GPU 区块隐藏（已做 v-if） |
| 无 swap | swapTotal=0，Swap 条隐藏 |
| df 段/GPU 段缺失 | 磁盘保留上一次数据；GPU 区块隐藏 |
| 首轮无 prev 快照 | CPU/网络速率/进程 CPU 显示 0 或「—」，第二轮起正常 |
| exec 连续失败 3 轮 | 自动降级 10s 慢轮询并继续重试 |
| WebView 崩溃/刷新 | 9s 心跳超时，后端自动停止采集 |

## 8. 落地阶段

| 阶段 | 内容 |
|---|---|
| P0 | `session.rs` 保留 handle；新建 `src-tauri/src/metrics/`（mod.rs / script.rs / parser.rs）；frame.rs 增加 0x05；系统信息区块先去 mock |
| P1 | Collector + watch + 0x06 帧 + 三个 Tauri 命令；CPU/内存/网络/磁盘/进程全链路；前端 store 会话隔离与可见性门控 |
| P2 | nvidia-smi/rocm-smi 采集与能力探测，接通已完成的 GPU 区块 |
| P3 | 设置项、心跳、全局信号量、Darwin 降级、失败退避 |
| P4 | parser 单测（comm 含空格/括号、多网卡、overlay）；docker sshd（Debian/Alpine）e2e：首帧顺序、断连残留=0；100 会话模拟压测验证信号量与错峰 |

> 落地备注（2026-10-03）：P0–P4 全部完成。间隔设置做成 1/2/3/5/10s 下拉（默认 3，热生效）；Darwin 未写专用代码，由「段缺失即降级」天然覆盖并补单测固化；100 会话压测用例 `ssh_metrics_100_sessions_gated` 标 `#[ignore]`（连接风暴会触发 sshd MaxStartups、干扰并行用例），以 `cargo test --test ssh_e2e -- --ignored --test-threads=1` 显式运行。

### 验收红线

- 无 GPU 单轮 P95 < 50ms；
- 0x06 payload < 4KB；
- 活动会话 Rhost CPU 增量 < 1%；
- 断连后远端残留采集进程为 0；
- 侧栏隐藏时该会话 SSH 抓包无 exec 流量；
- 100 会话场景下启动峰值 exec 并发 ≤ 24，无任务积压。

> 实测（busybox 容器 rhost-test-sshd，aarch64，100 会话）：
> 启动峰值 exec 并发 **= 24**（打满信号量、闸门真实削峰）；100/100 会话在 **1.31s** 内全部收到首帧（无饥饿/积压）；
> 0x06 最大 payload **733B**（< 4KB）；100 会话全部 shutdown 后在飞 exec 归零；
> 单会话未 stop 直接 shutdown 后不再产生任何 0x06 帧（`ssh_metrics_cancelled_on_shutdown`）。
> 「侧栏隐藏零 exec 流量」由前端门控单飞状态机保证（target=null 即 stop），未做抓包级自动化。

## 9. 测试资产

沿用项目已有 docker sshd 容器：

- `docker/debian-sshd`（rhost-test-sshd-debian，127.0.0.1:2223，test/rhost123）：完整 procfs + lastlog；
- 建议新增 Alpine 容器：验证 busybox/无 GPU/无 lastlog 降级路径；
- e2e 入口：`src-tauri/tests/ssh_e2e.rs`，新增采集帧序、断连清理用例。
