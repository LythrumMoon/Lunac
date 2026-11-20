// ── iframe 插件的主题 token 快照（2026-10-18）────────────────────
//
// 主窗口 / 插件悬浮窗都把「已算好的变量」通过握手后的 `theme.changed` 发给 iframe。
// iframe 没有宿主的 styles.css，所以不能只发 `_themeVarNames` 里那批内联变量：
// 这里补一份基础 token 名单，并从 `getComputedStyle` 取 :root 默认值。

export const BASE_THEME_TOKENS = [
  "--text",
  "--text-dim",
  "--text-muted",
  "--surface-glass",
  "--border-glass",
  "--accent",
  "--accent-rgb",
  "--ink-rgb",
  "--shade-rgb",
  "--danger-rgb",
  "--green",
  "--red",
  "--yellow",
  "--blue",
  "--radius",
  "--radius-search",
  "--radius-results",
  "--surface-alpha",
  "--bg-opacity",
  "--bg-blur",
  "--bg-saturate",
  "--btn-bg-rgb",
  "--btn-bg-alpha",
  "--btn-line-rgb",
  "--other-overlay-rgb",
  "--other-overlay-nav-alpha",
] as const;

/** 收集当前窗口的 token；同名以内联值为准，缺失时回落到 :root 计算值。 */
export function collectThemeTokens(extra: Iterable<string> = []): Record<string, string> {
  const inline = document.documentElement.style;
  const computed = getComputedStyle(document.documentElement);
  const out: Record<string, string> = {};
  for (const name of new Set<string>([...BASE_THEME_TOKENS, ...extra])) {
    const value = (inline.getPropertyValue(name) || computed.getPropertyValue(name)).trim();
    if (value) out[name] = value;
  }
  return out;
}
