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
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-shell";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";

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

/** 左栏那四类 + 收藏夹（收藏夹是前端合成的、没有可播上下文）。 */
type SideKind = "playlist" | "album" | "artist" | "show";
type MainKind = SideKind | "liked";

/** 一台可控制的 Connect 设备（`spotify_devices`）。 */
interface DeviceDto {
  id: string;
  name: string;
  kind: string;
  active: boolean;
  volume_percent: number;
}
/** 本机播放（librespot 子进程）的状态。 */
interface LibrespotDto {
  running: boolean;
  /** 找得到可执行文件 —— false 时面板把开关置灰并说清「去哪填路径」。 */
  available: boolean;
  path: string;
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
/** 左栏宽度（px）。拖动分隔条改它，**只写进 CSS 变量**（见 `applySideWidth`）。 */
let sideWidth = SIDE_W_DEFAULT;
/** 上一次同步给宿主的 `resizable`，避免每轮 tick 重复调宿主。 */
let lastResizable: boolean | null = null;
/** 标题栏那个关闭按钮**原样的 × 标记**（播放态会把它临时换成「← 回退」，
 *  离开播放态 / 离开这个插件时必须原样换回去，见 `syncTitlebarClose`）。 */
let closeBtnOriginal = "";

let pollTimer: number | undefined;
let pollBusy = false;
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

type MusicMode = "default" | "player";

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
function shellHtml(): string {
  return `
  <div class="music-root">
    <!-- ── 默认界面：顶部工具条 + 状态行 + 连接设置 + 歌单列 / 搜索结果 ──
         工具条与「返回」按钮**分属两态**：默认态用工具条上的「进播放界面」，
         播放态用歌名模块右端那个「返回」（music-back 这个 id 全文档只有一个）。 -->
    <div class="music-view" id="music-v-default">
      <!-- 顶部工具条：头像 / 账号 / 搜索（**带下拉预览**）/ 状态 / 设置。
           2026-09-28 第二次改：设备按钮与「展开播放面板」都**下移到最底下那条控制栏**
           （用户要求「严格对齐 Spotify」，Spotify 的工具条上本来就没有这两样）。 -->
      <div class="music-toolbar">
        <span class="music-avatar" id="music-avatar"></span>
        <span class="music-acc" id="music-acc"></span>
        <div class="music-search-wrap">
          <input class="music-search" id="music-search" type="search" autocomplete="off" spellcheck="false" placeholder="${esc(t("music.search_ph"))}">
          <!-- 下拉预览：绝对定位挂在工具条下方（**不占布局**，否则窗口高会跟着抖） -->
          <div class="music-dd hidden" id="music-search-dd"></div>
        </div>
        <span class="music-status" id="music-status">${t("music.loading")}</span>
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
            <input id="music-librespot-proxy" type="text" spellcheck="false" autocomplete="off" placeholder="http://127.0.0.1:7890">
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
      <div class="music-nowbar hidden" id="music-nowbar">
        <div class="music-bp" id="music-bp" title="${esc(t("music.seek"))}">
          <div class="music-bp-fill" id="music-bp-fill"></div>
          <div class="music-progress-tip" id="music-bp-tip">0:00</div>
        </div>
        <button class="music-bnow" id="music-bnow" title="${esc(t("music.open_player"))}">
          <span class="music-bnow-cover" id="music-bnow-cover"></span>
          <span class="music-bnow-main">
            <span class="music-bnow-n" id="music-bnow-n"></span>
            <span class="music-bnow-a" id="music-bnow-a"></span>
          </span>
        </button>
        <div class="music-bctl">
          <button class="music-round-btn music-mode-btn" id="music-b-mode" data-mode="off"></button>
          <button class="music-round-btn" id="music-b-prev" title="${esc(t("music.prev"))}">${SVG.prev}</button>
          <button class="music-round-btn music-play-btn" id="music-b-play" title="${esc(t("music.play_pause"))}">${SVG.play}</button>
          <button class="music-round-btn" id="music-b-next" title="${esc(t("music.next"))}">${SVG.next}</button>
        </div>
        <div class="music-bend">
          <button class="music-ghost-btn music-icon-btn" id="music-devices-btn" title="${esc(t("music.devices"))}">${SVG.speaker}</button>
          <span class="music-vol-ico">${SVG.volume}</span>
          <input class="music-vol" id="music-b-vol" type="range" min="0" max="100" step="1" value="50" title="${esc(t("music.volume"))}">
          <button class="music-round-btn" id="music-b-queue" title="${esc(t("music.queue"))}">${SVG.list}</button>
          <button class="music-ghost-btn music-icon-btn hidden" id="music-open-player" title="${esc(t("music.open_player"))}">${SVG.expand}</button>
          <div class="music-dd music-dev-dd hidden" id="music-dev-dd"></div>
          <div class="music-dd hidden" id="music-q-dd"></div>
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
          <!-- 歌名模块 H30：歌名（亮）+ 歌手·专辑（暗）+ 返回入口。
               「返回」放在这里而不是标题行 —— 播放态整条只有 130 高，
               标题行（状态 / 账号 / 齿轮）在默认态里，这一态不显示。
               时间不在这里（用户 2026-09-27 要求挪到控制块最左端、进度条下方）。 -->
          <div class="music-name">
            <div class="music-name-text">
              <span class="music-title" id="music-pv-title"></span>
              <span class="music-sub" id="music-pv-sub"></span>
            </div>
            <button class="music-ghost-btn music-icon-btn hidden" id="music-back" title="${esc(t("music.back"))}">${SVG.back}</button>
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
              <input class="music-vol" id="music-vol" type="range" min="0" max="100" step="1" value="50" title="${esc(t("music.volume"))}">
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

    <div class="music-msg" id="music-msg"></div>
  </div>`;
}

/** 只在「该节点还没显示这个值」时写 DOM —— 每 1s 一次轮询不能造成闪烁。 */
function setText(el: HTMLElement | null, text: string) {
  if (el && el.textContent !== text) el.textContent = text;
}
function setHtml(el: HTMLElement | null, html: string) {
  if (el && el.innerHTML !== html) el.innerHTML = html;
}
function show(el: HTMLElement | null, on: boolean) {
  el?.classList.toggle("hidden", !on);
}
function setAttr(el: HTMLElement | null, name: string, v: string) {
  if (el && el.getAttribute(name) !== v) el.setAttribute(name, v);
}

/** 把挂在 `#music-msg` 上的一次性提示按时抹掉。**每个 tick 调一次**。
 *
 *  **为什么用「看门狗」而不是在每个写入处加定时器**：那个元素有七八处写入方
 *  （控制失败、歌单拉取失败、歌词失败、连接成功提示…），逐处加定时器必然漏一处，
 *  漏掉的那一处就是又一条永久驻留的假状态。这里只统计「同一段文字挂了多久」：
 *  换字即为新提示、时钟归零，于是所有写入方都自动获得 8 秒后消失的行为。
 *  **按真实时间判、不按 tick 计数**：tick 的真实周期是「轮询耗时 + 1000ms」，
 *  实测被网络拉到 ~1.7s ⇒ 数 tick 的话 8 个 tick 会变成十几秒（2026-09-27 实测到）。
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
  const isPlayer = mode === "player";
  show(root.querySelector("#music-v-default"), !isPlayer);
  show(root.querySelector("#music-v-player"), isPlayer);
  // 播放态：提示行**不占布局**（浮在长条上）—— 用户要求播放态窗口严格 130 高。
  // 用 root 上的类切，比一串 `:has()` 好读也好调（默认态它照样在流里）。
  // 顺带一提：**默认态的外层垫料清不掉**（那条 CSS 只对 `music-player-on` 生效），
  // 窗口尺寸也按态区分，见 `applyResize`。
  root.classList.toggle("music-player-on", isPlayer);
  // **播放态禁用缩放**（用户 2026-09-28 定）：定尺长条手动拉一下只会被下一轮 tick
  // 贴回去，关掉它交互才一致。只在态变化时下发一次，别放进每秒那轮 tick。
  syncResizable(isPlayer);
  // 播放态顺带把标题栏那个 × 换成「← 回退」（同一个位置，两种语义）
  syncTitlebarClose();
  syncHead(root);
  if (isPlayer) {
    // 进播放界面时歌词盒刚从 display:none 里出来，几何量为 0 ⇒ 强制重新定位
    activeLine = -1;
    renderLyrics(root);
    syncLyrics(root); // 立刻对齐一次，别等下一秒那轮 tick
  } else {
    // 默认面板：主体只在连上之后才有意义（没登录时设置面板就是全部内容）
    show(root.querySelector("#music-body"), !!cfg?.connected);
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
  const want = mode === "player" ? "back" : "close";
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

/** 头顶那两个「进 / 出播放界面」按钮。每 tick 都会调（`show()` 幂等且只碰 class）——
 *  播放状态变了要能立刻反映，不能只等 `renderMode()` 那次。
 *  同时**在这里管底部控制栏的显隐**：没连上 Spotify 时那条栏没有意义（那时面板上
 *  只有连接设置），连上之后才出现 —— 设备按钮长在那条栏里，藏掉它就等于没法换设备。 */
function syncHead(root: HTMLElement) {
  const isPlayer = mode === "player";
  // 默认界面里若已有正在播的歌，给一个回播放界面的入口（否则「返回」是单向门）
  show(root.querySelector("#music-open-player"), !isPlayer && !!player?.track);
  show(root.querySelector("#music-back"), isPlayer);
  show(root.querySelector("#music-nowbar"), !isPlayer && !!cfg?.connected);
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
  show(root.querySelector("#music-setup"), !cfg?.connected);
  // 主体与工具条按钮的显隐跟「连没连」走；两栏的内容由 renderSide / renderMain 管
  show(root.querySelector("#music-body"), !!cfg?.connected);
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
  setText(root.querySelector("#music-redirect"), cfg?.redirect_uri ?? "");
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

/** 左栏列表。**四类只有一个渲染入口** —— 歌单那一栏额外把收藏夹合成在第一行。 */
function renderSide(root: HTMLElement) {
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
  let html = sideTab === "playlist" ? likedRowHtml() : "";
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

/** 主区。三种内容互斥：**搜索详细页 > 选中项详情 > 引导文案**。 */
function renderMain(root: HTMLElement) {
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
    : selected.kind === "playlist"
      ? t("music.playlists")
      : selected.kind === "album"
        ? t("music.tab_albums")
        : selected.kind === "artist"
          ? t("music.tab_artists")
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
  if (local?.available) {
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
      : await invoke<LibraryItemDto[]>(
          kind === "album" ? "spotify_albums" : kind === "artist" ? "spotify_artists" : "spotify_shows",
        );
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
    mainTracks = await invoke<TrackDto[]>("spotify_item_tracks", { kind, id: it.id });
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
  try {
    if (kind === "liked" || kind === "show") {
      const list = (kind === "liked" ? liked?.items : mainTracks) ?? [];
      if (!list.length) return;
      await invoke("spotify_play_uris", { uris: list.slice(Math.max(0, fromIdx)).map(tr => tr.uri) });
    } else if (it.uri) {
      await invoke("spotify_play_context", { contextUri: it.uri, offsetUri: "" });
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
  try {
    if (selected.kind === "liked" || selected.kind === "show") {
      await invoke("spotify_play_uris", { uris: list.slice(idx).map(x => x.uri) });
    } else {
      await invoke("spotify_play_context", { contextUri: selected.uri, offsetUri: tr.uri });
    }
  } catch (e) {
    note(root, errText(e));
  }
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
  renderDevices(root);
  if (started) {
    devices = await invoke<DeviceDto[]>("spotify_devices").catch(() => []);
    renderDevices(root);
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
      if (mine) break;
    } catch {
      // 刚起来那几秒接口可能还没认到它，继续等
    }
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
  setText(root.querySelector("#music-pv-pos"), fmtMs(pos));
  setText(root.querySelector("#music-pv-dur"), fmtMs(dur));

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

  const volume = player && player.volume_percent >= 0 ? String(player.volume_percent) : "";
  for (const vol of [root.querySelector<HTMLInputElement>("#music-vol"), root.querySelector<HTMLInputElement>("#music-b-vol")]) {
    // 用户正抓着音量条时不要抢（`document.activeElement` 也是拉条本身）
    if (vol && volume && document.activeElement !== vol) vol.value = volume;
  }

  // 「有没有活跃设备」只该管播放键：**设备按钮与展开按钮不在此列** ——
  // 没有活跃设备时恰恰要用它们（去挑一台设备 / 去看播放面板）。
  const disabled = !player?.active;
  root.querySelectorAll<HTMLButtonElement>(".music-bar button, .music-nowbar button").forEach(b => {
    if (b.id === "music-devices-btn" || b.id === "music-open-player" || b.id === "music-bnow") return;
    b.disabled = disabled;
  });
}

/** 按**真实流逝时间**把进度条推到「此刻应有的位置」—— 一次网络都不打。
 *
 *  **为什么需要这层本地插值**（用户 2026-09-29 报「进度条一跳一跳」）：
 *  宿主那轮轮询的真实周期是「Web API 往返 + 1000ms」，实测被网络拉到 ~1.7s ⇒
 *  只靠它写 `--seek-pct`，用户看到的是「跳一格、僵一两秒、再跳一格」。
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
  setText(root.querySelector("#music-pv-pos"), fmtMs(pos));
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
        // 每秒一次「按内容贴合尺寸」：内容变了（提示行换行 / 队列面板的首屏）或
        // 上一次因窗口最小化被拒，都会在这里自己补上。同值会被 `lastResize` 挡掉，
        // 所以不会与用户手动拖出来的尺寸打架。
        scheduleResize(root);
      } catch (e) {
        if (myGen !== gen) return;
        // 令牌失效（宿主已清空）⇒ 回落到配置态，让用户重新登录
        if (String(e).includes("ERR_NOT_CONNECTED")) {
          await refreshConfig(root);
        } else {
          setText(root.querySelector("#music-msg"), errText(e));
        }
      } finally {
        pollBusy = false;
      }
    }
    if (player?.track) void fetchLyricsFor(player.track);
    // 队列只在「面板开着 + 换歌了」时重拉：它每拉一次就是一次 Web API 调用
    if (queueOpen && player?.track && player.track.id !== queueFor) void loadQueue(root);
  }
  if (myGen !== gen) return;
  pollTimer = window.setTimeout(() => void tick(root, myGen), 1000);
}

function stopPolling() {
  if (pollTimer !== undefined) {
    window.clearTimeout(pollTimer);
    pollTimer = undefined;
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
  if (mode === "player") {
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
  const view = root.querySelector<HTMLElement>(".music-root");
  if (!view) return;
  currentRoot = view;
  // 每次挂载都回到初始形态：这个模块的状态是跨挂载存活的（懒加载模块），
  // 「上次打开时在播放界面」不该决定这一次。
  mode = "default";
  fieldsOverride = null;
  queueOpen = false;
  lastResize = "";
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
  // 搜索同理：面板重开时不该带着上次的词
  searchQuery = "";
  searchResult = null;
  searchLoading = false;
  searchOpen = false;
  searchPage = false;
  searchTab = "tracks";
  // 上一次挂载的进度线节点已经随 DOM 一起没了（`seekBars` 每次 attach 重新收集）
  seekBars = [];

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
  q<HTMLElement>("#music-setup-toggle")?.addEventListener("click", () => {
    mode = "default";
    renderMode(view);
    const box = q<HTMLElement>("#music-setup");
    if (box) box.classList.toggle("hidden");
  });
  q<HTMLElement>("#music-open-player")?.addEventListener("click", openPlayer);
  q<HTMLElement>("#music-bnow")?.addEventListener("click", openPlayer);
  q<HTMLElement>("#music-back")?.addEventListener("click", backToDefault);

  // ── 播放态：标题栏那个按钮的语义改写（见 syncTitlebarClose / stopAndBack）──
  // **必须在捕获阶段拦**：`#plugin-close` 的关窗监听器在 `plugin-window.ts` 里
  // （所有插件共用的入口，不能为一个插件改它）。捕获阶段挂在 `document` 上先于它执行，
  // `stopPropagation()` 就能让事件根本走不到那个按钮 ⇒ 共用入口一行都不用动。
  const onTitlebarClick = (ev: Event) => {
    if (!view.isConnected || mode !== "player") return;
    if (!(ev.target as HTMLElement | null)?.closest("#plugin-close")) return;
    ev.stopPropagation();
    ev.preventDefault();
    void stopAndBack(view);
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
      });
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
    }
  };
  for (const sel of ["#music-side-list", "#music-side-tabs", "#music-main", "#music-search-dd", "#music-dev-dd", "#music-q", "#music-q-dd"]) {
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
      void pushVolume(Number((ev.target as HTMLInputElement).value));
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
      setText(bar.querySelector<HTMLElement>(".music-progress-tip"), fmtMs((player?.track?.duration_ms ?? 0) * ratio));
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
  renderMode(view);
  renderPlayer(view);
  renderLyrics(view);
  renderQueue(view);
  syncPlayerBody(view);
  renderStatus(view);
  startPolling(view);
}

/** 播放一个上下文（歌单）。`offsetUri` 给定时从那一首开始 —— 这样队列就是整个歌单，
 *  「歌单里的歌」也就跟着被监控到（`player.context_uri` 会指向这个歌单）。 */
async function playContext(root: HTMLElement, contextUri: string, offsetUri: string) {
  if (!contextUri) return;
  try {
    await invoke("spotify_play_context", { contextUri, offsetUri: offsetUri || undefined });
  } catch (e) {
    setText(root.querySelector("#music-msg"), errText(e));
  }
}

/** 从某一首开始播（队列项点击）。Spotify 没有「删除队列项」接口，这是最接近的效果。 */
async function playUri(root: HTMLElement, uri: string) {
  try {
    await invoke("spotify_play_uri", { uri });
  } catch (e) {
    setText(root.querySelector("#music-msg"), errText(e));
  }
}

/** 供 attach.ts 的 detachPluginListeners 调用（显式停表；另有 `isConnected` 自停兜底）。 */
export function stopMusicPolling() {
  stopPolling();
  if (followTimer !== undefined) { window.clearTimeout(followTimer); followTimer = undefined; }
  if (resizeTimer !== undefined) { window.clearTimeout(resizeTimer); resizeTimer = undefined; }
  if (unlistenAuth) { unlistenAuth(); unlistenAuth = null; }
  // 进度线节点随面板一起被拆掉了，别留着悬空引用（下次 attach 会重新收集）
  seekBars = [];
  // 标题栏被我们换成「← 」的话要还原：悬浮窗会被复用去装别的插件
  restoreTitlebarClose();
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
    return { type: "html", content: shellHtml() };
  },
};

// 插件契约要求入口**默认导出**带 `execute` 的对象（`market.ts::pickExecute` 只认
// 「默认导出是函数」/「默认导出对象上的 execute」/「具名导出 execute」三种形态）。
// 只导 `musicPlugin` 这个名字的话，独立打包出来的包在市场里会报「没有可调用的 execute」
//  —— 内置构建看不出来（那边直接 import 具名符号），所以这条必须在这里补上。
export default musicPlugin;
