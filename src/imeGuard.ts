/**
 * IME 守卫：修复 WebKitGTK 下 CJK 输入法「字符重复 / 带出历史内容」。
 *
 * 根因（xterm.js 5.5 + WebKitGTK 事件模型不匹配）：
 * 1. 重复——Linux 输入法（ibus/fcitx）拦截按键后，WebKitGTK 不派发 keydown，却在
 *    合成提交时额外派发 inputType="insertText" 的 input 事件（有时还补一个 keypress）；
 *    xterm 的 _inputEvent 守卫（!e.composed || !_keyDownSeen）因 keydown 缺席而放行 →
 *    与 compositionend 的 _finalizeComposition 各发一次，同一段文字双倍上屏。
 * 2. 残留——xterm 只在 _keyUp 里清空隐藏 textarea（IME 缓冲），而 keyup 恰被输入法
 *    吞掉 → textarea 持续累积历史提交；WebKit 合成事件顺序与 Chromium 不同，
 *    substring 起点簿记错位时便把旧内容一并切出重发。
 *
 * 判据（关键）：输入法吞掉了按键，故合成提交后 WebKit 补发的「幽灵重复」事件
 * 没有对应的 keydown；而用户随后真实敲入的同一字符必然带 keydown。以「提交后是否
 * 出现过真实 keydown」区分二者，即可安全地同时拦掉重复的 input 与 keypress 两路，
 * 而绝不会误吃用户的真实输入——这消除了旧实现「只拦一次、窗口收窄」的妥协。
 *
 * 手术点（全部利用「祖先捕获先于目标监听」的确定性顺序，零侵入 xterm 内部）：
 * - capture 拦截：compositionend 之后、期间无真实 keydown、且 data 与刚提交串一致的
 *   input/keypress 事件 → stopPropagation，掐断全部重复通道。
 * - bubble 补清空：compositionend 之后（xterm 的 finalize 在 setTimeout(0) 里读完
 *   textarea 后）把 textarea.value 清空，等价于 xterm 在 keyup 里做的事——消除累积残留。
 *   若用户已开始下一段合成则跳过（合成中清空会错位起点簿记）。
 */

export function installImeGuard(host: HTMLElement): void {
  let composing = false;
  /** 最近一次 compositionupdate 的候选文本：部分 IME 在 end 上给空 data 时的兜底 */
  let pendingData = "";
  /** 最近一次合成提交的文本与时刻，用于识别 WebKit 紧随其后的重复派发 */
  let lastCommit = "";
  let lastCommitAt = 0;
  /**
   * 「自上次合成提交以来是否出现过真实 keydown」。初始 true（无盯防）；
   * compositionend 置 false 开始盯防幽灵重复，任何真实 keydown 置回 true。
   */
  let sawKeydown = true;

  // 被动观察 keydown（不 stopPropagation，不干扰 xterm 的按键处理）
  host.addEventListener("keydown", () => { sawKeydown = true; }, true);

  host.addEventListener(
    "compositionstart",
    () => {
      composing = true;
      pendingData = "";
    },
    true,
  );

  host.addEventListener(
    "compositionupdate",
    (e) => {
      const d = (e as CompositionEvent).data;
      if (d) pendingData = d;
    },
    true,
  );

  // bubble 阶段：在 xterm 的 compositionend 监听（目标上）之后运行，
  // 保证其 finalize 的 setTimeout 先入队，我们的清空后执行、不截胡待发数据。
  host.addEventListener("compositionend", (e) => {
    composing = false;
    const ce = e as CompositionEvent;
    // 兜底：end.data 为空的 IME 用最后一次 update 的文本，否则重复串对不上便拦不住
    lastCommit = ce.data || pendingData;
    lastCommitAt = performance.now();
    sawKeydown = false; // 开始盯防紧随其后的幽灵重复事件
    const ta = ce.target as HTMLTextAreaElement | null;
    if (!ta || ta.tagName !== "TEXTAREA") return;
    // 双层 setTimeout：确保排在 finalize 的读取之后（含其内部再排队的边缘情形）
    window.setTimeout(() => {
      window.setTimeout(() => {
        if (!composing) ta.value = "";
      }, 0);
    }, 0);
  });

  // 幽灵重复判据：文本与刚提交串一致、提交后短时窗内、且期间未出现真实 keydown。
  // keydown 判据是主锚点，500ms 只作二级兜底（防极晚的无关同文本事件被误拦）。
  const isGhostDup = (data: string | null) =>
    !!data &&
    !sawKeydown &&
    data === lastCommit &&
    performance.now() - lastCommitAt < 500;

  host.addEventListener(
    "input",
    (e) => {
      if (composing) return;
      const ie = e as InputEvent;
      if (ie.inputType === "insertText" && isGhostDup(ie.data)) e.stopPropagation();
    },
    true,
  );
  host.addEventListener(
    "keypress",
    (e) => {
      if (composing) return;
      const ke = e as KeyboardEvent;
      // WebKit 某些组合下提交后还会补发 keypress（单字提交最常见）
      if (ke.charCode && isGhostDup(String.fromCharCode(ke.charCode))) e.stopPropagation();
    },
    true,
  );
}
