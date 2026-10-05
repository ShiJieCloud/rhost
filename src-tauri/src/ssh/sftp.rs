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
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::TimeZone;
use log::debug;
use russh::client;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::rawsession::RawSftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use serde::Serialize;
use tauri::ipc::Channel;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::watch;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::session::{ClientHandler, SshSession};
use super::SshError;

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
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|e| {
                SshError::Channel(format!("远端拒绝 SFTP 子系统（sftp-server 可能未启用）: {e}"))
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
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferProgress {
    pub task_id: u64,
    pub transferred: u64,
    pub total: u64,
    pub speed: u64,
    pub done: bool,
}

/// 每会话的 SFTP 长驻会话缓存：懒加载，坏通道可失效重建；
/// 同时持有该会话所有进行中的传输任务控制柄。
pub(super) struct SftpState {
    inner: Mutex<Option<Arc<RawSftpSession>>>,
    tasks: Mutex<HashMap<u64, Arc<TransferCtl>>>,
    /// 文件管理操作（删除/重命名/复制等多请求串行操作）互斥锁，
    /// 避免递归遍历期间目录被另一个管理操作改动
    manage: Mutex<()>,
}

impl SftpState {
    pub(super) fn new() -> Self {
        Self {
            inner: Mutex::new(None),
            tasks: Mutex::new(HashMap::new()),
            manage: Mutex::new(()),
        }
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
    }

    /// 取缓存的 RawSftpSession；不存在则开新通道初始化。
    /// 持锁建链，并发列目录只在首次/重建时竞争，之后共享同一协议会话。
    async fn get(
        &self,
        handle: &Arc<Mutex<client::Handle<ClientHandler>>>,
    ) -> Result<Arc<RawSftpSession>, SshError> {
        let mut guard = self.inner.lock().await;
        if let Some(s) = guard.as_ref() {
            return Ok(s.clone());
        }
        let s = Arc::new(open_sftp(handle).await?);
        *guard = Some(s.clone());
        Ok(s)
    }

    /// 丢弃缓存的 SFTP 会话（通道异常后调用，下次 get 重新建立）
    async fn invalidate(&self) {
        *self.inner.lock().await = None;
    }
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

    async fn list_remote(
        &self,
        path: &Option<String>,
    ) -> Result<RemoteDirListing, SshError> {
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
                    Err(SftpError::Status(status))
                        if status.status_code == StatusCode::Eof =>
                    {
                        break Ok::<_, SshError>(())
                    }
                    Err(e) => {
                        break Err(SshError::Channel(format!("读取远端目录 {dir} 失败: {e}")))
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
    /// IPC [`Channel`] 流式推送，支持暂停/取消。**断点续传**（`resume` 开启时）：
    /// 若远端已存在同名文件且小于本地，从已有偏移处续写（不截断）；远端已完整则
    /// 直接跳过；远端大于本地（源文件变更）时截断重传。`resume` 关闭时一律全量重传。
    pub(crate) async fn upload_file(
        &self,
        task_id: u64,
        local_path: String,
        remote_path: String,
        chunk_kb: Option<u32>,
        resume: Option<bool>,
        channel: Channel<TransferProgress>,
    ) -> Result<(), SshError> {
        let chunk = chunk_kb.unwrap_or((DEFAULT_CHUNK / 1024) as u32).clamp(8, 1024) as usize * 1024;
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

        // async block 返回 Result<(已传, 总大小), (错误, 已传, 总大小)>，
        // 这样内部可用 ?，且错误路径也能拿到已传字节用于收尾进度
        let outcome: Result<(u64, u64), (SshError, u64, u64)> = async {
            let sftp = self
                .sftp
                .get(&self.handle)
                .await
                .map_err(|e| (e, 0u64, 0u64))?;
            let local = expand_tilde(&local_path);
            let mut src = File::open(&local)
                .await
                .map_err(|e| (SshError::Channel(format!("打开本地文件 {local} 失败: {e}")), 0, 0))?;
            let total = src
                .metadata()
                .await
                .map_err(|e| (SshError::Channel(format!("读取本地文件大小失败: {e}")), 0, 0))?
                .len();

            // 断点续传：探测远端已有大小，决定续写起点
            let remote_size = match sftp.lstat(&remote_path).await {
                Ok(meta) => meta.attrs.size.unwrap_or(0),
                Err(SftpError::Status(s)) if s.status_code == StatusCode::NoSuchFile => 0,
                Err(e) => {
                    return Err((
                        SshError::Channel(format!("探测远端文件 {remote_path} 失败: {e}")),
                        0,
                        total,
                    ))
                }
            };
            let offset = if !resume {
                0 // 关闭断点续传：一律截断全量重传
            } else if remote_size == total {
                return Ok((total, total)); // 已完整，直接完成
            } else if remote_size < total {
                remote_size // 从已有偏移续写
            } else {
                0 // 远端比本地大（源文件变更），截断重传
            };
            if offset > 0 {
                src.seek(SeekFrom::Start(offset))
                    .await
                    .map_err(|e| (SshError::Channel(format!("定位本地文件偏移失败: {e}")), 0, total))?;
            }

            // 续传时不带 TRUNCATE 保留已有内容；从头传时 TRUNCATE 清除残留
            let flags = if offset > 0 {
                OpenFlags::WRITE | OpenFlags::CREATE
            } else {
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE
            };
            let rhandle = sftp
                .open(&remote_path, flags, FileAttributes::default())
                .await
                .map_err(|e| {
                    (
                        SshError::Channel(format!("打开远端文件 {remote_path} 失败: {e}")),
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
                emit_progress(&channel, task_id, transferred, total, 0, false);
            }

            loop {
                wait_if_paused(&mut paused_rx, &cancel)
                    .await
                    .map_err(|e| (e, transferred, total))?;
                if cancel.is_cancelled() {
                    return Err((SshError::Cancelled, transferred, total));
                }
                let n = src
                    .read(&mut buf)
                    .await
                    .map_err(|e| {
                        (
                            SshError::Channel(format!("读取本地文件失败: {e}")),
                            transferred,
                            total,
                        )
                    })?;
                if n == 0 {
                    return Ok((transferred, total));
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
                offset += n as u64;
                transferred += n as u64;

                let now = Instant::now();
                if now.duration_since(last_emit) >= PROGRESS_INTERVAL {
                    let speed = ((transferred - last_bytes) * 1000)
                        / now.duration_since(last_emit).as_millis().max(1) as u64;
                    emit_progress(&channel, task_id, transferred, total, speed, false);
                    last_emit = now;
                    last_bytes = transferred;
                }
            }
        }
        .await;

        let (result, transferred, total) = match outcome {
            Ok((t, total)) => (Ok(()), t, total),
            Err((e, t, total)) => (Err(e), t, total),
        };

        self.sftp.remove_task(task_id).await;
        emit_progress(&channel, task_id, transferred, total, 0, true);
        result
    }

    /// 单文件下载：远端 → 本地。本地父目录不存在时自动创建；分块大小由
    /// `chunk_kb` 指定（缺省 64KB），进度经 IPC [`Channel`] 流式推送，支持暂停/取消。
    /// **断点续传**（`resume` 开启时）：若本地已有同名文件且小于远端，从已有偏移
    /// 处续写（APPEND）；本地已完整则直接跳过；本地大于远端（源文件变更）时截断
    /// 重传。`resume` 关闭时一律全量重传。
    pub(crate) async fn download_file(
        &self,
        task_id: u64,
        remote_path: String,
        local_path: String,
        chunk_kb: Option<u32>,
        resume: Option<bool>,
        channel: Channel<TransferProgress>,
    ) -> Result<(), SshError> {
        let chunk = chunk_kb.unwrap_or((DEFAULT_CHUNK / 1024) as u32).clamp(8, 1024) as usize * 1024;
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

        let outcome: Result<(u64, u64), (SshError, u64, u64)> = async {
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

            // 断点续传：探测本地已有大小，决定续写起点
            let local_size = tokio::fs::metadata(&local).await.map(|m| m.len()).unwrap_or(0);
            let offset = if !resume {
                0 // 关闭断点续传：一律截断全量重传
            } else if local_size == total {
                return Ok((total, total)); // 已完整，直接完成
            } else if local_size < total {
                local_size // 从已有偏移续写
            } else {
                0 // 本地比远端大（源文件变更），截断重传
            };
            // 续传用 APPEND 保留已有内容；从头传用 CREATE 截断
            let mut dst = if offset > 0 {
                OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(&local)
                    .await
                    .map_err(|e| {
                        (
                            SshError::Channel(format!("打开本地文件 {local} 续写失败: {e}")),
                            0,
                            total,
                        )
                    })?
            } else {
                File::create(&local).await.map_err(|e| {
                    (
                        SshError::Channel(format!("创建本地文件 {local} 失败: {e}")),
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
                emit_progress(&channel, task_id, transferred, total, 0, false);
            }

            loop {
                wait_if_paused(&mut paused_rx, &cancel)
                    .await
                    .map_err(|e| (e, transferred, total))?;
                if cancel.is_cancelled() {
                    return Err((SshError::Cancelled, transferred, total));
                }
                // russh-sftp 的 read 在 EOF 时返回 Status(Eof) 错误，而非空数据
                let read_res = sftp.read(rhandle.as_str(), offset, chunk as u32).await;
                let data = match read_res {
                    Ok(d) => d.data,
                    Err(SftpError::Status(status)) if status.status_code == StatusCode::Eof => {
                        return Ok((transferred, total));
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
                    return Ok((transferred, total));
                }
                dst.write_all(&data)
                    .await
                    .map_err(|e| {
                        (
                            SshError::Channel(format!("写入本地文件失败: {e}")),
                            transferred,
                            total,
                        )
                    })?;
                offset += data.len() as u64;
                transferred += data.len() as u64;

                let now = Instant::now();
                if now.duration_since(last_emit) >= PROGRESS_INTERVAL {
                    let speed = ((transferred - last_bytes) * 1000)
                        / now.duration_since(last_emit).as_millis().max(1) as u64;
                    emit_progress(&channel, task_id, transferred, total, speed, false);
                    last_emit = now;
                    last_bytes = transferred;
                }
            }
        }
        .await;

        let (result, transferred, total) = match outcome {
            Ok((t, total)) => (Ok(()), t, total),
            Err((e, t, total)) => (Err(e), t, total),
        };

        self.sftp.remove_task(task_id).await;
        emit_progress(&channel, task_id, transferred, total, 0, true);
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
    pub(crate) async fn sftp_copy_path(
        &self,
        src: String,
        dst: String,
    ) -> Result<(), SshError> {
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
                break Ok::<_, SshError>(())
            }
            Err(e) => {
                break Err(SshError::Channel(format!("读取目录 {dir} 失败: {e}")))
            }
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

/// 单文件同会话流式复制：固定 64KB read→write，两端句柄用完即关，
/// 不落地本地磁盘、不占整文件内存。
async fn copy_file_stream(
    sftp: &RawSftpSession,
    src: &str,
    dst: &str,
) -> Result<(), SshError> {
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
        match sftp.read(rhandle.as_str(), offset, DEFAULT_CHUNK as u32).await {
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
            Err(SftpError::Status(status)) if status.status_code == StatusCode::Eof => {
                break Ok(())
            }
            Err(e) => break Err(SshError::Channel(format!("读取源 {src} 失败: {e}"))),
        }
    };
    let _ = sftp.close(rhandle.as_str()).await;
    let _ = sftp.close(whandle.as_str()).await;
    result
}

/// 递归复制：软链 → readlink 后在目标位置建同指向软链（不跟随）；
/// 目录 → mkdir 后逐项递归；文件 → 同会话流式复制。
async fn copy_recursive(
    sftp: &RawSftpSession,
    src: &str,
    dst: &str,
) -> Result<(), SshError> {
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
) {
    let payload = TransferProgress {
        task_id,
        transferred,
        total,
        speed,
        done,
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
