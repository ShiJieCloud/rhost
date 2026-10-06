//! 主机配置持久化层。
//!
//! 存储策略：
//! - 连接配置（主机地址、端口、用户、认证方式等非敏感字段）以 JSON 文件形式
//!   存放在 Tauri 应用数据目录（`app_data_dir/connections.json`），人类可读、
//!   便于备份与迁移。
//! - 密码 / 私钥口令等敏感凭据**仅存系统钥匙串**，不写入任何磁盘文件：
//!   - macOS：通过 `security` 命令行工具（系统签名）操作 Keychain，加 `-A`
//!     参数允许所有应用访问，绕过 `cargo tauri dev` ad-hoc 签名的 ACL 限制；
//!   - 其他平台：使用 `keyring` crate。
//!
//! 写文件采用「先写临时文件再 rename」的原子策略，避免中途崩溃导致文件损坏。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
#[cfg(not(target_os = "macos"))]
use keyring::Entry;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

/// 单条主机连接的持久化表示。
///
/// 与前端 `Host` 类型对齐，但剔除运行时状态字段（`status` / `lat` / `cpu` /
/// `mem` / `uptime`）和敏感凭据（`password` / `keyPassphrase`）。
/// `extra` 保留该连接类型表单中的扩展字段（代理、跳板机、端口转发等），
/// 供未来前端消费。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredHost {
    /// 连接名称（唯一标识）
    pub id: String,
    /// 连接类型：ssh / sftp / local / serial / docker / telnet
    pub conn_type: String,
    pub user: String,
    pub ip: String,
    pub port: u16,
    pub os: String,
    /// 颜色标签：green / cyan / blue / purple / yellow / red
    pub color: String,
    /// 图标文字（名称首字母缩写）
    pub label: String,
    pub tag: String,
    pub group: String,
    /// 公钥认证私钥路径（仅路径，非敏感，可明文存储）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_path: Option<String>,
    /// 该连接类型的完整表单扩展数据（代理、跳板机、端口转发、终端设置等）
    #[serde(default)]
    pub extra: serde_json::Value,
    /// 最后更新时间戳（毫秒）
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Serialize, Deserialize)]
struct ConnectionsFile {
    version: u32,
    hosts: Vec<StoredHost>,
}

const FILE_VERSION: u32 = 1;
const FILE_NAME: &str = "connections.json";
const KEYRING_SERVICE: &str = "com.rhost.app";

/// 获取 connections.json 的完整路径（位于 Tauri app_data_dir 下）。
pub(crate) fn connections_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("获取应用数据目录失败: {e}"))?;
    Ok(dir.join(FILE_NAME))
}

/// 读取全部主机配置；文件不存在或为空时返回空列表。
pub fn load_hosts(app: &AppHandle) -> Result<Vec<StoredHost>, String> {
    let path = connections_path(app)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("读取 connections.json 失败: {e}"))?;
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let file: ConnectionsFile =
        serde_json::from_str(&raw).map_err(|e| format!("解析 connections.json 失败: {e}"))?;
    Ok(file.hosts)
}

/// 全量写入主机配置（原子写：临时文件 + rename）。
///
/// 调用方必须已持有全局配置写锁（`ConfigWriteLock`）。
pub(crate) fn write_hosts(path: &Path, hosts: &[StoredHost]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建数据目录失败: {e}"))?;
    }
    let file = ConnectionsFile {
        version: FILE_VERSION,
        hosts: hosts.to_vec(),
    };
    let json = serde_json::to_string_pretty(&file)
        .map_err(|e| format!("序列化 connections.json 失败: {e}"))?;

    // 原子写：先写到同目录临时文件，成功后 rename 覆盖
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| format!("写入临时文件失败: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("替换 connections.json 失败: {e}"))?;
    Ok(())
}

/// 新增或更新单条主机配置（按 id upsert）。
pub fn upsert_host(app: &AppHandle, mut host: StoredHost) -> Result<(), String> {
    host.updated_at = Utc::now().timestamp_millis();
    let path = connections_path(app)?;
    let mut hosts = load_hosts(app)?;
    if let Some(idx) = hosts.iter().position(|h| h.id == host.id) {
        hosts[idx] = host;
    } else {
        hosts.push(host);
    }
    write_hosts(&path, &hosts)
}

/// 按 id 删除主机配置。
pub fn delete_host(app: &AppHandle, id: &str) -> Result<(), String> {
    let path = connections_path(app)?;
    let mut hosts = load_hosts(app)?;
    hosts.retain(|h| h.id != id);
    write_hosts(&path, &hosts)
}

/* =========================================================
 *  导入合并（纯函数，不读盘；事务写盘由调用方在写锁内完成）
 * ========================================================= */

/// 主机导入合并结果
pub struct HostsMergeResult {
    /// 合并后的完整主机列表（existing 在前，incoming 追加在后）
    pub hosts: Vec<StoredHost>,
    /// 直接新增（id 无冲突）的条数
    pub added: usize,
    /// 因 id 冲突被改名的条数
    pub renamed: usize,
    /// 导入文件旧 id → 实际落盘 id 的映射（无冲突时映射到自身）。
    /// 用于重写 keys[].hosts 与 ui_state.sessions 引用。
    pub id_map: Vec<(String, String)>,
}

/// id 冲突时的第一次改名后缀
const IMPORT_SUFFIX: &str = "（导入）";

/// 将导入主机合并进现有列表（upsert by ID 的导入变体）。
///
/// 规则（§7.5）：
/// - id 无冲突：原样追加；
/// - id 已存在（含导入文件内部重复）：新 id = `原名（导入）`，再冲突追加序号
///   `原名（导入2）`、`原名（导入3）`……；
/// - 所有导入主机的 `keyPath` 强制清空（私钥路径不跨实例可信，避免连接上错误文件）；
/// - 纯函数：不读盘、不依赖 AppHandle，便于单测。
pub fn import_hosts_merge(existing: &[StoredHost], incoming: Vec<StoredHost>) -> HostsMergeResult {
    let mut hosts: Vec<StoredHost> = existing.to_vec();
    // 已占用 id 集合：现有 + 本次已合并（处理文件内部重复）
    let mut used: HashSet<String> = existing.iter().map(|h| h.id.clone()).collect();
    let mut added = 0usize;
    let mut renamed = 0usize;
    let mut id_map: Vec<(String, String)> = Vec::new();

    for mut host in incoming {
        let old_id = host.id.clone();
        host.key_path = None;

        if !used.contains(&old_id) {
            used.insert(old_id.clone());
            hosts.push(host);
            added += 1;
            id_map.push((old_id.clone(), old_id));
            continue;
        }

        // 冲突改名：base = `原名（导入）`，仍占用则追加从 2 起的序号
        let base = format!("{old_id}{IMPORT_SUFFIX}");
        let mut new_id = base.clone();
        let mut seq = 2;
        while used.contains(&new_id) {
            new_id = format!("{base}{seq}");
            seq += 1;
        }
        host.id = new_id.clone();
        used.insert(new_id.clone());
        hosts.push(host);
        renamed += 1;
        id_map.push((old_id, new_id));
    }

    HostsMergeResult {
        hosts,
        added,
        renamed,
        id_map,
    }
}

/* =========================================================
 *  密码存储：仅系统钥匙串
 * ========================================================= */

#[cfg(not(target_os = "macos"))]
fn keyring_entry(host_id: &str) -> Result<Entry, String> {
    Entry::new(KEYRING_SERVICE, host_id).map_err(|e| format!("创建钥匙串条目失败: {e}"))
}

/* ---- macOS 专用：security 命令行工具 ----
 *
 * macOS Keychain 基于代码签名做访问控制（ACL）。`cargo tauri dev` 模式下
 * 应用为 ad-hoc 签名，导致 keyring crate 写入成功但读取被拒（返回 NoEntry）。
 * `security` 命令行工具由系统签名，Keychain 信任它；配合 `-A` 参数允许所有
 * 应用访问该条目，彻底绕过签名限制。
 */

#[cfg(target_os = "macos")]
fn macos_security_save(host_id: &str, password: &str) -> Result<(), String> {
    let output = std::process::Command::new("security")
        .args([
            "add-generic-password",
            "-s",
            KEYRING_SERVICE,
            "-a",
            host_id,
            "-w",
            password,
            "-U", // 条目存在则更新
            "-A", // 允许所有应用访问（绕过 ad-hoc 签名 ACL）
        ])
        .output()
        .map_err(|e| format!("执行 security 命令失败: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "security add-generic-password 失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(target_os = "macos")]
fn macos_security_get(host_id: &str) -> Result<Option<String>, String> {
    let output = std::process::Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            KEYRING_SERVICE,
            "-a",
            host_id,
            "-w", // 仅输出密码
        ])
        .output()
        .map_err(|e| format!("执行 security 命令失败: {e}"))?;
    if output.status.success() {
        let pwd = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(Some(pwd))
    } else {
        // exit code 44 = 条目不存在
        Ok(None)
    }
}

#[cfg(target_os = "macos")]
fn macos_security_delete(host_id: &str) -> Result<(), String> {
    let output = std::process::Command::new("security")
        .args([
            "delete-generic-password",
            "-s",
            KEYRING_SERVICE,
            "-a",
            host_id,
        ])
        .output()
        .map_err(|e| format!("执行 security 命令失败: {e}"))?;
    // 条目不存在不报错
    let _ = output.status;
    Ok(())
}

/// 统一的钥匙串保存接口：macOS 用 security CLI，其余平台用 keyring crate。
fn keyring_save(host_id: &str, password: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        macos_security_save(host_id, password)
    }
    #[cfg(not(target_os = "macos"))]
    {
        keyring_entry(host_id)?
            .set_password(password)
            .map_err(|e| e.to_string())
    }
}

/// 统一的钥匙串读取接口。
fn keyring_get(host_id: &str) -> Result<Option<String>, String> {
    #[cfg(target_os = "macos")]
    {
        macos_security_get(host_id)
    }
    #[cfg(not(target_os = "macos"))]
    {
        match keyring_entry(host_id)?.get_password() {
            Ok(pwd) => Ok(Some(pwd)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// 统一的钥匙串删除接口。
fn keyring_delete(host_id: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        macos_security_delete(host_id)
    }
    #[cfg(not(target_os = "macos"))]
    {
        match keyring_entry(host_id)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/* ---- 对外接口 ---- */

/// 保存密码到系统钥匙串。
pub fn save_password(_app: &AppHandle, host_id: &str, password: &str) -> Result<(), String> {
    log::info!("save_password: 保存主机 [{host_id}] 的密码到系统钥匙串");
    keyring_save(host_id, password)
}

/// 从系统钥匙串读取密码。
pub fn get_password(_app: &AppHandle, host_id: &str) -> Result<Option<String>, String> {
    log::info!("get_password: 从系统钥匙串读取主机 [{host_id}] 的密码");
    keyring_get(host_id)
}

/// 从系统钥匙串删除密码。
pub fn delete_password(_app: &AppHandle, host_id: &str) -> Result<(), String> {
    log::info!("delete_password: 从系统钥匙串删除主机 [{host_id}] 的密码");
    if let Err(e) = keyring_delete(host_id) {
        log::warn!("delete_password: 钥匙串删除失败: {e}");
    }
    Ok(())
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    fn host(id: &str) -> StoredHost {
        StoredHost {
            id: id.into(),
            conn_type: "ssh".into(),
            user: "root".into(),
            ip: "127.0.0.1".into(),
            port: 22,
            os: String::new(),
            color: "blue".into(),
            label: "t".into(),
            tag: String::new(),
            group: String::new(),
            key_path: Some("/Users/me/.ssh/id_ed25519".into()),
            extra: serde_json::Value::Null,
            updated_at: 0,
        }
    }

    #[test]
    fn no_conflict_appends_and_maps_identity() {
        let existing = vec![host("a")];
        let r = import_hosts_merge(&existing, vec![host("b"), host("c")]);
        assert_eq!(r.hosts.len(), 3);
        assert_eq!(r.added, 2);
        assert_eq!(r.renamed, 0);
        assert_eq!(r.id_map, vec![("b".into(), "b".into()), ("c".into(), "c".into())]);
        // 导入项 keyPath 强制清空（存量项在另一个用例验证）
        assert!(r.hosts[1..].iter().all(|h| h.key_path.is_none()));
    }

    #[test]
    fn conflict_gets_import_suffix() {
        let existing = vec![host("a")];
        let r = import_hosts_merge(&existing, vec![host("a")]);
        assert_eq!(r.hosts.len(), 2);
        assert_eq!(r.added, 0);
        assert_eq!(r.renamed, 1);
        assert_eq!(r.hosts[1].id, "a（导入）");
        assert_eq!(r.id_map, vec![("a".into(), "a（导入）".into())]);
    }

    #[test]
    fn repeated_import_keeps_incrementing_and_internal_duplicates_rename() {
        // 已存在 a、a（导入） → 再导入两个 a：分别得 a（导入2）、a（导入3）
        let existing = vec![host("a"), host("a（导入）")];
        let r = import_hosts_merge(&existing, vec![host("a"), host("a")]);
        assert_eq!(r.renamed, 2);
        assert_eq!(r.hosts[2].id, "a（导入）2");
        assert_eq!(r.hosts[3].id, "a（导入）3");
    }

    #[test]
    fn existing_entries_keep_their_keypath() {
        let existing = vec![host("a")];
        let r = import_hosts_merge(&existing, vec![host("b")]);
        // 只有导入项清 keyPath，存量项不动
        assert!(r.hosts[0].key_path.is_some());
        assert!(r.hosts[1].key_path.is_none());
    }
}
