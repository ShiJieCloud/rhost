//! 落盘线程：独立 std 线程消费 mpsc 队列，攒批 flush（200ms/64KB），
//! 懒切换轮转（写批前比对切割点），时间回拨 reclock 处理，过期清理。
//!
//! 崩溃保护：ERROR 级由 Hub 侧同步写 stderr 副本，不经过本线程。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Datelike, Local};

use super::events;
use super::hub::{AppLogEntry, JSONL_MAX_BYTES, RotateStrategy, truncate_chars};

/// 攒批 flush 阈值：字节数
const FLUSH_BYTES: usize = 64 * 1024;
/// 攒批 flush 阈值：距上次 flush 的时长
const FLUSH_INTERVAL: Duration = Duration::from_millis(200);
/// 回拨判定阈值：墙钟回跳超过 1s
const ROLLBACK_THRESHOLD_MS: i64 = 1000;

/// Hub → 落盘线程的消息
pub enum WriterMsg {
    Entry(AppLogEntry),
    /// 热更新落盘相关配置（persist/rotate）
    UpdateConfig(WriterConfig),
    Shutdown,
}

#[derive(Debug, Clone)]
pub struct WriterConfig {
    pub persist: bool,
    pub rotate: RotateStrategy,
}

struct WriterState {
    file: Option<File>,
    /// 当前文件名（含 reclock 后缀形式）
    file_name: String,
    buf: String,
    last_flush: Instant,
    /// 上一写批的墙钟时间（回拨检测基准）
    last_wall: DateTime<Local>,
    /// 回拨状态：true 时使用 _reclock 后缀文件
    reclock: bool,
    /// persist_failed 只报一次（防风暴）
    persist_failed_reported: bool,
}

/// 落盘线程主循环
pub fn run(
    rx: mpsc::Receiver<WriterMsg>,
    dir: PathBuf,
    mut cfg: WriterConfig,
    queued_count: Arc<AtomicUsize>,
) {
    let mut state = WriterState {
        file: None,
        file_name: String::new(),
        buf: String::with_capacity(FLUSH_BYTES + 1024),
        last_flush: Instant::now(),
        last_wall: Local::now(),
        reclock: false,
        persist_failed_reported: false,
    };

    loop {
        match rx.recv_timeout(FLUSH_INTERVAL) {
            Ok(WriterMsg::Entry(entry)) => {
                queued_count.fetch_sub(1, Ordering::Relaxed);
                if cfg.persist {
                    state.buf.push_str(&serialize_entry(&entry));
                    state.buf.push('\n');
                    if state.buf.len() >= FLUSH_BYTES {
                        write_batch(&mut state, &dir, &mut cfg);
                    }
                }
            }
            Ok(WriterMsg::UpdateConfig(new_cfg)) => {
                // persist 关闭：flush 残余后停写
                if cfg.persist && !new_cfg.persist {
                    write_batch(&mut state, &dir, &mut cfg);
                    state.file = None;
                }
                cfg = new_cfg;
            }
            Ok(WriterMsg::Shutdown) => {
                write_batch(&mut state, &dir, &mut cfg);
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if !state.buf.is_empty() {
                    write_batch(&mut state, &dir, &mut cfg);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                write_batch(&mut state, &dir, &mut cfg);
                break;
            }
        }
    }
}

/// 序列化为 JSONL 单行；超 8KB 截断 msg 并追加顶层 `_truncated: true`
/// （截断作用于 msg 字符串本身——在 char boundary 上截，不会截断 JSON 转义中间）。
///
/// `pub(super)`：panic hook 旁路写文件时复用同一序列化/截断逻辑（§6.2）。
pub(super) fn serialize_entry(entry: &AppLogEntry) -> String {
    let line = serde_json::to_string(entry).unwrap_or_default();
    if line.len() <= JSONL_MAX_BYTES {
        return line;
    }
    // 估算需要砍掉的字节数，从 msg 中截断（msg 是最长字段）
    let mut e = entry.clone();
    e.truncated = Some(true);
    let excess = line.len() - JSONL_MAX_BYTES + 32; // +32 给 _truncated 字段留位
    let keep = e.msg.len().saturating_sub(excess);
    e.msg = truncate_chars(&e.msg, keep);
    let mut line = serde_json::to_string(&e).unwrap_or_default();
    while line.len() > JSONL_MAX_BYTES && !e.msg.is_empty() {
        let keep = e.msg.len() * 9 / 10;
        e.msg = truncate_chars(&e.msg, keep);
        line = serde_json::to_string(&e).unwrap_or_default();
    }
    line
}

/// 写一个攒批：回拨检测 → 轮转切割 → 文件被删重开 → write_all
fn write_batch(state: &mut WriterState, dir: &Path, cfg: &mut WriterConfig) {
    state.last_flush = Instant::now();
    if state.buf.is_empty() || !cfg.persist {
        state.buf.clear();
        return;
    }

    let now = Local::now();

    // 系统时间回拨检测：墙钟回跳 > 1s → 切换 reclock 后缀文件，避免覆盖旧日期日志
    let delta = now.timestamp_millis() - state.last_wall.timestamp_millis();
    if delta < -ROLLBACK_THRESHOLD_MS && !state.reclock {
        state.reclock = true;
        state.file = None; // 强制换文件
        if let Some(hub) = super::try_hub() {
            hub.log(AppLogEntry {
                seq: 0,
                ts: String::new(),
                level: "warn".to_string(),
                target: "app".to_string(),
                event_id: events::APP_CLOCK_ROLLBACK.to_string(),
                sid: None,
                msg: "系统时间回拨，日志文件切换".to_string(),
                kv: Some(serde_json::json!({ "offset_ms": -delta })),
                truncated: None,
            });
        }
    }
    state.last_wall = now;

    // 懒切换轮转：写批前比对切割点（文件名变化即跨点）
    let want_name = current_file_name_reclock(cfg.rotate, now, state.reclock);
    if want_name != state.file_name {
        state.file = None;
        state.file_name = want_name;
    }

    let path = dir.join(&state.file_name);

    // 运行中文件被外部删除：句柄仍有效但文件已消失，重开
    if state.file.is_some() && fs::metadata(&path).is_err() {
        state.file = None;
    }

    if state.file.is_none() {
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(f) => state.file = Some(f),
            Err(e) => {
                persist_failed(state, cfg, &path, &e);
                state.buf.clear();
                return;
            }
        }
    }

    if let Some(f) = state.file.as_mut()
        && let Err(e) = f.write_all(state.buf.as_bytes())
    {
        persist_failed(state, cfg, &path, &e);
    }
    state.buf.clear();
}

/// 落盘失败分级降级：stderr 一次性 + app.log.persist_failed ERROR + 关闭持久化
fn persist_failed(
    state: &mut WriterState,
    cfg: &mut WriterConfig,
    path: &Path,
    err: &std::io::Error,
) {
    eprintln!("[ERROR] [app] 日志落盘失败: {}: {err}", path.display());
    cfg.persist = false; // 不再反复尝试写盘
    if !state.persist_failed_reported {
        state.persist_failed_reported = true;
        if let Some(hub) = super::try_hub() {
            hub.log(AppLogEntry {
                seq: 0,
                ts: String::new(),
                level: "error".to_string(),
                target: "app".to_string(),
                event_id: events::APP_LOG_PERSIST_FAILED.to_string(),
                sid: None,
                msg: "日志落盘失败".to_string(),
                kv: Some(serde_json::json!({
                    "path": path.display().to_string(),
                    "err": err.to_string(),
                })),
                truncated: None,
            });
        }
    }
}

/// 当前写批应使用的文件名（不含 reclock）
pub fn current_file_name(rotate: RotateStrategy, now: DateTime<Local>) -> String {
    current_file_name_reclock(rotate, now, false)
}

fn current_file_name_reclock(
    rotate: RotateStrategy,
    now: DateTime<Local>,
    reclock: bool,
) -> String {
    let base = match rotate {
        RotateStrategy::Daily => format!("rhost_app_{}.log", now.format("%Y%m%d")),
        RotateStrategy::Weekly => {
            let w = now.iso_week();
            format!("rhost_app_{}-W{:02}.log", w.year(), w.week())
        }
        RotateStrategy::Monthly => format!("rhost_app_{}.log", now.format("%Y%m")),
        RotateStrategy::None => "rhost_app.log".to_string(),
    };
    if reclock {
        base.replace(".log", "_reclock.log")
    } else {
        base
    }
}

/// 过期清理：扫描目录中 `rhost_app_*.log`，按 mtime——
/// `retention_days` 过期与 `max_files` 超量任一满足即删除（独立判断，不互斥）；
/// 当前正在写入的文件豁免。返回 (删除数, 释放 KB)。
pub fn cleanup(
    dir: &Path,
    max_files: usize,
    retention_days: u64,
    current_file: &str,
) -> std::io::Result<(usize, u64)> {
    let mut files: Vec<(PathBuf, SystemTime, u64)> = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("rhost_app_") || !name.ends_with(".log") {
            continue;
        }
        if name == current_file {
            continue; // 当前写入文件永不删除
        }
        let meta = entry.metadata()?;
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        files.push((entry.path(), mtime, meta.len()));
    }

    let now = SystemTime::now();
    let retention = Duration::from_secs(retention_days * 86400);
    let mut to_delete: Vec<&(PathBuf, SystemTime, u64)> = Vec::new();

    // 规则 1：过期删除（retention_days > 0 时启用）
    if retention_days > 0 {
        for f in &files {
            if now.duration_since(f.1).unwrap_or_default() > retention {
                to_delete.push(f);
            }
        }
    }

    // 规则 2：超量删除（max_files > 0 时启用），mtime 倒序保留最新
    if max_files > 0 {
        let mut sorted: Vec<&(PathBuf, SystemTime, u64)> = files.iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        for f in sorted.into_iter().skip(max_files) {
            if !to_delete.iter().any(|d| d.0 == f.0) {
                to_delete.push(f);
            }
        }
    }

    let mut removed = 0usize;
    let mut freed = 0u64;
    for (path, _, len) in to_delete {
        if fs::remove_file(path).is_ok() {
            removed += 1;
            freed += len;
        }
    }
    Ok((removed, freed / 1024))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(y: i32, mo: u32, d: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, 12, 0, 0).unwrap()
    }

    #[test]
    fn file_name_daily() {
        assert_eq!(
            current_file_name(RotateStrategy::Daily, ts(2026, 10, 6)),
            "rhost_app_20261006.log"
        );
    }

    #[test]
    fn file_name_weekly_iso() {
        // 2026-10-06 是 ISO 2026-W41
        let name = current_file_name(RotateStrategy::Weekly, ts(2026, 10, 6));
        assert_eq!(name, "rhost_app_2026-W41.log");
        // 跨年周：2026-01-01 是周四，属 ISO 2026-W01（首周含首个周四）
        let name = current_file_name(RotateStrategy::Weekly, ts(2026, 1, 1));
        assert_eq!(name, "rhost_app_2026-W01.log");
        // 真跨年例：2025-12-29（周一）属 ISO 2025-W52？不——2025-01-01 周三，
        // 故 2024-12-30（周一）已进入 ISO 2025-W01
        let name = current_file_name(RotateStrategy::Weekly, ts(2024, 12, 30));
        assert_eq!(name, "rhost_app_2025-W01.log");
    }

    #[test]
    fn file_name_monthly_and_none() {
        assert_eq!(
            current_file_name(RotateStrategy::Monthly, ts(2026, 10, 6)),
            "rhost_app_202610.log"
        );
        assert_eq!(
            current_file_name(RotateStrategy::None, ts(2026, 10, 6)),
            "rhost_app.log"
        );
    }

    #[test]
    fn file_name_reclock_suffix() {
        assert_eq!(
            current_file_name_reclock(RotateStrategy::Daily, ts(2026, 10, 6), true),
            "rhost_app_20261006_reclock.log"
        );
    }

    #[test]
    fn jsonl_serialization_fields() {
        let entry = AppLogEntry {
            seq: 1024,
            ts: "2026-10-05T14:23:01.123+08:00".to_string(),
            level: "info".to_string(),
            target: "ssh".to_string(),
            event_id: "ssh.connect.start".to_string(),
            sid: Some("a1b2c3".to_string()),
            msg: "正在连接".to_string(),
            kv: Some(serde_json::json!({"host": "192.168.1.10", "port": 22})),
            truncated: None,
        };
        let line = serialize_entry(&entry);
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["seq"], 1024);
        assert_eq!(v["event_id"], "ssh.connect.start");
        assert_eq!(v["sid"], "a1b2c3");
        assert!(
            v.get("_truncated").is_none(),
            "未截断时不应有 _truncated 键"
        );
    }

    #[test]
    fn jsonl_optional_fields_omitted() {
        let entry = AppLogEntry {
            seq: 1,
            ts: "t".to_string(),
            level: "info".to_string(),
            target: "app".to_string(),
            event_id: "app.boot.start".to_string(),
            sid: None,
            msg: "m".to_string(),
            kv: None,
            truncated: None,
        };
        let line = serialize_entry(&entry);
        assert!(!line.contains("\"sid\""), "sid=None 时省略该键");
        assert!(!line.contains("\"kv\""), "kv=None 时省略该键");
    }

    #[test]
    fn jsonl_truncation_marks_top_level_field() {
        let entry = AppLogEntry {
            seq: 1,
            ts: "t".to_string(),
            level: "info".to_string(),
            target: "app".to_string(),
            event_id: "app.test".to_string(),
            sid: None,
            msg: "长".repeat(16 * 1024),
            kv: None,
            truncated: None,
        };
        let line = serialize_entry(&entry);
        assert!(line.len() <= JSONL_MAX_BYTES + 64, "截断后应在 8KB 量级");
        let v: serde_json::Value = serde_json::from_str(&line).expect("截断后 JSON 仍合法");
        assert_eq!(v["_truncated"], true, "_truncated 必须是顶层字段");
        assert!(
            v.get("kv")
                .map(|k| k.get("_truncated").is_none())
                .unwrap_or(true),
            "_truncated 不得出现在 kv 内"
        );
    }

    #[test]
    fn jsonl_msg_newline_escaped() {
        let entry = AppLogEntry {
            seq: 1,
            ts: "t".to_string(),
            level: "info".to_string(),
            target: "app".to_string(),
            event_id: "app.test".to_string(),
            sid: None,
            msg: "line1\nline2".to_string(),
            kv: None,
            truncated: None,
        };
        let line = serialize_entry(&entry);
        assert!(!line.contains('\n'), "JSONL 单行不得含换行");
        assert!(line.contains("\\n"), "msg 内换行应转义为字面 \\n");
    }

    #[test]
    fn cleanup_deletes_expired_and_excess() {
        let dir = std::env::temp_dir().join(format!("applog-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        // 造 5 个日志文件，mtime 分别为 40/10/5/2/1 天前
        let ages_days = [40u64, 10, 5, 2, 1];
        for (i, age) in ages_days.iter().enumerate() {
            let p = dir.join(format!("rhost_app_2026090{i}.log"));
            fs::write(&p, b"x").unwrap();
            let mtime = SystemTime::now() - Duration::from_secs(age * 86400);
            let f = File::options().write(true).open(&p).unwrap();
            f.set_modified(mtime).unwrap();
        }
        // 当前写入文件（豁免）
        fs::write(dir.join("rhost_app_20261006.log"), b"x").unwrap();

        // 规则1：retention=30 天 → 40 天前的删
        let (removed, _) = cleanup(&dir, 0, 30, "rhost_app_20261006.log").unwrap();
        assert_eq!(removed, 1);

        // 规则2：max_files=2 → 剩余 4 个里最旧的 2 个删
        let (removed, _) = cleanup(&dir, 2, 0, "rhost_app_20261006.log").unwrap();
        assert_eq!(removed, 2);

        // 当前文件豁免：清理后仍在
        assert!(dir.join("rhost_app_20261006.log").exists());

        let _ = fs::remove_dir_all(&dir);
    }
}
