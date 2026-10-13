// 一次性 CDP 端到端复现：聊天窗在「有在飞轮次」时被关掉，那一轮是否有收尾方。
// 用法（单个命令内跑完，避免沙箱在命令结束时回收 GUI 进程）：
//   node temp\pet-e2e.mjs
// 判据用算术答案（数字不出现在提问文本里），避免「用户气泡里就有关键词」的假阳性。
//
// 关窗用 `plugin:window|close`（= 用户点标题栏 × 走的那条 `getCurrentWindow().close()`），
// **不是** destroy —— 只有 close 会走 Tauri 的 CloseRequested，才谈得上「前端能否在关闭前收尾」。
//
// 「桌宠是否收到收尾」用**真开一扇桌宠窗**当证据（读它的气泡 #pet-chat-text）：
// `__TAURI_INTERNALS__.invoke` 是 non-writable/non-configurable，挂钩探针那条路走不通。
import { spawn, spawnSync } from "node:child_process";

const EXE = process.env.PET_E2E_EXE || "d:\\cc\\claude-code-cli-master\\app\\src-tauri\\target\\release\\lunac.exe";
const CWD = process.env.PET_E2E_CWD || "d:\\cc\\claude-code-cli-master\\app\\src-tauri";
const CDP = "http://127.0.0.1:9222";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const log = (...a) => console.log("[e2e]", ...a);
const results = [];
function record(name, ok, detail = "") {
  results.push({ name, ok, detail });
  log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? "  — " + detail : ""}`);
}

async function listTargets() {
  try {
    const r = await fetch(`${CDP}/json/list`);
    const all = await r.json();
    return all.filter((t) => t.type === "page" && /tauri\.localhost|index\.html|plugin\.html/.test(t.url));
  } catch { return []; }
}

function connect(wsUrl) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(wsUrl);
    let id = 0;
    const pending = new Map();
    const client = {
      send(method, params = {}) {
        return new Promise((res, rej) => {
          const mid = ++id;
          pending.set(mid, { res, rej });
          ws.send(JSON.stringify({ id: mid, method, params }));
        });
      },
      close() { try { ws.close(); } catch {} },
    };
    ws.onopen = () => resolve(client);
    ws.onmessage = (ev) => {
      const m = JSON.parse(ev.data);
      if (m.id && pending.has(m.id)) {
        const p = pending.get(m.id);
        pending.delete(m.id);
        if (m.error) p.rej(new Error(JSON.stringify(m.error)));
        else p.res(m.result);
      }
    };
    ws.onerror = (e) => reject(new Error("ws error " + (e?.message || "")));
  });
}

async function evl(client, expression, awaitPromise = true) {
  const r = await client.send("Runtime.evaluate", { expression, awaitPromise, returnByValue: true });
  if (r.exceptionDetails) throw new Error("eval exception: " + JSON.stringify(r.exceptionDetails));
  return r.result.value;
}

async function kindOf(t) {
  try {
    const c = await connect(t.webSocketDebuggerUrl);
    const k = await evl(c, "document.documentElement.dataset.windowKind || '?'");
    return { client: c, kind: k };
  } catch {
    return null;
  }
}

async function findKind(kind, tries = 40) {
  for (let i = 0; i < tries; i++) {
    for (const t of await listTargets()) {
      if (/plugin\.html/.test(t.url)) continue; // 桌宠窗不是 index.html
      const got = await kindOf(t);
      if (got && got.kind === kind) return got.client;
    }
    await sleep(500);
  }
  return null;
}

/** 桌宠窗（plugin.html）。 */
async function findPet(tries = 40) {
  for (let i = 0; i < tries; i++) {
    for (const t of await listTargets()) {
      if (/plugin\.html/.test(t.url)) {
        try { return await connect(t.webSocketDebuggerUrl); } catch { /* retry */ }
      }
    }
    await sleep(500);
  }
  return null;
}

async function readText(client) {
  try {
    return await evl(
      client,
      "(document.getElementById('pet-hidden-flow')?.innerText || '') + '\\n' + (document.getElementById('chat-log')?.innerText || '') + '\\n' + (document.querySelector('.agent-flow')?.innerText || '')",
    );
  } catch { return ""; }
}

async function waitForText(client, needle, timeoutMs) {
  const t0 = Date.now();
  while (Date.now() - t0 < timeoutMs) {
    const txt = await readText(client);
    if (typeof txt === "string" && txt.includes(needle)) return { ok: true, txt };
    await sleep(1000);
  }
  return { ok: false, txt: await readText(client) };
}

async function streamingOn(chat) {
  try { return await evl(chat, "document.getElementById('chat-stop-btn')?.style.display === 'flex'"); }
  catch { return false; }
}

async function waitStreaming(chat, timeoutMs) {
  const t0 = Date.now();
  while (Date.now() - t0 < timeoutMs) {
    if (await streamingOn(chat)) return true;
    await sleep(300);
  }
  return false;
}

/** 等这一轮**真正收尾**（`#chat-stop-btn` 收起 = 前端 `isStreaming=false`）。
 *  **为什么必须等**：`startAIChat` 开头有 `if (isStreaming) return;`（main.ts:10461）——
 *  上一轮还在飞时，在同一个窗里直接发起下一轮会被**静默丢弃**（invoke 不报错、只是没发）。
 *  `waitForText` 只要在流式途中命中那个数字就会立刻返回，此刻 `isStreaming` 仍是 true ⇒
 *  紧接的下一轮偶发假失败。这里显式清场，让用例间互不污染（本次 CASE5 的偶发失败即此）。 */
async function waitTurnIdle(chat, timeoutMs) {
  const t0 = Date.now();
  while (Date.now() - t0 < timeoutMs) {
    if (!(await streamingOn(chat))) return true;
    await sleep(300);
  }
  return false;
}

/** 在桌宠窗里自装 pet-chat-reply 监听（该窗口 label = plugin-pet）。
 *  用 CDP 直装而不靠桌宠插件自身：release 的 Modules 里没有 pet 插件，窗口只是个空壳，
 *  插件自己的监听不会装上。监听指向 `Window{label: plugin-pet}`，能命中 emitTo 的定向投递。 */
async function installPetListener(pet) {
  // 目标一出现就 attach，页面 JS 可能还没注入 __TAURI_INTERNALS__ ⇒ 先等它就绪。
  for (let i = 0; i < 60; i++) {
    try {
      const ready = await evl(pet, "typeof window.__TAURI_INTERNALS__?.transformCallback === 'function'");
      if (ready) break;
    } catch { /* retry */ }
    await sleep(300);
  }
  return await evl(pet, `new Promise((res) => {
    window.__petGot = [];
    const h = window.__TAURI_INTERNALS__.transformCallback((ev) => { window.__petGot.push(ev && ev.payload); });
    window.__TAURI_INTERNALS__.invoke('plugin:event|listen', { event: 'pet-chat-reply', target: { kind: 'Window', label: 'plugin-pet' }, handler: h })
      .then(() => res(true)).catch((e) => res('err:' + e));
  })`);
}

async function readPetGot(pet) {
  try { return await evl(pet, "JSON.parse(JSON.stringify(window.__petGot || []))"); }
  catch { return []; }
}

async function resetPetGot(pet) {
  try { await evl(pet, "window.__petGot = []"); } catch {}
}

async function waitPetGot(pet, re, timeoutMs) {
  const t0 = Date.now();
  let last = [];
  while (Date.now() - t0 < timeoutMs) {
    last = await readPetGot(pet);
    if (last.some((p) => re.test(String(p && p.text || "")))) return { ok: true, got: last };
    await sleep(500);
  }
  return { ok: false, got: last };
}

const EMIT = (event, payload) =>
  `window.__TAURI_INTERNALS__.invoke('plugin:event|emit', { event: ${JSON.stringify(event)}, payload: ${JSON.stringify(payload)} })`;

async function agentState(main) {
  try { return await evl(main, "window.__TAURI_INTERNALS__.invoke('get_status').then(x => x.state)"); }
  catch (e) { return "err:" + String(e); }
}

/** 用户点标题栏 × 走的那条路（`getCurrentWindow().close()`）。 */
async function closeLikeUserX(chat) {
  try { await evl(chat, "window.__TAURI_INTERNALS__.invoke('plugin:window|close', { label: 'plugin-chat' })"); }
  catch {}
}

async function main() {
  const app = spawn(EXE, [], { cwd: CWD, detached: true, stdio: "ignore" });
  app.unref();
  const pid = app.pid;
  log("launched pid", pid);

  let mainWin = null;
  for (let i = 0; i < 60; i++) {
    for (const t of await listTargets()) {
      if (/plugin\.html/.test(t.url)) continue;
      const got = await kindOf(t);
      if (got && got.kind === "main") { mainWin = got.client; break; }
    }
    if (mainWin) break;
    await sleep(500);
  }
  if (!mainWin) { log("FAIL: 主窗 CDP 目标未就绪"); return finish(pid, 1); }
  log("main window attached; agent state =", await agentState(mainWin));

  // 开一扇桌宠窗当「收尾是否送达」的证据源，并在其中自装 pet-chat-reply 监听。
  try { await evl(mainWin, "window.__TAURI_INTERNALS__.invoke('open_plugin_window',{pluginId:'pet',key:null,input:'',hidden:false})"); }
  catch (e) { log("open pet window err", String(e)); }
  const pet = await findPet();
  record("桌宠窗已开(证据源)", !!pet);
  if (!pet) return finish(pid, 1);
  const listenOk = await installPetListener(pet);
  record("桌宠窗监听已装", listenOk === true, String(listenOk));
  // 自检：从主窗定向发一条 pet-chat-reply → 桌宠窗必须收到（先证明探针本身有效）
  await evl(mainWin, "window.__TAURI_INTERNALS__.invoke('plugin:event|emit_to', { target:{kind:'AnyLabel',label:'plugin-pet'}, event:'pet-chat-reply', payload:{text:'__selftest__'} })");
  await sleep(1200);
  const selfGot = await readPetGot(pet);
  record("探针自检(定向投递可达桌宠窗)", selfGot.some((p) => p && p.text === "__selftest__"), "got=" + JSON.stringify(selfGot));
  await resetPetGot(pet);

  // ══ CASE 1：桌宠发一句 → 正常送达（聊天气泡含 51，且桌宠窗收到最终回复）
  log("CASE1: pet-chat-send 正常送达");
  await evl(mainWin, EMIT("pet-chat-send", { query: "Compute 17*3. Reply with only the number, no words.", files: [] }));
  let chat = await findKind("chat");
  if (!chat) { record("CASE1 聊天窗建立", false); return finish(pid, 1); }
  const r1 = await waitForText(chat, "51", 90000);
  record("CASE1 桌宠轮正常送达(聊天气泡含 51)", r1.ok);
  const p1 = await waitPetGot(pet, /51/, 20000);
  record("CASE1 桌宠窗收到最终回复(含 51)", p1.ok, "got=" + JSON.stringify(p1.got));
  const s1 = await agentState(mainWin);
  record("CASE1 轮末 agent 仍 ready", s1 === "ready", "state=" + s1);

  // ══ CASE 2（成本回归）：空闲时关聊天窗 → 不杀 agent、且不误发桌宠收尾
  log("CASE2: 空闲关窗（应保留 agent）");
  await resetPetGot(pet);
  await closeLikeUserX(chat);
  await sleep(2500);
  const stillChat = await findKind("chat", 2);
  const s2 = await agentState(mainWin);
  record("CASE2 空闲关窗后窗口确实关掉", !stillChat);
  record("CASE2 空闲关窗未杀 agent(仍 ready)", s2 === "ready", "state=" + s2);
  const got2 = await readPetGot(pet);
  record("CASE2 空闲关窗未误发桌宠收尾", got2.length === 0, "got=" + JSON.stringify(got2));

  // ══ CASE 3（核心）：有在飞轮次时关聊天窗 → 桌宠收到一句收尾 + agent 被停
  log("CASE3: 在飞轮次时关窗（本次核心）");
  await resetPetGot(pet);
  await evl(mainWin, EMIT("pet-chat-send", { query: "Count from 1 to 400 slowly, one number per line.", files: [] }));
  chat = await findKind("chat");
  if (!chat) { record("CASE3 聊天窗建立", false); return finish(pid, 1); }
  const streamed = await waitStreaming(chat, 60000);
  let petHidden = false;
  try { petHidden = await evl(chat, "!!document.getElementById('pet-hidden-flow')"); } catch {}
  record("CASE3 轮次确实进入流式(且确认是桌宠轮)", streamed && petHidden, "petHidden=" + petHidden);
  await closeLikeUserX(chat);
  const p3 = await waitPetGot(pet, /中断|interrupt/i, 15000);
  record("CASE3 桌宠收到一句收尾(含「中断」)", p3.ok, "got=" + JSON.stringify(p3.got));
  await sleep(1500);
  const stillChat3 = await findKind("chat", 2);
  record("CASE3 在飞关窗后窗口确实关掉", !stillChat3);
  const s3 = await agentState(mainWin);
  record("CASE3 在飞关窗已停 agent(应为 idle)", s3 === "idle", "state=" + s3);

  // ══ CASE 4（核心续）：重开聊天窗后，下一轮不被旧 result 吃掉
  log("CASE4: 重开聊天窗 → 下一轮不被旧 result 吃掉");
  await evl(mainWin, EMIT("pet-chat-send", { query: "Compute 19*3. Reply with only the number, no words.", files: [] }));
  const chat4 = await findKind("chat");
  if (!chat4) { record("CASE4 聊天窗重建", false); return finish(pid, 1); }
  const r4 = await waitForText(chat4, "57", 90000);
  record("CASE4 下一轮真实回复送达(含 57)", r4.ok);
  // 清场：确认 CASE4 这一轮**已收尾**再开 CASE5。`waitForText` 可能在流式途中就命中 "57"
  // （此刻 `isStreaming` 仍为 true）⇒ 不等待的话 CASE5 的 `startAIChat` 会被 `isStreaming` 静默丢弃。
  const midStream4 = await streamingOn(chat4); // 命中瞬间是否仍在流式 = 竞态窗口的直接证据
  const idle4 = await waitTurnIdle(chat4, 60000);
  record("CASE4 轮次已收尾(为 CASE5 清场)", idle4, `命中瞬间 streaming=${midStream4}`);

  // ══ CASE 5（回归）：同窗普通聊天不受影响
  log("CASE5: 同窗普通聊天");
  try { await evl(chat4, "window.__lunac_start_ai_chat('Compute 23*3. Reply with only the number, no words.')"); }
  catch (e) { log("CASE5 invoke err", String(e)); }
  const r5 = await waitForText(chat4, "69", 90000);
  record("CASE5 普通聊天正常(含 69)", r5.ok);
  await waitTurnIdle(chat4, 60000); // 同上：CASE6 也要在同一窗里发起新一轮，先等这一轮收尾

  // ══ CASE 6（回归）：pet-chat-stop 中止仍有效
  log("CASE6: pet-chat-stop 中止");
  try {
    await evl(chat4, "window.__lunac_start_ai_chat('Count from 1 to 3000 slowly, one number per line.')");
    await sleep(3000);
    await evl(mainWin, EMIT("pet-chat-stop", {}));
    await sleep(1500);
    record("CASE6 pet-chat-stop 已送达无异常", true);
  } catch (e) { record("CASE6 pet-chat-stop 已送达无异常", false, String(e)); }

  const failed = results.filter((x) => !x.ok);
  log("──────── 汇总 ────────");
  for (const r of results) log(`  ${r.ok ? "PASS" : "FAIL"}  ${r.name}${r.detail ? "  — " + r.detail : ""}`);
  log(`总计 ${results.length} 项，失败 ${failed.length} 项`);
  finish(pid, failed.length ? 1 : 0);
}

function finish(pid, code) {
  try { spawnSync("taskkill", ["/F", "/T", "/PID", String(pid)], { stdio: "ignore" }); } catch {}
  log("done, killing app. exit", code);
  process.exit(code);
}

main().catch((e) => { log("EXCEPTION", e?.stack || String(e)); process.exit(2); });
