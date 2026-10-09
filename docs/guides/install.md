# 安装

> description: 各平台下载安装 Rhost、系统要求、运行时权限说明、升级与卸载
>
> created: 2026-10-09 15:17:19
>
> updated: 2026-10-09 15:17:19
>
> author: [sjzhao](https://github.com/ShiJieCloud/rhost)


## 1. 系统要求

| 系统 | 最低版本 | WebView 运行时 | 说明 |
|---|---|---|---|
| macOS | 10.15 Catalina | WKWebView（系统内置） | Apple Silicon 与 Intel 均支持 |
| Windows | 10 | WebView2 Runtime | Windows 11 内置；Windows 10 早期版本需手动安装 |
| Linux | glibc ≥ 2.31 | webkit2gtk-4.1 | Ubuntu 20.04+ / Debian 11+ / Fedora 36+ / Arch |

Rhost 基于 Tauri 2，SSH 连接由纯 Rust 的 `russh` 实现，**无需安装 `libssh2` 或 `openssl` 系统库**。

## 2. 获取安装包

前往 [Releases 页面](https://github.com/ShiJieCloud/rhost/releases)，按系统选择：

| 系统 | 安装包 | 适用 |
|---|---|---|
| macOS Apple Silicon | `*_aarch64.dmg` | M1/M2/M3/M4 |
| macOS Intel | `*_x64.dmg` | Intel 芯片 |
| Windows | `*_x64-setup.exe` | NSIS 安装器 |
| Windows | `*_x64.msi` | MSI 安装器 |
| Linux | `*_amd64.deb` | Debian / Ubuntu |
| Linux | `*_x86_64.rpm` | Fedora / RHEL |
| Linux | `*_x86_64.AppImage` | 免安装，全发行版 |

> 下载后建议校验哈希：Releases 页面附带每个文件的 `SHA256`，可用 `shasum -a 256 文件名`（macOS/Linux）或 `certutil -hashfile 文件名 SHA256`（Windows）核对。

## 3. macOS 安装

### 3.1 安装

1. 双击 `.dmg`，把 `rhost.app` 拖入 `Applications` 文件夹；
2. 首次启动：右键 `rhost.app` → 「打开」→ 在弹出的 Gatekeeper 窗口点「打开」。

> Rhost 当前未做 Apple 公证，首次直接双击会提示「无法打开，因为来自身份不明的开发者」。右键「打开」可放行；若仍提示「已损坏」，见下一节。

### 3.2 移除隔离属性

若双击提示「"rhost" 已损坏，无法打开」，在终端执行：

```bash
xattr -dr com.apple.quarantine /Applications/rhost.app
```

此命令移除 macOS 对下载文件的隔离扩展属性。之后双击即可打开。右键「打开」对此「已损坏」错误无效，必须用此命令。

### 3.3 辅助功能权限（可选）

终端复制粘贴走 Tauri 剪贴板插件（不依赖辅助功能）。若发现粘贴行为异常，可在「系统设置 → 隐私与安全性 → 辅助功能」中添加 `rhost.app` 并启用。

## 4. Windows 安装

### 4.1 安装

1. 双击 `*_x64-setup.exe` 或 `*_x64.msi`；
2. 首次运行若出现 SmartScreen「已保护你的电脑」，点「更多信息」→「仍要运行」；
3. 按向导完成安装，默认安装到 `%LOCALAPPDATA%\Programs\rhost\`。

### 4.2 WebView2 Runtime

- Windows 11 内置 WebView2 Runtime，无需额外操作；
- Windows 10 早期版本若未安装，启动时会提示。前往 [WebView2 Runtime 官网](https://developer.microsoft.com/microsoft-edge/webview2/) 下载「Evergreen Standalone Installer」安装后重启 Rhost。

## 5. Linux 安装

### 5.1 系统依赖

Tauri 2 使用 `webkit2gtk-4.1`（不再使用旧的 4.0）：

```bash
# Ubuntu / Debian
sudo apt install -y libwebkit2gtk-4.1-0 libgtk-3-0 \
  libayatana-appindicator3-1 librsvg2-2

# Fedora
sudo dnf install -y webkit2gtk4.1 gtk3 \
  libayatana-appindicator3 librsvg2

# Arch Linux
sudo pacman -S --needed webkit2gtk-4.1 gtk3 \
  libayatana-appindicator librsvg
```

### 5.2 deb / rpm

```bash
# Debian / Ubuntu
sudo dpkg -i rhost_*_amd64.deb
sudo apt install -f   # 自动补齐缺失依赖

# Fedora / RHEL
sudo rpm -i rhost-*_x86_64.rpm
```

### 5.3 AppImage（免安装）

```bash
chmod +x rhost-*_x86_64.AppImage
./rhost-*_x86_64.AppImage
```

AppImage 不修改系统，适合无 root 权限或临时使用。如需系统集成（图标、文件关联），可用 [AppImageLauncher](https://github.com/TheAssassin/AppImageLauncher)。

## 6. 运行时权限说明

Rhost 遵循最小权限原则，仅在需要时申请权限。下表按平台说明实际使用的权限。

### 6.1 应用自身权限（capabilities）

Rhost 通过 `src-tauri/capabilities/default.json` 声明最小权限集：

| 权限 | 用途 |
|---|---|
| `clipboard-manager` 读写 | 终端复制选中文本、粘贴内容到终端 |
| `dialog` 打开/保存 | 选择私钥文件、导入/导出配置时选目录 |
| `opener` 打开 URL/路径 | 终端内点击 http(s) 链接交系统浏览器、reveal 日志目录 |
| `process` 重启 | 设置改动后重启应用使其生效 |
| `core:window` 拖拽/最小化/最大化/关闭 | 自绘标题栏窗口操作 |

未声明 `fs`（文件系统插件）、`http`（网络插件）、`shell`（命令执行）等高危权限。本地文件操作经 Rust 侧 `localfs` 命令处理，不经过 Tauri fs 插件权限网关；SSH 连接由 Rust 原生 socket 发起，不经过 Tauri http 插件。

### 6.2 系统钥匙串（凭据存储）

密码与私钥口令存入系统钥匙串，**不写入配置文件**：

| 系统 | 钥匙串服务 | 首次使用行为 |
|---|---|---|
| macOS | Keychain | 系统弹「rhost 想要使用钥匙串」授权，输入登录密码确认 |
| Windows | 凭据管理器 | 无弹窗，后台写入「Windows 凭据」 |
| Linux | libsecret（GNOME Keyring / KDE Wallet） | 首次解锁 keyring 时输入密码 |

私钥文件本身只记录本地路径，**绝不上传**任何第三方。

### 6.3 数据存储位置

| 平台 | 应用数据目录（配置、known_hosts） | 日志目录 |
|---|---|---|
| macOS | `~/Library/Application Support/com.rhost.app/` | `~/Library/Logs/com.rhost.app/` |
| Windows | `%APPDATA%\com.rhost.app\` | `%LOCALAPPDATA%\com.rhost.app\logs\` |
| Linux | `~/.local/share/com.rhost.app/` | `~/.local/share/com.rhost.app/logs/` |

关键文件：
- `connections.json`：主机与分组配置；
- `app_config.json`：应用设置；
- `known_hosts.json`：已信任主机指纹（不参与配置导入导出）。

## 7. 升级

1. 退出运行中的 Rhost；
2. 从 [Releases](https://github.com/ShiJieCloud/rhost/releases) 下载新版安装包；
3. 直接安装覆盖旧版（macOS 拖入 `/Applications` 替换、Windows 安装器覆盖、Linux 重装包）；
4. 启动新版。

升级会保留：主机配置、设置、`known_hosts`、钥匙串中的密码/口令。跨大版本升级若配置 schema 变更，启动时会自动迁移并提示。

> AppImage 升级：下载新版 `AppImage` 替换旧文件即可，配置不受影响。

## 8. 卸载

### 8.1 移除程序

```bash
# macOS
rm -rf /Applications/rhost.app

# Windows：控制面板 → 程序 → 卸载 rhost
# 或 winget uninstall com.rhost.app
```

```bash
# Linux deb
sudo dpkg -r rhost

# Linux rpm
sudo rpm -e rhost

# AppImage：直接删除文件
rm rhost-*_x86_64.AppImage
```

### 8.2 清理残留数据（可选）

卸载程序不删除配置与日志。如需彻底清理：

```bash
# macOS
rm -rf ~/Library/Application\ Support/com.rhost.app ~/Library/Logs/com.rhost.app

# Linux
rm -rf ~/.local/share/com.rhost.app
```

```powershell
# Windows PowerShell
Remove-Item -Recurse -Force "$env:APPDATA\com.rhost.app", "$env:LOCALAPPDATA\com.rhost.app"
```

钥匙串中的凭据需手动删除：macOS 在「钥匙串访问」搜索 `rhost` 删除；Windows 在「凭据管理器 → Windows 凭据」删除 `rhost` 开头条目。

## 9. 下一步

- [快速开始](../getting-started/index.md)：装好后连接第一台主机；
- [架构与通信协议](../explanation/architecture.md)：理解权限与数据流设计；
- [开发指南](./development.md)：从源码构建。
