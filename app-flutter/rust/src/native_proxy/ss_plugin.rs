//! SIP003 `simple-obfs`：`obfs=http` / `obfs=tls`（重建 v1 `native_proxy/ss_plugin.rs` 的这两档）。
//!
//! `v2ray-plugin` 的 websocket 模式**不需要**本文件：它就是把整条流塞进标准 WS，
//! 直接复用 `transport.rs`（见 `clash.rs` 的 `parse_ss_plugin`）。
//!
//! 逐字节对齐 `shadowsocks/simple-obfs` 的 `src/obfs_http.c` / `src/obfs_tls.c`
//! （2026-11-15 联网核对原文与 `obfs_tls.h` 的 packed 结构体，不是照抄记忆）：
//!
//! **`obfs=http`**（`http_request_template` / `deobfs_http_header`）
//!   · 首个写：先拼一段 WebSocket 升级请求，`Content-Length` = 首段长度，随后就是首段明文；
//!   · 之后：裸流（不再有头）；
//!   · 读方向：服务端连上就先发一个空的 101 响应（`send_empty_response_upon_connection`），
//!     客户端把第一个 `\r\n\r\n` 之前的内容整体丢掉，剩下原样透传。
//!
//! **`obfs=tls`**（`obfs_tls_request` / `deobfs_tls_response` / `deobfs_app_data`）
//!   · 首个写：138 字节固定 ClientHello 头 + `session_ticket` 扩展（里面藏首段载荷）
//!     + SNI 扩展 + 66 字节固定扩展；三个长度字段互相咬合：
//!     `record.len = total - 5`、`handshake.len = total - 9`、`ext_len = 4 + payload + 9 + host + 66`；
//!   · 之后每段：`0x17 0x03 0x03 | len u16 BE | payload`（len > 16384 服务端直接报错）；
//!   · 读方向：先吃 96 字节 ServerHello + 6 字节 CCS + 5 字节 encrypted_handshake 头
//!     （其 `len` = 首段载荷长度），首段之后才是同样的应用数据帧。
//!
//! `obfs=tls` 是**假 TLS**：它不能与真 TLS 叠加，所以这种节点的 `tls.enabled` 恒为 false。

use std::io::{self, Read, Write};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;

use super::transport::{BoxStream, Stream};
use super::SsObfs;

/// simple-obfs 的应用数据帧头（3 字节）。
const TLS_APP_HEADER: [u8; 3] = [0x17, 0x03, 0x03];
/// 服务端头总长：96 字节 ServerHello + 6 字节 CCS + 5 字节 encrypted_handshake 头。
const TLS_SERVER_HEAD: usize = 107;
/// 服务端 Hello 的固定长度（`sizeof(struct tls_server_hello)`，packed）。
const TLS_SERVER_HELLO: usize = 96;
/// 客户端 Hello 的固定长度（`sizeof(struct tls_client_hello)`，packed）。
const TLS_CLIENT_HELLO: usize = 138;
/// 单帧载荷上限（`deobfs_app_data` 里 `frame->len > 16384 ⇒ OBFS_ERROR`）。
const MAX_FRAME: usize = 16384;

/// `tls_client_hello_template.cipher_suites`（56 字节，逐字照抄）。
const TLS_CIPHER_SUITES: [u8; 56] = [
    0xc0, 0x2c, 0xc0, 0x30, 0x00, 0x9f, 0xcc, 0xa9, 0xcc, 0xa8, 0xcc, 0xaa, 0xc0, 0x2b, 0xc0, 0x2f,
    0x00, 0x9e, 0xc0, 0x24, 0xc0, 0x28, 0x00, 0x6b, 0xc0, 0x23, 0xc0, 0x27, 0x00, 0x67, 0xc0, 0x0a,
    0xc0, 0x14, 0x00, 0x39, 0xc0, 0x09, 0xc0, 0x13, 0x00, 0x33, 0x00, 0x9d, 0x00, 0x9c, 0x00, 0x3d,
    0x00, 0x3c, 0x00, 0x35, 0x00, 0x2f, 0x00, 0xff,
];

/// `tls_ext_others_template`（66 字节，逐字照抄）：
/// ec_point_formats + elliptic_curves + sig_algos + encrypt_then_mac + extended_master_secret。
const TLS_OTHER_EXTS: [u8; 66] = [
    0x00, 0x0B, 0x00, 0x04, 0x03, 0x01, 0x00, 0x02, // ec_point_formats
    0x00, 0x0A, 0x00, 0x0A, 0x00, 0x08, 0x00, 0x1d, 0x00, 0x17, 0x00, 0x19, 0x00,
    0x18, // elliptic_curves
    0x00, 0x0D, 0x00, 0x20, 0x00, 0x1E, // sig_algos
    0x06, 0x01, 0x06, 0x02, 0x06, 0x03, 0x05, 0x01, 0x05, 0x02, 0x05, 0x03, 0x04, 0x01, 0x04, 0x02,
    0x04, 0x03, 0x03, 0x01, 0x03, 0x02, 0x03, 0x03, 0x02, 0x01, 0x02, 0x02, 0x02, 0x03, 0x00, 0x16,
    0x00, 0x00, // encrypt_then_mac
    0x00, 0x17, 0x00, 0x00, // extended_master_secret
];

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Http,
    Tls,
}

/// 读方向的解析阶段（`simple-obfs` 的 `deobfs_stage` 同构，但能跨 `read` 边界）。
#[derive(Clone, Copy)]
enum ReadPhase {
    HttpHeader,
    /// `obfs=http` 剥掉响应头之后：剩余字节是裸流（不能再按 `\r\n\r\n` 找）。
    HttpBody,
    TlsServerHead,
    TlsFirstPayload(usize),
    TlsFrames,
}

pub struct ObfsStream {
    inner: BoxStream,
    mode: Mode,
    host: String,
    /// 首段是否已经写过（simple-obfs 的 `obfs_stage`）。
    wrote_first: bool,
    phase: ReadPhase,
    /// 已从 `inner` 读到、还没消化的密文。
    pending: Vec<u8>,
    /// 已解出、还没交给调用方的明文。
    plain: Vec<u8>,
}

/// 把一条**裸 TCP** 流包成 simple-obfs 的流（顺序：TCP → obfs → SS）。
pub fn wrap(stream: BoxStream, options: &SsObfs) -> Result<BoxStream, String> {
    let mode = match options.mode.trim().to_ascii_lowercase().as_str() {
        "http" => Mode::Http,
        "tls" => Mode::Tls,
        other => {
            return Err(format!(
                "SS 插件 simple-obfs 的 obfs={other} 不支持（只认 http / tls；已跳过）"
            ))
        }
    };
    let host = options.host.trim();
    let host = if host.is_empty() {
        return Err("SS 插件 simple-obfs 缺 obfs-host（已跳过）".to_string());
    } else {
        host.to_string()
    };
    Ok(Box::new(ObfsStream {
        inner: stream,
        mode,
        host,
        wrote_first: false,
        phase: match mode {
            Mode::Http => ReadPhase::HttpHeader,
            Mode::Tls => ReadPhase::TlsServerHead,
        },
        pending: Vec::new(),
        plain: Vec::new(),
    }))
}

impl ObfsStream {
    /// 把 `pending` 里的密文尽量消化成 `plain`；数据不够就原样留着等下一次 read。
    fn process(&mut self) -> io::Result<()> {
        loop {
            match self.phase {
                ReadPhase::HttpHeader => {
                    let Some(pos) = find_subsequence(&self.pending, b"\r\n\r\n") else {
                        return Ok(());
                    };
                    self.pending.drain(..pos + 4);
                    self.phase = ReadPhase::HttpBody;
                }
                ReadPhase::HttpBody => {
                    self.plain.extend_from_slice(&self.pending);
                    self.pending.clear();
                    return Ok(());
                }
                ReadPhase::TlsServerHead => {
                    if !self.pending.is_empty() && self.pending[0] != 0x16 {
                        return Err(invalid(
                            "simple-obfs 的 TLS 响应首字节不是 0x16（对端不是 obfs 服务端？）",
                        ));
                    }
                    if self.pending.len() < TLS_SERVER_HEAD {
                        return Ok(());
                    }
                    let msg_len = u16::from_be_bytes([
                        self.pending[TLS_SERVER_HELLO + 6 + 3],
                        self.pending[TLS_SERVER_HELLO + 6 + 4],
                    ]) as usize;
                    if msg_len > MAX_FRAME {
                        return Err(invalid("simple-obfs 的 TLS 首段长度超过 16384"));
                    }
                    self.pending.drain(..TLS_SERVER_HEAD);
                    self.phase = ReadPhase::TlsFirstPayload(msg_len);
                }
                ReadPhase::TlsFirstPayload(left) => {
                    if left == 0 {
                        self.phase = ReadPhase::TlsFrames;
                        continue;
                    }
                    let take = left.min(self.pending.len());
                    if take == 0 {
                        return Ok(());
                    }
                    self.plain.extend_from_slice(&self.pending[..take]);
                    self.pending.drain(..take);
                    self.phase = ReadPhase::TlsFirstPayload(left - take);
                }
                ReadPhase::TlsFrames => {
                    if self.pending.len() < 5 {
                        return Ok(());
                    }
                    if self.pending[..3] != TLS_APP_HEADER {
                        return Err(invalid("simple-obfs 的 TLS 帧头不是 17 03 03"));
                    }
                    let len = u16::from_be_bytes([self.pending[3], self.pending[4]]) as usize;
                    if len > MAX_FRAME {
                        return Err(invalid("simple-obfs 的 TLS 帧长度超过 16384"));
                    }
                    if self.pending.len() < 5 + len {
                        return Ok(());
                    }
                    self.plain.extend_from_slice(&self.pending[5..5 + len]);
                    self.pending.drain(..5 + len);
                }
            }
        }
    }

    fn http_header(&self, len: usize) -> String {
        let mut key = [0u8; 16];
        let _ = getrandom::getrandom(&mut key);
        let key = base64::engine::general_purpose::STANDARD.encode(key);
        // 与 `http_request_template` 同字段、同顺序（User-Agent 用固定的 curl 版本，
        // 服务端只校验 Upgrade / Host，不校验版本号）。
        format!(
            "GET / HTTP/1.1\r\nHost: {}\r\nUser-Agent: curl/7.61.0\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {}\r\nContent-Length: {}\r\n\r\n",
            self.host, key, len
        )
    }

    /// 首段：把载荷藏进假 ClientHello 的 `session_ticket` 扩展。
    fn tls_hello(&self, payload: &[u8]) -> Result<Vec<u8>, String> {
        let host = self.host.as_bytes();
        let total = TLS_CLIENT_HELLO + 4 + payload.len() + 9 + host.len() + TLS_OTHER_EXTS.len();
        if total > u16::MAX as usize {
            return Err("simple-obfs 的首段太长（超过 64 KiB）".to_string());
        }
        let mut out = Vec::with_capacity(total);
        out.push(0x16); // content_type: handshake
        out.extend_from_slice(&0x0301u16.to_be_bytes()); // record version
        out.extend_from_slice(&((total - 5) as u16).to_be_bytes()); // record len
        out.push(0x01); // handshake_type: ClientHello
        out.push(0);
        out.extend_from_slice(&((total - 9) as u16).to_be_bytes()); // handshake len
        out.extend_from_slice(&0x0303u16.to_be_bytes()); // client version
        out.extend_from_slice(&(now_secs() as u32).to_be_bytes());
        let mut random = [0u8; 28 + 32];
        getrandom::getrandom(&mut random).map_err(|_| "simple-obfs 生成随机数失败".to_string())?;
        out.extend_from_slice(&random[..28]);
        out.push(32); // session_id_len
        out.extend_from_slice(&random[28..]);
        out.extend_from_slice(&(TLS_CIPHER_SUITES.len() as u16).to_be_bytes());
        out.extend_from_slice(&TLS_CIPHER_SUITES);
        out.push(1); // comp_methods_len
        out.push(0); // null compression
        let ext_len = 4 + payload.len() + 9 + host.len() + TLS_OTHER_EXTS.len();
        out.extend_from_slice(&(ext_len as u16).to_be_bytes());
        // session_ticket（首段载荷藏在这里）
        out.extend_from_slice(&0x0023u16.to_be_bytes());
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        out.extend_from_slice(payload);
        // SNI
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&((host.len() + 5) as u16).to_be_bytes());
        out.extend_from_slice(&((host.len() + 3) as u16).to_be_bytes());
        out.push(0);
        out.extend_from_slice(&(host.len() as u16).to_be_bytes());
        out.extend_from_slice(host);
        out.extend_from_slice(&TLS_OTHER_EXTS);
        debug_assert_eq!(out.len(), total);
        Ok(out)
    }
}

impl Read for ObfsStream {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            if !self.plain.is_empty() {
                let take = out.len().min(self.plain.len());
                out[..take].copy_from_slice(&self.plain[..take]);
                self.plain.drain(..take);
                return Ok(take);
            }
            let mut chunk = [0u8; 16384];
            let read = self.inner.read(&mut chunk)?;
            if read == 0 {
                // 对端关闭：把尚未消化的字节丢掉（没有明文可交了）。
                return Ok(0);
            }
            self.pending.extend_from_slice(&chunk[..read]);
            self.process()?;
        }
    }
}

impl Write for ObfsStream {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if !self.wrote_first {
            self.wrote_first = true;
            match self.mode {
                Mode::Http => {
                    let header = self.http_header(data.len());
                    self.inner.write_all(header.as_bytes())?;
                    self.inner.write_all(data)?;
                }
                Mode::Tls => {
                    let hello = self
                        .tls_hello(data)
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
                    self.inner.write_all(&hello)?;
                }
            }
            return Ok(data.len());
        }
        match self.mode {
            // http 模式：首段之后就是裸流。
            Mode::Http => self.inner.write(data),
            // tls 模式：每段一个 5 字节头的应用数据帧（超过 16384 就拆帧）。
            Mode::Tls => {
                let mut written = 0;
                while written < data.len() {
                    let end = (written + MAX_FRAME).min(data.len());
                    let mut frame = Vec::with_capacity(5 + end - written);
                    frame.extend_from_slice(&TLS_APP_HEADER);
                    frame.extend_from_slice(&((end - written) as u16).to_be_bytes());
                    frame.extend_from_slice(&data[written..end]);
                    self.inner.write_all(&frame)?;
                    written = end;
                }
                Ok(data.len())
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Stream for ObfsStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_read_timeout(timeout)
    }
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::thread;

    /// `obfs=http`：请求行 / 关键头 / `Content-Length` 逐字对，且响应头被整体剥掉。
    #[test]
    fn http_obfs_prepends_an_upgrade_request_and_strips_the_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // simple-obfs 服务端连上就先发一个空的 101。
            stream
                .write_all(
                    b"HTTP/1.1 101 Switching Protocols\r\nServer: nginx/1.15.8\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: abc\r\n\r\n",
                )
                .unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            loop {
                stream.read_exact(&mut byte).unwrap();
                head.push(byte[0]);
                if head.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let text = String::from_utf8(head).unwrap();
            assert!(text.starts_with("GET / HTTP/1.1\r\n"), "{text}");
            assert!(text.contains("Host: cdn.example.com\r\n"), "{text}");
            assert!(text.contains("Upgrade: websocket\r\n"), "{text}");
            assert!(text.contains("Connection: Upgrade\r\n"), "{text}");
            assert!(text.contains("Sec-WebSocket-Key: "), "{text}");
            assert!(text.contains("Content-Length: 5\r\n"), "{text}");
            let mut payload = [0u8; 5];
            stream.read_exact(&mut payload).unwrap();
            assert_eq!(&payload, b"hello");
            // 之后是裸流。
            stream.write_all(b"world").unwrap();
            let _ = stream.shutdown(Shutdown::Write);
            thread::sleep(Duration::from_millis(50));
        });

        let tcp = TcpStream::connect(addr).unwrap();
        let stream: BoxStream = Box::new(tcp);
        let mut obfs = wrap(
            stream,
            &SsObfs {
                mode: "http".to_string(),
                host: "cdn.example.com".to_string(),
            },
        )
        .unwrap();
        obfs.write_all(b"hello").unwrap();
        let mut buffer = [0u8; 5];
        obfs.read_exact(&mut buffer).unwrap();
        assert_eq!(&buffer, b"world");
        handle.join().unwrap();
    }

    /// `obfs=tls`：ClientHello 的三个长度字段 / session_ticket / SNI 逐字节校验，
    /// 回程 107 字节服务端头 + 首段 + 应用数据帧。
    #[test]
    fn tls_obfs_hides_the_first_payload_in_the_session_ticket() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut hello = [0u8; TLS_CLIENT_HELLO];
            stream.read_exact(&mut hello).unwrap();
            assert_eq!(hello[0], 0x16);
            assert_eq!(&hello[1..3], &[0x03, 0x01]);
            assert_eq!(hello[5], 0x01, "handshake_type 必须是 ClientHello");
            assert_eq!(&hello[9..11], &[0x03, 0x03]);
            assert_eq!(hello[43], 32, "session_id_len 必须是 32");
            assert_eq!(
                u16::from_be_bytes([hello[76], hello[77]]) as usize,
                TLS_CIPHER_SUITES.len()
            );

            let mut ticket = [0u8; 4];
            stream.read_exact(&mut ticket).unwrap();
            assert_eq!(&ticket[..2], &[0x00, 0x23], "session_ticket 扩展必须排第一");
            let ticket_len = u16::from_be_bytes([ticket[2], ticket[3]]) as usize;
            assert_eq!(ticket_len, 5);
            let mut payload = [0u8; 5];
            stream.read_exact(&mut payload).unwrap();
            assert_eq!(&payload, b"hello");

            let mut sni = [0u8; 9];
            stream.read_exact(&mut sni).unwrap();
            assert_eq!(&sni[..2], &[0x00, 0x00], "第二个扩展必须是 SNI");
            let host_len = u16::from_be_bytes([sni[7], sni[8]]) as usize;
            assert_eq!(host_len, "cdn.example.com".len());
            let mut host = vec![0u8; host_len];
            stream.read_exact(&mut host).unwrap();
            assert_eq!(&host, b"cdn.example.com");
            let mut others = [0u8; TLS_OTHER_EXTS.len()];
            stream.read_exact(&mut others).unwrap();
            assert_eq!(others, TLS_OTHER_EXTS);

            // 回程：96 ServerHello + 6 CCS + 5 encrypted_handshake 头（len=首段长度）+ 首段。
            let mut server_head = vec![0u8; TLS_SERVER_HELLO];
            server_head[0] = 0x16;
            server_head[1..3].copy_from_slice(&0x0301u16.to_be_bytes());
            server_head[5] = 2;
            let ccs = [0x14, 0x03, 0x03, 0x00, 0x01, 0x01];
            let mut encrypted = [0x16, 0x03, 0x03, 0x00, 0x00];
            // 首段 5 字节（与客户端首段无关，服务端可以自由决定）。
            encrypted[3..5].copy_from_slice(&5u16.to_be_bytes());
            stream.write_all(&server_head).unwrap();
            stream.write_all(&ccs).unwrap();
            stream.write_all(&encrypted).unwrap();
            stream.write_all(b"world").unwrap();

            // 第二段客户端数据必须带 17 03 03 + 长度。
            let mut frame = [0u8; 5];
            stream.read_exact(&mut frame).unwrap();
            assert_eq!(&frame[..3], &TLS_APP_HEADER);
            let len = u16::from_be_bytes([frame[3], frame[4]]) as usize;
            assert_eq!(len, 3);
            let mut body = [0u8; 3];
            stream.read_exact(&mut body).unwrap();
            assert_eq!(&body, b"abc");
            let mut reply = vec![0x17, 0x03, 0x03];
            reply.extend_from_slice(&3u16.to_be_bytes());
            reply.extend_from_slice(b"xyz");
            stream.write_all(&reply).unwrap();
            let _ = stream.shutdown(Shutdown::Write);
            thread::sleep(Duration::from_millis(50));
        });

        let tcp = TcpStream::connect(addr).unwrap();
        let stream: BoxStream = Box::new(tcp);
        let mut obfs = wrap(
            stream,
            &SsObfs {
                mode: "tls".to_string(),
                host: "cdn.example.com".to_string(),
            },
        )
        .unwrap();
        obfs.write_all(b"hello").unwrap();
        obfs.write_all(b"abc").unwrap();
        let mut buffer = [0u8; 5];
        obfs.read_exact(&mut buffer).unwrap();
        assert_eq!(&buffer, b"world");
        let mut tail = [0u8; 3];
        obfs.read_exact(&mut tail).unwrap();
        assert_eq!(&tail, b"xyz");
        handle.join().unwrap();
    }

    /// 未知混淆模式 / 缺 host 必须明确报错（不静默退回裸 SS）。
    #[test]
    fn unknown_modes_are_rejected_with_a_reason() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let tcp = TcpStream::connect(addr).unwrap();
        let stream: BoxStream = Box::new(tcp);
        let error = wrap(
            stream,
            &SsObfs {
                mode: "quic".to_string(),
                host: "a.com".to_string(),
            },
        )
        .err()
        .expect("未知 obfs 必须报错");
        assert!(error.contains("obfs=quic"), "{error}");

        let tcp = TcpStream::connect(addr).unwrap();
        let stream: BoxStream = Box::new(tcp);
        let error = wrap(
            stream,
            &SsObfs {
                mode: "http".to_string(),
                host: "  ".to_string(),
            },
        )
        .err()
        .expect("缺 host 必须报错");
        assert!(error.contains("obfs-host"), "{error}");
    }

    /// 帧头错位必须报错，而不是把垃圾当明文交出去。
    #[test]
    fn malformed_tls_frames_are_rejected() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut head = vec![0u8; TLS_SERVER_HEAD];
            head[0] = 0x16;
            // 首段长度 0，紧接着是一个坏帧头。
            stream.write_all(&head).unwrap();
            stream
                .write_all(&[0x17, 0x03, 0x02, 0x00, 0x01, 0x41])
                .unwrap();
            let _ = stream.shutdown(Shutdown::Write);
            thread::sleep(Duration::from_millis(50));
        });

        let tcp = TcpStream::connect(addr).unwrap();
        let stream: BoxStream = Box::new(tcp);
        let mut obfs = wrap(
            stream,
            &SsObfs {
                mode: "tls".to_string(),
                host: "a.com".to_string(),
            },
        )
        .unwrap();
        let mut buffer = [0u8; 8];
        let error = obfs.read(&mut buffer).err().expect("坏帧必须报错");
        assert!(error.to_string().contains("帧头"), "{error}");
        handle.join().unwrap();
    }
}
