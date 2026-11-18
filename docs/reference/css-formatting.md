# CSS 格式规范

> description: Rhost CSS 文件编码、选择器、声明、数值、注释、变量与媒体查询格式约定
>
> created: 2026-11-18 13:01:36
>
> updated: 2026-11-18 13:22:33
>
> author:

## 1. 适用范围

本规范适用于 `frontend/src/` 下的 CSS 文件及 Vue 单文件组件 `<style>` 块中的原生 CSS。

新增样式必须遵守本规范。整理存量样式时，只调整书写格式，不得批量重命名选择器、调整规则顺序或改变渲染行为。

## 2. 文件与编码

| 项目 | 规范 |
|---|---|
| 编码 | UTF-8，无 BOM |
| 换行符 | LF（`\n`） |
| 缩进 | 2 个空格，禁止 Tab |
| 文件末尾 | 保留一个空行，不出现连续多余空行 |
| 行宽 | 建议不超过 100 字符 |
| 行尾 | 不得包含多余空格 |
| 分号 | 每条声明必须以 `;` 结尾 |
| 引号 | 统一使用双引号 `"`，用于 `content`、`url()`、需要引号的字体名等 |
| 文件命名 | 全小写，使用连字符，如 `user-card.css` |

## 3. 选择器规范

### 3.1 基本格式

多个选择器各占一行，逗号位于行尾；选择器与 `{` 之间保留一个空格，`{` 后换行。

```css
/* 正确 */
.card,
.panel,
.modal {
  color: #1677ff;
}

/* 错误 */
.card,.panel,.modal{color:#1677ff}
```

### 3.2 书写规则

- 伪类冒号后不加空格：`a:hover`。
- 组合符两侧各保留一个空格：`.list > li`、`.card + .card`。
- 新增类名采用 BEM：`.block__element--modifier`，全小写并使用连字符。
- **禁止使用 ID 选择器编写样式。**
- 选择器嵌套层级不得超过 3 层。
- 不为满足命名规范而在纯格式化改动中批量重命名既有选择器。

```css
/* 推荐 */
.user-card__title--active {
  color: #1677ff;
}
```

## 4. 声明块规范

### 4.1 基本格式

每条声明独占一行；属性按用途分组，组间空一行，同组属性之间不留空行。

```css
.selector {
  position: absolute;
  top: 0;
  right: 0;
  z-index: 10;

  display: flex;
  width: 100%;
  margin: 0 auto;
  padding: 12px 16px;

  font: 400 14px/1.5 "PingFang SC", sans-serif;
  color: #333;

  background-color: #fff;
  border: 1px solid #e5e5e5;
  border-radius: 8px;

  transition: all 0.2s ease;
}
```

### 4.2 属性分组顺序

| 顺序 | 分组 | 代表属性 |
|---|---|---|
| 1 | 定位 | `position`、`top`、`right`、`bottom`、`left`、`z-index` |
| 2 | 盒模型 | `display`、`width`、`height`、`margin`、`padding`、`overflow` |
| 3 | 排版 | `font`、`line-height`、`text-align`、`color` |
| 4 | 视觉 | `background`、`border`、`border-radius`、`box-shadow` |
| 5 | 动画 | `transition`、`animation`、`transform` |

未列出的属性按用途归入最接近的分组。整理既有声明时，必须先检查简写属性与长属性之间的覆盖关系，不得因排序改变样式结果。

### 4.3 空格、冒号与逗号

```css
/* 正确 */
color: #333;
margin: 0 auto;

/* 错误 */
color:#333;
margin : 0 auto;
```

- 属性名后紧跟冒号，冒号后保留一个空格。
- 属性值与分号之间不留空格。
- 逗号前不留空格，逗号后保留一个空格。
- 多组 `transition`、`box-shadow` 等长值可按组换行，后续行增加一级缩进。

## 5. 数值与单位

| 场景 | 规范写法 | 禁止写法 |
|---|---|---|
| 0 值 | `margin: 0;` | `margin: 0px;` |
| 小数 | `0.5` | `.5` |
| 负小数 | `-0.5em` | `-.5em` |
| 单位 | 小写：`px`、`rem`、`em`、`vh`、`s`、`ms`、`deg` | `PX`、`S` |
| 颜色 | 小写 Hex，可无损缩写时使用 3 位，否则使用 6 位：`#fff`、`#1677ff` | `#FFFFFF` |
| 透明度 | `rgba(0, 0, 0, 0.5)` | `rgba(0,0,0,.5)` |
| 时间 | `0.2s`、`200ms` | `0.2S` |

以下语境中的单位具有语义或兼容性作用，不得省略：

- `transition: 0s` 中的 `0s`。
- `flex: 0 0 0%` 中的 `0%`。
- `calc()` 等表达式中由语法或计算类型要求保留的单位。

## 6. 字体与内容

```css
.content::after {
  content: "";
  font-family: "Helvetica Neue", Arial, sans-serif;
}
```

- 显式声明字体族列表时，末尾必须包含通用族：`serif`、`sans-serif` 或 `monospace`。
- 含空格或特殊字符的字体名必须加双引号；无需引号的字体名和通用族不强行添加引号。
- `content` 值统一使用双引号。
- `url()` 中路径统一使用双引号。

```css
.icon {
  background-image: url("./images/icon.png");
}
```

## 7. 注释规范

### 7.1 文件与模块注释

长文件按 **Reset → 变量 → 布局 → 组件 → 工具类 → 媒体查询** 组织；没有内容的分区不保留。模块注释使用统一格式：

```css
/* ==========================================================================
   1. 布局 Layout
   ========================================================================== */
```

文件头只说明当前文件职责及项目内依赖，不写外部设计来源：

```css
/* 全局基础样式；设计变量见 styles/tokens.css */
```

### 7.2 单行与多行注释

注释默认放在对应规则上方，与规则保持相同缩进。优先使用单行注释，复杂原因可使用多行注释，避免在声明行尾堆叠长说明。

```css
/* 卡片基础样式 */
.card {
  padding: 16px;
}
```

```css
/* 多行说明
   第二行说明 */
```

### 7.3 注释内容

- 使用简洁中文，说明本项目的布局约束、兼容原因、覆盖原因或设计意图。
- 重点解释“为什么这样写”，避免复述属性名称和属性值。
- 注释必须与实际样式同步；样式修改后同步更新或删除失效注释。
- 覆盖第三方库而必须使用 `!important` 时，必须在注释中说明具体原因。
- `macOS`、`Windows`、`Linux` 和浏览器名称等技术平台标识，仅在解释兼容行为或原生特性时使用。

```css
/* 防止 flex 子元素撑破容器 */
.workbench-root {
  min-width: 0;
}
```

不推荐添加无信息量的注释：

```css
/* 设置最小宽度为 0 */
.workbench-root {
  min-width: 0;
}
```

### 7.4 禁止事项

- **禁止使用 `//` 注释。**
- **禁止出现与本项目无关的外部产品、项目、品牌或设计风格名称。**
- 禁止“视觉语言”“设计来源”“参考某产品”“同款”“移植自”“复制自”等来源说明。
- 禁止宣传性、营销性或无技术含义的表述。
- 禁止保留被注释掉的废弃 CSS 代码。
- 禁止在注释中记录作者、日期和修改历史；版本历史由 Git 管理。

## 8. 变量与媒体查询

### 8.1 CSS 变量

#### 8.1.1 基本格式

```text
--[作用域-]<类别>-<角色>[-<状态>][-<变体>]
```

- 统一使用小写英文和连字符，即 `kebab-case`。
- 只允许小写字母、数字和连字符，禁止驼峰、下划线、空格及末尾连字符。
- 名称表达用途，不直接描述当前颜色或尺寸。
- 基础语义变量可使用短名：`--accent`、`--success`、`--warning`、`--danger`、`--info`。

```text
正确：--bg-popup、--fg-secondary、--accent-hover、--radius-popup-large
错误：--bgPopup、--bg_popup、--dark-gray、--radius-10px、--color1
```

#### 8.1.2 类别前缀

| 前缀 | 用途 | 示例 |
|---|---|---|
| `bg` | 背景、立面、输入框、浮层 | `--bg-app`、`--bg-popup` |
| `fg` | 文本、图标等前景色 | `--fg-strong`、`--fg-secondary` |
| `border` | 边框、分隔线 | `--border-popup`、`--border-strong` |
| `font` | 字体族 | `--font-ui`、`--font-mono` |
| `fs` | 字号 | `--fs-ui`、`--fs-small` |
| `space` | 间距、内边距、外边距 | `--space-2`、`--space-4` |
| `w` / `h` | 明确对象的宽度、高度 | `--h-titlebar`、`--h-control` |
| `radius` | 圆角 | `--radius-control`、`--radius-dialog` |
| `shadow` | 阴影 | `--shadow-popup`、`--shadow-card` |
| `dur` | 动画、过渡时长 | `--dur-fast`、`--dur-slow` |
| `ease` | 动画缓动函数 | `--ease-standard` |
| `z` | 层级 | `--z-popup`、`--z-modal` |

未列入表中的类别使用含义明确的完整单词，不新增难以理解的一次性缩写。

#### 8.1.3 角色与语义

变量优先表达使用场景，例如 `--bg-input`、`--bg-selected`、`--fg-muted`、`--border-soft`、`--shadow-tooltip`、`--status-success`。

| 不推荐 | 推荐 | 原因 |
|---|---|---|
| `--dark-gray` | `--bg-popup` | 主题变化后不一定仍是深灰色 |
| `--green` | `--accent` | 强调色允许用户自定义 |
| `--blue-text` | `--link` | 表达链接语义而非颜色 |
| `--radius-10px` | `--radius-dialog` | 数值调整时无需修改变量名 |

基础色板可以保留 `--green`、`--blue` 等颜色名，但组件样式应优先消费 `--accent`、`--success` 等语义变量。

#### 8.1.4 状态与变体

状态放在基础名称之后，主题或强度变体放在状态之后：

```text
--accent-hover
--accent-hover-strong
--scrollbar-thumb-hover
--radius-popup-large
--hover-overlay-strong
```

- 状态后缀：`hover`、`active`、`focus`、`selected`、`disabled`、`open`、`error`。
- 变体后缀：`strong`、`soft`、`large`、`small`、`fast`、`slow`、`raw`。
- `raw` 仅表示未经主题别名处理的原始值。

#### 8.1.5 RGB 通道变量

需要与 `rgba()` 组合的颜色使用 `-rgb` 后缀，值只包含三个颜色通道：

```css
:root {
  --accent: #3ddc84;
  --accent-rgb: 61, 220, 132;
}

.overlay {
  background: rgba(var(--accent-rgb), 0.16);
}
```

- `-rgb` 变量不得包含 `rgb()`、`rgba()` 或透明度。
- 同时存在状态和主题变体时，顺序为：状态 → `rgb` → 变体，如 `--accent-hover-rgb-strong`。

#### 8.1.6 数字后缀

数字只用于具有明确基准和递增规则的尺度，不得用于区分任意变量：

```css
:root {
  /* 允许：--space-N = N × 4px */
  --space-1: 4px;
  --space-2: 8px;
  --space-4: 16px;
  --space-0-5: 2px;
}
```

禁止使用 `--color1`、`--size2`、`--panel-2`、`--muted-2` 等无语义编号；应按用途改为 `--bg-panel-secondary`、`--fg-subtle` 等名称。

#### 8.1.7 作用域、主题与兼容别名

- 全局变量定义在 `:root`，不重复添加项目名前缀。
- 仅供单个组件使用的局部变量可添加稳定的组件缩写，如 `--tb-safe-left`。
- 不同主题覆盖同一个消费变量，组件不得分别读取 `--bg-app-dark` 和 `--bg-app-light`。
- 旧变量别名集中放置并注明替代变量，新增代码不得继续消费兼容别名。
- 变量重命名必须同步检查全部 `var()` 引用；纯格式化任务不得批量修改变量名。

```css
:root {
  --bg-app: #191a1c;
}

:root[data-theme="light"] {
  --bg-app: #ffffff;
}
```

### 8.2 媒体查询

```css
@media (max-width: 768px) {
  .card {
    padding: 8px;
  }
}
```

- `@media` 与括号之间保留一个空格。
- 组件专属媒体查询写在相关组件之后；全局响应式规则集中放在媒体查询分区。
- 移动优先时使用 `min-width`，按断点数值升序排列。
- 桌面优先时使用 `max-width`，按断点数值降序排列。
- 同一组件不得混用移动优先和桌面优先两种排序方式。

## 9. 格式化边界

- 不改变选择器、属性值、规则顺序及样式层叠关系。
- 不合并或拆分简写属性，不删除浏览器前缀和看似冗余的声明。
- 不合并重复规则，不批量清理既有选择器命名。
- 属性分组整理必须保持简写属性与长属性的原有覆盖结果。
- 注释清理只修改注释内容，不附带调整无关样式。

## 10. At‑Rule 规则

### 10.1 @import

1. `@import` 必须放置在文件最开头，在所有规则、`@layer`之前；
2. 路径字符串统一双引号；
3. 导入第三方库建议绑定层叠层，隔离优先级：

```
@import url("./reset.css") layer(base);
```

4. 禁止在规则块内部写`@import`。

### 10.2 @layer 层叠层

- 层定义顺序决定优先级；层声明集中放在文件头部；
- 分层建议：`base`（重置） → `tokens`（变量） → `components` → `utilities` → `overrides`；
- 第三方样式放入独立 layer，避免污染业务层；
- 不允许零散到处定义`@layer`。

```
@layer base, tokens, components, utilities, overrides;
```

### 10.3 @keyframes 关键帧动画

1. 动画名称 kebab‑case，语义命名，禁止`anim1 / fade2`；
2. `from / to` 优先，百分比关键帧每个占独立一行；
3. 关键帧块内部属性同样遵循本规范属性分组；
4. 不要在关键帧内写大量非动画属性。

```
/* ✅ */
@keyframes fade‑in {
  from {
    opacity: 0;
  }
  to {
    opacity: 1;
  }
}

/* ❌ */
@keyframes a1{0%{opacity:0}100%{opacity:1}}
```

### 10.4 容器查询 @container

- `@container`与括号之间保留一个空格；
- 组件内的容器查询紧跟该组件规则块之后；
- 容器名称使用 kebab‑case。

### 10.5 @scope 作用域规则

- 项目如需使用`@scope`，嵌套层级不超过 2 层；
- `:scope`关键字前后保留空格；
- 存量格式化任务**禁止新增 @scope**。

### 10.6 calc () 表达式

1. 运算符两侧必须保留空格；
2. 单位必须保证运算类型匹配；
3. 复杂 calc 允许换行，增加一级缩进；

```
width: calc(100% - var(--space‑4));
```

❌ `calc(100%-var(--space‑4))`（运算符缺少空格）。

## 11. 特异性、!important、深度选择器（Vue ）

### 11.1 !important 约束

> 
> 原规范只提注释，补充使用边界：

1. **仅允许场景**：工具类强制覆盖第三方组件、应急兼容修复；
2. **业务组件普通样式禁止使用 `!important`**；
3. 使用时必须注释写明：覆盖对象、为什么必须 important，后续优化方向；
4. 存量格式化任务：**不得删除原有！important，也不得新增！important**。

```
/* 覆盖第三方弹窗库内联样式；待替换为官方暴露的css变量后移除 !important */
.dialog {
  max‑width: 640px !important;
}
```

### 11.2 低特异性优先

1. 尽量使用 class 选择器；避免长后代选择器，控制选择器简单扁平；
2. 需要降低特异性时优先使用 `:where()`，而不是滥用！important；

```
:where(.card) .title {
  color: #333;
}
```

3. `:is()`用于选择器分组，不增加特异性。

### 11.3 Vue scoped 与 :deep ()

1. `<style scoped>` 优先；穿透优先用组件 props/class，其次才用`:deep()`；
2. 禁止滥用`:deep()`修改子组件内部布局结构，只修改颜色、圆角、阴影等外观；
3. 所有`:deep()`规则统一写在组件样式末尾，加注释说明修改目标；
4. 禁止嵌套多层`:deep()`。

```
/* 修改子组件内部按钮外观，仅修改颜色 */
:deep(.sub‑btn) {
  color: var(--fg‑strong);
}
```

## 12. 原生 CSS 嵌套

> 
> 本项目目前是原生 CSS，非 Sass；

1. 嵌套只用于伪类、状态、媒体查询，**不要用来镜像 DOM 结构**；
2. 最大嵌套层级：**2 层**；超过则抽成独立 class；
3. `&`符号紧邻选择器，前后无多余空格；

```
.card {
  &:hover {
    box‑shadow: var(--shadow‑card);
  }
  @media (max‑width:768px) {
    padding: var(--space‑2);
  }
}
```

> 
> 存量代码格式化任务：**不把旧扁平 CSS 改写为嵌套写法**。

## 13. 性能与可维护性约束

1. **仅允许 transform /opacity/filter 做动画**；避免动画 width/height/margin 触发重排。
2. 禁止空规则块 `{}`；lint 直接拦截；
3. 避免通配符`*`全局样式；重置仅放在 reset 层；
4. 长列表可视区域外考虑使用 `content‑visibility:auto`；
5. 尽量复用 CSS 变量，避免重复硬编码色值、尺寸。

## 14. RTL 与逻辑属性（可选）

1. 如果需要支持阿拉伯语等 RTL 语言，优先使用逻辑属性：`inline‑start` / `inline‑end` / `block‑size` 替代固定`left/right/width`；
2. 不需要 RTL 的业务，不强求改写存量；新增布局可酌情选用。

## 15. 废弃样式、工具类规范

### 15.1 废弃样式处理

1. 废弃的 class、废弃 CSS 变量**禁止直接删除**，分两阶段：
   - 阶段 1：注释标记 `/* @deprecated v1.2.0，使用xxx替代；下个大版本移除 */`，不要保留废弃 CSS 代码块；
   - 阶段 2：确认无引用后彻底删除；
2. 禁止把废弃代码注释保留在文件中；
3. 废弃 CSS 变量：保留别名，注释 deprecated，指向新变量。

```
:root {
  /* @deprecated 使用 --bg‑card 替代，2.0版本移除 */
  --bg‑panel: var(--bg‑card);
}
```

### 15.2 工具类 Utility class

1. 工具类统一命名：`u‑[用途]‑[变体]`，kebab‑case，例如：`.u‑text‑center`、`.u‑hidden`；
2. 工具类尽量只做单一职责；
3. 工具类全部放在文件的【Utils】分区。

## 16. 禁止清单

- ❌ 禁止直接使用命名颜色关键字 `red / blue / gray`；优先 hex /rgb/css 变量；色板原始令牌除外；
- ❌ 禁止空声明块；
- ❌ 禁止行尾注释；
- ❌ 禁止 ID 选择器写业务样式；
- ❌ 禁止在 calc 表达式运算符前后缺少空格；
- ❌ 禁止嵌套超过 2 层（原生 CSS 嵌套）；
- ❌ 业务普通样式禁止随意！important；
- ❌ 禁止文件内保留大段注释掉的旧 CSS 代码。