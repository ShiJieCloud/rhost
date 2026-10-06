# Vim 中文显示 / 保存编码问题排查与修复

> 状态：已落地（2026-10-06）
> 面向：开发者、测试、遇到远端编辑器中文乱码的高级用户
> 涉及代码：`src-tauri/src/ssh/session.rs`、`frontend/src/stores/settings.ts`

---

## 1. 问题现象

在 rhost 终端连接远端主机，用 `vi` / `vim` 编辑文件输入中文时出现两个阶段的问题：

| 阶段 | 现象 | 触发条件 |
|---|---|---|
| 显示乱码 | 输入"测试"，vim 画面显示成 `~K~U` 形式 | 远端 locale 缺失或无效（容器/精简系统常见） |
| 保存报错 | `CONVERSION ERROR in line N; NNL, NNB written`，伴随 `E37` / `E162` | 在已损坏的"脏文件"上继续用新环境编辑 |

关键观察（容易误导排查方向）：

- 同一串中文，保存退出后 `cat 文件` 显示**完全正常**；
- 文件 hexdump 也是合法 UTF-8。

这两点说明 **终端传输链路没有编码问题**，问题出在 vim 进程对字符集的判定上。

---

## 2. 排查方法论：沿字节链路逐层取证

不要从"中文乱码"直接跳到"编码配置错了"。这类问题必须沿数据链路逐层抓**原始字节**，用证据定位是哪一层解释错了字节。

链路为：

```
前端 xterm.onData(JS 字符串)
  → TextEncoder 编码为 UTF-8 字节
  → Tauri IPC(ArrayBuffer)
  → SSH 通道 data()
  → 远端 PTY → vim 读入
vim 渲染
  → 远端 PTY 输出
  → SSH → IPC → xterm 显示
```

### 2.1 前端发出的字节

在 `TerminalPane.vue` 的 `term.onData` 回调临时埋点，用 `TextEncoder` 转 hex：

```ts
term.onData(data => {
  if (/[\u0080-\uFFFF]/.test(data)) {
    const hex = Array.from(new TextEncoder().encode(data))
      .map(b => b.toString(16).padStart(2, '0')).join(' ')
    console.log('onData', hex) // "测"= e6 b5 8b  "试"= e8 af 95
  }
  sendInput(id, data)
})
```

### 2.2 后端 IPC 收到的字节

在 `src-tauri/src/ipc.rs` 的 `write_terminal` 临时落盘：

```rust
if data.iter().any(|b| *b >= 0x80) {
    let hex: Vec<String> = data.iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write("/tmp/rhost_cap.log", format!("recv {}\n", hex.join(" ")));
}
```

### 2.3 vim 回传给终端的字节（决定性证据）

在 `src-tauri/src/ssh/session.rs` PTY 输出 `flush()` 处，把含高位字节的块转储。如果**vim 回传的字节本身就是 `~K` / `~U`（ASCII `0x7E 0x4B`）**，则可断定：

> 乱码发生在 vim 内部，xterm 只是忠实显示了 vim 的输出。终端、前端、IPC、SSH 全部无辜。

### 2.4 让 vim 自报编码

在 vim 里执行（对应三张关键状态）：

```vim
:set encoding?        " vim 内部缓冲区/显示用编码
:set fileencoding?    " 当前文件读写用编码（空 = 跟随 encoding）
:set fileencodings?   " 打开文件时的编码自动探测顺序
```

本机结论：`encoding=latin1`、`fileencoding=latin1`、`fileencodings=ucs-bom,utf-8,default,latin1`（默认探测顺序正常）。

### 2.5 查远端实际字符集与进程环境

```bash
locale charmap                      # 期望 UTF-8；空 LANG 时是 ANSI_X3.4-1968
locale -a                           # 系统实际生成了哪些 locale
# 查 vim 进程实际继承到的环境（比查父 shell 更准，vim 是 shell 的子进程）
tr '\0' '\n' < /proc/<vim_pid>/environ | grep -E '^(LANG|LC_)'
```

> 注意：`/proc/<bash_pid>/environ` 是进程**启动时**的环境快照。通过 init 脚本 `source` + `export` 注入的变量不会写回它，因此判断注入是否生效要看 **fork 出来的子进程（vim）的 environ**，不要看 bash 的。

---

## 3. 根因

### 3.1 显示乱码：无效 locale 使 vim 退回 latin1

vim 启动时调用 `setlocale(LC_ALL, "")`，据此选择内部 `encoding`：

- 成功设为 UTF-8 locale → `encoding=utf-8`，按多字节序列正确解析 UTF-8；
- 失败 → 退回内置默认 **latin1**，把 UTF-8 的每个字节当成一个独立字符，`E6` 渲染为 `~`、`B5` 渲染为 `K`、`8B` 为控制字符，于是"测试"变成 `~K~U`。

远端 locale 失效有两种常见形态：

1. **无 `LANG`**（等同 `C` / `POSIX` locale），`locale charmap` = `ANSI_X3.4-1968`。干净容器的默认状态。
2. **`LANG` 指向未生成的 locale**。本项目的实际触发路径：
   - 前端默认配置（`frontend/src/stores/settings.ts`）给每个会话注入 `LANG=zh_CN.UTF-8`（沿用 macOS 本机语言）；
   - 但 debian 容器 `locale -a` 只有 `C.utf8`，**没有生成 zh_CN**；
   - 变量名含 UTF-8 不代表 locale 可用——`locale` 会报 `Cannot set LC_ALL to default locale: No such file or directory`。

### 3.2 为什么 `LC_CTYPE=C.UTF-8` 救不了，必须 `LC_ALL`

这是本次排查最反直觉、也最关键的结论。在容器内控制变量实测 vim `encoding`：

| 环境变量组合 | vim encoding |
|---|---|
| 仅 `LC_CTYPE=C.UTF-8` | `utf-8` ✓ |
| 仅无效 `LANG=zh_CN.UTF-8` | `latin1` ✗ |
| 无效 `LANG` + `LC_CTYPE=C.UTF-8` | **`latin1` ✗（救不回）** |
| 无效 `LANG` + `LC_ALL=C.UTF-8` | **`utf-8` ✓** |

原因：`LC_CTYPE` 只覆盖字符处理类别，而 `LC_MESSAGES` 等其它类别仍回退到无效的 `LANG`，导致 vim 的 `setlocale(LC_ALL, "")` **整体失败**。只有优先级最高、覆盖全部类别的 `LC_ALL` 能统一压制无效 LANG。

POSIX locale 优先级：`LC_ALL` > 各 `LC_*` > `LANG`。

### 3.3 保存报错：历史脏文件的字节已不可逆损坏

`CONVERSION ERROR` 不是新机制的 bug，而是修复前 latin1 时期遗留的文件：

- 旧 latin1 会话把 UTF-8 三字节当成三个单字节字符；用户多次换行 / 删除 / 插入后，多字节序列被**切断、错位重排**；
- 典型 hexdump：正确的 `E6 B5 8B`（测）、`E8 AF 95`（试）被打散，尾字节 `95` / `AF` / `BF` 孤立出现在不同行首，产生任何 UTF-8 解码器都会拒绝的孤立 continuation byte；
- 新的 utf-8 vim 打开该文件严格解码失败 → 按 `fileencodings` 回退判为 `fileencoding=latin1` → 用户新输入的真中文无法转回 latin1 → 写盘时 `CONVERSION ERROR`。

这种字节边界已丢失的损坏无法自动还原，只能删除重建或人工剔除损坏字节。

---

## 4. 解决方案：三层 locale 保底

落点全部在 `src-tauri/src/ssh/session.rs`，核心是在 shell / 编辑器启动前为远端挑选一个**实测可用的 UTF-8 locale**。

> **关键认知：不能写死 `UTF-8`，也不能只写 `C.UTF-8`。**
> - `UTF-8` 是**字符集名**，不是 locale 名。`LC_ALL=UTF-8` 系统查无此项，直接报 `Cannot set LC_ALL ... No such file or directory` 并退回 C，vim 仍 latin1（实测）。
> - 没有全平台通用的 locale 名。`C.UTF-8` 虽为 glibc≥2.35 / musl 内置、免安装、语言中性，但 **glibc<2.35 的 CentOS 7 / RHEL 7-8 / Amazon Linux 2 没有它**；这些系统通常有 `en_US.UTF-8`。
>
> 因此只能在运行时拿一组候选名逐个用 `locale charmap` 实测，取第一个真能输出 UTF-8 的：`C.UTF-8` → `C.utf8` → `en_US.UTF-8` → `en_US.utf8`。

保底遵循两条铁律：

1. **实测优先，不看变量名**：以 `locale charmap` 实际输出（按 `UTF-?8` 宽松匹配，兼容输出 `UTF8` 的实现）为准，而非 `LANG` 字符串里是否含 "UTF-8"。
2. **只在失效时才覆盖**：远端 locale 本来有效时零改动，完整保留用户的界面语言、日期 / 数字格式偏好；绝不硬写 `LANG`。

### 4.1 init 脚本通道（决定性兜底，所有路径必走）

`init_script()` 生成的脚本经 SFTP 上传，shell 启动后静默 `source`。它不依赖 sshd 的 `AcceptEnv`，因此 **exec 与标准 shell 两条启动路径都会执行**，且在 shell 内 `export` 的变量会被后续 fork 的 vim 继承。探测片段（常量 `UTF8_LOCALE_PICK_SH`，exec 路径复用同一份）在用户自定义 env 块**之前**插入：

```sh
_rh_l=''
if ! printf '%s' "$(locale charmap 2>/dev/null)" | grep -qiE '^UTF-?8$'; then
  for _rh_c in C.UTF-8 C.utf8 en_US.UTF-8 en_US.utf8; do
    if LC_ALL="$_rh_c" locale charmap 2>/dev/null | grep -qiE '^UTF-?8$'; then
      _rh_l=$_rh_c
      break
    fi
  done
  if [ -z "$_rh_l" ] && { ! command -v locale >/dev/null 2>&1 || [ -z "$(locale charmap 2>/dev/null)" ]; }; then
    _rh_l=C.UTF-8
  fi
fi
# init 脚本随后：if [ -n "$_rh_l" ]; then export LC_ALL="$_rh_l"; fi
```

结果与分支：

- 已是有效 UTF-8（含 zh_CN 已生成的系统）→ 外层不进入，`_rh_l` 空，**零改动**；
- 非 UTF-8 且某个候选实测可用 → `_rh_l=<候选名>`，export `LC_ALL` 压住随后 env 块注入的无效 `LANG`；首选 `C.UTF-8`，旧发行版回退 `en_US.UTF-8`（实测跳过缺失候选继续探测的控制流）；
- 无 `locale` 命令（极简 busybox/musl）或 `locale charmap` 无输出（残损 busybox applet）→ 直接保底 `C.UTF-8`（musl 按 codeset 宽松接受并按 UTF-8 工作）；
- 有 `locale` 命令、charmap 正常（非空）但候选**全部**不可用（CentOS 极瘦镜像，连 en_US 都没装）→ `_rh_l` 空，**刻意不设**，避免无效 `LC_ALL` 让 glibc 程序刷 `Cannot set locale` 警告；此时需管理员安装 `glibc-langpack-en` / `locale-gen en_US`。

### 4.2 exec（抑制原生 MOTD）路径

`locale_login_cmd()` 构造的启动命令串复用同一份 `UTF8_LOCALE_PICK_SH`，供 readline / shell 自身在**启动瞬间**就拿到 UTF-8（早于 init 脚本 source）。探测到候选才带 `LC_ALL` 前缀 exec；`_rh_l` 空（已是 UTF-8，或极瘦镜像无候选）则裸 exec，绝不塞无效 locale：

```sh
# <UTF8_LOCALE_PICK_SH：同上，探测结果写入 _rh_l>
if [ -n "$_rh_l" ]; then
  LC_ALL="$_rh_l" exec '<shell>' -l
else
  exec '<shell>' -l
fi
```

### 4.3 标准 shell 路径的盲发（保留 LC_CTYPE）

标准 `request_shell` 路径在 shell 启动**前**通过 `channel.set_env("LC_CTYPE", "C.UTF-8")` 注入。此处**故意仍用 `LC_CTYPE` 而非 `LC_ALL`**：

- 这是无法先探测远端的"盲发"，`LC_ALL` 会无条件覆盖正常服务器上用户 `LANG` 的界面语言 / 地区格式；
- 此刻前端 env 的 `LANG` 尚未注入，`LC_CTYPE` 足以覆盖 shell/readline 的早期窗口；
- 是否被接受还受 sshd `AcceptEnv` 限制（alpine 镜像默认拒绝），所以它只是尽力而为；
- 针对"无效 LANG 拖垮 vim"的决定性兜底统一交给 §4.1 的有条件 `LC_ALL`。

### 4.4 三层分工小结

| 通道 | 时机 | 变量 | 是否有条件 | 主要解决 |
|---|---|---|---|---|
| `locale_login_cmd` exec | shell 启动命令 | `LC_ALL=<实测候选>` | 候选实测 charmap | readline、shell 自身 |
| `set_env` | shell 启动前（盲发） | `LC_CTYPE=C.UTF-8` | 否（受 AcceptEnv 约束） | 早期窗口，尽力而为 |
| init 脚本 source | shell 启动后、vim 前 | `LC_ALL=<实测候选>` | 候选实测 charmap | **vim 等子进程（决定性）** |

> 候选统一为 `C.UTF-8 C.utf8 en_US.UTF-8 en_US.utf8`；探测片段是常量 `UTF8_LOCALE_PICK_SH`，exec 与 init 两处共享，避免逻辑漂移。init 脚本经 exec 通道上传，落盘失败（`/tmp` 不可写等）会记 warn 事件 `ssh.session.init_script_failed` 并令 `init_cmd=None`——此时只剩 `set_env` 这条会被 AcceptEnv 限制的路径，vim 兜底可能缺席，排查时据此判断。

---

## 5. 验证

### 5.1 单元测试

`src-tauri/src/ssh/session.rs` 中：

- `utf8_locale_fallback_block_present_before_user_env`：守卫 init 脚本——候选顺序固定为 `C.UTF-8 C.utf8 en_US.UTF-8 en_US.utf8`、charmap 按 `UTF-?8` 实测、含无 locale 命令与 busybox 残损两路兜底、`export LC_ALL="$_rh_l"` 在用户 env 之前；断言不硬写 `LANG`、不出现非法的 `LC_ALL=UTF-8`、外层"非 UTF-8 才探测"、临时变量 unset。
- `locale_login_cmd_uses_candidate_pick_and_safe_fallback`：守卫 exec 路径复用同一探测（含 en_US 候选），命中带 `LC_ALL` 前缀 exec、无候选时行首裸 exec，并覆盖 shell 路径单引号转义。
- 现有 `env_block_lands_after_clear_line_in_script` 保证整体脚本段落顺序。

```bash
cd src-tauri && cargo test --lib
```

### 5.2 容器端到端（真实脚本 + 五场景）

用单元测试导出后端**实际生成**的 init 脚本（避免手抄误差），在无 LANG 的 login bash 中 source，随后注入无效 `LANG=zh_CN.UTF-8`，再驱动 vim：

- `locale charmap` = `UTF-8`；
- `:set encoding?` = `utf-8`；
- PTY 输入多行中文 `:w` 保存，**无 `CONVERSION ERROR`**；
- `iconv -f UTF-8 -t UTF-8 文件` 通过，hexdump 为合法 UTF-8。

探测片段本身在以下五环境单独验证（`_rh_l` 结果与 vim encoding）：

| 场景 | 环境构造 | `_rh_l` | vim encoding |
|---|---|---|---|
| A 干净 debian C locale | `env -i`（无 LANG） | `C.UTF-8` | utf-8 |
| B 首候选缺失（类 CentOS） | 假名候选在前、可用名在末位 | 跳过假名命中末位可用名 | utf-8 |
| C 已是 UTF-8 | `LANG=C.utf8` | 空（零改动） | utf-8 |
| D 极简 busybox/musl | alpine（无 `locale` 命令），ash 解析 | `C.UTF-8`（兜底） | —（musl 按 UTF-8） |
| E 极瘦 glibc 无任何 UTF-8 locale | 候选全不可用、charmap 非空 | 空（刻意不设） | latin1（需装语言包） |

```bash
docker exec -u test rhost-test-sshd-debian env -i PATH=/usr/local/bin:/usr/bin:/bin \
  HOME=/home/test TERM=xterm-256color /bin/bash -c '
. /tmp/real.ri
vim -u NONE -N -e -s --cmd "set encoding?" --cmd "qa!"   # 期望 encoding=utf-8
'
```

### 5.3 手工验证（真实 GUI）

1. 重新编译 / 重启 app 后，**断开并重新建立会话**（旧会话进程仍是 latin1，不会自动切换）；
2. `vim 新文件.txt` → `i` → 中文输入法输入 → `Esc` → `:wq`；
3. 画面中文正常、无转换错误，`cat` 正常。

---

## 6. 历史脏文件处理

对已经在 latin1 时期损坏的文件：

```bash
# 判断是否含非法 UTF-8
iconv -f UTF-8 -t UTF-8 文件 >/dev/null 2>&1 && echo 合法 || echo 已损坏

# 无保留价值（多为测试数据）：直接删除，并清理 vim swap
rm -f 文件 .文件.swp .文件.swo

# 需保留完好的英文/数字行：剔除非 UTF-8 字节后另存
iconv -f UTF-8 -t UTF-8 -c 旧文件 > 新文件   # -c 丢弃无法转换的字节
```

损坏字节的字符边界已丢失，`iconv` / `enca` 等工具无法还原原始中文，只能丢弃损坏部分。

---

## 7. 避坑清单

- **不要把字符集名当 locale 名**：`LC_ALL=UTF-8` 非法（系统报 No such file，退回 C，vim 仍 latin1）；要填的是 `C.UTF-8` / `en_US.UTF-8` 这类**已生成的 locale 名**。
- **没有全平台通用的 locale 名，必须候选探测**：`C.UTF-8` 在 glibc<2.35 的 CentOS 7 / RHEL 7-8 / Amazon Linux 2 不存在，写死会静默落空；候选 `C.UTF-8 C.utf8 en_US.UTF-8 en_US.utf8` 逐个 `locale charmap` 实测。
- **极瘦镜像（有 locale 但无任何 UTF-8 locale）不要硬塞**：此时设任何名都是无效值，glibc 程序会刷 `Cannot set locale`；应保持不设，提示装 `glibc-langpack-en` / `locale-gen`。
- **不要只凭 `LANG` 含 UTF-8 就认为远端是 UTF-8**；必须 `locale charmap` 实测（匹配放宽到 `UTF-?8`），注意未 `locale-gen` 的情况。
- **无效 `LANG` 存在时，`LC_CTYPE` 压不住，要用 `LC_ALL`**；这是 setlocale 全类别语义决定的，不是 vim 特例。
- **不要用 `set_env` 盲发 `LC_ALL`**：会误伤 locale 正常的服务器（覆盖界面语言）；无条件注入用 `LC_CTYPE`，有条件兜底用 `LC_ALL`。
- **不要硬写 `LANG`**：只统一字符处理类别（`LC_ALL` 保底），把语言 / 地区选择权留给用户和远端。
- **判断 init 脚本注入是否生效，看 vim 子进程的 `/proc/<pid>/environ`**，不要看父 bash 的启动快照。
- **init 脚本上传失败会静默降级 locale/CWD/提示符三项**：现以 warn 事件 `ssh.session.init_script_failed`（含 err）记录，排查时先看它；常见原因为 `/tmp` 不可写、磁盘满、exec 通道被 `ForceCommand` 限制。
- **排查显示乱码先抓"vim 回传字节"**：若回传已是 `~x` 即 vim 内部问题，可立即排除前端 / IPC / SSH，避免在传输层空转。
- **改完 locale 逻辑必须重新连接会话**：locale 在进程启动 / 子进程 fork 时确定，已存在的 latin1 会话不会热切换。
- **POSIX 兼容**：探测片段须在 bash/dash/ash 通用——用 `grep -qiE`、`for x in ...`、brace group `{ ...; }`，避免 bash 特有语法；已在 alpine busybox ash 实测。
