// ── 桌宠（L1，2026-09-29）────────────────────────────────────────────
// 形态与三条裁决见 `docs/agent-feature-backlog.md` 的 **L1 / L1-A**：
// 独立透明置顶窗 + 形象由用户导入 + 安装包零第三方模型资产。
//
// **本插件是拓展插件**（盘上 `Modules\pet\`）。源码放在 `builtin/` 只是因为
// 官方磁盘插件的打包入口统一指向这里（见 `app/vite.plugins.config.ts` 的
// `pluginEntries`）；`builtin/index.ts` **不**注册它 —— 它必须能被卸载，
// 而「卸载 = 完全不存在于本应用」的判据是盘上那份目录在不在（`hasDiskPlugin`）。
//
// **两个窗口、两种界面**（这一条是本文件的结构主轴）：
//   · **控制台** —— 搜索「桌宠」直接打开的那个内嵌面板（普通插件面板）；
//   · **桌宠窗** —— 控制台里按「显示桌宠」后由宿主开出的独立悬浮窗，
//     形态由清单的 `window` 段声明（无标题栏 / 不进任务栏 / 禁缩放 / 置顶）。
// 宿主对两个窗口都调 `execute()`，所以插件必须自己分流：判据是**当前页的 URL
// 末段**（`plugin.html` = 桌宠窗，其余 = 控制台）。宿主没给「我在哪个窗」的接口，
// 而两个窗加载的本来就是不同的 HTML —— 这是最不容易漂移的一条判据。
//
// **为什么「穿透开关」必须放在控制台**：穿透开着时桌宠窗**收不到任何鼠标事件**
// （那正是它的用途）。开关若只放在桌宠窗的右键菜单里，用户一按下去就再也
// 关不掉了 —— 只能去杀进程。所以两件事分开：**桌宠窗负责「做」**
// （所有窗口操作只能由它自己发，命令拿的是调用方那个窗口），**控制台负责「说」**
// （写配置 + 广播一条 `pet-control`）。
//
// **形象来源 = 用户自备**，两种二选一（`model` 优先，见 `loadConfig`）：
//   · 图片（png / jpg / webp / gif）—— 一次 `convertFileSrc` 就够；
//   · **Live2D `*.model3.json`** —— 引擎那一层在 `./live2d.js`（Core 从
//     `<Modules>\pet\engine\` 注入、相对引用自己改写，两条硬障碍的结论见
//     `docs/agent-feature-backlog.md` 的 **L1**，改前先重读）。
// Lunac 只提供**引擎 + 导入通道**，模型一律用户自备（安装包零第三方模型资产，
// 三条裁决与版权四条线见 L1-A / L1-B）。

import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import type { Plugin, PluginResult } from "../registry";
import { t } from "../../i18n.js";
import { ensureCubismCore, dirName, joinPath, listExpressions, live2dEnginePath, MOTION_EXTRA_GROUP, PET_SCALE_MAX, PET_SCALE_MIN, readModelJson, validateModelFile, type Live2DHandle } from "./live2d";
import {
  ACTION_TYPES,
  collectMotionParamValues,
  defaultKeysForIndex,
  enumerateTargets,
  hotkeyFromVts,
  listMotionFiles,
  loadHotkeys,
  makeHotkey,
  readCdi3Names,
  readExpressionParams,
  readVtsHotkeys,
  readVtsParamIds,
  saveHotkeys,
  type ActionType,
  type Cdi3Names,
  type Hotkey,
  type HotkeyAction,
  type HotkeyTargets,
  type MotionFileRef,
} from "./pet-hotkeys";
import {
  chainSeconds,
  loadChains,
  makeChain,
  makeStep,
  saveChains,
  stepSeconds,
  type ChainStep,
  type MotionChain,
} from "./pet-chains";
import {
  emptyPetParamCache,
  loadPetParams,
  savePetParams,
} from "./pet-params";
import { attachCustomSelect, customSelectHtml, type CustomSelectOption } from "./custom-select";
import type { MotionProgress } from "./live2d";
import {
  ANIM_MAX_MS,
  RATE_DEFAULT,
  RATE_MAX,
  RATE_MIN,
  clampRate,
  normalizeReplyText,
  shouldAnimateDirectly,
  splitUnits,
} from "./pet-typewriter";
import {
  OUT_MIN_H,
  OUT_STEP,
  STAGE_GAP,
  STAGE_H,
  STAGE_W,
  WIN_BASE_H,
  WIN_W_DEFAULT,
  baseHeightFromWindow,
  clampOutTop,
  clampWindowSize,
  outCap,
  quantizeTopH,
  shouldResize,
  targetWindowHeight,
} from "./pet-window-size";

/** 桌宠「看一眼屏幕」的暂存载荷落点。与 `main.ts`（聊天窗那侧）**必须同值**：
 *  桌宠窗没有 `__lunac_*` 桥、也调不到聊天窗的 `startAIChat`，所以「那句话 + 图片路径」
 *  经 localStorage（同源各窗共享）交接，由聊天窗取走即删（take 语义）。 */
const LOOK_SCREEN_KEY = "lunac.pet.look-screen.v1";

/** 控制台 → 桌宠窗的单向指令（宿主不给这个，是插件自己约的一条广播）。
 *  tauri 的 `emit` 是**广播**，所以桌宠窗只认自己关心的几种 `type`，其余原样忽略。 */
const CTL_EVENT = "pet-control";

/** 「AI 对话框」收发契约（2026-10-09）。契约另一端在 `main.ts`（聊天窗那侧），
 *  桌宠窗**只按契约收发**、不自己拼请求：
 *  · `pet-chat-send`（**广播**）—— 桌宠发起的一句话，聊天窗 / 主窗负责复用 `startAIChat`；
 *  · `pet-chat-reply`（**定向发到 `plugin-pet`**）—— 「桌宠发起的那一轮」的**最终回复**
 *    （不含思考过程；thinking 只活在桌宠窗的气泡占位里）。 */
const CHAT_SEND_EVENT = "pet-chat-send";
const CHAT_REPLY_EVENT = "pet-chat-reply";
/** `pet-chat-stop`（**广播**，2026-10-11）—— 用户在桌宠输入行点「中止」时发的一条。
 *  与上面两条同属这份契约，另一端仍在 `main.ts`（**常量必须同值**）。中止的**真正收尾**
 *  落在聊天窗（那一轮请求在它手里）：复用它既有的 `stopAIChat()`（内部 `finalizeInterruptedTurn`
 *  → `replyPetTurn` 定向回 `pet-chat-reply`，桌宠气泡随之显示「已中断」）。桌宠窗**不自己停**。 */
const CHAT_STOP_EVENT = "pet-chat-stop";

/** **口型包络**的下发事件（2026-10-10）。契约另一端在宿主的 `src/tts.rs`（`MOUTH_EVENT`，
 *  **定向** `emit_to("plugin-pet")`）：宿主在**开始朗读前**把这条 WAV 的「张嘴幅度」逐格发来，
 *  桌宠窗按同一条时间线写 `setMouthOpen`。**必须与 Rust 侧同名**。 */
const MOUTH_EVENT = "pet-tts-mouth";

/** **「语音播完」信号**（2026-10-13）。契约另一端在宿主的 `src/tts.rs`（`TTS_DONE_EVENT`，
 *  **定向** `emit_to("plugin-pet")`）：宿主在**这条 WAV 播完（或判定无需出声 / 合成失败）之后**
 *  发一条。桌宠窗据此把气泡自动收起的起算推迟到「语音播完」—— 见 `scheduleOutputAutoHide`。
 *  `tts` 关着时宿主根本不会被调用，桌宠窗自己按原 ≈6s 起算（见 CHAT_REPLY_EVENT 处理处）。
 *  **必须与 Rust 侧同名**。 */
const TTS_DONE_EVENT = "pet-tts-done";

/** 「主动反应」（2026-10-09）：桌宠**定时主动**对用户说一句话（复用 `sendToAI`）。
 *  **成本红线（用户口径）**：这条链路**持续烧 token** ⇒ 默认**关**，且「最小间隔 + 每日上限」
 *  两道闸；桌宠窗隐藏 / 最小化时必须停（别在看不见时偷偷烧 token）。
 *  间隔与每日上限都是**可键入的数字框**（2026-10-09 用户口径：间隔要能调小 / 自定义）。 */
/** 最小间隔的可键入范围（分钟）。**下限 1 分钟** —— 用户要求「调小 / 可自定义」，但 0 / 负值无意义。 */
const PROACTIVE_INTERVAL_MIN = 1;
const PROACTIVE_INTERVAL_MAX = 1440;
/** 每日上限的可键入范围（越界回落默认，见 loadConfig）。 */
const PROACTIVE_CAP_MIN = 1;
const PROACTIVE_CAP_MAX = 200;
/** 主动反应的回复到达后，气泡**自动展开**的停留时长（之后自动收起；用户本来开着的不收）。 */
const PROACTIVE_BOX_MS = 6000;
/** 一轮的最大等待：正常收尾由 `pet-chat-reply` 触发，这里只是兜底 —— 聊天窗没开 / 异常
 *  时不至于让桌宠永远停在「思考中」、也不再排下一轮。 */
const PROACTIVE_ROUND_TIMEOUT_MS = 90_000;

/** 「自动抓屏」间隔的可键入范围（分钟）。主动反应的一个**显式 opt-in 子开关**（默认关）。 */
const AUTOSHOT_MIN_MIN = 1;
const AUTOSHOT_MIN_MAX = 240;
/** 「屏幕变化触发」的**轮询间隔**范围（秒）与**灵敏度阈值**范围（16×9 灰度的平均绝对差，0..255，
 *  **越大越不敏感**）。两者都只在用户显式开启该子开关后才起作用。 */
const WATCH_SEC_MIN = 5;
const WATCH_SEC_MAX = 600;
const WATCH_DIFF_MIN = 1;
const WATCH_DIFF_MAX = 120;

/** 桌宠窗的三段布局（逻辑像素）与整体尺寸的**唯一真相源在 `./pet-window-size`**：
 *  缺省下 输出带（下限）75 + 模型区（参考）400 + 输入带 35 = **510**，三条**紧贴、无空隙**。
 *  ⚠️ 2026-10-13 第二轮起：模型区**不再定尺 400**，而是用 flex **跟随窗口剩余空间**
 *  （= 窗高 − 输出带 − 输入带）；`STAGE_W` / `STAGE_H` 改为**参考尺寸**（缺省窗口下的 300×400），
 *  供模型基准缩放（只缩不放，见 live2d-engine 的 fit）与基准窗高换算使用。
 *  `.pet-top` 的 `flex-basis` / `.pet-chat-output` 的高度由 CSS 变量 `--pet-top-h` 驱动（见 PET_CSS），
 *  数值初值取 `OUT_MIN_H`（75）；输出框随内容封顶后由 JS 改写它（见 applyOutHeight）。
 *  ⚠️ 2026-10-13 第三轮：输出带的上限**动态**取 `outCap(窗口基准高)`（窗矮则上限降，**绝不把模型区
 *  压到 0**），且最小窗高抬到 `WIN_H_MIN = OUT_MIN_H + IN_H + STAGE_MIN_H`（= 250）—— 三处口径同源。
 *  模型区的左右留白与图片定尺走 `--pet-stage-gap` / `--pet-stage-ref-w` / `--pet-stage-ref-h`
 *  （由 `attachPetWindow` 从上面这些常量注入，避免在 CSS 里再写一份）。
 *  ⚠️ 数值与 PET_CSS 里 `.pet-top` / `.pet-bottom` / `.pet-stage` **必须一致**，改一处要同步。 */

/** 配置落点。用 localStorage 而不是宿主命令：宿主**没有**通用的文本读写命令
 *  （只有 `read_tool_file` / `read_skill_file` 那几条面向工具与技能的），
 *  而两个窗口同源（`tauri://localhost` 或 dev 的 `5173`）⇒ 共享同一份 localStorage。
 *  **只用自己的 key 前缀**，不去碰宿主与别的插件的东西。 */
const CFG_KEY = "lunac.pet.v1";

/** 桌宠窗自己那一个窗口的 label（宿主按 `plugin-<id>` 命名）。 */
const PET_LABEL = "plugin-pet";

/** 滚轮每格的缩放倍率（`>1` 放大 / 其倒数缩小）。 */
const WHEEL_ZOOM_STEP = 1.1;

/** 「模型调试区」那组标准参数。范围按参数语义给：角度类**双向** -100..100，其它 0..100。
 *  `labelKey` 是 i18n 键（**五语言全给**，见 `i18n.ts`）。这几个 id 是 Cubism 的约定名，
 *  绝大多数模型都有；个别模型没有某个 id 时，引擎会静默忽略（不报错，也不动别的）。 */
const DEBUG_PARAMS: { id: string; min: number; max: number; labelKey: string }[] = [
  { id: "ParamAngleX", min: -100, max: 100, labelKey: "pet.param_head_x" },
  { id: "ParamAngleY", min: -100, max: 100, labelKey: "pet.param_head_y" },
  { id: "ParamAngleZ", min: -100, max: 100, labelKey: "pet.param_head_z" },
  { id: "ParamEyeLOpen", min: 0, max: 100, labelKey: "pet.param_eye_l" },
  { id: "ParamEyeROpen", min: 0, max: 100, labelKey: "pet.param_eye_r" },
  { id: "ParamMouthOpenY", min: 0, max: 100, labelKey: "pet.param_mouth" },
  { id: "ParamBodyAngleX", min: -100, max: 100, labelKey: "pet.param_body_x" },
  { id: "ParamBodyAngleZ", min: -100, max: 100, labelKey: "pet.param_body_z" },
  { id: "ParamBreath", min: 0, max: 100, labelKey: "pet.param_breath" },
];

interface PetConfig {
  /** 形象图片的**绝对路径**（用户经文件对话框选的）。空 = 没有图片形象。 */
  image: string;
  /** Live2D 模型的 `*.model3.json` **绝对路径**（用户经文件对话框选的）。**非空时优先于图片**。 */
  model: string;
  /** 形象的**整体缩放系数**（1 = 按参考模型区尺寸贴合，>1 放大 / <1 缩小）。它乘在「基准缩放」
   *  （按参考尺寸 contain）上，故与模型区当前大小无关 —— 模型区变大只是多出空间、不改形象大小。 */
  scale: number;
  /** 形象相对**基准位置**（窗口底边中心）的像素偏移。模型与图片两种模式都用它：
   *  Alt+拖动与滚轮锚点缩放都会改它，随 `scale` 一起持久化。 */
  offsetX: number;
  offsetY: number;
  /** **整体旋转（角度制，−180..180）**，绕模型中心转。模型模式的第三个变换维度
   *  （与 `scale` / `offsetX/Y` 同级、同持久化口径）；图片模式不用它。 */
  rotation: number;
  /** 鼠标穿透。**默认关**：开着时桌宠窗不吃任何鼠标事件，拖动与右键菜单都会失效。 */
  clickThrough: boolean;
  /** 各模型**当前生效的表情 id**（模型绝对路径 → 表情 id）。**空串/缺省 = 无表情（已还原）**。
   *  按模型路径分键：换模型不会清掉别的模型的记录，切回旧模型时恢复它自己的。
   *  设置窗与桌宠窗同源 localStorage，两边都读这一份 —— 重开设置窗、重启应用后高亮 / 表情仍在。 */
  expressions: Record<string, string>;
  /** 各模型**模型调试区**那组参数的持久化值（模型绝对路径 → { 参数 id → 数值 }）。
   *  **缺省 / 空 = 用引擎默认（0）**。与 `expressions` 同一口径：按模型路径分键，换模型不清别的
   *  模型的记录，切回旧模型时恢复它自己的。设置窗拖动 / 键入即写回；桌宠窗装载模型后自动应用。 */
  params: Record<string, Record<string, number>>;
  /** 各模型**参数类型的手动覆盖**（模型绝对路径 → { 参数 id → "switch" | "range" }）。
   *  自动判定（区间 0..1 + 实际只取 0/1）不可能 100% 准 ⇒ 给人一个出口；缺省 = 用自动判定。 */
  paramKinds: Record<string, Record<string, "switch" | "range">>;
  /** 各模型勾为**循环播放**的动作（模型绝对路径 → 动作文件名词干数组）。**空 = 没有**。
   *  桌宠窗装载时把它们交给引擎（`MountOptions.idleMotions`）；引擎在动作队列空时按顺序轮播，
   *  并用各文件自己的 `Meta.Duration` 做超时兜底。
   *  ⚠️ **语义（2026-10-10 用户更正）**：与「待机」无关 —— 就是「让模型循环播放这些动作」，
   *  当初是按待机动作设计的。**键名 `idleMotions` 属历史遗留**（改名会让已存配置全部失效），
   *  对外文案一律叫「循环播放」。 */
  idleMotions: Record<string, string[]>;
  /** **各模型当前开着的那批热键 id**（模型绝对路径 → 热键 id 数组）。热键「测试」是状态按钮，
   *  这些状态要**跨「隐藏 / 显示模型」与「关闭 / 重开插件」保持**（用户 2026-10-10 口径），
   *  **只有换模型才重置**（换模型会重新导入热键、id 全变 ⇒ 旧 id 自然对不上）。
   *  `motion` 类**不记**：它没有持续状态，记下来会让每次挂载都重播一次动作。 */
  hotkeyStates: Record<string, string[]>;
  /** 「主动反应」配置（含每日计数字段）。见 `ProactiveCfg`。 */
  proactive: ProactiveCfg;
  /** **本机离线语音朗读**开关（2026-10-10）。**默认 true** —— 用户明确要「读出其输出」；
   *  关掉后收到 `pet-chat-reply` 不再朗读。旧配置缺这个字段时回落 true（见 `loadConfig`）。 */
  tts: boolean;
  /** **打字速度**（单位/秒，范围 `RATE_MIN..RATE_MAX`，默认 `RATE_DEFAULT`）：**只**驱动输出框
   *  逐字揭示动画的每秒单位数（两个窗口同源 localStorage ⇒ 设置窗改、桌宠窗即时读到）。
   *  **与语音语速相互独立**（2026-10-12 用户口径）：语音语速 / 音量分别落在宿主
   *  `PetVoiceConfig.speed` / `volume`，不再由本字段折算。
   *  旧配置缺这个字段时回落默认（见 `loadConfig`）。 */
  speechRate: number;
  /** **窗口宽**（逻辑像素，2026-10-13）。用户在桌宠窗里拖边缘 / 拖窗上缩放手柄改过就记这里，
   *  重启后按它开窗（再 clamp 到合法区间，见 `clampWindowSize`）。缺字段回落 `WIN_W_DEFAULT`。 */
  winW: number;
  /** **窗口基准高**（逻辑像素，2026-10-13）：**无内容**（输出带回到预留下限 75）时窗口的高度。
   *  输出框变高时窗口高 = 基准高 + (输出带高 − 75)（模型区 / 输入带不参与、故模型不位移）。
   *  缺字段回落 `WIN_BASE_H`（= 510）。 */
  winH: number;
}

/** 「主动反应」配置。计数字段（`date` / `used`）也落在这里 —— **与配置共用同一个 localStorage
 *  键**（`CFG_KEY`），**不引入第二个存储**（用户口径）。 */
interface ProactiveCfg {
  /** **默认关**：这条链路持续烧 token，必须用户显式同意才开。 */
  enabled: boolean;
  /** 两次主动反应之间的**最小间隔**（分钟），范围 `PROACTIVE_INTERVAL_MIN..PROACTIVE_INTERVAL_MAX`。 */
  intervalMin: number;
  /** 每个自然日最多触发几次（越界回落默认，范围 `PROACTIVE_CAP_MIN..PROACTIVE_CAP_MAX`）。 */
  dailyCap: number;
  /** 「今日已用」计数对应的**自然日**（本地 `YYYY-MM-DD`）。≠ 今天 ⇒ 计数作废（跨日归零）。 */
  date: string;
  /** 今天已触发的主动反应次数。 */
  used: number;
  /** **「自动抓屏」子开关**（2026-10-10，默认关）：按 `autoShotMin` 分钟抓一屏交给 AI 看。
   *  **隐私红线**：只有用户显式开启 + 桌宠可见时才抓（隐藏即停），且计入每日上限。 */
  autoShot: boolean;
  /** 自动抓屏的间隔（分钟），范围 `AUTOSHOT_MIN_MIN..AUTOSHOT_MIN_MAX`。 */
  autoShotMin: number;
  /** **「屏幕变化触发」子开关**（2026-10-10，默认关）：每 `screenWatchSec` 秒抓一张 16×9 指纹
   *  （不落盘）与上一张比，平均绝对差超过 `screenDiff` 就当画面变了、主动说一句。
   *  **隐私红线**同 `autoShot`。 */
  screenWatch: boolean;
  /** 屏幕变化检测的轮询间隔（秒），范围 `WATCH_SEC_MIN..WATCH_SEC_MAX`。 */
  screenWatchSec: number;
  /** 变化判定的灵敏度阈值（16×9 灰度平均绝对差），范围 `WATCH_DIFF_MIN..WATCH_DIFF_MAX`。 */
  screenDiff: number;
}

const DEFAULT_CFG: PetConfig = {
  image: "", model: "", scale: 1, offsetX: 0, offsetY: 0, rotation: 0, clickThrough: false, expressions: {}, params: {}, paramKinds: {}, idleMotions: {}, hotkeyStates: {},
  proactive: { enabled: false, intervalMin: 30, dailyCap: 20, date: "", used: 0, autoShot: false, autoShotMin: 5, screenWatch: false, screenWatchSec: 20, screenDiff: 12 },
  tts: true,
  speechRate: RATE_DEFAULT,
  winW: WIN_W_DEFAULT,
  winH: WIN_BASE_H,
};

function loadConfig(): PetConfig {
  try {
    const raw = localStorage.getItem(CFG_KEY);
    if (!raw) return { ...DEFAULT_CFG };
    const o = JSON.parse(raw) as Partial<PetConfig>;
    // 表情记录：只收「非空字符串键 → 字符串值」，坏值直接丢掉（不让一个坏条目拖垮整份配置）
    const expressions: Record<string, string> = {};
    if (o.expressions && typeof o.expressions === "object") {
      for (const [k, v] of Object.entries(o.expressions)) {
        if (k && typeof v === "string") expressions[k] = v;
      }
    }
    // 调试参数记录：只收「非空键 → 数值」，坏条目逐个丢掉（不让一个坏值拖垮整份配置）。
    const params: Record<string, Record<string, number>> = {};
    if (o.params && typeof o.params === "object") {
      for (const [model, rec] of Object.entries(o.params)) {
        if (!model || !rec || typeof rec !== "object") continue;
        const vals: Record<string, number> = {};
        for (const [id, v] of Object.entries(rec)) {
          if (id && typeof v === "number" && Number.isFinite(v)) vals[id] = v;
        }
        if (Object.keys(vals).length) params[model] = vals;
      }
    }
    // 参数类型的手动覆盖：只收 "switch" / "range" 两个合法值，坏条目逐个丢掉。
    const paramKinds: Record<string, Record<string, "switch" | "range">> = {};
    const pkRaw = (o as { paramKinds?: unknown }).paramKinds;
    if (pkRaw && typeof pkRaw === "object") {
      for (const [model, rec] of Object.entries(pkRaw as Record<string, unknown>)) {
        if (!model || !rec || typeof rec !== "object") continue;
        const kinds: Record<string, "switch" | "range"> = {};
        for (const [id, v] of Object.entries(rec as Record<string, unknown>)) {
          if (id && (v === "switch" || v === "range")) kinds[id] = v;
        }
        if (Object.keys(kinds).length) paramKinds[model] = kinds;
      }
    }
    // 待机循环动作：模型路径 → 文件名词干数组，只收非空字符串（坏条目逐个丢掉）。
    const idleMotions: Record<string, string[]> = {};
    const imRaw = (o as { idleMotions?: unknown }).idleMotions;
    if (imRaw && typeof imRaw === "object") {
      for (const [model, arr] of Object.entries(imRaw as Record<string, unknown>)) {
        if (!model || !Array.isArray(arr)) continue;
        const names = arr.filter((x): x is string => typeof x === "string" && x.trim().length > 0);
        if (names.length) idleMotions[model] = names;
      }
    }
    // 开着的那批热键 id：同一口径（模型路径 → 非空字符串数组，坏条目逐个丢掉）。
    const hotkeyStates: Record<string, string[]> = {};
    const hsRaw = (o as { hotkeyStates?: unknown }).hotkeyStates;
    if (hsRaw && typeof hsRaw === "object") {
      for (const [model, arr] of Object.entries(hsRaw as Record<string, unknown>)) {
        if (!model || !Array.isArray(arr)) continue;
        const ids = arr.filter((x): x is string => typeof x === "string" && x.trim().length > 0);
        if (ids.length) hotkeyStates[model] = ids;
      }
    }
    // 主动反应：逐字段校验，越界回落默认（`date` / `used` 是每日计数字段，只收合法值）。
    const pd = DEFAULT_CFG.proactive;
    const pRaw = (o as { proactive?: unknown }).proactive as Partial<ProactiveCfg> | undefined;
    const proactive: ProactiveCfg =
      pRaw && typeof pRaw === "object"
        ? {
            enabled: pRaw.enabled === true,
            intervalMin:
              typeof pRaw.intervalMin === "number" && Number.isFinite(pRaw.intervalMin)
                ? Math.min(PROACTIVE_INTERVAL_MAX, Math.max(PROACTIVE_INTERVAL_MIN, Math.round(pRaw.intervalMin)))
                : pd.intervalMin,
            dailyCap:
              typeof pRaw.dailyCap === "number" && Number.isFinite(pRaw.dailyCap)
                ? Math.min(PROACTIVE_CAP_MAX, Math.max(PROACTIVE_CAP_MIN, Math.round(pRaw.dailyCap)))
                : pd.dailyCap,
            date: typeof pRaw.date === "string" ? pRaw.date : "",
            used: typeof pRaw.used === "number" && Number.isFinite(pRaw.used) && pRaw.used > 0 ? Math.floor(pRaw.used) : 0,
            // 「自动抓屏」/「屏幕变化触发」两个子开关：**只有显式 true 才算开**（默认关，隐私红线）。
            autoShot: pRaw.autoShot === true,
            autoShotMin:
              typeof pRaw.autoShotMin === "number" && Number.isFinite(pRaw.autoShotMin)
                ? Math.min(AUTOSHOT_MIN_MAX, Math.max(AUTOSHOT_MIN_MIN, Math.round(pRaw.autoShotMin)))
                : pd.autoShotMin,
            screenWatch: pRaw.screenWatch === true,
            screenWatchSec:
              typeof pRaw.screenWatchSec === "number" && Number.isFinite(pRaw.screenWatchSec)
                ? Math.min(WATCH_SEC_MAX, Math.max(WATCH_SEC_MIN, Math.round(pRaw.screenWatchSec)))
                : pd.screenWatchSec,
            screenDiff:
              typeof pRaw.screenDiff === "number" && Number.isFinite(pRaw.screenDiff)
                ? Math.min(WATCH_DIFF_MAX, Math.max(WATCH_DIFF_MIN, Math.round(pRaw.screenDiff)))
                : pd.screenDiff,
          }
        : { ...pd };
    // 窗口尺寸记忆：宽 / 高一起过一次 clamp（越界 / 非法都夹到合法区间）。
    const win = clampWindowSize(
      typeof o.winW === "number" ? o.winW : WIN_W_DEFAULT,
      typeof o.winH === "number" ? o.winH : WIN_BASE_H,
    );
    return {
      image: typeof o.image === "string" ? o.image : "",
      model: typeof o.model === "string" ? o.model : "",
      scale: typeof o.scale === "number" && o.scale >= PET_SCALE_MIN && o.scale <= PET_SCALE_MAX ? o.scale : 1,
      offsetX: typeof o.offsetX === "number" && Number.isFinite(o.offsetX) ? o.offsetX : 0,
      offsetY: typeof o.offsetY === "number" && Number.isFinite(o.offsetY) ? o.offsetY : 0,
      rotation:
        typeof o.rotation === "number" && Number.isFinite(o.rotation)
          ? Math.max(-180, Math.min(180, o.rotation))
          : 0,
      clickThrough: o.clickThrough === true,
      expressions,
      params,
      paramKinds,
      idleMotions,
      hotkeyStates,
      proactive,
      // 语音朗读：**默认 true**（老配置缺这个字段 ⇒ 回落到 true）。
      tts: typeof o.tts === "boolean" ? o.tts : true,
      // 速率：越界 / 非法回落默认（`clampRate` 同一套夹取口径）。
      speechRate: clampRate(typeof o.speechRate === "number" ? o.speechRate : RATE_DEFAULT),
      // 窗口尺寸记忆：越界 / 非法在 `clampWindowSize` 里夹到合法区间（打开时还会再 clamp 一次）。
      winW: win.w,
      winH: win.h,
    };
  } catch {
    // 配置坏了不该让桌宠开不出来 —— 回到默认值，并把坏值覆盖掉
    return { ...DEFAULT_CFG };
  }
}

function saveConfig(cfg: PetConfig): void {
  try {
    localStorage.setItem(CFG_KEY, JSON.stringify(cfg));
  } catch {
    /* 无痕模式 / 配额满：配置存不下不影响本次使用 */
  }
}

/** 本地「自然日」字符串（`YYYY-MM-DD`）。主动反应的每日计数按它归零。 */
function todayStr(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

/** 读「今日已用」次数：记录的日期不是今天 ⇒ 视为 0（**跨日自动归零**，不必写盘）。 */
function proactiveUsedToday(): number {
  const pr = loadConfig().proactive;
  if (pr.date !== todayStr()) return 0;
  return pr.used > 0 ? pr.used : 0;
}

/** 「今日已用」+1 并写回（跨日先把计数从 0 起算）。返回新值。 */
function bumpProactiveUsed(): number {
  const c = loadConfig();
  const today = todayStr();
  const used = (c.proactive.date === today ? c.proactive.used || 0 : 0) + 1;
  saveConfig({ ...c, proactive: { ...c.proactive, date: today, used } });
  return used;
}

/** 两组等长指纹的**平均绝对差**（0..255）—— 「屏幕变化触发」的粗判依据（16×9 灰度）。
 *  长度不等时返回 0（视为不可比，别误判为「变了」）。 */
function meanAbsDiff(a: number[], b: number[]): number {
  if (a.length !== b.length || a.length === 0) return 0;
  let sum = 0;
  for (let i = 0; i < a.length; i++) sum += Math.abs(a[i] - b[i]);
  return sum / a.length;
}

/** 距离**下一个自然日零点（+5s）**的毫秒数 —— 「触顶后跨日自愈」的重排统一用它。 */
function msUntilNextMidnight(): number {
  const now = new Date();
  const next = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1, 0, 0, 5);
  return Math.max(1000, next.getTime() - Date.now());
}

function esc(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

/** 「鼠标穿透」**切换按钮**的文案（开 / 关两态）。设置窗与桌宠右键菜单**共用同一份**，
 *  保证两处显示永远一致（用户 2026-10-09 口径：桌宠设置与模型之间的所有参数都要**对齐**）。 */
function throughLabel(on: boolean): string {
  return on ? t("pet.through_on") : t("pet.through_off");
}

/** 桌宠窗 / 设置窗的报错上报（`attachConsole` 里那份 `report` 在函数作用域内，这里要一份模块级的）。 */
function reportPet(what: string, e: unknown): void {
  void invoke("log_frontend", {
    level: "error",
    message: `[pet window] ${what}: ${e instanceof Error ? e.message : String(e)}`,
  }).catch(() => {});
}

/** 当前窗口的 label（宿主按 `plugin-<id>[-<key>]` 命名）。取不到当空串。 */
function winLabel(): string {
  try {
    return getCurrentWindow().label || "";
  } catch {
    return "";
  }
}

/** 当前页是不是**桌宠窗**（label `plugin-pet`）。
 *  ⚠️ **不能只看 `plugin.html`** —— 桌宠的**设置窗**（`plugin-pet-settings`）加载的是
 *  同一份 `plugin.html`，只看路径名会把设置窗误判成桌宠窗（2026-10-09 补）。 */
function inPetWindow(): boolean {
  return location.pathname.endsWith("plugin.html") && winLabel() === PET_LABEL;
}

/** 当前页是不是桌宠的**设置窗**（宿主里那一扇 keyed 窗）。 */
function inSettingsWindow(): boolean {
  return winLabel() === `${PET_LABEL}-settings`;
}

// ── Live2D 引擎的**运行时装载**（2026-09-30）──────────────────────────
// 引擎是插件目录里的**独立文件**（`<Modules>\pet\engine\live2d-engine.js`，里面内联了
// pixi 与 `pixi-live2d-display`），必须**等 Cubism Core 就绪之后**才 import —— 那个库在
// **模块求值时**就检查 `window.Live2DCubismCore`。「为什么不能同包内惰性 import」的完整
// 理由见 `./live2d.ts` 的文件头。
// 用**绝对 URL**（`convertFileSrc`）而不是相对 specifier：asset 协议把整条绝对路径
// percent-encode 进 URL 最后一段，相对解析必然落到协议根、不是插件目录。
type EngineModule = typeof import("./live2d-engine");
let enginePromise: Promise<EngineModule> | null = null;

function loadEngine(): Promise<EngineModule> {
  if (!enginePromise) {
    enginePromise = (async () => {
      await ensureCubismCore();
      const url = convertFileSrc(await live2dEnginePath());
      return (await import(/* @vite-ignore */ url)) as EngineModule;
    })().catch((e: unknown) => {
      enginePromise = null; // 失败不缓存：用户补上引擎文件 / 重装插件之后要能再试
      throw e instanceof Error ? e : new Error(String(e));
    });
  }
  return enginePromise;
}

// ══════════════════════════════════════════════════════════════════
//  控制台（主窗口里的普通面板）
// ══════════════════════════════════════════════════════════════════

/** 开关按钮的文案（「xxx：开 / 关」）。**必须是模块级函数**：`onClick` 里局部变量 `t`
 *  是 `ev.target`，会把 i18n 的 `t` 遮蔽 —— 在里面直接调 `t(...)` 会 TS2349。
 *  写成函数则**在调用时**去取 i18n（那时早已初始化）。 */
function toggleLabel(key: string, on: boolean): string {
  return `${t(key)}：${on ? "开" : "关"}`;
}

/** 开关按钮的纯两态文案（只显示「开 / 关」）。**必须是模块级函数**（同 `toggleLabel` 的
 *  `t` 遮蔽理由）。2026-10-10 用户口径：主动反应那一枚按钮原来写「让桌宠主动跟我说话：关」，
 *  太长 —— 名字改由左侧 `pet-label` 承担，按钮本身只留「关 / 开」。 */
function onOffText(on: boolean): string {
  return on ? "开" : "关";
}

/** 「主动反应」区块（设置窗）。**成本红线**：默认关 + 最小间隔 + 每日上限 + 今日已用回显。
 *  2026-10-10 追加两个**显式 opt-in** 子开关（隐私红线：默认关、桌宠不可见即停、计入每日上限）：
 *  「自动抓屏」（周期性抓屏给 AI 看）与「屏幕变化触发」（指纹比对画面变化后主动说一句）。
 *  原来那段「注意：会持续消耗 token…」的整段说明按用户口径**删除**。 */
function proactiveBlock(cfg: PetConfig): string {
  const pr = cfg.proactive;
  const used = proactiveUsedToday();
  const base = t("pet.proactive_usage", { used: String(used), cap: String(pr.dailyCap) });
  const usage = used >= pr.dailyCap ? `${base} · ${t("pet.proactive_reached")}` : base;
  return `
  <div class="pet-proactive" id="pet-proactive">
    <div class="pet-debug-title">${t("pet.proactive_title")}</div>
    <div class="pet-row">
      <span class="pet-label">${t("pet.proactive_enable")}</span>
      <button type="button" id="pet-proactive-enabled" class="pet-btn pet-btn-toggle${pr.enabled ? " on" : ""}" aria-pressed="${pr.enabled ? "true" : "false"}">${esc(onOffText(pr.enabled))}</button>
    </div>
    <div class="pet-row">
      <span class="pet-label">${t("pet.proactive_interval")}</span>
      <input type="text" id="pet-proactive-interval" class="pet-param-num" inputmode="numeric" autocomplete="off" spellcheck="false" value="${pr.intervalMin}" aria-label="${esc(t("pet.proactive_interval"))}">
      <span class="pet-val">${t("pet.proactive_minute")}</span>
    </div>
    <div class="pet-row">
      <span class="pet-label">${t("pet.proactive_cap")}</span>
      <input type="text" id="pet-proactive-cap" class="pet-param-num" inputmode="numeric" autocomplete="off" spellcheck="false" value="${pr.dailyCap}" aria-label="${esc(t("pet.proactive_cap"))}">
      <span class="pet-val">${t("pet.proactive_cap_unit")}</span>
    </div>
    <div class="pet-row">
      <span class="pet-label">${t("pet.proactive_autoshot")}</span>
      <button type="button" id="pet-autoshot-enabled" class="pet-btn pet-btn-toggle${pr.autoShot ? " on" : ""}" aria-pressed="${pr.autoShot ? "true" : "false"}">${esc(onOffText(pr.autoShot))}</button>
      <input type="text" id="pet-autoshot-min" class="pet-param-num" inputmode="numeric" autocomplete="off" spellcheck="false" value="${pr.autoShotMin}" aria-label="${esc(t("pet.proactive_autoshot"))}">
      <span class="pet-val">${t("pet.proactive_minute")}</span>
    </div>
    <div class="pet-row">
      <span class="pet-label">${t("pet.proactive_screenwatch")}</span>
      <button type="button" id="pet-screenwatch-enabled" class="pet-btn pet-btn-toggle${pr.screenWatch ? " on" : ""}" aria-pressed="${pr.screenWatch ? "true" : "false"}">${esc(onOffText(pr.screenWatch))}</button>
      <input type="text" id="pet-watch-sec" class="pet-param-num" inputmode="numeric" autocomplete="off" spellcheck="false" value="${pr.screenWatchSec}" aria-label="${esc(t("pet.proactive_watch_sec"))}">
      <span class="pet-val">${t("pet.proactive_second")}</span>
    </div>
    <div class="pet-row">
      <span class="pet-label">${t("pet.proactive_sensitivity")}</span>
      <input type="text" id="pet-watch-diff" class="pet-param-num" inputmode="numeric" autocomplete="off" spellcheck="false" value="${pr.screenDiff}" aria-label="${esc(t("pet.proactive_sensitivity"))}">
    </div>
    <div class="pet-note" id="pet-proactive-hint">${esc(t("pet.proactive_shot_hint"))}</div>
    <div class="pet-note" id="pet-proactive-usage">${esc(usage)}</div>
  </div>`;
}

/** 「语音语速」拉条的范围 —— **必须落在宿主 clamp 区间 `0.5..=2.0` 之内**
 *  （`tts_zipvoice::SPEED_MIN..=SPEED_MAX`，`PetVoiceConfig.clamped_speed()`）；映射为 1:1（拉条值 = 宿主 speed）。 */
const VOICE_SPEED_MIN = 0.5;
const VOICE_SPEED_MAX = 2.0;
const VOICE_SPEED_DEFAULT = 1.0;
/** 「语音音量」拉条的范围 —— 0 = 静音、1 = 原始（映射 1:1；与 `tts_zipvoice::VOLUME_MIN..=VOLUME_MAX` 一致）。
 *  上限取 1 而非 2：rodio 音量是对每条样本的乘数，>1 会削顶失真。 */
const VOICE_VOLUME_MIN = 0.0;
const VOICE_VOLUME_MAX = 1.0;
const VOICE_VOLUME_DEFAULT = 1.0;

/** 语音语速越界 / 非法 → 夹到 `[VOICE_SPEED_MIN, VOICE_SPEED_MAX]`（保留两位小数）。 */
function clampVoiceSpeed(v: number): number {
  if (typeof v !== "number" || !Number.isFinite(v)) return VOICE_SPEED_DEFAULT;
  return Math.min(VOICE_SPEED_MAX, Math.max(VOICE_SPEED_MIN, Math.round(v * 100) / 100));
}

/** 语音音量越界 / 非法 → 夹到 `[VOICE_VOLUME_MIN, VOICE_VOLUME_MAX]`（保留两位小数）。 */
function clampVoiceVolume(v: number): number {
  if (typeof v !== "number" || !Number.isFinite(v)) return VOICE_VOLUME_DEFAULT;
  return Math.min(VOICE_VOLUME_MAX, Math.max(VOICE_VOLUME_MIN, Math.round(v * 100) / 100));
}

/** 宿主 `reason_for` 会返回的**全部**机器可读 key（见 `tts_zipvoice::reason_for`）。
 *  前端只把这些翻成文案（`pet.tts_reason_<key>`）；其余（不该出现）原样显示，绝不吞掉。 */
const PET_VOICE_REASONS = ["disabled", "no_ref_audio", "no_ref_text", "ref_missing", "ref_out_of_spec", "assets_missing"];

/** 「语音朗读」开关区块（设置窗，2026-10-10；2026-10-12 加参考音色导入入口）。
 *  **默认开** —— 用户要「读出其输出」；关掉后桌宠窗收到最终回复也不再朗读。
 *  合成与出声都在宿主（WebView 放不出声音，见 archive ai-spec §4.6），**引擎是 ZipVoice 零样本克隆**：
 *   · 用**你导入的参考音色**朗读（参考音频 + 那句参考文本写进 `config\pet-voice.json`）。
 *  **静默语义**：配置不齐 / 资产缺失 / 合成失败时**不朗读、不弹错**（见 src/tts.rs 文件头）。
 *  **三条拉条相互独立**（用户 2026-10-12 口径）：打字速度（本插件 `PetConfig.speechRate`，只驱动
 *  逐字动画）/ 语音语速（宿主 `PetVoiceConfig.speed`）/ 语音音量（宿主 `PetVoiceConfig.volume`）。 */
function ttsBlock(cfg: PetConfig): string {
  return `
  <div class="pet-tts" id="pet-tts">
    <div class="pet-debug-title">${t("pet.tts_title")}</div>
    <div class="pet-row">
      <button type="button" id="pet-tts-enabled" class="pet-btn pet-btn-toggle${cfg.tts ? " on" : ""}" aria-pressed="${cfg.tts ? "true" : "false"}">${esc(toggleLabel("pet.tts_enable", cfg.tts))}</button>
    </div>
    <div class="pet-row">
      <span class="pet-label">${esc(t("pet.tts_rate_label"))}</span>
      <input type="range" id="pet-tts-rate" class="pet-range" min="${RATE_MIN}" max="${RATE_MAX}" step="0.5" value="${cfg.speechRate}" aria-label="${esc(t("pet.tts_rate_label"))}">
      <span class="pet-val" id="pet-tts-rate-val">${cfg.speechRate}</span>
    </div>
    <div class="pet-row">
      <span class="pet-label">${esc(t("pet.tts_speed_label"))}</span>
      <input type="range" id="pet-tts-speed" class="pet-range" min="${VOICE_SPEED_MIN}" max="${VOICE_SPEED_MAX}" step="0.1" value="${VOICE_SPEED_DEFAULT}" aria-label="${esc(t("pet.tts_speed_label"))}">
      <span class="pet-val" id="pet-tts-speed-val">${VOICE_SPEED_DEFAULT.toFixed(1)}×</span>
    </div>
    <div class="pet-row">
      <span class="pet-label">${esc(t("pet.tts_volume_label"))}</span>
      <input type="range" id="pet-tts-volume" class="pet-range" min="${VOICE_VOLUME_MIN}" max="${VOICE_VOLUME_MAX}" step="0.05" value="${VOICE_VOLUME_DEFAULT}" aria-label="${esc(t("pet.tts_volume_label"))}">
      <span class="pet-val" id="pet-tts-volume-val">${Math.round(VOICE_VOLUME_DEFAULT * 100)}%</span>
    </div>
    <div class="pet-tts-voice" id="pet-tts-voice">
      <span class="pet-label">${esc(t("pet.tts_voice_label"))}</span>
      <input type="text" id="pet-tts-reftext" class="pet-tts-text" autocomplete="off" spellcheck="false" placeholder="${esc(t("pet.tts_ref_text_ph"))}" aria-label="${esc(t("pet.tts_voice_label"))}">
      <div class="pet-row">
        <button type="button" id="pet-tts-addfile" class="pet-btn pet-btn-primary">${esc(t("pet.tts_add_audio"))}</button>
      </div>
    </div>
    <div class="pet-row pet-tts-actions">
      <button type="button" id="pet-tts-preview" class="pet-btn">${esc(t("pet.tts_preview"))}</button>
    </div>
    <div class="pet-note" id="pet-tts-status"></div>
    <div class="pet-note" id="pet-tts-msg"></div>
  </div>`;
}

/** 动作类型的中文名（设置窗下拉用）。 */
const ACTION_LABEL: Record<ActionType, string> = {
  motion: "播动作",
  chain: "动作链",
  // VTS 侧对应表情的动作类型叫 “Expression on/off” —— 这里同样按**开关**语义（可叠加、再点关闭）。
  expression: "表情开关",
  param: "设参数",
  artmesh: "ArtMesh 显隐",
  artmeshColor: "ArtMesh 染色",
  part: "Part 显隐",
  physics: "开关物理",
};

/** 「切换控件形态」按钮的图标：**上箭头 + 下箭头并存**（用户 2026-10-10 口径：不区分状态，
 *  就是一个上下箭头图标，替掉原来的「开关 / 连续」文字）。用内联 SVG 而不是字符，尺寸/取色可控。 */
const KIND_ICON =
  `<svg viewBox="0 0 16 16" width="13" height="13" aria-hidden="true" focusable="false">` +
  `<path d="M8 1.6 11.2 5.2H4.8Z" fill="currentColor"/>` +
  `<path d="M8 14.4 4.8 10.8h6.4Z" fill="currentColor"/></svg>`;

/** **热键区**（2026-10-10）：定义存模型目录下 `lunac-live2d\lunac.hotkeys.json`（用户口径：
 *  热键 / 表情 / 模型放同一个文件夹）。这里只出骨架 —— 行由 JS 按数据渲染
 *  （热键是动态列表，写死在模板里改不动，见 `attachSettingsWindow` 的 `refreshHotkeys`）。 */
function hotkeyBlock(): string {
  return `
  <div class="pet-hotkeys" id="pet-hotkeys">
    <div class="pet-row">
      <button type="button" id="pet-hk-import" class="pet-btn pet-btn-primary" title="以模型文件夹为准重建热键表：优先读同目录 vtube.json 里的作者热键，没有就递归扫文件夹里的「中文名.exp3.json」。会自动分配默认键并落盘；按键仅供 AI 触发，本地不做按键监听。">重新导入（模型文件夹）</button>
    </div>
    <div class="pet-hk-list" id="pet-hk-list"></div>
    <div class="pet-note" id="pet-hk-msg"></div>
  </div>`;
}

/** **动作页**（2026-10-10 用户口径：**行 = 文件**）：列模型文件夹里扫到的 `*.motion3.json`
 *  （宿主 `list_motion_files`），每行可改名 / 删除 / 播放。播放入口 = 引擎合成组的下标
 *  （`MOTION_EXTRA_GROUP` + 文件在扫描序里的位置），**文件名是 AI 侧的调用键**。
 *  与热键页同一纪律：行是 JS 渲染的动态列表，样式按结构类写。 */
function motionBlock(): string {
  return `
  <div class="pet-motions" id="pet-motions">
    <!-- **动作链**（2026-10-10 用户口径：与「恢复默认」一样放在动作页**最上面**）：
         把多个动作排进一条时间线，每步各设「循环播放时长 / 硬停秒」。 -->
    <div class="pet-chains" id="pet-chains">
      <div class="pet-row">
        <button type="button" id="pet-chain-add" class="pet-btn pet-btn-primary" title="新建一条动作链：把多个动作排进一条时间线，每个动作各设循环播放时长与硬停秒。可在「热键」页把动作链绑成热键，AI 也可按链名触发。">新建动作链</button>
      </div>
      <div class="pet-chain-list" id="pet-chain-list"></div>
      <div class="pet-note" id="pet-chain-msg"></div>
    </div>
    <div class="pet-row">
      <button type="button" id="pet-mo-add" class="pet-btn pet-btn-primary" title="在模型文件夹里生成一个覆盖本模型全部参数的 *.motion3.json 模板（1 秒 / 30fps / 循环），交给 AI 去编曲线。">新建动作（motion3 模板）</button>
    </div>
    <div class="pet-mo-list" id="pet-mo-list"></div>
    <div class="pet-note" id="pet-mo-msg"></div>
  </div>`;
}

// ── 热键：状态 + 渲染 + 落盘（2026-10-10）────────────────────────────
// **状态放模块级**（不放 attachSettingsWindow 里）：事件委托只注册一次（看上面的纪律），
// 若状态是每次 attach 的新闭包，重渲染后委托会用**旧**的那份 —— 模块级才是单一真相源。
// 所有节点都按 id 从**本窗口**的 document 取（设置窗是独立 WebView，id 不会与桌宠窗撞）。
const CORE_IDS_KEY = "lunac.pet.coreids.v1.";

/** 运行时 id 表（桌宠窗挂载模型时写进 localStorage 的那份）。热键页与调试区都靠它列目标。 */
interface CoreIdTable {
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

/** 读某模型的运行时 id 表（**只读 localStorage 那一份**）。没开过桌宠时返回空表。 */
function readCoreIdsLocal(modelPath: string): CoreIdTable {
  const empty: CoreIdTable = {
    parameters: [],
    drawables: [],
    parts: [],
    parameterRanges: {},
    physicsInputs: [],
    physicsOutputs: [],
  };
  try {
    const raw = localStorage.getItem(CORE_IDS_KEY + modelPath.toLowerCase());
    if (!raw) return empty;
    const o = JSON.parse(raw) as Partial<CoreIdTable>;
    return {
      parameters: Array.isArray(o.parameters) ? o.parameters : [],
      drawables: Array.isArray(o.drawables) ? o.drawables : [],
      parts: Array.isArray(o.parts) ? o.parts : [],
      parameterRanges:
        o.parameterRanges && typeof o.parameterRanges === "object" ? o.parameterRanges : {},
      physicsInputs: Array.isArray(o.physicsInputs) ? o.physicsInputs : [],
      physicsOutputs: Array.isArray(o.physicsOutputs) ? o.physicsOutputs : [],
    };
  } catch {
    return empty; // 坏数据当没有
  }
}

/** 读某模型的运行时 id 表：**优先读 `lunac.params.json` 里的 `coreIds`**（进插件读取之前
 *  就能拿到上次的数据；AI 也可能刚改过），文件没有（或 coreIds 为空）再回退 localStorage。
 *  回退时若 localStorage 那份有值 ⇒ 把它**播种**进文件（设置窗首次打开某模型的一次性迁移，
 *  让文件成为可见的单一入口）。设置窗拿不到运行中的实例，见 `pet-params.ts` 的文件头。 */
async function readCoreIdsAsync(modelPath: string): Promise<CoreIdTable> {
  const local = readCoreIdsLocal(modelPath);
  try {
    const cache = await loadPetParams(modelPath);
    const ci = cache?.coreIds;
    if (ci && (ci.parameters.length || ci.drawables.length || ci.parts.length)) return ci;
    // 文件没有 / coreIds 为空：把 localStorage 那份播种进文件（写一次，合并已有 params）。
    if (local.parameters.length || local.drawables.length || local.parts.length) {
      const seed = cache ?? emptyPetParamCache(modelPath);
      seed.coreIds = local;
      const cfgParams = loadConfig().params[modelPath];
      if (!Object.keys(seed.params).length && cfgParams && typeof cfgParams === "object") {
        seed.params = { ...cfgParams };
      }
      await savePetParams(modelPath, seed);
    }
  } catch (e) {
    console.warn("[pet] 读/播种 lunac.params.json 失败", e); // 一份坏缓存不该崩设置窗
  }
  return local;
}

/** 调试区当前渲染出的参数范围（id → {min,max}）。**每次重画整体重建**，供数值框夹取用。 */
let debugMeta = new Map<string, { min: number; max: number }>();

let hkList: Hotkey[] = [];
let hkTargets: HotkeyTargets = {
  motions: [],
  expressions: [],
  expressionFiles: {},
  parameters: [],
  drawables: [],
  parts: [],
  physicsInputs: [],
  physicsOutputs: [],
};
/** 现在处于「开」状态的热键 id —— 由桌宠窗广播的 `hotkey-states` 同步（开关按钮的选中态）。 */
let hkActive = new Set<string>();

function hkMsg(text: string): void {
  const n = document.getElementById("pet-hk-msg");
  if (n) n.textContent = text;
}

/** 热键表的**防抖落盘**（2026-10-11）：热键行的手工编辑（名称 / 类型 / 目标 / 录键 / 删除）此前
 *  只活在内存里 —— 重开设置窗会被 `refreshHotkeys` 从盘上读回、改动丢失。这里统一在改动后
 *  400ms 落一次盘（与参数持久化同一套防抖口径）。⚠️ 落在**本次调度时的模型**上：换模型后
 *  旧表不写到新模型（下一个 timer 会带新模型重建）。 */
let hkSaveTimer: ReturnType<typeof setTimeout> | null = null;
function scheduleSaveHotkeys(): void {
  const model = loadConfig().model.trim();
  if (!model) return;
  if (hkSaveTimer) clearTimeout(hkSaveTimer);
  hkSaveTimer = setTimeout(() => {
    hkSaveTimer = null;
    if (loadConfig().model.trim() !== model) return; // 换过模型：本次不落，别把旧表写进新模型
    void saveHotkeys(model, hkList).catch(e =>
      hkMsg("保存热键失败：" + (e instanceof Error ? e.message : String(e))),
    );
  }, 400);
}

/** 给 `keys` 为空的条目**按顺序补齐**默认键（用户口径：导入即可用；按键只作 AI 侧标识）。
 *  只填空的，不覆盖用户已录的键。返回是否有改动（调用方据此决定要不要落盘）。 */
function assignDefaultKeys(list: Hotkey[]): boolean {
  let changed = false;
  list.forEach((h, i) => {
    if (!h.keys) {
      h.keys = defaultKeysForIndex(i);
      changed = true;
    }
  });
  return changed;
}

/** 从 `lunac.hotkeys.json` 收集**带 `file` 的表情**（导入到子目录 / 别处、模型文件同层里没有的），
 *  交 `mountLive2D` 追加进 `settings.FileReferences.Expressions`。不这么做的话，那些表情虽然
 *  列在热键页里，引擎却**不认识** ⇒ `setExpression(name)` 是空操作（用户报的「表情完全没应用到
 *  当前模型」）。按绝对路径去重（Windows 大小写不敏感）。 */
async function collectHotkeyExpressions(modelPath: string): Promise<{ name: string; file: string }[]> {
  const out: { name: string; file: string }[] = [];
  const seen = new Set<string>();
  try {
    for (const hk of await loadHotkeys(modelPath)) {
      const a = hk.action;
      if (a.type !== "expression" || !a.file) continue;
      const key = a.file.toLowerCase();
      if (seen.has(key)) continue;
      seen.add(key);
      out.push({ name: a.target || hk.name, file: a.file });
    }
  } catch {
    /* 没有热键文件 / 读失败：返回空，照常挂载 */
  }
  return out;
}

/** 路径归一：Windows 大小写不敏感、`/` 与 `\` 等价。热键里的 `file` 与模型枚举出的路径要按它对齐。 */
function normPath(p: string): string {
  return p.toLowerCase().replace(/\//g, "\\");
}

/** 把每条 `expression` 动作的 `target` **解析成当前模型认识的那个名字**。
 *
 *  为什么必须解析：`target` 最终交给引擎的 `model.expression(id)`，它按模型的
 *  `FileReferences.Expressions[].Name` 查表 —— 而**文件名 ≠ 模型的 Name**（模型常给
 *  `Name` 另起一个值）。导入时若直接把文件名填进 `target`，热键就切不动（用户报的
 *  「target 直接引用了 name」）。这里按**绝对路径**与模型的表情表对齐，取模型那一侧的 id。
 *  对不上（文件不在模型的表情表里，例如模型没声明、同层也没扫到）时保留原值，交给
 *  `mountLive2D` 的 `extraExpressions` 注入时用它当 `Name`。返回是否有改动。 */
async function resolveHotkeyTargets(modelPath: string, list: Hotkey[]): Promise<boolean> {
  const need = list.filter(h => h.action.type === "expression" && !!h.action.file);
  if (!need.length) return false;
  let known: Awaited<ReturnType<typeof listExpressions>>;
  try {
    known = await listExpressions(modelPath);
  } catch {
    return false; // 读不到模型：不动（保持原值）
  }
  const byPath = new Map<string, string>();
  for (const e of known) if (e.file) byPath.set(normPath(e.file), e.id);
  let changed = false;
  for (const h of need) {
    if (h.action.type !== "expression" || !h.action.file) continue;
    const id = byPath.get(normPath(h.action.file));
    if (id && id !== h.action.target) {
      h.action.target = id;
      changed = true;
    }
  }
  return changed;
}

/** 收集「被热键里的表情用到的参数 id」—— 调试区据此把这一批**折叠**起来（用户 2026-10-10 口径：
 *  那一大批参数与热键的表情是重合的，不必特意罗列）。逐个读 `.exp3.json` 的 `Parameters`
 *  （同一文件走 `readExpressionParams` 的缓存）。读不到时返回空集合（调试区照常全列）。 */
async function collectExpressionParamIds(modelPath: string): Promise<Set<string>> {
  const out = new Set<string>();
  try {
    const files: string[] = [];
    for (const hk of await loadHotkeys(modelPath)) {
      if (hk.action.type === "expression" && hk.action.file) files.push(hk.action.file);
    }
    await Promise.all(
      files.map(async f => {
        for (const p of await readExpressionParams(f)) out.add(p.id);
      }),
    );
  } catch {
    /* 读不到热键 / 表情：返回空集合 */
  }
  return out;
}

/** 找模型目录里「模型全貌」的两份配置文件（VTS 约定就放在模型旁边）：
 *  `*.vtube.json`（作者热键 + 参数绑定）与 `*.cdi3.json`（参数 / 部件的可读名字）。
 *  走宿主命令 `scan_model_folder`（WebView 列不了目录）；读不到时返回空串，调用方各自回落。 */
async function findModelSideFiles(modelPath: string): Promise<{ dir: string; vtube: string; cdi3: string }> {
  const dir = dirName(modelPath);
  try {
    const s = await invoke<{ model3?: string[]; vtube?: string[]; cdi3?: string[] }>("scan_model_folder", { dir });
    return { dir, vtube: (s.vtube ?? [])[0] ?? "", cdi3: (s.cdi3 ?? [])[0] ?? "" };
  } catch {
    return { dir, vtube: "", cdi3: "" }; // 命令缺失 / 目录读不到：当成「没有这两份配置文件」
  }
}

/** 空的可读名字表（读不到 cdi3 时用，省得每处都判空）。 */
const EMPTY_CDI3: Cdi3Names = { parameters: {}, parts: {}, artMeshes: {} };

/** 开关判定的「实际取值」证据：把模型声明的**动作文件**（`FileReferences.Motions` 的
 *  `.motion3.json`）扫一遍，得到「参数 → 出现过哪些值」。开关类参数只出现 0/1，连续量会出现
 *  中间值。读不到 / 没动作 ⇒ 空表（判定退回纯区间，见 refreshDebug）。 */
async function collectSwitchEvidence(modelPath: string, dir: string): Promise<Map<string, Set<number>>> {
  try {
    const json = await readModelJson(modelPath);
    const files: string[] = [];
    for (const list of Object.values(json.FileReferences?.Motions ?? {})) {
      for (const m of list ?? []) {
        if (m?.File) files.push(joinPath(dir, m.File));
      }
    }
    if (!files.length) return new Map();
    return await collectMotionParamValues(files);
  } catch {
    return new Map();
  }
}

/** 从模型文件夹建 / 重建热键表。**优先读 `.vtube.json` 的作者热键**（用户 2026-10-10 口径：
 *  「导入时应该也一并读取」）；没有 / 空表就回落「递归扫 `*.exp3.json`」（只认中文名那一种形态）。
 *  两条路都：自动分配默认键 → `target` 按绝对路径解析成**模型那一侧的名字** → 落盘 → 刷新列表。 */
async function importHotkeysFromFolder(dirAbs: string, modelPath: string, vtubePath: string): Promise<number> {
  let list: Hotkey[] = [];
  if (vtubePath) {
    const vts = await readVtsHotkeys(vtubePath);
    list = vts.map(h => hotkeyFromVts(h, dirAbs)).filter((h): h is Hotkey => !!h);
  }
  if (!list.length) {
    let files: { path?: string; name?: string }[] = [];
    try {
      files = await invoke<{ path?: string; name?: string }[]>("list_exp3_files", { dir: dirAbs });
    } catch {
      files = [];
    }
    list = files
      .filter(f => typeof f?.path === "string" && f.path)
      .map(f => {
        const name = (f.name || "").trim() || "表情";
        return makeHotkey(name, "", { type: "expression", target: name, mode: "set", file: f.path });
      });
  }
  if (!list.length) return 0;
  assignDefaultKeys(list);
  await resolveHotkeyTargets(modelPath, list);
  await saveHotkeys(modelPath, list);
  hkList = list;
  renderHotkeys();
  return list.length;
}

// ── 动作页：状态 + 渲染（2026-10-10）────────────────────────────────
// **行 = 文件**（用户 2026-10-10 口径）：页里直接列模型文件夹里扫到的 `*.motion3.json`
// （每行一个，可改名 / 播放 / 删除）；「新建动作」在文件夹里生成一个**覆盖全参数**的
// motion3 模板，交给 AI 去编曲线。顺序 = `list_motion_files` 的路径序 = 引擎注入组里的序号。
// 与热键同一纪律：状态放**模块级**（委托只注册一次，重渲染后不能用旧闭包）。
let moFiles: MotionFileRef[] = [];

/** **动作链**（2026-10-10，backlog ㉝②）：状态放模块级（与 `moFiles` 同一纪律 —— 委托只注册
 *  一次，重渲染后不能用旧闭包）。存 `<模型目录>\lunac-live2d\lunac.chains.json`，由 `refreshChains` 从宿主读。 */
let chains: MotionChain[] = [];

/** 本插件**生成**的动作文件名前缀 —— 「恢复默认」只删这一批，绝不动作者/用户自己的动作。 */
const MO_GEN_PREFIX = "lunac_";

function moMsg(text: string): void {
  const n = document.getElementById("pet-mo-msg");
  if (n) n.textContent = text;
}

/** 该模型**勾为「循环播放」**的动作文件名集合（小写，比对时忽略大小写）。
 *  （键名/函数名沿用 `idle*` 是历史遗留，语义见 `PetConfig.idleMotions` 的注释。） */
function idleNameSet(model: string): Set<string> {
  const list = model ? loadConfig().idleMotions[model] ?? [] : [];
  return new Set(list.map(s => s.toLowerCase()));
}

/** 把「循环播放」列表里**已不存在**的文件名摘掉（删 / 改名 / 换文件夹之后调，避免死项）。
 *  依赖调用方已刷新 `moFiles`（本函数按它判存活）。 */
function pruneIdleMotions(): void {
  const model = loadConfig().model.trim();
  if (!model) return;
  const c = loadConfig();
  const cur = c.idleMotions[model];
  if (!cur || !cur.length) return;
  const alive = new Set(moFiles.map(f => f.name.toLowerCase()));
  const kept = cur.filter(s => alive.has(s.toLowerCase()));
  if (kept.length === cur.length) return;
  const next = { ...c.idleMotions };
  if (kept.length) next[model] = kept;
  else delete next[model];
  saveConfig({ ...c, idleMotions: next });
  renderMotions();
}

/** 按 `moFiles` 重画整个动作列表（行 = 文件；下标即播放序号）。 */
function renderMotions(): void {
  const box = document.getElementById("pet-mo-list");
  if (!box) return;
  if (!moFiles.length) {
    box.innerHTML = `<div class="pet-note">模型文件夹里还没有 *.motion3.json —— 点「新建动作」生成一个模板。</div>`;
    return;
  }
  const idle = idleNameSet(loadConfig().model.trim());
  box.innerHTML = moFiles
    .map((f, i) => {
      const on = idle.has(f.name.toLowerCase());
      return `<div class="pet-mo-row" data-mo-i="${i}">
      <input class="pet-mo-name" data-mo-name="${i}" spellcheck="false" value="${esc(f.name)}" title="${esc(f.path)}">
      <button type="button" class="pet-btn pet-btn-toggle pet-mo-idle${on ? " on" : ""}" data-mo-idle="${i}" aria-pressed="${on ? "true" : "false"}" title="循环播放：勾上后，模型闲下来（没有别的动作在播）时会按顺序轮流播放它；是否循环由该文件自己的 Meta.Loop 决定">循环播放：${on ? "开" : "关"}</button>
      <button type="button" class="pet-btn pet-mo-run" data-mo-run="${i}">播放</button>
      <button type="button" class="pet-btn pet-mo-del" data-mo-del="${i}">删除</button>
      <div class="pet-chain-bar pet-mo-bar" data-bar-idx="${i}"><i></i><span class="pet-mo-bar-t">${esc(f.name)}</span></div>
    </div>`;
    })
    .join("");
}

/** 扫模型文件夹里的 `*.motion3.json` 并重画。模型为空 / 没文件时给明确提示。 */
async function refreshMotions(): Promise<void> {
  const model = loadConfig().model.trim();
  if (!model) {
    moFiles = [];
    renderMotions();
    chains = [];
    renderChains();
    moMsg("先选一个 Live2D 模型 —— 「更换模型」里选模型文件夹。");
    return;
  }
  moFiles = await listMotionFiles(model);
  renderMotions();
  // 动作链与动作文件同一份清单（步骤下拉用 `moFiles`）⇒ 一起刷新。
  void refreshChains();
  moMsg(moFiles.length ? `共 ${moFiles.length} 个动作文件。` : "这个模型文件夹里没扫到 *.motion3.json。");
}

// ── 动作链（2026-10-10，backlog ㉝②）────────────────────────────────
// 把多个动作排在一条时间线上：每步 `loopSec`（循环播放时长）+ `stopSec`（硬停秒上限）。
// 存 `<模型目录>\lunac-live2d\lunac.chains.json`（宿主读写，见 pet-chains.ts）。**在动作页最上面**。

function chainMsg(text: string): void {
  const n = document.getElementById("pet-chain-msg");
  if (n) n.textContent = text;
}

/** 文件绝对路径 → 它在 `moFiles` 里的下标（进度条 / 播放都按这个下标走）。 */
function motionIndexOfFile(file: string): number {
  return moFiles.findIndex(f => f.path.toLowerCase() === file.toLowerCase());
}

/** 折叠态集合（**只活在内存**、不落盘）：收起的是哪几条链的 id。**缺省 = 展开** ——
 *  不隐藏既有功能；用户点一下箭头才收起（折叠态只显示一行摘要）。 */
let chainCollapsed = new Set<string>();

/** 折叠 / 展开箭头（线性 SVG，跟随按钮文字色；`on` 态旋转 90° 成「朝下」）。
 *  用 SVG 而不是字符，尺寸 / 取色可控（对齐 docs/ui-style.md §界面 4 的图标纪律）。 */
const CHEVRON_SVG =
  `<svg viewBox="0 0 16 16" width="11" height="11" aria-hidden="true" focusable="false">` +
  `<path d="M6 3.2 10.8 8 6 12.8" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/></svg>`;

/** 「中止对话」按钮的图标（方块 / 停止；严格线性 SVG，走 §界面 4 的残影语言）。
 *  `currentColor` 跟随按钮文字色（§界面 14.1 的图标按钮 = `rgb(var(--btn-line-rgb))`），不写死色值。 */
const STOP_SVG =
  `<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" ` +
  `stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false">` +
  `<g opacity="0.28" transform="translate(-0.7,-0.7)" stroke-width="1.6"><rect x="5" y="5" width="14" height="14" rx="2.5"/></g>` +
  `<rect x="5" y="5" width="14" height="14" rx="2.5"/>` +
  `<circle cx="8.6" cy="8.6" r="0.9" fill="currentColor" stroke="none"/></svg>`;

/** 窗上「缩放手柄」的图标（右下角斜向抓握；严格线性 SVG）。`currentColor` 跟随 `--text-dim`
 *  （hover 变 `--text`），不写死色值。 */
const RESIZE_SVG =
  `<svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" ` +
  `stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false">` +
  `<path d="M22 6 6 22"/><path d="M22 12 12 22"/><path d="M22 18 18 22"/></svg>`;

/** 一步的**折叠态摘要**（文件名词干 · 循环 · 硬停）。整条链的摘要 = 各步摘要拼接。 */
function chainStepSummary(s: ChainStep): string {
  const fname = s.file.split(/[\\/]/).pop() || "(未选)";
  const stop = s.stopSec > 0 ? `${s.stopSec.toFixed(1)}s` : "不限";
  return `${fname} · 循环 ${s.loopSec.toFixed(1)}s · 硬停 ${stop}`;
}

/** 读一个动作文件自己的 `Meta.Duration`（秒）—— 就是引擎注入时用的那一份。
 *  **这就是「动作完整做完的时间」**：`.motion3.json` 顶层 `Meta.Duration`（秒、含小数）。
 *  读不到 / 坏值 ⇒ `null`（调用方**不覆盖**用户已有值）。与 live2d-engine.ts 里算
 *  `durMs` 的口径完全一致（那边读不到退回 3s 保守值；这边读不到则留原值）。 */
async function readMotionDurationSec(file: string): Promise<number | null> {
  if (!file) return null;
  try {
    const res = await fetch(convertFileSrc(file));
    if (!res.ok) return null;
    const j = (await res.json()) as { Meta?: { Duration?: unknown } };
    const d = Number(j?.Meta?.Duration);
    return Number.isFinite(d) && d > 0 ? Math.min(3600, Math.max(0.1, d)) : null;
  } catch {
    return null;
  }
}

/** 重画整个动作链区（链是动态列表，改一条就整体重画）。 */
function renderChains(): void {
  const box = document.getElementById("pet-chain-list");
  if (!box) return;
  if (!chains.length) {
    box.innerHTML = `<div class="pet-note">还没有动作链 —— 点「新建动作链」，再把动作文件排进时间线。</div>`;
    return;
  }
  box.innerHTML = chains
    .map((c, ci) => {
      const collapsed = chainCollapsed.has(c.id);
      const steps = c.steps
        .map((s, si) => {
          const idx = motionIndexOfFile(s.file);
          const fname = s.file.split(/[\\/]/).pop() || "(未选)";
          const fileOpts: CustomSelectOption[] = moFiles.length
            ? moFiles.map(f => ({ value: f.path, label: f.name, selected: f.path.toLowerCase() === s.file.toLowerCase() }))
            : [{ value: "", label: "（没有动作文件）", selected: true }];
          const sec = stepSeconds(s);
          return `<div class="pet-chain-step">
          <div class="pet-chain-fields">
            ${customSelectHtml("", fileOpts, `data-chain-step-file="${ci}:${si}"`)}
            <label class="pet-chain-t">循环<input class="pet-chain-num" type="number" min="0.1" max="3600" step="0.1" data-chain-step-loop="${ci}:${si}" value="${s.loopSec}">s</label>
            <label class="pet-chain-t">硬停<input class="pet-chain-num" type="number" min="0" max="3600" step="0.1" data-chain-step-stop="${ci}:${si}" value="${s.stopSec}">s</label>
            <button type="button" class="pet-btn pet-chain-del" data-chain-step-del="${ci}:${si}" title="删掉这一步">×</button>
          </div>
          <div class="pet-chain-bar" data-bar-chain="${esc(c.id)}" data-bar-idx="${idx}"><i></i><span>${esc(fname)} · ${sec.toFixed(1)}s</span></div>
        </div>`;
        })
        .join("");
      // 折叠态的**摘要**（各行一句「文件 · 循环 · 硬停」，整条拼起来，过长由 CSS 省略）。
      const summary = c.steps.length
        ? c.steps.map(chainStepSummary).join("  ·  ")
        : "（还没有步骤）";
      return `<div class="pet-chain${collapsed ? " collapsed" : ""}" data-chain-i="${ci}">
      <div class="pet-chain-head">
        <button type="button" class="pet-btn pet-chain-toggle${collapsed ? "" : " on"}" data-chain-toggle="${ci}" aria-expanded="${collapsed ? "false" : "true"}" title="${collapsed ? "展开这条链" : "收起这条链"}">${CHEVRON_SVG}</button>
        <input class="pet-chain-name" data-chain-name="${ci}" spellcheck="false" value="${esc(c.name)}">
        <span class="pet-chain-total">共 ${chainSeconds(c).toFixed(1)}s</span>
        <button type="button" class="pet-btn pet-btn-toggle pet-chain-loopbtn${c.loop ? " on" : ""}" data-chain-loop="${ci}" aria-pressed="${c.loop ? "true" : "false"}" title="整链循环：整条链播完从头再来；关闭则播完即停（每步自己的循环 / 硬停照旧生效）">整链循环：${c.loop ? "开" : "关"}</button>
        <button type="button" class="pet-btn pet-btn-primary pet-chain-run" data-chain-run="${esc(c.id)}">播放</button>
        <button type="button" class="pet-btn pet-chain-del" data-chain-del="${ci}">删除</button>
      </div>
      <div class="pet-chain-summary">${esc(summary)}</div>
      <div class="pet-chain-steps">${steps}</div>
      <button type="button" class="pet-btn" data-chain-step-add="${ci}">添加步骤</button>
    </div>`;
    })
    .join("");
  wirePetCustomSelects(box);
}

/** 从宿主读该模型的链并重画。读不到 ⇒ 空（不报错）。 */
async function refreshChains(): Promise<void> {
  const model = loadConfig().model.trim();
  if (!model) {
    chains = [];
    renderChains();
    return;
  }
  chains = await loadChains(model);
  renderChains();
  renderHotkeys(); // 热键页的动作链下拉跟着链列表走（新增 / 删除链后要对齐）
  chainMsg(chains.length ? `共 ${chains.length} 条动作链。` : "");
}

/** 落盘 + 重画（链改了就存；失败只提示，内存里的改动留着）。 */
async function persistChains(): Promise<void> {
  const model = loadConfig().model.trim();
  if (!model) return;
  try {
    await saveChains(model, chains);
  } catch (e) {
    chainMsg("保存动作链失败：" + (e instanceof Error ? e.message : String(e)));
  }
}

/** 动作链某一步**换了动作文件**（自绘下拉 .custom-select 选定后调这里）：
 *  写回 + **循环秒数默认 = 该动作完整时长**（`.motion3.json` 的 `Meta.Duration`，只在切换
 *  这一次填；读不到时长 ⇒ 保持原值、绝不拿坏值覆盖），随后重画 + 落盘。 */
function commitChainFile(ci: number, si: number, file: string): void {
  const c = chains[ci];
  if (!c || !Number.isInteger(si)) return;
  void (async () => {
    const dur = await readMotionDurationSec(file);
    const cc = chains[ci];
    if (!cc) return;
    const steps = cc.steps.map((s, i) =>
      i === si ? { ...s, file, ...(dur !== null ? { loopSec: dur } : {}) } : s,
    );
    chains = chains.map((x, i) => (i === ci ? { ...x, steps } : x));
    renderChains();
    void persistChains();
  })();
}

/** 进度条：把 `index`（可指定 chainId）那一条的进度设成 `pct`（0..1）。 */
function setChainBar(chainId: string | null, index: number, pct: number | null): void {
  const sel = chainId
    ? `[data-bar-chain="${CSS.escape(chainId)}"][data-bar-idx="${index}"]`
    : `.pet-mo-row [data-bar-idx="${index}"]`;
  document.querySelectorAll<HTMLElement>(sel).forEach(el => {
    const i = el.querySelector("i");
    if (i) i.style.width = pct === null ? "0%" : `${Math.round(pct * 100)}%`;
  });
}


/** 按当前模型的运行时参数表生成一个 motion3 模板（每个参数一条**常数曲线**，取模型默认值）。
 *  这**不是**「好看的动作」—— 它只是把参数铺齐、时长 1s / 30fps / 循环，留给 AI 往里编曲线。
 *  拿不到运行时参数表（从没显示过桌宠）⇒ 返回 null（调用方提示先显示一次）。 */
async function buildMotionTemplate(modelPath: string): Promise<string | null> {
  const core = await readCoreIdsAsync(modelPath);
  if (!core.parameters.length) return null;
  const curves = core.parameters.map(id => {
    const r = core.parameterRanges[id];
    const v = r && Number.isFinite(r.def) ? r.def : 0;
    // `Segments` 是扁平数组：`[起点x, 起点y, 段类型, x, y]`；类型 0 = 线性（这里是一条常数段）。
    return { Target: "Parameter", Id: id, Segments: [0, v, 0, 1, v] };
  });
  return JSON.stringify(
    {
      Version: 3,
      Meta: {
        Duration: 1,
        Fps: 30,
        Loop: true,
        AreBeziersRestricted: true,
        CurveCount: curves.length,
        TotalSegmentCount: curves.length,
        TotalPointCount: curves.length * 2,
        UserDataCount: 0,
        TotalUserDataSize: 0,
      },
      Curves: curves,
      UserData: [],
    },
    null,
    2,
  );
}

/** 新动作的缺省字段（切换类型时用）。目标一律留空，逼用户自己选。 */
function defaultActionFor(type: ActionType): HotkeyAction {
  switch (type) {
    case "motion":
      return { type: "motion", group: "", index: 0 };
    case "chain":
      return { type: "chain", id: "" };
    case "param":
      return { type: "param", id: "", value: 0, mode: "set" };
    case "artmesh":
      return { type: "artmesh", id: "", value: 0 };
    case "artmeshColor":
      return { type: "artmeshColor", id: "", color: "#ffffff" };
    case "part":
      return { type: "part", id: "", value: 0 };
    case "physics":
      return { type: "physics", enabled: false };
    default:
      return { type: "expression", target: "", mode: "set" };
  }
}

/** 把一个字段写进动作（字段名来自 DOM 的 `data-hk-field`）。 */
function applyHotkeyField(a: HotkeyAction, field: string, val: string): void {
  const num = () => Number(val) || 0;
  if (a.type === "motion") {
    if (field === "group") a.group = val;
    else if (field === "index") a.index = Math.max(0, Math.round(num()));
  } else if (a.type === "chain") {
    if (field === "id") a.id = val;
  } else if (a.type === "expression") {
    if (field === "target") {
      a.target = val;
      // 换表情对象 ⇒ `file` 一并换成该表情的 `.exp3.json`（能对上就换、对不上清空）——
      // 否则叠加表情层读的是旧 `file`，与 `target` 对不上。
      a.file = hkTargets.expressionFiles[val] || "";
    } else if (field === "mode") a.mode = val === "clear" ? "clear" : "set";
  } else if (a.type === "param") {
    if (field === "id") a.id = val;
    else if (field === "mode") a.mode = val === "add" ? "add" : "set";
    else if (field === "value") a.value = num();
  } else if (a.type === "artmesh") {
    if (field === "id") a.id = val;
    else if (field === "value") a.value = Math.min(1, Math.max(0, num()));
  } else if (a.type === "artmeshColor") {
    if (field === "id") a.id = val;
    else if (field === "color") a.color = normalizeHex(val);
  } else if (a.type === "part") {
    if (field === "id") a.id = val;
    else if (field === "value") a.value = Math.min(1, Math.max(0, num()));
  } else if (a.type === "physics" && field === "enabled") {
    a.enabled = val === "1";
  }
}

/** `#rgb` / `#rrggbb` / 缺 `#` → 一律规范化成小写 `#rrggbb`（坏值 → `#ffffff`）。
 *  ArtMesh 染色存的就是这个串（人可读、可直接喂 `<input type="color">`）。 */
function normalizeHex(v: string): string {
  const s = (v || "").trim().replace(/^#/, "").toLowerCase();
  if (/^[0-9a-f]{3}$/.test(s)) return "#" + s.split("").map(c => c + c).join("");
  if (/^[0-9a-f]{6}$/.test(s)) return "#" + s;
  return "#ffffff";
}

/** `#rrggbb` → `{r,g,b}` 各 0..1（引擎 `setDrawableColor` 要的形态）。 */
function hexToRgb(v: string): { r: number; g: number; b: number } {
  const s = normalizeHex(v).slice(1);
  return {
    r: parseInt(s.slice(0, 2), 16) / 255,
    g: parseInt(s.slice(2, 4), 16) / 255,
    b: parseInt(s.slice(4, 6), 16) / 255,
  };
}

function hkOptList(values: string[], current: string): string {
  // 把**当前值**并进选项：模型换过 / 运行时 id 表缺失时枚举可能不全，已存的目标仍照常显示、不被清空。
  const list = current && !values.includes(current) ? [current, ...values] : values;
  const opts = list.length ? list : [""];
  return opts
    .map(v => `<option value="${esc(v)}"${v === current ? " selected" : ""}>${esc(v || "（无可用项）")}</option>`)
    .join("");
}

/** 与 `hkOptList` 同源，但出**自绘下拉**（.custom-select）要的选项数组（原生的只留其余下拉用）。 */
function hkOptArray(values: string[], current: string): CustomSelectOption[] {
  const list = current && !values.includes(current) ? [current, ...values] : values;
  const opts = list.length ? list : [""];
  return opts.map(v => ({ value: v, label: v || "（无可用项）", selected: v === current }));
}

/** 给一段 DOM 里所有自绘下拉（.custom-select）接上**共享组件**的交互。
 *  热键行 / 动作链每次**整体重画**后都要调一次（节点换了，监听器要重绑）。
 *  选定后写回**原状态**并走**原有落盘路径**（热键防抖落盘 `scheduleSaveHotkeys`、
 *  动作链 `commitChainFile`）—— 与原原生 `<select>` 的 `change` 分支行为一致。 */
function wirePetCustomSelects(box: ParentNode): void {
  box.querySelectorAll<HTMLElement>(".custom-select").forEach(dd => {
    attachCustomSelect(dd, {
      scope: document,
      onPick: (value) => {
        const row = dd.closest<HTMLElement>(".pet-hk-row");
        if (row) {
          const h = hkList.find(x => x.id === row.dataset.hkId);
          if (!h) return;
          const field = dd.getAttribute("data-hk-field");
          if (field === "type") {
            // 换类型 ⇒ 目标区整个换掉（同原 change 的 type 分支）。
            h.action = defaultActionFor(value as ActionType);
            renderHotkeys();
            scheduleSaveHotkeys();
          } else if (field === "target") {
            // 换表情对象 ⇒ `file` 一并跟着换（见 applyHotkeyField）；不重画（同原分支）。
            applyHotkeyField(h.action, "target", value);
            scheduleSaveHotkeys();
          }
          return;
        }
        const attr = dd.getAttribute("data-chain-step-file");
        if (attr) {
          const [ci, si] = attr.split(":").map(x => Number.parseInt(x, 10));
          commitChainFile(ci, si, value);
        }
      },
    });
  });
}

/** 按 `hkList` 重画整个列表（热键是动态列表，改一条就整体重画，最省事也最不易错）。 */
function renderHotkeys(): void {
  const box = document.getElementById("pet-hk-list");
  if (!box) return;
  box.innerHTML = hkList
    .map(h => {
      const a = h.action;
      const typeOpts: CustomSelectOption[] = ACTION_TYPES.map(
        x => ({ value: x, label: ACTION_LABEL[x], selected: x === a.type }),
      );
      let extra = "";
      if (a.type === "motion") {
        extra =
          `<select data-hk-field="group">${hkOptList(hkTargets.motions.map(m => m.group), a.group)}</select>` +
          `<input class="pet-hk-num" data-hk-field="index" inputmode="numeric" value="${a.index}">`;
      } else if (a.type === "expression") {
        // 「表情对象 target」**改成可选**（用户 2026-10-11 口径）。选项来源 = 模型可用的表情 id
        //（`enumerateTargets` 的 `expressions`，由 listExpressions 从 model3.json 的声明 + 目录扫描得来，
        //  与 `Live2DHandle.setExpression` 的 id 同源）。改选时 `file` 一并跟着换（见 applyHotkeyField），
        // 否则叠加表情层读的是旧 `file`、与 `target` 对不上。
        extra = customSelectHtml("", hkOptArray(hkTargets.expressions, a.target), 'data-hk-field="target" title="表情对象（模型可用的表情）"');
      } else if (a.type === "chain") {
        // 动作链：下拉选一条（id 引用，文件 / 名字改了也不断链）。链列表由 refreshChains 填。
        const list: { id: string; name: string }[] = chains.map(c => ({ id: c.id, name: c.name }));
        if (a.id && !list.some(c => c.id === a.id)) list.unshift({ id: a.id, name: "（已删除的动作链）" });
        extra = list.length
          ? `<select data-hk-field="id">${list.map(c => `<option value="${esc(c.id)}"${c.id === a.id ? " selected" : ""}>${esc(c.name || c.id)}</option>`).join("")}</select>`
          : `<span class="pet-hk-target">（还没有动作链）</span>`;
      } else if (a.type === "param") {
        extra =
          `<select data-hk-field="id">${hkOptList(hkTargets.parameters, a.id)}</select>` +
          `<select data-hk-field="mode"><option value="set"${a.mode === "set" ? " selected" : ""}>设为</option><option value="add"${a.mode === "add" ? " selected" : ""}>加</option></select>` +
          `<input class="pet-hk-num" data-hk-field="value" inputmode="decimal" value="${a.value}">`;
      } else if (a.type === "artmesh") {
        extra =
          `<select data-hk-field="id">${hkOptList(hkTargets.drawables, a.id)}</select>` +
          `<input class="pet-hk-num" data-hk-field="value" inputmode="decimal" value="${a.value}">`;
      } else if (a.type === "artmeshColor") {
        extra =
          `<select data-hk-field="id">${hkOptList(hkTargets.drawables, a.id)}</select>` +
          `<input class="pet-hk-color" type="color" data-hk-field="color" value="${esc(normalizeHex(a.color))}" title="点一下选颜色（原色 = #ffffff）">`;
      } else if (a.type === "part") {
        extra =
          `<select data-hk-field="id">${hkOptList(hkTargets.parts, a.id)}</select>` +
          `<input class="pet-hk-num" data-hk-field="value" inputmode="decimal" value="${a.value}">`;
      } else {
        extra = `<select data-hk-field="enabled"><option value="1"${a.enabled ? " selected" : ""}>开</option><option value="0"${!a.enabled ? " selected" : ""}>关</option></select>`;
      }
      return `<div class="pet-hk-row" data-hk-id="${esc(h.id)}">
        <input class="pet-hk-name" data-hk-field="name" spellcheck="false" value="${esc(h.name)}">
        ${customSelectHtml("", typeOpts, 'data-hk-field="type"')}
        ${extra}
        <button type="button" class="pet-btn pet-btn-toggle pet-hk-run${hkActive.has(h.id) ? " on" : ""}" data-hk-run="1" aria-pressed="${hkActive.has(h.id) ? "true" : "false"}">${hkActive.has(h.id) ? "开" : "关"}</button>
        <button type="button" class="pet-btn pet-hk-del" data-hk-del="1">删除</button>
      </div>`;
    })
    .join("");
  wirePetCustomSelects(box);
}

/** 读该模型的热键 + 目标清单，并重画。模型为空 / 读不到时给出明确提示。 */
async function refreshHotkeys(): Promise<void> {
  const model = loadConfig().model.trim();
  if (!model) {
    hkList = [];
    renderHotkeys();
    hkMsg("先选一个 Live2D 模型 —— 热键要跟它存在同一个文件夹里。");
    return;
  }
  // 运行时 id 表（参数 / ArtMesh / Part）优先读 `lunac-live2d\lunac.params.json`，回退桌宠窗
  // 挂载模型时写进 localStorage 的那份；都没开过 ⇒ 只剩动作 / 表情 / 物理三类可选（提示用户开一次桌宠）。
  const core = await readCoreIdsAsync(model);
  hkTargets = await enumerateTargets(model, core);
  hkList = await loadHotkeys(model);
  // 老文件里 `keys` 可能是空串（早期导入只建条目、不绑键）⇒ 补齐默认键并落盘一次。
  if (assignDefaultKeys(hkList)) {
    try {
      await saveHotkeys(model, hkList);
    } catch {
      /* 落盘失败不影响展示：改动留在内存里，下次导入/保存会再写 */
    }
  }
  renderHotkeys();
  hkMsg(
    core.parameters.length || core.drawables.length
      ? ""
      : "提示：先在主界面「显示桌宠」一次，才能拿到该模型的参数 / ArtMesh / Part 清单（现在只有动作、表情、物理可选）。",
  );
}

/** **桌宠设置界面**的内容 —— 它现在**只在独立的设置窗里**渲染
 *  （主窗搜索「桌宠」由 `main.ts` 的 `executePlugin` 特判、直接开那扇窗）。 */
function settingsHtml(cfg: PetConfig): string {
  const hasAny = !!cfg.model.trim() || !!cfg.image.trim();
  return `
<div class="pet-console">
  <div class="pet-row pet-row-main">
    <button id="pet-toggle" class="pet-btn pet-btn-primary">显示桌宠</button>
    <button id="pet-change" class="pet-btn">更换模型…</button>
    <button id="pet-clear" class="pet-btn" ${hasAny ? "" : "hidden"}>取消配置</button>
    <button type="button" id="pet-through" class="pet-btn pet-btn-toggle${cfg.clickThrough ? " on" : ""}" aria-pressed="${cfg.clickThrough ? "true" : "false"}">${esc(throughLabel(cfg.clickThrough))}</button>
  </div>
  <div class="pet-row">
    <span class="pet-label">形象大小</span>
    <input type="range" id="pet-scale" class="pet-range" min="40" max="800" step="5" value="${Math.round(cfg.scale * 100)}">
    <span class="pet-val" id="pet-scale-val">${Math.round(cfg.scale * 100)}%</span>
  </div>
  <div class="pet-row">
    <span class="pet-label">旋转角度</span>
    <input type="range" id="pet-rot" class="pet-range" min="-180" max="180" step="1" value="${Math.round(cfg.rotation)}">
    <span class="pet-val" id="pet-rot-val">${Math.round(cfg.rotation)}°</span>
  </div>
  <div class="pet-file" id="pet-file"></div>
  <div class="pet-note">该插件只提供 Live2D 模型入口。</div>
  <div class="pet-note">${t("pet.hint_interactions")}</div>
  <div class="pet-tabs" id="pet-tabs">
    <button type="button" class="pet-tab on" data-pet-tab="debug">${esc(t("pet.debug_title"))}</button>
    <button type="button" class="pet-tab" data-pet-tab="hotkey">${esc(t("pet.hotkey_title"))}</button>
    <button type="button" class="pet-tab" data-pet-tab="motion">${esc(t("pet.motions_title"))}</button>
    <button type="button" class="pet-tab" data-pet-tab="proactive">${esc(t("pet.proactive_title"))}</button>
    <button type="button" class="pet-tab" data-pet-tab="voice">${esc(t("pet.tts_title"))}</button>
    <button type="button" class="pet-tab" data-pet-tab="persona">${esc(t("pet.persona_title"))}</button>
  </div>
  <div class="pet-tabpage" data-pet-page="debug">
    <div class="pet-row pet-reset-row">
      <button type="button" id="pet-reset-debug" class="pet-btn" title="把该模型调过的参数全部恢复默认（会清掉你的调整）">恢复默认</button>
    </div>
    <div class="pet-debug" id="pet-debug">
      <div class="pet-debug-note" id="pet-debug-note" hidden></div>
      <div class="pet-debug-body" id="pet-debug-body"></div>
    </div>
  </div>
  <div class="pet-tabpage" data-pet-page="hotkey" hidden>
    <div class="pet-row pet-reset-row">
      <button type="button" id="pet-reset-hotkeys" class="pet-btn" title="清空当前热键表（可再点「重新导入」重建）">恢复默认</button>
    </div>
    ${hotkeyBlock()}
  </div>
  <div class="pet-tabpage" data-pet-page="motion" hidden>
    <div class="pet-row pet-reset-row">
      <button type="button" id="pet-reset-motions" class="pet-btn" title="删掉本模型文件夹里由本插件生成的 *.motion3.json">恢复默认</button>
    </div>
    ${motionBlock()}
  </div>
  <!-- 三个新分页（2026-10-13）：把原先散在页签之外的三块控件搬进来。
       迁移只挪节点、接线（事件委托 / 落盘）原样复用，语义不分叉。 -->
  <div class="pet-tabpage" data-pet-page="proactive" hidden>
    ${proactiveBlock(cfg)}
  </div>
  <div class="pet-tabpage" data-pet-page="voice" hidden>
    ${ttsBlock(cfg)}
  </div>
  <div class="pet-tabpage" data-pet-page="persona" hidden>
    <div class="pet-persona" id="pet-persona">
      <div class="pet-debug-title">${t("pet.persona_core_title")}</div>
      <div class="pet-note">${t("pet.persona_hint")}</div>
      <textarea id="pet-persona-text" class="pet-persona-text" rows="6" spellcheck="false" maxlength="8000" placeholder=""></textarea>
      <div class="pet-row pet-persona-actions">
        <button type="button" id="pet-persona-save" class="pet-btn pet-btn-primary">${t("settings.persona_save")}</button>
        <button type="button" id="pet-persona-clear" class="pet-btn">${t("pet.persona_clear")}</button>
        <button type="button" id="pet-persona-restart" class="pet-btn">${t("settings.persona_restart")}</button>
      </div>
      <div class="pet-note" id="pet-persona-msg"></div>
    </div>
    <!-- 事实记忆 = ModuleData\memory\pet\MEMORY.md（AI 经 Remember 写） -->
    <div class="pet-list" id="pet-memory">
      <div class="pet-debug-title">${t("pet.memory_title")}</div>
      <div class="pet-note">${t("pet.memory_desc")}</div>
      <div class="pet-list-items" id="pet-memory-items"></div>
      <div class="pet-row">
        <button type="button" id="pet-memory-clear" class="pet-btn pet-list-clear" data-pet-clear="memory" data-armed-text="${esc(t("pet.list_clear_armed"))}">${esc(t("pet.list_clear"))}</button>
      </div>
      <div class="pet-note" id="pet-memory-msg"></div>
    </div>
    <!-- 成长笔记 = ModuleData\memory\pet\GROWTH.md（AI 经 PetGrowth 写）—— 与上面那份**不是一个文件** -->
    <div class="pet-list" id="pet-growth">
      <div class="pet-debug-title">${t("pet.growth_title")}</div>
      <div class="pet-note">${t("pet.growth_desc")}</div>
      <div class="pet-list-items" id="pet-growth-items"></div>
      <div class="pet-row">
        <button type="button" id="pet-growth-clear" class="pet-btn pet-list-clear" data-pet-clear="growth" data-armed-text="${esc(t("pet.list_clear_armed"))}">${esc(t("pet.list_clear"))}</button>
      </div>
      <div class="pet-note" id="pet-growth-msg"></div>
    </div>
    <!-- 人格建议 = ModuleData\memory\pet\PERSONA_SUGGESTIONS.md；「接受」= 事前确认后并入核心
         人格，「清空」= 只丢弃建议（不写核心人格）-->
    <div class="pet-list" id="pet-suggest">
      <div class="pet-debug-title">${t("pet.suggest_title")}</div>
      <div class="pet-note">${t("pet.suggest_desc")}</div>
      <div class="pet-list-items" id="pet-suggest-items"></div>
      <div class="pet-row">
        <button type="button" id="pet-suggest-clear" class="pet-btn pet-list-clear" data-pet-clear="suggest" data-armed-text="${esc(t("pet.suggest_clear_armed"))}">${esc(t("pet.list_clear"))}</button>
      </div>
      <div class="pet-note" id="pet-suggest-msg"></div>
    </div>
  </div>
</div>`;
}

/** 注入本插件唯一那份样式（两个窗口共用；`pet-` 前缀保证不外溢）。
 *
 *  **控制台也要注**（2026-09-30 补）：主窗口里原先没有任何地方注入过 `PET_CSS` ——
 *  它只写在 `attachPetWindow` 里，于是「搜索打开的桌宠面板」一直是一堆裸按钮。 */
function injectPetStyle(): void {
  if (document.getElementById("pet-style")) return;
  const style = document.createElement("style");
  style.id = "pet-style";
  style.textContent = PET_CSS;
  document.head.appendChild(style);
}

async function attachSettings(root: HTMLElement): Promise<void> {
  const el = <T extends HTMLElement>(id: string) => root.querySelector<T>(`#${id}`);

  /** 宿主语音配置缓存（`get_pet_voice` 的返回）。**提前声明**：语音语速 / 音量拉条的 handler 与
   *  `refreshPetVoiceStatus` 都要读写它，声明放在监听器注册之前以免时序竞态（接口声明在后面，类型可见）。 */
  let petVoice: PetVoiceStatus | null = null;

  /** 出错必须**上报到宿主日志**（`log_frontend`）：本文件每个处理器都自带 `.catch()`，
   *  而主窗的 `window.onerror` / `unhandledrejection` **抓不到已被 catch 的异常** ⇒
   *  出错时表现为「点了没反应，`lunac-*.log` 里也一行没有」。2026-10-09 排查
   *  「桌宠控制台按钮全部失效」时就卡在这个盲区上。 */
  const report = (what: string, e: unknown) =>
    void invoke("log_frontend", {
      level: "error",
      message: `[pet console] ${what}: ${e instanceof Error ? e.message : String(e)}`,
    }).catch(() => {});

  /** 刷新「当前用的是哪个形象」与「取消配置」按钮的可见性。
   *  `extra` 只在刚验过一个 Live2D 模型时给（那句「纹理 N · 动作组 …」）。
   *  **状态按钮的文案由 `setToggle` 管**（它要问桌宠窗活没活着，是异步的）。 */
  const renderFile = (extra = "") => {
    void refreshDebug(); // 形象换了 ⇒ 调试区（可用性 + 表情按钮）跟着刷新
    const c = loadConfig();
    const model = c.model.trim();
    const image = c.image.trim();

    const clear = el("pet-clear");
    if (clear) clear.hidden = !(model || image);

    const box = el("pet-file");
    if (!box) return;
    const tip = extra ? `<br><span class="pet-dim">${esc(extra)}</span>` : "";
    if (model) box.innerHTML = `当前用 Live2D 模型：<code>${esc(model)}</code>${tip}`;
    else if (image) box.innerHTML = `当前用图片：<code>${esc(image)}</code>${tip}`;
    else box.innerHTML = `还没有模型 —— 点「更换模型」选一个文件夹（里面要有 <code>*.model3.json</code>）。`;
  };

  async function showPet() {
    await invoke("open_plugin_window", { pluginId: "pet", key: null, input: "" });
  }

  const setToggle = (open: boolean) => {
    const b = el("pet-toggle");
    // 可见时 = 「隐藏模型」（**只隐藏窗、不销毁**，用户 2026-10-10 口径）；否则 = 开起来。
    if (b) b.textContent = open ? "隐藏模型" : "显示桌宠";
  };

  /** 刷新「鼠标穿透」切换按钮的文案与高亮。**以持久化配置为准**（两个窗口同源 localStorage，
   *  所以桌宠窗改了穿透、设置窗重开时也读得到正确态）。 */
  const renderThrough = (on: boolean) => {
    const b = el<HTMLButtonElement>("pet-through");
    if (!b) return;
    b.textContent = throughLabel(on);
    b.classList.toggle("on", on);
    b.setAttribute("aria-pressed", on ? "true" : "false");
  };

  /** 桌宠窗的状态：**广播一条 ping，收到 pong 才知道**。
   *  宿主没有「查某插件窗在不在 / 可不可见」的命令（加一条要动 Rust + 重编 lunac.exe），
   *  而 ping/pong 走已有的全局事件，零宿主改动。400ms 无应答 = 当作没开。
   *  `visible` 由桌宠窗在 pong 里带上（**隐藏 ≠ 关闭**：隐藏的窗照样应答）—— 按钮据此显示
   *  「显示桌宠 / 收起来」，并在窗口被关掉后正确回落。 */
  const pingPetState = (): Promise<{ alive: boolean; visible: boolean }> =>
    new Promise(resolve => {
      const token = `p${Date.now()}_${Math.random().toString(36).slice(2, 7)}`;
      let done = false;
      const finish = (v: { alive: boolean; visible: boolean }) => {
        if (done) return;
        done = true;
        void off.then(f => f()).catch(() => {});
        resolve(v);
      };
      const off = listen<{ type?: string; token?: string; visible?: boolean }>(CTL_EVENT, ev => {
        if (ev.payload?.type === "pong" && ev.payload?.token === token) {
          finish({ alive: true, visible: ev.payload.visible !== false });
        }
      });
      void off.then(() => emit(CTL_EVENT, { type: "ping", token })).catch(() => {});
      setTimeout(() => finish({ alive: false, visible: false }), 400);
    });

  /** 兼容旧调用点：只关心「开着吗」。 */
  const pingPet = async (): Promise<boolean> => (await pingPetState()).alive;

  const refreshToggle = () => void pingPetState().then(st => setToggle(st.alive && st.visible));

  /** 桌宠窗可能被**销毁**（宿主走的是 `destroy()`，不会补发一条可见性事件）—— 光靠事件会一直
   *  卡在「收起来」。所以设置窗开着时**每约 3s 重 ping 一次**：答复（活着 / 可见）决定按钮文案，
   *  桌宠窗关掉后没有 pong ⇒ 自动回落成「显示桌宠」（用户 2026-10-10 报的卡住问题）。 */
  const togglePoll = setInterval(refreshToggle, 3000);

  // ── 模型调试区（2026-10-09）────────────────────────────────────
  // 拉条节流：**第一次立即发**（保证跟手），之后 50ms 内的连续拖动合并成一次尾发。
  // 别每像素一条 `emit`（拖一次能刷几百条广播，桌宠窗那侧的解码/渲染全被洪水淹没）。
  // **落盘也挂在这一条尾发上**：拖动每像素写一次 localStorage 同样要命，节流后再存。
  let pendingParam: { id: string; value: number } | null = null;
  let paramTimer: ReturnType<typeof setTimeout> | null = null;
  const emitParamThrottled = (id: string, value: number) => {
    pendingParam = { id, value };
    if (paramTimer) return; // 窗口期内：只更新待发值，等尾部那次一起发
    const fire = () => {
      paramTimer = null;
      const p = pendingParam;
      pendingParam = null;
      if (p) {
        const model = loadConfig().model.trim();
        if (model) persistParam(model, p.id, p.value); // 写回持久化（按当前模型分键）
        void emit(CTL_EVENT, { type: "set-param", id: p.id, value: p.value });
      }
    };
    fire();
    paramTimer = setTimeout(fire, 50);
  };

  /** 读某模型**持久化**的调试参数（缺省 = 空对象 = 全默认）。**优先读 `lunac.params.json`
   *  里的 `params`**（AI 可直接改它、重开设置窗反向同步），文件没有 / 为空再回退配置。 */
  const savedParams = async (modelPath: string): Promise<Record<string, number>> => {
    try {
      const cache = await loadPetParams(modelPath);
      if (cache && Object.keys(cache.params).length) return cache.params;
    } catch {
      /* 读文件失败 ⇒ 回退配置 */
    }
    const p = loadConfig().params[modelPath];
    return p && typeof p === "object" ? p : {};
  };

  /** 把某模型的 params **写回文件**——**约 400ms 防抖**（拖动时 `emitParamThrottled` 每 50ms
   *  一次，别每 50ms 写一次盘）。写前先读**最新文件内容**（保住 AI 的其它改动与 coreIds），
   *  只覆盖 `params` 那一份再写；失败只 warn，绝不打断拖动。 */
  let paramFileTimer: ReturnType<typeof setTimeout> | null = null;
  let paramFileModel: string | null = null;
  const flushParamFile = () => {
    paramFileTimer = null;
    const model = paramFileModel;
    paramFileModel = null;
    if (!model) return;
    void (async () => {
      try {
        const cache = (await loadPetParams(model)) ?? emptyPetParamCache(model);
        const cur = loadConfig().params[model];
        cache.params = cur && typeof cur === "object" ? { ...cur } : {};
        await savePetParams(model, cache);
      } catch (e) {
        console.warn("[pet] 写 lunac.params.json 失败", e);
      }
    })();
  };
  const scheduleParamFile = (model: string) => {
    paramFileModel = model;
    if (paramFileTimer) return; // 窗口期内：只记住模型，等尾部那次一起写
    paramFileTimer = setTimeout(flushParamFile, 400);
  };

  /** 把某模型某个参数的当前值写回持久化（与 expressions 同一口径：按模型路径分键）。
   *  **同时更新模型目录下 `lunac-live2d\lunac.params.json`**（防抖，见 `scheduleParamFile`）。 */
  const persistParam = (modelPath: string, id: string, value: number) => {
    const c = loadConfig();
    const all = { ...c.params };
    all[modelPath] = { ...(all[modelPath] ?? {}), [id]: value };
    saveConfig({ ...c, params: all });
    scheduleParamFile(modelPath);
  };

  /** 按某模型持久化的值**恢复**调试区控件（拉条 + 数值框）：缺省用 0（引擎默认）。
   *  设置窗打开 / 换模型时调，保证「重开设置窗」看到的是上次拖到的位置。**只更新显示、不转 emit**
   *  （桌宠窗那侧在装载模型时会自行应用持久化值，这里再发一遍是多余的往返）。
   *  控件清单以**当前渲染出来的** `debugMeta` 为准（调试区列的是全部参数，不是静态那 9 个）。 */
  const restoreParams = async (modelPath: string): Promise<void> => {
    const saved = await savedParams(modelPath);
    for (const id of debugMeta.keys()) {
      const v = typeof saved[id] === "number" ? saved[id] : 0;
      const slider = root.querySelector<HTMLInputElement>(`.pet-range[data-param="${id}"]`);
      if (slider) slider.value = String(v);
      const num = root.querySelector<HTMLInputElement>(`[data-param-val="${id}"]`);
      if (num) num.value = String(v);
      // 开关形态的行没有数值框：把隐藏 range 的值反映到按钮上（≥0.5 = 开）。
      const sw = root.querySelector<HTMLElement>(`[data-param-sw="${id}"]`);
      if (sw) {
        const on = v >= 0.5;
        sw.classList.toggle("on", on);
        sw.setAttribute("aria-pressed", on ? "true" : "false");
        sw.textContent = on ? "开" : "关";
      }
    }
  };

  /** 数值框提交（Enter / 失焦触发）：夹到该参数的合法范围后 → 回填显示、同步拉条、广播。
   *  非数字 / 空 ⇒ **不提交、不发 IPC**，把框恢复成当前合法值（拉条的位置就是当前值）。
   *  `data-param-val` 与拉条的 `data-param` 是同一个 id，靠它把两者对上。 */
  const commitParamNum = (inp: HTMLInputElement) => {
    const id = inp.dataset.paramVal;
    if (!id) return;
    const meta = debugMeta.get(id);
    if (!meta) return;
    const slider = root.querySelector<HTMLInputElement>(`.pet-range[data-param="${id}"]`);
    const raw = Number.parseFloat(inp.value.trim());
    if (!Number.isFinite(raw)) {
      inp.value = slider?.value ?? "0"; // 非法：恢复成当前值，别把 NaN 传出去
      return;
    }
    // 夹到合法范围；保留 3 位小数（参数区间常是 -1..1 / 0..1，不能取整）。
    const v = Math.round(Math.min(meta.max, Math.max(meta.min, raw)) * 1000) / 1000;
    inp.value = String(v);
    if (slider) slider.value = String(v);
    emitParamThrottled(id, v);
  };

  // ── 主动反应（2026-10-09）──────────────────────────────────────
  // 设置窗只做三件事：写配置、把「今日已用」显示出来、把改动**广播**给桌宠窗即时生效。
  // 计时与真正发送在桌宠窗那侧（见 attachPetWindow）。配置与计数**共用 `CFG_KEY` 一份存储**。
  /** 广播当前主动反应配置（开关 / 间隔 / 上限 + 两个子开关及其参数）—— 桌宠窗据此重排计时器。 */
  const emitProactive = () => {
    const pr = loadConfig().proactive;
    void emit(CTL_EVENT, {
      type: "set-proactive",
      enabled: pr.enabled,
      intervalMin: pr.intervalMin,
      dailyCap: pr.dailyCap,
      autoShot: pr.autoShot,
      autoShotMin: pr.autoShotMin,
      screenWatch: pr.screenWatch,
      screenWatchSec: pr.screenWatchSec,
      screenDiff: pr.screenDiff,
    });
  };

  /** 刷新「今日已用」回显（含触顶提示）。桌宠窗每触发一次会广播 `proactive-used` 让它更新。 */
  const renderProactiveUsage = () => {
    const box = el("pet-proactive-usage");
    if (!box) return;
    const pr = loadConfig().proactive;
    const used = proactiveUsedToday();
    const base = t("pet.proactive_usage", { used: String(used), cap: String(pr.dailyCap) });
    box.textContent = used >= pr.dailyCap ? `${base} · ${t("pet.proactive_reached")}` : base;
  };

  /** 每日上限数字框提交（Enter / 失焦触发）：夹到 `PROACTIVE_CAP_MIN..PROACTIVE_CAP_MAX` 后回填 + 广播。
   *  非数字 / 空 ⇒ 不提交、不发 IPC，把框恢复成当前合法值（同 `commitParamNum` 的口径）。 */
  const commitProactiveCap = (inp: HTMLInputElement) => {
    const c = loadConfig();
    const raw = Number.parseInt(inp.value.trim(), 10);
    if (!Number.isFinite(raw)) {
      inp.value = String(c.proactive.dailyCap);
      return;
    }
    const v = Math.min(PROACTIVE_CAP_MAX, Math.max(PROACTIVE_CAP_MIN, raw));
    inp.value = String(v);
    saveConfig({ ...c, proactive: { ...c.proactive, dailyCap: v } });
    emitProactive();
    renderProactiveUsage();
  };

  /** 最小间隔数字框提交（Enter / 失焦触发）：夹到 `PROACTIVE_INTERVAL_MIN..PROACTIVE_INTERVAL_MAX` 后
   *  回填 + 广播（桌宠窗据此重排计时器）。非数字 / 空 ⇒ 不提交、恢复成当前合法值（同 cap 口径）。
   *  （2026-10-09 用户口径：间隔改成**可键入的自定义值**、且要能调小 —— 原来只有 15/30/60/120 四档。） */
  const commitProactiveInterval = (inp: HTMLInputElement) => {
    const c = loadConfig();
    const raw = Number.parseInt(inp.value.trim(), 10);
    if (!Number.isFinite(raw)) {
      inp.value = String(c.proactive.intervalMin);
      return;
    }
    const v = Math.min(PROACTIVE_INTERVAL_MAX, Math.max(PROACTIVE_INTERVAL_MIN, raw));
    inp.value = String(v);
    saveConfig({ ...c, proactive: { ...c.proactive, intervalMin: v } });
    emitProactive();
  };

  /** 通用的「主动反应数字框」提交：解析 → 夹到 `[min, max]` → 回填 → 落盘 → 广播。
   *  非数字 / 空 ⇒ 不提交、把框恢复成当前值（同 cap / interval 口径）。 */
  const commitProactiveNum = (
    inp: HTMLInputElement,
    pick: (pr: ProactiveCfg) => number,
    min: number,
    max: number,
    apply: (pr: ProactiveCfg, v: number) => ProactiveCfg,
  ) => {
    const c = loadConfig();
    const raw = Number.parseInt(inp.value.trim(), 10);
    if (!Number.isFinite(raw)) {
      inp.value = String(pick(c.proactive));
      return;
    }
    const v = Math.min(max, Math.max(min, raw));
    inp.value = String(v);
    saveConfig({ ...c, proactive: apply(c.proactive, v) });
    emitProactive();
  };

  // ── 桌宠专属人格（2026-10-09，方案 A）──────────────────────────
  // 编辑 `config\pet-persona.md`：**只作用于「桌宠发起的那一轮」** —— 前端发送该轮时把它
  // 拼进该轮的用户消息（见 main.ts 的 startAgentChat 注入点）。因此**保存即时生效、不必重启
  // agent**；面板上那条「重启 AI」按钮只是与 AI 侧人格窗口**功能对齐**（走主窗的同一套重启路径）。
  const AGENT_RESTART_EVENT = "pet-agent-restart";
  const AGENT_RESTART_DONE_EVENT = "pet-agent-restart-done";
  // 委托的 `onClick` 里 `t` 被**事件目标**占用了（`const t = ev.target`），所以这几条在闭包内
  // 不能直接调 i18n 的 `t(...)` —— 提前在本作用域算好常量。
  const MSG_PERSONA_SAVED = t("settings.persona_saved");
  const MSG_PERSONA_CLEARED = t("pet.persona_cleared");
  const MSG_PERSONA_RESTARTING = t("pet.persona_restarting");

  const setPersonaMsg = (s: string) => {
    const m = el("pet-persona-msg");
    if (m) m.textContent = s;
  };

  /** 装载人格全文（面板打开时一次）。长度上限的唯一真相源是宿主，这里只做 `maxlength` 回显。 */
  const loadPetPersona = async () => {
    try {
      const st = await invoke<{ text?: string; maxChars?: number }>("get_pet_persona");
      const ta = el<HTMLTextAreaElement>("pet-persona-text");
      if (ta) {
        if (st?.maxChars) ta.maxLength = st.maxChars;
        ta.value = st?.text ?? "";
      }
    } catch (e) {
      report("读取桌宠人格失败", e);
    }
  };

  const savePetPersona = async (text: string, okMsg: string) => {
    try {
      await invoke("set_pet_persona", { text });
      setPersonaMsg(okMsg + t("pet.persona_effective"));
    } catch (e) {
      setPersonaMsg(t("settings.persona_failed", { err: String(e) }));
    }
  };

  // ── 人格页三张列表：事实记忆 / 成长笔记 / 人格建议（2026-10-13）──────────
  // 全部只调**既有**宿主命令，不新增落盘路径、不改命令语义：
  //   · 事实记忆 → `pet\MEMORY.md`（get_pet_memory / delete_pet_memory_note / clear_pet_memory）
  //   · 成长笔记 → `pet\GROWTH.md`（get_pet_growth / delete_pet_growth_note / clear_pet_growth）
  //   · 人格建议 → `pet\PERSONA_SUGGESTIONS.md`（get_persona_suggestions / delete_persona_suggestion
  //     / clear_persona_suggestions / merge_persona_suggestion）
  // 逐条删按**内容**匹配（与宿主 `delete_*_entry` 同口径）；「接受」= **事前**二次确认后并入核心
  // 人格（§8），「清空」= 只是丢弃建议（不写核心人格）—— 两者都走两段式确认。
  type PetListKind = "memory" | "growth" | "suggest";
  /** 三张列表的命令与容器 id（唯一映射，别处不再抄第二份）。三张都有「清空」（`clear` 非空）。
   *  `cleared` = 清空成功后的轻提示 i18n key（三张各自区分；建议那句写明「不改核心人格」）。 */
  const PET_LIST: Record<PetListKind, { get: string; del: string; clear: string | null; cleared: string; merge: boolean; items: string; msg: string }> = {
    memory: { get: "get_pet_memory", del: "delete_pet_memory_note", clear: "clear_pet_memory", cleared: "pet.memory_cleared", merge: false, items: "pet-memory-items", msg: "pet-memory-msg" },
    growth: { get: "get_pet_growth", del: "delete_pet_growth_note", clear: "clear_pet_growth", cleared: "pet.growth_cleared", merge: false, items: "pet-growth-items", msg: "pet-growth-msg" },
    suggest: { get: "get_persona_suggestions", del: "delete_persona_suggestion", clear: "clear_persona_suggestions", cleared: "pet.suggest_cleared", merge: true, items: "pet-suggest-items", msg: "pet-suggest-msg" },
  };

  const petListMsg = (k: PetListKind, text: string, bad = false) => {
    const m = el(PET_LIST[k].msg);
    if (!m) return;
    m.className = bad ? "pet-note pet-bad" : "pet-note";
    m.textContent = text;
  };

  /** 两段式确认（同 `main.ts` 回退按钮口径）：第一次点置 `data-armed="1"` 并把文案换成
   *  `data-armed-text`（写明将要发生什么），4 秒内再点才真执行；超时自动解除。
   *  **不用原生 `confirm`**（全仓一贯禁用）。 */
  const armedRun = (btn: HTMLElement, run: () => void) => {
    if (btn.dataset.armed === "1") {
      delete btn.dataset.armed;
      btn.textContent = btn.dataset.armedRestore ?? btn.textContent;
      delete btn.dataset.armedRestore;
      run();
      return;
    }
    btn.dataset.armedRestore = btn.textContent ?? "";
    btn.dataset.armed = "1";
    const armed = btn.dataset.armedText;
    if (armed) btn.textContent = armed;
    window.setTimeout(() => {
      if (btn.dataset.armed !== "1") return;
      delete btn.dataset.armed;
      btn.textContent = btn.dataset.armedRestore ?? btn.textContent;
      delete btn.dataset.armedRestore;
    }, 4000);
  };

  /** 把一个条目文本画进列表（`textContent` 赋值 ⇒ 不解析 HTML；`dataset` 存原文供删除 / 合并用）。 */
  const renderPetList = (k: PetListKind, entries: string[]) => {
    const box = el(PET_LIST[k].items);
    if (!box) return;
    const frag = document.createDocumentFragment();
    if (!entries.length) {
      const empty = document.createElement("div");
      empty.className = "pet-list-empty";
      empty.textContent = t(`pet.${k}_empty`);
      frag.appendChild(empty);
      box.replaceChildren(frag);
      return;
    }
    for (const text of entries) {
      const row = document.createElement("div");
      row.className = "pet-list-row";
      const txt = document.createElement("div");
      txt.className = "pet-list-text";
      txt.textContent = text;
      row.appendChild(txt);
      const acts = document.createElement("div");
      acts.className = "pet-list-acts";
      if (PET_LIST[k].merge) {
        const acc = document.createElement("button");
        acc.type = "button";
        acc.className = "pet-btn pet-suggest-accept";
        acc.dataset.petMerge = text;
        acc.dataset.armedText = t("pet.suggest_accept_armed");
        acc.textContent = t("pet.suggest_accept");
        acts.appendChild(acc);
      }
      const del = document.createElement("button");
      del.type = "button";
      del.className = "pet-btn pet-list-del";
      del.dataset.petDel = text;
      del.dataset.petList = k;
      del.dataset.armedText = t("pet.item_del_armed");
      del.textContent = t("pet.item_del");
      acts.appendChild(del);
      row.appendChild(acts);
      frag.appendChild(row);
    }
    box.replaceChildren(frag);
  };

  /** 读取一张列表；**失败也要在列表位置给出明确提示**（不白屏）。 */
  const refreshPetList = async (k: PetListKind): Promise<void> => {
    try {
      const p = await invoke<{ entries?: string[] }>(PET_LIST[k].get);
      renderPetList(k, p.entries ?? []);
    } catch (e) {
      report(`读取列表失败（${k}）`, e);
      const box = el(PET_LIST[k].items);
      if (box) {
        const fail = document.createElement("div");
        fail.className = "pet-list-empty pet-bad";
        fail.textContent = t("pet.list_load_fail");
        box.replaceChildren(fail);
      }
    }
  };

  /** 逐条删除（按内容匹配）；条目消失即最好反馈 ⇒ 成功不弹提示，失败才写一行。 */
  const deletePetListEntry = async (k: PetListKind, text: string): Promise<void> => {
    try {
      await invoke(PET_LIST[k].del, { text });
      petListMsg(k, "");
      await refreshPetList(k);
    } catch (e) {
      report(`删除列表条目失败（${k}）`, e);
      petListMsg(k, t("pet.list_fail"), true);
    }
  };

  /** 清空一张列表（仅对宿主提供 `clear_*` 的列表可用）。成功后刷新列表并给一句轻提示
   *  （文案取自 `PET_LIST[k].cleared`，三张表各自区分；人格建议那句写明「不改核心人格」）。 */
  const clearPetList = async (k: PetListKind): Promise<void> => {
    const cmd = PET_LIST[k].clear;
    if (!cmd) return;
    try {
      await invoke(cmd);
      petListMsg(k, t(PET_LIST[k].cleared));
      await refreshPetList(k);
    } catch (e) {
      report(`清空列表失败（${k}）`, e);
      petListMsg(k, t("pet.list_fail"), true);
    }
  };

  /** 「接受」一条人格建议 = **事前二次确认后**把它并入核心人格（`merge_persona_suggestion`）。
   *  合并成功 ⇒ 核心人格文本域刷新 + 建议列表刷新。 */
  const mergePetSuggestion = async (text: string): Promise<void> => {
    try {
      await invoke("merge_persona_suggestion", { text });
      await loadPetPersona();
      await refreshPetList("suggest");
      petListMsg("suggest", t("pet.suggest_accepted"));
    } catch (e) {
      report("合并人格建议失败", e);
      petListMsg("suggest", t("pet.merge_fail"), true);
    }
  };

  const showDebugNote = (msg: string) => {
    const note = el("pet-debug-note");
    if (!note) return;
    note.textContent = msg;
    note.hidden = !msg;
  };

  // ── 表情「开关态」+ 持久化（2026-10-09）────────────────────────────
  // 当前生效的表情 id（空串 = 没有高亮 / 已还原）。**以持久化为准**（不再只活在设置窗闭包里）：
  // 高亮的依据是 `PetConfig.expressions[当前模型]`，两个窗口同源 localStorage 都读得到，
  // 所以桌宠窗没开、设置窗被重建时，UI 仍能显示正确的高亮。同一时刻至多一个高亮 ——
  // 判定就是「按钮的 `data-expr` 等于 `activeExpr`」。
  let activeExpr = "";

  /** 读某模型**持久化**的表达 id（空串 = 无）。 */
  const savedExpr = (modelPath: string): string => {
    const v = loadConfig().expressions[modelPath];
    return typeof v === "string" ? v : "";
  };

  /** 把某模型当前的表达 id 写回持久化（`id` 为空则删掉该键 = 无表情）。 */
  const persistExpr = (modelPath: string, id: string) => {
    const c = loadConfig();
    const map = { ...c.expressions };
    if (id) map[modelPath] = id;
    else delete map[modelPath];
    saveConfig({ ...c, expressions: map });
  };

  /** 给表情按钮打高亮：只有 `activeExpr` 那一个加 `.on`，其余（含「还原」）都不加。 */
  const applyExprHighlight = () => {
    const box = el("pet-exprs");
    if (!box) return;
    box.querySelectorAll<HTMLElement>("[data-expr]").forEach(b => {
      b.classList.toggle("on", !!activeExpr && b.dataset.expr === activeExpr);
    });
  };

  /** 切到某个表情（空串 = 还原）：改高亮、**按当前模型写回持久化**、再广播给桌宠窗。
   *  桌宠窗那侧对空串走 `resetExpressions()`（见 attachPetWindow 的 `set-expression` 分支）。 */
  const setExpr = (id: string) => {
    activeExpr = id;
    applyExprHighlight();
    const model = loadConfig().model.trim();
    if (model) persistExpr(model, id);
    void emit(CTL_EVENT, { type: "set-expression", id });
  };

  /** 列出当前模型**可用的全部表情**（声明 + 目录里未声明的），**每个一个按钮**，外加一个「还原」。
   *  列表来自 `live2d.ts` 的统一助手（引擎和这里同一份，见 `listExpressions`）；
   *  读不到 / 没表情时**如实说清**，不留空白。按钮走事件委托（见下）。 */
  async function renderExpressions(modelPath: string): Promise<void> {
    const box = el("pet-exprs");
    if (!box) return;
    // 打开 / 换模型：先按该模型持久化的值恢复高亮（桌宠窗没开也能显示正确状态）
    activeExpr = savedExpr(modelPath);
    let list: Awaited<ReturnType<typeof listExpressions>> = [];
    try {
      list = await listExpressions(modelPath);
    } catch {
      box.innerHTML = `<span class="pet-bad">${esc(t("pet.debug_expr_read_fail"))}</span>`;
      return;
    }
    if (!list.length) {
      box.innerHTML = `<span class="pet-dim">${esc(t("pet.debug_no_expressions"))}</span>`;
      return;
    }
    // `data-expr=""`（空串）= 还原 —— 与「某个表情」用同一个属性分流，委托只认属性存在与否。
    box.innerHTML =
      list
        .map(
          e =>
            `<button type="button" class="pet-expr-btn" data-expr="${esc(e.id)}">${esc(e.name)}</button>`,
        )
        .join("") +
      `<button type="button" class="pet-expr-btn pet-expr-reset" data-expr="">${esc(t("pet.debug_reset"))}</button>`;
    // 重建出来的按钮是「无态」的：按上面读出的 `activeExpr` 把高亮补回去。
    applyExprHighlight();
  }

  /** 一行参数控件的渲染选项。 */
  interface ParamRowOpts {
    /** 该参数在运行期 Core 里的范围（取不到 = undefined）。 */
    range?: { min: number; max: number };
    /** 物理**输入**参数（`.physics3.json` 的输入集）。 */
    physIn: boolean;
    /** 物理**输出**参数（被物理写值的那些）。 */
    physOut: boolean;
    /** cdi3.json 的可读名（静态表没有时用）。 */
    friendly: string;
    /** 开关形态（布尔 / 0-1 切换）⇒ 画成开 / 关按钮。 */
    isSwitch: boolean;
  }

  /** 一行参数控件。`range` 给定时用它定区间，否则退回静态表 / 通用区间（-100..100）。
   *  形态优先级：**开关**（布尔 / 0-1）> 连续滑条。
   *  物理输入 / 输出的参数额外挂一个小标记；末尾那个小按钮可**手动覆盖**参数形态。 */
  const paramRowHtml = (id: string, o: ParamRowOpts): string => {
    const known = DEBUG_PARAMS.find(d => d.id === id);
    const min = o.range ? o.range.min : known ? known.min : -100;
    const max = o.range ? o.range.max : known ? known.max : 100;
    const span = max - min;
    const step = span <= 2 ? 0.01 : span <= 20 ? 0.1 : 1;
    debugMeta.set(id, { min, max });
    // 名字三级：静态表的中文名 > cdi3.json 的可读名 > 原始 id（`title` 始终给原始 id）。
    const label = known ? t(known.labelKey) : o.friendly || id;
    const tag = o.physIn
      ? `<span class="pet-phys-tag">${esc(t("pet.debug_phys_in"))}</span>`
      : o.physOut
        ? `<span class="pet-phys-tag">${esc(t("pet.debug_phys_out"))}</span>`
        : "";
    const kindBtn = `<button type="button" class="pet-param-kind" data-param-kind="${esc(id)}" data-kind="${o.isSwitch ? "switch" : "range"}" title="切换控件形态（开关 / 拉条）">${KIND_ICON}</button>`;
    // 开关行：留一个**隐藏的 range** 当值容器（`restoreParams` / 委托都按 `.pet-range[data-param]` 取），
    // 可见的是那颗开 / 关按钮。
    if (o.isSwitch) {
      return `<div class="pet-param">
        <span class="pet-label" title="${esc(id)}">${esc(label)}${tag}</span>
        <input type="range" class="pet-range" data-param="${esc(id)}" min="0" max="1" step="1" value="0" hidden>
        <button type="button" class="pet-btn pet-btn-toggle pet-param-on" data-param-sw="${esc(id)}" aria-pressed="false">关</button>
        ${kindBtn}
      </div>`;
    }
    return `<div class="pet-param">
      <span class="pet-label" title="${esc(id)}">${esc(label)}${tag}</span>
      <input type="range" class="pet-range" data-param="${esc(id)}" min="${min}" max="${max}" step="${step}" value="0">
      <input type="text" class="pet-param-num" data-param-val="${esc(id)}" inputmode="decimal" autocomplete="off" spellcheck="false" value="0" aria-label="${esc(id)}">
      ${kindBtn}
    </div>`;
  };

  /** 刷新整个调试区：列出**自由参数**（模型全部参数 − 已绑定热键 − 已绑定 VTS 追踪）+ 物理标记。
   *  模型模式才可用（图片模式整块禁用 + 说明）；桌宠没开就提示「先显示桌宠」。 */
  async function refreshDebug(): Promise<void> {
    const c = loadConfig();
    const hasModel = !!c.model.trim();
    const body = el("pet-debug-body");
    body?.classList.toggle("off", !hasModel);
    if (!hasModel || !body) {
      showDebugNote(t("pet.debug_need_model"));
      return;
    }

    const model = c.model.trim();
    const core = await readCoreIdsAsync(model);
    const side = await findModelSideFiles(model); // vtube.json / cdi3.json
    const cdi3 = side.cdi3 ? await readCdi3Names(side.cdi3) : EMPTY_CDI3;
    // 参数 / 物理输入输出与热键页**同一份枚举**（模型文件 + 运行时 id 表现取现算）。
    let targets: HotkeyTargets | null = null;
    try {
      targets = await enumerateTargets(model, core);
    } catch {
      targets = null;
    }
    const physIn = new Set(targets?.physicsInputs ?? []);
    const physOut = new Set(targets?.physicsOutputs ?? []);

    // **精简**（用户 2026-10-10 口径）：来源 = 运行时 id 表（拿不到时退回静态 9 个常用项），
    // 但**减去两类「已有归属」的参数** —— 它们在别处被驱动，铺在调试区只是噪音：
    //   ① 绑定到热键的（交给「热键」页）；② 绑定到 VTS 追踪的（`vtube.json` 的
    //      `ParameterSettings[].OutputLive2D`；用户实测该文件里 20 条全是追踪绑定）。
    // `cdi3.json` 只用来给名字，不额外造参数。
    const merged: string[] = [];
    const push = (id: string) => {
      if (id && !merged.includes(id)) merged.push(id);
    };
    const runtime = core.parameters.length ? core.parameters : DEBUG_PARAMS.map(p => p.id);
    for (const p of DEBUG_PARAMS) if (runtime.includes(p.id)) push(p.id); // 常用的排前面
    for (const id of runtime) push(id);
    const boundHotkey = await collectExpressionParamIds(model);
    const boundVts = new Set(side.vtube ? await readVtsParamIds(side.vtube) : []);
    const ids = merged.filter(id => !boundHotkey.has(id) && !boundVts.has(id));

    // **开关判定**：区间正好 0..1（Live2D 官方参数表的约定：`スイッチON/OFF は 0 ～ 1`）
    // **且**模型的动作曲线里该参数只取 0/1（连续量会出现中间值）。没证据时按区间算；
    // 手动覆盖（`paramKinds`）优先。
    const overrides = c.paramKinds[model] ?? {};
    const motionValues = await collectSwitchEvidence(model, side.dir);
    const isSwitch = (id: string): boolean => {
      const ov = overrides[id];
      if (ov) return ov === "switch";
      const r = core.parameterRanges[id];
      if (!r || r.min !== 0 || r.max !== 1) return false;
      const seen = motionValues.get(id);
      if (!seen || !seen.size) return true;
      for (const v of seen) if (v !== 0 && v !== 1) return false;
      return true;
    };

    debugMeta = new Map();
    body.innerHTML = ids
      .map(id =>
        paramRowHtml(id, {
          range: core.parameterRanges[id],
          physIn: physIn.has(id),
          physOut: physOut.has(id),
          friendly: cdi3.parameters[id] ?? "",
          isSwitch: isSwitch(id),
        }),
      )
      .join("");
    // 打开设置窗 / 换模型：把该模型**持久化**的参数恢复进拉条与数值框（重开设置窗不丢）
    await restoreParams(model);
    // 桌宠没开时：拖动仍会记住数值并广播，但看不到效果 —— 必须说清（值不会丢，只是没处显示）
    showDebugNote((await pingPet()) ? "" : t("pet.debug_pet_closed"));
  }

  // ── 「恢复默认」（2026-10-10 用户口径：三个分页最下方各一颗）──────────
  /** 调试页：清掉该模型的参数覆盖 ⇒ 控件与模型都回默认（0，与 `restoreParams` 同口径）。
   *  面板可能列了几百个参数，所以走**一条**批量广播（`reset-params`），不是每个参数一条。 */
  const resetDebugParams = async (): Promise<void> => {
    const c = loadConfig();
    const model = c.model.trim();
    if (!model) return;
    const all = { ...c.params };
    delete all[model];
    saveConfig({ ...c, params: all });
    // 文件是**单一入口** ⇒ 同步清掉它的 `params`（否则 `restoreParams` 又会把旧值读回来）。
    try {
      const cache = await loadPetParams(model);
      if (cache) {
        cache.params = {};
        await savePetParams(model, cache);
      }
    } catch (e) {
      console.warn("[pet] 清 lunac.params.json 的 params 失败", e);
    }
    void emit(CTL_EVENT, { type: "reset-params", ids: [...debugMeta.keys()] });
    await restoreParams(model);
    showDebugNote("参数已恢复默认。");
  };

  /** 热键页：清空热键表（落盘为空）。要重建就再点上面的「重新导入（模型文件夹）」。 */
  const resetHotkeys = () => {
    const model = loadConfig().model.trim();
    if (!model) return;
    hkList = [];
    hkActive = new Set();
    renderHotkeys();
    void saveHotkeys(model, []).catch(e => report("清空热键失败", e));
    hkMsg("已清空热键表。");
  };

  /** 动作页：删掉本插件**生成**的 `lunac_*.motion3.json`（**绝不动**作者/用户自己的动作）。 */
  const resetMotions = () => {
    void (async () => {
      const model = loadConfig().model.trim();
      if (!model) return;
      const mine = moFiles.filter(f => f.name.toLowerCase().startsWith(MO_GEN_PREFIX));
      if (!mine.length) {
        moMsg("没有本插件生成的动作文件（只删 lunac_ 开头的）。");
        return;
      }
      let n = 0;
      for (const f of mine) {
        try {
          await invoke("delete_motion_file", { modelPath: model, name: f.path.split(/[\\/]/).pop() || "" });
          n++;
        } catch (e) {
          report("删除动作文件失败", e);
        }
      }
      await refreshMotions();
      pruneIdleMotions(); // 删掉的文件若被勾过「待机循环」⇒ 顺手摘掉（不留指向不存在文件的死项）
      void emit(CTL_EVENT, { type: "reload" });
      moMsg(`已删除 ${n} 个生成的动作文件。`);
    })();
  };

  // ── 事件委托（2026-10-09 修「导入模型后所有按钮失效」）──────────────
  // **为什么改成委托**：这些按钮原来各绑各的 `addEventListener`，一旦面板 HTML 被整体
  // 重建（`resultsList.innerHTML = …`），监听器就随旧节点一起没了 —— 表现出来正是
  // 「面板还在、每个按钮点了都没反应」。委托挂在 **`root`（结果区那个稳定容器）** 上，
  // 面板怎么重建都不受影响；`closest()` 只认 pet 自己的 id，不会误伤别插件的面板。
  // 幂等（`DELEG` 标记）：重复 attach（重渲染）不会把同一批监听挂两遍。
  const DELEG = "__lunacPetConsoleDelegate";
  const marked = root as unknown as Record<string, unknown>;
  if (!marked[DELEG]) {
    marked[DELEG] = true;

    const onClick = (ev: Event) => {
      const t = ev.target as HTMLElement | null;
      if (!t) return;
      // 分页切换（模型调试 / 热键）：两页互斥，照 `.music-tab` 的语言（胶囊芯片 + on 选中态）。
      const tab = t.closest<HTMLElement>("[data-pet-tab]");
      if (tab) {
        const page = tab.dataset.petTab;
        root.querySelectorAll<HTMLElement>(".pet-tab").forEach(b => {
          b.classList.toggle("on", b.dataset.petTab === page);
        });
        root.querySelectorAll<HTMLElement>(".pet-tabpage").forEach(p => {
          p.hidden = p.dataset.petPage !== page;
        });
        return;
      }
      if (t.closest("#pet-toggle")) {
        // 一个**状态按钮**：**可见**着就**隐藏**（只收窗、**不销毁插件** —— 用户 2026-10-10 口径：
        // 原来这里调 `close_plugin_window` 会把整个桌宠窗销毁掉，看起来像「关闭了整个插件」）；
        // 没开 / 被隐藏就开（开之前先确保有模型）。
        void (async () => {
          const st = await pingPetState();
          if (st.alive && st.visible) {
            void emit(CTL_EVENT, { type: "hide" }).catch(e => report("隐藏模型失败", e));
            setToggle(false);
            return;
          }
          // 窗还在、只是被隐藏 ⇒ 让**它自己** `show()` 回来。**不走** `open_plugin_window`：
          // 那条复用路径会推 `plugin-window-input` ⇒ 桌宠窗整页重挂、热键开关等运行时状态丢失。
          if (st.alive) {
            void emit(CTL_EVENT, { type: "show" }).catch(e => report("显示模型失败", e));
            setToggle(true);
            void refreshDebug();
            return;
          }
          const c = loadConfig();
          if (!c.model.trim() && !c.image.trim() && !(await pickShape())) return;
          await showPet();
          setToggle(true);
          void refreshDebug(); // 桌宠开着之后，把「先显示桌宠」那句提示撤掉
        })().catch(e => report("显示桌宠失败", e));
      } else if (t.closest("#pet-change")) {
        void pickShape().catch(e => report("更换形象失败", e));
      } else if (t.closest("#pet-clear")) {
        saveConfig({ ...loadConfig(), model: "", image: "" });
        renderFile();
        void emit(CTL_EVENT, { type: "reload" });
        void refreshHotkeys(); // 清空形象 ⇒ 热键页也清空（没有模型就没有可绑的目标）
        void refreshMotions(); // 预设动作页同理
      } else if (t.closest("#pet-through")) {
        // **切换按钮**（2026-10-09 用户口径）：原来是个复选框、桌宠侧又只有「开启」一条单向动作
        // ⇒ 改成按钮后开 / 关都在同一个控件上，且两扇窗**显示同一份状态**（对齐）。
        const on = !loadConfig().clickThrough;
        saveConfig({ ...loadConfig(), clickThrough: on });
        renderThrough(on);
        void emit(CTL_EVENT, { type: "set-click-through", value: on });
      } else if (t.closest("#pet-proactive-enabled")) {
        // 开关**按钮**（2026-10-10 用户口径：勾选框一律改开关按钮）：点一下开 / 关。
        const btn = t.closest<HTMLElement>("#pet-proactive-enabled");
        if (!btn) return;
        const on = !btn.classList.contains("on");
        const c = loadConfig();
        saveConfig({ ...c, proactive: { ...c.proactive, enabled: on } });
        btn.classList.toggle("on", on);
        btn.setAttribute("aria-pressed", on ? "true" : "false");
        btn.textContent = onOffText(on); // 只显示「开 / 关」（名字在左侧 pet-label 上）
        emitProactive(); // 桌宠窗据此即时启停计时器
        renderProactiveUsage(); // 触顶提示会随上限 / 开关变化
      } else if (t.closest("#pet-autoshot-enabled")) {
        // 「自动抓屏」子开关（默认关的**显式 opt-in**；隐私红线：只有真开着才抓屏）。
        const btn = t.closest<HTMLElement>("#pet-autoshot-enabled");
        if (!btn) return;
        const on = !btn.classList.contains("on");
        const c = loadConfig();
        saveConfig({ ...c, proactive: { ...c.proactive, autoShot: on } });
        btn.classList.toggle("on", on);
        btn.setAttribute("aria-pressed", on ? "true" : "false");
        btn.textContent = onOffText(on);
        emitProactive(); // 桌宠窗据此启停自动抓屏计时器
      } else if (t.closest("#pet-screenwatch-enabled")) {
        // 「屏幕变化触发」子开关（默认关的**显式 opt-in**）。
        const btn = t.closest<HTMLElement>("#pet-screenwatch-enabled");
        if (!btn) return;
        const on = !btn.classList.contains("on");
        const c = loadConfig();
        saveConfig({ ...c, proactive: { ...c.proactive, screenWatch: on } });
        btn.classList.toggle("on", on);
        btn.setAttribute("aria-pressed", on ? "true" : "false");
        btn.textContent = onOffText(on);
        emitProactive(); // 桌宠窗据此启停屏幕变化轮询
      } else if (t.closest("#pet-tts-enabled")) {
        // 语音朗读开关：**只写配置**即可 —— 桌宠窗每次收到回复都重读配置（同源 localStorage）。
        const btn = t.closest<HTMLElement>("#pet-tts-enabled");
        if (!btn) return;
        const on = !btn.classList.contains("on");
        saveConfig({ ...loadConfig(), tts: on });
        btn.classList.toggle("on", on);
        btn.setAttribute("aria-pressed", on ? "true" : "false");
        btn.textContent = toggleLabel("pet.tts_enable", on);
      } else if (t.closest("#pet-tts-addfile")) {
        // 参考音色导入（2026-10-12）：选文件 → 宿主导入命令（校验 + 缓存）→ `set_pet_voice` 落盘。
        void addPetVoiceAudio().catch(e => report("导入参考音色失败", e));
      } else if (t.closest("#pet-tts-preview")) {
        previewPetVoice();
      } else if (t.closest("[data-param-sw]")) {
        // 调试区的**开关行**：点一下在 0 / 1 之间切（隐藏 range 当值容器，走同一条节流广播）。
        if (el("pet-debug-body")?.classList.contains("off")) return;
        const btn = t.closest<HTMLElement>("[data-param-sw]");
        const id = btn?.dataset.paramSw ?? "";
        if (!btn || !id) return;
        const on = !btn.classList.contains("on");
        btn.classList.toggle("on", on);
        btn.setAttribute("aria-pressed", on ? "true" : "false");
        btn.textContent = on ? "开" : "关";
        const slider = root.querySelector<HTMLInputElement>(`.pet-range[data-param="${id}"]`);
        if (slider) slider.value = on ? "1" : "0";
        emitParamThrottled(id, on ? 1 : 0);
      } else if (t.closest("[data-param-kind]")) {
        // 参数形态的**手动覆盖**（自动判定不可能全对）：在「开关 / 连续」之间切，按模型持久化。
        const btn = t.closest<HTMLElement>("[data-param-kind]");
        const id = btn?.dataset.paramKind ?? "";
        const model = loadConfig().model.trim();
        if (!btn || !id || !model) return;
        const c = loadConfig();
        const all = { ...c.paramKinds };
        const cur = btn.dataset.kind === "switch" ? "switch" : "range";
        all[model] = { ...(all[model] ?? {}), [id]: cur === "switch" ? "range" : "switch" };
        saveConfig({ ...c, paramKinds: all });
        void refreshDebug(); // 形态变了 ⇒ 整块重画
      } else if (t.closest("#pet-persona-save")) {
        const ta = el<HTMLTextAreaElement>("pet-persona-text");
        void savePetPersona(ta?.value ?? "", MSG_PERSONA_SAVED).catch(e => report("保存桌宠人格失败", e));
      } else if (t.closest("#pet-persona-clear")) {
        const ta = el<HTMLTextAreaElement>("pet-persona-text");
        if (ta) ta.value = "";
        void savePetPersona("", MSG_PERSONA_CLEARED).catch(e => report("清空桌宠人格失败", e));
      } else if (t.closest("#pet-persona-restart")) {
        // 「重启 AI」按钮：桌宠窗没有 `__lunac_reload_agent` 桥 ⇒ 广播给主窗执行（见 main.ts）。
        setPersonaMsg(MSG_PERSONA_RESTARTING);
        void emit(AGENT_RESTART_EVENT).catch(e => report("重启 AI 请求失败", e));
      } else if (t.closest("[data-pet-merge]")) {
        // 「接受」人格建议：**先二次确认**（`armedRun` 换文案「将把这条并入核心人格」），再动核心人格。
        const btn = t.closest<HTMLElement>("[data-pet-merge]");
        if (!btn) return;
        const text = btn.dataset.petMerge ?? "";
        armedRun(btn, () => {
          if (text) void mergePetSuggestion(text).catch(e => report("合并人格建议失败", e));
        });
      } else if (t.closest("[data-pet-del]")) {
        // 逐条删：同样两段式确认（列表条目不少，误点一下不该直接删掉）。
        const btn = t.closest<HTMLElement>("[data-pet-del]");
        if (!btn) return;
        const kind = btn.dataset.petList as PetListKind | undefined;
        const text = btn.dataset.petDel ?? "";
        if (!kind || !PET_LIST[kind]) return;
        armedRun(btn, () => {
          if (text) void deletePetListEntry(kind, text).catch(e => report("删除列表条目失败", e));
        });
      } else if (t.closest("[data-pet-clear]")) {
        // 三张列表（事实记忆 / 成长笔记 / 人格建议）统一走这一支：读 `data-pet-clear` 得到列表类别，
        // 复用 `armedRun` **两段式确认**（4s 超时自动解除）。「清空」= 只丢弃该列表内容；各表成功后
        // 的轻提示取自 `PET_LIST[k].cleared`，其中人格建议那句写明「不改核心人格」（与「接受」区分）。
        const btn = t.closest<HTMLElement>("[data-pet-clear]");
        if (!btn) return;
        const kind = btn.dataset.petClear as PetListKind | undefined;
        if (!kind || !PET_LIST[kind]) return;
        armedRun(btn, () => void clearPetList(kind).catch(e => report(`清空列表失败（${kind}）`, e)));
      } else if (t.closest("#pet-reset-debug")) {
        void resetDebugParams();
      } else if (t.closest("#pet-reset-hotkeys")) {
        resetHotkeys();
      } else if (t.closest("#pet-reset-motions")) {
        resetMotions();
      } else {
        // 表情按钮 = **开关态按钮**（`data-expr="某表情"`）。「还原」那枚 `data-expr=""` 与
        // 「点已生效的表情」是同一种语义（都=取消高亮），所以两者都收敛到 `setExpr("")`，
        // 不会打架；保留它只是省去「先找到那个高亮按钮再点它」这一步。
        const exprBtn = t.closest<HTMLElement>("[data-expr]");
        if (exprBtn) {
          const id = exprBtn.dataset.expr ?? "";
          setExpr(id && id === activeExpr ? "" : id); // 已生效 → 取消；未生效 / 还原 → 生效（或还原）
        }
      }
    };

    const onChange = (ev: Event) => {
      const t = ev.target as HTMLElement | null;
      if (!t) return;
      // 主动反应 / 语音朗读的开关已改成**按钮**，走 onClick（见上）；这里只剩文本框类。
      if (t.id === "pet-proactive-cap") {
        commitProactiveCap(t as HTMLInputElement);
        return;
      }
      if (t.id === "pet-proactive-interval") {
        commitProactiveInterval(t as HTMLInputElement);
        return;
      }
      if (t.id === "pet-autoshot-min") {
        commitProactiveNum(t as HTMLInputElement, pr => pr.autoShotMin, AUTOSHOT_MIN_MIN, AUTOSHOT_MIN_MAX, (pr, v) => ({ ...pr, autoShotMin: v }));
        return;
      }
      if (t.id === "pet-watch-sec") {
        commitProactiveNum(t as HTMLInputElement, pr => pr.screenWatchSec, WATCH_SEC_MIN, WATCH_SEC_MAX, (pr, v) => ({ ...pr, screenWatchSec: v }));
        return;
      }
      if (t.id === "pet-watch-diff") {
        commitProactiveNum(t as HTMLInputElement, pr => pr.screenDiff, WATCH_DIFF_MIN, WATCH_DIFF_MAX, (pr, v) => ({ ...pr, screenDiff: v }));
        return;
      }
      // 语音「参考音色」的参考文本：失焦提交（= 用户改完那句原话）。
      if (t.id === "pet-tts-reftext") {
        void commitPetVoiceText((t as HTMLInputElement).value).catch(e => report("保存参考文本失败", e));
        return;
      }
      // 调试数值框：失焦提交（`change` 对文本框就是「值变了且失去焦点」）。
      if (t.matches("[data-param-val]")) {
        if (el("pet-debug-body")?.classList.contains("off")) return; // 图片模式：整块禁用，维持现状
        commitParamNum(t as HTMLInputElement);
      }
    };

    // 数值框按 Enter 也提交（`change` 只在失焦时来，Enter 不来）—— 两种时机都要接住。
    const onKeydown = (ev: Event) => {
      const t = ev.target as HTMLElement | null;
      if (!t || (ev as KeyboardEvent).key !== "Enter") return;
      if (t.id === "pet-proactive-cap") {
        ev.preventDefault();
        commitProactiveCap(t as HTMLInputElement);
        return;
      }
      if (t.id === "pet-proactive-interval") {
        ev.preventDefault();
        commitProactiveInterval(t as HTMLInputElement);
        return;
      }
      if (t.id === "pet-autoshot-min") {
        ev.preventDefault();
        commitProactiveNum(t as HTMLInputElement, pr => pr.autoShotMin, AUTOSHOT_MIN_MIN, AUTOSHOT_MIN_MAX, (pr, v) => ({ ...pr, autoShotMin: v }));
        return;
      }
      if (t.id === "pet-watch-sec") {
        ev.preventDefault();
        commitProactiveNum(t as HTMLInputElement, pr => pr.screenWatchSec, WATCH_SEC_MIN, WATCH_SEC_MAX, (pr, v) => ({ ...pr, screenWatchSec: v }));
        return;
      }
      if (t.id === "pet-watch-diff") {
        ev.preventDefault();
        commitProactiveNum(t as HTMLInputElement, pr => pr.screenDiff, WATCH_DIFF_MIN, WATCH_DIFF_MAX, (pr, v) => ({ ...pr, screenDiff: v }));
        return;
      }
      // 参考音色文本按 Enter 也提交（`change` 只在失焦时来）。
      if (t.id === "pet-tts-reftext") {
        ev.preventDefault();
        void commitPetVoiceText((t as HTMLInputElement).value).catch(e => report("保存参考文本失败", e));
        return;
      }
      if (!t.matches("[data-param-val]")) return;
      ev.preventDefault();
      if (el("pet-debug-body")?.classList.contains("off")) return;
      commitParamNum(t as HTMLInputElement);
    };

    const onInput = (ev: Event) => {
      const t = ev.target as HTMLElement | null;
      if (!t) return;
      if (t.id === "pet-scale") {
        const v = Math.round(Number((t as HTMLInputElement).value)) / 100;
        const out = el("pet-scale-val");
        if (out) out.textContent = `${Math.round(v * 100)}%`;
        saveConfig({ ...loadConfig(), scale: v });
        void emit(CTL_EVENT, { type: "set-scale", value: v });
        return;
      }
      if (t.id === "pet-rot") {
        const v = Math.round(Number((t as HTMLInputElement).value));
        const out = el("pet-rot-val");
        if (out) out.textContent = `${v}°`;
        saveConfig({ ...loadConfig(), rotation: v });
        void emit(CTL_EVENT, { type: "set-rotation", value: v });
        return;
      }
      if (t.id === "pet-tts-rate") {
        // **打字速度**：只写本插件配置、驱动逐字动画（`speechRate` 是唯一真相源）。
        // 语音语速 / 音量已独立成条（见 #pet-tts-speed / #pet-tts-volume），不再由此折算。
        const v = clampRate(Number((t as HTMLInputElement).value));
        const out = el("pet-tts-rate-val");
        if (out) out.textContent = String(v);
        saveConfig({ ...loadConfig(), speechRate: v });
        return;
      }
      if (t.id === "pet-tts-speed") {
        // **语音语速**：直接写宿主 `PetVoiceConfig.speed`（1:1 映射），走既有 `set_pet_voice`。
        const v = clampVoiceSpeed(Number((t as HTMLInputElement).value));
        const out = el("pet-tts-speed-val");
        if (out) out.textContent = v.toFixed(1) + "×";
        void applyVoiceSpeed(v).catch(e => report("保存语速失败", e));
        return;
      }
      if (t.id === "pet-tts-volume") {
        // **语音音量**：直接写宿主 `PetVoiceConfig.volume`（1:1 映射），走既有 `set_pet_voice`。
        const v = clampVoiceVolume(Number((t as HTMLInputElement).value));
        const out = el("pet-tts-volume-val");
        if (out) out.textContent = Math.round(v * 100) + "%";
        void applyVoiceVolume(v).catch(e => report("保存音量失败", e));
        return;
      }
      // 调试拉条：数值框即时跟手（emit 走节流）
      const id = t.dataset.param;
      if (!id) return;
      if (el("pet-debug-body")?.classList.contains("off")) return; // 图片模式：再挡一道（键盘也能触发 input）
      const value = Number((t as HTMLInputElement).value);
      const out = root.querySelector<HTMLInputElement>(`[data-param-val="${id}"]`);
      // 双向同步：拖动拉条 → 框里的数值实时跟着变；但**别覆盖用户正在编辑的那只框**
      // （它获焦时不回写，否则一拖就把人家输到一半的内容冲掉）。
      if (out && document.activeElement !== out) out.value = String(value);
      emitParamThrottled(id, value);
    };

    root.addEventListener("click", onClick);
    root.addEventListener("change", onChange);
    root.addEventListener("input", onInput);
    root.addEventListener("keydown", onKeydown);
    // 桌宠窗里滚轮按指针缩放后会广播 `scale-changed` —— 让本面板的拉条跟着走
    // （**只更新显示、不回转 emit**，否则两边来回改会打架）。与委托监听同块，一并只挂一次、一并清。
    const offScale = await listen<{ type?: string; value?: unknown }>(CTL_EVENT, ev => {
      const p = ev.payload;
      if (p?.type !== "scale-changed" || typeof p.value !== "number") return;
      const v = Math.round(p.value * 100);
      const slider = el<HTMLInputElement>("pet-scale");
      if (slider) slider.value = String(v);
      const out = el("pet-scale-val");
      if (out) out.textContent = `${v}%`;
    });
    // 桌宠窗右键「恢复默认大小 / 位置」会把旋转一并归零 —— 广播过来让本面板拉条跟着走。
    const offRot = await listen<{ type?: string; value?: unknown }>(CTL_EVENT, ev => {
      const p = ev.payload;
      if (p?.type !== "rotation-changed" || typeof p.value !== "number") return;
      const v = Math.round(p.value);
      const slider = el<HTMLInputElement>("pet-rot");
      if (slider) slider.value = String(v);
      const out = el("pet-rot-val");
      if (out) out.textContent = `${v}°`;
    });
    // 桌宠窗广播「正在播的注入动作」的进度 ⇒ 画进度条（动作页每行 / 动作链每步下方）。
    // `p` 为 `null` = 当前没在播 ⇒ 全部清零。
    const offProgress = await listen<{ type?: string; chainId?: unknown; p?: unknown }>(CTL_EVENT, ev => {
      const pay = ev.payload;
      if (pay?.type !== "motion-progress") return;
      const p = pay.p as { index?: unknown; elapsedMs?: unknown; totalMs?: unknown } | null | undefined;
      // 先全部清零（简单、稳妥：同时最多只有一条在播）。
      root.querySelectorAll<HTMLElement>(".pet-chain-bar i").forEach(i => (i.style.width = "0%"));
      if (!p || typeof p.index !== "number" || typeof p.elapsedMs !== "number" || typeof p.totalMs !== "number") return;
      if (!(p.totalMs > 0)) return;
      const pct = Math.max(0, Math.min(1, p.elapsedMs / p.totalMs));
      const chainId = typeof pay.chainId === "string" && pay.chainId ? pay.chainId : null;
      setChainBar(chainId, p.index, pct);
    });

    const offProactive = await listen<{ type?: string }>(CTL_EVENT, ev => {
      if (ev.payload?.type !== "proactive-used") return;
      renderProactiveUsage();
    });
    // 桌宠窗右键菜单改了穿透 ⇒ 广播过来让本按钮跟着对齐（用户口径：设置与模型两处状态必须一致）。
    const offThrough = await listen<{ type?: string; value?: unknown }>(CTL_EVENT, ev => {
      if (ev.payload?.type !== "click-through-changed" || typeof ev.payload.value !== "boolean") return;
      renderThrough(ev.payload.value);
    });
    // 桌宠窗广播「现在开着哪些热键」⇒ 把每行的开关按钮对齐（**只改按钮，不整表重画**，
    // 免得正在输入的名字框被冲掉焦点）。
    const applyHotkeyStates = () => {
      root.querySelectorAll<HTMLElement>(".pet-hk-row").forEach(row => {
        const id = row.dataset.hkId;
        const btn = row.querySelector<HTMLElement>("[data-hk-run]");
        if (!id || !btn) return;
        const on = hkActive.has(id);
        btn.classList.toggle("on", on);
        btn.setAttribute("aria-pressed", on ? "true" : "false");
        btn.textContent = on ? "开" : "关";
      });
    };
    const offHkStates = await listen<{ type?: string; ids?: unknown }>(CTL_EVENT, ev => {
      const p = ev.payload;
      if (p?.type !== "hotkey-states" || !Array.isArray(p.ids)) return;
      hkActive = new Set(p.ids.filter((x): x is string => typeof x === "string"));
      applyHotkeyStates();
    });
    // 主窗重启完 agent ⇒ 回一条结果（定向到本窗），在人格区显示。
    const offRestart = await listen<{ ok?: boolean }>(AGENT_RESTART_DONE_EVENT, ev => {
      setPersonaMsg(ev.payload?.ok === false ? t("pet.persona_restart_failed") : t("pet.persona_restarted"));
    });
    petCleanup = () => {
      root.removeEventListener("click", onClick);
      root.removeEventListener("change", onChange);
      root.removeEventListener("input", onInput);
      root.removeEventListener("keydown", onKeydown);
      offScale();
      offRot();
      offProgress();
      offProactive();
      offThrough();
      offHkStates();
      offRestart();
      if (togglePoll) clearInterval(togglePoll); // 清掉那条约 3s 的状态轮询
      if (paramTimer) clearTimeout(paramTimer);
      delete marked[DELEG];
      petCleanup = null;
    };
  }

  // ── 语音「参考音色」导入 / 状态回显（2026-10-12）──────────────────────────────
  // 状态来自宿主 `get_pet_voice`（配置 + 是否可用 + 不可用原因）；参考文本 / 音色路径
  // 都经 `set_pet_voice` 写 `config\pet-voice.json`（**共用同一份 PetVoiceConfig，不新增第二套**）。
  // 「不可用原因」是宿主给的**机器可读 key**，这里翻成文案（见 i18n 的 pet.tts_reason_*）。
  interface PetVoiceCfg {
    enabled: boolean;
    reference_audio: string;
    reference_text: string;
    num_steps: number;
    num_threads: number;
    speed: number;
    /** 语音音量（0 = 静音、1 = 原始）。宿主 `PetVoiceConfig.volume`。 */
    volume: number;
  }
  interface PetVoiceStatus {
    config: PetVoiceCfg;
    assets_ready: boolean;
    available: boolean;
    reason: string;
    dir: string;
  }

  /** 把状态画进 `#pet-tts-status`；并把参考文本 / 语音语速 / 音量回填到控件（**不覆盖用户正在编辑 / 拖动的那个**）。 */
  function paintPetVoiceStatus(): void {
    const ta = el<HTMLInputElement>("pet-tts-reftext");
    if (petVoice && ta && document.activeElement !== ta) ta.value = petVoice.config.reference_text;
    // 语音语速 / 音量拉条：把宿主配置回填（正在拖动的那条不覆盖）。
    if (petVoice) {
      const speedEl = el<HTMLInputElement>("pet-tts-speed");
      if (speedEl && document.activeElement !== speedEl) {
        const v = clampVoiceSpeed(petVoice.config.speed);
        speedEl.value = String(v);
        const out = el("pet-tts-speed-val");
        if (out) out.textContent = v.toFixed(1) + "×";
      }
      const volEl = el<HTMLInputElement>("pet-tts-volume");
      if (volEl && document.activeElement !== volEl) {
        const v = clampVoiceVolume(petVoice.config.volume);
        volEl.value = String(v);
        const out = el("pet-tts-volume-val");
        if (out) out.textContent = Math.round(v * 100) + "%";
      }
    }
    const statusEl = el("pet-tts-status");
    if (!statusEl) return;
    if (!petVoice) {
      statusEl.textContent = "";
      statusEl.className = "pet-note";
      return;
    }
    if (petVoice.available) {
      statusEl.textContent = t("pet.tts_status_ready");
      statusEl.className = "pet-note pet-tts-ok";
    } else {
      statusEl.textContent = t("pet.tts_status_not_ready") + " · " + t("pet.tts_reason_" + (petVoice.reason || "no_ref_audio"));
      statusEl.className = "pet-note";
    }
  }

  async function refreshPetVoiceStatus(): Promise<void> {
    try {
      petVoice = await invoke<PetVoiceStatus>("get_pet_voice");
    } catch (e) {
      report("读取语音状态失败", e);
    }
    // 读到状态后回填语音语速 / 音量拉条（覆盖「拉条初次渲染时宿主状态还没读回」的窗口期）。
    paintPetVoiceStatus();
  }

  /** 把「语音语速」写回宿主（`PetVoiceConfig.speed`，1:1 映射）。**共用同一份 `PetVoiceConfig`**，
   *  不新增第二套配置。语音状态还没读回（`petVoice` 为 null）时跳过 —— `refreshPetVoiceStatus` 稍后回填。 */
  async function applyVoiceSpeed(speed: number): Promise<void> {
    const cur = petVoice?.config;
    if (!cur) return;
    if (cur.speed === speed) return;
    await invoke("set_pet_voice", { config: { ...cur, speed } });
    petVoice = { ...petVoice!, config: { ...cur, speed } };
  }

  /** 把「语音音量」写回宿主（`PetVoiceConfig.volume`，1:1 映射）。同一份 `PetVoiceConfig`。 */
  async function applyVoiceVolume(volume: number): Promise<void> {
    const cur = petVoice?.config;
    if (!cur) return;
    if (cur.volume === volume) return;
    await invoke("set_pet_voice", { config: { ...cur, volume } });
    petVoice = { ...petVoice!, config: { ...cur, volume } };
  }

  /** 把参考文本写进语音配置（**保留既有 `reference_audio` 与其余字段**）。 */
  async function commitPetVoiceText(value: string): Promise<void> {
    const cur = petVoice?.config;
    if (!cur || cur.reference_text === value) return;
    await invoke("set_pet_voice", { config: { ...cur, reference_text: value } });
    petVoice = { ...petVoice!, config: { ...cur, reference_text: value } };
    paintPetVoiceStatus();
  }

  /** 「添加音频文件」：选文件 → 宿主导入（校验 + 规整成 PCM16 WAV 缓存）→ 落进语音配置。 */
  async function addPetVoiceAudio(): Promise<void> {
    const msg = el("pet-tts-msg");
    const picked = await openFileDialog({
      multiple: false,
      directory: false,
      title: t("pet.tts_import_title"),
      filters: [{ name: t("pet.tts_import_filter"), extensions: ["wav", "mp3", "flac", "ogg", "m4a", "aac", "opus"] }],
    });
    const src = typeof picked === "string" ? picked : "";
    if (!src) return;
    try {
      const cached = await invoke<string>("import_pet_voice_audio", { path: src });
      const cur = petVoice?.config;
      const text = el<HTMLInputElement>("pet-tts-reftext")?.value ?? cur?.reference_text ?? "";
      await invoke("set_pet_voice", {
        config: { ...(cur ?? { enabled: true, reference_text: "", num_steps: 8, num_threads: 4, speed: 1, volume: 1 }), reference_audio: cached, reference_text: text },
      });
      await refreshPetVoiceStatus();
      if (msg) {
        msg.textContent = t("pet.tts_import_ok");
        msg.className = "pet-note";
      }
    } catch (e) {
      if (msg) {
        msg.textContent = t("pet.tts_import_fail") + (e instanceof Error ? e.message : String(e));
        msg.className = "pet-note pet-bad";
      }
    }
  }

  /** 「试听」：用 `tts_speak` 读一句固定示例（复用同一条朗读链路）。
   *  ⚠️ **不可用时宿主现在会返回原因 key**（与 `get_pet_voice` 的 `reason` **同一套**，见
   *  `tts_zipvoice::unavailable_reason`）⇒ 这里翻成文案显示在按钮下方（**不再「点了没反应」**）；
   *  桌宠**自动朗读**那条路径仍 `.catch(()=>{})` 吞掉 ⇒ 静默不读（用户 2026-10-12 裁决不变）。 */
  function previewPetVoice(): void {
    const msg = el("pet-tts-msg");
    if (msg) msg.textContent = "";
    void invoke("tts_speak", { text: t("pet.tts_preview_text"), enabled: true }).catch(e => {
      const key = e instanceof Error ? e.message : String(e);
      // 已知原因 key 走 i18n（复用状态行那组 pet.tts_reason_*）；其余（不该出现）原样显示，绝不吞掉。
      const shown = PET_VOICE_REASONS.includes(key) ? t("pet.tts_reason_" + key) : key;
      if (msg) {
        msg.textContent = shown;
        msg.className = "pet-note";
      }
      report("试听失败", e);
    });
  }

  void refreshPetVoiceStatus();

  /** **统一「更换形象」**（2026-10-09）：一个对话框里既能选 Live2D 模型（`.json`）
   *  也能选图片 —— `openFileDialog` 的过滤器把两类扩展名一起列上，**按扩展名分流**。
   *  控制台与桌宠窗右键菜单共用这一条路径，不必再维护两份。
   *  选模型时**当场验一遍**：桌宠窗小、用户可能刚把它收起来，报错必须落在看得见的地方。 */
  async function pickShape(): Promise<string> {
    // **选模型文件夹**（用户 2026-10-10 口径：不再选单个 json / 图片）—— VTS 的模型本来就是
    // 「一个文件夹」，且作者配好的东西（热键 / 参数绑定 / 可读名字）都在文件夹里的旁边文件里。
    // 所以这里列不了目录，交给宿主 `scan_model_folder` 扫；随后**一并读** vtube / cdi3 / 表情。
    const box = el("pet-file");
    const picked = await openFileDialog({
      multiple: false,
      directory: true,
      title: "选择 Live2D 模型文件夹（里面要有 *.model3.json）",
    });
    const dir = typeof picked === "string" ? picked : "";
    if (!dir) return "";
    if (box) box.innerHTML = "正在读取模型文件夹…";

    let model = "";
    let vtube = "";
    let cdi3 = "";
    try {
      const s = await invoke<{ model3?: string[]; vtube?: string[]; cdi3?: string[] }>("scan_model_folder", { dir });
      model = (s.model3 ?? [])[0] ?? "";
      vtube = (s.vtube ?? [])[0] ?? "";
      cdi3 = (s.cdi3 ?? [])[0] ?? "";
    } catch (e) {
      if (box) box.innerHTML = `<span class="pet-bad">读文件夹失败：${esc(e instanceof Error ? e.message : String(e))}</span>`;
      return "";
    }
    if (!model) {
      if (box) box.innerHTML = `<span class="pet-bad">这个文件夹里没有 *.model3.json。</span>`;
      return "";
    }
    const v = await validateModelFile(model);
    if (!v.ok) {
      if (box) box.innerHTML = `<span class="pet-bad">这个模型用不了：${esc(v.reason)}</span>`;
      return "";
    }
    saveConfig({ ...loadConfig(), model, image: "" });
    // 「一并读取」：作者热键（vtube.json，没有就回落扫 exp3）+ cdi3（参数可读名字）。
    const extra: string[] = [];
    try {
      const n = await importHotkeysFromFolder(dir, model, vtube);
      if (n) extra.push(`热键 ${n} 条`);
    } catch (e) {
      report("读取模型热键失败", e);
    }
    if (cdi3) extra.push("含 cdi3");
    renderFile([v.info, extra.join(" · ")].filter(Boolean).join("　"));
    void emit(CTL_EVENT, { type: "reload" });
    void refreshHotkeys(); // 换了模型 ⇒ 热键页的目标清单必须跟着换
    void refreshDebug(); // 调试区参数也要按新模型重建
    void refreshMotions(); // 预设动作页的动作组同理
    return model;
  }

  // ── 热键事件委托（2026-10-10）──────────────────────────────────
  // 与上面同一个纪律：委托挂在**稳定的 `root`** 上、只注册一次；状态在**模块级**
  //（见 renderHotkeys 那段注释）—— 两件事缺一，重渲染后就会「点了没反应」。
  const HK_DELEG = "__lunacPetHotkeyDelegate";
  const hkRoot = root as unknown as Record<string, unknown>;
  if (!hkRoot[HK_DELEG]) {
    hkRoot[HK_DELEG] = true;

    const rowOf = (t: HTMLElement) => t.closest<HTMLElement>(".pet-hk-row");
    const itemOf = (row: HTMLElement | null) =>
      row ? hkList.find(h => h.id === row.dataset.hkId) : undefined;

    root.addEventListener("click", (ev: Event) => {
      const t = ev.target as HTMLElement | null;
      if (!t || !t.closest("#pet-hotkeys")) return;
      if (t.closest("#pet-hk-import")) {
        // 「重新导入」：以**模型文件夹**为准重建热键表 —— 优先 `vtube.json` 里的作者热键，
        // 没有就递归扫文件夹里的「中文名.exp3.json」（导入即落盘覆盖，见 importHotkeysFromFolder）。
        void (async () => {
          const model = loadConfig().model.trim();
          if (!model) {
            hkMsg("先选一个 Live2D 模型 —— 「更换形象」里选模型文件夹。");
            return;
          }
          const side = await findModelSideFiles(model);
          try {
            const n = await importHotkeysFromFolder(side.dir, model, side.vtube);
            if (!n) {
              hkMsg("这个模型文件夹里没找到可导入的热键 / 表情。");
              return;
            }
            hkMsg(`已导入 ${n} 条（${side.vtube ? "取自 vtube.json" : "扫自文件夹"}），已落盘。`);
            // 让桌宠窗重挂一次：新导入的表情要进引擎的 `FileReferences.Expressions` 才会生效。
            void emit(CTL_EVENT, { type: "reload" });
          } catch (e) {
            hkMsg("导入失败：" + (e instanceof Error ? e.message : String(e)));
          }
        })();
        return;
      }
      const row = rowOf(t);
      const h = itemOf(row);
      if (!row || !h) return;
      if (t.closest("[data-hk-run]")) {
        // **状态按钮**（2026-10-10 用户口径）：点一下开、再点一下关。真正的施加 / 撤销在
        // 桌宠窗（那里才有实例），本窗只广播 + 依据回播的 `hotkey-states` 对齐按钮。
        void pingPet().then(alive => {
          if (!alive) {
            hkMsg("桌宠没显示 —— 先点上面「显示桌宠」，模型加载好后再点。");
            return;
          }
          void emit(CTL_EVENT, { type: "hotkey-toggle", id: h.id, action: h.action });
          hkMsg("");
        });
        return;
      }
      if (t.closest("[data-hk-del]")) {
        hkList = hkList.filter(x => x.id !== h.id);
        renderHotkeys();
        scheduleSaveHotkeys(); // 删除也要落盘（否则重开设置窗又冒出来）
        return;
      }
    });

    const commit = (ev: Event, live: boolean) => {
      const t = ev.target as HTMLElement | null;
      if (!t || !t.closest("#pet-hotkeys")) return;
      const row = rowOf(t);
      const h = itemOf(row);
      if (!row || !h) return;
      const field = t.dataset.hkField;
      if (!field) return;
      const val = (t as HTMLInputElement).value;
      // 注：`type` / `target` 两个下拉已改**自绘** .custom-select，选择走 wirePetCustomSelects
      // 的 onPick，不再经过这里的 change / input 委托 —— 这里只剩原生 input / select（name / group 等）。
      if (field === "name") {
        h.name = val;
        scheduleSaveHotkeys();
        return;
      }
      applyHotkeyField(h.action, field, val);
      // 换了目标（id / group）后当前值可能已不适用，重画一次让 UI 与数据一致；
      // 数值类字段**不重画**，否则每敲一个字符都会丢焦点。
      if (!live && (field === "id" || field === "group")) renderHotkeys();
      scheduleSaveHotkeys();
    };
    root.addEventListener("change", ev => commit(ev, false));
    root.addEventListener("input", ev => commit(ev, true));
  }

  // ── 动作页的事件委托（2026-10-10）────────────────────────────────
  // 与热键页同一套纪律（状态模块级 + 委托挂 root 只注册一次）。**行 = 文件**：
  // 播放走 `CTL_EVENT` 的 `motion-play`（真正的实例在桌宠窗）；改名 / 删除直接改**文件**，
  // 改完重扫 + 让桌宠窗重挂（注入组的序号会变，见 refreshMotions 的注释）。
  const MO_DELEG = "__lunacPetMotionDelegate";
  if (!hkRoot[MO_DELEG]) {
    hkRoot[MO_DELEG] = true;
    /** 行的下标（属性值是 i）。 */
    const idxOf = (t: HTMLElement, sel: string): number => {
      const el = t.closest<HTMLElement>(sel);
      return el ? Number.parseInt(el.getAttribute(sel.slice(1, -1)) ?? "", 10) : NaN;
    };
    /** 文件名的最后一段（host 命令只收纯文件名，不收路径）。 */
    const baseNameOf = (p: string) => p.split(/[\\/]/).pop() || "";

    /** 改完文件后：重扫列表 + 摘掉失效的待机循环项 + 让桌宠窗重挂（合成组序号跟着文件列表走）。 */
    const afterFileOp = async () => {
      await refreshMotions();
      pruneIdleMotions();
      void emit(CTL_EVENT, { type: "reload" });
    };

    root.addEventListener("click", (ev: Event) => {
      const t = ev.target as HTMLElement | null;
      if (!t || !t.closest("#pet-motions")) return;

      // ── 动作链（2026-10-10）───────────────────────────────────────
      const pair = (attr: string): [number, number] => {
        const el = t.closest<HTMLElement>(`[${attr}]`);
        const v = el?.getAttribute(attr) ?? "";
        const [a, b] = v.split(":").map(x => Number.parseInt(x, 10));
        return [a, b];
      };
      if (t.closest("#pet-chain-add")) {
        chains = [...chains, makeChain(`动作链 ${chains.length + 1}`)];
        renderChains();
        void persistChains();
        return;
      }
      if (t.closest("[data-chain-toggle]")) {
        // 展开 / 收起（只改内存态，不落盘、不打扰播放）。
        const [ci] = pair("data-chain-toggle");
        const c = chains[ci];
        if (!c) return;
        if (chainCollapsed.has(c.id)) chainCollapsed.delete(c.id);
        else chainCollapsed.add(c.id);
        renderChains();
        return;
      }
      if (t.closest("[data-chain-loop]")) {
        // 整链循环开关（落盘：`loop` 进 lunac.chains.json）。
        const [ci] = pair("data-chain-loop");
        const c = chains[ci];
        if (!c) return;
        chains = chains.map((x, i) => (i === ci ? { ...x, loop: !x.loop } : x));
        renderChains();
        void persistChains();
        return;
      }
      if (t.closest("[data-chain-del]")) {
        const [ci] = pair("data-chain-del");
        if (!Number.isInteger(ci)) return;
        const removed = chains[ci];
        if (removed) chainCollapsed.delete(removed.id);
        chains = chains.filter((_, i) => i !== ci);
        renderChains();
        void persistChains();
        return;
      }
      if (t.closest("[data-chain-step-add]")) {
        const [ci] = pair("data-chain-step-add");
        const c = chains[ci];
        if (!c) return;
        const file = moFiles[0]?.path ?? "";
        // 新增步骤也按该动作的完整时长填「循环秒数」（与「换动作」同一口径，见 commitChainField）。
        void (async () => {
          const dur = await readMotionDurationSec(file);
          const cc = chains[ci];
          if (!cc) return;
          const step = dur !== null ? { ...makeStep(file), loopSec: dur } : makeStep(file);
          const next = { ...cc, steps: [...cc.steps, step] };
          chains = chains.map((x, i) => (i === ci ? next : x));
          renderChains();
          void persistChains();
        })();
        return;
      }
      if (t.closest("[data-chain-step-del]")) {
        const [ci, si] = pair("data-chain-step-del");
        const c = chains[ci];
        if (!c || !Number.isInteger(si)) return;
        const next = { ...c, steps: c.steps.filter((_, i) => i !== si) };
        chains = chains.map((x, i) => (i === ci ? next : x));
        renderChains();
        void persistChains();
        return;
      }
      if (t.closest("[data-chain-run]")) {
        // 播放交给**桌宠窗**（实例在那边）：它按链 id 重新读盘解析文件 → 下标再喂引擎。
        const id = t.closest<HTMLElement>("[data-chain-run]")?.getAttribute("data-chain-run") ?? "";
        if (id) void emit(CTL_EVENT, { type: "chain-play", id });
        return;
      }
      if (t.closest("#pet-mo-add")) {
        // 「新建动作」= 在模型文件夹里**生成一个覆盖全参数的 motion3 模板**（用户口径），
        // 交给 AI 去编曲线。拿不到运行时参数表就提示先显示一次桌宠。
        void (async () => {
          const model = loadConfig().model.trim();
          if (!model) {
            moMsg("先选一个 Live2D 模型。");
            return;
          }
          const json = await buildMotionTemplate(model);
          if (!json) {
            moMsg("拿不到参数清单 —— 先「显示桌宠」一次，再来新建动作。");
            return;
          }
          const n = JSON.parse(json) as { Curves?: unknown[] };
          const name = `${MO_GEN_PREFIX}${Date.now().toString(36)}.motion3.json`;
          try {
            await invoke<string>("write_motion_file", { modelPath: model, name, json });
            moMsg(`已生成 ${name}（${n.Curves?.length ?? 0} 条参数曲线），可交给 AI 编。`);
            await afterFileOp();
          } catch (e) {
            moMsg("生成失败：" + (e instanceof Error ? e.message : String(e)));
          }
        })();
        return;
      }
      const idleI = idxOf(t, "[data-mo-idle]");
      if (Number.isInteger(idleI)) {
        // 「待机循环」开关（2026-10-10 用户口径）：写进配置 → 重画本行 + 让桌宠窗**重挂**
        // （待机列表是挂载时交给引擎的，改了必须重挂才生效）。
        const f = moFiles[idleI];
        const model = loadConfig().model.trim();
        if (!f || !model) return;
        const c = loadConfig();
        const cur = new Set(c.idleMotions[model] ?? []);
        const hit = [...cur].find(s => s.toLowerCase() === f.name.toLowerCase());
        if (hit !== undefined) cur.delete(hit);
        else cur.add(f.name);
        const next = { ...c.idleMotions };
        if (cur.size) next[model] = [...cur];
        else delete next[model];
        saveConfig({ ...c, idleMotions: next });
        renderMotions();
        void emit(CTL_EVENT, { type: "reload" });
        return;
      }
      const runI = idxOf(t, "[data-mo-run]");
      if (Number.isInteger(runI)) {
        void emit(CTL_EVENT, { type: "motion-play", group: MOTION_EXTRA_GROUP, index: runI });
        return;
      }
      const delI = idxOf(t, "[data-mo-del]");
      if (Number.isInteger(delI)) {
        void (async () => {
          const f = moFiles[delI];
          const model = loadConfig().model.trim();
          if (!f || !model) return;
          try {
            await invoke("delete_motion_file", { modelPath: model, name: baseNameOf(f.path) });
            await afterFileOp();
          } catch (e) {
            moMsg("删除失败：" + (e instanceof Error ? e.message : String(e)));
          }
        })();
      }
    });

    // 改名 = 改**文件名**（失焦 / Enter 触发）。输入框里是文件名词干，后缀恒为 `.motion3.json`。
    const commitName = (ev: Event, blurAfter: boolean) => {
      const t = ev.target as HTMLInputElement | null;
      const raw = t?.dataset.moName;
      if (!t || raw === undefined) return;
      const f = moFiles[Number.parseInt(raw, 10)];
      const model = loadConfig().model.trim();
      if (!f || !model) return;
      const want = t.value.trim().replace(/\.motion3\.json$/i, "");
      if (blurAfter) t.blur();
      if (!want || want === f.name) return;
      void (async () => {
        try {
          await invoke<string>("rename_motion_file", {
            modelPath: model,
            from: baseNameOf(f.path),
            to: `${want}.motion3.json`,
          });
          await afterFileOp();
        } catch (e) {
          moMsg("改名失败：" + (e instanceof Error ? e.message : String(e)));
        }
      })();
    };
    // ── 动作链字段（动作链名 / 步骤动作 / 循环时长 / 硬停秒）──────────────
    const chainPair = (el: HTMLElement | null, attr: string): [number, number] => {
      const v = el?.getAttribute(attr) ?? "";
      const [a, b] = v.split(":").map(x => Number.parseInt(x, 10));
      return [a, b];
    };
    const commitChainField = (ev: Event) => {
      const t = ev.target as HTMLElement | null;
      if (!t) return;
      if (t.dataset.chainName !== undefined) {
        const ci = Number.parseInt(t.dataset.chainName, 10);
        const c = chains[ci];
        if (!c) return;
        const name = (t as HTMLInputElement).value.trim() || c.name;
        chains = chains.map((x, i) => (i === ci ? { ...x, name } : x));
        void persistChains();
        return;
      }
      if (t.dataset.chainStepFile !== undefined) {
        // 兼容旧的**原生** select 路径（现已改成自绘 .custom-select，走 commitChainFile）；
        // 留一句兜底，避免万一还有原生 select 残留时静默失效。
        const [ci, si] = chainPair(t, "data-chain-step-file");
        commitChainFile(ci, si, (t as HTMLSelectElement).value);
        return;
      }
      const isLoop = t.dataset.chainStepLoop !== undefined;
      const isStop = t.dataset.chainStepStop !== undefined;
      if (isLoop || isStop) {
        const [ci, si] = chainPair(t, isLoop ? "data-chain-step-loop" : "data-chain-step-stop");
        const c = chains[ci];
        if (!c || !Number.isInteger(si)) return;
        const raw = Number.parseFloat((t as HTMLInputElement).value);
        const val = Number.isFinite(raw) ? raw : 0;
        const steps = c.steps.map((s, i) => {
          if (i !== si) return s;
          if (isLoop) return { ...s, loopSec: val > 0 ? Math.min(3600, val) : 3 };
          return { ...s, stopSec: val > 0 ? Math.min(3600, val) : 0 };
        });
        chains = chains.map((x, i) => (i === ci ? { ...x, steps } : x));
        renderChains();
        void persistChains();
      }
    };
    root.addEventListener("change", commitChainField);

    root.addEventListener("change", ev => commitName(ev, false));
    root.addEventListener("keydown", (ev: Event) => {
      if ((ev as KeyboardEvent).key !== "Enter") return;
      const t = ev.target as HTMLInputElement | null;
      if (t?.dataset.moName === undefined) return;
      ev.preventDefault();
      commitName(ev, true);
    });
  }

  injectPetStyle();
  renderFile();
  renderThrough(loadConfig().clickThrough);
  void loadPetPersona();
  refreshToggle();
  void refreshHotkeys();
  void refreshMotions();
  // 人格页三张列表（事实记忆 / 成长笔记 / 人格建议）：面板打开时各拉一次。
  void refreshPetList("memory");
  void refreshPetList("growth");
  void refreshPetList("suggest");
  // **每次打开插件自动拉起模型**（用户 2026-10-10 口径）：配了 Live2D 模型而桌宠窗没开时，
  // 直接把它开起来 —— 桌宠窗装载时会自行应用**持久化**的参数，于是「上次的参数」自动维持。
  // 放最后：先让本窗渲染完再拉形象，且失败也不影响设置窗可用。
  void (async () => {
    if (!loadConfig().model.trim()) return;
    if (await pingPet()) return; // 已经开着：不重复开
    await showPet();
    setToggle(true);
  })().catch(() => {});
}

// ══════════════════════════════════════════════════════════════════
//  桌宠窗
// ══════════════════════════════════════════════════════════════════

function petHtml(): string {
  return `
<div class="pet-root" id="pet-root">
  <!-- 对话改渲染（2026-10-10 用户口径；2026-10-13 第二轮补画布跟随窗口）：
       输出贴窗口上沿、输入贴下沿，两条**紧贴模型区**（无空隙）—— 缺省 上 75 / 模型区（参考）300×400 / 下 35。
       模型区高度**不再定尺**：用 flex 跟随窗口剩余空间（画布变高模型不变大、贴底、上方多出空间）。
       两条**始终占位**（对话收起时只隐藏内容），所以模型不会因开合对话而跳动。整块的显隐由 .pet-root
       上的 chat-on / chat-input-on 两个类控制（见 setChatOpen）。 -->
  <div class="pet-top">
    <div class="pet-chat-output" id="pet-chat-bubble">
      <div class="pet-chat-text" id="pet-chat-text"></div>
    </div>
  </div>
  <div class="pet-stage" id="pet-stage">
    <div class="pet-l2d" id="pet-l2d"></div>
    <img class="pet-img" id="pet-img" alt="" draggable="false">
    <div class="pet-empty" id="pet-empty">还没有形象<br><span>在控制台里选一张图片或一个 Live2D 模型</span></div>
  </div>
  <div class="pet-bottom">
    <div class="pet-chat-inputrow">
      <textarea class="pet-chat-input" id="pet-chat-input" rows="1" spellcheck="false" autocomplete="off" placeholder="${esc(t("pet.ai_chat_placeholder"))}" aria-label="${esc(t("pet.ai_chat"))}"></textarea>
      <button type="button" class="pet-chat-stop" id="pet-chat-stop" title="${esc(t("pet.chat_stop"))}" aria-label="${esc(t("pet.chat_stop"))}">${STOP_SVG}</button>
      <button type="button" class="pet-chat-close" id="pet-chat-close" title="${esc(t("pet.ai_chat_close"))}" aria-label="${esc(t("pet.ai_chat_close"))}">×</button>
    </div>
  </div>
  <div class="pet-err" id="pet-err"></div>
  <div class="pet-toast" id="pet-toast"></div>
  <div class="pet-menu" id="pet-menu">
    <button data-act="ai-chat">${t("pet.ai_chat")}</button>
    <button data-act="settings">桌宠设置…</button>
    <button data-act="look-screen">${t("pet.look_screen")}</button>
    <button data-act="change">更换模型…</button>
    <button data-act="reset">恢复</button>
    <button data-act="win-reset">${t("pet.win_reset")}</button>
    <button data-act="through" id="pet-menu-through">${esc(throughLabel(loadConfig().clickThrough))}</button>
    <button data-act="close" class="danger">关闭桌宠</button>
  </div>
  <!-- 窗上缩放手柄（2026-10-13）：无边框窗没有系统边框可抓，靠它给一个**看得见**的缩放入口
       （拖边缘 / 角的能力由宿主 resizable 提供，见 plugin_window.rs 的 .resizable(...)）。
       只在**输入区未展开**时显示 —— 输入区右下角是「关闭 ×」按钮，两者会重叠。
       图标是严格线性 SVG（无 emoji），颜色走 --text-dim → hover --text。 -->
  <div class="pet-resize-handle" id="pet-resize-handle" title="${esc(t("pet.resize_handle"))}" aria-label="${esc(t("pet.resize_handle"))}">${RESIZE_SVG}</div>
</div>`;
}

/** 桌宠窗的样式。**全部带 `pet-` 前缀**，宿主那套主题变量（`--text` / `--border-glass`
 *  …）照用，这样换主题时两个窗口观感一致（规范见 Modules 的插件开发规范 §4.3）。 */
const PET_CSS = `
.pet-root{position:fixed;inset:0;display:flex;flex-direction:column;align-items:center;justify-content:flex-start;overflow:hidden;}
/* 上：输出带（**预留下限 75**，可随内容变大 —— 由 JS 改写 --pet-top-h，见 applyOutHeight）；
   下：输入带 固定 35 —— **始终占位**（对话收起只隐藏内容、模型不跳动）；
   两条都紧贴模型（无空隙）。数值与 pet-window-size 的 IN_H / OUT_MIN_H 一致。 */
.pet-top{flex:0 0 var(--pet-top-h,75px);width:100%;display:flex;align-items:flex-start;justify-content:center;overflow:hidden;}
.pet-bottom{flex:0 0 35px;width:100%;display:flex;align-items:flex-end;justify-content:center;overflow:hidden;}
/* 模型区（画布）**跟随窗口剩余空间**（2026-10-13 第二轮）：
   flex:1 1 auto ⇒ 吃掉「输出带 + 输入带」之外的全部高度（= 窗高 − 输出带 − 输入带）；
   宽度 = 窗宽 − 左右留白（--pet-stage-gap，缺省 20 = 左右各 10，与 STAGE_W 一致）。
   画布变大时模型**不放大**（只是周围多出空间）、且**贴底**（模型逻辑见 live2d-engine 的 fit 与
   pet-window-size 的 modelBaseScale「只缩不放」）。min-height:0 让它在窗口很矮时能收到底。
   ⚠️ 2026-10-13 第三轮：模型区**不会低于 STAGE_MIN_H（140）** —— 这个下限由 JS 保证（输出带上限
   动态取 outCap、窗矮则上限降；最小窗高抬到 WIN_H_MIN = 75 + 35 + 140 = 250），CSS 的 min-height:0
   只负责不被内容顶开、不再承担下限。见 pet-window-size 的 STAGE_MIN_H / WIN_H_MIN / outCap。 */
.pet-stage{position:relative;width:calc(100% - var(--pet-stage-gap,20px));flex:1 1 auto;min-height:0;display:flex;align-items:flex-end;justify-content:center;cursor:grab;touch-action:none;}
.pet-stage:active{cursor:grabbing;}
/* 图片分支与模型分支**行为一致**（2026-10-13）：盒子定尺 = **参考模型区**（--pet-stage-ref-w/h，
   缺省 300×400），不随画布放大 ⇒ 画布变大时图片同样不放大、只多空间；画布小于参考时由
   max-width/max-height 收到画布内（object-fit:contain 不裁切）。底对齐 ⇒ object-position:bottom
   center 让它贴底、上方留白（与 Live2D 的 anchor=底边中心对齐）。 */
.pet-img{flex:0 0 auto;width:var(--pet-stage-ref-w,300px);height:var(--pet-stage-ref-h,400px);max-width:100%;max-height:100%;object-fit:contain;object-position:bottom center;transform-origin:50% 100%;pointer-events:none;-webkit-user-drag:none;filter:drop-shadow(0 6px 14px rgba(var(--shade-rgb),.35));}
.pet-l2d{position:absolute;inset:0;display:none;}
.pet-l2d.on{display:block;}
.pet-l2d canvas{display:block;}
.pet-err{position:absolute;left:8px;right:8px;bottom:8px;display:none;font-size:.68rem;line-height:1.6;color:var(--red);background:var(--surface-glass);border:1px solid var(--border-glass);border-radius:var(--radius);padding:8px 10px;word-break:break-all;backdrop-filter:blur(14px);-webkit-backdrop-filter:blur(14px);}
.pet-err.on{display:block;}
.pet-toast{position:absolute;left:50%;top:84px;transform:translateX(-50%);display:none;max-width:92%;font-size:.72rem;line-height:1.6;color:var(--text);background:var(--surface-glass);border:1px solid var(--border-glass);border-radius:var(--radius);padding:8px 12px;text-align:center;word-break:break-all;backdrop-filter:blur(14px);-webkit-backdrop-filter:blur(14px);box-shadow:0 6px 18px rgba(var(--shade-rgb),.28);}
.pet-toast.on{display:block;}
/* 输出框：**随内容封顶**（用户 2026-10-13 口径）—— 宽度**跟随窗口宽**（width:100% = 整窗宽，
   缺省即 320；比参考窄时靠 box-sizing:border-box + min-width:0 + max-width:100% + word-break
   收住，不产生横向溢出）；高度**跟随输出带** --pet-top-h（下限 75 = 预留带，上限 OUT_CAP_H）。
   做法：align-self:stretch 覆盖 .pet-top 的 flex-start（= 撑满输出带）；height / max-height 都取
   --pet-top-h ⇒ 盒高恒等于输出带高（下限 75 时即原「固定占满预留 75px」的行为）；文字靠顶
   （顶部对齐，**不做垂直居中**）。内容**超过上限**（= 输出带已被封在 OUT_CAP_H）时在盒内滚动
   （overflow-y:auto）。短 / 中回复盒高贴合内容 ⇒ **不再出现滚动条**。
   ⚠️ 盒子是 **flex 列**而不是 block（2026-10-13 修）：本模板里 .pet-chat-output 与 .pet-chat-text
   之间夹着换行 + 缩进（HTML 排版的空白文本节点），而本盒带 white-space:pre-wrap、**块级**容器会把
   这种纯空白文本节点当成真内容渲染成空行 —— 于是文字**上方 / 下方各多出约两行空白**（用户 2026-10-13
   报的「文字前面 / 上下都有空白」；无头 Chrome 实测：文字距盒顶 42.84px，而 padding-top 只有 6px，
   盒内容高 104 > 75 还平白多出一条滚动条）。flex 容器**不渲染**纯空白子文本节点（规范把整串空白
   子文本节点按 display:none 处理）⇒ 与模板缩进无关地保证文字贴顶。
   无正文时靠 :has(> .pet-chat-text:empty) 让它**不可见但仍占位** ⇒ 输出带回到下限 75px、
   开合对话时模型不位移。
   ⚠️ white-space:pre-wrap 会把回复里的换行 / 连续空行渲染成盒子内的**真实空白块**（用户报的段间空白）；
   故正文写进 DOM 前一律先过 JS 的 normalizeReplyText（换行归一 + 连续空白折叠 + 首尾空行丢弃，
   见 pet-typewriter.ts）—— 这里保留 pre-wrap 只为兜底单个空格。
   ⚠️ 滚动条只走全局那一条（ui-style.md 界面 6）：不得写 scrollbar-width / scrollbar-color、
   不得按容器另写、不得藏。 */
.pet-chat-output{display:none;width:100%;max-width:100%;min-width:0;align-self:stretch;height:var(--pet-top-h,75px);max-height:var(--pet-top-h,75px);min-height:0;box-sizing:border-box;overflow-y:auto;padding:6px 10px;font-size:.72rem;line-height:1.6;color:var(--text);text-align:left;background:var(--surface-glass);border:1px solid var(--border-glass);border-radius:var(--radius);backdrop-filter:blur(18px);-webkit-backdrop-filter:blur(18px);word-break:break-word;white-space:pre-wrap;}
.pet-root.chat-on .pet-chat-output{display:flex;flex-direction:column;}
/* 正文**不许被 flex 收缩**（flex-shrink:0）：否则盒子定尺时正文会被压到盒高、内容溢出到
   盒外 —— 那样既量不到正文的真实高度（本来要按它算盒高），滚动也会看不见。撑开 + 盒内滚。 */
.pet-chat-text{flex:0 0 auto;min-width:0;}
.pet-chat-output:has(> .pet-chat-text:empty){visibility:hidden;}
.pet-chat-output.thinking .pet-chat-text{opacity:.55;}
/* 输入行：高 35（贴窗口下沿）；只在 .pet-root.chat-input-on 时显示。
   「桌宠发起」的三种情况（语音输入 / 主动询问 / 抓屏）**不显示输入区** —— 见 setChatOpen 的 showInput。 */
.pet-chat-inputrow{display:none;width:100%;height:35px;box-sizing:border-box;align-items:center;gap:6px;padding:0 8px;}
.pet-root.chat-input-on .pet-chat-inputrow{display:flex;}
.pet-chat-input{box-sizing:border-box;flex:1 1 auto;height:35px;resize:none;padding:6px 10px;font:inherit;font-size:.72rem;line-height:1.5;color:var(--text);background:var(--surface-glass);border:1px solid var(--border-glass);border-radius:var(--radius);outline:none;caret-color:rgb(var(--other-overlay-rgb));backdrop-filter:blur(18px);-webkit-backdrop-filter:blur(18px);}
.pet-chat-input:focus{border-color:rgba(var(--btn-line-rgb), var(--btn-line-alpha));}
.pet-chat-close{appearance:none;flex:0 0 auto;height:35px;background:var(--surface-glass);border:1px solid var(--border-glass);border-radius:var(--radius);font:inherit;font-size:.9rem;line-height:1;color:var(--text-dim);cursor:pointer;padding:0 9px;}
.pet-chat-close:hover{color:var(--text);background:var(--border-glass);}
/* 「中止对话」图标按钮（形态照 §界面 14.1 的图标按钮：30×30 / 圆角 6px；颜色走按钮背景 + 按钮线条）。
   只在「正在生成」时出现 —— 由 on 类控制（见 setPetGenerating）。 */
.pet-chat-stop{display:none;flex:0 0 auto;align-items:center;justify-content:center;width:30px;height:30px;box-sizing:border-box;background:rgba(var(--btn-bg-rgb),var(--btn-bg-alpha));border:1px solid rgba(var(--btn-line-rgb),var(--btn-line-alpha));border-radius:6px;color:rgb(var(--btn-line-rgb));cursor:pointer;padding:0;}
.pet-chat-stop.on{display:flex;}
.pet-chat-stop:hover{background:rgba(var(--btn-bg-rgb),calc(var(--btn-bg-alpha) + 0.06));}
.pet-empty{position:absolute;left:50%;bottom:38%;transform:translateX(-50%);text-align:center;font-size:.72rem;line-height:1.7;color:var(--text-dim);background:var(--surface-glass);border:1px solid var(--border-glass);border-radius:var(--radius);padding:10px 14px;backdrop-filter:blur(14px);-webkit-backdrop-filter:blur(14px);}
.pet-empty span{font-size:.66rem;opacity:.75;}
.pet-menu{position:absolute;display:none;flex-direction:column;min-width:132px;padding:4px;background:var(--surface-glass);border:1px solid var(--border-glass);border-radius:var(--radius);backdrop-filter:blur(20px);-webkit-backdrop-filter:blur(20px);box-shadow:0 8px 24px rgba(var(--shade-rgb),.28);}
.pet-menu.open{display:flex;}
.pet-menu button{appearance:none;background:none;border:0;text-align:left;font:inherit;font-size:.72rem;color:var(--text);padding:6px 10px;border-radius:6px;cursor:pointer;white-space:nowrap;}
.pet-menu button:hover{background:var(--border-glass);}
.pet-menu button.danger:hover{color:var(--red);}
/* 窗上缩放手柄（2026-10-13）：右下角。**起点是斜向抓握图标**（严格线性 SVG + currentColor），
   色走 --text-dim → hover --text（§界面 2「文字 / 图标走四类派生」）。
   只在**输入区未展开**时显示（.pet-root 无 chat-input-on）—— 输入区右下角是「关闭 ×」按钮，会重叠。
   pointer-events 必须 auto（桌宠窗空白区是穿透 / 拖窗的，手柄要能点）。 */
.pet-resize-handle{position:absolute;right:2px;bottom:2px;width:16px;height:16px;display:none;align-items:center;justify-content:center;color:var(--text-dim);cursor:nwse-resize;z-index:6;pointer-events:auto;touch-action:none;-webkit-user-select:none;user-select:none;}
.pet-root:not(.chat-input-on) .pet-resize-handle{display:flex;}
.pet-resize-handle:hover{color:var(--text);}
.pet-console{padding:10px 12px 14px;font-size:.76rem;color:var(--text);}
.pet-row{display:flex;align-items:center;gap:8px;margin-bottom:10px;flex-wrap:wrap;}
/* 开头那排按钮（显示桌宠 / 更换模型 / 取消配置 / 穿透）**必须同一行**（用户 2026-10-10 口径）：
   不换行 + 收窄内边距，四个刚好排得下。 */
.pet-row-main{gap:6px;flex-wrap:nowrap;}
.pet-row-main .pet-btn{padding:6px 8px;font-size:.7rem;flex:0 0 auto;white-space:nowrap;}
.pet-btn{appearance:none;font:inherit;font-size:.74rem;color:var(--text);background:var(--surface-glass);border:1px solid var(--border-glass);border-radius:8px;padding:6px 12px;cursor:pointer;}
.pet-btn:hover{border-color:rgb(var(--other-overlay-rgb));color:rgb(var(--other-overlay-rgb));}
.pet-btn-primary{background:rgb(var(--other-overlay-rgb));border-color:rgb(var(--other-overlay-rgb));color:var(--text);font-weight:600;}
.pet-btn-primary:hover{color:var(--text);opacity:.92;}
.pet-btn-toggle.on{background:rgb(var(--other-overlay-rgb));border-color:rgb(var(--other-overlay-rgb));color:var(--text);font-weight:600;}
.pet-btn-toggle.on:hover{color:var(--text);opacity:.92;}
/* 分页（模型调试 / 热键，2026-10-10）—— 照 .music-tab / .chat-hist-tab 的胶囊芯片语言。 */
.pet-tabs{display:flex;flex-wrap:wrap;gap:6px;margin-top:12px;border-top:1px solid var(--border-glass);padding-top:11px;margin-bottom:10px;}
.pet-tab{appearance:none;font:inherit;font-size:.72rem;color:var(--text-dim);background:transparent;border:1px solid var(--border-glass);border-radius:999px;padding:4px 14px;cursor:pointer;}
.pet-tab:hover{color:var(--text);border-color:rgba(var(--btn-line-rgb), var(--btn-line-alpha));}
.pet-tab.on{color:var(--text);background:rgba(var(--btn-bg-rgb), var(--btn-bg-alpha));border-color:rgba(var(--btn-line-rgb), var(--btn-line-alpha));}
/* 页内的块不再各画一条上分隔线（那一排页签已经承担了分隔）。
   2026-10-13：主动反应 / 语音朗读 / 人格三页把原先在页签之外的三块搬了进来，同样去掉上分隔。 */
.pet-tabpage>.pet-hotkeys,.pet-tabpage>.pet-debug,.pet-tabpage>.pet-proactive,.pet-tabpage>.pet-tts,.pet-tabpage>.pet-persona{margin-top:0;border-top:0;padding-top:0;}
/* 「恢复默认」按钮行：三个分页各一颗，**贴在页顶**（2026-10-10 用户口径：从页底移上来）。 */
.pet-reset-row{margin-bottom:10px;}
.pet-phys-tag{margin-left:6px;font-size:.62rem;color:var(--text-dim);border:1px solid var(--border-glass);border-radius:999px;padding:0 6px;opacity:.85;}
.pet-check{display:flex;align-items:center;gap:6px;color:var(--text-dim);cursor:pointer;}
.pet-label{color:var(--text-dim);}
.pet-val{color:var(--text-dim);min-width:3.2em;}
.pet-file{margin:2px 0 10px;color:var(--text-dim);word-break:break-all;line-height:1.6;}
.pet-file code{font-size:.7rem;color:var(--text);}
.pet-dim{opacity:.72;}
.pet-bad{color:var(--red);}
.pet-note{color:var(--text-dim);opacity:.72;font-size:.68rem;line-height:1.7;border-top:1px solid var(--border-glass);padding-top:9px;}
.pet-note b{color:var(--text);opacity:.9;}
.pet-persona{margin-top:14px;border-top:1px solid var(--border-glass);padding-top:12px;display:flex;flex-direction:column;gap:8px;}
.pet-persona .pet-note{border-top:0;padding-top:0;}
.pet-persona-text{width:100%;box-sizing:border-box;resize:vertical;min-height:96px;font:inherit;font-size:.72rem;line-height:1.7;color:var(--text);background:rgba(var(--shade-rgb),calc(0.3 * var(--shade-scale)));border:1px solid var(--border-glass);border-radius:8px;padding:8px 10px;outline:none;caret-color:rgb(var(--other-overlay-rgb));}
.pet-persona-text:focus{border-color:rgb(var(--other-overlay-rgb));}
.pet-persona-actions{gap:8px;flex-wrap:wrap;}
/* 人格页的三张列表（事实记忆 / 成长笔记 / 人格建议，2026-10-13）—— 与核心人格块同款分隔线；
   行是 JS 渲染，样式按结构类写（行数 / 文本都来自宿主，样式不绑 id）。 */
.pet-list{margin-top:14px;border-top:1px solid var(--border-glass);padding-top:12px;}
.pet-list .pet-note{border-top:0;padding-top:0;margin-top:8px;}
.pet-list-items{display:flex;flex-direction:column;gap:8px;margin:10px 0;}
.pet-list-row{display:flex;flex-direction:column;gap:8px;padding:8px 10px;border:1px solid var(--border-glass);border-radius:8px;}
.pet-list-text{color:var(--text);font-size:.72rem;line-height:1.7;white-space:pre-wrap;word-break:break-word;}
.pet-list-acts{display:flex;flex-wrap:wrap;gap:6px;}
.pet-list-empty{color:var(--text-muted);font-size:.7rem;line-height:1.7;}
/* 两段式确认态（删除 / 清空 / 接受合并）：红色确认，见 docs/ui-style.md §界面 2 / §界面 11 白名单。 */
.pet-list-del[data-armed="1"],.pet-list-clear[data-armed="1"],.pet-suggest-accept[data-armed="1"]{color:var(--red);background:rgba(192,138,138,0.14);border-color:var(--red);}
.pet-debug{margin-top:12px;border-top:1px solid var(--border-glass);padding-top:11px;}
.pet-debug-title{font-weight:600;color:var(--text);margin-bottom:8px;}
.pet-debug-note{color:var(--text-dim);font-size:.68rem;line-height:1.7;margin-bottom:8px;}
.pet-debug-body.off{opacity:.45;pointer-events:none;}
.pet-param{display:flex;align-items:center;gap:8px;margin-bottom:8px;}
.pet-param .pet-label{min-width:5.5em;}
.pet-range{flex:1;min-width:0;height:4px;appearance:none;-webkit-appearance:none;background:var(--border-glass);border-radius:3px;outline:none;cursor:pointer;}
.pet-range::-webkit-slider-thumb{appearance:none;-webkit-appearance:none;width:14px;height:14px;border-radius:50%;background:rgb(var(--btn-line-rgb));border:0;cursor:pointer;}
.pet-range::-moz-range-thumb{width:14px;height:14px;border-radius:50%;background:rgb(var(--btn-line-rgb));border:0;cursor:pointer;}
.pet-param .pet-param-num{flex:0 0 auto;width:54px;box-sizing:border-box;padding:4px 7px;font:inherit;font-size:.7rem;text-align:right;font-variant-numeric:tabular-nums;color:var(--text);background:rgba(var(--shade-rgb),calc(0.3 * var(--shade-scale)));border:1px solid var(--border-glass);border-radius:5px;outline:none;caret-color:rgb(var(--other-overlay-rgb));}
.pet-param .pet-param-num:focus{border-color:rgba(var(--btn-line-rgb), var(--btn-line-alpha));}
.pet-debug-sub{color:var(--text-dim);margin:10px 0 6px;}
.pet-exprs{display:flex;flex-wrap:wrap;gap:6px;line-height:1.6;max-height:180px;overflow-y:auto;padding-right:4px;align-content:flex-start;}
.pet-expr-btn{appearance:none;font:inherit;font-size:.72rem;color:var(--text);background:var(--surface-glass);border:1px solid var(--border-glass);border-radius:8px;padding:5px 10px;cursor:pointer;}
.pet-expr-btn:hover{border-color:rgb(var(--other-overlay-rgb));color:rgb(var(--other-overlay-rgb));}
.pet-expr-reset{color:var(--text-dim);}
.pet-expr-btn.on{background:rgb(var(--other-overlay-rgb));border-color:rgb(var(--other-overlay-rgb));color:var(--text);font-weight:600;}
.pet-expr-btn.on:hover{color:var(--text);opacity:.92;}
.pet-proactive{margin-top:12px;border-top:1px solid var(--border-glass);padding-top:11px;}
.pet-proactive .pet-note{margin-top:8px;border-top:0;padding-top:0;}
.pet-proactive .pet-param-num{flex:0 0 auto;width:64px;box-sizing:border-box;padding:4px 7px;font:inherit;font-size:.7rem;text-align:right;font-variant-numeric:tabular-nums;color:var(--text);background:rgba(var(--shade-rgb),calc(0.3 * var(--shade-scale)));border:1px solid var(--border-glass);border-radius:5px;outline:none;caret-color:rgb(var(--other-overlay-rgb));}
.pet-proactive .pet-param-num:focus{border-color:rgba(var(--btn-line-rgb), var(--btn-line-alpha));}
.pet-tts{margin-top:12px;border-top:1px solid var(--border-glass);padding-top:11px;}
.pet-tts .pet-note{margin-top:8px;border-top:0;padding-top:0;}
/* 「参考音色」子块（2026-10-12）：参考文本输入框 + 「添加音频文件 / 试听」两枚按钮 + 状态行。 */
.pet-tts-voice{margin-top:10px;padding-top:10px;border-top:1px solid var(--border-glass);}
.pet-tts-voice .pet-label{display:block;margin-bottom:6px;}
.pet-tts-voice .pet-row{margin-top:8px;}
.pet-tts-actions{margin-top:8px;}
.pet-tts-text{display:block;width:100%;box-sizing:border-box;font:inherit;font-size:.72rem;color:var(--text);background:rgba(var(--shade-rgb),calc(0.3 * var(--shade-scale)));border:1px solid var(--border-glass);border-radius:5px;padding:6px 8px;outline:none;caret-color:rgb(var(--other-overlay-rgb));}
.pet-tts-text:focus{border-color:rgba(var(--btn-line-rgb), var(--btn-line-alpha));}
.pet-tts-ok{color:var(--green);}
.pet-tts .pet-bad{color:var(--red);}
/* 热键区（2026-10-10）—— 行是 JS 渲染的动态列表，样式必须按结构类写，不能绑 id。 */
.pet-hotkeys{margin-top:12px;border-top:1px solid var(--border-glass);padding-top:11px;}
.pet-hotkeys .pet-note{border-top:0;padding-top:0;margin-bottom:8px;}
.pet-hk-list{display:flex;flex-direction:column;gap:8px;margin-bottom:10px;}
.pet-hk-row{display:flex;flex-wrap:wrap;gap:6px;align-items:center;padding:8px;border:1px solid var(--border-glass);border-radius:8px;}
.pet-hk-row input,.pet-hk-row select{font:inherit;font-size:.7rem;color:var(--text);background:rgba(var(--shade-rgb),calc(0.3 * var(--shade-scale)));border:1px solid var(--border-glass);border-radius:5px;padding:4px 7px;outline:none;}
.pet-hk-row input:focus,.pet-hk-row select:focus{border-color:rgba(var(--btn-line-rgb), var(--btn-line-alpha));}
.pet-hk-name{flex:1 1 96px;min-width:72px;}
.pet-hk-target{flex:1 1 96px;min-width:72px;color:var(--text-dim);font-size:.7rem;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;}
/* 热键行的「类型 / 表情对象」改**自绘下拉**（.custom-select，共享组件，2026-10-11）：
   行内可伸缩（原先原生 select 无 flex、按内容宽度）。 */
.pet-hk-row .custom-select{flex:1 1 110px;min-width:96px;max-width:none;}
.pet-hk-num{flex:0 0 auto;width:56px;box-sizing:border-box;text-align:right;font-variant-numeric:tabular-nums;}
.pet-hk-color{flex:0 0 auto;width:40px;height:24px;box-sizing:border-box;padding:1px 2px;cursor:pointer;}
.pet-hk-run{flex:0 0 auto;color:rgb(var(--other-overlay-rgb));}
.pet-hk-del{flex:0 0 auto;color:var(--red);}
.pet-hk-del:hover{color:var(--red);border-color:var(--red);}
/* 预设动作页（2026-10-10）—— 与热键行同一套结构类写法（动态列表，不绑 id）。 */
.pet-motions{margin:0;}
.pet-mo-list{display:flex;flex-direction:column;gap:8px;margin-bottom:10px;}
.pet-mo-row{display:flex;flex-wrap:wrap;gap:6px;align-items:center;padding:8px;border:1px solid var(--border-glass);border-radius:8px;}
.pet-mo-row input,.pet-mo-row select{font:inherit;font-size:.7rem;color:var(--text);background:rgba(var(--shade-rgb),calc(0.3 * var(--shade-scale)));border:1px solid var(--border-glass);border-radius:5px;padding:4px 7px;outline:none;}
.pet-mo-row input:focus,.pet-mo-row select:focus{border-color:rgba(var(--btn-line-rgb), var(--btn-line-alpha));}
.pet-mo-name{flex:1 1 96px;min-width:72px;}
.pet-mo-row select{flex:1 1 110px;min-width:88px;}
.pet-mo-run{flex:0 0 auto;color:rgb(var(--other-overlay-rgb));}
.pet-mo-del{flex:0 0 auto;color:var(--red);}
.pet-mo-del:hover{color:var(--red);border-color:var(--red);}
/* 调试区的开关形态 + 参数形态覆盖按钮（2026-10-10）。 */
.pet-param-on{flex:0 0 auto;min-width:52px;}
.pet-param-kind{flex:0 0 auto;display:inline-flex;align-items:center;justify-content:center;appearance:none;width:24px;height:20px;color:var(--text-dim);background:transparent;border:1px solid var(--border-glass);border-radius:999px;padding:0;cursor:pointer;line-height:0;}
.pet-param-kind:hover{color:var(--text);border-color:rgba(var(--btn-line-rgb), var(--btn-line-alpha));}
/* ── 动作链（2026-10-10，backlog ㉝②；放在动作页最上面）── */
.pet-chains{margin-bottom:12px;padding-bottom:10px;border-bottom:1px solid var(--border-glass);}
.pet-chain-list{display:flex;flex-direction:column;gap:10px;margin-top:8px;}
.pet-chain{padding:8px;border:1px solid var(--border-glass);border-radius:8px;background:rgba(var(--shade-rgb),calc(0.14 * var(--shade-scale)));}
.pet-chain-head{display:flex;gap:6px;align-items:center;flex-wrap:wrap;}
/* 展开 / 收起箭头（2026-10-11）：小方按钮，SVG 跟随文字色；展开时旋转 90 度。 */
.pet-chain-toggle{flex:0 0 auto;display:inline-flex;align-items:center;justify-content:center;width:22px;height:22px;padding:0;line-height:0;color:var(--text-dim);}
.pet-chain-toggle svg{transition:transform .12s ease;}
.pet-chain-toggle.on svg{transform:rotate(90deg);}
.pet-chain-name{flex:1 1 110px;min-width:80px;font:inherit;font-size:.72rem;color:var(--text);background:rgba(var(--shade-rgb),calc(0.3 * var(--shade-scale)));border:1px solid var(--border-glass);border-radius:5px;padding:4px 7px;outline:none;}
.pet-chain-name:focus{border-color:rgba(var(--btn-line-rgb), var(--btn-line-alpha));}
.pet-chain-total{flex:0 0 auto;font-size:.66rem;color:var(--text-dim);}
.pet-chain-loopbtn{flex:0 0 auto;}
/* 折叠态**摘要**：一行、过长省略（只显示「文件 · 循环 · 硬停」的拼接）。 */
.pet-chain-summary{display:none;margin:6px 0 0;font-size:.66rem;line-height:1.5;color:var(--text-dim);white-space:nowrap;overflow:hidden;text-overflow:ellipsis;}
.pet-chain.collapsed .pet-chain-summary{display:block;}
/* 折叠：藏起步骤列表与「添加步骤」。 */
.pet-chain.collapsed .pet-chain-steps{display:none;}
.pet-chain.collapsed [data-chain-step-add]{display:none;}
.pet-chain-steps{display:flex;flex-direction:column;gap:6px;margin:8px 0;}
/* 每一步 = 上「一行四个控件」（名称 / 循环 / 硬停 / 删除，**不换行**）+ 下「进度条」。 */
.pet-chain-step{display:flex;flex-direction:column;gap:0;}
.pet-chain-fields{display:flex;align-items:center;gap:6px;flex-wrap:nowrap;min-width:0;}
/* 动作链每一步的**动作文件下拉**改自绘 .custom-select（2026-10-11）：占满该行剩余宽度。 */
.pet-chain-fields .custom-select{flex:1 1 auto;min-width:0;max-width:none;}
/* 仍为**原生 select** 的几处（热键行的 group / id / mode / enabled、动作行）：
   收合面按 docs/ui-style.md §界面 12 基准对齐（高 26 / 圆角 6 / 内边距 5px 8px /
   font-size .73rem / 底 rgba(var(--shade-rgb), calc(0.25 * var(--shade-scale))) /
   描边 var(--border-glass) = 按钮线条 α0.10 / 文字 var(--text)）；弹出层由 OS 绘制、
   不可样式化（§界面 12 硬豁免）。
   ⚠️ 热键行的「类型 / 表情对象」与动作链步骤的动作文件**已改自绘 .custom-select**（外观由
   styles.css 的共享规则给），不再进本组。
   单独成组、不并进上面的 input,select 共享规则：那条规则里有一批 input 专有覆盖
   （.pet-hk-color 的圆角 / 内边距等），合并进去会误伤它们。 */
.pet-hk-row select,.pet-mo-row select{box-sizing:border-box;height:26px;font-size:.73rem;background:rgba(var(--shade-rgb),calc(0.25 * var(--shade-scale)));border:1px solid var(--border-glass);border-radius:6px;padding:5px 8px;color:var(--text);}
.pet-chain-t{flex:0 0 auto;display:inline-flex;align-items:center;gap:3px;font-size:.66rem;color:var(--text-dim);white-space:nowrap;}
/* 「循环」「硬停」两个数字框**收窄**到约 4 个字符宽（用户 2026-10-11 口径）：
   字号 .7rem 下 4.2em 约 47px，去掉内边距 / 边框后仍能看清「3.0」「12.5」这类值。
   右对齐 + 等宽数字，多位数字也稳。 */
.pet-chain-num{width:4.2em;box-sizing:border-box;font:inherit;font-size:.7rem;color:var(--text);background:rgba(var(--shade-rgb),calc(0.3 * var(--shade-scale)));border:1px solid var(--border-glass);border-radius:5px;padding:3px 5px;outline:none;text-align:right;font-variant-numeric:tabular-nums;caret-color:rgb(var(--other-overlay-rgb));}
.pet-chain-del{flex:0 0 auto;color:var(--red);padding:2px 8px;}
.pet-chain-run{flex:0 0 auto;}
/* 进度条：动作页每行 / 动作链每步下方（子元素 i 为填充、span 为覆盖文字）。 */
.pet-chain-bar{position:relative;flex:1 1 100%;height:16px;margin-top:3px;border-radius:5px;overflow:hidden;background:rgba(var(--shade-rgb),calc(0.28 * var(--shade-scale)));border:1px solid var(--border-glass);}
/* 动作链里进度条是「一行控件」的下方兄弟（父容器已是竖向 flex）⇒ 改成不收缩、占满整行宽。 */
.pet-chain-step .pet-chain-bar{flex:0 0 auto;width:100%;}
.pet-chain-bar i{position:absolute;left:0;top:0;bottom:0;width:0;background:rgb(var(--other-overlay-rgb));opacity:.35;transition:width .08s linear;}
.pet-chain-bar span{position:absolute;left:6px;right:6px;top:0;bottom:0;display:flex;align-items:center;font-size:.62rem;color:var(--text-dim);white-space:nowrap;overflow:hidden;text-overflow:ellipsis;pointer-events:none;}
`;

let petCleanup: (() => void) | null = null;

async function attachPetWindow(root: HTMLElement): Promise<void> {
  const el = <T extends HTMLElement>(id: string) => root.querySelector<T>(`#${id}`);
  const stage = el("pet-stage");
  const img = el<HTMLImageElement>("pet-img");
  const l2d = el("pet-l2d");
  const errBox = el("pet-err");
  const toastBox = el("pet-toast");
  const empty = el("pet-empty");
  const menu = el("pet-menu");
  // ── AI 对话（2026-10-09 / 布局改 2026-10-10）────────────────────
  // 输出框在模型**上方**、输入行在模型**下方**（都是 `.pet-root` 的直接子层，不是 stage 的子节点）：
  // 所以点输入框 / 气泡不会冒泡到 stage 的 mousedown ⇒ **窗口拖动、Alt 拖动形象、滚轮缩放都不受影响**。
  // 收发契约另见文件头 CHAT_SEND_EVENT / CHAT_REPLY_EVENT。显隐在 `.pet-root` 上挂
  // `chat-on`（输出）/ `chat-input-on`（输入）两个类，见 setChatOpen。
  const petRoot = el("pet-root");
  const chatOut = el("pet-chat-bubble");
  const chatText = el("pet-chat-text");
  const chatClose = el("pet-chat-close");
  const chatStop = el("pet-chat-stop");
  const chatInput = el<HTMLTextAreaElement>("pet-chat-input");
  const resizeHandle = el("pet-resize-handle");

  // 画布（模型区）尺寸**参考值**注入 CSS 变量：唯一真相源 = pet-window-size 的常量（别在 CSS 里
  // 再写一份 300/400）。--pet-stage-gap 供 .pet-stage 的左右留白；--pet-stage-ref-w/h 供 .pet-img
  // 定尺（图片分支不随画布放大）。Live2D 的基准缩放另经 mountLive2D 的 refStageW/H 传入。
  petRoot?.style.setProperty("--pet-stage-gap", `${STAGE_GAP}px`);
  petRoot?.style.setProperty("--pet-stage-ref-w", `${STAGE_W}px`);
  petRoot?.style.setProperty("--pet-stage-ref-h", `${STAGE_H}px`);

  let raf = 0;
  let cfg = loadConfig();
  /** Live2D 那一份实例。图片模式下恒为 null。 */
  let l2dHandle: Live2DHandle | null = null;
  /** 正在播的动作链 id（进度上报时带上，设置窗据此只点亮**这条链**的进度条）。 */
  let playingChainId = "";
  /** 连发几次重画时只认最后一次 —— 装载是异步的，慢的那次不能覆盖快的。 */
  let paintGen = 0;
  /** 窗口是否可见。**最小化 / 隐藏时动画必须停**（WebView2 对最小化窗仍报 `visible`）。 */
  let visible = true;

  // ── 输出框「打字机」逐字动画（2026-10-12）────────────────────────────
  // 速率 / 单位切分 / 换行归一的**纯逻辑**在 `./pet-typewriter`（可 node 单测）；这里只管 DOM 与定时器。
  let typeTimer: ReturnType<typeof setInterval> | null = null;
  let typeUnits: string[] = [];
  let typeIdx = 0;
  let typeRevealed = "";
  let typeDone: (() => void) | null = null;
  /** 用户当前是否停在底部（自动跟随最后揭示的字符）。手动上滑 ⇒ false（暂停跟随），
   *  回到底部 ⇒ 恢复 true。由下面 `chatOut` 的 scroll 监听维护。 */
  let followBottom = true;
  // **自动跟随**：每拍滚到底；但**用户手动上滑后暂停**（回到底部再恢复），不跟用户抢滚动位置。
  chatOut?.addEventListener("scroll", () => {
    if (!chatOut) return;
    followBottom = chatOut.scrollHeight - chatOut.scrollTop - chatOut.clientHeight <= 2;
  });

  function showErr(msg: string) {
    if (!errBox) return;
    errBox.textContent = msg;
    errBox.classList.toggle("on", !!msg);
  }

  /** 「看一眼屏幕」的短时可见标识（隐私红线）。连发时重置计时，只留最后一次。 */
  let toastTimer: ReturnType<typeof setTimeout> | null = null;
  function showToast(msg: string, ms: number) {
    if (!toastBox) return;
    toastBox.textContent = msg;
    toastBox.classList.add("on");
    if (toastTimer) clearTimeout(toastTimer);
    toastTimer = setTimeout(() => {
      toastTimer = null;
      toastBox.classList.remove("on");
    }, ms);
  }

  /** 卸掉 Live2D 实例（画布与纹理一起收）。**不碰 `on` 类** —— 那个类由 `paint()` 决定，
   *  在这里顺手摘掉会把「上一行刚加上去的那次」一起摘掉（实测：画布停在 1×1）。
   *
   *  ⚠️ **`paint()` 在挂新模型之前会先调它，所以这里绝不允许抛**（2026-10-10 用户报
   *  「点开启循环播放依旧卡死」）：引擎侧已经吞掉了 pixi `Application.destroy()` 的异常，
   *  这里再兜一层 —— 任何销毁异常都不许中断「拆旧 → 挂新」这条链，否则新模型永远挂不上、
   *  旧模型又已拆掉，用户看到的就是「卡死」。 */
  function dropModel() {
    try {
      l2dHandle?.destroy();
    } catch (e) {
      console.warn("[pet] dropModel failed:", e);
    }
    l2dHandle = null;
  }

  // ── 渲染形象 ──────────────────────────────────────────────────
  // 两种来源**互斥**（存的时候就互斥了，见控制台的 pickShape）：模型优先。
  async function paint(): Promise<void> {
    const gen = ++paintGen;
    cfg = loadConfig();
    showErr("");
    const modelPath = cfg.model.trim();
    const imagePath = cfg.image.trim();

    if (modelPath && l2d) {
      if (img) {
        img.removeAttribute("src");
        img.style.display = "none";
      }
      if (empty) empty.style.display = "none";
      l2d.classList.add("on");
      stopLoop(); // 图片那条「呼吸感」rAF 与 Live2D 无关，模型模式下必须停
      dropModel();
      // **自愈**（2026-10-10 用户报「下方多出一个模型」）：旧版本在 `app.destroy` 抛错时会把画布
      // 留在 DOM 里（引擎已修，见 live2d-engine.ts 的 `canvas.remove()`）；这里再无条件清一次
      // 残留画布 —— 这样即使已经处于「两个模型上下排列」的状态，重挂一次即恢复。
      l2d.querySelectorAll("canvas").forEach(c => c.remove());
      // 重挂模型 ⇒ 引擎里那些叠加表情层随实例一起没了，开关状态要跟着清并向设置窗广播。
      activeHotkeys.clear();
      broadcastHotkeyStates();
      try {
        const { mountLive2D } = await loadEngine();
        // 导入的表情**不在模型同层**时引擎扫不到 ⇒ 从热键文件里把它们收集起来一并交进去，
        // 否则「列得出、点了切不动」（见 collectHotkeyExpressions）。
        const extraExpressions = await collectHotkeyExpressions(modelPath);
        // 预设动作：扫文件夹里的 `*.motion3.json`（不看 model3.json 的声明），按**同一份清单**
        // （`list_motion_files` 的路径序）注入成合成组 —— 序号必须与「预设动作」页的编号一致。
        const allMotions = await listMotionFiles(modelPath);
        const extraMotions = allMotions.map(f => ({ name: f.name, file: f.path }));
        // 「待机循环」：只把用户勾过的那几个交给引擎 —— 引擎自己驱动（按顺序轮播 + 超时兜底），
        // **不再借库的 idle 组**（那条路会让渲染永久卡死，见 live2d-engine.ts 的注释）。
        const idle = idleNameSet(modelPath);
        const idleMotions = allMotions.filter(f => idle.has(f.name.toLowerCase())).map(f => ({ name: f.name, file: f.path }));
        const h = await mountLive2D(l2d, {
          modelPath,
          scale: cfg.scale,
          offsetX: cfg.offsetX,
          offsetY: cfg.offsetY,
          rotation: cfg.rotation,
          // 参考模型区尺寸（缺省窗口下的 300×400）：模型基准缩放按它算 ⇒ 画布比它大时不放大、
          // 比它小时按画布收（见 live2d-engine 的 fit 与 pet-window-size 的 modelBaseScale）。
          refStageW: STAGE_W,
          refStageH: STAGE_H,
          extraExpressions,
          extraMotions,
          idleMotions,
          // 动作进度（2026-10-10）：引擎 → 本窗 → 设置窗画进度条（`motion-progress` 广播）。
          onMotionProgress: (p: MotionProgress | null) => {
            void emit(CTL_EVENT, {
              type: "motion-progress",
              chainId: playingChainId,
              p: p
                ? { index: p.index, elapsedMs: p.elapsedMs, totalMs: p.totalMs, chainStep: p.chainStep }
                : null,
            });
          },
        });
        if (gen !== paintGen) {
          h.destroy(); // 期间又换了一次形象 ⇒ 这一份作废
          return;
        }
        l2dHandle = h;
        // 装载期间用户可能又拖了大小滑块 / Alt 移动了形象 ⇒ 用最新配置对齐一次
        const latest = loadConfig();
        // **运行时 id 表 + 调试参数**一并落模型目录下的 `lunac-live2d\lunac.params.json`（2026-10-10，backlog ㉝③）：
        //   · `coreIds` 用引擎刚给的运行时表覆盖 ⇒ 设置窗**进插件读取之前**就能拿到它列目标
        //     （以前只有 localStorage 那份，没开过桌宠时是空的 —— 调试区 / 热键页一片空）；
        //   · 附带 `enumerateTargets` 的**物理输入 / 输出分类** ⇒ 桌宠轮把它注进给 AI 的参数词表，
        //     让 AI 知道哪些参数会被物理每帧覆盖（见 main.ts 的 buildPetActionHint）；
        //   · `params` 优先用**文件里的**（AI 可能刚改过，别覆盖它），文件为空再回退 config 播种；
        //   · 失败只 warn：一份坏缓存不该把桌宠窗搞崩。
        let fileParams: Record<string, number> | null = null;
        try {
          const ids = h.listCoreIds();
          const cache = (await loadPetParams(modelPath)) ?? emptyPetParamCache(modelPath);
          if (ids.parameters.length || ids.drawables.length || ids.parts.length) {
            const tg = await enumerateTargets(modelPath, ids);
            const coreIds: CoreIdTable = {
              ...ids,
              physicsInputs: tg.physicsInputs,
              physicsOutputs: tg.physicsOutputs,
            };
            cache.coreIds = coreIds;
            // localStorage 那份仍写：文件尚未生成时的老路径继续可用（桌宠轮读的就是它）。
            localStorage.setItem(CORE_IDS_KEY + modelPath.toLowerCase(), JSON.stringify(coreIds));
          }
          const cfgParams = latest.params[modelPath];
          if ((!cache.params || !Object.keys(cache.params).length) && cfgParams && typeof cfgParams === "object") {
            cache.params = { ...cfgParams };
          }
          fileParams = cache.params;
          await savePetParams(modelPath, cache);
        } catch (e) {
          console.warn("[pet] 读/写 lunac.params.json 失败", e); // 拿不到就不写：设置窗会提示「先显示桌宠一次」
        }
        h.setScale(latest.scale);
        h.setOffset(latest.offsetX, latest.offsetY);
        h.setRunning(visible);
        // 该模型**持久化**的表情：装载后自动应用 —— 重开桌宠窗 / 重启应用后表情仍在
        // （高亮的「真相」也在这一份里，见 attachSettings 的 savedExpr）。
        const savedExpr = latest.expressions[modelPath];
        if (savedExpr) h.setExpression(savedExpr);
        // 该模型**调试参数**：装载后自动应用（**优先用文件里的 `params`**，文件为空再回退 config）——
        // 重开桌宠窗参数仍在，且 AI 直接改文件的值能反向生效。
        const applyParams =
          fileParams && Object.keys(fileParams).length ? fileParams : latest.params[modelPath];
        if (applyParams) {
          for (const [id, value] of Object.entries(applyParams)) {
            if (typeof value === "number") h.setParameter(id, value);
          }
        }
        // **热键开关状态**：上次开着的那批重新施加（保持「先应用参数，再 restoreHotkeyStates」的既有次序）。
        // 用户 2026-10-10 口径：隐藏 / 显示模型、关闭 / 重开插件都不该把它们重置；换模型时 id 变了 ⇒ 自然恢复成「都没开」。
        await restoreHotkeyStates(modelPath);
      } catch (e) {
        if (gen !== paintGen) return;
        dropModel();
        l2d.classList.remove("on");
        // 失败要说出来：桌宠窗小、用户可能没在看，所以画布区之外还留了一条
        // 常驻的错误条（`.pet-err`），不然就是「一片透明的空白」。
        showErr(`Live2D 载入失败：${e instanceof Error ? e.message : String(e)}`);
      }
      return;
    }

    dropModel();
    l2d?.classList.remove("on");
    if (img && empty) {
      if (imagePath) {
        img.src = convertFileSrc(imagePath);
        img.style.display = "";
        empty.style.display = "none";
      } else {
        img.removeAttribute("src");
        img.style.display = "none";
        empty.style.display = "";
      }
    }
    if (img) imgTransform(0);
    if (visible) startLoop();
  }

  /** 图片模式的 transform：`translate` 放在 `scale` 之前 ⇒ 位移量是屏幕像素（不被 scale 放大），
   *  与模型模式 `setOffset` 的语义一致。`transform-origin` 由 CSS 钉在底边中心（缩放支座）。 */
  function imgTransform(wobble: number) {
    if (!img) return;
    img.style.transform = `translate(${cfg.offsetX}px, ${cfg.offsetY + wobble}px) scale(${cfg.scale})`;
  }

  /** 只改大小、**不重建**：Live2D 用实例自己的 `setScale`，图片用 transform。 */
  function applyScale(v: number) {
    cfg = { ...cfg, scale: v };
    if (l2dHandle) {
      l2dHandle.setScale(v);
      return;
    }
    imgTransform(0);
  }

  /** 相对基准位置移动形象（Alt+拖动）：Live2D 用实例的 `setOffset`，图片走 transform。 */
  function applyOffset(ox: number, oy: number) {
    cfg = { ...cfg, offsetX: ox, offsetY: oy };
    if (l2dHandle) {
      l2dHandle.setOffset(ox, oy);
      return;
    }
    imgTransform(0);
  }

  /** 整体旋转（角度制）：**Live2D 专属**（图片模式没有旋转这个维度，静默不动）。 */
  function applyRotation(deg: number) {
    cfg = { ...cfg, rotation: deg };
    l2dHandle?.setRotation(deg);
  }

  // ── 呼吸感：一条常驻 rAF。**窗口不可见时必须停掉** ─────────────
  // 这条不是装饰 —— 宿主不给「已隐藏」事件的话它会一直跑（WebView2 的
  // `document.visibilityState` 对最小化窗口永远是 `visible`，见 ai-spec §4.8）。
  const t0 = performance.now();
  function tick(now: number) {
    raf = requestAnimationFrame(tick);
    if (!root.isConnected) return; // 面板被换掉 ⇒ 下一帧不再续（自停兜底）
    if (img && img.style.display !== "none") {
      const y = Math.sin((now - t0) / 1400) * 3;
      imgTransform(y);
    }
  }
  function startLoop() {
    if (!raf) raf = requestAnimationFrame(tick);
  }
  function stopLoop() {
    if (raf) cancelAnimationFrame(raf);
    raf = 0;
  }

  // ── 穿透 ──────────────────────────────────────────────────────
  // 「穿透」= 两个开关一起动：宿主把窗设成点不透（前端收不到鼠标消息），同时**按需**启停
  // 全局 Alt+右键 钩子（穿透态下前端 contextmenu 已死，只能靠全局钩子旁听）。两者同进同退。
  async function applyClickThrough(on: boolean) {
    try {
      await invoke("plugin_window_set_click_through", { ignore: on });
    } catch (e) {
      console.error("[lunac pet] 设置穿透失败：", e);
    }
    // 钩子只在穿透态有意义；非穿透态保持关闭（幂等，重复调是空操作）。
    await invoke("pet_alt_rclick_watch", { on }).catch(() => {});
  }

  // ── 右键菜单 ──────────────────────────────────────────────────
  /** 菜单因「穿透态 Alt+右键」而临时关掉了穿透 ⇒ 关菜单时必须把穿透**恢复**回原状
   *  （否则用户点一次右键就永久失去穿透，而他这次操作的本意并不是改穿透）。 */
  let menuThroughPaused = false;
  /** 右键菜单里那条穿透按钮的文案（开 / 关两态，与设置窗**共用同一个 `throughLabel`**）。 */
  function updateMenuThrough() {
    const b = document.getElementById("pet-menu-through");
    if (b) b.textContent = throughLabel(cfg.clickThrough);
  }
  function closeMenu() {
    menu?.classList.remove("open");
    if (menuThroughPaused) {
      menuThroughPaused = false;
      void applyClickThrough(true);
    }
  }
  function openMenu(x: number, y: number) {
    if (!menu) return;
    updateMenuThrough(); // 每次打开都按当前状态刷一次（设置窗改过 / 上次本菜单改过都要对齐）
    menu.style.left = `${Math.max(2, Math.min(x, window.innerWidth - 140))}px`;
    menu.style.top = `${Math.max(2, Math.min(y, window.innerHeight - 240))}px`;
    menu.classList.add("open");
  }

  /** 穿透态下由全局钩子事件唤出菜单。钩子给的是**屏幕物理像素**，要换算到窗口内 CSS 像素。 */
  async function openMenuFromScreen(screenX: number, screenY: number): Promise<void> {
    // 穿透态点不到菜单 ⇒ 先临时关掉穿透（同时停钩子）；closeMenu() 会按 memory 恢复。
    if (cfg.clickThrough && !menuThroughPaused) {
      menuThroughPaused = true;
      await applyClickThrough(false);
    }
    let x = screenX;
    let y = screenY;
    try {
      const pos = await getCurrentWindow().outerPosition();
      const dpr = window.devicePixelRatio || 1;
      // 本窗无边框（清单 window 段），outer 左上 ≈ 内容区左上；物理像素 ÷ dpr = CSS 像素。
      x = (screenX - pos.x) / dpr;
      y = (screenY - pos.y) / dpr;
    } catch {
      // 换算失败（拿不到窗口位置）：退化为窗口内中心，至少菜单能出现、点得到。
      x = window.innerWidth / 2;
      y = window.innerHeight / 2;
    }
    openMenu(x, y);
  }

  stage?.addEventListener("contextmenu", ev => {
    ev.preventDefault();
    openMenu(ev.clientX, ev.clientY);
  });

  // ── 形象拖动（Alt+左键）与滚轮缩放 ─────────────────────────────
  /** 拖动**形象本身**（不发 `startDragging`，窗口不动）。按下时记住起点与当前偏移，
   *  move 里按累计位移调 `applyOffset`；松手才落盘（拖动中不刷 localStorage）。 */
  let dragFrom: { sx: number; sy: number; ox: number; oy: number } | null = null;
  const onShapeDragMove = (ev: MouseEvent) => {
    if (!dragFrom) return;
    applyOffset(dragFrom.ox + (ev.clientX - dragFrom.sx), dragFrom.oy + (ev.clientY - dragFrom.sy));
  };
  const onShapeDragUp = () => {
    if (!dragFrom) return;
    dragFrom = null;
    window.removeEventListener("mousemove", onShapeDragMove);
    window.removeEventListener("mouseup", onShapeDragUp);
    saveConfig(cfg);
  };
  function startShapeDrag(ev: MouseEvent) {
    dragFrom = { sx: ev.clientX, sy: ev.clientY, ox: cfg.offsetX, oy: cfg.offsetY };
    window.addEventListener("mousemove", onShapeDragMove);
    window.addEventListener("mouseup", onShapeDragUp);
    ev.preventDefault();
    closeMenu();
  }

  // 左键按住空白处 = 拖窗（无边框窗没有系统标题栏，只能自己发起）；
  // **Alt + 左键**改为拖动形象（模型偏移 / 图片 transform），**不**移动窗口。
  stage?.addEventListener("mousedown", ev => {
    if (ev.button !== 0) return;
    if (ev.altKey) {
      startShapeDrag(ev);
      return;
    }
    closeMenu();
    void getCurrentWindow()
      .startDragging()
      .catch(() => {});
  });
  root.addEventListener("mousedown", ev => {
    if (menu?.classList.contains("open") && !menu.contains(ev.target as Node)) closeMenu();
  });

  // ── 窗上「缩放手柄」（2026-10-13）────────────────────────────────
  // 无边框窗没有系统边框可抓 ⇒ 右下角给一个**看得见**的缩放入口：按下即发起宿主的
  // 「拖拽缩放」原生循环（`startResizeDragging`，方向 = 右下角 `SouthEast`）—— 与拖窗口边缘
  // （宿主 `resizable` 提供）是**同一套**原生行为，只是这一条有可视抓手。用户松手后由
  // `onResized` 记进配置（见窗口尺寸那一段）。鼠标穿透开着时收不到事件（符合穿透语义）。
  resizeHandle?.addEventListener("mousedown", ev => {
    if (ev.button !== 0) return;
    ev.preventDefault();
    ev.stopPropagation();
    closeMenu();
    void getCurrentWindow()
      .startResizeDragging("SouthEast")
      .catch(() => {});
  });

  // 滚轮：以**指针所在位置**为锚点缩放（锚点下的像素保持不动）。
  // `{passive:false}` 才能 `preventDefault` 拦住页面级滚动。
  stage?.addEventListener(
    "wheel",
    (ev: WheelEvent) => {
      const rect = stage?.getBoundingClientRect();
      if (!rect) return;
      // 没有形象（模型未载 / 图片未设）时不动；穿透态本就收不到本事件。
      if (!l2dHandle && (!img || img.style.display === "none")) return;
      ev.preventDefault();
      const px = ev.clientX - rect.left; // 舞台内坐标
      const py = ev.clientY - rect.top;
      const factor = ev.deltaY < 0 ? WHEEL_ZOOM_STEP : 1 / WHEEL_ZOOM_STEP;
      const next = Math.min(PET_SCALE_MAX, Math.max(PET_SCALE_MIN, cfg.scale * factor));
      if (next === cfg.scale) return; // 已到缩放上下限
      if (l2dHandle) {
        const r = l2dHandle.zoomAtPointer(factor, px, py);
        cfg = { ...cfg, scale: r.scale, offsetX: r.offsetX, offsetY: r.offsetY };
      } else {
        // 图片模式：锚点缩放走 CSS。`translate(t) scale(s)`，原点在底边中心（见 CSS）。
        const ox = rect.width / 2;
        const oy = rect.height;
        const ratio = next / cfg.scale;
        cfg = {
          ...cfg,
          scale: next,
          offsetX: (px - ox) * (1 - ratio) + ratio * cfg.offsetX,
          offsetY: (py - oy) * (1 - ratio) + ratio * cfg.offsetY,
        };
        imgTransform(0);
      }
      saveConfig(cfg);
      // 让**设置窗的拉条跟着走**（单向：桌宠窗 → 设置窗，见 attachSettings 的监听）。
      void emit(CTL_EVENT, { type: "scale-changed", value: cfg.scale });
    },
    { passive: false },
  );

  // 「看一眼屏幕」：抓当前显示器 → 作为带图片附件的一条消息送进**现有** AI 对话链路。
  // 桌宠窗没有 `__lunac_*` 桥，也不许自己拼请求 —— 所以这里只做两件事：
  //   ① 抓屏并把「那句话 + PNG 路径」暂存进 localStorage（同源各窗共享）；
  //   ② 打开（或前置）聊天独立窗，把 query 经 `input` 递过去触发消费。
  // 消费与真正发消息在 `main.ts` 的聊天窗那一侧（只有它加载 `main.ts`）。
  async function lookScreen(): Promise<void> {
    // 隐私红线：用户必须先看到「它在看屏幕」，再抓 —— 所以标识在前、抓屏在后。
    showToast(t("pet.look_screen_working"), 2000);
    let shot: { path?: string } | null = null;
    try {
      shot = await invoke<{ path: string; width: number; height: number }>("screenshot_monitor", {
        label: PET_LABEL,
      });
    } catch (e) {
      reportPet("看一眼屏幕失败", e);
      showToast(t("pet.look_screen_failed"), 2600);
      return;
    }
    const path = shot?.path;
    if (!path) {
      showToast(t("pet.look_screen_failed"), 2600);
      return;
    }
    // vision 关着 ⇒ 图片块会被静默丢弃。明确告知用户，别让 ta 以为发了。
    let vision = false;
    try {
      vision = !!(await invoke<{ vision?: boolean }>("get_ai_config"))?.vision;
    } catch {
      /* 读不到就按关处理（同 startAgentChat 的口径） */
    }
    if (!vision) {
      showToast(t("pet.look_screen_need_vision"), 3400);
      return;
    }
    const query = t("pet.look_screen_query");
    try {
      localStorage.setItem(LOOK_SCREEN_KEY, JSON.stringify({ query, path }));
    } catch {
      /* 存不下（无痕 / 配额满）：input 仍带 query，退化成纯文本提问 */
    }
    await invoke("open_plugin_window", { pluginId: "chat", key: null, input: query, hidden: true });
  }

  // ── AI 对话（2026-10-09；布局与「不显示输入区」口径 2026-10-10）──────
  // **统一出口**：任何「发一句话给 AI」的来源（输入框、将来的语音 …）都走 `sendToAI`，
  // 它只做两件事 —— emit 契约事件、把气泡切到「思考中」；真正请求、思考过程、对话历史都在
  // `main.ts` 那一侧。桌宠窗**不接收思考过程**：气泡只显示最终回复（用户口径）。
  let chatOpen = false;

  /** 换出 / 收起对话（**不新开窗口**，仍在桌宠窗内）。两个类分别控制输出 / 输入两条带：
   *  · `chat-on`       → 显示**输出**框（贴窗口上沿）。
   *  · `chat-input-on` → 显示**输入**区（贴窗口下沿）。
   *  `focusInput=false` 用于**自动展开**：只展示气泡、不抢走用户的键盘焦点。
   *  `showInput=false`（2026-10-10 用户口径）：**桌宠发起的三种情况不显示输入框** ——
   *  ① 语音输入 ② 主动询问 ③ 抓取屏幕图像；这时只弹输出、几秒后自动收起（见 showOutputBox）。 */
  function setChatOpen(on: boolean, focusInput = true, showInput = true) {
    chatOpen = on;
    if (!on) stopTypewriter(true); // 收起 / 隐藏 ⇒ 立即显全文 + 清定时器（不许隐藏后继续跑动画）
    petRoot?.classList.toggle("chat-on", on);
    petRoot?.classList.toggle("chat-input-on", on && showInput);
    if (on && showInput && focusInput) chatInput?.focus();
    applyOutHeight(true); // 开合对话 ⇒ 输出带高随之变化（关闭时回到下限 75、窗口缩回基准高）
  }

  // ── 窗口尺寸（随内容封顶 + 可调宽高 + 记忆，2026-10-13）────────────────
  // **纯换算在 `./pet-window-size`**（可 node 单测）；这里只管 CSS 变量、量测与下发。
  // 记忆落点 = 本插件自己的 localStorage 配置（`CFG_KEY` 的 winW / winH）：宿主**没有**按插件
  // id 存窗口尺寸的机制（`plugin_window.rs` 的尺寸只来自清单 + 内存里的 clamp，**不落盘**）。
  let winW = cfg.winW; // 窗口宽（用户拖边缘 / 角 / 手柄改过就记这里）
  let winBaseH = cfg.winH; // **无内容**时窗口的基准高（= 510 缺省；用户改过则是他拖出来的高度）
  let outTopH = OUT_MIN_H; // 当前生效的输出带高（= CSS 变量 `--pet-top-h`）
  let lastSentH = -1; // 上次**成功下发**的目标窗口高（跨档节流的比较基准）
  let failedH = -1; // 上次**下发失败**的目标高：同一个值不再重试（clamp / 回读失败不空转重试）
  let resizeSaveTimer: ReturnType<typeof setTimeout> | null = null; // 拖拽记账去抖

  /** 量测输出框正文的真实高度（+ 盒 padding 12 / border 2 = 14）。**必须在文本已写入后量** ——
   *  量的是**正文元素本身**（不受盒 overflow 裁剪影响，拿不到滚动容器的中间态）。 */
  function measureOutContentH(): number {
    if (!chatText) return 0;
    const r = chatText.getBoundingClientRect().height;
    return Number.isFinite(r) ? Math.ceil(r + 14) : 0;
  }

  /** 把输出带高写进 CSS 变量（`.pet-top` 的 flex-basis 与 `.pet-chat-output` 的 height 都取它）。 */
  function setOutTopVar(topH: number) {
    petRoot?.style.setProperty("--pet-top-h", `${topH}px`);
  }

  /** 按当前内容算出目标输出带高，并（必要时）下发窗口尺寸。
   *  `force` = 忽略跨档节流（收尾 / 清空 / 开合对话时用）；逐字增长走 `false`。
   *  ⚠️ **下发失败（clamp / 回读对不上）只记下目标、不重试同一个值** —— 免得陷入死循环重试。 */
  function applyOutHeight(force: boolean) {
    // **最小模型区保护**（2026-10-13 第三轮）：上限**动态**取 `outCap(窗口基准高)` —— 窗矮则上限
    // 自动降低，输出带绝不侵占模型区（见 pet-window-size 的 outCap / STAGE_MIN_H）。
    // 基准高（而非「当前窗高」）是**稳定**的：模型区高 = 基准高 − OUT_MIN_H − IN_H 与带高无关。
    const cap = outCap(winBaseH);
    const wanted = chatOpen && visible ? quantizeTopH(clampOutTop(measureOutContentH(), cap), OUT_STEP, cap) : OUT_MIN_H;
    outTopH = wanted;
    setOutTopVar(wanted);
    if (!visible) return; // 隐藏 / 最小化时不下发（唤出时 applyShown 会重新对齐）
    const targetH = targetWindowHeight(winBaseH, wanted);
    if (!force && !shouldResize(lastSentH, targetH)) return; // 没跨档 ⇒ 不下发（逐字增长不逐帧 resize）
    if (force) failedH = -1; // 收尾 / 清空是权威目标：允许对上次失败的值重试一次
    if (targetH === failedH) return; // 上次同一个目标失败过：不重试
    const h = targetH;
    void invoke("plugin_window_resize_anchored", { width: Math.round(winW), height: Math.round(h) })
      .then(() => {
        lastSentH = h;
      })
      .catch(() => {
        failedH = h; // 失败：记下这个目标，后续同一目标不再重发（防死循环）
      });
  }

  /** 更新气泡正文（**立即**显示，不走逐字动画）。用于「思考中」占位、中断占位等要瞬时可见的文本。
   *  正文先过 `normalizeReplyText`（换行归一 / 连续空白折叠 / 首尾空行丢弃，见 `./pet-typewriter`），
   *  这样 `white-space:pre-wrap` 不会再渲染出**上方 / 下方空白框**（成因见 PET_CSS 里 `.pet-chat-output`
   *  那段注释：pre-wrap 会把换行 / 纯空白文本节点渲染成真实空行）。
   *  每次更新都会**中止**正在跑的逐字动画（不得留悬挂 interval）。 */
  function setChatText(text: string, thinking = false) {
    stopTypewriter(false); // 换内容 ⇒ 先清上一轮动画（新内容马上覆盖它，不必补显全文）
    const shown = normalizeReplyText(text);
    if (chatText) chatText.textContent = shown;
    chatOut?.classList.toggle("thinking", !!thinking);
    followBottom = true;
    if (chatText?.parentElement) chatText.parentElement.scrollTop = 0;
    applyOutHeight(true); // 正文整体换掉 ⇒ 输出带高按新内容重算（立即，不走跨档节流）
  }

  /** 输出框滚到底（让刚揭示的字符保持在视野内）。 */
  function scrollChatToBottom() {
    if (chatOut) chatOut.scrollTop = chatOut.scrollHeight;
  }

  /** 逐字动画**正常跑完**的收尾：清定时器 + 回调（排自动收起）。此时文本已是全文。 */
  function finishTyping() {
    if (typeTimer) {
      clearInterval(typeTimer);
      typeTimer = null;
    }
    const done = typeDone;
    typeDone = null;
    typeUnits = [];
    typeIdx = 0;
    typeRevealed = "";
    applyOutHeight(true); // 动画收尾 ⇒ 按全文对齐一次（补齐最后一段档位）
    if (done) done();
  }

  /** 中止逐字动画并清定时器（**中断 / 新一轮 / 隐藏 / 收起**都走它，保证不留悬挂 interval）。
   *  `showFull=true` ⇒ 把尚未揭示的部分一次性补齐（中断 / 隐藏 / 收起时用，不留半句）；
   *  换轮时用 `false`（那一轮文本马上被「思考中」覆盖，补显无意义）。 */
  function stopTypewriter(showFull: boolean) {
    const running = typeTimer !== null;
    if (typeTimer) {
      clearInterval(typeTimer);
      typeTimer = null;
    }
    if (running) {
      if (showFull && chatText) chatText.textContent = typeUnits.join("");
      if (followBottom) scrollChatToBottom();
    }
    typeUnits = [];
    typeIdx = 0;
    typeRevealed = "";
    const done = typeDone;
    typeDone = null;
    if (running && done) done();
  }

  /** 开始逐字揭示（用户口径：**每秒 N 个单位**）。**每拍重渲染「已揭示的前缀」**（写同一个文本节点的
   *  `textContent`，不追加 DOM）—— 天然不产生追加式实现会留下的空白 / 换行残留。
   *  速率取自 `loadConfig().speechRate`（打字速度，与语音语速相互独立）。收尾 / 上限见 `stopTypewriter` /
   *  `shouldAnimateDirectly`；与自动收起（`PROACTIVE_BOX_MS`）的关系见调用处 `onDone`。 */
  function startTypewriter(text: string, onDone?: () => void) {
    stopTypewriter(false);
    const full = normalizeReplyText(text);
    chatOut?.classList.remove("thinking");
    followBottom = true;
    if (!chatText) {
      onDone?.();
      return;
    }
    const rate = clampRate(loadConfig().speechRate);
    const units = splitUnits(full);
    // 直接整体显示：空文本 / 超长（预估时长封顶）/ 窗口不可见（隐藏时不许跑动画）。
    if (!visible || shouldAnimateDirectly(units.length, rate, ANIM_MAX_MS)) {
      chatText.textContent = full;
      if (chatOut) chatOut.scrollTop = 0;
      applyOutHeight(true); // 整体显示 ⇒ 按全文对齐一次
      onDone?.();
      return;
    }
    typeUnits = units;
    typeIdx = 0;
    typeRevealed = "";
    typeDone = onDone ?? null;
    typeTimer = setInterval(() => {
      typeRevealed += typeUnits[typeIdx] ?? "";
      typeIdx++;
      if (chatText) chatText.textContent = typeRevealed;
      if (followBottom) scrollChatToBottom();
      applyOutHeight(false); // 逐字增长：跨档才下发（节流，见 applyOutHeight）
      if (typeIdx >= typeUnits.length) finishTyping();
    }, 1000 / rate);
  }

  /** 「中止对话」按钮的显隐（2026-10-11）：**只在「正在生成」时出现**。
   *  时序 = 两个时机切换（与聊天侧 `#chat-stop-btn` 由 `isStreaming` 控制同型）：
   *   · `sendToAI` 发出后 → on（进入「思考中」）；
   *   · 收到最终回复 `pet-chat-reply` → off（那一轮收尾，含被中止后的收尾回复）。
   *  ⚠️ **与输出框的 6 秒自动收起（`PROACTIVE_BOX_MS`）无关**：那个只管「气泡展示多久」，
   *  与「AI 是否还在生成」是两件事 —— 不共用、不改动它的时长。 */
  function setPetGenerating(on: boolean) {
    chatStop?.classList.toggle("on", on);
  }

  /** 「发一句话给 AI」的**唯一出口**（后续语音等其它输入复用同一入口，见文件头契约）。
   *  空输入不发（返回 false）；`files` 是可选附件绝对路径。返回是否真的发出。 */
  function sendToAI(query: string, files?: string[]): boolean {
    const q = query.trim();
    if (!q) return false;
    setChatText(t("pet.ai_chat_thinking"), true);
    // **输出框在「输出开始」就弹**（2026-10-11 修问题 1）：此前只在最终回复（`pet-chat-reply`）
    // 到达那一刻弹一次 —— 主动反应 / 抓屏这类「桌宠发起」的轮次在整轮期间（可能十几秒）什么都不显示，
    // 且一旦该轮没产生最终回复（vision 关 / busy / 中断）就永不弹出。这里改成**开头就弹**（只弹、不排自动收），
    // 让用户立刻看到「思考中」；最终回复到达时由逐字动画跑完再起算自动收起倒计时（见那边）。
    // 只在该轮**没有开着输入对话**时弹（chatOpen 为真 = 用户正在看，别动他的输入行）。
    if (!chatOpen) showOutputBox();
    const payload: { query: string; files?: string[] } = { query: q };
    if (files && files.length) payload.files = files;
    void emit(CHAT_SEND_EVENT, payload).catch(e => reportPet("发送 AI 消息失败", e));
    setPetGenerating(true); // 进入「思考中」：显示中止按钮（收尾时由 pet-chat-reply 关掉）
    return true;
  }

  // ── 主动反应：定时触发（2026-10-09）────────────────────────────
  // **成本红线**（用户口径）：默认关、有最小间隔 + 每日上限；**隐藏 / 最小化必须暂停**。
  // 三道闸都在 `scheduleProactive` 里判：启用 + 可见 + 未触顶。计时状态在暂停时保留剩余量。
  let proactiveRemainMs = 0; // 剩余毫秒（暂停时保留，恢复可见后从余量继续）
  let proactiveDeadline = 0; // 运行态：本次到点时刻（`performance.now` 基准）
  let proactiveTimer: ReturnType<typeof setTimeout> | null = null;
  let proactiveWatchdog: ReturnType<typeof setTimeout> | null = null; // 一轮的兜底超时
  let proactiveBusy = false; // 上一轮未收尾（还在「思考中」）⇒ 不叠发
  let outputBoxTimer: ReturnType<typeof setTimeout> | null = null; // 自动弹输出的自动收起
  let outputBoxAuto = false; // 输出区是不是「本轮自动展开」的（用户自己开着的不自动收）
  /** **语音是否还在播**（2026-10-13）：为真时**不起算**自动收起 —— 等宿主 `TTS_DONE_EVENT`
   *  （播完 / 未配好 / 合成失败都会发）把气泡多留一会儿。`tts` 关着时它恒为 false，
   *  于是行为与加这条之前一致（逐字动画跑完即起算原 ≈6s）。 */
  let ttsPlaying = false;
  /** 语音的**兜底看门狗**：万一那次 `TTS_DONE_EVENT` 丢了，3 分钟后强制放行自动收起（绝不永久不隐）。 */
  let ttsWatchdog: ReturnType<typeof setTimeout> | null = null;
  const TTS_WAIT_MAX_MS = 180_000;

  function clearProactiveTimer() {
    if (proactiveTimer) {
      clearTimeout(proactiveTimer);
      proactiveTimer = null;
    }
  }

  /** 安排下一次主动反应。`reset=true` = 从**完整间隔**重新起算（开关 / 参数改动后）；
   *  否则沿用暂停留下的余量（隐藏 → 显示「继续」）。三道闸任一不过就不排。 */
  function scheduleProactive(reset = false) {
    clearProactiveTimer();
    const pr = loadConfig().proactive;
    if (!pr.enabled) return; // 关：不跑
    if (!visible) return; // 不可见：不跑（恢复可见时再排，见 offVis）
    if (proactiveUsedToday() >= pr.dailyCap) {
      scheduleMidnightRecheck(); // 触顶：停到下一个自然日零点再自动重排（跨日自动恢复）
      return;
    }
    if (reset) proactiveRemainMs = 0;
    if (proactiveRemainMs <= 0) proactiveRemainMs = pr.intervalMin * 60_000;
    proactiveDeadline = performance.now() + proactiveRemainMs;
    proactiveTimer = setTimeout(fireProactive, proactiveRemainMs);
  }

  /** 触顶后停到**下一个自然日零点**再重排一次（跨日计数归零后自动恢复）。也占 `proactiveTimer`，
   *  所以隐藏 / 最小化时同样被 `pauseProactive` 暂停（用户口径：不可见时绝不烧 token）。 */
  function scheduleMidnightRecheck() {
    const ms = msUntilNextMidnight();
    proactiveRemainMs = ms;
    proactiveDeadline = performance.now() + ms;
    proactiveTimer = setTimeout(() => {
      proactiveTimer = null;
      proactiveRemainMs = 0;
      scheduleProactive(true);
      scheduleAutoShot(); // 跨日计数归零 ⇒ 两个子开关也恢复节奏
      scheduleScreenWatch();
    }, ms);
  }

  /** 暂停（隐藏 / 最小化）：撤掉计时器，把**剩余毫秒**留给下一次恢复。 */
  function pauseProactive() {
    if (!proactiveTimer) return;
    clearTimeout(proactiveTimer);
    proactiveTimer = null;
    proactiveRemainMs = Math.max(0, proactiveDeadline - performance.now());
  }

  /** 到点开跑一轮：只发一次（走统一出口 `sendToAI`）。这一轮的收尾由 `pet-chat-reply`
   *  触发（见 `onProactiveReply`），看门狗兜底。 */
  function fireProactive() {
    proactiveTimer = null;
    const pr = loadConfig().proactive;
    if (!pr.enabled || !visible) return;
    if (proactiveUsedToday() >= pr.dailyCap) {
      scheduleProactive(); // 期间上限被调小 ⇒ 转去等下一个自然日再排
      return;
    }
    if (proactiveBusy) {
      scheduleProactive(); // 上一轮没结束 ⇒ 跳过本轮，重排
      return;
    }
    proactiveRemainMs = 0;
    proactiveBusy = true;
    if (!sendToAI(t("pet.proactive_prompt"))) {
      proactiveBusy = false;
      scheduleProactive();
      return;
    }
    bumpProactiveUsed();
    void emit(CTL_EVENT, { type: "proactive-used" }); // 让设置窗的「今日已用」当场刷新
    proactiveWatchdog = setTimeout(() => {
      proactiveWatchdog = null;
      if (!proactiveBusy) return;
      proactiveBusy = false;
      scheduleProactive();
    }, PROACTIVE_ROUND_TIMEOUT_MS);
  }

  /** 主动反应那一轮收尾：让用户看到回复（自动弹输出一瞬）+ 排下一轮。用户手打 / 抓屏
   *  发起的轮次 `proactiveBusy` 为 false ⇒ 这里是空操作，不会误动它们的节奏。 */
  function onProactiveReply() {
    if (!proactiveBusy) return;
    proactiveBusy = false;
    if (proactiveWatchdog) {
      clearTimeout(proactiveWatchdog);
      proactiveWatchdog = null;
    }
    scheduleProactive(); // 让用户看到回复的那一步由下面 `pet-chat-reply` 的 showOutputBox 统一处理
  }

  // ── 主动反应的两个**显式 opt-in 子开关**（2026-10-10）──────────────────
  // **隐私红线**（预检 #72 ①）：抓屏**只在用户显式开启**时才发生，且桌宠不可见一律停（`visible` 闸）。
  //  · 「自动抓屏」：每 `autoShotMin` 分钟抓整屏 → 交给 AI 看（走 `screenshot_monitor`）。
  //  · 「屏幕变化触发」：每 `screenWatchSec` 秒抓一张 16×9 指纹（**不落盘**）比上一次，
  //    平均绝对差 ≥ `screenDiff` 就当画面变了 → 主动说一句（走 `screenshot_signature`）。
  // 两者都计入**同一个**每日上限（与主链路共享预算），抓屏那一刻都有可见的 toast 提示。
  let autoShotTimer: ReturnType<typeof setTimeout> | null = null;
  let screenWatchTimer: ReturnType<typeof setTimeout> | null = null;
  let lastSig: number[] | null = null; // 上一张指纹（变化检测用；不持久化，关窗即弃）
  /** 「图片输入没开 ⇒ 抓了也白抓」这条只记**一条**日志（整窗生命周期一次），免得每轮刷屏。 */
  let visionWarned = false;

  /** 抓当前显示器整屏 → 作为图片附件送进**现有** AI 链路（同 `lookScreen`，但这里是自动触发）。
   *  vision 关着 ⇒ 图片块会被静默丢弃 —— 直接不发（别白烧 token）。返回是否真的发出。 */
  async function sendScreenToAI(prompt: string): Promise<boolean> {
    let vision = false;
    try {
      vision = !!(await invoke<{ vision?: boolean }>("get_ai_config"))?.vision;
    } catch {
      /* 读不到就按关处理（同 lookScreen 口径） */
    }
    if (!vision) {
      // **静默跳过是最糟的失败**（用户 2026-10-10 报「并不发送给 AI」时无从判断）—— 落一条 warn 到日志。
      if (!visionWarned) {
        visionWarned = true;
        void invoke("log_frontend", {
          level: "warn",
          message:
            "[pet] 抓屏子开关已到点，但当前未开启「图片输入」⇒ 跳过（图片块会被静默丢弃，不白烧 token）。" +
            "要让它真的发出去，请在 AI 设置里开启图片输入 / 换支持视觉的模型。",
        }).catch(() => {});
      }
      return false;
    }
    showToast(t("pet.look_screen_working"), 2000); // 隐私红线：抓屏**前**先让用户看到「它在看屏幕」
    let shot: { path?: string } | null = null;
    try {
      shot = await invoke<{ path: string; width: number; height: number }>("screenshot_monitor", { label: PET_LABEL });
    } catch (e) {
      reportPet("自动抓屏失败", e);
      return false;
    }
    const path = shot?.path;
    if (!path) return false;
    return sendToAI(prompt, [path]);
  }

  const clearAutoShotTimer = () => {
    if (autoShotTimer) {
      clearTimeout(autoShotTimer);
      autoShotTimer = null;
    }
  };
  const clearScreenWatchTimer = () => {
    if (screenWatchTimer) {
      clearTimeout(screenWatchTimer);
      screenWatchTimer = null;
    }
  };

  /** 安排下一次「自动抓屏」。两道闸：子开关开 + 可见（+ 未触顶）。
   *  ⚠️ **刻意不看「主动反应」总开关**（2026-10-10 用户报障后定）：两个抓屏子开关各自就是
   *  一次**显式 opt-in**（默认关、有间隔与每日上限、有可见 toast），挂在总开关下面只会造成
   *  「开了子开关却什么都不发生、也没有任何提示」——正是用户报的「并没有发送给 AI」。 */
  function scheduleAutoShot() {
    clearAutoShotTimer();
    const pr = loadConfig().proactive;
    if (!pr.autoShot || !visible) return;
    if (proactiveUsedToday() >= pr.dailyCap) {
      // 触顶：跨日自愈（用**自己的**定时器重排，不去抢主链路那个 `proactiveTimer`）
      autoShotTimer = setTimeout(scheduleAutoShot, msUntilNextMidnight());
      return;
    }
    autoShotTimer = setTimeout(fireAutoShot, pr.autoShotMin * 60_000);
  }

  /** 到点抓一屏交给 AI。`proactiveBusy` 与主链路共用 —— 上一轮没收尾就跳过本轮、重排。 */
  async function fireAutoShot() {
    autoShotTimer = null;
    const pr = loadConfig().proactive;
    if (!pr.autoShot || !visible) return;
    if (proactiveUsedToday() >= pr.dailyCap || proactiveBusy) {
      scheduleAutoShot();
      return;
    }
    proactiveBusy = true;
    const ok = await sendScreenToAI(t("pet.autoshot_prompt"));
    if (!ok) {
      proactiveBusy = false;
      scheduleAutoShot();
      return;
    }
    bumpProactiveUsed();
    void emit(CTL_EVENT, { type: "proactive-used" }); // 设置窗的「今日已用」当场刷新
    proactiveWatchdog = setTimeout(() => {
      proactiveWatchdog = null;
      if (!proactiveBusy) return;
      proactiveBusy = false;
    }, PROACTIVE_ROUND_TIMEOUT_MS);
    scheduleAutoShot();
  }

  /** 安排下一次「屏幕变化」轮询。`cooldownMs>0` 时用它当间隔（触发后的冷却），否则用配置的秒数。
   *  与 `scheduleAutoShot` 同一口径：**不看「主动反应」总开关**（见那里的注释）。 */
  function scheduleScreenWatch(cooldownMs = 0) {
    clearScreenWatchTimer();
    const pr = loadConfig().proactive;
    if (!pr.screenWatch || !visible) return;
    if (proactiveUsedToday() >= pr.dailyCap) {
      screenWatchTimer = setTimeout(scheduleScreenWatch, msUntilNextMidnight());
      return;
    }
    screenWatchTimer = setTimeout(fireWatchTick, cooldownMs > 0 ? cooldownMs : pr.screenWatchSec * 1000);
  }

  /** 抓一张极小的指纹与上一张比：变了（且上一轮已收尾）就触发；没变就把这一张当成新基准。 */
  async function fireWatchTick() {
    screenWatchTimer = null;
    const pr = loadConfig().proactive;
    if (!pr.screenWatch || !visible) return;
    if (proactiveUsedToday() >= pr.dailyCap) return;
    let sig: number[] | null = null;
    try {
      const r = await invoke<number[]>("screenshot_signature", { label: PET_LABEL });
      if (Array.isArray(r) && r.length) sig = r.map(n => Number(n) || 0);
    } catch {
      /* 抓不到指纹就跳过这一轮，不打断节奏 */
    }
    if (sig) {
      const changed = !!lastSig && lastSig.length === sig.length && meanAbsDiff(lastSig, sig) >= pr.screenDiff;
      lastSig = sig;
      if (changed && !proactiveBusy) {
        void fireScreenChange(pr);
        return; // 触发那一支在收尾时自行重排
      }
    }
    scheduleScreenWatch();
  }

  /** 画面变化的触发：抓整屏 + 一句 prompt 交给 AI。冷却 = 主链路的**最小间隔**（避免连续变化连发）。 */
  async function fireScreenChange(pr: ProactiveCfg) {
    proactiveBusy = true;
    const ok = await sendScreenToAI(t("pet.watch_prompt"));
    if (!ok) {
      proactiveBusy = false;
    } else {
      bumpProactiveUsed();
      void emit(CTL_EVENT, { type: "proactive-used" });
      proactiveWatchdog = setTimeout(() => {
        proactiveWatchdog = null;
        if (!proactiveBusy) return;
        proactiveBusy = false;
      }, PROACTIVE_ROUND_TIMEOUT_MS);
    }
    lastSig = null; // 触发过：下一次重新建立基准，免得同一次变化反复触发
    scheduleScreenWatch(Math.max(pr.intervalMin, 1) * 60_000);
  }

  /** 进入「不可见」态：停动画 / 关菜单 / 暂停主动反应与两个抓屏子开关。
   *  **隐藏与最小化共用**（`offVis` 的 visible=false 与设置窗的「隐藏模型」都走它）。 */
  function applyHidden() {
    visible = false;
    stopTypewriter(true); // 隐藏 / 最小化 ⇒ 立即显全文 + 清定时器（不许在看不见时继续跑动画）
    stopLoop();
    l2dHandle?.setRunning(false); // Live2D 自己那份 ticker 也要停，见 ai-spec §4.8
    closeMenu();
    pauseProactive(); // 看不见时绝不烧 token
    clearAutoShotTimer(); // 两个抓屏子开关也停（隐私红线）；指纹基准一并丢掉
    clearScreenWatchTimer();
    lastSig = null;
  }

  /** 恢复「可见」态：接着跑动画 + 重排主动反应三个调度器。 */
  function applyShown() {
    visible = true;
    if (l2dHandle) l2dHandle.setRunning(true);
    else startLoop();
    scheduleProactive();
    scheduleAutoShot();
    scheduleScreenWatch();
    applyOutHeight(true); // 唤出 ⇒ 按当前正文重新对齐输出带高（隐藏期间没下发过）
  }

  /** 只弹**输出**、**不显示输入区** —— 用于「桌宠发起」的轮次（主动反应 / 抓取屏幕图像；语音输入同理），
   *  **本身不排自动收起**。自动收起由 `maybeScheduleAutoHide` 单独负责 —— 逐字动画**跑完**、
   *  且**语音播完**（若在朗读）之后才起算 `PROACTIVE_BOX_MS`，避免长回复在动画 / 朗读中途被收起。
   *  用户本来就开着输入对话（chatOpen 为真）⇒ 视为「他正在看」：不弹输入、也不自动收。 */
  function showOutputBox() {
    if (outputBoxTimer) {
      clearTimeout(outputBoxTimer);
      outputBoxTimer = null;
    }
    if (!chatOpen) {
      outputBoxAuto = true;
      setChatOpen(true, false, false); // 不抢焦点、**不要输入区**（第 3 参 false）
    }
  }

  /** 开始朗读（把 `ttsPlaying` 置真并起看门狗）。由 `CHAT_REPLY_EVENT` 处理处调用。 */
  function beginTtsWait() {
    ttsPlaying = true;
    if (ttsWatchdog) clearTimeout(ttsWatchdog);
    ttsWatchdog = setTimeout(() => {
      ttsWatchdog = null;
      ttsPlaying = false; // 兜底：等太久（信号丢了）⇒ 放行自动收起
      maybeScheduleAutoHide();
    }, TTS_WAIT_MAX_MS);
  }

  /** 语音播完（或那次朗读根本没法发声）⇒ 解除等待并（若其余条件也满足）起算自动收起。 */
  function endTtsWait() {
    if (ttsWatchdog) {
      clearTimeout(ttsWatchdog);
      ttsWatchdog = null;
    }
    ttsPlaying = false;
    maybeScheduleAutoHide();
  }

  /** **自动收起的唯一判据**：`outputBoxAuto`（本轮自动展开，用户自己开着的不收）+ 现在没在隐藏
   *  + 逐字动画已停 + 语音已播完。任一不满足就不起算（由对应的完成点再来调一次）。 */
  function maybeScheduleAutoHide() {
    if (!visible) return;
    if (typeTimer !== null) return; // 逐字动画还在跑
    if (ttsPlaying) return; // 语音还在播（TTS 未启用时恒 false，行为同加这条之前）
    scheduleOutputAutoHide();
  }

  /** 起算气泡自动收起（只在「本轮自动展开」`outputBoxAuto` 时生效；用户自己开着的不收）。
   *  唯一调用者是 `maybeScheduleAutoHide`（逐字动画停 + 语音播完之后）⇒ **动画 / 朗读有多长，
   *  气泡就至少多留 `PROACTIVE_BOX_MS`**。 */
  function scheduleOutputAutoHide() {
    if (outputBoxTimer) {
      clearTimeout(outputBoxTimer);
      outputBoxTimer = null;
    }
    if (!outputBoxAuto) return;
    outputBoxTimer = setTimeout(() => {
      outputBoxTimer = null;
      outputBoxAuto = false;
      if (chatOpen) setChatOpen(false);
    }, PROACTIVE_BOX_MS);
  }

  // 输入框：Enter 发送、Shift+Enter 换行。**组合输入中（isComposing）不发送** —— 中日文
  // 输入法回车选词会被误当成发送，这条很关键。
  const onChatKeydown = (ev: KeyboardEvent) => {
    if (ev.key !== "Enter" || ev.shiftKey || ev.isComposing) return;
    ev.preventDefault(); // 拦掉默认换行，Enter 用于发送
    if (chatInput && sendToAI(chatInput.value)) chatInput.value = "";
  };
  const onChatCloseClick = () => setChatOpen(false);
  // 「中止对话」：桌宠窗**不自己停**（那一轮请求在聊天窗手里，本窗直接 `invoke("stop_cli")` 会漏掉
  // 聊天窗前端的收尾状态）—— 只**广播** `CHAT_STOP_EVENT`，由聊天窗复用它既有的 `stopAIChat()` 收尾。
  // 按钮先按下即收起、气泡先切成「已中断」占位（真正的收尾回复随后由 `pet-chat-reply` 覆盖）。
  const onChatStopClick = () => {
    setPetGenerating(false);
    endTtsWait(); // 用户中断 ⇒ 停止语音等待并起算自动收起（沿用「用户中断即刻处理」语义）
    setChatText(t("pet.chat_reply_interrupted"), true);
    void emit(CHAT_STOP_EVENT).catch(e => reportPet("中止 AI 消息失败", e));
  };
  chatInput?.addEventListener("keydown", onChatKeydown);
  chatClose?.addEventListener("click", onChatCloseClick);
  chatStop?.addEventListener("click", onChatStopClick);

  // ── 口型（lip-sync，2026-10-10）────────────────────────────────
  // **为什么是宿主喂包络**：音频在宿主播（WebView 放不出声音，见 ai-spec §4.6）⇒ 宿主在
  // **开始播之前**把这条 WAV 的「张嘴幅度包络」定向发过来（`tts.rs` 的 `MOUTH_EVENT`），
  // 这里按 `fps` 逐格写 `setMouthOpen`，于是嘴与声音走同一条时间线。播完 `null` = 放手。
  let mouthTimer: ReturnType<typeof setInterval> | null = null;
  function driveMouth(env: number[], fps: number) {
    if (mouthTimer) {
      clearInterval(mouthTimer);
      mouthTimer = null;
    }
    if (!env.length || !fps) return;
    const step = 1000 / fps;
    const t0 = performance.now();
    mouthTimer = setInterval(() => {
      const i = Math.floor((performance.now() - t0) / step);
      if (i >= env.length) {
        if (mouthTimer) clearInterval(mouthTimer);
        mouthTimer = null;
        l2dHandle?.setMouthOpen(null); // 说完松手，让动作 / 物理自己演嘴
        return;
      }
      l2dHandle?.setMouthOpen(env[i] / 255);
    }, step);
  }
  // **定向**事件 ⇒ 按纪律用带 label 的 `listen`（不可是裸 `listen`，见 ai-spec §4.5）。
  const offMouth = await getCurrentWindow().listen<{ env?: unknown; fps?: unknown }>(
    MOUTH_EVENT,
    ev => {
      if (!inPetWindow()) return;
      const raw = Array.isArray(ev.payload?.env) ? ev.payload.env : [];
      const env = raw.map(v => (typeof v === "number" && Number.isFinite(v) ? v : 0));
      const fps = typeof ev.payload?.fps === "number" && ev.payload.fps > 0 ? ev.payload.fps : 40;
      driveMouth(env, fps);
    },
  );

  // 收到「桌宠发起的那一轮」的**最终回复**（**定向**到本窗，`emit_to("plugin-pet")`）⇒ 换成正文、退出思考态。
  // **定向**事件 ⇒ 按纪律用带 label 的 `getCurrentWindow().listen`（不可是裸 `listen`，见 ai-spec §4.5 /
  // 同上面的 MOUTH_EVENT）—— 裸 `listen` 的 target 是 `Any`，会把别的窗口的事件也收进来。
  const offChatReply = await getCurrentWindow().listen<{ text?: string }>(CHAT_REPLY_EVENT, ev => {
    if (!inPetWindow()) return;
    const text = typeof ev.payload?.text === "string" ? ev.payload.text : "";
    setPetGenerating(false); // 最终回复（含被中止的收尾回复）到达 ⇒ 结束「思考中」，隐藏中止按钮
    // 语音朗读：**只读最终回复** —— 本事件只在该轮收尾发一次，思考过程 / 流式增量都不经此处。
    // 失败必须 catch 掉，绝不能影响气泡显示（宿主那侧合成失败也会静默跳过，见 src/tts.rs）。
    // 与逐字动画相互独立（2026-10-12 拆分）：动画节拍取本插件 `speechRate`（打字速度），
    // 语音语速 / 音量各自独立（宿主 `PetVoiceConfig.speed` / `volume`）—— 两者不再绑定。
    // **自动收起改为「语音播完再隐」（2026-10-13）**：开朗读时先置 `ttsPlaying`，等宿主
    // `TTS_DONE_EVENT`（播完 / 未配好 / 合成失败都会发）再放行 ⇓ 的起算；`tts` 关着不置位，
    // 于是仍是「动画跑完即起算原 ≈6s」的兜底行为。
    if (text && loadConfig().tts) {
      beginTtsWait();
      void invoke("tts_speak", { text, enabled: true }).catch(() => {
        // 连命令都没发出去（异常）⇒ 解除等待，回落原 ≈6s，别把气泡永久钉住。
        endTtsWait();
      });
    }
    // 让用户看到回复：**自动弹输出**（输入区不显示）。轮次开头已由 `sendToAI` 弹过一次（显示「思考中」）；
    // 这里逐字揭示正文，**动画跑完 + 语音播完后再起算自动收起**（见 maybeScheduleAutoHide）。
    if (visible) showOutputBox();
    startTypewriter(text, () => maybeScheduleAutoHide());
    onProactiveReply(); // 若本轮是主动反应发起的 ⇒ 结束本轮 + 排下一轮（非主动轮是空操作）
  });

  // **「语音播完」信号**（**定向**事件 ⇒ 按纪律用带 label 的 `listen`，同 MOUTH_EVENT / CHAT_REPLY_EVENT）。
  const offTtsDone = await getCurrentWindow().listen(TTS_DONE_EVENT, () => {
    if (!inPetWindow()) return;
    endTtsWait();
  });

  menu?.addEventListener("click", ev => {
    const act = (ev.target as HTMLElement).closest<HTMLElement>("button")?.dataset.act;
    if (!act) return;
    closeMenu();
    void (async () => {
      if (act === "ai-chat") {
        // 同一个菜单项 = 开关：点一次换出、再点一次收起（也可靠气泡上的 × 关）。
        setChatOpen(!chatOpen);
        return;
      }
      if (act === "settings") {
        // 同一份「桌宠设置界面」**另起一窗**（label `plugin-pet-settings`）。
        await invoke("open_plugin_window", { pluginId: "pet", key: "settings", input: "" });
        return;
      }
      if (act === "look-screen") {
        await lookScreen();
        return;
      }
      if (act === "change") {
        // **与控制台同一条路径**（用户 2026-10-10：模型 = 一个文件夹）：选文件夹 → 宿主
        // `scan_model_folder` 取 `*.model3.json` → 校验 → 一并导入作者热键 → 重挂模型。
        // 旧实现是「选单个 .json / 图片」—— 与「模型即文件夹」的口径脱节（选完读不到模型），
        // 图片入口也已按用户口径撤掉，故一并废弃。
        const picked = await openFileDialog({
          multiple: false,
          directory: true,
          title: "选择 Live2D 模型文件夹（里面要有 *.model3.json）",
        });
        const dir = typeof picked === "string" ? picked : "";
        if (!dir) return;
        let model = "";
        let vtube = "";
        try {
          const s = await invoke<{ model3?: string[]; vtube?: string[] }>("scan_model_folder", { dir });
          model = (s.model3 ?? [])[0] ?? "";
          vtube = (s.vtube ?? [])[0] ?? "";
        } catch (e) {
          showToast("读文件夹失败", 2600);
          reportPet("更换模型：读文件夹失败", e);
          return;
        }
        if (!model) {
          showToast("这个文件夹里没有 *.model3.json", 2600);
          return;
        }
        const v = await validateModelFile(model);
        if (!v.ok) {
          showToast(`这个模型用不了：${v.reason}`, 3000);
          return;
        }
        saveConfig({ ...loadConfig(), model, image: "" });
        try {
          await importHotkeysFromFolder(dir, model, vtube); // 作者热键一并读（同控制台）
        } catch (e) {
          reportPet("更换模型：读取热键失败", e);
        }
        void paint();
        return;
      }
      if (act === "reset") {
        // 恢复默认「大小 + 注视点（偏移）+ 旋转」（2026-10-10 起含旋转）：
        // scale = 1、offset = (0,0)、rotation = 0。
        applyScale(1);
        applyOffset(0, 0);
        applyRotation(0);
        saveConfig(cfg);
        void emit(CTL_EVENT, { type: "scale-changed", value: 1 }); // 设置窗的拉条跟着对齐
        void emit(CTL_EVENT, { type: "rotation-changed", value: 0 });
        showToast("已恢复默认大小、位置与角度", 2000);
        return;
      }
      if (act === "win-reset") {
        // 恢复**默认窗口尺寸**（2026-10-13）：宽回 320、基准高回 510，并按当前输出带高重新对齐。
        winW = WIN_W_DEFAULT;
        winBaseH = WIN_BASE_H;
        cfg = { ...cfg, winW: WIN_W_DEFAULT, winH: WIN_BASE_H };
        saveConfig(cfg);
        failedH = -1; // 恢复默认是权威动作：允许对上次失败的目标重试
        applyOutHeight(true);
        showToast(t("pet.win_reset_done"), 2000);
        return;
      }
      if (act === "through") {
        // **切换按钮**（2026-10-09 用户口径）：原来只有「开启鼠标穿透」一条单向动作，点一次就
        // 永久失去穿透（只能去设置窗关）。现在开 / 关都在同一枚按钮上，且两扇窗状态对齐。
        // 注意：若菜单是「穿透态 Alt+右键」唤出的，`closeMenu()` 已先把穿透恢复成原状（cfg 未变），
        // 所以这里 `!cfg.clickThrough` 仍按**用户意图态**取反。
        const on = !cfg.clickThrough;
        cfg = { ...cfg, clickThrough: on };
        saveConfig(cfg);
        await applyClickThrough(on);
        updateMenuThrough();
        // 让设置窗那条按钮跟着对齐（两处状态必须一致）
        void emit(CTL_EVENT, { type: "click-through-changed", value: on });
        return;
      }
      if (act === "close") {
        await invoke("plugin_window_close").catch(() => {});
      }
    })().catch(e => reportPet("菜单动作失败", e));
  });

  // ── 执行一条热键动作（2026-10-10）──────────────────────────────
  // **触发端**：设置窗的「测试」按钮（广播 `hotkey-run`），以及后续的 AI 工具。本地**不做**
  // 按键监听（用户口径：按键仅留给 AI 操作）。目标全来自模型运行时，没有实例就提示一下。
  function runHotkey(a: HotkeyAction): void {
    const h = l2dHandle;
    if (!h) {
      showToast("桌宠还没显示 Live2D 模型", 2600);
      return;
    }
    switch (a.type) {
      case "motion":
        h.playMotion(a.group, a.index);
        break;
      case "chain":
        void playChainById(a.id);
        break;
      case "expression":
        if (a.mode === "clear" || !a.target) h.resetExpressions();
        else h.setExpression(a.target);
        break;
      case "param":
        h.setParameter(a.id, a.mode === "add" ? h.getParameter(a.id) + a.value : a.value);
        break;
      case "artmesh":
        h.setDrawableOpacity(a.id, a.value);
        break;
      case "part":
        h.setPartOpacity(a.id, a.value);
        break;
      case "physics":
        h.setPhysicsEnabled(a.enabled);
        break;
    }
  }

  // ── 热键「开关」状态（2026-10-10）─────────────────────────────
  // 设置窗的「测试」是**状态按钮**：点一下开、再点一下关。表情走**叠加层**（多条共存、
  // 各自可关，见 setExpressionLayer / VTS 的 Expression on/off）；其余类型开=执行、关=尽力还原。
  // 状态只在桌宠窗（有实例的那一侧）维护，改完广播 `hotkey-states` 让设置窗对齐按钮。
  const activeHotkeys = new Set<string>();
  /** 热键 id → 动作类型。**只为「哪些算持续状态」服务**：`motion` 是「播一次」而不是状态，
   *  持久化时跳过它（不然每次挂载都会把它重播一遍）。 */
  const hkTypeById = new Map<string, ActionType>();
  const broadcastHotkeyStates = () => {
    void emit(CTL_EVENT, { type: "hotkey-states", ids: [...activeHotkeys] });
  };

  /** 把当前开着的那批热键**按模型持久化**（用户 2026-10-10 口径：隐藏 / 显示模型、关闭 / 重开
   *  插件都不该重置，**只有换模型才重置** —— 换模型会重新导入热键、id 全变，旧 id 自然对不上）。
   *  ⚠️ **只在 `toggleHotkey` 里调**：挂载流程中 `activeHotkeys` 会先清空再逐个恢复，若在那之前
   *  调本函数就把刚读出来的存档抹掉了。 */
  function saveHotkeyStates() {
    const model = loadConfig().model.trim();
    if (!model) return;
    const ids = [...activeHotkeys].filter(id => {
      const ty = hkTypeById.get(id);
      return ty !== "motion" && ty !== "chain"; // 「播一次」的那两类没有持续状态，不记
    });
    const c = loadConfig();
    const next = { ...c.hotkeyStates };
    if (ids.length) next[model] = ids;
    else delete next[model];
    saveConfig({ ...c, hotkeyStates: next });
  }

  /** 模型挂载完成后，把**上次开着**的热键重新施加一遍（`motion` 类跳过：没有持续状态）。
   *  找不到 id（换了模型 / 重新导入过 ⇒ id 变了）就静默跳过 —— 那正是「换模型才重置」的语义。 */
  async function restoreHotkeyStates(modelPath: string): Promise<void> {
    const ids = loadConfig().hotkeyStates[modelPath] ?? [];
    if (!ids.length) return;
    let list: Hotkey[];
    try {
      list = await loadHotkeys(modelPath);
    } catch {
      return;
    }
    const byId = new Map(list.map(h => [h.id, h]));
    for (const id of ids) {
      const hk = byId.get(id);
      if (!hk || hk.action.type === "motion" || activeHotkeys.has(id)) continue;
      await toggleHotkey(id, hk.action);
    }
  }

  /** **播一条动作链**（2026-10-10）：按 id 从 `lunac.chains.json` 读回来，把「文件 → 注入组下标」
   *  折算好交给引擎。文件不在了的步骤直接跳过（链里留死项不该整条播不了）。 */
  async function playChainById(id: string): Promise<void> {
    const h = l2dHandle;
    const modelPath = cfg.model.trim();
    if (!h || !modelPath || !id) {
      showToast("桌宠还没显示 Live2D 模型", 2600);
      return;
    }
    const list = await loadChains(modelPath);
    const chain = list.find(c => c.id === id);
    if (!chain || !chain.steps.length) {
      showToast("这条动作链是空的", 2200);
      return;
    }
    const files = await listMotionFiles(modelPath);
    const steps = chain.steps
      .map(s => {
        const index = files.findIndex(f => f.path.toLowerCase() === s.file.toLowerCase());
        return { index, ms: stepSeconds(s) * 1000, hardMs: s.stopSec > 0 ? s.stopSec * 1000 : 0 };
      })
      .filter(s => s.index >= 0);
    if (!steps.length) {
      showToast("动作链里的动作文件都不在了", 2400);
      return;
    }
    playingChainId = id;
    // 整链循环（2026-10-11）：`chain.loop` 为真时，引擎在本链末步播完后从头再来。
    h.playChain(steps, chain.loop);
  }

  async function toggleHotkey(id: string, a: HotkeyAction): Promise<void> {
    const h = l2dHandle;
    if (!h) {
      showToast("桌宠还没显示 Live2D 模型", 2600);
      return;
    }
    const on = !activeHotkeys.has(id);
    hkTypeById.set(id, a.type);
    switch (a.type) {
      case "expression": {
        const params = a.file ? await readExpressionParams(a.file) : [];
        if (params.length) h.setExpressionLayer(id, on ? params : null);
        else if (on) {
          if (a.target) h.setExpression(a.target); // 读不到参数集：退回库的互斥表情
        } else h.resetExpressions();
        break;
      }
      case "artmesh":
        h.setDrawableOpacity(a.id, on ? a.value : 1);
        break;
      case "artmeshColor":
        // 开 = 染成该色、关 = 还原原色（`null`）。
        h.setDrawableColor(a.id, on ? hexToRgb(a.color) : null);
        break;
      case "part":
        h.setPartOpacity(a.id, on ? a.value : 1);
        break;
      case "physics":
        h.setPhysicsEnabled(on ? a.enabled : !a.enabled);
        break;
      case "param":
        h.setParameter(a.id, on ? a.value : 0);
        break;
      case "motion":
        if (on) h.playMotion(a.group, a.index);
        break;
      case "chain":
        // 动作链是**一次播放**（不是开关状态）：开 = 播一遍；关 = 停在当前步。
        if (on) await playChainById(a.id);
        else { playingChainId = ""; h.stopChain(); }
        break;
    }
    if (on) activeHotkeys.add(id);
    else activeHotkeys.delete(id);
    broadcastHotkeyStates();
    saveHotkeyStates(); // 状态落盘：隐藏 / 显示 / 重开插件后仍保持（只有换模型才重置）
  }

  // ── AI 工具动作（`PetControl` → 前端广播 `ai-action`，2026-10-10）─────
  // agent 侧的 `PetControl` 只回一句回执（不碰本机）；真正的效果由前端 `main.ts` 读到那条
  // `tool_use` 后**广播** `ai-action` 到这里。名字词表（热键键串 / 预设名 / 参数 id / 表情名）
  // 由前端在**桌宠轮**里注入给模型，这里只负责把名字解析回动作、落到模型上。
  // 解析不到就静默（名字来自 AI，可能已过时）；桌宠没显示模型时也静默（不去打扰用户）。
  async function runAiAction(action: string, name: string, value?: number): Promise<void> {
    const h = l2dHandle;
    const modelPath = cfg.model.trim();
    const want = name.trim();
    if (!h || !modelPath || !want) return;
    if (action === "auto") {
      // 「情绪标签」回退（`main.ts` 的 `consumePetTags`）：正文里的 `[名字]` 只带名字、不带
      // 类型 ⇒ 按「热键键串 → 动作文件名 → 表情名」依次尝试，命中即止（都命中不了就静默）。
      const list = await loadHotkeys(modelPath);
      const hk = list.find(x => x.keys.trim().toLowerCase() === want.toLowerCase());
      if (hk) { await toggleHotkey(hk.id, hk.action); return; }
      const files = await listMotionFiles(modelPath);
      const i = files.findIndex(f => f.name.trim().toLowerCase() === want.toLowerCase());
      if (i >= 0) { h.playMotion(MOTION_EXTRA_GROUP, i); return; }
      // 动作链（2026-10-10）：链名也进词表 ⇒ `[链名]` 标签按名字命中即整条播。
      const chs = await loadChains(modelPath);
      const ch = chs.find(c => c.name.trim().toLowerCase() === want.toLowerCase());
      if (ch) { await playChainById(ch.id); return; }
      h.setExpression(want); // 认不出的一律是空操作（库 `expression()` 查不到就返回 null）
      return;
    }
    if (action === "chain") {
      // 显式按链名 / id 触发。
      const chs = await loadChains(modelPath);
      const ch = chs.find(c => c.id === want) || chs.find(c => c.name.trim().toLowerCase() === want.toLowerCase());
      if (ch) await playChainById(ch.id);
      return;
    }
    if (action === "hotkey") {
      const list = await loadHotkeys(modelPath);
      // 优先按**键串**（AI 侧标识）匹配，退而按热键名匹配（容错：AI 可能用了显示名）。
      const hk =
        list.find(x => x.keys.trim().toLowerCase() === want.toLowerCase()) ||
        list.find(x => x.name.trim() === want);
      if (hk) await toggleHotkey(hk.id, hk.action);
      return;
    }
    if (action === "motion") {
      // **行 = 文件**：按动作文件名（词干）匹配；序号 = 它在扫描列表里的下标（= 注入组序号）。
      const files = await listMotionFiles(modelPath);
      const i = files.findIndex(f => f.name.trim().toLowerCase() === want.toLowerCase());
      if (i >= 0) h.playMotion(MOTION_EXTRA_GROUP, i);
      return;
    }
    if (action === "param") {
      h.setParameter(want, typeof value === "number" ? value : 0);
      return;
    }
    if (action === "expression") {
      if (value === 0) h.resetExpressions();
      else h.setExpression(want);
      return;
    }
    if (action === "artmeshColor") {
      // `value` = **0xRRGGBB**（打包成一个数字，`PetControl` 的 `value` 只有 number 一种形态）；
      // 越界 / 缺省一律当白 = 原色（等价「取消染色」）。
      const n = typeof value === "number" && Number.isFinite(value) ? Math.floor(value) : 0xffffff;
      const c = Math.min(0xffffff, Math.max(0, n));
      h.setDrawableColor(want, {
        r: ((c >> 16) & 255) / 255,
        g: ((c >> 8) & 255) / 255,
        b: (c & 255) / 255,
      });
    }
  }

  // ── 与控制台 / 宿主的通道 ─────────────────────────────────────
  const offCtl = await listen(CTL_EVENT, ev => {
    const p = ev.payload as
      | { type?: string; value?: unknown; token?: string; id?: string; name?: unknown; ids?: unknown; enabled?: unknown; intervalMin?: unknown; dailyCap?: unknown; autoShot?: unknown; autoShotMin?: unknown; screenWatch?: unknown; screenWatchSec?: unknown; screenDiff?: unknown; action?: unknown; group?: unknown; index?: unknown }
      | null;
    if (!p?.type) return;
    // 控制台问「桌宠窗开着吗」—— **只在桌宠窗里应答**（设置窗也是本插件的一份，不该应答）。
    // `visible` 一并带回：**隐藏 ≠ 关闭**（隐藏的窗照样应答），设置窗靠它区分「显示桌宠 / 收起来」。
    if (p.type === "ping") {
      if (inPetWindow()) {
        void emit(CTL_EVENT, { type: "pong", token: p.token, visible });
        broadcastHotkeyStates(); // 顺带把「现在开着哪些热键」报给设置窗（对齐按钮状态）
      }
      return;
    }
    if (p.type === "reload") {
      void paint();
      return;
    }
    // 设置窗的「隐藏模型」：**只隐藏本窗、不销毁**（用户 2026-10-10 口径 —— 原来那枚按钮会把
    // 整个桌宠窗关掉）。本窗自己 `hide()`，宿主不会补发 `plugin-window-visibility` ⇒ 这里
    // 主动走一遍 `applyHidden()` 把动画 / 定时器停掉；再显示走 `open_plugin_window` 的
    // 复用路径（宿主 `show()` + `announce_visibility_now` ⇒ 本窗收到 visible=true 恢复）。
    if (p.type === "hide" && inPetWindow()) {
      applyHidden();
      void getCurrentWindow().hide().catch(e => reportPet("隐藏模型失败", e));
      return;
    }
    // 设置窗的「显示桌宠」（本窗**只是被隐藏**、没被销毁）：自己 `show()` 回来 —— **刻意不走
    // `open_plugin_window`**：宿主复用路径会推一条 `plugin-window-input` ⇒ 本窗整页重挂
    // （重载模型，热键开关等运行时状态全丢）。见 `applyHidden` / `applyShown` 的分工。
    if (p.type === "show" && inPetWindow()) {
      void (async () => {
        try {
          const w = getCurrentWindow();
          await w.unminimize().catch(() => {});
          await w.show();
        } catch (e) {
          reportPet("显示模型失败", e);
        }
        applyShown();
      })();
      return;
    }
    if (p.type === "set-scale" && typeof p.value === "number") {
      saveConfig({ ...loadConfig(), scale: p.value });
      applyScale(p.value);
      return;
    }
    if (p.type === "set-rotation" && typeof p.value === "number") {
      saveConfig({ ...loadConfig(), rotation: p.value });
      applyRotation(p.value);
      return;
    }
    if (p.type === "set-click-through" && typeof p.value === "boolean") {
      // 记住**意图态**（关菜单时按它恢复），并刷新右键菜单那条按钮的文案（两处对齐）。
      cfg = { ...cfg, clickThrough: p.value };
      void applyClickThrough(p.value);
      updateMenuThrough();
    }
    // 主动反应配置改动（设置窗广播）：落盘 + **立即重排**（开关一开就生效）。两个子开关同理。
    if (p.type === "set-proactive") {
      const c = loadConfig();
      const pd = DEFAULT_CFG.proactive;
      const clampNum = (v: unknown, min: number, max: number, fallback: number) =>
        typeof v === "number" && Number.isFinite(v) ? Math.min(max, Math.max(min, Math.round(v))) : fallback;
      saveConfig({
        ...c,
        proactive: {
          ...c.proactive,
          enabled: p.enabled === true,
          intervalMin: clampNum(p.intervalMin, PROACTIVE_INTERVAL_MIN, PROACTIVE_INTERVAL_MAX, c.proactive.intervalMin),
          dailyCap: clampNum(p.dailyCap, PROACTIVE_CAP_MIN, PROACTIVE_CAP_MAX, c.proactive.dailyCap),
          // 子开关：**只有显式 true 才算开**（同 loadConfig 口径，默认关是隐私红线）。
          autoShot: p.autoShot === true,
          autoShotMin: clampNum(p.autoShotMin, AUTOSHOT_MIN_MIN, AUTOSHOT_MIN_MAX, pd.autoShotMin),
          screenWatch: p.screenWatch === true,
          screenWatchSec: clampNum(p.screenWatchSec, WATCH_SEC_MIN, WATCH_SEC_MAX, pd.screenWatchSec),
          screenDiff: clampNum(p.screenDiff, WATCH_DIFF_MIN, WATCH_DIFF_MAX, pd.screenDiff),
        },
      });
      scheduleProactive(true);
      if (p.screenWatch !== true || p.screenWatchSec !== c.proactive.screenWatchSec) lastSig = null;
      scheduleAutoShot();
      scheduleScreenWatch();
      return;
    }
    // 模型调试：**仅 Live2D 模型模式**处理（图片模式下 `l2dHandle` 恒为 null ⇒ 自然忽略）。
    if (p.type === "set-param" && typeof p.id === "string" && typeof p.value === "number") {
      l2dHandle?.setParameter(p.id, p.value);
      return;
    }
    // 「恢复默认」的批量重置：把这些参数一律设回 0（与设置窗 `restoreParams` 同口径）。
    if (p.type === "reset-params" && Array.isArray(p.ids)) {
      const h = l2dHandle;
      if (h) for (const id of p.ids) if (typeof id === "string") h.setParameter(id, 0);
      return;
    }
    if (p.type === "set-expression" && typeof p.id === "string") {
      if (!l2dHandle) return;
      if (p.id) l2dHandle.setExpression(p.id);
      else l2dHandle.resetExpressions(); // 空串 = 还原
      return;
    }
    // 热键动作（设置窗「测试」按钮 / 后续 AI 工具广播过来）：在**桌宠窗**里落到模型上。
    if (p.type === "hotkey-run" && p.action && typeof p.action === "object") {
      runHotkey(p.action as HotkeyAction);
      return;
    }
    // 热键**开关**（设置窗的状态按钮）：表情走叠加层、可单独关；状态回广播。
    if (p.type === "hotkey-toggle" && typeof p.id === "string" && p.action && typeof p.action === "object") {
      void toggleHotkey(p.id, p.action as HotkeyAction);
      return;
    }
    // 预设动作的**预览播放**（设置窗「预设动作」页的播放按钮）。
    if (p.type === "motion-play" && typeof p.group === "string") {
      const h = l2dHandle;
      if (!h) {
        showToast("桌宠还没显示 Live2D 模型", 2600);
        return;
      }
      h.playMotion(p.group, typeof p.index === "number" ? p.index : 0);
      return;
    }
    // 动作链的**播放**（设置窗动作页的「播放」按钮）：按链 id 在桌宠窗解析并播放。
    if (p.type === "chain-play" && typeof p.id === "string" && inPetWindow()) {
      void playChainById(p.id);
      return;
    }
    // AI 工具（`PetControl`）：前端读到该 `tool_use` 后广播过来（见 runAiAction）。
    if (p.type === "ai-action" && typeof p.action === "string" && typeof p.name === "string") {
      void runAiAction(p.action, p.name, typeof p.value === "number" ? p.value : undefined);
    }
  });

  const offVis = await listen<{ label?: string; visible?: boolean }>(
    "plugin-window-visibility",
    ev => {
      // 事件是**广播**的（同时开音乐窗时那条也会来），只认自己这一个 label
      if (ev.payload?.label && ev.payload.label !== PET_LABEL) return;
      if (ev.payload?.visible === false) {
        applyHidden();
      } else {
        applyShown();
      }
    },
  );

  injectPetStyle();
  // 窗口尺寸（2026-10-13）：
  //  · **可调宽高** —— 无边框窗也能拖边缘 / 角缩放，前提是宿主侧 `resizable` 为真（清单 `window`
  //    段的 resizable，见 build-plugins.ps1 的 pet 块）；dev 里若这扇窗是旧清单建的，这里再确保一次。
  //  · **记忆尺寸** —— 用本插件 localStorage 里的 winW / winH（宿主**不**按插件 id 存窗口尺寸），
  //    先 clamp 到合法区间再对齐（清单只在建窗那一刻生效，记忆 + clamp 才是权威）。
  //    ⚠️ clamp 的**下限由 pet-window-size 推导**（`WIN_H_MIN = 75 + 35 + STAGE_MIN_H`，2026-10-13
  //    第三轮把它抬到 250）—— 老用户存的过小尺寸（如 200）在这里被纠正，模型区不会再被压到 0。
  void invoke("plugin_window_set_resizable", { resizable: true }).catch(() => {});
  {
    const init = clampWindowSize(cfg.winW, cfg.winH);
    winW = init.w;
    winBaseH = init.h;
  }
  applyOutHeight(true); // 无内容 ⇒ 输出带回到下限 75、窗口 = 基准高（= 记忆值 / 缺省 510）

  // 用户拖边缘 / 角 / 手柄改尺寸后**记住**（去抖 250ms；拖拽期间 resize 事件很密）。
  // 程序化下发满足 `窗高 = 基准高 + (输出带高 − 75)`，故反推基准高对程序化改动恒等、不会漂移。
  // **最小模型区保护**（2026-10-13 第三轮）：手拖把窗变矮时**先收窄盒高**再反推基准高。
  const offResized = await getCurrentWindow().onResized(ev => {
    if (!visible) return;
    const size = ev.payload; // PhysicalSize（物理像素）
    const dpr = window.devicePixelRatio || 1;
    const winH = size.height / dpr;
    // 窗矮 ⇒ 上限（outCap）随之降低 ⇒ 把盒高收到新上限（**只收不放**：内容变短不在这里放大，
    // 那由 applyOutHeight 负责）。否则盒仍停在旧高、`.pet-stage` 会被压到 0（长回复 + 手拖到
    // 最小窗的病态，见 outCap / STAGE_MIN_H）。程序化下发恒满足 `盒高 ≤ outCap(窗高)`，故不触发。
    const cap = outCap(winH);
    if (outTopH > cap) {
      outTopH = cap;
      setOutTopVar(cap);
    }
    const topH = outTopH; // 反推基准高要用**收窄后**的盒高（否则记账会漂），别用 250ms 后的新值
    if (resizeSaveTimer) clearTimeout(resizeSaveTimer);
    resizeSaveTimer = setTimeout(() => {
      resizeSaveTimer = null;
      const next = clampWindowSize(size.width / dpr, baseHeightFromWindow(winH, topH));
      if (next.w === winW && next.h === winBaseH) return; // 与已记的一致 ⇒ 不写盘（收敛）
      winW = next.w;
      winBaseH = next.h;
      cfg = { ...cfg, winW: next.w, winH: next.h };
      saveConfig(cfg);
    }, 250);
  });
  void paint();
  // 配置里就写着要穿透（上次在控制台开的）⇒ 开窗即生效。
  // **动画不在这里起**：`paint()` 会按当前来源决定 —— 模型自己走 pixi 的 ticker，
  // 只有图片才需要那条 rAF（在这里无条件 startLoop 会在模型模式下把 rAF 又拉起来）。
  if (cfg.clickThrough) await applyClickThrough(true);
  // 开窗时把当前穿透状态广播一次：若设置窗正开着，让它那条按钮与这里对齐（两处状态一致）。
  void emit(CTL_EVENT, { type: "click-through-changed", value: cfg.clickThrough });
  scheduleProactive(true); // 开窗按配置排一次（默认关 ⇒ 空操作；关着时不排）
  scheduleAutoShot(); // 两个抓屏子开关：开窗也按配置排一次（默认关 ⇒ 空操作）
  scheduleScreenWatch();

  // 穿透态下，宿主全局钩子命中 Alt+右键 后**定向**发到本窗（label `plugin-pet`）的事件。
  // 收到即唤出菜单；唤出时的「临时关穿透 → 关菜单恢复」在 `openMenuFromScreen`/`closeMenu` 里。
  const offAltRclick = await listen<{ x?: number; y?: number }>("pet-alt-rclick", ev => {
    if (!inPetWindow()) return; // 定向事件只发本窗；这里再自查一道，避免将来复用时被误处理
    const x = Number(ev.payload?.x);
    const y = Number(ev.payload?.y);
    if (!Number.isFinite(x) || !Number.isFinite(y)) return;
    void openMenuFromScreen(x, y).catch(e => reportPet("Alt+右键唤出菜单失败", e));
  });

  petCleanup = () => {
    stopLoop();
    dropModel();
    if (toastTimer) clearTimeout(toastTimer);
    // 主动反应的三个计时器一并清（别泄漏）
    clearProactiveTimer();
    clearAutoShotTimer();
    clearScreenWatchTimer();
    if (proactiveWatchdog) clearTimeout(proactiveWatchdog);
    if (outputBoxTimer) clearTimeout(outputBoxTimer);
    if (ttsWatchdog) clearTimeout(ttsWatchdog); // 语音等待的兜底看门狗
    if (resizeSaveTimer) clearTimeout(resizeSaveTimer); // 拖拽记账去抖
    if (mouthTimer) {
      clearInterval(mouthTimer); // 口型驱动：别在窗关掉后还在跑
      mouthTimer = null;
    }
    offCtl();
    offVis();
    offAltRclick();
    offChatReply();
    offTtsDone();
    offMouth();
    offResized();
    l2dHandle?.setMouthOpen(null);
    chatInput?.removeEventListener("keydown", onChatKeydown);
    chatClose?.removeEventListener("click", onChatCloseClick);
    window.removeEventListener("mousemove", onShapeDragMove);
    window.removeEventListener("mouseup", onShapeDragUp);
    // 关窗清理：穿透态那条全局鼠标钩子必须撤掉（宿主 Destroyed 分支也会兜一道，这里主动一次）。
    void invoke("pet_alt_rclick_watch", { on: false }).catch(() => {});
  };
}

// ══════════════════════════════════════════════════════════════════
//  主窗（搜索）那一格：**没有页面了**（2026-10-09 用户要求）
// ══════════════════════════════════════════════════════════════════
//
// 入口改由 `main.ts` 的 `executePlugin` **特判**（与 AI 助手 / 音乐同一形态）：搜索「桌宠」
// 直接开**独立设置窗**，主窗**不渲染任何页面**。原来的 `launcherHtml` / `attachLauncher`
//（那句「已在独立窗口打开」+ 再打开按钮）**已整体删除** —— 它本身就是一页内嵌界面。

// ══════════════════════════════════════════════════════════════════

const petPlugin: Plugin = {
  id: "pet",
  name: "桌宠",
  keywords: ["桌宠", "宠物", "桌面宠物", "pet", "desktop pet", "live2d", "看板娘", "吉祥物"],
  description: "桌宠 —— 独立透明置顶的桌面形象窗（图片或 Live2D 模型，由你自己导入）",
  icon: "🐾",
  // **刻意不声明 `window.float`**：它的入口是**带 key 的附属窗**（`plugin-pet-settings`），
  // 不是自己的形象窗 —— `main.ts` 的 `executePlugin` 按 id 特判直接开那扇窗（2026-10-09）。
  permissions: [],

  async execute(_input: string): Promise<PluginResult> {
    // 三扇窗，一份代码：桌宠窗 / 设置窗 / 主窗（搜索）。
    if (inPetWindow()) return { type: "html", content: petHtml() };
    if (inSettingsWindow()) return { type: "html", content: settingsHtml(loadConfig()) };
    // 主窗（搜索「桌宠」）：**入口已被 `main.ts` 的 `executePlugin` 特判**成独立设置窗，
    // 不会走到这里、主窗也不渲染页面（2026-10-09 用户要求，与 AI 助手 / 音乐同形态）。
    // 留空内容只作兜底（理论上到不了）。
    return { type: "html", content: "" };
  },

  async attach(root: HTMLElement) {
    if (inPetWindow()) {
      await attachPetWindow(root);
      return;
    }
    if (inSettingsWindow()) {
      await attachSettings(root);
      return;
    }
    // 主窗：没有页面可挂载（入口已被 executePlugin 特判成独立设置窗）。
  },

  detach() {
    petCleanup?.();
    petCleanup = null;
  },
};

export default petPlugin;
