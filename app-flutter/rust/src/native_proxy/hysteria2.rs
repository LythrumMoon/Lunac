//! Hysteria2（QUIC + HTTP/3 认证 + TCP 代理 + Salamander / Gecko 混淆）。
//!
//! 重建 v1 `native_proxy/hysteria2.rs`。**协议依据**（2026-11-15 逐条核对，不凭记忆写）：
//!   · 官方规范 <https://hysteria.network/docs/developers/Protocol/>（v2.0.0 起的 "v4" 协议：
//!     QUIC + RFC 9221 datagram）—— 认证请求头、`TCPRequest` 帧、`UDPMessage` 布局、
//!     fragmentation 语义、Salamander / Gecko 两种混淆；
//!   · 参考实现 `apernet/hysteria` `core/client/client.go` + `core/internal/protocol/{http,proxy}.go`
//!     @ `app/v2.0.0`（状态码 233 / `Hysteria-Auth` / `Hysteria-CC-RX` / `Hysteria-Padding` /
//!     `Hysteria-UDP`）。
//!
//! **为什么是「阻塞外壳 + 内部 tokio」**：本目录其余协议都是 std 阻塞 socket
//! （`transport::BoxStream = Box<dyn Stream: Read + Write + Send>`），而 QUIC 必须先有一个
//! 异步运行时。这里用一个**进程内共享**的多线程 runtime（`runtime()`）承载所有 Hysteria2
//! 连接：拨号 / 读 / 写各自 `block_on` 一小段 async，外面看仍然是普通的阻塞流 ——
//! 于是 `server.rs` 的 `pipe()` 一行都不用改。
//!
//! **读超时语义要对齐 `pipe()`**：`pipe()` 把 `WouldBlock` / `TimedOut` 当作「这一轮没数据」，
//! 所以 `set_read_timeout(Some(d))` 到期这里必须返回 `WouldBlock`（不能返回 EOF，否则长连接
//! 会被当成对端关闭）。`quinn::RecvStream::read` 是 cancel-safe 的，超时取消不会丢字节。
//!
//! **混淆放在一个 loopback 中继里**（`obfs.rs` 之外的第二个设计点）：Salamander / Gecko 必须
//! 拿到「一个个 UDP 包」，而 quinn 只肯通过自定义 `AsyncUdpSocket` 交出包。自己在 Windows 上
//! 实现 `AsyncUdpSocket` 要连带实现可写轮询（`UdpPoller`），风险高、收益低。这里的做法是：
//! quinn 连 `127.0.0.1:<本地端口>`，中继线程负责「本地 ↔ 服务器」并按包做混淆 / 解混淆
//! （Gecko 的分片重组也在这里）。代价是每个连接多一次 loopback 拷贝 ——
//! 换来的是 quinn 侧零自定义、混淆层全是纯函数、可单测。
//!
//! **如实登记的边界**：
//!   · 每条被代理的 TCP 连接都会新开一条 QUIC 连接（握手 + 认证各一次），没有连接复用池；
//!   · 只做 `insecure`（跳过证书校验）与系统根证书两种；Hysteria2 常见的自签证书节点必须
//!     在链接里带 `insecure=1`（与 v1 同口径）；
//!   · UDP 转发（`UDPMessage` + datagram）本文件已备好编解码与分片重组，但**尚未**接进
//!     `udp.rs` 的上游表 —— 未接线时如实报「未接线」，不假装支持；
//!   · Brutal 拥塞控制不实现（quinn 不开放可插拔 CC；认证时 `Hysteria-CC-RX` 恒发 0，
//!     让服务端按它自己的 CC 自适应，与 v1 的 G6 结论一致）。

use std::cell::Cell;
use std::io::{self, Read, Write};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::ServerName;

use super::quic::{
    client_tls, connection_error_text, read_error_text, read_exact_error_text, resolve, runtime,
};
use super::transport::{BoxStream, Stream};

/// Hysteria2 的 ALPN 恒为 `h3`（服务端把自己伪装成一个普通 HTTP/3 服务器）。
pub const ALPN: &[u8] = b"h3";

/// `TCPRequest` 的帧类型（QUIC varint）。
const FRAME_TYPE_TCP_REQUEST: u64 = 0x401;
/// 规范给的上限（防 DoS，与参考实现一致）。
const MAX_ADDRESS: usize = 2048;
const MAX_MESSAGE: usize = 2048;
const MAX_PADDING: usize = 4096;
/// 认证成功的 HTTP 状态码（规范里写死的 233）。
pub const STATUS_AUTH_OK: u16 = 233;
/// QUIC 空闲 / 保活（参考实现默认值：30s / 10s）。
const MAX_IDLE: Duration = Duration::from_secs(30);
const KEEP_ALIVE: Duration = Duration::from_secs(10);
/// 握手超时（与 `transport::DIAL_TIMEOUT` 同量级，但 QUIC 握手多一次往返）。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(12);

/// 一个 Hysteria2 节点（`mod.rs` 的 `Node` 去掉与 QUIC 无关的字段）。
#[derive(Clone, Debug, PartialEq)]
pub struct Hysteria2Config {
    pub server: String,
    pub port: u16,
    /// `sni`；空 = 用 `server`。
    pub sni: String,
    /// 跳过证书校验（自签证书节点的唯一可行方式）。
    pub insecure: bool,
    pub password: String,
    /// `""` / `"salamander"` / `"gecko"`（其余混淆由 `clash.rs` 在解析期就拒掉）。
    pub obfs: String,
    pub obfs_password: String,
}

impl Hysteria2Config {
    /// 实际用于 TLS 的 SNI（空 `sni` 回落到节点地址）。
    pub fn server_name(&self) -> &str {
        if self.sni.trim().is_empty() {
            &self.server
        } else {
            &self.sni
        }
    }

    /// 混淆层是否需要（`salamander` / `gecko` 都要先过 Salamander 那层）。
    pub fn obfs_enabled(&self) -> bool {
        matches!(self.obfs.as_str(), "salamander" | "gecko")
    }

    /// Gecko 在 Salamander 之上再分片。
    pub fn gecko(&self) -> bool {
        self.obfs == "gecko"
    }
}

/// 拨号：连节点 → HTTP/3 认证 → 开一条 stream 发 `TCPRequest` → 返回阻塞流。
pub fn connect(
    config: &Hysteria2Config,
    target_host: &str,
    target_port: u16,
) -> Result<BoxStream, String> {
    let authority = authority(target_host, target_port);
    let stream = runtime().block_on(connect_async(config, authority))?;
    Ok(Box::new(stream))
}

/// 「host:port」的 authority 形态：IPv6 必须加方括号（`[::1]:443`）。
pub fn authority(host: &str, port: u16) -> String {
    let bare = host.trim_matches(['[', ']']);
    if bare.contains(':') {
        format!("[{bare}]:{port}")
    } else {
        format!("{bare}:{port}")
    }
}

/// QUIC varint 编码（RFC 9000 §16）。
#[cfg(test)]
pub fn varint(value: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(8);
    write_varint(&mut out, value);
    out
}

fn write_varint(out: &mut Vec<u8>, value: u64) {
    if value <= 63 {
        out.push(value as u8);
    } else if value <= 16_383 {
        out.push(0x40 | (value >> 8) as u8);
        out.push(value as u8);
    } else if value <= 1_073_741_823 {
        out.push(0x80 | (value >> 24) as u8);
        out.push((value >> 16) as u8);
        out.push((value >> 8) as u8);
        out.push(value as u8);
    } else {
        out.push(0xc0 | (value >> 56) as u8);
        for shift in [48, 40, 32, 24, 16, 8, 0] {
            out.push((value >> shift) as u8);
        }
    }
}

/// `TCPRequest` 帧：`[varint 0x401][varint 地址长度][地址][varint 填充长度][填充]`。
pub fn tcp_request_frame(address: &str, padding: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + address.len() + padding.len());
    write_varint(&mut out, FRAME_TYPE_TCP_REQUEST);
    write_varint(&mut out, address.len() as u64);
    out.extend_from_slice(address.as_bytes());
    write_varint(&mut out, padding.len() as u64);
    out.extend_from_slice(padding);
    out
}

/// `UDPMessage`：`[u32 session][u16 packet][u8 frag][u8 fragCount][varint addrLen][addr][payload]`。
///
/// 未分片时规范要求 `fragCount = 1`（此时 packet / frag 无意义，置 0）。
pub fn udp_message(
    session: u32,
    packet: u16,
    frag: u8,
    frag_count: u8,
    address: &str,
    data: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + address.len() + data.len());
    out.extend_from_slice(&session.to_be_bytes());
    out.extend_from_slice(&packet.to_be_bytes());
    out.push(frag);
    out.push(frag_count);
    write_varint(&mut out, address.len() as u64);
    out.extend_from_slice(address.as_bytes());
    out.extend_from_slice(data);
    out
}

/// 解析 `UDPMessage`（服务端回来的 datagram）。字段越界一律拒收。
pub fn parse_udp_message(bytes: &[u8]) -> Result<(u32, u16, u8, u8, String, Vec<u8>), String> {
    if bytes.len() < 9 {
        return Err("UDPMessage 太短".to_string());
    }
    let session = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let packet = u16::from_be_bytes([bytes[4], bytes[5]]);
    let frag = bytes[6];
    let count = bytes[7];
    let (len, used) = read_varint(&bytes[8..])?;
    let len = len as usize;
    if len == 0 || len > MAX_ADDRESS {
        return Err("UDPMessage 地址长度非法".to_string());
    }
    let start = 8 + used;
    let end = start + len;
    if end > bytes.len() {
        return Err("UDPMessage 地址越界".to_string());
    }
    let address = String::from_utf8_lossy(&bytes[start..end]).to_string();
    Ok((session, packet, frag, count, address, bytes[end..].to_vec()))
}

/// 读一个 QUIC varint，返回（值，占用字节数）。
pub fn read_varint(bytes: &[u8]) -> Result<(u64, usize), String> {
    let first = *bytes.first().ok_or("varint 截断")?;
    let width = 1usize << (first >> 6);
    if bytes.len() < width {
        return Err("varint 截断".to_string());
    }
    let mut value = (first & 0x3f) as u64;
    for byte in &bytes[1..width] {
        value = (value << 8) | *byte as u64;
    }
    Ok((value, width))
}

// ── Salamander / Gecko 混淆（纯函数，便于单测） ──────────────────────────

/// Salamander 的包密钥：`BLAKE2b-256(key + salt)`（规范原文）。
pub fn salamander_key(psk: &[u8], salt: &[u8; 8]) -> [u8; 32] {
    use blake2::digest::consts::U32;
    use blake2::{Blake2b, Digest};
    let mut hasher = Blake2b::<U32>::new();
    hasher.update(psk);
    hasher.update(salt);
    let digest = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&digest);
    key
}

/// 加一层 Salamander：`[8 字节 salt][payload ^ BLAKE2b-256(psk + salt)]`。
pub fn salamander_seal(psk: &[u8], payload: &[u8], salt: [u8; 8]) -> Vec<u8> {
    let key = salamander_key(psk, &salt);
    let mut out = Vec::with_capacity(payload.len() + 8);
    out.extend_from_slice(&salt);
    for (index, byte) in payload.iter().enumerate() {
        out.push(byte ^ key[index % 32]);
    }
    out
}

/// 拆 Salamander：短于 8 字节 / 无 payload 的包一律丢弃（规范要求）。
pub fn salamander_open(psk: &[u8], packet: &[u8]) -> Result<Vec<u8>, String> {
    if packet.len() < 9 {
        return Err("Salamander 包太短".to_string());
    }
    let mut salt = [0u8; 8];
    salt.copy_from_slice(&packet[..8]);
    let key = salamander_key(psk, &salt);
    Ok(packet[8..]
        .iter()
        .enumerate()
        .map(|(index, byte)| byte ^ key[index % 32])
        .collect())
}

/// Gecko 帧：`[0x80][msgID][chunkIdx:4 | totalChunks:4][u16 padLen][padding][chunk]`。
pub fn gecko_frame(msg_id: u8, index: u8, total: u8, chunk: &[u8], padding: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + padding.len() + chunk.len());
    out.push(0x80);
    out.push(msg_id);
    out.push((index << 4) | (total & 0x0f));
    out.extend_from_slice(&(padding.len() as u16).to_be_bytes());
    out.extend_from_slice(padding);
    out.extend_from_slice(chunk);
    out
}

/// 拆 Gecko 帧；不是分片帧（首字节高位为 0）时返回 `None`。
pub fn parse_gecko_frame(packet: &[u8]) -> Result<Option<(u8, u8, u8, Vec<u8>)>, String> {
    if packet.first().is_none_or(|first| first & 0x80 == 0) {
        return Ok(None);
    }
    if packet.len() < 5 {
        return Err("Gecko 帧太短".to_string());
    }
    let msg_id = packet[1];
    let index = packet[2] >> 4;
    let total = packet[2] & 0x0f;
    if !(2..=8).contains(&total) || index >= total {
        return Err("Gecko 分片编号非法".to_string());
    }
    let pad_len = u16::from_be_bytes([packet[3], packet[4]]) as usize;
    let start = 5 + pad_len;
    if start > packet.len() {
        return Err("Gecko 填充越界".to_string());
    }
    Ok(Some((msg_id, index, total, packet[start..].to_vec())))
}

/// 把一条长报文切成 `total` 片（2..=8），返回 `(index, chunk)`。
pub fn gecko_split(packet: &[u8], total: u8) -> Vec<(u8, Vec<u8>)> {
    let total = total.clamp(2, 8) as usize;
    let chunk = packet.len().div_ceil(total);
    if chunk == 0 {
        return vec![(0, Vec::new())];
    }
    packet
        .chunks(chunk)
        .enumerate()
        .map(|(index, part)| (index as u8, part.to_vec()))
        .collect()
}

// ── 阻塞流 ──────────────────────────────────────────────────────────────

/// 认证用的 HTTP/3 客户端句柄。
///
/// **为什么必须一直持有**（2026-11-15 真机定位到的根因）：`h3` 的 `SendRequest` 带一个
/// `sender_count` 引用计数，**最后一个 `SendRequest` 被 Drop 时会主动关掉整条 QUIC 连接**
/// （h3 内部以 `H3_NO_ERROR / "Connection closed by client"` 调 `h3_quinn` 的 `close`，
/// 后者直接 `quinn::Connection::close`）。认证一返回就丢掉句柄 = 认证刚过就把连接掐了：
/// 客户端看到的下一句是 `open_bi` 的 `closed`（`LocallyClosed`），服务端则记一条
/// `Application error 0x100 (remote): Connection closed by client`。
///
/// 于是这里把它当成「连接的一部分」持有到流 / 会话结束（Hysteria2 之后不再需要 HTTP/3，
/// 但句柄不能放）。
type AuthHandle = h3::client::SendRequest<h3_quinn::OpenStreams, bytes::Bytes>;

/// 一条 Hysteria2 的 TCP 隧道（QUIC bidi stream 的阻塞外壳）。
pub struct Hysteria2Stream {
    /// 保活：`Endpoint` / `Connection` 掉了，stream 立刻作废。
    _endpoint: quinn::Endpoint,
    /// 同理：`Connection` 必须活到流结束（也留给以后的 UDP 转发用）。
    _conn: quinn::Connection,
    /// 认证句柄必须活到流结束，否则它被 Drop 时会关掉整条 QUIC 连接（见 `AuthHandle`）。
    _auth: AuthHandle,
    /// 有混淆时才有：`Drop` 时会让中继线程退出。
    _obfs: Option<ObfsRelay>,
    send: quinn::SendStream,
    recv: quinn::RecvStream,
    read_timeout: Cell<Option<Duration>>,
}

impl Read for Hysteria2Stream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let timeout = self.read_timeout.get();
        let recv = &mut self.recv;
        // `None` = 读超时（到期）；`Some(..)` = quinn 的真实结果。
        let result: Option<Result<Option<usize>, quinn::ReadError>> = runtime().block_on(async {
            match timeout {
                Some(limit) => tokio::time::timeout(limit, recv.read(buffer)).await.ok(),
                None => Some(recv.read(buffer).await),
            }
        });
        match result {
            // 到期 = 「这一轮没数据」，与 TCP 的 SO_RCVTIMEO 同义。
            None => Err(io::Error::new(io::ErrorKind::WouldBlock, "读超时")),
            Some(Ok(Some(read))) => Ok(read),
            Some(Ok(None)) => Ok(0),
            Some(Err(error)) => Err(io::Error::new(
                io::ErrorKind::Other,
                read_error_text(&error),
            )),
        }
    }
}

impl Write for Hysteria2Stream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let send = &mut self.send;
        runtime()
            .block_on(async { send.write(buffer).await })
            .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Stream for Hysteria2Stream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.read_timeout.set(timeout);
        Ok(())
    }
}

// ── 真正的异步拨号 ──────────────────────────────────────────────────────

/// 一条已握手 + 已认证的 Hysteria2 QUIC 连接（TCP 流 / UDP 会话共用的那部分）。
///
/// 每个字段都是「必须活到连接结束」的：`Endpoint` / `Connection` 掉了连接就断；
/// `auth` 掉了 h3 会主动关连接（见 `AuthHandle`）；`obfs` 掉了中继线程会退出。
struct Handshake {
    endpoint: quinn::Endpoint,
    conn: quinn::Connection,
    /// 服务端是否开了 UDP 转发（认证回包的 `Hysteria-UDP`）。
    udp_enabled: bool,
    auth: AuthHandle,
    obfs: Option<ObfsRelay>,
}

/// QUIC 握手 + HTTP/3 认证（TCP / UDP 两条路径共用）。
async fn handshake_async(config: &Hysteria2Config) -> Result<Handshake, String> {
    let remote = resolve(&config.server, config.port)?;
    // sni 必须能被 rustls 接受（IP 也合法）；非法要在拨号前就报出来。
    ServerName::try_from(config.server_name().to_string())
        .map_err(|e| format!("Hysteria2 的 sni 非法：{e}"))?;

    let tls = Arc::new(build_tls(config)?);
    let quic_crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls)
        .map_err(|e| format!("Hysteria2 的 TLS 配置无法用于 QUIC：{e}"))?;
    let mut client_config = quinn::ClientConfig::new(Arc::new(quic_crypto));
    let mut transport = quinn::TransportConfig::default();
    transport
        .max_idle_timeout(Some(
            MAX_IDLE
                .try_into()
                .map_err(|_| "空闲超时非法".to_string())?,
        ))
        .keep_alive_interval(Some(KEEP_ALIVE))
        // RFC 9221（UDP 转发要用；不开也允许，但开了才能承载 datagram）。
        .datagram_receive_buffer_size(Some(1 << 20))
        .datagram_send_buffer_size(1 << 20)
        // 走 loopback 中继时 PMTU 探测只会量到 127.0.0.1；固定 1200 才不会被真实链路丢包。
        .initial_mtu(1200)
        .mtu_discovery_config(None);
    client_config.transport_config(Arc::new(transport));

    // 绑本地端口：有混淆时绑 127.0.0.1 给中继用，没混淆时直接对外。
    let bind: SocketAddr = if config.obfs_enabled() {
        "127.0.0.1:0".parse().expect("回环地址恒可解析")
    } else {
        match remote {
            SocketAddr::V4(_) => "0.0.0.0:0".parse().expect("IPv4 通配地址恒可解析"),
            SocketAddr::V6(_) => "[::]:0".parse().expect("IPv6 通配地址恒可解析"),
        }
    };
    let mut endpoint =
        quinn::Endpoint::client(bind).map_err(|e| format!("Hysteria2 绑定本地端口失败：{e}"))?;
    endpoint.set_default_client_config(client_config);

    let relay = if config.obfs_enabled() {
        // 中继把 127.0.0.1:<port> 上的包按混淆格式转给真实服务器。
        Some(ObfsRelay::start(
            endpoint
                .local_addr()
                .map_err(|e| format!("读本地端口失败：{e}"))?,
            remote,
            config,
        )?)
    } else {
        None
    };
    let dial_addr = relay.as_ref().map(ObfsRelay::address).unwrap_or(remote);

    let connecting = endpoint
        .connect(dial_addr, config.server_name())
        .map_err(|e| format!("Hysteria2 发起 QUIC 连接失败：{e}"))?;
    let conn = tokio::time::timeout(HANDSHAKE_TIMEOUT, connecting)
        .await
        .map_err(|_| format!("Hysteria2 的 QUIC 握手超时（{HANDSHAKE_TIMEOUT:?}）"))?
        .map_err(|e| format!("Hysteria2 的 QUIC 握手失败：{e}"))?;

    // 认证：`POST https://hysteria/auth`，成功 = 状态码 233；回包 `Hysteria-UDP` 决定能不能转发 UDP。
    // 句柄要带出去并由调用方持有（见 `AuthHandle`：丢掉它 = 关掉这条 QUIC 连接）。
    let (udp_enabled, auth) = authenticate(&conn, &config.password).await?;
    Ok(Handshake {
        endpoint,
        conn,
        udp_enabled,
        auth,
        obfs: relay,
    })
}

async fn connect_async(
    config: &Hysteria2Config,
    authority: String,
) -> Result<Hysteria2Stream, String> {
    let Handshake {
        endpoint,
        conn,
        auth,
        obfs,
        ..
    } = handshake_async(config).await?;

    // 开一条 bidi stream，发 TCPRequest，读 TCPResponse。
    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| format!("Hysteria2 开流失败：{}", connection_error_text(&e)))?;
    let request = tcp_request_frame(&authority, &random_padding(16, 64));
    send.write_all(&request)
        .await
        .map_err(|e| format!("Hysteria2 发 TCPRequest 失败：{e}"))?;

    let (ok, message) = read_tcp_response(&mut recv).await?;
    if !ok {
        return Err(format!(
            "Hysteria2 服务端拒绝转发 {authority}：{}",
            if message.is_empty() {
                "（无原因）".to_string()
            } else {
                message
            }
        ));
    }

    Ok(Hysteria2Stream {
        _endpoint: endpoint,
        _conn: conn,
        _auth: auth,
        _obfs: obfs,
        send,
        recv,
        read_timeout: Cell::new(None),
    })
}

// ── UDP 转发（RFC 9221 datagram + `UDPMessage` 分片）────────────────────

/// 一条 Hysteria2 的 UDP 转发会话（一个目标一条会话，Session ID 随机且唯一）。
pub struct Hysteria2Udp {
    conn: quinn::Connection,
    session: u32,
    packets: u16,
    /// 单条 datagram 能装多少字节（含 `UDPMessage` 头）；超出即分片。
    max_datagram: usize,
    incoming: std::sync::mpsc::Receiver<(String, u16, Vec<u8>)>,
    _endpoint: quinn::Endpoint,
    /// 认证句柄（丢掉它会关掉整条 QUIC 连接，见 `AuthHandle`）。
    _auth: AuthHandle,
    /// 有混淆时才有：`Drop` 时会让中继线程退出。
    _obfs: Option<ObfsRelay>,
    _reader: std::thread::JoinHandle<()>,
}

/// 拨一条 UDP 会话：握手 + 认证 + 起一个收包线程。
pub fn connect_udp(config: &Hysteria2Config) -> Result<Hysteria2Udp, String> {
    runtime().block_on(connect_udp_async(config))
}

async fn connect_udp_async(config: &Hysteria2Config) -> Result<Hysteria2Udp, String> {
    let Handshake {
        endpoint,
        conn,
        udp_enabled,
        auth,
        obfs,
    } = handshake_async(config).await?;
    if !udp_enabled {
        return Err(
            "Hysteria2 服务端没有启用 UDP 转发（认证回包 Hysteria-UDP 不是 true）".to_string(),
        );
    }
    let max_datagram = conn
        .max_datagram_size()
        .ok_or_else(|| "Hysteria2 的连接没有 datagram 预算（对端不支持 RFC 9221）".to_string())?;
    let session = random_u32();
    let (sender, incoming) = std::sync::mpsc::channel();
    let reader_conn = conn.clone();
    // 收包放在独立线程：`read_datagram()` 被 `block_on` 取消时可能丢包，交给常驻循环最稳。
    let reader = std::thread::Builder::new()
        .name("lunac-hysteria2-udp".to_string())
        .spawn(move || {
            let mut reassembly = UdpReassembly::default();
            runtime().block_on(async move {
                loop {
                    match reader_conn.read_datagram().await {
                        Ok(datagram) => {
                            if let Some(item) = reassembly.push(&datagram) {
                                if sender.send(item).is_err() {
                                    break;
                                }
                            }
                        }
                        // 连接关闭 / 被 reset：收包线程收摊。
                        Err(_) => break,
                    }
                }
            });
        })
        .map_err(|e| format!("Hysteria2 UDP 收包线程启动失败：{e}"))?;

    Ok(Hysteria2Udp {
        conn,
        session,
        packets: 0,
        max_datagram,
        incoming,
        _endpoint: endpoint,
        _auth: auth,
        _obfs: obfs,
        _reader: reader,
    })
}

impl Hysteria2Udp {
    /// 发一个数据报（超过 QUIC datagram 预算时按规范分片）。
    pub fn send(&mut self, host: &str, port: u16, payload: &[u8]) -> std::io::Result<()> {
        let address = authority(host, port);
        let packet = self.packets;
        self.packets = self.packets.wrapping_add(1);
        for frame in udp_fragments(self.session, packet, &address, payload, self.max_datagram) {
            let bytes = bytes::Bytes::from(frame);
            runtime()
                .block_on(self.conn.send_datagram_wait(bytes))
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        }
        Ok(())
    }

    /// 收一个数据报（`(host, port, payload)`）；`Ok(None)` = 暂时没有。
    pub fn recv(&mut self) -> std::io::Result<Option<(String, u16, Vec<u8>)>> {
        match self.incoming.try_recv() {
            Ok(item) => Ok(Some(item)),
            Err(std::sync::mpsc::TryRecvError::Empty) => Ok(None),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Hysteria2 UDP 会话已断开",
            )),
        }
    }
}

/// 把一条 payload 切成若干 `UDPMessage`：放得下就一片（`fragCount=1`），否则同 `packetID` 分片。
pub fn udp_fragments(
    session: u32,
    packet: u16,
    address: &str,
    payload: &[u8],
    max_datagram: usize,
) -> Vec<Vec<u8>> {
    let header = udp_message(session, packet, 0, 1, address, &[]).len();
    // 至少留 64 字节载荷，避免地址很长时退化成无限多片。
    let budget = max_datagram.saturating_sub(header + 1).max(64);
    if payload.len() <= budget {
        return vec![udp_message(session, packet, 0, 1, address, payload)];
    }
    let parts: Vec<&[u8]> = payload.chunks(budget).take(255).collect();
    let total = parts.len() as u8;
    parts
        .iter()
        .enumerate()
        .map(|(index, part)| udp_message(session, packet, index as u8, total, address, part))
        .collect()
}

/// 收包侧的分片重组（同一 `(SessionID, PacketID)` 收齐才交付；缺片整包丢弃）。
#[derive(Default)]
pub struct UdpReassembly {
    pending: std::collections::HashMap<(u32, u16), (u8, Vec<Option<Vec<u8>>>, std::time::Instant)>,
}

impl UdpReassembly {
    /// 喂一个 datagram；凑齐一整包时返回 `(host, port, payload)`。
    pub fn push(&mut self, datagram: &[u8]) -> Option<(String, u16, Vec<u8>)> {
        self.purge();
        let (session, packet, frag, count, address, data) = parse_udp_message(datagram).ok()?;
        let (host, port) = split_authority(&address)?;
        if count <= 1 {
            return Some((host, port, data));
        }
        let entry = self
            .pending
            .entry((session, packet))
            .or_insert_with(|| (count, vec![None; count as usize], std::time::Instant::now()));
        // 同一个 PacketID 的分片数变了 ⇒ 视为脏数据，整包丢。
        if entry.0 != count || frag >= count {
            self.pending.remove(&(session, packet));
            return None;
        }
        entry.1[frag as usize] = Some(data);
        if entry.1.iter().any(|slot| slot.is_none()) {
            return None;
        }
        let (_, slots, _) = self.pending.remove(&(session, packet))?;
        Some((host, port, slots.into_iter().flatten().flatten().collect()))
    }

    /// 半包最多留 5 秒（规范只要求有界，具体阈值由实现决定）。
    fn purge(&mut self) {
        self.pending
            .retain(|_, (_, _, at)| at.elapsed() < std::time::Duration::from_secs(5));
    }
}

/// `host:port` → `(host, port)`；IPv6 支持方括号形态。
pub fn split_authority(address: &str) -> Option<(String, u16)> {
    if let Some(rest) = address.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = tail.strip_prefix(':')?.parse().ok()?;
        return Some((host.to_string(), port));
    }
    let (host, port) = address.rsplit_once(':')?;
    Some((host.to_string(), port.parse().ok()?))
}

fn random_u32() -> u32 {
    let mut seed = [0u8; 4];
    let _ = getrandom::getrandom(&mut seed);
    u32::from_le_bytes(seed)
}

fn build_tls(config: &Hysteria2Config) -> Result<rustls::ClientConfig, String> {
    client_tls(config.insecure, &[ALPN.to_vec()])
}

/// HTTP/3 认证；返回「服务端是否开了 UDP」与**必须由调用方持有的**认证句柄。
async fn authenticate(
    conn: &quinn::Connection,
    password: &str,
) -> Result<(bool, AuthHandle), String> {
    let (mut driver, mut send_request) = h3::client::new(h3_quinn::Connection::new(conn.clone()))
        .await
        .map_err(|e| format!("Hysteria2 的 HTTP/3 建连失败：{e}"))?;
    // 驱动必须常驻：认证之后这条 QUIC 连接上还要跑 TCP 流的控制帧。
    tokio::spawn(async move {
        let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
    });

    let request = http::Request::builder()
        .method(http::Method::POST)
        .uri("https://hysteria/auth")
        .header("hysteria-auth", password)
        // 0 = 「客户端不知道自己的带宽」，让服务端用它自己的拥塞控制（不用 Brutal）。
        .header("hysteria-cc-rx", "0")
        .header("hysteria-padding", base64_padding())
        .body(())
        .map_err(|e| format!("Hysteria2 认证请求构造失败：{e}"))?;

    let mut stream = send_request
        .send_request(request)
        .await
        .map_err(|e| format!("Hysteria2 发认证请求失败：{e}"))?;
    stream
        .finish()
        .await
        .map_err(|e| format!("Hysteria2 认证请求收尾失败：{e}"))?;
    let response = stream
        .recv_response()
        .await
        .map_err(|e| format!("Hysteria2 读认证响应失败：{e}"))?;
    let status = response.status().as_u16();
    if status != STATUS_AUTH_OK {
        return Err(format!(
            "Hysteria2 认证失败（HTTP {status}）：密码 / 端口 / 混淆参数对不上，或服务端未开代理"
        ));
    }
    let udp = response
        .headers()
        .get("hysteria-udp")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    // 句柄原样交回去：Drop 它 = 关掉这条 QUIC 连接（见 `AuthHandle`）。
    Ok((udp, send_request))
}

fn base64_padding() -> String {
    use base64::Engine as _;
    let bytes = random_padding(16, 64);
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// 读 `TCPResponse`：`[u8 status][varint 长度][消息][varint 填充长度][填充]`。
async fn read_tcp_response(recv: &mut quinn::RecvStream) -> Result<(bool, String), String> {
    let mut status = [0u8; 1];
    recv.read_exact(&mut status).await.map_err(|e| {
        format!(
            "Hysteria2 读 TCPResponse 状态失败：{}",
            read_exact_error_text(&e)
        )
    })?;
    let message_len = read_varint_async(recv).await? as usize;
    if message_len > MAX_MESSAGE {
        return Err("Hysteria2 的 TCPResponse 消息长度非法".to_string());
    }
    let mut message = vec![0u8; message_len];
    if message_len > 0 {
        recv.read_exact(&mut message).await.map_err(|e| {
            format!(
                "Hysteria2 读 TCPResponse 消息失败：{}",
                read_exact_error_text(&e)
            )
        })?;
    }
    let padding_len = read_varint_async(recv).await? as usize;
    if padding_len > MAX_PADDING {
        return Err("Hysteria2 的 TCPResponse 填充长度非法".to_string());
    }
    if padding_len > 0 {
        let mut padding = vec![0u8; padding_len];
        recv.read_exact(&mut padding).await.map_err(|e| {
            format!(
                "Hysteria2 读 TCPResponse 填充失败：{}",
                read_exact_error_text(&e)
            )
        })?;
    }
    Ok((
        status[0] == 0,
        String::from_utf8_lossy(&message).to_string(),
    ))
}

async fn read_varint_async(recv: &mut quinn::RecvStream) -> Result<u64, String> {
    let mut first = [0u8; 1];
    recv.read_exact(&mut first)
        .await
        .map_err(|e| format!("Hysteria2 读 varint 失败：{}", read_exact_error_text(&e)))?;
    let width = 1usize << (first[0] >> 6);
    if width == 1 {
        return Ok((first[0] & 0x3f) as u64);
    }
    let mut rest = vec![0u8; width - 1];
    recv.read_exact(&mut rest)
        .await
        .map_err(|e| format!("Hysteria2 读 varint 失败：{}", read_exact_error_text(&e)))?;
    let mut value = (first[0] & 0x3f) as u64;
    for byte in rest {
        value = (value << 8) | byte as u64;
    }
    Ok(value)
}

/// 随机填充（规范要求 `Hysteria-Padding` 是一个变长随机串）。
fn random_padding(min: usize, max: usize) -> Vec<u8> {
    let span = max.saturating_sub(min).max(1);
    let mut seed = [0u8; 4];
    let _ = getrandom::getrandom(&mut seed);
    let len = min + (u32::from_le_bytes(seed) as usize % span);
    let mut out = vec![0u8; len];
    let _ = getrandom::getrandom(&mut out);
    out
}

// ── Salamander / Gecko 的 loopback 中继 ────────────────────────────────

/// 中继线程的读超时：既决定「多久检查一次 `stop`」，也是空闲时的CPU 开销来源。
const RELAY_POLL: Duration = Duration::from_millis(200);

/// 每个连接一份的 UDP 中继：`127.0.0.1:<本地>` ↔ 真实服务器，顺带做混淆。
///
/// **两个方向各一条线程**（2026-11-15 真机踩到的慢握手）：原来是单线程轮流 `recv_from`
/// 两个 socket，读超时又是 200ms —— 读 A 的时候 B 上的包最多要等 200ms 才被搬走。
/// Salamander 下勉强能用（1s 级），Gecko 每包还要拆 2~8 片、等对端重组，直接慢到 4~8s，
/// 偶尔把 QUIC 握手拖过 12s 超时。现在两个方向互不阻塞（读超时只是上限，有包立即返回），
/// 并且 `Drop` 时置 `stop` 让线程退出 —— 不再 `mem::forget` 泄漏线程。
struct ObfsRelay {
    local: SocketAddr,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for ObfsRelay {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl ObfsRelay {
    /// 起两个中继线程。`quinn_socket` 是 quinn 自己绑的那个本地地址（中继向它回包）。
    fn start(
        quinn_socket: SocketAddr,
        remote: SocketAddr,
        config: &Hysteria2Config,
    ) -> Result<Self, String> {
        let udp = std::net::UdpSocket::bind("127.0.0.1:0")
            .map_err(|e| format!("Hysteria2 混淆中继绑定失败：{e}"))?;
        let local = udp
            .local_addr()
            .map_err(|e| format!("Hysteria2 混淆中继取地址失败：{e}"))?;
        udp.set_read_timeout(Some(RELAY_POLL))
            .map_err(|e| format!("Hysteria2 混淆中继设超时失败：{e}"))?;
        let upstream = std::net::UdpSocket::bind(if remote.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        })
        .map_err(|e| format!("Hysteria2 混淆上游绑定失败：{e}"))?;
        upstream
            .connect(remote)
            .map_err(|e| format!("Hysteria2 混淆上游连接失败：{e}"))?;
        upstream
            .set_read_timeout(Some(RELAY_POLL))
            .map_err(|e| format!("Hysteria2 混淆上游设超时失败：{e}"))?;

        let psk = config.obfs_password.clone().into_bytes();
        let gecko = config.gecko();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let udp = Arc::new(udp);
        let upstream = Arc::new(upstream);

        let spawn = |name: &str, upstream_side: bool| -> Result<(), String> {
            let udp = udp.clone();
            let upstream = upstream.clone();
            let psk = psk.clone();
            let stop = stop.clone();
            std::thread::Builder::new()
                .name(name.to_string())
                .spawn(move || {
                    if upstream_side {
                        relay_to_server(udp, upstream, quinn_socket, psk, gecko, stop)
                    } else {
                        relay_to_client(udp, upstream, quinn_socket, psk, gecko, stop)
                    }
                })
                .map(|_| ())
                .map_err(|e| format!("Hysteria2 混淆中继线程启动失败：{e}"))
        };
        spawn("lunac-hysteria2-obfs-up", true)?;
        spawn("lunac-hysteria2-obfs-down", false)?;
        Ok(Self { local, stop })
    }

    fn address(&self) -> SocketAddr {
        self.local
    }
}

/// quinn → 服务器：按包混淆（Gecko 时先分片）。
fn relay_to_server(
    quinn_side: Arc<std::net::UdpSocket>,
    upstream: Arc<std::net::UdpSocket>,
    _quinn_addr: SocketAddr,
    psk: Vec<u8>,
    gecko: bool,
    stop: Arc<std::sync::atomic::AtomicBool>,
) {
    let mut buffer = vec![0u8; 65_536];
    let mut msg_id: u8 = 0;
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        let read = match quinn_side.recv_from(&mut buffer) {
            Ok((read, _from)) => read,
            // 读超时 / 暂时没包：回去看 `stop`。
            Err(_) => continue,
        };
        let packet = &buffer[..read];
        let mut frames: Vec<Vec<u8>> = Vec::new();
        if gecko && packet.first().is_some_and(|first| first & 0x80 != 0) {
            let parts = gecko_split(packet, 2 + (msg_id % 7));
            for (index, chunk) in parts.iter() {
                let padding = random_padding(0, 32);
                frames.push(gecko_frame(
                    msg_id,
                    *index,
                    parts.len() as u8,
                    chunk,
                    &padding,
                ));
            }
            msg_id = msg_id.wrapping_add(1);
        } else {
            frames.push(packet.to_vec());
        }
        for frame in frames {
            let mut salt = [0u8; 8];
            let _ = getrandom::getrandom(&mut salt);
            let sealed = salamander_seal(&psk, &frame, salt);
            let _ = upstream.send(&sealed);
        }
    }
}

/// 服务器 → quinn：解混淆（Gecko 时先重组）。
fn relay_to_client(
    quinn_side: Arc<std::net::UdpSocket>,
    upstream: Arc<std::net::UdpSocket>,
    quinn_addr: SocketAddr,
    psk: Vec<u8>,
    gecko: bool,
    stop: Arc<std::sync::atomic::AtomicBool>,
) {
    let mut buffer = vec![0u8; 65_536];
    let mut reassembly = GeckoReassembly::default();
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        let read = match upstream.recv(&mut buffer) {
            Ok(read) => read,
            Err(_) => continue,
        };
        let Ok(opened) = salamander_open(&psk, &buffer[..read]) else {
            continue;
        };
        if gecko {
            match parse_gecko_frame(&opened) {
                Ok(Some((id, index, total, chunk))) => {
                    if let Some(packet) = reassembly.push(id, index, total, chunk) {
                        let _ = quinn_side.send_to(&packet, quinn_addr);
                    }
                }
                // 短头包（非分片）直接透传。
                Ok(None) => {
                    let _ = quinn_side.send_to(&opened, quinn_addr);
                }
                Err(_) => {}
            }
        } else {
            let _ = quinn_side.send_to(&opened, quinn_addr);
        }
    }
}

/// Gecko 分片重组（按 `msgID` 收齐 `totalChunks` 片后按 `chunkIdx` 拼回）。
#[derive(Default)]
struct GeckoReassembly {
    pending: std::collections::HashMap<u8, (u8, Vec<Option<Vec<u8>>>)>,
}

impl GeckoReassembly {
    fn push(&mut self, id: u8, index: u8, total: u8, chunk: Vec<u8>) -> Option<Vec<u8>> {
        let entry = self
            .pending
            .entry(id)
            .or_insert_with(|| (total, vec![None; total as usize]));
        if entry.0 != total || index as usize >= entry.1.len() {
            self.pending.remove(&id);
            return None;
        }
        entry.1[index as usize] = Some(chunk);
        if entry.1.iter().any(|slot| slot.is_none()) {
            return None;
        }
        let (_, slots) = self.pending.remove(&id)?;
        Some(slots.into_iter().flatten().flatten().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_matches_rfc9000_examples() {
        // RFC 9000 附录 A.1 的五个例子。
        assert_eq!(varint(0), vec![0x00]);
        assert_eq!(varint(37), vec![0x25]);
        assert_eq!(varint(15293), vec![0x7b, 0xbd]);
        assert_eq!(varint(494_878_333), vec![0x9d, 0x7f, 0x3e, 0x7d]);
        assert_eq!(
            varint(151_288_809_941_952_652),
            vec![0xc2, 0x19, 0x7c, 0x5e, 0xff, 0x14, 0xe8, 0x8c]
        );
    }

    #[test]
    fn varint_round_trips() {
        for value in [0u64, 63, 64, 16_383, 16_384, 1_073_741_823, 1_073_741_824] {
            let encoded = varint(value);
            let (parsed, used) = read_varint(&encoded).expect("应能解析");
            assert_eq!(parsed, value);
            assert_eq!(used, encoded.len());
        }
        assert!(read_varint(&[0x40]).is_err());
    }

    #[test]
    fn authority_brackets_ipv6() {
        assert_eq!(authority("example.com", 443), "example.com:443");
        assert_eq!(authority("::1", 443), "[::1]:443");
        assert_eq!(authority("[2001:db8::1]", 8443), "[2001:db8::1]:8443");
    }

    #[test]
    fn tcp_request_frame_matches_spec_layout() {
        let frame = tcp_request_frame("example.com:443", b"");
        // 0x401 = 1025 ⇒ 两字节 varint：0x44 0x01（0x40 | 高位）。
        assert_eq!(&frame[..2], &[0x44, 0x01]);
        assert_eq!(frame[2], 15); // 地址长度
        assert_eq!(&frame[3..18], b"example.com:443");
        assert_eq!(frame[18], 0); // 填充长度
        assert_eq!(frame.len(), 19);
        // 解析回来要能对上（服务端用的是同一套 varint）。
        let (frame_type, used) = read_varint(&frame).expect("帧类型");
        assert_eq!((frame_type, used), (0x401, 2));
        let (length, used2) = read_varint(&frame[used..]).expect("地址长度");
        assert_eq!(length, 15);
        assert_eq!(&frame[used + used2..used + used2 + 15], b"example.com:443");
    }

    #[test]
    fn udp_message_round_trips_and_rejects_short_buffers() {
        let bytes = udp_message(7, 9, 0, 1, "1.1.1.1:53", b"dns");
        let (session, packet, frag, count, address, data) =
            parse_udp_message(&bytes).expect("应能解析");
        assert_eq!((session, packet, frag, count), (7, 9, 0, 1));
        assert_eq!(address, "1.1.1.1:53");
        assert_eq!(data, b"dns");
        assert!(parse_udp_message(&bytes[..6]).is_err());
    }

    #[test]
    fn salamander_round_trips_and_layout_is_salt_then_payload() {
        let psk = b"secret";
        let salt = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let sealed = salamander_seal(psk, b"hello world", salt);
        assert_eq!(sealed.len(), 11 + 8);
        assert_eq!(&sealed[..8], &salt);
        // 前 8 字节是 salt，payload 必然被改写过（不是明文）。
        assert_ne!(&sealed[8..], b"hello world");
        assert_eq!(
            salamander_open(psk, &sealed).expect("应能解开"),
            b"hello world"
        );
        // 换密码解不开（不是明文）。
        assert_ne!(
            salamander_open(b"other", &sealed).expect("长度合法即返回"),
            b"hello world"
        );
        // 太短的包直接拒。
        assert!(salamander_open(psk, &[0u8; 8]).is_err());
    }

    #[test]
    fn salamander_key_depends_on_salt_and_psk() {
        let a = salamander_key(b"k", &[0u8; 8]);
        let b = salamander_key(b"k", &[1u8; 8]);
        let c = salamander_key(b"kk", &[0u8; 8]);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_eq!(a, salamander_key(b"k", &[0u8; 8]));
    }

    #[test]
    fn gecko_frame_and_split_round_trip() {
        let packet: Vec<u8> = (0..100u8).collect();
        let parts = gecko_split(&packet, 4);
        assert_eq!(parts.len(), 4);
        let mut assembly = GeckoReassembly::default();
        let mut reassembled = None;
        for (index, chunk) in parts {
            let frame = gecko_frame(3, index, 4, &chunk, &[0xaa, 0xbb]);
            let (id, parsed_index, total, body) = parse_gecko_frame(&frame)
                .expect("应能解析")
                .expect("是分片帧");
            assert_eq!((id, parsed_index, total), (3, index, 4));
            assert_eq!(body, chunk);
            reassembled = assembly.push(id, parsed_index, total, body);
        }
        assert_eq!(reassembled.expect("四片齐全"), packet);
    }

    #[test]
    fn gecko_rejects_bad_frames_and_passes_short_header() {
        // 短头包（首字节高位 0）→ None。
        assert!(parse_gecko_frame(&[0x12, 0x34]).expect("合法").is_none());
        // 分片总数越界。
        assert!(parse_gecko_frame(&[0x80, 1, 0x01, 0, 0]).is_err());
        // 填充越界。
        assert!(parse_gecko_frame(&[0x80, 1, 0x24, 0xff, 0xff]).is_err());
    }

    #[test]
    fn config_falls_back_to_server_for_sni() {
        let mut config = Hysteria2Config {
            server: "node.example.com".into(),
            port: 8443,
            sni: String::new(),
            insecure: true,
            password: "pw".into(),
            obfs: "salamander".into(),
            obfs_password: "ob".into(),
        };
        assert_eq!(config.server_name(), "node.example.com");
        assert!(config.obfs_enabled());
        assert!(!config.gecko());
        config.sni = "sni.example.com".into();
        assert_eq!(config.server_name(), "sni.example.com");
        config.obfs = "gecko".into();
        assert!(config.gecko());
        config.obfs = String::new();
        assert!(!config.obfs_enabled());
    }

    #[test]
    fn split_authority_handles_domains_ipv4_and_brackets() {
        assert_eq!(
            split_authority("example.com:443"),
            Some(("example.com".to_string(), 443))
        );
        assert_eq!(
            split_authority("1.1.1.1:53"),
            Some(("1.1.1.1".to_string(), 53))
        );
        assert_eq!(
            split_authority("[2001:db8::1]:8443"),
            Some(("2001:db8::1".to_string(), 8443))
        );
        assert_eq!(split_authority("no-port"), None);
    }

    #[test]
    fn udp_fragments_send_one_message_when_it_fits() {
        let frames = udp_fragments(7, 1, "1.1.1.1:53", b"dns-query", 1200);
        assert_eq!(frames.len(), 1);
        let (session, packet, frag, count, address, data) =
            parse_udp_message(&frames[0]).expect("应能解析");
        assert_eq!((session, packet, frag, count), (7, 1, 0, 1));
        assert_eq!(address, "1.1.1.1:53");
        assert_eq!(data, b"dns-query");
    }

    #[test]
    fn udp_fragments_split_and_reassemble_in_order() {
        let payload: Vec<u8> = (0..1500u32).map(|i| (i % 251) as u8).collect();
        let frames = udp_fragments(9, 3, "example.com:443", &payload, 600);
        assert!(frames.len() >= 3, "1500 字节按 600 的预算应该切成多片");
        // 每一片都要带同一个 Session ID / Packet ID，并且 fragCount 一致。
        for frame in &frames {
            let (session, packet, _, count, ..) = parse_udp_message(frame).expect("分片可解析");
            assert_eq!((session, packet, count), (9, 3, frames.len() as u8));
        }
        // 乱序到达也要能拼回来（分片按 index 归位，不依赖顺序）。
        let mut reassembly = UdpReassembly::default();
        let mut last = None;
        for frame in frames.iter().rev() {
            last = reassembly.push(frame).or(last);
        }
        let (host, port, data) = last.expect("收齐后应交付");
        assert_eq!((host.as_str(), port), ("example.com", 443));
        assert_eq!(data, payload);
    }

    #[test]
    fn udp_reassembly_drops_incomplete_and_inconsistent_packets() {
        let payload: Vec<u8> = (0..900u32).map(|i| (i % 97) as u8).collect();
        let frames = udp_fragments(1, 2, "a.example:9", &payload, 400);
        assert!(frames.len() >= 2);
        let mut reassembly = UdpReassembly::default();
        // 只喂第一片 ⇒ 不交付。
        assert!(reassembly.push(&frames[0]).is_none());
        // 同一个 PacketID 的分片数变了 ⇒ 整包丢弃，且不会误交付。
        let mut other = frames[1].clone();
        other[7] = other[7].wrapping_add(1);
        assert!(reassembly.push(&other).is_none());
        // 脏数据（截断）直接忽略。
        assert!(reassembly.push(&frames[0][..6]).is_none());
    }
}
