// ── 插件 sidecar 的「宿主侧实现」（2026-10-06，L12 档 1）────────────────
//
// 这个文件**只跑在宿主里**（main.ts / plugin-window.ts 各一份实例），**不会**被打进插件包 ——
// 插件拿到的只是 `plugins/host.ts` 里声明的那份 `LunacSidecarApi` 契约（经 `__lunac_host` 全局）。
//
// 职责三件：
//   ① 把 `__lunac_host.sidecar` 的四个方法转发到宿主命令（`plugin_sidecar_*`）；
//   ② **信任卡**：`start()` 收到 `needs_trust` 时弹卡（如实列 command / args / sha256 / 授权端口），
//      用户裁决后才真正起进程 —— 与 MCP 的 `.mcp.json` 信任门同款，**默认拒绝**；
//   ③ **绑定活动插件**：一个窗口同时只挂一个插件面板，桥按 `activePluginId` 寻址；
//      换插件 / 关面板时收掉上一个插件的 sidecar（契约 ⑥「必须绑定面板」）。
//
// 插件自己**永远不碰端口**：`http()` 的目标由宿主去连（前端 CSP 的 `default-src` 不含
// `127.0.0.1`，插件直接 `fetch()` 会被 WebView 拦掉 —— 见 tauri.conf.json）。

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { t } from "../i18n.js";
import type { LunacSidecarApi, LunacSidecarHttpOptions } from "./host";

/** 当前窗口里挂着的那个插件（sidecar 的归属）。同一窗口一次只有一个面板。 */
let activePluginId: string | null = null;

/** 面板打开时绑定（由 main.ts / plugin-window.ts 调）。换插件会先收掉上一个的 sidecar。 */
export function activatePluginSidecar(pluginId: string | null): void {
  if (activePluginId && activePluginId !== pluginId) {
    void stopQuietly(activePluginId);
  }
  activePluginId = pluginId;
}

/** 面板关闭时解绑（并收掉 sidecar）。 */
export function deactivatePluginSidecar(): void {
  if (activePluginId) void stopQuietly(activePluginId);
  activePluginId = null;
}

async function stopQuietly(pluginId: string): Promise<void> {
  try {
    await invoke("plugin_sidecar_stop", { pluginId });
  } catch {
    // 收尾失败不打扰用户（进程侧的 Job Object 兜底：宿主退出时会一并收掉）
  }
}

function requirePlugin(): string {
  if (!activePluginId) throw new Error("sidecar: 当前没有活动的插件面板");
  return activePluginId;
}

function esc(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

/** 一张「要不要让这个插件起进程」的卡；返回用户的裁决。 */
function showSidecarTrustCard(info: {
  pluginId: string;
  command: string;
  args: string[];
  sha256: string;
  transport: string;
  allowLocalPorts: number[];
  allowLocalAny: boolean;
}): Promise<"once" | "always" | "deny"> {
  return new Promise(resolve => {
    const host = document.getElementById("results-list") ?? document.body;
    const card = document.createElement("div");
    card.className = "approval-card mcp-trust-card sidecar-trust-card";

    const portText = info.allowLocalAny
      ? t("plugin.sidecar_ports_any")
      : info.allowLocalPorts?.length
        ? info.allowLocalPorts.join(", ")
        : t("plugin.sidecar_ports_own");

    const row = (tag: string, val: string) =>
      `<div><span class="mcp-trust-tag">${esc(tag)}</span><code>${esc(val || "—")}</code></div>`;

    card.innerHTML = `
      <div class="approval-title">${esc(t("plugin.sidecar_trust_title"))}</div>
      <div class="mcp-trust-project">${esc(info.pluginId)}</div>
      <div class="mcp-trust-list">
        ${row("command", info.command)}
        ${row("args", (info.args ?? []).join(" "))}
        ${row("sha256", info.sha256)}
        ${row(t("plugin.sidecar_ports_tag"), portText)}
      </div>
      <div class="mcp-trust-hint">${esc(t("plugin.sidecar_trust_hint"))}</div>
      <div class="approval-actions">
        <button class="approval-btn" data-d="once">${esc(t("plugin.sidecar_trust_once"))}</button>
        <button class="approval-btn" data-d="always">${esc(t("plugin.sidecar_trust_always"))}</button>
        <button class="approval-btn danger" data-d="deny">${esc(t("plugin.sidecar_trust_deny"))}</button>
      </div>`;

    card.querySelectorAll<HTMLButtonElement>("button[data-d]").forEach(btn => {
      btn.addEventListener("click", () => {
        const d = (btn.dataset.d as "once" | "always" | "deny") ?? "deny";
        card.remove();
        resolve(d);
      });
    });
    host.appendChild(card);
    card.scrollIntoView({ block: "nearest" });
  });
}

/** 当前窗口里那份桥（`maybeAutoStartSidecar` 要用）。 */
let bridgeRef: LunacSidecarApi | null = null;

/** 面板打开且清单声明了 `sidecar.autostart` 时，宿主自动起进程（失败只记控制台，不打扰用户）。 */
export function maybeAutoStartSidecar(autostart: boolean): void {
  if (!autostart || !bridgeRef) return;
  void bridgeRef.start().catch(err => console.warn("[lunac sidecar] autostart failed:", err));
}

/** 造一份桥，交给 `installHostBridge({ t, apiVersion: 2, sidecar })`。 */
export function createSidecarBridge(): LunacSidecarApi {
  // 事件分发表：`event`（推送行的 method）→ 回调集
  const handlers = new Map<string, Set<(payload: unknown) => void>>();
  let listening = false;

  const ensureListen = (): void => {
    if (listening) return;
    listening = true;
    void listen<{ pluginId: string; payload: { method?: string } }>("plugin-sidecar-event", e => {
      const d = e.payload;
      const method = d?.payload?.method;
      if (typeof method === "string") {
        handlers.get(method)?.forEach(cb => {
          try {
            cb(d.payload);
          } catch (err) {
            console.error("[lunac sidecar] handler failed:", err);
          }
        });
      }
    });
    void listen<{ pluginId: string }>("plugin-sidecar-exit", e => {
      handlers.get("__exit__")?.forEach(cb => {
        try {
          cb(e.payload);
        } catch {
          /* ignore */
        }
      });
    });
  };

  const api: LunacSidecarApi = {
    async start() {
      ensureListen();
      const pluginId = requirePlugin();
      const first = await invoke<{ status: string; port?: number }>("plugin_sidecar_start", { pluginId });
      if (first?.status !== "needs_trust") {
        return { status: "started", port: first?.port ?? undefined };
      }
      const decision = await showSidecarTrustCard(first as never);
      if (decision === "deny") {
        await invoke("plugin_sidecar_deny", { pluginId }).catch(() => {});
        throw new Error(t("plugin.sidecar_denied"));
      }
      await invoke("plugin_sidecar_trust", { pluginId, remember: decision === "always" });
      const again = await invoke<{ status: string; port?: number }>("plugin_sidecar_start", { pluginId });
      return { status: "started", port: again?.port ?? undefined };
    },

    async request(method: string, params?: unknown) {
      ensureListen();
      return invoke("plugin_sidecar_request", { pluginId: requirePlugin(), method, params: params ?? null });
    },

    async http(opts: LunacSidecarHttpOptions) {
      ensureListen();
      return invoke<{ status: number; body: string }>("plugin_sidecar_http", {
        pluginId: requirePlugin(),
        method: opts.method ?? "GET",
        path: opts.path,
        body: opts.body ?? null,
        port: opts.port ?? null,
      });
    },

    on(event: string, cb: (payload: unknown) => void) {
      ensureListen();
      let set = handlers.get(event);
      if (!set) {
        set = new Set();
        handlers.set(event, set);
      }
      set.add(cb);
      return () => {
        set?.delete(cb);
      };
    },

    async stop() {
      return invoke<boolean>("plugin_sidecar_stop", { pluginId: requirePlugin() });
    },
  };
  bridgeRef = api;
  return api;
}
