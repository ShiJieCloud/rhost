//! SFTP 子系统：复用已认证 SSH 连接打开 `sftp` subsystem 通道，浏览/传输远端文件。
//!
//! 与 PTY / exec 通道相互独立：每条会话懒加载一个长驻 [`RawSftpSession`]
//! （SFTP 协议内部支持多请求并发），会话断开时随 [`SshSession`] 一并丢弃；
//! 通道中途异常时缓存可失效，下次操作自动重建（列目录重试一次）。
//!
//! 之所以直接用底层 [`RawSftpSession`] 而非高层 `SftpSession`：SFTP v3 的
//! `SSH_FILEXFER_ATTRS` 只携带数字 uid/gid，属主/属组名仅存在于 NAME 响应的
//! `longname`（`ls -l` 风格文本）中，高层 read_dir 将其丢弃，故自行收发
//! READDIR 并解析 longname。
//!
//! 文件传输：固定 64KB 分块顺序 read/write，进度经 Tauri IPC
//! [`Channel`] 流式推给前端（100ms 节流，收尾必发一次）；每个任务注册
//! [`TransferCtl`]（取消令牌 + 暂停开关），支持前端暂停/继续/取消。

use std::collections::HashMap;
use std::io::SeekFrom;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use chrono::TimeZone;
use log::debug;
use russh::client;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::rawsession::RawSftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use filetime::FileTime;
use tauri::ipc::Channel;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio::sync::watch;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use super::SshError;
use super::session::{ClientHandler, SshSession};

/// 默认单块传输大小：64KB（SFTP v3 规定服务端必须接受 ≥32KB，64KB 在多数服务端吞吐更优，
/// 且远小于整文件，保证流式不占内存）。前端可通过全局设置覆盖（8KB ~ 1MB）
const DEFAULT_CHUNK: usize = 64 * 1024;
/// 进度事件最小间隔（节流）；收尾时不受此限强制补发一次
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// 单个远端目录项（字段与前端 FsEntry / 本地 LocalFsEntry 对齐）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFsEntry {
    pub name: String,
    pub is_dir: bool,
    /// 是否为符号链接。指向目录的软链 is_dir 为 true，可双击进入；
    /// 指向文件的软链 is_dir 为 false，按文件处理。
    pub is_symlink: bool,
    /// 文件字节数；目录/未知恒为 0（前端显示 —）
    pub size: u64,
    /// 本地时区 "YYYY-MM-DD HH:MM"；远端未提供时为空串
    pub mtime: String,
    /// 权限字符串 "rwxr-xr-x"
    pub perm: String,
    /// 属主/属组 "user/group"（优先 longname 解析名，缺失时回退 uid/gid）
    pub owner: String,
}

/// 一次远端目录列举结果：path 为服务端实际路径
/// （请求路径缺省时为 realpath(".") 得到的登录默认目录）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDirListing {
    pub path: String,
    pub entries: Vec<RemoteFsEntry>,
}

/// 打开 sftp 子系统通道并完成 SFTP 协议初始化（SSH_FXP_INIT / VERSION 协商）。
///
/// 失败常见原因：远端 sshd 未配置 `Subsystem sftp`（受限 Shell / 精简容器）。
async fn open_sftp(
    handle: &Arc<Mutex<client::Handle<ClientHandler>>>,
) -> Result<RawSftpSession, SshError> {
    let channel = {
        let h = handle.lock().await;
        let channel = h
            .channel_open_session()
            .await
            .map_err(|e| SshError::Channel(format!("打开 SFTP 通道失败: {e}")))?;
        channel.request_subsystem(true, "sftp").await.map_err(|e| {
            SshError::Channel(format!(
                "远端拒绝 SFTP 子系统（sftp-server 可能未启用）: {e}"
            ))
        })?;
        channel
    };
    let sftp = RawSftpSession::new(channel.into_stream());
    sftp.set_timeout(20);
    sftp.init()
        .await
        .map_err(|e| SshError::Channel(format!("SFTP 协议初始化失败: {e}")))?;
    debug!("SFTP 子系统通道已建立");
    Ok(sftp)
}

/// 单个传输任务的运行时控制：取消令牌 + 暂停开关。
///
/// 取消为单向（CancellationToken::cancel 不可逆），暂停为布尔开关
/// （watch channel，循环里 await 其变回 false 即可继续）。
pub(super) struct TransferCtl {
    pub cancel: CancellationToken,
    pub paused: watch::Sender<bool>,
}

/// 进度事件载荷（与前端约定，camelCase）。`speed` 单位 B/s，
/// `done` 为 true 表示本次为收尾包，前端可据此切到完成态。
/// `sha256` 仅在开启完整性校验且已算出源文件哈希时随收尾包下发，供队列展示。
/// `skipped` 为 true 表示按覆盖策略跳过（未发生实际字节传输），前端展示"已跳过"。
/// `verifying` 为 true 表示字节已传完、正在做 SHA256 比对（收尾前的过渡态）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferProgress {
    pub task_id: u64,
    pub transferred: u64,
    pub total: u64,
    pub speed: u64,
    pub done: bool,
    pub skipped: bool,
    pub verifying: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// SFTP 单次传输选项：由前端从全局设置组装后随每次传输传入（camelCase）。
/// 仅含传输时即需后端消费的项；调度类（并发/重试/全局限速/黑名单）由前端实现，
/// 不进入此结构体。字段均设 `default`，保证旧前端缺传时后端可用安全缺省值。
/// 半成品固定走同目录 `.filepart`，与覆盖策略无关。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SftpTransferOptions {
    /// 目标同名文件已存在时的处理：overwrite 直接覆盖 / skip 跳过 /
    /// newer 仅当源文件更新时覆盖 / ask 返回 NEED_CONFIRM 由前端询问
    pub overwrite_policy: String,
    /// 传输完成后保留源文件 mtime 与权限属性
    pub preserve_meta: bool,
    /// 断点校验方式：size / sizeMtime / sha256（part 无源 mtime 语义，sizeMtime 退化为 size）
    pub resume_check: String,
    /// 单任务带宽限速 KB/s，0=不限
    pub task_rate_kb: u64,
    /// 传输完成后 SHA256 校验：在 rename 覆盖目标前比对 part 内容
    pub verify_hash: bool,
}

impl Default for SftpTransferOptions {
    fn default() -> Self {
        Self {
            overwrite_policy: "overwrite".into(),
            preserve_meta: false,
            resume_check: "size".into(),
            task_rate_kb: 0,
            verify_hash: false,
        }
    }
}

/// 每会话的 SFTP 长驻会话缓存：懒加载，坏通道可失效重建；
/// 同时持有该会话所有进行中的传输任务控制柄。
pub(super) struct SftpState {
    inner: Mutex<Option<Arc<RawSftpSession>>>,
    tasks: Mutex<HashMap<u64, Arc<TransferCtl>>>,
    /// 文件管理操作（删除/重命名/复制等多请求串行操作）互斥锁，
    /// 避免递归遍历期间目录被另一个管理操作改动
    manage: Mutex<()>,
    /// 会话标签（通道建立日志事件用）
    sid: String,
    /// 最近一次 SFTP 操作时间：空闲超时监控据此判断是否需要关闭缓存通道
    last_active: Mutex<Instant>,
    /// 空闲超时秒数（0=不自动关闭），由前端经 IPC 下发
    idle_timeout_sec: AtomicU64,
}

impl SftpState {
    pub(super) fn new(sid: &str) -> Self {
        Self {
            inner: Mutex::new(None),
            tasks: Mutex::new(HashMap::new()),
            manage: Mutex::new(()),
            sid: sid.to_string(),
            last_active: Mutex::new(Instant::now()),
            idle_timeout_sec: AtomicU64::new(0),
        }
    }

    /// 更新空闲超时配置（秒，0 表示不自动关闭）
    pub(super) fn set_idle_timeout(&self, secs: u64) {
        self.idle_timeout_sec.store(secs, Ordering::Relaxed);
    }

    /// 是否满足空闲关闭条件：已配置超时、超时内无任何 SFTP 操作、无进行中传输，
    /// 且当前确实缓存了通道。各锁独立获取，不与 `get` 形成嵌套死锁。
    async fn should_close_idle(&self) -> bool {
        let timeout = self.idle_timeout_sec.load(Ordering::Relaxed);
        if timeout == 0 {
            return false;
        }
        if self.last_active.lock().await.elapsed() < Duration::from_secs(timeout) {
            return false;
        }
        if !self.tasks.lock().await.is_empty() {
            return false;
        }
        self.inner.lock().await.is_some()
    }

    /// 注册一个新任务的控制柄并返回（键为前端传入的 task_id）
    pub(super) async fn register_task(&self, task_id: u64, ctl: Arc<TransferCtl>) {
        self.tasks.lock().await.insert(task_id, ctl);
    }

    /// 取指定任务控制柄（暂停/取消用）
    pub(super) async fn get_ctl(&self, task_id: u64) -> Option<Arc<TransferCtl>> {
        self.tasks.lock().await.get(&task_id).cloned()
    }

    /// 任务结束（成功/失败/取消）后从表中移除，避免表无限增长
    pub(super) async fn remove_task(&self, task_id: u64) {
        self.tasks.lock().await.remove(&task_id);
        // 传输结束视为一次活跃操作，避免长任务刚结束时被空闲监控立即关闭通道
        *self.last_active.lock().await = Instant::now();
    }

    /// 取缓存的 RawSftpSession；不存在则开新通道初始化。
    /// 持锁建链，并发列目录只在首次/重建时竞争，之后共享同一协议会话。
    async fn get(
        &self,
        handle: &Arc<Mutex<client::Handle<ClientHandler>>>,
    ) -> Result<Arc<RawSftpSession>, SshError> {
        // 任何 SFTP 操作都刷新活跃时间（空闲超时以此为准）
        *self.last_active.lock().await = Instant::now();
        let mut guard = self.inner.lock().await;
        if let Some(s) = guard.as_ref() {
            return Ok(s.clone());
        }
        let s = Arc::new(open_sftp(handle).await?);
        crate::applog::emit(
            log::Level::Debug,
            "ssh",
            crate::applog::events::SSH_SFTP_CHANNEL_OPEN,
            Some(&self.sid),
            "SFTP 通道建立",
            None,
        );
        *guard = Some(s.clone());
        Ok(s)
    }

    /// 丢弃缓存的 SFTP 会话（通道异常后调用，下次 get 重新建立）
    async fn invalidate(&self) {
        *self.inner.lock().await = None;
    }
}

/// 空闲监控：会话存活期间定时检查，超过 `idle_timeout_sec` 无 SFTP 操作且无进行中
/// 传输时关闭缓存的通道（下次操作自动重建），不触碰 SSH 主连接。0 表示不自动关闭。
/// 通过 Weak 持有自身：SshSession 释放后 Weak 升级失败，任务随之退出。
pub(super) fn spawn_sftp_idle_watch(state: Weak<SftpState>, cancel: CancellationToken) {
    tokio::spawn(async move {
        const TICK: Duration = Duration::from_secs(15);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = sleep(TICK) => {}
            }
            let Some(st) = state.upgrade() else { break };
            if st.should_close_idle().await {
                let idle_s = st.idle_timeout_sec.load(Ordering::Relaxed);
                st.invalidate().await;
                crate::applog::emit(
                    log::Level::Debug,
                    "ssh",
                    crate::applog::events::SSH_SFTP_CHANNEL_CLOSE,
                    Some(&st.sid),
                    "SFTP 通道空闲超时关闭",
                    Some(serde_json::json!({ "idle_s": idle_s })),
                );
            }
        }
    });
}

/// SFTP 权限位 → "rwxr-xr-x"（与 localfs 本地面板同一套规则，含 s/S/t/T）
fn fmt_perm(mode: u32) -> String {
    let mut s = String::with_capacity(9);
    for (bit, chr) in [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ] {
        s.push(if mode & bit != 0 { chr } else { '-' });
    }
    if mode & 0o4000 != 0 {
        s.replace_range(2..=2, if mode & 0o100 != 0 { "s" } else { "S" });
    }
    if mode & 0o2000 != 0 {
        s.replace_range(5..=5, if mode & 0o010 != 0 { "s" } else { "S" });
    }
    if mode & 0o1000 != 0 {
        s.replace_range(8..=8, if mode & 0o001 != 0 { "t" } else { "T" });
    }
    s
}

/// SFTP mtime（Unix 秒）→ 本地时间 "YYYY-MM-DD HH:MM"，与本地面板格式一致
fn fmt_mtime(ts: Option<u32>) -> String {
    match ts {
        Some(t) => chrono::Local
            .timestamp_opt(i64::from(t), 0)
            .single()
            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default(),
        None => String::new(),
    }
}

/// 从 NAME 响应的 longname（sshd 生成的 `ls -l` 风格文本）解析属主/属组名。
///
/// 标准布局：`-rw-r--r-- 1 root root 924 Oct 3 11:11 passwd`，
/// 字段依次为 权限串 / 硬链接数 / owner / group / 大小 / 日期 / 文件名。
/// 文件名可含空格故不能从后向前切，但前四字段不含空格，取第 2、3 字段即可。
/// longname 缺省或格式非标准时回退数字 uid/gid（与 FileZilla 等客户端一致）。
fn fmt_owner(longname: &str, attrs: &FileAttributes) -> String {
    let (user, group) = parse_longname_owner(longname).unwrap_or_else(|| {
        (
            attrs.uid.map(|u| u.to_string()).unwrap_or_default(),
            attrs.gid.map(|g| g.to_string()).unwrap_or_default(),
        )
    });
    if user.is_empty() && group.is_empty() {
        String::new()
    } else {
        format!("{user}/{group}")
    }
}

/// 校验 longname 首字段为合法 `ls -l` 类型+权限串后，取出 owner/group 名。
fn parse_longname_owner(longname: &str) -> Option<(String, String)> {
    let mut it = longname.split_whitespace();
    let kind_perm = it.next()?;
    let mut chars = kind_perm.bytes();
    let kind = chars.next()?;
    // 类型 1 字符 + 权限 9 字符（ACL 时可能多出 '+'/'.'），至少 10
    if kind_perm.len() < 10 || !matches!(kind, b'-' | b'd' | b'l' | b'c' | b'b' | b'p' | b's') {
        return None;
    }
    let _nlink = it.next()?;
    let user = it.next()?;
    let group = it.next()?;
    Some((user.to_string(), group.to_string()))
}

impl SshSession {
    /// 更新本会话的 SFTP 空闲超时（秒，0 表示不自动关闭）。
    /// 由前端在下发全局设置时调用，仅影响缓存通道的回收，不影响 SSH 主连接。
    pub(crate) fn set_sftp_idle_timeout(&self, secs: u64) {
        self.sftp.set_idle_timeout(secs);
    }

    /// 列举远端目录。`path` 为 None/空串时优先用 Shell CWD（由 OSC 6667 钩子上报），
    /// 仍无则用登录默认目录（realpath "."），保持 Shell 与 SFTP 目录一致。
    /// SFTP 通道失效时自动重建并重试一次，避免偶发断流后面板永久不可用。
    pub(crate) async fn sftp_list_dir(
        &self,
        path: Option<String>,
    ) -> Result<RemoteDirListing, SshError> {
        let wanted = match path.filter(|p| !p.is_empty()) {
            p @ Some(_) => p,
            None => self.get_cwd().await,
        };
        match self.list_remote(&wanted).await {
            Ok(res) => Ok(res),
            Err(first) => {
                debug!("SFTP 列目录失败，重建通道后重试一次: {first}");
                self.sftp.invalidate().await;
                self.list_remote(&wanted).await
            }
        }
    }

    async fn list_remote(&self, path: &Option<String>) -> Result<RemoteDirListing, SshError> {
        let sftp = self.sftp.get(&self.handle).await?;
        let fut = async {
            let dir = match path {
                Some(p) => p.clone(),
                None => {
                    let name = sftp
                        .realpath(".")
                        .await
                        .map_err(|e| SshError::Channel(format!("解析远端默认目录失败: {e}")))?;
                    name.files
                        .first()
                        .map(|f| f.filename.clone())
                        .ok_or_else(|| {
                            SshError::Channel("解析远端默认目录失败: 服务端未返回路径".to_string())
                        })?
                }
            };

            // 自行收发 OPENDIR/READDIR/CLOSE：高层 read_dir 会丢掉 NAME 包里
            // 携带属主属组名的 longname。EOF 以 Status(EOF) 形式作为 Err 返回。
            let handle = sftp
                .opendir(&dir)
                .await
                .map_err(|e| SshError::Channel(format!("打开远端目录 {dir} 失败: {e}")))?
                .handle;
            let mut files = Vec::new();
            let read_result = loop {
                match sftp.readdir(handle.as_str()).await {
                    Ok(name) => files.extend(name.files),
                    Err(SftpError::Status(status)) if status.status_code == StatusCode::Eof => {
                        break Ok::<_, SshError>(());
                    }
                    Err(e) => {
                        break Err(SshError::Channel(format!("读取远端目录 {dir} 失败: {e}")));
                    }
                }
            };
            // 句柄尽量关闭；关闭失败不影响已读到的数据
            let _ = sftp.close(handle.as_str()).await;
            read_result?;

            let mut entries = Vec::with_capacity(files.len());
            for f in files {
                if f.filename == "." || f.filename == ".." {
                    continue;
                }
                let attrs = f.attrs;
                let is_symlink = attrs.file_type().is_symlink();
                // 符号链接需跟随目标判断类型：指向目录的软链按目录处理，
                // 指向文件的按文件处理；stat 失败（坏链接/权限不足）回退为非目录。
                let is_dir = if is_symlink {
                    let entry_path = format!("{dir}/{}", f.filename);
                    sftp.stat(&entry_path)
                        .await
                        .map(|a| a.attrs.file_type().is_dir())
                        .unwrap_or(false)
                } else {
                    attrs.file_type().is_dir()
                };
                entries.push(RemoteFsEntry {
                    is_dir,
                    is_symlink,
                    size: attrs.size.unwrap_or(0),
                    mtime: fmt_mtime(attrs.mtime),
                    perm: attrs.permissions.map(fmt_perm).unwrap_or_default(),
                    owner: fmt_owner(&f.longname, &attrs),
                    name: f.filename,
                });
            }
            Ok::<_, SshError>(RemoteDirListing { path: dir, entries })
        };
        // RawSftpSession 自带单请求 20s 超时，这里再给整个列目录一个总时限，
        // 慢主机/异常网络下前端能收到明确失败而非无限转圈
        tokio::time::timeout(Duration::from_secs(20), fut)
            .await
            .map_err(|_| SshError::Channel("SFTP 操作超时（20 秒）".to_string()))?
    }

    /// 在远端创建目录（单层，不递归；父目录不存在时报错）。
    pub(crate) async fn sftp_mkdir(&self, path: String) -> Result<(), SshError> {
        let sftp = self.sftp.get(&self.handle).await?;
        sftp.mkdir(&path, FileAttributes::default())
            .await
            .map_err(|e| SshError::Channel(format!("创建目录 {path} 失败: {e}")))?;
        debug!("SFTP 创建目录: {path}");
        Ok(())
    }

    /// 单文件上传：本地 → 远端。分块大小由 `chunk_kb` 指定（缺省 64KB），进度经
    /// IPC [`Channel`] 流式推送，支持暂停/取消。数据先写入目标同目录的
    /// `<目标>.filepart`，完成并（开启时）通过 SHA256 校验后同目录 rename 覆盖目标；
    /// 校验失败只删 part，目标旧版本不受影响。**断点续传**（`resume` 开启时）：
    /// 以 part 已有大小判定续写起点；part 比源大（源已变更）时截断重传。
    /// `resume` 关闭时一律全量重传。
    pub(crate) async fn upload_file(
        &self,
        task_id: u64,
        local_path: String,
        remote_path: String,
        chunk_kb: Option<u32>,
        resume: Option<bool>,
        max_bytes: Option<u64>,
        options: SftpTransferOptions,
        channel: Channel<TransferProgress>,
    ) -> Result<(), SshError> {
        let chunk = chunk_kb
            .unwrap_or((DEFAULT_CHUNK / 1024) as u32)
            .clamp(8, 1024) as usize
            * 1024;
        let resume = resume.unwrap_or(true);
        let cancel = self.child_cancel();
        let (paused_tx, mut paused_rx) = watch::channel(false);
        self.sftp
            .register_task(
                task_id,
                Arc::new(TransferCtl {
                    cancel: cancel.clone(),
                    paused: paused_tx,
                }),
            )
            .await;

        // 源文件 SHA256（仅 verify 开启时算出）：随收尾进度包下发，队列展示用
        let mut final_hash: Option<String> = None;
        // async block 返回 Result<(已传, 总大小), (错误, 已传, 总大小)>，
        // 这样内部可用 ?，且错误路径也能拿到已传字节用于收尾进度
        let outcome: Result<(u64, u64, bool), (SshError, u64, u64)> = async {
            let sftp = self
                .sftp
                .get(&self.handle)
                .await
                .map_err(|e| (e, 0u64, 0u64))?;
            let local = expand_tilde(&local_path);
            let mut src = File::open(&local).await.map_err(|e| {
                (
                    SshError::Channel(format!("打开本地文件 {local} 失败: {e}")),
                    0,
                    0,
                )
            })?;
            let src_meta = src.metadata().await.map_err(|e| {
                (
                    SshError::Channel(format!("读取本地文件元数据失败: {e}")),
                    0,
                    0,
                )
            })?;
            let total = src_meta.len();
            // 源 mtime（Unix 秒）：用于 newer 覆盖比较与保留元数据
            let src_mtime: u64 = src_meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            // 源权限 mode（unix）；非 unix 平台回退 0o644
            let src_perm: u32 = {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    src_meta.permissions().mode()
                }
                #[cfg(not(unix))]
                {
                    0o644
                }
            };

            // 体积上限预检：在打开远端文件写入前拒绝，避免生成残留的远端空/截断文件
            if let Some(max) = max_bytes {
                if total > max {
                    return Err((
                        SshError::TransferRejected(format!(
                            "文件 {:.1}MB 超过 AI 传输上限（64MB），请通过 SFTP 文件面板手动传输，不要使用 scp/rsync 等命令行传输",
                            total as f64 / 1024.0 / 1024.0
                        )),
                        0u64,
                        total,
                    ));
                }
            }

            // 覆盖策略：目标已存在时按策略决定跳过/询问；overwrite 直接进入原有覆盖流程。
            // 仅探测一次，未命中策略（overwrite / 源更新）时继续走 .filepart + rename
            if options.overwrite_policy != "overwrite" {
                let target = probe_remote(&sftp, &remote_path).await.map_err(|e| {
                    (
                        SshError::Channel(format!("探测远端目标 {remote_path} 失败: {e}")),
                        0u64,
                        total,
                    )
                })?;
                if let Some(attrs) = target {
                    match options.overwrite_policy.as_str() {
                        // skip：目标已存在即视为完成，不创建任何文件
                        "skip" => return Ok((0, total, true)),
                        // newer：远端不旧于源（秒级 mtime）则跳过，避免用旧内容覆盖新内容
                        "newer" => {
                            let target_mtime = attrs.mtime.unwrap_or(0) as u64;
                            if target_mtime >= src_mtime {
                                return Ok((0, total, true));
                            }
                        }
                        // ask：交由前端询问，前缀 NEED_CONFIRM 是前后端约定标记
                        "ask" => {
                            return Err((
                                SshError::TransferRejected(format!(
                                    "NEED_CONFIRM: 远端文件已存在: {remote_path}"
                                )),
                                0u64,
                                total,
                            ));
                        }
                        _ => {}
                    }
                }
            }

            // 半成品固定写在目标同目录的 `<目标>.filepart`，完成后同目录 rename 覆盖目标：
            // 传输与校验通过前目标保持旧版本，失败只影响 part
            let part_path = format!("{remote_path}.filepart");
            let part_meta = sftp.lstat(&part_path).await;
            let part_attrs = match part_meta {
                Ok(m) => Some(m.attrs),
                Err(SftpError::Status(s)) if s.status_code == StatusCode::NoSuchFile => None,
                Err(e) => {
                    return Err((
                        SshError::Channel(format!("探测半成品文件 {part_path} 失败: {e}")),
                        0,
                        total,
                    ));
                }
            };
            // 断点续传以 part 大小为准：目标旧内容不参与续传
            let part_size = part_attrs.as_ref().and_then(|a| a.size).unwrap_or(0);
            let offset = if !resume || part_size > total {
                0 // 关闭续传 / part 比源大（源文件已变更）：截断重传
            } else if part_size == total {
                total // part 已完整：跳过传输，直接进入校验与 rename
            } else if part_size > 0 {
                let can_resume = match options.resume_check.as_str() {
                    "sha256" => {
                        // 前缀哈希比对；>32MB 退化为按大小续传（内容差异由完成后的校验兜底）
                        if part_size > 32 * 1024 * 1024 {
                            true
                        } else {
                            verify_prefix_sha256(&sftp, &part_path, &local, part_size).await
                        }
                    }
                    // sizeMtime：part 的 mtime 是上次写入时间，无法与源 mtime 比对，退化为按大小
                    _ => true,
                };
                if can_resume {
                    part_size // 从断点续写
                } else {
                    0 // 校验未通过，截断重传
                }
            } else {
                0
            };
            // 完整性校验：源（本地）哈希随传输流式累积，仅在开启校验时启用
            let mut src_hasher = options.verify_hash.then(Sha256::new);
            if offset > 0 {
                if let Some(h) = src_hasher.as_mut() {
                    // 续传：先补算已存在前缀，保证最终哈希覆盖整个文件
                    feed_local_prefix(h, &local, offset)
                        .await
                        .map_err(|e| (e, 0u64, total))?;
                }
                src.seek(SeekFrom::Start(offset)).await.map_err(|e| {
                    (
                        SshError::Channel(format!("定位本地文件偏移失败: {e}")),
                        0,
                        total,
                    )
                })?;
            }

            // 续传时不带 TRUNCATE 保留 part 已有内容；从头传时 TRUNCATE 清除残留
            let flags = if offset > 0 {
                OpenFlags::WRITE | OpenFlags::CREATE
            } else {
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE
            };
            let rhandle = sftp
                .open(&part_path, flags, FileAttributes::default())
                .await
                .map_err(|e| {
                    (
                        SshError::Channel(format!("打开半成品文件 {part_path} 失败: {e}")),
                        0,
                        total,
                    )
                })?
                .handle;

            let mut buf = vec![0u8; chunk];
            let mut offset = offset;
            let mut transferred: u64 = offset; // 进度含已续传字节
            let mut last_emit = Instant::now();
            let mut last_bytes = offset;
            // 续传起始即上报一次，前端进度条直接跳到断点位置
            if offset > 0 {
                emit_progress(&channel, task_id, transferred, total, 0, false, None);
            }
            let mut limiter = RateLimiter::new(options.task_rate_kb);

            loop {
                if let Err(e) = wait_if_paused(&mut paused_rx, &cancel).await {
                    // 用户取消：关闭句柄后删除 part；暂停/网络失败仍保留 part 供续传
                    let _ = sftp.close(rhandle.as_str()).await;
                    remove_part_file(&sftp, &part_path).await;
                    return Err((e, transferred, total));
                }
                if cancel.is_cancelled() {
                    let _ = sftp.close(rhandle.as_str()).await;
                    remove_part_file(&sftp, &part_path).await;
                    return Err((SshError::Cancelled, transferred, total));
                }
                // 只读到开始时的 total 为止：源文件若被追加，不把多余字节带上（收尾 stat 会报变更）
                let remaining = total.saturating_sub(offset);
                if remaining == 0 {
                    break;
                }
                let want = remaining.min(chunk as u64) as usize;
                let n = src.read(&mut buf[..want]).await.map_err(|e| {
                    (
                        SshError::Channel(format!("读取本地文件失败: {e}")),
                        transferred,
                        total,
                    )
                })?;
                if n == 0 {
                    break; // 传输完成：退出循环执行收尾（校验 / 同目录 rename）
                }
                sftp.write(rhandle.as_str(), offset, buf[..n].to_vec())
                    .await
                    .map_err(|e| {
                        (
                            SshError::Channel(format!("写入远端失败: {e}")),
                            transferred,
                            total,
                        )
                    })?;
                if let Some(h) = src_hasher.as_mut() {
                    h.update(&buf[..n]);
                }
                limiter.allow(n as u64).await;
                offset += n as u64;
                transferred += n as u64;

                let now = Instant::now();
                if now.duration_since(last_emit) >= PROGRESS_INTERVAL {
                    let speed = ((transferred - last_bytes) * 1000)
                        / now.duration_since(last_emit).as_millis().max(1) as u64;
                    emit_progress(&channel, task_id, transferred, total, speed, false, None);
                    last_emit = now;
                    last_bytes = transferred;
                }
            }

            // 收尾顺序：关闭写句柄 → 写 part 元数据 → 校验 part → 同目录 rename 覆盖目标。
            // 校验未通过只删除 part，目标旧版本保持不动
            let _ = sftp.close(rhandle.as_str()).await;
            // 数据已传完但在校验/rename 前被取消：同样按“取消删 part”处理
            if cancel.is_cancelled() {
                remove_part_file(&sftp, &part_path).await;
                return Err((SshError::Cancelled, transferred, total));
            }
            if options.preserve_meta {
                // 属性写在 part 上，rename 覆盖后随 inode 一并生效
                let mut attrs = sftp
                    .lstat(&part_path)
                    .await
                    .map(|m| m.attrs)
                    .unwrap_or_default();
                attrs.mtime = Some(src_mtime as u32);
                attrs.permissions = Some(src_perm);
                if let Err(e) = sftp.setstat(&part_path, attrs).await {
                    debug!("SFTP 保留元数据失败（不影响传输结果）: {e}");
                }
            }
            if options.verify_hash {
                // 数据已全部写完，进入整文件哈希比对：先推一个 100% 进度包，
                // 前端据此显示"校验中"转圈（最终 done 包在比对结束后才发出）
                emit_verifying(&channel, task_id, total);
                // 源哈希在传输过程中已流式计算完成，无需再读本地文件
                let local_hash = format!(
                    "{:x}",
                    src_hasher
                        .take()
                        .expect("verify 开启时已创建源哈希")
                        .finalize()
                );
                final_hash = Some(local_hash.clone());
                // 严格模式：源文件在传输期间不得被其它进程修改
                if let Err(e) = check_local_unchanged(&local, total, src_mtime).await {
                    remove_part_file(&sftp, &part_path).await;
                    return Err((e, transferred, total));
                }
                // 目标哈希仍需回读远端 part（SFTP 写入无内容回显）；
                // 回读失败同样删除 part，避免下次因大小相等被当成已完成而跳过校验
                let remote_hash = match remote_file_sha256(&sftp, &part_path).await {
                    Ok(h) => h,
                    Err(e) => {
                        remove_part_file(&sftp, &part_path).await;
                        return Err((e, transferred, total));
                    }
                };
                if remote_hash != local_hash {
                    let name = local.rsplit('/').next().unwrap_or(&local);
                    crate::applog::emit(
                        log::Level::Error,
                        "sftp",
                        crate::applog::events::SFTP_TRANSFER_VERIFY_FAILED,
                        Some(self.sftp.sid.as_str()),
                        "完整性校验失败",
                        Some(serde_json::json!({
                            "name": name,
                            "expect": local_hash,
                            "actual": remote_hash,
                        })),
                    );
                    // 校验不通过：只删 part，目标旧版本不受影响
                    remove_part_file(&sftp, &part_path).await;
                    return Err((
                        SshError::TransferRejected("文件完整性校验失败".to_string()),
                        transferred,
                        total,
                    ));
                }
                debug!("SFTP 上传 SHA256 校验通过: {part_path}");
            }
            // 校验通过（或未开启校验）才替换目标：同目录 rename 为原子覆盖
            if let Err(e) = rename_overwrite(&sftp, &part_path, &remote_path).await {
                // 保留 part：重试可继续续传或只重做 rename
                return Err((
                    SshError::Channel(format!(
                        "重命名半成品 {part_path} → {remote_path} 失败: {e}"
                    )),
                    transferred,
                    total,
                ));
            }
            debug!("SFTP 上传完成: {part_path} → {remote_path}");
            Ok((transferred, total, false))
        }
        .await;

        let (result, transferred, total, skipped) = match outcome {
            Ok((t, total, skipped)) => (Ok(()), t, total, skipped),
            Err((e, t, total)) => (Err(e), t, total, false),
        };

        self.sftp.remove_task(task_id).await;
        emit_final_progress(
            &channel,
            task_id,
            transferred,
            total,
            final_hash.as_deref(),
            skipped,
        );
        result
    }

    /// 单文件下载：远端 → 本地。本地父目录不存在时自动创建；分块大小由
    /// `chunk_kb` 指定（缺省 64KB），进度经 IPC [`Channel`] 流式推送，支持暂停/取消。
    /// 数据先写入目标同目录的 `<目标>.filepart`，完成并（开启时）通过 SHA256 校验后
    /// 同目录 rename 覆盖目标；校验失败只删 part，本地目标旧版本不受影响。
    /// **断点续传**（`resume` 开启时）：以 part 已有大小判定续写起点；part 比源大
    /// （源文件变更）时截断重传。`resume` 关闭时一律全量重传。
    pub(crate) async fn download_file(
        &self,
        task_id: u64,
        remote_path: String,
        local_path: String,
        chunk_kb: Option<u32>,
        resume: Option<bool>,
        max_bytes: Option<u64>,
        options: SftpTransferOptions,
        channel: Channel<TransferProgress>,
    ) -> Result<(), SshError> {
        let chunk = chunk_kb
            .unwrap_or((DEFAULT_CHUNK / 1024) as u32)
            .clamp(8, 1024) as usize
            * 1024;
        let resume = resume.unwrap_or(true);
        let cancel = self.child_cancel();
        let (paused_tx, mut paused_rx) = watch::channel(false);
        self.sftp
            .register_task(
                task_id,
                Arc::new(TransferCtl {
                    cancel: cancel.clone(),
                    paused: paused_tx,
                }),
            )
            .await;

        // 源文件 SHA256（仅 verify 开启时算出）：随收尾进度包下发，队列展示用
        let mut final_hash: Option<String> = None;
        let outcome: Result<(u64, u64, bool), (SshError, u64, u64)> = async {
            let sftp = self
                .sftp
                .get(&self.handle)
                .await
                .map_err(|e| (e, 0u64, 0u64))?;
            let local = expand_tilde(&local_path);
            if let Some(parent) = std::path::Path::new(&local).parent() {
                if !parent.as_os_str().is_empty() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
            }

            let attrs = sftp
                .lstat(&remote_path)
                .await
                .map_err(|e| {
                    (
                        SshError::Channel(format!("获取远端文件 {remote_path} 属性失败: {e}")),
                        0,
                        0,
                    )
                })?
                .attrs;
            let total = attrs.size.unwrap_or(0);
            // 远端 mtime / 权限：保留元数据与源变更检测用
            let remote_mtime: u64 = attrs.mtime.unwrap_or(0) as u64;
            let remote_perm: Option<u32> = attrs.permissions;

            // 体积上限预检：在创建本地文件前拒绝，避免生成残留的本地空/截断文件
            if let Some(max) = max_bytes {
                if total > max {
                    return Err((
                        SshError::TransferRejected(format!(
                            "文件 {:.1}MB 超过 AI 传输上限（64MB），请通过 SFTP 文件面板手动传输，不要使用 scp/rsync 等命令行传输",
                            total as f64 / 1024.0 / 1024.0
                        )),
                        0u64,
                        total,
                    ));
                }
            }

            // 覆盖策略：本地目标已存在时按策略决定跳过/询问；overwrite 直接进入原有覆盖流程
            if options.overwrite_policy != "overwrite" {
                if let Ok(meta) = tokio::fs::metadata(&local).await {
                    // 本地 mtime 秒级，与 SFTP 属性精度对齐
                    let local_mtime = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    match options.overwrite_policy.as_str() {
                        // skip：目标已存在即视为完成，不创建任何本地文件
                        "skip" => return Ok((0, total, true)),
                        // newer：本地不旧于远端源则跳过，避免用旧内容覆盖新内容
                        // （mtime 取不到时按 0 处理，此时宁可不跳过）
                        "newer" if local_mtime > 0 && local_mtime >= remote_mtime => {
                            return Ok((0, total, true))
                        }
                        // ask：交由前端询问，前缀 NEED_CONFIRM 是前后端约定标记
                        "ask" => {
                            return Err((
                                SshError::TransferRejected(format!(
                                    "NEED_CONFIRM: 本地文件已存在: {local}"
                                )),
                                0u64,
                                total,
                            ));
                        }
                        _ => {}
                    }
                }
            }

            // 半成品固定写在目标同目录的 `<目标>.filepart`，完成后同目录 rename 覆盖目标：
            // 传输与校验通过前本地目标保持旧版本，失败只影响 part
            let part_path = format!("{local}.filepart");
            let part_size = tokio::fs::metadata(&part_path)
                .await
                .map(|m| m.len())
                .unwrap_or(0);
            let offset = if !resume || part_size > total {
                0 // 关闭续传 / part 比源大（源文件已变更）：截断重传
            } else if part_size == total {
                total // part 已完整：跳过传输，直接进入校验与 rename
            } else if part_size > 0 {
                let can_resume = match options.resume_check.as_str() {
                    "sha256" => {
                        // 前缀哈希比对；>32MB 退化为按大小续传（内容差异由完成后的校验兜底）
                        if part_size > 32 * 1024 * 1024 {
                            true
                        } else {
                            verify_prefix_sha256(&sftp, &remote_path, &part_path, part_size).await
                        }
                    }
                    // sizeMtime：part 的 mtime 是上次写入时间，无法与源 mtime 比对，退化为按大小
                    _ => true,
                };
                if can_resume {
                    part_size // 从断点续写
                } else {
                    0 // 校验未通过，截断重传
                }
            } else {
                0
            };
            // 完整性校验：源（远端）与目标（本地）哈希随传输流式累积
            let mut src_hasher = options.verify_hash.then(Sha256::new);
            let mut dst_hasher = options.verify_hash.then(Sha256::new);
            if offset > 0 {
                // 续传：先补算两端已存在前缀，保证最终哈希覆盖整个文件
                if let Some(h) = src_hasher.as_mut() {
                    feed_remote_prefix(&sftp, &remote_path, offset, h)
                        .await
                        .map_err(|e| (e, 0u64, total))?;
                }
                if let Some(h) = dst_hasher.as_mut() {
                    feed_local_prefix(h, &part_path, offset)
                        .await
                        .map_err(|e| (e, 0u64, total))?;
                }
            }
            // 续传用 APPEND 保留 part 已有内容；从头传用 CREATE 截断
            let mut dst = if offset > 0 {
                OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(&part_path)
                    .await
                    .map_err(|e| {
                        (
                            SshError::Channel(format!("打开本地半成品 {part_path} 续写失败: {e}")),
                            0,
                            total,
                        )
                    })?
            } else {
                File::create(&part_path).await.map_err(|e| {
                    (
                        SshError::Channel(format!("创建本地半成品 {part_path} 失败: {e}")),
                        0,
                        total,
                    )
                })?
            };

            let rhandle = sftp
                .open(&remote_path, OpenFlags::READ, FileAttributes::default())
                .await
                .map_err(|e| {
                    (
                        SshError::Channel(format!("打开远端文件 {remote_path} 失败: {e}")),
                        0,
                        total,
                    )
                })?
                .handle;

            let mut offset = offset;
            let mut transferred: u64 = offset; // 进度含已续传字节
            let mut last_emit = Instant::now();
            let mut last_bytes = offset;
            // 续传起始即上报一次，前端进度条直接跳到断点位置
            if offset > 0 {
                emit_progress(&channel, task_id, transferred, total, 0, false, None);
            }
            let mut limiter = RateLimiter::new(options.task_rate_kb);

            loop {
                if let Err(e) = wait_if_paused(&mut paused_rx, &cancel).await {
                    // 用户取消：关闭句柄后删除 part；暂停/网络失败仍保留 part 供续传
                    let _ = dst.flush().await;
                    drop(dst);
                    let _ = sftp.close(rhandle.as_str()).await;
                    remove_part_file_local(&part_path).await;
                    return Err((e, transferred, total));
                }
                if cancel.is_cancelled() {
                    let _ = dst.flush().await;
                    drop(dst);
                    let _ = sftp.close(rhandle.as_str()).await;
                    remove_part_file_local(&part_path).await;
                    return Err((SshError::Cancelled, transferred, total));
                }
                // 只读到开始时的 total 为止：远端若被追加，不把多余字节写入本地（收尾 stat 会报变更）
                let remaining = total.saturating_sub(offset);
                if remaining == 0 {
                    break;
                }
                let want = remaining.min(chunk as u64) as u32;
                // russh-sftp 的 read 在 EOF 时返回 Status(Eof) 错误，而非空数据
                let read_res = sftp.read(rhandle.as_str(), offset, want).await;
                let data = match read_res {
                    Ok(d) => d.data,
                    Err(SftpError::Status(status)) if status.status_code == StatusCode::Eof => {
                        break; // 传输完成：退出循环执行收尾（校验 / 同目录 rename）
                    }
                    Err(e) => {
                        return Err((
                            SshError::Channel(format!("读取远端失败: {e}")),
                            transferred,
                            total,
                        ));
                    }
                };
                if data.is_empty() {
                    break;
                }
                dst.write_all(&data).await.map_err(|e| {
                    (
                        SshError::Channel(format!("写入本地文件失败: {e}")),
                        transferred,
                        total,
                    )
                })?;
                if let Some(h) = src_hasher.as_mut() {
                    h.update(&data);
                }
                if let Some(h) = dst_hasher.as_mut() {
                    h.update(&data);
                }
                limiter.allow(data.len() as u64).await;
                offset += data.len() as u64;
                transferred += data.len() as u64;

                let now = Instant::now();
                if now.duration_since(last_emit) >= PROGRESS_INTERVAL {
                    let speed = ((transferred - last_bytes) * 1000)
                        / now.duration_since(last_emit).as_millis().max(1) as u64;
                    emit_progress(&channel, task_id, transferred, total, speed, false, None);
                    last_emit = now;
                    last_bytes = transferred;
                }
            }

            // 收尾顺序：关闭本地句柄 → 写 part 元数据 → 校验 part → 同目录 rename 覆盖目标。
            // 校验未通过只删除 part，本地目标旧版本保持不动
            let _ = dst.flush().await;
            drop(dst);
            let _ = sftp.close(rhandle.as_str()).await;
            // 数据已传完但在校验/rename 前被取消：同样按“取消删 part”处理
            if cancel.is_cancelled() {
                remove_part_file_local(&part_path).await;
                return Err((SshError::Cancelled, transferred, total));
            }
            if options.preserve_meta {
                // 属性写在 part 上，rename 覆盖后随 inode 一并生效
                preserve_local_meta(&part_path, remote_mtime, remote_perm);
            }
            if options.verify_hash {
                // 数据已全部写完，进入整文件哈希比对：先推一个 100% 进度包，
                // 前端据此显示"校验中"转圈（最终 done 包在比对结束后才发出）
                emit_verifying(&channel, task_id, total);
                // 源、目标哈希都已在传输过程中流式算完，无需再读任何一端
                let remote_hash = format!(
                    "{:x}",
                    src_hasher
                        .take()
                        .expect("verify 开启时已创建源哈希")
                        .finalize()
                );
                let local_hash = format!(
                    "{:x}",
                    dst_hasher
                        .take()
                        .expect("verify 开启时已创建目标哈希")
                        .finalize()
                );
                final_hash = Some(remote_hash.clone());
                // 严格模式：远端源文件在传输期间不得被其它进程修改
                if let Err(e) =
                    check_remote_unchanged(&sftp, &remote_path, total, remote_mtime).await
                {
                    remove_part_file_local(&part_path).await;
                    return Err((e, transferred, total));
                }
                if local_hash != remote_hash {
                    let name = remote_path.rsplit('/').next().unwrap_or(&remote_path);
                    crate::applog::emit(
                        log::Level::Error,
                        "sftp",
                        crate::applog::events::SFTP_TRANSFER_VERIFY_FAILED,
                        Some(self.sftp.sid.as_str()),
                        "完整性校验失败",
                        Some(serde_json::json!({
                            "name": name,
                            "expect": remote_hash,
                            "actual": local_hash,
                        })),
                    );
                    // 校验不通过：只删 part，目标旧版本不受影响
                    remove_part_file_local(&part_path).await;
                    return Err((
                        SshError::TransferRejected("文件完整性校验失败".to_string()),
                        transferred,
                        total,
                    ));
                }
                debug!("SFTP 下载 SHA256 校验通过: {part_path}");
            }
            // 校验通过（或未开启校验）才替换目标：同目录 rename 为原子覆盖
            if let Err(e) = tokio::fs::rename(&part_path, &local).await {
                // 保留 part：重试可继续续传或只重做 rename
                return Err((
                    SshError::Channel(format!(
                        "重命名半成品 {part_path} → {local} 失败: {e}"
                    )),
                    transferred,
                    total,
                ));
            }
            debug!("SFTP 下载完成: {part_path} → {local}");
            Ok((transferred, total, false))
        }
        .await;

        let (result, transferred, total, skipped) = match outcome {
            Ok((t, total, skipped)) => (Ok(()), t, total, skipped),
            Err((e, t, total)) => (Err(e), t, total, false),
        };

        self.sftp.remove_task(task_id).await;
        emit_final_progress(
            &channel,
            task_id,
            transferred,
            total,
            final_hash.as_deref(),
            skipped,
        );
        result
    }

    /// 暂停/继续指定传输任务（paused=true 暂停，false 继续）
    pub(crate) async fn transfer_pause(&self, task_id: u64, paused: bool) -> Result<(), SshError> {
        let ctl = self.sftp.get_ctl(task_id).await.ok_or(SshError::NotFound)?;
        let _ = ctl.paused.send(paused);
        Ok(())
    }

    /// 取消指定传输任务
    pub(crate) async fn transfer_cancel(&self, task_id: u64) -> Result<(), SshError> {
        let ctl = self.sftp.get_ctl(task_id).await.ok_or(SshError::NotFound)?;
        ctl.cancel.cancel();
        Ok(())
    }

    /// 删除远端文件/目录。目录递归删除（DFS 串行）：先清空全部子孙再 rmdir；
    /// 符号链接只删链接本身，不触碰其指向的目标。整个操作持管理锁。
    pub(crate) async fn sftp_remove_path(&self, path: String) -> Result<(), SshError> {
        let _g = self.sftp.manage.lock().await;
        let sftp = self.sftp.get(&self.handle).await?;
        // lstat 不跟随软链：坏链接也能取到属性
        let attrs = sftp
            .lstat(&path)
            .await
            .map_err(|e| SshError::Channel(format!("获取 {path} 属性失败: {e}")))?
            .attrs;
        remove_recursive(&sftp, &path, &attrs).await
    }

    /// 重命名/移动远端路径。优先 `posix-rename@openssh.com`（原子语义，
    /// 目标存在时覆盖）；服务端不支持时降级标准 RENAME（目标存在则报错）。
    pub(crate) async fn sftp_rename_path(
        &self,
        old_path: String,
        new_path: String,
    ) -> Result<(), SshError> {
        let _g = self.sftp.manage.lock().await;
        let sftp = self.sftp.get(&self.handle).await?;
        if posix_rename(&sftp, &old_path, &new_path).await.is_ok() {
            debug!("SFTP posix-rename: {old_path} → {new_path}");
            return Ok(());
        }
        debug!("posix-rename 不可用，降级标准 rename: {old_path} → {new_path}");
        sftp.rename(&old_path, &new_path)
            .await
            .map_err(|e| SshError::Channel(format!("重命名 {old_path} → {new_path} 失败: {e}")))?;
        Ok(())
    }

    /// 创建零字节空文件（CREATE，不截断已存在文件；前端调用前做重名校验）
    pub(crate) async fn sftp_create_file(&self, path: String) -> Result<(), SshError> {
        let sftp = self.sftp.get(&self.handle).await?;
        let handle = sftp
            .open(&path, OpenFlags::CREATE, FileAttributes::default())
            .await
            .map_err(|e| SshError::Channel(format!("创建文件 {path} 失败: {e}")))?
            .handle;
        let _ = sftp.close(handle.as_str()).await;
        debug!("SFTP 创建文件: {path}");
        Ok(())
    }

    /// 远端复制（同主机内）。目录递归复制；符号链接复制为指向同目标的新软链
    /// （不跟随）；文件经同一 SFTP 会话 64KB 流式 read+write，不落地本地磁盘。
    /// 整个操作持管理锁。
    pub(crate) async fn sftp_copy_path(&self, src: String, dst: String) -> Result<(), SshError> {
        let _g = self.sftp.manage.lock().await;
        let sftp = self.sftp.get(&self.handle).await?;
        copy_recursive(&sftp, &src, &dst).await
    }

    /// 读取符号链接的目标路径
    pub(crate) async fn sftp_readlink(&self, path: String) -> Result<String, SshError> {
        let sftp = self.sftp.get(&self.handle).await?;
        let name = sftp
            .readlink(&path)
            .await
            .map_err(|e| SshError::Channel(format!("读取链接 {path} 失败: {e}")))?;
        name.files
            .first()
            .map(|f| f.filename.clone())
            .ok_or_else(|| SshError::Channel(format!("读取链接 {path} 失败：服务端未返回目标")))
    }
}

/// OpenSSH posix-rename 扩展名（OpenSSH 5.0+ 支持；语义同 renameat2 RENAME，
/// 目标存在时原子覆盖，标准 RENAME 则要求目标不存在）
const POSIX_RENAME: &str = "posix-rename@openssh.com";

/// 编码一个 SFTP string（u32 大端长度 + 原始字节）到 buf
fn put_sftp_string(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(&(s.len() as u32).to_be_bytes());
    buf.extend_from_slice(s.as_bytes());
}

/// 拼接父目录与子项名（父目录为 "/" 时不重复斜杠）
fn join_child(dir: &str, name: &str) -> String {
    if dir == "/" {
        format!("/{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// 列举目录的全部直接子项（跳过 "."/".."），返回 (名称, 属性)。
/// 调用方传入的 dir 必须是可打开目录；EOF 以 Status(EOF) 识别。
async fn list_children(
    sftp: &RawSftpSession,
    dir: &str,
) -> Result<Vec<(String, FileAttributes)>, SshError> {
    let handle = sftp
        .opendir(dir)
        .await
        .map_err(|e| SshError::Channel(format!("打开目录 {dir} 失败: {e}")))?
        .handle;
    let mut out = Vec::new();
    let read_result = loop {
        match sftp.readdir(handle.as_str()).await {
            Ok(name) => {
                for f in name.files {
                    if f.filename != "." && f.filename != ".." {
                        out.push((f.filename, f.attrs));
                    }
                }
            }
            Err(SftpError::Status(status)) if status.status_code == StatusCode::Eof => {
                break Ok::<_, SshError>(());
            }
            Err(e) => break Err(SshError::Channel(format!("读取目录 {dir} 失败: {e}"))),
        }
    };
    let _ = sftp.close(handle.as_str()).await;
    read_result?;
    Ok(out)
}

/// 递归删除：软链/非目录 → remove（软链只删链接本身）；目录 → 先删全部子孙再 rmdir。
/// 递归 async fn 需 Box::pin 避免无限大小 future。
async fn remove_recursive(
    sftp: &RawSftpSession,
    path: &str,
    attrs: &FileAttributes,
) -> Result<(), SshError> {
    let ftype = attrs.file_type();
    if ftype.is_symlink() || !ftype.is_dir() {
        sftp.remove(path)
            .await
            .map_err(|e| SshError::Channel(format!("删除 {path} 失败: {e}")))?;
        return Ok(());
    }
    let children = list_children(sftp, path).await?;
    for (name, child_attrs) in children {
        let child = join_child(path, &name);
        Box::pin(remove_recursive(sftp, &child, &child_attrs)).await?;
    }
    sftp.rmdir(path)
        .await
        .map_err(|e| SshError::Channel(format!("删除目录 {path} 失败: {e}")))?;
    Ok(())
}

/// 经 EXTENDED 发起 posix-rename；服务端不支持该扩展时返回 Failure，调用方据此降级
async fn posix_rename(
    sftp: &RawSftpSession,
    old_path: &str,
    new_path: &str,
) -> Result<(), SshError> {
    let mut data = Vec::with_capacity(old_path.len() + new_path.len() + 8);
    put_sftp_string(&mut data, old_path);
    put_sftp_string(&mut data, new_path);
    sftp.extended(POSIX_RENAME, data)
        .await
        .map_err(|e| SshError::Channel(format!("posix-rename 扩展请求失败: {e}")))?;
    Ok(())
}

/// 同目录原子覆盖：优先 posix-rename（目标存在也覆盖），不支持时降级标准 rename
async fn rename_overwrite(sftp: &RawSftpSession, from: &str, to: &str) -> Result<(), SshError> {
    if posix_rename(sftp, from, to).await.is_ok() {
        return Ok(());
    }
    sftp.rename(from, to)
        .await
        .map(|_| ())
        .map_err(|e| SshError::Channel(format!("rename {from} → {to} 失败: {e}")))
}

/// 校验失败时删除远端半成品；删除失败只记日志（目标未被触碰，不影响数据安全）
async fn remove_part_file(sftp: &RawSftpSession, part_path: &str) {
    if let Err(e) = sftp.remove(part_path).await {
        debug!("删除远端半成品 {part_path} 失败: {e}");
    }
}

/// 探测远端目标是否存在：`NoSuchFile` 视为不存在（Ok(None)），其余错误上抛。
/// 网络类错误不能被当成"目标不存在"，否则覆盖策略会误判并直接覆盖。
async fn probe_remote(
    sftp: &RawSftpSession,
    path: &str,
) -> Result<Option<FileAttributes>, SftpError> {
    match sftp.lstat(path).await {
        Ok(meta) => Ok(Some(meta.attrs)),
        Err(SftpError::Status(s)) if s.status_code == StatusCode::NoSuchFile => Ok(None),
        Err(e) => Err(e),
    }
}

/// 本地半成品删除，语义同 `remove_part_file`
async fn remove_part_file_local(part_path: &str) {
    if let Err(e) = tokio::fs::remove_file(part_path).await {
        debug!("删除本地半成品 {part_path} 失败: {e}");
    }
}

/// 单文件同会话流式复制：固定 64KB read→write，两端句柄用完即关，
/// 不落地本地磁盘、不占整文件内存。
async fn copy_file_stream(sftp: &RawSftpSession, src: &str, dst: &str) -> Result<(), SshError> {
    let rhandle = sftp
        .open(src, OpenFlags::READ, FileAttributes::default())
        .await
        .map_err(|e| SshError::Channel(format!("打开源文件 {src} 失败: {e}")))?
        .handle;
    let whandle = sftp
        .open(
            dst,
            OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
            FileAttributes::default(),
        )
        .await
        .map_err(|e| SshError::Channel(format!("打开目标文件 {dst} 失败: {e}")))?
        .handle;

    let mut offset = 0u64;
    let result = loop {
        match sftp
            .read(rhandle.as_str(), offset, DEFAULT_CHUNK as u32)
            .await
        {
            Ok(d) => {
                if d.data.is_empty() {
                    break Ok::<_, SshError>(());
                }
                let n = d.data.len() as u64;
                if let Err(e) = sftp.write(whandle.as_str(), offset, d.data).await {
                    break Err(SshError::Channel(format!("写入目标 {dst} 失败: {e}")));
                }
                offset += n;
            }
            Err(SftpError::Status(status)) if status.status_code == StatusCode::Eof => break Ok(()),
            Err(e) => break Err(SshError::Channel(format!("读取源 {src} 失败: {e}"))),
        }
    };
    let _ = sftp.close(rhandle.as_str()).await;
    let _ = sftp.close(whandle.as_str()).await;
    result
}

/// 递归复制：软链 → readlink 后在目标位置建同指向软链（不跟随）；
/// 目录 → mkdir 后逐项递归；文件 → 同会话流式复制。
async fn copy_recursive(sftp: &RawSftpSession, src: &str, dst: &str) -> Result<(), SshError> {
    // lstat 不跟随软链，保证链接复制为链接
    let attrs = sftp
        .lstat(src)
        .await
        .map_err(|e| SshError::Channel(format!("获取 {src} 属性失败: {e}")))?
        .attrs;
    let ftype = attrs.file_type();
    if ftype.is_symlink() {
        let target = sftp
            .readlink(src)
            .await
            .map_err(|e| SshError::Channel(format!("读取链接 {src} 失败: {e}")))?
            .files
            .first()
            .map(|f| f.filename.clone())
            .ok_or_else(|| SshError::Channel(format!("读取链接 {src} 失败：服务端未返回目标")))?;
        sftp.symlink(dst, &target)
            .await
            .map_err(|e| SshError::Channel(format!("创建链接 {dst} 失败: {e}")))?;
    } else if ftype.is_dir() {
        sftp.mkdir(dst, FileAttributes::default())
            .await
            .map_err(|e| SshError::Channel(format!("创建目录 {dst} 失败: {e}")))?;
        for (name, _) in list_children(sftp, src).await? {
            Box::pin(copy_recursive(
                sftp,
                &join_child(src, &name),
                &join_child(dst, &name),
            ))
            .await?;
        }
    } else {
        copy_file_stream(sftp, src, dst).await?;
    }
    Ok(())
}

/// 单任务令牌桶限速：按 `rate_kb`（KB/s）节流，0 表示不限速。
/// 采用按块匀速节流：每块按"应有耗时"累加虚拟发送时刻，超前则睡眠补足。
/// 相比固定 1 秒窗口（攒满一秒量后长睡一次），不会产生"全速→长睡"的锯齿，
/// 进度上报的瞬时速率因此保持平稳，也更接近设定的平均速率。
struct RateLimiter {
    rate_bps: u64,
    /// 下一块最早可发送的时刻；落后于当前时刻时从当前时刻重新起算（允许追赶）
    next_free: Option<Instant>,
}
impl RateLimiter {
    fn new(rate_kb: u64) -> Self {
        Self {
            rate_bps: rate_kb.saturating_mul(1024),
            next_free: None,
        }
    }
    async fn allow(&mut self, bytes: u64) {
        if self.rate_bps == 0 || bytes == 0 {
            return;
        }
        // 该块按目标速率应占用的时长（纳秒）；bytes 上限 1MB，乘法不会溢出
        let cost = Duration::from_nanos(bytes.saturating_mul(1_000_000_000) / self.rate_bps);
        let now = Instant::now();
        let start = match self.next_free {
            Some(t) if t > now => t,
            _ => now,
        };
        self.next_free = Some(start + cost);
        if start > now {
            sleep(start - now).await;
        }
    }
}

/// 断点续传补算：把本地文件前 `len` 字节增量喂入 hasher（不整文件读入内存）。
async fn feed_local_prefix(hasher: &mut Sha256, path: &str, len: u64) -> Result<(), SshError> {
    if len == 0 {
        return Ok(());
    }
    let mut f = File::open(path)
        .await
        .map_err(|e| SshError::Channel(format!("补算前缀哈希时打开本地文件 {path} 失败: {e}")))?;
    let mut remaining = len;
    let mut buf = vec![0u8; DEFAULT_CHUNK];
    while remaining > 0 {
        let want = remaining.min(buf.len() as u64) as usize;
        let n = f
            .read(&mut buf[..want])
            .await
            .map_err(|e| SshError::Channel(format!("补算前缀哈希时读取本地文件失败: {e}")))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        remaining -= n as u64;
    }
    Ok(())
}

/// 断点续传补算：把远端文件前 `len` 字节增量喂入 hasher（SFTP 分块读取）。
async fn feed_remote_prefix(
    sftp: &RawSftpSession,
    path: &str,
    len: u64,
    hasher: &mut Sha256,
) -> Result<(), SshError> {
    if len == 0 {
        return Ok(());
    }
    let rhandle = sftp
        .open(path, OpenFlags::READ, FileAttributes::default())
        .await
        .map_err(|e| SshError::Channel(format!("补算前缀哈希时打开远端文件 {path} 失败: {e}")))?
        .handle;
    let mut off = 0u64;
    while off < len {
        let want = (len - off).min(DEFAULT_CHUNK as u64) as u32;
        match sftp.read(rhandle.as_str(), off, want).await {
            Ok(d) => {
                if d.data.is_empty() {
                    break;
                }
                hasher.update(&d.data);
                off += d.data.len() as u64;
            }
            Err(SftpError::Status(status)) if status.status_code == StatusCode::Eof => break,
            Err(e) => {
                let _ = sftp.close(rhandle.as_str()).await;
                return Err(SshError::Channel(format!(
                    "补算前缀哈希时读取远端文件失败: {e}"
                )));
            }
        }
    }
    let _ = sftp.close(rhandle.as_str()).await;
    Ok(())
}

/// 严格校验：确认本地源文件在传输期间未被修改（大小 + 秒级 mtime）。
async fn check_local_unchanged(path: &str, size: u64, mtime: u64) -> Result<(), SshError> {
    let meta = tokio::fs::metadata(path)
        .await
        .map_err(|e| SshError::Channel(format!("校验源文件状态失败: {e}")))?;
    let now_size = meta.len();
    let now_mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if now_size != size || now_mtime != mtime {
        log::warn!(
            "源文件在传输期间已修改（size {size} → {now_size}, mtime {mtime} → {now_mtime}），按完整性校验失败处理"
        );
        return Err(SshError::TransferRejected(
            "文件完整性校验失败".to_string(),
        ));
    }
    Ok(())
}

/// 严格校验：确认远端源文件在传输期间未被修改（大小 + 秒级 mtime）。
async fn check_remote_unchanged(
    sftp: &RawSftpSession,
    path: &str,
    size: u64,
    mtime: u64,
) -> Result<(), SshError> {
    let attrs = sftp
        .lstat(path)
        .await
        .map_err(|e| SshError::Channel(format!("校验远端源文件状态失败: {e}")))?
        .attrs;
    let now_size = attrs.size.unwrap_or(0);
    let now_mtime = attrs.mtime.unwrap_or(0) as u64;
    if now_size != size || now_mtime != mtime {
        log::warn!(
            "远端源文件在传输期间已修改（size {size} → {now_size}, mtime {mtime} → {now_mtime}），按完整性校验失败处理"
        );
        return Err(SshError::TransferRejected(
            "文件完整性校验失败".to_string(),
        ));
    }
    Ok(())
}

/// 计算远端完整文件 SHA256（小写十六进制），用于传输完成后的完整性校验。
async fn remote_file_sha256(sftp: &RawSftpSession, path: &str) -> Result<String, SshError> {
    let rhandle = sftp
        .open(path, OpenFlags::READ, FileAttributes::default())
        .await
        .map_err(|e| SshError::Channel(format!("校验时打开远端文件 {path} 失败: {e}")))?
        .handle;
    let mut hasher = Sha256::new();
    let mut off = 0u64;
    loop {
        match sftp.read(rhandle.as_str(), off, DEFAULT_CHUNK as u32).await {
            Ok(d) => {
                if d.data.is_empty() {
                    break;
                }
                hasher.update(&d.data);
                off += d.data.len() as u64;
            }
            Err(SftpError::Status(status)) if status.status_code == StatusCode::Eof => break,
            Err(e) => {
                let _ = sftp.close(rhandle.as_str()).await;
                return Err(SshError::Channel(format!("校验时读取远端文件失败: {e}")));
            }
        }
    }
    let _ = sftp.close(rhandle.as_str()).await;
    Ok(format!("{:x}", hasher.finalize()))
}

/// 断点续传 sha256 校验：比对远端前 `len` 字节与本地文件前 `len` 字节的哈希，
/// 一致才认为已有半截属于同一源文件可续写。任一读取失败 / 长度不足保守返回 false
/// （触发截断重传）。调用方应确保 `len <= 32MB`（大文件退化为 sizeMtime），避免长耗时。
async fn verify_prefix_sha256(
    sftp: &RawSftpSession,
    remote_path: &str,
    local_path: &str,
    len: u64,
) -> bool {
    if len == 0 {
        return true;
    }
    // 读远端前 len 字节
    let mut remote_hash = Sha256::new();
    let rhandle = match sftp
        .open(remote_path, OpenFlags::READ, FileAttributes::default())
        .await
    {
        Ok(h) => h.handle,
        Err(e) => {
            debug!("sha256 校验打开远端失败，降级截断重传: {e}");
            return false;
        }
    };
    let mut off = 0u64;
    let mut ok = true;
    while off < len {
        let want = ((len - off) as usize).min(DEFAULT_CHUNK) as u32;
        match sftp.read(rhandle.as_str(), off, want).await {
            Ok(d) => {
                if d.data.is_empty() {
                    break;
                }
                remote_hash.update(&d.data);
                off += d.data.len() as u64;
            }
            Err(SftpError::Status(status)) if status.status_code == StatusCode::Eof => break,
            Err(e) => {
                debug!("sha256 校验读取远端失败，降级截断重传: {e}");
                ok = false;
                break;
            }
        }
    }
    let _ = sftp.close(rhandle.as_str()).await;
    if !ok || off != len {
        return false;
    }

    // 读本地前 len 字节（独立打开，不干扰调用方已打开的文件句柄位置）
    let mut local = match File::open(local_path).await {
        Ok(f) => f,
        Err(e) => {
            debug!("sha256 校验打开本地失败，降级截断重传: {e}");
            return false;
        }
    };
    let mut local_hash = Sha256::new();
    let mut remaining = len;
    let mut buf = vec![0u8; DEFAULT_CHUNK.min(remaining as usize)];
    while remaining > 0 {
        let want = buf.len().min(remaining as usize);
        match local.read(&mut buf[..want]).await {
            Ok(0) => break,
            Ok(n) => {
                local_hash.update(&buf[..n]);
                remaining -= n as u64;
            }
            Err(e) => {
                debug!("sha256 校验读取本地失败，降级截断重传: {e}");
                return false;
            }
        }
    }
    if remaining != 0 {
        return false;
    }
    remote_hash.finalize() == local_hash.finalize()
}

/// 下载完成后将本地文件的 mtime / 权限同步为远端值（preserve_meta 开启时）。
/// 失败仅 debug 日志，不影响传输结果。
fn preserve_local_meta(local_path: &str, mtime: u64, perm: Option<u32>) {
    if mtime != 0 {
        let ft = FileTime::from_unix_time(mtime as i64, 0);
        if let Err(e) = filetime::set_file_mtime(local_path, ft) {
            debug!("设置本地文件 mtime 失败（不影响传输结果）: {e}");
        }
    }
    #[cfg(unix)]
    if let Some(mode) = perm {
        use std::os::unix::fs::PermissionsExt;
        let p = std::fs::Permissions::from_mode(mode);
        if let Err(e) = std::fs::set_permissions(local_path, p) {
            debug!("设置本地文件权限失败（不影响传输结果）: {e}");
        }
    }
}

/// 展开路径开头的 ~ 为当前用户家目录（不引入 shellexpand 依赖）
fn expand_tilde(path: &str) -> String {
    if path == "~" || path.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{}{}", home, &path[1..]);
        }
    }
    path.to_string()
}

/// 经 IPC Channel 流式推送传输进度（前端 onmessage 消费）；
/// 通道关闭时仅记 debug，不中断传输（invoke 本身仍由前端 await）。
fn emit_progress(
    channel: &Channel<TransferProgress>,
    task_id: u64,
    transferred: u64,
    total: u64,
    speed: u64,
    done: bool,
    sha256: Option<&str>,
) {
    send_progress(
        channel,
        task_id,
        transferred,
        total,
        speed,
        done,
        sha256,
        false,
        false,
    );
}

/// 校验过渡包：字节已传完、进入 SHA256 比对阶段（前端据此把状态切到"校验中"）
fn emit_verifying(channel: &Channel<TransferProgress>, task_id: u64, total: u64) {
    send_progress(channel, task_id, total, total, 0, false, None, false, true);
}

/// 收尾包：除常规字段外附带"按覆盖策略跳过"标记，前端据此展示"已跳过"
fn emit_final_progress(
    channel: &Channel<TransferProgress>,
    task_id: u64,
    transferred: u64,
    total: u64,
    sha256: Option<&str>,
    skipped: bool,
) {
    send_progress(
        channel,
        task_id,
        transferred,
        total,
        0,
        true,
        sha256,
        skipped,
        false,
    );
}

#[allow(clippy::too_many_arguments)]
fn send_progress(
    channel: &Channel<TransferProgress>,
    task_id: u64,
    transferred: u64,
    total: u64,
    speed: u64,
    done: bool,
    sha256: Option<&str>,
    skipped: bool,
    verifying: bool,
) {
    let payload = TransferProgress {
        task_id,
        transferred,
        total,
        speed,
        done,
        skipped,
        verifying,
        sha256: sha256.map(str::to_string),
    };
    if let Err(e) = channel.send(payload) {
        debug!("发送 SFTP 进度（Channel 已关闭）: {e}");
    }
}

/// 若任务处于暂停态则阻塞，直到被恢复或取消；取消时返回 [`SshError::Cancelled`]
async fn wait_if_paused(
    paused_rx: &mut watch::Receiver<bool>,
    cancel: &CancellationToken,
) -> Result<(), SshError> {
    if !*paused_rx.borrow() {
        return Ok(());
    }
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Err(SshError::Cancelled),
            changed = paused_rx.changed() => {
                if changed.is_err() {
                    return Err(SshError::Cancelled);
                }
                if !*paused_rx.borrow() {
                    return Ok(());
                }
            }
        }
    }
}
