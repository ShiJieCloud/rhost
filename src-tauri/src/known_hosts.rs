//! 主机密钥指纹持久化（TOFU known_hosts）。
//!
//! 存储 `host + port` 精确匹配的已信任 SSH 主机密钥指纹，支撑首连确认
//! （TOFU）与密钥变更告警，设计见 `docs/explanation/design/hostkey-verification-design.md`：
//! - 键为用户配置的地址原样字符串 + 端口（IP/域名不解析等价，一主机一指纹，重复覆盖）；
//! - JSON + version 字段 + 原子写（临时文件 + rename，同 `store.rs::connections.json`）；
//! - 读取 fail-closed：文件不存在返回空；解析失败或 `version != 1`（未知未来版本）
//!   记 warn 后返回空——一律视为无记录走首连确认，绝不静默信任，下次 `record`
//!   直接以当前 schema 全量重写；
//! - 仅 [`path`] 触及 `AppHandle`，其余函数只接收 `PathBuf`（ssh/ 模块不依赖 tauri）。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

/// schema 版本：未知未来版本不做迁移，读取视为空、下次 record 全量重写
const FILE_VERSION: u32 = 1;
const FILE_NAME: &str = "known_hosts.json";

/// 单条已信任主机密钥记录
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownHostEntry {
    /// 主机地址（用户配置原样字符串）
    pub host: String,
    pub port: u16,
    /// 主机密钥算法标准名（如 `ssh-ed25519`）
    pub algo: String,
    /// SHA256 指纹（`SHA256:…`）
    pub fingerprint: String,
    /// 信任确认时间戳（epoch 毫秒）
    pub added_at: i64,
}

#[derive(Serialize, Deserialize)]
struct KnownHostsFile {
    version: u32,
    entries: Vec<KnownHostEntry>,
}

/// known_hosts.json 完整路径（app_data_dir 下）
pub fn path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("获取应用数据目录失败: {e}"))?;
    Ok(dir.join(FILE_NAME))
}

/// 读取全部条目；文件不存在返回空列表。
/// 解析失败（含部分损坏）或版本不符 → warn + 空（fail-closed）。
fn load_entries(path: &Path) -> Vec<KnownHostEntry> {
    let Ok(raw) = fs::read_to_string(path) else {
        // 不存在属正常（首次使用）；真实读失败下次 record 也会暴露，此处一律视为空
        return Vec::new();
    };
    if raw.trim().is_empty() {
        return Vec::new();
    }
    let file: KnownHostsFile = match serde_json::from_str(&raw) {
        Ok(f) => f,
        Err(e) => {
            log::warn!("known_hosts.json 解析失败，按无记录处理（fail-closed）: {e}");
            return Vec::new();
        }
    };
    if file.version != FILE_VERSION {
        log::warn!(
            "known_hosts.json 版本 {} 未知（当前 {}），按无记录处理并将在下次写入时全量重写",
            file.version,
            FILE_VERSION
        );
        return Vec::new();
    }
    file.entries
}

/// 查找 (host, port) 的已存指纹
pub fn lookup(path: &Path, host: &str, port: u16) -> Option<KnownHostEntry> {
    load_entries(path)
        .into_iter()
        .find(|e| e.host == host && e.port == port)
}

/// 新增或覆盖 (host, port) 记录（upsert；原子写：临时文件 + rename）。
pub fn record(path: &Path, entry: KnownHostEntry) -> Result<(), String> {
    let mut entries = load_entries(path);
    match entries
        .iter()
        .position(|e| e.host == entry.host && e.port == entry.port)
    {
        Some(idx) => entries[idx] = entry,
        None => entries.push(entry),
    }
    write_entries(path, &entries)
}

fn write_entries(path: &Path, entries: &[KnownHostEntry]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建数据目录失败: {e}"))?;
    }
    let file = KnownHostsFile {
        version: FILE_VERSION,
        entries: entries.to_vec(),
    };
    let json = serde_json::to_string_pretty(&file)
        .map_err(|e| format!("序列化 known_hosts.json 失败: {e}"))?;

    // 原子写：先写到同目录临时文件，成功后 rename 覆盖
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| format!("写入临时文件失败: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("替换 known_hosts.json 失败: {e}"))?;
    Ok(())
}

/// 当前 epoch 毫秒（record 时间戳；系统时钟早于 1970 时取 0）
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 唯一临时文件路径（测试内自清理，不引入 tempfile 依赖）
    fn tmp_path(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rhost-kh-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir.join(format!("{tag}.json"))
    }

    fn entry(host: &str, port: u16, fp: &str) -> KnownHostEntry {
        KnownHostEntry {
            host: host.into(),
            port,
            algo: "ssh-ed25519".into(),
            fingerprint: fp.into(),
            added_at: now_ms(),
        }
    }

    #[test]
    fn record_then_lookup_roundtrip() {
        let p = tmp_path("roundtrip");
        let _ = fs::remove_file(&p);
        record(&p, entry("a.example", 22, "SHA256:AAAA")).unwrap();
        let got = lookup(&p, "a.example", 22).expect("应命中");
        assert_eq!(got.fingerprint, "SHA256:AAAA");
        assert_eq!(got.algo, "ssh-ed25519");
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn upsert_overwrites_same_host_port() {
        let p = tmp_path("upsert");
        let _ = fs::remove_file(&p);
        record(&p, entry("a.example", 22, "SHA256:OLD")).unwrap();
        record(&p, entry("a.example", 22, "SHA256:NEW")).unwrap();
        // 一主机一指纹：覆盖而非追加
        assert_eq!(
            lookup(&p, "a.example", 22).unwrap().fingerprint,
            "SHA256:NEW"
        );
        let raw = fs::read_to_string(&p).unwrap();
        assert_eq!(raw.matches("SHA256:").count(), 1, "不得残留旧条目: {raw}");
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn distinct_hosts_and_ports_coexist() {
        let p = tmp_path("coexist");
        let _ = fs::remove_file(&p);
        record(&p, entry("a.example", 22, "SHA256:A")).unwrap();
        record(&p, entry("b.example", 22, "SHA256:B")).unwrap();
        record(&p, entry("a.example", 2222, "SHA256:C")).unwrap();
        assert_eq!(lookup(&p, "a.example", 22).unwrap().fingerprint, "SHA256:A");
        assert_eq!(lookup(&p, "b.example", 22).unwrap().fingerprint, "SHA256:B");
        assert_eq!(
            lookup(&p, "a.example", 2222).unwrap().fingerprint,
            "SHA256:C"
        );
        assert!(lookup(&p, "missing", 22).is_none());
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn missing_file_and_empty_lookup() {
        let p = tmp_path("missing-file");
        let _ = fs::remove_file(&p);
        assert!(
            lookup(&p, "a.example", 22).is_none(),
            "文件不存在应视为无记录"
        );
    }

    #[test]
    fn corrupted_file_is_fail_closed() {
        let p = tmp_path("corrupt");
        fs::write(&p, "{\"version\":1,\"entries\":[{\"host\":").unwrap();
        assert!(
            lookup(&p, "a.example", 22).is_none(),
            "损坏文件应视为无记录"
        );
        // record 以当前 schema 全量重写，覆盖损坏内容
        record(&p, entry("a.example", 22, "SHA256:FIXED")).unwrap();
        assert_eq!(
            lookup(&p, "a.example", 22).unwrap().fingerprint,
            "SHA256:FIXED"
        );
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn unknown_future_version_is_fail_closed() {
        let p = tmp_path("future");
        fs::write(
            &p,
            r#"{"version":2,"entries":[{"host":"a.example","port":22,"algo":"ssh-ed25519","fingerprint":"SHA256:EVIL","addedAt":1}]}"#,
        )
        .unwrap();
        assert!(
            lookup(&p, "a.example", 22).is_none(),
            "未知未来版本不做迁移，绝不静默信任"
        );
        let _ = fs::remove_file(&p);
    }
}
