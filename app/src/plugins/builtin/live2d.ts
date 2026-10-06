// ── Live2D 引擎层的「轻」那一半（L1，2026-09-30）──────────────────────
//
// 这里**不 import pixi、也不 import 引擎库**，只干三件事：
//   ① 路径与 `model3.json` 的引用改写（纯字符串，可单测）；
//   ② Cubism Core 的注入（`<Modules>\pet\engine\live2dcubismcore.min.js`，全局 `<script>`）；
//   ③ 控制台要用的「这个 json 能不能用」快速校验。
//
// **为什么必须拆成两个文件**（`./live2d.ts` 轻 + `./live2d-engine.ts` 重）：
// `pixi-live2d-display` 在**模块求值时**就 `if (!window.Live2DCubismCore) throw`（其 dist
// `cubism4.es.js` 末尾），而 Core 只能在运行时以全局 `<script>` 注入 ⇒ 引擎库**必须晚于**
// Core 求值。Rollup 的 `inlineDynamicImports`（插件包必须单文件，见 `vite.plugins.config.ts`）
// 会把动态 import **提前到 bundle 顶层求值**，所以「同包内惰性 import」这条路是死的。
// 唯一能控制求值时机的是**独立文件**：引擎那一份由 `pet.ts` 在 Core 就绪后
// `import(convertFileSrc(<插件目录>/engine/live2d-engine.js))` 拉进来（绝对 URL，
// 绕开 asset 协议下相对 specifier 必然落错的问题）。
//
// 其余结论（相对引用为什么要自己改写、Core 为什么自托管）见 `docs/agent-feature-backlog.md`
// 的 **L1** —— 改这里之前先重读那两条。

import { convertFileSrc, invoke } from "@tauri-apps/api/core";

/** 本插件 id。Core 与引擎产物都在 `<Modules>\<id>\engine\` 下。 */
const PLUGIN_ID = "pet";

/** Cubism Core 在插件目录内的相对路径（与清单 `dependencies[].dest` 同源）。 */
const CORE_REL = "engine/live2dcubismcore.min.js";
/** 引擎产物在插件目录内的相对路径（与 `scripts/build-plugins.ps1` 的 `extraEntries` 同源）。 */
const ENGINE_REL = "engine/live2d-engine.js";

// ══════════════════════════════════════════════════════════════════
//  路径（纯字符串 —— 这一份跑在 WebView 里，没有 node:path）
// ══════════════════════════════════════════════════════════════════

/** 拼接路径并折叠 `.` / `..`。**同时认 `\` 与 `/`**（Windows 的模型路径 + 模型里的引用两条来源）。 */
export function joinPath(baseDir: string, rel: string): string {
  const sep = baseDir.includes("\\") ? "\\" : "/";
  const parts = baseDir.replace(/[\\/]+$/, "").split(/[\\/]+/);
  for (const seg of rel.split(/[\\/]+/)) {
    if (!seg || seg === ".") continue;
    if (seg === "..") {
      // 不越过根（`C:` 那一段必须留住，否则整条会退化成相对路径）
      if (parts.length > 1) parts.pop();
      continue;
    }
    parts.push(seg);
  }
  return parts.join(sep);
}

/** 取父目录。没有分隔符时原样返回（调用方会因此得到一条清楚的 fetch 失败，而不是静默用错路径）。 */
export function dirName(p: string): string {
  const i = Math.max(p.lastIndexOf("\\"), p.lastIndexOf("/"));
  return i > 0 ? p.slice(0, i) : p;
}

// ══════════════════════════════════════════════════════════════════
//  模型定义（`*.model3.json`）
// ══════════════════════════════════════════════════════════════════

/** 本层真正用到的字段（其余字段原样透传 —— 引擎比我们懂它）。 */
export interface Model3Json {
  Version?: number;
  FileReferences?: {
    Moc?: string;
    Textures?: string[];
    Physics?: string;
    Pose?: string;
    DisplayInfo?: string;
    UserData?: string;
    Expressions?: { File?: string; Name?: string }[];
    Motions?: Record<string, { File?: string; Sound?: string }[]>;
  };
  [k: string]: unknown;
}

/** 8 处引用里的 5 个「单值」字段；另外三处是 `Textures[]` / `Expressions[].File` / `Motions{}.File`。 */
const SINGLE_REF_FIELDS = ["Moc", "Physics", "Pose", "DisplayInfo", "UserData"] as const;

/** 把每个**相对引用**过一遍 `abs()`。返回浅拷贝，不改原对象。 */
export function rewriteRefs(model: Model3Json, abs: (rel: string) => string): Model3Json {
  const fr = model.FileReferences ?? {};
  const out: NonNullable<Model3Json["FileReferences"]> = { ...fr };

  for (const k of SINGLE_REF_FIELDS) {
    const v = fr[k];
    if (typeof v === "string" && v) out[k] = abs(v);
  }
  if (Array.isArray(fr.Textures)) out.Textures = fr.Textures.map(abs);
  if (Array.isArray(fr.Expressions)) {
    out.Expressions = fr.Expressions.map(e => ({ ...e, File: e.File ? abs(e.File) : e.File }));
  }
  if (fr.Motions) {
    const motions: NonNullable<NonNullable<Model3Json["FileReferences"]>["Motions"]> = {};
    for (const [group, list] of Object.entries(fr.Motions)) {
      motions[group] = (list ?? []).map(x => {
        const item: { File?: string; Sound?: string } = { ...x };
        if (x.File) item.File = abs(x.File);
        // `Sound` 是**引擎自己**去拉的（相对该动作文件）。桌宠不做口型配音 ⇒ 不给引擎挂
        // sound 管理器，它会跳过。仍照常改写，是为了将来真要接上时路径是对的
        // （用户 2026-09-30：音频口型后续可能用得上）。
        if (x.Sound) item.Sound = abs(x.Sound);
        return item;
      });
    }
    out.Motions = motions;
  }
  return { ...model, FileReferences: out };
}

/** Cubism 4 = `Version >= 3` 且必须有 `FileReferences.Moc`。别的 json 一律当场说清楚。 */
export function assertCubism4(json: Model3Json): void {
  const v = Number(json?.Version ?? 0);
  if (!json?.FileReferences?.Moc || !(v >= 3)) {
    throw new Error("这不是 Cubism 4 的模型定义（缺 `FileReferences.Moc` 或 `Version < 3`）—— 请选 `*.model3.json`");
  }
}

export async function readModelJson(absPath: string): Promise<Model3Json> {
  let res: Response;
  try {
    res = await fetch(convertFileSrc(absPath));
  } catch (e) {
    throw new Error(`读不到模型文件（asset 协议拒绝或文件已移动）：${e instanceof Error ? e.message : String(e)}`);
  }
  if (!res.ok) throw new Error(`读模型定义失败（HTTP ${res.status}）`);
  try {
    return (await res.json()) as Model3Json;
  } catch {
    throw new Error("模型定义不是合法 JSON");
  }
}

/** 控制台选完文件后**先验一遍**：别等桌宠窗里才报错 —— 那个窗小，用户还可能已经把它收起来了。 */
export async function validateModelFile(
  absPath: string,
): Promise<{ ok: true; info: string } | { ok: false; reason: string }> {
  try {
    const json = await readModelJson(absPath);
    assertCubism4(json);
    const fr = json.FileReferences ?? {};
    const groups = Object.keys(fr.Motions ?? {});
    return {
      ok: true,
      info: `Cubism ${Number(json.Version)} · 纹理 ${fr.Textures?.length ?? 0} · 动作组 ${groups.length ? groups.join(" / ") : "无"}`,
    };
  } catch (e) {
    return { ok: false, reason: e instanceof Error ? e.message : String(e) };
  }
}

// ══════════════════════════════════════════════════════════════════
//  Cubism Core（专有文件，盘上那份由插件依赖装入）
// ══════════════════════════════════════════════════════════════════

/** 宿主 `plugins_dir_path()` = `<exe 根>\Modules`。 */
async function pluginDir(): Promise<string> {
  const modules = await invoke<string>("plugins_dir_path");
  return joinPath(modules, PLUGIN_ID);
}

/** Core 的绝对路径。 */
export async function cubismCorePath(): Promise<string> {
  return joinPath(await pluginDir(), CORE_REL);
}

/** 引擎产物（`./live2d-engine.ts` 打出来的那一份）的绝对路径。 */
export async function live2dEnginePath(): Promise<string> {
  return joinPath(await pluginDir(), ENGINE_REL);
}

export function cubismCoreLoaded(): boolean {
  return !!(globalThis as { Live2DCubismCore?: unknown }).Live2DCubismCore;
}

let corePromise: Promise<void> | null = null;

/** 按需注入 Core（幂等）。**失败不缓存** —— 用户补上文件或重装插件之后应当能再试。 */
export function ensureCubismCore(): Promise<void> {
  if (cubismCoreLoaded()) return Promise.resolve();
  if (!corePromise) {
    corePromise = loadCore().catch((e: unknown) => {
      corePromise = null;
      throw e instanceof Error ? e : new Error(String(e));
    });
  }
  return corePromise;
}

async function loadCore(): Promise<void> {
  await injectGlobalScript(convertFileSrc(await cubismCorePath()));
  if (!cubismCoreLoaded()) throw new Error("Cubism Core 已注入但全局对象仍不可见（文件可能不完整）");
}

function injectGlobalScript(src: string): Promise<void> {
  return new Promise<void>((resolve, reject) => {
    const s = document.createElement("script");
    s.src = src;
    s.async = false;
    s.onload = () => resolve();
    s.onerror = () => reject(new Error(`引擎文件加载失败（asset 协议拒绝或文件缺失）：${src}`));
    document.head.appendChild(s);
  });
}

// ══════════════════════════════════════════════════════════════════
//  挂载接口（**类型在这里，实现在 `./live2d-engine.ts`**）──────────────
// 放在这一份是刻意的：调用方（`pet.ts`）只需要类型，不该因此被拖进 pixi 那一坨。
// ══════════════════════════════════════════════════════════════════

export interface MountOptions {
  /** 用户选的 `*.model3.json` 绝对路径。 */
  modelPath: string;
  /** 形象大小（占容器宽度的比例）—— 与图片模式共用控制台那一个配置项。 */
  scale: number;
}

export interface Live2DHandle {
  /** 改形象大小（不需要重建实例）。 */
  setScale(scale: number): void;
  /** 停 / 起这一份实例自己的动画。**窗口不可见时必须停**（ai-spec §4.8：WebView2 对最小化窗
   *  仍报 `visible`，别指望浏览器替我们省电）。 */
  setRunning(on: boolean): void;
  destroy(): void;
}
