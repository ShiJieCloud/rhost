//! 本地文件系统列举：供 SFTP 面板"本地"栏展示真实目录内容。

use serde::Serialize;

/// 单个目录项（字段与前端 FsEntry 对应）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalFsEntry {
    pub name: String,
    pub is_dir: bool,
    /// 是否为符号链接（file_type 不跟随；is_dir 经 metadata 跟随判断）
    pub is_symlink: bool,
    /// 文件字节数；目录恒为 0（前端显示 —）
    pub size: u64,
    /// 本地时间 "YYYY-MM-DD HH:MM"；取不到为空串
    pub mtime: String,
    /// 权限字符串 "rwxr-xr-x"（不输出八进制）
    pub perm: String,
    /// 属主/属组 "user/group"
    pub owner: String,
}

/// 一次目录列举结果：带回实际路径（path 缺省时解析为 home）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalDirListing {
    pub path: String,
    pub entries: Vec<LocalFsEntry>,
}

/// 解析 home 目录（不引入 dirs crate：Unix 读 HOME，Windows 读 USERPROFILE）
fn home_dir() -> Option<String> {
    std::env::var("HOME")
        .ok()
        .or_else(|| std::env::var("USERPROFILE").ok())
        .filter(|s| !s.is_empty())
}

fn fmt_mtime(md: &std::fs::Metadata) -> String {
    match md.modified() {
        Ok(t) => {
            let dt: chrono::DateTime<chrono::Local> = t.into();
            dt.format("%Y-%m-%d %H:%M").to_string()
        }
        Err(_) => String::new(),
    }
}

/// 权限位 → "rwxr-xr-x"（含 setuid/setgid/sticky 的 s/S/t/T 惯例）
#[cfg(unix)]
fn fmt_perm(md: &std::fs::Metadata) -> String {
    use std::os::unix::fs::PermissionsExt;
    let mode = md.permissions().mode();
    let mut s = String::with_capacity(9);
    for (bit, chr) in [
        (0o400, 'r'), (0o200, 'w'), (0o100, 'x'),
        (0o040, 'r'), (0o020, 'w'), (0o010, 'x'),
        (0o004, 'r'), (0o002, 'w'), (0o001, 'x'),
    ] {
        s.push(if mode & bit != 0 { chr } else { '-' });
    }
    if mode & 0o4000 != 0 {
        let i = 2; // user x 位
        s.replace_range(i..=i, if mode & 0o100 != 0 { "s" } else { "S" });
    }
    if mode & 0o2000 != 0 {
        let i = 5; // group x 位
        s.replace_range(i..=i, if mode & 0o010 != 0 { "s" } else { "S" });
    }
    if mode & 0o1000 != 0 {
        let i = 8; // other x 位
        s.replace_range(i..=i, if mode & 0o001 != 0 { "t" } else { "T" });
    }
    s
}

#[cfg(not(unix))]
fn fmt_perm(_md: &std::fs::Metadata) -> String {
    "---------".to_string()
}

/// uid/gid → "user/group"。libc 反查，同目录大量条目属主重复，用缓存避免重复查询。
#[cfg(unix)]
fn fmt_owner(
    md: &std::fs::Metadata,
    uid_cache: &mut std::collections::HashMap<u32, String>,
    gid_cache: &mut std::collections::HashMap<u32, String>,
) -> String {
    use std::os::unix::fs::MetadataExt;
    let uid = md.uid();
    let gid = md.gid();
    let user = uid_cache.entry(uid).or_insert_with(|| {
        let p = unsafe { libc::getpwuid(uid) };
        if p.is_null() {
            uid.to_string()
        } else {
            unsafe { std::ffi::CStr::from_ptr((*p).pw_name) }
                .to_string_lossy()
                .into_owned()
        }
    });
    let group = gid_cache.entry(gid).or_insert_with(|| {
        let p = unsafe { libc::getgrgid(gid) };
        if p.is_null() {
            gid.to_string()
        } else {
            unsafe { std::ffi::CStr::from_ptr((*p).gr_name) }
                .to_string_lossy()
                .into_owned()
        }
    });
    format!("{user}/{group}")
}

#[cfg(not(unix))]
fn fmt_owner(
    _md: &std::fs::Metadata,
    _u: &mut std::collections::HashMap<u32, String>,
    _g: &mut std::collections::HashMap<u32, String>,
) -> String {
    String::new()
}

/// 列举本地目录。`path` 为 None/空串时用 home 目录（前端首次打开定位到用户主目录）。
/// 读目录可能耗时（网络盘/大目录），放阻塞线程池避免卡住异步运行时。
#[tauri::command]
pub async fn list_local_dir(path: Option<String>) -> Result<LocalDirListing, String> {
    let dir = match path {
        Some(p) if !p.is_empty() => p,
        _ => home_dir().ok_or_else(|| "无法确定用户主目录".to_string())?,
    };

    tauri::async_runtime::spawn_blocking(move || {
        let rd = std::fs::read_dir(&dir).map_err(|e| format!("无法读取目录 {dir}：{e}"))?;
        let mut entries = Vec::new();
        let mut uid_cache = std::collections::HashMap::new();
        let mut gid_cache = std::collections::HashMap::new();
        for item in rd {
            let Ok(ent) = item else { continue };
            let name = ent.file_name().to_string_lossy().into_owned();
            // file_type() 不跟随符号链接，用于识别软链；
            // metadata() 跟随链接，指向目录的快捷方式按目录处理
            let is_symlink = ent.file_type().map(|t| t.is_symlink()).unwrap_or(false);
            let Ok(md) = ent.metadata() else { continue };
            let is_dir = md.is_dir();
            entries.push(LocalFsEntry {
                name,
                is_dir,
                is_symlink,
                size: if is_dir { 0 } else { md.len() },
                mtime: fmt_mtime(&md),
                perm: fmt_perm(&md),
                owner: fmt_owner(&md, &mut uid_cache, &mut gid_cache),
            });
        }
        Ok(LocalDirListing { path: dir, entries })
    })
    .await
    .map_err(|e| format!("目录列举任务失败：{e}"))?
}

/// 本地创建目录（单层，不递归）。
#[tauri::command]
pub async fn mkdir(path: String) -> Result<(), String> {
    tokio::fs::create_dir(&path)
        .await
        .map_err(|e| format!("创建目录 {path} 失败：{e}"))
}

/// 本地删除：文件/符号链接 → remove_file（软链只删链接本身，不触碰目标）；
/// 目录 → remove_dir_all 递归删除。用 symlink_metadata 判类型，避免跟随软链。
#[tauri::command]
pub async fn local_remove(path: String) -> Result<(), String> {
    let md = tokio::fs::symlink_metadata(&path)
        .await
        .map_err(|e| format!("获取 {path} 属性失败：{e}"))?;
    if md.is_dir() {
        tokio::fs::remove_dir_all(&path)
            .await
            .map_err(|e| format!("删除目录 {path} 失败：{e}"))
    } else {
        // 普通文件与符号链接均走 remove_file
        tokio::fs::remove_file(&path)
            .await
            .map_err(|e| format!("删除 {path} 失败：{e}"))
    }
}

/// 本地重命名/移动（同盘原子；跨卷由系统执行复制+删除）。
#[tauri::command]
pub async fn local_rename(old_path: String, new_path: String) -> Result<(), String> {
    tokio::fs::rename(&old_path, &new_path)
        .await
        .map_err(|e| format!("重命名 {old_path} → {new_path} 失败：{e}"))
}

/// 本地创建零字节空文件（目标已存在时报错，不截断）。
#[tauri::command]
pub async fn local_create_file(path: String) -> Result<(), String> {
    tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .await
        .map(|_| ())
        .map_err(|e| format!("创建文件 {path} 失败：{e}"))
}

/// 本地递归复制：符号链接复制为同指向软链（不跟随）；目录递归；
/// 文件走 fs::copy。dst 的父目录需已存在（前端按目录树拼接）。
#[tauri::command]
pub async fn local_copy(src: String, dst: String) -> Result<(), String> {
    async fn rec(src: &str, dst: &str) -> Result<(), String> {
        let md = tokio::fs::symlink_metadata(src)
            .await
            .map_err(|e| format!("获取 {src} 属性失败：{e}"))?;
        let ftype = md.file_type();
        if ftype.is_symlink() {
            let target = tokio::fs::read_link(src)
                .await
                .map_err(|e| format!("读取链接 {src} 失败：{e}"))?;
            #[cfg(unix)]
            tokio::fs::symlink(&target, dst)
                .await
                .map_err(|e| format!("创建链接 {dst} 失败：{e}"))?;
            #[cfg(not(unix))]
            let _ = target;
        } else if md.is_dir() {
            tokio::fs::create_dir(dst)
                .await
                .map_err(|e| format!("创建目录 {dst} 失败：{e}"))?;
            let mut rd = tokio::fs::read_dir(src)
                .await
                .map_err(|e| format!("读取目录 {src} 失败：{e}"))?;
            while let Some(ent) = rd
                .next_entry()
                .await
                .map_err(|e| format!("遍历目录 {src} 失败：{e}"))?
            {
                let name = ent.file_name().to_string_lossy().into_owned();
                let s = if src == "/" {
                    format!("/{name}")
                } else {
                    format!("{src}/{name}")
                };
                let d = if dst == "/" {
                    format!("/{name}")
                } else {
                    format!("{dst}/{name}")
                };
                Box::pin(rec(&s, &d)).await?;
            }
        } else {
            tokio::fs::copy(src, dst)
                .await
                .map_err(|e| format!("复制 {src} → {dst} 失败：{e}"))?;
        }
        Ok(())
    }
    rec(&src, &dst).await
}

/// 在系统终端中打开指定目录（本地栏"在终端打开"，与远程栏菜单对等）。
/// 传入文件时打开其所在目录。进程 spawn 放阻塞线程池。
#[tauri::command]
pub async fn local_open_terminal(path: String) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    let dir = if p.is_dir() {
        p.to_path_buf()
    } else {
        p.parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf()
    };
    tauri::async_runtime::spawn_blocking(move || open_terminal(&dir))
        .await
        .map_err(|e| format!("终端任务失败：{e}"))?
}

/// macOS：`open -a Terminal <dir>`
#[cfg(target_os = "macos")]
fn open_terminal(dir: &std::path::Path) -> Result<(), String> {
    std::process::Command::new("open")
        .arg("-a")
        .arg("Terminal")
        .arg(dir)
        .spawn()
        .map_err(|e| format!("打开终端失败：{e}"))?;
    Ok(())
}

/// Linux：依次尝试常见终端模拟器；不支持工作目录参数的用进程 CWD 定位
#[cfg(target_os = "linux")]
fn open_terminal(dir: &std::path::Path) -> Result<(), String> {
    let attempts: &[(&str, &[&str])] = &[
        ("x-terminal-emulator", &[]),
        ("gnome-terminal", &["--working-directory"]),
        ("konsole", &["--workdir"]),
        ("xfce4-terminal", &["--working-directory"]),
        ("xterm", &[]),
    ];
    let mut last_err = String::new();
    for (bin, args) in attempts {
        let mut cmd = std::process::Command::new(bin);
        if args.is_empty() {
            cmd.current_dir(dir);
        } else {
            cmd.args(args.iter().copied()).arg(dir);
        }
        match cmd.spawn() {
            Ok(_) => return Ok(()),
            Err(e) => {
                last_err = format!("{bin}: {e}");
                continue;
            }
        }
    }
    Err(format!("未找到可用的终端模拟器（{last_err}）"))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn open_terminal(_dir: &std::path::Path) -> Result<(), String> {
    Err("当前平台暂不支持在系统终端打开".to_string())
}

