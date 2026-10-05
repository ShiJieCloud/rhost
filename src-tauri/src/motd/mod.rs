//! Rhost MOTD（Message of the Day）：SSH 认证成功后、PTY 交互 shell 启动前，
//! 客户端经**独立 exec 子通道并行**采集服务器状态，在本地组装为渲染指令数组
//! [`TermCmd`]，交由前端按序绘制产品化欢迎面板。
//!
//! 阶段顺序（由 [`crate::ssh::session::SshSession::connect`] 编排）：
//! 1. 认证成功 → 2. 并行 exec 采集 → 3. 模板占位符替换 → 4. 以 `Motd` 帧
//!    作为二进制通道首帧发出 → 5. 前端渲染 → 6. 才开启 PTY shell 数据流。
//!    由此保证 MOTD 始终出现在远端 PS1 提示符之前。
//!
//! 架构约束：
//! - 采集走独立 SSH exec 通道，**不申请 PTY、不经键盘输入、不解析远端 shell 输出**；
//! - 渲染在前端完成（按 `cls` 配色），本模块只产出结构化指令；
//! - 采集失败 / 超时 / 命令不存在时对应字段降级为 `N/A`，绝不影响主连接；
//! - 模板用 [`include_str!`] 编译期嵌入二进制，运行时不读磁盘。

use std::collections::HashMap;
use std::time::Duration;

use log::debug;
use russh::{ChannelMsg, client};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::ssh::session::ClientHandler;

/// 面板指令模板（编译期嵌入）：`t=print` 输出一行，`cls` 决定整行配色，
/// `{name}` 为状态占位符，`` `text` `` 段以链接色高亮（反引号本身不显示）。
const TEMPLATE_JSON: &str = include_str!("default.motd.json");

/// 采集总超时：慢主机/卡住的 exec 不应拖住会话建立后的任何流程
const COLLECT_TIMEOUT: Duration = Duration::from_secs(3);

/// 单条渲染指令（与前端约定的 `{ t, text, cls }` 结构）
#[derive(Debug, Clone, Serialize)]
pub struct TermCmd {
    pub t: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub cls: String,
}

/// 并行采集：为每条命令打开独立 exec 子通道，命令在服务端并发执行；
/// 读取阶段用 `tokio::spawn` 并发 drain 各通道，避免单条慢命令阻塞其余结果。
///
/// 采集项对齐用户指定清单：
/// - `uname -srm`：系统名（Linux/Darwin/...）、内核版本、机器架构（x86_64/aarch64/arm64）
/// - `uptime`：系统负载、开机时长、登录用户数
/// - `free -h`：内存、swap 占用
/// - `df -h /`：根磁盘使用率
/// - `lastlog -u $USER`：上次登录信息
/// - `ip route get 1.1.1.1`：服务器 IPv4（取 src 字段，回退 hostname -I）
/// - `/proc/[0-9]*`：进程数
/// - `getent passwd $USER`：登录 shell 路径（抑制服务端 MOTD 时使用）
///
/// sshd exec 会以「用户登录 shell -c」执行命令，fish/tcsh 等非 POSIX shell
/// 无法解析 POSIX 语法，因此每条命令体统一包一层 `/bin/sh -c '...'`
/// （POSIX 系统必有 /bin/sh），命令体内不依赖外层 shell 语义。
async fn collect_parallel(handle: &mut client::Handle<ClientHandler>) -> HashMap<String, String> {
    let cmds: [(&str, &str); 12] = [
        // -s/-r/-m 均为 POSIX 标准选项，busybox/macOS/BSD 通用，一行输出
        // "Linux 6.8.0 x86_64" / "Darwin 24.3.0 arm64"
        ("uname", "uname -srm"),
        ("uptime", "uptime"),
        ("mem", "free -h"),
        ("disk", "df -h /"),
        ("lastlog", "lastlog -u \"$USER\" 2>/dev/null"),
        // ip route get 输出 "$7 src 地址 $5 dev 网卡名"（直连路由与默认路由
        // 两种格式下 dev/src 的位置一致）；无 ip 命令时回退 hostname -I（无网卡名）
        (
            "ip",
            "set -- $(ip -4 route get 1.1.1.1 2>/dev/null); if [ -n \"$7\" ]; then echo \"$7 $5\"; else set -- $(hostname -I 2>/dev/null); echo \"$1\"; fi",
        ),
        // 进程数：数 /proc 下数字目录，POSIX glob 实现，busybox/bash 通用、
        // 不依赖 ps 的发行版差异；无 /proc（如 macOS）输出空值，本地降级 N/A。
        // 该字段同时作为「主机存在 /proc」的能力探测信号。
        (
            "proc",
            "if [ -d /proc ]; then set -- /proc/[0-9]*; echo $#; fi",
        ),
        ("shell", "getent passwd \"$USER\" 2>/dev/null | cut -d: -f7"),
        // 以下 4 项供右侧栏静态 HostInfo（0x05 帧）使用，与 MOTD 同批并行采集
        ("osrel", "cat /etc/os-release 2>/dev/null"),
        ("cores", "nproc 2>/dev/null"),
        // CPU 型号：x86 取 model name；ARM64 /proc/cpuinfo 无该字段，
        // 输出打标签的 Hardware/CPU part/CPU implementer 供后端映射
        // （Apple Silicon 虚拟机只暴露 implementer 0x61）。busybox/GNU sed 通用。
        (
            "cpuinfo",
            "sed -n 's/^model name[[:space:]]*:[[:space:]]*/MODEL=/p;s/^Hardware[[:space:]]*:[[:space:]]*/HW=/p;s/^CPU part[[:space:]]*:[[:space:]]*/PART=/p;s/^CPU implementer[[:space:]]*:[[:space:]]*/IMPL=/p' /proc/cpuinfo 2>/dev/null | head -n 8",
        ),
        ("tz", "date +%z 2>/dev/null"),
    ];

    // 包一层 POSIX sh；命令体内的单引号做标准 shell 转义
    let wrap = |body: &str| format!("/bin/sh -c '{}'", body.replace('\'', "'\\''"));

    // 1. 顺序打开通道并下发 exec（命令在服务端立即并发启动）
    let mut channels = Vec::with_capacity(cmds.len());
    for (key, body) in &cmds {
        match handle.channel_open_session().await {
            Ok(ch) => {
                if ch.exec(true, wrap(body)).await.is_ok() {
                    channels.push((key.to_string(), ch));
                }
            }
            Err(e) => debug!("MOTD exec 通道打开失败 [{key}]: {e}"),
        }
    }

    // 2. 并发读取所有通道（各命令已在服务端运行，drain 互不阻塞）
    let mut handles = Vec::with_capacity(channels.len());
    for (key, mut ch) in channels {
        handles.push(tokio::spawn(async move {
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
            (key, String::from_utf8(out).unwrap_or_default())
        }));
    }

    let mut results = HashMap::new();
    for h in handles {
        if let Ok((key, val)) = h.await {
            results.insert(key, val);
        }
    }
    results
}

/// 采集结果：渲染指令 + 从 passwd 探测到的登录 shell（供 exec 启动用）
/// + 各命令原始输出（供 metrics 模块构建结构化 HostInfo，避免重复开通道采集）
pub(crate) struct CollectOutcome {
    pub cmds: Vec<TermCmd>,
    pub login_shell: Option<String>,
    pub raw: HashMap<String, String>,
}

/// 入口：并行采集 → 解析 → 模板替换 → 返回渲染指令数组。
/// `custom_logo` 为用户自定义 ASCII LOGO（多行文本）；非空时替换模板内置
/// LOGO 段（`logo: true` 标记的行），空串保留内置，任何情况下不影响状态区。
/// 任何阶段失败（超时/会话取消/采集无输出）均返回 None（静默降级）。
pub(crate) async fn collect_and_build(
    handle: &mut client::Handle<ClientHandler>,
    cancel: &CancellationToken,
    custom_logo: &str,
) -> Option<CollectOutcome> {
    let gather = async { collect_parallel(handle).await };

    let raw = tokio::select! {
        _ = cancel.cancelled() => {
            debug!("MOTD 采集随会话取消，跳过欢迎面板");
            return None;
        }
        res = tokio::time::timeout(COLLECT_TIMEOUT, gather) => match res {
            Ok(raw) => raw,
            Err(_) => {
                debug!("MOTD 采集超时（{}s），跳过欢迎面板", COLLECT_TIMEOUT.as_secs());
                return None;
            }
        },
    };

    if raw.is_empty() {
        debug!("MOTD 采集无输出，跳过欢迎面板");
        return None;
    }

    let login_shell = raw
        .get("shell")
        .and_then(|s| s.lines().next())
        .map(str::trim)
        .filter(|s| s.starts_with('/'))
        .map(str::to_string);

    let values = build_values(&raw);
    Some(CollectOutcome {
        cmds: build_cmds(&values, custom_logo),
        login_shell,
        raw,
    })
}

const NA: &str = "N/A";

/// 把各命令原始输出解析为模板占位符的最终文本
fn build_values(raw: &HashMap<String, String>) -> HashMap<String, String> {
    let mut v = HashMap::new();
    v.insert("version".to_string(), env!("CARGO_PKG_VERSION").to_string());

    // uname -srm：首行空白切三段 = 系统名 / 内核版本 / 架构，缺失段各自降级 N/A
    let uname_fields: Vec<&str> = raw
        .get("uname")
        .map(|s| s.lines().next().unwrap_or("").split_whitespace().collect())
        .unwrap_or_default();
    let pick = |i: usize| -> String {
        uname_fields
            .get(i)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| NA.into())
    };
    v.insert("os".to_string(), pick(0));
    v.insert("kernel".to_string(), pick(1));
    v.insert("arch".to_string(), pick(2));

    // uptime 输出："14:23:01 up 1 day, 3:12, 2 users,  load average: 0.00, 0.01, 0.05"
    let uptime = raw.get("uptime").cloned().unwrap_or_default();
    v.insert(
        "load".to_string(),
        uptime_load(&uptime).unwrap_or_else(|| NA.into()),
    );
    v.insert(
        "users".to_string(),
        uptime_users(&uptime).unwrap_or_else(|| NA.into()),
    );
    v.insert(
        "uptime".to_string(),
        uptime_duration(&uptime).unwrap_or_else(|| NA.into()),
    );

    // free -h：解析 Mem: / Swap: 行（内存带总量，swap 只显示百分比）
    let mem = raw.get("mem").cloned().unwrap_or_default();
    v.insert(
        "mem".to_string(),
        parse_free_used_pct(&mem, "Mem:", true).unwrap_or_else(|| NA.into()),
    );
    v.insert(
        "swap".to_string(),
        parse_free_used_pct(&mem, "Swap:", false).unwrap_or_else(|| NA.into()),
    );

    // df -h /：第二行的 Size 与 Use%
    let disk = raw.get("disk").cloned().unwrap_or_default();
    v.insert(
        "disk".to_string(),
        parse_df(&disk).unwrap_or_else(|| NA.into()),
    );

    // lastlog：第二行的 Latest 时间 + From 来源
    let lastlog = raw.get("lastlog").cloned().unwrap_or_default();
    v.insert("last_login".to_string(), parse_lastlog(&lastlog));

    // ip：取首个非空 token（/bin/sh 包装层已保证是纯地址输出）
    v.insert(
        "ip".to_string(),
        raw.get("ip")
            .and_then(|s| s.split_whitespace().next())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| NA.into()),
    );

    // proc：/proc 数字目录计数
    v.insert(
        "proc".to_string(),
        raw.get("proc")
            .and_then(|s| s.split_whitespace().next())
            .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
            .map(str::to_string)
            .unwrap_or_else(|| NA.into()),
    );

    v
}

/// uptime 负载：取 "load average[s]:" 后第一个数字（1 分钟平均负载）。
fn uptime_load(s: &str) -> Option<String> {
    let idx = s.find("load average")?;
    let after = &s[idx + "load average".len()..];
    // 兼容 "load average:" 与 "load averages:"
    let after = after
        .trim_start_matches('s')
        .trim_start_matches(':')
        .trim_start();
    let token = after
        .split(|c: char| c == ',' || c.is_whitespace())
        .find(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit() || b == b'.'))?;
    Some(token.to_string())
}

/// uptime 登录用户数：逗号分段里找以 "user/users" 结尾的段，取其中纯数字 token。
fn uptime_users(s: &str) -> Option<String> {
    s.split(',')
        .map(str::trim)
        .find(|seg| seg.ends_with(" users") || seg.ends_with(" user"))
        .and_then(|seg| {
            seg.split_whitespace()
                .find(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(str::to_string)
}

/// uptime 开机时长：" up " 之后到第一个逗号（如 "1 day"、"42 min"、"2:15"）。
fn uptime_duration(s: &str) -> Option<String> {
    let after = s.split(" up ").nth(1)?;
    let dur = after.split(',').next().unwrap_or(after).trim();
    if dur.is_empty() {
        None
    } else {
        Some(dur.to_string())
    }
}

/// 解析 `free -h` 中以 `prefix`（"Mem:"/"Swap:"）开头的行。
/// `with_total=true` 返回 "X% of Y"（内存），否则仅 "X%"（swap）。
fn parse_free_used_pct(free_out: &str, prefix: &str, with_total: bool) -> Option<String> {
    let line = free_out
        .lines()
        .find(|l| l.trim_start().starts_with(prefix))?;
    let fields: Vec<&str> = line.split_whitespace().collect();
    // fields: [Mem:, total, used, free, shared, buff/cache, available]
    if fields.len() < 3 {
        return None;
    }
    let total = parse_human_size(fields[1])?;
    let used = parse_human_size(fields[2]).unwrap_or(0.0);
    if total <= 0.0 {
        // 无内存/无交换分区：内存仍给 0%（行存在即数据有效）
        return Some("0%".to_string());
    }
    let pct = (used / total * 100.0).round() as u64;
    if with_total {
        Some(format!("{pct}% of {}", human_size(total)))
    } else {
        Some(format!("{pct}%"))
    }
}

/// 解析 `df -h /` 输出，返回 "X% of Y"。
fn parse_df(df_out: &str) -> Option<String> {
    // 取第二行（数据行）
    let line = df_out.lines().nth(1)?;
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 5 {
        return None;
    }
    let total = parse_human_size(fields[1])?;
    let pct_str = fields[4].trim_end_matches('%');
    let pct = pct_str.parse::<u64>().ok()?;
    Some(format!("{pct}% of {}", human_size(total)))
}

/// 解析 `lastlog -u USER` 输出的最新登录时间与来源 IP；无数据返回 N/A。
fn parse_lastlog(out: &str) -> String {
    let Some(line) = out.lines().nth(1) else {
        return NA.into();
    };
    if line.contains("Never logged in") {
        return "No previous login".into();
    }
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 4 {
        return NA.into();
    }
    // From 列可能为空（本地 tty 登录），此时第 2 字段已是日期起始（星期缩写）
    let is_weekday = |s: &str| matches!(s, "Mon" | "Tue" | "Wed" | "Thu" | "Fri" | "Sat" | "Sun");
    let (date_start, from) = if is_weekday(fields[2]) {
        (2, None)
    } else {
        (3, Some(fields[2]))
    };
    // 日期字段起为时间（"Wed Oct  1 14:23:01 +0000 2025"）
    let time = fields[date_start..].join(" ");
    match from {
        Some(ip) => format!("{time} from {ip}"),
        None => time,
    }
}

/// 解析人类可读大小，返回字节数。支持 "7.8Gi" "1.2G" "100Mi" "0B" "512"。
fn parse_human_size(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() || s == "-" || s == "0" {
        return Some(0.0);
    }
    let num_end = s
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(s.len());
    let num: f64 = s[..num_end].parse().ok()?;
    let unit = s[num_end..].trim();
    let mult = match unit {
        "" | "B" => 1.0,
        "K" | "Ki" => 1024.0,
        "M" | "Mi" => 1024.0 * 1024.0,
        "G" | "Gi" => 1024.0 * 1024.0 * 1024.0,
        "T" | "Ti" => 1024.0_f64.powi(4),
        _ => return None,
    };
    Some(num * mult)
}

/// 字节数 → 人类可读（1024 进制，与 "7.8Gi" 风格一致）
fn human_size(bytes: f64) -> String {
    const UNITS: [&str; 5] = ["B", "Ki", "Mi", "Gi", "Ti"];
    let mut val = bytes;
    let mut unit = 0;
    while val >= 1024.0 && unit < UNITS.len() - 1 {
        val /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{:.0}{}", val, UNITS[unit])
    } else {
        format!("{val:.1}{}", UNITS[unit])
    }
}

/// `{name}` 占位符替换；未收录或名字不合法的占位符原样保留。
/// 另支持 `{name:width}`：把值**左对齐补空格到 width 列**，用于多列布局时
/// 让右列起点不随值长短漂移。状态值均为 ASCII，按字符数补齐即等价于显示列宽；
/// 值超过 width 时不截断（宁可该行撑开也不丢内容）。
fn substitute(text: &str, values: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after_brace = &rest[open + 1..];
        if let Some(rel) = after_brace.find('}') {
            let token = &after_brace[..rel];
            // 拆出可选宽度：{name} 或 {name:width}
            let (name, width) = match token.split_once(':') {
                Some((n, w)) if !w.is_empty() && w.bytes().all(|b| b.is_ascii_digit()) => {
                    (n, w.parse::<usize>().ok())
                }
                _ => (token, None),
            };
            if !name.is_empty()
                && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                && let Some(val) = values.get(name)
            {
                match width {
                    Some(w) => out.push_str(&format!("{val:<w$}")),
                    None => out.push_str(val),
                }
                rest = &after_brace[rel + 1..];
                continue;
            }
            out.push('{');
            rest = after_brace;
        } else {
            out.push_str(rest);
            rest = "";
            break;
        }
    }
    out.push_str(rest);
    out
}

/// 自定义 LOGO 防御性上限：最多 30 行、单行 200 字符（防粘贴超大文本拖垮渲染）
const LOGO_MAX_LINES: usize = 30;
const LOGO_MAX_COLS: usize = 200;

/// 把用户自定义 ASCII LOGO 文本规整为渲染指令行：去掉首尾空行，
/// 最多取 [`LOGO_MAX_LINES`] 行、单行最多 [`LOGO_MAX_COLS`] 字符；
/// cls 统一 "brand" 复用内置 LOGO 配色。空文本返回空数组。
fn custom_logo_cmds(custom_logo: &str) -> Vec<TermCmd> {
    custom_logo
        .trim_matches(['\n', '\r'])
        .lines()
        .take(LOGO_MAX_LINES)
        .map(|line| TermCmd {
            t: "print".to_string(),
            text: line.chars().take(LOGO_MAX_COLS).collect(),
            cls: "brand".to_string(),
        })
        .collect()
}

/// 读取编译期模板，替换占位符，输出渲染指令数组。
/// `custom_logo` 非空时替换模板中 `logo: true` 标记的内置 LOGO 段：
/// 自定义行插在段首位置（其余段落顺序不变）；空串保留内置 LOGO。
fn build_cmds(values: &HashMap<String, String>, custom_logo: &str) -> Vec<TermCmd> {
    let custom = custom_logo_cmds(custom_logo);
    match serde_json::from_str::<Vec<RawLine>>(TEMPLATE_JSON) {
        Ok(lines) => {
            let mut cmds = Vec::with_capacity(lines.len() + custom.len());
            let mut inserted = false;
            for l in lines {
                // 仅在传入自定义 LOGO 时跳过内置 LOGO 行：首个位置插入自定义段
                if l.logo && !custom.is_empty() {
                    if !inserted {
                        cmds.extend(custom.iter().cloned());
                        inserted = true;
                    }
                    continue;
                }
                cmds.push(TermCmd {
                    t: l.t,
                    text: substitute(&l.text, values),
                    cls: l.cls,
                });
            }
            if !inserted {
                cmds.extend(custom); // 防御：模板无 LOGO 段时追加（当前模板不会走到）
            }
            cmds
        }
        Err(e) => {
            debug!("MOTD 模板解析失败（不应发生，模板随编译期嵌入）: {e}");
            Vec::new()
        }
    }
}

/// 模板原始行结构（仅内部反序列化用）
#[derive(Debug, serde::Deserialize)]
struct RawLine {
    t: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    cls: String,
    /// 内置 LOGO 段标记：传入自定义 LOGO 时整段被替换
    #[serde(default)]
    logo: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses_and_substitutes() {
        let mut v = HashMap::new();
        v.insert("version".to_string(), "0.1.0".into());
        v.insert("os".to_string(), "Linux".into());
        v.insert("kernel".to_string(), "6.8.0".into());
        v.insert("arch".to_string(), "x86_64".into());
        v.insert("load".to_string(), "0.05".into());
        v.insert("users".to_string(), "2".into());
        v.insert("mem".to_string(), "23% of 7.8Gi".into());
        v.insert("swap".to_string(), "0%".into());
        v.insert("disk".to_string(), "1% of 290.0Gi".into());
        v.insert("uptime".to_string(), "1 day".into());
        v.insert("ip".to_string(), "192.168.1.1".into());
        v.insert(
            "last_login".to_string(),
            "Wed Oct 1 14:23:01 +0000 2025".into(),
        );

        let cmds = build_cmds(&v, "");
        assert!(cmds.len() > 10);
        let text_all = cmds
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text_all.contains("Rhost 0.1.0"));
        assert!(text_all.contains("(Linux 6.8.0 x86_64)"));
        assert!(text_all.contains("23% of 7.8Gi"));
    }

    #[test]
    fn custom_logo_replaces_builtin_and_keeps_layout() {
        let v: HashMap<String, String> = build_values(&HashMap::new());

        // 空自定义：内置 LOGO 原样保留
        let builtin = build_cmds(&v, "");
        assert!(builtin.iter().any(|c| c.text.contains("____")));

        // 非空自定义：内置 5 行 LOGO 整段消失，自定义行插在 LOGO 段位置
        let cmds = build_cmds(&v, "MY LOGO\nLINE2");
        assert!(!cmds.iter().any(|c| c.text.contains("____")));
        let idx = cmds
            .iter()
            .position(|c| c.text == "MY LOGO")
            .expect("自定义 LOGO 首行应存在");
        assert_eq!(cmds[idx].cls, "brand");
        assert_eq!(cmds[idx + 1].text, "LINE2");
        // LOGO 段后紧跟空行 + Welcome 行，其余布局不受影响
        assert_eq!(cmds[idx + 2].text, "");
        assert!(cmds[idx + 3].text.contains("Welcome to Rhost"));
    }

    #[test]
    fn custom_logo_caps_lines_and_width() {
        let v = HashMap::new();
        // 超行数截断到 30 行，且后续布局完整（空行 + Welcome）
        let long = (0..40).map(|i| format!("L{i}")).collect::<Vec<_>>().join("\n");
        let cmds = build_cmds(&v, &long);
        let pos = cmds
            .iter()
            .position(|c| c.text == "L0")
            .expect("自定义首行应存在");
        assert_eq!(cmds[pos + 29].text, "L29");
        assert_eq!(cmds[pos + 30].text, "");
        assert!(cmds[pos + 31].text.contains("Welcome to Rhost"));

        // 超宽单行截断到 200 字符
        let wide = "X".repeat(300);
        let cmds = build_cmds(&v, &wide);
        assert_eq!(cmds[0].text.chars().count(), 200);
    }

    #[test]
    fn template_two_column_values_aligned() {
        let lines: Vec<RawLine> = serde_json::from_str(TEMPLATE_JSON).expect("模板可解析");
        // 四行双列布局：以右列字段名（users/ip/proc/uptime）精确识别，
        // 不能用占位符数量（Welcome 行有 version/os/kernel/arch 四个）
        let right_fields = ["{users}", "{ip}", "{proc}", "{uptime}"];
        let two_col = lines
            .iter()
            .filter(|l| right_fields.iter().any(|f| l.text.contains(f)))
            .collect::<Vec<_>>();
        assert_eq!(two_col.len(), 4, "应有且仅有四行双列状态行");

        // 左列值给短串（由 {:20} 补齐到 20），右列值统一用标记，
        // 比较标记起点列：无论值长短都必须完全一致。
        let mut v = HashMap::new();
        for k in ["load", "mem", "swap", "disk"] {
            v.insert(k.to_string(), "a".to_string());
        }
        for k in ["users", "ip", "proc", "uptime"] {
            v.insert(k.to_string(), "@@V@@".to_string());
        }

        // 左静态区16 + 左值定宽20 + 右label区18 = 右值起点第54列(0-based)
        let mut col: Option<usize> = None;
        for l in &two_col {
            let pos = substitute(&l.text, &v)
                .find("@@V@@")
                .unwrap_or_else(|| panic!("右值缺失: {}", l.text));
            assert_eq!(pos, 54, "右列 value 未对齐: {}", l.text);
            col = Some(pos);
        }
        assert_eq!(col, Some(54));
    }

    #[test]
    fn human_size_roundtrip() {
        assert_eq!(
            parse_human_size("7.8Gi").unwrap(),
            7.8 * 1024.0 * 1024.0 * 1024.0
        );
        assert_eq!(parse_human_size("0B").unwrap(), 0.0);
        assert_eq!(human_size(0.0), "0B");
        assert_eq!(human_size(7.8 * 1024.0 * 1024.0 * 1024.0), "7.8Gi");
    }

    #[test]
    fn parse_uptime_fields() {
        let u = "14:23:01 up 1 day, 3:12, 2 users,  load average: 0.00, 0.01, 0.05";
        assert_eq!(uptime_load(u).unwrap(), "0.00");
        assert_eq!(uptime_users(u).unwrap(), "2");
        assert_eq!(uptime_duration(u).unwrap(), "1 day");

        // busybox 风格：无 users 段、只有一个用户
        let b = " 10:02:43 up 42 min, 1 user, load average: 0.10, 0.08, 0.02";
        assert_eq!(uptime_load(b).unwrap(), "0.10");
        assert_eq!(uptime_users(b).unwrap(), "1");
        assert_eq!(uptime_duration(b).unwrap(), "42 min");

        assert!(uptime_load("").is_none());
        assert!(uptime_users("").is_none());
        assert!(uptime_duration("").is_none());
    }

    #[test]
    fn parse_free_and_df() {
        let free = "\
              total        used        free      shared  buff/cache   available
Mem:          7.8Gi       1.2Gi       5.6Gi       1.0Mi       980Mi       6.3Gi
Swap:         2.0Gi          0B       2.0Gi";
        assert!(
            parse_free_used_pct(free, "Mem:", true)
                .unwrap()
                .starts_with("15% of 7.8Gi")
        );
        assert_eq!(parse_free_used_pct(free, "Swap:", false).unwrap(), "0%");

        let df = "Filesystem      Size  Used Avail Use% Mounted on\noverlay         290G  2.5G  288G   1% /";
        assert_eq!(parse_df(df).unwrap(), "1% of 290.0Gi");
    }

    #[test]
    fn parse_lastlog_never_and_logged() {
        let never = "Username         Port     From             Latest\ntest                                       **Never logged in**";
        assert_eq!(parse_lastlog(never), "No previous login");

        let logged = "Username         Port     From             Latest\ntest             pts/0    192.168.1.100    Wed Oct  1 14:23:01 +0000 2025";
        assert_eq!(
            parse_lastlog(logged),
            "Wed Oct 1 14:23:01 +0000 2025 from 192.168.1.100"
        );

        // From 列为空（本地 tty 登录）：不拼 from
        let local = "Username         Port     From             Latest\ntest             pts/0                     Wed Oct  1 14:23:01 +0000 2025";
        assert_eq!(parse_lastlog(local), "Wed Oct 1 14:23:01 +0000 2025");
    }

    #[test]
    fn substitution_known_and_unknown() {
        let mut v = HashMap::new();
        v.insert("version".to_string(), "0.1.0".to_string());
        assert_eq!(substitute("v{version}", &v), "v0.1.0");
        assert_eq!(substitute("v{future}", &v), "v{future}");

        // {name:width} 左对齐补空格到固定宽度
        v.insert("load".to_string(), "0.10".to_string());
        assert_eq!(substitute("[{load:8}]", &v), "[0.10    ]");
        // 值恰好等宽不补
        v.insert("mem".to_string(), "12345678".to_string());
        assert_eq!(substitute("[{mem:8}]", &v), "[12345678]");
        // 超长不截断（宁可撑开也不丢内容）
        v.insert("disk".to_string(), "100% of 999.9G".to_string());
        assert_eq!(substitute("[{disk:8}]", &v), "[100% of 999.9G]");

        // 核心诉求：无论值长短，右列 label 起点恒定（左静态区16 + 值宽20 = 第36列）
        let row = |val: &str| {
            let mut m = HashMap::new();
            m.insert("load".to_string(), val.to_string());
            substitute("  System load:  {load:20}Users logged in:", &m)
        };
        let short = row("0.0");
        let long = row("12.34, 5.67, 8.90");
        assert_eq!(short.find("Users"), long.find("Users"));
        assert_eq!(short.find("Users"), Some(36));

        // 非法宽度/未收录占位符原样保留
        assert_eq!(substitute("[{load:nope}]", &v), "[{load:nope}]");
        assert_eq!(substitute("[{missing:5}]", &v), "[{missing:5}]");
    }

    #[test]
    fn build_values_parses_uname() {
        // Linux x86_64
        let mut raw = HashMap::new();
        raw.insert("uname".to_string(), "Linux 6.8.0 x86_64\n".into());
        let v = build_values(&raw);
        assert_eq!(v.get("os").unwrap(), "Linux");
        assert_eq!(v.get("kernel").unwrap(), "6.8.0");
        assert_eq!(v.get("arch").unwrap(), "x86_64");

        // macOS（Darwin arm64）
        let mut raw = HashMap::new();
        raw.insert("uname".to_string(), "Darwin 24.3.0 arm64".into());
        let v = build_values(&raw);
        assert_eq!(v.get("os").unwrap(), "Darwin");
        assert_eq!(v.get("kernel").unwrap(), "24.3.0");
        assert_eq!(v.get("arch").unwrap(), "arm64");
    }

    #[test]
    fn build_values_handles_empty() {
        let v = build_values(&HashMap::new());
        assert_eq!(v.get("os").unwrap(), NA);
        assert_eq!(v.get("kernel").unwrap(), NA);
        assert_eq!(v.get("arch").unwrap(), NA);
        assert_eq!(v.get("load").unwrap(), NA);
        assert!(v.contains_key("last_login"));
    }
}
