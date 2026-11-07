/** 选项对话框中的「数据源同步」tab：WebDAV 连接信息 + 同步频率 + 请求头，随对话框「确定」保存。 */

import { select } from "../ui/Dialog";
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
  const prefix = `datasource-sync-${crypto.randomUUID()}`;

  const el = document.createElement("div");
  el.className = "datasource-sync-tab";
  const form = document.createElement("div");
  form.className = "datasource-sync-form";
  let fieldSeq = 0;
  const row = <T extends HTMLElement>(name: string, control: T): T => {
    control.id = `${prefix}-${fieldSeq++}`;
    const wrap = document.createElement("div"); wrap.className = "field";
    const label = document.createElement("label"); label.className = "field-label";
    label.htmlFor = control.id; label.textContent = name;
    const content = document.createElement("div"); content.className = "field-control";
    content.append(control); wrap.append(label, content); form.append(wrap);
    return control;
  };
  const input = (type = "text") => {
    const control = document.createElement("input");
    control.type = type; control.autocomplete = "off";
    return control;
  };
  const url = row("地址", input()); url.inputMode = "url"; url.required = true;
  url.placeholder = "https://dav.example.com/remote.php/dav/files/user";
  const user = row("用户名", input());
  const password = row("密码", input("password"));
  const passphrase = row("口令", input("password"));
  const headers = document.createElement("textarea");
  headers.rows = 3; headers.spellcheck = false; headers.autocomplete = "off";
  row("请求头", headers);
  // 同步行为置于连接字段之后。
  const interval = row("频率", select(INTERVAL_OPTIONS, "-1")) as HTMLSelectElement;
  el.append(form);

  // 秘密不回显：留空保留已保存值；请求头同样留空保留。秘密始终本机加密保存。
  const request = (): DatasourceSyncInput => ({
    url: url.value.trim(), user: user.value,
    auto_every: Number(interval.value), remember_password: true,
    ...(password.value ? { password: password.value } : {}),
    ...(passphrase.value ? { passphrase: passphrase.value } : {}),
    ...(headers.value.trim() ? { headers: headers.value } : {}),
  });
  const setConfig = (config: DatasourceSyncConfig) => {
    saved = config;
    url.value = config.url; user.value = config.user;
    interval.value = INTERVAL_OPTIONS.some(o => o.value === String(config.auto_every)) ? String(config.auto_every) : "-1";
    password.value = passphrase.value = headers.value = "";
    password.placeholder = config.password_set ? "已设置，留空保留" : "未设置";
    passphrase.placeholder = config.passphrase_set ? "已设置，留空保留" : "未设置";
    headers.placeholder = config.headers_set ? "已设置，留空保留" : "每行一条，如 Authorization: Bearer …";
    dirty = false;
  };
  for (const control of [url, user, password, passphrase, headers]) {
    control.addEventListener("input", () => { dirty = true; });
  }
  interval.addEventListener("change", () => { dirty = true; });

  const saveIfNeeded = async () => {
    if (!saved || dirty) {
      if (!url.value.trim()) { url.setAttribute("aria-invalid", "true"); throw new Error("请填写 WebDAV 基础目录地址"); }
      setConfig(await sync.save(request()));
    }
  };
  void sync.getConfig()
    .then((config) => { if (!disposed) setConfig(config); })
    .catch(() => undefined); // 读取失败保持空表单；保存时由对话框统一提示错误。

  return {
    el,
    saveIfNeeded,
    dispose: () => {
      if (disposed) return;
      disposed = true;
      password.value = passphrase.value = headers.value = "";
      void sync.cancel().catch(() => undefined).finally(release);
    },
  };
}
