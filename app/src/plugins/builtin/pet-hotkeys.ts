// ── pet-hotkeys.ts — 桌宠热键的数据模型 / 读写 / 目标枚举（2026-10-10）──────
//
// **一条热键是什么**：`按键组合 → 动作`。动作类型对齐 VTube Studio：
//   motion（播动作）/ expression（**表情开关**，VTS 的 “Expression on/off”）/ param（设/加参数）
//   / artmesh（ArtMesh 显隐）/ artmeshColor（ArtMesh **染色 / tint**，对应 VTS 的
//   “Art Mesh Color Preset”）/ part（Part 显隐）/ physics（开关物理）。
//
// **⚠️ 导入的表情为什么「都是」这一类（2026-10-10 查证）**：VTS / Cubism 的 `.exp3.json`
//   顶层 `Type` **恒为** `"Live2D Expression"` —— 靠它区分不出任何东西；「这个文件做什么」
//   只能看 `Parameters`（改了哪些参数、按什么 Blend）。而 VTS 侧的**动作类型**只有
//   “Expression on/off” 这一种对应表情 ⇒ 所有 `.exp3.json` 归到 `expression` 是对的，
//   不是识别不准。两条语义：① 多次叠加**互不冲掉**（VTS：变化的值会保留）、② 再触发一次 = 关。
//
// **存哪**：与模型**同目录**的 `lunac.hotkeys.json`（用户口径：热键 / 表情 / 模型放同一个
// 文件夹）。写盘由宿主命令 `write_pet_hotkeys` 做（WebView 不能写盘）；读同理由宿主。
//
// **「适配所有模型」的关键**：本文件里**不出现任何模型名**。所有可选目标都从
//   · `model3.json` 的 `FileReferences.Motions`（动作组）
//   · 目录里的 `*.exp3.json` + 声明（表情，复用 live2d.ts 的 listExpressions）
//   · 运行时 Core 的 id 表（参数 / ArtMesh / Part，见 Live2DHandle.listCoreIds）
//   · `.physics3.json` 的 `PhysicsSettings[].Input/Output`（**物理输入 / 输出参数**）
// 现取现算。狐仙那 26 条只是它在**自己文件夹里**的一份实例配置。

import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { dirName, joinPath, listExpressions, readModelJson } from "./live2d";

// ══════════════════════════════════════════════════════════════════
//  数据模型
// ══════════════════════════════════════════════════════════════════

/** 一条动作。`type` 判别联合 —— 每种类型只带自己那几个字段。 */
export type HotkeyAction =
  | { type: "motion"; group: string; index: number }
  | { type: "expression"; target: string; mode: "set" | "clear"; file?: string }
  | { type: "param"; id: string; value: number; mode: "set" | "add" }
  | { type: "artmesh"; id: string; value: number }
  | { type: "artmeshColor"; id: string; color: string }
  | { type: "part"; id: string; value: number }
  | { type: "physics"; enabled: boolean }
  /** **动作链**（2026-10-10）：整体播放 `lunac.chains.json` 里 id 为 `id` 的那条链。 */
  | { type: "chain"; id: string };

export const ACTION_TYPES = [
  "motion",
  "chain",
  "expression",
  "param",
  "artmesh",
  "artmeshColor",
  "part",
  "physics",
] as const;
export type ActionType = (typeof ACTION_TYPES)[number];

/** 一条热键。`keys` 是**规范化后的按键串**（如 `Ctrl+Numpad1` / `Shift+Numpad2`）。 */
export interface Hotkey {
  id: string;
  name: string;
  keys: string;
  action: HotkeyAction;
}

interface HotkeyFile {
  version: 1;
  /** 这份配置属于哪个模型文件（`*.model3.json` 的**文件名**）—— 拷文件夹时用来核对。 */
  model: string;
  hotkeys: Hotkey[];
}

/** 模型里可绑定的全部目标（设置窗的下拉数据源）。取不到的类别一律空数组。 */
export interface HotkeyTargets {
  /** 动作组 + 该组条数（`model3.json` 的 `Motions`）。 */
  motions: { group: string; count: number }[];
  /** 表情显示名（与 `Live2DHandle.setExpression` 的 id 同源）。 */
  expressions: string[];
  /** 表情 id → 该 `.exp3.json` 的**绝对路径**（可空串）。热键的「表情对象」下拉改选另一个表情时，
   *  用它把 `action.file` 一并换掉 —— 否则 `target` 与 `file` 会对不上（叠加表情层读的是 `file`）。 */
  expressionFiles: Record<string, string>;
  /** 全部参数 id（运行时 Core）。 */
  parameters: string[];
  /** ArtMesh（Drawable）id。 */
  drawables: string[];
  /** Part id。 */
  parts: string[];
  /** **物理输入**参数（`.physics3.json` 里被读作输入的那些参数）。 */
  physicsInputs: string[];
  /** **物理输出**参数（被物理写值的那些 —— 设它们需要物理关掉或靠「保持」压过）。 */
  physicsOutputs: string[];
}

// ══════════════════════════════════════════════════════════════════
//  读写（与模型同目录）
// ══════════════════════════════════════════════════════════════════

function baseName(p: string): string {
  return p.split(/[\\/]/).pop() || "";
}

function newId(): string {
  return Date.now().toString(36) + Math.random().toString(36).slice(2, 6);
}

export function makeHotkey(name = "新热键", keys = "", action?: HotkeyAction): Hotkey {
  return {
    id: newId(),
    name,
    keys,
    action: action ?? { type: "expression", target: "", mode: "set" },
  };
}

/** 导入时给条目**自动分配**的默认按键串（用户口径：导入即可用）。
 *  基键取小键盘 1..9，逐层叠修饰键：第 1 条 `Ctrl+Numpad1` … 第 9 条 `Ctrl+Numpad9`、
 *  第 10 条 `Shift+Numpad1` … 共 7 层 ×9 = 63 个互不重复。超出部分退回裸小键盘数字。
 *  **注意**：按键现在只作 AI 侧的标识（本地不做按键监听，见文件头），不再是用户按的快捷键。 */
export function defaultKeysForIndex(i: number): string {
  const bases = [
    "Numpad1",
    "Numpad2",
    "Numpad3",
    "Numpad4",
    "Numpad5",
    "Numpad6",
    "Numpad7",
    "Numpad8",
    "Numpad9",
  ];
  const tiers = ["Ctrl", "Shift", "Alt", "Ctrl+Shift", "Ctrl+Alt", "Shift+Alt", "Ctrl+Shift+Alt"];
  const idx = Math.max(0, Math.floor(i));
  const base = bases[idx % bases.length];
  const tier = Math.floor(idx / bases.length);
  return tier < tiers.length ? `${tiers[tier]}+${base}` : base;
}

/** 读该模型的热键。文件不存在 / 内容坏掉 ⇒ 空数组（不抛错）。 */
export async function loadHotkeys(modelPath: string): Promise<Hotkey[]> {
  let raw: string | null = null;
  try {
    raw = await invoke<string | null>("read_pet_hotkeys", { modelPath });
  } catch {
    return [];
  }
  if (!raw) return [];
  try {
    const o = JSON.parse(raw) as Partial<HotkeyFile>;
    const list = Array.isArray(o.hotkeys) ? o.hotkeys : [];
    return list.filter(
      (h): h is Hotkey => !!h && typeof h === "object" && typeof (h as Hotkey).keys === "string",
    );
  } catch {
    return [];
  }
}

/** 写回该模型的热键，返回落点绝对路径（前端展示「存到哪了」）。 */
export async function saveHotkeys(modelPath: string, hotkeys: Hotkey[]): Promise<string> {
  const file: HotkeyFile = { version: 1, model: baseName(modelPath), hotkeys };
  return invoke<string>("write_pet_hotkeys", {
    modelPath,
    json: JSON.stringify(file, null, 2),
  });
}

// ══════════════════════════════════════════════════════════════════
//  目标枚举
// ══════════════════════════════════════════════════════════════════

async function readJsonAbs(absPath: string): Promise<unknown> {
  const res = await fetch(convertFileSrc(absPath));
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

/** 从 `.physics3.json` 取**物理的输入 / 输出参数 id**（去重、保持出现次序）。
 *  Cubism 公开 schema：`PhysicsSettings[].Input[].Source` / `[].Output[].Destination`，
 *  参数 id 取 `Target`（缺时认 `Id`）。文件读不到 / 结构不认识 ⇒ 两个空数组。 */
async function physicsParams(absPath: string): Promise<{ inputs: string[]; outputs: string[] }> {
  const json = (await readJsonAbs(absPath)) as { PhysicsSettings?: unknown } | null;
  const inputs: string[] = [];
  const outputs: string[] = [];
  const push = (arr: string[], v: string) => {
    if (v && !arr.includes(v)) arr.push(v);
  };
  const pickId = (obj: unknown): string => {
    const o = obj as { Target?: unknown; Id?: unknown } | null | undefined;
    if (typeof o?.Target === "string" && o.Target) return o.Target;
    if (typeof o?.Id === "string" && o.Id) return o.Id;
    return "";
  };
  const settings = json?.PhysicsSettings;
  if (!Array.isArray(settings)) return { inputs, outputs };
  for (const s of settings) {
    const inp = (s as { Input?: unknown })?.Input;
    if (Array.isArray(inp)) {
      for (const i of inp) push(inputs, pickId((i as { Source?: unknown })?.Source));
    }
    const out = (s as { Output?: unknown })?.Output;
    if (Array.isArray(out)) {
      for (const o of out) push(outputs, pickId((o as { Destination?: unknown })?.Destination));
    }
  }
  return { inputs, outputs };
}

/**
 * 枚举该模型可绑定的全部目标。
 *
 * `coreIds` 由调用方（桌宠窗 / 引擎句柄）传入 —— 那是**运行时**才有的 id 表；
 * 设置窗拿不到运行中的实例时传空，此时参数 / ArtMesh / Part 三类退化为空（其余照常）。
 */
export async function enumerateTargets(
  modelPath: string,
  coreIds?: { parameters: string[]; drawables: string[]; parts: string[] },
): Promise<HotkeyTargets> {
  const out: HotkeyTargets = {
    motions: [],
    expressions: [],
    expressionFiles: {},
    parameters: coreIds?.parameters ?? [],
    drawables: coreIds?.drawables ?? [],
    parts: coreIds?.parts ?? [],
    physicsInputs: [],
    physicsOutputs: [],
  };

  let json: Awaited<ReturnType<typeof readModelJson>> | null = null;
  try {
    json = await readModelJson(modelPath);
  } catch {
    json = null;
  }
  if (json?.FileReferences?.Motions) {
    for (const [group, list] of Object.entries(json.FileReferences.Motions)) {
      out.motions.push({ group, count: Array.isArray(list) ? list.length : 0 });
    }
  }
  try {
    const items = await listExpressions(modelPath, json ?? undefined);
    out.expressions = items.map(e => e.id);
    for (const it of items) out.expressionFiles[it.id] = it.file;
  } catch {
    out.expressions = [];
  }

  // 物理文件由 model3.json 的 `FileReferences.Physics` 指过来（相对模型目录）。
  const physRel = json?.FileReferences?.Physics;
  if (typeof physRel === "string" && physRel) {
    try {
      const abs = joinPath(dirName(modelPath), physRel);
      const { inputs, outputs } = await physicsParams(abs);
      out.physicsInputs = inputs;
      out.physicsOutputs = outputs;
    } catch {
      /* 读不到物理文件：留空即可，不影响其它目标 */
    }
  }
  return out;
}

// ══════════════════════════════════════════════════════════════════
//  按键串（规范化 / 展示）
// ══════════════════════════════════════════════════════════════════

/** 一条 exp3 里的参数项（叠加表情时逐条施加）。`blend` 取 Cubism 的三种写法。 */
export interface ExprParam {
  id: string;
  value: number;
  blend: string;
}

/** `.exp3.json` 参数集的缓存（同一文件的 `Parameters` 只读一次）。键 = 绝对路径。 */
const exprParamCache = new Map<string, ExprParam[]>();

/** 读一个 `.exp3.json` 的 `Parameters`（叠加表情要按它逐条施加）。读不到返回空数组。
 *
 *  VTS 的 exp3 **恒**是 `{"Type":"Live2D Expression","Parameters":[…]}` —— `Type` 没有区分度，
 *  所以「这个文件到底做什么」只能看 `Parameters` 里改了哪些参数（见文件头的类型说明）。 */
export async function readExpressionParams(file: string): Promise<ExprParam[]> {
  const key = file.toLowerCase();
  const hit = exprParamCache.get(key);
  if (hit) return hit;
  let out: ExprParam[] = [];
  try {
    const res = await fetch(convertFileSrc(file));
    if (res.ok) {
      const o = (await res.json()) as { Parameters?: unknown };
      const list = Array.isArray(o.Parameters) ? o.Parameters : [];
      out = list
        .map(it => {
          const id = typeof (it as { Id?: unknown })?.Id === "string" ? (it as { Id: string }).Id : "";
          const value = (it as { Value?: unknown })?.Value;
          const blendRaw = (it as { Blend?: unknown })?.Blend;
          const blend = typeof blendRaw === "string" && blendRaw ? blendRaw : "Overwrite";
          return { id, value: typeof value === "number" ? value : NaN, blend };
        })
        .filter(p => !!p.id && Number.isFinite(p.value));
    }
  } catch {
    out = [];
  }
  exprParamCache.set(key, out);
  return out;
}

/** 由键盘事件生成规范按键串。修饰键顺序固定 Ctrl→Shift→Alt；主键取 `event.code`（键盘布局无关，
 *  `Numpad1` / `KeyA` / `F5` 这种），比 `event.key` 稳。 */
export function keysFromEvent(ev: KeyboardEvent): string | null {
  const mods: string[] = [];
  if (ev.ctrlKey) mods.push("Ctrl");
  if (ev.shiftKey) mods.push("Shift");
  if (ev.altKey) mods.push("Alt");
  const code = ev.code;
  if (!code) return null;
  // 只按修饰键不算一条完整按键
  if (/^(Control|Shift|Alt|Meta)(Left|Right)$/.test(code)) return null;
  const main = code.startsWith("Numpad")
    ? code
    : code.startsWith("Key")
      ? code.slice(3)
      : code.startsWith("Digit")
        ? code.slice(5)
        : code;
  return [...mods, main].join("+");
}

// ══════════════════════════════════════════════════════════════════
//  VTS 模型全貌：`.vtube.json` / `.cdi3.json`（2026-10-10）
// ══════════════════════════════════════════════════════════════════
//
// `.vtube.json` = VTube Studio 自动生成的模型配置，就放在模型旁边。**官方无 schema**、
// 但纯人类可读 JSON（见 VTS wiki「VTS Model File」）⇒ **只取我们要的两处、其余全忽略**，
// 字段名对不上就当没有（VTS 换版本改结构的代价必须由「容错」吸收，不能崩）：
//   · `Hotkeys[]` = **作者预设的热键**：`{ Name, Action, File, Folder }`。`Action` 是 VTS 的
//     动作类型名，与 VTS 公开 API 的 `HotkeyType` 同名（`ToggleExpression` / `TriggerAnimation` …）。
//   · `ParameterSettings[]` = 参数绑定：`OutputLive2D` 是模型参数 id（含 VTS 扩展参数，如 `Param139`）。
// `.cdi3.json` = Cubism Display Info：`Parameters[] / Parts[] / ArtMeshes[]` 各带 `Id` + `Name`
//   —— 调试区给参数 / 部件显示**可读名字**靠它（VTS 不需要它，我们要）。

/** 宽容读一个 JSON 文件；读不到 / 不是 JSON ⇒ `null`（不抛错）。 */
async function readJsonOrNull(absPath: string): Promise<unknown | null> {
  try {
    const res = await fetch(convertFileSrc(absPath));
    if (!res.ok) return null;
    return await res.json();
  } catch {
    return null;
  }
}

/** `.vtube.json` 里一条作者预设热键。 */
export interface VtsHotkey {
  name: string;
  /** VTS 动作类型名（`ToggleExpression` / `TriggerAnimation` / …）。 */
  action: string;
  /** 关联文件（相对模型目录，如 `去水印.exp3.json`）；可为空串。 */
  file: string;
}

/** 读 `.vtube.json` 的**作者热键表**。读不到 / 结构不认识 ⇒ 空数组。 */
export async function readVtsHotkeys(file: string): Promise<VtsHotkey[]> {
  const j = await readJsonOrNull(file);
  const list = (j as { Hotkeys?: unknown } | null)?.Hotkeys;
  if (!Array.isArray(list)) return [];
  const out: VtsHotkey[] = [];
  for (const it of list) {
    const o = it as { Name?: unknown; Action?: unknown; File?: unknown; Folder?: unknown };
    const action = typeof o?.Action === "string" ? o.Action.trim() : "";
    if (!action) continue;
    const base = typeof o?.File === "string" ? o.File.trim() : "";
    const folder = typeof o?.Folder === "string" ? o.Folder.trim() : "";
    const name = typeof o?.Name === "string" ? o.Name.trim() : "";
    out.push({ name: name || base, action, file: base ? (folder ? `${folder}/${base}` : base) : "" });
  }
  return out;
}

/** 读 `.vtube.json` 参数绑定用到的**模型参数 id**（`ParameterSettings[].OutputLive2D`，去重）。 */
export async function readVtsParamIds(file: string): Promise<string[]> {
  const j = await readJsonOrNull(file);
  const list = (j as { ParameterSettings?: unknown } | null)?.ParameterSettings;
  if (!Array.isArray(list)) return [];
  const out: string[] = [];
  for (const it of list) {
    const id = (it as { OutputLive2D?: unknown })?.OutputLive2D;
    if (typeof id === "string" && id && !out.includes(id)) out.push(id);
  }
  return out;
}

/** `.cdi3.json` 的可读名字表（参数 / 部件 / ArtMesh 各一份，`id → 名字`）。 */
export interface Cdi3Names {
  parameters: Record<string, string>;
  parts: Record<string, string>;
  artMeshes: Record<string, string>;
}

/** 读 `.cdi3.json` 的可读名字。读不到 / 缺字段 ⇒ 各表为空（不抛错）。 */
export async function readCdi3Names(file: string): Promise<Cdi3Names> {
  const out: Cdi3Names = { parameters: {}, parts: {}, artMeshes: {} };
  const j = await readJsonOrNull(file);
  if (!j) return out;
  const collect = (key: string, into: Record<string, string>) => {
    const list = (j as Record<string, unknown>)[key];
    if (!Array.isArray(list)) return;
    for (const it of list) {
      const o = it as { Id?: unknown; Name?: unknown };
      if (typeof o?.Id === "string" && o.Id && typeof o?.Name === "string" && o.Name) into[o.Id] = o.Name;
    }
  };
  collect("Parameters", out.parameters);
  collect("Parts", out.parts);
  collect("ArtMeshes", out.artMeshes);
  return out;
}

/** VTS 动作类型名 → 我们的动作类型。认不出的（换背景 / 截图 / 物品 / Twitch …）返回 `null`。 */
export function actionTypeFromVts(action: string): ActionType | null {
  switch (action) {
    case "ToggleExpression":
      return "expression";
    case "TriggerAnimation":
      return "motion";
    default:
      return null;
  }
}

/** 把一条 VTS 热键转成我们的热键（认不出的动作类型返回 `null`）。
 *  `dir` = 模型目录的绝对路径（VTS 的 `File` 是相对路径，要拼成绝对路径才喂得进引擎）。 */
export function hotkeyFromVts(h: VtsHotkey, dir: string): Hotkey | null {
  const type = actionTypeFromVts(h.action);
  if (!type) return null;
  const name = h.name || "热键";
  if (type === "expression") {
    const abs = h.file ? joinPath(dir, h.file) : "";
    return makeHotkey(name, "", { type: "expression", target: h.name, mode: "set", file: abs });
  }
  // 动作：`File` 只是文件名，组名 / 序号要从 model3.json 反查（暂留空，由用户在下拉里选）。
  return makeHotkey(name, "", { type: "motion", group: "", index: 0 });
}

// ══════════════════════════════════════════════════════════════════
//  预设动作（`lunac.motions.json`，2026-10-10）
// ══════════════════════════════════════════════════════════════════
//
// **一条动作 = 一个文件**（2026-10-10 用户口径，替代原先的 `lunac.motions.json` 预设表）：
// 「预设动作」页直接列模型文件夹里扫到的 `*.motion3.json`（见 [`listMotionFiles`]），播放走
// 引擎的 `playMotion(合成组, 下标)`；「新建动作」由桌宠窗/设置窗拼模板、宿主 `write_motion_file`
// 落盘（覆盖全参数的常数曲线，交给 AI 编）。宿主另有 `delete_motion_file` / `rename_motion_file`。

/** 一个动作文件（`*.motion3.json`）。预设动作页与桌宠窗注入**共用同一份**（下标即组内序号）。 */
export interface MotionFileRef {
  /** 展示名（文件名词干）。 */
  name: string;
  /** 绝对路径。 */
  path: string;
}

/** 递归扫模型文件夹里的全部 `*.motion3.json`（宿主 `list_motion_files`，**按路径排序**）。
 *
 *  用户口径（2026-10-10）：动作来源 = **文件夹里真实的 motion3 文件**，不看 `model3.json`
 *  的 `Motions` 声明。顺序即「注入到 `MOTION_EXTRA_GROUP` 组里的序号」，两处必须同一份。 */
export async function listMotionFiles(modelPath: string): Promise<MotionFileRef[]> {
  const dir = dirName(modelPath);
  if (!dir) return [];
  try {
    const files = await invoke<MotionFileRef[]>("list_motion_files", { dir });
    return Array.isArray(files)
      ? files.filter((f): f is MotionFileRef => !!f && typeof f.path === "string")
      : [];
  } catch {
    return [];
  }
}

/** 收集若干 `.motion3.json` 里**每个参数出现过的取值** —— 开关判定的「实际取值」那一半
 *（开关类参数在曲线里只会出现 0 / 1，连续量会出现中间值）。
 *
 *  `Curves[].Segments` 是**扁平**数组：`[x0, y0, type, …]`，每段的载荷按 type 定长 ——
 *  0=线性 / 2=阶梯 / 3=反阶梯 各带 `(x, y)`，1=贝塞尔带 `(cx1,cy1,cx2,cy2,x,y)`；
 *  段末那个 y 就是我们要的值（段起点是数组第 2 个元素）。 */
export async function collectMotionParamValues(files: string[]): Promise<Map<string, Set<number>>> {
  const out = new Map<string, Set<number>>();
  for (const f of files) {
    const j = await readJsonOrNull(f);
    const curves = (j as { Curves?: unknown } | null)?.Curves;
    if (!Array.isArray(curves)) continue;
    for (const c of curves) {
      const id = (c as { Id?: unknown })?.Id;
      const segs = (c as { Segments?: unknown })?.Segments;
      if (typeof id !== "string" || !id || !Array.isArray(segs) || segs.length < 3) continue;
      const set = out.get(id) ?? new Set<number>();
      if (typeof segs[1] === "number") set.add(segs[1]);
      let i = 2;
      while (i < segs.length) {
        const t = segs[i];
        i += t === 1 ? 7 : 3; // type + 载荷（贝塞尔 6 个、其余 2 个）
        const y = segs[i - 1];
        if (typeof y === "number") set.add(y);
      }
      out.set(id, set);
    }
  }
  return out;
}
