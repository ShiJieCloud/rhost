//! SSH 交互式 PTY 会话：russh 纯异步实现，禁止 spawn_blocking。
//!
//! 数据流：
//! ```text
//! russh ChannelReadHalf.wait()            前端键盘输入
//!   │  (PTY 原始字节 / 控制消息)                │ Vec<u8>
//!   ▼                                          ▼
//! PtyEvent 通道 (mpsc) ──► merge_task    write_task ──► ChannelWriteHalf.data_bytes()
//!   (4KB / 5ms 攒包)                        (直接透传)
//!   │
//!   ▼ 二进制帧 (1B 类型 + 4B 长度 + payload)
//! frame_rx ──► ipc 层转发到 Tauri Channel<Vec<u8>>
//! ```

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::BytesMut;
use log::{debug, warn};
use russh::client;
use russh::{ChannelMsg, Disconnect};
use tokio::sync::{Mutex, Semaphore, mpsc};
use tokio::time::{self, Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::frame::{FrameType, encode_frame};
use super::sftp::SftpState;
use super::{AuthMethod, SessionConfig, SshError};

/// 小包合并阈值：攒够 4KB 立即发帧
const MERGE_BYTES: usize = 4096;
/// 小包合并窗口：距本批首字节 5ms 仍未攒满也发帧
const MERGE_WINDOW: Duration = Duration::from_millis(5);
/// PTY 事件通道容量（背压：满了则 read_task 暂停读取，由 TCP 窗口自然限流）
const EVENT_QUEUE: usize = 128;
/// 帧输出通道容量
const FRAME_QUEUE: usize = 128;
/// 前端输入通道容量（键盘输入量小）
const WRITE_QUEUE: usize = 64;

/// RTT 探测周期：30s，与传输层 keepalive 同频；RTT 变化慢，更高频无收益
const RTT_INTERVAL: Duration = Duration::from_secs(30);
/// 单次 ping 等待上限：不响应 global request 的 sshd（罕见）静默跳过本轮
const RTT_TIMEOUT: Duration = Duration::from_secs(5);

/// PTY TERM 终端类型：申请 PTY 与上报状态栏共用此常量，保证两处一致
const TERM_TYPE: &str = "xterm-256color";
/// 终端字节流编码：xterm.js 仅按 UTF-8 解码，全链路透传不转码
const TERM_ENCODING: &str = "UTF-8";

/// SSH 握手真实协商出的算法（`kex_done` 回调填入，连接建立后读取上报）。
#[derive(Clone, Debug, Default)]
pub(crate) struct NegotiatedAlgo {
    /// 服务器主机密钥算法标准名（如 `ssh-ed25519`、`ecdsa-sha2-nistp256`）
    host_key: String,
    /// 对称加密算法标准名（如 `aes256-gcm@openssh.com`、`chacha20-poly1305@openssh.com`）
    cipher: String,
}

/// russh 客户端回调。
/// 当前阶段接受任意服务器主机密钥。
// TODO: 接入 known_hosts 校验，首次连接提示指纹确认
pub(crate) struct ClientHandler {
    /// 协商算法出口：russh 在 KEX 完成时经 `kex_done` 填入，Arc 共享给连接主流程
    algo: Arc<StdMutex<Option<NegotiatedAlgo>>>,
}

/// 会话初始化脚本（写入远端临时文件，由前端 source）。
///
/// 始终注入 CWD 上报钩子（保证 Shell→SFTP 目录同步与彩色提示符开关无关）；
/// 仅当 `color_prompt=true` 时才覆盖 PS1/PROMPT 并开启 ls/grep 着色，
/// 否则保留用户原有提示符配置。
///
/// 生成 init 脚本中的 export 块：值以单引号包裹（内部 `'` 按 POSIX 规则转义）。
/// 变量名必须匹配 `[A-Za-z_][A-Za-z0-9_]*`、值不得含换行（保持脚本单行结构），
/// 非法项静默跳过——该函数输出直接拼进远端执行的脚本，必须从严过滤注入面。
fn env_export_lines(env: &[(String, String)]) -> String {
    let mut out = String::new();
    for (k, v) in env {
        let valid = k.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && k.chars().skip(1).all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid || v.contains(['\n', '\r']) {
            continue;
        }
        out.push_str(&format!("export {k}='{}'\n", v.replace('\'', r"'\''")));
    }
    out
}

/// 生成远端 init 脚本内容。整体设计约束见下：
/// - 脚本经 exec 通道写临时文件后 source，绝不作为键盘输入发送——长命令经
///   PTY 回显既难看又会把 ESC 字节喂给 readline/终端导致命令被毁；
/// - 提示符仍完全由远端 shell 的 PS1/PROMPT 生成（透传架构不变）；
/// - 远端 PS1/PROMPT 已含颜色标记或用户名（`\\u`/`%n`）时跳过，不覆盖用户配置；
/// - 否则注入含用户名（`\\u@\\h`/`%n@%m`）的 PS1：`color_prompt=true` 时带颜色，
///   `color_prompt=false` 时为纯文本，避免远端默认提示符（如 `\\h:\\w\\$`）缺用户名；
/// - PS1 内嵌 ESC 字节（远端 printf 现场生成），\[ \] 标记非打印区间，
///   保证 readline 行宽计算正确；
/// - printf 清行必须在脚本**第一行**：回显行在 readline 接受命令（\r）的瞬间
///   已完整上屏，脚本越早清它，回显在屏幕上的存在时间越短；
/// - 末尾 rm -f 自删临时文件（fd 已打开，删路径不影响 source 读完）。
fn init_script(path: &str, color_prompt: bool, env: &[(String, String)]) -> String {
    // path 由后端生成（/tmp/.ri-<uuid8>），不含单引号/空格，无注入面
    let color_block = if color_prompt {
        "\
_rh_e=$(printf '\\033')
case \"$PS1$PROMPT\" in *'\\\\e['*|*'\\\\033['*|*\"$_rh_e\"*|*'%F'*|*'fg['*|*'\\\\u'*|*'%n'*) ;; *)
  if [ -n \"$ZSH_VERSION\" ]; then PROMPT='%F{green}%n@%m%f:%F{blue}%~%f%# '
  else PS1=\"\\[${_rh_e}[1;32m\\]\\u@\\h\\[${_rh_e}[0m\\]:\\[${_rh_e}[1;34m\\]\\w\\[${_rh_e}[0m\\]\\\\\\$ \"
  fi ;;
esac
unset _rh_e
export CLICOLOR=1
# 用户已定义同名 alias 时绝不覆盖（保留其自定义参数）
alias ls >/dev/null 2>&1 || { ls --color=auto -d . >/dev/null 2>&1 && alias ls='ls --color=auto'; }
alias grep >/dev/null 2>&1 || { echo . | grep --color=auto . >/dev/null 2>&1 && alias grep='grep --color=auto'; }
"
    } else {
        // 无色模式：仍注入含用户名的纯文本 PS1，避免远端默认提示符缺用户名
        "\
case \"$PS1$PROMPT\" in *'\\\\e['*|*'\\\\033['*|*'%F'*|*'fg['*|*'\\\\u'*|*'%n'*) ;; *)
  if [ -n \"$ZSH_VERSION\" ]; then PROMPT='%n@%m:%~%# '
  else PS1='\\u@\\h:\\w\\$ '
  fi ;;
esac
"
    };

    let env_block = env_export_lines(env);

    format!(
        "\
printf '\\033[1A\\r\\033[2K'
{env_block}{color_block}# CWD 上报钩子：每次提示符刷新前输出不可见 OSC 6667 序列（内含 $PWD），
# 后端 merge_task 解析后同步 SFTP 文件树到同一目录。
# bash 用 PROMPT_COMMAND，zsh 用 precmd_functions；前置注入不覆盖用户已有钩子。
_rh_cwd_hook() {{ printf '\\033]6667;%s\\007' \"$PWD\"; }}
if [ -n \"$ZSH_VERSION\" ]; then
  precmd_functions=( _rh_cwd_hook \"${{precmd_functions[@]}}\" )
else
  case \";$PROMPT_COMMAND;\" in *\";_rh_cwd_hook;\"*) ;; *)
    PROMPT_COMMAND=\"_rh_cwd_hook${{PROMPT_COMMAND:+;$PROMPT_COMMAND}}\" ;;
  esac
fi
rm -f '{path}'
printf '\\033]6666;rhinit\\007'
"
    )
}

/// 经独立 exec 通道把初始化脚本写入远端 `path`（尽力而为，失败不影响连接）。
/// 以远端退出状态确认落盘成功（cat 写失败/chmod 失败都会反映为非零），
/// 保证前端 source 时文件已完整。
async fn try_upload_script(
    handle: &mut client::Handle<ClientHandler>,
    path: &str,
    color_prompt: bool,
    env: &[(String, String)],
) -> bool {
    let result = async {
        let mut ch = handle
            .channel_open_session()
            .await
            .map_err(|e| e.to_string())?;
        ch.exec(true, format!("cat > {path} && chmod 600 {path}"))
            .await
            .map_err(|e| e.to_string())?;
        let script = init_script(path, color_prompt, env);
        ch.data(script.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        ch.eof().await.map_err(|e| e.to_string())?;
        // 等 cat 收尾并读取退出状态（ExitStatus 通常先于 Close 到达）
        let mut ok = true;
        while let Some(msg) = ch.wait().await {
            match msg {
                ChannelMsg::ExitStatus { exit_status } => ok = exit_status == 0,
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        let _ = ch.close().await;
        Ok::<bool, String>(ok)
    }
    .await;
    match result {
        Ok(ok) => ok,
        Err(e) => {
            debug!("初始化脚本写入失败 {path}（忽略，不影响连接）: {e}");
            false
        }
    }
}

/// 前端要发送的 source 命令：行首空格配合常见 HISTCONTROL=ignorespace 不进 history。
/// 命令刻意压到最短（删除临时文件由脚本末尾自删完成）：
/// 「PS1 + 本命令」典型宽度 ≈ 21+31 = 52 列，50 列以下终端才可能折行，
/// 单行清除才能精确抹掉回显。该命令由后端在 PTY 开启后立即注入（tty 缓冲暂存，
/// shell 就绪即执行），前端不参与时序。
fn source_cmd(path: &str) -> String {
    format!(" . \"{path}\" 2>/dev/null")
}

/// 注入脚本执行完成标记：不可见 OSC 序列，xterm 解析后静默丢弃（不上屏）。
/// merge_task 以它为「初始化完成」信号：检测到才放行 hold 住的 PTY 输出，
/// 前端首帧即脚本生效后的最终画面（无原始 PS1、无回显行的中间态）。
const INIT_DONE_MARKER: &[u8] = b"\x1b]6666;rhinit\x07";

/// Shell CWD 上报 OSC 序列前缀：`\x1b]6667;<path>\x07`。
/// 由 init_script 注入的 PROMPT_COMMAND / precmd 钩子在每次提示符刷新前输出，
/// merge_task 扫描到即提取 <path> 并发出 Cwd 帧同步 SFTP 文件树。
const CWD_MARKER_PREFIX: &[u8] = b"\x1b]6667;";
const OSC_TERMINATOR: u8 = 0x07;

/// 判定「shell 就绪」的输出静默阈值：PS1 刷完后 shell 阻塞读输入、不再输出，
/// 此时注入的命令回显必然紧跟 PS1（同一行），脚本单行清行才能精确命中。
/// 略大于攒包窗口（5ms），保证 PS1 批次完整刷完。
const SHELL_QUIET: Duration = Duration::from_millis(120);

/// 初始化 hold 状态：PTY 输出先攒在 buf 不上屏；cmd 为 Some 表示等静默注入，
/// None 表示已注入、等脚本末尾 marker
struct InitHold {
    buf: BytesMut,
    cmd: Option<Vec<u8>>,
    tx: mpsc::Sender<WriteReq>,
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }

    /// KEX 完成：记录本次真实协商出的主机密钥算法与加密算法。
    /// 在返回的 Future 被 poll 前同步写入（不把 `&mut self` 带入 async 块，
    /// 规避生命周期问题）；此时认证尚未开始，连接主流程读到时值已就绪。
    fn kex_done(
        &mut self,
        _shared_secret: Option<&[u8]>,
        names: &russh::Names,
        _session: &mut client::Session,
    ) -> impl core::future::Future<Output = Result<(), Self::Error>> + Send {
        *self.algo.lock().unwrap() = Some(NegotiatedAlgo {
            host_key: names.key.to_string(),
            cipher: names.cipher.as_ref().to_string(),
        });
        async { Ok(()) }
    }
}

/// 打开 session 通道并申请 PTY（不启动 shell：调用方需要先把 MOTD 首帧排好序）。
async fn open_pty(
    handle: &mut client::Handle<ClientHandler>,
    cfg: &SessionConfig,
) -> Result<russh::Channel<client::Msg>, SshError> {
    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| SshError::Channel(e.to_string()))?;
    channel
        .request_pty(
            true,
            TERM_TYPE,
            cfg.cols.max(1),
            cfg.rows.max(1),
            0,
            0,
            &[],
        )
        .await
        .map_err(|e| SshError::Channel(e.to_string()))?;
    Ok(channel)
}

/// 构造以登录 shell 替换自身的启动命令；远端 UTF-8 locale 缺失/失效时
/// （容器内常见两种：无 LANG 等同 C locale；或 LANG 指向未生成的 locale，
/// 如 `zh_CN.UTF-8` 但未 locale-gen——两种情况下 readline 都把粘贴的
/// UTF-8 字节转义成 `\nnn` 八进制文本，中文路径无法 cd）保底注入
/// `LC_CTYPE=C.UTF-8`。
///
/// 为什么写在命令串而非启动后脚本：readline 仅在初始化时读取一次 locale
/// （环境变量 LC_ALL/LC_CTYPE/LANG）并缓存；变量赋值前缀 + `exec`（POSIX
/// 特殊内建）会把 LC_CTYPE 带进替换后进程的环境，新 shell 启动时
/// `setlocale(LC_ALL, "")` 即读到 UTF-8。
///
/// 判断依据是 `locale charmap` **实测**当前字符集，而不是环境变量字符串：
/// LANG=zh_CN.UTF-8 但 locale 数据未生成时变量含 UTF-8、实际却是 ASCII。
/// 真正可用 UTF-8 时零改动；C.UTF-8 为 glibc 2.13+ / musl 内置，无需
/// locale-gen，实测可用才切换，否则静默回退普通启动（与改动前一致）。
fn locale_login_cmd(shell: &str) -> String {
    let q = shell.replace('\'', "'\\''");
    format!(
        "\
if [ \"$(locale charmap 2>/dev/null)\" = 'UTF-8' ]; then
  exec '{q}' -l
elif LC_CTYPE=C.UTF-8 locale charmap 2>/dev/null | grep -qi '^UTF-8$'; then
  LC_CTYPE=C.UTF-8 exec '{q}' -l
else
  exec '{q}' -l
fi",
        q = q,
    )
}

/// 打开 session 通道、申请 PTY 并启动交互式 shell（含抑制原生横幅逻辑）。
///
/// - `suppress=false`：标准 `request_shell`，sshd 打印原生 MOTD / Last login；
/// - `suppress=true`：以 exec 执行 `exec <login shell> -l`。sshd 的横幅
///   （do_motd / lastlog）只在 shell 请求路径输出，exec 路径天然不打印。
///   注意用「shell 启动参数 -l」而非 `exec -l`：后者是 shell 内建选项，
///   busybox ash 不支持（会报 illegal option 并退出）；而 `-l` 作为启动参数
///   bash/zsh/dash/ash/fish/tcsh 全部支持，且 stdin 是 PTY 故交互语义不变，
///   仍会加载 /etc/profile 等登录配置；外层 `exec` 是 POSIX 通用内建，
///   保证中间 shell 自我替换、不额外残留一层进程。
/// - exec 请求被拒（极少）或未探测到 shell 路径时，重开 PTY 回退标准 shell 请求
///   （同一 session 通道在一次会话子请求失败后不能再发 shell 请求）。
async fn open_interactive(
    handle: &mut client::Handle<ClientHandler>,
    cfg: &SessionConfig,
    suppress: bool,
    login_shell: Option<String>,
) -> Result<russh::Channel<client::Msg>, SshError> {
    let mut channel = open_pty(handle, cfg).await?;

    if suppress {
        if let Some(shell) = login_shell {
            // locale 保底必须在 shell 进程启动前注入（见 locale_login_cmd）：
            // readline 初始化时一次性读取并缓存 locale，shell 启动后再 export
            // 无法改变当前行编辑对非 ASCII 字节的处理。
            let cmd = locale_login_cmd(&shell);
            match channel.exec(true, cmd).await {
                Ok(()) => {
                    debug!("已以 exec(login shell -l) 启动，抑制服务端原生 MOTD: {shell}");
                    return Ok(channel);
                }
                Err(e) => warn!("exec 登录 shell 被拒，重开 PTY 回退标准 shell: {e}"),
            }
        } else {
            debug!("未探测到登录 shell 路径，抑制原生 MOTD 回退为标准 shell 请求");
        }
        // 回退路径：丢弃当前通道，重新申请 PTY + 标准 shell
        let _ = channel.close().await;
        channel = open_pty(handle, cfg).await?;
    }

    // 标准 shell 请求路径：尽力在启动前注入 UTF-8 字符集（接受与否取决于
    // sshd 的 AcceptEnv；want_reply=false 不等待回复、不会悬挂）。exec
    // 抑制路径已在命令串中解决，不会走到这里。被拒不影响后续 shell 请求。
    let _ = channel.set_env(false, "LC_CTYPE", "C.UTF-8").await;

    channel
        .request_shell(true)
        .await
        .map_err(|e| SshError::Channel(e.to_string()))?;
    Ok(channel)
}

/// PTY 事件：数据与结束信号走同一通道，保证帧顺序
enum PtyEvent {
    /// PTY 原始输出字节（stdout 与 stderr 合并）
    Data(Vec<u8>),
    /// 会话结束（远端 EOF/Close/退出状态）；payload 为可选原因
    Eof(Option<String>),
}

/// 前端 → 写任务的请求：输入数据与窗口尺寸复用同一通道，保持单写者
enum WriteReq {
    /// 键盘原始字节
    Data(Vec<u8>),
    /// PTY 尺寸变化（SSH window-change）
    Resize { cols: u32, rows: u32 },
}

/// 一条存活中的 SSH PTY 会话（由 SessionManager 持有）
pub struct SshSession {
    /// 取消令牌：关闭/异常时取消全部后台任务，防止任务泄漏
    cancel: CancellationToken,
    /// 前端请求入口：键盘输入与窗口尺寸变更
    write_tx: mpsc::Sender<WriteReq>,
    /// 已认证 SSH handle（russh Handle 内部为 Arc，clone 廉价）。
    /// 供 PTY 之外的独立 exec 子通道做主机指标采集（0x05/0x06 帧），
    /// 与终端通道物理隔离、互不影响；Mutex 串行化同一会话的 exec 通道操作。
    /// 同时供 sftp 模块懒加载 SFTP 子系统通道。
    pub(super) handle: Arc<Mutex<client::Handle<ClientHandler>>>,
    /// 帧发送口的 clone：动态指标（0x06）与 PTY 帧共用同一条 FIFO 队列，
    /// 经 ipc 层同一个 Channel 转发前端
    frame_tx: mpsc::Sender<Vec<u8>>,
    /// 动态指标采集任务句柄（启动后存在）。同步锁仅做瞬时取放，
    /// 不在持锁期间 await；stop 只取消令牌，任务异步退出
    metrics: StdMutex<Option<crate::metrics::collector::CollectorHandle>>,
    /// SFTP 子系统长驻会话（懒加载；远端文件面板列目录使用）
    pub(super) sftp: SftpState,
    /// Shell 当前工作目录：由 merge_task 解析 PTY 中的 OSC 6667 序列更新，
    /// sftp_list_dir 无参数时以此为默认目录，保持 Shell 与 SFTP CWD 一致。
    pub(super) cwd: Arc<Mutex<Option<String>>>,
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

/// 仅建立 TCP+SSH 握手并完成认证（密码或公钥），不申请 PTY/shell。
///
/// 由「正式连接」与「测试连接」共用，保证两者认证行为完全一致：
/// 测试通过 = 正式连接的认证阶段也必然通过。成功返回已认证的 handle，
/// 调用方负责断开；测试连接用完即断，不注册会话池。
pub(crate) async fn connect_and_auth(
    cfg: &SessionConfig,
    algo: Arc<StdMutex<Option<NegotiatedAlgo>>>,
) -> Result<client::Handle<ClientHandler>, SshError> {
    let config = Arc::new(client::Config {
        inactivity_timeout: Some(Duration::from_secs(300)),
        keepalive_interval: Some(Duration::from_secs(30)),
        nodelay: true,
        ..Default::default()
    });
    let mut handle =
        client::connect(config, (cfg.host.as_str(), cfg.port), ClientHandler { algo })
            .await
            .map_err(|e| SshError::Connect(e.to_string()))?;

    let auth = match &cfg.auth {
        AuthMethod::Password(password) => handle
            .authenticate_password(cfg.username.clone(), password.clone())
            .await
            .map_err(|e| SshError::Auth(e.to_string()))?,
        AuthMethod::Key { path, passphrase } => {
            let key_path = expand_tilde(path);
            let key = russh::keys::load_secret_key(
                &key_path,
                if passphrase.is_empty() {
                    None
                } else {
                    Some(passphrase.as_str())
                },
            )
            .map_err(|e| match e {
                // 无口令加载加密私钥 / 口令错误都报 KeyIsEncrypted，
                // 统一转成约定标记，前端弹口令框后带口令重试
                russh::keys::Error::KeyIsEncrypted => SshError::KeyEncrypted,
                other => SshError::Auth(format!("私钥加载失败（{key_path}）: {other}")),
            })?;
            handle
                .authenticate_publickey(
                    cfg.username.clone(),
                    russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), None),
                )
                .await
                .map_err(|e| SshError::Auth(e.to_string()))?
        }
    };
    if !auth.success() {
        let _ = handle
            .disconnect(Disconnect::ByApplication, "auth failed", "")
            .await;
        let reason = match &cfg.auth {
            AuthMethod::Password(_) => "用户名或密码错误",
            AuthMethod::Key { .. } => "服务器拒绝了该密钥",
        };
        return Err(SshError::Auth(reason.into()));
    }
    Ok(handle)
}

/// 全局同时在飞的指标 exec 通道上限（进程级保险丝）。
/// 正常仅活动会话采集（前端门控），远触不到该上限；批量会话齐启动/重连风暴等
/// 极端情况下削峰，避免瞬时打开大量 SSH channel 耗尽本机 fd 或撞远端 sshd MaxSessions。
const MAX_CONCURRENT_METRIC_EXEC: usize = 24;
static METRIC_EXEC_GATE: Semaphore = Semaphore::const_new(MAX_CONCURRENT_METRIC_EXEC);

/// 当前在飞的指标 exec 数（诊断/压测观测；宽松序足够）
static METRIC_EXEC_INFLIGHT: AtomicUsize = AtomicUsize::new(0);
/// 进程生命周期内在飞 exec 的历史峰值（压测验收「启动峰值 ≤ 24」实测读数）
static METRIC_EXEC_PEAK: AtomicUsize = AtomicUsize::new(0);

/// 指标 exec 并发观测值：`(当前在飞, 历史峰值)`。仅用于诊断与压测断言。
pub fn metric_exec_concurrency() -> (usize, usize) {
    (
        METRIC_EXEC_INFLIGHT.load(Ordering::Relaxed),
        METRIC_EXEC_PEAK.load(Ordering::Relaxed),
    )
}

/// 在飞计数 RAII 守卫：无论 exec 成功/失败/提前返回，离开作用域即递减。
struct InflightGuard;
impl InflightGuard {
    fn enter() -> Self {
        let n = METRIC_EXEC_INFLIGHT.fetch_add(1, Ordering::Relaxed) + 1;
        METRIC_EXEC_PEAK.fetch_max(n, Ordering::Relaxed);
        Self
    }
}
impl Drop for InflightGuard {
    fn drop(&mut self) {
        METRIC_EXEC_INFLIGHT.fetch_sub(1, Ordering::Relaxed);
    }
}

impl SshSession {
    /// 建立连接并完成密码认证；严格按「认证 → exec 采集 → MOTD 首帧 → 开启 PTY」
    /// 的顺序编排，保证 Rhost MOTD 始终先于远端 PS1 提示符出现。
    ///
    /// 返回 (会话句柄, 帧输出接收端)；帧接收端交给 ipc 层转发给前端 Channel。
    /// 初始化脚本（始终含 CWD 钩子，cfg.color_prompt 开启时含彩色提示符）已在
    /// PTY 开启后经写队列自动注入，merge_task 以 marker 为信号 hold 初始化输出，
    /// 前端无需参与时序编排。
    /// 后台任务（读/合并/写）均挂在 `cancel` 上，`shutdown()` 即全部取消。
    pub async fn connect(cfg: SessionConfig) -> Result<(Self, mpsc::Receiver<Vec<u8>>), SshError> {
        let cancel = CancellationToken::new();

        // 1. TCP + SSH 握手 + 认证（与测试连接共用，保证认证行为一致）。
        //    russh Handle 不支持 Clone，认证后即包进 Arc<Mutex>：
        //    连接阶段（MOTD/落盘/开 PTY）与连接后的指标 exec 通道共用同一个 handle，
        //    Mutex 串行化通道操作，Arc 保证 PTY 后台任务存活期间连接不被释放。
        //    algo 是 KEX 协商算法的出口（kex_done 回调已在认证前填入）。
        let algo = Arc::new(StdMutex::new(None));
        let handle =
            Arc::new(Mutex::new(connect_and_auth(&cfg, algo.clone()).await?));

        // 2.【阶段 A/B】PTY 尚未打开：独立 exec 子通道并行采集服务器状态，
        //    本地组装 MOTD 渲染指令数组（不碰 PTY、不经键盘、不解析 shell 输出）。
        //    3s 超时/失败/取消均静默降级为 None，绝不阻塞主连接。
        let motd_outcome = if cfg.motd {
            let mut h = handle.lock().await;
            crate::motd::collect_and_build(&mut h, &cancel).await
        } else {
            None
        };

        // 初始化脚本（CWD 钩子 + 可选彩色提示符）在 PTY 开启前经 exec 通道静默落盘，
        // 终端无感知。CWD 钩子始终注入以保证 Shell→SFTP 目录同步；彩色部分仅在
        // cfg.color_prompt 开启时生效。/tmp 不可写时静默降级，init_cmd 为 None。
        let tag = &Uuid::new_v4().simple().to_string()[..8];
        let tmp_path = format!("/tmp/.ri-{tag}");
        let init_cmd = {
            let mut h = handle.lock().await;
            if try_upload_script(&mut h, &tmp_path, cfg.color_prompt, &cfg.env).await {
                Some(source_cmd(&tmp_path))
            } else {
                None
            }
        };

        // 3. 建立帧 / 事件 / 输入通道；合并任务先待命（此时没有任何 PTY 事件）
        let (event_tx, event_rx) = mpsc::channel::<PtyEvent>(EVENT_QUEUE);
        let (frame_tx, frame_rx) = mpsc::channel::<Vec<u8>>(FRAME_QUEUE);
        let (write_tx, write_rx) = mpsc::channel::<WriteReq>(WRITE_QUEUE);

        // 4.【阶段 C】MOTD 指令数组作为整个二进制通道的**首帧**发出，
        //    紧接着发主机静态信息（0x05）。mpsc FIFO + 此刻尚无 PTY 数据帧，
        //    严格保证帧序：MOTD → HostInfo → 一切 PTY 输出。
        if let Some(outcome) = &motd_outcome {
            let json = serde_json::to_vec(&outcome.cmds).unwrap_or_default();
            if !json.is_empty()
                && frame_tx
                    .send(encode_frame(FrameType::Motd, &json))
                    .await
                    .is_err()
            {
                warn!("帧通道已关闭，放弃 MOTD 首帧");
            }

            // HostInfo：复用同一批采集 raw 解析，不额外开 exec 通道
            let info = crate::metrics::build_host_info(&outcome.raw);
            let json = serde_json::to_vec(&info).unwrap_or_default();
            if frame_tx
                .send(encode_frame(FrameType::HostInfo, &json))
                .await
                .is_err()
            {
                warn!("帧通道已关闭，放弃 HostInfo 帧");
            }
        }

        // Algo：SSH 握手真实协商算法，无条件发送（与 MOTD 开关无关）。
        // kex_done 早在认证前已写入；排在 HostInfo/MOTD 之后、PTY 首字节之前。
        let negotiated = algo.lock().unwrap().clone().unwrap_or_default();
        let algo_json = serde_json::json!({
            "host_key": negotiated.host_key,
            "cipher": negotiated.cipher,
            "term": TERM_TYPE,
            "enc": TERM_ENCODING,
        });
        let algo_bytes = serde_json::to_vec(&algo_json).unwrap_or_default();
        if frame_tx
            .send(encode_frame(FrameType::Algo, &algo_bytes))
            .await
            .is_err()
        {
            warn!("帧通道已关闭，放弃 Algo 帧");
        }

        // 5/6.【阶段 D】打开 PTY 并启动交互式 shell（抑制开关决定走 exec 还是 shell 请求）。
        //    MOTD 首帧已在帧队列中，PTY 输出无论多快都排在其后。
        let login_shell = motd_outcome.as_ref().and_then(|o| o.login_shell.clone());
        let channel = {
            let mut h = handle.lock().await;
            // motd 开启即走 exec 路径抑制 sshd 原生横幅，关闭走标准 shell
            open_interactive(&mut h, &cfg, cfg.motd, login_shell).await?
        };

        // 7. 拆分读写半通道，启动双向转发；PTY 输出从此刻起进入帧队列（恒在 MOTD 之后）
        let (read_half, write_half) = channel.split();
        // 留一个帧发送口给动态指标采集器（0x06），与 PTY 帧共用 FIFO 保序
        let metrics_frame_tx = frame_tx.clone();

        // 提示符注入交给 merge_task 的静默门：等 shell 启动输出静默（PS1 刷完、
        // readline 阻塞）后才经写队列注入——回显紧跟 PS1 同行，脚本单行清行精确
        // 命中；注入全过程的 PTY 输出被 hold 至脚本末尾 marker，前端零感知。
        let init_inject = init_cmd.map(|cmd| {
            let mut bytes = cmd.into_bytes();
            bytes.push(b'\r');
            (bytes, write_tx.clone())
        });

        tokio::spawn(read_task(read_half, event_tx.clone(), cancel.clone()));
        let cwd = Arc::new(Mutex::new(None));
        tokio::spawn(merge_task(
            event_rx,
            frame_tx,
            cancel.clone(),
            init_inject,
            cwd.clone(),
        ));
        tokio::spawn(write_task(write_half, write_rx, cancel.clone()));

        // 7.1 RTT 探测任务（0x07 帧）：独立于 MetricsCollector 运行——
        //     状态栏常显，不能随 Inspector 隐藏而停更；挂在会话 cancel 上随断连退出
        tokio::spawn(rtt_task(
            handle.clone(),
            metrics_frame_tx.clone(),
            cancel.clone(),
        ));

        // 8. handle（Arc<Mutex>）随 SshSession 保留：供连接建立后的独立 exec
        //    指标采集通道使用（主机指标 0x05/0x06 帧），与 PTY 半通道共同维持连接。
        debug!(
            "SSH 会话已建立: {}@{}:{} ({}x{})",
            cfg.username, cfg.host, cfg.port, cfg.cols, cfg.rows
        );

        Ok((
            Self {
                cancel,
                write_tx,
                handle,
                frame_tx: metrics_frame_tx,
                metrics: StdMutex::new(None),
                sftp: SftpState::new(),
                cwd,
            },
            frame_rx,
        ))
    }

    /// 将前端原始输入写入 PTY（异步排队，不阻塞调用方）
    pub async fn write(&self, data: Vec<u8>) -> Result<(), SshError> {
        self.write_tx
            .send(WriteReq::Data(data))
            .await
            .map_err(|_| SshError::Closed)
    }

    /// 同步 PTY 窗口尺寸：发送 SSH window-change（像素尺寸传 0，由对端忽略）
    pub async fn resize(&self, cols: u32, rows: u32) -> Result<(), SshError> {
        if cols == 0 || rows == 0 {
            // 容器不可见时 fit 可能得出 0，不能把远端 PTY 缩成 0x0
            return Ok(());
        }
        self.write_tx
            .send(WriteReq::Resize { cols, rows })
            .await
            .map_err(|_| SshError::Closed)
    }

    /// SFTP → Shell 方向同步：把远端交互式 shell 的工作目录切到 `path`。
    ///
    /// 经 PTY 写入通道发送 `cd '<quoted>'\\n`，shell 执行后提示符即反映新目录，
    /// 随后 init_script 注入的 CWD 钩子会上报新路径，形成闭环。路径含单引号
    /// 时按 shell 规则转义（`'` → `'\''`），避免注入。
    pub(crate) async fn sync_shell_cd(&self, path: &str) -> Result<(), SshError> {
        let quoted = format!("'{}'", path.replace('\'', "'\\''"));
        let cmd = format!("cd {quoted}\n");
        self.write_tx
            .send(WriteReq::Data(cmd.into_bytes()))
            .await
            .map_err(|_| SshError::Closed)
    }

    /// 取当前跟踪的 Shell CWD（由 OSC 6667 钩子上报，merge_task 更新）
    pub(super) async fn get_cwd(&self) -> Option<String> {
        self.cwd.lock().await.clone()
    }

    /// 在独立 exec 子通道执行采集脚本，返回 stdout/stderr 合并的文本。
    ///
    /// 与 PTY 交互通道完全隔离：不申请 PTY、不经键盘输入、不解析终端流，
    /// 用于 MOTD 之后的主机指标采集（0x05 静态信息 / 0x06 周期指标）。
    ///
    /// - `script_body` 为 `/bin/sh` 命令体（POSIX sh，兼容 fish/tcsh 用户 shell），
    ///   单引号按标准 shell 规则转义，禁止拼接任何会话参数字段；
    /// - 整次执行受 `timeout` 约束；超时后 future 与通道随作用域 drop 关闭，
    ///   远端 sh 收 SIGHUP/SIGPIPE 退出，无残留进程；
    /// - 同一会话的 exec 通道由 Mutex 串行（collector 侧另有单飞保证，不会竞争）。
    pub async fn exec_collect(
        &self,
        script_body: &str,
        timeout: Duration,
    ) -> Result<String, SshError> {
        let wrapped = format!("/bin/sh -c '{}'", script_body.replace('\'', "'\\''"));

        let fut = async {
            // 全局并发闸门：极端并发（批量会话齐启动/重连风暴/前端门控失效）下限制同时
            // 在飞的 exec 通道，保护本机 fd 与远端 sshd MaxSessions。正常负载下立即取牌。
            // permit 在整个 channel 读写期间持有，随本 async 块结束 drop 归还；
            // 排队取牌也被外层 timeout 覆盖，慢主机上不会无限挂起。
            let _permit = METRIC_EXEC_GATE
                .acquire()
                .await
                .map_err(|e| SshError::Channel(format!("exec 闸门已关闭: {e}")))?;
            // 取到牌才计入在飞（不计排队），守卫随本块结束 drop 递减，成功失败都不漏
            let _inflight = InflightGuard::enter();
            let handle = self.handle.lock().await;
            let mut ch = handle
                .channel_open_session()
                .await
                .map_err(|e| SshError::Channel(e.to_string()))?;
            ch.exec(true, wrapped)
                .await
                .map_err(|e| SshError::Channel(e.to_string()))?;

            let mut out = Vec::new();
            while let Some(msg) = ch.wait().await {
                match msg {
                    ChannelMsg::Data { data } => out.extend_from_slice(&data),
                    ChannelMsg::ExtendedData { data, .. } => out.extend_from_slice(&data),
                    ChannelMsg::Close => break,
                    _ => {}
                }
            }
            let _ = ch.close().await;
            // 远端命令输出均为 UTF-8 文本（/proc、df 等）；非法字节不致命，
            // 与 MOTD 采集一致用 from_utf8_lossy 语义交给解析层降级
            Ok::<_, SshError>(String::from_utf8_lossy(&out).into_owned())
        };

        match time::timeout(timeout, fut).await {
            Ok(res) => res,
            Err(_) => Err(SshError::Channel(format!(
                "exec 采集超时（{}s）",
                timeout.as_secs()
            ))),
        }
    }

    /// 启动动态指标采集（0x06）。幂等：已在采集则先停旧任务再以新间隔启动。
    /// `interval_ms` 夹在 1s~10s，防异常参数打爆远端；
    /// `iface` 为默认路由网卡名，网络计数优先取它，None/缺失时汇总非 lo 网卡。
    pub fn start_metrics(self: Arc<Self>, interval_ms: u64, iface: Option<String>) {
        let interval = Duration::from_millis(interval_ms.clamp(1000, 10_000));
        let mut guard = self.metrics.lock().unwrap();
        if let Some(old) = guard.take() {
            old.stop();
        }
        let handle = crate::metrics::collector::spawn(
            self.clone(),
            interval,
            self.cancel.clone(),
            self.frame_tx.clone(),
            iface,
        );
        *guard = Some(handle);
    }

    /// 停止动态指标采集（心跳超时由任务自行退出，此处为前端显式停/切换会话）
    pub fn stop_metrics(&self) {
        if let Ok(mut guard) = self.metrics.lock()
            && let Some(handle) = guard.take()
        {
            handle.stop();
        }
    }

    /// 前端心跳续约：刷新采集任务的 TTL
    pub fn metrics_heartbeat(&self) {
        if let Ok(guard) = self.metrics.lock()
            && let Some(handle) = guard.as_ref()
        {
            handle.heartbeat();
        }
    }

    /// 关闭会话：取消所有后台任务，释放 russh 连接
    pub fn shutdown(&self) {
        // 采集任务挂在会话 cancel 的 child token 上，根取消即连带停止
        if let Ok(mut guard) = self.metrics.lock() {
            *guard = None;
        }
        self.cancel.cancel();
    }

    /// 派生会话级取消令牌：SFTP 传输等独立任务挂在其上，
    /// 会话 shutdown 时连带取消（供同 crate 的 sftp 模块使用）
    pub(super) fn child_cancel(&self) -> CancellationToken {
        self.cancel.child_token()
    }
}

/// 读任务：独占读半通道，把服务端消息转成 PtyEvent。
/// 结束时 event_tx 随作用域 drop，merge_task 收到 None 后 flush 并退出。
async fn read_task(
    mut read_half: russh::ChannelReadHalf,
    event_tx: mpsc::Sender<PtyEvent>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            msg = read_half.wait() => {
                match msg {
                    Some(ChannelMsg::Data { data }) => {
                        if event_tx.send(PtyEvent::Data(data.to_vec())).await.is_err() {
                            break;
                        }
                    }
                    // stderr（扩展数据流）并入主流，交给 xterm 渲染
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        if event_tx.send(PtyEvent::Data(data.to_vec())).await.is_err() {
                            break;
                        }
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        let _ = event_tx
                            .send(PtyEvent::Eof(Some(format!("进程退出，状态码 {exit_status}"))))
                            .await;
                        break;
                    }
                    Some(ChannelMsg::ExitSignal { signal_name, error_message, .. }) => {
                        let _ = event_tx
                            .send(PtyEvent::Eof(Some(format!(
                                "进程被信号 {signal_name:?} 终止: {error_message}"
                            ))))
                            .await;
                        break;
                    }
                    // 远端主动关闭 / 半通道结束
                    Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => {
                        let _ = event_tx.send(PtyEvent::Eof(None)).await;
                        break;
                    }
                    // Success / Failure / WindowAdjusted 等控制消息暂不处理
                    _ => {}
                }
            }
        }
    }
}

/// 扫描字节流中的 CWD 上报 OSC 序列（`\x1b]6667;<path>\x07`），返回提取到的路径。
/// OSC 字节保留在原流中由 xterm.js 静默丢弃（未知 OSC 码不上屏），故此处不做移除，
/// 仅提取路径供后端同步 SFTP CWD。若前缀已到但终止符未到（跨包），返回 None 等待。
fn scan_cwd(buf: &[u8]) -> Option<String> {
    let mut i = 0;
    while i + CWD_MARKER_PREFIX.len() <= buf.len() {
        if &buf[i..i + CWD_MARKER_PREFIX.len()] == CWD_MARKER_PREFIX {
            let tail = &buf[i + CWD_MARKER_PREFIX.len()..];
            if let Some(rel) = tail.iter().position(|&b| b == OSC_TERMINATOR) {
                return Some(String::from_utf8_lossy(&tail[..rel]).into_owned());
            }
            // 前缀命中但无终止符：路径尚未收完，等下一批数据
            return None;
        }
        i += 1;
    }
    None
}

/// 合并任务：把高频小数据包按「4KB 或 5ms」攒成一帧，降低 IPC 次数。
/// 控制事件（Eof）到达时先冲刷缓存数据再发 Exit 帧，保证前端看到的顺序。
///
/// 提示符注入（init_inject 存在时）三阶段状态机，全程输出不上屏：
/// 1. AwaitQuiet：hold 全部 PTY 输出，120ms 静默判定 PS1 已刷完（readline 阻塞）
/// 2. 注入 source 命令——回显紧跟 PS1 同行，脚本单行清行精确命中
/// 3. AwaitMarker：检测到脚本末尾 OSC marker 后，hold 内容转入主攒包窗口，
///    紧随的着色 PS1 并入同帧放行——前端单帧绘制最终画面
/// 3s 超时兜底：尽力注入并直通（退化但不阻塞）。
///
/// 另外扫描 PTY 输出中的 CWD 上报 OSC 序列，命中即更新会话 cwd 并向前端发 Cwd 帧。
async fn merge_task(
    mut event_rx: mpsc::Receiver<PtyEvent>,
    frame_tx: mpsc::Sender<Vec<u8>>,
    cancel: CancellationToken,
    init_inject: Option<(Vec<u8>, mpsc::Sender<WriteReq>)>,
    cwd: Arc<Mutex<Option<String>>>,
) {
    let mut buf = BytesMut::with_capacity(MERGE_BYTES * 2);
    // 合并窗口定时器：仅在窗口开启时参与 select
    let window = time::sleep(MERGE_WINDOW);
    tokio::pin!(window);
    let mut window_open = false;

    let mut hold: Option<InitHold> = init_inject.map(|(cmd, tx)| InitHold {
        buf: BytesMut::with_capacity(8192),
        cmd: Some(cmd),
        tx,
    });
    // 静默计时（等 PS1 刷完）与 hold 硬超时，仅在对应阶段参与 select
    let quiet = time::sleep(SHELL_QUIET);
    tokio::pin!(quiet);
    let mut quiet_open = false;
    let hold_deadline = time::sleep(Duration::from_secs(3));
    tokio::pin!(hold_deadline);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            ev = event_rx.recv() => {
                match ev {
                    Some(PtyEvent::Data(data)) => {
                        if let Some(h) = &mut hold {
                            let scan = h.buf.len().saturating_sub(INIT_DONE_MARKER.len() - 1);
                            h.buf.extend_from_slice(&data);
                            if h.cmd.is_some() {
                                // 阶段 1：启动输出仍在刷新，重置静默计时
                                quiet.as_mut().reset(Instant::now() + SHELL_QUIET);
                                quiet_open = true;
                            } else if h.buf[scan..]
                                .windows(INIT_DONE_MARKER.len())
                                .any(|w| w == INIT_DONE_MARKER)
                            {
                                // 阶段 3：marker 到——转主攒包缓冲走正常窗口，
                                // 紧随 marker 的着色 PS1 并入同帧
                                let h = hold.take().unwrap();
                                buf.extend_from_slice(&h.buf[..]);
                                if !window_open {
                                    window.as_mut().reset(Instant::now() + MERGE_WINDOW);
                                    window_open = true;
                                }
                            }
                            continue;
                        }
                        buf.extend_from_slice(&data);
                        // 扫描 Shell CWD 上报序列，命中即更新会话 cwd 并向前端发 Cwd 帧。
                        // OSC 字节仍留在 buf 中随 PTY 流下发，xterm.js 对未知 OSC 码静默丢弃。
                        if let Some(path) = scan_cwd(&buf) {
                            *cwd.lock().await = Some(path.clone());
                            let _ = frame_tx
                                .send(encode_frame(FrameType::Cwd, path.as_bytes()))
                                .await;
                        }
                        if !window_open {
                            window.as_mut().reset(Instant::now() + MERGE_WINDOW);
                            window_open = true;
                        }
                        if buf.len() >= MERGE_BYTES {
                            flush(&mut buf, &frame_tx).await;
                            window_open = false;
                        }
                    }
                    Some(PtyEvent::Eof(reason)) => {
                        if let Some(h) = hold.take() {
                            buf.extend_from_slice(&h.buf[..]);
                        }
                        flush(&mut buf, &frame_tx).await;
                        let payload = reason.unwrap_or_default();
                        let _ = frame_tx
                            .send(encode_frame(FrameType::Exit, payload.as_bytes()))
                            .await;
                        break;
                    }
                    // 上游全部关闭：冲刷残包后退出
                    None => {
                        if let Some(h) = hold.take() {
                            buf.extend_from_slice(&h.buf[..]);
                        }
                        flush(&mut buf, &frame_tx).await;
                        break;
                    }
                }
            }
            _ = &mut window, if window_open => {
                flush(&mut buf, &frame_tx).await;
                window_open = false;
            }
            // 阶段 1→2：静默确认 shell 就绪，注入 source 命令
            _ = &mut quiet, if quiet_open => {
                quiet_open = false;
                if let Some(h) = &mut hold {
                    if let Some(cmd) = h.cmd.take() {
                        let _ = h.tx.send(WriteReq::Data(cmd)).await;
                    }
                }
            }
            // hold 超时：尽力注入（若还未），已攒内容直通放行
            _ = &mut hold_deadline, if hold.is_some() => {
                if let Some(mut h) = hold.take() {
                    if let Some(cmd) = h.cmd.take() {
                        let _ = h.tx.send(WriteReq::Data(cmd)).await;
                    }
                    buf.extend_from_slice(&h.buf[..]);
                    flush(&mut buf, &frame_tx).await;
                    window_open = false;
                }
            }
        }
    }
}

/// 把累积缓冲区封装为 Data 帧发出（空缓冲不发）
async fn flush(buf: &mut BytesMut, frame_tx: &mpsc::Sender<Vec<u8>>) {
    if buf.is_empty() {
        return;
    }
    let frame = encode_frame(FrameType::Data, &buf[..]);
    buf.clear();
    if frame_tx.send(frame).await.is_err() {
        warn!("帧输出通道已关闭，丢弃 PTY 数据");
    }
}

/// 写任务：串行处理前端输入与尺寸变更（均通过写半通道发出）。
/// 退出时尽力通知服务端关闭通道；Handle 随任务 drop 后底层连接释放。
async fn write_task(
    write_half: russh::ChannelWriteHalf<client::Msg>,
    mut write_rx: mpsc::Receiver<WriteReq>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            req = write_rx.recv() => {
                match req {
                    Some(WriteReq::Data(bytes)) => {
                        if write_half.data_bytes(bytes).await.is_err() {
                            break;
                        }
                    }
                    Some(WriteReq::Resize { cols, rows }) => {
                        // window-change 不要求对端回复；失败说明通道已断
                        if write_half.window_change(cols, rows, 0, 0).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    }
    let _ = write_half.close().await;
}

/// RTT 探测任务（0x07 帧）：SSH global request ping/pong 计时。
///
/// - 建连后立即测一次让首屏有值，之后每 30s 一轮；
/// - `send_ping` 等待 sshd 的 request 响应（RFC 4254 要求对 want_reply 的
///   global request 必回 success/failure，故任何标准 sshd 都能计时）；
/// - 单轮 5s 超时/协议错误静默跳过——断连判定交给 inactivity_timeout 与
///   PTY 通道错误，RTT 任务不做连接健康裁决；
/// - ping 期间持有 handle 锁，会短暂阻塞同会话 metrics exec 开通道：
///   正常 RTT 亚秒级，最坏 5s 超时导致 metrics 单轮跳过（有界影响，可接受）；
/// - 帧发送失败即前端通道已释放（窗口关闭），任务退出。
async fn rtt_task(
    handle: Arc<Mutex<client::Handle<ClientHandler>>>,
    frame_tx: mpsc::Sender<Vec<u8>>,
    cancel: CancellationToken,
) {
    loop {
        let fut = async {
            let h = handle.lock().await;
            let start = Instant::now();
            h.send_ping().await.map(|_| start.elapsed())
        };
        if let Ok(Ok(d)) = time::timeout(RTT_TIMEOUT, fut).await {
            let payload = serde_json::to_vec(&serde_json::json!({ "ms": d.as_millis() as u64 }))
                .unwrap_or_default();
            if frame_tx
                .send(encode_frame(FrameType::Rtt, &payload))
                .await
                .is_err()
            {
                break;
            }
        }
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = time::sleep(RTT_INTERVAL) => {}
        }
    }
}

#[cfg(test)]
mod gate_tests {
    use super::METRIC_EXEC_GATE;

    /// 全局闸门容量固定 24：占满后第 25 个取牌失败，permit 释放后立即可再取。
    /// lib 单测二进制内仅本测试接触该全局信号量（真实 exec 在独立的 e2e 二进制）。
    #[test]
    fn exec_gate_caps_at_24() {
        let permits: Vec<_> = (0..24)
            .map(|i| {
                METRIC_EXEC_GATE
                    .try_acquire()
                    .unwrap_or_else(|_| panic!("第 {i} 个 permit 应可获取"))
            })
            .collect();
        assert!(
            METRIC_EXEC_GATE.try_acquire().is_err(),
            "占满 24 个后不应再能取牌"
        );
        drop(permits);
        assert!(
            METRIC_EXEC_GATE.try_acquire().is_ok(),
            "permit 全部释放后应恢复可取"
        );
    }
}

#[cfg(test)]
mod env_export_tests {
    use super::{env_export_lines, init_script};

    #[test]
    fn normal_pairs_get_quoted_exports() {
        let out = env_export_lines(&[
            ("EDITOR".into(), "nvim".into()),
            ("PROXY_URL".into(), "http://127.0.0.1:7890".into()),
        ]);
        assert_eq!(out, "export EDITOR='nvim'\nexport PROXY_URL='http://127.0.0.1:7890'\n");
    }

    #[test]
    fn single_quote_in_value_is_posix_escaped() {
        let out = env_export_lines(&[("MSG".into(), "it's ok".into())]);
        // '\'' 关闭引号+转义引号+重开引号，shell 里还原为字面单引号
        assert_eq!(out, "export MSG='it'\\''s ok'\n");
    }

    #[test]
    fn illegal_keys_and_multiline_values_are_skipped() {
        let out = env_export_lines(&[
            ("1BAD".into(), "v".into()),      // 数字开头
            ("BAD KEY".into(), "v".into()),   // 含空格
            ("BAD;KEY".into(), "v".into()),   // 注入面：分号
            ("BAD-KEY".into(), "v".into()),   // 连字符
            ("".into(), "v".into()),          // 空键
            ("OK".into(), "a\nb".into()),     // 值含换行破坏脚本单行结构
            ("OK".into(), "a\rb".into()),     // 值含回车
            ("GOOD_1".into(), "v".into()),    // 合法：下划线+数字
            ("_X".into(), "v".into()),        // 合法：下划线开头
        ]);
        assert_eq!(out, "export GOOD_1='v'\nexport _X='v'\n");
    }

    #[test]
    fn env_block_lands_after_clear_line_in_script() {
        let script = init_script(
            "/tmp/.ri-test",
            false,
            &[("EDITOR".into(), "nvim".into())],
        );
        let clear = script.find("printf '\\033[1A\\r\\033[2K'").expect("清行必须是第一行");
        let env_line = script.find("export EDITOR='nvim'").expect("export 块应在脚本内");
        let hook = script.find("_rh_cwd_hook").expect("CWD 钩子应在脚本内");
        assert!(clear < env_line && env_line < hook, "顺序应为 清行 → export → 钩子");
        // color_prompt=false 时 export 之后不应有颜色块
        assert!(!script.contains("_rh_e"), "未开启提示符着色时不应有颜色块");
    }
}
