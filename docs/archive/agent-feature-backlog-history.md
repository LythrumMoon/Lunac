# Agent 待办清单——已完成 / 已闭环历史归档

> **本文档是什么**：`agent-feature-backlog.md` 中**已完成 / 已闭环**条目的压缩归档。每条只留「编号 + 标题 + 一句话结论 + 验证码 / 证据」，**不再保留逐条过程**（过程与实测数据的完整版见 Git 提交记录与 `docs/archive/ai-spec.md` §11 规则）。
>
> 建立于 2026-10-11 文档整合。**新规则一律写进 `docs/` 根下那 6 份现行文档，不要再回写本文件。**

---

## 一、桌面宿主 / 性能类事故（闭环）

| 编号 | 标题 | 一句话结论 | 证据 / 验证码 |
|---|---|---|---|
| M2-10 | 聊天窗整段会话渲染成 DOM ⇒ renderer 3.87 GB | 聊天流窗口化渲染（`CHAT_RENDER_WINDOW=120`）+ 动作条懒创建 + 提问节点窗口化 | CDP 实测 45,216 条消息 → 1,088,429 节点 / 3.87 GB；规则 76 |
| M2-12 | `chat.db` 298 MB / 一份会话 4.5 万条 | 根因 = `pruneContext()` 数组别名（`turns.push(current)` 忘了 `current = []`），一行修复 | 探针 `26→312→3744→44928→539136`，修复后 `26→24→24`；库去重重写 **298.54 MB → 0.81 MB**；规则 77 |
| M2-13 | 代理插件落地（系统代理 + librespot 联动） | `system_proxy.rs` + `librespot_set_proxy` + `proxy.ts`（多页）接上；`-x` 收进 `librespot_args()` | 已发布 6 条（`gh api` 核实）；§4.11；预检 #46 |
| M2-14 | 插件子进程必须随插件退出而终结 | `child_job.rs` 提为共用模块 + 每个 `spawn()` 后 `assign` + `stop_cli` 走 `taskkill /F /T` | `cargo test --bins` 178 passed；预检 #57 / 规则 78 |
| M2-9 | 音乐插件长连接断开后 `404 NO_ACTIVE_DEVICE` | 控制命令**不带** `device_id`（对准活跃设备），吃 404 则 `transfer_to(play:false)` 扶正后重发一次；Spotify 429 = 端点级长处罚 → 少打 + 按 `Retry-After` 停够 + 保留手动恢复 | §4.6「Spotify Web API 限流」；预检 #62 / #63 |
| M2-16 | 缓存 miss 归因：71 万 token 来自后台命令输出无预算 | A–E 五条全落地（含 D：往期索引 / 记忆块移出 `system`、改注 history 开头） | `cargo test --bins` 147 passed；规则 18 / 22 / 23 / 53 / 56 / 70 |
| M2-17 | 新建对话与上个对话「模型级隔离」 | 彻底移除「往期会话索引」自动注入；三处新会话入口清 `pendingAgentHistory` | core-agent `cargo test` 148 passed；规则 53 / 56 |
| M2-18 | 「AI 自建东西该放哪」四分法 | `env_block()` 新增 `placement_line`（插件→`Modules\`、技能→`skills\`、工具→`tools\`、其它→工作目录） | core-agent 148 passed；§20.1 |
| — | 权限三档改名 | `项目/完全` → `工作区/系统`（差别 = 是否受工作区限制）；`full` 仍关掉工作区锁（刻意保留） | i18n 5 语言；规则 21 |
| — | 代理插件前端重构 | 4 页（节点/订阅/日志/设置）+ 自制分段控件（消灭原生 select 与 number spinner）+ 去解释文案 | `tsc` / `vite build` / `build-plugins -Plugin proxy` 通过 |
| — | 简洁搜索结果区去除行内提示（2026-11-02） | 应用 / 插件 / OCR / 转换 / 自定义启动行只留「图标 + 标题」；**AI 助手 / 备忘录 / 网页搜索** 保留副标题显示键入内容，网页搜索固定 `搜索引擎:内容` | `npx tsc --noEmit` exit 0；规则见 `ui-style.md` §界面 10 |
| — | 代理面板日志贴底 / 去 native / 拉回拉手 / 拖动修复（2026-11-02） | 面板底部 padding 归 0 + 空消息行不占位（`gapBelowLog 0`）；删除左上 `native` 徽标；抽屉「拉回」改 60px 竖向 `<` 拉手（第二轮改到右端）；拖动改 pointer + mouse 双通道、命中区 16px、上限不再预留 360 | headless 1280×720 实拖：190→250；`cargo test --bin lunac native_proxy::` 56 passed |
| — | 代理插件第二轮收敛（2026-11-02） | 日志删展开/收起（常开）；抽屉删数量与「刷新服务器」；设置删「系统代理 / 本机播放走代理」，新增直连/代理域名编辑（落到 `mihomo.direct_domains/proxy_domains`，rule 模式优先于订阅规则）；网页搜索右键菜单去引擎 icon | `tsc` exit 0；`cargo check`（target-check）exit 0；proxy 0.10.3 |
| — | 代理插件第三轮：规则配置文件 / 模式两档 / TUN 闸门 / 订阅即刷新（2026-11-02） | `direct_domains/proxy_domains` 改为 `rule_mode`（blacklist/whitelist）+ `rule_domains`（约 90 条国内默认清单），名单层排在订阅规则之前；设置页改为「规则配置文件 + 黑/白滑轨 + 单份域名清单」；模式只留规则 / 全局；新增 `mihomo_is_elevated` / `mihomo_relaunch_elevated`，TUN 未提权时弹独立警告框、同意后提权重启；删除订阅独立面板，工具按钮直接刷新服务器；日志拖拽改为 mouse-only（避开 WebView2 `pointercancel`）；dev 下 TUN 闸门只提示用管理员终端重启（不自动重启），release 才提权重启 | `tsc` exit 0；`cargo check`（target-check）exit 0；`cargo test --bin lunac native_proxy::` 56 passed；proxy 0.10.5 |
| — | 对话流中途三态控制（§3-28.1，2026-11-02） | core-agent 新增 `pause` 输入与轮首/轮末 `drain_steering`：`steer="insert"` 的 user 在安全点注入、其余消息进 backlog；暂停强制落检查点 + `result.subtype="paused"`；前端「更多设置 → 生成中发送」三档，队列条改为立即插入；回合外到达的插入由主循环丢弃并提示重发 | `cargo test` 461 passed / 15 ignored；`tsc` exit 0；`cargo build --release` exit 0（待真机） |
| — | 自研引擎补 VLESS + REALITY（§3-25 第九轮，2026-11-02） | 新增 `reality.rs`：自写最小 TLS 1.3（X25519 key_share、HKDF-Expand-Label、Finished、AES/ChaCha 记录层）+ REALITY SessionId 封包（AuthKey 为 `HKDF-SHA256(X25519(eph, server_pk), salt=Random[:20], info="REALITY")`，AAD = SessionId 全零的 ClientHello）+ 临时证书 `HMAC-SHA512(AuthKey, ed25519_pk)` 绑定；因 ring 的私钥只能协商一次，自带 TweetNaCl 风格 X25519；`clash.rs` 解析 YAML `reality-opts` 与链接 `pbk`/`sid` | `cargo check` exit 0；`cargo test --bin lunac` 421 passed / 2 ignored（native_proxy 65 passed）；真机验收见下一行 |
| — | VLESS Vision 功能兼容（2026-11-02，G1 第一段） | VLESS 头写 protobuf `Flow=xtls-rprx-vision`（`0A 10 …`，addon_len=18）；新增 `VisionStream` 按 `[uuid:16][cmd][clen][plen][content][padding]` 解下行填充，`cmd=0` 续帧、`1/2` 结束转裸流；首帧无 UUID 时按裸流透传（对齐 Xray `XtlsUnpadding` 兜底） | `cargo test --bin lunac native_proxy::` 68 passed；待真机 |
| — | **VLESS + REALITY + Vision 真机打通（G1/G2 闭环，2026-11-02）** | 用官方 Xray 26.10.31 做对照（同一节点 `curl -x` 得 204）后逐层定位四个真 bug：①VLESS 地址类型误用 SOCKS5 的 1/3/4，服务端把域名当 IPv6 → 改用 Xray `protocol.AddressType` 1/2/3；②`connect` 提前阻塞读响应头，而 Vision 服务端要拿到首段数据才回响应头 → 改为「请求头 + 首段业务数据一次发、响应头推迟到首次读」；③Vision 上行首段按 `[uuid][cmd=End][clen][plen][content][padding]` 打帧后转裸流；④对端 `close_notify` 当 EOF 而非错误。另修 TLS 记录 AAD 与 `HKDF-Extract(0,0)` 的零字节 IKM | `live_node_fetches_generate_204` 对 `us1.oh-node.com:443`（REALITY + Vision）取到 204；`cargo test --bin lunac` 426 passed / 3 ignored；`cargo test --bin lunac native_proxy` 70 passed / 1 ignored；`cargo check` exit 0 |
| — | **传输层 `http` / `h2` / `grpc` / `xhttp`（G3，2026-11-03）** | 新增 `http_obfs.rs`（`PUT <path> HTTP/1.1` + 头，200 后转裸流）与 `h2tunnel.rs`（`network: h2` 的 POST 双向流；`network: grpc` 的 `/<service>/Tun` + `Hunk` protobuf，路径规则空 = `GunService`、`/` 开头取 `|` 前段；`network: xhttp` 按 Xray `splithttp` 对齐：stream-one 单 POST 双向流、stream-up 为 GET 下载 + POST 上传，路径带 UUID session、每个请求必须带 `Referer: …?x_padding=`（100–1000，服务端校验））；阻塞中继 ↔ 异步 h2 沿用 Hysteria2 的线程 + 双通道；`clash.rs` 放开 `network` 白名单并解析 `http-opts` / `h2-opts` / `grpc-opts` / `xhttp-opts`、链接 `type` / `serviceName` / `authority`、vmess JSON 的 `net`。修三个真坑：①h2 的 `poll_capacity` 只在容量增加时就绪，初始窗口要先读 `capacity()`；②h2 连接 future 必须与收发一起轮询（放进 `select!`），单靠 `tokio::spawn` 出现过「HEADERS 已发、DATA 不刷」；③本地 h2 测试服务端的 `accept()` 必须持续轮询、请求丢独立任务。`kcp` 明确不做（G3b：需完整 ARQ + FEC + 老式混淆/加密，且无真机可验收），继续在解析阶段跳过 | `cargo check` exit 0；`cargo test --bin lunac` 453 passed / 4 ignored；`native_proxy` 97 passed / 2 ignored；`rustfmt --check` 通过；真机 204 待补（无 h2 / grpc / xhttp 节点） |
| — | **UDP 上游：VLESS / Trojan / VMess / AnyTLS（G5 第一段，2026-11-02）** | 新增 `vless_udp.rs`：Vision 节点按 Xray 客户端行为把 UDP 改写成 Mux（`v1.mux.cool`）+ XUDP，单连接承载所有目标；新增 `trojan_udp.rs`（`cmd=0x03`，每包自带地址）、`vmess_udp.rs`（`cmd=0x02`，每目标一条连接）与 `anytls_udp.rs`（sing-box udp-over-tcp v2，`sp.v2.udp-over-tcp.arpa` + `isConnect` 头）；`udp.rs` 懒建上游并按目标地址回填 SOCKS5 数据报。修两个真 bug：①`cmd=Mux` 的 VLESS 头不带端口/地址（多写会被服务端当业务数据）；②XUDP 的地址是 `protocol.PortThenAddress`（端口在前），不是 SOCKS5 的地址在前 | 真机 `live_node_answers_dns_over_udp` 经节点查 `1.1.1.1:53` 收到同事务 ID 回包；`cargo test --bin lunac` 440 passed / 4 ignored；`native_proxy` 84 passed / 2 ignored；`cargo check` exit 0。Hysteria2 的 UDP（QUIC datagram + UDPMessage 分片）归入 G6 |
| — | **SS-2022 多用户 PSK（G10）+ 经节点真实测速（G14，2026-11-03）** | G10 按 SIP023：密码写 `iPSK:uPSK`，salt 之后按层插入 16 字节 identity header（`blake3::derive_key("shadowsocks 2022 identity subkey", iPSK \|\| salt)` 派生 AES 单块密钥，明文 = `blake3::hash(next_psk)[..16]`），会话仍用最后一个键（uPSK）派生子密钥；顺手把 salt + identity + 固定头 + 变长头改为一次 write（SIP022 §3.1.4）。G14 把 `delay()` 从「到节点 TCP 建连耗时」改为经节点 `HEAD http://www.gstatic.com/generate_204`，量到首个响应字节；失败如实报错、不回退 TCP 建连；`server::dial()` 抽出 `dial_node()` 复用 | `cargo test --bin lunac` 467 passed / 4 ignored；`native_proxy` 111 passed / 2 ignored；`rustfmt --check` 通过；真机多用户 204 与延迟对照待补 |
| — | **SS 的 SIP003 插件（G4，2026-11-03）** | 新增 `ss_plugin.rs`：simple-obfs `http`（首包前 WebSocket 升级请求 + `Content-Length`，之后裸流）与 `tls`（假 ClientHello，首个载荷藏 `session_ticket`，之后 `[0x17 03 03][len][payload]`；响应头 107 字节先剥掉）逐字节对齐 `shadowsocks/simple-obfs`；v2ray-plugin `websocket` 复用 `transport.rs`。`SsTunnel` 的底层流改为 `Box<dyn Tunnel>`，插件插在 TCP 与 SS 头之间；`clash.rs` 解析 YAML `plugin` / `plugin-opts` 与链接 `?plugin=obfs-local;obfs=…`，未知插件 / `quic` 模式在解析阶段跳过（不退回裸 SS）；`udp.rs` 明确「插件只承载 TCP」。不做：v2ray-plugin `quic`、kcptun / cloak / restls | `cargo check` exit 0；`cargo test --bin lunac` 466 passed / 4 ignored；`native_proxy` 110 passed / 2 ignored；`rustfmt --check` 通过；真机 204 待补（无带插件的 SS 节点） |
| — | **Hysteria2 `gecko` 混淆 + UDP 转发（G6，2026-11-03）** | 按 `apernet/hysteria` 的 `extras/obfs/gecko*.go` 对齐：Gecko 在 Salamander 之上只切 QUIC **长头**包（首字节 `& 0x80`），分 2–8 片、每片补到 `[min,max]`（缺省 512/1200），帧为 `[0x80][msgID][idx:4|total:4][padLen u16 BE][pad][chunk]`；短头包原样通过。新增 `hysteria2_udp.rs`：UDP 走 QUIC 不可靠数据报，`UDPMessage` = `[session u32][packet u16][fragId u8][fragCount u8][varint addrLen][host:port][payload]`，支持分片 / 重组（TTL + 上限），`Hysteria-UDP: false` 直接报错；抽出 `open_connection` / `authenticate` 供 TCP 与 UDP 共用，UDP 数据报缓冲开到 1 MiB。`udp.rs` 接上 Hysteria2 上游。`clash.rs` 支持 YAML `obfs: salamander|gecko` + `obfs-password` + `obfs-min/max-packet-size`、链接同名字段；obfs 写错 ⇒ 跳过整个节点（绝不退回无混淆）。Brutal 明确不做（quinn 无插拔 CC，`Hysteria-CC-RX` 恒发 0） | `cargo check` exit 0；`cargo test --bin lunac` 460 passed / 4 ignored；`native_proxy` 104 passed / 2 ignored；`rustfmt --check` 通过；真机 gecko / UDP 待补（无对应节点） |

## 二、Agent 能力与协议（闭环）

| 编号 | 标题 | 一句话结论 | 证据 / 验证码 |
|---|---|---|---|
| A13 扩展 第 1–4 步 | 项目级 MCP（`.mcp.json`，http + stdio） | stdio 传输泛化 + 读 `<cwd>\.mcp.json` + 信任门 + 推迟到首次查询前连接 + 前端信任卡 | 3 条单测；§20.1；规则 18 |
| A13 扩展 第 6 步 | 审批卡 / 计划卡「一大段文档」 | 真凶是计划卡 ⇒ 长计划默认折叠（`PLAN_PREVIEW_LINES=12`），**不用摘要替换原文** | agent-ui-spec §9；规则 68 |
| A13 扩展 第 5 步 | AI 运行时自建技能 | 工作区锁对 `<exe 根>\skills\**` 开**受审批**的例外（只放行、不代替审批） | 单测 `skills_dir_is_the_only_workspace_lock_exception`；规则 67 |
| — | MCP 配置改动检测 + 自动重启（两版） | agent 检测（空闲轮 `recv_timeout(1s)` + 事件带 `idle`）、前端执行；空闲立即重启、回合中等 `result`；只报一次 | 单测 `config_watch_reports_once_and_covers_new_files`；`cargo test --bins` 146 passed；规则 69 |
| — | MCP 其余能力 | Streamable HTTP / OAuth 2.1 / prompts / roots / elicitation 全部落地（协议升 `2025-06-18`） | 规则 55 / 84；`mcp_oauth.rs` |
| — | 内置工具与条件注册 | `defs()` 恒 **19 件**（17 件 + `PetControl`（2026-10-10）+ `NotebookEdit`（2026-10-11））；`Skill` / resources×2 / prompts×2 / `Remember` / `ImageGen` / `mcp__*` 条件注册 | 守门单测 `agent_tool_is_registered_and_filterable` |
| — | 权限 hooks（A9） | 8 事件介入；只认显式拒绝；hook 的 `allow` 只跳过审批卡；mtime 热重载 | 规则 61 |
| — | 结论：长期记忆「从未写入」 | 非缺失——`Remember` 靠模型主动调、复盘按 10 次提问触发且计数随重启清零 ⇒ 门槛够不着（是否调低待定） | 2026-10-07 观察 |
| §待办 2 补 | 可恢复结论索引（压缩丢结论 ⇒ 模型回到初始问题重答） | 压缩边界 + 轮末两处写 `temp\conclusions\<scope>.jsonl`；检索重注入紧随压缩摘要（可变层）；`system` / `tools` 前缀逐字节不变 | agent-e2e 实测（2026-10-22，`temp\agent-e2e\20261022-120249` / `-120351` / `-120627`）：`status=failed` 带 `reason` 与文件指针；压缩丢弃 3/5 条后日志「结论索引检索 … 命中 1/5 条」「注入 1/5 条」，模型正确答出被丢段里的 `ORBIT-7734` / `DROP-PROBE-404`；7 次提问 `system` / `tools` 哈希全同 |
| §待办 2 补二 | 语义分层压缩（L1–L6） | Elide 候选限定 L5、`Drop` 按层收缩边界且保配对；`Force` 不走层感知但被丢段进摘要 / 索引 | 单测 5 条 + agent-e2e（2026-10-22）：`/compact`（Force）丢弃 5 条（含 `is_error` 结果）后 `history` 仍以原用户诉求开头、失败条目在索引里 `status=failed`，模型答出 `DROP-PROBE-404`；`system` / `tools` 前缀不变 |
| §待办 3-9 B | `MemorySearch` 只读检索工具 | 免审批、只能模型主动调；读走既有 `lunac/memory_read` 桥，不新造读取路径 | 端到端（2026-10-22，`temp\agent-e2e\20261022-120548`）：~7.9 KB 记忆文件、needle 在 6000 字截断点之后 ⇒ `MemorySearch 命中 1 条（reason=ok）`，模型答出 `ORBITAL-FERRET-9`；`-McpExe` 相对路径 spawn 失败的坑已修（脚本转绝对路径） |
| §待办 4 S2 切片 | 任务级预算（跨提问累计） | `LUNAC_TASK_BUDGET_TOKENS`（默认 0 = 关）；口径与回合预算逐字一致；撞顶后**不再续命**（当前问先收口、后续每问最多一个回合预算），但**不硬拒绝新提问**（「预算不终止计划」）；`0` / 坏值退回关闭 | `cargo test` 303 passed / 5 ignored（+6 条）；agent-e2e（2026-10-22，`temp\agent-e2e\20261022-124258` / `-124451`）：单问跨顶 ⇒ `turns=2`、`budget_exhausted` 1 条；两问 ⇒ 第二问日志「任务级预算已用尽…本回合禁用预算续命（仍可作答）」且事件总数仍为 1；提示语已加「来自 harness、不是工具输出」的来源限定（首轮实测模型误当文件内容，见 `-124027`） |
| §待办 4 S2 切片 | 检查点 + `--resume`（跨进程续跑 / 预算持久化） | 新增 `core-agent/src/checkpoint.rs`：每回合 + `set_history` 后落 `history` / 会话 id / 提问序号 / 任务预算累计到 `ModuleData\tasks\sessions\`（原子写 / ≤32 MiB / 留最近 20 份）；`--checkpoint` 写、`--resume <id\|latest>` 读并沿用原会话 id；默认不写 ⇒ 宿主路径逐字节不变；读不到 exit 2 | 单测 6 条（5 条 checkpoint + `cli_checkpoint_and_resume_parse`）；`cargo test` 303 passed；release 实跑 `temp\agent-e2e\20261022-checkpoint`：A `--checkpoint` 落 q1/history 2/预算 6866；B `--resume latest` 恢复同 session、q2 续编、预算 6866/1000 撞顶仍答出 `BANANA-42`，再落 q2/history 4/预算 13758 |
| §待办 4 S2 切片 | `--continue`（跨回合续跑的 CLI 原语） | 单发下隐含 `--resume latest` + `--checkpoint`；没写 query 时用内置「接着做、别重复已完成步骤」提问且**不读 stdin**；常驻模式只 warn 不生效；调度循环留给外部（agent 不自循环，防无人值守时无限花钱） | 实跑 `temp\agent-e2e\20261022-continue`：A 建 `CONTINUE-MARKER-77`（pending）→ B 仅 `--print --continue` 即接上文并复述该待办；q1/history 4 → q2/history 6；无检查点时 exit 2 |
| §待办 4 S2 切片 | 调度层最小版（退出码 3 + `agent-task-loop.ps1`） | 单发新增退出码 **3 = 正常结束但 TodoWrite 清单未完成**（仅带检查点时出现，默认仍是 0/1）；`scripts\agent-task-loop.ps1` 循环 `--print --checkpoint` → `--print --continue`（`-MaxRounds` 封顶、`-WorkspaceDir` 同时作为 cwd、默认只读、写类需显式 `-SkipPermissions`） | `cargo test` 304 passed / 5 ignored（+`single_shot_exit_code_distinguishes_failure_and_unfinished`）；`npm run verify` 8/8 通过（新脚本恰一份 BOM）；实跑 `temp\agent-e2e\20261022-exit3` 得到 exit 3、`temp\agent-e2e\20261022-task-loop` 第 1 轮 exit 3 → 第 2 轮读出 `FIRST-LINE-MARK-52` 并 exit 0 |

## 三、调音引擎与音乐插件（闭环）

| 编号 | 标题 | 一句话结论 | 证据 / 验证码 |
|---|---|---|---|
| L5 S0 | DSP 引擎内核 | `tuning-engine/` 独立 crate + 七模块 + 四子命令 CLI；「实测增益 = 理论 ±0.1dB」 | 54 条单测；端到端 `+5.94dB = +5.94dB`；§4.10 |
| L5 S1 | 自身播放链路闭环 | 引擎以**库依赖**进宿主，套在本地播放链上；默认关闭（opt-in） | §4.6「调音」；预检 #51 |
| L5 S2/P4 | WASAPI loopback 闭环测量 | 两趟（直通量系统 / 走链量最终）相除得到「仅自身链（实测）」；面板画三条线 | 单测 `two_pass_subtraction_isolates_the_chain_from_the_system` |
| L5 P1–P5-7 | 调音页对齐 Peace | 可编辑链 + 数字界面 + 频响曲线独立窗 + 扬声器声道槽 + GraphicEQ / 卷积 / 延迟 / 复制 + 预设管理 + 默认档改 flat + 曲线反向同步 | 引擎 `cargo test` 96 绿、宿主 193 绿、`tsc` / build 通过 |
| L5 P7 | 在线音乐也进调音 | librespot `-B pipe -f F32` + `live_audio.rs`（无锁 SPSC ring）把裸 PCM 喂进**同一个** `TuningSource` | 宿主 `cargo test --bins` 199 绿 |
| L5 剩余 | A-B 盲测 | 两个快照槽 + 随机映射 + 揭晓；随机只在前端、不跨挂载；**不做自动等响度** | 实机验收通过；规则 73 / 预检 #73 |
| L9 | 音乐插件剩余项 | ①媒体库（`media_lib.rs` + `lofty`）②本地播放（`player.rs`，`rodio`）③播放列表解析 全部落地；**连接 QQ / 网易云经用户决定取消** | §4.6「本地媒体库」；预检 #47 / #49 |
| L7 | AI 聊天独立界面（第一版） | `pluginId="chat"`，加载 `index.html`，1280×720；主窗内嵌 / detached 两态去掉 | 宿主 `cargo test --bins` 112 passed |

## 四、成本与用量（闭环）

| 编号 | 标题 | 一句话结论 | 证据 / 验证码 |
|---|---|---|---|
| L6 归因 | 「Lunac 金额是 Trae 10 倍」 | 结案：**不是计价贵**——11.2 倍 = 命中率差 3.09× × token 结构差 3.63×（官方峰谷定价）；KPI 改用「每次提问成本」 | §9.1 难点 3 |
| L6 六件 | 降本措施 | 子代理 / 复盘用量并入 `result.usage`、价格表时段价 + 预置峰谷价、输出瘦身（成本 -41%）、对账基线工具 | §3.5；规则 63 |
| L6 跨源基线 | 真实账单对账 | 修掉两个真问题：① 节假日全天按谷价（修正 0.979 元）② 本地账漏记没跑完的回合；**验收口径 = 差额 0.000000、exit 0** | `reconcile-usage.ps1`；规则 87 |
| 规则 88 | 输出 token 是第一杠杆 | 输出占账单 61.7%；① 改已存在文件用 `Edit`（Write 调用 → 0）② 思考是最大单项且不回灌 ③ `max_tokens` 是三合一池（已抬 `THINKING_HEADROOM_TOKENS`） | 平台用量 + 自建 A/B 实测 |

## 五、桌宠 × AI（闭环）

| 编号 | 标题 | 一句话结论 | 证据 / 验证码 |
|---|---|---|---|
| L1 | Live2D 桌宠本体 | 独立透明置顶窗 + 引擎（`pixi.js@7` + `pixi-live2d-display-lipsyncpatch`）+ Core 自托管（带版本 + sha256 + 授权文件）+ 发布 | 真机渲染 18215 非透明像素；§4.8 |
| L1-A/B | 桌宠三条裁决 | 形态=完全独立窗（第二份 WebView2，边际 +1 renderer / 工作集 +109 MB）；模型由用户自备；实施顺序不动 | architecture-rendering §6.2 |
| M2-19 | 桌宠 × AI（① ② ③） | 手动抓屏 `screenshot_monitor` + 「看一眼屏幕」；对话框 + 主动反应（默认关）；用量来源 `source="pet"` + 成本面板来源下拉 | 契约 `pet-chat-send` / `pet-chat-reply`；§4.5 |
| M2-20 | 桌宠穿透开关对齐 + 专属人格 | 穿透改切换按钮 + 双向同步；`config\pet-persona.md` 按轮注入（即时生效、不重启） | §4.5 |
| M2-21 | 音乐插件队列 / 位置 / 视觉 / curl 导入 | 队列页 + 自定义队列 + 播位恢复 + 警告区不占位 + 拖动条减半 + curl 一键导入 | `tsc` exit 0；`build-plugins -Plugin music` |
| M2-22 | 桌宠 × AI 第二轮（多批） | 对话独立成线程 + 历史分页；间隔可自定义；默认人格；自动弹窗；TTS 语音朗读；`thinking` 按轮关；`emit_to` + `getCurrentWindow().listen`；热键改 AI 触发 + 默认键；表情可叠加开关 + 三方参数合并 | `tsc` exit 0；`build-plugins -Plugin pet`（v0.9.31） |
| A 批 | 桌宠窗口几何 + 输出框 + 语音 + 设置页（2026-10-22 真机验收） | 输出框封顶 255 / 窗口尺寸记忆 / 画布随窗与模型贴底 / 最小模型区；输出框贴顶无幽灵空行；打字机与语速、语音音量三条拉条互不影响；设置窗三页 + 人格页四项 + 建议 / 三张列表清空（两段式确认） | 真机验收通过；条目已从 `todo.md` 删除 |
| A 批 | 动作链 / 下拉组件 / 桌宠抓屏（M2-10 / M2-11 / 1-4 ①②） | 动作链折叠 / 换动作填时长 / 一行四控件 / 整链循环；下拉组件化并收编桌宠三处；自动 / 定时抓屏与屏幕变化触发（默认关 + 频次上限） | 真机验收通过；同日修为「循环列表热更新」：勾选不再重挂模型，`pet 0.9.64` |

## 六、界面与交互（闭环）

| 编号 | 标题 | 一句话结论 | 证据 / 验证码 |
|---|---|---|---|
| agent-ui-spec 阶段 1/2 | 对话区改造 | 思考块规范化 + 命令卡片 + `cli-stderr` 落 `.sys-note-error` + 回合自动折叠 + 运行方式三档 + 安全档位入口 + 越界卡片三动作 | agent-ui-spec §10 阶段 1/2 已落地 |
| U1 | 代码变更 diff 卡 | **事后审阅**（接受=标记已阅 / 拒绝=写回快照）；三级粒度；diff 由 `text-diff.ts` 有界重算 | `tsc` / `npm run build` exit 0；规则 86 |
| U2 | 多轮缩略导航 | 历史抽屉可摊成「轮次缩略」，点某轮 = 恢复并跳到那一轮；会话 Fork 取消 | `tsc` / `npm run build` exit 0 |
| M2-15 | 待办落盘 + 审批卡停靠 + 提示音 | 待办按回合存 `chat.db`；审批 / 计划卡挂 `#approval-dock`；提示音走宿主 `MessageBeep` | 规则 79 / 80 / 81；`cargo test --bins` 180 passed |
| M2-6 | 中断后飞行中命令卡收尾 | 三处调 `markFlyingCardsStopped`（停止 / closed / 回退）；只改视觉、不置 `card.done` | 规则 75 |

## 七、工程 / 发布 / 卫生（闭环）

| 编号 | 标题 | 一句话结论 | 证据 / 验证码 |
|---|---|---|---|
| §8.2 | 打包流程收敛 | 九步 → 八步；PaddleOCR 移出安装包（改 `ocr` 插件 `archive` 依赖，110.3 MB → 15.9 MB）；`ext-plugins` 用**新鲜度**守卫 | §8.2；`cargo` / 7z 校验 |
| §8.3 | 仓库卫生 | `_reference/core/` 归档 + gitignore；`.env` 不入库；大二进制不入库；`cli.exe` 不进包（打包前后双校验） | §8.3 红线 |
| L12 | 插件体系扩展 | ① 插件可自带 UI 框架写进 README ② sidecar 契约定稿并实施（宿主代请求 + 独立信任门 + 面板绑定） | `cargo test --bins` 206 passed；规则 71 |
| — | BOM 纪律 | `.ps1`/`.nsi` 恰一份 BOM 的判据与归一 + `commit.ps1` 自动愈合 + `npm run verify` 第 ⑧ 节守卫 | 规则 82；预检 #61 |
| — | 旧 CLI 差距面 | `getAllBaseTools()` 每个名字已归位（实现 / 待办 / 设想）；旧编号 `backlog §0–§8.x` 对应条目**全部完成并从清单删除** | backlog §5 边界声明 |
| 1-7 | 测试夹具「伪凭据字面量」与 secret scanning 误报 | 夹具改为拆段拼接（运行时字符串不变、断言未放宽）；GitHub 上 5 条合成假值告警**已由用户处理**（2026-10-22）。**未落地**：`verify-git.ps1` 的本地只读扫描建议（为保 `[8/8]` 分段未加） | 仅 `#[cfg(test)]`；2026-10-22 用户确认 |
| 3-8② | `live2d-split` 目录迁移 | **2026-10-22 用户裁定不做**：该目录不迁往 `transStorage`，条目已从待办删除 | 用户裁定（无需代码） |
| S2 | 检查点只记回合末尾，中断后续跑可能重跑工具 | **2026-10-22 补中间态**：每批工具结果回灌后立即落盘 ⇒ 续跑不重跑已完成的工具；新增 `approval`（审批 / 工作区口径，只对比告警、不自动沿用）；`cargo test --bins` 306 passed；真机取证 `temp\agent-e2e\20261022-midcp` | `testing.md` §测试 17 第 7 步 |
| 3-1d | cron 每次运行都是新会话 | **2026-10-22 接上检查点**：单发固定 `--checkpoint`，job 有 `lastSessionId` 且检查点还在时追加 `--resume`；`cron-state.json` 记会话 id，续跑失败自动丢弃；检查点改为按会话各留 20 份（全局 200）；host `cargo test --bins` 318 passed；真机步骤待 dev（`testing.md` §测试 19） | `app/src-tauri/src/cron.rs` |
| S2 | 长任务只有前台脚本循环 | **2026-10-22 通用任务队列首版**：宿主 `task_queue.rs`（3s tick / FIFO / 只读档 / 与 cron 共用单飞闸）+ agent `TaskEnqueue` / `TaskList` / `TaskCancel`；多轮用 `--continue --resume`，退 3 续跑；core 309 + host 321 passed；真实模型取证 TaskList 读侧与单发硬闸（`temp\agent-e2e\20261022-taskqueue`）；真机步骤待 dev（`testing.md` §测试 20） | `task_queue.rs` / `tools.rs` |

---

> **本归档不承载规则**：规则正文见 `docs/` 根下 6 份现行文档；历史过程见 `docs/archive/ai-spec.md` §11 与 Git 记录。

