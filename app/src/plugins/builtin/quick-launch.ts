// Custom Local Launch plugin — manages user-added paths/files/shortcuts.
//
// 数据源（2026-09 修订）：持久注册表 <exe 根>\ModuleData\custom\app_registry.json。
// 入口：
//   1. 搜索栏拖入 / 粘贴的文件（addFileChip → add_custom_app 自动注册）
//   2. 本面板「添加启动项」按钮（原生文件选择框）
//   3. 命令行/手动粘贴路径后回车（搜索栏气泡）
//
// 面板行为：打开即列出全部已注册自定义文件（含会话内附加但未注册的临时项），
// 支持逐条启动 / 删除（删除 = 注销 + 移除对应气泡）；删除/新增后派发事件让主
// 程序刷新面板，数据不会“消失”。
//
// 入口词已做多语言/拼音覆盖（launch/open/qidong/dakai/自定义/快速启动…），
// pluginRegistry 会对中文关键词自动生成拼音索引。

import type { Plugin } from "../registry";
import { open } from "@tauri-apps/plugin-shell";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { t } from "../../i18n.js";

interface CustomEntry {
  name: string;
  path: string;
  source?: string;
  icon?: string | null;
}

/** 列表行数据：persistent = 已注册（ModuleData 持久） */
interface LaunchRow {
  path: string;
  name: string;
  persistent: boolean;
}

export const quickLaunchPlugin: Plugin = {
  id: "quick-launch",
  name: "Custom Launch",
  keywords: [
    "open", "launch", "run", "app", "start", "file", "path", "exec", "custom",
    "打开", "启动", "运行", "应用", "程序", "文件", "路径", "自定义", "快速启动",
    "启动器", "快捷启动", "add",
  ],
  description: "Launch custom files, apps, or paths — 启动/运行自定义文件与程序",
  icon: "📌",
  badge: "Launch",

  async execute(_input: string) {
    // 收集已注册项 + 会话附加文件（合并，按路径去重）
    const attachedFiles: string[] = (window as any).__lunac_attached_files || [];
    let registered: CustomEntry[] = [];
    try {
      registered = await invoke<CustomEntry[]>("list_custom_apps");
    } catch {
      // registry 不可读时仅显示会话附加项
    }

    // 兜底：会话气泡若尚未写进注册表（旧会话 / 偶发失败）补注册，保证面板可见
    for (const p of attachedFiles) {
      invoke("add_custom_app", { name: "", path: p }).catch(() => {});
    }

    const byPath = new Map<string, LaunchRow>();
    for (const r of registered) {
      if (!r.path) continue;
      const k = r.path.toLowerCase();
      const existing = byPath.get(k);
      byPath.set(k, {
        path: r.path,
        name: existing?.name || r.name || displayNameOf(r.path),
        persistent: true,
      });
    }
    for (const p of attachedFiles) {
      const k = p.toLowerCase();
      const existing = byPath.get(k);
      byPath.set(k, {
        path: p,
        name: existing?.name || displayNameOf(p),
        persistent: !!existing?.persistent,
      });
    }
    const items = [...byPath.values()];

    const toolbar = `
      <div class="ql-toolbar">
        <span class="ql-title">${escapeHtml(t("plugin.quick-launch"))}</span>
        <button type="button" class="ql-add-btn" title="${escapeAttr(t("quicklaunch.add_file"))}">
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor"
               stroke-width="2.2" stroke-linecap="round"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>
          <span>${escapeHtml(t("quicklaunch.add_item"))}</span>
        </button>
      </div>`;

    if (items.length === 0) {
      return {
        type: "html",
        content: `${toolbar}<div class="ql-empty">${escapeHtml(t("quicklaunch.empty"))}</div>`,
      };
    }

    const rows = items.map((row) => {
      const badge = row.persistent
        ? escapeHtml(t("quicklaunch.registered"))
        : escapeHtml(t("quicklaunch.session"));
      return `<div class="result-item" style="cursor:pointer" data-custom-path="${escapeAttr(row.path)}">
          <div class="result-item-icon">📌</div>
          <div class="result-item-content">
            <div class="result-item-title">${escapeHtml(row.name)}</div>
            <div class="result-item-desc" title="${escapeAttr(row.path)}">${escapeHtml(row.path)}</div>
          </div>
          <button class="result-item-remove ql-remove" data-remove-path="${escapeAttr(row.path)}"
                  title="${escapeAttr(t("quicklaunch.remove"))}">×</button>
          <span class="result-item-badge app-badge">${badge}</span>
        </div>`;
    }).join("");

    return { type: "html", content: toolbar + rows };
  },
};

function displayNameOf(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

// ── Attach click handlers after DOM injection ──────────────────

export function attachQuickLaunchListeners(container: HTMLElement) {
  // “添加启动项”：原生多选文件对话框 → 写入注册表 → 通知主程序刷新面板
  container.querySelectorAll(".ql-add-btn").forEach((btn) => {
    btn.addEventListener("click", async (e) => {
      e.stopPropagation();
      try {
        const { open: pick } = await import("@tauri-apps/plugin-dialog");
        const selected = await pick({
          multiple: true,
          title: t("quicklaunch.add_file"),
        });
        const paths = Array.isArray(selected) ? selected : selected ? [selected] : [];
        for (const p of paths) {
          await invoke("add_custom_app", { name: "", path: p }).catch(() => {
            // already registered / missing — keep going
          });
        }
        if (paths.length > 0) {
          window.dispatchEvent(new CustomEvent("lunac:quicklaunch-changed"));
        }
      } catch {
        // dialog cancelled / error — do nothing
      }
    });
  });

  container.querySelectorAll("[data-custom-path]").forEach((el) => {
    const elem = el as HTMLElement;
    const path = elem.dataset.customPath;
    if (!path) return;

    // Remove button — 注销注册表项 + 摘除对应气泡；主程序收到后刷新面板
    const removeBtn = elem.querySelector(".ql-remove") as HTMLElement | null;
    if (removeBtn) {
      removeBtn.addEventListener("click", (e) => {
        e.stopPropagation();
        window.dispatchEvent(
          new CustomEvent("lunac:remove-attached-file", { detail: { path } }),
        );
      });
    }

    elem.addEventListener("click", (e) => {
      // Ignore clicks on the remove button (handled above)
      if (e.target === removeBtn || (removeBtn && removeBtn.contains(e.target as Node))) return;
      // Hide first, then launch — fire both concurrently, no await
      getCurrentWindow().hide().catch(() => {});
      invoke("launch_app", { path }).catch(async () => {
        await open(path);
      });
    });
  });
}

// ── Helpers ────────────────────────────────────────────────────

function escapeHtml(str: string): string {
  return str.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function escapeAttr(str: string): string {
  return str.replace(/"/g, "&quot;").replace(/&/g, "&amp;");
}
