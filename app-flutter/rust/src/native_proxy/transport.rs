//! 传输层：TCP / TLS / WebSocket（重建 v1 `native_proxy/transport.rs` 的子集）。
//!
//! 与 v1 的分工一致：本文件只管「把一条字节流接到节点上」，协议握手在
//! `trojan.rs` / `vless.rs` / `ss.rs` 里。
//!
//! **本版已重建**：TCP、TLS（native-tls / Windows schannel，支持自定义根证书
//! `ca` = PEM 追加到系统根证书之外）、WebSocket（RFC 6455 客户端，
//! 带掩码与 ping/pong）、HTTP 混淆（`network: http`：发一个普通 HTTP 请求，收到 200
//! 之后整条连接变裸字节流）。
//! **本版未重建（如实返回原因）**：HTTP/2 / gRPC / XHTTP（`h2tunnel.rs`）、
//! REALITY（需要自写 TLS 1.3，`reality.rs`）。

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use base64::Engine as _;

use super::{HttpOptions, Node, TlsOptions, WsOptions};

/// 引擎内部统一用「可读可写的字节流」；TLS / WS / 协议包装都实现它。
///
/// `set_read_timeout` 是透传阶段的关键：std 的阻塞 socket 没有 select，而上下行共用
/// 同一个（TLS 状态下不可拆分的）流对象。**必须设在真正被读的那个对象上** ——
/// 实测（Windows）：给 `try_clone()` 出来的副本设 SO_RCVTIMEO，对原句柄的读不生效。
pub trait Stream: Read + Write + Send {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        let _ = timeout;
        Ok(())
    }
}

impl Stream for TcpStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        TcpStream::set_read_timeout(self, timeout)
    }
}

impl Stream for native_tls::TlsStream<BoxStream> {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.get_ref().set_read_timeout(timeout)
    }
}

pub type BoxStream = Box<dyn Stream>;

/// 拨号超时（v1 是 12s；这里保持同量级，失败要**快速**而不是挂死）。
pub const DIAL_TIMEOUT: Duration = Duration::from_secs(12);

pub fn connect_tcp(host: &str, port: u16, timeout: Duration) -> io::Result<TcpStream> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, "没有可用地址");
    for addr in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                let _ = stream.set_read_timeout(None);
                let _ = stream.set_write_timeout(None);
                return Ok(stream);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// 包一层 TLS。`allow_insecure` 同时放宽证书链与主机名（v1 `skip-cert-verify` 同口径）。
pub fn wrap_tls(
    inner: BoxStream,
    options: &TlsOptions,
    default_host: &str,
) -> Result<BoxStream, String> {
    if !options.enabled {
        return Ok(inner);
    }
    let mut builder = native_tls::TlsConnector::builder();
    if options.allow_insecure {
        builder.danger_accept_invalid_certs(true);
        builder.danger_accept_invalid_hostnames(true);
    }
    if !options.alpn.is_empty() {
        // 只有 `alpn` feature 才有这把口子（Cargo.toml 已开）；Windows 的 schannel 后端支持它。
        let protocols: Vec<&str> = options.alpn.iter().map(String::as_str).collect();
        builder.request_alpns(&protocols);
    }
    if !options.ca.trim().is_empty() {
        let blocks = pem_blocks(&options.ca);
        if blocks.is_empty() {
            return Err("自定义根证书解析失败（没找到 CERTIFICATE 块）".to_string());
        }
        for block in blocks {
            match native_tls::Certificate::from_pem(block.as_bytes()) {
                Ok(cert) => {
                    builder.add_root_certificate(cert);
                }
                Err(e) => {
                    // 原始错误只进日志，界面上只说人话（§禁用 59）。
                    eprintln!("[lunac-proxy] 自定义根证书解析失败：{e}");
                    return Err("自定义根证书解析失败（PEM 格式不对）".to_string());
                }
            }
        }
    }
    let connector = builder
        .build()
        .map_err(|e| format!("TLS 初始化失败：{e}"))?;
    let server_name = if options.sni.trim().is_empty() {
        default_host
    } else {
        options.sni.trim()
    };
    let stream = connector.connect(server_name, inner).map_err(|e| match e {
        // `HandshakeError<S>` 的 Display 要求 `S: Debug`，而 `Box<dyn Stream>` 不是 Debug；
        // 这里按变体手工取消息，避免为了格式化给整条流加 Debug 约束。
        native_tls::HandshakeError::Failure(e) => format!("TLS 握手失败：{e}"),
        native_tls::HandshakeError::WouldBlock(_) => "TLS 握手未完成（底层流不可读）".to_string(),
    })?;
    Ok(Box::new(stream))
}

/// 从一段文本里切出所有 `-----BEGIN CERTIFICATE----- … -----END CERTIFICATE-----` 块。
///
/// Clash 的 `ca-str` / 分享链接的 `ca=` 允许把多张证书首尾相接贴在一起，
/// `native_tls::Certificate::from_pem` 一次只吃一张，所以先切开。
fn pem_blocks(text: &str) -> Vec<String> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(BEGIN) {
        let after = &rest[start + BEGIN.len()..];
        let Some(end) = after.find(END) else { break };
        out.push(format!("{BEGIN}\n{}{END}\n", &after[..end]));
        rest = &after[end + END.len()..];
    }
    out
}

/// 在已有流上做 WebSocket 客户端握手，返回按帧收发的包装。
pub fn connect_ws(
    mut inner: BoxStream,
    ws: &WsOptions,
    server: &str,
    port: u16,
) -> Result<WsTunnel, String> {
    let mut key_bytes = [0u8; 16];
    getrandom::getrandom(&mut key_bytes).map_err(|_| "生成 WebSocket key 失败".to_string())?;
    let key = base64::engine::general_purpose::STANDARD.encode(key_bytes);
    let path = if ws.path.trim().is_empty() {
        "/"
    } else {
        ws.path.trim()
    };
    let host = if ws.host.trim().is_empty() {
        if port == 80 || port == 443 {
            server.to_string()
        } else {
            format!("{server}:{port}")
        }
    } else {
        ws.host.trim().to_string()
    };
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    inner
        .write_all(request.as_bytes())
        .map_err(|e| format!("WebSocket 握手发送失败：{e}"))?;
    // 读响应头（上限 32KB，防止坏服务端把我们读到天荒地老）。
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(pos) = find_header_end(&buffer) {
            let head = String::from_utf8_lossy(&buffer[..pos]).to_string();
            let status_ok = head
                .lines()
                .next()
                .map(|line| line.contains(" 101"))
                .unwrap_or(false);
            if !status_ok {
                return Err(format!(
                    "WebSocket 握手失败：{}",
                    head.lines().next().unwrap_or("没有状态行")
                ));
            }
            let rest = buffer.split_off(pos);
            return Ok(WsTunnel {
                inner,
                read_buffer: rest,
                payload: Vec::new(),
                closed: false,
            });
        }
        if buffer.len() > 32 * 1024 {
            return Err("WebSocket 握手响应头过大".to_string());
        }
        let read = inner
            .read(&mut chunk)
            .map_err(|e| format!("WebSocket 握手读取失败：{e}"))?;
        if read == 0 {
            return Err("WebSocket 握手时对端关闭".to_string());
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|pos| pos + 4)
}

/// 在已有流上做 HTTP 混淆握手：发请求 → 读掉响应头（必须 200）→ 之后就是裸字节流。
pub fn connect_http(
    mut inner: BoxStream,
    http: &HttpOptions,
    server: &str,
    port: u16,
) -> Result<HttpTunnel, String> {
    let method = if http.method.trim().is_empty() {
        "GET"
    } else {
        http.method.trim()
    };
    let path = if http.path.trim().is_empty() {
        "/"
    } else {
        http.path.trim()
    };
    let host = if http.host.trim().is_empty() {
        if port == 80 || port == 443 {
            server.to_string()
        } else {
            format!("{server}:{port}")
        }
    } else {
        http.host.trim().to_string()
    };
    let request =
        format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: keep-alive\r\n\r\n");
    inner
        .write_all(request.as_bytes())
        .map_err(|e| format!("HTTP 混淆请求发送失败：{e}"))?;
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(pos) = find_header_end(&buffer) {
            let head = String::from_utf8_lossy(&buffer[..pos]).to_string();
            let first = head.lines().next().unwrap_or("没有状态行");
            // `HTTP/1.1 200 OK` / `HTTP/1.1 200`：按状态码判，不按原因短语判。
            let ok = first
                .split_whitespace()
                .nth(1)
                .map(|code| code == "200")
                .unwrap_or(false);
            if !ok {
                return Err(format!("HTTP 混淆握手失败：{first}"));
            }
            let rest = buffer.split_off(pos);
            return Ok(HttpTunnel {
                inner,
                read_buffer: rest,
            });
        }
        if buffer.len() > 32 * 1024 {
            return Err("HTTP 混淆响应头过大".to_string());
        }
        let read = inner
            .read(&mut chunk)
            .map_err(|e| format!("HTTP 混淆握手读取失败：{e}"))?;
        if read == 0 {
            return Err("HTTP 混淆握手时对端关闭".to_string());
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}

/// HTTP 混淆之后的裸流：只会多一段「响应头之后已经读到的字节」。
pub struct HttpTunnel {
    inner: BoxStream,
    read_buffer: Vec<u8>,
}

impl Read for HttpTunnel {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if !self.read_buffer.is_empty() {
            let count = out.len().min(self.read_buffer.len());
            out[..count].copy_from_slice(&self.read_buffer[..count]);
            self.read_buffer.drain(..count);
            return Ok(count);
        }
        self.inner.read(out)
    }
}

impl Write for HttpTunnel {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.inner.write(data)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Stream for HttpTunnel {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_read_timeout(timeout)
    }
}

/// 按需建立节点的传输层（TCP → TLS → WS / HTTP 混淆）。协议握手由调用方继续。
pub fn connect_transport(node: &Node) -> Result<BoxStream, String> {
    // REALITY 只能配裸 TCP（上游也没有 ws / h2 over REALITY 这种组合）。
    if node.reality.is_some() && (node.ws.is_some() || node.http.is_some() || node.extra.is_some())
    {
        return Err("REALITY 只支持裸 TCP（不能与 ws / http / h2 叠用）".to_string());
    }
    // HTTP/2 系传输（h2 / grpc）不是「套在阻塞 socket 上的一层」，必须整条走异步栈。
    if let Some(extra) = &node.extra {
        if node.ws.is_some() || node.http.is_some() {
            return Err(format!(
                "节点同时声明了 {} 与 ws / http 传输（只允许一种）",
                extra.kind()
            ));
        }
        return super::h2tunnel::connect(node, extra);
    }
    let tcp = connect_tcp(&node.server, node.port, DIAL_TIMEOUT)
        .map_err(|e| format!("连接节点 {}:{} 失败：{e}", node.server, node.port))?;
    let stream: BoxStream = Box::new(tcp);
    // REALITY：**替换** TLS 层（不是叠加）—— 它自己要把认证写进 ClientHello。
    if node.reality.is_some() {
        return super::reality::connect(stream, node);
    }
    let stream = wrap_tls(stream, &node.tls, &node.server)?;
    if node.ws.is_some() && node.http.is_some() {
        return Err("节点同时声明了 ws 与 http 两种传输（只允许一种）".to_string());
    }
    if let Some(ws) = &node.ws {
        let tunnel = connect_ws(stream, ws, &node.server, node.port)?;
        return Ok(Box::new(tunnel));
    }
    if let Some(http) = &node.http {
        let tunnel = connect_http(stream, http, &node.server, node.port)?;
        return Ok(Box::new(tunnel));
    }
    Ok(stream)
}

/// WebSocket 帧包装：写时加掩码，读时解帧（服务端帧不带掩码）。
pub struct WsTunnel {
    inner: BoxStream,
    read_buffer: Vec<u8>,
    payload: Vec<u8>,
    closed: bool,
}

impl WsTunnel {
    fn send_frame(&mut self, opcode: u8, data: &[u8]) -> io::Result<()> {
        let mut frame = Vec::with_capacity(data.len() + 14);
        frame.push(0x80 | opcode);
        let mask_bit = 0x80u8;
        if data.len() < 126 {
            frame.push(mask_bit | data.len() as u8);
        } else if data.len() <= u16::MAX as usize {
            frame.push(mask_bit | 126);
            frame.extend_from_slice(&(data.len() as u16).to_be_bytes());
        } else {
            frame.push(mask_bit | 127);
            frame.extend_from_slice(&(data.len() as u64).to_be_bytes());
        }
        let mut mask = [0u8; 4];
        getrandom::getrandom(&mut mask)
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "生成掩码失败"))?;
        frame.extend_from_slice(&mask);
        for (index, byte) in data.iter().enumerate() {
            frame.push(byte ^ mask[index % 4]);
        }
        self.inner.write_all(&frame)
    }

    /// 从读缓冲里解析一个帧；返回 `Ok(Some(()))` = 有载荷追加 / `Ok(None)` = 需要更多字节。
    fn parse_frame(&mut self) -> io::Result<Option<()>> {
        let buffer = &self.read_buffer;
        if buffer.len() < 2 {
            return Ok(None);
        }
        let first = buffer[0];
        let second = buffer[1];
        let opcode = first & 0x0F;
        let masked = second & 0x80 != 0;
        let mut length = (second & 0x7F) as u64;
        let mut offset = 2usize;
        if length == 126 {
            if buffer.len() < 4 {
                return Ok(None);
            }
            length = u16::from_be_bytes([buffer[2], buffer[3]]) as u64;
            offset = 4;
        } else if length == 127 {
            if buffer.len() < 10 {
                return Ok(None);
            }
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&buffer[2..10]);
            length = u64::from_be_bytes(bytes);
            offset = 10;
        }
        if masked {
            offset += 4;
        }
        if buffer.len() < offset + length as usize {
            return Ok(None);
        }
        let mut data = buffer[offset..offset + length as usize].to_vec();
        if masked {
            let mask = &buffer[offset - 4..offset];
            for (index, byte) in data.iter_mut().enumerate() {
                *byte ^= mask[index % 4];
            }
        }
        self.read_buffer.drain(..offset + length as usize);
        match opcode {
            0x0 | 0x1 | 0x2 => {
                self.payload.extend_from_slice(&data);
                Ok(Some(()))
            }
            0x8 => {
                self.closed = true;
                Ok(Some(()))
            }
            0x9 => {
                // ping → pong（客户端帧必须掩码）。
                self.send_frame(0xA, &data)?;
                Ok(Some(()))
            }
            0xA => Ok(Some(())),
            _ => Ok(Some(())),
        }
    }
}

impl Read for WsTunnel {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        loop {
            if !self.payload.is_empty() {
                let count = out.len().min(self.payload.len());
                out[..count].copy_from_slice(&self.payload[..count]);
                self.payload.drain(..count);
                return Ok(count);
            }
            if self.closed {
                return Ok(0);
            }
            // 先尽量把已缓冲的帧全部消费掉。
            while self.parse_frame()?.is_some() {
                if !self.payload.is_empty() || self.closed {
                    break;
                }
            }
            if !self.payload.is_empty() || self.closed {
                continue;
            }
            let mut chunk = [0u8; 8192];
            let read = self.inner.read(&mut chunk)?;
            if read == 0 {
                self.closed = true;
                return Ok(0);
            }
            self.read_buffer.extend_from_slice(&chunk[..read]);
        }
    }
}

impl Write for WsTunnel {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        // 单帧上限 64KB：更长的写拆成多帧（对端按序拼接）。
        let mut written = 0;
        while written < data.len() {
            let end = (written + 64 * 1024).min(data.len());
            self.send_frame(0x2, &data[written..end])?;
            written = end;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Stream for WsTunnel {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_read_timeout(timeout)
    }
}

/// WebSocket 客户端帧的长度编码（单测用；与 RFC 6455 的三种宽度一致）。
#[cfg(test)]
pub fn ws_length_encoding(len: usize) -> Vec<u8> {
    if len < 126 {
        vec![len as u8]
    } else if len <= u16::MAX as usize {
        let mut out = vec![126u8];
        out.extend_from_slice(&(len as u16).to_be_bytes());
        out
    } else {
        let mut out = vec![127u8];
        out.extend_from_slice(&(len as u64).to_be_bytes());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn concatenated_pem_certificates_are_split_into_blocks() {
        let text = "说明\n-----BEGIN CERTIFICATE-----\nAAA\n-----END CERTIFICATE-----\n中间\n-----BEGIN CERTIFICATE-----\nBBB\n-----END CERTIFICATE-----\n尾巴";
        let blocks = pem_blocks(text);
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].contains("AAA") && !blocks[0].contains("BBB"));
        assert!(blocks[1].contains("BBB"));
        assert!(blocks[0].trim_end().ends_with("-----END CERTIFICATE-----"));
    }

    #[test]
    fn a_bad_ca_is_reported_in_plain_words_before_connecting() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let stream = TcpStream::connect(addr).unwrap();
        let options = TlsOptions {
            enabled: true,
            ca: "这不是证书".to_string(),
            ..TlsOptions::default()
        };
        let error = match wrap_tls(Box::new(stream), &options, "example.com") {
            Ok(_) => panic!("坏 PEM 必须在握手前就被拒绝"),
            Err(error) => error,
        };
        assert_eq!(error, "自定义根证书解析失败（没找到 CERTIFICATE 块）");
    }

    #[test]
    fn websocket_length_encoding_covers_three_widths() {
        assert_eq!(ws_length_encoding(5), vec![5]);
        assert_eq!(ws_length_encoding(126), vec![126, 0, 126]);
        assert_eq!(
            ws_length_encoding(70_000).len(),
            9,
            "64 位长度 = 1 字节标记 + 8 字节"
        );
    }

    #[test]
    fn websocket_handshake_and_echo() {
        // 本地 mock 服务端：回 101，然后把收到的帧原样回显（服务端帧不掩码）。
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 1024];
            loop {
                let read = stream.read(&mut chunk).unwrap();
                if read == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..read]);
                if let Some(pos) = find_header_end(&buffer) {
                    buffer.drain(..pos);
                    break;
                }
            }
            stream
                .write_all(
                    b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
                )
                .unwrap();
            // 读一个客户端帧（带掩码）并回显为服务端帧。
            let mut frame = buffer;
            while frame.len() < 2 {
                let read = stream.read(&mut chunk).unwrap();
                if read == 0 {
                    return;
                }
                frame.extend_from_slice(&chunk[..read]);
            }
            let masked = frame[1] & 0x80 != 0;
            let mut length = (frame[1] & 0x7F) as usize;
            let mut offset = 2;
            if length == 126 {
                while frame.len() < 4 {
                    let read = stream.read(&mut chunk).unwrap();
                    frame.extend_from_slice(&chunk[..read]);
                }
                length = u16::from_be_bytes([frame[2], frame[3]]) as usize;
                offset = 4;
            }
            if masked {
                offset += 4;
            }
            while frame.len() < offset + length {
                let read = stream.read(&mut chunk).unwrap();
                frame.extend_from_slice(&chunk[..read]);
            }
            let mut data = frame[offset..offset + length].to_vec();
            if masked {
                let mask = frame[offset - 4..offset].to_vec();
                for (index, byte) in data.iter_mut().enumerate() {
                    *byte ^= mask[index % 4];
                }
            }
            let mut reply = vec![0x82u8];
            if data.len() < 126 {
                reply.push(data.len() as u8);
            } else {
                reply.push(126);
                reply.extend_from_slice(&(data.len() as u16).to_be_bytes());
            }
            reply.extend_from_slice(&data);
            stream.write_all(&reply).unwrap();
        });

        let tcp = TcpStream::connect(addr).unwrap();
        let mut tunnel = connect_ws(
            Box::new(tcp),
            &WsOptions {
                path: "/ws".to_string(),
                host: String::new(),
            },
            "127.0.0.1",
            addr.port(),
        )
        .unwrap();
        tunnel.write_all(b"hello").unwrap();
        let mut out = [0u8; 5];
        tunnel.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"hello");
        handle.join().unwrap();
    }

    /// HTTP 混淆：先发一个普通 HTTP 请求，读掉 200 响应头之后就是裸字节流。
    #[test]
    fn http_obfs_handshake_and_echo() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 1024];
            while find_header_end(&buffer).is_none() {
                let read = stream.read(&mut chunk).unwrap();
                assert!(read > 0);
                buffer.extend_from_slice(&chunk[..read]);
            }
            let head = String::from_utf8_lossy(&buffer).to_string();
            assert!(
                head.starts_with("GET /obfs HTTP/1.1\r\n"),
                "首行必须是 GET <path> HTTP/1.1：{head}"
            );
            assert!(head.contains("Host: cdn.example.com\r\n"), "{head}");
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\n")
                .unwrap();
            let read = stream.read(&mut chunk).unwrap();
            stream.write_all(&chunk[..read]).unwrap();
        });

        let tcp = TcpStream::connect(addr).unwrap();
        let mut tunnel = connect_http(
            Box::new(tcp),
            &HttpOptions {
                method: String::new(),
                path: "/obfs".to_string(),
                host: "cdn.example.com".to_string(),
            },
            "127.0.0.1",
            addr.port(),
        )
        .unwrap();
        tunnel.write_all(b"hello").unwrap();
        let mut out = [0u8; 5];
        tunnel.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"hello");
        handle.join().unwrap();
    }

    /// 非 200 必须如实报错（v1 `http_obfs_rejects_non_200` 同口径）。
    #[test]
    fn http_obfs_rejects_non_200() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 1024];
            while find_header_end(&buffer).is_none() {
                let read = stream.read(&mut chunk).unwrap();
                if read == 0 {
                    return;
                }
                buffer.extend_from_slice(&chunk[..read]);
            }
            stream.write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n").unwrap();
        });

        let tcp = TcpStream::connect(addr).unwrap();
        let error = connect_http(
            Box::new(tcp),
            &HttpOptions::default(),
            "127.0.0.1",
            addr.port(),
        )
        .err()
        .expect("403 必须拒绝");
        assert!(error.contains("403"), "{error}");
        handle.join().unwrap();
    }
}
