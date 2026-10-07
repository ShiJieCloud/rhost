//! SSH 端口转发引擎：本地转发（-L）、远程转发（-R）、动态转发 SOCKS5（-D）。
//!
//! 纯 tokio + russh 实现，不依赖 tauri（见 mod.rs 头注释），可脱离 Tauri 单测/e2e。
//!
//! 架构（对应 docs/explanation/design/tunnel-design.md §6.2）：
//! - `TunnelManager`：每会话一个，持有 russh handle 的 Arc 与会话级取消令牌，
//!   管理全部规则的启动/停止/快照；
//! - `TunnelEntry`：单条规则的运行时状态（状态机、计数器、脏标记、单规则并发闸门、令牌桶）；
//! - `RemoteRegistry`：-R 回调查表（ClientHandler 与 TunnelManager 共享同一实例），
//!   远端来连时按 (bind_host, bound_port) 查出目标与闸门；
//! - `RateLimiter`：手写令牌桶（惰性回灌，无第三方 crate），限制单规则接入速率。
//!
//! 硬约束：
//! - 所有后台任务挂 `CancellationToken`；共享数据 clone `Arc` 后释放锁，锁内不 await；
//! - 限流/闸门全部 `try_acquire`/`allow` 语义，失败立即拒绝，绝不排队；
//! - russh 错误按枚举变体匹配，禁止字符串解析；
//! - 转发内容零记录：日志仅允许规则 id、目标 host:port、状态与字节计数。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use crate::applog::{emit, events};
use log::debug;
use russh::client;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

use super::frame::{FrameType, encode_frame};
use super::SshError;

/* =========================================================
 * 常量（魔数集中定义，注释写理由；不开放配置）
 * ========================================================= */

/// 单会话规则上限：防止规则爆炸耗尽本地端口与内核资源；超限返回 `TUNNEL_LIMIT:`
pub(crate) const MAX_RULES_PER_SESSION: usize = 32;
/// 单规则并发上限：每连接吃 1 个本地 fd + 1 个 SSH channel + 2 个 copy 任务，
/// 64 在常规 `ulimit -n` 下留足余量
pub(crate) const MAX_CONN_PER_RULE: usize = 64;
/// 单会话转发全局上限：第二条防线，防止 -R 洪峰灌穿所有规则配额之和
pub(crate) const MAX_CONN_GLOBAL: usize = 512;
/// 转发为交互式流量，16KB 在延迟与吞吐间平衡
pub(crate) const COPY_BUF: usize = 16384;
/// SOCKS5 方法协商 + CONNECT 请求最大长度远小于此，防恶意客户端灌包
pub(crate) const SOCKS_HANDSHAKE_MAX: usize = 512;
/// 本地 accept 后到 channel 打开成功的总窗口（含 SOCKS 握手 + SSH channel open），
/// 逾期强制关闭 TCP，杜绝 slowloris 式慢速握手
pub(crate) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// 监听器监督器退避：1s 起翻倍
pub(crate) const SUPERVISOR_RETRY_BASE_MS: u64 = 1000;
/// 监督器退避封顶
pub(crate) const SUPERVISOR_RETRY_MAX_MS: u64 = 30000;
/// 连续 accept 失败后规则置 Error，不再无限重启
pub(crate) const SUPERVISOR_MAX_RETRIES: u32 = 5;
/// 单规则接入速率令牌桶突发上限；限流在 burst 前对突发友好，达限后平滑拒绝
pub(crate) const ACCEPT_RATE_BURST: u32 = 200;
/// 令牌桶每秒回灌量
pub(crate) const ACCEPT_RATE_REFILL: u32 = 100;
/// -R 回调中连接本地目标的超时，超时则 reject channel
pub(crate) const REMOTE_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// 端口冲突 `next` 策略最多顺延尝试的端口数
pub(crate) const NEXT_PORT_TRIES: u32 = 10;
/// 建立期瞬态错误退避：1s 起翻倍
pub(crate) const RETRY_BASE_MS: u64 = 1000;
/// 建立期退避封顶
pub(crate) const RETRY_MAX_MS: u64 = 10000;
/// 连接数/流量快照推送节流窗口；错误帧不节流，即时推送
pub(crate) const STATUS_THROTTLE: Duration = Duration::from_secs(1);
/// 规则停止时活动连接最大排空时间（秒），逾期强制 close
pub(crate) const SHUTDOWN_DRAIN_SEC: u64 = 3;

/* =========================================================
 * 核心类型（serde camelCase，与前端 types.ts TunnelRule 对齐）
 * ========================================================= */

/// 转发类型；序列化为 `local` / `remote` / `dynamic`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TunnelType {
    /// 本地转发（`ssh -L`）：本地监听，经 direct-tcpip 连远端目标
    Local,
    /// 远程转发（`ssh -R`）：远端监听，回调接入本地目标
    Remote,
    /// 动态转发（`ssh -D`）：本地 SOCKS5 代理，按请求动态决定目标
    Dynamic,
}

/// 与前端 `TunnelRule` 对齐的引擎入参（name/enabled 为配置态字段，引擎不消费，
/// 由前端在 invoke 前剥离）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelRule {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: TunnelType,
    /// 监听地址；空串归一化为 `127.0.0.1`
    pub bind_host: String,
    pub bind_port: u16,
    /// 目标地址；dynamic 为 None
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_port: Option<u16>,
}

/// 规则运行状态；序列化为 `starting` / `active` / `error` / `stopped`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TunnelState {
    Starting,
    Active,
    Error,
    Stopped,
}

/// 单条规则的运行时快照（0x0A 帧元素）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelStatus {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: TunnelType,
    pub state: TunnelState,
    pub bind_host: String,
    pub bind_port: u16,
    /// 实际监听端口（next 顺延 / 远端分配时不同于 bind_port）
    pub bound_port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_port: Option<u16>,
    pub active_connections: u32,
    /// 本地→远端方向累计字节
    pub bytes_up: u64,
    /// 远端→本地方向累计字节
    pub bytes_down: u64,
    pub error: Option<String>,
}

/* =========================================================
 * 令牌桶（手写，惰性回灌，无第三方 crate）
 * ========================================================= */

/// 令牌桶限流器：容量 `burst`，每秒回灌 `refill`。锁内只做浮点运算，不 await。
///
/// 允许 `burst` 规模的瞬时突发（对合法突发友好），持续速率超过 `refill/s` 后
/// 平滑拒绝，抵御本地端口扫描式洪峰打爆 fd。
pub(crate) struct RateLimiter {
    state: StdMutex<RateState>,
    burst: f64,
    refill_per_sec: f64,
}

struct RateState {
    tokens: f64,
    last: std::time::Instant,
}

impl RateLimiter {
    pub(crate) fn new(burst: u32, refill: u32) -> Self {
        Self {
            state: StdMutex::new(RateState {
                tokens: burst as f64,
                last: std::time::Instant::now(),
            }),
            burst: burst as f64,
            refill_per_sec: refill as f64,
        }
    }

    /// 取一枚令牌：桶空时返回 false（调用方立即关闭连接）。
    /// 惰性回灌：本次调用时按距上次的流逝时间一次性补足，无需后台定时器。
    pub(crate) fn allow(&self) -> bool {
        let mut st = self.state.lock().unwrap();
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(st.last).as_secs_f64();
        st.last = now;
        st.tokens = (st.tokens + elapsed * self.refill_per_sec).min(self.burst);
        if st.tokens >= 1.0 {
            st.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/* =========================================================
 * 远程转发回调查表
 * ========================================================= */

/// -R 回调命中后的本地转发目标
#[derive(Debug, Clone)]
pub struct RemoteTarget {
    pub host: String,
    pub port: u16,
}

/// 回调命中后拿到的完整上下文：本地目标 + 所属规则的并发闸门与计数器。
/// 远端来连与本地 accept 走同一套准入/计量，-R 不得绕过 MAX_CONN 防线。
#[derive(Clone)]
pub struct RemoteBinding {
    pub target: RemoteTarget,
    pub(crate) entry: Arc<TunnelEntry>,
}

/// -R 回调查表：key = (远端绑定地址, 远端实际绑定端口)。
/// `ClientHandler` 回调与 `TunnelManager` 共享同一实例（Arc clone）。
/// 锁内只做 HashMap 插入/删除/clone `RemoteBinding`（`Arc` clone），锁内不 await。
#[derive(Clone, Default)]
pub struct RemoteRegistry {
    inner: Arc<StdMutex<HashMap<(String, u16), RemoteBinding>>>,
}

impl RemoteRegistry {
    pub(crate) fn insert(&self, host: &str, port: u16, binding: RemoteBinding) {
        self.inner
            .lock()
            .unwrap()
            .insert((host.to_string(), port), binding);
    }

    pub(crate) fn remove(&self, host: &str, port: u16) -> Option<RemoteBinding> {
        self.inner.lock().unwrap().remove(&(host.to_string(), port))
    }

    /// 锁内 clone 出 `RemoteBinding`（内含 `Arc<TunnelEntry>`，clone 廉价）后立即放锁
    pub(crate) fn lookup(&self, host: &str, port: u16) -> Option<RemoteBinding> {
        self.inner
            .lock()
            .unwrap()
            .get(&(host.to_string(), port))
            .cloned()
    }
}

/* =========================================================
 * 规则运行时状态
 * ========================================================= */

/// 规则级计数器（活动连接数 + 双向流量）；原子操作，无锁
#[derive(Debug, Default)]
pub(crate) struct Counters {
    pub active: AtomicU32,
    pub bytes_up: AtomicU64,
    pub bytes_down: AtomicU64,
}

/// 单条规则的运行时状态：跨任务共享（accept 循环 / copy 任务 / 监督器 / 快照）。
/// 共享指针一律 `Arc<TunnelEntry>`；内部可变字段用原子量或 std 锁（锁内不 await）。
pub(crate) struct TunnelEntry {
    pub rule: TunnelRule,
    /// 单规则停止令牌（会话 cancel 是其父集）
    pub rule_cancel: CancellationToken,
    /// 当前状态（操作点直写；快照读取）
    pub state: StdMutex<TunnelState>,
    /// 最近一次失败的展示原因（Error 态携带，其余为 None）
    pub error: StdMutex<Option<String>>,
    /// 实际监听端口（next 顺延 / 远端分配时不同于 bind_port）
    pub bound_port: AtomicU16,
    pub counters: Arc<Counters>,
    /// 连接数/流量变化脏标记：由 copy 收尾/接入处置位，状态任务醒来检查并清零
    pub dirty: Arc<AtomicBool>,
    /// 单规则连接闸门（MAX_CONN_PER_RULE 许可制）
    pub rule_conns: Arc<Semaphore>,
    /// 接入速率令牌桶
    pub accept_limiter: RateLimiter,
}

impl TunnelEntry {
    fn new(rule: TunnelRule, session_cancel: &CancellationToken) -> Self {
        Self {
            rule,
            rule_cancel: session_cancel.child_token(),
            state: StdMutex::new(TunnelState::Stopped),
            error: StdMutex::new(None),
            bound_port: AtomicU16::new(0),
            counters: Arc::default(),
            dirty: Arc::new(AtomicBool::new(false)),
            rule_conns: Arc::new(Semaphore::new(MAX_CONN_PER_RULE)),
            accept_limiter: RateLimiter::new(ACCEPT_RATE_BURST, ACCEPT_RATE_REFILL),
        }
    }

    fn set_state(&self, state: TunnelState, error: Option<String>) {
        *self.state.lock().unwrap() = state;
        *self.error.lock().unwrap() = error;
    }

    fn current_state(&self) -> TunnelState {
        *self.state.lock().unwrap()
    }

    /// 快照组装：只读原子量与瞬时 std 锁，可在任意上下文调用
    fn status(&self) -> TunnelStatus {
        TunnelStatus {
            id: self.rule.id.clone(),
            kind: self.rule.kind,
            state: self.current_state(),
            bind_host: self.rule.bind_host.clone(),
            bind_port: self.rule.bind_port,
            bound_port: self.bound_port.load(Ordering::Relaxed),
            target_host: self.rule.target_host.clone(),
            target_port: self.rule.target_port,
            active_connections: self.counters.active.load(Ordering::Relaxed),
            bytes_up: self.counters.bytes_up.load(Ordering::Relaxed),
            bytes_down: self.counters.bytes_down.load(Ordering::Relaxed),
            error: self.error.lock().unwrap().clone(),
        }
    }
}

/* =========================================================
 * 启动参数
 * ========================================================= */

/// 端口冲突策略（设置项 `tunnelPortConflict`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortConflict {
    /// bind 失败立即报错
    Stop,
    /// bind 失败同报错，但批量自动启动场景前端不弹错误、继续后续规则
    Skip,
    /// 从 bind_port 起逐个尝试，最多 [`NEXT_PORT_TRIES`] 个
    Next,
}

impl PortConflict {
    /// 解析设置项字符串（前端直传原值）；未知值回退 Stop
    pub fn parse(s: &str) -> Self {
        match s {
            "skip" => Self::Skip,
            "next" => Self::Next,
            _ => Self::Stop,
        }
    }
}

/// SOCKS5 域名解析策略（设置项 `tunnelDnsResolve`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsResolve {
    /// 域名原样传给 sshd，由远端解析（默认）
    Remote,
    /// 本机 `lookup_host` 解析，取首个地址，失败回 SOCKS 错误码 0x04
    Local,
}

impl DnsResolve {
    /// 解析设置项字符串（前端直传原值）；未知值回退 Remote
    pub fn parse(s: &str) -> Self {
        match s {
            "local" => Self::Local,
            _ => Self::Remote,
        }
    }
}

/// `tunnel_start` 的引擎入参（前端设置项直传）
#[derive(Debug, Clone, Copy)]
pub struct StartOptions {
    pub port_conflict: PortConflict,
    pub dns_resolve: DnsResolve,
    /// 建立期瞬态错误重试次数（0–20）
    pub retry_count: u32,
}

/* =========================================================
 * 规则校验（后端独立再校验，不信任前端）
 * ========================================================= */

/// 校验 host：空串归一化 `127.0.0.1`；长度 ≤ 255；仅允许域名/IPv4/IPv6 字符。
/// 非法返回 Err（错误文本由调用方拼进 `TUNNEL_BAD_RULE:` 前缀）。
pub(crate) fn validate_host(field: &str, raw: &str) -> Result<String, String> {
    let host = raw.trim();
    if host.is_empty() {
        return Ok("127.0.0.1".to_string());
    }
    if host.len() > 255 {
        return Err(format!("{field} 长度超限（>255 字节）"));
    }
    // 域名/IPv4/IPv6 白名单：字母数字 + . - _ : [] %（zone id）；
    // 空白、斜杠、@、? 等注入面字符一律拒绝
    if !host
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '[' | ']' | '%'))
    {
        return Err(format!("{field} 含非法字符"));
    }
    Ok(host.to_string())
}

/// 校验单条规则：端口范围、host 合法性、类型与 target 字段匹配。
/// 通过后返回归一化（bind_host 空串 → 127.0.0.1）的规则副本。
pub(crate) fn validate_rule(rule: &TunnelRule) -> Result<TunnelRule, String> {
    if rule.id.is_empty() {
        return Err("规则 id 不能为空".into());
    }
    // 端口：1–65535；bind_port=0 仅对 remote 合法（由远端 sshd 分配，引擎保留支持）
    let bind_ok = (1..=u16::MAX as u32).contains(&(rule.bind_port as u32))
        || (rule.bind_port == 0 && rule.kind == TunnelType::Remote);
    if !bind_ok {
        return Err(format!("监听端口 {} 越界（1–65535）", rule.bind_port));
    }
    let bind_host = validate_host("监听地址", &rule.bind_host)?;

    match rule.kind {
        TunnelType::Dynamic => {
            // dynamic 无目标字段；携带即视为表单数据错乱
            if rule.target_host.is_some() || rule.target_port.is_some() {
                return Err("动态转发（SOCKS5）不包含目标地址字段".into());
            }
        }
        TunnelType::Local | TunnelType::Remote => {
            let target_host = rule.target_host.as_deref().unwrap_or("");
            let target_host = validate_host("目标地址", target_host)?;
            if target_host == "127.0.0.1" && rule.target_host.as_deref().unwrap_or("").is_empty() {
                // target_host 留空是表单错误（-L/-R 必须显式指定目标）
                return Err("目标地址不能为空".into());
            }
            let tp = rule.target_port.unwrap_or(0);
            if !(1..=u16::MAX as u32).contains(&(tp as u32)) {
                return Err(format!("目标端口 {tp} 越界（1–65535）"));
            }
            let _ = target_host;
        }
    }
    Ok(TunnelRule {
        id: rule.id.clone(),
        kind: rule.kind,
        bind_host,
        bind_port: rule.bind_port,
        // 归一化：local/remote 的 target_host 也做空值归一（上面已拒绝空串）
        target_host: rule.target_host.clone(),
        target_port: rule.target_port,
    })
}

/* =========================================================
 * TunnelManager
 * ========================================================= */

/// 每会话一个的端口转发管理器：与 SFTP 子系统、metrics 采集器平级，
/// 共用同一条已认证主连接与同一个会话 `cancel` 令牌、同一帧队列。
pub struct TunnelManager {
    /// 已认证 SSH handle（russh Handle 内部为 Arc，外层 Mutex 串行化通道操作）
    handle: Arc<Mutex<client::Handle<super::session::ClientHandler>>>,
    /// 帧发送口 clone：状态帧（0x0A）与 PTY 帧共用 FIFO 队列
    frame_tx: mpsc::Sender<Vec<u8>>,
    /// 会话级取消令牌 clone：会话断开时连带取消全部规则任务
    cancel: CancellationToken,
    /// -R 回调查表（与 ClientHandler 共享同一实例）
    remote: RemoteRegistry,
    /// 规则表：id → 运行时状态。std 锁只做瞬时取放（clone Arc），锁内不 await
    tunnels: Arc<StdMutex<HashMap<String, Arc<TunnelEntry>>>>,
    /// 会话级转发连接总闸（MAX_CONN_GLOBAL）
    global_conns: Arc<Semaphore>,
    /// 1s 节流状态任务已启动标记（整个 manager 生命周期仅 spawn 一次）
    status_task_started: AtomicBool,
}

impl TunnelManager {
    pub(crate) fn new(
        handle: Arc<Mutex<client::Handle<super::session::ClientHandler>>>,
        frame_tx: mpsc::Sender<Vec<u8>>,
        cancel: CancellationToken,
        remote: RemoteRegistry,
        // 会话级转发连接总闸：与 `ClientHandler` 共享同一实例（-R 回调同一套闸门）
        global_conns: Arc<Semaphore>,
    ) -> Self {
        Self {
            handle,
            frame_tx,
            cancel,
            remote,
            tunnels: Arc::default(),
            global_conns,
            status_task_started: AtomicBool::new(false),
        }
    }

    /// 推送全量状态快照帧（0x0A）。帧队列关闭（会话已收尾）时静默忽略。
    pub(crate) async fn push_status(&self) {
        push_tunnels_frame(&self.frame_tx, &self.tunnels).await;
    }

    /// 当前全部规则状态快照（e2e/调试用；与 0x0A 帧同一数据源）
    pub async fn statuses(&self) -> Vec<TunnelStatus> {
        let map = self.tunnels.lock().unwrap().clone();
        snapshot_of(&map)
    }

    /// 启动一条规则（幂等：已在 Starting/Active 返回 `TUNNEL_RUNNING:`）。
    ///
    /// 流程：后端独立校验（不信任前端）→ 幂等检查 → 规则数上限 →
    /// 建立条目（Starting）入表并推快照 → 按类型启动 → 失败置 Error 并推帧。
    pub async fn start(&self, rule: TunnelRule, opts: StartOptions) -> Result<(), SshError> {
        // 1. 校验：非法返回 TUNNEL_BAD_RULE；bind_host 空串归一化 127.0.0.1
        let rule = validate_rule(&rule)
            .map_err(|e| SshError::Tunnel(format!("TUNNEL_BAD_RULE: {e}")))?;

        // 2. 幂等：相同 id 已在 Active/Starting 返回 TUNNEL_RUNNING（前端按成功处理，
        //    不改变现有后台重试任务）；仅 Stopped/Error 会真正启动新任务
        {
            let map = self.tunnels.lock().unwrap();
            if let Some(existing) = map.get(&rule.id) {
                if matches!(
                    existing.current_state(),
                    TunnelState::Starting | TunnelState::Active
                ) {
                    return Err(SshError::Tunnel("TUNNEL_RUNNING: 规则已在运行".into()));
                }
            } else if map.len() >= MAX_RULES_PER_SESSION {
                return Err(SshError::Tunnel(format!(
                    "TUNNEL_LIMIT: 单会话规则数已达上限（{MAX_RULES_PER_SESSION}）"
                )));
            }
        }

        // 3. 建立运行时条目（Starting），入表并立即推快照（前端立刻可见）
        let entry = Arc::new(TunnelEntry::new(rule.clone(), &self.cancel));
        entry.set_state(TunnelState::Starting, None);
        self.tunnels
            .lock()
            .unwrap()
            .insert(rule.id.clone(), entry.clone());
        self.ensure_status_task();
        self.push_status().await;

        // 4. 按类型启动
        let result = match rule.kind {
            TunnelType::Local | TunnelType::Dynamic => self.start_local(entry.clone(), opts).await,
            TunnelType::Remote => self.start_remote(entry.clone(), opts).await,
        };
        if let Err(err) = &result {
            // 失败置 Error 并即时推帧；条目保留在表中供用户查看原因，可再次 start 覆盖。
            // 仅当仍处 Starting 才置 Error——若用户在建立期 stop（teardown 已置 Stopped）
            // 或会话已收尾，不得覆盖其状态
            if entry.current_state() == TunnelState::Starting {
                entry.set_state(TunnelState::Error, Some(err.to_string()));
                self.push_status().await;
            }
            emit(
                log::Level::Error,
                "tunnel",
                events::TUNNEL_START_FAILED,
                None,
                format!("隧道 {} 启动失败: {err}", rule.id),
                Some(serde_json::json!({
                    "rule_id": rule.id,
                    "kind": format!("{:?}", rule.kind),
                    "bind_host": rule.bind_host,
                    "bind_port": rule.bind_port,
                    "error": err.to_string(),
                })),
            );
        } else {
            emit(
                log::Level::Info,
                "tunnel",
                events::TUNNEL_START,
                None,
                format!("隧道 {} 启动成功（{}:{}）", rule.id, rule.bind_host, rule.bind_port),
                Some(serde_json::json!({
                    "rule_id": rule.id,
                    "kind": format!("{:?}", rule.kind),
                    "bind_host": rule.bind_host,
                    "bind_port": rule.bind_port,
                })),
            );
        }
        result
    }

    /// -L/-D：bind 策略 → Active 快照帧 → spawn 监督器（内含 accept 循环）
    async fn start_local(
        &self,
        entry: Arc<TunnelEntry>,
        opts: StartOptions,
    ) -> Result<(), SshError> {
        let listener = self.bind_with_policy(&entry, opts.port_conflict).await?;
        // bind 成功即推 Active 快照，随后 spawn 监听任务
        entry.set_state(TunnelState::Active, None);
        self.push_status().await;

        let ctx = Arc::new(ListenCtx {
            handle: self.handle.clone(),
            session_cancel: self.cancel.clone(),
            global_conns: self.global_conns.clone(),
            frame_tx: self.frame_tx.clone(),
            tunnels: self.tunnels.clone(),
            dns_resolve: opts.dns_resolve,
        });
        tokio::spawn(supervise_listen(entry, ctx, listener));
        Ok(())
    }

    /// -R：`tcpip_forward` 远端注册 → 写 `RemoteRegistry` → Active。
    /// 不 spawn 监听任务——远端来连走 `ClientHandler::server_channel_open_forwarded_tcpip` 回调。
    ///
    /// 建立期错误分类（§6.10，枚举变体匹配，禁止字符串解析）：
    /// - `RequestDenied`（sshd `AllowTcpForwarding no` / `GatewayPorts` 拒绝）：确定性失败，
    ///   不重试，返回 `TUNNEL_REMOTE_DENIED:`；
    /// - `Disconnect`/`SendError`：瞬态，按 `opts.retry_count` 指数退避重试
    ///   （1s 翻倍封顶 10s），重试期间保持 Starting；规则取消则放弃；
    /// - 其余变体：直接失败透传（连接不可恢复类错误重试无意义）。
    async fn start_remote(
        &self,
        entry: Arc<TunnelEntry>,
        opts: StartOptions,
    ) -> Result<(), SshError> {
        let rule = &entry.rule;
        let mut attempt: u32 = 0;
        loop {
            let res = {
                let h = self.handle.lock().await;
                h.tcpip_forward(&rule.bind_host, rule.bind_port as u32).await
            };
            match res {
                Ok(bound) => {
                    // 远端实际绑定端口：port=0 时由 sshd 分配并回报；
                    // 非 0 时 OpenSSH 回报请求端口；回报 0 则回退请求端口
                    let bound = u16::try_from(bound).unwrap_or(0);
                    let bound = if bound == 0 { rule.bind_port } else { bound };
                    if bound == 0 {
                        // bind_port=0 且远端未回报实际端口：无法定位回调 key
                        return Err(SshError::Tunnel(
                            "TUNNEL_REMOTE_DENIED: 远端未回报实际绑定端口，无法注册回调".into(),
                        ));
                    }
                    entry.bound_port.store(bound, Ordering::Relaxed);
                    self.remote.insert(
                        &rule.bind_host,
                        bound,
                        RemoteBinding {
                            target: RemoteTarget {
                                host: rule.target_host.clone().unwrap_or_default(),
                                port: rule.target_port.unwrap_or(0),
                            },
                            entry: entry.clone(),
                        },
                    );
                    entry.set_state(TunnelState::Active, None);
                    self.push_status().await;
                    emit(
                        log::Level::Info,
                        "tunnel",
                        events::TUNNEL_REMOTE_READY,
                        None,
                        format!("远程转发 {} 已就绪，监听 {}:{}", rule.id, rule.bind_host, bound),
                        Some(serde_json::json!({
                            "rule_id": rule.id,
                            "bind_host": rule.bind_host,
                            "bind_port": bound,
                        })),
                    );
                    return Ok(());
                }
                Err(russh::Error::RequestDenied) => {
                    emit(
                        log::Level::Warn,
                        "tunnel",
                        events::TUNNEL_REMOTE_DENIED,
                        None,
                        format!("远端拒绝远程转发: {}", rule.id),
                        Some(serde_json::json!({
                            "rule_id": rule.id,
                            "bind_host": rule.bind_host,
                            "bind_port": rule.bind_port,
                        })),
                    );
                    return Err(SshError::Tunnel(
                        "TUNNEL_REMOTE_DENIED: 远端服务器拒绝远程转发".into(),
                    ));
                }
                Err(e @ (russh::Error::Disconnect | russh::Error::SendError)) => {
                    if attempt >= opts.retry_count {
                        return Err(SshError::Tunnel(format!(
                            "远端端口绑定失败（重试 {attempt} 次后放弃）: {e}"
                        )));
                    }
                    attempt += 1;
                    let delay = retry_backoff(attempt);
                    debug!(
                        "隧道 {} tcpip_forward 瞬态失败（第 {attempt} 次，{}ms 后重试）: {e}",
                        rule.id,
                        delay.as_millis()
                    );
                    // rule_cancel 是会话 cancel 的子令牌，单一 select 覆盖两级取消
                    tokio::select! {
                        _ = entry.rule_cancel.cancelled() => {
                            return Err(SshError::Tunnel("规则已停止，取消建立重试".into()));
                        }
                        _ = tokio::time::sleep(delay) => {}
                    }
                }
                Err(e) => {
                    return Err(SshError::Tunnel(format!("远端端口绑定失败: {e}")));
                }
            }
        }
    }

    /// 按 `portConflict` 策略绑定 `<bind_host>:<bind_port>`：
    /// - `stop`/`skip`：bind 失败返回 `TUNNEL_PORT_IN_USE:`（skip 的「批量继续」
    ///   由前端按错误前缀分流，引擎语义与 stop 相同）；
    /// - `next`：从 bind_port 起逐个尝试，最多 [`NEXT_PORT_TRIES`] 个；
    ///   尽力策略：并发批量启动时各规则端口扫描间无全局锁，存在竞态属可接受边界。
    async fn bind_with_policy(
        &self,
        entry: &TunnelEntry,
        policy: PortConflict,
    ) -> Result<TcpListener, SshError> {
        let tries = match policy {
            PortConflict::Next => NEXT_PORT_TRIES,
            _ => 1,
        };
        let mut port = entry.rule.bind_port;
        let mut last_err: Option<std::io::Error> = None;
        for _ in 0..tries {
            match TcpListener::bind((entry.rule.bind_host.as_str(), port)).await {
                Ok(l) => {
                    entry.bound_port.store(port, Ordering::Relaxed);
                    return Ok(l);
                }
                Err(e) => {
                    last_err = Some(e);
                    if port == u16::MAX {
                        break;
                    }
                    port += 1;
                }
            }
        }
        if let Some(e) = last_err {
            emit(
                log::Level::Warn,
                "tunnel",
                events::TUNNEL_PORT_CONFLICT,
                None,
                format!("隧道 {} bind 失败（策略 {policy:?}）: {e}", entry.rule.id),
                Some(serde_json::json!({
                    "rule_id": entry.rule.id,
                    "bind_host": entry.rule.bind_host,
                    "bind_port": entry.rule.bind_port,
                    "policy": format!("{:?}", policy),
                    "error": e.to_string(),
                })),
            );
        }
        Err(SshError::Tunnel(format!(
            "TUNNEL_PORT_IN_USE: 本地端口 {}:{} 已被占用",
            entry.rule.bind_host, entry.rule.bind_port
        )))
    }

    /// 停止一条规则（幂等：不存在视为成功）。
    pub async fn stop(&self, rule_id: &str) -> Result<(), SshError> {
        let entry = { self.tunnels.lock().unwrap().get(rule_id).cloned() };
        let Some(entry) = entry else {
            return Ok(());
        };
        self.teardown(&entry).await;
        Ok(())
    }

    /// 停止单条规则：-R 先撤远端监听 → 停 accept/拒新接入 → 排空 ≤3s → Stopped。
    /// -L/-D 的 stop 语义相同（无远端撤销步骤）。
    async fn teardown(&self, entry: &Arc<TunnelEntry>) {
        // 1. -R：尽力撤远端转发（错误忽略，主连接可能已断）；随后删注册表——
        //    此后新回调一律 reject。cancel-tcpip-forward 仅拒绝新接入，
        //    对已建立的 forwarded-tcpip channel 无任何影响。
        if entry.rule.kind == TunnelType::Remote {
            let bound = entry.bound_port.load(Ordering::Relaxed);
            if bound != 0 {
                let h = self.handle.lock().await;
                let _ = h
                    .cancel_tcpip_forward(entry.rule.bind_host.as_str(), bound as u32)
                    .await;
            }
            self.remote
                .remove(&entry.rule.bind_host, entry.bound_port.load(Ordering::Relaxed));
        }
        // 2. 取消单规则令牌：-L/-D 停 accept；-R 回调查表已删，新回调 reject
        entry.rule_cancel.cancel();
        // 3. 排空活动连接 ≤ SHUTDOWN_DRAIN_SEC：计数器保持真实值不归零，
        //    逾期不再等待（copy 任务随后自然收尾，许可 RAII 归还）
        let deadline = tokio::time::Instant::now() + Duration::from_secs(SHUTDOWN_DRAIN_SEC);
        while entry.counters.active.load(Ordering::Relaxed) > 0
            && tokio::time::Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // 4. 置 Stopped 并即时推帧
        entry.set_state(TunnelState::Stopped, None);
        self.push_status().await;
        emit(
            log::Level::Info,
            "tunnel",
            events::TUNNEL_STOP,
            None,
            format!("隧道 {} 已停止", entry.rule.id),
            Some(serde_json::json!({
                "rule_id": entry.rule.id,
                "kind": format!("{:?}", entry.rule.kind),
            })),
        );
    }

    /// 会话收尾：取消全部规则任务并清理注册表。
    /// 由 `SshSession::shutdown` 在会话 cancel 前调用（同步实现，无 await）。
    pub(crate) fn shutdown(&self) {
        let entries: Vec<Arc<TunnelEntry>> = {
            let mut map = self.tunnels.lock().unwrap();
            map.drain().map(|(_, v)| v).collect()
        };
        for e in &entries {
            e.rule_cancel.cancel();
            e.set_state(TunnelState::Stopped, None);
        }
        self.remote.inner.lock().unwrap().clear();
        // 不等待排空：会话已断，copy 任务随 channel 死亡自然退出，许可 RAII 归还
    }

    /// 惰性启动 1s 节流状态推送任务（整个 manager 生命周期仅 spawn 一次）：
    /// 每秒醒来检查全部规则的脏标记，有变化才推全量快照（无变化不推）。
    fn ensure_status_task(&self) {
        if self
            .status_task_started
            .swap(true, Ordering::SeqCst)
        {
            return;
        }
        let tunnels = self.tunnels.clone();
        let frame_tx = self.frame_tx.clone();
        let cancel = self.cancel.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(STATUS_THROTTLE);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tick.tick() => {
                        let any_dirty = {
                            let map = tunnels.lock().unwrap();
                            let any = map.values().any(|e| e.dirty.load(Ordering::Relaxed));
                            if any {
                                for e in map.values() {
                                    e.dirty.store(false, Ordering::Relaxed);
                                }
                            }
                            any
                        };
                        if any_dirty {
                            push_tunnels_frame(&frame_tx, &tunnels).await;
                        }
                    }
                }
            }
        });
    }
}

/// 由规则表组装全量快照（锁外逐条读 entry 内部状态，无 await）
fn snapshot_of(map: &HashMap<String, Arc<TunnelEntry>>) -> Vec<TunnelStatus> {
    let mut list: Vec<TunnelStatus> = map.values().map(|e| e.status()).collect();
    // 按规则 id 稳定排序，前端整帧替换后展示顺序不抖动
    list.sort_by(|a, b| a.id.cmp(&b.id));
    list
}

/// 推送 0x0A 全量快照帧（帧队列关闭时静默忽略）
async fn push_tunnels_frame(
    frame_tx: &mpsc::Sender<Vec<u8>>,
    tunnels: &Arc<StdMutex<HashMap<String, Arc<TunnelEntry>>>>,
) {
    let list = {
        let map = tunnels.lock().unwrap();
        snapshot_of(&map)
    };
    let payload = serde_json::json!({ "tunnels": list });
    let json = serde_json::to_vec(&payload).unwrap_or_default();
    if let Err(e) = frame_tx.send(encode_frame(FrameType::Tunnel, &json)).await {
        emit(
            log::Level::Warn,
            "tunnel",
            events::TUNNEL_STATUS_PUSH_FAILED,
            None,
            format!("状态帧推送失败（帧通道已关闭）: {e}"),
            Some(serde_json::json!({"error": e.to_string()})),
        );
    }
}

/* =========================================================
 * 本地监听（-L/-D 共用）：监督器 + accept 循环 + 三层闸门 + 双向 copy
 * ========================================================= */

/// 本地监听任务共享上下文（-L/-D 通用；-R 回调走 RemoteBinding 同一套闸门）
struct ListenCtx {
    handle: Arc<Mutex<client::Handle<super::session::ClientHandler>>>,
    session_cancel: CancellationToken,
    global_conns: Arc<Semaphore>,
    frame_tx: mpsc::Sender<Vec<u8>>,
    tunnels: Arc<StdMutex<HashMap<String, Arc<TunnelEntry>>>>,
    dns_resolve: DnsResolve,
}

/// accept 循环退出原因
enum AcceptOutcome {
    /// 规则/会话取消，正常退出
    Cancelled,
    /// 系统级错误，交给监督器退避重启
    Fatal(std::io::Error),
}

/// accept 瞬时错误：内核偶尔返回的良性错误，直接 continue 不计监督器失败
fn is_transient_accept_error(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::Interrupted
            | std::io::ErrorKind::WouldBlock
    )
}

/// 指数退避：1s 起翻倍，封顶 SUPERVISOR_RETRY_MAX_MS
fn supervisor_backoff(failures: u32) -> Duration {
    let shift = failures.saturating_sub(1).min(20);
    Duration::from_millis(
        (SUPERVISOR_RETRY_BASE_MS.saturating_mul(1 << shift)).min(SUPERVISOR_RETRY_MAX_MS),
    )
}

/// 建立期瞬态错误退避（-R 重试）：attempt 从 1 起，1s 翻倍封顶 RETRY_MAX_MS
fn retry_backoff(attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(20);
    Duration::from_millis((RETRY_BASE_MS.saturating_mul(1 << shift)).min(RETRY_MAX_MS))
}

/// 监听器监督器：accept 循环因系统级错误（如 EMFILE）退出时按指数退避重启，
/// 复用原 bind_host:bound_port（next 顺延后的端口不变），规则保持 Active，
/// 已建立的活动连接不受影响；连续 SUPERVISOR_MAX_RETRIES 次失败才置 Error。
/// 重启瞬间端口被外部进程抢占时 bind 失败同样计入失败计数（运行中端口抢占不在自愈范围）。
async fn supervise_listen(entry: Arc<TunnelEntry>, ctx: Arc<ListenCtx>, listener: TcpListener) {
    let mut failures: u32 = 0;
    let mut cur = Some(listener);
    loop {
        // 取得监听器：首次直接用；重启先退避再 rebind（复用 bound_port）
        let listener = if let Some(l) = cur.take() {
            l
        } else {
            let delay = supervisor_backoff(failures);
            tokio::select! {
                _ = ctx.session_cancel.cancelled() => return,
                _ = entry.rule_cancel.cancelled() => return,
                _ = tokio::time::sleep(delay) => {}
            }
            let bind_port = entry.bound_port.load(Ordering::Relaxed);
            match TcpListener::bind((entry.rule.bind_host.as_str(), bind_port)).await {
                Ok(l) => l,
                Err(e) => {
                failures += 1;
                emit(
                    log::Level::Warn,
                    "tunnel",
                    events::TUNNEL_LISTENER_RETRY,
                    None,
                    format!("隧道 {} 监听器重启 bind 失败（第 {failures} 次）: {e}", entry.rule.id),
                    Some(serde_json::json!({
                        "rule_id": entry.rule.id,
                        "failures": failures,
                        "error": e.to_string(),
                    })),
                );
                if failures > SUPERVISOR_MAX_RETRIES {
                    entry.set_state(
                        TunnelState::Error,
                        Some(format!("监听器重启失败: {e}")),
                    );
                    push_tunnels_frame(&ctx.frame_tx, &ctx.tunnels).await;
                    return;
                }
                    continue;
                }
            }
        };
        // accept 循环
        match accept_loop(&entry, &ctx, listener).await {
            AcceptOutcome::Cancelled => return,
            AcceptOutcome::Fatal(e) => {
                failures += 1;
                if failures > SUPERVISOR_MAX_RETRIES {
                    emit(
                        log::Level::Error,
                        "tunnel",
                        events::TUNNEL_LISTENER_FAILED,
                        None,
                        format!("隧道 {} accept 系统错误（连续 {failures} 次，标记为错误）: {e}", entry.rule.id),
                        Some(serde_json::json!({
                            "rule_id": entry.rule.id,
                            "failures": failures,
                            "error": e.to_string(),
                        })),
                    );
                    entry.set_state(
                        TunnelState::Error,
                        Some(format!("监听器连续失败: {e}")),
                    );
                    push_tunnels_frame(&ctx.frame_tx, &ctx.tunnels).await;
                    return;
                }
                emit(
                    log::Level::Warn,
                    "tunnel",
                    events::TUNNEL_LISTENER_RETRY,
                    None,
                    format!("隧道 {} accept 系统错误（第 {failures} 次，{}ms 后重启）: {e}", entry.rule.id, supervisor_backoff(failures).as_millis()),
                    Some(serde_json::json!({
                        "rule_id": entry.rule.id,
                        "failures": failures,
                        "error": e.to_string(),
                    })),
                );
            }
        }
    }
}

/// accept 循环：接入后过三层闸门（令牌桶 → 单规则信号量 → 全局信号量），
/// 全部 try/allow 语义失败立即关闭，绝不排队；通过后 spawn 连接任务。
/// 监听任务挂 rule_cancel + 会话 cancel 双令牌。
async fn accept_loop(
    entry: &Arc<TunnelEntry>,
    ctx: &Arc<ListenCtx>,
    listener: TcpListener,
) -> AcceptOutcome {
    loop {
        let accepted = tokio::select! {
            _ = entry.rule_cancel.cancelled() => return AcceptOutcome::Cancelled,
            _ = ctx.session_cancel.cancelled() => return AcceptOutcome::Cancelled,
            r = listener.accept() => r,
        };
        match accepted {
            Ok((tcp, peer)) => {
                // 闸门 1：令牌桶限流（平滑突发，抵御端口扫描式洪峰）
                if !entry.accept_limiter.allow() {
                    drop(tcp);
                    continue;
                }
                // 闸门 2/3：单规则 + 全局信号量硬上限（OwnedSemaphorePermit
                // 即 RAII 守卫，连接任务任何退出路径都自动归还）
                let (rule_permit, global_permit) =
                    match (entry.rule_conns.clone().try_acquire_owned(), ctx.global_conns.clone().try_acquire_owned()) {
                        (Ok(a), Ok(b)) => (a, b),
                        (a, b) => {
                            drop((a, b, tcp));
                            continue;
                        }
                    };
                tokio::spawn(handle_local_conn(
                    tcp,
                    peer,
                    entry.clone(),
                    ctx.clone(),
                    (rule_permit, global_permit),
                ));
            }
            Err(e) if is_transient_accept_error(&e) => continue,
            Err(e) => return AcceptOutcome::Fatal(e),
        }
    }
}

/// 活动连接计数 RAII 守卫：drop 时活动数 -1 并置脏标记（推快照）
struct ConnGuard(Arc<Counters>, Arc<AtomicBool>);

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Relaxed);
        self.1.store(true, Ordering::Relaxed);
    }
}

/// 单个本地接入连接（-L/-D）：HANDSHAKE_TIMEOUT 仅覆盖 accept → channel 打开
/// 阶段（含 SOCKS 握手）；成功后进入 [`run_relay_conn`] 双向 copy。
/// 信号量许可从 accept 起由本任务持有（覆盖握手期），任何路径归还。
async fn handle_local_conn(
    mut tcp: TcpStream,
    peer: std::net::SocketAddr,
    entry: Arc<TunnelEntry>,
    ctx: Arc<ListenCtx>,
    permits: (
        tokio::sync::OwnedSemaphorePermit,
        tokio::sync::OwnedSemaphorePermit,
    ),
) {
    let opened = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        // -D：先做 SOCKS5 握手解析目标（计入握手窗口）
        let (target_host, target_port) = match entry.rule.kind {
            TunnelType::Dynamic => match socks5::negotiate(&mut tcp, ctx.dns_resolve).await {
                Ok(req) => (req.host, req.port),
                // 协议失败已在内部回复错误码；I/O 错误连接已死。此处仅记
                // 失败类别（转发内容零记录），断开不影响监听器
                Err(e) => {
                    emit(
                        log::Level::Warn,
                        "tunnel",
                        events::TUNNEL_SOCKS_HANDSHAKE_FAILED,
                        None,
                        format!("隧道 {} SOCKS5 握手失败: {e}", entry.rule.id),
                        Some(serde_json::json!({
                            "rule_id": entry.rule.id,
                            "error": e.to_string(),
                        })),
                    );
                    return None;
                }
            },
            _ => (
                entry.rule.target_host.clone().unwrap_or_default(),
                entry.rule.target_port.unwrap_or(0),
            ),
        };
        // channel 打开需持 handle 锁（瞬时），持锁仅到 open 完成；
        // 数据 copy 阶段只持有 channel，不持 handle 锁
        let h = ctx.handle.lock().await;
        h.channel_open_direct_tcpip(
            target_host,
            target_port as u32,
            peer.ip().to_string(),
            peer.port() as u32,
        )
        .await
        .ok()
    })
    .await;

    match opened {
        Ok(Some(channel)) => {
            // -D：目标 channel 已打开，必须先回 CONNECT 成功（REP 0x00）再进入
            // 数据透传（RFC 1928：响应之后才能转发应用数据，否则客户端会把
            // 目标服务的首包当作 SOCKS 响应解析）。写失败视同客户端已断开，
            // channel 随 run_relay_conn 正常收尾。
            if entry.rule.kind == TunnelType::Dynamic {
                let _ = tcp
                    .write_all(&socks5::connect_reply(socks5::reply::SUCCEEDED))
                    .await;
            }
            // 许可随连接任务收尾归还（run_relay_conn 内 RAII），本函数直接结束
            run_relay_conn(channel, tcp, entry, permits).await;
            return;
        }
        Ok(None) => {
            // channel 打开失败（远端不可达/被拒/规则停止）：
            // -D 回 SOCKS 一般失败码后关闭；-L 直接关。russh 不暴露 open
            // failure 细类（连接拒绝与不可达同码），统一 0x01。
            // 监听器继续服务新连接。
            if entry.rule.kind == TunnelType::Dynamic {
                socks5::reject(&mut tcp, socks5::reply::GENERAL_FAILURE).await;
            }
        }
        Err(_) => {
            // HANDSHAKE_TIMEOUT 超时：强制关闭（slowloris 防线）
            emit(
                log::Level::Warn,
                "tunnel",
                events::TUNNEL_SOCKS_HANDSHAKE_TIMEOUT,
                None,
                format!("隧道 {} 连接 {} 握手超时", entry.rule.id, peer),
                Some(serde_json::json!({
                    "rule_id": entry.rule.id,
                    "peer": peer.to_string(),
                })),
            );
        }
    }
    // 显式 drop 表意：握手失败的路径许可也在栈上归还（RAII）
    drop(permits);
}

/// 转发连接运行期总控（-L/-D accept 与 -R 回调共用）：活动计数 +1 →
/// 双向 copy → 收尾（计数 -1、置脏推快照；许可随本任务栈 RAII 归还）。
/// -R 回调以 `tokio::spawn(run_relay_conn(...))` 启动，任何退出路径自动收尾。
pub(crate) async fn run_relay_conn(
    channel: russh::Channel<client::Msg>,
    tcp: TcpStream,
    entry: Arc<TunnelEntry>,
    permits: (
        tokio::sync::OwnedSemaphorePermit,
        tokio::sync::OwnedSemaphorePermit,
    ),
) {
    entry.counters.active.fetch_add(1, Ordering::Relaxed);
    entry.dirty.store(true, Ordering::Relaxed);
    let _guard = ConnGuard(entry.counters.clone(), entry.dirty.clone());
    relay_channel_tcp(channel, tcp, &entry).await;
    // 显式 drop 表意：许可归还（RAII，任何路径都会归还）
    drop(permits);
}

/// channel ↔ TCP 双向 copy（16KB 缓冲）：
/// - 下行（channel → TCP）：字节计入 bytes_down；结束时 shutdown TCP 写端给客户端 EOF；
/// - 上行（TCP → channel）：字节计入 bytes_up；结束时对 channel 发 EOF 给远端。
/// 任一路结束即触发对侧收尾，两路都退出后本函数返回（许可随调用方栈回收）。
async fn relay_channel_tcp(
    channel: russh::Channel<client::Msg>,
    tcp: TcpStream,
    entry: &TunnelEntry,
) {
    let (rh, wh) = channel.split();
    let (mut tcp_r, mut tcp_w) = tcp.into_split();

    // 下行任务：channel → tcp
    let counters_down = entry.counters.clone();
    let dirty_down = entry.dirty.clone();
    let down = tokio::spawn(async move {
        let mut rh = rh;
        let mut reader = rh.make_reader();
        let mut buf = vec![0u8; COPY_BUF];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tcp_w.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                    counters_down.bytes_down.fetch_add(n as u64, Ordering::Relaxed);
                    dirty_down.store(true, Ordering::Relaxed);
                }
            }
        }
        // channel 已 EOF/断开：shutdown TCP 写端，客户端读端收到 EOF
        let _ = tcp_w.shutdown().await;
    });

    // 上行任务：tcp → channel
    let counters_up = entry.counters.clone();
    let dirty_up = entry.dirty.clone();
    let up = tokio::spawn(async move {
        let mut writer = wh.make_writer();
        let mut buf = vec![0u8; COPY_BUF];
        loop {
            match tcp_r.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if writer.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                    counters_up.bytes_up.fetch_add(n as u64, Ordering::Relaxed);
                    dirty_up.store(true, Ordering::Relaxed);
                }
            }
        }
        // 客户端已 EOF：对 channel 发 EOF，远端连接收到关闭
        let _ = wh.eof().await;
    });

    // 两路都退出才算连接结束（许可/计数在 handle_local_conn 栈上统一收尾）
    let _ = down.await;
    let _ = up.await;
}

/* =========================================================
 * SOCKS5 握手纯函数（阶段 C 实现；模块在此提前占位保证结构稳定）
 * ========================================================= */
pub(crate) mod socks5;

/* =========================================================
 * 单测
 * ========================================================= */
#[cfg(test)]
mod tests;
