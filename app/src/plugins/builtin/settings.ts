// Keep track of the last active settings category across open/close cycles
let activeSettingsCategory = "general";

// Unlisten functions for hotkey recording events — cleaned up on re-attach to avoid memory leaks
let _clickOutsideHandler: ((e: Event) => void) | null = null;
let _onRecordingCaptured: ((e: Event) => void) | null = null;
let _onRecordingCancelled: (() => void) | null = null;
let _unlistenHotkeyRecorded: (() => void) | null = null; // Tauri event (for Alt+Space via WndProc)

import type { Plugin } from "../registry";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-shell";
import { t, setLanguage, resetToSystemLanguage } from "../../i18n.js";


function esc(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

function formatKey(vk: number): string {
  const map: Record<number, string> = {
    0x0d: "Enter", 0x1b: "Esc", 0x20: "Space", 0x21: "PgUp", 0x22: "PgDn",
    0x23: "End", 0x24: "Home", 0x25: "←", 0x26: "↑", 0x27: "→", 0x28: "↓",
    0x2d: "Ins", 0x2e: "Del",
    0x70: "F1", 0x71: "F2", 0x72: "F3", 0x73: "F4", 0x74: "F5",
    0x75: "F6", 0x76: "F7", 0x77: "F8", 0x78: "F9",
    0x79: "F10", 0x7a: "F11", 0x7b: "F12",
    0xa0: "LShift", 0xa1: "RShift", 0xa2: "LCtrl", 0xa3: "RCtrl",
    0xa4: "LAlt", 0xa5: "RAlt",
    0x5b: "LWin", 0x5c: "RWin",
    0xbc: ",", 0xbe: ".", 0xbf: "/", 0xba: ";", 0xde: "'",
    0xdb: "[", 0xdd: "]", 0xdc: "\\", 0xc0: "`",
    0xbd: "-", 0xbb: "=",
  };
  if (map[vk]) return map[vk];
  if (vk >= 0x30 && vk <= 0x39) return String.fromCharCode(vk);
  if (vk >= 0x41 && vk <= 0x5a) return String.fromCharCode(vk);
  return String.fromCharCode(vk);
}

// ── Content pane builders (one per category) ────────────────────

/** Language options shown in the General pane. Names are native so they're
 *  recognizable in any UI language; only the "Follow System" label is
 *  translated via t(). */
const LANG_OPTIONS: { value: string; labelKey?: string; label?: string }[] = [
  { value: "system", labelKey: "settings.lang_follow_system" },
  { value: "zh-CN", label: "简体中文" },
  { value: "zh-TW", label: "繁體中文" },
  { value: "ja", label: "日本語" },
  { value: "ko", label: "한국어" },
  { value: "en", label: "English" },
];

function currentLangSelection(): string {
  try {
    return localStorage.getItem("lunac_lang") || "system";
  } catch { return "system"; }
}

function buildGeneralPane(hotkey: string, autoStart: boolean): string {
  const langOptions = LANG_OPTIONS.map(o => ({
    value: o.value,
    label: o.label ?? t(o.labelKey!),
    selected: o.value === currentLangSelection(),
  }));
  const langSelectHtml = renderCustomSelectInline("settings-language", langOptions);

  return `
    <div class="settings-pane" data-pane="general" id="sp-general">
      <div class="settings-pane-title">${t("settings.general")}</div>
      <div class="settings-row">
        <span class="settings-label">${t("settings.hotkey")}</span>
        <button id="settings-hotkey-btn" class="settings-hotkey" title="${t("settings.hotkey_record")}">
          ${esc(hotkey)}
        </button>
      </div>
      <div class="settings-row">
        <span class="settings-label">${t("settings.auto_start")}</span>
        <label class="settings-toggle">
          <input type="checkbox" id="settings-autostart" ${autoStart ? "checked" : ""}>
          <span class="settings-toggle-slider"></span>
        </label>
      </div>
      <div class="settings-row">
        <span class="settings-label">${t("settings.language")}</span>
        ${langSelectHtml}
      </div>
      <div class="settings-row">
        <span class="settings-label">${t("settings.background_title")}</span>
        <div class="settings-bg-actions">
          <button id="settings-bg-apply" class="settings-btn">${t("settings.background_apply")}</button>
          <button id="settings-bg-clear" class="settings-btn">${t("settings.background_clear")}</button>
        </div>
      </div>
    </div>`;
}

/** Local variant of renderCustomSelect (general pane) — plain labels, no icons. */
function renderCustomSelectInline(id: string, options: { value: string; label: string; selected: boolean }[]): string {
  const optsHtml = options.map(o =>
    `<div class="custom-select-option${o.selected ? ' selected' : ''}" data-value="${esc(o.value)}">${esc(o.label)}</div>`
  ).join("");
  const selectedLabel = options.find(o => o.selected)?.label || options[0]?.label || "";
  return `
    <div class="custom-select" id="${id}">
      <button class="custom-select-trigger" type="button">
        <span class="custom-select-label">${esc(selectedLabel)}</span>
        <svg class="custom-select-arrow" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="6 9 12 15 18 9"/></svg>
      </button>
      <div class="custom-select-dropdown">${optsHtml}</div>
    </div>`;
}

async function buildSearchPane(): Promise<string> {
  let engine = "google";
  try {
    const saved = localStorage.getItem("lunac-search-engine");
    // DuckDuckGo preset removed (2026-09) — migrate stale saved value back to Google
    if (saved === "google" || saved === "bing" || saved === "baidu") {
      engine = saved;
    } else if (saved) {
      engine = "google";
      localStorage.setItem("lunac-search-engine", "google");
    }
  } catch {}

  const engines = [
    { id: "google",     name: "Google",       url: "https://www.google.com/search?q={{query}}" },
    { id: "bing",       name: "Bing",         url: "https://www.bing.com/search?q={{query}}" },
    { id: "baidu",      name: "Baidu",        url: "https://www.baidu.com/s?wd={{query}}" },
  ];

  const renderCustomSelect = (id: string, options: { value: string; label: string; selected: boolean; tag?: string }[], allowDelete = false): string => {
    const optsHtml = options.map(o => {
      const del = allowDelete && o.value !== "__custom__" ? `<span class="cs-del" title="×">×</span>` : "";
      return `<div class="custom-select-option${o.selected ? ' selected' : ''}" data-value="${esc(o.value)}" data-tag="${o.tag || 'builtin'}">${esc(o.label)}${del}</div>`;
    }).join("");
    const sel = options.find(o => o.selected);
    return `
      <div class="custom-select" id="${id}">
        <button class="custom-select-trigger" type="button">
          <span class="custom-select-label">${esc(sel?.label || options[0]?.label || "")}</span>
          <svg class="custom-select-arrow" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="6 9 12 15 18 9"/></svg>
        </button>
        <div class="custom-select-dropdown">${optsHtml}</div>
      </div>`;
  };

  const engSelectHtml = renderCustomSelect("settings-search-engine", engines.map(e => ({
    value: e.id,
    label: e.name,
    selected: e.id === engine,
  })));

  return `
    <div class="settings-pane" data-pane="search" id="sp-search">
      <div class="settings-pane-title">${t("settings.web_search")}</div>
      <div class="settings-row">
        <span class="settings-label">${t("settings.search_engine")}</span>
        ${engSelectHtml}
      </div>
      <div class="settings-row" style="justify-content: flex-end;">
        <button id="settings-save-search-btn" class="settings-save-btn">${t("settings.save")}</button>
        <span id="settings-save-search-msg" class="settings-save-msg"></span>
      </div>
      <div class="settings-hint" style="margin-top:8px;font-size:0.7rem;color:var(--text-dim);">${t("settings.search_engine_hint")}</div>
    </div>`;
}

// ── AI 供应商/模型预设（单一数据源）─────────────────────────────
// buildAIPane 与 attachSettingsListeners 共用，避免两处漂移。
// 接口地址默认不带 /v1 —— 与主流供应商文档一致；实际 Anthropic 端点
// 由 cli.exe 启动时在 base 上派生（commands.rs 已处理 /v1 双重路径）。
interface ModelPreset { name: string; default_model: string; default_url: string; }
const PROVIDER_PRESETS: Record<string, ModelPreset> = {
  "openai":       { name: "OpenAI",         default_model: "gpt-5.5",            default_url: "https://api.openai.com" },
  "deepseek":     { name: "DeepSeek",       default_model: "deepseek-v4-pro",    default_url: "https://api.deepseek.com" },
  "anthropic":    { name: "Anthropic",      default_model: "claude-sonnet-5",    default_url: "https://api.anthropic.com" },
  "google":       { name: "Google Gemini",  default_model: "gemini-3.6-flash",   default_url: "https://generativelanguage.googleapis.com/v1beta/openai" },
  "zhipu":        { name: "Zhipu GLM",      default_model: "glm-5",              default_url: "https://open.bigmodel.cn/api/paas/v4" },
  "moonshot":     { name: "Moonshot Kimi",  default_model: "kimi-k3",            default_url: "https://api.moonshot.cn" },
  "qwen":         { name: "Qwen (Tongyi)",  default_model: "qwen3.8-max",        default_url: "https://dashscope.aliyuncs.com/compatible-mode" },
  "siliconflow":  { name: "SiliconFlow",    default_model: "Qwen/Qwen3-235B-A22B", default_url: "https://api.siliconflow.cn" },
  "custom":       { name: "Custom",         default_model: "",                   default_url: "" },
};

const MODEL_SUGGESTIONS: Record<string, string[]> = {
  "openai":     ["gpt-5.5", "gpt-5.5-pro", "gpt-5.4", "gpt-5.4-mini", "gpt-5.4-nano"],
  "deepseek":   ["deepseek-v4-pro", "deepseek-v4-flash"],
  "anthropic":  ["claude-sonnet-5", "claude-opus-5", "claude-fable-5", "claude-haiku-4-5-20251001"],
  "google":     ["gemini-3.6-flash", "gemini-3.5-flash", "gemini-3.1-pro-preview", "gemini-3.1-flash-lite"],
  "zhipu":      ["glm-5", "glm-4.7", "glm-4.5-air", "glm-4.5-flash"],
  "moonshot":   ["kimi-k3", "kimi-k2.6", "kimi-k2.7-code"],
  "qwen":       ["qwen3.8-max", "qwen3.7-max", "qwen3.7-plus", "qwen3.7-flash"],
  "siliconflow": ["Qwen/Qwen3-235B-A22B", "deepseek-ai/DeepSeek-V3", "Qwen/Qwen3-30B-A3B", "zai-org/GLM-5"],
  "custom":     [],
};

/** 由模型名反推其所属供应商（用于未配置 provider 时的默认值兜底）。 */
function inferProviderForModel(model: string, fallback: string): string {
  if (!model) return fallback;
  for (const [k, list] of Object.entries(MODEL_SUGGESTIONS)) {
    if (k !== "custom" && list.includes(model)) return k;
  }
  for (const [k, p] of Object.entries(PROVIDER_PRESETS)) {
    if (k !== "custom" && p.default_model === model) return k;
  }
  return fallback;
}

function buildAIPane(provider: string, baseUrl: string, model: string, apiKey: string): string {
  const masked = apiKey ? apiKey.slice(0, 4) + "\u2022\u2022\u2022\u2022" + apiKey.slice(-4) : "";

  // Filter out built-in providers the user deleted (persisted hidden-list)
  let hiddenProvs: string[] = [];
  try { hiddenProvs = JSON.parse(localStorage.getItem("lunac-hidden-providers") || "[]"); } catch {}
  const provKeys = Object.keys(PROVIDER_PRESETS).filter(p => !hiddenProvs.includes(p) || p === provider);
  // Custom dropdown HTML helper
  const renderCustomSelect = (id: string, options: { value: string; label: string; selected: boolean; tag?: string }[], allowDelete = false): string => {
    const optsHtml = options.map(o => {
      const del = allowDelete && o.value !== "__custom__" ? `<span class="cs-del" title="×">×</span>` : "";
      return `<div class="custom-select-option${o.selected ? ' selected' : ''}" data-value="${esc(o.value)}" data-tag="${o.tag || 'builtin'}">${esc(o.label)}${del}</div>`;
    }).join("");
    const selectedLabel = options.find(o => o.selected)?.label || options[0]?.label || "";
    return `
      <div class="custom-select" id="${id}">
        <button class="custom-select-trigger" type="button">
          <span class="custom-select-label">${esc(selectedLabel)}</span>
          <svg class="custom-select-arrow" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="6 9 12 15 18 9"/></svg>
        </button>
        <div class="custom-select-dropdown">${optsHtml}</div>
      </div>`;
  };
  const provOpts: { value: string; label: string; selected: boolean; tag?: string }[] = provKeys.map(p => ({
    value: p,
    label: PROVIDER_PRESETS[p].name,
    selected: p === provider
  }));
  // 已保存的自定义供应商（不在预设列表中）→ 显示为选中项
  if (provider && !provKeys.includes(provider)) {
    provOpts.push({ value: provider, label: provider, selected: true, tag: "custom" });
  }
  // 追加持久化的自定义供应商（重开设置面板仍在）
  try {
    const customProvs: string[] = JSON.parse(localStorage.getItem("lunac-custom-providers") || "[]");
    for (const p of customProvs) {
      if (p && !provKeys.includes(p) && p !== provider) {
        provOpts.push({ value: p, label: p, selected: false, tag: "custom" });
      }
    }
  } catch {}
  // 底部保留 "__custom__" 入口，用于新增自定义供应商
  provOpts.push({ value: "__custom__", label: t("settings.custom_provider"), selected: false });
  const provSelectHtml = renderCustomSelect("settings-provider", provOpts, true);

    // ── Model custom select ──────────────────────────────────
    // Filter out built-in models the user deleted (persisted hidden-list)
    let hiddenModels: string[] = [];
    try {
      const hm = JSON.parse(localStorage.getItem("lunac-hidden-models") || "{}");
      hiddenModels = (hm && hm[provider]) || [];
    } catch {}
    const modelList = (MODEL_SUGGESTIONS[provider] || []).filter(m => !hiddenModels.includes(m) || m === model);
    const hasCustomModel = model && !modelList.includes(model);
    const modelOptions: { value: string; label: string; selected: boolean; tag?: string }[] = modelList.map(m => ({
      value: m,
      label: m,
      selected: m === model
    }));
    if (hasCustomModel) {
      modelOptions.push({ value: model, label: `${model} (${t("settings.custom_model")})`, selected: true, tag: "custom" });
    }
    // 追加持久化的自定义模型（重开设置面板仍在）
    try {
      const map = JSON.parse(localStorage.getItem("lunac-custom-models") || "{}");
      const customModels: string[] = (map && map[provider]) || [];
      for (const m of customModels) {
        if (m && !modelList.includes(m) && m !== model) {
          modelOptions.push({ value: m, label: `${m} (${t("settings.custom_model")})`, selected: false, tag: "custom" });
        }
      }
    } catch {}
    modelOptions.push({ value: "__custom__", label: t("settings.custom_model"), selected: false });
    const modelSelectHtml = renderCustomSelect("settings-model", modelOptions, true);

  return `
    <div class="settings-pane" data-pane="ai" id="sp-ai">
      <div class="settings-pane-title">${t("settings.ai_model")}</div>
      <div class="settings-row">
        <span class="settings-label">${t("settings.provider")}</span>
        ${provSelectHtml}
      </div>
      <div class="settings-row">
        <span class="settings-label">${t("settings.model")}</span>
        ${modelSelectHtml}
      </div>
      <div class="settings-row">
        <span class="settings-label">${t("settings.base_url")}</span>
        <input type="text" id="settings-baseurl" class="settings-input" autocomplete="off" value="${esc(baseUrl)}" placeholder="${esc(PROVIDER_PRESETS[provider]?.default_url || "https://api.openai.com")}">
      </div>
      <div class="settings-row">
        <span class="settings-label">${t("settings.api_key")}</span>
        <input type="password" id="settings-apikey" class="settings-input" autocomplete="off" value="${esc(apiKey)}" placeholder="${masked || 'sk-...'}">
      </div>
      <div class="settings-row" style="justify-content: flex-end;">
        <button id="settings-save-ai-btn" class="settings-save-btn">${t("settings.save")}</button>
        <span id="settings-save-msg" class="settings-save-msg"></span>
      </div>
    </div>`;
}

// ── Skill Store (技能扩展) ─────────────────────────────────────

/** 推荐的国内可直连技能/智能体平台入口 */
const SKILL_SITES: { name: string; desc: string; url: string }[] = [
  { name: "Coze 扣子", desc: "字节跳动 AI 智能体 / 技能平台", url: "https://www.coze.cn/" },
  { name: "智谱清言智能体广场", desc: "智谱 AI 智能体 / 技能市场", url: "https://bigmodel.cn/marketplace/index/agent" },
  { name: "百度文心智能体平台", desc: "百度智能体创作与发布平台", url: "https://agents.baidu.com/" },
  { name: "腾讯元器", desc: "腾讯智能体创作平台", url: "https://yuanqi.tencent.com/" },
  { name: "Coze 技能页示例", desc: "xiaping.coze.com — Coze 技能页面", url: "http://xiaping.coze.com/" },
];

function loadSkillSites(): { name: string; url: string }[] {
  try {
    const raw = localStorage.getItem("lunac-skill-sites");
    const arr = raw ? JSON.parse(raw) : [];
    return Array.isArray(arr) ? arr.filter((s: any) => s && s.url) : [];
  } catch { return []; }
}

function saveSkillSites(list: { name: string; url: string }[]) {
  try { localStorage.setItem("lunac-skill-sites", JSON.stringify(list)); } catch {}
}

function skillEntryHtml(name: string, desc: string, url: string, removable: boolean): string {
  return `
    <div class="settings-marketplace-item">
      <div class="settings-marketplace-info">
        <span class="settings-marketplace-name">${esc(name)}</span>
        <span class="settings-marketplace-desc">${esc(desc)}</span>
      </div>
      <div style="display:flex;gap:6px;flex-shrink:0;">
        <button class="settings-skill-open" data-url="${esc(url)}">${t("settings.skill_open")}</button>
        ${removable ? `<button class="settings-skill-del" data-url="${esc(url)}" data-name="${esc(name)}">${t("settings.skill_remove")}</button>` : ""}
      </div>
    </div>`;
}

/** 新建技能编辑器的默认 SKILL.md 模板。 */
const SKILL_TEMPLATE = `---
name: my-skill
description: Describe what this skill does and when it should be used.
---

Write the concrete instructions/steps of this skill here.
`;

/** 已安装技能行 HTML（buildSkillsPane 初次渲染与操作后重渲染共用）。 */
function installedSkillRowHtml(s: { name: string; description: string; dir: string; key: string }): string {
  return `
    <div class="settings-marketplace-item">
      <div class="settings-marketplace-info">
        <span class="settings-marketplace-name">${esc(s.name)}</span>
        <span class="settings-marketplace-desc">${esc(s.description || s.dir)}</span>
      </div>
      <div style="display:flex;gap:6px;flex-shrink:0;">
        <button class="settings-skill-open" data-dir="${esc(s.dir)}" title="${esc(s.dir)}">${t("settings.skill_open")}</button>
        <button class="settings-skill-edit" data-key="${esc(s.key)}">${t("settings.edit")}</button>
        <button class="settings-skill-del-installed" data-key="${esc(s.key)}">${t("settings.skill_remove")}</button>
      </div>
    </div>`;
}

async function buildSkillsPane(): Promise<string> {
  const recommendedHtml = SKILL_SITES.map(r => skillEntryHtml(r.name, r.desc, r.url, false)).join("");
  const custom = loadSkillSites();
  const customHtml = custom.length === 0
    ? `<div class="settings-plugin-empty">${t("settings.skill_empty")}</div>`
    : custom.map(s => skillEntryHtml(s.name, s.url, s.url, true)).join("");

  // ── 已安装技能（Lunac 数据根 <exe 所在目录>\skills）—— 具体 skill 的前端体现 ──
  let installedHtml = "";
  try {
    const skills: Array<{ name: string; description: string; dir: string; key: string }> =
      await invoke("list_installed_skills");
    installedHtml = skills.length === 0
      ? `<div class="settings-plugin-empty">${t("settings.skills_none")}</div>`
      : skills.map(s => installedSkillRowHtml(s)).join("");
  } catch {
    installedHtml = `<div class="settings-plugin-empty">${t("settings.load_error")}</div>`;
  }

  return `
    <div class="settings-pane" data-pane="skills" id="sp-skills">
      <div class="settings-pane-title">${t("settings.skills")}</div>
      <div class="settings-marketplace-section">
        <div class="settings-marketplace-title">${t("settings.skills_installed")}</div>
        <div class="settings-hint" style="font-size:0.7rem;color:var(--text-dim);margin:2px 0 6px;">${t("settings.skills_installed_hint")}</div>
        <div class="settings-skill-list" id="settings-skill-installed">${installedHtml}</div>
      </div>
      <div class="settings-marketplace-section">
        <div class="settings-marketplace-title">${t("settings.skill_install")}</div>
        <div class="settings-marketplace-row" style="flex-wrap:wrap;">
          <input type="text" id="settings-skill-install-url" class="settings-input" autocomplete="off" placeholder="${t("settings.skill_install_url_ph")}" style="flex:1 1 180px;max-width:none;">
          <button id="settings-skill-install-url-btn" class="settings-install-btn">${t("settings.skill_install_url")}</button>
          <button id="settings-skill-new-btn" class="settings-install-btn">${t("settings.skill_new")}</button>
        </div>
        <div id="settings-skill-op-msg" style="font-size:0.7rem;color:var(--text-dim);margin-top:4px;display:none;"></div>
        <div id="settings-skill-editor" style="display:none;margin-top:6px;">
          <textarea id="settings-skill-editor-text" class="settings-input" spellcheck="false" style="flex:none;width:100%;max-width:100%;height:170px;font-family:ui-monospace,Consolas,monospace;font-size:0.68rem;resize:vertical;line-height:1.4;"></textarea>
          <div style="display:flex;gap:6px;justify-content:flex-end;margin-top:6px;">
            <button id="settings-skill-editor-save" class="settings-install-btn">${t("settings.save")}</button>
            <button id="settings-skill-editor-cancel" class="settings-btn">${t("settings.cancel")}</button>
          </div>
        </div>
      </div>
      <div class="settings-marketplace-section">
        <div class="settings-marketplace-title">${t("settings.skill_add_url")}</div>
        <div class="settings-marketplace-row">
          <input type="text" id="settings-skill-url" class="settings-input" autocomplete="off" placeholder="https://xxx.coze.com/" style="max-width:100%;">
          <button id="settings-skill-add" class="settings-install-btn">${t("settings.skill_add")}</button>
        </div>
        <div id="settings-skill-msg" style="font-size:0.7rem;color:var(--text-dim);margin-top:4px;display:none;"></div>
      </div>
      <div class="settings-marketplace-section">
        <div class="settings-marketplace-title">${t("settings.skill_recommended")}</div>
        ${recommendedHtml}
      </div>
      <div class="settings-marketplace-section">
        <div class="settings-marketplace-title">${t("settings.skill_custom")}</div>
        <div class="settings-skill-list" id="settings-skill-custom">${customHtml}</div>
      </div>
    </div>`;
}

async function buildPluginsPane(): Promise<string> {
  let toolsHtml = "";
  let communityHtml = "";
  try {
    const tools: Array<{ filename: string; name: string; description: string; valid: boolean }> =
      await invoke("list_tool_files");
    if (tools.length === 0) {
      toolsHtml = `<div class="settings-plugin-empty">${t("settings.no_tools")}</div>`;
    } else {
      for (const t of tools) {
        const badge = t.valid
          ? '<span class="tool-badge valid">✓</span>'
          : '<span class="tool-badge invalid">✗</span>';
        toolsHtml += `
          <div class="settings-plugin-item">
            <div class="settings-plugin-info">
              <span class="settings-plugin-name">${esc(t.name || t.filename)} ${badge}</span>
              <span class="settings-plugin-desc">${esc(t.description || t.filename)}</span>
            </div>
          </div>`;
      }
    }

    // ── Community preset tools (inside try so 'tools' is in scope) ──
    const communityTools = [
      { name: "Get Weather", desc: "Query wttr.in for city weather", cmd: "curl -s \"wttr.in/{{city}}?format=3\"" },
      { name: "Web Search", desc: "Search the web via Google", cmd: "curl -s \"https://www.google.com/search?q={{query}}\"" },
      { name: "System Info", desc: "Get Windows system information", cmd: "systeminfo | findstr /B /C:\"OS Name\" /C:\"Total Physical Memory\" /C:\"System Type\"" },
      { name: "List Files", desc: "List files in a directory", cmd: "dir \"{{path}}\" /b"},
    ];

    const installedNames = new Set(tools.map(t => t.filename));
    communityHtml = `<div class="settings-marketplace-title">${t("settings.community_plugins")}</div>`;
    for (const ct of communityTools) {
      const expectedFn = ct.name.replace(/\s/g, "-").toLowerCase() + ".json";
      const installed = installedNames.has(expectedFn);
      communityHtml += `
        <div class="settings-marketplace-item">
          <div class="settings-marketplace-info">
            <span class="settings-marketplace-name">${esc(ct.name)}</span>
            <span class="settings-marketplace-desc">${esc(ct.desc)}</span>
          </div>
          <button class="settings-install-btn${installed ? ' installed' : ''}"
            data-name="${esc(ct.name.replace(/\s/g, '-').toLowerCase())}"
            data-cmd="${esc(ct.cmd)}" data-desc="${esc(ct.desc)}"
            ${installed ? 'disabled' : ''}>${installed ? t("settings.installed_btn") : t("settings.install")}</button>
        </div>`;
    }
  } catch {
    toolsHtml = `<span class="settings-plugin-empty">${t("settings.load_error")}</span>`;
    communityHtml = `<span class="settings-plugin-empty">${t("settings.marketplace_error")}</span>`;
  }

  return `
    <div class="settings-pane" data-pane="plugins" id="sp-plugins">
      <div class="settings-pane-title">${t("settings.plugins")}</div>

      <!-- Install from URL -->
      <div class="settings-marketplace-section">
        <div class="settings-marketplace-title">${t("settings.install_from_url")}</div>
        <div class="settings-marketplace-row">
          <input type="text" id="settings-tool-url" class="settings-input" autocomplete="off" placeholder="https://example.com/tool.json" style="max-width:100%;">
          <button id="settings-install-url" class="settings-install-btn">${t("settings.install")}</button>
        </div>
        <div id="settings-install-msg" style="font-size:0.7rem;color:var(--text-dim);margin-top:4px;display:none;"></div>
      </div>

      <!-- Installed tools -->
      <div class="settings-marketplace-section">
        <div class="settings-marketplace-title">${t("settings.installed")}</div>
        <div class="settings-plugin-list">${toolsHtml}</div>
      </div>

      <!-- Community tools -->
      <div class="settings-marketplace-section">
        ${communityHtml}
      </div>

      <div class="settings-pane-footer">
        <button id="settings-open-tools" class="settings-tool-btn">${t("settings.open_tool_editor")}</button>
      </div>
    </div>`;
}

// ── Listener attachment ──────────────────────────────────────────

export async function attachSettingsListeners(container: HTMLElement) {
  console.log("[lunac settings] attachSettingsListeners called, container:", container.id, "children:", container.children.length);
  const styleId = "settings-sidebar-styles";
  if (!document.getElementById(styleId)) {
    const style = document.createElement("style");
    style.id = styleId;
    style.textContent = `
      /* Layout: sidebar + content */
      .settings-layout {
        display: flex;
        height: 100%;
        min-height: 240px;
        overflow: hidden;
      }
      .settings-sidebar {
        width: 30%;
        min-width: 100px;
        border-right: 1px solid var(--border-glass);
        background: rgba(255,255,255,0.02);
        display: flex;
        flex-direction: column;
        flex-shrink: 0;
      }
      .settings-sidebar-item {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 10px 12px 10px 0;
        font-size: 0.78rem;
        color: var(--text-dim);
        cursor: pointer;
        transition: background 0.1s, color 0.1s;
        border-left: 3px solid transparent;
        user-select: none;
      }
      .settings-sidebar-item:hover {
        background: rgba(255,255,255,0.04);
        color: var(--text);
      }
      .settings-sidebar-item.active {
        color: var(--accent);
        background: var(--accent-bg);
        border-left-color: var(--accent);
      }

      .settings-content {
        flex: 1;
        overflow: hidden;
        padding: 4px 0;
      }
      /* Allow native <select> dropdowns to overflow the content area and layout */
      .settings-layout.sel-open {
        overflow: visible;
      }
      .settings-content.sel-open {
        overflow: visible;
      }
      #results-list.sel-open,
      #results-container.sel-open {
        overflow: visible;
      }

      /* Panes */
      .settings-pane {
        display: none;
        padding: 0 14px;
      }
      .settings-pane.active {
        display: block;
      }
      .settings-pane-title {
        font-size: 0.8rem;
        font-weight: 600;
        color: var(--text);
        margin-bottom: 10px;
        padding-bottom: 6px;
        border-bottom: 1px solid var(--border-glass);
      }
      .settings-pane-footer {
        display: flex;
        justify-content: flex-end;
        padding-top: 8px;
      }

      /* Rows */
      .settings-row {
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: 6px 0;
        gap: 10px;
      }
      .settings-bg-actions {
        display: flex;
        gap: 6px;
        flex-shrink: 0;
      }
      .settings-label {
        font-size: 0.76rem;
        color: var(--text-dim);
        flex-shrink: 0;
      }
      .settings-input {
        flex: 1;
        max-width: 220px;
        height: 26px;
        box-sizing: border-box;
        padding: 5px 8px;
        font-size: 0.73rem;
        background: rgba(0,0,0,0.25);
        border: 1px solid var(--border-glass);
        border-radius: 6px;
        color: var(--text);
        outline: none;
        caret-color: var(--accent);
      }
      .settings-input:focus { border-color: var(--accent-border); }
      .settings-btn {
        flex-shrink: 0;
        padding: 5px 10px;
        font-size: 0.7rem;
        border-radius: 6px;
        border: 1px solid var(--border-glass);
        background: rgba(255,255,255,0.06);
        color: var(--text-dim);
        cursor: pointer;
        transition: background 0.1s, color 0.1s;
      }
      .settings-btn:hover {
        background: rgba(255,255,255,0.12);
        color: var(--text);
      }
      .settings-select {
        flex: 1;
        max-width: 220px;
        height: 26px;
        box-sizing: border-box;
        padding: 5px 8px;
        font-size: 0.73rem;
        background: rgba(0,0,0,0.25);
        border: 1px solid var(--border-glass);
        border-radius: 6px;
        color: var(--text);
        outline: none;
      }
      .settings-hotkey {
        padding: 5px 12px;
        font-size: 0.73rem;
        border-radius: 6px;
        border: 1px solid var(--accent-border);
        background: var(--accent-bg);
        color: var(--accent);
        cursor: pointer;
        min-width: 90px;
        text-align: center;
        transition: background 0.1s;
      }
      .settings-hotkey:hover { background: rgba(192,160,160,0.2); }
      .settings-hotkey.recording {
        border-color: var(--yellow);
        background: rgba(201,184,150,0.15);
        color: var(--yellow);
        animation: pulse-recording 0.8s infinite alternate;
      }
      @keyframes pulse-recording {
        from { opacity: 1; } to { opacity: 0.5; }
      }
      .settings-toggle {
        position: relative;
        display: inline-block;
        width: 36px;
        height: 20px;
        flex-shrink: 0;
      }
      .settings-toggle input { display: none; }
      .settings-toggle-slider {
        position: absolute;
        cursor: pointer;
        top: 0; left: 0; right: 0; bottom: 0;
        background: rgba(255,255,255,0.1);
        border-radius: 20px;
        transition: background 0.2s;
      }
      .settings-toggle-slider::before {
        content: "";
        position: absolute;
        height: 14px; width: 14px;
        left: 3px; bottom: 3px;
        background: var(--text-dim);
        border-radius: 50%;
        transition: transform 0.2s, background 0.2s;
      }
      .settings-toggle input:checked + .settings-toggle-slider {
        background: var(--accent-bg);
        border: 1px solid var(--accent-border);
      }
      .settings-toggle input:checked + .settings-toggle-slider::before {
        transform: translateX(16px);
        background: var(--accent);
      }
      .settings-plugin-list {
        max-height: 200px;
        overflow-y: auto;
      }
      .settings-plugin-item {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 6px 8px;
        border-radius: 6px;
        transition: background 0.1s;
      }
      .settings-plugin-item:hover { background: rgba(255,255,255,0.04); }
      .settings-plugin-info { display: flex; flex-direction: column; min-width: 0; }
      .settings-plugin-name { font-size: 0.76rem; color: var(--text); display: flex; align-items: center; gap: 6px; }
      .settings-plugin-desc { font-size: 0.68rem; color: var(--text-dim); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
      .settings-plugin-empty { color: var(--text-dim); font-size: 0.75rem; padding: 8px 0; }
      .tool-badge { font-size: 0.58rem; padding: 1px 5px; border-radius: 4px; font-weight: 600; }
      .tool-badge.valid { background: rgba(157,180,172,0.15); color: var(--green); }
      .tool-badge.invalid { background: rgba(192,138,138,0.15); color: var(--red); }
      .settings-tool-btn {
        padding: 5px 12px;
        font-size: 0.72rem;
        border-radius: 6px;
        border: 1px solid var(--accent-border);
        background: var(--accent-bg);
        color: var(--accent);
        cursor: pointer;
        transition: background 0.1s;
      }
      .settings-tool-btn:hover { background: rgba(192,160,160,0.2); }
      .settings-marketplace-section { margin-bottom: 14px; }
      .settings-marketplace-title {
        font-size: 0.7rem;
        font-weight: 600;
        color: var(--text-dim);
        text-transform: uppercase;
        letter-spacing: 0.5px;
        margin-bottom: 6px;
      }
      .settings-marketplace-row {
        display: flex;
        gap: 6px;
        align-items: center;
      }
      .settings-marketplace-item {
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: 6px 8px;
        border-radius: 6px;
        transition: background 0.1s;
      }
      .settings-marketplace-item:hover { background: rgba(255,255,255,0.04); }
      .settings-marketplace-info { display: flex; flex-direction: column; min-width: 0; }
      .settings-marketplace-name { font-size: 0.76rem; color: var(--text); }
      .settings-marketplace-desc { font-size: 0.68rem; color: var(--text-dim); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
      .settings-install-btn {
        padding: 3px 10px;
        font-size: 0.68rem;
        border-radius: 4px;
        border: 1px solid var(--green);
        background: rgba(157,180,172,0.1);
        color: var(--green);
        cursor: pointer;
        flex-shrink: 0;
        transition: background 0.1s;
      }
      .settings-install-btn:hover { background: rgba(157,180,172,0.2); }
      .settings-install-btn.installed {
        border-color: var(--text-muted);
        background: rgba(255,255,255,0.05);
        color: var(--text-dim);
        cursor: default;
      }
      .settings-save-btn {
        padding: 6px 18px;
        font-size: 0.74rem;
        border-radius: 6px;
        border: 1px solid var(--green);
        background: rgba(157,180,172,0.12);
        color: var(--green);
        cursor: pointer;
        transition: background 0.15s, color 0.15s;
      }
      .settings-save-btn:hover { background: rgba(157,180,172,0.25); }
      .settings-save-btn:active { background: rgba(157,180,172,0.35); }
      .settings-save-msg {
        font-size: 0.72rem;
        color: var(--green);
        margin-left: 8px;
        opacity: 0;
        transition: opacity 0.2s;
      }
      .settings-save-msg.show { opacity: 1; }

      /* Custom dropdown (replaces native <select> — broken in transparent WebView2 windows) */
      .custom-select {
        position: relative;
        flex: 1;
        max-width: 220px;
        height: 26px;
        box-sizing: border-box;
        display: flex;
        align-items: center;
        user-select: none;
      }
      .custom-select-trigger {
        width: 100%;
        height: 26px;
        box-sizing: border-box;
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 6px;
        padding: 5px 8px;
        font-size: 0.73rem;
        font-family: inherit;
        background: rgba(0,0,0,0.25);
        border: 1px solid var(--border-glass);
        border-radius: 6px;
        color: var(--text);
        cursor: pointer;
        outline: none;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
      }
      .custom-select-trigger:hover { border-color: var(--accent-border); }
      .custom-select-arrow {
        flex-shrink: 0;
        width: 10px;
        height: 10px;
        transition: transform 0.15s;
      }
      .custom-select.open .custom-select-arrow { transform: rotate(180deg); }
      .custom-select-dropdown {
        position: absolute;
        top: 100%;
        left: 0;
        right: 0;
        max-height: 240px;
        overflow-y: auto;
        overscroll-behavior: contain; /* 滚轮不穿透到外层列表 */
        background: rgba(24,24,37,0.97);
        backdrop-filter: blur(20px);
        -webkit-backdrop-filter: blur(20px);
        border: 1px solid var(--border-glass);
        border-radius: 6px;
        z-index: 999;
        display: none;
        margin-top: 0;
        box-shadow: 0 4px 16px rgba(0,0,0,0.4);
        /* Show a thin scrollbar so users can tell long option lists scroll
           (previously hidden — "dropdown can't scroll" reports) */
        scrollbar-width: thin; /* Firefox */
        scrollbar-color: rgba(255,255,255,0.25) transparent;
        -ms-overflow-style: auto; /* IE/Edge */
      }
      .custom-select-dropdown::-webkit-scrollbar { width: 6px; }
      .custom-select-dropdown::-webkit-scrollbar-track { background: transparent; }
      .custom-select-dropdown::-webkit-scrollbar-thumb {
        background: rgba(255,255,255,0.2);
        border-radius: 3px;
      }
      .custom-select.open .custom-select-dropdown { display: block; }
      .custom-select-option {
        padding: 6px 10px;
        font-size: 0.73rem;
        color: var(--text-dim);
        cursor: pointer;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
      }
      .custom-select-option:hover { background: var(--accent-bg); color: var(--text); }
      .custom-select-option.selected { color: var(--accent); font-weight: 600; }
      .custom-model-input {
        width: 100%;
        box-sizing: border-box;
        padding: 6px 10px;
        font-size: 0.73rem;
        color: var(--text);
        background: rgba(0,0,0,0.35);
        border: 1px solid var(--accent-border);
        border-radius: 4px;
        outline: none;
        caret-color: var(--accent);
      }
      .custom-model-edit {
        display: flex;
        gap: 4px;
        padding: 4px;
        align-items: center;
      }
      .custom-model-edit .custom-model-input { flex: 1; }
      .custom-model-ok {
        flex-shrink: 0;
        width: 26px;
        height: 26px;
        font-size: 0.85rem;
        line-height: 1;
        color: var(--text);
        background: rgba(192,160,160,0.18);
        border: 1px solid var(--accent-border);
        border-radius: 4px;
        cursor: pointer;
      }
      .custom-model-ok:hover { background: rgba(192,160,160,0.35); }
      .cs-del {
        display: inline-block;
        margin-left: 6px;
        padding: 0 3px;
        font-size: 0.7rem;
        color: var(--text-dim);
        border-radius: 3px;
        cursor: pointer;
        opacity: 0.6;
      }
      .custom-select-option:hover .cs-del { opacity: 1; color: var(--red, #c08a8a); }
      .cs-del:hover { background: rgba(192,138,138,0.2); }
      /* Detached (expanded) mode: let settings fill the whole window so no
         blank translucent area remains below the content. */
      #app.detached .plugin-result {
        height: 100%;
        box-sizing: border-box;
        padding: 8px 12px;
      }
      #app.detached .plugin-result > .settings-layout {
        height: 100%;
      }
      /* Skill store list */
      .settings-skill-list { max-height: 220px; overflow-y: auto; }
      .settings-skill-open {
        padding: 3px 10px;
        font-size: 0.68rem;
        border-radius: 4px;
        border: 1px solid var(--accent-border);
        background: var(--accent-bg);
        color: var(--accent);
        cursor: pointer;
        flex-shrink: 0;
        transition: background 0.1s;
      }
      .settings-skill-open:hover { background: rgba(192,160,160,0.2); }
      .settings-skill-del {
        padding: 3px 10px;
        font-size: 0.68rem;
        border-radius: 4px;
        border: 1px solid var(--border-glass);
        background: none;
        color: var(--text-dim);
        cursor: pointer;
        flex-shrink: 0;
        transition: background 0.1s, color 0.1s;
      }
      .settings-skill-del:hover { color: var(--red); background: rgba(192,138,138,0.1); }
      /* 印象派按钮背景清理（与全局 styles.css 一致：背景全透明，无扫笔/光效） */
      .settings-tool-btn, .settings-skill-open, .settings-install-btn,
      .settings-save-btn, .custom-model-ok, .settings-skill-del {
        background-image: none;
        box-shadow: none;
      }
      .settings-install-btn.installed { background-image: none; box-shadow: none; }
    `;
    document.head.appendChild(style);
  }

  // ── Sidebar category switching ───────────────────────────────
  const sidebarItems = container.querySelectorAll(".settings-sidebar-item");
  const panes = container.querySelectorAll(".settings-pane");

  sidebarItems.forEach(item => {
    item.addEventListener("click", () => {
      const cat = (item as HTMLElement).dataset.cat!;
      activeSettingsCategory = cat;
      sidebarItems.forEach(s => s.classList.remove("active"));
      item.classList.add("active");
      panes.forEach(p => p.classList.remove("active"));
      const target = container.querySelector(`[data-pane="${cat}"]`);
      if (target) target.classList.add("active");
    });
  });

  // Restore last active category (or default to "general")
  const restoreCat = activeSettingsCategory;
  const restoreItem = container.querySelector(`.settings-sidebar-item[data-cat="${restoreCat}"]`);
  const restorePane = container.querySelector(`[data-pane="${restoreCat}"]`);
  if (restoreItem) restoreItem.classList.add("active");
  else if (sidebarItems.length > 0) { sidebarItems[0].classList.add("active"); activeSettingsCategory = "general"; }
  if (restorePane) restorePane.classList.add("active");
  else if (panes.length > 0) panes[0].classList.add("active");

  // ── Hotkey recording ─────────────────────────────────────────
  const hotkeyBtn = container.querySelector("#settings-hotkey-btn") as HTMLElement | null;
  if (hotkeyBtn) {
    console.log("[lunac settings] hotkeyBtn FOUND, setting up JS recording listeners...");
    // Clean up previous listeners to avoid memory leaks from repeated open/close
    if (_onRecordingCaptured) {
      window.removeEventListener("lunac-recording-captured", _onRecordingCaptured);
      _onRecordingCaptured = null;
    }
    if (_onRecordingCancelled) {
      window.removeEventListener("lunac-recording-cancelled", _onRecordingCancelled);
      _onRecordingCancelled = null;
    }
    if (_clickOutsideHandler) {
      document.removeEventListener("click", _clickOutsideHandler);
      _clickOutsideHandler = null;
    }
    _unlistenHotkeyRecorded?.();
    _unlistenHotkeyRecorded = null;

    // Capture in local const so nested functions get narrowed type (TS18047 fix)
    const hkBtn = hotkeyBtn;

    // Shared handler: persist recorded combo + update UI
    function recordCombo(combo: string) {
      console.log("[lunac settings] RECORDING captured:", combo);
      (window as any).__lunac_recording_active = false;
      if (hkBtn.classList.contains("recording")) {
        hkBtn.textContent = combo;
        hkBtn.classList.remove("recording");
        hkBtn.setAttribute("data-original", combo);
        hkBtn.title = combo;
        invoke("set_hotkey_combo", { combo })
          .then(() => {
            console.log("[lunac settings] Hotkey saved:", combo);
            (window as any).__lunac_hotkey_is_alt_space = combo === "Alt+Space";
            // 同步刷新状态栏热键提示（Rust 已重装后端：优先 RegisterHotKey，失败才装钩子）
            (window as any).__lunac_refresh_hotkey_hint?.(combo);
          })
          .catch((err: any) => console.error("[lunac settings] FAILED to save hotkey:", err));
        invoke("set_recording_state", { recording: false }).catch(() => {});
      }
    }

    // ── JS-level recording: DOM custom event (for most keys) ─────
    _onRecordingCaptured = (e: Event) => {
      const { combo } = (e as CustomEvent).detail;
      recordCombo(combo);
    };
    window.addEventListener("lunac-recording-captured", _onRecordingCaptured);

    // ── Rust-level recording: Tauri event (for Alt+Space via WndProc) ──
    // Chromium swallows Space keydown in its internal menu mode when Alt is
    // held, so JS keydown never fires. The WndProc subclass directly emits
    // this Tauri event when RECORDING is true and SC_KEYMENU lParam=0x20 fires.
    listen<{ vk: number; modifiers: number; combo: string }>(
      "lunac-hotkey-recorded",
      (event) => { recordCombo(event.payload.combo); }
    ).then(unlisten => { _unlistenHotkeyRecorded = unlisten; });

    _onRecordingCancelled = () => {
      if (hotkeyBtn.classList.contains("recording")) {
        hotkeyBtn.classList.remove("recording");
        hotkeyBtn.textContent = hotkeyBtn.getAttribute("data-original") || "Alt+Space";
        invoke("set_recording_state", { recording: false }).catch(() => {});
      }
    };
    window.addEventListener("lunac-recording-cancelled", _onRecordingCancelled);

    hotkeyBtn.addEventListener("click", async () => {
      // Toggle recording off
      if (hotkeyBtn.classList.contains("recording")) {
        console.log("[lunac settings] Toggle recording OFF");
        (window as any).__lunac_recording_active = false;
        await invoke("set_recording_state", { recording: false }).catch(() => {});
        hotkeyBtn.classList.remove("recording");
        hotkeyBtn.textContent = hotkeyBtn.getAttribute("data-original") || "Alt+Space";
        return;
      }
      // Toggle recording on — sync Rust RECORDING FIRST to prevent LL hook from
      // swallowing the Space key before JS can capture it (race condition fix).
      console.log("[lunac settings] Toggle recording ON (syncing Rust first)...");
      const originalText = hotkeyBtn.textContent || "Alt+Space";
      hotkeyBtn.setAttribute("data-original", originalText);
      hotkeyBtn.classList.add("recording");
      hotkeyBtn.textContent = t("settings.hotkey_prompt");
      try {
        await invoke("set_recording_state", { recording: true });
        console.log("[lunac settings] set_recording_state(true) SUCCESS — LL hook will now pass keys through");
      } catch (e) {
        console.error("[lunac settings] set_recording_state(true) FAILED:", e, "— Alt+Space recording WILL NOT work!");
        hotkeyBtn.classList.remove("recording");
        hotkeyBtn.textContent = originalText;
        return;
      }
      // Only set JS flag AFTER Rust RECORDING is true — prevents race
      (window as any).__lunac_recording_active = true;
    });

    // Click outside hotkey button → cancel recording
    _clickOutsideHandler = (e: Event) => {
      if (!hotkeyBtn.classList.contains("recording")) return;
      if (!hotkeyBtn.contains(e.target as Node)) {
        hotkeyBtn.classList.remove("recording");
        hotkeyBtn.textContent = hotkeyBtn.getAttribute("data-original") || "Alt+Space";
        (window as any).__lunac_recording_active = false;
        invoke("set_recording_state", { recording: false }).catch(() => {});
      }
    };
    document.addEventListener("click", _clickOutsideHandler);
  } else {
    console.error("[lunac settings] hotkeyBtn NOT FOUND! Cannot set up recording. Searching for #settings-hotkey-btn inside:", container.id, "innerHTML length:", container.innerHTML.length);
  }

  // ── Auto-start toggle ────────────────────────────────────────
  const autoStartCheck = container.querySelector("#settings-autostart") as HTMLInputElement | null;
  if (autoStartCheck) {
    autoStartCheck.addEventListener("change", async () => {
      try {
        await invoke("set_auto_start", { enabled: autoStartCheck.checked });
      } catch {
        // Revert checkbox on failure to keep UI consistent
        autoStartCheck.checked = !autoStartCheck.checked;
      }
    });
  }

  // ── 自定义背景（需求：透明 + 毛玻璃，无默认图）────────────────
  // 选择图片 → convertFileSrc 转 asset URL → localStorage 持久化 →
  // 调用 main.ts 暴露的 __lunac_apply_bg 即时更新 #app-bg-image。
  const bgApply = container.querySelector("#settings-bg-apply") as HTMLElement | null;
  const bgClear = container.querySelector("#settings-bg-clear") as HTMLElement | null;
  if (bgApply) {
    bgApply.addEventListener("click", async () => {
      try {
        const { open } = await import("@tauri-apps/plugin-dialog");
        const picked = await open({
          multiple: false,
          title: t("settings.background_apply"),
          filters: [{ name: "Image", extensions: ["png", "jpg", "jpeg", "webp", "bmp", "gif"] }],
        });
        if (typeof picked === "string" && picked) {
          const url = convertFileSrc(picked);
          try { localStorage.setItem("lunac-bg-image", url); } catch {}
          (window as any).__lunac_apply_bg?.(url);
        }
      } catch (e) {
        console.error("[lunac settings] background apply failed:", e);
      }
    });
  }
  if (bgClear) {
    bgClear.addEventListener("click", () => {
      try { localStorage.removeItem("lunac-bg-image"); } catch {}
      (window as any).__lunac_apply_bg?.(null);
    });
  }

  // ── AI Provider ──────────────────────────────────────────────
  const providerDD = container.querySelector("#settings-provider") as HTMLElement | null;
  // let：供应商切换时 reloadModelSelector 会用 replaceWith 换成新节点并回写本引用，
  // 否则二次切换仍操作已被移除的旧节点（parentElement 为 null → 模型下拉卡死不更新）。
  let modelDD = container.querySelector("#settings-model") as HTMLElement | null;
  const searchEngineDD = container.querySelector("#settings-search-engine") as HTMLElement | null;
  const baseUrlInput = container.querySelector("#settings-baseurl") as HTMLInputElement | null;
  const apiKeyInput = container.querySelector("#settings-apikey") as HTMLInputElement | null;

  // ── Custom dropdown setup ────────────────────────────────────
  function setupCustomDropdown(dd: HTMLElement, onChange: (value: string) => void) {
    const trigger = dd.querySelector(".custom-select-trigger") as HTMLElement | null;
    const dropdown = dd.querySelector(".custom-select-dropdown") as HTMLElement | null;
    const label = dd.querySelector(".custom-select-label") as HTMLElement | null;
    if (!trigger || !dropdown || !label) return;

    // Helper: allow dropdown to overflow parent containers
    const setOverflowVisible = (v: boolean) => setDropdownOverflow(container, v);

    /**
     * Position the dropdown absolutely within .custom-select (position:relative)
     * so it sits flush below the trigger. IMPORTANT: position:fixed is broken
     * here — #results-container has `backdrop-filter`, which per spec makes it
     * the containing block for fixed descendants, so a viewport-coordinate top
     * lands ~searchbar-height BELOW the trigger (需求2 复测: 52px gap). Absolute
     * is immune to that offset. Height is still capped to fit the window, and
     * ancestors get overflow:visible via .sel-open so nothing is clipped.
     */
    function positionDropdown() {
      const dropEl = dd.querySelector(".custom-select-dropdown") as HTMLElement | null;
      const trigEl = dd.querySelector(".custom-select-trigger") as HTMLElement | null;
      if (!dropEl || !trigEl) return;
      const trigRect = trigEl.getBoundingClientRect();
      const MARGIN = 8;
      // 目标高度：约 4 行可见（紧凑，不把面板撑满窗口），
      // 更多项（如自定义模型）通过列表内部滚动条拉动查看。
      const TARGET_H = 120;
      const VH = window.innerHeight;
      const spaceBelow = VH - trigRect.bottom - MARGIN;
      const spaceAbove = trigRect.top - MARGIN;
      // 下方放得下就朝下展开；下方不够（如内嵌小窗底部）则翻到上方；
      // 高度固定 TARGET_H，极小窗口下保底 60px，始终可内部滚动。
      const openUp = spaceBelow < TARGET_H && spaceAbove > spaceBelow;
      const sideSpace = openUp ? spaceAbove : spaceBelow;
      const maxH = Math.max(60, Math.min(TARGET_H, sideSpace));
      dropEl.style.overflowY = "auto";
      dropEl.style.overscrollBehavior = "contain"; // 滚轮只滚本列表，不穿透外层
      dropEl.style.position = "absolute";
      dropEl.style.left = "0";
      dropEl.style.width = "100%";
      dropEl.style.maxHeight = maxH + "px";
      if (openUp) {
        dropEl.style.top = "auto";
        dropEl.style.bottom = "calc(100% + 4px)";
        dropEl.style.marginTop = "0";
        dropEl.style.marginBottom = "0";
      } else {
        dropEl.style.top = "calc(100% + 4px)";
        dropEl.style.bottom = "auto";
        dropEl.style.marginTop = "0";
        dropEl.style.marginBottom = "0";
      }
    }

    // Toggle open/close
    trigger.addEventListener("click", (e: Event) => {
      e.preventDefault(); e.stopPropagation();
      const wasOpen = dd.classList.contains("open");
      // Close all other custom selects first
      container.querySelectorAll(".custom-select.open").forEach(el => {
        if (el !== dd) el.classList.remove("open");
      });
      if (wasOpen) {
        dd.classList.remove("open");
        setOverflowVisible(false);
      } else {
        dd.classList.add("open");
        setOverflowVisible(true);
        // position:fixed + dynamic height — never split/clipped on small windows
        positionDropdown();
        // Re-position if the window is resized while open
        window.addEventListener("resize", positionDropdown, { once: true });
      }
    });

    // Select option (and handle per-option delete buttons)
    dropdown.addEventListener("click", (e: Event) => {
      const target = e.target as HTMLElement;
      // ── Delete button (×): remove the option (custom → also persisted) ──
      const delBtn = target.closest(".cs-del") as HTMLElement | null;
      if (delBtn) {
        e.preventDefault(); e.stopPropagation();
        const opt = delBtn.closest(".custom-select-option") as HTMLElement | null;
        if (!opt) return;
        const value = opt.getAttribute("data-value") || "";
        const tag = opt.getAttribute("data-tag") || "builtin";
        const isProvider = dd.id === "settings-provider";
        // Custom entries persist in localStorage; built-ins return next time the panel opens.
        if (tag === "custom") {
          try {
            if (isProvider) {
              const arr: string[] = JSON.parse(localStorage.getItem("lunac-custom-providers") || "[]");
              const i = arr.indexOf(value);
              if (i >= 0) { arr.splice(i, 1); localStorage.setItem("lunac-custom-providers", JSON.stringify(arr)); }
            } else {
              const prov = (container.querySelector("#settings-provider .custom-select-option.selected")?.getAttribute("data-value")) || "";
              const map = JSON.parse(localStorage.getItem("lunac-custom-models") || "{}");
              const arr: string[] = (map && map[prov]) || [];
              const i = arr.indexOf(value);
              if (i >= 0) { arr.splice(i, 1); map[prov] = arr; localStorage.setItem("lunac-custom-models", JSON.stringify(map)); }
            }
          } catch {}
        } else if (tag === "builtin") {
          // Built-in entries: persist a hidden-list so the deletion survives panel reopen
          try {
            if (isProvider) {
              const arr: string[] = JSON.parse(localStorage.getItem("lunac-hidden-providers") || "[]");
              if (!arr.includes(value)) { arr.push(value); localStorage.setItem("lunac-hidden-providers", JSON.stringify(arr)); }
            } else {
              const prov = (container.querySelector("#settings-provider .custom-select-option.selected")?.getAttribute("data-value")) || "";
              const map = JSON.parse(localStorage.getItem("lunac-hidden-models") || "{}");
              const arr: string[] = (map && map[prov]) || [];
              if (!arr.includes(value)) { arr.push(value); map[prov] = arr; localStorage.setItem("lunac-hidden-models", JSON.stringify(map)); }
            }
          } catch {}
        }
        // If the removed option was selected, fall back to the first remaining one
        if (opt.classList.contains("selected")) {
          const remaining = dropdown.querySelectorAll(".custom-select-option:not([data-value=\"__custom__\"])");
          const first = remaining[0] as HTMLElement | undefined;
          if (first) {
            dropdown.querySelectorAll(".custom-select-option").forEach(o => o.classList.remove("selected"));
            first.classList.add("selected");
            label.textContent = first.textContent?.replace(/\s*×$/, "").trim() || "";
            if (isProvider) onChange(first.getAttribute("data-value") || "");
          } else {
            label.textContent = "";
          }
        }
        opt.remove();
        return;
      }
      const opt = target.closest(".custom-select-option") as HTMLElement | null;
      if (!opt) return;
      e.preventDefault(); e.stopPropagation();
      const value = opt.getAttribute("data-value") || "";
      if (value === "__custom__") {
        // WebView2 does NOT support window.prompt() — use an inline input
        promptCustomModelInput(dd, label, (val) => {
          commitCustomOption(dd, val, label);
          onChange(val);
        }, "", setOverflowVisible);
        return;
      }
      // Update visual selection
      dropdown.querySelectorAll(".custom-select-option").forEach(o => o.classList.remove("selected"));
      opt.classList.add("selected");
      label.textContent = opt.textContent?.replace(/\s*×$/, "").trim() || "";
      dd.classList.remove("open");
      setOverflowVisible(false);
      onChange(value);
    });

    // 长列表（如模型多、含自定义项时）强制内部滚动：
    // 项超出可视区后滚轮在 WebView2 下可能被外层 #results-list 吞掉，
    // 这里手动滚自己，保证永远能滚到列表底部的自定义项/新增项。
    dropdown.addEventListener("wheel", (e: WheelEvent) => {
      const canUp = dropdown.scrollTop > 0;
      const canDown = dropdown.scrollTop + dropdown.clientHeight < dropdown.scrollHeight - 1;
      if ((e.deltaY < 0 && canUp) || (e.deltaY > 0 && canDown)) {
        e.preventDefault();
        dropdown.scrollTop += e.deltaY;
      }
    }, { passive: false });
  }

  // Close dropdowns on outside click
  document.addEventListener("click", (e: Event) => {
    const target = e.target as HTMLElement;
    let anyOpen = false;
    container.querySelectorAll(".custom-select.open").forEach(dd => {
      if (dd.contains(target)) {
        anyOpen = true;
      } else {
        dd.classList.remove("open");
      }
    });
    if (!anyOpen) setDropdownOverflow(container, false);
  });

  // Provider dropdown
  if (providerDD) {
    setupCustomDropdown(providerDD, (value) => {
      const preset = PROVIDER_PRESETS[value];
      if (baseUrlInput) {
        // 内置预设填默认地址；自定义供应商（不在预设表）清空等待填写
        baseUrlInput.value = preset?.default_url ?? "";
      }
      // Rebuild model selector for this provider
      const oldModel = modelDD?.querySelector(".custom-select-label")?.textContent || "";
      reloadModelSelector(value, oldModel);
    });
  }

  // Model dropdown init + inline edit. 供应商切换会用新节点 replaceWith 重建，
  // 每个新节点都必须重新绑定（下拉逻辑 + 双击标签编辑自定义模型）。
  const setupModelDropdown = (dd: HTMLElement) => {
    setupCustomDropdown(dd, () => {}); // onChange is no-op, save button handles persistence
    // Double-click the selected label to type a custom model
    const trigger = dd.querySelector(".custom-select-trigger") as HTMLElement | null;
    const label = dd.querySelector(".custom-select-label") as HTMLElement | null;
    if (trigger && label) {
      trigger.addEventListener("dblclick", () => {
        const oldVal = label.textContent || "";
        // WebView2 does NOT support window.prompt() — use an inline input
        promptCustomModelInput(dd, label, (val) => {
          commitCustomOption(dd, val, label);
        }, oldVal, (v) => setDropdownOverflow(container, v));
      });
    }
  };
  if (modelDD) setupModelDropdown(modelDD);

  // ── Save button (manual, replaces auto-save on blur/change) ────
  const saveAiBtn = container.querySelector("#settings-save-ai-btn") as HTMLButtonElement | null;
  const saveMsg = container.querySelector("#settings-save-msg") as HTMLElement | null;
  if (saveAiBtn) {
    saveAiBtn.addEventListener("click", async () => {
      saveAiBtn.disabled = true;
      saveAiBtn.textContent = t("settings.saving");
      try {
        await saveAIConfig();
        if (saveMsg) {
          saveMsg.textContent = t("settings.saved_ok");
          saveMsg.classList.add("show");
          setTimeout(() => saveMsg.classList.remove("show"), 2000);
        }
      } catch {
        if (saveMsg) {
          saveMsg.textContent = t("settings.saved_fail");
          saveMsg.style.color = "var(--red)";
          saveMsg.classList.add("show");
          setTimeout(() => {
            saveMsg.classList.remove("show");
            saveMsg.style.color = "";
          }, 2000);
        }
      }
      saveAiBtn.disabled = false;
      saveAiBtn.textContent = t("settings.save");
    });
  }

  /** Rebuild the model selector when provider changes (custom dropdown).
   *  切换供应商时不把上一供应商的模型名带过来 —— 旧模型仅当属于新供应商的
   *  预设或已保存自定义模型时才保留，否则回落到该供应商的默认模型，
   *  不再出现跨供应商模型被标成“(自定义)”并占据选中位的问题。 */
  function reloadModelSelector(provider: string, oldModel: string) {
    const row = modelDD?.parentElement;
    if (!row || !modelDD) return;
    const preset = PROVIDER_PRESETS[provider];
    const suggestions = MODEL_SUGGESTIONS[provider] || [];
    // 该供应商下已保存的自定义模型（切换后仍在列表中展示）
    let savedCustoms: string[] = [];
    try {
      const map = JSON.parse(localStorage.getItem("lunac-custom-models") || "{}");
      savedCustoms = ((map && map[provider]) || []).filter((m: string) => m && !suggestions.includes(m));
    } catch {}
    const selected = (oldModel && (suggestions.includes(oldModel) || savedCustoms.includes(oldModel)))
      ? oldModel
      : (preset?.default_model || "");
    // Filter out built-in models the user deleted (persisted hidden-list)
    let hiddenModels: string[] = [];
    try {
      const hm = JSON.parse(localStorage.getItem("lunac-hidden-models") || "{}");
      hiddenModels = ((hm && hm[provider]) || []).filter((m: string) => m !== selected);
    } catch {}
    const modelList = suggestions.filter(m => !hiddenModels.includes(m));
    const hasCustom = !!selected && !modelList.includes(selected);

    const optsHtml = modelList.map(m =>
      `<div class="custom-select-option${m === selected ? ' selected' : ''}" data-value="${esc(m)}" data-tag="builtin">${esc(m)}<span class="cs-del" title="×">×</span></div>`
    ).join("");
    const savedHtml = savedCustoms.filter(m => m !== selected).map(m =>
      `<div class="custom-select-option" data-value="${esc(m)}" data-tag="custom">${esc(m)} (${t("settings.custom_model")})<span class="cs-del" title="×">×</span></div>`
    ).join("");
    const customHtml = hasCustom
      ? `<div class="custom-select-option selected" data-value="${esc(selected)}" data-tag="custom">${esc(selected)} (${t("settings.custom_model")})<span class="cs-del" title="×">×</span></div>`
      : "";
    const newHtml = `
      <div class="custom-select" id="settings-model">
        <button class="custom-select-trigger" type="button">
          <span class="custom-select-label">${esc(selected || t("settings.select_placeholder"))}</span>
          <svg class="custom-select-arrow" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="6 9 12 15 18 9"/></svg>
        </button>
        <div class="custom-select-dropdown">
          ${optsHtml}
          ${savedHtml}
          ${customHtml}
          <div class="custom-select-option" data-value="__custom__">${t("settings.custom_placeholder")}</div>
        </div>
      </div>`;
    const tmp = document.createElement("div");
    tmp.innerHTML = newHtml;
    const newDD = tmp.firstElementChild as HTMLElement;
    modelDD.replaceWith(newDD);
    // 关键：回写外层引用，否则下一次供应商切换拿到的还是被移除的旧节点，
    // reloadModelSelector 会因 parentElement==null 提前 return → 模型下拉卡死。
    modelDD = newDD;
    // Re-setup listeners on the new dropdown (下拉 + 双击编辑)
    setupModelDropdown(newDD);
  }

  async function saveAIConfig() {
    try {
      const providerOpt = container.querySelector("#settings-provider .custom-select-option.selected");
      const provider = providerOpt?.getAttribute("data-value") || "openai";
      const modelOpt = container.querySelector("#settings-model .custom-select-option.selected");
      // 只认真正选中的选项（含自定义输入提交后生成的选中项）；未选择时为空串，
      // 由后端 set_ai_config 校验拒绝，避免把下拉占位符文本当模型名保存。
      const model = modelOpt?.getAttribute("data-value") || "";
      const baseUrl = (container.querySelector("#settings-baseurl") as HTMLInputElement)?.value || "";
      const apiKey = (container.querySelector("#settings-apikey") as HTMLInputElement)?.value || "";
      // Preserve existing anthropic_url if set (don't overwrite with empty)
      let anthropicUrl = "";
      try {
        const cur = await invoke<{ anthropic_url?: string }>("get_ai_config");
        anthropicUrl = cur.anthropic_url || "";
      } catch {}
      await invoke("set_ai_config", {
        provider,
        url: baseUrl,
        key: apiKey,
        model,
        anthropic_url: anthropicUrl,
      });
      // Persist to localStorage so config survives restart
      try {
        localStorage.setItem("lunac-ai-config", JSON.stringify({
          provider, url: baseUrl, key: apiKey, model, anthropic_url: anthropicUrl,
        }));
      } catch {}
    } catch (e) {
      console.warn("[lunac] saveAIConfig failed:", e);
    }
  }

  // ── Search engine save ──────────────────────────────────────
  const saveSearchBtn = container.querySelector("#settings-save-search-btn") as HTMLButtonElement | null;
  const saveSearchMsg = container.querySelector("#settings-save-search-msg") as HTMLElement | null;
  if (saveSearchBtn && saveSearchMsg) {
    saveSearchBtn.addEventListener("click", () => {
      try {
        const opt = container.querySelector("#settings-search-engine .custom-select-option.selected");
        const engine = opt?.getAttribute("data-value") || "google";
        localStorage.setItem("lunac-search-engine", engine);
        // Notify web-search plugin to refresh (via custom event)
        window.dispatchEvent(new CustomEvent("lunac-search-engine-changed", { detail: engine }));
        saveSearchMsg.textContent = t("settings.saved");
        saveSearchMsg.classList.add("show");
        setTimeout(() => { saveSearchMsg.textContent = ""; saveSearchMsg.classList.remove("show"); }, 1500);
      } catch (e) {
        console.warn("[lunac] saveSearchEngine failed:", e);
      }
    });
  }

  // Search engine dropdown
  if (searchEngineDD) {
    setupCustomDropdown(searchEngineDD, () => {}); // onChange is no-op, save button handles persistence
  }

  // ── Language selector ─────────────────────────────────────────
  const langDD = container.querySelector("#settings-language") as HTMLElement | null;
  if (langDD) {
    setupCustomDropdown(langDD, async (value) => {
      try {
        if (value === "system") {
          await resetToSystemLanguage();
        } else {
          setLanguage(value);
        }
      } catch (e) {
        console.warn("[lunac] applyLanguage failed:", e);
      }
      // Re-render the settings pane (and any open UI) in the new language.
      (window as any).__lunac_refresh_plugin?.("settings");
      (window as any).__lunac_apply_language?.();
    });
  }

  // ── Open Tool Editor ─────────────────────────────────────────
  const openToolsBtn = container.querySelector("#settings-open-tools");
  if (openToolsBtn) {
    openToolsBtn.addEventListener("click", () => {
      (window as any).__lunac_execute_tool_editor?.();
    });
  }

  // ── Install from URL ─────────────────────────────────────────
  const installUrlBtn = container.querySelector("#settings-install-url");
  const toolUrlInput = container.querySelector("#settings-tool-url") as HTMLInputElement | null;
  const installMsg = container.querySelector("#settings-install-msg") as HTMLElement | null;
  if (installUrlBtn && toolUrlInput && installMsg) {
    installUrlBtn.addEventListener("click", async () => {
      const url = toolUrlInput.value.trim();
      if (!url) { showMsg(t("settings.enter_url")); return; }
      try {
        new URL(url); // validate URL format
      } catch { showMsg(t("settings.invalid_url")); return; }

      installMsg.style.display = "block";
      installMsg.style.color = "var(--yellow)";
      installMsg.textContent = t("settings.downloading");
      try {
        const fn = await invoke<string>("download_tool_from_url", { url });
        showMsg(t("settings.installed_ok", { name: fn }), "var(--green)");
        toolUrlInput.value = "";
        // Reload settings to refresh tool list
        const evt = new CustomEvent("lunac-reload-settings");
        document.dispatchEvent(evt);
      } catch (e: any) {
        showMsg(t("settings.install_error", { err: String(e) }), "var(--red)");
      }
    });
  }
  function showMsg(text: string, color?: string) {
    if (!installMsg) return;
    installMsg.style.display = "block";
    if (color) installMsg.style.color = color;
    installMsg.textContent = text;
    setTimeout(() => { if (installMsg) installMsg.style.display = "none"; }, 4000);
  }

  // ── Community tool install buttons ───────────────────────────
  container.querySelectorAll(".settings-install-btn[data-cmd]").forEach(btn => {
    btn.addEventListener("click", async () => {
      const name = (btn as HTMLElement).dataset.name!;
      const cmd = (btn as HTMLElement).dataset.cmd!;
      const desc = (btn as HTMLElement).dataset.desc!;
      const toolJson = JSON.stringify({
        name,
        description: desc || name,
        inputSchema: { type: "object", properties: {}, required: [] },
        handler: { type: "shell", command: cmd },
      }, null, 2);

      try {
        const fn = name + ".json";
        await invoke("save_tool_file", { filename: fn, content: toolJson });
        (btn as HTMLElement).textContent = t("settings.installed_btn");
        (btn as HTMLElement).classList.add("installed");
        (btn as HTMLElement).setAttribute("disabled", "true");
        // Reload settings
        const evt = new CustomEvent("lunac-reload-settings");
        document.dispatchEvent(evt);
      } catch (e: any) {
        showMsg(t("settings.install_failed", { err: String(e) }), "var(--red)");
      }
    });
  });

  // ── Skill Store ──────────────────────────────────────────────
  const skillUrlInput = container.querySelector("#settings-skill-url") as HTMLInputElement | null;
  const skillAddBtn = container.querySelector("#settings-skill-add") as HTMLElement | null;
  const skillMsg = container.querySelector("#settings-skill-msg") as HTMLElement | null;
  if (skillMsg) {
    const showSkillMsg = (text: string, color?: string) => {
      skillMsg.style.display = "block";
      if (color) skillMsg.style.color = color;
      skillMsg.textContent = text;
      setTimeout(() => { skillMsg.style.display = "none"; }, 4000);
    };

    // (Re)render the "My Additions" list with open/remove bindings
    const renderCustomSkillList = () => {
      const listEl = container.querySelector("#settings-skill-custom");
      if (!listEl) return;
      const list = loadSkillSites();
      listEl.innerHTML = list.length === 0
        ? `<div class="settings-plugin-empty">${t("settings.skill_empty")}</div>`
        : list.map(s => skillEntryHtml(s.name, s.url, s.url, true)).join("");
      listEl.querySelectorAll(".settings-skill-open").forEach(btn => {
        btn.addEventListener("click", () => {
          const u = (btn as HTMLElement).dataset.url || "";
          if (u) open(u).catch(() => {});
        });
      });
      listEl.querySelectorAll(".settings-skill-del").forEach(btn => {
        btn.addEventListener("click", () => {
          const u = (btn as HTMLElement).dataset.url || "";
          const n = (btn as HTMLElement).dataset.name || u;
          saveSkillSites(loadSkillSites().filter(s => s.url !== u));
          renderCustomSkillList();
          showSkillMsg(t("settings.skill_removed", { name: n }), "var(--yellow)");
        });
      });
    };

    // Recommended entries: open in default browser
    container.querySelectorAll(".settings-skill-open[data-url]").forEach(btn => {
      btn.addEventListener("click", () => {
        const u = (btn as HTMLElement).dataset.url || "";
        if (u) open(u).catch(() => {});
      });
    });

    // ── 已安装技能：打开 / 编辑 / 删除 / URL 安装 / 新建 ──
    const installedListEl = container.querySelector("#settings-skill-installed") as HTMLElement | null;
    const installUrlInput = container.querySelector("#settings-skill-install-url") as HTMLInputElement | null;
    const installUrlBtn = container.querySelector("#settings-skill-install-url-btn") as HTMLButtonElement | null;
    const newSkillBtn = container.querySelector("#settings-skill-new-btn") as HTMLButtonElement | null;
    const opMsg = container.querySelector("#settings-skill-op-msg") as HTMLElement | null;
    const editorEl = container.querySelector("#settings-skill-editor") as HTMLElement | null;
    const editorText = container.querySelector("#settings-skill-editor-text") as HTMLTextAreaElement | null;
    const editorSave = container.querySelector("#settings-skill-editor-save") as HTMLButtonElement | null;
    const editorCancel = container.querySelector("#settings-skill-editor-cancel") as HTMLButtonElement | null;
    let skillEditorKey = ""; // "" = 新建（粘贴导入），否则为已有技能 key（覆盖保存）

    const showOp = (text: string, color?: string) => {
      if (!opMsg) return;
      opMsg.style.display = "block";
      if (color) opMsg.style.color = color;
      opMsg.textContent = text;
      setTimeout(() => { opMsg.style.display = "none"; opMsg.style.color = ""; }, 4000);
    };

    const openSkillEditor = (key: string, content: string) => {
      if (!editorEl || !editorText) return;
      skillEditorKey = key;
      editorText.value = content;
      editorEl.style.display = "block";
      editorText.focus();
    };
    const closeSkillEditor = () => { if (editorEl) editorEl.style.display = "none"; skillEditorKey = ""; };

    const bindInstalledRowActions = () => {
      // 打开技能目录（Explorer）
      installedListEl?.querySelectorAll<HTMLElement>(".settings-skill-open").forEach(btn => {
        btn.addEventListener("click", () => { const d = btn.dataset.dir || ""; if (d) open(d).catch(() => {}); });
      });
      // 编辑（读取 SKILL.md 全文 → 内联编辑器）
      installedListEl?.querySelectorAll<HTMLElement>(".settings-skill-edit").forEach(btn => {
        btn.addEventListener("click", async () => {
          const key = btn.dataset.key || "";
          try {
            const content = await invoke<string>("read_skill_file", { key });
            openSkillEditor(key, content);
          } catch (e: any) { showOp(String(e), "var(--red)"); }
        });
      });
      // 删除（两段式确认：WebView2 下原生 confirm 不可靠）
      installedListEl?.querySelectorAll<HTMLButtonElement>(".settings-skill-del-installed").forEach(btn => {
        btn.addEventListener("click", async () => {
          const key = btn.dataset.key || "";
          if (btn.dataset.armed !== "1") {
            btn.dataset.armed = "1";
            btn.textContent = t("settings.skill_del_confirm");
            setTimeout(() => { btn.dataset.armed = ""; btn.textContent = t("settings.skill_remove"); }, 3000);
            return;
          }
          try {
            await invoke("delete_skill", { key });
            await renderInstalledSkills();
            showOp(t("settings.skill_removed", { name: key }), "var(--yellow)");
          } catch (e: any) { showOp(String(e), "var(--red)"); }
        });
      });
    };

    const renderInstalledSkills = async () => {
      if (!installedListEl) return;
      try {
        const list: Array<{ name: string; description: string; key: string; dir: string }> =
          await invoke("list_installed_skills");
        installedListEl.innerHTML = list.length === 0
          ? `<div class="settings-plugin-empty">${t("settings.skills_none")}</div>`
          : list.map(s => installedSkillRowHtml(s)).join("");
        bindInstalledRowActions();
      } catch {
        installedListEl.innerHTML = `<div class="settings-plugin-empty">${t("settings.load_error")}</div>`;
      }
    };

    // URL 安装 raw SKILL.md
    const installSkillFromUrl = async () => {
      const url = (installUrlInput?.value || "").trim();
      if (!url) { showOp(t("settings.enter_url")); return; }
      try {
        const key = await invoke<string>("install_skill_from_url", { url });
        if (installUrlInput) installUrlInput.value = "";
        await renderInstalledSkills();
        showOp(t("settings.skill_installed_ok", { name: key }), "var(--green)");
      } catch (e: any) { showOp(String(e), "var(--red)"); }
    };
    if (installUrlBtn) installUrlBtn.addEventListener("click", installSkillFromUrl);
    if (installUrlInput) {
      installUrlInput.addEventListener("keydown", (e: KeyboardEvent) => {
        if (e.key === "Enter") { e.preventDefault(); installSkillFromUrl(); }
      });
    }

    // 新建（粘贴导入 SKILL.md 全文）
    if (newSkillBtn) newSkillBtn.addEventListener("click", () => openSkillEditor("", SKILL_TEMPLATE));

    // 编辑器：保存（覆盖已有 / 导入新建）、取消
    if (editorSave) {
      editorSave.addEventListener("click", async () => {
        const content = editorText?.value || "";
        if (!content.trim()) { showOp(t("settings.skill_editor_empty")); return; }
        try {
          if (skillEditorKey) {
            await invoke("save_skill_file", { key: skillEditorKey, content });
          } else {
            await invoke("import_skill_content", { content });
          }
          closeSkillEditor();
          await renderInstalledSkills();
          showOp(t("settings.saved_ok"), "var(--green)");
        } catch (e: any) { showOp(String(e), "var(--red)"); }
      });
    }
    if (editorCancel) editorCancel.addEventListener("click", closeSkillEditor);

    // buildSkillsPane 已内联渲染初始列表 → 这里绑定其行操作
    bindInstalledRowActions();

    const addSkillSite = () => {
      if (!skillUrlInput) return;
      let url = skillUrlInput.value.trim();
      if (!url) return;
      if (!/^https?:\/\//i.test(url)) url = "https://" + url;
      try { new URL(url); } catch { showSkillMsg(t("settings.skill_invalid_url"), "var(--red)"); return; }
      if (loadSkillSites().some(s => s.url === url) || SKILL_SITES.some(s => s.url === url)) {
        showSkillMsg(t("settings.skill_duplicate"), "var(--yellow)");
        return;
      }
      let name = url;
      try { name = new URL(url).hostname.replace(/^www\./, ""); } catch {}
      const list = loadSkillSites();
      list.push({ name, url });
      saveSkillSites(list);
      skillUrlInput.value = "";
      renderCustomSkillList();
      showSkillMsg(t("settings.skill_added", { name }), "var(--green)");
    };

    if (skillAddBtn) skillAddBtn.addEventListener("click", addSkillSite);
    if (skillUrlInput) {
      skillUrlInput.addEventListener("keydown", (e: KeyboardEvent) => {
        if (e.key === "Enter") { e.preventDefault(); addSkillSite(); }
      });
    }

    // Initial binding for the custom list rendered by buildSkillsPane
    renderCustomSkillList();
  }
}

// ── Plugin definition ────────────────────────────────────────────

/** Allow a custom-select dropdown to overflow its parent (open = true). */
function setDropdownOverflow(container: HTMLElement, v: boolean) {
  const layout = container.closest(".settings-layout");
  const content = container.closest(".settings-content");
  const list = container.closest("#results-list");
  const results = container.closest("#results-container");
  if (v) {
    layout?.classList.add("sel-open");
    content?.classList.add("sel-open");
    list?.classList.add("sel-open");
    results?.classList.add("sel-open");
  } else {
    layout?.classList.remove("sel-open");
    content?.classList.remove("sel-open");
    list?.classList.remove("sel-open");
    results?.classList.remove("sel-open");
  }
}

/**
 * Insert (or select) a custom value option in a custom dropdown.
 * The trailing "__custom__" entry is kept so users can keep adding new values.
 */
function commitCustomOption(dd: HTMLElement, val: string, label: HTMLElement) {
  const dropdown = dd.querySelector(".custom-select-dropdown");
  if (!dropdown || !val) return;
  dropdown.querySelectorAll(".custom-select-option").forEach(o => o.classList.remove("selected"));
  // Re-select an existing matching option (e.g. saved custom model) if present
  const existing = Array.from(dropdown.querySelectorAll(".custom-select-option"))
    .find(o => o.getAttribute("data-value") === val);
  if (existing) {
    existing.classList.add("selected");
  } else {
    const customOpt = document.createElement("div");
    customOpt.className = "custom-select-option selected";
    customOpt.setAttribute("data-value", val);
    customOpt.setAttribute("data-tag", "custom");
    customOpt.innerHTML = `${esc(val)}<span class="cs-del" title="×">×</span>`;
    const customEntry = Array.from(dropdown.querySelectorAll(".custom-select-option"))
      .find(o => o.getAttribute("data-value") === "__custom__");
    dropdown.insertBefore(customOpt, customEntry || null);
  }
  label.textContent = val;
  // Persist custom providers/models (点11) so they survive a panel reopen
  try {
    if (dd.id === "settings-provider") {
      const arr: string[] = JSON.parse(localStorage.getItem("lunac-custom-providers") || "[]");
      if (!arr.includes(val)) { arr.push(val); localStorage.setItem("lunac-custom-providers", JSON.stringify(arr)); }
    } else if (dd.id === "settings-model") {
      const prov = (document.querySelector("#settings-provider .custom-select-option.selected")?.getAttribute("data-value")) || "";
      const map = JSON.parse(localStorage.getItem("lunac-custom-models") || "{}");
      const arr: string[] = (map && map[prov]) || [];
      if (!arr.includes(val)) { arr.push(val); map[prov] = arr; localStorage.setItem("lunac-custom-models", JSON.stringify(map)); }
    }
  } catch {}
}

/**
 * Inline custom-model/provider input.
 * WebView2 不支持 window.prompt()，故用下拉内的内联编辑行。
 * 关键：不再用编辑器替换 dropdown 的整体 innerHTML —— 那会导致提交后
 * 原有预设选项全部丢失（「自定义后预设模型消失」）。这里把编辑行追加到
 * 列表末尾，提交/取消都只增删这一行，原有模型（分页）始终保留。
 */
function promptCustomModelInput(
  dd: HTMLElement,
  label: HTMLElement,
  onChange: (value: string) => void,
  initial: string,
  setOverflowVisible: (v: boolean) => void,
) {
  const dropdown = dd.querySelector(".custom-select-dropdown") as HTMLElement | null;
  if (!dropdown) return;
  // 清理可能遗留的旧编辑行
  dropdown.querySelectorAll(".custom-model-edit").forEach(el => el.remove());

  const editRow = document.createElement("div");
  editRow.className = "custom-model-edit";
  editRow.innerHTML = `
    <input type="text" class="custom-model-input" value="${esc(initial)}" placeholder="${t("settings.custom_model")}" autocomplete="off" />
    <button type="button" class="custom-model-ok" title="${t("settings.save")}">✓</button>
  `;
  // 追加到选项列表末尾（不动原有选项 DOM）
  dropdown.appendChild(editRow);
  dd.classList.add("open");
  setOverflowVisible(true);

  const input = editRow.querySelector("input");
  if (!input) return;
  input.focus();
  input.select();
  editRow.scrollIntoView({ block: "nearest" });

  const close = () => {
    editRow.remove(); // 只移除编辑行，保留所有模型选项
    dd.classList.remove("open");
    setOverflowVisible(false);
  };
  const commit = () => {
    const val = input.value.trim();
    if (val) {
      onChange(val); // __custom__/编辑路径在这里把自定义项插入列表并持久化
      label.textContent = val;
    }
    close();
  };
  input.addEventListener("keydown", (e: KeyboardEvent) => {
    if (e.key === "Enter") { e.preventDefault(); commit(); }
    else if (e.key === "Escape") { close(); }
  });
  input.addEventListener("blur", () => {
    // Delay so Enter/✓ commit() runs before this closes the editor.
    setTimeout(close, 100);
  });
  // ✓ button: keep focus (no blur) and commit
  const okBtn = editRow.querySelector(".custom-model-ok") as HTMLElement | null;
  if (okBtn) {
    okBtn.addEventListener("mousedown", (e: Event) => e.preventDefault());
    okBtn.addEventListener("click", (e: Event) => { e.preventDefault(); e.stopPropagation(); commit(); });
  }
}

export const settingsPlugin: Plugin = {
  id: "settings",
  name: "Settings",
  description: "Configure Lunac — hotkey, AI provider, plugins",
  keywords: ["settings", "设置", "config", "配置", "hotkey", "热键", "api", "key", "model", "plugin", "插件"],
  icon: "⚙️",

  async execute(_input: string) {
    let hotkey = "Ctrl+Alt+Space";
    let autoStart = false;
    // 未配置 provider 时由模型名反推其所属供应商，避免“OpenAI 供应商却带
    // DeepSeek 模型”等跨供应商脏数据；baseUrl 默认填该供应商文档地址。
    let provider = "";
    let baseUrl = "";
    let model = "";
    let apiKey = "";

    try {
      hotkey = await invoke<string>("get_hotkey_combo");
      // Sync JS fallback flag with loaded config
      (window as any).__lunac_hotkey_is_alt_space = hotkey === "Alt+Space";
      // 状态栏提示同步为实际热键
      (window as any).__lunac_refresh_hotkey_hint?.(hotkey);
    } catch {}

    try { autoStart = await invoke<boolean>("get_auto_start"); } catch {}

    try {
      const aiCfg = await invoke<{ provider: string; base_url: string; model: string; api_key: string }>("get_ai_config");
      provider = aiCfg.provider || "";
      baseUrl = aiCfg.base_url || "";
      model = aiCfg.model || "";
      apiKey = aiCfg.api_key || "";
    } catch {}

    // 未配置供应商（环境缺 AI_PROVIDER）：用模型名反推；仍无则回退 openai
    if (!provider) provider = inferProviderForModel(model, "openai");
    const preset = PROVIDER_PRESETS[provider];
    if (!baseUrl && preset) baseUrl = preset.default_url;
    if (!model && preset) model = preset.default_model;

    const generalPane = buildGeneralPane(hotkey, autoStart);
    const aiPane = buildAIPane(provider, baseUrl, model, apiKey);
    const pluginsPane = await buildPluginsPane();
    const searchPane = await buildSearchPane();
    const skillsPane = await buildSkillsPane();

    const html = `
      <div class="settings-layout">
        <div class="settings-sidebar">
          <div class="settings-sidebar-item" data-cat="general">
            <span>${t("settings.sidebar_general")}</span>
          </div>
          <div class="settings-sidebar-item" data-cat="ai">
            <span>${t("settings.sidebar_ai")}</span>
          </div>
          <div class="settings-sidebar-item" data-cat="search">
            <span>${t("settings.sidebar_search")}</span>
          </div>
          <div class="settings-sidebar-item" data-cat="skills">
            <span>${t("settings.sidebar_skills")}</span>
          </div>
          <div class="settings-sidebar-item" data-cat="plugins">
            <span>${t("settings.sidebar_plugins")}</span>
          </div>
        </div>
        <div class="settings-content">
          ${generalPane}
          ${aiPane}
          ${searchPane}
          ${skillsPane}
          ${pluginsPane}
        </div>
      </div>`;

    return { type: "html", content: html };
  },
};
