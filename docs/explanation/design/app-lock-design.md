# 应用锁设计方案

> description: 应用级锁屏（手动 / 闲置 / 失焦 / 冷启动触发的全屏遮罩）的凭据存储、IPC、状态机与交互设计
>
> created: 2026-10-16 12:18:38
>
> updated: 2026-11-19 10:30:00
>
> author:

---

## 1. 设计目标与硬约束

> 范围：本文覆盖**应用级锁**（手动 / 闲置 / 失焦 / 冷启动触发的全屏锁屏），包含凭据存储、IPC 契约、前端状态机、拦截点与设置面板。
> 边界：不做生物识别（Touch ID / Windows Hello）；不做会话级锁（已于 2026-11-19 移除，理由见 §9 备注）；不改变 SSH 连接生命周期，锁定不断连；不做锁定后窗口最小化 / 系统缩略图模糊。
> 关联代码：`src-tauri/src/config_crypto.rs`（KDF 参数既有约定）、`src-tauri/src/applog/persisted.rs`（`SettingsSection` 与原子写）、`src-tauri/src/config_io.rs`（导出组装范围）、`src-tauri/src/lock.rs`、`src-tauri/src/lib.rs`、`frontend/src/stores/settings.ts`、`frontend/src/stores/appLock.ts`、`frontend/src/lib/lock.ts`、`frontend/src/App.vue`、`frontend/src/components/wb/StatusBar.vue`、`frontend/src/components/LockPasswordEditor.vue`、`frontend/src/components/SettingsModal.vue`。

| 目标 | 量化口径 |
|---|---|
| 解锁验证快 | PBKDF2-SHA256 210000 轮，主流机型单次验证 < 300ms |
| 锁屏响应快 | 触发到遮罩可输入 ≤ 100ms；闲置检测 tick 间隔 1s |
| 锁定不断连 | 锁定 / 解锁动作产生 0 次 SSH 断连，已建立会话、转发、进行中的 SFTP 传输不受影响 |
| 锁定即静默 | 锁定后 1 个 tick（≤ 5s）内停止全部远端 metrics 采集与心跳；本地内存轮询即时停摆，解锁后自动恢复 |
| 重启不可绕过 | 已启用应用锁时，冷启动 splash 撤下即呈现锁屏，不输入密码无法进行任何交互 |
| 防爆破 | 同一锁连续失败 5 次进入冷却，30s 起指数翻倍、封顶 300s |
| 凭据文件极小 | `lock.json` < 300 字节，仅含 salt/hash/iters，不含任何主机信息 |
| 零明文泄露 | 密码不落盘、不入日志、不进配置导入导出；前端任何时刻拿不到 hash |

## 2. 必须遵守的既有项目约束

- **敏感凭据只进系统钥匙串或加盐哈希，不进 `app_config.json`**：主机密码走 Keychain / Credential Manager；锁密码不可恢复，采用 PBKDF2 哈希存独立文件，二者都不进 `settings` 节。
- **KDF 参数与配置加密导出保持一致**：PBKDF2-HMAC-SHA256、210000 轮、16B salt（`src-tauri/src/config_crypto.rs` 中 `KDF_ITERS`），不另造参数。
- **设置项双份对齐**：新增 settings 字段必须同时改前端 `AppSettings` + `DEFAULT_SETTINGS` 与后端 `SettingsSection` + `Default`，两端不得漂移。
- **文件原子写**：临时文件 + rename（见 `store.rs::write_hosts`）。
- **IPC 命名 snake_case，错误中文文案透传**。
- **密码 / PTY 字节流永不入 applog**；日志只记事件类别与布尔结果。

## 3. 目标与非目标

### 目标

1. 用户可设置 / 修改 / 清除**应用锁密码**；启用后可手动锁屏，可开启闲置 N 分钟自动锁屏、窗口失焦锁屏。关闭总开关**只停用锁屏触发、不清除密码**（用户直觉：开关 = 临时禁用）；清除密码是需单独验证的操作。
2. 已启用应用锁时**冷启动即锁定**：splash 撤下后必须先解锁，重启不能绕过。
3. 锁定期间暂停远端 metrics 与本地内存轮询，解锁后自动恢复。
4. 状态栏与首页标题栏提供常驻锁图标；连续解锁失败有冷却限速；忘记密码有明确的本机重置路径。

### 非目标

- 不做生物识别解锁（后续可叠加，不影响存储结构）。
- 不做会话级锁（已评估并移除，见 §9）。
- 不做锁定后隐藏 / 最小化窗口（macOS 调度中心缩略图模糊留待后续）。
- 不防御能读取进程内存或文件系统的本机攻击者。
- 锁定期间不中断 SSH 连接、进行中的 SFTP 传输与端口转发。

## 4. 总体架构

```text
┌─────────────────────────────── 前端（Vue） ───────────────────────────────┐
│                                                                          │
│  SettingsModal「安全」分区         StatusBar 最左侧 / 首页 TitleBar 锁图标 │
│  设置/修改/清除密码、闲置下拉、     快捷键 ⌘/Ctrl+Shift+L                │
│  失焦开关                   persist  手动锁定                            │
│         │ settings 节                    │                              │
│         ▼                                ▼                              │
│  stores/settings.ts                 stores/appLock.ts                   │
│  appLockEnabled / idleSec /          ┌────────────────────────────────┐   │
│  lockOnBlur                          │ appLocked:ref                │   │
│                                      │ initAppLock() 闲置/失焦 watcher│   │
│                                      │ lockApp() unlockApp(pwd)      │   │
│                                      │ refreshLockSlots()            │   │
│                                      └──────────────┬───────────────┘   │
│                                                     │ invoke           │
│  拦截点：TerminalPane sendInput/paste   ┌────────────▼──────────────────┐ │
│  DockPanel SFTP 操作前断言              │  AppLockOverlay（z:1000）     │ │
│  Inspector metricsTarget 门控           └────────────┬──────────────────┘ │
│  lib/ai.ts aiExecTool 出口断言                       │                   │
└──────────────────────────────────────────────────────┼───────────────────┘
                                                       │ IPC（snake_case）
┌─────────────────────────────── 后端（Rust） ─────────▼───────────────────┐
│  src-tauri/src/lock.rs                                                  │
│  LockState(Mutex<限速表>)  lock.json 读写（原子写）  PBKDF2 哈希/常量时间比对 │
│  命令：lock_status / lock_set_password / lock_unlock / lock_clear_password │
│  注册：lib.rs invoke_handler + .manage(LockState)                         │
│  凭据文件：<app_config_dir>/lock.json（独立于 app_config.json，不导出）      │
└───────────────────────────────────────────────────────────────────────────┘
```

## 5. 详细设计

### 5.1 凭据文件 lock.json

路径：`app.path().app_config_dir()/lock.json`。形态：

```json
{
  "version": 1,
  "app": { "salt": "<base64,16B>", "hash": "<base64,32B>", "iters": 210000 }
}
```

- 只有一个 `app` 槽位；`null` 或缺失表示未设置密码。
- 哈希算法：`hash = PBKDF2-HMAC-SHA256(password, salt, iters)`，输出 32B；salt 16B，经 OS CSPRNG 生成，每次设置 / 修改密码重新生成。
- `iters` 固定 210000（与 `config_crypto.rs::KDF_ITERS` 同值）；读文件时夹取 ≤ 1000000 防篡改 DoS。
- 比对使用常量时间比较（逐字节异或累加）。
- 写文件：同目录 `lock.json.tmp` 写完 rename；密码只以 `&str` / `Zeroizing<String>` 存在于调用栈。
- 旧 lock.json 如有 `session` 字段（升级前遗留），反序列化时自然忽略（serde 默认忽略未知字段），无需迁移。

文件损坏处理：反序列化失败时将原文件改名为 `lock.json.corrupt-<unix_ms>` 隔离，按「应用锁未设置」继续运行，前端 toast「锁数据已损坏，请重新设置密码」。

### 5.2 IPC 契约

4 个命令，集中在 `src-tauri/src/lock.rs`。所有命令已无 `kind` 参数——只剩一个 app 槽位。

| 命令 | 入参 | 返回 | 说明 |
|---|---|---|---|
| `lock_status` | 无 | `{ appPasswordSet: bool, fileCorrupted: bool }` | 启动与设置面板刷新时调用 |
| `lock_set_password` | `password: string`, `oldPassword: string\|null` | `()` | 首次设置 `oldPassword=null`；已有密码时必须传正确旧密码 |
| `lock_unlock` | `password: string` | `()` | 验证 + 限速；失败 Err 透传中文文案 |
| `lock_clear_password` | `password: string` | `()` | 清除前必须验证当前密码 |

错误文案（后端中文直传）：

- 密码错误且未冷却：`密码错误，还可尝试 N 次`
- 冷却中：`尝试过于频繁，请在 N 秒后重试`
- 设置密码时旧密码错误：`原密码错误`
- 长度非法：`密码长度需为 6~128 个字符`

**限速状态机**（内存态，重启清零）：

```text
验证成功 → fails=0, until=None
每次失败 → fails+=1
fails 每跨过 5 的倍数 → until = now + min(30 * 2^(批次-1), 300) s
冷却未结束直接拒绝，不执行 KDF
```

冷却序列 30s、60s、120s、240s、300s 封顶。`lock_set_password` / `lock_clear_password` 的旧密码校验走同一限速通道。

### 5.3 设置项（settings 节 3 字段）

| 字段 | 类型 | 默认值 | 语义 / 夹取 |
|---|---|---|---|
| `appLockEnabled` | boolean | `false` | 总开关；置 true 前必须已设置应用锁密码，前端拒绝裸开。关闭仅停用锁屏触发，**不清除密码** |
| `appLockIdleSec` | number | `300` | 无操作自动锁屏秒数；取值必须 ∈ 白名单，脏值回退 `300` |
| `appLockLockOnBlur` | boolean | `false` | 窗口失焦即锁屏 |

`appLockIdleSec` 下拉档位（白名单 `[0, 60, 180, 300, 600, 1800]`）：

| 下拉选项 | 落盘值（秒） |
|---|---|
| 关闭 | `0` |
| 1 分钟 | `60` |
| 3 分钟 | `180` |
| 5 分钟（默认） | `300` |
| 10 分钟 | `600` |
| 30 分钟 | `1800` |

**设置面板「安全」分区内容**：

1. 应用锁总开关（开启时若未设密码引导去设置）
2. 闲置自动锁屏下拉（档位见上表）
3. 窗口失焦锁定开关
4. **应用锁密码管理**：`LockPasswordEditor` 行内表单（设置 / 修改 / 清除，清除需验证当前密码）
5. 底部固定提示：「锁密码仅保存在本机，忘记后需退出应用并删除 lock.json 重置。」

### 5.4 触发、状态机与遮罩

`frontend/src/stores/appLock.ts`：

```text
状态：appLocked = ref(false)（初值由 initAppLock() 设定）
      lockSlots = ref({ appPasswordSet: false })

动作：
  initAppLock(): 启动时按 appLockEnabled + lock_status 决定初始锁屏态
  lockApp(reason): 置 appLocked=true，夺走焦点，停止 AI 流
  unlockApp(pwd): 调 lockUnlockOrReset() → 成功 appLocked=false 并重置活动时刻
  refreshLockSlots(): 调 lock_status
  suppressBlurLock(): 菜单弹窗打开时临时屏蔽 blur 锁
```

`lockApp()` 必须完成：

1. `appLocked.value = true`，遮罩随响应式渲染。
2. 夺走键盘焦点：`(document.activeElement as HTMLElement)?.blur()` + `blurAllTerms()`。
3. 停止全部在飞 AI 流（`aiStopChat`）。
4. 监控静默：metricsTarget / 内存定时器门控统一读 `appLocked`。
5. 遮罩密码框 autofocus。

**闲置 / 失焦检测**（`App.vue`）：

- `document` capture 阶段监听 `pointerdown` / `keydown` / `wheel` 更新 `lastActivityAt`。
- 1s 定时器：超刻则 `lockApp('idle')`。
- `appWindow().onFocusChanged` 监听失焦：`appLockLockOnBlur=true` 时立即锁。

**AppLockOverlay.vue**（`App.vue` 根部，`position:fixed;inset:0;z-index:1000`）：

- 不透明深色底 + 应用 Logo + 锁图标 + 密码输入（回车提交、错误文案、冷却倒计时禁用）。
- 盖住标题栏以下全部网页内容；窗口仍可被用户拖动 / 关闭。
- 解锁成功后焦点还给锁定前的活跃元素。

**手动入口**：

| 位置 | 组件 |
|---|---|
| 工作台状态栏最左侧（`StatusBar.vue`） | 锁图标按钮 |
| 首页标题栏右侧（`TitleBar.vue`） | 同上 |
| 全局快捷键 `⌘/Ctrl+Shift+L` | `App.vue` document.keydown |

图标仅在 `appLockEnabled=true` 且密码槽位存在时渲染。

### 5.5 拦截矩阵

前端单点拦截，后端不引入锁概念：

| 操作路径 | 应用锁定时 |
|---|---|
| 终端键盘 `sendInput` | 阻断（焦点已夺走 + isInputLocked 断言） |
| 粘贴 `pasteWithGuard` | 阻断 |
| SFTP 操作（`DockPanel.vue`） | sftpLocked computed 门控 UI + 调用前断言 |
| AI 工具执行 `aiExecTool`（`lib/ai.ts`） | 直接拒绝，工具结果返回「应用已锁定，请先解锁」；在飞流 lockApp 时停止 |
| 远端 metrics 采集（`Inspector.vue`） | metricsTarget 返回 null → 全部停止 |
| 本地内存轮询（`StatusBar.vue`） | clearInterval，解锁后重建 |
| 切标签 / 关标签 | 遮罩拦截 |
| SSH 建连 / 自动重连 / 断开 | 不拦截 |
| 进行中的 SFTP 传输、端口转发 | 不中断 |

### 5.6 锁定期间的监控暂停

两级锁只**门控既有轮询**，不新增后端机制；恢复全部走 watcher / 状态机。

**远端 metrics（`Inspector.vue`）**：`metricsTarget` computed 追加 `appLocked` 条件返回 null；既有 watcher 随即释放 exec 通道、清心跳定时器。

**本地内存轮询（`StatusBar.vue`）**：同一 watcher 追加 `appLocked` 条件，进入锁定 clearInterval，解锁后重建。

### 5.7 重启与冷启动锁定

| 对象 | 持久化内容 | 重启后状态 |
|---|---|---|
| 锁密码 | `lock.json`（salt/hash/iters） | 保留 |
| 锁开关与参数 | settings 节 3 字段 | 保留 |
| 应用锁状态 | **不持久化** | `initAppLock()` 重新判定：`appLockEnabled=true` 且 `lock_status.appPasswordSet=true` → 启动即锁定 |

启动序列（`App.vue` onMounted，splash 撤下之前）：

```text
await loadAppConfig()
await initAppLock()                  // 决定初始锁屏态
…loadHosts / restoreSessions…        // 在遮罩后后台照常进行
__hideSplash()                       // 遮罩已 ready，splash 安全撤下
```

要点：

- 会话恢复在锁屏遮罩**后方**照常发生，解锁后立即可用。
- 闲置计时从**解锁成功后**才开始（避免密码框输入期间累计导致刚解锁又被锁）。
- `lock_status` IPC 失败时**fail-closed**：只要 `appLockEnabled=true` 就按锁定处理。

### 5.8 后端模块骨架

`src-tauri/src/lock.rs`：

```rust
pub(crate) struct LockState {
    fails: Mutex<HashMap<&'static str, FailState>>,
}

#[derive(Serialize, Deserialize)]
struct LockFile { version: u32, app: Option<HashEntry> }
#[derive(Serialize, Deserialize)]
struct HashEntry { salt: String, hash: String, iters: u32 }
```

对外只暴露 4 个 `#[tauri::command]`（§5.2）与 `lock_path(app)`；哈希、读写、限速为模块私有函数。

## 6. 安全与降级

**威胁模型**：

| 威胁 | 是否防御 | 说明 |
|---|---|---|
| 使用者离开座位，旁人操作 / 观看 | 是 | 应用锁全屏遮罩 |
| 拿到热机但不知道锁密码 | 是 | 密码错误有冷却限速；无应用内绕过入口 |
| 重启应用绕过锁屏 | 是 | 启用后冷启动即锁定；`lock_status` 查询失败 fail-closed |
| 锁定期间监控数据外泄 | 是 | 锁定即停远端 metrics 与本地内存轮询 |
| 配置导出文件泄露 | 是 | `lock.json` 是独立文件，不被 `config_io.rs` 读取 |
| 本机恶意软件、进程内存 / 调试器读取 | 否 | 桌面应用无法防御 |
| 物理取走磁盘离线分析 | 部分 | 210000 轮 PBKDF2；主机密码仍在系统钥匙串 |
| 删除 `lock.json` 关锁 | 接受 | 本机攻击者本就可删应用数据 |

**安全红线**：

- 密码明文只在 IPC 参数与 KDF 调用栈存在，不落盘、不进 applog。
- 前端永远拿不到 salt/hash，验证只能经 `lock_unlock` 走后端限速通道。
- 遮罩必须**不透明**，禁止毛玻璃透出终端文字。
- 锁定必须移走终端焦点，不能只靠视觉遮挡。
- `lock.json` 不增任何导入 / 导出 / 备份代码路径。

**忘记密码**：退出应用后删除 `lock.json`（macOS 路径 `~/Library/Application Support/com.rhost.app/lock.json`），重启后锁未启用；`connections.json` 与钥匙串不受影响。

**降级**：

- `lock.json` 损坏：隔离 + 视为未设置 + toast 告警。
- 后端 IPC 异常：遮罩保持锁定并显示「解锁服务异常，点击重试」，**不提供前端绕过**。
- 未设置密码时：所有锁屏入口隐藏 / 不响应，闲置 watcher 不启动。
- `appLockEnabled=true` 但密码槽位缺失（`lock.json` 被删）：`initAppLock()` 不锁定、toast 告警并把开关写回 false。
- 系统时间被回拨：限速依赖单调时钟 `Instant`，闲置判定依赖 `Date.now()`，回拨最坏导致一次提前 / 延后锁屏。

## 7. 测试与验证

- **Rust 单测**（`lock.rs` 内 `#[cfg(test)]`）：
  - 设置后正确密码验证成功、错误密码失败；错误文案含剩余次数；
  - 两次设置生成的 salt 不同；hash 不等于密码任意片段的编码；
  - 常量时间比较：等长 / 不等长输入均返回正确布尔；
  - 限速：连续第 5 次失败后进入 30s 冷却，冷却期不执行 KDF 直接拒绝；批次翻倍序列正确、300s 封顶；成功验证清零；
  - `lock.json` 写后重读结构往返；损坏文件被改名隔离且按未设置处理；
  - `iters` 篡改为 2000000 时按损坏处理；
  - 空槽位（lock.json 被删）验证路径：`lockUnlockOrReset` 返回 `slot_missing`，`unlockApp` 据此直接解除锁定。
- **手工验证**：
  - 状态栏最左侧 + 首页标题栏锁图标可锁屏，态随锁定翻转；
  - 锁屏后键盘狂敲 / 粘贴，远端无任何输入到达；解锁后恢复；
  - 锁屏期间 SSH 不掉线，解锁后滚动缓冲完整；
  - 闲置下拉各档位与失焦即锁开关均生效；手改配置为白名单外值，重启回退 300；
  - 锁定后 5s 内 Inspector 无 metrics 网络请求、状态栏内存停刷；解锁后自动恢复；
  - 关闭应用锁开关后密码仍保留，重新开启直接生效、无需重设；
  - 清除密码须单独验证并通过密码编辑器操作；
  - 冷启动：启用应用锁后退出重启，splash 撤下即见锁屏；未启用时行为不变；
  - 删除 `lock.json` 后重启出现告警 toast 且开关回退为关；
  - 故意写坏 `lock.json` 重启，出现隔离文件与告警 toast；
  - 全量导出配置后搜索不到锁相关字段。
- **e2e**：不新增（现有仅覆盖 SSH 链路）。

## 8. 未决问题

- 生物识别（Touch ID / Windows Hello）：后续在验证原语上叠加即可。
- 锁屏时是否启用系统窗口模糊（macOS 调度中心缩略图）：待评估 Tauri 窗口级 API 后单独立项。
- 锁定快捷键暂为固定 `⌘/Ctrl+Shift+L`，待项目全局快捷键体系落地时统一接入。

## 9. 备注：会话锁移除说明（2026-11-19）

会话锁（`Session.locked` + `SessionLockOverlay` + 独立密码槽位 + 总开关）已于本日从代码中移除。移除原因：

1. **场景 niche**：SSH 终端领域（iTerm2、Terminal、PuTTY、Xshell）几乎没有会话级锁，主流做法就是应用级 master password。
2. **遮罩视觉突兀**：会话锁定后遮罩只盖终端区，右侧 AI 面板和底部 SFTP 仍亮着，与「锁定即静默」心智模型冲突；改成覆盖全工作区则与应用锁遮罩边界模糊。
3. **复杂度与收益不对等**：会话锁独有的 8+ 安全/死锁/门控问题（广播绕过、`lock.json` 丢失永久死锁、SFTP 23 处手动门控、Inspector metrics 门控 watcher 等），每新增一个面板都要在门控矩阵上加一行。
4. **两级密码/开关心智负担**：开关与密码解耦（关闭开关不清密码）的 UX 修正已让应用锁足够直觉，再加一级只会让用户困惑。

移除后的设计：应用锁是唯一的锁，全屏遮罩，单一密码槽位，一个总开关。相关改动：

- 删除前端组件 `SessionLockOverlay.vue`；
- `Session` 接口移除 `locked` 字段，`session.ts` 移除 `lockSession/unlockSession/lockAllSessions/unlockAllSessions/sessionLockReady/sessionLockKind/focusSessionLock/sessionLockFocusReq` 全部会话锁导出；
- `settings.ts` / persisted.rs 移除 `sessionLockEnabled` / `sessionLockUseAppPassword`；
- `lock.rs` 移除 session 槽位、`LockKind` 枚举、所有命令的 kind 参数，简化为单 app 槽位；
- `DockPanel.sftpLocked` / `Inspector.metricsTarget` / `AiChatPanel.activeSessionLocked` 门控简化为仅检查 `appLocked`。
