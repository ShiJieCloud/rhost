## 安装须知（产物未签名）
- **macOS**：从浏览器下载的 `.dmg` 装好后首次打开若提示「"rhost" 已损坏，无法打开」，运行 `xattr -dr com.apple.quarantine /Applications/rhost.app` 移除隔离属性后再打开（ad-hoc 签名 + quarantine 触发 Gatekeeper 标记"已损坏"，右键 → 打开对此错误无效）
- **Windows**：SmartScreen 选「仍要运行」
- **Linux**：AppImage 需 `chmod +x` 后运行；deb/rpm 用对应包管理器安装

签名/公证为后续独立事项，详见设计文档 §7。
