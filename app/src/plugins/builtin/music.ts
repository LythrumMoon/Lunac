// ── 音乐插件（歌词 + Spotify 播放控制）──────────────────────────
// 2026-09-27 重构。用户这一轮的要求（逐条落到下面各段）：
//   · **删掉手搜歌词**与其候选区 —— 改成「播放时自动抓这首的歌词」，主源 LRCLIB、
//     兜底源网易云（多源逻辑全在宿主 `music.rs` 的 `lyrics_get`）；
//   · 默认界面 = **歌单列**（抓用户歌单 → 展开看曲目 → 点曲目从它开始播）；
//   · 播放界面 = 三块定尺：封面 100×100 / 歌词 450×65 / 控制条 450×35，
//     控制条里「随机三态 + 上一首 + 播放 + 下一首」居中，右端一个**播放列表**（队列）按钮；
//   · **播放停止不再自动退回默认界面** —— 只有用户自己点「返回」才退。
//
// 分工：前端只画界面 + 轮询；**一切联网都在宿主**。原因：`tauri.conf.json` 的 CSP 是
// `default-src 'self' https://asset.localhost`，不含远端域 ⇒ 插件里 `fetch()` 会被拦掉
// （与插件市场索引同一条理由，ai-spec §11 规则 67）。
//
// 两条硬限制（**不假装能做**，见 ai-spec §4.6）：
//   · Spotify Web API **没有 Smart Shuffle 接口**（连读都读不到）⇒ 三态只能是
//     关 / 随机 / 单曲循环；
//   · **没有「删除队列项」接口** ⇒ 队列里的「移除」用「点它 = 从它开始播」代替，
//     文案如实这么写，不说「删除」。
//
// 纪律：
//   · **不整块 innerHTML 重绘**：每 1s 一次轮询，整块重绘会打断输入框焦点、
//     把歌词滚动位置打回顶部。只 patch 具体节点（见 renderPlayer / renderLyrics）。
//   · 轮询用 setTimeout 链 + busy 标志（不是 setInterval）—— 网络慢时调用不会叠起来。
//   · 关面板/切插件后必须停表：root 脱离文档（`isConnected === false`）即自停，
//     另外 attach.ts 的 detachPluginListeners 会显式调 `window.__lunac_music_stop`。

import type { Plugin, PluginResult } from "../registry";
import { t } from "../../i18n.js";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-shell";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
// 「本地音乐」页签选目录用。**静态 import**（与 convert.ts 一致）：磁盘插件包是
// `inlineDynamicImports` 的单文件产物，静态 import 才能保证它被内联进去。
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
// 读**本窗自己的 label**：音乐插件的两扇窗（主窗 `plugin-music` / 曲线窗
// `plugin-music-curve`）共用这一份代码，靠 label 区分该画哪一个界面。
// **不需要任何权限** —— label 是 Tauri 注入在页面里的元数据，不走 IPC。
import { getCurrentWindow } from "@tauri-apps/api/window";

/** **频响曲线窗**的 key（进宿主 `open_plugin_window` 的 `key` 参数 ⇒ label
 *  `plugin-music-curve`）。与宿主 `plugin_window::CURVE_WINDOW_KEY`（`"music-curve"`）
 *  是**同一件事的两半**：宿主那半由 `plugin_id + key` 拼出来，这里只给 key。 */
const CURVE_WIN_KEY = "curve";

/** 本窗口是不是**曲线窗**。只在模块加载时算一次（窗口不会换身份）。
 *
 *  ⚠️ 判据必须与宿主拼 label 的规则一致（`plugin-<id>-<key>`）；认错的后果是
 *  「主窗里画出一张孤零零的曲线」或者「曲线窗里跑起整套音乐面板」—— 都很难一眼看出来。
 *  `getCurrentWindow()` 在非 Tauri 环境（纯浏览器打开调试）会抛，兜成空串 ⇒ 当主窗处理。 */
const myWindowLabel = (() => {
  try {
    return getCurrentWindow().label;
  } catch {
    return "";
  }
})();
const isCurveWindow = myWindowLabel.endsWith(`-${CURVE_WIN_KEY}`);

// ── 宿主 DTO ──────────────────────────────────────────────────────
interface MusicConfigDto {
  client_id: string;
  port: number;
  redirect_uri: string;
  connected: boolean;
  display_name: string;
  /** 头像 URL（顶部工具条）。授权那一次从 `/me` 抓的，没连上时是空串。 */
  avatar: string;
  /** 当前用的是不是内置 Client ID（宿主 `music.rs` 的 `BUILTIN_CLIENT_ID`）。 */
  builtin: boolean;
  /** 本机播放（librespot）的两个设置 —— 设置面板里可编辑（2026-09-28 加）。 */
  librespot_path: string;
  librespot_proxy: string;
  /** 串流质量（kbps）。只有 96 / 160 / 320 三档（宿主侧的 `LIBRESPOT_BITRATES`）。 */
  librespot_bitrate: number;
}
interface TrackDto {
  id: string;
  /** `spotify:track:…` —— 播放 / 队列 / 歌单曲目点击都要用它。 */
  uri: string;
  name: string;
  artists: string;
  album: string;
  cover: string;
  duration_ms: number;
}
interface PlayerDto {
  connected: boolean;
  active: boolean;
  playing: boolean;
  progress_ms: number;
  volume_percent: number;
  device: string;
  track: TrackDto | null;
  /** 随机播放开关；与 `repeat` 一起决定控制条上那个三态按钮。 */
  shuffle: boolean;
  /** `off` / `context` / `track`。 */
  repeat: string;
  /** 当前播放上下文（`spotify:playlist:…`）。歌单列据此标出「正在播的就是这个歌单」。 */
  context_uri: string;
}
interface LyricsDto {
  found: boolean;
  instrumental: boolean;
  track: string;
  artist: string;
  album: string;
  duration: number;
  synced: string | null;
  plain: string | null;
}
interface PlaylistDto {
  id: string;
  uri: string;
  name: string;
  cover: string;
  total: number;
  owner: string;
}
interface QueueDto {
  current: TrackDto | null;
  items: TrackDto[];
}
/** 「我喜欢的歌曲」（收藏夹）。**不在 `/me/playlists` 里**，是独立资源，见 ai-spec §4.6。 */
interface LikedDto {
  /** Spotify 报的收藏总数（可能大于 `items.length`）。 */
  total: number;
  items: TrackDto[];
}
/** 搜索结果（顶部工具条那个搜索框 + 主区的搜索详细页）。
 *  **五类一起返回**：详细页是按页签分的，一次凑齐比切页签转圈好。 */
interface SearchDto {
  tracks: TrackDto[];
  playlists: PlaylistDto[];
  artists: LibraryItemDto[];
  albums: LibraryItemDto[];
  shows: LibraryItemDto[];
}

/** 库条目（专辑 / 歌手 / 电台）—— 与 `PlaylistDto` **刻意分开**：
 *  歌手身上没有 owner / total，混用会让「有没有值」的判定散成一片。
 *  歌单在左栏会**归一成同一个形状**（见 `libItemOfPlaylist`），于是渲染只有一套。 */
interface LibraryItemDto {
  id: string;
  /** 可播上下文。**电台是空串**（`context_uri` 不认 `spotify:show:`）⇒ 只能逐集播。 */
  uri: string;
  name: string;
  cover: string;
  subtitle: string;
  total: number;
}

/** 左栏那四类 + 收藏夹（收藏夹是前端合成的、没有可播上下文）。
 *
 *  **2026-10-01 加了「最常听」栏**（用户第 4 条）：`top`（原先并列的 `new` 新发行
 *  已于 2026-10-02 删除 —— 端点永久 403）。
 *  它与那四类**共用同一套行 / 卡片 / 主区渲染**（都是 `LibraryItemDto`），
 *  差别只在「去宿主拉哪条命令」和「点开之后按哪种 kind 拉曲目」：
 *  · `top` 的元素是**歌手** ⇒ 拉曲目按 `artist`
 *  **所以 `sideCache` / `selected.kind` 里存的是 side kind（`top`），
 *  而发给宿主的 kind 要过一次 `trackFetchKind()` 映射** —— 两件事不能混。
 *  （原先还有个 `new`（新发行）同样如此，2026-10-02 整条删除 —— 见 `SIDE_CMDS` 注释。）
 *
 *  `MainKind` 另有两个**前端合成**的入口（没有可播上下文，只能逐条播）：
 *  `liked`（我喜欢的歌曲）与 `top-tracks`（最常听的歌曲）。 */
type SideKind = "playlist" | "album" | "artist" | "show" | "top";
type MainKind = SideKind | "liked" | "top-tracks";

/** 把左栏的 kind 翻成 `spotify_item_tracks` 认的 kind。
 *
 *  「最常听」那一栏的元素就是歌手，端点完全相同 —— 所以这里只是把
 *  **栏目的名字**与**资源的种类**拆开（栏名进了 `sideCache` 的键，不能直接下发）。 */
function trackFetchKind(kind: MainKind): string {
  if (kind === "top") return "artist";
  return kind;
}

/** 左栏每一类对应哪条宿主命令（歌单那条不在这里 —— 它返回的是 `PlaylistDto`，
 *  要多一步 `libItemOfPlaylist` 转换，形状与其余几类不同）。
 *  `top` 是 2026-10-01 加的（用户第 4 条「发现」）。
 *  **`new`（`/browse/new-releases`）2026-10-02 删除**：该端点对个人应用永久 403
 *  （2024-11-27 + 2026-02-06 两批 Dev Mode 收紧），留着就是一个必然失败的入口。 */
const SIDE_CMDS: Record<Exclude<SideKind, "playlist">, string> = {
  album: "spotify_albums",
  artist: "spotify_artists",
  show: "spotify_shows",
  top: "spotify_top_artists",
};

/** 一台可控制的 Connect 设备（`spotify_devices`）。 */
interface DeviceDto {
  id: string;
  name: string;
  kind: string;
  active: boolean;
  volume_percent: number;
}
/** 「Spotify 请求闸」的状态（宿主 `spotify_api_state`，2026-10-02）。
 *
 *  **为什么要有它**：闸是**进程级 + 粘性**的（宿主一旦收到 429 就拉闸、刻意不自动过期，
 *  见 music.rs `SPOTIFY_STOPPED`）。面板是个随时开关的窗口，关掉再打开时模块态是新的
 *  —— 不主动查一次宿主的话，用户会看到一个「拉着的闸 + 没有恢复按钮」的死局。 */
interface SpotifyApiStateDto {
  stopped: boolean;
  reason: string;
  /** 「最早可重试时刻」的 unix 毫秒（`0` = 响应里没有 `Retry-After`）。
   *  面板按它本地算倒计时；**只是显示**，不拦用户手动恢复。 */
  retry_at_ms: number;
}
/** 本机播放（librespot 子进程）的状态。 */
interface LibrespotDto {
  running: boolean;
  /** 找得到可执行文件 —— false 时面板把开关置灰并说清「去哪填路径」。 */
  available: boolean;
  /** 有没有凭据（`librespot-cache\credentials.json`）。**没有它就点不出声**：
   *  没有凭据的 librespot 不是一台已登录的 Connect 设备，设备列表里根本不会有「Lunac」
   *  ⇒ 弹层里那行要换成一次性的「首次登录」，而不是给一个点了没用的开关。 */
  has_credentials: boolean;
  path: string;
}
/** 「播放在哪」的自动就位结果（宿主 `music_autoconfigure`，2026-09-30）。
 *
 *  **为什么要有它**：三条播放入口调 `PUT /me/player/play` 时若不带 `device_id`，
 *  Spotify 就落到「当前活跃设备」；桌面端没开、librespot 没起（或起来了但没被转移过）
 *  时**一台活跃设备都没有** ⇒ 404 `NO_ACTIVE_DEVICE` —— 用户看到的就是「桌面端不在时
 *  选不了歌」。所以面板打开时先由宿主把设备定下来，之后每次点歌都带着它。 */
interface AutoconfigDto {
  /** 五种（与宿主 `music_autoconfigure` 一一对应）：
   *  `official`（桌面端 / 手机 / 音箱等）/ `local`（本机 librespot）/
   *  `needs_login`（引擎在、没凭据）/ `start_failed`（**试着拉 librespot 但没起来**）/
   *  `none`（一台设备都没有）。
   *  `start_failed` 与 `none` **刻意分开**：前者该做的动作是「去设备列表手动开一下」，
   *  后者是「去开 Spotify 桌面端 / 手机端」，两句话不能共用。 */
  source: "official" | "local" | "needs_login" | "start_failed" | "none" | string;
  device_id: string;
  device_name: string;
  device_kind: string;
  desktop_running: boolean;
  /** 这一次调用把 librespot 拉起来了（面板据此提示一句）。 */
  started_local: boolean;
}

// ── 本地媒体库 DTO（宿主 `media_lib.rs`，2026-10-01）──────────────────
// **这一层与 Spotify 无关**：它只读本机磁盘上的音频文件（目录扫描 → 标签 / 时长 /
// 封面 → 落 SQLite）。所以它在「没连 Spotify」时也该能用 —— 见 `localPage` 那条
// 「主体不靠 connected 才显示」的例外。
//
// **本层只是索引**（扫描 / 标签 / 封面 / 落库）——「出声」是下面 `LocalPlayerDto`
// 那一层的事，两者命令前缀也刻意分开（`media_*` / `player_*`）。
interface MediaRootDto {
  /** 规范化后的目录绝对路径（**主键**，加/删/扫描都以它为凭据）。 */
  path: string;
  /** 末段目录名（列表那行显示它，完整路径放 title）。 */
  name: string;
  /** 已入库曲目数。 */
  tracks: number;
  /** 上次扫描完成的毫秒时间戳；0 = 还没扫过。 */
  last_scan_at: number;
  /** 目录现在还在不在（被删 / 拔盘的根要能看出来）。 */
  missing: boolean;
}
interface MediaTrackDto {
  path: string;
  title: string;
  artist: string;
  album: string;
  duration_ms: number;
  /** **已缓存封面的绝对路径**（空串 = 这首没有封面）。要经 `convertFileSrc` 变 URL。 */
  cover: string;
  ext: string;
  size: number;
}
interface MediaTracksDto {
  /** 命中总数（**不受 limit 影响**）—— 被截断时界面要如实说明。 */
  total: number;
  items: MediaTrackDto[];
}
interface ScanStatusDto {
  running: boolean;
  scanned: number;
  /** 候选文件总数（先数一遍再解析，所以进度是真的）。0 = 还在数。 */
  total: number;
  added: number;
  updated: number;
  removed: number;
  unchanged: number;
  failed: number;
  /** 扫描失败的原因（整轮失败才有；单个文件解析不了只进 `failed` 计数）。 */
  error: string;
  /** 本轮结束的毫秒时间戳；0 = 还没结束过。 */
  done_at: number;
}

// ── 本地**播放** DTO（宿主 `player.rs`，2026-10-01）────────────────────
// 与上面那层（索引）刻意分开：那边是「能看」（SQLite 里的元数据），这边是
// 「能听」（宿主里那条 rodio 音频流）。**出声只能在宿主** —— WebView 放不出声音，
// 与「联网必须走宿主」是同一条纪律（ai-spec §4.6）。
//
// ⚠️ **队列不在宿主**：宿主只认「当前这一首」，下一首是谁要有列表的上下文，
// 而列表的上下文只在这一层（见 player.rs 头注释）。
interface LocalPlayerDto {
  /** 有内容在播。**`false` 的三种含义要靠 `path` 区分**：
   *  没起过 / 已停（`path` 空）与**这一首放完了**（`path` 有值）——
   *  后者正是「自动下一首」的判据。 */
  active: boolean;
  playing: boolean;
  /** 当前（或最后）那一首的绝对路径；空串 = 这台机器上没有本地会话。 */
  path: string;
  position_ms: number;
  /** 解码器报的总时长。**0 = 报不出来**（没有 Xing 头的 mp3 就是这样）——
   *  这时不画进度、也不允许拖动（画一条假的进度比没有更糟）。 */
  duration_ms: number;
  volume: number;
  /** **这是一条要出画面的视频**（2026-10-01 用户第 8 条）。
   *  判据在宿主（`player.rs` → `media_lib::VIDEO_EXTS`），前端只读这个布尔 ——
   *  **别在这里再抄一份扩展名表**（那就是「加了一个格式、只有一侧认」）。 */
  video: boolean;
}

/** 一段可编辑的滤波器（宿主 `player.rs` 的 `TuningFilter`，P0 2026-10-02）。
 *
 *  **字段名必须逐字对齐 Rust 的 serde 形态**（`freq_hz` / `gain_db`，不是驼峰）：
 *  Tauri 只把**命令参数**做蛇形→驼峰转换，结构体内部仍由 serde 按原字段名反序列化
 *  —— 写成 `freqHz` 会被 `deny_unknown_fields` 拒掉，错误还很难看懂。
 *
 *  `on=false` 的段**保留在表里但不参与建链**（关掉是「暂时听听看」，不是删掉）；
 *  它只活在面板这一层，下发前由宿主摘掉（引擎不认识 `on`）。
 *  `gain_db` / `q` 是 `undefined` 就表示「这一档不要这个字段」—— 不许替引擎补零。 */
interface TuningFilter {
  kind: string;
  freq_hz: number;
  gain_db?: number;
  q?: number;
  on: boolean;
}

/** 一个滤波器类型的对外形态（宿主 `TuningKind`，与引擎 `Kind::ALL` 同源）。
 *  `gain` = 要不要显示增益框；`order` = 2 才显示 Q 框（一阶没有 Q，写了引擎会拒）。 */
interface TuningKind {
  name: string;
  gain: boolean;
  order: number;
}

/** 频响曲线（**宿主算、前端只画**）：`freqs` / `db` 一一对应，`sample_rate` 是
 *  算这条曲线用的采样率（= 最近一次播放那条流的）。前端**不许**自己算曲线 ——
 *  那等于把 RBJ 系数再实现一遍，两份必然漂移。 */
interface TuningResponse {
  sample_rate: number;
  freqs: number[];
  db: number[];
}

/** 调音状态（宿主 `player.rs` 的 `TuningDto`）。
 *
 *  **P0（2026-10-02）起允许下发整条滤波器数组**：一条链的合法性一律由引擎那份
 *  `ChainConfig::from_json` **逐条校验兜底**（未知字段 / 越界 / 「该有增益的必须有」），
 *  前端拼不出一条「界面上亮着、实际直通」的配置 —— 校验不过宿主就不落盘、界面回滚。
 *  `kinds` / `user_presets` 是**唯一名单**（与引擎 `Kind::ALL` / 宿主预设目录同源）
 *  —— 前端照它们画按钮与下拉，不自己维护第二份。`presets`（内置 5 档的键）仍由宿主下发，
 *  但 2026-10-03 起**面板不再画它们**（用户要求整批删掉）。
 *  文案不在 DTO 里：`music.tk_<类型>` 由 i18n 出（五语言），用户预设用宿主给的名字。 */
interface TuningDto {
  enabled: boolean;
  preset: string;
  presets: string[];
  /** **用户预设的显示名**（宿主 `config\tuning\presets\` 里那份，P5-1）。
   *  与内置 `presets` 分开两份：内置的文案走 i18n 键，用户预设用自己的名字。 */
  user_presets: string[];
  kinds: TuningKind[];
  preamp_db: number;
  /** **低音 / 高音快捷增益**（dB，2026-10-03）：中栏那两条水平滑块。
   *  引擎不认识它们 —— 宿主 `effective_json` 把非零值追加成 low_shelf / high_shelf 两条。 */
  bass_db: number;
  treble_db: number;
  filters: TuningFilter[];
  /** **图示均衡器**（P5-3）：一组「中心频率 + 增益」，引擎在解析期把它**展开成 peaking 段**
   *  追加到 `filters` 之后（见 `tuning-engine/src/config.rs` 的 `expand_graphic_eq`）。
   *  ⇒ 它是**顶层（全局）**字段，不随通道槽变；频点必须**严格升序**（Q 由相邻间距折算）。 */
  graphic_eq: TuningBand[];
  /** **全局延迟**（毫秒，P5-4）：所有声道同量，`0..=5000`（引擎 `MAX_DELAY_MS`）。
   *  超范围会让**整条链**编不出来 ⇒ 面板先夹住（见 `TUNE_DELAY_MAX_MS`）。 */
  delay_ms: number;
  /** **声道复制**（P5-4）：`[{from,to}]`，处理后把 `from` 的快照抄进 `to`。 */
  channel_copy: TuningCopy[];
  /** **卷积**用的 IR（脉冲响应）WAV 路径（P5-4）。`null` = 这条链不做卷积。
   *  相对路径由宿主锚到 **Lunac 根目录**（面板给的是文件对话框返回的绝对路径）。
   *  ⚠️ IR 必须是**单声道 + 采样率与这条流一致**，否则编链失败 —— 面板把错原样报出来。 */
  convolution: string | null;
  /** **通道段**（P5-2 声道槽）：全局槽在 `filters`，通道 k 在这里第 k 条段的 `then`。
   *  与 `filters` 同一种 `TuningFilter`（带 `on`），宿主下发前会摘掉。 */
  if_else: TuningCondBlock[];
  /** 最近一次建链那条流的声道数（`0` = 还不知道 ⇒ 面板按 2 兜底）。 */
  channels: number;
  /** **按声道的曲线**：与 `if_else` **同序**一一对应，第 i 条是 `if_else[i].channel`
   *  那条声道真正会发出的响应（全局链 + 它命中的分支）。由宿主算（曲线的权威在引擎）。 */
  cond_db: number[][];
  response: TuningResponse;
}

/** 一条**通道段**（宿主 `TuningCondBlock`，P5-2）。 */
interface TuningCondBlock {
  channel: number;
  then: TuningFilter[];
  else?: TuningFilter[];
}

/** 通道槽的显示名（Peace / 7.1 那套顺序）。纯标识，不入 i18n。 */
const CHANNEL_NAMES = ["L", "R", "C", "LFE", "SL", "SR", "BL", "BR"];

/** **图示均衡器的一段**（引擎 `RawGraphicBand`）：只有中心频率与增益 —— 没有 `on`、没有 Q。
 *  Q 由引擎按**相邻频点的对数间距**折算（`expand_graphic_eq`），所以这里**不许**碰它；
 *  频点也必须**严格升序**，面板只按这个顺序原样下发。 */
interface TuningBand {
  freq_hz: number;
  gain_db: number;
}

/** **一条声道复制**（宿主 `TuningCopy` / 引擎 `channel_copy` 的一条，P5-4）：处理后把
 *  `from` 的快照抄进 `to`。两边下标都必须在**当前流的声道数**以内 —— 越界在引擎里是
 *  `ChainRuntime::new` 的硬错误（**整条链**都编不出来），所以面板只让用户在流内选。 */
interface TuningCopy {
  from: number;
  to: number;
}

/** 全局延迟的上限（毫秒）—— **必须与引擎的 `MAX_DELAY_MS` 一致**。
 *  面板这一层先夹住，是为了不让一次手滑（比如敲成 6000）把**整条链**顶掉：
 *  引擎对超范围只回一句错误、链本身落不下去，听感上就是「调音突然全没了」。 */
const TUNE_DELAY_MAX_MS = 5000;

/** **实时测量**的结果（宿主 `tuning_probe.rs` 的 `MeasuredResponse`）。
 *
 *  2026-10-03 起测**两趟**（一趟直通、一趟走链），于是有三条线：
 *   · `db`      = **最终输出**（走链那趟的 Y/X）= 链 × 系统（EAPO / 别的软件都在里面）；
 *   · `chain_db`= **仅自身链**（= 最终输出 ÷ 系统）= 真正该跟 `ref_db` 对齐的那条；
 *   · `ref_db`  = 同一批频点上**宿主解析式算的链合成**（理论）。
 *  画三条：「仅自身链」贴住「合成」= 调音确实按你说的生效了；「最终输出」离得远说明
 *  还有别的软件在掺和（那不是本插件的问题）。
 *
 *  `freqs` 的长度**可能少于面板那 240 点**：扫频两端与能量过低的点会被摘掉，
 *  所以画的时候要按拿到的点直接连折线，**不要**按面板那套索引去对位。 */
interface MeasuredResponse {
  sample_rate: number;
  freqs: number[];
  db: number[];
  chain_db: number[];
  ref_db: number[];
  coverage: number;
  elapsed_ms: number;
}

// ── 定尺常量 ──────────────────────────────────────────────────────
/** **默认面板**的悬浮窗尺寸（`1280×720`，用户 2026-09-28 定）。
 *
 *  与宿主 `plugin_window.rs` 的 `MUSIC_W/MUSIC_H` **必须一致**（那边只是建窗初值，
 *  真正下发的是这里）—— 面板是「左栏 + 中间曲目表」的两栏布局，1280 才放得下。
 *  **这个态不追求内容严丝合缝**：两侧各自内部滚动，窗口是多大就是多大
 *  （所以不再有 `min(MUSIC_H, 实测)` 那套贴合逻辑）。
 *
 *  ⚠️ 播放态**不用**这两个值：它是定尺长条（见 `MUSIC_W_BAR`），
 *  宽度与这里完全不同 —— 别把某一个当成「全局窗口宽」。 */
const MUSIC_W = 1280;
const MUSIC_H = 720;
/** **播放态**的悬浮窗宽度（2026-09-28）：播放态的外层垫料被 CSS 清零
 *  （见 `styles.css` 里 `body:has(.music-root.music-player-on)` 那一组），
 *  所以窗口**就是长条本身** = 550，不再需要那 12px。
 *  用户报的「562×200 的渲染窗口」就是这个差 + 宿主旧的 `MIN_H = 200`。
 *  ⚠️ 改这里必须同时改那条 CSS（它把 `.plugin-result` 的 padding 收成 0）。 */
const MUSIC_W_BAR = 550;
/** 左栏宽度：默认值 + 拖动范围（用户 2026-09-28 要求「歌单列作为单独的一列可拉动」）。
 *  **上下限是为了不让某一栏被拖没** —— 拖到 0/全宽之后用户找不回分隔条。 */
const SIDE_W_DEFAULT = 300;
const SIDE_W_MIN = 180;
const SIDE_W_MAX = 520;
/** 用户手动滚歌词后，自动跟随挂起多久（毫秒）再重新对齐当前行。 */
const FOLLOW_RESUME_MS = 4000;
/** 搜索框的输入防抖：每敲一个字都打一次 Spotify 搜索太浪费（也容易撞限流）。 */
const SEARCH_DEBOUNCE_MS = 450;
/** 歌词滚动一轮的时长。**用户 2026-09-27 明确说「动画时间太短、阻尼度调高」** ——
 *  原先用 CSS `scroll-behavior: smooth`（Chromium 约 300–700ms 且时长不可控），
 *  现在改成 rAF 自己算，配合 `easeOutQuint`：起步快、收尾极缓，读起来像被阻尼压住。
 *  **因此 `.music-lyr` 上不能再留 `scroll-behavior: smooth`** —— CSS 平滑会接管
 *  每一帧 `scrollTop` 的赋值，和 rAF 缓动叠加成二次动画（走起来一顿一顿的）。 */
const LYRIC_SCROLL_MS = 800;
/** 一次性提示（`#music-msg`）挂多久后自动抹掉。
 *  **它不是状态，是「刚才那次操作的结果」** —— 不抹就会出现「正在播放」与
 *  「没有可控制的播放设备」同屏挂着（2026-09-27 用户报的正是这个）。 */
const MSG_TTL_MS = 8000;
/** 进度条本地插值的周期。**必须明显短于 `.music-progress-fill` 的 `0.3s` 补间**，
 *  补间才能首尾相接、看起来连续（见 `paintProgressSmooth`）。 */
const PROGRESS_TICK_MS = 200;
/** **Spotify 状态轮询的周期**（2026-10-02 从 1s 放宽到 3s；2026-10-03 再改成 15s）。
 *
 *  它每跑一次就是一次 `GET /me/player` —— 而 Spotify 对**这一族端点**的配额远低于普通
 *  API（官方口径：部分端点有各自的限额），**罚起来还是小时级的**：2026-10-03 实测同一个
 *  token 打 `/me/player*` 拿到 `Retry-After: 45000+`（≈12.5 小时），且**换 token 也不重置**
 *  （按 app 计的端点级处罚）。所以口径从「高频轮询保实时」改成「**长间隔 + 交互驱动**」：
 *
 *    · **15s 只是兜底**：别的客户端（手机 / 桌面端）改了播放态，本面板最迟 15s 跟上；
 *    · **一有交互就补一轮**（`pokePoll`）：点控制键 / 切页签 / 拉进度 / 敲搜索都会立刻
 *      提前下一拍 —— 「正在操作时它是实时的」这条观感由它保住，而不是靠高频轮询；
 *    · **面板隐藏即停表**（`visibilitychange`，见 `ensureGlobalPollListeners`）：隐藏期间
 *      一次都不打。
 *
 *  进度条位置的实时性**不受影响**：它走本地插值（`paintProgressSmooth`，200ms 一拍、零网络）。 */
const STATUS_POLL_MS = 15000;

// ── 本地媒体库（2026-10-01）───────────────────────────────────────
/** 一次取多少条本地曲目。宿主侧硬上限 `MAX_QUERY_LIMIT = 2000`；这里取 500 是因为
 *  这本来就是一个「浏览」视图（滚动 + 筛选），不是要把整库拉到内存里。
 *  取满了就在列表下方如实写「显示前 N / 共 M」。 */
const MEDIA_LIMIT = 500;
/** 扫描进度的轮询周期。扫描在宿主后台线程里跑，这里只读一个内存快照 —— 500ms
 *  足够让进度条看起来是连续的，又不至于一秒两次 IPC。 */
const MEDIA_POLL_MS = 500;
/** 本地筛选框的输入防抖。与搜索框同理：每敲一个字都跨进程查一次库太浪费。 */
const MEDIA_QUERY_DEBOUNCE_MS = 250;
/** 本地**播放**的轮询周期。比扫描那条快一倍：进度条要跟得上耳朵，而它读的只是
 *  宿主内存里的一个状态快照（不打网络、不碰磁盘），一秒两次 IPC 是划算的。 */
const LOCAL_POLL_MS = 500;
/** 视频画面的**纠偏阈值**（秒），2026-10-01 用户第 8 条。
 *
 *  画面与声音是**两个解码器**（画面 = 插件窗的 video 元素，声音 = 宿主的 rodio），
 *  两个时钟必然慢慢分开 —— 低于这个阈值就不动它：每拍都硬对齐会让画面每秒抖一下，
 *  而人对声画不同步的容忍度大约就在这个量级。超过阈值才纠一次（纠的是画面，
 *  声音那条流**一下都不碰**）。 */
const VIDEO_DRIFT_S = 0.3;

// ── 线性 SVG（docs/icon-style.md §1：stroke currentColor，不嵌 emoji）──
const SVG = {
  play: `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polygon points="6 4 20 12 6 20 6 4"/></svg>`,
  pause: `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><line x1="9" y1="4" x2="9" y2="20"/><line x1="15" y1="4" x2="15" y2="20"/></svg>`,
  prev: `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polygon points="19 5 9 12 19 19 19 5"/><line x1="5" y1="5" x2="5" y2="19"/></svg>`,
  next: `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polygon points="5 5 15 12 5 19 5 5"/><line x1="19" y1="5" x2="19" y2="19"/></svg>`,
  /** 随机（关 / 随机共用这一个；「关」靠 `data-mode` 变暗区分）。 */
  shuffle: `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="16 3 21 3 21 8"/><line x1="4" y1="20" x2="21" y2="3"/><polyline points="21 16 21 21 16 21"/><line x1="15" y1="15" x2="21" y2="21"/><line x1="4" y1="4" x2="9" y2="9"/></svg>`,
  /** 循环（单曲循环；那个「1」由 CSS `::after` 加，SVG 里不写字）。 */
  repeat: `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="17 1 21 5 17 9"/><path d="M3 11V9a4 4 0 0 1 4-4h14"/><polyline points="7 23 3 19 7 15"/><path d="M21 13v2a4 4 0 0 1-4 4H3"/></svg>`,
  list: `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><line x1="9" y1="6" x2="21" y2="6"/><line x1="9" y1="12" x2="21" y2="12"/><line x1="9" y1="18" x2="21" y2="18"/><circle cx="4" cy="6" r="1" fill="currentColor" stroke="none"/><circle cx="4" cy="12" r="1" fill="currentColor" stroke="none"/><circle cx="4" cy="18" r="1" fill="currentColor" stroke="none"/></svg>`,
  /** 进播放界面。 */
  expand: `<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="15 3 21 3 21 9"/><polyline points="9 21 3 21 3 15"/><line x1="21" y1="3" x2="14" y2="10"/><line x1="3" y1="21" x2="10" y2="14"/></svg>`,
  /** 回默认界面。**唯一**的退出播放界面的入口 —— 播放停止不会自动退出。 */
  back: `<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="15 18 9 12 15 6"/><line x1="3" y1="12" x2="9" y2="12"/></svg>`,
  chevron: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>`,
  copy: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15V5a2 2 0 0 1 2-2h10"/></svg>`,
  link: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M10 13a5 5 0 0 0 7.5.5l3-3a5 5 0 0 0-7-7l-1.5 1.5"/><path d="M14 11a5 5 0 0 0-7.5-.5l-3 3a5 5 0 0 0 7 7L12 19"/></svg>`,
  power: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M18.4 6.6a9 9 0 1 1-12.8 0"/><line x1="12" y1="2" x2="12" y2="12"/></svg>`,
  gear: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>`,
  volume: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"/><path d="M15.5 8.5a5 5 0 0 1 0 7"/></svg>`,
  /** 工具条上的「设备」按钮（2026-09-28）。画成一台小音箱，比「显示器」更贴
   *  Spotify 那种「可选的播放设备」语义。 */
  speaker: `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><rect x="5" y="2" width="14" height="20" rx="2"/><circle cx="12" cy="14" r="3.2"/><circle cx="12" cy="6" r="1" fill="currentColor" stroke="none"/></svg>`,
  /** 实心心形（「我喜欢的歌曲」那块封面上的图案）。**这个是 `fill`，不是描边** ——
   *  它是一块封面图（Spotify 的收藏夹封面就是实心心 + 渐变底），不是可点按钮。 */
  heart: `<svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor" stroke="none"><path d="M19 14c1.49-1.46 3-3.21 3-5.5A5.5 5.5 0 0 0 16.5 3c-1.76 0-3 .5-4.5 2-1.5-1.5-2.74-2-4.5-2A5.5 5.5 0 0 0 2 8.5c0 2.3 1.5 4.05 3 5.5l7 7Z"/></svg>`,
  // ── 本地媒体库那一页用到的四个（2026-10-01）──
  /** 「本地音乐」入口 + 目录行（画成文件夹）。 */
  folder: `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>`,
  /** 「添加目录」。 */
  plus: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>`,
  /** 移除一根目录（行尾那枚）。`music.pl-x` 用。 */
  x: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><line x1="6" y1="6" x2="18" y2="18"/><line x1="18" y1="6" x2="6" y2="18"/></svg>`,
  /** 没有封面的曲目行左侧占位（一个八分音符）。 */
  note: `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 18V6l10-2v12"/><circle cx="6.5" cy="18" r="2.5"/><circle cx="16.5" cy="16" r="2.5"/></svg>`,
  /** 「重新扫描」/「刷新」—— 画成环形箭头。 */
  refresh: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="21 3 21 9 15 9"/><path d="M20.4 14a8.5 8.5 0 1 1-1.6-7.5L21 9"/></svg>`,
  /** 「测量」—— 一条起伏的响应曲线（Lucide 的 `activity` 那一族的形状）。 */
  wave: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 12h3l2.5-6 3.5 12 3-8 2 2h4"/></svg>`,
  /** 编辑（改列表）。**路径逐字抄自 icon-style.md §3 的「编辑 ✎」** —— 规范里已经
   *  给好这条了，不许自己另画一条（§4.2 最后一条：已有的图形不要重复造路径）。 */
  edit: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M17 3a2.8 2.8 0 0 1 4 4L7.5 20.5 2 22l1.5-5.5Z"/></svg>`,
  /** 删除（垃圾桶）。**同样逐字抄自 icon-style.md §3** —— 它是危险操作，
   *  按 §4.2 用同风格线性图标 + 语义色（hover 变 `--red`，见 styles.css）。 */
  trash: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/><path d="M10 11v6M14 11v6M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>`,
  /** 停止本地播放（本地播放条右端那枚）。**它与暂停不是一回事**：暂停留着这台
   *  音频设备，停止会把设备一起放掉（见 player.rs 的 `player_stop`）。 */
  stop: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><rect x="6" y="6" width="12" height="12" rx="1.5"/></svg>`,
  /** 「本地与 Spotify 同时出声」那把开关（2026-10-01）。画成**两个相交的圆** ——
   *  「两路并存」最直白的图形（Lucide 里叫 `blend`）。**不画成两个喇叭**：12px 下
   *  两个喇叭会糊成一团，而两个圆在任何尺寸下都认得出来。
   *  开关的「开着」不用第二个图形表达，靠 `.on` 的强调色（与其他开关一致）。 */
  dual: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><circle cx="9" cy="9" r="6.5"/><circle cx="15" cy="15" r="6.5"/></svg>`,
  /** 「最常听的歌曲」那一行的图标（2026-10-01 用户第 4 条）。画成一条上扬的折线
   *  —— 「最常听 / 热度」最直白的图形（Lucide 的 `trending-up`，与上面这些同源）。
   *  与 `SVG.heart` 一样是 20px 的封面图案，但**保持描边风格**（它不是实心封面，
   *  它是「一行合成出来的入口」，与收藏夹那块渐变实心封面刻意区分开）。 */
  trend: `<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="22 7 13.5 15.5 8.5 10.5 2 17"/><polyline points="16 7 22 7 22 13"/></svg>`,
  // （工具条上那枚「调音推子」图标 2026-10-01 随入口一起删掉了：调音改成标题栏
  //  三段切换的第三段，那枚图标就没有地方落了。图形本身仍在 icon-style.md §3 里。）
};

// ── 模块级状态 ────────────────────────────────────────────────────
let cfg: MusicConfigDto | null = null;
let player: PlayerDto | null = null;
let lyrics: LyricsDto | null = null;
/** 已经抓过歌词的曲目 id，避免每分钟重复打歌词源。 */
let lyricsFor = "";
let lyricsLoading = false;
let lrcLines: LrcLine[] = [];
let activeLine = -1;
/** 用户对「凭据字段展开/收起」的手动选择；`null` = 跟随「有没有 Client ID」自动决定。 */
let fieldsOverride: boolean | null = null;
/** 用户刚点过、**还没保存**的串流质量档（kbps）。`null` = 显示配置里那一档。
 *  必须有这个中转：`renderSetup` 会被每秒那轮轮询反复调用，没有它的话用户点的
 *  那一档下一拍就被 `cfg` 里的旧值盖回去（输入框靠 `document.activeElement` 挡住，
 *  按钮没有这个保护）。保存成功即清空。 */
let bitrateOverride: number | null = null;

/** 已连接时，连接卡片（`#music-setup`）由**头像点击**开合（2026-10-01）。
 *  必须有这个状态：`renderSetup` 每轮轮询都会跑，它按「连没连」重设那张卡片的显隐 ——
 *  直接改 class 的话，用户点开头像的那张卡片会在**下一拍**被重新合上（看起来像点了没反应）。 */
let setupOpen = false;

/** 调音（2026-10-01 用户第 5 条）。`null` = 还没问过宿主 —— 这时设置页那一块
 *  用**不亮任何一档**的方式画（别默认点亮 `bass`：那是编出来的状态）。
 *  **没有「待保存值」这一层**：调音是**开关语义**（按下就生效、就直接落盘），
 *  与上面那个 `bitrateOverride` 刻意不同 —— 音质要重启 librespot 才生效，
 *  所以它必须攒着等「保存」；调音是实时的，攒着反而会让用户以为「点了没反应」。 */
let tuning: TuningDto | null = null;

/** 调音表的**可编辑镜像**（P0 2026-10-02）。为什么不能直接改 `tuning.filters`：
 *  那是一个「宿主的真值」快照，改它会让「失败时回滚到宿主真值」这条兜底失效
 *  （回滚时已经无从知道原来是什么）。所以改动一律进这份草稿，提交成功后
 *  **用宿主返回的 DTO 重铺草稿**（`syncTuningDraft`）—— 界面亮的永远是宿主的真值。
 *  `tuneOpen` = 哪一行的「类型」下拉是摊开的（-1 = 都收起）；它是**纯视图状态**，
 *  不进配置、不落盘。 */
let tunePreamp = 0;
/** **低音 / 高音快捷增益**（dB，2026-10-03 用户要求 Peace 那三条水平滑块的后两条）。
 *  它们**不是引擎字段**：宿主 `effective_json` 会追加成 low_shelf / high_shelf 两条
 *  （见 player.rs 的 `BASS_SHELF_HZ`）—— 所以它们不在这张频段柱里，曲线却算得进去。 */
let tuneBass = 0;
let tuneTreble = 0;
/** **当前通道槽**的草稿 —— 它就是下面 `tuneSlotDrafts[tuneSlot]` 那个数组**本身**
 *  （同一个引用），所以表里所有 `tuneFilters.push/splice/改字段` 都直接落在对应的槽上。 */
let tuneFilters: TuningFilter[] = [];
/** **通道槽草稿**（P5-2）：`[0]` = 全局槽（↔ 引擎的 `filters`），`[k]` = 通道 `k-1`
 *  （↔ `if_else[k-1].then`）。提交时**整份一起发** —— 所以切槽只是换「表在编辑哪一份」，
 *  不需要为了切槽而先提交（见 `selectTuneSlot`）。 */
let tuneSlotDrafts: TuningFilter[][] = [[]];
/** 当前编辑的槽：`0` = 全局，`k` = 通道 `k-1`。 */
let tuneSlot = 0;
/** **图示均衡器的草稿**（P5-3）：引擎的 `graphic_eq` 是**顶层（全局）**字段，所以这一份
 *  不随通道槽变。空数组 = 这条链没有 GraphicEQ。
 *  ⚠️ 2026-10-03 用户口径「滤波器与均衡器合并」⇒ **这排频段柱改编辑链滤波器了**（见
 *  `tuningBandHtml`），图示均衡器不再有独立界面。`tuneGeq` 保留为**纯透传**：读回来
 *  原样发回去，不静默清掉用户盘上已配好的 graphic_eq（那条纪律见 `commitTuningChain`）。 */
let tuneGeq: TuningBand[] = [];
/** **全局延迟的草稿**（毫秒，P5-4）。同样是顶层字段 ⇒ 不随通道槽变。 */
let tuneDelay = 0;
/** **声道复制的草稿**（P5-4）。顶层字段。 */
let tuneCopy: TuningCopy[] = [];
/** **卷积 IR 的路径草稿**（P5-4）。`null` = 不做卷积；顶层字段。 */
let tuneConv: string | null = null;
/** 声道复制那一行摊开着哪个「声道选择菜单」（P5-4）：`null` = 都收起。
 *  与 `tuneOpen` 一样是**纯视图状态**，不进配置、不落盘。 */
let tuneCopyMenu: { row: number; side: "from" | "to" } | null = null;
let tuneOpen = -1;
/** 提交序号（P0）。**防的是「后发先至」**：用户快速点几下（连着关几段 / 连点删除）时
 *  会同时在飞两三个 `player_tuning_set_chain`，而 IPC 的回包顺序不保证 —— 先发的那个
 *  后回来就会把 `tuning` 覆盖成旧状态（界面「闪回去一下」）。带上序号后，**只有最新的
 *  那一次提交的结果才写回**；过时的回包直接丢掉（服务端的真值仍是最新的那次）。 */
let tuneSeq = 0;

/** **最近一次实时测量**的结果（`null` = 还没测过 / 换了链之后已作废）。
 *  刻意只在内存里：它描述的是「某一次、某台设备、某个音量下的现状」，
 *  落盘会在下次启动时变成一条**看起来像真值**的陈旧曲线。 */
let tuneMeasured: MeasuredResponse | null = null;
/** 正在测量（按钮置灰、避免连点两次同时开两条回环流）。 */
let tuneProbing = false;

/** 预设**命名行**的状态（P5-1）。
 *
 *  WebView2 **不支持 `window.prompt`**（settings.ts 里那条注释踩过同一个坑），所以「保存为
 *  预设 / 重命名 / 删除确认」全走面板里这一行内联输入 —— 三种模式共用同一行，`null` = 收起。
 *  `overwrite` 只对 `save` 有意义：宿主回 `ERR_PRESET_EXISTS` 时把它翻成 `true`，
 *  再点一次就是确认覆盖。 */
type TunePresetEdit =
  | { mode: "save"; name: string; overwrite: boolean }
  | { mode: "rename"; from: string; name: string }
  | { mode: "delete"; name: string };
let tunePresetEdit: TunePresetEdit | null = null;

/** 曲线画布最近一次绘制用的**坐标映射**（拖动时把像素反算回频率 / 增益要用它）。
 *  由 `drawTuningCurve` 每画一次就更新 —— 两边共用同一份，才不会出现
 *  「画的时候按 A 布局、点的时候按 B 布局」的错位。 */
interface TuneGeom {
  w: number;
  h: number;
  padL: number;
  padT: number;
  plotW: number;
  plotH: number;
  span: number;
}
let tuneGeom: TuneGeom | null = null;

/** 正在拖的那一段（索引；-1 = 没在拖）。**存索引而不是对象引用** ——
 *  每次提交成功都会用宿主返回的 DTO 重铺草稿（`syncTuningDraft` 整批换对象），
 *  抓着旧对象会在下一拍操作到一个已经不在表里的段。 */
let tuneDrag = -1;
/** 这次拖动**真的改动过值**吗。只是「点一下手柄」不该白发一次提交
 *  （那会把 `preset` 从某一档改成 `custom`，而用户其实什么都没改）。 */
let tuneDragMoved = false;
/** 「一次拖动正在进行」：这期间**不许重建滤波器表**。
 *  重画表格会把 `<input type="range">` 整个换掉，正在拖的那个元素一被换掉，
 *  浏览器就收不到后续的 pointermove —— 表现是「滑块拖两下就断」。曲线照画，
 *  它没有这个问题（canvas 是同一个节点）。 */
let tuneHold = false;
/** 拖动中的提交节流（毫秒级）：每次 pointermove 都发一次 IPC 会把宿主刷屏，
 *  而曲线本来就要靠宿主算 —— 折中成「最多每 120ms 提交一次，松手时再补一次」。 */
let tuneCommitAt = 0;
let tuneCommitTimer: number | null = null;
/** 数字框 `change` 提交的**去抖窗口**（2026-10-05 用户要求「修掉误存」）。
 *  一次用户动作常会连触发多个 `change`（输完点别处 ⇒ 那个框 blur 提交，同时被点中的
 *  控件自己也提交），合并成一次 IPC / 一次落盘，避免「一个动作存两回」。 */
const TUNE_CHANGE_DEBOUNCE_MS = 300;
let tuneChangeTimer: number | null = null;

// ── A-B 盲测（2026-10-06）──────────────────────────────────────────
//
// 两个**快照槽**（A / B，宿主侧 `config\tuning\ab-*.json`）各存一份整条链的快照；
// 盲测的「随机映射 + 揭晓」**只在前端**（宿主只认槽名，不给它看谁是谁）。
// `tuneAbSlots` 是「左边那个按钮 / 右边那个按钮分别对应哪个槽」—— 盲测时把它换序，
// 于是界面上只出现 1 / 2，用户听不出哪个是 A。
let tuneAbHas: { a: boolean; b: boolean } = { a: false, b: false };
let tuneAbSlots: ("a" | "b")[] = ["a", "b"];
let tuneAbBlind = false;
let tuneAbRevealed = false;
let tuneAbActive: "a" | "b" | null = null;

/** 一份全新的默认段（「加一段」与「换到某个类型」时补齐缺省字段都用它）。
 *  `peaking` 是增益类里最常用的一种，1kHz/0dB/Q=0.707 是「听不出变化」的安全起点
 *  （用户改哪个框就是明确要动哪一项，不必先猜一个「有味道」的值）。 */
function newTuningFilter(): TuningFilter {
  return { kind: "peaking", freq_hz: 1000, gain_db: 0, q: 0.707, on: true };
}

/** 把宿主的真值铺进草稿。**每次成功提交/拉取之后都要调**（见上面那段注释）。
 *
 *  P5-2 起铺的是**一整排通道槽**（不只是全局那一份）：`[0]` = 宿主 `filters`，
 *  `[k]` = 宿主 `if_else` 里 `channel = k-1` 那条的 `then`。槽数 = `1 + 声道数`
 *  （声道数 `0` = 宿主还不知道 ⇒ 按 2 兜底，与宿主那边同一个口径）。 */
function syncTuningDraft() {
  tunePreamp = tuning?.preamp_db ?? 0;
  // 低音 / 高音快捷增益（2026-10-03）：宿主那两个字段直读（引擎不认识它们，
  // 由宿主 effective_json 追加成 shelf 两条）。
  tuneBass = tuning?.bass_db ?? 0;
  tuneTreble = tuning?.treble_db ?? 0;
  // 图示均衡器（P5-3）：**只留引擎认识的两个字段**（`deny_unknown_fields` 会拒多余的键）。
  // 频点顺序原样保留 —— 引擎要求严格升序，面板不许替用户重排。
  tuneGeq = (tuning?.graphic_eq ?? []).map((b) => ({ freq_hz: b.freq_hz, gain_db: b.gain_db }));
  // 延迟 / 声道复制 / 卷积（P5-4）：同样是顶层字段，整份重铺。
  tuneDelay = tuning?.delay_ms ?? 0;
  tuneCopy = (tuning?.channel_copy ?? []).map((c) => ({ from: c.from, to: c.to }));
  tuneConv = tuning?.convolution ?? null;
  // 行号会随重铺变化 ⇒ 收起那个声道选择菜单（不收的话它会对到别的行上）。
  tuneCopyMenu = null;
  // 声道数从宿主那份拿；它变了（换了素材）⇒ 槽数跟着变，下面顺手把 `tuneSlot` 夹回来。
  const ch = tuning?.channels && tuning.channels > 0 ? tuning.channels : 2;
  const slots: TuningFilter[][] = [(tuning?.filters ?? []).map((f) => ({ ...f }))];
  for (let k = 1; k <= ch; k++) {
    const block = (tuning?.if_else ?? []).find((b) => b.channel === k - 1);
    slots.push((block?.then ?? []).map((f) => ({ ...f })));
  }
  tuneSlotDrafts = slots;
  // 换了一条声道数更少的流 ⇒ 原来选的那一槽可能已经不存在了，夹回全局槽（那是唯一恒存在的）。
  if (tuneSlot < 0 || tuneSlot >= slots.length) tuneSlot = 0;
  // **当前槽的草稿就是这一排里的那一个数组本身**（不是副本）—— 表里的增删改直接落在它上面。
  tuneFilters = slots[tuneSlot];
}

/** 当前槽该画哪条曲线（P5-2）。`0` = 全局 ⇒ 宿主那条全局响应；通道槽 ⇒ 宿主按声道算的
 *  那条（全局 + 本声道命中的分支）。通道还没配任何段时宿主不会为它算曲线（`if_else`
 *  里压根没有这一条）⇒ 退回全局那条 —— 用户一加段提交，它就会自己长出来。 */
function tuneSlotCurve(): number[] {
  if (!tuning) return [];
  if (tuneSlot === 0) return tuning.response.db;
  const i = (tuning.if_else ?? []).findIndex((b) => b.channel === tuneSlot - 1);
  const row = i >= 0 ? tuning.cond_db?.[i] : undefined;
  return row && row.length ? row : tuning.response.db;
}

/** 把**全部通道槽草稿**拼成宿主的 `if_else`（P5-2）。全局槽不在这里 —— 它走 `filters`。
 *
 *  `else` 分支面板没有界面 ⇒ **原样保留**宿主那一份（与另外四项效果器同一条透传口径）：
 *  不回传就等于替用户把这半边删了。两边都空的块不发（宿主 `effective_json` 也会丢掉它）。 */
function tuneIfElsePayload(): TuningCondBlock[] {
  const prev = new Map((tuning?.if_else ?? []).map((b) => [b.channel, b]));
  const out: TuningCondBlock[] = [];
  for (let k = 1; k < tuneSlotDrafts.length; k++) {
    const channel = k - 1;
    const then = tuneSlotDrafts[k];
    const els = prev.get(channel)?.else ?? [];
    if (then.length === 0 && els.length === 0) continue;
    const blk: TuningCondBlock = { channel, then };
    if (els.length > 0) blk.else = els;
    out.push(blk);
  }
  return out;
}

/** 界面形态：`default` = 连接设置 + 两栏 + 底部控制栏；`player` = 封面 / 歌词 / 控制条。
 *  **只由用户手动切**（用户 2026-09-28 定）：播放不再自动进播放面板、暂停也不再自动
 *  退回歌单列 —— 进出都是「点一下」这一个动作（进：底部栏那个展开按钮或正在播放那块；
 *  出：长条上的返回，或标题栏那个 ← ）。 */
let mode: MusicMode = "default";

// ── 左栏（Library）与主区（2026-09-28 改成两栏）──────────────────
// 旧版是「一个歌单列，点一下在行内展开曲目」。新版按 Spotify 拆成
// **左栏选、主区看**：左栏四类（歌单 / 专辑 / 歌手 / 电台），主区显示选中项的
// 全部曲目。于是 `playlistTracks` / `expandedPlaylist` 那一套行内展开**整块删掉了**
// —— 留着会变成两套「曲目列表」的渲染，早晚不一致。

let playlists: PlaylistDto[] | null = null;
let playlistsLoading = false;

let queue: QueueDto | null = null;
let queueOpen = false;
/** 队列内容对应的曲目 id：只在换歌时才重拉（否则每秒一次 API 调用）。 */
let queueFor = "";

// ── 收藏夹（「我喜欢的歌曲」）与搜索（2026-09-27 加）───────────────
/** 收藏夹内容。`null` = 还没拉过。**面板上那一项是前端合成的** ——
 *  Spotify 不把它放进 `/me/playlists`（见 ai-spec §4.6 与 music.rs 的 `spotify_liked`）。
 *  **它现在是左栏歌单栏的第一项**（点它 = 主区显示收藏曲目），不再有「展开态」。 */
let liked: LikedDto | null = null;
let likedLoading = false;
/** 收藏夹读取失败的原因（缺 `user-library-read` 时就是那句 `Insufficient client scope`）。 */
let likedError = "";
/** 搜索词。非空 ⇒ 工具条给出下拉预览，回车/查看全部 ⇒ 主区进搜索详细页。 */
let searchQuery = "";
let searchResult: SearchDto | null = null;
let searchLoading = false;
let searchTimer: number | undefined;

// ── 默认面板（Spotify 复刻）的状态，2026-09-28 ──────────────────
/** 左栏现在显示哪一类。**四类共用一套渲染**（都归一成 `LibraryItemDto`）。 */
let sideTab: SideKind = "playlist";
/** 四类各自的列表缓存（与歌单曲目同一套思路：切回来不重打 API）。 */
const sideCache = new Map<SideKind, LibraryItemDto[]>();
const sideLoading = new Set<SideKind>();
const sideError = new Map<SideKind, string>();
/** 主区正在看的那一项。`null` = 还没选（主区显示引导文案）。 */
let selected: { kind: MainKind; id: string; uri: string; name: string; cover: string; subtitle: string } | null = null;
/** 主区那一列曲目（选中项的全部曲目）。 */
let mainTracks: TrackDto[] | null = null;
let mainLoading = false;
let mainError = "";
/** 搜索下拉预览是否展开（与「主区进搜索详细页」是两件事：这个是预览，那个是整页）。 */
let searchOpen = false;
/** 主区是否在显示搜索详细页。 */
let searchPage = false;
/** 详细页的页签。 */
let searchTab: "tracks" | "artists" | "albums" | "playlists" | "shows" = "tracks";
/** 设备列表 / 本机播放状态。`null` = 还没拉过。 */
let devices: DeviceDto[] | null = null;
let devicesOpen = false;
let librespot: LibrespotDto | null = null;
/** 播放时要带的 `device_id`。**空串 = 让宿主自己解析**（`resolve_play_device`）——
 *  两条路都保留：面板还没就位时（刚打开那一下）点歌也不该失败。 */
let playTarget = "";
/** `ensurePlayTarget` 正在飞（**用来合并并发调用**，2026-10-02）。
 *
 *  它有**两个**触发点：面板挂载时（init 末尾）与授权成功时（`spotify-auth` 事件回调）。
 *  而宿主那条 `music_autoconfigure` 的慢路（拉起 librespot 后轮询等它注册成设备）
 *  要 6 秒多、期间**每次都在打 `/me/player/devices`** —— 两份同时在飞就是两倍地打，
 *  这正是 release 日志里那 31 条 `429 QUOTA_EXCEEDED` 的触发条件。
 *  合并之后：同一时刻最多一份在飞，后来者复用它的结果。
 *  （宿主侧另有**设备快照 + 退避轮询**两道根治，见 music.rs 的 `DEVICE_SNAP`。） */
let playTargetBusy = false;
/** 上一次成功就位的时刻（`Date.now()`）。用来**跳过面板反复自动打开时的重复就位** ——
 *  发布版的音乐插件窗会在 Spotify 桌面端在跑时被自动拉起，每拉一次都重跑一遍
 *  `music_autoconfigure` 就是白白多打一轮设备接口。这个窗口内直接复用已有 `playTarget`。 */
let playTargetAt = 0;
/** 就位结果的复用窗口。**故意短**：设备几分钟内就可能换（关桌面端 / 开手机），
 *  超过它还是老实重跑一次；用户主动的动作（切本机播放 / 刚授权）一律 `force` 绕过。 */
const PLAY_TARGET_TTL_MS = 20_000;
/** **429 硬闸**的前端镜像（宿主是进程级状态，见 music.rs 的 `SPOTIFY_STOPPED`）。
 *
 *  宿主一旦收到 429 就把闸拉下：**此后所有 Spotify Web API 请求当场返回
 *  `ERR_SPOTIFY_STOPPED`、一个字节都不出网**，直到用户点面板上那个「恢复」。
 *  这里记一份用来：① 把「恢复」按钮摆出来；② 停掉本地状态轮询（反正都会被挡回）。
 *  **刻意不做自动过期**（宿主侧也没有）—— 什么时候重试由用户决定。 */
let apiStopped = false;
/** 拉闸原因（宿主给的那句话），显示在按钮的 tooltip 上。 */
let apiStopReason = "";
/** 「最早可重试时刻」的 unix 毫秒（`0` = 没有建议）。
 *
 *  **为什么存绝对时刻而不是「还剩几秒」**：倒计时得自己往下走 —— 存剩余秒数的话，
 *  只有下一次问宿主才会变，两次之间那个数字是冻住的。同机同时钟，本地算即可。
 *  **它只用来显示，不拦用户**：用户明确要求保留「随时能手动恢复」这个出口。 */
let apiRetryAtMs = 0;
/** 用户在**本次会话**里手动关过「本机播放」。关掉之后不再自动拉起它 ——
 *  否则「刚关掉的进程、重开面板又活了」会被当成 bug（宿主那边由 `allow_local` 承接）。 */
let localPlayOptOut = false;
/** 左栏宽度（px）。拖动分隔条改它，**只写进 CSS 变量**（见 `applySideWidth`）。 */
let sideWidth = SIDE_W_DEFAULT;
/** 上一次同步给宿主的 `resizable`，避免每轮 tick 重复调宿主。 */
let lastResizable: boolean | null = null;
/** 标题栏那个关闭按钮**原样的 × 标记**（播放态会把它临时换成「← 回退」，
 *  离开播放态 / 离开这个插件时必须原样换回去，见 `syncTitlebarClose`）。 */
let closeBtnOriginal = "";

// ── 本地媒体库的状态（2026-10-01）──────────────────────────────────
/** 主体现在显示的是不是「本地音乐」那一页。
 *
 *  它是一个**与 Spotify 无关的第三条主区形态**（另两条是搜索详细页、选中项详情），
 *  同时是「没连 Spotify 也允许显示主体」的唯一例外（见 renderMode / renderSetup）。
 *  **不进 `MusicMode`**：那个类型管的是窗口尺寸（默认态 / 播放态），本地页是默认态
 *  里的一块内容，两者维度不同。 */
let localPage = false;
/** 已添加的目录。`null` = 还没拉过（与「拉到了、是空的」必须分开）。 */
let mediaRoots: MediaRootDto[] | null = null;
let mediaRootsLoading = false;
let mediaRootsError = "";
/** 左侧选中的目录（`""` = 全部曲目）。 */
let mediaRootSel = "";
/** 左栏那排按钮的**编辑态**（2026-10-01，用户要求「修改列表」）。
 *  进入方式：那枚笔（`#music-local-list`）。编辑态下点一行 = 勾选 / 取消勾选。 */
let mediaEditMode = false;
/** 编辑态里**勾选中的**目录路径。点垃圾桶把它们摘出列表（暂存）。 */
let mediaSel = new Set<string>();
/** 已经**暂存删除**、还没落盘的目录路径。列表渲染时把它们滤掉；
 *  `× 退出并保存` 才真正逐条调 `media_remove_root`。
 *  **必须有这个中间层**：用户的要求是「退出并保存」，也就是点了×之前的删除都还能反悔
 *  —— 直接调宿主命令的话，勾错一行就没有回头路。 */
let mediaPendingDel = new Set<string>();
/** 那枚 `＋` 展开的**二选一**菜单是不是开着（2026-10-01）。
 *
 *  与 `devicesOpen` / `queueOpen` 同一种浮层：状态位在这里，画在
 *  `renderLocalAddMenu`。单开一个状态位而不是用 `classList.toggle` ——
 *  这个浮层的显隐由 `renderLocalHead` 一并接管（切进编辑态 / 换页时都要关掉），
 *  两边各自 toggle 一次必然有一次把状态搞歪。 */
let addMenuOpen = false;
/** 当前主区那批曲目。`null` = 还没拉过。 */
let mediaTracks: MediaTracksDto | null = null;
let mediaTracksLoading = false;
let mediaTracksError = "";
/** 已打开的那份播放列表文件（`null` = 主区显示的是媒体库那批曲目）。
 *
 *  **它只是主区的一个「临时来源」**，不占左栏、不入库：打开时把 `mediaTracks` 换成
 *  它的曲目，于是点播 / 队列 / 「正在播放」那一行标记全都照旧。
 *  点左栏任一目录、用筛选框、重新扫描 —— 这些都会走 `loadMediaTracks`，
 *  而那条路的第一步就是把 `mediaList` 清掉（= 回到媒体库那一侧）。 */
let mediaList: { path: string; name: string } | null = null;
/** 筛选词（标题 / 歌手 / 专辑 / 专辑歌手的模糊匹配，匹配在宿主的 SQL 里做）。 */
let mediaQuery = "";
let mediaQueryTimer: number | undefined;
/** 扫描进度快照。`null` = 还没轮询过。 */
let mediaScan: ScanStatusDto | null = null;
/** 扫描进度轮询表（只在扫描期间存在）。 */
let mediaScanTimer: number | undefined;

// ── 本地**播放**的状态（2026-10-01）────────────────────────────────
/** 宿主那条音频流的快照（`player.rs`）。`null` = 还没问过。 */
let localPlayer: LocalPlayerDto | null = null;
/** 上面那份快照是**什么时候读到的**（`Date.now()`，毫秒）。
 *
 *  只有画面层用它：宿主报的位置是「读那一拍」的位置，而画面每拍都要重新对齐 ——
 *  不把「从那一拍到现在流过的这段时间」加上去，画面会**永远慢半拍**（最坏差一整个
 *  轮询周期）。`position_ms` 本身没有时间戳，所以只能在这一层记。 */
let localPlayerAt = 0;
/** 本地**队列** = 用户点播那一刻主区那批曲目的**快照**。
 *
 *  **为什么存在这一层**：宿主只认「当前这一首」（见 player.rs 头注释）——
 *  「下一首是谁」需要列表的上下文，而列表只在这一层有。所以「整批可见曲目」就是
 *  这一轮的队列，顺序就是屏幕上的顺序。
 *  **为什么是快照而不是引用 `mediaTracks.items`**：筛选 / 重扫 / 换目录都会把那个
 *  对象整个换掉，正在放的那一首会突然找不到自己的名字（队列一抖，界面就跟着错）。 */
let localQueue: MediaTrackDto[] = [];
/** `localQueue` 里的当前下标。`-1` = 没有队列（还没播过 / 已停）。 */
let localQueueIndex = -1;
/** 有一条 `player_play` 在飞。
 *  **自动续播必须看它**：换曲期间宿主那边还停在上一首（甚至已放完）上，
 *  不挡一下就会连着触发两次「下一首」。 */
let localBusy = false;
/** 进度条拖动中的临时比例（`-1` = 没在拖）。拖动期间**不按宿主的位置画** ——
 *  否则 500ms 一次的快照会把用户手里的滑块一次次拽回去。 */
let localSeekRatio = -1;
/** 本地播放轮询表（只在实际有会话时存在）。 */
let localPollTimer: number | undefined;
/** **恢复播放的起点**（毫秒）。有会话时它跟着快照走；没有会话时（栏常驻）它是
 *  上次存下来的位置 —— 按播放键就从这里续上（2026-10-03「保留上次播放的记录」）。 */
let localResumeMs = 0;

/** 随机播放的**已播历史**（2026-10-03）：栈顶 = 上一次真正在放的那一首。
 *
 *  修的是用户报的那个：shuffle 下按「上一首」会**再随机一首**，而不是回到刚听的那首
 *  —— 根因是 `advanceLocal` 在 shuffle 分支里直接忽略了 `step`。有了这个栈，
 *  「上一首」= 出栈；「下一首 / 自动续播」= 随机 + 把当前这首压栈。
 *  只在 shuffle 下用；换一条播放线（另点一首 / 停止）时清空。 */
let localHistory: number[] = [];

/** **两条控制栏同屏只能有一条**（2026-10-03 用户口径）。默认显示「当前页自己的那条」；
 *  在左端那枚抽屉上点一下，就把**另一侧**那条换出来、这一条收进去（`barSwap` 翻过来）。
 *  在本地页 / 在线页之间切换时**复位**（换页 = 回到「这一页自己的那条」）。 */
let barSwap = false;

/** 现在该显示**本地**那条控制栏吗？两处（`syncHead` 与 `renderLocalNowbar`）**必须
 *  用同一个判据**，各写一份必然出现「两条同时亮 / 一条都没有」的窗口。 */
function localBarVisible(): boolean {
  return localPage !== barSwap;
}

/** 「上次播到哪儿」的持久化记录（2026-10-03 用户口径：「保留上次播放的记录」）。
 *
 *  存**整条队列 + 下标 + 位置**，而不是只存一个路径 —— 栏上要显示歌名 / 歌手 / 封面，
 *  而那份元数据只在队列里（宿主只认「当前这一首」，见 file 头 DTO 段）。
 *  队列是「点播那一刻可见曲目」的快照，几百首也就几十 KB，localStorage 放得下；
 *  写盘一律 `try/catch`（隐私模式 / 配额满了不该让播放出问题）。 */
const LOCAL_LAST_KEY = "lunac-music-local-last";
interface LocalLastRecord {
  queue: MediaTrackDto[];
  index: number;
  position_ms: number;
}
function readLocalLast(): LocalLastRecord | null {
  try {
    const raw = localStorage.getItem(LOCAL_LAST_KEY);
    if (!raw) return null;
    const v = JSON.parse(raw) as LocalLastRecord;
    if (!v || !Array.isArray(v.queue) || v.queue.length === 0) return null;
    const idx = Number(v.index);
    return {
      queue: v.queue,
      index: Number.isInteger(idx) && idx >= 0 && idx < v.queue.length ? idx : 0,
      position_ms: Math.max(0, Number(v.position_ms) || 0),
    };
  } catch {
    return null;
  }
}
/** 写盘。**没有队列就什么都不写**（空态不该把上次的好记录覆盖掉）。 */
function writeLocalLast(): void {
  if (localQueue.length === 0 || localQueueIndex < 0) return;
  try {
    localStorage.setItem(LOCAL_LAST_KEY, JSON.stringify({
      queue: localQueue,
      index: localQueueIndex,
      position_ms: localResumeMs,
    }));
  } catch { /* 隐私模式 / 配额 */ }
}

/** 「本地与 Spotify 可以同时出声」这把开关（2026-10-01，用户第 7 条）。
 *
 *  `false`（默认）= **互斥**：起一路就把另一路停掉（这本来的行为）；
 *  `true` = 两路各放各的 —— 本机那条流在 `player.rs` 里、Spotify 那条在远端，
 *  它们本来就不冲突，所以「共存」不需要任何额外机制，**只需要别去停对方**。
 *
 *  **两个入口共用这一个状态**：Spotify 底部控制栏的 `#music-dual-btn` 与本地
 *  播放条的 `#music-l-dual-btn`。它们不是两件事 —— 所以状态只有这一份、
 *  两枚按钮只是它的两个视图（各自都在自己的控制区里，用户在哪条栏上都够得着）。
 *  **存 localStorage**：这是一条跨会话的偏好，不是「本次界面的临时意图」。 */
const DUAL_PLAY_KEY = "lunac-music-dual-play";
let dualPlay = readDualPlay();

function readDualPlay(): boolean {
  // 插件窗的 localStorage 与主窗同源，**必须带自己的前缀**（`lunac-music-`），
  // 否则会和别的插件 / 主窗的键撞上（同 `lunac-search-engine` 的命名约定）。
  try { return localStorage.getItem(DUAL_PLAY_KEY) === "1"; } catch { return false; }
}

function writeDualPlay(v: boolean): void {
  try { localStorage.setItem(DUAL_PLAY_KEY, v ? "1" : "0"); } catch { /* 隐私模式 / 配额 */ }
}

/** 本地播放的**播放模式**（2026-10-01，用户第 8 条）：关 / 随机 / 循环列表 / 单曲循环。
 *
 *  **与 Spotify 那侧是两套东西，刻意不合成一个状态**：那边是**远端真相**
 *  （`spotify_set_play_mode`，只有 `off` / `shuffle` / `repeat_one` 三档，一律回读），
 *  这边是**前端自己算的** —— 宿主的播放器只认「当前这一首」，队列整个在这一层
 *  （`localQueue`），所以「下一首是谁」本来就只有这里能决定。
 *  **多出一档 `repeat_all`**：Spotify 的 web API 用 `repeat=context` 表达「循环列表」，
 *  而宿主那条本地播放命令压根没有模式参数，是我们自己在 `advanceLocal` 里绕回去的。 */
type LocalMode = "off" | "shuffle" | "repeat_all" | "repeat_one";
const LOCAL_MODE_KEY = "lunac-music-local-mode";
let localMode: LocalMode = readLocalMode();

function readLocalMode(): LocalMode {
  try {
    const v = localStorage.getItem(LOCAL_MODE_KEY);
    return v === "shuffle" || v === "repeat_all" || v === "repeat_one" ? v : "off";
  } catch { return "off"; }
}

function writeLocalMode(v: LocalMode): void {
  try { localStorage.setItem(LOCAL_MODE_KEY, v); } catch { /* 隐私模式 / 配额 */ }
}

/** 本地播放模式那一档的人话（按钮 title 用）。 */
function localModeTitle(): string {
  if (localMode === "shuffle") return t("music.mode_shuffle");
  if (localMode === "repeat_all") return t("music.mode_repeat_all");
  if (localMode === "repeat_one") return t("music.mode_repeat_one");
  return t("music.mode_off");
}

let pollTimer: number | undefined;
let pollBusy = false;
/** 「有交互就补一轮」的去抖定时器（见 `pokePoll`）。 */
let pokeTimer: number | undefined;
/** 两个全局监听器（`visibilitychange` + 面板内交互）**只装一次**。
 *  `attach` 会被反复调用，每次 `addEventListener` 都会**叠加** ⇒ 表现是「点一下补了
 *  好几轮请求」（本仓反复踩过的那类坑）。用一个模块级标志把住。 */
let globalPollListenersReady = false;
/** 进度条本地插值的定时器（见 `paintProgressSmooth`）。与 `pollTimer` 同生共死。 */
let progressTimer: number | undefined;
/** 进度条插值的基准：`progressBaseMs` 是在 `progressBaseAt` 这一刻从宿主读到的位置。
 *  两次轮询之间按真实流逝时间在它之上外推（渲染时由 `renderPlayer` 重新对齐）。 */
let progressBaseMs = 0;
let progressBaseAt = 0;
let unlistenAuth: (() => void) | null = null;
let currentRoot: HTMLElement | null = null;
/** 运行代次：attach 每次递增，旧回调据此丢弃自己的结果（防「关了又开」时的串扰）。 */
let gen = 0;

/** 用户在手动看歌词（滚轮 / 拖滚动条）⇒ 暂时不跟进度走。 */
let followSuspended = false;
/** 进度条拖动中的位置（0–1）；`< 0` = 没在拖。用来**盖住轮询**：
 *  手指下的填充与圆点不能被下一秒的 `player.progress_ms` 拽回去。 */
let seekRatio = -1;
/** 两条进度线（播放长条里那条 + 默认面板底部栏那条）。**回写与拖动都作用在这一组上** ——
 *  两处各记一份状态必然漂移（同预检 #39 ⑩「同一个值只写一处」）。attach 时重新收集。 */
let seekBars: HTMLElement[] = [];
/** 歌词缓动滚动在飞的那一帧。换歌 / 重渲染 / 用户接手时要取消它。 */
let glideRaf = 0;
let followTimer: number | undefined;
let resizeTimer: number | undefined;
/** `#music-msg` 看门狗：上次见到的文本 + 它是从什么时候开始挂着的（见 `sweepMsg`）。 */
let msgSeen = "";
let msgAt = 0;
/** 上一次下发的窗口尺寸，避免每轮都调一次 `set_size`。 */
let lastResize = "";

/** 界面形态。`default` / `settings` / `tuning` **共用默认态的窗口尺寸**（1280×720 定尺），
 *  `player` 与 `lplayer` 都是那条 **550×130 的长条**（前者画 Spotify 的曲目、后者画
 *  本地文件那一首）—— 判「要不要用长条尺寸」的地方一律写 `isBarMode()`（见 applyResize）。
 *  `tuning` 是 2026-10-01 用户要求**独立出来的一页**（调音不再长在设置页里，
 *  见 shellHtml 里 #music-v-tuning 那段注释）。
 *  `lplayer` 是 2026-10-01 用户口径「本地文件播放的情况下也添加小窗模式」——
 *  **刻意不与 `player` 合成一个态**：那两个视图的数据源（远端 Web API vs 宿主
 *  `player_*` 命令）与可用条件（后者不要求登录 Spotify）都不一样，合成一个态
 *  就会到处长出「如果是本地就…」的分支。 */
type MusicMode = "default" | "player" | "lplayer" | "settings" | "tuning";

/** 当前是不是「小窗长条」态（两种：远端 / 本地）。
 *  尺寸、垫料清零、缩放禁用这三件事对两者完全一样，所以收成一个判据 ——
 *  写成两处 `mode === "player" || mode === "lplayer"` 迟早只会改到一处。 */
function isBarMode(): boolean {
  return mode === "player" || mode === "lplayer";
}

interface LrcLine {
  at: number;
  text: string;
}

function esc(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

function fmtMs(ms: number): string {
  if (!isFinite(ms) || ms < 0) return "0:00";
  const total = Math.floor(ms / 1000);
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${String(s).padStart(2, "0")}`;
}

/** 进度条旁边那对时间用的格式：**mm:ss，两位都补零**（用户要的形态 = `00:00 / 00:00`）。
 *
 *  与 `fmtMs` 分开而不是改它：曲目行里的时长是 `3:45` 那种紧凑写法（几十行并排，
 *  补零反而显得松散），而进度旁边只有一对数字、要的是**位数固定**（不固定的话
 *  秒数从 `9` 变 `10` 时整串会左右抖一下）。两处用途不同，不许合并。 */
function fmtClock(ms: number): string {
  if (!isFinite(ms) || ms < 0) return "00:00";
  const total = Math.floor(ms / 1000);
  return `${String(Math.floor(total / 60)).padStart(2, "0")}:${String(total % 60).padStart(2, "0")}`;
}

/** 把宿主返回的哨兵错误码翻成当前语言；其余原样显示。
 *
 *  **Spotify 的拒因要翻成人话**（2026-09-27 加）：它把原因写在响应体的
 *  `message` / `reason` 里，而面板上那行小字是用户唯一的线索 ——
 * 原样贴一段 JSON 等于什么都没说。按**子串**判而不是解析 JSON：
 * 那条消息被宿主截断成 200 字、还带着格式化换行，正则解析反而是新的失败点。 */
function errText(e: unknown): string {
  const raw = e instanceof Error ? e.message : String(e);
  if (raw.includes("ERR_NO_CLIENT_ID")) return t("music.err_no_client_id");
  if (raw.includes("ERR_NOT_CONNECTED")) return t("music.err_not_connected");
  if (raw.includes("ERR_NO_QUERY")) return t("music.err_no_query");
  if (raw.includes("ERR_PORT_BUSY")) return t("music.err_port_busy", { port: String(cfg?.port ?? "") });
  if (raw.includes("ERR_BAD_URI") || raw.includes("ERR_BAD_ID")) return t("music.err_bad_uri");
  // **429 硬闸**（宿主 `spotify_send`，2026-10-02）：Spotify 通道整个停了，等用户手动恢复。
  // 记状态的动作放在这里 —— `errText` 是所有错误显示的**必经之路**（三十多处 catch 都调它），
  // 记在别处一定会漏。**必须排在下面那条之前**：两条都叫「限流」，但 `ERR_RATE_LIMITED`
  // 说的是歌词服务（LRCLIB），混用会把用户引去排查错的东西。
  if (raw.includes("ERR_SPOTIFY_STOPPED")) {
    apiStopped = true;
    return t("music.err_spotify_stopped");
  }
  const m = raw.match(/ERR_RATE_LIMITED:(\S*)/);
  if (m) return t("music.err_rate_limited", { s: m[1] || "?" });
  if (raw.includes("Insufficient client scope")) return t("music.err_scope");
  if (raw.includes("No active device") || raw.includes("NO_ACTIVE_DEVICE")) return t("music.err_no_device");
  if (raw.includes("Premium required") || raw.includes("PREMIUM_REQUIRED")) return t("music.err_premium");
  if (raw.includes("Restriction violated")) return t("music.err_restricted");
  return raw;
}

// ── LRC 解析 ──────────────────────────────────────────────────────
/** 解析 `[mm:ss.xx] 词`。一行可能有多个时间戳（副歌复用），逐个展开。 */
function parseLrc(text: string): LrcLine[] {
  const out: LrcLine[] = [];
  for (const raw of text.split(/\r?\n/)) {
    const stamps = [...raw.matchAll(/\[(\d{1,3}):(\d{1,2})(?:[.:](\d{1,3}))?\]/g)];
    if (stamps.length === 0) continue;
    const body = raw.replace(/\[[^\]]*\]/g, "").trim();
    if (!body) continue;
    for (const m of stamps) {
      const frac = m[3] ? Number(m[3].padEnd(3, "0")) / 1000 : 0;
      out.push({ at: Number(m[1]) * 60 + Number(m[2]) + frac, text: body });
    }
  }
  out.sort((a, b) => a.at - b.at);
  return out;
}

// ── 视图骨架 ──────────────────────────────────────────────────────
// 两态都在 DOM 里，靠 `.hidden` 切 —— 这样 `applyResize()` 量到的就是**当前那一态**
// 的真实内容高（`display:none` 的元素不参与布局，量出来是 0）。
/** **频响曲线窗**的外壳（2026-10-03）：曲线从调音页搬进这扇独立的窗。
 *
 *  为什么单独一扇窗（用户口径「参照 Peace」）：曲线是「一边看一边调」的东西，占满主界面
 *  会把推子与数值表挤没；而调音页本身要按 Peace 的三栏骨架排（左预设 / 中 EQ / 右通道槽）。
 *
 *  **刻意复用主页面那一组 id**（`#music-tune-canvas` / `#music-tune-measure` /
 *  `#music-tune-slots` …）：`drawTuningCurve` / `renderProbe` / `renderTuneSlots` /
 *  `wireTuningCurve` 一行都不用改就能在两边都跑 —— 另起一套 id 等于把这些函数抄两份，
 *  两份必然漂移。这些 render 对找不到的节点都是 null 安全的（各自都先 `querySelector`
 *  再判空）。
 *
 *  **只读展示**：这里**不画拖动手柄**（见 `drawTuningCurve` 里那条 `isCurveWindow` 判断）
 *  —— 同一个画布在两个窗里承担不同的编辑职责，迟早会出现「在窗里拖了一下、主页面没跟上」。
 *
 *  ⚠️ 本段是 JS 模板字符串 ⇒ 注释里不许出现反引号（预检 #15 / #39 ⑩）。 */
function curveShellHtml(): string {
  return `
  <div class="music-root music-curve-root">
    <div class="music-setview">
      <div class="music-set-head">
        <span class="music-set-title">${t("music.tuning_curve")}</span>
      </div>
      <div class="music-set-body">
        <div class="music-tune-plot">
          <!-- 曲线缩放（与调音页同一条交互）：画布默认铺满宽度，点 ＋/－ 按百分比变宽，
               外层容器横向滚动 —— 看细节放大、看全局缩回。 -->
          <div class="music-tune-toolbar">
            <button type="button" class="music-tune-zoom" id="music-tune-zoom-out" title="${esc(t("music.tuning_zoom_out"))}" aria-label="${esc(t("music.tuning_zoom_out"))}">－</button>
            <span class="music-tune-zoom-label" id="music-tune-zoom-label">100%</span>
            <button type="button" class="music-tune-zoom" id="music-tune-zoom-in" title="${esc(t("music.tuning_zoom_in"))}" aria-label="${esc(t("music.tuning_zoom_in"))}">＋</button>
          </div>
          <div class="music-tune-scroll" id="music-tune-scroll">
            <canvas id="music-tune-canvas" width="640" height="280"></canvas>
          </div>
          <div class="music-hint" id="music-tune-rate"></div>
        </div>
        <div class="music-tune-probe">
          <button type="button" class="music-btn" id="music-tune-measure">${SVG.wave}${t("music.tuning_measure")}</button>
          <div class="music-hint" id="music-tune-probe-hint">${esc(t("music.tuning_probe_hint"))}</div>
        </div>
        <div class="music-field">
          <label>${t("music.tuning_slots")}</label>
          <div class="music-seg music-slot-list" id="music-tune-slots"></div>
        </div>
      </div>
    </div>
    <div class="music-msg" id="music-msg"></div>
  </div>`;
}

function shellHtml(): string {
  return `
  <div class="music-root">
    <!-- ── 默认界面：顶部工具条 + 状态行 + 连接设置 + 歌单列 / 搜索结果 ──
         工具条与「返回」按钮**分属两态**：默认态用底部栏那个「进播放界面」，
         播放态用**标题栏那个 ←**（syncTitlebarClose 把 × 换成它；歌名模块右端
         原来还有一枚「返回歌单」，2026-10-01 已按用户口径删掉）。
         ⚠️ 本段在模板字符串里，注释里一个反引号都不能出现（预检 #15 / #39 ⑩）。 -->
    <div class="music-view" id="music-v-default">
      <!-- 顶部工具条：头像 / 账号 / 搜索（**带下拉预览**）/ 状态 / 设置。
           2026-09-28 第二次改：设备按钮与「展开播放面板」都**下移到最底下那条控制栏**
           （用户要求「严格对齐 Spotify」，Spotify 的工具条上本来就没有这两样）。 -->
      <div class="music-toolbar">
        <button class="music-avatar" id="music-avatar" title="${esc(t("music.account_btn"))}"></button>
        <span class="music-acc" id="music-acc"></span>
        <div class="music-search-wrap">
          <input class="music-search" id="music-search" type="search" autocomplete="off" spellcheck="false" placeholder="${esc(t("music.search_ph"))}">
          <!-- 下拉预览：绝对定位挂在工具条下方（**不占布局**，否则窗口高会跟着抖） -->
          <div class="music-dd hidden" id="music-search-dd"></div>
        </div>
        <span class="music-status" id="music-status">${t("music.loading")}</span>
        <!-- 「恢复」按钮（2026-10-02）：**只在 429 硬闸拉着时显示**（见 renderResumeBtn）。
             它是用户唯一的出口 —— 宿主一旦收到 429 就停掉所有 Spotify 请求、且不自动重试。
             放工具条而不是底部提示行：提示行在播放态是绝对定位 + pointer-events:none，点不到。
             ⚠️ 本段是 JS 模板字符串，注释里不能出现反引号（预检 #15 / #39 ⑩）。 -->
        <button class="music-ghost-btn hidden" id="music-resume">${t("music.resume")}</button>
        <!-- 这里原来的「本地音乐」文件夹按钮与「调音」推子按钮**已删除**（2026-10-01
             用户口径）：这两个入口收进了**标题栏那枚三段切换**（在线歌单 / 本地音乐 /
             调音，见 renderTitlebarSwitch）。标题栏是悬浮窗独有的，插件若被嵌在主窗口里
             就没有那一条 —— 那种情况下切换器会退到本工具条里（见 placeSwitcher），
             所以这里不给它们留第二份入口，否则「删掉」就变成了「多一份」。
             ⚠️ 本段是 JS 模板字符串，注释里不能出现反引号（预检 #15 / #39 ⑩）。 -->
        <button class="music-ghost-btn music-icon-btn" id="music-setup-toggle" title="${esc(t("music.settings"))}">${SVG.gear}</button>
      </div>

      <div class="music-setup hidden" id="music-setup">
        <div class="music-hint" id="music-setup-hint">${t("music.setup_hint")}</div>
        <!-- 凭据字段默认只在「没有内置 Client ID」或用户主动展开时显示：
             内置值让「登录」变成一步操作，Redirect URI 那一串对普通用户是噪音。 -->
        <button class="music-ghost-btn music-fields-toggle hidden" id="music-fields-toggle"></button>
        <div id="music-setup-fields">
          <div class="music-field">
            <label for="music-client-id">${t("music.client_id")}</label>
            <input id="music-client-id" type="text" spellcheck="false" autocomplete="off" placeholder="${esc(t("music.client_id_ph"))}">
          </div>
          <div class="music-field music-field-port">
            <label for="music-port">${t("music.port")}</label>
            <input id="music-port" type="text" inputmode="numeric" autocomplete="off">
          </div>
          <div class="music-field">
            <label>${t("music.redirect")}</label>
            <div class="music-redirect-row">
              <code id="music-redirect"></code>
              <button class="music-ghost-btn music-icon-btn" id="music-copy-redirect" title="${esc(t("music.copy"))}">${SVG.copy}</button>
            </div>
          </div>
          <!-- 本机播放（librespot）的两项设置：路径留空 = 自动探测；代理留空 = 直连。
               两者都不是「必填」，所以各带一行小字说明（见 renderSetup）。 -->
          <div class="music-field">
            <label for="music-librespot">${t("music.local_play")}</label>
            <input id="music-librespot" type="text" spellcheck="false" autocomplete="off" placeholder="librespot.exe">
          </div>
          <div class="music-field">
            <label for="music-librespot-proxy">proxy</label>
            <input id="music-librespot-proxy" type="text" spellcheck="false" autocomplete="off" placeholder="${esc(t("music.librespot_proxy_ph"))}">
          </div>
        </div>
        <div class="music-actions">
          <button class="music-btn" id="music-save">${t("music.save")}</button>
          <button class="music-btn music-primary" id="music-connect">${SVG.link} ${t("music.connect")}</button>
          <button class="music-btn" id="music-disconnect">${SVG.power} ${t("music.disconnect")}</button>
        </div>
      </div>

      <!-- 主体：**左栏（宽度可拖）+ 分隔条 + 主区**（用户 2026-09-28 的
           「歌单列作为单独的一列可拉动」「中间大的大部分区域作为歌曲列表」）。
           两栏各自内部滚动 —— 窗口是定尺 1280×720，不是靠内容撑出来的。 -->
      <div class="music-body hidden" id="music-body">
        <aside class="music-side" id="music-side">
          <div class="music-side-tabs" id="music-side-tabs">
            <button class="music-tab on" data-act="side-tab" data-tab="playlist">${t("music.playlists")}</button>
            <button class="music-tab" data-act="side-tab" data-tab="album">${t("music.tab_albums")}</button>
            <button class="music-tab" data-act="side-tab" data-tab="artist">${t("music.tab_artists")}</button>
            <button class="music-tab" data-act="side-tab" data-tab="show">${t("music.tab_shows")}</button>
            <!-- 「最常听」栏（2026-10-01 用户第 4 条）。**放在最后**：那四栏是「我的库」，
                 这一栏是「库外的东西」，顺序上按这个分组走。
                 原先与它并列的「新发行」2026-10-02 删除（端点永久 403）。
                 注：本段在模板字符串里，注释里不能出现反引号（预检 #15 / #39 ⑩）。 -->
            <button class="music-tab" data-act="side-tab" data-tab="top">${t("music.tab_top")}</button>
          </div>
          <!-- 「本地音乐」那一页的左栏头（与上面那排 Spotify 页签**互斥显示**）：
               一个标题 + 一个「添加目录」。它跟着 localPage 切，见 renderLocalSide。 -->
          <div class="music-local-head hidden" id="music-local-head">
            <span class="music-local-title">${t("music.local_library")}</span>
            <div class="music-local-head-btns">
              <!-- 正常态三枚（2026-10-01 用户口径）：
                   ＋ = 添加（**目录或播放列表文件**，两个入口合一）
                   ✎ = 修改列表（进编辑态）—— 取代原来那枚「打开播放列表文件」
                   ⟳ = 重扫。
                   图标一律取自 icon-style.md §3，不自己画（见 SVG.edit / SVG.trash）。 -->
              <button class="music-ghost-btn music-icon-btn" id="music-local-add" title="${esc(t("music.local_add"))}">${SVG.plus}</button>
              <button class="music-ghost-btn music-icon-btn" id="music-local-list" title="${esc(t("music.local_edit"))}">${SVG.edit}</button>
              <button class="music-ghost-btn music-icon-btn" id="music-local-scan" title="${esc(t("music.local_rescan"))}">${SVG.refresh}</button>
              <!-- 编辑态两枚（正常态隐藏，切法见 renderLocalHead）：
                   🗑 = 把勾选的行**摘出列表**（暂存，还能反悔）
                   × = 退出并保存（到这一步才真落盘） -->
              <button class="music-ghost-btn music-icon-btn hidden" id="music-local-del" title="${esc(t("music.local_del_sel"))}">${SVG.trash}</button>
              <button class="music-ghost-btn music-icon-btn hidden" id="music-local-editdone" title="${esc(t("music.local_edit_done"))}">${SVG.x}</button>
              <!-- 那个「＋」的二选一浮层（目录 / 播放列表文件）。画法见 renderLocalAddMenu，
                   定位参照是上面那排按钮（.music-local-head-btns 是 relative）。
                   注：本段在**模板字符串**里，注释里一个反引号都不能出现（预检 #15 / #39 ⑩）。 -->
              <div class="music-dd music-local-add-dd hidden" id="music-local-add-dd"></div>
            </div>
          </div>
          <div class="music-side-list" id="music-side-list"></div>
        </aside>
        <div class="music-split" id="music-split" role="separator" aria-orientation="vertical"></div>
        <main class="music-main" id="music-main"></main>
      </div>

      <!-- ── 底部控制栏（2026-09-28，Spotify 复刻）─────────────────────────
           用户要求「在默认面板下方添加 Spotify 那样的播放控制，并把一些按钮也
           移动到下方控制」。三列 grid：左 = 正在播放（点它 = 打开播放面板）、
           中 = 播放键组、右 = 设备 / 音量 / 队列 / 展开。
           栏顶那条 2px 细进度线**绝对定位**（不占高），拖动即可跳转。
           **两个弹层都挂在右端那一簇里、向上弹**：这条栏贴着窗口下沿，
           向下弹会跑到窗口外面（见 styles.css 里那两条覆盖 top 的规则）。
           注意本段是 JS 模板字符串 —— 注释里不能出现反引号（会把字符串截断）。 -->
      <div class="music-nowbar no-drawer hidden" id="music-nowbar">
        <!-- 「另一侧」的抽屉（2026-10-03 用户口径）：两条控制栏同屏只能有一条，
             另一条收进左端这一枚里 —— 点它把另一条换出来、这一条收进去。
             只在「同时出声」开着**且另一侧确实有东西**时出现（见 syncHead）。
             它是 grid 的第一列（auto 宽），隐藏时整列塌成 0 ⇒ 不开抽屉时外观不变。 -->
        <button type="button" class="music-drawer-btn hidden" id="music-b-drawer"></button>
        <div class="music-bp music-bp-timed" id="music-bp" title="${esc(t("music.seek"))}">
          <div class="music-bp-fill" id="music-bp-fill"></div>
          <div class="music-progress-tip" id="music-bp-tip">0:00</div>
          <span class="music-btime" id="music-b-time">00:00 / 00:00</span>
        </div>
        <button class="music-bnow" id="music-bnow" title="${esc(t("music.open_player"))}">
          <span class="music-bnow-cover" id="music-bnow-cover"></span>
          <span class="music-bnow-main">
            <span class="music-bnow-n" id="music-bnow-n"></span>
            <span class="music-bnow-a" id="music-bnow-a"></span>
          </span>
          <!-- 「进入小窗播放」的位置**收在左边这一块上**（2026-10-03 用户口径）：
               左块本身就是那个按钮，右端那枚重复的展开按钮已删。这个图标只是为了
               让「它可点」这件事看得见（同一条口径也铺到了本地那条栏，见下面）。 -->
          <span class="music-bnow-go">${SVG.expand}</span>
        </button>
        <div class="music-bctl">
          <button class="music-round-btn music-mode-btn" id="music-b-mode" data-mode="off"></button>
          <button class="music-round-btn" id="music-b-prev" title="${esc(t("music.prev"))}">${SVG.prev}</button>
          <button class="music-round-btn music-play-btn" id="music-b-play" title="${esc(t("music.play_pause"))}">${SVG.play}</button>
          <button class="music-round-btn" id="music-b-next" title="${esc(t("music.next"))}">${SVG.next}</button>
        </div>
        <div class="music-bend">
          <!-- 与本地播放条上那一枚是**同一个状态**（见 dualPlay 的注释）。
               两枚按钮各自长在自己的控制区里，用户在哪条栏上都够得着。 -->
          <button class="music-ghost-btn music-icon-btn music-dual-btn" id="music-dual-btn" title="${esc(t("music.dual_play"))}">${SVG.dual}</button>
          <button class="music-ghost-btn music-icon-btn" id="music-devices-btn" title="${esc(t("music.devices"))}">${SVG.speaker}</button>
          <span class="music-vol-ico">${SVG.volume}</span>
          <input class="music-vol" id="music-b-vol" type="range" min="0" max="100" step="1" value="50" style="--vol-pct:50%" title="${esc(t("music.volume"))}">
          <button class="music-round-btn" id="music-b-queue" title="${esc(t("music.queue"))}">${SVG.list}</button>
          <!-- 右端那枚「进入小窗播放」**已删**（2026-10-03 用户口径）：它与左边
               「正在播放」那一块是同一个动作（都进小窗播放态），右边这枚是重复入口。
               保留左边那块既有的按钮语义，右端这一簇从此只放设备 / 音量 / 队列。 -->
          <div class="music-dd music-dev-dd hidden" id="music-dev-dd"></div>
          <div class="music-dd hidden" id="music-q-dd"></div>
        </div>
      </div>

      <!-- ── 本地文件播放条（2026-10-01）───────────────────────────────
           与上面那条 Spotify 控制栏**不是一条**：那条的对象是远端播放器
           （Web API 的 spotify_* 命令），这条的对象是宿主里那条本机音频流
           （player.rs 的 player_* 命令）。两者的数据源、可用条件（这条不要求
           登录 Spotify）、能做的事都不一样 —— 合成一条会到处长出
           「如果是本地就…」的分支，那正是两套东西被缝在一起的样子。
           只在「本地页 + 有本地会话」时出现，见 renderLocalNowbar。
           注意本段是 JS 模板字符串 —— 注释里不能出现反引号。 -->
      <div class="music-nowbar music-lnowbar no-drawer hidden" id="music-lnowbar">
        <!-- 「另一侧」的抽屉，与上面那条栏同形同义（点击 = 把在线那条换出来）。 -->
        <button type="button" class="music-drawer-btn hidden" id="music-l-drawer"></button>
        <div class="music-bp" id="music-lbp" title="${esc(t("music.seek"))}">
          <div class="music-bp-fill" id="music-lbp-fill"></div>
          <div class="music-progress-tip" id="music-lbp-tip">0:00</div>
        </div>
        <!-- 左边这一块**就是**「进入小窗播放」的入口（2026-10-03 用户口径）：
             原来它是纯展示（本地没有小窗可进），2026-10-01 有了本地小窗之后那句话
             就不成立了 ⇒ 与 Spotify 那条栏同形，做成按钮，点它进 #music-v-lplayer。 -->
        <button class="music-bnow music-lnow" id="music-lnow" title="${esc(t("music.open_player"))}">
          <span class="music-bnow-cover" id="music-lnow-cover"></span>
          <span class="music-bnow-main">
            <span class="music-bnow-n" id="music-lnow-n"></span>
            <span class="music-bnow-a" id="music-lnow-a"></span>
          </span>
          <span class="music-bnow-go">${SVG.expand}</span>
        </button>
        <div class="music-bctl">
          <!-- 播放模式（关 / 随机 / 循环列表 / 单曲循环，2026-10-01 用户第 8 条）。
               与 Spotify 那枚 #music-b-mode **不是一件事**：那枚是远端真相，
               这枚是前端自己算的（见 localMode 的注释），所以两枚各记各的状态。
               注：本段在模板字符串里，注释里不能出现反引号（预检 #15 / #39 ⑩）。 -->
          <button class="music-round-btn music-mode-btn" id="music-l-mode" data-mode="off" title="${esc(t("music.mode_off"))}"></button>
          <button class="music-round-btn" id="music-l-prev" title="${esc(t("music.prev"))}">${SVG.prev}</button>
          <button class="music-round-btn music-play-btn" id="music-l-play" title="${esc(t("music.play_pause"))}">${SVG.play}</button>
          <button class="music-round-btn" id="music-l-next" title="${esc(t("music.next"))}">${SVG.next}</button>
          <button class="music-round-btn" id="music-l-stop" title="${esc(t("music.local_stop"))}">${SVG.stop}</button>
        </div>
        <div class="music-bend">
          <button class="music-ghost-btn music-icon-btn music-dual-btn" id="music-l-dual-btn" title="${esc(t("music.dual_play"))}">${SVG.dual}</button>
          <span class="music-ltime" id="music-l-time">0:00 / 0:00</span>
          <span class="music-vol-ico">${SVG.volume}</span>
          <input class="music-vol" id="music-l-vol" type="range" min="0" max="100" step="1" value="80" style="--vol-pct:80%" title="${esc(t("music.volume"))}">
          <!-- 右端那枚「进入小窗播放」**已删**（2026-10-03 用户口径）：动作与左边
               「正在播放」那一块重复，两边只留左边一个入口。 -->
        </div>
      </div>
    </div>

    <!-- ── 播放界面：**550×130 的长条**（封面 100 + 右侧列 450）──────────
         右侧列三块的高度加起来正好是长条高：歌名 30 + 歌词 65 + 控制 35 = 130。
         **这几个数都是用户 2026-09-27 给的定尺**：改任何一个都要同步
         styles.css 的 .music-name / .music-lyr / .music-bar，以及宿主
         plugin_window.rs 的 MUSIC_W（见 ai-spec §4.6 与预检 #39 ④）。 -->
    <!-- 注意：本段是 JS 模板字符串，**注释里不能出现反引号**（会把字符串截断）。 -->
    <div class="music-view music-player hidden" id="music-v-player">
      <div class="music-pv-card">
        <div class="music-pv-cover" id="music-pv-cover"></div>
        <div class="music-pv-col">
          <!-- 歌名模块 H30：歌名（亮）+ 歌手·专辑（暗）。 -->
          <div class="music-name">
            <div class="music-name-text">
              <span class="music-title" id="music-pv-title"></span>
              <span class="music-sub" id="music-pv-sub"></span>
            </div>
            <!-- 「返回歌单」那枚**已删除**（2026-10-01 用户口径：小窗播放里那个不是
                 状态栏里的返回按钮要去掉）。退出去仍然走**标题栏那个 ←** ——
                 它在播放态由 syncTitlebarClose 把 × 换成 ←，语义是「停播 + 回默认面板」
                 （见 stopAndBack），也是用户当初指定的出口。所以这里删掉不留缺口。 -->
          </div>

          <!-- 歌词模块 H65：正好两行可见（行高 32）。已过 + 正在 = 亮，未到 = 暗；
               点某一行 = 跳到那一句的进度。 -->
          <div class="music-lyr" id="music-lyr"></div>

          <!-- 控制面板 H35：进度条**叠在本块顶部**（绝对定位 ⇒ 不占高度，总高仍是 130），
               四个播放键在块内居中，右端音量条 80px + 播放列表按钮。 -->
          <div class="music-bar">
            <!-- 时间在控制块**最左端、进度条下方**（用户 2026-09-27 定）。
                 绝对定位 ⇒ 不参与四钮的居中计算。 -->
            <div class="music-time">
              <span id="music-pv-pos">0:00</span><span class="music-time-sep">/</span><span id="music-pv-dur">0:00</span>
            </div>
            <div class="music-progress" id="music-pv-progress" title="${esc(t("music.seek"))}">
              <div class="music-progress-fill" id="music-pv-fill"></div>
              <div class="music-progress-dot" id="music-pv-dot"></div>
              <div class="music-progress-tip" id="music-pv-tip">0:00</div>
            </div>
            <div class="music-bar-main">
              <button class="music-round-btn music-mode-btn" id="music-mode" data-mode="off"></button>
              <button class="music-round-btn" id="music-prev" title="${esc(t("music.prev"))}">${SVG.prev}</button>
              <button class="music-round-btn music-play-btn" id="music-play" title="${esc(t("music.play_pause"))}">${SVG.play}</button>
              <button class="music-round-btn" id="music-next" title="${esc(t("music.next"))}">${SVG.next}</button>
            </div>
            <div class="music-bar-end">
              <span class="music-vol-ico">${SVG.volume}</span>
              <input class="music-vol" id="music-vol" type="range" min="0" max="100" step="1" value="50" style="--vol-pct:50%" title="${esc(t("music.volume"))}">
              <button class="music-round-btn" id="music-queue-btn" title="${esc(t("music.queue"))}">${SVG.list}</button>
            </div>
          </div>
        </div>
      </div>

      <!-- 播放列表（队列）挂在长条**下方**、不再与歌词互斥：长条是定尺 130，
           塞不进队列。开着时窗口按内容长高（applyResize 实测贴合）。 -->
      <div class="music-q-wrap hidden" id="music-q-wrap">
        <div class="music-sec-head">
          <span class="music-sec-title">${t("music.queue")}</span>
          <span class="music-sec-note">${t("music.queue_hint")}</span>
        </div>
        <div class="music-q" id="music-q"></div>
      </div>
    </div>

    <!-- ── 本地小窗：**本地文件播放也有的那条长条**（2026-10-01 用户口径
         「在本地文件播放的情况下也添加小窗模式」）────────────────────────
         与上一个视图**共用同一套外壳类**（music-player / music-pv-card / music-pv-cover /
         music-name / music-bar / music-bp…）⇒ 尺寸（550×130）、圆角、配色、
         进度条的细线与命中区，一行业务 CSS 都不用重写。
         唯一的差别在**中间那一块**：远端那条长条放的是歌词（H65，见 .music-lyr），
         本地文件没有歌词 —— 那一格换成**大号进度线 + 时间**（见 .music-lp-mid）。
         三块的高度仍是 30 + 65 + 35 = 130，与 MUSIC_W_BAR / applyResize 对齐。
         数据与动作**都只有一份**：状态来自 localPlayer，按钮走的是本地播放那几条
         既有函数（见 attachMusicListeners 里那几条绑定）。
         注意本段是 JS 模板字符串 —— 注释里不能出现反引号（预检 #15 / #39 ⑩）。 -->
    <div class="music-view music-player hidden" id="music-v-lplayer">
      <div class="music-pv-card">
        <div class="music-pv-cover" id="music-lp-cover"></div>
        <div class="music-pv-col">
          <div class="music-name">
            <div class="music-name-text">
              <span class="music-title" id="music-lp-title"></span>
              <span class="music-sub" id="music-lp-sub"></span>
            </div>
          </div>
          <div class="music-lp-mid">
            <div class="music-bp music-lp-seek" id="music-lp-progress" title="${esc(t("music.seek"))}">
              <div class="music-bp-fill" id="music-lp-fill"></div>
              <div class="music-progress-tip" id="music-lp-tip">0:00</div>
            </div>
            <div class="music-lp-time" id="music-lp-time">0:00 / 0:00</div>
          </div>
          <div class="music-bar">
            <div class="music-bar-main">
              <button class="music-round-btn music-mode-btn" id="music-lp-mode" data-mode="off" title="${esc(t("music.mode_off"))}"></button>
              <button class="music-round-btn" id="music-lp-prev" title="${esc(t("music.prev"))}">${SVG.prev}</button>
              <button class="music-round-btn music-play-btn" id="music-lp-play" title="${esc(t("music.play_pause"))}">${SVG.play}</button>
              <button class="music-round-btn" id="music-lp-next" title="${esc(t("music.next"))}">${SVG.next}</button>
              <button class="music-round-btn" id="music-lp-stop" title="${esc(t("music.local_stop"))}">${SVG.stop}</button>
            </div>
            <div class="music-bar-end">
              <span class="music-vol-ico">${SVG.volume}</span>
              <input class="music-vol" id="music-lp-vol" type="range" min="0" max="100" step="1" value="80" style="--vol-pct:80%" title="${esc(t("music.volume"))}">
            </div>
          </div>
        </div>
      </div>
    </div>

    <!-- ── 设置页：**整屏**（2026-10-01 用户要求「设置图标进到一个新的设置页面」）──
         与 #music-v-player 同为整屏视图，由 mode = "settings" 切（见 renderMode）。
         职责划分是硬的：**这里只放「偏好」，「连接」仍归头像点开的 #music-setup**
         （Client ID / 端口 / 回调 / librespot 路径与代理），**「调音」也有自己的一页**
         （#music-v-tuning，2026-10-01 用户要求搬出去的）。理由与那张卡片里的
         「保存」按钮直接相关 —— 一个卡片里塞两套保存语义必然出岔子，而现在这几页
         各有各的保存。
         ⚠️ 本段是 JS 模板字符串 —— 注释里**不许出现反引号**，连「引用一个标识符」
         那种写法都不行：它会把模板字符串提前截断（预检 #15 / #39 ⑩，本轮又踩了一次）。 -->
    <div class="music-view music-setview hidden" id="music-v-settings">
      <div class="music-set-head">
        <button class="music-ghost-btn music-icon-btn" id="music-set-back" title="${esc(t("music.settings_back"))}">${SVG.back}</button>
        <span class="music-set-title">${t("music.settings")}</span>
      </div>
      <div class="music-set-body">
        <!-- 串流质量：三档互斥按钮。**不用原生 select 元素** —— WebView2 透明窗口里
             它不渲染弹出层（code-rules §5.2）。三档值只允许 96/160/320（宿主侧
             LIBRESPOT_BITRATES，抄自 librespot --help 的原文）。
             ⚠️ 模板字符串 ⇒ 本段注释里也不许出现反引号（预检 #39 ⑩）。 -->
        <div class="music-field">
          <label>${t("music.quality")}</label>
          <div class="music-seg" id="music-quality">
            ${[96, 160, 320].map((k) => `<button type="button" class="music-seg-btn" data-q="${k}">${k}k</button>`).join("")}
          </div>
          <div class="music-hint">${esc(t("music.quality_hint"))}</div>
          <div class="music-hint">${esc(t("music.quality_local_note"))}</div>
        </div>
        <!-- 调音页的入口（2026-10-03 用户要求「设置按钮入口」）。**不在这页里编辑调音**：
             调音是**按下即生效、即落盘**的实时功能，而这页那套是「攒着等保存」
             （音质要重启 librespot 才生效）—— 两种保存语义混在一张卡片里必然出岔子。
             所以这里只给一颗**进去的按钮**（切到 #music-v-tuning 那一页）。 -->
        <div class="music-field">
          <label>${t("music.tuning")}</label>
          <div class="music-actions">
            <button type="button" class="music-btn" id="music-set-tuning">${t("music.tuning_open")}</button>
          </div>
        </div>
        <div class="music-actions">
          <button class="music-btn" id="music-set-save">${t("music.save")}</button>
        </div>
      </div>
    </div>

    <!-- ── 调音页：**自己一页**（2026-10-01 用户口径）────────────────────
         用户明确「调音插件的界面不能放置到设置界面里，要新建一个界面专门用来显示调音」
         —— 所以它在 2026-10-01 从设置页那一块**整段搬过来**（上一版是「调音分区」）。
         入口只有一个：标题栏那枚三段切换的第三段（见 renderTitlebarSwitch）——
         **页内那枚返回按钮 2026-10-02 按用户要求取掉了**：切换器本身就是「回得去」的
         那一条路（点回「在线歌单 / 本地音乐」即离页），再摆一个返回是重复入口。
         版面顺序（2026-10-03 用户口径，本轮重排）：三栏骨架 ——
         左 = 工具条（保存 / 导入 / 导出）+ 我的预设；中 = 三条全局水平滑块 +
         **频段柱**（滤波器与均衡器已合并）；右 = 通道槽 + 全局效果器（延迟 / 复制 / 卷积）。
         四条纪律：① 控件**按下就生效、也当场落盘**（与设置页「音质」那两条攒着
         等保存完全不同：音质要重启 librespot 才生效，调音是实时的）；
         ② 滤波器类型的**名单来自宿主**（tuning.kinds，与引擎 Kind::ALL 同源）、
         用户预设名单也来自宿主（tuning.user_presets）—— 前端不维护第二份名单；
         ③ **两条播放链都生效**（2026-10-04 起）：本地文件走宿主 player.rs，在线歌单走
         librespot 的 -B pipe 输出 → 宿主的 live_audio.rs，两条套的是**同一个**
         TuningSource（共享同一份配置与版本号 ⇒ 在哪边调都立刻生效）。唯一例外是
         **首次授权那一次**：那一次不能带 -B pipe（OAuth 会把 URL 打到 stdout、污染
         PCM，见 librespot_args 的三条注释），重启一次 librespot 即接入；
         ④ 频段柱里的控件**全部动态渲染**（见 renderTuningFilters），这里只留容器。
         ⚠️ 本段是 JS 模板字符串 ⇒ 注释里不许出现反引号（预检 #15 / #39 ⑩）。 -->
    <div class="music-view music-setview hidden" id="music-v-tuning">
      <div class="music-set-head">
        <span class="music-set-title">${t("music.tuning")}</span>
        <!-- 频响曲线**不在这页里**（2026-10-03 用户口径「参照 Peace」）：它搬进一扇独立小窗，
             这页只留一颗按钮当入口。理由：曲线是「一边看一边调」的东西，占满这页会把
             推子与数值表挤没；而这一页要按 Peace 的三栏骨架排。 -->
        <button type="button" class="music-btn music-curve-open" id="music-curve-open">${SVG.wave}${t("music.tuning_curve_open")}</button>
        <button type="button" class="music-switch" id="music-tuning-on" role="switch" aria-checked="false" title="${esc(t("music.tuning"))}">
          <span class="music-switch-knob"></span>
        </button>
      </div>
      <!-- 三栏骨架（2026-10-03，参照 Peace 主窗 → 同日按用户第 7/8 条重排）：
             左 = 工具条 + 我的预设、中 = 三条全局滑块 + 频段柱、右 = 扬声器声道 + 全局效果器。
             **接线函数的 id 一律沿用**（music-tuning-user-presets / music-tune-slots /
             music-tuning-filters / music-preamp-range …）：这一页的 render 与委托全按 id 找节点，
             重排只动 HTML 的位置，逻辑一行都不用改。
             ⚠️ 本段是 JS 模板字符串 ⇒ 注释里不许出现反引号（预检 #15 / #39 ⑩）。 -->
      <div class="music-set-body music-tune-cols">
        <!-- ── 左栏：预设（2026-10-03 用户口径：工具条移到最上、内置 5 档整批删掉）── -->
        <div class="music-tune-col music-tune-col-presets">
          <div class="music-actions">
            <button type="button" class="music-btn" id="music-preset-save">${esc(t("music.tuning_preset_save"))}</button>
            <button type="button" class="music-btn" id="music-preset-import">${esc(t("music.tuning_preset_import"))}</button>
            <button type="button" class="music-btn" id="music-preset-export">${esc(t("music.tuning_preset_export"))}</button>
          </div>
          <div class="music-preset-list" id="music-tuning-user-presets"></div>
          <div class="music-preset-edit hidden" id="music-tuning-preset-edit">
            <div class="music-hint" id="music-preset-edit-hint"></div>
            <input type="text" id="music-preset-name" autocomplete="off" spellcheck="false" />
            <button type="button" class="music-btn" id="music-preset-ok"></button>
            <button type="button" class="music-btn" id="music-preset-cancel">${esc(t("music.tuning_preset_cancel"))}</button>
          </div>
          <!-- A-B 盲测（2026-10-06）：两个快照槽 + 随机映射 + 揭晓。
               快照 = **整条链**（宿主落 config\tuning\ab-a.json / ab-b.json）；
               切换走 player_tuning_ab_apply（先校验→落盘→改内存→版本号+1 ⇒ 不重播）。
               盲测时两个按钮只显示 1 / 2，映射由 tuneAbSlots 决定，点「揭晓」才告诉用户。 -->
          <div class="music-tune-ab" id="music-tune-ab">
            <div class="music-sub-label">${esc(t("music.tuning_ab_title"))}</div>
            <div class="music-actions">
              <button type="button" class="music-btn" id="music-ab-save-a">${esc(t("music.tuning_ab_save_a"))}</button>
              <button type="button" class="music-btn" id="music-ab-save-b">${esc(t("music.tuning_ab_save_b"))}</button>
            </div>
            <div class="music-actions" id="music-ab-play">
              <button type="button" class="music-btn" id="music-ab-pos1" data-pos="1">A</button>
              <button type="button" class="music-btn" id="music-ab-pos2" data-pos="2">B</button>
              <button type="button" class="music-btn" id="music-ab-blind">${esc(t("music.tuning_ab_blind"))}</button>
              <button type="button" class="music-btn hidden" id="music-ab-reveal">${esc(t("music.tuning_ab_reveal"))}</button>
            </div>
            <div class="music-hint" id="music-ab-hint"></div>
          </div>
        </div>
        <!-- ── 中栏：三条全局水平滑块 + 频段柱（2026-10-03 用户口径）────────────
             三条滑块 = 总增益 / 低音增益 / 高音增益（Peace 顶部那种「左标签 + 滑块 + 右数值框」）；
             频段柱 = **滤波器与均衡器合并**后的主体：频率可编辑 / 竖拉条 / 大号增益 / Q / 类型按钮。
             延迟属于全局效果器，已挪到右栏（与声道复制 / 卷积同处）。 -->
        <div class="music-tune-col music-tune-col-eq">
          <div class="music-tune-sliderrow">
            <label>${t("music.tuning_preamp")}</label>
            <input type="range" class="music-tune-hslider" id="music-preamp-range" min="-24" max="24" step="0.5" aria-label="${esc(t("music.tuning_preamp"))}" />
            <input type="text" class="music-tune-num" id="music-tune-preamp" inputmode="decimal" autocomplete="off" spellcheck="false" aria-label="${esc(t("music.tuning_preamp"))}" />
          </div>
          <div class="music-tune-sliderrow">
            <label>${t("music.tuning_bass")}</label>
            <input type="range" class="music-tune-hslider" id="music-bass-range" min="-24" max="24" step="0.5" aria-label="${esc(t("music.tuning_bass"))}" />
            <input type="text" class="music-tune-num" id="music-tune-bass" inputmode="decimal" autocomplete="off" spellcheck="false" aria-label="${esc(t("music.tuning_bass"))}" />
          </div>
          <div class="music-tune-sliderrow">
            <label>${t("music.tuning_treble")}</label>
            <input type="range" class="music-tune-hslider" id="music-treble-range" min="-24" max="24" step="0.5" aria-label="${esc(t("music.tuning_treble"))}" />
            <input type="text" class="music-tune-num" id="music-tune-treble" inputmode="decimal" autocomplete="off" spellcheck="false" aria-label="${esc(t("music.tuning_treble"))}" />
          </div>
          <!-- 频段柱：作用域是**右栏选中的那个通道槽**（全局槽 ↔ 引擎的 filters）。
               左侧那列图例（2026-10-06 用户要求）把「频率 / 增益 / 质量」各标一次，并与柱内
               三行**逐行对齐** —— 三个框都只有数字，少了它用户不知道哪个是哪个。行高与柱子
               共用 --tf-* 那四个变量（见 styles.css 的 .music-tune-filterwrap），改一处两边同步。
               第二格是空占位，对齐的是那根竖拉条（增益那一行同时有拉条与大号读数，只标一次）。 -->
          <div class="music-tune-filterwrap">
            <div class="music-tune-legend" aria-hidden="true">
              <span class="l-freq">${esc(t("music.tuning_freq"))}</span>
              <span class="l-slider"></span>
              <span class="l-gain">${esc(t("music.tuning_gain"))}</span>
              <span class="l-q">${esc(t("music.tuning_q"))}</span>
            </div>
            <div class="music-tune-filters" id="music-tuning-filters"></div>
          </div>
          <div class="music-actions">
            <button type="button" class="music-btn" id="music-tune-add">${SVG.plus}${t("music.tuning_add")}</button>
          </div>
        </div>
        <!-- ── 右栏：通道槽 + 全局效果器（延迟 / 声道复制 / 卷积）──────────────
             三样效果器都是**顶层字段** ⇒ 只在「全局」通道槽里显示（画在通道槽里会看起来像
             只作用于那条声道）。槽本身不落盘 —— 它只是「中栏那排频段柱在编辑哪一份草稿」
             这个视图状态；改动落在草稿里，提交时整排一起发（见 tuneIfElsePayload）。 -->
        <div class="music-tune-col music-tune-col-side">
          <div class="music-sub-label">${t("music.tuning_slots")}</div>
          <div class="music-seg music-slot-list music-slot-col" id="music-tune-slots"></div>
          <div class="music-field hidden" id="music-tune-delay">
            <div class="music-tune-sliderrow">
              <label>${t("music.tuning_delay")}</label>
              <input type="range" class="music-tune-hslider" id="music-delay-range" min="0" max="5000" step="1" aria-label="${esc(t("music.tuning_delay"))}" />
              <input type="text" class="music-tune-num" id="music-delay-num" inputmode="decimal" autocomplete="off" spellcheck="false" aria-label="${esc(t("music.tuning_delay"))}" />
            </div>
          </div>
          <div class="music-field hidden" id="music-tune-copy">
            <div class="music-copy-list" id="music-copy-list"></div>
            <div class="music-actions">
              <button type="button" class="music-btn" id="music-copy-add">${SVG.plus}${t("music.tuning_copy_add")}</button>
            </div>
          </div>
          <div class="music-field hidden" id="music-tune-conv">
            <label>${t("music.tuning_conv")}</label>
            <div class="music-hint" id="music-conv-path"></div>
            <div class="music-actions">
              <button type="button" class="music-btn" id="music-conv-pick">${esc(t("music.tuning_conv_pick"))}</button>
              <button type="button" class="music-btn hidden" id="music-conv-clear">${esc(t("music.tuning_conv_clear"))}</button>
            </div>
          </div>
        </div>
      </div>
    </div>

    <div class="music-msg" id="music-msg"></div>
  </div>`;
}

/** 只在「该节点还没显示这个值」时写 DOM —— 每 1s 一次轮询不能造成闪烁。 */
function setText(el: HTMLElement | null, text: string) {
  if (el && el.textContent !== text) el.textContent = text;
}
/** 只在「内容真的变了」时写 DOM。
 *
 *  ⚠️ 不能直接拿 `el.innerHTML !== html` 当判据：浏览器序列化会改写原文，
 *  最典型的是 void 元素 —— 模板里写的 `<input … />` 读回来是 `<input …>`。
 *  于是「读回来 ≠ 写进去」恒成立 ⇒ 判据恒真 ⇒ 每轮轮询都整块重建，
 *  正在聚焦的 `<input>` 被换成新节点、焦点被踢回 `<body>`，
 *  表现就是「点进输入框的瞬间输入动作被取消」（2026-10-06 实测）。
 *  修法：把待写 html 先塞进一个游离 `<template>` 归一化一次，两边都过同一套
 *  序列化再比；真变了才写。 */
function setHtml(el: HTMLElement | null, html: string) {
  if (!el || el.innerHTML === html) return;
  const probe = document.createElement("template");
  probe.innerHTML = html;
  if (el.innerHTML === probe.innerHTML) return;
  el.innerHTML = html;
}
function show(el: HTMLElement | null, on: boolean) {
  el?.classList.toggle("hidden", !on);
}
function setAttr(el: HTMLElement | null, name: string, v: string) {
  if (el && el.getAttribute(name) !== v) el.setAttribute(name, v);
}

/** 画笔音量条的「已生效」那一段（0–100）。
 *
 *  音量条**没有第二个元素**：填充就是 `.music-vol` 自己背景那条硬停在 `--vol-pct`
 *  上的渐变（见 styles.css 里那段注释）。所以「值变了要重画填充」这件事必须在
 *  每个写 `value` 的地方一起做 —— 收成这一个函数是因为那样的地方有三处
 *  （Spotify 两条 + 本地那一条），各写一遍必然漏掉一处（漏掉的表现是
 *  「音量在响、条子空着」，一个很难被当成 bug 报上来的错）。 */
function paintVolume(el: HTMLInputElement | null, percent: number) {
  if (!el) return;
  const pct = Math.max(0, Math.min(100, Math.round(percent)));
  el.style.setProperty("--vol-pct", `${pct}%`);
}

/** 把挂在 `#music-msg` 上的一次性提示按时抹掉。**每个 tick 调一次**。
 *
 *  **为什么用「看门狗」而不是在每个写入处加定时器**：那个元素有七八处写入方
 *  （控制失败、歌单拉取失败、歌词失败、连接成功提示…），逐处加定时器必然漏一处，
 *  漏掉的那一处就是又一条永久驻留的假状态。这里只统计「同一段文字挂了多久」：
 *  换字即为新提示、时钟归零，于是所有写入方都自动获得 8 秒后消失的行为。
 *  **按真实时间判、不按 tick 计数**：tick 的真实周期是「轮询耗时 + `STATUS_POLL_MS`」
 *  （2026-10-03 起 15000ms、隐藏时干脆停表，见它的注释）⇒ 数 tick 的话 8 个 tick 会变成分钟级（2026-09-27 实测到）。
 *  背景：2026-09-27 用户报「播放的时候同时提示没有可控制的播放设备」——
 *  那句其实是某次控制失败留下的**陈旧**文案，一直没被清掉。 */
function sweepMsg(root: HTMLElement) {
  const el = root.querySelector<HTMLElement>("#music-msg");
  if (!el) return;
  const cur = el.textContent ?? "";
  if (cur !== msgSeen) {
    msgSeen = cur;
    msgAt = Date.now();
    return;
  }
  if (cur && Date.now() - msgAt >= MSG_TTL_MS) {
    el.textContent = "";
    msgSeen = "";
    msgAt = 0;
  }
}

// ── 两态切换 ──────────────────────────────────────────────────────

/** 切形态。**只由用户的操作调用**（用户 2026-09-28 定）：
 *  进 = 底部栏那个展开按钮 / 「正在播放」那一块；
 *  出 = 播放长条上的返回，或标题栏那个 ← （= 停播 + 回默认面板）。
 *  **不进任何「跟着播放状态自动切」的逻辑**：播放开始不自动进播放面板、
 *  暂停 / 切歌也不自动退回歌单列 —— 那套会在换曲那一瞬来回横跳（见预检 #39 ⑤）。 */
function renderMode(root: HTMLElement) {
  const isBar = isBarMode();
  // 五态：`default` / `player` / `lplayer` / `settings` / `tuning`。**非小窗的三态与
  // default 共用默认态尺寸**（1280×720 定尺），所以下面所有判尺寸的地方只认 `isBar`。
  show(root.querySelector("#music-v-default"), mode === "default");
  show(root.querySelector("#music-v-player"), mode === "player");
  show(root.querySelector("#music-v-lplayer"), mode === "lplayer");
  show(root.querySelector("#music-v-settings"), mode === "settings");
  // 调音页（2026-10-01 用户要求独立成页）：只在切到它时刷一次宿主状态 ——
  // 那一页画的是宿主的真值，进来时必须重新拉（见 loadTuning）。
  const tuningNow = mode === "tuning";
  show(root.querySelector("#music-v-tuning"), tuningNow);
  if (tuningNow) {
    void loadTuning(root);
    // 进去就起一条轻轮询：曲线在**另一扇窗**里，用户在那边的拖动要能回到这排频段柱上
    //（2026-10-03 用户报的「反之不行」）。离开这一页就收掉。
    startTunePagePoll(root);
  } else {
    stopTunePagePoll();
  }
  // 播放态：提示行**不占布局**（浮在长条上）—— 用户要求播放态窗口严格 130 高。
  // 用 root 上的类切，比一串 `:has()` 好读也好调（默认态它照样在流里）。
  // 顺带一提：**默认态的外层垫料清不掉**（那条 CSS 只对 `music-player-on` 生效），
  // 窗口尺寸也按态区分，见 `applyResize`。
  root.classList.toggle("music-player-on", isBar);
  // **小窗态禁用缩放**（用户 2026-09-28 定）：定尺长条手动拉一下只会被下一轮 tick
  // 贴回去，关掉它交互才一致。只在态变化时下发一次，别放进每秒那轮 tick。
  syncResizable(isBar);
  // 播放态顺带把标题栏那个 × 换成「← 回退」（同一个位置，两种语义）
  syncTitlebarClose();
  syncHead(root);
  // 三段切换的「哪一段亮着」也跟着这一次切态走 —— 它是**唯一**的入口（工具条上
  // 那两个按钮已删），所以「切过去了但没点亮」会让人以为切换坏了。
  renderTitlebarSwitch();
  if (mode === "player") {
    // 进播放界面时歌词盒刚从 display:none 里出来，几何量为 0 ⇒ 强制重新定位
    activeLine = -1;
    renderLyrics(root);
    syncLyrics(root); // 立刻对齐一次，别等下一秒那轮 tick
  } else if (mode === "lplayer") {
    // 本地小窗：那一块刚显示出来（`.music-pv-card` 里是**自己的**一套节点），
    // 当场画一次 —— 别等下一秒那轮本地轮询，否则会先闪一下空壳。
    renderLocalMini(root);
  } else if (mode === "default") {
    // 默认面板：主体在连上之后才有意义（没登录时设置面板就是全部内容）。
    // **唯一的例外是本地音乐页** —— 它读的是本机磁盘，与 Spotify 登不登录无关。
    // 设置页（`settings`）**刻意不在这条分支里**：它是一个替换掉整块默认视图的
    // 整屏页，底下那些节点此刻全是隐藏的，重画它们纯属白干。
    show(root.querySelector("#music-body"), !!cfg?.connected || localPage);
    renderSide(root);
    renderMain(root);
    renderSearchDD(root);
    renderDevices(root);
  }
  scheduleResize(root);
}

/** 把「能不能缩放」同步给宿主（只在该值真的变了时发一次 IPC）。 */
function syncResizable(disabled: boolean) {
  const want = !disabled;
  if (!document.getElementById("plugin-titlebar")) return; // 内嵌在主窗口时没有这回事
  if (lastResizable === want) return;
  lastResizable = want;
  invoke("plugin_window_set_resizable", { resizable: want }).catch(() => {});
}

/** 播放态：把标题栏上那个 **×** 换成「**← 回退**」（用户 2026-09-28 定）。
 *
 *  **为什么换**：播放态是定尺长条、**缩放已关**（见 `syncResizable`），那个位置上
 *  的 × 会**直接关掉整个悬浮窗** —— 用户的本意通常只是「退出播放界面」，
 *  结果面板整个没了（要重新去主窗口开一次）。改成回退按钮，同样的位置就区分开了：
 *  默认态的 × 真的关窗，播放态的 ← 只退一层。
 *
 *  **图标在 JS 里换、不走 CSS**：那个 × 是 `plugin.html` 里写死的静态 SVG，
 *  用 CSS 藏一半再画一半要赌 `:has()` 与伪元素尺寸；直接换内容才是确定的。
 *  原样记在 `closeBtnOriginal` 里，换回来时**逐字还原**（否则来回切几次会漂）。
 *  点击本身在 `attachMusicListeners` 里以**捕获阶段**拦下 —— 那个按钮的关窗监听器
 *  在 `plugin-window.ts`（所有插件共用），不能为一个插件改它。 */
function syncTitlebarClose() {
  const btn = document.getElementById("plugin-close");
  if (!btn) return; // 内嵌在主窗口里 ⇒ 没有这条标题栏
  if (!closeBtnOriginal) closeBtnOriginal = btn.innerHTML;
  const want = isBarMode() ? "back" : "close";
  if (btn.dataset.mode === want) return;
  btn.dataset.mode = want;
  btn.innerHTML = want === "back" ? SVG.back : closeBtnOriginal;
  const label = want === "back" ? t("music.back") : "Close";
  btn.title = label;
  btn.setAttribute("aria-label", label);
}

/** 离开这个插件时把标题栏按钮还原（窗口会被复用去装别的插件，留着 ← 会串味）。 */
function restoreTitlebarClose() {
  const btn = document.getElementById("plugin-close");
  if (!btn || !closeBtnOriginal || btn.dataset.mode !== "back") return;
  btn.innerHTML = closeBtnOriginal;
  btn.title = "Close";
  btn.setAttribute("aria-label", "Close");
  btn.dataset.mode = "close";
}

// ── 标题栏那枚「三段切换」（2026-10-01 用户口径）──────────────────────
//
// 用户要求：工具条上那两个入口（本地音乐的文件夹、调音的推子）**删掉**，收成一枚
// 「大的切换状态按钮」放进标题栏 —— 也就是**顶掉那段插件名文字**（音乐插件的名字
// 就叫「音乐歌词」，所以用户说的「音乐歌词这个文本」就是它，见 CSS 里
// `body:has(.music-root) #plugin-title { display: none; }` 那条）。
//
// 三段 = 在线歌单 / 本地音乐 / 调音。**前两段不是两个 mode**：本地页是默认视图的
// 一个子态（`localPage`），所以「当前在哪一段」这个状态**不另存一份**，每次从
// `mode + localPage` 现算（`currentSegment`）—— 存一份必然与那两者漂移。

/** 当前在哪一段（`""` = 三段之外：播放态 / 设置页 —— 那时**不点亮任何一段**）。
 *  播放态与设置页不是这三段之一，硬点亮一段只会让用户以为「按了切换却不对」。 */
function currentSegment(): string {
  if (mode === "tuning") return "tuning";
  if (mode !== "default") return "";
  return localPage ? "local" : "online";
}

/** 把三段切换放到位，返回那个节点（`null` = 连退路都没有，理论上不会）。
 *
 *  **放哪儿由「有没有那条标题栏」决定**：悬浮窗里放进 `#plugin-titlebar`
 *  （紧挨着原来那段文字的位置）；插件被嵌在主窗口里时根本没有标题栏
 *  （`#plugin-titlebar` 是 `plugin.html` 独有的），那时退回工具条 —— 不退的话
 *  「本地音乐」就再也进不去了（它原来的入口刚刚被删掉）。 */
function placeSwitcher(root: HTMLElement): HTMLElement | null {
  const existed = document.getElementById("music-tb-switch");
  if (existed) return existed;
  const box = document.createElement("div");
  box.id = "music-tb-switch";
  box.className = "music-tb-switch";
  box.setAttribute("role", "tablist");
  // 它顶掉了原来那段带 `data-tauri-drag-region` 的标题文字 ⇒ **这条拖拽区得补上**，
  // 否则标题栏中间那块（切换器两侧的空隙）就拖不动窗口了。Tauri 只认「鼠标落下的
  // 那个元素自己有没有这个属性」，所以里面的按钮照常可点（与 `#plugin-pin` 那三个
  // 按钮长在同一条 titlebar 上同理）。
  box.setAttribute("data-tauri-drag-region", "");
  const segs: Array<[string, string]> = [
    ["online", t("music.tb_online")],
    ["local", t("music.tb_local")],
    ["tuning", t("music.tb_tuning")],
  ];
  box.innerHTML = segs
    .map(([seg, label]) =>
      `<button type="button" class="music-tb-seg" role="tab" data-seg="${seg}" title="${esc(label)}">${esc(label)}</button>`)
    .join("");

  const bar = document.getElementById("plugin-titlebar");
  const title = document.getElementById("plugin-title");
  if (bar && title) {
    bar.insertBefore(box, title.nextSibling);
  } else {
    // 内嵌形态：塞在工具条右端那两个按钮**之前**（状态行的右边、齿轮的左边）
    const gear = root.querySelector("#music-setup-toggle");
    const toolbar = root.querySelector(".music-toolbar");
    if (!toolbar) return null;
    toolbar.insertBefore(box, gear);
  }
  return box;
}

/** 只画状态（幂等）——`renderMode` 每次都会调它，所以「切到哪一段」与「哪一段亮着」
 *  永远同源。**不在这里查 `#plugin-titlebar`**：切换器可能长在工具条上（内嵌形态），
 *  直接按 id 找那一个节点即可。 */
function renderTitlebarSwitch() {
  const box = document.getElementById("music-tb-switch");
  if (!box) return;
  const cur = currentSegment();
  for (const btn of box.querySelectorAll<HTMLElement>(".music-tb-seg")) {
    setAttr(btn, "aria-pressed", String(btn.dataset.seg === cur));
  }
}

/** 离开这个插件时把它摘掉：悬浮窗会被复用去装别的插件，留着会串味
 *  （与 `restoreTitlebarClose` 同一条理由）。 */
function removeSwitcher() {
  document.getElementById("music-tb-switch")?.remove();
}

/** 播放态那个「← 」按下时做的事：**停播 + 回默认面板**（用户 2026-09-28 定）。
 *
 *  `pause` 而不是 `stop`：Spotify Web API **根本没有 stop**（宿主 `music.rs` 的
 *  action 白名单里就没有它），pause 之后窗口里那个「播放」按钮还能一键续上。
 *  因为**没有任何自动切态的逻辑**（用户同一天把「在播就进播放面板」也取消了），
 *  这里不必再记「退掉的是哪一首」—— 退回去它就停在默认面板上，不会被弹回来。 */
async function stopAndBack(root: HTMLElement) {
  try {
    await invoke("spotify_control", { action: "pause" });
  } catch (e) {
    note(root, errText(e));
  }
  mode = "default";
  renderMode(root);
}

/** 本地页里那枚**设置齿轮**：从工具条**搬到**主区那一排、落在「重扫」右边
 *  （2026-10-01 用户口径：「本地音乐的设置按钮放到扫描按钮的右边」）。
 *
 *  **搬的是同一个节点**，不是再造一枚：`#music-setup-toggle` 的点击监听在 attach 里
 *  按 id 绑死，而 `.music-local-head-row` 属于动态骨架（切回 Spotify 时 `#music-main`
 *  会被整个重写、回来时重建）—— 在里面另造一枚就只能改成委托，白多一条路。
 *  工具条那边因此会空出来：它本来在本地页就只剩这一枚，其余（头像 / 账号 / 搜索 / 状态）
 *  已被 `syncHead` 收掉 ⇒ 整条工具条塌成 0 高（它没有自己的 padding），不留空条。
 *
 *  ⚠️ **离开本地页时必须先搬回工具条**：`renderMode` 里 `syncHead` 排在 `renderMain`
 *  之前，所以那一刀在 `#music-main` 被重写之前就落下了。顺序反过来的话，齿轮会连同骨架
 *  一起被销毁 —— 表现是「切一次页签，设置按钮就没了」（而且不会报任何错）。 */
function placeGear(root: HTMLElement) {
  const gear = root.querySelector<HTMLElement>("#music-setup-toggle");
  const bar = root.querySelector<HTMLElement>(".music-toolbar");
  if (!gear || !bar) return;
  const row = root.querySelector<HTMLElement>(".music-local-head-row");
  const home = localPage && row ? row : bar;
  // 幂等：`syncHead` 每轮 tick 都会调它，父节点没变就什么都不做（appendChild 会真动 DOM）。
  if (gear.parentElement !== home) home.appendChild(gear);
}

/** 左端那枚抽屉 + 栅格列数**一起切**（2026-10-03）。
 *  栅格模板按「有没有第一列」写死的（`.music-nowbar` = 4 列含抽屉，`.no-drawer` = 3 列）
 *  —— 只切其中一个，三个在流里的元素（正在播放 / 播放键组 / 右端）就会挤错列。 */
function setDrawer(root: HTMLElement, barSel: string, drawerSel: string, on: boolean) {
  const bar = root.querySelector<HTMLElement>(barSel);
  if (bar) bar.classList.toggle("no-drawer", !on);
  show(root.querySelector<HTMLElement>(drawerSel), on);
}

/** 抽屉上的字指的是**另一侧**（在线栏上写「本地」，本地栏上写「在线」）。
 *  `dataset.label` 去重：`syncHead` 每秒跑一次，别每秒重写一遍 innerHTML。 */
function paintDrawer(btn: HTMLElement | null, labelKey: string, titleKey: string) {
  if (!btn) return;
  const label = t(labelKey);
  if (btn.dataset.label !== label) {
    btn.dataset.label = label;
    btn.innerHTML = `${SVG.expand}<span>${esc(label)}</span>`;
  }
  setAttr(btn, "title", t(titleKey));
}

/** 头顶那两个「进 / 出播放界面」按钮。每 tick 都会调（`show()` 幂等且只碰 class）——
 *  播放状态变了要能立刻反映，不能只等 `renderMode()` 那次。
 *  同时**在这里管底部控制栏的显隐**：没连上 Spotify 时那条栏没有意义（那时面板上
 *  只有连接设置），连上之后才出现 —— 设备按钮长在那条栏里，藏掉它就等于没法换设备。 */
function syncHead(root: HTMLElement) {
  const isPlayer = mode === "player";
  const onlineOk = !!cfg?.connected;
  // 底部控制栏是 Spotify 控制器 —— 本地音乐页里它没有对象（那一层不含播放），收掉。
  // **「进入小窗播放」的入口不在这里管**（2026-10-03）：它已收进左边「正在播放」
  // 那一块（那块永远跟着这条栏一起显隐），不再是一枚独立按钮。
  // 显隐用 `localBarVisible()`（与 `renderLocalNowbar` 同源）—— 左端那枚抽屉可以把
  // 这一条换出去，判据只此一处（见 barSwap）。
  show(root.querySelector("#music-nowbar"), !isPlayer && onlineOk && !localBarVisible());
  // 左端那枚「换到另一侧」的抽屉：**只在「同时出声」开着、且另一侧确实有东西时**给。
  // 关掉这把开关时另一侧早被停掉了（见 enforceExclusive），切过去只会是一条空栏。
  const canDrawer = dualPlay && onlineOk && localQueueIndex >= 0;
  setDrawer(root, "#music-nowbar", "#music-b-drawer", canDrawer);
  setDrawer(root, "#music-lnowbar", "#music-l-drawer", canDrawer);
  paintDrawer(root.querySelector("#music-b-drawer"), "music.drawer_local", "music.tb_local");
  paintDrawer(root.querySelector("#music-l-drawer"), "music.drawer_online", "music.tb_online");
  // 工具条上这几样是 **Spotify 那一侧专用的**（账号头像 / 账号名 / 搜索框 / 连接状态）
  // —— 用户 2026-10-01 的口径：「那一行是 Spotify 页面专用，本地播放界面不要」。
  // 本地音乐页读的是本机磁盘，与登没登录无关，带着它们只会让人以为「本地播放也要账号」。
  // 齿轮**在本地页不留在工具条**（2026-10-01 用户口径：本地页那枚要挪到主区的「重扫」
  // 右边，见 placeGear）—— 所以本地页整条工具条是空的、塌成 0 高。同一个节点两边通用，
  // 所以「两页指向同一页设置」仍然是天然的。原来那两枚入口按钮已删，两个入口收进了
  // 标题栏的三段切换（见 placeSwitcher）—— 那枚切换器与这四个节点互不干涉：
  // 它在标题栏里（悬浮窗），内嵌形态才退到工具条，且不受这里的显隐影响。
  for (const sel of [".music-search-wrap", "#music-avatar", "#music-acc", "#music-status"]) {
    show(root.querySelector<HTMLElement>(sel), !localPage);
  }
  // 齿轮的落点跟着 localPage 走（工具条 ⇄ 主区那一排）。**放在这里而不是各渲染函数里**：
  // syncHead 每秒都在跑，且排在 renderMain（会重写 #music-main）**之前** —— 见 placeGear 的注释。
  placeGear(root);
  // 「同时出声」那把开关的亮灭。**两枚按钮一起刷**（它们共用 `dualPlay`）——
  // `syncHead` 每秒都在跑，所以本地窗里点了之后，切回 Spotify 那条栏也是对的。
  root.querySelectorAll<HTMLElement>(".music-dual-btn")
    .forEach(b => b.classList.toggle("on", dualPlay));
}

// ── 渲染：状态行 / 设置 ───────────────────────────────────────────

function renderStatus(root: HTMLElement, msg?: string) {
  const st = root.querySelector<HTMLElement>("#music-status");
  if (msg) {
    setText(st, msg);
    return;
  }
  if (!cfg?.client_id) {
    setText(st, t("music.status_unconfigured"));
  } else if (!cfg.connected) {
    setText(st, t("music.status_disconnected"));
  } else if (!player) {
    // **「还没问过」与「问了、没有设备」是两件事**：`player` 为 null 只说明轮询
    // 还没跑完第一轮（或窗口刚打开），这时说「无活跃设备」是冤枉 —— 2026-09-27
    // 实测就是这个观感：窗一开先报一句「没有可控制的播放设备」，1 秒后才翻成正常。
    setText(st, t("music.status_connecting"));
  } else if (!player.active) {
    setText(st, t("music.status_no_device"));
  } else if (!player.playing) {
    setText(st, t("music.status_paused"));
  } else {
    setText(st, t("music.status_playing"));
  }
  // 顶部工具条：头像 + 账号名（未连接时都空着）。
  // 头像直接用远端 URL —— `img-src` 放行了 `https:`（CSP 只挡 `fetch`），
  // 不必为它加一条宿主命令。
  const av = root.querySelector<HTMLElement>("#music-avatar");
  if (av) {
    const url = cfg?.connected ? (cfg.avatar || "") : "";
    setHtml(av, url ? `<img src="${esc(url)}" alt="">` : "");
  }
  setText(root.querySelector("#music-acc"), cfg?.connected ? cfg.display_name || "" : "");
  // 设备名**不再单独占一行**（用户 2026-09-28 要求把它换成工具条上的设备按钮 ——
  // 「面板上不该有一条只能看、不能点的状态」）。信息不丢：挂在按钮的 title 上，
  // 并且没有活跃设备时给按钮加一个 `warn` 态（那正是用户最需要知道的时候）。
  const devBtn = root.querySelector<HTMLElement>("#music-devices-btn");
  if (devBtn) {
    setAttr(devBtn, "title", `${t("music.devices")}：${player?.device || t("music.device_none")}`);
    devBtn.classList.toggle("warn", !!cfg?.connected && !player?.active);
  }
}

function renderSetup(root: HTMLElement) {
  // 设置面板属于**默认界面**：连上之后主体（左栏 + 主区）才有内容。断开时 renderMode
  // 会把形态拉回 default，所以这里不会有「播放界面里突然弹出凭据字段」的情况。
  // 未连接 ⇒ 无条件摊着（那是这一步唯一要做的事）；已连接 ⇒ 只看用户点过头像没有。
  show(root.querySelector("#music-setup"), !cfg?.connected || setupOpen);
  // 主体与工具条按钮的显隐跟「连没连」走（**本地音乐页例外**，见 renderMode）；
  // 两栏的内容由 renderSide / renderMain 管
  show(root.querySelector("#music-body"), !!cfg?.connected || localPage);
  renderSide(root);
  renderMain(root);
  // 左栏当前那一类 + 收藏夹（收藏夹合成在歌单栏第一行，两处都要有数据）。
  // **放在这里而不是挂载处**：`refreshConfig` 会在「首次打开 / 登录成功 / 令牌失效回落」
  // 三条路径上都走到这里，一处接上就三条都覆盖（两侧的加载函数自己判重）。
  if (cfg?.connected) {
    void loadSide(root, sideTab);
    void loadLiked(root);
  }
  // 凭据字段：**只在真的用内置 Client ID 时**才默认收起来（那时用户只需点「连接」，
  // Redirect URI 那一串是噪音）。自己填 Client ID 的用户照旧看见这组字段。
  const showFields = fieldsOverride ?? !cfg?.builtin;
  show(root.querySelector("#music-setup-fields"), showFields);
  // 「保存」只在字段可见时才有意义（保存的就是那几个输入框）
  show(root.querySelector("#music-save"), showFields);
  const toggle = root.querySelector<HTMLElement>("#music-fields-toggle");
  show(toggle, !!cfg?.builtin);
  setText(toggle, showFields ? t("music.hide_fields") : t("music.use_own_client_id"));
  // 提示语按「用的是不是内置值」选：内置值那条是普通用户视角，自填那条才需要讲
  // developer.spotify.com 与 Redirect URI（见 i18n.ts 两段文案）。
  setText(
    root.querySelector("#music-setup-hint"),
    cfg?.builtin ? t("music.setup_builtin") : t("music.setup_hint"),
  );
  const idEl = root.querySelector<HTMLInputElement>("#music-client-id");
  const portEl = root.querySelector<HTMLInputElement>("#music-port");
  // 本机播放那两项：**留空就是「自动探测 / 直连」**，所以不校验、不拦保存。
  const lpEl = root.querySelector<HTMLInputElement>("#music-librespot");
  const pxEl = root.querySelector<HTMLInputElement>("#music-librespot-proxy");
  // 输入框里有内容时不要被轮询覆盖（用户可能正在敲）
  if (idEl && document.activeElement !== idEl) idEl.value = cfg?.client_id ?? "";
  // 兜底值必须与宿主 `music.rs` 的 `DEFAULT_PORT` 一致（8899），否则面板显示的
  // 回调地址与真正监听的那个端口对不上（配置读不到时才会走到这里）。
  if (portEl && document.activeElement !== portEl) portEl.value = String(cfg?.port ?? 8899);
  if (lpEl && document.activeElement !== lpEl) lpEl.value = cfg?.librespot_path ?? "";
  if (pxEl && document.activeElement !== pxEl) pxEl.value = cfg?.librespot_proxy ?? "";
  // 串流质量：显示「刚点过还没保存」的那一档，否则显示配置里的（宿主已兜过底，不会是 0）
  const curQ = bitrateOverride ?? cfg?.librespot_bitrate ?? 320;
  for (const btn of root.querySelectorAll<HTMLElement>("#music-quality .music-seg-btn")) {
    setAttr(btn, "aria-pressed", String(Number(btn.dataset.q) === curQ));
  }
  setText(root.querySelector("#music-redirect"), cfg?.redirect_uri ?? "");
}

// ── 调音（2026-10-01 用户第 5 条）───────────────────────────────────

/** 一个滤波器类型的显示名。**键与引擎 `Kind::name()` 同源**，同「认不出就回退成键本身」
 *  的口径（将来引擎加了类型而 i18n 没跟上时，界面显示 `low_shelf` 而不是空白）。 */
function tuningKindLabel(name: string): string {
  const label = t(`music.tk_${name}`);
  return label.startsWith("music.") ? name : label;
}

/** 夹取（拖动时把像素反算出来的值收进合法区间）。
 *  频率收在 20Hz–20kHz：上界远低于任何常见采样率的 Nyquist（44.1k 是 22050），
 *  否则引擎会以「freq_hz 必须落在 (0, Nyquist)」把**整条链**退回来 —— 一次越界
 *  会让用户刚拖到的位置全部回滚，很难理解。 */
function tuneClampFreq(f: number): number {
  if (!Number.isFinite(f)) return 1000;
  return Math.min(20000, Math.max(20, f));
}
function tuneClampGain(g: number): number {
  if (!Number.isFinite(g)) return 0;
  return Math.min(24, Math.max(-24, g));
}

/** 数字的显示形态：最多 `digits` 位小数，尾随的 0 自然去掉（`String(120.0)` = `"120"`）。
 *  `Number.isFinite` 挡住 NaN / Infinity —— 那两种值写进输入框就是一段看不懂的文本。 */
function fmtTuneNum(v: number | undefined, digits = 3): string {
  if (v === undefined || !Number.isFinite(v)) return "";
  return String(Math.round(v * 10 ** digits) / 10 ** digits);
}

/** 某一字段在草稿里的显示值（`change` 里判「值真的变了吗」与「非法输入回滚」共用）。 */
function tuneDraftField(f: TuningFilter, field: string): string {
  if (field === "freq_hz") return fmtTuneNum(f.freq_hz, 1);
  if (field === "gain_db") return fmtTuneNum(f.gain_db ?? 0, 1);
  if (field === "q") return fmtTuneNum(f.q ?? 0.707, 3);
  return "";
}

/** 拉一次调音状态。**失败不改 `tuning`**（保持上一次的真值，别把界面清成空白）。 */
async function loadTuning(root: HTMLElement) {
  let next: TuningDto;
  try {
    next = await invoke<TuningDto>("player_tuning_get");
  } catch (e) {
    note(root, errText(e));
    return;
  }
  // **只在宿主真值真的变了时才重铺草稿**：`renderMode` 在别的原因下也会走到这里
  // （窗口尺寸 / 三段切换…），无脑重铺会把用户正在输入框里改、还没提交的值冲掉
  // —— 而「输入到一半被自己的程序改回去」是最让人恼火的一类行为。
  if (JSON.stringify(next) !== JSON.stringify(tuning)) {
    tuning = next;
    syncTuningDraft();
    tuneOpen = -1;
  }
  await refreshTuningAb();
  renderTuning(root);
}

/** 画「测量」那一块（按钮可用性 + 结果摘要）。**幂等**。
 *
 *  摘要说四件事：**覆盖率**（量到了多宽的频段）、**仅自身链与合成曲线的平均偏差**
 *  （调音到底有没有生效 —— 这是唯一该拿来下判断的那一对）、**系统另有几 dB 平均影响**
 *  （EAPO / 别的软件那部分，只报数不评判），以及耗时。刻意**不做「好 / 坏」评判** ——
 *  音箱本身的频响起伏是正常现象，这台工具只负责把几条线摆在一起，不替用户下结论。 */
function renderProbe(root: HTMLElement) {
  const btn = root.querySelector<HTMLButtonElement>("#music-tune-measure");
  if (btn && btn.disabled !== tuneProbing) btn.disabled = tuneProbing;
  setAttr(btn, "aria-busy", String(tuneProbing));
  const hint = root.querySelector<HTMLElement>("#music-tune-probe-hint");
  if (!hint) return;
  if (tuneProbing) {
    setText(hint, t("music.tuning_measuring"));
    return;
  }
  const m = tuneMeasured;
  if (!m) {
    setText(hint, t("music.tuning_probe_hint"));
    return;
  }
  // **判据是「仅自身链 vs 合成」** —— 拿「最终输出」去比会被系统音效污染，而那正是
  // 两趟测量要分开的东西。`sys` 单独报：系统（EAPO / 别的软件）对这条链的平均影响。
  const mean = (a: number[], b: number[]) => {
    let sum = 0;
    let n = 0;
    for (let i = 0; i < a.length && i < b.length; i++) {
      sum += a[i] - b[i];
      n += 1;
    }
    return n > 0 ? sum / n : 0;
  };
  const dev = mean(m.chain_db ?? [], m.ref_db);
  const sys = mean(m.db, m.chain_db ?? []);
  setText(
    hint,
    t("music.tuning_probe_result", {
      cov: String(Math.round(m.coverage * 100)),
      dev: `${dev >= 0 ? "+" : ""}${dev.toFixed(1)}`,
      sys: `${sys >= 0 ? "+" : ""}${sys.toFixed(1)}`,
      ms: String(m.elapsed_ms),
    }),
  );
}

/** 画**预设那一块**（工具条 + 我的预设 + 命名行）。**幂等**（P5-1）。
 *
 *  状态一律从宿主的 DTO 派生（`user_presets`），前端**不维护第二份**清单 ——
 *  与「名单只在宿主这一份」那条纪律一致。命名行走 `tunePresetEdit`（WebView2 没有
 *  `window.prompt`，只能在页面里输入，见那个类型的注释）。
 *  2026-10-03 用户口径：内置那 5 档整批删掉、工具条移到顶部 —— 所以这里不再画内置列表。 */
function renderTuningPresets(root: HTMLElement) {
  if (!tuning) return;
  // 我的预设：一行 = 用它 + 重命名 + 删除
  const user = tuning.user_presets ?? [];
  const ub = root.querySelector<HTMLElement>("#music-tuning-user-presets");
  if (ub) {
    setHtml(
      ub,
      user
        .map((n) => {
          const on = tuning!.preset === n;
          return (
            `<div class="music-preset-user" data-preset="${esc(n)}" aria-pressed="${on}">` +
            `<button type="button" class="music-preset-item" data-act="use">${esc(n)}</button>` +
            `<button type="button" class="music-preset-icon" data-act="rename" title="${esc(t("music.tuning_preset_rename"))}">✎</button>` +
            `<button type="button" class="music-preset-icon" data-act="delete" title="${esc(t("music.tuning_preset_delete"))}">✕</button>` +
            `</div>`
          );
        })
        .join(""),
    );
  }
  // 命名行（保存 / 重命名 / 删除确认三态共用）
  const edit = tunePresetEdit;
  show(root.querySelector("#music-tuning-preset-edit"), edit !== null);
  if (!edit) return;
  const isDelete = edit.mode === "delete";
  setText(
    root.querySelector("#music-preset-edit-hint"),
    edit.mode === "save"
      ? t("music.tuning_preset_save_prompt")
      : edit.mode === "rename"
        ? t("music.tuning_preset_rename_prompt", { name: edit.from })
        : t("music.tuning_preset_delete_confirm", { name: edit.name }),
  );
  const input = root.querySelector<HTMLInputElement>("#music-preset-name");
  show(input, !isDelete);
  // 聚焦时不覆盖用户正在敲的字（同 `#music-tune-preamp` 那一族的口径）
  if (input && input !== document.activeElement && input.value !== edit.name) input.value = edit.name;
  setText(
    root.querySelector("#music-preset-ok"),
    edit.mode === "save"
      ? edit.overwrite
        ? t("music.tuning_preset_overwrite")
        : t("music.tuning_preset_ok")
      : edit.mode === "rename"
        ? t("music.tuning_preset_rename")
        : t("music.tuning_preset_delete"),
  );
}

/** 槽的显示名（`0` = 全局，`k` = 通道 `k-1`）。 */
function tuneSlotName(k: number): string {
  return k === 0 ? t("music.tuning_slot_global") : CHANNEL_NAMES[k - 1] ?? `#${k}`;
}

/** 画**通道槽选择器**（P5-2）。`aria-pressed` 是唯一的选中标志；槽名用 `CHANNEL_NAMES`
 *  （Peace / 7.1 那套顺序），后面那个小数字是**这条槽里开着的段数** —— 一眼能看出
 *  「哪条声道已经调过」，不必逐个点进去翻。 */
function renderTuneSlots(root: HTMLElement) {
  const box = root.querySelector<HTMLElement>("#music-tune-slots");
  if (box) {
    setHtml(
      box,
      tuneSlotDrafts
        .map((fs, k) => {
          const label = tuneSlotName(k);
          const on = fs.filter((f) => f.on).length;
          return `<button type="button" class="music-seg-btn music-slot-btn" data-slot="${k}" aria-pressed="${k === tuneSlot}" title="${esc(label)}">${esc(label)}<em>${on}</em></button>`;
        })
        .join(""),
    );
  }
}

/** 换槽（P5-2）。**只是换「表在编辑哪一份草稿」** —— 槽是视图状态，不进配置、不落盘，
 *  所以切槽**不需要先提交**（三份草稿一直并存着，提交时一起发，见 `tuneIfElsePayload`）。
 *  下拉 / 选中段一并收起：它们记的是行号，换了一份表就对不上号了。 */
function selectTuneSlot(root: HTMLElement, k: number) {
  if (k === tuneSlot || k < 0 || k >= tuneSlotDrafts.length) return;
  tuneSlot = k;
  tuneFilters = tuneSlotDrafts[k];
  tuneOpen = -1;
  renderTuning(root);
}

/** 声道下标的显示名（0 起）。超出那张表（>8 声道）用 `#n`（1 起）兜底 —— 纯标识，不入 i18n。 */
function channelName(i: number): string {
  return CHANNEL_NAMES[i] ?? "#" + (i + 1);
}

/** 画**声道复制**那一块（P5-4）。**幂等**。
 *
 *  每行 = 「源 → 目标」两个按钮 + 删除；按钮点开是一条**声道菜单**（当前流的每个声道一颗
 *  按钮）。只让用户在**流内**选 —— 越界在引擎里是 `ChainRuntime::new` 的硬错误，会让
 *  **整条链**编不出来（见 `TuningCopy` 的注释）。菜单开合是纯视图状态（`tuneCopyMenu`）。 */
function renderTuneCopy(root: HTMLElement) {
  const box = root.querySelector<HTMLElement>("#music-copy-list");
  if (!box) return;
  const n = Math.max(1, tuning?.channels || 2);
  setHtml(
    box,
    tuneCopy
      .map((c, i) => {
        const openSide = tuneCopyMenu?.row === i ? tuneCopyMenu.side : null;
        const menuFor = (side: "from" | "to") =>
          openSide === side
            ? `<div class="music-tune-menu">${Array.from({ length: n }, (_, k) => {
                const on = (side === "from" ? c.from : c.to) === k;
                return `<button type="button" class="music-seg-btn" data-act="pick" data-i="${i}" data-side="${side}" data-ch="${k}" aria-pressed="${on}">${esc(channelName(k))}</button>`;
              }).join("")}</div>`
            : "";
        const cell = (side: "from" | "to", v: number) =>
          `<button type="button" class="music-tune-kind" data-act="menu" data-i="${i}" data-side="${side}" aria-expanded="${openSide === side}"><span>${esc(channelName(v))}</span>${SVG.chevron}</button>`;
        return `<div class="music-copy-row" data-i="${i}">
        ${cell("from", c.from)}<span class="music-copy-arrow">→</span>${cell("to", c.to)}
        <button type="button" class="music-tune-del" data-act="del" data-i="${i}" title="${esc(t("music.tuning_del"))}">${SVG.trash}</button>
        ${menuFor("from")}${menuFor("to")}
      </div>`;
      })
      .join(""),
  );
  show(root.querySelector("#music-copy-empty"), tuneCopy.length === 0);
}

/** 画**全局效果器**那三块（延迟 / 声道复制 / 卷积，P5-4）。**幂等**。
 *  三样都是**顶层字段** ⇒ 只在「全局」通道槽里显示（同图示均衡器那条纪律）。 */
function renderTuneFx(root: HTMLElement) {
  const global = tuneSlot === 0;
  show(root.querySelector("#music-tune-delay"), global);
  show(root.querySelector("#music-tune-copy"), global);
  show(root.querySelector("#music-tune-conv"), global);
  if (!global || tuneHold) return;
  // 延迟：数字框**聚焦时不写**（用户正在输的字不该被重绘覆盖，同预增益那条口径）
  const num = root.querySelector<HTMLInputElement>("#music-delay-num");
  if (num && document.activeElement !== num && num.value !== fmtTuneNum(tuneDelay, 1)) {
    num.value = fmtTuneNum(tuneDelay, 1);
  }
  const rng = root.querySelector<HTMLInputElement>("#music-delay-range");
  if (rng && rng.value !== String(tuneDelay)) rng.value = String(tuneDelay);
  renderTuneCopy(root);
  // 卷积：显示当前 IR 路径（没有就如实说「没启用」）
  setText(root.querySelector("#music-conv-path"), tuneConv ?? t("music.tuning_conv_none"));
  show(root.querySelector("#music-conv-clear"), tuneConv !== null);
}

/** 一条「左标签 + 滑块 + 右数值框」的同步（预增益 / 低音 / 高音共用）。
 *  滑块直接写（没有焦点问题）；数值框**聚焦时不写** —— 用户正在输的字不该被重绘覆盖。
 *  少写滑块那一句，改预设之后它会停在上一个值上，看起来像「预设没生效」。 */
function syncTuneSlider(root: HTMLElement, rangeSel: string, numSel: string, v: number) {
  const range = root.querySelector<HTMLInputElement>(rangeSel);
  if (range && range.value !== fmtTuneNum(v, 1)) range.value = fmtTuneNum(v, 1);
  const num = root.querySelector<HTMLInputElement>(numSel);
  if (num && document.activeElement !== num && num.value !== fmtTuneNum(v, 1)) {
    num.value = fmtTuneNum(v, 1);
  }
}

// ── A-B 盲测（2026-10-06）──────────────────────────────────────────

/** 拉一次两槽的可用性（宿主只查文件在不在）。 */
async function refreshTuningAb() {
  try {
    tuneAbHas = await invoke<{ a: boolean; b: boolean }>("player_tuning_ab_state");
  } catch {
    // 宿主查不了就按「都没有」画（按钮置灰，不会误点）
    tuneAbHas = { a: false, b: false };
  }
}

/** 画 A-B 那一块。**幂等**：只改标签 / 置灰 / 高亮，不重建节点。 */
function renderTuningAb(root: HTMLElement) {
  for (const pos of [1, 2] as const) {
    const btn = root.querySelector<HTMLButtonElement>(`#music-ab-pos${pos}`);
    if (!btn) continue;
    const slot = tuneAbSlots[pos - 1];
    const has = slot === "a" ? tuneAbHas.a : tuneAbHas.b;
    btn.dataset.ab = slot;
    btn.textContent = tuneAbBlind ? String(pos) : slot.toUpperCase();
    btn.disabled = !has;
    btn.classList.toggle("active", has && tuneAbActive === slot);
  }
  const blindBtn = root.querySelector<HTMLButtonElement>("#music-ab-blind");
  if (blindBtn) {
    blindBtn.classList.toggle("active", tuneAbBlind);
    // 两槽都有快照才谈得上盲测（只有一个可切的时候，藏起来没意义）
    blindBtn.disabled = !(tuneAbHas.a && tuneAbHas.b);
  }
  root.querySelector("#music-ab-reveal")?.classList.toggle("hidden", !tuneAbBlind || tuneAbRevealed);
  const hint = root.querySelector<HTMLElement>("#music-ab-hint");
  if (hint) hint.textContent = tuneAbHintText();
}

function tuneAbHintText(): string {
  if (!tuneAbHas.a && !tuneAbHas.b) return t("music.tuning_ab_empty");
  if (!tuneAbBlind) {
    return t("music.tuning_ab_hint", {
      a: tuneAbHas.a ? "A" : "—",
      b: tuneAbHas.b ? "B" : "—",
    });
  }
  if (!tuneAbRevealed) return t("music.tuning_ab_blind_hint");
  return t("music.tuning_ab_revealed", {
    one: tuneAbSlots[0].toUpperCase(),
    two: tuneAbSlots[1].toUpperCase(),
  });
}

/** 把**当前这条链**快照进某个槽。 */
async function tuningAbSave(view: HTMLElement, slot: "a" | "b") {
  try {
    tuneAbHas = await invoke<{ a: boolean; b: boolean }>("player_tuning_ab_save", { slot });
    note(view, t(slot === "a" ? "music.tuning_ab_saved_a" : "music.tuning_ab_saved_b"));
  } catch (e) {
    note(view, errText(e));
  }
  renderTuningAb(view);
}

/** 切到某个位置对应的槽（盲测时「位置 → 槽」的映射是乱的）。 */
async function tuningAbApply(view: HTMLElement, pos: number) {
  const slot = tuneAbSlots[pos - 1];
  if (!slot) return;
  try {
    await invoke("player_tuning_ab_apply", { slot });
    tuneAbActive = slot;
    await loadTuning(view); // 链变了 ⇒ 把草稿同步成宿主真值
  } catch (e) {
    note(view, errText(e));
  }
  renderTuningAb(view);
}

/** 画调音页。**幂等**（与其余 render 同一口径：只在值变了时写 DOM）。
 *  名单全部来自宿主：类型 = `tuning.kinds`、曲线 = `tuning.response`、用户预设 =
 *  `tuning.user_presets` —— 前端**不维护第二份**（也不自己算曲线）。 */
function renderTuning(root: HTMLElement) {
  if (!tuning) return;
  // 总开关是**切换按钮**（role="switch"）：状态只有 `aria-checked` 一处，
  // 不再有「两个按钮谁亮着」这套需要同步的中间态。
  setAttr(root.querySelector("#music-tuning-on"), "aria-checked", String(tuning.enabled));
  renderProbe(root);
  renderTuningPresets(root);
  renderTuningAb(root);
  // 三条全局水平滑块（总增益 / 低音增益 / 高音增益）
  syncTuneSlider(root, "#music-preamp-range", "#music-tune-preamp", tunePreamp);
  syncTuneSlider(root, "#music-bass-range", "#music-tune-bass", tuneBass);
  syncTuneSlider(root, "#music-treble-range", "#music-tune-treble", tuneTreble);
  renderTuneSlots(root);
  renderTuningFilters(root);
  renderTuneFx(root);
  // 走 `applyTunePlotZoom`（而不是直接 draw）：把当前倍率写回画布宽度与百分比标签，
  // 重开插件窗（新 DOM）后标签与画布不会对不上。
  applyTunePlotZoom(root);
  setText(
    root.querySelector("#music-tune-rate"),
    t("music.tuning_rate", { rate: fmtTuneNum(tuning.response.sample_rate / 1000, 1) }),
  );
}

/** 一根**频段柱**（2026-10-03 用户口径「滤波器与均衡器合并」）：
 *  上→下 = 频率（可编辑）/ 增益竖拉条 / 大号增益 / Q / 类型按钮（+ 开关 + 删除）。
 *
 *  与旧的两套界面**共用同一组 `data-i` / `data-f` / `data-act`** —— 于是
 *  `#music-tuning-filters` 上那三条委托（click / input / change）与 `commitTuningChain`
 *  **一行都不用改**。⚠️ 一格里 `gain_db` 有两个控件（拉条 + 大号数字框），
 *  所以「还原焦点」必须按 `input[type="text"][data-f=…]` 找（见 commitTuningChain）。
 *  ⚠️ 本段是 JS 模板字符串 ⇒ 注释里不许出现反引号。 */
function tuningBandHtml(f: TuningFilter, i: number, kinds: TuningKind[], open: boolean): string {
  const meta = kinds.find((k) => k.name === f.kind);
  // 认不出的类型（宿主名单与盘上配置漂移）也照画：两个输入框按「配置里有没有那个字段」
  // 决定显隐 —— 否则用户面对一条本来就坏了的段，连改都改不动。
  const useGain = meta ? meta.gain : f.gain_db !== undefined;
  const useQ = meta ? meta.order !== 1 : f.q !== undefined;
  const gainVal = f.gain_db ?? 0;
  const freqIn = `<input type="text" class="music-tune-num music-band-freq" data-f="freq_hz" inputmode="decimal" autocomplete="off" spellcheck="false" value="${esc(fmtTuneNum(f.freq_hz, 1))}" aria-label="${esc(t("music.tuning_freq"))}" />`;
  // 拉条适合「一边听一边扫」，大号数字框适合「输一个准确值」——两者改同一个字段，
  // 所以共用 data-f（提交走同一条 change 委托）。范围与引擎判据对齐（收到 ±24）。
  const slider = useGain
    ? `<input type="range" class="music-tune-vslider" data-f="gain_db" min="-24" max="24" step="0.5" value="${esc(fmtTuneNum(gainVal, 1))}" aria-label="${esc(t("music.tuning_gain"))}" />`
    : `<div class="music-tune-vslider-na"></div>`;
  const gainIn = useGain
    ? `<input type="text" class="music-tune-num music-band-gain" data-f="gain_db" inputmode="decimal" autocomplete="off" spellcheck="false" value="${esc(fmtTuneNum(gainVal, 1))}" aria-label="${esc(t("music.tuning_gain"))}" />`
    : `<span class="music-band-gain na">—</span>`;
  const qIn = useQ
    ? `<input type="text" class="music-tune-num music-band-q" data-f="q" inputmode="decimal" autocomplete="off" spellcheck="false" value="${esc(fmtTuneNum(f.q ?? 0.707, 3))}" aria-label="${esc(t("music.tuning_q"))}" />`
    : "";
  const menu = open
    ? `<div class="music-tune-menu">${kinds
        .map(
          (k) =>
            `<button type="button" class="music-seg-btn" data-act="pick" data-kind="${esc(k.name)}" aria-pressed="${k.name === f.kind}">${esc(tuningKindLabel(k.name))}</button>`,
        )
        .join("")}</div>`
    : "";
  return `<div class="music-tune-band music-tune-row${f.on ? "" : " off"}" data-i="${i}">
      ${freqIn}
      ${slider}
      ${gainIn}
      ${qIn}
      <div class="music-band-foot">
        <button type="button" class="music-tune-kind" data-act="kind" aria-expanded="${open}"><span>${esc(tuningKindLabel(f.kind))}</span>${SVG.chevron}</button>
        <button type="button" class="music-tune-sw" data-act="on" aria-pressed="${f.on}" title="${esc(f.on ? t("music.tuning_off") : t("music.tuning_on"))}">${SVG.power}</button>
        <button type="button" class="music-tune-del" data-act="del" title="${esc(t("music.tuning_del"))}">${SVG.trash}</button>
      </div>
      ${menu}
    </div>`;
}

/** 画那排频段柱。**幂等靠 `setHtml` 的字符串比对** —— 每次提交后都会重画一次，
 *  若无脑 `innerHTML =` ，用户刚在别的框里输的字会被整批冲掉。 */
function renderTuningFilters(root: HTMLElement) {
  // 拖动（曲线手柄 / 拉条）进行中**不许重建**：重建会把正在被拖的那个
  // `<input type="range">` 换成新节点，浏览器随之收不到后续 pointermove，
  // 表现是「拖两下就断」。松手后那次提交会把柱子补画回来（那时 `tuneHold` 已清）。
  if (tuneHold) return;
  const box = root.querySelector<HTMLElement>("#music-tuning-filters");
  if (!box || !tuning) return;
  // 用户正在这张表里编辑（某个输入框有焦点）**也不许重建**：重建会把聚焦节点
  // 换掉、焦点被踢回 `<body>`，表现为「点进输入框后输入立刻被取消」。
  // 频段文本框本来就「只在失焦/回车才提交」，所以编辑期间没有必须刷新的理由；
  // 失焦那次提交会把柱子补画回来。
  if (box.contains(document.activeElement)) return;
  const kinds = tuning.kinds;
  setHtml(box, tuneFilters.map((f, i) => tuningBandHtml(f, i, kinds, tuneOpen === i)).join(""));
}

/** 把画布上的一个像素点反算回 `{ f, db }`（拖动时用）。
 *
 *  与 `drawTuningCurve` 里的正算**逐字对应**（同一套 pad / 对数横轴 / 线性纵轴），
 *  并且用的是同一次绘制存下来的 `tuneGeom` —— 两处各算一份迟早会错位。
 *  `zoom` 补偿：画布在 CSS 缩放（`zoom`）下 `clientWidth` 与 `getBoundingClientRect()`
 *  不等，按两者比例换算，否则缩放后拖到的和看到的位置对不上。 */
function tunePointToValue(cv: HTMLCanvasElement, clientX: number, clientY: number) {
  const g = tuneGeom;
  if (!g) return null;
  const r = cv.getBoundingClientRect();
  const sx = g.w / (r.width || g.w);
  const sy = g.h / (r.height || g.h);
  const x = (clientX - r.left) * sx;
  const y = (clientY - r.top) * sy;
  const lo = Math.log10(20);
  const hi = Math.log10(20000);
  const frac = (x - g.padL) / g.plotW;
  const f = 10 ** (lo + frac * (hi - lo));
  const db = g.span - ((y - g.padT) / g.plotH) * (2 * g.span);
  return { f, db, x, y };
}

/** 把 `{ f, db }` 正算回画布像素（画手柄用，与上面那条互为逆运算）。 */
function tuneValueToPoint(g: TuneGeom, f: number, db: number) {
  const lo = Math.log10(20);
  const hi = Math.log10(20000);
  const x = g.padL + ((Math.log10(Math.max(1, f)) - lo) / (hi - lo)) * g.plotW;
  const y = g.padT + ((g.span - db) / (2 * g.span)) * g.plotH;
  return { x, y };
}

/** 画频响曲线。**只画不算** —— 数据来自宿主（`tuning.response` / 通道槽的 `cond_db`，
 *  都是引擎的解析式；当前画哪条由 `tuneSlotCurve()` 定）。
 *
 *  纵轴范围按数据自适应，但**下限 ±12dB**：固定范围会把 ±30dB 的段切在框外
 *  （看不见等于没有），而下限太小又会让一条平直的链被放大成一条抖动的线。
 *
 *  2026-10-02 起这一块还是**可拖的编辑面**：每段画一个手柄，拖动改频率 / 增益；
 *  空白处按下则是**新增一段**（peaking）。手柄的位置来自**草稿**（`tuneFilters`），
 *  曲线本身来自宿主 —— 所以拖动时手柄立刻跟手，曲线在宿主算完之后跟上。
 *  曲线本身**绝不在这里算**：那等于把 RBJ 再实现一遍，两份必然漂移（§4.6 那条纪律）。 */
/** 曲线缩放倍率（2026-10-03，用户要求「放大缩小调音曲线界面」）：1 = 铺满宽度；
 *  > 1 时画布按百分比变宽、由 `.music-tune-scroll` 横向滚动。`drawTuningCurve` 读的是
 *  `clientWidth` ⇒ 放大后按新宽度重画，线不会糊。 */
let tunePlotZoom = 1;

/** 应用曲线缩放：改画布宽度百分比 + 百分比标签，再重画一次。 */
function applyTunePlotZoom(root: HTMLElement) {
  const cv = root.querySelector<HTMLCanvasElement>("#music-tune-canvas");
  if (cv) cv.style.width = `${tunePlotZoom * 100}%`;
  setText(root.querySelector("#music-tune-zoom-label"), `${Math.round(tunePlotZoom * 100)}%`);
  drawTuningCurve(root);
}

function drawTuningCurve(root: HTMLElement) {
  const cv = root.querySelector<HTMLCanvasElement>("#music-tune-canvas");
  if (!cv || !tuning) return;
  const w = cv.clientWidth || cv.width;
  const h = cv.clientHeight || cv.height;
  if (w < 2 || h < 2) return;
  const dpr = window.devicePixelRatio || 1;
  if (cv.width !== Math.round(w * dpr) || cv.height !== Math.round(h * dpr)) {
    cv.width = Math.round(w * dpr);
    cv.height = Math.round(h * dpr);
  }
  const ctx = cv.getContext("2d");
  if (!ctx) return;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, w, h);

  const css = getComputedStyle(root);
  const accent = css.getPropertyValue("--accent").trim() || "#7aa2f7";
  const grid = css.getPropertyValue("--border-glass").trim() || "rgba(255,255,255,0.12)";
  const dim = css.getPropertyValue("--text-dim").trim() || "rgba(255,255,255,0.55)";

  const { freqs } = tuning.response;
  // 画的是**当前槽**那条线（P5-2）：全局槽 = 全局响应，通道槽 = 全局 + 本声道命中的分支
  //（宿主按声道算好的，见 `cond_db`）—— 用户在通道 L 的槽里改动，就该看到 L 会发出的响应。
  const db = tuneSlotCurve();
  const padL = 30;
  const padR = 8;
  const padT = 8;
  const padB = 16;
  const plotW = Math.max(1, w - padL - padR);
  const plotH = Math.max(1, h - padT - padB);
  let maxAbs = 0;
  for (const v of db) if (Number.isFinite(v)) maxAbs = Math.max(maxAbs, Math.abs(v));
  // 实测那条线也要**装得下**：只按合成曲线定纵轴范围的话，实测偏出去的部分会被
  // 切在框外 —— 而「偏出去多少」恰恰是这次测量要看的东西。
  for (const v of tuneMeasured?.db ?? []) {
    if (Number.isFinite(v)) maxAbs = Math.max(maxAbs, Math.abs(v));
  }
  const span = Math.min(60, Math.max(12, Math.ceil((maxAbs + 1) / 6) * 6));

  const lo = Math.log10(20);
  const hi = Math.log10(20000);
  const xOf = (f: number) => padL + ((Math.log10(Math.max(1, f)) - lo) / (hi - lo)) * plotW;
  const yOf = (v: number) => padT + ((span - v) / (2 * span)) * plotH;
  // 这一份要留给拖动那侧用（把像素反算回频率 / 增益），见 tunePointToValue。
  const geom: TuneGeom = { w, h, padL, padT, plotW, plotH, span };
  tuneGeom = geom;

  ctx.lineWidth = 1;
  ctx.font = "9px system-ui, -apple-system, sans-serif";
  ctx.textBaseline = "middle";

  // 横线：0dB 实线（它是「直通」这条基准），其余虚线
  for (const v of [span, span / 2, 0, -span / 2, -span]) {
    ctx.strokeStyle = grid;
    ctx.globalAlpha = v === 0 ? 0.9 : 0.45;
    ctx.setLineDash(v === 0 ? [] : [3, 3]);
    ctx.beginPath();
    ctx.moveTo(padL, yOf(v));
    ctx.lineTo(padL + plotW, yOf(v));
    ctx.stroke();
  }
  // 竖线：十倍频程
  ctx.setLineDash([]);
  ctx.fillStyle = dim;
  for (const f of [20, 100, 1000, 10000, 20000]) {
    const x = xOf(f);
    ctx.globalAlpha = 0.45;
    ctx.strokeStyle = grid;
    ctx.beginPath();
    ctx.moveTo(x, padT);
    ctx.lineTo(x, padT + plotH);
    ctx.stroke();
    ctx.globalAlpha = 0.9;
    ctx.textAlign = f === 20 ? "left" : f === 20000 ? "right" : "center";
    ctx.fillText(f >= 1000 ? `${f / 1000}k` : String(f), x, padT + plotH + 8);
  }
  // 纵轴刻度
  ctx.textAlign = "right";
  for (const v of [span, span / 2, 0, -span / 2, -span]) {
    ctx.globalAlpha = 0.9;
    ctx.fillStyle = dim;
    ctx.fillText(`${v > 0 ? "+" : ""}${fmtTuneNum(v, 0)}`, padL - 3, yOf(v));
  }
  ctx.textAlign = "left";
  ctx.globalAlpha = 1;

  // 合成曲线（宿主解析式）。建不起链时宿主给的是平线，照样画。
  if (freqs.length >= 2 && db.length === freqs.length) {
    // 曲线下方的淡填充（视觉上区分「提升了多少」与「削了多少」）
    ctx.beginPath();
    for (let i = 0; i < freqs.length; i++) {
      const x = xOf(freqs[i]);
      const y = yOf(db[i]);
      if (i === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    }
    ctx.lineTo(xOf(freqs[freqs.length - 1]), yOf(0));
    ctx.lineTo(xOf(freqs[0]), yOf(0));
    ctx.closePath();
    ctx.globalAlpha = 0.12;
    ctx.fillStyle = accent;
    ctx.fill();
    ctx.globalAlpha = 1;
    // 描线
    ctx.beginPath();
    for (let i = 0; i < freqs.length; i++) {
      const x = xOf(freqs[i]);
      const y = yOf(db[i]);
      if (i === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    }
    ctx.strokeStyle = accent;
    ctx.lineWidth = 1.5;
    ctx.stroke();
  }

  // 两条**实测**曲线（2026-10-03 起测两趟；都是虚线 / 点线，与合成曲线的实线区分）。
  //   · 仅自身链（绿虚线）：**这条才是「调音有没有真的生效」的判据** —— 它跟合成曲线
  //     贴合就说明链按你说的在做；偏离就是链本身的问题。
  //   · 最终输出（蓝点线）：链 × 系统（EAPO / 别的软件）。它离合成曲线远**不是**本插件
  //     的错 —— 那正是系统里还有别的东西在动，这条线就是给用户看这件事的。
  // 颜色用语义色但**不表示「成功 / 失败」**（绿色这里只表示「另一路数据」）。
  const measured = tuneMeasured;
  if (measured && measured.freqs.length >= 2) {
    const chainColor = css.getPropertyValue("--green").trim() || "#8fc99a";
    const totalColor = css.getPropertyValue("--blue").trim() || "#9490b4";
    const line = (values: number[], dash: number[], stroke: string, width: number) => {
      if (values.length < 2) return;
      ctx.save();
      ctx.beginPath();
      ctx.rect(padL, padT, plotW, plotH);
      ctx.clip();
      ctx.beginPath();
      for (let i = 0; i < measured.freqs.length && i < values.length; i++) {
        const x = xOf(measured.freqs[i]);
        const y = yOf(values[i]);
        if (i === 0) ctx.moveTo(x, y);
        else ctx.lineTo(x, y);
      }
      ctx.setLineDash(dash);
      ctx.strokeStyle = stroke;
      ctx.lineWidth = width;
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.restore();
    };
    line(measured.db, [1.5, 3], totalColor, 1.2);
    line(measured.chain_db ?? [], [5, 3], chainColor, 1.5);
  }

  // 每段一个手柄。位置来自**草稿**（不是宿主曲线）：拖动时手柄立刻跟手，
  // 而曲线要等宿主算完才动 —— 两者不同步是刻意的（曲线不许在前端算）。
  //
  // ⚠️ **两扇窗都画手柄**（2026-10-03 用户明确「为何频响曲线无法支持调整」⇒ 曲线要能编辑）：
  // 拖动那条链路已抽到 `wireTuningCurveDrag`、两扇窗共用同一份（见 `wireTuningCurve`）。
  if (tuneFilters.length > 0) {
    ctx.save();
    ctx.font = "9px system-ui, -apple-system, sans-serif";
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    for (let i = 0; i < tuneFilters.length; i++) {
      const f = tuneFilters[i];
      // 不带增益的类型（低通 / 高通 / 陷波…）没有「增益」这个维度 ⇒ 把手柄摆在
      // 0dB 线上，它只能左右拖。画成三角以便和可上下拖的圆点区分开。
      const hasGain = (tuning.kinds.find((k) => k.name === f.kind)?.gain) ?? f.gain_db !== undefined;
      const { x, y } = tuneValueToPoint(geom, f.freq_hz, hasGain ? f.gain_db ?? 0 : 0);
      const active = i === tuneDrag;
      ctx.globalAlpha = f.on ? 1 : 0.4;
      ctx.beginPath();
      if (hasGain) {
        ctx.arc(x, y, active ? 6 : 4.5, 0, Math.PI * 2);
      } else {
        ctx.moveTo(x, y - 5);
        ctx.lineTo(x + 5, y + 3);
        ctx.lineTo(x - 5, y + 3);
        ctx.closePath();
      }
      ctx.fillStyle = active ? accent : "rgba(0,0,0,0.65)";
      ctx.fill();
      ctx.strokeStyle = accent;
      ctx.lineWidth = 1.5;
      ctx.stroke();
      // 段号写在手柄里：表里那一行与曲线上这一点**对得上号**（否则多段时无从对应）。
      if (hasGain && i < 9) {
        ctx.globalAlpha = 1;
        ctx.fillStyle = active ? "rgba(0,0,0,0.8)" : accent;
        ctx.fillText(String(i + 1), x, y + 0.5);
      }
    }
    ctx.globalAlpha = 1;
    ctx.restore();
  }
}

/** 换一个类型时**照引擎的判据摆好字段**：「该有增益的必须有、该没有的一个字都不许写」、
 *  一阶没有 Q。这条判据的**权威在引擎**（`config.rs`），这里只是提前摆好输入框，
 *  免得用户每换一次类型都被拒一次（每次被拒都要重来一遍，很烦）。 */
function applyTuningKind(f: TuningFilter, name: string, kinds: TuningKind[]) {
  f.kind = name;
  const meta = kinds.find((k) => k.name === name);
  if (!meta) return;
  if (meta.gain) {
    if (f.gain_db === undefined) f.gain_db = 0;
  } else {
    delete f.gain_db;
  }
  if (meta.order === 1) delete f.q;
  else if (f.q === undefined) f.q = 0.707;
}

/** 提交整条链（预增益 + 全部段）。**校验不过就回滚界面** —— 链的合法性一律由引擎的
 *  `ChainConfig::from_json` 兜底（见 TuningDto 注释），宿主校验失败时这次设置整个不落盘。 */
async function commitTuningChain(root: HTMLElement, quiet = false) {
  const seq = ++tuneSeq;
  // 提交前记下焦点（按「行号 + 字段名」，因为重画会把元素整批换掉）。
  // 少了这一步，用户按回车提交后焦点会掉到 body 上，接着敲的字全都落空。
  // **只记文本框**：拉条（`type="range"`）拖动中也会走到这里，把焦点还给
  // 「同一字段的第一个匹配元素」会把焦点从拉条挪到数字框上，拖动当场断掉。
  const active = document.activeElement as HTMLElement | null;
  const focusRow = active?.closest?.(".music-tune-row") as HTMLElement | null;
  const focusField =
    active instanceof HTMLInputElement && active.type === "text" ? active.dataset.f : undefined;
  const focusIdx = focusRow ? Number(focusRow.dataset.i) : -1;
  // 频段柱**之外**的文本框（预增益 / 低音 / 高音 / 延迟）：它们没有 `.music-tune-row`，
  // 靠 id 还原焦点（见文件末段那次还原）。
  const focusId =
    active instanceof HTMLInputElement && active.type === "text" && !focusRow ? active.id : "";
  let dto: TuningDto;
  try {
    dto = await invoke<TuningDto>("player_tuning_set_chain", {
      preampDb: tunePreamp,
      // 低音 / 高音快捷增益：引擎不认识它们，宿主会追加成 low_shelf / high_shelf 两条。
      bassDb: tuneBass,
      trebleDb: tuneTreble,
      // ⚠️ **全局槽 = `tuneSlotDrafts[0]`，不是 `tuneFilters`**（P5-2）：后者是「表正在编辑
      // 的那一份」，用户停在某个通道槽时它指的是那条声道 —— 拿它当全局发会把通道的段搬进全局。
      filters: tuneSlotDrafts[0],
      // 新效果器全部**走草稿**（它们现在都有界面了：P5-3 图示均衡器 / P5-4 延迟 + 复制 + 卷积）。
      // 宿主那条命令的语义是「**覆盖整条链**」⇒ 少发一样就等于把它清掉。
      graphicEq: tuneGeq,
      delayMs: tuneDelay,
      channelCopy: tuneCopy,
      convolution: tuneConv,
      // 通道段由**全部槽的草稿**拼（P5-2）—— 不是把宿主那份原样回传：用户这会儿
      // 可能正在某个通道槽里改，那份改动就在 `tuneSlotDrafts` 里。
      ifElse: tuneIfElsePayload(),
    });
  } catch (e) {
    // 已经有一次更新的提交在后面 ⇒ 这次的结果无所谓对错了，连报错都别报（会误导）
    if (seq !== tuneSeq) return;
    note(root, errText(e));
    await loadTuning(root);
    return;
  }
  if (seq !== tuneSeq) return;
  tuning = dto;
  syncTuningDraft();
  // `quiet`：拖动中的节流提交（最多每 120ms 一次）不该每次都喊一句「已保存」，
  // 否则提示行会在拖动期间一直闪。松手时那次会照常提示。
  if (!quiet) note(root, t("music.tuning_saved"));
  renderTuning(root);
  if (focusField && focusIdx >= 0 && focusIdx < tuneFilters.length) {
    // ⚠️ 必须带 `[type="text"]`：一格频段柱里 `gain_db` 有**两个**控件（竖拉条 + 大号数字框），
    // 只按 `data-f` 找会命中排在前面的拉条，焦点被从文本框挪走（同 tuneHold 那条注释的坑）。
    root
      .querySelector<HTMLInputElement>(`.music-tune-row[data-i="${focusIdx}"] input[type="text"][data-f="${focusField}"]`)
      ?.focus();
  } else if (focusId) {
    // 预增益 / 低音 / 高音 / 延迟那几个框：按 id 还原。少了这一句，任何一次提交
    // （别的控件触发的也算）都会把用户的焦点踢掉 —— 看着像「被误存 + 被踢出输入框」。
    root.querySelector<HTMLInputElement>(`#${focusId}`)?.focus();
  }
}

/** 只改总开关。**不走 `commitTuningChain`** —— 那条路会把整条链重发一遍，
 *  而「只是关一下」不该有任何副作用（宿主那条命令也只动 `enabled`）。 */
async function setTuningEnabled(root: HTMLElement, enabled: boolean) {
  const seq = ++tuneSeq;
  let dto: TuningDto;
  try {
    dto = await invoke<TuningDto>("player_tuning_set_enabled", { enabled });
  } catch (e) {
    if (seq !== tuneSeq) return;
    note(root, errText(e));
    await loadTuning(root);
    return;
  }
  if (seq !== tuneSeq) return;
  tuning = dto;
  syncTuningDraft();
  note(root, t("music.tuning_saved"));
  renderTuning(root);
}

/** 选一档预设 = 把它的段**展开进表**，并**顺手打开总开关** —— 不打开的话用户点了
 *  「低音增强」却什么都听不到，还得再去拨一下上面那个开关（那一步没有任何信息量）。
 *  走的是老命令 `player_tuning_set`：它「总开关 + 预设」一次原子落盘。 */
async function applyTuningPreset(root: HTMLElement, preset: string) {
  const seq = ++tuneSeq;
  let dto: TuningDto;
  try {
    dto = await invoke<TuningDto>("player_tuning_set", { enabled: true, preset });
  } catch (e) {
    if (seq !== tuneSeq) return;
    note(root, errText(e));
    await loadTuning(root);
    return;
  }
  if (seq !== tuneSeq) return;
  tuning = dto;
  syncTuningDraft();
  tuneOpen = -1;
  // 换了一档预设 = 换了一条链 ⇒ 上一次的实测曲线**不再描述现在这条链**，
  // 留着会让人拿它比一条根本不是它量的曲线。手改某一段则**保留**（那正是
  // 「测完照着实测去微调」的用法，见 drawTuningCurve 那段注释）。
  tuneMeasured = null;
  note(root, t("music.tuning_saved"));
  renderTuning(root);
}

// ── 用户预设（P5-1，2026-10-03）──────────────────────────────────
//
// 预设 = 一整条链的快照（宿主 `config\tuning\presets\`）。这里只管界面与三条动作；
// **文件对话框在两端各担一半**：前端弹框拿路径（WebView2 没有 `window.prompt`，
// 但 `@tauri-apps/plugin-dialog` 的 open / save 是好的），宿主读写文件。

/** 提交命名行（保存 / 重命名 / 删除三态共用）。 */
async function submitPresetEdit(root: HTMLElement) {
  const edit = tunePresetEdit;
  if (!edit) return;
  const name =
    edit.mode === "delete"
      ? edit.name
      : (root.querySelector<HTMLInputElement>("#music-preset-name")?.value ?? "").trim();
  if (edit.mode !== "delete" && !name) {
    note(root, t("music.tuning_preset_name_empty"));
    return;
  }
  try {
    if (edit.mode === "save") {
      tuning = await invoke<TuningDto>("player_tuning_preset_save", { name, overwrite: edit.overwrite });
      note(root, t("music.tuning_preset_saved", { name }));
    } else if (edit.mode === "rename") {
      tuning = await invoke<TuningDto>("player_tuning_preset_rename", { from: edit.from, to: name });
      note(root, t("music.tuning_preset_renamed", { name }));
    } else {
      tuning = await invoke<TuningDto>("player_tuning_preset_delete", { name });
      note(root, t("music.tuning_preset_deleted", { name }));
    }
    tunePresetEdit = null;
    syncTuningDraft();
    renderTuning(root);
  } catch (e) {
    const msg = errText(e);
    // 同名已存在：**不许静默盖掉**人家存的链 —— 把按钮换成「覆盖」，再点一次才算数。
    if (msg.includes("ERR_PRESET_EXISTS")) {
      if (edit.mode === "save") {
        tunePresetEdit = { mode: "save", name, overwrite: true };
        renderTuning(root);
      } else {
        note(root, t("music.tuning_preset_exists", { name }));
      }
      return;
    }
    note(root, msg);
  }
}

/** 导入一个预设文件：前端弹「打开」框拿路径，宿主读 + 校验 + 落盘 + 应用。 */
async function importTuningPreset(root: HTMLElement) {
  let picked: string | string[] | null;
  try {
    picked = await openDialog({
      multiple: false,
      filters: [{ name: "Lunac preset", extensions: ["json"] }],
    });
  } catch (e) {
    note(root, errText(e));
    return;
  }
  if (typeof picked !== "string") return; // 取消
  try {
    tuning = await invoke<TuningDto>("player_tuning_preset_import", { path: picked });
    syncTuningDraft();
    tuneOpen = -1;
    tuneMeasured = null; // 换了链，上一次实测作废（同 applyTuningPreset）
    renderTuning(root);
    note(root, t("music.tuning_preset_imported", { name: tuning.preset }));
  } catch (e) {
    note(root, errText(e));
  }
}

/** 选一个 IR（脉冲响应）WAV 并提交（P5-4）。
 *
 *  **只挑文件，不在这里校验**：引擎那两条硬要求（**单声道** + **采样率与这条流一致**）
 *  只有拿到渲染流的采样率才判得了，所以校验发生在提交那一次 —— 失败时
 *  `commitTuningChain` 会把错误原样报出来并把链回滚（见那条路径）。面板**不做**半套校验：
 *  自己解析 WAV 头等于把引擎那份实现再写一遍，两份必然漂移。 */
async function pickTuningIr(root: HTMLElement) {
  let picked: string | string[] | null;
  try {
    picked = await openDialog({
      multiple: false,
      filters: [{ name: "WAV", extensions: ["wav"] }],
    });
  } catch (e) {
    note(root, errText(e));
    return;
  }
  if (typeof picked !== "string") return; // 取消
  tuneConv = picked;
  await commitTuningChain(root);
}

/** 把**当前这条链**导出成预设文件（「另存为」框选路径，宿主写文件）。 */
async function exportTuningChain(root: HTMLElement) {
  if (!tuning) return;
  const stem = tuning.preset && tuning.preset !== "custom" ? tuning.preset : "lunac-chain";
  let path: string | null;
  try {
    path = await saveDialog({
      defaultPath: `${stem}.json`,
      filters: [{ name: "Lunac preset", extensions: ["json"] }],
    });
  } catch (e) {
    note(root, errText(e));
    return;
  }
  if (typeof path !== "string") return; // 取消
  try {
    await invoke("player_tuning_preset_export", { path });
    note(root, t("music.tuning_preset_exported", { path }));
  } catch (e) {
    note(root, errText(e));
  }
}

/** 拖动中的**节流提交**（见 `tuneHold` / `tuneCommitAt` 的注释）。
 *  宿主那条命令要「校验 + 落盘 + 换链」，连着几十次发既刷屏又刷盘；
 *  但也不能只在松手时提交 —— 那样曲线全程是旧的，拖动就没有反馈。 */
function tuneCommitSoon(root: HTMLElement) {
  const now = Date.now();
  if (now - tuneCommitAt >= 120) {
    tuneCommitAt = now;
    void commitTuningChain(root, true);
    return;
  }
  if (tuneCommitTimer == null) {
    tuneCommitTimer = window.setTimeout(() => {
      tuneCommitTimer = null;
      tuneCommitAt = Date.now();
      void commitTuningChain(root, true);
    }, 120);
  }
}

/** 数字框「编辑完成」（失焦 / 回车）提交的**去抖**（见 `TUNE_CHANGE_DEBOUNCE_MS`）。
 *
 *  与 `tuneCommitSoon`（拖动中的节流）刻意分开：这条只服务「输入框编辑完成」这一个语义。
 *  触发时机不变（仍是失焦 / 回车），只是把**同一拍里连发的多次提交合并成一次** ——
 *  这正是「有时候一个动作就会保存」的来路：输完点一下别处，那个框的 `change` 与被点中
 *  控件自己的提交会各发一次 IPC。 */
function tuneCommitDebounced(root: HTMLElement) {
  if (tuneChangeTimer != null) window.clearTimeout(tuneChangeTimer);
  tuneChangeTimer = window.setTimeout(() => {
    tuneChangeTimer = null;
    void commitTuningChain(root);
  }, TUNE_CHANGE_DEBOUNCE_MS);
}

/** 跑一次**实时测量**（宿主 `player_tuning_measure`，见 tuning_probe.rs）。
 *
 *  **会出声约 1.5 秒**（一段 20Hz→20kHz 的扫频）—— 按钮文案与提示行都如实写了这件事，
 *  不能让用户点下去「莫名其妙响一下」。宿主在测之前会先暂停我们自己正在放的那一首，
 *  测完还回去；外部程序的声音它管不着，只能靠提示行让用户先静音别的播放器。
 *
 *  失败时**保留上一次的结果**：一次打开失败不该把已经量到的曲线擦掉，
 *  那会让人以为「刚才那次白测了」。 */
async function runTuningMeasure(root: HTMLElement) {
  if (tuneProbing) return;
  tuneProbing = true;
  renderProbe(root);
  try {
    tuneMeasured = await invoke<MeasuredResponse>("player_tuning_measure");
  } catch (e) {
    note(root, errText(e));
  } finally {
    tuneProbing = false;
    renderTuning(root);
  }
}

/** 曲线画布上的**拖动编辑**（2026-10-02 用户第 6 条「自定义拖动」）—— 两扇窗共用。
 *
 *  手势与 Peace / REW 那一族一致：**拖已有的手柄**改它的频率（横向，对数轴）与增益
 *  （纵向）；**空白处按下**则在那个位置**新建一段**（peaking）。
 *  命中判定按**屏幕像素距离**而不是频率差：手柄本来就画在屏幕上，而低频端一个倍频程
 *  只占几像素，按频率差判会让高频端几乎点不中。 */
function wireTuningCurveDrag(root: HTMLElement) {
  const canvas = root.querySelector<HTMLCanvasElement>("#music-tune-canvas");
  if (!canvas) return;
  const hitTest = (cv: HTMLCanvasElement, clientX: number, clientY: number) => {
    const p = tunePointToValue(cv, clientX, clientY);
    const g = tuneGeom;
    if (!p || !g) return null;
    let idx = -1;
    let best = 14; // 14px 内算命中（手柄半径 4.5，再给手指一点余量）
    for (let i = 0; i < tuneFilters.length; i++) {
      const f = tuneFilters[i];
      const hasGain =
        (tuning?.kinds.find((k) => k.name === f.kind)?.gain) ?? f.gain_db !== undefined;
      const pt = tuneValueToPoint(g, f.freq_hz, hasGain ? f.gain_db ?? 0 : 0);
      const d = Math.hypot(pt.x - p.x, pt.y - p.y);
      if (d < best) {
        best = d;
        idx = i;
      }
    }
    // 「在绘图区之内吗」：**只有区内的空白按下才新增一段** —— 点在坐标轴 / 边距上
    // 多半是想看看图，不该凭空多出一段（多出来的那段还会立刻落盘）。
    const inside =
      p.x >= g.padL - 6 &&
      p.x <= g.padL + g.plotW + 6 &&
      p.y >= g.padT - 6 &&
      p.y <= g.padT + g.plotH + 6;
    return { p, idx, inside };
  };
  canvas.addEventListener("pointerdown", (ev) => {
    const cv = ev.currentTarget as HTMLCanvasElement;
    const hit = hitTest(cv, ev.clientX, ev.clientY);
    if (!hit) return;
    let idx = hit.idx;
    // 每次按下都重置「动过没有」：上一次拖动留下的 true 会让「只是点一下」也提交。
    tuneDragMoved = false;
    if (idx < 0 && !hit.inside) return;
    if (idx < 0) {
      // 空白处按下 = 就地新增。**这正是「自定义拖动」想要的手感**：直接在想动的
      // 地方画一个点，比「先加一段、再回表里填三个数字」快得多。
      tuneFilters.push({
        kind: "peaking",
        freq_hz: tuneClampFreq(hit.p.f),
        gain_db: tuneClampGain(hit.p.db),
        q: 0.707,
        on: true,
      });
      idx = tuneFilters.length - 1;
      tuneOpen = -1;
      tuneDragMoved = true;
    }
    tuneDrag = idx;
    tuneHold = true;
    cv.setPointerCapture(ev.pointerId);
    drawTuningCurve(root);
    ev.preventDefault();
  });
  canvas.addEventListener("pointermove", (ev) => {
    if (tuneDrag < 0) return;
    const cv = ev.currentTarget as HTMLCanvasElement;
    const f = tuneFilters[tuneDrag];
    const p = tunePointToValue(cv, ev.clientX, ev.clientY);
    if (!f || !p) return;
    const hasGain = (tuning?.kinds.find((k) => k.name === f.kind)?.gain) ?? f.gain_db !== undefined;
    const nf = tuneClampFreq(p.f);
    const ng = tuneClampGain(p.db);
    if (nf !== f.freq_hz || (hasGain && ng !== f.gain_db)) tuneDragMoved = true;
    f.freq_hz = nf;
    if (hasGain) f.gain_db = ng;
    drawTuningCurve(root); // 手柄立刻跟手
    tuneCommitSoon(root); // 曲线等宿主算完再跟上（节流）
    ev.preventDefault();
  });
  const endDrag = (ev: PointerEvent) => {
    if (tuneDrag < 0) return;
    const cv = ev.currentTarget as HTMLCanvasElement;
    const moved = tuneDragMoved;
    tuneDrag = -1;
    tuneDragMoved = false;
    tuneHold = false;
    if (cv.hasPointerCapture?.(ev.pointerId)) cv.releasePointerCapture(ev.pointerId);
    // 只是点了一下手柄（没挪）就不提交：那一次会把 `preset` 从某一档改成 `custom`，
    // 而用户其实什么都没改。松手那一下要清掉节流窗口，否则紧接着的下一次拖动第一次
    // `tuneCommitSoon` 会被节流吞掉。
    tuneCommitAt = 0;
    if (moved) void commitTuningChain(root);
  };
  canvas.addEventListener("pointerup", endDrag);
  canvas.addEventListener("pointercancel", endDrag);
}

/** 曲线那一块的控件接线（缩放 / 测量 / 通道槽）—— **调音页与曲线窗共用这一份**。
 *
 *  两处用的是**同一组 id**（见 `curveShellHtml` 的注释），所以接线只写一份；
 *  各写一份必然漂移，而漂移的表现是「在一个窗里点得动、另一个窗里点了没反应」——
 *  那种错没有任何日志，只能靠人来发现。 */
function wireTuningCurve(root: HTMLElement) {
  root.querySelector<HTMLElement>("#music-tune-measure")?.addEventListener("click", () => {
    void runTuningMeasure(root);
  });
  /** **锚点缩放**（2026-10-03 用户要求「跟随鼠标位置、滚轮调整大小」）：
   *  缩放前后把 `clientX` 底下的那个**频率**钉在原地 —— 否则放大时想看的那个峰会被
   *  推出视野，用户得一边缩一边横向拖回去。
   *  做法：记下鼠标在**当前画布**里的比例位置，改完宽度（`applyTunePlotZoom` 会重画）后
   *  把 `scrollLeft` 补回差值，使同一个比例仍然落在鼠标下。
   *  缩放倍率范围 100%–300%（与 `applyTunePlotZoom` 的写入一致）。 */
  const zoomAt = (clientX: number, next: number) => {
    const sc = root.querySelector<HTMLElement>("#music-tune-scroll");
    const cv = root.querySelector<HTMLCanvasElement>("#music-tune-canvas");
    if (!sc || !cv) return;
    const before = cv.getBoundingClientRect();
    const frac = before.width > 0 ? (clientX - before.left) / before.width : 0.5;
    const clamped = Math.min(3, Math.max(1, Math.round(next * 100) / 100));
    if (clamped === tunePlotZoom) return;
    tunePlotZoom = clamped;
    applyTunePlotZoom(root);
    const after = cv.getBoundingClientRect();
    sc.scrollLeft += after.left + frac * after.width - clientX;
  };
  const zoomFromCenter = (delta: number) => {
    const sc = root.querySelector<HTMLElement>("#music-tune-scroll");
    if (!sc) return;
    const r = sc.getBoundingClientRect();
    zoomAt(r.left + r.width / 2, tunePlotZoom + delta);
  };
  root.querySelector<HTMLElement>("#music-tune-zoom-in")?.addEventListener("click", () => zoomFromCenter(0.25));
  root.querySelector<HTMLElement>("#music-tune-zoom-out")?.addEventListener("click", () => zoomFromCenter(-0.25));
  // 滚轮：**必须非 passive** —— 要 `preventDefault()` 挡住「滚轮滚动整个面板」那套默认行为。
  root.querySelector<HTMLElement>("#music-tune-scroll")?.addEventListener(
    "wheel",
    (ev) => {
      ev.preventDefault();
      zoomAt(ev.clientX, tunePlotZoom + (ev.deltaY < 0 ? 0.2 : -0.2));
    },
    { passive: false },
  );
  // 画布上的**拖动编辑**（改频率 / 增益、空白处新增一段）——两扇窗共用同一份接线。
  wireTuningCurveDrag(root);
  // 通道槽（P5-2）：选一条声道看**它**的曲线（全局 / L / R / …）—— 曲线窗有自己的槽状态
  //（两个窗是两个 JS 上下文，`tuneSlot` 互相独立），选完只影响这扇窗画哪条线。
  root.querySelector<HTMLElement>("#music-tune-slots")?.addEventListener("click", (ev) => {
    const btn = (ev.target as HTMLElement).closest<HTMLElement>("[data-slot]");
    if (!btn) return;
    selectTuneSlot(root, Number(btn.dataset.slot));
  });
}

/** 曲线窗那条轻轮询的句柄（`undefined` = 没在跑）。 */
let curveTimer: number | undefined;
/** 曲线窗的刷新周期。**折中值**：拖动推子时这扇窗几乎跟得上，又不至于把宿主刷屏。 */
const CURVE_POLL_MS = 1200;

/** 停掉曲线窗的轮询（幂等）。`stopMusicPolling` 也调它 —— 那是**所有**音乐窗唯一的
 *  卸载钩子（`attach.ts` 的 detach 只认它），关窗时必须走到这里。 */
function stopCurvePoll() {
  if (curveTimer !== undefined) {
    window.clearTimeout(curveTimer);
    curveTimer = undefined;
  }
}

/** **调音页那条轻轮询**的句柄（`undefined` = 没在跑）。
 *
 *  **为什么必须有**（2026-10-03 用户报的「均衡器能影响频响曲线，但是反之不行」）：
 *  曲线已经搬进**另一扇窗**（`plugin-music-curve`），那扇窗每 1.2s 拉一次宿主真值
 *  ⇒「调音页改 EQ → 曲线跟着动」成立；反过来**不成立** —— 主窗原先只在「切进调音页」
 *  那一刻 `loadTuning` 一次，之后再没有谁去拉真值，于是用户在曲线窗拖手柄改的链，
 *  调音页那排频段柱一直停在旧值。
 *  `loadTuning` 只在宿主真值真的变了时才重铺草稿（那条「不无谓重绘」的口径）⇒
 *  常态一拍只花一次 IPC，也不会打断用户正在输的字（草稿没提交时宿主真值没变）。 */
let tunePageTimer: number | undefined;
const TUNE_PAGE_POLL_MS = 1200;

/** 停掉调音页轮询（幂等）。`stopMusicPolling` 也调它。 */
function stopTunePagePoll() {
  if (tunePageTimer !== undefined) {
    window.clearTimeout(tunePageTimer);
    tunePageTimer = undefined;
  }
}

/** 起调音页轮询（**幂等**：已在跑就不再起 —— `renderMode` 会被反复调用，
 *  每调一次就重置定时器的话，这个轮询等于永远不触发）。 */
function startTunePagePoll(root: HTMLElement) {
  if (tunePageTimer !== undefined) return;
  const tick = async () => {
    if (!root.isConnected || mode !== "tuning") {
      stopTunePagePoll();
      return;
    }
    await loadTuning(root);
    if (root.isConnected && mode === "tuning") {
      tunePageTimer = window.setTimeout(() => void tick(), TUNE_PAGE_POLL_MS);
    }
  };
  tunePageTimer = window.setTimeout(() => void tick(), TUNE_PAGE_POLL_MS);
}

/** **曲线窗**的挂载（2026-10-03）：拉一次真值 + 接线 + 一条轻轮询。**不碰**主面板那一整套。 */
async function attachCurveWindow(root: HTMLElement) {
  stopCurvePoll();
  // 实测结果只在内存里、且属于「刚才那条链」—— 新开的窗不该带着上一次的记录。
  tuneMeasured = null;
  tuneProbing = false;
  wireTuningCurve(root);
  await loadTuning(root);

  // **为什么要轮询**：链是主窗改的（推子 / 数值表 / 预设都在那边），这扇窗只是另一块
  // 显示器。`loadTuning` 内部只在宿主真值变了时才重铺草稿并重画（那条「不无谓重绘」的
  // 口径），所以常态的一拍只花一次 IPC。
  const tick = async () => {
    if (!root.isConnected) {
      stopCurvePoll(); // 窗没了 ⇒ 自停（`stopMusicPolling` 是显式那条路）
      return;
    }
    await loadTuning(root);
    if (root.isConnected) curveTimer = window.setTimeout(() => void tick(), CURVE_POLL_MS);
  };
  curveTimer = window.setTimeout(() => void tick(), CURVE_POLL_MS);
}

/** 打开（或复用）**频响曲线窗** —— 宿主那边是 `plugin-music-curve`，与主窗互不干扰
 *  （两个 label 两扇窗，见宿主 `plugin_window::CURVE_WINDOW_KEY`）。 */
async function openCurveWindow(root: HTMLElement) {
  try {
    await invoke("open_plugin_window", { pluginId: "music", key: CURVE_WIN_KEY, input: "" });
  } catch (e) {
    note(root, errText(e));
  }
}

// ── 渲染：左栏（Library）与主区 ────────────────────────────────────

function isCurrentTrack(tr: TrackDto): boolean {
  return !!tr.uri && player?.track?.uri === tr.uri;
}

/** 一条一次性提示（底部那行小字）。`sweepMsg` 会在 `MSG_TTL_MS` 后自动抹掉它。 */
function note(root: HTMLElement, text: string) {
  setText(root.querySelector("#music-msg"), text);
}

/** 曲目行。**主区与搜索页共用** —— 2026-09-28 两栏改造把「歌单行内展开」删掉了，
 *  所以不再需要传歌单 uri；但 `data-uri` 要留着：`syncPlayingMarks` 靠它标「正在播的那首」。
 *  `title` 只有播放队列在用：那里的每一行都点一下就**从这首开始重排队列**，
 *  这个副作用必须让用户看得到（Spotify 没有删队列项的接口，这是最接近的效果）。 */
function trackRowHtml(tr: TrackDto, idx: number, act: string, dataI = "", title = ""): string {
  return `<button class="music-tr${isCurrentTrack(tr) ? " playing" : ""}" data-act="${act}" data-uri="${esc(tr.uri)}"${dataI ? ` data-i="${esc(dataI)}"` : ""}${title ? ` title="${esc(title)}"` : ""}>
    <span class="music-tr-i">${idx + 1}</span>
    <span class="music-tr-main">
      <span class="music-tr-n">${esc(tr.name || "?")}</span>
      <span class="music-tr-a">${esc(tr.artists || tr.album)}</span>
    </span>
    <span class="music-tr-d">${fmtMs(tr.duration_ms)}</span>
  </button>`;
}

/** `PlaylistDto` → 左栏的统一形状。**唯一的适配点**（歌单的副标题是「所有者」）。 */
function libItemOfPlaylist(p: PlaylistDto): LibraryItemDto {
  return { id: p.id, uri: p.uri, name: p.name, cover: p.cover, subtitle: p.owner, total: p.total };
}

/** 左栏的一行。四类共用（专辑 / 歌手 / 电台都走它）—— 与旧的歌单行是同一套外观。
 *  `data-act="open"` 是**点击委托里那个唯一的分派名**：左栏行、搜索预览行、搜索详细页
 *  的卡片点下去都是同一件事（主区换成这一项），所以三处共用一个 act，不多写分支。
 *  **行尾不再有播放按钮**（用户 2026-09-28 删）：左栏与搜索预览里那枚圆形播放键都去掉，
 *  改成**双击这一行即播它**（见 `attachMusicListeners` 的 dblclick）—— 详细页头那个
 *  「播放全部」保留，所以「打开看曲目再决定」这条路并没有断。 */
function libRowHtml(kind: SideKind, it: LibraryItemDto): string {
  // **「正在播的就是它」按 uri 比对上下文**（专辑 / 歌手也有 uri，所以三条都能亮）
  const playing = !!it.uri && player?.context_uri === it.uri;
  const cur = selected?.kind === kind && selected.id === it.id;
  const cover = it.cover ? `<img src="${esc(it.cover)}" alt="">` : "";
  return `<div class="music-pl${playing ? " playing" : ""}${cur ? " on" : ""}" data-uri="${esc(it.uri)}">
    <button class="music-pl-btn" data-act="open" data-kind="${kind}" data-id="${esc(it.id)}" title="${esc(it.name || "?")}">
      <span class="music-pl-cover">${cover}</span>
      <span class="music-pl-main">
        <span class="music-pl-name">${esc(it.name || "?")}</span>
        <span class="music-pl-sub">${esc(it.subtitle)}</span>
      </span>
    </button>
  </div>`;
}

/** 「最常听的歌曲」那一项（左栏「最常听」栏的第一行，2026-10-01 用户第 4 条）。
 *
 *  **与收藏夹同一种做法：前端合成。** 原因也一样 —— `/me/top/tracks` 不是「某一类资源」，
 *  那一栏真正列出来的是**歌手**（`/me/top/artists`），歌曲这一份没有 id、也不是列表项。
 *  副标题写死「近 4 周」而不是含糊的「最常听」：那正是我们拉数据用的
 *  `time_range=short_term`，写清楚才不会被当成「有史以来」。 */
function topTracksRowHtml(): string {
  const cur = selected?.kind === "top-tracks";
  return `<div class="music-pl${cur ? " on" : ""}">
    <button class="music-pl-btn" data-act="open" data-kind="top-tracks" data-id="top-tracks" title="${esc(t("music.top_tracks"))}">
      <span class="music-pl-cover music-pl-trend-cover">${SVG.trend}</span>
      <span class="music-pl-main">
        <span class="music-pl-name">${esc(t("music.top_tracks"))}</span>
        <span class="music-pl-sub">${esc(t("music.top_tracks_sub"))}</span>
      </span>
    </button>
  </div>`;
}

/** 主区 / 搜索页用的卡片（Spotify 那种竖卡）。与左栏的行**不是一套**：
 *  左栏窄，只放得下行；主区宽，卡片才有「浏览」的感觉。 */
function libCardHtml(kind: SideKind, it: LibraryItemDto): string {
  const cover = it.cover ? `<img src="${esc(it.cover)}" alt="">` : "";
  return `<button class="music-card" data-act="open" data-kind="${kind}" data-id="${esc(it.id)}" title="${esc(it.name)}">
    <span class="music-card-cover${kind === "artist" ? " round" : ""}">${cover}</span>
    <span class="music-card-n">${esc(it.name)}</span>
    <span class="music-card-s">${esc(it.subtitle)}</span>
  </button>`;
}

/** 「我喜欢的歌曲」那一项（左栏歌单栏的第一行）。
 *
 *  **它是前端合成的**：Spotify 的 `/me/playlists` 里**没有**收藏夹（它走 `/me/tracks`），
 *  所以这一项不是从歌单列表里筛出来的，也不随歌单请求的成败消失。
 *
 *  代价：**收藏夹没有可播的 `context_uri`** ⇒ 播放只能走 `uris`（见 `spotify_play_uris`），
 *  于是 `player.context_uri` 匹配不到它，「正在播的就是它」那个标色对它无效。
 *  这一点不假装能做到（规范里也写着）。 */
function likedRowHtml(): string {
  const cur = selected?.kind === "liked";
  const n = liked ? String(liked.total) : "";
  // 拉失败时把原因当副标题（**不隐藏**）：缺 scope 时用户要看到「去重新登录」，
  // 而不是一个点了没反应的收藏夹。
  const sub = likedError || [cfg?.display_name || "", n].filter(Boolean).join(" · ");
  return `<div class="music-pl music-pl-liked${cur ? " on" : ""}${likedError ? " err" : ""}">
    <button class="music-pl-btn" data-act="open" data-kind="liked" data-id="liked" title="${esc(t("music.liked_songs"))}">
      <span class="music-pl-cover music-pl-liked-cover">${SVG.heart}</span>
      <span class="music-pl-main">
        <span class="music-pl-name">${esc(t("music.liked_songs"))}</span>
        <span class="music-pl-sub">${esc(sub)}</span>
      </span>
    </button>
  </div>`;
}

/** 左栏列表。**四类只有一个渲染入口** —— 歌单那一栏额外把收藏夹合成在第一行。
 *  `localPage` 时整块交给 `renderLocalSide`（本地目录那一栏与 Spotify 这四类
 *  是两套数据源，共用入口只会多出一串「如果是本地…」的分支）。 */
function renderSide(root: HTMLElement) {
  if (localPage) { renderLocalSide(root); return; }
  // 从本地页切回来时要把那排 Spotify 页签放回去、把本地页头收掉
  show(root.querySelector("#music-side-tabs"), true);
  show(root.querySelector("#music-local-head"), false);
  // ⚠️ 页签的选中态**只能查 `#music-side-tabs` 里的**：`.music-tab` 这个类
  // 搜索详细页那排页签也在用（`data-act="search-tab"`），整个 root 查一遍会把
  // 那边刚点亮的那个页签抹掉（实测：开着搜索详细页时点一下左栏，搜索页签就全灭）。
  root.querySelectorAll<HTMLElement>("#music-side-tabs .music-tab").forEach(el => {
    el.classList.toggle("on", el.dataset.tab === sideTab);
  });
  const box = root.querySelector<HTMLElement>("#music-side-list");
  if (!box) return;
  if (!cfg?.connected) {
    setHtml(box, `<div class="music-empty">${t("music.playlists_login")}</div>`);
    return;
  }
  const items = sideCache.get(sideTab);
  const err = sideError.get(sideTab);
  // 两栏的**第一行是前端合成**的（它们不是某一类资源，没有对应的列表端点）：
  // 歌单栏 = 收藏夹（`/me/tracks`），最常听栏 = 最常听的歌曲（`/me/top/tracks`）。
  let html = sideTab === "playlist" ? likedRowHtml() : sideTab === "top" ? topTracksRowHtml() : "";
  if (sideLoading.has(sideTab)) {
    html += `<div class="music-empty">${t("music.loading")}</div>`;
  } else if (err) {
    // 失败**不留空数组**（那会被渲染成「这一栏是空的」，把真因藏掉）
    html += `<div class="music-empty music-empty-err">${esc(err)}</div>`;
  } else if (items) {
    html += items.length === 0
      ? `<div class="music-empty">${sideTab === "playlist" ? t("music.playlists_empty") : t("music.lib_empty")}</div>`
      : items.map(it => libRowHtml(sideTab, it)).join("");
  }
  setHtml(box, html);
}

/** 只把「主区正在看哪一项」的选中态同步到左栏（**不重建列表**）。
 *
 *  **为什么不能顺手调 `renderSide`**：那会把整个左栏的 `innerHTML` 换掉，两个后果 ——
 *  ① 滚动位置被打回顶部（用户点第 20 行会被弹回第 1 行）；
 *  ② 被点的那个节点当场被换掉，浏览器**不会再发 `dblclick`**（UI Events 要求两次
 *     click 落在同一个 target 上）⇒「双击歌单即播放」永远触发不了。
 *  选中态只是 class 的事，就该只动 class（与 `syncPlayingMarks` 同一个思路）。 */
function syncSideSelection(root: HTMLElement) {
  root.querySelectorAll<HTMLElement>("#music-side-list .music-pl").forEach(el => {
    const btn = el.querySelector<HTMLElement>(".music-pl-btn");
    const on =
      !!selected && btn?.dataset.kind === selected.kind && btn?.dataset.id === selected.id;
    el.classList.toggle("on", on);
  });
}

/** 主区。三种内容互斥：**搜索详细页 > 选中项详情 > 引导文案**；
 *  本地页是第四条、优先级最高（它与前三者不可能同时成立：一进本地页就把
 *  `selected` / `searchPage` 清掉，见 listeners 里的 `local-toggle`）。 */
function renderMain(root: HTMLElement) {
  if (localPage) { renderLocalMain(root); return; }
  const box = root.querySelector<HTMLElement>("#music-main");
  if (!box) return;
  if (searchPage && searchQuery) {
    setHtml(box, searchPageHtml());
    return;
  }
  if (!selected) {
    setHtml(box, `<div class="music-empty">${cfg?.connected ? t("music.pick_hint") : t("music.playlists_login")}</div>`);
    return;
  }
  const cover = selected.cover ? `<img src="${esc(selected.cover)}" alt="">` : "";
  const kindLabel = selected.kind === "liked"
    ? t("music.liked_songs")
    : selected.kind === "top-tracks"
      ? t("music.top_tracks")
      : selected.kind === "playlist"
        ? t("music.playlists")
        : selected.kind === "album"
          ? t("music.tab_albums")
          : selected.kind === "artist"
            ? t("music.tab_artists")
            // 「最常听」栏的条目就是歌手 —— 主区头上那行小字写栏目名
            // （用户是从「最常听」点进来的，比写「歌手」更有信息量）
            : selected.kind === "top"
              ? t("music.tab_top")
              : t("music.tab_shows");
  let list: string;
  if (mainLoading) {
    list = `<div class="music-empty">${t("music.loading")}</div>`;
  } else if (mainError) {
    list = `<div class="music-empty music-empty-err">${esc(mainError)}</div>`;
  } else if (!mainTracks || mainTracks.length === 0) {
    list = `<div class="music-empty">${t("music.playlist_empty")}</div>`;
  } else {
    list = mainTracks.map((tr, i) => trackRowHtml(tr, i, "main-track", String(i))).join("");
  }
  setHtml(box, `<div class="music-page">
    <div class="music-page-head">
      <span class="music-page-cover${selected.kind === "artist" ? " round" : ""}">${cover}</span>
      <div class="music-page-info">
        <span class="music-page-kind">${esc(kindLabel)}</span>
        <span class="music-page-name">${esc(selected.name || "?")}</span>
        <span class="music-page-sub">${esc(selected.subtitle)}</span>
        <div class="music-page-actions">
          <button class="music-btn music-primary" data-act="main-play-all">${SVG.play} ${t("music.play_all")}</button>
        </div>
      </div>
    </div>
    <div class="music-list">${list}</div>
  </div>`);
}

/** 搜索**详细页**（主区那一整页，带页签）。数据来自同一次 `spotify_search`。 */
function searchPageHtml(): string {
  const r = searchResult;
  const tabs: [typeof searchTab, string][] = [
    ["tracks", t("music.tracks")],
    ["artists", t("music.tab_artists")],
    ["albums", t("music.tab_albums")],
    ["playlists", t("music.playlists")],
    ["shows", t("music.tab_shows")],
  ];
  const tabHtml = tabs
    .map(([k, label]) => `<button class="music-tab${searchTab === k ? " on" : ""}" data-act="search-tab" data-tab="${k}">${esc(label)}</button>`)
    .join("");
  let body: string;
  if (searchLoading || !r) {
    body = `<div class="music-empty">${t("music.loading")}</div>`;
  } else if (searchTab === "tracks") {
    body = r.tracks.length
      ? r.tracks.map((tr, i) => trackRowHtml(tr, i, "main-track", String(i))).join("")
      : `<div class="music-empty">${t("music.search_empty")}</div>`;
  } else {
    const items = searchTab === "artists" ? r.artists : searchTab === "albums" ? r.albums : searchTab === "shows" ? r.shows : r.playlists.map(libItemOfPlaylist);
    // 歌单走的是 `PlaylistDto`，其余三类是 `LibraryItemDto` ⇒ 先归一成卡片需要的形状
    const cards = items.map(it => libCardHtml(searchTab === "playlists" ? "playlist" : searchTab === "artists" ? "artist" : searchTab === "albums" ? "album" : "show", it));
    body = cards.length ? `<div class="music-grid">${cards.join("")}</div>` : `<div class="music-empty">${t("music.search_empty")}</div>`;
  }
  return `<div class="music-page">
    <div class="music-page-info music-page-info-flat">
      <span class="music-page-kind">${t("music.search_result")}</span>
      <span class="music-page-name">${esc(searchQuery)}</span>
    </div>
    <div class="music-tabs">${tabHtml}</div>
    <div class="music-list">${body}</div>
  </div>`;
}

// ── 动作：左栏 / 主区 / 搜索 / 设备（2026-09-28 两栏改造后的唯一一套）──────

// ── 渲染：搜索下拉预览 / 设备弹层 ─────────────────────────────────

/** 搜索**下拉预览**（工具条下方那一小块）。
 *  它与「主区进搜索详细页」是**两件事**：预览是几行速览，详细页是整页带页签。
 *  两块**不互斥**：详细页开着时用户继续敲字，预览仍该跟着更新。 */
function renderSearchDD(root: HTMLElement) {
  const box = root.querySelector<HTMLElement>("#music-search-dd");
  if (!box) return;
  const on = !!cfg?.connected && searchOpen && !!searchQuery;
  show(box, on);
  if (!on) return;
  if (!searchResult) {
    setHtml(box, searchLoading ? `<div class="music-empty">${t("music.loading")}</div>` : "");
    return;
  }
  const r = searchResult;
  const rows: string[] = [];
  // 预览要的是「一眼看出搜到了什么」，不是穷举：歌曲 3 条 + 其余各 2 条
  r.tracks.slice(0, 3).forEach((tr, i) => rows.push(trackRowHtml(tr, i, "dd-track")));
  r.artists.slice(0, 2).forEach(it => rows.push(libRowHtml("artist", it)));
  r.albums.slice(0, 2).forEach(it => rows.push(libRowHtml("album", it)));
  r.playlists.slice(0, 2).forEach(p => rows.push(libRowHtml("playlist", libItemOfPlaylist(p))));
  if (!rows.length) rows.push(`<div class="music-empty">${t("music.search_empty")}</div>`);
  rows.push(`<button class="music-dd-all" data-act="search-all">${t("music.search_all")}</button>`);
  setHtml(box, rows.join(""));
}

/** 设备弹层。**列表里的「本机播放」那一行是我们自己起的 librespot**，
 *  所以要单独摆在最上面 —— 它不在 Spotify 的设备列表里（进程起来之后才会出现）。 */
function renderDevices(root: HTMLElement) {
  const box = root.querySelector<HTMLElement>("#music-dev-dd");
  if (!box) return;
  show(box, devicesOpen);
  if (!devicesOpen) return;
  const local = librespot;
  let html = `<div class="music-dd-head">${t("music.devices")}</div>`;
  if (local?.available && !local.has_credentials) {
    // **没有凭据 = 这个开关点了也没用**（2026-09-30）：librespot 不会成为一台已登录的
    // Connect 设备，设备列表里永远没有「Lunac」。所以这行换成一次性的登录入口。
    html += `<button class="music-dev-row" data-act="local-login">
      <span class="music-dev-ico">${SVG.power}</span>
      <span class="music-dev-main">
        <span class="music-dev-n">${t("music.local_login")}</span>
        <span class="music-dev-s">${t("music.local_login_hint")}</span>
      </span>
    </button>`;
  } else if (local?.available) {
    html += `<button class="music-dev-row${local.running ? " on" : ""}" data-act="local-toggle">
      <span class="music-dev-ico">${SVG.power}</span>
      <span class="music-dev-main">
        <span class="music-dev-n">${t("music.local_play")}</span>
        <span class="music-dev-s">${esc(local.path || "")}</span>
      </span>
    </button>`;
  } else {
    // 找不到可执行文件时**说清去哪填**，而不是给一个点了没反应的开关
    html += `<div class="music-dev-row disabled">
      <span class="music-dev-ico">${SVG.power}</span>
      <span class="music-dev-main">
        <span class="music-dev-n">${t("music.local_play")}</span>
        <span class="music-dev-s">${t("music.local_no_exe")}</span>
      </span>
    </div>`;
  }
  const list = devices ?? [];
  html += list.length
    ? list.map(d => `<button class="music-dev-row${d.active ? " on" : ""}" data-act="dev-pick" data-id="${esc(d.id)}">
        <span class="music-dev-ico">${SVG.speaker}</span>
        <span class="music-dev-main">
          <span class="music-dev-n">${esc(d.name || "?")}</span>
          <span class="music-dev-s">${esc(d.kind)}</span>
        </span>
      </button>`).join("")
    : `<div class="music-empty">${t("music.device_none")}</div>`;
  setHtml(box, html);
}

/** 每秒一次的「谁在播」标色。**只改 class**（整块重建会把选中态与滚动位置打掉）。 */
function syncPlayingMarks(root: HTMLElement) {
  const ctx = player?.context_uri ?? "";
  const cur = player?.track?.uri ?? "";
  root.querySelectorAll<HTMLElement>(".music-pl").forEach(el => {
    el.classList.toggle("playing", !!ctx && el.dataset.uri === ctx);
  });
  root.querySelectorAll<HTMLElement>(".music-tr").forEach(el => {
    el.classList.toggle("playing", !!cur && el.dataset.uri === cur);
  });
}

// ── 本地媒体库：渲染（2026-10-01）────────────────────────────────
// 宿主 `media_lib.rs` 负责一切磁盘与解析（目录扫描 / 标签 / 时长 / 封面 / SQLite 索引）。
// 这一层只做三件事：① 画；② 调那六个命令；③ 把封面绝对路径转成 asset URL。
//
// **为什么与 Spotify 那套渲染完全分开**：数据源、可用条件（不要求登录）、可做的动作
// （这里没有「播放」）都不一样。硬塞进 `SideKind` / `trackRowHtml` 会到处长出
// 「如果是本地就跳过这一段」的分支 —— 那是两套东西被缝在一起的样子。

/** 扫描时间只精确到分钟，不必用 locale 相关的 `toLocaleString`（它在不同语言下
 *  会变格式，而这里只是「上次扫过」的一个注记）。 */
function fmtScanAt(ms: number): string {
  if (!ms) return "";
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** 左栏那排按钮的显隐：**正常态三枚 / 编辑态两枚**（2026-10-01 用户口径）。
 *  单独一个函数而不是散在 renderLocalSide 里 —— 进 / 出编辑态时要立刻生效，
 *  而那时列表未必需要重建。 */
function renderLocalHead(root: HTMLElement) {
  // 编辑态里那枚 `＋` 是藏起来的 —— 它展开的浮层必须跟着一起关，
  // 否则会留下一块没有锚点、又点不掉的浮层（用户口径：编辑态只有「删除选中 / 退出并保存」）。
  if (mediaEditMode) addMenuOpen = false;
  const set = (sel: string, v: boolean) => show(root.querySelector(sel), v);
  set("#music-local-add", !mediaEditMode);
  set("#music-local-list", !mediaEditMode);
  set("#music-local-scan", !mediaEditMode);
  set("#music-local-del", mediaEditMode);
  set("#music-local-editdone", mediaEditMode);
  renderLocalAddMenu(root);
}

/** `＋` 那个二选一浮层（2026-10-01 用户口径：**目录与播放列表文件合成一个入口**）。
 *
 *  为什么要浮层而不是「一口气弹两个对话框」：`＋` 的语义是「往列表里加东西」，
 *  而加目录（落库 + 扫描）与开播放列表（只读、不入库）是**两件完全不同的事**
 *  —— 用户按下时自己也不知道会走哪条，所以必须让他先选。
 *  复用 `.music-dev-row` 那套行样式，与设备弹层同形，不另造视觉。 */
function renderLocalAddMenu(root: HTMLElement) {
  const box = root.querySelector<HTMLElement>("#music-local-add-dd");
  if (!box) return;
  show(box, addMenuOpen);
  if (!addMenuOpen) return;
  setHtml(box, `
    <div class="music-dd-head">${esc(t("music.local_add_head"))}</div>
    <button class="music-dev-row" data-act="media-add-dir">
      <span class="music-dev-ico">${SVG.folder}</span>
      <span class="music-dev-main">
        <span class="music-dev-n">${esc(t("music.local_add_dir"))}</span>
        <span class="music-dev-s">${esc(t("music.local_add_dir_hint"))}</span>
      </span>
    </button>
    <button class="music-dev-row" data-act="media-add-list">
      <span class="music-dev-ico">${SVG.list}</span>
      <span class="music-dev-main">
        <span class="music-dev-n">${esc(t("music.local_open_playlist"))}</span>
        <span class="music-dev-s">${esc(t("music.local_add_list_hint"))}</span>
      </span>
    </button>`);
}

/** 左栏一根目录。**编辑态下整行可勾选**（2026-10-01）—— 行尾那枚 `×` 已经撤掉：
 *  删除统一走「勾选 → 垃圾桶 → × 保存」三步，行内再放一个直接落盘的删除按钮
 *  等于绕开暂存，勾错了没有回头路（用户原话「先取消你的删除按钮前端」）。 */
function mediaRootRowHtml(r: MediaRootDto): string {
  const sub = r.missing
    ? t("music.local_missing")
    : [
        t("music.local_n_tracks", { n: String(r.tracks) }),
        r.last_scan_at ? fmtScanAt(r.last_scan_at) : t("music.local_never_scanned"),
      ].join(" · ");
  const sel = mediaEditMode && mediaSel.has(r.path);
  return `<div class="music-pl${mediaRootSel === r.path ? " on" : ""}${r.missing ? " err" : ""}${sel ? " sel" : ""}">
    <button class="music-pl-btn" data-act="media-root" data-path="${esc(r.path)}" title="${esc(r.path)}">
      <span class="music-pl-cover music-pl-dir">${SVG.folder}</span>
      <span class="music-pl-main">
        <span class="music-pl-name">${esc(r.name || r.path)}</span>
        <span class="music-pl-sub">${esc(sub)}</span>
      </span>
    </button>
  </div>`;
}

/** 左栏：一道「全部曲目」+ 每一根已添加的目录。 */
function renderLocalSide(root: HTMLElement) {
  show(root.querySelector("#music-side-tabs"), false);
  show(root.querySelector("#music-local-head"), true);
  renderLocalHead(root);
  const box = root.querySelector<HTMLElement>("#music-side-list");
  if (!box) return;
  // 「全部曲目」那一行按**已暂存删除之后**的目录集算，免得出现
  // 「左栏只剩 0 个目录、右上角还写着一共 120 首」这种自相矛盾。
  const shown = (mediaRoots ?? []).filter((r) => !mediaPendingDel.has(r.path));
  const allTracks = shown.reduce((n, r) => n + r.tracks, 0);
  let html = `<div class="music-pl${mediaRootSel === "" ? " on" : ""}">
    <button class="music-pl-btn" data-act="media-root" data-path="" title="${esc(t("music.local_all"))}">
      <span class="music-pl-cover music-pl-dir">${SVG.note}</span>
      <span class="music-pl-main">
        <span class="music-pl-name">${esc(t("music.local_all"))}</span>
        <span class="music-pl-sub">${esc(t("music.local_n_tracks", { n: String(allTracks) }))}</span>
      </span>
    </button>
  </div>`;
  if (mediaRootsLoading) {
    html += `<div class="music-empty">${t("music.loading")}</div>`;
  } else if (mediaRootsError) {
    html += `<div class="music-empty music-empty-err">${esc(mediaRootsError)}</div>`;
  } else if (mediaRoots && shown.length === 0) {
    // 注意判据用 `mediaRoots` 非空 + `shown` 为空：**编辑态里把最后一条勾掉之后**
    // 该显示「还没有添加目录」，而不是继续画一片空白。
    html += `<div class="music-empty">${esc(t("music.local_no_roots"))}</div>`;
  } else if (mediaRoots) {
    html += shown.map(r => mediaRootRowHtml(r)).join("");
  }
  setHtml(box, html);
}

/** 一条本地曲目。**点它 = 播这一首**（`data-act="local-track"` 由 `onListClick` 接）：
 *  整个可见列表就是这一轮的队列，顺序就是屏幕上的顺序（见 `localQueue`）。
 *  `data-path` 是「正在播的那一行」的凭据（本地没有 URI）；`title` 放完整路径 ——
 *  用户的文件常常同名，列表上那一行是认不出来的。 */
function mediaTrackRowHtml(tr: MediaTrackDto, i: number): string {
  // 封面是宿主落到 `temp\music-covers\` 的文件 ⇒ 必须 `convertFileSrc`
  // （WebView2 的 CSP `img-src` 没有 `file:`，见 main.ts 里同类用法）。
  const cover = tr.cover
    ? `<img src="${esc(convertFileSrc(tr.cover))}" alt="">`
    : `<span class="music-mt-ico">${SVG.note}</span>`;
  const sub = [tr.artist, tr.album].filter(Boolean).join(" · ");
  const playing = !!localPlayer?.active && localPlayer.path === tr.path;
  return `<button class="music-mt${playing ? " playing" : ""}" data-act="local-track" data-i="${i}" data-path="${esc(tr.path)}" title="${esc(tr.path)}">
    <span class="music-mt-i">${i + 1}</span>
    <span class="music-mt-cover">${cover}</span>
    <span class="music-mt-main">
      <span class="music-mt-n">${esc(tr.title || "?")}</span>
      <span class="music-mt-a">${esc(sub)}</span>
    </span>
    <span class="music-mt-d">${fmtMs(tr.duration_ms)}</span>
  </button>`;
}

/** 主区那一页：筛选框 + 扫描进度 + 曲目表。
 *
 *  **骨架只在「还没有」时建一次**：筛选框长在骨架里，每次跟着 `setHtml` 重建的话，
 *  用户每敲一个字输入框就被换掉 —— 焦点、光标位置、输入法组字全丢。之后只 patch
 *  `#music-scan` 与 `#music-mt-list` 两个内层节点（与 renderPlayer 同一条纪律）。 */
function renderLocalMain(root: HTMLElement) {
  const box = root.querySelector<HTMLElement>("#music-main");
  if (!box) return;
  if (!box.querySelector("#music-local-page")) {
    box.innerHTML = `<div class="music-local-page" id="music-local-page">
      <div class="music-local-head-row">
        <input class="music-local-q" id="music-mt-q" type="search" autocomplete="off" spellcheck="false"
               placeholder="${esc(t("music.local_filter_ph"))}" value="${esc(mediaQuery)}">
        <span class="music-list-chip hidden" id="music-list-chip">
          <span class="music-list-chip-n" id="music-list-chip-n"></span>
          <button class="music-pl-x" data-act="media-list-close" title="${esc(t("music.local_close_playlist"))}">${SVG.x}</button>
        </span>
        <!-- 重扫（2026-10-01 用户口径）：**只留图标**，「重扫」那两个可见字去掉 ——
             它右边紧挨着设置齿轮（见 placeGear），带文字那一行会挤。
             title 仍然留着（鼠标悬浮还能问出这是什么）；去掉的是**可见文字**。
             ⚠️ 本行属于**动态骨架**（#music-main 会被 renderMain 整块重写、回来时重建），
             所以它只能走 data-act 委托，直接 addEventListener 会在下次重建时失效。 -->
        <button class="music-ghost-btn music-icon-btn" data-act="media-scan" title="${esc(t("music.local_rescan"))}">${SVG.refresh}</button>
      </div>
      <div class="music-scan hidden" id="music-scan"></div>
      <!-- 视频的**画面层**（2026-10-01 用户第 8 条）。里面那个 video 元素由
           renderLocalVideo 现造现拆（离开 / 换曲时整块清掉，见那里的注释）。
           注：本段在模板字符串里，注释里一个反引号都不能出现（预检 #15 / #39 ⑩）。 -->
      <div class="music-video hidden" id="music-video"></div>
      <div class="music-list" id="music-mt-list"></div>
    </div>`;
  }
  // 播放列表模式：**筛选框收起来、换成那枚「正在看哪份播放列表」的标记**。
  // 筛选是在宿主的 SQL 里做的（只认媒体库），对一份临时打开的播放列表没有意义 ——
  // 留着一个敲了没反应的输入框比藏掉它更糟。
  show(root.querySelector("#music-mt-q"), !mediaList);
  const chip = root.querySelector<HTMLElement>("#music-list-chip");
  show(chip, !!mediaList);
  if (mediaList) {
    setText(
      root.querySelector("#music-list-chip-n"),
      `${mediaList.name} · ${t("music.local_n_tracks", { n: String(mediaTracks?.total ?? 0) })}`,
    );
  }
  renderLocalScan(root);
  renderLocalVideo(root);
  renderLocalList(root);
  // 骨架刚建好（或刚重建）时**当场**把齿轮摆过去 —— `syncHead` 那一刀排在 renderMain
  // 之前，光靠它会让齿轮在工具条上多停一拍（肉眼可见的一闪）。
  placeGear(root);
}

/** 扫描进度那一行（`#music-scan`）。三种形态互斥：**进行中 > 出错 > 上一次的收尾**。 */
function renderLocalScan(root: HTMLElement) {
  const box = root.querySelector<HTMLElement>("#music-scan");
  if (!box) return;
  const st = mediaScan;
  let html = "";
  if (st?.running) {
    const total = st.total;
    // 「还没数完」时 total 为 0 —— 进度条画 0%，文字换成不带分母的那一句
    // （写「0 / 0」是在报一个假的进度）。
    const pct = total > 0 ? Math.min(100, Math.round((st.scanned / total) * 100)) : 0;
    const txt = total > 0
      ? t("music.local_scanning", { a: String(st.scanned), b: String(total) })
      : t("music.local_scanning_n", { a: String(st.scanned) });
    html = `<div class="music-scan-row">
      <span class="music-scan-bar"><span class="music-scan-fill" style="width:${pct}%"></span></span>
      <span class="music-scan-txt">${esc(txt)}</span>
    </div>`;
  } else if (st?.error) {
    html = `<div class="music-scan-row music-scan-err">${esc(st.error)}</div>`;
  } else if (st?.done_at) {
    html = `<div class="music-scan-row music-scan-done">${esc(t("music.local_scan_done", {
      added: String(st.added),
      updated: String(st.updated),
      removed: String(st.removed),
      failed: String(st.failed),
    }))}</div>`;
  }
  setHtml(box, html);
  show(box, !!html);
}

/** 曲目表（`#music-mt-list`）。被 `MEDIA_LIMIT` 截断时**如实说明** ——
 *  「只看到 500 首」与「库里只有 500 首」是两回事。 */
function renderLocalList(root: HTMLElement) {
  const box = root.querySelector<HTMLElement>("#music-mt-list");
  if (!box) return;
  let html: string;
  if (mediaTracksLoading) {
    html = `<div class="music-empty">${t("music.loading")}</div>`;
  } else if (mediaTracksError) {
    html = `<div class="music-empty music-empty-err">${esc(mediaTracksError)}</div>`;
  } else if (!mediaList && mediaRoots && mediaRoots.length === 0) {
    html = `<div class="music-empty">${esc(t("music.local_add_hint"))}</div>`;
  } else if (!mediaTracks || mediaTracks.items.length === 0) {
    // 播放列表里一首都没解出来 ⇒ 说的话不一样：那不是「库里没有」，
    // 而是「这份列表指向的文件都不在了」（见 media_lib.rs 的 read_playlist）。
    html = `<div class="music-empty">${esc(mediaList ? t("music.local_list_empty") : t("music.local_empty"))}</div>`;
  } else {
    html = mediaTracks.items.map((tr, i) => mediaTrackRowHtml(tr, i)).join("");
    if (mediaTracks.total > mediaTracks.items.length) {
      html += `<div class="music-empty">${esc(t("music.local_more", {
        a: String(mediaTracks.items.length),
        b: String(mediaTracks.total),
      }))}</div>`;
    }
  }
  setHtml(box, html);
}

// ── 视频的**画面层**（2026-10-01 用户第 8 条）────────────────────────

/** 画面层：把当前这条视频的画面画在主区顶部。
 *
 *  **它是一块「跟随图层」，不是第二个播放器** —— 这条约束定下了整段代码的形状：
 *    · 播放的**唯一真相仍在宿主**（rodio 那条流负责出声、进度、时长、四档模式、
 *      音量、同时出声开关）。这里只做两件事：跟着 `playing` 起停、偏差超阈值时纠一次；
 *    · `<video>` **一直是静音的**（`muted`）：让它也出声就是两条独立时钟的回声，
 *      而且两个音量会各调各的（宿主那份由 `player_volume` 保管）；
 *    · 于是「视频」与「音频」在上一层完全同构 —— 播放条、队列、播放模式、
 *      「放完了自动下一首」一行都不用改（判据还是宿主那条流的 `active`）。
 *
 *  **为什么画面归 WebView 出、而不是宿主逐帧解码**：1080p60 的 RGBA 走过 IPC 是
 *  每秒几百 MB，那条路根本不通；而 WebView2 自带解码器与合成器，这正是它擅长的事。
 *  代价是**容器覆盖面 = Chromium 的覆盖面**（mp4 / m4v / mov；mkv、avi 不行 ——
 *  见宿主 `media_lib.rs` 的 `VIDEO_EXTS`，那张表为什么只收三种写在那边）。 */
function renderLocalVideo(root: HTMLElement) {
  const box = root.querySelector<HTMLElement>("#music-video");
  if (!box) return;
  const lp = localPlayer;
  const on = !!lp && lp.video && lp.path !== "";
  show(box, on);
  if (!on) {
    // **整块拆掉**，不是只藏起来：留一个还在解码的 video 在后台，既不必要
    // （画面都看不见了）又会让「换到下一首音频」时多占一份解码器。
    if (box.firstChild) box.innerHTML = "";
    return;
  }
  const src = convertFileSrc(lp.path);
  let v = box.querySelector<HTMLVideoElement>(".music-video-el");
  if (!v || v.dataset.src !== src) {
    box.innerHTML = `<video class="music-video-el" muted playsinline preload="auto"></video>`;
    v = box.querySelector<HTMLVideoElement>(".music-video-el");
    if (!v) return;
    v.dataset.src = src;
    v.src = src;
    // 元数据一到就对齐一次：不等这一下的话，第一拍会出现「画面从头放、声音已经在
    // 第 3 秒」——那是这个功能最容易给人「做坏了」印象的一瞬。
    v.addEventListener("loadedmetadata", () => syncVideoToHost(v!, lp), { once: true });
    // 解不开就**如实说**（编解码器 / 容器不认），不留一块沉默的黑框
    v.addEventListener("error", () => {
      setText(root.querySelector("#music-msg"), t("music.video_unsupported"));
    });
  }
  syncVideoToHost(v, lp);
}

/** 把画面拉到与宿主那条音频流一致（见 `renderLocalVideo` 头注释的三条约束）。
 *  **只碰画面，不碰声音** —— 这里一行都不会去动宿主。 */
function syncVideoToHost(v: HTMLVideoElement, lp: LocalPlayerDto) {
  if (lp.playing) {
    // muted 的元素不受自动播放策略限制（Chromium 对静音媒体一律放行），
    // 所以这里不会出现「被拦下」的分支 —— 但仍然吞掉 rejection：那是浏览器
    // 的播放入口在 media 还没就绪时的正常拒绝，不该冒成一条红字。
    if (v.paused) void v.play().catch(() => {});
  } else if (!v.paused) {
    v.pause();
  }
  // 目标位置 = 宿主报的那一拍 + 从那一拍到现在**流过的时间**（在放时才加）。
  // 少了这一项，画面最坏会稳定地落后一整个轮询周期（500ms，肉眼能看出来）。
  const elapsed = lp.playing ? Date.now() - localPlayerAt : 0;
  const target = (lp.position_ms + Math.max(0, elapsed)) / 1000;
  // `readyState < 1`（还没有元数据）时设 currentTime 会被忽略甚至抛错 ——
  // 那一下由上面那个 `loadedmetadata` 补上。
  if (v.readyState >= 1 && !v.seeking && Math.abs(v.currentTime - target) > VIDEO_DRIFT_S) {
    v.currentTime = target;
  }
}

/** 本地小窗长条（`#music-v-lplayer`）——2026-10-01 用户口径「本地文件播放的情况下
 *  也添加小窗模式」。
 *
 *  **数据只有一个来源：`localPlayer`**，与 `#music-lnowbar` 同源 ⇒ 这里不另记任何
 *  状态（同预检 #39 ⑩「同一个值只写一处」）。进度的手画规则也与那条栏逐字一致：
 *  `localSeekRatio >= 0`（用户正拖着）时**不许覆盖**，否则滑块会被下一拍拽回去。
 *
 *  **不在这个态里就立刻返回**：它被 `renderLocalNowbar` 每一拍都调一次，而绝大多数
 *  时候用户根本不在小窗里。 */
function renderLocalMini(root: HTMLElement) {
  if (mode !== "lplayer") return;
  const box = root.querySelector<HTMLElement>("#music-v-lplayer");
  const lp = localPlayer;
  if (!box || !lp || !lp.path) return;
  // 曲名优先取队列里那份元数据（有歌手 / 专辑 / 封面）；对不上就退回文件名 ——
  // 与 renderLocalNowbar 同一条口径（「在放却叫不出名字」比「名字难看」糟得多）。
  const tr = localQueue[localQueueIndex];
  const name = tr?.title || lp.path.split(/[\\/]/).pop() || lp.path;
  setText(box.querySelector("#music-lp-title"), name);
  setText(box.querySelector("#music-lp-sub"), tr ? [tr.artist, tr.album].filter(Boolean).join(" · ") : "");
  setHtml(
    box.querySelector("#music-lp-cover"),
    tr?.cover ? `<img src="${esc(convertFileSrc(tr.cover))}" alt="">` : SVG.note,
  );
  setHtml(box.querySelector("#music-lp-play"), lp.active && lp.playing ? SVG.pause : SVG.play);
  // 播放模式钮：与本地栏那枚共用同一个 `localMode`（前端自记，见它的注释），
  // 所以这里直接画，不需要回读宿主。
  const lmb = box.querySelector<HTMLElement>("#music-lp-mode");
  if (lmb) {
    setAttr(lmb, "data-mode", localMode);
    setAttr(lmb, "title", localModeTitle());
    setHtml(lmb, localMode === "repeat_all" || localMode === "repeat_one" ? SVG.repeat : SVG.shuffle);
  }
  const dur = lp.duration_ms;
  const ratio = localSeekRatio >= 0
    ? localSeekRatio
    : dur > 0 ? Math.min(1, lp.position_ms / dur) : 0;
  const seek = box.querySelector<HTMLElement>("#music-lp-progress");
  if (seek && localSeekRatio < 0) seek.style.setProperty("--seek-pct", `${ratio * 100}%`);
  // 时长报不出来（没有 Xing 头的 mp3）时只显示走过的位置，不画一条假的进度。
  setText(box.querySelector("#music-lp-time"), dur > 0
    ? `${fmtClock(lp.position_ms)} / ${fmtClock(dur)}`
    : fmtClock(lp.position_ms));
  const vol = box.querySelector<HTMLInputElement>("#music-lp-vol");
  if (vol && document.activeElement !== vol) {
    vol.value = String(Math.round(lp.volume * 100));
    paintVolume(vol, lp.volume * 100);
  }
}

/** 本地播放条（`#music-lnowbar`）。
 *
 *  **「本地页 + 有会话」时出现**：没有会话时它没有任何可控制的对象（与
 *  `syncHead` 里那条 Spotify 栏同一条纪律）。而**离开本地页会停掉本地播放**
 *  （见 `setLocalPage`）——所以「条在不在」与「有没有声音」始终一致，
 *  不会出现「界面都退出了、声音还在放」（那正是本仓最反感的状态，见 player.rs
 *  头注释里那条生命周期）。
 *
 *  ⚠️ **2026-10-03 用户口径改了这里的判据**：原来「同时出声」开着时，这条栏会
 *  跟着到 Spotify 页上（否则会留下一条没有界面的音频流）。用户现在的口径是
 *  **两条播放控制栏同时只能存在一个** —— 所以判据收回成「只看 `localPage`」，
 *  与上面那条 Spotify 栏（`!localPage`）**严格互斥**，两条永远不会同屏。
 *
 *  代价要说清：并列播放开着时，从本地页切到在线歌单**不会**停掉本机那条流
 *  （那正是这把开关的意思），于是它会短暂处于「在放、但当前这一页没有它的控制条」
 *  ——切回本地页就又能控制。这是用户明确要的取舍，不是漏判。 */
function renderLocalNowbar(root: HTMLElement) {
  // **画面层的刷新挂在这里**（2026-10-01 用户第 8 条）——放在最前面、早于下面两条
  // `return`：那两条一旦命中（栏不可见 / 没有会话），画面层恰恰也该跟着拆掉，
  // 排在它们后面就会「栏收掉了、画面还挂在那里空转」。
  // 为什么选这个函数而不是各调用点：换曲走的是 `startLocalPlay` → 这里，而这条栏
  // 是**每一个本地状态变化**的必经之路，挂上来就不会有哪条路径漏掉。
  renderLocalVideo(root);
  // **本地小窗（#music-v-lplayer）也挂在这里**，理由与上面那条完全相同：它画的也是
  // `localPlayer`，而这条栏是每一个本地状态变化的必经之路。它自己在开头判「是不是
  // 小窗态」，不是在那个态里就立刻返回 —— 所以挂这儿不花什么钱。
  renderLocalMini(root);
  const bar = root.querySelector<HTMLElement>("#music-lnowbar");
  if (!bar) return;
  const lp = localPlayer;
  const live = !!lp && lp.path !== "";
  // **常驻**（2026-10-03 用户口径）：在本地音乐页就显示，不再随「有没有在播」消失。
  // 判据与 `#music-nowbar` 共用同一处（`localBarVisible`）—— 两条永远不同屏，
  // 而左端那枚抽屉可以把另一条换出来（见 syncHead 的 setDrawer）。
  const on = localBarVisible();
  show(bar, on);
  if (!on) return;

  // 元数据只有一个来源：队列里那一条。有会话时它是当前曲；**没有会话时它是上次那首**
  // （从 LocalLastRecord 恢复，见 readLocalLast）。「在放却叫不出名字」比「名字难看」糟得多，
  // 所以两边都退回文件名。
  const tr = localQueue[localQueueIndex] ?? null;
  const path = (lp && live ? lp.path : "") || tr?.path || "";
  if (!tr && !live) {
    // 空态（栏常驻之后才有这一支）：没播过任何本地歌曲时给一句话，而不是一条空白盒。
    setText(bar.querySelector("#music-lnow-n"), t("music.local_bar_empty"));
    setText(bar.querySelector("#music-lnow-a"), "");
    setHtml(bar.querySelector("#music-lnow-cover"), SVG.note);
    setHtml(bar.querySelector("#music-l-play"), SVG.play);
    setText(bar.querySelector("#music-l-time"), "0:00");
    const bp0 = bar.querySelector<HTMLElement>("#music-lbp");
    if (bp0 && localSeekRatio < 0) bp0.style.setProperty("--seek-pct", "0%");
  } else {
    const name = tr?.title || path.split(/[\\/]/).pop() || path;
    setText(bar.querySelector("#music-lnow-n"), name);
    setText(bar.querySelector("#music-lnow-a"), tr ? [tr.artist, tr.album].filter(Boolean).join(" · ") : "");
    setHtml(
      bar.querySelector("#music-lnow-cover"),
      tr?.cover ? `<img src="${esc(convertFileSrc(tr.cover))}" alt="">` : SVG.note,
    );
    // 播放键：**没起过 / 暂停 / 放完 都画三角**。没有会话时它的含义是「从上次的位置续播」
    // （见 `toggleLocalPause`）—— 在本机播放里没有「暂停了一首已经放完的歌」这回事。
    setHtml(bar.querySelector("#music-l-play"), lp?.active && lp?.playing ? SVG.pause : SVG.play);

    // 进度：**拖动期间不覆盖用户的手**（`localSeekRatio >= 0` 时那条线由指针事件画）；
    // 没有会话时画上次存下的位置（`localResumeMs`）。
    const dur = (lp && live ? lp.duration_ms : 0) || tr?.duration_ms || 0;
    const pos = lp && live ? lp.position_ms : localResumeMs;
    const ratio = localSeekRatio >= 0
      ? localSeekRatio
      : dur > 0 ? Math.min(1, pos / dur) : 0;
    const bp = bar.querySelector<HTMLElement>("#music-lbp");
    if (bp && localSeekRatio < 0) bp.style.setProperty("--seek-pct", `${ratio * 100}%`);
    // 时长报不出来（没有 Xing 头的 mp3）时**不画一条假的进度**，只显示走过的位置。
    setText(bar.querySelector("#music-l-time"), dur > 0
      ? `${fmtClock(pos)} / ${fmtClock(dur)}`
      : fmtClock(pos));
  }

  // 没有可播放的东西时，除音量外一律置灰（音量是宿主全局值，与有没有会话无关）——
  // 「栏常驻」之后才会出现这个组合，不置灰会让人以为按了没反应。
  const hasItem = !!tr || live;
  for (const sel of ["#music-l-prev", "#music-l-next", "#music-l-mode", "#music-l-stop", "#music-l-play"]) {
    const el = bar.querySelector<HTMLButtonElement>(sel);
    if (el) el.disabled = !hasItem;
  }

  // 播放模式钮（2026-10-01 用户第 8 条）。状态来自 `localMode`（前端自记，见它注释）——
  // 与 Spotify 那枚的差别只在这里：那枚每拍回读宿主，这枚不会漂移，所以直接画。
  // 图标分配与 Spotify 那枚对齐：关 / 随机 → 随机图；循环列表 / 单曲循环 → 循环图
  // （单曲循环那个小「1」由 `.music-mode-btn[data-mode="repeat_one"]::after` 加）。
  const lmb = bar.querySelector<HTMLElement>("#music-l-mode");
  if (lmb) {
    setAttr(lmb, "data-mode", localMode);
    setAttr(lmb, "title", localModeTitle());
    setHtml(lmb, localMode === "repeat_all" || localMode === "repeat_one" ? SVG.repeat : SVG.shuffle);
  }

  // 音量滑条：只在**没在拖它**时同步（`document.activeElement` 判 ——
  // 免得每 500ms 一次的快照把用户正拖着的滑块弹回去）。
  const vol = bar.querySelector<HTMLInputElement>("#music-l-vol");
  if (vol && document.activeElement !== vol) {
    const v = lp ? lp.volume : Number(vol.value) / 100;
    vol.value = String(Math.round(v * 100));
    paintVolume(vol, v * 100);
  }
}

/** 把「正在放的那一行」标出来。与 Spotify 那侧 `syncPlayingMarks` 同形，
 *  只是凭据换成本地路径（本地没有 URI）。 */
function syncLocalMarks(root: HTMLElement) {
  const cur = localPlayer?.active ? localPlayer.path : "";
  root.querySelectorAll<HTMLElement>(".music-mt").forEach(el => {
    el.classList.toggle("playing", !!cur && el.dataset.path === cur);
  });
}

/** 重画本地页的两栏。**只在 `localPage` 时做** —— 这个函数会被那几个异步动作
 *  在落地后调用，而那时用户可能已经切回 Spotify 那一侧了；不判的话会把
 *  `#music-main` 直接换成本地页（把用户正在看的歌单曲目顶掉）。 */
function refreshLocal(root: HTMLElement) {
  if (!localPage) return;
  renderLocalSide(root);
  renderLocalMain(root);
  renderLocalNowbar(root);
}

// ── 本地媒体库：动作 ──────────────────────────────────────────────

/** 拉目录列表。失败**不留空数组**（那会被渲染成「一根目录都没有」，把真因藏掉）。 */
async function loadMediaRoots(root: HTMLElement) {
  mediaRootsLoading = true;
  mediaRootsError = "";
  refreshLocal(root);
  try {
    mediaRoots = await invoke<MediaRootDto[]>("media_roots");
  } catch (e) {
    mediaRootsError = String(e);
  } finally {
    mediaRootsLoading = false;
    refreshLocal(root);
  }
}

/** 拉当前目录 + 当前筛选词下的曲目。
 *
 *  **这一步同时是「回到媒体库那一侧」**：任何走这条路的动作（点左栏某一根目录、
 *  敲筛选框、重新扫描、移除目录）都意味着用户想看的不是那份临时打开的播放列表了，
 *  所以第一步就把 `mediaList` 清掉 —— 把「退出播放列表」散在五六个调用点上，
 *  迟早会漏掉一个，表现是「点了目录却还在显示播放列表」。 */
async function loadMediaTracks(root: HTMLElement) {
  mediaList = null;
  mediaTracksLoading = true;
  mediaTracksError = "";
  refreshLocal(root);
  try {
    mediaTracks = await invoke<MediaTracksDto>("media_tracks", {
      // 空串 ⇒ 传 null（宿主的 `Option<String>` 收 None = 全库 / 不筛选）
      root: mediaRootSel || null,
      query: mediaQuery || null,
      limit: MEDIA_LIMIT,
    });
  } catch (e) {
    mediaTracksError = String(e);
  } finally {
    mediaTracksLoading = false;
    refreshLocal(root);
  }
}

/** 停掉扫描进度轮询（幂等）。 */
function stopMediaPoll() {
  if (mediaScanTimer !== undefined) {
    window.clearInterval(mediaScanTimer);
    mediaScanTimer = undefined;
  }
}

/** 扫描期间每 `MEDIA_POLL_MS` 读一次进度快照。
 *
 *  **自停条件看 `root.isConnected`**：这条轮询不属于 `tick` 那条链（面板一关就没有
 *  任何东西会来清它），所以必须自己判 —— 面板关了 / 窗口被复用去装别的插件之后，
 *  这个 root 就脱离了文档。 */
function pollMediaScan(root: HTMLElement) {
  if (mediaScanTimer !== undefined) return;
  mediaScanTimer = window.setInterval(() => {
    void (async () => {
      if (!root.isConnected) { stopMediaPoll(); return; }
      try {
        const st = await invoke<ScanStatusDto>("media_scan_status");
        mediaScan = st;
        if (!st.running) {
          stopMediaPoll();
          // 扫完了：目录行的条数 / 时间变了，新入库的歌也该出现
          await loadMediaRoots(root);
          await loadMediaTracks(root);
        }
      } catch {
        stopMediaPoll();
      }
      refreshLocal(root);
    })();
  }, MEDIA_POLL_MS);
}

/** 开始扫描（`path` 空 = 全部目录）。宿主**立刻返回**，进度靠上面那条轮询。 */
async function startMediaScan(root: HTMLElement, path?: string) {
  try {
    mediaScan = await invoke<ScanStatusDto>("media_scan_start", { root: path || null });
  } catch (e) {
    const raw = String(e);
    note(root, raw.includes("ERR_SCAN_RUNNING") ? t("music.local_scan_busy") : raw);
    return;
  }
  refreshLocal(root);
  pollMediaScan(root);
}

/** 加一根目录：选目录 → 落库 → **顺手扫它一次**（用户加目录就是想看到歌）。 */
async function addMediaRoot(root: HTMLElement) {
  let picked: string | string[] | null = null;
  try {
    picked = await openDialog({ directory: true, multiple: false, title: t("music.local_add_dir") });
  } catch (e) {
    note(root, String(e));
    return;
  }
  if (typeof picked !== "string" || !picked) return;
  try {
    mediaRoots = await invoke<MediaRootDto[]>("media_add_root", { path: picked });
  } catch (e) {
    note(root, String(e));
    return;
  }
  mediaRootSel = pickAddedRoot(picked);
  refreshLocal(root);
  await startMediaScan(root, mediaRootSel || picked);
}

/** 刚加的那根目录**规范化之后的路径**。宿主会把尾分隔符去掉（盘符根例外），
 *  所以选中的那个 `picked` 与新返回的列表里的 `path` 不能直接 `===`。 */
function pickAddedRoot(picked: string): string {
  const norm = (s: string) => s.replace(/[\\/]+$/, "").toLowerCase();
  const want = norm(picked);
  return mediaRoots?.find(r => norm(r.path) === want)?.path ?? "";
}

/** 进编辑态（左栏头部那枚笔）。**进入时清空勾选**：上一次退出时已清过，
 *  这里再清一次是为了「进编辑态」这个动作本身幂等 —— 不管从哪里调进来，
 *  起点都是「一个都没勾」。 */
function enterMediaEdit(root: HTMLElement) {
  mediaEditMode = true;
  mediaSel.clear();
  renderLocalSide(root);
}

/** 编辑态里那枚垃圾桶：把勾选的行**摘出列表**。
 *
 *  ⚠️ **只改前端集合，不落盘** —— 用户口径是「× 退出并保存」，也就是
 *  在按下 × 之前，删错了都还能反悔。真正调宿主 `media_remove_root` 的是
 *  `saveMediaEdit`。 */
function stageMediaDelete(root: HTMLElement) {
  if (!mediaSel.size) return; // 一个都没勾 = 什么也不做（不是「清空整个列表」）
  for (const p of mediaSel) mediaPendingDel.add(p);
  mediaSel.clear();
  renderLocalSide(root);
}

/** 退出编辑态并**保存**：把暂存的删除逐条落盘。
 *
 *  **先退编辑态再落盘**：宿主那边正在扫描时 `media_remove_root` 会拒
 *  （`ERR_SCAN_RUNNING`），若等命令回来才切 UI，用户按了 × 要愣住一拍才知道成败。
 *  反过来先切、失败再用 `note` 报一声，界面永远是即时响应的。
 *
 *  逐条而不是批量：宿主那条命令就是单个 `path`（见 `media_lib` 的
 *  `media_remove_root`），一串删除里有一条失败不该把前面成功的回滚掉 ——
 *  失败的那条**留在列表里**（它还在 `mediaRoots` 中），用户看得见。 */
async function saveMediaEdit(root: HTMLElement) {
  const del = Array.from(mediaPendingDel);
  mediaEditMode = false;
  mediaSel.clear();
  mediaPendingDel.clear();
  renderLocalHead(root);
  if (!del.length) {
    renderLocalSide(root);
    return;
  }
  let err = "";
  for (const path of del) {
    try {
      mediaRoots = await invoke<MediaRootDto[]>("media_remove_root", { path });
    } catch (e) {
      const raw = String(e);
      err = raw.includes("ERR_SCAN_RUNNING") ? t("music.local_remove_busy") : raw;
    }
  }
  // 被删掉的正是当前选中的那根 ⇒ 退回「全部」，否则主区会停在一个已经不存在的目录上
  if (mediaRootSel && !(mediaRoots ?? []).some(r => r.path === mediaRootSel)) mediaRootSel = "";
  renderLocalSide(root);
  await loadMediaTracks(root);
  if (err) note(root, err);
}

/** 媒体库那条路的报错翻成人话（与 `localErrText` 同一分工）。 */
function mediaErrText(e: unknown): string {
  const raw = e instanceof Error ? e.message : String(e);
  if (raw.includes("ERR_NOT_PLAYLIST")) return t("music.local_not_playlist");
  return raw;
}

/** 打开一份播放列表文件（`.m3u` / `.m3u8` / `.pls`）。
 *
 *  **它不动媒体库**：宿主只读那个文件（可能指向任何地方，包括没添加过的目录），
 *  解析结果直接当作主区那批曲目用 —— 于是点播、队列、「正在播放」那一行标记
 *  一行都不用改（这三样只认 `mediaTracks.items` 与 `localQueue`）。
 *
 *  ⚠️ **刻意不写进 `mediaRootSel`**：播放列表不是一根目录，它没有 `last_scan_at`、
 *  也不会被扫描更新；把它塞进左栏那套会把「扫描」这些动作的语义搅乱。 */
async function openMediaPlaylist(root: HTMLElement) {
  let picked: string | string[] | null = null;
  try {
    picked = await openDialog({
      multiple: false,
      title: t("music.local_open_playlist"),
      filters: [{ name: "Playlist", extensions: ["m3u", "m3u8", "pls"] }],
    });
  } catch (e) {
    note(root, String(e));
    return;
  }
  if (typeof picked !== "string" || !picked) return;

  mediaTracksLoading = true;
  mediaTracksError = "";
  mediaTracks = null;
  refreshLocal(root);
  try {
    const dto = await invoke<MediaTracksDto>("media_playlist", { path: picked });
    mediaTracks = dto;
    mediaList = { path: picked, name: picked.split(/[\\/]/).pop() || picked };
    // 左栏的「某一根目录」选中态要退回去：主区现在显示的不是它了
    mediaRootSel = "";
  } catch (e) {
    mediaList = null;
    mediaTracksError = mediaErrText(e);
  } finally {
    mediaTracksLoading = false;
    refreshLocal(root);
  }
}

/** 关掉那份播放列表，回到媒体库那一批曲目。 */
function closeMediaPlaylist(root: HTMLElement) {
  mediaList = null;
  void loadMediaTracks(root);
}

// ── 本地**播放**：动作（2026-10-01）────────────────────────────────
// 出声、解码、设备全在宿主（`player.rs`）；这一层只做三件事：发命令、画那条控制栏、
// 管队列（**队列是这里的事** —— 宿主只认「当前这一首」，见 file 头 DTO 段注释）。

/** 宿主那三条拒因翻成人话。原样贴出来都带着一串完整路径与英文解码器名，
 *  用户看不出下一步该做什么（与 `errText` 同理，但那条是给 Spotify 的）。 */
function localErrText(e: unknown): string {
  const raw = e instanceof Error ? e.message : String(e);
  if (raw.includes("ERR_NO_AUDIO_DEVICE")) return t("music.local_no_device");
  if (raw.includes("找不到这个文件")) return t("music.local_file_missing");
  if (raw.includes("解不开") || raw.includes("打不开")) return t("music.local_decode_failed");
  return raw;
}

/** 起播一个文件（换曲走同一条路）。**先把队列的游标摆好、再发命令** ——
 *  反过来的话，那 500ms 一次的快照会先看到「放完了」，把队列再往前推一首。
 *
 *  `resumeMs > 0` = **从上次的位置续播**（2026-10-03「保留上次播放的记录」）：
 *  宿主的 `player_play` 总是从头开始，所以这里补一次 `player_seek`。seek 失败不算
 *  错误（有些容器不支持，见 `seekLocal`）—— 那就从头放。
 *
 *  返回 `false` = **没起起来**（重入被丢 / 宿主拒了，提示已经发过）。调用方据此
 *  决定要不要跟着切视图 —— 「小窗打开成一个空壳」就是这么来的（见 openLocalMini）。 */
async function startLocalPlay(root: HTMLElement, path: string, resumeMs = 0): Promise<boolean> {
  if (localBusy) return false;
  // 起本地之前先按「互斥」闸门停掉 Spotify（dualPlay 开着时它什么都不做）。
  // **放在 `localBusy` 之后**：这一下若因为重入被丢掉，就没必要去停对面。
  await enforceExclusive(root, "local");
  localBusy = true;
  try {
    localPlayer = await invoke<LocalPlayerDto>("player_play", { path });
  } catch (e) {
    note(root, localErrText(e));
    localBusy = false;
    return false;
  }
  localBusy = false;
  localResumeMs = 0;
  if (resumeMs > 0) {
    try {
      localPlayer = await invoke<LocalPlayerDto>("player_seek", { positionMs: Math.round(resumeMs) });
      localResumeMs = localPlayer.position_ms;
    } catch { /* 见上面那条：能放就放，不能跳就从头 */ }
  }
  // 起播也是「刚读到一份 position_ms」——时刻一并刷新，否则画面层会拿上一次
  // 快照的旧时刻去外推（换曲时表现为画面先跳到上一首的位置）。
  localPlayerAt = Date.now();
  // 记住「这一轮在放什么」（见 LocalLastRecord）——关面板 / 重启后回来还能续上。
  writeLocalLast();
  renderLocalNowbar(root);
  syncLocalMarks(root);
  pollLocalPlayer(root);
  return true;
}

/** 点一行本地曲目：**整批可见曲目就是这一轮的队列**，从这一首开始。
 *  （与 Spotify 那侧「点队列里某一首 = 从它开始重排队列」是同一种约定。） */
async function playLocalTrack(root: HTMLElement, i: number) {
  const items = mediaTracks?.items ?? [];
  const tr = items[i];
  if (!tr) return;
  localQueue = items.slice();
  localQueueIndex = i;
  // 换了播放线 ⇒ 随机历史跟着清（那些下标指的是**上一条**队列，见 localHistory）。
  localHistory = [];
  await startLocalPlay(root, tr.path);
}

/** 上一首 / 下一首 / 自动续播。
 *  `auto` 时走到头就是**静默停下**（用户没点任何东西，弹一句提示只是噪音）。
 *
 *  **播放模式在这里落地**（2026-10-01 用户第 8 条）——宿主那条命令没有模式参数，
 *  「下一首是谁」本来就只有这一层知道（队列在前端，见 `localQueue`）：
 *  · `shuffle`     —— 在队列里**随机挑一首，且不等于当前这首**（真正的随机是可能
 *                     随机到自己，表现上就是「按了下一首却什么都没变」）；
 *                     队列只有一首时无从挑选，退回原曲。
 *                     ⚠️ **「上一首」不随机**（2026-10-03 用户报的 bug）：它走
 *                     `localHistory` 出栈，回到**刚才真正在放的那一首**；栈空才退回随机。
 *  · `repeat_all`  —— **两端都绕回去**（下一首越过队尾 → 队首；上一首越过队首 → 队尾）。
 *  · `repeat_one`  —— **只对自动续播生效**：放完了就重放这一首；用户手点上一首 /
 *                     下一首时照常换曲（Spotify 的 repeat-one 也是这个语义），
 *                     否则那两枚按钮就形同虚设。
 *  · `off`         —— 走到头就停（用户点的给一句提示，自动的不给）。 */
async function advanceLocal(root: HTMLElement, step: number, auto: boolean) {
  const n = localQueue.length;
  if (n === 0) return;
  if (auto && localMode === "repeat_one") {
    await startLocalPlay(root, localQueue[localQueueIndex]?.path ?? "");
    return;
  }
  let next: number;
  if (localMode === "shuffle" && n > 1) {
    if (step < 0 && localHistory.length > 0) {
      // 上一首 = 出栈（下标一定在范围内：换队列时已清空，见 localHistory）
      next = localHistory.pop() as number;
    } else {
      do { next = Math.floor(Math.random() * n); } while (next === localQueueIndex);
      // 把「刚才这一首」压栈，供下一首 / 自动续播之后的「上一首」回退
      if (localQueueIndex >= 0) localHistory.push(localQueueIndex);
    }
  } else {
    next = localQueueIndex + step;
  }
  if (localMode === "repeat_all" || localMode === "shuffle") {
    next = ((next % n) + n) % n;   // 绕过一圈也能落回 [0, n)
  } else if (next < 0 || next >= n) {
    if (!auto) note(root, t("music.local_queue_end"));
    return;
  }
  localQueueIndex = next;
  await startLocalPlay(root, localQueue[next].path);
}

/** 播放键。三种情形：**在放 ⇒ 暂停**；**暂停 ⇒ 继续**；**放完了 / 停过 ⇒ 重播这一首**。
 *  第四种（栏常驻之后才有）：**没有会话** ⇒ 从上次的位置续播那一首。 */
async function toggleLocalPause(root: HTMLElement) {
  if (localBusy) return;
  const lp = localPlayer;
  // 没有会话 = 栏里显示的是**上次那首**（见 LocalLastRecord）⇒ 从上次的位置续上。
  if (!lp || lp.path === "") {
    const path = localQueue[localQueueIndex]?.path ?? "";
    if (!path) return;
    await startLocalPlay(root, path, localResumeMs);
    return;
  }
  if (!lp.active) {
    // 放完了（宿主的 `active` 已经是 false）⇒ 这一下是「再放一遍」
    await startLocalPlay(root, lp.path);
    return;
  }
  try {
    localPlayer = await invoke<LocalPlayerDto>(lp.playing ? "player_pause" : "player_resume");
  } catch (e) {
    note(root, localErrText(e));
    return;
  }
  localPlayerAt = Date.now();   // 与 startLocalPlay 同理：这份快照是「此刻」读到的
  renderLocalNowbar(root);
  if (localPlayer?.active) pollLocalPlayer(root);
}

/** 停止本地播放（**连音频设备一起放掉**，不是暂停）。 */
async function stopLocalPlayback(root: HTMLElement) {
  stopLocalPoll();
  // **不清队列、不清下标**（2026-10-03 用户口径：「保留上次播放的记录」）——
  // 栏常驻之后，停止后它要继续显示刚才那一首，按播放键能从上次的位置续上。
  // 位置必须在清会话**之前**落下来：`localPlayer` 一空就没地方读 position 了。
  if (localPlayer?.path) localResumeMs = localPlayer.position_ms;
  localSeekRatio = -1;
  // 停止 = 这条播放线结束 ⇒ 随机历史跟着清（见 localHistory）
  localHistory = [];
  // **先把本地状态清掉、把栏收掉，再等宿主那条回执**：关窗 / 切页的路上界面
  // 已经在拆了，让那条栏多挂半拍只会闪一下（`player_stop` 也不可能失败到
  // 需要界面重画 —— 宿主那边「本来就没在放」和「刚收掉」是同一个结果）。
  localPlayer = null;
  // 与上面同一步：**没有会话就没有意义了**，留着旧时刻只会让下一首开头的
  // 漂移量算成一个巨大的值（`Date.now() - 旧时刻`）。
  localPlayerAt = 0;
  // 把「停在哪一首、停在几分几秒」写下来（栏常驻之后这是它的内容来源）。
  writeLocalLast();
  // 本地小窗里「没有会话」等于那一页没内容了 ⇒ 退回默认面板（用户在小窗里按停止、
  // 或队列放到最后一首，都会走到这里）。**先切态、后重画**：反过来会先闪一下空壳长条。
  const wasMini = mode === "lplayer";
  if (wasMini) mode = "default";
  renderLocalNowbar(root);
  syncLocalMarks(root);
  if (wasMini) renderMode(root);
  try {
    await invoke<LocalPlayerDto>("player_stop");
  } catch { /* 与上面同：没有会话也是个正常结果 */ }
}

/** 「互斥」那把闸门（2026-10-01，用户第 7 条）：起一路之前，把另一路停下来。
 *
 *  `dualPlay` 打开时**它什么都不做** —— 那正是「两者可以同时播放」的全部含义：
 *  本机那条流在 `player.rs` 里、Spotify 那条在远端，本来就不冲突，共存不需要
 *  任何额外机制，只需要**别去停对方**。
 *
 *  `who` = 即将要出声的那一路。两个分支都刻意不抛错、不报提示：停对面失败
 *  （远端断网 / 本地已经没会话）不该拦住用户刚点的那一下起播。 */
async function enforceExclusive(root: HTMLElement, who: "local" | "spotify") {
  if (dualPlay) return;
  if (who === "local") {
    // **只在对面真的在放时才发指令**：每点一首本地歌都朝远端打一次 pause 是白打，
    // 而且没连 Spotify 时那是一条必然报错的请求。
    if (cfg?.connected && player?.playing) {
      try { await invoke("spotify_control", { action: "pause" }); } catch { /* 见上 */ }
    }
    return;
  }
  // 反过来：把本地那条流停掉（`player_stop` 会把音频设备一起放掉，见 player.rs）。
  // 判据用 `localPlayer` 而不是 `localPlayer.active` —— 暂停中的那条流也算「占着」，
  // 用户若在 Spotify 里按下播放，本机那条就不该还停在暂停态挂着。
  if (localPlayer) await stopLocalPlayback(root);
}

/** 跳转（松手才发）。宿主那边 seek 失败**不算错误**（有些容器不支持），
 *  它只是如实回报当前位置 —— 所以这里拿到什么就画什么。 */
async function seekLocal(root: HTMLElement, positionMs: number) {
  // 没有会话（栏常驻时的常态）⇒ 宿主没有东西可跳。只把「恢复起点」挪过去，
  // 按播放键就从这里续上 —— 否则拖一下会被下一拍弹回去，看起来像坏了。
  if (!localPlayer || localPlayer.path === "") {
    localResumeMs = Math.max(0, Math.round(positionMs));
    writeLocalLast();
    renderLocalNowbar(root);
    return;
  }
  try {
    localPlayer = await invoke<LocalPlayerDto>("player_seek", { positionMs });
  } catch {
    return;
  }
  localResumeMs = localPlayer.position_ms;
  renderLocalNowbar(root);
}

/** 音量。**值由宿主保管**（跨曲目、跨次播放都留着，见 player.rs 的 `VOLUME`）——
 *  这里发完就把回执画出来，不自留一份，免得两边各记一个数。 */
async function setLocalVolume(root: HTMLElement, volume: number) {
  try {
    localPlayer = await invoke<LocalPlayerDto>("player_volume", { volume });
  } catch {
    return;
  }
  renderLocalNowbar(root);
}

/** 停掉本地播放轮询（幂等）。 */
function stopLocalPoll() {
  if (localPollTimer !== undefined) {
    window.clearInterval(localPollTimer);
    localPollTimer = undefined;
  }
}

/** 播放期间每 `LOCAL_POLL_MS` 读一次宿主状态。
 *
 *  **自成一条表、不并进 `tick`**：那条一秒一拍、且只在连上 Spotify 时才打网络；
 *  本地播放与本机账号无关（不登录也该能听），进度也要求更快。
 *
 *  **自停条件看 `root.isConnected`**（与扫描那条轮询同理）：这条表不属于 `tick`
 *  那条链，面板一关没有任何东西会来清它。 */
function pollLocalPlayer(root: HTMLElement) {
  if (localPollTimer !== undefined) return;
  localPollTimer = window.setInterval(() => {
    void (async () => {
      if (!root.isConnected) { stopLocalPoll(); return; }
      // 拖动进度条 / 有命令在飞时**不读**：读了只会把用户手里的位置拽回去，
      // 或者在换曲的半路上再触发一次「下一首」。
      if (localSeekRatio >= 0 || localBusy) return;
      let st: LocalPlayerDto;
      try {
        st = await invoke<LocalPlayerDto>("player_status");
      } catch {
        return;   // 宿主那条命令永不失败；真读不到就等下一拍，别把表停了
      }
      if (!root.isConnected) { stopLocalPoll(); return; }
      // 「这一首放完了」：`active` 为假、但 `path` 还指着刚才那一首。
      // **必须与上一拍比 path** —— 否则「从没起过」（path 空）也会被当成放完了。
      const finished = !st.active && st.path !== "" && st.path === localPlayer?.path;
      localPlayer = st;
      // 有会话时「恢复起点」跟着真实位置走 —— 面板一关（`detach`）就把它存下来，
      // 于是下次进来按播放键是从**上次听的地方**续上，而不是从头。
      localResumeMs = st.position_ms;
      // **快照的读取时刻**（2026-10-01 用户第 8 条）。必须紧挨着读回、且在下面
      // `renderLocalNowbar`（它内部会刷画面层）之前 —— 画面层要用它把
      // `position_ms` 外推到「此刻」（宿主报的是读那一拍的值，见 localPlayerAt 注释）。
      localPlayerAt = Date.now();
      renderLocalNowbar(root);
      syncLocalMarks(root);
      if (finished) {
        stopLocalPoll();
        await advanceLocal(root, 1, true);
      }
    })();
  }, LOCAL_POLL_MS);
}

/** 筛选框防抖。与搜索框同一套理由：每敲一个字跨进程查一次库太浪费。 */
function scheduleMediaQuery(root: HTMLElement, q: string) {
  mediaQuery = q;
  if (mediaQueryTimer !== undefined) window.clearTimeout(mediaQueryTimer);
  mediaQueryTimer = window.setTimeout(() => {
    mediaQueryTimer = undefined;
    void loadMediaTracks(root);
  }, MEDIA_QUERY_DEBOUNCE_MS);
}

// ── 动作：左栏 / 主区 / 搜索 / 设备 ───────────────────────────────

/** 把左栏宽度写进 `.music-root` 上的 CSS 变量（`--music-side-w`）。
 *
 *  **只写变量、不写 `style.width`**：拖动时每帧都要调它，而宽度有两处定义者
 *  （这里是 JS、CSS 里是 `flex: 0 0 var(--music-side-w)`）—— 只保留 CSS 那一处，
 *  拖动改的永远是同一个值，不会出现「内联宽 300 但 flex 基准 320」这种叠加误差。
 *  夹取在这里做（`SIDE_W_MIN / SIDE_W_MAX`）：拖动与「重置回默认宽」都走这条，
 *  于是「越界值永远进不了状态」是这一处保证的。 */
function applySideWidth(root: HTMLElement) {
  sideWidth = Math.min(SIDE_W_MAX, Math.max(SIDE_W_MIN, Math.round(sideWidth)));
  root.style.setProperty("--music-side-w", `${sideWidth}px`);
}

/** 按 `kind + id` 找回一项。**三条来源都要找**（搜索预览 / 搜索详细页 / 左栏列表）——
 *  同一张卡片可能来自其中任一处，所以点击处理里一律用 `data-id` 反查，
 *  而不是把整段数据塞进 DOM（塞进去就会在重建时和状态不一致）。 */
function findItem(kind: MainKind, id: string): LibraryItemDto | null {
  if (kind === "liked") {
    return { id: "liked", uri: "", name: t("music.liked_songs"), cover: "", subtitle: "", total: liked?.total ?? 0 };
  }
  // 「最常听的歌曲」也是**前端合成**的一项（见 topTracksRowHtml）：左栏那一栏列的是歌手，
  // 这一行是插在最前面的「歌曲」入口，它不是 `sideCache` 里的任何一条。
  if (kind === "top-tracks") {
    return {
      id: "top-tracks", uri: "", name: t("music.top_tracks"),
      cover: "", subtitle: t("music.top_tracks_sub"), total: 0,
    };
  }
  const fromSearch = kind === "artist"
    ? searchResult?.artists
    : kind === "album"
      ? searchResult?.albums
      : kind === "show"
        ? searchResult?.shows
        : searchResult?.playlists.map(libItemOfPlaylist);
  return sideCache.get(kind)?.find(x => x.id === id) ?? fromSearch?.find(x => x.id === id) ?? null;
}

/** 拉某一类左栏列表。歌单走 `/me/playlists`，其余三类各一条端点 —— 都归一成 `LibraryItemDto`。 */
async function loadSide(root: HTMLElement, kind: SideKind) {
  if (!cfg?.connected || sideLoading.has(kind) || sideCache.has(kind)) return;
  sideLoading.add(kind);
  sideError.delete(kind);
  renderSide(root);
  try {
    const items = kind === "playlist"
      ? (playlists = await invoke<PlaylistDto[]>("spotify_playlists")).map(libItemOfPlaylist)
      : await invoke<LibraryItemDto[]>(SIDE_CMDS[kind]);
    sideCache.set(kind, items);
  } catch (e) {
    // 失败**不留空数组**（那会被渲染成「这一栏是空的」，把真因藏起来）。
    // 缺 `user-follow-read` 时「歌手」那栏就是那句 `Insufficient client scope` —— 要去重新登录。
    sideError.set(kind, errText(e));
  } finally {
    sideLoading.delete(kind);
    renderSide(root);
  }
}

/** 选中一项 ⇒ 主区去拉它的全部曲目。
 *
 *  **收藏夹是特例**：它没有「按 id 拉曲目」的端点（内容来自 `spotify_liked`），
 *  所以直接拿缓存里的那串；没缓存就先拉一次。 */
async function openItem(root: HTMLElement, kind: MainKind, id: string) {
  const it = findItem(kind, id);
  if (!it) return;
  searchPage = false;
  searchOpen = false;
  selected = { kind, id: it.id, uri: it.uri, name: it.name, cover: it.cover, subtitle: it.subtitle };
  mainTracks = null;
  mainError = "";
  if (kind === "liked") {
    mainTracks = liked?.items ?? null;
    mainError = likedError;
    mainLoading = !liked;
    renderSearchDD(root);
    syncSideSelection(root);
    renderMain(root);
    if (!liked) await loadLiked(root);
    return;
  }
  mainLoading = true;
  renderSearchDD(root);
  syncSideSelection(root);
  renderMain(root);
  try {
    // **「最常听的歌曲」与那四类不是一回事**：它不是某一类资源，`spotify_item_tracks`
    // 里没有它这一档（那是个按 id 拉的端点，这份数据没有 id）。所以单独走一条命令，
    // 形状与收藏夹完全同源（都是 `Vec<TrackDto>`，都只能逐条播）。
    mainTracks = kind === "top-tracks"
      ? await invoke<TrackDto[]>("spotify_top_tracks")
      // 其余（含「最常听」栏）走按 id 拉的那条 —— **kind 要过一次 `trackFetchKind()`**：
      // `sideCache` 里存的是栏名（`top`），而宿主只认资源种类（`artist`）。
      : await invoke<TrackDto[]>("spotify_item_tracks", { kind: trackFetchKind(kind), id: it.id });
  } catch (e) {
    mainError = errText(e);
  } finally {
    mainLoading = false;
    syncSideSelection(root);
    renderMain(root);
  }
}

/** 播一整项（左栏行的「播放全部」/ 详情页头那个按钮）。
 *
 *  **收藏夹与电台只能逐条播**：前者没有合法上下文，后者的 `spotify:show:` 不被
 *  `context_uri` 接受（见 music.rs 的 `LibraryItemDto.uri` 说明）。 */
async function playWholeItem(root: HTMLElement, kind: MainKind, id: string, fromIdx = 0) {
  const it = findItem(kind, id);
  if (!it) return;
  // 起 Spotify 之前先按「互斥」闸门停掉本地那条流（dualPlay 开着时它什么都不做）
  await enforceExclusive(root, "spotify");
  try {
    // 收藏夹、电台、最常听的歌曲**都只能逐条播**：前两个没有合法上下文
    // （见 `LibraryItemDto.uri` 的说明），第三个压根不是一类资源、没有 uri。
    if (kind === "liked" || kind === "show" || kind === "top-tracks") {
      const list = (kind === "liked" ? liked?.items : mainTracks) ?? [];
      if (!list.length) return;
      await invoke("spotify_play_uris", { uris: list.slice(Math.max(0, fromIdx)).map(tr => tr.uri), deviceId: playTarget || undefined });
    } else if (it.uri) {
      await invoke("spotify_play_context", { contextUri: it.uri, offsetUri: "", deviceId: playTarget || undefined });
    } else {
      return;
    }
  } catch (e) {
    note(root, errText(e));
  }
}

/** 播主区列表里的第 idx 首。**用上下文播**（这样 next / 队列就是这一整张列表），
 *  只有收藏夹与电台退回「从这首开始逐条播」。 */
async function playMainTrack(root: HTMLElement, idx: number) {
  if (!selected) return;
  const list = mainTracks ?? [];
  const tr = list[idx];
  if (!tr) return;
  await enforceExclusive(root, "spotify");
  try {
    if (selected.kind === "liked" || selected.kind === "show") {
      await invoke("spotify_play_uris", { uris: list.slice(idx).map(x => x.uri), deviceId: playTarget || undefined });
    } else {
      await invoke("spotify_play_context", { contextUri: selected.uri, offsetUri: tr.uri, deviceId: playTarget || undefined });
    }
  } catch (e) {
    note(root, errText(e));
  }
}

/** 让「播放在哪」就位，并记住它（2026-09-30）。
 *
 *  **只在面板打开 / 刚登录成功时调一次**：宿主那边可能要等 librespot 注册成设备
 *  （最多约 6 秒），放进每秒的轮询只会白白多打 Web API。
 *
 *  宿主的规则见 `music.rs::music_autoconfigure`：有活跃设备就认它；没有就看有没有
 *  官方客户端（桌面端 / 手机 / 音箱），有就转移过去；一个都没有才拉起 librespot。 */
async function ensurePlayTarget(root: HTMLElement, force = false) {
  if (!cfg?.connected) return;
  // **复用窗口**（2026-10-02）：见 `playTargetAt` 的注释。窗口内已有就位结果就别再打
  // 一轮设备接口（发布版会在桌面端在跑时反复自动打开本面板）。设备列表照旧刷新 ——
  // 宿主侧有快照，这一步几乎零成本。
  if (!force && playTarget && Date.now() - playTargetAt < PLAY_TARGET_TTL_MS) {
    await loadDevices(root);
    return;
  }
  // **并发合并**（2026-10-02）：见 `playTargetBusy` 的注释。后来者直接返回 ——
  // 正在飞的那一份结束时同样会把 `playTarget` / 设备列表铺好，不需要第二份。
  if (playTargetBusy) return;
  playTargetBusy = true;
  const wasRunning = !!librespot?.running;
  // 「正在加载」只在**真的在等**的时候出现（2026-09-30 用户要求）：
  // 宿主那四条规则里，只有第 ④ 条（要拉 librespot + 等它注册成设备）会等约 6 秒，
  // 前三条就是一次接口往返（几十到几百毫秒）。所以挂一个 600ms 的定时器 ——
  // 快路径根本看不到这句话，慢路径才显示。**不做成「无条件先显示」**：那样在
  // 一切正常的面板上每次打开都会闪一句「正在启用本机播放」，纯噪音。
  const slowHint = window.setTimeout(() => note(root, t("music.auto_starting")), 600);
  try {
    const r = await invoke<AutoconfigDto>("music_autoconfigure", { allowLocal: !localPlayOptOut });
    playTarget = r.device_id || "";
    playTargetAt = Date.now();
    // 五种要说话的情形，**顺序即优先级**（都只是底部那行一次性小字，后写会盖掉前一条）。
    if (r.source === "needs_login") {
      // 引擎在、但没有凭据 ⇒ 起来也不会出现在设备列表里（ai-spec §4.6 已知缺口 1）。
      // **不自动打开浏览器**：那是很打扰的动作，而且多半还差一步后台登记（Redirect URIs）。
      note(root, t("music.auto_need_login"));
    } else if (r.source === "local" && !r.device_id) {
      // 进程起来了，却没在设备列表里出现（有凭据但没在 6 秒内注册上，或注册失败的兜底）。
      // **必须如实说**：报「已启用」是谎，用户会以为能用。
      note(root, t("music.auto_local_unregistered"));
    } else if (r.started_local && !wasRunning) {
      note(root, t("music.auto_local_started"));
    } else if (r.source === "start_failed") {
      // 我们试着起了、没起来（缺 exe / 起不来）⇒ 让用户手动拉一下（2026-09-30 用户要求）。
      // **不能说成「没有可用设备」**：那句话会把用户引去开 Spotify 桌面端。
      note(root, t("music.auto_start_failed"));
    } else if (r.source === "none" && !localPlayOptOut) {
      note(root, t("music.auto_none"));
    }
  } catch (e) {
    // 失败不阻断面板：播放时宿主还会自己解析一次设备（见 `resolve_play_device`）。
    playTarget = "";
    playTargetAt = 0;
    if (String(e) !== "ERR_NOT_CONNECTED") note(root, errText(e));
  } finally {
    // 收掉「正在加载」那个定时器：它若在结果写完之后才响，就会把刚写好那句话盖掉。
    window.clearTimeout(slowHint);
    // 放开并发闸（**必须在 finally 里**：慢路会走 6 秒多，中途任何一条 return
    // 忘了清就会把之后所有的 `ensurePlayTarget` 全部挡掉，表现是「再也自动就位不了」）。
    playTargetBusy = false;
  }
  await loadDevices(root);
}

/** 把「还剩多久」说成人话。`Retry-After` 可能是 12 小时这种量级，秒数对用户没意义。
 *  **必须走 i18n**（单位要跟着语言走）—— 所以这里只做取整，单位交给词条。 */
function fmtWait(secs: number): string {
  if (secs <= 0) return "";
  if (secs >= 3600) {
    const h = Math.floor(secs / 3600);
    const m = Math.round((secs % 3600) / 60);
    return m > 0 ? t("music.wait_hm", { h: String(h), m: String(m) }) : t("music.wait_h", { h: String(h) });
  }
  if (secs >= 60) return t("music.wait_m", { m: String(Math.round(secs / 60)) });
  return t("music.wait_s", { s: String(Math.ceil(secs)) });
}

/** 距「最早可重试时刻」还剩多少秒（按**本地时钟**算 ⇒ 倒计时会自己往下走）。 */
function retryLeftSecs(): number {
  if (apiRetryAtMs <= 0) return 0;
  return Math.max(0, Math.round((apiRetryAtMs - Date.now()) / 1000));
}

/** 问宿主一次闸的状态（含 `Retry-After` 倒计时），并把「恢复」按钮跟着刷新。
 *  面板挂载时、以及刚吃 429 时各调一次。**纯本地 IPC，不打 Spotify**。 */
async function refreshApiState(root: HTMLElement) {
  try {
    const st = await invoke<SpotifyApiStateDto>("spotify_api_state");
    apiStopped = st.stopped;
    apiStopReason = st.reason;
    apiRetryAtMs = st.retry_at_ms;
  } catch {
    /* 查不到就当没拉闸：真拉着的话下一次请求会返回 ERR_SPOTIFY_STOPPED 再补记 */
  }
  renderResumeBtn(root);
}

/** 摆 / 收工具条那枚「恢复」按钮（`#music-resume`）。**只在 429 硬闸拉着时出现**。
 *
 *  宿主默认**停到 `Retry-After` 到期**（那是 Spotify 自己给的时间，实测可能是 12 小时
 *  量级），但**不拦**用户提前试 —— 这是用户 2026-10-03 明确要求保留的出口。
 *  所以按钮上带一个倒计时：让用户知道「现在试大概率还会撞」，试与不试由他决定。 */
function renderResumeBtn(root: HTMLElement) {
  const btn = root.querySelector<HTMLButtonElement>("#music-resume");
  show(btn, apiStopped);
  if (!btn) return;
  if (!apiStopped) {
    btn.textContent = t("music.resume");
    btn.title = "";
    return;
  }
  const wait = fmtWait(retryLeftSecs());
  // 没给 `Retry-After`（或已到期）时**不要凭空写一个倒计时** —— 那就成了我们猜的秒数。
  btn.textContent = wait ? t("music.resume_in", { wait }) : t("music.resume");
  btn.title = wait
    ? t("music.resume_hint", { wait, reason: apiStopReason })
    : apiStopReason;
}

/** 用户点「恢复」：清掉宿主的 429 硬闸，然后重新就位一次。
 *
 *  **只有这一条路能清闸** —— 宿主的闸是进程级 + 粘性的（见 music.rs `SPOTIFY_STOPPED`）。 */
async function resumeSpotify(root: HTMLElement) {
  try {
    await invoke("spotify_resume");
  } catch {
    /* 纯内存操作，几乎不会失败；真失败下一次 `spotify_api_state` 会把真相捞回来 */
  }
  apiStopped = false;
  apiStopReason = "";
  apiRetryAtMs = 0;
  renderResumeBtn(root);
  note(root, t("music.resumed"));
  // 放开之后立刻重跑一次自动就位：设备列表要重拉、播放态要重读。
  void ensurePlayTarget(root, true);
}

/** 拉设备列表 + 本机播放状态。**这个函数在打开弹层时调一次**，
 *  之后只有用户点了「本机播放」才再拉（设备列表不需要每秒刷新）。 */
async function loadDevices(root: HTMLElement) {
  try {
    devices = await invoke<DeviceDto[]>("spotify_devices");
  } catch (e) {
    devices = [];
    note(root, errText(e));
  }
  try {
    librespot = await invoke<LibrespotDto>("librespot_status");
  } catch {
    librespot = null;
  }
  renderDevices(root);
}

/** 开 / 关本机播放。
 *  开的时候**顺手把播放转到它**：librespot 起完要几秒才在设备列表里出现，
 *  所以起来后轮询几次找「名字叫 Lunac 的那台」再转移 —— 用户点一下就该听到声音。 */
async function toggleLocalPlay(root: HTMLElement) {
  if (!librespot?.available) return;
  const started = librespot.running;
  try {
    librespot = await invoke<LibrespotDto>(started ? "librespot_stop" : "librespot_start");
  } catch (e) {
    note(root, errText(e));
    return;
  }
  // 记住用户的意愿（2026-09-30）：**关掉之后重开面板不该再被自动拉起来**。
  localPlayOptOut = started;
  renderDevices(root);
  if (started) {
    // 关掉本机播放 ⇒ 原来那台设备就没了，交给宿主重新解析一次；此时 allowLocal 已是
    // false，所以宿主不会再把它拉起来（那正是用户刚表达的意思）。
    playTarget = "";
    playTargetAt = 0;
    await ensurePlayTarget(root, true);
    return;
  }
  for (let i = 0; i < 6; i++) {
    await new Promise(r => window.setTimeout(r, 1500));
    try {
      const list = await invoke<DeviceDto[]>("spotify_devices");
      devices = list;
      const mine = list.find(d => d.name === "Lunac");
      if (mine && !mine.active) {
        await invoke("spotify_transfer", { deviceId: mine.id, play: true });
      }
      if (mine) { playTarget = mine.id; break; }
    } catch {
      // 刚起来那几秒接口可能还没认到它，继续等
    }
  }
  await loadDevices(root);
}

/** librespot 首次登录（**一次性**，用户点弹层里那一行时调）。
 *
 *  为什么必须有它：没有凭据的 librespot 不是一台已登录的 Connect 设备（本机 mDNS 还不通），
 *  设备列表里永远没有「Lunac」⇒ 点歌条条 `404 NO_ACTIVE_DEVICE`。这是「桌面端不在时
 *  选不了歌」的另一半，见 ai-spec §4.6 已知缺口 1。
 *  librespot 自己会打开系统浏览器；授权完成后凭据落盘，**同一个进程继续以设备身份跑**。
 *
 *  轮询用的是 `librespot_status`（**只查本地那个 credentials.json，零网络**），
 *  拿到凭据后才去问一次设备列表并转移 —— 别拿 Web API 当登录状态的轮询器。 */
async function startLocalLogin(root: HTMLElement) {
  try {
    librespot = await invoke<LibrespotDto>("librespot_login");
  } catch (e) {
    note(root, errText(e));
    return;
  }
  localPlayOptOut = false;   // 用户主动启用了本机播放
  renderDevices(root);
  note(root, t("music.local_login_started"));
  // 最多等 2 分钟（浏览器里要登录/点授权，急不来）。面板关掉就提前退出。
  for (let i = 0; i < 60; i++) {
    await new Promise(r => window.setTimeout(r, 2000));
    if (!root.isConnected) return;
    let st: LibrespotDto;
    try {
      st = await invoke<LibrespotDto>("librespot_status");
    } catch {
      continue;
    }
    librespot = st;
    if (!st.has_credentials) continue;
    // 凭据落盘 ⇒ 授权完成。有的版本登完就退出 ⇒ 用正常方式再起一次，然后找那台设备。
    if (!st.running) {
      librespot = await invoke<LibrespotDto>("librespot_start").catch(() => st);
    }
    for (let j = 0; j < 8; j++) {
      await new Promise(r => window.setTimeout(r, 1500));
      if (!root.isConnected) return;
      const list = await invoke<DeviceDto[]>("spotify_devices").catch(() => [] as DeviceDto[]);
      devices = list;
      const mine = list.find(d => d.name === "Lunac");
      if (mine) {
        if (!mine.active) await invoke("spotify_transfer", { deviceId: mine.id, play: false });
        playTarget = mine.id;
        break;
      }
    }
    note(root, t("music.local_login_ok"));
    break;
  }
  await loadDevices(root);
}

/** 把播放转到设备列表里点中的那一台（`PUT /me/player`）。
 *
 *  **`play: false`**：Spotify 转移设备时「原本在播的继续播、原本暂停的保持暂停」——
 *  用户在这里表达的只是「换台设备」，不该顺手把歌放起来（那会覆盖他自己的暂停）。
 *  转完顺手关掉弹层：选中结果就在工具条那个按钮上（title + warn 态，见 renderStatus）。 */
async function pickDevice(root: HTMLElement, id: string) {
  if (!id) return;
  try {
    await invoke("spotify_transfer", { deviceId: id, play: false });
    devicesOpen = false;
  } catch (e) {
    note(root, errText(e));
  }
  await loadDevices(root);
}

// ── 收藏夹的动作 ──────────────────────────────────────────────────

/** 拉收藏夹。**它是左栏歌单栏的第一项**，主区那一页也用它，所以两处都要重画。
 *  `liked` 已经有了就直接返回（同一份数据重复打接口没有意义）—— 想强制重拉，
 *  先把它置 `null`（`music-disconnect` 那条路径就是这么做的）。 */
async function loadLiked(root: HTMLElement) {
  if (!cfg?.connected || likedLoading || liked) return;
  likedLoading = true;
  likedError = "";
  renderSide(root);
  try {
    liked = await invoke<LikedDto>("spotify_liked");
  } catch (e) {
    // 失败**不留空数组**（那会被渲染成「收藏夹是空的」，把真因藏起来）。
    // 缺 `user-library-read` 时这里就是那句 `Insufficient client scope` —— 要去重新登录。
    likedError = errText(e);
  } finally {
    likedLoading = false;
    if (selected?.kind === "liked") {
      mainTracks = liked?.items ?? null;
      mainLoading = false;
      mainError = likedError;
    }
    renderSide(root);
    renderMain(root);
  }
}

// ── 搜索的动作 ────────────────────────────────────────────────────

/** 输入防抖（`SEARCH_DEBOUNCE_MS`）；**清空输入立刻收起预览与详细页**，不等防抖。
 *  `searchOpen` 在这里置位：敲字就展开预览，预览的显隐由 `renderSearchDD` 一处决定。 */
function scheduleSearch(root: HTMLElement, q: string) {
  searchQuery = q;
  searchOpen = !!q;
  if (searchTimer !== undefined) { window.clearTimeout(searchTimer); searchTimer = undefined; }
  if (!q) {
    searchResult = null;
    searchLoading = false;
    // 词清空了，详细页的标题就没内容了 ⇒ 一起退掉（否则主区会剩一个空标题的页）
    searchPage = false;
    renderSearchDD(root);
    renderMain(root);
    return;
  }
  searchLoading = true;
  renderSearchDD(root);
  renderMain(root);
  searchTimer = window.setTimeout(() => void runSearch(root, q), SEARCH_DEBOUNCE_MS);
}

async function runSearch(root: HTMLElement, q: string) {
  searchTimer = undefined;
  try {
    const r = await invoke<SearchDto>("spotify_search", { query: q });
    if (searchQuery !== q) return; // 用户又改了词 ⇒ 这次的返回作废，别覆盖新的
    searchResult = r;
  } catch (e) {
    if (searchQuery !== q) return;
    searchResult = null;
    note(root, errText(e));
  } finally {
    if (searchQuery === q) {
      searchLoading = false;
      renderSearchDD(root);
      renderMain(root);
    }
  }
}

// ── 渲染：播放队列 ────────────────────────────────────────────────

function renderQueue(root: HTMLElement) {
  const items = queue?.items ?? [];
  // 「点一首 = 从它开始播」是 Spotify API 限制下的替代方案（没有删队列项的接口），
  // 所以每行的提示就写这件事，不写「移除」。
  const hint = t("music.play_from_here");
  const html = items.length === 0
    ? `<div class="music-empty">${queue ? t("music.queue_empty") : t("music.loading")}</div>`
    : items.map((tr, i) => trackRowHtml(tr, i, "play-uri", "", hint)).join("");
  // **两处同源**：播放长条下方那块（`#music-q-wrap`）与默认面板底部栏的浮层（`#music-q-dd`）。
  // 同一份队列同一套行（`play-uri`），不写第二份渲染。
  for (const sel of ["#music-q", "#music-q-dd"]) setHtml(root.querySelector(sel), html);
}

async function loadQueue(root: HTMLElement) {
  try {
    queue = await invoke<QueueDto>("spotify_queue");
    queueFor = player?.track?.id ?? "";
  } catch (e) {
    queue = null;
    setText(root.querySelector("#music-msg"), errText(e));
  }
  renderQueue(root);
}

// ── 渲染：播放界面 ────────────────────────────────────────────────

/** 把当前曲目 / 进度 / 按钮状态刷到**两条控制条**上。
 *
 *  **两处必须同源**：播放长条（`#music-*`）与默认面板底部栏（`#music-b*`）是同一份状态的
 *  两个视图，各自记一份必然漂移（同预检 #39 ⑩「同一个值只写一处」：进度只写 `--seek-pct`，
 *  图标只由这一处决定）。 */
function renderPlayer(root: HTMLElement) {
  const track = player?.track ?? null;
  const cover = track?.cover ? `<img src="${esc(track.cover)}" alt="">` : "";
  const sub = [track?.artists, track?.album].filter(Boolean).join(" · ") || (player?.active ? "" : t("music.no_device_hint"));

  setHtml(root.querySelector("#music-pv-cover"), cover);
  setHtml(root.querySelector("#music-bnow-cover"), cover);
  setText(root.querySelector("#music-pv-title"), track?.name || t("music.no_track"));
  setText(root.querySelector("#music-bnow-n"), track?.name || t("music.no_track"));
  setText(root.querySelector("#music-pv-sub"), sub);
  setText(root.querySelector("#music-bnow-a"), sub);

  const dur = track?.duration_ms ?? 0;
  const pos = Math.min(player?.progress_ms ?? 0, dur || Number.MAX_SAFE_INTEGER);
  const pct = dur > 0 ? Math.max(0, Math.min(100, (pos / dur) * 100)) : 0;
  // 记下插值基准：这一轮轮询的 `pos` 是在「现在」读到的，
  // 之后由 `paintProgressSmooth` 按真实流逝时间在它之上外推（见那里的说明）。
  progressBaseMs = pos;
  progressBaseAt = Date.now();
  // 拖动进度条期间**不覆盖**：用户手指下的位置不能被下一秒的轮询拽回去。
  // 位置只写 `--seek-pct` 一个变量 —— 填充 / 圆点 / 时间气泡都由 CSS 从它取值。
  if (seekRatio < 0) {
    for (const el of seekBars) el.style.setProperty("--seek-pct", `${pct}%`);
  }
  // 时间一律走 `fmtClock`（mm:ss）—— 这三处（长条的一对、底部栏那一对）都是
  // 「紧挨着进度条的数字」，用户要的就是这个形态，见 fmtClock 的说明。
  setText(root.querySelector("#music-pv-pos"), fmtClock(pos));
  setText(root.querySelector("#music-pv-dur"), fmtClock(dur));
  setText(root.querySelector("#music-b-time"), `${fmtClock(pos)} / ${fmtClock(dur)}`);

  const playIcon = player?.playing ? SVG.pause : SVG.play;
  setHtml(root.querySelector("#music-play"), playIcon);
  setHtml(root.querySelector("#music-b-play"), playIcon);

  // 三态按钮：**状态一律从宿主回读**（`shuffle` + `repeat`），不自己记
  // 「我上次设成了什么」—— 用户在 Spotify 客户端里改了就会漂移。
  const m = playMode();
  const modeTitle = m === "shuffle" ? t("music.mode_shuffle") : m === "repeat_one" ? t("music.mode_repeat_one") : t("music.mode_off");
  for (const btn of [root.querySelector<HTMLElement>("#music-mode"), root.querySelector<HTMLElement>("#music-b-mode")]) {
    if (!btn) continue;
    setAttr(btn, "data-mode", m);
    setAttr(btn, "title", modeTitle);
    setHtml(btn, m === "repeat_one" ? SVG.repeat : SVG.shuffle);
  }

  const volume = player && player.volume_percent >= 0 ? player.volume_percent : -1;
  for (const vol of [root.querySelector<HTMLInputElement>("#music-vol"), root.querySelector<HTMLInputElement>("#music-b-vol")]) {
    // 用户正抓着音量条时不要抢（`document.activeElement` 也是拉条本身）
    if (vol && volume >= 0 && document.activeElement !== vol) {
      vol.value = String(volume);
      // 值写了就得把**填充**一起重画（条子只有这一个元素，见 paintVolume）
      paintVolume(vol, volume);
    }
  }

  // 「有没有活跃设备」只该管播放键：**设备按钮与「正在播放」那一块不在此列** ——
  // 没有活跃设备时恰恰要用它们（去挑一台设备 / 进去看播放面板）。
  // ⚠️ 选择器必须是 `#music-nowbar`，**不能是 `.music-nowbar`** —— 本地那条播放条
  // 也带着 `music-nowbar` 这个类（`#music-lnowbar`），用类选择器会把它一起 disable：
  // 「没连 Spotify」时本地那条栏的按钮全都点不动（本地播放本来就与账号无关）。
  const disabled = !player?.active;
  // ⚠️ 选择器必须把范围**限定在远端那条长条里**（`#music-v-player .music-bar`）：
  // 本地小窗（`#music-v-lplayer`）用的是同一套 `.music-bar` 类，而它的按钮**不归
  // Spotify 的活跃状态管**（本地播放与账号无关）—— 用类选择器会把它们一起 disable，
  // 表现是「小窗里一排键全是点不动的灰」。
  root.querySelectorAll<HTMLButtonElement>("#music-v-player .music-bar button, #music-nowbar button").forEach(b => {
    // 抽屉**不在此列**：没有活跃设备时恰恰要能靠它换到本地那条栏（见 setDrawer）。
    if (b.id === "music-devices-btn" || b.id === "music-bnow" || b.id === "music-b-drawer") return;
    b.disabled = disabled;
  });
}

/** 按**真实流逝时间**把进度条推到「此刻应有的位置」—— 一次网络都不打。
 *
 *  **为什么需要这层本地插值**（用户 2026-09-29 报「进度条一跳一跳」）：
 *  宿主那轮轮询的真实周期是「Web API 往返 + `STATUS_POLL_MS`」（1s → 3s → 15s、隐藏时更久，
 *  见 `STATUS_POLL_MS`）⇒ 只靠它写 `--seek-pct`，用户看到的是「跳一格、僵一段、再跳一格」。
 *  这里用一个 200ms 的本地定时器（明显快于 CSS 的 `0.3s` 补间，于是补间首尾相接）
 *  在两次轮询之间把位置推着走；每轮轮询回来由 `renderPlayer` 重新对齐基准。
 *  **纯本地计算，不增加任何 Web API 调用** —— 提速不提负载。
 *
 *  只在「正在播 + 有曲目 + 没在拖」时推：暂停 / 拖动时位置本就该定住。 */
function paintProgressSmooth(root: HTMLElement) {
  if (!root.isConnected) return;
  if (!player?.playing || seekRatio >= 0) return;
  const dur = player.track?.duration_ms ?? 0;
  if (dur <= 0) return;
  const pos = Math.min(progressBaseMs + (Date.now() - progressBaseAt), dur);
  const pct = Math.max(0, Math.min(100, (pos / dur) * 100));
  for (const el of seekBars) el.style.setProperty("--seek-pct", `${pct}%`);
  setText(root.querySelector("#music-pv-pos"), fmtClock(pos));
  // 底部栏那对数字也要跟着走 —— 它和填充条是同一个「此刻放到哪儿」的两种表达，
  // 只推一个的话（填充在动、数字不动）用户会以为卡了。
  setText(root.querySelector("#music-b-time"), `${fmtClock(pos)} / ${fmtClock(dur)}`);
}

function playMode(): "off" | "shuffle" | "repeat_one" {
  if (player?.repeat === "track") return "repeat_one";
  return player?.shuffle ? "shuffle" : "off";
}

/** 播放列表（队列）面板的显隐。
 *  **歌词不再与它互斥**（2026-09-27 改）：播放界面是定尺 130 的长条，塞不进队列，
 *  所以队列改挂在长条**下方**，两者可以同时存在 —— 旧版那套「谁占封面下面那一格」
 *  的互斥是 450 宽旧布局的产物。 */
function syncPlayerBody(root: HTMLElement) {
  show(root.querySelector("#music-q-wrap"), queueOpen);
  show(root.querySelector("#music-q-dd"), queueOpen);
  root.querySelector("#music-queue-btn")?.classList.toggle("on", queueOpen);
  root.querySelector("#music-b-queue")?.classList.toggle("on", queueOpen);
}

/** 重建歌词区。**只在数据变化 / 进播放界面时调用** —— 它整块换 DOM，
 *  放进每秒一次的 `tick` 会把滚动位置一起打掉。 */
function renderLyrics(root: HTMLElement) {
  const box = root.querySelector<HTMLElement>("#music-lyr");
  if (!box) return;
  cancelGlide();
  activeLine = -1;
  lrcLines = [];
  // 换歌后不再挂起跟随（上一首的「用户正在看」与本首无关）
  if (followSuspended) {
    followSuspended = false;
    if (followTimer !== undefined) { window.clearTimeout(followTimer); followTimer = undefined; }
  }

  if (lyricsLoading) {
    setHtml(box, `<div class="music-empty">${t("music.lyrics_loading")}</div>`);
    return;
  }
  if (lyrics?.synced) {
    const lines = parseLrc(lyrics.synced);
    if (lines.length > 0) {
      lrcLines = lines;
      // **整段都进 DOM**（用户能滚轮 / 拉条翻到底），着色交给 `syncLyrics`：
      // 已过 + 正在 = `.done`（亮），未到 = 无类（暗）。点击某行 = 跳到那一句。
      const hint = esc(t("music.seek_line"));
      setHtml(
        box,
        `<div class="music-lyr-lines">${lines
          .map((l, i) => `<div class="music-lyr-line" data-i="${i}" title="${hint}">${esc(l.text)}</div>`)
          .join("")}</div>`,
      );
      return;
    }
  }
  const plain = (lyrics?.plain || "").trim();
  if (plain) {
    setHtml(box, `<div class="music-lyr-plain">${esc(plain)}</div>`);
    return;
  }
  if (lyrics?.instrumental) {
    setHtml(box, `<div class="music-empty">${t("music.lyrics_instrumental")}</div>`);
    return;
  }
  setHtml(box, `<div class="music-empty">${t("music.lyrics_none")}</div>`);
}

/** 取消在飞的歌词缓动。 */
function cancelGlide() {
  if (glideRaf) { cancelAnimationFrame(glideRaf); glideRaf = 0; }
}

/** 缓动滚到目标位置（`LYRIC_SCROLL_MS` + easeOutQuint）。
 *  **为什么不用 `scroll-behavior: smooth`**：它的时长不可控（用户嫌太快），
 *  而且与 rAF 叠加会互相打架 —— CSS 平滑会接管每一帧 `scrollTop` 的赋值，
 *  一次滚动变成两段动画（走起来一顿一顿的）。 */
function glideTo(box: HTMLElement, top: number) {
  cancelGlide();
  const from = box.scrollTop;
  const delta = top - from;
  if (Math.abs(delta) < 1) { box.scrollTop = top; return; }
  let t0 = 0;
  const step = (ts: number) => {
    if (!t0) t0 = ts;
    const p = Math.min(1, (ts - t0) / LYRIC_SCROLL_MS);
    box.scrollTop = from + delta * (1 - Math.pow(1 - p, 5)); // easeOutQuint：收尾极缓
    glideRaf = p < 1 ? requestAnimationFrame(step) : 0;
  };
  glideRaf = requestAnimationFrame(step);
}

/** 跟着进度走：**着色 + 把当前行滚到顶部**。
 *
 *  着色规则（用户 2026-09-27 定）：**已过 + 正在 = 亮，未到 = 暗**。
 *  只动「变化的那一段」的 class（正常播放就是 1 行）—— 整段重扫在几百行的长歌词里
 *  每秒跑一次纯属浪费，而且多写 DOM 会打断正在跑的滚动缓动。
 *  **挂起跟随只影响滚动、不影响着色**：否则用户滚一下歌词，全篇颜色会一起冻 4 秒。 */
function syncLyrics(root: HTMLElement) {
  if (lrcLines.length === 0) return;
  const box = root.querySelector<HTMLElement>("#music-lyr");
  if (!box || !box.isConnected || box.clientHeight === 0) return;
  const pos = (player?.progress_ms ?? 0) / 1000;
  let idx = -1;
  for (let i = 0; i < lrcLines.length; i++) {
    if (lrcLines[i].at <= pos) idx = i;
    else break;
  }
  if (idx === activeLine) return;
  // 从 min 到 max 之间只有这一段的状态会变（`activeLine = -1` 时从第 0 行起）
  const lo = Math.min(activeLine, idx);
  const hi = Math.max(activeLine, idx);
  for (let i = Math.max(0, lo + 1); i <= hi; i++) {
    box.querySelector<HTMLElement>(`.music-lyr-line[data-i="${i}"]`)?.classList.toggle("done", i <= idx);
  }
  box.querySelector<HTMLElement>(".music-lyr-line.on")?.classList.remove("on");
  activeLine = idx;
  const cur = box.querySelector<HTMLElement>(`.music-lyr-line[data-i="${idx}"]`);
  if (!cur) {
    if (!followSuspended) glideTo(box, 0);
    return;
  }
  cur.classList.add("on");
  if (followSuspended) return; // 用户正自己翻：只上色，不抢滚动
  // 用**矩形差**算该行在滚动内容里的位置（不用 `offsetTop`：它相对的是最近的定位祖先，
  // 外层多包一层就会算错；也不能用 `scrollIntoView` —— 那会把整个面板一起滚）。
  const lineTop = cur.getBoundingClientRect().top - box.getBoundingClientRect().top + box.scrollTop;
  glideTo(box, Math.max(0, lineTop));
}

/** 用户手动看歌词（滚轮 / 拖滚动条）⇒ 挂起自动跟随，`FOLLOW_RESUME_MS` 后自己回来。
 *  只认 `wheel` 与 `pointerdown`，**不听 `scroll`** —— 自动滚动也会发 scroll 事件，
 *  那样一滚就会被自己挂起。 */
function suspendFollow(root: HTMLElement) {
  followSuspended = true;
  cancelGlide(); // 用户接手 ⇒ 立刻停掉在飞的缓动，别跟他抢滚动条
  if (followTimer !== undefined) window.clearTimeout(followTimer);
  followTimer = window.setTimeout(() => {
    followTimer = undefined;
    followSuspended = false;
    activeLine = -1; // 强制下一轮重新定位并滚回当前行
    syncLyrics(root);
  }, FOLLOW_RESUME_MS);
}

// ── 数据流 ────────────────────────────────────────────────────────

async function refreshConfig(root: HTMLElement) {
  try {
    cfg = await invoke<MusicConfigDto>("music_config_get");
  } catch (e) {
    setText(root.querySelector("#music-msg"), errText(e));
    return;
  }
  if (!cfg.connected && mode === "player") {
    // 令牌没了（或在别处断开）⇒ 播放界面没有意义，拉回默认界面让用户重新登录
    mode = "default";
    renderMode(root);
  }
  renderSetup(root);
  renderStatus(root);
}

/** 抓当前曲目的歌词。**同一首只抓一次**（用户已明确不要手搜，所以这里没有覆盖路径）。 */
async function fetchLyricsFor(track: TrackDto) {
  if (lyricsFor === track.id) return;
  lyricsFor = track.id;
  lyricsLoading = true;
  lyrics = null;
  const root = currentRoot;
  if (root) renderLyrics(root);
  try {
    lyrics = await invoke<LyricsDto>("lyrics_get", {
      title: track.name,
      artist: track.artists,
      album: track.album,
      duration: track.duration_ms / 1000,
    });
  } catch (e) {
    lyrics = { found: false, instrumental: false, track: "", artist: "", album: "", duration: 0, synced: null, plain: null };
    if (root) setText(root.querySelector("#music-msg"), errText(e));
  } finally {
    lyricsLoading = false;
    if (root) { renderLyrics(root); syncLyrics(root); }
  }
}

async function tick(root: HTMLElement, myGen: number) {
  if (myGen !== gen) return;
  // 关面板 / 切插件后 root 会脱离文档 ⇒ 自停（不必依赖外部调用清表）
  if (!root.isConnected) { stopPolling(); return; }

  // 一次性提示的回收放在网络那一段**之外**：未登录时也要能把它按时清掉。
  sweepMsg(root);

  if (cfg?.connected) {
    // **429 硬闸拉着**（宿主进程级，见 `apiStopped`）：不再打**任何** Spotify 请求 ——
    // `spotify_status` / `loadQueue` 都会被宿主当场挡回，白跑一趟还会刷提示。
    // 歌词走的是 LRCLIB，与 Spotify 无关，**照常拉**（所以下面那句在 if 之外）。
    if (!apiStopped) {
      // 窗口隐藏时不打网络（用户看不见，白耗流量和 Spotify 配额），但表继续走。
      if (!document.hidden && !pollBusy) {
        pollBusy = true;
        try {
          player = await invoke<PlayerDto>("spotify_status");
          if (myGen !== gen) return;
          renderStatus(root);
          renderPlayer(root);
          syncLyrics(root);
          syncPlayingMarks(root);
          syncHead(root);
          // 每次 tick 一次「按内容贴合尺寸」：内容变了（提示行换行 / 队列面板的首屏）
          // 或上一次因窗口最小化被拒，都会在这里自己补上。同值会被 `lastResize` 挡掉，
          // 所以不会与用户手动拖出来的尺寸打架。
          scheduleResize(root);
          // 「恢复」按钮上的倒计时得跟着时间走（`Retry-After` 短到几十秒时也要准）。
          // 只在拉闸时算 —— 正常态这个函数只做一次 `show(btn,false)`。
          if (apiStopped) renderResumeBtn(root);
        } catch (e) {
          if (myGen !== gen) return;
          // 令牌失效（宿主已清空）⇒ 回落到配置态，让用户重新登录
          if (String(e).includes("ERR_NOT_CONNECTED")) {
            await refreshConfig(root);
          } else {
            setText(root.querySelector("#music-msg"), errText(e));
            // `errText` 会把 429 哨兵记进 `apiStopped` ⇒ 立刻把「恢复」按钮摆出来。
            renderResumeBtn(root);
            // 刚拉闸时顺手把 `Retry-After` 倒计时捞回来（纯本地 IPC，不打 Spotify）。
            if (apiStopped) void refreshApiState(root);
          }
        } finally {
          pollBusy = false;
        }
      }
      // 队列只在「面板开着 + 换歌了」时重拉：它每拉一次就是一次 Web API 调用
      if (queueOpen && player?.track && player.track.id !== queueFor) void loadQueue(root);
    }
    if (player?.track) void fetchLyricsFor(player.track);
  }
  if (myGen !== gen) return;
  pollTimer = window.setTimeout(() => void tick(root, myGen), STATUS_POLL_MS);
}

function stopPolling() {
  if (pollTimer !== undefined) {
    window.clearTimeout(pollTimer);
    pollTimer = undefined;
  }
  if (pokeTimer !== undefined) {
    window.clearTimeout(pokeTimer);
    pokeTimer = undefined;
  }
  if (progressTimer !== undefined) {
    window.clearInterval(progressTimer);
    progressTimer = undefined;
  }
  gen++;          // 让在飞的回调失效
  pollBusy = false;
}

function startPolling(root: HTMLElement) {
  stopPolling();
  const myGen = gen;
  pollTimer = window.setTimeout(() => void tick(root, myGen), 200);
  // 进度条本地插值：与轮询同生共死，但不打网络（见 `paintProgressSmooth`）。
  progressTimer = window.setInterval(() => paintProgressSmooth(root), PROGRESS_TICK_MS);
}

/** **有交互就补一轮**（2026-10-03，用户要求）：「长间隔轮询」不影响正在操作时的实时感。
 *
 *  与 `startPolling` 的区别：**不动 `gen`、不重建进度条定时器** —— 只是把「排队中的
 *  下一拍」提前到现在。两道限流，免得「补一轮」反而比原来更吵：
 *    · **400ms 去抖**：一次连点（点 3 下「下一首」）只多发一轮；
 *    · **两次补轮之间至少隔 `POKE_MIN_GAP_MS`**：连点好几秒也最多 1 次/2s
 *      （不然疯狂点某个键时会比旧的 1Hz 还密）。
 *  隐藏时直接不发（与 tick 的 `document.hidden` 口径一致）。 */
const POKE_MIN_GAP_MS = 2000;
let lastPokeAt = 0;

function pokePoll(root: HTMLElement) {
  const wait = Math.max(400, POKE_MIN_GAP_MS - (Date.now() - lastPokeAt));
  if (pokeTimer !== undefined) window.clearTimeout(pokeTimer);
  pokeTimer = window.setTimeout(() => {
    pokeTimer = undefined;
    if (!root.isConnected || document.hidden) return;
    lastPokeAt = Date.now();
    if (pollTimer !== undefined) {
      window.clearTimeout(pollTimer);
      pollTimer = undefined;
    }
    pollTimer = window.setTimeout(() => void tick(root, gen), 0);
  }, wait);
}

/** 装两个**全局**监听器（只装一次；`attach` 会被反复调用，重复挂会叠加）：
 *
 *   ① `visibilitychange` —— **面板隐藏即停表**。`tick` 里虽有 `document.hidden` 兜底
 *      （隐藏时不打网络），但表还在空转；这里直接停掉，重新可见时立刻补一轮。
 *   ② 面板内**任意**交互（点击 / 输入 / 改值）—— 提前下一拍（见 `pokePoll`）。
 *      用捕获阶段挂在 `document` 上：面板里动态重画出来的节点（歌单行 / 设备列表 /
 *      滤波器行）**不需要逐个绑**，否则必然漏掉下一批重画出来的。
 *
 *  两个监听器都以 `currentRoot` 是否挂着为准 —— 没有面板在跑时它们是空转的。 */
function ensureGlobalPollListeners() {
  if (globalPollListenersReady) return;
  globalPollListenersReady = true;
  document.addEventListener("visibilitychange", () => {
    const root = currentRoot;
    if (!root || !root.isConnected) return;
    if (document.hidden) stopPolling();
    else startPolling(root);
  });
  const onInteract = (ev: Event) => {
    const root = currentRoot;
    if (!root || !root.isConnected || document.hidden) return;
    const t = ev.target as Node | null;
    if (!t || !root.contains(t)) return;
    pokePoll(root);
  };
  document.addEventListener("click", onInteract, true);
  document.addEventListener("input", onInteract, true);
  document.addEventListener("change", onInteract, true);
}

// ── 窗口尺寸（只对插件悬浮窗下发）─────────────────────────────────
// 播放界面是定尺的（封面 100 + 歌词/队列 + 控制条 35），窗口高必须跟着内容走，
// 否则会出现一大片空白。默认界面的歌单列「更长就滚」⇒ 贴合内容但**封顶**在 MUSIC_H。
//
// **只认 `#plugin-titlebar`**：那是插件悬浮窗独有的节点（plugin.html）。
// 同一份 music.ts 也会内嵌在主窗口的 `#results-list` 里跑，那时调 `plugin_window_resize`
// 会去改**主窗口**的尺寸 —— 灾难，所以必须先判断自己在哪个窗口里。

function scheduleResize(root: HTMLElement) {
  if (!document.getElementById("plugin-titlebar")) return;
  if (resizeTimer !== undefined) window.clearTimeout(resizeTimer);
  resizeTimer = window.setTimeout(() => void applyResize(root), 80);
}

async function applyResize(root: HTMLElement) {
  resizeTimer = undefined;
  if (!document.getElementById("plugin-titlebar")) return;
  // **默认面板是定尺 1280×720**（用户 2026-09-28 定）：两栏各自内部滚动，
  // 窗口不需要跟着内容走 —— 于是这里连「实测贴合」都不做了（那套留给播放态）。
  // 只有播放态才量：那是定尺长条，外层的标题栏（关闭操作栏 hover 时才占高）要算进去。
  let wantW = MUSIC_W;
  let need = MUSIC_H;
  if (isBarMode()) {
    // **`root` 是 `.music-root`，而 `.plugin-result` 与 `#results-list` 是它的外层** ——
    // 所以只能 `closest()` 往上找，不能 `querySelector()` 往下找（早先写错成后者，
    // 结果 wrap 永远是 null，尺寸一次都没下发过）。
    const wrap = root.closest<HTMLElement>(".plugin-result");
    const list = wrap?.closest<HTMLElement>("#results-list");
    if (!wrap || !list) return;
    const cs = getComputedStyle(list);
    const padY = parseFloat(cs.paddingTop || "0") + parseFloat(cs.paddingBottom || "0");
    // chrome = 窗口内高 − 列表可视高 = 标题栏 + 外层 padding + 边框。这样量，
    // 外层那几条 CSS 以后改了尺寸这里自动跟上，不用跟着改常量
    // （播放态那组零垫料之后，它正好等于「关闭操作栏的高度」：0 或 40）。
    const chrome = window.innerHeight - list.clientHeight;
    need = Math.round(chrome + padY + wrap.getBoundingClientRect().height);
    // 播放态宽度 550（垫料被 CSS 清零，窗口就是长条本身，见 `MUSIC_W_BAR`）
    wantW = MUSIC_W_BAR;
  }
  const key = `${wantW}x${need}`;
  if (key === lastResize) return;
  try {
    await invoke("plugin_window_resize", { width: wantW, height: need });
    // **只有真的贴合了才记账**（宿主回读尺寸确认过，见 plugin_window.rs）：
    // 没贴上时不记，下一轮 tick 会重来一次。
    lastResize = key;
  } catch {
    // 尺寸调不动不影响功能，不往面板上报错。
    // 宿主只在**回读尺寸对不上**时才回错（最小化 / 被系统夹住），此时不记账，
    // 用户把它恢复出来后的下一轮（每秒一次 tick）会自己重贴 —— 见 plugin_window.rs。
  }
}

// ── 交互 ──────────────────────────────────────────────────────────

export async function attachMusicListeners(root: HTMLElement) {
  // **曲线窗走另一条挂载**（2026-10-03）：它没有、也不该有主面板那一整套（轮询 / 播放 /
  // 设备 / 歌单 / 授权…）。判据放在最前面 —— 下面每一行都是「主窗」的前提。
  if (isCurveWindow) return attachCurveWindow(root);
  const view = root.querySelector<HTMLElement>(".music-root");
  if (!view) return;
  currentRoot = view;

  // 「隐藏即停表」+「一有交互就补一轮」两个监听器（只装一次，见 `ensureGlobalPollListeners`）。
  ensureGlobalPollListeners();
  // 每次挂载都回到初始形态：这个模块的状态是跨挂载存活的（懒加载模块），
  // 「上次打开时在播放界面」不该决定这一次。
  mode = "default";
  fieldsOverride = null;
  // 连接卡片的开合是**一次性意图**，不跨挂载（面板关掉再开，它该回到默认的收起态）
  setupOpen = false;
  // 未保存的质量档不能跨挂载留着：面板一关，用户点的那个「待保存」意图就作废了
  bitrateOverride = null;
  queueOpen = false;
  lastResize = "";
  // A-B 盲测的「随机映射 / 揭晓」不跨挂载：面板一关，这一次盲测就结束了
  tuneAbBlind = false;
  tuneAbRevealed = false;
  tuneAbSlots = ["a", "b"];
  tuneAbActive = null;
  lastResizable = null;
  msgSeen = "";
  msgAt = 0;
  // 收藏夹与左栏四类的缓存每次挂载都重置：面板重开要重新看一遍库
  // （数据是远端事实，缓存跨挂载留着只会显示上一次的陈旧列表）
  liked = null;
  likedError = "";
  playlists = null;
  sideCache.clear();
  sideLoading.clear();
  sideError.clear();
  sideTab = "playlist";
  selected = null;
  mainTracks = null;
  mainLoading = false;
  mainError = "";
  devices = null;
  devicesOpen = false;
  librespot = null;
  // 「播放在哪」每次挂载都重新解析（设备是远端事实，上次那台的 id 早失效了）。
  // **`localPlayOptOut` 不在这里重置** —— 它是「本次会话里用户关过本机播放」，
  // 跨挂载要留着，否则重开面板就把用户刚关掉的进程又拉起来。
  playTarget = "";
  // 搜索同理：面板重开时不该带着上次的词
  searchQuery = "";
  searchResult = null;
  searchLoading = false;
  searchOpen = false;
  searchPage = false;
  searchTab = "tracks";
  // 上一次挂载的进度线节点已经随 DOM 一起没了（`seekBars` 每次 attach 重新收集）
  seekBars = [];
  // 本地音乐页：**每次挂载都回到 Spotify 那一侧**（那是一个「进去看本机库」的浏览
  // 视图，上次停在哪儿不该决定这一次），数据也随之清空重拉 —— 磁盘上的文件是外部
  // 事实，跨挂载留缓存只会显示陈旧的库。
  localPage = false;
  mediaRoots = null;
  mediaRootsLoading = false;
  mediaRootsError = "";
  mediaRootSel = "";
  // 编辑态与它那两个集合同理：面板关掉再开，「上次勾了两行还没保存」这件事不存在了
  // （那份暂存只在这一次界面里有效，跨挂载留着会变成一段用户看不见、又会被 × 落盘的删除）
  mediaEditMode = false;
  mediaSel.clear();
  mediaPendingDel.clear();
  addMenuOpen = false;
  mediaTracks = null;
  mediaTracksLoading = false;
  mediaTracksError = "";
  mediaList = null;
  mediaQuery = "";
  mediaScan = null;
  stopMediaPoll();
  if (mediaQueryTimer !== undefined) { window.clearTimeout(mediaQueryTimer); mediaQueryTimer = undefined; }
  // 本地播放：**本机音频流的生命周期挂在插件窗上**（关窗由宿主 `player.rs` 收），
  // 挂载这一刻能走到这里，说明上一份界面已经拆过了 —— **会话本身**不能带进来
  // （它的路径 / 位置都已经不是真的了），但**上次播的那首要带进来**
  // （2026-10-03 用户口径：「保留上次播放的记录」）：队列 / 下标 / 位置从 localStorage
  // 恢复 ⇒ 底部那条栏一进来就有内容可显示，按播放键能从上次的位置续上。
  localPlayer = null;
  localBusy = false;
  localSeekRatio = -1;
  localHistory = [];
  stopLocalPoll();
  const lastPlayed = readLocalLast();
  localQueue = lastPlayed?.queue ?? [];
  localQueueIndex = lastPlayed ? lastPlayed.index : -1;
  localResumeMs = lastPlayed?.position_ms ?? 0;

  const msg = (s: string) => setText(view.querySelector("#music-msg"), s);
  const q = <T extends HTMLElement>(sel: string) => view.querySelector<T>(sel);

  // 授权窗口回调：宿主换完令牌会 emit 一次
  if (unlistenAuth) { unlistenAuth(); unlistenAuth = null; }
  unlistenAuth = await listen<{ ok: boolean; message: string }>("spotify-auth", (ev) => {
    if (!view.isConnected) return;
    if (ev.payload.ok) {
      msg(t("music.auth_ok"));
      void refreshConfig(view).then(() => {
        startPolling(view);
        // 刚登入 ⇒ 设备列表还是空的，同样要就位一次（**force**：这次设备可能真的变了，
        // 不能被复用窗口挡掉）。
        void ensurePlayTarget(view, true);
      });
    } else {
      msg(ev.payload.message || t("music.auth_failed"));
    }
  });

  // ── 两态互切 ──
  // **只由用户点**：没有任何「跟着播放状态自动切」的路径（见 renderMode 的注释）。
  const openPlayer = () => {
    mode = "player";
    renderMode(view);
  };
  /** 播放长条上那个返回：**只退一层、不停止播放**。它长在长条自己身上，语义是
   *  「把窗口让给默认面板」；「停播 + 退出」是标题栏那个 ← 的事（见 stopAndBack）。 */
  const backToDefault = () => {
    mode = "default";
    renderMode(view);
  };
  /** 设置页（2026-10-01）：齿轮进整屏设置页。**它不再是「切 #music-setup 的显隐」**
   *  —— 那个动作已经归头像了（见下），两者职责必须分开：一个是偏好、一个是连接。
   *  调音**不在这一页里**（用户 2026-10-01 要求独立成页，见 #music-v-tuning）——
   *  所以这里也不再顺手 loadTuning。 */
  const openSettings = () => {
    mode = "settings";
    renderMode(view);
  };
  q<HTMLElement>("#music-setup-toggle")?.addEventListener("click", openSettings);
  // 「恢复」：清宿主的 429 硬闸（用户唯一出口，见 `resumeSpotify`）
  q<HTMLElement>("#music-resume")?.addEventListener("click", () => void resumeSpotify(view));
  q<HTMLElement>("#music-set-back")?.addEventListener("click", backToDefault);
  /** 设置页里那颗「调音」按钮：切到调音页（**同一个视图**，标题栏那段切换也仍能进它 ——
   *  两处入口指向同一个 `mode`，不新增第二个状态）。 */
  q<HTMLElement>("#music-set-tuning")?.addEventListener("click", () => {
    mode = "tuning";
    renderMode(view);
  });
  // 调音页那枚返回按钮 **2026-10-02 按用户要求取掉了** —— 离页靠标题栏那枚三段
  // 切换（点回「在线歌单 / 本地音乐」），不再有第二个入口（见 #music-v-tuning 注释）。

  // ── 调音页（P0 2026-10-02：可编辑的链）────────────────────────────────
  // 全部**委托在容器上**：预设按钮、滤波器行、类型下拉都是动态渲染的，逐个绑定必然
  // 漏掉下一次重画出来的那一批。语义只有一个：「按下的那一刻就生效、就落盘」。
  // 总开关：**切换按钮**（role="switch"）—— 点一下就把当前状态取反。
  q<HTMLElement>("#music-tuning-on")?.addEventListener("click", () => {
    if (!tuning) return;
    void setTuningEnabled(view, !tuning.enabled);
  });
  // 曲线那一块的控件（缩放 / 测量 / 通道槽）**与曲线窗共用同一份接线**（见 wireTuningCurve）
  // —— 两处用的是同一组 id，各写一份必然漂移。
  wireTuningCurve(view);
  // 频响曲线窗的入口（曲线本身已不在这页里，见 curveShellHtml）。
  q<HTMLElement>("#music-curve-open")?.addEventListener("click", () => void openCurveWindow(view));
  // 三条全局水平滑块（总增益 / 低音 / 高音）：左标签 + 滑块 + 右数值框，两者改同一个值。
  // 滑块拖动中节流提交、松手补一次；数字框按 `change` 提交（不是每敲一个字发一次 IPC）。
  const hslider = (
    rangeSel: string,
    numSel: string,
    read: () => number,
    write: (v: number) => void,
  ) => {
    const range = q<HTMLInputElement>(rangeSel);
    range?.addEventListener("input", (ev) => {
      const v = Number.parseFloat((ev.target as HTMLInputElement).value);
      if (!Number.isFinite(v)) return;
      write(v);
      const num = view.querySelector<HTMLInputElement>(numSel);
      if (num) num.value = fmtTuneNum(v, 1);
      void tuneCommitSoon(view);
    });
    range?.addEventListener("change", (ev) => {
      const v = Number.parseFloat((ev.target as HTMLInputElement).value);
      if (Number.isFinite(v)) write(v);
      // 松手时**再提交一次**（拖动中那几次是节流提交，最终值未必赶上）
      void commitTuningChain(view);
    });
    q<HTMLInputElement>(numSel)?.addEventListener("change", (ev) => {
      const el = ev.target as HTMLInputElement;
      const v = Number.parseFloat(el.value.trim());
      // 非法（空 / 非数字）⇒ 不改草稿、也不发 IPC，把框恢复成草稿里的值
      if (!Number.isFinite(v)) {
        el.value = fmtTuneNum(read(), 1);
        return;
      }
      if (v === read()) return;
      write(v);
      // 去抖：与「另一控件自己的提交」合并成一次（见 tuneCommitDebounced）
      tuneCommitDebounced(view);
    });
  };
  hslider("#music-preamp-range", "#music-tune-preamp", () => tunePreamp, (v) => (tunePreamp = v));
  hslider("#music-bass-range", "#music-tune-bass", () => tuneBass, (v) => (tuneBass = v));
  hslider("#music-treble-range", "#music-tune-treble", () => tuneTreble, (v) => (tuneTreble = v));
  // 我的预设（P5-1）：一行三个动作共用一次委托（用 / 重命名 / 删除），
  // 删除要先过命名行那道确认 —— 那是用户的链，不该一点就没。
  q<HTMLElement>("#music-tuning-user-presets")?.addEventListener("click", (ev) => {
    const row = (ev.target as HTMLElement).closest<HTMLElement>(".music-preset-user");
    const name = row?.dataset.preset;
    if (!name) return;
    const act = (ev.target as HTMLElement).closest<HTMLElement>("[data-act]")?.dataset.act ?? "use";
    if (act === "rename") {
      tunePresetEdit = { mode: "rename", from: name, name };
      renderTuning(view);
      setTimeout(() => view.querySelector<HTMLInputElement>("#music-preset-name")?.select(), 0);
    } else if (act === "delete") {
      tunePresetEdit = { mode: "delete", name };
      renderTuning(view);
    } else {
      void applyTuningPreset(view, name);
    }
  });
  q<HTMLElement>("#music-preset-save")?.addEventListener("click", () => {
    if (!tuning) return;
    // 默认名只在「当前链来自某个**用户预设**」时才有意义 —— 内置那 5 档已经不在面板上了
    // （2026-10-03 整批删掉），默认档 `flat` 更是「没来源」，拿它当名字只会让人莫名其妙。
    const suggested = tuning.user_presets?.includes(tuning.preset) ? tuning.preset : "";
    tunePresetEdit = { mode: "save", name: suggested, overwrite: false };
    renderTuning(view);
    setTimeout(() => view.querySelector<HTMLInputElement>("#music-preset-name")?.select(), 0);
  });
  q<HTMLElement>("#music-preset-cancel")?.addEventListener("click", () => {
    tunePresetEdit = null;
    renderTuning(view);
  });
  q<HTMLElement>("#music-preset-ok")?.addEventListener("click", () => void submitPresetEdit(view));
  q<HTMLInputElement>("#music-preset-name")?.addEventListener("keydown", (ev) => {
    if (ev.key === "Enter") {
      ev.preventDefault();
      void submitPresetEdit(view);
    } else if (ev.key === "Escape") {
      ev.preventDefault();
      tunePresetEdit = null;
      renderTuning(view);
    }
  });
  q<HTMLElement>("#music-preset-import")?.addEventListener("click", () => void importTuningPreset(view));
  q<HTMLElement>("#music-preset-export")?.addEventListener("click", () => void exportTuningChain(view));
  // ── 全局效果器（P5-4）：延迟 / 声道复制 / 卷积 ─────────────────────────
  // 延迟：拉条拖动期间节流提交（同其它拉条），松手补一次；数字框按 `change` 提交。
  // **一律夹在 0..=上限内**：超范围引擎会拒**整条链**（听感上是「调音突然全没了」）。
  const delayRange = () => q<HTMLInputElement>("#music-delay-range");
  delayRange()?.addEventListener("input", (ev) => {
    const v = Number.parseFloat((ev.target as HTMLInputElement).value);
    if (!Number.isFinite(v)) return;
    tuneDelay = Math.min(TUNE_DELAY_MAX_MS, Math.max(0, v));
    const num = view.querySelector<HTMLInputElement>("#music-delay-num");
    if (num) num.value = fmtTuneNum(tuneDelay, 1);
    void tuneCommitSoon(view);
  });
  delayRange()?.addEventListener("change", (ev) => {
    const v = Number.parseFloat((ev.target as HTMLInputElement).value);
    if (Number.isFinite(v)) tuneDelay = Math.min(TUNE_DELAY_MAX_MS, Math.max(0, v));
    // 松手时**再提交一次**（拖动中那几次是节流提交，最终值未必赶上，同滤波器拉条那条）
    void commitTuningChain(view);
  });
  q<HTMLElement>("#music-delay-num")?.addEventListener("change", (ev) => {
    const el = ev.target as HTMLInputElement;
    const v = Number.parseFloat(el.value.trim());
    // 非法（空 / 非数字 / 越界）⇒ **不改草稿、也不发 IPC**，把框恢复成草稿里的值
    if (!Number.isFinite(v) || v < 0 || v > TUNE_DELAY_MAX_MS) {
      el.value = fmtTuneNum(tuneDelay, 1);
      return;
    }
    if (v === tuneDelay) return;
    tuneDelay = v;
    tuneCommitDebounced(view);
  });
  // 声道复制：三个动作共用一次委托（摊开声道菜单 / 选中一个声道 / 删掉这一行）。
  // 菜单开合是**纯视图**（不提交）；选声道与删除都要提交。
  q<HTMLElement>("#music-copy-list")?.addEventListener("click", (ev) => {
    const hit = (ev.target as HTMLElement).closest<HTMLElement>("[data-act]");
    if (!hit) return;
    const row = Number(hit.dataset.i);
    const act = hit.dataset.act;
    if (act === "menu") {
      const side = hit.dataset.side === "to" ? "to" : "from";
      const same = tuneCopyMenu?.row === row && tuneCopyMenu.side === side;
      tuneCopyMenu = same ? null : { row, side };
      renderTuneCopy(view);
    } else if (act === "pick") {
      const c = tuneCopy[row];
      const ch = Number(hit.dataset.ch);
      if (!c || !Number.isInteger(ch)) return;
      if (hit.dataset.side === "to") c.to = ch;
      else c.from = ch;
      tuneCopyMenu = null;
      void commitTuningChain(view);
    } else if (act === "del") {
      tuneCopy.splice(row, 1);
      // 行号变了 ⇒ 菜单要么收起、要么跟着往上挪（同滤波器表那条）
      if (tuneCopyMenu?.row === row) tuneCopyMenu = null;
      else if (tuneCopyMenu && tuneCopyMenu.row > row) tuneCopyMenu.row -= 1;
      void commitTuningChain(view);
    }
  });
  q<HTMLElement>("#music-copy-add")?.addEventListener("click", () => {
    const n = Math.max(2, tuning?.channels || 2);
    // 默认「左 → 右」：把单声道素材铺成两声道是最常见的那一次复制
    tuneCopy.push({ from: 0, to: Math.min(1, n - 1) });
    void commitTuningChain(view);
  });
  q<HTMLElement>("#music-conv-pick")?.addEventListener("click", () => void pickTuningIr(view));
  q<HTMLElement>("#music-conv-clear")?.addEventListener("click", () => {
    tuneConv = null;
    void commitTuningChain(view);
  });
  // A-B 盲测：存快照 / 切槽 / 开盲测 / 揭晓（2026-10-06）
  q<HTMLElement>("#music-ab-save-a")?.addEventListener("click", () => void tuningAbSave(view, "a"));
  q<HTMLElement>("#music-ab-save-b")?.addEventListener("click", () => void tuningAbSave(view, "b"));
  q<HTMLElement>("#music-ab-play")?.addEventListener("click", (ev) => {
    const hit = (ev.target as HTMLElement).closest<HTMLElement>("[data-pos]");
    const pos = Number(hit?.dataset.pos);
    // 盲测按钮也在这一行里，但它没有 `data-pos` ⇒ 这里自然只接两个切换键
    if (pos === 1 || pos === 2) void tuningAbApply(view, pos);
  });
  q<HTMLElement>("#music-ab-blind")?.addEventListener("click", () => {
    tuneAbBlind = !tuneAbBlind;
    tuneAbRevealed = false;
    // 换序 = 随机的「位置 → 槽」映射；关掉盲测就回到 A 左 B 右
    tuneAbSlots = tuneAbBlind && Math.random() < 0.5 ? ["b", "a"] : ["a", "b"];
    renderTuningAb(view);
  });
  q<HTMLElement>("#music-ab-reveal")?.addEventListener("click", () => {
    tuneAbRevealed = true;
    renderTuningAb(view);
  });
  // 频段柱：**点击**（逐段开关 / 摊开类型下拉 / 选类型 / 删除）与**改数字**
  // （`change`，即失焦或回车 —— 不是每敲一个字就发一次 IPC）分两条委托。
  const tuneTable = () => q<HTMLElement>("#music-tuning-filters");
  tuneTable()?.addEventListener("click", (ev) => {
    const hit = (ev.target as HTMLElement).closest<HTMLElement>("[data-act]");
    const row = hit?.closest<HTMLElement>(".music-tune-row");
    const i = Number(row?.dataset.i);
    const f = tuneFilters[i];
    if (!hit || !row || !f) return;
    const act = hit.dataset.act;
    if (act === "on") {
      // 关掉一段 ≠ 删掉它（宿主会把 `on=false` 的段留在配置里，见 TuningFilter）
      f.on = !f.on;
      void commitTuningChain(view);
    } else if (act === "del") {
      tuneFilters.splice(i, 1);
      // 行号变了 ⇒ 收起那个下拉（不收起的话它会对到别的行上）
      if (tuneOpen === i) tuneOpen = -1;
      else if (tuneOpen > i) tuneOpen -= 1;
      void commitTuningChain(view);
    } else if (act === "kind") {
      tuneOpen = tuneOpen === i ? -1 : i;
      renderTuningFilters(view);
    } else if (act === "pick") {
      const name = hit.dataset.kind;
      if (!name) return;
      applyTuningKind(f, name, tuning?.kinds ?? []);
      tuneOpen = -1;
      void commitTuningChain(view);
    }
  });
  // 拉条（`type="range"`）**拖动中**就走这条：改草稿 + 立刻重画手柄 + 节流提交，
  // 这样「一边听一边扫」时曲线是跟着走的。文本框**不走这条**（它按 `change` 提交，
  // 见下面那条 —— 每敲一个字发一次 IPC 既浪费又会被中间态拒绝）。
  tuneTable()?.addEventListener("input", (ev) => {
    const el = ev.target as HTMLInputElement;
    if (!(el instanceof HTMLInputElement) || el.type !== "range") return;
    const field = el.dataset.f;
    const row = el.closest<HTMLElement>(".music-tune-row");
    const f = tuneFilters[Number(row?.dataset.i)];
    if (!field || !f) return;
    const v = Number.parseFloat(el.value);
    if (!Number.isFinite(v)) return;
    if (field === "gain_db") f.gain_db = v;
    else if (field === "q") f.q = v;
    else return;
    // 同一个值有两个控件（数字框 + 拉条）：把这一行那个数字框与拉条右侧的读数
    // 一起同步过去，否则两处会各显示各的（用户会以为拉条没生效）。
    const num = row?.querySelector<HTMLInputElement>(`input.music-tune-num[data-f="${field}"]`);
    if (num) num.value = tuneDraftField(f, field);
    const out = el.closest(".music-tune-slider")?.querySelector("em");
    if (out) out.textContent = tuneDraftField(f, field);
    // 拖动期间不许重建表格（换了节点就收不到后续 pointermove）—— 见 renderTuningFilters
    tuneHold = true;
    drawTuningCurve(view);
    tuneCommitSoon(view);
  });
  tuneTable()?.addEventListener("change", (ev) => {
    const el = ev.target as HTMLInputElement;
    const field = el?.dataset?.f;
    if (!field) return;
    const row = el.closest<HTMLElement>(".music-tune-row");
    const f = tuneFilters[Number(row?.dataset.i)];
    if (!f) return;
    // 拉条松手：先把「不许重建」的门放开，下面这次提交才会把表补画回来
    // （不放开的话表格会一直停在拖动前的那一版）。
    if (el.type === "range") tuneHold = false;
    const v = Number.parseFloat(el.value.trim());
    // 非法输入（空 / 不是数字）**不改草稿、也不发 IPC**，把框恢复成草稿里的值
    // 「值没变」也直接返回：`change` 在失焦时同样会触发，不该白发一次 IPC。
    if (!Number.isFinite(v)) {
      el.value = tuneDraftField(f, field);
      return;
    }
    let changed = false;
    if (field === "freq_hz") {
      changed = v !== f.freq_hz;
      f.freq_hz = v;
    } else if (field === "gain_db") {
      changed = v !== f.gain_db;
      f.gain_db = v;
    } else if (field === "q") {
      changed = v !== f.q;
      f.q = v;
    } else {
      return;
    }
    // 拉条松手时**再提交一次**，哪怕「值没变」：拖动中那几次是节流提交，最后一下
    // 未必赶上；而草稿在 `input` 里就已经写成了这个值 ⇒ 「没变就不发」那条判据
    // 对它天然不成立。少了这一下，松手后的**最终值可能没落盘**。
    if (!changed && el.type !== "range") return;
    // 拉条松手：立刻提交（拖动中的节流提交未必赶上最终值）；
    // 文本框：走**去抖** —— 一次动作可能连触发多个 `change`，合并成一次 IPC。
    if (el.type === "range") void commitTuningChain(view);
    else tuneCommitDebounced(view);
  });
  q<HTMLElement>("#music-tune-add")?.addEventListener("click", () => {
    tuneFilters.push(newTuningFilter());
    void commitTuningChain(view);
  });

  // ── 频响曲线的拖动编辑**已抽到 `wireTuningCurveDrag`**（2026-10-03）──────
  // 曲线窗与调音页共用同一份接线（两处同一组 id），所以这里不再重复一份 ——
  // 「一个窗里能拖、另一个窗里拖不动」正是复制两份会得到的结果。

  /** 头像 = 账号与连接（2026-10-01 用户要求「把连接窗口放到点击头像处」）。
   *  未连接时那张卡片本来就一直摊着（`renderSetup` 里 `show(..., !connected)`），
   *  所以这里只在**已连接**时才有意义：点一下把它摊开 / 收起。 */
  q<HTMLElement>("#music-avatar")?.addEventListener("click", () => {
    if (mode !== "default") {
      mode = "default";
      renderMode(view);
    }
    setupOpen = !setupOpen;
    // 显隐**交给 renderSetup**，别在这里直接改 class：它每轮轮询都会按「连没连」
    // 重设一次，绕过它改的话下一拍就被冲掉（见 setupOpen 的注释）。
    renderSetup(view);
  });

  // 「进入小窗播放」只剩**左边「正在播放」那一块**这一个入口（2026-10-03 用户口径）：
  // 右端那两枚独立按钮（Spotify 条 / 本地条各一枚）已删，绑定也一并收掉。
  q<HTMLElement>("#music-bnow")?.addEventListener("click", openPlayer);
  // 「返回歌单」那枚（曾在歌名模块右端）**已删除**（2026-10-01 用户口径）：
  // 播放态退出去只走标题栏那个 ←（syncTitlebarClose 把 × 换成它）。

  // ── 本地音乐页的进出（2026-10-01）──
  // 入口是**标题栏那枚三段切换的第二段（本地音乐）**（2026-10-01 改口径：原来
  // 那枚工具条文件夹按钮已删）。这个开关**不要求已连接 Spotify** —— 本地库与账号
  // 无关，所以它也是「未登录时怎么进到主体」的唯一路径。
  const setLocalPage = (on: boolean) => {
    if (localPage === on) return;
    localPage = on;
    // 换页 = 回到「这一页自己的那条控制栏」（抽屉翻过的状态不跟着跨页，见 barSwap）
    barSwap = false;
    if (on) {
      // **切到本地页 = 停掉 Spotify**（2026-10-03 用户口径：「并列播放未开的情况下
      // 切换标签页会停止当前播放」）。用 pause 而不是 stop —— Spotify Web API 根本
      // 没有 stop（同 stopAndBack 的注释）。**并列播放开着时不动它**：那正是这把
      // 开关的字面意思（两条流同时出声）。反方向那一刀在 else 分支里（停本地）。
      if (!dualPlay && player?.playing) {
        invoke("spotify_control", { action: "pause" }).catch(() => {});
      }
      // 进本地页就把 Spotify 那两条选路清掉：回去时不会停在一个陈旧的选中项上，
      // 也不会让主区在两个数据源之间打架（renderMain 里本地页优先级最高）。
      searchPage = false;
      searchOpen = false;
      selected = null;
      mainTracks = null;
      mainError = "";
      mediaRoots = null;
      mediaTracks = null;
      mediaRootsError = "";
      mediaTracksError = "";
      mediaScan = null;
      // 上次打开的那份播放列表不带进来（它是「临时看了一眼」的东西，不是状态）
      mediaList = null;
    } else {
      stopMediaPoll();
      mediaQuery = "";
      if (mediaQueryTimer !== undefined) { window.clearTimeout(mediaQueryTimer); mediaQueryTimer = undefined; }
      // 左栏回到歌单那一栏（非本地分支会把那排页签放回来）
      sideTab = "playlist";
      // **离开本地页 = 停掉本地播放**：本地播放条长在这一页里，退出去就没有任何
      // 地方能控制它了（音量 / 暂停 / 切歌全在那条栏上）。界面没了声音还在放，
      // 是用户最难解释的状态 —— 与「关窗就带走 librespot」是同一条纪律，
      // 只是粒度更细。见 renderLocalNowbar 的注释。
      //
      // ⚠️ **「同时出声」开着时不停**（2026-10-01 用户第 7 条）：那正是用户要的共存
      // ——一离开这一页就把它掐掉，这把开关等于白给。**但「条在不在」得跟着一起放宽**，
      // 否则就会留下一条没有界面的音频流（`renderLocalNowbar` 的判据里加了 `dualPlay`，
      // 于是这条栏会跟着到 Spotify 页上）。两处必须同时成立，改一处就会破功。
      if (localPlayer?.path && !dualPlay) void stopLocalPlayback(view);
    }
    renderMode(view);
    renderStatus(view);
    // 本地那条控制栏的显隐 / 内容由这里画（**栏常驻之后它不再有轮询兜底**：
    // 没有会话时 localPoll 不跑，没人会周期性地重画它，见 renderLocalNowbar）。
    renderLocalNowbar(view);
    if (!on) return;
    void loadMediaRoots(view);
    void loadMediaTracks(view);
    // 面板打开前可能就已经在扫了（扫描是宿主的后台线程，跨越面板开关）—— 补一次快照，
    // 在跑就把进度轮询接回来，别让用户看到一条永远停在 0% 的假状态。
    void invoke<ScanStatusDto>("media_scan_status")
      .then(st => {
        if (!localPage || !view.isConnected) return;
        mediaScan = st;
        if (st.running) pollMediaScan(view);
        refreshLocal(view);
      })
      .catch(() => {});
  };

  // ── 标题栏那枚三段切换（2026-10-01 用户口径）──
  // 工具条上的文件夹 / 推子两枚已删，进「本地音乐」与「调音」**只走这一枚**
  // （三段：在线歌单 / 本地音乐 / 调音，见 placeSwitcher）。
  const switchSegment = (seg: string) => {
    if (seg === "tuning") {
      mode = "tuning";
      renderMode(view);   // 里面会 loadTuning 一次
      return;
    }
    mode = "default";
    setLocalPage(seg === "local");
    // ⚠️ `setLocalPage` 在「值没变」时会**提前 return**（点击当前那一段就是这种），
    // 而那时我们可能正从调音页 / 播放态回来 —— 所以切视图这一刀必须自己补，
    // 不能指望它替我们做（否则表现是「点了没反应」）。
    renderMode(view);
  };
  const switcher = placeSwitcher(view);
  switcher?.addEventListener("click", (ev) => {
    const btn = (ev.target as HTMLElement).closest<HTMLElement>(".music-tb-seg");
    const seg = btn?.dataset.seg;
    if (seg) switchSegment(seg);
  });
  // `＋` = 开/关那个二选一浮层（目录 / 播放列表文件）。真正干活的是浮层里那两行，
  // 由 `onListClick` 的 `media-add-dir` / `media-add-list` 分派 —— 浮层内容每次重画，
  // 直接绑会绑在已经被换掉的节点上。
  q<HTMLElement>("#music-local-add")?.addEventListener("click", () => {
    addMenuOpen = !addMenuOpen;
    renderLocalAddMenu(view);
  });
  // ✎ = 进编辑态；🗑 = 把勾选的行摘出列表（暂存）；× = 退出并保存（落盘）
  q<HTMLElement>("#music-local-list")?.addEventListener("click", () => enterMediaEdit(view));
  q<HTMLElement>("#music-local-del")?.addEventListener("click", () => stageMediaDelete(view));
  q<HTMLElement>("#music-local-editdone")?.addEventListener("click", () => void saveMediaEdit(view));
  q<HTMLElement>("#music-local-scan")?.addEventListener("click", () => void startMediaScan(view));
  // 筛选框的输入是**委托**的：它长在 renderLocalMain 动态建出来的骨架里，attach 这
  // 一刻还不存在，不能直接 querySelector 绑。
  q<HTMLElement>("#music-main")?.addEventListener("input", (ev) => {
    const el = ev.target as HTMLInputElement | null;
    if (el?.id !== "music-mt-q") return;
    scheduleMediaQuery(view, el.value.trim());
  });

  // ── 本地播放条（2026-10-01）──
  // 这条栏是**静态骨架**（长在 shell 里，不随 localPage 重建），所以直接绑。
  q<HTMLElement>("#music-l-prev")?.addEventListener("click", () => void advanceLocal(view, -1, false));
  q<HTMLElement>("#music-l-next")?.addEventListener("click", () => void advanceLocal(view, 1, false));
  q<HTMLElement>("#music-l-play")?.addEventListener("click", () => void toggleLocalPause(view));
  q<HTMLElement>("#music-l-stop")?.addEventListener("click", () => void stopLocalPlayback(view));
  // 左端那枚「换到另一侧」的抽屉（2026-10-03）：点它把**另一条**控制栏换出来。
  // 两条栏的显隐只由 `barSwap` 决定（见 localBarVisible），所以这里只翻标志 + 重画两处。
  q<HTMLElement>("#music-b-drawer")?.addEventListener("click", () => {
    barSwap = true;
    syncHead(view);
    renderLocalNowbar(view);
  });
  q<HTMLElement>("#music-l-drawer")?.addEventListener("click", () => {
    barSwap = false;
    syncHead(view);
    renderLocalNowbar(view);
  });
  const lvol = q<HTMLInputElement>("#music-l-vol");
  lvol?.addEventListener("input", () => {
    // 与 Spotify 那两条同理：拖动期间 renderLocalNowbar 会跳过同步，
    // 填充只在这里补（否则滑块在动、条子不动）。
    paintVolume(lvol, Number(lvol.value));
    void setLocalVolume(view, Number(lvol.value) / 100);
  });
  // 播放模式（2026-10-01 用户第 8 条）：关 → 随机 → 循环列表 → 单曲循环 → 关。
  // **四态而不是照抄 Spotify 那枚的三态**：用户点名要「循环列表」与「单曲循环」两档分开。
  // 按完给一句 `note` —— 这一下的反馈只有一个 8px 的小图标换了个样子。
  // **收成一个函数**：本地栏那枚与小窗里那枚是同一个 `localMode`、同一套循环顺序，
  // 两处各写一遍必然只会改到一处（另一处就会「按了不动」）。
  const cycleLocalMode = () => {
    const order: LocalMode[] = ["off", "shuffle", "repeat_all", "repeat_one"];
    localMode = order[(order.indexOf(localMode) + 1) % order.length];
    writeLocalMode(localMode);
    renderLocalNowbar(view);
    note(view, localModeTitle());
  };
  q<HTMLElement>("#music-l-mode")?.addEventListener("click", cycleLocalMode);

  // 进度线：点击 = 跳转，拖动 = 跟手（松手才发命令）。与 Spotify 那条同形，
  // 但**不与它共用绑定** —— 那条的时长来自远端状态，这条来自宿主解码器报的时长。
  // **本地有两条这样的线**（默认态那条 `#music-lbp` 与小窗那条 `#music-lp-progress`）：
  // 几何、命令、`localSeekRatio` 的加解锁全都一样，只有「时间数字画在哪儿」不同，
  // 所以共用这一份，差异用 `onPaint` 回给调用方 —— 两条各写一遍，必然会漏掉
  // 某一处的 `localSeekRatio = -1`，表现是「拖过一次之后进度条再也不跟手了」。
  const bindLocalSeek = (el: HTMLElement, onPaint: (ratio: number) => void) => {
    const ratioAt = (clientX: number): number => {
      const r = el.getBoundingClientRect();
      return Math.max(0, Math.min(1, (clientX - r.left) / Math.max(1, r.width)));
    };
    const paint = (ratio: number) => {
      localSeekRatio = ratio;
      el.style.setProperty("--seek-pct", `${ratio * 100}%`);
      onPaint(ratio);
    };
    el.addEventListener("pointerdown", (ev) => {
      // 时长报不出来（0）就不给拖 —— 拖了也算不出该跳到哪儿
      if ((localPlayer?.duration_ms ?? 0) <= 0) return;
      // `try` 是给**合成事件**留的：没有活动指针时 setPointerCapture 会抛 NotFoundError
      try { el.setPointerCapture(ev.pointerId); } catch { /* 没有活动指针 */ }
      el.classList.add("music-dragging");
      paint(ratioAt(ev.clientX));
    });
    el.addEventListener("pointermove", (ev) => {
      if (localSeekRatio < 0) return;
      paint(ratioAt(ev.clientX));
    });
    const end = (ev: PointerEvent) => {
      if (localSeekRatio < 0) return;
      const ratio = localSeekRatio;
      localSeekRatio = -1;   // 先解锁，下一拍 `renderLocalNowbar` 才能重新接管
      el.classList.remove("music-dragging");
      try { el.releasePointerCapture(ev.pointerId); } catch { /* 指针已经不在了 */ }
      void seekLocal(view, Math.round((localPlayer?.duration_ms ?? 0) * ratio));
    };
    el.addEventListener("pointerup", end);
    el.addEventListener("pointercancel", end);
  };
  const lbp = q<HTMLElement>("#music-lbp");
  if (lbp) {
    bindLocalSeek(lbp, (ratio) => {
      const dur = localPlayer?.duration_ms ?? 0;
      setText(lbp.querySelector<HTMLElement>(".music-progress-tip"), fmtMs(dur * ratio));
      const tEl = view.querySelector<HTMLElement>("#music-l-time");
      if (tEl) setText(tEl, `${fmtClock(dur * ratio)} / ${fmtClock(dur)}`);
    });
  }

  // ── 本地小窗（2026-10-01 用户口径：本地播放也要有小窗模式）──
  // 与上面那条栏是**同一组动作**（上一首 / 播放 / 下一首 / 停止 / 模式 / 音量 / 拖动跳转），
  // 差别只有「画在哪个视图上」。所以这里只多几条**指向同一个函数**的绑定，
  // 没有第二份业务逻辑 —— 这是它与 Spotify 那条长条最不一样的地方：
  // 那条是远端状态，这条从头到尾读的都是 `localPlayer`。
  const openLocalMini = async () => {
    // 有会话 ⇒ 直接进小窗。**只有「上次那首」时先续播再进** —— 栏常驻之后
    // 「没有会话」是常态，这一下若直接返回就是「点了没反应」（用户最反感的那种按钮）。
    if (!localPlayer || !localPlayer.path) {
      const path = localQueue[localQueueIndex]?.path ?? "";
      if (!path) return;
      const ok = await startLocalPlay(view, path, localResumeMs);
      if (!ok) return;   // 起不来（没有音频设备等）⇒ 别把小窗打开成一个空壳
    }
    mode = "lplayer";
    renderMode(view);
  };
  // 「进入小窗播放」的入口**在左边那一块上**（2026-10-03 用户口径，与 Spotify 那条
  // 栏同形）：右端那枚独立按钮已删，所以这一条绑定从它挪到了 `#music-lnow`。
  q<HTMLElement>("#music-lnow")?.addEventListener("click", () => void openLocalMini());
  q<HTMLElement>("#music-lp-prev")?.addEventListener("click", () => void advanceLocal(view, -1, false));
  q<HTMLElement>("#music-lp-next")?.addEventListener("click", () => void advanceLocal(view, 1, false));
  q<HTMLElement>("#music-lp-play")?.addEventListener("click", () => void toggleLocalPause(view));
  q<HTMLElement>("#music-lp-stop")?.addEventListener("click", () => void stopLocalPlayback(view));
  q<HTMLElement>("#music-lp-mode")?.addEventListener("click", cycleLocalMode);
  const lpVol = q<HTMLInputElement>("#music-lp-vol");
  lpVol?.addEventListener("input", () => {
    paintVolume(lpVol, Number(lpVol.value));
    void setLocalVolume(view, Number(lpVol.value) / 100);
  });
  const lps = q<HTMLElement>("#music-lp-progress");
  if (lps) {
    bindLocalSeek(lps, (ratio) => {
      const dur = localPlayer?.duration_ms ?? 0;
      setText(lps.querySelector<HTMLElement>(".music-progress-tip"), fmtMs(dur * ratio));
      const tEl = view.querySelector<HTMLElement>("#music-lp-time");
      if (tEl) setText(tEl, `${fmtClock(dur * ratio)} / ${fmtClock(dur)}`);
    });
  }

  // ── 播放态：标题栏那个按钮的语义改写（见 syncTitlebarClose / stopAndBack）──
  // **必须在捕获阶段拦**：`#plugin-close` 的关窗监听器在 `plugin-window.ts` 里
  // （所有插件共用的入口，不能为一个插件改它）。捕获阶段挂在 `document` 上先于它执行，
  // `stopPropagation()` 就能让事件根本走不到那个按钮 ⇒ 共用入口一行都不用动。
  const onTitlebarClick = (ev: Event) => {
    if (!view.isConnected || !isBarMode()) return;
    if (!(ev.target as HTMLElement | null)?.closest("#plugin-close")) return;
    ev.stopPropagation();
    ev.preventDefault();
    // 两种小窗的「←」语义**刻意不同**：
    // · 远端那条 = 「这一曲我放下了」（pause + 回面板）—— 它没有别的出口；
    // · 本地那条 = **只回面板、不停播** —— 本地那条流在默认面板上本来就有自己的
    //   停止键（`#music-l-stop`，小窗里是 `#music-lp-stop`），顺手停掉会让
    //   用户一退出去就得重新选曲、重新排队列。
    if (mode === "lplayer") backToDefault();
    else void stopAndBack(view);
  };
  document.addEventListener("click", onTitlebarClick, true);

  // 展开 / 收起「自己填 Client ID」的那组字段（用内置值时才默认收起）
  q<HTMLElement>("#music-fields-toggle")?.addEventListener("click", () => {
    fieldsOverride = !(fieldsOverride ?? !cfg?.builtin);
    renderSetup(view);
  });

  q<HTMLElement>("#music-copy-redirect")?.addEventListener("click", () => {
    const v = cfg?.redirect_uri || "";
    if (v) writeText(v).then(() => msg(t("music.copied"))).catch(() => {});
  });

  // 串流质量三档（**现在长在设置页里**，2026-10-01 从连接卡片搬出）：只改内存里的
  // 「待保存值」，不立刻落盘 —— 由设置页自己那个「保存」一次性提交，与连接卡片
  // 完全分开（一个卡片两套保存语义只会让人困惑，见 renderMode 那条注释）。
  for (const btn of view.querySelectorAll<HTMLElement>("#music-quality .music-seg-btn")) {
    btn.addEventListener("click", () => {
      bitrateOverride = Number(btn.dataset.q);
      renderSetup(view);
    });
  }

  /** 设置页的「保存」：**只提交音质这一项**（`music_config_set` 对其余字段是
   *  `Option` —— 不传就等于「别动」，所以这里不必把连接卡片里那些值再抄一遍）。 */
  q<HTMLElement>("#music-set-save")?.addEventListener("click", async () => {
    try {
      cfg = await invoke<MusicConfigDto>("music_config_set", { librespotBitrate: bitrateOverride });
      bitrateOverride = null;
      msg(t("music.saved"));
      renderSetup(view);
      renderStatus(view);
    } catch (e) {
      msg(errText(e));
    }
  });

  q<HTMLElement>("#music-save")?.addEventListener("click", async () => {
    const id = q<HTMLInputElement>("#music-client-id")?.value.trim() ?? "";
    const portRaw = q<HTMLInputElement>("#music-port")?.value.trim() ?? "";
    const port = Number(portRaw);
    if (!id) { msg(t("music.err_no_client_id")); return; }
    if (!Number.isInteger(port) || port < 1024 || port > 65535) { msg(t("music.err_bad_port")); return; }
    try {
      cfg = await invoke<MusicConfigDto>("music_config_set", {
        clientId: id,
        port,
        // 本机播放那两项**一起交上去**（留空 = 自动探测 / 直连，都是合法值）
        librespotPath: q<HTMLInputElement>("#music-librespot")?.value.trim() ?? "",
        librespotProxy: q<HTMLInputElement>("#music-librespot-proxy")?.value.trim() ?? "",
        // 音质**不在这里提交**（2026-10-01 起它归设置页，那个页面有自己的「保存」）：
        // 不传 = `None` = 别动，宿主本来就是这个语义。把两页的保存混在一处，
        // 就会出现「在设置页点了 320、回连接卡片点保存、结果把它一起落盘了」。
      });
      // **不碰 `bitrateOverride`**：它是设置页的待保存值，这张卡片的保存不该把它
      // 悄悄丢掉（改之前这里有一句 `bitrateOverride = null`，那是音质还住在这张
      // 卡片里时留下的）。
      msg(t("music.saved"));
      renderSetup(view);
      renderStatus(view);
      // 路径可能刚填上 ⇒ 让设备弹层里的「本机播放」重新判断一次可用性
      librespot = null;
    } catch (e) {
      msg(errText(e));
    }
  });

  q<HTMLElement>("#music-connect")?.addEventListener("click", async () => {
    // 先落盘再连：Client ID 只在输入框里没保存时，宿主拿不到它（宿主读 config\music.json）
    const id = q<HTMLInputElement>("#music-client-id")?.value.trim() ?? "";
    const port = Number(q<HTMLInputElement>("#music-port")?.value.trim() ?? "");
    if (id && Number.isInteger(port) && port >= 1024) {
      try {
        // **本机播放那两项传 null**（= 别动）：这条路径只是「先落盘再连」，
        // 把没编辑过的字段也传上去会覆盖用户已经保存的值。
        cfg = await invoke<MusicConfigDto>("music_config_set", {
          clientId: id,
          port,
          librespotPath: null,
          librespotProxy: null,
          // 串流质量同上：这条路径只是「先落盘再连」，别动用户选的档
          librespotBitrate: null,
        });
      } catch (e) {
        msg(errText(e));
        return;
      }
    }
    try {
      const url = await invoke<string>("spotify_connect");
      msg(t("music.browser_hint"));
      await open(url);
    } catch (e) {
      msg(errText(e));
    }
  });

  q<HTMLElement>("#music-disconnect")?.addEventListener("click", async () => {
    try {
      await invoke("spotify_disconnect");
      player = null;
      lyrics = null;
      lyricsFor = "";
      playlists = null;
      sideCache.clear();
      sideError.clear();
      queue = null;
      queueOpen = false;
      selected = null;
      mainTracks = null;
      mainError = "";
      searchResult = null;
      searchQuery = "";
      searchOpen = false;
      searchPage = false;
      devices = null;
      mode = "default";
      await refreshConfig(view);
      renderMode(view);
      renderQueue(view);
      msg(t("music.disconnected"));
    } catch (e) {
      msg(errText(e));
    }
  });

  // ── 点击委托（一份就够了）──
  // 左栏 / 主区 / 搜索预览 / 搜索详细页 / 设备弹层**整块都会被重建**，
  // 逐个给行挂监听会变成孤儿监听；而且「按 data-act 分派」的逻辑只该有一份
  // （多一份必然漏掉新加的动作）。**四个容器都要挂** —— 弹层里的行不在列表里。
  const onListClick = (ev: Event) => {
    const el = (ev.target as HTMLElement | null)?.closest<HTMLElement>("[data-act]");
    if (!el) return;
    const kind = (el.dataset.kind || "") as MainKind;
    const id = el.dataset.id || "";
    switch (el.dataset.act) {
      case "side-tab":
        sideTab = (el.dataset.tab || "playlist") as SideKind;
        renderSide(view);
        void loadSide(view, sideTab); // 有缓存就立刻返回（loadSide 自己判）
        break;
      case "search-tab":
        searchTab = (el.dataset.tab || "tracks") as typeof searchTab;
        renderMain(view);
        break;
      case "open":
        void openItem(view, kind, id);
        break;
      case "main-track":
        void playMainTrack(view, Number(el.dataset.i || "0"));
        break;
      case "main-play-all":
        if (selected) void playWholeItem(view, selected.kind, selected.id);
        break;
      case "dd-track":
        if (el.dataset.uri) void playUri(view, el.dataset.uri);
        break;
      case "play-uri":
        // 播放队列那一列（`#music-q` 里的行）。**这条以前漏了** ——
        // `renderQueue` 一直在发 `play-uri`，而分派里没有对应分支，
        // 于是点队列项毫无反应（「点一首 = 从它开始播」那个提示也成了空话）。
        if (el.dataset.uri) void playUri(view, el.dataset.uri);
        break;
      case "search-all":
        searchOpen = false;
        searchPage = true;
        renderSearchDD(view);
        renderMain(view);
        break;
      case "dev-pick":
        void pickDevice(view, id);
        break;
      case "local-toggle":
        void toggleLocalPlay(view);
        break;
      case "local-login":
        void startLocalLogin(view);
        break;
      // ── 本地音乐页（2026-10-01）──
      case "media-root": {
        const p = el.dataset.path || "";
        // **编辑态里这一下的语义完全不同**：不是「换来源」，而是「勾选它」。
        // 两个语义共用同一次点击是有意的 —— 但绝不能同时生效：编辑态下顺手换了
        // 主区来源，用户会以为那一勾没生效（主区确实变了，可列表上看不出来）。
        if (mediaEditMode) {
          if (p) (mediaSel.has(p) ? mediaSel.delete(p) : mediaSel.add(p));
          renderLocalSide(view);
          break;
        }
        // 换一根目录：只重拉曲目（左栏那一列的选中态用 renderLocalSide 重画，
        // 它的内容没变，重画是一次幂等的 setHtml）
        mediaRootSel = p;
        renderLocalSide(view);
        void loadMediaTracks(view);
        break;
      }
      case "media-scan":
        void startMediaScan(view);
        break;
      // `＋` 浮层里的两行（2026-10-01）：目录 vs 播放列表文件。
      // **先关浮层再干活** —— 目录对话框是模态的，浮层留着会在对话框背后一闪一闪。
      case "media-add-dir":
        addMenuOpen = false;
        renderLocalAddMenu(view);
        void addMediaRoot(view);
        break;
      case "media-add-list":
        addMenuOpen = false;
        renderLocalAddMenu(view);
        void openMediaPlaylist(view);
        break;
      case "media-list-close":
        closeMediaPlaylist(view);
        break;
      case "local-track":
        // 点一行本地曲目 = 从这一首开始放（整批可见曲目就是队列，见 playLocalTrack）
        void playLocalTrack(view, Number(el.dataset.i || "0"));
        break;
    }
  };
  for (const sel of ["#music-side-list", "#music-side-tabs", "#music-main", "#music-search-dd", "#music-dev-dd", "#music-q", "#music-q-dd", "#music-local-add-dd"]) {
    q<HTMLElement>(sel)?.addEventListener("click", onListClick);
  }

  // ── 左栏双击 = 播放这一项（用户 2026-09-28：行尾那枚播放按钮删掉，功能挪到双击）──
  // **只挂左栏**：单次点击不会再重建左栏（见 `syncSideSelection`），两次 click 才落在
  // 同一个 target 上 ⇒ 浏览器才会发 `dblclick`。主区那张卡片点一下就被整个换掉，
  // 双击根本无从触发（所以那里保留详细页头的「播放全部」）。
  q<HTMLElement>("#music-side-list")?.addEventListener("dblclick", (ev) => {
    const el = (ev.target as HTMLElement | null)?.closest<HTMLElement>('[data-act="open"]');
    if (!el) return;
    const kind = (el.dataset.kind || "") as MainKind;
    const id = el.dataset.id || "";
    if (id) void playWholeItem(view, kind, id);
  });

  // ── 设备按钮（在底部控制栏右端）──
  q<HTMLElement>("#music-devices-btn")?.addEventListener("click", () => {
    devicesOpen = !devicesOpen;
    renderDevices(view);
    if (devicesOpen) void loadDevices(view);
  });

  // ── 左栏宽度可拖（用户 2026-09-28：「歌单列作为单独的一列可拉动」）──
  // 用 `pointerdown` + window 上的 move/up：拖动可能拖出面板，挂在分隔条自己身上
  // 会在指针离开它时丢事件（而 `setPointerCapture` 在合成事件下会抛 NotFoundError，
  // 那个坑进度条已经踩过一次）。
  applySideWidth(view);
  q<HTMLElement>("#music-split")?.addEventListener("pointerdown", (ev) => {
    ev.preventDefault();
    const startX = ev.clientX;
    const startW = q<HTMLElement>("#music-side")?.getBoundingClientRect().width ?? sideWidth;
    const move = (e: PointerEvent) => {
      sideWidth = Math.min(SIDE_W_MAX, Math.max(SIDE_W_MIN, Math.round(startW + e.clientX - startX)));
      applySideWidth(view);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      document.body.classList.remove("music-resizing");
    };
    document.body.classList.add("music-resizing");
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  });

  // 点空白处收起浮层（下拉预览 / 设备弹层 / 队列浮层）。**用 `pointerdown`**：它先于 click，
  // 于是「点空白收起」与「点行执行动作」不会打架。
  document.addEventListener("pointerdown", (ev) => {
    const t = ev.target as HTMLElement | null;
    if (!t) return;
    if (devicesOpen && !t.closest("#music-dev-dd") && !t.closest("#music-devices-btn")) {
      devicesOpen = false;
      renderDevices(view);
    }
    if (searchOpen && !t.closest(".music-search-wrap")) {
      searchOpen = false;
      renderSearchDD(view);
    }
    // `＋` 那个二选一浮层。**必须排除按钮自身**：否则「开着的时候再点一下 ＋」
    // 会先被这里关掉、再被 click 打开，看起来就是点了没反应。
    if (addMenuOpen && !t.closest("#music-local-add-dd") && !t.closest("#music-local-add")) {
      addMenuOpen = false;
      renderLocalAddMenu(view);
    }
    // 队列浮层只在默认面板里是浮层（播放长条里那块是常驻面板，由 `#music-queue-btn` 管）
    if (queueOpen && !t.closest("#music-q-dd") && !t.closest("#music-b-queue") && !t.closest("#music-queue-btn")) {
      queueOpen = false;
      syncPlayerBody(view);
    }
  });

  // ── 搜索框 ──
  const searchEl = q<HTMLInputElement>("#music-search");
  searchEl?.addEventListener("input", () => scheduleSearch(view, searchEl.value.trim()));
  // 聚焦时若已经有词（刚点过别处收起了预览）⇒ 把预览放回来
  searchEl?.addEventListener("focus", () => {
    if (searchQuery) { searchOpen = true; renderSearchDD(view); }
  });
  searchEl?.addEventListener("keydown", (ev) => {
    // 回车 = 进**搜索详细页**（等价于预览底部那个「查看全部」）
    if (ev.key === "Enter" && searchQuery) {
      searchOpen = false;
      searchPage = true;
      renderSearchDD(view);
      renderMain(view);
      return;
    }
    // Esc 清空。`type=search` 原生也认 Esc，但各引擎的清空时机不一致 —— 自己来一次更可控。
    if (ev.key === "Escape") {
      searchEl.value = "";
      scheduleSearch(view, "");
    }
  });

  // ── 关闭操作栏是 hover 显隐的（纯 CSS，见 #plugin-titlebar）──
  // 所以鼠标进出窗口时必须**立刻**重算窗口尺寸，不能等下一秒那轮 tick ——
  // 否则那 40px 会先被裁掉一下才长回来。`scheduleResize` 自带「只在插件悬浮窗里做」的判断。
  document.body.addEventListener("mouseenter", () => scheduleResize(view));
  document.body.addEventListener("mouseleave", () => scheduleResize(view));

  // ── 播放控制（**播放长条与底部控制栏共用同一批处理函数**）──
  // 两条栏上的按钮是同一件事的两个入口，所以只写一份逻辑、按 id 各挂一次 ——
  // 写两遍必然有一遍漏掉 `scheduleResize` 这类连带动作（同预检 #39 ⑩）。
  const control = async (action: string, value?: number) => {
    // 播放键那一下也要过「互斥」闸门（dualPlay 开着时它什么都不做）。
    // **只认 `play`**：`pause` / `next` 之类不该因为本地在放就把本地掐掉 ——
    // 停一路不是「起一路」的对偶。
    if (action === "play") await enforceExclusive(view, "spotify");
    try {
      await invoke("spotify_control", { action, value });
    } catch (e) {
      msg(errText(e));
    }
  };
  /** 给「同一件事的两个入口」各挂一次同一个处理函数（播放长条里的 + 底部控制栏里的）。
   *
   *  ⚠️ 传进来的是**裸 id**（不带 `#`）—— 前缀加在这一行里。少了它，
   *  `querySelector("music-b-queue")` 会被当成**标签选择器**、永远返回 null，
   *  而 `?.` 会把 null 静静地吞掉 ⇒ 整条底部控制栏点了毫无反应（2026-09-28 实测踩到：
   *  同一次挂载里 `q("#music-side-list")` 正常，只有这批按钮全哑）。 */
  const on = (ids: string[], fn: () => void) => {
    for (const id of ids) q<HTMLElement>(`#${id}`)?.addEventListener("click", fn);
  };

  const toggleQueue = () => {
    queueOpen = !queueOpen;
    syncPlayerBody(view);
    renderQueue(view);
    if (queueOpen) void loadQueue(view);
    else renderPlayer(view); // 只需刷新按钮按下态
    scheduleResize(view);
  };
  on(["music-queue-btn", "music-b-queue"], toggleQueue);

  on(["music-prev", "music-b-prev"], () => void control("previous"));
  on(["music-next", "music-b-next"], () => void control("next"));
  on(["music-play", "music-b-play"], () => void control(player?.playing ? "pause" : "play"));

  // 「本地与 Spotify 同时出声」那把开关（2026-10-01，用户第 7 条）。
  // **两枚按钮（Spotify 底部栏 / 本地播放条）共用这一个处理函数** ——
  // 它们改的是同一个状态，各写一份必然有一份漏掉 `syncHead`（同预检 #39 ⑩）。
  // 反馈一句 `note`：这把开关的效果是「以后不再互相掐」，按下去的那一刻界面上
  // 什么都看不出来，不给一句话就不知道点没点上。
  const toggleDual = () => {
    dualPlay = !dualPlay;
    writeDualPlay(dualPlay);
    // 抽屉只属于**共存态**：关掉开关就翻回「当前页自己的那条控制栏」（见 barSwap）
    if (!dualPlay) barSwap = false;
    // 关掉这把开关的那一刻要补一刀：若本机那条流还占着、而界面又不在本地页，
    // 它下一拍就会变成「没有界面的声音」（本地条按新判据收掉了）—— 当场停掉。
    // **在本地页上则不动它**：那条栏还在，用户可以自己决定什么时候停。
    if (!dualPlay && !localPage && localPlayer) void stopLocalPlayback(view);
    renderLocalNowbar(view);
    syncHead(view);
    note(view, t(dualPlay ? "music.dual_on" : "music.dual_off"));
  };
  on(["music-dual-btn", "music-l-dual-btn"], toggleDual);
  // 音量条：**拖动过程中就要跟手**（2026-09-29 用户要求）。
  //
  // 以前只听 `change` —— 那是「松手」才触发的事件，所以拖动全程音量不动，用户看到的是
  // 「拖完了才跳一下」。改成听 `input`（拖动时连续触发）。
  //
  // 但每次都要走一条 `spotify_control`（Web API），高频 `input` 直接发会把请求打爆，
  // 所以做「**单飞 + 追最新**」：同时只允许一个请求在飞；飞的过程中用户又拖了，就记下
  // 最新值，落地后补发一次。**最终值一定会发出去**，中间值允许被合并掉（那正是我们要的节流）。
  // `step` 也从 5 改到 1 —— 21 档拖起来是「一段一段跳」的，101 档才叫渐进。
  let volTarget: number | null = null;
  let volInFlight = false;
  const pushVolume = async (v: number) => {
    volTarget = v;
    if (volInFlight) return;
    volInFlight = true;
    try {
      while (volTarget !== null) {
        const next = volTarget;
        volTarget = null;
        await control("volume", next);
      }
    } finally {
      volInFlight = false;
    }
  };
  for (const id of ["music-vol", "music-b-vol"]) {
    q<HTMLInputElement>(`#${id}`)?.addEventListener("input", (ev) => {
      const el = ev.target as HTMLInputElement;
      const v = Number(el.value);
      // **填充要当场重画**：拖动期间 `renderStatus` 那一路是**跳过**的
      // （`document.activeElement === el` 时不许抢），不在这里补一刀就会出现
      // 「滑块在动、底下那条填充不动」。
      paintVolume(el, v);
      void pushVolume(v);
    });
  }

  // 三态循环：关 → 随机 → 单曲循环 → 关。**不提供智能随机** ——
  // Spotify Web API 没有 Smart Shuffle 接口（连读都读不到），见 ai-spec §4.6。
  // （Spotify 是「随机」与「循环」两个独立按钮，本仓合成一个三态钮：宿主那条命令
  //   `spotify_set_play_mode` 收的就是 off / shuffle / repeat_one 三个值。）
  on(["music-mode", "music-b-mode"], () => {
    const cur = playMode();
    const next = cur === "off" ? "shuffle" : cur === "shuffle" ? "repeat_one" : "off";
    void invoke("spotify_set_play_mode", { mode: next }).catch((e) => msg(errText(e)));
  });

  // ── 进度条：点击 = 跳转；拖动 = 跟手 + 显示时间 ──
  // 用 pointer 事件而不是 `click`：拖动时要**实时**看见填充与时间跟着走，松手才真跳
  // （否则拖一次会打出几十个 seek 请求，还会把播放位置来回拉扯）。
  // **两条进度线共用这一套绑定**：播放长条上那条 3px + 底部栏顶上那条 2px 细线。
  seekBars = ["#music-pv-progress", "#music-bp"]
    .map(sel => q<HTMLElement>(sel))
    .filter((el): el is HTMLElement => !!el);
  for (const bar of seekBars) {
    const ratioAt = (clientX: number): number => {
      const r = bar.getBoundingClientRect();
      return Math.max(0, Math.min(1, (clientX - r.left) / Math.max(1, r.width)));
    };
    /** 只画不动播放：拖动期间每帧调它（两条进度线的填充 / 时间气泡一起跟手）。 */
    const paintSeek = (ratio: number) => {
      seekRatio = ratio;
      const pct = `${ratio * 100}%`;
      for (const el of seekBars) el.style.setProperty("--seek-pct", pct);
      const dur = player?.track?.duration_ms ?? 0;
      const at = fmtClock(dur * ratio);
      setText(bar.querySelector<HTMLElement>(".music-progress-tip"), fmtMs(dur * ratio));
      // 拖动时数字也要跟手：气泡是临时的、松手就没，而这两处是常驻的读数
      //（只让填充跟着走、数字停在原处，看起来就像「拖了没生效」）。
      setText(q<HTMLElement>("#music-pv-pos"), at);
      setText(q<HTMLElement>("#music-b-time"), `${at} / ${fmtClock(dur)}`);
    };
    bar.addEventListener("pointerdown", (ev) => {
      if ((player?.track?.duration_ms ?? 0) <= 0) return;
      // 捕获指针：拖出轨道范围后仍然收得到 pointermove / pointerup。
      // `try` 是给**合成事件**留的（没有活动指针时 setPointerCapture 会抛 NotFoundError）。
      try { bar.setPointerCapture(ev.pointerId); } catch { /* 没有活动指针 */ }
      bar.classList.add("music-dragging");
      paintSeek(ratioAt(ev.clientX));
    });
    bar.addEventListener("pointermove", (ev) => {
      if (seekRatio < 0) return;
      paintSeek(ratioAt(ev.clientX));
    });
    const endSeek = (ev: PointerEvent) => {
      if (seekRatio < 0) return;
      const ratio = seekRatio;
      seekRatio = -1; // 先解锁，下一轮 `renderPlayer` 才能重新接管填充与圆点
      bar.classList.remove("music-dragging");
      try { bar.releasePointerCapture(ev.pointerId); } catch { /* 指针已经不在了 */ }
      void control("seek", Math.round((player?.track?.duration_ms ?? 0) * ratio));
    };
    bar.addEventListener("pointerup", endSeek);
    bar.addEventListener("pointercancel", endSeek);
  }

  // 歌词区：滚轮 / 拖条 ⇒ 挂起自动跟随；**点某一行 ⇒ 跳到那一句**
  const lyrBox = q<HTMLElement>("#music-lyr");
  /** 指针在歌词区按下后有没有移动过。**拖滚动条松手同样会发 `click`** ——
   *  用它把「点一行歌词」与「拖完滚动条松手」分开，否则每拖一次都会跳进度。 */
  let lyrMoved = false;
  let lyrY = 0;
  lyrBox?.addEventListener("wheel", () => suspendFollow(view), { passive: true });
  lyrBox?.addEventListener("pointerdown", (ev) => {
    lyrMoved = false;
    lyrY = ev.clientY;
    suspendFollow(view);
  });
  lyrBox?.addEventListener("pointermove", (ev) => {
    if (Math.abs(ev.clientY - lyrY) > 4) lyrMoved = true;
  });
  lyrBox?.addEventListener("click", (ev) => {
    if (lyrMoved) return;
    const el = (ev.target as HTMLElement).closest<HTMLElement>(".music-lyr-line");
    const line = el ? lrcLines[Number(el.dataset.i)] : undefined;
    if (line) void control("seek", Math.round(line.at * 1000));
  });

  // 初始加载
  await refreshConfig(view);
  // 调音状态**不在这里拉**（2026-10-01 改）：它现在只服务调音页那一块，
  // 而那一页进来时 `renderMode` 会自己 `loadTuning`（见 renderMode）。
  // 在这里预拉一次只是给一个还没被打开过的页面备数据，白花一次 IPC。
  renderMode(view);
  renderPlayer(view);
  renderLyrics(view);
  renderQueue(view);
  syncPlayerBody(view);
  renderStatus(view);
  // 429 硬闸是**进程级 + 粘性**的：面板关掉再打开时模块态是新的，必须问宿主一次，
  // 否则用户会看到「请求全被拒 + 没有恢复按钮」的死局（见 SpotifyApiStateDto）。
  // **放在 startPolling 之前**：别让第一次 tick 在「还不知道闸拉着」时白打一轮请求。
  await refreshApiState(view);
  startPolling(view);
  // 「播放在哪」自动就位（2026-09-30）：桌面端 / 手机端在就直接用它，一个官方端都没有
  // 才自动拉起 librespot。**必须在这里做**，否则桌面端没开时点歌会落到
  // 「没有活跃设备」上（`PUT /me/player/play` 不带 device_id 时只认活跃设备）。
  // 不 await：它内部可能要等 librespot 注册（最多约 6 秒），不该把首屏卡住。
  void ensurePlayTarget(view);
}

/** 播放一个上下文（歌单）。`offsetUri` 给定时从那一首开始 —— 这样队列就是整个歌单，
 *  「歌单里的歌」也就跟着被监控到（`player.context_uri` 会指向这个歌单）。 */
async function playContext(root: HTMLElement, contextUri: string, offsetUri: string) {
  if (!contextUri) return;
  try {
    await invoke("spotify_play_context", { contextUri, offsetUri: offsetUri || undefined, deviceId: playTarget || undefined });
  } catch (e) {
    setText(root.querySelector("#music-msg"), errText(e));
  }
}

/** 从某一首开始播（队列项点击）。Spotify 没有「删除队列项」接口，这是最接近的效果。 */
async function playUri(root: HTMLElement, uri: string) {
  await enforceExclusive(root, "spotify");
  try {
    await invoke("spotify_play_uri", { uri, deviceId: playTarget || undefined });
  } catch (e) {
    setText(root.querySelector("#music-msg"), errText(e));
  }
}

/** 供 attach.ts 的 detachPluginListeners 调用（显式停表；另有 `isConnected` 自停兜底）。 */
export function stopMusicPolling() {
  // **曲线窗那条轮询也在这一条钩子上**（2026-10-03）：`attach.ts` 的 detach 只认这个函数，
  // 曲线窗与主窗共用它 ⇒ 两条都要在这里收，否则关掉曲线窗后它还在每 1.2s 打一次 IPC。
  stopCurvePoll();
  // 调音页那条轻轮询同理：页面切走后它自己会停，但关窗时也该立刻收掉。
  stopTunePagePoll();
  stopPolling();
  // 面板没了 ⇒ 两个全局监听器（`visibilitychange` / 交互）从此空转（它们只认 `currentRoot`）。
  currentRoot = null;
  if (followTimer !== undefined) { window.clearTimeout(followTimer); followTimer = undefined; }
  if (resizeTimer !== undefined) { window.clearTimeout(resizeTimer); resizeTimer = undefined; }
  // 本地页那两条表也要收：扫描进度轮询**不属于 tick 那条链**，不显式停就会一直
  // 挂着（它虽有 `root.isConnected` 自停，但那要等下一拍 —— 显式停更利落）。
  stopMediaPoll();
  if (mediaQueryTimer !== undefined) { window.clearTimeout(mediaQueryTimer); mediaQueryTimer = undefined; }
  // 本地播放也要收：宿主那条流的生命周期**只挂在「音乐插件窗关闭」上**
  // （`music::on_plugin_window_destroyed`），而插件被嵌在主窗口里时根本没有那扇窗
  // ——摘掉插件是这里唯一能走到的地方，不显式收就会留下一条没有界面的音频流。
  stopLocalPoll();
  if (localPlayer?.path) {
    // 先把「停在哪一首 / 几分几秒」写下来再清内存 —— 下次挂载要从它恢复
    // （见 attach 里的 readLocalLast）。
    localResumeMs = localPlayer.position_ms;
    writeLocalLast();
    localPlayer = null;
    localQueue = [];
    localQueueIndex = -1;
    void invoke("player_stop").catch(() => {});
  }
  if (unlistenAuth) { unlistenAuth(); unlistenAuth = null; }
  // 进度线节点随面板一起被拆掉了，别留着悬空引用（下次 attach 会重新收集）
  seekBars = [];
  // 抽屉的「翻到另一侧」也不带出去（下次挂载从「当前页自己那条」开始，见 barSwap）
  barSwap = false;
  // 标题栏被我们换成「← 」的话要还原：悬浮窗会被复用去装别的插件
  restoreTitlebarClose();
  // 三段切换同理：它长在**标题栏**里（不在 `.music-root` 内），面板拆掉时
  // 谁也带不走它 —— 不显式摘掉，下一个装进来的插件就会顶着「在线歌单 / 本地音乐 /
  // 调音」三条按钮。内嵌形态它长在工具条里（会随 root 一起消失），重复调用无副作用。
  removeSwitcher();
}

// 挂到 window 给 attach.ts 调（本模块是**懒加载**的：只有打开过音乐插件才存在这个钩子，
// 那边用 `?.()` 调用，不会因未加载而报错）。
(window as any).__lunac_music_stop = stopMusicPolling;

// ── 磁盘插件钩子（2026-09-28）─────────────────────────────────────
// 音乐插件要能被**独立打包**成 `<exe 根>\Modules\music\` 下的磁盘插件（见
// `app/vite.plugins.config.ts` + `scripts/build-plugins.ps1`）。磁盘插件的挂载只认
// 入口模块导出的 `attach(root)` / `detach()`（`market.ts::pickAttach`），所以把这两个
// 已有的函数转出去 —— 内置构建里多两个导出没有任何副作用（`attach.ts` 只在**磁盘上
// 没有可用的一份**时才走它那张硬编码表，见那张表头上的「磁盘插件优先」）。
export { attachMusicListeners as attach, stopMusicPolling as detach };

// ── Plugin 定义 ───────────────────────────────────────────────────

export const musicPlugin: Plugin = {
  id: "music",
  name: "音乐歌词",
  keywords: ["音乐", "歌词", "歌曲", "正在播放", "music", "lyrics", "spotify", "播放控制", "暂停", "下一首", "歌单", "播放列表"],
  description: "歌词自动抓取 (LRCLIB + 网易云兜底) + Spotify 播放控制 / 歌单 / 队列",
  icon: "🎵",
  badge: "music",
  // 「本插件一律在**悬浮窗**里打开」（2026-09-29 从 main.ts 的硬编码 `id === "music"` 改成声明）。
  // 为什么要声明而不是让宿主按 id 判：音乐已归入**拓展插件**，要能从市场独立打包分发
  // —— 磁盘插件的代码宿主没法写死，行为只能由插件自己声明（见 ai-spec §3.5「插件契约」）。
  // 这也是「用户 2026-09-28 定的：从 Lunac 进音乐界面时自动展开成独立界面」那条的执行点。
  permissions: ["window.float"],

  async execute(): Promise<PluginResult> {
    // **同一个插件、两扇窗、两份外壳**：宿主建曲线窗时用的是同一个 `plugin.html` + 同一份
    // `plugin_id`（`music`），只有窗口 label 不同 ⇒ 分岔点只能是这里（见 `isCurveWindow`）。
    return { type: "html", content: isCurveWindow ? curveShellHtml() : shellHtml() };
  },
};

// 插件契约要求入口**默认导出**带 `execute` 的对象（`market.ts::pickExecute` 只认
// 「默认导出是函数」/「默认导出对象上的 execute」/「具名导出 execute」三种形态）。
// 只导 `musicPlugin` 这个名字的话，独立打包出来的包在市场里会报「没有可调用的 execute」
//  —— 内置构建看不出来（那边直接 import 具名符号），所以这条必须在这里补上。
export default musicPlugin;
