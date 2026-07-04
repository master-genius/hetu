/**
 * 快捷键系统：动作 ↔ 组合键映射，可在设置中自定义。
 * 组合键字符串格式：修饰键(Ctrl/Alt/Shift/Meta) + e.code，如 "Ctrl+Alt+KeyR"、"Shift+Tab"、"Ctrl+ArrowLeft"。
 */

export type Action =
  | "newTab"
  | "splitRight"
  | "splitDown"
  | "closePane"
  | "cycleTab"
  | "prevTab"
  | "nextTab"
  | "focusLeft"
  | "focusRight"
  | "focusUp"
  | "focusDown"
  | "copy"
  | "paste";

/** 动作的展示名（设置界面用），顺序即展示顺序 */
export const ACTIONS: Array<{ action: Action; label: string }> = [
  { action: "newTab", label: "新建标签页" },
  { action: "cycleTab", label: "循环切换标签页" },
  { action: "prevTab", label: "上一个标签页" },
  { action: "nextTab", label: "下一个标签页" },
  { action: "splitRight", label: "向右切分" },
  { action: "splitDown", label: "向下切分" },
  { action: "closePane", label: "关闭当前分屏" },
  { action: "focusLeft", label: "焦点移到左侧分屏" },
  { action: "focusRight", label: "焦点移到右侧分屏" },
  { action: "focusUp", label: "焦点移到上方分屏" },
  { action: "focusDown", label: "焦点移到下方分屏" },
  { action: "copy", label: "复制" },
  { action: "paste", label: "粘贴" },
];

export const DEFAULT_KEYBINDINGS: Record<Action, string> = {
  newTab: "Ctrl+Shift+KeyT",
  splitRight: "Ctrl+Alt+KeyR",
  splitDown: "Ctrl+Alt+KeyD",
  closePane: "Ctrl+Shift+KeyW",
  cycleTab: "Shift+Tab",
  prevTab: "Ctrl+ArrowLeft",
  nextTab: "Ctrl+ArrowRight",
  focusLeft: "Alt+ArrowLeft",
  focusRight: "Alt+ArrowRight",
  focusUp: "Alt+ArrowUp",
  focusDown: "Alt+ArrowDown",
  copy: "Ctrl+Shift+KeyC",
  paste: "Ctrl+Shift+KeyV",
};

/** 键盘事件 → 组合键字符串（修饰键固定顺序） */
export function eventToCombo(e: KeyboardEvent): string {
  const parts: string[] = [];
  if (e.ctrlKey) parts.push("Ctrl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  if (e.metaKey) parts.push("Meta");
  // 纯修饰键本身不作为触发键
  if (!["ControlLeft", "ControlRight", "AltLeft", "AltRight", "ShiftLeft", "ShiftRight", "MetaLeft", "MetaRight"].includes(e.code)) {
    parts.push(e.code);
  }
  return parts.join("+");
}

/** 组合键字符串 → 人类可读（KeyT→T，ArrowLeft→←，Digit1→1 等） */
export function comboToLabel(combo: string): string {
  return combo
    .split("+")
    .map((p) => {
      if (p.startsWith("Key")) return p.slice(3);
      if (p.startsWith("Digit")) return p.slice(5);
      const map: Record<string, string> = {
        ArrowLeft: "←", ArrowRight: "→", ArrowUp: "↑", ArrowDown: "↓",
        Tab: "Tab", Escape: "Esc", Space: "Space", Enter: "Enter",
        Ctrl: "Ctrl", Alt: "Alt", Shift: "Shift", Meta: "Meta",
      };
      return map[p] ?? p;
    })
    .join(" + ");
}

/** 合并默认与用户覆盖 */
export function resolveBindings(overrides: Record<string, string> | undefined): Record<Action, string> {
  return { ...DEFAULT_KEYBINDINGS, ...(overrides ?? {}) } as Record<Action, string>;
}

/** 事件命中哪个动作（无则 null） */
export function matchAction(e: KeyboardEvent, bindings: Record<Action, string>): Action | null {
  const combo = eventToCombo(e);
  for (const a of Object.keys(bindings) as Action[]) {
    if (bindings[a] === combo) return a;
  }
  return null;
}
