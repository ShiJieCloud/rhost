//! 应用配置导入导出的组装、校验与合并逻辑（IPC 之外的纯逻辑层）。
//!
//! 本模块承载设计文档 §4/§7 的核心规则：
//! - 导出：读盘组装 v1 Schema → 脱敏 → scope 过滤 → SHA256 → 写文件；
//! - 导入：大小/JSON/版本/哈希校验 → 节级校验 → 合并（id 重映射）→ 事务写盘。
//!
//! 所有纯函数均不依赖 `AppHandle`，路径由 IPC 层传入，便于单测。

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::applog::persisted::{self, AppConfigFile};
use crate::store::StoredHost;

/// 导入文件大小硬上限（5MB，前后端共用同一数值：前端仅预检验验，后端权威）。
pub const MAX_IMPORT_FILE_BYTES: u64 = 5 * 1024 * 1024;

/// 导出/备份组装结果的大小上限（5MB；超限提示拆分范围，不静默截断）。
pub const MAX_EXPORT_FILE_BYTES: u64 = 5 * 1024 * 1024;

/// 快速连接历史导入合并后的条数上限
pub const MAX_QUICK_HISTORY_ENTRIES: usize = 50;

/// 当前导出 Schema 版本
const SCHEMA_VERSION: u32 = 1;

/* =========================================================
 *  导出（§4 / §7.1 / §7.6）
 * ========================================================= */

/// 导出范围
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportScope {
    /// 全量：connections/settings/keys/groups（+ 可选 ui_state/history）
    Full,
    /// 仅主机：connections + groups
    Hosts,
    /// 仅界面布局：ui_state
    Ui,
}

impl ExportScope {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "full" => Ok(Self::Full),
            "hosts" => Ok(Self::Hosts),
            "ui" => Ok(Self::Ui),
            other => Err(format!("未知导出范围: {other}")),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Hosts => "hosts",
            Self::Ui => "ui",
        }
    }
}

/// 递归把 JSON 所有对象的键排序（BTreeMap），得到与解析顺序无关的规范形态。
/// 用于 SHA-256：导入端与导出端对同一 data 重序列化结果字节一致
/// （不依赖 serde_json 是否开启 preserve_order feature）。
pub fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<&String, &Value> = map.iter().collect();
            let obj: Map<String, Value> = sorted
                .into_iter()
                .map(|(k, v)| (k.clone(), canonicalize_json(v)))
                .collect();
            Value::Object(obj)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(canonicalize_json).collect()),
        _ => value.clone(),
    }
}

/// 计算 data 节规范序列化字节的 SHA-256（小写 hex）
pub fn data_sha256(data: &Value) -> Result<String, String> {
    let bytes = serde_json::to_vec(&canonicalize_json(data))
        .map_err(|e| format!("data 序列化失败: {e}"))?;
    let digest = Sha256::digest(bytes);
    Ok(hex_lower(&digest))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// 获取本机主机名（meta.hostname，仅用于识别导出来源；失败返回空串）
pub fn hostname() -> String {
    let mut buf = [0u8; 256];
    // gethostname 成功返回 0；buf 不一定 NUL 结尾（恰好填满时），按 NUL 位置截断
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return String::new();
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..len]).trim().to_string()
}

/// 组装后的导出文档
pub struct AssembledDoc {
    /// 完整明文 Schema JSON 文本（pretty）
    pub json: String,
    /// 实际写入 data 的节数（日志 sections 计数用）
    pub section_count: usize,
}

/// 脱敏：连接对象剔除 `keyPath`（原地改 Value）
fn strip_connection_sensitive(conn: &StoredHost) -> Result<Value, String> {
    let mut v = serde_json::to_value(conn).map_err(|e| format!("连接序列化失败: {e}"))?;
    if let Some(obj) = v.as_object_mut() {
        obj.remove("keyPath");
    }
    Ok(v)
}

/// 脱敏：密钥数组剔除每个元素的 `privatePath`
fn strip_keys_sensitive(keys: &Value) -> Value {
    let mut v = keys.clone();
    if let Some(arr) = v.as_array_mut() {
        for item in arr.iter_mut() {
            if let Some(obj) = item.as_object_mut() {
                obj.remove("privatePath");
            }
        }
    }
    v
}

/// 导出时剔除 ui_state.sessions：会话/标签是本地运行时状态，不进入配置导出。
fn strip_sessions(ui_state: &Value) -> Value {
    let mut v = ui_state.clone();
    if let Some(obj) = v.as_object_mut() {
        obj.remove("sessions");
    }
    v
}

/// 组装明文导出 Schema（读盘数据 → 脱敏 → scope 过滤 → 完整性哈希）。
///
/// - `settings` 节磁盘为空（首启未写过）时以内置默认值兜底合并，保证导出的是
///   「当前生效的完整设置」；
/// - 不读任何密码/私钥内容；`keyPath`/`privatePath` 在组装时剔除；
/// - 不含 logs 节（日志路径是设备本地配置，§5）。
#[allow(clippy::too_many_arguments)]
pub fn assemble_export(
    hosts: &[StoredHost],
    cfg: &AppConfigFile,
    scope: ExportScope,
    include_ui: bool,
    include_history: bool,
    app_version: &str,
    now_rfc3339: &str,
    host_name: &str,
) -> Result<AssembledDoc, String> {
    let mut data = Map::new();

    let connections: Vec<Value> = hosts
        .iter()
        .map(strip_connection_sensitive)
        .collect::<Result<_, _>>()?;

    match scope {
        ExportScope::Hosts => {
            data.insert("connections".into(), Value::Array(connections));
            data.insert("groups".into(), cfg.groups.clone());
        }
        ExportScope::Ui => {
            data.insert("ui_state".into(), strip_sessions(&cfg.ui_state));
        }
        ExportScope::Full => {
            data.insert("connections".into(), Value::Array(connections));
            // settings：磁盘节覆盖内置默认（缺字段补默认，未知字段保留）
            let settings = merge_settings_default(&cfg.settings);
            data.insert("settings".into(), settings);
            data.insert("keys".into(), strip_keys_sensitive(&cfg.keys));
            data.insert("groups".into(), cfg.groups.clone());
            if include_ui {
                data.insert("ui_state".into(), strip_sessions(&cfg.ui_state));
            }
            if include_history {
                data.insert(
                    "quick_connect_history".into(),
                    cfg.quick_connect_history.clone(),
                );
            }
        }
    }

    let data_value = Value::Object(data);
    let sha = data_sha256(&data_value)?;

    let mut meta = Map::new();
    meta.insert("scope".into(), Value::String(scope.as_str().into()));
    meta.insert("include_passwords".into(), Value::Bool(false));
    meta.insert("hostname".into(), Value::String(host_name.into()));
    meta.insert("data_sha256".into(), Value::String(sha));

    let section_count = data_value.as_object().map(|m| m.len()).unwrap_or(0);

    let mut doc = Map::new();
    doc.insert("version".into(), Value::Number(SCHEMA_VERSION.into()));
    doc.insert("exported_at".into(), Value::String(now_rfc3339.into()));
    doc.insert("app_version".into(), Value::String(app_version.into()));
    doc.insert("meta".into(), Value::Object(meta));
    doc.insert("data".into(), data_value);

    let json = serde_json::to_string_pretty(&Value::Object(doc))
        .map_err(|e| format!("导出文档序列化失败: {e}"))?;

    if json.len() as u64 > MAX_EXPORT_FILE_BYTES {
        return Err(format!(
            "配置数据过大（{} 字节），无法导出（上限 {} 字节），请缩小导出范围",
            json.len(),
            MAX_EXPORT_FILE_BYTES
        ));
    }

    Ok(AssembledDoc {
        json,
        section_count,
    })
}

/// settings 磁盘节与内置默认值合并（磁盘字段覆盖默认；未知字段保留）。
fn merge_settings_default(disk: &Value) -> Value {
    let defaults = persisted::default_settings_value();
    match (defaults, disk) {
        (Value::Object(mut base), Value::Object(over)) => {
            for (k, v) in over {
                base.insert(k.clone(), v.clone());
            }
            Value::Object(base)
        }
        (_, Value::Object(_)) => disk.clone(),
        _ => persisted::default_settings_value(),
    }
}

/// 原子写出导出文件（同目录临时文件 + rename），返回写入字节数。
pub fn write_export_file(path: &Path, json: &str) -> Result<usize, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建导出目录失败: {e}"))?;
    }
    let tmp = path.with_extension("json.tmp");
    {
        let mut f =
            std::fs::File::create(&tmp).map_err(|e| format!("创建导出临时文件失败: {e}"))?;
        f.write_all(json.as_bytes())
            .map_err(|e| format!("写入导出临时文件失败: {e}"))?;
        f.sync_all().ok();
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("写入导出文件失败: {e}"))?;
    Ok(json.len())
}

/* =========================================================
 *  导入（§6 / §7.2 / §9）
 * ========================================================= */

/// 导入结果摘要（返回前端展示；恒需重启，不表达重启标志）
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub connections_added: usize,
    pub connections_renamed: usize,
    pub keys_added: usize,
    pub keys_renamed: usize,
    pub groups_added: usize,
    pub settings_changed: usize,
}

/// 导入失败的结构化阶段信息（IPC 据此埋点 stage/section/reason）
#[derive(Debug)]
pub struct ImportError {
    /// size / parse / hash / decrypt / version / write
    pub stage: &'static str,
    pub section: Option<String>,
    /// decrypt 阶段的机器可读细分原因码（bad_password/corrupted），
    /// 仅写入本地 applog，不随错误返回前端
    pub reason: Option<&'static str>,
    /// 面向用户的错误文案（IPC 拒绝时原样返回前端）
    pub error: String,
    /// 日志专用文案：比 error 更直白的细分描述；None 时日志直接用 error
    pub log_error: Option<String>,
}

impl ImportError {
    fn parse(section: &str, e: impl Into<String>) -> Self {
        Self {
            stage: "parse",
            section: Some(section.to_string()),
            reason: None,
            error: e.into(),
            log_error: None,
        }
    }
    fn stage(stage: &'static str, e: impl Into<String>) -> Self {
        Self {
            stage,
            section: None,
            reason: None,
            error: e.into(),
            log_error: None,
        }
    }
    /// decrypt 阶段失败：用户文案与日志文案按密码错误/文件损坏分流
    fn decrypt(e: crate::config_crypto::DecryptError) -> Self {
        Self {
            stage: "decrypt",
            section: None,
            reason: Some(e.kind.code()),
            error: e.user_message().to_string(),
            log_error: Some(e.log_message()),
        }
    }
}

/// 非关键节损坏跳过记录
#[derive(Debug, Clone)]
pub struct SkippedSection {
    pub section: String,
    pub error: String,
}

/// 校验 + 合并完成、尚未写盘的导入计划（IPC 在写锁内调用并落盘）
#[derive(Debug)]
pub struct PreparedImport {
    /// connections 节存在时：合并后的完整主机列表
    pub hosts: Option<Vec<StoredHost>>,
    /// 已按规则合并好各节的 app_config（直接整体原子写；logs/未知节原样保留）
    pub app_config: AppConfigFile,
    pub summary: ImportSummary,
    pub skipped: Vec<SkippedSection>,
    pub file_version: u32,
    /// meta.scope（仅日志记录；合并逻辑严禁据此分支）
    pub meta_scope: Option<String>,
}

/// 解析、校验并合并导入文件（纯函数，不写盘）。
///
/// - 明文文件走标准 Schema 解析；`meta.encrypted=true` 的 envelope 先用
///   `password` 内存解密（§5.1），解密后内容等价于明文文件；
/// - `hosts_only=false`：`import_config`，消费 data 中存在的全部节；
/// - `hosts_only=true`：`import_hosts`，只消费 connections/groups，
///   本地 ui_state.sessions 保持不变（会话不进入导入导出）；
/// - `logs` 节恒忽略；节缺失 = 不动本地；关键节损坏整体 Err（零写入），
///   非关键节损坏收集进 `skipped`。
pub fn prepare_import(
    payload: &str,
    existing_hosts: &[StoredHost],
    existing_cfg: &AppConfigFile,
    hosts_only: bool,
    password: Option<&str>,
) -> Result<PreparedImport, ImportError> {
    // 1. 顶层 JSON（可能是标准 Schema，也可能是加密 envelope）
    let envelope: Value = serde_json::from_str(payload)
        .map_err(|e| ImportError::stage("parse", format!("配置文件格式错误: {e}")))?;

    // 2. 加密 envelope：解密后得到标准 Schema 文档；decrypt 失败按
    //    bad_password（GCM 认证失败）/ corrupted（结构非法）细分进日志，
    //    用户文案仍由 config_crypto 统一给出
    let encrypted = envelope
        .get("meta")
        .and_then(|m| m.get("encrypted"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let doc = if encrypted {
        let pw = password
            .ok_or_else(|| ImportError::stage("decrypt", "该配置文件已加密，请输入密码"))?;
        let plain =
            crate::config_crypto::open_envelope(payload, pw).map_err(ImportError::decrypt)?;
        serde_json::from_str::<Value>(&plain)
            .map_err(|e| ImportError::stage("parse", format!("解密后的配置格式错误: {e}")))?
    } else {
        envelope
    };

    // 3. 顶层结构 / version
    let top = doc
        .as_object()
        .ok_or_else(|| ImportError::stage("parse", "配置文件格式错误：顶层必须是 JSON 对象"))?;

    let file_version = top
        .get("version")
        .and_then(Value::as_u64)
        .ok_or_else(|| ImportError::stage("parse", "配置文件格式错误：缺少整数 version 字段"))?
        as u32;

    let mut data = top
        .get("data")
        .cloned()
        .ok_or_else(|| ImportError::stage("parse", "配置文件格式错误：缺少 data 节"))?;
    if !data.is_object() {
        return Err(ImportError::stage(
            "parse",
            "配置文件格式错误：data 必须是对象",
        ));
    }

    // 4. 版本校验（MVP 仅相等；migrate_to_current 负责三分支守门）
    data = crate::config_migrate::migrate_to_current(data, file_version)
        .map_err(|e| ImportError::stage("version", e))?;

    // 5. 完整性哈希（明文且文件带 data_sha256 时才校验；加密文件的 GCM tag 已保证完整性）
    if let Some(meta) = top.get("meta")
        && let Some(expected) = meta.get("data_sha256").and_then(Value::as_str)
    {
        let actual = data_sha256(&data)
            .map_err(|e| ImportError::stage("hash", format!("校验值计算失败: {e}")))?;
        if !expected.eq_ignore_ascii_case(&actual) {
            return Err(ImportError::stage("hash", "文件校验失败，可能被篡改或损坏"));
        }
    }

    let meta_scope = top
        .get("meta")
        .and_then(|m| m.get("scope"))
        .and_then(Value::as_str)
        .map(str::to_string);

    // 5. 逐节合并
    let mut out_cfg = existing_cfg.clone();
    let mut summary = ImportSummary::default();
    let mut skipped: Vec<SkippedSection> = Vec::new();

    // 5.1 连接（关键）
    let mut final_hosts: Option<Vec<StoredHost>> = None;
    let mut id_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut final_ids: Option<std::collections::HashSet<String>> = None;
    if let Some(conn_v) = data.get("connections") {
        let arr = conn_v
            .as_array()
            .ok_or_else(|| ImportError::parse("connections", "connections 节必须是数组"))?;
        let mut incoming = Vec::with_capacity(arr.len());
        for (i, item) in arr.iter().cloned().enumerate() {
            let h = serde_json::from_value::<StoredHost>(item).map_err(|e| {
                ImportError::parse("connections", format!("第 {i} 项连接格式错误: {e}"))
            })?;
            incoming.push(h);
        }
        let merged = crate::store::import_hosts_merge(existing_hosts, incoming);
        for (old, new) in merged.id_map {
            id_map.insert(old, new);
        }
        summary.connections_added = merged.added;
        summary.connections_renamed = merged.renamed;
        final_ids = Some(merged.hosts.iter().map(|h| h.id.clone()).collect());
        final_hosts = Some(merged.hosts);
    }

    // 5.2 分组（关键；import_config 与 import_hosts 均消费）
    if let Some(groups_v) = data.get("groups") {
        let merged_groups =
            merge_groups(&out_cfg.groups, groups_v).map_err(|e| ImportError::parse("groups", e))?;
        summary.groups_added = merged_groups.added;
        out_cfg.groups = merged_groups.value;
    }

    // ui_state.sessions 不进入配置导入导出：导入时忽略文件中的 sessions 字段，
    // 保留本地 sessions（会话为临时状态，不随配置迁移）。

    // hosts_only 时不消费文件的 keys/settings/ui_state/history；
    // 本地 ui_state.sessions 也无需重映射——主机合并只增不删，本地 id 全部保留，
    // 本地引用恒有效（§6.1 的 id 映射仅作用于导入文件内的引用）。
    if !hosts_only {
        // 5.3 密钥（关键）
        if let Some(keys_v) = data.get("keys") {
            let merged_keys = merge_keys(&out_cfg.keys, keys_v, &id_map, final_ids.as_ref())
                .map_err(|e| ImportError::parse("keys", e))?;
            summary.keys_added = merged_keys.added;
            summary.keys_renamed = merged_keys.renamed;
            out_cfg.keys = merged_keys.value;
        }

        // 5.4 设置（关键；键级合并 + 跨平台快捷键过滤）
        if let Some(settings_v) = data.get("settings") {
            let merged = merge_settings(&out_cfg.settings, settings_v)
                .map_err(|e| ImportError::parse("settings", e))?;
            summary.settings_changed = merged.renamed;
            out_cfg.settings = merged.value;
        }

        // 5.5 界面状态（非关键；全量覆盖，但 sessions 字段忽略——保留本地 sessions）
        if let Some(ui_v) = data.get("ui_state") {
            let mut candidate = ui_v.clone();
            // 剥离导入文件中的 sessions，不做 ID 重映射
            if let Some(obj) = candidate.as_object_mut() {
                obj.remove("sessions");
            }
            match serde_json::from_value::<persisted::UiStateSection>(candidate.clone()) {
                Ok(_) => {
                    // 保留本地 sessions：导入配置不含 sessions，合并回本地 sessions
                    if let Some(local_sessions) = out_cfg.ui_state.get("sessions").cloned()
                        && let Some(obj) = candidate.as_object_mut()
                    {
                        obj.insert("sessions".into(), local_sessions);
                    }
                    out_cfg.ui_state = candidate;
                }
                Err(e) => skipped.push(SkippedSection {
                    section: "ui_state".into(),
                    error: format!("ui_state 节格式错误: {e}"),
                }),
            }
        }

        // 5.6 快速连接历史（非关键；追加 + 倒序截断 50，不去重）
        if let Some(hist_v) = data.get("quick_connect_history") {
            match merge_history(&out_cfg.quick_connect_history, hist_v) {
                Ok(value) => out_cfg.quick_connect_history = value,
                Err(e) => skipped.push(SkippedSection {
                    section: "quick_connect_history".into(),
                    error: e,
                }),
            }
        }
    }

    Ok(PreparedImport {
        hosts: final_hosts,
        app_config: out_cfg,
        summary,
        skipped,
        file_version,
        meta_scope,
    })
}

/* ---- 分节合并辅助 ---- */

/// 分节合并结果（settings 复用 `renamed` 表达「发生变化的键数」）
struct SectionMerge {
    value: Value,
    added: usize,
    renamed: usize,
}

/// 分组：按名去重并集，导入定义优先；name/color 必填
fn merge_groups(existing: &Value, incoming: &Value) -> Result<SectionMerge, String> {
    let inc_arr = incoming
        .as_array()
        .ok_or_else(|| "groups 节必须是数组".to_string())?;

    let mut out: Vec<Value> = existing.as_array().cloned().unwrap_or_default();
    let mut names: std::collections::HashSet<String> = out
        .iter()
        .filter_map(|g| g.get("name").and_then(Value::as_str).map(str::to_string))
        .collect();
    let mut added = 0;

    for (i, item) in inc_arr.iter().enumerate() {
        let obj = item
            .as_object()
            .ok_or_else(|| format!("groups 节第 {i} 项必须是对象"))?;
        let name = obj
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("groups 节第 {i} 项缺少非空 name"))?;
        obj.get("color")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("groups 节第 {i} 项缺少 color 字符串"))?;
        if names.contains(name) {
            // 导入定义优先：替换同名分组（保留其在原列表中的位置）
            if let Some(slot) = out
                .iter_mut()
                .find(|g| g.get("name").and_then(Value::as_str) == Some(name))
            {
                *slot = item.clone();
            }
        } else {
            names.insert(name.to_string());
            out.push(item.clone());
            added += 1;
        }
    }
    Ok(SectionMerge {
        value: Value::Array(out),
        added,
        renamed: 0,
    })
}

/// 密钥：id 冲突改名（id 与 name 都追加「（导入）」后缀），hosts 引用按连接
/// id 映射重写并剔除悬空引用。元素必须通过 SshKeySection 强类型校验。
fn merge_keys(
    existing: &Value,
    incoming: &Value,
    host_id_map: &std::collections::HashMap<String, String>,
    final_host_ids: Option<&std::collections::HashSet<String>>,
) -> Result<SectionMerge, String> {
    let inc_arr = incoming
        .as_array()
        .ok_or_else(|| "keys 节必须是数组".to_string())?;
    let mut out = existing.as_array().cloned().unwrap_or_default();
    let mut used: std::collections::HashSet<String> = out
        .iter()
        .filter_map(|k| k.get("id").and_then(Value::as_str).map(str::to_string))
        .collect();

    let mut added = 0;
    let mut renamed = 0;

    for (i, raw_item) in inc_arr.iter().enumerate() {
        // 强类型校验：缺 id/name/type 等整体终止
        let _typed = serde_json::from_value::<persisted::SshKeySection>(raw_item.clone())
            .map_err(|e| format!("keys 节第 {i} 项格式错误: {e}"))?;
        let mut item = raw_item.clone();
        let obj = item.as_object_mut().expect("已校验为对象");

        // §5：私钥路径不跨设备可信，导入一律清空（即使文件手写携带也剔除）
        obj.remove("privatePath");

        // hosts 引用重映射 + 悬空剔除
        if let Some(hosts) = obj.get("hosts").and_then(Value::as_array).cloned() {
            let new_hosts: Vec<Value> = hosts
                .iter()
                .filter_map(|h| h.as_str().map(str::to_string))
                .filter_map(|id| {
                    let mapped = host_id_map.get(&id).cloned().unwrap_or(id);
                    match final_host_ids {
                        Some(ids) if !ids.contains(&mapped) => None,
                        _ => Some(Value::String(mapped)),
                    }
                })
                .collect();
            obj.insert("hosts".into(), Value::Array(new_hosts));
        }

        let old_id = obj
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !used.contains(&old_id) {
            used.insert(old_id);
            out.push(item);
            added += 1;
        } else {
            let base = format!("{old_id}（导入）");
            let mut new_id = base.clone();
            let mut seq = 2;
            while used.contains(&new_id) {
                new_id = format!("{base}{seq}");
                seq += 1;
            }
            obj.insert("id".into(), Value::String(new_id.clone()));
            if let Some(name) = obj.get("name").and_then(Value::as_str).map(str::to_string) {
                obj.insert("name".into(), Value::String(format!("{name}（导入）")));
            }
            used.insert(new_id);
            out.push(item);
            renamed += 1;
        }
    }

    Ok(SectionMerge {
        value: Value::Array(out),
        added,
        renamed,
    })
}

/// 设置键级合并：导入字段覆盖本地，缺失保留本地。
/// 非 macOS 平台静默忽略值含 ⌘ 的 key.* 快捷键（跨平台容错）。
/// 合并结果必须通过 SettingsSection 强类型校验（类型错误即关键节损坏）。
fn merge_settings(existing: &Value, incoming: &Value) -> Result<SectionMerge, String> {
    let inc = incoming
        .as_object()
        .ok_or_else(|| "settings 节必须是对象".to_string())?;
    let mut merged = existing.as_object().cloned().unwrap_or_default();

    let mut changed = 0;
    for (k, v) in inc {
        #[cfg(not(target_os = "macos"))]
        if k.starts_with("key.") && v.as_str().is_some_and(|s| s.contains('⌘')) {
            continue;
        }
        if merged.get(k) != Some(v) {
            changed += 1;
        }
        merged.insert(k.clone(), v.clone());
    }

    let value = Value::Object(merged);
    // 强类型校验（未知字段经 extra 保留）
    serde_json::from_value::<persisted::SettingsSection>(value.clone())
        .map_err(|e| format!("settings 节格式错误: {e}"))?;

    Ok(SectionMerge {
        value,
        added: 0,
        renamed: changed,
    })
}

/// 快速连接历史：existing ++ incoming，按 timestamp 倒序（稳定），截断 50，不去重。
fn merge_history(existing: &Value, incoming: &Value) -> Result<Value, String> {
    let inc_arr = incoming
        .as_array()
        .ok_or_else(|| "quick_connect_history 节必须是数组".to_string())?;
    // 整节强校验：任一元素缺 host/timestamp 即跳过整节（调用方记 skipped）
    for (i, item) in inc_arr.iter().enumerate() {
        serde_json::from_value::<persisted::QuickConnectItem>(item.clone())
            .map_err(|e| format!("quick_connect_history 节第 {i} 项格式错误: {e}"))?;
    }
    let mut all = existing.as_array().cloned().unwrap_or_default();
    all.extend(inc_arr.iter().cloned());
    // sort_by 稳定排序：timestamp 降序；缺失/非法 timestamp 沉底（已强校验不会发生）
    all.sort_by(|a, b| {
        let ta = a.get("timestamp").and_then(Value::as_i64).unwrap_or(0);
        let tb = b.get("timestamp").and_then(Value::as_i64).unwrap_or(0);
        tb.cmp(&ta)
    });
    all.truncate(MAX_QUICK_HISTORY_ENTRIES);
    Ok(Value::Array(all))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::StoredHost;

    fn sample_host(id: &str) -> StoredHost {
        StoredHost {
            id: id.into(),
            conn_type: "ssh".into(),
            user: "root".into(),
            ip: "10.0.0.1".into(),
            port: 22,
            os: "Linux".into(),
            color: "green".into(),
            label: "R".into(),
            tag: "t".into(),
            group: "g".into(),
            key_path: Some("/Users/me/.ssh/id_ed25519".into()),
            extra: Value::Null,
            updated_at: 123,
        }
    }

    fn cfg_with_keys() -> AppConfigFile {
        let mut cfg = AppConfigFile::default();
        cfg.keys = serde_json::json!([
            {"id": "k1", "name": "key", "type": "ED25519", "privatePath": "/secret/id",
             "hosts": ["h1"], "publicKey": "ssh-ed25519 AAAA"}
        ]);
        cfg.groups = serde_json::json!([{"name": "g", "color": "red"}]);
        cfg.ui_state = serde_json::json!({"homeView": "hosts", "sessions": [
            {"sessionId": "s1", "hostId": "h1", "alias": ""}
        ]});
        cfg
    }

    #[test]
    fn scope_hosts_contains_only_connections_and_groups() {
        let doc = assemble_export(
            &[sample_host("h1")],
            &cfg_with_keys(),
            ExportScope::Hosts,
            false,
            false,
            "1.0.0",
            "2026-10-06T22:30:00+08:00",
            "testhost",
        )
        .unwrap();
        let v: Value = serde_json::from_str(&doc.json).unwrap();
        let data = v["data"].as_object().unwrap();
        assert_eq!(data.len(), 2);
        assert!(data.contains_key("connections"));
        assert!(data.contains_key("groups"));
        assert!(!data.contains_key("settings"));
        assert!(!data.contains_key("keys"));
        assert_eq!(doc.section_count, 2);
        // keyPath 已剔除
        assert!(v["data"]["connections"][0].get("keyPath").is_none());
        assert_eq!(v["meta"]["scope"], "hosts");
        assert_eq!(v["meta"]["include_passwords"], false);
        assert!(v["meta"]["data_sha256"].is_string());
    }

    #[test]
    fn scope_ui_contains_only_ui_state() {
        let doc = assemble_export(
            &[sample_host("h1")],
            &cfg_with_keys(),
            ExportScope::Ui,
            true,
            true,
            "1.0.0",
            "ts",
            "h",
        )
        .unwrap();
        let v: Value = serde_json::from_str(&doc.json).unwrap();
        let data = v["data"].as_object().unwrap();
        assert_eq!(data.len(), 1);
        assert!(data.contains_key("ui_state"));
        // sessions 不进入导出
        assert!(data["ui_state"].get("sessions").is_none());
        assert_eq!(data["ui_state"]["homeView"], "hosts");
    }

    #[test]
    fn scope_full_optional_sections_respect_flags_and_strips_private_path() {
        let doc = assemble_export(
            &[sample_host("h1")],
            &cfg_with_keys(),
            ExportScope::Full,
            false,
            false,
            "1.0.0",
            "ts",
            "h",
        )
        .unwrap();
        let v: Value = serde_json::from_str(&doc.json).unwrap();
        let data = v["data"].as_object().unwrap();
        assert!(data.contains_key("connections"));
        assert!(data.contains_key("settings"));
        assert!(data.contains_key("keys"));
        assert!(data.contains_key("groups"));
        assert!(!data.contains_key("ui_state"));
        assert!(!data.contains_key("quick_connect_history"));
        // privatePath 剔除，其余密钥字段保留
        let k = &data["keys"][0];
        assert!(k.get("privatePath").is_none());
        assert_eq!(k["publicKey"], "ssh-ed25519 AAAA");
        assert_eq!(k["hosts"][0], "h1");
        // 空 settings 磁盘节 → 默认值兜底
        assert_eq!(data["settings"]["uiTheme"], "dark");
        assert_eq!(data["settings"]["sftpChunkKb"], 64);

        // 勾选两个可选节后节数增加
        let doc2 = assemble_export(
            &[sample_host("h1")],
            &cfg_with_keys(),
            ExportScope::Full,
            true,
            true,
            "1.0.0",
            "ts",
            "h",
        )
        .unwrap();
        let v2: Value = serde_json::from_str(&doc2.json).unwrap();
        assert_eq!(v2["data"].as_object().unwrap().len(), 6);
    }

    #[test]
    fn sha256_is_deterministic_and_tamper_sensitive() {
        let d1 = serde_json::json!({"b": 1, "a": [1, 2], "c": {"z": 1, "y": 2}});
        let d2 = serde_json::json!({"c": {"y": 2, "z": 1}, "a": [1, 2], "b": 1});
        assert_eq!(data_sha256(&d1).unwrap(), data_sha256(&d2).unwrap());
        let d3 = serde_json::json!({"b": 2, "a": [1, 2], "c": {"z": 1, "y": 2}});
        assert_ne!(data_sha256(&d1).unwrap(), data_sha256(&d3).unwrap());
        // 64 位小写 hex
        assert_eq!(data_sha256(&d1).unwrap().len(), 64);
    }

    #[test]
    fn settings_disk_overrides_default_and_keeps_unknown() {
        let mut cfg = AppConfigFile::default();
        cfg.settings = serde_json::json!({"uiTheme": "light", "futureField": 7});
        let doc = assemble_export(
            &[],
            &cfg,
            ExportScope::Full,
            false,
            false,
            "1.0.0",
            "ts",
            "h",
        )
        .unwrap();
        let v: Value = serde_json::from_str(&doc.json).unwrap();
        assert_eq!(v["data"]["settings"]["uiTheme"], "light"); // 磁盘覆盖
        assert_eq!(v["data"]["settings"]["motd"], true); // 默认补齐
        assert_eq!(v["data"]["settings"]["futureField"], 7); // 未知保留
    }

    #[test]
    fn write_export_file_is_atomic_and_roundtrips() {
        let dir = std::env::temp_dir().join(format!(
            "rhost-export-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.rhost.json");
        let n = write_export_file(&path, "{\"ok\":true}").unwrap();
        assert_eq!(n, 11);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"ok\":true}");
        assert!(!path.with_extension("json.tmp").exists());
    }

    /* ---- 导入 ---- */

    /// 用给定 data 拼 v1 明文文档并附带正确 data_sha256
    fn import_doc(data: Value) -> String {
        let hash = data_sha256(&data).unwrap();
        serde_json::to_string(&serde_json::json!({
            "version": 1,
            "meta": {"scope": "full", "data_sha256": hash},
            "data": data,
        }))
        .unwrap()
    }

    fn host_value(id: &str, ip: &str) -> Value {
        serde_json::json!({
            "id": id, "connType": "ssh", "user": "root", "ip": ip,
            "port": 22, "os": "Linux", "color": "green", "label": "R",
            "tag": "t", "group": "g", "updatedAt": 1
        })
    }

    #[test]
    fn import_full_roundtrip_merges_every_section_and_remaps_refs() {
        let mut existing = cfg_with_keys();
        existing.ui_state = serde_json::json!({"homeView": "hosts", "sessions": ["h1", "gone"]});
        existing.settings = serde_json::json!({"uiTheme": "dark"});

        let data = serde_json::json!({
            "connections": [host_value("h2", "10.0.0.9")],
            "groups": [
                {"name": "g", "color": "blue"},
                {"name": "new", "color": "x"}
            ],
            "keys": [
                {"id": "k1", "name": "key", "type": "ED25519",
                 "hosts": ["h2", "ghost"], "publicKey": "P"}
            ],
            "settings": {"uiTheme": "light", "customFuture": 7},
            "ui_state": {"homeView": "sftp", "sessions": ["h1", "h2", "ghost"]},
            "quick_connect_history": [{"host": "10.0.0.9", "timestamp": 200}],
            "logs": {"level": "trace"}
        });
        let p = prepare_import(
            &import_doc(data),
            &[sample_host("h1")],
            &existing,
            false,
            None,
        )
        .unwrap();

        // 连接
        let hosts = p.hosts.as_ref().unwrap();
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[1].id, "h2");
        assert_eq!(p.summary.connections_added, 1);
        assert_eq!(p.summary.connections_renamed, 0);

        // 分组：g 被导入定义覆盖（不计 added），new 新增
        assert_eq!(p.summary.groups_added, 1);
        let groups = p.app_config.groups.as_array().unwrap();
        assert_eq!(groups[0]["color"], "blue");
        assert_eq!(groups[1]["name"], "new");

        // 密钥：k1 与本地冲突 → id/name 改名；hosts 悬空引用 ghost 剔除，h2 保留
        assert_eq!(p.summary.keys_added, 0);
        assert_eq!(p.summary.keys_renamed, 1);
        let key = &p.app_config.keys.as_array().unwrap()[1];
        assert_eq!(key["id"], "k1（导入）");
        assert_eq!(key["name"], "key（导入）");
        assert_eq!(key["hosts"], serde_json::json!(["h2"]));
        // 本地密钥原样保留
        assert_eq!(p.app_config.keys.as_array().unwrap()[0]["id"], "k1");

        // 设置：两个键与本地不同（uiTheme dark→light + 未知新键）
        assert_eq!(p.summary.settings_changed, 2);
        assert_eq!(p.app_config.settings["uiTheme"], "light");
        assert_eq!(p.app_config.settings["customFuture"], 7);

        // ui_state：全量覆盖（homeView），但 sessions 忽略导入值、保留本地 sessions
        assert_eq!(p.app_config.ui_state["homeView"], "sftp");
        assert_eq!(
            p.app_config.ui_state["sessions"],
            serde_json::json!(["h1", "gone"])
        );

        // history 追加
        let hist = p.app_config.quick_connect_history.as_array().unwrap();
        assert_eq!(hist.len(), 1);
        assert_eq!(hist[0]["host"], "10.0.0.9");
        // data.logs 存在不影响导入（logs 节恒忽略，准备成功即证明未被当作关键节）
    }

    #[test]
    fn import_rejects_tampered_hash_bad_version_and_encrypted() {
        // 哈希不匹配
        let bad = r#"{"version":1,"meta":{"data_sha256":"deadbeef"},"data":{}}"#;
        let err = prepare_import(bad, &[], &AppConfigFile::default(), false, None).unwrap_err();
        assert_eq!(err.stage, "hash");

        // 版本过高
        let mut d = serde_json::json!({"version": 99, "data": {}});
        d["data"] = serde_json::json!({});
        let doc = serde_json::to_string(&d).unwrap();
        let err = prepare_import(&doc, &[], &AppConfigFile::default(), false, None).unwrap_err();
        assert_eq!(err.stage, "version");

        // 加密文件
        let enc = r#"{"version":1,"meta":{"encrypted":true},"data":{}}"#;
        let err = prepare_import(enc, &[], &AppConfigFile::default(), false, None).unwrap_err();
        assert_eq!(err.stage, "decrypt");

        // 顶层非对象 / 缺 version
        let err = prepare_import("[]", &[], &AppConfigFile::default(), false, None).unwrap_err();
        assert_eq!(err.stage, "parse");
        let err = prepare_import(
            r#"{"data":{}}"#,
            &[],
            &AppConfigFile::default(),
            false,
            None,
        )
        .unwrap_err();
        assert_eq!(err.stage, "parse");
    }

    #[test]
    fn import_critical_section_corrupt_aborts_entire_import() {
        // settings 值类型错误（uiTheme 应为字符串）→ 关键节损坏，零写入
        let data = serde_json::json!({
            "connections": [host_value("h9", "1.1.1.1")],
            "settings": {"uiTheme": 123}
        });
        let err = prepare_import(
            &import_doc(data),
            &[],
            &AppConfigFile::default(),
            false,
            None,
        )
        .unwrap_err();
        assert_eq!(err.stage, "parse");
        assert_eq!(err.section.as_deref(), Some("settings"));

        // 连接元素缺必填 port
        let data = serde_json::json!({"connections": [{"id": "x", "connType": "ssh"}]});
        let err = prepare_import(
            &import_doc(data),
            &[],
            &AppConfigFile::default(),
            false,
            None,
        )
        .unwrap_err();
        assert_eq!(err.section.as_deref(), Some("connections"));

        // 分组缺 color
        let data = serde_json::json!({"groups": [{"name": "g"}]});
        let err = prepare_import(
            &import_doc(data),
            &[],
            &AppConfigFile::default(),
            false,
            None,
        )
        .unwrap_err();
        assert_eq!(err.section.as_deref(), Some("groups"));
    }

    #[test]
    fn import_non_critical_corrupt_section_is_skipped_others_apply() {
        // ui_state 类型错（sessions 应为字符串数组）—— UiStateSection 校验较宽松，
        // 这里用 history 元素缺 timestamp 制造确定性跳过
        let data = serde_json::json!({
            "settings": {"uiTheme": "light"},
            "quick_connect_history": [{"host": "h"}]
        });
        let p = prepare_import(
            &import_doc(data),
            &[],
            &AppConfigFile::default(),
            false,
            None,
        )
        .unwrap();
        assert_eq!(p.skipped.len(), 1);
        assert_eq!(p.skipped[0].section, "quick_connect_history");
        // 关键节仍然生效
        assert_eq!(p.app_config.settings["uiTheme"], "light");
    }

    #[test]
    fn import_hosts_only_consumes_connections_groups_and_preserves_local_sessions() {
        let mut existing = cfg_with_keys();
        existing.ui_state = serde_json::json!({"homeView": "hosts", "sessions": ["h1", "ghost"]});
        existing.settings = serde_json::json!({"uiTheme": "dark"});

        let data = serde_json::json!({
            "connections": [
                host_value("h1", "2.2.2.2"),  // 与本地 h1 冲突 → 改名
                host_value("h3", "3.3.3.3"),
            ],
            "groups": [{"name": "g2", "color": "yellow"}],
            "settings": {"uiTheme": "light"},
            "keys": [{"id": "k9", "name": "n", "type": "RSA"}],
            "ui_state": {"homeView": "sftp"},
        });
        let p = prepare_import(
            &import_doc(data),
            &[sample_host("h1")],
            &existing,
            true,
            None,
        )
        .unwrap();

        let hosts = p.hosts.as_ref().unwrap();
        assert_eq!(hosts.len(), 3);
        assert_eq!(hosts[1].id, "h1（导入）");
        assert_eq!(hosts[2].id, "h3");
        assert_eq!(p.summary.connections_added, 1);
        assert_eq!(p.summary.connections_renamed, 1);
        assert_eq!(p.summary.groups_added, 1);

        // 文件 settings/keys/ui_state 一律不消费；本地 ui_state 完全不动
        assert_eq!(p.app_config.settings["uiTheme"], "dark");
        assert_eq!(p.app_config.keys.as_array().unwrap().len(), 1);
        assert_eq!(
            p.app_config.ui_state["sessions"],
            serde_json::json!(["h1", "ghost"])
        );
        assert_eq!(p.app_config.ui_state["homeView"], "hosts");
    }

    #[test]
    fn import_history_is_appended_sorted_desc_and_capped_at_50() {
        let mut existing = AppConfigFile::default();
        existing.quick_connect_history = serde_json::json!([{"host": "old", "timestamp": 100}]);
        let mut incoming: Vec<Value> = (0..60)
            .map(|i| serde_json::json!({"host": format!("n{i}"), "timestamp": 1000 - i}))
            .collect();
        // 追加一个更早的，验证排序后沉底被截断
        incoming.push(serde_json::json!({"host": "early", "timestamp": 1}));
        let data = serde_json::json!({"quick_connect_history": incoming});
        let p = prepare_import(&import_doc(data), &[], &existing, false, None).unwrap();
        let hist = p.app_config.quick_connect_history.as_array().unwrap();
        assert_eq!(hist.len(), 50);
        assert_eq!(hist[0]["timestamp"], 1000);
        assert_eq!(hist[49]["host"], "n49"); // n0(1000)..n49(951) 保留，其余截断
    }

    #[test]
    fn import_encrypted_envelope_roundtrips_and_requires_password() {
        let data = serde_json::json!({
            "connections": [host_value("h2", "10.0.0.9")],
            "settings": {"uiTheme": "light"},
        });
        let plain = import_doc(data);
        let envelope = crate::config_crypto::seal_envelope(&plain, "p@ss 词").unwrap();

        // 无密码 → decrypt（请输入密码；无细分原因码）
        let err =
            prepare_import(&envelope, &[], &AppConfigFile::default(), false, None).unwrap_err();
        assert_eq!(err.stage, "decrypt");
        assert_eq!(err.reason, None);
        assert_eq!(err.log_error, None);

        // 错密码 → decrypt / bad_password（GCM tag 校验失败，零写入）；
        // 用户文案保持统一，日志文案直白归类
        let err = prepare_import(
            &envelope,
            &[],
            &AppConfigFile::default(),
            false,
            Some("nope"),
        )
        .unwrap_err();
        assert_eq!(err.stage, "decrypt");
        assert_eq!(err.reason, Some("bad_password"));
        assert_eq!(err.error, "密码错误或文件损坏");
        assert!(
            err.log_error
                .as_deref()
                .is_some_and(|m| m.contains("密码错误"))
        );

        // 结构被破坏（salt 长度非法：8 字节零的 base64）→ decrypt / corrupted
        let mut broken: Value = serde_json::from_str(&envelope).unwrap();
        broken["meta"]["salt"] = serde_json::json!("AAAAAAAAAAA=");
        let err = prepare_import(
            &broken.to_string(),
            &[],
            &AppConfigFile::default(),
            false,
            Some("p@ss 词"),
        )
        .unwrap_err();
        assert_eq!(err.stage, "decrypt");
        assert_eq!(err.reason, Some("corrupted"));
        assert!(
            err.log_error
                .as_deref()
                .is_some_and(|m| m.contains("文件损坏"))
        );

        // 正确密码 → 标准合并流程
        let p = prepare_import(
            &envelope,
            &[],
            &AppConfigFile::default(),
            false,
            Some("p@ss 词"),
        )
        .unwrap();
        assert_eq!(p.summary.connections_added, 1);
        assert_eq!(p.app_config.settings["uiTheme"], "light");
    }
}
