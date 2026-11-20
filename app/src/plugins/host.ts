// ── 磁盘插件的「宿主桥」（2026-09-28）─────────────────────────────
//
// 为什么需要它：内置插件是**编译进主 bundle 的 TS 模块**，可以直接 `import { t } from "../i18n.js"`
// —— 它们与宿主共享同一个模块实例。而 `<exe 根>\Modules\<id>\` 下的**磁盘插件**是
// 一份独立打包的 ESM（经 asset 协议 `import()`），它 import 到的 i18n 会是**另一个实例**：
// 语言没初始化过（`initI18n()` 只在宿主里调过），于是 `t()` 全走 key 兜底、界面上一片 `music.play`。
//
// 所以磁盘插件不打包 i18n，而是**通过宿主桥拿宿主的 `t`**：
//   · 宿主（main.ts / plugin-window.ts）启动时调 `installHostBridge(...)` 把能力挂到
//     `globalThis.__lunac_host`；
//   · 插件 `import { t } from ".../host.js"`，这个模块在**主 bundle 里**就是宿主自己的实现、
//     在**插件 bundle 里**就读那个全局对象 —— 同一份源码，两种构建都对。
//
// **API 面越小越稳**：这是宿主与插件之间的长期契约，每加一项都要考虑「插件用旧版宿主怎么办」。
// 现在只有 `t` 与 `apiVersion`（插件可据此判断要不要用新能力）。

/** sidecar 启动结果（信任卡已在桥内部处理完，插件只会看到 `started`）。 */
export interface LunacSidecarStartResult {
  status: "started";
  /** http 通道的端口（stdio-only 时为 undefined）；插件**不要**自己 fetch 它 —— 见 `http()` */
  port?: number;
}

/** 宿主代请求的选项（端口由宿主去连，插件 JS 从不碰 socket —— CSP）。 */
export interface LunacSidecarHttpOptions {
  method?: string;
  /** 必须是「以 / 开头的相对路径」，如 `/v1/models` */
  path: string;
  body?: string;
  /** 省略 = 该插件自己 sidecar 的端口；给了就必须在授权范围内（见清单 `allowLocalPorts`） */
  port?: number;
}

/**
 * **sidecar**（2026-10-06，L12 档 1）：插件自带的本机进程。
 *
 * 起进程 / 收进程 / 通信全由**宿主**代管（清单里的 `sidecar` 段 + `permissions: ["process.spawn"]`）。
 * 首次启动会弹一张**信任卡**（如实列出 command / args / sha256 / 授权端口）—— 用户批准后才真的起。
 * `apiVersion >= 2` 才有这个字段（旧宿主上没有，务必 `?.` 兜底）。
 */
export interface LunacSidecarApi {
  /** 起进程（未信任时桥内部会先弹信任卡；用户拒绝 ⇒ reject）。 */
  start(): Promise<LunacSidecarStartResult>;
  /** 走 stdio 发一条 NDJSON 请求并等回包。 */
  request(method: string, params?: unknown): Promise<unknown>;
  /** 由**宿主**代请求 `127.0.0.1:<port>`（插件自己 fetch 会被 CSP 拦掉）。 */
  http(opts: LunacSidecarHttpOptions): Promise<{ status: number; body: string }>;
  /** 订阅 sidecar 主动推（`event` = 推送行里的 `method`）；返回退订函数。 */
  on(event: string, cb: (payload: unknown) => void): () => void;
  /** 收进程（面板关闭时宿主也会自动收）。 */
  stop(): Promise<boolean>;
}

/** 宿主代请求（2026-10-18，apiVersion >= 3）。插件自己 fetch 会被 CSP 拦掉。 */
export interface LunacNetRequest {
  url: string;
  method?: "GET" | "POST" | "PUT" | "DELETE" | "HEAD";
  headers?: Record<string, string>;
  body?: string;
  timeoutMs?: number;
  maxBytes?: number;
}

export interface LunacNetResponse {
  url: string;
  status: number;
  headers: Record<string, string>;
  body: string;
}

export interface LunacNetApi {
  /** 清单必须声明 `permissions: ["net.fetch"]`，否则宿主回 E_DENIED。 */
  request(req: LunacNetRequest): Promise<LunacNetResponse>;
}

export interface LunacPluginStorage {
  get<T = unknown>(key: string): Promise<T | undefined>;
  set(key: string, value: unknown): Promise<void>;
  remove(key: string): Promise<void>;
  keys(): Promise<string[]>;
}

export interface LunacMediaOptions {
  /** 防盗链要求的 Referer（缺省不发送）。 */
  referer?: string;
  /** 自定义 User-Agent（缺省由宿主发 `Lunac-MediaProxy/1.0`）。 */
  userAgent?: string;
  /** 需要登录态的站点：原样转发为 `Cookie`（2026-10-29 加）。 */
  cookie?: string;
}

export interface LunacMediaApi {
  /** 需要 Referer / UA 的媒体地址；返回可直接给 `<img>` / `<video>` 的 URL。
   *
   *  宿主用 `lunac-media://` 代理并原样转发 `Range`（视频拖动可用）。
   *  清单必须声明 `permissions: ["media.proxy"]`，否则宿主回 403。
   *  只接受 http(s) 绝对地址；非 http(s) 原样返回。 */
  proxy(url: string, options?: LunacMediaOptions): string;
}

export interface LunacEngineApi {
  /** 宿主 QuickJS 沙箱执行来源脚本：无文件 / 网络 / 进程绑定；
   *  清单必须声明 `permissions: ["engine.eval"]`。超时 / 内存上限由宿主兜底。 */
  eval(code: string, options?: { timeoutMs?: number; args?: unknown }): Promise<unknown>;
}

export interface LunacSniffApi {
  /** 打开一个隐藏 WebView 访问剧集页，拦截 m3u8 / mp4 请求并返回地址。
   *  清单必须声明 `permissions: ["webview.sniff"]`，否则宿主回 E_DENIED。
   *  找不到媒体地址时抛错（含等待时长）。 */
  media(url: string, options?: { timeoutMs?: number }): Promise<string[]>;
}

/** 拼音编码（2026-10-22 加；`apiVersion = 3` 的增补字段，无权限门）。
 *
 *  阅读插件的「拼音检索」用：插件在 iframe 里没有中文转拼音的能力，宿主用既有
 *  `pinyin` crate 批量编码。**只读、无副作用**；单条截断 512 字符、每次最多 2000 条。 */
export interface LunacPinyinEntry {
  /** 全拼（中文连写、ASCII 小写、词间最多一个空格）。 */
  full: string;
  /** 首字母（只含汉字音节首字母；没有汉字时为空串）。 */
  initials: string;
}

export interface LunacPinyinApi {
  /** 输出与输入逐条对齐（第 i 条结果对应第 i 条文本）。 */
  encode(texts: string[]): Promise<LunacPinyinEntry[]>;
}

/** 宿主暴露给磁盘插件的能力。**只加不减** —— 插件列表里有旧版本，删字段等于把老插件打哑。 */
export interface LunacHostApi {
  /** 宿主 i18n 的 `t()`（与内置插件用的是同一个语言状态） */
  t: (key: string, params?: Record<string, string>) => string;
  /** 桥的协议版本。宿主加能力时递增，插件可据此做兼容分支（当前 = 3） */
  apiVersion: number;
  /** 插件自带本机进程（2026-10-06 加；`apiVersion >= 2` 才有）。 */
  sidecar?: LunacSidecarApi;
  /** 宿主代请求（2026-10-18 加；`apiVersion >= 3` 才有）。 */
  net?: LunacNetApi;
  /** 插件 KV（2026-10-18 加；`apiVersion >= 3` 才有）。 */
  storage?: LunacPluginStorage;
  /** 媒体代理（2026-10-18 加；`lunac-media`，需 `permissions: ["media.proxy"]`）。 */
  media?: LunacMediaApi;
  /** 脚本沙箱（2026-10-18 加；`apiVersion >= 3`，需 `permissions: ["engine.eval"]`）。 */
  engine?: LunacEngineApi;
  /** WebView 嗅探（2026-10-21 加；`apiVersion = 3` 的增补字段，需 `permissions: ["webview.sniff"]`）。 */
  sniff?: LunacSniffApi;
  /** 拼音编码（2026-10-22 加；`apiVersion = 3` 的增补字段，只读、无权限门）。 */
  pinyin?: LunacPinyinApi;
}

const HOST_KEY = "__lunac_host";

/** 由宿主在启动时调用一次（幂等）。插件侧永不调用。 */
export function installHostBridge(api: LunacHostApi): void {
  (globalThis as Record<string, unknown>)[HOST_KEY] = api;
}

/** 取宿主桥；不在宿主里（例如单测 / 静态分析）返回 null。 */
export function getHost(): LunacHostApi | null {
  const h = (globalThis as Record<string, unknown>)[HOST_KEY] as LunacHostApi | undefined;
  return h ?? null;
}

/** 翻译。**桥不可用时退回 key 本身**（显示 `music.play` 也比抛异常把整个插件打哑好）。 */
export function t(key: string, params?: Record<string, string>): string {
  const h = getHost();
  if (h && typeof h.t === "function") return h.t(key, params);
  return key;
}
