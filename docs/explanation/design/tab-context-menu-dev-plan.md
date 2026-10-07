# 标签栏右键菜单开发计划

> description: 标签栏右键菜单设计方案的分阶段实现计划，覆盖 store 基建、批量关闭、复制会话、按键拦截、菜单与 inline 重命名
> 创建时间：2026-10-07 14:23:03
> 更新时间：2026-10-07 14:23:03
> 作者：

---

## 1. 概述

本计划是 [标签栏右键菜单设计方案](./tab-context-menu-design.md) 的实现分解，按依赖关系拆分为 8 个阶段，每阶段可独立提交。**阶段 1 是后续所有阶段的前置依赖**（数据格式变更），其余阶段尽量并行。

**关联代码**：
- 前端：`frontend/src/stores/session.ts`、`frontend/src/components/wb/WbTabs.vue`、`frontend/src/components/wb/ContextMenu.vue`、`frontend/src/style.css`
- 后端：`src-tauri/src/applog/persisted.rs`（`UiStateSection`）、`src-tauri/src/config_io.rs`（`remap_sessions`）
- 文档：`docs/.vitepress/config.mts`（登记本计划）

**前置约束**：
- 所有批量关闭必须复用 `closeSession`，禁止另写清理路径
- `openSession` 通用打开逻辑不得修改（同主机复用），复制会话走独立路径
- 会话/标签不进入配置迁移与导入导出

---

## 2. 阶段总览与依赖

```text
阶段 1 数据层（Session.alias + 持久化格式 + 后端类型）
  │
  ├─→ 阶段 2 批量关闭函数（依赖 1）
  ├─→ 阶段 3 复制会话（依赖 1）
  ├─→ 阶段 4 ContextMenu 按键拦截（无依赖，可并行）
  └─→ 阶段 5 WbTabs 右键菜单 + 中键关闭（依赖 2、3、4）
        │
        ├─→ 阶段 6 inline 重命名编辑（依赖 5）
        ├─→ 阶段 7 会话属性弹窗 + SFTP（依赖 5）
        └─→ 阶段 8 联调与验证（依赖全部）
```

---

## 3. 阶段 1：数据层改造（Session.alias + 持久化格式）

**目标**：`Session` 增加 `alias` 字段；`ui_state.sessions` 由 `string[]` 改为 `SessionEntry[]`；后端类型同步；不进入迁移/导入导出。

### 3.1 前端 `Session` 接口

文件：`frontend/src/stores/session.ts`

1. 在 `Session` 接口（第 13-28 行）增加字段：
   ```ts
   /** 标签自定义别名；空字符串表示无别名，UI 回落展示 host.id */
   alias: string
   ```

2. 所有创建 `Session` 的地方补 `alias: ''`：
   - `restoreSessions()`（第 890-898 行）：从持久化 `SessionEntry.alias` 读取
   - `openSession()`（第 917-920 行）：`alias: ''`

### 3.2 `persistSessions` 格式变更

文件：`frontend/src/stores/session.ts` 第 874-877 行

```ts
function persistSessions() {
  if (!isTauri) return
  patchUiState({
    sessions: sessions.value.map(s => ({
      sessionId: s.id,
      hostId: s.host.id,
      alias: s.alias,
    })),
  })
}
```

### 3.3 `restoreSessions` 格式变更

文件：`frontend/src/stores/session.ts` 第 880-903 行

```ts
export function restoreSessions() {
  const raw = getSnapshot()?.uiState.sessions
  // 新格式：SessionEntry[] = [{ sessionId, hostId, alias }]
  // 旧格式 string[] 读取失败直接清空（不迁移）
  const entries: Array<{ sessionId: string; hostId: string; alias: string }> = []
  if (Array.isArray(raw)) {
    for (const x of raw) {
      if (x && typeof x === 'object' && 'sessionId' in x && 'hostId' in x) {
        entries.push({
          sessionId: String((x as any).sessionId),
          hostId: String((x as any).hostId),
          alias: typeof (x as any).alias === 'string' ? (x as any).alias : '',
        })
      }
    }
  }
  // 不再按 hostId 去重（移除 [...new Set(hostIds)]），同一主机多会话逐条恢复
  const valid = entries.filter(e => hosts.value.some(h => h.id === e.hostId))
  if (!valid.length) return
  const restored: Session[] = valid.map((e, i) => ({
    id: e.sessionId,                              // 用持久化的 sessionId
    host: hosts.value.find(h => h.id === e.hostId)!,
    state: (i === valid.length - 1 ? 'connecting' : 'idle') as SessionState,
    startedAt: Date.now(),
    disconnectReason: '',
    retryAt: null,
    resetSeq: 0,
    alias: e.alias,
  }))
  // ... 其余不变
}
```

### 3.4 后端 `UiStateSection` 类型

文件：`src-tauri/src/applog/persisted.rs` 第 339-342 行

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct SessionEntry {
    pub session_id: String,
    pub host_id: String,
    #[serde(default)]
    pub alias: String,
}

pub(crate) struct UiStateSection {
    // ... 其余字段不变
    #[serde(default)]
    sessions: Vec<SessionEntry>,
}
```

### 3.5 后端 `config_io.rs` 移除 `remap_sessions`

文件：`src-tauri/src/config_io.rs`

1. 删除 `remap_sessions` 闭包（第 466-481 行）
2. 删除导入流程中对 `sessions` 字段的重映射调用（第 504-510 行附近）
3. 导出流程不包含 `sessions`（导出的 `ui_state` 已剔除 sessions，确认现有逻辑）
4. 更新相关测试（第 936、950、1091 行附近的 sessions 断言改为新格式或直接忽略）

### 3.6 验收

- [ ] `Session` 对象均有 `alias` 字段，类型为 `string`
- [ ] 重启应用后别名正确恢复
- [ ] 旧 `string[]` 格式数据升级后 sessions 为空（不崩溃）
- [ ] 配置导出文件不含 `sessions` 字段
- [ ] 配置导入时文件中的 `sessions` 被忽略
- [ ] 后端编译通过

---

## 4. 阶段 2：store 批量关闭函数

**目标**：新增 5 个批量关闭函数 + `setSessionAlias`，全部复用 `closeSession`。

文件：`frontend/src/stores/session.ts`

### 4.1 新增函数（紧接 `closeSession` 之后，约第 963 行）

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
```

### 4.2 验收

- [ ] `closeSessionsToLeft(id)` 关闭 id 左侧所有标签，最左侧标签调用无效果
- [ ] `closeOtherSessions(id)` 关闭除 id 外所有标签，活动标签为 id
- [ ] `closeDisconnectedSessions()` 只关闭 `offline` 态，不关闭 `idle`，返回关闭数量
- [ ] `closeAllSessions()` 关闭全部，`activeSessionId` 为 `null`
- [ ] `setSessionAlias(id, 'xxx')` 修改别名并持久化；`setSessionAlias(id, '')` 清除别名
- [ ] 批量关闭 20 个标签 UI 无卡死

---

## 5. 阶段 3：复制会话

**目标**：`duplicateSession` 创建独立会话，`connecting` 态加入，8s 超时兜底。

文件：`frontend/src/stores/session.ts`

### 5.1 新增 `duplicateSession`

```ts
const duplicating = new Set<string>()
const DUPLICATE_TIMEOUT_MS = 8000

/** 复制会话：基于源会话 id，创建全新独立会话，返回新 sessionId。
 *  新会话立即以 connecting 态加入 sessions；连接成功转 online，失败转 offline + toast。
 *  并发保护：duplicating 集合 + 8s setTimeout 兜底。 */
export async function duplicateSession(sourceSessionId: string): Promise<string | null> {
  if (duplicating.has(sourceSessionId)) return null
  duplicating.add(sourceSessionId)
  const timer = setTimeout(() => duplicating.delete(sourceSessionId), DUPLICATE_TIMEOUT_MS)
  try {
    const source = sessions.value.find(s => s.id === sourceSessionId)
    if (!source) return null
    // 基于源主机配置创建全新独立会话（复用 openSession 的连接逻辑但不复用会话）
    const newId = crypto.randomUUID()
    sessions.value.push({
      id: newId,
      host: source.host,           // 复用源主机配置
      state: 'connecting',         // 立即进入 connecting 态
      startedAt: Date.now(),
      disconnectReason: '',
      retryAt: null,
      resetSeq: 0,
      alias: '',                   // 新会话无别名
    })
    activeSessionId.value = newId
    persistSessions()
    // 发起连接（复用既有连接通道）；成功 → online，失败 → offline + toast
    // 连接实现参考 openSession 内的 connect_ssh 调用路径
    // ...
    return newId
  } finally {
    clearTimeout(timer)
    duplicating.delete(sourceSessionId)
  }
}
```

> 实现注意：复制会话的连接发起逻辑需复用 `openSession` 中 `connect_ssh` 的调用路径，但**不复用** `openSession` 的"同主机复用"判断。可将连接逻辑抽取为内部函数 `connectSession(id)` 供 `openSession` 和 `duplicateSession` 共同调用。

### 5.2 验收

- [ ] `duplicateSession` 后新标签立即出现，状态为 `connecting`
- [ ] 源会话状态不变
- [ ] 同一源会话快速连点只创建一个（`duplicating` 互斥生效，菜单项 disabled）
- [ ] 连接失败后新会话转 `offline`，toast 提示
- [ ] 模拟 IPC 异常（如 8s 无响应）后 `duplicating` 集合自动清理，菜单项恢复可用

---

## 6. 阶段 4：ContextMenu 按键拦截增强

**目标**：菜单可见期间区分三类按键，防止普通字符穿透到终端，同时放行系统快捷键。

文件：`frontend/src/components/wb/ContextMenu.vue` 第 67-83 行 `onKeydown`

### 6.1 修改 `onKeydown`

在现有导航键处理分支末尾追加 `e.stopPropagation()`，并在函数末尾追加非导航/非修饰键的 `stopPropagation`：

```ts
function onKeydown(e: KeyboardEvent) {
  if (!visible.value) return
  const isNav = ['Escape', 'ArrowUp', 'ArrowDown', 'Enter'].includes(e.key)
  const isModifierCombo = e.ctrlKey || e.metaKey || e.altKey

  if (e.key === 'Escape') {
    closeMenu()
    e.stopPropagation()  // 新增：阻止 Escape 穿透到终端
    return
  }
  if (e.key === 'ArrowUp')   { e.preventDefault(); move(-1); e.stopPropagation(); return }
  if (e.key === 'ArrowDown') { e.preventDefault(); move(1);  e.stopPropagation(); return }
  if (e.key === 'Enter')     { pick(activeIdx.value); e.stopPropagation(); return }

  // 非导航键：修饰键组合放行（复制粘贴等）；普通单字符阻止穿透
  if (!isModifierCombo) {
    e.stopPropagation()
  }
}
```

### 6.2 验收

- [ ] 菜单打开时按 `a`/`b`/空格等字符，终端不收到输入
- [ ] 菜单打开时按 `Ctrl+C`/`Cmd+V`，终端复制粘贴正常
- [ ] 菜单打开时按 `ArrowUp`/`ArrowDown`，菜单项导航，终端光标不移动
- [ ] 菜单打开时按 `Enter`，选中菜单项，终端不收到回车
- [ ] 菜单打开时按 `Escape`，菜单关闭，终端不受影响
- [ ] 菜单关闭后键盘事件恢复正常（监听已移除）

---

## 7. 阶段 5：WbTabs 右键菜单 + 中键关闭

**目标**：标签右键菜单、空白区菜单、中键关闭，复用 `ContextMenu`。

文件：`frontend/src/components/wb/WbTabs.vue`、`frontend/src/style.css`

### 7.1 引入依赖与状态

```ts
import { ref, computed } from 'vue'
import ContextMenu from './ContextMenu.vue'
import {
  sessions, activeSessionId, closeSession, openSession,
  closeOtherSessions, closeSessionsToLeft, closeSessionsToRight,
  closeDisconnectedSessions, closeAllSessions,
  reconnectBackend, disconnectSession, setSessionAlias, duplicateSession,
} from '../../stores/session'
import { showNewConn } from '../../stores/ui'        // 按实际 store 名调整
import { openDock } from '../../stores/dock'         // 按实际 store 名调整
import { toast } from '../../composables/useToast'   // 按实际工具调整

interface MenuState { x: number; y: number; targetId: string | null }
const menu = ref<MenuState | null>(null)
const editingId = ref<string | null>(null)  // inline 重命名编辑中的 session id
const skipSave = ref(false)
```

### 7.2 标签右键菜单构建（computed）

```ts
const tabMenuItems = computed(() => {
  const s = sessions.value.find(x => x.id === menu.value?.targetId)
  if (!s) return []
  const isConnecting = s.state === 'connecting' || s.state === 'reconnecting'
  const hasOther = sessions.value.length > 1
  const idx = sessions.value.findIndex(x => x.id === s.id)
  const hasLeft = idx > 0
  const hasRight = idx < sessions.value.length - 1
  const hasOffline = sessions.value.some(x => x.state === 'offline')
  const isDuplicating = duplicating.has(s.id)  // 从 session store 导出 duplicating 或提供 getter

  return [
    { label: '重命名标签', action: () => startRename(s.id) },
    { label: '复制会话（新建独立连接）', disabled: isConnecting || isDuplicating,
      action: () => { void duplicateSession(s.id) } },
    { label: '重新连接', disabled: !['idle','offline','reconnecting'].includes(s.state),
      action: () => { activeSessionId.value = s.id; void reconnectBackend(s.id) } },
    { label: '断开连接（不关闭标签）', disabled: !['online','connecting','reconnecting'].includes(s.state),
      action: () => { void disconnectSession(s.id) } },
    { type: 'separator' },
    { label: '关闭标签', action: () => closeSession(s.id) },
    { label: '关闭其他标签页', disabled: !hasOther, action: () => closeOtherSessions(s.id) },
    { label: '关闭左侧标签页', disabled: !hasLeft, action: () => closeSessionsToLeft(s.id) },
    { label: '关闭右侧标签页', disabled: !hasRight, action: () => closeSessionsToRight(s.id) },
    { label: '关闭已断开的标签', disabled: !hasOffline,
      action: () => { const n = closeDisconnectedSessions(); if (n > 0) toast(`已关闭 ${n} 个标签`) } },
    { label: '关闭所有标签页', action: () => closeAllSessions() },
    { type: 'separator' },
    { label: '打开 SFTP 面板', disabled: s.state !== 'online',
      action: () => { activeSessionId.value = s.id; openDock('sftp') } },
    { label: '会话属性', action: () => openSessionProps(s.id) },
  ]
})
```

### 7.3 空白区菜单

```ts
const barMenuItems = computed(() => [
  { label: '新建连接…', action: () => { showNewConn.value = true } },
  { type: 'separator' },
  { label: '关闭已断开的标签',
    disabled: !sessions.value.some(s => s.state === 'offline'),
    action: () => { const n = closeDisconnectedSessions(); if (n > 0) toast(`已关闭 ${n} 个标签`) } },
  { label: '关闭全部标签',
    disabled: sessions.value.length === 0,
    action: () => closeAllSessions() },
])
```

### 7.4 事件绑定

模板中 `.tab` 增加：
```html
<div
  v-for="s in sessions"
  :key="s.id"
  class="tab"
  :class="{ active: activeSessionId === s.id, editing: editingId === s.id }"
  @click="activeSessionId = s.id"
  @contextmenu.prevent="onTabContext($event, s)"
  @mousedown="onTabMouseDown($event, s)"
>
```

```ts
function onTabContext(e: MouseEvent, s: Session) {
  // 右键不切换活动标签
  menu.value = { x: e.clientX, y: e.clientY, targetId: s.id }
}

function onTabMouseDown(e: MouseEvent, s: Session) {
  if (e.button === 1) {
    e.preventDefault()  // 抑制中键自动滚动
    closeSession(s.id)
  }
}

function onBarContext(e: MouseEvent) {
  // 命中 .tab / .tab-add / .quick-menu 不弹
  const target = e.target as HTMLElement
  if (target.closest('.tab, .tab-add, .quick-menu')) return
  menu.value = { x: e.clientX, y: e.clientY, targetId: null }
}
```

### 7.5 ContextMenu 挂载

模板末尾：
```html
<ContextMenu
  v-if="menu"
  v-model:visible="menuVisible"
  :x="menu.x"
  :y="menu.y"
  :items="menu.targetId ? tabMenuItems : barMenuItems"
  @close="menu = null"
/>
```

### 7.6 标签别名展示

`.tab .name` 改为：
```html
<span class="name">{{ s.alias || s.host.id }}</span>
```

### 7.7 验收

- [ ] 标签右键弹出菜单，菜单内容随会话状态正确禁用/启用
- [ ] 空白区右键弹出空白区菜单
- [ ] 中键点击标签关闭该标签，不触发页面滚动
- [ ] 右键不切换活动标签
- [ ] 菜单项动作全部走 store 函数
- [ ] 标签显示别名（无别名回落 host.id）

---

## 8. 阶段 6：inline 重命名编辑

**目标**：标签原地编辑，Enter/blur 保存，ESC 取消，超长不撑破布局。

文件：`frontend/src/components/wb/WbTabs.vue`

### 8.1 模板编辑态

```html
<span v-if="editingId !== s.id" class="name">{{ s.alias || s.host.id }}</span>
<input
  v-else
  ref="renameInput"
  class="rename-input"
  type="text"
  :maxlength="50"
  :value="s.alias || s.host.id"
  @keydown.enter="commitRename"
  @keydown.esc="cancelRename"
  @blur="commitRename"
/>
```

### 8.2 编辑逻辑

```ts
const renameInput = ref<HTMLInputElement | null>(null)
const renameOriginal = ref('')

function startRename(id: string) {
  editingId.value = id
  skipSave.value = false
  const s = sessions.value.find(x => x.id === id)
  renameOriginal.value = s?.alias || s?.host.id || ''
  // 下一帧聚焦 + 全选
  nextTick(() => {
    const el = renameInput.value
    if (el) { el.focus(); el.select() }
  })
}

function commitRename() {
  if (skipSave.value) { restoreRename(); return }
  const el = renameInput.value
  const id = editingId.value
  if (!el || !id) { editingId.value = null; return }
  const trimmed = el.value.trim()
  setSessionAlias(id, trimmed)  // 空串清除别名
  editingId.value = null
}

function cancelRename() {
  skipSave.value = true
  renameInput.value?.blur()  // 触发 blur → commitRename 读取 skipSave 放弃
}

function restoreRename() {
  editingId.value = null
}

// 切换标签时丢弃修改
watch(activeSessionId, (newId) => {
  if (editingId.value && newId !== editingId.value) {
    skipSave.value = true
    editingId.value = null
  }
})
```

### 8.3 样式

文件：`frontend/src/style.css`

```css
.tab .rename-input {
  width: 100%;
  max-width: 100%;
  background: transparent;
  border: 1px solid var(--accent);
  color: inherit;
  font-size: inherit;
  padding: 0 4px;
  outline: none;
}
.tab { overflow: hidden; }  /* 确保输入框不超出标签 */
```

### 8.4 验收

- [ ] 右键→重命名，标签文本变输入框，预填充并全选
- [ ] Enter 保存，别名显示在标签上
- [ ] 点击输入框外（blur）保存
- [ ] ESC 放弃，恢复原文本
- [ ] 空/全空格保存后清除别名，显示 host.id
- [ ] 输入超过 50 字符被截断
- [ ] 输入框不超出标签宽度（max-width + overflow:hidden 生效）
- [ ] 编辑中切换其他标签，修改被丢弃
- [ ] 窗口 resize / 标签栏滚动时输入框不错位
- [ ] 重启应用别名持久化

---

## 9. 阶段 7：会话属性弹窗 + SFTP

**目标**：会话属性弹窗展示元信息；SFTP 面板唤起。

### 9.1 会话属性弹窗

复用全局弹窗挂载点（`App.vue`）。弹窗内容只读，展示：
- 会话 ID（`session.id`）
- 主机 ID / 名称（`session.host`）
- 连接状态（`session.state`）
- 端口 / 用户名（来自 `host`）
- 连接时长（`Date.now() - session.startedAt`）
- 后端会话 ID（`session.backendId`，未连接为"未连接"）

不展示密码、私钥内容。

### 9.2 SFTP 面板

菜单动作 `openDock('sftp')`，确保活动会话已设为目标会话。

### 9.3 验收

- [ ] 不同会话状态打开弹窗，元信息正确
- [ ] 弹窗内不可编辑主机配置
- [ ] online 态打开 SFTP 面板正常
- [ ] 非 online 态"打开 SFTP 面板"禁用

---

## 10. 阶段 8：联调与验证

### 10.1 全量手工验证清单

1. **重命名标签**（见 §8.4）
2. **复制会话**：源会话 online 复制；生成独立标签；源会话不受影响；两个会话可独立重连/断开；connecting/reconnecting 态复制禁用；连接失败 toast
3. **关闭左侧标签页**：中间标签右键左侧全关；最左侧标签右键"关闭左侧"置灰；活动标签落在关闭区间自动切换后继
4. **会话属性弹窗**：元信息正确，只读
5. **按键不穿透**：菜单打开时普通字符不进终端，Ctrl+C/V 正常
6. **中键关闭**：中键点击标签关闭，不触发滚动
7. **空白区菜单**：新建连接、关闭已断开、关闭全部
8. **持久化**：重命名/重排后重启，别名与会话顺序恢复
9. **同主机多会话**：复制同主机两次，两个标签独立，重启均恢复
10. **配置导入导出**：导出不含 sessions，导入忽略 sessions

### 10.2 回归验证

- [ ] 左侧主机列表点击打开主机仍走 `openSession`（同主机复用）
- [ ] `+` 快速连接、空白区"新建连接…"正常
- [ ] 终端输入、复制粘贴正常
- [ ] 配置导入导出功能正常

---

## 11. 风险与注意事项

| 风险 | 应对 |
|---|---|
| `openSession` 与 `duplicateSession` 连接逻辑重复 | 抽取内部 `connectSession(id)` 共用，避免分叉 |
| `restoreSessions` 旧格式读取崩溃 | 严格类型判断，非 `SessionEntry[]` 直接清空，不抛异常 |
| inline 编辑输入框撑破布局 | `maxlength=50` + `max-width:100%` + 父 `overflow:hidden` 三重保障 |
| ContextMenu 按键拦截误杀快捷键 | 修饰键组合（ctrl/meta/alt）一律放行，仅拦截无修饰键的普通字符 |
| 复制会话 IPC 异常导致 `duplicating` 永久锁死 | 8s `setTimeout` 兜底，`finally` 清理 + 超时清理双保险 |
| 批量关闭时 `sessions` 数组遍历索引漂移 | 先快照 id 数组，再逐个 `closeSession` |

---

## 12. 提交顺序

建议按阶段顺序提交，每阶段独立 commit：

1. `feat(tabs): session alias 字段 + sessions 持久化格式改为 SessionEntry[]`
2. `feat(tabs): 新增批量关闭函数 closeSessionsToLeft/Right/Other/Disconnected/All + setSessionAlias`
3. `feat(tabs): 新增 duplicateSession 复制会话（含并发保护与超时兜底）`
4. `feat(context-menu): 菜单可见期间按键不穿透终端，放行系统快捷键`
5. `feat(tabs): 标签右键菜单 + 空白区菜单 + 中键关闭`
6. `feat(tabs): 标签 inline 原地重命名编辑`
7. `feat(tabs): 会话属性弹窗 + SFTP 面板入口`
8. `docs: 登记开发计划到 config.mts`
