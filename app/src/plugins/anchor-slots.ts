// ── 常驻挂载锚点（2026-10-18）────────────────────────────────────
//
// `chat-sidebar` / `settings-section` 由清单 `render.slots` 声明，**只支持 iframe 引擎**
// （DOM 插件没有独立文档，宿主无法把它的样式 / 脚本限制在槽位内，见 docs/todo.md C.4.1）。
//
// 宿主职责：建容器 → 挂 iframe → 跑同一套信道（握手 / mount / 能力校验 / 主题快照）。
// 纪律：
//   · 无插件 ⇒ 调用方把容器隐藏，**不留空占位**；
//   · 卸载 / 重扫 ⇒ 调用方 dispose，本模块摘监听并清空容器（不残留）。

import { invoke } from "@tauri-apps/api/core";
import { t } from "../i18n";
import { ChannelHost, pluginFrameUrl } from "./channel/host";
import { collectThemeTokens } from "./channel/theme";
import { installedMarketPlugins, type MarketPluginInfo } from "./market";

/** iframe 的预期 origin（Windows 自定义协议 = `http://lunac-plugin.localhost`）。 */
const IFRAME_ORIGIN = "http://lunac-plugin.localhost";

export type AnchorSlot = "chat-sidebar" | "settings-section";

export interface AnchorMountOptions {
  slot: AnchorSlot;
  onLog?: (level: string, message: string) => void;
}

export interface AnchorMountHandle {
  readonly count: number;
  dispose(): void;
}

/** 当前活跃的信道（主题切换时统一转发；dispose 时逐个摘掉）。 */
const liveHosts = new Set<ChannelHost>();

/** 宿主换主题 / 换 token 后调用：把快照推给所有常驻锚点。 */
export function themeChangedAnchorPlugins(): void {
  for (const host of liveHosts) host.themeChanged();
}

/** 这个槽位该挂哪些插件（`valid` 且 iframe 引擎；顺序沿用扫描的目录名升序）。 */
export function anchorPlugins(slot: AnchorSlot): MarketPluginInfo[] {
  return installedMarketPlugins().filter(
    (plugin) =>
      plugin.valid &&
      plugin.render?.engine === "iframe" &&
      (plugin.render.slots ?? []).includes(slot),
  );
}

/** 把声明了该槽位的插件挂进容器；返回句柄（`count = 0` 时调用方应隐藏容器）。 */
export function mountAnchorPlugins(
  container: HTMLElement,
  options: AnchorMountOptions,
): AnchorMountHandle {
  const hosts: ChannelHost[] = [];
  const sections: HTMLElement[] = [];
  const observers: ResizeObserver[] = [];

  for (const plugin of anchorPlugins(options.slot)) {
    const section = document.createElement("section");
    section.className = "anchor-plugin";
    section.dataset.pluginId = plugin.id;

    const frame = document.createElement("iframe");
    frame.className = "anchor-plugin-frame";
    // 与主窗内嵌面板同一口径：allow-same-origin 只用于 targetOrigin 校验，仍与宿主跨源。
    frame.setAttribute("sandbox", "allow-scripts allow-same-origin");
    frame.src = pluginFrameUrl(plugin.id);
    section.appendChild(frame);
    container.appendChild(section);

    let failed = false;
    const fail = (message: string): void => {
      if (failed) return; // 握手超时与 mount 失败可能先后到达，只留一条
      failed = true;
      const note = document.createElement("div");
      note.className = "anchor-plugin-error";
      note.textContent = message;
      section.appendChild(note);
    };

    const host = new ChannelHost({
      iframe: frame,
      pluginId: plugin.id,
      permissions: plugin.permissions ?? [],
      engine: "iframe",
      origin: IFRAME_ORIGIN,
      appearance: plugin.render.appearance,
      getTheme: () => ({ mode: "dark", tokens: collectThemeTokens() }),
      handlers: {
        "net.fetch": async (payload) => {
          const req = payload as {
            url: string;
            method?: string;
            headers?: Record<string, string>;
            body?: string;
            timeoutMs?: number;
            maxBytes?: number;
          };
          return invoke("plugin_net_request", {
            pluginId: plugin.id,
            url: req.url,
            method: req.method,
            headers: req.headers,
            body: req.body,
            timeoutMs: req.timeoutMs,
            maxBytes: req.maxBytes,
          });
        },
      },
      onReady: () => {
        // 用 section 自己的尺寸：设置分组是多个插件纵向排列，不能都拿容器总高。
        const width = section.clientWidth || container.clientWidth;
        const height = section.clientHeight || container.clientHeight;
        void host
          .mount(null, options.slot, width > 0 && height > 0 ? { width, height } : undefined)
          .catch((e: unknown) => fail(e instanceof Error ? e.message : String(e)));
      },
      onResize: (size) => {
        frame.style.height = `${size.height}px`;
      },
      onLog: (level, message) => {
        options.onLog?.(level, `[plugin ${plugin.id}] ${message}`);
      },
      onError: (error) => {
        fail(`${t("plugin.float_init_failed")}（${error.code}: ${error.message}）`);
      },
    });

    host.start();
    hosts.push(host);
    sections.push(section);
    liveHosts.add(host);

    // 挂载时容器可能还藏在未激活的设置分页里（尺寸 0）；可见 / 折叠变化时再推一次。
    if (typeof ResizeObserver !== "undefined") {
      const observer = new ResizeObserver((entries) => {
        const rect = entries[0]?.contentRect;
        if (!rect || rect.width <= 0) return;
        host.resizeTo({
          width: Math.round(rect.width),
          height: Math.round(rect.height),
        });
      });
      observer.observe(section);
      observers.push(observer);
    }
  }

  let disposed = false;
  return {
    count: hosts.length,
    dispose() {
      if (disposed) return;
      disposed = true;
      for (const observer of observers) observer.disconnect();
      for (const host of hosts) {
        liveHosts.delete(host);
        host.dispose();
      }
      for (const section of sections) section.remove();
      observers.length = 0;
      hosts.length = 0;
      sections.length = 0;
    },
  };
}
