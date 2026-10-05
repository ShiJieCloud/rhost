//! IPC 适配层：仅做参数校验与转发，不含 SSH 业务逻辑。
//!
//! 终端二进制流经 `Channel<Vec<u8>>` 直推前端（不走 Emitter/JSON）；
//! 低频控制消息（连接结果、错误文本）走 invoke 返回值。

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, State};

use crate::ssh::sftp::TransferProgress;
use crate::ssh::session::connect_and_auth;
use crate::ssh::{AuthMethod, SessionConfig, SessionManager};
use crate::store::{self, StoredHost};

use russh::Disconnect;
use std::time::{Duration, Instant};

/* =========================================================
 *  慢 IPC 调用统计（§4.4：ipc.slow_call，阈值 2000ms，只告警不阻断）
 * ========================================================= */

/// 慢调用阈值：超过即产出 `ipc.slow_call` WARN
const SLOW_CALL_THRESHOLD: Duration = Duration::from_millis(2000);

/// 命令计时守卫：创建即开始计时，命令返回（成功/失败）drop 时检查耗时。
///
/// RAII 方式对同步与 async 命令通用：async fn 中守卫随 future 存活，
/// await 挂起期间计时继续，统计的是调用方感知的墙钟总时长（含排队等待）。
#[allow(dead_code)] // 经宏构造，部分平台/配置下字段仅在 Drop 中读取
pub(crate) struct SlowCallGuard {
    cmd: &'static str,
    start: Instant,
}

impl SlowCallGuard {
    pub(crate) fn new(cmd: &'static str) -> Self {
        Self {
            cmd,
            start: Instant::now(),
        }
    }
}

impl Drop for SlowCallGuard {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed();
        if elapsed >= SLOW_CALL_THRESHOLD {
            crate::applog::emit(
                log::Level::Warn,
                "ipc",
                crate::applog::events::IPC_SLOW_CALL,
                None,
                "IPC 调用耗时过长",
                Some(serde_json::json!({
                    "cmd": self.cmd,
                    "elapsed_ms": elapsed.as_millis() as u64,
                })),
            );
        }
    }
}

/// 命令体首行插入：`slow_span!("cmd_name");`
/// 守卫绑定到当前作用域，函数返回时自动结算。
/// `#[macro_export]`：供 fonts / localfs 等其他命令模块复用。
#[macro_export]
macro_rules! slow_span {
    ($cmd:literal) => {
        let _slow_call_guard = $crate::ipc::SlowCallGuard::new($cmd);
    };
}


/// 前端新建会话时提交的连接参数
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectPayload {
    pub host: String,
    pub port: u16,
    pub username: String,
    /// 密码认证凭据（与 keyPath 二选一）
    #[serde(default)]
    pub password: Option<String>,
    /// 公钥认证：私钥路径（提供时忽略 password）
    #[serde(default)]
    pub key_path: Option<String>,
    /// 公钥认证：私钥口令（无口令可缺省）
    #[serde(default)]
    pub passphrase: Option<String>,
    /// PTY 初始列数
    pub cols: u32,
    /// PTY 初始行数
    pub rows: u32,
    /// 连接成功后绘制 Rhost MOTD 欢迎面板（缺省开启）；
    /// 开启时同时抑制 sshd 原生 MOTD / Last login，避免双重横幅
    #[serde(default = "default_true")]
    pub motd: bool,
    /// 彩色提示符注入（缺省开启；关闭时不落盘、不注入、不 hold 初始化输出）
    #[serde(default = "default_true")]
    pub color_prompt: bool,
    /// 登录后经 init 脚本 export 注入远端 shell 的环境变量（非法键值由后端过滤）
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// 自定义 MOTD ASCII LOGO（多行文本）；空串使用内置 LOGO
    #[serde(default)]
    pub motd_logo: String,
}

/// serde 默认值：布尔开关缺省为 true
fn default_true() -> bool {
    true
}

impl ConnectPayload {
    /// keyPath 优先 → 公钥认证；否则密码认证（密码缺省为空串）
    fn into_auth(&self) -> AuthMethod {
        match &self.key_path {
            Some(path) if !path.is_empty() => AuthMethod::Key {
                path: path.clone(),
                passphrase: self.passphrase.clone().unwrap_or_default(),
            },
            _ => AuthMethod::Password(self.password.clone().unwrap_or_default()),
        }
    }
}

/// 连接成功返回：会话 ID（提示符注入由后端在 PTY 开启后自动完成，无需前端参与）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectResult {
    pub session_id: String,
}

/// 建立 SSH PTY 会话。
///
/// 每个会话绑定独立的 `channel`（数据流隔离，不全局广播）；
/// 后台 spawn 一个转发任务，把该会话的帧流推入 channel，
/// channel 被前端释放（窗口关闭/导航离开）时 send 失败，转发任务自动退出。
#[tauri::command]
pub async fn connect_ssh(
    payload: ConnectPayload,
    channel: Channel<Vec<u8>>,
    manager: State<'_, SessionManager>,
) -> Result<ConnectResult, String> {
    slow_span!("connect_ssh");
    // 先记录目标地址并取出认证方式（下面 payload 字段被 move 进 cfg）
    let target = format!("{}:{}", payload.host, payload.port);
    let auth = payload.into_auth();
    let motd = payload.motd;
    let color_prompt = payload.color_prompt;
    let cfg = SessionConfig {
        host: payload.host,
        port: payload.port,
        username: payload.username,
        auth,
        cols: payload.cols,
        rows: payload.rows,
        motd,
        color_prompt,
        env: payload.env,
        motd_logo: payload.motd_logo,
    };
    let (session_id, mut frame_rx) = manager.create(cfg).await.map_err(|e| {
        // 落盘日志（带目标地址），方便在 ~/Library/Logs/com.rhost.app/rhost.log 排查
        log::error!("connect_ssh 失败 [{target}]: {e}");
        format!("{e}（目标 {target}）")
    })?;

    tauri::async_runtime::spawn(async move {
        while let Some(frame) = frame_rx.recv().await {
            if channel.send(frame).is_err() {
                // 前端通道已释放，结束转发（会话本体由 disconnect_session 关闭）
                break;
            }
        }
    });

    Ok(ConnectResult { session_id })
}

/// 写入前端键盘原始字节到 PTY
#[tauri::command]
pub async fn write_terminal(
    session_id: &str,
    data: Vec<u8>,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    slow_span!("write_terminal");
    manager.write(session_id, data).await.map_err(|e| {
        log::error!("write_terminal 失败 (session={session_id}): {e}");
        e.to_string()
    })
}

/// 同步 PTY 窗口尺寸：xterm fit 得到新行列后调用，远端 vim/top 随即重绘
#[tauri::command]
pub async fn resize_terminal(
    session_id: &str,
    cols: u32,
    rows: u32,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    slow_span!("resize_terminal");
    manager
        .resize(session_id, cols, rows)
        .await
        .map_err(|e| e.to_string())
}

/// 断开会话：取消令牌触发全部后台任务退出，并从会话池移除
#[tauri::command]
pub async fn disconnect_session(
    session_id: &str,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    slow_span!("disconnect_session");
    manager.disconnect(session_id).await;
    Ok(())
}

/// SFTP 列举远端目录。`path` 缺省时由后端 canonicalize(".") 解析为登录默认目录。
/// SFTP 子系统通道在会话内懒加载并缓存，断流自动重建重试一次。
#[tauri::command]
pub async fn sftp_list_dir(
    session_id: &str,
    path: Option<String>,
    manager: State<'_, SessionManager>,
) -> Result<crate::ssh::sftp::RemoteDirListing, String> {
    slow_span!("sftp_list_dir");
    manager
        .sftp_list_dir(session_id, path)
        .await
        .map_err(|e| {
            log::error!("sftp_list_dir 失败 (session={session_id}): {e}");
            e.to_string()
        })
}

/// SFTP 创建目录（单层）。
#[tauri::command]
pub async fn sftp_mkdir(
    session_id: &str,
    path: String,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    slow_span!("sftp_mkdir");
    manager
        .sftp_mkdir(session_id, path)
        .await
        .map_err(|e| {
            log::error!("sftp_mkdir 失败 (session={session_id}): {e}");
            e.to_string()
        })
}

/// 上传单个文件。同名远端文件默认覆盖；进度经 IPC [`Channel`] 流式推送。
#[tauri::command]
pub async fn sftp_upload(
    session_id: &str,
    task_id: u64,
    local_path: String,
    remote_path: String,
    chunk_kb: Option<u32>,
    resume: Option<bool>,
    channel: Channel<TransferProgress>,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    // 传输命令豁免慢调用统计：大文件传输天然长耗时，进度由 Channel 流式推送，
    // invoke 总时长不反映卡顿（且用户主动暂停期间 invoke 挂起会误报）。
    manager
        .sftp_upload(session_id, task_id, local_path, remote_path, chunk_kb, resume, channel)
        .await
        .map_err(|e| {
            log::error!("sftp_upload 失败 (session={session_id}, task={task_id}): {e}");
            e.to_string()
        })
}

/// 下载单个文件。本地父目录不存在时自动创建；进度经 IPC [`Channel`] 流式推送。
#[tauri::command]
pub async fn sftp_download(
    session_id: &str,
    task_id: u64,
    remote_path: String,
    local_path: String,
    chunk_kb: Option<u32>,
    resume: Option<bool>,
    channel: Channel<TransferProgress>,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    // 同 sftp_upload：豁免慢调用统计
    manager
        .sftp_download(session_id, task_id, remote_path, local_path, chunk_kb, resume, channel)
        .await
        .map_err(|e| {
            log::error!("sftp_download 失败 (session={session_id}, task={task_id}): {e}");
            e.to_string()
        })
}

/// SFTP → Shell 方向同步：让远端交互式 shell `cd` 到 `path`（路径由后端做 shell 转义）。
/// 用户在 SFTP 文件树切换目录时调用，使终端提示符与文件树保持同一目录。
#[tauri::command]
pub async fn sftp_sync_cwd(
    session_id: &str,
    path: String,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    slow_span!("sftp_sync_cwd");
    manager
        .sftp_sync_cwd(session_id, path)
        .await
        .map_err(|e| e.to_string())
}

/// 暂停（paused=true）或继续（paused=false）指定传输任务
#[tauri::command]
pub async fn sftp_transfer_pause(
    session_id: &str,
    task_id: u64,
    paused: bool,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    slow_span!("sftp_transfer_pause");
    manager
        .sftp_transfer_pause(session_id, task_id, paused)
        .await
        .map_err(|e| e.to_string())
}

/// 取消指定传输任务
#[tauri::command]
pub async fn sftp_transfer_cancel(
    session_id: &str,
    task_id: u64,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    slow_span!("sftp_transfer_cancel");
    manager
        .sftp_transfer_cancel(session_id, task_id)
        .await
        .map_err(|e| e.to_string())
}

/// 删除远端文件/目录（目录递归；软链只删链接本身）
#[tauri::command]
pub async fn sftp_remove(
    session_id: &str,
    path: String,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    slow_span!("sftp_remove");
    manager
        .sftp_remove(session_id, path)
        .await
        .map_err(|e| {
            log::error!("sftp_remove 失败 (session={session_id}): {e}");
            e.to_string()
        })
}

/// 重命名/移动远端路径
#[tauri::command]
pub async fn sftp_rename(
    session_id: &str,
    old_path: String,
    new_path: String,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    slow_span!("sftp_rename");
    manager
        .sftp_rename(session_id, old_path, new_path)
        .await
        .map_err(|e| {
            log::error!("sftp_rename 失败 (session={session_id}): {e}");
            e.to_string()
        })
}

/// 创建远端零字节空文件
#[tauri::command]
pub async fn sftp_create_file(
    session_id: &str,
    path: String,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    slow_span!("sftp_create_file");
    manager
        .sftp_create_file(session_id, path)
        .await
        .map_err(|e| {
            log::error!("sftp_create_file 失败 (session={session_id}): {e}");
            e.to_string()
        })
}

/// 同主机远端复制（目录递归；软链复制为同指向软链）
#[tauri::command]
pub async fn sftp_copy(
    session_id: &str,
    src: String,
    dst: String,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    slow_span!("sftp_copy");
    manager
        .sftp_copy(session_id, src, dst)
        .await
        .map_err(|e| {
            log::error!("sftp_copy 失败 (session={session_id}): {e}");
            e.to_string()
        })
}

/// 读取远端符号链接目标路径
#[tauri::command]
pub async fn sftp_readlink(
    session_id: &str,
    path: String,
    manager: State<'_, SessionManager>,
) -> Result<String, String> {
    slow_span!("sftp_readlink");
    manager
        .sftp_readlink(session_id, path)
        .await
        .map_err(|e| e.to_string())
}

/// 默认动态指标采集间隔（毫秒）；Step 8 起由设置项覆盖
const DEFAULT_METRICS_INTERVAL_MS: u64 = 3000;

/// 启动右侧栏动态指标采集（0x06 帧）。前端在 Inspector 可见且会话在线时调用，
/// 之后每 5s 调一次 `metrics_heartbeat` 续约；隐藏/关闭后 9s 无心跳任务自停。
/// `iface` 为 0x05 已采集的默认路由网卡名，网络计数优先取它；缺省汇总非 lo 网卡。
#[tauri::command]
pub async fn start_metrics(
    session_id: &str,
    interval_ms: Option<u64>,
    iface: Option<String>,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    slow_span!("start_metrics");
    let interval = interval_ms.unwrap_or(DEFAULT_METRICS_INTERVAL_MS);
    manager
        .start_metrics(session_id, interval, iface)
        .await
        .map_err(|e| e.to_string())
}

/// 显式停止动态指标采集（切换会话/Inspector 隐藏时调用，即时释放 exec 通道）
#[tauri::command]
pub async fn stop_metrics(
    session_id: &str,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    slow_span!("stop_metrics");
    manager
        .stop_metrics(session_id)
        .await
        .map_err(|e| e.to_string())
}

/// 指标采集心跳：维持任务存活，前端约每 5s 一次
#[tauri::command]
pub async fn metrics_heartbeat(
    session_id: &str,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    slow_span!("metrics_heartbeat");
    manager
        .metrics_heartbeat(session_id)
        .await
        .map_err(|e| e.to_string())
}

/// 测试连接返回：成功时附带 TCP→认证全链路耗时
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub latency_ms: u64,
}

/// 测试 SSH 连接：仅做 TCP 握手 + 认证（密码/公钥），成功即断，不申请 PTY、不注册会话池。
///
/// 与正式连接共用 [`connect_and_auth`]，保证认证行为完全一致；用 10s 超时包裹，
/// 避免不可达主机让用户久等。错误直接转字符串返回，前端可区分"连不上/认证错/超时"，
/// 私钥加密时错误以 KEY_ENCRYPTED 前缀返回，前端据此弹口令框重试。
#[tauri::command]
pub async fn test_ssh_connection(payload: ConnectPayload) -> Result<TestResult, String> {
    slow_span!("test_ssh_connection");
    let auth = payload.into_auth();
    let cfg = SessionConfig {
        host: payload.host,
        port: payload.port,
        username: payload.username,
        auth,
        cols: 0,
        rows: 0,
        // 测试连接只验证握手与认证，不绘制 MOTD、不注入提示符/环境变量
        motd: false,
        color_prompt: false,
        env: Vec::new(),
        motd_logo: String::new(),
    };

    let start = Instant::now();
    // 测试连接不需要协商算法出口，给个一次性空槽即可（kex_done 仍会写入）；
    // sid 用固定值便于排障时过滤测试连接日志
    let algo = std::sync::Arc::new(std::sync::Mutex::new(None));
    let handle = tokio::time::timeout(
        Duration::from_secs(10),
        connect_and_auth(&cfg, algo, "test"),
    )
    .await
    .map_err(|_| "连接超时（10 秒内未响应）".to_string())?
    .map_err(|e| e.to_string())?;

    let latency_ms = start.elapsed().as_millis() as u64;
    // 立即断开，不注册会话池、不留后台任务
    let _ = handle
        .disconnect(Disconnect::ByApplication, "test connection ok", "")
        .await;
    Ok(TestResult { latency_ms })
}

/* =========================================================
 *  主机配置持久化（JSON 文件 + 系统钥匙串）
 * ========================================================= */

/// 加载全部主机配置（不含密码，密码需单独调用 `get_connection_password`）。
#[tauri::command]
pub async fn load_connections(app: AppHandle) -> Result<Vec<StoredHost>, String> {
    slow_span!("load_connections");
    store::load_hosts(&app)
}

/// 保存单条主机配置（新增或更新，按 id 去重）。密码不入 JSON，
/// 需前端在保存后另行调用 `save_connection_password`。
#[tauri::command]
pub async fn save_connection(app: AppHandle, host: StoredHost) -> Result<(), String> {
    slow_span!("save_connection");
    store::upsert_host(&app, host)
}

/// 删除主机配置，同时清理钥匙串中对应的密码条目。
#[tauri::command]
pub async fn delete_connection(app: AppHandle, id: String) -> Result<(), String> {
    slow_span!("delete_connection");
    store::delete_host(&app, &id)?;
    store::delete_password(&app, &id)
}

/// 保存密码到系统钥匙串（失败时 fallback 到加密文件）。
#[tauri::command]
pub async fn save_connection_password(
    app: AppHandle,
    id: String,
    password: String,
) -> Result<(), String> {
    slow_span!("save_connection_password");
    store::save_password(&app, &id, &password)
}

/// 从系统钥匙串读取指定主机的密码（失败时从加密文件读取）；未设置时返回 None。
#[tauri::command]
pub async fn get_connection_password(app: AppHandle, id: String) -> Result<Option<String>, String> {
    slow_span!("get_connection_password");
    store::get_password(&app, &id)
}

/// 状态栏：终端工具自身内存占用（Rhost 主进程 RSS / 系统总内存，字节）
#[tauri::command]
pub fn get_app_memory() -> crate::sysmon::AppMemory {
    slow_span!("get_app_memory");
    crate::sysmon::app_memory()
}

/* =========================================================
 *  应用日志（App Log）：订阅 / 上报 / 缓冲管理 / 目录 / 配置
 * ========================================================= */

use crate::applog::{self, HubConfig, LogBatch, LogConfigPayload, ReportAppLogInput};

/// 订阅应用日志：先推 replay（seq > sinceId；越界推全量并附 lost_because 元信息），
/// 再持续推增量。多订阅者广播，前端释放 Channel 后后端 send 失败自动摘除订阅。
#[tauri::command]
pub fn subscribe_app_logs(
    channel: Channel<LogBatch>,
    since_id: Option<u64>,
) -> Result<(), String> {
    slow_span!("subscribe_app_logs");
    let Some(hub) = applog::try_hub() else {
        return Err("日志系统未初始化".to_string());
    };
    hub.subscribe(channel, since_id);
    Ok(())
}

/// 前端日志上报入口：白名单校验（仅 level/eventId/msg/kv，web.* 域）
#[tauri::command]
pub fn report_app_log(input: ReportAppLogInput) -> Result<(), String> {
    slow_span!("report_app_log");
    if let Some(hub) = applog::try_hub() {
        hub.report(input);
    }
    Ok(())
}

/// 清空内存缓冲（面板「清空日志」按钮；只清内存，不删磁盘文件）
#[tauri::command]
pub fn clear_app_log_buffer() -> Result<(), String> {
    slow_span!("clear_app_log_buffer");
    if let Some(hub) = applog::try_hub() {
        hub.clear_buffer();
    }
    Ok(())
}

/// 系统文件管理器打开日志目录（当前运行实例实际写入目录）
#[tauri::command]
pub fn reveal_log_dir() -> Result<(), String> {
    slow_span!("reveal_log_dir");
    let Some(hub) = applog::try_hub() else {
        return Err("日志系统未初始化".to_string());
    };
    tauri_plugin_opener::open_path(hub.log_dir(), None::<&str>).map_err(|e| e.to_string())
}

/// 系统文件管理器打开设置面板中配置的日志存储路径，返回实际打开的绝对路径。
///
/// 与 `reveal_log_dir` 的区别：设置面板允许查看「已填写但尚未重启生效」的目标目录。
/// 处理：空串回退当前生效目录；展开 `~`；目录不存在则创建（重启后 Hub 同样会建，
/// 此处提前建好便于用户确认落点）；路径指向普通文件等无法创建的情况返回错误。
#[tauri::command]
pub fn reveal_log_storage_dir(path: String) -> Result<String, String> {
    slow_span!("reveal_log_storage_dir");
    let Some(hub) = applog::try_hub() else {
        return Err("日志系统未初始化".to_string());
    };
    let dir = hub.resolve_storage_dir(&path);
    if !dir.is_absolute() {
        return Err(format!("请填写绝对路径或以 ~ 开头：{}", dir.display()));
    }

    let metadata = std::fs::metadata(&dir);
    match metadata {
        Ok(md) if md.is_dir() => {}
        Ok(_) => {
            return Err(format!("路径已存在但不是目录：{}", dir.display()));
        }
        Err(_) => {
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("创建目录失败 {}：{e}", dir.display()))?;
        }
    }

    tauri_plugin_opener::open_path(&dir, None::<&str>).map_err(|e| e.to_string())?;
    Ok(dir.display().to_string())
}

/// 热更新日志配置（前端 saveSettings 后调用）。
/// storage_path 仅重启生效（此处忽略，不做热切换）；日志相关键逐字段发 app.settings.change。
#[tauri::command]
pub fn set_log_config(config: LogConfigPayload) -> Result<(), String> {
    slow_span!("set_log_config");
    let Some(hub) = applog::try_hub() else {
        return Ok(()); // 日志系统未初始化时静默丢弃
    };
    let new_cfg = HubConfig::from_payload(&config);
    hub.set_config_with_audit(new_cfg);
    Ok(())
}
