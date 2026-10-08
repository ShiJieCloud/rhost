::: v-pre

# 触发发版流水线操作指南

> description: 如何通过推送 v* tag 或手动触发 GitHub Actions release.yml 完成三平台打包、Release 草稿生成与发布的完整操作步骤
> 创建时间：2026-10-08 15:48:47
> 更新时间：2026-10-08 15:48:47
> 作者：

---

## 1. 适用场景

维护者需要在以下任一场景触发 `.github/workflows/release.yml`：

- **正式发版**：打 `v*` tag，自动构建三平台产物 + 创建 Release 草稿；
- **构建验证**：只想验证三平台能否打包，不污染 Releases 页面；
- **重跑失败流水线**：tag 已 push 但 build/release job 失败，需要重跑；
- **预发布**：发 `beta` / `rc` 版本并手动标记 Pre-release。

设计背景与字段语义见 [ci-release-design.md](../explanation/design/ci-release-design.md)；Release body 文案模板见 [release-notes-template.md](release-notes-template.md)。

## 2. 前置检查

执行触发前确认以下条件：

| 检查项 | 期望值 | 命令 / 位置 |
|---|---|---|
| 当前分支 | `main` 且已 push 到远端 | `git status` / `git log origin/main..HEAD` 应为空 |
| `frontend/package.json` 的 `version` | 与将打的 tag 数字一致（如 `1.0.0`） | `grep '"version"' frontend/package.json` |
| `src-tauri/tauri.conf.json` 的 `version` | 与上同 | `grep '"version"' src-tauri/tauri.conf.json` |
| `src-tauri/Cargo.toml` 的 `version` | 与上同 | `grep '^version' src-tauri/Cargo.toml` |
| 本地工作区 | 干净，无未提交改动 | `git status` |
| 远端 tag | 不存在同名 tag | `git ls-remote --tags origin v1.0.0` 应无输出 |

> 三个版本号字段**必须一致**；release.yml 的 changelog 通过 `--pkg frontend/package.json` 取版本号，若与 tag 不符会生成错误 body。

## 3. 场景一：正式发版（tag 触发）

适用：首次公开版（如 v1.0.0）、稳定版迭代（如 v1.1.0）。

### 3.1 操作步骤

```bash
# 1. 切到 main 并拉取最新
git checkout main
git pull --ff-only origin main

# 2. 确认版本号已更新并提交（若尚未更新）
#    编辑 frontend/package.json / src-tauri/tauri.conf.json / src-tauri/Cargo.toml
git add frontend/package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml
git commit -m "chore(release): bump version to 1.0.0"
git push origin main

# 3. 打 tag（必须以 v 开头，否则不匹配 on.push.tags: ['v*']）
git tag v1.0.0

# 4. 推送 tag 触发流水线
git push origin v1.0.0
```

### 3.2 流水线行为

tag 推送后 release.yml 执行两条 job：

1. **build**（macOS / ubuntu-22.04 / windows 三平台并行，最多 60min）
   - Rust 工具链 + `Swatinem/rust-cache` 缓存
   - Node 22 + `npm ci` 装前端依赖
   - Linux 装系统依赖（`libwebkit2gtk-4.1-dev` 等）
   - `tauri-apps/tauri-action@v0` 构建（`tagName: ''` 跳过内置发布）
   - 上传 workflow artifact `bundles-<os>`
2. **release**（仅 tag 触发执行，`needs: build` 通过后跑）
   - `actions/download-artifact@v4` 下载所有 `bundles-*`
   - `npx conventional-changelog-cli@latest` 生成最新段 changelog
   - 拼接 `.github/release-body-prefix.md` + `---` + changelog → `release-body.md`
   - `softprops/action-gh-release@v2` 创建**草稿** Release，挂载 dmg/msi/exe/deb/rpm/AppImage

### 3.3 监控与发布

```text
监控：https://github.com/ShiJieCloud/rhost/actions/workflows/release.yml
Release 草稿：https://github.com/ShiJieCloud/rhost/releases
```

1. 在 Actions 页面等待 build 三平台全绿 + release job 全绿；
2. 进入 Releases 页面找到名为 `Rhost v1.0.0` 的**草稿**；
3. 核对 body（安装须知 + changelog）与附件列表；
4. 必要时编辑 body，参考 [release-notes-template.md](release-notes-template.md) §2 完整模板手写亮点；
5. 取消勾选 `Set as a pre-release`（默认未勾选）；
6. 点 `Publish release` 发布——草稿转正式，对外可见。

> ⚠️ 草稿在发布前对外不可见，可反复编辑、删除附件重传；一旦 Publish 即对外公开，撤销需 Delete release。

## 4. 场景二：手动触发（仅构建验证）

适用：升级 Tauri / Rust 版本后验证三平台能否打包，不发版。

### 4.1 操作步骤

1. 浏览器打开 `https://github.com/ShiJieCloud/rhost/actions/workflows/release.yml`；
2. 右上角点 `Run workflow` 下拉；
3. Branch 选 `main`（不要选其他分支，除非明确要验证分支构建）；
4. 点绿色 `Run workflow` 按钮。

### 4.2 流水线行为

- 仅跑 build job，三平台并行；
- **不跑 release job**（`if: startsWith(github.ref, 'refs/tags/')` 条件不满足）；
- 产物在 Actions 运行详情页的 `Artifacts` 区，可下载 `bundles-macos-latest` / `bundles-ubuntu-22.04` / `bundles-windows-latest` 验证；
- 不创建 Release，不污染 Releases 页面。

> 适合首次升级依赖、改 `tauri.conf.json` 后的回归验证。产物保留 90 天后由 GitHub 自动清理。

## 5. 场景三：重跑失败流水线

### 5.1 build job 失败

直接在 Actions 运行详情页点右上角 `Re-run failed jobs` / `Re-run all jobs`，无需重打 tag。常用于网络拉取依赖超时、Rust 编译 OOM 等瞬时故障。

### 5.2 release job 失败

release job 失败不会自动重跑（`needs: build` 已通过）。两种处理：

```bash
# 方式 A：删除远端 tag 后重打重推（推荐，保证 changelog 重新生成）
git push origin :refs/tags/v1.0.0   # 删远端 tag
git tag -d v1.0.0                   # 删本地 tag
git tag v1.0.0                      # 重打
git push origin v1.0.0              # 重推触发完整流水线
```

```text
方式 B：仅重跑 release job
在运行详情页点 Re-run failed jobs（仅 release job 会被重跑，build artifact 复用）
适用：release-body 拼接脚本失败、softprops 上传失败等 release 阶段瞬时故障
```

> 方式 A 会重新构建（耗时），但保证完整一致；方式 B 快但若 build artifact 已过期（>90 天）则不可用。

## 6. 场景四：预发布（beta / rc）

release.yml 的 `prerelease: false` 是**硬编码**，tag 名带 `-beta` / `-rc` 后缀不会自动标记 Pre-release。两种做法：

### 6.1 简单做法（不改 yml）

```bash
git tag v1.0.0-beta.1
git push origin v1.0.0-beta.1
```

流水线仍创建草稿，发布前在 Releases 页面**手动勾选** `Set as a pre-release`，再 `Publish release`。

### 6.2 自动识别（改 yml）

修改 `.github/workflows/release.yml` 第 131 行：

```yaml
prerelease: ${{ contains(github.ref_name, '-beta') || contains(github.ref_name, '-rc') }}
```

此后 `v1.0.0-beta.1` / `v1.0.0-rc.1` 自动标记 Pre-release，`v1.0.0` 仍为正式版。修改属 yml 变更，需单独 commit。

## 7. 排查

### 7.1 推了 tag 但 Actions 没触发

| 可能原因 | 排查方法 |
|---|---|
| tag 不以 `v` 开头 | `git tag -l` 检查命名，必须匹配 `v*` |
| tag 推到了非默认分支 | release.yml 的 `on.push.tags` 全局生效，但若仓库设置了分支保护 / tag 保护规则需检查 Settings → Branches / Tags |
| workflow 文件未在 main | `git ls-tree origin/main .github/workflows/release.yml` 应有输出 |
| Actions 被仓库禁用 | Settings → Actions → General，应允许 `Allow all actions` |

### 7.2 Release 草稿里没有 body 或 body 为空

| 可能原因 | 排查方法 |
|---|---|
| 无符合 Conventional Commits 的提交 | 检查 `git log v上一版本..v当前版本 --oneline`，应包含 `feat:` / `fix:` 等前缀 |
| `.github/release-body-prefix.md` 不存在 | 确认文件已入库：`git ls-files .github/release-body-prefix.md` |
| release job 的 `Build release body` step 失败 | Actions 运行详情页查看该 step 日志 |

### 7.3 artifact 缺失（如 macOS 无 dmg）

- `tauri.conf.json` 的 `bundle.targets` 应为 `"all"`；
- 检查 build job 的 `Upload bundle artifacts` step，`if-no-files-found: warn` 只警告不失败，需手动看日志；
- macOS 产物依赖 ad-hoc 签名 identity，本地能构建但 CI 若缺环境变量可能跳过 dmg，查看 `tauri-action` 日志。

### 7.4 三平台构建耗时过长或卡死

- `timeout-minutes: 60` 上限，超过自动取消；
- 首次构建无缓存 30–60min 正常，二次有 Rust/npm 缓存应降至 10–20min；
- 若持续卡死，检查 `Swatinem/rust-cache` 是否命中（Actions 日志 `Cache hit` 字样）。

## 8. 注意事项

- **产物未签名**：macOS 首次打开需右键 → 打开；Windows 有 SmartScreen 提示；Linux AppImage 需 `chmod +x`。详见 [release-notes-template.md](release-notes-template.md) 安装须知段；
- **并发控制**：同 tag/dispatch 排队不取消（`cancel-in-progress: false`），避免半成品 artifact；
- **commit 规范**：仓库已配 commitlint + husky，不规范 commit 本地即被拦截，changelog 也会有内容；
- **版本号三处一致**：`frontend/package.json` / `src-tauri/tauri.conf.json` / `src-tauri/Cargo.toml`，发版前必须同步；
- **同一 tag 不重推**：GitHub 不允许同 tag 名二次触发（除非先删），删 tag 重推见 §5.2 方式 A。
