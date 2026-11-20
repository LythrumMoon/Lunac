//! Shadowsocks AEAD（重建 v1 `native_proxy/ss.rs` 的 SIP004 TCP 部分）。
//!
//! 支持：`aes-128-gcm` / `aes-256-gcm` / `chacha20-ietf-poly1305`（v1 的三种 AEAD）。
//! **SS-2022（`2022-blake3-*`，SIP022）已重建，但不在本文件**：见 [`super::ss2022`]
//! （BLAKE3 子密钥、带类型/时间戳/请求 salt 的头块、12 字节小端计数器）。
//! **SIP004 的 UDP**已重建：见本文件的 `udp_encrypt` / `udp_decrypt`（每包独立 salt、
//! nonce 恒 0）；SS-2022 的 UDP 见 [`super::ss2022::UdpClient`]。
//! **SIP003 插件**不在本文件：`simple-obfs`（http / tls）见 [`super::ss_plugin`]，
//! `v2ray-plugin` 的 websocket 模式由 `clash.rs` 折成 WS 传输（`transport.rs`）。
//!
//! 报文格式（SIP004）：
//! ```text
//! [salt][encrypted length(2) + tag][encrypted payload + tag]...
//! ```
//! TCP 方向：客户端先发一个 salt，随后所有分块用同一 subkey；服务端返回的流也以一个 salt 开头。
//! subkey = HKDF-SHA1(master_key, salt, "ss-subkey")，nonce = 12 字节小端计数器（每块 +1）。
//!
//! master_key 由密码经 OpenSSL `EVP_BytesToKey`（MD5 链）导出 —— 这是 SS AEAD 的历史口径，
//! 不是 PBKDF2；这里自己实现 MD5（RFC 1321），与 v1 的 `evp_bytes_to_key` 逐字节一致。
//!
//! 加密实现选 RustCrypto 的纯 Rust 栈（aes-gcm / chacha20poly1305 / hkdf + sha1）：
//! ring 在 Windows 上要求本机有 gcc，用户机器不一定有。

use std::io::{self, Read, Write};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Aes256Gcm};
use chacha20poly1305::ChaCha20Poly1305;
use hkdf::Hkdf;
use sha1::Sha1;

use super::encode_target;
use super::transport::{BoxStream, Stream};

const MAX_CHUNK: usize = 0x3FFF;
const TAG_LEN: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cipher {
    Aes128Gcm,
    Aes256Gcm,
    Chacha20IetfPoly1305,
}

impl Cipher {
    pub fn parse(method: &str) -> Result<Self, String> {
        match method.trim().to_ascii_lowercase().as_str() {
            "aes-128-gcm" => Ok(Self::Aes128Gcm),
            "aes-256-gcm" => Ok(Self::Aes256Gcm),
            "chacha20-ietf-poly1305" => Ok(Self::Chacha20IetfPoly1305),
            other => Err(format!(
                "不支持的 SS 加密：{other}（本版只支持 aes-128-gcm / aes-256-gcm / chacha20-ietf-poly1305）"
            )),
        }
    }

    pub fn key_len(self) -> usize {
        match self {
            Self::Aes128Gcm => 16,
            Self::Aes256Gcm | Self::Chacha20IetfPoly1305 => 32,
        }
    }

    #[cfg(test)]
    pub fn name(self) -> &'static str {
        match self {
            Self::Aes128Gcm => "aes-128-gcm",
            Self::Aes256Gcm => "aes-256-gcm",
            Self::Chacha20IetfPoly1305 => "chacha20-ietf-poly1305",
        }
    }
}

/// 一把方向确定后的 AEAD 密钥。
enum AeadKey {
    Aes128(Box<Aes128Gcm>),
    Aes256(Box<Aes256Gcm>),
    ChaCha(Box<ChaCha20Poly1305>),
}

impl AeadKey {
    fn new(cipher: Cipher, key: &[u8]) -> Result<Self, String> {
        match cipher {
            Cipher::Aes128Gcm => Aes128Gcm::new_from_slice(key)
                .map(|k| Self::Aes128(Box::new(k)))
                .map_err(|_| "SS 密钥长度非法（aes-128-gcm 需要 16 字节）".to_string()),
            Cipher::Aes256Gcm => Aes256Gcm::new_from_slice(key)
                .map(|k| Self::Aes256(Box::new(k)))
                .map_err(|_| "SS 密钥长度非法（aes-256-gcm 需要 32 字节）".to_string()),
            Cipher::Chacha20IetfPoly1305 => ChaCha20Poly1305::new_from_slice(key)
                .map(|k| Self::ChaCha(Box::new(k)))
                .map_err(|_| "SS 密钥长度非法（chacha20 需要 32 字节）".to_string()),
        }
    }

    fn seal(&self, counter: u64, plain: &[u8]) -> Result<Vec<u8>, String> {
        let nonce = nonce_bytes(counter);
        let sealed = match self {
            Self::Aes128(key) => key.encrypt(aes_gcm::Nonce::from_slice(&nonce), plain),
            Self::Aes256(key) => key.encrypt(aes_gcm::Nonce::from_slice(&nonce), plain),
            Self::ChaCha(key) => key.encrypt(chacha20poly1305::Nonce::from_slice(&nonce), plain),
        };
        sealed.map_err(|_| "SS 加密失败".to_string())
    }

    fn open(&self, counter: u64, data: &[u8]) -> Result<Vec<u8>, String> {
        let nonce = nonce_bytes(counter);
        let opened = match self {
            Self::Aes128(key) => key.decrypt(aes_gcm::Nonce::from_slice(&nonce), data),
            Self::Aes256(key) => key.decrypt(aes_gcm::Nonce::from_slice(&nonce), data),
            Self::ChaCha(key) => key.decrypt(chacha20poly1305::Nonce::from_slice(&nonce), data),
        };
        opened.map_err(|_| "SS 解密失败（密码 / 加密方式不匹配）".to_string())
    }
}

/// OpenSSL `EVP_BytesToKey`（MD5 链）。SS AEAD 的 master key 口径。
pub fn evp_bytes_to_key(password: &str, key_len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(key_len);
    let mut previous: Vec<u8> = Vec::new();
    while out.len() < key_len {
        let mut input = Vec::with_capacity(previous.len() + password.len());
        input.extend_from_slice(&previous);
        input.extend_from_slice(password.as_bytes());
        let digest = md5(&input);
        out.extend_from_slice(&digest);
        previous = digest.to_vec();
    }
    out.truncate(key_len);
    out
}

fn subkey(cipher: Cipher, master: &[u8], salt: &[u8]) -> Result<AeadKey, String> {
    let hkdf = Hkdf::<Sha1>::new(Some(salt), master);
    let mut okm = vec![0u8; cipher.key_len()];
    hkdf.expand(b"ss-subkey", &mut okm)
        .map_err(|_| "SS 子密钥派生失败".to_string())?;
    AeadKey::new(cipher, &okm)
}

fn nonce_bytes(counter: u64) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[..8].copy_from_slice(&counter.to_le_bytes());
    nonce
}

/// SIP004 的 UDP：每个数据报都是 `salt ‖ AEAD(subkey, nonce=0, ATYP addr port ‖ payload)`。
///
/// 与 TCP 不同，UDP 没有分块与非零计数器：一个数据报就是一条独立消息。
/// SS-2022 的 UDP（SIP022 §3.2，带会话 ID / 包序号 / 重放窗口）在
/// [`super::ss2022::UdpClient`] 里实现，这两个函数只做 SIP004，因此会明确拒绝。
pub fn udp_encrypt(method: &str, password: &str, plain: &[u8]) -> Result<Vec<u8>, String> {
    if super::ss2022::is_2022(method) {
        return Err(
            "SS-2022 的 UDP 走 ss2022::UdpClient（SIP022 §3.2），不是 SIP004 的逐包 salt"
                .to_string(),
        );
    }
    let cipher = Cipher::parse(method)?;
    let master = evp_bytes_to_key(password, cipher.key_len());
    let mut salt = vec![0u8; cipher.key_len()];
    getrandom::getrandom(&mut salt).map_err(|_| "生成 SS UDP salt 失败".to_string())?;
    let key = subkey(cipher, &master, &salt)?;
    let sealed = key.seal(0, plain)?;
    salt.extend_from_slice(&sealed);
    Ok(salt)
}

/// SIP004 的 UDP 解密（失败 = 口令 / 加密方式不匹配）。
pub fn udp_decrypt(method: &str, password: &str, packet: &[u8]) -> Result<Vec<u8>, String> {
    if super::ss2022::is_2022(method) {
        return Err(
            "SS-2022 的 UDP 走 ss2022::UdpClient（SIP022 §3.2），不是 SIP004 的逐包 salt"
                .to_string(),
        );
    }
    let cipher = Cipher::parse(method)?;
    let salt_len = cipher.key_len();
    if packet.len() <= salt_len {
        return Err("SS UDP 数据报过短".to_string());
    }
    let master = evp_bytes_to_key(password, salt_len);
    let key = subkey(cipher, &master, &packet[..salt_len])?;
    key.open(0, &packet[salt_len..])
}

/// 建好 TCP 后调用：写 salt + 首个分块（目标地址），返回 SS 隧道。
///
/// `2022-blake3-*` 不在本文件的 SIP004 路径里：分流到 [`super::ss2022::connect`]
/// （子密钥派生、头块格式、AEAD 计数器都不同）。
pub fn connect(
    mut inner: BoxStream,
    method: &str,
    password: &str,
    host: &str,
    port: u16,
) -> Result<BoxStream, String> {
    if super::ss2022::is_2022(method) {
        return super::ss2022::connect(inner, method, password, host, port);
    }
    let cipher = Cipher::parse(method)?;
    let master = evp_bytes_to_key(password, cipher.key_len());
    let mut salt = vec![0u8; cipher.key_len()];
    getrandom::getrandom(&mut salt).map_err(|_| "生成 SS salt 失败".to_string())?;
    let enc_key = subkey(cipher, &master, &salt)?;
    inner
        .write_all(&salt)
        .map_err(|e| format!("SS salt 发送失败：{e}"))?;
    let mut tunnel = SsTunnel {
        inner,
        cipher,
        enc_key: Some(enc_key),
        dec_key: None,
        dec_master: master,
        enc_nonce: 0,
        dec_nonce: 0,
        dec_started: false,
        incoming: Vec::new(),
        payload: Vec::new(),
    };
    tunnel
        .write_all(&encode_target(host, port))
        .map_err(|e| format!("SS 目标地址发送失败：{e}"))?;
    Ok(Box::new(tunnel))
}

struct SsTunnel {
    inner: BoxStream,
    cipher: Cipher,
    enc_key: Option<AeadKey>,
    dec_key: Option<AeadKey>,
    /// master key：下行 salt 到手后才能派生下行子密钥。
    dec_master: Vec<u8>,
    enc_nonce: u64,
    dec_nonce: u64,
    dec_started: bool,
    /// 还没凑够一个完整分块的密文。
    ///
    /// **必须缓冲**：透传阶段的读超时是 5ms（见 `server::pipe`），一次 `read` 只拿到
    /// 半个分块是常态；用 `read_exact` 会把已读到的字节丢掉，整条流就错位了。
    incoming: Vec<u8>,
    payload: Vec<u8>,
}

impl SsTunnel {
    /// 尽力把 `incoming` 里的完整分块解出来；返回是否解出了明文。
    fn parse_chunk(&mut self) -> io::Result<bool> {
        if !self.dec_started {
            let key_len = self.cipher.key_len();
            if self.incoming.len() < key_len {
                return Ok(false);
            }
            let salt: Vec<u8> = self.incoming.drain(..key_len).collect();
            let key = subkey(self.cipher, &self.dec_master, &salt)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            self.dec_key = Some(key);
            self.dec_started = true;
        }
        const LEN_BLOCK: usize = 2 + TAG_LEN;
        if self.incoming.len() < LEN_BLOCK {
            return Ok(false);
        }
        let len_plain = {
            let key = self
                .dec_key
                .as_ref()
                .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "SS 下行密钥缺失"))?;
            key.open(self.dec_nonce, &self.incoming[..LEN_BLOCK])
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
        };
        if len_plain.len() != 2 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "SS 分块长度字段非法",
            ));
        }
        let payload_len = u16::from_be_bytes([len_plain[0], len_plain[1]]) as usize;
        if payload_len == 0 || payload_len > MAX_CHUNK {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "SS 分块长度越界",
            ));
        }
        let total = LEN_BLOCK + payload_len + TAG_LEN;
        if self.incoming.len() < total {
            return Ok(false);
        }
        let plain = {
            let key = self
                .dec_key
                .as_ref()
                .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "SS 下行密钥缺失"))?;
            key.open(self.dec_nonce + 1, &self.incoming[LEN_BLOCK..total])
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
        };
        if plain.len() != payload_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "SS 分块解密长度不符",
            ));
        }
        self.incoming.drain(..total);
        self.dec_nonce += 2;
        self.payload.extend_from_slice(&plain);
        Ok(true)
    }
}

impl Read for SsTunnel {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        loop {
            if !self.payload.is_empty() {
                let count = out.len().min(self.payload.len());
                out[..count].copy_from_slice(&self.payload[..count]);
                self.payload.drain(..count);
                return Ok(count);
            }
            if self.parse_chunk()? {
                continue;
            }
            let mut chunk = [0u8; 16 * 1024];
            let read = self.inner.read(&mut chunk)?;
            if read == 0 {
                return Ok(0);
            }
            self.incoming.extend_from_slice(&chunk[..read]);
        }
    }
}

impl Stream for SsTunnel {
    fn set_read_timeout(&self, timeout: Option<std::time::Duration>) -> io::Result<()> {
        self.inner.set_read_timeout(timeout)
    }
}

impl Write for SsTunnel {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        let key = self
            .enc_key
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "SS 上行密钥缺失"))?;
        let mut written = 0;
        while written < data.len() {
            let end = (written + MAX_CHUNK).min(data.len());
            let chunk = &data[written..end];
            let len_bytes = (chunk.len() as u16).to_be_bytes();
            let sealed_len = key
                .seal(self.enc_nonce, &len_bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            self.enc_nonce += 1;
            let sealed_body = key
                .seal(self.enc_nonce, chunk)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            self.enc_nonce += 1;
            self.inner.write_all(&sealed_len)?;
            self.inner.write_all(&sealed_body)?;
            written = end;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

// ── MD5（RFC 1321；只给 EVP_BytesToKey 用，避免为它引入一个 crate）──────────

pub fn md5(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];
    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;

    let mut message = input.to_vec();
    let bit_len = (input.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_le_bytes());

    for chunk in message.chunks(64) {
        let mut m = [0u32; 16];
        for (index, word) in m.iter_mut().enumerate() {
            *word = u32::from_le_bytes([
                chunk[index * 4],
                chunk[index * 4 + 1],
                chunk[index * 4 + 2],
                chunk[index * 4 + 3],
            ]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let temp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                a.wrapping_add(f)
                    .wrapping_add(K[i])
                    .wrapping_add(m[g])
                    .rotate_left(S[i]),
            );
            a = temp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    out[..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..].copy_from_slice(&d0.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_matches_rfc1321_vectors() {
        assert_eq!(
            md5(b""),
            [
                0xd4, 0x1d, 0x8c, 0xd9, 0x8f, 0x00, 0xb2, 0x04, 0xe9, 0x80, 0x09, 0x98, 0xec, 0xf8,
                0x42, 0x7e
            ]
        );
        assert_eq!(
            md5(b"abc"),
            [
                0x90, 0x01, 0x50, 0x98, 0x3c, 0xd2, 0x4f, 0xb0, 0xd6, 0x96, 0x3f, 0x7d, 0x28, 0xe1,
                0x7f, 0x72
            ]
        );
    }

    #[test]
    fn evp_bytes_to_key_matches_openssl_vector() {
        // `openssl enc -aes-128-cbc -pass pass:password -P -md md5` 的 key。
        let key = evp_bytes_to_key("password", 16);
        assert_eq!(
            key,
            vec![
                0x5f, 0x4d, 0xcc, 0x3b, 0x5a, 0xa7, 0x65, 0xd6, 0x1d, 0x83, 0x27, 0xde, 0xb8, 0x82,
                0xcf, 0x99
            ]
        );
    }

    #[test]
    fn cipher_names_are_stable() {
        assert_eq!(Cipher::parse("aes-128-gcm").unwrap().name(), "aes-128-gcm");
        assert_eq!(Cipher::parse("AES-256-GCM").unwrap().name(), "aes-256-gcm");
        assert_eq!(
            Cipher::parse("chacha20-ietf-poly1305").unwrap().name(),
            "chacha20-ietf-poly1305"
        );
        assert!(Cipher::parse("rc4-md5").is_err());
    }

    #[test]
    fn chunks_seal_and_open_round_trip() {
        for cipher in [
            Cipher::Aes128Gcm,
            Cipher::Aes256Gcm,
            Cipher::Chacha20IetfPoly1305,
        ] {
            let master = vec![7u8; cipher.key_len()];
            let salt = vec![9u8; cipher.key_len()];
            let key = subkey(cipher, &master, &salt).unwrap();
            let plain = b"hello shadowsocks".to_vec();
            let sealed = key.seal(0, &plain).unwrap();
            let opened = key.open(0, &sealed).unwrap();
            assert_eq!(opened, plain);
        }
    }

    #[test]
    fn tampered_ciphertext_fails_authentication() {
        let key = subkey(Cipher::Aes128Gcm, &[1u8; 16], &[2u8; 16]).unwrap();
        let mut sealed = key.seal(3, b"payload").unwrap();
        sealed[0] ^= 0xFF;
        assert!(key.open(3, &sealed).is_err());
    }

    #[test]
    fn nonce_is_little_endian_counter() {
        assert_eq!(nonce_bytes(1)[0], 1);
        assert_eq!(nonce_bytes(256)[1], 1);
        assert_eq!(nonce_bytes(1)[8..], [0, 0, 0, 0]);
    }

    #[test]
    fn sip004_udp_round_trip_and_wrong_password_fails() {
        // `ATYP=domain + example.com + port 53 + "dns"` 的真实数据报形状。
        let plain = {
            let mut bytes = vec![0x03, 11];
            bytes.extend_from_slice(b"example.com");
            bytes.extend_from_slice(&53u16.to_be_bytes());
            bytes.extend_from_slice(b"dns");
            bytes
        };
        for method in ["aes-128-gcm", "aes-256-gcm", "chacha20-ietf-poly1305"] {
            let packet = udp_encrypt(method, "pw", &plain).unwrap();
            assert_eq!(udp_decrypt(method, "pw", &packet).unwrap(), plain);
            assert!(udp_decrypt(method, "other", &packet).is_err());
            // 每个数据报都要换 salt（前 key_len 字节不能相同到离谱）。
            assert_ne!(packet, udp_encrypt(method, "pw", &plain).unwrap());
        }
        // SS-2022 的 UDP 由 ss2022::UdpClient 承载，这两个函数必须明确拒绝。
        assert!(udp_encrypt("2022-blake3-aes-128-gcm", "pw", &plain).is_err());
        assert!(udp_decrypt("2022-blake3-aes-128-gcm", "pw", &[0u8; 32]).is_err());
        assert!(udp_decrypt("aes-128-gcm", "pw", &[0u8; 4]).is_err());
    }

    /// 一次只吐 1 个字节的流：模拟透传阶段 5ms 读超时把分块切碎的情形。
    struct Drip {
        data: Vec<u8>,
        position: usize,
        written: usize,
    }

    impl Read for Drip {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            if out.is_empty() || self.position >= self.data.len() {
                return Ok(0);
            }
            let count = 1.min(out.len()).min(self.data.len() - self.position);
            out[..count].copy_from_slice(&self.data[self.position..self.position + count]);
            self.position += count;
            Ok(count)
        }
    }

    impl Write for Drip {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.written += buf.len();
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Stream for Drip {}

    #[test]
    fn fragmented_reads_survive_partial_chunks() {
        let cipher = Cipher::Aes128Gcm;
        let master = evp_bytes_to_key("pw", cipher.key_len());
        let salt = [3u8; 16];
        let key = subkey(cipher, &master, &salt).unwrap();
        let mut response = salt.to_vec();
        let mut nonce = 0u64;
        for text in [b"hello".as_slice(), b"world".as_slice()] {
            response
                .extend_from_slice(&key.seal(nonce, &(text.len() as u16).to_be_bytes()).unwrap());
            response.extend_from_slice(&key.seal(nonce + 1, text).unwrap());
            nonce += 2;
        }
        let drip = Drip {
            data: response,
            position: 0,
            written: 0,
        };
        let mut tunnel = connect(Box::new(drip), "aes-128-gcm", "pw", "a.test", 80).unwrap();
        let mut out = [0u8; 10];
        tunnel.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"helloworld");
    }
}
