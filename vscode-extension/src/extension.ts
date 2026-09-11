// ── Extension entry point ─────────────────────────────────────────
// Lunac VS Code Extension
// Path A: Chat Participant (native VS Code Chat integration)
// Path B: Webview Panel (standalone UI pane)

import * as vscode from "vscode";
import { registerChatParticipant } from "./chatParticipant";
import { registerWebviewPanel } from "./webviewPanel";
import { registerChatView } from "./chatView";

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
