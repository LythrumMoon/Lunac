// ── Webview Panel (Path B) ────────────────────────────────────────
// Opens a side panel showing Lunac status, quick actions, and
// a link to open the full Lunac desktop app.

import * as vscode from "vscode";
import { PROVIDERS } from "./providers";

export function registerWebviewPanel(context: vscode.ExtensionContext) {
  context.subscriptions.push(
    vscode.commands.registerCommand("lunac.openPanel", () => {
      // If panel already exists, reveal it
      const existing = findExistingPanel();
      if (existing) {
        existing.reveal(vscode.ViewColumn.Beside);
        return;
      }

      const panel = vscode.window.createWebviewPanel(
        "lunacPanel",
        "Lunac",
        vscode.ViewColumn.Beside,
        {
          enableScripts: true,
          retainContextWhenHidden: true,
          localResourceRoots: [
            vscode.Uri.joinPath(context.extensionUri, "media"),
          ],
        },
      );

      panel.webview.html = getWebviewContent();
      panel.iconPath = vscode.Uri.joinPath(context.extensionUri, "media", "icon.png");

      // Handle messages from webview
      panel.webview.onDidReceiveMessage((msg) => {
        switch (msg.command) {
          case "openSettings":
            vscode.commands.executeCommand(
              "workbench.action.openSettings",
              "lunac",
            );
            break;
          case "openChat":
            // Focus the Chat view so user can use the Lunac participant
            vscode.commands.executeCommand("workbench.action.chat.focus");
            panel.dispose();
            break;
        }
      });

      panel.onDidDispose(() => {
        // Cleanup if needed
      });
    }),
  );
}

function findExistingPanel(): vscode.WebviewPanel | undefined {
  // VS Code doesn't expose a way to find panels by ID,
  // so we track via a WeakRef pattern or just always create new.
  return undefined;
}

function getWebviewContent(): string {
  const provider = vscode.workspace.getConfiguration("lunac").get<string>("provider") || "deepseek";
  const cfg = PROVIDERS[provider];
  const model = cfg ? cfg.defaultModel : "?";
  const providerName = cfg ? cfg.name : provider;

  return /* html */ `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Lunac</title>
  <style>
    :root {
      --bg: var(--vscode-editor-background);
      --fg: var(--vscode-editor-foreground);
      --accent: var(--vscode-textLink-foreground);
      --border: var(--vscode-panel-border);
      --button-bg: var(--vscode-button-background);
      --button-fg: var(--vscode-button-foreground);
      --button-hover: var(--vscode-button-hoverBackground);
      --input-bg: var(--vscode-input-background);
      --input-fg: var(--vscode-input-foreground);
      --input-border: var(--vscode-input-border);
      --desc: var(--vscode-descriptionForeground);
    }
    * { margin: 0; padding: 0; box-sizing: border-box; }
    body {
      background: var(--bg);
      color: var(--fg);
      font-family: var(--vscode-font-family);
      font-size: 13px;
      padding: 20px;
      line-height: 1.6;
    }
    h1 { font-size: 18px; font-weight: 600; margin-bottom: 4px; }
    .subtitle { color: var(--desc); font-size: 12px; margin-bottom: 20px; }
    .card {
      border: 1px solid var(--border);
      border-radius: 8px;
      padding: 16px;
      margin-bottom: 16px;
    }
    .card-title {
      font-weight: 600;
      margin-bottom: 8px;
      display: flex;
      align-items: center;
      gap: 8px;
    }
    .card-desc { color: var(--desc); font-size: 12px; margin-bottom: 12px; }
    button {
      background: var(--button-bg);
      color: var(--button-fg);
      border: none;
      border-radius: 4px;
      padding: 6px 14px;
      font-size: 12px;
      cursor: pointer;
      font-family: inherit;
    }
    button:hover { background: var(--button-hover); }
    .status-dot {
      width: 8px;
      height: 8px;
      border-radius: 50%;
      display: inline-block;
    }
    .status-dot.online { background: #4caf50; }
    .provider-tag {
      background: var(--input-bg);
      border: 1px solid var(--input-border);
      border-radius: 4px;
      padding: 4px 10px;
      font-size: 11px;
      color: var(--desc);
    }
  </style>
</head>
<body>
  <h1>&#x2728; Lunac</h1>
  <p class="subtitle">Multi-model AI Chat &middot; OCR &middot; App Launcher</p>

  <div class="card">
    <div class="card-title">
      <span class="status-dot online"></span>
      Chat Participant Ready
    </div>
    <div class="card-desc">
      Active provider: <strong>${providerName}</strong>
      <span class="provider-tag">${model}</span>
    </div>
    <button onclick="openChat()">Open Chat (Ctrl+Shift+I)</button>
    <button onclick="switchProvider()" style="margin-left:8px">Switch Provider</button>
  </div>

  <div class="card">
    <div class="card-title">&#x1F5A5;&#xFE0F; Lunac Desktop App</div>
    <div class="card-desc">
      Full-featured desktop launcher with global hotkey (Alt+Space),
      OCR, file actions, and agent mode.
    </div>
    <button onclick="openSettings()">Configure Settings</button>
  </div>

  <div class="card">
    <div class="card-title">&#x1F4CB; Quick Start</div>
    <div class="card-desc" style="margin-bottom:0">
      1. Press <strong>Ctrl+Shift+I</strong> (or Cmd+Shift+I on Mac) to open the Chat view<br>
      2. Select <strong>Lunac</strong> from the chat participant dropdown<br>
      3. Ask a question — AI responds via ${providerName}
    </div>
  </div>

  <script>
    const vscode = acquireVsCodeApi();

    function openChat() {
      vscode.postMessage({ command: "openChat" });
    }

    function openSettings() {
      vscode.postMessage({ command: "openSettings" });
    }

    function switchProvider() {
      // Trigger the configureProvider command via the VS Code command palette
      vscode.postMessage({ command: "openChat" });
    }

    // Prevent context menu on the webview
    document.addEventListener('contextmenu', (e) => e.preventDefault());
  </script>
</body>
</html>`;
}
