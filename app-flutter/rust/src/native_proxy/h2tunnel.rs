//! HTTP/2 系传输：`network: h2` 与 `network: grpc`（重建 v1 `native_proxy` 的这两个传输）。
//!
//! **为什么单开一个文件**：v2 其余传输（TCP / TLS / WS / HTTP 混淆）都是 std 阻塞 socket
//! 上的「套一层字节流」，而 HTTP/2 必须有异步运行时。这里与 `hysteria2.rs` 同一套做法：
//! 共享一个 2 线程 tokio runtime，把 `h2::client` 的读写各 `block_on` 一小段，
//! 对外仍是 `transport::BoxStream`（`Read + Write + Send` + `set_read_timeout`）——
//! 于是 `server.rs::pipe()` 与各协议握手（trojan / vless / vmess）一行都不用改。
//!
//! 协议形态（与 Xray / v2ray 的 `network: h2` / `grpc` / `xhttp` 同口径）：
//!   · `h2`   —— `POST https://<authority><path>`，请求体 / 响应体就是裸字节；
//!   · `grpc` —— `POST https://<authority>/<serviceName>/Tun`，`content-type: application/grpc`、
//!     `te: trailers`；**每条消息 = 5 字节 gRPC 头 + `Hunk{data}` protobuf**（`0A <varint 长度> <数据>`，
//!     2026-11-15 真机校正：发裸字节会被 Xray 判 `cannot parse invalid wire-format data`）；
//!     服务端回包同理（`grpc_frame` / `grpc_deframe` / `hunk_encode` / `hunk_decode`）；
//!     响应头**推迟到第一次读**再取（grpc-go 在收到首个消息前不发响应头，否则互相等死）；
//!   · `xhttp`（SplitHTTP）—— `stream-one` = 一条 `POST <path>/` 双向承载；`stream-up` =
//!     `POST <path>/<uuid>` 上传 + `GET <path>/<uuid>` 下载（同一连接）；每个请求都必须带
//!     `Referer: https://<authority><path>?x_padding=<100..1000 个 X>`，否则服务端 400。
//!     `packet-up` 未做（按 `stream-one` 发，与 v1 同口径）。
//!
//! **窗口与背压**：h2 的流控必须自己搬 —— 写侧要先 `reserve_capacity(n)` 再 `poll_capacity`
//! 等额度（**顺序不能反**：没申请额度时 `capacity()` 恒为 0、`poll_capacity` 也不会被唤醒，
//! 表现就是「第一次写就挂住 30 秒」），读到的每块都要 `release_capacity`，否则对端会卡在
//! 窗口上（表现为「大文件传到一半不动」）。
//!
//! **如实登记的边界**：只做「一条流承载整条代理连接」（Xray 的 standard 模式）；
//! 不做 gRPC 的多路复用（`grpc` 的 multi-mode / `h2` 的 mux.cool）与 XHTTP 的 `xmux`。

use std::cell::Cell;
use std::io::{self, Read, Write};

// XHTTP 的用例单独成模块（文件末尾的 `tests` 只覆盖 h2 / gRPC 的回环）。
#[cfg(test)]
mod xhttp_tests {
    use super::*;
    use std::net::SocketAddr;

    /// XHTTP 的路径 / padding / session 规则（Xray `GetNormalizedPath` 与 `uuid.New()`）。
    #[test]
    fn xhttp_parts_and_padding_follow_xray_rules() {
        assert_eq!(xhttp_parts("/x"), ("/x/".to_string(), String::new()));
        assert_eq!(
            xhttp_parts("x/y?ed=2048"),
            ("/x/y/".to_string(), "ed=2048".to_string())
        );
        assert_eq!(xhttp_parts("/"), ("/".to_string(), String::new()));
        assert_eq!(xhttp_parts(""), ("/".to_string(), String::new()));

        for _ in 0..50 {
            let padding = xhttp_padding();
            assert!(
                (XHTTP_PADDING_MIN..=XHTTP_PADDING_MAX).contains(&padding.len()),
                "padding 长度 {} 必须落在 [{XHTTP_PADDING_MIN}, {XHTTP_PADDING_MAX}]",
                padding.len()
            );
            assert!(padding.chars().all(|c| c == 'X'), "repeat-x 只允许 X");
        }

        let session = xhttp_session_id();
        assert_eq!(session.len(), 36, "必须是带连字符的 UUID");
        let widths: Vec<usize> = session.split('-').map(str::len).collect();
        assert_eq!(widths, vec![8, 4, 4, 4, 12]);
        assert_eq!(&session[14..15], "4", "版本位必须是 4（UUID v4）");
        assert!(
            matches!(&session[19..20], "8" | "9" | "a" | "b"),
            "variant 位必须在 [89ab]"
        );
    }

    /// 从请求的 `Referer` 里取出 `x_padding` 的长度（Xray 服务端 `IsPaddingValid` 同口径）。
    fn referer_padding(request: &http::Request<h2::RecvStream>) -> usize {
        let referer = request
            .headers()
            .get("referer")
            .expect("每个 XHTTP 请求都必须带 Referer")
            .to_str()
            .expect("referer 必须是可见字符");
        assert!(
            referer.starts_with("https://"),
            "referer 必须是绝对 URL：{referer}"
        );
        referer
            .split("x_padding=")
            .nth(1)
            .expect("referer 的 query 里必须有 x_padding")
            .len()
    }

    fn assert_padding_in_range(length: usize) {
        assert!(
            (XHTTP_PADDING_MIN..=XHTTP_PADDING_MAX).contains(&length),
            "x_padding 长度 {length} 超出 [{XHTTP_PADDING_MIN}, {XHTTP_PADDING_MAX}]，服务端会 400"
        );
    }

    /// `stream-one`：一条 POST 双向承载（服务端模拟 Xray：校验 padding / 路径 / 头，再回显）。
    #[test]
    fn xhttp_stream_one_echoes_with_padding() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = runtime().block_on(async move {
            tokio::net::TcpListener::bind(addr)
                .await
                .expect("绑本地端口")
        });
        let local = listener.local_addr().expect("取本地地址");
        let server = runtime().spawn(async move {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            let Ok(mut connection) = h2::server::handshake(socket).await else {
                return;
            };
            while let Some(accepted) = connection.accept().await {
                let Ok((request, mut respond)) = accepted else {
                    break;
                };
                assert_eq!(request.method(), Method::POST);
                assert_eq!(request.uri().path(), "/x/", "路径要补尾斜杠");
                assert_eq!(request.uri().query(), Some("ed=2048"));
                assert_eq!(
                    request.headers().get("content-type").unwrap(),
                    "application/grpc"
                );
                assert_padding_in_range(referer_padding(&request));
                tokio::spawn(async move {
                    let mut body = request.into_body();
                    let response = http::Response::builder()
                        .status(200)
                        .body(())
                        .expect("响应");
                    let Ok(mut send) = respond.send_response(response, false) else {
                        return;
                    };
                    while let Some(chunk) = body.data().await {
                        let Ok(chunk) = chunk else { break };
                        let _ = body.flow_control().release_capacity(chunk.len());
                        if send.send_data(chunk, false).is_err() {
                            return;
                        }
                    }
                    let _ = send.send_data(Bytes::new(), true);
                });
            }
        });

        let mut stream = runtime().block_on(async move {
            let tcp = tokio::net::TcpStream::connect(local).await.expect("连本地");
            tunnel_over(
                tcp,
                format!("127.0.0.1:{}", local.port()),
                ExtraTransport::XHttp {
                    host: String::new(),
                    path: "/x?ed=2048".into(),
                    mode: "stream-one".into(),
                },
            )
            .await
            .expect("建 xhttp 隧道")
        });
        stream.write_all(b"hello-xhttp").expect("写");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("设读超时");
        let mut buffer = [0u8; 32];
        let read = stream.read(&mut buffer).expect("读回显");
        assert_eq!(&buffer[..read], b"hello-xhttp");
        drop(stream);
        server.abort();
    }

    /// `stream-up`：`POST <path>/<session>` 上传 + `GET <path>/<session>` 下载（同一连接）。
    #[test]
    fn xhttp_stream_up_splits_upload_and_download() {
        use std::sync::{Arc, Mutex};

        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = runtime().block_on(async move {
            tokio::net::TcpListener::bind(addr)
                .await
                .expect("绑本地端口")
        });
        let local = listener.local_addr().expect("取本地地址");
        let uploaded: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let state = uploaded.clone();
        let server = runtime().spawn(async move {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            let Ok(mut connection) = h2::server::handshake(socket).await else {
                return;
            };
            while let Some(accepted) = connection.accept().await {
                let Ok((request, mut respond)) = accepted else {
                    break;
                };
                let path = request.uri().path().to_string();
                assert!(path.starts_with("/x/"), "session 必须落在路径上：{path}");
                let session = path.trim_start_matches("/x/").to_string();
                assert_eq!(session.len(), 36, "session 必须是 UUID：{session}");
                assert_padding_in_range(referer_padding(&request));
                let state = state.clone();
                if request.method() == Method::POST {
                    assert_eq!(
                        request.headers().get("content-type").unwrap(),
                        "application/grpc"
                    );
                    tokio::spawn(async move {
                        // 服务端先回 200，再读上传流（与 Xray 的顺序一致）。
                        let response = http::Response::builder()
                            .status(200)
                            .body(())
                            .expect("响应");
                        let Ok(_send) = respond.send_response(response, false) else {
                            return;
                        };
                        let mut body = request.into_body();
                        while let Some(chunk) = body.data().await {
                            let Ok(chunk) = chunk else { break };
                            let _ = body.flow_control().release_capacity(chunk.len());
                            state.lock().unwrap().extend_from_slice(&chunk);
                        }
                        // `_send` 不主动结束：上传流由客户端关；这里只要别 drop 掉 RecvStream 之外的东西。
                    });
                } else {
                    assert_eq!(request.method(), Method::GET);
                    assert!(
                        request.headers().get("content-type").is_none(),
                        "下载流不该带 content-type"
                    );
                    tokio::spawn(async move {
                        let mut body = request.into_body();
                        let response = http::Response::builder()
                            .status(200)
                            .body(())
                            .expect("响应");
                        let Ok(mut send) = respond.send_response(response, false) else {
                            return;
                        };
                        // 等上传流把数据写进共享缓冲（最多 5s），再回显后结束。
                        let deadline = std::time::Instant::now() + Duration::from_secs(5);
                        loop {
                            let ready = !state.lock().unwrap().is_empty();
                            if ready || std::time::Instant::now() > deadline {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                        let payload = state.lock().unwrap().clone();
                        let _ = body.data().await;
                        let _ = send.send_data(Bytes::from(payload), true);
                    });
                }
            }
        });

        let mut stream = runtime().block_on(async move {
            let tcp = tokio::net::TcpStream::connect(local).await.expect("连本地");
            tunnel_over(
                tcp,
                format!("127.0.0.1:{}", local.port()),
                ExtraTransport::XHttp {
                    host: String::new(),
                    path: "x".into(),
                    mode: "stream-up".into(),
                },
            )
            .await
            .expect("建 xhttp 隧道")
        });
        stream.write_all(b"up-and-down").expect("写");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("设读超时");
        let mut buffer = [0u8; 32];
        let read = stream.read(&mut buffer).expect("读回显");
        assert_eq!(&buffer[..read], b"up-and-down");
        drop(stream);
        server.abort();
    }
}

use std::net::ToSocketAddrs;
use std::sync::OnceLock;
use std::time::Duration;

use bytes::Bytes;
use http::{Method, Request};

use super::transport::{BoxStream, Stream};
use super::{ExtraTransport, Node};

/// HTTP/2 的 ALPN 标识（服务端只认这个）。
const ALPN_H2: &str = "h2";
/// TCP 拨号超时（与 `transport::DIAL_TIMEOUT` 同量级）。
const DIAL_TIMEOUT: Duration = Duration::from_secs(12);
/// gRPC 消息头的长度：1 字节压缩标志 + 4 字节大端长度。
const GRPC_HEADER: usize = 5;
/// XHTTP 的 padding 长度范围（Xray `GetNormalizedXPaddingBytes` 的缺省 `[100, 1000]`）。
const XHTTP_PADDING_MIN: usize = 100;
const XHTTP_PADDING_MAX: usize = 1000;

/// 所有 h2 / grpc 连接共用的 tokio runtime（QUIC 那个在 `hysteria2.rs`，互不影响）。
fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("lunac-h2")
            .build()
            .expect("创建 HTTP/2 运行时失败")
    })
}

/// 建一条 h2 / grpc 隧道（对外是阻塞流）。
pub fn connect(node: &Node, extra: &ExtraTransport) -> Result<BoxStream, String> {
    let stream = runtime().block_on(connect_async(node, extra))?;
    Ok(Box::new(stream))
}

async fn connect_async(node: &Node, extra: &ExtraTransport) -> Result<H2Stream, String> {
    let addr = (node.server.as_str(), node.port)
        .to_socket_addrs()
        .map_err(|e| format!("解析节点 {}:{} 失败：{e}", node.server, node.port))?
        .next()
        .ok_or_else(|| format!("节点 {}:{} 没有可用地址", node.server, node.port))?;
    let tcp = tokio::time::timeout(DIAL_TIMEOUT, tokio::net::TcpStream::connect(addr))
        .await
        .map_err(|_| format!("连接节点 {addr} 超时（{DIAL_TIMEOUT:?}）"))?
        .map_err(|e| format!("连接节点 {addr} 失败：{e}"))?;
    let _ = tcp.set_nodelay(true);

    // h2 的 ALPN 必须协商出 `h2`：TLS 配置单独构造（不复用 `transport::wrap_tls`，
    // 那个是 native-tls 同步版，拿不到 tokio 的异步流）。
    if node.tls.enabled {
        let mut builder = native_tls::TlsConnector::builder();
        builder.request_alpns(&[ALPN_H2]);
        if node.tls.allow_insecure {
            builder.danger_accept_invalid_certs(true);
            builder.danger_accept_invalid_hostnames(true);
        }
        let connector = tokio_native_tls::TlsConnector::from(
            builder
                .build()
                .map_err(|e| format!("h2 的 TLS 初始化失败：{e}"))?,
        );
        let sni = if node.tls.sni.trim().is_empty() {
            node.server.clone()
        } else {
            node.tls.sni.clone()
        };
        let tls = connector
            .connect(&sni, tcp)
            .await
            .map_err(|e| format!("h2 的 TLS 握手失败（{sni}）：{e}"))?;
        let authority = authority_for(node, extra);
        return tunnel_over(tls, authority, extra.clone()).await;
    }
    // 明文 h2（h2c prior knowledge）：本引擎只在对端明确没开 TLS 时走这条路。
    let authority = authority_for(node, extra);
    tunnel_over(tcp, authority, extra.clone()).await
}

/// `:authority`：h2 用配置里的 `host`（缺省 = 节点地址），grpc 用节点地址。
fn authority_for(node: &Node, extra: &ExtraTransport) -> String {
    match extra {
        ExtraTransport::H2 { host, .. } if !host.trim().is_empty() => host.trim().to_string(),
        _ => format!("{}:{}", node.server, node.port),
    }
}

/// 在任意异步字节流上建 h2 隧道（拆出来是为了单测能用明文 TCP 回环）。
async fn tunnel_over<S>(io: S, authority: String, extra: ExtraTransport) -> Result<H2Stream, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut client, connection) = tokio::time::timeout(DIAL_TIMEOUT, h2::client::handshake(io))
        .await
        .map_err(|_| format!("h2 握手超时（{DIAL_TIMEOUT:?}）"))?
        .map_err(|e| format!("h2 握手失败：{e}"))?;
    // 连接驱动必须常驻（它负责读写整条 TCP 上的帧）。
    tokio::spawn(async move {
        let _ = connection.await;
    });

    // XHTTP 用流的方式与 h2 / grpc 不同（可能要两条流），单独走。
    if let ExtraTransport::XHttp { path, mode, .. } = &extra {
        return open_xhttp(&mut client, &authority, path, mode).await;
    }

    let (path, grpc) = match &extra {
        ExtraTransport::H2 { path, .. } => {
            let path = if path.trim().is_empty() {
                "/".to_string()
            } else {
                path.clone()
            };
            (path, false)
        }
        ExtraTransport::Grpc { service_name } => {
            let service = service_name.trim_matches('/');
            let service = if service.is_empty() {
                "GunService"
            } else {
                service
            };
            (format!("/{service}/Tun"), true)
        }
        ExtraTransport::XHttp { .. } => unreachable!("XHTTP 已在上面单独处理"),
    };

    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(format!("https://{authority}{path}"));
    if grpc {
        builder = builder
            .header("content-type", "application/grpc")
            .header("te", "trailers");
    }
    let request = builder
        .body(())
        .map_err(|e| format!("h2 请求构造失败：{e}"))?;

    let (response, send) = client
        .send_request(request, false)
        .map_err(|e| format!("h2 发请求失败：{e}"))?;
    if grpc {
        // Xray / grpc-go 的服务端在**收到首个消息之前不会发响应头**（handler 先把
        // header 挂着，等有数据或结束时才 flush）。这里若先等响应头就会与对端互相
        // 等死 —— 实测表现就是「等响应头 12s 超时」。所以把响应头推迟到第一次读。
        return Ok(H2Stream::deferred(send, response));
    }
    let recv = await_ok(response, extra.kind()).await?;
    Ok(H2Stream::new(send, recv, grpc))
}

/// 等响应头，非 200 视为服务端拒绝（错误里带上是哪条传输）。
async fn await_ok(
    response: h2::client::ResponseFuture,
    what: &str,
) -> Result<h2::RecvStream, String> {
    let response = tokio::time::timeout(DIAL_TIMEOUT, response)
        .await
        .map_err(|_| format!("{what} 等响应头超时（{DIAL_TIMEOUT:?}）"))?
        .map_err(|e| format!("{what} 读响应头失败：{e}"))?;
    if response.status() != http::StatusCode::OK {
        return Err(format!(
            "{what} 服务端拒绝了请求：HTTP {}",
            response.status().as_u16()
        ));
    }
    Ok(response.into_body())
}

/// XHTTP 的路径规范化（与 Xray `GetNormalizedPath` / `GetNormalizedQuery` 同规则）：
/// 拆出 `?query`，补前导 `/` 与结尾 `/`。返回 `(规范化路径, 原样 query)`。
pub fn xhttp_parts(path: &str) -> (String, String) {
    let (raw, query) = match path.split_once('?') {
        Some((path, query)) => (path, query.to_string()),
        None => (path, String::new()),
    };
    let mut normalized = if raw.starts_with('/') {
        raw.to_string()
    } else {
        format!("/{raw}")
    };
    if !normalized.ends_with('/') {
        normalized.push('/');
    }
    (normalized, query)
}

/// 每个 XHTTP 请求都要带的 `x_padding`：`[100, 1000]` 个 `X`（`repeat-x`，Xray 的缺省方法）。
///
/// 服务端 `IsPaddingValid` 会校验长度，**不发或长度不对直接 400**，所以这里不允许失败兜底成空串。
pub fn xhttp_padding() -> String {
    let mut seed = [0u8; 2];
    let span = XHTTP_PADDING_MAX - XHTTP_PADDING_MIN + 1;
    let length = match getrandom::getrandom(&mut seed) {
        Ok(()) => XHTTP_PADDING_MIN + (u16::from_be_bytes(seed) as usize % span),
        // 取不到随机数就取区间中值：长度仍合法，只是不再随机。
        Err(_) => (XHTTP_PADDING_MIN + XHTTP_PADDING_MAX) / 2,
    };
    "X".repeat(length)
}

/// `stream-up` 的会话号：带连字符的 UUID v4（Xray 用 `uuid.New().String()`）。
pub fn xhttp_session_id() -> String {
    let mut bytes = [0u8; 16];
    let _ = getrandom::getrandom(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// 按 Xray `FillStreamRequest` 的口径拼一条 XHTTP 请求：
/// `Referer` 用 `x_padding` 占满 query（**替换**配置里的 query，与 `ApplyPaddingToQuery` 同口径），
/// 上行（`upload`）才带 `content-type: application/grpc`。
fn xhttp_request(
    authority: &str,
    path: &str,
    query: &str,
    upload: bool,
) -> Result<Request<()>, String> {
    let mut target = path.to_string();
    if !query.trim().is_empty() {
        target.push('?');
        target.push_str(query);
    }
    let referer = format!("https://{authority}{path}?x_padding={}", xhttp_padding());
    let mut builder = Request::builder()
        .method(if upload { Method::POST } else { Method::GET })
        .uri(format!("https://{authority}{target}"))
        .header("referer", referer);
    if upload {
        builder = builder.header("content-type", "application/grpc");
    }
    builder
        .body(())
        .map_err(|e| format!("xhttp 请求构造失败：{e}"))
}

/// 建 XHTTP 隧道：`stream-one` 一条 POST 双向；`stream-up` 上传 POST + 下载 GET（同一连接）。
///
/// **边界（与 v1 同口径）**：`packet-up` 未实现 —— 配置里写了也按 `stream-one` 发；
/// 服务端 `mode` 为空时三种都接受。也不做 Xray 的 `xmux` / 多路复用。
async fn open_xhttp(
    client: &mut h2::client::SendRequest<Bytes>,
    authority: &str,
    path: &str,
    mode: &str,
) -> Result<H2Stream, String> {
    let (base, query) = xhttp_parts(path);
    if mode.trim().eq_ignore_ascii_case("stream-up") {
        let target = format!("{base}{}", xhttp_session_id());
        let upload = xhttp_request(authority, &target, &query, true)?;
        let (upload_response, send) = client
            .send_request(upload, false)
            .map_err(|e| format!("xhttp 上传流发请求失败：{e}"))?;
        // 上传流的响应体是空的（服务端先回 200 再读请求体）。用后台任务把它读完 ——
        // 直接 drop `RecvStream` 会发 RST_STREAM，把上传流一起掐掉。
        tokio::spawn(async move {
            if let Ok(Ok(response)) = tokio::time::timeout(DIAL_TIMEOUT, upload_response).await {
                let mut body = response.into_body();
                while let Some(Ok(_)) = body.data().await {}
            }
        });
        let download = xhttp_request(authority, &target, &query, false)?;
        let (download_response, _) = client
            .send_request(download, true)
            .map_err(|e| format!("xhttp 下载流发请求失败：{e}"))?;
        let recv = await_ok(download_response, "xhttp 下载流").await?;
        return Ok(H2Stream::new(send, recv, false));
    }
    let request = xhttp_request(authority, &base, &query, true)?;
    let (response, send) = client
        .send_request(request, false)
        .map_err(|e| format!("xhttp 发请求失败：{e}"))?;
    let recv = await_ok(response, "xhttp").await?;
    Ok(H2Stream::new(send, recv, false))
}

/// gRPC 消息封装：`[0x00][u32 大端长度][正文]`。
pub fn grpc_frame(data: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(GRPC_HEADER + data.len());
    frame.push(0);
    frame.extend_from_slice(&(data.len() as u32).to_be_bytes());
    frame.extend_from_slice(data);
    frame
}

/// Xray `transport/internet/grpc/encoding` 的 protobuf 消息体：`Hunk { bytes data = 1; }`。
///
/// 每条 gRPC 消息的**正文**不是裸字节，而是一个 protobuf：`tag=0x0A` + varint 长度 + 数据。
/// 实测（2026-11-15，Xray 26）发裸字节会被服务端判 `cannot parse invalid wire-format data`。
fn hunk_encode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 6);
    out.push(0x0A);
    let mut len = data.len() as u64;
    loop {
        let mut byte = (len & 0x7F) as u8;
        len >>= 7;
        if len != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if len == 0 {
            break;
        }
    }
    out.extend_from_slice(data);
    out
}

/// 解一个 `Hunk`，返回（data, 消费的字节数）。不完整的输入 ⇒ `None`。
fn hunk_decode(body: &[u8]) -> Option<(Vec<u8>, usize)> {
    let mut offset = 0;
    while offset < body.len() {
        let tag = body[offset];
        offset += 1;
        match tag & 0x07 {
            0 => {
                let (_, used) = read_varint(body.get(offset..)?)?;
                offset += used;
            }
            1 => offset += 8,
            2 => {
                let (len, used) = read_varint(body.get(offset..)?)?;
                offset += used;
                let end = offset.checked_add(len as usize)?;
                let data = body.get(offset..end)?;
                return Some((data.to_vec(), end));
            }
            5 => offset += 4,
            _ => return None,
        }
    }
    Some((Vec::new(), offset))
}

fn read_varint(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    let mut shift = 0u32;
    for (index, byte) in bytes.iter().enumerate() {
        value |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Some((value, index + 1));
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
    None
}

/// 从 `input` 里拆出所有「完整」的 gRPC 消息正文，追加到 `output`；
/// 不完整的尾巴留在 `input` 里等下一块。返回拆出的条数。
pub fn grpc_deframe(input: &mut Vec<u8>, output: &mut Vec<u8>) -> usize {
    let mut count = 0;
    loop {
        if input.len() < GRPC_HEADER {
            break;
        }
        let len = u32::from_be_bytes([input[1], input[2], input[3], input[4]]) as usize;
        if input.len() < GRPC_HEADER + len {
            break;
        }
        output.extend_from_slice(&input[GRPC_HEADER..GRPC_HEADER + len]);
        input.drain(..GRPC_HEADER + len);
        count += 1;
    }
    count
}

/// 一条 h2 / grpc 隧道（HTTP/2 流的阻塞外壳）。
pub struct H2Stream {
    send: h2::SendStream<Bytes>,
    recv: Option<h2::RecvStream>,
    /// gRPC：响应头要等到第一次读才取（见 `tunnel_over` 里的注释）。
    pending_response: Option<h2::client::ResponseFuture>,
    read_timeout: Cell<Option<Duration>>,
    /// 已解帧、待交给上层的明文。
    pending: Vec<u8>,
    /// gRPC 模式下还没拆完的原始字节（含不完整的消息头）。
    raw: Vec<u8>,
    grpc: bool,
    finished: bool,
}

impl H2Stream {
    fn new(send: h2::SendStream<Bytes>, recv: h2::RecvStream, grpc: bool) -> Self {
        Self {
            send,
            recv: Some(recv),
            pending_response: None,
            read_timeout: Cell::new(None),
            pending: Vec::new(),
            raw: Vec::new(),
            grpc,
            finished: false,
        }
    }

    /// gRPC 专用：响应头还没读，留给第一次 `pull`。
    fn deferred(send: h2::SendStream<Bytes>, response: h2::client::ResponseFuture) -> Self {
        Self {
            send,
            recv: None,
            pending_response: Some(response),
            read_timeout: Cell::new(None),
            pending: Vec::new(),
            raw: Vec::new(),
            grpc: true,
            finished: false,
        }
    }

    /// 第一次读之前把响应头解析掉（同时校验 200）。
    fn ensure_recv(&mut self) -> io::Result<()> {
        let Some(response) = self.pending_response.take() else {
            return Ok(());
        };
        let resolved =
            runtime().block_on(async { tokio::time::timeout(DIAL_TIMEOUT, response).await });
        let response = match resolved {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("grpc 读响应头失败：{error}"),
                ))
            }
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("grpc 等响应头超时（{DIAL_TIMEOUT:?}）"),
                ))
            }
        };
        if response.status() != http::StatusCode::OK {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("grpc 服务端拒绝了请求：HTTP {}", response.status().as_u16()),
            ));
        }
        self.recv = Some(response.into_body());
        Ok(())
    }

    /// 把一段明文写进 h2 流：grpc 先套 `Hunk` protobuf + 5 字节消息头；随后按对端窗口分块搬。
    fn write_payload(&mut self, payload: Vec<u8>) -> io::Result<()> {
        if self.finished {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "h2 流已结束"));
        }
        let mut data = match self.grpc {
            true => Bytes::from(grpc_frame(&hunk_encode(&payload))),
            false => Bytes::from(payload),
        };
        while !data.is_empty() {
            // 顺序很重要：先向连接申请额度，再等额度。没申请时 `capacity()` 恒为 0，
            // 而 `poll_capacity` 也不会被唤醒（h2 的 `send_data` 文档同口径）。
            self.send.reserve_capacity(data.len());
            let polled = runtime().block_on(async {
                tokio::time::timeout(
                    Duration::from_secs(30),
                    std::future::poll_fn(|cx| self.send.poll_capacity(cx)),
                )
                .await
            });
            let capacity = match polled {
                Ok(Some(Ok(capacity))) => capacity,
                Ok(Some(Err(e))) => {
                    return Err(io::Error::new(io::ErrorKind::Other, e.to_string()))
                }
                Ok(None) => return Err(io::Error::new(io::ErrorKind::BrokenPipe, "h2 流已关闭")),
                Err(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "h2 写窗口 30 秒没有更新（对端不收数据）",
                    ))
                }
            };
            let take = capacity.min(data.len()).max(1);
            let chunk = data.split_to(take);
            self.send
                .send_data(chunk, false)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        }
        Ok(())
    }

    /// 拉一块原始字节（带读超时）。`Ok(None)` = 流结束。
    fn pull(&mut self) -> io::Result<Option<Vec<u8>>> {
        if self.finished {
            return Ok(None);
        }
        self.ensure_recv()?;
        let timeout = self.read_timeout.get();
        let recv = self.recv.as_mut().expect("ensure_recv 已填好 recv");
        let result: Option<Result<Option<Bytes>, h2::Error>> = runtime().block_on(async {
            let next = async {
                match recv.data().await {
                    Some(Ok(bytes)) => {
                        let _ = recv.flow_control().release_capacity(bytes.len());
                        Ok(Some(bytes))
                    }
                    Some(Err(error)) => Err(error),
                    None => Ok(None),
                }
            };
            match timeout {
                Some(limit) => tokio::time::timeout(limit, next).await.ok(),
                None => Some(next.await),
            }
        });
        match result {
            // 到期 = 「这一轮没数据」（与 `pipe()` 对 WouldBlock 的处理一致）。
            None => Err(io::Error::new(io::ErrorKind::WouldBlock, "读超时")),
            Some(Ok(Some(bytes))) => Ok(Some(bytes.to_vec())),
            Some(Ok(None)) => {
                self.finished = true;
                Ok(None)
            }
            Some(Err(error)) => Err(io::Error::new(io::ErrorKind::Other, error.to_string())),
        }
    }

    /// 把已到手的原始字节搬进 `pending`（gRPC 时先解 gRPC 消息头，再解 `Hunk`）。
    fn fill_pending(&mut self, chunk: Vec<u8>) -> io::Result<()> {
        if self.grpc {
            self.raw.extend_from_slice(&chunk);
            let mut bodies = Vec::new();
            grpc_deframe(&mut self.raw, &mut bodies);
            let mut offset = 0;
            while offset < bodies.len() {
                let Some((data, used)) = hunk_decode(&bodies[offset..]) else {
                    // 半个 protobuf：把剩余字节塞回 raw 前面等下一块（正常不出现）。
                    self.raw.splice(0..0, bodies[offset..].iter().copied());
                    break;
                };
                self.pending.extend_from_slice(&data);
                offset += used;
            }
        } else {
            self.pending.extend_from_slice(&chunk);
        }
        Ok(())
    }
}

impl Read for H2Stream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            if !self.pending.is_empty() {
                let take = buffer.len().min(self.pending.len());
                buffer[..take].copy_from_slice(&self.pending[..take]);
                self.pending.drain(..take);
                return Ok(take);
            }
            match self.pull()? {
                Some(chunk) => self.fill_pending(chunk)?,
                None => {
                    // 流结束了：gRPC 模式下若有残留（不完整的消息）也算结束。
                    return Ok(0);
                }
            }
        }
    }
}

impl Write for H2Stream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.write_payload(buffer.to_vec())?;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Stream for H2Stream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.read_timeout.set(timeout);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    #[test]
    fn grpc_messages_carry_a_hunk_protobuf_body() {
        assert_eq!(hunk_encode(b"hi"), vec![0x0A, 0x02, b'h', b'i']);
        assert_eq!(
            hunk_decode(&[0x0A, 0x02, b'h', b'i']),
            Some((b"hi".to_vec(), 4))
        );
        // 长度 ≥ 128 时 varint 占两字节。
        let big = vec![7u8; 300];
        let encoded = hunk_encode(&big);
        assert_eq!(&encoded[..3], &[0x0A, 0xAC, 0x02]);
        assert_eq!(hunk_decode(&encoded).unwrap().0, big);
        // 只给了半个正文 ⇒ 等下一块。
        assert!(hunk_decode(&[0x0A, 0x05, b'a']).is_none());
    }

    #[test]
    fn grpc_frame_has_the_five_byte_header() {
        let frame = grpc_frame(b"hi");
        assert_eq!(&frame[..5], &[0, 0, 0, 0, 2]);
        assert_eq!(&frame[5..], b"hi");
        // 空消息也要有头（gRPC 允许空 message）。
        assert_eq!(grpc_frame(b""), vec![0, 0, 0, 0, 0]);
    }

    #[test]
    fn grpc_deframe_round_trips_and_keeps_partial_frames() {
        let mut wire = Vec::new();
        wire.extend_from_slice(&grpc_frame(b"first"));
        wire.extend_from_slice(&grpc_frame(b""));
        wire.extend_from_slice(&grpc_frame(b"third"));
        // 切成两半喂进去：第一半拆不出完整的第三帧。
        let mut input = Vec::new();
        let mut output = Vec::new();
        input.extend_from_slice(&wire[..10]);
        let first = grpc_deframe(&mut input, &mut output);
        assert_eq!(first, 1, "第一帧完整、第二帧不完整");
        assert_eq!(output, b"first");
        input.extend_from_slice(&wire[10..]);
        let rest = grpc_deframe(&mut input, &mut output);
        assert_eq!(rest, 2);
        assert_eq!(output, b"firstthird");
        assert!(input.is_empty());
    }

    #[test]
    fn grpc_deframe_ignores_short_input() {
        let mut input = vec![0u8, 0, 0];
        let mut output = Vec::new();
        assert_eq!(grpc_deframe(&mut input, &mut output), 0);
        assert!(output.is_empty());
        assert_eq!(input.len(), 3, "不完整的头必须留着");
    }

    #[test]
    fn authority_prefers_h2_host_and_defaults_to_server() {
        let node = Node {
            name: "n".into(),
            server: "node.example.com".into(),
            port: 443,
            protocol: super::super::Protocol::Trojan {
                password: "p".into(),
            },
            tls: super::super::TlsOptions {
                enabled: true,
                ..Default::default()
            },
            ws: None,
            http: None,
            extra: None,
            reality: None,
            mux: false,
        };
        assert_eq!(
            authority_for(
                &node,
                &ExtraTransport::H2 {
                    host: "edge.example.com".into(),
                    path: "/".into()
                }
            ),
            "edge.example.com"
        );
        assert_eq!(
            authority_for(
                &node,
                &ExtraTransport::Grpc {
                    service_name: "gun".into()
                }
            ),
            "node.example.com:443"
        );
    }

    #[test]
    fn runtime_runs_spawned_tasks_with_time() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let flag = std::sync::Arc::new(AtomicBool::new(false));
        let worker = flag.clone();
        runtime().spawn(async move {
            worker.store(true, Ordering::SeqCst);
        });
        runtime().block_on(async {
            tokio::time::sleep(Duration::from_millis(200)).await;
        });
        assert!(flag.load(Ordering::SeqCst), "spawn 的任务必须真的跑起来");
    }

    /// 明文 h2 回环：服务端把请求体原样回写，客户端写什么就该读回什么。
    #[test]
    fn h2_tunnel_echoes_over_plain_loopback() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = runtime().block_on(async move {
            tokio::net::TcpListener::bind(addr)
                .await
                .expect("绑本地端口")
        });
        let local = listener.local_addr().expect("取本地地址");
        let server = runtime().spawn(async move {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            let Ok(mut connection) = h2::server::handshake(socket).await else {
                return;
            };
            while let Some(accepted) = connection.accept().await {
                let Ok((request, mut respond)) = accepted else {
                    break;
                };
                tokio::spawn(async move {
                    let mut body = request.into_body();
                    let response = http::Response::builder()
                        .status(200)
                        .body(())
                        .expect("响应");
                    let Ok(mut send) = respond.send_response(response, false) else {
                        return;
                    };
                    while let Some(chunk) = body.data().await {
                        let Ok(chunk) = chunk else { break };
                        if chunk.is_empty() {
                            let _ = send.send_data(Bytes::new(), true);
                            return;
                        }
                        let _ = body.flow_control().release_capacity(chunk.len());
                        if send.send_data(chunk, false).is_err() {
                            return;
                        }
                    }
                    let _ = send.send_data(Bytes::new(), true);
                });
            }
        });

        let mut stream = runtime().block_on(async move {
            let tcp = tokio::net::TcpStream::connect(local).await.expect("连本地");
            tunnel_over(
                tcp,
                format!("127.0.0.1:{}", local.port()),
                ExtraTransport::H2 {
                    host: String::new(),
                    path: "/".into(),
                },
            )
            .await
            .expect("建 h2 隧道")
        });
        stream.write_all(b"hello-h2").expect("写");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("设读超时");
        let mut buffer = [0u8; 32];
        let read = stream.read(&mut buffer).expect("读回显");
        assert_eq!(&buffer[..read], b"hello-h2");
        drop(stream);
        server.abort();
    }

    /// gRPC 模式：写入的数据要套消息头，读回时自动拆掉。
    #[test]
    fn grpc_tunnel_echoes_with_framing() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = runtime().block_on(async move {
            tokio::net::TcpListener::bind(addr)
                .await
                .expect("绑本地端口")
        });
        let local = listener.local_addr().expect("取本地地址");
        let server = runtime().spawn(async move {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            let Ok(mut connection) = h2::server::handshake(socket).await else {
                return;
            };
            while let Some(accepted) = connection.accept().await {
                let Ok((request, mut respond)) = accepted else {
                    break;
                };
                // 服务端也要看 `content-type` 与路径对得上。
                assert_eq!(
                    request.headers().get("content-type").unwrap(),
                    "application/grpc"
                );
                assert_eq!(request.uri().path(), "/gun/Tun");
                tokio::spawn(async move {
                    let mut body = request.into_body();
                    let response = http::Response::builder()
                        .status(200)
                        .body(())
                        .expect("响应");
                    let Ok(mut send) = respond.send_response(response, false) else {
                        return;
                    };
                    // 收到的每块先拆掉 gRPC 头、再解 `Hunk` 拿明文，然后按同样格式回写。
                    let mut raw = Vec::new();
                    while let Some(chunk) = body.data().await {
                        let Ok(chunk) = chunk else { break };
                        let _ = body.flow_control().release_capacity(chunk.len());
                        raw.extend_from_slice(&chunk);
                        let mut bodies = Vec::new();
                        grpc_deframe(&mut raw, &mut bodies);
                        if bodies.is_empty() {
                            continue;
                        }
                        let Some((data, _)) = hunk_decode(&bodies) else {
                            continue;
                        };
                        let echo = hunk_encode(&data);
                        if send
                            .send_data(Bytes::from(grpc_frame(&echo)), false)
                            .is_err()
                        {
                            return;
                        }
                    }
                    let _ = send.send_data(Bytes::new(), true);
                });
            }
        });

        let mut stream = runtime().block_on(async move {
            let tcp = tokio::net::TcpStream::connect(local).await.expect("连本地");
            tunnel_over(
                tcp,
                format!("127.0.0.1:{}", local.port()),
                ExtraTransport::Grpc {
                    service_name: "gun".into(),
                },
            )
            .await
            .expect("建 grpc 隧道")
        });
        stream.write_all(b"hello-grpc").expect("写");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("设读超时");
        let mut buffer = [0u8; 32];
        let read = stream.read(&mut buffer).expect("读回显");
        assert_eq!(&buffer[..read], b"hello-grpc");
        drop(stream);
        server.abort();
    }
}
