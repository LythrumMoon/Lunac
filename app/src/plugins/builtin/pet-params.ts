// ── pet-params.ts — 桌宠「调试参数 + 运行时 id 表」缓存（2026-10-10）──────
//
// **为什么存在**（backlog ㉝③）—— 一举解决两件事：
//   ① 进入插件（设置窗）**读取之前**就能拿到上次的数据。设置窗**拿不到运行中的桌宠实例**：
//      它列参数 / ArtMesh / Part 全靠桌宠窗挂载模型时写进 localStorage 的「运行时 id 表」
//      （`pet.ts` 的 `CORE_IDS_KEY`）—— 没开过桌宠时那份是空的 ⇒ 调试区 / 热键页一片空。
//   ② **AI 可直接改这个文件**，用户重开设置窗时**反向同步**进设置面板。
// 于是把「运行时 id 表 + 调试参数」一并落到模型目录下 `lunac-live2d\` 里的文件里。
//
// **存哪**：与模型**同目录**下的 `lunac-live2d\lunac.params.json`（沿用 `lunac.hotkeys.json` /
// `lunac.chains.json` 的同一条最小口子纪律：宿主命令只认模型目录 + 固定文件名，不接受
// 任意路径）。写盘由宿主命令 `write_pet_params` 做（WebView 不能写盘）；读同理由宿主
// `read_pet_params`。热键 / 动作链各有自己那一份，本文件只管「调试参数」这一份。
//
// **两半各管什么、谁读谁写**：
//   · `coreIds` —— 运行时 id 表（parameters / drawables / parts / parameterRanges /
//     physicsInputs / physicsOutputs）。**桌宠窗**挂载模型时用引擎给的 `listCoreIds()` +
//     `enumerateTargets()` 的物理分类覆盖写入；**设置窗只读**它列目标，桌宠轮读它给 AI 注词表。
//   · `params`  —— 调试区每个参数的**保持值**（`PetConfig.params[模型路径]` 的镜像）。
//     **设置窗**在拖动 / 提交时写（防抖），**桌宠窗**挂载时读并应用（文件优先、config 兜底）。
//
// **容错是硬要求**：解析失败 / 文件缺字段 / 字段错型一律**当空处理**，绝不抛错 ——
// 绝不因为一份坏缓存把设置窗或桌宠窗搞崩。

import { invoke } from "@tauri-apps/api/core";

/** 运行时 id 表（与 `pet.ts` 的 `CoreIdTable` 同构）。 */
export interface PetParamCoreIds {
  parameters: string[];
  drawables: string[];
  parts: string[];
  /** 参数取值范围（Cubism Core 的 min/max/default）；旧数据没有这个字段时为空对象。 */
  parameterRanges: Record<string, { min: number; max: number; def: number }>;
  /** **物理输入**参数 id（`.physics3.json`）；旧数据没有这个字段时为空数组。 */
  physicsInputs: string[];
  /** **物理输出**参数 id（被物理每帧写值的那些）；旧数据没有这个字段时为空数组。 */
  physicsOutputs: string[];
}

/** `lunac.params.json` 的完整结构（AI 会直接读写它，见文件头）。 */
export interface PetParamCache {
  version: 1;
  /** 这份缓存属于哪个模型文件（`*.model3.json` 的**文件名**）—— 拷文件夹时用来核对。 */
  model: string;
  coreIds: PetParamCoreIds;
  /** 调试区每个参数的保持值（参数 id → 数值）。 */
  params: Record<string, number>;
}

function baseName(p: string): string {
  return p.split(/[\\/]/).pop() || "";
}

/** 一个空缓存（文件不存在 / 坏掉时的起点）。 */
export function emptyPetParamCache(modelPath: string): PetParamCache {
  return {
    version: 1,
    model: baseName(modelPath),
    coreIds: {
      parameters: [],
      drawables: [],
      parts: [],
      parameterRanges: {},
      physicsInputs: [],
      physicsOutputs: [],
    },
    params: {},
  };
}

function strArray(v: unknown): string[] {
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
}

function numMap(v: unknown): Record<string, number> {
  const out: Record<string, number> = {};
  if (v && typeof v === "object") {
    for (const [k, val] of Object.entries(v as Record<string, unknown>)) {
      if (typeof val === "number" && Number.isFinite(val)) out[k] = val;
    }
  }
  return out;
}

function rangeMap(v: unknown): Record<string, { min: number; max: number; def: number }> {
  const out: Record<string, { min: number; max: number; def: number }> = {};
  if (v && typeof v === "object") {
    for (const [k, val] of Object.entries(v as Record<string, unknown>)) {
      const r = val as { min?: unknown; max?: unknown; def?: unknown };
      if (r && typeof r === "object") {
        out[k] = {
          min: typeof r.min === "number" && Number.isFinite(r.min) ? r.min : 0,
          max: typeof r.max === "number" && Number.isFinite(r.max) ? r.max : 0,
          def: typeof r.def === "number" && Number.isFinite(r.def) ? r.def : 0,
        };
      }
    }
  }
  return out;
}

/** 读该模型的参数缓存。缺文件 / 坏 JSON / 非对象 ⇒ `null`（当没有）。字段错型一律当空，不抛错。 */
export async function loadPetParams(modelPath: string): Promise<PetParamCache | null> {
  let raw: string | null = null;
  try {
    raw = await invoke<string | null>("read_pet_params", { modelPath });
  } catch {
    return null;
  }
  if (!raw) return null;
  try {
    const o = JSON.parse(raw) as Partial<PetParamCache> | null;
    if (!o || typeof o !== "object") return null;
    const ci = (o.coreIds ?? {}) as Partial<PetParamCoreIds>;
    return {
      version: 1,
      model: typeof o.model === "string" ? o.model : baseName(modelPath),
      coreIds: {
        parameters: strArray(ci.parameters),
        drawables: strArray(ci.drawables),
        parts: strArray(ci.parts),
        parameterRanges: rangeMap(ci.parameterRanges),
        physicsInputs: strArray(ci.physicsInputs),
        physicsOutputs: strArray(ci.physicsOutputs),
      },
      params: numMap(o.params),
    };
  } catch {
    return null;
  }
}

/** 写回该模型的参数缓存，返回落点绝对路径（前端展示「存到哪了」）。 */
export async function savePetParams(modelPath: string, cache: PetParamCache): Promise<string> {
  return invoke<string>("write_pet_params", { modelPath, json: JSON.stringify(cache, null, 2) });
}
