# SSH 端口转发开发计划

> description: SSH 端口转发设计方案的分阶段实现计划，按依赖关系拆分为 7 个阶段，覆盖引擎、IPC、前端、联调
> 创建时间：2026-10-07 21:33:58
> 更新时间：2026-10-07 21:33:58
> 作者：

---

## 1. 概述

本计划是 [SSH 端口转发设计方案](./tunnel-design.md) 的实现分解，按依赖关系拆分为 7 个阶段，每阶段可独立提交。

**关联代码**：
- 后端：`src-tauri/src/ssh/{tunnel.rs,session.rs,manager.rs,mod.rs,frame.rs}`、`src-tauri/src/{ipc.rs,lib.rs}`
- 前端：`frontend/src/stores/{session.ts,tunnels.ts}`、`frontend/src/components/wb/{DockPanel.vue,TunnelPane.vue}`、`frontend/src/components/SettingsModal.vue`、`frontend/src/style.css`
- 测试：`src-tauri/tests/ssh_e2e.rs`、`docker/debian-sshd/`
- 文档：`docs/explanation/design/tunnel-design.md`、`docs/.vitepress/config.mts`

**前置约束**：
- 前端规则编辑与持久化（`NewConnectionModal.vue`、`types.ts`、`hosts.ts`）已就绪，**不在改动范围**
- russh 错误判断必须基于枚举变体匹配，禁止字符串解析
- 所有后台任务挂 `CancellationToken`；禁止持锁 await
- 并发/限流/超时参数全部为内部常量，不开放配置

---

## 2. 阶段总览与依赖

```text
阶段 A 引擎基建（tunnel.rs 骨架 + RemoteRegistry + 帧常量 + 单测）
  │
  ├─→ 阶段 B 本地转发 -L（监听 + 监督器 + 闸门 + copy）
  ├─→ 阶段 C 动态转发 -D SOCKS5（依赖 B）
  ├─→ 阶段 D 远程转发 -R（回调 + 所有权验证 + 并发闸门，依赖 B）
  │
  └─→ 阶段 E IPC 命令（tunnel_start/stop + 错误前缀，依赖 B/C/D）
        │
        ├─→ 阶段 F 前端 store + 帧分发 + Dock 页签（依赖 E）
        └─→ 阶段 G 联调与验证（依赖全部）
```

**关键路径**：A → B → D → E → F → G。C 与 D 可并行。

---

## 3. 阶段 A：引擎基建

**目标**：建立 `ssh/tunnel.rs` 骨架——类型定义、`RemoteRegistry`、`TunnelManager` 结构、令牌桶、帧常量、单测。

### 3.1 帧常量

文件：`src-tauri/src/ssh/frame.rs`

- 新增 `FRAME_TUNNEL: u8 = 0x0A`；同步登记 `docs/architecture.md` §7.2 帧表

### 3.2 核心类型与常量

文件：`src-tauri/src/ssh/tunnel.rs`（新建）

- `TunnelType`、`TunnelRule`、`TunnelStatus`、`TunnelState` 枚举/结构体（serde camelCase）
- 魔数常量：`MAX_RULES_PER_SESSION=32`、`MAX_CONN_PER_RULE=64`、`MAX_CONN_GLOBAL=512`、`COPY_BUF=16384`、`SOCKS_HANDSHAKE_MAX=512`、`HANDSHAKE_TIMEOUT=10s`、`SUPERVISOR_RETRY_BASE_MS=1000`、`SUPERVISOR_RETRY_MAX_MS=30000`、`SUPERVISOR_MAX_RETRIES=5`、`ACCEPT_RATE_BURST=200`、`ACCEPT_RATE_REFILL=100`、`REMOTE_CONNECT_TIMEOUT=5s`、`NEXT_PORT_TRIES=10`、`RETRY_BASE_MS=1000`、`RETRY_MAX_MS=10000`、`STATUS_THROTTLE=1s`、`SHUTDOWN_DRAIN_SEC=3`
- `RateLimiter`（手写令牌桶，惰性回灌，无第三方 crate）
- `RemoteRegistry` + `RemoteBinding`（value 含 `Arc<TunnelEntry>`，锁内不 await）
- `TunnelEntry` / `TunnelManager` 结构体（含 `global_conns: Semaphore`）

### 3.3 单测

- 规则校验：端口越界、host 空值归一化、非法字符
- 令牌桶：突发 200 全通过、第 201 拒绝、静置 1s 恢复
- `RemoteRegistry`：插入/查找/删除/key 命中
- 状态快照序列化 camelCase

**验收**：`cargo test -p rhost ssh::tunnel` 全绿。

---

## 4. 阶段 B：本地转发 -L

**目标**：`tunnel_start` 本地分支——bind 策略、监听任务、监督器、双信号量闸门、双向 copy。

### 4.1 start 流程

文件：`src-tauri/src/ssh/tunnel.rs`

- 规则校验 → `TUNNEL_BAD_RULE:`；`bind_host` 空串归一化 `127.0.0.1`
- 按 `tunnelPortConflict` 绑定：`stop` 直接报错 / `skip` 返回错误（前端继续）/ `next` 顺延最多 10 次
- bind 成功推 `Active` 快照帧，spawn 监听任务

### 4.2 监听任务 + 监督器

- accept 循环挂 `rule_cancel` + 会话 `cancel` 双令牌
- 接入后三层闸门：令牌桶 `allow()` → 单规则信号量 `try_acquire()` → 全局信号量 `try_acquire()`，失败立即关闭
- `handle_one`：`HANDSHAKE_TIMEOUT` 仅覆盖 accept → channel 打开阶段
- channel 打开失败关 TCP 归还许可；成功后拆两路 copy（16KB），RAII 守卫持有许可
- 监督器：accept 系统错误（EMFILE 等）按 1s→30s 指数退避重启，复用原 `bind_host:bind_port`，连错 5 次置 `Error`

### 4.3 单测 + e2e

- 单测：信号量许可三路径归还（正常/错误/取消）、握手超时强制关闭
- e2e：容器内 HTTP 服务，本地 GET 校验内容；高并发 100 连接断言 64 成功其余被拒

**验收**：e2e `-L` 全绿；真机浏览器访问映射端口正常。

---

## 5. 阶段 C：动态转发 -D（SOCKS5）

**目标**：RFC 1928「无认证 + CONNECT」子集，复用 B 的监听/copy/闸门。

### 5.1 SOCKS5 握手纯函数

文件：`src-tauri/src/ssh/tunnel.rs` `socks5` 子模块

- 方法协商：仅支持 `0x00`，否则 `05 FF` 关闭
- CONNECT 请求解析：`ATYP` 支持 IPv4/域名/IPv6；`CMD` 仅 `0x01`，其余回 `0x07`
- 总读取上限 `SOCKS_HANDSHAKE_MAX=512`，握手计入 `HANDSHAKE_TIMEOUT`
- DNS 策略：`remote` 域名透传 sshd；`local` 本机 `lookup_host` 取首地址，失败回 `0x04`

### 5.2 单测 + e2e

- 单测：方法协商（0x00/不支持）、三地址类型解析、域名超长、非法 VER/ATYP、回复字节精确断言
- e2e：本地 SOCKS 端口经手写最小客户端访问容器 HTTP，覆盖 IPv4 与域名两种 ATYP

**验收**：e2e `-D` 全绿；curl `--socks5` 访问成功。

---

## 6. 阶段 D：远程转发 -R

**目标**：`tcpip-forward` 请求 + Handler 回调接入 + 并发闸门。**最高风险阶段**，必须先验证 Channel 所有权。

### 6.1 前置验证（编码前）

- 查阅 russh 0.63.3 官方 `forward-tcpip` 示例与 `Channel` 的 `Send` 实现，确认 owned channel 可安全跨任务移动
- 若不能直接 move：改用 channel 分离（`into_parts`/`make_reader`+`make_writer`）或消息中继包装，代码注释写明选型理由

### 6.2 start 流程

- `handle.tcpip_forward(&bind_host, bind_port)`；非 loopback 受 sshd `GatewayPorts` 约束，被拒按 `RequestDenied` 处理
- 成功返回 `bound_port`，以 `(bind_host, bound_port)` 为 key 写 `RemoteRegistry`，value 为 `RemoteBinding { target, entry: Arc<TunnelEntry> }`
- `RequestDenied` 不重试返回 `TUNNEL_REMOTE_DENIED:`；`Disconnect`/`SendError` 按退避重试
- 推 `Active` 快照；不 spawn 监听任务

### 6.3 Handler 回调

文件：`src-tauri/src/ssh/session.rs` `ClientHandler`

- 新增字段 `remote: RemoteRegistry`，`connect_and_auth` 构造时创建，`SshSession::connect` 取出同一 Arc 交 `TunnelManager`
- 实现 `server_channel_open_forwarded_tcpip`：
  1. 查 `RemoteRegistry`，命中 std 锁内 clone `RemoteBinding` 放锁；未命中 `reject(AdministrativelyProhibited)`
  2. 并发闸门：`entry.rule_conns` + `global_conns` `try_acquire`，失败立即 reject
  3. `TcpStream::connect` 本地目标包 `REMOTE_CONNECT_TIMEOUT=5s`，失败 `reject(ConnectFailed)`
  4. `reply.accept()`，spawn 双向 copy（计数/脏标记/收尾同 -L）
  5. 回调 async 块不得持 `&mut self` 跨 await
- 停止：`cancel_tcpip_forward`（仅拒新接入）→ 删注册表 → 取消 `rule_cancel` → 排空 ≤3s → 推 `Stopped`

### 6.4 单测 + e2e

- 单测：构造模拟回调 + spawn 最小场景，断言 channel 在回调返回后可正常读写
- e2e：`-R` 并发压测 ≥64 路并发接入，断言无 panic/IO 挂起/channel 提前 drop；多规则并发 `next` ≥8 条断言无重复 bound_port

**验收**：e2e `-R` 全绿；真机 `curl 127.0.0.1:<远端端口>` 回源成功。

---

## 7. 阶段 E：IPC 命令

**目标**：`tunnel_start` / `tunnel_stop` invoke 命令 + 错误前缀协议。

### 7.1 invoke 命令

文件：`src-tauri/src/ipc.rs`、`src-tauri/src/lib.rs`

- `tunnel_start(sessionId, rule: TunnelRuleDto, portConflict, dnsResolve, retryCount)` → `Result<(), String>`
- `tunnel_stop(sessionId, ruleId)` → `Result<(), String>`
- `lib.rs` `invoke_handler!` 登记
- `TunnelRuleDto` 为 camelCase，字段同 `frontend/src/types.ts` `TunnelRule`（`name`/`enabled` 由前端剥离）

### 7.2 错误前缀协议

| 前缀 | 触发条件 |
|---|---|
| `TUNNEL_RUNNING:` | 幂等信号：已在 `Active`/`Starting` |
| `TUNNEL_BAD_RULE:` | 字段非法 |
| `TUNNEL_PORT_IN_USE:` | 端口占用且策略未解决 |
| `TUNNEL_REMOTE_DENIED:` | 远端策略拒绝 |
| `TUNNEL_LIMIT:` | 规则数/连接数超限 |

### 7.3 日志

- 每条状态跃迁输出 debug 日志：`tunnelId + state + boundPort + error`；**永远不打印载荷**

**验收**：`cargo check` 无错；invoke 命令在 Tauri dev 工具中可调。

---

## 8. 阶段 F：前端 store + Dock 页签

**目标**：`stores/tunnels.ts`、帧分发、desired 校正、Dock 页签 UI、设置项消费。

### 8.1 `stores/tunnels.ts`（新建）

文件：`frontend/src/stores/tunnels.ts`

- `tunnelSnapshots: shallowRef(Map<sessionId, TunnelStatus[]>)` + `triggerRef`
- `desiredRules: Map<sessionId, Set<string>>`（内存态）
- `startTunnel` / `stopTunnel` / `toggleDesired` / `startSavedOnConnect` / `restoreOnReconnect`
- `syncDesiredFromSnapshot(sessionId, statuses)`：**每次**收到 `0x0A` 快照都执行，后端 Active 的 id 加入 desired，Stopped/Error 不改
- 错误前缀分流：`TUNNEL_RUNNING:` 视为成功；`TUNNEL_LIMIT:` 连接数超限仅 debug；规则数超限按 skip 继续

### 8.2 `stores/session.ts` 帧分发

文件：`frontend/src/stores/session.ts`

- `handleFrame` 增加 `FRAME_TUNNEL = 0x0A` 分支，调 `handleTunnelFrame`
- `handleTunnelFrame`：先 `syncDesiredFromSnapshot` 校正 desired，再整帧替换 `tunnelSnapshots`
- `connectBackend` 置 `online` 后：首次连接调 `startSavedOnConnect`；`reconnecting` 代次调 `restoreOnReconnect`
- 会话移除时清理两个 Map 条目
- `dockTab` 类型扩为 `'sftp' | 'log' | 'tunnel'`；`ui_state` hydrate 白名单 + `patchUiState` 同步

### 8.3 Dock 页签 UI

文件：`frontend/src/components/wb/DockPanel.vue`、新建 `frontend/src/components/wb/TunnelPane.vue`（或并入 DockPanel 按体量决定）

- 新增第三个页签按钮（链路/波形图标）
- 面板布局：当前会话全部规则按类型分组，含禁用态；每行：状态点 + 绑定信息 + 目标信息 + 开关 + 活动连接数 + 流量
- 开关状态由 `desiredRules` 驱动；启用规则默认开，禁用规则默认关但可见
- 停止开关 tooltip：「停止不再接受新连接，已有连接最多等待 3s 完成传输」；排空期计数器保持真实值
- 状态点：Active 绿、Starting 黄脉冲、Error 红、Stopped 灰；流量 B/KB/MB/GB 自适应
- 无规则时空态文案 + 「去主机配置面板添加端口转发规则」按钮
- 非 Tauri 环境开关 toast `[dev] 浏览器模式无 Tauri 后端`

### 8.4 设置项消费

文件：`frontend/src/components/SettingsModal.vue`、`frontend/src/stores/settings.ts`

- 移除 5 项 TODO 徽章：`tunnelAutoStart`、`tunnelPortConflict`、`tunnelRetryCount`、`tunnelReconnect`、`tunnelDnsResolve`
- `tunnelDnsResolve` desc 补充：「`local` 时本机 DNS/hosts/VPN 与远端解析结果可能不一致，排错困难」
- **`tunnelNotify` 从 schema 移除**：`settings.ts` 类型删除 + SettingsModal 行删除 + 后端 `SettingsSection` 字段删除；老配置残留值由后端 `#[serde(flatten)] extra` 静默保留
- 保留 TODO：`tunnelKeepaliveSec`（desc 补注「主连接保活已覆盖」）、`tunnelIdleTimeoutSec`

**验收**：IDE 诊断无错；真机三模式启停/状态/流量显示正常。

---

## 9. 阶段 G：联调与验证

**目标**：全量测试矩阵 + 真机验证 + 文档收尾。

### 9.1 全量测试

- Rust 单测全绿（`cargo test`）
- e2e 全绿（`cargo test --test ssh_e2e`）
- 前端构建无错（`pnpm build`）
- IDE 诊断无错（4 个改动文件）

### 9.2 真机验证矩阵

1. 主机编辑三条规则 → 保存重开回填正确 → 重启应用仍在
2. 连接后自动启动，Dock 状态/连接数/流量实时变化
3. 端口占用三策略：stop 报错 / skip 静默继续 / next 顺延
4. `-R` 对 `AllowTcpForwarding no` 服务器验证拒绝提示且终端正常
5. 拔网/休眠触发重连，规则自动恢复
6. 大流量转发同时使用终端 vim + SFTP，确认无卡顿/串流
7. 配置导出再导入，规则随主机保留
8. macOS + Linux 各跑一轮

### 9.3 文档收尾

- `tunnel-design.md` 头部「更新时间」改为最终完成时间
- `docs/.vitepress/config.mts` 登记本开发计划
- `docs/architecture.md` §7.2 帧表确认已登记 `0x0A`

---

## 10. 风险与回退

| 风险 | 缓解 | 回退 |
|---|---|---|
| russh `server_channel_open_forwarded_tcpip` Channel 所有权问题 | 阶段 D 前置验证 + e2e 并发压测 | 改用 channel 分离/消息中继，代码注释写明 |
| 监督器重启逻辑复杂 | 单测覆盖退避序列 + 超限置 Error | 简化为「出错即推 Error，前端按 desired 重启」 |
| 排空停机 3s 窗口用户困惑 | Dock tooltip 明确提示 | 改为立即强杀（`SHUTDOWN_DRAIN_SEC=0`） |
| 状态帧 1s 节流期间用户看不到瞬时变化 | 状态跃迁即时推帧兜底 | 缩短节流窗口到 500ms |
