/**
 * IME 守卫：修复 WebKitGTK 下 CJK 输入法「字符重复 / 带出历史内容」。
 *
 * 根因（xterm.js 5.5 + WebKitGTK 事件模型不匹配）：
 * 1. 重复——Linux 输入法（ibus/fcitx）拦截按键后，WebKitGTK 不派发 keydown，却在
 *    合成提交时额外派发 inputType="insertText" 的 input 事件；xterm 的 _inputEvent
 *    守卫（!e.composed || !_keyDownSeen）因 keydown 缺席而放行 → 与 compositionend
 *    的 _finalizeComposition 各发一次，同一段文字双倍上屏。
 * 2. 残留——xterm 只在 _keyUp 里清空隐藏 textarea（IME 缓冲），而 keyup 恰被输入法
 *    吞掉 → textarea 持续累积历史提交；WebKit 合成事件顺序与 Chromium 不同，
 *    substring 起点簿记错位时便把旧内容一并切出重发。
 *
 * 手术点（全部利用「祖先捕获先于目标监听」的确定性顺序，零侵入 xterm 内部）：
 * - capture 拦截：compositionend 后 120ms 内、data 与刚提交串完全一致的
 *   input/keypress 事件 → stopPropagation，掐断重复通道（只拦一次，签名不可能
 *   误伤人类输入：120ms 内重打一模一样的串不现实）。
 * - bubble 补清空：compositionend 之后（xterm 的 finalize 在 setTimeout(0) 里读完
 *   textarea 后）把 textarea.value 清空，等价于 xterm 在 keyup 里做的事——
 *   消除累积残留。若用户已开始下一段合成则跳过（起点簿记以当前 value 长度为准，
 *   合成中清空反而会错位）。
 */

export function installImeGuard(host: HTMLElement): void {
  let composing = false;
  /** 最近一次合成提交的文本与时刻，用于识别 WebKit 紧随其后的重复派发 */
  let lastCommit = "";
  let lastCommitAt = 0;

  host.addEventListener(
    "compositionstart",
    () => {
      composing = true;
    },
    true,
  );

  // bubble 阶段：在 xterm 的 compositionend 监听（目标上）之后运行，
  // 保证其 finalize 的 setTimeout 先入队，我们的清空后执行、不截胡待发数据。
  host.addEventListener("compositionend", (e) => {
    composing = false;
    lastCommit = e.data ?? "";
    lastCommitAt = performance.now();
    const ta = e.target as HTMLTextAreaElement | null;
    if (!ta || ta.tagName !== "TEXTAREA") return;
    // 双层 setTimeout：确保排在 finalize 的读取之后（含其内部再排队的边缘情形）
    window.setTimeout(() => {
      window.setTimeout(() => {
        if (!composing) ta.value = "";
      }, 0);
    }, 0);
  });

  // capture 阶段：祖先捕获必先于 xterm 挂在 textarea 上的监听 → 可在其发送前掐断。
  const suppressDup = (e: Event, data: string | null) => {
    if (
      data &&
      data === lastCommit &&
      performance.now() - lastCommitAt < 120
    ) {
      e.stopPropagation();
      lastCommit = ""; // 只拦截一次，后续同文本输入不受影响
    }
  };
  host.addEventListener(
    "input",
    (e) => {
      const ie = e as InputEvent;
      if (ie.inputType === "insertText") suppressDup(e, ie.data);
    },
    true,
  );
  host.addEventListener(
    "keypress",
    (e) => {
      const ke = e as KeyboardEvent;
      // WebKit 某些组合下提交后还会补发 keypress（单字提交最常见）
      if (ke.charCode) suppressDup(e, String.fromCharCode(ke.charCode));
    },
    true,
  );
}
