# 开发指南

> status: 与 main 分支当前实现一致（2026-10-05）
>
> audience: 准备在本机跑起项目、提交第一个改动的开发者
>
> scope: 环境准备、启动/构建/测试、常见开发任务步骤、编码规范、调试方法
>
> related: 系统架构与通信协议见 [architecture.md](../explanation/architecture.md)

## 1. 环境要求

| 依赖 | 版本要求 | 说明 |
|---|---|---|
| Node.js | **≥ 20.19**（建议 22 LTS） | Vite 8 的最低要求；仓库未附 `.nvmrc` |
| Rust | **≥ 1.90（stable）** | `Cargo.toml` 声明 `rust-version = "1.90"`、`edition = "2024"`；用 rustup 安装 |
| Xcode 命令行工具 | 最新 | macOS 编译原生依赖与 Tauri 必需：`xcode-select --install` |
| Docker（可选） | 任意近期版本 | 仅跑 SSH 端到端测试时需要 |

> 注意：SSH 实现使用纯 Rust 的 **russh**，不需要安装旧文档提到的 libssh2/openssl。

macOS 安装示例：

```bash
# Rust（如尚未安装）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# Node 建议用 nvm/fnm 管理；Tauri CLI 已在前端 devDependencies 中，无需全局安装
```

Windows / Linux 的额外依赖参考 Tauri 2 官方前置依赖文档（WebView2、`webkit2gtk` 等），日常开发以 macOS 为主。

## 2. 首次启动

### 方式 A：只调前端（最快，无桌面壳）

```bash
cd frontend
npm install
npm run dev
# 打开 http://localhost:5173
```

浏览器模式下 `isTauri === false`：主机列表使用 `src/data/mockHosts.ts`，点击连接会提示"浏览器模式无 Tauri 后端"，适合调试 UI、弹窗、设置、布局等纯前端逻辑。

### 方式 B：完整桌面应用（Tauri，前后端一起跑）

```bash
# 任选其一：

# 1) 从 Rust 侧启动（tauri.conf.json 的 beforeDevCommand 会自动起前端 vite）
cd src-tauri
cargo tauri dev

# 2) 或从前端侧启动（@tauri-apps/cli 会自动定位 ../src-tauri）
cd frontend
npm install
npm run tauri dev
```

- Vite 固定占用 **5173**（`strictPort: true`），端口被占用会直接报错；
- 桌面窗口加载的也是 5173，前端代码改动经 HMR 生效；**右键菜单/弹层若未热更新，在窗口内按 ⌘R 强制刷新**；
- Rust 代码改动会触发后端重新编译并重启窗口。

## 3. 构建与静态检查

```bash
# 前端：先 vue-tsc 全量类型检查，再 vite 打包（类型不过即失败）
cd frontend
npm run build

# Rust 侧常规检查（在 src-tauri 目录）
cargo check
cargo clippy        # 仓库未强制，但建议提交前本地跑
cargo test          # 含内联单测；e2e 在无 Docker 容器时自动跳过

# 完整桌面安装包
cargo tauri build
```

注意：

- TS 配置开启了 `noUnusedLocals` / `noUnusedParameters` / `noFallthroughCasesInSwitch`，未使用变量会导致构建失败；
- 仓库**尚未配置** ESLint / Prettier / rustfmt.toml，格式请遵循第 5 节的既有风格。

## 4. 测试

### 4.1 Rust 内联单测

协议编解码等纯逻辑（如 `ssh/frame.rs`）使用文件内 `#[cfg(test)] mod tests`，直接：

```bash
cd src-tauri && cargo test
```

### 4.2 SSH 端到端测试（需要本地容器）

测试文件：`src-tauri/tests/ssh_e2e.rs`，连接 `127.0.0.1:2222`（账号 `test` / `rhost123`）。无容器时测试**自动跳过**（连接失败视为环境缺失，不误报失败）。

启动测试用 sshd（e2e 默认目标）：

```bash
docker run -d --name rhost-test-sshd -p 2222:2222 \
  -e PASSWORD_ACCESS=true -e USER_NAME=test -e USER_PASSWORD=rhost123 \
  lscr.io/linuxserver/openssh-server:latest
```

仓库还自带 MOTD 全功能测试容器（Debian，含 lastlog/procps，端口 **2223**，供手工验证 MOTD/指标）：

```bash
docker build -t rhost-sshd-debian docker/debian-sshd
docker run -d --name rhost-test-sshd-debian -p 2223:22 rhost-sshd-debian
# 应用中新建主机 127.0.0.1:2223，账号 test / rhost123
```

### 4.3 前端

目前没有单元测试框架，前端改动以 `npm run build` 类型检查 + 浏览器/桌面手动验证为准。

## 5. 编码规范

### 5.1 通用风格（前端）

- TypeScript 严格模式；单引号、无分号、2 空格缩进（与全仓库现有代码保持一致）；
- 组件统一 **`<script setup lang="ts">`**，组合式 API；
- 命名：组件/类型/接口 `PascalCase`；变量/函数 `camelCase`；常量 `UPPER_SNAKE_CASE`；
- 跨组件共享类型放 `src/types.ts`，帧/指标等与后端对应的类型就近定义在 store 中，并注释"与后端 xxx 对应"；
- 不用 `any` 掩盖结构问题；`invoke<T>()` 显式标注返回类型。

### 5.2 状态管理（无 Pinia）

- 全局状态 = store 文件里的模块级单例：`export const xxx = ref(...)`，直接 import 使用；
- 高频整帧替换数据用 `shallowRef(new Map())` + 手动 `triggerRef`，避免深响应式开销；
- 组件不直接散落 `invoke` SSH 业务：优先在 store 中封装动作函数（参考 `stores/session.ts` 的 `openSession/closeSession/sendInput`）；
- 所有后端调用必须判 `isTauri`，浏览器模式给出 mock 或降级提示，不允许直接抛异常。

### 5.3 组件与 UI 约定

- 全局弹窗在 `App.vue` **常驻挂载**，通过 store 的 `showXxx` ref 控制显隐；
- 弹层/右键菜单用 `<Teleport to="body">`，显隐用 `defineModel<boolean>('visible')` 契约，外部点击/Esc/滚动关闭，document 级监听必须成对移除（参考 `components/wb/ContextMenu.vue`）；
- 拦截 xterm 等组件原生事件（copy/paste）时，在 **capture 阶段**注册抢在内部 handler 前，并保证不干扰 Ctrl+C 等无选区信号；
- 反馈统一走 `toast(msg, type, duration)`（`composables/useToast.ts`），错误一般先 `console.error` 留痕再 toast；
- 样式：全局令牌（`--bg/--panel/--border/--green…/--rhost-status-*`）定义在 `src/style.css`，组件内用 `<style scoped>`；全站等宽字体、深色基调；不要引入新的 UI/CSS 框架。

### 5.4 Rust 风格（本仓库注释密度是硬要求）

- 模块顶部写 `//!`：分层职责、存储/并发策略，复杂流程配数据流图（参考 `ssh/session.rs`、`ssh/sftp.rs`）；
- 每个 `pub` 项与结构体字段写 `///`，重点解释**为什么、边界条件、单位**；魔数常量化并注释取值理由（如 `MERGE_BYTES = 4096`、64KB 分块、30s RTT）；
- 业务错误在模块内定义 `thiserror` 枚举（如 `SshError`），`ipc.rs` 只负责 `.map_err(|e| e.to_string())`；
- 跨层错误字符串前缀即协议（如 `KEY_ENCRYPTED:`），改动前先搜前端消费方；既有前缀清单见 `architecture.md` §7.3（含主机密钥的 `HOSTKEY_UNKNOWN:` / `HOSTKEY_MISMATCH:`，payload 三段 `{algo}|{fingerprint}|{pubkey}`，`{pubkey}` 为 OpenSSH 格式完整公钥）；
- 异步铁律：
  - russh 是纯异步，**禁止用 `spawn_blocking` 包 SSH 读写**；
  - 会话池访问统一"clone Arc → 立即释放锁 → 再 await"，不持锁 await；
  - 后台任务树挂 `CancellationToken`，会话断开必须能全部退出；暂停用 `watch<bool>`，背压用有界 mpsc；
- 文件 IO 一律 tokio fs，固定缓冲流式读写，禁止整文件入内存；落盘用"临时文件 + rename"原子写；
- 远端采集/解析一律可降级：缺字段给 `N/A`/0，任何异常不得影响连接主流程；
- serde 跨层结构加 `#[serde(rename_all = "camelCase")]`。

### 5.5 Git 提交

使用 Conventional Commits + 中文描述：

```
feat: 新增 SFTP 断点续传
fix: 修复彩色提示符在无 locale 容器下的乱码
style(favicon): 更新图标设计
refactor(sftp): 传输队列改为串行调度
```

## 6. 常见开发任务（操作手册）

### 6.1 新增一个 invoke 命令

1. `src-tauri/src/ipc.rs`（或对应模块文件）定义入参结构（`Deserialize` + camelCase）与 `#[tauri::command] pub async fn xxx(...) -> Result<T, String>`，业务逻辑调用 `ssh/` 等模块；
2. 在 `src-tauri/src/lib.rs` 的 `invoke_handler!` 列表登记命令名；
3. 前端在对应 store 中 `await invoke<ResultT>('xxx', { camelCaseArgs })`，`isTauri` 分支外提供降级；
4. 如涉及权限，在 `src-tauri/capabilities/default.json` 增补。

### 6.2 新增一个设置项

1. `frontend/src/stores/settings.ts`：在 `AppSettings` 接口加字段并写 JSDoc 说明、在 `DEFAULT_SETTINGS` 给默认值；
2. `frontend/src/components/SettingsModal.vue`：在 `PANELS` 对应分组的 `rows` 加一行（`key/title/desc/kind/options/keywords`），控件渲染由数据驱动，无需改模板；搜索、脏检测、单项重置自动生效；
3. 消费方读 `savedSettings.xxx`（注意部分终端构造参数如 scrollback 只对新建终端生效）。

### 6.3 新增一种终端帧类型（前后端双改，谨慎）

1. 后端 `ssh/frame.rs` 的 `FrameType` 加变体与注释（说明 payload 与时机），编码处发送；
2. 前端 `stores/session.ts` 同步 `FRAME_*` 常量，并在 `handleFrame` 加分支；
3. JSON payload 字段风格与既有帧保持一致（指标类 snake_case，其他 camelCase），TS 接口写对齐注释；
4. 补内联单测（encode/decode roundtrip）。

### 6.4 新增全局弹窗

参考 `components/PasswordModal.vue`：store 中放 `showXxx` 单例；App.vue 常驻挂载；遮罩点击/Esc 关闭；必要时用 Promise + resolve 做"等待用户输入"（`composables/usePasswordPrompt.ts` 模式）。

### 6.5 加日志与排错

- Rust 用 `log::debug!/error!`，带 session id 与目标地址；dev 构建日志落盘：
  `~/Library/Logs/com.rhost.app/rhost.log`
- 桌面窗口可打开 WebView 开发者工具查看前端日志与网络（dev 构建）；
- 前端兜底分支的 `console.warn/error` 保留，便于排查 Tauri 能力缺失（如剪贴板、opener）。

## 7. 开发约束速查（来自既有实现，勿轻易违反）

- 一条 SSH 连接同一时刻只持有一个 SFTP 通道：传输队列串行、管理操作用 per-session Mutex；
- 终端数据只走二进制帧 Channel，禁止改成全局 Event/Emitter；
- 64KB 固定分块、传输任务独立取消令牌；断点续传按"相等跳过 / 偏小续传 / 偏大截断"；
- 传输成功才刷新对侧目录，失败/取消不刷新；
- 密码只进系统钥匙串，连接配置 JSON 不含任何敏感凭据；
- MOTD/彩色提示符注入由后端完成并 hold 初始化输出，前端不参与时序编排；
- 终端 UTF-8 全链路透传，不解析/篡改 PTY 业务字节流（OSC 6667 等自有标记除外）。

## 8. 已知的文档/工程偏差

- 根目录 `README.md` 的部分技术描述已过时（写的是 Pinia、ssh2、sled、spawn_blocking、旧目录结构），与当前实现不符时**以本文档与代码为准**；
- 仓库暂无 lint/format 配置与 CI，提交前至少保证 `npm run build` 与 `cargo test` 通过；
- 代码中注释带"逻辑待实现"的功能（如 SFTP 覆盖策略的部分分支、SHA256 断点校验）以 `settings.ts` 字段注释为准，勿假设已生效。
