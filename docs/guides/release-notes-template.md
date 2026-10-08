::: v-pre

# 发版 Release notes 模板

> description: 发版时编写 GitHub Release body 的完整模板与字段说明，含 rhost 产物下载表、SHA256 校验、未签名提示与流水线自动段说明
> 创建时间：2026-10-08 10:58:02
> 更新时间：2026-10-08 16:10:24
> 作者：

---

## 1. 何时用本模板

每次打 tag `v*` 触发流水线生成 Release 草稿后，维护者在 GitHub Releases 页面核对/编辑 body 时参考本模板。提供两套模板：

- **§2 完整模板**：重要发版（如 `v1.0.0`、首个公开版、含破坏性变更）由维护者手写，亮点 / 下载表 / 校验 / 破坏性变更俱全；
- **§4 过渡模板**：日常发版由流水线自动生成（`generate_release_notes: true`），维护者零手写，落地 [ci-release-design.md §10](../explanation/design/ci-release-design.md) 阶段 2 后切换为 CHANGELOG.md 提取。

## 2. 完整 Release body 模板（手写场景）

复制到 GitHub Release 草稿，把 `{{...}}` 占位符替换为实际值：

````markdown
# Rhost v{{VERSION}}

- **发布日期**：{{YYYY-MM-DD}}
- **版本类型**：稳定版 / 预发布（rc）
- **对比范围**：v{{PREV_VERSION}}...v{{VERSION}}
- **完整变更日志**：https://github.com/{{ORG}}/rhost/compare/v{{PREV_VERSION}}...v{{VERSION}}

> {{一句话总结本版本，例如：新增隧道面板实时流量统计，修复断线重连端口泄漏。}}

## ✨ 亮点

- {{亮点 1，例如：隧道面板支持实时流量统计与连接数展示}}
- {{亮点 2}}

## 🚀 新功能

- feat(scope): {{功能描述}}（#{{PR}}，@{{贡献者}}）

## 🐛 修复

- fix(scope): {{问题描述}}（#{{PR}}，@{{贡献者}}）

## 🔧 优化与重构

- perf/refactor(scope): {{变更描述}}（#{{PR}}，@{{贡献者}}）

## ⚠️ 破坏性变更

> 没有破坏性变更请写"无"。

- **变更**：{{描述破坏性变更}}
  - 旧用法：`{{旧配置键 / IPC 命令 / 帧字段}}`
  - 新用法：`{{新配置键 / IPC 命令 / 帧字段}}`
  - 迁移指南：{{链接或步骤}}

## 📦 下载与安装

> **产物未签名**：macOS 从浏览器下载的 `.dmg` 装好后若提示「"rhost" 已损坏，无法打开」，运行 `xattr -dr com.apple.quarantine /Applications/rhost.app` 移除隔离属性后再打开（右键 → 打开对此错误无效）；Windows SmartScreen 选「仍要运行」；Linux AppImage 需 `chmod +x` 后运行，deb/rpm 用对应包管理器安装。签名/公证为后续独立事项，详见设计文档 §7。

从下方 **Assets** 区域下载对应平台安装包（SHA256 落地后从 Assets 的 `checksums.txt` 比对）：

| 平台 | 文件格式 | SHA256 |
| --- | --- | --- |
| macOS (Apple Silicon) | `.dmg` | {{SHA256}} |
| macOS (Intel) | `.dmg` | 暂未提供（见设计文档 §9） |
| Windows (x86_64) | `.msi` / `-setup.exe` | {{SHA256}} / {{SHA256}} |
| Linux (x86_64) | `.deb` / `.rpm` / `.AppImage` | {{SHA256}} / {{SHA256}} / {{SHA256}} |

rhost 为 GUI 桌面应用，不提供 `curl | sh` 一键安装；目前仅通过 GitHub Release 分发，暂未接入 Homebrew / Scoop / cargo-binstall 等包管理器。

## 🔐 制品校验

> 当前流水线未生成 `checksums.txt`，规划中（见 §7 未决问题）；落地后从 Assets 下载 `checksums.txt`，以上方下载表的 SHA256 比对。

校验方式：

```bash
# Linux / macOS
sha256sum -c checksums.txt

# macOS（备选）
shasum -a 256 -c checksums.txt

# Windows PowerShell
Get-FileHash {{FILE_NAME}} -Algorithm SHA256
```

> rhost 产物当前为 ad-hoc 未签名，Sigstore / cosign 验证未接入，见设计文档 §7。

## 🙏 贡献者

感谢本次发布的所有贡献者：

@{{USER1}}、@{{USER2}}、@{{USER3}}

> 自动场景由 `generate_release_notes` 的「New Contributors」段覆盖，无需手写。

## 📋 已知问题

- {{已知问题 1，及规避方案}}
- {{已知问题 2，及规避方案}}

## 🔗 相关链接

- 文档：https://{{ORG}}.github.io/rhost/
- 讨论区：https://github.com/{{ORG}}/rhost/discussions
- 问题反馈：https://github.com/{{ORG}}/rhost/issues
````

## 3. 段落来源说明

| 段落 | 手写场景（§2） | 自动场景（§4 / §10 阶段 2） |
|---|---|---|
| 顶部元信息（发布日期、版本类型、对比范围、完整变更日志链接） | 维护者手填 | 流水线不生成，需手填或留空 |
| 一句话总结 / 亮点 | 维护者手写 | 无 |
| 新功能 / 修复 / 优化 | 维护者按 PR 标题归类 | `generate_release_notes` 自动列 PR，不分类 |
| 破坏性变更 | 维护者手写 | 无（自动模式不识别破坏性） |
| 下载表（含 SHA256 列） | 维护者按 §6.4 命名规范填平台/格式 + 手填 SHA256 | 无（产物自动挂 Assets，body 不含表；SHA256 需流水线增强，见 §7） |
| 制品校验命令 | 模板固定 | 模板固定（自动模式不含校验段） |
| 贡献者 | 维护者手写 | `generate_release_notes` 的「New Contributors」段自动覆盖 |
| 已知问题 | 维护者手写 | 无 |
| 相关链接 | 维护者手填 | 无 |
| 产物未签名提示 | 下载段 blockquote 保留 | workflow `body` 写入前缀 |

## 4. 过渡模板（流水线自动生成场景）

当前 `release.yml` 用 `generate_release_notes: true` 自动生成，body 结构如下（见 [ci-release-design.md §6.1](../explanation/design/ci-release-design.md)）：

```markdown
## 安装须知（产物未签名）
- **macOS**：从浏览器下载的 `.dmg` 装好后首次打开若提示「"rhost" 已损坏，无法打开」，运行 `xattr -dr com.apple.quarantine /Applications/rhost.app` 移除隔离属性后再打开（ad-hoc 签名 + quarantine 触发 Gatekeeper 标记"已损坏"，右键 → 打开对此错误无效）
- **Windows**：SmartScreen 选「仍要运行」
- **Linux**：AppImage 需 `chmod +x` 后运行；deb/rpm 用对应包管理器安装

签名/公证为后续独立事项，详见设计文档 §7。

---

<!-- 以下由 generate_release_notes: true 自动追加：What's Changed + New Contributors + Full Changelog 链接 -->
```

落地 [ci-release-design.md §10](../explanation/design/ci-release-design.md) 阶段 2 后，自动段从 CHANGELOG.md 最新段提取（带 Added/Changed/Fixed 分类），关闭 `generate_release_notes`。

## 5. 发版流程

1. 三处版本对齐：`src-tauri/tauri.conf.json` 的 `version`、`src-tauri/Cargo.toml` 的 `version`、`frontend/package.json` 的 `version`；
2. 提交改动并推送；
3. 打 tag：`git tag v0.1.0 && git push origin v0.1.0`；
4. 等 Actions 跑完（build + release 两 job，约 30-60 分钟，首次无缓存偏长）；
5. 去 GitHub Releases 页面查看草稿：产物已挂载、body 已含「安装须知」+ 自动 changelog；
6. 重要发版：按 §2 完整模板重写 body；日常发版：仅核对自动段；
7. 点 **Publish release** 发布。

## 6. 与流水线的关系

本模板对应 `.github/workflows/release.yml`（设计见 [ci-release-design.md](../explanation/design/ci-release-design.md)）的 release job 输出：

- build job 三平台并行构建，上传 workflow artifact；
- release job 等 build 全绿后下载 artifact，用 `softprops/action-gh-release@v2` 创建草稿；
- body 前缀「安装须知」由 workflow 写入，changelog 由 `generate_release_notes: true` 调 GitHub API 自动追加；
- 产物文件名遵循 [ci-release-design.md §6.4](../explanation/design/ci-release-design.md) 命名规范（`rhost_{{VERSION}}_{{arch}}.{{ext}}`），Release 资产名保持 Tauri 默认，禁止重命名。

## 7. 未决问题

- `CHANGELOG.md` 引入方案已规划，见 [ci-release-design.md §10](../explanation/design/ci-release-design.md)；落地前 Release body 仍用 `generate_release_notes: true` 自动生成，本模板 §4 作为过渡；
- `checksums.txt` 生成与挂载：当前流水线未产出，需 release job 加 `sha256sum` 步骤生成并挂 Assets；落地后本模板 §2 的「制品校验」段方可使用；
- Sigstore / cosign 制品签名：与 macOS/Windows 代码签名同属"未签名"现状，见 [ci-release-design.md §9](../explanation/design/ci-release-design.md) 签名/公证未决项；
- macOS Intel (x86_64) 产物未提供，见 [ci-release-design.md §9](../explanation/design/ci-release-design.md)；
- Homebrew / Scoop / cargo-binstall 等包管理器分发渠道未接入，待用户诉求后评估。

:::
