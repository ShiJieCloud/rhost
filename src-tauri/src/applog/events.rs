//! 日志事件目录：`event_id` 常量集中定义（编译期防手误）。
//!
//! 命名规范：`domain.action[.subaction]`，全小写蛇形；
//! `domain` 与日志 `target` 同源（app/ssh/sftp/metrics/ipc/web/external）。
//! 新增事件必须同步：本常量表 + 设计文档 §4 表格 + 前端 stores/applog.ts（web.* 域）。

/* ---- 应用生命周期（target: app） ---- */
pub const APP_BOOT_START: &str = "app.boot.start";
pub const APP_BOOT_CONFIG_LOADED: &str = "app.boot.config_loaded";
pub const APP_BOOT_LOG_DIR_READY: &str = "app.boot.log_dir_ready";
pub const APP_BOOT_KEYCHAIN_READY: &str = "app.boot.keychain_ready";
pub const APP_BOOT_READY: &str = "app.boot.ready";
pub const APP_BOOT_FAILED: &str = "app.boot.failed";
pub const APP_SHUTDOWN_START: &str = "app.shutdown.start";
pub const APP_SHUTDOWN_SESSIONS_CLOSED: &str = "app.shutdown.sessions_closed";
pub const APP_SHUTDOWN_LOG_FLUSHED: &str = "app.shutdown.log_flushed";
pub const APP_EXIT: &str = "app.exit";
pub const APP_SETTINGS_CHANGE: &str = "app.settings.change";
pub const APP_LOG_CLEANUP: &str = "app.log.cleanup";
pub const APP_LOG_PERSIST_FAILED: &str = "app.log.persist_failed";
pub const APP_LOG_DROPPED: &str = "app.log.dropped";
pub const APP_LOG_CORRUPT_LINE: &str = "app.log.corrupt_line";
/// 日志配置写入后端配置文件失败（logStoragePath 等重启后无法保留）
pub const APP_LOG_CONFIG_PERSIST_FAILED: &str = "app.log.config_persist_failed";
pub const APP_CLOCK_ROLLBACK: &str = "app.clock.rollback";
/// panic hook 兜底事件：崩溃现场信息（thread/location/backtrace），
/// 由 panic hook 直接追加日志文件，不走 Hub 管线（§6.2/§13）
pub const APP_PANIC: &str = "app.panic";

/* ---- 应用配置导入导出（target: app） ---- */
/// 配置导出/备份成功
pub const APP_CONFIG_EXPORT: &str = "app.config.export";
/// 配置导出失败（stage=read/assemble/write）
pub const APP_CONFIG_EXPORT_FAILED: &str = "app.config.export_failed";
/// 配置导入开始（文件解析与版本校验通过后）
pub const APP_CONFIG_IMPORT_START: &str = "app.config.import.start";
/// 配置导入成功（kv 为各节合并计数）
pub const APP_CONFIG_IMPORT_COMPLETE: &str = "app.config.import.complete";
/// 配置导入失败（stage=size/parse/hash/decrypt/version/write）
pub const APP_CONFIG_IMPORT_FAILED: &str = "app.config.import.failed";
/// 非关键节（ui_state/quick_connect_history）损坏被容错跳过
pub const APP_CONFIG_IMPORT_SECTION_SKIPPED: &str = "app.config.import_section_skipped";
/// 恢复默认设置成功（仅 settings 节）
pub const APP_CONFIG_SETTINGS_RESET: &str = "app.config.settings_reset";
/// set_app_config_section 节名非法或 value schema 校验失败
pub const APP_CONFIG_WRITE_REJECTED: &str = "app.config.write_rejected";

/* ---- SSH 连接生命周期（target: ssh） ---- */
pub const SSH_CONNECT_START: &str = "ssh.connect.start";
pub const SSH_CONNECT_TCP: &str = "ssh.connect.tcp";
pub const SSH_CONNECT_TCP_FAILED: &str = "ssh.connect.tcp_failed";
pub const SSH_HANDSHAKE_START: &str = "ssh.handshake.start";
pub const SSH_HANDSHAKE_COMPLETE: &str = "ssh.handshake.complete";
pub const SSH_HANDSHAKE_FAILED: &str = "ssh.handshake.failed";
pub const SSH_HOSTKEY_FINGERPRINT: &str = "ssh.hostkey.fingerprint";
pub const SSH_AUTH_START: &str = "ssh.auth.start";
pub const SSH_AUTH_SUCCESS: &str = "ssh.auth.success";
pub const SSH_AUTH_FAILED: &str = "ssh.auth.failed";
pub const SSH_SESSION_CREATE: &str = "ssh.session.create";
pub const SSH_SESSION_CHANNEL_OPEN: &str = "ssh.session.channel_open";
pub const SSH_SESSION_PTY: &str = "ssh.session.pty";
pub const SSH_SESSION_ENV: &str = "ssh.session.env";
pub const SSH_SESSION_SHELL: &str = "ssh.session.shell";
pub const SSH_SESSION_FAILED: &str = "ssh.session.failed";
pub const SSH_SESSION_READY: &str = "ssh.session.ready";
pub const SSH_SESSION_INIT_SCRIPT_FAILED: &str = "ssh.session.init_script_failed";
pub const SSH_DISCONNECT: &str = "ssh.disconnect";
pub const SSH_RECONNECT: &str = "ssh.reconnect";
pub const SSH_SFTP_CHANNEL_OPEN: &str = "ssh.sftp.channel.open";
pub const SSH_SFTP_CHANNEL_CLOSE: &str = "ssh.sftp.channel.close";

/* ---- SFTP 传输与管理（target: sftp） ---- */
pub const SFTP_TRANSFER_ENQUEUE: &str = "sftp.transfer.enqueue";
pub const SFTP_TRANSFER_START: &str = "sftp.transfer.start";
pub const SFTP_TRANSFER_COMPLETE: &str = "sftp.transfer.complete";
pub const SFTP_TRANSFER_FAILED: &str = "sftp.transfer.failed";
pub const SFTP_TRANSFER_CANCEL: &str = "sftp.transfer.cancel";
pub const SFTP_TRANSFER_PAUSE: &str = "sftp.transfer.pause";
pub const SFTP_TRANSFER_RESUME: &str = "sftp.transfer.resume";
pub const SFTP_TRANSFER_VERIFY_FAILED: &str = "sftp.transfer.verify_failed";
pub const SFTP_MANAGE_REMOVE: &str = "sftp.manage.remove";
pub const SFTP_MANAGE_RENAME: &str = "sftp.manage.rename";
pub const SFTP_MANAGE_MKDIR: &str = "sftp.manage.mkdir";
pub const SFTP_MANAGE_COPY: &str = "sftp.manage.copy";

/* ---- 指标 / IPC / 前端 / 第三方 crate ---- */
pub const METRICS_COLLECT_START: &str = "metrics.collect.start";
pub const METRICS_COLLECT_STOP: &str = "metrics.collect.stop";
pub const METRICS_COLLECT_DEGRADED: &str = "metrics.collect.degraded";
pub const IPC_SLOW_CALL: &str = "ipc.slow_call";
pub const WEB_IPC_ERROR: &str = "web.ipc.error";
pub const WEB_UNHANDLED_ERROR: &str = "web.unhandled_error";
/// 第三方 crate 日志统一事件：原始 target 放 kv.crate_target
pub const EXTERNAL_RAW_LOG: &str = "external.raw_log";

/// 前端上报允许的 event_id 域（report_app_log 校验白名单）：
/// web.* 前端通用事件 + sftp.* 传输队列事件（队列管理逻辑在前端，§7.3）
pub const REPORT_ALLOWED_PREFIXES: &[&str] = &["web.", "sftp."];

/// 敏感设置键静态集合：settings.change 埋点统一查表脱敏，不记值只记 changed
pub const SENSITIVE_SETTING_KEYS: &[&str] = &[
    "password",
    "passphrase",
    "privateKey",
    "private_key",
    "token",
    "secret",
    "env", // env 数组含 value，整体只记 key 列表
];

/// 判断设置键是否敏感（子串匹配小写形式，覆盖 env.*.value 等派生键）
pub fn is_sensitive_key(key: &str) -> bool {
    let lower = key.to_lowercase();
    SENSITIVE_SETTING_KEYS
        .iter()
        .any(|s| lower.contains(&s.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_id_follows_naming_convention() {
        let all = [
            APP_BOOT_START, APP_BOOT_CONFIG_LOADED, APP_BOOT_LOG_DIR_READY,
            APP_BOOT_KEYCHAIN_READY, APP_BOOT_READY, APP_BOOT_FAILED,
            APP_SHUTDOWN_START, APP_SHUTDOWN_SESSIONS_CLOSED, APP_SHUTDOWN_LOG_FLUSHED,
            APP_EXIT, APP_SETTINGS_CHANGE, APP_LOG_CLEANUP, APP_LOG_PERSIST_FAILED,
            APP_LOG_DROPPED, APP_LOG_CORRUPT_LINE, APP_LOG_CONFIG_PERSIST_FAILED,
            APP_CLOCK_ROLLBACK, APP_PANIC,
            APP_CONFIG_EXPORT, APP_CONFIG_EXPORT_FAILED, APP_CONFIG_IMPORT_START,
            APP_CONFIG_IMPORT_COMPLETE, APP_CONFIG_IMPORT_FAILED,
            APP_CONFIG_IMPORT_SECTION_SKIPPED,
            APP_CONFIG_SETTINGS_RESET, APP_CONFIG_WRITE_REJECTED,
            SSH_CONNECT_START, SSH_CONNECT_TCP, SSH_CONNECT_TCP_FAILED,
            SSH_HANDSHAKE_START, SSH_HANDSHAKE_COMPLETE, SSH_HANDSHAKE_FAILED,
            SSH_HOSTKEY_FINGERPRINT, SSH_AUTH_START, SSH_AUTH_SUCCESS, SSH_AUTH_FAILED,
            SSH_SESSION_CREATE, SSH_SESSION_CHANNEL_OPEN, SSH_SESSION_PTY, SSH_SESSION_ENV,
            SSH_SESSION_SHELL, SSH_SESSION_FAILED, SSH_SESSION_READY,
            SSH_SESSION_INIT_SCRIPT_FAILED, SSH_DISCONNECT,
            SSH_RECONNECT, SSH_SFTP_CHANNEL_OPEN, SSH_SFTP_CHANNEL_CLOSE,
            SFTP_TRANSFER_ENQUEUE, SFTP_TRANSFER_START, SFTP_TRANSFER_COMPLETE,
            SFTP_TRANSFER_FAILED, SFTP_TRANSFER_CANCEL, SFTP_TRANSFER_PAUSE,
            SFTP_TRANSFER_RESUME, SFTP_TRANSFER_VERIFY_FAILED, SFTP_MANAGE_REMOVE,
            SFTP_MANAGE_RENAME, SFTP_MANAGE_MKDIR, SFTP_MANAGE_COPY,
            METRICS_COLLECT_START, METRICS_COLLECT_STOP, METRICS_COLLECT_DEGRADED,
            IPC_SLOW_CALL, WEB_IPC_ERROR, WEB_UNHANDLED_ERROR, EXTERNAL_RAW_LOG,
        ];
        for id in all {
            assert!(
                id.chars().all(|c| c.is_ascii_lowercase() || c == '.' || c == '_'),
                "event_id 必须全小写蛇形: {id}"
            );
            let mut parts = id.split('.');
            let domain = parts.next().unwrap();
            assert!(
                ["app", "ssh", "sftp", "metrics", "ipc", "web", "external"].contains(&domain),
                "未知 domain: {id}"
            );
            assert!(parts.next().is_some(), "event_id 至少两段: {id}");
        }
    }

    #[test]
    fn sensitive_key_matching_is_case_insensitive() {
        assert!(is_sensitive_key("password"));
        assert!(is_sensitive_key("Password"));
        assert!(is_sensitive_key("env.0.value"));
        assert!(!is_sensitive_key("logLevel"));
    }

    /// 提取文本中所有反引号/引号包裹的 `domain.action.sub` 形态事件 ID
    fn extract_ids(text: &str, delimiter: char) -> std::collections::BTreeSet<String> {
        text.split(delimiter)
            .skip(1)
            .step_by(2) // 分隔符之间的奇数段为被包裹内容
            .filter(|s| {
                let mut it = s.split('.');
                let domain = it.next().unwrap_or("");
                let rest: Vec<&str> = it.collect();
                ["app", "ssh", "sftp", "metrics", "ipc", "web", "external"].contains(&domain)
                    && !rest.is_empty()
                    && rest.iter().all(|p| {
                        !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                    })
            })
            .map(String::from)
            .collect()
    }

    /// 设计文档事件表 ↔ events.rs 常量值一致性：
    /// 新增/改名 event_id 时若漏改任一侧，本测试失败（§12 验收）。
    #[test]
    fn doc_table_matches_constants() {
        let manifest = env!("CARGO_MANIFEST_DIR"); // …/src-tauri
        let doc_path = std::path::Path::new(manifest)
            .join("..")
            .join("docs")
            .join("applog-design.md");
        let doc = std::fs::read_to_string(&doc_path)
            .unwrap_or_else(|e| panic!("读取设计文档失败 {}：{e}", doc_path.display()));
        let source = include_str!("events.rs");

        let doc_ids = extract_ids(&doc, '`');
        let code_ids = extract_ids(source, '"');

        let missing_in_doc: Vec<_> = code_ids.difference(&doc_ids).collect();
        let missing_in_code: Vec<_> = doc_ids.difference(&code_ids).collect();
        assert!(
            missing_in_doc.is_empty() && missing_in_code.is_empty(),
            "event_id 文档与代码不一致（代码缺={missing_in_code:?}，文档缺={missing_in_doc:?}）"
        );
    }
}
