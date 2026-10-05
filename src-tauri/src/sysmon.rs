//! 终端工具自身资源监控：供底部状态栏展示 Rhost 进程驻留内存（RSS）。
//!
//! 口径说明：
//! - used = Rhost 主进程驻留物理内存（russh 会话/字节缓冲/Tauri 运行时）；
//! - total = 本机物理内存总量，占比即工具对整机内存的压力；
//! - macOS 上 WKWebView 的 WebContent 渲染进程由 launchd 托管（ppid=1），
//!   无法可靠归属到本进程，故不计入（Linux/Windows 同口径，保持跨平台一致）。
//!
//! [`System`] 实例全局复用，每次只刷新「当前进程内存 + 系统内存」，
//! 不枚举全量进程、不取 CPU，1s 级轮询开销可忽略。

use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

/// 工具内存占用快照（字节，前端按 1024 换算 MB）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppMemory {
    /// 工具主进程驻留内存
    pub used_bytes: u64,
    /// 系统物理内存总量
    pub total_bytes: u64,
}

fn system() -> &'static Mutex<System> {
    static SYS: OnceLock<Mutex<System>> = OnceLock::new();
    // 初始不刷新任何内容：进程/系统内存均在轮询时按需精确刷新
    SYS.get_or_init(|| Mutex::new(System::new_with_specifics(RefreshKind::nothing())))
}

/// 采集一次工具内存占用；取不到进程时 used 降级为 0
pub fn app_memory() -> AppMemory {
    let mut sys = system().lock().expect("sysinfo 锁中毒");
    sys.refresh_memory();

    let used_bytes = match sysinfo::get_current_pid() {
        Ok(pid) => {
            sys.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[pid]),
                false,
                ProcessRefreshKind::nothing().with_memory(),
            );
            sys.process(pid).map(|p| p.memory()).unwrap_or(0)
        }
        Err(_) => 0,
    };

    AppMemory {
        used_bytes,
        total_bytes: sys.total_memory(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_memory_returns_real_values() {
        let m = app_memory();
        assert!(m.total_bytes > 0, "系统总内存应大于 0");
        assert!(m.used_bytes > 0, "测试进程自身 RSS 应大于 0");
        assert!(m.used_bytes < m.total_bytes, "进程 RSS 不应超过系统总内存");
    }
}
