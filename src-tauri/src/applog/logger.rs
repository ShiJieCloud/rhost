//! log::Log 门面实现：捕获第三方 crate 与旧代码的 `log::*!` 调用。
//!
//! 项目自身埋点走 `applog::emit`（带 event_id）；经本门面进入的记录统一包装为
//! `external.raw_log`（原始 target 进 kv.crate_target），因为宏路径无法携带 event_id。
//! target 含 `_pty` 前缀的记录被拒（PTY 字节流严禁入管线，§8）。

use log::{Metadata, Record};

use super::hub::{AppLogEntry, current_level_filter};
use super::{events, try_hub};

/// 全局静态 logger 实例（set_logger 需要 &'static）
pub static APP_LOGGER: AppLogger = AppLogger;

pub struct AppLogger;

impl log::Log for AppLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= current_level_filter() && !metadata.target().contains("_pty")
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let Some(hub) = try_hub() else { return };

        hub.log(AppLogEntry {
            seq: 0,
            ts: String::new(),
            level: record.level().as_str().to_lowercase(),
            target: "external".to_string(),
            event_id: events::EXTERNAL_RAW_LOG.to_string(),
            sid: None,
            msg: format!("{}", record.args()),
            kv: Some(serde_json::json!({
                "crate_target": record.target(),
            })),
            truncated: None,
        });
    }

    fn flush(&self) {}
}
