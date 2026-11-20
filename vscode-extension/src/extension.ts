// ── Extension entry point ─────────────────────────────────────────
// Lunac VS Code Extension
// Path A: Chat Participant (native VS Code Chat integration)
// Path B: Webview Panel (standalone UI pane)

import * as vscode from "vscode";
import { registerChatParticipant } from "./chatParticipant";
import { registerWebviewPanel } from "./webviewPanel";
import { registerChatView, askInChat } from "./chatView";

export function activate(context: vscode.ExtensionContext) {
  // ── URI Handler: vscode://lunac.lunac/chat → open Lunac Chat ──
  // Triggered by the Lunac desktop app's "Attach to VSCode" button.
  context.subscriptions.push(
    vscode.window.registerUriHandler({
      async handleUri(uri: vscode.Uri) {
        if (uri.path === "/chat") {
          // Focus the Lunac AI Chat view in the activity bar
          await vscode.commands.executeCommand("lunac.focusChat");
        }
      },
    }),
  );

  // ── Focus Chat command ───────────────────────────────────────
  context.subscriptions.push(
    vscode.commands.registerCommand("lunac.focusChat", () => {
      // Opens the Lunac activity bar view
      vscode.commands.executeCommand("workbench.view.extension.lunac-sidebar");
    }),
  );

  // ── Status bar indicator ─────────────────────────────────────
  const providerName =
    vscode.workspace.getConfiguration("lunac").get<string>("provider") || "deepseek";

  const statusBarItem = vscode.window.createStatusBarItem(
    vscode.StatusBarAlignment.Right,
    100,
  );
  statusBarItem.text = `$(sparkle) Lunac · ${providerName}`;
  statusBarItem.tooltip = "Lunac AI Chat";
  statusBarItem.command = "lunac.focusChat";
  statusBarItem.show();
  context.subscriptions.push(statusBarItem);

  // ── Register Chat Participant (Path A) ───────────────────────
  registerChatParticipant(context);

  // ── Register Webview Panel (Path B) ──────────────────────────
  registerWebviewPanel(context);

  // ── Register Chat WebviewView (Path C: Secondary Sidebar) ─────
  registerChatView(context);

  // ── Ask about the current file / selection (§3-23 分片 1) ─────
  // 只拼「路径 + 行范围」，**不贴文件内容**：agent 自己有 Read，贴一遍等于
  // 把同一段代码付两次钱，而且贴的是编辑器里的旧文本（磁盘可能已经变了）。
  context.subscriptions.push(
    vscode.commands.registerCommand("lunac.askAboutFile", async (arg?: unknown) => {
      const uri = targetUri(arg);
      if (!uri) {
        void vscode.window.showInformationMessage(
          "Lunac：先打开一个文件，或从资源管理器里右键一个文件。",
        );
        return;
      }
      const rel = vscode.workspace.asRelativePath(uri, false).replace(/\\/g, "/");
      const range = selectedRange(uri);
      const scope = range ? `第 ${range.startLine + 1}–${range.endLine + 1} 行` : "整个文件";
      const text =
        `请分析 \`${rel}\`（${scope}）。先读它，再说清它做什么、有哪些风险点；` +
        `不确定的地方单独标成未验证，不要猜。`;
      const res = askInChat(text);
      if (!res.ok) {
        void vscode.window.showErrorMessage(`Lunac：${res.error}`);
        return;
      }
      await vscode.commands.executeCommand("lunac.focusChat");
    }),
  );

  // ── Listen for config changes → update status bar ────────────
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration("lunac.provider")) {
        const newProvider =
          vscode.workspace.getConfiguration("lunac").get<string>("provider") || "deepseek";
        statusBarItem.text = `$(sparkle) Lunac · ${newProvider}`;
      }
    }),
  );
}

export function deactivate() {
  // No cleanup needed — VS Code tears down participants and panels.
}

/**
 * 命令的「问谁」：右键传 `Uri`（资源管理器），命令面板 / 编辑器右键走当前编辑器。
 * 两种情况都拿不到 ⇒ 返回 `undefined`，由调用方给一句可操作提示。
 */
function targetUri(arg?: unknown): vscode.Uri | undefined {
  if (arg instanceof vscode.Uri) return arg;
  return vscode.window.activeTextEditor?.document.uri;
}

/** 当前编辑器里与 `uri` 同一个文件、且非空选区的行范围（0 基）。 */
function selectedRange(uri: vscode.Uri): { startLine: number; endLine: number } | undefined {
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.uri.toString() !== uri.toString()) return undefined;
  if (editor.selection.isEmpty) return undefined;
  return {
    startLine: editor.selection.start.line,
    endLine: editor.selection.end.line,
  };
}
