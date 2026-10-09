# 常见问题

> description: Rhost 使用过程中的常见问题与解答
>
> created: 2026-10-09 20:55:46
>
> updated: 2026-10-09 20:55:46
>
> author: [sjzhao](https://github.com/ShiJieCloud/rhost)

## 1. 安装与启动

### 1.1 macOS 提示「"rhost" 已损坏，无法打开」

当前发布产物未做代码签名与公证，从浏览器下载的 `.dmg` 装好后首次打开会触发 Gatekeeper 隔离。运行以下命令移除隔离属性：

```bash
xattr -dr com.apple.quarantine /Applications/rhost.app
```

> 右键 → 打开对此错误无效，必须执行上述命令。

### 1.2 Windows SmartScreen 拦截

弹窗中选择「仍要运行」即可正常安装/运行。

### 1.3 Linux AppImage 无法运行

```bash
chmod +x 文件名.AppImage
./文件名.AppImage
```

## 2. 连接问题

### 2.1 连接超时或失败

检查项：

- 主机地址和端口是否正确
- 网络是否可达（`ping` 或 `telnet host port`）
- 防火墙是否放行 SSH 端口
- 认证方式与凭据是否匹配

### 2.2 主机密钥指纹不一致

首次连接会记录主机密钥指纹。后续连接若指纹不一致，会弹出红色警告。这通常意味着：

- 服务器重装或更换了 SSH 密钥
- 存在中间人攻击风险

确认是合法变更后，可在提示中更新指纹。机制细节见 [SSH 主机密钥校验](../explanation/design/hostkey-verification-design.md)。

## 3. 终端问题

### 3.1 Vim 中文乱码

见 [Vim 中文编码问题](../guides/troubleshooting/vim-utf8-locale.md)。

### 3.2 终端输出卡顿或内存占用高

Rhost 终端字节流走原生二进制 IPC 帧，不经 JSON 序列化，性能优于 Electron 类工具。若仍卡顿，可在「全局设置」中调整回滚缓冲行数。

## 4. SFTP 问题

### 4.1 大文件传输中断后能否续传

支持断点续传。在「全局设置」中开启 `sftpResume`，传输中断后重新传输同一文件时会自动续传。

## 5. 配置问题

### 5.1 配置导入导出是否包含密码和私钥

**不包含**。出于安全考虑，密码、私钥路径、日志配置均不参与导入导出。详细排除项见 [配置导入导出](../guides/config-import-export.md) §6。

### 5.2 导入配置后需要重启吗

需要。导入后应用会提示「立即重启」，重启后配置生效。

## 6. 开发相关

### 6.1 如何从源码构建

见 [开发指南](../guides/development.md)。

### 6.2 如何贡献代码

见 [贡献指南](../guides/contributing.md)。

## 7. 下一步

- [安装](../guides/install.md)：各平台安装与权限说明
- [快速开始](../getting-started/index.md)：从下载到连接第一台主机
- [故障排查](../guides/troubleshooting/applog-troubleshooting.md)：应用日志排查手册
