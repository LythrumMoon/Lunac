// 本地模型决策开关（2026-10-24 从「设置 → AI」迁到输入栏「更多设置 → 其他」）。
//
// 只负责这一块的**渲染 + 事件编排**（与设置面板原来的实现等价）：
//   开 ⇒（依赖未就位先下载，带进度/可取消）→ 起 sidecar（落盘信任）→ 写开关 → 重启 agent；
//   关 ⇒ 写开关 → 收 sidecar → 重启 agent。
// 新增：每次打开 / 切换后调一次宿主 `local_model_probe`，把「端点已注入」升级成
// 「真实探活成功 / 失败原因」—— 2026-10-24 的实验里出现过「llama-server 空转、零推理」
// 却仍显示已启用的情况，这一行就是为它补的可观测性。

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { t } from "./i18n.js";
import { refreshMarketPlugins, hasDiskPlugin, diskPluginReady } from "./plugins/market";
import { startPluginSidecarTrusted, stopPluginSidecar } from "./plugins/sidecar";
import { InstallOverall, formatSpeed, ringHtml, updateRing } from "./download-progress.js";

interface LocalModelProbe {
  enabled: boolean;
  ready: boolean;
  url: string;
  latencyMs: number;
  model: string | null;
  error: string | null;
}

/** 宿主 `local_decision_status`（2026-10-24）：**决策层**是否真的参与，而不是只看模型探活。 */
interface LocalDecisionStatus {
  enabled: boolean;
  configEnabled: boolean;
  configDecide: boolean;
  endpoint: string;
  slots: number;
  counts: { prewarm: number; safety: number; memory: number; rewrite: number; route: number };
  lastLine: string | null;
}

let toggle: HTMLInputElement | null = null;
let enabled = false;
let label: HTMLElement | null = null;
let hint: HTMLElement | null = null;
let status: HTMLElement | null = null;
let decisionEl: HTMLElement | null = null;
let warn: HTMLElement | null = null;
let depsRow: HTMLElement | null = null;
let depsBtn: HTMLButtonElement | null = null;
let progress: HTMLElement | null = null;
let okBtn: HTMLButtonElement | null = null;
let cancelBtn: HTMLButtonElement | null = null;
let lastProbe: LocalModelProbe | null = null;
let lastDecision: LocalDecisionStatus | null = null;

function cacheEls(root: HTMLElement): void {
  toggle = root.querySelector<HTMLInputElement>("#chat-local-model-toggle");
  label = root.querySelector<HTMLElement>("#chat-local-model-label");
  hint = root.querySelector<HTMLElement>("#chat-local-model-hint");
  status = root.querySelector<HTMLElement>("#chat-local-model-status");
  decisionEl = root.querySelector<HTMLElement>("#chat-local-model-decision");
  warn = root.querySelector<HTMLElement>("#chat-local-model-warn");
  depsRow = root.querySelector<HTMLElement>("#chat-local-model-deps");
  depsBtn = root.querySelector<HTMLButtonElement>("#chat-local-model-download");
  progress = root.querySelector<HTMLElement>("#chat-local-model-progress");
  okBtn = root.querySelector<HTMLButtonElement>("#chat-local-model-ok");
  cancelBtn = root.querySelector<HTMLButtonElement>("#chat-local-model-cancel");
}

function defaultHint(): string {
  return !hasDiskPlugin("local-model")
    ? t("settings.local_model_missing")
    : !diskPluginReady("local-model")
      ? t("settings.local_model_deps_missing")
      : t("settings.local_model_hint");
}

function resetHint(): void {
  if (!hint) return;
  hint.textContent = defaultHint();
  hint.style.color = "var(--text-dim)";
}

function setMsg(s: string, isErr = false): void {
  if (!s) return resetHint();
  if (!hint) return;
  hint.textContent = s;
  hint.style.color = isErr ? "var(--red)" : "var(--text-dim)";
}

/** 探活状态行：只有真跑过一次探活才显示结论。 */
function renderProbeStatus(): void {
  if (!status) return;
  if (!lastProbe) {
    status.textContent = "";
    return;
  }
  if (!lastProbe.enabled) {
    status.textContent = t("settings.local_model_status_off");
    status.style.color = "var(--text-dim)";
    return;
  }
  if (lastProbe.ready) {
    status.textContent = t("settings.local_model_status_ready", {
      ms: String(lastProbe.latencyMs),
      model: lastProbe.model || "unknown",
    });
    status.style.color = "var(--green)";
  } else {
    status.textContent = t("settings.local_model_status_failed", {
      err: lastProbe.error || "unknown",
    });
    status.style.color = "var(--red)";
  }
}

async function probeNow(): Promise<void> {
  try {
    lastProbe = await invoke<LocalModelProbe>("local_model_probe");
  } catch (e) {
    lastProbe = {
      enabled: true,
      ready: false,
      url: "",
      latencyMs: 0,
      model: null,
      error: String(e instanceof Error ? e.message : e),
    };
  }
  renderProbeStatus();
  await refreshDecision();
}

/** 决策层状态行（2026-10-24）：把「模型探活成功」与「决策真的在跑」分开显示。
 *  两者不一致时（模型 ready、判定 0 条）用黄色提示，正是 C 组实验踩过的现场。 */
function renderDecision(): void {
  if (!decisionEl) return;
  const d = lastDecision;
  if (!d) {
    decisionEl.textContent = "";
    return;
  }
  if (!d.enabled) {
    decisionEl.textContent = d.configEnabled && !d.configDecide
      ? t("settings.local_decision_only_probe")
      : t("settings.local_decision_off");
    decisionEl.style.color = "var(--text-dim)";
    return;
  }
  const n = d.counts.prewarm + d.counts.safety + d.counts.memory + d.counts.rewrite + d.counts.route;
  decisionEl.textContent = n > 0
    ? t("settings.local_decision_on", { n: String(n) })
    : t("settings.local_decision_zero");
  decisionEl.style.color = n > 0 ? "var(--green)" : "var(--yellow)";
}

async function refreshDecision(): Promise<void> {
  try {
    lastDecision = await invoke<LocalDecisionStatus>("local_decision_status");
  } catch {
    lastDecision = null;
  }
  renderDecision();
}

/** 刚开启时模型还在加载：隔几秒重试探活，成功后停止（最多 3 次）。 */
async function probeAfterStart(): Promise<void> {
  for (const delay of [1_000, 3_000, 6_000]) {
    await new Promise((r) => window.setTimeout(r, delay));
    await probeNow();
    if (lastProbe?.ready) return;
  }
}

/** 语言切换时重刷本块的静态文案与探活行（数据不重新请求）。 */
export function renderLocalModelLabels(): void {
  if (label) label.textContent = t("settings.local_model");
  // 与设置面板同一套 .settings-toggle（全局样式，见 docs/ui-style.md §界面 2）。
  if (toggle) {
    toggle.checked = enabled;
    toggle.title = t("settings.local_model");
    toggle.setAttribute("aria-label", t("settings.local_model"));
  }
  if (depsBtn) depsBtn.textContent = t("settings.local_model_download_deps");
  if (okBtn) okBtn.textContent = t("settings.local_model_confirm");
  if (cancelBtn) cancelBtn.textContent = t("settings.local_model_cancel");
  resetHint();
  renderProbeStatus();
  renderDecision();
}

/** 挂载到「更多设置 → 其他」页的容器上（只挂一次；重复调用只重刷文案）。 */
export async function mountLocalModelSection(root: HTMLElement): Promise<void> {
  const initialEnabled = await invoke<boolean>("local_model_config_get").catch(() => false);
  enabled = initialEnabled;
  const installed = hasDiskPlugin("local-model");
  const ready = diskPluginReady("local-model");

  root.innerHTML = `
    <div class="chat-more-row">
      <span class="chat-more-label" id="chat-local-model-label"></span>
      <label class="settings-toggle">
        <input type="checkbox" id="chat-local-model-toggle">
        <span class="settings-toggle-slider"></span>
      </label>
    </div>
    <div class="settings-hint" id="chat-local-model-hint" style="font-size:0.7rem;color:var(--text-dim);margin:2px 0 6px;"></div>
    <div class="settings-hint" id="chat-local-model-status" style="font-size:0.7rem;margin:2px 0 6px;"></div>
    <div class="settings-hint" id="chat-local-model-decision" style="font-size:0.7rem;margin:2px 0 6px;"></div>
    <div class="chat-more-actions${installed && !ready ? "" : " hidden"}" id="chat-local-model-deps" style="flex-wrap:wrap;margin:2px 0 6px;">
      <div class="dl-inline hidden" id="chat-local-model-progress"></div>
      <button type="button" id="chat-local-model-download"></button>
    </div>
    <div class="settings-hint hidden" id="chat-local-model-warn" style="font-size:0.72rem;color:var(--yellow);margin:2px 0 6px;line-height:1.6;">
      <span id="chat-local-model-warn-text"></span>
      <div class="chat-more-actions" style="margin-top:6px;">
        <button type="button" id="chat-local-model-ok"></button>
        <button type="button" id="chat-local-model-cancel"></button>
      </div>
    </div>`;
  const warnText = root.querySelector<HTMLElement>("#chat-local-model-warn-text");
  if (warnText) warnText.textContent = t("settings.local_model_warn");
  cacheEls(root);
  renderLocalModelLabels();
  void probeNow();

  const showDeps = (on: boolean) => depsRow?.classList.toggle("hidden", !on);
  const setDepsBtn = (text: string, disabled = false) => {
    if (!depsBtn) return;
    depsBtn.textContent = text;
    depsBtn.disabled = disabled;
  };

  let depBusy = false;
  let depCancelled = false;
  let depUnlisten: (() => void) | undefined;

  const installDeps = async (): Promise<"ok" | "cancelled" | "failed"> => {
    if (depBusy) return "failed";
    depBusy = true;
    depCancelled = false;
    showDeps(true);
    setDepsBtn(t("settings.local_model_deps_cancel"));
    setMsg(t("settings.local_model_deps_downloading"));
    if (progress) {
      progress.classList.remove("hidden");
      progress.innerHTML =
        ringHtml(-1) +
        `<span class="dl-meta"><span class="dl-phase">${t("settings.local_model_deps_downloading")}</span>` +
        `<span class="dl-speed">—</span></span>`;
    }
    const overall = new InstallOverall();
    try {
      depUnlisten = await listen("plugin-install-progress", (ev) => {
        const p = ev.payload as {
          id: string;
          phase: string;
          downloaded: number;
          total: number;
          index: number;
          count: number;
        };
        if (p.id !== "local-model") return;
        // 两条依赖（llama-server 压缩包 + GGUF）逐个下载 ⇒ 必须看**总体**进度，
        // 否则换文件时环会从 100% 掉回 0%（2026-10-27）。
        const s = overall.push(p);
        if (progress) {
          updateRing(progress, s.percent);
          const ph = progress.querySelector<HTMLElement>(".dl-phase");
          const sp = progress.querySelector<HTMLElement>(".dl-speed");
          if (ph) ph.textContent = s.extracting
            ? t("settings.plugins_market_extracting")
            : t("settings.local_model_deps_downloading");
          if (sp) sp.textContent = s.extracting ? "—" : formatSpeed(s.speed);
        }
      });
    } catch { /* 挂不上监听不该拦住安装本身 */ }
    let result: "ok" | "cancelled" | "failed" = "failed";
    try {
      await invoke("install_plugin_dependencies", { id: "local-model" });
      result = "ok";
    } catch {
      result = depCancelled ? "cancelled" : "failed";
    } finally {
      try { depUnlisten?.(); } catch { /* ignore */ }
      depUnlisten = undefined;
      depBusy = false;
    }
    progress?.classList.add("hidden");
    await refreshMarketPlugins().catch(() => {});
    showDeps(!diskPluginReady("local-model"));
    if (result === "ok") {
      setMsg(t("settings.local_model_deps_done"));
    } else if (result === "cancelled") {
      setMsg(t("settings.local_model_deps_cancelled"));
    } else {
      setMsg(t("settings.local_model_deps_failed"), true);
    }
    setDepsBtn(result === "failed"
      ? t("settings.local_model_deps_retry")
      : t("settings.local_model_download_deps"));
    return result;
  };

  const turnOn = async () => {
    setMsg(t("settings.local_model_working"));
    try {
      if (!hasDiskPlugin("local-model")) throw new Error(t("settings.local_model_missing"));
      if (!diskPluginReady("local-model")) {
        const r = await installDeps();
        if (r === "cancelled") return;
        if (r !== "ok") throw new Error(t("settings.local_model_deps_failed"));
      }
      await startPluginSidecarTrusted("local-model");
      await invoke("local_model_config_set", { enabled: true });
      enabled = true;
      renderLocalModelLabels();
      setMsg("");
      await (window as any).__lunac_reload_agent?.();
      // 等 sidecar 加载模型后再探活（llama-server 开始监听时 /health 仍可能是 503）。
      void probeAfterStart();
    } catch (e) {
      enabled = false;
      renderLocalModelLabels();
      setMsg(String(e instanceof Error ? e.message : e), true);
    }
  };

  const turnOff = async () => {
    setMsg(t("settings.local_model_working"));
    try {
      await invoke("local_model_config_set", { enabled: false });
      await stopPluginSidecar("local-model");
      enabled = false;
      renderLocalModelLabels();
      setMsg("");
      await (window as any).__lunac_reload_agent?.();
      lastProbe = { enabled: false, ready: false, url: "", latencyMs: 0, model: null, error: null };
      renderProbeStatus();
      await refreshDecision();
    } catch (e) {
      enabled = true;
      renderLocalModelLabels();
      setMsg(String(e instanceof Error ? e.message : e), true);
    }
  };

  toggle?.addEventListener("click", (e) => e.stopPropagation());
  toggle?.addEventListener("change", () => {
    if (!toggle) return;
    warn?.classList.add("hidden");
    if (toggle.checked) {
      // 开：先回退勾选，弹**内联警告**；确认后才真信任 + 起进程（不用会被布局吞掉的信任卡）。
      toggle.checked = false;
      warn?.classList.remove("hidden");
    } else {
      void turnOff();
    }
  });

  depsBtn?.addEventListener("click", () => {
    if (depBusy) {
      depCancelled = true;
      setDepsBtn(t("settings.local_model_deps_cancel"), true);
      void invoke("cancel_plugin_dependency_install").catch(() => {});
      return;
    }
    void installDeps();
  });

  okBtn?.addEventListener("click", () => {
    warn?.classList.add("hidden");
    void turnOn();
  });
  cancelBtn?.addEventListener("click", () => warn?.classList.add("hidden"));
}

/** 供调试 / 测试读取最近一次探活结果。 */
export function lastLocalModelProbe(): LocalModelProbe | null {
  return lastProbe;
}
