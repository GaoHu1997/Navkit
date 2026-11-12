/** 选项对话框中的「数据源同步」tab：WebDAV 连接信息 + 同步频率 + 请求头，随对话框「确定」保存。 */

import { field, select } from "../ui/Dialog";
import { asIpcError, formatError, type IpcError } from "../core/execution";
import type { DatasourceSync, DatasourceSyncConfig, DatasourceSyncInput } from "../core/datasourceSync";

export interface DatasourceSyncTab {
  el: HTMLElement;
  /** 保存未落库的修改（对话框「确定」前调用）。 */
  saveIfNeeded(): Promise<void>;
  /** 对话框关闭时调用：清理秘密输入并恢复自动同步计时。 */
  dispose(): void;
}

const INTERVAL_OPTIONS = [
  { value: "-1", label: "暂停同步" },
  { value: "3", label: "每 3 分钟" },
  { value: "15", label: "每 15 分钟" },
  { value: "30", label: "每 30 分钟" },
  { value: "60", label: "每 60 分钟" },
];

export function createDatasourceSyncTab(sync: DatasourceSync): DatasourceSyncTab {
  const release = sync.pause();
  let saved: DatasourceSyncConfig | null = null;
  let dirty = false, disposed = false;
  let initializationError: IpcError | null = null;
  const prefix = `datasource-sync-${crypto.randomUUID()}`;

  const el = document.createElement("div");
  el.className = "datasource-sync-tab";
  const form = document.createElement("div");
  form.className = "datasource-sync-form";
  const section = (title: string) => {
    const group = document.createElement("section");
    group.className = "settings-section";
    const heading = document.createElement("h3");
    heading.className = "settings-section-title";
    heading.textContent = title;
    group.appendChild(heading);
    form.appendChild(group);
    return group;
  };
  const connection = section("WebDAV 连接");
  const security = section("安全与请求");
  const behavior = section("同步行为");
  let fieldSeq = 0;
  const row = <T extends HTMLElement>(group: HTMLElement, key: string, name: string, help: string, control: T): T => {
    control.id = `${prefix}-${fieldSeq++}`;
    control.dataset.syncField = key;
    control.dataset.settingsField = `sync_${key}`;
    const wrap = field("", control);
    wrap.classList.add("settings-field");
    const copy = wrap.querySelector<HTMLElement>(".field-label")!;
    copy.classList.add("settings-field-copy");
    const label = document.createElement("label"); label.className = "settings-field-label";
    label.htmlFor = control.id; label.textContent = name;
    const hint = document.createElement("p"); hint.className = "settings-field-help";
    hint.id = `${control.id}-help`; hint.textContent = help;
    control.setAttribute("aria-describedby", hint.id);
    copy.append(label, hint);
    wrap.querySelector<HTMLElement>(".field-control")!.classList.add("settings-field-value");
    group.appendChild(wrap);
    return control;
  };
  const input = (type = "text") => {
    const control = document.createElement("input");
    control.type = type; control.autocomplete = "off";
    return control;
  };
  const url = row(connection, "url", "地址", "WebDAV 基础目录地址。", input()); url.inputMode = "url"; url.required = true;
  url.placeholder = "https://dav.example.com/remote.php/dav/files/user";
  const user = row(connection, "user", "用户名", "连接 WebDAV 使用的账号。", input());
  const password = row(connection, "password", "密码", "不回显；留空保留本机已保存的密码。", input("password"));
  const passphrase = row(security, "passphrase", "口令", "不回显；留空保留本机已保存的口令。", input("password"));
  const headers = document.createElement("textarea");
  headers.rows = 3; headers.spellcheck = false; headers.autocomplete = "off";
  row(security, "headers", "请求头", "每行一条；留空保留已保存内容。", headers);
  // 同步行为置于连接字段之后。
  const interval = row(behavior, "auto_every", "频率", "自动同步使用的时间间隔。", select(INTERVAL_OPTIONS, "-1")) as HTMLSelectElement;
  el.appendChild(form);
  const controls = [url, user, password, passphrase, headers, interval];
  controls.forEach(control => { control.disabled = true; });
  form.setAttribute("aria-busy", "true");
  const loadError = document.createElement("p"); loadError.id = `${prefix}-load-error`;
  loadError.setAttribute("role", "alert"); loadError.hidden = true; el.appendChild(loadError);

  // 密码与口令不回显、留空保留；请求头回显，未修改时不重提交，避免随地址变更带到新端点。
  const request = (): DatasourceSyncInput => ({
    url: url.value.trim(), user: user.value,
    auto_every: Number(interval.value), remember_password: true,
    ...(password.value ? { password: password.value } : {}),
    ...(passphrase.value ? { passphrase: passphrase.value } : {}),
    ...(headers.value.trim() && headers.value !== (saved?.headers ?? "") ? { headers: headers.value } : {}),
  });
  const setConfig = (config: DatasourceSyncConfig) => {
    saved = config;
    url.value = config.url; user.value = config.user;
    interval.value = INTERVAL_OPTIONS.some(o => o.value === String(config.auto_every)) ? String(config.auto_every) : "-1";
    password.value = passphrase.value = "";
    headers.value = config.headers ?? "";
    password.placeholder = config.password_set ? "已设置，留空保留" : "未设置";
    passphrase.placeholder = config.passphrase_set ? "已设置，留空保留" : "未设置";
    headers.placeholder = "每行一条，如 Authorization: Bearer …";
    dirty = false;
  };
  for (const control of [url, user, password, passphrase, headers]) {
    control.addEventListener("input", () => { dirty = true; });
  }
  interval.addEventListener("change", () => { dirty = true; });

  const saveIfNeeded = async () => {
    await initialized;
    if (disposed) throw new Error("数据源同步设置已关闭，没有保存配置");
    if (initializationError) throw initializationError;
    if (!saved) throw new Error("数据源同步配置尚未成功读取，没有保存配置");
    if (dirty) {
      if (!url.value.trim()) { url.setAttribute("aria-invalid", "true"); throw new Error("请填写 WebDAV 基础目录地址"); }
      setConfig(await sync.save(request()));
    }
  };
  const initialized = Promise.resolve().then(() => sync.getConfig())
    .then((config) => { if (!disposed) { setConfig(config); controls.forEach(control => { control.disabled = false; }); } })
    .catch((error) => {
      initializationError = asIpcError(error, "datasource_sync_get_config");
      console.error("数据源同步配置读取失败，禁止保存未知基线", initializationError);
      if (!disposed) {
        loadError.textContent = `配置读取失败，不能保存；请关闭后重新打开设置重试。
${formatError(initializationError)}`;
        loadError.hidden = false;
        controls.forEach(control => control.setAttribute("aria-describedby", [control.getAttribute("aria-describedby"), loadError.id].filter(Boolean).join(" ")));
      }
    }).finally(() => { if (!disposed) form.setAttribute("aria-busy", "false"); });

  return {
    el,
    saveIfNeeded,
    dispose: () => {
      if (disposed) return;
      disposed = true;
      password.value = passphrase.value = headers.value = "";
      void sync.cancel().catch(error => { console.error("关闭数据源同步设置时取消失败", error); }).finally(release);
    },
  };
}
