//! 自定义二进制帧协议：1 字节类型 + 4 字节大端长度 + payload。
//!
//! 高频终端数据只走该协议（经 Tauri Channel<Vec<u8>> 传输），
//! 避免 JSON 序列化带来的 CPU/内存损耗。

/// 帧类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameType {
    /// PTY 原始输出（stdout/stderr 合并流）
    Data = 0x01,
    /// 会话结束。payload 为 JSON `{"reason": <展示文本>, "lost": <bool>}`：
    /// lost=true 表示未收到远端退出状态连接即关闭（网络中断等），前端可自动重连；
    /// lost=false 表示远端进程正常退出/被信号终止（用户主动 exit 等），不重连。
    Exit = 0x02,
    /// 会话内部错误（payload 为错误描述文本，UTF-8）。
    /// 协议预留：当前连接错误走 invoke 返回值，运行期错误并入 Exit 帧。
    #[allow(dead_code)]
    Error = 0x03,
    /// Rhost MOTD 欢迎面板指令数组（payload 为 JSON 序列化的 `[{t,text,cls}]`）。
    /// 由后端在 PTY 数据流开启前作为首帧发出，前端解析后按 cls 配色绘制，
    /// 保证 MOTD 始终出现在远端 shell 提示符之前。
    Motd = 0x04,
    /// 主机静态信息（连接建立时发送一次，payload 为 JSON HostInfo）。
    /// 在 MOTD 首帧之后、PTY Data 帧之前发出（mpsc FIFO 保序）。
    HostInfo = 0x05,
    /// 主机动态指标周期帧（payload 为 JSON Metrics：CPU/内存/网络/磁盘/进程/GPU），
    /// 由每会话 MetricsCollector 经独立 exec 通道采集后推送。
    Metrics = 0x06,
    /// 连接 RTT 探测结果（payload 为 JSON `{"ms": 23}`），
    /// 由独立定时任务经 SSH global request 计时后推送，与 Metrics 并行。
    Rtt = 0x07,
    /// Shell 当前工作目录变更通知（payload 为绝对路径 UTF-8 文本）。
    /// 由 merge_task 解析 PTY 输出中的 OSC 6667 序列后发出，前端据此
    /// 同步 SFTP 文件树到同一目录（Shell → SFTP 方向同步）。
    Cwd = 0x08,
    /// SSH 握手真实协商算法（连接建立时发送一次，payload 为 JSON AlgoInfo：
    /// 主机密钥算法 / 对称加密算法 / PTY 终端类型 / 终端字符编码）。
    /// 状态栏据此显示真实值，而非硬编码占位。
    Algo = 0x09,
}

impl FrameType {
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

/// 编码一帧：`[type: u8][len: u32 BE][payload...]`
pub fn encode_frame(frame_type: FrameType, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + payload.len());
    out.push(frame_type.as_u8());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// 解码一帧（输入必须恰好是一帧），返回 (类型字节, payload)。
/// 当前由前端解析帧，后端仅测试使用。
#[allow(dead_code)]
pub fn decode_frame(frame: &[u8]) -> Option<(u8, &[u8])> {
    if frame.len() < 5 {
        return None;
    }
    let frame_type = frame[0];
    let len = u32::from_be_bytes([frame[1], frame[2], frame[3], frame[4]]) as usize;
    if frame.len() < 5 + len {
        return None;
    }
    Some((frame_type, &frame[5..5 + len]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip() {
        let payload = b"hello pty";
        let frame = encode_frame(FrameType::Data, payload);
        assert_eq!(frame[0], 0x01);
        assert_eq!(&frame[1..5], &9u32.to_be_bytes());
        let (t, p) = decode_frame(&frame).unwrap();
        assert_eq!(t, 0x01);
        assert_eq!(p, payload);
    }

    #[test]
    fn frame_empty_payload() {
        let frame = encode_frame(FrameType::Exit, b"");
        let (t, p) = decode_frame(&frame).unwrap();
        assert_eq!(t, 0x02);
        assert!(p.is_empty());
    }

    #[test]
    fn frame_hostinfo_and_metrics() {
        let payload = br#"{"os":"Linux","cores":32}"#;
        let frame = encode_frame(FrameType::HostInfo, payload);
        let (t, p) = decode_frame(&frame).unwrap();
        assert_eq!(t, 0x05);
        assert_eq!(p, payload);

        let payload = br#"{"seq":1,"cpu":{"util":38.2}}"#;
        let frame = encode_frame(FrameType::Metrics, payload);
        let (t, p) = decode_frame(&frame).unwrap();
        assert_eq!(t, 0x06);
        assert_eq!(p, payload);
    }

    #[test]
    fn frame_rtt() {
        let payload = br#"{"ms":23}"#;
        let frame = encode_frame(FrameType::Rtt, payload);
        let (t, p) = decode_frame(&frame).unwrap();
        assert_eq!(t, 0x07);
        assert_eq!(p, payload);
    }
}
