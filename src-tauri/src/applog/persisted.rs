//! 应用整体配置的后端持久化（`app_config.json`，位于 Tauri app config dir）。
//!
//! 定位：与 WebView localStorage 独立的**后端侧应用配置文件**，按「整体配置」组织，
//! 当前仅承载日志配置节（`logStoragePath`「重启生效」要求后端在冷启动瞬间、
//! 前端通道就绪前读到目录），后续可在同一文件内增加 `terminal` / `ui` 等节，
//! 并作为整体配置导入导出的数据源。
//!
//! 文件示例：
//! ```json
//! {
//!   "version": 1,
//!   "logs": { "collect": true, "level": "info", "storage_path": "", ... }
//! }
//! ```
//!
//! 兼容策略（导入导出与版本演进的基础）：
//! - 顶层 `version`：结构版本号，后续迁移依据；
//! - 所有字段 `#[serde(default)]`：文件缺失 / 损坏 / 旧版缺字段均安全回退默认；
//! - `#[serde(flatten)] extra`：保留本版本不认识的顶层节——新版本写入的配置
//!   被旧版本「读—改—写」时不会丢失（前向兼容）。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::hub::{HubConfig, RotateStrategy};

/// 当前配置结构版本
const CURRENT_VERSION: u32 = 1;

/// 配置文件名（app config dir 下）
pub const APP_CONFIG_FILE_NAME: &str = "app_config.json";

/// 后端侧应用配置（按节扩展；当前只有日志一节）
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub logs: HubConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            logs: HubConfig::default(),
        }
    }
}

/// `logs` 节磁盘表示（全部字段带默认值：部分缺失/旧版本文件仍可解析）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct LogsSection {
    collect: bool,
    level: String,
    max_lines: u64,
    persist: bool,
    storage_path: String,
    rotate: String,
    max_files: u64,
    retention_days: u64,
    /// logs 节内本版本不认识的字段，读改写时原样保留
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for LogsSection {
    fn default() -> Self {
        Self::from(&HubConfig::default())
    }
}

impl From<&HubConfig> for LogsSection {
    fn from(cfg: &HubConfig) -> Self {
        Self {
            collect: cfg.collect,
            level: cfg.level.to_string().to_lowercase(),
            max_lines: cfg.max_lines as u64,
            persist: cfg.persist,
            storage_path: cfg.storage_path.clone(),
            rotate: cfg.rotate.as_str().to_string(),
            max_files: cfg.max_files as u64,
            retention_days: cfg.retention_days,
            extra: serde_json::Map::new(),
        }
    }
}

impl LogsSection {
    fn into_hub_config(self) -> HubConfig {
        HubConfig {
            collect: self.collect,
            level: super::hub::parse_level(&self.level),
            // 0 会导致缓冲容量/清理数量非法，夹到 1
            max_lines: self.max_lines.max(1) as usize,
            persist: self.persist,
            storage_path: self.storage_path,
            rotate: RotateStrategy::from_str(&self.rotate),
            max_files: self.max_files.max(1) as usize,
            retention_days: self.retention_days,
        }
    }
}

/// 配置文件顶层结构（version + 分节；未知节经 extra 原样保留）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct AppConfigFile {
    version: u32,
    logs: LogsSection,
    /// 本版本不认识的其他顶层节（如未来的 terminal/ui），读改写时原样带回
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for AppConfigFile {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            logs: LogsSection::default(),
            extra: serde_json::Map::new(),
        }
    }
}

/// 启动时读取整体配置：文件不存在 / JSON 损坏 / 字段非法均回退默认值。
pub fn load(path: &Path) -> AppConfig {
    let file = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str::<AppConfigFile>(&text).unwrap_or_default(),
        Err(_) => return AppConfig::default(),
    };
    // 未来在此按 file.version 做结构迁移（v1 → v2 …）
    AppConfig {
        logs: file.logs.into_hub_config(),
    }
}

/// 完整写入整体配置（整体导入 / 初始化场景）。原子写，目标父目录自动创建。
//
// 预留 API：当前生产路径用 save_logs（读改写保留未知节）；待「设置整体导入/导出」
// 落地时由其消费。单测已覆盖往返，故以 allow(dead_code) 显式标记为有意保留。
#[allow(dead_code)]
pub fn save(path: &Path, cfg: &AppConfig) -> std::io::Result<()> {
    let file = AppConfigFile {
        version: CURRENT_VERSION,
        logs: LogsSection::from(&cfg.logs),
        extra: serde_json::Map::new(),
    };
    write_atomic(path, &file)
}

/// 只更新 `logs` 节：读出现有文件（含未知节）→ 替换 logs → 原子写回。
/// `set_log_config` 使用，保证未来新增配置节不被覆盖丢失。
pub fn save_logs(path: &Path, logs: &HubConfig) -> std::io::Result<()> {
    let mut file = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str::<AppConfigFile>(&text).unwrap_or_default(),
        Err(_) => AppConfigFile::default(),
    };
    file.version = CURRENT_VERSION;
    // 替换已知字段，带回旧 logs 节内的未知字段（前向兼容）
    let mut new_logs = LogsSection::from(logs);
    new_logs.extra = std::mem::take(&mut file.logs.extra);
    file.logs = new_logs;
    write_atomic(path, &file)
}

/// 原子写：同目录临时文件 + rename（同卷 rename 原子，崩溃不留半截 JSON）
fn write_atomic(path: &Path, file: &AppConfigFile) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(file)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = tmp_file_path(path);
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// 同目录临时文件名（与目标同卷，保证 rename 原子）
fn tmp_file_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| format!(".{}.tmp", n.to_string_lossy()))
        .unwrap_or_else(|| format!(".{APP_CONFIG_FILE_NAME}.tmp"));
    let Some(parent) = path.parent() else {
        return PathBuf::from(name);
    };
    parent.join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_dir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "rhost-cfg-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn custom_logs() -> HubConfig {
        HubConfig {
            collect: false,
            level: log::LevelFilter::Debug,
            max_lines: 1234,
            persist: false,
            storage_path: "~/my-logs".to_string(),
            rotate: RotateStrategy::Weekly,
            max_files: 7,
            retention_days: 99,
        }
    }

    #[test]
    fn missing_file_returns_default() {
        let dir = unique_dir("missing");
        let cfg = load(&dir.join(APP_CONFIG_FILE_NAME));
        let d = HubConfig::default();
        assert_eq!(cfg.logs.collect, d.collect);
        assert_eq!(cfg.logs.persist, d.persist);
        assert_eq!(cfg.logs.storage_path, d.storage_path);
    }

    #[test]
    fn corrupted_json_returns_default() {
        let dir = unique_dir("corrupt");
        let f = dir.join(APP_CONFIG_FILE_NAME);
        std::fs::write(&f, "{ not json").unwrap();
        assert!(load(&f).logs.persist);
    }

    #[test]
    fn save_then_load_roundtrip_custom_logs() {
        let dir = unique_dir("roundtrip");
        // 父目录不存在也应自动建
        let f = dir.join("nested").join(APP_CONFIG_FILE_NAME);
        save(&f, &AppConfig { logs: custom_logs() }).unwrap();

        let loaded = load(&f).logs;
        assert!(!loaded.collect);
        assert_eq!(loaded.level, log::LevelFilter::Debug);
        assert_eq!(loaded.max_lines, 1234);
        assert!(!loaded.persist);
        assert_eq!(loaded.storage_path, "~/my-logs");
        assert_eq!(loaded.rotate, RotateStrategy::Weekly);
        assert_eq!(loaded.max_files, 7);
        assert_eq!(loaded.retention_days, 99);

        // 原子写不留临时文件；顶层带 version
        assert!(!dir.join("nested/.app_config.json.tmp").exists());
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(raw["version"], 1);
        assert!(raw["logs"].is_object());
    }

    #[test]
    fn partial_fields_merge_with_defaults() {
        let dir = unique_dir("partial");
        let f = dir.join(APP_CONFIG_FILE_NAME);
        // 旧版本文件：logs 节只有两个字段
        std::fs::write(&f, r#"{"version":1,"logs":{"persist":false,"storage_path":"/var/log/x"}}"#)
            .unwrap();
        let cfg = load(&f).logs;
        assert!(!cfg.persist);
        assert_eq!(cfg.storage_path, "/var/log/x");
        assert_eq!(cfg.max_lines, HubConfig::default().max_lines);
    }

    #[test]
    fn missing_logs_section_returns_default_logs() {
        let dir = unique_dir("nologs");
        let f = dir.join(APP_CONFIG_FILE_NAME);
        std::fs::write(&f, r#"{"version":1}"#).unwrap();
        let cfg = load(&f).logs;
        assert_eq!(cfg.persist, HubConfig::default().persist);
    }

    #[test]
    fn illegal_level_and_zeros_fall_back_safely() {
        let dir = unique_dir("illegal");
        let f = dir.join(APP_CONFIG_FILE_NAME);
        std::fs::write(
            &f,
            r#"{"logs":{"level":"VERBOSE","max_lines":0,"max_files":0}}"#,
        )
        .unwrap();
        let cfg = load(&f).logs;
        assert_eq!(cfg.level, log::LevelFilter::Info); // 非法级别回 Info
        assert!(cfg.max_lines >= 1); // 0 夹到 1
        assert!(cfg.max_files >= 1);
    }

    #[test]
    fn save_logs_preserves_unknown_sections() {
        let dir = unique_dir("preserve");
        let f = dir.join(APP_CONFIG_FILE_NAME);
        // 模拟「未来版本」写入的未知顶层节与未知 logs 字段
        std::fs::write(
            &f,
            r#"{
                "version": 1,
                "logs": {"persist": true, "future_log_flag": 42},
                "terminal": {"fontSize": 14},
                "ui": {"theme": "dark"}
            }"#,
        )
        .unwrap();

        save_logs(&f, &custom_logs()).unwrap();

        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        // 未知顶层节原样保留（前向兼容：旧版读改写不丢新版配置）
        assert_eq!(raw["terminal"]["fontSize"], 14);
        assert_eq!(raw["ui"]["theme"], "dark");
        // logs 已更新为新值；logs 节内未知字段也保留
        assert_eq!(raw["logs"]["storage_path"], "~/my-logs");
        assert_eq!(raw["logs"]["level"], "debug");
        assert_eq!(raw["logs"]["future_log_flag"], 42);
    }
}
