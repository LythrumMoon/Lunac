// ── 音乐插件（歌词 + Spotify 播放控制）──────────────────────────
// 2026-09-27。用户要求「歌词抓取」与「Spotify 播放控制」合成**一个**功能插件。
//
// 分工：
//   · 前端只画界面 + 轮询；**一切联网都在宿主**（`src-tauri/src/music.rs`）。
//     原因：`tauri.conf.json` 的 CSP 是 `default-src 'self' https://asset.localhost`，
//     不含远端域 ⇒ 插件里 `fetch()` 会被拦掉（与插件市场索引同一条理由，ai-spec §11 规则 67）。
//   · 歌词源 = LRCLIB（免费、无需 key）。Spotify 走官方 OAuth（PKCE + 环回地址）。
//
// 界面（内嵌插件视图 360px 高，不是 detached）：
//   [状态行]                          [设置]
//   [封面] 歌名 / 歌手 · 专辑 / 进度条 / 时间
//   [上一首] [播放暂停] [下一首]  [音量 ——●——]
//   [歌词头：来源 + 手动搜索框] [搜索]
//   [歌词区（可滚，同步行高亮）]
//
// 纪律：
//   · **不整块 innerHTML 重绘**：每 1s 一次轮询，整块重绘会打断输入框焦点、
//     把歌词滚动位置打回顶部。只 patch 具体节点（见 renderPlayer / renderLyrics）。
//   · 轮询用 setTimeout 链 + busy 标志（不是 setInterval）—— 网络慢时调用不会叠起来。
//   · 关面板/切插件后必须停表：root 脱离文档（`isConnected === false`）即自停，
//     另外 main.ts 的 closePluginView 会显式调 `window.__lunac_music_stop`。

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
}
interface TrackDto {
  id: string;
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

// ── 线性 SVG（docs/icon-style.md §1：stroke currentColor，不嵌 emoji）──
const SVG = {
  play: `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polygon points="6 4 20 12 6 20 6 4"/></svg>`,
  pause: `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><line x1="9" y1="4" x2="9" y2="20"/><line x1="15" y1="4" x2="15" y2="20"/></svg>`,
  prev: `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polygon points="19 5 9 12 19 19 19 5"/><line x1="5" y1="5" x2="5" y2="19"/></svg>`,
  next: `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polygon points="5 5 15 12 5 19 5 5"/><line x1="19" y1="5" x2="19" y2="19"/></svg>`,
  copy: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15V5a2 2 0 0 1 2-2h10"/></svg>`,
  link: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M10 13a5 5 0 0 0 7.5.5l3-3a5 5 0 0 0-7-7l-1.5 1.5"/><path d="M14 11a5 5 0 0 0-7.5-.5l-3 3a5 5 0 0 0 7 7L12 19"/></svg>`,
  power: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M18.4 6.6a9 9 0 1 1-12.8 0"/><line x1="12" y1="2" x2="12" y2="12"/></svg>`,
  gear: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>`,
};

// ── 模块级状态 ────────────────────────────────────────────────────
let cfg: MusicConfigDto | null = null;
let player: PlayerDto | null = null;
let lyrics: LyricsDto | null = null;
/** 已经抓过歌词的曲目 id，避免每分钟重复打 LRCLIB。 */
let lyricsFor = "";
let lyricsLoading = false;
/** 用户手动搜过并选中候选后，就不再被「自动抓当前曲目」覆盖。换歌时自动复位。 */
let manualPick = false;
/** 手动搜索的候选（钉在歌词上方，点一条即切换）。 */
let cands: LyricsDto[] = [];
let lrcLines: LrcLine[] = [];
let activeLine = -1;

let pollTimer: number | undefined;
let pollBusy = false;
let unlistenAuth: (() => void) | null = null;
let currentRoot: HTMLElement | null = null;
/** 运行代次：attach 每次递增，旧回调据此丢弃自己的结果（防「关了又开」时的串扰）。 */
let gen = 0;

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

/** 把宿主返回的哨兵错误码翻成当前语言；其余原样显示。 */
function errText(e: unknown): string {
  const raw = e instanceof Error ? e.message : String(e);
  if (raw.includes("ERR_NO_CLIENT_ID")) return t("music.err_no_client_id");
  if (raw.includes("ERR_NOT_CONNECTED")) return t("music.err_not_connected");
  if (raw.includes("ERR_NO_QUERY")) return t("music.err_no_query");
  if (raw.includes("ERR_PORT_BUSY")) return t("music.err_port_busy", { port: String(cfg?.port ?? "") });
  const m = raw.match(/ERR_RATE_LIMITED:(\S*)/);
  if (m) return t("music.err_rate_limited", { s: m[1] || "?" });
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

// ── 视图 ──────────────────────────────────────────────────────────

function shellHtml(): string {
  return `
  <div class="music-root">
    <div class="music-head">
      <span class="music-status" id="music-status">${t("music.loading")}</span>
      <span class="music-device" id="music-device"></span>
      <button class="music-ghost-btn" id="music-setup-toggle" title="${esc(t("music.settings"))}">${SVG.gear}</button>
    </div>

    <!-- 配置区（未配置时默认展开） -->
    <div class="music-setup hidden" id="music-setup">
      <div class="music-hint">${t("music.setup_hint")}</div>
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
          <button class="music-ghost-btn" id="music-copy-redirect" title="${esc(t("music.copy"))}">${SVG.copy}</button>
        </div>
      </div>
      <div class="music-actions">
        <button class="music-btn" id="music-save">${t("music.save")}</button>
        <button class="music-btn music-primary" id="music-connect">${SVG.link} ${t("music.connect")}</button>
        <button class="music-btn" id="music-disconnect">${SVG.power} ${t("music.disconnect")}</button>
      </div>
      <div class="music-msg" id="music-msg"></div>
    </div>

    <!-- 播放器 -->
    <div class="music-player hidden" id="music-player">
      <div class="music-now">
        <div class="music-cover" id="music-cover"></div>
        <div class="music-meta">
          <div class="music-title" id="music-title"></div>
          <div class="music-sub" id="music-sub"></div>
          <div class="music-progress"><div class="music-progress-fill" id="music-progress"></div></div>
          <div class="music-time"><span id="music-pos">0:00</span><span id="music-dur">0:00</span></div>
        </div>
      </div>
      <div class="music-controls">
        <button class="music-round-btn" id="music-prev" title="${esc(t("music.prev"))}">${SVG.prev}</button>
        <button class="music-round-btn music-play-btn" id="music-play" title="${esc(t("music.play_pause"))}">${SVG.play}</button>
        <button class="music-round-btn" id="music-next" title="${esc(t("music.next"))}">${SVG.next}</button>
        <input class="music-vol" id="music-vol" type="range" min="0" max="100" step="5" value="50" title="${esc(t("music.volume"))}">
      </div>
    </div>

    <!-- 歌词 -->
    <div class="music-lyrics-wrap">
      <div class="music-lyrics-head">
        <span class="music-lyrics-title" id="music-lyrics-title">${t("music.lyrics")}</span>
        <input id="music-lyrics-q" type="text" spellcheck="false" autocomplete="off" placeholder="${esc(t("music.lyrics_ph"))}">
        <button class="music-ghost-btn" id="music-lyrics-search">${t("music.search")}</button>
      </div>
      <div class="music-lyrics" id="music-lyrics"></div>
    </div>
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

// ── 渲染 ──────────────────────────────────────────────────────────

function renderSetup(root: HTMLElement, force?: boolean) {
  const box = root.querySelector<HTMLElement>("#music-setup");
  const playerBox = root.querySelector<HTMLElement>("#music-player");
  if (!box || !playerBox) return;
  const needSetup = force ?? (!cfg?.connected || !cfg?.client_id);
  show(box, needSetup);
  show(playerBox, !needSetup);
  const idEl = root.querySelector<HTMLInputElement>("#music-client-id");
  const portEl = root.querySelector<HTMLInputElement>("#music-port");
  // 输入框里有内容时不要被轮询覆盖（用户可能正在敲）
  if (idEl && document.activeElement !== idEl) idEl.value = cfg?.client_id ?? "";
  if (portEl && document.activeElement !== portEl) portEl.value = String(cfg?.port ?? 8888);
  setText(root.querySelector("#music-redirect"), cfg?.redirect_uri ?? "");
}

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
  } else if (!player?.active) {
    setText(st, t("music.status_no_device"));
  } else {
    setText(st, player.playing ? t("music.status_playing") : t("music.status_paused"));
  }
  setText(
    root.querySelector("#music-device"),
    cfg?.connected && cfg.display_name ? t("music.account", { name: cfg.display_name }) : "",
  );
}

function renderPlayer(root: HTMLElement) {
  const track = player?.track ?? null;
  const cover = root.querySelector<HTMLElement>("#music-cover");
  const nextCover = track?.cover
    ? `<img src="${esc(track.cover)}" alt="">`
    : "";
  setHtml(cover, nextCover);
  setText(root.querySelector("#music-title"), track?.name || t("music.no_track"));
  const sub = [track?.artists, track?.album].filter(Boolean).join(" · ");
  setText(root.querySelector("#music-sub"), sub || (player?.active ? "" : t("music.no_device_hint")));

  const dur = track?.duration_ms ?? 0;
  const pos = Math.min(player?.progress_ms ?? 0, dur || Number.MAX_SAFE_INTEGER);
  const pct = dur > 0 ? Math.max(0, Math.min(100, (pos / dur) * 100)) : 0;
  const fill = root.querySelector<HTMLElement>("#music-progress");
  if (fill) fill.style.width = `${pct}%`;
  setText(root.querySelector("#music-pos"), fmtMs(pos));
  setText(root.querySelector("#music-dur"), fmtMs(dur));

  const playBtn = root.querySelector<HTMLElement>("#music-play");
  setHtml(playBtn, player?.playing ? SVG.pause : SVG.play);

  const vol = root.querySelector<HTMLInputElement>("#music-vol");
  if (vol && document.activeElement !== vol && player && player.volume_percent >= 0) {
    vol.value = String(player.volume_percent);
  }
  const disabled = !player?.active;
  root.querySelectorAll<HTMLButtonElement>(".music-controls button").forEach(b => { b.disabled = disabled; });
}

/** 重建歌词区。**只在数据变化时调用**（抓取完成 / 手动搜索 / 换歌）——
 *  它整块换 DOM，若放进每秒一次的 `tick` 会把滚动位置和焦点一起打掉。
 *  每 tick 只走 `syncActiveLine()`（仅改 class + 滚动）。 */
function renderLyrics(root: HTMLElement) {
  const box = root.querySelector<HTMLElement>("#music-lyrics");
  const title = root.querySelector<HTMLElement>("#music-lyrics-title");
  if (!box) return;
  activeLine = -1;
  lrcLines = [];

  // 手动搜索的候选钉在最上方（点一条即切换，见 attachMusicListeners 里的委托监听）
  const candHtml = cands.length
    ? `<div class="music-cands">${cands
        .map((l, i) => `<button class="music-cand" data-i="${i}">${esc(`${l.track || "?"} — ${l.artist || "?"}`)}</button>`)
        .join("")}</div>`
    : "";

  if (lyricsLoading) {
    setText(title, t("music.lyrics"));
    setHtml(box, `${candHtml}<div class="music-lyric-empty">${t("music.lyrics_loading")}</div>`);
    return;
  }

  if (!lyrics || !lyrics.found) {
    setText(title, t("music.lyrics"));
    setHtml(box, `${candHtml}<div class="music-lyric-empty">${t("music.lyrics_none")}</div>`);
    return;
  }

  setText(title, t("music.lyrics_from", { track: lyrics.track || "?", artist: lyrics.artist || "?" }));

  if (lyrics.synced) {
    lrcLines = parseLrc(lyrics.synced);
    if (lrcLines.length > 0) {
      const lines = lrcLines
        .map((l, i) => `<div class="music-lrc-line" data-i="${i}">${esc(l.text)}</div>`)
        .join("");
      setHtml(box, `${candHtml}<div class="music-lrc-lines">${lines}</div>`);
      return;
    }
  }

  const plain = (lyrics.plain || "").trim();
  if (plain) {
    setHtml(box, `${candHtml}<div class="music-lrc-plain">${esc(plain)}</div>`);
    return;
  }
  if (lyrics.instrumental) {
    setHtml(box, `${candHtml}<div class="music-lyric-empty">${t("music.lyrics_instrumental")}</div>`);
    return;
  }
  setHtml(box, `${candHtml}<div class="music-lyric-empty">${t("music.lyrics_none")}</div>`);
}

/** 同步行高亮：只在「当前行变了」时动 DOM，否则每 1s 重算会打断用户手动滚动。 */
function syncActiveLine(root: HTMLElement) {
  if (lrcLines.length === 0) return;
  const pos = (player?.progress_ms ?? 0) / 1000;
  let idx = -1;
  for (let i = 0; i < lrcLines.length; i++) {
    if (lrcLines[i].at <= pos) idx = i;
    else break;
  }
  if (idx === activeLine) return;
  const box = root.querySelector<HTMLElement>("#music-lyrics");
  if (!box) return;
  box.querySelectorAll<HTMLElement>(".music-lrc-line.active").forEach(el => el.classList.remove("active"));
  const next = box.querySelector<HTMLElement>(`.music-lrc-line[data-i="${idx}"]`);
  if (next) {
    next.classList.add("active");
    // 用**矩形差**算该行在歌词盒内容坐标里的位置，不用 `offsetTop`：后者相对的是
    // 最近的定位祖先（`offsetParent` 可能是 `#results-list` 或 body），一旦歌词外层
    // 多包一层就会算错。`scrollIntoView` 也不行 —— 它会把**整个插件面板**一起滚。
    const boxRect = box.getBoundingClientRect();
    const lineRect = next.getBoundingClientRect();
    const lineTop = (lineRect.top - boxRect.top) + box.scrollTop;
    const target = lineTop - box.clientHeight / 2 + lineRect.height / 2;
    box.scrollTo({ top: Math.max(0, target), behavior: "smooth" });
  } else if (box.scrollTop !== 0) {
    box.scrollTo({ top: 0, behavior: "smooth" });
  }
  activeLine = idx;
}

// ── 数据流 ────────────────────────────────────────────────────────

async function refreshConfig(root: HTMLElement) {
  try {
    cfg = await invoke<MusicConfigDto>("music_config_get");
  } catch (e) {
    setText(root.querySelector("#music-msg"), errText(e));
    return;
  }
  renderSetup(root);
  renderStatus(root);
}

/** 抓当前曲目的歌词；同一首只抓一次（除非用户手动搜过）。 */
async function fetchLyricsFor(track: TrackDto) {
  if (manualPick) return;
  if (lyricsFor === track.id) return;
  lyricsFor = track.id;
  lyricsLoading = true;
  lyrics = null;
  cands = [];          // 换歌 ⇒ 上一次手动搜索的候选不再适用
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
    if (root) renderLyrics(root);
  }
}

async function tick(root: HTMLElement, myGen: number) {
  if (myGen !== gen) return;
  // 关面板 / 切插件后 root 会脱离文档 ⇒ 自停（不必依赖外部调用清表）
  if (!root.isConnected) { stopPolling(); return; }

  if (cfg?.connected) {
    // 窗口隐藏时不打网络（用户看不见，白耗流量和 Spotify 配额），但表继续走。
    if (!document.hidden && !pollBusy) {
      pollBusy = true;
      try {
        player = await invoke<PlayerDto>("spotify_status");
        if (myGen !== gen) return;
        renderStatus(root);
        renderPlayer(root);
        syncActiveLine(root);
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
    if (player?.track) {
      // 换歌 ⇒ 手动选的歌词不再适用，回到「按当前曲目自动抓」
      if (manualPick && player.track.id !== lyricsFor) {
        manualPick = false;
        cands = [];
      }
      if (!manualPick) void fetchLyricsFor(player.track);
    }
  }
  if (myGen !== gen) return;
  pollTimer = window.setTimeout(() => void tick(root, myGen), 1000);
}

function stopPolling() {
  if (pollTimer !== undefined) {
    window.clearTimeout(pollTimer);
    pollTimer = undefined;
  }
  gen++;          // 让在飞的回调失效
  pollBusy = false;
}

function startPolling(root: HTMLElement) {
  stopPolling();
  const myGen = gen;
  pollTimer = window.setTimeout(() => void tick(root, myGen), 200);
}

// ── 交互 ──────────────────────────────────────────────────────────

export async function attachMusicListeners(root: HTMLElement) {
  const view = root.querySelector<HTMLElement>(".music-root");
  if (!view) return;
  currentRoot = view;

  const msg = (s: string) => setText(view.querySelector("#music-msg"), s);
  const q = <T extends HTMLElement>(sel: string) => view.querySelector<T>(sel);

  // 授权窗口回调：宿主换完令牌会 emit 一次
  if (unlistenAuth) { unlistenAuth(); unlistenAuth = null; }
  unlistenAuth = await listen<{ ok: boolean; message: string }>("spotify-auth", (ev) => {
    if (!view.isConnected) return;
    if (ev.payload.ok) {
      msg(t("music.auth_ok"));
      void refreshConfig(view).then(() => startPolling(view));
    } else {
      msg(ev.payload.message || t("music.auth_failed"));
    }
  });

  q<HTMLElement>("#music-setup-toggle")?.addEventListener("click", () => {
    const box = q<HTMLElement>("#music-setup");
    if (box) box.classList.toggle("hidden");
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
      cfg = await invoke<MusicConfigDto>("music_config_set", { clientId: id, port });
      msg(t("music.saved"));
      renderSetup(view);
      renderStatus(view);
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
        cfg = await invoke<MusicConfigDto>("music_config_set", { clientId: id, port });
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
      manualPick = false;
      await refreshConfig(view);
      msg(t("music.disconnected"));
    } catch (e) {
      msg(errText(e));
    }
  });

  const control = async (action: string, value?: number) => {
    try {
      await invoke("spotify_control", { action, value });
    } catch (e) {
      msg(errText(e));
    }
  };
  q<HTMLElement>("#music-prev")?.addEventListener("click", () => void control("previous"));
  q<HTMLElement>("#music-next")?.addEventListener("click", () => void control("next"));
  q<HTMLElement>("#music-play")?.addEventListener("click", () => {
    void control(player?.playing ? "pause" : "play");
  });
  q<HTMLInputElement>("#music-vol")?.addEventListener("change", (ev) => {
    const v = Number((ev.target as HTMLInputElement).value);
    void control("volume", v);
  });

  // 手动搜歌词：命中第一条即显示；同时列出候选供切换
  const doSearch = async () => {
    const text = q<HTMLInputElement>("#music-lyrics-q")?.value.trim() ?? "";
    if (!text) return;
    lyricsLoading = true;
    renderLyrics(view);
    try {
      const list = await invoke<LyricsDto[]>("lyrics_search", { q: text });
      lyricsLoading = false;
      if (!list.length) {
        lyrics = null;
        cands = [];
        manualPick = true;
        msg(t("music.lyrics_none"));
        renderLyrics(view);
        return;
      }
      manualPick = true;
      // 默认选中**第一条带时间轴**的：LRCLIB 的搜索结果里常混进「歌名+歌手」当
      // 曲名的搬运条目（实测搜 Creep 时第一条就是 `radiohead  creep`），只有 plain；
      // 直接取 list[0] 会让用户以为「这歌没歌词」。
      lyrics = list.find(x => x.synced) || list[0];
      cands = list;
      renderLyrics(view);
    } catch (e) {
      lyricsLoading = false;
      renderLyrics(view);
      msg(errText(e));
    }
  };
  q<HTMLElement>("#music-lyrics-search")?.addEventListener("click", () => void doSearch());
  q<HTMLInputElement>("#music-lyrics-q")?.addEventListener("keydown", (ev) => {
    if (ev.key === "Enter") { ev.preventDefault(); void doSearch(); }
  });

  // 初始加载
  bindLyricsDelegation(view);
  await refreshConfig(view);
  renderPlayer(view);
  renderLyrics(view);
  renderStatus(view);
  startPolling(view);
}

/** 候选列表（最多 8 条）的点击用**委托**挂在歌词盒上 —— 歌词区每次重建都会换掉
 *  `.music-cand` 节点，逐个 addEventListener 会在重渲染后变成孤儿监听（或重复叠加）。 */
function bindLyricsDelegation(root: HTMLElement) {
  const box = root.querySelector<HTMLElement>("#music-lyrics");
  if (!box) return;
  box.addEventListener("click", (ev) => {
    const btn = (ev.target as HTMLElement).closest<HTMLElement>(".music-cand");
    if (!btn) return;
    const idx = Number(btn.dataset.i || "-1");
    const pick = cands[idx];
    if (!pick) return;
    lyrics = pick;
    manualPick = true;
    renderLyrics(root);
  });
}

/** 供 main.ts 的 closePluginView 调用（显式停表；另有 `isConnected` 自停兜底）。 */
export function stopMusicPolling() {
  stopPolling();
  if (unlistenAuth) { unlistenAuth(); unlistenAuth = null; }
}

// 挂到 window 给 main.ts 调（本模块是**懒加载**的：只有打开过音乐插件才存在这个钩子，
// main.ts 那边用 `?.()` 调用，不会因未加载而报错）。
(window as any).__lunac_music_stop = stopMusicPolling;

// ── Plugin 定义 ───────────────────────────────────────────────────

export const musicPlugin: Plugin = {
  id: "music",
  name: "音乐歌词",
  keywords: ["音乐", "歌词", "歌曲", "正在播放", "music", "lyrics", "spotify", "播放控制", "暂停", "下一首"],
  description: "歌词抓取 (LRCLIB) + Spotify 播放控制 (Spotify Playback + Lyrics)",
  icon: "🎵",
  badge: "music",

  async execute(): Promise<PluginResult> {
    return { type: "html", content: shellHtml() };
  },
};
