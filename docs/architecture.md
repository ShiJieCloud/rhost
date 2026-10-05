# Rhost 架构与通信协议

> 状态：与 main 分支当前实现一致（2026-10-05）
> 读者：首次接触本项目、需要快速理解系统如何运转的开发者
> 范围：整体分层、前后端模块职责、IPC 协议、核心数据流、持久化与安全
> 配套文档：环境搭建与编码规范见 [development.md](./development.md)；指标采集细节见 [metrics-design.md](./metrics-design.md)

---

## 1. 项目简介

Rhost 是一款基于 **Tauri 2** 的跨平台 SSH 远程主机管理器（对标 FinalShell），核心能力：

- 多标签 SSH 终端（xterm.js 渲染，UTF-8 透传）
- 内置 SFTP 文件管理器：双栏浏览、上传/下载队列、断点续传、暂停/取消
- 主机与密钥管理：配置本地持久化，密码进系统钥匙串
- 服务器监控：MOTD 欢迎面板 + CPU/内存/磁盘/网络/进程/GPU 周期指标
- 断线重连、Shell ↔ SFTP 工作目录双向同步

架构上最核心的设计决策是：**终端高频字节流走自定义二进制帧（Tauri Channel），不经 JSON；低频控制消息走 invoke/JSON**，从而规避大量终端输出时的序列化开销与 ANSI 内容损坏。

## 2. 技术栈

| 层 | 技术 | 说明 |
|---|---|---|
| 桌面框架 | Tauri 2.12 | 无边框窗口（`decorations: false`），自绘标题栏 |
| 后端 | Rust edition 2024（rustc ≥ 1.90）、tokio 全异步 | russh 0.63 / russh-sftp 3，**纯 async，禁止 `spawn_blocking` 包 SSH IO** |
| 前端 | Vue 3.5（`<script setup>`）+ TypeScript 6 + Vite 8 | **未使用 Pinia、Vue Router**；状态用模块级单例 |
| 终端 | @xterm/xterm 6 + addon-fit + addon-web-links | 透明背景，固定 ANSI 配色 |
| 其他 | thiserror、keyring、sysinfo、chrono、uuid、tauri-plugin-opener / clipboard-manager | |
| 校验 | 前端构建含 `vue-tsc` 类型检查；Rust 内联单测 + e2e | 仓库暂无 ESLint / Prettier / rustfmt 配置 |

## 3. 仓库目录结构

```
rhost/
├── frontend/                  # Vue3 前端
│   ├── vite.config.ts         # 固定 5173 端口；Tauri devUrl 指向这里
│   └── src/
│       ├── main.ts            # 入口（仅 createApp，无路由/状态库）
│       ├── App.vue            # 根组件：HomeView / Workbench 切换 + 全局弹窗常驻
│       ├── style.css          # 全局样式与设计令牌（唯一全局 CSS）
│       ├── types.ts           # 跨模块共享类型（含与 Rust 结构对齐说明）
│       ├── views/             # 顶层视图：HomeView、Workbench；home/ 为首页子视图
│       ├── components/        # 通用组件；wb/ 子目录为工作台组件
│       ├── stores/            # 模块级单例状态（hosts/session/settings/groups/keys）
│       ├── composables/       # useToast、usePasswordPrompt
│       ├── lib/tauri.ts       # isTauri 探测、Tauri API 动态 import
│       └── data/mockHosts.ts  # 浏览器 dev 模式的 mock 数据
├── src-tauri/
│   ├── tauri.conf.json        # 窗口、devUrl、构建钩子
│   ├── capabilities/          # Tauri 权限白名单（window/opener/clipboard）
│   └── src/
│       ├── main.rs / lib.rs   # 启动入口；插件与 invoke 命令注册、State 注入
│       ├── ipc.rs             # IPC 适配层（薄）：参数校验 + 转发，无业务逻辑
│       ├── ssh/               # frame / session / manager / sftp
│       ├── metrics/           # 远端指标解析（纯函数）+ collector（周期采集）
│       ├── motd/              # MOTD 结构化指令生成
│       ├── store.rs           # connections.json + 系统钥匙串
│       ├── localfs.rs         # 本地文件系统命令
│       └── sysmon.rs          # 本机内存采集
│   └── tests/ssh_e2e.rs       # 依赖本地 Docker sshd 的端到端测试
└── docker/debian-sshd/        # MOTD 全功能测试容器（端口 2223）
```

## 4. 整体分层

```
┌──────────────────────────── 前端（Vue3）────────────────────────────┐
│  views / components（TerminalPane、DockPanel、Inspector、Settings…） │
│          │ 只依赖 stores 层的导出函数与单例 ref                       │
│  stores（session / hosts / settings / groups / keys）               │
│          │ invoke() 低频 JSON          │ Channel 高频二进制帧       │
└──────────┼──────────────────────────────┼──────────────────────────┘
           ▼                              ▼
┌─────────────────────────── Rust（Tauri）────────────────────────────┐
│  ipc.rs        #[tauri::command]：校验入参、转错误为 String           │
│    ├─ ssh/manager.rs   会话池 Arc<RwLock<HashMap<id, Arc<SshSession>>>>│
│    │    ├─ session.rs  russh 连接/认证/PTY 读写/小包合并/保活/重连     │
│    │    ├─ sftp.rs     懒加载 SFTP 子通道、流式传输、取消/暂停        │
│    │    └─ frame.rs    二进制帧编解码（纯函数 + 单测）                │
│    ├─ metrics/  motd/   独立 exec 通道采集，与 PTY 物理隔离           │
│    ├─ store.rs         JSON 配置 + 系统钥匙串（原子写）              │
│    └─ localfs.rs / sysmon.rs                                        │
└─────────────────────────────────────────────────────────────────────┘
          ▲                              ▲
          └──────── 远端服务器：SSH（PTY / SFTP subsystem / exec）─────┘
```

分层边界要点：

- **`ssh/` 模块除 manager 外不依赖 tauri**（见 `ssh/mod.rs` 头注释），可脱离 Tauri 单测/e2e。
- **`ipc.rs` 永远是薄层**：文件头注释明确"仅做参数校验与转发，不含 SSH 业务逻辑"。业务逻辑放 `ssh/` 等模块。
- **前端组件不直接 invoke SSH 业务**：统一经 `stores/session.ts` 等状态层封装，组件只消费响应式状态与动作函数。

## 5. 后端模块导览

| 文件 | 职责 | 新人关注的关键点 |
|---|---|---|
| `lib.rs` | 注册插件、注入 `SessionManager` 全局 State、登记全部 invoke 命令 | **新增命令必须在此 `invoke_handler!` 列表登记** |
| `ipc.rs` | 命令入参结构（`#[serde(rename_all = "camelCase")]`）、调用 manager、错误转 String | 连接命令会 spawn 帧转发任务：Channel send 失败即退出 |
| `ssh/frame.rs` | 帧类型枚举与 `encode_frame/decode_frame` | 帧类型是前后端共同协议常量，改动需同步前端 |
| `ssh/session.rs` | 单条 SSH 会话：russh handler、认证、PTY 读写、4KB/5ms 攒包、RTT、指标调度、彩色提示符注入 | 文件头有完整数据流 ASCII 图；常量化队列容量与超时时长 |
| `ssh/manager.rs` | 会话池生命周期：create/write/resize/disconnect、指标开关 | 统一手法：**clone Arc 后立刻释放锁，禁止持锁 await** |
| `ssh/sftp.rs` | 每会话懒加载一个 `RawSftpSession`；列目录解析 longname；64KB 流式传输；`TransferCtl`（取消令牌 + 暂停 watch）；管理操作 per-session Mutex 串行 | 一条 SSH 连接同一时刻只有一个 SFTP 通道，传输队列因此串行 |
| `metrics/` | `HostInfo`（连接时一次）与 `Metrics`（周期）解析；全是输入命令文本的纯函数 | 任何解析失败降级为 `N/A`/0，不影响主连接 |
| `motd/` | 生成结构化指令 `[{t,text,cls}]`，前端渲染为 ANSI | 后端不下发终端字节，只下发结构化数据 |
| `store.rs` | `connections.json`（原子写：临时文件 + rename）+ 系统钥匙串 | 密码/口令绝不入 JSON |
| `localfs.rs` | 本地侧目录列举与文件操作（供 SFTP 面板本地栏） | |
| `sysmon.rs` | Rhost 主进程 RSS，驱动底部状态栏 | |

## 6. 前端模块导览

### 6.1 视图骨架（App.vue）

应用只有"首页 / 工作台"两个顶层视图，**不用路由**：

- `appView`（`stores/session.ts`）在 `'home' | 'workbench'` 间切换；
- Workbench 一旦挂载过就用 `v-show` 保活（终端/xterm 状态不被销毁），即使关闭全部会话也不卸载；
- 全局弹窗（NewConnection / Settings / Group / Keys / Password / Toast）在 App.vue 常驻挂载，通过各 store 的 `showXxx` ref 控制显隐。

工作台结构（`views/Workbench.vue`）：`WbSideBar + (WbTabs → TerminalPane → DockPanel) + Inspector + StatusBar`。

### 6.2 stores（模块级单例，非 Pinia）

| 模块 | 内容 |
|---|---|
| `stores/session.ts`（最大，~840 行） | 会话列表/活动会话、布局状态持久化、**二进制帧解析与分发**、建连/重连/断开、终端 sink 注册补发、resize 节流、指标/心跳 |
| `stores/hosts.ts` | 主机列表、筛选、与后端 `StoredHost` 的双向转换、增删改 |
| `stores/settings.ts` | `AppSettings` 接口 + 默认值 + localStorage 持久化 + 强调色应用 |
| `stores/groups.ts` / `stores/keys.ts` | 分组、密钥管理 |

约定：`export const xxx = ref(...)` 即为全局单例；高频整帧替换的数据用 `shallowRef(new Map())` + 手动 `triggerRef`。

### 6.3 双环境运行（浏览器也能开页面）

`lib/tauri.ts` 导出 `isTauri`（检测 `window.__TAURI_INTERNALS__`）。所有后端调用点都做降级：

- Tauri 环境：invoke / Channel 真实后端；
- 浏览器 dev（`npm run dev` 直接开 5173）：主机用 `MOCK_HOSTS`，终端连接打印 `[dev] 浏览器模式无 Tauri 后端`，页面与交互仍可调试。

## 7. 前后端通信协议（重点）

### 7.1 双通道分工

| 通道 | 用途 | 数据形态 |
|---|---|---|
| **Tauri `Channel<Vec<u8>>`** | 终端输出、MOTD/HostInfo/Metrics/RTT/CWD/Algo 帧、SFTP 传输进度 | 自定义二进制帧，不做 JSON 序列化 |
| `invoke(cmd, args)` | 建连、键盘写入、resize、断开、SFTP 管理、配置读写、指标心跳 | JSON，返回 `Result<T, String>` |

每个会话在前端 `connectBackend` 时新建**独立 Channel**（数据流隔离，不全局广播）；后端在 `connect_ssh` 中 spawn 转发任务把帧 mpsc 推入 Channel，前端释放 Channel 后 send 失败，转发任务自动退出。

### 7.2 二进制帧格式

```
┌ type:u8 ┬ length:u32 大端 ┬ payload (length 字节) ┐
```

类型定义以后端 `src-tauri/src/ssh/frame.rs` 为准，前端在 `stores/session.ts` 维护同值常量（`FRAME_*`）：

| type | 名称 | payload | 时机 |
|---|---|---|---|
| 0x01 | Data | PTY 原始字节（stdout/stderr 合并） | 流式 |
| 0x02 | Exit | 可选关闭原因（UTF-8） | 会话结束 |
| 0x03 | Error | 错误文本（协议预留，当前运行期错误并入 Exit） | — |
| 0x04 | Motd | JSON 指令数组 `[{t,text,cls}]` | **首帧**，先于一切 PTY 数据 |
| 0x05 | HostInfo | JSON 主机静态信息 | 建连一次 |
| 0x06 | Metrics | JSON 动态指标（cpu/mem/net/disks/procs/gpus） | 周期 |
| 0x07 | Rtt | JSON `{"ms":23}` | 建连即测 + 30s |
| 0x08 | Cwd | 绝对路径文本（OSC 6667 解析得到） | Shell cd 后 |
| 0x09 | Algo | JSON 真实协商算法（host_key/cipher/term/enc） | 建连一次 |

前端解析注意（`session.ts` 中均有实现）：

- Channel 回调拿到的可能是 `number[] / ArrayBuffer / Uint8Array`，统一用 `toU8()` 转换；
- 长度按大端组装后 `>>> 0`；payload 用 `subarray`，不拷贝；
- 单帧 JSON parse 失败静默丢弃，**绝不影响终端主流程**；
- 终端 sink 尚未注册（组件未挂载）时帧进入 `pendingFrames` 排队，注册后补发，避免丢失连接初期输出。

### 7.3 invoke 序列化约定

- Rust 侧结构体一律 `#[serde(rename_all = "camelCase")]`，前端用 camelCase；
- 例外：**帧内 JSON 指标字段保留 snake_case**（如 `swap_total`、`rx_rate`），TS 接口注释标注"与后端 xxx 对应，snake_case 保紧凑"；
- 前端向 `write_terminal` 传 `Vec<u8>` 时必须 `Array.from(bytes)`（Uint8Array 会被 JSON 序列化成对象）；
- 命令错误统一是字符串；**错误前缀即协议**：如私钥口令错误返回 `KEY_ENCRYPTED: …`，前端据此弹口令框并重试，不要改成普通文案。

## 8. 核心数据流

### 8.1 建连与终端收发

1. 前端 `openSession(hostId)` → 创建前端会话（复用同主机旧会话）→ 切到 workbench；
2. `TerminalPane` 挂载 xterm 后调用 `attachTerminal(id, sink, cols, rows)`，注册 sink 并触发 `connectBackend`；
3. `connectBackend` 解析密码（会话缓存 → 系统钥匙串 → 专用密码弹窗 → 回写钥匙串），新建 Channel，invoke `connect_ssh`；
4. 后端 `manager.create` 完成 russh 连接认证，注册会话池，spawn 帧转发；PTY 开启前/初期按设置完成 MOTD、彩色提示符注入（输出被 hold 到不可见 marker 再放行，首帧即最终画面）；
5. 后续：键盘 `term.onData → sendInput → write_terminal`；远端输出 `Data 帧 → sink → xterm.write`；
6. Exit 帧 → 状态置 offline 并在终端打印关闭提示；网络层异常由重连逻辑（`reconnectTick`/`reconnectBackend`）处理，重连途中的旧 Exit 静默收尾。

### 8.2 PTY resize

xterm fit → `onResize` → 前端节流（leading 立即发 + 50ms trailing 合并，trailing dirty 时收尾强制再发一次）→ `resize_terminal` → russh window-change。代码与原因注释见 `stores/session.ts` 的 `RESIZE_TRAIL_MS` 段落。

### 8.3 指标采集（另见 metrics-design.md）

只有"活动 Tab + Inspector 可见"的会话采集：前端 `start_metrics` 带间隔启动后端 `MetricsCollector`，经**独立 exec 通道**跑普通用户可读命令（禁止 sudo/落盘/拼接），解析为 Metrics 帧周期推送；面板隐藏时心跳停止，后端停采集；会话断开随 CancellationToken 清理。

### 8.4 SFTP 浏览与传输

- 每会话首次 SFTP 操作时懒加载 SFTP 子通道（复用已认证 SSH 连接），异常失效后下次自动重建；
- 列目录自行收发 READDIR 并解析 `longname`（高层 API 会丢弃属主/属组名）；
- 上传/下载：固定分块（默认 64KB，设置可调 32KB–1MB）顺序流式读写；进度走独立 Channel（100ms 节流，收尾必发）；
- 每个任务绑定独立 `CancellationToken`（取消即关句柄、释放流）与 `watch<bool>`（暂停/继续）；前端传输队列串行执行（单通道约束）；
- 断点续传：比较对端/本地已有文件大小，等于总大小跳过、小于则偏移续传、大于则截断重传；
- Shell ↔ SFTP 目录双向同步：Shell 内 cd → 后端解析 OSC 6667 → 0x08 Cwd 帧 → SFTP 面板跟随；面板内切目录 → 向 Shell 发送 cd。

### 8.5 配置与密码

- 主机非敏感配置：`app_data_dir/connections.json`（带 version，全量写时临时文件 + rename）；
- 密码 / 私钥口令：仅系统钥匙串（macOS 经 `security` 命令绕 ad-hoc 签名 ACL，其他平台 keyring crate）；前端永不预加载密码，连接时按需读取；
- 前端偏好（设置）与工作台布局：localStorage（`rhost.settings`、`rhost.layout.v1`），写盘 200ms 防抖，读取失败回退默认。

## 9. 安全与权限边界

- Tauri capabilities 白名单最小化：仅 window 拖拽/最小化/最大化/关闭、opener 打开 URL、剪贴板读写（见 `src-tauri/capabilities/default.json`）；
- 终端链接点击经 opener 插件交系统浏览器（WebView 内 window.open 不可靠）；
- 剪贴板统一用 Tauri 插件（WKWebView 对 Web Clipboard API 有限制）；多行/危险控制字符粘贴默认弹确认（pasteGuard，可在设置开关）；
- 远端指标/初始化遵循"零命令拼接、零远端落盘（初始化脚本除外且自动降级）、零 sudo、零常驻"；
- 私钥只记录本地路径，不上传任何第三方。

## 10. 新增功能时先看哪里

| 你要做的事 | 首先阅读 |
|---|---|
| 加一个后端命令 | `src-tauri/src/ipc.rs` 命令写法 + `lib.rs` 的登记列表 + 前端调用处 |
| 加一种终端帧 | `ssh/frame.rs` 枚举 + `stores/session.ts` `FRAME_*` 与 `handleFrame` |
| 加一个设置项 | `stores/settings.ts`（接口 + 默认值）+ `components/SettingsModal.vue` 的 `PANELS` |
| 改终端行为 | `components/wb/TerminalPane.vue` + `stores/session.ts` |
| 改 SFTP | `components/wb/DockPanel.vue` + `ssh/sftp.rs` + `ipc.rs` 相关命令 |
| 加全局弹窗/提示 | `composables/useToast.ts`、`components/PasswordModal.vue` 模式 + App.vue 常驻挂载 |
