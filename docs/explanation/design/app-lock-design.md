# 应用锁与会话锁设计方案

> description: 应用级锁屏与会话级遮罩冻结两级锁机制的凭据存储、IPC、状态机与交互设计
>
> created: 2026-10-16 12:18:38
>
> updated: 2026-10-17 00:02:02
>
> author:

---

## 1. 设计目标与硬约束

> 范围：本文覆盖两级锁——**应用级锁**（手动 / 闲置 / 失焦 / 冷启动触发的全屏锁屏）与**会话级锁**（单个 SSH 会话的画面遮罩 + 输入冻结），包含凭据存储、IPC 契约、前端状态机、拦截点与设置面板。
> 边界：不做生物识别（Touch ID / Windows Hello）；不做每个会话各自独立的密码（会话锁共用一个「会话锁密码」，或复用应用锁密码，理由见 §6.5）；不改变 SSH 连接生命周期，锁定不断连；不做锁定后窗口最小化 / 系统缩略图模糊。
> 关联代码：`src-tauri/src/config_crypto.rs`（KDF 参数既有约定）、`src-tauri/src/applog/persisted.rs`（`SettingsSection` 与原子写）、`src-tauri/src/config_io.rs`（导出组装范围）、`src-tauri/src/ipc.rs`、`src-tauri/src/lib.rs`、`frontend/src/stores/settings.ts`、`frontend/src/stores/session.ts`、`frontend/src/App.vue`、`frontend/src/components/wb/StatusBar.vue`、`frontend/src/components/wb/WbTabs.vue`、`frontend/src/components/wb/Inspector.vue`、`frontend/src/components/wb/TerminalPane.vue`、`frontend/src/components/SettingsModal.vue`、`frontend/src/lib/ai.ts`、`frontend/src/lib/termRegistry.ts`。

| 目标 | 量化口径 |
|---|---|
| 解锁验证快 | PBKDF2-SHA256 210000 轮，主流机型单次验证 < 300ms |
| 锁屏响应快 | 触发到遮罩可输入 ≤ 100ms；闲置检测 tick 间隔 1s |
| 锁定不断连 | 锁定 / 解锁动作产生 0 次 SSH 断连，已建立会话、转发、进行中的 SFTP 传输不受影响 |
| 锁定即静默 | 锁定后 1 个 tick（≤ 5s）内停止全部远端 metrics 采集与心跳；本地内存轮询即时停摆，解锁后自动恢复 |
| 重启不可绕过 | 已启用应用锁时，冷启动 splash 撤下即呈现锁屏，不输入密码无法进行任何交互 |
| 防爆破 | 同一类锁连续失败 5 次进入冷却，30s 起指数翻倍、封顶 300s |
| 凭据文件极小 | `lock.json` < 300 字节，仅含 salt/hash/iters，不含任何主机信息 |
| 零明文泄露 | 密码不落盘、不入日志、不进配置导入导出；前端任何时刻拿不到 hash |

## 2. 必须遵守的既有项目约束

- **敏感凭据只进系统钥匙串或加盐哈希，不进 `app_config.json`**：主机密码走 Keychain / Credential Manager（`src-tauri/src/store.rs`）；锁密码不可恢复，采用 PBKDF2 哈希存独立文件，二者都不进 `settings` 节。
- **KDF 参数与配置加密导出保持一致**：PBKDF2-HMAC-SHA256、210000 轮、16B salt（`src-tauri/src/config_crypto.rs` 中 `KDF_ITERS`），不另造参数。
- **设置项双份对齐**：新增 settings 字段必须同时改前端 `AppSettings` + `DEFAULT_SETTINGS`（`frontend/src/stores/settings.ts`）与后端 `SettingsSection` + `Default`（`src-tauri/src/applog/persisted.rs`），写前强类型校验，两端不得漂移。
- **文件原子写**：临时文件 + rename（见 `store.rs::write_hosts`）。
- **IPC 命名 snake_case，错误中文文案透传**（与 `store.rs`、`ipc.rs` 现有风格一致）。
- **密码 / PTY 字节流永不入 applog**；日志只记事件类别与布尔结果。
- **AI 写操作有统一裁决路径**：会话锁对 AI 的拦截必须落在 `aiExecTool` 调用前的单点，不在各工具卡片内分散判断。

## 3. 现状盘点与差距

| 位置 | 现状 | 差距 |
|---|---|---|
| `src-tauri/src/config_crypto.rs` | 已有 PBKDF2-SHA256 + AES-GCM 封装与 210000 轮常量 | 无「只哈希校验」场景的封装；锁功能复用其参数约定，新增 `lock.rs` |
| `src-tauri/src/applog/persisted.rs` | `app_config.json` 分节强类型读写、原子写 | 无锁凭据节；锁凭据**不进**此文件，新建 `lock.json` |
| `src-tauri/src/config_io.rs` | 导出仅组装 `app_config.json` 各节 + `connections.json` | 独立文件天然不导出，无需改导出代码，补测试断言即可 |
| `frontend/src/stores/session.ts` | `Session` 有 `state`/`alias`/`resetSeq` 等运行时字段 | 无 `locked` 字段与锁定 / 解锁 action |
| `TerminalPane.vue` | 键盘输入单点 `sendInput`（L303）、粘贴单点 `pasteWithGuard` | 两处需加锁态断言；xterm 锁定时需 `blur()` 防焦点残留收键 |
| `frontend/src/lib/ai.ts` | `aiExecTool(sessionId, ...)` 是 AI 工具执行唯一 IPC 出口（L251） | 需在此出口前按 scope 检查会话锁；应用锁锁定时停止 AI 流 |
| `WbTabs.vue` | 标签结构为 `dot + name/rename-input + close`，右键菜单含重命名 / 重连 / 关闭等项 | 缺锁图标位与「锁定会话 / 解锁会话」「锁定全部会话」入口 |
| `StatusBar.vue` | `<footer class="statusbar">` 首元素为连接状态；内含 `get_app_memory` 本地内存轮询定时器（受 `memPauseHidden` 门控） | 最左侧缺应用锁图标；内存定时器缺锁定门控 |
| `Inspector.vue` | `metricsTarget` computed + `reconcileMetrics` 状态机统一门控远端 metrics 启停与 5s 心跳，target 为 null 即停 | 门控条件缺「应用锁定 / 激活会话被锁」两个判定 |
| `App.vue` | 启动序列：`loadAppConfig` → `loadHosts` → `restoreSessions`，splash 取晚撤下 | 缺启动锁定初始化，需在 splash 撤下前确定初始锁屏态 |
| `SettingsModal.vue` | 分区：外观 / 终端 / 快捷键 / 高级 / 监控 / SFTP / 转发 / AI / 关于 | 缺「安全」分区承载两级锁配置 |
| 全局 | 弹窗遮罩 `z-index:60`、toast `z-index:90`（`style.css`） | 应用锁遮罩取 `z-index:1000` 盖住一切 |

结论：本设计补齐独立凭据文件与后端验证原语、两级锁的前端状态机与遮罩、输入 / 粘贴 / SFTP / AI 拦截点、标签与状态栏锁图标、监控轮询锁定门控、冷启动锁定与闲置检测，以及「安全」设置分区。

## 4. 目标与非目标

### 目标

1. 用户可设置 / 修改 / 清除独立的**应用锁密码**；启用后可手动锁屏，可开启闲置 N 分钟自动锁屏、窗口失焦锁屏。
2. 已启用应用锁时**冷启动即锁定**：splash 撤下后必须先解锁，重启不能绕过锁屏（§6.7）。
3. 用户可设置独立的**会话锁密码**；也可勾选「会话锁使用应用锁密码」（默认关闭）复用应用锁密码。
4. 会话锁定后：终端画面被**不透明遮罩**覆盖，键盘输入、粘贴、以该会话发起的 SFTP 操作与 AI 工具调用全部冻结；SSH 连接保持、远端输出持续进入滚动缓冲，解锁后完整可见。
5. 两级锁锁定期间暂停对应范围的监控轮询（远端 metrics + 本地内存），解锁后经既有门控状态机自动恢复（§6.6）。
6. 标签页与状态栏提供常驻锁图标，上锁一次点击完成；连续解锁失败有冷却限速；忘记密码有明确的本机重置路径。

### 非目标

- 不做生物识别解锁（后续可在验证原语上叠加，不影响本设计存储结构）。
- 不做按会话各自不同的密码；会话锁是全局一个密码（单密码与多密码在同一热机威胁模型下强度相同，且多密码与运行时态会话、批量上锁主路径冲突）。
- 不做锁定后隐藏 / 最小化窗口（macOS 调度中心缩略图模糊留待后续，见 §9）。
- 不防御能读取进程内存或文件系统的本机攻击者（威胁模型见 §7）。
- 锁定期间不中断 SSH 连接、进行中的 SFTP 传输与端口转发（这些是用户已授权动作，暂停新轮询即可）。

## 5. 总体架构

```text
┌─────────────────────────────── 前端（Vue） ───────────────────────────────┐
│                                                                          │
│  SettingsModal「安全」分区          StatusBar 最左侧 / 首页 TitleBar 锁图标 │
│  设置/修改/清除密码、闲置下拉、       WbTabs 每个标签最前方锁图标（两态）    │
│  复用选项                  persist   快捷键 ⌘/Ctrl+Shift+L               │
│         │ settings 节                    │ 手动锁定 / 点击上锁            │
│         ▼                                ▼                                │
│  stores/settings.ts                 stores/appLock.ts（新建，模块级单例）  │
│  appLockEnabled / idleSec /          ┌────────────────────────────────┐   │
│  lockOnBlur / sessionFollowApp      │ appLocked:ref（含冷启动初值）    │   │
│                                     │ initAppLock() 闲置/失焦 watcher │   │
│  stores/session.ts                   │ lockApp() unlockApp(pwd)        │   │
│  Session.locked + actions           │ lockStatus（密码是否已设置）     │   │
│         ▲                           └──────────────┬─────────────────┘   │
│         │                                            │ invoke             │
│  拦截点：TerminalPane sendInput/paste   ┌────────────▼──────────────────┐  │
│  DockPanel SFTP 操作前断言              │  AppLockOverlay（z:1000）     │  │
│  Inspector metricsTarget 门控           │  SessionLockOverlay（终端区内）│  │
│  StatusBar 内存定时器门控               └────────────┬──────────────────┘  │
│  lib/ai.ts aiExecTool 出口断言                       │                     │
└──────────────────────────────────────────────────────┼─────────────────────┘
                                                        │ IPC（snake_case）
┌─────────────────────────────── 后端（Rust） ──────────▼───────────────────┐
│  src-tauri/src/lock.rs（新建）                                            │
│  LockState(Mutex<限速表>)  lock.json 读写（原子写）  PBKDF2 哈希/常量时间比对 │
│  命令：lock_status / lock_set_password / lock_unlock / lock_clear_password │
│  注册：lib.rs invoke_handler + .manage(LockState)                         │
│  凭据文件：<app_config_dir>/lock.json（独立于 app_config.json，不导出）      │
└───────────────────────────────────────────────────────────────────────────┘
```

两级锁关系：应用锁是**全局交互闸门**（锁定后整个窗口只剩密码框可交互）；会话锁是**单会话冻结态**（运行时态，重启即清空）。会话锁的验证原语与应用锁完全相同，仅 hash 槽位不同；勾选复用时前端直接以 `kind="app"` 请求验证，后端不感知「复用」语义。

## 6. 详细设计

### 6.1 凭据文件 lock.json

路径：`app.path().app_config_dir()/lock.json`（与 `app_config.json` 同目录）。形态：

```json
{
  "version": 1,
  "app": { "salt": "<base64,16B>", "hash": "<base64,32B>", "iters": 210000 },
  "session": null
}
```

- `app` / `session` 槽位为 `null` 或缺失表示该类锁**未设置密码**（功能不可用）；设置后为记录。
- 哈希算法：`hash = PBKDF2-HMAC-SHA256(password, salt, iters)`，输出 32B；salt 16B，经 OS CSPRNG 生成，每次设置 / 修改密码重新生成。
- `iters` 固定 210000（与 `config_crypto.rs::KDF_ITERS` 同值，随其一起演进）；读文件时夹取 ≤ 1000000 防篡改 DoS，超限按文件损坏处理。
- 比对使用常量时间比较（`subtle` 式逐字节异或累加，不引入新依赖，手写 8 行即可）。
- 写文件：同目录 `lock.json.tmp` 写完 rename；文件权限继承 umask（桌面平台不另设，因内容为加盐哈希）。
- 密码只以 `&str` / `Zeroizing<String>` 存在于命令调用栈，函数结束即清零；**hash / salt / 文件内容不提供任何返回前端的 IPC**。

文件损坏处理：反序列化失败时将原文件改名为 `lock.json.corrupt-<unix_ms>` 隔离，按「两级锁均未设置」继续运行，同时写 `warn` 级 applog（原因码 `lock_store_corrupted`），前端启动后 toast「锁数据已损坏，应用锁已关闭，请重新设置密码」。此取舍与 `app_config.json` 损坏回退默认值的既有哲学一致：不能让安全配置文件损坏导致应用不可用。

### 6.2 IPC 契约

新增 4 个命令，集中在 `src-tauri/src/lock.rs`，在 `lib.rs` 注册并 `.manage(LockState::default())`。

| 命令 | 入参 | 返回 | 说明 |
|---|---|---|---|
| `lock_status` | 无 | `{ appPasswordSet: bool, sessionPasswordSet: bool }` | 启动与设置面板刷新时调用，只给布尔，不给任何哈希材料 |
| `lock_set_password` | `kind: "app"\|"session"`, `password: string`, `oldPassword: string\|null` | `()` | 首次设置 `oldPassword=null`；槽位已有密码时必须传正确旧密码，否则报错。密码长度 6~128（字符数，前后端双校验） |
| `lock_unlock` | `kind`, `password` | `()` | 验证 + 限速；失败 Err 透传中文文案 |
| `lock_clear_password` | `kind`, `password` | `()` | 清除槽位前必须验证当前密码；应用锁密码被清除时，前端联动把 `appLockEnabled` 置 false |

错误文案（后端直接拼中文，前端原样 toast / 行内展示）：

- 密码错误且未冷却：`密码错误，还可尝试 N 次`（N 从 5 递减）。
- 冷却中：`尝试过于频繁，请在 N 秒后重试`。
- 设置密码时旧密码错误：`原密码错误`。
- 长度非法：`密码长度需为 6~128 个字符`。

**限速状态机**（内存态，`HashMap<Kind, FailState>`，重启清零）：

```text
验证成功 → fails=0, until=None
每次失败 → fails+=1
fails 每跨过 5 的倍数 → until = now + min(30 * 2^(批次-1), 300) s
冷却未结束直接拒绝，不执行 KDF（同时省 CPU）
```

即冷却序列 30s、60s、120s、240s、300s 封顶。`lock_set_password` / `lock_clear_password` 中的旧密码校验走同一验证函数，同受限速约束。

applog 只记：`lock.verify`，kv `{ kind: "app"|"session", ok: bool, backoff: bool }`；**禁止**记录密码、密码长度以外内容（长度也不记）。

### 6.3 设置项（settings 节新增 4 字段）

| 字段 | 类型 | 默认值 | 语义 / 夹取 |
|---|---|---|---|
| `appLockEnabled` | boolean | `false` | 应用锁总开关；置 true 前必须已设置应用锁密码，前端拒绝裸开 |
| `appLockIdleSec` | number | `300` | 无操作自动锁屏秒数；下拉固定档位，取值必须 ∈ 白名单，手改脏值回退 `300` |
| `appLockLockOnBlur` | boolean | `false` | 窗口失焦即锁屏（切其他应用 / 点击桌面） |
| `sessionLockUseAppPassword` | boolean | `false` | true 时会话锁用应用锁密码验证；开启前提是应用锁密码已设置 |

`appLockIdleSec` 下拉档位（设置面板渲染为 select，不接受自由输入）：

| 下拉选项 | 落盘值（秒） |
|---|---|
| 关闭 | `0` |
| 1 分钟 | `60` |
| 3 分钟 | `180` |
| 5 分钟（默认） | `300` |
| 10 分钟 | `600` |
| 30 分钟 | `1800` |

白名单常量前端（`stores/settings.ts` 旁）与消费侧各持一份 `[0, 60, 180, 300, 600, 1800]`，hydrate 时不在表内的值替换为 `300`。

同步改动：`AppSettings` 接口、`DEFAULT_SETTINGS`、后端 `SettingsSection` 字段与 `Default`（四处缺一不可，字段名与 §6.3 表格一致）。

设置面板新增「**安全**」分区（`SettingsModal.vue` 分区数组，位于「终端」之后），内容：

1. 应用锁卡片：开关（开启时若未设密码先展开「新密码 + 确认密码」行内表单）；「修改密码」（旧密码 + 新密码 + 确认）；闲置自动锁屏下拉（档位见上表，选「关闭」时展示一句说明文字）；失焦锁定开关；关闭开关时弹密码确认后调 `lock_clear_password`。
2. 会话锁卡片：「设置 / 修改会话锁密码」行内表单；`sessionLockUseAppPassword` 复选框（勾选后隐藏独立密码表单；取消勾选若独立密码槽位为空，菜单入口引导回此面板设置）。
3. 底部一行固定提示：「锁密码仅保存在本机，忘记后需退出应用并删除 lock.json 重置。」

### 6.4 应用锁：触发、状态机与遮罩

新建 `frontend/src/stores/appLock.ts`（模块级单例，风格同 `showSettings`）：

```text
状态：appLocked = ref(false)（初值由 initAppLock() 在启动序列中设定，见 §6.7）
动作：
  initAppLock(): 启动时按 appLockEnabled + lock_status 决定初始锁屏态
  lockApp(reason: 'manual'|'idle'|'blur'|'startup')
  unlockApp(pwd): 调 lock_unlock('app') → 成功 appLocked=false 并重置活动时刻
  refreshLockStatus(): 调 lock_status
```

`lockApp()` 必须完成的动作（≤ 100ms 内）：

1. `appLocked.value = true`，遮罩随响应式渲染。
2. **夺走键盘焦点**：`(document.activeElement as HTMLElement)?.blur()`，并经 `termRegistry.ts` 对所有 xterm 实例调 `term.blur()`——防止遮罩盖住后焦点仍在终端、按键继续走 `onData`。
3. 停止全部在飞 AI 流：对每个 streaming scope 走既有「停止生成」路径（`ai_stop_chat`，语义同手动停止）；未裁决提案保持 pending，不自动执行。
4. 监控静默：远端 metrics 与本地内存轮询的门控统一读 `appLocked`，状态翻转后自动停摆（§6.6），本函数不直接操作定时器。
5. 遮罩密码框 `autofocus`（`reason='startup'` 时同样在 splash 撤下后聚焦）。

**闲置 / 失焦检测**（`appLockEnabled=true` 时在 `App.vue` 启动一次）：

- `document` capture 阶段监听 `pointerdown` / `keydown` / `wheel`，事件发生即更新 `lastActivityAt = Date.now()`；`appLocked=true` 期间不更新、不计时。
- 1s `setInterval` 检查：未锁定且 `appLockIdleSec>0` 且超刻则 `lockApp('idle')`。
- 经 `lib/tauri.ts` 的 `appWindow().onFocusChanged` 监听失焦：`appLockLockOnBlur=true` 时失焦立即锁；为 false 时不特殊处理（失焦时长自然计入闲置）。
- 重新聚焦时补一次 `Date.now() - lastActivityAt` 判定，双保险——窗口后台时 WebView 定时器会被节流。
- 在遮罩密码框中的输入不刷新 `lastActivityAt`（锁定态本来就暂停计时，解锁成功那一刻重置）。

**AppLockOverlay.vue**（`App.vue` 根部、所有弹窗之后挂载）：

- `position:fixed;inset:0;z-index:1000`，**不透明**深色底（不用毛玻璃，避免透出文字）+ 应用 Logo + 锁形图标 + 单一密码输入（密码框、回车提交、错误文案、冷却倒计时禁用输入）。
- 视觉上盖住标题栏、原生红绿灯下方的全部网页内容；窗口仍可被用户拖动 / 关闭（macOS 红绿灯可用），不限制 OS 级窗口操作。
- 解锁验证期间输入框禁用并显示 loading；成功后焦点还给锁定前的活跃元素（记录 `activeElement`，终端则还需 `term.focus()` 回原会话）。

**手动入口（常驻图标，两态）**：

| 位置 | 组件 | 行为 |
|---|---|---|
| 工作台底部状态栏**最左侧开头**（`StatusBar.vue` 的 `<footer>` 首元素，先于连接状态） | 锁图标按钮 | 未锁态：开锁图标，点击 `lockApp('manual')`；已锁态：闭合锁图标（强调色），但此时被遮罩覆盖不可点，解锁瞬间可见态切换 |
| 首页标题栏右侧操作区（`TitleBar.vue`，首页无状态栏，复用同一图标组件） | 同上 | 同上；首页锁定后遮罩文案与工作台一致 |

- 图标仅在 `appLockEnabled=true` 且应用锁密码槽位存在时渲染；否则不占位。
- 未锁态默认 60% 不透明度、hover 全显（降低视觉噪音）；已锁态强调色常显。
- 全局快捷键 `⌘+Shift+L`（macOS）/ `Ctrl+Shift+L`（非 macOS），在 `App.vue` 注册一次 `document.keydown`，设置面板打开时不触发；快捷键本期不进 settings 可配置项（现有快捷键体系整体标注 TODO，待统一落地）。

### 6.5 会话锁：运行时态、遮罩与拦截

**状态**：`Session` 接口新增 `locked: boolean`（默认 false）。纯运行时态，不写 `ui_state`、不参与热启动恢复；关闭标签随 Session 移除而解除，断线重连（同一 Session 对象）保持锁定。

**action**（放 `session.ts`）：`lockSession(id)` / `unlockSession(id, pwd)`。`lockSession` 内：置位 → 该会话若有焦点则 `term.blur()` → 取消该会话在飞 AI exec（复用按会话取消的 `ai_cancel_execs` 既有路径）。`unlockSession`：按 `sessionLockUseAppPassword` 决定 `kind='app'` 还是 `'session'`，调 `lock_unlock`；成功清位并 `term.focus()`。

**入口（以标签锁图标为主）**：

- `WbTabs.vue` 每个标签**最前方**（`.dot` 之前）新增锁图标按钮 `.tab-lock`，区分两态：
  - **未锁态**：开锁小图标，平时低透明度、hover 全显；点击即 `lockSession(s.id)`，**上锁不验证密码**（仅解锁需密码），一次点击完成。
  - **已锁态**：闭合锁图标，强调色常显；点击时若该标签非激活先切为激活，再聚焦其 `SessionLockOverlay` 密码框（密码只在遮罩内输入，菜单 / 图标均不收密码）。
  - 图标与关闭钮一致用 `@click.stop` / `@mousedown.stop`，不触发标签切换与拖拽滚动。
- 标签右键菜单保留「锁定会话 / 解锁会话」作为辅助入口；标签栏空白区菜单新增「锁定全部会话」（对所有在线会话批量置位）。
- 未设置任何会话锁可用凭据（独立密码未设且未勾选复用）时点击图标，toast「请先在设置 → 安全中设置会话锁密码」并打开设置面板安全分区。

**SessionLockOverlay.vue**：挂在 `TerminalPane.vue` 终端容器内（`position:absolute;inset:0`，仅盖终端区，不盖标签栏与状态栏），不透明深色底 + 锁图标 +「此会话已锁定」+ 密码输入条（回车验证、错误文案复用 §6.2）。离线 / 重连浮层与锁遮罩并存时锁遮罩在上层。

**拦截矩阵**（前端单点拦截，后端不引入锁概念）：

| 操作路径 | 应用锁定时 | 该会话锁定时 |
|---|---|---|
| 终端键盘 `sendInput`（`TerminalPane.vue` L303） | 阻断（焦点已夺走，断言兜底） | 阻断 |
| 粘贴 `pasteWithGuard` | 阻断 | 阻断 |
| SFTP 新建传输 / 文件管理操作（`DockPanel.vue` 各 `sftp_*` 调用前） | 遮罩已挡住全部 UI | 激活会话锁定时按钮禁用 + 调用前断言，toast 提示先解锁 |
| AI 工具执行 `aiExecTool`（`lib/ai.ts` L251） | 锁定瞬间停止全部流（§6.4） | 该 scope 调用直接拒绝，工具结果返回「会话已锁定，请先解锁」；在飞 exec 取消 |
| 远端 metrics 采集（`Inspector.vue` 门控） | 全部停止（target=null） | 激活会话被锁时停止；切到未锁会话自动恢复（§6.6） |
| 本地内存轮询（`StatusBar.vue` 定时器） | 停止，解锁后重启（§6.6） | 不停止（应用仍可操作，仅单会话冻结） |
| 切标签 / 关标签 | 不可操作（遮罩拦截） | 允许；关闭即解除该会话锁 |
| SSH 建连 / 自动重连 / 断开 | 不拦截 | 不拦截 |
| 进行中的 SFTP 传输、端口转发 | 不中断（已授权通道继续） | 不中断 |

### 6.6 锁定期间的监控暂停

两级锁只**门控既有轮询**，不新增后端机制；恢复全部走已有的 watcher / 状态机，无手动重启路径。

**远端 metrics（`Inspector.vue`）**：在现有 `metricsTarget` computed 的返回条件中追加两个判定——`appLocked === true` 或「激活会话 `locked === true`」时返回 `null`。既有 `watch(metricsTarget, reconcileMetrics, { immediate: true })` 随即收敛为：`stop_metrics` 释放 exec 通道、清 5s 心跳定时器（后端任务 9s TTL 后自然消亡，无需特殊处理）。门控解除后 target 重新求值、watcher 自动 `start_metrics` + 心跳。切标签时按激活会话的锁定态自然切换，无需额外事件。

**本地内存轮询（`StatusBar.vue`）**：现有定时器受 `memPauseHidden` 门控。在同一处 watcher 追加 `appLocked` 条件：进入锁定即 `clearInterval(memTimer)`，解锁后按 `memInterval` 重建。会话锁不影响该定时器（应用其余部分仍可正常操作）。

**不暂停的东西**（显式列举，避免实现误伤）：SSH 主连接与 RTT 心跳、自动重连退避、进行中的 SFTP 传输、端口转发通道、applog 采集。暂停的只是「周期性发起的只读采集」，锁定期间指标面板无新数据、解锁后从当前时刻继续（历史曲线不回填锁定时段）。

### 6.7 重启与冷启动锁定

| 对象 | 持久化内容 | 重启后状态 |
|---|---|---|
| 两级锁密码 | `lock.json`（salt/hash/iters） | 保留，无需重设 |
| 锁开关与参数 | settings 节 4 字段 | 保留 |
| 应用锁状态 | **不持久化**（内存态） | 由 `initAppLock()` 重新判定：`appLockEnabled=true` 且 `lock_status.appPasswordSet=true` → 启动即锁定 |
| 会话锁状态 | **不持久化**（内存态） | 恢复出的会话 `locked` 一律 false；会话标签本身经 `ui_state.sessions` 热启动恢复，但锁态不跟随 |

启动序列改造（`App.vue` onMounted，splash 撤下之前）：

```text
await loadAppConfig()                 // 既有：hydrate settings（含 appLockEnabled）
await initAppLock()                   // 新增：invoke lock_status
                                      //  enabled && appPasswordSet → appLocked=true('startup')
                                      //  enabled 但密码槽位缺失（lock.json 被删/损坏）
                                      //    → 不锁，toast 告警，并把 appLockEnabled 写回 false
…loadHosts / restoreSessions（既有，遮罩下后台照常进行，不等待解锁）…
既有 splash 取晚逻辑 → __hideSplash()
```

要点：

- 会话恢复与自动重连在锁屏遮罩**后方照常发生**，解锁后立即可用，不让解锁等连接；遮罩不透明，恢复过程中的主机名 / MOTD / 终端输出均不可见。
- 闲置计时从**解锁成功后**才开始（`lastActivityAt` 初始化为解锁时刻），避免「启动后闲置计时在密码框输入期间累计导致刚解锁又被锁」。
- `lock_status` IPC 失败（后端异常）时**fail-closed**：只要 `appLockEnabled=true` 就按锁定处理，遮罩上解锁请求会持续重试 IPC 并提示，不能因状态查询失败而敞开应用。

### 6.8 后端模块骨架

新建 `src-tauri/src/lock.rs`，`lib.rs` 增加 `mod lock;` 与命令注册：

```rust
pub(crate) struct LockState {
    /// 限速计数：kind → (连续失败次数, 冷却结束时刻)
    fails: Mutex<HashMap<String, (u32, Option<Instant>)>>,
}

#[derive(Serialize, Deserialize)]
struct LockFile { version: u32, app: Option<HashEntry>, session: Option<HashEntry> }
#[derive(Serialize, Deserialize)]
struct HashEntry { salt: String, hash: String, iters: u32 } // 均 base64 / 数值
```

对外只暴露 4 个 `#[tauri::command]`（§6.2）与 `lock_path(app)`；哈希、读写、限速为模块私有函数。密码参数类型用 `Zeroizing<String>`，沿用 `config_crypto.rs` 的清零约定。

## 7. 安全与降级

**威胁模型（显式边界）**：

| 威胁 | 是否防御 | 说明 |
|---|---|---|
| 使用者离开座位，旁人操作 / 观看 | 是 | 应用锁全屏遮罩；会话锁遮罩盖终端内容并冻结操作 |
| 拿到热机但不知道锁密码 | 是 | 密码错误有冷却限速；无应用内绕过入口 |
| 重启应用绕过锁屏 | 是 | 启用后冷启动即锁定（§6.7）；`lock_status` 查询失败 fail-closed |
| 锁定期间监控数据外泄 | 是 | 锁定即停远端 metrics 采集与本地内存轮询（§6.6） |
| 配置导出文件泄露 | 是 | `lock.json` 是独立文件，`config_io.rs` 组装导出时根本不读取它 |
| 本机恶意软件、进程内存 / 调试器读取 | 否 | 桌面应用无法防御，不在本设计目标内（release 已禁 DevTools 与默认快捷键） |
| 物理取走磁盘离线分析 | 部分 | 锁密码是 210000 轮 PBKDF2 哈希；主机密码仍在系统钥匙串（OS 级加密） |
| 删除 / 篡改 `lock.json` 关锁 | 接受 | 本机攻击者本就可删应用数据；删除关锁不会泄露任何会话密码，主机配置不受益于此攻击 |

**安全红线**：

- 密码明文只在 IPC 参数与 KDF 调用栈存在，不落盘、不进 applog、不进错误文案；日志仅记 kind 与成败布尔。
- 前端永远拿不到 salt/hash，验证只能经 `lock_unlock` 走后端限速通道。
- 两级遮罩必须**不透明**，禁止毛玻璃透出终端文字。
- 锁定动作必须移走终端焦点，不能只靠视觉遮挡（遮挡挡不住已聚焦 xterm 的键盘事件）。
- `lock.json` 不增任何导入 / 导出 / 备份代码路径；评审以导出组装代码白名单为准（§3）。

**忘记密码**：应用内**不提供**绕过（任何可绕过的入口都会使锁失效）。重置方式：退出应用后删除 `lock.json`（macOS 路径形态 `~/Library/Application Support/com.rhost.app/lock.json`），重启后两级锁均为未启用状态；`connections.json` 与钥匙串中的主机密码不受影响。该说明写入设置面板与后续操作指南。

**降级**：

- `lock.json` 损坏：隔离 + 视为未设置 + toast 告警（§6.1）。
- 后端命令不可用（IPC 异常）：应用锁遮罩保持锁定并显示「解锁服务异常，点击重试」，**不提供前端绕过**；会话锁遮罩同样仅允许重试。
- 未设置应用锁密码时：所有锁屏入口隐藏 / 不响应，闲置 watcher 不启动；未设置会话锁凭据时：标签锁图标引导去设置，不产生半启用状态。
- `appLockEnabled=true` 但应用锁密码槽位缺失（`lock.json` 被删）：`initAppLock()` 不锁定、toast 告警并把开关写回 false（§6.7），避免进入「想开开不了、想解没有密码」的死态。
- 系统时间被回拨：限速只依赖单调时钟 `Instant`，闲置判定依赖 `Date.now()` 差值，回拨最坏导致一次提前 / 延后锁屏，无安全后果。

## 8. 测试与验证

- **Rust 单测**（`lock.rs` 内 `#[cfg(test)]`）：
  - 设置后正确密码验证成功、错误密码失败；错误文案含剩余次数；
  - 两次设置生成的 salt 不同；hash 不等于密码任意片段的编码；
  - 常量时间比较：等长 / 不等长输入均返回正确布尔；
  - 限速：连续第 5 次失败后进入 30s 冷却，冷却期调用不执行 KDF 直接拒绝；批次翻倍序列正确、300s 封顶；成功验证清零；
  - `lock.json` 写后重读结构往返；损坏文件被改名隔离且按未设置处理；
  - `iters` 篡改为 2000000 时按损坏处理；
  - 导出测试：新增断言 `assemble_export` 全 scope 产物不含 `lock.json` 任何字段（文件隔离的结构性保证）。
- **手工验证**：
  - 标签锁图标两态正确：未锁低透明度开锁图标、上锁一次点击完成；已锁为强调色闭合锁，点击聚焦遮罩密码框；点击图标不误触发切标签 / 拖拽 / 关标签；
  - 状态栏最左侧锁图标（工作台）与首页标题栏锁图标均可锁屏，态随锁定翻转；
  - 锁屏后键盘狂敲 / 粘贴，远端无任何输入到达；解锁后剪贴板粘贴行为恢复；
  - 锁屏期间 SSH 不掉线（远端 `w` / 持续输出任务验证），解锁后滚动缓冲完整；
  - 闲置下拉各档位（含「关闭」）与失焦即锁开关均生效，切换后无需重启即时作用；手改配置文件为白名单外的值，重启后回退 300；
  - 应用锁定后 5s 内 Inspector 无 metrics 网络请求、状态栏内存指标停刷；解锁后两者自动恢复，锁定时段曲线不回填；激活会话被会话锁锁定时 metrics 停止、切到未锁会话恢复；
  - 会话锁定后 SFTP 按钮禁用、AI 对该会话工具调用被拒、遮罩不透明看不到历史输出；
  - 勾选 / 取消「复用应用锁密码」两种路径解锁均通；关闭应用锁需密码；
  - 冷启动：启用应用锁后退出重启，splash 撤下即见锁屏且会话在遮罩后恢复，解锁立即可用；未启用时启动行为与现状一致；删除 `lock.json` 后重启出现告警 toast 且开关回退为关；
  - 故意写坏 `lock.json` 重启，出现隔离文件与告警 toast；
  - 全量导出配置后在导出文件中搜索不到锁相关字段。
- **e2e**：不新增（现有 e2e 仅覆盖 SSH 链路）。

## 9. 未决问题

- 生物识别（Touch ID / Windows Hello）：本期不做。后续在 `lock_unlock` 之外增加「系统凭据因子」验证通道即可，`lock.json` 结构与前端状态机无需改动。
- 锁屏时是否最小化窗口或启用系统窗口模糊（让 macOS 调度中心 / Win-Tab 缩略图也不显示内容）：本期仅做应用内遮罩，待评估 Tauri 窗口级 API 后单独立项。
- 锁定快捷键暂为固定 `⌘/Ctrl+Shift+L`，不进 settings 可配置项；待项目全局快捷键体系落地时与 `key.*` 设置项统一接入。
