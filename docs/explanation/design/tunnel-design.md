# SSH 端口转发设计方案

> description: 本地转发 -L、远程转发 -R、动态转发 SOCKS5 -D 的引擎、持久化、状态协议与会话内管理界面设计
>
> created: 2026-10-07 18:37:33
>
> updated: 2026-10-07 23:22:03
>
> author: [sjzhao](https://github.com/ShiJieCloud/rhost)

---

章节目录：1. 设计目标与硬约束 · 2. 既有项目约束 · 3. 现状盘点与差距 · 4. 目标与非目标 · 5. 总体架构 · 6. 详细设计（规则模型 / 三模式引擎 / 状态帧 / IPC / 前端 / 重连 / 设置矩阵） · 7. 安全与降级 · 8. 测试与验证 · 9. 未决问题

## 1. 设计目标与硬约束

> scope: 本文覆盖 SSH 端口转发三种模式——本地转发（`-L`）、远程转发（`-R`）、动态转发（`-D`，本地 SOCKS5 代理）——的后端转发引擎、规则持久化链路、前后端状态协议、会话内管理 UI、设置项消费与断线重连联动。
> 边界：不覆盖跳板机（ProxyJump）、HTTP/SOCKS5 **外连代理**（连接 SSH 服务器本身时走的代理）、X11 转发、Unix domain socket（streamlocal）转发。
> 术语约定：**面向用户的界面文案统一称「端口转发」**（主机配置卡片、设置分组、Dock 页签），与 RFC 4254 "TCP/IP Port Forwarding" 及 OpenSSH `-L/-R/-D` 术语一致；「隧道」仅作为搜索关键词与口语保留。代码标识符沿用英文 `tunnel`（`ssh/tunnel.rs`、`stores/tunnels.ts`、`tunnel*` 设置 key、帧常量），不做改名以免配置 schema 迁移。
> 关联代码：新增 `src-tauri/src/ssh/tunnel.rs`；改 `src-tauri/src/ssh/{session.rs,manager.rs,mod.rs,frame.rs}`、`src-tauri/src/{ipc.rs,lib.rs}`；改 `frontend/src/stores/session.ts`、新增 `frontend/src/stores/tunnels.ts`、改 `frontend/src/components/wb/DockPanel.vue`、`frontend/src/style.css`。**前端规则编辑与持久化（`NewConnectionModal.vue`、`types.ts`、`hosts.ts`）已就绪，不在改动范围**。

| 目标 | 量化口径 |
|---|---|
| 本地/SOCKS 监听建立 | 本机 `bind` 成功返回 < 100ms |
| 远程转发建立 | 仅 1 个 SSH global-request 往返，耗时 ≤ 1 个链路 RTT |
| 转发吞吐 | 单连接双向 copy 使用 16KB 缓冲，相对直连额外损耗 < 5%（1Gb 局域网 iperf-over-TCP 对照） |
| 规则容量 | 单会话规则上限 32 条；超限 `tunnel_start` 拒绝 |
| 连接容量（高并发） | 每条规则活动连接上限 64（`tokio::Semaphore` 许可制）；单会话全局上限 512；超限立即关闭新 TCP，不排队不堆积 |
| 内存上界 | 每活动连接 ≤ 2×16KB copy 缓冲 + 内核 socket 缓冲；单会话转发内存 ≤ 512×32KB ≈ 16MB，与规则数/连接数线性有界 |
| 慢连接攻击防护 | 本地接入后 10s 内必须完成（SOCKS 握手 / channel 打开），超时强制关闭；单规则接入速率令牌桶限流 100/s（突发 200） |
| 监听器可用性 | accept 循环遇系统级错误（如 `EMFILE`）由监督器指数退避自动重启（1s→30s 封顶，连续 5 次失败置 Error），端口不静默消失 |
| 空闲开销 | 无活动连接时任务全部阻塞在 `accept`/回调上，CPU 占用 ≈ 0，无轮询 |
| 状态推送时延 | 状态跃迁（启停/失败）即时推帧；活动连接数与流量变化 1s 节流合并推送 |
| 优雅停机 | 停止规则：先停 accept，活动连接最多排空 3s，逾期强制关闭；会话断开/应用退出后 1s 内全部监听 socket 释放 |
| 故障隔离 | 任一规则、任一转发连接失败不得影响同一 SSH 会话的终端、SFTP、其他规则 |

## 2. 必须遵守的既有项目约束

- `ssh/` 模块除 manager 外**不依赖 tauri**（`src-tauri/src/ssh/mod.rs` 头注释），转发引擎只使用 russh + tokio，可脱离 Tauri 单测/e2e；
- 高频字节流走每会话独立 Channel 的自定义二进制帧，低频控制走 invoke/JSON；**禁止全局 Event 广播**（见 [architecture.md](../architecture.md) §7）；
- `ipc.rs` 是薄层：只做参数校验与转发，业务逻辑放 `ssh/`；新增 invoke 命令必须在 `src-tauri/src/lib.rs` 的 `invoke_handler!` 登记；
- 前端组件不直接 invoke SSH 业务，统一经 stores 封装；高频整帧替换数据用 `shallowRef(new Map())` + `triggerRef`；
- 后台任务全部挂 `CancellationToken`；共享数据一律 clone `Arc` 后释放锁，**禁止持锁 await**；
- Rust 结构体 `#[serde(rename_all = "camelCase")]`；错误前缀即协议（如 `KEY_ENCRYPTED:`），不得改成普通文案；
- **russh 错误判断必须基于 `russh::Error` 枚举变体匹配，禁止解析错误字符串做分支**——版本升级会改变错误文本，字符串匹配将导致重试/降级逻辑静默失效（§6.10 给出枚举白名单）；
- 密码/私钥口令仅入系统钥匙串，绝不进入 `connections.json`、extra、日志；
- 新增二进制帧类型必须同步 `frame.rs`、`stores/session.ts` 的 `FRAME_*`、[architecture.md](../architecture.md) §7.2 帧表三处。

## 3. 现状盘点与差距

| 位置 | 现状 | 差距 |
|---|---|---|
| `frontend/src/components/NewConnectionModal.vue` 端口转发卡片 | **结构化规则编辑器已上线**：卡片列表，每条规则含类型（本地/远程/动态）、名称、启用开关、绑定地址+端口、目标地址+端口（动态无目标）；支持增删改、拖拽/键盘排序、逐条启用；`save()` 把 `tunnelRules` 写入 `Host.tunnels` | 无（数据模型与表单均已完成） |
| `frontend/src/types.ts` `TunnelRule` | 已定义 `{ id, type, name, enabled, bindHost, bindPort, targetHost?, targetPort? }`；`Host.tunnels?: TunnelRule[]` | 无 |
| `frontend/src/stores/hosts.ts` ↔ `src-tauri/src/store.rs` `StoredHost.extra` | 持久化链路**已打通**：`storedToHost` 从 `extra.tunnels` 还原、`hostToStored` 写 `{ tunnels }`；`connections.json` 与配置导入导出自动覆盖 | 无 |
| `frontend/src/stores/settings.ts` | 8 个 `tunnel*` 设置项已持久化，设置面板已挂 TODO 徽章 | 无引擎消费 |
| russh 0.63.3 API | `Handle::tcpip_forward` / `cancel_tcpip_forward` / `channel_open_direct_tcpip`、Handler 回调 `server_channel_open_forwarded_tcpip` 均已提供 | 全仓库零调用 |
| `ssh/session.rs` `ClientHandler` | 仅实现 `check_server_key`/`kex_done`；`SshSession` 持有 `handle: Arc<Mutex<Handle>>` 与会话级 `cancel`、`frame_tx` | 无远程转发回调；会话无端口转发生命周期管理 |
| `ssh/frame.rs` | 帧类型 0x01–0x09 | 无端口转发状态帧 |
| `DockPanel.vue` | 底部 Dock 有 `sftp` / `log` 两个页签 | 无端口转发管理入口 |
| docker 测试容器 `docker/debian-sshd/` | 已有依赖本地 sshd 的 e2e 基建（`src-tauri/tests/ssh_e2e.rs`） | 需补转发用例与容器内 TCP 服务 |

结论：本设计补齐四块——①Rust 三模式转发引擎；②2 个 invoke 命令 + 1 种状态帧；③前端端口转发 store 与 Dock 管理页签；④自动启动/重试/重连恢复联动与设置项落地。规则编辑与持久化已在位。

## 4. 目标与非目标

### 目标

1. **本地转发 `-L`**：按规则的 `bindHost`（默认 `127.0.0.1`）本地监听，每个接入连接经 `direct-tcpip` 通道连到规则目标，多连接复用同一 SSH 主连接；
2. **远程转发 `-R`**：按规则的 `bindHost` 向远端发 `tcpip-forward` 请求；远端来连时由客户端回调接入本地目标；支持停止时 `cancel-tcpip-forward`；
3. **动态转发 `-D`**：本地 SOCKS5 服务端，实现 RFC 1928 的「无认证 + CONNECT」子集，支持 IPv4 / 域名 / IPv6 目标地址；
4. 规则随主机配置持久化（`extra.tunnels` 结构化数组，**链路已就绪，本期不改 schema**），新建/编辑/导入导出全链路保留；
5. 会话内可逐条启停、查看状态（活动/失败原因/实际端口/活动连接数/双向流量）；
6. 连接建立后按设置自动启动**已启用**（`enabled=true`）的规则；转发瞬态失败按设置重试；SSH 断线重连成功后自动恢复规则；
7. 落地 5 个设置项：`tunnelAutoStart`、`tunnelPortConflict`、`tunnelRetryCount`、`tunnelReconnect`、`tunnelDnsResolve`；`tunnelNotify` 从设置 schema 移除（失败 toast 改为内置默认行为）。

### 非目标

- 跳板机（ProxyJump）与 SSH 外连代理（`proxyType`/`proxyHost`/`jumpHost` 表单字段继续不持久化、不消费）；
- 会话内添加「临时规则」（规则一律在主机配置面板维护；Dock 页签只做运行态启停与观测，不做规则增删）；
- SOCKS5 用户/密码认证、`BIND`、`UDP ASSOCIATE`；
- X11 转发、streamlocal（Unix socket）转发；
- `tunnelIdleTimeoutSec`（空闲自动断开）：保持 TODO 不消费；
- `tunnelKeepaliveSec`：保持 TODO 不消费——主连接已有的 RTT global-request 任务（30s 周期，见 `ssh/session.rs` `rtt_task`）客观上覆盖了 NAT 保活作用，转发通道无需独立保活报文；
- 系统级桌面通知：不引入 `tauri-plugin-notification`；`tunnelNotify` 设置项移除，失败提示固定为应用内 toast；
- 流量数据持久化：字节计数仅存内存，重启/重连清零。

## 5. 总体架构

`-L` / `-D`（本地监听）数据流：

```text
本地进程(浏览器/curl)                 SSH 主连接(加密)                 远端目标
     │ 127.0.0.1:8080 TCP                  │ direct-tcpip channel      │
     ▼                                     ▼                           ▼
TcpListener (tunnel.rs) ──accept──► Handle.channel_open_direct_tcpip(host,port)
     ◄────────────── tokio::io::copy ×2（16KB）──────────────►
     (-D 时 accept 后先做 SOCKS5 握手解析目标，再开 channel)
```

`-R`（远端监听）数据流：

```text
远端进程                 sshd 监听 127.0.0.1:9090            Rhost 客户端            本地目标
  │ TCP 连入                   │ forwarded-tcpip open          │ handler 回调           │
  ▼                            ▼                               ▼                       ▼
                    tcpip-forward(请求期) ──► RemoteRegistry 查表 ── TcpStream::connect ──►
                    ◄──────── tokio::io::copy ×2（16KB）────────►
```

模块关系：

```text
DockPanel.vue ──► stores/tunnels.ts ──invoke tunnel_start/stop──► ipc.rs ──► SessionManager
                                                                          │
frame 0x0A ◄── frame_tx ◄── TunnelManager（ssh/tunnel.rs，每会话一个）◄──┘
                                  │ 本地 -L/-D: TcpListener 任务
                                  │ 远程 -R  : RemoteRegistry + tcpip-forward
stores/session.ts 收帧 ──► tunnels.ts 快照 Map ──► DockPanel tunnel 页签
```

`TunnelManager` 作为 `SshSession` 的新字段，与 SFTP 子系统、metrics 采集器平级，共用同一条已认证主连接与同一个 `cancel` 令牌、同一帧队列。

## 6. 详细设计

### 6.1 规则模型与持久化格式（现状，不改 schema）

规则模型与持久化链路**已实现**，本节为其正式契约。`connections.json` 单个主机的 `extra.tunnels` 数组：

```json
{
  "extra": {
    "tunnels": [
      { "id": "t8f3a1c2", "type": "local",   "name": "Web 面板", "enabled": true,  "bindHost": "127.0.0.1", "bindPort": 8080, "targetHost": "localhost",   "targetPort": 80 },
      { "id": "t91bd0e4", "type": "remote",  "name": "远程回调", "enabled": false, "bindHost": "127.0.0.1", "bindPort": 9090, "targetHost": "localhost",   "targetPort": 3000 },
      { "id": "tc27f5a8", "type": "dynamic", "name": "SOCKS5",  "enabled": true,  "bindHost": "127.0.0.1", "bindPort": 1080 }
    ]
  }
}
```

字段契约（与 `frontend/src/types.ts` `TunnelRule` 一一对应）：

| 字段 | 类型 | 取值 |
|---|---|---|
| `id` | string | 前端生成的随机 id（`tnlUid()`），主机内唯一；引擎以它为规则主键，兼作 `tunnel_stop` 参数 |
| `type` | string | `local` / `remote` / `dynamic` |
| `name` | string | 显示名；留空时表单自动生成（如「本地转发 :8080」） |
| `enabled` | boolean | **配置态启用开关**：自动启动（`tunnelAutoStart`）只覆盖 `enabled=true` 的规则；Dock 页签展示全部规则，可临时启停不改写此字段 |
| `bindHost` | string | 监听地址；表单留空默认 `127.0.0.1`。`local`/`dynamic` 为本机监听地址，`remote` 为请求远端监听的地址 |
| `bindPort` | number | 1–65535；表单留空默认 80 |
| `targetHost` / `targetPort` | string / number | `local`/`remote` 必填（targetPort 留空默认 80）；`dynamic` 无此二字段 |

字段语义按类型对照（与表单 UI 的列名一致）：

| 类型 | bind 侧 | target 侧 | 等价 OpenSSH |
|---|---|---|---|
| `local` 本地转发 | 本地监听 | 远端可达目标 | `ssh -L bindHost:bindPort:targetHost:targetPort` |
| `remote` 远程转发 | 远端监听 | 本地可达目标 | `ssh -R bindHost:bindPort:targetHost:targetPort` |
| `dynamic` 动态转发 | 本地 SOCKS5 监听 | 无（按 SOCKS 请求动态决定） | `ssh -D bindHost:bindPort` |

注意：表单 UI **允许同一端口配置多条规则**（如不同 bindHost），编辑器不做去重；端口冲突统一由引擎在启动时按 `tunnelPortConflict` 策略处理，见 §6.3。配置导入导出无需改动——`connections.json` 整体已在导出范围（见 [config-import-export-design.md](./config-import-export-design.md) §5）。

### 6.2 后端模块 `ssh/tunnel.rs`

核心类型（serde 均为 camelCase）：

```rust
pub enum TunnelType { Local, Remote, Dynamic }

/// 与前端 TunnelRule 对齐的引擎入参（name/enabled 为配置态字段，引擎不消费，
/// 由前端在 invoke 前剥离）
pub struct TunnelRule {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: TunnelType,
    pub bind_host: String,           // 空串归一化为 "127.0.0.1"
    pub bind_port: u16,
    pub target_host: Option<String>, // dynamic 为 None
    pub target_port: Option<u16>,
}

pub enum TunnelState { Starting, Active, Error, Stopped }

pub struct TunnelStatus {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: TunnelType,
    pub state: TunnelState,
    pub bind_host: String,
    pub bind_port: u16,
    pub bound_port: u16,          // 实际监听端口（next 顺延 / 远端分配时不同于 bind_port）
    pub target_host: Option<String>,
    pub target_port: Option<u16>,
    pub active_connections: u32,
    pub bytes_up: u64,            // 本地→远端方向累计
    pub bytes_down: u64,          // 远端→本地方向累计
    pub error: Option<String>,
}
```

`RemoteRegistry`（远程转发回调查表，`ClientHandler` 与 `TunnelManager` 共享同一实例）：

```rust
#[derive(Clone)]
pub struct RemoteTarget { pub host: String, pub port: u16 }

/// 回调命中后拿到的完整上下文：本地目标 + 所属规则的并发闸门与计数器。
/// 远端来连与本地 accept 走同一套准入/计量，-R 不得绕过 MAX_CONN 防线。
pub struct RemoteBinding {
    pub target: RemoteTarget,
    pub entry: Arc<TunnelEntry>,   // 内含 rule_conns / counters / dirty
}

#[derive(Default)]
pub struct RemoteRegistry {
    /// key = (远端绑定地址, 远端实际绑定端口)，如 ("127.0.0.1", 9090)
    inner: Arc<StdMutex<HashMap<(String, u16), RemoteBinding>>>,
}
```

锁内只做 HashMap 插入/删除/clone `RemoteBinding`（`Arc` clone），**锁内不 await**。

`TunnelManager`：

```rust
pub struct TunnelManager {
    handle: Arc<Mutex<client::Handle<ClientHandler>>>,
    frame_tx: mpsc::Sender<Vec<u8>>,
    cancel: CancellationToken,         // 会话级令牌 clone
    remote: RemoteRegistry,
    tunnels: Arc<Mutex<HashMap<String, TunnelEntry>>>,
    global_conns: Arc<Semaphore>,      // MAX_CONN_GLOBAL：会话级转发连接总闸
}

struct TunnelEntry {
    rule: TunnelRule,
    rule_cancel: CancellationToken,    // 单规则停止令牌（会话 cancel 是其父集）
    state: Arc<Mutex<TunnelStateInner>>,
    counters: Arc<Counters>,           // AtomicU32 活动连接 + AtomicU64×2 流量
    dirty: Arc<AtomicBool>,            // 连接数/流量变化脏标记
    rule_conns: Arc<Semaphore>,        // MAX_CONN_PER_RULE：单规则连接闸门
    accept_limiter: RateLimiter,       // 令牌桶：ACCEPT_RATE_REFILL/s，突发 ACCEPT_RATE_BURST
}
```

限流与闸门全部 `try_acquire`/`allow` 语义：失败立即拒绝，**绝不排队等待**（排队会把背压传导成内存与 fd 堆积）。许可通过 RAII 守卫绑定在连接任务生命周期上，任务以任何方式退出都自动归还。

方法：`start(rule, opts) -> Result<(), SshError>`、`stop(id)`、`snapshot() -> Vec<TunnelStatus>`、`shutdown()`。所有 invoke 入口先 clone `Arc` 再放会话池锁（manager 层既有手法）。

魔数（集中定义为 `const`，注释写理由）：

| 常量 | 值 | 理由 |
|---|---|---|
| `MAX_RULES_PER_SESSION` | 32 | 单会话规则上限：防止规则爆炸耗尽本地端口与内核资源；超限返回 `TUNNEL_LIMIT:` |
| `MAX_CONN_PER_RULE` | 64 | 单规则并发上限：每连接吃 1 个本地 fd + 1 个 SSH channel + 2 个 copy 任务，64 在常规 `ulimit -n` 下留足余量 |
| `MAX_CONN_GLOBAL` | 512 | 单会话转发全局上限：第二条防线，防止 `-R` 洪峰灌穿所有规则配额之和 |
| `COPY_BUF` | 16384 | 转发为交互式流量，16KB 在延迟与吞吐间平衡 |
| `SOCKS_HANDSHAKE_MAX` | 512 | SOCKS5 方法协商 + CONNECT 请求最大长度远小于此，防恶意客户端灌包 |
| `HANDSHAKE_TIMEOUT` | 10s | 本地 accept 后到 channel 打开成功的总窗口（含 SOCKS 握手 + SSH channel open），逾期强制关闭 TCP |
| `CONN_COPY_TIMEOUT` | 0（无空闲超时） | 运行期 copy 不设空闲超时（由 `tunnelIdleTimeoutSec` 将来接管，见非目标）；连接长活不断 |
| `SUPERVISOR_RETRY_BASE_MS` / `SUPERVISOR_RETRY_MAX_MS` | 1000 / 30000 | 监听器监督器退避：1s 起翻倍，封顶 30s |
| `SUPERVISOR_MAX_RETRIES` | 5 | 连续 accept 失败后规则置 Error，不再无限重启 |
| `ACCEPT_RATE_BURST` | 200 | 单规则接入速率令牌桶突发上限；限流在 burst 前对突发友好，达限后平滑拒绝 |
| `ACCEPT_RATE_REFILL` | 100 | 令牌桶每秒回灌量；> 1000QPS 场景另行评估 |
| `REMOTE_CONNECT_TIMEOUT` | 5s | `-R` 回调中连接本地目标的超时，超时则 reject channel |
| `NEXT_PORT_TRIES` | 10 | 端口冲突 `next` 策略最多顺延尝试的端口数 |
| `RETRY_BASE_MS` / `RETRY_MAX_MS` | 1000 / 10000 | 建立期瞬态错误退避：1s 起翻倍，封顶 10s |
| `STATUS_THROTTLE` | 1s | 连接数/流量快照推送节流窗口；错误帧不节流，即时推送 |
| `SHUTDOWN_DRAIN_SEC` | 3 | 规则停止时活动连接最大排空时间，逾期强制 close |

### 6.3 本地转发 `-L`

`start` 流程：

1. 校验规则（后端独立再校验一遍，不信任前端：端口范围、host 长度/字符合法性），非法返回 `TUNNEL_BAD_RULE:`；`bind_host` 空串归一化为 `127.0.0.1`；
2. 按 `tunnelPortConflict` 绑定 `<bind_host>:<bind_port>`：
   - `stop`：bind 失败立即返回 `TUNNEL_PORT_IN_USE: 本地端口 <p> 已被占用`；
   - `skip`：bind 失败语义同错误，但仅用于「批量自动启动」场景——前端收到该错误后继续启动后续规则、不弹错误 toast（单条手动启动时与 `stop` 表现一致）；
   - `next`：从 `bind_port` 起逐个尝试，最多 `NEXT_PORT_TRIES` 个；全部失败返回 `TUNNEL_PORT_IN_USE:`；成功端口写入 `bound_port`；**`next` 为尽力策略：并发批量启动多条规则时，各规则的端口扫描之间无全局锁，可能出现 A 占用候选端口导致 B bind 失败的竞态，属可接受边界**；
3. bind 成功即向帧队列推一帧 `Active` 快照，随后 spawn 监听任务；监听任务本身由**监督器**包裹，accept 循环因系统级错误（如 `EMFILE`）退出时按 `SUPERVISOR_RETRY_BASE_MS` 指数退避重启（封顶 `SUPERVISOR_RETRY_MAX_MS`），连续 `SUPERVISOR_MAX_RETRIES` 次失败才置 `Error`；单条 listener 重启不影响其他规则。

监听任务（挂在 `rule_cancel` 与会话 `cancel` 双重令牌上）：

```text
loop {
  select! {
    _ = rule_cancel.cancelled() => break,
    accepted = listener.accept() => {
      let (tcp, peer) = accepted?;
      // 高并发闸门：令牌桶限流（平滑突发）+ 信号量许可（硬上限）
      if !rate_limiter.allow() { tcp 立即 drop; continue; }
      if per_rule_semaphore.try_acquire().is_err() { tcp 立即 drop; continue; }
      if global_semaphore.try_acquire().is_err() { tcp 立即 drop; continue; }
      spawn(handle_one(tcp, peer));
    }
  }
}
```

`handle_one`（`HANDSHAKE_TIMEOUT` 仅覆盖「accept → channel 打开成功」阶段，不覆盖运行期 copy）：

1. `handle.channel_open_direct_tcpip(target_host, target_port, "127.0.0.1", peer_port)`；channel 打开失败（含远端不可达）→ 关本地 TCP、归还双信号量许可、推脏标记，监听器继续；
2. 成功后拆两路 copy：`channel.make_reader()` → `tcp`（下行字节计入 `bytes_down`）、`tcp` → `channel.make_writer()`（上行计入 `bytes_up`）；
3. 任一路结束即收尾：对另一侧发 shutdown（`TcpStream::shutdown(both)` / drop writer），两个 copy 任务自然退出，归还许可、连接数减一、推脏标记；
4. russh `Handle` 为 `Arc<Mutex>`：channel 打开需持锁（瞬时），**持锁仅到 open 完成**；数据 copy 阶段只持有 channel，不持 handle 锁；
5. **许可 RAII 持有**：两个信号量许可保存在 `handle_one` 的 RAII 守卫中，连接任务任何退出路径（正常/错误/取消）都自动归还，杜绝泄漏。

### 6.4 动态转发 `-D`（SOCKS5）

监听建立、连接上限、copy 与 §6.3 完全相同，差异仅在 accept 后、open channel 前插入 SOCKS5 握手（纯函数解析，放在 `tunnel.rs` 的 `socks5` 子模块，无网络依赖、可单测）。**整个握手过程受 `HANDSHAKE_TIMEOUT` 约束，杜绝慢速握手攻击（slowloris）**：

1. **方法协商**：读 `[VER, NMETHODS, METHODS...]`；`VER != 0x05` 关连接。仅支持无认证（方法 `0x00`）：客户端方法列表含 `0x00` 则回复 `05 00`，否则回复 `05 FF` 后关闭；
2. **CONNECT 请求**：读 `[VER, CMD, RSV, ATYP, DST.ADDR..., DST.PORT×2]`，总读取量以 `SOCKS_HANDSHAKE_MAX` 为上限：
   - `CMD` 仅支持 `0x01`（CONNECT），其余回复失败码 `0x07`；
   - `ATYP=0x01`：IPv4 4 字节；`ATYP=0x04`：IPv6 16 字节；`ATYP=0x03`：首字节域名长度 + 域名（1–255）；其他回复 `0x08`；
3. **DNS 策略**（`tunnelDnsResolve`）：
   - `remote`（默认）：`ATYP=0x03` 时域名原样传给 `channel_open_direct_tcpip`，由远端 sshd 解析；IPv4/IPv6 也按地址文本透传；
   - `local`：域名先在本机 `tokio::net::lookup_host` 解析，**优先取 IPv4 地址**（宿主解析结果可能以 IPv6 在先，如 macOS `localhost` → `::1`，而远端 sshd 未必监听 IPv6，直取首个会造成 CONNECT 无谓失败），无 IPv4 才取首个；解析失败回复 `0x04`（Host unreachable）后关闭；
4. channel 打开成功回复 `05 00 00 01 00 00 00 00 00 00`（`BND.ADDR/PORT` 填零，SOCKS 客户端普遍不校验）；失败统一回复 `0x01`（General failure）——russh 不暴露 channel open 失败细类（连接拒绝与网络不可达同为 RequestDenied/ConnectFailed 语义模糊），故不定义 `0x03/0x05` 细分码；
5. 回复后进入与 §6.3 相同的双向 copy。

握手期间任何 I/O 错误/畸形报文：关闭 TCP、不 panic、不影响监听器；**握手字节计入 `HANDSHAKE_TIMEOUT`，超出即 RST**。

### 6.5 远程转发 `-R`

启动：

1. `handle.tcpip_forward(&bind_host, bind_port)`（`bind_host` 为空归一化为 `127.0.0.1`；非 loopback 地址受 sshd `GatewayPorts` 策略约束，被拒按 `RequestDenied` 处理）：
   - 成功返回远端**实际绑定端口**（传 0 由远端分配；UI 必填端口，引擎保留支持），写入 `bound_port` 并以 `(bind_host, bound_port)` 为 key 写入 `RemoteRegistry`，value 为含该规则 `TunnelEntry` 的 `RemoteBinding`；
   - `russh::Error::RequestDenied`（sshd `AllowTcpForwarding no` 等）**不重试**，返回 `TUNNEL_REMOTE_DENIED: 远端服务器拒绝远程转发`；
   - 连接瞬态错误（`Disconnect`/`SendError`）按 §6.10 退避重试；
2. 推 `Active` 快照；不 spawn 监听任务——远端来连走 Handler 回调。

`ClientHandler` 新增字段 `remote: RemoteRegistry`，在 `connect_and_auth` 构造 handler 时创建；`SshSession::connect` 取出同一 Arc 交给 `TunnelManager`。实现回调 `server_channel_open_forwarded_tcpip`：

1. 以回调参数中的远端绑定地址 + `connected_port` 查 `RemoteRegistry`：
   - 命中：std 锁内 clone 出 `RemoteBinding` 后立即放锁；
   - 未命中：`reply.reject(ChannelOpenFailure::AdministrativelyProhibited)` 后返回；
2. **并发闸门（与本地 accept 同一套防线）**：对 `entry.rule_conns` 与会话 `global_conns` 做 `try_acquire`，任一失败立即 `reply.reject(ChannelOpenFailure::AdministrativelyProhibited)` 并返回；远端无 accept 循环可挂令牌桶，以信号量硬拒绝为唯一准入控制；许可以 RAII 守卫带入后续 spawn 任务；
3. **先连本地目标再 accept**：`TcpStream::connect((host, port))` 包 `REMOTE_CONNECT_TIMEOUT`；失败 `reply.reject(ChannelOpenFailure::ConnectFailed)`；
4. `reply.accept()`，随后 spawn 双向 copy（计数/脏标记/收尾同 §6.3）；
5. 回调的 async 块不得持有 `&mut self` 跨越 await（沿用 `kex_done` 既有手法：先 clone/取出数据，再 `async move`）。

**Channel 所有权约束（高风险项）**：russh 0.63.3 中 `server_channel_open_forwarded_tcpip` 回调收到的 `Channel<Msg>` 为 owned 值，但 channel 资源生命周期与回调上下文存在强绑定风险——直接 `async move` 进 spawn 任务必须验证。

落地四条硬约束：

1. 编码前先核对 russh 0.63.3 官方 `forward-tcpip` 示例与 `Channel` 的 `Send` 实现，确认 owned channel 可安全跨任务移动；
2. 若证实不能直接 move，改用 channel 分离（`into_parts`/`make_reader`+`make_writer`）或消息中继包装，并在代码注释中写明选型理由；
3. e2e 必须包含 `-R` 并发压测用例（≥64 路并发接入，断言无 panic、无 IO 挂起、无 channel 提前 drop）；
4. 单测构造「回调 + spawn」最小场景，断言 channel 在回调返回后仍可正常读写。

停止：`handle.cancel_tcpip_forward(&bind_host, bound_port)`（错误忽略，主连接可能已断）→ 从 `RemoteRegistry` 删除（此后新回调一律 reject）→ 取消 `rule_cancel`（停止接受新连接）→ **排空活动连接最多 `SHUTDOWN_DRAIN_SEC` 秒**，逾期强制关闭 → 推 `Stopped` 快照。`-L`/`-D` 的 stop 语义相同：先停 accept，再按同一排空窗口收尾。

**停机语义警示**：`cancel-tcpip-forward` 仅通知 sshd 不再接受**新接入请求**，对已建立的 forwarded-tcpip channel **无任何影响**；现存连接的终止完全依赖本地 `rule_cancel` + 3s 排空窗口，实现时不得误以为该调用会断开存量连接。

### 6.6 状态帧 `0x0A`

`ssh/frame.rs` 新增：

| type | 名称 | payload | 时机 |
|---|---|---|---|
| `0x0A` | Tunnel | JSON `{"tunnels":[TunnelStatus,...]}`，**全量快照** | **状态跃迁**（Starting→Active/Error/Stopped）即时推送，不做节流；仅**活动连接数 / 流量数值**变化走 1s 节流合并 |

推送双通道：**状态跃迁**（start/stop/error 路径）在操作点直接推帧，即时到达；**连接数/流量**变化只置脏标记，由 `TunnelManager` 内的状态任务 `tokio::time::interval(STATUS_THROTTLE)` 醒来检查，有脏则清标记并推全量快照，无变化不推。规则总数上限 32，全量 JSON 成本可忽略，换取前端无合并逻辑。前端 `stores/session.ts` 增加 `FRAME_TUNNEL = 0x0A` 分发到 `stores/tunnels.ts` 的 `handleTunnelFrame(sessionId, json)`，以 `shallowRef(new Map())` 整帧替换。实现时同步登记 [architecture.md](../architecture.md) §7.2 帧表。

会话断开时前端清空该会话快照（与 metrics 等会话态处理一致）。

### 6.7 invoke 命令与错误前缀

新增两个命令（`ipc.rs` 薄封装，`lib.rs` 登记）：

| 命令 | 入参 | 返回 | 说明 |
|---|---|---|---|
| `tunnel_start` | `sessionId: string`, `rule: TunnelRuleDto`, `portConflict: string`, `dnsResolve: string`, `retryCount: number` | `()` | 幂等：相同 `id` 已在 `Active` 或 `Starting` 状态返回 `TUNNEL_RUNNING:`（前端按正常处理，不当错误弹窗）；批量自动启动遇 `TUNNEL_LIMIT:` 时跳过本条继续启动剩余规则 |
| `tunnel_stop` | `sessionId: string`, `ruleId: string` | `()` | 幂等：不存在视为成功 |

`TunnelRuleDto` 为 camelCase 结构体，字段同 §6.1。状态全部走帧通道，不提供 `tunnel_list`（前端启动/停止后必收到快照帧；重连后由前端重新启动规则自然重建状态）。

错误前缀协议（前端据此分流 toast 文案与批量策略）：

| 前缀 | 含义 | 前端处理 |
|---|---|---|
| `TUNNEL_RUNNING:` | 规则已在 `Active`/`Starting`（幂等信号） | 视为成功，不 toast、不改 desired |
| `TUNNEL_BAD_RULE:` | 规则字段非法 | 表单内提示，不发请求（后端为兜底） |
| `TUNNEL_PORT_IN_USE:` | 本地端口占用且策略未解决 | 红色状态 + toast；批量场景 `skip` 不 toast |
| `TUNNEL_REMOTE_DENIED:` | 远端策略拒绝 | 红色状态 + toast，不重试 |
| `TUNNEL_LIMIT:` | 该规则连接数达上限 或 规则数达上限 | 连接数超限仅 debug 日志与计数，不 toast；规则数超限按 `skip` 策略继续后续规则 |

### 6.8 日志与可观测性

- 每条状态跃迁输出 debug 日志：`tunnelId + state + boundPort + error`，便于用户 issue 定位；**永远不打印业务载荷数据**；
- 状态跃迁即时推帧 + 1s 节流合并连接数/流量，双通道互补。

### 6.9 前端 store 与 UI

新增 `frontend/src/stores/tunnels.ts`：

| 导出 | 内容 |
|---|---|
| `tunnelSnapshots` | `shallowRef(Map<sessionId, TunnelStatus[]>)` + `triggerRef` |
| `desiredRules` | `Map<sessionId, Set<string>>`：用户**期望运行**的规则 id 集合（内存态，不持久化），Dock 页签开关的响应依据、重连恢复的依据 |
| `startTunnel(session, rule, opts)` | 把 id 加入 desired → invoke `tunnel_start`；失败移出 desired 并 toast |
| `stopTunnel(sessionId, ruleId)` | 从 desired 移出 id → invoke `tunnel_stop` |
| `startSavedOnConnect(session)` | 读 `session.host.tunnels` 中 `enabled=true` 的规则：`tunnelAutoStart` 开启时批量启动（`skip`/`TUNNEL_LIMIT` 策略继续后续规则）；否则列表仅展示状态 |
| `restoreOnReconnect(session)` | `tunnelReconnect` 开启时，按 `desiredRules` 批量重启 |
| `toggleDesired(sessionId, ruleId, on)` | Dock 页签开关的直达函数：on → 调用 `startTunnel`；off → 调用 `stopTunnel` |
| `syncDesiredFromSnapshot(sessionId, statuses)` | **每次收到 `0x0A` 快照帧都执行，用后端真实 Active 状态校正 desiredRules**（幂等）：后端状态为 Active 的 id 同步加入 desired；Stopped/Error 不修改 desired |

`stores/session.ts`：

- `handleFrame` 增加 `FRAME_TUNNEL = 0x0A` 分支，调 `handleTunnelFrame`；
- `handleTunnelFrame` 在每次快照到达时：先调 `syncDesiredFromSnapshot` 校正 desired，再整帧替换 `tunnelSnapshots`——`desiredRules` 只是**期望意图**，后端帧才是唯一事实源；防止前端 store 热重载 / 页面刷新后期望集与后端 Active 状态漂移（Dock 开关显示关闭但后端仍在监听）；
- `connectBackend` 成功置 `online` 后：首次连接调 `startSavedOnConnect`；`reconnecting` 代次调 `restoreOnReconnect`（自动重连与手动重连路径统一经过此处）；
- 会话移除/最终断开时清理两个 Map 的该会话条目。

`DockPanel.vue`：

- `dockTab` 类型从 `'sftp' | 'log'` 扩为 `'sftp' | 'log' | 'tunnel'`；`stores/session.ts` 的 `ui_state` hydrate 白名单与 `patchUiState` 同步增加 `tunnel`；
- 新增第三个页签按钮（图标复用现有 SVG 体系的链路/波形图标）与 `TunnelPane` 面板（直接写在 DockPanel 内或新建 `components/wb/TunnelPane.vue`，实现时按文件体量决定）；
- 面板布局（当前会话的全部规则，按类型分组，含禁用态规则）：

```text
┌ 端口转发 ──────────────────────────── [全部启动] [全部停止] ┐
│ 本地转发 (-L)                                                │
│  ● 8080 → localhost:80      [开关]  活动 2  ↑12KB ↓1.1MB     │
│  ○ 3306 → db.internal:3306 [开关]  端口被占用（原因 tooltip） │
│ 远程转发 (-R)                                                │
│  ● 9090 → localhost:3000   [开关]  活动 0                    │
│ 动态转发 SOCKS5 (-D)                                         │
│  ● 1080                    [开关]  socks5://127.0.0.1:1080   │
└──────────────────────────────────────────────────────────────┘
```

- 每条规则的开关状态由 `desiredRules` 驱动（id 在集合内 = 引擎已收到 start/正在运行）；启用（`enabled=true`）规则默认开启，禁用（`enabled=false`）规则默认关闭但可见；
- 状态点：Active 绿、Starting 黄（脉冲）、Error 红、Stopped 灰；流量按 B/KB/MB/GB 自适应格式（复用 SFTP 队列既有格式化函数）；
- 停止规则时 Dock 开关 tooltip 提示「停止不再接受新连接，已有连接最多等待 3s 完成传输」，排空期间 `active_connections` 计数器保持真实值，不归零；
- 非 Tauri（浏览器 dev）环境：面板可见但开关给出与终端一致的 `[dev] 浏览器模式无 Tauri 后端` toast；
- 无规则时展示空态文案 + 「去主机配置面板添加端口转发规则」按钮（通过 `openNewConnectionModal(hostId)` 打开编辑态弹窗）。

### 6.10 自动启动、重试与重连恢复

- **自动启动**：见 §6.9 `startSavedOnConnect`，仅在终端连接进入 `online` 后触发，失败不回滚连接状态、不写终端缓冲；
- **建立重试**：仅引擎层瞬态失败重试——russh `Error::Disconnect` / `Error::SendError`（**枚举变体匹配，禁止字符串解析**）；`RequestDenied`、`TUNNEL_BAD_RULE`、端口占用属确定性失败，不重试；按 `tunnelRetryCount`（0–20）做指数退避 `1s,2s,4s...` 封顶 10s，重试期间状态为 `Starting`；
- **幂等语义**：`tunnel_start` 对已 `Active` 或 `Starting` 状态的规则返回 `TUNNEL_RUNNING:` 幂等信号（前端按成功处理），**不改变**现有后台重试任务；仅 `Stopped`/`Error` 会真正启动新任务；
- **监听器自愈（高可用）**：`-L`/`-D` 的 accept 循环因系统级错误（fd 耗尽、网络子系统瞬时故障）退出时，由监督器按指数退避自动重启，**复用原 `bind_host:bind_port`**（`bound_port` 不变），规则保持 `Active`，已建立的活动连接不受影响；连续失败超限才置 `Error`。若重启瞬间端口被外部进程抢占，bind 直接失败并计入监督器失败计数，达到 `SUPERVISOR_MAX_RETRIES` 置 `Error`——**运行中端口抢占不在自愈范围**；
- **运行期 channel 故障**：单条已建立的转发 channel 断开只终止该 TCP 连接，监听器继续服务新连接；
- **主连接重连**：russh 重连即全新后端会话（新 `session_id`、新 `TunnelManager`），转发规则不可能在后端自行存活；由前端 `restoreOnReconnect` 在 online 后按 `desiredRules` 批量重启，`next` 策略分配的实际端口以新快照为准。

### 6.11 设置项消费矩阵

| 设置项 | 本期处理 | 消费位置 |
|---|---|---|
| `tunnelAutoStart` | 落地，移除 TODO 徽章 | 前端 `startSavedOnConnect` |
| `tunnelPortConflict` | 落地，移除 TODO 徽章 | 引擎 bind 策略 + 前端批量 `skip` 分流 |
| `tunnelRetryCount` | 落地，移除 TODO 徽章 | 引擎退避重试 |
| `tunnelReconnect` | 落地，移除 TODO 徽章 | 前端重连恢复 |
| `tunnelDnsResolve` | 落地，移除 TODO 徽章；**设置项 desc 补充提示**：`local` 时本机 DNS/hosts/VPN 与远端解析结果可能不一致，排错困难 | SOCKS5 握手后域名处理 |
| `tunnelNotify` | **从 schema 移除**（`settings.ts` 类型 + `SettingsModal.vue` 行 + 后端 `SettingsSection` 字段）；失败 toast 为内置默认行为；老配置中的残留值由后端 `#[serde(flatten)] extra` 静默保留不消费 | — |
| `tunnelKeepaliveSec` | 不消费，保留 TODO 徽章，描述补注「主连接保活已覆盖」 | — |
| `tunnelIdleTimeoutSec` | 不消费，保留 TODO 徽章 | — |

并发/限流/超时参数（`MAX_CONN_*`、令牌桶、`HANDSHAKE_TIMEOUT`、`SHUTDOWN_DRAIN_SEC`、监督器退避等）全部为**内部常量，不开放配置**——用户无合理调整动机，暴露只会增加排错噪音。

## 7. 安全与降级

- **绑定地址由用户显式控制**：规则 `bindHost` 默认 `127.0.0.1`（表单留空即归一化）；若用户主动填非 loopback 地址（如 `0.0.0.0`），视为有意向局域网开放监听，引擎不拦截，但规则编辑 UI 在 bindHost 非 loopback 时给出「将向局域网暴露此端口」的弱提示；`-R` 的非 loopback 绑定能否生效取决于远端 sshd `GatewayPorts`；
- **DoS 与资源耗尽防线（高并发安全）**：
  - 规则数上限 32/会话、连接上限 64/规则 + 512/会话双闸门（`tokio::Semaphore` 许可制，`try_acquire` 失败即关闭，零排队）；
  - 接入速率令牌桶限流（100/s 回灌、200 突发），抵御本地端口扫描式洪峰打爆 fd；
  - `HANDSHAKE_TIMEOUT` 10s 覆盖「accept → channel 打开」全窗口，慢速握手（slowloris 式）占用不得超过此时长；
  - 内存有界：copy 缓冲固定 16KB×2/连接，全会话转发驻留内存 ≤ 16MB，不随流量突增膨胀；
  - 状态帧 1s 节流 + 脏标记合并，流量/连接数抖动不会反向打爆前端 IPC 通道；
- **载荷零记录**：转发内容不写日志、不落盘、不进帧队列；日志仅允许出现规则 id、目标 host:port、状态与字节计数（debug 级）；
- **extra 白名单**：`extra` 当前只持久化 `tunnels` 数组（见 `frontend/src/stores/hosts.ts` `hostToStored`）；密码、私钥口令不进入 extra；
- **SOCKS5 攻击面**：握手读取有字节上限；不支持 UDP 与认证协商；畸形报文静默断连；监听不接受任何认证凭据，故无凭据泄漏面；
- **远端拒绝**（`AllowTcpForwarding no`/channel 打开被拒）：规则置 Error 并提示，终端/SFTP/其他规则不受影响；
- **目标不可达/连接被拒**：仅关闭对应那一条 TCP 连接（SOCKS 回标准错误码），监听器持续服务；
- **连接数打满**：新连接立即关闭（`-L` 为 RST/FIN，`-D` 在握手前直接关闭），不排队、不占用额外内存；
- **优雅停机与排空**：停止规则先关 accept、排空 ≤ 3s 再强杀，避免文件传输/数据库会话被静默截断；排空窗口内新连接已被拒绝，不会「边排空边进新流量」；
- **主连接断开**：会话 `cancel` 触发所有 listener 关闭、注册项清理、状态任务退出、信号量许可随任务 RAII 全量回收；`-R` 的 `cancel-tcpip-forward` 尽力而为，连接已断时忽略错误；
- **应用退出**：`SessionManager::shutdown_all` → 各 `SshSession::shutdown` → `TunnelManager::shutdown` 同步链路覆盖，无孤儿监听端口、无悬挂许可。

## 8. 测试与验证

- **Rust 单测**（`ssh/tunnel.rs` 内联 `#[cfg(test)]`）：
  - 规则校验：端口越界、bind_host/target_host 空值归一化与非法字符、dynamic 携带 target 字段时报错；
  - SOCKS5 握手纯函数：方法协商（含 0x00 与不支持方法）、CONNECT 三地址类型解析、域名超长、非 CONNECT/非法 VER/ATYP、回复报文字节精确断言；
  - `RemoteRegistry`：插入/查找/删除/端口 key 命中；
  - **回调 + spawn 最小场景**：构造模拟 `server_channel_open_forwarded_tcpip` 回调，断言 channel 在回调返回后仍可正常读写（对应 §6.5 所有权约束）；
  - 状态快照序列化字段为 camelCase；
  - **并发闸门**：单规则信号量 64 许可耗尽后第 65 条被拒；全局 512 许可独立耗尽；许可在任务取消/错误/正常三条路径均归还（用 `Semaphore::available_permits` 断言）；
  - **令牌桶**：突发 200 内全通过、第 201 条拒绝、静置 1s 后恢复 ~100 许可；
  - **监督器**：模拟 accept 连续失败（注入错误），断言退避序列 1s→2s→4s→…→30s 封顶、第 6 次置 Error 不再重启；
  - **握手超时**：客户端接入后 10s 不发数据，连接被强制关闭且许可归还；
  - **排空停机**：停止时挂一条慢 copy 连接，断言 ≤3s 内等待、超时后强制关闭；
- **e2e**（扩展 `src-tauri/tests/ssh_e2e.rs`，容器 `docker/debian-sshd/` 内预置或测试时拉起 TCP 服务）：
  - `-L`：容器内起 HTTP 服务（python3 http.server），本地经 `127.0.0.1:<映射端口>` GET 成功并校验内容；
  - `-D`：本地 SOCKS 端口经 HTTP 客户端（或容器内 `curl --socks5` 不可用，则在 e2e 进程内手写最小 SOCKS5 客户端）访问容器 HTTP 服务，覆盖 IPv4 与域名两种 ATYP；
  - `-R`：测试主机进程起临时 HTTP 服务，invoke 建立远程转发后在容器内 exec `curl http://127.0.0.1:<远端端口>` 验证回源到主机；
  - **高并发**：同一 `-L` 规则并行 100 条 TCP（并发数 > 单规则配额），断言 64 条成功、其余被拒、无 listener 崩溃、许可全部归还；
  - **`-R` 并发压测**：≥64 路并发经远端 forwarded-tcpip 接入，断言无 panic、无 IO 挂起、无 channel 提前 drop（对应 §6.5 所有权约束）；
  - **多规则并发 `next`**：≥8 条 `next` 策略规则并发启动，断言各自 bind 成功或返回 `TUNNEL_PORT_IN_USE:`、无崩溃、无重复 bound_port；
  - **慢速握手**：100 条慢握手连接挂住监听器 10s+，断言监听器继续接受新连接、慢连接被批量清理；
  - 生命周期：`tunnel_stop` 后端口确认关闭；重复 start 返回 `TUNNEL_RUNNING:`；会话断开后监听端口释放；
- **手工验证矩阵**（真机）：
  1. 主机编辑配置三条规则 → 保存重开回填正确 → 重启应用仍在；
  2. 连接后自动启动，Dock「端口转发」页签状态/连接数/流量随浏览器访问实时变化；
  3. 端口占用三策略：占用 `-L` 端口后重连，分别验证 stop 报错、skip 静默继续、next 顺延端口；
  4. `-R` 对一台 `AllowTcpForwarding no` 的服务器（容器可配）验证拒绝提示且终端正常；
  5. 拔网/休眠触发自动重连，规则随重连自动恢复；手动开关状态与 desired 一致；
  6. 端口转发大流量传输同时使用终端 vim 与 SFTP 传输，确认无卡顿/无串流；
  7. 配置导出再导入（含仅主机范围），规则随主机保留；
  8. macOS 与 Linux 各跑一轮（Windows 暂缺真机，至少保证编译通过）。

## 9. 未决问题

1. **`next` 顺延端口是否回写配置**：本文定为不回写——实际端口只在运行时快照展示，重连后重新协商，避免静默改掉用户配置。如需固定，用户应改填空闲端口；
2. **编辑主机后运行中规则的联动**：`NewConnectionModal` 保存触发 `rehostSession` 更新会话的 `host`，但已启动的规则仍在按旧配置运行。本期不自动 diff 重放——Dock 页签手动停启即可；若后续反馈强烈，再评估「保存时自动重启受影响规则」。

其余原未决项已随现状收敛：绑定地址由规则 `bindHost` 承载（默认 loopback，见 §7）；Dock 不提供临时规则，规则增删统一回主机配置面板。
