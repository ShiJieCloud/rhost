mod applog;
mod fonts;
mod ipc;
mod localfs;
mod metrics;
mod motd;
pub mod ssh;
mod store;
mod sysmon;

use applog::events as ev;
use ssh::manager::SessionManager;
use tauri::{Manager, RunEvent};

/// 应用启动时刻（退出时计算 uptime_s）
static BOOT_INSTANT: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = BOOT_INSTANT.set(std::time::Instant::now());

    tauri::Builder::default()
        // 终端内 http(s) 链接点击后交系统默认浏览器打开（WebView 内 window.open 不可靠）
        .plugin(tauri_plugin_opener::init())
        // 终端复制/粘贴的剪贴板读写（WKWebView 对 Web Clipboard API 读取有限制）
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            // ---- Hub 最先初始化，保证 boot.start 是全局第一条日志 ----
            // 后端整体配置文件（与 WebView localStorage 独立）：logStoragePath 重启生效
            // 的唯一载体，同时为后续整体配置导入导出铺垫；缺失/损坏时 load 回退默认值
            let app_config_file = app
                .path()
                .app_config_dir()
                .map(|d| d.join(applog::persisted::APP_CONFIG_FILE_NAME))
                .unwrap_or_else(|_| {
                    std::env::temp_dir()
                        .join("com.rhost.app")
                        .join(applog::persisted::APP_CONFIG_FILE_NAME)
                });
            let app_cfg = applog::persisted::load(&app_config_file);
            let log_dir = match applog::init(app_cfg.logs, app_config_file.clone()) {
                Ok(dir) => Some(dir),
                Err(e) => {
                    // Hub 初始化失败降级为无日志运行（emit 全静默），不阻断应用启动
                    eprintln!("[FATAL] applog::init 失败: {e}");
                    None
                }
            };

            // 启动埋点序列（§4.1 时序约束）：
            // boot.start → boot.config_loaded → boot.log_dir_ready → log.cleanup（可选）
            // → boot.keychain_ready → boot.ready
            applog::emit(
                log::Level::Info,
                "app",
                ev::APP_BOOT_START,
                None,
                "Rhost 启动中",
                Some(serde_json::json!({
                    "version": env!("CARGO_PKG_VERSION"),
                    "os": std::env::consts::OS,
                    "arch": std::env::consts::ARCH,
                })),
            );

            // 记录后端配置文件加载完成及其路径（kv 平铺，避免嵌套对象在面板
            // 显示为 [object Object]；生效中的具体配置可用 jq 查 app_config.json）
            applog::emit(
                log::Level::Info,
                "app",
                ev::APP_BOOT_CONFIG_LOADED,
                None,
                "配置文件已加载",
                Some(serde_json::json!({
                    "elapsed_ms": BOOT_INSTANT.get().map(|t| t.elapsed().as_millis() as u64).unwrap_or(0),
                    "path": app_config_file.display().to_string(),
                })),
            );

            if let Some(dir) = &log_dir {
                applog::emit(
                    log::Level::Info,
                    "app",
                    ev::APP_BOOT_LOG_DIR_READY,
                    None,
                    "日志目录已就绪",
                    Some(serde_json::json!({ "path": dir.to_string_lossy() })),
                );
                // 启动时过期日志清理（§6.4）
                if let Some(hub) = applog::try_hub() {
                    hub.cleanup_logs();
                }
            }

            applog::emit(
                log::Level::Info,
                "app",
                ev::APP_BOOT_KEYCHAIN_READY,
                None,
                "密钥存储已连接",
                Some(serde_json::json!({
                    "backend": if cfg!(target_os = "macos") {
                        "macOS Keychain"
                    } else if cfg!(target_os = "windows") {
                        "Windows Credential Manager"
                    } else {
                        "libsecret"
                    }
                })),
            );

            applog::emit(
                log::Level::Info,
                "app",
                ev::APP_BOOT_READY,
                None,
                "Rhost 启动完成",
                Some(serde_json::json!({
                    "elapsed_ms": BOOT_INSTANT.get().map(|t| t.elapsed().as_millis() as u64).unwrap_or(0),
                })),
            );

            Ok(())
        })
        // 会话池注入全局状态，供命令按 session_id 查找
        .manage(SessionManager::new())
        .invoke_handler(tauri::generate_handler![
            ipc::connect_ssh,
            ipc::write_terminal,
            ipc::resize_terminal,
            ipc::disconnect_session,
            ipc::sftp_list_dir,
            ipc::sftp_mkdir,
            ipc::sftp_sync_cwd,
            ipc::sftp_upload,
            ipc::sftp_download,
            ipc::sftp_transfer_pause,
            ipc::sftp_transfer_cancel,
            ipc::sftp_remove,
            ipc::sftp_rename,
            ipc::sftp_create_file,
            ipc::sftp_copy,
            ipc::sftp_readlink,
            ipc::start_metrics,
            ipc::stop_metrics,
            ipc::metrics_heartbeat,
            ipc::test_ssh_connection,
            ipc::load_connections,
            ipc::save_connection,
            ipc::delete_connection,
            ipc::save_connection_password,
            ipc::get_connection_password,
            ipc::get_app_memory,
            ipc::subscribe_app_logs,
            ipc::report_app_log,
            ipc::clear_app_log_buffer,
            ipc::reveal_log_dir,
            ipc::reveal_log_storage_dir,
            ipc::set_log_config,
            fonts::check_fonts,
            localfs::list_local_dir,
            localfs::mkdir,
            localfs::local_remove,
            localfs::local_rename,
            localfs::local_create_file,
            localfs::local_copy,
            localfs::local_open_terminal,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // 退出序列（§4.1/§6.2）：shutdown.start → sessions_closed →
            // log_flushed → app.exit（最后一条）；随后 Hub 关闭并 join 落盘线程，
            // 保证上述 4 条完整落盘、无半截行。
            if let RunEvent::ExitRequested { .. } = event {
                let shutdown_instant = std::time::Instant::now();

                let manager = app_handle.state::<SessionManager>();
                let active = tauri::async_runtime::block_on(manager.session_count());
                applog::emit(
                    log::Level::Info,
                    "app",
                    ev::APP_SHUTDOWN_START,
                    None,
                    "正在退出",
                    Some(serde_json::json!({ "active_sessions": active })),
                );

                let closed = tauri::async_runtime::block_on(manager.shutdown_all());
                applog::emit(
                    log::Level::Info,
                    "app",
                    ev::APP_SHUTDOWN_SESSIONS_CLOSED,
                    None,
                    "会话已全部关闭",
                    Some(serde_json::json!({
                        "count": closed,
                        "elapsed_ms": shutdown_instant.elapsed().as_millis() as u64,
                    })),
                );

                applog::emit(
                    log::Level::Info,
                    "app",
                    ev::APP_SHUTDOWN_LOG_FLUSHED,
                    None,
                    "日志缓冲区已落盘",
                    Some(serde_json::json!({
                        "pending": applog::try_hub().map(|h| h.pending_count()).unwrap_or(0),
                    })),
                );

                applog::emit(
                    log::Level::Info,
                    "app",
                    ev::APP_EXIT,
                    None,
                    "Rhost 退出",
                    Some(serde_json::json!({
                        "uptime_s": BOOT_INSTANT.get().map(|t| t.elapsed().as_secs()).unwrap_or(0),
                        "active_sessions": 0,
                    })),
                );

                if let Some(hub) = applog::try_hub() {
                    hub.shutdown();
                }
            }
        });
}
