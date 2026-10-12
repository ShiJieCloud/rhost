# AI 跨会话记忆（M3.3）设计方案

> description: 跨会话记忆的提取、存储、注入与管理设计，复用 M3.2 目录约定与 system 注入装配点
>
> created: 2026-10-12 12:15:04
>
> updated: 2026-10-12 12:15:04
>
> author:

---

## 1. 设计目标与硬约束

> 范围：M3.3 阶段「记忆 Memory」设置分组的落地——记忆条目数据结构、后端原子落盘 IPC、对话完成后自动提取、前端 system 注入装配、设置界面查看/增删/清空管理。
> 边界：不覆盖向量/语义检索（无本地 embedding 设施，MVP 按时间序全量注入）；不做消息级「记住这条」按钮；不做记忆的文件管理器目录投放（与技能/规则的用户自述文件定位不同，记忆由应用持有）。
> 关联代码：`src-tauri/src/ai.rs`、`frontend/src/lib/ai.ts`、`frontend/src/components/AiChatPanel.vue`、`frontend/src/components/SettingsModal.vue`、`frontend/src/stores/settings.ts`

| 目标 | 量化口径 |
|---|---|
| 提取不阻塞对话 | 提取在 `ai-done(completed)` 后后台异步执行，对话主流程 0 额外等待 |
| 提取频率可控 | 未提取新增内容 ≥ 4000 字符才触发一轮提取（约 2k tokens），一轮对话最多触发 1 次 |
| 注入预算可控 | 单条记忆 ≤ 200 字符；注入合计 ≤ 8KB（UTF-8 字节），超出丢弃最旧 |
| 存储有界 | 条数上限即 `aiMemoryLimit`（默认 100，10~1000），超出淘汰最旧 |
| 失败无害 | 目录缺失/读取失败/提取失败/解析失败 100% 降级为空注入或跳过本轮，对话主流程不受影响 |

必须遵守的既有项目约束：

- AI 上下文注入规则：首页对话不注入主机信息，工作台仅注入低敏 `user@ip:port`——记忆为**用户级蒸馏事实**，全局统一注入两个 scope，与技能/规则的「全局统一」裁决一致（见 §6.3 论证）；
- 终端内容零自动回传边界不变：记忆提取的输入是**对话消息本身**（已发送给模型的内容），不引入任何终端输出；
- 设置项集中在 `frontend/src/stores/settings.ts`，默认值显式声明；
- 错误即协议：IPC 返回字符串错误带前缀，前端据此分流；
- 文档联动：设置项变更须同步 `docs/reference/global-settings.md`（见 §6.5）。

## 2. 必须遵守的既有项目约束

- 后端 AI 模块保持无会话状态：`src-tauri/src/ai.rs` 现有命令均为一次性调用，记忆读写同样无状态、原子写（`tmp + rename`，对齐 `ai_save_chat` 先例）；
- `app_config_dir/ai/` 目录结构既有（`chats/`、`skills/`、`rules/` 平级），新增 `memory/` 子目录存 `memory.json`，不新开顶层目录；
- `AiWireMessage` 与 OpenAI 消息同构、后端透传——注入发生在前端 `buildWireMessages()`，后端协议层零改动；
- system 注入先例已存在四条（规则、技能、滚动摘要、SSH 会话上下文），记忆沿用同一注入点与拼装风格；
- 配置导出仅覆盖 `app_config.json` 的 connections/settings/keys/groups/ui_state/quick_connect_history——`ai/memory/` 与会话记录同属本地运行时数据，**不参与导出**（对齐 known_hosts 与 chats 的既有口径，无需改动 config_io）。

## 3. 现状盘点与差距

| 位置 | 现状 | 差距 |
|---|---|---|
| `stores/settings.ts` | `aiMemoryEnabled`（默认 true）、`aiMemoryLimit`（默认 100）仅持久化，无任何消费方 | 无记忆条目存储结构、无注入、无提取逻辑 |
| `SettingsModal.vue` AI 分组 | 记忆分组两行均带 `todo: 仅持久化，待接入 AI 服务` | 缺记忆列表、逐条删除、清空、手动添加 |
| `AiChatPanel.vue` `buildWireMessages()` | 规则/技能/摘要/会话上下文四条 system 注入已稳定 | 缺记忆注入段 |
| `src-tauri/src/ai.rs` | 会话记录落盘 `ai/chats/`；技能/规则扫描已落地 | 缺记忆条目读写命令与提取调用 |
| `docs/reference/global-settings.md` | §9.5 记忆两行标 ⚠️ 未开发 | 落地后需同步为 ✅ |

结论：本设计补齐三块——后端记忆条目存储与提取 IPC、前端注入装配与提取触发、设置界面记忆管理。

## 4. 目标与非目标

### 目标

1. 数据结构：`app_config_dir/ai/memory/memory.json`，单文件 JSON 数组，原子写；
2. 后端命令：`ai_list_memories`（读取全量）、`ai_save_memories`（覆盖写入，含上限淘汰）、`ai_clear_memories`（清空）、`ai_extract_memories`（非流式提取调用，对齐 `ai_summarize`）；
3. 前端注入：`buildWireMessages()` 按「规则 → 技能 → 记忆 → 摘要 → 会话上下文」顺序装配 system 消息；
4. 提取触发：对话完成后（`ai-done` reason=completed）后台检查，未提取新增内容 ≥ 4000 字符时发起一轮提取；
5. 设置界面：记忆分组改为「总开关 + 条数上限 + 记忆列表（逐条删除）+ 手动添加 + 清空」，移除两行 `todo` 标记；
6. 预算与截断：单条 ≤ 200 字符、注入合计 ≤ 8KB，超限丢弃最旧。

### 非目标

- 不做向量/语义检索（无本地 embedding 设施，MVP 按时间序全量注入）；
- 不做消息级「记住这条」按钮（后续里程碑可加）；
- 不做记忆的文件管理器目录投放（记忆由应用持有，非用户自述文件）；
- 不做记忆的导入导出（本地运行时数据，不参与配置导出）；
- 不做按 scope 隔离存储（用户裁决：全局统一，见 §6.3）。

## 5. 总体架构

```text
app_config_dir/ai/
├── chats/            （既有，会话记录）
├── skills/           （既有，技能文件）
├── rules/            （既有，规则文件）
└── memory/           （新增）
    └── memory.json   （单文件 JSON 数组，原子写）

┌─ 设置界面（管理路径）─────────────────────────────────────┐
│ SettingsModal ──► ai_list_memories()                     │
│                     └─► 列表渲染（内容/时间/来源/删除）   │
│                   ai_save_memories(items)                │
│                     └─► 手动添加/逐条删除后覆盖写入        │
│                   ai_clear_memories()                    │
│                     └─► 清空确认后删除文件                 │
└───────────────────────────────────────────────────────────┘

┌─ 发送路径（注入路径）─────────────────────────────────────┐
│ AiChatPanel.startStream()                                │
│   └─► buildWireMessages(assets, memories)                │
│         ├─ [规则 system]   （既有）                       │
│         ├─ [技能 system]   （既有）                       │
│         ├─ [记忆 system]   ◄── ai_list_memories          │
│         ├─ [摘要 system]   （既有）                       │
│         ├─ [会话上下文 system]（既有，最靠后）            │
│         └─ 历史消息原文                                   │
└───────────────────────────────────────────────────────────┘

┌─ 提取路径（对话完成后后台）───────────────────────────────┐
│ AiChatPanel.onDone(completed)                             │
│   └─► maybeExtractMemories()                             │
│         ├─ 未提取新增内容 ≥ 4000 字符？                   │
│         ├─ 是 → 组装提取请求（指令 + 当前记忆 + 新对话）   │
│         ├─ ai_extract_memories(messages)                  │
│         ├─ 解析返回 JSON 数组 → 合并去重 → 截断           │
│         └─ ai_save_memories(merged)                      │
│         └─ 失败静默（console 记录，不影响对话）            │
└───────────────────────────────────────────────────────────┘
```

关键取舍——**记忆由应用持有，非用户自述文件**：技能/规则是用户主动维护的 `.md` 文件，天然适合文件管理器投放；记忆是模型从对话中蒸馏的结构化条目，需要 id、时间戳、来源等元数据，单文件 JSON 比目录平铺 `.md` 更贴合。用户通过设置界面查看/增删/清空，不直接编辑文件。

## 6. 详细设计

### 6.1 记忆条目数据结构

```rust
/// 一条跨会话记忆（serde camelCase）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryItem {
    /// 唯一标识（UUID 短码，前端生成）
    pub id: String,
    /// 记忆内容（≤ 200 字符，提取时截断）
    pub content: String,
    /// 创建时间（epoch ms）
    pub created_at: u64,
    /// 来源 scope（提取时的对话归属，仅供展示）
    pub source_scope: String,
}
```

- `id`：前端用 `crypto.randomUUID()` 生成，提取合并时保留已有条目的 id；
- `content`：单条 ≤ 200 字符（约 100 tokens，足够一条偏好/事实描述），提取指令中明确要求模型输出简洁条目；前端保存时截断至 200 字符；
- `sourceScope`：记录提取时的对话归属（`global` 或 SSH 会话 id），仅用于设置界面展示来源，不影响注入范围（全局注入）；
- 存储顺序：按 `createdAt` 升序（最旧在前），淘汰时从头部移除。

### 6.2 后端命令（`src-tauri/src/ai.rs`）

```rust
#[tauri::command]
pub fn ai_list_memories(app: AppHandle) -> Result<Vec<MemoryItem>, String>

#[tauri::command]
pub fn ai_save_memories(app: AppHandle, items: Vec<MemoryItem>) -> Result<(), String>

#[tauri::command]
pub fn ai_clear_memories(app: AppHandle) -> Result<(), String>

#[tauri::command]
pub async fn ai_extract_memories(app: AppHandle, messages: Vec<AiMessage>) -> Result<String, String>
```

- `ai_list_memories`：读取 `memory.json`，不存在返回空 Vec，解析失败返回空 Vec（失败无害）；不报错；
- `ai_save_memories`：覆盖写入 `memory.json`（`tmp + rename` 原子写）；写入前按 `aiMemoryLimit` 淘汰最旧（从头部移除至 ≤ limit）；`create_dir_all` 自动创建目录；
- `ai_clear_memories`：删除 `memory.json`，不存在视为成功（对齐 `ai_clear_chat`）；
- `ai_extract_memories`：非流式调用（对齐 `ai_summarize` 的 60s timeout、非流式响应体解析）；输入为提取指令 + 当前记忆清单 + 新对话片段，输出为 JSON 数组字符串（模型直接输出 JSON，前端解析）；返回 `String` 而非 `Vec<MemoryItem>`——模型输出可能不完美，前端负责解析、校验、合并、生成 id；
- 四个命令按既有 IPC 先例登记 `slow_span!` 埋点；记忆内容不入日志。

提取指令（`EXTRACT_MEMORIES_PROMPT`）要点：

```text
你是记忆提取器。从对话历史中提取值得长期记住的用户偏好、项目上下文、重要事实。
要求：
1. 只提取跨会话有价值的信息（用户偏好、项目约定、常用环境、重要决策）；
2. 不提取一次性问题、闲聊、已解决的临时故障；
3. 不记录密码、私钥、API Key、Token 等敏感凭据；
4. 每条记忆为简洁中文陈述句，≤ 200 字符；
5. 输出为 JSON 数组，每项含 content 字段；若无可提取内容，输出 []；
6. 直接输出 JSON，不要包裹 markdown 代码块，不要评论。
若输入中包含「已有记忆」，将新提取的内容与已有记忆合并去重后输出完整数组。
```

### 6.3 前端注入装配（`AiChatPanel.vue`）

**注入位置与顺序**：`buildWireMessages()` systems 数组中，记忆插在技能之后、摘要之前：

1. 规则 system（既有）；
2. 技能 system（既有）；
3. **记忆 system**（新增）：`aiMemoryEnabled && memories 非空` 时注入；
4. 摘要 system（既有）；
5. 会话上下文 system（既有，最靠后）。

**全局注入论证**：记忆是用户级蒸馏事实（偏好、项目约定、常用环境），与技能/规则的「用户自述内容」定位一致——M3.2 已裁决全局统一注入两个 scope，记忆沿用同一裁决。首页 scope 不注入主机信息的规则针对的是**当前连接状态**（`buildSessionContext()` 返回 null），记忆内容不含实时连接信息，不违反该规则。

**注入模板**（合并为单条 system，与技能/规则同风格）：

```text
# 跨会话记忆（以下是你与用户的历史交互中记住的事实）

- 用户偏好使用 zsh，默认编辑器为 nvim
- 生产环境部署在 10.0.0.0/24 网段，变更需先在 staging 验证
- 项目使用 pnpm 而非 npm
```

**预算控制**：单条 ≤ 200 字符（保存时截断）；注入合计 ≤ 8KB UTF-8 字节（`TextEncoder` 计量，与技能/规则同口径）；超限时从**最旧**开始丢弃，在末尾追加「（其余 N 条记忆因总量超限未注入）」。8KB 取值理由：与技能/规则单类预算 16KB 同量级，记忆条目更短（200 vs 8KB），100 条 × 200 字符 ≈ 20KB 最坏情况，8KB 预算可注入约 40 条，覆盖绝大多数场景。

**异步化方案**：与技能/规则一致——`startStream` 调用点先 `await Promise.all([aiListPromptAssets(), aiListMemories()])`，再把 assets 与 memories 作为参数传入 `buildWireMessages(assets?, memories?)`；每轮工具循环续跑都重新拉取，永远最新。

### 6.4 提取触发与合并逻辑（`AiChatPanel.vue`）

**触发时机**：`ai-done` 事件 reason=completed 时，在 `maybeSummarize()` 之后追加调用 `maybeExtractMemories()`。

**触发条件**（全部满足才发起）：

1. `isTauri && !streaming.value && !extracting`（防重入，对齐 `summarizing` 先例）；
2. `aiMemoryEnabled` 为 true；
3. 未提取新增内容 ≥ 4000 字符（`EXTRACT_TRIGGER_CHARS = 4000`，约 2k tokens，避免每轮短对话都触发）；

**未提取新增内容的界定**：新增 `extractedUpTo` ref（对齐 `summaryUpTo`），记录上次提取覆盖的最后一条消息 id；触发时取 `extractedUpTo` 之后、`summaryUpTo` 之前的消息（已摘要部分不再重复提取——摘要已压缩，提取应面向原始对话）。若 `summaryUpTo` 为空则取全部消息。统计这些消息的 content 字符数（`reduce` 求和，对齐 `maybeSummarize` 的口径）。

**提取请求组装**：

```text
[input] AiWireMessage[]:
  - 若已有记忆非空：{ role: 'user', content: '【已有记忆】\n' + JSON.stringify(现有记忆内容数组) }
  - 追加待提取消息原文（role + content）
```

**响应处理**：

1. 模型返回 JSON 字符串，前端 `JSON.parse` 为数组；
2. 校验每项为含 `content` 字段的对象，`content` 为字符串且非空；
3. 每条生成 `id: crypto.randomUUID()`、`createdAt: Date.now()`、`sourceScope: scope.value`；
4. 与现有记忆合并：按 `content` 精确匹配去重（新条目与旧条目 content 相同则保留旧条目及其 id/createdAt）；
5. 按 `createdAt` 升序排序，若总数 > `aiMemoryLimit` 则从头部淘汰最旧；
6. 调用 `ai_save_memories(merged)` 覆盖写入；
7. 更新 `extractedUpTo = 本次提取覆盖的最后一条消息 id`；
8. 失败静默：console.warn 记录，不更新 `extractedUpTo`（下次完成后重试）。

**与滚动摘要的关系**：摘要面向「当前会话的上下文压缩」，记忆面向「跨会话的用户级事实」。两者独立触发、互不依赖——摘要覆盖 `summaryUpTo`，提取覆盖 `extractedUpTo`，同一轮对话完成后可先后执行。摘要是会话级（随 chats 落盘），记忆是全局级（独立 memory.json）。

**`extractedUpTo` 的持久化**：随会话记录落盘（`AiChatRecord` 新增可选字段 `extractedUpTo?: string`），重启后从上次位置继续，避免重复提取已处理内容。

### 6.5 设置界面（`SettingsModal.vue`）

记忆分组改造：

- 保留总开关行（`aiMemoryEnabled`），移除 `todo` 标记；
- 保留条数上限行（`aiMemoryLimit`），移除 `todo` 标记；
- 新增「记忆列表」行：新增 row kind（如 `kind: 'memoryList'`），与 `assetList` 同构——rows 是静态 schema，memories 数据由组件内独立 `ref` 持有，schema 行只携带标识；每行渲染：内容（超长截断显示）、创建时间（格式化）、来源 scope（global 显示「全局」，否则显示会话 id 前 8 位）、删除按钮；空列表显示引导文案「对话中产生的偏好与事实将自动记录在这里」；
- 新增「手动添加」行：输入框 + 添加按钮，输入校验（非空、≤ 200 字符），添加后调用 `ai_save_memories` 追加；
- 新增「清空记忆」按钮行：危险操作，二次确认后调用 `ai_clear_memories`；
- 列表数据加载时机：设置弹窗打开且切到 AI 页签时拉取一次（对齐技能/规则列表）；
- 隐私提示：分组 desc 追加「记忆内容将随对话发送给模型服务商」。

文档联动（同一 PR）：

- `docs/reference/global-settings.md` §9.5：两行状态从 ⚠️ 未开发改为 ✅，补充记忆列表/手动添加/清空的设置项说明。

### 6.6 前端 API 封装（`lib/ai.ts`）

```ts
/** 一条跨会话记忆（serde camelCase） */
export interface AiMemoryItem {
  id: string
  content: string
  createdAt: number
  sourceScope: string
}

export function aiListMemories(): Promise<AiMemoryItem[]>
export function aiSaveMemories(items: AiMemoryItem[]): Promise<void>
export function aiClearMemories(): Promise<void>
export function aiExtractMemories(messages: AiWireMessage[]): Promise<string>
```

`AiChatRecord` 新增可选字段：

```ts
export interface AiChatRecord {
  // ... 既有字段 ...
  /** 记忆提取覆盖的最后一条消息 id；此后消息待提取 */
  extractedUpTo?: string
}
```

## 7. 安全与降级

- 敏感数据：提取指令明确禁止记录密码/私钥/API Key/Token；记忆内容为用户级蒸馏事实，发送给模型服务商属预期行为，设置界面明示；记忆内容**不入日志**（applog 不记录 content）；
- 路径安全：后端固定解析 `app_config_dir/ai/memory/`，前端不传路径，无路径穿越面；
- 降级矩阵：
  - `memory.json` 不存在 → 空列表，不注入；
  - `memory.json` 解析失败 → 空列表，不注入（fail-open，对齐 known_hosts 解析失败按无记录处理的先例）；
  - 提取调用失败（网络/超时/模型错误） → 静默跳过本轮，console.warn 记录，不更新 `extractedUpTo`；
  - 提取返回非法 JSON → 跳过本轮，console.warn 记录；
  - 保存失败 → console.error 记录，记忆在内存中保留（下次提取时重试保存）；
- 预算兜底：单条 200 字符 + 注入合计 8KB 双闸，防误投超长内容撑爆上下文；
- 隐私边界：记忆提取的输入是对话消息本身（已发送给模型的内容），不引入终端输出，不扩大数据暴露面。

## 8. 测试与验证

- 后端单测（`ai.rs` `#[cfg(test)]`）：
  - `ai_save_memories` 上限淘汰：写入超过 limit 的条目，验证最旧被淘汰；
  - `ai_list_memories` 解析失败降级：写入非法 JSON，验证返回空 Vec 而非报错；
  - `ai_clear_memories` 幂等：文件不存在时调用不报错；
- 前端：
  - 提取触发条件：字符数不足 4000 时不触发；
  - 合并去重：新旧 content 相同时保留旧 id/createdAt；
  - 注入预算：超 8KB 时最旧条目被丢弃并标注；
- 手工验证：
  - 开启记忆后完成一轮 ≥ 4000 字符的对话，检查设置界面记忆列表出现条目；
  - 关闭总开关后对话，验证记忆不注入、不提取；
  - 手动添加/删除/清空功能正常；
  - 重启应用后 `extractedUpTo` 从上次位置继续，不重复提取。

## 9. 未决问题

- **提取质量**：模型可能提取低价值或过时信息。当前依赖提取指令约束 + 用户手动管理兜底；若实际使用中噪声过高，后续可考虑引入「记忆置信度」或「用户确认后才入库」机制。
- **合并去重粒度**：当前按 content 精确匹配去重，语义重复（如「用户偏好 zsh」vs「用户喜欢用 zsh」）无法识别。向量检索或 LLM 语义去重为后续扩展点。
