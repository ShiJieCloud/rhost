//! SSH 模块：纯 tokio 异步实现，不依赖 tauri。
//!
//! 分层职责：
//! - [`frame`]：自定义二进制帧编解码（1 字节类型 + 4 字节大端长度 + payload）
//! - [`session`]：russh 连接、密码认证、PTY 读写、小包合并
//! - [`manager`]：会话池（Arc<RwLock<HashMap>> + CancellationToken 生命周期管理）

pub mod frame;
pub mod manager;
pub mod session;
pub mod sftp;

pub use manager::SessionManager;

/// 认证方式：一键连接仅支持密码与公钥两种，其余（agent/OTP）暂不支持
#[derive(Debug, Clone)]
pub enum AuthMethod {
    /// 密码认证
    Password(String),
    /// 公钥认证：私钥路径（允许 ~ 前缀）+ 私钥口令（无口令为空串）
    Key { path: String, passphrase: String },
}

/// 建立一条 SSH PTY 会话所需的连接参数
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: AuthMethod,
    /// PTY 初始列数（终端宽度）
    pub cols: u32,
    /// PTY 初始行数（终端高度）
    pub rows: u32,
    /// 连接成功后是否绘制 Rhost MOTD 欢迎面板（本地采集服务器状态并注入终端）。
    /// 开启时同时抑制 sshd 原生 MOTD 与 Last Login（以 exec 路径启动登录 shell，
    /// sshd 在该路径通常不打印原生横幅），避免与 Rhost MOTD 双重横幅；
    /// 关闭时走标准 shell 请求，放行并叠加服务器原生输出。
    pub motd: bool,
    /// 开启彩色提示符注入（后端自动注入，无需前端 gate）。
    pub color_prompt: bool,
    /// 登录后经 init 脚本 export 注入远端交互 shell 的环境变量。
    /// source 在当前 shell 执行，export 直接生效，且不受 sshd AcceptEnv
    /// 默认限制（AcceptEnv 只放行 LANG/LC_*，SSH env 请求方式会被静默拒绝）。
    pub env: Vec<(String, String)>,
    /// 用户自定义 MOTD ASCII LOGO（多行文本）；空串使用内置 LOGO
    pub motd_logo: String,
}

/// SSH 模块统一错误类型（ipc 层转字符串返回前端）
#[derive(Debug, thiserror::Error)]
pub enum SshError {
    #[error("连接失败: {0}")]
    Connect(String),
    #[error("认证失败: {0}")]
    Auth(String),
    /// 私钥已加密：无口令加载失败或口令错误（russh-keys 对两者都报 KeyEncrypted）。
    /// 前缀 KEY_ENCRYPTED 是前后端约定的标记，前端据此弹口令框后重试
    #[error("KEY_ENCRYPTED: 私钥已加密，需要口令（或口令错误）")]
    KeyEncrypted,
    #[error("通道错误: {0}")]
    Channel(String),
    #[error("会话不存在或已关闭")]
    NotFound,
    #[error("会话已关闭")]
    Closed,
    #[error("传输已取消")]
    Cancelled,
}
