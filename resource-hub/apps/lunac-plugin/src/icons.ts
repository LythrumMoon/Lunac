// 插件内联线性图标：全部 `stroke="currentColor"`，随主题文字色 / 四类变量走。
// 规范见 `docs/ui-style.md` §界面 4：动作按钮默认「图标 + 文字」。

const ICONS: Record<string, string> = {
  search: '<circle cx="11" cy="11" r="7"/><path d="M16.5 16.5L21 21"/>',
  open: '<path d="M7 17L17 7"/><path d="M8 7h9v9"/>',
  back: '<path d="M19 12H5"/><path d="M12 19l-7-7 7-7"/>',
  bookmark: '<path d="M6 3h12a1 1 0 0 1 1 1v17l-7-4-7 4V4a1 1 0 0 1 1-1z"/>',
  import: '<path d="M12 3v11"/><path d="M8 10l4 4 4-4"/><path d="M4 19h16"/>',
  // 导出（B3，2026-10-28）：与 import 同构、箭头向上出托盘，保持一套笔触。
  export: '<path d="M12 17V6"/><path d="M8 10l4-4 4 4"/><path d="M4 19h16"/>',
  trash: '<path d="M4 7h16"/><path d="M9 7V4h6v3"/><path d="M6 7l1 13h10l1-13"/>',
  close: '<path d="M6 6l12 12"/><path d="M18 6L6 18"/>',
  library: '<path d="M4 5h6v14H4z"/><path d="M14 5h6v6h-6z"/><path d="M14 15h6v4h-6z"/>',
  sources: '<path d="M12 3l8 4-8 4-8-4 8-4z"/><path d="M4 12l8 4 8-4"/><path d="M4 17l8 4 8-4"/>',
  overview: '<path d="M4 6h16"/><path d="M4 12h16"/><path d="M4 18h16"/>',
  heart: '<path d="M20.8 4.6a5.5 5.5 0 0 0-7.8 0L12 5.6l-1-1a5.5 5.5 0 0 0-7.8 7.8l1 1L12 21l7.8-7.6 1-1a5.5 5.5 0 0 0 0-7.8z"/>',
  history: '<path d="M3 12a9 9 0 1 0 3-6.7"/><path d="M3 4v5h5"/><path d="M12 7v5l3 2"/>',
  sort: '<path d="M4 7h10"/><path d="M4 12h7"/><path d="M4 17h4"/><path d="M17 5v14"/><path d="M14 16l3 3 3-3"/>',
  filter: '<path d="M4 5h16l-6 7v6l-4 2v-8z"/>',
  play: '<path d="M7 4l13 8-13 8z"/>',
  chevron: '<path d="M6 9l6 6 6-6"/>',
  swap: '<path d="M4 7h13"/><path d="M14 4l3 3-3 3"/><path d="M20 17H7"/><path d="M10 14l-3 3 3 3"/>',
  refresh: '<path d="M20 11a8 8 0 1 0-2.3 5.7"/><path d="M20 5v6h-6"/>',
  link: '<path d="M10 13a5 5 0 0 0 7 0l2-2a5 5 0 0 0-7-7l-1 1"/><path d="M14 11a5 5 0 0 0-7 0l-2 2a5 5 0 0 0 7 7l1-1"/>',
};

export function rhIcon(name: string, size = 14): string {
  return `<svg class="rh-icon" width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${ICONS[name] ?? ""}</svg>`;
}
