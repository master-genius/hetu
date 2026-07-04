/** 连接对话框（新建标签页选择连接项）与设置对话框 */

import { api } from "./ipc";
import { getSettings, updateSettings } from "./settings";
import { allThemes, BUILTIN_THEMES, resolveTheme } from "./themes";
import { toast } from "./ui";
import type { ConnParams, Profile, ThemeDef } from "./types";

// ---------- 连接对话框 ----------

export function showConnectDialog(
  onConnect: (params: ConnParams) => Promise<void>,
  onLocal?: () => Promise<void>,
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
          <label>认证方式
            <select name="auth">
              <option value="key">私钥（直接证书验证）</option>
              <option value="password">密码</option>
            </select>
          </label>
          <div class="auth-key">
            <label>私钥路径
              <div class="row">
                <input name="keyPath" class="grow" placeholder="~/.ssh/id_ed25519">
                <button type="button" class="btn browse-key">浏览</button>
              </div>
            </label>
            <label>私钥口令（可选）<input name="passphrase" type="password" autocomplete="off"></label>
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
  const authSel = form.elements.namedItem("auth") as HTMLSelectElement;
  const syncAuthUI = () => {
    (overlay.querySelector(".auth-key") as HTMLElement).style.display =
      authSel.value === "key" ? "" : "none";
    (overlay.querySelector(".auth-pass") as HTMLElement).style.display =
      authSel.value === "password" ? "" : "none";
  };
  authSel.addEventListener("change", syncAuthUI);

  let selectedProfileId: string | null = null;

  const fillForm = (p: Profile) => {
    selectedProfileId = p.id;
    field("name").value = p.name;
    field("host").value = p.host;
    field("port").value = String(p.port);
    field("user").value = p.user;
    authSel.value = p.auth;
    field("keyPath").value = p.keyPath ?? "";
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

  overlay.querySelector(".browse-key")!.addEventListener("click", async () => {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({ multiple: false, title: "选择私钥文件" });
    if (typeof picked === "string") field("keyPath").value = picked;
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
      auth: authSel.value as "key" | "password",
      keyPath: field("keyPath").value.trim() || undefined,
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
      source: "manual",
      note: field("note").value.trim() || null,
      keepalive: p.keepalive ?? null,
      timeout: p.timeout ?? null,
    };
    await api.profileSave(profile);
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
    if (p.auth === "key" && !p.keyPath) {
      toast("私钥认证需要指定私钥路径", true);
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
      await onConnect(p);
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
  overlay.className = "modal-overlay";
  overlay.innerHTML = `
    <div class="modal settings">
      <h3>设置</h3>
      <div class="settings-body">
        <section>
          <h4>字体</h4>
          <p class="section-desc">未安装的字体自动回退到内置 JetBrains Mono NL / Noto Sans SC。</p>
          <div class="row">
            <label class="grow">主字体（英文 / 代码）<input name="fontFamily" spellcheck="false"></label>
            <label class="grow">CJK 字体（中日韩）<input name="cjkFontFamily" spellcheck="false"></label>
          </div>
          <div class="row">
            <label class="narrow">字号 <input name="fontSize" type="number" min="8" max="32"></label>
            <label class="grow">字重
              <select name="fontWeight">
                <option value="100">100 Thin</option>
                <option value="200">200 ExtraLight</option>
                <option value="300">300 Light</option>
                <option value="400">400 Regular</option>
                <option value="500">500 Medium</option>
              </select>
            </label>
          </div>
        </section>
        <section>
          <h4>主题</h4>
          <p class="section-desc">内置 16 套暗色、11 套亮色，可基于任一主题派生自定义配色。点击色板即可切换。</p>
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
          <label class="check"><input name="blur" type="checkbox"> 毛玻璃虚化（透明时仍保持终端内容清晰）</label>
        </section>
        <section>
          <h4>行为</h4>
          <label>点击“+”新建标签页时
            <select name="newTabMode">
              <option value="local">直接打开本地终端</option>
              <option value="dialog">弹出连接选择</option>
            </select>
          </label>
          <label class="check"><input name="autoReconnect" type="checkbox"> 连接断开后自动重连</label>
          <label class="check"><input name="copyOnSelect" type="checkbox"> 选中文本即复制到剪贴板</label>
          <label class="check"><input name="confirmOverwrite" type="checkbox"> 上传遇同名文件时提示确认（默认直接覆盖）</label>
          <label class="check"><input name="restoreSession" type="checkbox"> 记住最后的会话（下次启动自动重开并连接）</label>
        </section>
      </div>
      <div class="modal-actions">
        <button type="button" class="btn" data-act="close">关闭</button>
      </div>
    </div>`;
  document.body.appendChild(overlay);

  const q = <T extends HTMLElement>(sel: string) => overlay.querySelector(sel) as T;
  const input = (n: string) => overlay.querySelector(`[name="${n}"]`) as HTMLInputElement;

  input("fontFamily").value = s.fontFamily;
  input("cjkFontFamily").value = s.cjkFontFamily;
  input("fontSize").value = String(s.fontSize);
  (overlay.querySelector('[name="fontWeight"]') as HTMLSelectElement).value = s.fontWeight;
  input("opacity").value = String(s.opacity);
  q<HTMLElement>(".opacity-val").textContent = `${Math.round(s.opacity * 100)}%`;
  input("blur").checked = s.blur;
  (overlay.querySelector('[name="newTabMode"]') as HTMLSelectElement).value = s.newTabMode;
  input("autoReconnect").checked = s.autoReconnect;
  input("copyOnSelect").checked = s.copyOnSelect;
  input("confirmOverwrite").checked = s.confirmOverwrite;
  input("restoreSession").checked = s.restoreSession;

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
      for (const t of matched) {
        grid.appendChild(makeThemeCard(t, t.id === selectedThemeId, () => selectTheme(t.id)));
      }
      groupsEl.appendChild(grid);
    }
  };

  const selectTheme = (id: string) => {
    selectedThemeId = id;
    // 仅切主题，opacity/blur/其它设置保持不变
    void updateSettings({ theme: id });
    renderThemeGroups();
    syncTitlebarUI();
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
  renderThemeGroups();
  syncTitlebarUI();
  followInput.addEventListener("change", () => {
    titlebarPicker.disabled = followInput.checked;
  });

  const commit = () => {
    void updateSettings({
      fontFamily: input("fontFamily").value,
      cjkFontFamily: input("cjkFontFamily").value,
      fontSize: parseInt(input("fontSize").value, 10) || 14,
      fontWeight: (overlay.querySelector('[name="fontWeight"]') as HTMLSelectElement).value,
      theme: selectedThemeId,
      titlebarColor: input("titlebarFollow").checked ? null : input("titlebarColor").value,
      opacity: parseFloat(input("opacity").value),
      blur: input("blur").checked,
      newTabMode: (overlay.querySelector('[name="newTabMode"]') as HTMLSelectElement)
        .value as "local" | "dialog",
      autoReconnect: input("autoReconnect").checked,
      copyOnSelect: input("copyOnSelect").checked,
      confirmOverwrite: input("confirmOverwrite").checked,
      restoreSession: input("restoreSession").checked,
    });
  };

  overlay.querySelectorAll("input, select").forEach((el) => {
    el.addEventListener("change", commit);
  });
  input("opacity").addEventListener("input", () => {
    q<HTMLElement>(".opacity-val").textContent =
      `${Math.round(parseFloat(input("opacity").value) * 100)}%`;
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

  const close = () => overlay.remove();
  overlay.querySelector('[data-act="close"]')!.addEventListener("click", close);
  overlay.addEventListener("mousedown", (e) => {
    if (e.target === overlay) close();
  });
}

/** 主题配色示例卡片：背景 + 前景示意文字 + 一排 ANSI 色板 */
function makeThemeCard(theme: ThemeDef, selected: boolean, onClick: () => void): HTMLElement {
  const c = theme.colors;
  const card = document.createElement("button");
  card.type = "button";
  card.className = "theme-card" + (selected ? " selected" : "");
  card.style.background = c.background ?? "#000";
  card.style.color = c.foreground ?? "#fff";
  card.title = theme.name;

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
