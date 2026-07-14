/** 连接对话框（新建标签页选择连接项）、设置对话框与关于弹窗 */

import { api } from "./ipc";
import { getSettings, updateSettings } from "./settings";
import { allThemes, BUILTIN_THEMES, resolveTheme } from "./themes";
import { customSelect, toast, type CSOption } from "./ui";
import {
  ACTIONS, comboToLabel, DEFAULT_KEYBINDINGS, eventToCombo, resolveBindings, type Action,
} from "./keybindings";
import type { ConnParams, Profile, Settings, ThemeDef } from "./types";

// 推荐字体（字重并入名字）；用户可在此基础上从系统字体中另选。
const MONO_FONTS = [
  "JetBrains Mono NL", // 内置（Regular，默认）
  "JetBrains Mono NL Light", // 内置（Light）
  "Cascadia Code",
  "Fira Code",
  "Source Code Pro",
  "Hack",
  "Consolas",
  "Menlo",
  "monospace",
];
const CJK_FONTS = [
  "Noto Sans CJK SC",
  "Noto Sans CJK SC Light",
  "Noto Sans CJK SC Medium",
  "Noto Sans SC",
  "Source Han Sans SC",
  "Microsoft YaHei",
  "PingFang SC",
  "WenQuanYi Micro Hei",
  "sans-serif",
];

// ---------- 关于弹窗 ----------

const REPO_URL = "https://github.com/master-genius/hetu";

/** 标题栏「？」：软件简要信息（版本 / 开发者 / 许可证 / 仓库，仓库地址点击复制） */
export function showAboutDialog(): void {
  const overlay = document.createElement("div");
  overlay.className = "modal-overlay";
  overlay.innerHTML = `
    <div class="modal small about">
      <h3>HetuShell · 河图</h3>
      <div class="about-body">
        <p class="about-desc">河图终端：终端 + AI/工具链的融合体验，原生支持自实现 SSH。</p>
        <div class="about-grid">
          <span>版本</span><b class="about-version">—</b>
          <span>开发者</span><b>BraveWang</b>
          <span>许可证</span><b>木兰宽松许可证 第2版（Mulan PSL v2）</b>
          <span>仓库</span><b><button type="button" class="about-repo" title="点击复制地址"></button></b>
        </div>
      </div>
      <div class="modal-actions center">
        <button type="button" class="btn primary" data-act="close">关闭</button>
      </div>
    </div>`;
  const repoBtn = overlay.querySelector(".about-repo") as HTMLButtonElement;
  repoBtn.textContent = REPO_URL;
  repoBtn.addEventListener("click", async () => {
    try {
      const { writeText } = await import("@tauri-apps/plugin-clipboard-manager");
      await writeText(REPO_URL);
      toast("仓库地址已复制到剪贴板");
    } catch {
      toast("复制失败", true);
    }
  });
  // 版本号取自 tauri.conf.json（打包元数据），避免前端硬编码不同步
  void import("@tauri-apps/api/app")
    .then(({ getVersion }) => getVersion())
    .then((v) => ((overlay.querySelector(".about-version") as HTMLElement).textContent = `v${v}`))
    .catch(() => {});
  const close = () => overlay.remove();
  overlay.querySelector('[data-act="close"]')!.addEventListener("click", close);
  overlay.addEventListener("mousedown", (e) => {
    if (e.target === overlay) close();
  });
  document.body.appendChild(overlay);
}

// ---------- 连接对话框 ----------

/** 打开连接窗时的预填意图（供 hssh 等场景复用）：选中已保存连接项，或临时主机。 */
export type ConnPrefill =
  | { kind: "profile"; profile: Profile }
  | { kind: "adhoc"; host: string; user?: string; port?: number };

export function showConnectDialog(
  onConnect: (params: ConnParams, profileId?: string) => Promise<void>,
  onLocal?: () => Promise<void>,
  prefill?: ConnPrefill,
) {
  const overlay = document.createElement("div");
  overlay.className = "modal-overlay";
  overlay.innerHTML = `
    <div class="modal connect">
      <h3>新建连接</h3>
      <div class="connect-layout">
        <div class="profile-list">
          <div class="profile-list-head">连接项（含 ~/.ssh/config 导入）</div>
          <div class="profile-items"></div>
        </div>
        <form class="connect-form">
          <label>名称 <input name="name" placeholder="my-server" required></label>
          <div class="row">
            <label class="grow">主机 <input name="host" placeholder="example.com" required></label>
            <label class="port">端口 <input name="port" type="number" value="22" min="1" max="65535"></label>
          </div>
          <label>用户名 <input name="user" placeholder="root" required></label>
          <label>认证方式 <span class="cs-mount" data-cs="auth"></span></label>
          <div class="auth-key">
            <input name="keyPath" type="hidden">
            <label>
              <span class="key-label-row">私钥内容
                <button type="button" class="btn tiny browse-key">从文件导入</button>
              </span>
              <textarea name="keyData" class="key-area" spellcheck="false" autocomplete="off"
                placeholder="粘贴私钥内容（-----BEGIN OPENSSH PRIVATE KEY----- …），或点「从文件导入」"></textarea>
            </label>
            <label>私钥口令（可选）<input name="passphrase" type="password" autocomplete="off"></label>
            <p class="hint">私钥内容随连接项保存到应用配置（profiles.json，权限 0600），不依赖你的文件路径。</p>
          </div>
          <div class="auth-pass" style="display:none">
            <label>密码 <input name="password" type="password" autocomplete="off"></label>
          </div>
          <details class="advanced">
            <summary>高级选项</summary>
            <label>备注 / 标记 <input name="note" placeholder="生产环境 · 华东节点" spellcheck="false"></label>
            <div class="row">
              <label class="grow">保活间隔（秒）<input name="keepalive" type="number" min="1" max="3600" placeholder="15"></label>
              <label class="grow">连接超时（秒）<input name="timeout" type="number" min="1" max="600" placeholder="20"></label>
            </div>
          </details>
          <div class="modal-actions">
            <button type="button" class="btn save-profile">保存连接项</button>
            <span class="grow"></span>
            <button type="button" class="btn" data-act="cancel">取消</button>
            <button type="submit" class="btn primary">连接</button>
          </div>
          <p class="hint">密码与口令不会被保存，仅用于本次连接。</p>
        </form>
      </div>
    </div>`;
  document.body.appendChild(overlay);

  const form = overlay.querySelector(".connect-form") as HTMLFormElement;
  const field = (n: string) => form.elements.namedItem(n) as HTMLInputElement;
  // 认证方式：自定义下拉（值 authSel.getValue()），改动视为手动编辑
  const syncAuthUI = () => {
    const v = authSel.getValue();
    (overlay.querySelector(".auth-key") as HTMLElement).style.display = v === "key" ? "" : "none";
    (overlay.querySelector(".auth-pass") as HTMLElement).style.display =
      v === "password" ? "" : "none";
  };
  const authSel = customSelect(
    [
      { value: "key", label: "私钥（直接证书验证）" },
      { value: "password", label: "密码" },
    ],
    "key",
    () => {
      connectProfileId = null;
      syncAuthUI();
    },
  );
  (overlay.querySelector('.cs-mount[data-cs="auth"]') as HTMLElement).appendChild(authSel.el);

  let selectedProfileId: string | null = null;
  // 连接来源的连接项 id（供会话恢复）：仅当表单未被用户改动、直接来自某连接项时有效。
  // 一旦手动编辑任一字段即清空 → 视为临时连接，不参与会话恢复。
  let connectProfileId: string | null = null;
  form.addEventListener("input", () => (connectProfileId = null));
  form.addEventListener("change", () => (connectProfileId = null));

  const fillForm = (p: Profile) => {
    selectedProfileId = p.id;
    connectProfileId = p.id;
    field("name").value = p.name;
    field("host").value = p.host;
    field("port").value = String(p.port);
    field("user").value = p.user;
    authSel.setValue(p.auth);
    field("keyPath").value = p.keyPath ?? "";
    field("keyData").value = p.keyData ?? "";
    field("note").value = p.note ?? "";
    field("keepalive").value = p.keepalive != null ? String(p.keepalive) : "";
    field("timeout").value = p.timeout != null ? String(p.timeout) : "";
    syncAuthUI();
  };

  const itemsEl = overlay.querySelector(".profile-items") as HTMLElement;
  const renderProfiles = (profiles: Profile[]) => {
    itemsEl.textContent = "";
    // 最上方固定：本地终端（不建立 SSH 连接，直接运行本机 shell）
    if (onLocal) {
      const local = document.createElement("div");
      local.className = "profile-item local";
      local.innerHTML = `<b>本地终端</b><small>直接运行本机 shell（bash）</small><span class="badge">local</span>`;
      local.addEventListener("click", async () => {
        await onLocal();
        close();
      });
      itemsEl.appendChild(local);
    }
    if (profiles.length === 0) {
      itemsEl.insertAdjacentHTML(
        "beforeend",
        `<p class="hint">暂无 SSH 连接项，请在右侧手动填写。</p>`,
      );
    }
    for (const p of profiles) {
      const item = document.createElement("div");
      item.className = "profile-item";
      item.innerHTML = `<b></b><small></small><em class="note"></em><span class="badge"></span>`;
      item.querySelector("b")!.textContent = p.name;
      item.querySelector("small")!.textContent = `${p.user}@${p.host}:${p.port}`;
      const noteEl = item.querySelector(".note") as HTMLElement;
      if (p.note) noteEl.textContent = p.note;
      else noteEl.remove();
      item.querySelector(".badge")!.textContent = p.source === "ssh_config" ? "ssh config" : "已保存";
      item.addEventListener("click", () => {
        itemsEl.querySelectorAll(".profile-item").forEach((i) => i.classList.remove("selected"));
        item.classList.add("selected");
        fillForm(p);
      });
      item.addEventListener("dblclick", () => form.requestSubmit());
      itemsEl.appendChild(item);
    }
  };
  void api.profilesList().then(renderProfiles).catch(() => renderProfiles([]));

  // 预填（hssh 无密码时复用）：填好字段并聚焦密码/口令框，让用户直接输入后回车连接。
  if (prefill?.kind === "profile") {
    fillForm(prefill.profile);
    requestAnimationFrame(() =>
      (prefill.profile.auth === "password" ? field("password") : field("passphrase"))?.focus(),
    );
  } else if (prefill?.kind === "adhoc") {
    field("name").value = prefill.host;
    field("host").value = prefill.host;
    if (prefill.user) field("user").value = prefill.user;
    if (prefill.port) field("port").value = String(prefill.port);
    authSel.setValue("password");
    syncAuthUI();
    connectProfileId = null;
    requestAnimationFrame(() => field("password").focus());
  }

  // 从文件导入：读取私钥内容填入文本框（随后随连接项自存，不再依赖该文件路径）
  overlay.querySelector(".browse-key")!.addEventListener("click", async () => {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({ multiple: false, title: "选择私钥文件" });
    if (typeof picked !== "string") return;
    try {
      const content = await api.readKeyFile(picked);
      field("keyData").value = content;
      field("keyPath").value = ""; // 已内联内容，清除路径依赖
      // 程序化赋值不触发 input 事件，需手动视作"表单已改动"：置空来源连接项，
      // 否则会话恢复会误用连接项里原来的密钥而非刚导入的这把。
      connectProfileId = null;
      toast("已导入私钥内容");
    } catch (err) {
      toast(`读取私钥失败: ${err}`, true);
    }
  });

  const numOrUndef = (n: string): number | undefined => {
    const v = parseInt(field(n).value, 10);
    return Number.isFinite(v) && v > 0 ? v : undefined;
  };

  const paramsFromForm = (): ConnParams | null => {
    const name = field("name").value.trim() || field("host").value.trim();
    const host = field("host").value.trim();
    const user = field("user").value.trim();
    if (!host || !user) return null;
    return {
      name,
      host,
      port: parseInt(field("port").value, 10) || 22,
      user,
      auth: authSel.getValue() as "key" | "password",
      keyPath: field("keyPath").value.trim() || undefined,
      keyData: field("keyData").value.trim() || undefined,
      passphrase: field("passphrase").value || undefined,
      password: field("password").value || undefined,
      keepalive: numOrUndef("keepalive"),
      timeout: numOrUndef("timeout"),
    };
  };

  overlay.querySelector(".save-profile")!.addEventListener("click", async () => {
    const p = paramsFromForm();
    if (!p) {
      toast("请先填写主机与用户名", true);
      return;
    }
    const profile: Profile = {
      id: selectedProfileId?.startsWith("sshcfg:") || !selectedProfileId
        ? crypto.randomUUID()
        : selectedProfileId,
      name: p.name,
      host: p.host,
      port: p.port,
      user: p.user,
      auth: p.auth,
      keyPath: p.keyPath ?? null,
      keyData: p.keyData ?? null,
      source: "manual",
      note: field("note").value.trim() || null,
      keepalive: p.keepalive ?? null,
      timeout: p.timeout ?? null,
    };
    await api.profileSave(profile);
    // 保存后表单即代表该连接项，随后「连接」可参与会话恢复
    selectedProfileId = profile.id;
    connectProfileId = profile.id;
    toast("连接项已保存");
    void api.profilesList().then(renderProfiles);
  });

  const close = () => overlay.remove();
  overlay.querySelector('[data-act="cancel"]')!.addEventListener("click", close);
  overlay.addEventListener("mousedown", (e) => {
    if (e.target === overlay) close();
  });

  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const p = paramsFromForm();
    if (!p) return;
    if (p.auth === "key" && !p.keyData && !p.keyPath) {
      toast("私钥认证需要粘贴私钥内容或从文件导入", true);
      return;
    }
    if (p.auth === "password" && !p.password) {
      toast("请输入密码", true);
      return;
    }
    const submitBtn = form.querySelector('[type="submit"]') as HTMLButtonElement;
    submitBtn.disabled = true;
    submitBtn.textContent = "连接中…";
    try {
      await onConnect(p, connectProfileId ?? undefined);
      close();
    } catch (err) {
      toast(String(err), true);
      submitBtn.disabled = false;
      submitBtn.textContent = "连接";
    }
  });
}

// ---------- 设置对话框 ----------

/** 自定义主题中可编辑的颜色键（与 xterm ITheme 对齐） */
const THEME_COLOR_KEYS = [
  "foreground", "background", "cursor",
  "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
  "brightBlack", "brightRed", "brightGreen", "brightYellow",
  "brightBlue", "brightMagenta", "brightCyan", "brightWhite",
];

export function showSettingsDialog() {
  const s = getSettings();
  const overlay = document.createElement("div");
  // peek：设置弹窗遮罩更淡，方便实时预览主题/透明度/模糊改动
  overlay.className = "modal-overlay peek";
  // 先展示轻量骨架（标题 + loading），让用户立即看到点击反馈，
  // 再用 rAF 延后填充完整内容（7 个 section + 33 张主题卡片 + 快捷键列表），
  // 避免大 innerHTML 解析 + backdrop-filter 合成阻塞主线程导致点击无响应感。
  overlay.innerHTML = `
    <div class="modal settings">
      <h3>设置</h3>
      <div class="settings-loading" style="padding:40px;text-align:center;opacity:0.6">加载中…</div>
    </div>`;
  document.body.appendChild(overlay);

  const fillContent = () => {
    const modal = overlay.querySelector(".modal")!;
    modal.innerHTML = `
      <h3>设置</h3>
      <div class="settings-body">
        <section>
          <h4>字体</h4>
          <p class="section-desc">字重并入字体名（如 JetBrains Mono NL Light）。列表顶部为推荐字体，分隔线下为本机已装字体；等宽与 CJK 可各自选择。</p>
          <div class="row">
            <label class="grow">主字体（英文 / 代码）<span class="cs-mount" data-cs="fontFamily"></span></label>
            <label class="grow">CJK 字体（中日韩）<span class="cs-mount" data-cs="cjkFontFamily"></span></label>
          </div>
          <div class="row">
            <label class="narrow">字号 <input name="fontSize" type="number" min="8" max="32"></label>
          </div>
          <div class="row">
            <label class="grow">标签页字体（空=同主字体）<input name="tabFontFamily" spellcheck="false" placeholder="同主字体"></label>
            <label class="narrow">标签字号 <input name="tabFontSize" type="number" min="0" max="24" placeholder="12"></label>
          </div>
        </section>
        <section>
          <h4>主题</h4>
          <p class="section-desc">内置 25 套暗色、18 套亮色/中性（含 Sweet 糖果系列，按名称排序），可基于任一主题派生自定义配色。点击色板即可切换。</p>
          <div class="theme-picker-head">
            <span>配色主题</span>
            <button type="button" class="btn new-theme">基于当前新建</button>
          </div>
          <div class="theme-groups"></div>
          <div class="row" style="margin-top:10px">
            <label class="check grow"><input name="titlebarFollow" type="checkbox"> 标题栏颜色跟随主题</label>
            <label class="narrow">自定义 <input name="titlebarColor" type="color"></label>
          </div>
          <div class="theme-editor" style="display:none"></div>
        </section>
        <section>
          <h4>窗口</h4>
          <p class="section-desc">半透明与毛玻璃效果依赖系统合成器（macOS / Windows / KDE 等）。</p>
          <label>背景不透明度 <span class="opacity-val"></span>
            <input name="opacity" type="range" min="0.3" max="1" step="0.01"></label>
          <label class="check"><input name="bgBlur" type="checkbox"> 背景虚化（终端整体背景光晕/玻璃效果；关闭仅保留透明度，弹窗模糊不受影响）</label>
          <label class="check"><input name="blur" type="checkbox"> 毛玻璃虚化（透明时仍保持终端内容清晰）</label>
          <label>模糊程度 <span class="blur-val"></span>
            <input name="blurAmount" type="range" min="0" max="100" step="1"></label>
          <label class="check"><input name="frosted" type="checkbox"> 磨砂质感（同色系细颗粒，独立于毛玻璃）</label>
          <label>磨砂程度 <span class="frost-val"></span>
            <input name="frostStrength" type="range" min="0" max="100" step="1"></label>
          <label>还原尺寸 <span class="restore-size-val"></span>
            <input name="restoreSize" type="range" min="50" max="90" step="1"></label>
          <label>图片预览上限 <span class="max-image-mb-val"></span>
            <input name="maxImageMb" type="range" min="32" max="512" step="1"></label>
          <div class="settings-field">
            <span>界面圆角</span>
            <div class="radius-picker"></div>
          </div>
          <div class="settings-field">
            <div class="cursor-field-header">
              <span>光标样式</span>
              <button type="button" class="btn cursor-reset" title="重置为跟随主题">重置</button>
            </div>
            <div class="cursor-field-row">
              <span class="cursor-sub-label">形状</span>
              <div class="cursor-picker"></div>
              <span class="cursor-sub-label">颜色</span>
              <input name="cursorColor" type="color" class="color-input">
            </div>
          </div>
        </section>
        <section>
          <h4>标签页</h4>
          <label class="check"><input name="tabBarFill" type="checkbox"> 标签页平分横向宽度</label>
          <p class="section-desc">仅一个标签页时不显示标签栏；新建标签页用工具栏“+”或 Ctrl+Shift+T。</p>
        </section>
        <section>
          <h4>行为</h4>
          <label>点击“+”新建标签页时 <span class="cs-mount" data-cs="newTabMode"></span></label>
          <label class="check"><input name="autoReconnect" type="checkbox"> 连接断开后自动重连</label>
          <label class="check"><input name="copyOnSelect" type="checkbox"> 选中文本即复制到剪贴板</label>
          <label class="check"><input name="showScrollbar" type="checkbox"> 显示终端滚动条</label>
          <label class="check"><input name="webgl" type="checkbox"> WebGL 硬件加速渲染（关闭后回退 Canvas，可解决部分 GPU 驱动导致的乱码；乱码时按 Ctrl+Shift+R 重建）</label>
          <label class="check"><input name="mcrEnabled" type="checkbox"> 最小对比度提亮（MCR）<input name="mcrMax" type="number" min="1.1" max="2" step="0.01" placeholder="1.6" style="width:56px;margin-left:8px"></label>
          <p class="section-desc">按透明度自适应提亮前景色对比度，值越高文字越清晰但颜色偏移也越大（1.1–2.0）。</p>
          <label class="check"><input name="confirmOverwrite" type="checkbox"> 上传遇同名文件时提示确认（默认直接覆盖）</label>
          <label class="check"><input name="restoreSession" type="checkbox"> 记住最后的会话（下次启动自动重开并连接）</label>
          <label class="check"><input name="trackRemoteCwd" type="checkbox"> 追踪远程工作目录（连接时注入隐形标记，经 /proc 读实时目录）</label>
          <label>本地终端 Shell
            <div class="row">
              <input name="shell" class="grow" spellcheck="false" placeholder="默认（自动推断）">
              <button type="button" class="btn browse-shell">浏览</button>
            </div>
          </label>
          <p class="section-desc">空或"默认"按平台自动选择（Linux/macOS 用 $SHELL，Windows 用 PowerShell）；可填命令名（如 zsh）或绝对路径。启动失败时自动回退到默认。</p>
        </section>
        <section>
          <h4>下载</h4>
          <label>默认下载目录（空 = 系统 Downloads）
            <div class="row">
              <input name="downloadDir" class="grow" spellcheck="false" placeholder="留空自动">
              <button type="button" class="btn browse-dl">浏览</button>
            </div>
          </label>
          <label class="check"><input name="askDownloadLocation" type="checkbox"> 每次下载都询问保存位置</label>
          <p class="section-desc">右键或 Ctrl+单击终端里的文件即可下载；Ctrl+拖到右侧文件管理器则下载到对应目录。</p>
        </section>
        <section>
          <h4>快捷键</h4>
          <p class="section-desc">点击右侧组合键即可修改；按下新组合键生效，Esc 取消。终端聚焦时快捷键优先于 shell。</p>
          <div class="keybind-list"></div>
          <div class="modal-actions" style="margin-top:8px">
            <button type="button" class="btn kb-reset">恢复默认快捷键</button>
          </div>
        </section>
      </div>
      <div class="modal-actions">
        <button type="button" class="btn" data-act="close">关闭</button>
      </div>
    </div>`;

  const q = <T extends HTMLElement>(sel: string) => overlay.querySelector(sel) as T;
  const input = (n: string) => overlay.querySelector(`[name="${n}"]`) as HTMLInputElement;

  input("fontSize").value = String(s.fontSize);
  input("opacity").value = String(s.opacity);
  q<HTMLElement>(".opacity-val").textContent = `${Math.round(s.opacity * 100)}%`;
  input("bgBlur").checked = s.bgBlur;
  input("blur").checked = s.blur;
  input("blurAmount").value = String(s.blurAmount);
  q<HTMLElement>(".blur-val").textContent = `${Math.round(s.blurAmount)}px`;
  input("frosted").checked = s.frosted;
  input("frostStrength").value = String(s.frostStrength);
  q<HTMLElement>(".frost-val").textContent = `${s.frostStrength}%`;
  input("restoreSize").value = String(s.restoreSize);
  q<HTMLElement>(".restore-size-val").textContent = `${s.restoreSize}%`;
  input("maxImageMb").value = String(s.maxImageMb ?? 128);
  q<HTMLElement>(".max-image-mb-val").textContent = `${s.maxImageMb ?? 128} MB`;
  input("autoReconnect").checked = s.autoReconnect;
  input("copyOnSelect").checked = s.copyOnSelect;
  input("showScrollbar").checked = s.showScrollbar;
  input("webgl").checked = s.webgl;
  input("mcrEnabled").checked = s.mcrEnabled;
  input("mcrMax").value = String(s.mcrMax ?? 1.6);
  input("confirmOverwrite").checked = s.confirmOverwrite;
  input("restoreSession").checked = s.restoreSession;
  input("tabFontFamily").value = s.tabFontFamily;
  input("tabFontSize").value = s.tabFontSize ? String(s.tabFontSize) : "";
  input("tabBarFill").checked = s.tabBarFill;
  input("trackRemoteCwd").checked = s.trackRemoteCwd;
  input("shell").value = s.shell ?? "";
  input("downloadDir").value = s.downloadDir;
  input("askDownloadLocation").checked = s.askDownloadLocation;
  overlay.querySelector(".browse-dl")!.addEventListener("click", async () => {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({ directory: true, title: "选择默认下载目录" });
    if (typeof picked === "string") {
      input("downloadDir").value = picked;
      commit();
    }
  });
  overlay.querySelector(".browse-shell")!.addEventListener("click", async () => {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({ title: "选择终端 Shell 可执行文件" });
    if (typeof picked === "string") {
      input("shell").value = picked;
      commit();
    }
  });

  // 圆角级别选择器：小图标（方形 → 大圆角），不显示文字
  let cornerRadius = s.cornerRadius;
  const radiusPicker = q<HTMLElement>(".radius-picker");
  // 用宽于高的圆角矩形（而非正方形）呈现，rx 远小于半高，五级都清晰可辨、不会退化成圆
  const RADII: Array<{ v: Settings["cornerRadius"]; r: number }> = [
    { v: "square", r: 0 }, { v: "xs", r: 2 }, { v: "sm", r: 4 }, { v: "md", r: 6 }, { v: "lg", r: 8 },
  ];
  const renderRadius = () => {
    radiusPicker.textContent = "";
    for (const { v, r } of RADII) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "radius-opt" + (v === cornerRadius ? " selected" : "");
      b.title = v;
      b.innerHTML = `<svg viewBox="0 0 30 24" width="26" height="21"><rect x="3" y="4" width="24" height="16" rx="${r}" fill="none" stroke="currentColor" stroke-width="2.2"/></svg>`;
      b.addEventListener("click", () => {
        cornerRadius = v;
        renderRadius();
        commit();
      });
      radiusPicker.appendChild(b);
    }
  };
  renderRadius();

  // 光标样式选择器（复用 radius-picker 的按钮组模式）
  const cursorPicker = q<HTMLElement>(".cursor-picker");
  let cursorStyle: "block" | "bar" = s.cursorStyle === "bar" ? "bar" : "block";
  const renderCursor = () => {
    cursorPicker.textContent = "";
    const opts: Array<{ v: "block" | "bar"; label: string; svg: string }> = [
      { v: "block", label: "方块", svg: '<rect x="8" y="4" width="14" height="16" fill="currentColor"/>' },
      { v: "bar", label: "竖线", svg: '<rect x="12" y="4" width="3" height="16" fill="currentColor"/>' },
    ];
    for (const { v, label, svg } of opts) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "radius-opt" + (v === cursorStyle ? " selected" : "");
      b.title = label;
      b.innerHTML = `<svg viewBox="0 0 30 24" width="26" height="21">${svg}</svg>`;
      b.addEventListener("click", () => {
        cursorStyle = v;
        renderCursor();
        commit();
      });
      cursorPicker.appendChild(b);
    }
  };
  renderCursor();

  // 光标颜色选择器
  const cursorColorInput = input("cursorColor") as HTMLInputElement;
  const cursorResetBtn = q<HTMLElement>(".cursor-reset");
  // null = 跟随主题；有值 = 自定义颜色
  let cursorColor: string | null = s.cursorColor ?? null;
  if (cursorColor && /^#[0-9a-fA-F]{6}$/.test(cursorColor)) {
    cursorColorInput.value = cursorColor;
  } else {
    cursorColorInput.value = "#88c0d0"; // 占位颜色，实际跟随主题
  }
  cursorColorInput.addEventListener("input", () => {
    const v = cursorColorInput.value.trim();
    cursorColor = /^#[0-9a-fA-F]{6}$/.test(v) ? v : null;
    debouncedCommit();
  });
  cursorResetBtn.addEventListener("click", () => {
    cursorColor = null;
    cursorColorInput.value = "#88c0d0";
    debouncedCommit();
  });
  const toOpts = (names: string[]) => names.map((n) => ({ value: n, label: n }));
  // 字体列表可能很长：限高 280px 可滚动，弹框紧凑、顶部内置默认字体始终可见
  const monoFontSel = customSelect(toOpts(MONO_FONTS), s.fontFamily, () => commit(), 280);
  const cjkFontSel = customSelect(toOpts(CJK_FONTS), s.cjkFontFamily, () => commit(), 280);
  q<HTMLElement>('.cs-mount[data-cs="fontFamily"]').appendChild(monoFontSel.el);
  q<HTMLElement>('.cs-mount[data-cs="cjkFontFamily"]').appendChild(cjkFontSel.el);
  // 异步注入系统字体：推荐组 + 分隔线 + 系统字体（去掉与推荐重复者）
  void api
    .listFonts()
    .then((sys) => {
      const build = (defaults: string[]): CSOption[] => {
        const extra = sys.filter((f) => !defaults.includes(f));
        return extra.length
          ? [...toOpts(defaults), { separator: true }, ...toOpts(extra)]
          : toOpts(defaults);
      };
      monoFontSel.setOptions(build(MONO_FONTS));
      cjkFontSel.setOptions(build(CJK_FONTS));
    })
    .catch(() => {});
  const newTabModeSel = customSelect(
    [
      { value: "local", label: "直接打开本地终端" },
      { value: "dialog", label: "弹出连接选择" },
    ],
    s.newTabMode,
    () => commit(),
  );
  q<HTMLElement>('.cs-mount[data-cs="newTabMode"]').appendChild(newTabModeSel.el);

  // ---------- 快捷键编辑 ----------
  let kbOverrides: Record<string, string> = { ...s.keybindings };
  const kbList = q<HTMLElement>(".keybind-list");
  // 当前正在捕获时的清理函数：关闭弹窗时必须调用，否则捕获监听器泄漏、吞掉全部键盘输入
  let cancelCapture: (() => void) | null = null;
  // 允许作为触发键的功能键（无修饰键时也可用，如 Shift+Tab、F5、方向键）
  const SPECIAL_KEYS = new Set([
    "Tab", "Escape", "Enter", "Space", "Backspace",
    "ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown",
    "Home", "End", "PageUp", "PageDown", "Insert", "Delete",
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
  ]);
  const renderKeybinds = () => {
    const eff = resolveBindings(kbOverrides);
    kbList.textContent = "";
    for (const { action, label } of ACTIONS) {
      const row = document.createElement("div");
      row.className = "keybind-row";
      const name = document.createElement("span");
      name.textContent = label;
      const keyBtn = document.createElement("button");
      keyBtn.type = "button";
      keyBtn.className = "btn kb-key";
      keyBtn.textContent = eff[action] ? comboToLabel(eff[action]) : "未绑定";
      keyBtn.addEventListener("click", () => captureKey(action, keyBtn));
      row.append(name, keyBtn);
      kbList.appendChild(row);
    }
  };
  const captureKey = (action: Action, btn: HTMLButtonElement) => {
    cancelCapture?.(); // 同一时刻只允许一个捕获
    btn.textContent = "按下组合键…";
    btn.classList.add("capturing");
    const onKey = (ev: KeyboardEvent) => {
      ev.preventDefault();
      ev.stopPropagation();
      // 纯修饰键不作为触发键，等待完整组合
      if (["Control", "Alt", "Shift", "Meta"].includes(ev.key)) return;
      if (ev.code === "Escape") {
        cleanup();
        renderKeybinds();
        return;
      }
      // 拒绝无修饰键的普通字符（否则会吞掉终端里正常打字）
      const hasMod = ev.ctrlKey || ev.altKey || ev.metaKey;
      if (!hasMod && !SPECIAL_KEYS.has(ev.code)) {
        toast("请使用带 Ctrl/Alt 修饰键，或功能键（Tab/方向键/F 键）的组合", true);
        cleanup();
        renderKeybinds();
        return;
      }
      const combo = eventToCombo(ev);
      // 该组合若已被其它动作占用，先解绑对方（避免两动作同组合导致其一永久失效）
      const eff = resolveBindings(kbOverrides);
      for (const other of Object.keys(eff) as Action[]) {
        if (other !== action && eff[other] === combo) kbOverrides[other] = "";
      }
      // 与默认相同则移除覆盖，否则记录覆盖
      if (DEFAULT_KEYBINDINGS[action] === combo) delete kbOverrides[action];
      else kbOverrides[action] = combo;
      cleanup();
      void updateSettings({ keybindings: kbOverrides });
      renderKeybinds();
    };
    const cleanup = () => {
      window.removeEventListener("keydown", onKey, true);
      btn.classList.remove("capturing");
      cancelCapture = null;
    };
    cancelCapture = cleanup;
    window.addEventListener("keydown", onKey, true);
  };
  // 重内容延后一帧渲染，让弹窗外壳先瞬间出现（消除卡顿感）
  requestAnimationFrame(renderKeybinds);
  q<HTMLButtonElement>(".kb-reset").addEventListener("click", () => {
    kbOverrides = {};
    void updateSettings({ keybindings: {} });
    renderKeybinds();
  });

  // 当前选中主题 id（由色板卡片维护，切换主题仅 patch theme，不动透明度/模糊）
  let selectedThemeId = s.theme;

  const groupsEl = overlay.querySelector(".theme-groups") as HTMLElement;
  const renderThemeGroups = () => {
    groupsEl.textContent = "";
    const themes = allThemes(getSettings().customThemes);
    const groups: Array<[string, (t: ThemeDef) => boolean]> = [
      ["暗色", (t) => t.base === "dark" && BUILTIN_THEMES.includes(t)],
      ["亮色", (t) => t.base === "light" && BUILTIN_THEMES.includes(t)],
      ["自定义", (t) => !BUILTIN_THEMES.includes(t)],
    ];
    for (const [label, filter] of groups) {
      const matched = themes.filter(filter);
      if (matched.length === 0) continue;
      const groupLabel = document.createElement("div");
      groupLabel.className = "theme-group-label";
      groupLabel.textContent = label;
      groupsEl.appendChild(groupLabel);
      const grid = document.createElement("div");
      grid.className = "theme-grid";
      const custom = label === "自定义";
      // 按名称排序展示
      const sorted = [...matched].sort((a, b) => a.name.localeCompare(b.name, "zh"));
      for (const t of sorted) {
        const card = makeThemeCard(
          t,
          t.id === selectedThemeId,
          () => selectTheme(t.id),
          custom ? () => deleteTheme(t.id) : undefined,
          custom ? () => editTheme(t) : undefined,
        );
        card.dataset.themeId = t.id;
        grid.appendChild(card);
      }
      groupsEl.appendChild(grid);
    }
  };

  const selectTheme = (id: string) => {
    selectedThemeId = id;
    // 仅切主题，opacity/blur/其它设置保持不变
    void updateSettings({ theme: id });
    // 只切换选中态高亮，不重建整个卡片网格（避免每次点击闪烁/卡顿）
    groupsEl.querySelectorAll(".theme-card").forEach((c) => {
      c.classList.toggle("selected", (c as HTMLElement).dataset.themeId === id);
    });
    syncTitlebarUI();
  };

  const deleteTheme = (id: string) => {
    const custom = getSettings().customThemes.filter((t) => t.id !== id);
    // 若删的是当前主题，回落到其归一化的明暗默认主题（base 可能是某内置主题 id）
    let theme = getSettings().theme;
    if (theme === id) {
      const removed = getSettings().customThemes.find((t) => t.id === id);
      const baseBuiltin = BUILTIN_THEMES.find((t) => t.id === removed?.base);
      theme = baseBuiltin?.base === "light" ? "light" : "dark";
      selectedThemeId = theme;
    }
    void updateSettings({ customThemes: custom, theme });
    renderThemeGroups();
    syncTitlebarUI();
  };

  // 编辑已保存的自定义主题：用其副本打开逐色编辑器，保存后原地更新（id 不变）。
  // 不改变当前启用的主题——仅当被编辑的正是当前主题时，applySettings 会自动重应用新配色。
  const editTheme = (theme: ThemeDef) => {
    const editor = q<HTMLElement>(".theme-editor");
    const draft: ThemeDef = { ...theme, colors: { ...theme.colors } };
    renderThemeEditor(editor, draft, async () => {
      const custom = getSettings().customThemes.map((t) => (t.id === draft.id ? draft : t));
      await updateSettings({ customThemes: custom });
      renderThemeGroups();
      syncTitlebarUI();
      editor.style.display = "none";
      toast("主题已更新");
    });
    editor.style.display = "";
  };

  // 标题栏颜色：默认跟随主题，取消勾选后可单独取色
  const followInput = input("titlebarFollow");
  const titlebarPicker = input("titlebarColor");
  const syncTitlebarUI = () => {
    followInput.checked = !getSettings().titlebarColor;
    titlebarPicker.value =
      getSettings().titlebarColor ??
      (resolveTheme(selectedThemeId, getSettings().customThemes).colors.background ?? "#10151c").slice(0, 7);
    titlebarPicker.disabled = followInput.checked;
  };
  requestAnimationFrame(renderThemeGroups); // 33 张色板卡片延后一帧，弹窗先出现
  syncTitlebarUI();
  followInput.addEventListener("change", () => {
    titlebarPicker.disabled = followInput.checked;
  });

  const commit = () => {
    void updateSettings({
      fontFamily: monoFontSel.getValue(),
      cjkFontFamily: cjkFontSel.getValue(),
      fontSize: parseInt(input("fontSize").value, 10) || 16,
      fontWeight: "normal",
      theme: selectedThemeId,
      titlebarColor: input("titlebarFollow").checked ? null : input("titlebarColor").value,
      opacity: parseFloat(input("opacity").value),
      bgBlur: input("bgBlur").checked,
      blur: input("blur").checked,
      blurAmount: parseFloat(input("blurAmount").value),
      frosted: input("frosted").checked,
      frostStrength: parseInt(input("frostStrength").value, 10) || 0,
      restoreSize: parseInt(input("restoreSize").value, 10) || 78,
      maxImageMb: Math.max(32, Math.min(512, parseInt(input("maxImageMb").value, 10) || 128)),
      cursorStyle,
      cursorColor,
      newTabMode: newTabModeSel.getValue() as "local" | "dialog",
      autoReconnect: input("autoReconnect").checked,
      copyOnSelect: input("copyOnSelect").checked,
      showScrollbar: input("showScrollbar").checked,
      webgl: input("webgl").checked,
      mcrEnabled: input("mcrEnabled").checked,
      mcrMax: Math.max(1.1, Math.min(2, parseFloat(input("mcrMax").value) || 1.6)),
      confirmOverwrite: input("confirmOverwrite").checked,
      restoreSession: input("restoreSession").checked,
      cornerRadius,
      tabBarFill: input("tabBarFill").checked,
      tabFontFamily: input("tabFontFamily").value,
      tabFontSize: parseInt(input("tabFontSize").value, 10) || 0,
      trackRemoteCwd: input("trackRemoteCwd").checked,
      shell: input("shell").value.trim(),
      downloadDir: input("downloadDir").value.trim(),
      askDownloadLocation: input("askDownloadLocation").checked,
    });
  };

  overlay.querySelectorAll("input, select").forEach((el) => {
    el.addEventListener("change", commit);
  });
  // 滑条拖动时 input 事件高频触发：每次 commit 都会 IPC 写盘 + 遍历所有终端
  // refit()，导致拖动卡顿。debounce 90ms，change（松手）时已有通用绑定立即 commit。
  let commitTimer: number | undefined;
  const debouncedCommit = () => {
    window.clearTimeout(commitTimer);
    commitTimer = window.setTimeout(commit, 90);
  };
  input("opacity").addEventListener("input", () => {
    q<HTMLElement>(".opacity-val").textContent =
      `${Math.round(parseFloat(input("opacity").value) * 100)}%`;
    debouncedCommit();
  });
  input("blurAmount").addEventListener("input", () => {
    q<HTMLElement>(".blur-val").textContent = `${Math.round(parseFloat(input("blurAmount").value))}px`;
    debouncedCommit();
  });
  input("frostStrength").addEventListener("input", () => {
    q<HTMLElement>(".frost-val").textContent = `${input("frostStrength").value}%`;
    debouncedCommit();
  });
  input("restoreSize").addEventListener("input", () => {
    q<HTMLElement>(".restore-size-val").textContent = `${input("restoreSize").value}%`;
    debouncedCommit();
  });
  input("maxImageMb").addEventListener("input", () => {
    q<HTMLElement>(".max-image-mb-val").textContent = `${input("maxImageMb").value} MB`;
    debouncedCommit();
  });

  // 基于当前主题新建自定义主题
  const editor = q<HTMLElement>(".theme-editor");
  overlay.querySelector(".new-theme")!.addEventListener("click", () => {
    const base = resolveTheme(selectedThemeId, getSettings().customThemes);
    const draft: ThemeDef = {
      id: crypto.randomUUID(),
      name: `${base.name} · 自定义`,
      base: BUILTIN_THEMES.some((t) => t.id === base.id) ? (base.id as "dark" | "light") : base.base,
      colors: { ...base.colors },
    };
    renderThemeEditor(editor, draft, async () => {
      const custom = [...getSettings().customThemes, draft];
      selectedThemeId = draft.id;
      await updateSettings({ customThemes: custom, theme: draft.id });
      renderThemeGroups();
      syncTitlebarUI();
      editor.style.display = "none";
      toast("自定义主题已保存并启用");
    });
    editor.style.display = "";
  });

  const close = () => {
    cancelCapture?.(); // 关闭前解除可能仍在进行的快捷键捕获，防止监听器泄漏
    overlay.remove();
  };
  overlay.querySelector('[data-act="close"]')!.addEventListener("click", close);
  overlay.addEventListener("mousedown", (e) => {
    if (e.target === overlay) close();
  });
  };

  requestAnimationFrame(fillContent);
}

/** 主题配色示例卡片：背景 + 前景示意文字 + 一排 ANSI 色板；自定义主题带删除按钮 */
function makeThemeCard(
  theme: ThemeDef,
  selected: boolean,
  onClick: () => void,
  onDelete?: () => void,
  onEdit?: () => void,
): HTMLElement {
  const c = theme.colors;
  const card = document.createElement("button");
  card.type = "button";
  card.className = "theme-card" + (selected ? " selected" : "");
  card.style.background = c.background ?? "#000";
  card.style.color = c.foreground ?? "#fff";
  card.title = theme.name;

  if (onEdit || onDelete) {
    const bar = document.createElement("div");
    bar.className = "tc-actions";
    if (onEdit) {
      const edit = document.createElement("span");
      edit.className = "tc-act tc-edit";
      edit.textContent = "✎";
      edit.title = "编辑此自定义主题";
      edit.addEventListener("click", (e) => {
        e.stopPropagation();
        onEdit();
      });
      bar.appendChild(edit);
    }
    if (onDelete) {
      const del = document.createElement("span");
      del.className = "tc-act tc-del";
      del.textContent = "×";
      del.title = "删除此自定义主题";
      del.addEventListener("click", (e) => {
        e.stopPropagation();
        onDelete();
      });
      bar.appendChild(del);
    }
    card.appendChild(bar);
  }

  const preview = document.createElement("div");
  preview.className = "tc-preview";
  // 提示符 + 命令样例，用前景/绿/蓝色呈现，直观看出对比度
  preview.innerHTML =
    `<span style="color:${c.green ?? c.foreground}">➜</span> ` +
    `<span style="color:${c.blue ?? c.foreground}">~/hetu</span> ` +
    `<span style="color:${c.foreground}">ls</span>`;

  const chips = document.createElement("div");
  chips.className = "tc-chips";
  for (const key of ["red", "green", "yellow", "blue", "magenta", "cyan"]) {
    const chip = document.createElement("span");
    chip.style.background = c[key] ?? "#888";
    chips.appendChild(chip);
  }

  const name = document.createElement("div");
  name.className = "tc-name";
  name.textContent = theme.name;

  card.append(preview, chips, name);
  card.addEventListener("click", onClick);
  return card;
}

function renderThemeEditor(container: HTMLElement, draft: ThemeDef, onSave: () => void) {
  container.textContent = "";
  const nameLabel = document.createElement("label");
  nameLabel.textContent = "主题名称 ";
  const nameInput = document.createElement("input");
  nameInput.value = draft.name;
  nameInput.addEventListener("input", () => (draft.name = nameInput.value));
  nameLabel.appendChild(nameInput);
  container.appendChild(nameLabel);

  const grid = document.createElement("div");
  grid.className = "color-grid";
  for (const key of THEME_COLOR_KEYS) {
    const cell = document.createElement("label");
    cell.className = "color-cell";
    const picker = document.createElement("input");
    picker.type = "color";
    picker.value = (draft.colors[key] ?? "#000000").slice(0, 7);
    picker.addEventListener("input", () => (draft.colors[key] = picker.value));
    const span = document.createElement("span");
    span.textContent = key;
    cell.append(picker, span);
    grid.appendChild(cell);
  }
  container.appendChild(grid);

  const actions = document.createElement("div");
  actions.className = "modal-actions";
  const saveBtn = document.createElement("button");
  saveBtn.className = "btn primary";
  saveBtn.textContent = "保存主题";
  saveBtn.addEventListener("click", onSave);
  actions.appendChild(saveBtn);
  container.appendChild(actions);
}
