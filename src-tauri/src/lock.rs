//! 应用锁 / 会话锁凭据存储与验证（docs/explanation/design/app-lock-design.md）。
//!
//! 方案：
//! - 锁密码不可恢复，`PBKDF2-HMAC-SHA256`（210000 轮，复用
//!   [`crate::config_crypto::KDF_ITERS`]）派生 32B 哈希，与 16B 随机 salt
//!   一起存入独立文件 `<app_config_dir>/lock.json`；
//! - 文件分 `app` / `session` 两个槽位，`null` 表示该类锁未设置密码；
//! - 凭据**不进** `app_config.json`，因此天然不参与配置导入导出；
//! - 连续验证失败限速：每 5 次失败冷却 30s 起指数翻倍、300s 封顶；
//! - 密码只以 `Zeroizing<String>` 存在于命令调用栈，不落日志。

use std::collections::HashMap;
use std::fs;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use aes_gcm::aead::rand_core::RngCore;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use chrono::Utc;
use pbkdf2::pbkdf2_hmac_array;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tauri::{AppHandle, Manager, State};
use zeroize::Zeroizing;

use crate::config_crypto::KDF_ITERS;

const LOCK_FILE_NAME: &str = "lock.json";
const LOCK_FILE_VERSION: u32 = 1;
const SALT_LEN: usize = 16;
const HASH_LEN: usize = 32;
/// 兼容读取的最大迭代轮数（防篡改文件造成 DoS）
const MAX_KDF_ITERS: u32 = 1_000_000;

/// 密码字符数范围（按 Unicode scalar 计数，前后端双校验）
const PWD_MIN_CHARS: usize = 6;
const PWD_MAX_CHARS: usize = 128;

/// 连续失败多少次进入冷却
const FAIL_BATCH: u32 = 5;
/// 首批冷却秒数，之后每批翻倍
const BACKOFF_BASE_SECS: u64 = 30;
/// 冷却上限秒数
const BACKOFF_CAP_SECS: u64 = 300;

/* =========================================================
 *  文件结构
 * ========================================================= */

#[derive(Clone, Serialize, Deserialize)]
struct HashEntry {
    /// base64(16B salt)
    salt: String,
    /// base64(32B PBKDF2 输出)
    hash: String,
    /// PBKDF2 迭代轮数
    iters: u32,
}

#[derive(Clone, Serialize, Deserialize)]
struct LockFile {
    version: u32,
    #[serde(default)]
    app: Option<HashEntry>,
    #[serde(default)]
    session: Option<HashEntry>,
}

impl Default for LockFile {
    fn default() -> Self {
        Self {
            version: LOCK_FILE_VERSION,
            app: None,
            session: None,
        }
    }
}

impl HashEntry {
    /// 校验字段长度与轮数范围；非法值意味着文件损坏或被篡改。
    fn valid(&self) -> bool {
        let salt_ok = B64
            .decode(self.salt.as_bytes())
            .map(|b| b.len() == SALT_LEN)
            .unwrap_or(false);
        let hash_ok = B64
            .decode(self.hash.as_bytes())
            .map(|b| b.len() == HASH_LEN)
            .unwrap_or(false);
        salt_ok && hash_ok && (1..=MAX_KDF_ITERS).contains(&self.iters)
    }
}

fn lock_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("获取应用配置目录失败: {e}"))?;
    Ok(dir.join(LOCK_FILE_NAME))
}

/// 读取 lock.json。
///
/// - 文件不存在：返回默认（两槽位均空）；
/// - 文件损坏（解析失败 / 字段非法）：改名为 `lock.json.corrupt-<unix_ms>`
///   隔离，返回默认，由调用方按「未设置」继续（设计 §6.1 降级）。
///
/// 返回值第二项 `corrupted` 表示本次读取是否发生了损坏隔离。
fn load_lock_file(path: &std::path::Path) -> (LockFile, bool) {
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return (LockFile::default(), false),
    };
    let parsed: Result<LockFile, _> = serde_json::from_str(&raw);
    let file = match parsed {
        Ok(f) if f.version == LOCK_FILE_VERSION && f.app.as_ref().map_or(true, HashEntry::valid)
            && f.session.as_ref().map_or(true, HashEntry::valid) =>
        {
            f
        }
        _ => {
            let corrupt_name = format!(
                "{}.corrupt-{}",
                LOCK_FILE_NAME,
                Utc::now().timestamp_millis()
            );
            let corrupt_path = path.with_file_name(corrupt_name);
            // 隔离失败不阻断：最坏情况是损坏文件仍在原地，下次读取再次隔离
            let _ = fs::rename(path, &corrupt_path);
            log::warn!(
                "lock.json 已损坏，已隔离为 {}",
                corrupt_path.display()
            );
            return (LockFile::default(), true);
        }
    };
    (file, false)
}

/// 原子写：同目录临时文件写完后 rename。
fn save_lock_file(path: &std::path::Path, file: &LockFile) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败: {e}"))?;
    }
    let json = serde_json::to_string_pretty(file).map_err(|e| format!("序列化 lock.json 失败: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| format!("写入 lock.json 临时文件失败: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("替换 lock.json 失败: {e}"))?;
    Ok(())
}

/* =========================================================
 *  哈希与比对
 * ========================================================= */

fn derive_hash(password: &str, salt: &[u8], iters: u32) -> Zeroizing<[u8; HASH_LEN]> {
    Zeroizing::new(pbkdf2_hmac_array::<Sha256, HASH_LEN>(
        password.as_bytes(),
        salt,
        iters,
    ))
}

fn new_entry(password: &str) -> Result<HashEntry, String> {
    let mut salt = [0u8; SALT_LEN];
    aes_gcm::aead::OsRng.fill_bytes(&mut salt);
    let hash = derive_hash(password, &salt, KDF_ITERS);
    Ok(HashEntry {
        salt: B64.encode(salt),
        hash: B64.encode(hash.as_slice()),
        iters: KDF_ITERS,
    })
}

/// 常量时间比对（hash 等长，长度不等直接判否不泄露有效信息）。
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

fn verify_entry(password: &str, entry: &HashEntry) -> bool {
    let Ok(salt) = B64.decode(entry.salt.as_bytes()) else {
        return false;
    };
    let expected = match B64.decode(entry.hash.as_bytes()) {
        Ok(b) if b.len() == HASH_LEN => b,
        _ => return false,
    };
    let actual = derive_hash(password, &salt, entry.iters);
    constant_time_eq(&actual[..], &expected)
}

fn valid_password_len(password: &str) -> bool {
    (PWD_MIN_CHARS..=PWD_MAX_CHARS).contains(&password.chars().count())
}

/* =========================================================
 *  限速状态
 * ========================================================= */

#[derive(Default)]
struct FailState {
    /// 累计连续失败次数（成功验证清零）
    fails: u32,
    /// 冷却结束时刻；None 表示不在冷却期
    until: Option<Instant>,
}

/// 解锁失败限速表（内存态，重启清零），同时序列化同槽位的文件写操作。
#[derive(Default)]
pub struct LockState {
    inner: Mutex<HashMap<&'static str, FailState>>,
}

#[derive(Debug, PartialEq, Eq)]
enum VerifyOutcome {
    Ok,
    /// 密码错误，本批次还可尝试 `remaining` 次
    BadPassword { remaining: u32 },
    /// 冷却中，还需等待 `secs` 秒
    Backoff { secs: u64 },
}

/// 第 n 批（n 从 1 起）的冷却秒数：30, 60, 120, 240, 300, 300 …
fn backoff_secs(batch: u32) -> u64 {
    let shift = batch.saturating_sub(1).min(8);
    BACKOFF_BASE_SECS
        .saturating_mul(1u64.wrapping_shl(shift))
        .min(BACKOFF_CAP_SECS)
}

/// 执行一次带限速的密码验证，调用方必须持 `LockState` 锁。
fn verify_locked(
    map: &mut HashMap<&'static str, FailState>,
    kind: &'static str,
    slot: Option<&HashEntry>,
    password: &str,
) -> VerifyOutcome {
    let st = map.entry(kind).or_default();

    if let Some(until) = st.until {
        if let Some(secs) = until.checked_duration_since(Instant::now()) {
            return VerifyOutcome::Backoff {
                secs: secs.as_secs().max(1),
            };
        }
        // 冷却已结束
        st.until = None;
    }

    let passed = slot.map(|e| verify_entry(password, e)).unwrap_or(false);
    if passed {
        *st = FailState::default();
        return VerifyOutcome::Ok;
    }

    st.fails = st.fails.saturating_add(1);
    if st.fails % FAIL_BATCH == 0 {
        let batch = st.fails / FAIL_BATCH;
        st.until = Some(Instant::now() + Duration::from_secs(backoff_secs(batch)));
        VerifyOutcome::Backoff {
            secs: backoff_secs(batch),
        }
    } else {
        VerifyOutcome::BadPassword {
            remaining: FAIL_BATCH - (st.fails % FAIL_BATCH),
        }
    }
}

/* =========================================================
 *  锁类型
 * ========================================================= */

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LockKind {
    App,
    Session,
}

impl LockKind {
    fn key(self) -> &'static str {
        match self {
            LockKind::App => "app",
            LockKind::Session => "session",
        }
    }

    fn slot<'a>(self, file: &'a LockFile) -> Option<&'a HashEntry> {
        match self {
            LockKind::App => file.app.as_ref(),
            LockKind::Session => file.session.as_ref(),
        }
    }

    fn set_slot(self, file: &mut LockFile, entry: Option<HashEntry>) {
        match self {
            LockKind::App => file.app = entry,
            LockKind::Session => file.session = entry,
        }
    }
}

impl TryFrom<&str> for LockKind {
    type Error = String;
    fn try_from(v: &str) -> Result<Self, String> {
        match v {
            "app" => Ok(LockKind::App),
            "session" => Ok(LockKind::Session),
            other => Err(format!("未知的锁类型: {other}")),
        }
    }
}

/* =========================================================
 *  applog
 * ========================================================= */

fn log_verify(kind: &str, outcome: &VerifyOutcome) {
    let (ok, backoff) = match outcome {
        VerifyOutcome::Ok => (true, false),
        VerifyOutcome::BadPassword { .. } => (false, false),
        VerifyOutcome::Backoff { .. } => (false, true),
    };
    crate::applog::emit(
        log::Level::Info,
        "app",
        "lock.verify",
        None,
        "锁密码验证",
        Some(serde_json::json!({ "kind": kind, "ok": ok, "backoff": backoff })),
    );
}

/* =========================================================
 *  IPC 命令
 * ========================================================= */

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LockStatusPayload {
    app_password_set: bool,
    session_password_set: bool,
    /// 本次读取是否发生了 lock.json 损坏隔离（前端用于告警文案）
    file_corrupted: bool,
}

/// 查询两级锁是否已设置密码（只返回布尔，不返回任何哈希材料）。
#[tauri::command]
pub fn lock_status(app: AppHandle) -> Result<LockStatusPayload, String> {
    let path = lock_path(&app)?;
    let (file, corrupted) = load_lock_file(&path);
    Ok(LockStatusPayload {
        app_password_set: file.app.is_some(),
        session_password_set: file.session.is_some(),
        file_corrupted: corrupted,
    })
}

/// 设置 / 修改锁密码。
///
/// 槽位为空时 `oldPassword` 必须为 null（首次设置）；槽位已有密码时必须传
/// 正确旧密码，旧密码验证同样受限速约束。
#[tauri::command]
pub fn lock_set_password(
    app: AppHandle,
    state: State<'_, LockState>,
    kind: String,
    password: String,
    old_password: Option<String>,
) -> Result<(), String> {
    let kind = LockKind::try_from(kind.as_str())?;
    let password = Zeroizing::new(password);
    if !valid_password_len(&password) {
        return Err(format!(
            "密码长度需为 {PWD_MIN_CHARS}~{PWD_MAX_CHARS} 个字符"
        ));
    }

    let path = lock_path(&app)?;
    let mut map = state.inner.lock().map_err(|e| format!("锁状态锁中毒: {e}"))?;
    let (mut file, _) = load_lock_file(&path);

    if kind.slot(&file).is_some() {
        // 修改密码：必须验证旧密码（与解锁共用限速通道）
        let old = Zeroizing::new(old_password.unwrap_or_default());
        let outcome = verify_locked(&mut map, kind.key(), kind.slot(&file), &old);
        log_verify(kind.key(), &outcome);
        match outcome {
            VerifyOutcome::Ok => {}
            VerifyOutcome::BadPassword { .. } => return Err("原密码错误".to_string()),
            VerifyOutcome::Backoff { secs } => {
                return Err(format!("尝试过于频繁，请在 {secs} 秒后重试"));
            }
        }
    }

    let entry = new_entry(&password)?;
    kind.set_slot(&mut file, Some(entry));
    save_lock_file(&path, &file)
}

/// 验证密码（解锁）。失败计入限速；冷却期不执行 KDF。
#[tauri::command]
pub fn lock_unlock(
    app: AppHandle,
    state: State<'_, LockState>,
    kind: String,
    password: String,
) -> Result<(), String> {
    let kind = LockKind::try_from(kind.as_str())?;
    let password = Zeroizing::new(password);

    let path = lock_path(&app)?;
    let mut map = state.inner.lock().map_err(|e| format!("锁状态锁中毒: {e}"))?;
    let (file, _) = load_lock_file(&path);

    let outcome = verify_locked(&mut map, kind.key(), kind.slot(&file), &password);
    log_verify(kind.key(), &outcome);
    match outcome {
        VerifyOutcome::Ok => Ok(()),
        VerifyOutcome::BadPassword { remaining } => {
            Err(format!("密码错误，还可尝试 {remaining} 次"))
        }
        VerifyOutcome::Backoff { secs } => {
            Err(format!("尝试过于频繁，请在 {secs} 秒后重试"))
        }
    }
}

/// 清除锁密码（关闭锁）。清除前必须验证当前密码。
#[tauri::command]
pub fn lock_clear_password(
    app: AppHandle,
    state: State<'_, LockState>,
    kind: String,
    password: String,
) -> Result<(), String> {
    let kind = LockKind::try_from(kind.as_str())?;
    let password = Zeroizing::new(password);

    let path = lock_path(&app)?;
    let mut map = state.inner.lock().map_err(|e| format!("锁状态锁中毒: {e}"))?;
    let (mut file, _) = load_lock_file(&path);

    if kind.slot(&file).is_none() {
        // 槽位本就为空：幂等成功
        return Ok(());
    }
    let outcome = verify_locked(&mut map, kind.key(), kind.slot(&file), &password);
    log_verify(kind.key(), &outcome);
    match outcome {
        VerifyOutcome::Ok => {}
        VerifyOutcome::BadPassword { remaining } => {
            return Err(format!("密码错误，还可尝试 {remaining} 次"));
        }
        VerifyOutcome::Backoff { secs } => {
            return Err(format!("尝试过于频繁，请在 {secs} 秒后重试"));
        }
    }
    kind.set_slot(&mut file, None);
    save_lock_file(&path, &file)
}

/* =========================================================
 *  单测
 * ========================================================= */

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn entry_for(pw: &str) -> HashEntry {
        new_entry(pw).unwrap()
    }

    #[test]
    fn entry_roundtrip_and_verify() {
        let e = entry_for("correct horse");
        assert!(e.valid());
        assert_eq!(B64.decode(e.salt.as_bytes()).unwrap().len(), SALT_LEN);
        assert_eq!(B64.decode(e.hash.as_bytes()).unwrap().len(), HASH_LEN);
        assert_eq!(e.iters, KDF_ITERS);
        assert!(verify_entry("correct horse", &e));
        assert!(!verify_entry("wrong horse", &e));
        // 哈希不是密码的任何直接编码
        assert!(!e.hash.contains("correct"));
    }

    #[test]
    fn fresh_salt_each_time() {
        let a = entry_for("same pw");
        let b = entry_for("same pw");
        assert_ne!(a.salt, b.salt);
        assert_ne!(a.hash, b.hash);
    }

    #[test]
    fn constant_time_eq_cases() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn password_length_bounds() {
        assert!(!valid_password_len("short")); // 5
        assert!(valid_password_len("123456")); // 6
        assert!(valid_password_len(&"中".repeat(128))); // 按字符数
        assert!(!valid_password_len(&"中".repeat(129)));
    }

    #[test]
    fn backoff_sequence_caps() {
        assert_eq!(backoff_secs(1), 30);
        assert_eq!(backoff_secs(2), 60);
        assert_eq!(backoff_secs(3), 120);
        assert_eq!(backoff_secs(4), 240);
        assert_eq!(backoff_secs(5), 300);
        assert_eq!(backoff_secs(6), 300);
    }

    #[test]
    fn verify_success_resets_failures() {
        let mut map: HashMap<&'static str, FailState> = HashMap::new();
        let e = entry_for("pw-1234");

        for _ in 0..3 {
            assert!(matches!(
                verify_locked(&mut map, "app", Some(&e), "bad"),
                VerifyOutcome::BadPassword { .. }
            ));
        }
        assert_eq!(verify_locked(&mut map, "app", Some(&e), "pw-1234"), VerifyOutcome::Ok);
        // 清零后重新给 4 次错误额度
        match verify_locked(&mut map, "app", Some(&e), "bad") {
            VerifyOutcome::BadPassword { remaining } => assert_eq!(remaining, 4),
            other => panic!("应为 BadPassword，实际 {other:?}"),
        }
    }

    #[test]
    fn five_failures_trigger_backoff_then_recover() {
        let mut map: HashMap<&'static str, FailState> = HashMap::new();
        let e = entry_for("pw-1234");

        for i in 1..=4u32 {
            match verify_locked(&mut map, "app", Some(&e), "bad") {
                VerifyOutcome::BadPassword { remaining } => assert_eq!(remaining, 5 - i),
                other => panic!("第 {i} 次应为 BadPassword，实际 {other:?}"),
            }
        }
        // 第 5 次直接进入冷却，且不消耗尝试额度文案
        match verify_locked(&mut map, "app", Some(&e), "bad") {
            VerifyOutcome::Backoff { secs } => assert_eq!(secs, 30),
            other => panic!("第 5 次应为 Backoff，实际 {other:?}"),
        }
        // 冷却期即使密码正确也拒绝
        assert!(matches!(
            verify_locked(&mut map, "app", Some(&e), "pw-1234"),
            VerifyOutcome::Backoff { .. }
        ));

        // 手动制造冷却已过期
        map.get_mut("app").unwrap().until = Some(Instant::now() - Duration::from_secs(1));
        assert_eq!(
            verify_locked(&mut map, "app", Some(&e), "pw-1234"),
            VerifyOutcome::Ok
        );
    }

    #[test]
    fn second_backoff_batch_doubles() {
        let mut map: HashMap<&'static str, FailState> = HashMap::new();
        let e = entry_for("pw-1234");
        // 第一批 5 次 → 30s 冷却，令其过期
        for _ in 0..5 {
            let _ = verify_locked(&mut map, "app", Some(&e), "bad");
        }
        map.get_mut("app").unwrap().until = Some(Instant::now() - Duration::from_secs(1));
        // 第二批再 5 次 → 60s
        for _ in 0..4 {
            assert!(matches!(
                verify_locked(&mut map, "app", Some(&e), "bad"),
                VerifyOutcome::BadPassword { .. }
            ));
        }
        match verify_locked(&mut map, "app", Some(&e), "bad") {
            VerifyOutcome::Backoff { secs } => assert_eq!(secs, 60),
            other => panic!("第二批应为 60s Backoff，实际 {other:?}"),
        }
    }

    #[test]
    fn missing_slot_never_passes() {
        let mut map: HashMap<&'static str, FailState> = HashMap::new();
        // 槽位 None：任何密码（含空串）都按失败计入限速
        assert!(matches!(
            verify_locked(&mut map, "session", None, "anything"),
            VerifyOutcome::BadPassword { .. }
        ));
    }

    #[test]
    fn file_absent_returns_default() {
        let p = std::env::temp_dir().join(format!("rhost-lock-nonexistent-{}.json", Uuid::new_v4()));
        let (f, corrupted) = load_lock_file(&p);
        assert!(!corrupted);
        assert!(f.app.is_none() && f.session.is_none());
    }

    #[test]
    fn file_write_read_roundtrip() {
        let p = std::env::temp_dir().join(format!("rhost-lock-roundtrip-{}.json", Uuid::new_v4()));
        let mut f = LockFile::default();
        f.app = Some(entry_for("app-pw-1"));
        f.session = Some(entry_for("sess-pw-1"));
        save_lock_file(&p, &f).unwrap();

        let (loaded, corrupted) = load_lock_file(&p);
        assert!(!corrupted);
        assert!(verify_entry("app-pw-1", loaded.app.as_ref().unwrap()));
        assert!(verify_entry("sess-pw-1", loaded.session.as_ref().unwrap()));
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn corrupted_file_is_quarantined() {
        let p = std::env::temp_dir().join(format!("rhost-lock-corrupt-{}.json", Uuid::new_v4()));
        fs::write(&p, b"{ this is not json").unwrap();
        let (f, corrupted) = load_lock_file(&p);
        assert!(corrupted);
        assert!(f.app.is_none() && f.session.is_none());
        // 原文件已改名隔离，同目录存在 corrupt 文件
        let dir = p.parent().unwrap();
        let mut found = false;
        if let Ok(entries) = fs::read_dir(dir) {
            for ent in entries.flatten() {
                if ent
                    .file_name()
                    .to_string_lossy()
                    .starts_with("lock.json.corrupt-")
                {
                    found = true;
                    let _ = fs::remove_file(ent.path());
                }
            }
        }
        assert!(found, "损坏文件应被改名隔离");
    }

    #[test]
    fn tampered_iters_is_corrupt() {
        let p = std::env::temp_dir().join(format!("rhost-lock-iters-{}.json", Uuid::new_v4()));
        let mut f = LockFile::default();
        let mut e = entry_for("pw-1234");
        e.iters = MAX_KDF_ITERS + 1;
        f.app = Some(e);
        save_lock_file(&p, &f).unwrap();
        let (loaded, corrupted) = load_lock_file(&p);
        assert!(corrupted);
        assert!(loaded.app.is_none());
        let _ = fs::remove_file(&p);
        if let Ok(entries) = fs::read_dir(p.parent().unwrap()) {
            for ent in entries.flatten() {
                if ent.file_name().to_string_lossy().starts_with("lock.json.corrupt-") {
                    let _ = fs::remove_file(ent.path());
                }
            }
        }
    }
}
