# AI 技能/规则配置（M3.2）设计方案

> description: 技能与规则文件的目录约定、后端扫描 IPC、逐条开关与 system prompt 注入装配，含设置界面管理与预算控制
>
> created: 2026-10-12 11:07:53
>
> updated: 2026-10-12 11:37:44
>
> author:

---

## 1. 设计目标与硬约束

> 范围：M3.2 阶段「技能 SKILL / 规则 Rules」两个设置分组的落地——文件目录约定、后端扫描读取 IPC、前端逐条开关与 system prompt 注入装配、设置界面列表管理。
> 边界：不覆盖「记忆 Memory」分组（独立里程碑）；不覆盖技能声明工具/触发器（模型无读文件工具，无法按需加载，见 §9）；不做按主机/scope 隔离（用户裁决：全局统一）；不做内置文件编辑器。
> 关联代码：`src-tauri/src/ai.rs`、`src-tauri/src/ipc.rs`（reveal 先例）、`frontend/src/lib/ai.ts`、`frontend/src/components/AiChatPanel.vue`、`frontend/src/components/SettingsModal.vue`、`frontend/src/stores/settings.ts`

| 目标 | 量化口径 |
|---|---|
| 零延迟感知 | 发送路径上目录扫描 + 文件读取 < 10ms（本地小文件，无缓存实时读） |
| 预算可控 | 单文件注入 ≤ 8KB（超出截断并标记）；每类合计注入 ≤ 16KB |
| 零网络依赖 | 加载链路 0 额外网络请求，内容仅随既有 `ai_chat_stream` 请求发出 |
| 失败无害 | 目录缺失/读取失败/格式错误 100% 降级为空注入，对话主流程不受影响 |
| 发现即可用 | 新投放的 `.md` 文件默认启用，无需重启应用（发送时实时读取） |

必须遵守的既有项目约束：

- AI 上下文注入规则：首页对话不注入主机信息，工作台仅注入低敏连接信息（`user@ip:port`）——技能/规则为**用户自述内容**，全局统一注入两个 scope，不携带任何主机信息（用户决策）；
- 终端内容零自动回传边界不变：技能/规则是用户主动维护的静态文件，与终端输出无关；
- 设置项集中在 `frontend/src/stores/settings.ts`，默认值显式声明；
- 错误即协议：IPC 返回字符串错误带前缀，前端据此分流；
- 文档联动：设置项变更须同步 `docs/reference/global-settings.md`（见 §6.5）。

## 2. 必须遵守的既有项目约束

- 后端 AI 模块保持无会话状态：`src-tauri/src/ai.rs` 现有命令（`ai_chat_stream` / `ai_save_chat` 等）均为一次性调用，技能/规则读取同样无状态、每次实时扫盘，不引入缓存一致性负担；
- `app_config_dir/ai/` 目录结构既有（`chats/` 子目录存会话记录），新增 `skills/`、`rules/` 子目录与其平级，不新开顶层目录；
- `AiWireMessage` 与 OpenAI 消息同构、后端透传——注入发生在前端 `buildWireMessages()`，后端协议层零改动；
- system 注入先例已存在两条（滚动摘要、SSH 会话上下文，`AiChatPanel.vue` `buildWireMessages()`），技能/规则沿用同一注入点与拼装风格；
- 文件管理器打开目录先例：`reveal_log_storage_dir`（`src-tauri/src/ipc.rs`，tauri-plugin-opener），新命令复用同一插件。

## 3. 现状盘点与差距

| 位置 | 现状 | 差距 |
|---|---|---|
| `stores/settings.ts` | `aiSkillsEnabled/aiSkillsPath/aiRulesEnabled/aiRulesPath` 四个 key 仅持久化，无任何消费方 | 路径 key 语义无效（SSH 客户端无「项目根目录」），缺逐条开关状态 |
| `SettingsModal.vue` AI 分组 | 技能/规则分组仅开关 + 路径输入，标 `todo: 仅持久化` | 缺文件列表、逐条开关、目录入口 |
| `AiChatPanel.vue` `buildWireMessages()` | 摘要 system + 会话上下文 system 两条注入已稳定 | 缺技能/规则注入段 |
| `src-tauri/src/ai.rs` | 会话记录落盘 `app_config_dir/ai/chats/`；无文件扫描能力 | 缺 skills/rules 目录扫描与读取命令 |
| `docs/reference/global-settings.md` | 全文无 AI 章节（M1/M2 均未同步，既有缺口） | 借本次设置项变更补齐 AI 章节 |

结论：本设计补齐三块——后端目录扫描 IPC、前端注入装配与逐条开关、设置界面列表管理；同时清理两个无效占位路径 key。

## 4. 目标与非目标

### 目标

1. 目录约定：`app_config_dir/ai/skills/` 与 `app_config_dir/ai/rules/`，单文件平铺，`.md` 即一条技能/规则；
2. 后端命令：`ai_list_prompt_assets`（扫描 + 读取 + 截断）、`ai_reveal_prompt_dir`（文件管理器打开目录）；
3. 前端注入：`buildWireMessages()` 按「规则 → 技能 → 摘要 → 会话上下文」顺序装配 system 消息；
4. 逐条开关：`aiSkillsDisabled` / `aiRulesDisabled` 文件名禁用名单（默认 `[]` 即全部启用）；
5. 设置界面：技能/规则分组改为「总开关 + 文件列表（逐条开关 + 预览）+ 打开目录」，移除两个路径输入行；
6. 预算与截断：单文件 8KB、每类合计 16KB，超限截断并标注。

### 非目标

- 不做内置编辑器（新建/编辑/删除文件由用户在文件管理器中完成）；
- 不做技能触发器、关键词自动激活、按 scope/主机绑定；
- 不做技能内容的分片按需加载（模型无读文件工具，注入即全量）；
- 不动「记忆 Memory」分组；
- 不做文件变更的文件系统监听（发送时实时读取，天然最新）。

## 5. 总体架构

```text
app_config_dir/ai/
├── chats/            （既有，会话记录）
├── skills/           （新增，*.md 平铺，用户投放）
└── rules/            （新增，*.md 平铺，用户投放）

┌─ 设置界面（管理路径）────────────────────────────────┐
│ SettingsModal ──► ai_list_prompt_assets()            │
│                     └─► 列表渲染（名称/描述/大小/开关）│
│                   ai_reveal_prompt_dir(kind)          │
│                     └─► 系统文件管理器打开目录        │
└──────────────────────────────────────────────────────┘

┌─ 发送路径（注入路径）────────────────────────────────┐
│ AiChatPanel.sendMessage()                            │
│   └─► buildWireMessages()                            │
│         ├─ [规则 system]   ◄── ai_list_prompt_assets │
│         ├─ [技能 system]   ◄── （过滤 disabled 名单） │
│         ├─ [摘要 system]   （既有）                   │
│         ├─ [会话上下文 system]（既有，最靠后）        │
│         └─ 历史消息原文                              │
└──────────────────────────────────────────────────────┘
```

关键取舍——**读取无缓存、发送时实时扫描**：本地目录扫描 + 小文件读取为亚毫秒级，引入 mtime 缓存只会带来「用户改了文件但注入旧内容」的一致性负担；每次发送实时读，永远最新，实现最简。管理路径与注入路径共用同一命令，后端无状态。

## 6. 详细设计

### 6.1 目录与文件格式

- 目录：`app_config_dir/ai/skills/`、`app_config_dir/ai/rules/`；首次扫描或 reveal 时 `create_dir_all` 自动创建，用户无需手工建目录；
- 扫描规则：仅顶层 `.md` 文件（扩展名小写比较）；跳过隐藏文件（`.` 开头）、子目录、符号链接（避免逃逸出配置目录读入意外内容）；按文件名字典序排序，保证注入顺序稳定；
- 文件格式：纯 markdown，可选 YAML frontmatter：

```markdown
---
name: nginx 排障
description: Nginx 502/504 排查步骤与常用命令
---

正文内容……
```

- frontmatter 仅识别 `name` / `description` 两个标量字段；无 frontmatter 时 `name` 取文件名去扩展名，`description` 为空。**解析实现：手写极简解析（`---` 围栏内逐行 `key: value`，只认这两个键），不引入 YAML crate**——项目依赖仅有 `serde`/`serde_json`，为两个标量字段新增 serde_yaml 不值；解析失败（无闭合围栏、行无冒号等）不报错，按无 frontmatter 处理（正文仍注入，含原文 frontmatter 块）；
- 标识：文件名（含扩展名）作为禁用名单与列表渲染的稳定 key——重命名文件即失效旧开关状态，属于可接受行为（禁用名单存的是用户意图，文件都没了意图自然失效）。

### 6.2 后端命令（`src-tauri/src/ai.rs`）

```rust
struct PromptAsset {
    file_name: String,        // "nginx-debug.md"，稳定 key
    name: String,             // frontmatter name 或文件名去扩展
    description: Option<String>,
    content: String,          // 截断后的正文（含 frontmatter 时保留原文整体）
    truncated: bool,          // 单文件 8KB 截断标记
    size_bytes: u64,          // 原始文件大小
}

struct PromptAssets {
    skills: Vec<PromptAsset>,
    rules: Vec<PromptAsset>,
}

#[tauri::command]
pub async fn ai_list_prompt_assets(app: AppHandle) -> Result<PromptAssets, String>

#[tauri::command]
pub async fn ai_reveal_prompt_dir(app: AppHandle, kind: String) -> Result<String, String>
// kind ∈ {"skills", "rules"}，非法值报错；create_dir_all 后复用 opener 插件 reveal
```

- `ai_list_prompt_assets` 无参数：目录由后端固定解析，前端不传任何路径，**无路径穿越面**；禁用过滤在前端完成（§6.3），后端始终返回全量清单（设置界面需要完整列表渲染开关）；
- 两个新命令按既有 IPC 先例登记 `slow_span!` 埋点（对齐 `reveal_log_storage_dir`）；文件内容不入日志；
- 单文件读取上限：先 `metadata` 取大小，> 8KB 时只读前 8KB（`take`），`truncated = true`；8KB 取值理由：约 2k~4k tokens（中英混合），足够一份排障手册级技能，又防止用户误投大文件撑爆上下文；
- 单文件读取失败（权限/编码）：跳过该文件，不影响其余文件返回；UTF-8 解码失败按 `from_utf8_lossy` 容错；
- 目录不存在/创建失败：返回空 Vec，不报错——对话主流程不因技能目录问题中断。

### 6.3 前端注入装配（`AiChatPanel.vue` `buildWireMessages()`）

system 消息顺序（注意力靠后原则：越稳定、越全局的约定越靠前）：

1. **规则 system**（合并为一条）：`aiRulesEnabled && rules 非空` 时注入；
2. **技能 system**（合并为一条）：`aiSkillsEnabled && skills 非空` 时注入；
3. 摘要 system（既有）；
4. 会话上下文 system（既有，最靠后贴近对话）。

合并为单条而非每文件一条：部分 OpenAI 兼容层对多 system 消息支持不一，合并最稳。模板：

```text
# 用户自定义规则（遵循以下约定响应）

## nginx 排障（rules/nginx.md）

<正文>

## 另一规则（rules/other.md，内容已截断）

<正文>
```

- 过滤逻辑：前端按 `aiSkillsDisabled` / `aiRulesDisabled`（存 `file_name`）过滤；`truncated` 文件在标题行追加「（内容已截断）」；
- **异步化方案**：`buildWireMessages()` 现为同步纯函数，而 `ai_list_prompt_assets` 是异步 invoke——保持 `buildWireMessages` 纯函数不变，其唯一调用点（startStream，`AiChatPanel.vue` 内 `aiChatStream(...)` 调用处）改为先 `await aiListPromptAssets()` 再把 assets 作为参数传入；startStream 是每轮工具循环的入口，故**每轮续跑都重新拉取**，与「永远最新」一致，不存在轮次间使用陈旧快照的问题；
- 合计预算：过滤后拼接时累计 **UTF-8 字节数**（`new TextEncoder().encode(s).length`，与后端单文件 8KB 字节口径一致；不用 JS `string.length`——UTF-16 code unit 对中文计量偏差约 3 倍），每类超 16KB 时按顺序丢弃靠后文件，并在该条 system 末尾追加「（其余 N 个文件因总量超限未注入）」；16KB 取值理由：与滚动摘要触发阈值 `SUMMARY_TRIGGER_CHARS = 8000` 同量级，规则 + 技能合计最坏 32KB 注入仍处中小上下文规模；
- 注入时机：手动发送、重新生成、工具循环续跑全部汇入 startStream 单一入口，技能/规则天然每轮都在，无额外接线点；
- 首页 scope 同样注入（全局统一裁决）；技能/规则内容不含主机信息，不违反上下文注入规则。

### 6.4 设置项变更（`stores/settings.ts`）

| key | 变更 | 默认 | 说明 |
|---|---|---|---|
| `aiSkillsEnabled` | 保留 | `true` | 技能总开关 |
| `aiRulesEnabled` | 保留 | `true` | 规则总开关 |
| `aiSkillsPath` | **移除** | — | 占位 key，从未被消费；目录改由后端固定解析 |
| `aiRulesPath` | **移除** | — | 同上 |
| `aiSkillsDisabled` | 新增 | `[]` | 技能禁用名单（`file_name` 数组） |
| `aiRulesDisabled` | 新增 | `[]` | 规则禁用名单（`file_name` 数组） |

配置导入导出：settings 节全量导出天然包含新 key；旧导出文件含 `aiSkillsPath`/`aiRulesPath` 时导入后成为无消费方的冗余 key，无害（不阻断、不报错），不专门做迁移清理。

### 6.5 设置界面（`SettingsModal.vue`）

技能/规则两个分组同构改造：

- 保留总开关行（`aiSkillsEnabled` / `aiRulesEnabled`），移除 `todo` 标记；
- 移除路径输入行；
- 新增「打开目录」按钮行（`ai_reveal_prompt_dir`），desc 注明实际目录路径（命令返回值）；
- 新增文件列表行：新增 row kind（如 `kind: 'assetList'`），注意 SettingsModal 的 rows 是**静态 schema**——assets 数据不放 schema，组件内独立 `ref` 持有，渲染 `kind === 'assetList'` 时从 ref 读取，schema 行只携带目标类别（skills/rules）标识；每行渲染：开关（写 `aiSkillsDisabled`/`aiRulesDisabled`）、名称、描述（无描述显示文件名）、大小、截断标记；空目录显示引导文案「将 .md 文件放入目录即自动生效」；总开关关闭时列表与「打开目录」沿用既有 `disabledKey: 'aiEnabled'` 机制禁用；
- 列表数据加载时机：设置弹窗打开且切到 AI 页签时拉取一次；点「打开目录」后提示用户投放文件，返回弹窗手动刷新（提供刷新按钮，不做 fs 监听）；
- 隐私提示：分组 desc 追加「文件内容将随对话发送给模型服务商」。

文档联动（同一 PR）：

- `docs/reference/global-settings.md`：新增 AI 章节（该文当前完全无 AI 设置，属既有缺口，本次至少覆盖技能/规则/终端执行三个分组的全部 key）；
- `docs/.vitepress/config.mts`：sidebar 登记本文档。

### 6.6 前端 API 封装（`lib/ai.ts`）

```ts
export interface AiPromptAsset {
  fileName: string
  name: string
  description?: string
  content: string
  truncated: boolean
  sizeBytes: number
}

export interface AiPromptAssets {
  skills: AiPromptAsset[]
  rules: AiPromptAsset[]
}

export async function aiListPromptAssets(): Promise<AiPromptAssets>
export async function aiRevealPromptDir(kind: 'skills' | 'rules'): Promise<string>
```

## 7. 安全与降级

- 路径安全：后端固定解析 `app_config_dir/ai/{skills,rules}`，前端不传路径；扫描跳过符号链接，杜绝目录逃逸；
- 敏感数据：技能/规则文件为用户主动维护的静态内容，发送给模型服务商属预期行为，设置界面明示；文件内容**不入日志**（applog 不记录 content）；
- 降级矩阵：目录不存在 → 空列表；单文件读取失败 → 跳过该文件；frontmatter 非法 → 按无 frontmatter 处理；`ai_list_prompt_assets` 整体失败 → 前端按空清单注入，对话不受影响，console 记录原因；
- 预算兜底：单文件 8KB + 每类 16KB 双闸，防误投大文件撑爆上下文。

## 8. 测试与验证

- 后端单测（`ai.rs` `#[cfg(test)]`）：扫描过滤（隐藏文件/子目录/非 `.md`/符号链接跳过、排序稳定）；frontmatter 极简解析（有/无/无闭合围栏/仅 name 无 description/含陌生键忽略）；8KB 截断与 `truncated` 标记；非 UTF-8 文件 lossy 容错；
- 前端：`buildWireMessages` 注入顺序（规则先于摘要先于会话上下文）、禁用名单过滤、16KB 合计预算（UTF-8 字节口径）丢弃逻辑；
- 手工验证：投放两个技能文件 + 一个规则文件 → 设置界面列表出现 → 关掉其中一条 → 发送消息抓包确认 system 内容只含启用项 → 重命名文件后开关状态重置为启用。

## 9. 未决问题

- 记忆 Memory 分组（`aiMemoryEnabled`/`aiMemoryLimit` 同为占位）：跨会话记忆的写入/淘汰/注入策略，独立里程碑设计；
- 技能与 tool calling 联动（如技能声明「本技能允许自动执行的命令模式」）：待 Plan-and-Act（M3.3）阶段一并考虑；
- 规则/技能是否区分首页与工作台注入（当前裁决全局统一，若用户后续提出场景化需求再评估）。
