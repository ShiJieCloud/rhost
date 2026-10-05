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
    };

    let start = Instant::now();
    // 测试连接不需要协商算法出口，给个一次性空槽即可（kex_done 仍会写入）
    let algo = std::sync::Arc::new(std::sync::Mutex::new(None));
    let handle = tokio::time::timeout(
        Duration::from_secs(10),
        connect_and_auth(&cfg, algo),
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
    store::load_hosts(&app)
}

/// 保存单条主机配置（新增或更新，按 id 去重）。密码不入 JSON，
/// 需前端在保存后另行调用 `save_connection_password`。
#[tauri::command]
pub async fn save_connection(app: AppHandle, host: StoredHost) -> Result<(), String> {
    store::upsert_host(&app, host)
}

/// 删除主机配置，同时清理钥匙串中对应的密码条目。
#[tauri::command]
pub async fn delete_connection(app: AppHandle, id: String) -> Result<(), String> {
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
    store::save_password(&app, &id, &password)
}

/// 从系统钥匙串读取指定主机的密码（失败时从加密文件读取）；未设置时返回 None。
#[tauri::command]
pub async fn get_connection_password(app: AppHandle, id: String) -> Result<Option<String>, String> {
    store::get_password(&app, &id)
}

/// 状态栏：终端工具自身内存占用（Rhost 主进程 RSS / 系统总内存，字节）
#[tauri::command]
pub fn get_app_memory() -> crate::sysmon::AppMemory {
    crate::sysmon::app_memory()
}
