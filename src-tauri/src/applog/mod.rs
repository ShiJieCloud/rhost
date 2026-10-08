//! 应用日志（App Log）中心：Rhost 客户端自身运行事件的采集、过滤、缓冲、推送与落盘。
//!
//! 架构（设计文档 docs/applog-design.md §3）：
//! ```text
//! applog::emit（项目埋点，带 event_id）     report_app_log（前端上报）
//!        │                                        │
//!   log::*! 宏（第三方 crate / 旧代码兜底）          │
//!        │                                        │
//!        ▼                                        ▼
//!   applog::Hub（全局单例）：级别过滤 → 分配 seq → 打时间戳
//!        │               │                    │
//!   ① 环形缓冲      ② 订阅者 Channel      ③ mpsc → 落盘线程
//!    (replay 用)     广播（多订阅者）       （攒批 flush + 轮转 + 清理）
//! ```
//!
//! 硬约束（§1/§8）：
//! - 高频推送走 Tauri Channel，禁止全局 Event；
//! - PTY 字节流严禁进入管线：target 含 `_pty` 前缀的日志被 Hub 拒绝；
//! - 埋点零副作用：禁止 IO/网络/锁等待，只读内存已有变量。

// events 常量表是事件注册表：部分常量由 P2 埋点阶段消费，暂未见引用不视为死代码
#[allow(dead_code)]
pub mod events;
pub mod hub;
mod logger;
pub mod persisted;
pub mod writer;

pub use hub::{AppLogEntry, Hub, HubConfig, LogBatch, LogConfigPayload, ReportAppLogInput};

use std::sync::OnceLock;
use std::sync::atomic::Ordering;

use hub::RotateStrategy;

/// 全局 Hub 单例（启动时 `init` 一次，之后经 `try_hub()` 访问）
static HUB: OnceLock<Hub> = OnceLock::new();

/// 项目埋点统一入口（带 event_id 的结构化日志）。
///
/// 与 `log::*!` 宏的区别：宏路径无法携带 event_id，仅作第三方 crate 与旧代码的
/// 兜底入口（统一包装为 `external.raw_log`）；项目自身埋点必须走本函数。
///
/// 零副作用约束：本函数只读写内存（原子量 + Mutex），无任何 IO 阻塞点。
pub fn emit(
    level: log::Level,
    target: &str,
    event_id: &str,
    sid: Option<&str>,
    msg: impl Into<String>,
    kv: Option<serde_json::Value>,
) {
    let Some(hub) = try_hub() else { return };
    hub.log(AppLogEntry {
        seq: 0,
        ts: String::new(),
        level: level.as_str().to_lowercase(),
        target: target.to_string(),
        event_id: event_id.to_string(),
        sid: sid.map(|s| s.to_string()),
        msg: msg.into(),
        kv,
        truncated: None,
    });
}

/// 初始化日志中心：创建 Hub（落盘线程）、注册 log::Log 门面、启动周期任务线程。
///
/// 必须在 Tauri `.setup()` 最前面调用（先于其他全局状态挂载），
/// 保证 `app.boot.start` 是全局第一条日志。
/// `app_config_file` 为后端整体配置文件路径（app config dir/app_config.json），
/// 由调用方先用 [`persisted::load_full`] 读取并取 `logs_config()` 节，此处只负责
/// 交给 Hub 供后续 `set_log_config` 回写 logs 节。
/// 返回日志目录实际路径（供 boot.log_dir_ready 事件 kv）。
pub fn init(
    cfg: HubConfig,
    app_config_file: std::path::PathBuf,
) -> std::io::Result<std::path::PathBuf> {
    let hub = Hub::new(cfg, app_config_file)?;
    let dir = hub.log_dir().to_path_buf();
    if HUB.set(hub).is_err() {
        return Ok(dir); // 重复初始化（单测场景）：静默忽略
    }
    log::set_logger(&logger::APP_LOGGER).expect("applog::init 只能调用一次");
    log::set_max_level(hub::current_level_filter());
    install_panic_hook();
    spawn_janitor();
    Ok(dir)
}

/// 访问全局 Hub（未初始化时返回 None：单测等场景允许无 Hub 运行）
pub fn try_hub() -> Option<&'static Hub> {
    HUB.get()
}

/// 周期任务线程（janitor）：
/// - 每 5s 检查 dropped 计数并产出 `app.log.dropped`（§3 决策 4，不依赖下一条日志触发）；
/// - 每 1h 执行一次过期日志清理（§6.4，启动时清理由 lib.rs 在 setup 流程中显式触发）。
fn spawn_janitor() {
    std::thread::Builder::new()
        .name("applog-janitor".into())
        .spawn(|| {
            let mut tick: u32 = 0;
            loop {
                std::thread::sleep(std::time::Duration::from_secs(5));
                let Some(hub) = try_hub() else { continue };
                if hub.shutdown_flag.load(Ordering::Relaxed) {
                    break;
                }
                hub.report_dropped();
                tick = tick.wrapping_add(1);
                // 5s × 720 = 3600s = 1h
                if tick % 720 == 0 {
                    hub.cleanup_logs();
                }
            }
        })
        .expect("applog janitor 线程启动失败");
}

/* =========================================================
 *  panic hook 兜底（§6.2/§13）
 * ========================================================= */

/// 安装 panic hook：
/// - 先调用先前的 hook（保留默认 stderr 输出行为）；
/// - 再把 panic 现场以 `app.panic` 结构化 JSONL **直接追加**当前日志文件，
///   绕过 Hub/mpsc/落盘线程——panic 时管线可能已不可用（Mutex 中毒、线程异常）；
/// - 追加与落盘线程并发安全：O_APPEND 每次写原子定位末尾，单行 < 8KB 不会交错；
/// - 兜底逻辑自身再 panic 时直接 abort，防止 hook 递归。
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // 1. 默认行为（stderr 标准 panic 文本）
        previous(info);
        // 2. JSONL 尽力追加；hook 内任何二次 panic → abort
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            append_panic_entry(info);
        }))
        .is_err()
        {
            let mut err = std::io::stderr();
            use std::io::Write;
            let _ = err.write_all(b"[FATAL] panic hook panicked again, aborting\n");
            std::process::abort();
        }
    }));
}

/// 将 panic 现场序列化为 `app.panic` 行并追加日志文件；任何失败静默降级
/// （默认 hook 已输出 stderr，此处仅为尽力落盘）。
fn append_panic_entry(info: &std::panic::PanicHookInfo<'_>) {
    let Some(hub) = try_hub() else { return };
    // 锁占用时降级 (true, Daily)：崩溃取证宁写勿漏；persist=false 尊重用户开关只留 stderr
    let (persist, rotate) = hub
        .panic_file_params()
        .unwrap_or((true, RotateStrategy::Daily));
    if !persist {
        return;
    }

    let msg = panic_payload_msg(info.payload());
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_default();
    let thread = std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_string();
    // force_capture 无视 RUST_BACKTRACE；release 下符号可能不全但保留地址栈
    let backtrace = truncate_at_char_boundary(
        &std::backtrace::Backtrace::force_capture().to_string(),
        4096,
    );

    let entry = AppLogEntry {
        // seq 仍从全局原子分配：panic 行与管线内 seq 不冲突（仅文件内可能因旁路
        // 先写而与未 flush 的行呈倒序——崩溃时刻以时间戳为准，属可接受取舍）
        seq: hub.alloc_seq(),
        ts: chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false),
        level: "error".to_string(),
        target: "app".to_string(),
        event_id: events::APP_PANIC.to_string(),
        sid: None,
        msg,
        kv: Some(serde_json::json!({
            "thread": thread,
            "location": location,
            "backtrace": backtrace,
        })),
        truncated: None,
    };

    let line = writer::serialize_entry(&entry);
    let file_name = writer::current_file_name(rotate, chrono::Local::now());
    let path = hub.log_dir().join(file_name);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
    }
}

/// panic payload → 文本（常见为 `&str` / `String`，其余给兜底描述）；限 1KB。
fn panic_payload_msg(payload: &(dyn std::any::Any + Send)) -> String {
    let raw = if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "非字符串 panic payload".to_string()
    };
    // msg 1KB + backtrace 4KB，连同 JSON 框架保证单行不超 8KB
    truncate_at_char_boundary(&raw, 1024)
}

/// 在 char boundary 上截断，超出部分以省略号收尾
fn truncate_at_char_boundary(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// panic hook 端到端：触发真实 panic → 今日日志文件末行是合法 JSONL 的 app.panic
    #[test]
    fn panic_hook_appends_jsonl_entry() {
        let dir = std::env::temp_dir().join(format!(
            "rhost-panic-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cfg = HubConfig {
            storage_path: dir.to_string_lossy().to_string(),
            ..HubConfig::default()
        };
        let hub = Hub::new(cfg, dir.join(persisted::APP_CONFIG_FILE_NAME)).unwrap();
        // HUB 是进程级 OnceLock：已有 init/其他测试占用时跳过本用例
        if HUB.set(hub).is_err() {
            return;
        }

        // 先放一个 no-op hook，使 install 取到的 previous 不向测试输出噪音；
        // 用例结束恢复真正的默认 hook
        let real_default = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        install_panic_hook();

        let result = std::panic::catch_unwind(|| {
            panic!("panic hook 测试探针");
        });
        assert!(result.is_err());
        std::panic::set_hook(real_default);

        let fname = writer::current_file_name(RotateStrategy::Daily, chrono::Local::now());
        let content = std::fs::read_to_string(dir.join(&fname)).unwrap();
        let last = content.lines().last().unwrap();
        let v: serde_json::Value = serde_json::from_str(last).unwrap();
        assert_eq!(v["event_id"], "app.panic");
        assert_eq!(v["level"], "error");
        assert_eq!(v["target"], "app");
        assert!(v["seq"].as_u64().is_some_and(|n| n >= 1));
        assert!(v["msg"].as_str().unwrap().contains("panic hook 测试探针"));
        assert!(
            v["kv"]["location"]
                .as_str()
                .unwrap()
                .contains("applog/mod.rs")
        );
        assert!(v["kv"]["backtrace"].is_string());
        assert!(v["kv"]["thread"].is_string());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
