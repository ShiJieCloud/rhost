# 应用日志（App Log）设计方案与规范

> 状态：设计定稿，待落地（2026-10-05）
> 日期：2026-10-05
> 范围：Rhost 应用自身运行日志的采集、过滤、内存缓冲、底部面板输出、磁盘持久化与轮转清理
> 关联代码：`src-tauri/src/lib.rs`、`src-tauri/src/ipc.rs`、`frontend/src/stores/settings.ts`（log* 字段）、`frontend/src/components/SettingsModal.vue`（日志 tab）、`frontend/src/components/wb/DockPanel.vue`（日志面板，当前为 mock）
> 边界：**本文档只覆盖应用日志**（Rhost 客户端自己的运行事件）。终端会话录制（PTY 输出落盘）是另一独立特性，不复用本管线的任何字段与目录。

---

## 1. 设计目标与硬约束

| 目标 | 量化口径 |
|---|---|
| 单一事实来源 | 前端面板看到的日志与磁盘文件出自同一份结构化模型（`AppLogEntry`），时间戳同一个时钟；文件端 JSONL、面板端纯文本，两种格式单点生成 |
| 低开销 | 单条日志处理 < 0.1ms；落盘不阻塞 tokio worker；面板未打开时推送路径零分配 |
| 不丢关键事件 | ERROR/WARN 不落盘失败（写入异常降级 stderr，不 panic、不影响业务） |
| 设置热生效 | logCollect / logLevel / logPersist / logMaxLines 修改后无需重启立即生效 |
| 安全 | 永不记录密码、私钥、PTY 数据流、剪贴板内容、文件内容（脱敏规则见 §8） |

必须遵守的既有项目约束：

- **高频流走 Tauri `Channel`，禁止全局 Event/Emitter**（architecture.md §通信协议）；日志面板订阅同样走 Channel，不复用全局事件总线；
- 终端 PTY 字节流**严禁**进入日志管线（数据量与敏感内容双重原因）；
- 一条 SSH 连接同一时刻只持有一个 SFTP 通道——SFTP 日志只记录事件，不得为日志新增任何通道操作；
- 密码只进系统钥匙串，日志中出现的连接信息不得包含凭据；
- 沿用 `log` crate 作为后端宏门面（已有依赖，`log::info!` 等调用点无需改写）。

## 2. 现状盘点

| 位置 | 现状 | 差距 |
|---|---|---|
| `settings.ts` log* 字段 + SettingsModal 日志 tab | 9 个设置项 UI 完整（采集/级别/行数/持久化/路径/命名/切割/文件数/保留天数） | **无任何代码消费**，纯界面 |
| `src-tauri/src/lib.rs` | 仅 debug 构建挂 `tauri-plugin-log`（Debug 级），落 `~/Library/Logs/com.rhost.app/rhost.log` | release 无日志；格式不可控；无法推送前端；不响应设置 |
| `DockPanel.vue` 日志 tab | 面板 UI 完整：级别筛选、搜索高亮、自动滚动跟随、清空 | 数据源是 `LOG_SEED` mock + SFTP 事件本地 `pushLog`；行数上限硬编码 500；不落盘 |
| `development.md` §6.5 | 约定 Rust 用 `log::debug!/error!` 带 session id | 缺少统一事件目录与格式规范 |

结论：UI 骨架与设置模型已就绪，缺的是**后端日志中心 + IPC 订阅协议 + 面板数据接入**。本设计补齐这三块，并把 mock 数据源整体替换。

## 3. 总体架构

日志中心设在**后端**（新模块 `src-tauri/src/applog/`）。前端日志（含 SFTP 队列事件、IPC 错误）也通过 invoke 上报到后端，与后端 `log::` 宏汇入同一条管线：

```
┌─ 后端各模块: log::info!(target: "ssh", ...) ─────────────┐
│  (applog::Logger 实现 log::Log，启动时 set_logger)        │
├─ 前端: invoke report_app_log { level, target, msg, kv } ─┤
│                                                          ▼
│                                          ┌── applog::Hub（全局单例）──┐
│                                          │ 1. 级别过滤（logLevel 热读） │
│                                          │ 2. 分配单调递增 seq         │
│                                          │ 3. 打时间戳（本地, 毫秒）    │
│                                          └──────┬───────┬───────┬─────┘
│                                                 │       │       │
│                        ① 环形缓冲 VecDeque      │       │       │ ③ mpsc → 落盘线程
│                          (logMaxLines, replay 用)│       │       │   批量 flush (200ms/64KB)
│                                                 │       │       │   轮转检查 + 定期清理
│                                                 │       ▼       │
│                                                 │  ② 订阅者 Channel 增量推送
│                                                 │    (面板打开时注册; 未订阅零开销)
│                                                 ▼
└────────────────────────────── 前端 DockPanel 日志 tab（replay + 增量渲染）
```

关键决策：

1. **自实现 `log::Log`，弃用 `tauri-plugin-log`**。plugin-log 面向开发者调试：release 不启用、格式不可控、无法推送到前端面板、不响应运行时设置变更。保留 `log` crate 门面后，全部 `log::*!` 调用点与第三方 crate（russh 等）日志自动进入管线，零改造成本。
2. **前端日志也上报后端**。若前端本地维护一份再与后端推送合并，两路时间戳会乱序、落盘与显示不一致。统一走后端的代价是每条前端日志多一跳 invoke（低频，可忽略），换来：单一时钟、单一 seq、面板数据 = 磁盘内容。
3. **订阅制推送**。面板（DockPanel 日志 tab）打开时 `subscribe_app_logs(since_id, channel)`：先收 replay（环形缓冲存量），再收增量；关闭时 drop Channel，后端 send 失败自动摘除订阅（复用 connect_ssh 帧转发任务的既有模式）。未订阅时推送路径为零开销。**支持多订阅者广播**（面板 + 诊断导出 + 未来其他消费者），Hub 维护订阅者列表，send 失败即移除；不采用单订阅者替换模型，避免旧订阅静默断流。
4. **落盘走独立 std 线程**。mpsc 有界队列（1024 条）隔离 IO，tokio worker 永不被磁盘阻塞。**背压分三级**：
   - 高水位（80%）：丢弃 DEBUG/INFO，累计 `dropped` 计数；
   - 硬满：WARN/ERROR 不再入队，直接同步写 stderr + 累计 `dropped` 计数；
   - `dropped` 计数由 Hub 内 5s 定时器检查，非零即主动产出一条 `app.log.dropped` WARN（不依赖下一条日志触发）；
   - stderr 输出做 1s 内相同错误最多 5 条的简单限流，防止风暴。
5. **双格式单源**：Hub 产出结构化 `AppLogEntry`；落盘线程将其 `serde_json` 序列化为 JSONL 直写文件（机器友好，§5.1）；面板订阅收到同一模型后在前端格式化为纯文本行（人类可读，§5.2）。两种格式共用 schema，不存在拼装漂移。

## 4. 日志事件目录（记什么）

四级语义与使用场景：

| 级别 | 语义 | 典型场景 |
|---|---|---|
| ERROR | 用户操作失败 / 组件异常 | 连接失败、传输失败、IPC 命令报错、落盘失败 |
| WARN | 可恢复的异常 / 用户需知晓的降级 | 重连、传输取消/暂停、断点续传截断重传、指标采集降级、慢调用 |
| INFO | 关键生命周期节点 | 应用启动/退出、连接建立/断开、传输开始/完成、设置变更 |
| DEBUG | 排障细节（默认不采集） | 握手协商明细、keepalive 心跳、续传偏移计算、队列调度细节 |

**`event_id` 命名规范**（每条日志必填，与 `msg` 解耦）：

- 格式：`domain.action[.subaction]`，全小写蛇形，如 `ssh.connect.start`、`sftp.transfer.complete`；
- `domain` 与 `target` 同源（`app`/`ssh`/`sftp`/`metrics`/`ipc`/`web`）；
- **为什么需要**：`msg` 是中文人类可读文案，不可作为机器筛选键（文案调整/未来 i18n 会破坏日志聚合规则、测试断言与告警）；`event_id` 是恒定的事件类型标识，`jq 'select(.event_id=="ssh.auth.failed")'` 精确匹配；
- **集中定义**：后端 `applog/events.rs` 以常量形式注册全部 event_id（编译期防手误）；前端 `stores/applog.ts` 同步同一份常量表（`web.*` 域），文档本节表格即事实清单，三者必须同步；
- **面板默认不显示** event_id（保持纯文本简洁），stores 层保留字段供筛选/搜索扩展，后续可加"显示事件 ID"调试开关。

### 4.1 应用生命周期（target: `app`）

启动过程分阶段输出，每个阶段一条 INFO，失败时对应阶段产出 ERROR 并带 `stage` 标记——排障时可精确定位启动死在哪个环节：

| 事件 | event_id | 级别 | 消息与 kv |
|---|---|---|---|
| 启动开始 | `app.boot.start` | INFO | `Rhost 启动中` version、os、arch |
| 配置加载 | `app.boot.config_loaded` | INFO | `配置文件已加载` elapsed_ms、path（`app_config.json` 完整路径；不内嵌 settings 快照——kv 保持平铺，生效配置直接查该文件） |
| 日志目录就绪 | `app.boot.log_dir_ready` | INFO | `日志目录已就绪` path |
| 过期日志清理 | `app.log.cleanup` | INFO | `日志清理` removed、freed_kb（启动时 + 每小时清理，仅在实际删除文件时产出，见 §6.4） |
| 密钥存储就绪 | `app.boot.keychain_ready` | INFO | `密钥存储已连接` backend（macOS Keychain / Windows Credential Manager / libsecret） |
| 启动完成 | `app.boot.ready` | INFO | `Rhost 启动完成` elapsed_ms（从 boot.start 到 ready 的总耗时） |
| 启动阶段失败 | `app.boot.failed` | ERROR | `启动失败` stage（当前阶段 event_id）、err |
| 退出开始 | `app.shutdown.start` | INFO | `正在退出` active_sessions |
| 会话全部关闭 | `app.shutdown.sessions_closed` | INFO | `会话已全部关闭` count、elapsed_ms |
| 日志缓冲落盘 | `app.shutdown.log_flushed` | INFO | `日志缓冲区已落盘` pending（flush 前剩余条数）。固定为 INFO：退出序列是排障里程碑，任何级别下都必须完整落盘（否则默认 INFO 时"卡在 flush"判据失效） |
| 退出完成 | `app.exit` | INFO | `Rhost 退出` uptime_s、active_sessions |
| 进程 panic | `app.panic` | ERROR | `线程发生 panic` thread、location（file:line:col）、backtrace（截断 4KB）。绕过 Hub 直接追加当前日志文件（崩溃时管线可能已不可用），同时保留默认 panic hook 的 stderr 输出 |
| 设置变更 | `app.settings.change` | INFO | `设置变更` key、value（敏感键只记 `changed=true`，不记值） |
| 日志落盘异常 | `app.log.persist_failed` | ERROR | `日志落盘失败` path、err（只报一次，防风暴） |
| 时间回拨 | `app.clock.rollback` | WARN | `系统时间回拨，日志文件切换` offset_ms（见 §6.3 reclock 机制） |
| 损坏行容错 | `app.log.corrupt_line` | WARN | `日志文件存在损坏行，已跳过` file、line（见 §6.5） |
| 队列溢出丢弃 | `app.log.dropped` | WARN | `日志队列溢出，已丢弃` dropped=n（与 §9 背压计数配合） |
| 配置导出成功 | `app.config.export` | INFO | `配置已导出` path（导出目标路径）、scope、sections、bytes、encrypted（是否密码加密）、elapsed_ms |
| 配置导出失败 | `app.config.export_failed` | WARN | `配置导出失败` stage（read/assemble/encrypt/write）、error |
| 配置导入开始 | `app.config.import.start` | INFO | `配置导入开始` file_bytes、file_version、scope（仅信息记录，不参与导入逻辑） |
| 配置导入成功 | `app.config.import.complete` | INFO | `配置导入完成` connections_added、connections_renamed、keys_added、keys_renamed、groups_added、settings_changed、elapsed_ms |
| 配置导入失败 | `app.config.import.failed` | WARN | `配置导入失败` stage（size/parse/hash/decrypt/version/write）、error（decrypt 阶段为日志细分文案：密码错误 / 文件损坏或格式无效）、source（文件名不含内容）、bytes；stage=parse 时另带 section；stage=decrypt 时另带 reason（`bad_password` 结构合法但 GCM 认证失败 / `corrupted` envelope 结构非法） |
| 非关键节损坏跳过 | `app.config.import_section_skipped` | WARN | `导入节已跳过` section、error（ui_state/quick_connect_history 容错） |
| 恢复默认设置 | `app.config.settings_reset` | INFO | `设置已恢复默认` elapsed_ms |
| 配置写穿被拒绝 | `app.config.write_rejected` | WARN | `配置节写入被拒绝` section、reason（节名非法或 schema 校验失败） |

启动时序约束：

1. **Hub 最先初始化**——`app.boot.start` 必须成为第一条日志，Hub（set_logger、落盘线程、清理扫描）在 `lib.rs` 的 `.setup()` 最前面执行，先于 SessionManager 等其他全局状态挂载；
2. **阶段顺序固定**：`boot.start` → `boot.config_loaded` → `boot.log_dir_ready` → `log.cleanup`（可选）→ `boot.keychain_ready` → `boot.ready`；
3. **任一阶段失败**：产出 `app.boot.failed`（带 stage），不产出后续阶段事件；`boot.ready` 缺失即表示启动未完成；
4. **config_loaded 只记配置文件路径**——kv 保持平铺（`elapsed_ms`、`path`），不内嵌 settings 对象：嵌套对象无法在面板纯文本展示（会显示 `[object Object]`），生效中的具体配置直接查看 path 指向的 `app_config.json`；该文件不含密码（密码走系统钥匙串），路径本身可记录。

退出时序约束（与 §6.2 的 ExitRequested 钩子对齐）：

1. **阶段顺序固定**：`shutdown.start`（ExitRequested 触发时）→ `shutdown.sessions_closed`（SessionManager 全部关闭后）→ `shutdown.log_flushed`（Hub 收到关闭信号、落盘线程 flush 完成前）→ `app.exit`（join 落盘线程后，放行 Runtime 退出前，必须是最后一条）；
2. **排障语义**：用户强杀/崩溃时，日志末尾停在哪个阶段即卡在哪里——只有 `shutdown.start` 表示卡在关会话；有 `sessions_closed` 无 `log_flushed` 表示卡在落盘 flush；有 `log_flushed` 无 `exit` 表示卡在线程 join；
3. `app.exit` 之后不再产出任何日志（Hub 已关闭，落盘线程已退出）。

### 4.2 SSH 连接生命周期（target: `ssh`，带会话标签）

连接过程分阶段输出，失败时在对应阶段产出 ERROR——与启动阶段同理，排障时精确定位卡在哪段：

| 事件 | event_id | 级别 | 消息与 kv |
|---|---|---|---|
| 发起连接 | `ssh.connect.start` | INFO | `正在连接` host、port、user、auth（password/publickey/agent） |
| TCP 建立 | `ssh.connect.tcp` | DEBUG | `TCP 已连接` rtt_ms |
| TCP 失败 | `ssh.connect.tcp_failed` | ERROR | `TCP 连接失败` host、port、err（超时/拒绝/网络不可达） |
| 握手开始 | `ssh.handshake.start` | DEBUG | `SSH 握手开始` |
| 握手失败 | `ssh.handshake.failed` | ERROR | `SSH 握手失败` err、server_banner（若有） |
| 握手完成 | `ssh.handshake.complete` | INFO | `SSH 握手完成` server_banner、kex、host_key、cipher（复用 0x09 Algo 帧数据） |
| 主机密钥 | `ssh.hostkey.fingerprint` | DEBUG | `主机密钥指纹` fingerprint（SHA256:…） |
| 认证开始 | `ssh.auth.start` | DEBUG | `开始认证` method、user |
| 认证成功 | `ssh.auth.success` | INFO | `认证成功` method、user |
| 认证失败 | `ssh.auth.failed` | ERROR | `认证失败` method、reason（服务端返回文本，**不含密码**） |
| 会话创建开始 | `ssh.session.create` | DEBUG | `创建会话` term、cols、rows、enc |
| 通道打开 | `ssh.session.channel_open` | DEBUG | `会话通道已打开` channel_id |
| PTY 分配 | `ssh.session.pty` | DEBUG | `PTY 已分配` term、cols、rows |
| 环境变量就绪 | `ssh.session.env` | DEBUG | `环境变量已设置` locale（实际生效值）、fallback（是否回退到 C.UTF-8） |
| shell 启动 | `ssh.session.shell` | DEBUG | `shell 已启动` shell（bash/zsh/sh）、init_injected（CWD hook 是否注入） |
| 会话创建失败 | `ssh.session.failed` | ERROR | `会话创建失败` stage（channel/pty/env/shell）、err |
| 会话建立 | `ssh.session.ready` | INFO | `会话已建立` term、cols、rows、enc、elapsed_ms（create→ready 耗时） |
| 初始化脚本上传失败 | `ssh.session.init_script_failed` | WARN | `会话初始化脚本上传失败…` err（打开 exec 通道被拒、远端退出码非零、无退出状态；常见 /tmp 不可写或磁盘满）；CWD 同步、彩色提示符、UTF-8 locale 自动配置随之跳过 |
| 断开（主动/异常） | `ssh.disconnect` | INFO/WARN | `连接已断开` reason、uptime_s |
| 重连中 | `ssh.reconnect` | WARN | `连接中断，准备重连` attempt、max、delay_ms |
| SFTP 通道建立 | `ssh.sftp.channel.open` | DEBUG | `SFTP 通道建立` |
| SFTP 通道关闭 | `ssh.sftp.channel.close` | DEBUG | `SFTP 通道空闲超时关闭` idle_s |

连接时序约束：

1. **阶段顺序固定**：`connect.start` → `connect.tcp` → `handshake.start` → `handshake.complete` → `auth.start` → `auth.success` → `session.create` → `session.ready`；
2. **任一阶段失败**：产出对应阶段的 ERROR 事件（如 `ssh.connect.tcp_failed`、`ssh.handshake.failed`、`ssh.auth.failed`、`ssh.session.failed`），不产出后续阶段事件；`session.ready` 缺失即表示连接未完成；
3. **认证方法顺序**：支持多轮认证时（如先 key 后 password），每轮产出独立 `auth.start` + `auth.failed/success` 对，结果按最终 `session.ready` 判定；
4. **重连复用同一链路**：`ssh.reconnect` 触发后重走完整连接序列，sid 保持不变。

会话创建内部时序（`session.create` 与 `session.ready` 之间）：

1. **子步骤顺序固定**：`channel_open` → `pty` → `env` → `shell`，对应 russh 的 `channel_open_session()` → `request_pty()` → `setenv()` → `exec(shell)` 调用序列；
2. **失败统一走 `ssh.session.failed`**，kv 中 `stage` 字段标识失败子步骤（`channel`/`pty`/`env`/`shell`），不拆出独立 event_id——失败是低频事件，阶段语义由 stage 承载，避免 event_id 膨胀；
3. **`env` 子步骤的排障价值**：kv 中 `fallback=true` 表示远端 locale 缺失已回退 C.UTF-8（对应项目约束「SSH 终端必须确保 UTF-8，检查 locale charmap 失败时回退」）；若用户报告终端中文乱码，先查该事件的 `locale` 与 `fallback`；
4. **`shell` 子步骤的 `init_injected`**：记录 SFTP CWD hook（PROMPT_COMMAND/precmd_functions）是否成功注入——若为 `false`，后续 SFTP 目录双向同步不会工作，排障时优先检查该项；
5. **子步骤全部 DEBUG 级**：正常流程下 INFO 视图只看到 `session.ready` 一条，DEBUG 视图才展开子步骤；`session.ready` 的 `elapsed_ms`（create→ready）用于发现「会话创建慢」类问题（如远端 shell 初始化脚本耗时过长）。

### 4.3 SFTP 传输与管理（target: `sftp`）

传输任务七个生命周期事件（与 DockPanel 现有 pushLog 文案对齐迁移）：

| 事件 | event_id | 级别 | 消息与 kv |
|---|---|---|---|
| 入队 | `sftp.transfer.enqueue` | INFO | `上传/下载加入队列` name、size、dir（up/down）、from、to |
| 开始 | `sftp.transfer.start` | INFO | `上传/下载开始` name、size、resume_from（续传偏移，0 为全新） |
| 完成 | `sftp.transfer.complete` | INFO | `上传/下载完成` name、size、elapsed_ms、speed_bps（B/s）、verify（none/pass/fail） |
| 失败 | `sftp.transfer.failed` | ERROR | `上传/下载失败` name、err、transferred、total |
| 取消 | `sftp.transfer.cancel` | WARN | `上传/下载已取消` name、transferred、total |
| 暂停 | `sftp.transfer.pause` | WARN | `已暂停` name、pct |
| 恢复 | `sftp.transfer.resume` | INFO | `上传/下载继续` name、pct（与暂停配对的显式用户动作，默认级别须可见） |
| 校验失败 | `sftp.transfer.verify_failed` | ERROR | `完整性校验失败` name、expect、actual |

管理操作记 INFO（成功）/ ERROR（失败），event_id 分别为 `sftp.manage.remove` / `sftp.manage.rename` / `sftp.manage.mkdir` / `sftp.manage.copy`，kv 带 path（rename/copy 带 src+dst）。**不记录文件内容**。

### 4.4 指标与其他（target: `metrics` / `ipc` / `web`）

| 事件 | event_id | 级别 | 说明 |
|---|---|---|---|
| 指标采集启动 | `metrics.collect.start` | DEBUG | interval、session |
| 指标采集停止 | `metrics.collect.stop` | DEBUG | session |
| 采集能力缺失（常态） | `metrics.collect.degraded` | DEBUG | `GPU 指标不可用，跳过`（component=gpu, reason=not_present）/ `目标主机不支持指标采集，转入慢轮询`（component=exec, reason=unsupported，首轮从未成功即失败，如无 procfs）：目标主机本就没有该能力（云主机/虚拟机常态），非故障；每采集任务首次确认记一次 |
| 采集运行时降级（故障） | `metrics.collect.degraded` | WARN | `采集连续失败，指标降级为慢轮询`：component=exec、reason=runtime_failure、consecutive、backoff_ms、err；仅曾采集成功后连续失败才告警，边沿触发，恢复后再次达到阈值才重记 |
| 慢 IPC 调用 | `ipc.slow_call` | WARN | `IPC 调用耗时过长` cmd、elapsed_ms（阈值 2000ms，只告警不阻断）。**豁免**：`sftp_upload`/`sftp_download` 等流式传输命令——大文件天然长耗时，进度由 Channel 推送，invoke 总时长不反映卡顿且包含用户主动暂停时间；传输卡顿监控应基于进度帧间隔而非 invoke 时长 |
| 前端 IPC 错误 | `web.ipc.error` | ERROR | 前端 invoke 失败统一上报：cmd、err |
| 前端未捕获异常 | `web.unhandled_error` | ERROR | window.onerror / unhandledrejection：err、stack 摘要（截断 500 字符） |

### 4.5 明确不记录（黑名单）

- 密码、私钥内容/指纹外的任何密钥材料、passphrase
- PTY 输入输出字节流（命令、回显）
- 剪贴板内容（pasteGuard 只记拦截事件与长度/行数特征）
- 传输/浏览的文件内容
- 环境变量值（`settings.env` 只记 key 列表）

## 5. 日志格式规范

### 5.1 文件日志格式（JSON Lines，机器友好）

文件端使用 **JSONL**（`*.log` 实为每行一条 JSON），便于日志聚合、搜索脚本和外部工具（`jq`、ELK）消费。面板端保持 §5.2 的纯文本格式——两种格式从同一份结构化模型生成，单点序列化。

```jsonl
{"seq":1024,"ts":"2026-10-05T14:23:01.123+08:00","level":"info","target":"ssh","event_id":"ssh.connect.start","sid":"a1b2c3","msg":"正在连接","kv":{"host":"192.168.1.10","port":22,"user":"deploy","auth":"publickey"}}
```

规则：

1. **字段与 §5.3 IPC 载荷字段一一对应**，字段名统一 `snake_case`：
   - `seq` (u64)：全局单调递增序列号（职责见 §7.1）
   - `ts` (string)：ISO 8601 格式带毫秒与时区偏移，如 `2026-10-05T14:23:01.123+08:00`（本地时间，保留时区信息便于跨设备分析）
   - `level` (string)：小写 `debug`/`info`/`warn`/`error`
   - `target` (string)：§4 模块短名
   - `event_id` (string)：**必填**，`domain.action[.subaction]` 事件类型标识，命名规范见 §4 开头
   - `sid` (string, optional)：session uuid 前 6 位，全局事件省略该键
   - `msg` (string)：中文事件消息
   - `kv` (object, optional)：结构化键值对，值类型为 string / number / bool；空对象时省略该键
2. **单行一条**，以 LF 结尾，无尾部逗号，JSON 中不含换行（`msg` 内换行转义为 `\n`）。
3. **单条长度上限 8KB**（JSON 允许比面板文本格式更丰富的嵌套 kv），超出截断 `msg` 并追加 **顶层字段** `"_truncated":true`（不属于 `kv` 对象，确保 `jq` 筛选不遗漏）；截断保证输出 JSON 仍合法，不截断在转义字符中间。
4. **落盘线程直接 `serde_json::to_string` 后追加**，不保留换行格式化；文件可读性由 `jq` 等工具负责。

> 完整文件样例见 `docs/samples/rhost_app_20261005.log`（24 条，覆盖启动/连接/SFTP/重连/崩溃降级/退出全链路）。

### 5.2 面板日志格式（纯文本，人类可读）

面板端保留现有 DockPanel 日志 tab 的渲染格式，与 JSONL 独立——后端推送结构化 `AppLogEntry`，前端本地格式化为纯文本行。

```
2026-10-05 14:23:01.123 [INFO ] [ssh] [a1b2c3] 认证成功 method=publickey user=deploy
└────────┬──────────────┘ └──┬──┘ └─┬─┘ └──┬──┘ └──────┬──────┘ └────────┬────────┘
     本地时间(毫秒)        级别定宽5  target  会话标签     消息            kv 字段
```

规则：

1. **时间戳**：本地时区 `YYYY-MM-DD HH:MM:SS.mmm`（用户排障读本地时间最直觉；不做 UTC）。由后端 Hub 统一打戳，前端上报不带时间。
2. **级别**：定宽 5 字符左对齐（`INFO ` / `WARN ` / `ERROR` / `DEBUG`），保证列对齐。
3. **target**：模块短名，取 §4 目录（`app`/`ssh`/`sftp`/`metrics`/`ipc`），不暴露 Rust 模块路径细节。
4. **会话标签**：仅会话相关事件携带，取 session uuid 前 6 位（`[a1b2c3]`），全局事件省略该段。
5. **消息**：中文短语，面向用户可读（面板直接展示原文），不含换行；消息内必须换行时转义为字面 `\n`。
6. **kv**：`key=value` 空格分隔，结构化尾巴。值含空格或特殊字符用双引号包裹，值内双引号转义 `\"`。JSONL 内数值一律不带单位（单位进 key 名：`elapsed_ms`、`speed_bps`（值单位 B/s）、`rtt_ms`），保证 jq 可直接数值比较；**面板显示层**对字节键（size/total/transferred/resume_from 等）转换为可读单位（如 `524684643` → `500.37MB`）、`speed_bps` 显示为 `speed=11.84MB/s`、`pct` 追加 `%`，仅影响展示不改文件内容。
7. **单行一条**，面板列表按行渲染。单条长度上限 2KB，超出截断并追加 `…[truncated]`。

示例（一次完整连接）：

```text
2026-10-05 14:23:00.812 [INFO ] [ssh] [a1b2c3] 正在连接 host=192.168.1.10 port=22 user=deploy auth=publickey
2026-10-05 14:23:00.835 [DEBUG] [ssh] [a1b2c3] TCP 已连接 rtt_ms=23
2026-10-05 14:23:01.002 [INFO ] [ssh] [a1b2c3] SSH 握手完成 server_banner="SSH-2.0-OpenSSH_9.6p1" kex=curve25519-sha256 host_key=ssh-ed25519 cipher=chacha20-poly1305
2026-10-05 14:23:01.123 [INFO ] [ssh] [a1b2c3] 认证成功 method=publickey user=deploy
2026-10-05 14:23:01.240 [INFO ] [ssh] [a1b2c3] 会话已建立 term=xterm-256color cols=120 rows=32 enc=UTF-8
```

面板渲染细节：复用现有 `log-row`（time / level / msg 三段），time 显示 `HH:MM:SS`（当天）或 `MM-DD HH:MM:SS`（跨天）；msg 段渲染 `消息 + kv`（kv 弱化颜色）。级别着色沿用现有 `.log-row.INFO/.WARN/.ERROR/.DEBUG` 样式。

> 面板渲染效果样例见 `docs/samples/panel_sample.md`（含全部级别/级别筛选/仅 ERROR/搜索高亮/跨天显示五种视图）。

### 5.3 IPC 载荷（与文件 JSONL 同构）

Channel 推送与 invoke 上报均传结构化 JSON，**字段与 §5.1 文件 JSONL 完全一致**——落盘线程收到后直接序列化写盘，零拼装；面板渲染处格式化为 §5.2 文本行。单一 schema 从源头杜绝两种格式字段漂移：

```jsonc
// AppLogEntry（snake_case，与指标类帧一致）
{ "seq": 1024, "ts": "2026-10-05T14:23:01.123+08:00", "level": "info", "target": "ssh",
  "event_id": "ssh.auth.success", "sid": "a1b2c3", "msg": "认证成功",
  "kv": { "method": "publickey", "user": "deploy" } }
```

**前端上报入参白名单**：`report_app_log` **仅接受** `level, eventId, msg, kv` 四个字段，前端不得传入 `seq`、`ts`、`sid`（服务端强制生成/覆盖）。结构体上单独定义 `ReportAppLogInput`，不复用完整 `AppLogEntry`，从类型层面杜绝前端伪造序列号、时间戳或会话标签。

## 6. 持久化设计

### 6.1 目录与命名

| 项 | 规则 |
|---|---|
| 默认目录 | 平台应用日志目录（`app_log_dir`）：macOS `~/Library/Logs/com.rhost.app/`、Linux `~/.local/share/com.rhost.app/logs/`、Windows `%LOCALAPPDATA%\com.rhost.app\logs\`。与现有 dev 构建 plugin-log 落点一致 |
| `logStoragePath` | 语义由「会话日志目录」**修正为「应用日志目录」**；留空 = 默认目录；支持 `~` 展开。默认值由 `~/ssh-logs` 改为空串（避免与会话录制目录混淆；旧值非空时保留生效）。设置面板输入框留空时以 placeholder 展示本机解析后的默认绝对路径——经 `AppConfigSnapshot.defaultLogDir`（`load_app_config`）由后端唯一来源下发，前端不按平台自行拼接 |
| 文件名 | 固定模板 `rhost_app_${date}.log`（`logNaming` 已 disabled，不开放自定义）。`${date}` 按切割策略格式化：daily `YYYYMMDD`、weekly `YYYY-Www`（ISO 周）、monthly `YYYYMM`、none 固定 `rhost_app.log` |

### 6.2 写入策略

- 落盘线程消费 mpsc 队列，**攒批 flush**：满 64KB 或距上次 flush 超过 200ms 时写盘；`OpenOptions append + create`。
- **不做每条 fsync**（断电丢最后 200ms 可接受）；应用退出流程中显式 flush + join 落盘线程。**Tauri 退出钩子顺序**：`RunEvent::ExitRequested` 中先向 Hub 发关闭信号 → Hub 等落盘线程 flush 完并 join → 再允许 Runtime 退出，避免子线程被粗暴终止导致日志截断。
- **写失败分级降级**：
  - 磁盘满 / 无写入权限：一次性向 stderr 输出并往面板注入 `app.log.persist_failed` ERROR，随后关闭持久化（logPersist 视同关闭），**不再反复尝试写盘**；
  - 运行中日志文件被外部删除（Linux/macOS 句柄仍有效但文件已消失）：下次写批前检测文件句柄是否仍关联磁盘文件（`fstat` 对比 inode），若文件已消失则尝试重新 `open`；重开失败按磁盘满处理关闭持久化；
  - 权限中途被修改：同磁盘满处理。
- **崩溃前 ERROR 保护**：ERROR 级别日志在入 mpsc 队列的同时，**一份副本直接同步追加 stderr**（不攒批、不依赖 mpsc），保证崩溃前最后一批 ERROR 至少落在 stderr；配合 panic hook 兜底。

### 6.3 轮转（切割）

- **懒切换**：落盘线程每次写批前比对当前系统时间与当前文件的切割点，跨点则关闭旧句柄、按新 `${date}` 开新文件。无独立定时器，无流量零 IO。
- 切换失败（新文件创建失败）保持写旧文件并报 `app.log.persist_failed` ERROR。
- **系统时间回拨处理**：轮转切割以系统单调时钟（`Instant`）为辅助判断，不完全依赖墙钟。当检测到墙钟时间相对上次记录回跳超过 1s，生成特殊后缀日志文件 `rhost_app_YYYYMMDD_reclock.log`，避免覆盖旧日期日志；同时产出 `app.clock.rollback` WARN。

### 6.4 清理（保留策略）

- 时机：**启动时一次 + 运行中每 1 小时一次**（tokio interval，挂载在 Hub 内）。
- 规则：扫描日志目录中匹配 `rhost_app_*.log` 的文件，按 mtime 倒序——**`logRetentionDays` 与 `logMaxFiles` 任一满足即删除**（先按过期天数删，再按超量文件数删，两条规则独立判断，不互斥）。当前正在写入的文件永不删除。
- 清理动作本身记 INFO：`日志清理` removed=n freed_kb。

### 6.5 JSONL 损坏容错

- 读取历史日志文件（诊断导出、面板 replay 旧文件）时，遇到无法解析的行直接跳过，不中断整个文件解析；同时在导出结果或面板中注入一条 `app.log.corrupt_line` WARN（带文件名与行号）。

## 7. 内存缓冲与面板推送（IPC 协议）

### 7.1 后端环形缓冲

- `VecDeque<AppLogEntry>`，容量 = `logMaxLines`（默认 5000，热生效：调小时立即截头）。
- 每条分配 `seq: u64` 全局单调递增（跨重启归零即可，面板订阅时以 replay 全量重建，不依赖 seq 持久化）。

**`seq` 的四个职责**（不因"不用窗口续传"就省掉）：

1. **面板订阅的续传游标**：replay（环形缓冲快照）与增量推送之间天然存在窗口期，客户端记下最后 `seq=N`，重订阅时带 `since_id=N`，Hub 补发 `seq>N` 的增量，不漏不爆；
2. **同毫秒时间戳的排序仲裁**：后端宏、前端上报两路汇入，同毫秒多条日志常见，`ts` 相同无法定序，`seq` 提供唯一全局顺序；
3. **丢失检测**：mpsc 队列满时会丢 DEBUG/INFO（§9 背压策略），消费方发现 seq 从 1024 直接跳到 1030，即知丢了 5 条；
4. **面板 v-for key**：替代现有 DockPanel mock 的本地 `logSeq`，全局唯一不冲突。

**`sinceId` 越界处理**：Hub 维护当前缓冲最小可用 seq（`min_seq`）。当 `sinceId < min_seq`（客户端游标已被逐出缓冲），不再增量补发，而是返回全量现存缓冲数据，并在该批次附带元信息 `{ "lost_because": "buffer_evict", "from_seq": min_seq }`；前端面板可提示「部分更早日志已从内存缓冲区丢弃」。

### 7.2 IPC 命令（ipc.rs 新增，camelCase 入参，沿用既有风格）

| 命令 | 入参 | 返回 | 说明 |
|---|---|---|---|
| `subscribe_app_logs` | `channel: Channel<LogBatch>`、`sinceId: Option<u64>` | `()` | 先推 replay 批（`seq > sinceId` 的部分；`sinceId=None` 或 `sinceId < min_seq` 推全量并附带 `lost_because` 元信息），再持续推增量批（攒 100ms 或 20 条，单批硬上限 50 条防爆量）。**支持多订阅者广播**：Hub 维护订阅者列表，send 失败即移除该订阅 |
| `report_app_log` | `level, eventId, msg, kv?` | `()` | 前端日志上报入口。服务端校验 level 白名单、eventId 必须是 `web.*` 域、msg 长度 ≤ 2KB，target 固定填 `web`；`seq`/`ts`/`sid` 由服务端生成，前端不得传入 |
| `clear_app_log_buffer` | `()` | `()` | 清空内存缓冲（面板「清空日志」按钮）；**只清内存，不删磁盘文件** |
| `reveal_log_dir` | `()` | `()` | 系统文件管理器打开日志目录（复用 tauri-plugin-opener） |

`LogBatch` 结构：

```jsonc
// 单批推送，entries 按 seq 升序，最多 50 条
{ "entries": [ { "seq": 1024, ... } ],
  "lost_because": "buffer_evict",  // 可选，仅 replay 批次携带
  "from_seq": 4001                  // 可选，仅 replay 批次携带
}
```

### 7.3 前端接入

- 新增 `frontend/src/stores/applog.ts`：模块级单例 `logs`（ref 数组）、`subscribe()`（Workbench 挂载时调用一次，Channel 回调批量 push，按 `logMaxLines` 截断）、`clear()`、`report(level, msg, kv?)`（fire-and-forget，`isTauri()` 守卫，非 Tauri 环境降级 `console`）。
- `DockPanel.vue`：
  - 删除 `LOG_SEED`、mock 定时器与本地 `pushLog`；数据改绑 `stores/applog.ts` 的 `logs`；
  - 现有 SFTP 事件的 `pushLog(...)` 调用点平移为 `applog.report('info', ...)`（文案不变，kv 补齐 size/elapsed 等）；
  - 行数上限改读 `savedSettings.logMaxLines`（替换硬编码 500）；
  - 「清空日志」按钮改调 `clear_app_log_buffer`；
  - 工具栏新增「打开日志目录」按钮（`reveal_log_dir`）与采集关闭态占位（`logCollect=false` 时显示「日志采集已关闭」并停止渲染新行）。
- 级别筛选保持**显示侧过滤**（采集侧已被 `logLevel` 过滤过一道，面板筛选只是视图层再筛），现有 `levelFilter` 逻辑不变。
- `logCollect=false` 时：后端 Hub 整体短路（不缓冲、不推送、不落盘），仅 ERROR 落 stderr。

## 8. 安全与脱敏规范

| 数据 | 规则 |
|---|---|
| 密码 / passphrase | 任何字段永不出现；认证事件只记 method 与服务端 reason |
| 私钥 | 只记路径与指纹（SHA256），不记内容 |
| 主机/路径/用户名 | 允许记录（排障必需；用户本地文件） |
| 设置变更 | 敏感键（密码类、env 值）只记 `key changed`，不记值 |
| pasteGuard 拦截 | 只记 `行数/长度/是否含控制字符` 特征，不记内容 |
| 上报入口 | `report_app_log` 服务端侧二次校验：msg 不含 `\x00`–`\x08` 控制字符，超长截断 |

**敏感配置 key 静态集合**：后端维护 `SENSITIVE_SETTING_KEYS: &[&str]` 常量（如 `password`、`passphrase`、`private_key`、`token`、`env.*.value` 等），`app.settings.change` 埋点代码统一查表脱敏，不写分散 if 判断，防止新增设置项时漏加敏感字段。

**PTY 隔离硬约束**：所有 `log::*!` 调用**禁止**将 PTY 原始 buffer 作为参数传入；PTY 相关 target 命名统一加 `_pty` 前缀（如 `ssh_pty`），单元测试审计禁止该前缀 target 的日志进入 Hub。

**日志埋点零副作用**：所有日志埋点代码**禁止执行任何 IO、网络调用、锁等待**，只读取内存中已存在的变量；埋点本身不能失败、不能阻塞业务线程。

## 9. 性能预算

| 项 | 预算 |
|---|---|
| 单条日志处理（过滤+打戳+入队） | < 0.1ms，不触达 tokio 阻塞点 |
| 落盘线程 | 独立 std thread；mpsc 有界 1024，高水位 80% 丢 DEBUG/INFO；硬满时 WARN/ERROR 同步写 stderr + 计数，不阻塞 |
| Channel 推送 | 未订阅零开销；订阅时攒批 100ms/20 条（单批硬上限 50 条），面板渲染 5000 行内滚动 60fps（现有 log-list 已验证） |
| 启动清理扫描 | 目录文件数 ≤ logMaxFiles 上限量级，一次性 readdir，< 10ms |
| ERROR 崩溃保护 | ERROR 级同步写 stderr（不攒批），延迟 < 1ms，独立于 mpsc 队列 |

## 10. 设置项落地对照

### 10.1 设置键与生效方式

| 设置键（前端/IPC，camelCase） | `logs` 节字段（snake_case） | 消费方 | 生效方式 |
|---|---|---|---|
| `logCollect` | `collect` | 后端 Hub 总开关 + 面板占位 | 热生效（`saveSettings` → invoke `set_log_config`） |
| `logLevel` | `level` | 后端 Hub 过滤（log crate max_level 同步） | 热生效 |
| `logMaxLines` | `max_lines` | 后端环形缓冲容量 + 面板截断 | 热生效，调小立即截头 |
| `logPersist` | `persist` | 落盘线程启停 | 热生效（关闭即 flush 后停写） |
| `logStoragePath` | `storage_path` | 落盘目录 | **重启生效**（切换目录需重建句柄与清理作用域，不做热切换）；Settings UI 在该项旁加醒目提示「修改后需重启应用生效」，保存时检测变更弹窗提醒 |
| `logNaming` | —（固定模板，不落配置） | UI 已 disabled | — |
| `logRotate` | `rotate` | 落盘线程切割点计算 | 热生效（下一切割点按新策略） |
| `logMaxFiles` / `logRetentionDays` | `max_files` / `retention_days` | 清理任务 | 热生效（下一轮清理采用） |

### 10.2 两份配置存储与同步方向

同一份日志设置在两处各有角色，**后端不反向读 localStorage**：

| 存储 | 位置 | 角色 | 写入时机 |
|---|---|---|---|
| WebView localStorage | 前端 `SETTINGS_KEY` | UI 真值来源（全部应用设置，不止日志） | 前端 `saveSettings` |
| 后端 `app_config.json` | Tauri app config dir | 冷启动载体：init 必须在前端通道就绪前确定日志目录；同时是**未来整体配置导入导出的数据源** | 前端 `set_log_config` 时后端读—改—写 |

同步通道与时机：`set_log_config` invoke，两处触发——① SettingsModal `saveSettings` 后；② App.vue `onMounted` 启动时自动同步一次（覆盖升级场景：用户从未在新版打开过设置，localStorage 中既有值也会落入后端文件）。IPC 入参 `LogConfigPayload` 为 camelCase，后端转换后写入 snake_case 的 `logs` 节。

### 10.3 `app_config.json` 结构

定位：后端侧**整体应用配置文件**，按节组织；当前仅 `logs` 一节，后续在同一文件内增加 `terminal` / `ui` 等节，整体导入导出以此文件为数据源。

路径（Tauri `app_config_dir`）：macOS `~/Library/Application Support/com.rhost.app/app_config.json`；Linux `~/.config/com.rhost.app/app_config.json`；Windows `%APPDATA%/com.rhost.app/app_config.json`。

```json
{
  "version": 1,
  "logs": {
    "collect": true,
    "level": "info",
    "max_lines": 5000,
    "persist": true,
    "storage_path": "",
    "rotate": "daily",
    "max_files": 100,
    "retention_days": 30
  }
}
```

结构与兼容规则：

- 顶层 `version`（当前 `1`）：结构版本号，后续 schema 迁移依据；`load` 预留按版本迁移的位置；
- 分节组织：每个功能域一个顶层节；`set_log_config` 经**读—改—写只替换 `logs` 节**，其他节不受影响（同目录 `.tmp` + rename 原子写，崩溃不留半截 JSON，父目录自动创建）；
- 全字段容错：顶层缺节、节内缺字段、文件缺失/JSON 损坏均回退默认值；非法 `level` 回 `info`，`max_lines`/`max_files` 为 0 夹到 1；
- **前向兼容**：顶层未知节与 `logs` 节内未知字段均经 `#[serde(flatten)]` 原样保留——新版本写入的配置被旧版本读改写时不丢失（导入高版本导出文件的基础）；
- 启动读取：lib.rs setup 最前 `persisted::load` → 取 `logs` 节构造 `HubConfig` → `applog::init`；
- 写失败处理：不回滚本次热更新（本运行仍按新配置），发 `app.log.config_persist_failed` WARN（kv：path、err），语义即「重启后该设置不会保留」；变更任一字段（含 `logStoragePath`）发 `app.settings.change` INFO 审计事件；
- 对外 API：`load() -> AppConfig { logs }`（整体读取）、`save(&AppConfig)`（整体写入，供未来导入落盘）、`save_logs(&HubConfig)`（仅更新日志节）。

## 11. 落地阶段

| 阶段 | 内容 | 验收 |
|---|---|---|
| P0 | `applog` 模块：`log::Log` 实现 + Hub + 环形缓冲 + 落盘线程（含三级背压）+ 轮转/清理 + panic hook 兜底；lib.rs 替换 tauri-plugin-log；`set_log_config` 接入设置 | release 构建落盘文件按天切割；改级别热生效；启动清理过期文件；单测覆盖切割点计算、脱敏、背压丢弃与 stderr 同步写 |
| P1 | IPC 四命令 + `stores/applog.ts` + DockPanel 去 mock（数据接入、清空、打开目录、采集关闭占位、logStoragePath 重启提示） | 面板显示 = 磁盘文件内容一致；SFTP 传输全流程事件可见；清空只清内存；修改 logStoragePath 弹重启提示 |
| P2 | SSH 生命周期事件埋点（session.rs 按 §4.2 补 `log::` 调用）；前端 IPC 错误统一上报；多订阅者协议验证 | 连接成功/失败/重连全链路面板可见；断网重连 WARN 序列正确；面板与诊断导出可同时订阅 |
| P3 | 慢 IPC 统计、`dropped` 计数透出、诊断导出（打包最近 3 个日志文件为 zip，冻结轮转 + 二次脱敏扫描 + zip 大小上限 10MB） | 慢调用 WARN 可见；导出 zip 含脱敏后日志且不含半截 JSONL |

## 12. 测试与验收

- **单测（Rust）**：JSONL 序列化（字段完整、event_id 必填校验、可选键省略、8KB 截断 `_truncated` 为顶层字段、msg 换行转义、JSON 截断不破坏转义字符）、面板文本格式化（kv 引号转义、2KB 截断）、切割点计算（daily/weekly/monthly 跨点）、清理策略（过期+超量任一满足、当前文件豁免）、级别过滤、环形缓冲截头（含 min_seq 更新）、event_id 常量表与文档 §4 表格一致性（编译期解析 markdown 校验）、背压策略（高水位丢 DEBUG/INFO、硬满 WARN/ERROR 写 stderr、dropped 计数定时上报）、sinceId < min_seq 时返回 lost_because 元信息、PTY `_pty` 前缀 target 被 Hub 拒绝。
- **e2e**：docker SSH 容器连接 → 面板出现 §4.2 完整序列；SFTP 传输 → 入队/开始/完成事件 kv 正确；改 `logLevel=warn` 后 DEBUG/INFO 立即停止；`logPersist=false` 时目录无新增；落盘文件每行 `jq` 可解析且字段与面板显示一致；模拟磁盘满 → 一次性 ERROR 后关闭持久化不再重试；模拟 panic → panic 信息写入 stderr 并尝试追加日志文件。
- **手动**：面板 5000 行滚动/搜索/自动跟随流畅；「打开日志目录」落点正确；应用退出后日志文件无半截行（flush 完整）；修改 logStoragePath 后 UI 提示重启生效；诊断导出 zip 可正常解压且无半截 JSONL。

---

## 13. 评审回应与决策清单

| 评审项 | 决策 | 说明 |
|---|---|---|
| **mpsc 背压：WARN/ERROR 队列满后无降级** | ✅ 采纳 | §3 决策 4 改为三级背压：高水位丢 DEBUG/INFO、硬满 WARN/ERROR 同步写 stderr + 计数、stderr 限流防风暴 |
| **dropped 计数依赖下一条 WARN 触发，可能静默** | ✅ 采纳 | §3 决策 4 改为 Hub 内 5s 定时器主动检查并产出 `app.log.dropped` |
| **单订阅者隐式替换，旧订阅静默断流** | ✅ 采纳 | §3 决策 3 改为多订阅者广播，Hub 维护订阅者列表，send 失败即移除；不采用单订阅者替换 |
| **logStoragePath 仅重启生效，用户无感知** | ✅ 采纳 | §10 增加 Settings UI 醒目提示「修改后需重启应用生效」+ 保存时弹窗提醒 |
| **JSONL 截断 `_truncated` 未说明层级** | ✅ 采纳 | §5.1 明确为顶层字段（不属于 kv），保证 `jq` 筛选不遗漏；截断保证 JSON 合法 |
| **环形缓冲调小截头后 sinceId 越界无定义** | ✅ 采纳 | §7.1 新增 `min_seq` 维护与 `lost_because` 元信息协议；§7.2 `subscribe_app_logs` 支持 `sinceId < min_seq` 时推全量 + 元信息 |
| **应用崩溃时内存缓冲日志全部丢失** | ✅ 部分采纳 | §6.2 增加 ERROR 级同步写 stderr（不攒批）；panic hook 兜底写 stderr + 尝试追加日志文件（不走 Hub 管线） |
| **日志轮转异常处理不完整** | ✅ 采纳 | §6.2/§6.3 补充磁盘满/文件被删/权限变更/时间回拨的降级与特殊文件命名 |
| **诊断导出 zip 边界缺失** | ✅ 采纳 | §11 P3 补充：导出时冻结轮转、二次脱敏扫描、zip 大小上限 10MB |
| **第三方 crates 日志 event_id 缺失** | ✅ 采纳 | §4.4 新增 `external.raw_log` event_id，原始 crate target 放 kv.crate_target；可按 crate 单独配置级别过滤 |
| **report_app_log 前端可伪造 seq/ts/sid** | ✅ 采纳 | §5.3/§7.2 明确 `ReportAppLogInput` 白名单（仅 level/eventId/msg/kv），seq/ts/sid 服务端生成，类型层面隔离 |
| **系统时间回拨导致日志文件命名混乱** | ✅ 采纳 | §6.3 新增单调时钟辅助判断 + `reclock` 后缀文件 + `app.clock.rollback` WARN |
| **JSONL 损坏行无容错** | ✅ 采纳 | §6.5 新增损坏行跳过 + `app.log.corrupt_line` WARN |
| **AppLogEntry 单结构体三用（内存/IPC/JSONL）** | ✅ 采纳 | §5.3 明确内存模型与 IPC 入参分离：`AppLogEntry`（完整）vs `ReportAppLogInput`（白名单） |
| **落盘 mpsc 拆双通道（普通/高危）** | ❌ 不采纳 | 三级背压已解决高危日志被淹没问题；双通道增加复杂度收益有限，维持单通道 |
| **event_id 编译期校验** | ✅ 采纳 | §12 单测增加编译期解析 markdown 表格与 `events.rs` 常量一致性校验 |
| **环形缓冲内存监控** | 📝 备注 | §9 性能预算中不额外增加运行时指标，P3 诊断导出时可通过 `dropped` 计数与日志总量估算 |
| **PTY 输出误入日志管线** | ✅ 采纳 | §8 新增 `_pty` 前缀 target 约定 + 单元测试审计禁止该前缀进入 Hub |
| **日志埋点执行 IO/网络/锁等待** | ✅ 采纳 | §8 新增「埋点零副作用」硬约束 |
| **敏感配置 key 分散 if 判断易漏** | ✅ 采纳 | §8 新增 `SENSITIVE_SETTING_KEYS` 静态集合常量统一查表 |
| **前端 5000 行 DOM 渲染性能天花板** | 📝 备注 | 当前 DockPanel log-list 已验证 5000 行 60fps；虚拟滚动记为 P2 后续优化，文档 §9 已备注单批 50 条上限 |
| **Channel 单批 20 条可能爆发** | ✅ 采纳 | §7.2/§9 增加单批硬上限 50 条 |
| **日志健康指标暴露** | 📝 备注 | P3 诊断导出已包含 dropped 计数；不额外增加实时指标面板 |
| **仅 stderr 便携模式** | ❌ 不采纳 | `logPersist=false` 已实现相同效果（仅 stderr），不新增独立模式 |
| **清理规则「先过期后超量」语义模糊** | ✅ 采纳 | §6.4 明确「任一满足即删除」，两条规则独立判断不互斥 |
| **Tauri 退出钩子顺序导致日志截断** | ✅ 采纳 | §6.2 明确 `ExitRequested` 中先关闭 Hub → flush → join → 再允许 Runtime 退出 |
