//! tunnel 模块单测：规则校验 / 令牌桶 / RemoteRegistry / 快照序列化。

use super::*;

/* ---- 规则校验 ---- */

fn rule(id: &str, kind: TunnelType, bind_port: u16, target: Option<(&str, u16)>) -> TunnelRule {
    TunnelRule {
        id: id.into(),
        kind,
        bind_host: "127.0.0.1".into(),
        bind_port,
        target_host: target.map(|(h, _)| h.to_string()),
        target_port: target.map(|(_, p)| p),
    }
}

#[test]
fn rule_port_out_of_range_rejected() {
    // local：端口 0 非法（仅 remote 保留 0 = 远端分配）
    assert!(validate_rule(&rule("a", TunnelType::Local, 0, Some(("localhost", 80)))).is_err());
    // u16 上限即 65535 合法；>65535 由 u16 类型天然拒绝，此处验证下界
    assert!(validate_rule(&rule("b", TunnelType::Local, 1, Some(("localhost", 80)))).is_ok());
    assert!(validate_rule(&rule("c", TunnelType::Local, 65535, Some(("localhost", 80)))).is_ok());
    // target 端口越界
    assert!(validate_rule(&rule("d", TunnelType::Local, 8080, Some(("localhost", 0)))).is_err());
}

#[test]
fn rule_empty_bind_host_normalized() {
    let mut r = rule("a", TunnelType::Local, 8080, Some(("localhost", 80)));
    r.bind_host = "  ".into();
    let v = validate_rule(&r).unwrap();
    assert_eq!(v.bind_host, "127.0.0.1");
}

#[test]
fn rule_illegal_host_chars_rejected() {
    let mut r = rule("a", TunnelType::Local, 8080, Some(("localhost", 80)));
    r.bind_host = "0.0.0.0; rm -rf /".into();
    assert!(validate_rule(&r).is_err());
    r.bind_host = "host with space".into();
    assert!(validate_rule(&r).is_err());
    // IPv6 / 域名合法
    r.bind_host = "[::1]".into();
    assert!(validate_rule(&r).is_ok());
    r.bind_host = "db.internal.example".into();
    assert!(validate_rule(&r).is_ok());
}

#[test]
fn rule_dynamic_must_not_carry_target() {
    assert!(validate_rule(&rule("a", TunnelType::Dynamic, 1080, None)).is_ok());
    assert!(
        validate_rule(&rule("b", TunnelType::Dynamic, 1080, Some(("localhost", 80)))).is_err()
    );
}

#[test]
fn rule_local_remote_require_target() {
    // -L 必须有目标
    assert!(validate_rule(&rule("a", TunnelType::Local, 8080, None)).is_err());
    assert!(validate_rule(&rule("b", TunnelType::Remote, 9090, None)).is_err());
    // target_host 留空串也不行
    let mut r = rule("c", TunnelType::Local, 8080, Some(("", 80)));
    r.target_host = Some("".into());
    assert!(validate_rule(&r).is_err());
    // target_port 缺省（None）非法
    let mut r = rule("d", TunnelType::Local, 8080, Some(("localhost", 80)));
    r.target_port = None;
    assert!(validate_rule(&r).is_err());
}

#[test]
fn rule_empty_id_rejected() {
    assert!(validate_rule(&rule("", TunnelType::Local, 8080, Some(("localhost", 80)))).is_err());
}

/* ---- 令牌桶 ---- */

#[test]
fn rate_limiter_burst_allows_200_then_rejects() {
    let rl = RateLimiter::new(ACCEPT_RATE_BURST, ACCEPT_RATE_REFILL);
    // 突发 200 全通过
    for _ in 0..ACCEPT_RATE_BURST {
        assert!(rl.allow(), "突发窗口内的请求不应被拒绝");
    }
    // 第 201 条拒绝
    assert!(!rl.allow(), "超出突发容量应被拒绝");
}

#[tokio::test]
async fn rate_limiter_refills_after_idle() {
    let rl = RateLimiter::new(ACCEPT_RATE_BURST, ACCEPT_RATE_REFILL);
    for _ in 0..ACCEPT_RATE_BURST {
        assert!(rl.allow());
    }
    assert!(!rl.allow());
    // 静置 1s：回灌 ~100 枚
    tokio::time::sleep(Duration::from_millis(1050)).await;
    let mut granted = 0;
    while rl.allow() {
        granted += 1;
    }
    assert!(
        (90..=110).contains(&granted),
        "1s 回灌后应恢复约 100 枚许可，实际 {granted}"
    );
}

/* ---- RemoteRegistry ---- */

#[test]
fn remote_registry_insert_lookup_remove() {
    let reg = RemoteRegistry::default();
    assert!(reg.lookup("127.0.0.1", 9090).is_none());

    let entry = Arc::new(TunnelEntry::new(
        rule("r1", TunnelType::Remote, 9090, Some(("localhost", 3000))),
        &CancellationToken::new(),
    ));
    reg.insert(
        "127.0.0.1",
        9090,
        RemoteBinding {
            target: RemoteTarget {
                host: "localhost".into(),
                port: 3000,
            },
            entry: entry.clone(),
        },
    );

    // key 命中：同一 (host, port) 查回
    let hit = reg.lookup("127.0.0.1", 9090).expect("命中");
    assert_eq!(hit.target.host, "localhost");
    assert_eq!(hit.target.port, 3000);
    assert_eq!(hit.entry.rule.id, "r1");
    // 不同端口不命中
    assert!(reg.lookup("127.0.0.1", 9091).is_none());

    // 删除后不再命中
    assert!(reg.remove("127.0.0.1", 9090).is_some());
    assert!(reg.lookup("127.0.0.1", 9090).is_none());
    assert!(reg.remove("127.0.0.1", 9090).is_none());
}

/* ---- 快照序列化 ---- */

#[test]
fn status_serializes_camel_case() {
    let entry = TunnelEntry::new(
        rule("t1", TunnelType::Dynamic, 1080, None),
        &CancellationToken::new(),
    );
    entry.set_state(TunnelState::Active, None);
    entry.bound_port.store(1080, Ordering::Relaxed);
    entry.counters.active.store(2, Ordering::Relaxed);
    entry.counters.bytes_up.store(12, Ordering::Relaxed);
    entry.counters.bytes_down.store(1153433, Ordering::Relaxed);

    let s = entry.status();
    let json = serde_json::to_value(&s).unwrap();
    // 字段名全部 camelCase
    for key in [
        "id",
        "type",
        "state",
        "bindHost",
        "bindPort",
        "boundPort",
        "activeConnections",
        "bytesUp",
        "bytesDown",
        "error",
    ] {
        assert!(json.get(key).is_some(), "缺少字段 {key}");
    }
    // 枚举值序列化为小写
    assert_eq!(json["type"], "dynamic");
    assert_eq!(json["state"], "active");
    // dynamic 无 target 字段（skip_serializing_if）
    assert!(json.get("targetHost").is_none());
    assert!(json.get("targetPort").is_none());
    // dynamic 的 target 序列化不受影响
    let entry2 = TunnelEntry::new(
        rule("t2", TunnelType::Local, 8080, Some(("localhost", 80))),
        &CancellationToken::new(),
    );
    let json2 = serde_json::to_value(entry2.status()).unwrap();
    assert_eq!(json2["targetHost"], "localhost");
    assert_eq!(json2["targetPort"], 80);
}

/* ---- 状态机基本行为 ---- */

#[test]
fn entry_state_transitions() {
    let entry = TunnelEntry::new(
        rule("s1", TunnelType::Local, 8080, Some(("localhost", 80))),
        &CancellationToken::new(),
    );
    assert_eq!(entry.current_state(), TunnelState::Stopped);
    entry.set_state(TunnelState::Starting, None);
    assert_eq!(entry.current_state(), TunnelState::Starting);
    entry.set_state(TunnelState::Error, Some("端口被占用".into()));
    assert_eq!(entry.current_state(), TunnelState::Error);
    assert_eq!(entry.status().error.as_deref(), Some("端口被占用"));
    entry.set_state(TunnelState::Active, None);
    assert_eq!(entry.status().error, None);
}

/* ---- 策略解析 ---- */

#[test]
fn port_conflict_and_dns_parse() {
    assert_eq!(PortConflict::parse("stop"), PortConflict::Stop);
    assert_eq!(PortConflict::parse("skip"), PortConflict::Skip);
    assert_eq!(PortConflict::parse("next"), PortConflict::Next);
    assert_eq!(PortConflict::parse("bogus"), PortConflict::Stop);

    assert_eq!(DnsResolve::parse("remote"), DnsResolve::Remote);
    assert_eq!(DnsResolve::parse("local"), DnsResolve::Local);
    assert_eq!(DnsResolve::parse("bogus"), DnsResolve::Remote);
}

/* ---- validate_host 独立行为 ---- */

#[test]
fn validate_host_edge_cases() {
    assert_eq!(validate_host("监听地址", "").unwrap(), "127.0.0.1");
    assert_eq!(validate_host("监听地址", "0.0.0.0").unwrap(), "0.0.0.0");
    // 超长拒绝
    let long = "a".repeat(256);
    assert!(validate_host("监听地址", &long).is_err());
}
