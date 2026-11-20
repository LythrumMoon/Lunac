//! VMess（AEAD 头 + 标准分块体）客户端（重建 v1 `native_proxy/vmess.rs`）。
//!
//! 协议依据（2026-11-15 逐段核对上游）：
//!   · 官方协议文档 <https://www.v2fly.org/en_US/developer/protocols/vmess.html>
//!     （EAuID / ALength / AHeader、指令段字段与顺序、标准分块格式、服务端响应）；
//!   · v2ray-core `proxy/vmess/aead/authid.go` / `encrypt.go` / `kdf.go` / `consts.go`
//!     （KDF 嵌套 HMAC 的确切用法、`CreateAuthID`、`SealVMessAEADHeader`）；
//!   · v2ray-core `proxy/vmess/encoding/client.go` / `auth.go` 与 `common/crypto/auth.go`
//!     （指令段逐字段写序、分块读写、`GenerateChunkNonce` 的**大端**计数器）。
//!
//! 线格式：
//! ```text
//! EAuID[16] | ALength[18] | Nonce[8] | AHeader[len+16] | 标准分块体
//! 指令段明文：
//!   version[1]=1 | bodyIV[16] | bodyKey[16] | respV[1] | option[1]=0x0d |
//!   (padLen<<4 | sec)[1] | reserved=0 | cmd=0x01 | port[2 BE] |
//!   atyp(1=IPv4 / 2=域名 / 3=IPv6) + addr | pad[padLen] | fnv1a32[4 BE]
//! 标准分块（本实现声明 `Opt S|M|P`；节点开了 `Opt A` 时再带上 0x10）：
//!   掩码后 / AEAD 后的 len[2 BE 或 18 B]（含 GCM tag 与填充） | 密文[len−padding] | 明文填充[padding]
//!   结束标记 = 一个明文为空的块（len = 16 + padding）……
//! ```
//!
//! **本版口径（如实登记）**：
//!   · 头：`alterId = 0` ⇒ AEAD 头；`alterId ≠ 0` ⇒ **老式 MD5 头**
//!     （`HMAC-MD5(uuid, ts[8 BE])` + `AES-128-CFB(CmdKey, MD5(ts×4))`，2026-11-15 重建，
//!     逐行对照 v2fly v4.45.2 `proxy/vmess/encoding/client.go` 的 `isAEAD=false` 分支）；
//!   · `CmdKey = MD5(uuid ‖ "c48619fe-8f02-49e0-b9e9-edf763e17e21")`（`common/protocol/id.go`）；
//!   · 声明 `Opt S|M|P`（0x0d = 分块流 + Shake 掩码 + 全局填充）：与 v2ray 客户端默认一致。
//!     分块长度与填充长度都按顺序取自 `SHAKE128(bodyIV)` 的 XOF（v2fly `encoding/auth.go`
//!     的 `ShakeSizeParser`）—— **先**取 `next()%64` 当填充长度，**再**取 `next()` 异或长度字段；
//!     响应方向换成 `responseBodyIV` 播种。
//!   · `Opt A`（认证长度，0x10，**按节点配置开启**，对齐 mihomo `authenticated-length`）：
//!     长度字段自己也是 18 字节 AEAD 密文（`2 字节 (总长−16) + 16 字节 tag`），
//!     密钥 `KDF16(requestBodyKey, "auth_len")`、nonce 与体同样的 `bodyIV` 计数器；
//!     **长度与体各用一条独立计数器**（都从 0 起，v2fly `GenerateChunkNonce` 各建一份），
//!     响应方向仍用 `requestBodyKey` / `requestBodyIV`（v2fly `DecodeResponseBody` 就是这么写的）；
//!     填充仍来自 `SHAKE128` 那条 XOF（`Opt P` 与 `Opt A` 可共存 —— 上游先取 padding 再换 size parser）；
//!   · 体加密固定 `AES-128-GCM`（客户端的指令段自己声明，节点 JSON 里的 `scy` 不参与）；
//!   · **UDP（`cmd = 0x02`）**：与 Xray 的非 cone 路径同口径 —— 目标地址写在指令段里，
//!     体仍是标准分块的字节流，**一个分块 = 一个数据报**（服务端把每次
//!     `WriteMultiBuffer` 当一个包派发；空块是结束标记，不能拿来发空包）。
//!     一条连接只服务一个目标（见 [`connect_udp`]）。

use std::io::{self, Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::aes::cipher::BlockEncrypt;
use aes_gcm::aes::Aes128;
use aes_gcm::{Aes128Gcm, Nonce};
use sha2::{Digest, Sha256};
// `Update` / `read` 与 `sha2::Digest`、`std::io::Read` 同名：不把 trait 带进作用域，一律限定调用。
use sha3::Shake128;

use super::ss::md5;
use super::transport::{BoxStream, Stream};

/// 指令段固定字段。
pub const VERSION: u8 = 1;
pub const OPT_CHUNK_STREAM: u8 = 0x01;
/// 分块长度用 `SHAKE128(bodyIV)` 的字节流异或（v2fly `RequestOptionChunkMasking`）。
pub const OPT_CHUNK_MASKING: u8 = 0x04;
/// 每个分块后附 0..63 字节明文填充，长度同样取自那条 XOF（`RequestOptionGlobalPadding`）。
pub const OPT_GLOBAL_PADDING: u8 = 0x08;
/// 长度字段本身用 `auth_len` 子密钥做 AEAD（`RequestOptionAuthenticatedLength`）。
pub const OPT_AUTHENTICATED_LENGTH: u8 = 0x10;
/// 我们声明的选项：`Opt S|M|P`（与 v2ray 客户端默认一致）。
pub const OPT_REQUEST: u8 = OPT_CHUNK_STREAM | OPT_CHUNK_MASKING | OPT_GLOBAL_PADDING;
/// 开了 `Opt A` 时的选项：`Opt S|M|P|A`（填充保留，长度字段改走 AEAD）。
pub const OPT_REQUEST_AUTH_LENGTH: u8 = OPT_REQUEST | OPT_AUTHENTICATED_LENGTH;
pub const SECURITY_AES128_GCM: u8 = 0x03;
pub const CMD_TCP: u8 = 0x01;
pub const CMD_UDP: u8 = 0x02;

/// 标准分块的明文上限（协议文档：最大 2^14）。
const MAX_CHUNK: usize = 16 * 1024;

/// 响应头长度段的密文长度：2 字节明文 + 16 字节 GCM tag。
const RESP_LEN_SEALED: usize = 18;

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(&hasher.finalize());
    out
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let (ipad, opad) = hmac_pads(key);
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner);
    let mut out = [0u8; 32];
    out.copy_from_slice(&outer.finalize());
    out
}

fn hmac_pads(key: &[u8]) -> ([u8; 64], [u8; 64]) {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&sha256(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for index in 0..64 {
        ipad[index] ^= block[index];
        opad[index] ^= block[index];
    }
    (ipad, opad)
}

/// 第 `depth` 层：`H_d(m) = H_{d-1}((k_d⊕opad) ‖ H_{d-1}((k_d⊕ipad) ‖ m))`，`H_0 = HMAC("VMess AEAD KDF", ·)`。
///
/// 这是 v2ray `aead.KDF` 的真实语义 —— 它把**上一层 HMAC 当作下一层的哈希函数**用
/// （`hmac.New(parent.Create, value)`），不是简单地把上一层的输出再 HMAC 一次。
fn kdf_layer(depth: usize, path: &[&[u8]], message: &[u8]) -> [u8; 32] {
    if depth == 0 {
        return hmac_sha256(b"VMess AEAD KDF", message);
    }
    let (ipad, opad) = hmac_pads(path[depth - 1]);
    let mut inner_message = Vec::with_capacity(64 + message.len());
    inner_message.extend_from_slice(&ipad);
    inner_message.extend_from_slice(message);
    let inner = kdf_layer(depth - 1, path, &inner_message);
    let mut outer_message = Vec::with_capacity(64 + 32);
    outer_message.extend_from_slice(&opad);
    outer_message.extend_from_slice(&inner);
    kdf_layer(depth - 1, path, &outer_message)
}

/// VMess 的 KDF：`path` 的每一项依次成为外层 HMAC 的 key（最后一项在最外层）。
pub fn kdf(key: &[u8], path: &[&[u8]]) -> [u8; 32] {
    kdf_layer(path.len(), path, key)
}

pub fn kdf16(key: &[u8], path: &[&[u8]]) -> [u8; 16] {
    let full = kdf(key, path);
    let mut out = [0u8; 16];
    out.copy_from_slice(&full[..16]);
    out
}

fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= *byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn fnv1a32(data: &[u8]) -> u32 {
    let mut hash = 0x811c_9dc5u32;
    for byte in data {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    if getrandom::getrandom(&mut out).is_err() {
        // 取不到随机数也要能跑：用时间兜底（安全性与随机性下降，但不 panic）。
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        out.copy_from_slice(&nanos.to_le_bytes()[..N.min(8)]);
    }
    out
}

/// 任意长度的随机字节（分块填充用；`random_bytes` 只支持定长）。
fn random_bytes_vec(count: usize) -> Vec<u8> {
    let mut out = vec![0u8; count];
    if getrandom::getrandom(&mut out).is_err() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1)
            .to_le_bytes();
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = nanos[i % nanos.len()];
        }
    }
    out
}

fn aes128_ecb_encrypt(key: &[u8; 16], block: &[u8; 16]) -> [u8; 16] {
    let cipher = Aes128::new_from_slice(key).expect("AES-128 key 长度固定 16 字节");
    let mut buf = aes_gcm::aes::cipher::generic_array::GenericArray::clone_from_slice(block);
    cipher.encrypt_block(&mut buf);
    let mut out = [0u8; 16];
    out.copy_from_slice(&buf);
    out
}

fn gcm_seal(key: &[u8; 16], nonce: &[u8; 12], plain: &[u8], aad: &[u8]) -> Vec<u8> {
    let cipher = Aes128Gcm::new_from_slice(key).expect("AES-128-GCM key 长度固定 16 字节");
    cipher
        .encrypt(Nonce::from_slice(nonce), Payload { msg: plain, aad })
        .unwrap_or_default()
}

fn gcm_open(key: &[u8; 16], nonce: &[u8; 12], sealed: &[u8], aad: &[u8]) -> Option<Vec<u8>> {
    let cipher = Aes128Gcm::new_from_slice(key).expect("AES-128-GCM key 长度固定 16 字节");
    cipher
        .decrypt(Nonce::from_slice(nonce), Payload { msg: sealed, aad })
        .ok()
}

/// `CmdKey = MD5(uuid[16] ‖ "c48619fe-8f02-49e0-b9e9-edf763e17e21")`。
///
/// 依据：Xray / v2fly `common/protocol/id.go` 的 `NewID`。**不是裸 `MD5(uuid)`** ——
/// 少了那串 magic，服务端解不出 AuthID、找不到用户，连接会被直接关掉
/// （2026-11-15 对照上游源码修正；此前真机 VMess「对端直接关闭」就是这条）。
pub fn cmd_key(uuid: &[u8; 16]) -> [u8; 16] {
    const MAGIC: &[u8] = b"c48619fe-8f02-49e0-b9e9-edf763e17e21";
    let mut input = Vec::with_capacity(16 + MAGIC.len());
    input.extend_from_slice(uuid);
    input.extend_from_slice(MAGIC);
    md5(&input)
}

/// 老式（MD5 头）VMess 的 AuthID：`HMAC-MD5(uuid, unix[8 BE])`。
///
/// 依据 v2fly `proxy/vmess/encoding/client.go::EncodeRequestHeader`（`DefaultIDHash`
/// 写入 uint64 时间戳后取 `Sum`）与 `common/protocol/id.go::DefaultIDHash`
/// （`hmac.New(md5.New, key)`，key = UUID 的 16 字节）。
pub fn legacy_auth_id(uuid: &[u8; 16], unix: i64) -> [u8; 16] {
    hmac_md5(uuid, &unix.to_be_bytes())
}

/// `HMAC-MD5`（RFC 2104；块长 64）。
fn hmac_md5(key: &[u8], message: &[u8]) -> [u8; 16] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..16].copy_from_slice(&md5(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for index in 0..64 {
        ipad[index] ^= block[index];
        opad[index] ^= block[index];
    }
    let mut inner = Vec::with_capacity(64 + message.len());
    inner.extend_from_slice(&ipad);
    inner.extend_from_slice(message);
    let mut outer = Vec::with_capacity(64 + 16);
    outer.extend_from_slice(&opad);
    outer.extend_from_slice(&md5(&inner));
    md5(&outer)
}

/// 老式请求头（与响应头）的 IV：`MD5(时间戳[8 BE] ×4)`。
fn legacy_timestamp_iv(unix: i64) -> [u8; 16] {
    let stamp = unix.to_be_bytes();
    let mut input = Vec::with_capacity(32);
    for _ in 0..4 {
        input.extend_from_slice(&stamp);
    }
    md5(&input)
}

/// AES-128-CFB（CFB128，按字节推进，支持任意长度输入）。
///
/// Go 的 `cipher.NewCFBEncrypter/Decrypter` 就是这个模式：`out = in ⊕ E(上一段密文)`。
/// 老式 VMess 的请求头与响应头（4 字节）都用它；体仍是 AES-128-GCM 分块，互不影响。
struct Cfb128 {
    cipher: Aes128,
    /// 当前块的 keystream（`E(上一段密文)`）。
    keystream: [u8; 16],
    /// 当前块已用掉的字节数。
    used: usize,
    /// 当前块已产出的**密文**（块满后作为下一次反馈）。
    feedback: [u8; 16],
    decrypt: bool,
}

impl Cfb128 {
    fn new(key: &[u8; 16], iv: &[u8; 16], decrypt: bool) -> Self {
        Self {
            cipher: Aes128::new_from_slice(key).expect("AES-128 key 长度固定 16 字节"),
            keystream: aes128_ecb_encrypt(key, iv),
            used: 0,
            feedback: [0u8; 16],
            decrypt,
        }
    }

    fn apply(&mut self, data: &mut [u8]) {
        for byte in data.iter_mut() {
            if self.used == 16 {
                self.keystream = aes128_ecb_encrypt_from(&self.cipher, &self.feedback);
                self.used = 0;
                self.feedback = [0u8; 16];
            }
            let input = *byte;
            let output = input ^ self.keystream[self.used];
            // 反馈永远是密文：加密时是输出，解密时是输入。
            self.feedback[self.used] = if self.decrypt { input } else { output };
            *byte = output;
            self.used += 1;
        }
    }
}

fn aes128_ecb_encrypt_from(cipher: &Aes128, block: &[u8; 16]) -> [u8; 16] {
    let mut buf = aes_gcm::aes::cipher::generic_array::GenericArray::clone_from_slice(block);
    cipher.encrypt_block(&mut buf);
    let mut out = [0u8; 16];
    out.copy_from_slice(&buf);
    out
}

/// 老式请求头：`AuthID[16] ‖ AES-128-CFB(CmdKey, MD5(ts×4), 指令段)`。
pub fn legacy_seal_header(uuid: &[u8; 16], unix: i64, header: &[u8]) -> Vec<u8> {
    let mut sealed = header.to_vec();
    Cfb128::new(&cmd_key(uuid), &legacy_timestamp_iv(unix), false).apply(&mut sealed);
    let mut out = Vec::with_capacity(16 + sealed.len());
    out.extend_from_slice(&legacy_auth_id(uuid, unix));
    out.extend_from_slice(&sealed);
    out
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// EAuID：`AES-128-ECB(KDF16(CmdKey, "AES Auth ID Encryption"), unix[8 BE] | rand[4] | crc32[4])`。
pub fn auth_id(key: &[u8; 16], unix: i64, rand: [u8; 4]) -> [u8; 16] {
    let mut plain = [0u8; 16];
    plain[..8].copy_from_slice(&unix.to_be_bytes());
    plain[8..12].copy_from_slice(&rand);
    let crc = crc32_ieee(&plain[..12]);
    plain[12..16].copy_from_slice(&crc.to_be_bytes());
    aes128_ecb_encrypt(&kdf16(key, &[b"AES Auth ID Encryption"]), &plain)
}

/// 指令段（未加校验和的版本由调用方拼好，这里只负责整段 AEAD 封装）。
pub fn seal_header(key: &[u8; 16], header: &[u8]) -> Vec<u8> {
    let unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    seal_header_at(key, header, unix, random_bytes::<4>(), random_bytes::<8>())
}

fn seal_header_at(
    key: &[u8; 16],
    header: &[u8],
    unix: i64,
    rand: [u8; 4],
    nonce: [u8; 8],
) -> Vec<u8> {
    let auth = auth_id(key, unix, rand);
    let len_key = kdf16(key, &[b"VMess Header AEAD Key_Length", &auth, &nonce]);
    let len_nonce_full = kdf(key, &[b"VMess Header AEAD Nonce_Length", &auth, &nonce]);
    let mut len_nonce = [0u8; 12];
    len_nonce.copy_from_slice(&len_nonce_full[..12]);
    let sealed_len = gcm_seal(
        &len_key,
        &len_nonce,
        &(header.len().min(u16::MAX as usize) as u16).to_be_bytes(),
        &auth,
    );

    let body_key = kdf16(key, &[b"VMess Header AEAD Key", &auth, &nonce]);
    let body_nonce_full = kdf(key, &[b"VMess Header AEAD Nonce", &auth, &nonce]);
    let mut body_nonce = [0u8; 12];
    body_nonce.copy_from_slice(&body_nonce_full[..12]);
    let sealed_header = gcm_seal(&body_key, &body_nonce, header, &auth);

    let mut out = Vec::with_capacity(16 + sealed_len.len() + 8 + sealed_header.len());
    out.extend_from_slice(&auth);
    out.extend_from_slice(&sealed_len);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed_header);
    out
}

/// 指令段明文（不含「随机 Padding」与校验和：这两段由 [`instruction_section`] 补）。
fn address_bytes(host: &str) -> Vec<u8> {
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

/// 拼 TCP 的指令段（仅测试用；等价 `instruction_section_cmd(..., CMD_TCP, ...)`）。
#[cfg(test)]
pub fn instruction_section(
    body_iv: &[u8; 16],
    body_key: &[u8; 16],
    response_v: u8,
    pad_len: usize,
    host: &str,
    port: u16,
    pad: &[u8],
) -> Vec<u8> {
    instruction_section_cmd(
        body_iv,
        body_key,
        response_v,
        pad_len,
        CMD_TCP,
        OPT_REQUEST,
        host,
        port,
        pad,
    )
}

/// 拼完整指令段（含随机 padding 与 FNV1a 校验和），命令由调用方给（`0x01=TCP` / `0x02=UDP`）。
///
/// 字段顺序对齐官方文档：`Port` 在 `Address Type` **之前**（与 VLESS 相反）。
pub fn instruction_section_cmd(
    body_iv: &[u8; 16],
    body_key: &[u8; 16],
    response_v: u8,
    pad_len: usize,
    command: u8,
    option: u8,
    host: &str,
    port: u16,
    pad: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 + pad_len);
    out.push(VERSION);
    out.extend_from_slice(body_iv);
    out.extend_from_slice(body_key);
    out.push(response_v);
    out.push(option);
    out.push(((pad_len as u8 & 0x0f) << 4) | SECURITY_AES128_GCM);
    out.push(0); // reserved
    out.push(command);
    out.extend_from_slice(&port.to_be_bytes());
    out.extend_from_slice(&address_bytes(host));
    out.extend_from_slice(&pad[..pad_len.min(pad.len())]);
    let checksum = fnv1a32(&out);
    out.extend_from_slice(&checksum.to_be_bytes());
    out
}

/// 体加密的分块 nonce：`copy(bodyIV)` 的前 2 字节写成**大端**计数器（官方文档同款）。
pub fn chunk_nonce(body_iv: &[u8; 16], counter: u16) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&body_iv[..12]);
    nonce[..2].copy_from_slice(&counter.to_be_bytes());
    nonce
}

/// `SHAKE128(iv)` 的 XOF 游标：分块长度掩码与填充长度都从这里按顺序取字节。
///
/// 口径对齐 v2fly `proxy/vmess/encoding/auth.go` 的 `ShakeSizeParser`：
///   · `next()` 读 2 字节大端 u16；
///   · `NextPaddingLen()` = `next() % 64`（0..=63）；
///   · 同一块里**先**取填充长度、**后**取长度掩码（写侧与读侧都是这个顺序）。
struct ShakeMask {
    xof: sha3::Shake128Reader,
}

impl ShakeMask {
    fn new(iv: &[u8; 16]) -> Self {
        let mut hasher = Shake128::default();
        sha3::digest::Update::update(&mut hasher, iv);
        Self {
            xof: sha3::digest::ExtendableOutput::finalize_xof(hasher),
        }
    }

    fn next_u16(&mut self) -> u16 {
        let mut buf = [0u8; 2];
        sha3::digest::XofReader::read(&mut self.xof, &mut buf);
        u16::from_be_bytes(buf)
    }

    fn next_padding(&mut self) -> usize {
        (self.next_u16() % 64) as usize
    }
}

/// `Opt A` 的长度字段封装器：`len_auth = KDF16(bodyKey, "auth_len")`，nonce = `iv` 上的
/// 独立大端计数器（与体加密的计数器**各走一条**，都从 0 起）。
///
/// 口径对齐 v2fly `crypto.AEADChunkSizeParser`：
///   · `Encode` 写 `size − 16` 的 2 字节明文再 Seal ⇒ 线上 18 字节；
///   · `Decode` Open 后把 16 字节 tag 加回去 ⇒ 得到「密文 + tag + 填充」总长。
struct LengthAuth {
    key: [u8; 16],
    iv: [u8; 16],
    counter: u16,
}

impl LengthAuth {
    fn new(key: [u8; 16], iv: [u8; 16]) -> Self {
        Self {
            key,
            iv,
            counter: 0,
        }
    }

    /// 把「本块总长（密文 + tag + 填充）」封装成 18 字节。
    fn encode(&mut self, total: usize) -> [u8; 18] {
        let plain = (total.saturating_sub(16) as u16).to_be_bytes();
        let nonce = chunk_nonce(&self.iv, self.counter);
        self.counter = self.counter.wrapping_add(1);
        let sealed = gcm_seal(&self.key, &nonce, &plain, &[]);
        let mut out = [0u8; 18];
        out.copy_from_slice(&sealed[..18]);
        out
    }

    /// 解 18 字节长度字段；返回值已加回 16 字节 tag（与上游 `Decode` 一致）。
    fn decode(&mut self, sealed: &[u8]) -> Option<usize> {
        let nonce = chunk_nonce(&self.iv, self.counter);
        self.counter = self.counter.wrapping_add(1);
        let plain = gcm_open(&self.key, &nonce, sealed, &[])?;
        if plain.len() < 2 {
            return None;
        }
        Some(u16::from_be_bytes([plain[0], plain[1]]) as usize + 16)
    }
}

/// 标准分块的体加密器（AES-128-GCM，明文上限 16 KiB）。
struct BodyWriter {
    key: [u8; 16],
    iv: [u8; 16],
    counter: u16,
    /// 长度掩码 + 填充长度的随机源（`Opt M|P` 时必有；两条流共用同一个游标）。
    /// 开 `Opt A` 后这条 XOF 只剩填充用（长度字段改走 AEAD）。
    mask: Option<ShakeMask>,
    /// `Opt A`：长度字段的 AEAD 封装器（独立计数器）。
    length_auth: Option<LengthAuth>,
}

impl BodyWriter {
    fn seal(&mut self, plain: &[u8]) -> Vec<u8> {
        let nonce = chunk_nonce(&self.iv, self.counter);
        self.counter = self.counter.wrapping_add(1);
        let sealed = gcm_seal(&self.key, &nonce, plain, &[]);
        // 上游顺序：先取填充长度，再编码长度字段（两条 XOF / 计数器互不影响）。
        let padding_len = self
            .mask
            .as_mut()
            .map(|mask| mask.next_padding())
            .unwrap_or(0);
        let total = sealed.len() + padding_len;
        let mut out = Vec::with_capacity(18 + total);
        match self.length_auth.as_mut() {
            Some(length_auth) => out.extend_from_slice(&length_auth.encode(total)),
            None => {
                let size_field = match self.mask.as_mut() {
                    Some(mask) => mask.next_u16() ^ total as u16,
                    None => total as u16,
                };
                out.extend_from_slice(&size_field.to_be_bytes());
            }
        }
        out.extend_from_slice(&sealed);
        if padding_len > 0 {
            // 填充是**明文**，服务端按自己的 XOF 算出长度后直接丢弃。
            out.extend_from_slice(&random_bytes_vec(padding_len));
        }
        out
    }
}

/// 单条 VMess 连接（一个 TCP 流）。
pub struct VmessTunnel {
    stream: BoxStream,
    writer: BodyWriter,
    response_key: [u8; 16],
    response_iv: [u8; 16],
    response_v: u8,
    body_counter: u16,
    /// 响应分块的长度掩码 / 填充长度来源（`SHAKE128(responseBodyIV)`）。
    read_mask: Option<ShakeMask>,
    /// `Opt A` 的响应侧长度字段：密钥来自**请求**的 `bodyKey`（`auth_len` 子密钥），
    /// IV 也是**请求**的 `bodyIV`（v2fly `DecodeResponseBody` 同款）。
    read_len_auth: Option<LengthAuth>,
    /// 半包时缓存「已从 XOF 取过、但数据还没到齐」的块：`(密文+填充总长, 填充长度)`。
    /// 不缓存会重复推进 XOF 游标，之后每个块都解错。
    pending_len: Option<(usize, usize)>,
    /// 原始密文（响应头 + 分块）的增量缓冲：跨读超时不丢半包。
    read_buffer: Vec<u8>,
    /// 已解出的明文块（UDP 模式下**一个块 = 一个数据报**，不能被合并）。
    pending_chunks: std::collections::VecDeque<Vec<u8>>,
    payload: Vec<u8>,
    header_done: bool,
    closed: bool,
    /// 老式（MD5 头，`alterId≠0`）：响应头是 CFB 明文流，不是 AEAD 两段。
    legacy: bool,
}

impl VmessTunnel {
    /// 消费响应头（`AEAD Resp Header Len Key/IV` + `AEAD Resp Header Key/IV`）。
    ///
    /// 响应头的 AEAD **不推进**体加密的计数器：体从 counter = 0 重新开始。
    fn parse_response_header(&mut self) -> io::Result<bool> {
        if self.legacy {
            return self.parse_legacy_response_header();
        }
        if self.read_buffer.len() < RESP_LEN_SEALED {
            return Ok(false);
        }
        let len_key = kdf16(&self.response_key, &[b"AEAD Resp Header Len Key"]);
        let len_iv_full = kdf(&self.response_iv, &[b"AEAD Resp Header Len IV"]);
        let mut len_iv = [0u8; 12];
        len_iv.copy_from_slice(&len_iv_full[..12]);
        let Some(len_plain) =
            gcm_open(&len_key, &len_iv, &self.read_buffer[..RESP_LEN_SEALED], &[])
        else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "VMess 响应头长度解密失败（节点口令 / 协议不匹配）",
            ));
        };
        if len_plain.len() < 2 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "VMess 响应头长度异常",
            ));
        }
        let header_len = u16::from_be_bytes([len_plain[0], len_plain[1]]) as usize;
        if self.read_buffer.len() < RESP_LEN_SEALED + header_len + 16 {
            return Ok(false);
        }
        let body_key = kdf16(&self.response_key, &[b"AEAD Resp Header Key"]);
        let body_iv_full = kdf(&self.response_iv, &[b"AEAD Resp Header IV"]);
        let mut body_iv = [0u8; 12];
        body_iv.copy_from_slice(&body_iv_full[..12]);
        let sealed = self.read_buffer[RESP_LEN_SEALED..RESP_LEN_SEALED + header_len + 16].to_vec();
        let Some(plain) = gcm_open(&body_key, &body_iv, &sealed, &[]) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "VMess 响应头解密失败（节点口令 / 协议不匹配）",
            ));
        };
        if plain.len() < 4 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "VMess 响应头过短",
            ));
        }
        if plain[0] != self.response_v {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "VMess 响应头校验失败（期望 V={}，实际 {}）",
                    self.response_v, plain[0]
                ),
            ));
        }
        self.read_buffer.drain(..RESP_LEN_SEALED + header_len + 16);
        self.header_done = true;
        Ok(true)
    }

    /// 老式响应头：`AES-128-CFB(MD5(bodyKey), MD5(bodyIV))` 解出的前 4 字节
    /// = `[V][option][cmdID][dataLen]`（v2fly `DecodeResponseHeader` 的非 AEAD 分支）。
    ///
    /// 体仍是 AES-128-GCM 分块（我们声明 `security=3`），所以这 4 字节之后
    /// 不再需要 CFB 状态 —— 直接切回原始流读分块。
    fn parse_legacy_response_header(&mut self) -> io::Result<bool> {
        const LEGACY_HEADER: usize = 4;
        if self.read_buffer.len() < LEGACY_HEADER {
            return Ok(false);
        }
        let mut head = self.read_buffer[..LEGACY_HEADER].to_vec();
        Cfb128::new(&self.response_key, &self.response_iv, true).apply(&mut head);
        if head[0] != self.response_v {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "VMess 响应头校验失败（期望 V={}，实际 {}）",
                    self.response_v, head[0]
                ),
            ));
        }
        if head[2] != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("VMess 响应命令 0x{:02x} 本版不支持", head[2]),
            ));
        }
        self.read_buffer.drain(..LEGACY_HEADER);
        self.header_done = true;
        Ok(true)
    }

    /// 解析一个标准分块；`Ok(false)` = 还差字节。
    fn parse_chunk(&mut self) -> io::Result<bool> {
        // `Opt A` 的长度字段是 18 字节 AEAD，否则是 2 字节（可能被 XOF 掩码）。
        let size_bytes = if self.read_len_auth.is_some() { 18 } else { 2 };
        let (len, padding) = match self.pending_len {
            Some(cached) => cached,
            None => {
                if self.read_buffer.len() < size_bytes {
                    return Ok(false);
                }
                let padding = self
                    .read_mask
                    .as_mut()
                    .map(|mask| mask.next_padding())
                    .unwrap_or(0);
                let len = match self.read_len_auth.as_mut() {
                    Some(length_auth) => {
                        match length_auth.decode(&self.read_buffer[..size_bytes]) {
                            Some(len) => len,
                            None => {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "VMess 分块长度解密失败",
                                ))
                            }
                        }
                    }
                    None => {
                        let size_field =
                            u16::from_be_bytes([self.read_buffer[0], self.read_buffer[1]]);
                        match self.read_mask.as_mut() {
                            Some(mask) => (mask.next_u16() ^ size_field) as usize,
                            None => size_field as usize,
                        }
                    }
                };
                if len < 16 + padding {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "VMess 分块长度异常",
                    ));
                }
                self.pending_len = Some((len, padding));
                (len, padding)
            }
        };
        if self.read_buffer.len() < size_bytes + len {
            return Ok(false);
        }
        self.pending_len = None;
        // 填充是明文且总在块尾，去掉之后再解 GCM。
        let sealed = self.read_buffer[size_bytes..size_bytes + len - padding].to_vec();
        self.read_buffer.drain(..size_bytes + len);
        let nonce = chunk_nonce(&self.response_iv, self.body_counter);
        self.body_counter = self.body_counter.wrapping_add(1);
        let Some(plain) = gcm_open(&self.response_key, &nonce, &sealed, &[]) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "VMess 分块解密失败",
            ));
        };
        if plain.is_empty() {
            // 结束标记：`len == 16 + padding`（明文为空的块）。
            self.closed = true;
        } else {
            self.pending_chunks.push_back(plain);
        }
        Ok(true)
    }

    /// 发一个数据报（UDP）：**单独一个标准分块**，服务端按一个包派发。
    pub fn send_datagram(&mut self, data: &[u8]) -> io::Result<()> {
        if data.len() > MAX_CHUNK {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "VMess UDP 单包超过 16 KiB",
            ));
        }
        if data.is_empty() {
            // 空块是结束标记，绝不能当空数据报发出去。
            return Ok(());
        }
        let frame = self.writer.seal(data);
        self.stream.write_all(&frame)
    }

    /// 收一个数据报；`Ok(None)` = 暂时没有（超时或半包）。
    pub fn recv_datagram(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop {
            if let Some(packet) = self.pending_chunks.pop_front() {
                return Ok(Some(packet));
            }
            if self.closed {
                return Ok(None);
            }
            if !self.header_done {
                if self.parse_response_header()? {
                    continue;
                }
            } else if self.parse_chunk()? {
                continue;
            }
            let mut chunk = [0u8; 16 * 1024];
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    self.closed = true;
                    return Ok(None);
                }
                Ok(read) => self.read_buffer.extend_from_slice(&chunk[..read]),
                Err(ref e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::TimedOut =>
                {
                    return Ok(None)
                }
                Err(e) => return Err(e),
            }
        }
    }
}

impl Read for VmessTunnel {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.payload.is_empty() {
                if let Some(next) = self.pending_chunks.pop_front() {
                    self.payload = next;
                    continue;
                }
            }
            if !self.payload.is_empty() {
                let count = out.len().min(self.payload.len());
                out[..count].copy_from_slice(&self.payload[..count]);
                self.payload.drain(..count);
                return Ok(count);
            }
            if self.closed {
                return Ok(0);
            }
            if !self.header_done {
                if self.parse_response_header()? {
                    continue;
                }
            } else if self.parse_chunk()? {
                continue;
            }
            let mut chunk = [0u8; 16 * 1024];
            let read = self.stream.read(&mut chunk)?;
            if read == 0 {
                self.closed = true;
                return Ok(0);
            }
            self.read_buffer.extend_from_slice(&chunk[..read]);
        }
    }
}

impl Write for VmessTunnel {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut written = 0usize;
        while written < data.len() {
            let end = (written + MAX_CHUNK).min(data.len());
            let frame = self.writer.seal(&data[written..end]);
            self.stream.write_all(&frame)?;
            written = end;
        }
        if data.is_empty() {
            return Ok(0);
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

impl Stream for VmessTunnel {
    fn set_read_timeout(&self, timeout: Option<std::time::Duration>) -> io::Result<()> {
        self.stream.set_read_timeout(timeout)
    }
}

/// 建好传输层后调用：发 AEAD 头，返回可直接透传的流（响应头在首次读时消费）。
pub fn connect(
    stream: BoxStream,
    uuid: &str,
    alter_id: u32,
    authenticated_length: bool,
    host: &str,
    port: u16,
) -> Result<BoxStream, String> {
    Ok(Box::new(open(
        stream,
        uuid,
        alter_id,
        authenticated_length,
        host,
        port,
        CMD_TCP,
    )?))
}

/// UDP（`cmd=0x02`）会话：目标固定在指令段里，收发都以「一个分块 = 一个数据报」为单位。
pub fn connect_udp(
    stream: BoxStream,
    uuid: &str,
    alter_id: u32,
    authenticated_length: bool,
    host: &str,
    port: u16,
) -> Result<VmessTunnel, String> {
    open(
        stream,
        uuid,
        alter_id,
        authenticated_length,
        host,
        port,
        CMD_UDP,
    )
}

fn open(
    mut stream: BoxStream,
    uuid: &str,
    alter_id: u32,
    authenticated_length: bool,
    host: &str,
    port: u16,
    command: u8,
) -> Result<VmessTunnel, String> {
    let id = super::vless::parse_uuid(uuid)?;
    // `alterId = 0` ⇒ AEAD 头；`alterId ≠ 0` ⇒ 老式 MD5 头（v2fly `isAEAD` 的判定）。
    let legacy = alter_id != 0;
    // `Opt A` 只对 AEAD 头 + AES-128-GCM 生效（老式头 / 其它安全档上游也不认这一位）。
    let auth_length = authenticated_length && !legacy;

    let body_key: [u8; 16] = random_bytes();
    let body_iv: [u8; 16] = random_bytes();
    let response_v: [u8; 1] = random_bytes();
    let pad_len = (random_bytes::<1>()[0] & 0x0f) as usize;
    let pad: [u8; 16] = random_bytes();

    let header = instruction_section_cmd(
        &body_iv,
        &body_key,
        response_v[0],
        pad_len,
        command,
        if auth_length {
            OPT_REQUEST_AUTH_LENGTH
        } else {
            OPT_REQUEST
        },
        host,
        port,
        &pad,
    );
    let request_bytes = if legacy {
        legacy_seal_header(&id, now_unix(), &header)
    } else {
        seal_header(&cmd_key(&id), &header)
    };
    stream
        .write_all(&request_bytes)
        .map_err(|e| format!("VMess 请求头发送失败：{e}"))?;

    // 响应侧的 Key/IV：AEAD = SHA256(请求侧) 的前 16 字节；老式 = MD5(请求侧)。
    let (response_key, response_iv) = if legacy {
        (md5(&body_key), md5(&body_iv))
    } else {
        let mut key = [0u8; 16];
        key.copy_from_slice(&sha256(&body_key)[..16]);
        let mut iv = [0u8; 16];
        iv.copy_from_slice(&sha256(&body_iv)[..16]);
        (key, iv)
    };

    Ok(VmessTunnel {
        stream,
        writer: BodyWriter {
            key: body_key,
            iv: body_iv,
            counter: 0,
            // 请求方向：`SHAKE128(bodyIV)`；服务端按同一份 XOF 解长度与填充
            // （开 `Opt A` 后只剩填充用）。
            mask: Some(ShakeMask::new(&body_iv)),
            length_auth: auth_length
                .then(|| LengthAuth::new(kdf16(&body_key, &[b"auth_len"]), body_iv)),
        },
        response_key,
        response_iv,
        response_v: response_v[0],
        body_counter: 0,
        read_mask: Some(ShakeMask::new(&response_iv)),
        // 响应侧长度字段用**请求**的 `auth_len` 子密钥与 `bodyIV`（v2fly 同款）。
        read_len_auth: auth_length
            .then(|| LengthAuth::new(kdf16(&body_key, &[b"auth_len"]), body_iv)),
        pending_len: None,
        read_buffer: Vec::new(),
        pending_chunks: std::collections::VecDeque::new(),
        payload: Vec::new(),
        header_done: false,
        closed: false,
        legacy,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    /// 测试桩：按我们声明的 `Opt M|P` 读一个请求分块并解出明文。
    fn read_masked_chunk(
        stream: &mut TcpStream,
        mask: &mut ShakeMask,
        key: &[u8; 16],
        iv: &[u8; 16],
        counter: u16,
    ) -> Vec<u8> {
        let mut len_bytes = [0u8; 2];
        stream.read_exact(&mut len_bytes).unwrap();
        let padding = mask.next_padding();
        let len = (mask.next_u16() ^ u16::from_be_bytes(len_bytes)) as usize;
        let mut buf = vec![0u8; len];
        stream.read_exact(&mut buf).unwrap();
        gcm_open(key, &chunk_nonce(iv, counter), &buf[..len - padding], &[]).unwrap()
    }

    /// 测试桩：按服务端口径封一个响应分块（`SHAKE128(responseBodyIV)`、先填充后掩码）。
    fn sealed_chunk(
        key: &[u8; 16],
        iv: &[u8; 16],
        counter: u16,
        mask: &mut ShakeMask,
        plain: &[u8],
    ) -> Vec<u8> {
        let sealed = gcm_seal(key, &chunk_nonce(iv, counter), plain, &[]);
        let padding = mask.next_padding();
        let total = sealed.len() + padding;
        let mut out = Vec::with_capacity(2 + total);
        out.extend_from_slice(&(mask.next_u16() ^ total as u16).to_be_bytes());
        out.extend_from_slice(&sealed);
        let mut pad = vec![0u8; padding];
        for byte in pad.iter_mut() {
            *byte = 0xA5;
        }
        out.extend_from_slice(&pad);
        out
    }

    #[test]
    fn kdf_matches_upstream_vector() {
        // v2ray-core `proxy/vmess/aead/kdf_test.go` 的标准向量。
        let value = kdf(
            b"Demo Key for KDF Value Test",
            &[
                b"Demo Path for KDF Value Test",
                b"Demo Path for KDF Value Test2",
                b"Demo Path for KDF Value Test3",
            ],
        );
        let hex: String = value.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "53e9d7e1bd7bd25022b71ead07d8a596efc8a845c7888652fd684b4903dc8892"
        );
    }

    #[test]
    fn auth_id_carries_timestamp_and_crc() {
        let key = [7u8; 16];
        let unix = 1_700_000_000i64;
        let auth = auth_id(&key, unix, [1, 2, 3, 4]);
        // 用同一把 KDF 出来的 AES-128 密钥解回去（等价于服务端的 `AuthIDDecoder`）。
        let cipher = Aes128::new_from_slice(&kdf16(&key, &[b"AES Auth ID Encryption"])).unwrap();
        let mut block = aes_gcm::aes::cipher::generic_array::GenericArray::clone_from_slice(&auth);
        aes_gcm::aes::cipher::BlockDecrypt::decrypt_block(&cipher, &mut block);
        assert_eq!(i64::from_be_bytes(block[..8].try_into().unwrap()), unix);
        assert_eq!(&block[8..12], &[1, 2, 3, 4]);
        assert_eq!(
            u32::from_be_bytes(block[12..16].try_into().unwrap()),
            crc32_ieee(&block[..12])
        );
    }

    #[test]
    fn instruction_section_layout_matches_doc() {
        let iv = [0x11u8; 16];
        let key = [0x22u8; 16];
        let header = instruction_section(&iv, &key, 0x33, 4, "example.com", 443, &[0u8; 16]);
        assert_eq!(header[0], VERSION);
        assert_eq!(&header[1..17], &iv);
        assert_eq!(&header[17..33], &key);
        assert_eq!(header[33], 0x33);
        assert_eq!(header[34], OPT_REQUEST);
        assert_eq!(header[35], (4 << 4) | SECURITY_AES128_GCM);
        assert_eq!(header[36], 0);
        assert_eq!(header[37], CMD_TCP);
        // Port 在 Address Type 之前（与 VLESS 相反）。
        assert_eq!(&header[38..40], &443u16.to_be_bytes());
        assert_eq!(header[40], 0x02); // domain
        assert_eq!(header[41], 11);
        assert_eq!(&header[42..53], b"example.com");
        assert_eq!(&header[53..57], &[0u8; 4]); // 4 字节 padding
        let checksum = u32::from_be_bytes(header[57..61].try_into().unwrap());
        assert_eq!(checksum, fnv1a32(&header[..57]));
        // 地址类型：IPv4=1 / IPv6=3（与 VLESS 一致，不是 SOCKS5 的 3/4）。
        assert_eq!(
            instruction_section(&iv, &key, 0, 0, "1.2.3.4", 80, &[])[40],
            0x01
        );
        assert_eq!(
            instruction_section(&iv, &key, 0, 0, "::1", 80, &[])[40],
            0x03
        );
    }

    /// 掩码 / 填充的字节流必须与独立实现（Node/OpenSSL）算出的 `SHAKE128(iv)` 一致。
    #[test]
    fn chunk_masking_and_padding_follow_the_shake128_stream() {
        // `SHAKE128(0x00 × 16)` 的前 8 字节（Node `crypto.createHash('shake128')` 算出）。
        let iv = [0u8; 16];
        let mut mask = ShakeMask::new(&iv);
        assert_eq!(mask.next_u16(), 0x8f8e);
        assert_eq!(mask.next_u16(), 0x4f61);
        assert_eq!(mask.next_u16(), 0x2e61);
        assert_eq!(mask.next_u16(), 0xffb9);
    }

    /// 写侧：**先**取填充长度（`next()%64`）、**后**取长度掩码；填充是明文追加在块尾。
    #[test]
    fn masked_writer_hides_the_length_and_appends_cleartext_padding() {
        let iv = [0u8; 16];
        let mut writer = BodyWriter {
            key: [1u8; 16],
            iv,
            counter: 0,
            mask: Some(ShakeMask::new(&iv)),
            length_auth: None,
        };
        let frame = writer.seal(b"hello");
        let pad = (0x8f8eu16 % 64) as usize;
        assert_eq!(pad, 14, "这条 IV 上 SHAKE 的第一个 u16 取模 64");
        let expected = 0x4f61u16 ^ (5 + 16 + pad) as u16;
        assert_eq!(u16::from_be_bytes([frame[0], frame[1]]), expected);
        assert_eq!(frame.len(), 2 + 21 + pad, "2 字节长度 + 密文 + 明文填充");
    }

    /// 半包：只有 2 字节长度时不能推进 XOF，否则之后每个块都解错。
    #[test]
    fn partial_chunk_does_not_advance_the_shake_stream_twice() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (_held, _) = listener.accept().unwrap();
            thread::sleep(std::time::Duration::from_millis(200));
        });
        let tcp = TcpStream::connect(addr).unwrap();
        let key = [0x11u8; 16];
        let iv = [0x22u8; 16];
        let mut mask = ShakeMask::new(&iv);
        let frame = sealed_chunk(&key, &iv, 0, &mut mask, b"HELLO");
        let mut tunnel = VmessTunnel {
            stream: Box::new(tcp),
            writer: BodyWriter {
                key,
                iv,
                counter: 0,
                mask: None,
                length_auth: None,
            },
            response_key: key,
            response_iv: iv,
            response_v: 0,
            body_counter: 0,
            read_mask: Some(ShakeMask::new(&iv)),
            read_len_auth: None,
            pending_len: None,
            read_buffer: frame[..2].to_vec(),
            pending_chunks: std::collections::VecDeque::new(),
            payload: Vec::new(),
            header_done: true,
            closed: false,
            legacy: false,
        };
        assert!(!tunnel.parse_chunk().unwrap(), "只有长度字段时还不成块");
        tunnel.read_buffer.extend_from_slice(&frame[2..]);
        assert!(tunnel.parse_chunk().unwrap());
        assert_eq!(tunnel.pending_chunks.pop_front().unwrap(), b"HELLO");
        handle.join().unwrap();
    }

    #[test]
    fn chunk_nonce_is_big_endian_counter() {
        let iv = [
            0xAA, 0xBB, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C,
            0x0D, 0x0E,
        ];
        let nonce = chunk_nonce(&iv, 0x0102);
        assert_eq!(
            &nonce[..2],
            &[0x01, 0x02],
            "计数器是大端（v1 记成小端是错的）"
        );
        assert_eq!(
            &nonce[2..],
            &iv[2..12],
            "后 10 字节来自请求 IV 的第 3~12 字节"
        );
    }

    /// 服务端测试桩：按协议文档解请求头、回响应头与分块。
    #[test]
    fn aead_header_and_body_round_trip_against_a_mock_server() {
        let uuid = "11111111-2222-3333-4444-555555555555";
        let id = super::super::vless::parse_uuid(uuid).unwrap();
        let key = cmd_key(&id);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut auth = [0u8; 16];
            stream.read_exact(&mut auth).unwrap();
            let mut sealed_len = [0u8; RESP_LEN_SEALED];
            stream.read_exact(&mut sealed_len).unwrap();
            let mut nonce = [0u8; 8];
            stream.read_exact(&mut nonce).unwrap();

            let len_key = kdf16(&key, &[b"VMess Header AEAD Key_Length", &auth, &nonce]);
            let mut len_nonce = [0u8; 12];
            len_nonce.copy_from_slice(
                &kdf(&key, &[b"VMess Header AEAD Nonce_Length", &auth, &nonce])[..12],
            );
            let len_plain = gcm_open(&len_key, &len_nonce, &sealed_len, &auth).unwrap();
            let header_len = u16::from_be_bytes([len_plain[0], len_plain[1]]) as usize;

            let header_key = kdf16(&key, &[b"VMess Header AEAD Key", &auth, &nonce]);
            let mut header_nonce = [0u8; 12];
            header_nonce
                .copy_from_slice(&kdf(&key, &[b"VMess Header AEAD Nonce", &auth, &nonce])[..12]);
            let mut sealed_header = vec![0u8; header_len + 16];
            stream.read_exact(&mut sealed_header).unwrap();
            let header = gcm_open(&header_key, &header_nonce, &sealed_header, &auth).unwrap();

            assert_eq!(header[0], VERSION);
            assert_eq!(header[34], OPT_REQUEST);
            assert_eq!(header[35] & 0x0f, SECURITY_AES128_GCM);
            assert_eq!(header[37], CMD_TCP);
            assert_eq!(u16::from_be_bytes([header[38], header[39]]), 80);
            assert_eq!(header[40], 0x02);
            assert_eq!(&header[42..53], b"example.com");
            let body_len = header.len() - 4;
            assert_eq!(
                u32::from_be_bytes(header[body_len..].try_into().unwrap()),
                fnv1a32(&header[..body_len])
            );

            let body_iv: [u8; 16] = header[1..17].try_into().unwrap();
            let body_key: [u8; 16] = header[17..33].try_into().unwrap();
            let response_v = header[33];

            // 读一个分块（明文 "PING"），长度字段被 SHAKE128 掩码异或过。
            let mut mask = ShakeMask::new(&body_iv);
            let plain = read_masked_chunk(&mut stream, &mut mask, &body_key, &body_iv, 0);
            assert_eq!(&plain, b"PING");

            // 回响应头 + 一个分块 + 结束标记。
            let response_key: [u8; 16] = sha256(&body_key)[..16].try_into().unwrap();
            let response_iv: [u8; 16] = sha256(&body_iv)[..16].try_into().unwrap();
            let payload = [response_v, 0u8, 0u8, 0u8];
            let header_key = kdf16(&response_key, &[b"AEAD Resp Header Key"]);
            let mut header_nonce = [0u8; 12];
            header_nonce.copy_from_slice(&kdf(&response_iv, &[b"AEAD Resp Header IV"])[..12]);
            let sealed_header = gcm_seal(&header_key, &header_nonce, &payload, &[]);
            let len_key = kdf16(&response_key, &[b"AEAD Resp Header Len Key"]);
            let mut len_nonce = [0u8; 12];
            len_nonce.copy_from_slice(&kdf(&response_iv, &[b"AEAD Resp Header Len IV"])[..12]);
            let sealed_len = gcm_seal(
                &len_key,
                &len_nonce,
                &(payload.len() as u16).to_be_bytes(),
                &[],
            );
            let mut out = Vec::new();
            out.extend_from_slice(&sealed_len);
            out.extend_from_slice(&sealed_header);
            let mut resp_mask = ShakeMask::new(&response_iv);
            out.extend_from_slice(&sealed_chunk(
                &response_key,
                &response_iv,
                0,
                &mut resp_mask,
                b"PONG",
            ));
            out.extend_from_slice(&sealed_chunk(
                &response_key,
                &response_iv,
                1,
                &mut resp_mask,
                b"",
            ));
            stream.write_all(&out).unwrap();
        });

        let tcp = TcpStream::connect(addr).unwrap();
        let stream: BoxStream = Box::new(tcp);
        let mut tunnel = connect(stream, uuid, 0, false, "example.com", 80).unwrap();
        tunnel.write_all(b"PING").unwrap();
        let mut out = [0u8; 4];
        tunnel.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"PONG");
        let mut tail = [0u8; 1];
        assert_eq!(tunnel.read(&mut tail).unwrap(), 0);
        handle.join().unwrap();
    }

    /// `Opt A`：长度字段是 18 字节 AEAD，密钥 `KDF16(bodyKey, "auth_len")`，
    /// 且长度与体**各用一条独立计数器**（都从 0 起）。
    #[test]
    fn authenticated_length_field_is_sealed_with_the_auth_len_subkey() {
        let body_key = [0x33u8; 16];
        let body_iv = [0u8; 16];
        let mut writer = BodyWriter {
            key: body_key,
            iv: body_iv,
            counter: 0,
            mask: Some(ShakeMask::new(&body_iv)),
            length_auth: Some(LengthAuth::new(kdf16(&body_key, &[b"auth_len"]), body_iv)),
        };
        let frame = writer.seal(b"hello");
        let padding = (0x8f8eu16 % 64) as usize;
        assert_eq!(padding, 14, "填充仍来自 SHAKE128(bodyIV) 的首个 u16");
        assert_eq!(
            frame.len(),
            18 + 16 + 5 + padding,
            "18 字节长度 + 密文 + 明文填充"
        );
        // 长度字段：`auth_len` 子密钥解出「密文 + tag + 填充」的总长。
        let mut reader = LengthAuth::new(kdf16(&body_key, &[b"auth_len"]), body_iv);
        assert_eq!(reader.decode(&frame[..18]), Some(16 + 5 + padding));
        // 体密钥解不了长度字段（两条子密钥互不通用）。
        let mut wrong = LengthAuth::new(body_key, body_iv);
        assert_eq!(wrong.decode(&frame[..18]), None);
        // 体仍用 bodyKey + 体计数器 0。
        let plain = gcm_open(
            &body_key,
            &chunk_nonce(&body_iv, 0),
            &frame[18..18 + 21],
            &[],
        )
        .unwrap();
        assert_eq!(&plain, b"hello");
    }

    /// `Opt A` 端到端：指令段声明 0x10；请求 / 响应两个方向都按「长度字段 AEAD」读写，
    /// 响应侧长度密钥与 IV 用的是**请求**的 `auth_len` / `bodyIV`（v2fly 同款）。
    #[test]
    fn authenticated_length_round_trips_against_a_mock_server() {
        let uuid = "11111111-2222-3333-4444-555555555555";
        let id = super::super::vless::parse_uuid(uuid).unwrap();
        let key = cmd_key(&id);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let (header, body_iv, body_key, response_v) = read_request_header(&mut stream, &key);
            assert_eq!(
                header[34], OPT_REQUEST_AUTH_LENGTH,
                "指令段必须带上 Opt A（0x10）"
            );

            // 读请求分块：18 字节 AEAD 长度字段 + 密文 + 明文填充。
            let mut request_length = LengthAuth::new(kdf16(&body_key, &[b"auth_len"]), body_iv);
            let mut request_mask = ShakeMask::new(&body_iv);
            let mut size_field = [0u8; 18];
            stream.read_exact(&mut size_field).unwrap();
            let padding = request_mask.next_padding();
            let total = request_length.decode(&size_field).unwrap();
            let mut body = vec![0u8; total];
            stream.read_exact(&mut body).unwrap();
            let plain = gcm_open(
                &body_key,
                &chunk_nonce(&body_iv, 0),
                &body[..total - padding],
                &[],
            )
            .unwrap();
            assert_eq!(&plain, b"PING");

            // 响应头 + 两个分块（PONG、结束标记）。
            let (response_key, response_iv) =
                write_response_header(&mut stream, &body_key, &body_iv, response_v);
            // 长度字段一律用**请求**的 auth_len 子密钥与 bodyIV。
            let mut response_length = LengthAuth::new(kdf16(&body_key, &[b"auth_len"]), body_iv);
            let mut resp_mask = ShakeMask::new(&response_iv);
            let chunk = sealed_chunk_auth_length(
                &response_key,
                &response_iv,
                0,
                &mut resp_mask,
                &mut response_length,
                b"PONG",
            );
            stream.write_all(&chunk).unwrap();
            let end = sealed_chunk_auth_length(
                &response_key,
                &response_iv,
                1,
                &mut resp_mask,
                &mut response_length,
                b"",
            );
            stream.write_all(&end).unwrap();
        });

        let tcp = TcpStream::connect(addr).unwrap();
        let stream: BoxStream = Box::new(tcp);
        let mut tunnel = connect(stream, uuid, 0, true, "example.com", 80).unwrap();
        tunnel.write_all(b"PING").unwrap();
        let mut out = [0u8; 4];
        tunnel.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"PONG");
        let mut tail = [0u8; 1];
        assert_eq!(tunnel.read(&mut tail).unwrap(), 0);
        handle.join().unwrap();
    }

    /// **真机验收（`#[ignore]`，需要外部服务器）**：取链接文件里第一条 `alterId=0` 的 VMess 节点，
    /// 强行打开 `Opt A` 再拨 `www.baidu.com:80` 发 HEAD。
    ///
    /// 用法：
    /// ```powershell
    /// $env:LUNAC_LIVE_LINKS = "d:\cc\lunac-master\temp\live-links.txt"
    /// cargo test --lib live_authenticated_length -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore]
    fn live_authenticated_length_node_answers() {
        use std::io::{Read, Write};
        use std::time::{Duration, Instant};

        use super::super::{clash, transport, Node, Protocol};

        let path = match std::env::var("LUNAC_LIVE_LINKS") {
            Ok(value) if !value.trim().is_empty() => value,
            _ => {
                eprintln!("跳过：没有设置 LUNAC_LIVE_LINKS");
                return;
            }
        };
        let text = std::fs::read_to_string(&path).expect("读不到链接文件");
        let mut node: Option<Node> = None;
        for line in text.lines().map(str::trim) {
            if !line.starts_with("vmess://") {
                continue;
            }
            let Ok(parsed) = clash::parse_link(line) else {
                continue;
            };
            if let Protocol::Vmess {
                alter_id,
                authenticated_length,
                ..
            } = &parsed.protocol
            {
                if *alter_id == 0 && !*authenticated_length {
                    node = Some(parsed);
                    break;
                }
            }
        }
        let Some(mut node) = node else {
            eprintln!("跳过：链接文件里没有 alterId=0 的 VMess 节点");
            return;
        };
        let Protocol::Vmess { uuid, alter_id, .. } = node.protocol.clone() else {
            unreachable!()
        };
        node.protocol = Protocol::Vmess {
            uuid: uuid.clone(),
            alter_id,
            security: "auto".to_string(),
            authenticated_length: true,
        };
        let stream = transport::connect_transport(&node).expect("建传输失败");
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .expect("设置读超时失败");
        let started = Instant::now();
        let mut tunnel = connect(stream, &uuid, alter_id, true, "www.baidu.com", 80)
            .expect("VMess Opt A 拨号失败");
        tunnel
            .write_all(b"HEAD / HTTP/1.1\r\nHost: www.baidu.com\r\nConnection: close\r\n\r\n")
            .expect("发请求失败");
        let mut first = [0u8; 1];
        tunnel.read_exact(&mut first).expect("读响应失败");
        println!(
            "PASS VMess Opt A（{}）: {}ms",
            node.name,
            started.elapsed().as_millis()
        );
    }

    /// 服务端桩：按 `Opt A` 封一个响应分块（长度字段走 AEAD，填充仍来自 SHAKE）。
    fn sealed_chunk_auth_length(
        key: &[u8; 16],
        iv: &[u8; 16],
        counter: u16,
        mask: &mut ShakeMask,
        length_auth: &mut LengthAuth,
        plain: &[u8],
    ) -> Vec<u8> {
        let sealed = gcm_seal(key, &chunk_nonce(iv, counter), plain, &[]);
        let padding = mask.next_padding();
        let total = sealed.len() + padding;
        let mut out = Vec::with_capacity(18 + total);
        out.extend_from_slice(&length_auth.encode(total));
        out.extend_from_slice(&sealed);
        out.extend_from_slice(&vec![0xA5u8; padding]);
        out
    }

    /// 样例 UUID（16 字节）—— 下面几条向量的输入。
    const SAMPLE_UUID: [u8; 16] = [
        0xb8, 0x31, 0x38, 0x1d, 0x63, 0x24, 0x4d, 0x53, 0xad, 0x4f, 0x8c, 0xda, 0x48, 0xb3, 0x08,
        0x11,
    ];

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn cmd_key_matches_upstream_definition() {
        // 期望值用 .NET MD5 独立算出：MD5(uuid[16] ‖ "c48619fe-8f02-49e0-b9e9-edf763e17e21")。
        assert_eq!(
            hex(&cmd_key(&SAMPLE_UUID)),
            "b50d916ac0cec067981af8e5f38a758f"
        );
        // 裸 MD5(uuid) 是另一串 —— 之前就是错在这里。
        assert_ne!(hex(&cmd_key(&SAMPLE_UUID)), hex(&md5(&SAMPLE_UUID)));
    }

    #[test]
    fn legacy_auth_id_and_header_iv_match_reference() {
        let unix = 1_700_000_000i64;
        // .NET `HMACMD5(uuid, ts[8 BE])` 独立算出的期望值。
        assert_eq!(
            hex(&legacy_auth_id(&SAMPLE_UUID, unix)),
            "cb4c98431253a6f41d16adc57e6d3550"
        );
        // MD5(ts[8 BE] ×4)。
        assert_eq!(
            hex(&legacy_timestamp_iv(unix)),
            "7a29d7bdc972aaefb1f54e5205589e78"
        );
    }

    #[test]
    fn cfb128_follows_the_definition_and_round_trips() {
        let key = [0x2bu8; 16];
        let iv = [0x0fu8; 16];
        let plain: Vec<u8> = (0..40u8).collect();
        let mut sealed = plain.clone();
        Cfb128::new(&key, &iv, false).apply(&mut sealed);
        // 定义：ct1 = pt1 ⊕ E(iv)；ct2 = pt2 ⊕ E(ct1)。
        let stream1 = aes128_ecb_encrypt(&key, &iv);
        for (i, byte) in sealed.iter().take(16).enumerate() {
            assert_eq!(*byte, plain[i] ^ stream1[i]);
        }
        let mut feedback = [0u8; 16];
        feedback.copy_from_slice(&sealed[..16]);
        let stream2 = aes128_ecb_encrypt(&key, &feedback);
        for (i, byte) in sealed.iter().skip(16).take(16).enumerate() {
            assert_eq!(*byte, plain[16 + i] ^ stream2[i]);
        }
        // 逐字节喂与一次性喂必须等价（CFB 的块内游标不能丢）。
        let mut one_by_one = plain.clone();
        let mut cfb = Cfb128::new(&key, &iv, false);
        for byte in one_by_one.iter_mut() {
            cfb.apply(std::slice::from_mut(byte));
        }
        assert_eq!(one_by_one, sealed);
        // 解密还原。
        let mut opened = sealed.clone();
        Cfb128::new(&key, &iv, true).apply(&mut opened);
        assert_eq!(opened, plain);
    }

    /// 老式（MD5 头）端到端：`alterId=2` ⇒ HMAC-MD5 AuthID + CFB 头 + GCM 体。
    #[test]
    fn legacy_header_and_body_round_trip_against_a_mock_server() {
        let uuid = "11111111-2222-3333-4444-555555555555";
        let id = super::super::vless::parse_uuid(uuid).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut auth = [0u8; 16];
            stream.read_exact(&mut auth).unwrap();
            // 服务端在时间窗里复算 AuthID（与 Xray `AuthIDDecoder` 的做法一致）。
            let now = now_unix();
            let stamp = (now - 120..=now + 120)
                .find(|ts| legacy_auth_id(&id, *ts) == auth)
                .expect("AuthID 必须能由 HMAC-MD5(uuid, ts) 复算出来");

            let mut cfb = Cfb128::new(&cmd_key(&id), &legacy_timestamp_iv(stamp), true);
            let mut read_decrypt = |count: usize| {
                let mut buf = vec![0u8; count];
                stream.read_exact(&mut buf).unwrap();
                cfb.apply(&mut buf);
                buf
            };
            let mut header = read_decrypt(41); // version..atyp
            assert_eq!(header[0], VERSION);
            assert_eq!(header[34], OPT_REQUEST);
            assert_eq!(header[35] & 0x0f, SECURITY_AES128_GCM);
            assert_eq!(header[37], CMD_TCP);
            assert_eq!(u16::from_be_bytes([header[38], header[39]]), 80);
            let pad_len = (header[35] >> 4) as usize;
            match header[40] {
                0x01 => header.extend(read_decrypt(4)),
                0x03 => header.extend(read_decrypt(16)),
                0x02 => {
                    let len = read_decrypt(1);
                    let count = len[0] as usize;
                    header.extend(len);
                    header.extend(read_decrypt(count));
                }
                other => panic!("意外的地址类型 {other}"),
            }
            assert_eq!(&header[42..53], b"example.com");
            header.extend(read_decrypt(pad_len + 4));
            let body_len = header.len() - 4;
            assert_eq!(
                u32::from_be_bytes(header[body_len..].try_into().unwrap()),
                fnv1a32(&header[..body_len]),
                "老式头的校验和必须与 v2fly 同口径"
            );

            let body_iv: [u8; 16] = header[1..17].try_into().unwrap();
            let body_key: [u8; 16] = header[17..33].try_into().unwrap();
            let response_v = header[33];

            // 读一个分块（明文 "PING"），长度被 SHAKE128(bodyIV) 掩码过。
            let mut mask = ShakeMask::new(&body_iv);
            let plain = read_masked_chunk(&mut stream, &mut mask, &body_key, &body_iv, 0);
            assert_eq!(&plain, b"PING");

            // 响应：CFB 4 字节头 + GCM 分块（响应侧 Key/IV = MD5(请求侧)，掩码用 responseBodyIV）。
            let response_key = md5(&body_key);
            let response_iv = md5(&body_iv);
            let mut head = vec![response_v, 0u8, 0u8, 0u8];
            Cfb128::new(&response_key, &response_iv, false).apply(&mut head);
            stream.write_all(&head).unwrap();
            let mut resp_mask = ShakeMask::new(&response_iv);
            stream
                .write_all(&sealed_chunk(
                    &response_key,
                    &response_iv,
                    0,
                    &mut resp_mask,
                    b"PONG",
                ))
                .unwrap();
            stream
                .write_all(&sealed_chunk(
                    &response_key,
                    &response_iv,
                    1,
                    &mut resp_mask,
                    b"",
                ))
                .unwrap();
        });

        let tcp = TcpStream::connect(addr).unwrap();
        let stream: BoxStream = Box::new(tcp);
        let mut tunnel = connect(stream, uuid, 2, false, "example.com", 80).unwrap();
        tunnel.write_all(b"PING").unwrap();
        let mut out = [0u8; 4];
        tunnel.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"PONG");
        let mut tail = [0u8; 1];
        assert_eq!(tunnel.read(&mut tail).unwrap(), 0);
        handle.join().unwrap();
    }

    /// 服务端侧：解 AEAD 头，返回（指令段, bodyIV, bodyKey, responseV）。
    fn read_request_header(
        stream: &mut TcpStream,
        key: &[u8; 16],
    ) -> (Vec<u8>, [u8; 16], [u8; 16], u8) {
        let mut auth = [0u8; 16];
        stream.read_exact(&mut auth).unwrap();
        let mut sealed_len = [0u8; RESP_LEN_SEALED];
        stream.read_exact(&mut sealed_len).unwrap();
        let mut nonce = [0u8; 8];
        stream.read_exact(&mut nonce).unwrap();

        let len_key = kdf16(key, &[b"VMess Header AEAD Key_Length", &auth, &nonce]);
        let mut len_nonce = [0u8; 12];
        len_nonce
            .copy_from_slice(&kdf(key, &[b"VMess Header AEAD Nonce_Length", &auth, &nonce])[..12]);
        let len_plain = gcm_open(&len_key, &len_nonce, &sealed_len, &auth).unwrap();
        let header_len = u16::from_be_bytes([len_plain[0], len_plain[1]]) as usize;

        let header_key = kdf16(key, &[b"VMess Header AEAD Key", &auth, &nonce]);
        let mut header_nonce = [0u8; 12];
        header_nonce.copy_from_slice(&kdf(key, &[b"VMess Header AEAD Nonce", &auth, &nonce])[..12]);
        let mut sealed_header = vec![0u8; header_len + 16];
        stream.read_exact(&mut sealed_header).unwrap();
        let header = gcm_open(&header_key, &header_nonce, &sealed_header, &auth).unwrap();
        let body_iv = header[1..17].try_into().unwrap();
        let body_key = header[17..33].try_into().unwrap();
        let response_v = header[33];
        (header, body_iv, body_key, response_v)
    }

    /// 服务端侧：拼响应头（与分块体同一套 AEAD 口径）。
    fn write_response_header(
        stream: &mut TcpStream,
        body_key: &[u8; 16],
        body_iv: &[u8; 16],
        response_v: u8,
    ) -> ([u8; 16], [u8; 16]) {
        let response_key: [u8; 16] = sha256(body_key)[..16].try_into().unwrap();
        let response_iv: [u8; 16] = sha256(body_iv)[..16].try_into().unwrap();
        let payload = [response_v, 0u8, 0u8, 0u8];
        let header_key = kdf16(&response_key, &[b"AEAD Resp Header Key"]);
        let mut header_nonce = [0u8; 12];
        header_nonce.copy_from_slice(&kdf(&response_iv, &[b"AEAD Resp Header IV"])[..12]);
        let sealed_header = gcm_seal(&header_key, &header_nonce, &payload, &[]);
        let len_key = kdf16(&response_key, &[b"AEAD Resp Header Len Key"]);
        let mut len_nonce = [0u8; 12];
        len_nonce.copy_from_slice(&kdf(&response_iv, &[b"AEAD Resp Header Len IV"])[..12]);
        let sealed_len = gcm_seal(
            &len_key,
            &len_nonce,
            &(payload.len() as u16).to_be_bytes(),
            &[],
        );
        let mut out = Vec::new();
        out.extend_from_slice(&sealed_len);
        out.extend_from_slice(&sealed_header);
        stream.write_all(&out).unwrap();
        (response_key, response_iv)
    }

    /// UDP：指令段 `cmd=0x02` + 目标在指令段里；**一个分块 = 一个数据报**。
    #[test]
    fn udp_session_uses_command_two_and_chunk_per_datagram() {
        let uuid = "11111111-2222-3333-4444-555555555555";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let id = super::super::vless::parse_uuid(uuid).unwrap();
            let key = cmd_key(&id);
            let (header, body_iv, body_key, response_v) = read_request_header(&mut stream, &key);
            assert_eq!(header[37], CMD_UDP, "UDP 的命令字节必须是 0x02");
            assert_eq!(u16::from_be_bytes([header[38], header[39]]), 53);
            assert_eq!(header[40], 0x02, "domain");
            assert_eq!(&header[42..53], b"example.com");

            let mut mask = ShakeMask::new(&body_iv);
            let plain = read_masked_chunk(&mut stream, &mut mask, &body_key, &body_iv, 0);
            assert_eq!(&plain, b"query", "一个分块携带一个数据报");

            let (response_key, response_iv) =
                write_response_header(&mut stream, &body_key, &body_iv, response_v);
            let mut resp_mask = ShakeMask::new(&response_iv);
            stream
                .write_all(&sealed_chunk(
                    &response_key,
                    &response_iv,
                    0,
                    &mut resp_mask,
                    b"answer",
                ))
                .unwrap();
        });

        let tcp = TcpStream::connect(addr).unwrap();
        tcp.set_read_timeout(Some(std::time::Duration::from_millis(200)))
            .unwrap();
        let stream: BoxStream = Box::new(tcp);
        let mut tunnel = connect_udp(stream, uuid, 0, false, "example.com", 53).unwrap();
        tunnel.send_datagram(b"query").unwrap();
        assert_eq!(
            tunnel.recv_datagram().unwrap().expect("必须解出一个数据报"),
            b"answer"
        );
        assert!(
            tunnel.recv_datagram().unwrap().is_none(),
            "没有新包时必须返回 None（不是空包）"
        );
        handle.join().unwrap();
    }
}
