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

/** 宿主暴露给磁盘插件的能力。**只加不减** —— 插件列表里有旧版本，删字段等于把老插件打哑。 */
export interface LunacHostApi {
  /** 宿主 i18n 的 `t()`（与内置插件用的是同一个语言状态） */
  t: (key: string, params?: Record<string, string>) => string;
  /** 桥的协议版本。宿主加能力时递增，插件可据此做兼容分支（当前 = 1） */
  apiVersion: number;
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
