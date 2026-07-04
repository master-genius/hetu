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
      item.innerHTML = `<b></b><small></small><span class="badge"></span>`;
      item.querySelector("b")!.textContent = p.name;
      item.querySelector("small")!.textContent = `${p.user}@${p.host}:${p.port}`;
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
          <label>主字体 <input name="fontFamily"></label>
          <label>CJK 字体 <input name="cjkFontFamily"></label>
          <div class="row">
            <label>字号 <input name="fontSize" type="number" min="8" max="32"></label>
            <label>字重
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
          <div class="row">
            <label class="grow">当前主题 <select name="theme"></select></label>
            <button type="button" class="btn new-theme">基于当前新建</button>
          </div>
          <div class="row">
            <label class="check grow"><input name="titlebarFollow" type="checkbox"> 标题栏颜色跟随主题</label>
            <label>标题栏颜色 <input name="titlebarColor" type="color"></label>
          </div>
          <div class="theme-editor" style="display:none"></div>
        </section>
        <section>
          <h4>窗口</h4>
          <label>背景不透明度 <input name="opacity" type="range" min="0.3" max="1" step="0.01">
            <span class="opacity-val"></span></label>
          <label class="check"><input name="blur" type="checkbox"> 毛玻璃虚化（透明时保持内容清晰）</label>
        </section>
        <section>
          <h4>行为</h4>
          <label>新建标签页（“+” 按钮）
            <select name="newTabMode">
              <option value="local">直接打开本地终端</option>
              <option value="dialog">弹出连接选择</option>
            </select>
          </label>
          <label class="check"><input name="autoReconnect" type="checkbox"> 断开后自动重连（默认开启）</label>
          <label class="check"><input name="copyOnSelect" type="checkbox"> 选中即复制</label>
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

  const themeSel = overlay.querySelector('[name="theme"]') as HTMLSelectElement;
  const refreshThemeOptions = () => {
    themeSel.textContent = "";
    const themes = allThemes(getSettings().customThemes);
    const groups: Array<[string, (t: ThemeDef) => boolean]> = [
      ["暗色", (t) => t.base === "dark" && BUILTIN_THEMES.includes(t)],
      ["亮色", (t) => t.base === "light" && BUILTIN_THEMES.includes(t)],
      ["自定义", (t) => !BUILTIN_THEMES.includes(t)],
    ];
    for (const [label, filter] of groups) {
      const matched = themes.filter(filter);
      if (matched.length === 0) continue;
      const og = document.createElement("optgroup");
      og.label = label;
      for (const t of matched) {
        const opt = document.createElement("option");
        opt.value = t.id;
        opt.textContent = t.name;
        og.appendChild(opt);
      }
      themeSel.appendChild(og);
    }
    themeSel.value = getSettings().theme;
  };
  refreshThemeOptions();

  // 标题栏颜色：默认跟随主题，取消勾选后可单独取色
  const followInput = input("titlebarFollow");
  const titlebarPicker = input("titlebarColor");
  const syncTitlebarUI = () => {
    followInput.checked = !getSettings().titlebarColor;
    titlebarPicker.value =
      getSettings().titlebarColor ??
      (resolveTheme(themeSel.value, getSettings().customThemes).colors.background ?? "#10151c").slice(0, 7);
    titlebarPicker.disabled = followInput.checked;
  };
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
      theme: themeSel.value,
      titlebarColor: input("titlebarFollow").checked ? null : input("titlebarColor").value,
      opacity: parseFloat(input("opacity").value),
      blur: input("blur").checked,
      newTabMode: (overlay.querySelector('[name="newTabMode"]') as HTMLSelectElement)
        .value as "local" | "dialog",
      autoReconnect: input("autoReconnect").checked,
      copyOnSelect: input("copyOnSelect").checked,
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
    const base = resolveTheme(themeSel.value, getSettings().customThemes);
    const draft: ThemeDef = {
      id: crypto.randomUUID(),
      name: `${base.name} · 自定义`,
      base: BUILTIN_THEMES.some((t) => t.id === base.id) ? (base.id as "dark" | "light") : base.base,
      colors: { ...base.colors },
    };
    renderThemeEditor(editor, draft, async () => {
      const custom = [...getSettings().customThemes, draft];
      await updateSettings({ customThemes: custom, theme: draft.id });
      refreshThemeOptions();
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
