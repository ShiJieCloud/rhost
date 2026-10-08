//! SSH PTY 端到端集成测试。
//!
//! 依赖本地测试容器（密码认证）：
//! ```bash
//! docker run -d --name rhost-test-sshd -p 2222:2222 \
//!   -e PASSWORD_ACCESS=true -e USER_NAME=test -e USER_PASSWORD=rhost123 \
//!   lscr.io/linuxserver/openssh-server:latest
//! # 该镜像默认 sshd 配置禁用转发；隧道用例（tunnel_*）需开启转发、放宽 preauth
//! # 并发并关闭来源惩罚（高并发闸门用例以 sshd 自身端口为数据面目标）后重启：
//! docker exec rhost-test-sshd sed -i 's/^AllowTcpForwarding no/AllowTcpForwarding yes/' /config/sshd/sshd_config
//! docker exec rhost-test-sshd sh -c 'echo "MaxStartups 100" >> /config/sshd/sshd_config'
//! docker exec rhost-test-sshd sh -c 'echo "PerSourcePenalties no" >> /config/sshd/sshd_config'
//! docker restart rhost-test-sshd
//! ```
//! 无容器时测试自动跳过（连接失败即视为环境缺失，不误报失败）。

use std::time::Duration;

use rhost_lib::ssh::frame::decode_frame;
use rhost_lib::ssh::session::SshSession;
use rhost_lib::ssh::{AuthMethod, HostKeyCheck, HostKeyPolicy, SessionConfig};

fn test_cfg() -> SessionConfig {
    SessionConfig {
        host: "127.0.0.1".into(),
        port: 2222,
        username: "test".into(),
        auth: AuthMethod::Password("rhost123".into()),
        cols: 120,
        rows: 32,
        motd: false,
        color_prompt: true,
        env: vec![],
        motd_logo: String::new(),
        // 既有用例不关心主机密钥校验：显式跳过（doc(hidden)，仅测试可达）；
        // 信任路径行为由下方 ssh_hostkey_tofu_trust_and_persist 专项覆盖
        host_key: HostKeyPolicy::SkipForTests,
    }
}

/// 连接 → 写命令 → 收到回显数据帧 → 主动断开（验证任务取消无泄漏）
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_pty_echo_roundtrip() {
    let (session, mut frame_rx) = match SshSession::connect(test_cfg(), "e2e001").await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("跳过：测试容器不可用（{e}）");
            return;
        }
    };

    // 1. 发送 window-change：100 列 × 40 行
    session.resize(100, 40).await.expect("window-change");
    // 等服务端处理窗口尺寸变更
    tokio::time::sleep(Duration::from_millis(200)).await;

    // 2. stty size 输出 "rows cols"，即 "40 100"；echo 验证回显链路
    session
        .write(b"stty size; echo RHOST_E2E_OK\r".to_vec())
        .await
        .expect("写入命令");

    // 收集帧流直到看到全部标记（帧格式：1B 类型 + 4B 长度 + payload）
    let mut got = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let text_all = |g: &[u8]| String::from_utf8_lossy(g).to_string();
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(3), frame_rx.recv()).await {
            // frame_rx 收到的是编码帧（1B 类型 + 4B 长度 + payload），解码后取 payload
            Ok(Some(frame)) => {
                if let Some((_, payload)) = decode_frame(&frame) {
                    got.extend_from_slice(payload);
                }
            }
            _ => break,
        }
        let t = text_all(&got);
        if t.contains("40 100") && t.contains("RHOST_E2E_OK") {
            break;
        }
    }

    session.shutdown();
    // shutdown 后写应失败（后台任务退出，通道关闭）
    tokio::time::sleep(Duration::from_millis(100)).await;

    let text = String::from_utf8_lossy(&got);
    assert!(
        text.contains("RHOST_E2E_OK"),
        "未收到命令回显，帧流: {text}"
    );
    assert!(
        text.contains("40 100"),
        "window-change 未生效（stty size 应输出 40 100），帧流: {text}"
    );
}

/// 彩色提示符注入：后端在 PTY 开启后自动注入 source 命令，初始化输出被 hold 至
/// 脚本末尾的 OSC marker，同帧放行「回显 + ANSI 清行 + marker + 着色 PS1」全过程。
/// 断言：注入回显与清行序列、marker 在同一帧（前端单帧绘制最终画面，无中间态闪烁）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_color_prompt_inject_selferase() {
    let (session, mut frame_rx) = match SshSession::connect(test_cfg(), "e2e001").await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("跳过：测试容器不可用（{e}）");
            return;
        }
    };

    // 后端已自动注入：逐帧收集直到 marker 出现（hold 放行帧）或超时
    const MARKER: &[u8] = b"\x1b]6666;rhinit\x07";
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let mut released = false;
    while tokio::time::Instant::now() < deadline && !released {
        match tokio::time::timeout(Duration::from_secs(3), frame_rx.recv()).await {
            Ok(Some(frame)) => {
                if let Some((_, payload)) = decode_frame(&frame) {
                    frames.push(payload.to_vec());
                    if payload.windows(MARKER.len()).any(|w| w == MARKER) {
                        released = true;
                    }
                }
            }
            _ => break,
        }
    }
    // 再等一个攒包窗口，让紧随 marker 的着色 PS1 帧到达
    tokio::time::sleep(Duration::from_millis(300)).await;
    while let Ok(Some(frame)) =
        tokio::time::timeout(Duration::from_millis(300), frame_rx.recv()).await
    {
        if let Some((_, payload)) = decode_frame(&frame) {
            frames.push(payload.to_vec());
        }
    }

    session.shutdown();

    assert!(released, "应在 8s 内收到初始化完成 marker（hold 放行）");
    let marker_frame = frames
        .iter()
        .find(|p| p.windows(MARKER.len()).any(|w| w == MARKER))
        .expect("marker 帧存在");
    let text = String::from_utf8_lossy(marker_frame);
    assert!(
        text.contains(".ri-"),
        "放行帧应含注入命令回显（与清行序列同帧，前端不可见）: {text:?}"
    );
    assert!(
        text.contains("\u{1b}[1A\r\u{1b}[2K"),
        "放行帧应含 ANSI 清行序列: {text:?}"
    );
    // 着色 PS1（绿色 user@host 的 SGR 序列）在 marker 同帧或紧随其后的帧内
    let all: Vec<u8> = frames.concat();
    let tail = String::from_utf8_lossy(&all);
    assert!(
        tail.contains("\u{1b}[1;32m"),
        "应出现着色后的 PS1（SGR 亮绿）: {tail:?}"
    );
}

/// RTT 探测：连接建立后收到 0x07 帧（JSON {"ms": ...}），且周期帧持续到达。
/// 验证 russh global-request ping 在真实 sshd 上工作。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_rtt_frame_received() {
    let (session, mut frame_rx) = match SshSession::connect(test_cfg(), "e2e001").await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("跳过：测试容器不可用（{e}）");
            return;
        }
    };

    let mut rtt_values = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(40);
    while tokio::time::Instant::now() < deadline && rtt_values.len() < 2 {
        match tokio::time::timeout(Duration::from_secs(35), frame_rx.recv()).await {
            Ok(Some(frame)) => {
                if let Some((0x07, payload)) = decode_frame(&frame) {
                    let v: serde_json::Value =
                        serde_json::from_slice(payload).expect("RTT JSON 解析");
                    rtt_values.push(v["ms"].as_u64().expect("ms 字段"));
                }
            }
            _ => break,
        }
    }

    session.shutdown();

    assert!(!rtt_values.is_empty(), "未收到 RTT 帧（0x07）");
    // 本地容器 RTT 极小（< 50ms），上限只校验量纲合理
    assert!(
        rtt_values.iter().all(|&ms| ms < 5000),
        "RTT 异常: {rtt_values:?}"
    );
    assert!(
        rtt_values.len() >= 2,
        "30s 周期内未收到第二帧，周期探测可能失效"
    );
}

/// MOTD：认证成功后、PTY 数据之前，客户端经并行 exec 通道采集状态，
/// 以独立 Motd 帧（0x04，JSON 指令数组）作为二进制通道首帧发出。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_motd_panel_injected() {
    let mut cfg = test_cfg();
    cfg.motd = true;
    let (session, mut frame_rx) = match SshSession::connect(cfg, "e2e001").await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("跳过：测试容器不可用（{e}）");
            return;
        }
    };

    let mut motd_text = String::new();
    let mut saw_data_before_motd = false;
    let mut got_motd = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    while tokio::time::Instant::now() < deadline && !got_motd {
        if let Ok(Some(frame)) =
            tokio::time::timeout(Duration::from_millis(500), frame_rx.recv()).await
        {
            let Some((ftype, payload)) = decode_frame(&frame) else {
                continue;
            };
            match ftype {
                0x04 => {
                    // Motd 帧：payload 是 [{t,text,cls}] JSON
                    let cmds: Vec<MotdCmd> =
                        serde_json::from_slice(payload).expect("MOTD JSON 解析");
                    assert!(cmds.len() > 10, "MOTD 指令数量异常: {}", cmds.len());
                    motd_text = cmds
                        .iter()
                        .map(|c| c.text.clone())
                        .collect::<Vec<_>>()
                        .join("\n");
                    got_motd = true;
                }
                0x01 => saw_data_before_motd = true,
                _ => {}
            }
        }
    }

    assert!(got_motd, "未收到 Motd 首帧");
    assert!(
        !saw_data_before_motd,
        "PTY Data 帧出现在 Motd 帧之前，顺序被破坏"
    );
    assert!(motd_text.contains("Rhost"), "MOTD 缺少横幅: {motd_text}");
    assert!(
        motd_text.contains("System load"),
        "MOTD 缺少状态行: {motd_text}"
    );

    // 抑制路径以 exec '<shell>' -l 启动：验证 PTY 中的 shell 仍可交互，
    // 且确实是登录 shell（bash: shopt login_shell；容器用户 shell 为 /bin/bash）。
    session
        .write(b"shopt -q login_shell && echo RHOST_LOGIN_OK; echo RHOST_INTERACT_OK\r".to_vec())
        .await
        .expect("写入探测命令");

    let mut pty = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(1000), frame_rx.recv()).await {
            Ok(Some(frame)) => {
                if let Some((0x01, payload)) = decode_frame(&frame) {
                    pty.extend_from_slice(payload);
                }
            }
            _ => break,
        }
        let t = String::from_utf8_lossy(&pty);
        if t.contains("RHOST_LOGIN_OK") && t.contains("RHOST_INTERACT_OK") {
            break;
        }
    }
    let pty_text = String::from_utf8_lossy(&pty);
    assert!(
        pty_text.contains("RHOST_INTERACT_OK"),
        "抑制路径 shell 不可交互，帧流: {pty_text}"
    );
    assert!(
        pty_text.contains("RHOST_LOGIN_OK"),
        "exec '<shell>' -l 未以登录模式启动，帧流: {pty_text}"
    );

    session.shutdown();
}

/// HostInfo（0x05）：紧随 MOTD 首帧之后、PTY Data 之前到达，内容为真实采集的
/// 主机静态信息（JSON）。motd 关闭时不发该帧（复用同一采集批，无额外通道）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_host_info_frame_order_and_content() {
    let mut cfg = test_cfg();
    cfg.motd = true;
    let (session, mut frame_rx) = match SshSession::connect(cfg, "e2e001").await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("跳过：测试容器不可用（{e}）");
            return;
        }
    };

    let mut got_motd = false;
    let mut saw_data = false;
    let mut info: Option<serde_json::Value> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    // 收到首帧 PTY Data 后再补取几帧即可结束（0x05 必在其之前入队）
    while tokio::time::Instant::now() < deadline && info.is_none() {
        if let Ok(Some(frame)) =
            tokio::time::timeout(Duration::from_millis(500), frame_rx.recv()).await
        {
            let Some((ftype, payload)) = decode_frame(&frame) else {
                continue;
            };
            match ftype {
                0x04 => {
                    assert!(!saw_data, "MOTD 帧出现在 PTY Data 之后");
                    got_motd = true;
                }
                0x05 => {
                    assert!(got_motd, "HostInfo 帧出现在 MOTD 帧之前，顺序被破坏");
                    assert!(!saw_data, "HostInfo 帧出现在 PTY Data 之后，顺序被破坏");
                    info = Some(serde_json::from_slice(payload).expect("HostInfo JSON 解析"));
                }
                0x01 => saw_data = true,
                _ => {}
            }
        }
    }

    let info = info.expect("未收到 HostInfo(0x05) 帧");
    assert_eq!(info["os"], "Linux");
    assert!(
        info["kernel"]
            .as_str()
            .is_some_and(|s| !s.is_empty() && s != "N/A"),
        "内核版本缺失: {}",
        info["kernel"]
    );
    assert_ne!(info["arch"], "N/A");
    assert!(!info["distro"].as_str().unwrap_or("").is_empty());
    assert_eq!(info["procfs"], true, "Linux 容器应探测到 /proc");
    // 测试容器跑在 Apple Silicon 虚拟化环境（aarch64，仅暴露 implementer 0x61）
    assert_eq!(info["cpu_model"], "Apple Silicon", "ARM CPU 型号映射异常");

    session.shutdown();
}

/// motd=false 时不产生 0x04/0x05 帧，PTY 仍正常（开关隔离不回归）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_no_host_info_when_motd_disabled() {
    let cfg = test_cfg(); // motd: false
    let (session, mut frame_rx) = match SshSession::connect(cfg, "e2e001").await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("跳过：测试容器不可用（{e}）");
            return;
        }
    };

    let mut saw_special = false;
    let mut saw_data = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline && !saw_data {
        if let Ok(Some(frame)) =
            tokio::time::timeout(Duration::from_millis(500), frame_rx.recv()).await
            && let Some((ftype, _)) = decode_frame(&frame)
        {
            if ftype == 0x04 || ftype == 0x05 {
                saw_special = true;
            }
            if ftype == 0x01 {
                saw_data = true;
            }
        }
    }
    assert!(saw_data, "motd 关闭后 PTY 数据帧应正常到达");
    assert!(!saw_special, "motd 关闭时不应发送 0x04/0x05 帧");
    session.shutdown();
}

/// 会话建立（PTY 已开启）后，保留的 handle 仍可打开独立 exec 通道执行采集：
/// 正常返回输出；超时返回 Err 且远端命令被丢弃；之后 PTY 交互不受影响。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_exec_collect_after_connect() {
    let (session, mut frame_rx) = match SshSession::connect(test_cfg(), "e2e001").await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("跳过：测试容器不可用（{e}）");
            return;
        }
    };

    // 1. 正常采集：stdout 合并返回
    let out = session
        .exec_collect("echo RHOST_EXEC_OK; uname -srm", Duration::from_secs(3))
        .await
        .expect("exec 采集应成功");
    assert!(out.contains("RHOST_EXEC_OK"), "exec 输出异常: {out}");
    assert!(out.contains("Linux"), "uname 输出异常: {out}");

    // 2. 超时：慢命令必须在指定时间返回 Err（通道随 future drop 关闭）
    let timed_out = session
        .exec_collect("sleep 2", Duration::from_millis(200))
        .await
        .is_err();
    assert!(timed_out, "exec 采集未按超时取消");

    // 3. exec 与超时之后 PTY 终端仍可正常交互（通道物理隔离）
    session
        .write(b"echo RHOST_PTY_STILL_OK\r".to_vec())
        .await
        .expect("写入命令");
    let mut pty = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(1000), frame_rx.recv()).await {
            Ok(Some(frame)) => {
                if let Some((0x01, payload)) = decode_frame(&frame) {
                    pty.extend_from_slice(payload);
                }
            }
            _ => break,
        }
        if String::from_utf8_lossy(&pty).contains("RHOST_PTY_STILL_OK") {
            break;
        }
    }
    assert!(
        String::from_utf8_lossy(&pty).contains("RHOST_PTY_STILL_OK"),
        "exec 采集后 PTY 交互异常"
    );

    session.shutdown();
}

/// MOTD 帧 JSON 结构（仅测试反序列化用）
#[derive(serde::Deserialize)]
struct MotdCmd {
    text: String,
}

/// 在 frame_rx 上等到 0x02 Exit 帧，返回解析后的 JSON payload。
/// 其它帧（PTY Data/Metrics 等）丢弃。
async fn recv_exit(
    frame_rx: &mut tokio::sync::mpsc::Receiver<Vec<u8>>,
    window: Duration,
) -> Option<serde_json::Value> {
    let deadline = tokio::time::Instant::now() + window;
    while tokio::time::Instant::now() < deadline {
        let remain = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remain, frame_rx.recv()).await {
            Ok(Some(frame)) => {
                if let Some((0x02, payload)) = decode_frame(&frame) {
                    return Some(serde_json::from_slice(payload).expect("Exit JSON 解析"));
                }
            }
            _ => break,
        }
    }
    None
}

/// 写标记命令并在 PTY Data 帧流中等到该标记回显（确认 shell 已可交互，
/// 避开连接初期的彩色提示符注入窗口）。
async fn wait_shell_echo(
    session: &SshSession,
    frame_rx: &mut tokio::sync::mpsc::Receiver<Vec<u8>>,
    marker: &str,
) {
    session
        .write(format!("echo {marker}\r").into_bytes())
        .await
        .expect("写入同步命令");
    let mut pty = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(1000), frame_rx.recv()).await {
            Ok(Some(frame)) => {
                if let Some((0x01, payload)) = decode_frame(&frame) {
                    pty.extend_from_slice(payload);
                }
            }
            _ => break,
        }
        if String::from_utf8_lossy(&pty).contains(marker) {
            return;
        }
    }
    panic!(
        "未等到 shell 回显 {marker}，帧流: {:?}",
        String::from_utf8_lossy(&pty)
    );
}

/// 正常退出（PTY 执行 exit）：sshd 先发送 ExitStatus，Exit 帧必须标记 lost=false，
/// 前端据此判定"用户主动退出"而不触发断线自动重连。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_exit_frame_clean_exit_not_lost() {
    let (session, mut frame_rx) = match SshSession::connect(test_cfg(), "e2e001").await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("跳过：测试容器不可用（{e}）");
            return;
        }
    };

    wait_shell_echo(&session, &mut frame_rx, "RHOST_EXIT_SYNC").await;
    session.write(b"exit\r".to_vec()).await.expect("写入 exit");

    let exit = recv_exit(&mut frame_rx, Duration::from_secs(10))
        .await
        .expect("正常 exit 后未收到 Exit 帧");
    assert_eq!(
        exit["lost"], false,
        "远端正常退出（ExitStatus）不应标记为连接丢失: {exit}"
    );
    assert!(
        exit["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("状态码 0"),
        "reason 应含退出状态码: {exit}"
    );

    // 连接已由远端关闭，shutdown 仅做本地任务清理（幂等安全）
    session.shutdown();
}

/// 连接意外丢失（SIGKILL 当前会话的 per-connection sshd，无 ExitStatus 通道）：
/// russh 只能收到 Eof/Close，Exit 帧必须标记 lost=true，前端据此触发自动重连。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_exit_frame_killed_connection_is_lost() {
    let (session, mut frame_rx) = match SshSession::connect(test_cfg(), "e2e001").await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("跳过：测试容器不可用（{e}）");
            return;
        }
    };

    wait_shell_echo(&session, &mut frame_rx, "RHOST_KILL_SYNC").await;
    // 交互 shell 的父进程即 per-connection sshd（已降权为 test，可被自身 SIGKILL）；
    // sshd 被瞬间杀死，来不及发送 ExitStatus/ExitSignal，TCP 直接关闭
    session
        .write(b"kill -9 $PPID\r".to_vec())
        .await
        .expect("写入 kill");

    let exit = recv_exit(&mut frame_rx, Duration::from_secs(10))
        .await
        .expect("连接被杀死后未收到 Exit 帧");
    assert_eq!(
        exit["lost"], true,
        "无 ExitStatus 的连接中断必须标记为连接丢失（lost=true）: {exit}"
    );

    session.shutdown();
}

/// 密码错误时应返回认证失败
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_auth_failure() {
    let mut cfg = test_cfg();
    cfg.auth = AuthMethod::Password("wrong-password".into());
    match SshSession::connect(cfg, "e2e001").await {
        Err(e) => assert!(e.to_string().contains("认证失败"), "错误类型不符: {e}"),
        Ok(_) => {
            eprintln!("跳过：测试容器不可用");
        }
    }
}

/// 主机密钥 TOFU 信任路径（hostkey-verification-design.md §8）：
/// ① Verify + trust=false 首连 → 失败，错误串为 `HOSTKEY_UNKNOWN: {algo}|{fp}`
///（端到端验证错误协议）；② 解析指纹后以 trust=true + 该指纹重试 → 成功；
/// ③ 断言指纹已按 schema 落盘到临时文件路径。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_hostkey_tofu_trust_and_persist() {
    // 独立临时 known_hosts，避免污染真实应用数据目录
    let kh = std::env::temp_dir().join(format!("rhost-kh-e2e-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&kh);

    let mut cfg = test_cfg();
    cfg.host_key = HostKeyPolicy::Verify(HostKeyCheck {
        host: "127.0.0.1".into(),
        port: 2222,
        stored: None,
        trust: false,
        trust_fp: None,
        persist: true,
        path: kh.clone(),
    });

    // ① 首连：未信任指纹必须失败，且错误为协议前缀 + `algo|fp` payload
    let first = match SshSession::connect(cfg.clone(), "e2e001").await {
        Err(e) => e.to_string(),
        Ok(_) => panic!("首连未信任指纹不应成功（不存在静默接受路径）"),
    };
    if !first.starts_with("HOSTKEY_UNKNOWN:") {
        // 非 HOSTKEY 错误 = 容器缺失等环境问题，跳过（不误报）
        eprintln!("跳过：测试容器不可用（{first}）");
        let _ = std::fs::remove_file(&kh);
        return;
    }
    let payload = first["HOSTKEY_UNKNOWN:".len()..].trim();
    // payload 三段：`algo|fingerprint|pubkey`（pubkey 含空格但不含 `|`）
    let mut parts = payload.split('|');
    let algo = parts.next().expect("payload 缺少 algo");
    let fp = parts.next().expect("payload 缺少 fingerprint");
    let _pubkey = parts.next().expect("payload 缺少 pubkey");
    assert!(algo.starts_with("ssh-"), "算法名异常: {algo}");
    assert!(fp.starts_with("SHA256:"), "指纹应为 SHA256 形态: {fp}");
    assert!(
        _pubkey.starts_with(&format!("{algo} ")),
        "pubkey 应以 `{algo} ` 开头: {_pubkey}"
    );
    assert!(!kh.exists(), "首连拒绝时不得落盘");

    // ② trust=true + 弹窗确认的指纹（= 第一次握手实际看到的）重试 → 成功
    cfg.host_key = HostKeyPolicy::Verify(HostKeyCheck {
        host: "127.0.0.1".into(),
        port: 2222,
        stored: None,
        trust: true,
        trust_fp: Some(fp.to_string()),
        persist: true,
        path: kh.clone(),
    });
    let (session, _frame_rx) = match SshSession::connect(cfg, "e2e001").await {
        Ok(v) => v,
        Err(e) => panic!("信任重试应成功（fp={fp}）: {e}"),
    };
    session.shutdown();

    // ③ 断言落盘：schema 字段齐全且指纹与确认值一致（确认的 = 落盘的）
    let raw = std::fs::read_to_string(&kh).expect("信任成功后 known_hosts 应已落盘");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("落盘文件应为合法 JSON");
    assert_eq!(v["version"], 1, "schema 版本应为 1: {raw}");
    let entries = v["entries"].as_array().expect("entries 数组");
    assert_eq!(entries.len(), 1, "应恰好一条记录: {raw}");
    assert_eq!(entries[0]["host"], "127.0.0.1");
    assert_eq!(entries[0]["port"], 2222);
    assert_eq!(entries[0]["algo"], algo);
    assert_eq!(
        entries[0]["fingerprint"], fp,
        "落盘指纹必须与确认值逐字一致"
    );
    assert!(
        entries[0]["addedAt"].as_i64().is_some(),
        "缺少 addedAt 时间戳"
    );

    // 收尾：再连一次（stored 命中路径）应无感通过，且不产生第二条记录
    let mut cfg2 = test_cfg();
    cfg2.host_key = HostKeyPolicy::Verify(HostKeyCheck {
        host: "127.0.0.1".into(),
        port: 2222,
        stored: Some((algo.to_string(), fp.to_string())),
        trust: false,
        trust_fp: None,
        persist: true,
        path: kh.clone(),
    });
    match SshSession::connect(cfg2, "e2e001").await {
        Ok((session, _)) => session.shutdown(),
        Err(e) => panic!("已存指纹命中路径应无感通过: {e}"),
    }
    let raw2 = std::fs::read_to_string(&kh).unwrap();
    assert_eq!(raw, raw2, "Match 路径不得改写落盘文件");
    let _ = std::fs::remove_file(&kh);
}

/// 「仅本次连接」路径（hostkey-verification-design.md §6.5）：
/// trust=true + persist=false → 连接成功但绝不落盘；下次新建会话
/// （trust=false、stored=None）仍重新进入首连确认。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_hostkey_trust_once_does_not_persist() {
    // 独立临时 known_hosts，避免污染真实应用数据目录
    let kh = std::env::temp_dir().join(format!("rhost-kh-once-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&kh);

    // ① 首连拿一次指纹：未信任失败；非 HOSTKEY 错误 = 容器缺失，跳过
    let mut cfg = test_cfg();
    cfg.host_key = HostKeyPolicy::Verify(HostKeyCheck {
        host: "127.0.0.1".into(),
        port: 2222,
        stored: None,
        trust: false,
        trust_fp: None,
        persist: true,
        path: kh.clone(),
    });
    let first = match SshSession::connect(cfg, "e2e001").await {
        Err(e) => e.to_string(),
        Ok(_) => panic!("首连未信任指纹不应成功（不存在静默接受路径）"),
    };
    if !first.starts_with("HOSTKEY_UNKNOWN:") {
        eprintln!("跳过：测试容器不可用（{first}）");
        return;
    }
    let (_algo, fp) = {
        let payload = first["HOSTKEY_UNKNOWN:".len()..].trim();
        let mut parts = payload.split('|');
        let algo = parts.next().expect("payload 缺少 algo").to_string();
        let fp = parts.next().expect("payload 缺少 fingerprint").to_string();
        (algo, fp)
    };

    // ② 仅本次连接：trust=true + persist=false → 成功且不落盘
    let mut cfg2 = test_cfg();
    cfg2.host_key = HostKeyPolicy::Verify(HostKeyCheck {
        host: "127.0.0.1".into(),
        port: 2222,
        stored: None,
        trust: true,
        trust_fp: Some(fp.to_string()),
        persist: false,
        path: kh.clone(),
    });
    match SshSession::connect(cfg2, "e2e001").await {
        Ok((session, _)) => session.shutdown(),
        Err(e) => panic!("仅本次连接重试应成功: {e}"),
    }
    assert!(!kh.exists(), "persist=false 不得写入 known_hosts");

    // ③ 下次新建会话（trust=false）应重新进入首连确认，而非命中记录
    let mut cfg3 = test_cfg();
    cfg3.host_key = HostKeyPolicy::Verify(HostKeyCheck {
        host: "127.0.0.1".into(),
        port: 2222,
        stored: None,
        trust: false,
        trust_fp: None,
        persist: true,
        path: kh.clone(),
    });
    match SshSession::connect(cfg3, "e2e001").await {
        Err(e) => assert!(
            e.to_string().starts_with("HOSTKEY_UNKNOWN:"),
            "应重新进入首连确认: {e}"
        ),
        Ok(_) => panic!("仅本次连接不应留下可命中的落盘记录"),
    }
    let _ = std::fs::remove_file(&kh);
}

/// 0x06 Metrics 帧 JSON（仅测试反序列化用）
#[derive(serde::Deserialize)]
struct MetricsFrame {
    seq: u64,
    cpu: MetricsCpu,
    mem: MetricsMem,
    net: MetricsNet,
    disks: Vec<FrameDisk>,
    procs: FrameProcs,
    gpus: Vec<FrameGpu>,
}
#[derive(Debug, serde::Deserialize)]
struct FrameDisk {
    mount: String,
    total: u64,
    used: u64,
    available: u64,
    pct: u8,
}
#[derive(Debug, serde::Deserialize)]
struct FrameProcs {
    total: usize,
    top: Vec<FrameProc>,
}
#[derive(Debug, serde::Deserialize)]
struct FrameProc {
    pid: u32,
    name: String,
    cpu: f32,
    rss: u64,
}
#[derive(Debug, serde::Deserialize)]
struct FrameGpu {
    #[allow(dead_code)]
    index: u32,
    #[allow(dead_code)]
    name: String,
    #[allow(dead_code)]
    util: f32,
    #[allow(dead_code)]
    mem_used: u64,
    #[allow(dead_code)]
    mem_total: u64,
    #[allow(dead_code)]
    temp: f32,
    #[allow(dead_code)]
    power: f32,
    #[allow(dead_code)]
    power_limit: f32,
}
#[derive(Debug, serde::Deserialize)]
struct MetricsCpu {
    util: f32,
    /// 1/5/15 分钟负载；反序列化即校验固定 3 元素
    load: Option<[f32; 3]>,
}
#[derive(Debug, serde::Deserialize)]
struct MetricsMem {
    total: u64,
    available: u64,
    used: u64,
    swap_total: u64,
    swap_free: u64,
    swap_used: u64,
}
#[derive(Debug, serde::Deserialize)]
struct MetricsNet {
    rx_rate: f64,
    tx_rate: f64,
    rx_bytes: u64,
    tx_bytes: u64,
}

/// 在 frame_rx 上等到下一帧 0x06；其它帧（PTY Data 等）丢弃
async fn recv_metrics(
    frame_rx: &mut tokio::sync::mpsc::Receiver<Vec<u8>>,
    window: Duration,
) -> Option<MetricsFrame> {
    let deadline = tokio::time::Instant::now() + window;
    while tokio::time::Instant::now() < deadline {
        let remain = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remain, frame_rx.recv()).await {
            Ok(Some(frame)) => {
                if let Some((0x06, payload)) = decode_frame(&frame) {
                    return Some(serde_json::from_slice(payload).expect("Metrics JSON 解析"));
                }
            }
            _ => break,
        }
    }
    None
}

/// 动态指标采集：start 后按间隔收到 0x06 帧，stop 后停止，重复 start 可恢复
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_metrics_collector_lifecycle() {
    let (session, mut frame_rx) = match SshSession::connect(test_cfg(), "e2e001").await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("跳过：测试容器不可用（{e}）");
            return;
        }
    };
    let session = std::sync::Arc::new(session);

    // 1. 启动采集（1s 为后端允许的最小间隔；容器默认网卡 eth0）
    session.clone().start_metrics(1000, Some("eth0".into()));
    session.metrics_heartbeat();

    // 2. 应在短时间内连续收到指标帧（首帧 CPU 无前置快照 util=0，取到第二帧）
    let first = recv_metrics(&mut frame_rx, Duration::from_secs(8))
        .await
        .expect("未收到首个 Metrics 帧");
    assert_eq!(first.seq, 1, "首帧 seq 应为 1");
    assert!(first.mem.total > 0, "MemTotal 应大于 0: {:?}", first.mem);
    assert!(first.mem.used <= first.mem.total, "used 不应超过 total");
    assert!(
        first.mem.available <= first.mem.total,
        "available 不应超过 total"
    );
    assert!(
        (0.0..=100.0).contains(&first.cpu.util),
        "util 越界: {}",
        first.cpu.util
    );

    // loadavg 在 Linux 容器必然存在：1/5/15 三值均非负
    let load = first.cpu.load.expect("应采集到 /proc/loadavg");
    assert_eq!(load.len(), 3);
    assert!(load.iter().all(|v| *v >= 0.0), "负载不应为负: {load:?}");

    // Swap 恒等式：free/used 均不超过 total（swap_total=0 的主机两者也为 0）
    assert!(
        first.mem.swap_free <= first.mem.swap_total,
        "swap_free 越界: {:?}",
        first.mem
    );
    assert!(
        first.mem.swap_used <= first.mem.swap_total,
        "swap_used 越界: {:?}",
        first.mem
    );

    // 网络首帧：容器 eth0 必有累计字节；速率因无前置快照为 0，且所有值非负有限
    assert!(
        first.net.rx_bytes > 0,
        "eth0 累计接收字节应大于 0: {:?}",
        first.net
    );
    assert!(
        first.net.tx_bytes > 0,
        "eth0 累计发送字节应大于 0: {:?}",
        first.net
    );
    assert_eq!(first.net.rx_rate, 0.0, "首帧 rx_rate 应为 0");
    assert_eq!(first.net.tx_rate, 0.0, "首帧 tx_rate 应为 0");
    assert!(
        first.net.rx_rate.is_finite() && first.net.tx_rate.is_finite(),
        "速率必须有限"
    );

    // 磁盘首帧（第 0 轮无条件夹带 df）：容器根分区 overlay / 必在且数值自洽，
    // tmpfs/shm 等伪 fs 与 /etc/hosts 注入挂载不得出现
    let root_disk = first
        .disks
        .iter()
        .find(|d| d.mount == "/")
        .unwrap_or_else(|| panic!("首帧磁盘列表应含根分区 /: {:?}", first.disks));
    assert!(root_disk.total > 0, "磁盘 total 应大于 0: {root_disk:?}");
    assert!(
        root_disk.used <= root_disk.total,
        "磁盘 used 越界: {root_disk:?}"
    );
    assert!(
        root_disk.available <= root_disk.total,
        "磁盘 available 越界: {root_disk:?}"
    );
    assert!(root_disk.pct <= 100, "磁盘 pct 越界: {root_disk:?}");
    assert!(
        !first
            .disks
            .iter()
            .any(|d| d.mount == "/etc/hosts" || d.mount == "/dev/shm"),
        "伪 fs/注入挂载未过滤干净: {:?}",
        first.disks.iter().map(|d| &d.mount).collect::<Vec<_>>()
    );

    let second = recv_metrics(&mut frame_rx, Duration::from_secs(5))
        .await
        .expect("未收到第二个 Metrics 帧");
    assert_eq!(second.seq, 2, "第二帧 seq 应为 2");
    assert!(second.mem.total > 0);

    // 第二帧为非夹带轮（tick=1）：脚本不带 df，磁盘列表沿用上一次缓存，不闪空
    assert_eq!(
        second.disks.len(),
        first.disks.len(),
        "非夹带轮磁盘应沿用缓存: first={:?} second={:?}",
        first.disks.iter().map(|d| &d.mount).collect::<Vec<_>>(),
        second.disks.iter().map(|d| &d.mount).collect::<Vec<_>>()
    );

    // 进程：容器必有进程；首帧无 per-pid 前置快照，榜单 CPU 全为 0
    assert!(first.procs.total > 0, "进程总数应大于 0: {:?}", first.procs);
    assert!(
        first.procs.top.len() <= 5,
        "Top 榜单最多 5 条: {:?}",
        first.procs
    );
    assert!(!first.procs.top.is_empty(), "容器至少有 1 个进程可上榜");
    for p in &first.procs.top {
        assert!(!p.name.is_empty(), "进程名不应为空: {p:?}");
        assert_eq!(p.cpu, 0.0, "首帧进程 CPU 应为 0: {p:?}");
        // rss = 页数 × PAGESIZE(4096)，必为页对齐
        assert_eq!(p.rss % 4096, 0, "rss 未按页对齐: {p:?}");
    }

    // 第二帧：总数仍为正，榜单不超过 5，CPU 非负有限（容器空闲故通常接近 0），
    // 相邻帧 pid 不重复
    assert!(second.procs.total > 0);
    assert!(second.procs.top.len() <= 5);
    let mut pids = second.procs.top.iter().map(|p| p.pid).collect::<Vec<_>>();
    pids.sort_unstable();
    let len_before = pids.len();
    pids.dedup();
    assert_eq!(pids.len(), len_before, "榜单出现重复 pid: {pids:?}");
    assert!(
        second
            .procs
            .top
            .iter()
            .all(|p| p.cpu >= 0.0 && p.cpu.is_finite()),
        "进程 CPU 越界: {:?}",
        second.procs.top
    );

    // GPU：测试容器无 nvidia-smi，首帧静态探测即为空 → gpus 恒为空数组，
    // 前端区块隐藏；探测后后续轮次不再带 GPU 段（帧字段仍存在且为空）
    assert!(
        first.gpus.is_empty(),
        "无 GPU 容器首帧不应出现显卡: {:?}",
        first.gpus
    );
    assert!(
        second.gpus.is_empty(),
        "无 GPU 容器次帧 gpus 应仍为空: {:?}",
        second.gpus
    );

    // 第二帧：累计字节单调不减，速率为非负有限值（容器空闲故通常接近 0）
    assert!(
        second.net.rx_bytes >= first.net.rx_bytes,
        "rx_bytes 倒退: {:?}",
        second.net
    );
    assert!(
        second.net.tx_bytes >= first.net.tx_bytes,
        "tx_bytes 倒退: {:?}",
        second.net
    );
    assert!(
        second.net.rx_rate >= 0.0 && second.net.rx_rate.is_finite(),
        "rx_rate 异常"
    );
    assert!(
        second.net.tx_rate >= 0.0 && second.net.tx_rate.is_finite(),
        "tx_rate 异常"
    );

    // 3. stop 后 2.5s（>2 个采集间隔）内不应再有指标帧
    session.stop_metrics();
    let leaked = recv_metrics(&mut frame_rx, Duration::from_millis(2500)).await;
    assert!(
        leaked.is_none(),
        "stop 后仍收到 Metrics 帧: {:?}",
        leaked.map(|f| f.seq)
    );

    // 4. 重新启动（幂等路径）后恢复推送；新任务 seq 重新从 1 计数
    session.clone().start_metrics(1000, Some("eth0".into()));
    let restarted = recv_metrics(&mut frame_rx, Duration::from_secs(8))
        .await
        .expect("重启后未恢复 Metrics 帧");
    assert_eq!(restarted.seq, 1, "重启后 seq 应重新从 1 开始");

    session.shutdown();
}

/// 在 frame_rx 上等到首帧 0x06，返回其 payload 字节数（压测：帧大小红线 < 4KB）
async fn first_metrics_payload_len(
    frame_rx: &mut tokio::sync::mpsc::Receiver<Vec<u8>>,
    window: Duration,
) -> Option<usize> {
    let deadline = tokio::time::Instant::now() + window;
    while tokio::time::Instant::now() < deadline {
        let remain = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remain, frame_rx.recv()).await {
            Ok(Some(frame)) => {
                if let Some((0x06, payload)) = decode_frame(&frame) {
                    return Some(payload.len());
                }
            }
            _ => break,
        }
    }
    None
}

/// 轮询等待全局在飞 exec 归零（断连清理无残留）。并行运行的其它用例只可能造成瞬时
/// 非零（单轮 exec 几十毫秒），因此判据是「期限内至少回到 0」而非恒为 0。
async fn wait_inflight_drain(window: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + window;
    while tokio::time::Instant::now() < deadline {
        if rhost_lib::ssh::session::metric_exec_concurrency().0 == 0 {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

/// 断连清理：未显式 stop、直接 shutdown（模拟掉线/关窗）后采集任务随之取消，
/// 不再产生帧、在飞 exec 排空，无残留任务。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_metrics_cancelled_on_shutdown() {
    let (session, mut frame_rx) = match SshSession::connect(test_cfg(), "e2e001").await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("跳过：测试容器不可用（{e}）");
            return;
        }
    };
    let session = std::sync::Arc::new(session);
    session.clone().start_metrics(1000, None);

    // 连续两帧确认采集确实在跑
    let f1 = recv_metrics(&mut frame_rx, Duration::from_secs(8))
        .await
        .expect("未收到首帧");
    let f2 = recv_metrics(&mut frame_rx, Duration::from_secs(5))
        .await
        .expect("未收到次帧");
    assert_eq!(f1.seq, 1);
    assert_eq!(f2.seq, 2);

    // 模拟异常断开：不显式 stop_metrics，直接取消整棵任务树
    session.shutdown();

    // 之后 2.5s 内不应再收到任何 Metrics 帧（通道关闭或静默都应是 None，收到即任务残留）
    let late = recv_metrics(&mut frame_rx, Duration::from_millis(2500)).await;
    assert!(
        late.is_none(),
        "shutdown 后仍收到 Metrics 帧，采集任务未随会话取消"
    );
    assert!(
        wait_inflight_drain(Duration::from_secs(3)).await,
        "shutdown 后在飞 exec 未排空，存在通道/任务泄漏"
    );
}

/// 高并发压测：100 个会话同时启动采集，验证全局信号量把启动峰值 exec 压在 ≤24、
/// 闸门排队不造成饥饿（全部会话都能在期限内拿到首帧）、payload < 4KB、
/// 全部关闭后在飞 exec 归零。
///
/// 属负载测试，默认 `#[ignore]`：其连接风暴会触发 sshd MaxStartups 限流，干扰并行的
/// 普通用例（如 ssh_auth_failure 可能收到 Connect 错误而非认证拒绝）。
/// 显式运行：`cargo test --test ssh_e2e -- --ignored --test-threads=1`
#[ignore]
#[tokio::test(flavor = "multi_thread")]
async fn ssh_metrics_100_sessions_gated() {
    const N: usize = 100;
    const BATCH: usize = 10; // 控制并发握手数，避开 sshd 默认 MaxStartups 随机丢弃

    // 分批建立 100 条真实连接（首条失败即容器缺失，整体跳过）
    let mut sessions: Vec<std::sync::Arc<SshSession>> = Vec::with_capacity(N);
    let mut receivers: Vec<tokio::sync::mpsc::Receiver<Vec<u8>>> = Vec::with_capacity(N);
    let mut connected_ok = true;
    for batch_start in (0..N).step_by(BATCH) {
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..BATCH.min(N - batch_start) {
            set.spawn(SshSession::connect(test_cfg(), "e2e001"));
        }
        while let Some(res) = set.join_next().await {
            match res.unwrap() {
                Ok((s, rx)) => {
                    sessions.push(std::sync::Arc::new(s));
                    receivers.push(rx);
                }
                Err(e) if sessions.is_empty() => {
                    eprintln!("跳过：测试容器不可用（{e}）");
                    return;
                }
                Err(e) => {
                    connected_ok = false;
                    eprintln!("第 {batch_start} 批连接失败: {e}");
                }
            }
        }
    }
    assert!(connected_ok, "建立 100 条连接过程中出现失败");
    assert_eq!(sessions.len(), N, "应建立 {N} 条连接");

    // 全部会话同时启动采集（collector 自带 0~300ms 错峰），并起心跳防止 9s TTL 自停
    let mut hb = tokio::task::JoinSet::new();
    for s in &sessions {
        s.clone().start_metrics(1000, None);
        let s = s.clone();
        hb.spawn(async move {
            for _ in 0..40 {
                tokio::time::sleep(Duration::from_millis(500)).await;
                s.metrics_heartbeat();
            }
        });
    }

    // 每个会话独立等首帧（含 payload 大小），全部须在期限内到达 = 闸门排队无饥饿/无积压
    let mut waiters = tokio::task::JoinSet::new();
    for mut rx in receivers {
        waiters.spawn(
            async move { first_metrics_payload_len(&mut rx, Duration::from_secs(30)).await },
        );
    }
    let mut got = 0usize;
    let mut max_payload = 0usize;
    while let Some(res) = waiters.join_next().await {
        let len = res
            .unwrap()
            .expect("某会话在期限内未收到首帧（闸门排队饥饿/积压）");
        got += 1;
        max_payload = max_payload.max(len);
    }
    assert_eq!(got, N, "应有 {N} 个会话收到首帧，实际 {got}");
    assert!(
        max_payload < 4096,
        "0x06 payload 超过 4KB 红线: {max_payload}B"
    );

    let (_inflight, peak) = rhost_lib::ssh::session::metric_exec_concurrency();
    eprintln!(
        "100 会话压测：实测启动峰值 exec 并发 = {peak}（上限 24），最大 payload = {max_payload}B"
    );
    assert!(peak <= 24, "启动峰值 exec 并发 {peak} 超过信号量上限 24");
    assert!(peak >= 1, "峰值读数异常: {peak}");

    // 全部关闭：在飞 exec 必须排空（无通道泄漏）
    for s in &sessions {
        s.shutdown();
    }
    hb.abort_all();
    assert!(
        wait_inflight_drain(Duration::from_secs(5)).await,
        "100 会话关闭后在飞 exec 未归零"
    );
}

/* ============================================================
 * 端口转发 e2e（-L / -D / -R / next 策略 / 生命周期）
 *
 * 目标服务：容器内用 exec 起一个循环 nc 服务（每个接入客户端收到固定
 * 标记后连接被关闭），随被测会话 shutdown 自动消亡，无需容器预置服务。
 * 前提：容器为 Debian 系（OpenBSD netcat + curl），连接失败即跳过。
 * ============================================================ */

use rhost_lib::ssh::tunnel::{
    DnsResolve, PortConflict, StartOptions, TunnelManager, TunnelRule, TunnelState, TunnelType,
};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/** 起一条本地/远程规则（target 127.0.0.1:target_port） */
fn fwd_rule(id: &str, kind: TunnelType, bind_port: u16, target_port: u16) -> TunnelRule {
    TunnelRule {
        id: id.into(),
        kind,
        bind_host: "127.0.0.1".into(),
        bind_port,
        target_host: Some("127.0.0.1".into()),
        target_port: Some(target_port),
    }
}

/** 起一条动态（SOCKS5）规则 */
fn socks_rule(id: &str, bind_port: u16) -> TunnelRule {
    TunnelRule {
        id: id.into(),
        kind: TunnelType::Dynamic,
        bind_host: "127.0.0.1".into(),
        bind_port,
        target_host: None,
        target_port: None,
    }
}

const STOP_OPTS: StartOptions = StartOptions {
    port_conflict: PortConflict::Stop,
    dns_resolve: DnsResolve::Remote,
    retry_count: 0,
};

/// 轮询等待指定规则达到期望状态，超时返回该规则最终状态（供断言信息）
async fn wait_state(
    tunnel: &TunnelManager,
    rule_id: &str,
    want: TunnelState,
    timeout: Duration,
) -> Option<rhost_lib::ssh::tunnel::TunnelStatus> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let cur = tunnel
            .statuses()
            .await
            .into_iter()
            .find(|s| s.id == rule_id);
        if let Some(st) = &cur {
            if st.state == want {
                return cur;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return cur;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// -L/-D 数据面目标：容器内 sshd 自身端口（连接即回 `SSH-2.0` banner）。
/// 不依赖额外标记服务——容器为 busybox 工具链，`printf | nc -lk` 在 stdin EOF
/// 后不发数据即退出，无法作为可靠标记源；sshd 并发能力也足以支撑闸门压测。
const SSHD_TARGET_PORT: u16 = 2222;
const BANNER: &[u8] = b"SSH-2.0";

/// 连接 127.0.0.1:port 读取服务端首包（如 sshd banner）。
/// 连不上返回 None；连上后对端立即关闭/被闸门拒绝时返回空数据。
async fn probe_banner(port: u16, timeout: Duration) -> Option<Vec<u8>> {
    use tokio::io::AsyncReadExt as _;
    let mut stream = tokio::time::timeout(timeout, TcpStream::connect(("127.0.0.1", port)))
        .await
        .ok()?
        .ok()?;
    let mut buf = [0u8; 256];
    let n = tokio::time::timeout(timeout, stream.read(&mut buf))
        .await
        .ok()?
        .unwrap_or(0);
    Some(buf[..n].to_vec())
}

/// SOCKS5 一次性客户端：无认证握手 → CONNECT 请求（IPv4/域名）→ 读响应与后续数据。
/// 返回 (REP 码, 连接建立后的全部数据)。
async fn socks_connect(
    port: u16,
    target: SocksTarget<'_>,
    timeout: Duration,
) -> Result<(u8, Vec<u8>), String> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let mut s = tokio::time::timeout(timeout, TcpStream::connect(("127.0.0.1", port)))
        .await
        .map_err(|_| "连接 SOCKS 监听超时".to_string())?
        .map_err(|e| format!("连接 SOCKS 监听失败: {e}"))?;

    // 方法协商：仅 0x00
    s.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
    let mut method = [0u8; 2];
    s.read_exact(&mut method).await.unwrap();
    if method != [0x05, 0x00] {
        return Err(format!("方法协商响应异常: {method:?}"));
    }

    // CONNECT 请求
    let mut req = vec![0x05, 0x01, 0x00];
    let port = match target {
        SocksTarget::Ipv4(ip, p) => {
            req.push(0x01);
            req.extend_from_slice(&ip.octets());
            p
        }
        SocksTarget::Domain(host, p) => {
            req.push(0x03);
            req.push(host.len() as u8);
            req.extend_from_slice(host.as_bytes());
            p
        }
    };
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req).await.unwrap();

    // 响应：VER REP RSV ATYP BND.ADDR BND.PORT（最短 10 字节，IPv4 BND）
    let mut reply = [0u8; 10];
    s.read_exact(&mut reply).await.unwrap();
    let rep = reply[1];

    // 建立成功才继续读目标服务首包（如 sshd banner）
    let mut data = [0u8; 256];
    if rep == 0x00 {
        let n = tokio::time::timeout(timeout, s.read(&mut data))
            .await
            .map_err(|_| "读目标首包超时".to_string())?
            .map_err(|e| format!("读目标首包失败: {e}"))?;
        return Ok((rep, data[..n].to_vec()));
    }
    Ok((rep, Vec::new()))
}

enum SocksTarget<'a> {
    Ipv4(std::net::Ipv4Addr, u16),
    Domain(&'a str, u16),
}

/// -L 基础链路：容器内 sshd（经 direct-tcpip）读回 banner；0x0A 状态帧可解析且含
/// Active 规则；stop 后规则 Stopped、本地端口释放。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tunnel_local_forward_roundtrip_and_status_frame() {
    let (session, mut frame_rx) = match SshSession::connect(test_cfg(), "e2e001").await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("跳过：测试容器不可用（{e}）");
            return;
        }
    };
    let session = std::sync::Arc::new(session);

    let tunnel = session.tunnel();
    tunnel
        .start(
            fwd_rule("e2e-l1", TunnelType::Local, 21001, SSHD_TARGET_PORT),
            STOP_OPTS,
        )
        .await
        .expect("启动 -L 规则");

    let st = wait_state(
        tunnel,
        "e2e-l1",
        TunnelState::Active,
        Duration::from_secs(10),
    )
    .await
    .expect("规则应 Active");
    assert_eq!(st.bound_port, 21001, "固定端口 bind 后 bound_port 应一致");

    // 0x0A 状态帧：JSON {"tunnels":[...]} 含 active 的 e2e-l1
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut saw_frame = false;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(2), frame_rx.recv()).await {
            Ok(Some(frame)) => {
                if let Some((0x0A, payload)) = decode_frame(&frame) {
                    let v: serde_json::Value = match serde_json::from_slice(payload) {
                        Ok(v) => v,
                        Err(e) => panic!("0x0A payload 非 JSON: {e}"),
                    };
                    let hit = v["tunnels"].as_array().is_some_and(|arr| {
                        arr.iter()
                            .any(|r| r["id"] == "e2e-l1" && r["state"] == "active")
                    });
                    if hit {
                        saw_frame = true;
                        break;
                    }
                }
            }
            _ => break,
        }
    }
    assert!(saw_frame, "未收到含 e2e-l1 active 的 0x0A 状态帧");

    // 数据面：本地监听 → SSH direct-tcpip → 容器内 sshd banner
    let data = probe_banner(21001, Duration::from_secs(10))
        .await
        .expect("应能连接本地转发监听");
    assert!(
        data.windows(BANNER.len()).any(|w| w == BANNER),
        "未读到 sshd banner: {:?}",
        String::from_utf8_lossy(&data)
    );

    // 停止：Stopped + 端口释放
    tunnel.stop("e2e-l1").await.expect("停止规则");
    wait_state(
        tunnel,
        "e2e-l1",
        TunnelState::Stopped,
        Duration::from_secs(5),
    )
    .await
    .expect("规则应 Stopped");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        tokio::time::timeout(
            Duration::from_secs(2),
            TcpStream::connect(("127.0.0.1", 21001))
        )
        .await
        .map(|r| r.is_err())
        .unwrap_or(true),
        "停止后本地端口应已释放"
    );

    session.shutdown();
}

/// -L 高并发闸门：100 条并发 > 单规则 64 许可——恰好 64 条成功、其余立即关闭、
/// 监听器不崩溃且继续服务新连接。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tunnel_local_forward_high_concurrency_gate() {
    let (session, _frame_rx) = match SshSession::connect(test_cfg(), "e2e001").await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("跳过：测试容器不可用（{e}）");
            return;
        }
    };
    let session = std::sync::Arc::new(session);

    let tunnel = session.tunnel();
    tunnel
        .start(
            fwd_rule("e2e-c1", TunnelType::Local, 21002, SSHD_TARGET_PORT),
            STOP_OPTS,
        )
        .await
        .expect("启动 -L 规则");
    wait_state(
        tunnel,
        "e2e-c1",
        TunnelState::Active,
        Duration::from_secs(10),
    )
    .await
    .expect("规则应 Active");

    const N: usize = 100;
    const EXPECT_OK: usize = 64;
    let mut set = tokio::task::JoinSet::new();
    for _ in 0..N {
        set.spawn(probe_banner(21002, Duration::from_secs(20)));
    }
    let (mut ok, mut fail) = (0usize, 0usize);
    while let Some(res) = set.join_next().await {
        match res.unwrap() {
            Some(data) if data.windows(BANNER.len()).any(|w| w == BANNER) => ok += 1,
            _ => fail += 1,
        }
    }
    eprintln!("高并发：ok={ok} fail={fail}");
    assert_eq!(ok, EXPECT_OK, "应恰好 {EXPECT_OK} 条成功（闸门 64 许可）");
    assert_eq!(ok + fail, N, "全部连接均应有明确结局（无悬挂）");

    // 监听器存活：闸门打满后新连接照常服务
    let data = probe_banner(21002, Duration::from_secs(10))
        .await
        .expect("高并发后监听器应继续服务");
    assert!(
        data.windows(BANNER.len()).any(|w| w == BANNER),
        "后续连接未读到 sshd banner"
    );

    session.shutdown();
}

/// -D SOCKS5：IPv4 ATYP、域名 ATYP（local 解析 / remote 解析）三条链路，
/// 以及非法 CMD（BIND）回 0x07 命令不支持。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tunnel_dynamic_socks5_forward() {
    let (session, _frame_rx) = match SshSession::connect(test_cfg(), "e2e001").await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("跳过：测试容器不可用（{e}）");
            return;
        }
    };
    let session = std::sync::Arc::new(session);

    let tunnel = session.tunnel();
    // local 解析规则
    tunnel
        .start(
            socks_rule("e2e-d1", 21003),
            StartOptions {
                dns_resolve: DnsResolve::Local,
                ..STOP_OPTS
            },
        )
        .await
        .expect("启动 -D(local) 规则");
    wait_state(
        tunnel,
        "e2e-d1",
        TunnelState::Active,
        Duration::from_secs(10),
    )
    .await
    .expect("规则应 Active");
    // remote 解析规则
    tunnel
        .start(socks_rule("e2e-d2", 21004), STOP_OPTS)
        .await
        .expect("启动 -D(remote) 规则");
    wait_state(
        tunnel,
        "e2e-d2",
        TunnelState::Active,
        Duration::from_secs(10),
    )
    .await
    .expect("规则应 Active");

    // (a) IPv4 ATYP → 容器内 sshd
    let (rep, data) = socks_connect(
        21003,
        SocksTarget::Ipv4(std::net::Ipv4Addr::LOCALHOST, SSHD_TARGET_PORT),
        Duration::from_secs(10),
    )
    .await
    .expect("IPv4 CONNECT 失败");
    assert_eq!(rep, 0x00, "IPv4 CONNECT 应成功");
    assert!(
        data.windows(BANNER.len()).any(|w| w == BANNER),
        "IPv4 链路未读到 sshd banner"
    );

    // (b) 域名 ATYP + local 解析（localhost → 127.0.0.1）
    let (rep, data) = socks_connect(
        21003,
        SocksTarget::Domain("localhost", SSHD_TARGET_PORT),
        Duration::from_secs(10),
    )
    .await
    .expect("域名(local) CONNECT 失败");
    assert_eq!(rep, 0x00, "域名 CONNECT（local 解析）应成功");
    assert!(
        data.windows(BANNER.len()).any(|w| w == BANNER),
        "域名链路（local 解析）未读到 sshd banner"
    );

    // (c) 域名 ATYP + remote 解析（域名透传 sshd）
    let (rep, data) = socks_connect(
        21004,
        SocksTarget::Domain("localhost", SSHD_TARGET_PORT),
        Duration::from_secs(10),
    )
    .await
    .expect("域名(remote) CONNECT 失败");
    assert_eq!(rep, 0x00, "域名 CONNECT（remote 解析）应成功");
    assert!(
        data.windows(BANNER.len()).any(|w| w == BANNER),
        "域名链路（remote 解析）未读到 sshd banner"
    );

    // (d) 非法 CMD（0x02 BIND）→ REP 0x07 命令不支持
    let mut s = TcpStream::connect(("127.0.0.1", 21003)).await.unwrap();
    s.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
    let mut method = [0u8; 2];
    s.read_exact(&mut method).await.unwrap();
    s.write_all(&[0x05, 0x02, 0x00, 0x01, 127, 0, 0, 1, 0x00, 0x50])
        .await
        .unwrap();
    let mut reply = [0u8; 10];
    s.read_exact(&mut reply).await.unwrap();
    assert_eq!(
        reply[1], 0x07,
        "BIND 应回命令不支持(0x07)，实际 {:#x}",
        reply[1]
    );

    session.shutdown();
}

/// -R 远程转发：远端（容器）绑定端口回源到本进程 HTTP 目标；curl 单发 + 64 路并发
/// 全部成功；停止后远端端口不再接受连接。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tunnel_remote_forward_curl_roundtrip_and_concurrency() {
    const BODY: &str = "RHOST-R-OK";
    let (session, _frame_rx) = match SshSession::connect(test_cfg(), "e2e001").await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("跳过：测试容器不可用（{e}）");
            return;
        }
    };
    let session = std::sync::Arc::new(session);

    // 本机目标：一次性 HTTP 应答服务（-R 回调从本进程接入）
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let target_port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                // 读到请求头即可应答（不解析，忽略解析错误）
                let _ = tokio::time::timeout(Duration::from_secs(5), sock.read(&mut buf)).await;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    BODY.len(),
                    BODY
                );
                let _ =
                    tokio::time::timeout(Duration::from_secs(5), sock.write_all(resp.as_bytes()))
                        .await;
            });
        }
    });

    let tunnel = session.tunnel();
    tunnel
        .start(
            fwd_rule("e2e-r1", TunnelType::Remote, 24001, target_port),
            STOP_OPTS,
        )
        .await
        .expect("启动 -R 规则");
    let st = wait_state(
        tunnel,
        "e2e-r1",
        TunnelState::Active,
        Duration::from_secs(10),
    )
    .await
    .expect("规则应 Active");
    assert_eq!(st.bound_port, 24001, "远端回报端口应与请求一致");

    // 单发：容器内 curl 经远端转发回源到本进程
    let out = session
        .exec_collect(
            "curl -s -m 5 http://127.0.0.1:24001/",
            Duration::from_secs(15),
        )
        .await
        .expect("容器内 curl 执行失败");
    assert!(
        out.contains(BODY),
        "远端转发未回源到本地目标，curl 输出: {out}"
    );

    // 64 路并发：恰好打满单规则许可，全部应成功（无 panic / 挂起 / channel 提前 drop）
    let out = session
        .exec_collect(
            "for i in $(seq 1 64); do curl -s -m 10 http://127.0.0.1:24001/ & done; wait",
            Duration::from_secs(40),
        )
        .await
        .expect("并发 curl 执行失败");
    let hits = out.matches(BODY).count();
    eprintln!("-R 并发：{hits}/64 回源成功");
    assert_eq!(hits, 64, "64 路并发应全部回源成功（实际 {hits}）");

    // 停止后远端端口关闭：curl 直接失败（连接拒绝）
    tunnel.stop("e2e-r1").await.expect("停止规则");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let out = session
        .exec_collect(
            "curl -s -m 3 http://127.0.0.1:24001/ >/dev/null 2>&1; echo EXIT:$?",
            Duration::from_secs(10),
        )
        .await
        .expect("容器内 curl 探测执行失败");
    assert!(!out.contains("EXIT:0"), "停止后远端端口仍可连接: {out}");

    session.shutdown();
}

/// next 策略：8 条规则抢占同一 bind_port，全部 Active 且实际端口互不重复。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tunnel_next_policy_multi_rule_distinct_ports() {
    let (session, _frame_rx) = match SshSession::connect(test_cfg(), "e2e001").await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("跳过：测试容器不可用（{e}）");
            return;
        }
    };
    let session = std::sync::Arc::new(session);

    // 探测一个空闲端口作为抢占比对基准
    let probe = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let base = probe.local_addr().unwrap().port();
    drop(probe);

    let tunnel = session.tunnel();
    const NEXT_OPTS: StartOptions = StartOptions {
        port_conflict: PortConflict::Next,
        dns_resolve: DnsResolve::Remote,
        retry_count: 0,
    };
    let ids: Vec<String> = (0..8).map(|i| format!("e2e-n{i}")).collect();
    for id in &ids {
        tunnel
            .start(fwd_rule(id, TunnelType::Local, base, 9), NEXT_OPTS)
            .await
            .unwrap_or_else(|e| panic!("next 规则 {id} 启动失败: {e}"));
    }

    // 等待全部 Active
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let sts = tunnel.statuses().await;
        let hit = sts
            .iter()
            .filter(|s| ids.contains(&s.id))
            .filter(|s| s.state == TunnelState::Active)
            .count();
        if hit == ids.len() || tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let sts: Vec<_> = tunnel
        .statuses()
        .await
        .into_iter()
        .filter(|s| ids.contains(&s.id))
        .collect();
    assert_eq!(sts.len(), ids.len(), "8 条规则均应入表");
    let mut ports: Vec<u16> = Vec::new();
    for s in &sts {
        assert_eq!(
            s.state,
            TunnelState::Active,
            "规则 {} 未 Active: {s:?}",
            s.id
        );
        assert!(
            s.bound_port >= base && s.bound_port < base + 10,
            "顺延端口应落在 base..base+10: {}",
            s.bound_port
        );
        ports.push(s.bound_port);
    }
    ports.sort_unstable();
    let distinct = ports.len();
    ports.dedup();
    assert_eq!(ports.len(), distinct, "存在重复 bound_port: {ports:?}");
    assert_eq!(ports.len(), ids.len(), "8 条规则端口应互不相同: {ports:?}");

    for id in &ids {
        tunnel.stop(id).await.unwrap();
    }
    session.shutdown();
}

/// 生命周期：重复 start 幂等返回 TUNNEL_RUNNING；stop 后端口释放；Error/Stopped
/// 可重新启动；会话 shutdown 后监听 socket 释放。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tunnel_lifecycle_idempotent_and_release() {
    let (session, _frame_rx) = match SshSession::connect(test_cfg(), "e2e001").await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("跳过：测试容器不可用（{e}）");
            return;
        }
    };
    let session = std::sync::Arc::new(session);
    let tunnel = session.tunnel();

    let rule = fwd_rule("e2e-life", TunnelType::Local, 21006, 9); // 目标不可达：连接级失败不影响规则
    tunnel
        .start(rule.clone(), STOP_OPTS)
        .await
        .expect("首次启动");
    wait_state(
        tunnel,
        "e2e-life",
        TunnelState::Active,
        Duration::from_secs(10),
    )
    .await
    .expect("规则应 Active");

    // 幂等：Active/Starting 重复 start → TUNNEL_RUNNING 错误前缀
    let err = tunnel
        .start(rule.clone(), STOP_OPTS)
        .await
        .expect_err("重复启动应报 TUNNEL_RUNNING");
    assert!(
        err.to_string().starts_with("TUNNEL_RUNNING:"),
        "错误前缀应为 TUNNEL_RUNNING:，实际: {err}"
    );

    // 停止 → Stopped → 本地端口释放
    tunnel.stop("e2e-life").await.expect("停止");
    wait_state(
        tunnel,
        "e2e-life",
        TunnelState::Stopped,
        Duration::from_secs(5),
    )
    .await
    .expect("规则应 Stopped");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        tokio::time::timeout(
            Duration::from_secs(2),
            TcpStream::connect(("127.0.0.1", 21006))
        )
        .await
        .map(|r| r.is_err())
        .unwrap_or(true),
        "停止后本地端口应已释放"
    );

    // Stopped 重新 start：真正的新任务
    tunnel.start(rule, STOP_OPTS).await.expect("重新启动");
    wait_state(
        tunnel,
        "e2e-life",
        TunnelState::Active,
        Duration::from_secs(10),
    )
    .await
    .expect("重启后应 Active");

    // 会话 shutdown：监听 socket 应随之释放
    session.shutdown();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        tokio::time::timeout(
            Duration::from_secs(2),
            TcpStream::connect(("127.0.0.1", 21006))
        )
        .await
        .map(|r| r.is_err())
        .unwrap_or(true),
        "会话断开后监听端口应释放"
    );
}
