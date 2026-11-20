//! Shadowsocks 2022（SIP022）TCP 客户端。
//!
//! 协议依据（2026-11-15 联网核对原文
//! <https://github.com/shadowsocks/shadowsocks-org/blob/main/docs/doc/sip022.md>）：
//!   · PSK 必须是**固定长度的随机密钥**（base64 传输），不再用 `EVP_BytesToKey`；
//!   · 子密钥 `session_subkey = blake3::derive_key("shadowsocks 2022 session subkey", PSK ‖ salt)`，
//!     salt 长度 = PSK 长度；
//!   · nonce 是 **12 字节小端计数器**，每加/解密一次 +1；
//!   · 请求流 = `salt ‖ 定长头块 ‖ 变长头块 ‖ (长度块 ‖ 载荷块)*`，
//!     定长头明文 = `type(0) | timestamp(u64be) | length(u16be)`（11 字节），
//!     变长头明文 = `ATYP+地址+端口 | padding 长度(u16be) | padding | 初始载荷`；
//!   · 响应流 = `服务端 salt ‖ 定长头块（同时充当第一个长度块）‖ (载荷块 ‖ 长度块)*`，
//!     定长头明文 = `type(1) | timestamp | 请求 salt | length`，客户端必须校验 type 与请求 salt；
//!   · salt + 两个头块**必须在一次 write 里发出**（规范明写，否则包长可被指纹识别）。
//!
//! **本版口径（如实登记）**：
//!   · 单用户 PSK 与 **SIP023 多用户**（`iPSK:uPSK[:…]`）都做：salt 之后、第一个 AEAD 块之前
//!     逐层插入 16 字节 identity header（明文 = `blake3(下一层 PSK)[..16]`，密钥 =
//!     `blake3::derive_key("shadowsocks 2022 identity subkey", 本层 PSK ‖ salt)` 的 AES 单块）；
//!     会话子密钥仍由**最后一段**（uPSK）派生；UDP 方向的多用户还没做（会明确报错）；
//!   · `2022-blake3-chacha20-poly1305` 一并支持（同一套分块，只换 AEAD）；
//!   · UDP（SIP022 §3.2）见本文件下半部分的 [`UdpClient`]（只支持 aes 方法，见其文档）。

use std::io::{self, Read, Write};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aes::cipher::{BlockDecrypt, BlockEncrypt};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Aes256Gcm};
use base64::Engine as _;
use chacha20poly1305::ChaCha20Poly1305;

use super::encode_target;
use super::transport::{BoxStream, Stream};

const TAG_LEN: usize = 16;
/// 单个载荷块的明文上限（规范上限是 65535；这里取 32 KiB，写出的包更均匀）。
const MAX_PAYLOAD: usize = 32 * 1024;
/// 请求定长头的明文长度：`type(1) + timestamp(8) + length(2)`。
const REQUEST_HEADER_LEN: usize = 11;
/// 请求变长头的 padding 上限（SIP022 §3.1.4；Xray 客户端取的也是 900）。
const MAX_PADDING: usize = 900;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cipher2022 {
    Aes128Gcm,
    Aes256Gcm,
    ChaCha20Poly1305,
}

impl Cipher2022 {
    pub fn parse(method: &str) -> Result<Self, String> {
        match method.trim().to_ascii_lowercase().as_str() {
            "2022-blake3-aes-128-gcm" => Ok(Self::Aes128Gcm),
            "2022-blake3-aes-256-gcm" => Ok(Self::Aes256Gcm),
            "2022-blake3-chacha20-poly1305" => Ok(Self::ChaCha20Poly1305),
            other => Err(format!("不支持的 SS-2022 加密：{other}（已跳过）")),
        }
    }

    /// PSK 与 salt 的长度（两者相同）。
    pub fn key_len(self) -> usize {
        match self {
            Self::Aes128Gcm => 16,
            Self::Aes256Gcm | Self::ChaCha20Poly1305 => 32,
        }
    }

    #[cfg(test)]
    pub fn name(self) -> &'static str {
        match self {
            Self::Aes128Gcm => "2022-blake3-aes-128-gcm",
            Self::Aes256Gcm => "2022-blake3-aes-256-gcm",
            Self::ChaCha20Poly1305 => "2022-blake3-chacha20-poly1305",
        }
    }
}

/// 这个方法名是不是 SS-2022（`native_proxy::ss` 用它分流）。
pub fn is_2022(method: &str) -> bool {
    method.trim().to_ascii_lowercase().starts_with("2022-")
}

/// 分享链接里的密码字段是 base64 的 PSK（不是用户口令）。
///
/// 多用户写法是 `iPSK:uPSK`（SIP023，可继续用 `:` 串更多层）；本函数只给**单用户**场景用，
/// 遇到多段会明确报错（UDP 方向还没做 identity header）。
pub fn decode_psk(password: &str, key_len: usize) -> Result<Vec<u8>, String> {
    let mut psks = decode_psks(password, key_len)?;
    if psks.len() != 1 {
        return Err(
            "SS-2022 多用户（iPSK:uPSK，SIP023）的 UDP 尚未重建；请换单用户 PSK 节点".to_string(),
        );
    }
    Ok(psks.remove(0))
}

/// SIP023 多用户：`iPSK:uPSK[:…]`，每段都是 base64 的 `key_len` 字节。
pub fn decode_psks(password: &str, key_len: usize) -> Result<Vec<Vec<u8>>, String> {
    let parts: Vec<&str> = password
        .split(':')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() {
        return Err("SS-2022 的密码是空的".to_string());
    }
    let mut psks = Vec::with_capacity(parts.len());
    for (index, part) in parts.iter().enumerate() {
        let psk = decode_one_psk(part, key_len).map_err(|reason| {
            if parts.len() > 1 {
                format!("第 {} 段 PSK：{reason}", index + 1)
            } else {
                reason
            }
        })?;
        psks.push(psk);
    }
    Ok(psks)
}

fn decode_one_psk(text: &str, key_len: usize) -> Result<Vec<u8>, String> {
    let engines: [base64::engine::general_purpose::GeneralPurpose; 2] = [
        base64::engine::general_purpose::STANDARD,
        base64::engine::general_purpose::STANDARD_NO_PAD,
    ];
    let mut raw = None;
    for engine in engines {
        if let Ok(bytes) = engine.decode(text) {
            raw = Some(bytes);
            break;
        }
    }
    let Some(raw) = raw else {
        return Err(
            "SS-2022 的密码不是合法 base64 PSK（应为 openssl rand -base64 N 的结果）".to_string(),
        );
    };
    if raw.len() != key_len {
        return Err(format!(
            "SS-2022 PSK 长度不对：{} 需要 {key_len} 字节，实际 {} 字节",
            key_len,
            raw.len()
        ));
    }
    Ok(raw)
}

/// `session_subkey = blake3::derive_key("shadowsocks 2022 session subkey", PSK ‖ salt)`。
pub fn session_key(psk: &[u8], salt: &[u8]) -> [u8; 32] {
    let mut material = Vec::with_capacity(psk.len() + salt.len());
    material.extend_from_slice(psk);
    material.extend_from_slice(salt);
    blake3::derive_key("shadowsocks 2022 session subkey", &material)
}

/// SIP023 的 identity 子密钥：
/// `blake3::derive_key("shadowsocks 2022 identity subkey", iPSK ‖ salt)`。
fn identity_key(psk: &[u8], salt: &[u8]) -> [u8; 32] {
    let mut material = Vec::with_capacity(psk.len() + salt.len());
    material.extend_from_slice(psk);
    material.extend_from_slice(salt);
    blake3::derive_key("shadowsocks 2022 identity subkey", &material)
}

/// 多用户（SIP023）：salt 之后、第一个 AEAD 块之前，每层一个 **16 字节** identity header。
///
/// 明文 = `blake3::hash(下一层 PSK)[..16]`；密钥 = 本层 PSK 派生的 identity 子密钥，
/// 用 **AES 单块**（长度随 cipher 的 key_len：16 ⇒ AES-128，32 ⇒ AES-256）加密。
fn identity_headers(cipher: Cipher2022, psks: &[Vec<u8>], salt: &[u8]) -> Result<Vec<u8>, String> {
    if psks.len() < 2 {
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity((psks.len() - 1) * 16);
    for pair in psks.windows(2) {
        let subkey = identity_key(&pair[0], salt);
        let mut block = [0u8; 16];
        block.copy_from_slice(&blake3::hash(&pair[1]).as_bytes()[..16]);
        block_encrypt(&subkey[..cipher.key_len()], &mut block)?;
        out.extend_from_slice(&block);
    }
    Ok(out)
}

/// 请求变长头的随机 padding 长度：`1..=MAX_PADDING`（长度必须随机，定长会被指纹识别）。
fn random_padding_len() -> usize {
    let mut seed = [0u8; 4];
    let _ = getrandom::getrandom(&mut seed);
    1 + (u32::from_le_bytes(seed) as usize % MAX_PADDING)
}

/// 12 字节小端计数器 nonce。
fn nonce_bytes(counter: u64) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[..8].copy_from_slice(&counter.to_le_bytes());
    nonce
}

enum Key {
    Aes128(Box<Aes128Gcm>),
    Aes256(Box<Aes256Gcm>),
    ChaCha(Box<ChaCha20Poly1305>),
}

impl Key {
    fn new(cipher: Cipher2022, subkey: &[u8; 32]) -> Result<Self, String> {
        match cipher {
            Cipher2022::Aes128Gcm => Aes128Gcm::new_from_slice(&subkey[..16])
                .map(|key| Self::Aes128(Box::new(key)))
                .map_err(|_| "SS-2022 子密钥长度非法（aes-128-gcm）".to_string()),
            Cipher2022::Aes256Gcm => Aes256Gcm::new_from_slice(subkey)
                .map(|key| Self::Aes256(Box::new(key)))
                .map_err(|_| "SS-2022 子密钥长度非法（aes-256-gcm）".to_string()),
            Cipher2022::ChaCha20Poly1305 => ChaCha20Poly1305::new_from_slice(subkey)
                .map(|key| Self::ChaCha(Box::new(key)))
                .map_err(|_| "SS-2022 子密钥长度非法（chacha20）".to_string()),
        }
    }

    fn seal(&self, counter: u64, plain: &[u8]) -> Result<Vec<u8>, String> {
        self.seal_nonce(&nonce_bytes(counter), plain)
    }

    fn open(&self, counter: u64, data: &[u8]) -> Result<Vec<u8>, String> {
        self.open_nonce(&nonce_bytes(counter), data)
    }

    /// UDP 用：nonce 直接取独立头里的 12 字节（不是 TCP 的小端计数器）。
    fn seal_nonce(&self, nonce: &[u8; 12], plain: &[u8]) -> Result<Vec<u8>, String> {
        let sealed = match self {
            Self::Aes128(key) => key.encrypt(aes_gcm::Nonce::from_slice(nonce), plain),
            Self::Aes256(key) => key.encrypt(aes_gcm::Nonce::from_slice(nonce), plain),
            Self::ChaCha(key) => key.encrypt(chacha20poly1305::Nonce::from_slice(nonce), plain),
        };
        sealed.map_err(|_| "SS-2022 加密失败".to_string())
    }

    fn open_nonce(&self, nonce: &[u8; 12], data: &[u8]) -> Result<Vec<u8>, String> {
        let opened = match self {
            Self::Aes128(key) => key.decrypt(aes_gcm::Nonce::from_slice(nonce), data),
            Self::Aes256(key) => key.decrypt(aes_gcm::Nonce::from_slice(nonce), data),
            Self::ChaCha(key) => key.decrypt(chacha20poly1305::Nonce::from_slice(nonce), data),
        };
        opened.map_err(|_| "SS-2022 解密失败（PSK / 加密方式不匹配）".to_string())
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 读数阶段：salt → 响应定长头 → 长度块 / 载荷块 交替。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Salt,
    Header,
    Length,
    Payload,
}

pub struct Ss2022Tunnel {
    inner: BoxStream,
    cipher: Cipher2022,
    psk: Vec<u8>,
    request_salt: Vec<u8>,
    enc: Key,
    enc_counter: u64,
    dec: Option<Key>,
    dec_counter: u64,
    stage: Stage,
    next_len: usize,
    /// 半包缓冲：透传阶段读超时 5ms，一次 `read` 拿到半个块是常态。
    incoming: Vec<u8>,
    payload: Vec<u8>,
    closed: bool,
}

impl Ss2022Tunnel {
    fn decrypt(&self, counter: u64, data: &[u8]) -> io::Result<Vec<u8>> {
        let Some(key) = self.dec.as_ref() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "SS-2022 下行子密钥还没派生",
            ));
        };
        key.open(counter, data)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// 尽力消费一个完整的读取单元；`Ok(false)` = 还差字节。
    fn parse_step(&mut self) -> io::Result<bool> {
        match self.stage {
            Stage::Salt => {
                let salt_len = self.cipher.key_len();
                if self.incoming.len() < salt_len {
                    return Ok(false);
                }
                let salt: Vec<u8> = self.incoming.drain(..salt_len).collect();
                self.dec = Some(
                    Key::new(self.cipher, &session_key(&self.psk, &salt))
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
                );
                self.dec_counter = 0;
                self.stage = Stage::Header;
                Ok(true)
            }
            Stage::Header => {
                let plain_len = 1 + 8 + self.cipher.key_len() + 2;
                if self.incoming.len() < plain_len + TAG_LEN {
                    return Ok(false);
                }
                let sealed: Vec<u8> = self.incoming.drain(..plain_len + TAG_LEN).collect();
                let plain = self.decrypt(self.dec_counter, &sealed)?;
                self.dec_counter += 1;
                if plain[0] != 1 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "SS-2022 响应头类型不是 1（服务端流）",
                    ));
                }
                let salt_start = 9;
                let salt_end = salt_start + self.cipher.key_len();
                if plain[salt_start..salt_end] != self.request_salt[..] {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "SS-2022 响应头里的请求 salt 不匹配",
                    ));
                }
                self.next_len = u16::from_be_bytes([plain[salt_end], plain[salt_end + 1]]) as usize;
                self.stage = if self.next_len == 0 {
                    Stage::Length
                } else {
                    Stage::Payload
                };
                Ok(true)
            }
            Stage::Length => {
                if self.incoming.len() < 2 + TAG_LEN {
                    return Ok(false);
                }
                let sealed: Vec<u8> = self.incoming.drain(..2 + TAG_LEN).collect();
                let plain = self.decrypt(self.dec_counter, &sealed)?;
                self.dec_counter += 1;
                self.next_len = u16::from_be_bytes([plain[0], plain[1]]) as usize;
                self.stage = if self.next_len == 0 {
                    Stage::Length
                } else {
                    Stage::Payload
                };
                Ok(true)
            }
            Stage::Payload => {
                if self.incoming.len() < self.next_len + TAG_LEN {
                    return Ok(false);
                }
                let sealed: Vec<u8> = self.incoming.drain(..self.next_len + TAG_LEN).collect();
                let plain = self.decrypt(self.dec_counter, &sealed)?;
                self.dec_counter += 1;
                self.payload.extend_from_slice(&plain);
                self.stage = Stage::Length;
                Ok(true)
            }
        }
    }
}

impl Read for Ss2022Tunnel {
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
            if self.parse_step()? {
                continue;
            }
            let mut chunk = [0u8; 16 * 1024];
            let read = self.inner.read(&mut chunk)?;
            if read == 0 {
                self.closed = true;
                return Ok(0);
            }
            self.incoming.extend_from_slice(&chunk[..read]);
        }
    }
}

impl Write for Ss2022Tunnel {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        let mut written = 0usize;
        while written < data.len() {
            let end = (written + MAX_PAYLOAD).min(data.len());
            let slice = &data[written..end];
            let sealed_len = self
                .enc
                .seal(self.enc_counter, &(slice.len() as u16).to_be_bytes())
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            self.enc_counter += 1;
            let sealed_payload = self
                .enc
                .seal(self.enc_counter, slice)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            self.enc_counter += 1;
            let mut frame = Vec::with_capacity(sealed_len.len() + sealed_payload.len());
            frame.extend_from_slice(&sealed_len);
            frame.extend_from_slice(&sealed_payload);
            self.inner.write_all(&frame)?;
            written = end;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Stream for Ss2022Tunnel {
    fn set_read_timeout(&self, timeout: Option<std::time::Duration>) -> io::Result<()> {
        self.inner.set_read_timeout(timeout)
    }
}

/// 建好 TCP 后调用：一次写出 `salt + 定长头 + 变长头`，返回 SS-2022 隧道。
pub fn connect(
    mut inner: BoxStream,
    method: &str,
    password: &str,
    host: &str,
    port: u16,
) -> Result<BoxStream, String> {
    let cipher = Cipher2022::parse(method)?;
    let psks = decode_psks(password, cipher.key_len())?;
    // 会话子密钥由**最后一段** PSK（uPSK）派生；前面几段只用来生成 identity header。
    let session_psk = psks
        .last()
        .cloned()
        .ok_or_else(|| "SS-2022 的密码是空的".to_string())?;
    let mut salt = vec![0u8; cipher.key_len()];
    getrandom::getrandom(&mut salt).map_err(|_| "生成 SS-2022 salt 失败".to_string())?;
    let enc = Key::new(cipher, &session_key(&session_psk, &salt))?;
    let identities = identity_headers(cipher, &psks, &salt)?;

    // 变长头：`SOCKS5 目标地址 ‖ padding 长度 ‖ padding`。
    //
    // **padding 不能是 0**（2026-11-15 真机踩到）：SIP022 §3.1.4 要求「变长头里的 padding
    // 与 payload 至少有一样非空」，两者都为 0 时服务端 **MUST** 拒（Xray 原文
    // `request without payload and padding is not allowed`）。我们这一版不塞 early data，
    // 所以必须放一段随机长度的 padding（Xray 取 `1..=900`，内容是全 0）。
    let padding_len = random_padding_len();
    let mut variable = encode_target(host, port);
    variable.extend_from_slice(&(padding_len as u16).to_be_bytes());
    variable.resize(variable.len() + padding_len, 0);
    if variable.len() > u16::MAX as usize {
        return Err("SS-2022 目标头过长".to_string());
    }

    let mut fixed = Vec::with_capacity(REQUEST_HEADER_LEN);
    fixed.push(0u8); // HeaderTypeClientStream
    fixed.extend_from_slice(&now_secs().to_be_bytes());
    fixed.extend_from_slice(&(variable.len() as u16).to_be_bytes());

    let sealed_fixed = enc.seal(0, &fixed)?;
    let sealed_variable = enc.seal(1, &variable)?;
    let mut head = Vec::with_capacity(
        salt.len() + identities.len() + sealed_fixed.len() + sealed_variable.len(),
    );
    head.extend_from_slice(&salt);
    // SIP023：identity header 紧跟在 salt 之后、第一个 AEAD 块之前。
    head.extend_from_slice(&identities);
    head.extend_from_slice(&sealed_fixed);
    head.extend_from_slice(&sealed_variable);
    // 规范 3.1.4：salt 与两个头块必须**一次** write，否则包长会被指纹识别。
    inner
        .write_all(&head)
        .map_err(|e| format!("SS-2022 请求头发送失败：{e}"))?;

    Ok(Box::new(Ss2022Tunnel {
        inner,
        cipher,
        psk: session_psk,
        request_salt: salt,
        enc,
        enc_counter: 2,
        dec: None,
        dec_counter: 0,
        stage: Stage::Salt,
        next_len: 0,
        incoming: Vec::new(),
        payload: Vec::new(),
        closed: false,
    }))
}

// ══════════════════════════════════════════════════════════════════
//  UDP（SIP022 §3.2，2026-11-15 按原文实现）
// ══════════════════════════════════════════════════════════════════
//
// 一个数据报 = `AES-block(独立头) ‖ AEAD(主头 ‖ 载荷)`：
//   · 独立头明文 16B = `客户端会话 ID(8) ‖ 包序号 u64be(8)`；
//   · 独立头用 **AES 分组密码**（不是 AEAD）以 PSK 直接加密：aes-128-gcm ⇒ AES-128，
//     aes-256-gcm ⇒ AES-256；
//   · 子密钥 = `blake3::derive_key(…, PSK ‖ 独立头[0..8])`；
//   · nonce = `独立头[4..16]`（12B，**不是** TCP 那个小端计数器）；
//   · 主头：客户端 `type=0 | ts | paddingLen | padding | ATYP addr port`，
//     服务端 `type=1 | ts | 客户端会话 ID(8) | paddingLen | padding | ATYP addr port`。
// 重放：按 `独立头[8..16]` 的包序号做 64 位滑动窗口（§3.2.4）；时间戳偏差 > 30s 直接拒。

/// 服务端会话在「刚收到过包」的多久内拒绝被替换（§3.2.4 允许的替代策略：一新一旧）。
const SERVER_SESSION_GRACE: Duration = Duration::from_secs(60);
/// 时间戳允许的偏差（§3.2.3）。
const TIMESTAMP_SKEW: u64 = 30;

/// 一条 UDP 会话的客户端状态（按目标地址懒创建，闲置 60s 由上层回收）。
pub struct UdpClient {
    cipher: Cipher2022,
    psk: Vec<u8>,
    client_sid: [u8; 8],
    /// 客户端方向的子密钥（只依赖自己的会话 ID，建会话时派生一次）。
    client_key: Key,
    packet_id: u64,
    /// 服务端会话（新的在前，最多两个；见 §3.2.4 的「一新一旧」）。
    servers: Vec<ServerSession>,
}

struct ServerSession {
    sid: [u8; 8],
    key: Key,
    window: ReplayWindow,
    last_seen: Instant,
}

/// 64 位滑动窗口（WireGuard 同款）：挡重复与超窗口的包序号。
#[derive(Default)]
struct ReplayWindow {
    highest: Option<u64>,
    bits: u64,
}

impl ReplayWindow {
    /// `true` = 这个序号没出现过（可以继续解密）。
    fn check(&mut self, id: u64) -> bool {
        match self.highest {
            None => {
                self.highest = Some(id);
                self.bits = 1;
                true
            }
            Some(highest) if id > highest => {
                let shift = id - highest;
                self.bits = if shift >= 64 {
                    1
                } else {
                    (self.bits << shift) | 1
                };
                self.highest = Some(id);
                true
            }
            Some(highest) => {
                let diff = highest - id;
                if diff >= 64 {
                    return false;
                }
                let bit = 1u64 << diff;
                if self.bits & bit != 0 {
                    return false;
                }
                self.bits |= bit;
                true
            }
        }
    }
}

/// AES 分组加解密（PSK 长度决定 AES-128 / AES-256）。
fn block_encrypt(psk: &[u8], block: &mut [u8; 16]) -> Result<(), String> {
    use aes::cipher::generic_array::GenericArray;
    match psk.len() {
        16 => {
            let cipher = aes::Aes128::new_from_slice(psk)
                .map_err(|_| "SS-2022 AES-128 密钥长度非法".to_string())?;
            cipher.encrypt_block(GenericArray::from_mut_slice(block));
            Ok(())
        }
        32 => {
            let cipher = aes::Aes256::new_from_slice(psk)
                .map_err(|_| "SS-2022 AES-256 密钥长度非法".to_string())?;
            cipher.encrypt_block(GenericArray::from_mut_slice(block));
            Ok(())
        }
        other => Err(format!("SS-2022 的 PSK 长度 {other} 不能用于 UDP 独立头")),
    }
}

fn block_decrypt(psk: &[u8], block: &mut [u8; 16]) -> Result<(), String> {
    use aes::cipher::generic_array::GenericArray;
    match psk.len() {
        16 => {
            let cipher = aes::Aes128::new_from_slice(psk)
                .map_err(|_| "SS-2022 AES-128 密钥长度非法".to_string())?;
            cipher.decrypt_block(GenericArray::from_mut_slice(block));
            Ok(())
        }
        32 => {
            let cipher = aes::Aes256::new_from_slice(psk)
                .map_err(|_| "SS-2022 AES-256 密钥长度非法".to_string())?;
            cipher.decrypt_block(GenericArray::from_mut_slice(block));
            Ok(())
        }
        other => Err(format!("SS-2022 的 PSK 长度 {other} 不能用于 UDP 独立头")),
    }
}

impl UdpClient {
    pub fn new(method: &str, password: &str) -> Result<Self, String> {
        let cipher = Cipher2022::parse(method)?;
        if !matches!(cipher, Cipher2022::Aes128Gcm | Cipher2022::Aes256Gcm) {
            return Err(
                "SS-2022 的 chacha 方法没有定义 UDP 独立头加密（SIP022 §4 是可选项，\
                 主流实现只支持 2022-blake3-aes-128/256-gcm）；请换 aes 节点"
                    .to_string(),
            );
        }
        let psk = decode_psk(password, cipher.key_len())?;
        let mut client_sid = [0u8; 8];
        getrandom::getrandom(&mut client_sid)
            .map_err(|_| "生成 SS-2022 UDP 会话 ID 失败".to_string())?;
        let client_key = Key::new(cipher, &session_key(&psk, &client_sid))?;
        Ok(Self {
            cipher,
            psk,
            client_sid,
            client_key,
            packet_id: 0,
            servers: Vec::new(),
        })
    }

    /// 打一个客户端数据报（`type=0`，padding 长度 0）。
    pub fn seal(&mut self, host: &str, port: u16, payload: &[u8]) -> Result<Vec<u8>, String> {
        let mut separate = [0u8; 16];
        separate[..8].copy_from_slice(&self.client_sid);
        separate[8..].copy_from_slice(&self.packet_id.to_be_bytes());
        self.packet_id = self.packet_id.wrapping_add(1);
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&separate[4..16]);

        let mut plain = Vec::with_capacity(1 + 8 + 2 + 1 + 4 + payload.len() + 2);
        plain.push(0u8); // HeaderTypeClientPacket
        plain.extend_from_slice(&now_secs().to_be_bytes());
        plain.extend_from_slice(&0u16.to_be_bytes()); // padding 长度
        plain.extend_from_slice(&encode_target(host, port));
        plain.extend_from_slice(payload);
        let body = self.client_key.seal_nonce(&nonce, &plain)?;

        block_encrypt(&self.psk, &mut separate)?;
        let mut packet = Vec::with_capacity(16 + body.len());
        packet.extend_from_slice(&separate);
        packet.extend_from_slice(&body);
        Ok(packet)
    }

    /// 解一个服务端数据报；`Ok(None)` = 重放 / 不属于这条会话（静默丢弃）。
    pub fn open(&mut self, packet: &[u8]) -> Result<Option<(String, u16, Vec<u8>)>, String> {
        if packet.len() < 16 + TAG_LEN + 1 {
            return Err("SS-2022 UDP 包太短".to_string());
        }
        let mut separate = [0u8; 16];
        separate.copy_from_slice(&packet[..16]);
        block_decrypt(&self.psk, &mut separate)?;
        let mut sid = [0u8; 8];
        sid.copy_from_slice(&separate[..8]);
        let packet_id = u64::from_be_bytes(separate[8..16].try_into().expect("16 字节独立头"));
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&separate[4..16]);

        let client_sid = self.client_sid;
        let session = self.session_for(sid)?;
        if !session.window.check(packet_id) {
            return Ok(None); // 重放 / 超窗口
        }
        let plain = session.key.open_nonce(&nonce, &packet[16..])?;
        if plain.len() < 1 + 8 + 8 + 2 + 1 + 2 {
            return Err("SS-2022 UDP 主头太短".to_string());
        }
        if plain[0] != 1 {
            return Err("SS-2022 UDP 包类型不是服务端（1）".to_string());
        }
        let timestamp = u64::from_be_bytes(plain[1..9].try_into().expect("8 字节时间戳"));
        if now_secs().abs_diff(timestamp) > TIMESTAMP_SKEW {
            return Err("SS-2022 UDP 时间戳偏差超过 30 秒（按重放处理）".to_string());
        }
        if plain[9..17] != client_sid {
            return Err("SS-2022 UDP 响应里的客户端会话 ID 不匹配".to_string());
        }
        let padding = u16::from_be_bytes([plain[17], plain[18]]) as usize;
        let mut offset = 19 + padding;
        if plain.len() < offset {
            return Err("SS-2022 UDP 头部长度越界".to_string());
        }
        let Some((host, port, used)) = super::udp::parse_socks_addr(&plain[offset..]) else {
            return Err("SS-2022 UDP 响应地址不合法".to_string());
        };
        offset += used;
        session.last_seen = Instant::now();
        Ok(Some((host, port, plain[offset..].to_vec())))
    }

    /// 找到/接受一条服务端会话。§3.2.4：服务端换会话 ID 时，若旧会话 60 秒内还来过包，
    /// **拒绝**新会话（允许的替代策略）；否则旧会话降级、新会话接管。
    fn session_for(&mut self, sid: [u8; 8]) -> Result<&mut ServerSession, String> {
        if let Some(index) = self.servers.iter().position(|item| item.sid == sid) {
            return Ok(&mut self.servers[index]);
        }
        let fresh = self
            .servers
            .first()
            .map(|item| item.last_seen.elapsed() < SERVER_SESSION_GRACE)
            .unwrap_or(false);
        if fresh {
            return Err("SS-2022 UDP 服务端在 60 秒内换了会话 ID（按 §3.2.4 拒绝）".to_string());
        }
        let key = Key::new(self.cipher, &session_key(&self.psk, &sid))?;
        self.servers.insert(
            0,
            ServerSession {
                sid,
                key,
                window: ReplayWindow::default(),
                last_seen: Instant::now(),
            },
        );
        self.servers.truncate(2);
        Ok(&mut self.servers[0])
    }
}

/// 测试桩用：按服务端视角解一个客户端 UDP 包，返回 `(客户端会话 ID, 主头 ‖ 载荷)`。
#[cfg(test)]
pub(crate) fn test_open_udp_packet(
    psk: &[u8],
    packet: &[u8],
) -> Result<([u8; 8], Vec<u8>), String> {
    if packet.len() < 16 {
        return Err("包太短".to_string());
    }
    let mut separate = [0u8; 16];
    separate.copy_from_slice(&packet[..16]);
    block_decrypt(psk, &mut separate)?;
    let mut sid = [0u8; 8];
    sid.copy_from_slice(&separate[..8]);
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&separate[4..16]);
    let key = Key::new(Cipher2022::Aes128Gcm, &session_key(psk, &sid))?;
    let plain = key.open_nonce(&nonce, &packet[16..])?;
    Ok((sid, plain))
}

/// 测试桩用：按服务端视角拼一个回包（固定 aes-128-gcm、包序号 0）。
#[cfg(test)]
pub(crate) fn test_seal_udp_reply(
    psk: &[u8],
    server_sid: [u8; 8],
    client_sid: [u8; 8],
    host: &str,
    port: u16,
    payload: &[u8],
) -> Vec<u8> {
    let mut separate = [0u8; 16];
    separate[..8].copy_from_slice(&server_sid);
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&separate[4..16]);
    let key = Key::new(Cipher2022::Aes128Gcm, &session_key(psk, &server_sid)).expect("子密钥");
    let mut plain = vec![1u8];
    plain.extend_from_slice(&now_secs().to_be_bytes());
    plain.extend_from_slice(&client_sid);
    plain.extend_from_slice(&0u16.to_be_bytes());
    plain.extend_from_slice(&encode_target(host, port));
    plain.extend_from_slice(payload);
    let body = key.seal_nonce(&nonce, &plain).expect("加密");
    block_encrypt(psk, &mut separate).expect("独立头");
    let mut packet = Vec::new();
    packet.extend_from_slice(&separate);
    packet.extend_from_slice(&body);
    packet
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    /// 服务端测试桩：按 SIP022 §3.2 拼一个服务端数据报。
    fn server_reply(
        psk: &[u8],
        server_sid: [u8; 8],
        packet_id: u64,
        client_sid: [u8; 8],
        timestamp: u64,
        host: &str,
        port: u16,
        payload: &[u8],
    ) -> Vec<u8> {
        let mut separate = [0u8; 16];
        separate[..8].copy_from_slice(&server_sid);
        separate[8..].copy_from_slice(&packet_id.to_be_bytes());
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&separate[4..16]);
        let key = Key::new(Cipher2022::Aes128Gcm, &session_key(psk, &server_sid)).unwrap();
        let mut plain = vec![1u8];
        plain.extend_from_slice(&timestamp.to_be_bytes());
        plain.extend_from_slice(&client_sid);
        plain.extend_from_slice(&0u16.to_be_bytes());
        plain.extend_from_slice(&encode_target(host, port));
        plain.extend_from_slice(payload);
        let body = key.seal_nonce(&nonce, &plain).unwrap();
        block_encrypt(psk, &mut separate).unwrap();
        let mut packet = Vec::new();
        packet.extend_from_slice(&separate);
        packet.extend_from_slice(&body);
        packet
    }

    /// UDP：客户端封包 → 服务端解包（逐字段）→ 服务端回包 → 客户端解包；重放被窗口挡住。
    #[test]
    fn udp_round_trip_and_replay_window() {
        let psk_text = "MDEyMzQ1Njc4OWFiY2RlZg==";
        let psk = decode_psk(psk_text, 16).unwrap();
        let mut client = UdpClient::new("2022-blake3-aes-128-gcm", psk_text).unwrap();
        let packet = client.seal("example.com", 53, b"query").unwrap();

        // 服务端视角：独立头用 PSK 解，body 用会话子密钥解。
        let mut separate = [0u8; 16];
        separate.copy_from_slice(&packet[..16]);
        block_decrypt(&psk, &mut separate).unwrap();
        let mut client_sid = [0u8; 8];
        client_sid.copy_from_slice(&separate[..8]);
        assert_eq!(
            u64::from_be_bytes(separate[8..16].try_into().unwrap()),
            0,
            "第一个包的序号从 0 开始"
        );
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&separate[4..16]);
        let key = Key::new(Cipher2022::Aes128Gcm, &session_key(&psk, &client_sid)).unwrap();
        let plain = key.open_nonce(&nonce, &packet[16..]).unwrap();
        assert_eq!(plain[0], 0, "客户端包类型是 0");
        let timestamp = u64::from_be_bytes(plain[1..9].try_into().unwrap());
        assert!(now_secs().abs_diff(timestamp) <= 1);
        assert_eq!(&plain[9..11], &0u16.to_be_bytes(), "padding 长度 0");
        let (host, port, used) = super::super::udp::parse_socks_addr(&plain[11..]).unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 53);
        assert_eq!(&plain[11 + used..], b"query");

        // 第二个包的序号必须是 1（同一会话递增）。
        let second = client.seal("example.com", 53, b"again").unwrap();
        let mut separate2 = [0u8; 16];
        separate2.copy_from_slice(&second[..16]);
        block_decrypt(&psk, &mut separate2).unwrap();
        assert_eq!(u64::from_be_bytes(separate2[8..16].try_into().unwrap()), 1);
        assert_eq!(&separate2[..8], &client_sid[..], "同一会话 ID 不变");

        let mut server_sid = [0u8; 8];
        server_sid[0] = 0x7e;
        let reply = server_reply(
            &psk,
            server_sid,
            0,
            client_sid,
            now_secs(),
            "1.1.1.1",
            53,
            b"answer",
        );
        let (host, port, payload) = client.open(&reply).unwrap().expect("必须解出服务端包");
        assert_eq!(host, "1.1.1.1");
        assert_eq!(port, 53);
        assert_eq!(payload, b"answer");
        assert!(
            client.open(&reply).unwrap().is_none(),
            "同一个包重放必须被滑动窗口挡住"
        );
    }

    /// 时间戳偏差 > 30s、客户端会话 ID 不匹配、60 秒内换服务端会话 —— 都要拒绝。
    #[test]
    fn udp_rejects_stale_timestamp_foreign_sid_and_session_switch() {
        let psk_text = "MDEyMzQ1Njc4OWFiY2RlZg==";
        let psk = decode_psk(psk_text, 16).unwrap();
        let mut client = UdpClient::new("2022-blake3-aes-128-gcm", psk_text).unwrap();
        let packet = client.seal("example.com", 53, b"query").unwrap();
        let mut separate = [0u8; 16];
        separate.copy_from_slice(&packet[..16]);
        block_decrypt(&psk, &mut separate).unwrap();
        let mut client_sid = [0u8; 8];
        client_sid.copy_from_slice(&separate[..8]);

        let stale = server_reply(
            &psk,
            [1u8; 8],
            0,
            client_sid,
            now_secs().saturating_sub(31),
            "1.1.1.1",
            53,
            b"x",
        );
        let error = client.open(&stale).err().expect("过期时间戳必须拒绝");
        assert!(error.contains("时间戳"), "{error}");

        let foreign = server_reply(&psk, [1u8; 8], 1, [9u8; 8], now_secs(), "1.1.1.1", 53, b"x");
        let error = client.open(&foreign).err().expect("别的会话的包必须拒绝");
        assert!(error.contains("会话 ID"), "{error}");

        // 先接受一个正常会话，紧接着换一个服务端会话 ID ⇒ 按 §3.2.4 拒绝。
        let ok = server_reply(
            &psk,
            [1u8; 8],
            2,
            client_sid,
            now_secs(),
            "1.1.1.1",
            53,
            b"x",
        );
        assert!(client.open(&ok).unwrap().is_some());
        let switched = server_reply(
            &psk,
            [2u8; 8],
            0,
            client_sid,
            now_secs(),
            "1.1.1.1",
            53,
            b"y",
        );
        let error = client.open(&switched).err().expect("60 秒内换会话必须拒绝");
        assert!(error.contains("60 秒"), "{error}");
    }

    #[test]
    fn udp_rejects_chacha_methods_with_a_reason() {
        let psk32 = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
        let error = UdpClient::new("2022-blake3-chacha20-poly1305", &psk32)
            .err()
            .expect("chacha 的 UDP 没有独立头定义");
        assert!(error.contains("chacha"), "{error}");
    }

    #[test]
    fn method_dispatch_and_key_lengths() {
        assert!(is_2022("2022-blake3-aes-128-gcm"));
        assert!(is_2022("  2022-BLAKE3-AES-256-GCM "));
        assert!(!is_2022("aes-128-gcm"));
        assert_eq!(
            Cipher2022::parse("2022-blake3-aes-128-gcm")
                .unwrap()
                .key_len(),
            16
        );
        assert_eq!(
            Cipher2022::parse("2022-blake3-aes-256-gcm")
                .unwrap()
                .key_len(),
            32
        );
        assert_eq!(
            Cipher2022::parse("2022-blake3-chacha20-poly1305")
                .unwrap()
                .name(),
            "2022-blake3-chacha20-poly1305"
        );
        assert!(Cipher2022::parse("2022-blake3-des").is_err());
    }

    #[test]
    fn psk_must_be_base64_of_the_exact_key_length() {
        // 16 字节全 0 的 base64。
        let psk = decode_psk("AAAAAAAAAAAAAAAAAAAAAA==", 16).unwrap();
        assert_eq!(psk, vec![0u8; 16]);
        // 无 padding 的写法也接受。
        assert_eq!(decode_psk("AAAAAAAAAAAAAAAAAAAAAA", 16).unwrap().len(), 16);
        // 长度不对要明确指出。
        let error = decode_psk("AAAAAAAAAAAAAAAAAAAAAA==", 32).err().unwrap();
        assert!(error.contains("长度不对"), "{error}");
        // 多用户写法在「单用户入口」明确拒绝（UDP 还没做 identity header）。
        let multi = "AAAAAAAAAAAAAAAAAAAAAA==:AAAAAAAAAAAAAAAAAAAAAA==";
        let error = decode_psk(multi, 16).err().unwrap();
        assert!(error.contains("多用户"), "{error}");
        // 多用户本身能解码（TCP 走 decode_psks）。
        assert_eq!(decode_psks(multi, 16).unwrap().len(), 2);
        // 某一段坏了要指出是哪一段。
        let error = decode_psks("AAAAAAAAAAAAAAAAAAAAAA==:bbb", 16)
            .err()
            .unwrap();
        assert!(error.contains("第 2 段"), "{error}");
        // 非 base64。
        assert!(decode_psk("不是 base64", 16).is_err());
    }

    #[test]
    fn nonce_is_little_endian_counter() {
        assert_eq!(nonce_bytes(1), [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            nonce_bytes(0x0102),
            [0x02, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn session_key_depends_on_both_psk_and_salt() {
        let psk = [7u8; 16];
        let a = session_key(&psk, &[1u8; 16]);
        let b = session_key(&psk, &[2u8; 16]);
        let c = session_key(&[8u8; 16], &[1u8; 16]);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_eq!(a, session_key(&psk, &[1u8; 16]));
    }

    /// 服务端测试桩：按 SIP022 解请求流、回响应流。
    #[test]
    fn tcp_round_trip_against_a_mock_server() {
        // 16 字节 PSK 的 base64。
        let psk_text = "MDEyMzQ1Njc4OWFiY2RlZg==";
        let psk = decode_psk(psk_text, 16).unwrap();
        let cipher = Cipher2022::Aes128Gcm;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request_salt = [0u8; 16];
            stream.read_exact(&mut request_salt).unwrap();
            let request_key = session_key(&psk, &request_salt);
            let request_aead = Key::new(cipher, &request_key).unwrap();

            // 定长头：type=0 / timestamp / 变长头长度。
            let mut sealed = [0u8; REQUEST_HEADER_LEN + TAG_LEN];
            stream.read_exact(&mut sealed).unwrap();
            let fixed = request_aead.open(0, &sealed).unwrap();
            assert_eq!(fixed[0], 0);
            let variable_len = u16::from_be_bytes([fixed[9], fixed[10]]) as usize;

            // 变长头：SOCKS5 目标地址 + padding 长度 + padding。
            let mut sealed = vec![0u8; variable_len + TAG_LEN];
            stream.read_exact(&mut sealed).unwrap();
            let variable = request_aead.open(1, &sealed).unwrap();
            assert_eq!(variable[0], 0x03);
            assert_eq!(variable[1], 11);
            assert_eq!(&variable[2..13], b"example.com");
            assert_eq!(&variable[13..15], &80u16.to_be_bytes());
            let padding_len = u16::from_be_bytes([variable[15], variable[16]]) as usize;
            // SIP022 §3.1.4：padding 与 payload 不能同时为 0。
            assert!(
                (1..=MAX_PADDING).contains(&padding_len),
                "padding 长度 {padding_len} 必须落在 [1, {MAX_PADDING}]"
            );
            assert_eq!(
                variable.len(),
                15 + 2 + padding_len,
                "变长头 = 地址 + 2 字节长度 + padding"
            );

            // 载荷块：长度块（counter=2）+ 载荷块（counter=3）。
            let mut sealed = [0u8; 2 + TAG_LEN];
            stream.read_exact(&mut sealed).unwrap();
            let plain = request_aead.open(2, &sealed).unwrap();
            let payload_len = u16::from_be_bytes([plain[0], plain[1]]) as usize;
            let mut sealed = vec![0u8; payload_len + TAG_LEN];
            stream.read_exact(&mut sealed).unwrap();
            assert_eq!(request_aead.open(3, &sealed).unwrap(), b"PING");

            // 响应：自己的 salt + 定长头（含请求 salt）+ 载荷块。
            let mut response_salt = [3u8; 16];
            response_salt[0] = 0x5a;
            let response_key = session_key(&psk, &response_salt);
            let response_aead = Key::new(cipher, &response_key).unwrap();
            let mut header = Vec::new();
            header.push(1u8);
            header.extend_from_slice(&now_secs().to_be_bytes());
            header.extend_from_slice(&request_salt);
            header.extend_from_slice(&4u16.to_be_bytes());
            let sealed_header = response_aead.seal(0, &header).unwrap();
            let sealed_payload = response_aead.seal(1, b"PONG").unwrap();
            let mut out = Vec::new();
            out.extend_from_slice(&response_salt);
            out.extend_from_slice(&sealed_header);
            out.extend_from_slice(&sealed_payload);
            stream.write_all(&out).unwrap();
        });

        let tcp = TcpStream::connect(addr).unwrap();
        let stream: BoxStream = Box::new(tcp);
        let mut tunnel = connect(
            stream,
            "2022-blake3-aes-128-gcm",
            psk_text,
            "example.com",
            80,
        )
        .unwrap();
        tunnel.write_all(b"PING").unwrap();
        let mut out = [0u8; 4];
        tunnel.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"PONG");
        // 服务端关连接 ⇒ 读到 EOF。
        let mut tail = [0u8; 1];
        assert_eq!(tunnel.read(&mut tail).unwrap(), 0);
        handle.join().unwrap();
    }

    #[test]
    fn response_with_wrong_request_salt_is_rejected() {
        let psk_text = "MDEyMzQ1Njc4OWFiY2RlZg==";
        let psk = decode_psk(psk_text, 16).unwrap();
        let cipher = Cipher2022::Aes128Gcm;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request_salt = [0u8; 16];
            stream.read_exact(&mut request_salt).unwrap();
            // 把请求流的头块读掉，避免客户端写阻塞（内容不校验）。
            let mut sealed = [0u8; REQUEST_HEADER_LEN + TAG_LEN];
            stream.read_exact(&mut sealed).unwrap();
            let request_key = session_key(&psk, &request_salt);
            let request_aead = Key::new(cipher, &request_key).unwrap();
            let fixed = request_aead.open(0, &sealed).unwrap();
            let variable_len = u16::from_be_bytes([fixed[9], fixed[10]]) as usize;
            let mut sealed = vec![0u8; variable_len + TAG_LEN];
            stream.read_exact(&mut sealed).unwrap();

            // 回一个 salt 不匹配的响应头。
            let response_salt = [9u8; 16];
            let response_aead = Key::new(cipher, &session_key(&psk, &response_salt)).unwrap();
            let mut header = Vec::new();
            header.push(1u8);
            header.extend_from_slice(&now_secs().to_be_bytes());
            header.extend_from_slice(&[0u8; 16]);
            header.extend_from_slice(&0u16.to_be_bytes());
            let mut out = Vec::new();
            out.extend_from_slice(&response_salt);
            out.extend_from_slice(&response_aead.seal(0, &header).unwrap());
            stream.write_all(&out).unwrap();
        });

        let tcp = TcpStream::connect(addr).unwrap();
        let stream: BoxStream = Box::new(tcp);
        let mut tunnel = connect(
            stream,
            "2022-blake3-aes-128-gcm",
            psk_text,
            "example.com",
            80,
        )
        .unwrap();
        let mut out = [0u8; 1];
        let error = tunnel.read(&mut out).expect_err("salt 不匹配必须报错");
        assert!(error.to_string().contains("salt"), "{error}");
        handle.join().unwrap();
    }

    /// SIP023 多用户：salt 之后紧跟一个 16 字节 identity header，服务端能解出
    /// `blake3(uPSK)[..16]`，随后用 uPSK 走完整 2022 握手。
    #[test]
    fn multi_user_psk_sends_an_identity_header() {
        use base64::Engine as _;

        let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        let identity_psk = [1u8; 16];
        let user_psk = [2u8; 16];
        let password = format!("{}:{}", encode(&identity_psk), encode(&user_psk));
        let cipher = Cipher2022::Aes128Gcm;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request_salt = [0u8; 16];
            stream.read_exact(&mut request_salt).unwrap();

            // identity header：用 iPSK 派生的子密钥单块解密，内容必须是 blake3(uPSK)[..16]。
            let mut identity = [0u8; 16];
            stream.read_exact(&mut identity).unwrap();
            let subkey = identity_key(&identity_psk, &request_salt);
            block_decrypt(&subkey[..16], &mut identity).unwrap();
            assert_eq!(
                identity,
                blake3::hash(&user_psk).as_bytes()[..16],
                "identity header 明文必须是 uPSK 的 blake3 前 16 字节"
            );

            // 之后与单用户完全一致，但会话子密钥来自 uPSK。
            let request_aead = Key::new(cipher, &session_key(&user_psk, &request_salt)).unwrap();
            let mut sealed = [0u8; REQUEST_HEADER_LEN + TAG_LEN];
            stream.read_exact(&mut sealed).unwrap();
            let fixed = request_aead.open(0, &sealed).unwrap();
            let variable_len = u16::from_be_bytes([fixed[9], fixed[10]]) as usize;
            let mut sealed = vec![0u8; variable_len + TAG_LEN];
            stream.read_exact(&mut sealed).unwrap();
            let variable = request_aead.open(1, &sealed).unwrap();
            assert_eq!(&variable[2..13], b"example.com");

            let mut sealed = [0u8; 2 + TAG_LEN];
            stream.read_exact(&mut sealed).unwrap();
            let plain = request_aead.open(2, &sealed).unwrap();
            let payload_len = u16::from_be_bytes([plain[0], plain[1]]) as usize;
            let mut sealed = vec![0u8; payload_len + TAG_LEN];
            stream.read_exact(&mut sealed).unwrap();
            assert_eq!(request_aead.open(3, &sealed).unwrap(), b"PING");

            // 响应也用 uPSK 派生。
            let response_salt = [4u8; 16];
            let response_aead = Key::new(cipher, &session_key(&user_psk, &response_salt)).unwrap();
            let mut header = Vec::new();
            header.push(1u8);
            header.extend_from_slice(&now_secs().to_be_bytes());
            header.extend_from_slice(&request_salt);
            header.extend_from_slice(&4u16.to_be_bytes());
            let mut out = Vec::new();
            out.extend_from_slice(&response_salt);
            out.extend_from_slice(&response_aead.seal(0, &header).unwrap());
            out.extend_from_slice(&response_aead.seal(1, b"PONG").unwrap());
            stream.write_all(&out).unwrap();
            let _ = stream.shutdown(std::net::Shutdown::Write);
            thread::sleep(Duration::from_millis(50));
        });

        let tcp = TcpStream::connect(addr).unwrap();
        let stream: BoxStream = Box::new(tcp);
        let mut tunnel = connect(
            stream,
            "2022-blake3-aes-128-gcm",
            &password,
            "example.com",
            80,
        )
        .unwrap();
        tunnel.write_all(b"PING").unwrap();
        let mut reply = [0u8; 4];
        tunnel.read_exact(&mut reply).unwrap();
        assert_eq!(&reply, b"PONG");
        handle.join().unwrap();
    }

    /// 多用户写法给 UDP 用时必须**明确报错**（identity header 还没进数据报）。
    #[test]
    fn multi_user_psk_is_rejected_for_udp_with_a_reason() {
        use base64::Engine as _;
        let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        let password = format!("{}:{}", encode(&[1u8; 16]), encode(&[2u8; 16]));
        let error = UdpClient::new("2022-blake3-aes-128-gcm", &password)
            .err()
            .expect("多用户 UDP 必须报错");
        assert!(error.contains("UDP"), "{error}");
    }
}
