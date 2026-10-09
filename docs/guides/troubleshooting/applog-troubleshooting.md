# 应用日志排查手册

> status: 与 `applog-design.md` 配套
>
> 面向：开发者、测试、高级用户
>
> 前提：已按 `applog-design.md` 落地下文所述事件

## 1. 排查基础

### 1.1 五个关键字段

| 字段 | 作用 | 排查中怎么用它 |
|---|---|---|
| `sid` | 会话 uuid 前 6 位，一次 SSH 连接全链路共用 | **分组键**。一次连接从 `connect.start` 到 `disconnect` 的 sid 不变，jq `select(.sid=="a1b2c3")` 拉出完整链路 |
| `seq` | 全局单调递增序号 | **时序仲裁**。同毫秒内多条日志按 seq 排；seq 跳号（如 1024→1030）说明中间丢了 6 条，应触发 `app.log.dropped` |
| `event_id` | 事件类型标识（如 `ssh.auth.failed`） | **阶段定位**。缺失哪个 event_id 就知道卡在哪段——有 `auth.start` 无 `auth.success` = 认证卡住 |
| `target` | 模块来源（`app`/`ssh`/`sftp`/`metrics`/`ipc`/`web`） | **范围过滤**。只看 SSH 层事件 `select(.target=="ssh")` |
| `level` | `debug`/`info`/`warn`/`error` | **优先级**。先看 ERROR，再看 WARN，DEBUG 用于深入 |

### 1.2 排查入口选择

| 场景 | 推荐入口 |
|---|---|
| 快速定位某次连接的问题 | 面板搜索 sid → 级别筛选 INFO+ |
| 分析跨天/跨文件的完整链路 | `jq` 按 sid 聚合多文件 |
| 批量统计（如"今天有多少次认证失败"） | `jq` + `sort`/`uniq -c` |
| 崩溃后无面板可用 | 直接读 JSONL 文件，`jq` 或 `grep` |

---

## 2. 命令行排查（jq 模式）

### 2.1 基础过滤

```bash
# 拉取某会话全部日志（sid = a1b2c3）
jq 'select(.sid=="a1b2c3")' rhost_app_20261005.log

# 按 seq 排序（多文件时确保时序）
jq -s 'map(select(.sid=="a1b2c3")) | sort_by(.seq)' rhost_app_*.log

# 只看 ERROR + WARN（快速定位故障点）
jq 'select(.sid=="a1b2c3" and (.level=="error" or .level=="warn"))' rhost_app_*.log

# 只看关键阶段事件（隐藏 DEBUG 噪音）
jq 'select(.sid=="a1b2c3" and .level!="debug")' rhost_app_*.log
```

### 2.2 按 event_id 追踪

```bash
# 追踪一次完整连接的所有阶段事件
jq 'select(.sid=="a1b2c3" and (.event_id | startswith("ssh."))) | {seq, event_id, msg}' rhost_app_*.log

# 查看所有上传操作的结果分布
jq 'select(.event_id | startswith("sftp.transfer.")) | {event_id, sid, kv}' rhost_app_*.log | jq -s 'group_by(.event_id) | map({event_id: .[0].event_id, count: length})'

# 查找所有认证失败及其原因
jq 'select(.event_id=="ssh.auth.failed") | {ts, sid, kv}' rhost_app_*.log
```

### 2.3 时间范围查询

```bash
# 某主机 + 某时间段 + 排除 DEBUG
jq 'select(.kv.host=="192.168.1.10" and .ts>="2026-10-05T09:40:00+08:00" and .level!="debug")' rhost_app_*.log

# 跨天合并后按时间排序
jq -s 'add | map(select(.ts>="2026-10-05T00:00:00" and .ts<"2026-10-06T00:00:00")) | sort_by(.seq)' rhost_app_20261005.log rhost_app_20261006.log
```

### 2.4 聚合统计

```bash
# 今天各 event_id 出现次数（快速看活跃操作）
jq 'select(.ts | startswith("2026-10-05")) | .event_id' rhost_app_20261005.log | sort | uniq -c | sort -rn

# 各会话连接时长分布（disconnect.uptime_s）
jq 'select(.event_id=="ssh.disconnect") | {sid, uptime_s: .kv.uptime_s}' rhost_app_*.log

# 上传失败的错误类型分布
jq 'select(.event_id=="sftp.transfer.failed") | .kv.err' rhost_app_*.log | sort | uniq -c | sort -rn
```

---

## 3. 面板排查（DockPanel 日志 tab）

### 3.1 基本操作

| 操作 | 效果 |
|---|---|
| 搜索框输入 `a1b2c3` | 过滤出 sid 为 a1b2c3 的全部事件 |
| 级别筛选切到 `INFO+` | 隐藏 DEBUG，只看关键阶段 |
| 级别筛选切到 `ERROR` | 只看错误，快速定位故障 |
| 搜索框输入 `event_id:ssh.auth.failed` | 按 event_id 精确过滤（如果面板支持） |
| 点击时间戳 | 自动滚动到该位置，查看前后上下文 |
| 自动滚动开关 | 追新日志时打开；分析历史时关闭 |

### 3.2 排查模式

**模式 A：追踪某次连接**
1. 面板搜索该连接的 sid
2. 级别筛 INFO+，看阶段是否完整
3. 若有 ERROR，切到 ERROR 只看故障点
4. 发现卡在哪个 event_id 后，切回 ALL 查看该 event_id 前后的 DEBUG 日志

**模式 B：批量查看失败**
1. 级别筛 ERROR
2. 逐条查看，记录 sid
3. 对每个 sid 切回 ALL，追溯完整链路

---

## 4. 按场景排查

### 4.1 SSH 连接不上

**完整阶段检查清单**：

```
ssh.connect.start      ← 必须存在，发起连接
  ↓
ssh.connect.tcp        ← 缺失 = 网络层问题（超时/拒绝/不可达）
  ↓
ssh.handshake.start    ← 缺失 = TCP 建立后 SSH 服务无响应
  ↓
ssh.handshake.complete ← 缺失 = 算法协商失败（看 ssh.handshake.failed）
  ↓
ssh.auth.start         ← 缺失 = 握手后卡住
  ↓
ssh.auth.success       ← 缺失 = 认证失败（看 ssh.auth.failed reason）
  ↓
ssh.session.create     ← 缺失 = 认证通过但会话创建未开始
  ↓
ssh.session.ready      ← 缺失 = PTY 分配或 shell 启动卡住（看 ssh.session.failed）
```

**jq 命令**：

```bash
# 拉取某 sid 的全部 SSH 阶段事件
jq 'select(.sid=="a1b2c3" and (.event_id | startswith("ssh."))) | {seq, ts, event_id, level, msg}' rhost_app_*.log

# 快速检查该 sid 是否到达 session.ready
jq 'select(.sid=="a1b2c3" and .event_id=="ssh.session.ready")' rhost_app_*.log
# 无输出 = 连接未完成，看 ERROR 定位
jq 'select(.sid=="a1b2c3" and .level=="error")' rhost_app_*.log
```

**常见故障模式**：

| 最后一条日志 | 含义 | 排查方向 |
|---|---|---|
| `ssh.connect.start` 后无后续 | 连接未发出或网络完全不通 | 检查本地网络、DNS、防火墙 |
| `ssh.connect.tcp` 后无 `handshake.start` | TCP 建立但 SSH 服务无 banner | 检查目标 22 端口是否被非 SSH 服务占用 |
| `ssh.handshake.start` 后无 `handshake.complete` | 算法协商失败 | 检查服务端支持的 kex/cipher 列表 |
| `ssh.auth.start` 后无 `auth.success` | 认证失败或卡住 | 查看 `ssh.auth.failed` reason；检查密钥权限、agent |
| `ssh.session.create` 后无 `session.ready` | PTY 分配或 shell 启动失败 | 检查服务端 /bin/bash 是否存在；检查 PTY 配额 |

### 4.2 认证失败

```bash
# 查看所有认证失败事件
jq 'select(.event_id=="ssh.auth.failed") | {ts, sid, kv}' rhost_app_*.log

# 查看某次认证失败的完整链路（向上追溯 connect.start，向下看是否重试）
jq -s 'map(select(.sid=="a1b2c3")) | sort_by(.seq)' rhost_app_*.log
```

**关键 kv 字段**：
- `method`：失败的方法（publickey/password/agent）
- `reason`：服务端返回文本（**不含密码**）

**注意**：密码本身不会出现在任何日志中。

### 4.3 SFTP 传输问题

**传输七事件链路**：

```
sftp.transfer.enqueue   ← 入队（看 ts 差 = 队列等待时长）
  ↓
sftp.transfer.start     ← 开始（看 resume_from 是否续传）
  ↓
sftp.transfer.complete  ← 成功
  或
sftp.transfer.failed    ← 失败（看 err）
  或
sftp.transfer.cancel    ← 用户取消
```

**jq 命令**：

```bash
# 查看某文件的所有传输事件
jq 'select(.sid=="a1b2c3" and .kv.name=="deploy.sh") | {seq, event_id, kv}' rhost_app_*.log

# 查看是否有续传（resume_from > 0）
jq 'select(.event_id=="sftp.transfer.start" and .kv.resume_from > 0) | {sid, name: .kv.name, resume_from: .kv.resume_from}' rhost_app_*.log

# 上传失败的错误分布
jq 'select(.event_id=="sftp.transfer.failed") | {sid, name: .kv.name, err: .kv.err, transferred: .kv.transferred, total: .kv.total}' rhost_app_*.log
```

**常见故障模式**：

| 现象 | 排查 |
|---|---|
| `enqueue` 后无 `start` | 队列卡住或前端未触发执行，检查 `sftp.transfer.start` 缺失的 sid |
| `start` 后无 `complete`/`failed`/`cancel` | 传输中途断连，查看同一 sid 的 `ssh.disconnect` 或 `ssh.reconnect` |
| `failed` with `No space left on device` | 远程磁盘满 |
| `failed` with `Permission denied` | 远程目录无写权限 |
| `resume_from > 0` 且最终 `failed` | 续传后仍失败，可能是文件内容中途变更 |

### 4.4 应用启动卡住或失败

**启动阶段检查清单**：

```
app.boot.start          ← 必须存在，第一条日志
  ↓
app.boot.config_loaded  ← 缺失 = 配置解析死
  ↓
app.boot.log_dir_ready  ← 缺失 = 日志目录初始化失败
  ↓
app.log.cleanup         ← 可选（无过期文件则无）
  ↓
app.boot.keychain_ready ← 缺失 = 密钥存储连接失败
  ↓
app.boot.ready          ← 缺失 = 启动未完成，搜 app.boot.failed
```

**jq 命令**：

```bash
# 查看最近一次启动的所有阶段
jq 'select(.event_id | startswith("app.boot.")) | {seq, event_id, msg, kv}' rhost_app_*.log

# 查看是否有启动失败
jq 'select(.event_id=="app.boot.failed") | {ts, kv}' rhost_app_*.log

# 若 boot.ready 缺失，看最后一条 boot 事件是什么
jq 'select(.event_id | startswith("app.boot.")) | {seq, event_id}' rhost_app_*.log | tail -5
```

### 4.5 应用退出慢或卡死

**退出阶段检查清单**：

```
app.shutdown.start           ← ExitRequested 触发
  ↓
app.shutdown.sessions_closed ← 缺失 = 关会话卡住
  ↓
app.shutdown.log_flushed     ← 缺失 = 落盘 flush 卡住
  ↓
app.exit                     ← 必须最后一条
```

**jq 命令**：

```bash
# 查看最近一次退出的阶段
jq 'select(.event_id | startswith("app.shutdown.") or .event_id=="app.exit") | {seq, event_id, msg, kv}' rhost_app_*.log

# 如果 exit 缺失，看最后一条日志是什么
jq -s 'sort_by(.seq) | last' rhost_app_*.log
```

**故障模式**：

| 日志末尾停在 | 含义 | 排查方向 |
|---|---|---|
| `app.shutdown.start` | 卡在关闭会话 | 检查是否有会话的 `ssh.disconnect` 未产出 |
| `app.shutdown.sessions_closed` | 卡在落盘 flush | 检查磁盘 IO、文件句柄、是否有超大日志缓冲区 |
| `app.shutdown.log_flushed` | 卡在线程 join | 检查落盘线程是否死锁 |
| 无 `app.shutdown.start` | 未正常触发 ExitRequested | 检查是否被系统强制终止（SIGKILL）或崩溃 |

### 4.6 日志本身的问题

**日志丢失检测**：

```bash
# 检查 seq 是否连续（跳号 = 丢失）
jq '.seq' rhost_app_*.log | awk 'NR>1 && $1!=prev+1 {print "GAP between", prev, "and", $1, "lost", $1-prev-1, "lines"} {prev=$1}'

# 查看是否有 dropped 事件
jq 'select(.event_id=="app.log.dropped") | {ts, kv}' rhost_app_*.log
```

**落盘失败**：

```bash
# 查看所有落盘失败事件
jq 'select(.event_id=="app.log.persist_failed") | {ts, kv}' rhost_app_*.log

# 查看时间回拨事件
jq 'select(.event_id=="app.clock.rollback") | {ts, kv}' rhost_app_*.log
```

**JSONL 损坏**：

```bash
# 逐行验证 JSON 合法性
while IFS= read -r line; do echo "$line" | jq empty 2>/dev/null || echo "INVALID: $line"; done < rhost_app_20261005.log

# 查看是否有 corrupt_line 事件（由读取时自动产出）
jq 'select(.event_id=="app.log.corrupt_line") | {ts, kv}' rhost_app_*.log
```

---

## 5. 高级排查技巧

### 5.1 跨文件追踪（多天连接未断）

```bash
# 合并多天后按 sid + seq 排序
jq -s 'add | map(select(.sid=="a1b2c3")) | sort_by(.seq)' rhost_app_20261005.log rhost_app_20261006.log
```

### 5.2 关联分析（连接与传输）

```bash
# 查找所有"连接后立即上传"的模式
# 先提取 connect.start 和 transfer.start 的时间差
jq -s '
  group_by(.sid) |
  map(select(length > 1)) |
  map({
    sid: .[0].sid,
    connect_ts: (.[] | select(.event_id=="ssh.connect.start") | .ts),
    first_transfer_ts: (.[] | select(.event_id=="sftp.transfer.start") | .ts)
  }) |
  map(select(.first_transfer_ts != null))
' rhost_app_*.log
```

### 5.3 性能问题排查（慢 IPC）

```bash
# 查看所有慢 IPC 调用
jq 'select(.event_id=="ipc.slow_call") | {ts, kv}' rhost_app_*.log

# 查看 IPC 错误的分布
jq 'select(.event_id=="web.ipc.error") | {ts, kv}' rhost_app_*.log
```

### 5.4 前端未捕获异常

```bash
# 查看前端报错
jq 'select(.event_id=="web.unhandled_error") | {ts, kv}' rhost_app_*.log
```

---

## 6. 速查卡

| 我想查... | 用这条 jq |
|---|---|
| 某次连接的全部日志 | `jq 'select(.sid=="a1b2c3")'` |
| 今天所有 ERROR | `jq 'select(.level=="error")'` |
| 某 event_id 的所有出现 | `jq 'select(.event_id=="ssh.auth.failed")'` |
| 某文件的所有传输事件 | `jq 'select(.kv.name=="deploy.sh")'` |
| 连接时长分布 | `jq 'select(.event_id=="ssh.disconnect") \| .kv.uptime_s'` |
| 按 target 分组计数 | `jq '.target' \| sort \| uniq -c` |
| 最后 10 条日志 | `jq -s 'sort_by(.seq) \| .[-10:]'` |
| seq 是否连续 | `jq '.seq' \| awk 'NR>1&&$1!=p+1{print"GAP",p,$1}{p=$1}'` |

---

## 7. 注意事项

1. **sid 是运行时生成的**：重启应用后新连接的 sid 会变化，不要硬编码 sid 做长期监控；长期追踪用 `host+user+port` 组合或自定义标签
2. **seq 跨重启归零**：跨重启分析用 `(ts, seq)` 组合，或按文件 mtime 分段
3. **DEBUG 默认不采集**：排查时若 DEBUG 日志缺失，先检查 `logLevel` 设置是否为 `debug`
4. **敏感信息不在日志中**：密码、私钥、PTY 数据、文件内容永不记录；若排查需要这些内容，需通过其他手段获取
5. **logStoragePath 修改后需重启**：当天日志可能分布在旧目录和新目录，排查时注意文件路径
