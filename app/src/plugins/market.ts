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

/** 已 import 过的模块缓存：`execute` 是懒加载的，但同一个插件只 import 一次。 */
const modules = new Map<string, unknown>();

export function installedMarketPlugins(): MarketPluginInfo[] {
  return installed;
}

/** 扫描插件目录，并把有效插件（重新）注册进 registry。
 *
 *  安装 / 卸载后各调一次即可 —— **不必重启前端**（这与「技能改完要重启 agent」是两回事：
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
  // 先摘后挂：registry 不去重，重复 register 会让结果区出现两行（见 `PluginRegistry.unregister`）
  for (const p of list) pluginRegistry.unregister(p.id);
  for (const p of list) {
    if (p.valid) pluginRegistry.register(wrap(p));
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
  if (cached) return cached;
  // `/* @vite-ignore */` 让 Vite 不要把这一行当成「可分析的静态导入」——
  // 参数是运行时才拼出来的 asset URL，打包器无从分析（不写它 dev 下会报 warning）
  const url = convertFileSrc(info.entryPath);
  const mod = await import(/* @vite-ignore */ url);
  modules.set(info.id, mod);
  return mod;
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
