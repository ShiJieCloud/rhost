mod applog;
mod config_crypto;
mod config_io;
mod config_migrate;
mod fonts;
mod ipc;
mod known_hosts;
mod localfs;
mod metrics;
mod motd;
pub mod ssh;
mod store;
mod sysmon;

use applog::events as ev;
use ssh::manager::SessionManager;
use tauri::{Manager, RunEvent, WebviewUrl, webview::WebviewWindowBuilder};

/// 应用启动时刻（退出时计算 uptime_s）
static BOOT_INSTANT: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// 构造 prevent-default 插件：关闭 WebView 原生右键菜单（Reload/Inspect Element 等），
/// 防止其叠加在自定义 ContextMenu 之上；同时禁用浏览器默认快捷键（F3/Ctrl+J 等）。
/// - dev 构建：保留 DEV_TOOLS（F12）与 RELOAD（Ctrl+R）便于调试
/// - release 构建：禁用全部默认快捷键
#[cfg(debug_assertions)]
fn prevent_default_plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    use tauri_plugin_prevent_default::Flags;
    tauri_plugin_prevent_default::Builder::new()
        .with_flags(Flags::all().difference(Flags::DEV_TOOLS | Flags::RELOAD))
        .build()
}
#[cfg(not(debug_assertions))]
fn prevent_default_plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri_plugin_prevent_default::init()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = BOOT_INSTANT.set(std::time::Instant::now());

    tauri::Builder::default()
        // 终端内 http(s) 链接点击后交系统默认浏览器打开（WebView 内 window.open 不可靠）
        .plugin(tauri_plugin_opener::init())
        // 终端复制/粘贴的剪贴板读写（WKWebView 对 Web Clipboard API 读取有限制）
        .plugin(tauri_plugin_clipboard_manager::init())
        // 配置导入导出：系统打开/保存文件对话框
        .plugin(tauri_plugin_dialog::init())
        // 配置导入后提示「立即重启」：relaunch()
        .plugin(tauri_plugin_process::init())
        // 关闭 WebView 原生右键菜单与浏览器默认快捷键（详见 prevent_default_plugin）
        .plugin(prevent_default_plugin())
        // 全局配置写锁：串行化 app_config.json + connections.json 的所有写操作
        // （节写穿 / 连接增删 / 导入 / 重置），必须在 setup 与命令注册前挂载
        .manage(applog::persisted::ConfigWriteLock::new())
        .setup(|app| {
            // 后端整体配置文件（前端 localStorage 已下线，此文件为唯一真相源）：
            // logStoragePath 重启生效的唯一载体，同时为整体配置导入导出与首帧注入提供数据；
            // 缺失/损坏时 load_full 回退默认值
            let app_config_file = app
                .path()
                .app_config_dir()
                .map(|d| d.join(applog::persisted::APP_CONFIG_FILE_NAME))
                .unwrap_or_else(|_| {
                    std::env::temp_dir()
                        .join("com.rhost.app")
                        .join(applog::persisted::APP_CONFIG_FILE_NAME)
                });
            let app_cfg = applog::persisted::load_full(&app_config_file);

            // 首帧注入：splash 背景模式必须在页面任何脚本执行前同步可知（异步 IPC 会导致
            // 透明/深色底闪烁）。窗口改由 Rust 创建（tauri.conf.json windows 留空），经
            // initialization_script 写入 window.__RHOST_BOOT__；settings 节缺失/非法回退默认透明
            let splash_transparent = app_cfg
                .settings
                .get("splashTransparent")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let boot_script = format!(
                "window.__RHOST_BOOT__={};",
                serde_json::json!({
                    "splashTransparent": splash_transparent,
                    // 同步注入运行平台（前端据此切换标题栏安全区与窗口控件渲染），
                    // 取 std::env::consts::OS：macos / windows / linux
                    "platform": std::env::consts::OS,
                })
            );
            // 属性与原静态窗口定义保持一致；label 固定 "main"（capabilities/default.json 按此授权）
            // 跨平台标题栏策略：
            // - macOS：decorations(true) + Overlay 标题栏样式 + hidden_title，原生红绿灯浮于
            //   前端标题栏左侧；hidden_title 隐藏系统标题文字，避免与前端 logo 的 "Rhost"
            //   重影。需 macOSPrivateApi(true)（tauri.conf.json）+ transparent 才能让标题栏
            //   区域与内容融合（backdrop-filter 毛玻璃生效）。前端预留左侧安全区。
            // - Windows/Linux：decorations(false) 无边框，由前端在标题栏右侧自绘窗口控制按钮。
            //   title_bar_style / hidden_title / traffic_light_position 均为 macOS 专属 API，
            //   须 #[cfg] 隔离以免他平台编译失败。
            // macOS 专属 cfg 块内会对 builder 重新赋值，仅该平台需要 mut；
            // 非 macOS 平台不重赋值，须压制 unused_mut 警告
            #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
            let mut window_builder = WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::App("index.html".into()),
            )
            .title("Rhost")
            .inner_size(1200.0, 760.0)
            .min_inner_size(940.0, 600.0)
            .resizable(true)
            .decorations(cfg!(target_os = "macos"))
            .transparent(cfg!(target_os = "macos"))
            .initialization_script(&boot_script);

            #[cfg(target_os = "macos")]
            {
                window_builder = window_builder
                    .title_bar_style(tauri::TitleBarStyle::Overlay)
                    .hidden_title(true)
                    // 红绿灯位置：x 左边距，y 顶部偏移（逻辑像素）；y=0 紧贴顶部，越大越往下
                    .traffic_light_position(tauri::LogicalPosition::new(12.0, 24.0));
            }

            window_builder.build()?;

            // Hub 在窗口创建之后初始化：建窗不产生 applog，boot.start 仍是全局第一条日志
            let log_dir = match applog::init(app_cfg.logs_config(), app_config_file.clone()) {
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
            ipc::tunnel_start,
            ipc::tunnel_stop,
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
            ipc::load_app_config,
            ipc::set_app_config_section,
            ipc::export_config,
            ipc::read_import_file,
            ipc::import_config,
            ipc::import_hosts,
            ipc::reset_settings_config,
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
