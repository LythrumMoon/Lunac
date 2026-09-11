// ── Shortcut manager ─────────────────────────────────────────────
// Centralized global shortcut registration and persistence.
// The hotkey is handled natively in Rust (hotkey.rs) — 默认优先
// RegisterHotKey（内核级、无键盘钩子），注册失败才回退 WH_KEYBOARD_LL。
// Recording from the settings plugin calls Rust IPC to update it in real time.

import { invoke } from "@tauri-apps/api/core";

const DEFAULT_SHORTCUT = "Ctrl+Alt+Space";
const STORAGE_KEY = "lunac_shortcut";

export function loadShortcut(): string {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved && /^(Ctrl|Alt|Shift|Meta)(\+(Ctrl|Alt|Shift|Meta))*\+.+$/.test(saved)) {
      return saved;
    }
  } catch {}
  return DEFAULT_SHORTCUT;
}

export function saveShortcut(shortcut: string) {
  try {
    localStorage.setItem(STORAGE_KEY, shortcut);
  } catch {}
}

let registeredShortcut: string | null = null;

/** Apply a new global shortcut via Rust IPC. Returns true on success. */
export async function registerGlobalShortcut(shortcut: string): Promise<boolean> {
  try {
    await invoke("set_hotkey_combo", { combo: shortcut });
    registeredShortcut = shortcut;
    saveShortcut(shortcut);
    console.log("[lunac] Shortcut registered:", shortcut);
    return true;
  } catch (err) {
    console.warn("[lunac] Shortcut registration failed for", shortcut, err);
    return false;
  }
}

/** Get the current shortcut string. Falls back to localStorage then default. */
export function getCurrentShortcut(): string {
  if (registeredShortcut) return registeredShortcut;
  return loadShortcut();
}
