/** 设置存取 + 应用（字体/主题/透明度）到 UI 与所有终端 */

import { api } from "./ipc";
import { applyThemeToUI, frostNoiseUrl, resolveTheme } from "./themes";
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
  applyThemeToUI(activeTheme(), s.opacity, s.blur, s.blurAmount, s.titlebarColor, s.bgBlur);
  root.style.setProperty("--ui-font", fontStack());
  // 圆角级别（CSS 按 data-radius 提供各级 --radius 变量）
  root.dataset.radius = s.cornerRadius;
  // 标签栏平分宽度
  root.dataset.tabFill = s.tabBarFill ? "1" : "0";
  // 标签页字体/字号：字体空则同主字体；字号不填（0）固定默认 12，不跟随终端字号
  const tabFont = s.tabFontFamily.trim() ? `${s.tabFontFamily}, ${fontStack()}` : fontStack();
  const tabSize = s.tabFontSize > 0 ? s.tabFontSize : 12;
  root.style.setProperty("--tab-font", tabFont);
  root.style.setProperty("--tab-font-size", `${tabSize}px`);
  // 终端滚动条显隐（CSS 按 data-scrollbar 开关 .xterm-viewport 的滚动条）
  root.dataset.scrollbar = s.showScrollbar ? "1" : "0";
  // 文件面板列表字号：跟随终端字号自动取「小一号」，钳制在 [11, 19]，不提供独立选项
  const exSize = Math.min(19, Math.max(11, s.fontSize - 1));
  root.style.setProperty("--ex-font-size", `${exSize}px`);
  // 磨砂质感：独立于毛玻璃的表面颗粒层（同色系噪点，随主题背景取色）。
  // 程度 0–100 → 贴图 alpha 0–45（上限 ≈18% 透明度）
  const frosted = s.frosted && s.frostStrength > 0;
  root.dataset.frost = frosted ? "1" : "0";
  if (frosted) {
    const bg = activeTheme().colors.background ?? "#10151c";
    const alpha = Math.round(Math.min(100, Math.max(0, s.frostStrength)) * 0.45);
    root.style.setProperty("--frost-noise", frostNoiseUrl(bg, alpha));
  }
  for (const fn of listeners) fn(s);
}
