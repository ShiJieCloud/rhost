# 应用配置导入导出设计方案

> status: 设计定稿，待落地
>
> 日期：2026-10-06
>
> scope: Rhost 全量应用配置（主机连接、应用设置、SSH 密钥元数据、分组、UI 状态等）的导出为文件与从文件恢复
>
> 关联代码：`src-tauri/src/store.rs`、`src-tauri/src/applog/persisted.rs`、`src-tauri/src/ipc.rs`、`frontend/src/stores/settings.ts`、`frontend/src/stores/hosts.ts`、`frontend/src/stores/keys.ts`、`frontend/src/stores/groups.ts`、`frontend/src/stores/session.ts`、`frontend/src/components/SettingsModal.vue`
>
> 边界：**本文档只覆盖应用配置**（可复现运行环境所需的数据与设置）。终端会话录制文件、日志文件、SFTP 传输历史等体积大或时效性强的数据不纳入导出范围。**主机密钥信任库 `known_hosts.json` 也不纳入**（本机 TOFU 运行态数据，换机后应重新首连确认，见 §5 与 `hostkey-verification-design.md` §4）。

## 1. 设计目标与约束

| 目标 | 量化口径 |
|---|---|
| 一键迁移 | 用户换机或重装后，通过一个文件即可恢复完整运行环境（连接列表、设置、密钥元数据、分组、布局偏好） |
| 操作极简 | 导出 = 选范围（默认全量，下拉切换）→ 选保存位置 → 完成；导入 = 选文件 → 完成。无预览弹窗、无 merge/overwrite 选择，导入自动识别导出范围 |
| 安全 | 密码与私钥内容绝不导出；钥匙串数据无法跨设备/平台批量迁移 |
| 可回滚 | 导入/重置不做自动备份；由「导出到文件」（全量配置、密码加密）提供**用户主动**导出兜底 |
| 版本兼容 | 配置结构带版本号，新旧版本之间可安全降级与升级读取 |

必须遵守的既有项目约束：

- **密码只进系统钥匙串**（`store.rs`），不写入任何磁盘文件；导出时钥匙串数据不可访问；
- **原子写**：`connections.json` 与 `app_config.json` 已采用「临时文件 + rename」策略，导入导出沿用；
- `app_config.json` 使用 `#[serde(flatten)] extra` 保留未知节，导入写回时不得丢失；
- **单一事实来源**：所有配置以**后端文件**为唯一持久化层；localStorage 仅作运行时缓存（启动时从后端加载），不承担持久化职责——导入导出因此不存在「前后端双写不一致」窗口（见 §3）；
- **写入串行化**：`app_config.json` 的所有写入（写穿/导入/恢复默认设置）经**全局异步互斥锁**串行执行；原子 rename 只保证崩溃安全，不保证多写者逻辑顺序（见 §7.5）；
- 后端冷启动依赖的日志配置（`logStoragePath` 等）存于 `app_config.json` `logs` 节，导入后部分设置需重启生效。

---

## 2. 现状盘点

| 位置 | 现状 | 差距 |
|---|---|---|
| `SettingsModal.vue` →「配置文件」区域 | 已有「导出」「导入」两个按钮占位；导出当前实现为剪贴板 JSON（纯前端） | 缺少文件选择、后端组装 |
| `store.rs` | `load_hosts` / `upsert_host` / `delete_host` / `write_hosts`（原子写） | `write_hosts` 为私有函数，需提升可见性供 IPC 批量导入调用 |
| `applog/persisted.rs` | `app_config.json` 仅有 `logs` 一节；`save` 当前标记 `#[allow(dead_code)]` | 需扩展 `settings` / `keys` / `groups` / `ui_state` / `quick_connect_history` 节；`save` 正式启用 |
| 各 `stores/*.ts` | **localStorage 是唯一持久化层**：`rhost.settings`、`rhost.sshKeys`、`rhost.groups`、`rhost.layout`、`rhost.sessions`、`rhost.homeView`、`rhost.quickConnect.history`、`rhost.logWrap` | 需整体迁移到后端文件持久化（启动加载 + 写穿），localStorage 降级为运行时缓存 |
| `ipc.rs` | 已有 `load_connections`、`save_connection`、`delete_connection`、`set_log_config` 等 | 需新增配置统一读写（`load_app_config` / `set_app_config_section`）与 `export_config` / `import_config` |

**双轨风险（本次重构的动机）**：若导入仅由后端写磁盘文件、前端再自行写 localStorage，两者之间存在崩溃窗口——后端已写盘而前端写 localStorage 失败/刷新/报错时，磁盘与内存状态割裂，主机列表、密钥、分组出现混合旧新数据的诡异 bug。因此本设计采用**方案 A：统一后端持久化**，localStorage 不再是数据源，导入后前端只需从后端重新加载，不存在双写窗口。

结论：原子写基础设施已就绪，缺的是**统一后端配置存储层（app_config.json 全节化）+ 统一 Schema + 导入导出 IPC**。本设计补齐这三块；因当前无老用户，localStorage 旧 key 直接弃用，不做存量迁移（§3.1）。

---

## 3. 数据范围与存储分层

**单一事实来源：后端文件。** localStorage 不再承担持久化职责，仅作运行时缓存——前端启动时从后端全量加载到内存 store，之后的变更通过 IPC **写穿**（write-through）到后端文件。导入导出因此不存在「前后端双写不一致」窗口。

```
┌─ 后端文件层（唯一持久化）────────────────────────────┐
│  app_data_dir/connections.json                      │  主机连接列表（密码已剔除）
│  app_config_dir/app_config.json                     │  统一配置文件，按节组织：
│    ├─ logs                                          │    日志配置（冷启动需要，现有 HubConfig）
│    ├─ settings                                      │    应用设置（外观/终端/快捷键/SFTP 等，log* 字段除外）
│    ├─ keys                                          │    SSH 密钥元数据（含 privatePath；私钥内容仅存 ~/.ssh，不入库；导出时剔除 privatePath）
│    ├─ groups                                        │    分组定义
│    ├─ ui_state                                      │    homeView / layout / sessions / logWrap / configExportScope
│    └─ quick_connect_history                         │    快速连接历史
└─────────────────────────────────────────────────────┘
         ▲ 启动时 load_app_config 全量加载
         │ 运行时 set_app_config_section 写穿（原子写）
┌─ 前端内存层（响应式 store，非持久化）─────────────────┐
│  settings / hosts / keys / groups / session ...      │
└─────────────────────────────────────────────────────┘
```

**为什么 `settings` 节不含 `log*` 字段**：`logStoragePath` 等日志配置要求后端冷启动瞬间可读（前端通道就绪前），由现有 `logs` 节（HubConfig）承担权威存储。前端 `AppSettings.logXxx` 加载时从 `logs` 节映射填充，保存时写回 `logs` 节——同一字段只存一处，避免双源。

### 3.1 无存量迁移（当前无老用户）

产品尚未发布，localStorage 仅存在于开发环境——**不做任何 localStorage → 后端的迁移机制**（无 `migrate_local_storage` IPC、无迁移按钮、无 `migrated` 标记）。重构落地后：各 store 改为后端写穿，localStorage 旧 key 直接弃用（开发机上的旧数据可忽略）；后端文件缺失时走 `#[serde(default)]` 默认回退，即全新初始状态。

### 3.2 明确排除

- **密码 / 私钥口令**：仅存系统钥匙串，不导出。导入后连接首次使用需重新输入并重新存入钥匙串。
- **私钥文件内容与路径**：不读取 `~/.ssh` 下的私钥文件；**导出时剔除密钥 `privatePath` 与连接 `keyPath` 两处路径引用**（路径泄漏本机目录结构，跨设备也无意义）。导入后路径字段为空，用户需重新选择私钥文件完成绑定。
- **日志配置**：`logs` 节（含 `logStoragePath`）不参与导入导出——设备本地配置，跨设备无意义（§5）。
- **运行时状态**：连接状态、`status` / `lat` / `cpu` / `mem` / `uptime`、SFTP 传输队列、日志文件内容。
- **大体积数据**：终端会话录制、历史日志文件。

---

## 4. 导出文件 Schema

单文件 JSON，建议扩展名 `.rhost.json`（或 `.json`）。

```json
{
  "version": 1,
  "exported_at": "2026-10-06T22:30:00+08:00",
  "app_version": "1.2.0",
  "meta": {
    "scope": "full",
    "include_passwords": false,
    "hostname": "MacBook-Pro",
    "data_sha256": "a1b2c3d4..."
  },
  "data": {
    "connections": [
      {
        "id": "prod-web-01",
        "connType": "ssh",
        "user": "admin",
        "ip": "192.168.1.10",
        "port": 22,
        "os": "Ubuntu 22.04",
        "color": "green",
        "label": "PW",
        "tag": "生产",
        "group": "生产环境",
        "extra": {},
        "updatedAt": 1728222600000
      }
    ],
    "settings": {
      "uiTheme": "dark",
      "accent": "#3ddc84",
      "opacity": 96,
      "splashDurationMs": 400,
      "splashTransparent": true,
      "fontFamily": "JetBrains Mono",
      "fontSize": 13,
      "lineHeight": 145,
      "fontWeight": "400",
      "colorPrompt": true,
      "motd": true,
      "motdLogoOn": false,
      "motdLogo": "",
      "scrollback": 10000,
      "trimOnCopy": true,
      "pasteGuard": false,
      "rightClick": "menu",
      "cursorStyle": "bar",
      "cursorBlink": true,
      "autoReconnect": true,
      "autoReconnectMaxAttempts": 5,
      "env": [{ "key": "EDITOR", "value": "nvim" }],
      "key.newTab": "⌘+T",
      "key.closeTab": "⌘+W",
      "key.splitV": "⌘+D",
      "key.splitH": "⌘+E",
      "key.clear": "⌘+L",
      "key.palette": "⌘+K",
      "key.find": "⌘+F",
      "key.settings": "⌘+,",
      "gpuAccel": true,
      "renderer": "auto",
      "memInterval": 2,
      "metricsInterval": 3,
      "memPauseHidden": true,
      "memAlertMb": 300,
      "sftpChunkKb": 64,
      "sftpResume": true,
      "sftpResumeCheck": "size",
      "sftpOverwritePolicy": "newer",
      "sftpUploadTemp": true,
      "sftpPreserveMeta": false,
      "sftpGlobalConcurrency": 3,
      "sftpHostConcurrency": 1,
      "sftpGlobalRateKb": 0,
      "sftpTaskRateKb": 0,
      "sftpRetryCount": 3,
      "sftpRetryIntervalMs": 1000,
      "sftpIdleTimeoutSec": 300,
      "sftpVerifyHash": false,
      "sftpBlacklist": ".DS_Store\nThumbs.db",
      "sftpShowHidden": false,
      "autoUpdate": true,
      "updateChannel": "stable"
    },
    "keys": [
      {
        "id": "k_seed_prod",
        "name": "生产环境部署",
        "type": "ED25519",
        "bits": "256",
        "comment": "deploy@prod-web-01",
        "fingerprint": "SHA256:9xK2...",
        "publicKey": "ssh-ed25519 AAAAC3...",
        "passphrase": true,
        "created": "2026-05-10T12:00:00Z",
        "lastUsed": "2026-10-06T10:00:00Z",
        "hosts": ["prod-web-01"],
        "tags": ["生产", "部署"]
      }
    ],
    "groups": [
      { "name": "生产环境", "color": "green" },
      { "name": "测试环境", "color": "purple" },
      { "name": "开发环境", "color": "blue" },
      { "name": "其他", "color": "yellow" }
    ],
    "ui_state": {
      "homeView": "hosts",
      "layout": {},
      "sessions": ["prod-web-01", "prod-db-01"],
      "logWrap": true
    },
    "quick_connect_history": [
      { "host": "root@192.168.1.1", "timestamp": 1728222600000 }
    ]
  }
}
```

**字段说明：**

- `version`：配置结构版本，当前为 `1`，用于未来迁移与兼容性校验。
- `exported_at`：ISO 8601 带时区偏移，用于追溯。
- `app_version`：导出时应用版本号，辅助诊断兼容问题。
- `meta.scope`：导出范围（见 §4.1）。后端组装时按 scope 过滤，导入端只认 `data` 中实际存在的节。
- `meta.data_sha256`：**明文导出时**对 `data` 节计算的 SHA-256（导出端写入，导入端重算比对，§7.2 步骤 3）；加密 envelope 不需要（GCM tag 已保证完整性）。
- `meta.include_passwords`：始终为 `false`，显式声明不含密码。
- `meta.hostname`：导出设备名，帮助用户识别来源。
- `data` 下各节均可独立缺失（缺失节不动本地）；但存在的节必须通过 schema 校验，否则整体终止（§9.4）。

### 4.1 导出范围（scope）

当前支持三种范围，通过 `meta.scope` 标识。后端组装全量 Schema 后按 scope 过滤：

| scope | 含义 | 过滤后 `data` 包含的节 |
|---|---|---|
| `full` | 全量导出（默认） | 默认含 `connections` / `settings` / `keys` / `groups`；**`ui_state` 与 `quick_connect_history` 默认排除**，仅在导出面板勾选「包含界面布局」「包含快速连接历史」时写入（减少跨设备导入导致的布局破坏，见 §8.1） |
| `hosts` | 仅主机 | 仅 `connections` + `groups`（groups 作为主机归属元数据附带，否则导入方会丢分组色） |
| `ui` | 仅界面布局 | 仅 `ui_state`（homeView / layout / sessions / logWrap / configExportScope）；不含外观设置（主题/强调色/字号属于 `settings`，随 `full` 导出） |

**Schema 扩展约定：** 后续新增 scope（如 `settings`、`keys`）时，枚举值加在 `meta.scope`，后端过滤逻辑扩展子集即可，**不改 `version`**；旧版本应用遇到未知 scope 按「导入包含的节」处理（容错）。

**实现约束：`meta.scope` 仅是导出时的过滤依据与信息记录（含日志 kv）；导入代码路径严禁读取 `meta.scope`**，只检查 `data` 中实际存在哪些节（§6.1）。此约束作为 code review 检查点落实，避免 scope 被误用作导入判断条件。

---

## 5. 敏感信息处理策略

| 数据项 | 导出策略 | 导入策略 |
|---|---|---|
| 连接密码 | **不导出**。钥匙串数据无法跨设备/平台批量迁移，且安全风险高。 | 导入后密码字段为空，首次连接时提示用户输入并重新存入钥匙串。 |
| 私钥口令 | 同上，不导出。 | 同上。 |
| 私钥文件内容与路径 | **不导出**。密钥 `privatePath` 与连接 `keyPath` 一并剔除（路径泄漏本机目录结构，跨设备无意义）。 | 导入后 `privatePath` / `keyPath` 为空，密钥元数据（指纹/公钥/备注）与连接其余字段保留；用户需重新选择私钥文件完成绑定。 |
| SSH 密钥元数据 | 导出公钥、指纹、注释等（`privatePath` 剔除，见上行）。 | 直接恢复元数据列表。 |
| 日志配置（`logs` 节，含 `logStoragePath`） | **不参与导入导出**——日志路径是设备本地配置，跨设备无意义。 | 导入端忽略 `logs` 节（即使文件中存在也不写入）。 |
| 主机密钥信任库（`app_data_dir/known_hosts.json`） | **不参与导入导出**——TOFU 指纹是本机当面核对后建立的信任，导出到他机会绕过该机用户的首连确认；换机/重装后重新弹首连确认即为正确安全语义。 | 不在导出 Schema 中；导入端即使遇到该节也忽略。文件独立于 `connections.json` / `app_config.json`，导入全程不读取、不写入。 |

### 5.1 密码加密导出（P1 可选功能）

导出面板勾选「加密导出」后，导出文件整体加密。**加解密全部在后端完成**——明文 JSON 不经过 IPC 返回前端（避免超大配置 IPC 传输与明文暴露面）：

- **导出**：前端弹密码输入 → `export_config(path, scope, include_ui, include_history, password)` → 后端组装明文 Schema → PBKDF2-SHA256 派生密钥 → AES-256-GCM 加密 → **后端直接写 envelope 文件**（RustCrypto `aes-gcm` + `pbkdf2` crate）。
- **导入**：前端读文件 → 检测顶层 `meta.encrypted == true` → 提示输入密码 → `import_config(payload, password)` → 后端内存解密（GCM auth tag 校验，密码错误即报「密码错误或文件损坏」）→ 明文走标准解析/校验/写盘流程。
- **密码安全红线**：密码只以前端输入的 IPC 字符串参数存在，**不落盘、不进 localStorage、不进任何日志**（applog 红线：禁记 `password` 参数）；后端使用完毕立即 `zeroize`。
- ~~`assemble_config` IPC~~ **已取消**——不再把完整明文 JSON 通过 IPC 返回前端，消除明文暴露与超大配置 OOM 面。

**Envelope 格式**（`version` 不变仍为 1——加密封装在传输层，解密后是标准 Schema）：

```json
{
  "version": 1,
  "meta": {
    "encrypted": true,
    "cipher": "aes-256-gcm",
    "kdf": "pbkdf2-sha256",
    "kdf_iters": 210000,
    "salt": "<base64, 16B>",
    "iv": "<base64, 12B>"
  },
  "ciphertext": "<base64>"
}
```

解密后 `ciphertext` 内容 = 标准 Schema 的完整 JSON（含明文 `meta` + `data`）。加密文件**不需要** `data_sha256`（GCM auth tag 已提供完整性校验）。

**P1 含义**：设计已定型，实现可在 MVP 之后第一个迭代落地；勾选框入口已预留在 §8.1。

---

## 6. 导入策略：固定智能合并

**设计原则：不弹窗、不选择、一键完成。** 导入固定走「智能合并」逻辑；**不做自动备份、不提供撤销**——失败场景由「前置校验零写入 + 原子写」兜住，后悔场景由用户**主动**用【导出到文件】（全量配置、密码加密）兜底。

### 6.1 智能合并规则（唯一固定模式）

合并规则按节独立适用，**导入文件缺失的节完全不动本地数据**（这就是 scope 在导入端的自然语义——导入端无需读取 `meta.scope`，只需检查 `data` 中实际存在哪些节）：

- **连接**：按 `id` 合并。**id 冲突时继续导入并重命名**：导入的连接分配新 id，显示名追加「（导入）」后缀；本地原连接**保留不动**（禁止静默覆盖用户已有连接）。连接里的 `keyPath` 已剔除（§5），导入后为空需用户重选。
- **设置**：导入文件中存在的字段覆盖本地；缺失字段保留本地值。**跨平台容错**：快捷键 `key.*` 含平台修饰键（如 macOS `⌘`），目标平台不认识的修饰键静默忽略（回退默认绑定）。
- **密钥**：按 `id` 合并，冲突策略同连接（id 冲突 → 新 id + 名称追加「（导入）」）。
- **分组**：按名称去重并集，导入文件中的分组定义优先（颜色等元数据以导入为准）。
- **UI 状态**：全量覆盖（`homeView`、`layout`、`sessions`、`logWrap` 等）。
- **快速连接历史**：追加导入文件中的记录，不去重；合并后按时间倒序**只保留最近 50 条**（防多次导入无限膨胀）。

**id 重映射（防引用悬空）**：连接 id 冲突被改新 id 后，必须同步修正两处引用：
- `keys[].hosts`（按连接 id 引用主机）——旧 id → 新 id 映射，映射不到的 id 从列表剔除；
- `ui_state.sessions`（会话恢复列表，同为连接 id）——同样重映射，映射不到的丢弃。
**导入文件中 `keys[].hosts` 引用了文件内不存在的连接 id 时**，同样剔除（防导入半成品引用）。

**「仅主机」导入示例**：文件 `data` 仅含 `connections` + `groups` 两节 → 只按 §6.1 规则合并连接与分组，`settings` / `keys` / `ui_state` 等本地数据**完全不变**。

### 6.2 无自动备份（决策定稿）

导入**不做自动备份**，理由：

1. 无 UI 消费入口的备份是隐形死代码（撤销入口已移除，用户不知道备份存在）；
2. 失败场景已被「前置校验零写入（§9）+ 原子写」兜住——校验全在写盘之前，崩溃/写坏不会留中间态；
3. 后悔场景由【导出到文件】提供**用户主动**导出（全量配置、密码加密）——主动导出的用户才知道文件在哪。

---

## 7. 后端 IPC API 设计

新增六个 IPC 命令（注册于 `ipc.rs` `generate_handler!`），分两组：

- **导入导出/重置**：`export_config` / `import_config` / `import_hosts` / `reset_settings_config`
- **统一配置读写**（方案 A 的日常读写通道）：`load_app_config` / `set_app_config_section`

所有导入导出命令**不再需要前端传快照**——后端自己掌握全部数据，这正是不一致风险消除后的自然结果。

### 7.1 `export_config`

```rust
#[tauri::command]
async fn export_config(
    app: AppHandle,
    path: String,
    scope: String,          // "full" | "hosts" | "ui"，见 §4.1
    include_ui: bool,       // 仅 scope=full 有效：附加 ui_state 节
    include_history: bool,  // 仅 scope=full 有效：附加 quick_connect_history 节
    password: Option<String>, // P1：加密导出密码；None=明文
) -> Result<String, String>
```

- 后端自行读取 `connections.json`、`app_config.json`（含 settings/keys/groups/ui_state/history 各节）。
- **组装大小上限检查**：组装明文 Schema 后检查 JSON 字符串长度；若超过 `MAX_IMPORT_FILE_BYTES`（5MB）→ 拒绝并返回错误「配置数据过大，无法导出」，记 `app.config.export_failed`（`stage=assemble`）。这是为加密场景兜底（明文超限则加密输出必超限）。
- 按 `scope` 过滤 `data` 节（`hosts`：只保留 `connections` + `groups`；`ui`：只保留 `ui_state`；`full`：保留 `connections`/`settings`/`keys`/`groups`，再按 `include_ui` / `include_history` 附加对应节；被排除的节**整个 key 不写入**）→ `meta.scope` 写入对应值。
- **路径脱敏**：组装时剔除密钥对象的 `privatePath` 与连接对象的 `keyPath`（路径泄漏本机目录结构，跨设备无意义，§3.2/§5）。
- **加密写文件（P1）**：`password` 为 `Some` 时，后端 PBKDF2-SHA256 派生密钥 → AES-256-GCM 加密 → 写 envelope 文件（§5.1）。**密码使用完毕立即 `zeroize`**。
- **明文写文件**：`password` 为 `None` 时，写标准 Schema JSON。**明文场景生成 SHA-256 校验**：对 `data` 节（解析后重新序列化——`serde_json::Value` 默认 BTreeMap 键序，两端序列化结果确定一致）计算 SHA-256，写入 `meta.data_sha256`；导入端同样「解析 → 重序列化 data 节 → 重算比对」，用于发现传输/存储篡改。
- 原子写入用户指定的 `path`，返回实际写入的绝对路径。

```
前端选 scope（默认 full）+ 附加勾选
       │
       ▼
invoke export_config(path, scope, include_ui, include_history)
       │
       ▼
后端读 connections.json + app_config.json
组装 ExportSchema → 按 scope/勾选过滤
       │
       ▼
write_atomic(path, schema) → 返回绝对路径
```

### 7.2 `import_config`

```rust
#[tauri::command]
async fn import_config(
    app: AppHandle,
    payload: String,           // 导入文件完整 JSON 文本（或加密 envelope）
    password: Option<String>,  // 加密文件的解密密码；明文文件传 None
) -> Result<ImportSummary, String>
```

**执行顺序：**

1. **大小限制**：`payload` 字节数 > `MAX_IMPORT_FILE_BYTES`（**5MB**，正常全量配置 < 100KB）→ 直接拒绝，toast「配置文件过大（>5MB），请确认文件来源」，记 `app.config.import.failed`（`stage=size`，kv 带 `source`（文件名）+ `bytes`）。**阈值单一来源**：`MAX_IMPORT_FILE_BYTES` 为后端唯一常量，前端 stat 预检阈值通过 `AppConfigSnapshot` 下发，禁止前端另写死。
2. **解密（如需要）**：`meta.encrypted == true` 且提供了 `password` → 后端内存解密（AES-256-GCM tag 校验）。**面向用户统一**：认证失败 toast「密码错误或文件损坏」（不区分密码错与密文篡改，避免预言机）；**本地日志细分**：记 `import.failed`（`stage=decrypt`，另带 `reason`：`bad_password`=envelope 结构合法但 GCM 认证失败，`error` 记「密码错误（GCM 认证失败；若确认密码无误则为文件损坏）」；`corrupted`=envelope JSON/字段/算法标识/长度非法，`error` 记「文件损坏或格式无效：{细节}」）。**密码用毕即 `zeroize`，不进任何日志**。
3. **完整性校验（非加密场景）**：明文文件且存在 `meta.data_sha256` → 重算 `data` 节 SHA-256 比对；不匹配 → 拒绝「文件校验失败，可能被篡改或损坏」，记 `import.failed`（`stage=hash`）。加密文件跳过此步（GCM tag 已保证）。
4. **解析校验与版本校验**（详见 §9）：合法 JSON → 整数 `version` → **仅接受 `version == CURRENT`（当前=1）；`version > CURRENT` 拒绝并提示升级；`version < 1` 视为格式错误**（迁移链 MVP 不实现，见 §9.3）→ **逐节 schema 校验（§9.4）：关键节失败整体终止；非关键节容错跳过 + 告警**。致命失败立即返回错误，**不写入任何内容**。
5. **原子写入**（**只处理 `data` 中实际存在的节**，缺失节不动本地——天然支持「仅主机」等部分导出文件；**导入逻辑不读取 `meta.scope`**，§4.1；**`logs` 节即使存在也忽略**，§5）：
   - `data.connections` 存在 → `store::import_hosts_merge`（id 冲突重命名，§6.1）→ `store::write_hosts`（原子写，需提升为 `pub`）。
   - `data.settings` 存在 → 写 `settings` 节（`save_section`；**不含 log\* 字段**，日志配置不参与导入，§5）。
   - `data.keys` / `data.groups` / `data.ui_state` / `data.quick_connect_history` 存在 → 各自 `save_section`（history 合并后截断至 50 条，§6.1）。
6. **返回摘要**：

```rust
struct ImportSummary {
    connections_added: usize,
    connections_renamed: usize,  // id 冲突被重命名追加「（导入）」的数量
    keys_added: usize,
    keys_renamed: usize,
    groups_added: usize,
    settings_changed: usize,
}
// 注：导入完成后一律要求重启（§8.3），不再通过返回字段表达——恒真字段是噪音。
```

前端收到成功后**调用 `load_app_config` + `loadHosts` 重新加载全部状态**（§8.4），无需写回任何数据。

### 7.3 `load_app_config`（启动全量加载）

```rust
#[tauri::command]
fn load_app_config(app: AppHandle) -> AppConfigSnapshot
```

```rust
struct AppConfigSnapshot {
    logs: HubConfig,                       // logs 节（前端映射为 AppSettings.log*）
    settings: serde_json::Value,           // settings 节（其余 AppSettings 字段）
    keys: Vec<serde_json::Value>,          // keys 节
    groups: Vec<serde_json::Value>,        // groups 节
    ui_state: serde_json::Value,           // homeView/layout/sessions/logWrap/configExportScope
    quick_connect_history: Vec<serde_json::Value>,
    max_import_file_bytes: u64,            // 导入大小阈值（前端 stat 预检的唯一来源，禁止前端写死）
}
```

- 前端启动时调用一次，初始化全部 store。
- 缺失节/字段经 `#[serde(default)]` 回退默认，永远成功（不返回 Result）。

### 7.4 `set_app_config_section`（写穿持久化）

```rust
#[tauri::command]
async fn set_app_config_section(
    app: AppHandle,
    section: String,          // "logs" | "settings" | "keys" | "groups" | "ui_state" | "quick_connect_history"
    value: serde_json::Value,
) -> Result<(), String>
```

- **获取全局写锁**（§7.5）→ 读出 `app_config.json` → **校验 value（§7.5）** → 替换指定节 → 原子写回；未知顶层节与节内未知字段经 `extra` 保留（沿用 `persisted.rs` 既有模式）。
- **两级校验**：
  1. `section` 白名单：非法节名直接拒绝；
  2. `value` schema 校验（§7.5）：畸形 value 拒绝写入，记 `app.config.write_rejected`，**不污染磁盘**。
- 前端各 store 的变更统一走此通道（防抖 300ms 仅为体验优化，**正确性不依赖前端防抖**——并发写由后端串行锁兜底）。

### 7.5 全局写锁与 Schema 校验

#### 并发控制：全局异步写锁

`app_config.json` 与 `connections.json` 的所有写入共享一个 `tokio::sync::Mutex<()>`（注册为 Tauri managed state）——**一把锁同时保护两个配置文件**，因为 `import_config` 需要跨文件原子语义（连接 + 配置节在同一临界区内写完）：

```rust
pub struct ConfigWriteLock(pub tokio::sync::Mutex<()>);
```

**必须持锁的操作（临界区 = 读-改-写 全程）：**

| 操作 | 临界区内容 |
|---|---|
| `set_app_config_section` | 读旧文件 → 校验 → 替换节 → 原子写回 |
| `import_config` / `import_hosts` | 解析校验 → 合并（含 id 重映射）→ 原子写 `connections.json` 与各节（import_hosts 只写 connections 与 groups） |
| `save_connection` / `delete_connection`（既有连接 IPC） | 读 → 改 → 原子写 `connections.json`——**纳入同一把锁**，否则与 `import_config` 并发会 lost update |
| `reset_settings_config` | settings 节替换为默认值 → 原子写回 |
| `save_logs`（applog 既有） | 纳入同一锁，消除日志配置写与其他写的并发 |

**为什么不用 `std::sync::Mutex`**：临界区含文件 IO，std 锁会阻塞 async worker 线程；`tokio::sync::Mutex` 在 IO 等待时让出执行权。

**为什么不用读写锁**：写入是用户交互级低频，读取集中在启动与导入后刷新，无读放大；互斥锁推理更简单。

**为什么前端防抖不够**：防抖只合并**同一组件**的连续变更；多个组件并发改不同节时仍产生并发 IPC——「读-改-写」交错会导致后写覆盖先写（lost update）。原子 rename 只能保证文件不损坏，不能保证逻辑顺序，故必须在后端串行化。前端 300ms 防抖保留，仅作减少写盘频率的体验优化。

**`connections.json` 同样受锁保护**：`import_config` 会写连接文件，与 `save_connection` / `delete_connection` 存在真实并发窗口（导入进行中用户恰好保存一条连接 → 读-改-写交错 → lost update），故既有连接写 IPC 也必须获取同一把锁（见上表）。

#### Per-section Schema 校验（写前执行）

每个节有对应的强类型 Rust struct，**反序列化即校验**；校验在持锁期间、原子写之前执行，失败即拒绝并返回错误——**磁盘不受任何影响**。

| section | 校验目标 | 校验语义 |
|---|---|---|
| `logs` | `LogsSection`（既有） | 沿用现有 sanitize：非法枚举/越界值回退默认（如 `max_lines=0→1`） |
| `settings` | `SettingsSection`（新增强类型 struct，字段与前端 `AppSettings` 对齐，log\* 除外） | 字段类型不符 → 拒绝；未知字段经 `extra` 保留（前向兼容） |
| `keys` | `Vec<SshKeySection>` | 元素缺 `id`/`name`/`type` → 拒绝 |
| `groups` | `Vec<GroupSection>` | 元素缺 `id`/`name`/`color` → 拒绝 |
| `ui_state` | `UiStateSection` | 已知子字段类型校验；未知子字段保留 |
| `quick_connect_history` | `Vec<QuickConnectItem>` | 元素缺必填字段 → 拒绝 |

拒绝时记 WARN `app.config.write_rejected`（kv：`section`、`reason`），前端 toast「设置保存失败」。

### 7.6 `reset_settings_config`（恢复默认设置）

```rust
#[tauri::command]
async fn reset_settings_config(app: AppHandle) -> Result<(), String>
```

**只重置 `settings` 节回默认值**（外观/终端/快捷键/SFTP 等），**不动** `connections` / `keys` / `groups` / `logs` / `ui_state` / `quick_connect_history`。

- 持写锁（§7.5）：读 `app_config.json` → `settings` 节替换为 `SettingsSection::default()` → 原子写回；
- 前端收到成功后按 §8.4 刷新 + toast「设置已恢复默认，部分设置需重启后生效」+「立即重启」（§8.3 同款 relaunch）；
- 记 `app.config.settings_reset`。

### 7.7 `import_hosts`（单独导入主机）

```rust
#[tauri::command]
async fn import_hosts(
    app: AppHandle,
    payload: String,           // 导入文件完整 JSON 文本（或加密 envelope）
    password: Option<String>,
) -> Result<ImportSummary, String>
```

从主机列表页直接导入主机，无需进设置面板。语义 = `import_config` 的 `hosts` 子集：

- **复用 `import_config` 的完整校验链**（§7.2 步骤 1~4：大小/解密/哈希/版本/节校验），但**只消费 `data.connections` 与 `data.groups` 两节**，其余节一律忽略（即使文件中存在也不写入）——与「仅主机」导出文件天然配对，也兼容全量导出文件（只取其中主机部分）。
- 连接合并与 id 冲突重命名、id 重映射规则同 §6.1（`ui_state.sessions` 重映射同样适用）。
- 持写锁（§7.5），临界区同 `import_config`。
- 前端入口：主机列表页工具栏【导入主机】按钮 → 系统打开对话框 → 流程同 §8.3（stat 预检/加密密码/摘要 toast）；**不弹重启确认**——只合并主机与分组两节，热刷新后即完全生效。
- 日志事件复用 `app.config.import.*`（kv `scope=hosts`）。

---

## 8. 前端 UI/UX 流程

**核心原则：每个操作 ≤ 2 步，无中间弹窗，无 merge/overwrite 选择。** 唯一的「弹窗」是系统文件对话框（OS 原生，不计入应用弹窗）。

### 8.1 设置面板入口

`SettingsModal.vue` →「配置文件」区域：

```
┌────────────────────────────────────────────────────────┐
│  配置导入导出                                           │
│  将连接、设置、密钥等导出为文件，                         │
│  或从文件恢复。导入时缺失的节不动本地数据。                │
├────────────────────────────────────────────────────────┤
│  导出范围  [ 全量 ▾ ]   ← 常驻下拉：全量 / 仅主机 / 仅界面布局 │
│  导出始终密码加密（AES-256-GCM，见 §5.1），含全部可选节         │
│  [ 导出到文件... ]  [ 从文件恢复... ]                        │
│  ──────────────────────────────────────────────────────  │
│  [ 恢复默认设置 ]   ← 仅重置设置节，普通确认弹窗（§7.6）      │
└────────────────────────────────────────────────────────┘
```

**范围下拉设计：**

- 复用 SettingsModal 现有的 `kind: 'select'` 行样式。
- 范围选择持久化到后端 `ui_state` 节（`configExportScope`），由 `set_app_config_section` 写穿。
- 默认值 `full`。
- 导出**固定包含全部可选节**（界面布局、快速连接历史）且**固定密码加密**——无勾选框，点击导出时两次输入密码确认。
- 「全量配置」导出即升级/换机前的兜底快照（原「立即备份」入口已移除——两者后端链路等价，差异仅明文与加密，统一为加密导出）。

### 8.2 导出流程（2 步）

1. 用户点击「导出到文件...」→ **固定要求设置密码**（两次输入确认，§5.1）→ 弹出系统保存对话框，默认文件名按范围区分：
   - 全量：`rhost_config_YYYYMMDD_HHMMSS.json`
   - 仅主机：`rhost_hosts_YYYYMMDD_HHMMSS.json`
   - 仅界面布局：`rhost_ui_YYYYMMDD_HHMMSS.json`
2. 用户确认保存位置：调用 `export_config(path, scope, true, true, password)` → **后端加密并直接写 envelope 文件**（明文不经 IPC，§5.1）→ toast `加密配置已导出至 ...`。

**注意：** 导出不再由前端收集任何数据——后端自己读盘组装，这是统一持久化的直接收益。

### 8.3 导入流程（2 步）

1. 用户点击「从文件恢复...」→ 弹出系统打开对话框，选择 `.json` / `.rhost.json` 文件。
2. 用户确认文件 → 前端 stat 预检大小（阈值取自 `AppConfigSnapshot`，当前 5MB，超限直接拒绝不读内容）→ 读取文件 → **若检测到 `meta.encrypted == true`：提示输入密码 → `import_config(payload, password)`，后端解密（§5.1）**；明文文件 → `import_config(payload, null)` → 成功后从后端重新加载全部状态（§8.4）→ toast 摘要 → 弹出**重启确认弹窗**：

   ```
   ┌────────────────────────────┐
   │  导入完成                    │
   │  配置已导入，重启应用后完全生效。│
   │  是否立即重启？               │
   │              [稍后] [立即重启] │
   └────────────────────────────┘
   ```

   **导入完成后一律要求重启**（任何 scope 的 `import_config` 均弹窗，与导入范围无关；但工具栏 `import_hosts` 除外，见 §7.7）：布局/会话恢复依赖启动序列；部分设置在运行时不监听热更新。弹窗由全局 `useConfirm`（ConfirmModal）承载，「立即重启」调用 `@tauri-apps/plugin-process` 的 `relaunch()`；「稍后」/Esc/关闭仅关弹窗（§8.4 已把内存态刷成与磁盘一致，继续使用期间写穿安全，不会回写旧数据）。设置面板不再提供常驻「立即重启」入口。

### 8.4 全局状态刷新（导入后）

前端收到 `import_config` 成功后，统一走此刷新路径（**只从后端加载，不写 localStorage**）：

1. `loadHosts()` → 重新加载连接列表；
2. `load_app_config()` → 映射到各 store：
   - `logs` → `savedSettings` 的 `log*` 字段；
   - `settings` → `savedSettings` 其余字段 + `applyAccent()`；
   - `keys` → `keys` store；
   - `groups` → `groups` store；
   - `ui_state` → `homeView`、`sessions`、`layout`、`logWrap`、导出范围下拉；
   - `quick_connect_history` → 快速连接历史；
3. 弹出重启确认弹窗（§8.3）。

**为什么刷新之后还要求重启**：刷新保证内存态与磁盘一致（继续使用期间写穿安全、UI 立即反映新数据）；重启保证**冷启动路径**也读到新配置——`logs` 节只在 Hub 初始化时读取、布局/会话恢复只在启动序列执行。两者职责互补，缺一不可。

### 8.5 恢复默认设置流程（2 步）

1. 用户点击「恢复默认设置」→ 弹出**普通确认弹窗**（非危险操作，普通文案）：「重置所有外观、终端、快捷键等设置回默认值，连接和密钥不会受影响。」
2. 用户确认 → 调用 `reset_settings_config`（§7.6）→ 按 §8.4 刷新内存态 → toast「设置项已恢复为默认值」（重启提示以 toast 文案表达，不弹重启确认——仅 settings 节，多数项热生效）。

---

## 9. 导入校验与版本迁移

### 9.1 合法性校验（版本判断之前）

任一失败即拒绝导入，**不写入任何内容**：

1. **合法 JSON**：文件内容必须能被 `serde_json::from_str` 解析；失败 toast「配置文件格式错误，请检查文件是否完整」。
2. **顶层结构**：必须是 JSON 对象，且含**整数** `version` 字段；缺失或非整数按格式错误处理。
3. **`data` 字段**（若存在）必须是对象；节内类型错误按 §9.4 处理（关键节整体终止，非关键节容错跳过）。

### 9.2 版本校验（MVP：仅相等判断）

设文件版本为 `file_version`，当前 `CURRENT_SCHEMA_VERSION = 1`。MVP 阶段**迁移链仅留骨架，不做实现**（§9.3），版本校验只有两条路径：

| 条件 | 处理 |
|---|---|
| `file_version == 1` | **直接解析**，零迁移开销 |
| `file_version > 1` | **拒绝导入**，toast「配置文件来自更高版本客户端，请升级应用」 |
| `file_version < 1` 或非整数 | **拒绝导入**，toast「配置文件格式错误」（§9.1 已覆盖非整数，此处为语义兜底） |

### 9.3 迁移框架骨架（留待 schema 首次变更时实现）

后端新增 `src-tauri/src/config_migrate.rs`，**MVP 只保留空骨架 + 版本校验**，完整迁移链在第一次 schema 变更时补齐：

```rust
/// 单步迁移：输入旧版本 data JSON，输出新版本 data JSON。
/// 纯函数，只负责结构变换（字段补齐、改名、类型转换），不读写磁盘。
type MigrationFn = fn(serde_json::Value) -> Result<serde_json::Value, String>;

/// 迁移注册表：索引 i 存放 v(i+1) → v(i+2) 的迁移函数。
/// MVP 为空表；发布 v2 时在末尾追加 migrate_v1_to_v2，CURRENT_SCHEMA_VERSION 自动随之 +1。
static MIGRATIONS: &[MigrationFn] = &[];

/// 版本号自动推导：1 + 迁移步数。新增迁移只需在 MIGRATIONS 末尾追加函数，不会出现「加了迁移忘了改常量」的不一致。
pub const CURRENT_SCHEMA_VERSION: u32 = 1 + MIGRATIONS.len() as u32;

/// MVP：仅版本相等校验。file_version > CURRENT 返回 Err；== CURRENT 原样返回；< CURRENT 当前不可达（MIGRATIONS 为空）。
pub fn migrate_to_current(mut data: serde_json::Value, file_version: u32) -> Result<serde_json::Value, String> {
    if file_version > CURRENT_SCHEMA_VERSION {
        return Err("配置文件来自更高版本客户端，请升级应用".into());
    }
    for v in file_version..CURRENT_SCHEMA_VERSION {
        data = MIGRATIONS[(v - 1) as usize](data)?;  // MVP 不会进入循环
    }
    Ok(data)
}
```

**未来启用迁移链时的约定（变更前补全）：**

- 每步迁移是**纯函数** `Value → Value`：不依赖 `AppHandle`、不读写磁盘，便于单测；
- **字段补齐优先靠 `#[serde(default)]`** 反序列化兜底；迁移脚本只处理默认值无法表达的**结构性变更**（改名、层级调整、类型转换）；
- 每步迁移必须配单测：旧版 JSON 样本 → 迁移 → 断言新结构关键字段；
- 迁移链**只升不降**：`file_version > CURRENT` 永不尝试降级解析；
- 迁移失败：整个导入失败，toast「配置文件版本迁移失败：{原因}」，记 `app.config.import.failed`（`stage=migrate`），**未写入任何内容**（迁移在写盘之前）。

### 9.4 节级解析策略：缺失跳过；关键节损坏即终止，非关键节容错

节按重要性分两级：

| 级别 | 节 | 解析失败行为 |
|---|---|---|
| **关键节** | `connections`、`settings`、`keys`、`groups` | **整体终止导入**，toast「配置文件内容损坏：{section} 节格式错误」，记 `app.config.import.failed`（`stage=parse`、`section`），**零写入** |
| **非关键节** | `ui_state`、`quick_connect_history` | **容错跳过该节**（本地数据不动），记 WARN `app.config.import_section_skipped`（kv：`section`、`error`）；导入继续，完成后 toast 追加告警「界面布局/快速连接历史数据损坏，已跳过」 |

- **节缺失**（`data` 中不含该 key）：按「不动本地」处理（§6.1）——这是 scope 部分导出的正常语义，不算错误。
- **关键节禁止「部分成功部分失败」**：任一关键节校验不过，其他已通过的节也一律不写入——校验发生在写盘**之前**（§7.2 步骤 2），终止时零写入，磁盘不会留下新旧混杂的中间态，也无需回滚。
- **非关键节容错的理由**：布局/历史损坏不危及主机与凭证等核心资产；跳过不写入 = 本地原数据不受影响，语义与「节缺失」完全一致，用户仍能获得连接和设置。

### 9.5 向后兼容（旧应用读新配置）

- 利用 `#[serde(default)]` + `#[serde(flatten)] extra`（已在 `persisted.rs` 实现），旧版本读改写不丢未知字段。
- 旧应用遇到 `version` 更高的文件直接走 §9.2 第三分支拒绝，不会误解析。

---

## 10. 可观测性：导入导出操作日志

所有导入/导出/重置操作接入既有 applog 管线（`log::info!/warn!`，target `"app.config"`），事件 ID 遵循 `domain.action[.subaction]` 规范，在后端 `events.rs` 常量表登记并与前端 `stores/applog.ts` 同步。

### 10.1 事件目录

| event_id | 级别 | 触发时机 | kv 字段 |
|---|---|---|---|
| `app.config.export` | INFO | 导出成功 | `path`（导出文件路径）、`scope`、`sections`（实际写入的节数）、`bytes`、`elapsed_ms` |
| `app.config.export_failed` | WARN | 导出失败 | `stage`（`read`/`assemble`/`encrypt`/`write`）、`error`；成功事件另带 `encrypted` 布尔（不含密码） |
| `app.config.import.start` | INFO | 导入开始（文件解析通过后） | `file_bytes`、`file_version`、`scope`（来自 meta，仅信息记录） |
| `app.config.import.complete` | INFO | 导入成功 | `connections_added`、`connections_renamed`、`keys_added`、`keys_renamed`、`groups_added`、`settings_changed`、`elapsed_ms` |
| `app.config.import.failed` | WARN | 导入失败（用户可见的拒绝/终止） | `stage`（`size`/`parse`/`hash`/`decrypt`/`version`/`write`；`migrate` 预留至迁移链启用）、`error`（decrypt 阶段为日志细分文案，与返回前端的统一用户文案不同）、`source`（文件名，不含内容）、`bytes`；`stage=parse` 时另带 `section`；`stage=decrypt` 时另带 `reason`（`bad_password`/`corrupted`） |
| `app.config.import_section_skipped` | WARN | 非关键节损坏被容错跳过（§9.4） | `section`、`error` |
| `app.config.settings_reset` | INFO | 恢复默认设置成功（仅 settings 节） | `elapsed_ms` |
| `app.config.write_rejected` | WARN | `set_app_config_section` schema 校验拒绝 | `section`、`reason` |

### 10.2 日志内容红线

- **禁记配置值**：settings 字段值、`extra` 内容、密钥 `publicKey`、连接 IP/用户名等一律不进 kv——只记**计数、枚举、路径（导出目标路径）**；
- **禁记密码**：`password` 参数任何情况下不进日志（含 ERROR 级）；
- **禁记文件内容**：导入 payload 原文不记；
- `elapsed_ms` 用于性能观察：预期导出/导入 P99 < 500ms（纯本地文件 IO）。

### 10.3 排障路径示例

导入失败排查：`event_id:app.config.import.failed` → 看 `stage` 定位阶段 → `stage=version` 即版本过高；`stage=migrate` 看 `error` 定位迁移步；`stage=decrypt` 看 `reason` 区分密码错误（`bad_password`）与文件损坏（`corrupted`，细节在 `error`）。配合 `app.config.import.start` 的 `file_version` 可还原完整上下文。

---

## 11. 错误处理与回滚

| 阶段 | 错误场景 | 处理策略 |
|---|---|---|
| 解析 | JSON 损坏 / 非对象 / 缺少 `version` / `version` 非整数 | 拒绝，toast「配置文件格式错误」。**未写入任何内容**。 |
| 大小 | 文件 > 5MB（`MAX_IMPORT_FILE_BYTES`） | 拒绝，toast「配置文件过大（>5MB），请确认文件来源」，记 `app.config.import.failed`（`stage=size`）。**未写入任何内容**。 |
| 校验 | `version > CURRENT_SCHEMA_VERSION` | 拒绝，toast「配置文件来自更高版本客户端，请升级应用」。**未写入任何内容**。 |
| 校验 | `version < 1`（0 或负数） | 拒绝，toast「配置文件格式错误」。**未写入任何内容**。MVP 无迁移链，`version < CURRENT` 当前不可达；迁移链启用后此分支改为迁移失败处理（§9.3）。 |
| 校验 | `meta.data_sha256` 与 `data` 节重算不匹配（明文文件） | 拒绝，toast「文件校验失败，可能被篡改或损坏」，记 `import.failed`（`stage=hash`）。**未写入任何内容**。 |
| 校验 | 加密文件密码错误（envelope 合法、GCM tag 校验失败） | 拒绝，toast「密码错误或文件损坏」，记 `import.failed`（`stage=decrypt`、`reason=bad_password`，日志文案直述「密码错误」）。**未写入任何内容**。 |
| 解析 | 加密 envelope 结构损坏（JSON/`meta`/算法标识/`salt`/`iv`/`ciphertext` 字段缺失或非法） | 拒绝，toast 为具体格式提示，记 `import.failed`（`stage=decrypt`、`reason=corrupted`，日志文案「文件损坏或格式无效：{细节}」）。**未写入任何内容**。 |
| 解析 | 关键节结构异常（`connections`/`settings`/`keys`/`groups`） | **整体终止导入**，toast「配置文件内容损坏：{section} 节格式错误」，记 `app.config.import.failed`（`stage=parse`）。**未写入任何内容**。 |
| 解析 | 非关键节结构异常（`ui_state`/`quick_connect_history`） | **容错跳过该节**，记 WARN `app.config.import_section_skipped`；导入继续，完成后 toast 追加告警；本地原有数据不动（§9.4）。 |
| 写入 | `connections.json` / `app_config.json` 写入失败（含 tmp→rename 失败） | 原子写保证原文件不被破坏；toast 明确「磁盘 IO 失败，配置未变更」；记 `app.config.import.failed`（`stage=write`）。 |
| 写穿 | `set_app_config_section` 节名非法 / value schema 校验失败 | 拒绝写入，toast「设置保存失败」，记 `app.config.write_rejected`；磁盘不受影响。 |
| 回滚 | 导入/重置后后悔 | **无自动备份与撤销**（§6.2）；需用户事先用【导出到文件】（全量配置、密码加密）主动导出。 |

---

## 12. 实现步骤（文件变更清单）

### 后端（`src-tauri/src/`）

| 文件 | 变更 |
|---|---|
| `ipc.rs` | 新增 `export_config` / `import_config` / `import_hosts` / `reset_settings_config` / `load_app_config` / `set_app_config_section` 六个 IPC 命令；注册到 `generate_handler!`；新增 `ImportSummary` / `AppConfigSnapshot` 结构体；实现明文导出 `data_sha256` 生成与导入校验；注册 `ConfigWriteLock` managed state，**既有 `save_connection` / `delete_connection` 同步纳入同一把锁**（§7.5）。 |
| `config_migrate.rs`（新建） | `CURRENT_SCHEMA_VERSION` 常量 + `migrate_to_current()` **MVP 仅做版本相等校验**（==1 通过 / >1 拒绝 / <1 拒绝）；`MIGRATIONS` 空注册表骨架预留（§9.3）；三分支单测。 |
| `config_crypto.rs`（新建，P1） | PBKDF2-SHA256 密钥派生 + AES-256-GCM 加解密（RustCrypto `aes-gcm`/`pbkdf2`/`zeroize` crate）；envelope 组装/解析（§5.1）；密码用毕 `zeroize`。 |
| `applog/events.rs` | 登记 §10.1 全部 8 个 `app.config.*` 事件 ID 常量。 |
| `store.rs` | `write_hosts` 提升为 `pub`；新增 `import_hosts_merge`（id 冲突 → 新 id + 名称追加「（导入）」，§6.1）。 |
| `applog/persisted.rs` | 扩展 `AppConfigFile` 新增 `settings` / `keys` / `groups` / `ui_state` / `quick_connect_history` 节（全部 `#[serde(default)]` + `extra` 保留未知节）；新增各节强类型 struct（§7.5 校验目标）；新增 `save_section(section, value)` 通用写穿接口（**内部获取 `ConfigWriteLock`**，`save_logs` 同步纳入同一锁）；解除 `save` 的 `#[allow(dead_code)]`。 |

### 前端（`frontend/src/`）

| 文件 | 变更 |
|---|---|
| `stores/settings.ts` | **移除 localStorage 持久化**；`savedSettings` 改为内存态；新增 `loadFromBackend(snapshot)`（从 `load_app_config` 结果映射 logs + settings 节）；新增 `persistSection()`（防抖 300ms，调用 `set_app_config_section`）；`saveSettings` 同步写穿。 |
| `stores/keys.ts` | 移除 localStorage；`keys` 改为内存态；变更时调用 `set_app_config_section('keys', ...)` 写穿。 |
| `stores/groups.ts` | 移除 localStorage；同上写穿 `'groups'`。 |
| `stores/homeView.ts` | 移除 localStorage；同上写穿 `'ui_state'`。 |
| `stores/session.ts` | 移除 `rhost.layout` / `rhost.sessions` localStorage；布局与会话恢复列表写穿 `'ui_state'`。 |
| `views/home/QuickConnectView.vue` | 移除 `rhost.quickConnect.history` localStorage；写穿 `'quick_connect_history'`。 |
| `components/wb/DockPanel.vue` | 移除 `rhost.logWrap` localStorage；写穿 `'ui_state'`。 |
| `components/SettingsModal.vue` | 移除「导出到剪贴板」与「立即备份」按钮（后者与全量加密导出链路等价，统一入口）；「配置文件」区域为「导出范围」常驻下拉（全量配置/仅主机与分组/仅界面偏好）+【导出…】/【导入…】/【恢复默认设置】，无常驻重启按钮；导出固定包含全部可选节且固定密码加密（无勾选框，点击导出时两次输入密码）；`onExportFile()` / `onImportFile()` 接后端 IPC（导入前 stat 预检，阈值取自 `AppConfigSnapshot.max_import_file_bytes`；加密文件传 password 给后端解密）；`onImportFile()` 成功后经全局 `useConfirm`（ConfirmModal）弹「立即重启 / 稍后」确认，确认调 `relaunch()`（`@tauri-apps/plugin-process`，需确认插件已注册）。 |
| `stores/applog.ts` | 同步登记 §10.1 的 8 个 `app.config.*` 事件 ID（与后端 `events.rs` 对齐）。 |
| 主机列表页工具栏 | 新增【导入主机】按钮 → `import_hosts` IPC（§7.7），流程同 §8.3（stat 预检/加密密码/摘要 toast），不弹重启确认（仅主机/分组节，热刷新即生效）。 |
| `App.vue`（启动序列） | 启动序列保持精简：`load_app_config` → `loadHosts` → `restoreSessions`。无 localStorage 迁移步骤（§3.1）。 |

### 设计文档

- 新建 `docs/config-import-export.md`（本文档）。

---

## 13. 待决策项（P2）

1. ~~密码加密导出~~ → **已提升为 P1 可选功能**，设计定稿见 §5.1：**加解密全部在后端完成**（RustCrypto PBKDF2-SHA256 + AES-256-GCM，密码经 IPC 传入、用毕 `zeroize`、不进日志）；明文不经 IPC 返回前端；envelope 封装不改 schema `version`。
2. **SFTP 传输历史/日志文件导出**：维持结论——**不纳入**配置导出。配置导出聚焦「可复现环境」，日志文件体积大且非配置。
3. **完全覆盖模式**：维持 P2 高级隐藏选项（长按或右键展开），**不做默认入口**；默认仅「智能合并」。
4. **更多导出范围**：`meta.scope` 已落地 `full` / `hosts` / `ui`，`full` 默认含核心数据（connections/settings/keys/groups），`ui_state` / `quick_connect_history` 在导出时固定包含（§4.1/§8.1）。Schema 约定新增 scope 不改 `version`。后续可按需加 `settings`（仅设置）、`keys`（仅密钥）等。
5. **撤销导入功能与自动/手动备份**：**均已移除**（决策定稿）。失败场景由「前置校验零写入 + 原子写」兜住；后悔场景由【导出到文件】（全量配置、密码加密，用户主动导出）兜住。若未来重新引入撤销，需新增 `undo_import_config` IPC + 导出文件扫描/配对/sha256 校验逻辑。
