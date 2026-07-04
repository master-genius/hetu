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
  applyThemeToUI(activeTheme(), s.opacity, s.blur, s.titlebarColor);
  document.documentElement.style.setProperty("--ui-font", fontStack());
  for (const fn of listeners) fn(s);
}
