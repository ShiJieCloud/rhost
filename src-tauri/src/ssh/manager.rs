//! 会话管理器：统一管理所有存活 SSH 会话的生命周期。

use std::collections::HashMap;
use std::sync::Arc;

use log::debug;
use tokio::sync::{RwLock, mpsc};
use uuid::Uuid;

use super::session::SshSession;
use super::sftp::{RemoteDirListing, TransferProgress};
use super::{SessionConfig, SshError};
use tauri::ipc::Channel;

/// 会话池：session_id (uuid v4) → 会话句柄
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<String, Arc<SshSession>>>>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 建立新会话：连接成功后注册进会话池，返回 (session_id, 帧接收端)。
    /// 连接阶段不持锁，避免慢网络阻塞其他会话的读写。
    pub async fn create(
        &self,
        cfg: SessionConfig,
    ) -> Result<(String, mpsc::Receiver<Vec<u8>>), SshError> {
        let (session, frame_rx) = SshSession::connect(cfg).await?;
        let id = Uuid::new_v4().to_string();
        self.sessions
            .write()
            .await
            .insert(id.clone(), Arc::new(session));
        debug!("会话注册: {id}");
        Ok((id, frame_rx))
    }

    /// 向指定会话写入前端输入（复制 Arc 后释放读锁，不持锁 await）
    pub async fn write(&self, session_id: &str, data: Vec<u8>) -> Result<(), SshError> {
        let session = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).cloned()
        };
        match session {
            Some(s) => s.write(data).await,
            None => Err(SshError::NotFound),
        }
    }

    /// 同步 PTY 尺寸：转发 window-change（复制 Arc 后释放读锁）
    pub async fn resize(&self, session_id: &str, cols: u32, rows: u32) -> Result<(), SshError> {
        let session = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).cloned()
        };
        match session {
            Some(s) => s.resize(cols, rows).await,
            None => Err(SshError::NotFound),
        }
    }

    /// 启动某会话的动态指标采集（幂等，间隔由前端指定，单位毫秒；
    /// `iface` 为默认路由网卡，网络计数优先取它）
    pub async fn start_metrics(
        &self,
        session_id: &str,
        interval_ms: u64,
        iface: Option<String>,
    ) -> Result<(), SshError> {
        let session = { self.sessions.read().await.get(session_id).cloned() };
        match session {
            Some(s) => {
                s.start_metrics(interval_ms, iface);
                Ok(())
            }
            None => Err(SshError::NotFound),
        }
    }

    /// 停止某会话的动态指标采集
    pub async fn stop_metrics(&self, session_id: &str) -> Result<(), SshError> {
        let session = { self.sessions.read().await.get(session_id).cloned() };
        match session {
            Some(s) => {
                s.stop_metrics();
                Ok(())
            }
            None => Err(SshError::NotFound),
        }
    }

    /// 前端心跳续约，维持指标采集存活
    pub async fn metrics_heartbeat(&self, session_id: &str) -> Result<(), SshError> {
        let session = { self.sessions.read().await.get(session_id).cloned() };
        match session {
            Some(s) => {
                s.metrics_heartbeat();
                Ok(())
            }
            None => Err(SshError::NotFound),
        }
    }

    /// SFTP 列举远端目录：path 为 None 时返回登录默认目录（远端家目录）
    pub(crate) async fn sftp_list_dir(
        &self,
        session_id: &str,
        path: Option<String>,
    ) -> Result<RemoteDirListing, SshError> {
        let session = { self.sessions.read().await.get(session_id).cloned() };
        match session {
            Some(s) => s.sftp_list_dir(path).await,
            None => Err(SshError::NotFound),
        }
    }

    /// SFTP 创建目录
    pub(crate) async fn sftp_mkdir(
        &self,
        session_id: &str,
        path: String,
    ) -> Result<(), SshError> {
        let session = { self.sessions.read().await.get(session_id).cloned() };
        match session {
            Some(s) => s.sftp_mkdir(path).await,
            None => Err(SshError::NotFound),
        }
    }

    /// 上传单个文件：本地路径 → 远端路径（同名覆盖），进度经事件推送
    pub(crate) async fn sftp_upload(
        &self,
        session_id: &str,
        task_id: u64,
        local_path: String,
        remote_path: String,
        chunk_kb: Option<u32>,
        resume: Option<bool>,
        channel: Channel<TransferProgress>,
    ) -> Result<(), SshError> {
        let session = { self.sessions.read().await.get(session_id).cloned() };
        match session {
            Some(s) => s.upload_file(task_id, local_path, remote_path, chunk_kb, resume, channel).await,
            None => Err(SshError::NotFound),
        }
    }

    /// 下载单个文件：远端路径 → 本地路径（本地父目录自动创建）
    pub(crate) async fn sftp_download(
        &self,
        session_id: &str,
        task_id: u64,
        remote_path: String,
        local_path: String,
        chunk_kb: Option<u32>,
        resume: Option<bool>,
        channel: Channel<TransferProgress>,
    ) -> Result<(), SshError> {
        let session = { self.sessions.read().await.get(session_id).cloned() };
        match session {
            Some(s) => s.download_file(task_id, remote_path, local_path, chunk_kb, resume, channel).await,
            None => Err(SshError::NotFound),
        }
    }

    /// SFTP → Shell 方向同步：让远端交互式 shell `cd` 到指定目录
    pub(crate) async fn sftp_sync_cwd(
        &self,
        session_id: &str,
        path: String,
    ) -> Result<(), SshError> {
        let session = { self.sessions.read().await.get(session_id).cloned() };
        match session {
            Some(s) => s.sync_shell_cd(&path).await,
            None => Err(SshError::NotFound),
        }
    }

    /// SFTP 删除文件/目录（目录递归）
    pub(crate) async fn sftp_remove(
        &self,
        session_id: &str,
        path: String,
    ) -> Result<(), SshError> {
        let session = { self.sessions.read().await.get(session_id).cloned() };
        match session {
            Some(s) => s.sftp_remove_path(path).await,
            None => Err(SshError::NotFound),
        }
    }

    /// SFTP 重命名/移动
    pub(crate) async fn sftp_rename(
        &self,
        session_id: &str,
        old_path: String,
        new_path: String,
    ) -> Result<(), SshError> {
        let session = { self.sessions.read().await.get(session_id).cloned() };
        match session {
            Some(s) => s.sftp_rename_path(old_path, new_path).await,
            None => Err(SshError::NotFound),
        }
    }

    /// SFTP 创建空文件
    pub(crate) async fn sftp_create_file(
        &self,
        session_id: &str,
        path: String,
    ) -> Result<(), SshError> {
        let session = { self.sessions.read().await.get(session_id).cloned() };
        match session {
            Some(s) => s.sftp_create_file(path).await,
            None => Err(SshError::NotFound),
        }
    }

    /// SFTP 同主机复制（目录递归）
    pub(crate) async fn sftp_copy(
        &self,
        session_id: &str,
        src: String,
        dst: String,
    ) -> Result<(), SshError> {
        let session = { self.sessions.read().await.get(session_id).cloned() };
        match session {
            Some(s) => s.sftp_copy_path(src, dst).await,
            None => Err(SshError::NotFound),
        }
    }

    /// SFTP 读取符号链接目标
    pub(crate) async fn sftp_readlink(
        &self,
        session_id: &str,
        path: String,
    ) -> Result<String, SshError> {
        let session = { self.sessions.read().await.get(session_id).cloned() };
        match session {
            Some(s) => s.sftp_readlink(path).await,
            None => Err(SshError::NotFound),
        }
    }

    /// 暂停/继续某传输任务
    pub(crate) async fn sftp_transfer_pause(
        &self,
        session_id: &str,
        task_id: u64,
        paused: bool,
    ) -> Result<(), SshError> {
        let session = { self.sessions.read().await.get(session_id).cloned() };
        match session {
            Some(s) => s.transfer_pause(task_id, paused).await,
            None => Err(SshError::NotFound),
        }
    }

    /// 取消某传输任务
    pub(crate) async fn sftp_transfer_cancel(
        &self,
        session_id: &str,
        task_id: u64,
    ) -> Result<(), SshError> {
        let session = { self.sessions.read().await.get(session_id).cloned() };
        match session {
            Some(s) => s.transfer_cancel(task_id).await,
            None => Err(SshError::NotFound),
        }
    }

    /// 断开并移除会话：触发取消令牌，后台任务自行收尾退出
    pub async fn disconnect(&self, session_id: &str) -> bool {
        let session = self.sessions.write().await.remove(session_id);
        if let Some(s) = session {
            debug!("会话注销: {session_id}");
            s.shutdown();
            true
        } else {
            false
        }
    }
}
