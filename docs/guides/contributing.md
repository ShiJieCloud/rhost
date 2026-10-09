# 贡献指南

> description: Rhost 项目的贡献流程、分支策略、Commit 规范与 PR 要求
>
> created: 2026-10-09 20:28:07
>
> updated: 2026-10-09 20:28:07
>
> author: [sjzhao](https://github.com/ShiJieCloud/rhost)

## 1. 项目地址

| 平台 | 地址 |
|---|---|
| GitHub | https://github.com/ShiJieCloud/rhost |
| GitCode | https://gitcode.com/qq_20185737/rhost |

两个平台同步镜像，PR 可提交到任意一方。

## 2. 开发环境

### 2.1 依赖

| 工具 | 版本要求 |
|---|---|
| Rust | 稳定版（含 `rustup`） |
| Node.js | 18+ |
| pnpm | 9+ |

平台依赖详见 [开发指南](./development.md)。

### 2.2 启动开发

```bash
# 安装依赖
pnpm install

# 启动前端 + Tauri 开发模式
cd frontend && pnpm tauri dev
```

### 2.3 初始化 Git 钩子

仓库使用 `husky` + `commitlint` 校验提交信息。首次安装依赖后自动执行 `prepare` 脚本初始化钩子：

```bash
pnpm install   # 自动执行 husky 初始化
```

## 3. 分支策略

### 3.1 分支命名

| 类型 | 命名格式 | 示例 |
|---|---|---|
| 功能 | `feature/<简短描述>` | `feature/tunnel-traffic-stats` |
| 修复 | `fix/<简短描述>` | `fix/ssh-port-leak` |
| 文档 | `docs/<简短描述>` | `docs/contributing-guide` |
| 重构 | `refactor/<简短描述>` | `refactor/session-store` |
| 杂务 | `chore/<简短描述>` | `chore/update-deps` |

### 3.2 工作流

1. 从 `main` 分支创建功能分支
2. 开发并提交（遵循 Commit 规范）
3. 推送到远端
4. 提交 Pull Request 到 `main`
5. Code Review 通过后合并

## 4. Commit 规范

本项目采用 [Conventional Commits](https://www.conventionalcommits.org/zh-hans/v1.0.0/) 规范。

### 4.1 格式

```
<type>(<scope>): <description>

[optional body]

[optional footer]
```

### 4.2 type

| type | 说明 |
|---|---|
| `feat` | 新功能 |
| `fix` | 修复 Bug |
| `docs` | 文档变更 |
| `refactor` | 重构（不改变功能） |
| `perf` | 性能优化 |
| `test` | 测试相关 |
| `chore` | 构建/工具/依赖等杂务 |
| `style` | 代码格式（不影响逻辑） |

### 4.3 scope

scope 标明影响范围，可选。常用值：

- `ssh` — SSH 连接与会话
- `tunnel` — 端口转发
- `sftp` — SFTP 文件传输
- `terminal` — 终端
- `host` — 主机与密钥管理
- `ui` — 前端界面
- `config` — 配置导入导出
- `docs` — 文档
- `ci` — CI/CD

### 4.4 示例

```
feat(tunnel): 支持实时流量统计
fix(ssh): 修复断线后端口未释放
docs(contributing): 补充 PR 流程说明
refactor(session): 拆分会话状态管理
perf(sftp): 优化大文件传输分块策略
chore(deps): 升级 tauri 至 2.12
```

### 4.5 自动校验

- `commitlint` 在 `git commit` 时自动校验 `commit-msg`，不符合规范会被拒绝
- `CHANGELOG.md` 由 `conventional-changelog` 根据 commit 自动生成

## 5. Pull Request

### 5.1 提交流程

1. Fork 仓库（或在本仓库创建分支）
2. 创建功能分支并开发
3. 确保本地构建通过：
   ```bash
   cd frontend && pnpm build
   cargo build --manifest-path src-tauri/Cargo.toml
   ```
4. 推送分支并提交 PR
5. 填写 PR 描述，说明变更内容与动机

### 5.2 PR 描述模板

```markdown
## 变更内容

<!-- 简要说明本次 PR 的变更 -->

## 动机

<!-- 为什么需要这个变更，解决了什么问题 -->

## 测试方式

<!-- 如何验证变更的正确性 -->

## 关联 Issue

<!-- Closes #123 -->
```

### 5.3 Review 要求

- 至少 1 名维护者 Review 通过后方可合并
- CI 检查（构建、类型检查）必须通过
- 涉及 IPC 帧、配置 schema、设置项的变更需同步更新文档

## 6. 代码规范

### 6.1 Rust

- 遵循 `rustfmt` 格式，提交前运行 `cargo fmt`
- 遵循 `clippy` 建议，提交前运行 `cargo clippy`
- 公开函数需有文档注释

### 6.2 TypeScript / Vue

- 前端代码通过 `vue-tsc` 类型检查
- 组件使用 `<script setup lang="ts">`
- 遵循现有代码风格

### 6.3 文档

- 文档位于 `docs/` 目录，遵循 [文档规范](../explanation/design/docs-spec.md)
- 涉及功能变更时同步更新对应指南

## 7. 下一步

- [开发指南](./development.md)：环境搭建与项目结构；
- [文档规范](../explanation/design/docs-spec.md)：文档编写规范；
- [架构与通信协议](../explanation/architecture.md)：整体架构与 IPC 协议。
