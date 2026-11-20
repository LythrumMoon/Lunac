//! SOCKS5 UDP ASSOCIATE（重建 v1 `native_proxy/udp.rs`）。
//!
//! 控制连接的生命周期绑定数据转发：控制连接一断，转发立即结束（v1 同口径）。
//! 行为：
//!   · 分配一个回环 UDP 端口并在 SOCKS5 应答里返回；
//!   · 从客户端收到数据报 → 解析 `RSV RSV FRAG ATYP ADDR PORT DATA` → 按规则判定；
//!   · 直连：直接从上游 socket 发出，响应头写**真实来源地址**；
//!   · 经节点：按目标地址懒创建一条上游会话（见 [`Upstream`]），响应头写**目标地址**；
//!   · `FRAG != 0` 一律丢弃（v1：不做分片重组）。
//!
//! **已重建的上游**（2026-11-15）：
//!   · SS-SIP004（每个数据报独立 salt + 子密钥、nonce 恒 0）；
//!   · SS-2022（SIP022 §3.2：AES 分组密码的独立头 + 会话子密钥 + 重放窗口，见
//!     [`ss2022::UdpClient`]）；
//!   · VLESS（非 Vision：`cmd=UDP` + 每包长度前缀；Vision：Mux `v1.mux.cool` + XUDP，
//!     见 [`super::xudp::XudpStream`]）；
//!   · VMess（`cmd=0x02` + 目标在指令段里，**一个分块 = 一个数据报**）；
//!   · AnyTLS（UoT v2 connect 模式：先连魔法域名 `sp.v2.udp-over-tcp.arpa`，再
//!     `isConnect=1 | SOCKS 目标` + `[len u16][payload]`，见 [`super::uot::UotStream`]）；
//!   · Trojan（`ATYP addr port CRLF u16 len CRLF payload`）。
//! **已重建（2026-11-15 回填）**：Hysteria2 的 UDP（QUIC datagram + `UDPMessage` 分片/重组）。
//! **已重建（2026-11-16）**：TUIC v5 的 UDP（QUIC datagram + `Packet` 分片/重组，`native` 模式）。
//! **仍未重建**：SS 的 SIP003 插件（它只承载 TCP）—— 命中时**丢弃并只打一行日志**
//! （UDP 没有错误信道），不静默假装成功。

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::server::Target;
use super::transport::BoxStream;
use super::{anytls, ss, ss2022, trojan, uot, vless, vmess, Protocol, Shared};

/// 上游会话闲置多久后回收（UDP 没有显式关闭，只能按时间收）。
const SESSION_IDLE: Duration = Duration::from_secs(60);
/// 经节点的 TCP 型上游（Trojan）读超时：与透传期同量级。
const NODE_READ_TIMEOUT: Duration = Duration::from_millis(5);

/// 处理一个 UDP ASSOCIATE 请求（`requested` 是客户端声明的接收地址，通常 0.0.0.0:0）。
pub fn serve(client: &mut TcpStream, shared: &Shared, requested: Target) -> Result<(), String> {
    // `PROCESS-NAME` 规则：控制连接与数据报来自同一个进程，查一次就够。
    let process = if shared.needs_process() {
        super::process::name_for_peer(client)
    } else {
        None
    };
    let client_socket =
        UdpSocket::bind("127.0.0.1:0").map_err(|e| format!("UDP ASSOCIATE 绑定失败：{e}"))?;
    let up_socket = UdpSocket::bind("127.0.0.1:0").map_err(|e| format!("UDP 上游绑定失败：{e}"))?;
    client_socket
        .set_nonblocking(true)
        .map_err(|e| format!("UDP 非阻塞失败：{e}"))?;
    up_socket
        .set_nonblocking(true)
        .map_err(|e| format!("UDP 非阻塞失败：{e}"))?;
    let local = client_socket
        .local_addr()
        .map_err(|e| format!("读取 UDP 端口失败：{e}"))?;
    // SOCKS5 成功应答：BND.ADDR = 127.0.0.1，BND.PORT = 数据端口。
    let reply = [
        0x05,
        0x00,
        0x00,
        0x01,
        127,
        0,
        0,
        1,
        (local.port() >> 8) as u8,
        (local.port() & 0xFF) as u8,
    ];
    client
        .write_all(&reply)
        .map_err(|e| format!("UDP ASSOCIATE 应答失败：{e}"))?;
    client
        .set_read_timeout(Some(Duration::from_millis(150)))
        .map_err(|e| format!("设置控制连接超时失败：{e}"))?;

    let mut client_addr: Option<SocketAddr> = None;
    if requested.port != 0 {
        if let Ok(ip) = requested.host.parse::<IpAddr>() {
            if !ip.is_unspecified() {
                client_addr = Some(SocketAddr::new(ip, requested.port));
            }
        }
    }
    let mut sessions: HashMap<(String, u16), NodeSession> = HashMap::new();
    // 同一句原因只打一次日志（UDP 每个数据报都会命中一次，不节流会刷屏）。
    let mut warned: Vec<String> = Vec::new();
    let mut packet = vec![0u8; 65535];
    let mut response = vec![0u8; 65535];
    loop {
        // 控制连接一断就停（客户端靠它保活）。
        let mut probe = [0u8; 1];
        match client.read(&mut probe) {
            Ok(0) => break,
            Ok(_) => {}
            Err(ref e)
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => {
            }
            Err(_) => break,
        }
        // 客户端 → 上游
        loop {
            match client_socket.recv_from(&mut packet) {
                Ok((len, from)) => {
                    client_addr = Some(from);
                    let Some((target, payload)) = parse_udp_datagram(&packet[..len]) else {
                        continue;
                    };
                    let (route, detail) = shared.decide_route(&target.host, process.as_deref());
                    if route != super::Route::Node {
                        super::record_request(&target.host, target.port, route, detail);
                        if route == super::Route::Direct {
                            if let Ok(addrs) = (target.host.as_str(), target.port).to_socket_addrs()
                            {
                                for addr in addrs {
                                    let _ = up_socket.send_to(&payload, addr);
                                    break;
                                }
                                shared.up.fetch_add(payload.len() as u64, Ordering::Relaxed);
                            }
                        }
                        continue;
                    }
                    let key = (target.host.clone(), target.port);
                    if !sessions.contains_key(&key) {
                        match open_session(shared, &target) {
                            Ok(upstream) => {
                                super::record_request(
                                    &target.host,
                                    target.port,
                                    super::Route::Node,
                                    detail,
                                );
                                sessions.insert(
                                    key.clone(),
                                    NodeSession {
                                        upstream,
                                        last: Instant::now(),
                                    },
                                );
                            }
                            Err(reason) => {
                                super::record_request(
                                    &target.host,
                                    target.port,
                                    super::Route::Rejected,
                                    format!("{detail}；{reason}"),
                                );
                                let line = format!("[lunac-proxy] UDP 经节点不可用：{reason}");
                                if !warned.contains(&line) {
                                    eprintln!("{line}");
                                    warned.push(line);
                                }
                                continue;
                            }
                        }
                    }
                    let mut drop_key = false;
                    if let Some(session) = sessions.get_mut(&key) {
                        session.last = Instant::now();
                        if session.upstream.send(&target, &payload).is_err() {
                            drop_key = true;
                        }
                    }
                    if drop_key {
                        sessions.remove(&key);
                    }
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
        // 经节点的上游 → 客户端
        sessions.retain(|_, session| session.last.elapsed() < SESSION_IDLE);
        let destination = client_addr;
        for session in sessions.values_mut() {
            loop {
                match session.upstream.recv() {
                    Ok(Some((host, port, payload))) => {
                        let Some(destination) = destination else {
                            continue;
                        };
                        let mut frame = Vec::with_capacity(3 + payload.len() + 32);
                        frame.extend_from_slice(&[0u8, 0u8, 0u8]);
                        frame.extend_from_slice(&super::encode_target(&host, port));
                        frame.extend_from_slice(&payload);
                        let _ = client_socket.send_to(&frame, destination);
                    }
                    Ok(None) => break,
                    Err(reason) => {
                        // UDP 没有错误信道：至少把这句原因打一次，别让它看上去像「没回包」。
                        let line = format!("[lunac-proxy] UDP 上游读失败：{reason}");
                        if !warned.contains(&line) {
                            eprintln!("{line}");
                            warned.push(line);
                        }
                        break;
                    }
                }
            }
        }
        // 上游 → 客户端（直连）
        loop {
            match up_socket.recv_from(&mut response) {
                Ok((len, from)) => {
                    let Some(header) = udp_header(from) else {
                        continue;
                    };
                    let Some(destination) = client_addr else {
                        continue;
                    };
                    let mut frame = Vec::with_capacity(header.len() + len);
                    frame.extend_from_slice(&header);
                    frame.extend_from_slice(&response[..len]);
                    let _ = client_socket.send_to(&frame, destination);
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
    }
    Ok(())
}

/// 一条「经节点」的 UDP 上游会话（按目标地址懒创建）。
struct NodeSession {
    upstream: Upstream,
    last: Instant,
}

/// 上游实现。
///
/// SS 走一个独立 UDP socket（服务端按五元组回包，天然按目标隔离）；
/// Trojan 走一条独立 TCP 流（UDP ASSOCIATE，按目标隔离）。
enum Upstream {
    Ss {
        socket: UdpSocket,
        node: SocketAddr,
        method: String,
        password: String,
    },
    /// SS-2022（SIP022 §3.2）：每个目标一条会话，带自己的会话 ID / 包序号 / 重放窗口。
    Ss2022 {
        socket: UdpSocket,
        node: SocketAddr,
        client: ss2022::UdpClient,
    },
    Trojan {
        stream: BoxStream,
        incoming: Vec<u8>,
    },
    /// 非 Vision 的 VLESS：`cmd=UDP`（0x02）+ 每包 `[len u16][payload]`；
    /// 目标由请求头固定 ⇒ 一条连接只服务一个目标（与 Xray 的非 cone 路径同口径）。
    VlessUdp {
        stream: vless::VlessUdpStream,
        host: String,
        port: u16,
    },
    /// VMess 的 UDP：`cmd=UDP`（0x02）+ 目标在指令段里，一个标准分块 = 一个数据报。
    VmessUdp {
        tunnel: vmess::VmessTunnel,
        host: String,
        port: u16,
    },
    /// AnyTLS 的 UDP：UoT v2（connect 模式），目标固定在 UoT 请求里。
    Uot {
        stream: uot::UotStream,
        host: String,
        port: u16,
    },
    /// VLESS(Vision) 的 UDP：Mux（`v1.mux.cool`）+ XUDP，一条 TCP 流承载所有目标。
    Xudp { stream: super::xudp::XudpStream },
    /// Hysteria2 的 UDP：QUIC datagram + `UDPMessage`（目标写在每条消息里，
    /// 一条连接就能服务多个目标；这里按 udp.rs 的会话粒度仍是一目标一条）。
    Hysteria2 {
        session: super::hysteria2::Hysteria2Udp,
    },
    /// TUIC 的 UDP：QUIC datagram（`native` 模式）+ `Packet` 分片，
    /// 目标写在每条消息里（一个 assoc id 一条连接，与上面的会话粒度一致）。
    Tuic { session: super::tuic::TuicUdp },
}

impl Upstream {
    fn send(&mut self, target: &Target, payload: &[u8]) -> io::Result<()> {
        match self {
            Self::Ss {
                socket,
                node,
                method,
                password,
            } => {
                let mut plain = Vec::with_capacity(payload.len() + 32);
                plain.extend_from_slice(&super::encode_target(&target.host, target.port));
                plain.extend_from_slice(payload);
                let packet = ss::udp_encrypt(method, password, &plain)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                socket.send_to(&packet, *node)?;
                Ok(())
            }
            Self::Ss2022 {
                socket,
                node,
                client,
            } => {
                let packet = client
                    .seal(&target.host, target.port, payload)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                socket.send_to(&packet, *node)?;
                Ok(())
            }
            Self::Trojan { stream, .. } => {
                let frame = trojan::udp_frame(&target.host, target.port, payload);
                stream.write_all(&frame)
            }
            Self::VlessUdp { stream, .. } => stream.send(payload),
            Self::VmessUdp { tunnel, .. } => tunnel.send_datagram(payload),
            Self::Uot { stream, .. } => stream.send(payload),
            Self::Xudp { stream } => stream
                .send(&target.host, target.port, payload)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Self::Hysteria2 { session } => session.send(&target.host, target.port, payload),
            Self::Tuic { session } => session.send(&target.host, target.port, payload),
        }
    }

    /// 上游回来的数据报（`(host, port, payload)`）；`Ok(None)` = 暂时没有。
    fn recv(&mut self) -> io::Result<Option<(String, u16, Vec<u8>)>> {
        match self {
            Self::Ss {
                socket,
                method,
                password,
                ..
            } => {
                let mut buffer = vec![0u8; 65535];
                let (len, _from) = match socket.recv_from(&mut buffer) {
                    Ok(value) => value,
                    Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                    Err(e) => return Err(e),
                };
                let plain = ss::udp_decrypt(method, password, &buffer[..len])
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                let Some((host, port, used)) = parse_socks_addr(&plain) else {
                    return Ok(None);
                };
                Ok(Some((host, port, plain[used..].to_vec())))
            }
            Self::Ss2022 { socket, client, .. } => {
                let mut buffer = vec![0u8; 65535];
                let (len, _from) = match socket.recv_from(&mut buffer) {
                    Ok(value) => value,
                    Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                    Err(e) => return Err(e),
                };
                match client.open(&buffer[..len]) {
                    Ok(value) => Ok(value),
                    // 重放 / 不属于这条会话：静默丢弃，不当成错误。
                    Err(reason) => {
                        if !reason.contains("会话 ID 不匹配") && !reason.contains("时间戳")
                        {
                            return Err(io::Error::new(io::ErrorKind::InvalidData, reason));
                        }
                        Ok(None)
                    }
                }
            }
            Self::VlessUdp { stream, host, port } => match stream.recv()? {
                Some(payload) => Ok(Some((host.clone(), *port, payload))),
                None => Ok(None),
            },
            Self::VmessUdp { tunnel, host, port } => match tunnel.recv_datagram()? {
                Some(payload) => Ok(Some((host.clone(), *port, payload))),
                None => Ok(None),
            },
            Self::Uot { stream, host, port } => match stream.recv()? {
                Some(payload) => Ok(Some((host.clone(), *port, payload))),
                None => Ok(None),
            },
            Self::Xudp { stream } => match stream.recv() {
                Ok(value) => Ok(value),
                Err(reason) => Err(io::Error::new(io::ErrorKind::InvalidData, reason)),
            },
            // 目标地址由 `UDPMessage` 自带，直接回 `(host, port, payload)`。
            Self::Hysteria2 { session } => session.recv(),
            // TUIC 的 `Packet` 也自带来/去地址（分片已在 `tuic.rs` 里重组）。
            Self::Tuic { session } => session.recv(),
            Self::Trojan { stream, incoming } => {
                let mut chunk = [0u8; 8192];
                match stream.read(&mut chunk) {
                    Ok(0) => {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "Trojan UDP 上游关闭",
                        ))
                    }
                    Ok(read) => incoming.extend_from_slice(&chunk[..read]),
                    Err(ref e)
                        if e.kind() == io::ErrorKind::WouldBlock
                            || e.kind() == io::ErrorKind::TimedOut => {}
                    Err(e) => return Err(e),
                }
                match trojan::parse_udp_frame(incoming) {
                    Some((host, port, payload, used)) => {
                        incoming.drain(..used);
                        Ok(Some((host, port, payload)))
                    }
                    None => Ok(None),
                }
            }
        }
    }
}

fn open_session(shared: &Shared, target: &Target) -> Result<Upstream, String> {
    let node = shared
        .active_node()
        .ok_or_else(|| "规则要求走节点，但引擎里没有节点".to_string())?
        .clone();
    match &node.protocol {
        Protocol::Ss {
            method,
            password,
            obfs,
        } => {
            if obfs.is_some() {
                // v1 同口径：SIP003 插件只承载 TCP，UDP 直接记一行「不转发」。
                return Err("SS 插件 simple-obfs 只承载 TCP（UDP 不转发）".to_string());
            }
            let node_addr = (node.server.as_str(), node.port)
                .to_socket_addrs()
                .map_err(|e| format!("解析节点地址失败：{e}"))?
                .next()
                .ok_or_else(|| "节点地址无解析结果".to_string())?;
            let socket =
                UdpSocket::bind("127.0.0.1:0").map_err(|e| format!("SS UDP 绑定失败：{e}"))?;
            socket
                .set_nonblocking(true)
                .map_err(|e| format!("SS UDP 非阻塞失败：{e}"))?;
            if super::ss2022::is_2022(method) {
                return Ok(Upstream::Ss2022 {
                    socket,
                    node: node_addr,
                    client: ss2022::UdpClient::new(method, password)?,
                });
            }
            Ok(Upstream::Ss {
                socket,
                node: node_addr,
                method: method.clone(),
                password: password.clone(),
            })
        }
        Protocol::Trojan { password } => {
            let stream = super::transport::connect_transport(&node)?;
            let stream = trojan::connect_udp(stream, password, &target.host, target.port)?;
            stream
                .set_read_timeout(Some(NODE_READ_TIMEOUT))
                .map_err(|e| format!("Trojan UDP 读超时设置失败：{e}"))?;
            Ok(Upstream::Trojan {
                stream,
                incoming: Vec::new(),
            })
        }
        Protocol::Vless { uuid, flow } => {
            let stream = super::transport::connect_transport(&node)?;
            stream
                .set_read_timeout(Some(NODE_READ_TIMEOUT))
                .map_err(|e| format!("VLESS UDP 读超时设置失败：{e}"))?;
            if vless::is_vision(flow) {
                // Vision 的服务端明确拒绝 `cmd=UDP`（`flow doesn't support UDP`）⇒ 必须 Mux + XUDP。
                let stream = vless::connect_mux(stream, uuid, flow)?;
                return Ok(Upstream::Xudp {
                    stream: super::xudp::XudpStream::new(stream),
                });
            }
            // 非 Vision（且本引擎不开 cone）：Xray 同口径直接 `cmd=UDP`，目标写在请求头里。
            let stream = vless::connect_udp(stream, uuid, &target.host, target.port)?;
            Ok(Upstream::VlessUdp {
                stream,
                host: target.host.clone(),
                port: target.port,
            })
        }
        Protocol::Vmess {
            uuid,
            alter_id,
            authenticated_length,
            ..
        } => {
            let stream = super::transport::connect_transport(&node)?;
            stream
                .set_read_timeout(Some(NODE_READ_TIMEOUT))
                .map_err(|e| format!("VMess UDP 读超时设置失败：{e}"))?;
            // Xray 的非 cone 路径就是 `cmd=UDP`：目标写在指令段里，一条连接一个目标。
            let tunnel = vmess::connect_udp(
                stream,
                uuid,
                *alter_id,
                *authenticated_length,
                &target.host,
                target.port,
            )?;
            Ok(Upstream::VmessUdp {
                tunnel,
                host: target.host.clone(),
                port: target.port,
            })
        }
        Protocol::AnyTls { password } => {
            // AnyTLS 的 UDP 走 UoT v2：会话目标 = 魔法域名，真正的目标写在 UoT 请求里。
            // 会话走池（与 TCP 共用同一个池，UoT 只是又一条 stream）。
            let key = anytls::session_key(
                &node.server,
                node.port,
                &node.tls.sni,
                node.transport(),
                node.tls.allow_insecure,
                password,
            );
            let stream = anytls::connect_pooled(
                &key,
                || super::transport::connect_transport(&node),
                password,
                uot::MAGIC_ADDRESS,
                0,
            )?;
            stream
                .set_read_timeout(Some(NODE_READ_TIMEOUT))
                .map_err(|e| format!("AnyTLS UDP 读超时设置失败：{e}"))?;
            let stream = uot::UotStream::connect(stream, &target.host, target.port)?;
            Ok(Upstream::Uot {
                stream,
                host: target.host.clone(),
                port: target.port,
            })
        }
        Protocol::Hysteria2 {
            password,
            obfs,
            obfs_password,
            insecure,
        } => {
            let config = super::hysteria2::Hysteria2Config {
                server: node.server.clone(),
                port: node.port,
                sni: node.tls.sni.clone(),
                insecure: *insecure || node.tls.allow_insecure,
                password: password.clone(),
                obfs: obfs.clone(),
                obfs_password: obfs_password.clone(),
            };
            // 目标写在每条 `UDPMessage` 里；服务端没开 UDP 时这里如实报错（不静默丢包）。
            let session = super::hysteria2::connect_udp(&config)?;
            Ok(Upstream::Hysteria2 { session })
        }
        Protocol::Tuic {
            uuid,
            password,
            udp_relay_mode,
        } => {
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
            // 目标写在每条 `Packet` 里；`udp-relay-mode: quic` 在 `tuic.rs` 里如实报错。
            let session = super::tuic::connect_udp(&config)?;
            Ok(Upstream::Tuic { session })
        }
    }
}

/// 解析 SOCKS5 地址（`ATYP ADDR PORT`），返回 `(host, port, 已消费字节数)`。
pub fn parse_socks_addr(bytes: &[u8]) -> Option<(String, u16, usize)> {
    let atyp = *bytes.first()?;
    let mut offset = 1usize;
    let host = match atyp {
        0x01 => {
            if bytes.len() < offset + 4 + 2 {
                return None;
            }
            let host = Ipv4Addr::new(
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            )
            .to_string();
            offset += 4;
            host
        }
        0x03 => {
            if bytes.len() < offset + 1 {
                return None;
            }
            let len = bytes[offset] as usize;
            offset += 1;
            if bytes.len() < offset + len + 2 {
                return None;
            }
            let host = String::from_utf8_lossy(&bytes[offset..offset + len]).to_string();
            offset += len;
            host
        }
        0x04 => {
            if bytes.len() < offset + 16 + 2 {
                return None;
            }
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&bytes[offset..offset + 16]);
            let host = Ipv6Addr::from(octets).to_string();
            offset += 16;
            host
        }
        _ => return None,
    };
    let port = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
    offset += 2;
    Some((host, port, offset))
}

/// 解析 `RSV RSV FRAG ATYP ADDR PORT DATA`；分片（FRAG != 0）与坏包返回 None。
pub fn parse_udp_datagram(bytes: &[u8]) -> Option<(Target, Vec<u8>)> {
    if bytes.len() < 4 || bytes[0] != 0 || bytes[1] != 0 {
        return None;
    }
    if bytes[2] != 0 {
        return None; // 分片不支持（v1 同口径）
    }
    let (host, port, used) = parse_socks_addr(&bytes[3..])?;
    Some((Target { host, port }, bytes[3 + used..].to_vec()))
}

/// 生成响应头（`RSV RSV FRAG ATYP ADDR PORT`）。
pub fn udp_header(from: SocketAddr) -> Option<Vec<u8>> {
    let mut out = vec![0u8, 0u8, 0u8];
    match from.ip() {
        IpAddr::V4(ip) => {
            out.push(0x01);
            out.extend_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            out.push(0x04);
            out.extend_from_slice(&ip.octets());
        }
    }
    out.extend_from_slice(&from.port().to_be_bytes());
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn datagram_round_trip_for_domain_and_ip() {
        let mut frame = vec![0, 0, 0, 0x03, 11];
        frame.extend_from_slice(b"example.com");
        frame.extend_from_slice(&53u16.to_be_bytes());
        frame.extend_from_slice(b"dns");
        let (target, payload) = parse_udp_datagram(&frame).unwrap();
        assert_eq!(target.host, "example.com");
        assert_eq!(target.port, 53);
        assert_eq!(payload, b"dns");

        let mut frame = vec![0, 0, 0, 0x01, 8, 8, 8, 8];
        frame.extend_from_slice(&53u16.to_be_bytes());
        frame.extend_from_slice(b"dns");
        let (target, payload) = parse_udp_datagram(&frame).unwrap();
        assert_eq!(target.host, "8.8.8.8");
        assert_eq!(target.port, 53);
        assert_eq!(payload, b"dns");

        let header = udp_header("8.8.8.8:53".parse().unwrap()).unwrap();
        assert_eq!(&header[..4], &[0, 0, 0, 0x01]);
        assert_eq!(&header[8..10], &53u16.to_be_bytes());
    }

    #[test]
    fn fragmented_or_truncated_datagrams_are_rejected() {
        let mut frame = vec![0, 0, 1, 0x01, 8, 8, 8, 8];
        frame.extend_from_slice(&53u16.to_be_bytes());
        frame.extend_from_slice(b"dns");
        assert!(parse_udp_datagram(&frame).is_none());
        assert!(parse_udp_datagram(&[0, 0, 0, 0x01, 8]).is_none());
        assert!(parse_udp_datagram(&[0, 0, 0, 0x09, 1, 2]).is_none());
        // IPv6：ATYP=4 + 16 字节 + 端口。
        let mut v6 = vec![0, 0, 0, 0x04];
        v6.extend_from_slice(&[0u8; 16]);
        v6.extend_from_slice(&53u16.to_be_bytes());
        v6.extend_from_slice(b"d");
        let (target, payload) = parse_udp_datagram(&v6).unwrap();
        assert_eq!(target.host, "::");
        assert_eq!(payload, b"d");
    }

    /// SS-SIP004 上游：假 UDP 节点解密请求、回一个加密响应。
    #[test]
    fn ss_udp_session_round_trips_against_a_mock_node() {
        let node = UdpSocket::bind("127.0.0.1:0").unwrap();
        let node_addr = node.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let mut buffer = [0u8; 65535];
            let (len, peer) = node.recv_from(&mut buffer).unwrap();
            let plain = ss::udp_decrypt("aes-256-gcm", "pw", &buffer[..len]).unwrap();
            let (host, port, used) = parse_socks_addr(&plain).unwrap();
            assert_eq!(host, "example.com");
            assert_eq!(port, 53);
            assert_eq!(&plain[used..], b"query");
            // 回一个「目标为本机」的数据报（内容随意，客户端只看地址与载荷）。
            let mut reply = super::super::encode_target("1.1.1.1", 53);
            reply.extend_from_slice(b"answer");
            let packet = ss::udp_encrypt("aes-256-gcm", "pw", &reply).unwrap();
            node.send_to(&packet, peer).unwrap();
        });

        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_nonblocking(true).unwrap();
        let mut upstream = Upstream::Ss {
            socket,
            node: node_addr,
            method: "aes-256-gcm".to_string(),
            password: "pw".to_string(),
        };
        upstream
            .send(
                &Target {
                    host: "example.com".to_string(),
                    port: 53,
                },
                b"query",
            )
            .unwrap();
        let mut received = None;
        for _ in 0..200 {
            if let Ok(Some(value)) = upstream.recv() {
                received = Some(value);
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        let (host, port, payload) = received.expect("必须收到模拟节点的回包");
        assert_eq!(host, "1.1.1.1");
        assert_eq!(port, 53);
        assert_eq!(payload, b"answer");
        handle.join().unwrap();
    }

    /// SS-2022 UDP 上游（SIP022 §3.2）：假节点按规范解独立头 / 会话子密钥，再回一个
    /// 带客户端会话 ID 的服务端包。
    #[test]
    fn ss2022_udp_session_round_trips_against_a_mock_node() {
        let psk_text = "MDEyMzQ1Njc4OWFiY2RlZg==";
        let psk = ss2022::decode_psk(psk_text, 16).unwrap();
        let node = UdpSocket::bind("127.0.0.1:0").unwrap();
        let node_addr = node.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let mut buffer = [0u8; 65535];
            let (len, peer) = node.recv_from(&mut buffer).unwrap();
            let (client_sid, plain) = ss2022::test_open_udp_packet(&psk, &buffer[..len]).unwrap();
            assert_eq!(plain[0], 0, "客户端包类型必须是 0");
            let padding = u16::from_be_bytes([plain[9], plain[10]]) as usize;
            let body = &plain[11 + padding..];
            let (host, port, used) = parse_socks_addr(body).unwrap();
            assert_eq!(host, "example.com");
            assert_eq!(port, 53);
            assert_eq!(&body[used..], b"query");
            let packet =
                ss2022::test_seal_udp_reply(&psk, [9u8; 8], client_sid, "1.1.1.1", 53, b"answer");
            node.send_to(&packet, peer).unwrap();
            packet
        });

        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_nonblocking(true).unwrap();
        let mut upstream = Upstream::Ss2022 {
            socket,
            node: node_addr,
            client: ss2022::UdpClient::new("2022-blake3-aes-128-gcm", psk_text).unwrap(),
        };
        upstream
            .send(
                &Target {
                    host: "example.com".to_string(),
                    port: 53,
                },
                b"query",
            )
            .unwrap();
        let mut received = None;
        for _ in 0..200 {
            if let Ok(Some(value)) = upstream.recv() {
                received = Some(value);
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        let (host, port, payload) = received.expect("必须收到模拟节点的回包");
        assert_eq!(host, "1.1.1.1");
        assert_eq!(port, 53);
        assert_eq!(payload, b"answer");
        // 同一个包再解一次必须被 64 位滑动窗口挡掉（`Ok(None)`，不是第二次成功）。
        let packet = handle.join().unwrap();
        let Upstream::Ss2022 { client, .. } = &mut upstream else {
            panic!("上游必须仍是 SS-2022");
        };
        assert!(
            client.open(&packet).unwrap().is_none(),
            "重放的包必须被丢弃"
        );
    }

    /// Trojan UDP 上游：假 TCP 节点校验握手与分帧，回一个同样分帧的数据报。
    #[test]
    fn trojan_udp_session_round_trips_against_a_mock_node() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // 握手：56 字符 hex + CRLF。
            let mut line = Vec::new();
            let mut byte = [0u8; 1];
            loop {
                stream.read_exact(&mut byte).unwrap();
                line.push(byte[0]);
                if line.ends_with(b"\r\n") {
                    break;
                }
            }
            assert_eq!(line.len(), 58);
            // 然后是 cmd 字节 + 地址 + CRLF。
            stream.read_exact(&mut byte).unwrap();
            assert_eq!(byte[0], 0x03, "UDP ASSOCIATE 的 cmd 必须是 0x03");
            let mut address = Vec::new();
            loop {
                stream.read_exact(&mut byte).unwrap();
                address.push(byte[0]);
                if address.ends_with(b"\r\n") {
                    break;
                }
            }
            assert_eq!(address[0], 0x03);
            assert_eq!(&address[2..13], b"example.com");

            // 一个 UDP 帧：ATYP addr port | len | CRLF | payload。
            let mut frame = Vec::new();
            loop {
                let mut chunk = [0u8; 256];
                let read = stream.read(&mut chunk).unwrap();
                if read == 0 {
                    return;
                }
                frame.extend_from_slice(&chunk[..read]);
                if let Some((host, port, payload, _used)) = trojan::parse_udp_frame(&frame) {
                    assert_eq!(host, "example.com");
                    assert_eq!(port, 53);
                    assert_eq!(payload, b"query");
                    let reply = trojan::udp_frame("1.1.1.1", 53, b"answer");
                    stream.write_all(&reply).unwrap();
                    return;
                }
            }
        });

        let tcp = TcpStream::connect(addr).unwrap();
        let stream: BoxStream = Box::new(tcp);
        let stream = trojan::connect_udp(stream, "pw", "example.com", 53).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(5)))
            .unwrap();
        let mut upstream = Upstream::Trojan {
            stream,
            incoming: Vec::new(),
        };
        upstream
            .send(
                &Target {
                    host: "example.com".to_string(),
                    port: 53,
                },
                b"query",
            )
            .unwrap();
        let mut received = None;
        for _ in 0..200 {
            if let Ok(Some(value)) = upstream.recv() {
                received = Some(value);
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        let (host, port, payload) = received.expect("必须收到模拟节点的回包");
        assert_eq!(host, "1.1.1.1");
        assert_eq!(port, 53);
        assert_eq!(payload, b"answer");
        handle.join().unwrap();
    }

    /// 非 Vision 的 VLESS UDP 上游：`cmd=UDP`（0x02）+ 每包长度前缀，
    /// 一条连接只服务一个目标，回包带的就是这个目标。
    #[test]
    fn vless_udp_session_uses_command_two_and_returns_its_target() {
        struct Mock {
            input: std::io::Cursor<Vec<u8>>,
            output: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
        }
        impl Read for Mock {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                self.input.read(out)
            }
        }
        impl Write for Mock {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.output.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        impl super::super::transport::Stream for Mock {}

        let mut response = vec![0u8, 0u8]; // version + addonLen(0)
        response.extend_from_slice(&5u16.to_be_bytes());
        response.extend_from_slice(b"reply");
        let written = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let stream = vless::connect_udp(
            Box::new(Mock {
                input: std::io::Cursor::new(response),
                output: written.clone(),
            }),
            "11111111-2222-3333-4444-555555555555",
            "example.com",
            53,
        )
        .unwrap();
        let mut upstream = Upstream::VlessUdp {
            stream,
            host: "example.com".to_string(),
            port: 53,
        };
        let target = Target {
            host: "example.com".to_string(),
            port: 53,
        };
        upstream.send(&target, b"query").unwrap();
        let bytes = written.lock().unwrap().clone();
        assert_eq!(bytes[18], 0x02, "command = UDP");
        assert_eq!(&bytes[bytes.len() - 7..], b"\x00\x05query");

        let (host, port, payload) = upstream.recv().unwrap().expect("必须解出模拟节点的回包");
        assert_eq!(host, "example.com");
        assert_eq!(port, 53);
        assert_eq!(payload, b"reply");
    }

    /// 一条 A 查询（`example.com`，ID 由调用方给）。
    fn dns_query(id: u16) -> Vec<u8> {
        let mut query = Vec::with_capacity(29);
        query.extend_from_slice(&id.to_be_bytes());
        query.extend_from_slice(&[0x01, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0]);
        query.extend_from_slice(b"\x07example\x03com\x00\x00\x01\x00\x01");
        query
    }

    /// 经 SOCKS5 UDP ASSOCIATE 发一个 DNS 查询，返回应答载荷。
    fn udp_round_trip_through_proxy(
        node: &super::super::Node,
        host: &str,
        port: u16,
        payload: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, String> {
        use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize};
        use std::sync::Arc;
        let shared = Arc::new(Shared {
            nodes: vec![node.clone()],
            rules: Vec::new(),
            active: AtomicUsize::new(0),
            alive: AtomicBool::new(true),
            mode: AtomicU8::new(0),
            sources: Vec::new(),
            fallback_proxy: true,
            up: AtomicU64::new(0),
            down: AtomicU64::new(0),
            geoip: None,
        });
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let proxy = listener.local_addr().map_err(|e| e.to_string())?;
        let handle = thread::spawn(move || {
            let (mut client, _) = listener.accept().unwrap();
            let mut greeting = [0u8; 3];
            client.read_exact(&mut greeting).unwrap();
            client.write_all(&[0x05, 0x00]).unwrap();
            let mut request = [0u8; 10];
            client.read_exact(&mut request).unwrap();
            let requested = Target {
                host: "0.0.0.0".to_string(),
                port: 0,
            };
            let _ = serve(&mut client, &shared, requested);
        });

        let mut control = TcpStream::connect(proxy).map_err(|e| e.to_string())?;
        control
            .set_read_timeout(Some(timeout))
            .map_err(|e| e.to_string())?;
        control
            .write_all(&[0x05, 0x01, 0x00])
            .map_err(|e| e.to_string())?;
        let mut method = [0u8; 2];
        control.read_exact(&mut method).map_err(|e| e.to_string())?;
        control
            .write_all(&[0x05, 0x03, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
            .map_err(|e| e.to_string())?;
        let mut bind = [0u8; 10];
        control.read_exact(&mut bind).map_err(|e| e.to_string())?;
        let udp_port = u16::from_be_bytes([bind[8], bind[9]]);

        let socket = UdpSocket::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        socket
            .set_read_timeout(Some(timeout))
            .map_err(|e| e.to_string())?;
        let mut frame = vec![0u8, 0u8, 0u8];
        frame.extend_from_slice(&super::super::encode_target(host, port));
        frame.extend_from_slice(payload);
        socket
            .send_to(&frame, ("127.0.0.1", udp_port))
            .map_err(|e| e.to_string())?;
        let mut buffer = vec![0u8; 4096];
        let (len, _) = socket
            .recv_from(&mut buffer)
            .map_err(|e| format!("UDP 回包超时：{e}"))?;
        let (_, answer) = parse_udp_datagram(&buffer[..len])
            .ok_or_else(|| "回包不是 SOCKS5 UDP 帧".to_string())?;
        drop(control);
        let _ = handle.join();
        Ok(answer)
    }

    /// **真机 UDP 验收（`#[ignore]`，需要外部服务器）**：读 `LUNAC_LIVE_LINKS`，逐节点经
    /// SOCKS5 UDP ASSOCIATE 查一次 `1.1.1.1:53`（`example.com` A 记录），打印结果矩阵。
    ///
    /// 只对「协议支持 UDP」的节点有意义；SS-2022 多用户（SIP023）按规范不支持 UDP，
    /// 会如实打印失败原因（不是缺陷）。
    #[test]
    #[ignore]
    fn live_udp_nodes_answer_dns() {
        let path = match std::env::var("LUNAC_LIVE_LINKS") {
            Ok(value) if !value.trim().is_empty() => value,
            _ => {
                eprintln!("跳过：没有设置 LUNAC_LIVE_LINKS");
                return;
            }
        };
        let text = std::fs::read_to_string(&path).expect("读不到链接文件");
        let parsed = super::super::clash::parse_subscription(&text);
        let timeout = Duration::from_secs(8);
        let mut passed = 0usize;
        for (index, node) in parsed.nodes.iter().enumerate() {
            let id = 0x1000 + index as u16;
            let started = Instant::now();
            let outcome =
                udp_round_trip_through_proxy(node, "1.1.1.1", 53, &dns_query(id), timeout)
                    .and_then(|answer| {
                        if answer.len() < 12 {
                            return Err(format!("应答过短（{} 字节）", answer.len()));
                        }
                        if answer[0..2] != id.to_be_bytes() {
                            return Err("应答的事务 ID 与请求不一致".to_string());
                        }
                        if answer[2] & 0x80 == 0 {
                            return Err("应答没有 QR 位（不是 DNS 响应）".to_string());
                        }
                        Ok(())
                    });
            let millis = started.elapsed().as_millis();
            match outcome {
                Ok(()) => {
                    passed += 1;
                    println!(
                        "PASS  {:<28} {:>6} ms  {}",
                        node.name,
                        millis,
                        node.protocol.kind()
                    );
                }
                Err(reason) => {
                    println!(
                        "FAIL  {:<28} {:>6} ms  {} => {}",
                        node.name,
                        millis,
                        node.protocol.kind(),
                        reason
                    );
                }
            }
        }
        println!("结果：{passed}/{} 通过", parsed.nodes.len());
        assert!(passed > 0, "一个节点都没通，先看上面的 FAIL 原因");
    }
}
