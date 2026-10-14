# AI 多模态输入（图片与文本文件附件）设计方案

> description: 附件链路：粘贴/拖拽/选文件三入口采集图片与文本/代码文件、客户端压缩与内联、预览 chip、发送装配与回显，输入框外观对齐参考图
>
> created: 2026-10-13 09:43:42
>
> updated: 2026-10-13 10:34:36
>
> author:

---

## 1. 设计目标与硬约束

> 范围：本文覆盖工作台会话与首页全局会话的「附件」完整链路——三入口采集（粘贴 / 拖拽 / 选择文件）、客户端校验（图片压缩、文本白名单与二进制守卫）、附件预览 chip、随 user 消息发送（图片走 OpenAI 兼容 content parts、文本/代码文件内联为 `<file>` 块）、消息记录落盘与历史回显，以及输入区外观重构（对齐参考图：大圆角多行输入框 + 底部工具行）。
> 边界：不覆盖参考图中的 `@ Agent` 选择、`Auto ▾` 模型切换、麦克风语音输入（本次列为非目标，后续单独特性）；不覆盖图片与纯文本/代码之外的模态（音频 / 视频 / PDF/Office 等二进制文档）；不做模型 vision 能力探测与自动路由。
> 关联代码：`src-tauri/src/ai.rs`、`src-tauri/src/lib.rs`、`frontend/src/lib/ai.ts`、`frontend/src/components/AiChatPanel.vue`

| 目标 | 量化口径 |
|---|---|
| 三入口可用 | 粘贴、拖拽、点击 `+` 选择文件均可添加附件；三入口产出同构附件对象，共用一条分流与校验链路 |
| 图片体积有界（传输） | 单图长边 ≤1568px、编码后 ≤2MB；单条消息 ≤4 张、图片合计 ≤6MB；超限拒绝并提示 |
| 文本/代码体积有界 | 单文件原始 ≤1MB、单条消息 ≤4 个（扩展名白名单 + 二进制守卫）；超限/非法拒绝并提示 |
| 体积有界（落盘） | 图片落盘的是压缩后 data URL（典型 1568px JPEG <500KB）；文本文件落盘的是 UTF-8 原文，均非原始二进制 |
| 兼容不回归 | 纯文本消息仍以字符串 content 发送（不升级为 content parts）；纯文本文件消息同样保持字符串 |
| 低改造 | 不新增 Tauri 插件、不新增运行时依赖；后端仅把 `content` 字段由 `String` 放宽为透传 JSON |
| 失败不阻断 | 采集/压缩/读取/发送任一环节失败均降级提示，用户文本输入不丢失 |

必须遵守的既有项目约束见 §2。

## 2. 必须遵守的既有项目约束

- 后端只定义工具协议、不执行工具；工具循环（裁决 → 执行 → 续跑）由前端驱动（`ai.rs` `run_stream` 单轮透传，前端 `settleProposalRound` 推进）；
- `ai_chat_stream` 的 messages 由前端组装、后端原样透传到上游 `chat/completions`；后端不解释 content 语义，仅需能承载 content parts；
- 隐私边界：附件内容作为上下文发往模型服务商，与用户显式发送文本同口径；终端内容仍以用户显式选中发送为唯一入口，附件功能不触碰终端输出；
- 会话按 scope 完全隔离（`global` / 会话 id），附件随消息一并持久化，切换 scope 加载对应记录；
- 消息持久化 `persist()` 将 streaming 收敛为 stopped；附件作为 `AiChatMessage` 字段（`images` / `files`）随记录落盘；
- 摘要与记忆提取已使用纯文本投影（`maybeSummarize` / 记忆提取按 `m.content` 构造），图片 base64 与文本文件内容均不得进入这两条链路（`files` 为独立字段，非 `content`）；
- 计划模式（Plan-and-Act）生成计划时同样接受附件：`ai_plan_generate` 的 `content` 与对话链路同构（纯文本经内联后为字符串，含图片时为 content parts 数组），见 §6.7；
- 输入框既有行为保持：Enter 发送、`/plan ` 前缀检测、终端引用块并入 user 消息、编辑重发截断语义；
- 写操作授权语义（权限档 / 会话级豁免表 / 授权弹窗）不受本设计影响。

## 3. 现状盘点与差距

| 位置 | 现状 | 差距 |
|---|---|---|
| `src-tauri/src/ai.rs` `AiMessage` | `content: Option<String>`，随 `ChatRequest` 原样透传 | 字符串无法承载 `[{type:"text"},{type:"image_url",...}]`，图片消息反序列化失败 |
| `src-tauri/src/lib.rs` 建窗 | 未设置拖放处理器开关（Tauri 默认拦截 OS 文件拖放） | HTML5 `drop` 事件不携带 File，拖拽入口不可用 |
| `frontend/src/lib/ai.ts` `AiWireMessage` | `content: string \| null` | 需扩展为 `string \| AiContentPart[] \| null` |
| `frontend/src/lib/ai.ts` `AiChatMessage` | 无附件字段 | 需新增 `images?: AiImageAttachment[]` 与 `files?: AiTextAttachment[]` 承载落盘与回显 |
| `frontend/src/components/AiChatPanel.vue` | 单行 `<input>`；`buildWireMessages` 过滤 `content.trim()` 为空的 user 消息 | 无附件采集/预览/压缩/读取；无 `images` / `files` 装配；纯附件消息（无文本）会被过滤丢弃 |
| 输入区 UI | 单行 flex：[权限档] [Plan] [input] [发送] | 与参考图差距：需大圆角容器 + 多行 textarea + 底部工具行 + 圆形发送按钮 + `+` 附件按钮 |

结论：本设计补齐五块——后端 content 透传放宽、拖放开关放开以启用 HTML5 采集、前端图片与文本/代码附件采集/压缩/读取/预览/装配链路、输入区外观重构，以及附件类型判别（`kind`）驱动的分流与回显。

## 4. 目标与非目标

### 目标

1. 三种附件入口：粘贴（剪贴板截图/图片）、拖拽（从文件管理器拖入输入区）、点击 `+` 选择文件；图片与文本/代码文件共用同一容器与分流链路；
2. 客户端校验与压缩（图片）：格式白名单、原始大小上限、长边缩放、体积上限，压缩后生成统一附件对象；
3. 客户端校验（文本/代码）：扩展名白名单 + 无扩展名常见文件名匹配、原始大小上限、UTF-8 读取、二进制守卫（含 NUL 字节拒绝）；
4. 附件预览 chip 区：图片显示缩略图、文件显示文件图标，均带文件名，可单个移除；超出数量上限时拒绝新增并提示；
5. 发送装配：含图消息以 OpenAI 兼容 content parts 发送；文本/代码文件内联为 `<file name="…">…</file>` 块并入 user 文本；纯文本消息与纯文本文件消息均保持字符串 content；
6. 落盘与回显：附件随 user 消息持久化（`images` / `files`），历史消息中回显缩略图与文件名 chip；
7. 输入区外观对齐参考图：大圆角多行输入框、底部工具行（`+` / 权限档 / Plan）、右下绿色圆形上箭头发送按钮。

### 非目标

- 不实现 `@ Agent` 选择器、`Auto ▾` 模型切换下拉、麦克风语音输入（参考图中存在但本次显式排除）；
- 不支持音频、视频与 PDF/Office 等二进制文档（白名单不含，且二进制守卫会拒绝改扩展名绕过）；
- 不做服务端 vision 能力探测、不做「不支持视觉的模型」自动降级为文本描述；
- 不做附件上传到对象存储/CDN（图片以 data URL、文本以原文内联，纯本地到服务商）；
- 不做图片压缩质量、单文件大小阈值等可配置项（上限采用固定魔数，不新增设置项）。

## 5. 总体架构

```text
采集入口（三态）
   ├─ 粘贴：paste 事件 → clipboardData.files（仅图片）→ File
   ├─ 拖拽：dragover/drop 事件 → dataTransfer.files → File（过滤：图片 or 白名单文本）
   └─ 选择：点击 + → 隐藏 <input type="file" accept="图片 MIME + 文本扩展名"> → File
            │
            ▼
     addFiles(File[])：按 MIME 分流
            ├─ image/*  → addImageFile：白名单 → 原始大小 → 解码 → canvas 缩放重编码 → 体积/数量
            └─ 其他     → addTextFile ：扩展名白名单 → 原始大小 → UTF-8 读取 → 二进制守卫 → 数量
            │
            ▼
     附件列表 attachments: AiAttachment[]（kind: 'image' | 'text'，预览 chip，可移除）
            │
            ▼
     send()：content 可为空、附件非空即可发送（规划模式与附件互斥）
            │
            ▼
     buildWireMessages()：
        ├─ 文本文件 → 内联 `<file name="…">…</file>` 块并入 user 文本
        └─ 有图片   → 装配为 content parts 数组（文本在前、图片在后）；仅有文本文件时保持字符串
            │
            ▼
     ai_chat_stream（后端 content 字段原样透传）→ 上游 chat/completions
            │
            ▼
     AiChatMessage.images / .files 随 persist() 落盘 → 历史回显缩略图与文件 chip
```

与既有模块的关系：采集与压缩全在前端完成（Web API：`paste` / `drop` / `<input type=file>` / `canvas`），后端零新增依赖，仅把 `AiMessage.content` 放宽为透传 JSON。ReAct 工具循环、授权、计划等链路对本特性透明。

## 6. 详细设计

### 6.1 三入口采集（`AiChatPanel.vue`）

| 入口 | 触发 | 取值方式 |
|---|---|---|
| 粘贴 | 输入区 `@paste` | `e.clipboardData.files`（截图/复制图片，仅图片）；命中图片则 `preventDefault` 阻止二进制粘入 textarea |
| 拖拽 | 输入区 `@dragover.prevent` + `@drop` | `e.dataTransfer.files`；仅收图片与白名单文本文件，其余静默忽略；拖拽全程给输入区加 `.dragging` 高亮 |
| 选择 | `+` 按钮 → 隐藏 `<input type="file">` | `input.files`；`accept` 由「图片 MIME + 文本/代码扩展名」派生；选择后清空 `input.value` 以允许重复选同一文件 |

- **拖放开关**：Tauri 默认启用拖放处理器（拦截 OS 文件拖放并改发 Tauri 事件），导致 webview 收不到携带 File 的 HTML5 `drop`。在 `src-tauri/src/lib.rs` 建窗处追加 `.disable_drag_drop_handler()` 关闭该处理器，让 webview 原生处理 HTML5 拖放。经检索前端无任何 Tauri 拖放事件（`tauri://drag-drop`）消费方，放开无回归影响；
- 三入口统一收敛到 `addFiles(files: File[])`，内部按 MIME 分流：`image/*` → `addImageFile(file, name?)`，其余 → `addTextFile(file)`；`name` 为拖拽/选择可得文件名，粘贴缺省为「粘贴的图片」；
- 拖拽/选择可携带多个文件：逐个分流处理，各自受数量上限约束。

### 6.2 客户端校验（图片压缩 / 文本读取）

附件类型（落盘与回显形态，字段 camelCase，`kind` 为判别字段）：

```ts
/** 图片附件：dataUrl 为客户端压缩后的 data URL */
export interface AiImageAttachment {
  id: string
  kind: 'image'
  /** 压缩后 data URL（image/png / image/jpeg / image/webp），随消息落盘与回显 */
  dataUrl: string
  /** 展示名：拖拽/选择取文件名；粘贴为「粘贴的图片」 */
  name: string
  /** 压缩后近似字节数（base64 长度换算） */
  sizeBytes: number
  width: number
  height: number
}

/** 文本/代码附件：按 UTF-8 读入纯文本，发送时内联为 <file> 块 */
export interface AiTextAttachment {
  id: string
  kind: 'text'
  name: string
  /** 原始文件字节数 */
  sizeBytes: number
  /** UTF-8 文本内容 */
  text: string
}

export type AiAttachment = AiImageAttachment | AiTextAttachment
```

图片流程（`addImageFile`）：

1. **格式白名单**：`file.type ∈ {image/png, image/jpeg, image/webp, image/gif}`，否则 toast「不支持的图片格式」并拒绝；
2. **原始大小校验**：`file.size > 8MB` 拒绝（防超大文件读入内存）；
3. **解码**：`createImageBitmap(file)` 解码；解码失败 toast「图片解码失败」并拒绝；
4. **缩放**：长边 `> 1568px` 时按比例缩放到长边 1568（`canvas` 绘制），否则保持原尺寸；
5. **编码**：缩放过的图按原 MIME 重编码（PNG/WebP 走 `toDataURL(mime)`；JPEG 走 `image/jpeg` 质量 0.85；GIF 取首帧转 PNG，配合 `createImageBitmap` 首帧语义）；未缩放的图直接复用原始 data URL（避免无谓重编码损失）；
6. **压缩后体积校验**：data URL 解算字节数 `> 2MB` 拒绝（提示「图片过大，请压缩后重试」）；
7. **数量与总量校验**：当前图片数 `≥ 4` 时拒新；附件合计 `> 6MB` 时拒新；
8. **入列**：生成附件对象 push 到 `attachments`，`URL.revokeObjectURL` 释放临时资源（若使用）。

- 尺寸/编码参数依据：OpenAI 视觉建议长边不超过 1568px，超出部分会被服务端缩放且不提升识别质量，客户端先行缩放可同时降低 token 与传输体积；canvas 重编码统一到 JPEG 0.85（有损但体积显著下降），PNG 保留以维持截图文字清晰度；
- 压缩在 `createImageBitmap` + `OffscreenCanvas`（不可用时退化为 `document.createElement('canvas')`）中完成，主线程仅做一次解码与一次编码，单图耗时可接受。

文本/代码流程（`addTextFile`）：

1. **类型白名单**：扩展名（含点，小写）命中 `FILE_ALLOWED_EXTS`，或无扩展名的整名命中 `FILE_ALLOWED_NAMES`（`dockerfile` / `makefile` / `rakefile` / `gemfile` / `procfile` / `brewfile`）；否则 toast「不支持的文本/代码文件」并拒绝；
2. **原始大小校验**：`file.size > 1MB` 拒绝（内容将内联进上下文，过大推高 token 成本）；
3. **数量校验**：当前文本文件数 `≥ 4` 时拒新；
4. **读取**：`file.text()` 按 UTF-8 读取；读取失败 toast「文件读取失败」并拒绝；
5. **二进制守卫**：前 8192 字符内出现 NUL（`\u0000`）即视为二进制，toast「文件疑似二进制，已拒绝」并拒绝（防改扩展名绕过白名单）；
6. **入列**：生成附件对象 push 到 `attachments`（存 UTF-8 原文，不做截断）。

`FILE_ALLOWED_EXTS`（小写含点）分组如下，新增类型只需在数组追加：

| 分组 | 扩展名 |
|---|---|
| 文本 / 文档 | `.txt` `.md` `.markdown` `.log` `.csv` `.tsv` `.rst` `.adoc` `.tex` |
| 配置 / 数据 | `.json` `.json5` `.yaml` `.yml` `.toml` `.ini` `.conf` `.cfg` `.env` `.properties` `.xml` `.plist` `.editorconfig` |
| 代码 | `.py` `.js` `.mjs` `.cjs` `.ts` `.tsx` `.jsx` `.vue` `.svelte` `.rs` `.go` `.java` `.kt` `.kts` `.c` `.h` `.cc` `.cpp` `.hpp` `.cs` `.rb` `.php` `.swift` `.scala` `.sh` `.bash` `.zsh` `.fish` `.bat` `.ps1` `.sql` `.graphql` `.gql` `.proto` `.lua` `.pl` `.pm` `.r` `.dart` `.ex` `.exs` `.html` `.htm` `.css` `.scss` `.sass` `.less` `.styl` `.vim` `.gradle` `.cmake` `.tf` `.tfvars` `.hcl` `.diff` `.patch` `.mk` |

### 6.3 附件状态与预览 chip（`AiChatPanel.vue`）

- `const attachments = ref<AiAttachment[]>([])`（图片与文本文件混排，按 `kind` 判别）；
- chip 区位于 textarea 上方、终端引用块同层（`.ctx-strip`）：图片 chip 为缩略图（`<img :src="a.dataUrl">`，22×22 圆角）；文本文件 chip 为文件图标 + 文件名（等宽字体，截断省略）；两者均带移除按钮 `✕`；
- 删除单个：从 `attachments` 移除；发送成功后清空；
- 附件区仅在非空时渲染，不占位。

### 6.4 wire 装配（`lib/ai.ts` + `buildWireMessages`）

```ts
/** OpenAI 兼容 content part（后端透传） */
export type AiContentPart =
  | { type: 'text'; text: string }
  | { type: 'image_url'; image_url: { url: string } }

export interface AiWireMessage {
  role: 'user' | 'assistant' | 'system' | 'tool'
  content: string | AiContentPart[] | null
  tool_calls?: AiToolCall[]
  tool_call_id?: string
}
```

`buildWireMessages` 对 user 消息的装配：

```ts
// 文本/代码文件内联为 <file> 块（置于用户文本之后）
function withFileBlocks(content: string, files?: AiTextAttachment[]): string {
  if (!files?.length) return content
  const blocks = files
    .map(f => `<file name="${f.name.replace(/["<>]/g, '')}">\n${f.text}\n</file>`)
    .join('\n\n')
  return content.trim() ? `${content}\n\n${blocks}` : blocks
}

// 含附件 user 消息：文本文件内联；有图片时装配为 content parts（文本在前、图片在后）
if (m.role === 'user' && (m.images?.length || m.files?.length)) {
  const text = withFileBlocks(m.content, m.files)
  if (m.images?.length) {
    const parts: AiContentPart[] = []
    if (text.trim()) parts.push({ type: 'text', text })
    for (const img of m.images) parts.push({ type: 'image_url', image_url: { url: img.dataUrl } })
    wired.push({ role: 'user', content: parts })
  } else {
    wired.push({ role: 'user', content: text }) // 仅有文本文件：仍是字符串 content
  }
  continue
}
wired.push({ role: m.role, content: m.content })
```

- **可用性过滤放宽**：现有过滤 `m.status !== 'streaming' && (m.content.trim() || m.toolProposals?.length)` 需追加 `|| m.images?.length || m.files?.length`，否则纯附件消息（无文本）会被上下文丢弃；
- 纯文本消息与纯文本文件消息均保持 `content: string`，仅当消息含图片时才升级为 content parts 数组（兼容性 + 体积）；
- 文件名在 `<file name="…">` 属性中去除 `"` `<` `>`，避免破坏块标记；
- 含图消息在工具循环的每一轮续跑中都会重发（含 base64），token 与请求体随之增大——这是视觉对话的固有成本，MVP 接受；不额外做图片去重/缓存。文本文件同理在每轮续跑中重发原文；两者单条消息数量与体积均已有上限约束。

### 6.5 消息记录、标题与回显

```ts
// AiChatMessage 新增字段
images?: AiImageAttachment[]
files?: AiTextAttachment[]
```

- `send()` 放宽发送条件：`input` 非空**或** `attachments` 非空即可发送；user 消息 `content` 可为空串，`images` / `files` 分别携带两类附件快照（按 `kind` 拆分）；
- `persist()` 标题兜底：首条 user 消息 `content` 为空（纯附件）时，有图片取「图片」、否则取首个文本文件名；
- 历史回显：user 气泡在正文上方渲染缩略图行（图片，`title` 显示文件名）与文件 chip 行（文本/代码，文件图标 + 文件名），样式与输入区 chip 一致；
- 编辑重发（`startEdit`）同步把该消息的 `images` / `files` 回填到 `attachments`（图片补 `kind: 'image'` 判别字段），保证「编辑后重发」内容一致；删除消息时附件随之丢弃（本就在消息对象上）；
- 摘要 / 记忆提取继续只用 `m.content` 文本，图片与文本文件均不进入这两条链路（无需改动）。

### 6.6 输入区外观重构（对齐参考图）

结构调整（将现有单行 `.chat-input` 改为容器式 composer）：

```text
┌───────────────────────────────────────────────┐
│ [终端引用 chip]  [附件 chip ×N（图片缩略图/文件图标）] │  ← .ctx-strip（有则显示）
│                                               │
│  <textarea>（多行自增高，1~6 行后内部滚动）       │  ← .composer-input
│   Enter 发送 / Shift+Enter 换行                 │
│                                               │
│  ＋   ✋ 手动审批 ▾   Plan            （绿色圆↑）  │  ← .composer-bar
└───────────────────────────────────────────────┘
```

- 容器 `.composer`：`border:1px solid var(--border); border-radius:14px; background:var(--panel-2)`；聚焦时边框变 `var(--green)`；
- 文本域：改用 `<textarea rows="1">`，随输入自增高（`scrollHeight` 限制在 1~6 行区间，超出内部滚动）；保留 `inputRef` 的 focus/选中语义；
- 键盘：`Enter` 发送、`Shift+Enter` 换行，保留 `e.isComposing` 组合输入保护（中文输入法回车不误发）；
- 底部工具行 `.composer-bar`：左侧 `+` 附件按钮（图标按钮）、权限档（现 `<select>` 改为 ✋ 图标的胶囊外观，仍是原生下拉，弱化风格不变，放权档黄色警示保留）、Plan 开关（保持按钮样式）；右侧发送 / 停止按钮；
- 发送按钮：改为**绿色圆形上箭头**（`border-radius:50%`，上箭头图标），尺寸 28px；停止按钮沿用现状（方形停止块），同一位置切换；
- Plan 面板（`.plan-panel`）与授权弹窗（`.auth-wrap`）仍固定在 composer 上方，层级关系不变；附件 chip 与终端引用块共用 `.ctx-strip` 行，两者可同时存在（引用块在前、图片在后）。

### 6.7 与既有功能的交互

| 场景 | 处理 |
|---|---|
| 规划模式 + 有附件 | 共存：规划生成复用同一 `buildUserContent` 口径（文本/代码附件内联为 `<file>` 块、含图片时为 content parts），附件一并交给 Planner 参考；取消规划时原文与附件一并填回输入框 |
| 终端引用块 + 附件 | 共存：引用文本并入 `content` 文本；文本文件内联为 `<file>` 块，图片进入 content parts |
| AI 未启用 / 未配置 | 沿用 `configHint` 禁用输入区（含 `+` 按钮与拖拽） |
| 编辑重发 | 回填该消息 `images` / `files` 到附件区（§6.5） |
| 会话切换 / 清空 | `attachments` 随 scope 变化清空（与 `quote` 同一生命周期处理点） |
| 崩溃重载 | 附件已随消息落盘，重载后照常回显；未发送的附件为内存态，重载丢失（与草稿同语义） |

### 6.8 魔数取值

| 魔数 | 取值 | 理由 |
|---|---|---|
| 单图原始上限 | 8MB | 防超大文件读入内存；正常截图/照片远小于此 |
| 长边上限 | 1568px | OpenAI 视觉推荐上限，超出不提升识别质量且推高 token |
| 单图编码后上限 | 2MB | 压缩后仍超限说明内容异常复杂，拒绝并提示 |
| 单消息图片数 | 4 | 主流客户端惯例；更多则上下文膨胀、模型注意力分散 |
| 图片合计上限 | 6MB | 约束单轮请求体大小，兼顾多图场景 |
| JPEG 质量 | 0.85 | 视觉识别与体积的平衡点 |
| 单文件原始上限 | 1MB | 文本内容将内联进上下文，约 1~2 万行；更大则 token 成本过高 |
| 单消息文本文件数 | 4 | 与图片数量上限一致，约束单轮上下文体积 |
| 二进制守卫扫描长度 | 8192 字符 | 只查头部即可捕获 NUL，兼顾性能与准确性 |
| textarea 行数 | 1~6 行 | 超 6 行内部滚动，避免输入区吞掉消息区高度 |

## 7. 安全与降级

- 敏感数据：附件（图片 base64 与文本原文）仅经「本地 → 用户配置的模型服务商」链路，不经第三方；附件内容不写入应用日志（`applog`），错误提示仅含文件名与大小，不含 base64 片段或文件内容；
- 白名单校验：图片仅放行 `image/png|jpeg|webp|gif`；文本/代码仅放行扩展名白名单（或整名白名单），非法类型在读入前拒绝；
- 二进制守卫：文本文件读取后校验内容是否含 NUL 字节，防止把二进制文件改扩展名后当作文本内联（既避免发送乱码，也避免用户误发敏感二进制）；
- 失败降级：采集失败（图片解码/超限/格式、文件超限/读取失败/二进制）仅 toast 提示，不影响已输入文本与其余附件；发送失败沿用既有 `ai-error` 处理；图片与文本文件内容均不参与摘要/记忆提取，避免无谓上传与体积膨胀；
- 兼容降级：上游模型不支持视觉时，请求返回 HTTP 4xx，错误按既有 `ai-error` 提示展示（不自动剥离图片重试，避免静默改变用户意图）；
- 纯文本回归：无附件消息的 wire 形态与现状完全一致，工具循环、计划、授权链路不受影响。

## 8. 测试与验证

- 单测（`src-tauri/src/ai.rs`）：
  - `AiMessage.content` 为 content parts 数组时序列化/反序列化往返正确；
  - 纯文本消息（`content` 为字符串）与 `content: null`（assistant 携 tool_calls）序列化结果与现状一致；
- 前端构建：`pnpm build`（cwd `frontend/`，vue-tsc + vite）通过；
- 手工验证：
  1. 粘贴截图 → 出现 chip → 发送 → 模型能识别图片内容；
  2. 从文件管理器拖入图片（含多张）→ 高亮 + chip → 发送正常；
  3. 点击 `+` 选择文件 → chip → 发送正常；重复选同一文件可再次添加；
  4. 超 4 张 / 超大图 / 非图片文件 → 分别被拒绝并提示；
  5. 纯图片无文本发送 → 消息正常展示缩略图、上下文不丢失模型能应答；
  6. 含图消息触发工具循环 → 续跑正常；历史切换/重载后缩略图仍展示；
  7. 规划模式 + 附件 → 被拦截并提示；移除附件后可发送；
  8. 编辑含图历史消息 → 图片回填附件区；
  9. 拖入 / 选择 / 拖拽文本或代码文件（如 `.ts` / `.md` / `Dockerfile`）→ 出现文件 chip → 发送后模型能读到文件内容；
  10. 超 1MB 文件、非白名单扩展名（如 `.pdf` / `.zip`）、改扩展名的二进制文件 → 分别被拒绝并提示；
  11. 纯文件无文本发送 → 消息展示文件 chip、上下文不丢失模型能应答；历史重载后 chip 仍展示；
  12. 图片 + 文本文件 + 文本混合发送 → 上游收到 `<file>` 文本块与图片 content parts；编辑该消息 → 两类附件均回填；
  13. 规划模式（Toggle 或 `/plan `）+ 附件 → 计划生成时能参考附件内容；规划「取消」后原文与附件一并回到输入区；规划失败转普通模式时附件同样生效。

## 9. 未决问题

- 含图消息在 ReAct 多轮续跑中重复发送 base64 的 token 成本偏高；是否在多轮后仅保留首轮图片、后续轮降级为「图片已提供」的文本占位，留待后续观察成本后再定；
- 上游模型不支持视觉时的错误文案较原始（HTTP 4xx 原文透传）；是否需要针对「不支持图片输入」做更友好的识别与提示，待定；
- 图片查看交互（点击缩略图放大预览）本次仅做基础回显（新窗口打开 data URL），是否引入内置 lightbox 待定；
- 文本/代码文件当前按原文全量内联，未做逐文件截断；单文件 1MB 上限在极端场景仍可能显著推高 token，是否需要引入「超阈值截断 + 标注」待观察后定；
- 文本文件扩展名白名单为静态数组，是否需要支持用户自定义扩展名（设置项）待定。