# WebView 滚动行为设计方案

> description: 约定 WebView 内自绘滚动容器的回弹禁用、表头横向同步与滚动条配色规则
>
> created: 2026-11-26 16:13:00
>
> updated: 2026-11-26 16:17:28
>
> author:

## 1. 设计目标与硬约束

> 范围：Tauri WebView 内所有自绘滚动容器，首个落地对象为 SFTP 文件列表（`.pane-list`）。
> 边界：不覆盖 xterm 终端内部滚动；不覆盖 WKWebView 原生主滚动（DOM 内部滚动不受 NSScrollView 的 elasticity 控制）。
> 关联代码：`frontend/src/style.css`、`frontend/src/components/wb/DockPanel.vue`

| 目标 | 量化口径 |
|---|---|
| 无越界滚动 | 滚动条滑块到达端点后继续拖动，`scrollLeft/scrollTop` 保持在 `[0, max]`，松手后无回弹位移 |
| 表头始终对齐 | 任意横向滚动位置，表头列与文件行的水平偏移差为 0px |
| 可滚动范围守恒 | `scrollWidth - clientWidth` 只由文件行内容决定，表头不参与滚动内容 |
| 同步开销可控 | 每次 `scroll` 只写一次 `style.transform`，不触发 Vue 响应式更新 |

## 2. 必须遵守的既有项目约束

- 拖拽自动滚动依赖 `.pane-list` 作为滚动容器（命中 `closest('.pane-list')`），不能把滚动移到外层，否则落点判定与自动滚动失效。
- 表头 `.list-header` 与行 `.row` 必须共享同一套列宽规则，最小总宽统一为 `580px`。
- 滚动条轨道、角落与容器背景统一使用主题变量 `var(--panel)`，不引入新的硬编码颜色。
- 滚动容器内仍受全局"默认不可选中"策略约束，同步逻辑不得改变该行为。

## 3. 现状盘点与差距

| 位置 | 现状 | 差距 |
|---|---|---|
| `.pane-list` | `overflow: auto`，横纵均可滚动 | 未设置 `overscroll-behavior`，macOS 橡皮筋过冲导致滑块到头后仍可拖动内容 |
| 表头 | 曾被放入滚动内容并设置 `position: sticky` | WebKit 会把 sticky 偏移计入可滚动范围，出现"到头仍可拖"与横向缝隙 |
| 滚动条 | 只定义 `::-webkit-scrollbar-thumb` | `track` / `corner` 透明，横滚到边界露出深色底 |
| 表头同步 | 无 | 表头移出滚动容器后，需要手动跟随横向偏移 |

结论：需要"禁用回弹 + 表头不参与滚动 + 横向偏移同步 + 轨道同色"四条规则共同生效。

## 4. 目标与非目标

### 目标

1. 滚动条滑块到达端点后不可继续拖动内容。
2. 横向滚动时表头与文件列始终对齐，纵向滚动时表头固定。
3. 滚动到左右边界不再出现深色块或空白条。
4. 不改变拖拽传输、筛选、排序等既有交互。

### 非目标

- 不接管触摸板惯性与动量滚动，交给系统处理。
- 不修改 xterm 内部滚动实现。
- 不实现自绘滚动条组件。

## 5. 总体架构

```text
.pane
├─ .pane-path          路径 / 筛选输入
├─ .list-header-wrap   overflow: hidden（负责裁剪横向偏移）
│   └─ .list-header    列宽与 .row 一致，translateX 同步滚动
└─ .pane-list-wrap
    └─ .pane-list      overflow: auto（唯一滚动容器）
        ├─ .pane-empty
        └─ .row ...
```

数据流：`scroll` 事件由 `.pane-list` 触发，读取 `scrollLeft` 后直接写入同级 `.list-header` 的 `transform`，不经过响应式状态。

## 6. 详细设计

| 规则 | 实现 | 理由 |
|---|---|---|
| 禁用橡皮筋回弹 | `.pane-list { overscroll-behavior: none; }` | WebKit 弹性过冲会让滚动条到头后仍能拉动内容，松手才回弹 |
| 表头不进入滚动区 | 表头置于 `.list-header-wrap`（`overflow: hidden`），不放在 `.pane-list` 内 | `sticky` 表头会撑大可滚动范围并产生横向缝隙 |
| 横向偏移同步 | `scroll` 回调写 `translateX(-scrollLeft)px` | 不进响应式状态，避免每帧触发组件重渲染 |
| 宽度一致 | 表头 `width: 100%; min-width: 580px`，行 `min-width: 580px` | 保证可滚动范围与列宽一致，边界无空白 |
| 滚动条槽同色 | `::-webkit-scrollbar-track`、`::-webkit-scrollbar-corner` 设为 `var(--panel)` | 消除横滚到边界的深色条 |
| 表头区分底色 | 表头与 `.list-header-wrap` 使用 `var(--panel-2)`，比列表底色更亮 | 提供表头区域标识，同时覆盖滚动条占位让出的区域 |

同步实现（`frontend/src/components/wb/DockPanel.vue`）：

```ts
/** 列表横向滚动时同步表头偏移，避免表头参与滚动区导致可滚范围异常 */
function onListScroll(e: Event) {
  const list = e.currentTarget as HTMLElement
  const header = list.closest('.pane')?.querySelector<HTMLElement>('.list-header')
  if (header) header.style.transform = `translateX(${-list.scrollLeft}px)`
}
```

样式实现（`frontend/src/style.css`）：

```css
.pane-list {
  overflow: auto;
  /* 禁用 macOS/iOS 的橡皮筋回弹 */
  overscroll-behavior: none;
  background: var(--panel);
}
.pane-list::-webkit-scrollbar-track,
.pane-list::-webkit-scrollbar-corner {
  background: var(--panel);
}
.list-header-wrap {
  flex-shrink: 0;
  overflow: hidden;
  background: var(--panel-2);
}
.list-header {
  width: 100%;
  min-width: 580px;
  background: var(--panel-2);
}
```

降级方案：若目标系统 WebKit 版本不遵循 `overscroll-behavior`，在 `scroll` 回调中钳制滚动位置；该方式可能在弹性过程中产生轻微抖动，仅在确认属性不生效时启用。

```ts
const maxX = list.scrollWidth - list.clientWidth
const maxY = list.scrollHeight - list.clientHeight
if (list.scrollLeft < 0) list.scrollLeft = 0
else if (list.scrollLeft > maxX) list.scrollLeft = maxX
if (list.scrollTop < 0) list.scrollTop = 0
else if (list.scrollTop > maxY) list.scrollTop = maxY
```

魔数与取值：

| 常量 | 取值 | 理由 |
|---|---|---|
| 列表最小总宽 | `580px` | 五列固定宽度与名称列弹性布局的最小可用宽度；表头与行共用 |
| 滚动条尺寸 | `var(--sb-size)` | 与全局滚动条规格一致，不单独定义 |
| 同步目标 | `transform: translateX` | 不改变布局，仅合成层平移，避免滚动时重排 |

## 7. 安全与降级

- 敏感数据：本设计只涉及滚动偏移与表头位移，不读取、不记录任何文件内容或凭证。
- 失败降级：
  - 找不到表头元素时静默跳过，列表滚动不受影响；
  - `overscroll-behavior` 未生效时启用 JS 钳制；
  - 钳制仍无效则保留系统回弹行为，不影响功能正确性。

## 8. 测试与验证

- 编译校验：`cd frontend && npx vue-tsc -b`。
- 手工验证：
  1. 横向滚动条滑块拖到最左/最右后继续拖动，内容不再位移，松手无回弹。
  2. 任意横向滚动位置，表头「名称 / 大小 / 修改时间 / 权限 / 用户·组」与文件列对齐。
  3. 纵向滚动时表头固定，横向滚动时表头跟随。
  4. 横滚到左右边界不出现深色条或空白。
  5. 筛选高亮、拖拽入队与边缘自动滚动、排序点击均正常。

## 9. 未决问题

- 是否把 `overscroll-behavior: none` 推广到日志、AI 会话等其他自绘滚动容器。
- 最小总宽 `580px` 是否需要随面板宽度自适应调整。
