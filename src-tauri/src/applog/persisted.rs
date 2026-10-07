//! 应用整体配置的后端持久化（`app_config.json`，位于 Tauri app config dir）。
//!
//! 定位：应用配置的**唯一持久化层**，按节组织：
//! - `logs`：日志配置（冷启动需要，HubConfig；`logStoragePath`「重启生效」）
//! - `settings`：应用设置（外观/终端/快捷键/SFTP 等，不含 log* 字段）
//! - `keys`：SSH 密钥元数据（私钥内容仅存 ~/.ssh）
//! - `groups`：分组定义
//! - `ui_state`：homeView / layout / sessions / logWrap / 导出范围勾选
//! - `quick_connect_history`：快速连接历史
//!
//! 兼容策略（导入导出与版本演进的基础）：
//! - 顶层 `version`：结构版本号，后续迁移依据；
//! - 所有字段 `#[serde(default)]`：文件缺失 / 损坏 / 旧版缺字段均安全回退默认；
//! - `#[serde(flatten)] extra`：保留本版本不认识的顶层节——新版本写入的配置
//!   被旧版本「读—改—写」时不会丢失（前向兼容）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::hub::{HubConfig, RotateStrategy};

/// 全局配置写锁：串行化所有对 `app_config.json` 与 `connections.json` 的写操作
/// （settings/keys/groups/ui_state/history 写穿、日志配置落盘、连接增删、
/// 配置导入/备份恢复/重置）。
///
/// 两个文件可能在同一事务内被写（导入），因此共用一把锁，避免交叉写导致
/// 「connections 已更新但 app_config 引用未重映射」之类的中间态被读到。
#[derive(Clone)]
pub struct ConfigWriteLock(pub Arc<tokio::sync::Mutex<()>>);

impl ConfigWriteLock {
    pub fn new() -> Self {
        Self(Arc::new(tokio::sync::Mutex::new(())))
    }
}

impl Default for ConfigWriteLock {
    fn default() -> Self {
        Self::new()
    }
}

/// 当前配置结构版本
const CURRENT_VERSION: u32 = 1;

/// 配置文件名（app config dir 下）
pub const APP_CONFIG_FILE_NAME: &str = "app_config.json";

/// 允许写穿（`save_section`）的节名白名单
pub const SECTIONS: &[&str] = &[
    "logs",
    "settings",
    "keys",
    "groups",
    "ui_state",
    "quick_connect_history",
];

/* =========================================================
 *  各节强类型表示（反序列化即 schema 校验）
 * ========================================================= */

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
    extra: Map<String, Value>,
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
            extra: Map::new(),
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

/// 环境变量键值对（settings.env 元素）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EnvVarSection {
    #[serde(default)]
    key: String,
    #[serde(default)]
    value: String,
}

/// `settings` 节强类型表示。字段与前端 `AppSettings` 对齐（不含 log* 字段——
/// 日志配置的权威存储是 `logs` 节）；字段缺失补默认、**类型不符即反序列化失败**
/// （写前校验拒绝）；未知字段经 `extra` 保留（前向兼容）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct SettingsSection {
    // 外观
    ui_theme: String,
    accent: String,
    opacity: i64,
    splash_duration_ms: i64,
    splash_transparent: bool,
    // 字体
    font_family: String,
    font_size: i64,
    line_height: i64,
    font_weight: String,
    // 终端
    color_prompt: bool,
    motd: bool,
    motd_logo_on: bool,
    motd_logo: String,
    scrollback: i64,
    trim_on_copy: bool,
    paste_guard: bool,
    right_click: String,
    cursor_style: String,
    cursor_blink: bool,
    auto_reconnect: bool,
    auto_reconnect_max_attempts: i64,
    env: Vec<EnvVarSection>,
    // 快捷键（键名含点，serde 按磁盘 camelCase 名显式 rename）
    #[serde(rename = "key.newTab")]
    key_new_tab: String,
    #[serde(rename = "key.closeTab")]
    key_close_tab: String,
    #[serde(rename = "key.splitV")]
    key_split_v: String,
    #[serde(rename = "key.splitH")]
    key_split_h: String,
    #[serde(rename = "key.clear")]
    key_clear: String,
    #[serde(rename = "key.palette")]
    key_palette: String,
    #[serde(rename = "key.find")]
    key_find: String,
    #[serde(rename = "key.settings")]
    key_settings: String,
    // 高级
    gpu_accel: bool,
    renderer: String,
    // 监控
    mem_interval: i64,
    metrics_interval: i64,
    mem_pause_hidden: bool,
    mem_alert_mb: i64,
    // SFTP
    sftp_chunk_kb: i64,
    sftp_resume: bool,
    sftp_resume_check: String,
    sftp_overwrite_policy: String,
    sftp_upload_temp: bool,
    sftp_preserve_meta: bool,
    sftp_global_concurrency: i64,
    sftp_host_concurrency: i64,
    sftp_global_rate_kb: i64,
    sftp_task_rate_kb: i64,
    sftp_retry_count: i64,
    sftp_retry_interval_ms: i64,
    sftp_idle_timeout_sec: i64,
    sftp_verify_hash: bool,
    sftp_blacklist: String,
    sftp_show_hidden: bool,
    // 关于
    auto_update: bool,
    update_channel: String,
    /// 本版本不认识的 settings 字段，读改写原样保留
    #[serde(flatten)]
    extra: Map<String, Value>,
}

impl Default for SettingsSection {
    /// 与前端 `DEFAULT_SETTINGS`（非 log* 部分）严格保持一致：
    /// 「恢复默认设置」写入此默认值，两端不得漂移。
    fn default() -> Self {
        Self {
            ui_theme: "dark".into(),
            accent: "#3ddc84".into(),
            opacity: 96,
            splash_duration_ms: 400,
            splash_transparent: true,

            font_family: "JetBrains Mono".into(),
            font_size: 13,
            line_height: 145,
            font_weight: "400".into(),

            color_prompt: true,
            motd: true,
            motd_logo_on: false,
            motd_logo: String::new(),
            scrollback: 10000,
            trim_on_copy: true,
            paste_guard: false,
            right_click: "menu".into(),
            cursor_style: "bar".into(),
            cursor_blink: true,
            auto_reconnect: true,
            auto_reconnect_max_attempts: 5,
            env: vec![
                EnvVarSection {
                    key: "EDITOR".into(),
                    value: "nvim".into(),
                },
                EnvVarSection {
                    key: "LANG".into(),
                    value: "zh_CN.UTF-8".into(),
                },
            ],

            key_new_tab: "⌘+T".into(),
            key_close_tab: "⌘+W".into(),
            key_split_v: "⌘+D".into(),
            key_split_h: "⌘+E".into(),
            key_clear: "⌘+L".into(),
            key_palette: "⌘+K".into(),
            key_find: "⌘+F".into(),
            key_settings: "⌘+,".into(),

            gpu_accel: true,
            renderer: "auto".into(),

            mem_interval: 2,
            metrics_interval: 3,
            mem_pause_hidden: true,
            mem_alert_mb: 300,

            sftp_chunk_kb: 64,
            sftp_resume: true,
            sftp_resume_check: "size".into(),
            sftp_overwrite_policy: "newer".into(),
            sftp_upload_temp: true,
            sftp_preserve_meta: false,
            sftp_global_concurrency: 3,
            sftp_host_concurrency: 1,
            sftp_global_rate_kb: 0,
            sftp_task_rate_kb: 0,
            sftp_retry_count: 3,
            sftp_retry_interval_ms: 1000,
            sftp_idle_timeout_sec: 300,
            sftp_verify_hash: false,
            sftp_blacklist: ".DS_Store\nThumbs.db".into(),
            sftp_show_hidden: false,

            auto_update: true,
            update_channel: "stable".into(),

            extra: Map::new(),
        }
    }
}

/// `keys` 节元素：id/name/type 必填（缺失即拒绝），其余字段缺省；未知字段保留。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SshKeySection {
    id: String,
    name: String,
    #[serde(rename = "type")]
    key_type: String,
    #[serde(default)]
    bits: String,
    #[serde(default)]
    comment: String,
    #[serde(default)]
    fingerprint: String,
    #[serde(default)]
    public_key: String,
    /// 私钥本地路径；日常写穿保留，**导出组装时剔除**
    #[serde(default, skip_serializing_if = "Option::is_none")]
    private_path: Option<String>,
    #[serde(default)]
    passphrase: bool,
    #[serde(default)]
    created: Option<String>,
    #[serde(default)]
    last_used: Option<String>,
    #[serde(default)]
    hosts: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

/// `groups` 节元素：name/color 必填
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GroupSection {
    name: String,
    color: String,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

/// `ui_state.sessions` 条目：会话持久化唯一键 + 主机 id + 标签别名
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionEntry {
    pub session_id: String,
    pub host_id: String,
    #[serde(default)]
    pub alias: String,
}

/// `ui_state` 节：已知子字段校验类型，未知子字段经 extra 保留
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct UiStateSection {
    home_view: Option<String>,
    layout: Value,
    sessions: Vec<SessionEntry>,
    log_wrap: Option<bool>,
    config_export_scope: Option<String>,
    config_export_include_ui: Option<bool>,
    config_export_include_history: Option<bool>,
    config_export_encrypted: Option<bool>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

/// `quick_connect_history` 节元素：host/timestamp 必填
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuickConnectItem {
    host: String,
    timestamp: i64,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

/* =========================================================
 *  文件顶层结构与读取
 * ========================================================= */

fn empty_object() -> Value {
    Value::Object(Map::new())
}
fn empty_array() -> Value {
    Value::Array(Vec::new())
}

/// 配置文件顶层结构（version + 分节；未知节经 extra 原样保留）。
///
/// 新节以 `Value` 形态持有：磁盘上是什么就保留什么，强类型 struct 仅在
/// 写穿/导入前做反序列化校验，避免已知字段之外的任何信息被读改写丢掉。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfigFile {
    version: u32,
    logs: LogsSection,
    #[serde(default = "empty_object")]
    pub settings: Value,
    #[serde(default = "empty_array")]
    pub keys: Value,
    #[serde(default = "empty_array")]
    pub groups: Value,
    #[serde(default = "empty_object")]
    pub ui_state: Value,
    #[serde(default = "empty_array")]
    pub quick_connect_history: Value,
    /// 本版本不认识的其他顶层节，读改写时原样带回
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for AppConfigFile {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            logs: LogsSection::default(),
            settings: empty_object(),
            keys: empty_array(),
            groups: empty_array(),
            ui_state: empty_object(),
            quick_connect_history: empty_array(),
            extra: Map::new(),
        }
    }
}

impl AppConfigFile {
    /// 取出某节的原始 Value（节名非法返回 None）
    pub fn section_value(&self, section: &str) -> Option<Value> {
        Some(match section {
            "logs" => serde_json::to_value(&self.logs).unwrap_or_else(|_| empty_object()),
            "settings" => self.settings.clone(),
            "keys" => self.keys.clone(),
            "groups" => self.groups.clone(),
            "ui_state" => self.ui_state.clone(),
            "quick_connect_history" => self.quick_connect_history.clone(),
            _ => return None,
        })
    }

    fn set_section_value(&mut self, section: &str, value: Value) {
        match section {
            "logs" => {
                if let Ok(v) = serde_json::from_value::<LogsSection>(value) {
                    self.logs = v;
                }
            }
            "settings" => self.settings = value,
            "keys" => self.keys = value,
            "groups" => self.groups = value,
            "ui_state" => self.ui_state = value,
            "quick_connect_history" => self.quick_connect_history = value,
            _ => {}
        }
    }

    /// logs 节转 HubConfig（启动热路径）
    pub fn logs_config(&self) -> HubConfig {
        self.logs.clone().into_hub_config()
    }
}

/// 读取完整配置文件（全节）；文件不存在 / JSON 损坏均回退默认值。
pub fn load_full(path: &Path) -> AppConfigFile {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str::<AppConfigFile>(&text).unwrap_or_default(),
        Err(_) => AppConfigFile::default(),
    }
}

/// 只更新 `logs` 节：读出现有文件（含未知节）→ 替换 logs → 原子写回。
pub fn save_logs(path: &Path, logs: &HubConfig) -> std::io::Result<()> {
    let mut file = load_full(path);
    file.version = CURRENT_VERSION;
    // 替换已知字段，带回旧 logs 节内的未知字段（前向兼容）
    let mut new_logs = LogsSection::from(logs);
    new_logs.extra = std::mem::take(&mut file.logs.extra);
    file.logs = new_logs;
    write_atomic(path, &file)
}

/* =========================================================
 *  通用节写穿（白名单 + 强类型校验 + 读改写）
 * ========================================================= */

/// 校验某节 value 的 schema，返回**规范化后**（未知字段已保留）的可落盘 Value。
///
/// - 未知节名 → Err；
/// - 字段类型不符 / 数组元素缺必填字段 → Err（调用方记 write_rejected，零写入）；
/// - 字段缺失 → 强类型 struct 的 `#[serde(default)]` 补默认。
fn validate_section(section: &str, value: Value) -> Result<Value, String> {
    let normalized = match section {
        "logs" => {
            let mut typed = serde_json::from_value::<LogsSection>(value)
                .map_err(|e| format!("logs 节格式错误: {e}"))?;
            // sanitize：非法枚举/0 值在 into_hub_config 内夹取，再转回磁盘形态；
            // 未知 logs 字段（extra）原样带回
            let extra = std::mem::take(&mut typed.extra);
            let mut sanitized = LogsSection::from(&typed.into_hub_config());
            sanitized.extra = extra;
            serde_json::to_value(&sanitized).map_err(|e| format!("logs 节序列化失败: {e}"))?
        }
        "settings" => {
            let typed = serde_json::from_value::<SettingsSection>(value)
                .map_err(|e| format!("settings 节格式错误: {e}"))?;
            serde_json::to_value(&typed).map_err(|e| format!("settings 节序列化失败: {e}"))?
        }
        "keys" => validate_array::<SshKeySection>(section, value)?,
        "groups" => validate_array::<GroupSection>(section, value)?,
        "ui_state" => {
            let typed = serde_json::from_value::<UiStateSection>(value)
                .map_err(|e| format!("ui_state 节格式错误: {e}"))?;
            serde_json::to_value(&typed).map_err(|e| format!("ui_state 节序列化失败: {e}"))?
        }
        "quick_connect_history" => validate_array::<QuickConnectItem>(section, value)?,
        other => return Err(format!("未知配置节: {other}")),
    };
    Ok(normalized)
}

/// 数组节逐元素反序列化校验，再整体序列化回数组
fn validate_array<T>(section: &str, value: Value) -> Result<Value, String>
where
    T: Serialize + for<'de> Deserialize<'de>,
{
    let arr = value
        .as_array()
        .ok_or_else(|| format!("{section} 节必须是数组"))?;
    let mut typed_items: Vec<T> = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().cloned().enumerate() {
        let typed = serde_json::from_value::<T>(item)
            .map_err(|e| format!("{section} 节第 {i} 项格式错误: {e}"))?;
        typed_items.push(typed);
    }
    serde_json::to_value(&typed_items).map_err(|e| format!("{section} 节序列化失败: {e}"))
}

/// 通用写穿：校验 value → 读出现有文件 → 替换指定节 → 原子写回。
///
/// 未知顶层节与节内未知字段均原样保留。调用方必须已持有全局配置写锁。
pub fn save_section(path: &Path, section: &str, value: Value) -> Result<(), String> {
    if !SECTIONS.contains(&section) {
        return Err(format!("未知配置节: {section}"));
    }
    let normalized = validate_section(section, value)?;
    let mut file = load_full(path);
    file.version = CURRENT_VERSION;
    file.set_section_value(section, normalized);
    write_atomic(path, &file).map_err(|e| format!("写入 app_config.json 失败: {e}"))
}

/// 仅将 settings 节替换为内置默认值（「恢复默认设置」），其余节不动。
pub fn replace_settings_default(path: &Path) -> Result<(), String> {
    let value = default_settings_value();
    save_section(path, "settings", value)
}

/// settings 节默认值的 JSON 表示（前端恢复默认的后端权威来源）
pub fn default_settings_value() -> Value {
    serde_json::to_value(SettingsSection::default())
        .expect("SettingsSection 默认值序列化不可能失败")
}

/// 原子写：同目录临时文件 + rename（同卷 rename 原子，崩溃不留半截 JSON）
pub(crate) fn save_full(path: &Path, file: &AppConfigFile) -> std::io::Result<()> {
    write_atomic(path, file)
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
    use serde_json::json;

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
    fn save_then_load_roundtrip_custom_logs() {
        let dir = unique_dir("roundtrip");
        // 父目录不存在也应自动建
        let f = dir.join("nested").join(APP_CONFIG_FILE_NAME);
        save_logs(&f, &custom_logs()).unwrap();

        let loaded = load_full(&f).logs_config();
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
        let raw: Value =
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
        let cfg = load_full(&f).logs_config();
        assert!(!cfg.persist);
        assert_eq!(cfg.storage_path, "/var/log/x");
        assert_eq!(cfg.max_lines, HubConfig::default().max_lines);
    }

    #[test]
    fn missing_logs_section_returns_default_logs() {
        let dir = unique_dir("nologs");
        let f = dir.join(APP_CONFIG_FILE_NAME);
        std::fs::write(&f, r#"{"version":1}"#).unwrap();
        let cfg = load_full(&f).logs_config();
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
        let cfg = load_full(&f).logs_config();
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

        let raw: Value =
            serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        // 未知顶层节原样保留（前向兼容：旧版读改写不丢新版配置）
        assert_eq!(raw["terminal"]["fontSize"], 14);
        assert_eq!(raw["ui"]["theme"], "dark");
        // logs 已更新为新值；logs 节内未知字段也保留
        assert_eq!(raw["logs"]["storage_path"], "~/my-logs");
        assert_eq!(raw["logs"]["level"], "debug");
        assert_eq!(raw["logs"]["future_log_flag"], 42);
    }

    /* ---- 全节化 / save_section ---- */

    #[test]
    fn load_full_defaults_new_sections() {
        let dir = unique_dir("newsections");
        let f = dir.join(APP_CONFIG_FILE_NAME);
        std::fs::write(&f, r#"{"version":1}"#).unwrap();
        let file = load_full(&f);
        assert!(file.settings.is_object());
        assert!(file.keys.is_array());
        assert!(file.groups.is_array());
        assert!(file.ui_state.is_object());
        assert!(file.quick_connect_history.is_array());
    }

    #[test]
    fn save_section_rejects_unknown_name() {
        let dir = unique_dir("secname");
        let f = dir.join(APP_CONFIG_FILE_NAME);
        let err = save_section(&f, "unknown_section", json!({"a": 1})).unwrap_err();
        assert!(err.contains("未知配置节"));
        assert!(!f.exists(), "拒绝时不得写盘");
    }

    #[test]
    fn save_section_settings_validates_types_and_fills_defaults() {
        let dir = unique_dir("settings_ok");
        let f = dir.join(APP_CONFIG_FILE_NAME);
        // 只给两个字段；缺失字段补默认
        save_section(&f, "settings", json!({"uiTheme": "light", "fontSize": 15})).unwrap();
        let raw: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(raw["settings"]["uiTheme"], "light");
        assert_eq!(raw["settings"]["fontSize"], 15);
        assert_eq!(raw["settings"]["motd"], true); // 默认补齐
        assert_eq!(raw["settings"]["key.newTab"], "⌘+T");
        assert_eq!(raw["settings"]["updateChannel"], "stable");
    }

    #[test]
    fn save_section_settings_rejects_wrong_type_without_write() {
        let dir = unique_dir("settings_bad");
        let f = dir.join(APP_CONFIG_FILE_NAME);
        // 先写一份合法内容
        save_section(&f, "settings", json!({"uiTheme": "dark"})).unwrap();
        // 类型错误（fontSize 给字符串）必须拒绝
        let err = save_section(&f, "settings", json!({"fontSize": "big"})).unwrap_err();
        assert!(err.contains("settings 节格式错误"), "实际: {err}");
        // 磁盘仍是旧值
        let raw: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(raw["settings"]["uiTheme"], "dark");
    }

    #[test]
    fn save_section_keys_requires_id_name_type() {
        let dir = unique_dir("keys_req");
        let f = dir.join(APP_CONFIG_FILE_NAME);
        let ok = json!([
            {"id": "k1", "name": "n", "type": "ED25519", "hosts": ["h1"]}
        ]);
        save_section(&f, "keys", ok).unwrap();
        // 缺 name → 拒绝且零写入（删文件模拟零写入检查）
        let f2 = dir.join("app_config2.json");
        let bad = json!([{"id": "k2", "type": "RSA"}]);
        assert!(save_section(&f2, "keys", bad).is_err());
        assert!(!f2.exists());
    }

    #[test]
    fn save_section_groups_requires_name_and_color() {
        let dir = unique_dir("groups_req");
        let f = dir.join(APP_CONFIG_FILE_NAME);
        save_section(&f, "groups", json!([{"name": "g1", "color": "red"}])).unwrap();
        let bad = dir.join("bad.json");
        assert!(save_section(&bad, "groups", json!([{"name": "g2"}])).is_err());
        assert!(!bad.exists());
    }

    #[test]
    fn save_section_preserves_sibling_and_unknown_sections() {
        let dir = unique_dir("siblings");
        let f = dir.join(APP_CONFIG_FILE_NAME);
        save_section(&f, "groups", json!([{"name": "g", "color": "blue"}])).unwrap();
        save_section(&f, "settings", json!({"uiTheme": "dark"})).unwrap();
        save_section(&f, "keys", json!([])).unwrap();

        // 注入未知顶层节与节内未知字段，再写另一节，均不得丢
        let mut raw: Value =
            serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        raw["future_top"] = json!({"x": 1});
        raw["settings"]["futureField"] = json!(99);
        std::fs::write(&f, serde_json::to_string_pretty(&raw).unwrap()).unwrap();

        save_section(&f, "groups", json!([{"name": "g2", "color": "green"}])).unwrap();
        let after: Value =
            serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(after["future_top"]["x"], 1);
        assert_eq!(after["settings"]["futureField"], 99);
        assert_eq!(after["settings"]["uiTheme"], "dark");
        assert_eq!(after["groups"][0]["name"], "g2");
    }

    #[test]
    fn save_section_ui_state_and_history_roundtrip() {
        let dir = unique_dir("ui_hist");
        let f = dir.join(APP_CONFIG_FILE_NAME);
        save_section(
            &f,
            "ui_state",
            json!({"homeView": "hosts", "sessions": [
                {"sessionId": "s1", "hostId": "a", "alias": ""},
                {"sessionId": "s2", "hostId": "b", "alias": "prod"}
            ], "logWrap": false,
                   "futureUiFlag": "keep"}),
        )
        .unwrap();
        save_section(
            &f,
            "quick_connect_history",
            json!([{"host": "root@1.1.1.1", "timestamp": 123}]),
        )
        .unwrap();
        let raw: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(raw["ui_state"]["homeView"], "hosts");
        assert_eq!(raw["ui_state"]["futureUiFlag"], "keep"); // 未知子字段保留
        assert_eq!(raw["quick_connect_history"][0]["host"], "root@1.1.1.1");

        // history 元素缺 timestamp 拒绝
        let bad = dir.join("bad.json");
        assert!(save_section(&bad, "quick_connect_history", json!([{"host": "x"}])).is_err());
    }

    #[test]
    fn replace_settings_default_resets_only_settings() {
        let dir = unique_dir("reset_settings");
        let f = dir.join(APP_CONFIG_FILE_NAME);
        save_section(&f, "settings", json!({"uiTheme": "light", "accent": "#000000"})).unwrap();
        save_section(&f, "groups", json!([{"name": "g", "color": "red"}])).unwrap();

        replace_settings_default(&f).unwrap();

        let raw: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(raw["settings"]["uiTheme"], "dark");
        assert_eq!(raw["settings"]["accent"], "#3ddc84");
        // 其他节不动
        assert_eq!(raw["groups"][0]["name"], "g");
    }

    #[test]
    fn logs_section_sanitizes_zero_values_on_save() {
        let dir = unique_dir("logs_sanitize");
        let f = dir.join(APP_CONFIG_FILE_NAME);
        save_section(
            &f,
            "logs",
            json!({"level": "WEIRD", "maxLines": 0, "maxFiles": 0}),
        )
        .unwrap();
        let cfg = load_full(&f);
        let hub = cfg.logs_config();
        assert_eq!(hub.level, log::LevelFilter::Info);
        assert!(hub.max_lines >= 1);
        assert!(hub.max_files >= 1);
    }
}
