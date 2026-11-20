//! XUDP（Xray 的 UDP over Mux）——v1 `native_proxy/vless_udp.rs` 的重建。
//!
//! **为什么需要它**：VLESS 的 Vision 账号在服务端明确拒绝 `cmd=UDP`
//! （`inbound.go`：`flow doesn't support UDP`），必须改成 **Mux（`v1.mux.cool`）**
//! 并把数据报按 XUDP 封装；VMess 同理（inbound 只处理 TCP / Mux）。
//! 非 Vision 的 VLESS 不走这里 —— Xray 只在 Vision / cone 时转 Mux，其余直接 `cmd=UDP`
//! （见 [`super::vless::connect_udp`]）。
//!
//! 帧格式（2026-11-15 联网核对 `common/xudp/xudp.go` 与 `common/mux/frame.go`）：
//! ```text
//! metaLen   u16 BE        // 只覆盖「元数据」，不含 dataLen 与 data
//! sessionID u16 BE        // XUDP 恒 0
//! status    u8            // 1=New 2=Keep 3=End 4=KeepAlive
//! option    u8            // bit0 = data，bit1 = error
//! [status=New 或 Keep 且带宽带目标时]
//!   network u8            // 1=TCP 2=UDP
//!   port    u16 BE        // **端口在地址之前**（Xray 的 PortThenAddress）
//!   addr    ATYP + 地址   // 1=IPv4 2=域名 3=IPv6（Xray 的 AddressType）
//! [仅 status=New] globalID 8 字节（服务端只当不透明值）
//! dataLen   u16 BE
//! data
//! ```
//! 首包用 `New`（带目标 + 8 字节 GlobalID），之后每包用 `Keep`，并**始终带目标地址**
//! （一条 mux 流里会混多个目标，服务端按帧里的地址路由）。
//! 保活包 `status=4`、无数据帧（`option` 的 data 位为 0）直接跳过。
//!
//! 读取方向接受 `Keep` / `End` / `KeepAlive`；`End` 视为会话关闭。

use std::io::{self, Read, Write};

use super::transport::BoxStream;

/// XUDP 的 mux 流 ID（Xray 客户端恒用 0）。
const SESSION_ID: u16 = 0;
const STATUS_NEW: u8 = 1;
const STATUS_KEEP: u8 = 2;
const STATUS_END: u8 = 3;
const STATUS_KEEPALIVE: u8 = 4;
const OPTION_DATA: u8 = 0x01;
const NETWORK_UDP: u8 = 0x02;

/// 一条 XUDP 会话（底层是一条已经写成 `cmd=Mux` 的 VLESS/VMess 流）。
pub struct XudpStream {
    inner: BoxStream,
    opened: bool,
    pending: Vec<u8>,
    /// 最近一次出现的目标（服务端的 Keep 帧可能不带地址）。
    last_target: Option<(String, u16)>,
}

impl XudpStream {
    pub fn new(inner: BoxStream) -> Self {
        Self {
            inner,
            opened: false,
            pending: Vec::new(),
            last_target: None,
        }
    }

    /// 发一个数据报到 `host:port`。
    pub fn send(&mut self, host: &str, port: u16, payload: &[u8]) -> Result<(), String> {
        if payload.len() > u16::MAX as usize {
            return Err("XUDP 数据报超过 65535 字节".to_string());
        }
        let status = if self.opened { STATUS_KEEP } else { STATUS_NEW };
        let mut frame = Vec::with_capacity(payload.len() + 64);
        frame.extend_from_slice(&[0, 0]); // metaLen 占位
        frame.extend_from_slice(&SESSION_ID.to_be_bytes());
        frame.push(status);
        frame.push(OPTION_DATA);
        frame.push(NETWORK_UDP);
        // PortThenAddress：端口在地址之前。
        frame.extend_from_slice(&port.to_be_bytes());
        frame.extend_from_slice(&encode_address(host));
        if !self.opened {
            // GlobalID：服务端只当 8 字节不透明值（全 0 合法）。
            frame.extend_from_slice(&[0u8; 8]);
            self.opened = true;
        }
        // metaLen **只覆盖元数据**（不含 dataLen 与 data）。
        let meta_len = (frame.len() - 2) as u16;
        frame[0..2].copy_from_slice(&meta_len.to_be_bytes());
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        frame.extend_from_slice(payload);
        self.inner
            .write_all(&frame)
            .map_err(|e| format!("XUDP 发送失败：{e}"))?;
        self.last_target = Some((host.to_string(), port));
        Ok(())
    }

    /// 收一个数据报；`Ok(None)` = 暂时没有。
    pub fn recv(&mut self) -> Result<Option<(String, u16, Vec<u8>)>, String> {
        loop {
            if !self.fill(2)? {
                return Ok(None);
            }
            let meta_len = u16::from_be_bytes([self.pending[0], self.pending[1]]) as usize;
            if meta_len < 4 {
                return Err(format!("XUDP 元数据长度非法：{meta_len}"));
            }
            if !self.fill(2 + meta_len)? {
                return Ok(None);
            }
            let meta = self.pending[2..2 + meta_len].to_vec();
            let status = meta[2];
            let option = meta[3];
            let mut target = None;
            if (status == STATUS_NEW || status == STATUS_KEEP)
                && meta.len() > 4
                && meta[4] == NETWORK_UDP
            {
                let (host, port, _used) = parse_port_then_address(&meta[5..])
                    .ok_or_else(|| "XUDP 帧里的目标地址非法".to_string())?;
                target = Some((host, port));
            }
            // dataLen 在 meta 之后；不够就留着整帧下次再解。
            if !self.fill(2 + meta_len + 2)? {
                return Ok(None);
            }
            let data_len =
                u16::from_be_bytes([self.pending[2 + meta_len], self.pending[2 + meta_len + 1]])
                    as usize;
            if !self.fill(2 + meta_len + 2 + data_len)? {
                return Ok(None);
            }
            let data_start = 2 + meta_len + 2;
            let data = self.pending[data_start..data_start + data_len].to_vec();
            self.pending.drain(..data_start + data_len);

            match status {
                STATUS_END => return Err("XUDP 会话已被对端关闭".to_string()),
                STATUS_KEEPALIVE => continue,
                _ => {}
            }
            if option & OPTION_DATA == 0 {
                continue;
            }
            let (host, port) = match target {
                Some(value) => value,
                None => self
                    .last_target
                    .clone()
                    .ok_or_else(|| "XUDP 帧既没带地址，也没有可复用的目标".to_string())?,
            };
            return Ok(Some((host, port, data)));
        }
    }

    /// 保证 `pending` 至少有 `need` 字节；`false` = 暂时没数据（非阻塞 / 超时）。
    fn fill(&mut self, need: usize) -> Result<bool, String> {
        while self.pending.len() < need {
            let mut chunk = [0u8; 65535];
            match self.inner.read(&mut chunk) {
                Ok(0) => return Err("XUDP 流被对端关闭".to_string()),
                Ok(read) => self.pending.extend_from_slice(&chunk[..read]),
                Err(ref e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::TimedOut =>
                {
                    return Ok(false)
                }
                Err(e) => return Err(format!("XUDP 读取失败：{e}")),
            }
        }
        Ok(true)
    }
}

/// Xray 的地址类型：1=IPv4 / 2=域名 / 3=IPv6（与 SOCKS5 的 3=域名不同）。
fn encode_address(host: &str) -> Vec<u8> {
    let text = host.trim().trim_matches(['[', ']']);
    let mut out = Vec::with_capacity(17);
    if let Ok(ip) = text.parse::<std::net::Ipv4Addr>() {
        out.push(0x01);
        out.extend_from_slice(&ip.octets());
    } else if let Ok(ip) = text.parse::<std::net::Ipv6Addr>() {
        out.push(0x03);
        out.extend_from_slice(&ip.octets());
    } else {
        let bytes = text.as_bytes();
        let len = bytes.len().min(255);
        out.push(0x02);
        out.push(len as u8);
        out.extend_from_slice(&bytes[..len]);
    }
    out
}

/// 解 `端口 u16 BE | ATYP 地址`，返回 `(host, port, 已消费字节数)`。
fn parse_port_then_address(bytes: &[u8]) -> Option<(String, u16, usize)> {
    if bytes.len() < 3 {
        return None;
    }
    let port = u16::from_be_bytes([bytes[0], bytes[1]]);
    let mut offset = 2;
    let host = match bytes[offset] {
        0x01 => {
            if bytes.len() < offset + 1 + 4 {
                return None;
            }
            let ip = std::net::Ipv4Addr::new(
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
                bytes[offset + 4],
            );
            offset += 5;
            ip.to_string()
        }
        0x02 => {
            if bytes.len() < offset + 2 {
                return None;
            }
            let len = bytes[offset + 1] as usize;
            if bytes.len() < offset + 2 + len {
                return None;
            }
            let host = String::from_utf8_lossy(&bytes[offset + 2..offset + 2 + len]).to_string();
            offset += 2 + len;
            host
        }
        0x03 => {
            if bytes.len() < offset + 1 + 16 {
                return None;
            }
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&bytes[offset + 1..offset + 17]);
            offset += 17;
            std::net::Ipv6Addr::from(octets).to_string()
        }
        _ => return None,
    };
    Some((host, port, offset))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::thread;

    /// 一条 `cmd=Mux` 的 VLESS 流 + XUDP：请求帧逐字段校验，回程 Keep 帧能解出数据报。
    #[test]
    fn xudp_round_trip_through_a_mock_vless_mux_server() {
        let uuid = [0x11u8; 16];
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // VLESS 请求头：version + uuid + addonLen(0) + cmd=3（Mux）。
            let mut head = [0u8; 19];
            stream.read_exact(&mut head).unwrap();
            assert_eq!(head[0], 0x00);
            assert_eq!(&head[1..17], &uuid);
            assert_eq!(head[17], 0x00, "非 Vision ⇒ addonLen=0");
            assert_eq!(head[18], 0x03, "UDP 走 Mux");
            // **cmd=Mux 的 VLESS 头不带端口与地址**（v1 的坑 ①）：紧接着就是 mux 帧。

            let mut meta_len_bytes = [0u8; 2];
            stream.read_exact(&mut meta_len_bytes).unwrap();
            let meta_len = u16::from_be_bytes(meta_len_bytes) as usize;
            assert!(meta_len >= 4);
            let mut meta = vec![0u8; meta_len];
            stream.read_exact(&mut meta).unwrap();
            assert_eq!(&meta[..2], &0u16.to_be_bytes(), "session ID 恒 0");
            assert_eq!(meta[2], STATUS_NEW, "首包是 New");
            assert_eq!(meta[3], OPTION_DATA);
            assert_eq!(meta[4], NETWORK_UDP);
            assert_eq!(u16::from_be_bytes([meta[5], meta[6]]), 53, "端口在地址之前");
            assert_eq!(meta[7], 0x01, "IPv4");
            assert_eq!(&meta[8..12], &[1, 1, 1, 1]);
            assert_eq!(&meta[12..20], &[0u8; 8], "New 帧带 8 字节 GlobalID");

            let mut len = [0u8; 2];
            stream.read_exact(&mut len).unwrap();
            let data_len = u16::from_be_bytes(len) as usize;
            let mut data = vec![0u8; data_len];
            stream.read_exact(&mut data).unwrap();
            assert_eq!(&data, b"query");

            // VLESS 响应头（version + addonLen=0），之后才是 mux 帧。
            stream.write_all(&[0u8, 0u8]).unwrap();
            // 回程：Keep 帧（带目标地址）+ 数据。
            let mut reply_meta = Vec::new();
            reply_meta.extend_from_slice(&SESSION_ID.to_be_bytes());
            reply_meta.push(STATUS_KEEP);
            reply_meta.push(OPTION_DATA);
            reply_meta.push(NETWORK_UDP);
            reply_meta.extend_from_slice(&53u16.to_be_bytes());
            reply_meta.push(0x01);
            reply_meta.extend_from_slice(&[8, 8, 8, 8]);
            let mut frame = Vec::new();
            frame.extend_from_slice(&(reply_meta.len() as u16).to_be_bytes());
            frame.extend_from_slice(&reply_meta);
            frame.extend_from_slice(&(6u16).to_be_bytes());
            frame.extend_from_slice(b"answer");
            stream.write_all(&frame).unwrap();
            let _ = stream.shutdown(Shutdown::Write);
            thread::sleep(std::time::Duration::from_millis(50));
        });

        let tcp = TcpStream::connect(addr).unwrap();
        let stream: BoxStream = Box::new(tcp);
        // 用真的 `connect_mux`：它会写 `version | uuid | addonLen=0 | cmd=3` 且**不带地址**。
        let stream =
            super::super::vless::connect_mux(stream, "11111111-1111-1111-1111-111111111111", "")
                .unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_millis(5)))
            .unwrap();
        let mut xudp = XudpStream::new(stream);
        xudp.send("1.1.1.1", 53, b"query").unwrap();
        let mut received = None;
        for _ in 0..200 {
            if let Some(value) = xudp.recv().unwrap() {
                received = Some(value);
                break;
            }
            thread::sleep(std::time::Duration::from_millis(5));
        }
        let (host, port, payload) = received.expect("必须收到 mock 服务端的回包");
        assert_eq!(host, "8.8.8.8");
        assert_eq!(port, 53);
        assert_eq!(payload, b"answer");
        handle.join().unwrap();
    }

    /// 域名 + IPv6 的地址编码与「端口在前」解析必须自洽。
    #[test]
    fn address_encoding_is_port_then_xray_atyp() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&443u16.to_be_bytes());
        bytes.extend_from_slice(&encode_address("example.com"));
        let (host, port, used) = parse_port_then_address(&bytes).unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 443);
        assert_eq!(used, bytes.len());

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&53u16.to_be_bytes());
        bytes.extend_from_slice(&encode_address("2001:db8::1"));
        let (host, port, _used) = parse_port_then_address(&bytes).unwrap();
        assert_eq!(host, "2001:db8::1");
        assert_eq!(port, 53);
    }

    /// 保活帧（status=4）要跳过，不能当成数据。
    #[test]
    fn keepalive_frames_are_skipped() {
        let mut frame = Vec::new();
        frame.extend_from_slice(&4u16.to_be_bytes());
        frame.extend_from_slice(&SESSION_ID.to_be_bytes());
        frame.push(STATUS_KEEPALIVE);
        frame.push(OPTION_DATA);
        frame.extend_from_slice(&0u16.to_be_bytes());
        let stream: BoxStream = Box::new(CursorStream {
            data: frame,
            pos: 0,
        });
        let mut xudp = XudpStream::new(stream);
        // 保活之后直接 EOF ⇒ 如实报「流被对端关闭」，而不是给出空数据报。
        let error = xudp.recv().err().expect("EOF 必须报错");
        assert!(error.contains("关闭"), "{error}");
    }

    struct CursorStream {
        data: Vec<u8>,
        pos: usize,
    }

    impl Read for CursorStream {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            if self.pos >= self.data.len() {
                return Ok(0);
            }
            let take = out.len().min(self.data.len() - self.pos);
            out[..take].copy_from_slice(&self.data[self.pos..self.pos + take]);
            self.pos += take;
            Ok(take)
        }
    }

    impl Write for CursorStream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl super::super::transport::Stream for CursorStream {}
}
