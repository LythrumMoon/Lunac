// tool-editor.ts — MCP tool definition manager
// Manage user-defined tools in Lunac 数据根（exe 所在目录）\tools\
import type { Plugin } from "../registry";
import { invoke } from "@tauri-apps/api/core";
import { t } from "../../i18n.js";

interface ToolEntry {
  filename: string;
  name: string;
  description: string;
  valid: boolean;
}

const TOOL_TEMPLATE = `{
  "name": "my-tool",
  "description": "What this tool does",
  "inputSchema": {
    "type": "object",
    "properties": {},
    "required": []
  },
  "handler": {
    "type": "shell",
    "command": "echo hello"
  }
}`;

function esc(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

function escapeAttr(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

async function loadToolFiles(): Promise<ToolEntry[]> {
  try {
    return await invoke<ToolEntry[]>("list_tool_files");
  } catch {
    return [];
  }
}

function getContainer(): HTMLElement {
  return document.getElementById("results-list")!;
}

async function renderToolList(container: HTMLElement) {
  const tools = await loadToolFiles();

  if (tools.length === 0) {
    container.innerHTML = `
      <div style="color:var(--text-dim);padding:16px 12px;font-size:0.82rem;text-align:center;">
        ${t("tooleditor.no_tools")}</div>`;
    return;
  }

  let html = "";
  for (const tool of tools) {
    const status = tool.valid
      ? `<span class="tool-status valid">${t("tooleditor.valid")}</span>`
      : `<span class="tool-status invalid">${t("tooleditor.invalid")}</span>`;
    html += `
      <div class="tool-item" data-filename="${escapeAttr(tool.filename)}">
        <div class="tool-item-header">
          <span class="tool-item-icon">🔧</span>
          <div class="tool-item-info">
            <div class="tool-item-name">${esc(tool.name || tool.filename)} ${status}</div>
            <div class="tool-item-desc">${esc(tool.description || t("tooleditor.no_description"))}</div>
          </div>
        </div>
        <div class="tool-item-actions">
          <button class="tool-btn-edit" data-filename="${escapeAttr(tool.filename)}">${t("tooleditor.edit")}</button>
          <button class="tool-btn-del" data-filename="${escapeAttr(tool.filename)}">${t("tooleditor.del")}</button>
        </div>
      </div>`;
  }
  container.innerHTML = html;
}

async function showEditor(filename?: string) {
  const container = getContainer();

  let existing = "";
  let title = t("tooleditor.new_title");
  if (filename) {
    try {
      existing = await invoke<string>("read_tool_file", { filename });
      title = t("tooleditor.edit_title", { name: esc(filename) });
    } catch {
      existing = TOOL_TEMPLATE;
    }
  } else {
    existing = TOOL_TEMPLATE;
  }

  container.innerHTML = `
    <div class="tool-editor">
      <div class="tool-editor-header">
        <span>${title}</span>
        <button id="tool-editor-back">${t("tooleditor.back")}</button>
      </div>
      <textarea id="tool-editor-textarea" spellcheck="false">${esc(existing)}</textarea>
      <div class="tool-editor-msg" id="tool-editor-msg" style="display:none;font-size:0.72rem;color:var(--red);"></div>
      <div class="tool-editor-actions">
        <button id="tool-editor-save">${t("tooleditor.save")}</button>
        <button id="tool-editor-cancel">${t("tooleditor.cancel")}</button>
      </div>
    </div>`;

  const showMsg = (text: string, color = "var(--red)") => {
    const msg = document.getElementById("tool-editor-msg");
    if (msg) {
      msg.style.display = "block";
      msg.style.color = color;
      msg.textContent = text;
      setTimeout(() => { if (msg) msg.style.display = "none"; }, 4000);
    }
  };

  document.getElementById("tool-editor-back")!.addEventListener("click", async () => {
    await renderToolList(container);
    attachToolEditorListeners(); // re-bind after DOM replace
  });
  document.getElementById("tool-editor-cancel")!.addEventListener("click", async () => {
    await renderToolList(container);
    attachToolEditorListeners(); // re-bind after DOM replace
  });
  document.getElementById("tool-editor-save")!.addEventListener("click", async () => {
    const textarea = document.getElementById("tool-editor-textarea") as HTMLTextAreaElement;
    const content = textarea.value;
    try {
      JSON.parse(content);
    } catch (e) {
      showMsg(t("tooleditor.invalid_json", { err: String(e) }));
      return;
    }

    let saveFn = filename || "";
    if (!saveFn) {
      try {
        const tool = JSON.parse(content);
        saveFn = (tool.name || "tool").replace(/[^a-zA-Z0-9_-]/g, "-") + ".json";
      } catch {
        saveFn = "new-tool.json";
      }
    }

    try {
      await invoke("save_tool_file", { filename: saveFn, content });
      await renderToolList(container);
      attachToolEditorListeners(); // re-bind after DOM replace
    } catch (err: any) {
      showMsg(t("tooleditor.save_failed", { err: String(err) }));
    }
  });
}

export function attachToolEditorListeners() {
  // Inject styles once
  const styleId = "tool-editor-styles";
  if (!document.getElementById(styleId)) {
    const style = document.createElement("style");
    style.id = styleId;
    style.textContent = `
      .tool-item { display:flex;align-items:center;justify-content:space-between;padding:8px 12px;border-radius:6px;margin-bottom:2px;cursor:default;transition:background 0.1s; }
      .tool-item:hover { background:rgba(255,255,255,0.05); }
      .tool-item-header { display:flex;align-items:center;gap:8px;flex:1;min-width:0; }
      .tool-item-icon { flex-shrink:0; }
      .tool-item-info { min-width:0; }
      .tool-item-name { font-size:0.8rem;color:var(--text);display:flex;align-items:center;gap:6px; }
      .tool-item-desc { font-size:0.7rem;color:var(--text-dim);overflow:hidden;text-overflow:ellipsis;white-space:nowrap; }
      .tool-status { font-size:0.6rem;padding:1px 6px;border-radius:4px;font-weight:600; }
      .tool-status.valid { background:rgba(157,180,172,0.15);color:var(--green); }
      .tool-status.invalid { background:rgba(192,138,138,0.15);color:var(--red); }
      .tool-item-actions { display:flex;gap:4px;flex-shrink:0; }
      .tool-item-actions button { padding:3px 10px;font-size:0.68rem;border-radius:4px;border:1px solid var(--border-glass);background:rgba(255,255,255,0.05);color:var(--text-dim);cursor:pointer;transition:background 0.1s; }
      .tool-item-actions button:hover { background:rgba(255,255,255,0.1);color:var(--text); }
      .tool-btn-del:hover { color:var(--red) !important; }
      .tool-btn-del.armed { background:var(--red) !important;color:#fff !important;border-color:var(--red) !important; }
      .tool-editor { display:flex;flex-direction:column;gap:8px;padding:8px 12px;height:280px; }
      .tool-editor-header { display:flex;align-items:center;justify-content:space-between; }
      .tool-editor-header span { font-size:0.82rem;font-weight:600;color:var(--text); }
      .tool-editor-header button { padding:4px 10px;font-size:0.72rem;border-radius:4px;border:1px solid var(--border-glass);background:none;color:var(--text-dim);cursor:pointer; }
      .tool-editor-header button:hover { color:var(--text);background:rgba(255,255,255,0.05); }
      #tool-editor-textarea { flex:1;background:rgba(0,0,0,0.3);border:1px solid var(--border-glass);border-radius:6px;padding:10px;color:var(--text);font-family:'Cascadia Code','Fira Code','Consolas',monospace;font-size:0.75rem;line-height:1.5;outline:none;resize:none;caret-color:var(--accent); }
      #tool-editor-textarea:focus { border-color:var(--accent-border); }
      .tool-editor-actions { display:flex;gap:6px;justify-content:flex-end; }
      #tool-editor-save { padding:5px 16px;font-size:0.75rem;border-radius:6px;border:1px solid var(--accent-border);background:var(--accent-bg);color:var(--accent);cursor:pointer; }
      #tool-editor-save:hover { background:rgba(192,160,160,0.2); }
      #tool-editor-cancel { padding:5px 12px;font-size:0.75rem;border-radius:6px;border:1px solid var(--border-glass);background:none;color:var(--text-dim);cursor:pointer; }
      #tool-editor-cancel:hover { color:var(--text);background:rgba(255,255,255,0.05); }
      /* 印象派按钮背景清理（与全局 styles.css 一致：背景全透明，无扫笔/光效） */
      .tool-item-actions button, .tool-editor-header button,
      #tool-editor-save, #tool-editor-cancel {
        background-image: none;
        box-shadow: none;
      }
    `;
    document.head.appendChild(style);
  }

  const container = getContainer();

  // New tool button
  const newBtn = document.getElementById("tool-new-btn");
  if (newBtn) {
    newBtn.addEventListener("click", () => showEditor());
    // Also set cursor style
    newBtn.style.cursor = "pointer";
  }

  // Edit buttons
  container.querySelectorAll(".tool-btn-edit").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      const fn = (btn as HTMLElement).dataset.filename!;
      showEditor(fn);
    });
  });

  // Delete buttons — two-step confirm (avoids WebView2 script dialogs)
  container.querySelectorAll(".tool-btn-del").forEach((btn) => {
    btn.addEventListener("click", async (e) => {
      e.stopPropagation();
      const fn = (btn as HTMLElement).dataset.filename!;
      const el = btn as HTMLElement;
      if (el.dataset.armed !== "1") {
        el.dataset.armed = "1";
        el.textContent = t("tooleditor.confirm_del");
        el.classList.add("armed");
        setTimeout(() => {
          delete el.dataset.armed;
          el.textContent = t("tooleditor.del");
          el.classList.remove("armed");
        }, 2500);
        return;
      }
      try {
        await invoke("delete_tool_file", { filename: fn });
      } catch (err: any) {
        const msg = document.createElement("div");
        msg.style.cssText = "color:var(--red);font-size:0.72rem;padding:4px 12px;";
        msg.textContent = t("tooleditor.delete_failed", { err: String(err) });
        container.prepend(msg);
        setTimeout(() => msg.remove(), 4000);
      }
      await renderToolList(container);
      // Re-attach listeners after re-render
      attachToolEditorListeners();
    });
  });
}

const toolEditor: Plugin = {
  id: "tool-editor",
  name: "Tool Editor",
  description: "Manage AI Agent custom tools (MCP bridge)",
  keywords: ["tool", "tools", "工具", "mcp", "agent", "skill", "插件", "扩展"],
  icon: "🔧",

  async execute(_input: string) {
    const tools = await loadToolFiles();
    let html = '<div class="plugin-result">';
    html += `<div style="display:flex;justify-content:space-between;align-items:center;padding:4px 12px 8px;">
      <span style="font-size:0.75rem;color:var(--text-dim);">${t("tooleditor.tool_count", { count: String(tools.length) })}</span>
      <button id="tool-new-btn" style="padding:4px 12px;font-size:0.72rem;border-radius:6px;border:1px solid var(--accent-border);background:var(--accent-bg);color:var(--accent);">${t("tooleditor.new_tool")}</button>
    </div>`;
    html += '<div style="overflow-y:auto;max-height:260px;">';

    if (tools.length === 0) {
      html += `<div style="color:var(--text-dim);padding:16px 12px;font-size:0.82rem;text-align:center;">${t("tooleditor.no_tools")}</div>`;
    } else {
      for (const tool of tools) {
        const status = tool.valid
          ? `<span class="tool-status valid">${t("tooleditor.valid")}</span>`
          : `<span class="tool-status invalid">${t("tooleditor.invalid")}</span>`;
        html += `
          <div class="tool-item" data-filename="${escapeAttr(tool.filename)}">
            <div class="tool-item-header">
              <span class="tool-item-icon">🔧</span>
              <div class="tool-item-info">
                <div class="tool-item-name">${esc(tool.name || tool.filename)} ${status}</div>
                <div class="tool-item-desc">${esc(tool.description || t("tooleditor.no_description"))}</div>
              </div>
            </div>
            <div class="tool-item-actions">
              <button class="tool-btn-edit" data-filename="${escapeAttr(tool.filename)}">${t("tooleditor.edit")}</button>
              <button class="tool-btn-del" data-filename="${escapeAttr(tool.filename)}">${t("tooleditor.del")}</button>
            </div>
          </div>`;
      }
    }
    html += "</div></div>";
    return { type: "html", content: html };
  },
};

export default toolEditor;
