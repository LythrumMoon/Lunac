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
import { ensureCubismCore, live2dEnginePath, validateModelFile, type Live2DHandle } from "./live2d";

/** 控制台 → 桌宠窗的单向指令（宿主不给这个，是插件自己约的一条广播）。
 *  tauri 的 `emit` 是**广播**，所以桌宠窗只认自己关心的几种 `type`，其余原样忽略。 */
const CTL_EVENT = "pet-control";

/** 配置落点。用 localStorage 而不是宿主命令：宿主**没有**通用的文本读写命令
 *  （只有 `read_tool_file` / `read_skill_file` 那几条面向工具与技能的），
 *  而两个窗口同源（`tauri://localhost` 或 dev 的 `5173`）⇒ 共享同一份 localStorage。
 *  **只用自己的 key 前缀**，不去碰宿主与别的插件的东西。 */
const CFG_KEY = "lunac.pet.v1";

/** 桌宠窗自己那一个窗口的 label（宿主按 `plugin-<id>` 命名）。 */
const PET_LABEL = "plugin-pet";

interface PetConfig {
  /** 形象图片的**绝对路径**（用户经文件对话框选的）。空 = 没有图片形象。 */
  image: string;
  /** Live2D 模型的 `*.model3.json` **绝对路径**（用户经文件对话框选的）。**非空时优先于图片**。 */
  model: string;
  /** 形象占窗口宽度的比例。窗口是定尺的，所以「大小」调的是形象本身。 */
  scale: number;
  /** 鼠标穿透。**默认关**：开着时桌宠窗不吃任何鼠标事件，拖动与右键菜单都会失效。 */
  clickThrough: boolean;
}

const DEFAULT_CFG: PetConfig = { image: "", model: "", scale: 1, clickThrough: false };

function loadConfig(): PetConfig {
  try {
    const raw = localStorage.getItem(CFG_KEY);
    if (!raw) return { ...DEFAULT_CFG };
    const o = JSON.parse(raw) as Partial<PetConfig>;
    return {
      image: typeof o.image === "string" ? o.image : "",
      model: typeof o.model === "string" ? o.model : "",
      scale: typeof o.scale === "number" && o.scale >= 0.4 && o.scale <= 2 ? o.scale : 1,
      clickThrough: o.clickThrough === true,
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

function esc(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

/** 当前页是不是桌宠窗（见文件头的「两个窗口」一段）。 */
function inPetWindow(): boolean {
  return location.pathname.endsWith("plugin.html");
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

function consoleHtml(cfg: PetConfig): string {
  const hasModel = cfg.model.trim().length > 0;
  const hasImage = cfg.image.trim().length > 0;
  return `
<div class="pet-console">
  <div class="pet-row pet-row-main">
    <button id="pet-show" class="pet-btn pet-btn-primary">${hasModel || hasImage ? "显示桌宠" : "导入形象并显示"}</button>
    <button id="pet-hide" class="pet-btn">收起来</button>
  </div>
  <div class="pet-row">
    <button id="pet-pick-model" class="pet-btn">选择 Live2D 模型…</button>
    <button id="pet-clear-model" class="pet-btn" hidden>不用模型</button>
  </div>
  <div class="pet-row">
    <button id="pet-pick" class="pet-btn">选择图片…</button>
    <label class="pet-check">
      <input type="checkbox" id="pet-through" ${cfg.clickThrough ? "checked" : ""}>
      鼠标穿透（桌宠不吃鼠标点击，桌面照常操作）
    </label>
  </div>
  <div class="pet-row">
    <span class="pet-label">形象大小</span>
    <input type="range" id="pet-scale" min="40" max="200" step="5" value="${Math.round(cfg.scale * 100)}">
    <span class="pet-val" id="pet-scale-val">${Math.round(cfg.scale * 100)}%</span>
  </div>
  <div class="pet-file" id="pet-file"></div>
  <div class="pet-note">
    形象由你自己准备 —— Lunac 只带引擎，不随包分发任何模型或素材（桌宠是独立的悬浮窗，
    与搜索窗同时存在）。<b>Live2D 要选那个 <code>*.model3.json</code></b>：同目录的纹理、
    动作、表情会自动带上，不需要你逐个挑。<br>
    <b>穿透开着时桌宠窗收不到鼠标事件</b>，所以关掉它的开关只放在这里 ——
    桌宠窗自己那份右键菜单在穿透状态下是收不到点击的。
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

async function attachConsole(root: HTMLElement): Promise<void> {
  const el = <T extends HTMLElement>(id: string) => root.querySelector<T>(`#${id}`);

  /** 把「当前用的是哪个形象」与几个按钮的文案一并刷成最新的。
   *  `extra` 只在刚验过一个 Live2D 模型时给（那句「纹理 N · 动作组 …」）。 */
  const renderFile = (extra = "") => {
    const c = loadConfig();
    const model = c.model.trim();
    const image = c.image.trim();

    const pickModel = el("pet-pick-model");
    const pickImage = el("pet-pick");
    const clearModel = el("pet-clear-model");
    const show = el("pet-show");
    if (pickModel) pickModel.textContent = model ? "更换 Live2D 模型…" : "选择 Live2D 模型…";
    if (pickImage) pickImage.textContent = image ? "更换图片…" : "选择图片…";
    if (clearModel) clearModel.hidden = !model;
    if (show) show.textContent = model || image ? "显示桌宠" : "导入形象并显示";

    const box = el("pet-file");
    if (!box) return;
    const tip = extra ? `<br><span class="pet-dim">${esc(extra)}</span>` : "";
    if (model) box.innerHTML = `当前用 Live2D 模型：<code>${esc(model)}</code>${tip}`;
    else if (image) box.innerHTML = `当前用图片：<code>${esc(image)}</code>${tip}`;
    else box.innerHTML = `还没有形象 —— 选一张图片（png / jpg / webp / gif），或选一个 Live2D 模型（<code>*.model3.json</code>）。`;
  };

  async function showPet() {
    await invoke("open_plugin_window", { pluginId: "pet", key: null, input: "" });
  }

  el("pet-show")?.addEventListener("click", () => {
    void (async () => {
      const c = loadConfig();
      if (!c.model.trim() && !c.image.trim()) {
        const picked = await pickImage();
        if (!picked) return;
      }
      await showPet();
    })().catch(e => console.error("[lunac pet] 显示桌宠失败：", e));
  });

  el("pet-hide")?.addEventListener("click", () => {
    void invoke("close_plugin_window", { pluginId: "pet" }).catch(() => {});
  });

  el("pet-pick")?.addEventListener("click", () => {
    void pickImage();
  });

  el("pet-pick-model")?.addEventListener("click", () => {
    void pickModel();
  });

  el("pet-clear-model")?.addEventListener("click", () => {
    saveConfig({ ...loadConfig(), model: "" });
    renderFile();
    void emit(CTL_EVENT, { type: "reload" });
  });

  el("pet-through")?.addEventListener("change", ev => {
    const on = (ev.target as HTMLInputElement).checked;
    const c = loadConfig();
    saveConfig({ ...c, clickThrough: on });
    void emit(CTL_EVENT, { type: "set-click-through", value: on });
  });

  const scale = el<HTMLInputElement>("pet-scale");
  scale?.addEventListener("input", () => {
    const v = Math.round(Number(scale.value)) / 100;
    const out = el("pet-scale-val");
    if (out) out.textContent = `${Math.round(v * 100)}%`;
    saveConfig({ ...loadConfig(), scale: v });
    void emit(CTL_EVENT, { type: "set-scale", value: v });
  });

  /** 选图片 → 存配置 → 通知已开着的桌宠窗换形象。返回选中的路径（取消则空串）。 */
  async function pickImage(): Promise<string> {
    const picked = await openFileDialog({
      multiple: false,
      directory: false,
      filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg", "webp", "gif", "bmp"] }],
    });
    if (typeof picked !== "string" || !picked) return "";
    // 两种来源互斥：选了图片就是「从此用图片」（不互斥的话模型永远压着图片，
    // 用户会看到「选了图片但什么都没变」）。
    saveConfig({ ...loadConfig(), image: picked, model: "" });
    renderFile();
    void emit(CTL_EVENT, { type: "reload" });
    return picked;
  }

  /** 选 Live2D 模型 → **当场验一遍** → 存配置 → 广播。
   *  验在这里是刻意的：桌宠窗小，而且用户很可能刚把它收起来 —— 报错必须落在看得见的地方。 */
  async function pickModel(): Promise<void> {
    const picked = await openFileDialog({
      multiple: false,
      directory: false,
      filters: [{ name: "Live2D 模型定义", extensions: ["json"] }],
    });
    if (typeof picked !== "string" || !picked) return;
    const box = el("pet-file");
    if (box) box.innerHTML = "正在检查模型…";
    const v = await validateModelFile(picked);
    if (!v.ok) {
      if (box) box.innerHTML = `<span class="pet-bad">这个文件用不了：${esc(v.reason)}</span>`;
      return;
    }
    // 与 pickImage 对称：选了模型就是「从此用模型」
    saveConfig({ ...loadConfig(), model: picked, image: "" });
    renderFile(v.info);
    void emit(CTL_EVENT, { type: "reload" });
  }

  injectPetStyle();
  renderFile();
}

// ══════════════════════════════════════════════════════════════════
//  桌宠窗
// ══════════════════════════════════════════════════════════════════

function petHtml(): string {
  return `
<div class="pet-root" id="pet-root">
  <div class="pet-stage" id="pet-stage">
    <div class="pet-l2d" id="pet-l2d"></div>
    <img class="pet-img" id="pet-img" alt="" draggable="false">
    <div class="pet-empty" id="pet-empty">还没有形象<br><span>在控制台里选一张图片或一个 Live2D 模型</span></div>
  </div>
  <div class="pet-err" id="pet-err"></div>
  <div class="pet-menu" id="pet-menu">
    <button data-act="pick">更换图片…</button>
    <button data-act="through">开启鼠标穿透</button>
    <button data-act="bigger">大一点</button>
    <button data-act="smaller">小一点</button>
    <button data-act="close" class="danger">关闭桌宠</button>
  </div>
</div>`;
}

/** 桌宠窗的样式。**全部带 `pet-` 前缀**，宿主那套主题变量（`--text` / `--border-glass`
 *  …）照用，这样换主题时两个窗口观感一致（规范见 Modules 的插件开发规范 §4.3）。 */
const PET_CSS = `
.pet-root{position:fixed;inset:0;display:flex;align-items:flex-end;justify-content:center;overflow:hidden;}
.pet-stage{position:relative;width:100%;height:100%;display:flex;align-items:flex-end;justify-content:center;cursor:grab;touch-action:none;}
.pet-stage:active{cursor:grabbing;}
.pet-img{width:100%;height:100%;object-fit:contain;object-position:bottom center;transform-origin:50% 100%;pointer-events:none;-webkit-user-drag:none;filter:drop-shadow(0 6px 14px rgba(0,0,0,.35));}
.pet-l2d{position:absolute;inset:0;display:none;}
.pet-l2d.on{display:block;}
.pet-l2d canvas{display:block;}
.pet-err{position:absolute;left:8px;right:8px;bottom:8px;display:none;font-size:.68rem;line-height:1.6;color:var(--red);background:var(--surface-glass);border:1px solid var(--border-glass);border-radius:var(--radius);padding:8px 10px;word-break:break-all;backdrop-filter:blur(14px);-webkit-backdrop-filter:blur(14px);}
.pet-err.on{display:block;}
.pet-empty{position:absolute;left:50%;bottom:38%;transform:translateX(-50%);text-align:center;font-size:.72rem;line-height:1.7;color:var(--text-dim);background:var(--surface-glass);border:1px solid var(--border-glass);border-radius:var(--radius);padding:10px 14px;backdrop-filter:blur(14px);-webkit-backdrop-filter:blur(14px);}
.pet-empty span{font-size:.66rem;opacity:.75;}
.pet-menu{position:absolute;display:none;flex-direction:column;min-width:132px;padding:4px;background:var(--surface-glass);border:1px solid var(--border-glass);border-radius:var(--radius);backdrop-filter:blur(20px);-webkit-backdrop-filter:blur(20px);box-shadow:0 8px 24px rgba(0,0,0,.28);}
.pet-menu.open{display:flex;}
.pet-menu button{appearance:none;background:none;border:0;text-align:left;font:inherit;font-size:.72rem;color:var(--text);padding:6px 10px;border-radius:6px;cursor:pointer;white-space:nowrap;}
.pet-menu button:hover{background:var(--border-glass);}
.pet-menu button.danger:hover{color:var(--red);}
.pet-console{padding:10px 12px 14px;font-size:.76rem;color:var(--text);}
.pet-row{display:flex;align-items:center;gap:8px;margin-bottom:10px;flex-wrap:wrap;}
.pet-row-main{gap:10px;}
.pet-btn{appearance:none;font:inherit;font-size:.74rem;color:var(--text);background:var(--surface-glass);border:1px solid var(--border-glass);border-radius:8px;padding:6px 12px;cursor:pointer;}
.pet-btn:hover{border-color:var(--accent);color:var(--accent);}
.pet-btn-primary{background:var(--accent);border-color:var(--accent);color:#fff;font-weight:600;}
.pet-btn-primary:hover{color:#fff;opacity:.92;}
.pet-check{display:flex;align-items:center;gap:6px;color:var(--text-dim);cursor:pointer;}
.pet-label{color:var(--text-dim);}
.pet-val{color:var(--text-dim);min-width:3.2em;}
.pet-file{margin:2px 0 10px;color:var(--text-dim);word-break:break-all;line-height:1.6;}
.pet-file code{font-size:.7rem;color:var(--text);}
.pet-dim{opacity:.72;}
.pet-bad{color:var(--red);}
.pet-note{color:var(--text-dim);opacity:.72;font-size:.68rem;line-height:1.7;border-top:1px solid var(--border-glass);padding-top:9px;}
.pet-note b{color:var(--text);opacity:.9;}
`;

let petCleanup: (() => void) | null = null;

async function attachPetWindow(root: HTMLElement): Promise<void> {
  const el = <T extends HTMLElement>(id: string) => root.querySelector<T>(`#${id}`);
  const stage = el("pet-stage");
  const img = el<HTMLImageElement>("pet-img");
  const l2d = el("pet-l2d");
  const errBox = el("pet-err");
  const empty = el("pet-empty");
  const menu = el("pet-menu");

  let raf = 0;
  let cfg = loadConfig();
  /** Live2D 那一份实例。图片模式下恒为 null。 */
  let l2dHandle: Live2DHandle | null = null;
  /** 连发几次重画时只认最后一次 —— 装载是异步的，慢的那次不能覆盖快的。 */
  let paintGen = 0;
  /** 窗口是否可见。**最小化 / 隐藏时动画必须停**（WebView2 对最小化窗仍报 `visible`）。 */
  let visible = true;

  function showErr(msg: string) {
    if (!errBox) return;
    errBox.textContent = msg;
    errBox.classList.toggle("on", !!msg);
  }

  /** 卸掉 Live2D 实例（画布与纹理一起收）。**不碰 `on` 类** —— 那个类由 `paint()` 决定，
   *  在这里顺手摘掉会把「上一行刚加上去的那次」一起摘掉（实测：画布停在 1×1）。 */
  function dropModel() {
    l2dHandle?.destroy();
    l2dHandle = null;
  }

  // ── 渲染形象 ──────────────────────────────────────────────────
  // 两种来源**互斥**（存的时候就互斥了，见控制台的 pickImage / pickModel）：模型优先。
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
      try {
        const { mountLive2D } = await loadEngine();
        const h = await mountLive2D(l2d, { modelPath, scale: cfg.scale });
        if (gen !== paintGen) {
          h.destroy(); // 期间又换了一次形象 ⇒ 这一份作废
          return;
        }
        l2dHandle = h;
        h.setScale(loadConfig().scale); // 装载期间用户可能又拖了大小滑块
        h.setRunning(visible);
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
    if (img) img.style.transform = `scale(${cfg.scale}) translateY(0)`;
    if (visible) startLoop();
  }

  /** 只改大小、**不重建**：Live2D 用实例自己的 `setScale`，图片用 transform。 */
  function applyScale(v: number) {
    cfg = { ...cfg, scale: v };
    if (l2dHandle) {
      l2dHandle.setScale(v);
      return;
    }
    if (img) img.style.transform = `scale(${v}) translateY(0)`;
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
      img.style.transform = `scale(${cfg.scale}) translateY(${y.toFixed(2)}px)`;
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
  async function applyClickThrough(on: boolean) {
    try {
      await invoke("plugin_window_set_click_through", { ignore: on });
    } catch (e) {
      console.error("[lunac pet] 设置穿透失败：", e);
    }
  }

  // ── 右键菜单 ──────────────────────────────────────────────────
  function closeMenu() {
    menu?.classList.remove("open");
  }
  function openMenu(x: number, y: number) {
    if (!menu) return;
    menu.style.left = `${Math.max(2, Math.min(x, window.innerWidth - 140))}px`;
    menu.style.top = `${Math.max(2, Math.min(y, window.innerHeight - 150))}px`;
    menu.classList.add("open");
  }

  stage?.addEventListener("contextmenu", ev => {
    ev.preventDefault();
    openMenu(ev.clientX, ev.clientY);
  });
  // 左键按住空白处 = 拖窗（无边框窗没有系统标题栏，只能自己发起）。
  stage?.addEventListener("mousedown", ev => {
    if (ev.button !== 0) return;
    closeMenu();
    void getCurrentWindow()
      .startDragging()
      .catch(() => {});
  });
  root.addEventListener("mousedown", ev => {
    if (menu?.classList.contains("open") && !menu.contains(ev.target as Node)) closeMenu();
  });

  menu?.addEventListener("click", ev => {
    const act = (ev.target as HTMLElement).closest<HTMLElement>("button")?.dataset.act;
    if (!act) return;
    closeMenu();
    void (async () => {
      if (act === "pick") {
        const picked = await openFileDialog({
          multiple: false,
          directory: false,
          filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg", "webp", "gif", "bmp"] }],
        });
        if (typeof picked === "string" && picked) {
          // 选了图片 = 从此用图片（两种来源互斥，见 paint）
          saveConfig({ ...loadConfig(), image: picked, model: "" });
          void paint();
        }
        return;
      }
      if (act === "through") {
        saveConfig({ ...loadConfig(), clickThrough: true });
        await applyClickThrough(true);
        return;
      }
      if (act === "bigger" || act === "smaller") {
        const cur = loadConfig().scale;
        const next = Math.min(2, Math.max(0.4, +(cur + (act === "bigger" ? 0.1 : -0.1)).toFixed(2)));
        saveConfig({ ...loadConfig(), scale: next });
        applyScale(next);
        return;
      }
      if (act === "close") {
        await invoke("plugin_window_close").catch(() => {});
      }
    })().catch(e => console.error("[lunac pet]", e));
  });

  // ── 与控制台 / 宿主的通道 ─────────────────────────────────────
  const offCtl = await listen(CTL_EVENT, ev => {
    const p = ev.payload as { type?: string; value?: unknown } | null;
    if (!p?.type) return;
    if (p.type === "reload") {
      void paint();
      return;
    }
    if (p.type === "set-scale" && typeof p.value === "number") {
      saveConfig({ ...loadConfig(), scale: p.value });
      applyScale(p.value);
      return;
    }
    if (p.type === "set-click-through" && typeof p.value === "boolean") {
      void applyClickThrough(p.value);
    }
  });

  const offVis = await listen<{ label?: string; visible?: boolean }>(
    "plugin-window-visibility",
    ev => {
      // 事件是**广播**的（同时开音乐窗时那条也会来），只认自己这一个 label
      if (ev.payload?.label && ev.payload.label !== PET_LABEL) return;
      if (ev.payload?.visible === false) {
        visible = false;
        stopLoop();
        l2dHandle?.setRunning(false); // Live2D 自己那份 ticker 也要停，见 ai-spec §4.8
        closeMenu();
      } else {
        visible = true;
        if (l2dHandle) l2dHandle.setRunning(true);
        else startLoop();
      }
    },
  );

  injectPetStyle();
  void paint();
  // 配置里就写着要穿透（上次在控制台开的）⇒ 开窗即生效。
  // **动画不在这里起**：`paint()` 会按当前来源决定 —— 模型自己走 pixi 的 ticker，
  // 只有图片才需要那条 rAF（在这里无条件 startLoop 会在模型模式下把 rAF 又拉起来）。
  if (cfg.clickThrough) await applyClickThrough(true);

  petCleanup = () => {
    stopLoop();
    dropModel();
    offCtl();
    offVis();
  };
}

// ══════════════════════════════════════════════════════════════════

const petPlugin: Plugin = {
  id: "pet",
  name: "桌宠",
  keywords: ["桌宠", "宠物", "桌面宠物", "pet", "desktop pet", "live2d", "看板娘", "吉祥物"],
  description: "桌宠 —— 独立透明置顶的桌面形象窗（图片或 Live2D 模型，由你自己导入）",
  icon: "🐾",
  // **刻意不声明 `window.float`**：搜索打开的是**控制台**（普通内嵌面板），
  // 桌宠窗由控制台的「显示桌宠」打开。理由见文件头「为什么穿透开关必须放在控制台」。
  permissions: [],

  async execute(_input: string): Promise<PluginResult> {
    if (inPetWindow()) return { type: "html", content: petHtml() };
    return { type: "html", content: consoleHtml(loadConfig()) };
  },

  async attach(root: HTMLElement) {
    if (inPetWindow()) await attachPetWindow(root);
    else await attachConsole(root);
  },

  detach() {
    petCleanup?.();
    petCleanup = null;
  },
};

export default petPlugin;
