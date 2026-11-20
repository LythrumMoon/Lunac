import Hls from "hls.js";
import type {
  HlsConfig,
  Loader,
  LoaderCallbacks,
  LoaderConfiguration,
  LoaderContext,
  LoaderStats,
} from "hls.js";
import type { Playback } from "@resource-hub/source-sdk";

// ── 视频播放器（B7，2026-10-18）────────────────────────────────────
//
// HLS 走 hls.js（WebView2 没有原生 HLS）；MP4 直接交给 <video>。
// 线路切换、字幕、进度回写都由本组件完成，宿主侧只保存进度。
//
// **防盗链 / CSP**：所有 http(s) 请求都经 `resolveUrl` 交给宿主的
// `lunac-media` 代理。HLS 的分片、子清单、密钥、字幕都是相对地址，
// 直接交给 hls.js 会绕过代理（而且被 CSP `connect-src` 拦），所以这里
// 自定义 loader：请求前把地址代理化，清单返回后把其中的 URI 全部改写成
// **绝对原始地址**，下一跳再由 loader 代理 —— hls.js 的相对解析仍然正常。

export interface PlayerLabels {
  exit: string;
  line: string;
  loading: string;
  error: string;
  retry: string;
  unsupported: string;
  noSource: string;
  /** 容器 / 编码解不了（MKV 里的 HEVC、HLS 里的 hvc1 等）。 */
  codecUnsupported: string;
  copyLink: string;
  copied: string;
}

export interface PlayerOptions {
  title: string;
  playbacks: Playback[];
  initialPosition?: number;
  labels: PlayerLabels;
  /** 把原始 http(s) 地址换成宿主代理地址；缺省原样返回。 */
  resolveUrl?(url: string): string;
  onProgress?(position: number, duration: number): void;
  onExit(): void;
}

export interface PlayerHandle {
  destroy(): void;
  /** 播放器 UI 是否还挂在文档里。
   *
   *  宿主**整段替换结果区**（重开插件 › `plugin-window-input` › 再次 render）时，
   *  插件的实例还活着，但播放器那棵 DOM 已经不在文档里了 —— 这时必须当作
   *  「已卸载」处理（见 app.ts 的 `paintNow` 与下面的 `onTimeUpdate` 看门狗）。 */
  isAttached(): boolean;
}

const ICONS: Record<string, string> = {
  exit: '<path d="M15 18l-6-6 6-6"/><path d="M9 12h12"/>',
  refresh: '<path d="M20 11a8 8 0 1 0-2.3 5.7"/><path d="M20 5v6h-6"/>',
  copy: '<rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V6a2 2 0 0 1 2-2h9"/>',
};

function icon(name: string): string {
  return `<svg class="rh-player-icon" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${ICONS[name] ?? ""}</svg>`;
}

function esc(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

const STYLE = `
.rh-player{position:absolute;inset:0;display:flex;flex-direction:column;background:#000;color:var(--text);font:inherit}
.rh-player-bar{display:flex;align-items:center;gap:10px;padding:calc(8px + env(safe-area-inset-top)) 14px 8px;background:rgba(var(--surface-rgb,22,24,27),.92);border-bottom:1px solid var(--border-glass);flex:none}
.rh-player-title{flex:1;min-width:0;text-align:center;font-size:.88rem;color:var(--text-dim);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.rh-player-btn{display:inline-flex;align-items:center;gap:6px;border:1px solid transparent;background:transparent;color:var(--text-dim);border-radius:6px;padding:6px 8px;cursor:pointer;font:inherit;font-size:.82rem;white-space:nowrap}
.rh-player-btn:hover{background:rgba(var(--accent-rgb),.08);color:var(--text)}
.rh-player-btn.primary{border-color:rgba(var(--accent-rgb),.42);background:rgba(var(--accent-rgb),.12);color:var(--text)}
.rh-player-icon{flex:none}
.rh-player-stage{position:relative;flex:1;min-height:0;background:#000;display:flex;align-items:center;justify-content:center}
.rh-player-video{width:100%;height:100%;background:#000;display:block}
.rh-player-message{position:absolute;inset:0;display:none;align-items:center;justify-content:center;padding:24px;text-align:center;color:var(--text-dim);background:rgba(var(--shade-rgb,0,0,0),.55);font-size:.88rem;line-height:1.6}
.rh-player-message.is-visible{display:flex}
.rh-player-retry{position:absolute;left:50%;bottom:18%;transform:translateX(-50%);display:none}
.rh-player-retry.is-visible{display:block}
.rh-player-copy{position:absolute;left:50%;bottom:14%;transform:translateX(-50%);display:none;gap:8px;align-items:center}
.rh-player-copy.is-visible{display:flex}
.rh-player-lines{display:flex;gap:6px;padding:8px 14px calc(8px + env(safe-area-inset-bottom));background:rgba(var(--surface-rgb,22,24,27),.92);border-top:1px solid var(--border-glass);flex:none;overflow-x:auto}
.rh-player-lines.is-hidden{display:none}
.rh-player-line{flex:none;border:1px solid var(--border-glass);background:transparent;color:var(--text-dim);border-radius:999px;padding:4px 10px;font:inherit;font-size:.78rem;cursor:pointer}
.rh-player-line:hover{background:rgba(var(--accent-rgb),.06);color:var(--text)}
.rh-player-line.is-active{border-color:rgba(var(--accent-rgb),.42);background:rgba(var(--accent-rgb),.12);color:var(--text)}
`;

// ── HLS 代理 loader ───────────────────────────────────────────────
//
// 宿主代理只认识「一个 http(s) 地址」。清单里出现的是相对 URI（分片、子
// 清单、密钥），hls.js 默认会相对**代理后的地址**解析，拼出来的 URL 是
// 错的。所以：清单下载完成后，把里面的 URI 改写成绝对**原始**地址；
// 每个请求仍由 loader 交给 `resolveUrl` 代理。

type ResolveUrl = (url: string) => string;

function absoluteHttpUrl(value: string, base: string): string | null {
  try {
    const url = new URL(value, base);
    return url.protocol === "http:" || url.protocol === "https:" ? url.toString() : null;
  } catch {
    return null;
  }
}

function isPlaylist(type: LoaderContext["type"]): boolean {
  return (
    type === "manifest"
    || type === "level"
    || type === "audioTrack"
    || type === "subtitleTrack"
    || type === "steering-manifest"
    || type === "interstitial-asset-list"
  );
}

function rewritePlaylist(text: string, baseUrl: string): string {
  return text
    .split(/\r?\n/)
    .map((line) => {
      const trimmed = line.trim();
      if (!trimmed) return line;
      // 裸 URI 行（分片地址、#EXT-X-STREAM-INF 的下一行）
      if (!trimmed.startsWith("#")) {
        return absoluteHttpUrl(trimmed, baseUrl) ?? line;
      }
      // 标签属性里的 URI="..."（#EXT-X-KEY / #EXT-X-MAP / #EXT-X-MEDIA ...）
      if (!trimmed.includes('URI="')) return line;
      return line.replace(/URI="([^"]*)"/g, (match, value: string) => {
        const absolute = absoluteHttpUrl(value, baseUrl);
        return absolute ? `URI="${absolute}"` : match;
      });
    })
    .join("\n");
}

function createProxyLoader(resolve: ResolveUrl) {
  const Base = Hls.DefaultConfig.loader;
  return class LunacProxyLoader implements Loader<LoaderContext> {
    private inner: Loader<LoaderContext>;
    constructor(config: HlsConfig) {
      this.inner = new Base(config);
    }
    get context(): LoaderContext | null {
      return this.inner.context;
    }
    set context(value: LoaderContext | null) {
      this.inner.context = value;
    }
    get stats(): LoaderStats {
      return this.inner.stats;
    }
    set stats(value: LoaderStats) {
      this.inner.stats = value;
    }
    destroy(): void {
      this.inner.destroy();
    }
    abort(): void {
      this.inner.abort();
    }
    getCacheAge(): number | null {
      return this.inner.getCacheAge?.() ?? null;
    }
    getResponseHeader(name: string): string | null {
      return this.inner.getResponseHeader?.(name) ?? null;
    }
    load(
      context: LoaderContext,
      config: LoaderConfiguration,
      callbacks: LoaderCallbacks<LoaderContext>,
    ): void {
      const original = context.url;
      const wrapped: LoaderCallbacks<LoaderContext> = {
        ...callbacks,
        onSuccess: (response, stats, ctx, networkDetails) => {
          let data = response.data;
          if (typeof data === "string" && isPlaylist(ctx.type)) {
            data = rewritePlaylist(data, original);
          }
          // 把 URL 还原成原始地址：hls.js 后续的相对解析与片段记录都基于它。
          callbacks.onSuccess({ ...response, url: original, data }, stats, ctx, networkDetails);
        },
      };
      this.inner.load({ ...context, url: resolve(original) }, config, wrapped);
    }
  };
}

/** 所有还活着的 `<video>`（模块级登记）。
 *
 *  为什么要有它：`paint()` 会整树重建，只要有一条路径忘了 `destroy()`，旧 `<video>` 就会
 *  **脱离 DOM 但继续解码** —— 浏览器不会回收仍在播的媒体元素。2026-10-29 用 CDP `Media` 域
 *  实测到过：页面里 `document.querySelectorAll("video").length === 0`，浏览器里仍有 media player
 *  带 `D3D11VideoDecoder` 在跑，用户听到的就是「退出后还在响」。
 *  有了登记表，重绘 / 关窗 / 隐藏时可以无差别兜底停掉。 */
const liveVideos = new Set<HTMLVideoElement>();

/** 把一个 `<video>` 拉回空状态（幂等；元素已脱离文档也不会抛）。 */
function stopElement(video: HTMLVideoElement): void {
  try {
    video.pause();
  } catch {
    // 元素已销毁：忽略
  }
  try {
    video.srcObject = null;
  } catch {
    // 个别 WebView 只读：忽略
  }
  video.removeAttribute("src");
  try {
    video.load();
  } catch {
    // 同上
  }
}

/** 停掉**所有**登记过的播放器。宿主拆 DOM / 收面板 / 关窗前调用（没有播放器时是空操作）。 */
export function stopAllPlayers(): void {
  for (const video of liveVideos) stopElement(video);
  liveVideos.clear();
}

export function mountPlayer(root: HTMLElement, options: PlayerOptions): PlayerHandle {
  // 同一时刻只允许一个播放器。任何一条漏了 `destroy()` 的路径都会让旧 `<video>` 继续出声，
  // 听感就是「同一段声音隔半秒又响一次」（2026-10-31 报障：每条线路都会）。
  // 正常路径由 `paintNow()` 先停；这里兜底，代价是一次空循环。
  if (liveVideos.size > 0) stopAllPlayers();
  const playbacks = options.playbacks.filter((playback) => playback.url.trim() !== "");
  const resolve: ResolveUrl = (url) => options.resolveUrl?.(url) ?? url;
  root.innerHTML = `<style>${STYLE}</style>
<div class="rh-player">
  <header class="rh-player-bar">
    <button class="rh-player-btn" data-action="exit">${icon("exit")}<span>${esc(options.labels.exit)}</span></button>
    <span class="rh-player-title">${esc(options.title)}</span>
  </header>
  <div class="rh-player-stage">
    <video class="rh-player-video" controls playsinline preload="metadata"></video>
    <div class="rh-player-message" data-role="message"></div>
    <div class="rh-player-retry" data-role="retry">
      <button class="rh-player-btn primary" data-action="retry">${icon("refresh")}<span>${esc(options.labels.retry)}</span></button>
    </div>
    <div class="rh-player-copy" data-role="copy">
      <button class="rh-player-btn primary" data-action="copy">${icon("copy")}<span>${esc(options.labels.copyLink)}</span></button>
    </div>
  </div>
  <footer class="rh-player-lines is-hidden" data-role="lines"></footer>
</div>`;

  const requireEl = <T extends Element>(selector: string): T => {
    const el = root.querySelector<T>(selector);
    if (!el) throw new Error("Player UI 初始化失败");
    return el;
  };
  const video = requireEl<HTMLVideoElement>(".rh-player-video");
  liveVideos.add(video);
  const message = requireEl<HTMLElement>('[data-role="message"]');
  const retry = requireEl<HTMLElement>('[data-role="retry"]');
  const copyBox = requireEl<HTMLElement>('[data-role="copy"]');
  const lines = requireEl<HTMLElement>('[data-role="lines"]');

  let hls: Hls | null = null;
  let currentIndex = -1;
  let lastSavedAt = 0;
  let destroyed = false;

  function destroyHls(): void {
    if (!hls) return;
    try {
      hls.destroy();
    } catch {
      // 已销毁 / 上游异常：忽略
    }
    hls = null;
  }

  function setMessage(text: string, canRetry: boolean): void {
    message.textContent = text;
    message.classList.toggle("is-visible", text !== "");
    retry.classList.toggle("is-visible", canRetry);
  }

  function renderLines(): void {
    if (playbacks.length <= 1) {
      lines.innerHTML = "";
      lines.classList.add("is-hidden");
      return;
    }
    lines.classList.remove("is-hidden");
    lines.innerHTML = playbacks
      .map((playback, index) => `<button class="rh-player-line${index === currentIndex ? " is-active" : ""}" data-line="${index}">${esc(options.labels.line)} ${index + 1} · ${esc(playback.type.toUpperCase())}</button>`)
      .join("");
  }

  function clearTracks(): void {
    for (const track of Array.from(video.querySelectorAll("track"))) track.remove();
  }

  /** 把 `<video>` 从「正在拉流」拉回空状态。
   *
   *  只 `pause()` 不够：MSE（hls.js）挂在元素上的 SourceBuffer 仍会继续请求分片，
   *  表现就是「关掉了还在跑流量 / 还有声音」。这里按顺序 暂停 → 解绑 src → 重新
   *  `load()` 中止在途请求 → 清字幕轨。 */
  function teardownVideo(): void {
    cancelFrameWatch();
    cancelFrameGate();
    clearTracks();
    stopElement(video);
  }

  function addTracks(playback: Playback): void {
    clearTracks();
    (playback.subtitles ?? []).forEach((subtitle, index) => {
      const track = document.createElement("track");
      track.kind = "subtitles";
      track.label = subtitle.label;
      track.srclang = subtitle.lang ?? `sub-${index + 1}`;
      track.src = resolve(subtitle.url);
      if (index === 0) track.default = true;
      video.appendChild(track);
    });
  }

  // ── 「有声音、没画面」看门狗（2026-10-29 报障）────────────────────
  //
  // 现象：某些线路的**视频轨编码**（HLS/MP4 里的 HEVC、AV1…）Chromium 解不了，但音频轨是
  // AAC ⇒ `<video>` 照常出声，`error` 永远不触发（`onVideoError` 兜不住），画面就一直停在
  // 浏览器自带的转圈 / 我们那句「正在加载视频…」上。
  // 判据：已经在播、没报错、却**一帧都没出来**（`videoWidth === 0` 且 `totalVideoFrames === 0`）。
  // 命中就复用「编码不受支持 + 复制直链」这条出口，让用户换线路而不是干等。
  const FRAME_WATCH_MS = 8_000;
  let frameWatch: ReturnType<typeof setTimeout> | null = null;

  function cancelFrameWatch(): void {
    if (frameWatch !== null) {
      clearTimeout(frameWatch);
      frameWatch = null;
    }
  }

  function armFrameWatch(): void {
    cancelFrameWatch();
    frameWatch = setTimeout(() => {
      frameWatch = null;
      if (destroyed || video.error) return;
      // 没真正在播（自动播放被拦 / 已暂停）时不下结论。
      if (video.paused || video.currentTime < 1.2) return;
      const quality = video.getVideoPlaybackQuality?.();
      if (video.videoWidth > 0 || (quality?.totalVideoFrames ?? 0) > 0) return;
      setMessage(options.labels.codecUnsupported, true);
      copyBox.classList.add("is-visible");
      // 慢源可能 8s 后才出首帧：再等 5s，真出画面就把提示撤掉（用户已点过「复制直链」则不动）。
      window.setTimeout(() => {
        if (destroyed || video.error) return;
        if (message.textContent !== options.labels.codecUnsupported) return;
        const q = video.getVideoPlaybackQuality?.();
        if (video.videoWidth > 0 || (q?.totalVideoFrames ?? 0) > 0) {
          setMessage("", false);
          copyBox.classList.remove("is-visible");
        }
      }, 5_000);
    }, FRAME_WATCH_MS);
  }

  /** `playing` 的两个职责：开播后才开始算「等画面」的兜底 8 秒；以及 8s 无帧看门狗。 */
  const onPlaying = (): void => {
    startGateTimer();
    armFrameWatch();
  };

  video.addEventListener("playing", onPlaying);

  // ── 音画同起（2026-10-30 首修 / 2026-10-31 加固）────────────────
  //
  // 现象：有的线路**音频先解出来**，视频轨还要等解码器初始化 + 首个关键帧，于是「先出声、
  // 1~3s 后才出画面」。用户侧听起来就是「声音在放，画面卡在正在加载」。
  // 做法：起播先静音；满足「**两帧**已呈现 + 有画面尺寸」才恢复成用户原来的静音状态。
  //
  // 为什么是两帧而不是一帧（2026-10-31 用户二次报障）：解码器起播时常先吐一帧**起始黑帧**
  // （第一个可显示的关键帧之前的占位），`requestVideoFrameCallback` 对它照常回调 ——
  // 一帧就放声 = 在黑屏时就开始响，用户看到的仍是「先出声后出画面」。
  // 为什么兜底 8s 从 `playing` 起算而不是 `load()`：慢源拉清单 / 首个分片可能就花掉 7–8s，
  // 从 load 起算的话闸门会在真正开播 1s 后就到期，等于没设。
  const FIRST_FRAME_GATE_MS = 8_000;
  /** 判定「画面真的出来了」所需的最小已呈现帧数（1 帧可能只是起始黑帧）。 */
  const FIRST_FRAME_MIN_FRAMES = 2;
  let firstFrameSeen = false;
  let framesSeen = 0;
  let gateTimer: ReturnType<typeof setTimeout> | null = null;
  let mutedBeforeGate = false;
  let frameProbe: number | null = null;

  function cancelFrameGate(): void {
    if (gateTimer !== null) {
      clearTimeout(gateTimer);
      gateTimer = null;
    }
    if (frameProbe !== null) {
      try {
        video.cancelVideoFrameCallback(frameProbe);
      } catch {
        // 引擎没有该 API：忽略
      }
      frameProbe = null;
    }
  }

  function releaseFirstFrame(): void {
    if (firstFrameSeen) return;
    firstFrameSeen = true;
    cancelFrameGate();
    video.muted = mutedBeforeGate;
    if (message.textContent === options.labels.loading) setMessage("", false);
  }

  /** 起播静音。**8s 兜底计时不在这里启动** —— 它挂在 `playing` 上（见 `startGateTimer`）。 */
  function armFirstFrameGate(): void {
    cancelFrameGate();
    firstFrameSeen = false;
    framesSeen = 0;
    mutedBeforeGate = video.muted;
    video.muted = true;
  }

  /** 真正开播后才开始算「等画面」的 8 秒（纯音频流 / 引擎不给帧回调时靠它放声）。 */
  function startGateTimer(): void {
    if (firstFrameSeen || gateTimer !== null) return;
    gateTimer = setTimeout(() => {
      gateTimer = null;
      releaseFirstFrame();
    }, FIRST_FRAME_GATE_MS);
  }

  /** 第一帧呈现的判据：`requestVideoFrameCallback` 最准；没有就退回 `loadeddata` + 宽高。 */
  function watchFirstFrame(): void {
    if (firstFrameSeen) return;
    if (typeof video.requestVideoFrameCallback !== "function") {
      const check = (): void => {
        if (video.videoWidth > 0) releaseFirstFrame();
      };
      video.addEventListener("loadeddata", check, { once: true });
      video.addEventListener("playing", check, { once: true });
      return;
    }
    // 每呈现一帧回调一次，所以这里自己数帧 —— 不用 metadata.presentedFrames：
    // 那是**元素生命周期内累计**的，换线路后一上来就 ≥ 阈值，等于第二帧规则失效。
    const probe = (): void => {
      frameProbe = null;
      if (firstFrameSeen || destroyed) return;
      framesSeen += 1;
      if (framesSeen >= FIRST_FRAME_MIN_FRAMES && video.videoWidth > 0) {
        releaseFirstFrame();
        return;
      }
      frameProbe = video.requestVideoFrameCallback(probe);
    };
    frameProbe = video.requestVideoFrameCallback(probe);
  }

  /** 复制直链：iframe 里 clipboard API 可能被权限拦，退回 execCommand。 */
  async function copyLink(text: string): Promise<boolean> {
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(text);
        return true;
      }
    } catch {
      // 权限被拒：走下面的一次性 textarea
    }
    try {
      const area = document.createElement("textarea");
      area.value = text;
      area.setAttribute("readonly", "");
      area.style.position = "fixed";
      area.style.opacity = "0";
      root.appendChild(area);
      area.select();
      const ok = document.execCommand("copy");
      area.remove();
      return ok;
    } catch {
      return false;
    }
  }

  function load(index: number, position: number): void {
    const playback = playbacks[index];
    if (!playback) {
      setMessage(options.labels.noSource, true);
      return;
    }
    currentIndex = index;
    renderLines();
    destroyHls();
    teardownVideo();
    copyBox.classList.remove("is-visible");
    setMessage(options.labels.loading, false);
    // 先静音起播，等第一帧呈现再恢复（见下方「音画同起」）。
    armFirstFrameGate();
    watchFirstFrame();
    addTracks(playback);

    const resume = (): void => {
      if (position > 0 && Number.isFinite(video.duration) && position < video.duration - 1) {
        try {
          video.currentTime = position;
        } catch {
          // 媒体元数据尚未就绪：保持从头播放
        }
      }
      // 「正在加载视频…」由第一帧到达时撤掉：元数据到位 ≠ 画面能出来。
    };
    video.addEventListener("loadedmetadata", resume, { once: true });

    if (playback.type === "mp4") {
      video.src = resolve(playback.url);
      void video.play().catch(() => {});
      return;
    }
    if (playback.type === "hls") {
      // **先 hls.js，后原生**：WebView2（Chromium 155）对
      // `application/vnd.apple.mpegurl` 回 `maybe`，但它其实没有 HLS 解复用器 ——
      // 走原生分支只会把 `video.src` 指向代理地址后永远停在 readyState 0
      // （2026-10-21 报障：CMS 采集源「无法加载视频本身」即此）。
      if (!Hls.isSupported()) {
        if (video.canPlayType("application/vnd.apple.mpegurl")) {
          video.src = resolve(playback.url);
          void video.play().catch(() => {});
          return;
        }
        setMessage(options.labels.unsupported, true);
        copyBox.classList.add("is-visible");
        return;
      }
      const instance = new Hls({
        enableWorker: false,
        loader: createProxyLoader(resolve),
        // 媒体必须过宿主代理（带 Referer / UA），链路上多一跳，
        // hls.js 默认 10s 的清单超时对采集站偏紧；放宽到 30s 并允许重试。
        manifestLoadingTimeOut: 30_000,
        levelLoadingTimeOut: 30_000,
        fragLoadingTimeOut: 30_000,
        manifestLoadingMaxRetry: 2,
        levelLoadingMaxRetry: 2,
        fragLoadingMaxRetry: 3,
      });
      hls = instance;
      instance.on(Hls.Events.ERROR, (_event, data) => {
        if (!data.fatal) return;
        setMessage(options.labels.error, true);
        // HEVC 的 HLS（hvc1）也会走到这里：给「复制直链」出口，用户可换本机播放器。
        copyBox.classList.add("is-visible");
        destroyHls();
      });
      instance.loadSource(playback.url);
      instance.attachMedia(video);
      instance.on(Hls.Events.MANIFEST_PARSED, () => {
        void video.play().catch(() => {});
      });
      return;
    }
    setMessage(options.labels.unsupported, true);
  }

  function saveProgress(force: boolean): void {
    const now = Date.now();
    if (!force && now - lastSavedAt < 5_000) return;
    lastSavedAt = now;
    if (Number.isFinite(video.duration)) {
      options.onProgress?.(video.currentTime, video.duration);
    }
  }

  function exit(): void {
    if (destroyed) return;
    saveProgress(true);
    options.onExit();
  }

  const onTimeUpdate = (): void => {
    // 看门狗（2026-10-31 报障：不管哪条退出路径，音频都会留在后台播）。
    // 元素的 `isConnected` 变 false = 宿主已经把播放器那棵 DOM 丢掉了（整段替换结果区 /
    // 重开插件 / 收面板），但 `<video>` 被 `liveVideos` 引用着，浏览器**不会**因此停声。
    // `timeupdate` 在播放中约 4Hz ⇒ 最迟 250ms 内自己拆掉，不赌宿主每条退出路径都
    // 记得调 `handle.destroy()`。
    if (!video.isConnected) {
      handle.destroy();
      return;
    }
    saveProgress(false);
  };
  const onEnded = (): void => {
    if (Number.isFinite(video.duration)) options.onProgress?.(video.duration, video.duration);
  };
  const onVideoError = (): void => {
    if (!video.getAttribute("src") && !hls) return;
    // 3 = DECODE_ERR、4 = SRC_NOT_SUPPORTED：MKV 里的 HEVC / AC3 等编码解不了。
    const code = video.error?.code;
    if (code === 3 || code === 4) {
      setMessage(options.labels.codecUnsupported, true);
      copyBox.classList.add("is-visible");
      return;
    }
    setMessage(options.labels.error, true);
  };
  const onClick = (event: MouseEvent): void => {
    const target = event.target as HTMLElement | null;
    const line = target?.closest<HTMLElement>("[data-line]");
    if (line) {
      const index = Number(line.dataset.line ?? "0");
      load(index, video.currentTime || 0);
      return;
    }
    const action = target?.closest<HTMLElement>("[data-action]")?.dataset.action;
    if (action === "exit") {
      exit();
      return;
    }
    if (action === "retry") {
      load(currentIndex, video.currentTime || options.initialPosition || 0);
      return;
    }
    if (action === "copy") {
      const playback = playbacks[currentIndex];
      if (!playback) return;
      void copyLink(playback.url).then((ok) => {
        if (destroyed) return;
        setMessage(ok ? options.labels.copied : options.labels.error, true);
      });
    }
  };

  const onPageHide = (): void => {
    // 宿主移除 iframe / 独立窗关闭会走到这里：同步拆掉，别指望 GC 回收在途拉流。
    handle.destroy();
  };
  const onVisibility = (): void => {
    // 窗口被最小化 / 切到后台时先停流（不销毁 UI，回来还能继续播）。
    if (document.visibilityState === "hidden") teardownVideo();
  };

  const handle: PlayerHandle = {
    isAttached(): boolean {
      return video.isConnected;
    },
    destroy(): void {
      if (destroyed) return;
      destroyed = true;
      // 顺序是「先停元素、再拆 hls」：反过来时 MSE 仍可能在拉最后几个分片。
      teardownVideo();
      destroyHls();
      liveVideos.delete(video);
      video.removeEventListener("timeupdate", onTimeUpdate);
      video.removeEventListener("ended", onEnded);
      video.removeEventListener("error", onVideoError);
      video.removeEventListener("playing", onPlaying);
      root.removeEventListener("click", onClick);
      window.removeEventListener("pagehide", onPageHide);
      document.removeEventListener("visibilitychange", onVisibility);
      root.innerHTML = "";
    },
  };

  video.addEventListener("timeupdate", onTimeUpdate);
  video.addEventListener("ended", onEnded);
  video.addEventListener("error", onVideoError);
  root.addEventListener("click", onClick);
  window.addEventListener("pagehide", onPageHide);
  document.addEventListener("visibilitychange", onVisibility);

  if (playbacks.length === 0) {
    setMessage(options.labels.noSource, true);
    renderLines();
  } else {
    load(0, options.initialPosition ?? 0);
  }

  return handle;
}
