// ── 代理插件（Modules\proxy，2026-11-01 三段式重构）───────────────────
//
// 布局（独立窗 1280×720，见 `lunac-plugin.json` 的 `window`）：
//
//   ┌ 工具区（一行按钮，点开下拉菜单，再点开界面内独立面板）──────────────┐
//   │ 服务器导入 / 服务器订阅更新 / 设置                                  │
//   ├ 中间区（左侧抽屉 + 中央大字按钮 + 速度 / 地区 / 模式 / TUN）────────┤
//   ├ 日志区（可拖拽调整高度；每行是「目标域名 + 走向」）──────────────────┤
//   └──────────────────────────────────────────────────────────────┘
//
// **只有自研引擎**（2026-11-01，§待办 3-26 K3/K4）：界面上不再出现 mihomo 字样 /
// 下载内核入口；引擎编进宿主，随安装包分发，插件 `dependencies` 为空。
//
// **TUN**：引擎还没有 TUN（需要 wintun + 用户态 TCP/IP 栈）。开关**可见但会如实
// 报错并回弹** —— 不假装切换成功（`docs/forbidden.md` §禁用 1）。
//
// 样式一律走 `styles.css` 的 `.px-*` 段；颜色只用主题变量。

import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import type { Plugin, PluginResult } from "../registry";
import { t } from "../../i18n.js";

/** 服务（系统代理）开/关变化 —— 搜索区状态栏的「代理:开/关」泡泡框据此更新。 */
const PROXY_STATE_EVENT = "proxy-service-state";

interface ProxyEntry {
  id: string;
  label: string;
  server: string;
  bypass: string;
}

interface MihomoSubscription {
  name: string;
  url: string;
}

/** 宿主 `MihomoConfig`（`config\proxy.json` 的 `mihomo` 段）。 */
interface MihomoConfig {
  enabled: boolean;
  mixed_port: number;
  controller_port: number;
  mode: string;
  subscriptions: MihomoSubscription[];
  tun: boolean;
  engine: string;
  /** 规则名单语义：`blacklist`（名单直连）/ `whitelist`（名单强制代理）。 */
  rule_mode?: string;
  /** 规则名单域名（一行一个）。 */
  rule_domains?: string[];
}

interface ProxyConfig {
  entries: ProxyEntry[];
  active: string;
  link_librespot: boolean;
  mihomo?: MihomoConfig;
}

/** 宿主 `MihomoStatus`（2026-11-01 起带流量 / 连接 / 选中项）。 */
interface MihomoStatus {
  installed: boolean;
  /** **引擎**是否在跑（打开面板就会静默拉起，测延迟靠它）。 */
  running: boolean;
  /** **服务**（系统代理）是否开启 —— 开关与状态泡泡框看这个。 */
  enabled: boolean;
  version: string;
  mixed_port: number;
  controller_port: number;
  config_path: string;
  log_path: string;
  subscriptions: number;
  nodes: { index: number; name: string; kind: string }[];
  engine: string;
  mode: string;
  tun: boolean;
  bytes_up: number;
  bytes_down: number;
  connections: number;
  active: number;
  selected: string;
}

/** 宿主 `native_proxy::ServerView`。 */
interface ServerView {
  index: number;
  name: string;
  kind: string;
  server: string;
  port: number;
  transport: string;
  source: string;
  selected: boolean;
}

/** 宿主 `native_proxy::RequestView`。 */
interface RequestView {
  ts: string;
  host: string;
  port: number;
  via: string;
  node: string;
}

interface ExitInfo {
  ok: boolean;
  ip: string;
  country: string;
  region: string;
  city: string;
  error: string;
}

// ── 模块级状态 ────────────────────────────────────────────────────
// 接管型面板同时只有一个实例，模块级足够；`detach()` 清空。

let cfg: ProxyConfig | null = null;
let st: MihomoStatus | null = null;
let servers: ServerView[] = [];
let requests: RequestView[] = [];
let exitInfo: ExitInfo | null = null;
let busy = false;
let msgTimer: number | undefined;
let statusTimer: number | undefined;
let logTimer: number | undefined;
let exitTimer: number | undefined;
let lastSample: { t: number; up: number; down: number } | null = null;
let drawerOpen = false;
let activePanel = "";
let logHeight = 190;
const KINDS = ["ss", "trojan", "vless", "vmess", "anytls", "hysteria2"] as const;
const CIPHERS = ["aes-256-gcm", "aes-128-gcm", "chacha20-ietf-poly1305"] as const;
let kindPick = "ss";
let cipherPick = "aes-256-gcm";
/** 节点名 → 延迟（毫秒）；`NaN` = 测试中，`-1` = 失败，缺省 = 还没测。 */
let delays = new Map<string, number>();
let delayBusy = false;

function esc(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

function q<T extends HTMLElement>(sel: string): T | null {
  return document.querySelector<T>(sel);
}

function note(text: string): void {
  const el = q<HTMLElement>("#px-msg");
  if (!el) return;
  el.textContent = text;
  if (msgTimer !== undefined) window.clearTimeout(msgTimer);
  msgTimer = window.setTimeout(() => {
    const cur = q<HTMLElement>("#px-msg");
    if (cur) cur.textContent = "";
    msgTimer = undefined;
  }, 4000);
}

function fmtBytes(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v >= 100 ? v.toFixed(0) : v.toFixed(1)} ${units[i]}`;
}

function fmtSpeed(bps: number): string {
  return `${fmtBytes(bps)}/s`;
}

function hhmmss(ts: string): string {
  // 宿主写的是 UTC；面板显示**本地时间**（用户看日志时不换算时区）。
  const d = new Date(ts);
  if (!Number.isNaN(d.getTime())) {
    return d.toLocaleTimeString([], { hour12: false });
  }
  const m = ts.match(/T(\d{2}:\d{2}:\d{2})/);
  return m ? m[1] : ts.replace("T", " ").replace("Z", "");
}

// ── 取数 ──────────────────────────────────────────────────────────

async function refresh(): Promise<void> {
  try {
    cfg = await invoke<ProxyConfig>("proxy_config_get");
    st = await invoke<MihomoStatus>("mihomo_status");
    servers = await invoke<ServerView[]>("mihomo_servers").catch(() => []);
  } catch (e) {
    note(String(e));
    return;
  }
  if (st?.running) {
    void refreshExit();
    startPolling();
  } else {
    stopPolling();
    exitInfo = null;
    lastSample = null;
  }
  render();
  broadcastServiceState();
}

/** 把「服务开/关」广播给主窗状态栏（泡泡框）。失败无所谓：状态栏自己也会在启动时读一次。 */
function broadcastServiceState(): void {
  void emit(PROXY_STATE_EVENT, { enabled: !!st?.enabled }).catch(() => {});
}

async function refreshExit(): Promise<void> {
  try {
    exitInfo = await invoke<ExitInfo>("mihomo_exit_info");
  } catch {
    exitInfo = null;
  }
  renderRegion();
}

function startPolling(): void {
  if (statusTimer === undefined) {
    statusTimer = window.setInterval(() => void pollStatus(), 1000);
    logTimer = window.setInterval(() => void pollLog(), 2000);
    exitTimer = window.setInterval(() => void refreshExit(), 60000);
  }
}

function stopPolling(): void {
  if (statusTimer !== undefined) window.clearInterval(statusTimer);
  if (logTimer !== undefined) window.clearInterval(logTimer);
  if (exitTimer !== undefined) window.clearInterval(exitTimer);
  statusTimer = undefined;
  logTimer = undefined;
  exitTimer = undefined;
}

/** 每秒取一次状态：算速率、刷新连接数、引擎被外部停掉时同步界面。 */
async function pollStatus(): Promise<void> {
  try {
    st = await invoke<MihomoStatus>("mihomo_status");
  } catch {
    return;
  }
  if (!st.running) {
    stopPolling();
    exitInfo = null;
    lastSample = null;
    render();
    return;
  }
  const now = performance.now();
  if (lastSample) {
    const dt = (now - lastSample.t) / 1000;
    if (dt > 0.2) {
      const up = Math.max(0, st.bytes_up - lastSample.up) / dt;
      const down = Math.max(0, st.bytes_down - lastSample.down) / dt;
      setText("#px-up", fmtSpeed(up));
      setText("#px-down", fmtSpeed(down));
    }
  }
  lastSample = { t: now, up: st.bytes_up, down: st.bytes_down };
  renderState();
}

/** 每两秒取一次请求日志（宿主只保留最近 500 条）。 */
async function pollLog(): Promise<void> {
  try {
    requests = await invoke<RequestView[]>("mihomo_requests", { limit: 200 });
  } catch {
    return;
  }
  renderLog();
}

function setText(sel: string, text: string): void {
  const el = q<HTMLElement>(sel);
  if (el && el.textContent !== text) el.textContent = text;
}

// ── 渲染 ──────────────────────────────────────────────────────────

function render(): void {
  renderState();
  renderOrb();
  renderMode();
  renderTun();
  renderServers();
  renderLog();
  renderRegion();
  const portEl = q<HTMLInputElement>("#px-set-port");
  if (portEl && document.activeElement !== portEl) {
    portEl.value = String(cfg?.mihomo?.mixed_port ?? st?.mixed_port ?? 0);
  }
  setText("#px-set-engine", st?.engine === "mihomo" ? "mihomo" : "native");
  setText("#px-set-log", st?.log_path || "—");
  // 规则配置文件：黑/白名单开关 + 域名清单（输入中不覆写，避免打断光标）。
  const white = (cfg?.mihomo?.rule_mode ?? "blacklist") === "whitelist";
  const modeTgl = q<HTMLInputElement>("#px-rule-mode");
  if (modeTgl) modeTgl.checked = white;
  const modeLabel = q<HTMLElement>("#px-rule-mode-label");
  if (modeLabel) {
    modeLabel.textContent = white ? t("proxy.px_rule_whitelist") : t("proxy.px_rule_blacklist");
  }
  const list = q<HTMLTextAreaElement>("#px-rule-domains");
  if (list && document.activeElement !== list) {
    list.value = (cfg?.mihomo?.rule_domains ?? []).join("\n");
  }
}

function renderState(): void {
  const el = q<HTMLElement>("#px-state");
  if (!el) return;
  if (!st) {
    el.textContent = "";
    return;
  }
  const legacy = st.engine === "mihomo";
  // 「引擎在跑」不再等于「服务开着」：打开面板会静默拉起引擎（测延迟用），
  // 系统代理仍可以是关的（2026-11-03）。
  const on = !!st.enabled;
  el.classList.toggle("warn", legacy || !st.running);
  if (legacy) {
    el.textContent = t("proxy.m_legacy_engine");
    return;
  }
  el.textContent = !st.running
    ? t("proxy.px_state_stopped")
    : on
      ? t("proxy.px_state_running", { port: String(st.mixed_port) })
      : t("proxy.px_state_ready", { port: String(st.mixed_port) });
}

function renderOrb(): void {
  const orb = q<HTMLButtonElement>("#px-orb");
  if (!orb) return;
  const legacy = st?.engine === "mihomo";
  // 大字按钮 = **服务**开关（系统代理），不是引擎开关：引擎由面板静默拉起。
  const on = !!st?.enabled;
  orb.disabled = busy || !st || legacy;
  orb.textContent = busy
    ? t("proxy.m_working")
    : on
      ? t("proxy.px_orb_stop")
      : t("proxy.px_orb_start");
  orb.dataset.state = on ? "running" : "stopped";
  orb.classList.toggle("legacy", legacy);
}

function renderMode(): void {
  const label = q<HTMLElement>("#px-mode-label");
  if (!label) return;
  // 面板只给两档（2026-11-02 用户口径）：规则模式 / 全局代理。
  // 旧配置里的 `direct` 不再有入口，按规则模式展示（引擎仍按配置执行）。
  const mode = (cfg?.mihomo?.mode ?? st?.mode ?? "rule") === "global" ? "global" : "rule";
  label.textContent = mode === "global" ? t("proxy.px_mode_global") : t("proxy.px_mode_rule");
  document.querySelectorAll<HTMLElement>("#px-mode-menu [data-mode]").forEach((b) => {
    b.classList.toggle("on", b.dataset.mode === mode);
  });
}

function renderTun(): void {
  const tgl = q<HTMLInputElement>("#px-tun");
  if (tgl) tgl.checked = !!st?.tun;
}

function renderRegion(): void {
  const el = q<HTMLElement>("#px-region");
  if (!el) return;
  const node = st?.selected || "";
  const src = servers.find((s) => s.name === node)?.source || "";
  if (!st?.running) {
    el.textContent = t("proxy.px_region_idle");
    return;
  }
  const where = exitInfo?.ok
    ? [exitInfo.country, exitInfo.region, exitInfo.city, exitInfo.ip].filter(Boolean).join(" · ")
    : t("proxy.px_region_unknown");
  el.textContent = [where, node ? `${src ? `${src} / ` : ""}${node}` : ""]
    .filter(Boolean)
    .join("　");
}

function renderServers(): void {
  const box = q<HTMLElement>("#px-servers");
  if (!box) return;
  if (servers.length === 0) {
    box.innerHTML = `<div class="px-empty">${esc(t("proxy.px_server_empty"))}</div>`;
    return;
  }
  const running = !!st?.running;
  box.innerHTML = servers
    .map((s) => {
      const ms = delays.get(s.name);
      const delay =
        ms === undefined
          ? ""
          : Number.isNaN(ms)
            ? `<span class="px-delay pending">…</span>`
            : ms < 0
              ? `<span class="px-delay bad" title="${esc(t("proxy.px_delay_failed"))}">×</span>`
              : `<span class="px-delay${ms <= 150 ? " fast" : ms >= 400 ? " slow" : ""}">${Math.round(ms)} ms</span>`;
      return `<button type="button" class="px-server${s.selected ? " on" : ""}" data-server="${esc(s.name)}"${running ? "" : " disabled"}>
        <span class="px-server-main">
          <span class="px-server-name">${esc(s.name || t("proxy.m_unnamed"))}</span>
          <span class="px-server-sub">${esc(s.server)}:${s.port}　${esc(s.transport)}</span>
        </span>
        <span class="px-server-side">
          ${delay}
          <span class="px-tag">${esc(s.kind)}</span>
          <span class="px-server-src">${esc(s.source || t("proxy.px_manual"))}</span>
        </span>
      </button>`;
    })
    .join("");
}

function renderLog(): void {
  const box = q<HTMLElement>("#px-log-list");
  if (!box) return;
  if (requests.length === 0) {
    box.innerHTML = `<div class="px-empty">${esc(t("proxy.px_log_empty"))}</div>`;
    return;
  }
  const srcOf = new Map(servers.map((s) => [s.name, s.source]));
  box.innerHTML = [...requests]
    .reverse()
    .map((r) => {
      const src = r.node ? srcOf.get(r.node) || "" : "";
      const via =
        r.via === "proxy"
          ? [src, r.node].filter(Boolean).join(" / ")
          : r.via === "direct"
            ? t("proxy.px_via_direct")
            : t("proxy.px_via_reject");
      return `<div class="px-log-row" data-via="${esc(r.via)}">
        <span class="px-log-ts">${esc(hhmmss(r.ts))}</span>
        <span class="px-log-host" title="${esc(`${r.host}:${r.port}`)}">${esc(r.host)}<span class="px-log-port">:${r.port}</span></span>
        <span class="px-log-via">${esc(via)}</span>
      </div>`;
    })
    .join("");
}

// ── 抽屉 / 面板 ────────────────────────────────────────────────────

function setDrawer(open: boolean): void {
  drawerOpen = open;
  const drawer = q<HTMLElement>("#px-drawer");
  drawer?.classList.toggle("open", open);
  const rail = q<HTMLElement>("#px-rail");
  rail?.classList.toggle("hidden", open);
  // 拉回拉手只在抽屉展开时出现（与「服务器」拉手互斥，位置相同）。
  q<HTMLElement>("#px-drawer-close")?.classList.toggle("hidden", !open);
}

function openPanel(name: string): void {
  activePanel = name;
  const layer = q<HTMLElement>("#px-layer");
  layer?.classList.remove("hidden");
  document.querySelectorAll<HTMLElement>(".px-panel").forEach((p) => {
    p.classList.toggle("hidden", p.dataset.panel !== name);
  });
}

function closePanel(): void {
  activePanel = "";
  q<HTMLElement>("#px-layer")?.classList.add("hidden");
}

function closeMenus(): void {
  document.querySelectorAll<HTMLElement>("#px-tools .px-tool").forEach((el) => el.classList.remove("open"));
}

// ── 动作 ──────────────────────────────────────────────────────────

async function act(fn: () => Promise<void>): Promise<void> {
  if (busy) return;
  busy = true;
  renderOrb();
  try {
    await fn();
  } catch (e) {
    note(String(e));
  }
  busy = false;
  await refresh();
}

async function doOrb(): Promise<void> {
  if (!st) return;
  await act(async () => {
    // 只看「服务」开关：引擎可能已经为测延迟静默跑着（2026-11-03）。
    if (st?.enabled) {
      await invoke("mihomo_stop");
    } else {
      await invoke("mihomo_start");
    }
  });
}

async function doMode(mode: string): Promise<void> {
  // 模式在引擎启动时展开：服务开着 ⇒ 必须先关服务；服务关着但引擎为测延迟跑着 ⇒
  // 这里静默停一下引擎、改完再拉起来（用户不需要知道引擎的存在，2026-11-03）。
  if (st?.enabled) {
    note(t("proxy.px_mode_running"));
    return;
  }
  const wasRunning = !!st?.running;
  try {
    if (wasRunning) await invoke("mihomo_engine_stop");
    try {
      await invoke("mihomo_set_mode", { mode });
      note(t("proxy.px_mode_saved"));
    } catch (e) {
      note(String(e));
    }
  } finally {
    // 改失败也要把引擎拉回来，否则测延迟会莫名其妙失效。
    if (wasRunning) await invoke("mihomo_engine_start").catch(() => {});
  }
  await refresh();
}

async function doTun(on: boolean): Promise<void> {
  try {
    const next = await invoke<MihomoStatus>("mihomo_set_tun", { enabled: on });
    st = next;
    note(on ? t("proxy.px_tun_on") : t("proxy.px_tun_off"));
  } catch (e) {
    // 宿主如实拒绝：回弹开关并把原因显示出来（不假装已开启）。
    note(String(e));
  }
  const tgl = q<HTMLInputElement>("#px-tun");
  if (tgl) tgl.checked = !!st?.tun;
  renderTun();
}

/** TUN 的管理员闸门：先问宿主当前是否已提权；未提权就弹独立警告框，
 *  **同意之前不改配置**（开关回弹）。 */
async function requestTun(on: boolean): Promise<void> {
  const tgl = q<HTMLInputElement>("#px-tun");
  if (!on) {
    await doTun(false);
    return;
  }
  const elevated = await invoke<boolean>("mihomo_is_elevated").catch(() => false);
  if (elevated) {
    await doTun(true);
    return;
  }
  // 未提权：dev 与 release 的处置不同 —— dev 不能自动重启（会丢 vite），只给手动步骤。
  if (tgl) tgl.checked = false;
  const dev = await invoke<boolean>("mihomo_is_dev_build").catch(() => false);
  const title = q<HTMLElement>("#px-tun-title");
  const body = q<HTMLElement>("#px-tun-body");
  const ok = q<HTMLButtonElement>("#px-tun-ok");
  const cancel = q<HTMLButtonElement>("#px-tun-cancel");
  if (title) title.textContent = dev ? t("proxy.px_tun_dev_title") : t("proxy.px_tun_warn_title");
  if (body) body.textContent = dev ? t("proxy.px_tun_dev_body") : t("proxy.px_tun_warn_body");
  ok?.classList.toggle("hidden", dev);
  if (cancel) cancel.textContent = dev ? t("proxy.px_tun_dev_ok") : t("proxy.px_cancel");
  q<HTMLElement>("#px-tun-modal")?.classList.remove("hidden");
}

async function doSelect(name: string): Promise<void> {
  if (!st?.running) {
    note(t("proxy.px_select_hint"));
    return;
  }
  try {
    await invoke("mihomo_select", { group: "", node: name });
    note(t("proxy.px_selected", { name }));
    servers = await invoke<ServerView[]>("mihomo_servers").catch(() => servers);
    renderServers();
    void refreshExit();
  } catch (e) {
    note(String(e));
  }
}

async function doRefreshSubs(): Promise<void> {
  try {
    const n = await invoke<number>("mihomo_refresh_subs");
    note(t("proxy.px_subs_updated", { n: String(n) }));
  } catch (e) {
    note(String(e));
  }
  await refresh();
}

/** 测试连通性：逐个 **经节点** 发 HTTP 探针（宿主 `mihomo_delay`），结果写进抽屉列表。 */
async function doTestDelays(): Promise<void> {
  if (delayBusy) return;
  // 测延迟只需要**引擎**（本机监听口），不需要开启服务。打开面板时已经在后台静默
  // 拉起；若还没就绪（刚打开的一瞬间 / 启动失败）这里再补一次（2026-11-03）。
  if (!st?.running) {
    try {
      st = await invoke<MihomoStatus>("mihomo_engine_start");
      servers = await invoke<ServerView[]>("mihomo_servers").catch(() => servers);
      renderServers();
    } catch {
      note(t("proxy.px_delay_need_running"));
      return;
    }
  }
  const names = servers.map((s) => s.name).filter(Boolean);
  if (names.length === 0) {
    note(t("proxy.px_delay_no_nodes"));
    return;
  }
  delayBusy = true;
  delays = new Map(names.map((name) => [name, Number.NaN]));
  renderDelayButton();
  renderServers();
  let ok = 0;
  for (const name of names) {
    try {
      const ms = await invoke<number>("mihomo_delay", { name });
      delays.set(name, ms);
      ok++;
    } catch {
      delays.set(name, -1);
    }
    renderServers();
  }
  delayBusy = false;
  renderDelayButton();
  note(t("proxy.px_delay_done", { ok: String(ok), n: String(names.length) }));
}

/** 工具区按钮在测速期间置灰并换文案。 */
function renderDelayButton(): void {
  const btn = q<HTMLButtonElement>("#px-delay-btn");
  if (!btn) return;
  btn.disabled = delayBusy;
  btn.textContent = delayBusy ? t("proxy.px_delay_testing") : t("proxy.px_tool_delay");
}

/** 打开面板时**静默拉起引擎**（只开本机监听口，不碰系统代理、不改服务开关）。
 *  这样用户不必先「开启服务」就能测节点延迟（2026-11-03）。失败不打扰用户 ——
 *  面板照常显示，测延迟点下去会按节点如实报错。 */
async function ensureEngine(): Promise<void> {
  try {
    await invoke<MihomoStatus>("mihomo_engine_start");
    // 引擎起来后节点清单才完整（订阅在启动时拉平），轮询也才有意义。
    await refresh();
  } catch (e) {
    console.warn("[proxy] engine auto-start failed:", e);
  }
}

/** 保存「规则配置文件」：名单语义（黑 / 白）+ 域名清单，落到 `config\proxy.json`
 *  的 mihomo 段。`rule` 模式下引擎把它插在订阅规则之前（见 `native_proxy::clash::build`）：
 *  黑名单 = 名单内直连，白名单 = 名单内强制走代理。 */
async function doSaveDomains(): Promise<void> {
  if (!cfg?.mihomo) return;
  const domains = (q<HTMLTextAreaElement>("#px-rule-domains")?.value ?? "")
    .split(/\r?\n/)
    .map((s) => s.trim().toLowerCase())
    .filter((s) => s && !s.startsWith("#") && !s.startsWith("//"));
  const next = {
    ...cfg.mihomo,
    // 0 = 由宿主在启动时挑一个空闲端口（原语义保留）。
    mixed_port: Math.max(0, Math.min(65535, Number(q<HTMLInputElement>("#px-set-port")?.value ?? "0") || 0)),
    rule_mode: q<HTMLInputElement>("#px-rule-mode")?.checked ? "whitelist" : "blacklist",
    rule_domains: domains,
  };
  try {
    st = await invoke<MihomoStatus>("mihomo_config_set", { cfg: next });
    note(t("proxy.px_set_saved"));
    await refresh();
  } catch (e) {
    note(String(e));
  }
}

async function doCustomAdd(): Promise<void> {
  const name = (q<HTMLInputElement>("#px-c-name")?.value ?? "").trim();
  const server = (q<HTMLInputElement>("#px-c-server")?.value ?? "").trim();
  const port = Number(q<HTMLInputElement>("#px-c-port")?.value ?? "0");
  const credential = (q<HTMLInputElement>("#px-c-cred")?.value ?? "").trim();
  if (!server || !port || !credential) {
    note(t("proxy.px_custom_required"));
    return;
  }
  try {
    st = await invoke<MihomoStatus>("mihomo_node_add_custom", {
      name,
      server,
      port,
      kind: kindPick,
      credential,
      cipher: kindPick === "ss" ? cipherPick : null,
    });
    note(t("proxy.m_node_added"));
    closePanel();
    // 重建服务器缓存（引擎在跑时宿主会拒绝，属预期：新节点随下次启动生效）。
    await invoke("mihomo_refresh_subs").catch(() => {});
    await refresh();
  } catch (e) {
    note(String(e));
  }
}

async function readClipboard(): Promise<string> {
  const { readText } = await import("@tauri-apps/plugin-clipboard-manager");
  const text = await readText();
  return text ?? "";
}

async function doPasteRead(): Promise<void> {
  try {
    const text = await readClipboard();
    const area = q<HTMLTextAreaElement>("#px-paste-text");
    if (area) area.value = text;
    if (!text.trim()) note(t("proxy.px_paste_empty"));
  } catch (e) {
    note(String(e));
  }
}

async function doPasteImport(): Promise<void> {
  const text = q<HTMLTextAreaElement>("#px-paste-text")?.value ?? "";
  const lines = text
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter((l) => l && !l.startsWith("#"));
  if (lines.length === 0) {
    note(t("proxy.px_paste_empty"));
    return;
  }
  let ok = 0;
  let fail = 0;
  for (const link of lines) {
    try {
      await invoke("mihomo_node_add", { link });
      ok++;
    } catch {
      fail++;
    }
  }
  note(t("proxy.px_paste_done", { ok: String(ok), fail: String(fail) }));
  if (ok > 0) closePanel();
  if (ok > 0) await invoke("mihomo_refresh_subs").catch(() => {});
  await refresh();
}

// ── 日志区高度（拖拽 + 折叠，持久化）────────────────────────────────

function applyLogHeight(): void {
  const root = q<HTMLElement>(".proxy-panel");
  if (!root) return;
  root.style.setProperty("--px-log-h", `${logHeight}px`);
}

function loadLogPrefs(): void {
  try {
    const h = Number(localStorage.getItem("lunac.proxy.logHeight"));
    if (Number.isFinite(h) && h >= 80 && h <= 480) logHeight = h;
  } catch {
    /* localStorage 不可用时用默认值 */
  }
}

function saveLogPrefs(): void {
  try {
    localStorage.setItem("lunac.proxy.logHeight", String(Math.round(logHeight)));
  } catch {
    /* 忽略 */
  }
}

function wireLogResize(): void {
  const handle = q<HTMLElement>("#px-log-handle");
  const panel = q<HTMLElement>(".proxy-panel");
  if (!handle || !panel) return;
  let startY = 0;
  let startH = 0;
  let dragging = false;
  // 上限按**面板总高**算，并给中间区留 220px（168px 大按钮 + 速度 / 地区 / 底部控件）。
  // 不能用 `stage.clientHeight`：拖到一半 stage 变小会把上限一起拉低（越拖越拖不动）；
  // 也不能留 360px —— 小窗口下上限直接跌到下限，表现为「拉不动」。
  const maxLogHeight = (): number => {
    const top = q<HTMLElement>(".px-top")?.offsetHeight ?? 50;
    const msg = q<HTMLElement>("#px-msg")?.offsetHeight ?? 0;
    return Math.max(96, panel.clientHeight - top - msg - 220 - 24);
  };
  const stop = () => {
    if (!dragging) return;
    dragging = false;
    window.removeEventListener("mousemove", onMove);
    window.removeEventListener("mouseup", stop);
    window.removeEventListener("blur", stop);
    saveLogPrefs();
  };
  const onMove = (ev: MouseEvent) => {
    if (!dragging) return;
    // 按键已经松开（鼠标在窗口外抬起时收不到 mouseup）：就地收尾，下一次还能拖。
    if ((ev.buttons & 1) === 0) {
      stop();
      return;
    }
    logHeight = Math.min(maxLogHeight(), Math.max(80, startH + (startY - ev.clientY)));
    applyLogHeight();
    ev.preventDefault();
  };
  // **只用 mouse 事件**（2026-11-02 第三次修复）：WebView2 在 pointerdown 之后可能因原生
  // 选择 / 拖拽发出 `pointercancel`，把同一段拖动提前结束 —— 表现正是「指针变成 ns-resize
  // 但高度不跟」。鼠标输入在 Chromium 里一定有兼容 mouse 事件，这条链路不经过 pointer 流。
  handle.addEventListener("mousedown", (ev) => {
    if (ev.button !== 0) return;
    ev.preventDefault();
    startY = ev.clientY;
    startH = logHeight;
    dragging = true;
    // 先移除再挂：上一轮若在窗口外松开（收不到 mouseup），这里立刻恢复。
    window.removeEventListener("mousemove", onMove);
    window.removeEventListener("mouseup", stop);
    window.removeEventListener("blur", stop);
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", stop);
    window.addEventListener("blur", stop);
  });
}

// ── 面板 HTML ─────────────────────────────────────────────────────

function kindsHtml(): string {
  return KINDS.map(
    (k) => `<button type="button" class="px-chip${k === kindPick ? " on" : ""}" data-kind="${k}">${k}</button>`,
  ).join("");
}

function ciphersHtml(): string {
  return CIPHERS.map(
    (c) => `<button type="button" class="px-chip${c === cipherPick ? " on" : ""}" data-cipher="${c}">${c}</button>`,
  ).join("");
}

function panelHtml(): string {
  return `<div class="plugin-result proxy-panel">
  <header class="px-top">
    <div class="px-brand">${esc(t("proxy.title"))}</div>
    <nav class="px-tools" id="px-tools">
      <div class="px-tool-group">
        <div class="px-tool">
          <button type="button" class="px-tool-btn" data-tool="import">${esc(t("proxy.px_tool_import"))}<span class="px-caret">▾</span></button>
          <div class="px-menu">
            <button type="button" data-open="custom">${esc(t("proxy.px_import_custom"))}</button>
            <button type="button" data-open="paste">${esc(t("proxy.px_import_paste"))}</button>
          </div>
        </div>
        <div class="px-tool">
          <button type="button" class="px-tool-btn" data-action="refresh-subs">${esc(t("proxy.px_tool_subs"))}</button>
        </div>
        <div class="px-tool">
          <button type="button" class="px-tool-btn" id="px-delay-btn" data-action="delay">${esc(t("proxy.px_tool_delay"))}</button>
        </div>
        <div class="px-tool">
          <button type="button" class="px-tool-btn" data-open="settings">${esc(t("proxy.px_tool_settings"))}</button>
        </div>
      </div>
    </nav>
    <div class="px-state" id="px-state"></div>
  </header>

  <section class="px-stage" id="px-stage">
    <div class="px-center">
      <button type="button" class="px-orb" id="px-orb" data-state="stopped">${esc(t("proxy.px_orb_start"))}</button>
      <div class="px-speeds">
        <span class="px-speed"><span class="px-speed-label">↑</span><b id="px-up">0 B/s</b></span>
        <span class="px-speed"><span class="px-speed-label">↓</span><b id="px-down">0 B/s</b></span>
      </div>
      <div class="px-region" id="px-region"></div>
      <div class="px-controls">
        <div class="px-tool px-mode">
          <button type="button" class="px-tool-btn" id="px-mode-btn">
            <span id="px-mode-label">${esc(t("proxy.px_mode_rule"))}</span><span class="px-caret">▾</span>
          </button>
          <div class="px-menu up" id="px-mode-menu">
            <button type="button" data-mode="rule">${esc(t("proxy.px_mode_rule"))}</button>
            <button type="button" data-mode="global">${esc(t("proxy.px_mode_global"))}</button>
          </div>
        </div>
        <label class="px-tun" title="${esc(t("proxy.px_tun_hint"))}">
          <span>${esc(t("proxy.px_tun"))}</span>
          <input type="checkbox" id="px-tun" />
          <i class="px-switch"></i>
        </label>
      </div>
    </div>

    <button type="button" class="px-rail" id="px-rail" title="${esc(t("proxy.px_drawer_open"))}">${esc(t("proxy.px_drawer_open"))}</button>
    <aside class="px-drawer" id="px-drawer">
      <div class="px-drawer-head">
        <b>${esc(t("proxy.px_drawer_title"))}</b>
      </div>
      <div class="px-drawer-list" id="px-servers"></div>
    </aside>
    <!-- 拉回：与左侧「服务器」拉手同一形状（竖向贴在左缘），只是高度 60 且用 < 图标 -->
    <button type="button" class="px-rail px-rail-close hidden" id="px-drawer-close" title="${esc(t("proxy.px_drawer_close"))}" aria-label="${esc(t("proxy.px_drawer_close"))}">
      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="15 18 9 12 15 6"/></svg>
    </button>
  </section>

  <section class="px-log" id="px-log">
    <div class="px-log-handle" id="px-log-handle" title="${esc(t("proxy.px_log_resize"))}"></div>
    <div class="px-log-head">
      <b>${esc(t("proxy.px_log_title"))}</b>
      <span class="px-log-hint">${esc(t("proxy.px_log_hint"))}</span>
    </div>
    <div class="px-log-list" id="px-log-list"></div>
  </section>

  <div class="px-msg" id="px-msg"></div>

  <!-- TUN 的管理员闸门：独立警告框，同意后才请求提权重启。 -->
  <div class="px-modal hidden" id="px-tun-modal">
    <div class="px-modal-card">
      <b id="px-tun-title">${esc(t("proxy.px_tun_warn_title"))}</b>
      <p id="px-tun-body">${esc(t("proxy.px_tun_warn_body"))}</p>
      <div class="px-modal-btns">
        <button type="button" class="px-btn" id="px-tun-cancel">${esc(t("proxy.px_cancel"))}</button>
        <button type="button" class="px-btn primary" id="px-tun-ok">${esc(t("proxy.px_tun_elevate"))}</button>
      </div>
    </div>
  </div>

  <div class="px-layer hidden" id="px-layer">
    <section class="px-panel hidden" data-panel="custom">
      <div class="px-panel-head">
        <b>${esc(t("proxy.px_import_custom"))}</b>
        <button type="button" class="px-btn" data-close>${esc(t("proxy.px_close"))}</button>
      </div>
      <div class="px-form">
        <label class="px-field"><span>${esc(t("proxy.px_field_name"))}</span>
          <input id="px-c-name" type="text" autocomplete="off" spellcheck="false" placeholder="${esc(t("proxy.m_sub_name_ph"))}" /></label>
        <label class="px-field"><span>${esc(t("proxy.px_field_server"))}</span>
          <input id="px-c-server" type="text" autocomplete="off" spellcheck="false" placeholder="example.com" /></label>
        <label class="px-field small"><span>${esc(t("proxy.px_field_port"))}</span>
          <input id="px-c-port" type="number" min="1" max="65535" placeholder="443" /></label>
        <label class="px-field"><span>${esc(t("proxy.px_field_cred"))}</span>
          <input id="px-c-cred" type="password" autocomplete="off" spellcheck="false" placeholder="${esc(t("proxy.px_cred_ph"))}" /></label>
        <div class="px-field wide"><span>${esc(t("proxy.px_field_kind"))}</span>
          <div class="px-chips" id="px-kind">${kindsHtml()}</div></div>
        <div class="px-field wide" id="px-cipher-wrap"><span>${esc(t("proxy.px_field_cipher"))}</span>
          <div class="px-chips" id="px-cipher">${ciphersHtml()}</div></div>
      </div>
      <div class="px-panel-foot">
        <button type="button" class="px-btn primary" id="px-c-add">${esc(t("proxy.px_add"))}</button>
      </div>
    </section>

    <section class="px-panel hidden" data-panel="paste">
      <div class="px-panel-head">
        <b>${esc(t("proxy.px_import_paste"))}</b>
        <button type="button" class="px-btn" data-close>${esc(t("proxy.px_close"))}</button>
      </div>
      <p class="px-note">${esc(t("proxy.px_paste_hint"))}</p>
      <textarea id="px-paste-text" spellcheck="false" placeholder="${esc(t("proxy.px_paste_ph"))}"></textarea>
      <div class="px-panel-foot">
        <button type="button" class="px-btn" id="px-paste-read">${esc(t("proxy.px_paste_read"))}</button>
        <button type="button" class="px-btn primary" id="px-paste-import">${esc(t("proxy.px_paste_import"))}</button>
      </div>
    </section>

    <section class="px-panel hidden" data-panel="settings">
      <div class="px-panel-head">
        <b>${esc(t("proxy.px_tool_settings"))}</b>
        <button type="button" class="px-btn" data-close>${esc(t("proxy.px_close"))}</button>
      </div>
      <div class="px-setrow plain"><span><b>${esc(t("proxy.px_set_port"))}</b></span>
        <input id="px-set-port" class="px-num" type="number" min="0" max="65535" placeholder="0" /></div>
      <div class="px-setrow plain"><span><b>${esc(t("proxy.px_set_engine"))}</b></span><span id="px-set-engine">native</span></div>
      <div class="px-setrow plain"><span><b>${esc(t("proxy.px_set_log"))}</b></span><span id="px-set-log" class="px-wrap">—</span></div>
      <div class="px-setrow plain"><span><b>${esc(t("proxy.px_set_rule_profile"))}</b></span>
        <span class="px-rule-mode">
          <em id="px-rule-mode-label">${esc(t("proxy.px_rule_blacklist"))}</em>
          <label class="px-tun"><input type="checkbox" id="px-rule-mode" /><i class="px-switch"></i></label>
        </span>
      </div>
      <div class="px-field wide">
        <span>${esc(t("proxy.px_rule_domains"))}</span>
        <textarea id="px-rule-domains" rows="6" spellcheck="false"></textarea>
      </div>
      <div class="px-panel-foot">
        <button type="button" class="px-btn primary" id="px-dom-save">${esc(t("proxy.px_set_save"))}</button>
      </div>
    </section>
  </div>
</div>`;
}

export const proxyPlugin: Plugin = {
  id: "proxy",
  name: "Proxy",
  keywords: ["proxy", "代理", "系统代理", "梯子", "vpn", "socks5", "http proxy", "订阅", "节点"],
  description: "系统代理 + 自研代理内核（订阅 / 服务器 / 规则 / 请求日志）",
  icon: "🌐",
  badge: "Network",
  permissions: ["layout.takeover", "window.float"],
  execute: async (): Promise<PluginResult> => ({ type: "html", content: panelHtml() }),
};

// ── 装配 ──────────────────────────────────────────────────────────

export async function attach(): Promise<void> {
  const panel = q<HTMLElement>(".proxy-panel");
  if (!panel) return;
  loadLogPrefs();
  applyLogHeight();
  wireLogResize();

  // 工具区：下拉菜单 + 打开面板 + 订阅更新（直接刷新服务器，不开面板）。
  q<HTMLElement>("#px-tools")?.addEventListener("click", (ev) => {
    const target = ev.target as HTMLElement;
    const openBtn = target.closest<HTMLElement>("[data-open]");
    if (openBtn?.dataset.open) {
      closeMenus();
      openPanel(openBtn.dataset.open);
      return;
    }
    if (target.closest("[data-action='refresh-subs']")) {
      closeMenus();
      void doRefreshSubs();
      return;
    }
    if (target.closest("[data-action='delay']")) {
      closeMenus();
      void doTestDelays();
      return;
    }
    const toolBtn = target.closest<HTMLElement>("[data-tool]");
    if (toolBtn) {
      const tool = toolBtn.parentElement;
      const wasOpen = tool?.classList.contains("open");
      closeMenus();
      if (tool && !wasOpen) tool.classList.add("open");
    }
  });

  // 模式菜单（在中央区，单独挂）。
  q<HTMLElement>("#px-mode-btn")?.addEventListener("click", (ev) => {
    ev.stopPropagation();
    q<HTMLElement>("#px-mode-btn")?.parentElement?.classList.toggle("open");
  });
  q<HTMLElement>("#px-mode-menu")?.addEventListener("click", (ev) => {
    const btn = (ev.target as HTMLElement).closest<HTMLElement>("[data-mode]");
    q<HTMLElement>("#px-mode-btn")?.parentElement?.classList.remove("open");
    if (btn?.dataset.mode) void doMode(btn.dataset.mode);
  });

  // 抽屉
  q<HTMLElement>("#px-rail")?.addEventListener("click", () => setDrawer(true));
  q<HTMLElement>("#px-drawer-close")?.addEventListener("click", () => setDrawer(false));
  q<HTMLElement>("#px-servers")?.addEventListener("click", (ev) => {
    const btn = (ev.target as HTMLElement).closest<HTMLElement>("[data-server]");
    if (btn?.dataset.server) void doSelect(btn.dataset.server);
  });

  // 中央
  q<HTMLElement>("#px-orb")?.addEventListener("click", () => void doOrb());
  q<HTMLInputElement>("#px-tun")?.addEventListener("change", (ev) => {
    void requestTun((ev.target as HTMLInputElement).checked);
  });
  // TUN 警告框：同意 ⇒ 提权重启；取消 ⇒ 关框并保持关闭。
  q<HTMLElement>("#px-tun-cancel")?.addEventListener("click", () => {
    q<HTMLElement>("#px-tun-modal")?.classList.add("hidden");
    const tgl = q<HTMLInputElement>("#px-tun");
    if (tgl) tgl.checked = !!st?.tun;
  });
  q<HTMLElement>("#px-tun-ok")?.addEventListener("click", () => {
    q<HTMLElement>("#px-tun-modal")?.classList.add("hidden");
    void invoke("mihomo_relaunch_elevated").catch((e) => note(String(e)));
  });

  // 面板：关闭 / 协议与加密选择
  q<HTMLElement>("#px-layer")?.addEventListener("click", (ev) => {
    const target = ev.target as HTMLElement;
    if (target.closest("[data-close]")) {
      closePanel();
      return;
    }
    const kind = target.closest<HTMLElement>("[data-kind]");
    if (kind?.dataset.kind) {
      kindPick = kind.dataset.kind;
      panel.querySelectorAll<HTMLElement>("#px-kind [data-kind]").forEach((b) => {
        b.classList.toggle("on", b.dataset.kind === kindPick);
      });
      const wrap = q<HTMLElement>("#px-cipher-wrap");
      wrap?.classList.toggle("hidden", kindPick !== "ss");
    }
    const cipher = target.closest<HTMLElement>("[data-cipher]");
    if (cipher?.dataset.cipher) {
      cipherPick = cipher.dataset.cipher;
      panel.querySelectorAll<HTMLElement>("#px-cipher [data-cipher]").forEach((b) => {
        b.classList.toggle("on", b.dataset.cipher === cipherPick);
      });
    }
  });

  q<HTMLElement>("#px-c-add")?.addEventListener("click", () => void doCustomAdd());
  q<HTMLElement>("#px-paste-read")?.addEventListener("click", () => void doPasteRead());
  q<HTMLElement>("#px-paste-import")?.addEventListener("click", () => void doPasteImport());
  // 黑 / 白名单滑轨：只改文案，保存仍走「保存」按钮（避免半途改动落盘）。
  q<HTMLInputElement>("#px-rule-mode")?.addEventListener("change", (ev) => {
    const white = (ev.target as HTMLInputElement).checked;
    const label = q<HTMLElement>("#px-rule-mode-label");
    if (label) label.textContent = white ? t("proxy.px_rule_whitelist") : t("proxy.px_rule_blacklist");
  });
  q<HTMLButtonElement>("#px-dom-save")?.addEventListener("click", () => void doSaveDomains());

  // 点到菜单外就收起（只在面板内监听：接管型面板可能和别的面板共存）。
  panel.addEventListener("click", (ev) => {
    const target = ev.target as HTMLElement;
    if (!target.closest(".px-tool")) closeMenus();
    if (!target.closest(".px-mode")) {
      q<HTMLElement>("#px-mode-btn")?.parentElement?.classList.remove("open");
    }
  });

  await refresh();
  if (st?.running) void pollLog();
  // 静默拉起引擎（不挡首屏）：这是「不开启服务也能测延迟」的前提（2026-11-03）。
  // 引擎启动时会把订阅拉平（`clash::build`），所以不再另跑一次「只拉订阅」——
  // 那两条路并发会互相把对方判成「已在运行 / 先停用」。
  void ensureEngine();
}

/** 面板关闭时收尾：停掉轮询，清空状态（下次打开重新问一遍宿主）。 */
export function detach(): void {
  // 引擎的收尾规则（2026-11-03）：
  //   · 服务开着 ⇒ 引擎留在后台继续跑（用户此刻正在用代理，关面板不能断网）；
  //   · 服务没开 ⇒ 关掉引擎（它只是为了测延迟临时拉起的）。
  const engineRunning = !!st?.running;
  const serviceOn = !!st?.enabled;
  if (msgTimer !== undefined) window.clearTimeout(msgTimer);
  msgTimer = undefined;
  stopPolling();
  closeMenus();
  closePanel();
  if (engineRunning && !serviceOn) {
    void invoke("mihomo_engine_stop").catch(() => {});
  }
  broadcastServiceState();
  cfg = null;
  st = null;
  servers = [];
  requests = [];
  exitInfo = null;
  lastSample = null;
  busy = false;
  drawerOpen = false;
  activePanel = "";
  delays = new Map<string, number>();
  delayBusy = false;
}

export default proxyPlugin;
