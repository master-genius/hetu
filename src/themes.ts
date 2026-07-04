/** 内置主题库（暗色 16 / 亮色 11，收录主流配色）+ 自定义主题解析。
 *  colors 键与 xterm ITheme 对齐。 */

import type { ThemeDef } from "./types";

/** 紧凑构造：ansi 依次为 black red green yellow blue magenta cyan white × 普通/明亮 */
function t(
  id: string,
  name: string,
  base: "dark" | "light",
  fg: string,
  bg: string,
  cursor: string,
  selection: string,
  ansi: string[],
): ThemeDef {
  const keys = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];
  const colors: Record<string, string> = {
    foreground: fg,
    background: bg,
    cursor,
    cursorAccent: bg,
    selectionBackground: selection,
  };
  keys.forEach((k, i) => {
    colors[k] = ansi[i];
    colors[`bright${k[0].toUpperCase()}${k.slice(1)}`] = ansi[i + 8];
  });
  return { id, name, base, colors };
}

// ---------- 暗色（16） ----------

const DARK: ThemeDef[] = [
  t("dark", "HetuShell 暗色（默认）", "dark", "#d8dee9", "#10151c", "#88c0d0", "#3b4a5a80", [
    "#1c2430", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#c8ccd4",
    "#5c6773", "#ef7c86", "#a9d48a", "#f0cf8f", "#74bcf7", "#d48ae8", "#68c8d4", "#eceff4",
  ]),
  t("one-dark", "One Dark", "dark", "#abb2bf", "#282c34", "#528bff", "#3e445180", [
    "#282c34", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#abb2bf",
    "#5c6370", "#e06c75", "#98c379", "#d19a66", "#61afef", "#c678dd", "#56b6c2", "#ffffff",
  ]),
  t("dracula", "Dracula", "dark", "#f8f8f2", "#282a36", "#f8f8f2", "#44475a90", [
    "#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2",
    "#6272a4", "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df", "#a4ffff", "#ffffff",
  ]),
  t("nord", "Nord", "dark", "#d8dee9", "#2e3440", "#d8dee9", "#434c5e90", [
    "#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0", "#e5e9f0",
    "#4c566a", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#8fbcbb", "#eceff4",
  ]),
  t("gruvbox-dark", "Gruvbox Dark", "dark", "#ebdbb2", "#282828", "#ebdbb2", "#50494580", [
    "#282828", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#a89984",
    "#928374", "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#ebdbb2",
  ]),
  t("solarized-dark", "Solarized Dark", "dark", "#839496", "#002b36", "#839496", "#07364280", [
    "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5",
    "#586e75", "#cb4b16", "#859900", "#b58900", "#268bd2", "#6c71c4", "#93a1a1", "#fdf6e3",
  ]),
  t("tokyo-night", "Tokyo Night", "dark", "#c0caf5", "#1a1b26", "#c0caf5", "#33467c80", [
    "#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6",
    "#414868", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5",
  ]),
  t("catppuccin-mocha", "Catppuccin Mocha", "dark", "#cdd6f4", "#1e1e2e", "#f5e0dc", "#585b7080", [
    "#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#bac2de",
    "#585b70", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#a6adc8",
  ]),
  t("monokai", "Monokai", "dark", "#f8f8f2", "#272822", "#f8f8f2", "#49483e90", [
    "#272822", "#f92672", "#a6e22e", "#e6db74", "#66d9ef", "#ae81ff", "#a1efe4", "#f8f8f2",
    "#75715e", "#f92672", "#a6e22e", "#e6db74", "#66d9ef", "#ae81ff", "#a1efe4", "#f9f8f5",
  ]),
  t("github-dark", "GitHub Dark", "dark", "#c9d1d9", "#0d1117", "#58a6ff", "#1f6feb50", [
    "#484f58", "#ff7b72", "#3fb950", "#d29922", "#58a6ff", "#bc8cff", "#39c5cf", "#b1bac4",
    "#6e7681", "#ffa198", "#56d364", "#e3b341", "#79c0ff", "#d2a8ff", "#56d4dd", "#f0f6fc",
  ]),
  t("ayu-dark", "Ayu Dark", "dark", "#b3b1ad", "#0a0e14", "#e6b450", "#273747a0", [
    "#01060e", "#ea6c73", "#91b362", "#f9af4f", "#53bdfa", "#fae994", "#90e1c6", "#c7c7c7",
    "#686868", "#f07178", "#c2d94c", "#ffb454", "#59c2ff", "#ffee99", "#95e6cb", "#ffffff",
  ]),
  t("rose-pine", "Rosé Pine", "dark", "#e0def4", "#191724", "#e0def4", "#40384990", [
    "#26233a", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba", "#e0def4",
    "#6e6a86", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba", "#e0def4",
  ]),
  t("kanagawa", "Kanagawa", "dark", "#dcd7ba", "#1f1f28", "#c8c093", "#2d4f6780", [
    "#16161d", "#c34043", "#76946a", "#c0a36e", "#7e9cd8", "#957fb8", "#6a9589", "#c8c093",
    "#727169", "#e82424", "#98bb6c", "#e6c384", "#7fb4ca", "#938aa9", "#7aa89f", "#dcd7ba",
  ]),
  t("everforest-dark", "Everforest Dark", "dark", "#d3c6aa", "#2d353b", "#d3c6aa", "#47525890", [
    "#475258", "#e67e80", "#a7c080", "#dbbc7f", "#7fbbb3", "#d699b6", "#83c092", "#d3c6aa",
    "#5c6a72", "#e67e80", "#a7c080", "#dbbc7f", "#7fbbb3", "#d699b6", "#83c092", "#fdf6e3",
  ]),
  t("night-owl", "Night Owl", "dark", "#d6deeb", "#011627", "#80a4c2", "#1d3b5380", [
    "#011627", "#ef5350", "#22da6e", "#addb67", "#82aaff", "#c792ea", "#21c7a8", "#ffffff",
    "#575656", "#ef5350", "#22da6e", "#ffeb95", "#82aaff", "#c792ea", "#7fdbca", "#ffffff",
  ]),
  t("palenight", "Palenight", "dark", "#a6accd", "#292d3e", "#ffcc00", "#44425880", [
    "#292d3e", "#f07178", "#c3e88d", "#ffcb6b", "#82aaff", "#c792ea", "#89ddff", "#d0d0d0",
    "#676e95", "#f07178", "#c3e88d", "#ffcb6b", "#82aaff", "#c792ea", "#89ddff", "#ffffff",
  ]),
];

// ---------- 亮色（11） ----------

const LIGHT: ThemeDef[] = [
  t("light", "HetuShell 亮色（默认）", "light", "#383a42", "#fafafa", "#526eff", "#c4d2e880", [
    "#383a42", "#e45649", "#50a14f", "#c18401", "#4078f2", "#a626a4", "#0184bc", "#a0a1a7",
    "#696c77", "#f26d5f", "#5fb85e", "#d99a12", "#5487f5", "#bb40b8", "#0699d6", "#ffffff",
  ]),
  t("solarized-light", "Solarized Light", "light", "#657b83", "#fdf6e3", "#657b83", "#eee8d5b0", [
    "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5",
    "#586e75", "#cb4b16", "#859900", "#b58900", "#268bd2", "#6c71c4", "#93a1a1", "#fdf6e3",
  ]),
  t("github-light", "GitHub Light", "light", "#24292f", "#ffffff", "#0969da", "#b6e3ff80", [
    "#24292f", "#cf222e", "#116329", "#4d2d00", "#0969da", "#8250df", "#1b7c83", "#6e7781",
    "#57606a", "#a40e26", "#1a7f37", "#633c01", "#218bff", "#a475f9", "#3192aa", "#8c959f",
  ]),
  t("gruvbox-light", "Gruvbox Light", "light", "#3c3836", "#fbf1c7", "#3c3836", "#d5c4a180", [
    "#fbf1c7", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#7c6f64",
    "#928374", "#9d0006", "#79740e", "#b57614", "#076678", "#8f3f71", "#427b58", "#3c3836",
  ]),
  t("catppuccin-latte", "Catppuccin Latte", "light", "#4c4f69", "#eff1f5", "#dc8a78", "#acb0be60", [
    "#5c5f77", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299", "#acb0be",
    "#6c6f85", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299", "#bcc0cc",
  ]),
  t("ayu-light", "Ayu Light", "light", "#5c6773", "#fafafa", "#ff6a00", "#d1e4f480", [
    "#000000", "#f07171", "#86b300", "#f2ae49", "#399ee6", "#a37acc", "#4cbf99", "#c7c7c7",
    "#686868", "#f07171", "#86b300", "#f2ae49", "#399ee6", "#a37acc", "#4cbf99", "#d1d1d1",
  ]),
  t("tokyo-night-day", "Tokyo Night Day", "light", "#3760bf", "#e1e2e7", "#3760bf", "#b7c1e380", [
    "#b4b5b9", "#f52a65", "#587539", "#8c6c3e", "#2e7de9", "#9854f1", "#007197", "#6172b0",
    "#a1a6c5", "#f52a65", "#587539", "#8c6c3e", "#2e7de9", "#9854f1", "#007197", "#3760bf",
  ]),
  t("rose-pine-dawn", "Rosé Pine Dawn", "light", "#575279", "#faf4ed", "#575279", "#dfdad980", [
    "#f2e9e1", "#b4637a", "#286983", "#ea9d34", "#56949f", "#907aa9", "#d7827e", "#575279",
    "#9893a5", "#b4637a", "#286983", "#ea9d34", "#56949f", "#907aa9", "#d7827e", "#575279",
  ]),
  t("everforest-light", "Everforest Light", "light", "#5c6a72", "#fdf6e3", "#5c6a72", "#e0dcc780", [
    "#5c6a72", "#f85552", "#8da101", "#dfa000", "#3a94c5", "#df69ba", "#35a77c", "#e0dcc7",
    "#a6b0a0", "#f85552", "#8da101", "#dfa000", "#3a94c5", "#df69ba", "#35a77c", "#fdf6e3",
  ]),
  t("material-lighter", "Material Lighter", "light", "#546e7a", "#fafafa", "#272727", "#cfd8dc90", [
    "#000000", "#e53935", "#91b859", "#f6a434", "#6182b8", "#7c4dff", "#39adb5", "#a0a0a0",
    "#546e7a", "#e53935", "#91b859", "#f6a434", "#6182b8", "#7c4dff", "#39adb5", "#ffffff",
  ]),
  t("one-light", "One Light", "light", "#383a42", "#f9f9f9", "#526eff", "#e5e5e690", [
    "#696c77", "#e45649", "#50a14f", "#c18401", "#4078f2", "#a626a4", "#0184bc", "#a0a1a7",
    "#383a42", "#e45649", "#50a14f", "#c18401", "#4078f2", "#a626a4", "#0184bc", "#ffffff",
  ]),
];

export const BUILTIN_THEMES: ThemeDef[] = [...DARK, ...LIGHT];

/** 按 id 解析主题：先查自定义，再查内置；自定义主题继承其 base 内置主题的缺省色 */
export function resolveTheme(id: string, custom: ThemeDef[]): ThemeDef {
  const c = custom.find((th) => th.id === id);
  if (c) {
    // base 允许指向任意内置主题 id；归一化出 dark/light 基调供 UI 使用
    const base = BUILTIN_THEMES.find((th) => th.id === c.base) ?? BUILTIN_THEMES[0];
    return { ...c, base: base.base, colors: { ...base.colors, ...c.colors } };
  }
  return BUILTIN_THEMES.find((th) => th.id === id) ?? BUILTIN_THEMES[0];
}

export function allThemes(custom: ThemeDef[]): ThemeDef[] {
  return [...BUILTIN_THEMES, ...custom];
}

/** 主题色应用到 UI 层 CSS 变量。titlebarColor 为空时标题栏跟随主题背景色。 */
export function applyThemeToUI(
  theme: ThemeDef,
  opacity: number,
  blur: boolean,
  titlebarColor?: string | null,
) {
  const root = document.documentElement;
  const bg = theme.colors.background ?? "#10151c";
  const fg = theme.colors.foreground ?? "#d8dee9";
  const chromeAlpha = Math.min(1, opacity + 0.05);
  root.dataset.base = theme.base;
  root.style.setProperty("--term-bg", bg);
  root.style.setProperty("--term-fg", fg);
  root.style.setProperty("--accent", theme.colors.cursor ?? "#88c0d0");
  root.style.setProperty("--bg-alpha", String(opacity));
  root.style.setProperty("--bg-rgba", hexToRgba(bg, opacity));
  root.style.setProperty("--chrome-rgba", hexToRgba(bg, chromeAlpha));
  root.style.setProperty("--titlebar-rgba", hexToRgba(titlebarColor || bg, chromeAlpha));
  root.style.setProperty("--blur", blur ? "blur(24px) saturate(1.25)" : "none");
}

export function hexToRgba(hex: string, alpha: number): string {
  const m = hex.replace("#", "");
  const r = parseInt(m.slice(0, 2), 16);
  const g = parseInt(m.slice(2, 4), 16);
  const b = parseInt(m.slice(4, 6), 16);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}
