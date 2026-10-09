# 标签栏右键菜单设计方案

> description: 工作台标签栏右键菜单设计：新增重命名、复制会话、关闭左侧与会话属性，移除复制地址类菜单项
>
> created: 2026-10-07 12:54:58
>
> updated: 2026-10-07 14:20:05
>
> author: [sjzhao](https://github.com/ShiJieCloud/rhost)

## 1. 设计目标与硬约束

> scope: 本文覆盖工作台会话标签栏（终端 Tab）的右键上下文菜单、标签栏空白区菜单，以及鼠标中键关闭标签；改动主要在前端。
> 边界：不覆盖左侧主机列表（`WbSideBar.vue`）、底部 Dock 面板（`DockPanel.vue` 已自带右键菜单）、主机管理的新增/删除流程；**复制会话基于已有连接能力建立新连接，本期不设计后端能力细节**；其余功能不涉及 Rust 后端改动。
>
> **会话与标签的定位**：打开的会话/标签是**本地运行时 UI 状态**，不是可迁移配置。`ui_state.sessions` 的格式变更（含 `alias`）**不进入配置迁移框架**，也**不进入配置导入导出**——导出文件不含 sessions，导入时忽略 sessions 字段；旧格式数据在升级后直接丢弃（会话为临时状态，丢失可从侧栏重开）。
>
> 关联代码：`frontend/src/components/wb/WbTabs.vue`、`frontend/src/components/wb/ContextMenu.vue`、`frontend/src/stores/session.ts`、`frontend/src/stores/hosts.ts`、`frontend/src/style.css`（`.tabs` / `.tab` 段）。

| 目标 | 量化口径 |
|---|---|
| 补齐标签管理入口 | 关闭类操作 6 项（当前/其他/左侧/右侧/全部/已断开）均可一次点击完成，无需先切换到目标标签 |
| 连接态操作可达 | 任意状态的标签，不切换焦点即可执行连接、重连或断开；动作成功率 100% 走既有 store 状态机，复制会话为新增独立会话路径 |
| 标签个性化 | 支持标签重命名；支持复制会话，新建一套独立 PTY 连接，登录回到家目录 |
| 低开销 | 菜单为纯前端 computed 构造，弹出路径无 IPC 调用（复制会话动作点击后才发起 IPC），一次菜单构建 < 16ms（单帧内） |
| 批量操作流畅 | 连续关闭 20 个标签同步完成，UI 无卡死（< 100ms），后端断连 IPC 异步发出且互不阻塞 |

必须遵守的既有项目约束（修订变更标记）：

- ❗**原有约束"同一主机复用同一会话"不再适用于复制会话菜单项**：`openSession(hostId)` 原有逻辑保持不变；复制会话是新增独立 API，对同一个主机创建第二个独立会话 PTY，因此本期提供**复制会话（新建独立连接）**。普通打开主机依旧遵循：对已打开的主机只激活、不新建。**左侧主机列表点击打开主机、`+` 快速连接、空白区"新建连接…"均走 `openSession` 既有逻辑（同主机复用），不受复制会话新能力影响——开发时不得为支持复制会话而修改 `openSession` 的通用打开路径。**
- **关闭标签 = 断开 + 清理**：`closeSession(id)` 内部已完成取消自动重连、`disconnect_session`、终端与指标 map 清理、`ui_state.sessions` 持久化，批量操作必须逐个复用它，禁止另写清理路径。
- **断开 ≠ 关闭**：`disconnectSession(id)` 保留标签与断线前终端快照、冻结输入；菜单中"断开连接（不关闭标签）"与"关闭标签"是两个独立动作。
- **右键菜单统一组件**：全应用复用 `ContextMenu.vue`（Teleport 浮层、视口边缘夹取、方向键导航、外部 mousedown/scroll/resize 关闭），不引入第三方菜单库。
- **重命名标签**：标签自定义别名持久化到 `ui_state.sessions` 的 `SessionEntry.alias` 字段；`host.id` 为底层不变标识，UI 展示优先使用别名，无别名回落 `host.id`。**alias 绑定在 session 实例（`sessionId`），不绑定 host**：会话关闭后从侧栏重新打开（`openSession` 新建 session）不会继承旧 alias；同一主机的多个会话 alias 相互独立，各自持久化。
- **复制会话**：复制会话生成全新 sessionId，复用原主机连接参数，拉起独立 PTY 会话，登录后回到家目录；新会话独立生命周期，与源会话互不干扰。
- **文档与代码同 PR**：本设计落地 PR 必须同步更新本文与 `docs/.vitepress/config.mts` 登记；菜单能力不修改配置 schema。

## 2. 必须遵守的既有项目约束

- 标签顺序即 `sessions` 数组顺序；会话存活集合持久化在 `app_config.json` 的 `ui_state.sessions`。**本期将存储格式由 `string[]`（主机 ID 列表）改为 `SessionEntry[]`**，每个条目形如 `{ sessionId, hostId, alias }`——其中 `sessionId` 为持久化唯一键（解决同主机多会话的区分问题），`hostId` 用于恢复时查找主机，`alias` 为标签别名（可空）。由 `persistSessions()` 维护读写。
  - **不进入配置迁移框架**：`string[]` → `SessionEntry[]` 的变更不写迁移脚本；升级后首次启动读取旧格式失败时直接清空 sessions（会话为临时状态，用户可从侧栏重开）。
  - **不进入配置导入导出**：导出时从 `ui_state` 中剔除 `sessions` 字段；导入时忽略文件中的 `sessions` 字段，不做 ID 重映射（移除 `config_io.rs` 中 `remap_sessions` 逻辑）。
  - 后端 `UiStateSection.sessions` 类型由 `Vec<String>` 改为 `Vec<SessionEntry>`（`sessionId`/`hostId`/`alias` 三字段）。
- 会话五态由 store 单一驱动：`idle`（冷启动懒恢复、未连接）/ `connecting` / `online` / `reconnecting`（含退避等待与尝试中）/ `offline`。菜单的可用态只允许读取这五态判定，不自建状态。
- 重连入口统一为 `reconnectBackend(id)`（内部取消退避定时器、断旧连接、按已记录 PTY 尺寸重连）；断开入口统一为 `disconnectSession(id)`（登记手动旗标、取消自动重连、置 `offline`）。
- 新增：`duplicateSession(sourceSessionId)`：基于源会话主机配置创建全新独立会话，打开新标签，登录回到家目录。
- 新增：会话属性弹窗，读取当前会话/主机元信息；复用全局弹窗挂载点。
- 全局弹窗已在 `App.vue` 挂载；**重命名使用标签 inline 原地编辑**（标签文本直接替换为输入框）；会话属性使用独立弹窗。
- 视觉规范沿用 `ContextMenu` 既有样式（深色半透明、184px 宽、28px 行高、危险项红色），危险操作（批量关闭）不新增二次确认弹窗——与现有单标签 × 关闭、SFTP 删除的即时操作习惯保持一致；误关可从左侧栏一键重新打开，且主机配置不受影响。

## 3. 现状盘点与差距

| 位置 | 现状 | 差距 |
|---|---|---|
| `frontend/src/components/wb/WbTabs.vue` | 仅 52 行：`+` 快速连接下拉、单击切换标签、`×` 关闭单个标签 | 无右键菜单；无批量关闭（缺少关闭左侧）；无重命名标签、复制会话、会话属性入口；无重连/断开入口（只能在终端顶部状态条或右侧 Inspector 操作，且仅对活动标签生效） |
| `frontend/src/stores/session.ts` | 有 `closeSession`、`reconnectBackend`、`disconnectSession`、`openSession` | 缺"关闭其他/左侧/右侧/已断开/全部"批量函数；缺少 `duplicateSession` 复制会话；缺少会话 alias 读写（`Session` 增加 `alias: string`，空串表示无别名）；批量逻辑若写在组件里会绕过清理与持久化 |
| `frontend/src/components/wb/ContextMenu.vue` | 通用菜单组件，支持图标/快捷键/禁用/危险态/分隔符、边缘夹取、键盘导航 | 菜单打开时未拦截按键向底层元素传播：在 SFTP 面板使用时焦点在文件列表无副作用，但标签栏菜单弹出时焦点在活动终端（xterm hidden textarea），按键会穿透进 PTY |
| 标签栏样式（`style.css`） | `.tabs` / `.tab` / `.tab-add` / `.quick-menu` 已定义，标签栏高 40px；`.tab .name` 已有 `white-space:nowrap` + `text-overflow:ellipsis` | 需要增加标签重命名编辑态样式；别名超长展示复用 `.tab .name` 既有省略规则，不新增样式；菜单依旧 Teleport 到 body |

结论：本设计补齐**标签右键菜单、空白区菜单、中键关闭**三块交互，新增重命名标签、复制会话、关闭左侧标签页、会话属性；store 补充批量关闭（含关闭左侧）、复制会话、别名持久化基建；同时给 `ContextMenu` 增加按键不穿透的通用增强。

## 4. 目标与非目标

### 目标

1. 标签右键提供：重命名标签、复制会话、重新连接、断开连接；6 种关闭操作；打开 SFTP 面板；会话属性弹窗。
2. 标签栏空白区右键提供新建连接、关闭已断开标签、关闭全部标签。
3. 鼠标中键单击标签即关闭该标签（含 `preventDefault` 抑制中键自动滚动）。
4. 菜单按会话五态实时呈现可用/禁用；右键**不切换**活动标签（与 VS Code、iTerm2 行为一致）；复制会话、重新连接、打开 SFTP 面板这些需要会话上下文动作执行时激活对应标签。
5. 菜单打开期间**普通输入按键**不穿透进活动 PTY；系统快捷键（Ctrl/Cmd/Alt 组合）放行到终端，复制粘贴等不受影响。

### 非目标

- ~~不做标签自定义标题（重命名）~~：**本期纳入目标，增加 alias 持久化**。
- ~~不做"复制 / 重复标签"~~：**本期纳入目标，复制会话生成完全独立 PTY 会话，不再受旧的单会话约束**。
- 不做标签颜色标记、未读消息角标、标签钉选。
- 不新增全局键盘快捷键（如 ⌘W 关闭标签），本期仅鼠标交互；菜单内不显示未实现的快捷键提示。
- 不做关闭确认弹窗。
- **不做标签过多溢出处理**：不新增左右滚动箭头、全量标签下拉列表、活动标签自动滚入视野；保留现有横向滚动（触控板/Shift+滚轮）+ `max-width:190px` + 文本省略，标签过多时用户手动滚动浏览。留待后续迭代。
- 复制会话基于已有连接能力建立新连接，其余不新增前端日志 event_id；连接/断开动作经既有命令，后端 `ssh.session.*` 生命周期事件完整记录。

## 5. 总体架构

```text
┌─ WbTabs.vue（改造） ─────────────────────────────────────────┐
│  .tab @contextmenu → onTabContext(e, session)   ─┐           │
│  .tab @mousedown(button=1) → closeSession(id)    │ 写菜单状态  │
│  .tabs 空白 @contextmenu → onBarContext(e)       ┘            │
│                                                              │
│  menu = ref<{ x, y, targetId: string | null }>(null)        │
│            │ targetId 非空 = 标签菜单；null = 空白区菜单      │
│            ▼                                                 │
│  buildTabMenu(s) / buildBarMenu()  ← 只读会话五态生成项      │
│            │                                                 │
│  <ContextMenu v-model:visible :x :y :items />（既有组件）   │
└───────────────┬──────────────────────────────────────────────┘
                │ 动作全部调用 store 函数，组件不写业务清理
                ▼
┌─ stores/session.ts（新增 5 个编排函数 + alias 读写） ─────────┐
│  closeOtherSessions(id)        closeSessionsToRight(id)     │
│  closeSessionsToLeft(id)       closeDisconnectedSessions() │
│  closeAllSessions()                                         │
│  duplicateSession(sourceId) // 复制会话，新建独立连接           │
│  setSessionAlias(sessionId, alias) // 标签重命名别名持久化   │
└──────────────────────────────────────────────────────────────┘
  复用：reconnectBackend / disconnectSession / openSession
  复用：persistSessions（closeSession / setSessionAlias 内调用落盘）

┌─ ContextMenu.vue（小幅通用增强） ────────────────────────────┐
│  visible 期间 document 捕获阶段 keydown：                    │
│  导航键菜单处理后 stopPropagation（防终端副作用）；            │
│  修饰键组合（Ctrl/Cmd/Alt+key）放行到终端；                   │
│  普通单字符 stopPropagation()，阻止穿透到 xterm textarea/PTY  │
└──────────────────────────────────────────────────────────────┘

┌ 弹窗层 ──────────────────────────────────────────────────────┐
│  重命名 inline 原地编辑；会话属性弹窗；SFTP dock 唤起          │
└──────────────────────────────────────────────────────────────┘
```

> 变更说明：复制会话基于已有连接能力建立新连接；`ui_state.sessions` 存储格式由 `string[]` 改为 `SessionEntry[]`（含 `sessionId`/`hostId`/`alias`），不进入配置迁移与导入导出；新增 `closeSessionsToLeft` 批量关闭函数；重命名采用标签 inline 原地编辑。其余 IPC / 二进制帧 / 配置 schema 无改动。

## 6. 详细设计

### 6.1 标签右键菜单（targetId 非空）

菜单按目标会话 `s` 的当前状态构造；分组 + 分隔线严格遵循如下顺序：

```text
├─ 重命名标签
├─ 复制会话（新建独立连接，登录回到家目录）
├─ 重新连接
├─ 断开连接（不关闭标签）
├─ ——— 分隔线 ———
├─ 关闭标签
├─ 关闭其他标签页
├─ 关闭左侧标签页
├─ 关闭右侧标签页
├─ 关闭已断开的标签
├─ 关闭所有标签页
├─ ——— 分隔线 ———
├─ 打开 SFTP 面板（当前会话唤起 SFTP，同通道）
└─ 会话属性
```

不可用项以 `disabled` 展示而非隐藏，保持菜单形态稳定。

| 组 | 菜单项 | 可用条件 | 动作 |
|---|---|---|---|
| 会话操作 | 重命名标签 | 始终可用 | 触发标签 **inline 原地编辑**（详见 §6.1.1）；保存触发为 **Enter 或输入框失焦（blur）双通道**；保存时调用 `setSessionAlias(s.id, trimmedAlias)`，空字符串清除别名，UI 回退显示 `host.id`；执行 `persistSessions()` 持久化别名。**超长别名由标签 `.name` 的 `white-space:nowrap` + `text-overflow:ellipsis` 省略展示，不改变标签固定宽度** |
| 会话操作 | 复制会话（新建独立连接，登录回到家目录） | `state !== connecting && state !== reconnecting` 且该源会话无进行中的复制请求 | 调用 `duplicateSession(s.id)`，基于源主机配置创建全新独立会话，生成新 sessionId，新建标签页；新会话登录后回到家目录；激活新打开的会话标签。**并发保护：前端对同一源会话做互斥（进行中时菜单项 disabled），防止快速连点并发创建大量会话** |
| 会话操作 | 重新连接 | `state` ∈ `idle`、`offline`、`reconnecting` | 先 `activeSessionId = s.id`，再 `reconnectBackend(s.id)`；`idle` 态首次连接同样走该入口 |
| 会话操作 | 断开连接（不关闭标签） | `state` ∈ `online`、`connecting`、`reconnecting` | `disconnectSession(s.id)`（不 await，fire-and-forget，与 `closeSession` 内 `disconnect_session` 处理一致）；标签保留、终端快照保留、输入冻结 |
| — | 分隔线 | — | — |
| 关闭批量 | 关闭标签 | 始终可用 | `closeSession(s.id)` |
| 关闭批量 | 关闭其他标签页 | 除 `s` 外存在 ≥1 个标签 | `closeOtherSessions(s.id)`，完成后活动标签为 `s` |
| 关闭批量 | 关闭左侧标签页 | `s` 左侧存在 ≥1 个标签 | `closeSessionsToLeft(s.id)`；若活动标签落在被关闭范围，沿用 `closeSession` 原有后继激活逻辑 |
| 关闭批量 | 关闭右侧标签页 | `s` 右侧存在 ≥1 个标签 | `closeSessionsToRight(s.id)`；若活动标签落在被关闭范围，沿用 `closeSession` 原有后继激活逻辑 |
| 关闭批量 | 关闭已断开的标签 | 存在 ≥1 个 `state === 'offline'` 标签 | `const n = closeDisconnectedSessions(); if (n > 0) toast(\`已关闭 ${n} 个标签\`)`；store 只返回关闭数量，toast 由组件层负责；目标 `s` 自身离线时一并关闭，菜单随组件卸载自然消失 |
| 关闭批量 | 关闭所有标签页 | `sessions.length > 0`（恒真） | `closeAllSessions()`；全部关闭后 `activeSessionId = null`，工作台显示空态 |
| — | 分隔线 | — | — |
| 面板与信息 | 打开 SFTP 面板（当前会话唤起 SFTP，同通道） | `state === 'online'` | `activeSessionId = s.id` 后 `openDock('sftp')`；非在线态禁用，SFTP 依赖在线会话通道 |
| 面板与信息 | 会话属性 | 始终可用 | 弹出会话属性弹窗，展示会话 ID、主机信息、连接状态、端口、用户名等元数据；仅查看，不在弹窗修改主机配置 |

特殊状态说明：

1. `connecting` / `reconnecting` 状态：复制会话禁用，避免同时大量发起连接；
2. `connecting` 态点"断开连接（不关闭标签）"即取消本次建连：`disconnectSession` 内部连接代次 +1，在途 `connect_ssh` 即使成功也按孤儿连接处理；
3. `reconnecting` 态"重新连接"与"断开连接（不关闭标签）"同时可用；前者手动立即重试接管退避定时器，后者放弃自动重连进入离线冻结态；
4. 菜单弹出后不主动重建菜单项内容；若会话状态变化导致 `buildTabMenu` 重新 computed、`items` 引用变化，`ContextMenu` 仅重夹取位置（不重建内容）；点击菜单项时实时读取最新状态；`disabled` 项被 ContextMenu 拦截不会执行；
5. 重命名别名允许空值；清空别名则标签 UI 恢复原始 `host.id`；
6. 复制会话源会话不受任何改动，新会话完全独立生命周期。

#### 6.1.1 重命名标签 inline 原地编辑交互

重命名不使用弹窗，采用**标签原地编辑**：点击「重命名标签」后，目标标签的文本区域被替换为 `<input>`，编辑完成或取消后恢复为文本。

```text
触发：右键菜单 → 重命名标签
  │
  ├─ 标签 .name 区域原地替换为 <input>
  ├─ 输入框宽度贴合标签容器（不超出标签边界）
  ├─ value 预填充当前别名（无别名则填充 host.id）
  ├─ 自动全选已有文本（便于直接覆盖）
  └─ 自动聚焦

确认保存（双通道，同一保存逻辑）：
  - Enter 键 → 保存
  - 输入框 blur（失焦）→ 保存
  - 说明：点击输入框外任意区域（含菜单外、空白处、其他 DOM）触发 blur 即保存；
        blur 保存与 Enter 走同一 setSessionAlias + persistSessions 路径。

取消（仅 ESC 主动取消）：
  - ESC 键 → 设置 skipSave 旗标 → 让输入框失焦 → blur 处理读取旗标，放弃修改，恢复原有标签文本，不写 store
  - 注意：ESC 必须先置 skipSave 再 blur，避免 blur 先触发保存。

保存逻辑：
  1. 若 skipSave 旗标为真 → 直接恢复原文本，退出
  2. 对输入值 trim() 去除首尾空白
  3. 若结果为空字符串 → 调用 setSessionAlias(id, "") 清除别名
  4. 否则 → 调用 setSessionAlias(id, trimmedAlias)
  5. 执行 persistSessions() 持久化
```

**边界与保护：**

- **不允许换行**：输入框为 `type="text"` 单行，天然不接受换行；粘贴含换行的文本时仅保留首行（或直接过滤 `\n`/`\r`），防止多行撑破标签栏。
- **超长输入不撑破布局**：inline 输入框若随文本增长会撑开标签宽度，破坏标签栏横向布局。必须双重限制：
  - `input` 设置 `maxlength`（如 50 字符），强制最大输入长度；
  - `input` 设置 `max-width` 等于标签内容区宽度（`width: 100%` + 父容器 `overflow:hidden`），视觉上不超出标签边界。
- **编辑中切换标签 → 丢弃修改**：编辑态下若 `activeSessionId` 变化（点击其他标签、批量关闭导致活动标签切换），先置 `skipSave` 旗标再失焦，blur 处理读取旗标丢弃修改、恢复原文本，**不保存**。普通点击空白处的 blur 才保存。`skipSave` 旗标在进入编辑态时重置为 `false`，ESC 与切换标签共用同一旗标。
- **窗口 resize / 标签栏滚动 → 不错位**：输入框渲染在标签 `.name` 原位（非 Teleport），随标签 DOM 自然流动；resize/滚动时位置始终与标签对齐，无需手动定位。
- **编辑态下菜单已关闭**：点击「重命名标签」后 ContextMenu 关闭，输入框接管键盘事件，不与菜单按键拦截冲突。

### 6.2 标签栏空白区菜单（targetId 为 null）

在 `.tabs` 容器空白处右键弹出；保持原有逻辑，复制会话、重命名、会话属性不出现于空白菜单。

| 菜单项 | 可用条件 | 动作 |
|---|---|---|
| 新建连接… | 始终可用 | `showNewConn.value = true`（打开新建连接弹窗） |
| 分隔线 | — | — |
| 关闭已断开的标签 | 存在 `offline` 标签 | `const n = closeDisconnectedSessions(); if (n > 0) toast(\`已关闭 ${n} 个标签\`)`；store 返回数量，组件负责 toast |
| 关闭全部标签 | `sessions.length > 0` | `closeAllSessions()`；无标签时禁用 |

### 6.3 右键与中键事件契约

```text
@contextmenu（.tab 根元素）
  1. preventDefault()（屏蔽浏览器原生菜单）
  2. 不修改 activeSessionId（右键不抢焦点）
  3. menu = { x: e.clientX, y: e.clientY, targetId: s.id }

@contextmenu（.tabs 空白）
  1. preventDefault()
  2. 命中 .tab / .tab-add / .quick-menu 则不弹（交由元素自身逻辑）
  3. menu = { x: e.clientX, y: e.clientY, targetId: null }

@mousedown（.tab 根元素）
  event.button === 1（中键）时：
  1. preventDefault()（抑制中键自动滚动 / 橡皮筋）
  2. closeSession(s.id)
  菜单打开与否不影响中键关闭。
```

> 右键点在 `×` 关闭按钮上，依旧弹出标签菜单，关闭仅响应左键 click。

### 6.4 store 批量关闭函数（新增 closeSessionsToLeft；其余更新）

新增 5 个导出函数 + alias 读写 + 复制会话；批量函数统一原则：**先快照 id 数组，再逐个调用 closeSession，防止数组遍历索引漂移**。

```ts
/** 关闭除 id 外的全部会话；完成后激活 id */
export function closeOtherSessions(id: string) {
  const targets = sessions.value.filter(s => s.id !== id).map(s => s.id)
  targets.forEach(closeSession)
  activeSessionId.value = id
}

/** 关闭 id 左侧（按当前标签顺序）全部会话 */
export function closeSessionsToLeft(id: string) {
  const idx = sessions.value.findIndex(s => s.id === id)
  if (idx <= 0) return
  sessions.value.slice(0, idx).map(s => s.id).forEach(closeSession)
}

/** 关闭 id 右侧全部会话 */
export function closeSessionsToRight(id: string) {
  const idx = sessions.value.findIndex(s => s.id === id)
  if (idx < 0) return
  sessions.value.slice(idx + 1).map(s => s.id).forEach(closeSession)
}

/** 关闭全部 offline 标签；返回实际关闭数量（toast 用） */
export function closeDisconnectedSessions(): number {
  const targets = sessions.value.filter(s => s.state === 'offline').map(s => s.id)
  targets.forEach(closeSession)
  return targets.length
}

/** 关闭全部标签，进入工作台空态 */
export function closeAllSessions() {
  sessions.value.map(s => s.id).forEach(closeSession)
}

/** 设置标签别名；空字符串 "" 表示清除别名，UI 回落展示 host.id。 */
export function setSessionAlias(sessionId: string, alias: string) {
  const item = sessions.value.find(s => s.id === sessionId)
  if (!item) return
  item.alias = alias
  persistSessions()
}

/** 复制会话：基于源会话 id，创建全新独立会话，返回新 sessionId。
 *  并发保护：duplicating 集合记录进行中的源会话 id；正常路径 finally 清理；
 *  超时兜底：setTimeout 8s 后强制从 duplicating 移除（防止 IPC 进程崩溃
 *  导致 finally 不执行、菜单项永久置灰）。
 *  新会话状态：duplicateSession 触发后立即以 connecting 态加入 sessions，
 *  连接成功后转 online，失败转 offline。 */
const duplicating = new Set<string>()
const DUPLICATE_TIMEOUT_MS = 8000
export async function duplicateSession(sourceSessionId: string): Promise<string | null> {
  if (duplicating.has(sourceSessionId)) return null
  duplicating.add(sourceSessionId)
  const timer = setTimeout(() => duplicating.delete(sourceSessionId), DUPLICATE_TIMEOUT_MS)
  try {
    // 基于源主机配置创建全新独立会话，新会话登录回到家目录
    // 新建 session 立即以 connecting 态加入 sessions 列表，持久化，激活新标签
    // 连接成功 → online；失败 → offline + toast
    // 拿到新 sessionId 后返回
  } finally {
    clearTimeout(timer)
    duplicating.delete(sourceSessionId)
  }
}
```

设计要点：

- `idle` 标签不属于"已断开"，不会被 `closeDisconnectedSessions` 清理；
- `closeSession` 自带取消重连、IPC 断开、清理 map、数组移除、活动标签后继、持久化；批量函数不再重复实现；
- 别名变更 `setSessionAlias` 主动调用 `persistSessions()` 写配置；
- **持久化格式**：`persistSessions()` 将 `sessions` 映射为 `SessionEntry[]`（`{ sessionId, hostId, alias }`）写入 `ui_state.sessions`；`restoreSessions()` 读取 `SessionEntry[]`，按 `hostId` 查找主机、用持久化的 `sessionId` 作为前端会话 id、`alias` 赋给 `Session.alias`。**`sessionId` 是持久化唯一键，`restoreSessions` 不再按 `hostId` 去重**（移除原有 `[...new Set(hostIds)]` 逻辑），同一主机的多个会话逐条恢复；旧 `string[]` 格式读取失败时直接清空（不迁移）。`openSession` 仍按 `hostId` 复用（左侧栏打开），`duplicateSession` 不走 `openSession`，两者互不干扰。
- 复制会话为异步函数；新会话**立即以 `connecting` 态加入 sessions**，连接成功转 `online`，失败转 `offline` + toast；源会话不受影响；`duplicating` 集合有 8s 超时兜底，防止 IPC 异常导致菜单项永久置灰；
- 批量关闭多次调用 `patchUiState`，依靠内部 300ms 防抖合并写盘。

### 6.5 ContextMenu 按键不穿透增强

菜单可见期间在 `document` 捕获阶段监听 `keydown`（`capture: true`）。`stopPropagation` 只阻止事件向下游（xterm textarea）传播，不阻止浏览器默认行为；因此**必须区分三类按键**，避免误杀系统快捷键：

| 按键类型 | 判定 | 处理 |
|---|---|---|
| 菜单导航键 | `Escape` / `ArrowUp` / `ArrowDown` / `Enter` | ContextMenu 内部处理（导航/确认/关闭）后 `stopPropagation()`，阻止终端光标移动或回车注入 PTY |
| 修饰键组合 | `e.ctrlKey` / `e.metaKey` / `e.altKey` 任一为真（如 Ctrl+C、Cmd+V、Alt+F4） | **直接放行**，不 `stopPropagation`，让系统快捷键落到终端 textarea（复制/粘贴等不受影响） |
| 普通单字符 | 无修饰键的可打印字符、空格、Backspace、Delete 等 | `stopPropagation()`，阻止输入字符穿透到 PTY 终端 |

> 关键：Ctrl/Cmd/Alt 组合键绝不能 `stopPropagation`，否则终端收不到粘贴（Ctrl+V）等操作——即使系统快捷键本身未被 `preventDefault`，事件不再向下传递也会导致粘贴失效。导航键与普通字符则必须 `stopPropagation`，避免菜单操作时终端产生副作用。菜单关闭时移除监听。
>
> **对现有 `onKeydown` 的改动点**：当前 `ContextMenu.vue` 的 `onKeydown`（第 67-83 行）仅处理导航键且不调用 `stopPropagation`。需在 `onKeydown` 末尾追加：若不是导航键且不是修饰键组合，则 `e.stopPropagation()`；导航键在各自处理分支末尾追加 `e.stopPropagation()`。

### 6.6 菜单定位与高度

标签栏在窗口底部，由 ContextMenu 自带 `clampPos` 视口边缘夹取自动向上翻折；增加菜单项后菜单估算高度更新；小窗口使用 `max-height + vertical scroll` 兜底。

## 7. 安全与降级

- **复制会话**：完全复用源会话主机连接参数；不会修改源会话任何状态；后端 IPC 失败 toast 提示，不崩溃。
- **复制会话并发保护**：前端对同一源会话用 `duplicating` 集合做互斥，进行中时菜单项 disabled，防止快速连点并发创建大量会话；设 8s `setTimeout` 兜底，IPC 异常（如进程崩溃导致 `finally` 不执行）时自动释放锁，避免菜单项永久置灰。
- **重命名标签**：别名仅存储本地配置，仅 UI 展示使用；底层 `host.id` / `session.id` 保持不变，不会干扰会话恢复逻辑；支持清空别名。**alias 绑定 session 实例而非 host**：session 删除重建后 alias 丢失属预期行为，不做跨 session 继承。
- **会话属性弹窗**：仅展示元信息；不展示密码、私钥敏感凭据。
- **后端 IPC 失败**：断开/重连/复制会话失败沿用原有错误提示；菜单层不吞错。
- **批量关闭部分失败**：单个会话 IPC 失败不阻断其余标签关闭；前端标签移除；孤儿会话依靠后端连接代次回收。
- **菜单渲染隔离**：菜单构建抛错只影响右键菜单；标签切换、中键关闭不受影响。

## 8. 测试与验证

- 单测（未来）：会话五态 × 更新后的全部菜单项禁用矩阵；新增 `closeSessionsToLeft`、`setSessionAlias`、复制会话返回状态校验。

手工验证补充项：

1. **重命名标签（inline 原地编辑）**：右键→重命名，标签文本变输入框、预填充并全选；Enter 保存后标签显示别名；**输入框失焦（点击空白处）同样保存**；ESC 放弃恢复原文本；空/全空格保存清除别名恢复 `host.id`；输入超长不撑破标签（maxlength + max-width 生效）；编辑中切换其他标签丢弃修改（不保存）；窗口 resize/标签栏滚动输入框不错位；重启应用别名持久化正确；
2. **复制会话**：源会话 online 状态复制；生成全新独立标签，登录回到家目录；源会话不受影响；两个会话可以独立重连/断开；connecting/reconnecting 状态复制会话置灰不可点击；连接失败 toast 提示；
3. **关闭左侧标签页**：中间标签右键，左侧全部关闭；最左侧标签右键"关闭左侧"置灰；活动标签落在关闭区间自动切换后继标签；
4. **会话属性弹窗**：不同会话状态打开弹窗，元信息正确；仅查看，不可编辑主机配置；
5. 其余原有项：重新连接、断开连接、关闭其他/右侧/已断开/全部、中键关闭、键盘防穿透、小窗口菜单定位、空白区菜单逻辑同原有测试用例。

## 9. 未决问题

1. **批量操作可观测性**：是否新增前端上报事件，本期不做；复制会话、批量关闭后端通过 `ssh.session` 生命周期日志记录。

> 已完成：标签重命名（inline 原地编辑）、复制会话（新建独立连接）、关闭左侧标签页纳入本期实现；移除旧菜单复制 SSH 命令、复制连接地址、编辑主机菜单项；复制会话不设计后端能力细节。
