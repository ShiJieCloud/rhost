//! Hub：日志中心全局单例。
//!
//! 职责：级别过滤（热生效）→ 分配全局单调递增 seq → 打 ISO 8601 时间戳 →
//! ① 环形缓冲（VecDeque，容量 = logMaxLines，调小立即截头）
//! ② 订阅者 Channel 广播（多订阅者，send 失败即移除）
//! ③ mpsc 有界队列（1024）→ 落盘线程，三级背压（§3 决策 4）：
//!    高水位 80% 丢 DEBUG/INFO；硬满 WARN/ERROR 同步写 stderr（1s 内相同错误最多 5 条）；
//!    dropped 计数由 janitor 线程 5s 定时器上报。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;

use super::events;
use super::writer::{self, WriterConfig, WriterMsg};

/// 单条 JSONL 长度上限 8KB（超出截断 msg 并追加顶层 `_truncated: true`）
pub const JSONL_MAX_BYTES: usize = 8 * 1024;
/// 面板单条文本上限 2KB（前端格式化时截断）
pub const PANEL_MSG_MAX_BYTES: usize = 2 * 1024;
/// mpsc 队列容量
const QUEUE_CAPACITY: usize = 1024;
/// 高水位：80%，越过即丢 DEBUG/INFO
const HIGH_WATER: usize = QUEUE_CAPACITY * 8 / 10;
/// stderr 限流窗口
const STDERR_WINDOW: Duration = Duration::from_secs(1);
/// stderr 限流：窗口内相同错误最多条数
const STDERR_MAX_PER_WINDOW: u32 = 5;

/// 应用日志结构化条目（内存 / IPC / JSONL 共用同一 schema）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppLogEntry {
    pub seq: u64,
    pub ts: String,
    pub level: String,
    pub target: String,
    pub event_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sid: Option<String>,
    pub msg: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kv: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "_truncated")]
    pub truncated: Option<bool>,
}

/// 前端上报入参白名单（仅允许 level / eventId / msg / kv；
/// seq/ts/sid 由服务端生成，从类型层面杜绝伪造）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportAppLogInput {
    pub level: String,
    pub event_id: String,
    pub msg: String,
    #[serde(default)]
    pub kv: Option<serde_json::Value>,
}

/// IPC 推送批次（entries 按 seq 升序，单批硬上限 50 条）
#[derive(Debug, Clone, Serialize)]
pub struct LogBatch {
    pub entries: Vec<AppLogEntry>,
    /// 仅 replay 批次携带：客户端游标已被逐出缓冲
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lost_because: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_seq: Option<u64>,
}

/// 日志切割策略
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotateStrategy {
    Daily,
    Weekly,
    Monthly,
    None,
}

impl RotateStrategy {
    pub fn from_str(s: &str) -> Self {
        match s {
            "weekly" => RotateStrategy::Weekly,
            "monthly" => RotateStrategy::Monthly,
            "none" => RotateStrategy::None,
            _ => RotateStrategy::Daily,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            RotateStrategy::Daily => "daily",
            RotateStrategy::Weekly => "weekly",
            RotateStrategy::Monthly => "monthly",
            RotateStrategy::None => "none",
        }
    }
}

/// Hub 配置（热更新；storage_path 仅重启生效——init 时消费一次）
#[derive(Debug, Clone)]
pub struct HubConfig {
    pub collect: bool,
    pub level: log::LevelFilter,
    pub max_lines: usize,
    pub persist: bool,
    pub storage_path: String,
    pub rotate: RotateStrategy,
    pub max_files: usize,
    pub retention_days: u64,
}

impl Default for HubConfig {
    fn default() -> Self {
        Self {
            collect: true,
            level: log::LevelFilter::Info,
            max_lines: 5000,
            persist: true,
            storage_path: String::new(),
            rotate: RotateStrategy::Daily,
            max_files: 100,
            retention_days: 30,
        }
    }
}

/// IPC 入参：set_log_config 传递的设置快照（camelCase）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogConfigPayload {
    pub collect: bool,
    pub level: String,
    pub max_lines: u32,
    pub persist: bool,
    pub storage_path: String,
    pub rotate: String,
    pub max_files: u32,
    pub retention_days: u64,
}

impl HubConfig {
    pub fn from_payload(p: &LogConfigPayload) -> Self {
        Self {
            collect: p.collect,
            level: parse_level(&p.level),
            max_lines: p.max_lines as usize,
            persist: p.persist,
            storage_path: p.storage_path.clone(),
            rotate: RotateStrategy::from_str(&p.rotate),
            max_files: p.max_files as usize,
            retention_days: p.retention_days,
        }
    }

    /// 落盘线程关心的子集
    pub fn writer_config(&self) -> WriterConfig {
        WriterConfig {
            persist: self.persist,
            rotate: self.rotate,
        }
    }
}

pub fn parse_level(s: &str) -> log::LevelFilter {
    match s.to_lowercase().as_str() {
        "off" => log::LevelFilter::Off,
        "error" => log::LevelFilter::Error,
        "warn" => log::LevelFilter::Warn,
        "debug" => log::LevelFilter::Debug,
        "trace" => log::LevelFilter::Trace,
        _ => log::LevelFilter::Info,
    }
}

/// 全局级别过滤缓存（供 log::Log::enabled 快速判定，避免每条都锁 Mutex）
static CURRENT_LEVEL: AtomicUsize = AtomicUsize::new(log::LevelFilter::Info as usize);

pub fn current_level_filter() -> log::LevelFilter {
    match CURRENT_LEVEL.load(Ordering::Relaxed) {
        0 => log::LevelFilter::Off,
        1 => log::LevelFilter::Error,
        2 => log::LevelFilter::Warn,
        3 => log::LevelFilter::Info,
        4 => log::LevelFilter::Debug,
        5 => log::LevelFilter::Trace,
        _ => log::LevelFilter::Info,
    }
}

fn set_level_filter(level: log::LevelFilter) {
    CURRENT_LEVEL.store(level as usize, Ordering::Relaxed);
    log::set_max_level(level);
}

/// Hub 内部可变状态
struct HubState {
    config: HubConfig,
    buffer: VecDeque<AppLogEntry>,
    subscribers: Vec<Channel<LogBatch>>,
}

pub struct Hub {
    seq: AtomicU64,
    dropped: AtomicUsize,
    min_seq: AtomicU64,
    state: Mutex<HubState>,
    writer_tx: std::sync::mpsc::SyncSender<WriterMsg>,
    /// mpsc 队列深度估算（Hub 侧 try_send 成功 ++ / Writer 侧消费 --）
    queued_count: Arc<AtomicUsize>,
    log_dir: PathBuf,
    /// 后端整体配置文件（app config dir/app_config.json）；
    /// set_log_config 时回写 logs 节，下次启动 init 读取（logStoragePath 重启生效的载体）
    app_config_file: PathBuf,
    writer_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// janitor 线程退出信号（shutdown 时置位）
    pub(crate) shutdown_flag: Arc<AtomicBool>,
    /// stderr 限流：相同文本在 1s 窗口内最多 5 条
    stderr_limiter: Mutex<HashMap<String, (Instant, u32)>>,
}

impl Hub {
    pub fn new(cfg: HubConfig, app_config_file: PathBuf) -> std::io::Result<Self> {
        let log_dir = resolve_log_dir(&cfg.storage_path);
        std::fs::create_dir_all(&log_dir)?;

        let (writer_tx, writer_rx) = std::sync::mpsc::sync_channel::<WriterMsg>(QUEUE_CAPACITY);
        let queued_count = Arc::new(AtomicUsize::new(0));
        let writer_handle = {
            let rx = writer_rx;
            let dir = log_dir.clone();
            let wcfg = cfg.writer_config();
            let qc = Arc::clone(&queued_count);
            std::thread::Builder::new()
                .name("applog-writer".into())
                .spawn(move || writer::run(rx, dir, wcfg, qc))?
        };

        set_level_filter(cfg.level);

        Ok(Self {
            seq: AtomicU64::new(1),
            dropped: AtomicUsize::new(0),
            min_seq: AtomicU64::new(1),
            state: Mutex::new(HubState {
                config: cfg,
                buffer: VecDeque::new(),
                subscribers: Vec::new(),
            }),
            writer_tx,
            queued_count,
            log_dir,
            app_config_file,
            writer_handle: Mutex::new(Some(writer_handle)),
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            stderr_limiter: Mutex::new(HashMap::new()),
        })
    }

    pub fn log_dir(&self) -> &Path {
        &self.log_dir
    }

    /// 解析设置面板配置的日志存储路径：
    /// 空串 → 当前运行实例实际生效目录（系统默认或上次持久化的自定义路径）；
    /// 非空 → 展开 `~` 前缀。仅做路径解析，不触碰磁盘（创建由调用方按需进行）。
    pub fn resolve_storage_dir(&self, storage_path: &str) -> PathBuf {
        if storage_path.trim().is_empty() {
            self.log_dir.to_path_buf()
        } else {
            PathBuf::from(expand_tilde(storage_path.trim()))
        }
    }

    /// panic hook 专用：分配全局 seq（原子操作，Mutex 中毒也安全）。
    /// 旁路写文件的 panic 行仍占用 seq 空间，保持全局单调不冲突。
    pub(crate) fn alloc_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }

    /// panic hook 专用：非阻塞读取 `(persist, rotate)`。
    /// - 正常拿到锁：读当前生效配置（尊重用户的 logPersist 开关）；
    /// - 锁占用（WouldBlock）：不等待，降级为 `(true, Daily)`——崩溃取证宁写勿漏；
    /// - 锁中毒：into_inner 强取（仅读 Copy 字段，安全）。
    pub(crate) fn panic_file_params(&self) -> Option<(bool, RotateStrategy)> {
        match self.state.try_lock() {
            Ok(g) => Some((g.config.persist, g.config.rotate)),
            Err(std::sync::TryLockError::Poisoned(p)) => {
                let g = p.into_inner();
                Some((g.config.persist, g.config.rotate))
            }
            Err(std::sync::TryLockError::WouldBlock) => None,
        }
    }

    /// 核心入口：所有日志统一走这里（emit / log 宏兜底 / 前端上报）
    pub fn log(&self, mut entry: AppLogEntry) {
        // PTY 隔离硬约束：_pty 前缀 target 拒绝入管线（§8）
        if entry.target.contains("_pty") {
            return;
        }
        let Ok(level) = entry.level.parse::<log::Level>() else {
            return;
        };

        let (collect, persist, max_lines) = {
            let state = self.state.lock().unwrap();
            (
                state.config.collect,
                state.config.persist,
                state.config.max_lines,
            )
        };

        // 采集总开关：关闭时仅 ERROR 落 stderr（§7.3）
        if !collect {
            if level == log::Level::Error {
                self.write_stderr(&entry);
            }
            return;
        }

        // 级别过滤（热生效）
        if level > current_level_filter() {
            return;
        }

        // 分配 seq + 打时间戳（本地，毫秒，带时区偏移）
        entry.seq = self.seq.fetch_add(1, Ordering::Relaxed);
        entry.ts = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false);

        // ① 环形缓冲（容量满截头，维护 min_seq）
        {
            let mut state = self.state.lock().unwrap();
            if state.buffer.len() >= max_lines {
                if let Some(removed) = state.buffer.pop_front() {
                    self.min_seq.store(removed.seq + 1, Ordering::Relaxed);
                }
            }
            state.buffer.push_back(entry.clone());
        }

        // ② 订阅者广播（未订阅零开销）
        self.broadcast(&entry);

        // ③ 落盘（含 ERROR 崩溃保护与三级背压）
        if persist {
            self.try_persist(entry, level);
        } else if level == log::Level::Error {
            // 不落盘时 ERROR 仍同步 stderr（崩溃保护兜底）
            self.write_stderr(&entry);
        }
    }

    /// 前端上报入口：白名单校验（仅 web.* 域），msg ≤ 2KB
    pub fn report(&self, input: ReportAppLogInput) {
        if input.level.parse::<log::Level>().is_err() {
            return;
        }
        if !events::REPORT_ALLOWED_PREFIXES
            .iter()
            .any(|p| input.event_id.starts_with(p))
        {
            return;
        }
        // 控制字符校验：msg 不含 \x00-\x08（§8）
        if input.msg.chars().any(|c| ('\x00'..='\x08').contains(&c)) {
            return;
        }
        let msg = truncate_chars(&input.msg, PANEL_MSG_MAX_BYTES);
        // target 与 event_id domain 同源（§4：domain 与 target 同源）
        let target = input
            .event_id
            .split('.')
            .next()
            .unwrap_or("web")
            .to_string();
        self.log(AppLogEntry {
            seq: 0,
            ts: String::new(),
            level: input.level.to_lowercase(),
            target,
            event_id: input.event_id,
            sid: None,
            msg,
            kv: input.kv,
            truncated: None,
        });
    }

    /// 注册订阅者：先推 replay（seq > since_id 的存量；越界推全量 + 元信息），
    /// 再加入增量广播列表。send 失败的订阅者在下次广播时自动摘除。
    pub fn subscribe(&self, channel: Channel<LogBatch>, since_id: Option<u64>) {
        const BATCH_LIMIT: usize = 50;
        let (entries, lost_because, from_seq) = {
            let state = self.state.lock().unwrap();
            let min_seq = self.min_seq.load(Ordering::Relaxed);
            match since_id {
                Some(since) if since >= min_seq => (
                    state
                        .buffer
                        .iter()
                        .filter(|e| e.seq > since)
                        .cloned()
                        .collect::<Vec<_>>(),
                    None,
                    None,
                ),
                Some(_) => (
                    state.buffer.iter().cloned().collect::<Vec<_>>(),
                    Some("buffer_evict".to_string()),
                    Some(min_seq),
                ),
                None => (state.buffer.iter().cloned().collect::<Vec<_>>(), None, None),
            }
        };

        // replay 分批推送（单批硬上限 50 条）；空 replay 也发一个空批作订阅确认；
        // 元信息只挂在首个 replay 批次
        if entries.is_empty() {
            let _ = channel.send(LogBatch {
                entries: vec![],
                lost_because,
                from_seq,
            });
        } else {
            for (i, chunk) in entries.chunks(BATCH_LIMIT).enumerate() {
                let _ = channel.send(LogBatch {
                    entries: chunk.to_vec(),
                    lost_because: if i == 0 { lost_because.clone() } else { None },
                    from_seq: if i == 0 { from_seq } else { None },
                });
            }
        }

        self.state.lock().unwrap().subscribers.push(channel);
    }

    /// 单条增量广播（增量不攒批：日志频率低，100ms/20 条攒批的收益
    /// 抵不上实现复杂度；单批 50 上限约束的是 replay 与突发场景）
    fn broadcast(&self, entry: &AppLogEntry) {
        let mut state = self.state.lock().unwrap();
        if state.subscribers.is_empty() {
            return;
        }
        let batch = LogBatch {
            entries: vec![entry.clone()],
            lost_because: None,
            from_seq: None,
        };
        state
            .subscribers
            .retain(|sub| sub.send(batch.clone()).is_ok());
    }

    /// 三级背压落盘（§3 决策 4）
    fn try_persist(&self, entry: AppLogEntry, level: log::Level) {
        // ERROR 崩溃保护：入队的同时同步写 stderr 副本（不攒批、不依赖 mpsc）
        if level == log::Level::Error {
            self.write_stderr(&entry);
        }

        let queued = self.queued_count.load(Ordering::Relaxed);
        if queued >= QUEUE_CAPACITY {
            // 硬满：WARN/ERROR 不再入队，同步写 stderr + 计数
            self.dropped.fetch_add(1, Ordering::Relaxed);
            if level <= log::Level::Warn {
                self.write_stderr(&entry);
            }
            return;
        }
        if queued >= HIGH_WATER && level >= log::Level::Info {
            // 高水位：丢 DEBUG/INFO
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        match self.writer_tx.try_send(WriterMsg::Entry(entry)) {
            Ok(()) => {
                self.queued_count.fetch_add(1, Ordering::Relaxed);
            }
            Err(std::sync::mpsc::TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {}
        }
    }

    /// stderr 输出（带限流：1s 内相同文本最多 5 条，防风暴）
    fn write_stderr(&self, entry: &AppLogEntry) {
        let key = format!("{}:{}", entry.event_id, entry.msg);
        let mut limiter = self.stderr_limiter.lock().unwrap();
        let now = Instant::now();
        let (window_start, count) = limiter.entry(key).or_insert((now, 0));
        if now.duration_since(*window_start) > STDERR_WINDOW {
            *window_start = now;
            *count = 0;
        }
        if *count >= STDERR_MAX_PER_WINDOW {
            return;
        }
        *count += 1;
        drop(limiter);
        eprintln!(
            "[{}] [{}] {}",
            entry.level.to_uppercase(),
            entry.target,
            entry.msg
        );
    }

    /// janitor 5s 定时器回调：dropped 计数非零即产出 app.log.dropped
    pub fn report_dropped(&self) {
        let count = self.dropped.swap(0, Ordering::Relaxed);
        if count > 0 {
            self.log(AppLogEntry {
                seq: 0,
                ts: String::new(),
                level: "warn".to_string(),
                target: "app".to_string(),
                event_id: events::APP_LOG_DROPPED.to_string(),
                sid: None,
                msg: "日志队列溢出，已丢弃".to_string(),
                kv: Some(serde_json::json!({ "dropped": count })),
                truncated: None,
            });
        }
    }

    /// 过期日志清理（启动时 + janitor 每小时）：任一规则满足即删除
    pub fn cleanup_logs(&self) {
        let (max_files, retention_days) = {
            let state = self.state.lock().unwrap();
            (state.config.max_files, state.config.retention_days)
        };
        let current = writer::current_file_name(
            self.state.lock().unwrap().config.rotate,
            chrono::Local::now(),
        );
        match writer::cleanup(&self.log_dir, max_files, retention_days, &current) {
            Ok((removed, freed_kb)) if removed > 0 => {
                self.log(AppLogEntry {
                    seq: 0,
                    ts: String::new(),
                    level: "info".to_string(),
                    target: "app".to_string(),
                    event_id: events::APP_LOG_CLEANUP.to_string(),
                    sid: None,
                    msg: "日志清理".to_string(),
                    kv: Some(serde_json::json!({
                        "removed": removed,
                        "freed_kb": freed_kb,
                    })),
                    truncated: None,
                });
            }
            _ => {}
        }
    }

    /// 热更新配置：级别立即生效；max_lines 调小立即截头；
    /// persist/rotate/清理参数转发落盘线程；storage_path 仅重启生效（忽略）。
    pub fn set_config(&self, cfg: HubConfig) {
        set_level_filter(cfg.level);
        let _ = self
            .writer_tx
            .send(WriterMsg::UpdateConfig(cfg.writer_config()));
        let mut state = self.state.lock().unwrap();
        if cfg.max_lines < state.config.max_lines {
            while state.buffer.len() > cfg.max_lines {
                if let Some(removed) = state.buffer.pop_front() {
                    self.min_seq.store(removed.seq + 1, Ordering::Relaxed);
                }
            }
        }
        state.config = cfg;
    }

    /// 清空内存缓冲（面板「清空日志」按钮；只清内存，不删磁盘文件）
    pub fn clear_buffer(&self) {
        let mut state = self.state.lock().unwrap();
        state.buffer.clear();
        self.min_seq
            .store(self.seq.load(Ordering::Relaxed), Ordering::Relaxed);
    }

    /// 落盘队列中未消费条数（app.shutdown.log_flushed 事件 kv）
    pub fn pending_count(&self) -> usize {
        self.queued_count.load(Ordering::Relaxed)
    }

    /// 热更新配置并逐字段审计（app.settings.change）：敏感键只记 changed=true（§8）
    pub fn set_config_with_audit(&self, new_cfg: HubConfig) {
        let old = self.state.lock().unwrap().config.clone();
        self.set_config(new_cfg.clone());

        // 逐字段对比日志相关键，产出变更事件（配置键均非敏感，仍统一查表）
        let changes: Vec<(&str, String)> = [
            (
                "logCollect",
                old.collect != new_cfg.collect,
                new_cfg.collect.to_string(),
            ),
            (
                "logLevel",
                old.level != new_cfg.level,
                new_cfg.level.to_string().to_lowercase(),
            ),
            (
                "logMaxLines",
                old.max_lines != new_cfg.max_lines,
                new_cfg.max_lines.to_string(),
            ),
            (
                "logPersist",
                old.persist != new_cfg.persist,
                new_cfg.persist.to_string(),
            ),
            (
                "logRotate",
                old.rotate != new_cfg.rotate,
                new_cfg.rotate.as_str().to_string(),
            ),
            (
                "logMaxFiles",
                old.max_files != new_cfg.max_files,
                new_cfg.max_files.to_string(),
            ),
            (
                "logRetentionDays",
                old.retention_days != new_cfg.retention_days,
                new_cfg.retention_days.to_string(),
            ),
            (
                "logStoragePath",
                old.storage_path != new_cfg.storage_path,
                new_cfg.storage_path.clone(),
            ),
        ]
        .into_iter()
        .filter(|(_, changed, _)| *changed)
        .map(|(k, _, v)| (k, v))
        .collect();

        for (key, value) in changes {
            let kv = if events::is_sensitive_key(key) {
                serde_json::json!({ "key": key, "changed": true })
            } else {
                serde_json::json!({ "key": key, "value": value })
            };
            self.log(AppLogEntry {
                seq: 0,
                ts: String::new(),
                level: "info".to_string(),
                target: "app".to_string(),
                event_id: events::APP_SETTINGS_CHANGE.to_string(),
                sid: None,
                msg: "设置变更".to_string(),
                kv: Some(kv),
                truncated: None,
            });
        }
        // 落盘职责已拆至 IPC 层（set_log_config 持全局写锁后调 save_logs）：
        // Hub 只负责内存热更新与审计，持久化失败的 WARN 也由 IPC 层记录。
    }

    /// 后端整体配置文件路径（app_config.json）；IPC 层在写锁内据此落盘 logs 节
    pub fn app_config_file(&self) -> &Path {
        &self.app_config_file
    }

    /// 关闭 Hub：通知落盘线程 flush 并 join（Tauri ExitRequested 钩子调用）
    pub fn shutdown(&self) {
        self.shutdown_flag.store(true, Ordering::Relaxed);
        let _ = self.writer_tx.send(WriterMsg::Shutdown);
        if let Some(handle) = self.writer_handle.lock().unwrap().take() {
            let _ = handle.join();
        }
    }
}

/// 解析日志目录：logStoragePath 留空 = 平台默认目录；支持 ~ 展开
fn resolve_log_dir(storage_path: &str) -> PathBuf {
    if storage_path.is_empty() {
        default_log_dir()
    } else {
        PathBuf::from(expand_tilde(storage_path))
    }
}

/// 平台默认日志目录（logStoragePath 留空时的落点；同时经 AppConfigSnapshot 下发给设置面板展示）
pub fn default_log_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join("Library/Logs/com.rhost.app");
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".local/share/com.rhost.app/logs");
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            return PathBuf::from(local).join("com.rhost.app").join("logs");
        }
    }
    std::env::temp_dir().join("com.rhost.app/logs")
}

fn expand_tilde(path: &str) -> String {
    if path == "~" || path.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{}{}", home, &path[1..]);
        }
    }
    path.to_string()
}

/// 按字符边界截断到 max_bytes 以内
pub fn truncate_chars(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_respects_char_boundary() {
        let s = "中文测试abc"; // 中文 3 字节/字
        let t = truncate_chars(s, 5);
        assert_eq!(t, "中"); // 第 5 字节落在「文」中间，回退到「中」
        assert_eq!(truncate_chars("abc", 5), "abc");
    }

    #[test]
    fn truncate_chars_no_panic_on_exact_boundary() {
        let s = "中文";
        assert_eq!(truncate_chars(s, 3), "中");
        assert_eq!(truncate_chars(s, 6), "中文");
    }

    #[test]
    fn level_parsing() {
        assert_eq!(parse_level("debug"), log::LevelFilter::Debug);
        assert_eq!(parse_level("WARN"), log::LevelFilter::Warn);
        assert_eq!(parse_level("unknown"), log::LevelFilter::Info);
    }
}
