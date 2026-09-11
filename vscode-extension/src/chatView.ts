// ── Lunac AI Chat WebviewView (sidebar, Trae-style right panel) ───
// Directly drives cli.exe via the stream-json protocol — NO dependency on
// the Lunac desktop app or its HTTP bridge, so Lunac needs no installer.
//
// Stream-json protocol (same as the desktop frontend):
//   stdin  line:  {"type":"user","message":{"role":"user","content":"..."}}
//   stdin  line:  {"type":"control_response","response":{"subtype":"success",
//                   "request_id":..., "response":{"behavior":"allow"|"deny",...}}}
//   stdout line:  {"type":"system"|"assistant"|"user"|"stream_event"|"control_request"|...}
//
// Spawn args mirror app/src-tauri/src/commands.rs start_cli_process:
//   cli.exe --print --verbose --input-format stream-json --output-format
//   stream-json --include-partial-messages --permission-prompt-tool stdio
//   --permission-mode acceptEdits --add-dir <workdir> .

import * as vscode from "vscode";
import { spawn, ChildProcessWithoutNullStreams } from "child_process";
import { existsSync } from "fs";
import { join } from "path";
import { resolveConfig } from "./providers";

export function registerChatView(context: vscode.ExtensionContext) {
  const provider = new ChatViewProvider(context);
  context.subscriptions.push(
    vscode.window.registerWebviewViewProvider("lunac.chatView", provider, {
      webviewOptions: { retainContextWhenHidden: true },
    })
  );
}

// ── CLI process session ──────────────────────────────────────────

class CliAgentSession {
  private child: ChildProcessWithoutNullStreams | null = null;
  private stdoutBuf = "";

  get running(): boolean {
    return this.child !== null && this.child.exitCode === null;
  }

  /** Resolve cli.exe: config → extension dir release/Lunac → user home .lunac → env. */
  static resolveCliPath(context: vscode.ExtensionContext): string | null {
    const cfg = vscode.workspace.getConfiguration("lunac");
    const configured = cfg.get<string>("cliPath");
    if (configured && existsSync(configured)) return configured;

    const candidates = [
      join(context.extensionPath, "..", "release", "Lunac", "cli.exe"),
      join(context.extensionPath, "cli.exe"),
      join(process.env.USERPROFILE || process.env.HOME || "", ".lunac", "cli.exe"),
    ];
    if (process.env.LUNAC_CLI_PATH && existsSync(process.env.LUNAC_CLI_PATH)) {
      candidates.unshift(process.env.LUNAC_CLI_PATH);
    }
    for (const p of candidates) {
      if (existsSync(p)) return p;
    }
    return null;
  }

  /** Start cli.exe with the stream-json agent protocol. */
  start(context: vscode.ExtensionContext, onEvent: (obj: Record<string, unknown>) => void): { ok: boolean; error?: string } {
    if (this.running) return { ok: true };

    const cliPath = CliAgentSession.resolveCliPath(context);
    if (!cliPath) {
      return { ok: false, error: "cli.exe not found. Install Lunac (portable) or set lunac.cliPath." };
    }

    // Workspace root = agent working directory (falls back to home dir)
    const workdir =
      vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ||
      process.env.USERPROFILE ||
      process.env.HOME ||
      process.cwd();

    // Provider env (same mapping as the desktop app's ensure_agent_running)
    const provider = vscode.workspace.getConfiguration("lunac").get<string>("provider") || "deepseek";
    let env: NodeJS.ProcessEnv = { ...process.env };
    try {
      const cfg = resolveConfig(provider);
      const base = cfg.apiUrl.replace(/\/+$/, "");
      // Providers whose Anthropic endpoint is NOT base+"/anthropic" (e.g. Zhipu)
      // are handled via lunac.apiUrl already pointing at the anthropic route.
      env = {
        ...process.env,
        ANTHROPIC_BASE_URL: provider === "anthropic" ? base : `${base}/anthropic`,
        ANTHROPIC_AUTH_TOKEN: cfg.apiKey,
        ANTHROPIC_MODEL: cfg.model,
        ANTHROPIC_SMALL_FAST_MODEL: cfg.model,
        ANTHROPIC_CLI_DISABLE_TELEMETRY: "true",
      };
      delete env.ANTHROPIC_API_KEY; // avoid x-api-key overriding Bearer auth
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      return { ok: false, error: `Provider config: ${msg}` };
    }

    // Ripgrep vendor dir must be on PATH for the compiled cli.exe
    const rgDir = join(context.extensionPath, "..", "release", "Lunac", "utils", "vendor", "ripgrep", "x64-win32");
    if (existsSync(join(rgDir, "rg.exe"))) {
      env = { ...env, PATH: `${rgDir};${env.PATH || ""}`, USE_BUILTIN_RIPGREP: "0" };
    }

    const args = [
      "--print",
      "--verbose",
      "--input-format", "stream-json",
      "--output-format", "stream-json",
      "--include-partial-messages",
      "--permission-prompt-tool", "stdio",
      "--permission-mode", "acceptEdits",
      "--add-dir", workdir,
      ".",
    ];

    try {
      this.child = spawn(cliPath, args, {
        cwd: workdir,
        env,
        windowsHide: true,
      });
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      return { ok: false, error: `Failed to start cli.exe: ${msg}` };
    }

    const child = this.child;
    this.stdoutBuf = "";

    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      this.stdoutBuf += chunk;
      const lines = this.stdoutBuf.split(/\r?\n/);
      this.stdoutBuf = lines.pop() || ""; // keep incomplete tail
      for (const line of lines) {
        const trimmed = line.trim();
        if (!trimmed || !trimmed.startsWith("{")) continue;
        try {
          const obj = JSON.parse(trimmed) as Record<string, unknown>;
          onEvent(obj);
        } catch {
          // non-JSON debug line — ignore
        }
      }
    });

    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (chunk: string) => {
      const text = chunk.toString().trim();
      if (text) console.log(`[lunac cli] ${text}`);
    });

    child.on("exit", (code) => {
      this.child = null;
      onEvent({ type: "system", subtype: "closed", code });
    });
    child.on("error", (err) => {
      this.child = null;
      onEvent({ type: "system", subtype: "error", message: err.message });
    });

    return { ok: true };
  }

  /** Write one JSON line to cli.exe stdin. */
  send(obj: unknown): boolean {
    if (!this.running || !this.child) return false;
    this.child.stdin.write(JSON.stringify(obj) + "\n");
    return true;
  }

  stop() {
    if (this.child && this.child.exitCode === null) {
      this.child.stdin.end();
      this.child.kill();
    }
    this.child = null;
  }
}

// ── Webview provider ─────────────────────────────────────────────

class ChatViewProvider implements vscode.WebviewViewProvider {
  private session = new CliAgentSession();

  constructor(private readonly context: vscode.ExtensionContext) {}

  resolveWebviewView(webviewView: vscode.WebviewView) {
    webviewView.webview.options = { enableScripts: true };
    webviewView.webview.html = getChatHtml();

    const post = (obj: unknown) => webviewView.webview.postMessage(obj);

    webviewView.webview.onDidReceiveMessage((msg) => {
      switch (msg.type) {
        case "checkStatus": {
          const online = this.session.running;
          const provider = vscode.workspace.getConfiguration("lunac").get<string>("provider") || "deepseek";
          post({ type: "status", online, provider });
          break;
        }
        case "startAgent": {
          const res = this.session.start(this.context, (ev) => post(ev));
          post({ type: "agentStarted", ok: res.ok, message: res.error || "Agent ready" });
          break;
        }
        case "stopAgent": {
          this.session.stop();
          post({ type: "status", online: false, provider: "" });
          break;
        }
        case "sendMessage": {
          const line = { type: "user", message: { role: "user", content: String(msg.text || "") } };
          if (!this.session.send(line)) {
            post({ type: "error", message: "Agent not running. Start it first." });
          }
          break;
        }
        case "respondApproval": {
          const inner = msg.allow
            ? { behavior: "allow", updatedInput: {}, toolUseID: msg.toolUseId }
            : { behavior: "deny", message: "User denied in Lunac (VSCode)", interrupt: false, toolUseID: msg.toolUseId };
          this.session.send({
            type: "control_response",
            response: { subtype: "success", request_id: msg.requestId, response: inner },
          });
          break;
        }
        case "clearChat": {
          post({ type: "clear" });
          break;
        }
      }
    });
  }
}

// ── Webview HTML (Lunac glassmorphism style, VSCode theme vars) ──

function getChatHtml(): string {
  return `<!DOCTYPE html>
<html lang="zh-CN"><head><meta charset="UTF-8"><meta name="viewport" content="width=device-width, initial-scale=1.0">
<style>
:root{--bg:var(--vscode-sideBar-background,#1e1e2e);--fg:var(--vscode-sideBar-foreground,#cdd6f4);
--input-bg:var(--vscode-input-background,#313244);--border:var(--vscode-input-border,#45475a);
--accent:var(--vscode-button-background,#89b4fa);--accent-fg:var(--vscode-button-foreground,#1e1e2e);
--tool-bg:var(--vscode-editor-background,#181825);--dim:var(--vscode-descriptionForeground,#6c7086);
--red:var(--vscode-errorForeground,#f38ba8);--green:#a6e3a1;--yellow:#f9e2af}
*{box-sizing:border-box;margin:0;padding:0}
body{font-family:var(--vscode-font-family);font-size:13px;color:var(--fg);background:var(--bg);
display:flex;flex-direction:column;height:100vh;overflow:hidden}
#status{display:flex;align-items:center;gap:8px;padding:8px 12px;font-size:11.5px;color:var(--dim);
border-bottom:1px solid var(--border);flex-shrink:0}
#status .dot{width:8px;height:8px;border-radius:50%;background:var(--dim)}
#status .dot.on{background:var(--green)}#status .dot.off{background:var(--red)}
#status .provider{flex:1;text-align:right;opacity:.7}
#chat{flex:1;overflow-y:auto;padding:10px 12px;scrollbar-width:thin}
.msg{margin-bottom:8px;line-height:1.55}
.msg.user{background:rgba(137,180,250,.12);border:1px solid rgba(137,180,250,.25);
border-radius:10px;padding:6px 10px;white-space:pre-wrap;word-break:break-word}
.msg.assistant{white-space:pre-wrap;word-break:break-word}
.msg.assistant.streaming{border-right:2px solid var(--accent);padding-right:4px}
.thinking{color:var(--dim);font-style:italic;padding:2px 0;margin-bottom:4px}
.thinking summary{cursor:pointer;list-style:none;font-size:12px}
.thinking summary::-webkit-details-marker{display:none}
.thinking summary::before{content:"▸ "}
.thinking[open] summary::before{content:"▾ "}
.thinking .body{white-space:pre-wrap;font-size:12px}
.tool-card{background:var(--tool-bg);border:1px solid var(--border);border-radius:8px;
padding:6px 10px;margin:6px 0;font-size:12px}
.tool-card .name{color:var(--accent);font-weight:600}
.tool-card .input{color:var(--fg);font-family:Consolas,monospace;font-size:11px;margin-top:3px;
word-break:break-all;white-space:pre-wrap}
.tool-card .result{color:var(--green);font-family:Consolas,monospace;font-size:11px;
margin-top:4px;border-top:1px solid var(--border);padding-top:4px;white-space:pre-wrap;word-break:break-word}
.tool-card .result.error{color:var(--red)}
.tool-card.ok{border-left:2px solid var(--green)}
.tool-card .summary{cursor:pointer;list-style:none}
.tool-card .summary::-webkit-details-marker{display:none}
.tool-card .summary::before{content:"▸ "}
.tool-card[open] .summary::before{content:"▾ "}
#approval{display:none;flex-shrink:0;margin:0 12px 8px;background:var(--tool-bg);
border:1px solid var(--yellow);border-radius:10px;padding:8px 10px}
#approval.show{display:block}
#approval .title{font-size:12px;color:var(--yellow);margin-bottom:6px}
#approval .cmd{font-family:Consolas,monospace;font-size:11.5px;word-break:break-all;
white-space:pre-wrap;margin-bottom:8px}
#approval .btns{display:flex;gap:8px}
#approval button{flex:1;padding:5px 0;border:none;border-radius:6px;cursor:pointer;font-size:12px;font-family:inherit}
#approval .allow{background:var(--accent);color:var(--accent-fg)}
#approval .deny{background:transparent;color:var(--fg);border:1px solid var(--border)}
#input-area{display:flex;gap:8px;padding:8px 12px;border-top:1px solid var(--border);flex-shrink:0}
#input-area textarea{flex:1;background:var(--input-bg);color:var(--fg);border:1px solid var(--border);
border-radius:8px;padding:6px 8px;resize:none;font-family:inherit;font-size:13px;min-height:32px;max-height:120px}
#input-area textarea:focus{outline:none;border-color:var(--accent)}
#input-area button{background:var(--accent);color:var(--accent-fg);border:none;padding:6px 12px;
border-radius:8px;cursor:pointer;font-size:12px;font-family:inherit}
#input-area button:disabled{opacity:.5;cursor:not-allowed}
.empty{color:var(--dim);text-align:center;padding:24px 8px;font-size:12px}
</style></head><body>
<div id="status"><span class="dot off" id="dot"></span><span id="statusText">未连接</span>
<span class="provider" id="providerText"></span></div>
<div id="chat"><div class="empty" id="welcome">启动 Agent 后开始对话</div></div>
<div id="approval"><div class="title">⚠ 权限请求</div><div class="cmd" id="approvalCmd"></div>
<div class="btns"><button class="allow" id="approvalAllow">允许</button>
<button class="deny" id="approvalDeny">拒绝</button></div></div>
<div id="input-area"><textarea id="msgInput" placeholder="输入消息，Enter 发送 / Shift+Enter 换行…" rows="1"></textarea>
<button id="sendBtn">发送</button></div>
<script>
(function(){
var vscode=acquireVsCodeApi();
var chat=document.getElementById("chat"),input=document.getElementById("msgInput");
var sendBtn=document.getElementById("sendBtn"),dot=document.getElementById("dot");
var statusText=document.getElementById("statusText"),providerText=document.getElementById("providerText");
var approval=document.getElementById("approval"),approvalCmd=document.getElementById("approvalCmd");
var online=false;
function esc(s){return String(s).replace(/&/g,"&amp;").replace(/</g,"&lt;").replace(/>/g,"&gt;")}
function setStatus(on,provider){online=on;dot.className="dot "+(on?"on":"off");
statusText.textContent=on?"Agent 在线":"未连接";providerText.textContent=provider||""}
function scrollBottom(){chat.scrollTop=chat.scrollHeight}
function addMsg(role,text){var d=document.createElement("div");d.className="msg "+role;
d.textContent=text;chat.appendChild(d);scrollBottom();return d}
var welcome=document.getElementById("welcome");
// ── stream rendering state ──
var cur={}; // current block: {kind, el, elText, toolArgs, toolInputEl, toolName}
function blockStart(cb){
  if(cb.type==="thinking"){
    var det=document.createElement("details");det.className="thinking";
    det.innerHTML="<summary>思考中…</summary><div class=\"body\"></div>";
    chat.appendChild(det);cur={kind:"thinking",el:det.querySelector(".body"),raw:""};
  } else if(cb.type==="tool_use"){
    var tc=document.createElement("div");tc.className="tool-card";
    tc.innerHTML="<div class=\"name\">🔧 "+esc(cb.name||"tool")+"</div><div class=\"input\"></div>";
    chat.appendChild(tc);cur={kind:"tool",el:tc,input:tc.querySelector(".input"),name:cb.name||"",raw:""};
  } else {
    var m=document.createElement("div");m.className="msg assistant streaming";
    chat.appendChild(m);cur={kind:"text",el:m,raw:""};
  }
  if(welcome){welcome.remove();welcome=null}
  scrollBottom();
}
function blockDelta(delta){
  if(!cur)return;
  if(delta.type==="text_delta"&&delta.text){cur.raw+=delta.text;if(cur.kind==="text")cur.el.textContent=cur.raw;}
  else if(delta.type==="thinking_delta"&&delta.thinking){cur.raw+=delta.thinking;if(cur.kind==="thinking")cur.el.textContent=cur.raw;}
  else if(delta.type==="input_json_delta"&&delta.partial_json){
    if(cur.kind==="tool"){cur.raw+=delta.partial_json;renderToolInput(cur);}
  }
  scrollBottom();
}
function renderToolInput(c){
  var display=c.raw;
  try{var obj=JSON.parse(c.raw);
    if(obj&&typeof obj==="object"&&c.name==="Bash"&&typeof obj.command==="string")display=obj.command;
    else if(obj&&typeof obj==="object")display=JSON.stringify(obj).slice(0,300);
  }catch(e){}
  c.input.textContent=display.length>300?display.slice(0,300)+"…":display;
}
function blockStop(){
  if(cur&&cur.kind==="text")cur.el.className="msg assistant";
  if(cur&&cur.kind==="tool"){cur.el.className+=" ok";}
  cur={};
}
// ── assistant full message (fallback when no partial stream) ──
function renderAssistant(content){
  content.forEach(function(block){
    if(block.type==="thinking"&&block.thinking){var det=document.createElement("details");det.className="thinking";
      det.innerHTML="<summary>思考中…</summary><div class=\"body\">"+esc(block.thinking)+"</div>";chat.appendChild(det);}
    else if(block.type==="text"&&block.text){addMsg("assistant",block.text);}
    else if(block.type==="tool_use"){var tc=document.createElement("div");tc.className="tool-card";
      tc.innerHTML="<div class=\"name\">🔧 "+esc(block.name||"tool")+"</div><div class=\"input\">"+esc(JSON.stringify(block.input||{}))+"</div>";
      chat.appendChild(tc);}
    else if(block.type==="tool_result"){
      var ok=!block.is_error;
      var text=typeof block.content==="string"?block.content:JSON.stringify(block.content||"");
      text=text.length>600?text.slice(0,600)+"…":text;
      var tc=document.createElement("details");tc.className="tool-card "+(ok?"ok":"");
      tc.innerHTML="<summary class=\"summary\">"+(ok?"✓ 完成":"✗ 失败")+"</summary><div class=\"result"+(ok?"":" error")+"\">"+esc(text)+"</div>";
      chat.appendChild(tc);}
  });
  scrollBottom();
}
// ── permission approval ──
var pendingApproval=null;
function showApproval(req){
  pendingApproval=req;
  var display="";
  try{var obj=req.input;if(obj&&typeof obj==="object"){
      if(req.toolName==="Bash"&&typeof obj.command==="string")display=obj.command;
      else display=JSON.stringify(obj,null,1);}
    else display=String(req.input);}catch(e){display=String(req.input||"")}
  approvalCmd.textContent="工具: "+req.toolName+"\n"+display;
  approval.classList.add("show");
}
// ── host messages ──
window.addEventListener("message",function(ev){
  var data=ev.data;
  switch(data.type){
    case "status":setStatus(data.online,data.provider);break;
    case "agentStarted":
      setStatus(data.ok,data.provider||"");
      if(data.ok){statusText.textContent="Agent 已启动";var m=addMsg("assistant","Agent 已就绪，可以开始对话。");}
      else addMsg("assistant","⚠ "+data.message);
      break;
    case "error":addMsg("assistant","⚠ "+data.message);break;
    case "user":addMsg("user",data.message&&data.message.content||"");break;
    case "stream_event":{
      var evt=data.event;
      if(evt&&evt.type==="content_block_start")blockStart(evt.content_block);
      else if(evt&&evt.type==="content_block_delta")blockDelta(evt.delta);
      else if(evt&&evt.type==="content_block_stop")blockStop();
      else if(evt&&evt.type==="message_stop")blockStop();
      break;}
    case "assistant":renderAssistant(data.message&&data.message.content||[]);break;
    case "control_request":{
      if(data.request&&data.request.subtype==="can_use_tool"){
        showApproval({requestId:data.request_id,toolName:data.request.tool_name||"unknown",
          input:data.request.input,toolUseId:data.request.tool_use_id});}
      break;}
    case "system":{
      if(data.subtype==="closed"){setStatus(false,"");addMsg("assistant","Agent 已停止（exit "+data.code+"）。");
        approval.classList.remove("show");pendingApproval=null;}
      else if(data.subtype==="error")addMsg("assistant","⚠ "+data.message);
      break;}
    case "clear":chat.innerHTML="";chat.appendChild(welcome=document.createElement("div"));welcome.className="empty";welcome.textContent="启动 Agent 后开始对话";break;
  }
});
// ── input ──
function doSend(){var text=input.value.trim();if(!text||!online)return;
  vscode.postMessage({type:"sendMessage",text:text});input.value="";input.style.height="auto"}
sendBtn.addEventListener("click",doSend);
input.addEventListener("keydown",function(e){
  if(e.key==="Enter"&&!e.shiftKey){e.preventDefault();doSend()}});
input.addEventListener("input",function(){this.style.height="auto";this.style.height=Math.min(this.scrollHeight,120)+"px"});
document.getElementById("approvalAllow").addEventListener("click",function(){
  if(pendingApproval){vscode.postMessage({type:"respondApproval",requestId:pendingApproval.requestId,
    allow:true,toolUseId:pendingApproval.toolUseId});approval.classList.remove("show");pendingApproval=null;}});
document.getElementById("approvalDeny").addEventListener("click",function(){
  if(pendingApproval){vscode.postMessage({type:"respondApproval",requestId:pendingApproval.requestId,
    allow:false,toolUseId:pendingApproval.toolUseId});approval.classList.remove("show");pendingApproval=null;}});
// ── init ──
vscode.postMessage({type:"checkStatus"});
})();
</script></body></html>`;
}
