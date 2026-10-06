// scripts/mcp-test-server.mjs — Lunac MCP + OAuth 本机测试服务器（回归夹具，2026-10-05）
//
// 一个进程同时扮演三件事：
//   ① MCP Streamable HTTP 服务器（POST /mcp：initialize / notifications/initialized /
//      tools/list / tools/call），只暴露一个工具 `echo`；
//   ② OAuth 2.1 授权服务器：RFC 9728 受保护资源元数据 + RFC 8414 授权服务器元数据 +
//      RFC 7591 动态注册 + Authorization Code + PKCE(S256) + refresh_token；
//   ③ 控制面（测试用）：GET /__stats 看计数、GET /__expire 让当前 access_token 立即失效。
//
// **授权端点自动同意**（不弹登录页，直接 302 回 code）—— 这样整条 OAuth 链路可无人值守地跑完，
// 而 agent 侧该走的步骤（起环回端口 → 拉浏览器 → 等回调 → 换令牌 → 落盘）一步都不少。
//
// 用途：验收 `agent` 的远端 MCP 接入（`config\mcp.json` 的 `servers[]`）与 OAuth 2.1 授权
// （见 docs/agent-feature-backlog.md 的 M2-7 ③⑤）。跑法（任选一个临时目录放配置与输入）：
//
//   # 1) 起服务器
//   node scripts/mcp-test-server.mjs                    # MODE 缺省 = static
//   $env:MODE="oauth"; node scripts/mcp-test-server.mjs # OAuth 模式
//   $env:REJECT_FIRST="1"                               # 额外让第一个 /mcp 请求回 401
//
//   # 2) 配置（<dir>\mcp.json）
//   { "servers": [ { "name": "e2e", "url": "http://127.0.0.1:8791/mcp",
//                    "auth": "static", "headers": { "Authorization": "Bearer static-test-token" } } ] }
//   #    OAuth 模式把上面那条换成 { "name": "e2e", "url": "http://127.0.0.1:8791/mcp", "auth": "oauth" }
//
//   # 3) 跑 agent（stdin 每行一条 JSON；给它一条 user 消息让它调 mcp__echo）
//   $env:LUNAC_MCP_FILE="<dir>\mcp.json"
//   & { '<user 消息那行 JSON>'; Start-Sleep -Seconds 55 } | agent.exe
//
// 判据：`system/init` 的 tools 里出现 `mcp__echo`；模型调用后 tool_result 为 `echo: <text>`。
// OAuth 模式再看 `GET /__stats`：首次授权 authorize/register/token 各 +1，且 `mcp-tokens.json`
// 落在 `mcp.json` 同级；**重启后** authorize/register 保持 0（不再开浏览器），401 触发 `refreshed` +1。
//
// 注意：本服务器**不持久化**（重启即清内存）。为了“重启后旧 refresh_token 仍可用”这一
// 真实行为（真实 AS 会记得），它对形如 `rt-*` 的未知 refresh_token 也放行 —— 见 `/token`。

import http from 'node:http';
import crypto from 'node:crypto';

const PORT = Number(process.env.PORT || 8791);
const MODE = (process.env.MODE || 'static').toLowerCase(); // static | oauth
const REJECT_FIRST = process.env.REJECT_FIRST === '1';
const ORIGIN = `http://127.0.0.1:${PORT}`;
const STATIC_TOKEN = 'static-test-token';

const stats = { authorize: 0, token: 0, register: 0, mcp: 0, unauthorized: 0, toolCalls: 0, refreshed: 0 };
const clients = new Map(); // client_id -> { redirect_uris }
const codes = new Map();   // code -> { client_id, redirect_uri, challenge, resource, used }
const access = new Map();  // access_token -> {}
const refresh = new Map(); // refresh_token -> client_id
// 「拒绝一次」是**全局一次**（不是每个 token 一次）：第一次 /mcp 回 401，逼客户端走
// 「刷新令牌 → 原样重发」那条路。若按 token 计，刷新出来的新 token 又会被拒一次，
// 而客户端只重试一次 ⇒ 反而永远连不上（这是本夹具第一版的 bug）。
let rejectLeft = REJECT_FIRST ? 1 : 0;

const TOOL = {
  name: 'echo',
  description: 'Echo back the provided text. (Lunac MCP e2e test tool)',
  inputSchema: {
    type: 'object',
    properties: { text: { type: 'string', description: 'text to echo back' } },
    required: ['text'],
  },
};

const log = (...a) => console.error('[mcp-e2e]', ...a);
const json = (res, code, obj, headers = {}) => {
  const b = JSON.stringify(obj);
  res.writeHead(code, { 'content-type': 'application/json', 'content-length': Buffer.byteLength(b), ...headers });
  res.end(b);
};
const text = (res, code, s, ct = 'text/plain') => { res.writeHead(code, { 'content-type': ct }); res.end(s); };
const readBody = (req) => new Promise((r) => { let b = ''; req.on('data', (c) => (b += c)); req.on('end', () => r(b)); });
const parseForm = (s) => Object.fromEntries(new URLSearchParams(s));
const s256 = (v) => crypto.createHash('sha256').update(String(v)).digest('base64url');

function mint(clientId, keepRefresh) {
  const at = 'at-' + crypto.randomBytes(10).toString('hex');
  const rt = keepRefresh || 'rt-' + crypto.randomBytes(10).toString('hex');
  access.set(at, {});
  refresh.set(rt, clientId);
  return { access_token: at, token_type: 'Bearer', expires_in: 3600, refresh_token: rt, scope: 'mcp' };
}

const server = http.createServer(async (req, res) => {
  const u = new URL(req.url, ORIGIN);
  const p = u.pathname;

  // ── 控制面 ───────────────────────────────────────────────────────
  if (p === '/__stats') return json(res, 200, { mode: MODE, rejectFirst: REJECT_FIRST, ...stats });
  if (p === '/__expire') { access.clear(); return json(res, 200, { ok: true }); }

  // ── RFC 9728 受保护资源元数据（客户端两种拼法都试，这里都答）────────
  if (p === '/.well-known/oauth-protected-resource' || p === '/.well-known/oauth-protected-resource/mcp') {
    return json(res, 200, { resource: `${ORIGIN}/mcp`, authorization_servers: [ORIGIN] });
  }
  // ── RFC 8414 授权服务器元数据 ────────────────────────────────────
  if (p === '/.well-known/oauth-authorization-server' || p === '/.well-known/oauth-authorization-server/mcp') {
    return json(res, 200, {
      issuer: ORIGIN,
      authorization_endpoint: `${ORIGIN}/authorize`,
      token_endpoint: `${ORIGIN}/token`,
      registration_endpoint: `${ORIGIN}/register`,
      response_types_supported: ['code'],
      grant_types_supported: ['authorization_code', 'refresh_token'],
      code_challenge_methods_supported: ['S256'],
      token_endpoint_auth_methods_supported: ['none'],
      scopes_supported: ['mcp'],
    });
  }
  // ── RFC 7591 动态客户端注册 ──────────────────────────────────────
  if (p === '/register' && req.method === 'POST') {
    stats.register++;
    let body = {};
    try { body = JSON.parse((await readBody(req)) || '{}'); } catch { /* ignore */ }
    const id = 'client-' + crypto.randomBytes(6).toString('hex');
    const uris = Array.isArray(body.redirect_uris) ? body.redirect_uris : [];
    clients.set(id, { redirect_uris: uris });
    log('register ->', id, uris);
    return json(res, 200, { client_id: id, redirect_uris: uris, token_endpoint_auth_method: 'none' });
  }
  // ── 授权端点（测试用：自动同意）──────────────────────────────────
  if (p === '/authorize') {
    stats.authorize++;
    const q = u.searchParams;
    const clientId = q.get('client_id') || '';
    const redirect = q.get('redirect_uri') || '';
    const c = clients.get(clientId);
    log('authorize', { clientId, redirect, ck: q.get('code_challenge_method') });
    if (!c) return text(res, 400, 'unknown client_id');
    if (!c.redirect_uris.includes(redirect)) return text(res, 400, 'redirect_uri not registered');
    if (q.get('code_challenge_method') !== 'S256') return text(res, 400, 'PKCE S256 required');
    const code = crypto.randomBytes(16).toString('hex');
    codes.set(code, {
      client_id: clientId,
      redirect_uri: redirect,
      challenge: q.get('code_challenge') || '',
      resource: q.get('resource') || '',
      used: false,
    });
    const loc = `${redirect}?code=${encodeURIComponent(code)}&state=${encodeURIComponent(q.get('state') || '')}`;
    res.writeHead(302, { location: loc });
    return res.end();
  }
  // ── 令牌端点 ────────────────────────────────────────────────────
  if (p === '/token' && req.method === 'POST') {
    stats.token++;
    const f = parseForm(await readBody(req));
    if (f.grant_type === 'authorization_code') {
      const rec = codes.get(f.code || '');
      log('token:authorization_code', { ok: !!rec, verifier: !!f.code_verifier });
      if (!rec || rec.used) return json(res, 400, { error: 'invalid_grant' });
      if (rec.redirect_uri !== f.redirect_uri) return json(res, 400, { error: 'invalid_grant', error_description: 'redirect_uri mismatch' });
      if (s256(f.code_verifier || '') !== rec.challenge) return json(res, 400, { error: 'invalid_grant', error_description: 'PKCE verification failed' });
      rec.used = true;
      return json(res, 200, mint(rec.client_id));
    }
    if (f.grant_type === 'refresh_token') {
      // 本夹具**不持久化**：进程重启后 refresh 表是空的。真实 AS 会记得已签发的
      // refresh_token，所以这里对「形如 rt-* 的未知 token」也放行，**模拟重启后仍有效的 AS**。
      const known = refresh.get(f.refresh_token || '');
      const clientId = known || (String(f.refresh_token || '').startsWith('rt-') ? 'restored-client' : null);
      log('token:refresh_token', { known: !!known, accepted: !!clientId });
      if (!clientId) return json(res, 400, { error: 'invalid_grant' });
      stats.refreshed++;
      return json(res, 200, mint(clientId, f.refresh_token));
    }
    return json(res, 400, { error: 'unsupported_grant_type' });
  }
  // ── MCP Streamable HTTP ─────────────────────────────────────────
  if (p === '/mcp' && req.method === 'POST') {
    stats.mcp++;
    const auth = String(req.headers['authorization'] || '');
    const tok = auth.replace(/^Bearer\s+/i, '');
    let ok = MODE === 'static' ? auth === `Bearer ${STATIC_TOKEN}` : access.has(tok);
    if (ok && MODE === 'oauth' && rejectLeft > 0) {
      // 模拟「服务端不认这份令牌」（过期 / 轮换）—— 专为验 401 → 只刷新不开浏览器 而设
      rejectLeft--;
      ok = false;
      log('mcp: forced 401 (once)');
    }
    if (!ok) {
      stats.unauthorized++;
      log('mcp -> 401');
      return json(res, 401, { error: 'invalid_token' }, {
        'www-authenticate': `Bearer error="invalid_token", resource_metadata="${ORIGIN}/.well-known/oauth-protected-resource"`,
      });
    }
    let body = {};
    try { body = JSON.parse((await readBody(req)) || '{}'); } catch { /* ignore */ }
    const method = body.method || '(none)';
    log('mcp', method, body.id ?? '(notification)');
    if (body.id === undefined || body.id === null) { res.writeHead(202); return res.end(); }
    if (method === 'initialize') {
      res.setHeader('mcp-session-id', 'sess-' + crypto.randomBytes(8).toString('hex'));
      return json(res, 200, {
        jsonrpc: '2.0', id: body.id,
        result: { protocolVersion: '2025-06-18', capabilities: { tools: {} }, serverInfo: { name: 'lunac-mcp-e2e', version: '0.1.0' } },
      });
    }
    if (method === 'tools/list') return json(res, 200, { jsonrpc: '2.0', id: body.id, result: { tools: [TOOL] } });
    if (method === 'tools/call') {
      stats.toolCalls++;
      const args = (body.params && body.params.arguments) || {};
      const echoed = `echo: ${args.text ?? '(no text)'}`;
      log('tools/call ->', echoed);
      return json(res, 200, { jsonrpc: '2.0', id: body.id, result: { content: [{ type: 'text', text: echoed }], isError: false } });
    }
    return json(res, 200, { jsonrpc: '2.0', id: body.id, error: { code: -32601, message: `Method not found: ${method}` } });
  }

  text(res, 404, 'not found');
});

server.listen(PORT, '127.0.0.1', () => log(`listening on ${ORIGIN} mode=${MODE} rejectFirst=${REJECT_FIRST}`));
