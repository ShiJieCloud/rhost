::: v-pre

# CI 三平台打包与发布设计方案

> description: GitHub Actions 用 tauri-action 产出 macOS/Windows/Linux 安装包、softprops/action-gh-release 发布 Release 草稿的流水线设计
>
> created: 2026-10-08 09:28:16
>
> updated: 2026-10-09 09:23:07
>
> author: [sjzhao](https://github.com/ShiJieCloud/rhost)

## 1. 设计目标与硬约束

> scope: 本文覆盖 GitHub Actions 构建流水线——三平台安装包产物、tag 触发的 Release 发布、手动触发产物下载。
> 边界：不覆盖代码签名与公证（macOS Developer ID / Windows 证书）、自动更新（tauri-updater）、e2e 测试入 CI、PR 触发的快速检查 job（后续可选）。
> 关联代码：`.github/workflows/release.yml`（新建）、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`、`frontend/package.json`、`.github/workflows/docs.yml`（写法参考）

| 目标 | 量化口径 |
|---|---|
| 三平台产物 | 每次构建产出：macOS `.app`/`.dmg`、Windows `.msi`/`.exe`(NSIS)、Linux `.deb`/`.rpm`/`.AppImage` |
| 发布操作 | 打 tag `v*` 即自动生成 Release 草稿，产物已挂载、changelog 已自动生成，人工核对后发布 |
| 构建可复现 | `Cargo.lock` 与 `frontend/package-lock.json` 均已入库，CI 用 `npm ci` / cargo 锁定解析 |
| 缓存生效 | Rust 编译缓存（Swatinem/rust-cache）+ npm 缓存（setup-node），二次构建显著提速 |
| 最小权限 | workflow 仅申请 `contents: write`（创建 Release 所需），无额外 secrets 依赖 |

必须遵守的既有项目约束：

- 前端包管理器为 **npm**（`frontend/package-lock.json` 为准；根目录 `pnpm-workspace.yaml` 仅是 `allowBuilds` 残留，忽略）；
- `tauri.conf.json` 的 `bundle.targets: "all"`、`version: "1.0.0"`；根 `package.json` 是 docs 站点专用，与构建无关；
- 窗口由 `lib.rs` 代码创建（`WebviewWindowBuilder`），CI 无头环境可正常构建；
- 当前为 ad-hoc 签名（无签名 identity），CI 产物同样未签名——见 §7 与非目标。

## 2. 必须遵守的既有项目约束

- 仓库布局为 Tauri 标准双目录：`frontend/`（Node 侧，含 `@tauri-apps/cli`）与 `src-tauri/`（Rust 侧）；`tauri.conf.json` 的 `beforeBuildCommand` 以 `cwd: ../frontend` 执行 `npm run build`（含 `vue-tsc -b` 类型检查），因此 CI 必须先在 `frontend/` 装依赖；
- Node 版本基线 22（docs.yml 已用 22，保持一致）；
- workflow 文件内注释风格与 docs.yml 一致（中文注释说明意图）。

## 3. 现状盘点与差距

| 位置 | 现状 | 差距 |
|---|---|---|
| `.github/workflows/` | 仅 `docs.yml`（VitePress 部署 Pages） | 无任何构建/打包流水线 |
| `README.md` | 声称「.github/workflows/ CI 自动打包脚本」 | 与事实不符，流水线落地后成立 |
| 版本号 | `tauri.conf.json` / `Cargo.toml` / `frontend/package.json` 均为 `0.1.0` | 可直接打首个 tag |
| 产物分发 | 无 | 无法向用户提供安装包 |

结论：本设计补齐「三平台打包、tag 发布、产物分发」三块，版本号无需改动。

## 4. 目标与非目标

### 目标

1. 新建 `.github/workflows/release.yml`：三平台 matrix 构建，产物上传；
2. `push tag v*` 触发：自动创建 **Release 草稿**（draft），挂载全部平台产物，自动生成 changelog，维护者核对后直接发布；
3. `workflow_dispatch` 手动触发：仅构建并上传 workflow artifact，用于发版前验证，不产生 Release。

### 非目标

- 不做 macOS 签名/公证（无 Apple 开发者账号时产物下载后需手动放行 Gatekeeper，见 §7）；
- 不做 Windows Authenticode 签名；
- 不接入 tauri-updater 自动更新（updater 需要 `includeUpdaterJson` 与签名密钥，届时另立设计）；
- 不在 CI 跑 e2e（需 docker sshd 服务，另行评估）；不新增 PR 快速检查 job（可后续追加到本文件）；
- 不做 macOS x86_64 目标（matrix 仅 `macos-latest` = arm64；Intel Mac 支持列入未决问题）。

## 5. 总体架构

```text
push tag v* ─┐
             ├─▶ release.yml ─┐
dispatch ────┘                 │
                               ├─ build job（matrix 三跑，timeout 60min，concurrency: release-<ref>）
                               │     ├─ checkout
                               │     ├─ rust stable + rust-cache(src-tauri)
                               │     ├─ node 22 + npm cache(frontend lock)
                               │     ├─ [linux] apt 装 webkit/gtk/appindicator/rsvg/patchelf
                               │     ├─ npm ci（frontend/）
                               │     ├─ tauri-action：仅构建（tagName='' 跳过发布，beforeBuildCommand 先跑前端构建）
                               │     └─ upload-artifact：bundle 目录 → bundles-<os>
                               │
                               └─ release job（仅 tag 触发，needs build 全绿，timeout 10min）
                                     ├─ download-artifact（pattern: bundles-*）
                                     └─ softprops/action-gh-release：创建草稿 + 挂载分发产物（不含 .app）
```

## 6. 详细设计

### 6.1 workflow 全文（`.github/workflows/release.yml`）

> 设计取舍：build 与 release 拆成两段。build matrix 三平台并行构建并上传 workflow artifact；release job 等 build 全绿后才创建 Release 草稿并挂载全部产物——避免单平台失败时草稿只挂部分产物被误判为"全部就绪"。tauri-action 仅用于构建（`tagName: ''` 跳过其内置发布步骤），发布交给 `softprops/action-gh-release@v2` 统一控制。

```yaml
# 三平台打包与发布：tag v* → Release 草稿；手动触发 → 仅产物（无 Release）
# 产物未签名（ad-hoc）：macOS 从浏览器下载的 dmg 装好后若提示「"rhost" 已损坏，无法打开」，运行 xattr -dr com.apple.quarantine 移除隔离属性后再打开（右键打开对此错误无效）；Windows/Linux 有 SmartScreen/未知来源提示
# Release body 由 release-body-prefix.md（安装须知）+ conventional-changelog 最新段拼接，关闭 generate_release_notes（ci-release-design §10 阶段2）
name: Build & Release

on:
  push:
    tags: ['v*']
  workflow_dispatch:

# 创建 Release 草稿所需的最小权限
permissions:
  contents: write

# 同一 tag/dispatch 不并发；dispatch 排队不取消，避免产物半成品
concurrency:
  group: release-${{ github.ref }}
  cancel-in-progress: false

jobs:
  build:
    strategy:
      fail-fast: false
      matrix:
        os: [macos-latest, ubuntu-22.04, windows-latest]
    runs-on: ${{ matrix.os }}
    timeout-minutes: 60  # 首次无缓存构建可能 30-60min，卡死时及时止损
    steps:
      - name: Checkout
        # v5 是最小 node24 major：v6 把 credentials 移出 .git/config，v7 默认阻止 fork-PR checkout，不越级
        uses: actions/checkout@v5

      - name: Setup Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Rust cache
        uses: Swatinem/rust-cache@v2
        with:
          workspaces: src-tauri

      - name: Setup Node
        # v5 默认启用 package-manager cache，已用 cache: npm 显式指定不受影响
        uses: actions/setup-node@v5
        with:
          node-version: 22
          cache: npm
          cache-dependency-path: frontend/package-lock.json

      # Tauri v2 Linux 构建的系统依赖（webkit2gtk-4.1 为 Tauri 2 基线）
      - name: Install Linux system deps
        if: runner.os == 'Linux'
        run: |
          sudo apt-get update
          sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev \
            libayatana-appindicator3-dev librsvg2-dev patchelf

      # beforeBuildCommand 会在 frontend/ 里跑 npm run build（含 vue-tsc 类型检查）
      - name: Install frontend deps
        working-directory: frontend
        run: npm ci

      # 仅构建：tagName 传空串跳过 tauri-action 内置发布步骤；发布交由下游 release job
      - name: Build
        uses: tauri-apps/tauri-action@v0
        with:
          projectPath: src-tauri
          tagName: ''
          includeUpdaterJson: false

      # build 产物上传 workflow artifact；tag 触发时由 release job 下载后统一挂载到 Release
      # upload-artifact 必须用 v6：v5 仍声明 node20，会被强制跑 Node 24 并报警告
      - name: Upload bundle artifacts
        uses: actions/upload-artifact@v6
        with:
          name: bundles-${{ matrix.os }}
          path: |
            src-tauri/target/release/bundle/dmg/*.dmg
            src-tauri/target/release/bundle/macos/*.app
            src-tauri/target/release/bundle/msi/*.msi
            src-tauri/target/release/bundle/nsis/*.exe
            src-tauri/target/release/bundle/deb/*.deb
            src-tauri/target/release/bundle/rpm/*.rpm
            src-tauri/target/release/bundle/appimage/*.AppImage
          if-no-files-found: warn

  # 仅 tag 触发执行：等三平台 build 全绿后下载全部 artifact，
  # 用 conventional-changelog 最新段 + 安装须知前缀拼接 Release body，创建草稿
  release:
    needs: build
    if: startsWith(github.ref, 'refs/tags/')
    runs-on: ubuntu-latest
    timeout-minutes: 10
    steps:
      # 需完整历史供 conventional-changelog 解析 commit
      - name: Checkout
        uses: actions/checkout@v5
        with:
          fetch-depth: 0

      # 为 npx conventional-changelog-cli 提供运行时；version 取自 frontend/package.json
      - name: Setup Node
        uses: actions/setup-node@v5
        with:
          node-version: 22
          cache: npm
          cache-dependency-path: frontend/package-lock.json

      # 下载所有平台 artifact，按 artifact 名解压到 <name>/ 子目录
      # download-artifact 必须用 v7：与 upload-artifact v6 配对，v5 改变了单 artifact 下载路径
      - name: Download all bundle artifacts
        uses: actions/download-artifact@v7
        with:
          path: bundles
          pattern: bundles-*

      # 拼接 Release body：安装须知前缀 + 分隔线 + CHANGELOG 最新段
      # --pkg frontend/package.json 让版本号取 1.0.0；--tag-prefix v 按 v* tag 切段
      - name: Build release body
        run: |
          npx --yes conventional-changelog-cli@latest -p angular \
            --pkg frontend/package.json --tag-prefix 'v' --release-count 1 \
            -o changelog-latest.md
          cat .github/release-body-prefix.md > release-body.md
          printf '\n---\n\n' >> release-body.md
          cat changelog-latest.md >> release-body.md
          echo '--- release-body.md ---'
          cat release-body.md

      # 统一创建 Release 草稿，glob 挂载分发产物（.app 目录不挂载，用户应下 dmg）
      # softprops v2 优先 body_path；不再使用 generate_release_notes 与 body
      - name: Create release draft
        uses: softprops/action-gh-release@v2
        with:
          name: Rhost ${{ github.ref_name }}
          draft: true
          prerelease: false
          body_path: release-body.md
          files: |
            bundles/**/*.dmg
            bundles/**/*.msi
            bundles/**/*.exe
            bundles/**/*.deb
            bundles/**/*.rpm
            bundles/**/*.AppImage
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

### 6.2 关键取值与理由

| 取值 | 理由 |
|---|---|
| `ubuntu-22.04` | Tauri 2 文档基线，`libwebkit2gtk-4.1` 包名稳定；`ubuntu-latest`（24.04）亦可用但不作为锚点 |
| `fail-fast: false` | 单平台失败不吞掉其余平台的产物，便于定位平台相关问题 |
| `includeUpdaterJson: false` | 未接入 tauri-updater，不生成 `latest.json` |
| `projectPath: src-tauri` | action 以该目录定位 `tauri.conf.json`；`beforeBuildCommand` 的 `cwd: ../frontend` 相对解析不变 |
| build → release 两段式 | build 仅构建+上传 artifact；release `needs: build` 全绿后才创建草稿，避免单平台失败导致草稿只挂部分产物被误判为"全部就绪" |
| build `tagName: ''` | 空串让 tauri-action 跳过内置发布步骤，发布权交给 release job 的 `softprops/action-gh-release@v2`，单一职责 |
| `softprops/action-gh-release@v2` | 比 tauri-action 内置发布更可控；`files` 用 glob 只挂分发产物（dmg/msi/exe/deb/rpm/AppImage），排除 `.app` 目录；`body_path` 指向拼接好的 release-body.md |
| `body_path` + conventional-changelog | Release body 由 `.github/release-body-prefix.md`（安装须知）+ conventional-changelog 最新段拼接，单一来源；关闭 `generate_release_notes` 避免与 CHANGELOG.md 重复（详见 §10） |
| release job `fetch-depth: 0` | 需完整 git 历史供 conventional-changelog 解析 commit message |
| action 版本（checkout@v5 / setup-node@v5 / upload@v6 / download@v7） | v5 为最小 node24 major；upload v6/download v7 配对，v5 改变了单 artifact 下载路径 |
| `concurrency: release-${{ github.ref }}` | 同一 tag/dispatch 不并发；dispatch 排队不取消，避免产物半成品；与 docs.yml 的 concurrency 风格一致 |
| build `timeout-minutes: 60` | 首次无缓存构建可能 30-60min，卡死时及时止损，避免长时间占用 runner |
| release `timeout-minutes: 10` | 仅下载+创建草稿，10min 足够 |
| `if-no-files-found: warn` | targets 为 `all`，个别平台某格式可能未产出，警告即可不阻断 |

### 6.3 产物与版本

- 版本号来源于 `tauri.conf.json` 的 `version`（当前 `1.0.0`），文件名形如 `rhost_1.0.0_aarch64.dmg`、`rhost_1.0.0_x64-setup.exe`、`rhost_1.0.0_amd64.deb`；
- 发版流程：三处 version 对齐 → 提交 → `git tag v1.0.0 && git push origin v1.0.0` → Actions 生成草稿 → 人工核对发布；
- tag 与 `tauri.conf.json` version 不一致时以 conf 为准（产物名按 conf 版本），发布前人工核对。

### 6.4 产物命名规范

Tauri 2 默认产物名由 `productName` + `version` + `arch` 派生，可定制空间有限。本节约束项目层面的取舍，使产物名稳定、可预期、可被脚本解析。

**命名来源与硬约束：**

| 字段 | 来源 | 取值 | 备注 |
|---|---|---|---|
| 产品名段 | `tauri.conf.json` → `productName` | `rhost`（全小写） | 已固化，禁止改为 `Rhost`/`RHOST`，否则所有产物名连带变更 |
| 版本段 | `tauri.conf.json` → `version` | `1.0.0`（语义化版本） | 禁止从 tag 字符串解析版本号；tag 必须与 conf 对齐（见 §6.3） |
| 架构段 | Tauri 按 runner 自动注入 | `aarch64` / `x64` / `amd64` / `x86_64` | 各平台格式不同，见下表 |
| OS 段 | 不注入 | — | dmg/msi/exe/deb/rpm/AppImage 后缀已天然区分 OS，无需再追加 OS 段 |

**各平台产物命名（Tauri 默认，不可重命名）：**

| 平台 | 格式 | 示例（v1.0.0, arm64/x64） | Release 是否挂载 |
|---|---|---|---|
| macOS | DMG | `rhost_1.0.0_aarch64.dmg` / `rhost_1.0.0_x64.dmg` | ✅ |
| macOS | App bundle | `rhost.app`（目录，无版本/架构） | ❌（仅本地构建产物，release job 显式排除） |
| Windows | NSIS | `rhost_1.0.0_x64-setup.exe` | ✅ |
| Windows | MSI | `rhost_1.0.0_x64_en-US.msi` | ✅ |
| Linux | Deb | `rhost_1.0.0_amd64.deb` | ✅ |
| Linux | Rpm | `rhost_1.0.0.x86_64.rpm` | ✅ |
| Linux | AppImage | `rhost_1.0.0_x64.AppImage` | ✅ |

> 注：rpm 用 `.` 分隔版本与架构、deb 用 `amd64` 表示 x64、MSI 含语言段 `en-US`——均为 Tauri/bundler 默认行为，**不得手动重命名**。

**Release 资产展示规则：**

- Release 资产名保持 Tauri 默认输出，**禁止在 workflow 中重命名**；
- Release 标题：`Rhost v1.0.0`（与 §6.1 release job 的 `name` 字段一致）；
- Release body：由 `.github/release-body-prefix.md`（安装须知前缀）+ conventional-changelog 最新段拼接为 `release-body.md`，经 `body_path` 注入；维护者核对后直接发布，无需手写说明；
- 预发布（pre-release）用 `v1.0.0-rc.1` 形式，conf `version` 同步为 `1.0.0-rc.1`，产物文件名随之变化。

**命名规范的非目标：**

- 不为产物加 OS 段（`macos`/`windows`/`linux`），后缀已足够区分；
- 不引入构建时间戳、commit hash、构建号——版本号已唯一标识一次发版；
- 不做 universal2 / x86_64 macOS 产物（见 §9 未决问题）。

## 7. 安全与降级

- 权限最小化：workflow 级 `contents: write` 是创建 Release 的必要权限；未申请 `packages`/`id-token` 等；
- 无自定义 secrets：仅用内置 `GITHUB_TOKEN`，无泄漏面；签名体系接入前不引入任何密钥；
- 产物未签名的用户侧影响（写入 Release 说明模板）：macOS 从浏览器下载的 `.dmg` 装好后若提示「"rhost" 已损坏，无法打开」，运行 `xattr -dr com.apple.quarantine /Applications/rhost.app` 移除隔离属性后再打开（ad-hoc 签名 + quarantine 触发 Gatekeeper 标记"已损坏"，右键 → 打开对此错误无效）；Windows SmartScreen 选「仍要运行」；签名/公证为后续独立事项；
- 构建失败：`fail-fast: false` 保证其余平台 build 产物仍上传 artifact；release job `needs: build` 要求三平台全绿才执行，单平台失败时**不会创建 Release 草稿**，无半成品草稿风险；单平台失败先查该 runner 的系统依赖与 Rust 缓存，必要时 `rust-cache` 加 `cache-all-crates` 或清理重跑；
- 超时降级：build `timeout-minutes: 60`、release `timeout-minutes: 10`，卡死时自动取消不堆积 runner。

## 8. 测试与验证

- 语法与触发验证：workflow YAML 本地可用 `actionlint`（若有）或推送后看 Actions 解析结果；
- 手动触发一轮：`workflow_dispatch` → 三平台 build job 全绿 → 下载 `bundles-*` artifact，核对三平台产物齐全（dispatch 不触发 release job，无 Release 草稿）；
- tag 验证：打 `v1.0.0` → build 全绿 → release job 创建草稿、挂载 6-7 个分发产物（不含 `.app`）、body 含安装须知 + CHANGELOG 最新段 → 维护者核对后直接发布；
- 冒烟：下载产物在本机安装启动（macOS arm64 可真机验证；Windows/Linux 产物至少确认文件完整可解包）。

## 9. 未决问题

- macOS x86_64：是否为 Intel Mac 增加 `macos-13` matrix 条目或出 universal2 包（构建时长翻倍），待有 Intel 用户诉求后决策；
- macOS 签名/公证、Windows Authenticode：需要开发者账号与证书 secrets，发布对外正式版前评估；
- e2e 测试入 CI（ubuntu runner 起 docker/debian-sshd 容器跑 `cargo test --test ssh_e2e`）与 PR 快速检查 job（`cargo check` + `vue-tsc`）——待构建流水线稳定后追加；
- tauri-updater 自动更新通道（`autoUpdate`/`updateChannel` 设置项已预留但后端未实现）——另立设计；

## 10. CHANGELOG.md 引入方案

> status: 已落地。`CHANGELOG.md` 入库，`release.yml` release job 用 conventional-changelog 最新段 + `.github/release-body-prefix.md` 拼接 `body_path`，`generate_release_notes` 已关闭；§9 对应未决项已移除。

### 10.1 目标与非目标

目标：

- 提供入库的、可 diff、可离线检索的版本变更记录，不依赖 GitHub 平台；
- Release body 从 `CHANGELOG.md` 最新段提取，单一来源，关闭 `generate_release_notes`。

非目标：

- 不自动 commit `CHANGELOG.md` 回 main（避免 bot token 与分支保护冲突，由开发者发版前手工提交）；
- 不引入 release-please / changesets（与 Tauri 双目录 + npm/cargo 混合构建不匹配，过重）。

### 10.2 格式与位置

- 格式：[Keep a Changelog 1.1.0](https://keepachangelog.com/zh-CN/1.1.0/) + SemVer 2.0.0；
- 位置：仓库根 `CHANGELOG.md`（社区惯例，用户克隆即可见）；
- 段落结构：`## [VERSION] - YYYY-MM-DD` 下分 `### Added` / `### Changed` / `### Deprecated` / `### Removed` / `### Fixed` / `### Security`。

### 10.3 前置依赖：conventional commits

CHANGELOG 自动生成依赖 conventional commits 规范（`feat`/`fix`/`docs`/`refactor`/`chore`/`perf`/`test` 前缀）。项目当前未强制，落地前需：

- 在根 `package.json` 加 `commitlint` + `husky` 钩子（commit-msg 检查）；
- README 补充 commit 规范说明；
- 历史 commit 不回溯改写，仅约束新 commit。

### 10.4 工具与调用

- 工具：`conventional-changelog-cli`（npm 包）；
- 写回 `CHANGELOG.md`（prepend 最新段）：`npx conventional-changelog -p angular -i CHANGELOG.md -s`；
- 仅生成最新段到文件：`npx conventional-changelog -p angular --release-count 1 -o changelog-latest.md`；
- 不加 `devDependency`，不污染任何 `package.json`（根 `package.json` 是 docs 站点专用，`frontend/` 是前端专用，均不承载项目级工具）。

### 10.5 落地阶段

**阶段 1：手工生成（最小引入）**

- 开发者发版前本地跑 `npx conventional-changelog -p angular -i CHANGELOG.md -s`；
- 提交 `CHANGELOG.md` → 打 tag → 推送；
- `release.yml` 的 release job **不变**，仍用 `generate_release_notes: true`（CHANGELOG.md 仅入库，不进 Release body）；
- 优势：零 CI 改动，先建立 commit 规范与历史档案。

**阶段 2：CI 提取塞进 Release body**

新增仓库文件 `.github/release-body-prefix.md`，内容为「安装须知」前缀（避免 shell 转义反引号）：

```markdown
## 安装须知（产物未签名）
- **macOS**：从浏览器下载的 `.dmg` 装好后首次打开若提示「"rhost" 已损坏，无法打开」，运行 `xattr -dr com.apple.quarantine /Applications/rhost.app` 移除隔离属性后再打开（ad-hoc 签名 + quarantine 触发 Gatekeeper 标记"已损坏"，右键 → 打开对此错误无效）
- **Windows**：SmartScreen 选「仍要运行」
- **Linux**：AppImage 需 `chmod +x` 后运行；deb/rpm 用对应包管理器安装

签名/公证为后续独立事项，详见设计文档 §7。
```

release job 的 `Create release draft` step 前加一步构建 body，并改用 `body_path`：

```yaml
      - name: Build release body
        run: |
          npx conventional-changelog -p angular --release-count 1 -o changelog-latest.md
          cat .github/release-body-prefix.md > release-body.md
          printf '\n---\n\n' >> release-body.md
          cat changelog-latest.md >> release-body.md

      - name: Create release draft
        uses: softprops/action-gh-release@v2
        with:
          name: Rhost ${{ github.ref_name }}
          draft: true
          prerelease: false
          body_path: release-body.md
          files: |
            bundles/**/*.dmg
            bundles/**/*.msi
            bundles/**/*.exe
            bundles/**/*.deb
            bundles/**/*.rpm
            bundles/**/*.AppImage
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

- 关闭 `generate_release_notes`（删除该字段）；
- `CHANGELOG.md` 仍由开发者发版前手工提交（CI 不自动 commit 回 main）。

### 10.6 落地检查清单

- [x] 根 `package.json` 加 `commitlint` + `husky`；
- [x] README 补 commit 规范说明；
- [x] 首次 `npx conventional-changelog -p angular -i CHANGELOG.md -s` 生成历史档案并提交；
- [x] 新增 `.github/release-body-prefix.md`；
- [x] `release.yml` release job 加 §10.5 阶段 2 步骤，改用 `body_path`；
- [x] 删除 `generate_release_notes: true` 与原 `body` 字段；
- [x] 验证：打测试 tag → 草稿 body 含「安装须知」+ CHANGELOG 最新段。

### 10.7 风险与回退

- conventional-changelog 解析失败（commit 不规范）→ 生成空段，Release body 仅含「安装须知」前缀；回退：临时恢复 `generate_release_notes: true`；
- 阶段 2 上线后若 changelog 质量差，可一键回滚 `release.yml` 到阶段 1 状态（git revert）；
- `body_path` 与 `body` 不可同时生效（softprops v2 优先 `body_path`），切换时务必删旧 `body`。

:::
