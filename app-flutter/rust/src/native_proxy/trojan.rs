//! Trojan 协议（重建 v1 `native_proxy/trojan.rs`）。
//!
//! 握手（服务端 spec，v1 单测逐字节对齐）：
//! ```text
//! hex(sha224(password)) CRLF <cmd> <ATYP addr> <port BE> CRLF
//! ```
//! `cmd = 0x01` = CONNECT，`cmd = 0x03` = UDP ASSOCIATE。
//! UDP 阶段每个数据报：`<ATYP addr> <port BE> <u16 len BE> CRLF <payload>`（双向同格式）。
//!
//! TLS 由 `transport::connect_transport` 负责；这里只写协议头与分帧。

use std::io::Write;

use sha2::{Digest, Sha224};

use super::encode_target;
use super::transport::BoxStream;

/// 建好传输层后调用：写 Trojan 请求头，返回可直接透传的流。
pub fn connect(
    mut stream: BoxStream,
    password: &str,
    host: &str,
    port: u16,
) -> Result<BoxStream, String> {
    let head = handshake_bytes(password, host, port);
    stream
        .write_all(&head)
        .map_err(|e| format!("Trojan 握手发送失败：{e}"))?;
    Ok(stream)
}

/// UDP ASSOCIATE（`cmd = 0x03`）：握手之后按 [`udp_frame`] 收发数据报。
pub fn connect_udp(
    mut stream: BoxStream,
    password: &str,
    host: &str,
    port: u16,
) -> Result<BoxStream, String> {
    let head = handshake_udp_bytes(password, host, port);
    stream
        .write_all(&head)
        .map_err(|e| format!("Trojan UDP 握手发送失败：{e}"))?;
    Ok(stream)
}

/// 请求头字节（单测直接比对；不含载荷）。
pub fn handshake_bytes(password: &str, host: &str, port: u16) -> Vec<u8> {
    handshake_with_command(password, 0x01, host, port)
}

/// UDP ASSOCIATE 的请求头（与 CONNECT 只差 cmd 字节）。
pub fn handshake_udp_bytes(password: &str, host: &str, port: u16) -> Vec<u8> {
    handshake_with_command(password, 0x03, host, port)
}

fn handshake_with_command(password: &str, command: u8, host: &str, port: u16) -> Vec<u8> {
    let digest = Sha224::digest(password.as_bytes());
    let mut out = Vec::with_capacity(120);
    out.extend_from_slice(hex_lower(&digest).as_bytes());
    out.extend_from_slice(b"\r\n");
    out.push(command);
    out.extend_from_slice(&encode_target(host, port));
    out.extend_from_slice(b"\r\n");
    out
}

/// 一个 UDP 数据报的线上格式：`ATYP addr port | len | CRLF | payload`。
pub fn udp_frame(host: &str, port: u16, payload: &[u8]) -> Vec<u8> {
    let payload = &payload[..payload.len().min(u16::MAX as usize)];
    let mut out = Vec::with_capacity(32 + payload.len());
    out.extend_from_slice(&encode_target(host, port));
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(payload);
    out
}

/// 解析一个 UDP 数据报；`None` = 还差字节或格式不对。
/// 返回 `(host, port, payload, 已消费字节数)`。
pub fn parse_udp_frame(bytes: &[u8]) -> Option<(String, u16, Vec<u8>, usize)> {
    let (host, port, used) = super::udp::parse_socks_addr(bytes)?;
    if bytes.len() < used + 2 + 2 {
        return None;
    }
    let len = u16::from_be_bytes([bytes[used], bytes[used + 1]]) as usize;
    if &bytes[used + 2..used + 4] != b"\r\n" {
        return None;
    }
    let start = used + 4;
    if bytes.len() < start + len {
        return None;
    }
    Some((host, port, bytes[start..start + len].to_vec(), start + len))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn udp_associate_header_and_frame_layout() {
        let head = handshake_udp_bytes("abc", "example.com", 53);
        // 56 字符 hex + CRLF + cmd。
        assert_eq!(head[56], b'\r');
        assert_eq!(head[57], b'\n');
        assert_eq!(head[58], 0x03, "UDP ASSOCIATE 的 cmd 必须是 0x03");
        assert_eq!(head[59], 0x03); // ATYP = domain
        assert_eq!(&head[72..74], &53u16.to_be_bytes());

        let frame = udp_frame("example.com", 53, b"dns");
        assert_eq!(frame[0], 0x03);
        assert_eq!(frame[1], 11);
        assert_eq!(&frame[2..13], b"example.com");
        assert_eq!(&frame[13..15], &53u16.to_be_bytes());
        assert_eq!(&frame[15..17], &3u16.to_be_bytes(), "长度 = 载荷长度");
        assert_eq!(&frame[17..19], b"\r\n");
        assert_eq!(&frame[19..], b"dns");

        let (host, port, payload, used) = parse_udp_frame(&frame).unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 53);
        assert_eq!(payload, b"dns");
        assert_eq!(used, frame.len());
        // 半包必须返回 None（由上层缓冲）。
        assert!(parse_udp_frame(&frame[..frame.len() - 1]).is_none());
    }

    #[test]
    fn handshake_head_matches_trojan_spec() {
        // SHA-224("abc") 的标准向量（FIPS 180-4 示例）。
        let head = handshake_bytes("abc", "example.com", 443);
        let hex = String::from_utf8_lossy(&head[..56]).to_string();
        assert_eq!(
            hex,
            "23097d223405d8228642a477bda255b32aadbce4bda0b3f7e36c9da7"
        );
        // 结构断言：56 字符 hex + CRLF + 0x01 + 域名字符串 + 端口 + CRLF。
        assert_eq!(head[56], b'\r');
        assert_eq!(head[57], b'\n');
        assert_eq!(head[58], 0x01);
        assert_eq!(head[59], 0x03); // ATYP = domain
        assert_eq!(head[60], 11); // "example.com"
        assert_eq!(&head[61..72], b"example.com");
        assert_eq!(&head[72..74], &443u16.to_be_bytes());
        assert_eq!(&head[74..], b"\r\n");
    }
}
