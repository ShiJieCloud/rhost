//! AI 助手后端（M1）。
//!
//! 职责分层（后续 Agent/ReAct 在本层之上演进，协议层刻意不引 SDK）：
//! - 协议：OpenAI 兼容 `chat/completions` 流式（SSE）与 `GET /models` 探活；
//! - 凭据：API Key 仅存系统钥匙串（复用 [`crate::store`] 的跨平台封装），
//!   旧版明文 `settings.aiApiKey` 作为只读回退，前端保存后即完成迁移；
//! - 会话记录：按 scope（"global" | SSH 会话 id）落盘到 `app_config_dir/ai/chats/`；
//! - 跨会话记忆：全局记忆条目落盘到 `app_config_dir/ai/memory/memory.json`（M3.3）。
//!
//! 流式事件（`app.emit` 广播，前端按 scope 过滤）：
//! - `ai-token` `{scope, kind: "content"|"reasoning", delta}`：增量 token；
//! - `ai-done`  `{scope, reason: "completed"|"stopped"}`：正常结束或用户中止；
//! - `ai-error` `{scope, message}`：流式阶段错误（建连前错误直接以 Err 返回）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;
use std::time::Instant;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::applog::persisted::{self, APP_CONFIG_FILE_NAME};
use crate::store;

/// API Key 在系统钥匙串中的 account 名（service 复用 com.rhost.app）
const AI_KEYRING_ACCOUNT: &str = "ai-api-key";

/// 进行中的对话流：scope → 取消令牌。同一 scope 发起新请求会顶替（取消）旧请求
#[derive(Default)]
pub struct AiState {
    cancels: StdMutex<HashMap<String, CancellationToken>>,
    /// 会话在飞 exec 任务取消令牌：session_id → Vec<CancellationToken>。
    /// 同一会话可能并行多个只读工具，用 Vec 收集；会话切换时全部取消。
    exec_cancels: StdMutex<HashMap<String, Vec<CancellationToken>>>,
}

impl AiState {
    /// 登记新令牌并取消该 scope 上可能残留的旧请求，返回本次令牌
    fn register(&self, scope: &str) -> CancellationToken {
        let token = CancellationToken::new();
        let mut map = self.cancels.lock().unwrap();
        if let Some(old) = map.insert(scope.to_string(), token.clone()) {
            old.cancel();
        }
        token
    }

    fn unregister(&self, scope: &str, token: &CancellationToken) {
        let mut map = self.cancels.lock().unwrap();
        // 仅移除自己：极端情况下新请求已顶替，不能误删新令牌
        if map.get(scope).is_some_and(|t| t == token) {
            map.remove(scope);
        }
    }

    /// 登记新 exec 任务令牌，返回克隆供 tokio::select! 使用
    fn register_exec(&self, session_id: &str) -> CancellationToken {
        let token = CancellationToken::new();
        let mut map = self.exec_cancels.lock().unwrap();
        map.entry(session_id.to_string())
            .or_default()
            .push(token.clone());
        token
    }

    /// 取消并清除指定会话的全部在飞 exec 任务令牌
    fn cancel_execs(&self, session_id: &str) {
        let mut map = self.exec_cancels.lock().unwrap();
        if let Some(tokens) = map.remove(session_id) {
            for t in tokens {
                t.cancel();
            }
        }
    }

    /// exec 完成后注销令牌（正常/超时/取消均调用，避免列表膨胀）
    fn unregister_exec(&self, session_id: &str, token: &CancellationToken) {
        let mut map = self.exec_cancels.lock().unwrap();
        if let Some(tokens) = map.get_mut(session_id) {
            tokens.retain(|t| t != token);
            if tokens.is_empty() {
                map.remove(session_id);
            }
        }
    }
}

/* =========================================================
 *  配置读取
 * ========================================================= */

/// 对话请求所需的生效配置（已完成默认值/钥匙串回退）
struct AiCfg {
    base_url: String,
    model: String,
    api_key: Option<String>,
}

/// 读取 app_config.json 中的 AI 设置。
/// Key 解析顺序：系统钥匙串 → 旧版明文 settings.aiApiKey（兼容迁移前版本）。
fn read_ai_settings(app: &AppHandle) -> Result<AiCfg, String> {
    let path = app_config_file(app)?;
    let file = persisted::load_full(&path);
    let s = &file.settings;

    if !s.get("aiEnabled").and_then(|v| v.as_bool()).unwrap_or(true) {
        return Err("AI 助手未启用，请先在设置中开启".to_string());
    }
    let base_url = s
        .get("aiBaseUrl")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .trim_end_matches('/')
        .to_string();
    if base_url.is_empty() {
        return Err("未配置接口地址 Base URL".to_string());
    }
    let model = s
        .get("aiModel")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if model.is_empty() {
        return Err("未配置模型".to_string());
    }
    let api_key = keyring_or_legacy(s.get("aiApiKey").and_then(|v| v.as_str()))?;
    Ok(AiCfg {
        base_url,
        model,
        api_key,
    })
}

/// Key 解析：settings 明文非空（兼容迁移前版本）直接采用；否则读系统钥匙串
fn keyring_or_legacy(legacy: Option<&str>) -> Result<Option<String>, String> {
    if let Some(k) = legacy.map(str::trim).filter(|k| !k.is_empty()) {
        return Ok(Some(k.to_string()));
    }
    if let Some(k) = store::keyring_get(AI_KEYRING_ACCOUNT)?.filter(|k| !k.trim().is_empty()) {
        return Ok(Some(k));
    }
    Ok(None)
}

/// 测试连接专用：草稿为空时回退到钥匙串，再回退到已保存的 settings 明文
fn resolve_api_key(app: &AppHandle, draft: Option<&str>) -> Result<Option<String>, String> {
    if let Some(k) = draft.map(str::trim).filter(|k| !k.is_empty()) {
        return Ok(Some(k.to_string()));
    }
    if let Some(k) = store::keyring_get(AI_KEYRING_ACCOUNT)?.filter(|k| !k.trim().is_empty()) {
        return Ok(Some(k));
    }
    let path = app_config_file(app)?;
    let legacy = persisted::load_full(&path)
        .settings
        .get("aiApiKey")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|k| !k.trim().is_empty());
    Ok(legacy)
}

fn app_config_file(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|d| d.join(APP_CONFIG_FILE_NAME))
        .map_err(|e| format!("获取应用配置目录失败: {e}"))
}

/* =========================================================
 *  流式对话
 * ========================================================= */

/// 前端传入的对话消息。
/// M2.3 起支持 OpenAI 工具消息形态：assistant 可携带 `tool_calls`（模型历史中的提案），
/// `tool` 角色消息回传执行结果（`tool_call_id` 关联）；content 允许为 null。
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct AiMessage {
    pub(crate) role: String,
    #[serde(default)]
    pub(crate) content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_call_id: Option<String>,
}

/// assistant 历史消息中的工具调用记录（与线上协议同构）
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ToolCall {
    pub(crate) id: String,
    #[serde(rename = "type")]
    pub(crate) call_type: String,
    pub(crate) function: ToolFn,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ToolFn {
    pub(crate) name: String,
    /// JSON 字符串形式的参数（协议约定，原样透传）
    pub(crate) arguments: String,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [AiMessage],
    stream: bool,
    /// 模型可调用的工具定义（enableTools 时为内置 propose_command，见 [`builtin_tools`]）
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<serde_json::Value>,
}

/// 发起一轮流式对话。token 经 `ai-token` 事件增量推送，结束推送 `ai-done`；
/// 同 scope 的旧请求会被自动取消。
/// `enable_tools` 为 true 时附加内置工具定义；模型请求工具调用时经 `ai-tool-calls`
/// 事件透传提案（后端不执行工具，裁决与循环驱动由前端完成）。
#[tauri::command]
pub async fn ai_chat_stream(
    app: AppHandle,
    state: State<'_, AiState>,
    scope: String,
    messages: Vec<AiMessage>,
    enable_tools: Option<bool>,
) -> Result<(), String> {
    validate_scope(&scope)?;
    if messages.is_empty() {
        return Err("消息列表为空".to_string());
    }
    let cfg = read_ai_settings(&app)?;

    let tools = enable_tools.unwrap_or(false).then(builtin_tools);
    let token = state.register(&scope);
    let outcome = run_stream(&app, &cfg, &scope, &messages, token.clone(), tools).await;
    state.unregister(&scope, &token);
    outcome
}

/// 用户主动中止指定 scope 的流式回复（幂等：无进行中请求时无操作）
#[tauri::command]
pub fn ai_stop_chat(state: State<'_, AiState>, scope: String) {
    if let Some(t) = state.cancels.lock().unwrap().get(&scope) {
        t.cancel();
    }
}

/// 内置工具定义。后端只定义协议，不执行——
/// 提案经 `ai-tool-calls` 事件透传给前端，由前端路由：
/// - 只读工具（read_file / list_directory / get_system_info）经 SSH exec channel
///   自动执行，无需用户授权；
/// - propose_command 经用户裁决后写入终端 PTY 执行。
fn builtin_tools() -> serde_json::Value {
    serde_json::json!([
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "读取远程服务器上的文件内容。适用于查看配置文件、日志片段、脚本源码等。单次最多返回 8KB 文本，超出截断。路径不存在或无权限时返回错误信息。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "文件的绝对路径，如 /etc/nginx/nginx.conf" }
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "list_directory",
                "description": "列出远程服务器上指定目录的文件和子目录，包含名称、类型、大小、权限、修改时间，以及条目类型统计（目录/普通文件/符号链接/socket 等数量）。适用于探索目录结构、查找配置文件位置、了解目录内容分布。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "目录的绝对路径，如 /etc/nginx" }
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "get_system_info",
                "description": "获取远程服务器的系统信息快照：操作系统、内核版本、CPU 核数、内存、磁盘使用率、运行时间。无需参数。适用于了解环境后进行诊断。",
                "parameters": { "type": "object", "properties": {} }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "edit_file",
                "description": "编辑远程服务器上的文件，用 new_text 精确替换 old_text。适用于修改配置项、修复代码片段等小范围变更。old_text 必须与文件中现有内容完全匹配（含空白），否则替换失败。执行前会自动备份原文件。单次替换内容 ≤ 64KB。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "文件的绝对路径，如 /etc/nginx/nginx.conf" },
                        "old_text": { "type": "string", "description": "要被替换的原始文本，必须与文件内容完全匹配" },
                        "new_text": { "type": "string", "description": "替换后的新文本" }
                    },
                    "required": ["path", "old_text", "new_text"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "将内容写入远程服务器上的文件，全量覆盖（文件不存在则创建）。适用于新建文件或大幅重写。执行前若文件已存在会自动备份。单次写入内容 ≤ 64KB。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "文件的绝对路径，如 /etc/nginx/conf.d/default.conf" },
                        "content": { "type": "string", "description": "要写入的完整文件内容" }
                    },
                    "required": ["path", "content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "propose_command",
                "description": "向用户当前活动的终端提案一条命令。命令会完整展示给用户，经用户批准后写入终端并执行。仅在需要执行命令行操作、重启服务、安装软件等场景时调用。文件读写请使用 read_file / edit_file / write_file，不要用此工具执行 cat/sed/echo 重定向等文件操作。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "要写入终端的命令原文，必须是用户可读可审的完整命令"
                        },
                        "reason": {
                            "type": "string",
                            "description": "一句话说明执行这条命令的目的"
                        }
                    },
                    "required": ["command"]
                }
            }
        }
    ])
}

/// 只读工具执行结果（前端 camelCase）
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolExecResult {
    /// stdout + stderr 合并文本，已截断至 8KB（超时时为部分片段）
    output: String,
    /// true 表示原始输出超出 8KB 已截断
    truncated: bool,
    /// 远端命令退出码：Some(0)=成功；Some(n>0)=命令业务失败；None=通道未给出 ExitStatus
    exit_code: Option<i32>,
    /// true 表示执行超时，output 含已收到的部分输出
    timed_out: bool,
}

/// 工具名 → shell 命令构造（后端单点，前端不参与拼命令）
fn build_tool_command(tool_name: &str, args: &str) -> Result<String, String> {
    let v: serde_json::Value =
        serde_json::from_str(args).map_err(|e| format!("工具参数解析失败: {e}"))?;
    match tool_name {
        "read_file" => {
            let path = v
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or("read_file 缺少 path 参数")?;
            Ok(format!("head -c 8192 {}", shell_quote(path)))
        }
        "list_directory" => {
            let path = v
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or("list_directory 缺少 path 参数")?;
            // ls -la 列表 + find 类型统计（f=普通文件 d=目录 l=符号链接 s=socket 等）
            Ok(format!(
                "ls -la --time-style=long-iso {}; echo '--- 类型统计 ---'; find {} -maxdepth 1 -printf '%y\\n' 2>/dev/null | sort | uniq -c",
                shell_quote(path),
                shell_quote(path)
            ))
        }
        "get_system_info" => Ok(concat!(
            "echo '== OS =='; ",
            "(cat /etc/os-release 2>/dev/null || uname -s) | head -5; ",
            "echo '== KERNEL =='; uname -r; ",
            "echo '== CPU =='; nproc 2>/dev/null; ",
            "echo '== MEMORY =='; free -h 2>/dev/null || echo 'N/A'; ",
            "echo '== DISK =='; df -h / 2>/dev/null; ",
            "echo '== UPTIME =='; uptime 2>/dev/null"
        )
        .to_string()),
        "edit_file" => {
            let path = v
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or("edit_file 缺少 path 参数")?;
            let old_text = v
                .get("old_text")
                .and_then(|v| v.as_str())
                .ok_or("edit_file 缺少 old_text 参数")?;
            let new_text = v
                .get("new_text")
                .and_then(|v| v.as_str())
                .ok_or("edit_file 缺少 new_text 参数")?;
            check_path_safe(path)?;
            if old_text.len() > FILE_EDIT_MAX_BYTES || new_text.len() > FILE_EDIT_MAX_BYTES {
                return Err(format!(
                    "edit_file 内容超过 {} 字节上限",
                    FILE_EDIT_MAX_BYTES
                ));
            }
            if old_text.is_empty() {
                return Err("edit_file old_text 不能为空".to_string());
            }
            // \Q...\E 禁用 old_text 的正则元字符解析，按字面量匹配
            let old_escaped = format!("\\Q{}\\E", old_text);
            // new_text 转义 perl 替换侧的特殊字符：\ $ @ /
            let new_escaped = perl_replacement_escape(new_text);
            Ok(format!(
                "[ -f {} ] && cp {} {}.bak.$(date +%s) 2>/dev/null; perl -0pi -e 's/{}/{}/g' {}",
                shell_quote(path),
                shell_quote(path),
                shell_quote(path),
                old_escaped,
                new_escaped,
                shell_quote(path)
            ))
        }
        "write_file" => {
            let path = v
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or("write_file 缺少 path 参数")?;
            let content = v
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or("write_file 缺少 content 参数")?;
            check_path_safe(path)?;
            if content.len() > FILE_EDIT_MAX_BYTES {
                return Err(format!(
                    "write_file 内容超过 {} 字节上限",
                    FILE_EDIT_MAX_BYTES
                ));
            }
            // heredoc 用单引号定界符防变量/命令替换展开
            Ok(format!(
                "[ -f {} ] && cp {} {}.bak.$(date +%s) 2>/dev/null; cat > {} << 'RHOST_EOF'\n{}\nRHOST_EOF",
                shell_quote(path),
                shell_quote(path),
                shell_quote(path),
                shell_quote(path),
                content
            ))
        }
        _ => Err(format!("未知工具: {tool_name}")),
    }
}

/// 文件编辑单次内容上限：64KB
const FILE_EDIT_MAX_BYTES: usize = 64 * 1024;

/// 禁止写入的系统关键文件黑名单（精确匹配路径）
const FORBIDDEN_PATHS: &[&str] = &[
    "/etc/passwd",
    "/etc/shadow",
    "/etc/gshadow",
    "/etc/sudoers",
    "/etc/sudoers.d",
];

/// 校验写入路径是否在黑名单中
fn check_path_safe(path: &str) -> Result<(), String> {
    if FORBIDDEN_PATHS.iter().any(|p| path == *p) {
        return Err(format!("禁止写入系统关键文件: {}", path));
    }
    Ok(())
}

/// perl 替换侧（new_text）转义：\ $ @ / 需转义，避免被 perl 解析为变量/分隔符
fn perl_replacement_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' | '$' | '@' | '/' => out.push('\\'),
            _ => {}
        }
        out.push(c);
    }
    out
}

/// 单引号 shell 转义：' → '\'' （与 exec_collect 的转义口径一致）
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// 输出截断至 8KB UTF-8 字节（与 PROMPT_ASSET_MAX_BYTES 同口径）
fn cap_output(s: &str) -> (bool, String) {
    const MAX: usize = 8 * 1024;
    let bytes = s.as_bytes();
    if bytes.len() <= MAX {
        return (false, s.to_string());
    }
    // 在 UTF-8 边界截断，避免拆分多字节字符
    let mut end = MAX;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    (true, s[..end].to_string())
}

/// 执行只读工具：经 SessionManager 查找会话，exec_collect 执行 shell 命令。
/// 工具名白名单 + 单引号转义防止注入；exec channel 与 PTY 共享同一 SSH 连接。
#[tauri::command]
pub async fn ai_exec_tool(
    session_id: String,
    tool_name: String,
    args: String,
    state: State<'_, crate::ssh::manager::SessionManager>,
    ai_state: State<'_, AiState>,
) -> Result<ToolExecResult, String> {
    let cmd = build_tool_command(&tool_name, &args)?;

    // 登记取消令牌：会话切换时前端调 ai_cancel_execs 取消在飞 exec
    let token = ai_state.register_exec(&session_id);
    let exec_fut = state.exec_collect(&session_id, &cmd, std::time::Duration::from_secs(10));
    // exec_collect 与取消令牌竞速：令牌取消时 exec_fut 被 drop → 通道关闭
    let result = tokio::select! {
        _ = token.cancelled() => ToolExecResult {
            output: String::new(),
            truncated: false,
            exit_code: None,
            timed_out: false,
        },
        r = exec_fut => match r {
            Ok(exec) => {
                let (truncated, capped) = cap_output(&exec.output);
                ToolExecResult {
                    output: capped,
                    truncated,
                    exit_code: exec.exit_code,
                    timed_out: exec.timed_out,
                }
            }
            // 通道级故障（会话不存在/channel_open_session 失败等）：无任何输出
            Err(_) => ToolExecResult {
                output: String::new(),
                truncated: false,
                exit_code: None,
                timed_out: false,
            },
        }
    };
    ai_state.unregister_exec(&session_id, &token);
    Ok(result)
}

/// 取消指定会话的全部在飞 exec 任务（会话切换时前端调用，防后端堆积）
#[tauri::command]
pub fn ai_cancel_execs(session_id: String, ai_state: State<'_, AiState>) {
    ai_state.cancel_execs(&session_id);
}

async fn run_stream(
    app: &AppHandle,
    cfg: &AiCfg,
    scope: &str,
    messages: &[AiMessage],
    token: CancellationToken,
    tools: Option<serde_json::Value>,
) -> Result<(), String> {
    let client = http_client()?;
    let url = format!("{}/chat/completions", cfg.base_url);
    let mut req = client
        .post(&url)
        .json(&ChatRequest {
            model: &cfg.model,
            messages,
            stream: true,
            tools,
        });
    if let Some(key) = &cfg.api_key {
        req = req.bearer_auth(key);
    }

    // 建连阶段也可被中止
    let send_fut = req.send();
    let response = tokio::select! {
        _ = token.cancelled() => {
            let _ = app.emit("ai-done", serde_json::json!({ "scope": scope, "reason": "stopped" }));
            return Ok(());
        }
        r = send_fut => r.map_err(|e| format!("请求模型服务失败: {e}"))?,
    };

    let status = response.status();
    if !status.is_success() {
        // 错误体截断，避免上游 HTML 错误页刷屏
        let body = response.text().await.unwrap_or_default();
        let snippet: String = body.chars().take(500).collect();
        return Err(format!("模型服务返回 HTTP {status}: {snippet}"));
    }

    let mut stream = response.bytes_stream();
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    // 本轮响应累积器：tool_calls 分片拼接 + finish_reason（M2.3 工具提案）
    let mut acc = StreamAcc::default();

    macro_rules! emit_done {
        ($reason:expr) => {
            let _ = app.emit(
                "ai-done",
                serde_json::json!({ "scope": scope, "reason": $reason }),
            );
        };
    }

    loop {
        let chunk = tokio::select! {
            _ = token.cancelled() => {
                emit_done!("stopped");
                return Ok(());
            }
            next = stream.next() => next,
        };

        match chunk {
            None => {
                // 对端关闭流（未显式给 [DONE] 也视为完成）
                finish_stream(app, scope, &acc);
                emit_done!("completed");
                return Ok(());
            }
            Some(Err(e)) => {
                let msg = format!("读取模型响应失败: {e}");
                let _ = app.emit(
                    "ai-error",
                    serde_json::json!({ "scope": scope, "message": msg }),
                );
                return Err(msg);
            }
            Some(Ok(bytes)) => {
                buf.extend_from_slice(&bytes);
                // 按行消费所有完整行，残余字节留待下个 chunk
                while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = buf.drain(..=pos).collect();
                    handle_sse_line(app, scope, &line, &mut acc)?;
                    if line_stops(&line) {
                        finish_stream(app, scope, &acc);
                        emit_done!("completed");
                        return Ok(());
                    }
                }
            }
        }
    }
}

/* =========================================================
 *  工具调用累积（M2.3 协议层，纯解析不执行）
 * ========================================================= */

/// 单个工具调用的流式分片累积（id/name 首片到达，arguments 逐片追加）
#[derive(Default, Debug)]
struct ToolCallAcc {
    id: String,
    name: String,
    arguments: String,
}

/// 一轮响应的累积状态
#[derive(Default)]
struct StreamAcc {
    /// delta.tool_calls 按协议 index 累积
    tool_calls: std::collections::HashMap<usize, ToolCallAcc>,
    /// choices[0].finish_reason（如 "tool_calls" / "stop"）
    finish_reason: Option<String>,
}

/// `ai-tool-calls` 事件载荷：透传给前端的工具提案
#[derive(Serialize)]
struct ToolCallOut {
    id: String,
    name: String,
    arguments: String,
}

/// 流自然结束（[DONE] 或对端关闭）时调用：若模型请求了工具调用，
/// 按协议 index 顺序发 `ai-tool-calls` 事件。后端不执行、不裁决——
/// 提案持久化、用户批准、tool 结果回传均由前端驱动（设计 §6.2 前端循环）。
fn finish_stream(app: &AppHandle, scope: &str, acc: &StreamAcc) {
    if acc.tool_calls.is_empty() {
        return;
    }
    // 宽松处理：个别实现不给 finish_reason=tool_calls，有累积分片即视为提案
    let mut idxs: Vec<usize> = acc.tool_calls.keys().copied().collect();
    idxs.sort_unstable();
    let calls: Vec<ToolCallOut> = idxs
        .iter()
        .filter_map(|i| acc.tool_calls.get(i))
        .map(|tc| ToolCallOut {
            id: tc.id.clone(),
            name: tc.name.clone(),
            arguments: tc.arguments.clone(),
        })
        .collect();
    if !calls.is_empty() {
        let _ = app.emit(
            "ai-tool-calls",
            serde_json::json!({ "scope": scope, "calls": calls }),
        );
    }
}

/// 判断 SSE 行是否为结束标记 `data: [DONE]`
fn line_stops(line: &[u8]) -> bool {
    let text = String::from_utf8_lossy(line);
    let payload = text.trim().strip_prefix("data:").unwrap_or("").trim();
    payload == "[DONE]"
}

/// 解析单行 SSE：`data: {json}` → 提取 delta.content / delta.reasoning_content 并推送，
/// delta.tool_calls 分片累积进 acc，finish_reason 记录待流结束时裁决（M2.3）
fn handle_sse_line(
    app: &AppHandle,
    scope: &str,
    line: &[u8],
    acc: &mut StreamAcc,
) -> Result<(), String> {
    let text = String::from_utf8_lossy(line);
    let Some(payload) = text.trim().strip_prefix("data:") else {
        return Ok(()); // 空行 / 注释行 / event: 行，忽略
    };
    let payload = payload.trim();
    if payload.is_empty() || payload == "[DONE]" {
        return Ok(());
    }
    let v: serde_json::Value = match serde_json::from_str(payload) {
        Ok(v) => v,
        Err(_) => return Ok(()), // 个别非 JSON 心跳行不致命
    };
    // 上游错误帧（HTTP 200 但体内报错）
    if let Some(msg) = v.get("error").and_then(|e| {
        e.get("message")
            .and_then(|m| m.as_str())
            .or_else(|| e.as_str())
    }) {
        let _ = app.emit(
            "ai-error",
            serde_json::json!({ "scope": scope, "message": msg }),
        );
        return Err(msg.to_string());
    }
    if let Some(fr) = v
        .pointer("/choices/0/finish_reason")
        .and_then(|f| f.as_str())
    {
        acc.finish_reason = Some(fr.to_string());
    }
    let Some(delta) = v
        .pointer("/choices/0/delta")
        .and_then(|d| d.as_object())
    else {
        return Ok(());
    };
    if let Some(c) = delta.get("content").and_then(|c| c.as_str()) {
        if !c.is_empty() {
            let _ = app.emit(
                "ai-token",
                serde_json::json!({ "scope": scope, "kind": "content", "delta": c }),
            );
        }
    }
    // 思考过程：DeepSeek/部分 OpenAI 兼容层用 reasoning_content，其他实现用 reasoning
    if let Some(r) = delta
        .get("reasoning_content")
        .or_else(|| delta.get("reasoning"))
        .and_then(|r| r.as_str())
    {
        if !r.is_empty() {
            let _ = app.emit(
                "ai-token",
                serde_json::json!({ "scope": scope, "kind": "reasoning", "delta": r }),
            );
        }
    }
    // 工具调用分片：首片带 id/name，arguments 按 index 逐片追加（OpenAI 流式协议）
    if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
        for tc in tcs {
            acc_tool_call(acc, tc);
        }
    }
    Ok(())
}

/// 累积单个 tool_call 分片（独立函数便于单元测试）
fn acc_tool_call(acc: &mut StreamAcc, tc: &serde_json::Value) {
    let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
    let slot = acc.tool_calls.entry(idx).or_default();
    if let Some(id) = tc.get("id").and_then(|x| x.as_str()) {
        slot.id = id.to_string();
    }
    if let Some(name) = tc.pointer("/function/name").and_then(|x| x.as_str()) {
        slot.name = name.to_string();
    }
    if let Some(args) = tc
        .pointer("/function/arguments")
        .and_then(|x| x.as_str())
    {
        slot.arguments.push_str(args);
    }
}

/* =========================================================
 *  测试连接
 * ========================================================= */

/// `GET {baseUrl}/models` 探活结果
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiTestResult {
    latency_ms: u64,
    status: u16,
    model_count: usize,
    /// 仅回传前 20 个模型 id，供 UI 展示
    models: Vec<String>,
}

/// 测试连接：使用设置面板草稿值（可能尚未保存）请求模型列表。
/// `api_key` 为空串/None 时依次回退：钥匙串 → 已保存明文。
#[tauri::command]
pub async fn ai_test_connection(
    app: AppHandle,
    base_url: String,
    api_key: Option<String>,
) -> Result<AiTestResult, String> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("未配置接口地址 Base URL".to_string());
    }
    let key = resolve_api_key(&app, api_key.as_deref())?;

    let client = http_client()?;
    let mut req = client.get(format!("{base}/models"));
    if let Some(k) = &key {
        req = req.bearer_auth(k);
    }

    let started = Instant::now();
    let resp = timeout(std::time::Duration::from_secs(10), req.send())
        .await
        .map_err(|_| "连接超时（10s）".to_string())?
        .map_err(|e| format!("请求失败: {e}"))?;
    let latency_ms = started.elapsed().as_millis() as u64;
    let status = resp.status();

    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let snippet: String = body.chars().take(300).collect();
        return Err(format!("HTTP {status}: {snippet}"));
    }

    let data: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析模型列表失败: {e}"))?;
    let models: Vec<String> = data
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let model_count = models.len();
    Ok(AiTestResult {
        latency_ms,
        status: status.as_u16(),
        model_count,
        models: models.into_iter().take(20).collect(),
    })
}

/* =========================================================
 *  API Key 钥匙串
 * ========================================================= */

/// 保存 API Key 到系统钥匙串（空串视为误调用，直接拒绝；"留空不修改"由前端保证不调用）
#[tauri::command]
pub fn ai_set_api_key(key: String) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("API 密钥不能为空".to_string());
    }
    store::keyring_save(AI_KEYRING_ACCOUNT, key)
}

/// 查询钥匙串中是否已保存 API Key（用于设置框 placeholder 文案）
#[tauri::command]
pub fn ai_has_api_key() -> bool {
    store::keyring_get(AI_KEYRING_ACCOUNT)
        .ok()
        .flatten()
        .is_some_and(|k| !k.trim().is_empty())
}

/* =========================================================
 *  会话记录落盘（ai/chats/{scope}.json）
 * ========================================================= */

/// scope 白名单："global" 或 SSH 会话 id（uuid 风格），禁止路径穿越
fn validate_scope(scope: &str) -> Result<(), String> {
    let ok = !scope.is_empty()
        && scope.len() <= 64
        && scope
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    ok.then_some(())
        .ok_or_else(|| format!("非法的会话标识: {scope}"))
}

fn chats_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("获取应用配置目录失败: {e}"))?
        .join("ai")
        .join("chats");
    Ok(dir)
}

fn chat_path(app: &AppHandle, scope: &str) -> Result<PathBuf, String> {
    validate_scope(scope)?;
    Ok(chats_dir(app)?.join(format!("{scope}.json")))
}

/// 保存（覆盖）一条会话记录；record 结构由前端定义，后端原样存取
#[tauri::command]
pub fn ai_save_chat(app: AppHandle, scope: String, record: serde_json::Value) -> Result<(), String> {
    let path = chat_path(&app, &scope)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建会话目录失败: {e}"))?;
    }
    let json = serde_json::to_string_pretty(&record)
        .map_err(|e| format!("序列化会话记录失败: {e}"))?;
    // 原子写：临时文件 + rename，与 connections.json 同一策略
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("写入临时文件失败: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("替换会话文件失败: {e}"))?;
    Ok(())
}

/// 读取一条会话记录；不存在返回 None
#[tauri::command]
pub fn ai_load_chat(app: AppHandle, scope: String) -> Result<Option<serde_json::Value>, String> {
    let path = chat_path(&app, &scope)?;
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("读取会话记录失败: {e}"))?;
    let value = serde_json::from_str(&raw).map_err(|e| format!("解析会话记录失败: {e}"))?;
    Ok(Some(value))
}

/// 删除一条会话记录（清空对话）；文件不存在视为成功
#[tauri::command]
pub fn ai_clear_chat(app: AppHandle, scope: String) -> Result<(), String> {
    let path = chat_path(&app, &scope)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("删除会话记录失败: {e}")),
    }
}

/* =========================================================
 *  滚动摘要（M1.5）
 * ========================================================= */

/// 摘要器指令：合并旧摘要与新对话，输出结构化事实摘要
const SUMMARIZE_PROMPT: &str = "你是对话摘要器。请将用户提供的对话历史合并压缩为一份供后续对话使用的上下文摘要，要求：\n1. 保留用户的意图、目标与关键约束；\n2. 保留已达成的重要结论与决策；\n3. 保留未完成的事项；\n4. 用简洁的中文条目列出，总长不超过 300 字，直接输出摘要内容，不要评论或寒暄。\n若输入中包含「旧摘要」，将新对话内容合并进旧摘要后输出完整摘要。";

/// 非流式响应体（仅取首个 choice 的 message.content）
#[derive(Deserialize)]
struct NonStreamResp {
    choices: Vec<NonStreamChoice>,
}

#[derive(Deserialize)]
struct NonStreamChoice {
    message: NonStreamMessage,
}

#[derive(Deserialize)]
struct NonStreamMessage {
    #[serde(default)]
    content: String,
}

/// 非流式对话调用（滚动摘要/记忆提取共用）：system 指令 + 消息，60s 超时，
/// 取首个 choice 的 message.content；空内容报错
async fn chat_non_stream(
    cfg: &AiCfg,
    sys_prompt: &str,
    messages: Vec<AiMessage>,
) -> Result<String, String> {
    let mut wired: Vec<AiMessage> = Vec::with_capacity(messages.len() + 1);
    wired.push(AiMessage {
        role: "system".into(),
        content: Some(sys_prompt.into()),
        tool_calls: None,
        tool_call_id: None,
    });
    wired.extend(messages);

    let client = http_client()?;
    let url = format!("{}/chat/completions", cfg.base_url);
    let mut req = client.post(&url).json(&ChatRequest {
        model: &cfg.model,
        messages: &wired,
        stream: false,
        tools: None,
    });
    if let Some(key) = &cfg.api_key {
        req = req.bearer_auth(key);
    }

    let resp = timeout(std::time::Duration::from_secs(60), req.send())
        .await
        .map_err(|_| "请求超时（60s）".to_string())?
        .map_err(|e| format!("请求模型服务失败: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let snippet: String = body.chars().take(300).collect();
        return Err(format!("模型服务返回 HTTP {status}: {snippet}"));
    }
    let data: NonStreamResp = resp
        .json()
        .await
        .map_err(|e| format!("解析响应失败: {e}"))?;
    let content = data
        .choices
        .first()
        .map(|c| c.message.content.trim().to_string())
        .unwrap_or_default();
    if content.is_empty() {
        return Err("模型返回了空内容".to_string());
    }
    Ok(content)
}

/// 滚动摘要：把旧摘要与待压缩消息合并为一份新摘要（非流式，一次性返回）。
/// 摘要请求不经过 scope 事件通道，与进行中的对话流互不干扰。
#[tauri::command]
pub async fn ai_summarize(app: AppHandle, messages: Vec<AiMessage>) -> Result<String, String> {
    if messages.is_empty() {
        return Err("摘要输入为空".to_string());
    }
    let cfg = read_ai_settings(&app)?;
    chat_non_stream(&cfg, SUMMARIZE_PROMPT, messages).await
}

/* =========================================================
 *  技能/规则提示资产（M3.2：ai/skills/ 与 ai/rules/）
 * ========================================================= */

/// 单文件注入上限：8KB（约 2k~4k tokens，中英混合），超出截断并标记 truncated
const PROMPT_ASSET_MAX_BYTES: u64 = 8 * 1024;

/// 一条技能/规则文件（目录顶层 .md 平铺，文件名即稳定 key）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptAsset {
    /// 含扩展名的文件名（"nginx.md"），前端禁用名单与列表渲染的稳定 key
    file_name: String,
    /// frontmatter name；缺省时取文件名去扩展
    name: String,
    description: Option<String>,
    /// 截断后的文件全文（含 frontmatter 原文）
    content: String,
    truncated: bool,
    size_bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptAssets {
    skills: Vec<PromptAsset>,
    rules: Vec<PromptAsset>,
}

/// 解析 ai/{skills,rules} 目录；kind 已由命令入口白名单校验
fn prompt_asset_dir(app: &AppHandle, kind: &str) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_config_dir()
        .map_err(|e| format!("获取应用配置目录失败: {e}"))?
        .join("ai")
        .join(kind))
}

/// 极简 frontmatter 解析：`---` 围栏内逐行 `key: value`，仅识别 name/description。
/// 解析失败（无起始/闭合围栏等）按无 frontmatter 处理，不报错。
fn parse_frontmatter(content: &str) -> (Option<String>, Option<String>) {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (None, None);
    }
    let mut name = None;
    let mut description = None;
    for line in lines {
        if line.trim() == "---" {
            return (name, description);
        }
        if let Some((k, v)) = line.split_once(':') {
            let v = v.trim();
            match k.trim() {
                "name" if !v.is_empty() => name = Some(v.to_string()),
                "description" if !v.is_empty() => description = Some(v.to_string()),
                _ => {}
            }
        }
    }
    // 无闭合围栏：按无 frontmatter 处理
    (None, None)
}

/// 读取单个文件：> 8KB 只读前 8KB 并标记截断；UTF-8 解码容错；失败返回 None（跳过）
fn read_prompt_asset(path: &Path, file_name: &str) -> Option<PromptAsset> {
    use std::io::Read;
    let size = std::fs::metadata(path).ok()?.len();
    let truncated = size > PROMPT_ASSET_MAX_BYTES;
    let mut buf = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(PROMPT_ASSET_MAX_BYTES)
        .read_to_end(&mut buf)
        .ok()?;
    let content = String::from_utf8_lossy(&buf).into_owned();
    let (fm_name, fm_desc) = parse_frontmatter(&content);
    // 文件名已确认以 ".md" 结尾（小写比较），去掉末 3 字节即扩展名
    let stem = &file_name[..file_name.len().saturating_sub(3)];
    Some(PromptAsset {
        file_name: file_name.to_string(),
        name: fm_name.unwrap_or_else(|| stem.to_string()),
        description: fm_desc,
        content,
        truncated,
        size_bytes: size,
    })
}

/// 扫描单个目录：仅顶层 .md，跳过隐藏文件/子目录/符号链接，按文件名字典序。
/// 目录不存在/不可读返回空 Vec（失败无害，对话主流程不受影响）。
fn scan_prompt_dir(dir: &Path) -> Vec<PromptAsset> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in rd.flatten() {
        // file_type 不跟随符号链接，可正确识别并跳过 symlink
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_file() {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if file_name.starts_with('.') || !file_name.to_ascii_lowercase().ends_with(".md") {
            continue;
        }
        if let Some(asset) = read_prompt_asset(&entry.path(), &file_name) {
            out.push(asset);
        }
    }
    out.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    out
}

/// 列出技能/规则提示资产。无参数：目录由后端固定解析，前端不传路径，无路径穿越面；
/// 禁用过滤在前端完成，此处始终返回全量清单（设置界面需要完整列表渲染开关）。
/// 首次扫描自动创建目录；目录/单文件失败均降级为空，不报错。
#[tauri::command]
pub fn ai_list_prompt_assets(app: AppHandle) -> Result<PromptAssets, String> {
    crate::slow_span!("ai_list_prompt_assets");
    let skills_dir = prompt_asset_dir(&app, "skills")?;
    let rules_dir = prompt_asset_dir(&app, "rules")?;
    let _ = std::fs::create_dir_all(&skills_dir);
    let _ = std::fs::create_dir_all(&rules_dir);
    Ok(PromptAssets {
        skills: scan_prompt_dir(&skills_dir),
        rules: scan_prompt_dir(&rules_dir),
    })
}

/// 系统文件管理器打开技能/规则目录（首次调用自动创建），返回实际目录的绝对路径
#[tauri::command]
pub fn ai_reveal_prompt_dir(app: AppHandle, kind: String) -> Result<String, String> {
    crate::slow_span!("ai_reveal_prompt_dir");
    let kind = kind.trim();
    if kind != "skills" && kind != "rules" {
        return Err(format!("非法的目录类型: {kind}"));
    }
    let dir = prompt_asset_dir(&app, kind)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败 {}：{e}", dir.display()))?;
    tauri_plugin_opener::open_path(&dir, None::<&str>).map_err(|e| e.to_string())?;
    Ok(dir.display().to_string())
}

/* =========================================================
 *  跨会话记忆（M3.3：ai/memory/memory.json）
 * ========================================================= */

/// 单条记忆内容上限：200 字符（约 100 tokens，足够一条偏好/事实描述）；
/// 前端保存前已截断，此处为写入兜底
const MEMORY_CONTENT_MAX_CHARS: usize = 200;

/// 记忆条数上限的默认值与可调范围（对应 settings.aiMemoryLimit）
const MEMORY_LIMIT_DEFAULT: usize = 100;
const MEMORY_LIMIT_MIN: usize = 10;
const MEMORY_LIMIT_MAX: usize = 1000;

/// 一条跨会话记忆（全局存储、全局注入；source_scope 仅记录提取来源供展示）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryItem {
    /// 唯一标识（前端生成，提取合并时保留）
    id: String,
    /// 记忆内容（≤ 200 字符）
    content: String,
    /// 创建时间（epoch ms）
    created_at: u64,
    /// 提取时的对话归属（global | SSH 会话 id），仅展示用
    source_scope: String,
}

fn memory_file(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_config_dir()
        .map_err(|e| format!("获取应用配置目录失败: {e}"))?
        .join("ai")
        .join("memory")
        .join("memory.json"))
}

/// 读取记忆文件；不存在/解析失败均返回空 Vec（失败无害，对话主流程不受影响）
fn load_memories(path: &Path) -> Vec<MemoryItem> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&raw).unwrap_or_else(|e| {
        log::warn!("memory.json 解析失败，按空记忆处理: {e}");
        Vec::new()
    })
}

/// 记忆条数上限：读 settings.aiMemoryLimit，缺省/越界/读取失败回退默认
fn memory_limit(app: &AppHandle) -> usize {
    let Ok(path) = app_config_file(app) else {
        return MEMORY_LIMIT_DEFAULT;
    };
    persisted::load_full(&path)
        .settings
        .get("aiMemoryLimit")
        .and_then(|v| v.as_u64())
        .map(|n| (n as usize).clamp(MEMORY_LIMIT_MIN, MEMORY_LIMIT_MAX))
        .unwrap_or(MEMORY_LIMIT_DEFAULT)
}

/// 原子写记忆文件（tmp + rename，对齐 ai_save_chat）；写入前裁剪：
/// 单条内容截断至 200 字符，总数超 limit 从头部淘汰最旧
fn save_memories_file(path: &Path, items: &mut Vec<MemoryItem>, limit: usize) -> Result<(), String> {
    for it in items.iter_mut() {
        if it.content.chars().count() > MEMORY_CONTENT_MAX_CHARS {
            it.content = it.content.chars().take(MEMORY_CONTENT_MAX_CHARS).collect();
        }
    }
    if items.len() > limit {
        let overflow = items.len() - limit;
        items.drain(..overflow);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建记忆目录失败: {e}"))?;
    }
    let json = serde_json::to_string_pretty(items).map_err(|e| format!("序列化记忆失败: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("写入临时文件失败: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("替换记忆文件失败: {e}"))?;
    Ok(())
}

/// 删除记忆文件；不存在视为成功
fn clear_memories_file(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("删除记忆文件失败: {e}")),
    }
}

/// 读取全部记忆条目。无参数：文件由后端固定解析，前端不传路径，无路径穿越面
#[tauri::command]
pub fn ai_list_memories(app: AppHandle) -> Result<Vec<MemoryItem>, String> {
    crate::slow_span!("ai_list_memories");
    Ok(load_memories(&memory_file(&app)?))
}

/// 覆盖写入记忆条目（含单条 200 字符截断与设置上限淘汰最旧）
#[tauri::command]
pub fn ai_save_memories(app: AppHandle, mut items: Vec<MemoryItem>) -> Result<(), String> {
    crate::slow_span!("ai_save_memories");
    let limit = memory_limit(&app);
    save_memories_file(&memory_file(&app)?, &mut items, limit)
}

/// 清空记忆；文件不存在视为成功
#[tauri::command]
pub fn ai_clear_memories(app: AppHandle) -> Result<(), String> {
    crate::slow_span!("ai_clear_memories");
    clear_memories_file(&memory_file(&app)?)
}

/// 记忆提取指令：从对话历史提取跨会话有价值的用户偏好/项目事实，输出 JSON 数组
const EXTRACT_MEMORIES_PROMPT: &str = "你是记忆提取器。从对话历史中提取值得长期记住的用户偏好、项目上下文、重要事实。要求：\n1. 只提取跨会话有价值的信息（用户偏好、项目约定、常用环境、重要决策）；\n2. 不提取一次性问题、闲聊、已解决的临时故障；\n3. 不记录密码、私钥、API Key、Token 等敏感凭据；\n4. 每条记忆为简洁中文陈述句，不超过 200 字符；\n5. 输出为 JSON 数组，每项为含 content 字段的对象；若无可提取内容，输出 []；\n6. 直接输出 JSON，不要包裹 markdown 代码块，不要评论。\n若输入中包含「已有记忆」，将新提取的内容与已有记忆合并去重后输出完整数组。";

/// 记忆提取：非流式调用，返回模型输出的 JSON 字符串。
/// 解析/校验/合并/生成 id 在前端完成——模型输出可能不完美，前端容错，
/// 后端不做结构假设。与对话流互不干扰（同 ai_summarize 通道）。
#[tauri::command]
pub async fn ai_extract_memories(app: AppHandle, messages: Vec<AiMessage>) -> Result<String, String> {
    if messages.is_empty() {
        return Err("提取输入为空".to_string());
    }
    let cfg = read_ai_settings(&app)?;
    chat_non_stream(&cfg, EXTRACT_MEMORIES_PROMPT, messages).await
}

/* =========================================================
 *  HTTP 客户端（共享连接池）
 * ========================================================= */

/// 全局共享 reqwest 客户端：连接池/TLS 会话复用，避免每轮对话重新建连
fn http_client() -> Result<reqwest::Client, String> {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    if let Some(c) = CLIENT.get() {
        return Ok(c.clone());
    }
    let client = reqwest::Client::builder()
        .user_agent(concat!("rhost/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(8))
        .pool_idle_timeout(std::time::Duration::from_secs(90))
        .build()
        .map_err(|e| format!("初始化 HTTP 客户端失败: {e}"))?;
    // 并发首次调用时可能重复构建，丢弃失败者无害（Client 构造无副作用）
    Ok(CLIENT.get_or_init(|| client).clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_validation() {
        assert!(validate_scope("global").is_ok());
        assert!(validate_scope("a1b2c3d4-1234").is_ok());
        assert!(validate_scope("").is_err());
        assert!(validate_scope("../etc/passwd").is_err());
        assert!(validate_scope("a/b").is_err());
        assert!(validate_scope("a.b").is_err());
        assert!(validate_scope(&"x".repeat(65)).is_err());
    }

    #[test]
    fn sse_done_detection() {
        assert!(line_stops(b"data: [DONE]\n"));
        assert!(line_stops(b"data:[DONE] \r\n"));
        assert!(!line_stops(b"data: {}\n"));
        assert!(!line_stops(b"\n"));
    }

    #[test]
    fn tool_call_fragment_accumulation() {
        let mut acc = StreamAcc::default();
        // 首片：id + name + arguments 首段
        acc_tool_call(&mut acc, &serde_json::json!({
            "index": 0, "id": "call_1", "type": "function",
            "function": { "name": "propose_command", "arguments": "{\"command\":\"ls" }
        }));
        // 次片：仅 arguments 追加
        acc_tool_call(&mut acc, &serde_json::json!({
            "index": 0, "function": { "arguments": " -la\"}" }
        }));
        // 第二个调用独立累积
        acc_tool_call(&mut acc, &serde_json::json!({
            "index": 1, "id": "call_2", "type": "function",
            "function": { "name": "propose_command", "arguments": "{}" }
        }));
        let c0 = &acc.tool_calls[&0];
        assert_eq!(c0.id, "call_1");
        assert_eq!(c0.name, "propose_command");
        assert_eq!(c0.arguments, "{\"command\":\"ls -la\"}");
        assert_eq!(acc.tool_calls[&1].id, "call_2");
        assert_eq!(acc.finish_reason, None);
    }

    #[test]
    fn builtin_tools_shape() {
        let v = builtin_tools();
        let arr = v.as_array().expect("tools 应为数组");
        assert_eq!(arr.len(), 6);
        let names: Vec<&str> = arr
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec![
                "read_file",
                "list_directory",
                "get_system_info",
                "edit_file",
                "write_file",
                "propose_command"
            ]
        );
        // propose_command 为最后一个，需含 command 参数
        assert_eq!(arr[5]["function"]["parameters"]["required"][0], "command");
        // read_file / list_directory 需 path 参数
        assert_eq!(arr[0]["function"]["parameters"]["required"][0], "path");
        assert_eq!(arr[1]["function"]["parameters"]["required"][0], "path");
        // edit_file 需 path/old_text/new_text
        assert_eq!(arr[3]["function"]["parameters"]["required"].as_array().unwrap().len(), 3);
        // write_file 需 path/content
        assert_eq!(arr[4]["function"]["parameters"]["required"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn build_tool_command_mapping() {
        // read_file
        let cmd = build_tool_command("read_file", r#"{"path":"/etc/hosts"}"#).unwrap();
        assert!(cmd.contains("head -c 8192"));
        assert!(cmd.contains("'/etc/hosts'"));
        // list_directory
        let cmd = build_tool_command("list_directory", r#"{"path":"/var/log"}"#).unwrap();
        assert!(cmd.starts_with("ls -la"));
        assert!(cmd.contains("'/var/log'"));
        // get_system_info 无参数
        let cmd = build_tool_command("get_system_info", "{}").unwrap();
        assert!(cmd.contains("uname"));
        // edit_file：含 perl 替换 + 备份
        let cmd = build_tool_command(
            "edit_file",
            r#"{"path":"/tmp/a","old_text":"foo","new_text":"bar"}"#
        )
        .unwrap();
        assert!(cmd.contains("perl -0pi"));
        assert!(cmd.contains("\\Qfoo\\E"));
        assert!(cmd.contains("bar"));
        assert!(cmd.contains(".bak.$(date +%s)"));
        // write_file：含 heredoc + 备份
        let cmd = build_tool_command("write_file", r#"{"path":"/tmp/b","content":"hello"}"#).unwrap();
        assert!(cmd.contains("cat > '/tmp/b'"));
        assert!(cmd.contains("RHOST_EOF"));
        assert!(cmd.contains("hello"));
        // 路径黑名单拒绝
        assert!(build_tool_command("write_file", r#"{"path":"/etc/passwd","content":"x"}"#).is_err());
        // 未知工具
        assert!(build_tool_command("rm_rf", "{}").is_err());
        // 缺少参数
        assert!(build_tool_command("read_file", "{}").is_err());
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("/etc/hosts"), "'/etc/hosts'");
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn cap_output_truncates_at_8kb() {
        let short = "hello".repeat(10);
        let (truncated, _) = cap_output(&short);
        assert!(!truncated);
        // 8192 字节边界
        let long = "a".repeat(9000);
        let (truncated, capped) = cap_output(&long);
        assert!(truncated);
        assert_eq!(capped.len(), 8192);
        // 多字节字符边界
        let utf8 = "你".repeat(3000); // 每个 3 字节 = 9000 字节
        let (truncated, capped) = cap_output(&utf8);
        assert!(truncated);
        assert!(capped.len() <= 8192);
        assert!(capped.chars().all(|c| c == '你')); // 不拆字
    }

    #[test]
    fn ai_message_tool_wire_roundtrip() {
        // assistant tool_calls 消息（content null）与 tool 结果消息序列化往返
        let msgs: Vec<AiMessage> = serde_json::from_str(
            r#"[
            {"role":"assistant","content":null,"tool_calls":[
                {"id":"call_1","type":"function",
                 "function":{"name":"propose_command","arguments":"{\"command\":\"ls\"}"}}
            ]},
            {"role":"tool","tool_call_id":"call_1","content":"已执行"}
        ]"#,
        )
        .expect("反序列化失败");
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].content, None);
        assert_eq!(msgs[0].tool_calls.as_ref().unwrap()[0].id, "call_1");
        assert_eq!(msgs[1].tool_call_id.as_deref(), Some("call_1"));
        // 再序列化：skip_serializing_if 保证 None 字段不出现
        let back = serde_json::to_value(&msgs).unwrap();
        assert!(back[1].get("tool_calls").is_none());
        assert_eq!(back[1]["tool_call_id"], "call_1");
    }

    /* -------- M3.2 技能/规则提示资产 -------- */

    #[test]
    fn frontmatter_parse() {
        // 标准 frontmatter
        let (n, d) = parse_frontmatter("---\nname: nginx 排障\ndescription: 502 排查\n---\n\n正文");
        assert_eq!(n.as_deref(), Some("nginx 排障"));
        assert_eq!(d.as_deref(), Some("502 排查"));
        // 无 frontmatter
        let (n, d) = parse_frontmatter("# 标题\n正文");
        assert_eq!((n, d), (None, None));
        // 无闭合围栏 → 按无 frontmatter 处理
        let (n, d) = parse_frontmatter("---\nname: x\n正文没有闭合");
        assert_eq!((n, d), (None, None));
        // 未知键忽略；空值忽略
        let (n, d) = parse_frontmatter("---\ntags: a,b\ndescription:\nname: x\n---\n");
        assert_eq!(n.as_deref(), Some("x"));
        assert_eq!(d, None);
        // BOM 容错
        let (n, _) = parse_frontmatter("\u{feff}---\nname: bom\n---\n");
        assert_eq!(n.as_deref(), Some("bom"));
    }

    #[test]
    fn prompt_dir_scan_filters_and_sorts() {
        let dir = std::env::temp_dir().join(format!("rhost-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        // 合法文件（乱序写入，验证排序）
        std::fs::write(dir.join("b-two.md"), "二").unwrap();
        std::fs::write(dir.join("a-one.md"), "---\nname: 一\n---\n一").unwrap();
        // 应被跳过：隐藏文件、非 .md、子目录
        std::fs::write(dir.join(".hidden.md"), "x").unwrap();
        std::fs::write(dir.join("note.txt"), "x").unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        // 符号链接（指向目录外目标）应被跳过
        let outside = std::env::temp_dir().join(format!("rhost-out-{}", uuid::Uuid::new_v4()));
        std::fs::write(&outside, "外部内容").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, dir.join("link.md")).unwrap();

        let assets = scan_prompt_dir(&dir);
        let names: Vec<&str> = assets.iter().map(|a| a.file_name.as_str()).collect();
        assert_eq!(names, ["a-one.md", "b-two.md"]);
        assert_eq!(assets[0].name, "一"); // frontmatter name 优先
        assert_eq!(assets[1].name, "b-two"); // 缺省回退文件名去扩展
        assert_eq!(assets[0].description, None);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&outside);
    }

    #[test]
    fn prompt_asset_truncation() {
        let dir = std::env::temp_dir().join(format!("rhost-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let big = "x".repeat((PROMPT_ASSET_MAX_BYTES + 100) as usize);
        let path = dir.join("big.md");
        std::fs::write(&path, &big).unwrap();

        let asset = read_prompt_asset(&path, "big.md").unwrap();
        assert!(asset.truncated);
        assert_eq!(asset.content.len(), PROMPT_ASSET_MAX_BYTES as usize);
        assert_eq!(asset.size_bytes, PROMPT_ASSET_MAX_BYTES + 100);

        // 目录不存在 → 空列表（失败无害）
        assert!(scan_prompt_dir(&dir.join("nonexistent")).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /* -------- M3.3 跨会话记忆 -------- */

    fn mem(id: &str, content: &str, created_at: u64) -> MemoryItem {
        MemoryItem {
            id: id.into(),
            content: content.into(),
            created_at,
            source_scope: "global".into(),
        }
    }

    #[test]
    fn memory_save_truncates_and_evicts_oldest() {
        let dir = std::env::temp_dir().join(format!("rhost-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("memory.json");
        let long = "x".repeat(MEMORY_CONTENT_MAX_CHARS + 50);

        // limit=2：淘汰最旧的一条（a），保留 b/c
        let mut items = vec![mem("a", "一", 1), mem("b", "二", 2), mem("c", "三", 3)];
        save_memories_file(&path, &mut items, 2).unwrap();
        let loaded = load_memories(&path);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, "b");
        assert_eq!(loaded[1].id, "c");

        // 单条内容超 200 字符被截断
        let mut items = vec![mem("a", &long, 1)];
        save_memories_file(&path, &mut items, 10).unwrap();
        let loaded = load_memories(&path);
        assert_eq!(loaded[0].content.chars().count(), MEMORY_CONTENT_MAX_CHARS);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn memory_load_degrades_on_missing_or_invalid() {
        let dir = std::env::temp_dir().join(format!("rhost-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("memory.json");
        // 不存在 → 空
        assert!(load_memories(&path).is_empty());
        // 非法 JSON → 空（fail-open）
        std::fs::write(&path, "{not json").unwrap();
        assert!(load_memories(&path).is_empty());
        // 结构不符 → 空
        std::fs::write(&path, r#"{"a":1}"#).unwrap();
        assert!(load_memories(&path).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn memory_clear_idempotent() {
        let dir = std::env::temp_dir().join(format!("rhost-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("memory.json");
        // 不存在时删除不报错
        clear_memories_file(&path).unwrap();
        save_memories_file(&path, &mut vec![mem("a", "一", 1)], 10).unwrap();
        clear_memories_file(&path).unwrap();
        assert!(load_memories(&path).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
