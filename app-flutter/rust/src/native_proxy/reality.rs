//! REALITY 客户端：**自写 TLS 1.3** + SessionId 认证（重建 v1 `native_proxy/reality.rs`）。
//!
//! **为什么必须自写**：REALITY 把认证信息藏在 ClientHello 的 `legacy_session_id` 里，并在
//! 证书签名位塞一段 HMAC —— rustls / native-tls 都不给改这两处（`session_id` 由库自己随机，
//! 证书链也必须能验通），所以只能自己走一遍 TLS 1.3 握手。
//!
//! 协议口径（逐条对齐上游，不凭记忆）：
//!   · `XTLS/Xray-core/transport/internet/reality/reality.go` 的 `UClient`（客户端构造）；
//!   · `XTLS/REALITY/tls.go` 的 `Server`（服务端如何校验 —— 决定我们该发什么）。
//!
//! 步骤（与上游一一对应）：
//!   1. 生成临时 X25519 密钥对，与节点公钥（`pbk`）做 ECDH 得 `shared`；
//!   2. `AuthKey = HKDF-SHA256(ikm=shared, salt=ClientHello.random[:20], info="REALITY")`（32B）；
//!   3. 明文 SessionId（16B）= `[ver_x, ver_y, ver_z, 0][u32 BE unix 时间][short_id(0–8B)]`；
//!   4. `sealed = AES-256-GCM(AuthKey).seal(nonce=random[20:32], pt=SessionId[:16], aad=ClientHello)`，
//!      其中 **aad 是 SessionId 区已被清零的那份 ClientHello**（上游先 `copy(hello.Raw[39:],
//!      全零 SessionId)` 再 seal，seal 完才把密文写回 `Raw[39:]`）；密文 16+16=32B 正好填满；
//!   5. 正常完成 TLS 1.3 握手；服务端在 `Certificate` 里给的是**临时 Ed25519 证书**，
//!      其 `signatureValue` 被替换成 `HMAC-SHA512(AuthKey, 公钥)` —— 命中即认定「真的是
//!      REALITY 服务端」，不命中则当作被劫持，如实报错（不发蜘蛛请求）。
//!
//! **如实登记（未做）**：uTLS 指纹模仿（JA3/JA4 装成 Chrome）与 HelloRetryRequest —— 前者需要
//! 一整套 ClientHello 模板 + 扩展排列，与「能不能连上」无关；本实现发的是我们自己的 ClientHello
//! （TLS 1.3 + `X25519MLKEM768`/`X25519` 两组 key_share + 标准扩展），服务端只校验 SessionId 与短 ID。
//! 真机验收：Xray 25.6.8 与 26.10.31 均通过（后者必须走混合组，见 `GROUP_X25519MLKEM768`）。

use std::io::{self, Read, Write};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ml_kem::{Decapsulate as _, Kem as _};
use ring::aead;
use ring::digest;
use ring::hkdf;
use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature;
use x25519_dalek as x25519;

use super::transport::{BoxStream, Stream};
use super::Node;

/// 写进 SessionId 前 3 字节的客户端版本号。服务端可配 `minClientVer` / `maxClientVer`
/// 做区间比较（`XTLS/REALITY/tls.go` 的 `Value(hs.c.ClientVer[:])`），所以这个数不能乱填：
/// 取仓库里真机对照过的 `26.10.31`（见 `docs/todo.md` 的 Xray 26.10.31 记录）。
const CLIENT_VERSION: [u8; 3] = [26, 10, 31];

const TLS13: u16 = 0x0304;
const LEGACY_VERSION: u16 = 0x0303;
/// 纯 X25519 的 NamedGroup。
const GROUP_X25519: u16 = 0x001d;
/// 混合后量子组 `X25519MLKEM768`（draft-ietf-tls-ecdhe-mlkem，IANA 4588）。
///
/// **为什么必须发**：Xray 26 的 REALITY 直接拒没有这个组的 ClientHello
/// （`XTLS/reality` 原文 `reject outdated/strange Client Hello that doesn't have
/// X25519MLKEM768 before optional X25519`）；Xray ≤25 不认识它，但会退回 X25519。
const GROUP_X25519MLKEM768: u16 = 0x11ec;
/// ML-KEM-768 的封装密钥 / 密文长度（FIPS 203）。
const MLKEM768_EK: usize = 1184;
const MLKEM768_CT: usize = 1088;
const MAX_RECORD: usize = 16 * 1024;
/// SessionId 在 ClientHello 原始字节里的固定偏移（上游注释 `hello.Raw[39:]`）：
/// 1B 握手类型 + 3B 长度 + 2B legacy_version + 32B random + 1B SessionId 长度前缀。
const SESSION_ID_OFFSET: usize = 39;
const SESSION_ID_LEN: usize = 32;

const CONTENT_CHANGE_CIPHER_SPEC: u8 = 20;
const CONTENT_ALERT: u8 = 21;
const CONTENT_HANDSHAKE: u8 = 22;
const CONTENT_APPLICATION_DATA: u8 = 23;

const HS_CLIENT_HELLO: u8 = 1;
const HS_SERVER_HELLO: u8 = 2;
const HS_ENCRYPTED_EXTENSIONS: u8 = 8;
const HS_CERTIFICATE: u8 = 11;
const HS_CERTIFICATE_VERIFY: u8 = 15;
const HS_FINISHED: u8 = 20;
const HS_KEY_UPDATE: u8 = 24;

/// `TLS 1.3, server CertificateVerify` 前面那 64 个空格（RFC 8446 §4.4.3）。
const CERT_VERIFY_CONTEXT: &[u8] = b"TLS 1.3, server CertificateVerify";

fn err(message: impl Into<String>) -> String {
    message.into()
}

fn random_bytes(count: usize) -> Result<Vec<u8>, String> {
    let mut buf = vec![0u8; count];
    SystemRandom::new()
        .fill(&mut buf)
        .map_err(|_| err("REALITY: 生成随机数失败"))?;
    Ok(buf)
}

// ── 密钥派生（RFC 8446 §7.1）──────────────────────────────────────────

/// `HKDF-Expand-Label`（RFC 8446 §7.1）：`HkdfLabel` = `u16 长度 ‖ "tls13 "+label ‖ context`。
fn expand_label(secret: &[u8], label: &str, context: &[u8], len: usize) -> Result<Vec<u8>, String> {
    struct Len(usize);
    impl hkdf::KeyType for Len {
        fn len(&self) -> usize {
            self.0
        }
    }
    let mut info = Vec::with_capacity(4 + label.len() + context.len());
    info.extend_from_slice(&(len as u16).to_be_bytes());
    let full = format!("tls13 {label}");
    if full.len() > 255 || context.len() > 255 {
        return Err(err("REALITY: HKDF label / context 过长"));
    }
    info.push(full.len() as u8);
    info.extend_from_slice(full.as_bytes());
    info.push(context.len() as u8);
    info.extend_from_slice(context);
    let prk = hkdf::Prk::new_less_safe(hkdf::HKDF_SHA256, secret);
    let mut out = vec![0u8; len];
    prk.expand(&[info.as_slice()], Len(len))
        .map_err(|_| err("REALITY: HKDF 展开失败"))?
        .fill(&mut out)
        .map_err(|_| err("REALITY: HKDF 输出长度不符"))?;
    Ok(out)
}

/// `Derive-Secret(secret, label, transcript)` = `HKDF-Expand-Label(secret, label, SHA256(transcript), 32)`。
fn derive_secret(secret: &[u8], label: &str, transcript: &[u8]) -> Result<Vec<u8>, String> {
    let hash = digest::digest(&digest::SHA256, transcript);
    expand_label(secret, label, hash.as_ref(), 32)
}

fn sha256(data: &[u8]) -> Vec<u8> {
    digest::digest(&digest::SHA256, data).as_ref().to_vec()
}

/// `HKDF-Extract(salt, ikm)`（RFC 5869）：`PRK = HMAC-SHA256(salt, ikm)`。
///
/// 自己算而不是用 `hkdf::Salt`：ring 不暴露 `Prk` 的原始字节，而 TLS 1.3 的下一级
/// `HKDF-Expand-Label` 需要把它当 secret 再喂进去；空 salt 按规范换成 32 个零字节。
fn hkdf_extract(salt: &[u8], ikm: &[u8]) -> Vec<u8> {
    let zeros = [0u8; 32];
    let key = hmac::Key::new(
        hmac::HMAC_SHA256,
        if salt.is_empty() { &zeros } else { salt },
    );
    hmac::sign(&key, ikm).as_ref().to_vec()
}

// ── ClientHello ──────────────────────────────────────────────────────

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn extension(out: &mut Vec<u8>, kind: u16, body: &[u8]) {
    push_u16(out, kind);
    push_u16(out, body.len() as u16);
    out.extend_from_slice(body);
}

/// 组一个 TLS 1.3 ClientHello（SessionId 区先留全零，由 [`seal_session_id`] 之后写回）。
///
/// `raw[39..71]` 必须是 32 字节 SessionId 占位 —— 上游直接按这个固定偏移改字节。
///
/// `key_shares` 的顺序就是 `key_share` 扩展里的顺序（REALITY 26 要求
/// `X25519MLKEM768` 排在 `X25519` **之前**，所以调用方先放混合组）。
fn build_client_hello(
    server_name: &str,
    alpn: &[String],
    key_shares: &[(u16, Vec<u8>)],
    random: &[u8],
) -> Result<Vec<u8>, String> {
    if random.len() != 32 || key_shares.is_empty() {
        return Err(err("REALITY: random / key_share 长度不对"));
    }
    let mut body = Vec::new();
    push_u16(&mut body, LEGACY_VERSION);
    body.extend_from_slice(random);
    body.push(SESSION_ID_LEN as u8);
    let offset = 4 + body.len();
    if offset != SESSION_ID_OFFSET {
        return Err(err("REALITY: ClientHello 偏移算错（SessionId 必须在 39）"));
    }
    body.extend_from_slice(&[0u8; SESSION_ID_LEN]);
    // cipher suites：只声明我们真能用的两个（AES-128-GCM / ChaCha20-Poly1305）
    push_u16(&mut body, 4);
    push_u16(&mut body, 0x1301);
    push_u16(&mut body, 0x1303);
    body.extend_from_slice(&[1, 0]); // 压缩方法：只有 null

    let mut exts = Vec::new();
    // server_name (0x0000)
    let mut sni = Vec::new();
    push_u16(&mut sni, (server_name.len() + 3) as u16);
    sni.push(0); // host_name
    push_u16(&mut sni, server_name.len() as u16);
    sni.extend_from_slice(server_name.as_bytes());
    extension(&mut exts, 0x0000, &sni);
    // supported_groups (0x000a)：与 key_share 同序（混合组在前）
    let mut groups = Vec::new();
    push_u16(&mut groups, (key_shares.len() * 2) as u16);
    for (group, _) in key_shares {
        push_u16(&mut groups, *group);
    }
    extension(&mut exts, 0x000a, &groups);
    // signature_algorithms (0x000d)：REALITY 的临时证书是 Ed25519，必须声明
    let mut algs = Vec::new();
    push_u16(&mut algs, 6);
    for alg in [0x0807u16, 0x0403, 0x0804] {
        push_u16(&mut algs, alg);
    }
    extension(&mut exts, 0x000d, &algs);
    // supported_versions (0x002b)：TLS 1.3
    //
    // **ClientHello 里这个扩展体是 `u8 列表长度 + 版本列表`**（RFC 8446 §4.2.1），
    // 不是 `u16` —— 2026-11-15 真机：漏掉那个 1 字节长度，Xray 的 REALITY 直接回
    // `TLS alert 70 (protocol_version)`（它的 `clientHelloMsg.unmarshal` 会把这个扩展读成空列表）。
    let mut versions = Vec::new();
    versions.push(2u8);
    push_u16(&mut versions, TLS13);
    extension(&mut exts, 0x002b, &versions);
    // key_share (0x0033)：每个组一条 `group u16 | 长度 u16 | 数据`
    let mut shares = Vec::new();
    for (group, data) in key_shares {
        push_u16(&mut shares, *group);
        push_u16(&mut shares, data.len() as u16);
        shares.extend_from_slice(data);
    }
    let mut body_shares = Vec::with_capacity(2 + shares.len());
    push_u16(&mut body_shares, shares.len() as u16);
    body_shares.extend_from_slice(&shares);
    extension(&mut exts, 0x0033, &body_shares);
    // ALPN (0x0010)：节点写了才发
    if !alpn.is_empty() {
        let mut list = Vec::new();
        let mut names = Vec::new();
        for item in alpn {
            names.push(item.len() as u8);
            names.extend_from_slice(item.as_bytes());
        }
        push_u16(&mut list, names.len() as u16);
        list.extend_from_slice(&names);
        extension(&mut exts, 0x0010, &list);
    }

    push_u16(&mut body, exts.len() as u16);
    body.extend_from_slice(&exts);

    let mut raw = Vec::with_capacity(body.len() + 4);
    raw.push(HS_CLIENT_HELLO);
    raw.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
    raw.extend_from_slice(&body);
    Ok(raw)
}

/// 明文 SessionId（16 字节）：版本 3 字节 + 保留 0 + 大端 unix 时间 + 短 ID。
fn session_id_plain(short_id: &[u8], now_unix: u32) -> Result<[u8; 16], String> {
    if short_id.len() > 8 {
        return Err(format!(
            "REALITY: short_id 最多 8 字节（实际 {}）",
            short_id.len()
        ));
    }
    let mut plain = [0u8; 16];
    plain[..3].copy_from_slice(&CLIENT_VERSION);
    // plain[3] = 0（保留位）
    plain[4..8].copy_from_slice(&now_unix.to_be_bytes());
    plain[8..8 + short_id.len()].copy_from_slice(short_id);
    Ok(plain)
}

/// 用 `AuthKey` 把 16 字节明文 SessionId 封成 32 字节（AAD = **SessionId 区已清零**的 ClientHello）。
fn seal_session_id(
    auth_key: &[u8],
    random: &[u8],
    plain: &[u8; 16],
    raw_with_zeroed_session_id: &[u8],
) -> Result<Vec<u8>, String> {
    if auth_key.len() != 32 || random.len() != 32 {
        return Err(err("REALITY: AuthKey / random 长度不对"));
    }
    let unbound = aead::UnboundKey::new(&aead::AES_256_GCM, auth_key)
        .map_err(|_| err("REALITY: AES-256-GCM 初始化失败"))?;
    let key = aead::LessSafeKey::new(unbound);
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&random[20..32]);
    let mut sealed = plain.to_vec();
    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::from(raw_with_zeroed_session_id),
        &mut sealed,
    )
    .map_err(|_| err("REALITY: SessionId 封装失败"))?;
    Ok(sealed)
}

/// 计算 `AuthKey`：`X25519(临时私钥, 服务端公钥)` → `HKDF-SHA256(salt=random[:20], info="REALITY")`。
fn auth_key(shared: &[u8], random: &[u8]) -> Result<Vec<u8>, String> {
    if random.len() != 32 {
        return Err(err("REALITY: random 长度不对"));
    }
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, &random[..20]).extract(shared);
    struct Len(usize);
    impl hkdf::KeyType for Len {
        fn len(&self) -> usize {
            self.0
        }
    }
    let mut out = vec![0u8; 32];
    prk.expand(&[b"REALITY".as_slice()], Len(32))
        .map_err(|_| err("REALITY: AuthKey 派生失败"))?
        .fill(&mut out)
        .map_err(|_| err("REALITY: AuthKey 长度不符"))?;
    Ok(out)
}

// ── 记录层 ────────────────────────────────────────────────────────────

struct RecordKeys {
    key: aead::LessSafeKey,
    iv: [u8; 12],
    seq: u64,
}

impl RecordKeys {
    fn new(secret: &[u8], suite: &'static aead::Algorithm) -> Result<Self, String> {
        let key_bytes = expand_label(secret, "key", &[], suite.key_len())?;
        let iv = expand_label(secret, "iv", &[], 12)?;
        let unbound = aead::UnboundKey::new(suite, &key_bytes)
            .map_err(|_| err("REALITY: 记录层密钥初始化失败"))?;
        let mut iv_array = [0u8; 12];
        iv_array.copy_from_slice(&iv);
        Ok(Self {
            key: aead::LessSafeKey::new(unbound),
            iv: iv_array,
            seq: 0,
        })
    }

    /// nonce = IV 与「序号按大端写进低 8 字节」逐字节异或（RFC 8446 §5.3）。
    fn nonce(&self) -> [u8; 12] {
        let mut nonce = self.iv;
        let seq = self.seq.to_be_bytes();
        for index in 0..8 {
            nonce[4 + index] ^= seq[index];
        }
        nonce
    }

    fn seal(&mut self, content_type: u8, data: &[u8], out: &mut Vec<u8>) -> Result<(), String> {
        let mut inner = data.to_vec();
        inner.push(content_type);
        let nonce = self.nonce();
        let len = inner.len() + 16;
        out.push(CONTENT_APPLICATION_DATA);
        out.extend_from_slice(&[0x03, 0x03]);
        out.extend_from_slice(&(len as u16).to_be_bytes());
        self.key
            .seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(&out[out.len() - 5..]),
                &mut inner,
            )
            .map_err(|_| err("REALITY: 记录加密失败"))?;
        out.extend_from_slice(&inner);
        self.seq = self.seq.wrapping_add(1);
        Ok(())
    }

    /// 解密一条已收齐的记录；返回（内层内容类型, 明文）。
    fn open(&mut self, header: &[u8], payload: &[u8]) -> Result<(u8, Vec<u8>), String> {
        let nonce = self.nonce();
        let mut buffer = payload.to_vec();
        let plain = self
            .key
            .open_in_place(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(header),
                &mut buffer,
            )
            .map_err(|_| err("REALITY: 记录解密失败（服务端不是 REALITY 或短 ID/公钥不对）"))?;
        let mut end = plain.len();
        while end > 0 && plain[end - 1] == 0 {
            end -= 1;
        }
        if end == 0 {
            return Err(err("REALITY: 记录里没有内容类型字节"));
        }
        let content_type = plain[end - 1];
        self.seq = self.seq.wrapping_add(1);
        Ok((content_type, plain[..end - 1].to_vec()))
    }
}

// ── 握手 ──────────────────────────────────────────────────────────────

/// 一条握手消息（已跨记录拼好）。
struct HandshakeBuf {
    data: Vec<u8>,
}

impl HandshakeBuf {
    fn new() -> Self {
        Self { data: Vec::new() }
    }

    /// 取出下一条完整的握手消息（1B 类型 + 3B 长度）。
    fn next(&mut self) -> Option<(u8, Vec<u8>)> {
        if self.data.len() < 4 {
            return None;
        }
        let len = ((self.data[1] as usize) << 16)
            | ((self.data[2] as usize) << 8)
            | self.data[3] as usize;
        if self.data.len() < 4 + len {
            return None;
        }
        let kind = self.data[0];
        let body = self.data[4..4 + len].to_vec();
        self.data.drain(..4 + len);
        Some((kind, body))
    }
}

/// 从 `inner` 读一条完整记录：返回（内容类型, 5 字节头, 载荷）。超时按 `WouldBlock` 上抛。
fn read_record(inner: &mut BoxStream) -> io::Result<(u8, [u8; 5], Vec<u8>)> {
    let mut header = [0u8; 5];
    inner.read_exact(&mut header)?;
    let len = u16::from_be_bytes([header[3], header[4]]) as usize;
    if len == 0 || len > MAX_RECORD + 2048 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("REALITY: 记录长度异常（{len}）"),
        ));
    }
    let mut payload = vec![0u8; len];
    inner.read_exact(&mut payload)?;
    Ok((header[0], header, payload))
}

fn alert_text(payload: &[u8]) -> String {
    if payload.len() >= 2 {
        format!("TLS alert level={} description={}", payload[0], payload[1])
    } else {
        "TLS alert（内容不完整）".to_string()
    }
}

/// 解析 ServerHello：返回（选中的套件, 密钥组, 服务端 key_share, 回显的 SessionId）。
fn parse_server_hello(body: &[u8]) -> Result<(u16, u16, Vec<u8>, Vec<u8>), String> {
    if body.len() < 38 {
        return Err(err("REALITY: ServerHello 过短"));
    }
    let version = u16::from_be_bytes([body[0], body[1]]);
    if version != LEGACY_VERSION {
        return Err(format!(
            "REALITY: ServerHello legacy_version 异常（{version:#06x}）"
        ));
    }
    let sid_len = body[34] as usize;
    if body.len() < 35 + sid_len + 3 {
        return Err(err("REALITY: ServerHello 结构不完整"));
    }
    let session_id = body[35..35 + sid_len].to_vec();
    let mut pos = 35 + sid_len;
    let suite = u16::from_be_bytes([body[pos], body[pos + 1]]);
    pos += 3; // 套件 2 + 压缩方法 1
    if body.len() < pos + 2 {
        return Err(err("REALITY: ServerHello 缺扩展长度"));
    }
    let ext_len = u16::from_be_bytes([body[pos], body[pos + 1]]) as usize;
    pos += 2;
    let end = (pos + ext_len).min(body.len());
    while pos + 4 <= end {
        let kind = u16::from_be_bytes([body[pos], body[pos + 1]]);
        let len = u16::from_be_bytes([body[pos + 2], body[pos + 3]]) as usize;
        let start = pos + 4;
        if start + len > end {
            break;
        }
        if kind == 0x0033 {
            let share = &body[start..start + len];
            if share.len() < 4 {
                return Err(err("REALITY: key_share 过短"));
            }
            let group = u16::from_be_bytes([share[0], share[1]]);
            if group != GROUP_X25519 && group != GROUP_X25519MLKEM768 {
                return Err(format!(
                    "REALITY: 服务端选了不支持的密钥组 0x{group:04x}（只做 x25519 / X25519MLKEM768）"
                ));
            }
            let key_len = u16::from_be_bytes([share[2], share[3]]) as usize;
            if share.len() < 4 + key_len {
                return Err(err("REALITY: key_share 数据不完整"));
            }
            return Ok((suite, group, share[4..4 + key_len].to_vec(), session_id));
        }
        pos = start + len;
    }
    Err(err(
        "REALITY: ServerHello 里没有 key_share（可能是 HelloRetryRequest，未实现）",
    ))
}

/// 解析 Certificate 消息，取出叶子证书 DER。
fn leaf_certificate(body: &[u8]) -> Result<Vec<u8>, String> {
    if body.len() < 4 {
        return Err(err("REALITY: Certificate 过短"));
    }
    let mut pos = 1; // 跳过 certificate_request_context 的长度（服务端发的是空）
    pos += body[0] as usize;
    if body.len() < pos + 3 {
        return Err(err("REALITY: Certificate 缺列表长度"));
    }
    let _list_len =
        ((body[pos] as usize) << 16) | ((body[pos + 1] as usize) << 8) | body[pos + 2] as usize;
    pos += 3;
    if body.len() < pos + 3 {
        return Err(err("REALITY: Certificate 列表为空"));
    }
    let der_len =
        ((body[pos] as usize) << 16) | ((body[pos + 1] as usize) << 8) | body[pos + 2] as usize;
    pos += 3;
    if body.len() < pos + der_len {
        return Err(err("REALITY: 叶子证书长度越界"));
    }
    Ok(body[pos..pos + der_len].to_vec())
}

/// 读一个 DER TLV：返回（tag, 内容起点, 内容长度, 下一个 TLV 的起点）。
fn der_tlv(data: &[u8], pos: usize) -> Result<(u8, usize, usize, usize), String> {
    if data.len() < pos + 2 {
        return Err(err("DER: 数据不足"));
    }
    let tag = data[pos];
    let first = data[pos + 1];
    let (len, header) = if first & 0x80 == 0 {
        (first as usize, 2)
    } else {
        let count = (first & 0x7f) as usize;
        if count == 0 || count > 4 || data.len() < pos + 2 + count {
            return Err(err("DER: 长度字段不合法"));
        }
        let mut len = 0usize;
        for index in 0..count {
            len = (len << 8) | data[pos + 2 + index] as usize;
        }
        (len, 2 + count)
    };
    let start = pos + header;
    if data.len() < start + len {
        return Err(err("DER: 内容越界"));
    }
    Ok((tag, start, len, start + len))
}

/// 从 X.509 证书里取（Ed25519 公钥 32 字节, `signatureValue` 字节）。
///
/// 只做 REALITY 需要的那几个字段 —— 不验签名链（临时证书本来就没有可验的链）。
fn cert_public_key_and_signature(der: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
    let (tag, start, len, _) = der_tlv(der, 0)?;
    if tag != 0x30 {
        return Err(err("证书不是 SEQUENCE"));
    }
    let cert = &der[start..start + len];
    // Certificate = SEQUENCE{ tbsCertificate, signatureAlgorithm, signatureValue }
    let (tbs_tag, tbs_start, tbs_len, next) = der_tlv(cert, 0)?;
    if tbs_tag != 0x30 {
        return Err(err("tbsCertificate 不是 SEQUENCE"));
    }
    let (_, _, _, sig_end) = der_tlv(cert, next)?; // signatureAlgorithm
    let (sig_tag, sig_start, sig_len, _) = der_tlv(cert, sig_end)?;
    if sig_tag != 0x03 {
        return Err(err("signatureValue 不是 BIT STRING"));
    }
    let signature = cert[sig_start + 1..sig_start + sig_len].to_vec(); // 去掉 unused-bits 字节

    let tbs = &cert[tbs_start..tbs_start + tbs_len];
    let mut pos = 0;
    let (first_tag, _, _, first_next) = der_tlv(tbs, 0)?;
    if first_tag == 0xa0 {
        pos = first_next; // version [0] EXPLICIT
    }
    // serialNumber / signature / issuer / validity / subject 之后就是 SPKI
    for _ in 0..5 {
        let (_, _, _, next) = der_tlv(tbs, pos)?;
        pos = next;
    }
    let (spki_tag, spki_start, spki_len, _) = der_tlv(tbs, pos)?;
    if spki_tag != 0x30 {
        return Err(err("subjectPublicKeyInfo 不是 SEQUENCE"));
    }
    let spki = &tbs[spki_start..spki_start + spki_len];
    let (_, _, _, after_alg) = der_tlv(spki, 0)?;
    let (bits_tag, bits_start, bits_len, _) = der_tlv(spki, after_alg)?;
    if bits_tag != 0x03 || bits_len < 2 {
        return Err(err("subjectPublicKey 不是 BIT STRING"));
    }
    let public_key = spki[bits_start + 1..bits_start + bits_len].to_vec();
    Ok((public_key, signature))
}

/// 校验「这是 REALITY 的临时证书」：`HMAC-SHA512(AuthKey, 公钥)` 与证书签名位逐字节相等。
fn verify_temporary_certificate(leaf_der: &[u8], auth_key_bytes: &[u8]) -> Result<(), String> {
    let (public_key, signature) = cert_public_key_and_signature(leaf_der)
        .map_err(|e| format!("REALITY: 证书解析失败（{e}）"))?;
    if public_key.len() != 32 {
        return Err(format!(
            "REALITY: 证书公钥不是 32 字节 Ed25519（实际 {}）⇒ 收到的不是 REALITY 临时证书",
            public_key.len()
        ));
    }
    let key = hmac::Key::new(hmac::HMAC_SHA512, auth_key_bytes);
    let digest = hmac::sign(&key, &public_key);
    if digest.as_ref() != signature.as_slice() {
        return Err(
            "REALITY: 证书签名与 AuthKey 的 HMAC 不匹配 ⇒ 对端不是 REALITY（或 short_id / 公钥不对）"
                .to_string(),
        );
    }
    Ok(())
}

fn verify_certificate_verify(
    public_key: &[u8],
    sig: &[u8],
    transcript: &[u8],
) -> Result<(), String> {
    let mut message = vec![0x20u8; 64];
    message.extend_from_slice(CERT_VERIFY_CONTEXT);
    message.push(0);
    message.extend_from_slice(&sha256(transcript));
    signature::UnparsedPublicKey::new(&signature::ED25519, public_key)
        .verify(&message, sig)
        .map_err(|_| err("REALITY: 服务端 CertificateVerify 验签失败"))
}

fn finished_verify_data(secret: &[u8], transcript: &[u8]) -> Result<Vec<u8>, String> {
    let key = expand_label(secret, "finished", &[], 32)?;
    let hmac_key = hmac::Key::new(hmac::HMAC_SHA256, &key);
    Ok(hmac::sign(&hmac_key, &sha256(transcript)).as_ref().to_vec())
}

/// 完成 REALITY 的 TLS 1.3 握手，返回（读密钥, 写密钥）。
fn handshake(
    inner: &mut BoxStream,
    node: &Node,
    reality: &super::RealityOptions,
) -> Result<(RecordKeys, RecordKeys), String> {
    let server_name = if node.tls.sni.trim().is_empty() {
        node.server.trim()
    } else {
        node.tls.sni.trim()
    };
    if server_name.is_empty() {
        return Err(err("REALITY: 没有 SNI（tls.sni 与 server 都为空）"));
    }
    if reality.public_key.len() != 32 {
        return Err(format!(
            "REALITY: pbk 必须是 32 字节 X25519 公钥（实际 {}）",
            reality.public_key.len()
        ));
    }

    let random = random_bytes(32)?;
    // **一对临时密钥要算两次 ECDH**（2026-11-15 真机踩到）：
    //   ① REALITY 认证：`ECDH(临时私钥, pbk)`，服务端拿 ClientHello 里的公钥做对称运算；
    //   ② TLS 1.3：`ECDH(临时私钥, ServerHello 的 key_share)`。
    // 两次必须用**同一把**临时私钥（公钥在 ClientHello 里已经发出去了）。
    // ring 的 `agree_ephemeral` 会把私钥吃掉（设计上「一把钥匙只能协商一次」），
    // 所以这里改用 `x25519-dalek` 的 `StaticSecret`（可反复 `diffie_hellman`）。
    // 曾经把 ① 的结果直接当成 ② 的 shared 用：服务端日志显示认证通过、握手也发出了，
    // 但客户端一解密 EE 就失败（`记录解密失败`）。
    let mut secret_bytes = [0u8; 32];
    getrandom::getrandom(&mut secret_bytes)
        .map_err(|_| err("REALITY: 生成 X25519 临时密钥失败"))?;
    let ephemeral = x25519::StaticSecret::from(secret_bytes);
    let x25519_share = x25519::PublicKey::from(&ephemeral).to_bytes().to_vec();

    // Xray 26 起 REALITY 强制要求 `X25519MLKEM768`（draft-ietf-tls-ecdhe-mlkem）：
    // key_share 数据 = ML-KEM-768 封装密钥(1184) ‖ x25519 公钥(32)，且该组必须排在
    // x25519 **之前**；服务端 REALITY 认证只取后面那 32 字节做 ECDH，TLS 1.3 的 shared
    // 才是 `ML-KEM 共享密钥(32) ‖ x25519 共享密钥(32)`。
    let (mlkem_dk, mlkem_ek) = ml_kem::MlKem768::generate_keypair();
    let mut hybrid = ml_kem::KeyExport::to_bytes(&mlkem_ek).as_slice().to_vec();
    if hybrid.len() != MLKEM768_EK {
        return Err(err("REALITY: ML-KEM-768 封装密钥长度异常"));
    }
    hybrid.extend_from_slice(&x25519_share);
    let key_shares = vec![
        (GROUP_X25519MLKEM768, hybrid),
        (GROUP_X25519, x25519_share.clone()),
    ];

    let mut hello = build_client_hello(server_name, &node.tls.alpn, &key_shares, &random)?;
    let plain = session_id_plain(
        &reality.short_id,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as u32)
            .unwrap_or(0),
    )?;
    let mut peer_key = [0u8; 32];
    peer_key.copy_from_slice(&reality.public_key);
    let peer = x25519::PublicKey::from(peer_key);
    let auth_shared = ephemeral.diffie_hellman(&peer);
    let auth = auth_key(auth_shared.as_bytes(), &random)?;
    let sealed = seal_session_id(&auth, &random, &plain, &hello)?;
    hello[SESSION_ID_OFFSET..SESSION_ID_OFFSET + SESSION_ID_LEN].copy_from_slice(&sealed);

    // ClientHello（明文握手记录）
    let mut record = Vec::with_capacity(hello.len() + 5);
    record.push(CONTENT_HANDSHAKE);
    record.extend_from_slice(&[0x03, 0x01]);
    record.extend_from_slice(&(hello.len() as u16).to_be_bytes());
    record.extend_from_slice(&hello);
    inner
        .write_all(&record)
        .map_err(|e| format!("REALITY: 发送 ClientHello 失败：{e}"))?;

    let mut transcript = hello.clone();
    let mut handshake_buf = HandshakeBuf::new();

    // ServerHello（明文）
    let (suite, group, server_share) = loop {
        let (kind, _header, payload) =
            read_record(inner).map_err(|e| format!("REALITY: 读 ServerHello 失败：{e}"))?;
        match kind {
            CONTENT_CHANGE_CIPHER_SPEC => continue,
            CONTENT_ALERT => {
                return Err(format!(
                    "REALITY: 服务端拒绝连接（{}）",
                    alert_text(&payload)
                ))
            }
            CONTENT_HANDSHAKE => {}
            _ => return Err(err("REALITY: ServerHello 之前收到意外记录")),
        }
        handshake_buf.data.extend_from_slice(&payload);
        let Some((hs_kind, body)) = handshake_buf.next() else {
            continue;
        };
        if hs_kind != HS_SERVER_HELLO {
            return Err(format!("REALITY: 期望 ServerHello，收到握手类型 {hs_kind}"));
        }
        transcript.extend_from_slice(&handshake_message_bytes(HS_SERVER_HELLO, &body));
        let parsed = parse_server_hello(&body)?;
        handshake_buf.data.clear();
        break (parsed.0, parsed.1, parsed.2);
    };

    let suite_alg = match suite {
        0x1301 => &aead::AES_128_GCM,
        0x1303 => &aead::CHACHA20_POLY1305,
        other => {
            return Err(format!(
            "REALITY: 服务端选了不支持的套件 0x{other:04x}（只做 AES-128-GCM / ChaCha20-Poly1305）"
        ))
        }
    };

    // 密钥表（RFC 8446 §7.1）：ECDHE 用**ServerHello 里的 key_share** 与同一把临时私钥算。
    //
    // 混合组时 shared = `ML-KEM 共享密钥(32) ‖ x25519 共享密钥(32)`，顺序与
    // draft-ietf-tls-ecdhe-mlkem §4.1 一致（ML-KEM 在**前**，与 key_share 里的拼接顺序相反）。
    let tls_shared: Vec<u8> = match (group, server_share.len()) {
        (GROUP_X25519MLKEM768, len) if len == MLKEM768_CT + 32 => {
            let ct = ml_kem::Ciphertext::<ml_kem::MlKem768>::try_from(&server_share[..MLKEM768_CT])
                .map_err(|_| err("REALITY: 服务端的 ML-KEM 密文解析失败"))?;
            let mlkem_shared = mlkem_dk.decapsulate(&ct);
            let mut share = [0u8; 32];
            share.copy_from_slice(&server_share[MLKEM768_CT..]);
            let x_shared = ephemeral.diffie_hellman(&x25519::PublicKey::from(share));
            let mut out = Vec::with_capacity(64);
            out.extend_from_slice(mlkem_shared.as_slice());
            out.extend_from_slice(x_shared.as_bytes());
            out
        }
        (GROUP_X25519, 32) => {
            let mut share = [0u8; 32];
            share.copy_from_slice(&server_share);
            ephemeral
                .diffie_hellman(&x25519::PublicKey::from(share))
                .as_bytes()
                .to_vec()
        }
        (other, len) => {
            return Err(format!(
                "REALITY: 服务端 key_share 组 0x{other:04x} / 长度 {len} 不匹配（x25519 要 32，\
                 X25519MLKEM768 要 {}）",
                MLKEM768_CT + 32
            ))
        }
    };
    let zeros = [0u8; 32];
    let early_secret = hkdf_extract(&zeros, &zeros);
    let derived = derive_secret(&early_secret, "derived", &[])?;
    let handshake_secret = hkdf_extract(&derived, &tls_shared);
    let client_hs = derive_secret(&handshake_secret, "c hs traffic", &transcript)?;
    let server_hs = derive_secret(&handshake_secret, "s hs traffic", &transcript)?;
    let mut read_keys = RecordKeys::new(&server_hs, suite_alg)?;
    let mut write_keys = RecordKeys::new(&client_hs, suite_alg)?;

    // 加密的握手消息：EE → Certificate → CertificateVerify → Finished
    let mut leaf: Option<Vec<u8>> = None;
    // (签名算法, 签名, **CertificateVerify 之前的** transcript)
    //
    // 第三个字段必须是「没把自己算进去」的快照：RFC 8446 §4.4.3 的签名内容 = 到
    // Certificate 为止的握手记录，**不含** CertificateVerify 本身。2026-11-15 真机：
    // 一开始把 CV 也追加进 transcript 再验签，永远失败（`服务端 CertificateVerify 验签失败`）。
    let mut cert_verify: Option<(Vec<u8>, Vec<u8>, Vec<u8>)> = None;
    loop {
        if let Some((kind, body)) = handshake_buf.next() {
            let raw = handshake_message_bytes(kind, &body);
            match kind {
                HS_ENCRYPTED_EXTENSIONS => transcript.extend_from_slice(&raw),
                HS_CERTIFICATE => {
                    leaf = Some(leaf_certificate(&body)?);
                    transcript.extend_from_slice(&raw);
                }
                HS_CERTIFICATE_VERIFY => {
                    if body.len() < 4 {
                        return Err(err("REALITY: CertificateVerify 过短"));
                    }
                    let sig_len = u16::from_be_bytes([body[2], body[3]]) as usize;
                    if body.len() < 4 + sig_len {
                        return Err(err("REALITY: CertificateVerify 签名长度越界"));
                    }
                    cert_verify = Some((
                        body[..2].to_vec(),
                        body[4..4 + sig_len].to_vec(),
                        transcript.clone(),
                    ));
                    transcript.extend_from_slice(&raw);
                }
                HS_FINISHED => {
                    let expected = finished_verify_data(&server_hs, &transcript)?;
                    if body != expected {
                        return Err(err("REALITY: 服务端 Finished 校验失败"));
                    }
                    transcript.extend_from_slice(&raw);
                    break;
                }
                HS_KEY_UPDATE => {
                    return Err(err("REALITY: 握手阶段收到 KeyUpdate（未实现）"));
                }
                _ => transcript.extend_from_slice(&raw),
            }
            continue;
        }
        let (kind, header, payload) =
            read_record(inner).map_err(|e| format!("REALITY: 读握手记录失败：{e}"))?;
        match kind {
            CONTENT_CHANGE_CIPHER_SPEC => continue,
            CONTENT_ALERT => {
                return Err(format!(
                    "REALITY: 服务端中断握手（{}）",
                    alert_text(&payload)
                ))
            }
            CONTENT_APPLICATION_DATA => {
                let (inner_type, plain) = read_keys.open(&header, &payload)?;
                if inner_type == CONTENT_ALERT {
                    return Err(format!("REALITY: 服务端中断握手（{}）", alert_text(&plain)));
                }
                handshake_buf.data.extend_from_slice(&plain);
            }
            _ => return Err(err("REALITY: 握手阶段收到意外记录")),
        }
    }

    let leaf = leaf.ok_or_else(|| err("REALITY: 服务端没有发证书"))?;
    verify_temporary_certificate(&leaf, &auth)?;
    let (public_key, _) = cert_public_key_and_signature(&leaf)?;
    let (_, signature, cv_transcript) =
        cert_verify.ok_or_else(|| err("REALITY: 服务端没有发 CertificateVerify"))?;
    verify_certificate_verify(&public_key, &signature, &cv_transcript)?;

    // 客户端 Finished → 应用数据密钥
    let client_finished = finished_verify_data(&client_hs, &transcript)?;
    // 发出去的必须是**完整的握手消息**（1B 类型 + 3B 长度 + verify_data），
    // 不能只写 32 字节 verify_data —— 2026-11-15 真机：只写 verify_data 时服务端解密成功但
    // 解析失败（`tls: handshake message of length 7180960 bytes exceeds maximum`），
    // 我们这边收到的是 `alert 80 (internal_error)`。
    let finished_msg = handshake_message_bytes(HS_FINISHED, &client_finished);
    let mut out = Vec::new();
    write_keys.seal(CONTENT_HANDSHAKE, &finished_msg, &mut out)?;
    inner
        .write_all(&out)
        .map_err(|e| format!("REALITY: 发送 Finished 失败：{e}"))?;

    let derived2 = derive_secret(&handshake_secret, "derived", &[])?;
    let master = hkdf_extract(&derived2, &zeros);
    // 应用数据密钥的 transcript 是 **`ClientHello...server Finished`**（RFC 8446 §7.1 的
    // 「Application Data」一栏），**不含客户端的 Finished** —— 2026-11-15 真机验证：
    // 把客户端 Finished 也塞进去，两个方向都会 `bad record MAC`（服务端日志原话）。
    let client_ap = derive_secret(&master, "c ap traffic", &transcript)?;
    let server_ap = derive_secret(&master, "s ap traffic", &transcript)?;
    Ok((
        RecordKeys::new(&server_ap, suite_alg)?,
        RecordKeys::new(&client_ap, suite_alg)?,
    ))
}

/// 重建握手消息原始字节（类型 + 3 字节长度 + 正文）—— 进 transcript 的必须是这个形态。
fn handshake_message_bytes(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(body.len() + 4);
    raw.push(kind);
    raw.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
    raw.extend_from_slice(body);
    raw
}

/// REALITY 之后的字节流：读时解记录、写时按 16KB 分片加密。
pub struct RealityStream {
    inner: BoxStream,
    read_keys: RecordKeys,
    write_keys: RecordKeys,
    /// 还没解密的原始字节（记录可能被拆成多次 read 到达）
    raw: Vec<u8>,
    /// 已解出的应用数据
    plain: Vec<u8>,
}

impl RealityStream {
    /// 尝试从 `raw` 里解出一条记录；不够一条就返回 `false`（需要继续读）。
    fn poll_record(&mut self) -> Result<bool, String> {
        if self.raw.len() < 5 {
            return Ok(false);
        }
        let len = u16::from_be_bytes([self.raw[3], self.raw[4]]) as usize;
        if self.raw.len() < 5 + len {
            return Ok(false);
        }
        let header: [u8; 5] = self.raw[..5].try_into().expect("5 字节头");
        let payload = self.raw[5..5 + len].to_vec();
        self.raw.drain(..5 + len);
        // 明文 alert（服务端关连接时可能发）：不加密，直接按内容报错，别拿去解密。
        if header[0] == CONTENT_ALERT {
            return Err(format!(
                "REALITY: 对端发来 alert（{}）",
                alert_text(&payload)
            ));
        }
        let (kind, plain) = self.read_keys.open(&header, &payload)?;
        match kind {
            CONTENT_APPLICATION_DATA => {
                self.plain.extend_from_slice(&plain);
                Ok(true)
            }
            CONTENT_ALERT => Err(format!("REALITY: 对端发来 alert（{}）", alert_text(&plain))),
            CONTENT_HANDSHAKE => {
                // 握手之后的 NewSessionTicket / KeyUpdate：前一概忽略，后者我们没有实现
                if plain.first() == Some(&HS_KEY_UPDATE) {
                    return Err(err("REALITY: 对端要求 KeyUpdate（未实现）"));
                }
                Ok(true)
            }
            _ => Ok(true),
        }
    }
}

impl Read for RealityStream {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        loop {
            if !self.plain.is_empty() {
                let count = out.len().min(self.plain.len());
                out[..count].copy_from_slice(&self.plain[..count]);
                self.plain.drain(..count);
                return Ok(count);
            }
            match self.poll_record() {
                Ok(true) => continue,
                Ok(false) => {}
                Err(message) => return Err(io::Error::new(io::ErrorKind::InvalidData, message)),
            }
            let mut chunk = [0u8; 8192];
            let read = self.inner.read(&mut chunk)?;
            if read == 0 {
                return Ok(0);
            }
            self.raw.extend_from_slice(&chunk[..read]);
        }
    }
}

impl Write for RealityStream {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut out = Vec::with_capacity(data.len() + 32);
        for chunk in data.chunks(MAX_RECORD) {
            self.write_keys
                .seal(CONTENT_APPLICATION_DATA, chunk, &mut out)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        }
        self.inner.write_all(&out)?;
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Stream for RealityStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_read_timeout(timeout)
    }
}

/// 拨号入口：在已经连通的裸 TCP 上做 REALITY 握手，返回之后的字节流。
pub fn connect(mut inner: BoxStream, node: &Node) -> Result<BoxStream, String> {
    let reality = node
        .reality
        .as_ref()
        .ok_or_else(|| err("REALITY: 节点没有 reality 参数"))?;
    // REALITY 与其它「再套一层」的传输互斥：上游也没有这种组合（ws over reality 属于未定义）。
    if node.ws.is_some() || node.http.is_some() || node.extra.is_some() {
        return Err(err("REALITY: 只支持裸 TCP（不能与 ws / http / h2 叠用）"));
    }
    let (read_keys, write_keys) = handshake(&mut inner, node, reality)?;
    Ok(Box::new(RealityStream {
        inner,
        read_keys,
        write_keys,
        raw: Vec::new(),
        plain: Vec::new(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_layout_and_offset() {
        let random = [7u8; 32];
        let key_share = [9u8; 32];
        let hello = build_client_hello(
            "example.com",
            &["h2".to_string()],
            &[(GROUP_X25519, key_share.to_vec())],
            &random,
        )
        .unwrap();
        assert_eq!(hello[0], HS_CLIENT_HELLO);
        // SessionId 的**长度字节**在 38（4 握手头 + 2 版本 + 32 random），内容从 39 开始
        assert_eq!(
            hello[SESSION_ID_OFFSET - 1],
            32,
            "SessionId 长度前缀必须是 32"
        );
        assert!(hello[SESSION_ID_OFFSET..SESSION_ID_OFFSET + 32]
            .iter()
            .all(|b| *b == 0));

        let plain = session_id_plain(&[0xaa, 0xbb], 1_700_000_000).unwrap();
        assert_eq!(&plain[..3], &CLIENT_VERSION);
        assert_eq!(plain[3], 0);
        assert_eq!(
            u32::from_be_bytes([plain[4], plain[5], plain[6], plain[7]]),
            1_700_000_000
        );
        assert_eq!(&plain[8..10], &[0xaa, 0xbb]);
        assert_eq!(&plain[10..], &[0u8; 6]);
        assert!(
            session_id_plain(&[0u8; 9], 0).is_err(),
            "short_id 最多 8 字节"
        );
    }

    #[test]
    fn seal_uses_zeroed_session_id_as_ad() {
        let auth = [3u8; 32];
        let random = [5u8; 32];
        let plain = [6u8; 16];
        let mut hello =
            build_client_hello("a.example", &[], &[(GROUP_X25519, vec![1u8; 32])], &random)
                .unwrap();
        let zeroed = hello.clone();
        let sealed = seal_session_id(&auth, &random, &plain, &zeroed).unwrap();
        assert_eq!(sealed.len(), 32, "16 字节明文 + 16 字节 GCM tag");

        // 用同一把钥匙、同一 nonce、同一 AAD 能解开 ⇒ AAD 就是「SessionId 全零」的那份
        let unbound = aead::UnboundKey::new(&aead::AES_256_GCM, &auth).unwrap();
        let key = aead::LessSafeKey::new(unbound);
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&random[20..]);
        let mut buffer = sealed.clone();
        let opened = key
            .open_in_place(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(&zeroed[..]),
                &mut buffer,
            )
            .unwrap();
        assert_eq!(opened, plain);

        // 改成「已写入密文」的 AAD 必须解不开（证明 AAD 不是随手一份）
        hello[SESSION_ID_OFFSET..SESSION_ID_OFFSET + 32].copy_from_slice(&sealed);
        let mut buffer2 = sealed;
        assert!(key
            .open_in_place(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(&hello[..]),
                &mut buffer2,
            )
            .is_err());
    }

    #[test]
    fn client_hello_declares_tls13_and_key_share() {
        let hello = build_client_hello(
            "node.example",
            &["h2".to_string()],
            &[(GROUP_X25519, vec![2u8; 32])],
            &[4u8; 32],
        )
        .unwrap();
        // supported_versions 扩展里必须有 0x0304（类型 2B + 扩展长度 2B + **列表长度 1B** + 版本 2B）
        let needle = [0x00u8, 0x2b, 0x00, 0x03, 0x02, 0x03, 0x04];
        assert!(
            hello.windows(needle.len()).any(|w| w == needle),
            "必须声明 TLS 1.3"
        );
        // key_share：类型 2B + 扩展长度 2B(0x26) + 向量长度 2B(0x24) + (组 2B + 长度 2B + 32B)
        let share = [0x00u8, 0x33, 0x00, 0x26, 0x00, 0x24, 0x00, 0x1d, 0x00, 0x20];
        assert!(hello.windows(share.len()).any(|w| w == share));
        // SNI 里能读到域名
        assert!(String::from_utf8_lossy(&hello).contains("node.example"));
    }

    /// Xray 26 的 REALITY 要求 `X25519MLKEM768` 排在最前，且数据 = 封装密钥(1184) ‖ x25519(32)。
    #[test]
    fn client_hello_carries_the_hybrid_post_quantum_share_first() {
        let x = vec![0x5au8; 32];
        let mut hybrid = vec![0x11u8; MLKEM768_EK];
        hybrid.extend_from_slice(&x);
        let hello = build_client_hello(
            "node.example",
            &[],
            &[
                (GROUP_X25519MLKEM768, hybrid.clone()),
                (GROUP_X25519, x.clone()),
            ],
            &[3u8; 32],
        )
        .unwrap();

        // supported_groups：列表长度 4，顺序 0x11ec → 0x001d
        let groups = [0x00u8, 0x0a, 0x00, 0x06, 0x00, 0x04, 0x11, 0xec, 0x00, 0x1d];
        assert!(
            hello.windows(groups.len()).any(|w| w == groups),
            "组顺序必须是混合组在前"
        );

        // key_share 第一条：0x11ec + 长度 1216，且紧跟着的 1184 字节是封装密钥
        let start = hello
            .windows(4)
            .position(|w| w == [0x11u8, 0xec, 0x04, 0xc0])
            .expect("key_share 里必须有 0x11ec / 1216 字节");
        assert_eq!(
            &hello[start + 4..start + 4 + MLKEM768_EK],
            &hybrid[..MLKEM768_EK],
            "封装密钥必须原样进 key_share"
        );
        assert_eq!(
            &hello[start + 4 + MLKEM768_EK..start + 4 + MLKEM768_EK + 32],
            &x[..],
            "x25519 公钥必须跟在封装密钥后面"
        );
    }

    #[test]
    fn expand_label_prefixes_tls13_and_carries_context() {
        let secret = [1u8; 32];
        let a = expand_label(&secret, "key", &[], 16).unwrap();
        let b = expand_label(&secret, "iv", &[], 12).unwrap();
        assert_eq!(a.len(), 16);
        assert_eq!(b.len(), 12);
        assert_ne!(a[..12], b, "不同 label 必须给出不同输出");
        // 长度写进 HkdfLabel 前两个字节：手工拼一份对照
        let mut info = Vec::new();
        info.extend_from_slice(&16u16.to_be_bytes());
        info.push(b"tls13 key".len() as u8);
        info.extend_from_slice(b"tls13 key");
        info.push(0);
        let prk = hkdf::Prk::new_less_safe(hkdf::HKDF_SHA256, &secret);
        struct L(usize);
        impl hkdf::KeyType for L {
            fn len(&self) -> usize {
                self.0
            }
        }
        let mut manual = [0u8; 16];
        prk.expand(&[&info], L(16))
            .unwrap()
            .fill(&mut manual)
            .unwrap();
        assert_eq!(a, manual.to_vec());
    }

    #[test]
    fn der_tlv_reads_long_form_length() {
        // 0x30 0x81 0x80 + 128 字节：长形式长度
        let mut der = vec![0x30, 0x81, 0x80];
        der.extend(std::iter::repeat(0x00).take(128));
        let (tag, start, len, next) = der_tlv(&der, 0).unwrap();
        assert_eq!((tag, start, len, next), (0x30, 3, 128, 131));
        // 截断必须报错而不是静默取 0
        assert!(der_tlv(&der[..100], 0).is_err());
        // 短形式
        let short = [0x02u8, 0x03, 1, 2, 3];
        let (tag, start, len, next) = der_tlv(&short, 0).unwrap();
        assert_eq!((tag, start, len, next), (0x02, 2, 3, 5));
    }

    /// REALITY 的临时证书必须是「Ed25519 公钥 + HMAC-SHA512(AuthKey, 公钥)」。
    ///
    /// 这里手工拼一份**最小但结构合法**的 X.509（不需要真签名 —— REALITY 认的是 HMAC），
    /// 用它证明：① DER 解析能拿到公钥与签名位；② 用对应 AuthKey 能验通，换一把就失败。
    #[test]
    fn temporary_certificate_is_accepted_only_with_the_right_auth_key() {
        /// DER 包裹：内容 < 128 字节时用短形式，否则用长形式（本用例里外层会超过 127）。
        fn wrap(tag: u8, contents: &[u8]) -> Vec<u8> {
            let mut out = vec![tag];
            if contents.len() < 0x80 {
                out.push(contents.len() as u8);
            } else {
                out.push(0x81);
                out.push(contents.len() as u8);
            }
            out.extend_from_slice(contents);
            out
        }

        let public_key = [0x11u8; 32];
        let auth = [0x22u8; 32];
        let hmac_key = hmac::Key::new(hmac::HMAC_SHA512, &auth);
        let signature = hmac::sign(&hmac_key, &public_key).as_ref().to_vec();

        // SPKI = SEQUENCE{ SEQUENCE{ OID 1.3.101.112 }, BIT STRING{ 0x00, 公钥 } }
        let alg = wrap(0x30, &[0x06, 0x03, 0x2b, 0x65, 0x70]);
        let mut bit_string = vec![0x00];
        bit_string.extend_from_slice(&public_key);
        let mut spki_body = alg.clone();
        spki_body.extend_from_slice(&wrap(0x03, &bit_string));
        let spki = wrap(0x30, &spki_body);

        // TBSCertificate：版本 [0] + serial / signature / issuer / validity / subject + SPKI
        let mut tbs_body = vec![0xa0, 0x03, 0x02, 0x01, 0x02];
        tbs_body.extend_from_slice(&[0x02, 0x01, 0x01]);
        tbs_body.extend_from_slice(&alg);
        tbs_body.extend_from_slice(&wrap(0x30, &[]));
        tbs_body.extend_from_slice(&wrap(0x30, &[]));
        tbs_body.extend_from_slice(&wrap(0x30, &[]));
        tbs_body.extend_from_slice(&spki);
        let tbs = wrap(0x30, &tbs_body);

        let mut cert_body = tbs;
        cert_body.extend_from_slice(&alg);
        let mut sig_bits = vec![0x00]; // unused bits
        sig_bits.extend_from_slice(&signature);
        cert_body.extend_from_slice(&wrap(0x03, &sig_bits));
        let cert = wrap(0x30, &cert_body);

        assert_eq!(
            cert_public_key_and_signature(&cert).unwrap(),
            (public_key.to_vec(), signature)
        );
        verify_temporary_certificate(&cert, &auth).expect("正确 AuthKey 必须验通");
        assert!(
            verify_temporary_certificate(&cert, &[0x33u8; 32]).is_err(),
            "换钥匙必须失败"
        );
    }

    #[test]
    fn leaf_certificate_skips_context_and_reads_first_entry() {
        let mut body = vec![0u8];
        body.extend_from_slice(&[0x00, 0x00, 0x03]);
        body.extend_from_slice(&[0x00, 0x00, 0x02, 0xaa, 0xbb]);
        body.extend_from_slice(&[0x00, 0x00, 0x00]); // 第二条（空）
        assert_eq!(leaf_certificate(&body).unwrap(), vec![0xaa, 0xbb]);
    }

    #[test]
    fn server_hello_parsing_rejects_unknown_group_and_missing_share() {
        // legacy 0x0303 + random(32) + sid 长度 0 + 套件 0x1301 + 压缩 0 + 扩展
        let mut body = vec![0x03, 0x03];
        body.extend_from_slice(&[0u8; 32]);
        body.push(0);
        body.extend_from_slice(&[0x13, 0x01, 0x00]);
        let mut exts = Vec::new();
        exts.extend_from_slice(&[0x00, 0x33, 0x00, 0x24, 0x00, 0x1d, 0x00, 0x20]);
        exts.extend_from_slice(&[0x09u8; 32]);
        body.extend_from_slice(&(exts.len() as u16).to_be_bytes());
        body.extend_from_slice(&exts);
        let (suite, group, share, _) = parse_server_hello(&body).unwrap();
        assert_eq!(suite, 0x1301);
        assert_eq!(group, GROUP_X25519);
        assert_eq!(share, vec![0x09u8; 32]);

        // 换成 secp256r1（0x0017）必须如实拒绝（扩展 = 类型2+长度2+组2+密钥长度2+32）
        let mut bad = body.clone();
        let pos = bad.len() - 36;
        bad[pos] = 0x00;
        bad[pos + 1] = 0x17;
        assert!(parse_server_hello(&bad).is_err());

        // 没有 key_share
        let mut none = vec![0x03, 0x03];
        none.extend_from_slice(&[0u8; 32]);
        none.push(0);
        none.extend_from_slice(&[0x13, 0x01, 0x00, 0x00, 0x00]);
        assert!(parse_server_hello(&none).is_err());
    }

    /// **真网**验证：对一台普通 TLS 1.3 服务器走完整握手，证书检查必须**如实失败**。
    ///
    /// 这条用例证明的是「我们的 TLS 1.3 客户端能握手成功」：只要握手、密钥派生、Finished
    /// 校验、证书解析任何一步写错，就根本走不到 REALITY 的证书判定；走到那一步才说明
    /// 前面的实现对得上。取证方式与 hysteria2 的真机用例一致 —— 需要外网，默认 `ignore`。
    #[test]
    #[ignore = "需要外网 TLS 1.3 服务器（人工 / CI 外网环境下用 --ignored 跑）"]
    fn real_tls13_server_completes_handshake_then_fails_the_reality_check() {
        let server = "www.cloudflare.com";
        let tcp = super::super::transport::connect_tcp(
            server,
            443,
            super::super::transport::DIAL_TIMEOUT,
        )
        .expect("连不上外网");
        let node = Node {
            name: "probe".into(),
            server: server.into(),
            port: 443,
            protocol: super::super::Protocol::Trojan {
                password: "x".into(),
            },
            tls: super::super::TlsOptions {
                enabled: true,
                sni: server.into(),
                allow_insecure: true,
                alpn: Vec::new(),
                ca: String::new(),
            },
            ws: None,
            http: None,
            extra: None,
            reality: Some(super::super::RealityOptions {
                public_key: vec![0x42u8; 32],
                short_id: vec![0x01u8; 8],
                spider_x: String::new(),
            }),
            mux: false,
        };
        let result = connect(Box::new(tcp), &node);
        let message = match result {
            Ok(_) => panic!("普通 TLS 服务器不可能通过 REALITY 的证书校验"),
            Err(message) => message,
        };
        assert!(
            message.contains("REALITY"),
            "失败必须落在 REALITY 的证书判定上（说明 TLS 1.3 握手本身通了），实际：{message}"
        );
    }
}
