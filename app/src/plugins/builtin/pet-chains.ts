// ── pet-chains.ts — 桌宠「动作链」的数据模型 / 读写（2026-10-10）──────────
//
// **一条动作链**：把多个动作排在**一条时间线**上，逐个播放；每一步带两个时间参数：
//   · `loopSec` —— **循环播放时长**（秒）：这一步的动作播够这么久就切下一步。
//     动作本身若很快结束（`Meta.Duration` 短 / 非 Loop），引擎会在这一步内**反复重播**它，
//     凑满 `loopSec` 再走 —— 这就是「循环播放」。
//   · `stopSec` —— **硬停秒**（秒，可选；`0` / 缺省 = 不限）：从**本步开始**算，到这一秒
//     无论动作播到哪都**强制**切下一步。它是 `loopSec` 的**上限保护**（通常 ≥ `loopSec`，
//     用于兜住「某个动作永远不结束」）。⇒ 本步实际时长 = `min(loopSec, stopSec || loopSec)`。
//
// **存哪**：模型目录下 `lunac-live2d\lunac.chains.json`（沿用热键的同一条口径：热键 / 动作 /
// 模型放同一个文件夹，拷走整个文件夹即完整）。写盘由宿主命令 `write_pet_chains` 做
//（WebView 不能写盘）；读同理由宿主。
//
// **怎么触发**：① 热键动作类型 `chain`（`{ type:"chain"; id }`）选中一条链后整体播放；
// ② AI：链名进「动作词表」，`PetControl` 的 `action:"auto"` / 正文 `[链名]` 标签按名字命中即播。

import { invoke } from "@tauri-apps/api/core";

/** 动作链里的一步。`file` = 该步动作的**绝对路径**（与 `listMotionFiles` 同源）。 */
export interface ChainStep {
  /** 该步动作文件（`*.motion3.json`）的绝对路径。 */
  file: string;
  /** 循环播放时长（秒）：播够这么久就切下一步。 */
  loopSec: number;
  /** 硬停秒（秒）：从本步开始算，到这一秒强制切下一步；`0` / 缺省 = 不限（= `loopSec`）。 */
  stopSec: number;
}

/** 一条动作链。`id` 用于热键引用（文件名会变，id 不变）。 */
export interface MotionChain {
  id: string;
  name: string;
  steps: ChainStep[];
  /** **整链循环**（2026-10-11）：`true` = 整条链播完**从头再来**；`false`（缺省）= 播完即停（保持原行为）。
   *  与**每步**的 `loopSec` / `stopSec` **叠加**（逐行照旧生效）—— 这一位只管「整链播完之后是否重来」，
   *  不改变任何一步自身的时长语义（见 `stepSeconds`）。 */
  loop: boolean;
}

interface ChainFile {
  version: 1;
  /** 这份配置属于哪个模型文件（`*.model3.json` 的**文件名**）—— 拷文件夹时用来核对。 */
  model: string;
  chains: MotionChain[];
}

const MIN_SEC = 0.1;
const MAX_SEC = 3600;

function newId(): string {
  return Date.now().toString(36) + Math.random().toString(36).slice(2, 6);
}

/** 新建一条空链。 */
export function makeChain(name = "新动作链"): MotionChain {
  return { id: newId(), name, steps: [], loop: false };
}

/** 新建一步（`file` 由调用方从当前动作文件清单里挑）。 */
export function makeStep(file = ""): ChainStep {
  return { file, loopSec: 3, stopSec: 0 };
}

/** 把任意来源的秒数夹到合法区间（`<=0` 时给默认值）。 */
function clampSec(v: unknown, def: number): number {
  const n = typeof v === "number" && Number.isFinite(v) ? v : def;
  if (n <= 0) return def;
  return Math.min(MAX_SEC, Math.max(MIN_SEC, n));
}

function baseName(p: string): string {
  return p.split(/[\\/]/).pop() || "";
}

/** 读该模型的动作链。文件不存在 / 内容坏掉 ⇒ 空数组（不抛错）。 */
export async function loadChains(modelPath: string): Promise<MotionChain[]> {
  let raw: string | null = null;
  try {
    raw = await invoke<string | null>("read_pet_chains", { modelPath });
  } catch {
    return [];
  }
  if (!raw) return [];
  try {
    const o = JSON.parse(raw) as Partial<ChainFile>;
    const list = Array.isArray(o.chains) ? o.chains : [];
    return list
      .filter((c): c is MotionChain => !!c && typeof c === "object" && typeof (c as MotionChain).id === "string")
      .map(c => ({
        id: String(c.id),
        name: typeof c.name === "string" && c.name ? c.name : "动作链",
        // 整链循环：缺省 / 坏值一律当 false（保持「播完即停」的旧行为，不让坏数据把链卡成死循环）。
        loop: (c as MotionChain).loop === true,
        steps: (Array.isArray(c.steps) ? c.steps : [])
          .filter((s): s is ChainStep => !!s && typeof s === "object" && typeof (s as ChainStep).file === "string")
          .map(s => ({
            file: s.file,
            loopSec: clampSec(s.loopSec, 3),
            stopSec: typeof s.stopSec === "number" && Number.isFinite(s.stopSec) && s.stopSec > 0
              ? clampSec(s.stopSec, 3)
              : 0,
          })),
      }));
  } catch {
    return [];
  }
}

/** 写回该模型的动作链，返回落点绝对路径（前端展示「存到哪了」）。 */
export async function saveChains(modelPath: string, chains: MotionChain[]): Promise<string> {
  const file: ChainFile = { version: 1, model: baseName(modelPath), chains };
  return invoke<string>("write_pet_chains", { modelPath, json: JSON.stringify(file, null, 2) });
}

/** 一步的**实际时长**（秒）：`loopSec` 与硬停秒取小（硬停是上限保护）。 */
export function stepSeconds(s: ChainStep): number {
  const hard = s.stopSec > 0 ? s.stopSec : s.loopSec;
  return Math.min(s.loopSec, hard);
}

/** 整条链的时长（秒）。 */
export function chainSeconds(c: MotionChain): number {
  return c.steps.reduce((sum, s) => sum + stepSeconds(s), 0);
}
