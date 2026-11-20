//! Xray `mux.cool` 客户端（`docs/todo.md` §3-29 G15 的 Mux 部分）。
//!
//! 协议依据（2026-11-18 逐段核对上游 Xray-core，不是凭记忆）：
//!   · `common/mux/frame.go` —— 帧布局、四种状态、选项位、地址族（1=IPv4 / 2=域名 / 3=IPv6）；
//!   · `common/mux/client.go` —— 魔法目的地 `v1.mux.cool:9527`；默认并发 8；空闲检查 16s；
//!   · `common/mux/writer.go` —— `New` 带目标、`Keep` / `End` 不带目标，流式数据按 8 KiB 分块；
//!   · `common/mux/server.go` —— 服务端把目的地为 `v1.mux.cool` 的连接切换成 mux worker；
//!   · `proxy/vless/outbound/outbound.go` —— VLESS 只在域名是 `v1.mux.cool` 时用 `cmd=Mux`。
//!
//! 线格式：
//! ```text
//! metaLen u16 BE | sessionID u16 BE | status u8 | option u8 | 额外元数据
//!   [option & 0x01] dataLen u16 BE | data
//! 额外元数据：
//!   New(1)       network u8 | port u16 BE | ATYP + addr        （TCP 的 network=0x01）
//!   Keep(2)      无（TCP 的后续数据帧不带地址；UDP 才带 network + 地址）
//!   End(3)       无
//!   KeepAlive(4) 无
//! ```
//! `metaLen` **只覆盖元数据**（不含 `dataLen` 与 `data`）。
//!
//! 与 [`super::xudp`] 的区别：XUDP 是「一条 mux 流里混多个 UDP 目标」，每个 Keep 帧都要带地址；
//! 这里是一批 TCP 连接各占一个 sessionID，Keep 帧只带 sid 与数据。
//!
//! **接入方式**：VLESS / VMess / Trojan 先按普通协议拨一条到 `v1.mux.cool:9527` 的连接
//! （VLESS 用 `cmd=Mux` 且不写地址），之后这条连接上跑 mux 帧。同节点的多条 TCP 流共用
//! 一条底层连接（每条流一个 sessionID，单连接最多 [`MAX_STREAMS_PER_SESSION`] 条），
//! 与 [`super::anytls`] 的池 / 读线程结构同构。
//!
//! 边界（`forbidden.md` §禁用 1：不假装能力）：
//!   · 只做 mux.cool。Clash 的 `smux`（smux / yamux / h2mux）是**另一套不兼容的协议**，
//!     不从 `smux:` 读取，也不冒充；
//!   · 不实现 UDP over mux（UDP 仍走各协议自己的 `cmd=UDP` / XUDP 路径）；
//!   · 不发 `KeepAlive` 帧：会话没有 stream 时空闲 60s 由客户端直接关掉，
//!     比 Xray 服务端的会话空闲超时更短，不需要保活占位。

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::transport::{BoxStream, Stream};
use super::Node;

/// mux 连接的魔法目的地（上游 `client.go` 的 `muxCoolAddress` / `muxCoolPort`）。
pub const MAGIC_ADDRESS: &str = "v1.mux.cool";
pub const MAGIC_PORT: u16 = 9527;

pub const STATUS_NEW: u8 = 1;
pub const STATUS_KEEP: u8 = 2;
pub const STATUS_END: u8 = 3;
pub const STATUS_KEEPALIVE: u8 = 4;
pub const OPTION_DATA: u8 = 0x01;
pub const NETWORK_TCP: u8 = 0x01;

/// 一条会话最多承载的并发 stream 数（Xray 默认 `concurrency = 8`）。
pub const MAX_STREAMS_PER_SESSION: usize = 8;

/// 会话读线程的轮询粒度，同时也是写方等待 I/O 锁的上界。
const READ_POLL: Duration = Duration::from_millis(5);

/// 没有任何 stream 的会话空闲这么久就自行关闭。
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// 单个数据帧的载荷上限（上游 `writer.go` 用 8 KiB 切分）。
const MAX_FRAME_PAYLOAD: usize = 8 * 1024;

/// 元数据长度上限（上游 `frame.go` 的 `metaLen > 512` 直接判非法）。
const MAX_META_LEN: usize = 512;

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

fn push_frame(out: &mut Vec<u8>, meta: &[u8], data: &[u8]) {
    out.extend_from_slice(&(meta.len() as u16).to_be_bytes());
    out.extend_from_slice(meta);
    if !data.is_empty() {
        out.extend_from_slice(&(data.len().min(u16::MAX as usize) as u16).to_be_bytes());
        out.extend_from_slice(&data[..data.len().min(u16::MAX as usize)]);
    }
}

/// `New` 帧：`sid | 1 | opt | network | port | addr [| dataLen | data]`。
pub fn new_frame(sid: u16, host: &str, port: u16, data: &[u8]) -> Vec<u8> {
    let mut meta = Vec::with_capacity(32);
    meta.extend_from_slice(&sid.to_be_bytes());
    meta.push(STATUS_NEW);
    meta.push(if data.is_empty() { 0 } else { OPTION_DATA });
    meta.push(NETWORK_TCP);
    meta.extend_from_slice(&port.to_be_bytes());
    meta.extend_from_slice(&encode_address(host));
    let mut out = Vec::with_capacity(2 + meta.len() + 2 + data.len());
    push_frame(&mut out, &meta, data);
    out
}

/// `Keep` 帧（TCP）：`sid | 2 | opt [| dataLen | data]`，不带地址。
pub fn keep_frame(sid: u16, data: &[u8]) -> Vec<u8> {
    let mut meta = Vec::with_capacity(4);
    meta.extend_from_slice(&sid.to_be_bytes());
    meta.push(STATUS_KEEP);
    meta.push(if data.is_empty() { 0 } else { OPTION_DATA });
    let mut out = Vec::with_capacity(2 + meta.len() + 2 + data.len());
    push_frame(&mut out, &meta, data);
    out
}

/// `End` 帧：`sid | 3 | 0`。
pub fn end_frame(sid: u16) -> Vec<u8> {
    let [high, low] = sid.to_be_bytes();
    let mut out = Vec::with_capacity(6);
    push_frame(&mut out, &[high, low, STATUS_END, 0], &[]);
    out
}

/// 一个 stream 的接收侧：对端 `Keep` 塞进队列，`End` / 会话结束置关闭位。
struct StreamState {
    data: Mutex<VecDeque<u8>>,
    closed: AtomicBool,
    wake: Condvar,
}

impl StreamState {
    fn new() -> Self {
        Self {
            data: Mutex::new(VecDeque::new()),
            closed: AtomicBool::new(false),
            wake: Condvar::new(),
        }
    }

    fn push(&self, data: &[u8]) {
        if let Ok(mut guard) = self.data.lock() {
            guard.extend(data.iter().copied());
        }
        self.wake.notify_all();
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.wake.notify_all();
    }
}

/// 一条 mux 会话：一条底层连接上承载多个 stream。
struct Session {
    io: Mutex<BoxStream>,
    streams: Mutex<HashMap<u16, Arc<StreamState>>>,
    next_sid: AtomicU32,
    closed: AtomicBool,
}

impl Session {
    fn write_bytes(&self, bytes: &[u8]) -> io::Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "mux 会话已关闭"));
        }
        let mut io = self
            .io
            .lock()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "mux 会话锁中毒"))?;
        let result = io.write_all(bytes);
        if result.is_err() {
            // 写失败说明底层连接已经不可用：立刻把会话标成关闭，池里不会再捡到它。
            drop(io);
            self.shutdown();
        }
        result
    }

    fn write_frame(&self, frame: &[u8]) -> io::Result<()> {
        self.write_bytes(frame)
    }

    /// 新开一个 stream：登记 sid → 发 `New` 帧（目标在帧里）。
    fn open_stream(&self, host: &str, port: u16) -> io::Result<(u16, Arc<StreamState>)> {
        let state = Arc::new(StreamState::new());
        let sid = {
            let mut streams = self
                .streams
                .lock()
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "mux stream 表锁中毒"))?;
            if streams.len() >= MAX_STREAMS_PER_SESSION {
                return Err(io::Error::new(io::ErrorKind::WouldBlock, "mux 会话已满"));
            }
            let mut sid = self.next_sid.fetch_add(1, Ordering::Relaxed) as u16;
            while streams.contains_key(&sid) {
                sid = sid.wrapping_add(1);
            }
            streams.insert(sid, Arc::clone(&state));
            sid
        };
        let frame = new_frame(sid, host, port, &[]);
        if let Err(error) = self.write_frame(&frame) {
            if let Ok(mut map) = self.streams.lock() {
                map.remove(&sid);
            }
            return Err(error);
        }
        Ok((sid, state))
    }

    /// 关闭整条会话（对端断开 / 出错 / 池里被丢弃）。
    fn shutdown(&self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Ok(map) = self.streams.lock() {
            for state in map.values() {
                state.close();
            }
        }
    }
}

/// 会话读线程：短超时轮询 + 按 sid 分发帧；没有 stream 且空闲超时就自行退出。
fn spawn_reader(session: Arc<Session>) {
    let _ = thread::Builder::new()
        .name("lunac-mux".to_string())
        .spawn(move || {
            let mut buffer: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 16 * 1024];
            let mut last_active = Instant::now();
            loop {
                if session.closed.load(Ordering::SeqCst) {
                    break;
                }
                let idle = session
                    .streams
                    .lock()
                    .map(|guard| guard.is_empty())
                    .unwrap_or(true);
                if idle {
                    if last_active.elapsed() >= IDLE_TIMEOUT {
                        break;
                    }
                } else {
                    last_active = Instant::now();
                }
                let outcome = match session.io.lock() {
                    Ok(mut io) => io.read(&mut chunk),
                    Err(_) => break,
                };
                match outcome {
                    Ok(0) => break,
                    Ok(read) => {
                        buffer.extend_from_slice(&chunk[..read]);
                        if !dispatch(&session, &mut buffer) {
                            break;
                        }
                    }
                    Err(ref e)
                        if e.kind() == io::ErrorKind::WouldBlock
                            || e.kind() == io::ErrorKind::TimedOut
                            || e.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            session.shutdown();
        });
}

/// 从会话读缓冲里取帧并分发；返回 `false` = 会话级结束（结构坏掉 / 未知状态）。
fn dispatch(session: &Arc<Session>, buffer: &mut Vec<u8>) -> bool {
    loop {
        if buffer.len() < 2 {
            return true;
        }
        let meta_len = u16::from_be_bytes([buffer[0], buffer[1]]) as usize;
        if meta_len < 4 || meta_len > MAX_META_LEN {
            return false;
        }
        if buffer.len() < 2 + meta_len {
            return true;
        }
        let meta = &buffer[2..2 + meta_len];
        let sid = u16::from_be_bytes([meta[0], meta[1]]);
        let status = meta[2];
        let option = meta[3];

        let mut consumed = 2 + meta_len;
        let mut data: Vec<u8> = Vec::new();
        if option & OPTION_DATA != 0 {
            if buffer.len() < consumed + 2 {
                return true;
            }
            let data_len = u16::from_be_bytes([buffer[consumed], buffer[consumed + 1]]) as usize;
            if buffer.len() < consumed + 2 + data_len {
                return true;
            }
            data = buffer[consumed + 2..consumed + 2 + data_len].to_vec();
            consumed += 2 + data_len;
        }
        buffer.drain(..consumed);

        match status {
            STATUS_NEW | STATUS_KEEP => {
                let state = session
                    .streams
                    .lock()
                    .ok()
                    .and_then(|guard| guard.get(&sid).cloned());
                match state {
                    Some(state) => {
                        if !data.is_empty() {
                            state.push(&data);
                        }
                    }
                    None => {
                        // 上游同口径：不认识的 sid 回一个 `End`，数据丢掉。
                        let _ = session.write_frame(&end_frame(sid));
                    }
                }
            }
            STATUS_END => {
                let state = session
                    .streams
                    .lock()
                    .ok()
                    .and_then(|mut guard| guard.remove(&sid));
                if let Some(state) = state {
                    state.close();
                }
            }
            // 本实现不发 `KeepAlive`，但服务端可能发；数据位为 1 时丢掉即可。
            STATUS_KEEPALIVE => {}
            _ => return false,
        }
    }
}

/// 一个已开好的 stream（`Read` / `Write` 只看自己的 sid）。
pub struct MuxStream {
    session: Arc<Session>,
    sid: u16,
    state: Arc<StreamState>,
    read_timeout: Cell<Option<Duration>>,
}

impl Read for MuxStream {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let mut guard = self
            .state
            .data
            .lock()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "mux stream 锁中毒"))?;
        loop {
            if !guard.is_empty() {
                let count = out.len().min(guard.len());
                for slot in out.iter_mut().take(count) {
                    *slot = guard.pop_front().expect("len 已核对");
                }
                return Ok(count);
            }
            if self.state.closed.load(Ordering::SeqCst)
                || self.session.closed.load(Ordering::SeqCst)
            {
                return Ok(0);
            }
            match self.read_timeout.get() {
                None => {
                    guard =
                        self.state.wake.wait(guard).map_err(|_| {
                            io::Error::new(io::ErrorKind::Other, "mux stream 锁中毒")
                        })?;
                }
                Some(timeout) => {
                    let (next, result) =
                        self.state.wake.wait_timeout(guard, timeout).map_err(|_| {
                            io::Error::new(io::ErrorKind::Other, "mux stream 锁中毒")
                        })?;
                    guard = next;
                    if result.timed_out()
                        && guard.is_empty()
                        && !self.state.closed.load(Ordering::SeqCst)
                        && !self.session.closed.load(Ordering::SeqCst)
                    {
                        // 与其它协议同口径：超时必须回 `WouldBlock`，不能回 EOF。
                        return Err(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            "mux stream 读超时",
                        ));
                    }
                }
            }
        }
    }
}

impl Write for MuxStream {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut written = 0usize;
        while written < data.len() {
            let end = (written + MAX_FRAME_PAYLOAD).min(data.len());
            self.session
                .write_frame(&keep_frame(self.sid, &data[written..end]))?;
            written = end;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut io = self
            .session
            .io
            .lock()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "mux 会话锁中毒"))?;
        io.flush()
    }
}

impl Stream for MuxStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        // 单条 stream 的超时用条件变量等，不能改会话读线程的超时。
        self.read_timeout.set(timeout);
        Ok(())
    }
}

impl Drop for MuxStream {
    fn drop(&mut self) {
        if !self.state.closed.swap(true, Ordering::SeqCst) {
            let _ = self.session.write_frame(&end_frame(self.sid));
        }
        self.state.wake.notify_all();
        if let Ok(mut map) = self.session.streams.lock() {
            map.remove(&self.sid);
        }
    }
}

/// 会话池的键：同一台服务器 + 同一份凭据 + 同一套 TLS / 传输参数才可复用。
///
/// 凭据只以 sha256 进键（池里不出现明文）；REALITY 的公钥 / shortId 也进键
/// （不同 REALITY 参数必须各建一条会话）。
pub fn session_key(node: &Node) -> String {
    let mut hasher = Sha256::new();
    hasher.update(node.protocol.kind().as_bytes());
    hasher.update(b"|");
    hasher.update(node.server.as_bytes());
    hasher.update(b"|");
    hasher.update(node.port.to_be_bytes());
    hasher.update(b"|");
    hasher.update(node.tls.sni.as_bytes());
    hasher.update(b"|");
    hasher.update(node.transport().as_bytes());
    hasher.update(b"|");
    hasher.update([node.tls.allow_insecure as u8]);
    for alpn in &node.tls.alpn {
        hasher.update(alpn.as_bytes());
        hasher.update(b",");
    }
    if let Some(reality) = &node.reality {
        hasher.update(b"|reality|");
        hasher.update(&reality.public_key);
        hasher.update(b"|");
        hasher.update(&reality.short_id);
    }
    hasher.update(b"|");
    match &node.protocol {
        super::Protocol::Vless { uuid, flow } => {
            hasher.update(uuid.as_bytes());
            hasher.update(b"|");
            hasher.update(flow.as_bytes());
        }
        super::Protocol::Vmess {
            uuid,
            alter_id,
            security,
            authenticated_length,
        } => {
            hasher.update(uuid.as_bytes());
            hasher.update(b"|");
            hasher.update(alter_id.to_be_bytes());
            hasher.update(b"|");
            hasher.update(security.as_bytes());
            hasher.update(b"|");
            hasher.update([*authenticated_length as u8]);
        }
        super::Protocol::Trojan { password } => hasher.update(password.as_bytes()),
        other => hasher.update(other.kind().as_bytes()),
    }
    let digest = hasher.finalize();
    let hex: String = digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("mux|{}:{}|{hex}", node.server, node.port)
}

fn pool() -> &'static Mutex<HashMap<String, Vec<Arc<Session>>>> {
    static POOL: OnceLock<Mutex<HashMap<String, Vec<Arc<Session>>>>> = OnceLock::new();
    POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 从池里取一条还有余量的活跃会话。
fn checkout(key: &str) -> Option<Arc<Session>> {
    let mut guard = pool().lock().ok()?;
    let picked = match guard.get_mut(key) {
        Some(list) => {
            list.retain(|session| !session.closed.load(Ordering::SeqCst));
            list.iter()
                .find(|session| {
                    session
                        .streams
                        .lock()
                        .map(|map| map.len() < MAX_STREAMS_PER_SESSION)
                        .unwrap_or(false)
                })
                .cloned()
        }
        None => None,
    };
    if guard.get(key).map(|list| list.is_empty()).unwrap_or(false) {
        guard.remove(key);
    }
    picked
}

fn drop_session(key: &str, session: &Arc<Session>) {
    session.shutdown();
    if let Ok(mut guard) = pool().lock() {
        if let Some(list) = guard.get_mut(key) {
            list.retain(|item| !Arc::ptr_eq(item, session));
        }
        if guard.get(key).map(|list| list.is_empty()).unwrap_or(false) {
            guard.remove(key);
        }
    }
}

/// 关掉池里的全部会话（引擎停止时调用，不靠 60s 空闲超时慢慢释放）。
pub fn close_all_sessions() {
    let sessions: Vec<Arc<Session>> = match pool().lock() {
        Ok(mut guard) => guard.drain().flat_map(|(_, list)| list).collect(),
        Err(_) => return,
    };
    for session in sessions {
        session.shutdown();
    }
}

/// 建 mux 会话的底层连接：普通传输 + 到 `v1.mux.cool:9527` 的协议握手。
fn open_connection(node: &Node) -> Result<BoxStream, String> {
    let stream = super::transport::connect_transport(node)?;
    match &node.protocol {
        // VLESS 的 `cmd=Mux` 只写 uuid / addon / 命令，**不写端口与地址**（上游同口径）。
        super::Protocol::Vless { uuid, flow } => super::vless::connect_mux(stream, uuid, flow),
        // VMess / Trojan 用普通命令把目的地写成 `v1.mux.cool:9527`，服务端按域名切到 mux worker。
        super::Protocol::Vmess {
            uuid,
            alter_id,
            authenticated_length,
            ..
        } => super::vmess::connect(
            stream,
            uuid,
            *alter_id,
            *authenticated_length,
            MAGIC_ADDRESS,
            MAGIC_PORT,
        ),
        super::Protocol::Trojan { password } => {
            super::trojan::connect(stream, password, MAGIC_ADDRESS, MAGIC_PORT)
        }
        other => Err(format!(
            "mux.cool 只支持 VLESS / VMess / Trojan（当前节点是 {}）",
            other.kind()
        )),
    }
}

/// 新开一条会话：设短读超时 + 起读线程；`New` 帧留给 `open_stream`。
fn start_session(stream: BoxStream) -> Result<Arc<Session>, String> {
    stream
        .set_read_timeout(Some(READ_POLL))
        .map_err(|e| format!("mux 会话读超时设置失败：{e}"))?;
    let session = Arc::new(Session {
        io: Mutex::new(stream),
        streams: Mutex::new(HashMap::new()),
        next_sid: AtomicU32::new(1),
        closed: AtomicBool::new(false),
    });
    spawn_reader(Arc::clone(&session));
    Ok(session)
}

fn register(key: &str, session: &Arc<Session>) {
    if let Ok(mut guard) = pool().lock() {
        guard
            .entry(key.to_string())
            .or_default()
            .push(Arc::clone(session));
    }
}

/// 建到 `host:port` 的 mux stream：优先复用池里同一节点的会话，满员 / 失效才拨号新建。
pub fn connect_pooled(node: &Node, host: &str, port: u16) -> Result<BoxStream, String> {
    connect_with(session_key(node), || open_connection(node), host, port)
}

/// `connect_pooled` 的可注入版本（单测用假服务端，不必真起 TLS / 协议握手）。
fn connect_with(
    key: String,
    open: impl Fn() -> Result<BoxStream, String>,
    host: &str,
    port: u16,
) -> Result<BoxStream, String> {
    if let Some(session) = checkout(&key) {
        match session.open_stream(host, port) {
            Ok((sid, state)) => {
                return Ok(Box::new(MuxStream {
                    session,
                    sid,
                    state,
                    read_timeout: Cell::new(None),
                }))
            }
            // 取会话与开流之间有别的线程把它占满了：**不能丢这条会话**（里面还有活跃流），
            // 直接去新建一条。
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            // 写失败 / 会话已关闭：丢掉它，换一条新会话。
            Err(_) => drop_session(&key, &session),
        }
    }
    let session = start_session(open()?)?;
    register(&key, &session);
    let (sid, state) = session
        .open_stream(host, port)
        .map_err(|e| format!("mux 开启 stream 失败：{e}"))?;
    Ok(Box::new(MuxStream {
        session,
        sid,
        state,
        read_timeout: Cell::new(None),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::AtomicUsize;

    /// `New` / `Keep` / `End` 三种帧的字节布局必须与上游一致。
    #[test]
    fn frames_match_the_upstream_layout() {
        let frame = new_frame(7, "example.com", 443, &[]);
        let meta_len = u16::from_be_bytes([frame[0], frame[1]]) as usize;
        assert_eq!(meta_len, 4 + 1 + 2 + 2 + 11);
        let meta = &frame[2..2 + meta_len];
        assert_eq!(&meta[..2], &7u16.to_be_bytes());
        assert_eq!(meta[2], STATUS_NEW);
        assert_eq!(meta[3], 0, "没有数据时不置 data 位");
        assert_eq!(meta[4], NETWORK_TCP);
        assert_eq!(&meta[5..7], &443u16.to_be_bytes(), "端口在地址之前");
        assert_eq!(meta[7], 0x02, "域名");
        assert_eq!(meta[8], 11);
        assert_eq!(&meta[9..], b"example.com");
        assert_eq!(frame.len(), 2 + meta_len, "空载荷不带 dataLen");

        let frame = new_frame(9, "1.2.3.4", 80, b"hi");
        let meta_len = u16::from_be_bytes([frame[0], frame[1]]) as usize;
        assert_eq!(frame[9], 0x01, "IPv4");
        assert_eq!(frame[5], OPTION_DATA, "带数据时置 data 位");
        let data_len = u16::from_be_bytes([frame[2 + meta_len], frame[2 + meta_len + 1]]);
        assert_eq!(data_len, 2);
        assert_eq!(&frame[2 + meta_len + 2..], b"hi");

        let frame = keep_frame(7, b"payload");
        let meta_len = u16::from_be_bytes([frame[0], frame[1]]) as usize;
        assert_eq!(meta_len, 4, "TCP 的 Keep 帧只有 4 字节元数据（不带地址）");
        assert_eq!(frame[4], STATUS_KEEP);
        assert_eq!(frame[5], OPTION_DATA);
        assert_eq!(&frame[6..8], &7u16.to_be_bytes());
        assert_eq!(&frame[8..], b"payload");

        let frame = end_frame(7);
        assert_eq!(frame, vec![0, 4, 0, 7, STATUS_END, 0]);
    }

    /// 池键稳定、且不出现明文凭据。
    #[test]
    fn session_key_is_stable_and_hides_credentials() {
        let node = Node {
            name: "t".to_string(),
            server: "example.com".to_string(),
            port: 443,
            protocol: super::super::Protocol::Trojan {
                password: "super-secret-password".to_string(),
            },
            tls: super::super::TlsOptions {
                enabled: true,
                sni: "example.com".to_string(),
                ..Default::default()
            },
            ws: None,
            http: None,
            extra: None,
            reality: None,
            mux: true,
        };
        let key = session_key(&node);
        assert_eq!(key, session_key(&node));
        assert!(!key.contains("super-secret-password"), "{key}");
        assert!(key.starts_with("mux|example.com:443|"));
    }

    /// 假 mux 服务端：只接受一条连接，按帧解析；Keep 帧的数据原样回写。
    fn spawn_mock(
        accepted: Arc<AtomicUsize>,
        allow_second: Arc<AtomicBool>,
        second_seen: Arc<AtomicBool>,
    ) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            accepted.fetch_add(1, Ordering::SeqCst);
            stream
                .set_read_timeout(Some(Duration::from_millis(50)))
                .unwrap();
            let mut buffer: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                // 检查是否还有第二条连接排队（只允许一条）。
                if allow_second.load(Ordering::SeqCst) {
                    listener.set_nonblocking(true).unwrap();
                    if listener.accept().is_ok() {
                        second_seen.store(true, Ordering::SeqCst);
                    }
                    return;
                }
                while buffer.len() >= 2 {
                    let meta_len = u16::from_be_bytes([buffer[0], buffer[1]]) as usize;
                    if buffer.len() < 2 + meta_len {
                        break;
                    }
                    let meta = buffer[2..2 + meta_len].to_vec();
                    let sid = u16::from_be_bytes([meta[0], meta[1]]);
                    let status = meta[2];
                    let option = meta[3];
                    let mut consumed = 2 + meta_len;
                    let mut data: Vec<u8> = Vec::new();
                    if option & OPTION_DATA != 0 {
                        if buffer.len() < consumed + 2 {
                            break;
                        }
                        let len =
                            u16::from_be_bytes([buffer[consumed], buffer[consumed + 1]]) as usize;
                        if buffer.len() < consumed + 2 + len {
                            break;
                        }
                        data = buffer[consumed + 2..consumed + 2 + len].to_vec();
                        consumed += 2 + len;
                    }
                    buffer.drain(..consumed);
                    if status == STATUS_KEEP && !data.is_empty() {
                        // 回一个 Keep 帧：`echo:<原数据>`。
                        let reply = keep_frame(
                            sid,
                            format!("echo:{}", String::from_utf8_lossy(&data)).as_bytes(),
                        );
                        let _ = stream.write_all(&reply);
                    }
                }
                match stream.read(&mut chunk) {
                    Ok(0) => return,
                    Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                    Err(_) => {}
                }
            }
        });
        addr
    }

    /// 两条 TCP 流复用同一条底层连接；第一条关掉不影响第二条。
    #[test]
    fn two_streams_share_one_connection_and_end_only_closes_its_own() {
        let accepted = Arc::new(AtomicUsize::new(0));
        let allow_second = Arc::new(AtomicBool::new(false));
        let second_seen = Arc::new(AtomicBool::new(false));
        let addr = spawn_mock(accepted.clone(), allow_second.clone(), second_seen.clone());
        let key = format!("mux-test|{addr}");

        let open = || -> Result<BoxStream, String> {
            TcpStream::connect(addr)
                .map(|stream| Box::new(stream) as BoxStream)
                .map_err(|e| e.to_string())
        };

        let mut first = connect_with(key.clone(), open, "one.test", 80).unwrap();
        first
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        first.write_all(b"alpha").unwrap();
        let mut out = [0u8; 10];
        first.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"echo:alpha");

        let mut second = connect_with(key.clone(), open, "two.test", 80).unwrap();
        second
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        second.write_all(b"beta").unwrap();
        let mut out = [0u8; 9];
        second.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"echo:beta");

        // 关掉第一条；第二条仍可收发（End 只关自己的 sid）。
        drop(first);
        thread::sleep(Duration::from_millis(100));
        second.write_all(b"gamma").unwrap();
        let mut out = [0u8; 10];
        second.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"echo:gamma");

        // 会话确实只有一条底层连接：再等一会儿，服务端不应 accept 到第二条。
        thread::sleep(Duration::from_millis(200));
        assert_eq!(accepted.load(Ordering::SeqCst), 1, "两条流必须复用一条连接");
        allow_second.store(true, Ordering::SeqCst);
        thread::sleep(Duration::from_millis(150));
        assert!(!second_seen.load(Ordering::SeqCst), "不允许出现第二条连接");
        drop(second);
    }
}
