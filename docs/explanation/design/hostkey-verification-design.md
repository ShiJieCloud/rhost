# SSH 主机密钥校验（known_hosts）设计方案

> description: SSH 首连指纹确认、known_hosts 持久化、密钥变更告警的两阶段校验设计
>
> created: 2026-10-08 09:28:16
>
> updated: 2026-10-08 12:13:11
>
> author: [sjzhao](https://github.com/ShiJieCloud/rhost)

## 1. 设计目标与硬约束

> scope: 本文覆盖 SSH 建连阶段的服务器主机密钥校验——首次连接指纹确认（TOFU）、指纹持久化、密钥不匹配告警与更新。
> 边界：不覆盖用户公钥认证（authorized_keys）、SSH CA 证书链校验、known_hosts 多指纹/哈希条目、指纹管理界面。
> 关联代码：`src-tauri/src/ssh/session.rs`、`src-tauri/src/ssh/mod.rs`、`src-tauri/src/ipc.rs`、`src-tauri/src/known_hosts.rs`（新建）、`frontend/src/stores/session.ts`、`frontend/src/composables/useHostKeyPrompt.ts`（新建）、`frontend/src/components/HostKeyModal.vue`（新建）、`frontend/src/App.vue`、`frontend/src/views/home/QuickConnectView.vue`

| 目标 | 量化口径 |
|---|---|
| 防 MITM | 指纹不匹配时连接必须失败，前端给出红色警告；不存在任何静默接受路径 |
| 零额外握手延迟 | 已存指纹命中路径不增加任何往返（校验发生在既有 `check_server_key` 回调内，内存比对 < 1ms） |
| TOFU 确认一致性 | 用户在弹窗确认的指纹必须与最终落盘指纹一致（重试握手竞态窗口关闭，见 §6.4） |
| 落盘可靠性 | `known_hosts.json` 原子写（临时文件 + rename），进程崩溃不产生半截文件 |
| 主流程零影响 | 校验逻辑任何异常（文件读失败等）不允许 panic；失败按 fail-closed 处理并降级为可解释错误 |

必须遵守的既有项目约束：

- 错误前缀即协议：沿用 `KEY_ENCRYPTED:` / `TUNNEL_*:` 模式，新增 `HOSTKEY_UNKNOWN:` / `HOSTKEY_MISMATCH:` 两个前缀，前端据此分流，**不得改为普通文案**；
- `ssh/` 模块除 manager 外不依赖 tauri（`ssh/mod.rs` 头注释）——known_hosts 文件 I/O 仅接收 `PathBuf`，`AppHandle` 只在 ipc 层出现；
- 后台任务禁止持锁 await；`ClientHandler` 回调同步段不把 `&mut self` 带入 async 块（沿用 `kex_done` 手法）；
- 日志永不打印敏感载荷；指纹属于公开信息可记录。

## 2. 必须遵守的既有项目约束

- 高频流走 Tauri Channel 二进制帧，低频控制走 invoke/JSON——本设计全部走 invoke 与错误字符串，不新增帧类型；
- serde 结构体统一 `#[serde(rename_all = "camelCase")]`，前端 camelCase；
- 配置持久化模式：JSON + version 字段 + 原子写（同 `src-tauri/src/store.rs` 的 `connections.json`）；
- russh 错误判断基于枚举变体匹配，禁止字符串解析；
- 连接代次门（`connectGen`）与自动重连（`reconnectAttemptMap`）语义不得破坏；
- 错误前缀 / invoke 入参变更须在同 PR 更新 `docs/architecture.md`、`docs/development.md` 的前缀清单（docs-spec「代码改动联动」）——本文新增 `HOSTKEY_UNKNOWN:` / `HOSTKEY_MISMATCH:` 即适用。

## 3. 现状盘点与差距

| 位置 | 现状 | 差距 |
|---|---|---|
| `src-tauri/src/ssh/session.rs` L299-L322 `check_server_key` | 计算 SHA256 指纹后仅记日志，无条件 `Ok(true)` | 不比对、不落盘、不询问用户 |
| `src-tauri/src/ssh/session.rs` L67-L69 | 注释「当前阶段接受任意服务器主机密钥」+ TODO | 待整体移除 |
| `src-tauri/src/ssh/mod.rs` `SshError` | 有 `KEY_ENCRYPTED:` 前缀先例 | 缺主机密钥两类错误变体 |
| `src-tauri/src/ipc.rs` `connect_ssh` / `test_ssh_connection` | 两条命令均无 `AppHandle` 参数、payload 无信任字段 | 无法读取/写入 known_hosts 文件；门禁重试无法构造 `HostKeyCheck` |
| 前端 `stores/session.ts` `connectBackend` | catch 仅置 `disconnectReason` 并按退避续试，无前缀分流 | 缺 `HOSTKEY_*:` 分流与确认弹窗重试 |
| 前端弹窗体系 | `usePasswordPrompt` + `PasswordModal`（App.vue 常驻挂载）模式成熟 | 缺指纹确认弹窗 |
| e2e `src-tauri/tests/ssh_e2e.rs` | `SessionConfig` 无校验字段 | 需显式传 `HostKeyPolicy::SkipForTests` 保持既有用例 |

结论：本设计补齐「校验决策、指纹持久化、用户确认交互、错误协议」四块，russh 回调挂载点已就绪。

## 4. 目标与非目标

### 目标

1. 首次连接（known_hosts 无记录）：连接失败返回 `HOSTKEY_UNKNOWN:`，前端弹窗展示指纹，用户「接受并保存」后重试一次，成功后指纹落盘；亦可「仅本次连接」——本次会话可信但不落盘，下次新建会话重新确认；
2. 再次连接：内存比对指纹，一致直接通过（无感）；
3. 密钥变更（指纹不匹配）：连接失败返回 `HOSTKEY_MISMATCH:`，前端红色警告弹窗，提供「断开」与「更新指纹并重连」两条路径；
4. 弹窗取消 / 拒绝：不落盘、不重试，连接保持失败态；
5. known_hosts 按文内 §6.1 schema 持久化于 `app_data_dir/known_hosts.json`。

### 非目标

- 不做 OpenSSH 标准格式 `~/.ssh/known_hosts` 的读写与共享，也不参与应用配置导入导出（见 `config-import-export.md` §5）：信任库是本机 TOFU 运行态数据，换机/重装后按首连重新确认即为正确安全语义，导出他机的指纹反而会绕过该机用户的当面核对；
- **不做 host 别名归并**：主机标识 = 用户输入的地址 + 端口，`my.server.com:22` 与 `1.2.3.4:22` 视为两条独立记录——同一服务器换别名连接会再次弹首连确认。这是**体验限制而非安全缺陷**（OpenSSH 同样按主机名分别存储），弹窗文案注明「主机标识为你输入的地址 + 端口」；多别名归并为后续迭代；
- 不做 hashed entries、通配符（`*.example.com`）、多指纹（轮换记录）、CA 证书信任链；
- 不提供「关闭校验」设置项（校验恒开）；
- 不做指纹管理界面（删除/查看全部记录），清理靠删除文件；
- 自动重连路径不弹窗（见 §6.6）。

## 5. 总体架构

两阶段确认：**第一次连接故意失败并携带指纹 → 前端弹窗 → 确认后带 trust 参数重试第二次连接**。复用既有错误前缀协议，避免在 russh 回调内阻塞等待前端确认所需的反向 IPC 接线。

```text
前端 session.ts                ipc.rs / manager          session.rs (russh 回调)           known_hosts.json
     │                              │                          │                               │
     │── invoke connect_ssh ───────▶│ lookup(host,port)        │                               │
     │   (trust=false)              │── HostKeyCheck{stored} ─▶│                               │
     │                              │                          │ check_server_key:             │
     │                              │                          │  stored=None,trust=false      │
     │                              │                          │  → verdict=Unknown, Ok(false) │
     │◀── Err HOSTKEY_UNKNOWN:ed25519|SHA256:xxx ──────────────│◀─ 连接中止                    │
     │                              │                          │                               │
     │ confirmHostKey(弹窗) → 信任  │                          │                               │
     │── invoke connect_ssh ───────▶│ lookup → HostKeyCheck    │                               │
     │   (trust=true, trustFp=xxx)  │   {stored:None,trust,trustFp}                            │
     │                              │                          │ check_server_key:             │
     │                              │                          │  trust && trustFp==当前指纹    │
     │                              │                          │  → Ok(true) + record() ──────▶│ 原子写
     │◀── Ok(sessionId) ────────────│                          │                               │
```

密钥变更路径：`stored=Some(旧)` 且当前指纹不等 → `Ok(false)` + verdict=Mismatch → `HOSTKEY_MISMATCH:` → 红色警告弹窗 → 「更新指纹并重连」时同样以 `trust=true, trustFp=新指纹` 重试，`record()` 按 host+port 覆盖旧条目（重试命中 §6.4 决策表第 1 行——该行对 `stored` 不作区分，故能覆盖落盘）。

## 6. 详细设计

### 6.1 持久化 schema（`known_hosts.json`）

```json
{
  "version": 1,
  "entries": [
    {
      "host": "127.0.0.1",
      "port": 2223,
      "algo": "ssh-ed25519",
      "fingerprint": "SHA256:AbCd…",
      "addedAt": 1760000000000
    }
  ]
}
```

- 存放路径：`app_data_dir/known_hosts.json`（macOS `~/Library/Application Support/com.rhost.app/`）；
- 键为 `host + port` 精确匹配（host 为用户配置原样字符串，IP/域名不解析等价）；一主机一指纹，重复 `record` 覆盖；
- 写入：临时文件 + rename 原子写，父目录自动创建（同 `store.rs::write_hosts` 手法）；
- 读取：文件不存在返回空；JSON 解析失败（含部分损坏）、**`version != 1`（未知未来版本）** 均记 warn 日志并返回空——不尝试猜测或迁移未知版本，下次 `record` 直接以当前 schema 全量重写。fail-closed：一律视为无记录走首连确认，绝不静默信任。

### 6.2 后端新模块 `src-tauri/src/known_hosts.rs`

纯 fs 模块（仅 `path()` 触及 `AppHandle`）：

```rust
pub struct KnownHostEntry { pub host: String, pub port: u16, pub algo: String,
                            pub fingerprint: String, pub added_at: i64 }
pub fn path(app: &AppHandle) -> Result<PathBuf, String>;
pub fn lookup(path: &Path, host: &str, port: u16) -> Option<KnownHostEntry>;
pub fn record(path: &Path, entry: KnownHostEntry) -> Result<(), String>; // upsert by host+port
```

- `record` 在 russh 回调线程做同步小文件 I/O（几 KB）：可接受——落盘必须与握手完成**同步收敛**，**禁止 `tokio::spawn` 异步写**（spawn 未完成时用户断开重连会读到旧文件，产生「连接成功但马上重弹确认」竞态）。放弃「阻塞写 + 超时保护」方案：`std::fs` 无超时原语，实现需额外线程 + join 超时，复杂度高于收益（写失败路径已由 §7 语义兜底）；
- 写失败不阻断会话，行为契约见 §7：指纹仅保留在本会话内存，本次连接继续可信，下次新建会话重新走首连确认，并输出 ERROR 日志提示用户。
- 单测：upsert 覆盖、多条目共存、损坏文件容错、原子写回读。

### 6.3 配置与错误协议

`ssh/mod.rs`：

```rust
pub struct HostKeyCheck {
    pub host: String,                     // 主机地址（用户配置原样字符串，落盘键）
    pub port: u16,                        // 端口（落盘键）
    pub stored: Option<(String, String)>, // 磁盘已存 (algo, fingerprint)
    pub trust: bool,                      // 用户已在 UI 确认
    pub trust_fp: Option<String>,         // 用户确认的指纹（防重试竞态，见 §6.4）
    pub persist: bool,                    // 「仅本次连接」= false：本次可信但不落盘
    pub path: PathBuf,                    // 落盘路径
}

/// 主机密钥校验策略：非 `Option` 字段（见下），两个显式变体强制作者做出选择，
/// 杜绝「忘填 → 静默跳过校验」的 Option 缺省语义
pub enum HostKeyPolicy {
    /// 校验恒开：ipc 两条命令（connect_ssh / test_ssh_connection）恒构造此变体
    Verify(HostKeyCheck),
    /// 仅 e2e 集成测试可达（doc(hidden) 防误用）：跳过校验
    #[doc(hidden)]
    SkipForTests,
}
```

`host` / `port` 是 `record()` 落盘的必需键：`known_hosts.json` 条目以「地址 + 端口 + 算法」唯一定位，`check_server_key` 回调内无法回查 payload，故由 ipc 层构造 `HostKeyCheck` 时一并传入。

- `SessionConfig` 增加 `pub host_key: HostKeyPolicy`（**非 `Option`**：`None` 的缺省语义即「跳过校验」，属危险默认；枚举强制显式选择、漏填即编译错误，`SkipForTests` 经 `doc(hidden)` 仅供 e2e，生产路径恒为 `Verify`）；
- `SshError` 新增（payload 三段 `{algo}|{fingerprint}|{pubkey}`，`pubkey` 为 OpenSSH 格式 `algorithm base64`，不含 `|`，前端按 `|` 切三段）：
  - `HostKeyUnknown(String)` → `HOSTKEY_UNKNOWN: ssh-ed25519|SHA256:xxx|ssh-ed25519 AAAA…`
  - `HostKeyMismatch(String)` → `HOSTKEY_MISMATCH: ssh-ed25519|SHA256:xxx|ssh-ed25519 AAAA…`

`ipc.rs` `ConnectPayload` 增加 `#[serde(default)] trust_host_key: bool`、`#[serde(default)] trust_fingerprint: Option<String>`、`#[serde(default = "default_true")] trust_host_key_persist: bool`（「仅本次连接」= false；缺省 true 保持既有「接受并保存」落盘行为）；**`connect_ssh` 与 `test_ssh_connection` 均增加 `app: tauri::AppHandle` 参数**——后者同样调用 `connect_and_auth` 命中 `check_server_key`（§3 现状），QuickConnectView 门禁的重试（§6.6）必须能构造 `HostKeyCheck`。两条命令统一：lookup → 构造 `HostKeyPolicy::Verify(HostKeyCheck)` → 置入 `SessionConfig`。无新增 invoke 命令。

**`trust_fingerprint` 是不可信 IPC 输入，后端必须校验**：非空、以 `SHA256:` 开头、主体仅含 Base64 字符集（`[A-Za-z0-9+/=]`，长度 ≥ 40；实现时须与 russh `fingerprint(HashAlg::Sha256)` 实际输出逐字核对——若含 padding 的 `=` 不得误拒，§8 单测以 russh 对测试容器密钥算出的指纹为合法样本）。非法值直接返回参数错误（`信任指纹格式非法`），**绝不进入 `HostKeyCheck`**。**`trust=true` 必须携带 `trust_fingerprint`，缺省时后端按 `trust=false` 处理（§6.4 第 3 行）**——该约束在后端决策表闭环，不依赖前端自觉。校验实现为纯函数并纳入单测。纵深防御动机：防止伪造 IPC 请求携带攻击者指纹完成落盘（虽然 §6.4 决策表中 `trust=true` 落盘的仍是「本次握手实际看到」的指纹而非传入值，但格式校验拦截脏输入应在最外层完成）。

### 6.4 校验决策表（`check_server_key` 重写）

`ClientHandler` 增加 `hk: Arc<StdMutex<Option<HostKeyVerdict>>>` 共享槽（复用 `algo` 字段手法）。判定结果用**四态枚举**而非 `matched: bool`——`connect_and_auth` 错误映射必须区分「密钥变更告警」（Mismatch，红色弹窗可更新）与「首连未确认」（Unknown，普通确认弹窗）两种拒绝语义，布尔值无法承载，故定义：

```rust
pub(crate) enum VerdictKind {
    Trusted,  // trust 重试命中（用户确认指纹 = 本次握手指纹）：通过并落盘
    Match,    // 磁盘记录比对一致：通过
    Mismatch, // 密钥变更（确认指纹或已存指纹 ≠ 本次指纹）：拒绝
    Unknown,  // 首连未知（无记录且未信任）：拒绝
}

pub(crate) struct HostKeyVerdict {
    kind: VerdictKind,
    algo: String,
    fingerprint: String,
    pubkey: String, // OpenSSH 格式 `algorithm base64`，前端「查看完整公钥」展开区用
}
```

评估自上而下、命中即止（trust 覆盖分支必须先于 stored 比对，理由见下）：

| # | 条件 | 判定 | 返回 |
|---|---|---|---|
| 1 | `trust=true` 且 `trust_fp` 等于当前指纹 | 通过并落盘 | `Ok(true)`；`persist=true` 时 `record()`（upsert 覆盖旧条目），「仅本次连接」跳过落盘；verdict=Trusted |
| 2 | `trust=true` 但 `trust_fp` 不等于当前指纹 | 告警 | `Ok(false)`，verdict=Mismatch |
| 3 | `trust=true` 但 `trust_fp=None` | 未确认 | 按 `trust=false` 走第 4–6 行，绝不落盘 |
| 4 | `trust=false` 且 `stored=Some` 且指纹相等 | 通过 | `Ok(true)`，verdict=Match |
| 5 | `trust=false` 且 `stored=Some` 且不等 | 告警 | `Ok(false)`，verdict=Mismatch |
| 6 | `trust=false` 且 `stored=None` | 首连确认 | `Ok(false)`，verdict=Unknown |

**为何 trust 覆盖分支（第 1–3 行）先于 stored 比对**：mismatch 的「更新指纹并重连」以 `trust=true, trust_fp=新指纹` 重试时 `stored=Some(旧)` 仍不等于当前指纹，若 stored 比对在前会再次命中第 5 行 Mismatch、`record()` 永不可达。第 1 行对 `stored` 不作区分——首连信任与变更更新统一由此行落盘覆盖。

**`trust_fp=None` 按 `trust=false` 处理（第 3 行）**：`trust=true` 必须携带用户确认过的指纹（§6.3 硬约束）；缺省即「未确认」，落回 stored 比对路径，杜绝伪造 IPC 携带 `trust=true` 直接把当前指纹落盘的旁路。

**TOFU 竞态防护（`trust_fp` 的意义）**：弹窗展示的是第一次握手看到的指纹 A；若第二次握手服务器密钥已变为 B（服务器轮换或注入），`trust_fp=A ≠ B` → 按 Mismatch 拒绝，保证「确认的 = 落盘的」。

`connect_and_auth` 连接失败错误映射：先读 verdict 槽——Mismatch → `SshError::HostKeyMismatch`、Unknown → `SshError::HostKeyUnknown`，否则维持现有 `SshError::Connect`。握手成功且 trust 时由 `record()` 落盘（info 日志含指纹）。

算法名 / 指纹 / 公钥统一由 `PublicKeyOrCertificate::public_key()` 取内层 `ssh_key::PublicKey` 计算（普通密钥与证书分支同一出口）：`algo = pk.algorithm()`、`fingerprint = pk.fingerprint(HashAlg::Sha256)`（输出 `SHA256:…`）、`pubkey = format!("{algo} {}", pk.public_key_base64())`（OpenSSH 格式）。

### 6.5 前端弹窗

- `composables/useHostKeyPrompt.ts`：仿 `usePasswordPrompt.ts` 模块级单例，`confirmHostKey(info): Promise<HostKeyAction>`，四态 `'reject' | 'once' | 'save' | 'update'`；info 含 `{ kind: 'unknown' | 'mismatch', host, port, algo, fingerprint, pubkey }`（`parseHostKeyError` 正则按三段 `algo|fp|pubkey` 解析）；
- `components/HostKeyModal.vue`：结构仿 `PasswordModal.vue`（mask + 卡片 + 键盘事件）。`unknown` 态正文以引导段开篇——「无法确认主机 `<host>` 的真实性。该主机尚未记录在 `~/.ssh/known_hosts` 中，请核对下方指纹是否与服务器管理员提供的一致。」（主机与路径用 `<code>` chip 突出）；下方为键值信息面板（主机、地址 `host:port`、密钥类型、等宽字体指纹行）——指纹行带「复制」按钮（复用 `stores/keys.ts` 的 `copyText`，成功后 1.6s「已复制」反馈；指纹块亦可选中逐字比对）；
  - **「查看完整公钥」展开区**：指纹面板下方设一低调展开开关（左置 ▶ 箭头，展开旋转 90°，文案「查看完整公钥 / 收起完整公钥」），默认收起；展开后以 `<pre>` 等宽块展示 `info.pubkey`（OpenSSH 格式 `algorithm base64`，`white-space: pre-wrap` + `word-break: break-all`，可选中复制），供用户与服务器侧 `ssh-keyscan` 输出逐字核对；每次打开弹窗重置为收起态；
  - 面板下方为 ⚠ 警示行（unknown：「指纹不一致时请勿继续，可能存在中间人攻击风险。」）；
  - `kind=unknown`：标题「无法验证主机真实性」→ 按钮「取消（ghost）/ 仅本次连接 / 接受并保存（主按钮）」——「仅本次连接」本次会话可信但不写 known_hosts（`persist=false`），下次新建会话重新确认；
  - `kind=mismatch`：红色警告「主机密钥已变更！可能是服务器重装，也可能存在中间人攻击」→ 按钮「断开（默认）/ 更新指纹并重连（危险样式）」；
- 键盘行为（硬性要求）：
  - **ESC 一律 = 取消**（拒绝信任、不落盘、连接保持失败态）；
  - **Enter 不得触发危险动作**：`kind=unknown` 时 Enter 等价「接受并保存」（主按钮）；`kind=mismatch` 时默认焦点固定在「断开」，Enter 仅触发「断开」，「更新指纹并重连」**只能鼠标点击**——高危操作禁止成为回车默认动作。
- `App.vue` 在 PasswordModal 旁常驻挂载。

### 6.6 连接流程接入（`stores/session.ts`）

`connectBackend(s, cols, rows, trustHostKey = false, trustFp?, trustPersist = true)`：payload 增加三个字段（含 `trustHostKeyPersist`）。catch 中解析前缀（`algo|fp|pubkey` 按 `|` 切三段，第三段为完整公钥）：

- **自动重连中**（`reconnectAttemptMap.has(s.id)`）：删除重连记录、状态置 offline、`disconnectReason` 写明「主机密钥校验失败：{algo}|{fp}」，**不弹窗**——后台循环重试禁止打断用户；**该分流须先于既有 `scheduleAutoReconnect` 续试判断直接返回，不得落入续试**；但**不得只把信息藏在 `disconnectReason`**（否则服务器密钥变更后表现为「反复掉线且无解释」），必须显性告知：
  - 一次性 toast：「主机密钥校验失败，请手动重新连接完成指纹确认」（复用 useToast，仅此场景发一次）；
  - 会话状态点置错误红（复用现有错误态样式），悬停/详情可见 `disconnectReason` 全文；
- 手动路径：置 offline 后弹窗；`save`/`update` → `connectBackend(s, cols, rows, true, fp)`（落盘）、`once` → `connectBackend(s, cols, rows, true, fp, false)`（仅本次连接不落盘）重试一次；`reject` → 维持失败态。

`views/home/QuickConnectView.vue` 的 `test_ssh_connection` 门禁：catch 中仿既有 `KEY_ENCRYPTED` 分支解析两个前缀，弹窗确认后带 trust + persist 参数重试一次。

## 7. 安全与降级

- 指纹/算法属公开信息，可入日志；**密码、私钥口令、PTY 字节流永不因本设计入日志或文件**；
- fail-closed 原则：known_hosts 读失败 → 视为无记录走首连确认；
- **`record` 落盘失败的行为契约（必须精确实现）**：
  1. **绝不断开已建立的 SSH 会话**——本次连接继续可信，指纹保留在本会话内存中；
  2. 该内存信任**不跨会话**：下次新建会话读到文件无记录，重新走首连确认弹窗；
  3. ERROR 级日志明示用户后果：「无法保存主机密钥，下次连接将重新确认指纹」。
  禁止的两种错误倾向：① 写失败即断开已成功的会话（过度反应）；② 写失败后把信任状态挂到任何跨会话位置（模块级单例/全局缓存），导致下次连接跳过确认；
- 校验恒开，无旁路开关；e2e 的 `HostKeyPolicy::SkipForTests` 经 `doc(hidden)` 标记仅测试可达，生产路径恒为 `Verify`；
- mismatch 的「更新指纹并重连」必须经过显式危险样式按钮二次确认，不做自动覆盖。

## 8. 测试与验证

- 单测：`known_hosts.rs`（upsert/共存/原子写回读）；**损坏与版本容错**（部分损坏 JSON → 空、`version=2` 未知版本 → 空）；`trust_fingerprint` 格式校验纯函数（合法 / 空串 / 缺 `SHA256:` 前缀 / 非法字符 / 与 russh 真实指纹输出逐字一致的样本）；决策表全分支（抽象为纯函数后测）：trust 覆盖通过（`stored=None` 与 `stored=Some(旧)` 两态均须落盘）、`trust_fp`≠当前 → Mismatch、`trust=true` 缺 `trust_fp` → 按 false 走比对、stored 相等/不等、None+false → Unknown；
- e2e：既有 `ssh_e2e.rs` 全部用例传 `host_key: HostKeyPolicy::SkipForTests` 保持全绿；新增一条 trust 路径用例：先以 `Verify` + trust=false 连接 → 失败，从错误串 `HOSTKEY_UNKNOWN: {algo}|{fp}|{pubkey}` 按 `|` 切三段解析指纹（顺带端到端验证错误协议与 pubkey 格式），再以 trust=true + 该指纹重试 → 成功，断言临时文件路径落盘；
- 手工验证矩阵（docker/debian-sshd，端口 2223）：
  1. 首连 → 弹窗 → 信任 → 连接成功 → 重连不弹；
  2. 容器内 `rm /etc/ssh/ssh_host_* && ssh-keygen -A && 重启 sshd` → 重连 → 红色警告 → 分别验证「断开」与「更新指纹并重连」；
  3. 弹窗取消 → 连接失败且 `known_hosts.json` 无新条目；mismatch 弹窗中 Enter 触发「断开」、Tab/点击才可达「更新指纹并重连」；
  4. 检查 `~/Library/Application Support/com.rhost.app/known_hosts.json` 内容；
  5. 快速连视图门禁路径同样弹窗；
  6. **落盘失败路径**：将 `known_hosts.json` 所在目录临时置为只读（`chmod 555`）后连接 → 会话建立成功 + ERROR 日志出现「下次连接将重新确认」→ 恢复权限后重连 → 重新弹首连确认（验证内存信任不跨会话）；
  7. **自动重连告知**：信任主机后制造密钥变更 → 断网触发自动重连 → 不弹窗，但出现一次性 toast + 状态点红色 + `disconnectReason` 详细文案。

## 9. 未决问题

- ~~known_hosts 是否纳入配置导入导出~~ **已决策（2026-10-08）：不纳入**——见 §4 非目标与 `config-import-export.md` §5；
- 未来若支持 SSH CA，`check_server_key` 的 Certificate 分支需要独立的信任策略——本设计按普通指纹处理，不阻塞。
