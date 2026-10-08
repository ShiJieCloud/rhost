//! IPC 适配层：仅做参数校验与转发，不含 SSH 业务逻辑。
//!
//! 终端二进制流经 `Channel<Vec<u8>>` 直推前端（不走 Emitter/JSON）；
//! 低频控制消息（连接结果、错误文本）走 invoke 返回值。

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

use crate::ssh::session::connect_and_auth;
use crate::ssh::sftp::TransferProgress;
use crate::ssh::{AuthMethod, HostKeyCheck, HostKeyPolicy, SessionConfig, SessionManager};
use crate::store::{self, StoredHost};

use russh::Disconnect;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

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
    /// 主机密钥 TOFU 二阶段重试：用户已在 UI 确认信任（缺省 false）
    #[serde(default)]
    pub trust_host_key: bool,
    /// 用户确认的指纹（`SHA256:…`）；trust_host_key=true 时必须携带，
    /// 缺省由后端决策表按未确认处理（hostkey-verification-design.md §6.4 第 3 行）
    #[serde(default)]
    pub trust_fingerprint: Option<String>,
    /// 信任后是否写入 known_hosts：「仅本次连接」= false；
    /// 缺省 true 保持既有「接受并保存」落盘行为（设计 §6.5）
    #[serde(default = "default_true")]
    pub trust_host_key_persist: bool,
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

/// 校验 `trust_fingerprint`（不可信 IPC 输入，纵深防御 §6.3）：非空、`SHA256:`
/// 前缀、主体仅 Base64 字符集且长度 ≥ 40（SHA256 无 padding 为 43 字符、
/// 含 padding 44，均覆盖）。非法值在最外层拦截，绝不进入 `HostKeyCheck`。
/// 纯函数，单测见下方 `hostkey_validate_tests`。
fn validate_trust_fingerprint(fp: &str) -> Result<(), String> {
    let body = fp
        .strip_prefix("SHA256:")
        .ok_or_else(|| "信任指纹格式非法：缺少 SHA256: 前缀".to_string())?;
    if body.len() < 40 {
        return Err(format!("信任指纹格式非法：主体长度 {} 不足 40", body.len()));
    }
    if !body
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
    {
        return Err("信任指纹格式非法：主体含非 Base64 字符".to_string());
    }
    Ok(())
}

/// 两条连接命令统一的主机密钥策略构造：lookup → `HostKeyPolicy::Verify`。
/// 读失败按无记录处理（fail-closed，走首连确认）。
fn host_key_policy(app: &AppHandle, payload: &ConnectPayload) -> Result<HostKeyPolicy, String> {
    let path = crate::known_hosts::path(app)?;
    // trust_fingerprint 是不可信输入：格式非法直接返回参数错误
    let trust_fp = match payload.trust_fingerprint.as_deref() {
        Some(fp) => {
            validate_trust_fingerprint(fp)?;
            Some(fp.to_string())
        }
        None => None,
    };
    let stored = crate::known_hosts::lookup(&path, &payload.host, payload.port)
        .map(|e| (e.algo, e.fingerprint));
    Ok(HostKeyPolicy::Verify(HostKeyCheck {
        host: payload.host.clone(),
        port: payload.port,
        stored,
        trust: payload.trust_host_key,
        trust_fp,
        persist: payload.trust_host_key_persist,
        path,
    }))
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
    app: AppHandle,
    manager: State<'_, SessionManager>,
) -> Result<ConnectResult, String> {
    slow_span!("connect_ssh");
    // 主机密钥校验：读取/写入 known_hosts 需 AppHandle（仅 ipc 层触及，设计 §6.3）
    let host_key = host_key_policy(&app, &payload)?;
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
        host_key,
    };
    let (session_id, mut frame_rx) = manager.create(cfg).await.map_err(|e| {
        // 主机密钥协议错误（HOSTKEY_*:）原样透传：payload `algo|fp` 需被前端
        // 精确解析，追加「（目标 …）」后缀会破坏切分；其余错误照常附加目标地址
        let msg = e.to_string();
        if msg.starts_with("HOSTKEY_") {
            return msg;
        }
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

/* =========================================================
 *  端口转发（tunnel-design.md §6.7）
 * ========================================================= */

/// 端口转发规则入参（camelCase，与前端 types.ts `TunnelRule` 对齐；
/// name/enabled 为前端配置态字段，前端 invoke 前剥离，引擎不消费）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelRuleDto {
    pub id: String,
    /// 转发类型：`local` / `remote` / `dynamic`（serde 直接映射引擎枚举）
    #[serde(rename = "type")]
    pub kind: crate::ssh::tunnel::TunnelType,
    pub bind_host: String,
    pub bind_port: u16,
    #[serde(default)]
    pub target_host: Option<String>,
    #[serde(default)]
    pub target_port: Option<u16>,
}

/// 启动一条端口转发规则。
///
/// 错误即协议：返回字符串带前缀（前端据此分流，勿改文案）：
/// `TUNNEL_RUNNING:`（幂等成功）/ `TUNNEL_BAD_RULE:` / `TUNNEL_PORT_IN_USE:` /
/// `TUNNEL_REMOTE_DENIED:` / `TUNNEL_LIMIT:`。
#[tauri::command]
pub async fn tunnel_start(
    session_id: String,
    rule: TunnelRuleDto,
    port_conflict: String,
    dns_resolve: String,
    retry_count: u32,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    slow_span!("tunnel_start");
    let opts = crate::ssh::tunnel::StartOptions {
        port_conflict: crate::ssh::tunnel::PortConflict::parse(&port_conflict),
        dns_resolve: crate::ssh::tunnel::DnsResolve::parse(&dns_resolve),
        // 引擎约定 0–20，越界钳位（前端已是滑杆范围，此处兜底）
        retry_count: retry_count.min(20),
    };
    let rule = crate::ssh::tunnel::TunnelRule {
        id: rule.id,
        kind: rule.kind,
        bind_host: rule.bind_host,
        bind_port: rule.bind_port,
        target_host: rule.target_host,
        target_port: rule.target_port,
    };
    manager
        .tunnel_start(&session_id, rule, opts)
        .await
        .map_err(|e| e.to_string())
}

/// 停止一条端口转发规则（幂等：规则不存在视为成功）
#[tauri::command]
pub async fn tunnel_stop(
    session_id: String,
    rule_id: String,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    slow_span!("tunnel_stop");
    manager
        .tunnel_stop(&session_id, &rule_id)
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
    manager.sftp_list_dir(session_id, path).await.map_err(|e| {
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
    manager.sftp_mkdir(session_id, path).await.map_err(|e| {
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
        .sftp_upload(
            session_id,
            task_id,
            local_path,
            remote_path,
            chunk_kb,
            resume,
            channel,
        )
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
        .sftp_download(
            session_id,
            task_id,
            remote_path,
            local_path,
            chunk_kb,
            resume,
            channel,
        )
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
    manager.sftp_remove(session_id, path).await.map_err(|e| {
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
    manager.sftp_copy(session_id, src, dst).await.map_err(|e| {
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
/// 私钥加密时错误以 KEY_ENCRYPTED 前缀返回，前端据此弹口令框重试；
/// 主机密钥未信任/变更时以 HOSTKEY_UNKNOWN / HOSTKEY_MISMATCH 前缀返回（同样命中
/// check_server_key，QuickConnect 门禁的重试必须能构造 HostKeyCheck，设计 §6.3）。
#[tauri::command]
pub async fn test_ssh_connection(
    payload: ConnectPayload,
    app: AppHandle,
) -> Result<TestResult, String> {
    slow_span!("test_ssh_connection");
    let host_key = host_key_policy(&app, &payload)?;
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
        host_key,
    };

    let start = Instant::now();
    // 测试连接不需要协商算法出口，给个一次性空槽即可（kex_done 仍会写入）；
    // sid 用固定值便于排障时过滤测试连接日志。
    // -R 回调共享件给默认值：registry 为空表、闸门不参与，测试连接不做转发
    let algo = std::sync::Arc::new(std::sync::Mutex::new(None));
    let handle = tokio::time::timeout(
        Duration::from_secs(10),
        connect_and_auth(
            &cfg,
            algo,
            "test",
            Default::default(),
            std::sync::Arc::new(tokio::sync::Semaphore::new(
                crate::ssh::tunnel::MAX_CONN_GLOBAL,
            )),
        ),
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
pub async fn save_connection(
    app: AppHandle,
    state: State<'_, crate::applog::persisted::ConfigWriteLock>,
    host: StoredHost,
) -> Result<(), String> {
    slow_span!("save_connection");
    // 读—改—写 connections.json 必须在全局配置写锁内，避免与导入/删除交叉丢数据
    let _guard = state.0.lock().await;
    store::upsert_host(&app, host)
}

/// 删除主机配置，同时清理钥匙串中对应的密码条目。
#[tauri::command]
pub async fn delete_connection(
    app: AppHandle,
    state: State<'_, crate::applog::persisted::ConfigWriteLock>,
    id: String,
) -> Result<(), String> {
    slow_span!("delete_connection");
    {
        // 锁只保护 connections.json 读改写；keychain 操作不涉及配置文件，放锁外
        let _guard = state.0.lock().await;
        store::delete_host(&app, &id)?;
    }
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
pub fn subscribe_app_logs(channel: Channel<LogBatch>, since_id: Option<u64>) -> Result<(), String> {
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
///
/// 内存热更新（Hub）与落盘（app_config.json 的 logs 节）在同一把全局配置写锁内：
/// 与导入/重置互斥。写失败不回滚热更新（本次运行仍按新配置），仅 WARN 留痕。
#[tauri::command]
pub async fn set_log_config(
    state: State<'_, crate::applog::persisted::ConfigWriteLock>,
    config: LogConfigPayload,
) -> Result<(), String> {
    slow_span!("set_log_config");
    let Some(hub) = applog::try_hub() else {
        return Ok(()); // 日志系统未初始化时静默丢弃
    };
    let new_cfg = HubConfig::from_payload(&config);
    let cfg_for_disk = new_cfg.clone();
    let path = hub.app_config_file().to_path_buf();

    let _guard = state.0.lock().await;
    hub.set_config_with_audit(new_cfg);
    if let Err(e) = crate::applog::persisted::save_logs(&path, &cfg_for_disk) {
        crate::applog::emit(
            log::Level::Warn,
            "app",
            crate::applog::events::APP_LOG_CONFIG_PERSIST_FAILED,
            None,
            "日志配置持久化失败，重启后将回退",
            Some(serde_json::json!({
                "path": path.display().to_string(),
                "err": e.to_string(),
            })),
        );
    }
    Ok(())
}

/* =========================================================
 *  应用配置：统一快照读取 / 单节写穿（导入导出底座）
 * ========================================================= */

use crate::applog::persisted::{self as cfg_persisted, ConfigWriteLock};

/// app_config.json 完整路径（app config dir 下）
fn app_config_file(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|d| d.join(cfg_persisted::APP_CONFIG_FILE_NAME))
        .map_err(|e| format!("获取应用配置目录失败: {e}"))
}

/// 前端启动时一次性拉取的配置快照（后端文件为唯一真相源）。
/// 各节以原始 JSON 形态透传，前端按各自 store 解析；未知节/未知字段由后端保留。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfigSnapshot {
    /// logs 节（落盘形态，字段为 snake_case，与 persisted::LogsSection 序列化一致；
    /// 注意区别于 set_log_config 的入参 LogConfigPayload——后者是 camelCase）
    logs: serde_json::Value,
    settings: serde_json::Value,
    keys: serde_json::Value,
    groups: serde_json::Value,
    ui_state: serde_json::Value,
    quick_connect_history: serde_json::Value,
    /// 导入文件大小上限（后端权威常量，前端预检取此值）
    max_import_file_bytes: u64,
    /// 日志存储路径留空时的平台默认目录（前端输入框 placeholder）
    default_log_dir: String,
}

/// 读取应用配置全节快照（localStorage 下线后前端各 store 的唯一数据来源）。
#[tauri::command]
pub fn load_app_config(app: AppHandle) -> Result<AppConfigSnapshot, String> {
    slow_span!("load_app_config");
    let path = app_config_file(&app)?;
    let file = cfg_persisted::load_full(&path);
    Ok(AppConfigSnapshot {
        logs: file
            .section_value("logs")
            .unwrap_or_else(|| serde_json::json!({})),
        settings: file.settings,
        keys: file.keys,
        groups: file.groups,
        ui_state: file.ui_state,
        quick_connect_history: file.quick_connect_history,
        max_import_file_bytes: crate::config_io::MAX_IMPORT_FILE_BYTES,
        default_log_dir: crate::applog::hub::default_log_dir().display().to_string(),
    })
}

/// 通用单节写穿：白名单校验 + 强类型 schema 校验 + 原子写（300ms 防抖在前端）。
/// 非法节名 / 类型错误一律拒绝且零写入，并记 `app.config.write_rejected`。
#[tauri::command]
pub async fn set_app_config_section(
    app: AppHandle,
    state: State<'_, ConfigWriteLock>,
    section: String,
    value: serde_json::Value,
) -> Result<(), String> {
    slow_span!("set_app_config_section");
    let path = app_config_file(&app)?;
    let _guard = state.0.lock().await;
    cfg_persisted::save_section(&path, &section, value).map_err(|e| {
        crate::applog::emit(
            log::Level::Warn,
            "app",
            crate::applog::events::APP_CONFIG_WRITE_REJECTED,
            None,
            "配置节写入被拒绝",
            Some(serde_json::json!({"section": section, "reason": e})),
        );
        e
    })
}

/* =========================================================
 *  配置导出 / 备份（§7.1 / §7.6）
 * ========================================================= */

/// 导出配置到用户选定文件。后端自行读盘组装，明文 JSON 不经 IPC 返回前端。
///
/// - `scope`：full / hosts / ui（§4.1）；
/// - `include_ui` / `include_history` 仅 scope=full 时附加可选节；
/// - `password = Some(_)`：PBKDF2-SHA256 + AES-256-GCM 加密写 envelope（§5.1），
///   密码经 Zeroizing 包裹，函数结束即清零，不进任何日志。
/// 返回实际写入的绝对路径。
#[tauri::command]
pub async fn export_config(
    app: AppHandle,
    path: String,
    scope: String,
    include_ui: bool,
    include_history: bool,
    password: Option<String>,
) -> Result<String, String> {
    slow_span!("export_config");
    let started = Instant::now();
    let out_path = std::path::PathBuf::from(&path);
    // 红线：密码不落盘/不进日志；Zeroizing 在任意返回路径 drop 时清零
    let password = password.map(Zeroizing::new);
    let encrypted = password.is_some();

    let export_scope = crate::config_io::ExportScope::parse(&scope)
        .inspect_err(|e| emit_export_failed("assemble", e, started))?;

    // 读盘（原子 rename 保证不会读到半截文件；导出不需持写锁）
    let hosts = store::load_hosts(&app).inspect_err(|e| emit_export_failed("read", e, started))?;
    let cfg_path = app_config_file(&app)?;
    let cfg = cfg_persisted::load_full(&cfg_path);

    let now = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let assembled = crate::config_io::assemble_export(
        &hosts,
        &cfg,
        export_scope,
        include_ui,
        include_history,
        env!("CARGO_PKG_VERSION"),
        &now,
        &crate::config_io::hostname(),
    )
    .inspect_err(|e| emit_export_failed("assemble", e, started))?;

    // 加密分支（§5.1）：明文 Schema → PBKDF2/AES-GCM envelope；明文不经 IPC
    let output_json = match password.as_ref() {
        Some(pw) => crate::config_crypto::seal_envelope(&assembled.json, pw)
            .inspect_err(|e| emit_export_failed("encrypt", e, started))?,
        None => assembled.json.clone(),
    };
    // 导出产物必须能被导入端接受（envelope 5MB 预检）：base64 膨胀后超限则拒绝
    if output_json.len() as u64 > crate::config_io::MAX_IMPORT_FILE_BYTES {
        let msg = "配置数据过大，无法导出（超出文件大小上限），请缩小导出范围";
        emit_export_failed("assemble", msg, started);
        return Err(msg.to_string());
    }

    let bytes = crate::config_io::write_export_file(&out_path, &output_json)
        .inspect_err(|e| emit_export_failed("write", e, started))?;

    let abs_path = std::fs::canonicalize(&out_path)
        .unwrap_or(out_path)
        .display()
        .to_string();
    crate::applog::emit(
        log::Level::Info,
        "app",
        crate::applog::events::APP_CONFIG_EXPORT,
        None,
        "配置已导出",
        Some(serde_json::json!({
            "path": abs_path,
            "scope": export_scope.as_str(),
            "encrypted": encrypted,
            "sections": assembled.section_count,
            "bytes": bytes,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        })),
    );
    Ok(abs_path)
}

/// 导出失败统一埋点（kv 不含文件内容/密码，仅 stage 与错误信息、耗时）
fn emit_export_failed(stage: &str, err: &str, started: Instant) {
    crate::applog::emit(
        log::Level::Warn,
        "app",
        crate::applog::events::APP_CONFIG_EXPORT_FAILED,
        None,
        "配置导出失败",
        Some(serde_json::json!({
            "stage": stage,
            "error": err,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        })),
    );
}

/* =========================================================
 *  配置导入（§7.2 / §7.7 / §9）
 * ========================================================= */

/// `read_import_file` 返回：文件内容（前端预览/二次确认）+ 权威大小与文件名。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportFileContent {
    /// UTF-8 文件全文（上限 5MB；当前版本仅明文 JSON）
    text: String,
    bytes: u64,
    /// 仅文件名（日志/UI 展示用，不含路径）
    file_name: String,
}

/// 读取用户选定的导入文件：先 metadata 大小预检（超限不读内容），再按 UTF-8 读取。
#[tauri::command]
pub async fn read_import_file(path: String) -> Result<ImportFileContent, String> {
    slow_span!("read_import_file");
    let p = std::path::PathBuf::from(&path);
    let bytes = std::fs::metadata(&p)
        .map_err(|e| format!("读取文件信息失败: {e}"))?
        .len();
    if bytes > crate::config_io::MAX_IMPORT_FILE_BYTES {
        return Err(format!(
            "配置文件过大（{} 字节，上限 {} 字节），请确认文件来源",
            bytes,
            crate::config_io::MAX_IMPORT_FILE_BYTES
        ));
    }
    let text = std::fs::read_to_string(&p)
        .map_err(|e| format!("读取配置文件失败（仅支持 UTF-8 文本）: {e}"))?;
    let file_name = p
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    Ok(ImportFileContent {
        text,
        bytes,
        file_name,
    })
}

/// 导入失败统一埋点（kv 红线：仅 stage/section/reason/文件名/字节数/错误，不含内容与密码）
fn emit_import_failed(
    stage: &str,
    section: Option<&str>,
    reason: Option<&str>,
    err: &str,
    source: &str,
    bytes: u64,
) {
    let mut kv = serde_json::json!({
        "stage": stage,
        "error": err,
        "source": source,
        "bytes": bytes,
    });
    if let Some(sec) = section {
        kv["section"] = serde_json::Value::String(sec.to_string());
    }
    // decrypt 阶段细分：bad_password（密码错误）/ corrupted（文件损坏）
    if let Some(reason) = reason {
        kv["reason"] = serde_json::Value::String(reason.to_string());
    }
    crate::applog::emit(
        log::Level::Warn,
        "app",
        crate::applog::events::APP_CONFIG_IMPORT_FAILED,
        None,
        "配置导入失败",
        Some(kv),
    );
}

/// 导入事务内核：大小预检 → 持写锁读现状 → 纯函数解析合并 → 同临界区原子写两文件。
/// `hosts_only=true` 对应 §7.7 `import_hosts`（只消费 connections/groups）。
async fn run_import(
    app: &AppHandle,
    state: &State<'_, ConfigWriteLock>,
    payload: String,
    source: String,
    password: Option<String>,
    hosts_only: bool,
) -> Result<crate::config_io::ImportSummary, String> {
    let started = Instant::now();
    let bytes = payload.len() as u64;

    // 红线：解密密码用 Zeroizing 包裹，任意返回路径 drop 时清零，不进日志
    let password = password.map(Zeroizing::new);

    if bytes > crate::config_io::MAX_IMPORT_FILE_BYTES {
        let msg = format!(
            "配置文件过大（{} 字节，上限 {} 字节），请确认文件来源",
            bytes,
            crate::config_io::MAX_IMPORT_FILE_BYTES
        );
        emit_import_failed("size", None, None, &msg, &source, bytes);
        return Err(msg);
    }

    let _guard = state.0.lock().await;

    // 临界区内读现状（与合并、写盘构成事务，杜绝并发写丢失）
    let existing_hosts = store::load_hosts(app)
        .inspect_err(|e| emit_import_failed("read", None, None, e, &source, bytes))?;
    let cfg_path = app_config_file(app)?;
    let existing_cfg = cfg_persisted::load_full(&cfg_path);

    let prepared = crate::config_io::prepare_import(
        &payload,
        &existing_hosts,
        &existing_cfg,
        hosts_only,
        password.as_ref().map(|z| z.as_str()),
    )
    .map_err(|ie| {
        // 日志用细分文案（decrypt：密码错误/文件损坏），返回前端仍用统一用户文案
        let log_err = ie.log_error.as_deref().unwrap_or(&ie.error);
        emit_import_failed(
            ie.stage,
            ie.section.as_deref(),
            ie.reason,
            log_err,
            &source,
            bytes,
        );
        ie.error
    })?;

    // 解析通过：导入开始
    crate::applog::emit(
        log::Level::Info,
        "app",
        crate::applog::events::APP_CONFIG_IMPORT_START,
        None,
        if hosts_only {
            "主机导入开始"
        } else {
            "配置导入开始"
        },
        Some(serde_json::json!({
            "file_bytes": bytes,
            "file_version": prepared.file_version,
            "scope": prepared.meta_scope,
            "hosts_only": hosts_only,
        })),
    );

    // 非关键节容错跳过
    for s in &prepared.skipped {
        crate::applog::emit(
            log::Level::Warn,
            "app",
            crate::applog::events::APP_CONFIG_IMPORT_SECTION_SKIPPED,
            None,
            "导入配置节损坏，已跳过",
            Some(serde_json::json!({
                "section": s.section,
                "error": s.error,
            })),
        );
    }

    // 先写 app_config 再写 connections：两写均为临时文件+原子 rename；
    // 若 connections 写失败，app_config 的引用重映射也不会造成悬空
    // （sessions/keys.hosts 指向的最终 id 集合包含全部本地 id）。
    cfg_persisted::save_full(&cfg_path, &prepared.app_config).map_err(|e| {
        let msg = format!("写入应用配置失败: {e}");
        emit_import_failed("write", None, None, &msg, &source, bytes);
        msg
    })?;
    if let Some(new_hosts) = &prepared.hosts {
        let conn_path = store::connections_path(app)
            .inspect_err(|e| emit_import_failed("write", None, None, e, &source, bytes))?;
        store::write_hosts(&conn_path, new_hosts)
            .inspect_err(|e| emit_import_failed("write", None, None, e, &source, bytes))?;
    }

    let s = &prepared.summary;
    crate::applog::emit(
        log::Level::Info,
        "app",
        crate::applog::events::APP_CONFIG_IMPORT_COMPLETE,
        None,
        if hosts_only {
            "主机导入完成"
        } else {
            "配置导入完成"
        },
        Some(serde_json::json!({
            "connections_added": s.connections_added,
            "connections_renamed": s.connections_renamed,
            "keys_added": s.keys_added,
            "keys_renamed": s.keys_renamed,
            "groups_added": s.groups_added,
            "settings_changed": s.settings_changed,
            "skipped_sections": prepared.skipped.len(),
            "hosts_only": hosts_only,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        })),
    );

    Ok(prepared.summary)
}

/// 全量/选择性导入配置文件（§7.2）。导入成功后前端必须重启应用再加载新配置。
#[tauri::command]
pub async fn import_config(
    app: AppHandle,
    state: State<'_, ConfigWriteLock>,
    payload: String,
    source: String,
    password: Option<String>,
) -> Result<crate::config_io::ImportSummary, String> {
    slow_span!("import_config");
    run_import(&app, &state, payload, source, password, false).await
}

/// 仅导入主机（Hosts 列表「导入主机」入口，§7.7）：
/// 只消费 connections/groups；本地其余各节（含 ui_state）完全不动。
#[tauri::command]
pub async fn import_hosts(
    app: AppHandle,
    state: State<'_, ConfigWriteLock>,
    payload: String,
    source: String,
    password: Option<String>,
) -> Result<crate::config_io::ImportSummary, String> {
    slow_span!("import_hosts");
    run_import(&app, &state, payload, source, password, true).await
}

/* =========================================================
 *  重置（§7.6）
 * ========================================================= */

/// 恢复默认设置：仅把 settings 节替换为默认值，连接/密钥/分组/logs/ui_state/历史均不动。
#[tauri::command]
pub async fn reset_settings_config(
    app: AppHandle,
    state: State<'_, ConfigWriteLock>,
) -> Result<(), String> {
    slow_span!("reset_settings_config");
    let started = Instant::now();
    let _guard = state.0.lock().await;

    let path = app_config_file(&app)?;
    cfg_persisted::replace_settings_default(&path)?;

    crate::applog::emit(
        log::Level::Info,
        "app",
        crate::applog::events::APP_CONFIG_SETTINGS_RESET,
        None,
        "设置已恢复默认",
        Some(serde_json::json!({
            "elapsed_ms": started.elapsed().as_millis() as u64,
        })),
    );
    Ok(())
}

#[cfg(test)]
mod hostkey_validate_tests {
    use super::validate_trust_fingerprint;

    /// 43 字符无 padding 样本（russh fingerprint(Sha256) 的实际输出形态）与
    /// 44 字符含 padding 形态均必须放行（勿误拒，设计 §6.3）
    #[test]
    fn accepts_real_russh_fingerprint_shapes() {
        let no_pad = format!("SHA256:{}", "A".repeat(43));
        assert!(validate_trust_fingerprint(&no_pad).is_ok());
        let padded = format!("SHA256:{}=", "A".repeat(43));
        assert!(validate_trust_fingerprint(&padded).is_ok());
        // 混合 Base64 字符（+/=）
        assert!(
            validate_trust_fingerprint("SHA256:AbCd+/9=AbCd+/9=AbCd+/9=AbCd+/9=AbCd+/9=").is_ok()
        );
    }

    #[test]
    fn rejects_empty_and_missing_prefix() {
        assert!(validate_trust_fingerprint("").is_err(), "空串必须拒绝");
        assert!(
            validate_trust_fingerprint(&"A".repeat(43)).is_err(),
            "缺 SHA256: 前缀必须拒绝"
        );
        assert!(
            validate_trust_fingerprint("MD5:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").is_err()
        );
    }

    #[test]
    fn rejects_short_body_and_illegal_chars() {
        assert!(
            validate_trust_fingerprint("SHA256:AbC").is_err(),
            "主体过短必须拒绝"
        );
        assert!(
            validate_trust_fingerprint("SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA!")
                .is_err(),
            "主体含非法字符必须拒绝"
        );
        assert!(
            validate_trust_fingerprint("SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA:")
                .is_err(),
            "主体含冒号必须拒绝"
        );
        // 空格（注入面）必须拒绝
        assert!(
            validate_trust_fingerprint("SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA A").is_err(),
            "主体含空格必须拒绝"
        );
    }
}
