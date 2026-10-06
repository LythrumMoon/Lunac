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

/** 宿主暴露给磁盘插件的能力。**只加不减** —— 插件列表里有旧版本，删字段等于把老插件打哑。 */
export interface LunacHostApi {
  /** 宿主 i18n 的 `t()`（与内置插件用的是同一个语言状态） */
  t: (key: string, params?: Record<string, string>) => string;
  /** 桥的协议版本。宿主加能力时递增，插件可据此做兼容分支（当前 = 2） */
  apiVersion: number;
  /** 插件自带本机进程（2026-10-06 加；`apiVersion >= 2` 才有）。 */
  sidecar?: LunacSidecarApi;
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
