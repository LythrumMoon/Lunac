// ── 第三方插件（市场）适配层（L1，2026-09-21）────────────────────
//
// 为什么需要这一层：**内置插件是编译进 bundle 的 TS 模块**（`builtin/index.ts` 里静态 import），
// 而第三方插件是**磁盘上的、已经编译好的 ESM**（`<exe 根>\plugins\<id>\index.js`）。
// 两者必须能被同一套东西消费 —— 结果区的渲染、拼音匹配、`pluginIconSvg()`、i18n 的
// `plugin.<id>` 全都只认 `Plugin` 接口，所以这里做的事就是「磁盘包 → 普通 Plugin 对象」。
//
// **加载通道**：CSP 是 `script-src 'self' 'unsafe-inline' https://asset.localhost`
// （见 `app/src-tauri/tauri.conf.json`），所以插件代码**只能经 asset 协议** import
// （`convertFileSrc` 把绝对路径变成 `https://asset.localhost/...`）——
// 不能走 CDN、也不能用 `file:`。这也是「插件必须自带依赖」的原因。
//
// **插件的模块契约**（写插件的人只需要记住这一条）：
//   入口默认导出 `{ execute(input) => string | { type, content } }`；
//   也接受「默认导出就是一个函数」或「具名导出 execute」两种写法。
//   入参是用户在搜索栏里输入的原始文本（与内置插件一致），返回值是文本或 HTML 结果块。
//
// **信任模型要说清楚**：插件是本机可执行代码，`type: 'html'` 的结果会被 `innerHTML` 渲染
// （与内置插件同路），所以「装了谁」等于「信任谁的代码」—— 界面上必须把来源与这一点写出来，
// 别让用户以为它只是个配置项。

import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { t } from "../i18n.js";
import { pluginRegistry, type Plugin, type PluginResult } from "./registry";
import { isBasePlugin } from "./kinds";

/** 与 Rust 侧 `plugin_market::InstalledPlugin` 一一对应（字段名由 serde 直接序列化，改动要两边一起改）。 */
export interface MarketPluginInfo {
  id: string;
  name: string;
  description: string;
  keywords: string[];
  icon: string;
  version: string;
  entry: string;
  homepage: string;
  dir: string;
  /** 入口文件绝对路径（Rust 侧已校验它在插件目录内） */
  entryPath: string;
  /** 清单与入口都通过校验 */
  valid: boolean;
  /** `valid === false` 时的原因（原样显示，不猜） */
  error: string;
  /** 这个插件声明的依赖（2026-09-28）。宿主在装插件时已顺带拉好；
   *  界面上只用来提示「它会拉什么」—— 装完即已就位。 */
  dependencies: MarketDependency[];
  /** 这个插件声明的宿主能力（2026-09-28），如 `layout.takeover`。
   *  **是「告知」不是沙箱**：插件是本机可执行代码，绕过桥直接 `invoke()` 照样能调宿主命令。
   *  界面上如实列出来，让用户知道装的东西要什么（见 ai-spec §3.5）。 */
  permissions: string[];
}

/** 与 Rust 侧 `plugin_market::PluginDependency` 一一对应（2026-09-28）。 */
export interface MarketDependency {
  /** `file`（缺省，下载到 dest）/ `npm`（走 npm install） */
  type: string;
  url: string;
  package: string;
  version: string;
  dest: string;
  sha256: string;
}

/** 市场索引里的一条（与 Rust 侧 `plugin_market::PluginIndexEntry` 一一对应）。
 *
 *  它**只描述「去哪下」**：`url` 是 https 的插件 zip 地址，其余字段只用于下载前的预览。
 *  装完之后一切以**包内清单**为准（`MarketPluginInfo`），索引说什么不再重要。 */
export interface MarketIndexEntry {
  id: string;
  name: string;
  description: string;
  version: string;
  url: string;
  keywords: string[];
  icon: string;
  homepage: string;
}

/** 拉市场索引（宿主去拉，见 `commands::fetch_plugin_index` 的两条理由）。
 *
 *  **失败不吞**：把原因原样带回去让面板显示出来，市场则退回「只有本机插件」那种形态
 *  （也就是这个功能不存在时的样子）。索引拉不到不是错误状态 —— 它只是一份推荐清单。 */
export async function fetchPluginIndex(): Promise<{ list: MarketIndexEntry[]; error: string }> {
  try {
    return { list: await invoke<MarketIndexEntry[]>("fetch_plugin_index"), error: "" };
  } catch (e) {
    return { list: [], error: String(e) };
  }
}

/** 最近一次扫描的结果（设置面板渲染「插件目录」那一段时直接读它）。 */
let installed: MarketPluginInfo[] = [];

/** 已 import 过的模块缓存：`execute` 是懒加载的，但同一个插件只 import 一次。
 *
 *  **带版本号**（2026-09-28）：升级装的是**同一个目录、同一个 entry 路径**，光看路径分不出
 *  新旧代码 —— 缓存会把用户按回旧版。所以记下装载时的 `version`，`refreshMarketPlugins()`
 *  发现版本变了就把它从缓存里摘掉（下次 `execute` 重新 import）。 */
const modules = new Map<string, { version: string; mod: unknown }>();

/** 磁盘插件的挂载钩子（模块里导出的 `attach` / `detach`）。
 *  **只有真加载过模块的插件才有**：没打开过的插件没有钩子要收，这是有意的。 */
const hooks = new Map<string, { detach?: () => void }>();

/** **本层注册过**的拓展插件 id（只增删于 `refreshMarketPlugins`，非本层注册的绝不动）。
 *
 *  为什么需要单独记一份：`refreshMarketPlugins` 拿到的是**当前**扫描结果，
 *  「卸载后」那个 id 已经**不在**结果里了 —— 光遍历结果去 `unregister` 永远摘不掉它，
 *  于是卸载完插件还留在 registry 里、还能被搜到（2026-09-29 实测：卸了 OCR 搜索里还有它）。
 *  有了这份名单才能算出「上次有、这次没了」并摘干净 —— 这是「卸载 = 完全不存在于本应用」的
 *  最后一环，见 ai-spec §3.5。 */
const marketRegistered = new Set<string>();

export function installedMarketPlugins(): MarketPluginInfo[] {
  return installed;
}

/** 扫描插件目录，并把有效插件（重新）注册进 registry。
 *
 *  安装 / 卸载 / 升级后各调一次即可 —— **不必重启前端**（这与「技能改完要重启 agent」是两回事：
 *  技能是 agent 的能力、插件是前端的界面件）。失败只记控制台：插件目录出问题不该
 *  让整个搜索不可用。 */
export async function refreshMarketPlugins(): Promise<MarketPluginInfo[]> {
  let list: MarketPluginInfo[] = [];
  try {
    list = await invoke<MarketPluginInfo[]>("list_installed_plugins");
  } catch (e) {
    console.warn("[lunac] 插件目录扫描失败（按「没有第三方插件」处理）", e);
    return installed;
  }
  installed = list;
  // 版本变了的、或已经不存在的 id ⇒ 丢掉模块缓存与挂载钩子：
  // 前者是「升级后还在用旧代码」，后者是「卸载了但模块还占着内存（还可能留着定时器）」
  const live = new Map(list.map(p => [p.id, p.version]));
  for (const id of [...modules.keys()]) {
    const v = live.get(id);
    if (v === undefined || modules.get(id)!.version !== v) modules.delete(id);
  }
  for (const id of [...hooks.keys()]) {
    if (!live.has(id)) hooks.delete(id);
  }
  // ① 先摘「上次注册过、这次盘上没有」的 —— 卸载后必须从 registry 里消失，
  //    否则它还能被搜到（判据见 `marketRegistered` 的注释）。
  const usable = new Set(list.filter(p => p.valid).map(p => p.id));
  for (const id of [...marketRegistered]) {
    if (!usable.has(id)) {
      pluginRegistry.unregister(id);
      marketRegistered.delete(id);
    }
  }
  // ② 再挂上这次盘上可用的（先摘后挂：registry 不去重，重复 register 会出两行）。
  // **基础插件以 bundle 为准**：盘上就算被人塞了一份同名目录（`Modules\memo\`），也不许顶掉
  // 内置实现 —— 用户要求「基础插件不能安装 / 卸载」，见 kinds.ts 的 BASE_PLUGIN_IDS。
  for (const p of list) {
    if (isBasePlugin(p.id) || !p.valid) continue;
    pluginRegistry.unregister(p.id);
    pluginRegistry.register(wrap(p));
    marketRegistered.add(p.id);
  }
  return list;
}

function wrap(info: MarketPluginInfo): Plugin {
  return {
    id: info.id,
    name: info.name || info.id,
    // 关键词为空时退回名字 —— 否则这个插件在搜索里永远匹配不到任何输入
    keywords: info.keywords.length > 0 ? info.keywords : [info.name || info.id],
    description: info.description || "",
    // 图标留空是合法的：结果区会走 `pluginIconSvg()` 给主题图标，这里只是兜底
    icon: info.icon || "",
    // 声明的宿主能力原样带过去（`layout.takeover` 这类要让 main.ts 判定的东西）
    permissions: info.permissions || [],
    execute: async (input: string): Promise<PluginResult> => {
      const mod = await loadModule(info);
      const fn = pickExecute(mod);
      if (!fn) {
        throw new Error(t("settings.plugins_market_no_execute", { id: info.id }));
      }
      const out = await fn(input);
      return normalizeResult(out, info.id);
    },
  };
}

async function loadModule(info: MarketPluginInfo): Promise<unknown> {
  const cached = modules.get(info.id);
  if (cached) return cached.mod;
  // `/* @vite-ignore */` 让 Vite 不要把这一行当成「可分析的静态导入」——
  // 参数是运行时才拼出来的 asset URL，打包器无从分析（不写它 dev 下会报 warning）
  const url = convertFileSrc(info.entryPath);
  const mod = await import(/* @vite-ignore */ url);
  modules.set(info.id, { version: info.version, mod });
  return mod;
}

/** 按 id 找最近一次扫描到的插件信息（attach/detach 钩子要拿它去加载模块）。 */
function infoOf(id: string): MarketPluginInfo | undefined {
  return installed.find(p => p.id === id);
}

/** 磁盘上（`Modules\<id>\`）是否有一份**可用**的插件。
 *
 *  给 `attach.ts` 判「该用用户装的那份，还是用编译进 bundle 的内置那份」用。
 *  **坏包不算**：`valid === false` 的目录顶不掉内置实现 —— 否则用户装坏一个包，
 *  连内置的那份也跟着不能用了。 */
export function hasDiskPlugin(id: string): boolean {
  const info = infoOf(id);
  return !!info && info.valid;
}

/** 用**磁盘插件模块自带的 `attach(root)`** 挂载（2026-09-28）。
 *
 *  成功返回 true —— 调用方（`attach.ts`）据此判定「这个插件自己会挂载，不用宿主那份硬编码表」。
 *  没导出钩子 / 模块加载失败都返回 false（与「内置表未命中」同义：没有监听要挂）。
 *  失败**只记控制台**：插件坏了不该让结果区整个渲染不出来。 */
export async function externalAttach(id: string, root: HTMLElement): Promise<boolean> {
  const info = infoOf(id);
  if (!info) return false;
  let mod: unknown;
  try {
    mod = await loadModule(info);
  } catch (e) {
    console.warn("[lunac] 插件模块加载失败，跳过挂载：", id, e);
    return false;
  }
  const hook = pickAttach(mod);
  if (!hook) return false;
  try {
    await hook.attach(root);
  } catch (e) {
    console.warn("[lunac] 插件 attach 失败：", id, e);
    return false;
  }
  hooks.set(id, hook.detach ? { detach: hook.detach } : {});
  return true;
}

/** 面板关闭时收尾（与 `externalAttach` 成对）。**同步**：调用方在关窗路径上，
 *  不能等一个 await（见 main.ts / plugin-window.ts 的关闭流程）。 */
export function externalDetach(id: string): void {
  const h = hooks.get(id);
  hooks.delete(id);
  if (!h?.detach) return;
  try {
    h.detach();
  } catch (e) {
    console.warn("[lunac] 插件 detach 失败：", id, e);
  }
}

/** 从模块里取挂载钩子：具名导出优先，其次默认导出的对象（与 `pickExecute` 同一套宽容度）。 */
function pickAttach(
  mod: unknown,
): { attach: (root: HTMLElement) => unknown; detach?: () => void } | null {
  const m = mod as { attach?: unknown; detach?: unknown; default?: unknown };
  const def = m?.default as { attach?: unknown; detach?: unknown } | undefined;
  const attach =
    typeof m?.attach === "function"
      ? (m.attach as (root: HTMLElement) => unknown)
      : def && typeof def.attach === "function"
        ? (def.attach as (root: HTMLElement) => unknown)
        : null;
  if (!attach) return null;
  const detach =
    typeof m?.detach === "function"
      ? (m.detach as () => void)
      : def && typeof def.detach === "function"
        ? (def.detach as () => void)
        : undefined;
  return { attach, detach };
}

/** 从模块里取 `execute`：默认导出的对象 / 默认导出的函数 / 具名导出，三种都认。 */
function pickExecute(mod: unknown): ((input: string) => unknown) | null {
  const m = mod as { default?: unknown; execute?: unknown };
  const def = m?.default;
  if (typeof def === "function") return def as (input: string) => unknown;
  if (def && typeof (def as { execute?: unknown }).execute === "function") {
    return (def as { execute: (input: string) => unknown }).execute;
  }
  if (typeof m?.execute === "function") return m.execute as (input: string) => unknown;
  return null;
}

/** 返回值归一化：字符串按文本块，`{type, content}` 原样，其余情况**如实报错**而不是渲染一个空块。 */
function normalizeResult(out: unknown, id: string): PluginResult {
  if (typeof out === "string") return { type: "text", content: out };
  if (out && typeof out === "object" && "content" in (out as Record<string, unknown>)) {
    const o = out as { type?: unknown; content?: unknown };
    return {
      type: o.type === "html" ? "html" : "text",
      content: String(o.content ?? ""),
    };
  }
  throw new Error(t("settings.plugins_market_bad_result", { id }));
}
