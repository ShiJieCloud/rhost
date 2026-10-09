<div align="center">
  <img src="https://cdn.jsdelivr.net/gh/ShiJieCloud/rhost@main/docs/public/logo.svg" alt="Rhost Logo" width="160" />
</div>

<h1 align="center">Rhost</h1>

<p align="center">
  跨平台高性能远程主机管理器（SSH 终端 + SFTP + 端口转发 + 服务器监控）
</p>

<p align="center">
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/badge/license-MIT-blue.svg" /></a>
  <img alt="Platform" src="https://img.shields.io/badge/platform-macos%20%7C%20windows%20%7C%20linux-green" />
  <img alt="Rust" src="https://img.shields.io/badge/rust-1.90+-orange.svg" />
  <img alt="Tauri" src="https://img.shields.io/badge/tauri-2.12-purple.svg" />
  <img alt="Vue" src="https://img.shields.io/badge/vue-3.5-42b883.svg" />
</p>

<p align="center">
  <a href="#-快速开始">快速开始</a> · <a href="docs/index.md">文档</a> · <a href="CHANGELOG.md">更新日志</a>
</p>

---

**Rhost** 是一款基于 Rust + Tauri 2 开发的开源桌面远程主机管理工具。
采用**原生二进制 IPC 流**架构，终端高频字节流走自定义二进制帧 Channel，不经 JSON；低频控制消息走 invoke/JSON，从而规避大量终端输出时的 JSON 序列化开销与 ANSI 内容损坏，解决 Electron 类终端工具内存高、多连接并发卡顿问题。

## ✨ 核心特性

- 🖥️ **多标签 SSH 终端**：xterm.js 渲染、UTF-8 全链路透传；每会话独立 IPC Channel，数据流完全隔离
- ♻️ **断线自动重连**：区分断线原因，网络意外中断触发指数退避自动重连（1s 起翻倍、封顶 30s），手动断开 / 远端正常退出不重连
- 📁 **内置 SFTP 文件管理器**：双栏浏览、流式传输队列、断点续传、暂停 / 取消；Shell 与 SFTP 工作目录双向同步
- 🔗 **SSH 端口转发**：本地端口转发、远程端口转发、SOCKS5 动态代理，状态实时推送与流量统计
- 📈 **服务器监控**：MOTD 欢迎面板 + CPU / 内存 / 磁盘 / 网络 / 进程 / GPU 周期指标，经独立 exec 通道采集，零 sudo、零远端落盘
- 🔐 **主机密钥校验**：TOFU（首次信任）+ known_hosts 指纹持久化，密钥变更弹红色警告，防中间人攻击
- 🔑 **安全凭证存储**：密码 / 私钥口令存入系统密钥环（macOS 钥匙串 / Windows 凭据管理器 / Linux libsecret），配置本地 JSON 原子写持久化，**绝不入配置文件**
- 📂 **主机与密钥管理**：分组、备注、快速连接、连接状态监控、密钥管理
- 📤 **配置导入导出**：整体配置与主机列表批量导入导出，导入后一键重启生效
- 📝 **应用日志系统**：启动 / 退出埋点序列、日志持久化与过期清理、前端日志面板实时订阅
- ⚡ **高性能纯异步内核**：russh 纯 async 实现，4KB/5ms 攒包、有界背压，UI 永不阻塞
- 🎨 **原生跨平台桌面**：基于 Tauri，体积小、内存占用低，支持 macOS / Windows / Linux

## 📐 技术架构

- **后端**：Rust（edition 2024）+ Tokio 全异步 + russh + russh-sftp + keyring
  - 所有 SSH IO 纯 async，每个会话绑定独立 IPC Channel，Rust 层统一会话生命周期管理
  - 终端字节流走自定义二进制帧（`type:u8 + len:u32 + payload`），低频控制消息走 invoke / JSON
  - `ssh/` 模块除 manager 外不依赖 Tauri，可独立单测 / e2e；`ipc.rs` 永远是薄层（仅参数校验与转发）
- **前端**：Vue 3.5（`<script setup>`）+ TypeScript + Vite + Xterm.js
  - 原始二进制流经 IPC Channel 接收，直接送入 xterm 渲染 ANSI 终端序列
  - 模块级单例状态管理（未使用 Pinia / Vue Router），组件化拆分：服务器树、终端面板、SFTP 面板、监控面板、全局设置

> 架构亮点：**终端数据流不走 JSON 序列化**，大量输出无序列化开销、ANSI 内容不损坏。

## 📸 截图

<p align="center">
  <img src="https://cdn.jsdelivr.net/gh/ShiJieCloud/rhost@main/docs/assets/workbench-main.png" alt="工作台全景" /><br/>
  <sub>工作台全景：多标签终端 + MOTD 欢迎面板 + SFTP 文件管理 + 实时服务器监控</sub>
</p>

<p align="center">
  <img src="https://cdn.jsdelivr.net/gh/ShiJieCloud/rhost@main/docs/assets/sftp.png" alt="SFTP 文件管理" /><br/>
  <sub>SFTP 双栏文件管理：流式传输队列、断点续传、暂停 / 取消与文件校验</sub>
</p>

<p align="center">
  <img src="https://cdn.jsdelivr.net/gh/ShiJieCloud/rhost@main/docs/assets/tunnel-pane.png" alt="端口转发" /><br/>
  <sub>可视化端口转发：本地 / 远程 / SOCKS5 规则，SSH 命令实时预览</sub>
</p>

<p align="center">
  <img src="https://cdn.jsdelivr.net/gh/ShiJieCloud/rhost@main/docs/assets/quick-connect.png" alt="快速连接" /><br/>
  <sub>快速连接：输入 SSH 命令一键建立连接，内置示例与历史</sub>
</p>

## 🚀 快速开始

### 环境依赖（macOS）

```bash
# 安装 Xcode 命令行工具
xcode-select --install
# 系统库依赖
brew install libssh2 openssl@3 cmake pkg-config
# 安装 rustup
curl --proto '=https' --tlsv1.2 https://sh.rustup.rs -sSf | sh
source "$HOME/.cargo/env"
# 安装 Tauri CLI
cargo install tauri-cli --version "^2.0.0" --locked
```

### 环境依赖（Windows）

- 安装 [rustup](https://rustup.rs/)（推荐 `stable-msvc` 工具链）与 [Node.js](https://nodejs.org/)（建议用 [nvm-windows](https://github.com/coreybutler/nvm-windows) 管理）

### 环境依赖（Linux）

Tauri 2 使用 `webkit2gtk-4.1`（不再使用 4.0），各发行版包名略有差异：

```bash
# Ubuntu / Debian
sudo apt update
sudo apt install -y libwebkit2gtk-4.1-dev libgtk-3-dev \
  libayatana-appindicator3-dev librsvg2-dev libssl-dev \
  cmake pkg-config build-essential

# Fedora
sudo dnf install -y webkit2gtk4.1-devel gtk3-devel \
  libayatana-appindicator3-devel librsvg2-devel openssl-devel \
  cmake pkgconf-pkg-config gcc-c++

# Arch Linux
sudo pacman -S --needed webkit2gtk-4.1 gtk3 \
  libayatana-appindicator librsvg libressl cmake pkgconf base-devel
```

> SSH 连接基于纯 Rust 的 [russh](https://crates.io/crates/russh) 实现，无需系统 `libssh2` 或 `openssl` 动态库；Tauri CLI 已在前端 `devDependencies` 中，无需全局安装。

### 拉取代码 & 启动开发

```bash
git clone https://github.com/ShiJieCloud/rhost.git
cd rhost

# 安装前端依赖
cd frontend
npm install
cd ..

# 启动开发模式
cargo tauri dev
```

### 构建生产安装包

```bash
cargo tauri build
```

## 📂 项目目录

```
rhost/
├── .github/
│   └── workflows/                # CI 流水线
│       ├── release.yml            #   三平台打包与发布
│       └── docs.yml               #   文档站构建与部署
├── docs/                          # VitePress 文档站
├── docker/
│   └── debian-sshd/               # MOTD 全功能测试容器（端口 2223）
├── frontend/                      # Vue 3 前端
│   ├── src/
│   │   ├── main.ts                #   入口（无路由 / 状态库）
│   │   ├── App.vue                #   根组件：首页 / 工作台切换 + 全局弹窗常驻
│   │   ├── views/                 #   顶层视图：HomeView、Workbench
│   │   ├── components/            #   通用组件；wb/ 为工作台组件
│   │   ├── stores/                #   模块级单例状态（session/hosts/settings/groups/keys/tunnels/applog）
│   │   ├── composables/           #   useToast / usePasswordPrompt / useHostKeyPrompt
│   │   ├── lib/tauri.ts           #   isTauri 探测与 Tauri API 动态 import
│   │   └── types.ts               #   跨模块共享类型
│   ├── vite.config.ts             #   固定 5173 端口
│   └── package.json
├── src-tauri/                     # Rust 后端
│   ├── capabilities/              #   Tauri 权限白名单
│   ├── icons/                     #   应用图标（macOS / Windows / Linux / iOS / Android）
│   ├── tests/
│   │   └── ssh_e2e.rs             #   依赖本地 Docker sshd 的端到端测试
│   ├── src/
│   │   ├── main.rs / lib.rs       #   启动入口；插件与命令注册、State 注入
│   │   ├── ipc.rs                 #   IPC 适配层（薄）：参数校验 + 转发
│   │   ├── ssh/                   #   SSH 连接、PTY、SFTP、保活、隧道
│   │   │   ├── frame.rs            #     二进制帧编解码
│   │   │   ├── session.rs          #     russh 连接 / 认证 / PTY 读写 / 攒包 / 重连
│   │   │   ├── manager.rs         #     会话池生命周期管理
│   │   │   ├── sftp.rs             #     SFTP 子通道、流式传输、取消 / 暂停
│   │   │   └── tunnel/             #     端口转发与 SOCKS5
│   │   ├── metrics/               #   远端指标解析与周期采集
│   │   ├── motd/                   #   MOTD 结构化指令生成
│   │   ├── applog/                 #   应用日志系统（hub / logger / writer / persisted）
│   │   ├── store.rs                #   connections.json + 系统钥匙串
│   │   ├── localfs.rs              #   本地文件系统操作
│   │   ├── known_hosts.rs          #   主机密钥指纹持久化
│   │   ├── config_io.rs           #   配置读写
│   │   ├── config_crypto.rs       #   配置加密
│   │   ├── config_migrate.rs      #   配置迁移
│   │   ├── fonts.rs                #   系统字体检测
│   │   └── sysmon.rs               #   本机内存采集
│   ├── tauri.conf.json            #   窗口、devUrl、构建钩子
│   ├── build.rs
│   └── Cargo.toml
├── package.json                   # 文档站与 commit 工具脚本
├── pnpm-workspace.yaml
├── CHANGELOG.md                   # 由 conventional-changelog 自动生成
├── LICENSE
└── README.md
```

## 🤝 贡献

> 项目尚在早期开发阶段，欢迎提交 Issue 讨论需求与 Bug。

1. Fork 本仓库
2. 创建功能分支：`git checkout -b feature/xxx`
3. 提交改动：`git commit -m 'feat: add xxx'`
4. 推送分支：`git push origin feature/xxx`
5. 提交 Pull Request

### Commit 规范

本项目采用 [Conventional Commits](https://www.conventionalcommits.org/zh-hans/v1.0.0/) 规范，提交信息格式为 `type[(scope)]: description`：

- `feat` 新功能 / `fix` 修复 / `docs` 文档 / `refactor` 重构 / `perf` 性能 / `test` 测试 / `chore` 杂务 / `style` 格式
- description 用简明中文或英文，示例：`feat(tunnel): 支持实时流量统计`、`fix(ssh): 修复断线端口泄漏`

仓库已配置 `commitlint` + `husky` 钩子，`git commit` 时自动校验 `commit-msg`，不符合规范会被拒绝；`CHANGELOG.md` 由 `conventional-changelog` 据此自动生成。

## ⚠️ 注意事项

- 本项目为开源学习项目，**不提供任何担保**，生产环境使用请自行评估风险。
- 服务器密码不会明文保存在本地配置文件，全部交由操作系统密钥环安全存储。
- 私钥文件路径仅做本地记录，不会上传或同步至任何第三方服务器。

## 📄 License

[MIT](LICENSE) © Rhost Contributors
