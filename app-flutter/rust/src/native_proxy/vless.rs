//! VLESS 协议（重建 v1 `native_proxy/vless.rs`：基础链路 + Vision）。
//!
//! 请求头（Xray 口径）：
//! ```text
//! 0x00 version
//! uuid[16]
//! addonLen[1]（Vision 时 18：`0x0A 0x10 "xtls-rprx-vision"`）
//! 0x01 command (TCP)
//! port[2 BE]
//! atyp (1=IPv4 / 2=domain / 3=IPv6) + addr
//! ```
//! 响应头：`version[1] addonLen[1] addons[addonLen]`，之后才是目标数据。
//!
//! **UDP**（2026-11-15 核对 Xray `vless/outbound/outbound.go` 与
//! `vless/encoding/addons.go`）：
//!   · 非 Vision、未开 cone ⇒ `cmd=0x02`（UDP）+ 端口地址，随后每包
//!     `[length u16 BE][payload]`（`EncodeBodyAddons` 的 `MultiLengthPacketWriter` /
//!     `DecodeBodyAddons` 的 `LengthPacketReader`）；目标由请求头固定 ⇒ 一条连接只服务一个目标；
//!   · Vision（或开了 cone）⇒ 服务端拒绝 `cmd=UDP`，必须转 Mux + XUDP（见 [`connect_mux`]）。
//!
//! **请求头的端口在地址之前**（Xray `WriteAddressPort`），命令 `0x01=TCP / 0x02=UDP / 0x03=Mux`。
//!
//! **Vision（`flow=xtls-rprx-vision`）**，与 v1 第九轮的实现同口径：
//!   · 服务端要**先拿到首段业务数据**才回响应头 ⇒ 响应头必须推迟到第一次 `read` 再消费
//!     （提前阻塞会死等；v1 当时踩过，这里沿用同样的结构）；
//!   · 上行：首段包成 `[uuid:16][cmd=1][contentLen u16][paddingLen u16][content][padding]`，
//!     之后是裸流（`cmd=0` 表示后面还有帧，`1/2` 表示这帧之后转裸流；uuid 只写一帧）；
//!   · 下行：**首帧**是 `uuid[16] | cmd | contentLen u16 | paddingLen u16`，之后是
//!     `cmd | contentLen u16 | paddingLen u16`（5 字节头，uuid 每方向只出现一次）。
//!     `cmd == 0` 继续按帧解填充，`cmd != 0`（1=EndPadding / 2=Direct）之后转裸流。
//!     XUDP / Mux 这类非 TLS 流量服务端一直发 `cmd=0` 的填充帧，所以必须支持多帧。
//!   · 本版上行 paddingLen 恒为 0（服务端只按该长度跳过，不校验内容）；v1 的
//!     「TLS-in-TLS long padding / 直通指纹伪装」仍未做。

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr};

use super::transport::{BoxStream, Stream};

/// Vision 首帧帧头：`uuid[16] | cmd | contentLen u16 | paddingLen u16`。
const VISION_HEADER_LEN: usize = 21;
/// 后续帧帧头（不带 uuid）：`cmd | contentLen u16 | paddingLen u16`。
const VISION_HEADER_LEN_AFTER_FIRST: usize = 5;

/// 解析 UUID（带 / 不带短横线的 32 位十六进制）为 16 字节。
pub fn parse_uuid(text: &str) -> Result<[u8; 16], String> {
    let clean: String = text.chars().filter(|c| *c != '-').collect();
    if clean.len() != 32 || !clean.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("VLESS uuid 非法：{text}"));
    }
    let mut out = [0u8; 16];
    for index in 0..16 {
        let byte = u8::from_str_radix(&clean[index * 2..index * 2 + 2], 16)
            .map_err(|_| format!("VLESS uuid 非法：{text}"))?;
        out[index] = byte;
    }
    Ok(out)
}

/// `flow` 是否是 Vision（Xray 的 `xtls-rprx-vision` / `-udp443` 变体）。
pub fn is_vision(flow: &str) -> bool {
    flow.trim()
        .to_ascii_lowercase()
        .starts_with("xtls-rprx-vision")
}

/// 请求头字节（单测直接比对）。
///
/// **不要复用 `encode_target`**：VLESS 的地址类型码与 SOCKS5 不同
/// （1=IPv4 / 2=域名 / 3=IPv6；SOCKS5 的域名是 3），混用会让服务端把域名当 IPv6 解析。
pub fn request_header(uuid: &[u8; 16], host: &str, port: u16, vision: bool) -> Vec<u8> {
    header(uuid, 0x01, host, port, vision)
}

/// 请求头（`command` 由调用方给：1=TCP / 2=UDP / 3=Mux；Mux 不带端口与地址）。
fn header(uuid: &[u8; 16], command: u8, host: &str, port: u16, vision: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(80);
    out.push(0x00); // version
    out.extend_from_slice(uuid);
    if vision {
        // protobuf：field 1 (flow) = "xtls-rprx-vision"。
        out.push(18); // addon length
        out.push(0x0A);
        out.push(0x10);
        out.extend_from_slice(b"xtls-rprx-vision");
    } else {
        out.push(0x00); // addon length
    }
    out.push(command);
    if command != 0x03 {
        out.extend_from_slice(&port.to_be_bytes());
        out.extend_from_slice(&encode_address(host));
    }
    out
}

/// VLESS 地址：`ATYP + addr`（端口在地址之前，由调用方拼）。
fn encode_address(host: &str) -> Vec<u8> {
    let text = host.trim().trim_matches(['[', ']']);
    let mut out = Vec::with_capacity(17);
    if let Ok(ip) = text.parse::<Ipv4Addr>() {
        out.push(0x01);
        out.extend_from_slice(&ip.octets());
    } else if let Ok(ip) = text.parse::<Ipv6Addr>() {
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

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    /// 还没消费响应头（Vision 服务端要等首段数据才发）。
    ResponseHeader,
    /// Vision 帧（解填充中）。
    VisionFrame,
    /// 裸流（Vision 的 `cmd != 0` 之后，或本来就不是 Vision）。
    Raw,
}

/// VLESS 流：写请求头后立即返回；响应头与 Vision 帧都在第一次 `read` 时才处理。
pub struct VlessStream {
    inner: BoxStream,
    uuid: [u8; 16],
    vision: bool,
    /// Vision 的首段是否已经写过（之后是裸流）。
    wrote: bool,
    /// 下行是否已经收过带 uuid 的首帧。Vision 的 uuid **每个方向只出现一次**
    /// （Xray `XtlsPadding` 写完就把 `writeOnceUserUUID` 置空），之后都是 5 字节头。
    vision_uuid_seen: bool,
    phase: Phase,
    /// 已从 `inner` 读到、还没消化的字节。
    pending: Vec<u8>,
    /// 已解出的明文（Vision 帧的 content）。
    payload: Vec<u8>,
}

/// 建好传输层后调用：写请求头并返回流（响应头延迟到首次读）。
pub fn connect(
    mut stream: BoxStream,
    uuid: &str,
    flow: &str,
    host: &str,
    port: u16,
) -> Result<BoxStream, String> {
    let id = parse_uuid(uuid)?;
    let vision = is_vision(flow);
    let head = request_header(&id, host, port, vision);
    stream
        .write_all(&head)
        .map_err(|e| format!("VLESS 请求头发送失败：{e}"))?;
    Ok(Box::new(VlessStream {
        inner: stream,
        uuid: id,
        vision,
        wrote: false,
        vision_uuid_seen: false,
        phase: Phase::ResponseHeader,
        pending: Vec::new(),
        payload: Vec::new(),
    }))
}

/// `cmd=Mux`（3）的 VLESS 头：**不带端口与地址**，之后是 `v1.mux.cool` 帧
/// （XUDP 用；v1 当时踩过「多写地址被服务端当业务数据」的坑）。
pub fn connect_mux(mut stream: BoxStream, uuid: &str, flow: &str) -> Result<BoxStream, String> {
    let id = parse_uuid(uuid)?;
    let vision = is_vision(flow);
    let mut head = Vec::with_capacity(40);
    head.push(0x00); // version
    head.extend_from_slice(&id);
    if vision {
        head.push(18);
        head.push(0x0A);
        head.push(0x10);
        head.extend_from_slice(b"xtls-rprx-vision");
    } else {
        head.push(0x00);
    }
    head.push(0x03); // command = Mux
    stream
        .write_all(&head)
        .map_err(|e| format!("VLESS Mux 请求头发送失败：{e}"))?;
    Ok(Box::new(VlessStream {
        inner: stream,
        uuid: id,
        vision,
        wrote: false,
        vision_uuid_seen: false,
        phase: Phase::ResponseHeader,
        pending: Vec::new(),
        payload: Vec::new(),
    }))
}

/// `cmd=UDP`（0x02）的 VLESS 头：非 Vision 节点走这条（Xray 只在 Vision / cone 时才转 Mux+XUDP）。
/// 目标由请求头固定 ⇒ 每个目标一条连接；体内每包 `[length u16 BE][payload]`。
pub fn connect_udp(
    mut stream: BoxStream,
    uuid: &str,
    host: &str,
    port: u16,
) -> Result<VlessUdpStream, String> {
    let id = parse_uuid(uuid)?;
    let head = header(&id, 0x02, host, port, false);
    stream
        .write_all(&head)
        .map_err(|e| format!("VLESS UDP 请求头发送失败：{e}"))?;
    Ok(VlessUdpStream {
        inner: stream,
        header_read: false,
        pending: Vec::new(),
    })
}

/// `cmd=UDP` 的 VLESS 流：读方向先吃响应头，再按 `[length u16][payload]` 逐包解。
pub struct VlessUdpStream {
    inner: BoxStream,
    header_read: bool,
    pending: Vec<u8>,
}

impl VlessUdpStream {
    /// 再读一段；超时 / 无数据返回 `false`（UDP 轮询语义）。
    fn pump(&mut self) -> io::Result<bool> {
        let mut chunk = [0u8; 8192];
        match self.inner.read(&mut chunk) {
            Ok(0) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "VLESS UDP 流被对端关闭",
            )),
            Ok(read) => {
                self.pending.extend_from_slice(&chunk[..read]);
                Ok(true)
            }
            Err(ref e)
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut =>
            {
                Ok(false)
            }
            Err(e) => Err(e),
        }
    }

    /// 发一个数据报：`[length u16 BE][payload]`。
    pub fn send(&mut self, payload: &[u8]) -> io::Result<()> {
        if payload.len() > u16::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "VLESS UDP 单包超过 65535 字节",
            ));
        }
        let mut frame = Vec::with_capacity(2 + payload.len());
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        frame.extend_from_slice(payload);
        self.inner.write_all(&frame)
    }

    /// 收一个数据报；`Ok(None)` = 暂时没有。
    pub fn recv(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop {
            if !self.header_read {
                if self.pending.len() >= 2 {
                    let addon_len = self.pending[1] as usize;
                    if self.pending.len() >= 2 + addon_len {
                        self.pending.drain(..2 + addon_len);
                        self.header_read = true;
                        continue;
                    }
                }
            } else if self.pending.len() >= 2 {
                let length = u16::from_be_bytes([self.pending[0], self.pending[1]]) as usize;
                if self.pending.len() >= 2 + length {
                    self.pending.drain(..2);
                    let payload: Vec<u8> = self.pending.drain(..length).collect();
                    return Ok(Some(payload));
                }
            }
            if !self.pump()? {
                return Ok(None);
            }
        }
    }
}

impl VlessStream {
    /// 从底层再读一段放进 `pending`；`false` = 对端关闭。
    fn fill(&mut self) -> io::Result<bool> {
        let mut chunk = [0u8; 8192];
        let read = self.inner.read(&mut chunk)?;
        if read == 0 {
            return Ok(false);
        }
        self.pending.extend_from_slice(&chunk[..read]);
        Ok(true)
    }

    fn need(&mut self, len: usize) -> io::Result<()> {
        while self.pending.len() < len {
            if !self.fill()? {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "VLESS 流在头部中途关闭",
                ));
            }
        }
        Ok(())
    }

    /// 消费响应头：`version[1] addonLen[1] addons[addonLen]`。
    fn read_response_header(&mut self) -> io::Result<()> {
        self.need(2)?;
        let addon_len = self.pending[1] as usize;
        self.need(2 + addon_len)?;
        self.pending.drain(..2 + addon_len);
        self.phase = if self.vision {
            Phase::VisionFrame
        } else {
            Phase::Raw
        };
        Ok(())
    }

    /// 解一个 Vision 帧；数据不够返回 `Ok(false)`。
    ///
    /// 帧头：**首帧** = `uuid[16] | cmd | contentLen u16 | paddingLen u16`（21 字节），
    /// 之后 = `cmd | contentLen u16 | paddingLen u16`（5 字节）—— 对齐 Xray
    /// `XtlsPadding` 里 `writeOnceUserUUID` 只写一次的行为。
    fn parse_vision_frame(&mut self) -> io::Result<bool> {
        let header_len = if self.vision_uuid_seen {
            VISION_HEADER_LEN_AFTER_FIRST
        } else {
            VISION_HEADER_LEN
        };
        if self.pending.len() < header_len {
            return Ok(false);
        }
        if !self.vision_uuid_seen && self.pending[..16] != self.uuid[..] {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "VLESS Vision 响应帧的 uuid 不匹配",
            ));
        }
        let base = header_len - 5;
        let command = self.pending[base];
        let content_len =
            u16::from_be_bytes([self.pending[base + 1], self.pending[base + 2]]) as usize;
        let padding_len =
            u16::from_be_bytes([self.pending[base + 3], self.pending[base + 4]]) as usize;
        let total = header_len + content_len + padding_len;
        if self.pending.len() < total {
            return Ok(false);
        }
        self.vision_uuid_seen = true;
        self.payload
            .extend_from_slice(&self.pending[header_len..header_len + content_len]);
        self.pending.drain(..total);
        if command != 0 {
            // 1/2 = 这帧之后转裸流。
            self.phase = Phase::Raw;
        }
        Ok(true)
    }
}

impl Read for VlessStream {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            if !self.payload.is_empty() {
                let take = out.len().min(self.payload.len());
                out[..take].copy_from_slice(&self.payload[..take]);
                self.payload.drain(..take);
                return Ok(take);
            }
            match self.phase {
                Phase::ResponseHeader => {
                    self.read_response_header()?;
                }
                Phase::VisionFrame => {
                    if self.parse_vision_frame()? {
                        continue;
                    }
                    if !self.fill()? {
                        if self.pending.is_empty() {
                            return Ok(0);
                        }
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "VLESS Vision 帧被截断",
                        ));
                    }
                }
                Phase::Raw => {
                    if !self.pending.is_empty() {
                        self.payload.extend_from_slice(&self.pending);
                        self.pending.clear();
                        continue;
                    }
                    return self.inner.read(out);
                }
            }
        }
    }
}

impl Write for VlessStream {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if !self.vision || self.wrote {
            return self.inner.write(data);
        }
        self.wrote = true;
        // 首段：`cmd=1`（这帧之后转裸流）+ padding 0。超过 u16 的写入拆成多帧，
        // 非末帧用 `cmd=0`（后面还有帧）；**uuid 只放在第一帧**（Vision 每方向只写一次）。
        let mut offset = 0usize;
        while offset < data.len() {
            let end = (offset + u16::MAX as usize).min(data.len());
            let last = end == data.len();
            let mut frame = Vec::with_capacity(VISION_HEADER_LEN + end - offset);
            if offset == 0 {
                frame.extend_from_slice(&self.uuid);
            }
            frame.push(if last { 1 } else { 0 });
            frame.extend_from_slice(&((end - offset) as u16).to_be_bytes());
            frame.extend_from_slice(&0u16.to_be_bytes()); // padding 长度
            frame.extend_from_slice(&data[offset..end]);
            self.inner.write_all(&frame)?;
            offset = end;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Stream for VlessStream {
    fn set_read_timeout(&self, timeout: Option<std::time::Duration>) -> io::Result<()> {
        self.inner.set_read_timeout(timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// 只读不写的流：`connect` 会先写请求头，用 `Cursor` 会把预置的响应内容覆盖掉。
    struct ReadOnly {
        data: Cursor<Vec<u8>>,
    }

    impl Read for ReadOnly {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            self.data.read(out)
        }
    }

    impl Write for ReadOnly {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Stream for ReadOnly {}

    /// 双向测试桩：读预置数据、把写到的字节存进共享缓冲（用例里再断言布局）。
    struct Duplex {
        input: Cursor<Vec<u8>>,
        output: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    }

    impl Read for Duplex {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(out)
        }
    }

    impl Write for Duplex {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.output.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Stream for Duplex {}

    #[test]
    fn uuid_is_parsed_from_hex_with_dashes() {
        let parsed = parse_uuid("11111111-2222-3333-4444-555555555555").unwrap();
        assert_eq!(parsed[0], 0x11);
        assert_eq!(parsed[15], 0x55);
        assert!(parse_uuid("not-a-uuid").is_err());
    }

    #[test]
    fn request_header_matches_vless_layout() {
        let uuid = parse_uuid("11111111-2222-3333-4444-555555555555").unwrap();
        let header = request_header(&uuid, "example.com", 443, false);
        assert_eq!(header[0], 0x00);
        assert_eq!(&header[1..17], &uuid);
        assert_eq!(header[17], 0x00); // addon len
        assert_eq!(header[18], 0x01); // cmd
        assert_eq!(&header[19..21], &443u16.to_be_bytes()); // 端口在地址之前
        assert_eq!(header[21], 0x02); // domain
        assert_eq!(header[22], 11);
        assert_eq!(&header[23..34], b"example.com");
    }

    #[test]
    fn request_header_uses_xray_address_types() {
        let uuid = [0u8; 16];
        let ipv4 = request_header(&uuid, "1.2.3.4", 80, false);
        assert_eq!(ipv4[21], 0x01);
        assert_eq!(&ipv4[22..26], &[1, 2, 3, 4]);
        let ipv6 = request_header(&uuid, "::1", 80, false);
        assert_eq!(ipv6[21], 0x03);
        assert_eq!(ipv6.len(), 22 + 16);
    }

    /// Vision 的请求头 addon 必须逐字节是 `0A 10 "xtls-rprx-vision"`。
    #[test]
    fn vision_header_carries_flow_addons() {
        let uuid = [7u8; 16];
        let header = request_header(&uuid, "v.example.com", 443, true);
        assert_eq!(header[17], 18, "addon 长度");
        assert_eq!(&header[18..20], &[0x0A, 0x10]);
        assert_eq!(&header[20..36], b"xtls-rprx-vision");
        assert_eq!(header[36], 0x01, "command 在 addon 之后");
        assert_eq!(&header[37..39], &443u16.to_be_bytes());
        assert!(is_vision("xtls-rprx-vision"));
        assert!(is_vision("xtls-rprx-vision-udp443"));
        assert!(!is_vision(""));
        assert!(!is_vision("none"));
    }

    #[test]
    fn response_header_is_consumed_before_payload() {
        let payload = {
            let mut bytes = vec![0u8, 0u8];
            bytes.extend_from_slice(b"HTTP/1.1 204");
            bytes
        };
        let stream: BoxStream = Box::new(ReadOnly {
            data: Cursor::new(payload),
        });
        let mut out = connect(
            stream,
            "11111111-2222-3333-4444-555555555555",
            "",
            "a.test",
            80,
        )
        .unwrap();
        let mut buffer = [0u8; 12];
        out.read_exact(&mut buffer).unwrap();
        assert_eq!(&buffer, b"HTTP/1.1 204");
    }

    /// Vision：首段写入被包成 `[uuid][cmd=1][len][pad=0][data]`；
    /// 下行**只有首帧带 uuid**，之后是 5 字节头（`cmd=0` 继续解填充，`cmd≠0` 之后转裸流）。
    #[test]
    fn vision_first_write_is_framed_and_downlink_unpadded() {
        let uuid = parse_uuid("11111111-2222-3333-4444-555555555555").unwrap();
        let mut response = vec![0u8, 0u8]; // version + addonLen(0)
        let frame = |with_uuid: bool, command: u8, content: &[u8], padding: &[u8]| {
            let mut out = Vec::new();
            if with_uuid {
                out.extend_from_slice(&uuid);
            }
            out.push(command);
            out.extend_from_slice(&(content.len() as u16).to_be_bytes());
            out.extend_from_slice(&(padding.len() as u16).to_be_bytes());
            out.extend_from_slice(content);
            out.extend_from_slice(padding);
            out
        };
        response.extend_from_slice(&frame(true, 0, b"hello", b"pp"));
        response.extend_from_slice(&frame(false, 0, b"world", b"qq"));
        response.extend_from_slice(&frame(false, 1, b"again", b""));
        response.extend_from_slice(b"RAW");

        let written = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let stream: BoxStream = Box::new(Duplex {
            input: Cursor::new(response),
            output: written.clone(),
        });
        let mut out = connect(
            stream,
            "11111111-2222-3333-4444-555555555555",
            "xtls-rprx-vision",
            "v.example.com",
            443,
        )
        .unwrap();
        out.write_all(b"GET / HTTP/1.1\r\n").unwrap();

        // 写入内容 = 请求头（18 字节 addon）+ Vision 首帧。
        let head = request_header(&uuid, "v.example.com", 443, true);
        let bytes = written.lock().unwrap().clone();
        assert_eq!(&bytes[..head.len()], &head[..]);
        let frame = &bytes[head.len()..];
        assert_eq!(&frame[..16], &uuid);
        assert_eq!(frame[16], 1, "首帧之后转裸流");
        assert_eq!(
            u16::from_be_bytes([frame[17], frame[18]]) as usize,
            b"GET / HTTP/1.1\r\n".len()
        );
        assert_eq!(&frame[19..21], &0u16.to_be_bytes(), "padding 长度 0");
        assert_eq!(&frame[21..], b"GET / HTTP/1.1\r\n");
        assert_eq!(frame.len(), VISION_HEADER_LEN + b"GET / HTTP/1.1\r\n".len());

        let mut buffer = [0u8; 15];
        out.read_exact(&mut buffer).unwrap();
        assert_eq!(&buffer, b"helloworldagain");
        let mut tail = [0u8; 3];
        out.read_exact(&mut tail).unwrap();
        assert_eq!(&tail, b"RAW");
    }

    /// 非 Vision 的 UDP：`cmd=0x02` + 端口在地址之前；体内每包 `[len u16][payload]`。
    #[test]
    fn udp_request_uses_command_two_and_length_prefixed_body() {
        let written = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut response = vec![0u8, 0u8]; // version + addonLen(0)
        response.extend_from_slice(&5u16.to_be_bytes());
        response.extend_from_slice(b"reply");
        let stream: BoxStream = Box::new(Duplex {
            input: Cursor::new(response),
            output: written.clone(),
        });
        let mut out = connect_udp(
            stream,
            "11111111-2222-3333-4444-555555555555",
            "dns.example.com",
            53,
        )
        .unwrap();
        out.send(b"query").unwrap();

        let bytes = written.lock().unwrap().clone();
        let head = bytes[..bytes.len() - 7].to_vec();
        assert_eq!(head[0], 0x00, "version");
        let parsed = parse_uuid("11111111-2222-3333-4444-555555555555").unwrap();
        assert_eq!(&head[1..17], &parsed[..]);
        assert_eq!(head[17], 0x00, "addon len");
        assert_eq!(head[18], 0x02, "command = UDP");
        assert_eq!(&head[19..21], &53u16.to_be_bytes());
        assert_eq!(head[21], 0x02, "domain");
        assert_eq!(head[22] as usize, "dns.example.com".len());
        assert_eq!(&head[23..], b"dns.example.com");
        assert_eq!(&bytes[bytes.len() - 7..], b"\x00\x05query");

        assert_eq!(out.recv().unwrap().unwrap(), b"reply");
    }

    /// Vision 帧的 uuid 不对必须报错。
    #[test]
    fn vision_frame_with_foreign_uuid_is_rejected() {
        let mut response = vec![0u8, 0u8];
        response.extend_from_slice(&[9u8; 16]);
        response.extend_from_slice(&[1u8, 0, 0, 0, 0]);

        let stream: BoxStream = Box::new(ReadOnly {
            data: Cursor::new(response),
        });
        let mut out = connect(
            stream,
            "11111111-2222-3333-4444-555555555555",
            "xtls-rprx-vision",
            "v.example.com",
            443,
        )
        .unwrap();
        let mut buffer = [0u8; 1];
        let error = out.read(&mut buffer).err().expect("uuid 不匹配必须报错");
        assert!(error.to_string().contains("uuid"), "{error}");
    }
}
