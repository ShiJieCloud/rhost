//! SOCKS5 握手解析（RFC 1928「无认证 + CONNECT」子集）。
//!
//! 解析逻辑拆成纯函数（无网络依赖、可单测），I/O 包装函数统一受
//! `SOCKS_HANDSHAKE_MAX` 读取上限与外层 `HANDSHAKE_TIMEOUT` 约束，
//! 杜绝 slowloris 式慢速握手。
//!
//! 仅支持：方法协商只接受无认证（0x00）；CONNECT 命令（0x01）；
//! ATYP 支持 IPv4 / 域名 / IPv6。畸形报文/超限一律断连，不 panic、不影响监听器。

use std::net::Ipv6Addr;

use log::debug;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::{DnsResolve, SOCKS_HANDSHAKE_MAX};

/// SOCKS 版本号
pub(crate) const SOCKS_VER: u8 = 0x05;
/// 认证方法：无认证
pub(crate) const METHOD_NO_AUTH: u8 = 0x00;
/// 不支持的认证方法回复码
pub(crate) const METHOD_NONE_ACCEPTABLE: u8 = 0xFF;
/// CONNECT 命令码
pub(crate) const CMD_CONNECT: u8 = 0x01;

/// CONNECT 回复错误码。
/// 注：`0x03 Network unreachable` 与 `0x05 Connection refused` 需区分远端
/// connect 失败细类，russh 不暴露 open failure 细节（同回 RequestDenied），
/// 引擎统一按 `GENERAL_FAILURE` 回复，故不定义这两个码。
pub(crate) mod reply {
    /// CONNECT 成功（RFC 1928）；目标 channel 打开后由引擎回复
    pub const SUCCEEDED: u8 = 0x00;
    pub const GENERAL_FAILURE: u8 = 0x01;
    pub const HOST_UNREACHABLE: u8 = 0x04;
    pub const CMD_NOT_SUPPORTED: u8 = 0x07;
    pub const ATYP_NOT_SUPPORTED: u8 = 0x08;
}

/// 地址类型
pub(crate) const ATYP_IPV4: u8 = 0x01;
pub(crate) const ATYP_DOMAIN: u8 = 0x03;
pub(crate) const ATYP_IPV6: u8 = 0x04;

/// 解析成功的 CONNECT 请求：目标主机（IP 文本或域名）与端口
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SocksRequest {
    pub host: String,
    pub port: u16,
}

/// 方法协商回复：`[VER, METHOD]`。客户端方法列表含无认证时选 0x00，
/// 否则 0xFF（无可接受方法，客户端应关闭）。
pub(crate) fn method_reply(methods: &[u8]) -> [u8; 2] {
    if methods.contains(&METHOD_NO_AUTH) {
        [SOCKS_VER, METHOD_NO_AUTH]
    } else {
        [SOCKS_VER, METHOD_NONE_ACCEPTABLE]
    }
}

/// CONNECT 请求解析（纯函数）。`buf` 为 `[VER, CMD, RSV, ATYP, ADDR..., PORT...]`。
/// 成功返回目标；失败返回应回复的错误码（畸形 VER/截断报文按一般失败 0x01 处理）。
pub(crate) fn parse_connect(buf: &[u8]) -> Result<SocksRequest, u8> {
    if buf.len() < 4 {
        return Err(reply::GENERAL_FAILURE);
    }
    if buf[0] != SOCKS_VER {
        return Err(reply::GENERAL_FAILURE);
    }
    if buf[1] != CMD_CONNECT {
        return Err(reply::CMD_NOT_SUPPORTED);
    }
    let atyp = buf[3];
    let (host, consumed) = match atyp {
        ATYP_IPV4 => {
            if buf.len() < 4 + 4 {
                return Err(reply::GENERAL_FAILURE);
            }
            (
                std::net::Ipv4Addr::new(buf[4], buf[5], buf[6], buf[7]).to_string(),
                4 + 4,
            )
        }
        ATYP_IPV6 => {
            if buf.len() < 4 + 16 {
                return Err(reply::GENERAL_FAILURE);
            }
            let octets: [u8; 16] = buf[4..20].try_into().expect("16 字节切片");
            (Ipv6Addr::from(octets).to_string(), 4 + 16)
        }
        ATYP_DOMAIN => {
            if buf.len() < 5 {
                return Err(reply::GENERAL_FAILURE);
            }
            let len = buf[4] as usize;
            // 域名长度 1–255；0 长度视为畸形
            if len == 0 {
                return Err(reply::GENERAL_FAILURE);
            }
            if buf.len() < 5 + len {
                return Err(reply::GENERAL_FAILURE);
            }
            match std::str::from_utf8(&buf[5..5 + len]) {
                Ok(s) => (s.to_string(), 5 + len),
                Err(_) => return Err(reply::GENERAL_FAILURE),
            }
        }
        _ => return Err(reply::ATYP_NOT_SUPPORTED),
    };
    if buf.len() < consumed + 2 {
        return Err(reply::GENERAL_FAILURE);
    }
    let port = u16::from_be_bytes([buf[consumed], buf[consumed + 1]]);
    Ok(SocksRequest { host, port })
}

/// 构造 CONNECT 回复报文：`05 code 00 01 BND.ADDR(4) BND.PORT(2)`，
/// BND 填零（SOCKS 客户端普遍不校验）。
pub(crate) fn connect_reply(code: u8) -> [u8; 10] {
    let mut out = [0u8; 10];
    out[0] = SOCKS_VER;
    out[1] = code;
    out[3] = ATYP_IPV4;
    out
}

/// 握手失败原因：I/O 错误（连接断开/超时）或协议拒绝（需回复错误码后关闭）
#[derive(Debug)]
pub(crate) enum NegotiateError {
    Io(std::io::Error),
    Rejected(u8),
}

impl std::fmt::Display for NegotiateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NegotiateError::Io(e) => write!(f, "i/o 错误: {e}"),
            // SOCKS 回复错误码（reply:: 常量）
            NegotiateError::Rejected(code) => write!(f, "协议拒绝（reply 0x{code:02x}）"),
        }
    }
}

/// 带读取上限的 SOCKS 连接包装：`SOCKS_HANDSHAKE_MAX` 覆盖整个握手过程的
/// 累计读取量，读写统一走此包装避免借用冲突
struct SocksConn<'a> {
    tcp: &'a mut TcpStream,
    remaining: usize,
}

impl SocksConn<'_> {
    async fn read_exact(&mut self, n: usize) -> Result<Vec<u8>, NegotiateError> {
        if n > self.remaining {
            // 累计读取量超限：按一般失败拒绝（攻击面：恶意客户端灌包）
            return Err(NegotiateError::Rejected(reply::GENERAL_FAILURE));
        }
        self.remaining -= n;
        let mut buf = vec![0u8; n];
        self.tcp
            .read_exact(&mut buf)
            .await
            .map_err(NegotiateError::Io)?;
        Ok(buf)
    }

    async fn write_all(&mut self, buf: &[u8]) -> Result<(), NegotiateError> {
        self.tcp.write_all(buf).await.map_err(NegotiateError::Io)
    }

    /// 回复错误码并半关闭，返回 Rejected 错误（调用方收到后直接断开即可）
    async fn reject(&mut self, code: u8) -> NegotiateError {
        let _ = self.tcp.write_all(&connect_reply(code)).await;
        let _ = self.tcp.shutdown().await;
        NegotiateError::Rejected(code)
    }
}

/// 执行完整 SOCKS5 握手（方法协商 + CONNECT 解析），返回目标。
///
/// DNS 策略（`dns`）：
/// - `Remote`：域名/IP 文本原样返回，交给 sshd 解析（默认）；
/// - `Local`：域名先在本机 `lookup_host` 解析取首个地址，失败回 0x04。
///
/// 协议失败（方法不支持/CMD/ATYP/畸形）在内部回复错误码后返回
/// `Rejected`，调用方只需关闭连接；I/O 错误时连接已死，直接返回。
/// 调用方负责以 `HANDSHAKE_TIMEOUT` 包裹本函数（握手计入握手窗口）。
pub(crate) async fn negotiate(
    tcp: &mut TcpStream,
    dns: DnsResolve,
) -> Result<SocksRequest, NegotiateError> {
    let mut c = SocksConn {
        tcp,
        remaining: SOCKS_HANDSHAKE_MAX,
    };

    // ---- 1. 方法协商：[VER, NMETHODS, METHODS...] ----
    let head = c.read_exact(2).await?;
    if head[0] != SOCKS_VER {
        debug!("SOCKS5 非法版本 {}", head[0]);
        return Err(c.reject(reply::GENERAL_FAILURE).await);
    }
    let methods = c.read_exact(head[1] as usize).await?;
    let mrep = method_reply(&methods);
    c.write_all(&mrep).await?;
    if mrep[1] == METHOD_NONE_ACCEPTABLE {
        // 05 FF 已回复：客户端无可接受方法，断开
        debug!("SOCKS5 客户端不支持无认证方法");
        let _ = c.tcp.shutdown().await;
        return Err(NegotiateError::Rejected(METHOD_NONE_ACCEPTABLE));
    }

    // ---- 2. CONNECT 请求：[VER, CMD, RSV, ATYP, ADDR..., PORT...] ----
    let head = c.read_exact(4).await?;
    if head[1] != CMD_CONNECT {
        debug!("SOCKS5 不支持的命令 {}", head[1]);
        return Err(c.reject(reply::CMD_NOT_SUPPORTED).await);
    }
    let atyp = head[3];
    let (mut addr_len, with_len_prefix) = match atyp {
        ATYP_IPV4 => (4usize, false),
        ATYP_IPV6 => (16, false),
        ATYP_DOMAIN => (0, true),
        _ => return Err(c.reject(reply::ATYP_NOT_SUPPORTED).await),
    };
    let mut req = Vec::with_capacity(4 + SOCKS_HANDSHAKE_MAX + 2);
    req.extend_from_slice(&head);
    if with_len_prefix {
        let len_byte = c.read_exact(1).await?;
        req.extend_from_slice(&len_byte);
        if len_byte[0] == 0 {
            // 0 长度域名为畸形报文
            return Err(c.reject(reply::GENERAL_FAILURE).await);
        }
        // 域名地址实际长度来自 len 字节（超限由 read_exact 的 remaining 兜底拒绝）
        addr_len = len_byte[0] as usize;
    }
    let addr = c.read_exact(addr_len).await?;
    req.extend_from_slice(&addr);
    let port_bytes = c.read_exact(2).await?;
    req.extend_from_slice(&port_bytes);

    let mut request = match parse_connect(&req) {
        Ok(r) => r,
        Err(code) => return Err(c.reject(code).await),
    };
    if dns == DnsResolve::Local && request.host.parse::<std::net::IpAddr>().is_err() {
        // local 策略：本机解析域名，失败回 Host unreachable。
        // 优先取 IPv4：宿主解析结果可能以 IPv6 在先（如 macOS localhost → ::1），
        // 而远端 sshd 未必监听 IPv6，直取首个会造成 CONNECT 无谓失败。
        let host = request.host.clone();
        match tokio::net::lookup_host((host.as_str(), request.port)).await {
            Ok(addrs) => {
                let addrs: Vec<_> = addrs.collect();
                match addrs.iter().find(|sa| sa.is_ipv4()).or_else(|| addrs.first()) {
                    Some(sa) => request.host = sa.ip().to_string(),
                    None => return Err(c.reject(reply::HOST_UNREACHABLE).await),
                }
            }
            Err(e) => {
                debug!("SOCKS5 本机解析 {} 失败: {e}", request.host);
                return Err(c.reject(reply::HOST_UNREACHABLE).await);
            }
        }
    }
    Ok(request)
}

/// 回复失败并断开（尽力而为；客户端可能已关闭，忽略写错误）。
/// 供调用方在 channel 打开失败时回复对应错误码。
pub(crate) async fn reject(tcp: &mut TcpStream, code: u8) {
    let _ = tcp.write_all(&connect_reply(code)).await;
    let _ = tcp.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_negotiation_selects_no_auth() {
        assert_eq!(method_reply(&[0x00]), [0x05, 0x00]);
        // 客户端同时提供多种方法，含 0x00 即可选
        assert_eq!(method_reply(&[0x01, 0x02, 0x00]), [0x05, 0x00]);
        // 不含 0x00：05 FF
        assert_eq!(method_reply(&[0x01, 0x02]), [0x05, 0xFF]);
        assert_eq!(method_reply(&[]), [0x05, 0xFF]);
    }

    #[test]
    fn connect_ipv4_atyp() {
        let buf = [0x05, 0x01, 0x00, 0x01, 10, 0, 0, 1, 0x1F, 0x90];
        let req = parse_connect(&buf).unwrap();
        assert_eq!(req.host, "10.0.0.1");
        assert_eq!(req.port, 8080);
    }

    #[test]
    fn connect_domain_atyp() {
        let mut buf = vec![0x05, 0x01, 0x00, 0x03, 9];
        buf.extend_from_slice(b"localhost");
        buf.extend_from_slice(&443u16.to_be_bytes());
        let req = parse_connect(&buf).unwrap();
        assert_eq!(req.host, "localhost");
        assert_eq!(req.port, 443);
    }

    #[test]
    fn connect_ipv6_atyp() {
        let mut buf = vec![0x05, 0x01, 0x00, 0x04];
        buf.extend_from_slice(&Ipv6Addr::LOCALHOST.octets());
        buf.extend_from_slice(&22u16.to_be_bytes());
        let req = parse_connect(&buf).unwrap();
        assert_eq!(req.host, "::1");
        assert_eq!(req.port, 22);
    }

    #[test]
    fn connect_rejects_bad_cmd_ver_atyp() {
        // CMD != CONNECT
        let buf = [0x05, 0x02, 0x00, 0x01, 1, 2, 3, 4, 0, 80];
        assert_eq!(parse_connect(&buf), Err(reply::CMD_NOT_SUPPORTED));
        // VER != 5
        let buf = [0x04, 0x01, 0x00, 0x01, 1, 2, 3, 4, 0, 80];
        assert_eq!(parse_connect(&buf), Err(reply::GENERAL_FAILURE));
        // ATYP 非法
        let buf = [0x05, 0x01, 0x00, 0x09, 1, 2, 3, 4, 0, 80];
        assert_eq!(parse_connect(&buf), Err(reply::ATYP_NOT_SUPPORTED));
        // 截断报文
        let buf = [0x05, 0x01, 0x00, 0x01, 1, 2];
        assert_eq!(parse_connect(&buf), Err(reply::GENERAL_FAILURE));
    }

    #[test]
    fn connect_rejects_zero_length_domain() {
        let buf = [0x05, 0x01, 0x00, 0x03, 0, 0, 80];
        assert_eq!(parse_connect(&buf), Err(reply::GENERAL_FAILURE));
    }

    #[test]
    fn reply_bytes_exact() {
        // 成功回复：05 00 00 01 00 00 00 00 00 00
        assert_eq!(connect_reply(0x00), [5, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(connect_reply(reply::CMD_NOT_SUPPORTED)[1], 0x07);
        assert_eq!(connect_reply(reply::ATYP_NOT_SUPPORTED)[1], 0x08);
    }
}
