# 启动动画（Splash）设计方案

> status: 已落地（2026-10-06）
>
> 关联文档：`docs/applog-design.md`（生命周期事件）、`docs/architecture.md`

## 1. 目标与非目标

### 目标
1. **消除启动空窗**：从窗口出现到 Vue 界面可交互之间，当前存在无反馈空窗期（webview 初始化 + JS bundle 加载 + Vue mount + `loadHosts`）。用启动画面填充，杜绝白屏/黑屏闪烁。
2. **建立品牌感**：启动画面以**项目 LOGO** 为视觉主体 —— 即产品统一 Logo 组件 [AppLogo.vue](../frontend/src/components/AppLogo.vue)（终端窗口 + `>_` 命令提示符线框，**不含** favicon.svg 的黑色圆角底座），以**默认主题色** `#3ddc84`（`currentColor` 机制，与产品内 `.qc-mark` 等处着色一致）渲染，并施加**呼吸动画**（缩放 + 光晕），传达"正在启动且存活"的第一印象。
3. **零依赖、零阻塞**：启动画面必须在 JS bundle 加载之前可见 —— 纯 HTML + CSS 实现，不引入任何 npm 依赖，不拖慢真实启动。

### 非目标
- 不做真实启动进度反馈。Splash 阶段（HTML 首帧 → Vue ready）拿不到任何真实事件流，呼吸动画是"存活信号"而非进度，**不伪造进度**（设计原则：宁可模糊，不可撒谎）。
- 不跟随用户自定义强调色（`applyAccent` 注入的 `--accent-rgb`）。冷启动时应用 CSS/后端快照尚未加载，splash 固定使用默认主题色 `#3ddc84` 字面量（splash 渲染时 `style.css` 尚未加载，`:root` 变量不存在，无法引用 `var(--green)`）。
- 不做恢复会话等业务的等待动画。`loadHosts` + `restoreSessions` 为本地毫秒级操作，无需分阶段展示。

## 2. 改造前的问题

改造前启动时序：

```
窗口创建(原生, decorations:false) ──► webview 加载 index.html ──► 下载/执行 JS bundle
   │                                    │
   │  ◄── 空窗 ①：原生窗口默认白底（macOS）   │
   │                                      ──► style.css 生效（此刻才有深色背景）
   │                                        ──► Vue mount → onMounted:
   │                                              initGlobalErrorReporting()
   │                                              syncLogConfig()
   │                                              await loadHosts()
   │                                              restoreSessions()
   └────────── 全程用户盯着一块无内容的窗口 ──────────────────► 界面突现
```

问题点：
- **空窗 ①**：`tauri.conf.json` 未设置 `backgroundColor`，macOS 上窗口首帧为白色，随后被深色覆盖产生"白闪"。
- **空窗 ②**：`index.html` 的 `<div id="app">` 为空，深色背景也要等 `style.css`（打进 JS bundle）加载后才生效。
- **突现**：主界面从无到有硬切，无交接过渡。

## 3. 方案总览

分三层解决，每层独立兜底：

| 层 | 手段 | 消除的问题 |
|---|---|---|
| 原生窗口层 | `tauri.conf.json` 启用 `"transparent": true`（macOS 配 `"macOSPrivateApi": true`） | 空窗 ①（webview 渲染前的白底；默认窗口透明、LOGO 悬浮桌面，深色底模式见 §4.1） |
| HTML Splash 层 | `index.html` 内联 `<style>` + `<div id="splash">`（随首字节渲染，动画纯 CSS） | 空窗 ②（bundle 加载期） |
| Vue 交接层 | 启动流程完成后为 splash 加 `.splash--done` 类淡出移除 | 突现问题 |

Splash DOM 放在 `#app` **外部**（Vue `mount('#app')` 不会触碰它），`position:fixed; z-index:99999` 覆盖主界面。

### 3.1 决策记录：不采用独立 Splash 窗口

曾评估"双窗口方案"：`tauri.conf.json` 同时创建 `splashscreen`（无边框/透明/置顶小窗）与 `main`（默认隐藏），Rust 侧 Tokio 任务完成初始化后发事件，前端调 `close_splash` 关 splash 并显示主窗。该方案是为"**启动期存在秒级 Rust 侧重初始化**"（钥匙串解密、DB 迁移、Russh 预连接等）的架构设计的。

本项目不采用，理由：
1. **瓶颈不存在**：启动期只有 `loadHosts` / `restoreSessions` 毫秒级本地 IPC；SSH 连接按需建立（`ssh.connect` 生命周期），启动期无引擎初始化。
2. **首帧无优势**：splash 窗口的动画内容同样要等它自己的 webview 加载 HTML，相比内嵌方案几乎无提前，反而多一份 webview 加载开销。
3. **复杂度不对称**：跨窗口交接（Windows 下切换闪烁）、透明/无边框/置顶窗口的三平台差异、Rust↔splash↔main 三方通信竞态，均为净增成本。

切换触发条件（满足其一可重新评估）：启动期出现秒级 Rust 侧初始化；或产品要求 splash 为独立小卡片样式（主窗口全屏在其后）。届时内嵌方案的 `hideSplash` 交接点改为等待 Rust 事件即可，现有设计不锁死升级路径。

## 4. 视觉设计

### 4.1 视觉主体：项目 LOGO 呼吸

> 直接呈现产品统一 Logo（`AppLogo.vue`：终端窗口 + `>_` 提示符**线框**，无底座无边框），以默认主题色渲染并施加呼吸动画。

```
┌──────────────────────────────────────┐
│                                      │
│               ╭──────╮               │
│               │  >_  │               │  ← AppLogo 线框，#3ddc84 描边
│               ╰──────╯                      缩放 + 光晕同步呼吸
│            ░░▒▒▓▓▒▒░░                │  ← LOGO 背后光晕（示意）
│                                      │
│                                      │
└──────────────────────────────────────┘
```

> 画面仅保留呼吸 LOGO，无文字 —— 单一视觉焦点更干净。

规格：

| 元素 | 规格 |
|---|---|
| LOGO | **内联** [AppLogo.vue](../frontend/src/components/AppLogo.vue) 的 SVG：`viewBox="0 0 24 24"`、`stroke="currentColor"`、`stroke-width="1.5"`、round cap/join；图形为圆角窗口 `rect(2.5,4,19,16,rx3)` + `>` 折线 + `_` 下划线。内联而非 `<img>`，保证随首帧渲染且受 CSS 控制 |
| 显示尺寸 | `96px`（splash 为全屏焦点场景，大于产品空态的 64px；线宽 1.5 等比放大约 6px，观感一致） |
| 主题色 | 父级设 `color:#3ddc84`，经 `stroke="currentColor"` 传导 —— 与产品内 `.qc-mark{color:var(--green)}` 同一着色机制；splash 阶段 CSS 变量未定义，必须写字面量（见 §1 非目标） |
| 呼吸·缩放 | LOGO 容器 `scale 1 → 1.035`，`2s ease-in-out infinite alternate` |
| 呼吸·光晕 | LOGO 背后独立光晕层：圆形 `radial-gradient(rgba(61,220,132,.28), transparent 70%)`，直径约 2.4× LOGO，`opacity .35 → .75`，与缩放同周期同相位 |
| 光晕实现 | 用独立 `radial-gradient` 层动 `opacity`，**不用 `drop-shadow` 滤镜动画** —— 后者逐帧触发 SVG filter 重绘，前者仅合成层透明度变化 |
| 布局 | 垂直居中偏上：偏移施加在 **LOGO 自身**（`translateY(-4vh)`，keyframes 两帧均需携带）。**禁止**施加在 `#splash` 容器上 —— 容器随 `inset:0` 上移会在底部露出 4vh 缝隙，状态栏等底层 UI 穿透（实测缺陷） |
| 层级 | `z-index: 99999`，高于应用内所有浮层（ContextMenu 9999 等），splash 期间不允许任何主界面元素穿透 |
| 背景与主界面 | 双模式（设置 `splashTransparent`，重启生效）：**透明**（默认）= 原生窗口 `transparent` + `html/body` 全透明，LOGO 悬浮桌面；**深色** = Rust 创建主窗口时经 `initialization_script` 注入 `window.__RHOST_BOOT__`（首帧前、任何页面脚本之前；数据源为 app_config.json 的 settings 节），`<head>` 内联脚本据此给 `<html>` 加 `splash-solid` 类（`background:#06090d`，特异度覆盖透明规则），LOGO 悬浮深色底。**禁止改走异步 IPC 或 localStorage**（前者首帧读不到导致透明/深色闪烁；localStorage 已下线为配置通道，旧版本残留值不会随设置更新，曾导致开关失效）。两种模式下主界面均以 `.splash-active #app{visibility:hidden}` 隐藏，`__hideSplash` 首行同时移除两个类 —— 主界面显现与 LOGO 淡出同步 |

取舍说明：早期方案的打字机文字与底部不定进度条**未采用** —— 呼吸 LOGO 已传达"正在启动且存活"，打字机与进度条会与呼吸主体争夺视觉焦点，且打字机文本属装饰性伪语义。

**备选方向**（未来需要可启用）：
- B. 带文字：LOGO 下方加 `Rhost` 等宽字体文字淡入（早期版本曾实现后移除）。
- C. 零动画：静态 LOGO，无呼吸。

以下实现均按当前代码描述。

### 4.2 动效参数汇总

| 动画 | 时长 | 缓动 | 参数 |
|---|---|---|---|
| LOGO 呼吸·缩放 | 2s ×∞ | ease-in-out, alternate | `scale 1 → 1.035` |
| LOGO 呼吸·光晕 | 2s ×∞ | ease-in-out, alternate | `opacity .35 → .75`（与缩放同相位） |

呼吸周期选 2s：静息呼吸节奏，快了显得焦躁、慢了像卡顿。alternate 模式保证往复平滑无跳变。

所有动画只在 splash 存活期内运行，淡出后随 DOM 移除，无残留。

### 4.3 无障碍与降级

```css
@media (prefers-reduced-motion: reduce) {
  .splash-mark,.splash-glow{animation:none}
  .splash-glow{opacity:.5}          /* 光晕定格中间值 */
}
```

- 减少动态偏好下：LOGO 静止 + 中等光晕，仅保留淡出过渡（或直接移除）。
- 高分屏/低端机：呼吸只动 `transform` 与合成层 `opacity`，GPU 加速，无 layout / SVG filter 重绘。

## 5. 交接时序

```
T0   窗口显示（默认原生窗口透明、LOGO 悬浮桌面；关闭透明设置时为 #06090d 深色底）
T0+~30ms   HTML 首帧：splash 完整可见，LOGO 呼吸开始
T1    JS bundle 就绪，Vue mount
T2    onMounted: initGlobalErrorReporting → await loadAppConfig() hydrate 各 store
      （此步完成后 savedSettings.splashDurationMs 才是用户设置值，时长必须在此之后读取）
T2'   minDelay(splashDurationMs) 开始计时；syncLogConfig / loadHosts /
      restoreSessions 记为 pending promise
T3    Promise.all([
        minDelay(splashDurationMs),
        Promise.race([loadHosts/restoreSessions 等启动任务, 3s 硬超时])
      ]) 完成 → document.getElementById('splash').classList.add('splash--done')
T3+300ms   splash.remove()，交出交互权
```

参数约定：
- **最短展示时长**：外观设置项 `splashDurationMs`（范围 200~5000ms，步进 100，默认 400；消费侧用 `Math.min/Math.max` 夹取防手改配置文件越界，重启生效）。**取值时序红线**：时长只能在 `await loadAppConfig()` 完成后读取——配置快照在此之前尚未 hydrate，模块同步阶段/`onMounted` 首个 await 之前求值会恒为默认 400ms，用户设置静默失效（曾发缺陷）。`minDelay` 起点为 hydrate 完成时刻（本地 IPC 仅数十 ms，对起点影响可忽略）。生产构建若快于此值则补齐，避免"闪一下"的廉价感；dev 模式下 vite 编译较慢，通常自然超过。
- **3s 硬超时只兜底启动任务**（loadHosts / restoreSessions 等挂起时放行），**不截断用户配置的最短展示时长**——`race` 仅包住任务 promise，minDelay 在 race 之外 `Promise.all` 等待，保证 4000/5000ms 等设置完整兑现。
- **淡出 250ms**，`opacity 1→0` + `ease-out`；不缩放不位移，主界面在 splash 之下自然透出，即"渐显交接"。
- 主界面**不设入场隐藏**（不做 `.app{opacity:0}`）：若 JS 中途失败，主界面永远不可见的风险不可接受；淡出覆盖已足够优雅。

## 6. 兜底与降级

| 故障 | 兜底 |
|---|---|
| JS bundle 加载失败 / Vue mount 抛错 | `main.ts` 顶层注册 `window.addEventListener('error'/'unhandledrejection')` → 立即 `hideSplash()`。宁可露出错误，不让 splash 卡死窗口 |
| 前述兜底未触发（极端：错误监听前已崩） | `index.html` 内联 `<script>setTimeout(hideSplash, 8000)</script>` 硬超时（内联原生 JS，不依赖 bundle） |
| `loadHosts` 挂起 | `Promise.all` 改为带 3s 超时竞速：`Promise.race([boot, delay(3000)])`，超时也放行交接（业务自行容错，splash 不背锅） |
| 平台不支持原生窗口透明（部分 Linux 环境） | 在外观设置关闭「启动动画透明背景」，`splash-solid` 深色模式不依赖原生透明（仅 webview 渲染前一瞬不可控） |

## 7. 实现要点

涉及文件：`frontend/index.html`、`frontend/src/main.ts`、`frontend/src/App.vue`、`src-tauri/tauri.conf.json`。不新增依赖、不新增组件文件。

### 7.1 index.html（核心改动）

LOGO 直接内联 [AppLogo.vue](../frontend/src/components/AppLogo.vue) 的 SVG（无底座线框，`currentColor` 着色；发布时需与组件源同步更新，可加构建校验，见 §9）：

```html
<!doctype html>
<html lang="en" class="splash-active">
  <head>
    <meta charset="UTF-8" />
    <link rel="icon" type="image/svg+xml" href="/favicon.svg" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>Rhost</title>
    <script>
      // 启动画面背景模式：Rust 创建主窗口时经 initialization_script 在页面任何脚本
      // 之前注入 window.__RHOST_BOOT__（读 app_config.json 的 settings.splashTransparent，
      // 见 src-tauri/src/lib.rs）。关闭透明时给 html 加 splash-solid（深色底），置于 head
      // 保证零闪烁；纯浏览器 dev 访问（无注入）默认透明。
      try {
        var boot = window.__RHOST_BOOT__
        var transparent = !(boot && boot.splashTransparent === false)
        if (!transparent) document.documentElement.classList.add('splash-solid')
      } catch (e) {}
    </script>
    <style>
      html,body{margin:0;height:100%;background:transparent}
      /* splash 存活期：页面全透明（透出桌面），主界面隐藏；
         撤除后 splash-active 移除，style.css 的 body 深色底随之恢复 */
      .splash-active body{background:transparent}
      .splash-active #app{visibility:hidden}
      /* 深色底模式（设置关闭透明）：html.splash-solid 特异度 (0,1,2) 覆盖 .splash-active body */
      html.splash-solid,html.splash-solid body{background:#06090d}
      /* LOGO 悬浮层自身无任何背景 */
      #splash{position:fixed;inset:0;z-index:99999;pointer-events:none;
        display:flex;align-items:center;justify-content:center;
        font-family:'JetBrains Mono','Cascadia Code','SF Mono',ui-monospace,
        Menlo,Consolas,'Courier New',monospace;
        transition:opacity .25s ease-out}
      #splash.splash--done{opacity:0;pointer-events:none}
      /* 上移偏移放在 LOGO 自身（keyframes 两帧都要带，缺帧会导致位移渐变丢失），
         不能放在 #splash 容器上 —— 容器上移会使 inset:0 底部露缝（状态栏穿透） */
      .splash-mark{position:relative;width:96px;height:96px;color:#3ddc84;
        transform:translateY(-4vh);
        animation:splash-breathe 2s ease-in-out infinite alternate}
      .splash-glow{position:absolute;inset:-70%;border-radius:50%;opacity:.35;
        background:radial-gradient(circle,rgba(61,220,132,.28),transparent 70%);
        animation:splash-glow 2s ease-in-out infinite alternate}
      .splash-logo{position:relative;width:100%;height:100%;display:block}
      @keyframes splash-breathe{
        from{transform:translateY(-4vh) scale(1)}
        to{transform:translateY(-4vh) scale(1.035)}
      }
      @keyframes splash-glow{to{opacity:.75}}
      @media (prefers-reduced-motion:reduce){
        .splash-mark,.splash-glow{animation:none}
        .splash-glow{opacity:.5}
      }
    </style>
  </head>
  <body>
    <div id="splash">
      <div class="splash-mark">
        <i class="splash-glow"></i>
        <!-- 内联自 frontend/src/components/AppLogo.vue：线框 LOGO，currentColor 着色 -->
        <svg class="splash-logo" viewBox="0 0 24 24" fill="none" stroke="currentColor"
             stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <rect x="2.5" y="4" width="19" height="16" rx="3" />
          <path d="M7 9.5L9.5 12L7 14.5" />
          <path d="M13 14.5h4" />
        </svg>
      </div>
    </div>
    <div id="app"></div>
    <script>
      // 撤除 splash：幂等，恢复主界面可见 + 淡出后移除 DOM（docs/splash-design.md §5）
      window.__hideSplash = function () {
        document.documentElement.classList.remove('splash-active')
        document.documentElement.classList.remove('splash-solid')
        var el = document.getElementById('splash')
        if (!el || el.classList.contains('splash--done')) return
        el.classList.add('splash--done')
        setTimeout(function () { el && el.remove() }, 300)
      }
      // 硬超时兜底：即使 bundle 完全失败也不让 splash 卡死窗口（docs/splash-design.md §6）
      setTimeout(window.__hideSplash, 8000)
    </script>
    <script type="module" src="/src/main.ts"></script>
  </body>
</html>
```

呼吸实现说明：缩放与光晕分属两层，各自只动 `transform` / `opacity`（合成器路径），同周期 `alternate` 天然同相位，无需 JS 同步。LOGO 着色由 `.splash-mark` 的 `color` 经 `stroke="currentColor"` 传导，与产品内组件同一机制。

### 7.2 main.ts（注册 JS 层错误兜底）

```ts
import { createApp } from 'vue'
import './style.css'
import App from './App.vue'

// JS 层兜底：任何加载期错误立即撤掉 splash（§6）
window.addEventListener('error', () => (window as any).__hideSplash?.())
window.addEventListener('unhandledrejection', () => (window as any).__hideSplash?.())

createApp(App).mount('#app')
```

### 7.3 App.vue（真实交接时机）

`onMounted` 为"最短时长（用户可调，须在配置 hydrate 后读取）+ 启动任务"竞速：

```ts
// 设置项 splashDurationMs（200~5000，消费侧夹取防手改配置文件越界）
const clamp = (v: unknown) => Math.min(5000, Math.max(200, Number(v) || 400))

onMounted(async () => {
  initGlobalErrorReporting()
  try { await loadAppConfig() } catch (e) { /* 内置默认值兜底 */ }
  // hydrate 完成后才读用户时长；在此之前求值恒为默认 400
  const minDelay = new Promise(r => setTimeout(r, clamp(savedSettings.splashDurationMs)))
  const restBoot = (async () => {
    syncLogConfig()
    await loadHosts()
    restoreSessions()
  })()
  // 3s race 只兜底启动任务，不截断 minDelay
  await Promise.all([
    minDelay,
    Promise.race([restBoot, new Promise(r => setTimeout(r, 3000))]),
  ])
  ;(window as any).__hideSplash?.()
})
```

### 7.4 窗口创建与首帧注入（tauri.conf.json + lib.rs）

`tauri.conf.json` 的 `windows` 留空，主窗口在 `setup` 中以 `WebviewWindowBuilder` 创建——属性与原静态窗口一致（label `"main"`、1200×760 / 最小 940×600、`decorations:false`、`transparent:true`），并在页面任何脚本执行前注入启动设置：

```rust
let splash_transparent = app_cfg
    .settings.get("splashTransparent").and_then(|v| v.as_bool()).unwrap_or(true);
let boot_script = format!("window.__RHOST_BOOT__={};",
    serde_json::json!({ "splashTransparent": splash_transparent }));

WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
    /* …尺寸 / 无边框 / 透明… */
    .initialization_script(&boot_script)
    .build()?;
```

`macOSPrivateApi: true` 为 macOS WKWebView 透明的必要开关，配合 `html/body{background:transparent}` 与 `.splash-active body{background:transparent}`，splash 存活期窗口完全透明、LOGO 悬浮于桌面之上；撤除时 `splash-active` 移除，`style.css` 的 body 深色底恢复，与 LOGO 淡出同步。两点观感代价：撤除瞬间存在"桌面 → 深色主界面"的一次跳变；浅色桌面上绿描边 LOGO 对比度下降。不接受该代价的用户可在外观设置关闭透明，走 `splash-solid` 深色底模式。

## 8. 验收标准

1. 打包产物冷启动：默认窗口透明、LOGO 悬浮桌面（外观设置关闭透明则首帧即 `#06090d` 深色底），**全程无白闪**；splash ≤ 100ms 内可见（macOS / Windows）。
2. LOGO 与 [AppLogo.vue](../frontend/src/components/AppLogo.vue) 图形一致（线框终端窗口 `>_`，无底座），描边呈默认主题色 `#3ddc84`；呼吸动画周期 2s，缩放与光晕同步、平滑无跳变。
3. splash 可见时长 ≥ `splashDurationMs` 设置值（默认 400ms）；主界面 ready 后 ≤ 300ms 完成淡出并移除 DOM（DevTools 确认 `#splash` 消失）。
4. 断网 + DevTools 阻断 JS：8s 后 splash 自动撤除，窗口不永久卡动画。
5. `prefers-reduced-motion: reduce` 下呼吸停止、光晕定格中间值，淡出正常。
6. dev（vite）与 build 两种模式行为一致。
7. 无新增 npm 依赖；`vue-tsc -b` 零错误；现有前后端测试不回归。
8. 外观设置中「启动动画时长」（200~5000ms，步进 100）与「启动动画透明背景」开关均可正常保存（app_config.json settings 节），修改后重启生效；开关两种状态冷启动首帧即正确背景（注入脚本 DevTools 中可见 `window.__RHOST_BOOT__`），手改配置文件越界时长被消费侧夹取到合法范围。

## 9. 后续可选增强

1. 在 splash 中展示版本号 —— 需引入构建期注入（`vite define`），当前为避免版本漂移未展示。
2. LOGO 内联副本与 [AppLogo.vue](../frontend/src/components/AppLogo.vue) 的构建期同步校验脚本，比对两者 SVG path 内容防漂移（低优先级）。
