mod fonts;
mod ipc;
mod localfs;
mod metrics;
mod motd;
pub mod ssh;
mod store;
mod sysmon;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // 终端内 http(s) 链接点击后交系统默认浏览器打开（WebView 内 window.open 不可靠）
        .plugin(tauri_plugin_opener::init())
        // 终端复制/粘贴的剪贴板读写（WKWebView 对 Web Clipboard API 读取有限制）
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Debug)
                        .build(),
                )?;
            }
            Ok(())
        })
        // 会话池注入全局状态，供命令按 session_id 查找
        .manage(ssh::SessionManager::new())
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
            fonts::check_fonts,
            localfs::list_local_dir,
            localfs::mkdir,
            localfs::local_remove,
            localfs::local_rename,
            localfs::local_create_file,
            localfs::local_copy,
            localfs::local_open_terminal,
        ])
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}
