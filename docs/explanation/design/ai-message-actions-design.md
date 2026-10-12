# AI 消息操作按钮设计方案

> description: AI 聊天面板消息操作增强——消息底栏（复制全文/重新生成/编辑重发/删除）与代码块头部（复制/插入终端/直接执行/存为片段）的状态矩阵与摘要回滚策略，供 M2 落地参考
>
> created: 2026-10-11 19:40:29
>
> updated: 2026-10-11 21:06:14
>
> author:

---

## 1. 设计目标与硬约束

> 范围：`frontend/src/components/AiChatPanel.vue` 消息级操作（`bubble-foot`），以及 `frontend/src/components/AiMarkdown.vue` 代码块级操作（`md-code-head`，插入终端/直接执行/存为片段在此层针对单个代码块）。
> 边界：不覆盖 M2 的 PTY 执行协议与 tool calling 细节（本文只定义按钮契约）；不覆盖面板头部操作（清空对话，已有）；不覆盖记忆/规则/技能注入。
> 关联代码：`frontend/src/components/AiChatPanel.vue`、`frontend/src/components/AiMarkdown.vue`、`frontend/src/lib/ai.ts`、`frontend/src/stores/session.ts`

| 目标 | 量化口径 |
|---|---|
| 零网络依赖 | 复制/编辑/删除/重新生成为纯前端本地操作，除重新生成发起模型请求外点击响应 < 50ms 可感知 |
| 低视觉噪音 | 按钮图标 14px、按钮热区 ≥ 22px；foot 默认隐藏，hover 显隐过渡 ≤ 150ms |
| 状态完备 | 下文状态矩阵中每个（按钮 × 消息状态）格子的可见/隐藏行为唯一确定，无歧义 |
| 摘要一致性 | 编辑重发/删除后，`buildWireMessages()` 产出的上下文不包含已截断消息，且摘要覆盖点关系自洽 |

必须遵守的既有项目约束：

- 面板宽度 280~480px（专注模式可放宽），底栏空间窄：按钮一律图标 + `title` tooltip，不用文字按钮；
- 剪贴板统一走 `@tauri-apps/plugin-clipboard-manager` 的 `writeText`（先例：`frontend/src/components/wb/TerminalPane.vue`）；
- 消息模型 `AiChatMessage`（`frontend/src/lib/ai.ts`）字段不变，操作按钮不新增持久化字段；
- 滚动摘要 `summary`/`summaryUpTo`（M1.5）已随会话落盘，编辑/删除操作必须处理摘要覆盖点回滚；
- streaming 态消息不可被操作（发送/编辑/删除均已被 `streaming.value` 守卫拦截）。

## 2. 必须遵守的既有项目约束

- 「已删主机只提示不删历史」的用户决策同源：本地会话数据归用户自由编辑，删除操作不做二次确认弹窗（可撤销能力后期再议）；
- Toast 类型取值 `'ok' | 'info' | 'warn' | 'err'`（`frontend/src/composables/useToast`），错误提示用 `'err'`；
- 头部操作按钮走 `Teleport to="#rp-head-actions"` 注入（先例：`ai-action-btn`），消息级按钮不走 Teleport，直接内联 `bubble-foot`；
- 图标风格与现有 24 viewBox / stroke currentColor SVG 一致（先例：头部清空按钮、发送按钮）。

## 3. 现状盘点与差距

| 位置 | 现状 | 差距 |
|---|---|---|
| `AiChatPanel.vue` `bubble-foot`（assistant） | error 态显示「重试」文字按钮；stopped 态仅显示「已中止」文本 | 无复制全文；done/stopped 态无法重新生成 |
| `AiChatPanel.vue` `bubble-foot`（user） | 完全没有操作区 | 编辑重发、复制、删除全部缺失 |
| `AiMarkdown.vue` `md-code-head` | 仅语言标签 + 「复制」按钮 | 缺插入终端/直接执行/存为片段（针对单个代码块） |
| 显隐策略 | 常显 | 每条消息下挂按钮行造成视觉噪音，需 hover 淡入 |
| 摘要联动 | `retry()` 只截断到最后一条 user，未考虑 `summaryUpTo` 落在截断范围内的情形 | 编辑重发/删除需要摘要回滚规则 |

结论：本设计把操作拆为两层——消息级（底栏，针对全文）与代码块级（代码块头部，针对单个代码块），并补齐「截断类操作 × 滚动摘要」的一致性规则，避免 M2 落地时遗漏。

## 4. 目标与非目标

### 目标

1. 消息级（assistant 底栏，针对全文）：复制全文、重新生成（done/stopped/error 均支持）；
2. 消息级（user 底栏）：编辑重发、复制、删除；
3. 代码块级（`md-code-head`，针对单个代码块）：复制（已有）、插入终端/直接执行（M2）、存为片段（依赖片段库）；
4. 交互统一：hover 淡入、图标化 + tooltip、streaming 隐藏；
5. 定义截断类操作（编辑重发/删除/重新生成）与滚动摘要的回滚规则。

### 非目标

- 不做点赞/点踩（本地产品无反馈消费链路）；
- 不做消息多选、转发、引用追问；
- 不在消息底栏提供「插入终端」——全文没有可执行的确定语义，终端操作一律针对代码块（无代码块的纯文本回复不出现终端操作入口）；
- 不实现 PTY 写入协议细节（M2 范围，见 `docs/explanation/design/` 后续 M2 设计文档）；
- 不改动 `AiMarkdown` 渲染管线与 `AiChatMessage` 数据结构。

## 5. 总体架构

```text
bubble (hover 触发器)
├── msg-head        AI 消息：AppLogo + Agent；user 消息：无头部
├── 正文            assistant → AiMarkdown；user → 纯文本
│   └── md-code-head（每个代码块，常显）
│       ├── md-code-lang        语言标签
│       └── [复制] [插入终端(M2)] [直接执行(M2)] [存为片段(依赖片段库)]
└── bubble-foot     opacity 0 → hover 时 1（transition 150ms）
    ├── assistant:  [复制全文] [重新生成]
    └── user:       [编辑重发] [复制] [删除]
```

数据流：所有按钮只操作组件内 `messages` / `summary` / `summaryUpTo` 状态，随后统一走 `persist()` 落盘（复用现有通道，不新增 IPC）。代码块级操作由 `AiMarkdown` 内部处理，通过 emit 把「插入终端/直接执行」意图上报给宿主 `AiChatPanel`（携带代码文本），PTY 写入的会话定位由宿主决定。

## 6. 详细设计

### 6.1 按钮状态矩阵

消息级（`bubble-foot`，针对全文）：

| 按钮 | user | assistant done | assistant streaming | assistant stopped | assistant error |
|---|---|---|---|---|---|
| 复制全文 | — | ✓（有内容） | 隐藏 | ✓（有内容） | ✓（有内容） |
| 重新生成 | — | ✓ | 隐藏 | ✓ | ✓（沿用现有） |
| 编辑重发 | ✓（非 streaming 期间） | — | 隐藏 | — | — |
| 复制（user） | ✓ | — | 隐藏 | — | — |
| 删除 | ✓ | — | 隐藏 | — | — |

代码块级（`md-code-head`，针对单个代码块，assistant 消息内每个代码块独立出现）：

| 按钮 | 条件 |
|---|---|
| 复制 | 常显（已有） |
| 插入终端 | M2 起；仅 workbench 视图（首页无终端目标） |
| 直接执行 | M2 起；仅 workbench 视图 |
| 存为片段 | 依赖片段库功能立项 |

规则：整个会话处于 `streaming` 时，所有消息的 foot 均隐藏（编辑重发/删除会与进行中的流产生竞态）；流式中的代码块同样只保留「复制」，终端/片段操作在流结束后可用。

### 6.2 按钮行为定义

| 按钮 | 层级 | 行为 | 优先级 | 实现要点 |
|---|---|---|---|---|
| 复制全文 | 消息级 | `writeText(m.content)` 写入原始 Markdown（代码块含围栏原文，与渲染无关）；toast「已复制」 | P0 | 复用 `AiMarkdown.vue` 复制失败的 toast 降级模式 |
| 重新生成 | 消息级 | 现有 `retry()` 逻辑放宽适用态：error 之外扩展到 done/stopped；定位最后一条 user 消息截断重发 | P0 | `retry()` 已实现核心逻辑，只需改可见条件 |
| 编辑重发 | 消息级 | 把该条 `content` 填回输入框并聚焦；用户确认发送时，截断该条及其后全部消息，再走正常 `send()` | P0 | 填回时不立即截断——用户可能取消编辑；截断发生在真正发送时 |
| 复制（user） | 消息级 | `writeText(m.content)` | P0 | 同复制全文 |
| 删除 | 消息级 | 删除该条 user 消息及紧随其后的 assistant 回复，然后 `persist()` | P2 | 不弹确认框（见 §2） |
| 插入终端 | 代码块级 | 将**当前代码块**文本写入目标会话 PTY，**不回车**，用户可先编辑 | P1（M2） | 取数对象是所点击的代码块本身（非「首个代码块」/全文）；目标会话见 §9 |
| 直接执行 | 代码块级 | 将**当前代码块**文本写入 PTY + 回车；多行命令需用户在设置中确认「多行直接执行」开关（M2 细化） | P1（M2） | 与插入终端共用取数逻辑，仅末尾追加 `\n` |
| 存为片段 | 代码块级 | 将**当前代码块**文本写入命令片段库 | P1~P2 | 依赖片段库功能立项（目前不存在），可能调整 |

### 6.3 摘要回滚规则（截断类操作 × 滚动摘要）

编辑重发、删除、重新生成都会截断 `messages`。截断后按以下规则校正 `summary`/`summaryUpTo`：

```text
设被截断的第一条消息为 cut（即 cut 及其后的消息全部移除）：

1. summaryUpTo 为空                      → 摘要本就不存在，无操作
2. summaryUpTo 对应消息在 cut 之前        → 摘要覆盖点未受损，摘要保留
3. summaryUpTo 对应消息 ∈ [cut, …]       → 摘要覆盖的消息已被删除，
                                           保守策略：清空 summary 与 summaryUpTo
```

保守策略的理由：摘要由被删消息合并而来，删除后无法重建（原始内容已不存在）；宁可让上下文回退为全文携带（消息变少后大概率不再触发阈值），也不允许上下文引用已被用户删除的「事实」。

`retry()` 现有实现按内容定位最后一条 user 截断，M2 落地时统一改为按索引截断并接入上述规则。

### 6.4 交互细节

- 消息级 foot 容器：`opacity: 0; transition: opacity 0.15s;`，消息块 `:hover` 时 `opacity: 1`；流式期间整个 foot `v-show` 隐藏；
- 代码块级按钮位于 `md-code-head` 右侧（复制按钮旁），随头部常显；M2 前不渲染占位图标（避免不可用入口）；
- 图标统一 24 viewBox / stroke currentColor / 实际渲染 14px，颜色 `var(--muted-2)`，hover 变 `var(--text)`；
- tooltip 用原生 `title` 属性（与头部 `ai-action-btn` 一致），不引入 tooltip 组件；
- 「已中止」标签保留在 foot 内，与图标共存；
- 代码块级操作在 `AiMarkdown` 内部仅 emit 意图（如 `emit('terminal', { code, run })`），PTY 写入与会话定位由宿主 `AiChatPanel` 执行——`AiMarkdown` 保持纯展示组件，不依赖 session store。

## 7. 安全与降级

- 敏感数据：复制/编辑操作仅触及会话内存与剪贴板，不写日志、不落额外文件；用户消息可能含密钥等敏感粘贴内容，不做任何上报（本项目 AI 请求本就仅发送用户主动提交的内容）；
- 失败降级：剪贴板写入失败 toast `'err'` 并 `console.error`（沿用 `AiMarkdown.vue` 模式）；persist 失败已有 `console.error` 降级，不阻断 UI；
- 竞态防护：所有截断类操作在 `streaming.value === true` 时不可达（foot 隐藏）；`loadSeq` 守卫沿用，scope 切换后旧的编辑/删除结果不落地。

## 8. 测试与验证

- 单测：摘要回滚规则 §6.3 的三种分支——可在 `AiChatPanel` 抽出纯函数 `reconcileSummaryAfterCut(messages, summary, summaryUpTo, cutIndex)` 后对 `frontend` 侧逻辑做 vitest 断言（若仓库未引入前端测试框架则降级为手工验证）；
- 手工验证清单（状态矩阵逐格）：
  1. AI 消息 done/stopped/error 三态 hover，确认底栏按钮组与矩阵一致，复制全文内容与原文（含 Markdown 围栏）逐字节一致；
  2. done 态点重新生成：旧回复被截断、重新流式输出；
  3. user 消息编辑重发：输入框回填 → 截断其后的回复 → 新回复流式生成；
  4. 构造长对话触发摘要后，删除一条位于 `summaryUpTo` 覆盖范围内的消息，检查落盘 JSON 中 `summary`/`summaryUpTo` 已清空；再删除覆盖范围外的消息，确认摘要保留；
  5. assistant 消息含多个代码块时，每个 `md-code-head` 独立可用，复制内容为所点击代码块本身（非首个代码块）；
  6. 首页（home）视图中代码块头部不出现插入终端/直接执行入口；
  7. streaming 期间 hover 任意消息，foot 不可见；流式中的代码块头部仅有复制可用。

## 9. 未决问题

- ~~「插入终端/直接执行」的目标会话取值~~ 已裁决：取点击时的当前活动会话，不做消息快照，见 [ai-terminal-exec-design.md](./ai-terminal-exec-design.md) §6.1；
- 片段库功能尚未立项，「存为片段」的数据模型与入口（Dock？右键菜单？）待其设计文档确定后回填；
- ~~「多行命令直接执行」是否需要独立设置开关~~ 已裁决：新增设置项 `aiMultilineExec`（默认 `false`），见 [ai-terminal-exec-design.md](./ai-terminal-exec-design.md) §6.3；
- 前端测试框架（vitest）未引入，§8 单测的落地形态取决于仓库是否补齐该基建。
