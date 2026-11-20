//! QUIC 系协议（Hysteria2 / TUIC）共用的运行时、TLS 配置与错误翻译。
//!
//! 三个东西只允许一份实现（`forbidden.md` §禁用 62「同一个值不许两份实现」）：
//!   · **进程内共享的 tokio runtime** —— QUIC 必须先有异步运行时，而本目录其余协议
//!     都是 std 阻塞 socket；两个 QUIC 协议各建一个 runtime 只会白占线程；
//!   · **`insecure` / 系统根证书两种 TLS 1.3 配置** —— 两者的区别只在 ALPN，
//!     「跳过证书校验」的实现必须逐字一致，不能一边校验签名一边不校验；
//!   · **quinn 错误 → 人话** —— 服务端拒绝时最有价值的是应用层关闭码 + 原因串
//!     （`forbidden.md` §禁用 59：原始错误不进界面）。
//!
//! 各协议自己保留协议级文案（如「Hysteria2 的 QUIC 握手失败」），这里只给
//! 「应用层关闭（错误码 N：原因）」这一层。

use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::{Arc, OnceLock};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};

/// 所有 QUIC 连接共用的事件循环（2 个 worker 足够：IO 全在 quinn 里）。
///
/// 注意：这个函数**只在非 runtime 线程**上 `block_on`（`hysteria2.rs` / `tuic.rs`
/// 的阻塞外壳都是被 `server.rs` 的透传线程调用）。
pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("lunac-quic")
            .build()
            .expect("创建 QUIC 运行时失败")
    })
}

/// 解析节点地址（取第一个结果；QUIC 节点没有「多 IP 轮换」这一层）。
pub fn resolve(host: &str, port: u16) -> Result<SocketAddr, String> {
    (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("解析 {host}:{port} 失败：{e}"))?
        .next()
        .ok_or_else(|| format!("{host}:{port} 没有可用地址"))
}

/// QUIC 用的 rustls 配置：只允许 TLS 1.3（QUIC 的硬要求），ALPN 由调用方给。
///
/// `insecure` 是自签证书节点的唯一可行方式；不跳过时用系统根（webpki-roots）校验。
pub fn client_tls(insecure: bool, alpn: &[Vec<u8>]) -> Result<rustls::ClientConfig, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| format!("QUIC 只支持 TLS 1.3：{e}"))?;
    let mut tls = if insecure {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(SkipVerify))
            .with_no_client_auth()
    } else {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        builder.with_root_certificates(roots).with_no_client_auth()
    };
    tls.alpn_protocols = alpn.to_vec();
    Ok(tls)
}

/// `insecure=1` / `skip-cert-verify` 时的证书校验器：全部放行。
#[derive(Debug)]
struct SkipVerify;

impl ServerCertVerifier for SkipVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ECDSA_NISTP521_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ED25519,
        ]
    }
}

/// quinn 的连接级错误 → 一句人话。
pub fn connection_error_text(error: &quinn::ConnectionError) -> String {
    use quinn::ConnectionError as C;
    let reason = |bytes: &bytes::Bytes| String::from_utf8_lossy(bytes).trim().to_string();
    match error {
        C::ApplicationClosed(close) => {
            let code = u64::from(close.error_code);
            let reason = reason(&close.reason);
            if reason.is_empty() {
                format!("应用层关闭（错误码 {code}）")
            } else {
                format!("应用层关闭（错误码 {code}：{reason}）")
            }
        }
        C::ConnectionClosed(close) => {
            let reason = reason(&close.reason);
            if reason.is_empty() {
                format!("传输层关闭（错误码 {:#x}）", u64::from(close.error_code))
            } else {
                format!(
                    "传输层关闭（错误码 {:#x}：{reason}）",
                    u64::from(close.error_code)
                )
            }
        }
        C::TransportError(error) => format!("传输层错误（{error}）"),
        C::TimedOut => "空闲超时（对端长时间没有回包）".to_string(),
        C::VersionMismatch => "QUIC 版本不匹配".to_string(),
        C::Reset => "连接被对端重置".to_string(),
        C::LocallyClosed => "本地已关闭".to_string(),
        C::CidsExhausted => "连接 ID 用尽".to_string(),
    }
}

/// 流读错误 → 一句人话（`connection lost` 只是外层包装，真正原因在 `ConnectionError` 里）。
pub fn read_error_text(error: &quinn::ReadError) -> String {
    use quinn::ReadError as E;
    match error {
        E::ConnectionLost(inner) => format!("QUIC 连接已断开：{}", connection_error_text(inner)),
        E::Reset(code) => format!("对端重置了这条流（错误码 {}）", u64::from(*code)),
        E::ClosedStream => "流已经关闭".to_string(),
        E::IllegalOrderedRead => "同一条流上重复读".to_string(),
        E::ZeroRttRejected => "0-RTT 被拒绝".to_string(),
    }
}

/// `read_exact` 只会多包一层，真正的原因还是 `ReadError`。
pub fn read_exact_error_text(error: &quinn::ReadExactError) -> String {
    match error {
        quinn::ReadExactError::FinishedEarly(read) => format!("对端提前关流（只读到 {read} 字节）"),
        quinn::ReadExactError::ReadError(inner) => read_error_text(inner),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_close_keeps_the_server_reason() {
        let close = quinn::ApplicationClose {
            error_code: quinn::VarInt::from_u32(0),
            reason: bytes::Bytes::from_static(b"authentication: token mismatch"),
        };
        let text = connection_error_text(&quinn::ConnectionError::ApplicationClosed(close));
        assert!(text.contains("token mismatch"), "{text}");
        assert!(text.contains("错误码 0"), "{text}");
    }

    #[test]
    fn tls_config_uses_tls13_and_the_requested_alpn() {
        let tls = client_tls(true, &[b"h3".to_vec()]).expect("配置必须可用");
        assert_eq!(tls.alpn_protocols, vec![b"h3".to_vec()]);
    }
}
