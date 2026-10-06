//! 配置导出文件的 Schema 版本迁移框架。
//!
//! 导入文件（`.rhost.json`）顶层带整数 `version` 标识**结构版本**（非应用版本）。
//! 当配置结构发生不兼容演进（字段改名、层级调整、类型转换）时，在 [`MIGRATIONS`]
//! 末尾追加一个纯函数迁移步骤即可，当前版本号由注册表长度自动推导。
//!
//! MVP 阶段没有任何结构变更：迁移表为空，本模块实际只承担导入时的版本守门——
//! 高版本文件拒绝（防止旧应用误解析新结构），低版本/非法版本拒绝。
//!
//! 约定（首次启用迁移链前补全）：
//! - 每步迁移是纯函数 `Value → Value`：不依赖 `AppHandle`、不读写磁盘，便于单测；
//! - 字段补齐优先靠 `#[serde(default)]`，迁移只处理默认值无法表达的结构性变更；
//! - 迁移链只升不降：`file_version > CURRENT` 永不尝试降级解析。

use serde_json::Value;

/// 单步迁移：输入旧版本 data JSON，输出新版本 data JSON。
/// 纯函数，只负责结构变换（字段补齐、改名、类型转换），不读写磁盘。
type MigrationFn = fn(Value) -> Result<Value, String>;

/// 迁移注册表：索引 i 存放 v(i+1) → v(i+2) 的迁移函数。
/// MVP 为空表；发布 v2 时在末尾追加 migrate_v1_to_v2，CURRENT_SCHEMA_VERSION 自动 +1。
static MIGRATIONS: &[MigrationFn] = &[];

/// 版本号自动推导：1 + 迁移步数。新增迁移只需在 MIGRATIONS 末尾追加函数，
/// 不会出现「加了迁移忘了改常量」的不一致。
pub const CURRENT_SCHEMA_VERSION: u32 = 1 + MIGRATIONS.len() as u32;

/// 将导入文件的 data 节迁移到当前结构版本。
///
/// - `file_version == CURRENT`：原样返回（MVP 唯一可达的成功路径）；
/// - `file_version > CURRENT`：拒绝，提示升级应用（不做降级解析）；
/// - `file_version < 1`：拒绝（格式错误）；
/// - `file_version` 为其他低值：依次执行迁移链（MVP 不可达）。
pub fn migrate_to_current(data: Value, file_version: u32) -> Result<Value, String> {
    if file_version < 1 {
        return Err("配置文件格式错误".to_string());
    }
    if file_version > CURRENT_SCHEMA_VERSION {
        return Err("配置文件来自更高版本客户端，请升级应用".to_string());
    }
    let mut data = data;
    for v in file_version..CURRENT_SCHEMA_VERSION {
        data = MIGRATIONS[(v - 1) as usize](data)?;
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn current_version_is_one_in_mvp() {
        assert_eq!(CURRENT_SCHEMA_VERSION, 1);
    }

    #[test]
    fn equal_version_passes_through() {
        let data = json!({"connections": []});
        let out = migrate_to_current(data.clone(), 1).unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn higher_version_is_rejected() {
        let err = migrate_to_current(json!({}), 2).unwrap_err();
        assert!(err.contains("更高版本"), "实际错误: {err}");
    }

    #[test]
    fn zero_version_is_rejected() {
        assert!(migrate_to_current(json!({}), 0).is_err());
    }
}
