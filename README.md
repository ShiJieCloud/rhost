# Rhost
> Cross-platform High-performance Remote Host Manager
> 跨平台高性能远程主机管理器（SSH终端 + SFTP + 端口转发）

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macos%20%7C%20windows%20%7C%20linux-green)]()
[![Rust](https://img.shields.io/badge/rust-1.75+-orange.svg)]()
[![Tauri](https://img.shields.io/badge/tauri-2.0-purple.svg)]()

**Rhost** 是一款基于 Rust + Tauri 2 开发的开源桌面远程主机管理工具，对标 FinalShell。
采用**原生二进制IPC流**架构，解决 Electron 类终端工具内存高、多连接并发卡顿问题。

## ✨ 核心特性
- 🖥️ **多标签SSH终端**：同时打开多个远程会话，独立会话隔离管理
- 📁 **内置SFTP文件管理器**：浏览目录、上传/下载、拖拽传输、修改文件权限
- 🔗 **SSH跳板机 & 端口转发**：支持跳转主机、本地端口转发
- 🔐 **安全凭证存储**：密码/私钥口令存入系统密钥环（Mac钥匙串 / Windows凭据管理器 / Linux密钥环），配置信息本地Sled持久化
- 📂 **服务器分组管理**：主机分组、备注、快速连接、连接状态监控
- ♻️ **连接保活 & 断线重连**：自动心跳检测，网络恢复自动重连
- ⚡ **高性能二进制流**：Tauri IPC Channel 直接传输原始字节流，**无JSON、无Base64编码损耗**，UI永不阻塞
- 🎨 **原生跨平台桌面**：基于Tauri，体积小、内存占用低，支持macOS / Windows / Linux

## 📐 技术架构
- **后端**：Rust + Tokio + ssh2 + sled + tauri-plugin-keyring
  - 所有SSH阻塞IO全部放入`spawn_blocking`线程池，避免阻塞UI
  - 每个SSH会话绑定独立IPC Channel，数据流完全隔离
  - Rust层统一会话生命周期管理，防止内存泄漏
- **前端**：Vue3 + TypeScript + Xterm.js + Pinia
  - 原始二进制流通过IPC Channel接收，直接送入xterm渲染ANSI终端序列
  - 组件化拆分：服务器树、终端面板、SFTP面板、全局设置

> 架构亮点：**终端数据流不走JSON序列化**，彻底规避大量输出场景下的序列化开销、ANSI内容损坏问题。

## 📸 截图（待补充）
> 开发阶段，截图后续补充
![preview](docs/assets/preview.png)

## 🚀 快速开发
### 环境依赖（macOS）
```bash
# 安装Xcode命令行工具
xcode-select --install
# 系统库依赖
brew install libssh2 openssl@3 cmake pkg-config
# 安装rustup
curl --proto '=https' --tlsv1.2 https://sh.rustup.rs -sSf | sh
source "$HOME/.cargo/env"
# 安装Tauri CLI
cargo install tauri-cli --version "^2.0.0" --locked
```

### 拉取代码 & 启动开发
```bash
git clone https://github.com/xxx/rhost.git
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
├── docs/                 # 开发文档系列
├── frontend/             # Vue3前端
├── src-tauri/            # Rust后端
│   ├── src/
│   │   ├── session/      # 全局会话管理器
│   │   ├── ssh/          # SSH连接、PTY、保活
│   │   ├── sftp/         # SFTP文件操作
│   │   ├── tunnel/       # 跳板、端口转发
│   │   ├── storage/      # Sled本地存储
│   │   ├── crypto/       # 系统密钥环封装
│   │   └── utils/        # 通用工具与错误定义
└── .github/workflows/    # CI自动打包脚本
```

## 📖 开发文档
项目连载开发文档，可在`docs/`目录查看：
1. [01-project-init.md](docs/01-project-init.md) 项目立项 & 整体架构设计
2. [02-env-config.md](docs/02-env-config.md) 项目初始化 & 工程环境配置规范
3. [03-session-model.md](docs/03-session-model.md) SSH会话模型 & IPC二进制数据流（待编写）

## 🤝 贡献指南
> 项目尚在早期开发阶段，欢迎提交Issue讨论需求、Bug。
1. Fork 本仓库
2. 创建你的功能分支：`git checkout -b feature/xxx`
3. 提交改动：`git commit -m 'feat: add xxx'`
4. 推送分支：`git push origin feature/xxx`
5. 提交Pull Request

## ⚠️ 注意事项
- 本项目为开源学习项目，**不提供任何担保**，生产环境使用请自行评估风险。
- 服务器密码不会明文保存在本地数据库，全部交由操作系统密钥环安全存储。
- 私钥文件路径仅做记录，不会上传或同步至任何第三方服务器。

## 📄 License
MIT License. See [LICENSE](LICENSE) file.
