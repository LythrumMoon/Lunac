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

// ── 表情来源（2026-10-09）────────────────────────────────────────────
// 「可用表情」有两个来源，且**引擎与设置窗必须看到同一份**，否则会出现「设置窗列得出、
// 但点了切不过去」的假按钮：
//   ① `model3.json` 的 `FileReferences.Expressions`（模型自带的声明）；
//   ② 宿主命令 `list_expression_files(model_path)` 扫模型文件**同层目录**里所有
//      `*.exp3.json`（目录里存在、模型却没声明的那些）。
// ②里与①**按绝对路径去重**（Windows 大小写不敏感、`/` 与 `\` 等价），①优先。

/** 一个「可用表情」条目。 */
export interface ExpressionItem {
  /** 引擎 `setExpression` / `model.expression()` 匹配用的 id —— 声明过的取 `Name`，
   *  仅目录里发现的取命令给的 `name`。多数模型的 `Name` 与显示名同值。 */
  id: string;
  /** 设置窗按钮上那枚显示名。 */
  name: string;
  /** `.exp3.json` 的**绝对路径**（声明过但没有 `File` 的为空串）。 */
  file: string;
  /** 是否来自 `model3.json` 的声明。`false` = 只在目录里发现 —— 引擎必须把它
   *  **追加进 settings** 才能切到（见 `./live2d-engine.ts` 的 `mountLive2D`）。 */
  declared: boolean;
}

/** 路径末段（去扩展名），`Name` 缺省时的兜底显示名。 */
function fileStem(p: string): string {
  const seg = p.split(/[\\/]/).pop() || "";
  return seg.replace(/\.[^.]+$/, "");
}

/** 去重键：Windows 路径大小写不敏感、`/` 与 `\` 等价 —— 统一小写 + 正斜杠再比。 */
function pathKey(p: string): string {
  return p.replace(/[\\/]+/g, "/").toLowerCase();
}

/** 取该模型**可用的全部表情**（设置窗按钮 + 引擎追加共用这一份）。
 *
 *  `json` 可传入调用方已经读过的那一份（引擎就不必再读一遍）。
 *  宿主命令不在 / 调用失败时**静默降级**为只用①—— 模型自带的表情照常可用，
 *  不会因为「宿主还没实现 list_expression_files」就整个表情区报错。 */
export async function listExpressions(modelPath: string, json?: Model3Json): Promise<ExpressionItem[]> {
  const j = json ?? (await readModelJson(modelPath));
  const modelDir = dirName(modelPath);
  const declared = j.FileReferences?.Expressions ?? [];

  const out: ExpressionItem[] = [];
  const seen = new Set<string>();
  for (const e of declared) {
    const file = e.File ? joinPath(modelDir, e.File) : "";
    // 显示名 / id 同源：`Name` 优先，缺失时退回 `.exp3.json` 文件名词干
    const name = (e.Name || (e.File ? fileStem(e.File) : "")).trim();
    if (!name) continue; // 既无 Name 也无 File ⇒ 列出来也切不到，不留空按钮
    if (file) seen.add(pathKey(file));
    out.push({ id: name, name, file, declared: true });
  }

  let extra: { path?: string; name?: string }[] = [];
  try {
    extra = (await invoke<{ path?: string; name?: string }[]>("list_expression_files", {
      model_path: modelPath,
    })) ?? [];
  } catch {
    extra = []; // 命令缺失 / 目录读不到：只用声明的那部分
  }
  for (const f of extra) {
    const file = typeof f?.path === "string" ? f.path : "";
    if (!file || seen.has(pathKey(file))) continue;
    const name = ((typeof f?.name === "string" && f.name.trim()) || fileStem(file)).trim();
    if (!name) continue;
    seen.add(pathKey(file));
    out.push({ id: name, name, file, declared: false });
  }
  return out;
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

/** 形象缩放的上下限 —— **与 `pet.ts` 的 `loadConfig` 校验区间必须同值**：
 *  超出区间的值会被 `loadConfig` 判非法、静默跳回 1，滚轮缩放若越界就会「松手后再开窗变回 100%」。 */
export const PET_SCALE_MIN = 0.4;
export const PET_SCALE_MAX = 8;

/** 预设动作的**合成动作组名**：桌宠窗挂载时把扫到的 `*.motion3.json` 注入到这个名字下
 *  （`settings.FileReferences.Motions[MOTION_EXTRA_GROUP]`），前端按同一份文件清单的下标播放。
 *  **唯一真相源**（`pet-hotkeys.ts` / `pet.ts` / `live2d-engine.ts` 都从这里取，别各写各的）。 */
export const MOTION_EXTRA_GROUP = "Lunac";

export interface MountOptions {
  /** 用户选的 `*.model3.json` 绝对路径。 */
  modelPath: string;
  /** 形象大小（占容器宽度的比例）—— 与图片模式共用控制台那一个配置项。 */
  scale: number;
  /** 形象相对**基准位置**（`fit()` 算出的底边中心）的像素偏移。换形象 / 重开窗后恢复。 */
  offsetX?: number;
  offsetY?: number;
  /** **整体旋转**（角度制）。绕模型自身的中心（`model.anchor` 那一点）转。缺省 0。 */
  rotation?: number;
  /** **额外表情**：引擎自己扫不到的 `.exp3.json`（导入到子目录 / 别处、模型文件同层里没有的）。
   *  桌宠窗在挂载前从 `lunac.hotkeys.json` 收集、按绝对路径去重后交进来。库的
   *  `model.expression(Name)` 只认 `settings.FileReferences.Expressions` 里登记过的项 ——
   *  不登记就是「列得出、点了切不动」（见 live2d-engine.ts 里追加那一段）。 */
  extraExpressions?: { name: string; file: string }[];
  /** **额外动作**：扫到的 `*.motion3.json`（用户口径：预设动作不看 `model3.json` 的声明，
   *  直接扫文件夹）。挂载时按**数组顺序**注入到 `MOTION_EXTRA_GROUP` 组里，序号即下标。 */
  extraMotions?: { name: string; file: string }[];
  /** **循环播放的动作**（2026-10-10 用户口径；2026-10-10 再更正：**与「待机」无关** ——
   *  它只是「让模型把被勾选的动作循环播放」，当初是按待机动作设计的，名字沿用至今）：
   *  被勾选的 `*.motion3.json`（是 `extraMotions` 的子集）。**引擎自己驱动**（按顺序轮播），
   *  不再借库的 idle 组 —— 库里那条 `startRandomMotion(groups.idle)` 会把「idle 预约」与
   *  用户显式播放互相顶掉，实测会让整个渲染永久卡死（详见 `live2d-engine.ts` 的兜底与驱动器注释）。 */
  idleMotions?: { name: string; file: string }[];
  /** **动作进度上报**（2026-10-10，backlog ㉝② 的进度条）：引擎会把「当前正在播的注入动作」
   *  的进度按 `Meta.Duration` 折算后定期回调过来（约 12Hz）。`null` = 当前没有在播我们注入的
   *  动作（或读不到该文件时长）⇒ 调用方据此把进度条清掉。**纯上报，不影响渲染**。 */
  onMotionProgress?: (info: MotionProgress | null) => void;
}

/** 一条「正在播的注入动作」的进度（引擎 → 桌宠窗 → 设置窗的进度条）。 */
export interface MotionProgress {
  /** 该动作在 `MOTION_EXTRA_GROUP` 里的下标（= `list_motion_files` 清单里的位置）。 */
  index: number;
  /** 本轮已播毫秒（循环播放取模，每轮从 0 起）。 */
  elapsedMs: number;
  /** 该动作自身的总时长（`Meta.Duration`，毫秒）。 */
  totalMs: number;
  /** 由动作链驱动时是第几步（0 起）；单动作播放时为 -1。 */
  chainStep: number;
  /** 动作链里**本步**的时长（毫秒）；单动作播放时为 0。 */
  stepTotalMs: number;
}

/** `zoomAtPointer` 的返回值：新的整体缩放系数 +（为锚住指针而调整后的）偏移。 */
export interface ZoomResult {
  scale: number;
  offsetX: number;
  offsetY: number;
}

export interface Live2DHandle {
  /** 改形象大小（不需要重建实例）。 */
  setScale(scale: number): void;
  /** 相对基准位置的像素偏移（Alt+拖动形象用）。与 `setScale` 各改各的，互不覆盖。
   *  图片模式不走这里（图片走 CSS transform）。 */
  setOffset(dx: number, dy: number): void;
  /** **整体旋转**（角度制，绕模型中心）。与 `setScale` / `setOffset` 各改各的、互不覆盖。
   *  图片模式不走这里。 */
  setRotation(deg: number): void;
  /** **指针锚点缩放**：以舞台内坐标 `(px,py)` 为锚点缩放 —— 锚点下的那个像素保持不动。
   *  `factor` 乘在**当前整体缩放系数**上（`>1` 放大 / `<1` 缩小），结果夹在
   *  `[PET_SCALE_MIN, PET_SCALE_MAX]`。返回新系数与调整后的偏移（调用方据此持久化）。 */
  zoomAtPointer(factor: number, px: number, py: number): ZoomResult;
  /** 停 / 起这一份实例自己的动画。**窗口不可见时必须停**（ai-spec §4.8：WebView2 对最小化窗
   *  仍报 `visible`，别指望浏览器替我们省电）。 */
  setRunning(on: boolean): void;
  /** 设一个 Live2D 参数并**保持**它。模型自播的 Idle / 动作**每帧都会改写参数**，所以
   *  「设一次」会被覆盖 —— 引擎内部会在每帧 update 前把这份值写回（做法与依据见
   *  `./live2d-engine.ts` 的 `heldParams`）。参数不存在时静默忽略（库自身会兜住）。 */
  setParameter(id: string, value: number): void;
  /** 读一个 Live2D 参数的**当前值**（运行时 Core 那份；取不到时返回 0）。
   *  热键的「加」模式（`param.mode === "add"`）拿它当基准。 */
  getParameter(id: string): number;
  /** 切一个表情，按 `*.model3.json` 的 `FileReferences.Expressions[].Name` 匹配。
   *  **一次性**：库内部做平滑过渡，不是每帧保持（与参数那套不同）。 */
  setExpression(id: string): void;
  /** 清除当前表情、还原默认（等价 `ExpressionManager.resetExpression()`）。 */
  resetExpressions(): void;
  /** **叠加一条表情层**（`key` = 热键 id；`params` = 该 `.exp3.json` 的 `Parameters`，
   *  `null` / 空数组 = 取消这一条）。与 `setExpression` **不同**：后者是库的互斥表情
   *  （切下一个会把上一个冲掉），这一条是**可叠加**的 —— 多条同时生效、互不冲掉，
   *  各自可单独取消（对齐 VTube Studio 的「Expression on/off」：变化的值会保留）。
   *  施加时机：每帧在动作 / 物理之后、用户「保持」参数之前（见 `./live2d-engine.ts`）。 */
  setExpressionLayer(key: string, params: { id: string; value: number; blend: string }[] | null): void;
  /** **口型（张嘴幅度）**：`0..1` 归一化（0 = 闭嘴、1 = 满量程），`null` = **放手**
   *  （让动作 / 物理自己演嘴）。朗读期间由前端按宿主下发的包络逐帧写（见 `tts.rs` 的
   *  `MOUTH_EVENT` 与 `pet.ts` 的 `driveMouth`）；写在表情层同级、用户「保持」之下。
   *  用哪条参数：优先模型声明的 LipSync 参数，退回 `ParamMouthOpenY`。 */
  setMouthOpen(v: number | null): void;

  // ── 热键（2026-10-10）需要的能力 ──────────────────────────────
  /** 播一个**动作**：按 `model3.json` 的 `FileReferences.Motions[组名]` + 组内序号。
   *  `index` 缺省 0（该组第一条）。组名不存在 / 该组为空时静默忽略。 */
  playMotion(group: string, index?: number): void;
  /** **播放一条动作链**（2026-10-10，backlog ㉝②）。`steps` 由桌宠窗折算好：
   *  `index` = 注入组里的动作下标、`ms` = 本步循环播放时长、`hardMs` = 硬停上限（`0` = 不限）。
   *  引擎按时间线逐步播放（本步动作提前结束就重播，凑满 `ms` 再走；到 `hardMs` 强制切）。
   *  `loop`（2026-10-11）= **整链循环**：末步播完**从头再来**；缺省 `false` = 播完即停。
   *  与每步的 `ms` / `hardMs` **叠加**（逐行照旧生效）—— 这一位只管「整链播完之后」。 */
  playChain(steps: { index: number; ms: number; hardMs: number }[], loop?: boolean): void;
  /** 停掉正在播的动作链（不碰其它动作 / 参数）。 */
  stopChain(): void;
  /** 设一个 **ArtMesh（Drawable）的不透明度并保持** —— VTS「隐藏某个配件」那类热键靠它。
   *  与参数那套同理：每帧写回，不被动作 / 表情冲掉。id 不存在时静默忽略。 */
  setDrawableOpacity(id: string, value: number): void;
  /** 设一个 **ArtMesh 的乘法色（tint）并保持** —— VTS「Art Mesh 颜色预设」那类热键靠它。
   *  `rgb` 三通道各 `0..1`（`{r:1,g:1,b:1}` = 原色、不改）；`null` = **还原原色**。
   *  写的是 Cubism Core 的 `drawables.multiplyColors`（框架每帧渲染时读它，见 `./live2d-engine.ts`）。
   *  id 不存在时静默忽略。 */
  setDrawableColor(id: string, rgb: { r: number; g: number; b: number } | null): void;
  /** 设一个 **Part 的不透明度并保持**（整块配件一起隐藏时用它比逐 ArtMesh 省事）。 */
  setPartOpacity(id: string, value: number): void;
  /** 开关模型**自身的物理**（`.physics3.json` 那套）。关掉 = 不再每帧评估物理，
   *  于是被物理驱动的输出参数**完全由我们（参数保持 / 动作）决定**。 */
  setPhysicsEnabled(on: boolean): void;
  /** 列出模型运行时的 id 表（参数 / ArtMesh / Part）—— 热键与调试区的「可选目标」。
   *  取不到时对应数组为空（不抛错）。`parameterRanges` 是该参数的取值区间（Cubism Core 的
   *  minimum/maximum/default），调试区拉条靠它定范围；取不到时为空对象。 */
  listCoreIds(): {
    parameters: string[];
    drawables: string[];
    parts: string[];
    parameterRanges: Record<string, { min: number; max: number; def: number }>;
  };

  destroy(): void;
}
