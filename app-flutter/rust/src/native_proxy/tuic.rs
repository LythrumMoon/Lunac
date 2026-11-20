//! TUIC v5（QUIC 上的轻量代理协议：Authenticate / Connect / Packet / Dissociate / Heartbeat）。
//!
//! **协议依据**（2026-11-16 逐条核对，不凭记忆写）：
//!   · 官方规范 `EAimTY/tuic` 的 `PROTOCOL.md`（v5：五个命令、`Address` 编码、`Packet` 布局）；
//!   · 参考实现 `SagerNet/sing-quic` @ `main`：`tuic/protocol.go`（`Version = 5`、
//!     `AuthenticateLen = 2 + 16 + 32`）、`tuic/client.go`（`ExportKeyingMaterial(uuid, password, 32)`、
//!     认证走单向流、心跳走 datagram）、`tuic/packet.go`（`Packet` 头 10 字节 + 地址、
//!     `Dissociate` 走单向流、分片按 `(session, packet)` 重组、10 秒 TTL）、
//!     `tuic/address.go`（`0x00` 域名 / `0x01` IPv4 / `0x02` IPv6 / `0xff` None）。
//!
//! **与 Hysteria2 的差别（别照抄）**：
//!   · 认证令牌不是密码本身，而是 TLS Keying Material Exporter（RFC 5705）：
//!     `TOKEN = exporter(label = UUID 原始 16 字节, context = 密码原始字节, 32)`；
//!   · 认证走**单向流**（`[5,0][uuid][token]`），没有「认证回包」——失败由服务端
//!     直接关连接，所以错误文案只能来自 `ConnectionError` 的原因串；
//!   · TCP 转发是**双向流**：`[5,1][地址]` 之后就是裸字节；
//!   · UDP 默认走 **QUIC datagram**（`native` 模式），没有 HTTP/3 那一层；
//!   · 心跳是 datagram `[5,4]`，本实现交给 QUIC 的 `keep_alive_interval`（见下）。
//!
//! **如实登记的边界**：
//!   · **QUIC 连接复用（2026-11-18）**：同一节点的多条 TCP 连接共用一个 `quinn::Connection`，
//!     每条连接只开一条双向流（最多 `MAX_STREAMS_PER_CONN` 条），认证只在建连时做一次
//!     （与 sing-quic 同口径）；UDP（`native`）仍**每个目标一条连接** —— datagram 没有
//!     stream id，多条 `TuicUdp` 共用连接会互相偷包；
//!   · UDP 只实现 `native`（datagram）模式；节点的 `udp-relay-mode: quic` 在
//!     解析阶段保留原值，`udp.rs` 遇到时**如实报错**（不静默降级成丢包）；
//!   · 不做 0-RTT（`reduce-rtt`），不做拥塞控制选择（交给 quinn 默认）；
//!   · TUIC v4（只有 token、没有 uuid）不支持 —— 链接里没有 `uuid:password` 形态时解析期就拒。

use std::cell::Cell;
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rustls::pki_types::ServerName;
use sha2::{Digest, Sha256};

use super::quic::{connection_error_text, read_error_text, resolve, runtime};
use super::transport::{BoxStream, Stream};

/// TUIC 的协议版本字节。
pub const VERSION: u8 = 0x05;
/// `0x00` Authenticate：单向流，`[5,0][uuid 16][token 32]`。
pub const CMD_AUTHENTICATE: u8 = 0x00;
/// `0x01` Connect：双向流，`[5,1][地址]` 之后是裸字节。
pub const CMD_CONNECT: u8 = 0x01;
/// `0x02` Packet：UDP 数据报（`native` 模式）。
pub const CMD_PACKET: u8 = 0x02;
/// `0x03` Dissociate：单向流，`[5,3][assoc u16]`。
pub const CMD_DISSOCIATE: u8 = 0x03;
/// `0x04` Heartbeat：datagram，`[5,4]`。
pub const CMD_HEARTBEAT: u8 = 0x04;

/// 默认 ALPN（与服务端默认一致；Clash 的 `alpn` 列表可覆盖）。
pub const ALPN: &[u8] = b"h3";

/// QUIC 空闲 / 保活：TUIC 原实现用 10s 心跳；这里让 QUIC 层自己保活
/// （等价效果，少一条 TUIC 心跳路径）。
const MAX_IDLE: Duration = Duration::from_secs(30);
const KEEP_ALIVE: Duration = Duration::from_secs(10);
/// 握手（含 TLS 1.3）超时：与其它协议同量级。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(12);
/// 分片重组的缓存 TTL（参考实现 10 秒）。
const DEFRAG_TTL: Duration = Duration::from_secs(10);
/// 域名长度上限（`Address` 的长度字段只有一个字节）。
const MAX_DOMAIN: usize = 255;

/// 一个 TUIC 节点（`mod.rs` 的 `Node` 去掉与 QUIC 无关的字段）。
#[derive(Clone, Debug, PartialEq)]
pub struct TuicConfig {
    pub server: String,
    pub port: u16,
    /// `sni`；空 = 用 `server`（`disable-sni` 的节点由 `clash.rs` 把 sni 写成 server）。
    pub sni: String,
    /// 跳过证书校验（自签证书节点的唯一可行方式）。
    pub insecure: bool,
    /// ALPN 列表（Clash 的 `alpn`）；空 = `h3`。
    pub alpn: Vec<String>,
    pub uuid: String,
    pub password: String,
    /// `native`（datagram）/ `quic`（每条包一条单向流）；只实现了 `native`。
    pub udp_relay_mode: String,
}

impl TuicConfig {
    /// 实际用于 TLS 的 SNI（空 `sni` 回落到节点地址）。
    pub fn server_name(&self) -> &str {
        if self.sni.trim().is_empty() {
            &self.server
        } else {
            &self.sni
        }
    }

    /// ALPN 字节列表；空列表回落到 `h3`。
    pub fn alpn_bytes(&self) -> Vec<Vec<u8>> {
        if self.alpn.is_empty() {
            vec![ALPN.to_vec()]
        } else {
            self.alpn
                .iter()
                .map(|item| item.as_bytes().to_vec())
                .collect()
        }
    }

    /// UDP 是否走参考实现默认的 datagram 模式。
    pub fn native_udp(&self) -> bool {
        self.udp_relay_mode.trim().is_empty() || self.udp_relay_mode.eq_ignore_ascii_case("native")
    }

    /// 连接池的键：同一台服务器 + 同一份凭据 + 同一套 TLS 参数才可复用。
    ///
    /// 凭据只以 sha256 进键（池里不出现明文密码副本）；`udp_relay_mode` 不影响 TCP 握手，不进键。
    pub fn pool_key(&self) -> String {
        let digest = Sha256::digest(format!("{}|{}", self.uuid.trim(), self.password).as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let alpn = self
            .alpn_bytes()
            .iter()
            .map(|item| String::from_utf8_lossy(item).to_string())
            .collect::<Vec<String>>()
            .join(",");
        format!(
            "{}:{}|{}|{}|{}|{}",
            self.server,
            self.port,
            self.server_name(),
            self.insecure,
            alpn,
            digest
        )
    }
}

/// TUIC 的地址编码：`[类型][地址][端口 2 字节大端]`。
///
/// 类型：`0x00` 域名（1 字节长度 + 名字）/ `0x01` IPv4 / `0x02` IPv6；
/// 分片的后续片用 `0xff`（没有地址）。
pub fn encode_address(host: &str, port: u16) -> Result<Vec<u8>, String> {
    let bare = host.trim_matches(['[', ']']);
    let mut out = Vec::with_capacity(1 + bare.len() + 3);
    match bare.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => {
            out.push(0x01);
            out.extend_from_slice(&ip.octets());
        }
        Ok(std::net::IpAddr::V6(ip)) => {
            out.push(0x02);
            out.extend_from_slice(&ip.octets());
        }
        Err(_) => {
            let bytes = bare.as_bytes();
            if bytes.is_empty() {
                return Err("TUIC 目标地址为空".to_string());
            }
            if bytes.len() > MAX_DOMAIN {
                return Err(format!("TUIC 域名超过 {MAX_DOMAIN} 字节：{bare}"));
            }
            out.push(0x00);
            out.push(bytes.len() as u8);
            out.extend_from_slice(bytes);
        }
    }
    out.extend_from_slice(&port.to_be_bytes());
    Ok(out)
}

/// 解析地址；返回（`None` = `0xff` 无地址）与已消费字节数。
pub fn parse_address(bytes: &[u8]) -> Result<(Option<(String, u16)>, usize), String> {
    let kind = *bytes.first().ok_or("TUIC 地址截断（缺类型）")?;
    let (host, offset) = match kind {
        0x00 => {
            let len = *bytes.get(1).ok_or("TUIC 地址截断（缺长度）")? as usize;
            if len == 0 {
                return Err("TUIC 域名为空".to_string());
            }
            let end = 2 + len;
            if bytes.len() < end + 2 {
                return Err("TUIC 地址截断（域名或端口）".to_string());
            }
            let host = String::from_utf8(bytes[2..end].to_vec())
                .map_err(|_| "TUIC 域名不是合法 UTF-8".to_string())?;
            (host, end)
        }
        0x01 => {
            if bytes.len() < 1 + 4 + 2 {
                return Err("TUIC 地址截断（IPv4）".to_string());
            }
            let ip = std::net::Ipv4Addr::new(bytes[1], bytes[2], bytes[3], bytes[4]);
            (ip.to_string(), 5)
        }
        0x02 => {
            if bytes.len() < 1 + 16 + 2 {
                return Err("TUIC 地址截断（IPv6）".to_string());
            }
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&bytes[1..17]);
            (std::net::Ipv6Addr::from(octets).to_string(), 17)
        }
        0xff => return Ok((None, 1)),
        other => return Err(format!("TUIC 未知地址类型 {other:#x}")),
    };
    let port = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
    Ok((Some((host, port)), offset + 2))
}

/// `Authenticate` 帧：`[5,0][uuid 16][token 32]`（总长 50，与参考实现 `AuthenticateLen` 一致）。
pub fn authenticate_frame(uuid: &[u8; 16], token: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + 16 + 32);
    out.push(VERSION);
    out.push(CMD_AUTHENTICATE);
    out.extend_from_slice(uuid);
    out.extend_from_slice(token);
    out
}

/// `Connect` 帧：`[5,1][地址]`。
pub fn connect_frame(host: &str, port: u16) -> Result<Vec<u8>, String> {
    let mut out = vec![VERSION, CMD_CONNECT];
    out.extend_from_slice(&encode_address(host, port)?);
    Ok(out)
}

/// `Packet` 帧：`[5,2][assoc 2][packet 2][total 1][frag 1][size 2][地址][payload]`。
///
/// `address` 为 `None` ⇒ 地址写成 `0xff`（分片的后续片）。
pub fn packet_frame(
    assoc: u16,
    packet: u16,
    total: u8,
    frag: u8,
    address: Option<(&str, u16)>,
    payload: &[u8],
) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(10 + payload.len() + 32);
    out.push(VERSION);
    out.push(CMD_PACKET);
    out.extend_from_slice(&assoc.to_be_bytes());
    out.extend_from_slice(&packet.to_be_bytes());
    out.push(total);
    out.push(frag);
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    match address {
        Some((host, port)) => out.extend_from_slice(&encode_address(host, port)?),
        None => out.push(0xff),
    }
    out.extend_from_slice(payload);
    Ok(out)
}

/// 解析后的 `Packet`。
#[derive(Clone, Debug, PartialEq)]
pub struct Packet {
    pub assoc: u16,
    pub packet: u16,
    pub total: u8,
    pub frag: u8,
    /// `None` = 地址是 `0xff`。
    pub address: Option<(String, u16)>,
    pub payload: Vec<u8>,
}

/// 解析 `Packet`（datagram 模式下去掉 `[5,2]` 后也可能只解析主体）。
pub fn parse_packet(bytes: &[u8]) -> Result<Packet, String> {
    if bytes.len() < 10 {
        return Err("TUIC Packet 太短".to_string());
    }
    if bytes[0] != VERSION {
        return Err(format!("TUIC 版本不是 5（收到 {}）", bytes[0]));
    }
    if bytes[1] != CMD_PACKET {
        return Err(format!("TUIC 命令不是 Packet（收到 {}）", bytes[1]));
    }
    let assoc = u16::from_be_bytes([bytes[2], bytes[3]]);
    let packet = u16::from_be_bytes([bytes[4], bytes[5]]);
    let total = bytes[6];
    let frag = bytes[7];
    let size = u16::from_be_bytes([bytes[8], bytes[9]]) as usize;
    let (address, used) = parse_address(&bytes[10..])?;
    let start = 10 + used;
    let end = start + size;
    if end > bytes.len() {
        return Err("TUIC Packet 载荷越界".to_string());
    }
    Ok(Packet {
        assoc,
        packet,
        total,
        frag,
        address,
        payload: bytes[start..end].to_vec(),
    })
}

/// UDP 分片重组：按 `(assoc, packet)` 聚合，所有片到齐才交出去。
#[derive(Default)]
pub struct Defragger {
    pending: HashMap<(u16, u16), Fragments>,
}

struct Fragments {
    total: u8,
    parts: Vec<Option<Vec<u8>>>,
    address: Option<(String, u16)>,
    at: Instant,
}

impl Defragger {
    /// 喂一片；凑齐时返回（来源地址，完整载荷）。
    pub fn feed(&mut self, packet: Packet) -> Option<(String, u16, Vec<u8>)> {
        if packet.total <= 1 {
            packet
                .address
                .map(|(host, port)| (host, port, packet.payload))
        } else {
            if packet.frag >= packet.total {
                return None;
            }
            let key = (packet.assoc, packet.packet);
            let entry = self.pending.entry(key).or_insert_with(|| Fragments {
                total: packet.total,
                parts: vec![None; packet.total as usize],
                address: None,
                at: Instant::now(),
            });
            // 同一个 packet id 换了分片总数 ⇒ 旧缓存作废（参考实现同口径）。
            if entry.total != packet.total {
                entry.total = packet.total;
                entry.parts = vec![None; packet.total as usize];
                entry.address = None;
            }
            if entry.address.is_none() {
                entry.address = packet.address;
            }
            entry.at = Instant::now();
            let index = packet.frag as usize;
            if entry.parts[index].is_none() {
                entry.parts[index] = Some(packet.payload);
            }
            if entry.parts.iter().any(Option::is_none) {
                None
            } else {
                let (_, entry) = self.pending.remove_entry(&key)?;
                let address = entry.address?;
                let mut joined = Vec::new();
                for part in entry.parts {
                    joined.extend_from_slice(part.as_deref().unwrap_or_default());
                }
                Some((address.0, address.1, joined))
            }
        }
    }

    /// 丢掉超时的半包（只在下一次 `feed` 时按需调用，不另起线程）。
    pub fn purge(&mut self) {
        self.pending
            .retain(|_, entry| entry.at.elapsed() < DEFRAG_TTL);
    }
}

/// 同一个 QUIC 连接最多承载的并发双向流（TUIC 的每条被代理连接 = 一条流）。
pub const MAX_STREAMS_PER_CONN: usize = 32;

/// 池里没有任何 stream 的连接空闲这么久就关掉（QUIC 的 keep-alive 会让空闲超时永不触发，
/// 必须自己收，否则切节点 / 换订阅后旧连接会一直挂着）。
const POOL_IDLE_SECS: u64 = 60;

/// 池里的一条 QUIC 连接（`Endpoint` 保活，`streams` 记当前占用的双向流数）。
struct PoolEntry {
    /// 只保活：`Endpoint` 掉了连接就没了，没有任何读路径。
    _endpoint: quinn::Endpoint,
    conn: quinn::Connection,
    streams: AtomicUsize,
    /// 最近一次被取用 / 归还的 Unix 秒（空闲回收用）。
    last_used: AtomicU64,
}

impl PoolEntry {
    fn alive(&self) -> bool {
        self.conn.close_reason().is_none()
    }
}

fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|age| age.as_secs())
        .unwrap_or(0)
}

fn pool() -> &'static Mutex<HashMap<String, Vec<Arc<PoolEntry>>>> {
    static POOL: OnceLock<Mutex<HashMap<String, Vec<Arc<PoolEntry>>>>> = OnceLock::new();
    POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 回收死连接与长时间空闲的连接（两种都会真正 `close`，不是只从表里删掉）。
fn sweep(pool: &mut HashMap<String, Vec<Arc<PoolEntry>>>, now: u64) {
    for list in pool.values_mut() {
        list.retain(|entry| {
            let idle = now.saturating_sub(entry.last_used.load(Ordering::Relaxed));
            let keep = entry.alive()
                && (entry.streams.load(Ordering::Relaxed) > 0 || idle < POOL_IDLE_SECS);
            if !keep {
                entry.conn.close(0u32.into(), b"lunac: idle");
            }
            keep
        });
    }
    pool.retain(|_, list| !list.is_empty());
}

fn checkout(key: &str) -> Option<Arc<PoolEntry>> {
    let mut guard = pool().lock().ok()?;
    sweep(&mut guard, epoch_secs());
    let picked = guard.get(key).and_then(|list| {
        list.iter()
            .find(|entry| entry.streams.load(Ordering::Relaxed) < MAX_STREAMS_PER_CONN)
            .cloned()
    });
    if let Some(entry) = &picked {
        entry.last_used.store(epoch_secs(), Ordering::Relaxed);
    }
    picked
}

fn register(key: &str, entry: Arc<PoolEntry>) {
    if let Ok(mut guard) = pool().lock() {
        guard.entry(key.to_string()).or_default().push(entry);
    }
}

fn forget(key: &str, entry: &Arc<PoolEntry>) {
    entry.conn.close(0u32.into(), b"lunac: session retired");
    if let Ok(mut guard) = pool().lock() {
        if let Some(list) = guard.get_mut(key) {
            list.retain(|item| !Arc::ptr_eq(item, entry));
        }
        if guard.get(key).map(|list| list.is_empty()).unwrap_or(false) {
            guard.remove(key);
        }
    }
}

/// 关掉池里的全部 QUIC 连接（引擎停止时调用，不靠 30s 空闲超时慢慢释放）。
pub fn close_all_sessions() {
    let entries: Vec<Arc<PoolEntry>> = match pool().lock() {
        Ok(mut guard) => guard.drain().flat_map(|(_, list)| list).collect(),
        Err(_) => return,
    };
    for entry in entries {
        entry.conn.close(0u32.into(), b"lunac: engine stopped");
    }
}

/// 一条 TUIC 的 TCP 隧道（QUIC 双向流的阻塞外壳）。
pub struct TuicStream {
    /// 保活与复用：`Endpoint` / `Connection` 掉了，stream 立刻作废；`Drop` 时把流计数还回去。
    entry: Arc<PoolEntry>,
    send: quinn::SendStream,
    recv: quinn::RecvStream,
    read_timeout: Cell<Option<Duration>>,
}

impl Read for TuicStream {
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

impl Write for TuicStream {
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

impl Stream for TuicStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.read_timeout.set(timeout);
        Ok(())
    }
}

impl Drop for TuicStream {
    fn drop(&mut self) {
        self.entry.streams.fetch_sub(1, Ordering::Relaxed);
        self.entry.last_used.store(epoch_secs(), Ordering::Relaxed);
    }
}

/// 拨号：优先复用池里的 QUIC 连接；没有才握手（握手内含 TLS exporter 认证）。
pub fn connect(
    config: &TuicConfig,
    target_host: &str,
    target_port: u16,
) -> Result<BoxStream, String> {
    let stream = runtime().block_on(connect_async(config, target_host, target_port))?;
    Ok(Box::new(stream))
}

async fn connect_async(
    config: &TuicConfig,
    target_host: &str,
    target_port: u16,
) -> Result<TuicStream, String> {
    let key = config.pool_key();
    let entry = match checkout(&key) {
        Some(entry) => entry,
        None => {
            let handshake = handshake_async(config).await?;
            let entry = Arc::new(PoolEntry {
                _endpoint: handshake.endpoint,
                conn: handshake.conn,
                streams: AtomicUsize::new(0),
                last_used: AtomicU64::new(epoch_secs()),
            });
            register(&key, Arc::clone(&entry));
            entry
        }
    };
    let (mut send, recv) = match entry.conn.open_bi().await {
        Ok(pair) => pair,
        Err(_error) => {
            // 连接可能刚好被服务端收掉：丢掉它，换一条新连接重试一次。
            forget(&key, &entry);
            let handshake = handshake_async(config).await?;
            let fresh = Arc::new(PoolEntry {
                _endpoint: handshake.endpoint,
                conn: handshake.conn,
                streams: AtomicUsize::new(0),
                last_used: AtomicU64::new(epoch_secs()),
            });
            register(&key, Arc::clone(&fresh));
            let pair = fresh
                .conn
                .open_bi()
                .await
                .map_err(|e| format!("TUIC 开流失败：{}", connection_error_text(&e)))?;
            fresh.streams.fetch_add(1, Ordering::Relaxed);
            let header = connect_frame(target_host, target_port)?;
            let (mut send, recv) = pair;
            send.write_all(&header)
                .await
                .map_err(|e| format!("TUIC 发 Connect 失败：{e}"))?;
            return Ok(TuicStream {
                entry: fresh,
                send,
                recv,
                read_timeout: Cell::new(None),
            });
        }
    };
    let header = connect_frame(target_host, target_port)?;
    send.write_all(&header)
        .await
        .map_err(|e| format!("TUIC 发 Connect 失败：{e}"))?;
    entry.streams.fetch_add(1, Ordering::Relaxed);
    Ok(TuicStream {
        entry,
        send,
        recv,
        read_timeout: Cell::new(None),
    })
}

// ── UDP（native / datagram 模式） ───────────────────────────────────────

/// 一条 TUIC 的 UDP 转发会话（一个 assoc id 一条 QUIC 连接，与 `udp.rs` 的会话粒度一致）。
pub struct TuicUdp {
    conn: quinn::Connection,
    _endpoint: quinn::Endpoint,
    assoc: u16,
    packets: u16,
    /// 一条 datagram 能装多少字节（quinn 已按对端通告值算好）。
    max_datagram: usize,
    incoming: Receiver<(String, u16, Vec<u8>)>,
}

impl TuicUdp {
    pub fn send(&mut self, host: &str, port: u16, payload: &[u8]) -> io::Result<()> {
        let frames = self
            .frames(host, port, payload)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        for frame in frames {
            self.conn
                .send_datagram(bytes::Bytes::from(frame))
                .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))?;
        }
        Ok(())
    }

    /// 上游回来的数据报（`(host, port, payload)`）；`Ok(None)` = 暂时没有。
    pub fn recv(&mut self) -> io::Result<Option<(String, u16, Vec<u8>)>> {
        match self.incoming.try_recv() {
            Ok(value) => Ok(Some(value)),
            Err(std::sync::mpsc::TryRecvError::Empty) => Ok(None),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Ok(None),
        }
    }

    /// 按 datagram 预算切帧；放得下就是一片，放不下按 `total/frag` 分片。
    fn frames(&mut self, host: &str, port: u16, payload: &[u8]) -> Result<Vec<Vec<u8>>, String> {
        let packet = self.packets;
        self.packets = self.packets.wrapping_add(1);
        let address = encode_address(host, port)?;
        let first_header = 10 + address.len();
        if first_header + payload.len() <= self.max_datagram {
            return Ok(vec![packet_frame(
                self.assoc,
                packet,
                1,
                0,
                Some((host, port)),
                payload,
            )?]);
        }
        // 后续片没有地址（`0xff`，1 字节），所以统一按「最胖的头」切，保证每片都放得下。
        let chunk = self.max_datagram.saturating_sub(10 + 1);
        if chunk == 0 {
            return Err("TUIC 的 datagram 预算太小（连 Packet 头都放不下）".to_string());
        }
        let total = payload.len().div_ceil(chunk);
        if total > u8::MAX as usize {
            return Err("UDP 包太大：TUIC 分片数超过 255".to_string());
        }
        let mut frames = Vec::with_capacity(total);
        for (index, part) in payload.chunks(chunk).enumerate() {
            let target = if index == 0 { Some((host, port)) } else { None };
            frames.push(packet_frame(
                self.assoc,
                packet,
                total as u8,
                index as u8,
                target,
                part,
            )?);
        }
        Ok(frames)
    }
}

impl Drop for TuicUdp {
    fn drop(&mut self) {
        let conn = self.conn.clone();
        let assoc = self.assoc;
        // 礼貌地 Dissociate（服务端据此立即回收 UDP socket）；随后关闭整条连接。
        // 已经在 tokio runtime 里（例如未来被异步任务持有）时不再 `block_on` ——
        // 那会直接 panic；直接关连接，服务端同样会回收。
        if tokio::runtime::Handle::try_current().is_err() {
            let _ = runtime().block_on(async move {
                if let Ok(mut stream) = conn.open_uni().await {
                    let frame = [
                        VERSION,
                        CMD_DISSOCIATE,
                        (assoc >> 8) as u8,
                        (assoc & 0xFF) as u8,
                    ];
                    let _ = stream.write_all(&frame).await;
                    let _ = stream.finish();
                }
            });
        }
        self.conn.close(0u32.into(), b"");
    }
}

/// 拨一条 UDP 会话：握手 + 认证 + 起一个收包任务。
pub fn connect_udp(config: &TuicConfig) -> Result<TuicUdp, String> {
    runtime().block_on(connect_udp_async(config))
}

async fn connect_udp_async(config: &TuicConfig) -> Result<TuicUdp, String> {
    if !config.native_udp() {
        return Err(format!(
            "TUIC 的 udp-relay-mode={} 本版未实现（只支持 native / datagram）",
            config.udp_relay_mode
        ));
    }
    let handshake = handshake_async(config).await?;
    let max_datagram = handshake.conn.max_datagram_size().ok_or_else(|| {
        "TUIC 对端没有通告 max_datagram_frame_size（不能走 native UDP）".to_string()
    })?;
    let assoc = random_u16();
    let (sender, incoming) = std::sync::mpsc::channel();
    spawn_reader(handshake.conn.clone(), sender);
    Ok(TuicUdp {
        conn: handshake.conn,
        _endpoint: handshake.endpoint,
        assoc,
        packets: 0,
        max_datagram,
        incoming,
    })
}

/// 收包任务：datagram → `Packet` → 分片重组 → 塞进通道。
///
/// 用 std 线程 + `block_on`（与 Hysteria2 的 UDP 读线程同形）：连接关闭时
/// `receive_datagram` 返回错误，线程自然退出；不占 runtime 的 worker 名额。
fn spawn_reader(conn: quinn::Connection, sender: Sender<(String, u16, Vec<u8>)>) {
    std::thread::spawn(move || {
        runtime().block_on(async move {
            let mut defragger = Defragger::default();
            loop {
                let data = match conn.read_datagram().await {
                    Ok(data) => data,
                    Err(_) => break,
                };
                if data.len() >= 2 && data[0] == VERSION && data[1] == CMD_HEARTBEAT {
                    continue;
                }
                let packet = match parse_packet(&data) {
                    Ok(packet) => packet,
                    // 认不出的包直接丢（UDP 没有错误信道）；半包按 TTL 回收。
                    Err(_) => continue,
                };
                defragger.purge();
                if let Some(value) = defragger.feed(packet) {
                    if sender.send(value).is_err() {
                        break;
                    }
                }
            }
        });
    });
}

// ── 握手（TCP / UDP 共用） ──────────────────────────────────────────────

struct Handshake {
    endpoint: quinn::Endpoint,
    conn: quinn::Connection,
}

async fn handshake_async(config: &TuicConfig) -> Result<Handshake, String> {
    let remote = resolve(&config.server, config.port)?;
    // sni 必须能被 rustls 接受（IP 也合法）；非法要在拨号前就报出来。
    ServerName::try_from(config.server_name().to_string())
        .map_err(|e| format!("TUIC 的 sni 非法：{e}"))?;

    let tls = std::sync::Arc::new(super::quic::client_tls(
        config.insecure,
        &config.alpn_bytes(),
    )?);
    let quic_crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls)
        .map_err(|e| format!("TUIC 的 TLS 配置无法用于 QUIC：{e}"))?;
    let mut client_config = quinn::ClientConfig::new(std::sync::Arc::new(quic_crypto));
    let mut transport = quinn::TransportConfig::default();
    transport
        .max_idle_timeout(Some(
            MAX_IDLE
                .try_into()
                .map_err(|_| "空闲超时非法".to_string())?,
        ))
        .keep_alive_interval(Some(KEEP_ALIVE))
        // RFC 9221：UDP native 模式要用；TCP 节点上也只多一点预算。
        .datagram_receive_buffer_size(Some(1 << 20))
        .datagram_send_buffer_size(1 << 20)
        .initial_mtu(1200)
        .mtu_discovery_config(None);
    client_config.transport_config(std::sync::Arc::new(transport));

    let bind: std::net::SocketAddr = match remote {
        std::net::SocketAddr::V4(_) => "0.0.0.0:0".parse().expect("IPv4 通配地址恒可解析"),
        std::net::SocketAddr::V6(_) => "[::]:0".parse().expect("IPv6 通配地址恒可解析"),
    };
    let mut endpoint =
        quinn::Endpoint::client(bind).map_err(|e| format!("TUIC 绑定本地端口失败：{e}"))?;
    endpoint.set_default_client_config(client_config);

    let connecting = endpoint
        .connect(remote, config.server_name())
        .map_err(|e| format!("TUIC 发起 QUIC 连接失败：{e}"))?;
    let conn = tokio::time::timeout(HANDSHAKE_TIMEOUT, connecting)
        .await
        .map_err(|_| format!("TUIC 的 QUIC 握手超时（{HANDSHAKE_TIMEOUT:?}）"))?
        .map_err(|e| format!("TUIC 的 QUIC 握手失败：{e}"))?;

    authenticate(&conn, config).await?;
    Ok(Handshake { endpoint, conn })
}

/// 认证：单向流 `[5,0][uuid][token]`，token = TLS exporter。
async fn authenticate(conn: &quinn::Connection, config: &TuicConfig) -> Result<(), String> {
    let uuid = super::vless::parse_uuid(&config.uuid)?;
    let mut token = [0u8; 32];
    conn.export_keying_material(&mut token, &uuid, config.password.as_bytes())
        .map_err(|e| format!("TUIC 导出认证令牌失败：{e:?}"))?;
    let mut send = conn
        .open_uni()
        .await
        .map_err(|e| format!("TUIC 打开认证流失败：{}", connection_error_text(&e)))?;
    send.write_all(&authenticate_frame(&uuid, &token))
        .await
        .map_err(|e| format!("TUIC 发认证帧失败：{e}"))?;
    send.finish()
        .map_err(|e| format!("TUIC 认证流收尾失败：{e}"))?;
    Ok(())
}

fn random_u16() -> u16 {
    let mut seed = [0u8; 2];
    let _ = getrandom::getrandom(&mut seed);
    u16::from_le_bytes(seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用自签证书（DER，CN=localhost，2026-11-15 生成，有效期一年）。
    /// 只用于本文件的假服务端 —— 客户端走 `insecure`，不校验证书链。
    const TEST_CERT_B64: &str = "MIICqTCCAZGgAwIBAgIJAMBXQDVnBvIMMA0GCSqGSIb3DQEBCwUAMBQxEjAQBgNVBAMTCWxvY2FsaG9zdDAeFw0yNjExMTUwNjEyMDBaFw0yNzExMTYwNjEyMDBaMBQxEjAQBgNVBAMTCWxvY2FsaG9zdDCCASIwDQYJKoZIhvcNAQEBBQADggEPADCCAQoCggEBAMTIMxna0wNon9/bnvV9WkZ8+INbaYhoiBfQHWWSLIDOTW8BOqOEhE6zY8/iHn/kc6JgLjRpkAZHgzSBZ3PMYPwxH+WB3kUlCjipxqLqXXHw6sGBNV8X/tIFpIfVy8DL4YJ2wjBAr9UJGK67DewPTMYIY5CiZmBKkmfRIB1aVba3xB4SJYwE8shsC5IV6AArg1007KnEMc+cZS15Y/ikm4Sv+xWQI0rv3Sw/w5pt5u1t+1+ZkZRusi6UCfoJeMqTmvNlmeBCR7lFqPCYNGLCxUAnKQf8S50jT8WSSFFcUzmjPsf6JjG9Ms0lh0w0aYX+AvM6cPbG+p4FKcR3x6520dkCAwEAATANBgkqhkiG9w0BAQsFAAOCAQEAp9QW2SD26rQK1Cx7hst7kmr3WDFg23j5w6pGCufvd0zaKVNGS0ysdYaDzrINm+0kCX0Ni1EEY8hTZxH0bmTEtPdAhxODTgHkruiEwTMqHanwoxK9N4hziLR4zd9ATh5i/9rEb5UnaWV8benhsMSmzJnQE+xyVEGiNZTMYY+7NTgCAeQsiqO+W2+MBdmBZbMf50dD5bZODzk9gFihdHo5PUUFQlE6NmAUVg4MWm1fHUWYGiBhFnGCm5h8pmWeJs2nx0BXtSTwHG3ndfWAbnvePBQEXdmWrG09A3qPWk3SzLcSfq7A5AqwAI4egM5nDl2RwU6IPmg/OTvpYQD1j2K2zw==";
    /// 与上面证书配对的 PKCS#1 RSA 私钥（DER）。
    const TEST_KEY_B64: &str = "MIIEowIBAAKCAQEAxMgzGdrTA2if39ue9X1aRnz4g1tpiGiIF9AdZZIsgM5NbwE6o4SETrNjz+Ief+RzomAuNGmQBkeDNIFnc8xg/DEf5YHeRSUKOKnGoupdcfDqwYE1Xxf+0gWkh9XLwMvhgnbCMECv1QkYrrsN7A9MxghjkKJmYEqSZ9EgHVpVtrfEHhIljATyyGwLkhXoACuDXTTsqcQxz5xlLXlj+KSbhK/7FZAjSu/dLD/Dmm3m7W37X5mRlG6yLpQJ+gl4ypOa82WZ4EJHuUWo8Jg0YsLFQCcpB/xLnSNPxZJIUVxTOaM+x/omMb0yzSWHTDRphf4C8zpw9sb6ngUpxHfHrnbR2QIDAQABAoIBAGeYlf54izpyLCin8GmBErOlOxe8WjSsA2m18KP2bmNpOK7F0N8g9p6QJm3ctv3RgWbsh6akJGSJ5XMQ5Su7XxulqQskcaWpOFZYWvCNvaAsmwpAQZ7MfSSIP/QbA+d97jzG1LiedXMMFOh4JX4vF9XxuKj+NjNfTvBE0klhs1o7PDqf7qWI0P/cbiDO63LBpqyqH/FF4U7zSgHZeUXLdne5iTU4xnpwneFa+k5GYRcAqVdXPpPSciJQ51ukWxiXlZa9Jf31XqxBmMi/vzdSjKM7sieE/u4Vn55wnkAyFHxZvJgA3uEV/pLxSCr6NAPXRswro4sSgVAFf0+plHNWDNkCgYEA31h7ReQjF4dO7yoroF4Kaf49bcs9N2qJHcZkT8Y5gz/IaAnWxfKhnLftxmLj6DZeh/hWp9H3QSitN4o/+u6ZEGKQQaJzgPBSdF8T2xwfb8AHWCW8rKKQIGaQIhaKITkHzabBE7r+69XChhr8ud4s3GOFsvdScIVmeJm1S/GgZUsCgYEA4Y16qzOK90gT6RDuiXMrE1vI/4yGN4sLtBwpExMWE3thlGP9qMQnEpH2spu/IDpN/2a5CbOeh5avYhxcPHQgEdAD82mD8zAsUNIYcwtqGPS/RfXMee5MYw6xrumSN1s3hOIkrQFHyNn5uTPuMUe4YN7nAaWZsfs+/uONV2sFwusCgYEA08B5B7eXibwwm1I5EI94giuYpBr4DC1GmOM6weOnLYfwgqspZH14ujQsDFY/zC6YC4Po7K2GJdHiAHFj3Y4Pstgv+vqBVfMDye9GzQtpxrKig/ErY+WwjfePmbIS/hKrElCSaBl3jbMZ/bliePprP/xfi2EvWR0xHRBmMf2BW+cCgYAdms5RDlDdKC7E1p4YmG88+T0mnbE4NR3yJAgYItcw827ZZU50Clr+iv4Qb3TkrAJ01WzQIy1ZqqOWgxZwMOXMsLr0BhN1pgsLCKiA4Syp3eYFLsAJ+Qc5sUINLUct0OE2peAefR6I9GNctb475agDvv4fLzc/LzUTi5aPtNJqxQKBgFKZyh/ZmMmHCqz7M17eZo+zumXbOBDmhGs4iMn7ZA1Q887yFZtngSM57n0uqO7elWU/2HIEE7nT30/KyIOhQoFiuzlHhN/7G50QnGXbU2HLI++bKf1Fw1V4UwHlRrhq67OUrwemWXlmY82d/QM914IVtiq/yunNQbsKAddld+4g";

    const TEST_UUID: &str = "11111111-2222-3333-4444-555555555555";
    const TEST_PASSWORD: &str = "pw";

    fn decode_b64(text: &str) -> Vec<u8> {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(text)
            .expect("测试夹具是合法 base64")
    }

    fn server_endpoint() -> quinn::Endpoint {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs1KeyDer};
        let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
        let mut tls = rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .expect("TLS 1.3")
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(decode_b64(TEST_CERT_B64))],
                PrivateKeyDer::Pkcs1(PrivatePkcs1KeyDer::from(decode_b64(TEST_KEY_B64))),
            )
            .expect("测试证书与私钥必须配对");
        tls.alpn_protocols = vec![ALPN.to_vec()];
        let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls)
            .expect("服务端 TLS 配置可用于 QUIC");
        let mut config = quinn::ServerConfig::with_crypto(std::sync::Arc::new(crypto));
        let mut transport = quinn::TransportConfig::default();
        transport
            .datagram_receive_buffer_size(Some(1 << 20))
            .datagram_send_buffer_size(1 << 20);
        config.transport_config(std::sync::Arc::new(transport));
        // `Endpoint::server` 需要「当前有 tokio runtime」；测试线程本身没有，
        // 借共享 runtime 的 enter guard 建端点（真正的驱动在服务端线程的 block_on 里）。
        let _guard = runtime().enter();
        quinn::Endpoint::server(config, "127.0.0.1:0".parse().expect("回环地址"))
            .expect("服务端端点")
    }

    fn test_config(port: u16) -> TuicConfig {
        TuicConfig {
            server: "127.0.0.1".to_string(),
            port,
            sni: "localhost".to_string(),
            insecure: true,
            alpn: Vec::new(),
            uuid: TEST_UUID.to_string(),
            password: TEST_PASSWORD.to_string(),
            udp_relay_mode: "native".to_string(),
        }
    }

    /// 假服务端：读认证帧并用**自己这一侧**的 exporter 复算令牌，再按命令分流。
    ///
    /// 这条断言（`token` 一致）是整个 TUIC 认证正确性的关键证据：标签 / 上下文 / 长度
    /// 只要有一处与参考实现不同，两边导出的字节就不会相等。
    fn spawn_server(endpoint: quinn::Endpoint, expect_udp: bool) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            runtime().block_on(async move {
                let incoming = endpoint.accept().await.expect("客户端必须来连");
                let conn = incoming.await.expect("QUIC 握手必须成功");

                // 认证：`[5,0][uuid][token]`。
                let mut auth = conn.accept_uni().await.expect("认证流");
                let mut frame = [0u8; 50];
                auth.read_exact(&mut frame)
                    .await
                    .expect("认证帧必须完整（2 + 16 + 32）");
                assert_eq!(&frame[..2], &[VERSION, CMD_AUTHENTICATE]);
                let mut uuid = [0u8; 16];
                uuid.copy_from_slice(&frame[2..18]);
                assert_eq!(
                    uuid,
                    super::super::vless::parse_uuid(TEST_UUID).unwrap(),
                    "uuid 必须按原始 16 字节发送"
                );
                let mut token = [0u8; 32];
                conn.export_keying_material(&mut token, &uuid, TEST_PASSWORD.as_bytes())
                    .expect("服务端也能导出");
                assert_eq!(&frame[18..], &token, "exporter 的 label / context 必须一致");

                if expect_udp {
                    // UDP：收一个 Packet，回一个同 assoc / 同 packet id 的 Packet。
                    let datagram = conn.read_datagram().await.expect("客户端必须发来 UDP 包");
                    let packet = parse_packet(&datagram).expect("Packet 必须可解析");
                    assert_eq!(packet.payload, b"query");
                    let reply = packet_frame(
                        packet.assoc,
                        packet.packet,
                        1,
                        0,
                        Some(("8.8.8.8", 53)),
                        b"answer",
                    )
                    .expect("回包");
                    conn.send_datagram(bytes::Bytes::from(reply))
                        .expect("发送回包");
                } else {
                    // TCP：`[5,1][地址]` 之后是裸字节；回 `pong` 再关写侧。
                    let (mut send, mut recv) = conn.accept_bi().await.expect("Connect 流");
                    let mut head = [0u8; 2];
                    recv.read_exact(&mut head).await.expect("Connect 头");
                    assert_eq!(&head, &[VERSION, CMD_CONNECT]);
                    // 读地址：域名要按长度字段消耗，IPv4/IPv6 定长。
                    let mut kind = [0u8; 1];
                    recv.read_exact(&mut kind).await.expect("地址类型");
                    match kind[0] {
                        0x00 => {
                            let mut len = [0u8; 1];
                            recv.read_exact(&mut len).await.expect("域名长度");
                            let mut rest = vec![0u8; len[0] as usize + 2];
                            recv.read_exact(&mut rest).await.expect("域名与端口");
                        }
                        0x01 => {
                            let mut rest = [0u8; 6];
                            recv.read_exact(&mut rest).await.expect("IPv4 与端口");
                        }
                        0x02 => {
                            let mut rest = [0u8; 18];
                            recv.read_exact(&mut rest).await.expect("IPv6 与端口");
                        }
                        other => panic!("unexpected address type {other:#x}"),
                    }
                    let mut payload = [0u8; 4];
                    recv.read_exact(&mut payload).await.expect("客户端正文");
                    assert_eq!(&payload, b"ping");
                    send.write_all(b"pong").await.expect("回写");
                    send.finish().expect("关写侧");
                    // 等对端读走再结束（避免连接被过早关闭）。
                    let _ =
                        tokio::time::timeout(Duration::from_secs(2), recv.read_to_end(64)).await;
                }
                // 连接进了客户端池，`wait_idle` 会一直等到 30s 空闲超时；留一小段时间让最后
                // 一个回包送达，再主动关掉。
                tokio::time::sleep(Duration::from_millis(200)).await;
                conn.close(0u32.into(), b"test done");
            })
        })
    }

    #[test]
    fn a_mock_server_accepts_the_authenticated_tcp_stream() {
        let endpoint = server_endpoint();
        let port = endpoint.local_addr().unwrap().port();
        let server = spawn_server(endpoint, false);

        let mut stream = connect(&test_config(port), "example.com", 443)
            .expect("TUIC 拨号必须成功（假服务端会校验认证令牌）");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream.write_all(b"ping").expect("写正文");
        let mut reply = [0u8; 4];
        stream.read_exact(&mut reply).expect("读回包");
        assert_eq!(&reply, b"pong");
        drop(stream);
        server.join().expect("服务端线程不能 panic");
    }

    #[test]
    fn a_second_tcp_stream_reuses_the_same_quic_connection() {
        let endpoint = server_endpoint();
        let port = endpoint.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            runtime().block_on(async move {
                let incoming = endpoint.accept().await.expect("客户端必须来连");
                let conn = incoming.await.expect("QUIC 握手必须成功");
                // 认证只在建连时发生一次。
                let mut auth = conn.accept_uni().await.expect("认证流");
                let mut frame = [0u8; 50];
                auth.read_exact(&mut frame).await.expect("认证帧");
                assert_eq!(&frame[..2], &[VERSION, CMD_AUTHENTICATE]);
                // 两条双向流各自 `[5,1][地址]` + 正文，各回一个 pong。
                for _ in 0..2 {
                    let (mut send, mut recv) = conn.accept_bi().await.expect("Connect 流");
                    let mut head = [0u8; 2];
                    recv.read_exact(&mut head).await.expect("Connect 头");
                    assert_eq!(&head, &[VERSION, CMD_CONNECT]);
                    let mut kind = [0u8; 1];
                    recv.read_exact(&mut kind).await.expect("地址类型");
                    match kind[0] {
                        0x00 => {
                            let mut len = [0u8; 1];
                            recv.read_exact(&mut len).await.expect("域名长度");
                            let mut rest = vec![0u8; len[0] as usize + 2];
                            recv.read_exact(&mut rest).await.expect("域名与端口");
                        }
                        0x01 => {
                            let mut rest = [0u8; 6];
                            recv.read_exact(&mut rest).await.expect("IPv4 与端口");
                        }
                        0x02 => {
                            let mut rest = [0u8; 18];
                            recv.read_exact(&mut rest).await.expect("IPv6 与端口");
                        }
                        other => panic!("unexpected address type {other:#x}"),
                    }
                    let mut payload = [0u8; 4];
                    recv.read_exact(&mut payload).await.expect("客户端正文");
                    assert_eq!(&payload, b"ping");
                    send.write_all(b"pong").await.expect("回写");
                    send.finish().expect("关写侧");
                }
                // 不该再出现第二条 QUIC 连接（复用生效的硬证据）。
                let second =
                    tokio::time::timeout(Duration::from_millis(300), endpoint.accept()).await;
                assert!(second.is_err(), "第二条 TCP 流必须复用同一条 QUIC 连接");
            })
        });

        let config = test_config(port);
        let mut first = connect(&config, "a.example", 80).expect("第一条流");
        first
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        first.write_all(b"ping").expect("写正文");
        let mut reply = [0u8; 4];
        first.read_exact(&mut reply).expect("读回包");
        assert_eq!(&reply, b"pong");
        drop(first);

        let mut second = connect(&config, "b.example", 443).expect("第二条流（复用）");
        second
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        second.write_all(b"ping").expect("写正文");
        second.read_exact(&mut reply).expect("读回包");
        assert_eq!(&reply, b"pong");
        drop(second);
        server.join().expect("服务端线程不能 panic");
    }

    #[test]
    fn a_mock_server_relays_udp_packets_with_the_original_assoc_id() {
        let endpoint = server_endpoint();
        let port = endpoint.local_addr().unwrap().port();
        let server = spawn_server(endpoint, true);

        let mut session = connect_udp(&test_config(port)).expect("TUIC UDP 会话");
        session.send("1.2.3.4", 53, b"query").expect("发 UDP 包");
        let mut reply = None;
        for _ in 0..300 {
            if let Some(value) = session.recv().expect("读回包") {
                reply = Some(value);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let (host, port, payload) = reply.expect("假服务端必须回一个 Packet");
        assert_eq!((host.as_str(), port), ("8.8.8.8", 53));
        assert_eq!(payload, b"answer");
        drop(session);
        server.join().expect("服务端线程不能 panic");
    }

    #[test]
    fn authenticate_frame_is_version_command_uuid_token() {
        let uuid = super::super::vless::parse_uuid("11111111-2222-3333-4444-555555555555").unwrap();
        let token = [7u8; 32];
        let frame = authenticate_frame(&uuid, &token);
        assert_eq!(frame.len(), 50, "参考实现 AuthenticateLen = 2 + 16 + 32");
        assert_eq!(&frame[..2], &[VERSION, CMD_AUTHENTICATE]);
        assert_eq!(&frame[2..18], &uuid);
        assert_eq!(&frame[18..], &token);
    }

    #[test]
    fn address_encoding_matches_the_tuic_families() {
        // 域名：0x00 + 长度 + 名字 + 端口。
        let domain = encode_address("example.com", 443).unwrap();
        assert_eq!(domain[0], 0x00);
        assert_eq!(domain[1], 11);
        assert_eq!(&domain[2..13], b"example.com");
        assert_eq!(&domain[13..], &443u16.to_be_bytes());

        // IPv4：0x01 + 4 字节。
        let v4 = encode_address("1.2.3.4", 80).unwrap();
        assert_eq!(v4[0], 0x01);
        assert_eq!(&v4[1..5], &[1, 2, 3, 4]);
        assert_eq!(&v4[5..], &80u16.to_be_bytes());

        // IPv6：0x02 + 16 字节（方括号形态也要认）。
        let v6 = encode_address("[::1]", 8080).unwrap();
        assert_eq!(v6[0], 0x02);
        assert_eq!(v6.len(), 1 + 16 + 2);

        assert!(encode_address("", 1).is_err());
        assert!(encode_address(&"a".repeat(256), 1).is_err());
    }

    #[test]
    fn address_round_trips_and_none_is_a_single_byte() {
        for (host, port) in [("example.com", 443), ("1.2.3.4", 80), ("2001:db8::1", 53)] {
            let bytes = encode_address(host, port).unwrap();
            let (parsed, used) = parse_address(&bytes).unwrap();
            assert_eq!(used, bytes.len());
            let (parsed_host, parsed_port) = parsed.unwrap();
            assert_eq!(parsed_port, port);
            // IPv6 的文本形态会被归一化，比较解析后的地址而不是原串。
            match host.parse::<std::net::IpAddr>() {
                Ok(ip) => assert_eq!(parsed_host.parse::<std::net::IpAddr>().unwrap(), ip),
                Err(_) => assert_eq!(parsed_host, host),
            }
        }
        assert_eq!(parse_address(&[0xff]).unwrap(), (None, 1));
    }

    #[test]
    fn connect_frame_is_version_and_command_then_address() {
        let frame = connect_frame("example.com", 443).unwrap();
        assert_eq!(&frame[..2], &[VERSION, CMD_CONNECT]);
        assert_eq!(&frame[2..], &encode_address("example.com", 443).unwrap());
    }

    #[test]
    fn packet_round_trips_and_fragments_reassemble_out_of_order() {
        let first = packet_frame(7, 9, 2, 0, Some(("1.2.3.4", 53)), b"hello ").unwrap();
        let second = packet_frame(7, 9, 2, 1, None, b"world").unwrap();

        let parsed = parse_packet(&first).unwrap();
        assert_eq!(parsed.assoc, 7);
        assert_eq!(parsed.packet, 9);
        assert_eq!(parsed.total, 2);
        assert_eq!(parsed.address, Some(("1.2.3.4".to_string(), 53)));
        assert_eq!(parsed.payload, b"hello ");

        let mut defragger = Defragger::default();
        // 先到后一片：还没凑齐。
        assert_eq!(defragger.feed(parse_packet(&second).unwrap()), None);
        let assembled = defragger
            .feed(parse_packet(&first).unwrap())
            .expect("两片到齐必须交出去");
        assert_eq!(assembled.0, "1.2.3.4");
        assert_eq!(assembled.1, 53);
        assert_eq!(assembled.2, b"hello world");
    }

    #[test]
    fn a_single_fragment_packet_is_passed_through() {
        let frame = packet_frame(1, 1, 1, 0, Some(("example.com", 53)), b"dns").unwrap();
        let mut defragger = Defragger::default();
        let (host, port, payload) = defragger.feed(parse_packet(&frame).unwrap()).unwrap();
        assert_eq!((host.as_str(), port), ("example.com", 53));
        assert_eq!(payload, b"dns");
    }

    #[test]
    fn packets_with_a_bad_version_or_truncated_body_are_rejected() {
        let mut frame = packet_frame(1, 1, 1, 0, Some(("1.2.3.4", 53)), b"x").unwrap();
        frame[0] = 4;
        assert!(parse_packet(&frame).is_err());
        let truncated = &packet_frame(1, 1, 1, 0, Some(("1.2.3.4", 53)), b"x").unwrap()[..10];
        assert!(parse_packet(truncated).is_err());
    }

    #[test]
    fn config_defaults_alpn_to_h3_and_native_udp() {
        let config = TuicConfig {
            server: "a.com".to_string(),
            port: 443,
            sni: String::new(),
            insecure: true,
            alpn: Vec::new(),
            uuid: "11111111-2222-3333-4444-555555555555".to_string(),
            password: "pw".to_string(),
            udp_relay_mode: "native".to_string(),
        };
        assert_eq!(config.alpn_bytes(), vec![ALPN.to_vec()]);
        assert!(config.native_udp());
        assert_eq!(config.server_name(), "a.com");
    }
}
