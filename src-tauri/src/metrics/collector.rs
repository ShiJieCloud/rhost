//! 动态指标采集器（0x06 帧）：按固定间隔在独立 exec 通道采集 /proc 数据，
//! 与 PTY 流物理隔离。单飞、错峰、可取消；前端用心跳维持，超时自动停。
//!
//! 生命周期：
//! - [`spawn`] 在会话 cancel 下派生 child token，返回 [`CollectorHandle`]；
//! - 前端每 5s 调 `metrics_heartbeat` 续约，超过 [`HEARTBEAT_TTL`] 未续约
//!   （Inspector 隐藏/页面关闭/崩溃）采集任务自行退出；
//! - 会话 `shutdown()` 取消根 token，子任务同步取消，无任务泄漏。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use log::debug;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use crate::metrics::{
    CpuMetricsPayload, CpuTimes, DISK_INTERVAL_ROUNDS, DiskMetrics, GpuItemPayload, MetricsPayload,
    NetCounters, NetMetricsPayload, ProcsPayload, build_procs_payload, byte_rate, cpu_util,
    metrics_script, parse_metrics_output,
};
use crate::ssh::frame::{FrameType, encode_frame};
use crate::ssh::session::SshSession;

/// 单轮采集（含建 exec 通道）超时；与间隔默认值一致，慢主机不堆积
const ROUND_TIMEOUT: Duration = Duration::from_secs(3);
/// 连续失败多少轮后进入慢轮询退避
const FAILURE_BACKOFF_THRESHOLD: u32 = 3;
/// 退避态轮询间隔：故障主机上降低重试频率，继续重试而非放弃，任一轮成功即恢复
const BACKOFF_INTERVAL: Duration = Duration::from_secs(10);

/// 根据连续失败轮数决定下一轮节拍：未达阈值用正常间隔，达到 [`FAILURE_BACKOFF_THRESHOLD`]
/// 降到 [`BACKOFF_INTERVAL`]。纯函数便于单测。
pub(crate) fn round_delay(interval: Duration, consecutive_failures: u32) -> Duration {
    if consecutive_failures >= FAILURE_BACKOFF_THRESHOLD {
        BACKOFF_INTERVAL
    } else {
        interval
    }
}
/// 心跳续约 TTL：前端 5s 一次心跳，9s 无心跳判定无人消费，自动停止
pub(crate) const HEARTBEAT_TTL: Duration = Duration::from_secs(9);

/// 采集任务句柄：由 SshSession 持有，用于续约/停止
pub(crate) struct CollectorHandle {
    cancel: CancellationToken,
    last_heartbeat: Arc<StdMutex<Instant>>,
}

impl CollectorHandle {
    /// 刷新续约时间（前端心跳）
    pub(crate) fn heartbeat(&self) {
        *self.last_heartbeat.lock().unwrap() = Instant::now();
    }

    /// 主动停止采集（重复 start 时先停旧任务，或前端显式 stop）
    pub(crate) fn stop(&self) {
        self.cancel.cancel();
    }
}

/// 启动一个采集任务。`root` 为会话根取消令牌，任务挂其 child token 上。
/// `iface` 为默认路由网卡名（0x05 已采集），网络计数优先取该网卡；None 时汇总非 lo。
pub(crate) fn spawn(
    session: Arc<SshSession>,
    interval: Duration,
    root: CancellationToken,
    frame_tx: mpsc::Sender<Vec<u8>>,
    iface: Option<String>,
) -> CollectorHandle {
    let cancel = root.child_token();
    let last_heartbeat = Arc::new(StdMutex::new(Instant::now()));
    tokio::spawn(run(
        session,
        interval,
        cancel.clone(),
        frame_tx,
        last_heartbeat.clone(),
        iface,
    ));
    CollectorHandle {
        cancel,
        last_heartbeat,
    }
}

/// 采集主循环：立即采首帧 → 等待 interval（无追赶突发），串行天然单飞。
async fn run(
    session: Arc<SshSession>,
    interval: Duration,
    cancel: CancellationToken,
    frame_tx: mpsc::Sender<Vec<u8>>,
    last_heartbeat: Arc<StdMutex<Instant>>,
    iface: Option<String>,
) {
    // 启动错峰 0~300ms：多个会话同时连接时避免 exec 通道齐步走
    let jitter_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::from(d.subsec_nanos() % 300))
        .unwrap_or(0);
    tokio::select! {
        _ = cancel.cancelled() => return,
        _ = sleep(Duration::from_millis(jitter_ms)) => {}
    }

    let mut prev_cpu: Option<CpuTimes> = None;
    // 网卡速率需 (采样时刻, 计数) 二元组做差；墙钟差兼容后台 Tab 节流后的非固定间隔
    let mut prev_net: Option<(Instant, NetCounters)> = None;
    // 磁盘低频夹带：第 0 轮（首帧）立即采，之后每 DISK_INTERVAL_ROUNDS 轮一次；
    // 非夹带轮/df 失败沿用上一次缓存，UI 不闪空
    let mut tick: u32 = 0;
    let mut disks_cache: Vec<DiskMetrics> = Vec::new();
    // 进程 CPU 差值需 per-pid (采样时刻, 累计节拍) 表；每轮以本轮存活进程重建，
    // 退出的进程自然淘汰，无泄漏
    let mut prev_procs: HashMap<u32, (Instant, u64)> = HashMap::new();
    let mut procs_cache: Option<ProcsPayload> = None;
    // GPU 静态探测态：None=尚未探测（首轮乐观带段试一次）；Some(false)=空输出判定无卡，
    // 后续轮次永久不带段（省 nvidia-smi 冷启动开销）；Some(true)=每轮带段。
    // 已确认有卡后单轮空输出视为瞬态失败，沿用缓存、不改判，避免驱动抖动导致区块闪没。
    let mut gpu_supported: Option<bool> = None;
    let mut gpus_cache: Vec<GpuItemPayload> = Vec::new();
    // 连续 exec 失败计数：成功即清零；计满 FAILURE_BACKOFF_THRESHOLD 轮降为慢轮询
    let mut consecutive_failures: u32 = 0;
    let mut seq: u64 = 0;

    loop {
        // 心跳 TTL 检查：无人消费则静默退出（不取消会话本身）
        if last_heartbeat.lock().unwrap().elapsed() > HEARTBEAT_TTL {
            debug!("指标采集心跳超时，自动停止");
            return;
        }

        // 采样时刻取 exec 之前：远端 /proc 读数近似此刻，相邻轮次差值即真实墙钟间隔
        let sampled_at = Instant::now();
        let with_disk = tick.is_multiple_of(DISK_INTERVAL_ROUNDS);
        tick = tick.wrapping_add(1);
        // 未判定无卡前轮轮带段（首轮探测 + 已确认有卡后的常态采集）
        let with_gpu = gpu_supported != Some(false);
        match session
            .exec_collect(&metrics_script(with_disk, with_gpu), ROUND_TIMEOUT)
            .await
        {
            Ok(text) => {
                // exec 成功即解除退避（即使部分 marker 段缺失也算通道健康）
                consecutive_failures = 0;
                let raw = parse_metrics_output(&text, iface.as_deref());
                // CPU 利用率需相邻两次快照差值；首帧（或本轮缺 stat）util 为 0
                let util = match (prev_cpu, raw.cpu) {
                    (Some(p), Some(c)) => cpu_util(p, c),
                    _ => 0.0,
                };
                if raw.cpu.is_some() {
                    prev_cpu = raw.cpu;
                }

                // 网络速率同理：首帧（或本轮缺 netdev）速率为 0，只记录快照不算差
                let mut net = NetMetricsPayload {
                    rx_rate: 0.0,
                    tx_rate: 0.0,
                    rx_bytes: 0,
                    tx_bytes: 0,
                };
                if let Some(cur) = raw.net {
                    if let Some((prev_at, prev)) = prev_net {
                        let dt = sampled_at.saturating_duration_since(prev_at);
                        net.rx_rate = byte_rate(prev.rx_bytes, cur.rx_bytes, dt);
                        net.tx_rate = byte_rate(prev.tx_bytes, cur.tx_bytes, dt);
                    }
                    net.rx_bytes = cur.rx_bytes;
                    net.tx_bytes = cur.tx_bytes;
                    prev_net = Some((sampled_at, cur));
                }

                // 磁盘仅夹带轮有新解析结果（df 失败时为 None）；缓存沿用上一次
                if let Some(disks) = raw.disks {
                    disks_cache = disks;
                }

                // 进程：本轮有 stat 快照才算榜（首帧新进程 CPU 全 0，次帧起有效）；
                // 段缺失时沿用上一次榜单；prev 表用本轮存活集合重建
                if let Some(proc_raw) = &raw.procs {
                    let payload_procs = build_procs_payload(proc_raw, &prev_procs, sampled_at);
                    prev_procs = proc_raw
                        .procs
                        .iter()
                        .map(|p| (p.pid, (sampled_at, p.cpu_ticks)))
                        .collect();
                    procs_cache = Some(payload_procs);
                }

                // GPU：Some(空) 在未探测时终审为无卡；非空更新缓存；
                // None（未带段）与有卡后的瞬时空输出均不改变现状
                if let Some(gpus) = raw.gpus {
                    if gpus.is_empty() {
                        if gpu_supported.is_none() {
                            gpu_supported = Some(false);
                        }
                    } else {
                        gpu_supported = Some(true);
                        gpus_cache = gpus;
                    }
                }

                seq += 1;

                let payload = MetricsPayload {
                    seq,
                    cpu: CpuMetricsPayload {
                        util,
                        load: raw.load,
                    },
                    mem: raw.mem,
                    net,
                    disks: disks_cache.clone(),
                    procs: procs_cache.clone().unwrap_or(ProcsPayload {
                        total: 0,
                        top: Vec::new(),
                    }),
                    gpus: gpus_cache.clone(),
                };
                let json = serde_json::to_vec(&payload).unwrap_or_default();
                // 帧接收端关闭（连接终结）：任务退出
                if frame_tx
                    .send(encode_frame(FrameType::Metrics, &json))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            // 采集失败静默降级：跳过本轮不发帧（前端保留最后一帧），累计失败数，
            // 绝不影响 PTY 主通道；连续失败达阈值后由下方节拍切慢轮询
            Err(e) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                debug!("指标采集本轮失败（连续第 {consecutive_failures} 轮）: {e}");
            }
        }

        // 节拍：正常用 interval，连续失败达阈值降为 10s 慢轮询。
        // 注意退避 sleep(10s) 长于心跳 TTL(9s) 但不会误杀活跃会话——前端无论 collector
        // 睡多久都每 5s 续约 last_heartbeat；影响仅为「UI 消失后」最坏 ~10s+9s 才自停。
        // cancel 仍立即醒，会话 shutdown 不被拖延。
        let delay = round_delay(interval, consecutive_failures);
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = sleep(delay) => {}
        }
    }
}

#[cfg(test)]
mod backoff_tests {
    use super::{BACKOFF_INTERVAL, round_delay};
    use std::time::Duration;

    #[test]
    fn delay_normal_until_threshold_then_backoff() {
        let normal = Duration::from_secs(3);
        // 0/1/2 次失败：维持正常间隔
        assert_eq!(round_delay(normal, 0), normal);
        assert_eq!(round_delay(normal, 1), normal);
        assert_eq!(round_delay(normal, 2), normal);
        // 连续 3 次起降为 10s 慢轮询
        assert_eq!(round_delay(normal, 3), BACKOFF_INTERVAL);
        assert_eq!(round_delay(normal, 10), BACKOFF_INTERVAL);
    }

    #[test]
    fn delay_recovers_immediately_on_success() {
        // 成功清零后（调用方传 0）立即恢复正常间隔，不残留退避
        assert_eq!(
            round_delay(Duration::from_secs(2), 0),
            Duration::from_secs(2)
        );
    }
}
