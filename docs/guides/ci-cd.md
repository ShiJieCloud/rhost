# CI/CD 流水线

> description: Rhost 的 GitHub Actions 流水线说明，包括文档部署与三平台构建发布
>
> created: 2026-10-09 20:37:02
>
> updated: 2026-10-09 20:37:02
>
> author: [sjzhao](https://github.com/ShiJieCloud/rhost)

## 1. 概览

Rhost 使用 GitHub Actions 实现 CI/CD，共有两条工作流：

| 工作流 | 文件 | 触发方式 | 用途 |
|---|---|---|---|
| 文档部署 | `.github/workflows/docs.yml` | push 到 main（docs 相关路径）或手动 | 构建 VitePress 并部署到 GitHub Pages |
| 构建发布 | `.github/workflows/release.yml` | push tag `v*` 或手动 | 三平台打包 + 创建 Release 草稿 |

流水线设计详见 [CI 三平台打包与发布设计方案](../explanation/design/ci-release-design.md)。

## 2. 文档部署（docs.yml）

### 2.1 触发条件

- 推送到 `main` 分支且以下路径有变更时自动触发：
  - `docs/**`
  - `package.json`
  - `package-lock.json`
  - `.github/workflows/docs.yml`
- 支持 `workflow_dispatch` 手动触发

### 2.2 流程

1. **检出代码**：`actions/checkout@v5`，`fetch-depth: 0`（`lastUpdated` 依赖提交历史）
2. **设置环境**：Node 22 + npm 缓存
3. **安装依赖**：`npm ci`
4. **配置 Pages**：`actions/configure-pages@v5`，自动确定 `base_path`
5. **构建站点**：`npm run docs:build`，写入 `.nojekyll`
6. **上传产物**：`actions/upload-pages-artifact@v3`
7. **部署**：`actions/deploy-pages@v4`

### 2.3 并发控制

```yaml
concurrency:
  group: pages
  cancel-in-progress: false
```

同一时刻只允许一次部署，排队不取消，避免发布中断。

## 3. 构建发布（release.yml）

### 3.1 触发条件

- 推送 `v*` tag 时自动触发（创建 Release 草稿）
- `workflow_dispatch` 手动触发（仅构建，不创建 Release）

### 3.2 总体结构

```
release.yml
├── build job（三平台并行）
│   ├── macos-latest
│   ├── ubuntu-22.04
│   └── windows-latest
└── release job（仅 tag 触发，needs: build）
    └── 创建 Release 草稿
```

### 3.3 build job（三平台并行构建）

**矩阵**：`[macos-latest, ubuntu-22.04, windows-latest]`，`fail-fast: false`，`timeout-minutes: 60`

**步骤**：

| 步骤 | 说明 |
|---|---|
| Checkout | `actions/checkout@v5` |
| Setup Rust | `dtolnay/rust-toolchain@stable` |
| Rust cache | `Swatinem/rust-cache@v2`，workspaces: `src-tauri` |
| Setup Node | Node 22 + npm 缓存（`frontend/package-lock.json`） |
| Install Linux deps | 仅 Linux：`libwebkit2gtk-4.1-dev`、`libgtk-3-dev`、`libayatana-appindicator3-dev`、`librsvg2-dev`、`patchelf` |
| Install frontend deps | `frontend/` 目录下 `npm ci` |
| Build | `tauri-apps/tauri-action@v0`，`tagName: ''` 跳过内置发布 |
| Upload artifacts | `actions/upload-artifact@v6`，上传各平台 bundle |

**产物**：

| 平台 | 产物格式 |
|---|---|
| macOS | `.dmg`、`.app` |
| Windows | `.msi`、`.exe`（NSIS） |
| Linux | `.deb`、`.rpm`、`.AppImage` |

`.app` 目录不挂载到 Release，用户应下载 `.dmg`。

### 3.4 release job（创建 Release 草稿）

**触发条件**：仅 `push tag v*` 时执行，`needs: build` 全绿后运行，`timeout-minutes: 10`

**步骤**：

1. **下载产物**：`actions/download-artifact@v7`，按 `bundles-*` pattern 下载所有平台产物
2. **生成 changelog**：`conventional-changelog-cli` 从 Conventional Commits 提取最新段
3. **拼接 Release body**：`.github/release-body-prefix.md`（安装须知） + `---` + changelog
4. **创建草稿**：`softprops/action-gh-release@v2`，挂载分发产物（`.dmg`/`.msi`/`.exe`/`.deb`/`.rpm`/`.AppImage`）

**Release body 结构**：

```markdown
## 安装须知（产物未签名）
（来自 .github/release-body-prefix.md）

---

（来自 conventional-changelog 的最新段，含 Added/Changed/Fixed 分类）
```

### 3.5 并发控制

```yaml
concurrency:
  group: release-${{ github.ref }}
  cancel-in-progress: false
```

同一 tag/dispatch 排队不取消，避免半成品产物。

## 4. 发版操作流程

详细操作步骤见 [触发发版流水线操作指南](release-trigger.md)。

### 4.1 前置检查

发版前确认三处版本号一致：

| 文件 | 字段 |
|---|---|
| `frontend/package.json` | `version` |
| `src-tauri/tauri.conf.json` | `version` |
| `src-tauri/Cargo.toml` | `version` |

### 4.2 正式发版

```bash
git checkout main
git pull --ff-only origin main

# 更新三处版本号并提交
git add frontend/package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml
git commit -m "chore(release): bump version to 1.0.0"
git push origin main

# 打 tag（必须以 v 开头）
git tag v1.0.0
git push origin v1.0.0
```

### 4.3 手动构建验证

适用于升级依赖后验证三平台打包，不创建 Release：

1. 打开 `https://github.com/ShiJieCloud/rhost/actions/workflows/release.yml`
2. 点 `Run workflow` → 选 `main` → 运行

产物在 Actions 运行详情页的 Artifacts 区下载，保留 90 天。

### 4.4 发布 Release

1. 等待 build + release job 全绿
2. 进入 Releases 页面找到草稿 `Rhost v<version>`
3. 核对 body（安装须知 + changelog）与附件
4. 重要发版按 [Release notes 模板](../reference/release-notes-template.md) 手写亮点
5. 点 **Publish release** 发布

## 5. 产物说明

### 5.1 产物格式

| 平台 | 格式 | 说明 |
|---|---|---|
| macOS | `.dmg` | 拖动到 Applications 安装 |
| Windows | `.msi` / `.exe` | MSI 安装包或 NSIS 安装程序 |
| Linux | `.deb` / `.rpm` / `.AppImage` | 包管理器安装或直接运行 |

### 5.2 未签名说明

当前产物均未做代码签名与公证：

- **macOS**：首次打开提示「"rhost" 已损坏，无法打开」，运行 `xattr -dr com.apple.quarantine /Applications/rhost.app` 移除隔离属性
- **Windows**：SmartScreen 弹窗，选「仍要运行」
- **Linux AppImage**：需 `chmod +x` 后运行

签名/公证为后续迭代事项，详见 [ci-release-design.md §7](../explanation/design/ci-release-design.md)。

## 6. 缓存策略

| 缓存 | 工具 | 路径 |
|---|---|---|
| Rust 编译缓存 | `Swatinem/rust-cache@v2` | `src-tauri` workspace |
| npm 依赖缓存 | `actions/setup-node@v5` | `frontend/package-lock.json` |

首次构建 30-60 分钟，二次有缓存应降至 10-20 分钟。

## 7. 相关文档

- [触发发版流水线操作指南](release-trigger.md)：完整发版操作步骤与故障排查
- [发版 Release notes 模板](../reference/release-notes-template.md)：Release body 编写模板
- [CI 三平台打包与发布设计方案](../explanation/design/ci-release-design.md)：流水线设计决策
- [贡献指南](contributing.md)：Commit 规范与 PR 流程
