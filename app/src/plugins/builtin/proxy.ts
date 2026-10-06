// ── 代理插件（Modules\proxy，2026-10-02）─────────────────────────────
//
// **它管两件不同的事，别混**（这是本插件最容易讲错的一句话）：
//
//   ① **系统代理**（Windows 的 WinINet 设置，注册表 `HKCU\…\Internet Settings`）
//      —— 影响浏览器 / 商店应用 / 一部分走 WinINet 的程序。宿主那半边
//      （`system_proxy.rs`）2026-10-01 就写好了，但**一直没有界面**（死代码到本轮）。
//   ② **本机播放（librespot）的代理** —— 它**根本不读系统代理**，必须用启动参数
//      `-x <http-proxy>` 单独告诉它（见 music.rs 的 `librespot_set_proxy`）。
//      librespot 与 Spotify 的长连接跨境时会被周期性掐断（M2-9：约 27–36 分钟一次，
//      重连后 `active device` 变空 ⇒ 控制条条 `404 NO_ACTIVE_DEVICE`），走代理是根治。
//
// 所以界面上那条「联动」开关不是顺手加的：**只开系统代理，本机播放一点都没变** ——
// 一个用户最容易误解、而且误解之后完全看不出哪里错了的点。
//
// **样式纪律**：控件一律走 `styles.css` 里 `.proxy-*` 那一段，颜色只用主题变量
// （`--text` / `--text-dim` / `--accent*` / `--red`），不写死颜色。
//
// **磁盘插件契约**（同 ocr.ts）：默认导出带 `execute` 的对象 + 具名导出 `attach(root)`；
// 它是**接管型**（`permissions: ["layout.takeover"]`），所以 `attach` 收到的 root 是
// **文档根**（`document.body`），控件 id 全局唯一、直接用 `document` 取即可。
//
// ⚠️ 本文件的字符串里**不许出现反引号注释**（预检 #15 / #39 ⑩ 踩过 5 次）——
// 但本文件**不是**模板字符串，正常用反引号拼 HTML 是可以的；只是别在注释里写。

import { invoke } from "@tauri-apps/api/core";
import type { Plugin, PluginResult } from "../registry";
import { t } from "../../i18n.js";

/** 宿主 `system_proxy.rs` 的 `SystemProxy`：**当前系统的真实现状**（不从我们的配置推断）。 */
interface SystemProxy {
  enabled: boolean;
  server: string;
  bypass: string;
}

/** 宿主 `ProxyEntry`。 */
interface ProxyEntry {
  id: string;
  label: string;
  server: string;
  bypass: string;
}

/** 宿主 `ProxyConfig`（`config\proxy.json`）。`backup` 不往界面上带（那是宿主的兜底）。
 *
 *  `mihomo`（2026-10-03）**可选**：旧配置里没有这个键；而列表类操作（增删条目）走的是
 *  `proxy_config_set`，那个命令在宿主侧会**整体继承**磁盘上的 `mihomo` —— 这里不必回传，
 *  也就不必为它构造对象（见 system_proxy.rs 的 `proxy_config_set`）。
 */
interface ProxyConfig {
  entries: ProxyEntry[];
  active: string;
  link_librespot: boolean;
  mihomo?: MihomoConfig;
}

/** 宿主 `MihomoConfig`。`secret` / `links` **刻意不在界面类型里** ——
 *  前者是本机控制口的凭据（前端不该看到），后者本轮未支持（见 mihomo.rs 模块头）。 */
interface MihomoSubscription {
  name: string;
  url: string;
}
interface MihomoConfig {
  enabled: boolean;
  mixed_port: number;
  controller_port: number;
  mode: string;
  subscriptions: MihomoSubscription[];
  release_base: string;
  tun: boolean;
}

/** 宿主 `MihomoStatus`。 */
interface MihomoStatus {
  installed: boolean;
  running: boolean;
  version: string;
  mixed_port: number;
  controller_port: number;
  config_path: string;
  log_path: string;
  subscriptions: number;
}

/** 宿主 `mihomo_proxies()` 的一项（节点或组）。 */
interface MihomoProxy {
  name: string;
  type: string;
  is_group: boolean;
  now: string;
  all: string[];
  history: { time: string; delay: number }[];
}

// ── 模块级状态 ────────────────────────────────────────────────────
// 这份状态**只服务一个面板实例**（接管型面板同时只有一个），所以模块级足够，
// 不必再套一层闭包。`detach` 时清掉，下次打开就是干净的。

let cfg: ProxyConfig | null = null;
let sys: SystemProxy | null = null;
/** 内核状态（安装 / 运行 / 端口）。`null` = 还没问到。 */
let mstatus: MihomoStatus | null = null;
/** 内核里的节点与组（只在运行时可取；没跑就是空表）。 */
let mproxies: MihomoProxy[] = [];
/** 内核区块自己的防重入（与列表的 `busy` 分开：两者可以并存，互不阻塞）。 */
let mbusy = false;
/** 防重入：一次「启用/停用」里有两次 IPC（系统代理 + 联动），期间按钮要禁用。 */
let busy = false;
/** 头像一次性提示。**它不是状态** —— `renderPanel` 不读它，只负责按时抹掉。 */
let msgTimer: number | undefined;

function esc(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

function q<T extends HTMLElement>(sel: string): T | null {
  return document.querySelector<T>(sel);
}

/** 一条一次性提示（面板底部那行小字），3 秒后自己抹掉。 */
function note(text: string) {
  const el = q<HTMLElement>("#proxy-msg");
  if (!el) return;
  el.textContent = text;
  if (msgTimer !== undefined) window.clearTimeout(msgTimer);
  msgTimer = window.setTimeout(() => {
    const cur = q<HTMLElement>("#proxy-msg");
    if (cur) cur.textContent = "";
    msgTimer = undefined;
  }, 3000);
}

function newId(): string {
  return Date.now().toString(36) + Math.random().toString(36).slice(2, 6);
}

/** 前端这一层的**即时反馈**校验；**权威判据仍在宿主**（`validate_server` / `proxy_enable`）。
 *  这里做一遍只是「别让用户敲完等一个来回才被告知少了个端口」—— 两层都要有，不是重复。 */
function validateServer(server: string): string | null {
  // 判的是 **trim 之后**的值（提交时也会 trim）：首尾空白是手抖，不是非法
  const s = server.trim();
  if (!s) return t("proxy.err_empty");
  if (/\s/.test(s)) return t("proxy.err_space");
  if (!s.includes(":")) return t("proxy.err_port");
  return null;
}

/** `http(s)://` 才算「能给 librespot 用」的那一类（见 music.rs 的 `validate_librespot_proxy`）。
 *  界面据此在行内标一句提示 —— 否则用户会开一个 socks5 的联动、然后对着「没反应」发愁。 */
function isHttpProxy(server: string): boolean {
  const s = server.trim().toLowerCase();
  return s.startsWith("http://") || s.startsWith("https://");
}

// ── 取数 ──────────────────────────────────────────────────────────

async function refresh(): Promise<void> {
  try {
    cfg = await invoke<ProxyConfig>("proxy_config_get");
  } catch (e) {
    note(String(e));
    return;
  }
  try {
    // 真实现状：用户可能刚在「Internet 选项」里改过，所以**不能**从 cfg.active 推
    sys = await invoke<SystemProxy>("proxy_system_get");
  } catch {
    sys = null;
  }
  await refreshMihomo();
  renderPanel();
}

/** 内核那半边取数。**节点列表只在运行时才问** —— 没跑就去问 Clash API 必然失败，
 *  而那个失败没有任何信息量（只是「内核没起来」的另一种说法）。 */
async function refreshMihomo(): Promise<void> {
  try {
    mstatus = await invoke<MihomoStatus>("mihomo_status");
  } catch {
    mstatus = null;
  }
  if (mstatus?.running) {
    try {
      mproxies = await invoke<MihomoProxy[]>("mihomo_proxies");
    } catch {
      mproxies = [];
    }
  } else {
    mproxies = [];
  }
}

// ── 渲染 ──────────────────────────────────────────────────────────

function renderPanel(): void {
  renderState();
  renderMihomo();
  renderList();
  const link = q<HTMLInputElement>("#proxy-link");
  if (link && document.activeElement !== link) link.checked = !!cfg?.link_librespot;
}

// ── 内核区块（mihomo，2026-10-03）──────────────────────────────────
//
// 这一块与下面那张「手动代理列表」是**两种互斥的用法**：要么用内核托管（订阅/测速/规则），
// 要么手填一条现成的代理地址。界面把内核放在上面（它是主推路径），但**不强行隐藏**列表 ——
// 「只想指向本机已有的 7890」是最常见的轻量用法，藏起来反而逼人去看一堆用不上的设置。

function renderMihomo(): void {
  const m = cfg?.mihomo;
  const st = mstatus;

  // 静态输入框回填：**只在没聚焦时**（同 `#proxy-link` 的纪律，否则用户打字打字被覆盖）
  const backfill = (sel: string, val: string) => {
    const el = q<HTMLInputElement>(sel);
    if (el && document.activeElement !== el && !el.value) el.value = val;
  };
  if (m) {
    backfill("#mihomo-port", String(m.mixed_port));
    backfill("#mihomo-ctrl", String(m.controller_port));
    backfill("#mihomo-release", m.release_base);
  }

  const stateEl = q<HTMLElement>("#mihomo-state");
  if (stateEl) {
    if (!st) {
      stateEl.textContent = "";
    } else if (!st.installed) {
      stateEl.textContent = t("proxy.m_need_core");
    } else if (st.running) {
      stateEl.textContent = t("proxy.m_running", { ver: st.version, port: String(st.mixed_port) });
    } else {
      stateEl.textContent = t("proxy.m_stopped", { ver: st.version });
    }
    stateEl.classList.toggle("warn", !!st?.installed && !st?.running);
  }

  // 主按钮：未装 → 下载；已装未跑 → 启动；在跑 → 停止
  const actEl = q<HTMLElement>("#mihomo-actions");
  if (actEl) {
    const dis = mbusy ? " disabled" : "";
    if (!st?.installed) {
      actEl.innerHTML = `<button type="button" class="proxy-btn primary" id="mihomo-install"${dis}>${esc(t("proxy.m_install"))}</button>`;
    } else if (st.running) {
      actEl.innerHTML = `<button type="button" class="proxy-btn danger" id="mihomo-stop"${dis}>${esc(t("proxy.m_stop"))}</button>`;
    } else {
      actEl.innerHTML = `<button type="button" class="proxy-btn primary" id="mihomo-start"${dis}>${esc(t("proxy.m_start"))}</button>`;
    }
    if (mbusy) {
      const busy = document.createElement("span");
      busy.className = "proxy-row-note";
      busy.textContent = t("proxy.m_working");
      actEl.appendChild(busy);
    }
  }

  // 模式分段：把当前 mode 那一格点亮（2026-10-06 由原生 `<select>` 改成自制分段控件，
  // 与输入栏「思考开关」同一形态 —— 项目不用原生下拉，见 agent-ui-spec §5.3）
  const modeBox = q<HTMLElement>("#mihomo-mode");
  if (modeBox && m) {
    modeBox.querySelectorAll<HTMLElement>("[data-mode]").forEach((b) => {
      b.classList.toggle("on", b.dataset.mode === m.mode);
    });
  }

  renderMihomoSubs(m);
  renderMihomoNodes();
}

/** 订阅列表（含删除）。空表要明说「所以节点列表会是空的」—— 否则用户会以为坏了。 */
function renderMihomoSubs(m: MihomoConfig | undefined): void {
  const box = q<HTMLElement>("#mihomo-subs");
  if (!box) return;
  const subs = m?.subscriptions ?? [];
  if (subs.length === 0) {
    box.innerHTML = `<div class="proxy-empty">${esc(t("proxy.m_no_subs"))}</div>`;
    return;
  }
  box.innerHTML = subs
    .map(
      (s, i) => `<div class="proxy-row" data-sub="${i}">
      <div class="proxy-row-main">
        <div class="proxy-row-label">${esc(s.name || t("proxy.m_unnamed"))}</div>
        <div class="proxy-row-server">${esc(s.url)}</div>
      </div>
      <button type="button" class="proxy-btn danger" data-mact="sub-del"${mbusy ? " disabled" : ""}>${esc(t("proxy.del"))}</button>
    </div>`,
    )
    .join("");
}

/** 节点 / 组列表（运行时才非空）。组可切换成员，节点可测速。 */
function renderMihomoNodes(): void {
  const box = q<HTMLElement>("#mihomo-nodes");
  if (!box) return;
  if (!mstatus?.running) {
    box.innerHTML = "";
    return;
  }
  if (mproxies.length === 0) {
    box.innerHTML = `<div class="proxy-empty">${esc(t("proxy.m_no_nodes"))}</div>`;
    return;
  }
  box.innerHTML = mproxies
    .map((p) => {
      const delay = latestDelay(p);
      const dcls = delay === null ? "dim" : delay > 800 ? "bad" : "ok";
      const dTxt = delay === null ? "—" : `${delay} ms`;
      if (p.is_group) {
        // 组的成员是**分段按钮**（不是原生下拉，2026-10-06）：候选少、点一下即切，比下拉快。
        // 每次重绘整批换掉 ⇒ 点击仍走 `#mihomo-nodes` 上的委托（同本文件其它列表）。
        const picks = [...p.all]
          .map(
            (n) =>
              `<button type="button" class="proxy-seg-btn${n === p.now ? " on" : ""}" data-group-pick="${esc(n)}"${mbusy ? " disabled" : ""}>${esc(n)}</button>`,
          )
          .join("");
        return `<div class="proxy-row proxy-row-group" data-group="${esc(p.name)}">
          <div class="proxy-row-main">
            <div class="proxy-row-label">${esc(p.name)}<span class="proxy-tag">${esc(p.type)}</span></div>
            <div class="proxy-seg proxy-seg-wrap">${picks}</div>
          </div>
        </div>`;
      }
      return `<div class="proxy-row" data-node="${esc(p.name)}">
        <div class="proxy-row-main">
          <div class="proxy-row-label">${esc(p.name)}<span class="proxy-tag">${esc(p.type)}</span></div>
        </div>
        <span class="proxy-delay ${dcls}">${esc(dTxt)}</span>
        <button type="button" class="proxy-btn" data-mact="delay" data-name="${esc(p.name)}"${mbusy ? " disabled" : ""}>${esc(t("proxy.m_test"))}</button>
      </div>`;
    })
    .join("");
}

/** 最近一次测速结果（Clash 的 history 最后一项）；没有就是 null。 */
function latestDelay(p: MihomoProxy): number | null {
  const h = p.history;
  if (!Array.isArray(h) || h.length === 0) return null;
  const last = h[h.length - 1];
  const d = Number(last?.delay);
  return Number.isFinite(d) && d > 0 ? d : null;
}

/** 顶部那行「系统里到底是什么样」。**差异要如实说出来** —— 「配置里启用了、系统里没生效」
 *  正是组策略把我们的写入压回去时的样子，瞒着它比报错更糟。 */
function renderState(): void {
  const el = q<HTMLElement>("#proxy-state");
  if (!el) return;
  if (!sys) {
    el.textContent = "";
    return;
  }
  const activeEntry = cfg?.entries.find((e) => e.id === cfg?.active);
  if (!sys.enabled) {
    el.textContent = t("proxy.state_off");
  } else {
    el.textContent = t("proxy.state_on", { server: sys.server });
  }
  el.classList.toggle("warn", !sys.enabled && !!activeEntry);
  if (!sys.enabled && activeEntry) {
    const name = activeEntry.label || activeEntry.server;
    el.textContent += "  " + t("proxy.state_mismatch", { label: name });
  }
}

function renderList(): void {
  const box = q<HTMLElement>("#proxy-list");
  if (!box) return;
  const entries = cfg?.entries ?? [];
  if (entries.length === 0) {
    box.innerHTML = `<div class="proxy-empty">${esc(t("proxy.empty"))}</div>`;
    return;
  }
  box.innerHTML = entries
    .map((e) => {
      const active = cfg?.active === e.id;
      const socks = !isHttpProxy(e.server);
      return `<div class="proxy-row${active ? " on" : ""}" data-id="${esc(e.id)}">
        <div class="proxy-row-main">
          <div class="proxy-row-label">${esc(e.label || e.server)}${active ? `<span class="proxy-tag">${esc(t("proxy.in_use"))}</span>` : ""}</div>
          <div class="proxy-row-server">${esc(e.server)}${socks ? `<span class="proxy-row-note">${esc(t("proxy.socks_note"))}</span>` : ""}</div>
        </div>
        <button type="button" class="proxy-btn${active ? "" : " primary"}" data-act="${active ? "disable" : "enable"}"${busy ? " disabled" : ""}>${esc(active ? t("proxy.disable") : t("proxy.enable"))}</button>
        <button type="button" class="proxy-btn danger" data-act="del"${busy ? " disabled" : ""}>${esc(t("proxy.del"))}</button>
      </div>`;
    })
    .join("");
}

// ── 动作 ──────────────────────────────────────────────────────────

/** 把「这条 server」同步给本机播放（联动开着时才调）。**空串 = 回直连**。 */
async function syncLibrespot(server: string): Promise<void> {
  try {
    await invoke("librespot_set_proxy", { proxy: server });
  } catch (e) {
    // 失败要说出来：宿主那条命令会挡住 socks5 / 缺 scheme 这类值，
    // 而「系统代理开了、本机播放其实没走」是最难被发现的一种半生效。
    note(t("proxy.err_librespot", { err: String(e) }));
  }
}

async function doEnable(id: string): Promise<void> {
  const entry = cfg?.entries.find((e) => e.id === id);
  if (!entry || busy) return;
  busy = true;
  renderList();
  try {
    sys = await invoke<SystemProxy>("proxy_enable", { id });
  } catch (e) {
    busy = false;
    note(String(e));
    renderList();
    return;
  }
  busy = false;
  if (cfg?.link_librespot) await syncLibrespot(entry.server);
  if (!sys.enabled) note(t("proxy.err_not_applied"));
  await refresh();
}

async function doDisable(): Promise<void> {
  if (busy) return;
  busy = true;
  renderList();
  try {
    sys = await invoke<SystemProxy>("proxy_disable");
  } catch (e) {
    busy = false;
    note(String(e));
    renderList();
    return;
  }
  busy = false;
  // 联动开着：关闭代理 = 本机播放也回直连（这是那个开关的字面意思，界面文案里写了）
  if (cfg?.link_librespot) await syncLibrespot("");
  await refresh();
}

async function doDelete(id: string): Promise<void> {
  if (!cfg || busy) return;
  // 删的正好是启用中的那条：**先停代理再删** —— 不然系统里会留着一条指向已删条目的
  // 代理，而界面上再也找不到「停用」那个按钮（只能去系统设置里关）。
  if (cfg.active === id) {
    await doDisable();
    if (!cfg) return;
  }
  const next: ProxyConfig = {
    entries: cfg.entries.filter((e) => e.id !== id),
    active: cfg.active === id ? "" : cfg.active,
    link_librespot: cfg.link_librespot,
  };
  try {
    cfg = await invoke<ProxyConfig>("proxy_config_set", { config: next });
    note(t("proxy.removed"));
  } catch (e) {
    note(String(e));
  }
  await refresh();
}

async function doAdd(): Promise<void> {
  if (!cfg) return;
  const labelEl = q<HTMLInputElement>("#proxy-label");
  const serverEl = q<HTMLInputElement>("#proxy-server");
  const server = (serverEl?.value ?? "").trim();
  const err = validateServer(server);
  if (err) {
    note(err);
    return;
  }
  if (cfg.entries.some((e) => e.server.trim() === server)) {
    note(t("proxy.err_dup"));
    return;
  }
  const next: ProxyConfig = {
    entries: [...cfg.entries, { id: newId(), label: (labelEl?.value ?? "").trim(), server, bypass: "" }],
    active: cfg.active,
    link_librespot: cfg.link_librespot,
  };
  try {
    cfg = await invoke<ProxyConfig>("proxy_config_set", { config: next });
    if (labelEl) labelEl.value = "";
    if (serverEl) serverEl.value = "";
    note(t("proxy.added"));
  } catch (e) {
    note(String(e));
  }
  await refresh();
}

/** 联动开关。**打开时立刻同步一次**（如果已有启用中的那条）—— 否则用户会以为
 *  「我开了开关，怎么本机播放还是直连」，还得再去点一次启用。 */
async function doToggleLink(on: boolean): Promise<void> {
  if (!cfg) return;
  const next: ProxyConfig = { entries: cfg.entries, active: cfg.active, link_librespot: on };
  try {
    cfg = await invoke<ProxyConfig>("proxy_config_set", { config: next });
  } catch (e) {
    note(String(e));
    await refresh();
    return;
  }
  const activeEntry = cfg.entries.find((e) => e.id === cfg?.active);
  if (on && activeEntry) await syncLibrespot(activeEntry.server);
  if (!on) note(t("proxy.saved"));
  renderPanel();
}

// ── 内核动作（mihomo）─────────────────────────────────────────────

/** 统一收口：置忙 → 跑 → 记一条失败 → 复位 → 重取数。
 *  每个动作都走它，避免「某个动作忘了复位 mbusy 导致按钮永久禁用」这类漏。 */
async function mihomoAct(fn: () => Promise<void>): Promise<void> {
  if (mbusy) return;
  mbusy = true;
  renderMihomo();
  try {
    await fn();
  } catch (e) {
    note(String(e));
  }
  mbusy = false;
  await refresh();
}

/** 保存内核设置（订阅 / 端口 / 下载源）。**不动 secret / links** —— 宿主会续上旧值。 */
async function saveMihomo(m: MihomoConfig, okKey: string): Promise<void> {
  mstatus = await invoke<MihomoStatus>("mihomo_config_set", { cfg: m });
  note(t(okKey));
}

async function doMihomoInstall(): Promise<void> {
  await mihomoAct(async () => {
    const msg = await invoke<string>("mihomo_install_core");
    note(msg);
  });
}

async function doMihomoStart(): Promise<void> {
  await mihomoAct(async () => {
    mstatus = await invoke<MihomoStatus>("mihomo_start");
    // 联动开着 ⇒ 本机播放也要指到内核的混合端口（与手动启用那条同一语义）。
    if (cfg?.link_librespot) await syncLibrespot(`http://127.0.0.1:${mstatus.mixed_port}`);
  });
}

async function doMihomoStop(): Promise<void> {
  await mihomoAct(async () => {
    mstatus = await invoke<MihomoStatus>("mihomo_stop");
    if (cfg?.link_librespot) await syncLibrespot("");
  });
}

async function doMihomoSave(): Promise<void> {
  const m = cfg?.mihomo;
  if (!m) return;
  const port = Number((q<HTMLInputElement>("#mihomo-port")?.value ?? "").trim()) || m.mixed_port;
  const ctrl = Number((q<HTMLInputElement>("#mihomo-ctrl")?.value ?? "").trim()) || m.controller_port;
  if (port === ctrl) {
    note(t("proxy.m_err_same_port"));
    return;
  }
  const release = (q<HTMLInputElement>("#mihomo-release")?.value ?? "").trim();
  await mihomoAct(() =>
    saveMihomo({ ...m, mixed_port: port, controller_port: ctrl, release_base: release }, "proxy.m_saved"),
  );
}

async function doSubAdd(): Promise<void> {
  const m = cfg?.mihomo;
  if (!m || mbusy) return;
  const nameEl = q<HTMLInputElement>("#mihomo-sub-name");
  const urlEl = q<HTMLInputElement>("#mihomo-sub-url");
  const url = (urlEl?.value ?? "").trim();
  if (!/^https?:\/\//i.test(url)) {
    note(t("proxy.m_err_url"));
    return;
  }
  const name = (nameEl?.value ?? "").trim() || `sub_${m.subscriptions.length}`;
  if (m.subscriptions.some((s) => s.url === url)) {
    note(t("proxy.err_dup"));
    return;
  }
  if (nameEl) nameEl.value = "";
  if (urlEl) urlEl.value = "";
  await mihomoAct(() => saveMihomo({ ...m, subscriptions: [...m.subscriptions, { name, url }] }, "proxy.m_saved"));
}

async function doSubDel(idx: number): Promise<void> {
  const m = cfg?.mihomo;
  if (!m) return;
  await mihomoAct(() =>
    saveMihomo({ ...m, subscriptions: m.subscriptions.filter((_, i) => i !== idx) }, "proxy.m_saved"),
  );
}

async function doMode(mode: string): Promise<void> {
  await mihomoAct(async () => {
    mstatus = await invoke<MihomoStatus>("mihomo_set_mode", { mode });
  });
}

async function doDelay(name: string): Promise<void> {
  if (mbusy) return;
  mbusy = true;
  renderMihomoNodes();
  try {
    const ms = await invoke<number>("mihomo_delay", { name });
    note(t("proxy.m_delay_ok", { name, ms: String(ms) }));
  } catch (e) {
    note(String(e));
  }
  mbusy = false;
  await refreshMihomo();
  renderMihomoNodes();
}

async function doSelect(group: string, node: string): Promise<void> {
  if (mbusy) return;
  mbusy = true;
  try {
    await invoke("mihomo_select", { group, node });
  } catch (e) {
    note(String(e));
  }
  mbusy = false;
  await refreshMihomo();
  renderMihomoNodes();
}

async function doMihomoLog(): Promise<void> {
  try {
    const text = await invoke<string>("mihomo_log");
    const pre = q<HTMLElement>("#mihomo-log");
    if (pre) pre.textContent = text || t("proxy.m_log_empty");
  } catch (e) {
    note(String(e));
  }
}

// ── 面板 HTML ─────────────────────────────────────────────────────

function panelHtml(): string {
  return `<div class="plugin-result proxy-panel">
  <div class="proxy-head">
    <div class="proxy-title">${esc(t("proxy.title"))}</div>
    <div class="proxy-state" id="proxy-state"></div>
  </div>

  <div class="proxy-body">
    <nav class="proxy-nav" id="proxy-nav">
      <button type="button" class="proxy-nav-btn on" data-page="proxies">${esc(t("proxy.nav_proxies"))}</button>
      <button type="button" class="proxy-nav-btn" data-page="subs">${esc(t("proxy.nav_subs"))}</button>
      <button type="button" class="proxy-nav-btn" data-page="log">${esc(t("proxy.nav_log"))}</button>
      <button type="button" class="proxy-nav-btn" data-page="settings">${esc(t("proxy.nav_settings"))}</button>
    </nav>

    <div class="proxy-pages">
      <section class="proxy-page on" data-page="proxies">
        <div class="proxy-sec-title">${esc(t("proxy.m_title"))}</div>
        <div class="proxy-state" id="mihomo-state" title="${esc(t("proxy.m_hint"))}"></div>
        <div class="proxy-actions" id="mihomo-actions"></div>
        <div class="proxy-seg" id="mihomo-mode">
          <button type="button" class="proxy-seg-btn" data-mode="rule">rule</button>
          <button type="button" class="proxy-seg-btn" data-mode="global">global</button>
          <button type="button" class="proxy-seg-btn" data-mode="direct">direct</button>
        </div>
        <div class="proxy-list" id="mihomo-nodes"></div>
      </section>

      <section class="proxy-page" data-page="subs">
        <div class="proxy-list" id="mihomo-subs"></div>
        <div class="proxy-form">
          <input id="mihomo-sub-name" type="text" autocomplete="off" spellcheck="false" placeholder="${esc(t("proxy.m_sub_name_ph"))}" />
          <input id="mihomo-sub-url" type="text" autocomplete="off" spellcheck="false" placeholder="${esc(t("proxy.m_sub_url_ph"))}" />
          <button type="button" class="proxy-btn primary" id="mihomo-sub-add">${esc(t("proxy.m_sub_add"))}</button>
        </div>
      </section>

      <section class="proxy-page" data-page="log">
        <div class="proxy-form">
          <button type="button" class="proxy-btn" id="mihomo-log-btn">${esc(t("proxy.m_log"))}</button>
        </div>
        <pre class="proxy-log" id="mihomo-log"></pre>
      </section>

      <section class="proxy-page" data-page="settings">
        <div class="proxy-form">
          <input id="mihomo-port" type="number" min="1" max="65535" autocomplete="off" placeholder="${esc(t("proxy.m_port_ph"))}" />
          <input id="mihomo-ctrl" type="number" min="1" max="65535" autocomplete="off" placeholder="${esc(t("proxy.m_ctrl_ph"))}" />
          <button type="button" class="proxy-btn" id="mihomo-save">${esc(t("proxy.m_save"))}</button>
        </div>
        <div class="proxy-form">
          <input id="mihomo-release" type="text" autocomplete="off" spellcheck="false" placeholder="${esc(t("proxy.m_release_ph"))}" />
        </div>
        <div class="proxy-sec-title" title="${esc(t("proxy.hint"))}">${esc(t("proxy.manual_title"))}</div>
        <div class="proxy-list" id="proxy-list"></div>
        <div class="proxy-form">
          <input id="proxy-label" type="text" autocomplete="off" spellcheck="false" placeholder="${esc(t("proxy.label_ph"))}" />
          <input id="proxy-server" type="text" autocomplete="off" spellcheck="false" placeholder="${esc(t("proxy.server_ph"))}" />
          <button type="button" class="proxy-btn primary" id="proxy-add">${esc(t("proxy.add"))}</button>
        </div>
        <label class="proxy-link" title="${esc(t("proxy.link_hint"))}">
          <input type="checkbox" id="proxy-link" />
          <span>${esc(t("proxy.link"))}</span>
        </label>
        <div class="proxy-msg" id="proxy-msg"></div>
      </section>
    </div>
  </div>
</div>`;
}

export const proxyPlugin: Plugin = {
  id: "proxy",
  name: "Proxy",
  keywords: ["proxy", "代理", "系统代理", "梯子", "vpn", "socks5", "http proxy", "clash", "mihomo", "订阅", "节点"],
  description: "系统代理管理 + mihomo 内核托管（订阅 / 测速 / 规则）+ 本机播放走代理",
  icon: "🌐",
  badge: "Network",
  // 接管型：面板 HTML 写进结果区、切成 detached（与 OCR 同一条路，见 attach.ts）
  permissions: ["layout.takeover"],
  execute: async (): Promise<PluginResult> => ({ type: "html", content: panelHtml() }),
};

/** 磁盘插件契约：面板打开后由宿主调起（root = 文档根，见文件头）。 */
export async function attach(): Promise<void> {
  // ── 左侧导航（2026-10-06 多页重构）──────────────────────────────
  // 纯前端切页：`display` 切换，**不重取数据**（各页数据仍由 `refresh()` 统一刷）。
  // ⚠️ 本插件是 `layout.takeover` 型 ⇒ `document` 上可能同时存在别的面板，
  // 选择器一律从 `.proxy-panel` 往下找，不用全局 `querySelectorAll`。
  const panel = q<HTMLElement>(".proxy-panel");
  panel?.querySelector<HTMLElement>("#proxy-nav")?.addEventListener("click", (ev) => {
    const btn = (ev.target as HTMLElement).closest<HTMLElement>("[data-page]");
    const page = btn?.dataset.page;
    if (!panel || !page || btn?.classList.contains("on")) return;
    panel.querySelectorAll<HTMLElement>("#proxy-nav [data-page]").forEach((b) => {
      b.classList.toggle("on", b.dataset.page === page);
    });
    panel.querySelectorAll<HTMLElement>(".proxy-page").forEach((s) => {
      s.classList.toggle("on", s.dataset.page === page);
    });
    // 进日志页顺手拉一次 —— 否则用户看到的是一张空页面，还得自己去找「获取日志」
    if (page === "log") void doMihomoLog();
  });

  // 用**委托**而不是逐个绑定：列表每次重绘都会换掉整批按钮（同 ai-spec 规则 76 那条
  // 「重绘处不该重新绑监听」的教训）。
  q<HTMLElement>("#proxy-list")?.addEventListener("click", (ev) => {
    const btn = (ev.target as HTMLElement).closest<HTMLElement>("[data-act]");
    const row = btn?.closest<HTMLElement>(".proxy-row");
    const id = row?.dataset.id;
    if (!btn || !id) return;
    const act = btn.dataset.act;
    if (act === "enable") void doEnable(id);
    else if (act === "disable") void doDisable();
    else if (act === "del") void doDelete(id);
  });
  q<HTMLElement>("#proxy-add")?.addEventListener("click", () => void doAdd());
  // ── 内核区块（mihomo）───────────────────────────────────────────
  // 主按钮由 `renderMihomo` 动态重画 ⇒ 同样用委托绑，不在重绘处重新绑监听。
  q<HTMLElement>("#mihomo-actions")?.addEventListener("click", (ev) => {
    const id = (ev.target as HTMLElement).id;
    if (id === "mihomo-install") void doMihomoInstall();
    else if (id === "mihomo-start") void doMihomoStart();
    else if (id === "mihomo-stop") void doMihomoStop();
  });
  q<HTMLElement>("#mihomo-subs")?.addEventListener("click", (ev) => {
    const btn = (ev.target as HTMLElement).closest<HTMLElement>("[data-mact='sub-del']");
    const row = btn?.closest<HTMLElement>("[data-sub]");
    const idx = row?.dataset.sub;
    if (btn && idx !== undefined) void doSubDel(Number(idx));
  });
  // 节点列表：一个 click 委托吃两种动作 —— 组的分段按钮（切换成员）与节点的测速按钮。
  q<HTMLElement>("#mihomo-nodes")?.addEventListener("click", (ev) => {
    const el = ev.target as HTMLElement;
    const pick = el.closest<HTMLElement>("[data-group-pick]");
    if (pick?.dataset.groupPick !== undefined) {
      const row = pick.closest<HTMLElement>("[data-group]");
      if (row?.dataset.group) void doSelect(row.dataset.group, pick.dataset.groupPick);
      return;
    }
    const btn = el.closest<HTMLElement>("[data-mact='delay']");
    const name = btn?.dataset.name;
    if (btn && name) void doDelay(name);
  });
  q<HTMLElement>("#mihomo-save")?.addEventListener("click", () => void doMihomoSave());
  q<HTMLElement>("#mihomo-sub-add")?.addEventListener("click", () => void doSubAdd());
  for (const sel of ["#mihomo-sub-name", "#mihomo-sub-url"]) {
    q<HTMLInputElement>(sel)?.addEventListener("keydown", (ev) => {
      if ((ev as KeyboardEvent).key === "Enter") void doSubAdd();
    });
  }
  // 模式：分段按钮（原为原生 `<select>`，2026-10-06 换掉）
  q<HTMLElement>("#mihomo-mode")?.addEventListener("click", (ev) => {
    const btn = (ev.target as HTMLElement).closest<HTMLElement>("[data-mode]");
    if (btn?.dataset.mode) void doMode(btn.dataset.mode);
  });
  q<HTMLElement>("#mihomo-log-btn")?.addEventListener("click", () => void doMihomoLog());
  // 两个输入框上按回车 = 添加（用户敲完地址最自然的动作）
  for (const sel of ["#proxy-label", "#proxy-server"]) {
    q<HTMLInputElement>(sel)?.addEventListener("keydown", (ev) => {
      if ((ev as KeyboardEvent).key === "Enter") void doAdd();
    });
  }
  q<HTMLInputElement>("#proxy-link")?.addEventListener("change", (ev) => {
    void doToggleLink((ev.target as HTMLInputElement).checked);
  });
  await refresh();
}

/** 面板关闭时收尾。没有定时器要停（`msgTimer` 自己会超时），只清状态 ——
 *  下次打开重新问一遍宿主，别把上一次的列表留在内存里当真相。 */
export function detach(): void {
  if (msgTimer !== undefined) {
    window.clearTimeout(msgTimer);
    msgTimer = undefined;
  }
  cfg = null;
  sys = null;
  mstatus = null;
  mproxies = [];
  busy = false;
  mbusy = false;
}

export default proxyPlugin;
