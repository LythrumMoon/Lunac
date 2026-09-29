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
// **形象来源 = 用户自备的图片**（png / jpg / webp / gif）。Live2D 引擎是下一步：
// 它卡在两个具体问题上 —— ① `.model3.json` 里的纹理 / 动作是**相对引用**，而
// asset 协议的 URL 是把整个绝对路径 percent-encode 进最后一段的（`convertFileSrc`
// 在上游就是这么写的），相对解析必然落到错的地方，必须先引资源改写；
// ② Cubism Core 是 Live2D 的专有文件（可再分发但有义务），且它的官方 URL
// **不带版本号** ⇒ 写死 sha256 会在上游某次更新后把安装整个卡住。
// 这两条都要先做一次最小实验再决定，别在没模型可验收的情况下先把引擎塞进来。

import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import type { Plugin, PluginResult } from "../registry";

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
  /** 形象图片的**绝对路径**（用户经文件对话框选的）。空 = 还没有形象。 */
  image: string;
  /** 形象占窗口宽度的比例。窗口是定尺的，所以「大小」调的是形象本身。 */
  scale: number;
  /** 鼠标穿透。**默认关**：开着时桌宠窗不吃任何鼠标事件，拖动与右键菜单都会失效。 */
  clickThrough: boolean;
}

const DEFAULT_CFG: PetConfig = { image: "", scale: 1, clickThrough: false };

function loadConfig(): PetConfig {
  try {
    const raw = localStorage.getItem(CFG_KEY);
    if (!raw) return { ...DEFAULT_CFG };
    const o = JSON.parse(raw) as Partial<PetConfig>;
    return {
      image: typeof o.image === "string" ? o.image : "",
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

// ══════════════════════════════════════════════════════════════════
//  控制台（主窗口里的普通面板）
// ══════════════════════════════════════════════════════════════════

function consoleHtml(cfg: PetConfig): string {
  const hasImage = cfg.image.trim().length > 0;
  return `
<div class="pet-console">
  <div class="pet-row pet-row-main">
    <button id="pet-show" class="pet-btn pet-btn-primary">${hasImage ? "显示桌宠" : "导入形象并显示"}</button>
    <button id="pet-hide" class="pet-btn">收起来</button>
  </div>
  <div class="pet-row">
    <button id="pet-pick" class="pet-btn">${hasImage ? "更换形象…" : "选择图片…"}</button>
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
  <div class="pet-file" id="pet-file">${
    hasImage ? `当前形象：<code>${esc(cfg.image)}</code>` : "还没有形象 —— 选一张图片（png / jpg / webp / gif 都行）。"
  }</div>
  <div class="pet-note">
    形象由你自己准备，Lunac 不随包分发任何模型或素材（桌宠是独立的悬浮窗，
    与搜索窗同时存在）。<b>穿打开着时桌宠窗收不到鼠标事件</b>，所以关掉它的开关
    只放在这里 —— 桌宠窗自己那份右键菜单在穿透状态下是收不到点击的。
  </div>
</div>`;
}

async function attachConsole(root: HTMLElement): Promise<void> {
  const cfg = loadConfig();
  const el = <T extends HTMLElement>(id: string) => root.querySelector<T>(`#${id}`);

  const renderFile = () => {
    const box = el("pet-file");
    if (!box) return;
    const c = loadConfig();
    box.innerHTML = c.image.trim()
      ? `当前形象：<code>${esc(c.image)}</code>`
      : "还没有形象 —— 选一张图片（png / jpg / webp / gif 都行）。";
  };

  async function showPet() {
    await invoke("open_plugin_window", { pluginId: "pet", input: "" });
  }

  el("pet-show")?.addEventListener("click", () => {
    void (async () => {
      if (!loadConfig().image.trim()) {
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
    void pickImage().then(p => {
      if (p) renderFile();
    });
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
    saveConfig({ ...loadConfig(), image: picked });
    renderFile();
    void emit(CTL_EVENT, { type: "reload" });
    return picked;
  }
}

// ══════════════════════════════════════════════════════════════════
//  桌宠窗
// ══════════════════════════════════════════════════════════════════

function petHtml(): string {
  return `
<div class="pet-root" id="pet-root">
  <div class="pet-stage" id="pet-stage">
    <img class="pet-img" id="pet-img" alt="" draggable="false">
    <div class="pet-empty" id="pet-empty">还没有形象<br><span>在控制台里选一张图片</span></div>
  </div>
  <div class="pet-menu" id="pet-menu">
    <button data-act="pick">更换形象…</button>
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
.pet-note{color:var(--text-dim);opacity:.72;font-size:.68rem;line-height:1.7;border-top:1px solid var(--border-glass);padding-top:9px;}
.pet-note b{color:var(--text);opacity:.9;}
`;

let petCleanup: (() => void) | null = null;

async function attachPetWindow(root: HTMLElement): Promise<void> {
  const el = <T extends HTMLElement>(id: string) => root.querySelector<T>(`#${id}`);
  const stage = el("pet-stage");
  const img = el<HTMLImageElement>("pet-img");
  const empty = el("pet-empty");
  const menu = el("pet-menu");

  let raf = 0;
  let cfg = loadConfig();

  // ── 渲染形象 ──────────────────────────────────────────────────
  function paint() {
    cfg = loadConfig();
    if (img && empty) {
      if (cfg.image.trim()) {
        img.src = convertFileSrc(cfg.image.trim());
        img.style.display = "";
        empty.style.display = "none";
      } else {
        img.removeAttribute("src");
        img.style.display = "none";
        empty.style.display = "";
      }
    }
    if (img) img.style.transform = `scale(${cfg.scale}) translateY(0)`;
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
          saveConfig({ ...loadConfig(), image: picked });
          paint();
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
        paint();
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
      paint();
      return;
    }
    if (p.type === "set-scale" && typeof p.value === "number") {
      saveConfig({ ...loadConfig(), scale: p.value });
      paint();
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
        stopLoop();
        closeMenu();
      } else {
        startLoop();
      }
    },
  );

  if (!document.getElementById("pet-style")) {
    const style = document.createElement("style");
    style.id = "pet-style";
    style.textContent = PET_CSS;
    document.head.appendChild(style);
  }

  paint();
  // 配置里就写着要穿透（上次在控制台开的）⇒ 开窗即生效；否则先起动画
  if (cfg.clickThrough) await applyClickThrough(true);
  startLoop();

  petCleanup = () => {
    stopLoop();
    offCtl();
    offVis();
  };
}

// ══════════════════════════════════════════════════════════════════

const petPlugin: Plugin = {
  id: "pet",
  name: "桌宠",
  keywords: ["桌宠", "宠物", "桌面宠物", "pet", "desktop pet", "live2d", "看板娘", "吉祥物"],
  description: "桌宠 —— 独立透明置顶的桌面形象窗（形象由你自己导入）",
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
