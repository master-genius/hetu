/** 设置存取 + 应用（字体/主题/透明度）到 UI 与所有终端 */

import { api } from "./ipc";
import { applyThemeToUI, resolveTheme } from "./themes";
import type { Settings, ThemeDef } from "./types";

let current: Settings | null = null;
const listeners = new Set<(s: Settings) => void>();

export async function loadSettings(): Promise<Settings> {
  current = await api.settingsGet();
  applySettings();
  return current;
}

export function getSettings(): Settings {
  if (!current) throw new Error("settings not loaded");
  return current;
}

export async function updateSettings(patch: Partial<Settings>): Promise<void> {
  current = { ...getSettings(), ...patch };
  await api.settingsSet(current);
  applySettings();
}

export function onSettingsChange(fn: (s: Settings) => void): void {
  listeners.add(fn);
}

export function activeTheme(): ThemeDef {
  const s = getSettings();
  return resolveTheme(s.theme, s.customThemes);
}

/** 组合完整字体栈：主字体 + CJK 回退 */
export function fontStack(): string {
  const s = getSettings();
  return `${s.fontFamily}, ${s.cjkFontFamily}, monospace`;
}

function applySettings() {
  const s = getSettings();
  const root = document.documentElement;
  applyThemeToUI(activeTheme(), s.opacity, s.blur, s.blurAmount, s.titlebarColor);
  root.style.setProperty("--ui-font", fontStack());
  // 圆角级别（CSS 按 data-radius 提供各级 --radius 变量）
  root.dataset.radius = s.cornerRadius;
  // 标签栏平分宽度
  root.dataset.tabFill = s.tabBarFill ? "1" : "0";
  // 标签页字体/字号：字体空则同主字体；字号 0 则自动取终端字号 - 2（下限 9）
  const tabFont = s.tabFontFamily.trim() ? `${s.tabFontFamily}, ${fontStack()}` : fontStack();
  const tabSize = s.tabFontSize > 0 ? s.tabFontSize : Math.max(9, s.fontSize - 2);
  root.style.setProperty("--tab-font", tabFont);
  root.style.setProperty("--tab-font-size", `${tabSize}px`);
  for (const fn of listeners) fn(s);
}
