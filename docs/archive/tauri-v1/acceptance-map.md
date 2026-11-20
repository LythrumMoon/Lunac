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
| 8 | VMess AEAD（alterId=0，AES-128-GCM + ChunkStream，响应头回显校验） | [`vmess.rs`](../app/src-tauri/src/native_proxy/vmess.rs) `VmessTunnel::connect` | `vmess::tests::kdf_matches_upstream_vector`、`auth_id_carries_timestamp_and_crc`、`chunk_nonce_is_little_endian_counter`、`request_header_and_chunks_round_trip` | 13 | ❌（2026-11-03 真机：ws+TLS 节点「对端直接关闭」） |
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
   `obfs=salamander` 与 `obfs=gecko` 各一）、`tuic://`、`ss://…?plugin=kcptun`（未知插件）、
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
| A | 3 | 订阅**只留** `tuic://` 与 `ss://…?plugin=` 两条 → 点「启动」 | 直接报「自研引擎没有可用节点：只支持 ss / trojan / vless / vmess / anytls / hysteria2 …」；日志每条一行「跳过」；**没有**「已启动」 ⇒ `A3-error.txt` + 日志片段 | ⬜ |
| B | 4 | 先记下 `设置 → 网络 → 代理` 的原始值 → 启用插件 → 关闭插件 | 启用后系统代理指向 `127.0.0.1:<port>`；关闭后**逐字还原**原值；`curl -x http://127.0.0.1:<port> https://www.gstatic.com/generate_204 -I` 得 `HTTP/1.1 204` ⇒ 前后两次代理设置截图 + `B4-curl.txt` | ⬜ |
| B | 5 | 停掉当前节点（或把节点地址改成黑洞）→ `curl -x http://127.0.0.1:<port> http://127.0.0.1:8788/health` | 仍返回 200 + 健康内容（本机网段命中 `lan_rules()` 直连，不经过节点）；日志里没有该请求的上游连接 ⇒ `B5-curl.txt` + 日志片段 | ⬜ |
| B | 6 | 节点 A 时 `curl -x … https://api.ipify.org`；切到节点 B 再跑一次 | 两次出口 IP 不同，且分别落在 A / B 节点的归属地区 ⇒ `B6-a.txt` + `B6-b.txt` | ⬜ |
| C | 7 | 订阅加入 `hysteria2://…?obfs=nope`、`ss://…?plugin=kcptun`、`tuic://…` 各一条 → 启动 | 三条都**不在**节点列表；日志**逐条**有「跳过」+ 原因（未知 obfs / 未知插件 / tuic）；若仍有可用节点，启动照常成功 ⇒ 列表截图 + 日志片段 | ⬜ |
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
| 协议 | TUIC v5、SSR | QUIC 另一套 / 已淘汰；机场占比极低 |
| 协议特性 | REALITY 的 uTLS 指纹模仿（JA3/JA4 ≠ Chrome）、Vision 的 TLS-in-TLS long padding / 直通、VMess `alterId≠0` / chunk masking / global padding | REALITY 主体与 Vision 功能链路（含上行首帧 padding）已支持；指纹模仿需要自研 uTLS；其余是老式认证 |
| SS-2022 | UDP | TCP（含多用户 `iPSK:uPSK`）已支持；UDP 需另写 SIP022 数据报封装 |
| AnyTLS | 填充策略、多流复用 | 当前「一条入站一个 Stream」，服务端接受这种用法 |
| Hysteria2 | Brutal 拥塞控制 | quinn 不开放可插拔 CC；`Hysteria-CC-RX` 恒发 0，让对端按自己的 CC 自适应（gecko / UDP 已支持） |
| 传输 | `kcp`、XHTTP `packet-up`、v2ray-plugin 的 `quic` 模式、simple-obfs 之外的 SS 插件（kcptun / cloak / restls） | `http` / `h2` / `grpc` / `xhttp`（stream-one / stream-up）与 SS 的 simple-obfs（http / tls）/ v2ray-plugin（websocket）已支持（2026-11-03）；`kcp` 需完整 ARQ + FEC + 老式混淆/加密且无真机可验收（见 todo G3b） |
| 规则 | `GEOIP` / `RULE-SET` | 需要外部数据库；「国内直连」退化为「只本机 / 私网直连」 |
| 其它 | TUN、Clash 控制口 / 策略组、规则热切换、Mux 多路复用、自定义 CA / `client-fingerprint` | TUN 需管理员 + wintun；策略组随 mihomo 退场一并消失；测速已改为经节点 HTTP 探针（目标 = 直连可达的 `www.baidu.com/`，且未开启服务也能测） |

## 六、维护规则

1. **新增一条能力** ⇒ 在第三节加一行，必须同时填「测试函数名」；写不出测试名就填 `—` 并在状态里用 ⬜，不要填 🟡。
2. **真机通过** ⇒ 必须先在 §四 按「具体步骤」照做一遍、把「成功体现」里的证据文件落盘，再改状态与日期 / 证据路径，不删行；
   失败也记（状态保持 🟡 并在备注里写现象）。只有文案没有留档 ⇒ 仍是 🟡。
3. **删代码** ⇒ 删对应行；不要留「已废弃」行。
4. 本节表格的基数必须与 `cargo test --bin lunac native_proxy -- --list` 的条数一致；不一致说明表过期。
