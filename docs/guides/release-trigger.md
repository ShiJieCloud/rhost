# 触发发版流水线操作指南

> description: 发版流水线的重跑失败、预发布与故障排查等进阶操作；基础发版流程见 CI/CD 流水线文档
>
> created: 2026-10-08 15:48:47
>
> updated: 2026-10-09 20:45:00
>
> author: [sjzhao](https://github.com/ShiJieCloud/rhost)

## 1. 适用场景

维护者需要在以下任一场景触发 `.github/workflows/release.yml`：

- **正式发版**：打 `v*` tag，自动构建三平台产物 + 创建 Release 草稿；
- **构建验证**：只想验证三平台能否打包，不污染 Releases 页面；
- **重跑失败流水线**：tag 已 push 但 build/release job 失败，需要重跑；
- **预发布**：发 `beta` / `rc` 版本并手动标记 Pre-release。

> 基础发版流程（前置检查、正式发版、手动构建验证、发布 Release）见 [CI/CD 流水线 §4](ci-cd.md)；Release body 文案模板见 [../reference/release-notes-template.md](../reference/release-notes-template.md)；设计背景见 [ci-release-design.md](../explanation/design/ci-release-design.md)。

## 2. 重跑失败流水线

### 2.1 build job 失败

直接在 Actions 运行详情页点右上角 `Re-run failed jobs` / `Re-run all jobs`，无需重打 tag。常用于网络拉取依赖超时、Rust 编译 OOM 等瞬时故障。

### 2.2 release job 失败

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

## 3. 预发布（beta / rc）

release.yml 的 `prerelease: false` 是**硬编码**，tag 名带 `-beta` / `-rc` 后缀不会自动标记 Pre-release。两种做法：

### 3.1 简单做法（不改 yml）

```bash
git tag v1.0.0-beta.1
git push origin v1.0.0-beta.1
```

流水线仍创建草稿，发布前在 Releases 页面**手动勾选** `Set as a pre-release`，再 `Publish release`。

### 3.2 自动识别（改 yml）

修改 `.github/workflows/release.yml` 中 `prerelease` 字段：

```yaml
prerelease: ${{ contains(github.ref_name, '-beta') || contains(github.ref_name, '-rc') }}
```

此后 `v1.0.0-beta.1` / `v1.0.0-rc.1` 自动标记 Pre-release，`v1.0.0` 仍为正式版。修改属 yml 变更，需单独 commit。

## 4. 故障排查

### 4.1 推了 tag 但 Actions 没触发

| 可能原因 | 排查方法 |
|---|---|
| tag 不以 `v` 开头 | `git tag -l` 检查命名，必须匹配 `v*` |
| tag 推到了非默认分支 | release.yml 的 `on.push.tags` 全局生效，但若仓库设置了分支保护 / tag 保护规则需检查 Settings → Branches / Tags |
| workflow 文件未在 main | `git ls-tree origin/main .github/workflows/release.yml` 应有输出 |
| Actions 被仓库禁用 | Settings → Actions → General，应允许 `Allow all actions` |

### 4.2 Release 草稿里没有 body 或 body 为空

| 可能原因 | 排查方法 |
|---|---|
| 无符合 Conventional Commits 的提交 | 检查 `git log v上一版本..v当前版本 --oneline`，应包含 `feat:` / `fix:` 等前缀 |
| `.github/release-body-prefix.md` 不存在 | 确认文件已入库：`git ls-files .github/release-body-prefix.md` |
| release job 的 `Build release body` step 失败 | Actions 运行详情页查看该 step 日志 |

### 4.3 artifact 缺失（如 macOS 无 dmg）

- `tauri.conf.json` 的 `bundle.targets` 应为 `"all"`；
- 检查 build job 的 `Upload bundle artifacts` step，`if-no-files-found: warn` 只警告不失败，需手动看日志；
- macOS 产物依赖 ad-hoc 签名 identity，本地能构建但 CI 若缺环境变量可能跳过 dmg，查看 `tauri-action` 日志。

### 4.4 三平台构建耗时过长或卡死

- `timeout-minutes: 60` 上限，超过自动取消；
- 首次构建无缓存 30–60min 正常，二次有 Rust/npm 缓存应降至 10–20min；
- 若持续卡死，检查 `Swatinem/rust-cache` 是否命中（Actions 日志 `Cache hit` 字样）。

## 5. 注意事项

- **产物未签名**：macOS 从浏览器下载的 `.dmg` 装好后若提示「"rhost" 已损坏，无法打开」，运行 `xattr -dr com.apple.quarantine /Applications/rhost.app` 移除隔离属性后再打开（右键 → 打开对此错误无效）；Windows 有 SmartScreen 提示；Linux AppImage 需 `chmod +x`。详见 [../reference/release-notes-template.md](../reference/release-notes-template.md) 安装须知段；
- **并发控制**：同 tag/dispatch 排队不取消（`cancel-in-progress: false`），避免半成品 artifact；
- **commit 规范**：仓库已配 commitlint + husky，不规范 commit 本地即被拦截，changelog 也会有内容；
- **同一 tag 不重推**：GitHub 不允许同 tag 名二次触发（除非先删），删 tag 重推见 §2.2 方式 A。
