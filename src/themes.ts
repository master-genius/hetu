/** 内置主题库（暗色 25 / 亮色·中性 18，收录主流配色与 Sweet 糖果系列）+ 自定义主题解析。
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
  t("rose-pine-moon", "Rosé Pine Moon", "dark", "#e0def4", "#232136", "#e0def4", "#44415a90", [
    "#393552", "#eb6f92", "#3e8fb0", "#f6c177", "#9ccfd8", "#c4a7e7", "#ea9a97", "#e0def4",
    "#6e6a86", "#eb6f92", "#3e8fb0", "#f6c177", "#9ccfd8", "#c4a7e7", "#ea9a97", "#e0def4",
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
  t("tomorrow-night", "Tomorrow Night", "dark", "#c5c8c6", "#1d1f21", "#c5c8c6", "#37383980", [
    "#1d1f21", "#cc6666", "#b5bd68", "#f0c674", "#81a2be", "#b294bb", "#8abeb7", "#c5c8c6",
    "#969896", "#cc6666", "#b5bd68", "#f0c674", "#81a2be", "#b294bb", "#8abeb7", "#ffffff",
  ]),
  t("oceanic-next", "Oceanic Next", "dark", "#c0c5ce", "#1b2b34", "#c0c5ce", "#33455080", [
    "#1b2b34", "#ec5f67", "#99c794", "#fac863", "#6699cc", "#c594c5", "#5fb3b3", "#c0c5ce",
    "#65737e", "#ec5f67", "#99c794", "#fac863", "#6699cc", "#c594c5", "#5fb3b3", "#d8dee9",
  ]),
  t("snazzy", "Snazzy", "dark", "#eff0eb", "#282a36", "#97979b", "#44475a90", [
    "#282a36", "#ff5c57", "#5af78e", "#f3f99d", "#57c7ff", "#ff6ac1", "#9aedfe", "#f1f1f0",
    "#686868", "#ff5c57", "#5af78e", "#f3f99d", "#57c7ff", "#ff6ac1", "#9aedfe", "#eff0eb",
  ]),
  t("cobalt2", "Cobalt2", "dark", "#ffffff", "#193549", "#ffc600", "#0d3a5880", [
    "#000000", "#ff0000", "#38de21", "#ffe50a", "#1460d2", "#ff005d", "#00bbbb", "#bbbbbb",
    "#555555", "#f40e17", "#3bd01d", "#edc809", "#5555ff", "#ff55ff", "#6ae3fa", "#ffffff",
  ]),
  t("zenburn", "Zenburn", "dark", "#dcdccc", "#3f3f3f", "#dcdccc", "#5f5f5f90", [
    "#3f3f3f", "#cc9393", "#7f9f7f", "#e3ceab", "#8cd0d3", "#dc8cc3", "#93e0e3", "#dcdccc",
    "#709080", "#dca3a3", "#bfebbf", "#f0dfaf", "#a2d7dd", "#ec93d3", "#b3ffff", "#ffffff",
  ]),
  // ——— Sweet 系列（糖果风：深蓝紫底 + 粉紫霓虹强调）———
  t("sweet", "Sweet 糖果", "dark", "#e3e6f0", "#222235", "#dd5299", "#6a5cdc55", [
    "#16161f", "#ed254e", "#71f79f", "#f9dc5c", "#7cb7ff", "#c74ded", "#00c1e4", "#dbe1e8",
    "#5c5e70", "#f25e77", "#93fab5", "#fce780", "#9ac8ff", "#d47ff2", "#4fd4ef", "#f2f4fa",
  ]),
  t("sweet-mars", "Sweet Mars 蜜桃", "dark", "#eadfe4", "#231a20", "#ff5c8a", "#a4506c55", [
    "#191218", "#ff5c8a", "#9ff28f", "#ffcc66", "#7aa5ff", "#e07dce", "#6cd8d0", "#e5dae0",
    "#665260", "#ff7ca1", "#b8f7ab", "#ffd985", "#98baff", "#ea9cdc", "#8ce4dd", "#f6edf2",
  ]),
  t("sweet-grape", "Sweet 葡萄", "dark", "#e6e0f2", "#1d1728", "#a06ef5", "#7a4fd655", [
    "#141020", "#f2568c", "#7ee8a2", "#f5d76e", "#8f9bff", "#b07df2", "#5cd6e8", "#ddd6ee",
    "#5a5272", "#f677a3", "#9ceeb8", "#f8e18d", "#a8b2ff", "#c298f6", "#7fe0ef", "#f1edf9",
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
  // ——— 经典亮色 ———
  t("tomorrow", "Tomorrow", "light", "#4d4d4c", "#ffffff", "#4d4d4c", "#d6d6d6b0", [
    "#000000", "#c82829", "#718c00", "#eab700", "#4271ae", "#8959a8", "#3e999f", "#ffffff",
    "#8e908c", "#c82829", "#718c00", "#eab700", "#4271ae", "#8959a8", "#3e999f", "#efefef",
  ]),
  t("papercolor-light", "PaperColor Light", "light", "#444444", "#eeeeee", "#444444", "#c6c6c680", [
    "#bcbcbc", "#af0000", "#008700", "#5f8700", "#0087af", "#878787", "#005f87", "#444444",
    "#bcbcbc", "#d70000", "#5f8700", "#d75f00", "#0087af", "#8700af", "#005faf", "#005f87",
  ]),
  t("selenized-light", "Selenized Light", "light", "#53676d", "#fbf3db", "#53676d", "#ece3ccb0", [
    "#ece3cc", "#d2212d", "#489100", "#ad8900", "#0072d4", "#ca4898", "#009c8f", "#909995",
    "#d5cdb6", "#cc1729", "#428b00", "#a78300", "#006dce", "#c44392", "#00978a", "#3a4d53",
  ]),
  // ——— 中性（介于暗色与亮色之间的雾灰亮色）———
  t("neutral-gray", "中性灰 Neutral Gray", "light", "#2f3338", "#cfd2d6", "#2f3338", "#b3b8be90", [
    "#3b3f45", "#b23b3b", "#3f7d3f", "#9a7d1f", "#3f66b2", "#8a4fa0", "#2f8a8a", "#6f757b",
    "#2f3338", "#c14b4b", "#4f8d4f", "#ad8d2f", "#4f76c2", "#9a5fb0", "#3f9a9a", "#8b9198",
  ]),
  t("warm-gray", "暖灰 Warm Gray", "light", "#3a352f", "#d6d0c8", "#3a352f", "#bcb5aa90", [
    "#453f36", "#b04030", "#5f7d2f", "#a07d20", "#4a66a0", "#8a5090", "#2f8580", "#726c62",
    "#2f2a24", "#c05040", "#6f8d3f", "#b08d30", "#5a76b0", "#9a60a0", "#3f9590", "#8a8478",
  ]),
  t("cool-slate", "青灰 Cool Slate", "light", "#2b3138", "#c9d0d4", "#2b3138", "#aeb8bf90", [
    "#363d45", "#a83b46", "#3f7d5f", "#8a7d2f", "#3f66aa", "#7a5fa0", "#2f8a90", "#6b737b",
    "#232a31", "#b84b56", "#4f8d6f", "#9a8d3f", "#4f76ba", "#8a6fb0", "#3f9aa0", "#878f97",
  ]),
  // ——— Sweet 系列亮色 ———
  t("sweet-milkshake", "Sweet 奶昔", "light", "#4a3a50", "#faf1f6", "#d9418f", "#f0c2dc80", [
    "#4a3a50", "#d6336c", "#2f9e5f", "#b8860b", "#4a63d9", "#a63bbf", "#0f95a8", "#a894a6",
    "#6d5a72", "#e0507f", "#3fae6f", "#c8961b", "#5a73e9", "#b64bcf", "#1fa5b8", "#c7b6c5",
  ]),
];

export const BUILTIN_THEMES: ThemeDef[] = [...DARK, ...LIGHT];

/** 将 hex 颜色向黑色混合（factor=0.2 → 80%原色+20%黑），用于浅色主题提对比度 */
function darken(hex: string, factor: number): string {
  const m = hex.replace("#", "");
  if (m.length !== 6) return hex;
  const r = Math.round(parseInt(m.slice(0, 2), 16) * (1 - factor));
  const g = Math.round(parseInt(m.slice(2, 4), 16) * (1 - factor));
  const b = Math.round(parseInt(m.slice(4, 6), 16) * (1 - factor));
  return `#${r.toString(16).padStart(2, "0")}${g.toString(16).padStart(2, "0")}${b.toString(16).padStart(2, "0")}`;
}

/** 浅色主题对比度增强：前景色和 ANSI 色统一加深（不动背景/选区/光标） */
function boostLightContrast(theme: ThemeDef): ThemeDef {
  if (theme.base !== "light") return theme;
  const c = { ...theme.colors };
  const FG = 0.18;  // 前景色加深幅度
  const ANSI = 0.15; // ANSI 色加深幅度
  if (c.foreground) c.foreground = darken(c.foreground, FG);
  const names = ["black","red","green","yellow","blue","magenta","cyan","white"];
  for (const n of names) {
    if (c[n]) c[n] = darken(c[n], ANSI);
    const bright = `bright${n[0].toUpperCase()}${n.slice(1)}`;
    if (c[bright]) c[bright] = darken(c[bright], ANSI);
  }
  return { ...theme, colors: c };
}

export function resolveTheme(id: string, custom: ThemeDef[]): ThemeDef {
  const c = custom.find((th) => th.id === id);
  let result: ThemeDef;
  if (c) {
    // base 允许指向任意内置主题 id；归一化出 dark/light 基调供 UI 使用
    const base = BUILTIN_THEMES.find((th) => th.id === c.base) ?? BUILTIN_THEMES[0];
    result = { ...c, base: base.base, colors: { ...base.colors, ...c.colors } };
  } else {
    result = BUILTIN_THEMES.find((th) => th.id === id) ?? BUILTIN_THEMES[0];
  }
  return boostLightContrast(result);
}

export function allThemes(custom: ThemeDef[]): ThemeDef[] {
  return [...BUILTIN_THEMES, ...custom];
}

/** 主题色应用到 UI 层 CSS 变量。titlebarColor 为空时标题栏跟随主题背景色。 */
/**
 * 磨砂颗粒贴图：运行时 Canvas 生成的白噪声（逐像素独立随机，无低频结构，
 * 平铺天然无缝，不会出现分形噪声那种斑块拼接感）。
 *
 * 颗粒**同色系**：每个像素围绕主题背景色小幅抖动（±26/通道）——暗色主题出
 * 深色系颗粒、亮色主题出浅色系颗粒，绝不会在暗底上冒出发亮的灰白点。
 * alpha 由「磨砂程度」设置换算而来。按 (背景色, alpha) 缓存，换主题/调程度
 * 时才重新生成。
 */

const frostCache = new Map<string, string>();
export function frostNoiseUrl(bg: string, alpha: number): string {
  const key = `${bg}:${alpha}`;
  const hit = frostCache.get(key);
  if (hit) return hit;
  const m = bg.replace("#", "");
  const br = parseInt(m.slice(0, 2), 16) || 0;
  const bgc = parseInt(m.slice(2, 4), 16) || 0;
  const bb = parseInt(m.slice(4, 6), 16) || 0;
  // 256×256 贴图以 128px 显示（styles.css background-size）：每个噪点仅占
  // 0.5 CSS 像素，亚像素滤波互相平均 → 细密平滑的哑光面；DPR 2 下对齐物理像素。
  const size = 256;
  const c = document.createElement("canvas");
  c.width = c.height = size;
  const ctx = c.getContext("2d");
  if (!ctx) return "none";
  const img = ctx.createImageData(size, size);
  const clamp = (v: number) => (v < 0 ? 0 : v > 255 ? 255 : v | 0);
  for (let i = 0; i < img.data.length; i += 4) {
    const j = (Math.random() * 2 - 1) * 26; // 同一像素三通道同步抖动，保持同色系
    img.data[i] = clamp(br + j);
    img.data[i + 1] = clamp(bgc + j);
    img.data[i + 2] = clamp(bb + j);
    img.data[i + 3] = alpha;
  }
  ctx.putImageData(img, 0, 0);
  const url = `url(${c.toDataURL("image/png")})`;
  // 上限保留少量条目：既缓存玻璃(固定 alpha)与磨砂(可变 alpha)两张，又避免反复
  // 调滑杆时无限累积 data URL。超出时淘汰最早插入的一条。
  if (frostCache.size >= 6) frostCache.delete(frostCache.keys().next().value as string);
  frostCache.set(key, url);
  return url;
}

export function applyThemeToUI(
  theme: ThemeDef,
  opacity: number,
  blur: boolean,
  blurAmount: number,
  titlebarColor?: string | null,
  bgBlur: boolean = true,
) {
  const root = document.documentElement;
  const bg = theme.colors.background ?? "#10151c";
  const fg = theme.colors.foreground ?? "#d8dee9";
  const chromeAlpha = Math.min(1, opacity + 0.05);
  // 文件面板透明度自适应：低透明度时略高（+4%）保持可用性，
  // 高透明度时略低（-4%）保持通透美感，中间范围与主窗口一致
  let panelAlpha: number;
  if (opacity <= 0.42) {
    panelAlpha = Math.min(1, opacity + 0.04);
  } else if (opacity > 0.81) {
    panelAlpha = Math.max(0, opacity - 0.04);
  } else {
    panelAlpha = opacity;
  }
  root.dataset.base = theme.base;
  // 让原生控件（下拉列表、复选框、滚动条）跟随明暗，修正 select/option 白底看不清
  root.style.colorScheme = theme.base;
  root.style.setProperty("--term-bg", bg);
  root.style.setProperty("--term-fg", fg);
  root.style.setProperty("--accent", theme.colors.cursor ?? "#88c0d0");
  root.style.setProperty("--bg-alpha", String(opacity));
  root.style.setProperty("--bg-rgba", hexToRgba(bg, opacity));
  root.style.setProperty("--chrome-rgba", hexToRgba(bg, chromeAlpha));
  root.style.setProperty("--panel-rgba", hexToRgba(bg, panelAlpha));
  root.style.setProperty("--titlebar-rgba", hexToRgba(titlebarColor || bg, chromeAlpha));
  const px = Math.max(0, Math.round(blurAmount));
  root.style.setProperty("--blur", blur && px > 0 ? `blur(${px}px) saturate(1.3)` : "none");
  // 磨砂质感已独立为单独设置（frosted/frostStrength），由 settings.ts 应用
  // 终端背景毛玻璃（光晕内容层 #app::after + 玻璃面层 #glass-veil）：
  // 桌面像素拿不到，改为自己垫光晕内容，由玻璃面层 backdrop-filter 真模糊。
  // 「背景虚化」(bgBlur) 为总开关：关闭后终端整体背景不做任何虚化/光晕/玻璃处理，
  // 但透明度(--bg-rgba)仍作用于底色，弹窗模糊(各自固定值)也不受影响。
  const bgOn = bgBlur && blur && px > 0;
  // 终端背景虚化不做饱和度增强：saturate>1 会放大暗色主题背景本身的蓝黑底色，
  // 使整体"发蓝"。此处保持中性，忠实呈现主题黑色（弹窗模糊另有各自的 saturate）。
  root.style.setProperty("--bg-blur", bgOn ? `blur(${px}px)` : "none");
  root.dataset.glass = bgOn ? "1" : "0";
  // 玻璃内容层已改用确定性文本点（#glass-content .gc-dot），不再需要 canvas 噪声。
  // 文本点颜色由 CSS color-mix(var(--term-fg), var(--term-bg)) 自动跟随主题。
}

export function hexToRgba(hex: string, alpha: number): string {
  const m = hex.replace("#", "");
  const r = parseInt(m.slice(0, 2), 16);
  const g = parseInt(m.slice(2, 4), 16);
  const b = parseInt(m.slice(4, 6), 16);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}
