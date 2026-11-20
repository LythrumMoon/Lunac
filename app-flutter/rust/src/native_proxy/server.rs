//! 入站服务器：SOCKS5（CONNECT）与 HTTP（CONNECT / 绝对 URI）共用同一个端口。
//!
//! 重建自 v1 `native_proxy/server.rs` 的 `handle_client`。分工：
//!   · 本文件只做「握手 + 目标解析 + 双向透传」；
//!   · 走直连还是节点由 `Shared::should_proxy` + `rules` 决定；
//!   · 协议 / 传输在 `ss.rs` / `trojan.rs` / `vless.rs` / `transport.rs`。
//!
//! 错误口径（v1 同款）：握手失败就给对端一个最小可理解的应答（SOCKS5 0x07 / HTTP 400 /
//! 502），并把原因打到 stderr；**绝不**把内部细节拼进 HTML 或假装连接成功。

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, Shutdown, TcpStream};
#[cfg(test)]
use std::net::IpAddr;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use super::transport::{BoxStream, DIAL_TIMEOUT};
use super::{ss, transport, trojan, vless, Shared};

/// 握手阶段的读超时（客户端连上后不发请求时不要永久占线程）。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// 一个被请求的目标。
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub host: String,
    pub port: u16,
}

pub fn handle_client(mut client: TcpStream, shared: std::sync::Arc<Shared>) {
    let _ = client.set_nodelay(true);
    let _ = client.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
    let mut first = [0u8; 1];
    if client.read_exact(&mut first).is_err() {
        return;
    }
    // `PROCESS-NAME` 规则要回答「这条连接是哪个进程发起的」。
    // 没有这类规则就不枚举 TCP 表（每次 1–3 ms，浏览器开一堆连接时不值得白花）。
    let process = if shared.needs_process() {
        super::process::name_for_peer(&client)
    } else {
        None
    };
    let result = if first[0] == 0x05 {
        socks5(&mut client, &shared, process.as_deref())
    } else {
        http(&mut client, first[0], &shared, process.as_deref())
    };
    if let Err(reason) = result {
        eprintln!("[lunac-proxy] 入站连接结束：{reason}");
    }
}

fn socks5(
    client: &mut TcpStream,
    shared: &std::sync::Arc<Shared>,
    process: Option<&str>,
) -> Result<(), String> {
    // greeting：ver=5, nmethods, methods —— 版本号已经在 `handle_client` 里读过，
    // 这里只读方法数（再读一次 ver 会把 nmethods 当版本号，直接判成「不是 SOCKS5」）。
    let mut count = [0u8; 1];
    read_exact(client, &mut count).map_err(|e| format!("SOCKS5 greeting 读取失败：{e}"))?;
    if count[0] == 0 {
        let _ = client.write_all(&[0x05, 0xFF]);
        return Err("SOCKS5 客户端没有提供认证方法".to_string());
    }
    let mut methods = vec![0u8; count[0] as usize];
    read_exact(client, &mut methods).map_err(|e| format!("SOCKS5 methods 读取失败：{e}"))?;
    if !methods.contains(&0x00) {
        let _ = client.write_all(&[0x05, 0xFF]);
        return Err("客户端不支持无认证方式".to_string());
    }
    client
        .write_all(&[0x05, 0x00])
        .map_err(|e| format!("SOCKS5 方法应答失败：{e}"))?;

    // request：ver, cmd, rsv, atyp, addr, port
    let mut head = [0u8; 4];
    read_exact(client, &mut head).map_err(|e| format!("SOCKS5 请求头读取失败：{e}"))?;
    let command = head[1];
    let target = read_socks_target(client, head[3])?;
    if command == 0x03 {
        // UDP ASSOCIATE：交给 udp.rs（只做直连）。
        return super::udp::serve(client, shared.as_ref(), target);
    }
    if command != 0x01 {
        reply_socks(client, 0x07)?;
        return Err(format!("SOCKS5 命令 0x{command:02x} 不支持"));
    }
    let remote = match dial(shared, &target, process) {
        Ok(remote) => remote,
        Err(reason) => {
            let _ = reply_socks(client, 0x01);
            return Err(reason);
        }
    };
    reply_socks(client, 0x00)?;
    pipe(client, remote, Vec::new(), shared.clone())
}

pub(crate) fn read_socks_target(client: &mut TcpStream, atyp: u8) -> Result<Target, String> {
    let host = match atyp {
        0x01 => {
            let mut octets = [0u8; 4];
            read_exact(client, &mut octets).map_err(|e| format!("SOCKS5 IPv4 读取失败：{e}"))?;
            Ipv4Addr::from(octets).to_string()
        }
        0x04 => {
            let mut octets = [0u8; 16];
            read_exact(client, &mut octets).map_err(|e| format!("SOCKS5 IPv6 读取失败：{e}"))?;
            Ipv6Addr::from(octets).to_string()
        }
        0x03 => {
            let mut len = [0u8; 1];
            read_exact(client, &mut len).map_err(|e| format!("SOCKS5 域名长度读取失败：{e}"))?;
            let mut name = vec![0u8; len[0] as usize];
            read_exact(client, &mut name).map_err(|e| format!("SOCKS5 域名读取失败：{e}"))?;
            String::from_utf8_lossy(&name).to_string()
        }
        other => return Err(format!("SOCKS5 地址类型 0x{other:02x} 不支持")),
    };
    let mut port = [0u8; 2];
    read_exact(client, &mut port).map_err(|e| format!("SOCKS5 端口读取失败：{e}"))?;
    Ok(Target {
        host,
        port: u16::from_be_bytes(port),
    })
}

pub(crate) fn reply_socks(client: &mut TcpStream, code: u8) -> Result<(), String> {
    // 绑定地址一律给 0.0.0.0:0（客户端只用状态码）。
    let reply = [0x05, code, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
    client
        .write_all(&reply)
        .map_err(|e| format!("SOCKS5 应答失败：{e}"))
}

fn http(
    client: &mut TcpStream,
    first: u8,
    shared: &std::sync::Arc<Shared>,
    process: Option<&str>,
) -> Result<(), String> {
    const MAX_HEAD: usize = 64 * 1024;
    let mut head = vec![first];
    let mut chunk = [0u8; 2048];
    let end = loop {
        if let Some(pos) = find_header_end(&head) {
            break pos;
        }
        if head.len() > MAX_HEAD {
            let _ = write_http_error(client, 431, "Request Header Fields Too Large");
            return Err("HTTP 请求头过大".to_string());
        }
        let read = client
            .read(&mut chunk)
            .map_err(|e| format!("HTTP 请求头读取失败：{e}"))?;
        if read == 0 {
            return Err("HTTP 客户端在请求头未完成时关闭".to_string());
        }
        head.extend_from_slice(&chunk[..read]);
    };
    let header_text = String::from_utf8_lossy(&head[..end]).to_string();
    let leftover = head[end..].to_vec();
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_ascii_uppercase();
    let target_text = parts.next().unwrap_or("");
    let version = parts.next().unwrap_or("HTTP/1.1");
    if method.is_empty() || target_text.is_empty() {
        let _ = write_http_error(client, 400, "Bad Request");
        return Err("HTTP 请求行非法".to_string());
    }

    if method == "CONNECT" {
        let target = parse_authority(target_text)?;
        let remote = match dial(shared, &target, process) {
            Ok(remote) => remote,
            Err(reason) => {
                let _ = write_http_error(client, 502, "Bad Gateway");
                return Err(reason);
            }
        };
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .map_err(|e| format!("HTTP CONNECT 应答失败：{e}"))?;
        return pipe(client, remote, leftover, shared.clone());
    }

    if target_text.starts_with("https://") {
        let _ = write_http_error(client, 400, "Bad Request");
        return Err("HTTPS 绝对 URI 需要先 CONNECT".to_string());
    }
    if !target_text.starts_with("http://") {
        let _ = write_http_error(client, 400, "Bad Request");
        return Err("只支持 CONNECT 与 http:// 绝对 URI".to_string());
    }
    let url = url::Url::parse(target_text).map_err(|e| format!("绝对 URI 解析失败：{e}"))?;
    let host = url
        .host_str()
        .ok_or_else(|| "绝对 URI 缺主机".to_string())?
        .to_string();
    let port = url.port_or_known_default().unwrap_or(80);
    let target = Target { host, port };
    let mut remote = match dial(shared, &target, process) {
        Ok(remote) => remote,
        Err(reason) => {
            let _ = write_http_error(client, 502, "Bad Gateway");
            return Err(reason);
        }
    };
    // 重写请求行：绝对 URI → origin-form；其余头原样转发。
    let mut path = url.path().to_string();
    if path.is_empty() {
        path.push('/');
    }
    if let Some(query) = url.query() {
        path.push('?');
        path.push_str(query);
    }
    let mut request = format!("{method} {path} {version}\r\n");
    for line in lines {
        if line.is_empty() {
            continue;
        }
        request.push_str(line);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    remote
        .write_all(request.as_bytes())
        .map_err(|e| format!("转发 HTTP 请求头失败：{e}"))?;
    pipe(client, remote, leftover, shared.clone())
}

fn parse_authority(text: &str) -> Result<Target, String> {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix('[') {
        // [::1]:443
        let (host, tail) = rest
            .split_once(']')
            .ok_or_else(|| "CONNECT IPv6 地址缺右括号".to_string())?;
        let port = tail
            .strip_prefix(':')
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(443);
        return Ok(Target {
            host: host.to_string(),
            port,
        });
    }
    match text.rsplit_once(':') {
        Some((host, port)) => Ok(Target {
            host: host.to_string(),
            port: port
                .parse::<u16>()
                .map_err(|_| format!("CONNECT 端口非法：{port}"))?,
        }),
        None => Ok(Target {
            host: text.to_string(),
            port: 443,
        }),
    }
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|pos| pos + 4)
}

fn write_http_error(client: &mut TcpStream, code: u16, text: &str) -> io::Result<()> {
    let body = format!("{code} {text}\n");
    let response = format!(
        "HTTP/1.1 {code} {text}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    client.write_all(response.as_bytes())
}

/// 建立到目标的连接（直连或经当前节点），并把这一跳记进请求日志。
///
/// 「走直连还是走节点」只由 [`Shared::decide_route`] 决定 —— 日志区展示的原因与
/// 真正拨号用的是同一份判定，不允许两处各写一套。
pub fn dial(shared: &Shared, target: &Target, process: Option<&str>) -> Result<BoxStream, String> {
    let (route, detail) = shared.decide_route(&target.host, process);
    match route {
        super::Route::Direct => {
            match transport::connect_tcp(&target.host, target.port, DIAL_TIMEOUT) {
                Ok(stream) => {
                    super::record_request(&target.host, target.port, route, detail);
                    Ok(Box::new(stream))
                }
                Err(e) => {
                    super::record_request(
                        &target.host,
                        target.port,
                        super::Route::Rejected,
                        format!("{detail}；直连失败：{e}"),
                    );
                    Err(format!("直连 {}:{} 失败：{e}", target.host, target.port))
                }
            }
        }
        super::Route::Rejected => {
            super::record_request(&target.host, target.port, route, detail.clone());
            Err(detail)
        }
        super::Route::Node => {
            let node = shared
                .active_node()
                .ok_or_else(|| "规则要求走节点，但引擎里没有节点".to_string())?
                .clone();
            match dial_node(&node, target) {
                Ok(stream) => {
                    super::record_request(&target.host, target.port, route, detail);
                    Ok(stream)
                }
                Err(reason) => {
                    super::record_request(
                        &target.host,
                        target.port,
                        super::Route::Rejected,
                        format!("{detail}；连接失败：{reason}"),
                    );
                    Err(reason)
                }
            }
        }
    }
}

/// 按**指定节点**建立到目标的连接（不看规则、不看当前选中项）。
///
/// 测速探针用：面板要对每个节点都测一次，不能临时切节点（会打断正在走的连接）。
pub fn dial_node(node: &super::Node, target: &Target) -> Result<BoxStream, String> {
    if !node.protocol.implemented() {
        return Err(format!(
            "自研引擎尚未重建 {} 协议（请换 ss / trojan / vless / vmess / anytls / hysteria2 / tuic 节点）",
            node.protocol.kind()
        ));
    }
    // Hysteria2 走 QUIC，没有「先建 TCP 传输」这一步，必须在 `connect_transport` 之前分叉。
    if let super::Protocol::Hysteria2 {
        password,
        obfs,
        obfs_password,
        insecure,
    } = &node.protocol
    {
        let config = super::hysteria2::Hysteria2Config {
            server: node.server.clone(),
            port: node.port,
            sni: node.tls.sni.clone(),
            // 分享链接里的 `insecure=1` 与 `allowInsecure` 任一为真即跳过证书校验。
            insecure: *insecure || node.tls.allow_insecure,
            password: password.clone(),
            obfs: obfs.clone(),
            obfs_password: obfs_password.clone(),
        };
        return super::hysteria2::connect(&config, &target.host, target.port);
    }
    // TUIC 也走 QUIC：同样必须在 `connect_transport` 之前分叉。
    if let super::Protocol::Tuic {
        uuid,
        password,
        udp_relay_mode,
    } = &node.protocol
    {
        let config = super::tuic::TuicConfig {
            server: node.server.clone(),
            port: node.port,
            sni: node.tls.sni.clone(),
            insecure: node.tls.allow_insecure,
            alpn: node.tls.alpn.clone(),
            uuid: uuid.clone(),
            password: password.clone(),
            udp_relay_mode: udp_relay_mode.clone(),
        };
        return super::tuic::connect(&config, &target.host, target.port);
    }
    // AnyTLS 也在 `connect_transport` 之前分叉：会话要先进池里复用，不能每条连接都拨一次 TLS。
    if let super::Protocol::AnyTls { password } = &node.protocol {
        let key = super::anytls::session_key(
            &node.server,
            node.port,
            &node.tls.sni,
            node.transport(),
            node.tls.allow_insecure,
            password,
        );
        return super::anytls::connect_pooled(
            &key,
            || transport::connect_transport(node),
            password,
            &target.host,
            target.port,
        );
    }
    // Xray 的 mux.cool：同一节点的多条 TCP 流共用一条到 `v1.mux.cool` 的底层连接。
    // 只对 VLESS / VMess / Trojan 生效（`mux.rs::connect_pooled` 会对其它协议如实报错）。
    if node.mux {
        return super::mux::connect_pooled(node, &target.host, target.port);
    }
    let stream = transport::connect_transport(node)?;
    match &node.protocol {
        super::Protocol::Ss {
            method,
            password,
            obfs,
        } => {
            // SIP003 simple-obfs 夹在「TCP」与「SS 的 salt / 地址头」之间。
            let stream = match obfs {
                Some(options) => super::ss_plugin::wrap(stream, options)?,
                None => stream,
            };
            ss::connect(stream, method, password, &target.host, target.port)
        }
        super::Protocol::Trojan { password } => {
            trojan::connect(stream, password, &target.host, target.port)
        }
        super::Protocol::Vless { uuid, flow } => {
            vless::connect(stream, uuid, flow, &target.host, target.port)
        }
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
            &target.host,
            target.port,
        ),
        other => Err(format!("自研引擎尚未重建 {} 协议", other.kind())),
    }
}

/// 透传期远端流的读超时。
///
/// std 的阻塞 socket 没有 select，而「客户端 → 远端」和「远端 → 客户端」两个方向
/// 共用同一个（TLS 状态下不可拆分的）流对象。用短读超时把下行的阻塞读切成小片，
/// 每片之后把上行队列搬过去：既不需要 `mio` 那一层，也不会让上行被下行饿死。
/// 5ms 的空转代价 = 每条空闲连接每秒约 200 次 `recv`（微秒级），换来上行零额外延迟。
const DOWN_POLL: Duration = Duration::from_millis(5);

/// 双向透传：客户端 → 远端在子线程（只往队列里塞），远端 → 客户端在当线程。
fn pipe(
    client: &mut TcpStream,
    mut remote: BoxStream,
    pending: Vec<u8>,
    shared: std::sync::Arc<Shared>,
) -> Result<(), String> {
    use std::sync::mpsc::{channel, TryRecvError};

    // 握手阶段设的读超时必须在透传前清掉，否则长连接会被 30s 掐断。
    let _ = client.set_read_timeout(None);
    if !pending.is_empty() {
        shared.up.fetch_add(pending.len() as u64, Ordering::Relaxed);
        remote
            .write_all(&pending)
            .map_err(|e| format!("转发首个请求体失败：{e}"))?;
    }
    let mut client_read = client
        .try_clone()
        .map_err(|e| format!("复制客户端句柄失败：{e}"))?;
    let (tx, rx) = channel::<Vec<u8>>();
    let up_shared = std::sync::Arc::clone(&shared);
    let upload = thread::Builder::new()
        .name("lunac-proxy-up".to_string())
        .spawn(move || {
            let mut chunk = [0u8; 16 * 1024];
            loop {
                match client_read.read(&mut chunk) {
                    // 空 Vec 是「上行结束」的哨兵（半关闭：继续把下行读完再收摊）。
                    Ok(0) => {
                        let _ = tx.send(Vec::new());
                        break;
                    }
                    Ok(read) => {
                        up_shared.up.fetch_add(read as u64, Ordering::Relaxed);
                        if tx.send(chunk[..read].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        let _ = tx.send(Vec::new());
                        break;
                    }
                }
            }
        })
        .ok();
    let _ = remote.set_read_timeout(Some(DOWN_POLL));

    let mut down = [0u8; 16 * 1024];
    let mut upload_open = true;
    let mut broken = false;
    while !broken {
        // ① 先把上行队列搬空（这一轮里有多少搬多少）。
        if upload_open {
            loop {
                match rx.try_recv() {
                    Ok(data) if data.is_empty() => {
                        upload_open = false;
                        break;
                    }
                    Ok(data) => {
                        if remote.write_all(&data).is_err() {
                            // 远端不再收：下行可能还有尾巴，继续读完再退出。
                            upload_open = false;
                            break;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        upload_open = false;
                        break;
                    }
                }
            }
        }
        // ② 读一段下行。
        match remote.read(&mut down) {
            Ok(0) => break,
            Ok(read) => {
                shared.down.fetch_add(read as u64, Ordering::Relaxed);
                if client.write_all(&down[..read]).is_err() {
                    broken = true;
                }
            }
            Err(ref e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::TimedOut
                    || e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => broken = true,
        }
    }
    let _ = client.shutdown(Shutdown::Both);
    if let Some(handle) = upload {
        let _ = handle.join();
    }
    Ok(())
}

fn read_exact(stream: &mut TcpStream, buffer: &mut [u8]) -> io::Result<()> {
    stream.read_exact(buffer)
}

/// 把「域名:端口」解析成 Target（HTTP CONNECT 的 authority 形态）。
#[cfg(test)]
pub fn target_from_authority(text: &str) -> Result<Target, String> {
    parse_authority(text)
}

/// 纯函数：SOCKS5 地址类型 + 值 → Target（单测用）。
#[cfg(test)]
pub fn socks_target_from_parts(atyp: u8, bytes: &[u8], port: u16) -> Result<Target, String> {
    let host = match atyp {
        0x01 if bytes.len() == 4 => {
            IpAddr::V4(Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]))
        }
        0x03 if !bytes.is_empty() => {
            return Ok(Target {
                host: String::from_utf8_lossy(bytes).to_string(),
                port,
            })
        }
        0x04 if bytes.len() == 16 => {
            let mut octets = [0u8; 16];
            octets.copy_from_slice(bytes);
            IpAddr::V6(Ipv6Addr::from(octets))
        }
        _ => return Err("SOCKS5 地址非法".to_string()),
    };
    Ok(Target {
        host: host.to_string(),
        port,
    })
}

/// 引擎是否在跑（`udp.rs` 用）。
#[allow(dead_code)] // 保留给「引擎是否在跑」的只读查询；当前调用方直接用 Shared 状态。
pub fn engine_alive(shared: &Shared) -> bool {
    shared.alive.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize};
    use std::sync::Arc;

    fn shared_direct_only() -> Arc<Shared> {
        Arc::new(Shared {
            nodes: Vec::new(),
            rules: Vec::new(),
            active: AtomicUsize::new(0),
            alive: AtomicBool::new(true),
            mode: AtomicU8::new(0),
            sources: Vec::new(),
            fallback_proxy: true,
            up: AtomicU64::new(0),
            down: AtomicU64::new(0),
            geoip: None,
        })
    }

    /// 本地 echo 服务：读 4 字节原样回写（测试用）。
    fn spawn_echo() -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0u8; 4];
                if stream.read_exact(&mut buffer).is_ok() {
                    let _ = stream.write_all(&buffer);
                }
            }
        });
        addr
    }

    fn spawn_proxy() -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let shared = shared_direct_only();
        thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                handle_client(stream, shared);
            }
        });
        addr
    }

    #[test]
    fn absolute_uri_is_split_into_authority_and_path() {
        let url = url::Url::parse("http://example.com:8080/a/b?c=1").unwrap();
        assert_eq!(url.host_str().unwrap(), "example.com");
        assert_eq!(url.port_or_known_default().unwrap(), 8080);
        assert_eq!(url.path(), "/a/b");
        assert_eq!(url.query(), Some("c=1"));
    }

    #[test]
    fn authority_parsing_handles_ipv6_and_default_port() {
        assert_eq!(
            target_from_authority("[::1]:8443").unwrap(),
            Target {
                host: "::1".to_string(),
                port: 8443
            }
        );
        assert_eq!(
            target_from_authority("example.com").unwrap(),
            Target {
                host: "example.com".to_string(),
                port: 443
            }
        );
    }

    #[test]
    fn socks_target_parts_cover_three_address_types() {
        assert_eq!(
            socks_target_from_parts(0x01, &[1, 2, 3, 4], 80).unwrap(),
            Target {
                host: "1.2.3.4".to_string(),
                port: 80
            }
        );
        assert_eq!(
            socks_target_from_parts(0x03, b"example.com", 443).unwrap(),
            Target {
                host: "example.com".to_string(),
                port: 443
            }
        );
        assert!(socks_target_from_parts(0x04, &[0u8; 16], 80).is_ok());
    }

    #[test]
    fn read_timeout_on_boxed_stream_reaches_the_socket() {
        // `pipe` 靠 `Box<dyn Stream>` 的 `set_read_timeout` 把下行阻塞读切成小片。
        // 这条必须真的作用到底层 socket 上，否则透传会退化成永久阻塞（上行永远搬不出去）。
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = TcpStream::connect(addr).unwrap();
        let (_peer, _) = listener.accept().unwrap();
        let mut stream: BoxStream = Box::new(client);
        stream
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let started = std::time::Instant::now();
        let mut buffer = [0u8; 1];
        let error = stream.read(&mut buffer).unwrap_err();
        assert!(
            error.kind() == io::ErrorKind::TimedOut || error.kind() == io::ErrorKind::WouldBlock,
            "读超时没有生效：{error:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn socks5_end_to_end_in_direct_mode() {
        let echo = spawn_echo();
        let proxy = spawn_proxy();
        let mut client = TcpStream::connect(proxy).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        // greeting
        client.write_all(&[0x05, 0x01, 0x00]).unwrap();
        let mut reply = [0u8; 2];
        client.read_exact(&mut reply).unwrap();
        assert_eq!(reply, [0x05, 0x00]);
        // CONNECT 127.0.0.1:<echo>
        let mut request = vec![0x05, 0x01, 0x00, 0x01];
        request.extend_from_slice(&[127, 0, 0, 1]);
        request.extend_from_slice(&echo.port().to_be_bytes());
        client.write_all(&request).unwrap();
        let mut reply = [0u8; 10];
        client.read_exact(&mut reply).unwrap();
        assert_eq!(reply[0], 0x05);
        assert_eq!(reply[1], 0x00);
        // 透传
        client.write_all(b"ping").unwrap();
        let mut out = [0u8; 4];
        client.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"ping");
    }

    #[test]
    fn http_connect_end_to_end_in_direct_mode() {
        let echo = spawn_echo();
        let proxy = spawn_proxy();
        let mut client = TcpStream::connect(proxy).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let request = format!(
            "CONNECT 127.0.0.1:{} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
            echo.port()
        );
        client.write_all(request.as_bytes()).unwrap();
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 256];
        loop {
            let read = client.read(&mut chunk).unwrap();
            buffer.extend_from_slice(&chunk[..read]);
            if buffer.windows(4).any(|w| w == b"\r\n\r\n") {
                assert!(String::from_utf8_lossy(&buffer).contains("200"));
                break;
            }
        }
        client.write_all(b"ping").unwrap();
        let mut out = [0u8; 4];
        client.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"ping");
    }

    #[test]
    fn unsupported_socks_command_gets_0x07() {
        let proxy = spawn_proxy();
        let mut client = TcpStream::connect(proxy).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        client.write_all(&[0x05, 0x01, 0x00]).unwrap();
        let mut reply = [0u8; 2];
        client.read_exact(&mut reply).unwrap();
        assert_eq!(reply, [0x05, 0x00]);
        // CMD = BIND (0x02) 不支持
        let mut request = vec![0x05, 0x02, 0x00, 0x01];
        request.extend_from_slice(&[127, 0, 0, 1]);
        request.extend_from_slice(&80u16.to_be_bytes());
        client.write_all(&request).unwrap();
        let mut reply = [0u8; 10];
        client.read_exact(&mut reply).unwrap();
        assert_eq!(reply[1], 0x07);
    }
}
