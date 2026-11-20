//! AnyTLS 客户端（重建 v1 `native_proxy/anytls.rs`）。
//!
//! 协议依据（2026-11-15 逐段核对上游，不是凭记忆）：
//!   · <https://github.com/anytls/anytls-go/blob/main/docs/protocol.md>（认证 / 会话帧 / padding 规则）；
//!   · `proxy/padding/padding.go`（默认 paddingScheme、`stop`、`c` 检查符、md5 算法）；
//!   · `proxy/session/session.go` 的 `writeConn`（**逐行照抄**的分包 / 补 `cmdWaste` 逻辑）；
//!   · `cmd/client/myclient.go`（`padding0` 的真实取值 = 方案里 pkt 0 的尺寸，不吃 `sha256` 的 32 字节）。
//!
//! 线格式：
//! ```text
//! 认证（TLS 之后立刻发）：
//!   sha256(password)[32] | padding0 长度[2 BE] | padding0（全 0）
//! 会话帧：
//!   command[1] | streamId[4 BE] | dataLen[2 BE] | data
//! ```
//! 客户端**必须**在新会话上先发 `cmdSettings`；本实现把「settings + SYN + 首个 PSH（目标地址）」
//! 拼成同一次写（= 上游的 buffering 语义，pkt 计数器因此从 1 开始算这一包）。
//!
//! **会话复用（2026-11-16，`docs/todo.md` §3-29 G11）**：
//!   · 一条 TLS 会话上并发承载多个 stream（`cmdSYN` / `cmdPSH` / `cmdFIN` 按 sid 分发），
//!     会话按「服务器 + 凭据 + TLS 参数」在池里复用，满 `MAX_STREAMS_PER_SESSION` 个 stream
//!     再开新会话；会话空闲 60s 自行关闭；
//!   · std 的阻塞流没有 select，读写共用同一个不可拆分的 TLS 对象 ⇒ 会话读线程按 `READ_POLL`
//!     （5ms）短超时轮询，写方最多等一个轮询周期（与 `pipe()` 的 `DOWN_POLL` 同量级）；
//!   · 不主动发 `cmdHeartRequest`（上游也标了「Active keepalive checking is not implemented yet」），
//!     但**会**回 `cmdHeartResponse`，收到 `cmdUpdatePaddingScheme` 会采纳给后续新会话；
//!   · UDP 走 UoT v2（`udp.rs` 的魔法域名 `sp.v2.udp-over-tcp.arpa`），UoT 的每条流也只是
//!     会话池里的一条 stream（与 TCP 共用同一个池）。

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::ss::md5;
use super::transport::{BoxStream, Stream};

pub const CMD_WASTE: u8 = 0;
pub const CMD_SYN: u8 = 1;
pub const CMD_PSH: u8 = 2;
pub const CMD_FIN: u8 = 3;
pub const CMD_SETTINGS: u8 = 4;
pub const CMD_ALERT: u8 = 5;
pub const CMD_UPDATE_PADDING_SCHEME: u8 = 6;
pub const CMD_SYNACK: u8 = 7;
pub const CMD_HEART_REQUEST: u8 = 8;
pub const CMD_HEART_RESPONSE: u8 = 9;
pub const CMD_SERVER_SETTINGS: u8 = 10;

/// 会话帧头：`cmd[1] + sid[4] + len[2]`。
pub const FRAME_HEADER: usize = 7;

/// 上游默认 paddingScheme（与 `padding.go` 的 `defaultPaddingScheme` 逐字节一致）。
pub const DEFAULT_SCHEME: &str = "stop=8\n0=30-30\n1=100-400\n2=400-500,c,500-1000,c,500-1000,c,500-1000,c,500-1000\n3=9-9,500-1000\n4=500-1000\n5=500-1000\n6=500-1000\n7=500-1000";

/// padding 方案里的一项：固定尺寸 / 区间（TLS PlainText 字节数）或 `c` 检查符。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PadItem {
    Size(usize),
    Range(usize, usize),
    Check,
}

/// 解析后的 padding 方案。
#[derive(Clone, Debug)]
pub struct PaddingScheme {
    raw: Vec<u8>,
    stop: u32,
    rows: HashMap<u32, Vec<PadItem>>,
}

impl PaddingScheme {
    /// 解析上游格式；`stop` 缺失或非法 ⇒ `None`（上游同样拒绝）。
    pub fn parse(raw: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(raw).ok()?;
        let mut stop: Option<u32> = None;
        let mut rows: HashMap<u32, Vec<PadItem>> = HashMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            if key == "stop" {
                stop = value.parse::<u32>().ok();
                continue;
            }
            let Ok(index) = key.parse::<u32>() else {
                continue;
            };
            let mut items = Vec::new();
            for part in value.split(',') {
                let part = part.trim();
                if part == "c" {
                    items.push(PadItem::Check);
                    continue;
                }
                let Some((min, max)) = part.split_once('-') else {
                    continue;
                };
                let (Ok(min), Ok(max)) = (min.trim().parse::<usize>(), max.trim().parse::<usize>())
                else {
                    continue;
                };
                let (min, max) = (min.min(max), min.max(max));
                if min == 0 {
                    continue;
                }
                items.push(if min == max {
                    PadItem::Size(min)
                } else {
                    PadItem::Range(min, max)
                });
            }
            rows.insert(index, items);
        }
        Some(Self {
            raw: raw.to_vec(),
            stop: stop?,
            rows,
        })
    }

    pub fn default_scheme() -> Self {
        Self::parse(DEFAULT_SCHEME.as_bytes()).expect("默认 paddingScheme 必须能解析")
    }

    pub fn stop(&self) -> u32 {
        self.stop
    }

    /// `padding-md5`：上游取**原始字节**的 md5（小写 hex）。
    pub fn md5_hex(&self) -> String {
        md5(&self.raw).iter().map(|b| format!("{b:02x}")).collect()
    }

    /// 第 `pkt` 个包的分包 / 填充策略（`c` 原样保留，由 `write_conn` 解释）。
    ///
    /// 区间在 `[min, max)` 上均匀取值（上游用 `crypto/rand` 同款语义）；取不到随机数就退回 `min`。
    pub fn payload_sizes(&self, pkt: u32) -> Vec<PadItem> {
        let Some(items) = self.rows.get(&pkt) else {
            return Vec::new();
        };
        items
            .iter()
            .map(|item| match item {
                PadItem::Size(size) => PadItem::Size(*size),
                PadItem::Range(min, max) => PadItem::Size(random_in(*min, *max)),
                PadItem::Check => PadItem::Check,
            })
            .collect()
    }
}

/// `[min, max)` 上的均匀随机；`max <= min` 时返回 `min`。
fn random_in(min: usize, max: usize) -> usize {
    if max <= min {
        return min;
    }
    let mut bytes = [0u8; 8];
    if getrandom::getrandom(&mut bytes).is_err() {
        return min;
    }
    min + (u64::from_le_bytes(bytes) % (max - min) as u64) as usize
}

/// 服务端下发的方案只作用于「连到该服务器的客户端」的后续会话（上游同口径）。
fn adopted() -> &'static Mutex<Option<PaddingScheme>> {
    static ADOPTED: OnceLock<Mutex<Option<PaddingScheme>>> = OnceLock::new();
    ADOPTED.get_or_init(|| Mutex::new(None))
}

fn active_scheme() -> PaddingScheme {
    adopted()
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
        .unwrap_or_else(PaddingScheme::default_scheme)
}

fn adopt_scheme(raw: &[u8]) {
    if let Some(scheme) = PaddingScheme::parse(raw) {
        if let Ok(mut guard) = adopted().lock() {
            *guard = Some(scheme);
        }
    }
}

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// 认证包：`sha256(password) | padding0 长度[2 BE] | padding0`。
///
/// `padding0` **就是方案里 pkt 0 的尺寸**（不是「减去 34」）：上游 `myclient.go` 直接把
/// `GenerateRecordPayloadSizes(0)` 的第一个值当地址写进去。
pub fn auth_bytes(password: &str, padding0: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(34 + padding0);
    out.extend_from_slice(&sha256(password.as_bytes()));
    out.extend_from_slice(&(padding0 as u16).to_be_bytes());
    out.extend(std::iter::repeat(0u8).take(padding0));
    out
}

/// 拼一个会话帧。
pub fn frame(cmd: u8, sid: u32, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(FRAME_HEADER + data.len());
    out.push(cmd);
    out.extend_from_slice(&sid.to_be_bytes());
    out.extend_from_slice(&(data.len().min(u16::MAX as usize) as u16).to_be_bytes());
    out.extend_from_slice(&data[..data.len().min(u16::MAX as usize)]);
    out
}

/// 同一个会话最多承载的并发 stream 数（满了就再开一条会话）。
pub const MAX_STREAMS_PER_SESSION: usize = 8;

/// 会话读线程的轮询粒度，同时也是写方等待 I/O 锁的上界。
const READ_POLL: Duration = Duration::from_millis(5);

/// 没有任何 stream 的会话空闲这么久就自行关闭（不让池里的连接无限挂着）。
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// 会话的写侧状态：上游 `writeConn` 的 pkt 计数与 padding 开关都挂在**会话**上。
struct SessionIo {
    stream: BoxStream,
    scheme: PaddingScheme,
    settings: String,
    pkt: u32,
    padding: bool,
    first: bool,
}

impl SessionIo {
    /// 按上游 `writeConn` 逐行移植：先按方案分包，剩余部分用 `cmdWaste` 补到目标尺寸。
    fn write_conn(&mut self, payload: &[u8]) -> io::Result<()> {
        let mut buffer = payload.to_vec();
        if !self.padding {
            return self.stream.write_all(&buffer);
        }
        self.pkt += 1;
        let pkt = self.pkt;
        if pkt >= self.scheme.stop() {
            self.padding = false;
            return self.stream.write_all(&buffer);
        }
        let plan = self.scheme.payload_sizes(pkt);
        let mut offset = 0usize;
        for item in plan {
            let remain = buffer.len() - offset;
            match item {
                PadItem::Check => {
                    // `c`：上一个分包发完若已无用户数据，就不再补后面的填充包。
                    if remain == 0 {
                        break;
                    }
                }
                PadItem::Size(target) | PadItem::Range(target, _) => {
                    if remain > target {
                        // 这一包全被用户数据占满。
                        self.stream.write_all(&buffer[offset..offset + target])?;
                        offset += target;
                    } else if remain > 0 {
                        // 这一包 = 剩下的用户数据 + 补到 target 的 padding。
                        let pad_len = target.saturating_sub(remain + FRAME_HEADER);
                        if pad_len > 0 {
                            let waste = frame(CMD_WASTE, 0, &vec![0u8; pad_len]);
                            buffer.extend_from_slice(&waste);
                        }
                        self.stream.write_all(&buffer[offset..])?;
                        offset = buffer.len();
                    } else {
                        // 这一包全是 padding。
                        let waste = frame(CMD_WASTE, 0, &vec![0u8; target]);
                        self.stream.write_all(&waste)?;
                        offset = buffer.len();
                    }
                }
            }
        }
        if offset < buffer.len() {
            self.stream.write_all(&buffer[offset..])?;
        }
        Ok(())
    }
}

/// 一个 stream 的接收侧：服务端 `cmdPSH` 塞进队列，`cmdFIN` 置关闭位。
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

/// 一条 AnyTLS 会话：一条 TLS 连接上承载多个 stream（上游 `Session` 同语义）。
struct Session {
    io: Mutex<SessionIo>,
    streams: Mutex<HashMap<u32, Arc<StreamState>>>,
    next_sid: AtomicU32,
    closed: AtomicBool,
    /// 服务端 `cmdAlert` 的原文（会话级）。
    alert: Mutex<String>,
}

impl Session {
    fn write_frame(&self, cmd: u8, sid: u32, data: &[u8]) -> io::Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            // 会话被服务端 `cmdAlert` 掐掉时，把它的原话带进错误里（否则只剩「已关闭」）。
            let alert = self
                .alert
                .lock()
                .map(|text| text.clone())
                .unwrap_or_default();
            let message = if alert.is_empty() {
                "AnyTLS 会话已关闭".to_string()
            } else {
                format!("AnyTLS 会话已关闭：{alert}")
            };
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, message));
        }
        let bytes = frame(cmd, sid, data);
        let mut io = self
            .io
            .lock()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "AnyTLS 会话锁中毒"))?;
        io.write_conn(&bytes)
    }

    /// 新开一个 stream：SYN + PSH(目标地址) 走**同一次写**；会话的第一包再带上 settings。
    fn open_stream(&self, host: &str, port: u16) -> io::Result<(u32, Arc<StreamState>)> {
        let sid = self.next_sid.fetch_add(1, Ordering::Relaxed);
        let state = Arc::new(StreamState::new());
        self.streams
            .lock()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "AnyTLS stream 表锁中毒"))?
            .insert(sid, Arc::clone(&state));
        let mut io = self
            .io
            .lock()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "AnyTLS 会话锁中毒"))?;
        let mut bytes = if io.first {
            io.first = false;
            frame(CMD_SETTINGS, 0, io.settings.as_bytes())
        } else {
            Vec::new()
        };
        bytes.extend_from_slice(&frame(CMD_SYN, sid, &[]));
        bytes.extend_from_slice(&frame(CMD_PSH, sid, &super::encode_target(host, port)));
        if let Err(error) = io.write_conn(&bytes) {
            // 先放掉会话锁再清 stream 表（两把锁的顺序固定为「stream 表 → 会话」）。
            drop(io);
            if let Ok(mut map) = self.streams.lock() {
                map.remove(&sid);
            }
            return Err(error);
        }
        Ok((sid, state))
    }

    /// 关闭整条会话（服务端断开 / 出错 / 池里被丢弃）。
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
        .name("lunac-anytls".to_string())
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
                    Ok(mut io) => io.stream.read(&mut chunk),
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

/// 从会话读缓冲里取帧并分发；返回 `false` = 会话级结束（alert / 结构坏掉）。
fn dispatch(session: &Arc<Session>, buffer: &mut Vec<u8>) -> bool {
    while buffer.len() >= FRAME_HEADER {
        let cmd = buffer[0];
        let sid = u32::from_be_bytes([buffer[1], buffer[2], buffer[3], buffer[4]]);
        let len = u16::from_be_bytes([buffer[5], buffer[6]]) as usize;
        if buffer.len() < FRAME_HEADER + len {
            return true;
        }
        let data = buffer[FRAME_HEADER..FRAME_HEADER + len].to_vec();
        buffer.drain(..FRAME_HEADER + len);
        match cmd {
            CMD_PSH => {
                let state = session
                    .streams
                    .lock()
                    .ok()
                    .and_then(|guard| guard.get(&sid).cloned());
                if let Some(state) = state {
                    state.push(&data);
                }
            }
            CMD_FIN => {
                let state = session
                    .streams
                    .lock()
                    .ok()
                    .and_then(|mut guard| guard.remove(&sid));
                if let Some(state) = state {
                    state.close();
                }
            }
            CMD_ALERT => {
                if let Ok(mut alert) = session.alert.lock() {
                    *alert = String::from_utf8_lossy(&data).to_string();
                }
                return false;
            }
            CMD_HEART_REQUEST => {
                let _ = session.write_frame(CMD_HEART_RESPONSE, sid, &[]);
            }
            CMD_UPDATE_PADDING_SCHEME => adopt_scheme(&data),
            // SYNACK 的错误文本由随后的 FIN / 关闭体现；serverSettings 当前没有可执行项。
            CMD_SYNACK | CMD_SERVER_SETTINGS => {}
            // cmdWaste / cmdSettings / cmdHeartResponse：数据已完整读出并丢弃。
            _ => {}
        }
    }
    true
}

/// 一个已开好的 stream（`Read` / `Write` 只看自己的 sid）。
pub struct AnytlsStream {
    session: Arc<Session>,
    sid: u32,
    state: Arc<StreamState>,
    read_timeout: Cell<Option<Duration>>,
}

impl Read for AnytlsStream {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let mut guard = self
            .state
            .data
            .lock()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "AnyTLS stream 锁中毒"))?;
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
                    guard = self.state.wake.wait(guard).map_err(|_| {
                        io::Error::new(io::ErrorKind::Other, "AnyTLS stream 锁中毒")
                    })?;
                }
                Some(timeout) => {
                    let (next, result) =
                        self.state.wake.wait_timeout(guard, timeout).map_err(|_| {
                            io::Error::new(io::ErrorKind::Other, "AnyTLS stream 锁中毒")
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
                            "AnyTLS stream 读超时",
                        ));
                    }
                }
            }
        }
    }
}

impl Write for AnytlsStream {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut written = 0usize;
        while written < data.len() {
            let end = (written + u16::MAX as usize).min(data.len());
            self.session
                .write_frame(CMD_PSH, self.sid, &data[written..end])?;
            written = end;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut io = self
            .session
            .io
            .lock()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "AnyTLS 会话锁中毒"))?;
        io.stream.flush()
    }
}

impl Stream for AnytlsStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        // 单条 stream 的超时用条件变量等，不能改会话读线程的超时。
        self.read_timeout.set(timeout);
        Ok(())
    }
}

impl Drop for AnytlsStream {
    fn drop(&mut self) {
        if !self.state.closed.swap(true, Ordering::SeqCst) {
            let _ = self.session.write_frame(CMD_FIN, self.sid, &[]);
        }
        self.state.wake.notify_all();
        if let Ok(mut map) = self.session.streams.lock() {
            map.remove(&self.sid);
        }
    }
}

/// 会话池的键：同一台服务器 + 同一份凭据 + 同一套 TLS 参数才可复用。
///
/// 密码只以 sha256 进键（与认证包同一个哈希，池里不出现明文副本）。
pub fn session_key(
    server: &str,
    port: u16,
    sni: &str,
    transport: &str,
    insecure: bool,
    password: &str,
) -> String {
    let digest = sha256(password.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    format!("{server}:{port}|{sni}|{transport}|{insecure}|{digest}")
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

/// 建一条新会话：认证 → 起读线程；`SYN` 留给 `open_stream`。
fn start_session(mut stream: BoxStream, password: &str) -> Result<Arc<Session>, String> {
    let scheme = active_scheme();
    let padding0 = scheme
        .payload_sizes(0)
        .into_iter()
        .find_map(|item| match item {
            PadItem::Size(size) | PadItem::Range(size, _) => Some(size),
            PadItem::Check => None,
        })
        .unwrap_or(0);
    stream
        .write_all(&auth_bytes(password, padding0))
        .map_err(|e| format!("AnyTLS 认证包发送失败：{e}"))?;
    // 读线程靠短超时轮询（std 的阻塞流没有 select，读写共用同一个流对象）。
    stream
        .set_read_timeout(Some(READ_POLL))
        .map_err(|e| format!("AnyTLS 会话读超时设置失败：{e}"))?;
    let settings = format!(
        "v=2\nclient=lunac/{}\npadding-md5={}",
        env!("CARGO_PKG_VERSION"),
        scheme.md5_hex()
    );
    let session = Arc::new(Session {
        io: Mutex::new(SessionIo {
            stream,
            scheme,
            settings,
            pkt: 0,
            padding: true,
            first: true,
        }),
        streams: Mutex::new(HashMap::new()),
        next_sid: AtomicU32::new(1),
        closed: AtomicBool::new(false),
        alert: Mutex::new(String::new()),
    });
    spawn_reader(Arc::clone(&session));
    Ok(session)
}

/// 建到 `host:port` 的 stream：优先复用池里同一节点的会话，满员 / 失效才拨号新建。
///
/// `open` 只在需要新会话时调用（不会为了「试试看」白建一条 TLS 连接）。
pub fn connect_pooled(
    key: &str,
    open: impl Fn() -> Result<BoxStream, String>,
    password: &str,
    host: &str,
    port: u16,
) -> Result<BoxStream, String> {
    if let Some(session) = checkout(key) {
        match session.open_stream(host, port) {
            Ok((sid, state)) => {
                return Ok(Box::new(AnytlsStream {
                    session,
                    sid,
                    state,
                    read_timeout: Cell::new(None),
                }))
            }
            // 服务端已经收掉这条会话：丢掉它，换一条新会话（只重试一次）。
            Err(_) => drop_session(key, &session),
        }
    }
    let session = start_session(open()?, password)?;
    if let Ok(mut guard) = pool().lock() {
        guard
            .entry(key.to_string())
            .or_default()
            .push(Arc::clone(&session));
    }
    let (sid, state) = session
        .open_stream(host, port)
        .map_err(|e| format!("AnyTLS 开启 stream 失败：{e}"))?;
    Ok(Box::new(AnytlsStream {
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
    use std::thread;

    #[test]
    fn default_padding_scheme_matches_upstream() {
        let scheme = PaddingScheme::default_scheme();
        assert_eq!(scheme.stop(), 8);
        assert_eq!(scheme.rows.get(&0).unwrap(), &vec![PadItem::Size(30)]);
        assert_eq!(
            scheme.rows.get(&3).unwrap(),
            &vec![PadItem::Size(9), PadItem::Range(500, 1000)]
        );
        // `2=400-500,c,500-1000,c,...`：5 个尺寸 + 4 个 c 检查符。
        let pkt2 = scheme.rows.get(&2).unwrap();
        assert_eq!(pkt2.len(), 9);
        assert_eq!(pkt2[0], PadItem::Range(400, 500));
        assert_eq!(pkt2[1], PadItem::Check);
        // 取值必须落在区间里（`Range` 的下界含、上界不含，与上游 `[min, max)` 一致）。
        match scheme.payload_sizes(2)[0] {
            PadItem::Size(size) => assert!((400..500).contains(&size), "取到 {size}"),
            other => panic!("区间必须落地成具体尺寸：{other:?}"),
        }
        // 未定义的包 / stop 之后没有策略。
        assert!(scheme.payload_sizes(9).is_empty());
        // md5 是原始字节的 md5，长度固定 32 个小写 hex。
        let md5 = scheme.md5_hex();
        assert_eq!(md5.len(), 32);
        assert!(md5
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert!(
            PaddingScheme::parse(b"0=30-30").is_none(),
            "缺 stop 必须拒绝"
        );
    }

    #[test]
    fn auth_packet_carries_sha256_and_padding() {
        // sha256("password") 的标准值。
        assert_eq!(
            sha256(b"password"),
            [
                0x5e, 0x88, 0x48, 0x98, 0xda, 0x28, 0x04, 0x71, 0x51, 0xd0, 0xe5, 0x6f, 0x8d, 0xc6,
                0x29, 0x27, 0x73, 0x60, 0x3d, 0x0d, 0x6a, 0xab, 0xbd, 0xd6, 0x2a, 0x11, 0xef, 0x72,
                0x1d, 0x15, 0x42, 0xd8
            ]
        );
        let packet = auth_bytes("password", 30);
        assert_eq!(packet.len(), 32 + 2 + 30);
        assert_eq!(&packet[32..34], &[0x00, 0x1e]);
        assert!(packet[34..].iter().all(|b| *b == 0));
    }

    #[test]
    fn frame_layout_matches_spec() {
        let bytes = frame(CMD_PSH, 7, b"hi");
        assert_eq!(bytes.len(), FRAME_HEADER + 2);
        assert_eq!(bytes[0], CMD_PSH);
        assert_eq!(&bytes[1..5], &7u32.to_be_bytes());
        assert_eq!(&bytes[5..7], &[0, 2]);
        assert_eq!(&bytes[7..], b"hi");
    }

    /// 服务端测试桩：读帧（容忍 padding 造成的半包与 `cmdWaste`）。
    fn read_frame(stream: &mut TcpStream) -> Option<(u8, u32, Vec<u8>)> {
        let mut header = [0u8; FRAME_HEADER];
        stream.read_exact(&mut header).ok()?;
        let sid = u32::from_be_bytes([header[1], header[2], header[3], header[4]]);
        let len = u16::from_be_bytes([header[5], header[6]]) as usize;
        let mut data = vec![0u8; len];
        stream.read_exact(&mut data).ok()?;
        Some((header[0], sid, data))
    }

    /// 测试用：按给定会话键连（工厂每次新建一条 TCP，模拟「需要新会话时拨号」）。
    fn connect_via_mock(
        addr: std::net::SocketAddr,
        key: &str,
        password: &str,
        host: &str,
        port: u16,
    ) -> Result<BoxStream, String> {
        connect_pooled(
            key,
            || {
                TcpStream::connect(addr)
                    .map(|tcp| Box::new(tcp) as BoxStream)
                    .map_err(|e| format!("连不上测试服务端：{e}"))
            },
            password,
            host,
            port,
        )
    }

    /// 测试桩：接受一条连接并校验认证包。
    fn accept_and_authenticate(listener: &TcpListener, password: &[u8]) -> TcpStream {
        let (mut stream, _) = listener.accept().unwrap();
        let mut auth = [0u8; 34];
        stream.read_exact(&mut auth).unwrap();
        let padding0 = u16::from_be_bytes([auth[32], auth[33]]) as usize;
        let mut skip = vec![0u8; padding0];
        stream.read_exact(&mut skip).unwrap();
        assert_eq!(&auth[..32], &sha256(password), "认证包必须是 sha256(密码)");
        assert!(skip.iter().all(|b| *b == 0), "padding0 必须是 0");
        stream
    }

    /// 写完后把入站残余（padding 的 `cmdWaste`）读空再关：带未读数据直接 close 会 RST，
    /// 把最后一段下行吞掉（Windows loopback 上尤其明显）。
    fn drain_then_close(mut stream: TcpStream) {
        let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
        while read_frame(&mut stream).is_some() {}
    }

    /// 两个 stream 共用**一条**会话：只认证一次、只发一次 settings，sid 各自独立。
    #[test]
    fn two_streams_reuse_one_session_and_stay_isolated() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let key = format!("test-anytls-reuse-{addr}");
        let handle = thread::spawn(move || {
            let mut stream = accept_and_authenticate(&listener, b"pw");
            let mut settings = 0;
            let mut syns = 0;
            let mut targets: HashMap<u32, Vec<u8>> = HashMap::new();
            let mut fins = 0;
            while fins < 2 {
                let Some((cmd, sid, data)) = read_frame(&mut stream) else {
                    break;
                };
                match cmd {
                    CMD_SETTINGS => {
                        settings += 1;
                        let text = String::from_utf8_lossy(&data).to_string();
                        assert!(text.contains("v=2"), "settings 必须带 v=2");
                        assert!(text.contains("padding-md5="), "settings 必须带 padding-md5");
                    }
                    CMD_SYN => {
                        syns += 1;
                        assert_eq!(sid, syns, "sid 必须从 1 起逐个递增");
                    }
                    CMD_PSH => {
                        // 每个 sid 的第一条 PSH 是目标地址 ⇒ 回一条可区分的内容。
                        if !targets.contains_key(&sid) {
                            targets.insert(sid, data);
                            stream
                                .write_all(&frame(CMD_PSH, sid, format!("hello-{sid}").as_bytes()))
                                .unwrap();
                        }
                    }
                    CMD_FIN => fins += 1,
                    _ => {}
                }
            }
            (settings, syns)
        });

        let mut first = connect_via_mock(addr, &key, "pw", "a.com", 80).unwrap();
        let mut out = [0u8; 7];
        first.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"hello-1");
        let mut second = connect_via_mock(addr, &key, "pw", "b.com", 443).unwrap();
        second.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"hello-2", "第二条 stream 必须复用同一条会话");

        drop(first);
        drop(second);
        let (settings, syns) = handle.join().unwrap();
        assert_eq!(settings, 1, "复用会话时 settings 只发一次");
        assert_eq!(syns, 2, "每条 stream 各一个 SYN");
    }

    /// 服务端的 FIN 只关它自己的 stream，另一条继续读得到。
    #[test]
    fn a_fin_closes_only_its_own_stream() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let key = format!("test-anytls-fin-{addr}");
        let handle = thread::spawn(move || {
            let mut stream = accept_and_authenticate(&listener, b"pw");
            let mut targets = 0;
            while targets < 2 {
                let Some((cmd, sid, data)) = read_frame(&mut stream) else {
                    return;
                };
                if cmd == CMD_PSH && !data.is_empty() {
                    targets += 1;
                    if targets == 2 {
                        stream.write_all(&frame(CMD_FIN, 1, &[])).unwrap();
                        stream
                            .write_all(&frame(CMD_PSH, sid, b"still-alive"))
                            .unwrap();
                        break;
                    }
                }
            }
            drain_then_close(stream);
        });

        let mut first = connect_via_mock(addr, &key, "pw", "a.com", 80).unwrap();
        let mut second = connect_via_mock(addr, &key, "pw", "b.com", 443).unwrap();
        let mut out = [0u8; 11];
        second.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"still-alive");
        let mut tail = [0u8; 1];
        assert_eq!(
            first.read(&mut tail).unwrap(),
            0,
            "被 FIN 的 stream 读回 EOF"
        );
        handle.join().unwrap();
    }

    /// 服务端 `cmdAlert` 是会话级的：所有 stream 一起结束。
    #[test]
    fn an_alert_closes_every_stream() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let key = format!("test-anytls-alert-{addr}");
        let handle = thread::spawn(move || {
            let mut stream = accept_and_authenticate(&listener, b"pw");
            let mut targets = 0;
            while targets < 2 {
                let Some((cmd, _sid, data)) = read_frame(&mut stream) else {
                    return;
                };
                if cmd == CMD_PSH && !data.is_empty() {
                    targets += 1;
                }
            }
            stream.write_all(&frame(CMD_ALERT, 0, b"blocked")).unwrap();
            drain_then_close(stream);
        });

        let mut first = connect_via_mock(addr, &key, "pw", "a.com", 80).unwrap();
        let mut second = connect_via_mock(addr, &key, "pw", "b.com", 443).unwrap();
        let mut tail = [0u8; 1];
        assert_eq!(first.read(&mut tail).unwrap(), 0);
        assert_eq!(second.read(&mut tail).unwrap(), 0);
        handle.join().unwrap();
    }

    #[test]
    fn padding_scheme_from_server_is_adopted_for_later_sessions() {
        adopt_scheme(b"stop=2\n0=4-4\n1=8-8");
        let scheme = active_scheme();
        assert_eq!(scheme.stop(), 2);
        assert_eq!(scheme.payload_sizes(0), vec![PadItem::Size(4)]);
        // 复位，避免污染同进程其它用例。
        if let Ok(mut guard) = adopted().lock() {
            *guard = None;
        }
    }
}
