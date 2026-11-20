# Lunac 验收地图（实际取证版，2026-11-01）

> **本文档是什么**：一张「**能力 → 代码 → 自动化用例（具体测试名）→ 真机条目 → 状态**」的对照表。
> 只登记**能指到证据**的项，不写「计划支持」。
> **2026-11-01 回填**：§四 每条补齐「具体步骤」与「真机成功的体现（判据 ⇒ 留什么证据）」——
> 状态格里的 ✅ 必须能对上这两列，看文案 / 看返回值都算证据，但**必须落盘**（见 §四.0）。
> 三份文档的分工：
>
> | 文档 | 管什么 |
> |---|---|
> | [`todo.md`](./todo.md) | **还剩什么没做**（未完成 / 未验证） |
> | [`testing.md`](./testing.md) | **怎么跑**（命令、步骤、判据） |
> | **本文** | **已经证明了什么**（哪条能力、靠哪个测试、真机跑到第几条） |
>
> [`acceptance-roadmap.md`](./acceptance-roadmap.md) 是 2026-10-22 **宿主全量 47 条的批次执行顺序**（怎么排、先跑谁）；
> 本文只做**账本**，且当前仅覆盖自研代理引擎这一块。两者不冲突：路线图管排队，本文管结论。

## 一、状态图例

| 符号 | 含义 |
|---|---|
| ✅ | 自动化绿 **且** 真机绿（真机绿必须带日期 / 证据路径，且能对上 §四 的「具体步骤」与「成功体现」两列） |
| 🟡 | 自动化绿，真机**未跑** —— 当前 `native_proxy` 绝大多数项处于此态 |
| ⬜ | 已实现但**没有任何测试**覆盖（只能靠真机） |
| ❌ | 明确不做（如实记录的边界，不是缺陷） |

> **纪律**：🟡 不许写成 ✅。「代码看着对」不算证据；真机验收一律按 [`testing.md`](./testing.md) §测试 27 的判据。

## 二、基线（2026-10-31 实跑）

```powershell
cd app\src-tauri
cargo check --bin lunac
cargo test --bin lunac native_proxy     # 111 passed / 2 ignored（ignored 是真机用例）
cargo test --bin lunac native_proxy -- --list   # 取准确测试名，用于维护下表
```

| 模块 | 用例数 |
|---|---|
| `clash::tests` | 24 |
| `ss_plugin::tests` | 5 |
| `ss::tests` | 8 |
| `rules::tests` | 6 |
| `hysteria2::tests` | 8 |
| `hysteria2_udp::tests` | 4 |
| `reality::tests` | 8 |
| `vmess::tests` | 4 |
| `server::tests` | 3 |
| `vless::tests` | 9 |
| `vless_udp::tests` | 4 |
| `vmess_udp::tests` | 3 |
| `trojan_udp::tests` | 3 |
| `anytls_udp::tests` | 3 |
| `anytls::tests` | 2 |
| `transport::tests` | 2 |
| `http_obfs::tests` | 2 |
| `h2tunnel::tests` | 9 |
| `udp::tests` | 2 |
| 根 `tests`（端到端） | 2 |
| `trojan::tests` | 2 |
| **合计** | **113**（含 2 条 `#[ignore]` 真机用例） |

## 二bis、v2 重建状态（2026-11-15）

> **背景**：v1 `app/src-tauri/src/native_proxy/`（下表引用的 21 个文件）已随目录误删**永久丢失**
> —— `git archive 4532bed`（11-05 stash 快照）只含已跟踪文件，`D:\cc` 全盘搜索 0 结果。
> **本文件的 §三 从此只作「能力账本」**（引用路径保留为历史坐标，不再是可运行的代码位置）；
> v2 的实际实现在 `app-flutter/rust/src/native_proxy/`，逐项重建状态见下表。
>
> 验证命令（v2）：
> ```powershell
> cd app-flutter\rust; cargo test --lib native_proxy      # 200 passed / 5 ignored（2026-11-18）
> cd app-flutter\rust; cargo test --lib                    # 590 passed / 6 ignored（2026-11-18）
> cd app-flutter; flutter analyze; flutter test           # 0 issue / 308 passed（2026-11-18）
> cd app-flutter; flutter build windows --debug            # 通过（2026-11-15，lunac.exe）
> ```
>
> **真机全协议验收（2026-11-15，`43.103.55.150`：Xray 26.10.31 / v2fly 5.53.0 / sing-box 1.14.4，
> 自签 `CN=lunac.test`，端口 80/443/8081-8084/8443-8448/8450/8451 公网可达；另起 Xray 25.6.8 的
> REALITY loopback 节点 `127.0.0.1:28459` 作为第 19 条）**
> 跑法：`$env:LUNAC_LIVE_LINKS="temp\live-links.txt"` → `cargo test --lib live_nodes -- --ignored --nocapture`
>（链接与口令只在本机 `temp/`，不入库）。结果 **19/19**（2026-11-15 首跑 18/19；2026-11-16 补上
> `X25519MLKEM768` 后全绿）：
> ✅ SS legacy aes-256-gcm 446ms ｜ ✅ SS+obfs http 152ms ｜ ✅ SS+obfs tls 156ms ｜
> ✅ **SS-2022 多用户（SIP023 EIH）152ms** ｜ ✅ VLESS 裸 TCP 145ms ｜ ✅ Trojan TLS 276ms ｜
> ✅ VLESS Vision 321ms ｜ ✅ VLESS WS 393ms ｜ ✅ **VLESS gRPC 242ms** ｜ ✅ VLESS XHTTP 717ms ｜
> ✅ VMess AEAD WS 294ms ｜ ✅ AnyTLS 240ms ｜ ✅ **Hysteria2 salamander 418ms / gecko 302ms** ｜
> ✅ VLESS h2（v2fly，Xray 26 已移除 h2）839ms ｜ ✅ VMess 老式 MD5 头（aid=64，v2fly，
> 需 `v2ray.vmess.aead.forced=false`）234ms ｜ ✅ VMess AEAD TCP+TLS 1022ms ｜
> ✅ **VLESS + REALITY（Xray 26.10.31，公网 443）372ms** ｜
> ✅ **VLESS + REALITY（Xray 25.6.8，loopback 28459）428ms** ｜
> 另单跑自定义 CA（同一 Vision 节点，`ca=` 内联 base64 PEM、不开 `allowInsecure`）：**1/1 通过 238ms**。
> 2026-11-16 复跑（19 条全绿，`live-links.txt` 原样）：SS 228ms ｜ obfs-http 1371ms ｜ obfs-tls 203ms ｜
> SS-2022 169ms ｜ VLESS 158ms ｜ Trojan 316ms ｜ Vision 259ms ｜ WS 335ms ｜ gRPC 311ms ｜ XHTTP 398ms ｜
> VMess WS 352ms ｜ AnyTLS 244ms ｜ hy2 salamander 381ms / gecko 697ms ｜ **REALITY（26，443）372ms** ｜
> h2 481ms ｜ VMess legacy 285ms ｜ VMess TCP+TLS 604ms ｜ REALITY（25，28459）483ms。
> 另外把第 19 条的同一 REALITY 配置换到 Xray **26.10.31** loopback（`127.0.0.1:28457`）单跑：**1/1 通过 361ms**，
> 服务端日志 `proxy/vless/inbound: received request for tcp:www.baidu.com:80` + `freedom: connection opened`。
>
> **真机 UDP 验收（2026-11-16，同一批 19 条链接）**：新增 `live_udp_nodes_answer_dns`
> （`$env:LUNAC_LIVE_LINKS=…` → `cargo test --lib live_udp_nodes_answer_dns -- --ignored --nocapture`），
> 每个节点经 **SOCKS5 UDP ASSOCIATE** 向 `1.1.1.1:53` 查一次 `example.com`，校验事务 ID 与 QR 位。
> 结果 **15/19**：✅ VLESS 裸 TCP 433ms ｜ ✅ Trojan 496ms ｜ ✅ **Vision（XUDP）482ms** ｜
> ✅ VLESS WS 542ms ｜ ✅ gRPC 369ms ｜ ✅ XHTTP 532ms ｜ ✅ VMess WS 543ms ｜ ✅ **AnyTLS（UoT v2）467ms** ｜
> ✅ **Hysteria2 salamander 464ms / gecko 452ms** ｜ ✅ **REALITY（26，443）498ms** ｜
> ✅ h2 690ms ｜ ✅ VMess legacy 681ms ｜ ✅ VMess TCP+TLS 748ms ｜ ✅ REALITY（25，28459）527ms。
> 未过的 4 条各有明确原因（不是「再试一次」）：
> ① `ss-legacy`（8081）与 ② `ss-obfs-http` / ③ `ss-obfs-tls`：前者的 UDP 包**根本没到服务器**
> —— 在 VPS 上 `tcpdump -ni any udp port 8081` 零包，说明阿里云安全组只放行了 8443/8444 的 UDP
> （Hysteria2 两条正是走这两个端口才通）；后两者是 `simple-obfs` 只承载 TCP，UDP 按设计拒绝并打一行日志；
> ④ `ss2022-multiuser`：SIP023 的 EIH 只用于 TCP，多用户 PSK 按规范不支持 UDP（已如实拒绝）。
> 另把 SS 单用户 UDP 放 443/udp 临时入站对照：tcpdump 同样零包 ⇒ 仍是安全组未放行，代码侧无法在本机验完。
>
> 真机修掉的根因（①~⑦；①~⑤ 2026-11-15，⑥~⑦ 2026-11-16；都有服务端日志 / 逐字节对照为证）：
> ① **Hysteria2**：认证完把 `h3` 的 `SendRequest` 句柄丢了 —— h3 计数归零会主动 `close` 整条 QUIC
> 连接（sing-box 侧 `Application error 0x100 (remote): Connection closed by client`），表现为
> 「认证 233 通过后 open_bi 立刻 closed / connection lost」；
> ② **Hysteria2 + 混淆**：Gecko 慢到 4~13s 是 loopback 中继单线程轮流读两个 socket（200ms 读超时
> ⇒ 每个方向最多等 200ms，分片的包要等好几轮）；改成两个方向各一条线程后 300~480ms，且 `Drop`
> 时置 `stop` 不再泄漏线程；
> ③ **SS-2022**：变长头里 padding 恒为 0 且没有 early data —— SIP022 §3.1.4 要求二者不能同时为空，
> Xray 回 `request without payload and padding is not allowed`；改成随机 `1..=900` 字节 padding；
> ④ **gRPC**：响应头必须推迟到第一次读（grpc-go 收到首个消息前不发头，原实现先等头 ⇒ 互相等死
> 12s 超时），且消息正文必须是 `Hunk{bytes data = 1}` protobuf（`0A <varint len> <data>`），
> 发裸字节会被判 `cannot parse invalid wire-format data`；
> ⑤ **REALITY**：`supported_versions` 扩展少了 1 字节列表长度（被读成空列表 ⇒ `alert 70`）、
> 认证 ECDH 与 TLS 1.3 的 ECDHE 必须是**同一把**临时私钥却只算了一次、CertificateVerify 的
> transcript 误把自己算进去、客户端 Finished 只发了 32 字节 verify_data 而没带握手头。
> ⑥ **REALITY × Xray 26（2026-11-16）**：26 起要求 ClientHello 先带 `X25519MLKEM768`
> （IANA 4588）再带 X25519，缺则 `XTLS/reality` 直接 `peerPub2 == nil` 跳出、把连接当「真站点」转发。
> 现在同时发两组 key_share：混合组的 key_share 数据 = `ML-KEM-768 封装密钥(1184) ‖ x25519 公钥(32)`，
> 而 TLS shared secret 的拼接顺序相反 = `ML-KEM 共享密钥(32) ‖ x25519 共享密钥(32)`
> （对齐 `XTLS/reality/key_schedule.go` 的 `clientSharedSecret`）；服务端选混合组就按 1120 字节
> `密文(1088) ‖ x25519 公钥(32)` 解封。单测 `client_hello_carries_the_hybrid_post_quantum_share_first`。
> ⑦ **Vision 下行填充帧（2026-11-16，UDP 验收才发现）**：原先以为「下行每帧都带 uuid」，实际
> Xray `proxy.XtlsPadding` 写完首帧就把 `writeOnceUserUUID` 置空 ⇒ **uuid 每方向只出现一次**，
> 之后全是 5 字节头 `cmd | contentLen u16 | paddingLen u16`。TCP 恰好只用到首帧（`cmd=1` 之后转裸流）
> 所以一直是绿的；XUDP / Mux 这类非 TLS 流量服务端**一直发 `cmd=0` 的填充帧**，
> 第二帧起就撞 `uuid 不匹配`（逐字节 dump 到 `uuid | 00 | 0010 | 0068 | …` 与随后的 5 字节帧才定性）。
> 修法：`vision_uuid_seen` 只校验首帧 uuid，后续按 5 字节头解析；上行拆帧时 uuid 也只写第一帧。
>
> 第 19 条（REALITY）怎么起：服务器上 `systemd-run --unit=lunac-reality25 /tmp/x25/xray run
> -c /tmp/xray25-reality.json`（Xray 25.6.8，`listen 127.0.0.1:18459`，与 443 同名
> `privateKey` / `shortIds`），本机 `ssh -N -L 28459:127.0.0.1:18459 root@…`；
> 配置文件模板在 `temp/reality.json`（只改端口）。Xray 26 的对照节点同法：
> `systemd-run --unit=lunac-reality --property=Restart=no /usr/local/bin/xray run -c /tmp/xray-reality.json`
> （`listen 127.0.0.1:18457`），本机 `ssh -N -L 28457:127.0.0.1:18457 root@…`。

| §三 行 | 能力 | v2 实现 | v2 自动化用例 | 状态 |
|---|---|---|---|---|
| 1 | 入站 SOCKS5 CONNECT + HTTP CONNECT / 绝对 URI（同端口） | `native_proxy/server.rs` | `socks5_end_to_end_in_direct_mode`、`http_connect_end_to_end_in_direct_mode`、`absolute_uri_is_split_into_authority_and_path`、`read_timeout_on_boxed_stream_reaches_the_socket` | 🟡 |
| 2 | SOCKS5 UDP ASSOCIATE（直连 + 经节点；控制连接生命周期绑定） | `native_proxy/udp.rs` | `datagram_round_trip_for_domain_and_ip`、`fragmented_or_truncated_datagrams_are_rejected`、`ss_udp_session_round_trips_against_a_mock_node`、`ss2022_udp_session_round_trips_against_a_mock_node`、`trojan_udp_session_round_trips_against_a_mock_node`、`vless_udp_session_uses_command_two_and_returns_its_target`、`udp_session_uses_command_two_and_chunk_per_datagram`（VMess）、`connect_request_and_datagram_framing` / `request_uses_socks_address_types` / `partial_datagram_returns_none`（AnyTLS 的 UoT v2）、`xudp_round_trip_through_a_mock_vless_mux_server`；**真机** `live_udp_nodes_answer_dns`（`#[ignore]`） | ✅（**2026-11-16 真机 15/19**：VLESS / Vision-XUDP / Trojan / VMess（两代头）/ AnyTLS-UoT / Hysteria2（含 gecko）/ REALITY / WS / gRPC / XHTTP / h2 全部经 SOCKS5 UDP ASSOCIATE 查到 DNS；唯一的 3 条 SS 是**安全组未放行 UDP**（tcpdump 零包）与 `simple-obfs` 只承载 TCP，SS-2022 多用户按 SIP023 规范不支持 UDP） |
| 2b | UDP 上游：SS-SIP004（每包独立 salt + 子密钥、nonce=0）、SS-2022（SIP022 §3.2：AES 分组密码独立头 + 会话子密钥 + 12B nonce + 64 位滑动窗口 + 30s 时间戳闸 + 60s 内拒换会话）、**VLESS（按 Xray 分派：非 Vision 直接 `cmd=0x02` + 头里带目标 + 每包 `[len u16 BE][payload]`；Vision（或 cone）才转 Mux `v1.mux.cool` + XUDP `metaLen|session|status|option|network|port→addr|globalID|dataLen|data`）**、**VMess（`cmd=0x02` + 目标在指令段里，一个标准分块 = 一个数据报）**、**AnyTLS（UoT v2 connect 模式：魔法域名 `sp.v2.udp-over-tcp.arpa` + `isConnect=1 | SOCKS 目标` + `[len u16][payload]`）**、**TUIC v5（QUIC datagram + `[5,2]` 头 + 地址 + 载荷，分片重组）** 与 Trojan（`ATYP addr port ‖ u16 len ‖ CRLF ‖ payload`） | `native_proxy/udp.rs` 的 `Upstream` + `ss::udp_encrypt/udp_decrypt` + `ss2022::UdpClient` + `vless::VlessUdpStream` / `xudp::XudpStream` + `vmess::VmessTunnel::send_datagram/recv_datagram` + `uot::UotStream` + `tuic::TuicUdp` + `trojan::connect_udp/udp_frame/parse_udp_frame` | `sip004_udp_round_trip_and_wrong_password_fails`、`udp_round_trip_and_replay_window`、`udp_rejects_stale_timestamp_foreign_sid_and_session_switch`、`udp_rejects_chacha_methods_with_a_reason`、`udp_request_uses_command_two_and_length_prefixed_body`（`cmd=0x02` + 端口在地址之前 + 长度前缀，响应头被跳过）、`vless_udp_session_uses_command_two_and_returns_its_target`、`udp_session_uses_command_two_and_chunk_per_datagram`（VMess 的 `cmd=0x02` + 一个分块一个包）、`xudp_round_trip_through_a_mock_vless_mux_server`（Vision 的 `cmd=Mux` 头不带地址 + XUDP 首帧逐字节）、`address_encoding_is_port_then_xray_atyp`、`keepalive_frames_are_skipped`、`udp_associate_header_and_frame_layout`、上表四条 mock 节点端到端 | ✅ / 🟡（**2026-11-16 真机**：VLESS（含 Vision-XUDP / REALITY）/ Trojan / VMess 两代头 / AnyTLS-UoT / Hysteria2 全部查到 DNS；**SS 的 SIP004 UDP 未验完** —— 服务器安全组只放行 8443/8444 的 UDP，`tcpdump udp port 8081/443` 零包，需要用户放行端口后才能补测） |
| 3 | SS AEAD（aes-128-gcm / aes-256-gcm / chacha20-ietf-poly1305） | `native_proxy/ss.rs` | `cipher_names_are_stable`、`evp_bytes_to_key_matches_openssl_vector`、`md5_matches_rfc1321_vectors`、`chunks_seal_and_open_round_trip`、`tampered_ciphertext_fails_authentication`、`nonce_is_little_endian_counter`、`fragmented_reads_survive_partial_chunks` | ✅（**2026-11-15 真机**：aes-256-gcm 446ms；obfs http 152ms / tls 156ms 见 5d） |
| 4 / 5 | SS-2022 单用户（SIP022：`blake3::derive_key` 子密钥、带类型 / 时间戳 / 请求 salt 的头块、12 字节小端计数器；aes-128-gcm / aes-256-gcm / chacha20-poly1305） | `native_proxy/ss2022.rs` | `method_dispatch_and_key_lengths`、`psk_must_be_base64_of_the_exact_key_length`、`nonce_is_little_endian_counter`、`session_key_depends_on_both_psk_and_salt`、`tcp_round_trip_against_a_mock_server`（本地回环端到端）、`response_with_wrong_request_salt_is_rejected` | ✅（**2026-11-15 真机**：单用户 loopback（Xray 26 单 key 入站）160ms；公网多用户 152ms 见 5b） |
| 5b | SS-2022 多用户（SIP023 EIH：salt 后逐层 16 字节 identity header，会话子密钥用最后一段 uPSK；UDP 明确不支持） | `native_proxy/ss2022.rs` 的 `decode_psks` / `identity_headers` | `multi_user_psk_sends_an_identity_header`（mock 服务端解密 identity 块并核对 `blake3(uPSK)[..16]`，再用 uPSK 走完整握手）、`multi_user_psk_is_rejected_for_udp_with_a_reason`、`psk_must_be_base64_of_the_exact_key_length` | ✅（**2026-11-15 真机 152ms**：Xray 26 的 `password: iPSK` + `clients[].password: uPSK` 多用户入站；根因是变长头缺 padding，见 §二开头 ③） |
| 5d | SIP003 `simple-obfs`（`obfs=http` / `obfs=tls`，逐字节对齐 `obfs_http.c` / `obfs_tls.c`；假 TLS 不与真 TLS 叠加；UDP 明确不转发） | `native_proxy/ss_plugin.rs` + `server.rs` 的 `dial_node` | `http_obfs_prepends_an_upgrade_request_and_strips_the_response`、`tls_obfs_hides_the_first_payload_in_the_session_ticket`（138 字节 ClientHello / session_ticket / SNI / 66 字节固定扩展逐字节校验）、`unknown_modes_are_rejected_with_a_reason`、`malformed_tls_frames_are_rejected`、`ss_simple_obfs_maps_to_obfs_options` | ✅（**2026-11-15 真机**：8082 obfs=http 152ms / 8083 obfs=tls 156ms） |
| 5c | SIP003 插件 `v2ray-plugin` 的 **websocket** 模式（整条 SS 流塞进 WS 二进制帧；`tls` 为裸标志） | `native_proxy/clash.rs` `parse_ss_plugin` + `transport.rs` 的 WS | `ss_v2ray_plugin_websocket_maps_to_ws_and_tls`（含 `mode=quic` 明确拒绝）、`websocket_handshake_and_echo` | 🟡（解析 + 传输层各有覆盖；真机 204 待补） |
| 6 | Trojan | `native_proxy/trojan.rs` | `handshake_head_matches_trojan_spec`（SHA-224 标准向量） | ✅（**2026-11-15 真机**：8443 自签 276ms） |
| 7 | VLESS（响应头消费；地址类型 1/2/3） | `native_proxy/vless.rs` | `request_header_matches_vless_layout`、`request_header_uses_xray_address_types`、`uuid_is_parsed_from_hex_with_dashes`、`response_header_is_consumed_before_payload` | ✅（**2026-11-15 真机**：8084 裸 TCP 145ms） |
| 7c | VLESS Vision（`flow=xtls-rprx-vision`：请求头 addon `0A 10 "xtls-rprx-vision"`；上行首帧 `uuid|cmd|clen|plen|content|padding` 之后转裸流；下行逐帧解填充；响应头推迟到首次读，避免服务端等首段数据时死锁） | `native_proxy/vless.rs` 的 `VlessStream` / `request_header` | `vision_header_carries_flow_addons`、`vision_first_write_is_framed_and_downlink_unpadded`（逐字节校验首帧并解两帧 + 裸流）、`vision_frame_with_foreign_uuid_is_rejected`、`response_header_is_consumed_before_payload` | ✅（**2026-11-15 真机**：8444 Vision 321ms；**2026-11-16 修下行填充帧**：`uuid` 每方向只出现一次，之后是 5 字节头 `cmd｜contentLen｜paddingLen`，XUDP 场景服务端一直发 `cmd=0` 的填充帧 —— 见 §二开头 ⑦；真机 8444 的 UDP 查询 482ms 通过） |
| 8 | VMess（**AEAD 头（`alterId=0`）+ 老式 MD5 头（`alterId≠0`）**；标准分块体 `security=3`；**声明 `Opt S|M|P`**；**`Opt A`（认证长度，YAML `authenticated-length`，默认关）**） | `native_proxy/vmess.rs` | `kdf_matches_upstream_vector`（v2ray 官方向量）、`cmd_key_matches_upstream_definition`（`MD5(uuid‖magic)`，.NET 独立向量）、`auth_id_carries_timestamp_and_crc`、`instruction_section_layout_matches_doc`、`chunk_nonce_is_big_endian_counter`、**`chunk_masking_and_padding_follow_the_shake128_stream`**（`SHAKE128(0×16)` 前 8 字节与 Node/OpenSSL 独立实现一致）、**`masked_writer_hides_the_length_and_appends_cleartext_padding`**、**`partial_chunk_does_not_advance_the_shake_stream_twice`**（半包不得重复推进 XOF）、`aead_header_and_body_round_trip_against_a_mock_server`（本地回环端到端，含掩码 / 填充）、`legacy_auth_id_and_header_iv_match_reference`（.NET HMACMD5/MD5 向量）、`cfb128_follows_the_definition_and_round_trips`、`legacy_header_and_body_round_trip_against_a_mock_server`（`alterId=2` 端到端）、**`authenticated_length_field_is_sealed_with_the_auth_len_subkey`**（18 字节长度字段只能由 `auth_len` 子密钥解开、体密钥解不开）、**`authenticated_length_round_trips_against_a_mock_server`**（`Opt A` 双向端到端，响应长度用请求的 key / IV）、**`clash_yaml_reads_vmess_authenticated_length`** | ✅（**2026-11-15 真机**：AEAD ws+TLS 294ms / AEAD TCP+TLS 1022ms / 老式 MD5 头（aid=64，v2fly）234ms；当天修掉 `CmdKey` 少拼 magic 的根因。**2026-11-16 补 `Opt M|P`**（SHAKE128 掩码 + 每块 0..63 字节明文填充，口径对齐 v2fly `encoding/auth.go` 的 `ShakeSizeParser`：先取 `next()%64` 当填充、再取 `next()` 异或长度字段），真机 3/3：AEAD ws+TLS 1360ms / 老式 aid=64 167ms / AEAD TCP+TLS 471ms。**2026-11-16 补 `Opt A`**（认证长度：18 字节长度字段 AEAD，`auth_len` 子密钥，长度 / 体各一条独立计数器），真机 v2fly **73ms**（`live_authenticated_length_node_answers`，`#[ignore]`）） |
| 10 / 11 | Hysteria2（含 salamander / gecko）**TCP + UDP** | `native_proxy/hysteria2.rs`、`native_proxy/udp.rs` 的 `Upstream::Hysteria2` | `varint_matches_rfc9000_examples`、`varint_round_trips`、`authority_brackets_ipv6`、`tcp_request_frame_matches_spec_layout`（0x401 帧逐字节）、`salamander_round_trips_and_layout_is_salt_then_payload`（`BLAKE2b-256(key+salt)` + `[8B salt][payload]`）、`gecko_frame_and_split_round_trip`、`gecko_rejects_bad_frames_and_passes_short_header`、`udp_message_round_trips_and_rejects_short_buffers`、`udp_fragments_send_one_message_when_it_fits`、`udp_fragments_split_and_reassemble_in_order`、`udp_reassembly_drops_incomplete_and_inconsistent_packets`、`split_authority_handles_domains_ipv4_and_brackets`、`config_falls_back_to_server_for_sni` | ✅（**2026-11-15 真机：salamander 418ms / gecko 302ms**（sing-box 1.14.4，UDP 8443/8444）。当天修掉两处：`h3::client::SendRequest` 句柄不能丢（丢了 h3 会 close 整条 QUIC 连接）+ 混淆中继改双线程；**2026-11-16 UDP 转发真机通过**：salamander 464ms / gecko 452ms（经 SOCKS5 UDP ASSOCIATE 查 DNS）） |
| 9 | AnyTLS（认证包 / 会话帧 / 上游默认 paddingScheme / `cmdUpdatePaddingScheme` 采纳；单 stream，不做会话池与多流复用） | `native_proxy/anytls.rs` | `default_padding_scheme_matches_upstream`、`auth_packet_carries_sha256_and_padding`、`frame_layout_matches_spec`、`session_round_trip_against_a_mock_server`（本地回环端到端：认证 → settings/SYN/PSH → padding → PSH/FIN）、`padding_scheme_from_server_is_adopted_for_later_sessions` | ✅（**2026-11-15 真机**：8451 自签 240ms） |
| 12 | 传输 TCP / TLS（**含自定义根证书 `ca`**）/ WebSocket / **HTTP 混淆（`network: http`）** | `native_proxy/transport.rs` 的 `connect_http` / `HttpTunnel` / `wrap_tls`（`pem_blocks` 逐张 `add_root_certificate`） | `websocket_handshake_and_echo`、`websocket_length_encoding_covers_three_widths`、`http_obfs_handshake_and_echo`（逐字节断言 `GET /obfs HTTP/1.1` + `Host`，200 之后双向透传）、`http_obfs_rejects_non_200`、`concatenated_pem_certificates_are_split_into_blocks`、`a_bad_ca_is_reported_in_plain_words_before_connecting` | 🟡（HTTP 混淆 / 自定义 CA 本地通过；**自签 CA 服务端真机 204 待补**） |
| 12b / 12c / 12d | **H2 / gRPC**（`network: h2` / `grpc`）、**XHTTP（stream-one + stream-up）** | `native_proxy/h2tunnel.rs`（h2 / gRPC / XHTTP） | `grpc_frame_has_the_five_byte_header`、`grpc_deframe_round_trips_and_keeps_partial_frames`、`grpc_deframe_ignores_short_input`、`grpc_messages_carry_a_hunk_protobuf_body`、`authority_prefers_h2_host_and_defaults_to_server`、`h2_and_grpc_are_parsed_from_links_and_yaml`、`runtime_runs_spawned_tasks_with_time`、`h2_tunnel_echoes_over_plain_loopback`、`grpc_tunnel_echoes_with_framing`、**`xhttp_parts_and_padding_follow_xray_rules`**、**`xhttp_stream_one_echoes_with_padding`**、**`xhttp_stream_up_splits_upload_and_download`**、**`xhttp_is_parsed_from_links_and_yaml`** | 🟡（**2026-11-15 重建**：一条 HTTP/2 流承载整条代理连接；gRPC = `[0][u32 len]` 头 + `Hunk{data}` protobuf，且响应头推迟到首次读（grpc-go 收到首个消息前不发头）；XHTTP 按 Xray `splithttp` 源码对齐 —— `stream-one` = 单 POST、`stream-up` = `POST/GET <path>/<uuid>`、每请求必带 `Referer …?x_padding=100..1000 个 X`（缺则服务端 400）。写侧 `reserve_capacity` → `poll_capacity`；读侧到期返回 `WouldBlock`。**真机：h2 522ms / gRPC 309ms（loopback）· 266ms（公网 8447）/ XHTTP 277ms 全部通过**；`packet-up` / `xmux` 未做） |
| 12e | VLESS / Trojan + **REALITY**（自写 TLS 1.3：ClientHello 的 SessionId 里加密塞版本+时间戳+短 ID，临时证书的签名位 = `HMAC-SHA512(AuthKey, Ed25519 公钥)`） | `native_proxy/reality.rs`（`handshake` / `build_client_hello` / `seal_session_id` / `verify_temporary_certificate` / `RealityStream`） | `session_id_layout_and_offset`、`seal_uses_zeroed_session_id_as_ad`、`client_hello_declares_tls13_and_key_share`（逐字节断言 `002b 0003 02 0304`，含 1 字节列表长度）、`expand_label_prefixes_tls13_and_carries_context`、`der_tlv_reads_long_form_length`、`temporary_certificate_is_accepted_only_with_the_right_auth_key`、`leaf_certificate_skips_context_and_reads_first_entry`、`server_hello_parsing_rejects_unknown_group_and_missing_share`、`client_hello_carries_the_hybrid_post_quantum_share_first`、`vless_reality_link_and_yaml_are_parsed_and_ws_is_rejected`（链接 / YAML 解析 + 与 ws 叠用如实拒绝）；**真网**：`real_tls13_server_completes_handshake_then_fails_the_reality_check`（`#[ignore]`，对 `www.cloudflare.com:443` 走完整 TLS 1.3 握手） | ✅（**2026-11-15 重建 + 真机**：Xray **25.6.8** 的 REALITY 节点 **428ms 通过**（VLESS+Vision 全链路）；当天修掉四个根因 —— `supported_versions` 少 1 字节长度（`alert 70`）、认证 ECDH 与 TLS ECDHE 必须共用一把临时私钥（改用 `x25519-dalek`，ring 的 `agree_ephemeral` 只能算一次）、CertificateVerify 的 transcript 不能含自己、客户端 Finished 必须带握手头。**2026-11-16：Xray 26.10.31 也通过** —— 公网 443 **372ms**、loopback 28457 **361ms**，靠 ClientHello 同时携带 `X25519MLKEM768` 与 `X25519` 两组 key_share（见 §二开头 ⑥）；仍未做 uTLS 指纹（JA3/JA4）与 HelloRetryRequest） |
| 13 | 规则 `DOMAIN*` / `IP-CIDR*` / **`PROCESS-NAME`（按发起进程分流，2026-11-15）** / **`GEOIP`（本地 MaxMind DB，2026-11-16）** / `MATCH` + 本机 / 私网直连；**插件名单层（黑 / 白名单，2026-11-16）** | `native_proxy/rules.rs`、`native_proxy/process.rs`（Win32 `GetExtendedTcpTable` + `QueryFullProcessImageNameW`）、`native_proxy/geoip.rs`（MaxMind DB v2：24/28/32 位搜索树 + 数据段 TLV / 指针）、`native_proxy/mod.rs`（`Shared::geoip` + `start_json` 按需加载） | `domain_rules_match_exact_suffix_and_keyword`、`ip_rules_only_apply_to_literal_ips`、`lan_and_loopback_always_direct`、`match_is_the_fallback`、`unknown_rules_are_skipped`、`cidr_edges`、`process_name_rules_match_the_owning_process_only_when_known`、`a_process_rule_turns_on_the_process_lookup`、`a_local_connection_resolves_to_this_test_binary`（真实端口的进程 = 测试二进制）、`an_unused_port_has_no_owner`、`geoip_rules_need_a_database_and_a_literal_ip`、**`a_v4_database_answers_the_right_country`、`v4_addresses_are_mapped_into_a_v6_database`、`record_sizes_28_and_32_read_the_same_tree`、`a_geoip_rule_routes_a_literal_ip_by_country`、`country_lookup_falls_back_through_the_documented_paths`、`a_missing_database_reports_the_expected_path`**、**`policy_field_decides_which_way_a_hit_goes`、`domain_rules_expand_blacklist_and_whitelist`、`plugin_layer_wins_over_subscription_rules`、`match_policy_is_the_second_field`** | 🟡（`RULE-SET` 自 2026-11-18 起就地展开：`http` / `file` / `inline` × `domain` / `ipcidr` / `classical`，`format: mrs` 如实跳过；`.x` / `*.x` 按后缀**近似**；GEOIP 的测试库按规范在用例里逐字节构造，缺库路径有断言；真机按进程 / 国家 / 规则集分流待跑） |
| 14 | 订阅：分享链接（`ss` / `trojan` / `vless` / `vmess` / `anytls` / `hysteria2` / `tuic`）、整段 base64、**Clash / mihomo YAML 的 `proxies:` 段**；不支持的项逐条给原因 | `native_proxy/clash.rs`（`parse_subscription` / `parse_link` / `parse_clash_yaml` / `clash_node`） | `ss_link_reads_method_and_password`、`trojan_link_reads_tls_sni_and_ws`、`vless_link_reads_flow_and_ws`、`vmess_link_reads_json_payload_and_keeps_altered_id`、`hysteria2_reads_obfs_and_insecure`、**`tuic_v5_credentials_and_relay_mode_are_read_from_links_and_yaml`**、**`subscription_accepts_base64_and_skips_bad_lines`**、**`clash_top_level_rules_are_returned_in_source_order`**、`ss_v2ray_plugin_websocket_maps_to_ws_and_tls`、`ss_simple_obfs_is_rejected_with_reason`、`clash_yaml_parses_the_common_proxy_types`、`clash_yaml_accepts_flow_style_and_legacy_ws_keys`、`clash_yaml_skips_unsupported_entries_with_a_reason`、`clash_yaml_supports_v2ray_plugin_and_base64_wrapping`、`clash_yaml_reads_http_network`、`vless_link_reads_http_transport`、`non_clash_text_still_reports_what_is_wrong`、`a_node_can_carry_a_custom_root_certificate`（`ca-str` 内联 PEM）、`a_share_link_can_carry_a_custom_root_certificate`（`ca=` 纯 PEM 与整段 base64 两种写法）、**`ipv6_servers_lose_the_url_brackets`**（2026-11-16：`[2001:db8::1]` 解析成 `2001:db8::1`）；Rust `subscription_rules_are_expanded_after_the_plugin_name_layer`（订阅 `rules:` → 引擎规则表）、**`rule_set_entries_expand_from_inline_providers`**（`rule-providers:` + `RULE-SET` 就地展开）、**`share_links_read_the_mux_flag_and_default_to_off`**（`mux=1` / vmess JSON 的 `mux`，默认关） | 🟡（解析层齐全；真机订阅待跑。**Clash `rules:` 自 2026-11-16 起生效**（排在插件名单层之后）；**`RULE-SET` 自 2026-11-18 起展开**（见行 26）；`ca` **路径**不读盘，只认 `ca-str`） |
| 15 | 生命周期 start / stop / status / select / delay（**经节点 HTTP 探针**：`HEAD http://www.baidu.com/`，量首个响应字节，`scope:"http"`）+ 端口占用回写；**模式热切 / 累计流量 / 出口地区** | `native_proxy/mod.rs`、`native_proxy/server.rs` 的 `dial_node` | `encode_target_uses_socks_address_types`、`parse_config_collects_nodes_and_skips`、`parse_config_reads_mode_and_puts_the_plugin_rule_layer_first`、`mode_global_and_direct_override_the_rule_table`、`probe_sends_a_head_request_and_accepts_any_status`、`probe_reports_a_closed_connection`、`delay_reports_the_http_scope`、`country_response_needs_200_and_a_two_letter_body` | 🟡（探针 / 拨号 / 模式 / 流量 / 出口解析有覆盖；`stop` / `select` / 端口回写仍无覆盖，真机延迟与出口地区待对照） |
| 16 | 系统代理启停与还原 | `rust/src/proxy.rs` | `status_json_has_the_contract_fields` 等（见 `proxy.rs` 单测） | 🟡 |
| 17 | 与 mihomo 的 `engine` 分派 | — | — | ❌（v1 已删除 mihomo，v2 只报告是否找到内核文件） |
| 18 | **代理面板三段式**（工具区 / 抽屉 + 168px 大字开关 / 常驻可拖拽日志）与请求日志 | `lib/search/proxy_panel.dart`、`lib/platform/proxy.dart`、Rust `lunac_proxy_requests` / `_local_mode` / `_exit_info` | `proxy entry opens the 640 panel and Esc returns`、`request log parsing keeps the panel line format`、`panel config round-trips and normalises bad values`（含 `extraRules` 往返 / 缺字段⇒空表 / 空文本⇒空表）、`engine status carries mode, traffic and the drawer columns`、Rust `request_log_keeps_only_the_latest_entries` / `requests_json_reports_recorded_entries`、`custom server reply maps the generated link and reasons`、Rust `custom_server_builds_a_link_the_parser_accepts` / `custom_server_rejects_bad_fields_with_reasons` / `custom_links_can_enable_mux_cool`、`subscription_rules_are_expanded_after_the_plugin_name_layer`；**真机** `live_panel_flow_records_requests_and_switches_mode`（`#[ignore]`：启动 → 经节点请求 → 日志可见 → 模式热切 → 停止，已通过） | 🟡（面板 / 解析 / 自定义服务器 / 自定义规则传递 / 真机链路有覆盖；**自定义服务器 2026-11-16 扩到 ss / trojan / vless / vmess / anytls / hysteria2 / tuic**；**设置页「自定义规则」多行框 2026-11-16 接到 `startLocalProxy(rules:)`**，排名单层之后、订阅规则之前；真机 1280×720 观感、拖拽手感、名单与实际分流待跑） |
| 19 | **视觉验证工具链**（§待办 3-28.5）：`Screenshot` 抓虚拟桌面 / 像素矩形 → PNG 路径 + 尺寸；`ImageDiff` 逐像素差异 + 百分比 + 包围盒 | `core-agent/src/screenshot.rs`（GDI `BitBlt`）、`core-agent/src/image_diff.rs`、`core-agent/src/tools.rs`（schema + 三张名单）、`lib/search/auto_approval.dart` | `resolve_rect` 四态 / `image_diff` 五条（identical / 单像素 / threshold 与均值 / 尺寸不同 / 缺路径）、`agent_tool_is_registered_and_filterable`（工具数 38）；**真机** `live_capture_writes_a_png`（`#[ignore]`，PASS 1707×960） | 🟡（抓帧 / diff 有覆盖；交互式框选（v1 `screenshot_overlay_*`，宿主侧）与固定 viewport/DPR 渲染脚本未做；GDI 不抓受 DRM 保护的内容、不按窗口抓帧） |
| 20 | **通用后台任务队列**（§待办 3-28.16 ①）：FIFO 只读单发 + 续轮 + 取消 pending | `app-flutter/rust/src/task_queue.rs`、`lib/platform/task_queue.dart`、`core-agent/src/tools.rs`（`TaskEnqueue` / `TaskList` / `TaskCancel` 契约） | `snapshot_normalizes_missing_bare_and_corrupt_files`、`preview_is_truncated_but_the_contract_fields_stay`、`cancel_only_touches_pending_items`、`claim_flips_pending_to_running_and_leaves_others_alone`、`stale_running_items_are_marked_failed_without_rerunning`、`rounds_are_clamped_and_continuation_needs_a_resumable_session`、`disabled_tool_list_keeps_the_autonomy_tools_out`；Flutter `task items map the queue contract and label states` / `missing status falls back to pending and unknown states stay verbatim` | 🟡（解析 / 认领 / 取消 / 陈旧恢复有覆盖；**真机**入队 → pending→running→done、关宿主重开记 failed 待跑） |
| 21 | **命令类产出识别**（§待办 3-28.13 ②）：工作区元数据指纹前后抓帧做差，识别 `Cmd` / `PowerShell` 写出的文件；命中的产出物标「命令产出」且**不给还原**（没有前像） | `app-flutter/rust/src/sandbox.rs`（`fingerprint_json` / `workspace_fingerprint_json`）、`lib/platform/sandbox.dart`（`readWorkspaceFingerprint` / `diffWorkspaceFingerprints`）、`lib/search/chat_panel.dart`（`_cmdFingerprints` / `_cmdOutputs` / `_TurnMeta.nonRevertible`） | Rust `fingerprint_captures_new_files_and_skips_heavy_dirs`（新增文件出现 / 改过指纹变化 / `node_modules` 不进 / 不存在目录 `ok=false`）；Flutter `workspace fingerprints diff into created and updated files`（改过与新增都算、没动不算、删除不算、排序稳定） | 🟡（抓帧 / 求差 / 还原按钮隐藏有覆盖；**真机**一条 `> out.txt` 命令后看产出物卡待跑） |
| 22 | **技能安装三道闸**（§待办 3-28.9）：安全扫描（danger 先确认）→ 清单 `sha256` 校验（不符拒装）→ `allowed-tools` 权限 diff（升级扩大才确认）；安装 / 保存 / 删除都进安全账本 | `app-flutter/rust/src/skills_admin.rs`（`install_with` / `frontmatter` / `sha256_hex`）、`lib/platform/skills_admin.dart`、`lib/search/settings_panel.dart`（`_installSkillFromUrl` / `_confirmPermissionExpand`）、`security_ledger` 的两个新原因码 | Rust `install_blocks_a_dangerous_skill_until_the_scan_is_accepted`、`list_and_read_expose_sha256_and_allowed_tools`、`install_rejects_a_hash_mismatch_before_writing`、`upgrading_a_skill_requires_confirmation_only_when_tools_grow`、`a_blocked_install_is_audited_as_denied_and_accepted_installs_are_marked`；Flutter `skills_admin_test.dart` 2 条 + `security_ledger_test.dart` 的 25 码枚举断言 | 🟡（三道闸与审计行有覆盖；**真机**点一遍「哈希不符 → 正确哈希 → 权限扩大确认」待跑；「精选」索引本身属外部状态未做） |
| 23 | **轨迹里的 SYSTEM / CONTEXT 两栏 + 当前会话轨迹页签 + 逐轮时间线**（§待办 3-28.12 ①②）：agent 每轮上报系统提示词 / 上下文块的字符数与 SHA-256（不发正文）；会话落盘 `promptTimeline`（每次 `system/init` 一条，≤50 条，`system` / `context` 保留为最新一次兼容字段、与最后一条同源）；抽屉「轨迹」页签可看当前（未落盘）会话；JSONL 导出按轮写 `type=system` / `type=context`（老会话保持单条） | `core-agent/src/main.rs`（`init_prompt_stats` / `emit_init`）、`app-flutter/rust/src/session_export.rs`、`lib/search/chat_model.dart`（`PromptSnapshot` / `promptTimeline`）、`lib/platform/chat_history.dart`（`promptTimeline` 解析 + 老会话合成一条）、`lib/search/chat_panel.dart`（`_buildTracePage` / `_showTrace` / `_traceColumn` / 逐轮摘要） | Rust `system_and_context_lines_precede_the_messages`、**`a_prompt_timeline_exports_one_pair_of_lines_per_turn`**（每轮一对行、兼容字段不重复导出）；core-agent `init_prompt_stats_hash_the_prompt_and_context_without_leaking_them`（含 SHA-256 标准向量与「正文不出现」断言）；Flutter `system and context stats come from init and persist into history`、**`prompt timeline keeps one snapshot per init and caps at fifty`**、**`old sessions synthesize one snapshot instead of pretending per-turn data`** | 🟡（哈希 / 逐轮落盘 / 导出 / 老会话降级有覆盖；真机 `jq` 校验待跑） |
| 24 | **TUIC v5**（§待办 3-29 G12，2026-11-16）：QUIC + TLS exporter 认证（`label`=uuid、`context`=密码）；TCP = `[5,1][地址]` 双向流；UDP = `native` datagram（`[5,2]` + 10 字节头 + 地址 + 载荷；按 `(assoc, packet)` 分片重组，10s TTL）；Dissociate = 单向流 `[5,3][assoc]`；`udp-relay-mode: quic` 如实拒绝；**QUIC 连接复用（2026-11-18）**：同一节点多条 TCP 连接共用一个 `quinn::Connection`（单连接最多 32 条双向流），认证只在建连时做一次；无 stream 且空闲 60s 的连接与引擎 `stop` 时全部关闭；UDP 仍每目标一条连接 | `app-flutter/rust/src/native_proxy/tuic.rs`（`connect` / `connect_async` / `PoolEntry` / `connect_udp` / `encode_address` / `packet_frame` / `Defragger`）、`quic.rs`（共享 runtime / TLS / 错误翻译）、`clash.rs`（`parse_tuic` + YAML `tuic`）、`server.rs::dial_node`、`udp.rs::Upstream::Tuic` | `authenticate_frame_is_version_command_uuid_token`、`address_encoding_matches_the_tuic_families`、`address_round_trips_and_none_is_a_single_byte`、`connect_frame_is_version_and_command_then_address`、`packet_round_trips_and_fragments_reassemble_out_of_order`、`a_single_fragment_packet_is_passed_through`、`packets_with_a_bad_version_or_truncated_body_are_rejected`、`config_defaults_alpn_to_h3_and_native_udp`、**`a_mock_server_accepts_the_authenticated_tcp_stream`**（quinn 假服务端复算 exporter 令牌 + TCP 回显）、**`a_mock_server_relays_udp_packets_with_the_original_assoc_id`**、**`a_second_tcp_stream_reuses_the_same_quic_connection`**（只接受一条连接、两条流，300ms 内无第二条连接）、`tuic_v5_credentials_and_relay_mode_are_read_from_links_and_yaml`；`quic.rs` 的 `application_close_keeps_the_server_reason` / `tls_config_uses_tls13_and_the_requested_alpn` | 🟡（帧级 + 假服务端端到端全部通过；**真机 204 待补** —— 手上无 TUIC 节点） |
| 25 | **AnyTLS 多流复用**（§待办 3-29 G11，2026-11-16）：一条 TLS 会话并发承载最多 8 条 stream（`cmdSYN` / `cmdPSH` / `cmdFIN` 按 sid 分发，`cmdAlert` 会话级）；会话池按「服务器 + 凭据 sha256 + TLS 参数」复用、空闲 60s 自关（引擎 `stop_json` 时 `close_all_sessions()` 直接关闭）；TCP 与 UoT 的 UDP 共用同一个池；单条 stream 的读超时用条件变量（超时回 `WouldBlock`） | `app-flutter/rust/src/native_proxy/anytls.rs`（`Session` / `StreamState` / `connect_pooled` / `session_key`）、`server.rs::dial_node`、`udp.rs`（UoT 分支） | `two_streams_reuse_one_session_and_stay_isolated`（只认证一次 / settings 只发一次 / sid 各自独立）、`a_fin_closes_only_its_own_stream`、`an_alert_closes_every_stream`、`default_padding_scheme_matches_upstream`、`auth_packet_carries_sha256_and_padding`、`frame_layout_matches_spec`、`padding_scheme_from_server_is_adopted_for_later_sessions` | 🟡（假服务端多流 / 会话复用 / FIN / ALERT 全部通过；**真机多连接复用待补** —— 手上无 AnyTLS 节点） |
| 26 | **`RULE-SET` 规则集展开**（§待办 3-29 G13，2026-11-18）：订阅 `rule-providers:`（`http` / `file` / `inline`）× `behavior`（`domain` / `ipcidr` / `classical`）就地展开成普通规则行，位置不变；`+.x` ⇒ `DOMAIN-SUFFIX`，`.x` / `*.x` ⇒ `DOMAIN-SUFFIX`（**近似**，引擎只有后缀匹配），其余 ⇒ `DOMAIN`；`ipcidr` 按 `:` 选 `IP-CIDR` / `IP-CIDR6`；`classical` 取前两段并补策略位；`format: mrs` / 未知 behavior / 缺失 provider 一律如实跳过并记原因；`http` 规则集缓存到 `<exe>/ModuleData/proxy/rulesets/<md5(url)>.ruleset`（有缓存直接用、过期只后台刷新，无缓存才同步拉一次，5s 超时） | `app-flutter/rust/src/native_proxy/ruleset.rs`（`expand` / `expand_entry` / `load_entries` / `refresh_in_background`）、`clash.rs`（`RuleProvider` + `parse_rule_providers` + `yaml_payload`）、`mod.rs::parse_config` | `domain_entries_follow_the_documented_wildcards`、`ipcidr_and_classical_entries_keep_their_shape`、`inline_provider_expands_in_place_and_unknown_names_are_reported`、`an_http_provider_is_fetched_once_then_served_from_cache`（本地 HTTP 服务端计数：第二次不发请求）、`mrs_and_unknown_behaviors_are_reported_not_guessed`、`rule_set_entries_expand_from_inline_providers`（`parse_config` 端到端） | 🟡（展开 / 缓存 / 跳过原因有覆盖；**真机订阅的 `RULE-SET` 分流待跑**；`.x` / `*.x` 的近似已在文档写明） |
| 27 | **历史会话的「更改文件」列表**（§重建补记 M5z-补56 的 2026-11-18 补齐）：从落盘过程快照里的写类工具重建列表（相对路径按当时工作区解析、小写去重）；**只还原列表不假装可回退** —— 打开该页时逐条 `readChangeDiff`，查不到快照的行标「已无快照」 | `lib/search/chat_model.dart`（`stepEntryOf` / `changedFilesFromSteps` / `changedFilesOf(resolve:)`）、`lib/search/chat_panel.dart`（`_seedChangedFilesFromSteps` / `_computeChangeStats` / `_noSnapshot` / `_buildChangeRow`） | Flutter `restored sessions rebuild the changed-file list from their steps`（解析相对路径 / 不猜工作区 / 坏步骤跳过）、`changedFilesOf trusts only the write tools and de-duplicates`；**真机**切回历史会话看待办抽屉的「更改文件」页待跑 | 🟡（重建与「已无快照」标注有单测；真机切会话对照待跑） |
| 28 | **暂停的回执**（§待办 3-28.1 的 2026-11-18 补齐）：agent 在轮首安全点停下并发 `result.subtype="paused"`，界面落一条「已暂停本回合（轮首安全点）。已完成的工具不会重跑，直接发下一条消息即可继续。」—— 不再与正常成功同形 | `lib/search/chat_model.dart`（`_applyResult`） | Flutter `a paused round ends with a notice instead of looking like success`（busy 收口 / 不是错误卡 / 旁注含「已暂停」「继续」）；**真机**点一遍暂停 → 继续待跑 | 🟡（收尾语义有单测；真机三态对照待跑） |
| 29 | **Xray `mux.cool` 多路复用**（§待办 3-29 G15，2026-11-18）：同一节点的多条 TCP 连接共用一条到 `v1.mux.cool:9527` 的底层连接；VLESS 用 `cmd=Mux`（不写地址），VMess / Trojan 用普通命令把目的地写成魔法域名；帧 = `metaLen｜sid｜status｜option｜[network｜port｜addr]｜[dataLen｜data]`，`New` 带目标、`Keep` / `End` 不带；单连接最多 8 条流、空闲 60s 自关、引擎 `stop` 时全部关闭；**只从分享链接 `mux=1` / vmess JSON 的 `mux` 字段启用**（Clash 的 `smux` 是另一套不兼容协议，不认） | `app-flutter/rust/src/native_proxy/mux.rs`（`Session` / `connect_pooled` / `session_key` / `new_frame` / `keep_frame` / `end_frame`）、`server.rs::dial_node`、`clash.rs`（`parse_trojan` / `parse_vless` / `parse_vmess`）、`mod.rs`（`build_custom_link_json` 的 `mux` + `stop_json` 的 `close_all_sessions`） | `frames_match_the_upstream_layout`、`two_streams_share_one_connection_and_end_only_closes_its_own`（假服务端只接受一条连接、两条流各回各的）、`session_key_is_stable_and_hides_credentials`、`share_links_read_the_mux_flag_and_default_to_off`、`custom_links_can_enable_mux_cool`；**真机 mux 节点待补**（手上无开启 mux 的节点） | 🟡（帧布局 / 复用 / 隔离 / 关闭 / 配置读取有覆盖；真机待补） |

## 三、能力账本（`app/src-tauri/src/native_proxy/`，21 个文件）

> 「真机条目」= [`testing.md`](./testing.md) §测试 27 的编号（A1–J16）。

| # | 能力 | 实现 | 自动化用例（测试函数名） | 真机条目 | 状态 |
|---|---|---|---|---|---|
| 1 | 入站 SOCKS5 CONNECT + HTTP CONNECT / 绝对 URI（同一端口） | [`server.rs`](../app/src-tauri/src/native_proxy/server.rs) `handle_client` | `tests::socks5_end_to_end_in_direct_mode`、`tests::http_connect_end_to_end_in_direct_mode`、`server::tests::absolute_uri_is_split_into_authority_and_path` | 4、5、16（TCP 半段） | 🟡 |
| 2 | 入站 SOCKS5 UDP ASSOCIATE（控制连接生命周期绑定） | [`udp.rs`](../app/src-tauri/src/native_proxy/udp.rs) `serve` | `udp::tests::datagram_round_trip_for_domain_and_ip`、`udp::tests::fragmented_or_truncated_datagrams_are_rejected` | 16 | 🟡 |
| 2b | UDP 上游：SS（SIP004）+ VLESS（Vision → Mux + XUDP）+ Trojan + VMess + AnyTLS（UoT v2）+ Hysteria2（QUIC datagram） | [`udp.rs`](../app/src-tauri/src/native_proxy/udp.rs) `forward`、[`ss.rs`](../app/src-tauri/src/native_proxy/ss.rs) `SsUdp`、[`vless_udp.rs`](../app/src-tauri/src/native_proxy/vless_udp.rs)、[`trojan_udp.rs`](../app/src-tauri/src/native_proxy/trojan_udp.rs)、[`vmess_udp.rs`](../app/src-tauri/src/native_proxy/vmess_udp.rs)、[`anytls_udp.rs`](../app/src-tauri/src/native_proxy/anytls_udp.rs)、[`hysteria2_udp.rs`](../app/src-tauri/src/native_proxy/hysteria2_udp.rs) | `ss::tests::sip004_udp_round_trip`、`vless_udp::tests::xudp_*`、`trojan_udp::tests::*`、`vmess_udp::tests::*`、`anytls_udp::tests::*`、`hysteria2_udp::tests::*`；真机 `vless_udp::tests::live_node_answers_dns_over_udp` | 16 | 🟡（VLESS 真机 DNS 已验；其余待对应节点） |
| 3 | SS AEAD（SIP004：aes-128-gcm / aes-256-gcm / chacha20-ietf-poly1305） | [`ss.rs`](../app/src-tauri/src/native_proxy/ss.rs) `Cipher`、`SsTunnel` | `ss::tests::cipher_names_are_stable`、`evp_bytes_to_key_matches_openssl_vector`、`round_trip_for_each_legacy_cipher` | 4、5 | ❌（2026-11-03 真机：对 Xray 与 shadowsocks-libev 均「SS 解密失败」，见 `temp\live-links.txt`） |
| 4 | SS UDP（SIP004：每包独立 salt + AEAD，nonce 恒 0） | [`ss.rs`](../app/src-tauri/src/native_proxy/ss.rs) `SsUdp::connect/send/recv` | `ss::tests::sip004_udp_round_trip` | 16 | 🟡 |
| 5 | SS-2022（SIP022：三种 `2022-blake3-*`，`blake3::derive_key` 子密钥）+ 多用户 `iPSK:uPSK`（SIP023 identity header） | [`ss.rs`](../app/src-tauri/src/native_proxy/ss.rs) `Cipher::is_2022` 分支、`decode_psks`、`identity_header` | `ss::tests::round_trip_for_each_2022_cipher`、`psk_decoding_handles_single_and_multi_user`、`multi_user_psk_sends_identity_header_and_uses_last_key` | 12 | ❌（2026-11-03 真机：多用户节点被服务端 RST） |
| 5b | SS 的 SIP003 插件：simple-obfs `http` / `tls`、v2ray-plugin `websocket` | [`ss_plugin.rs`](../app/src-tauri/src/native_proxy/ss_plugin.rs) `SsPlugin`、`ObfsHttpTunnel`、`ObfsTlsTunnel`；[`ss.rs`](../app/src-tauri/src/native_proxy/ss.rs) `SsTunnel::connect` | `ss_plugin::tests::parse_obfs_and_v2ray_plugin_specs`、`obfs_http_tunnel_prepends_upgrade_request`、`obfs_tls_client_hello_matches_simple_obfs_layout`、`obfs_tls_tunnel_round_trips_through_mock_server`、`tls_app_frame_carries_five_byte_header`；解析见 `clash::tests::ss_link_and_yaml_parse_plugins` | 4、5、12 | ❌（2026-11-03 真机：obfs 握手通过但 SS 解密失败，随 row 3 一起红） |
| 6 | Trojan（`hex(sha224(pwd)) CRLF 0x01 <addr> CRLF` + TLS） | [`trojan.rs`](../app/src-tauri/src/native_proxy/trojan.rs) `connect` | `trojan::tests::handshake_head_matches_trojan_spec` | 11（Trojan+WS） | ✅（2026-11-03 真机 8443 自签 + `allowInsecure=1`，1465ms） |
| 7 | VLESS（响应头延迟消费；地址类型用 Xray 的 1/2/3） | [`vless.rs`](../app/src-tauri/src/native_proxy/vless.rs) `connect`、`VlessStream` | `vless::tests::request_header_matches_vless_layout`、`request_header_places_port_before_address`、`request_header_uses_xray_address_types`、`uuid_is_parsed_from_hex_with_dashes` | 11 | ✅ |
| 7c | VLESS Vision（flow addons + 上行首帧 padding + 下行解填充） | [`vless.rs`](../app/src-tauri/src/native_proxy/vless.rs) `header`、`VlessStream` | `vless::tests::vision_header_carries_flow_addons`、`vision_reader_unpads_frames_then_passes_raw`、`vision_reader_passes_stream_without_uuid_prefix`；真机 `live_node_fetches_generate_204` | 11（真机已验证 204） | ✅ |
| 7b | VLESS + REALITY（自写 TLS 1.3、SessionId 认证、临时证书绑定） | [`reality.rs`](../app/src-tauri/src/native_proxy/reality.rs) `connect` | `reality::tests::x25519_matches_rfc7748_vector`、`x25519_base_point_matches_rfc7748_public_key`、`reality_seal_uses_zeroed_session_id_as_aad`、`client_hello_has_session_id_at_fixed_offset_and_tls13`、`hkdf_expand_label_matches_rfc8448_style_lengths`、`rfc8448_handshake_secrets_match`、`nonce_xor_is_big_endian_counter`、`der_tlv_reads_long_form_length` | 11（真机已验证 204） | ❌（2026-11-03 真机复测：「证书签名不是 64 字节（非 REALITY 临时证书）」⇒ 我们的 ClientHello 未被认可） |
| 8 | VMess AEAD（alterId=0，AES-128-GCM + ChunkStream，响应头回显校验） | [`vmess.rs`](../app/src-tauri/src/native_proxy/vmess.rs) `VmessTunnel::connect` | `vmess::tests::kdf_matches_upstream_vector`、`auth_id_carries_timestamp_and_crc`、`chunk_nonce_is_little_endian_counter`、`request_header_and_chunks_round_trip` | 13 | ❌（2026-11-03 真机：ws+TLS 节点「对端直接关闭」；**2026-11-15 定位根因 = `CmdKey` 少拼 `"c48619fe-…"`，已修**；**待重跑真机**） |
| 9 | AnyTLS（单 Stream、无填充策略；`cmdSettings` 必发） | [`anytls.rs`](../app/src-tauri/src/native_proxy/anytls.rs) `AnytlsTunnel::connect` | `anytls::tests::auth_settings_and_stream_round_trip`、`empty_padding_md5_is_standard_md5_of_empty_string` | 14 | ✅（2026-11-03 真机 8451 自签，319ms） |
| 10 | Hysteria2（QUIC + HTTP/3，`:status 233` 认证，TCP 方向） | [`hysteria2.rs`](../app/src-tauri/src/native_proxy/hysteria2.rs) `Hysteria2Tunnel::connect` | `hysteria2::tests::varint_matches_rfc9000_examples`、`authority_brackets_ipv6`、`qp_style_headers_are_accepted_by_http` | 15 | ✅（2026-11-03 真机 UDP 8443，587ms） |
| 11 | Hysteria2 `salamander` 混淆（`AsyncUdpSocket` 包装） | [`hysteria2.rs`](../app/src-tauri/src/native_proxy/hysteria2.rs) `SalamanderSocket` | `hysteria2::tests::salamander_round_trip_and_short_datagram`、`blake2b256_matches_known_vector_and_key_precedes_salt` | 15 | ✅（2026-11-03 真机 UDP 8443 salamander，587ms） |
| 11b | Hysteria2 `gecko` 混淆（长头包切 2–8 片 + 随机填充 + 接收重组） | [`hysteria2.rs`](../app/src-tauri/src/native_proxy/hysteria2.rs) `GeckoSocket`、`encode_gecko_frame`、`gecko_pad_len` | `hysteria2::tests::gecko_frame_round_trips_and_pads_to_packet_bounds`、`gecko_rejects_malformed_frames`、`obfs_spec_maps_defaults_and_keys`；解析见 `clash::tests::hysteria2_link_reads_insecure_and_obfs`、`clash_yaml_accepts_hysteria2_with_salamander_and_gecko` | 15 | ✅（2026-11-03 真机 UDP 8444 gecko，928ms；sing-box 1.14.4 接受 gecko 配置） |
| 11c | Hysteria2 UDP 转发（QUIC 不可靠数据报 + `UDPMessage` 分片 / 重组） | [`hysteria2_udp.rs`](../app/src-tauri/src/native_proxy/hysteria2_udp.rs) `Hysteria2Udp::connect/send/try_recv`、[`udp.rs`](../app/src-tauri/src/native_proxy/udp.rs) `forward` | `hysteria2_udp::tests::message_layout_matches_hysteria_spec`、`large_payload_is_fragmented_and_reassembled`、`duplicate_fragment_is_dropped`、`authority_parses_ipv4_ipv6_and_domain` | 15、16 | 🟡（`Hysteria-UDP: false` 时如实报错） |
| 12 | 传输 TCP / TLS / WebSocket / REALITY（握手 + 分帧） | [`transport.rs`](../app/src-tauri/src/native_proxy/transport.rs) `connect`、`WsTunnel`；REALITY 转 [`reality.rs`](../app/src-tauri/src/native_proxy/reality.rs) | `transport::tests::websocket_handshake_and_echo`、`websocket_len_encoding_covers_three_widths`；REALITY 见 7b | 11、13 | 🟡（TLS/WS 真机 ✅：8443 trojan、8445 ws；REALITY 见 7b ❌） |
| 12b | 传输 HTTP 混淆（`network: http`） | [`http_obfs.rs`](../app/src-tauri/src/native_proxy/http_obfs.rs) `HttpTunnel::handshake` | `http_obfs::tests::http_obfs_handshake_and_echo`、`http_obfs_rejects_non_200` | 11 | 🟡（本地回环通过；**Xray 26 已移除 `network: http`，真机无法验证**） |
| 12c | 传输 HTTP/2（`network: h2`）与 gRPC（`network: grpc`，`Hunk` protobuf） | [`h2tunnel.rs`](../app/src-tauri/src/native_proxy/h2tunnel.rs) `connect_h2` / `connect_grpc` | `h2tunnel::tests::h2_transport_echoes_through_local_server`、`grpc_transport_wraps_hunk_and_echoes`、`grpc_path_follows_xray_service_name_rules`、`grpc_frame_matches_hunk_layout`、`grpc_decoder_waits_for_partial_frames`、`tls_config_honors_insecure_switch`（`skip-cert-verify`） | 11、18 | ✅（2026-11-03 真机 gRPC 8447，327ms；`skip-cert-verify` 生效；`h2` 传输已被 Xray 26 移除，只能保留本地回环） |
| 12d | 传输 XHTTP / SplitHTTP（`network: xhttp`，`stream-one` + `stream-up`，含 padding / session） | [`h2tunnel.rs`](../app/src-tauri/src/native_proxy/h2tunnel.rs) `connect_xhttp`、`run_xhttp_stream_up` | `h2tunnel::tests::xhttp_stream_one_echoes_with_padding`、`xhttp_stream_up_splits_upload_and_download`、`xhttp_parts_normalizes_trailing_slash_and_query`、`xhttp_padding_referer_carries_100_to_1000_marks`、`tls_config_honors_insecure_switch` | 11、18 | ✅（2026-11-03 真机 8448，914ms；`packet-up` 未做） |
| 13 | 规则 `DOMAIN*` / `IP-CIDR*` / `MATCH` + 本机 / 私网直连 | [`rules.rs`](../app/src-tauri/src/native_proxy/rules.rs) `decide`、`lan_rules` | `rules::tests` 6 条（域名三态 / IP 仅字面量 / IPv6 家族隔离 / 私网覆盖 / MATCH 兜底 / 未知规则跳过） | 5 | 🟡 |
| 14 | 订阅：Clash YAML / base64 / 分享链接（含 `vmess://JSON`、REALITY `pbk`/`sid`、`http/h2/grpc/xhttp` 传输、Hysteria2 `obfs` 的 salamander / gecko、SS `plugin`）+ 跳过不支持项 | [`clash.rs`](../app/src-tauri/src/native_proxy/clash.rs) `build`、`parse_link`、`hysteria_obfs`、`yaml_plugin_args` | `clash::tests` 24 条（各协议链接解析、YAML 取舍、REALITY 公钥 / shortId、http/h2/grpc/xhttp 选项、Hysteria2 obfs / 尺寸与非法值跳过、SS 插件与非法插件跳过、重名消歧、不支持项拒绝） | 3、7、11–15 | 🟡 |
| 15 | 生命周期 start / stop / status / select / delay（**经节点 HTTP 探针**）+ 端口占用回写 | [`mod.rs`](../app/src-tauri/src/native_proxy/mod.rs) `start/stop/status/select/delay/proxies_json`、[`server.rs`](../app/src-tauri/src/native_proxy/server.rs) `dial_node` | —（仅 `start` / `status` 由两条端到端覆盖；`stop` / `select` / `delay` / 端口回写无覆盖） | 1、2、6、9、10 | ⬜ |
| 16 | 系统代理启停与还原（与 mihomo 共用 `system_proxy.rs`） | [`system_proxy.rs`](../app/src-tauri/src/system_proxy.rs) | — | 4 | ⬜ |
| 17 | 与 mihomo 的 `engine` 分派 | [`mihomo.rs`](../app/src-tauri/src/mihomo.rs) | — | 10（默认值改造见 [`todo.md`](./todo.md) §待办 3-26） | ⬜ |

## 四、真机 16 条（步骤 + 成功体现，全部未跑）

### §四.0 一次性前置

1. `cd app ; npm run tauri:dev`，代理插件已启用；准备一份**同时含**这些节点的订阅：
   `ss://`（SIP004）、`ss://`（2022-blake3-*）、`ss://`（simple-obfs http / tls 与 v2ray-plugin ws 各一）、
   `trojan://`、`vless://`（TCP 与 WS 各一）、
   `vmess://`（TCP 与 WS 各一，`alterId: 0`）、`anytls://`、`hysteria2://`（含 `insecure=1` 与
   `obfs=salamander` 与 `obfs=gecko` 各一）、`tuic://`（v5，`uuid:password`）、`ssr://…`（未知协议）、`ss://…?plugin=kcptun`（未知插件）、
   `hysteria2://…?obfs=nope`（非法混淆）各至少一条 —— 后三类用于验证解析阶段跳过。
2. 证据目录：`<仓库根>\temp\evidence\<YYYY-MM-DD>\`（已在 `.gitignore` 的 `temp\` 下，不进仓）。
   每条把「看得到的文案」原样存成 `A1.txt` 这类小文件，或从 `temp\logs\lunac-*.log` 截对应几行；
   `curl` 一律加 `-sS -o <文件> -w '%{http_code}\n'`，**状态码也要落盘**。
3. 下文 `<port>` = 状态行显示的混合端口；`curl` 一律走 `-x http://127.0.0.1:<port>`（UDP 条目除外）。
4. 判据里的「⇒ 证据」就是状态格要写的那份文件名；**没有落盘证据不许改 ✅**。

### §四.1 16 条步骤与判据

| 段 | 条目 | 具体步骤（照做） | 真机成功的体现（判据 ⇒ 留什么证据） | 状态 |
|---|---|---|---|---|
| A | 1 | 删掉 `<exe 根>\config\proxy.json`（或只删 `engine` 字段）→ 重启 → 代理插件点「启动」 | 状态行 `内核运行中 · native · 混合端口 <port>`；回写后的 `proxy.json` **没有** `"engine": "mihomo"` ⇒ `A1-status.txt` + `proxy.json` 副本 | ⬜ |
| A | 2 | 同上启动后看按钮区；再把 `proxy.json` 改成 `"engine": "mihomo"` 重启一次 | 按钮区**没有**「下载内核」；旧配置时状态行给「已不再内置 mihomo …」且**整排按钮不出现**（不给点了必失败的入口） ⇒ `A2-buttons.txt` + `A2-legacy.txt` | ⬜ |
| A | 3 | 订阅**只留** `ssr://` 与 `ss://…?plugin=` 两条 → 点「启动」 | 直接报「自研引擎没有可用节点：只支持 ss / trojan / vless / vmess / anytls / hysteria2 / tuic …」；日志每条一行「跳过」；**没有**「已启动」 ⇒ `A3-error.txt` + 日志片段 | ⬜ |
| B | 4 | 先记下 `设置 → 网络 → 代理` 的原始值 → 启用插件 → 关闭插件 | 启用后系统代理指向 `127.0.0.1:<port>`；关闭后**逐字还原**原值；`curl -x http://127.0.0.1:<port> https://www.gstatic.com/generate_204 -I` 得 `HTTP/1.1 204` ⇒ 前后两次代理设置截图 + `B4-curl.txt` | ⬜ |
| B | 5 | 停掉当前节点（或把节点地址改成黑洞）→ `curl -x http://127.0.0.1:<port> http://127.0.0.1:8788/health` | 仍返回 200 + 健康内容（本机网段命中 `lan_rules()` 直连，不经过节点）；日志里没有该请求的上游连接 ⇒ `B5-curl.txt` + 日志片段 | ⬜ |
| B | 6 | 节点 A 时 `curl -x … https://api.ipify.org`；切到节点 B 再跑一次 | 两次出口 IP 不同，且分别落在 A / B 节点的归属地区 ⇒ `B6-a.txt` + `B6-b.txt` | ⬜ |
| C | 7 | 订阅加入 `hysteria2://…?obfs=nope`、`ss://…?plugin=kcptun`、`ssr://…` 各一条 → 启动 | 三条都**不在**节点列表；日志**逐条**有「跳过」+ 原因（未知 obfs / 未知插件 / 不支持的协议）；若仍有可用节点，启动照常成功 ⇒ 列表截图 + 日志片段 | ⬜ |
| C | 8 | ①用脚本对混合端口发 SOCKS5 `CMD=0x02`（BIND）；②直接发 `GET https://example.com/ HTTP/1.1`（绝对 URI、无 CONNECT） | ①回 `0x07`（命令不支持）；②回 `400` ⇒ `C8-bind.bin` + `C8-absuri.txt` | ⬜ |
| C | 9 | 先手动监听默认端口占住它 → 点「启动」 | 日志「端口 N 被占用，改用一个空闲端口」，启动仍成功；状态行端口是**实际**端口，`curl -x` 实际端口得 204 ⇒ `C9-log.txt` + `C9-curl.txt` | ⬜ |
| D | 10 | 手写 `"engine": "mihomo"` → 启动；改回 `native` → 启动；运行中调 `mihomo_set_engine("native")` 与 `("mihomo")` | 第一次报「当前版本已不再内置 mihomo …」，**不**静默跑 native；改回后正常接管；运行中切换报「先停用当前内核，再切换」；`mihomo` 参数报同一句「已不再内置」且**配置未被改写** ⇒ 三组文案 + 前后 `proxy.json` diff | ⬜ |
| D | 10b | **先不开启服务**，只打开面板（引擎应被静默拉起），对每个节点点一次「测速」；再开启服务测一次；再断网后点一次 | 未开服务时也能出数（延迟 = 经节点取直连可达的 `www.baidu.com/` 的耗时）；开启服务后数字同量级；断网 / 节点不可用时**如实报错**（不显示乐观的 TCP 建连数字） ⇒ 三次结果 + 一条错误文案 + `mihomo_status` 里 `running=true / enabled=false` | ⬜ |
| E | 11 | 订阅含 `VLESS + TLS`、`VLESS + WS + TLS`、`Trojan + WS`、`VLESS + REALITY`（带 `pbk`/`sid`）、`VLESS + REALITY + Vision` 各一条，逐个切换并 `curl -x … generate_204`；再放 `security=reality` 但缺 `pbk`、`network=kcp` 各一条 | 前五条都 **204**（REALITY 失败时日志要能区分「认证失败 / 被中转」与「网络错误」）；缺 `pbk` 与 kcp **不在列表**且日志各一行「跳过」 ⇒ 五次 curl + 日志片段 | ⬜（`VLESS + REALITY + Vision` 已在开发机验到 204，其余待跑） |
| F | 12 | 加 `2022-blake3-aes-128-gcm` / `-aes-256-gcm` / `-chacha20-poly1305` 三条，逐个切换并 curl；再加多用户 PSK（`iPSK:uPSK`）与长度错误的 PSK 各一条 | 三条都 **204**；多用户节点也 **204**（SIP023 identity header）；PSK 长度不对报「base64 解出不是 16 / 32 字节」——**不是**静默连不上 ⇒ 四次 curl + 一段错误文案 | ⬜ |
| F | 12b | 分别加 `ss://…?plugin=obfs-local;obfs=http;obfs-host=…`、`…;obfs=tls`、以及 `plugin: v2ray-plugin` + `plugin-opts.mode: websocket`（可选 `tls: true`）各一条，逐个 curl；再加一条 `mode: quic` | 三条插件节点都 **204**；`mode: quic` **不在列表**且日志一行「跳过」；UDP（DNS）经插件节点应无回包且日志一行「SS 插件只承载 TCP」 ⇒ 三次 curl + 两段日志 | ⬜ |
| G | 13 | `vmess://` 的 TCP 一条、WS（+TLS）一条，各切一次并 curl；加一条 `alterId: 2`；把可用节点的 `uuid` 改错一位再连 | 两条都 **204**；`alterId≠0` 不在列表 + 一行「跳过」；错 `uuid` 在**第一次读响应头**就报「VMess 响应头的回显字节不匹配」或「VMess 解密失败」，**不**挂到 12s 超时（日志时间戳相差 < 2s） ⇒ 两次 curl + 错误文案 + 时间戳 | ⬜ |
| H | 14 | `anytls://password@host:port?sni=…` 节点 curl；密码改错再连；同一节点连续访问两个不同站点 | 正常 **204**；错密码**快速**失败（日志 TLS / EOF，明显短于 12s 拨号超时），`cmdAlert` 有文本时进日志；两个站点都成功 ⇒ 两次 curl + 日志耗时 | ⬜ |
| I | 15 | 依次：①`insecure=1` 节点 curl；②自签但**不带** `insecure`；③密码改错；④目标改成不可达；⑤`salamander` 的 `obfs-password` 正确 / 错误各一次；⑥`gecko` 节点（含自定义 `obfs-min-packet-size` / `obfs-max-packet-size`）curl；⑦经该节点解析一个域名（UDP） | ①204；②12s 内 TLS 错误（不是静默超时）；③「Hysteria2 认证失败：HTTP <code>」；④「Hysteria2 出站被拒绝：<服务端消息>」；⑤正确 204 / 错误握手失败；⑥204（UDP / DNS 走 ⑦）；⑦有 DNS 应答；UDPMessage 分片重组有单测 ⇒ 七段日志 + 三次 curl | ⬜ |
| J | 16 | 先 `curl --socks5-hostname 127.0.0.1:<port> https://www.gstatic.com/generate_204` 拿 204；再用 `scripts` 里的小 UDP 客户端：①直连 `127.0.0.1` 上的 DNS / QUIC；②`8.8.8.8:53` 走 SS 节点（再用错密码跑一次）；③分别换 VLESS / Trojan / VMess / AnyTLS / Hysteria2 节点重跑 ②；④构造 `FRAG != 0` 的数据报 | TCP 204；①有回包，**关掉控制连接后不再有回包**；②有 DNS 应答，错密码时**无应答**且日志**仅一行**「UDP 上游不可用」；③五类节点都能拿到 DNS 应答；④带 FRAG 的被丢弃 ⇒ `J16-tcp.txt` + 七段 UDP 输出 + 日志 | ⬜ |

> 跑完一段就把上表与第三节对应行的 🟡/⬜ 改成 ✅，并在**状态格**写
> `✅（YYYY-MM-DD，temp\evidence\<日期>\<文件名>）`；证据优先用日志片段 / `curl` 输出，不用截图猜。
> 任何一条只做到「代码看着对」⇒ 保持 🟡；只做到「弹出了正确文案但没落盘」⇒ 也保持 🟡（§六 规则 2）。

## 五、已登记的缺口（❌ 不做 / 未做，别误以为支持）

| 类别 | 项 | 理由 |
|---|---|---|
| 协议 | SSR（已淘汰） | 机场占比极低；**TUIC v5 已于 2026-11-16 支持**（见第二节行 24，UDP 只做 `native`，TCP 连接复用 2026-11-18） |
| 协议特性 | REALITY 的 uTLS 指纹模仿（JA3/JA4 ≠ Chrome）、Vision 的 TLS-in-TLS long padding / 直通、VMess `alterId≠0` / chunk masking / global padding | REALITY 主体与 Vision 功能链路（含上行首帧 padding）已支持；指纹模仿需要自研 uTLS；其余是老式认证 |
| Hysteria2 | Brutal 拥塞控制 | quinn 不开放可插拔 CC；`Hysteria-CC-RX` 恒发 0，让对端按自己的 CC 自适应（gecko / UDP 已支持） |
| 传输 | `kcp`、XHTTP `packet-up`、v2ray-plugin 的 `quic` 模式、simple-obfs 之外的 SS 插件（kcptun / cloak / restls） | `http` / `h2` / `grpc` / `xhttp`（stream-one / stream-up）与 SS 的 simple-obfs（http / tls）/ v2ray-plugin（websocket）已支持（2026-11-03）；`kcp` 需完整 ARQ + FEC + 老式混淆/加密且无真机可验收（见 todo G3b） |
| 多路复用 | Clash 的 `smux`（smux / yamux / h2mux） | `mux.cool` 已于 2026-11-18 支持（见第二节行 29）；`smux` 是**另一套不兼容的协议**，不从 `smux:` 读取，也不冒充 |
| 其它 | TUN、Clash 控制口 / 策略组、规则热切换、`client-fingerprint` | TUN 需管理员 + wintun；策略组随 mihomo 退场一并消失；测速已改为经节点 HTTP 探针（目标 = 直连可达的 `www.baidu.com/`，且未开启服务也能测） |

## 六、维护规则

1. **新增一条能力** ⇒ 在第三节加一行，必须同时填「测试函数名」；写不出测试名就填 `—` 并在状态里用 ⬜，不要填 🟡。
2. **真机通过** ⇒ 必须先在 §四 按「具体步骤」照做一遍、把「成功体现」里的证据文件落盘，再改状态与日期 / 证据路径，不删行；
   失败也记（状态保持 🟡 并在备注里写现象）。只有文案没有留档 ⇒ 仍是 🟡。
3. **删代码** ⇒ 删对应行；不要留「已废弃」行。
4. 本节表格的基数必须与 `cargo test --bin lunac native_proxy -- --list` 的条数一致；不一致说明表过期。
