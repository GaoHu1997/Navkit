import { checkbox, field, numberInput, passwordInput, textInput } from "./Dialog";
import type { RuntimeProfileBindings, RuntimeProfileDraft, RuntimeProfileSubmission, RuntimeProfileView } from "../core/runtime-profile";
import { assertProfileView, hasPlaintextRuntimeSecret, isRuntimeParameter, runtimeParameterKeys, tlsModeFromUi, tlsModeToUi, validateRuntimeDraft } from "../core/runtime-profile-editor";

export interface RuntimeProfileContainers { advanced?: HTMLElement; tls?: HTMLElement; ssh?: HTMLElement; http?: HTMLElement }
export interface RuntimeProfileEditorOptions {
  container: HTMLElement;
  containers?: RuntimeProfileContainers;
  status: HTMLElement;
  connectionId: string;
  editing: boolean;
  bindings?: RuntimeProfileBindings;
  rawParameters(): string;
  pickFile(): Promise<string | null>;
  /** PL-17 is offered only by the explicit H2 entry supported by the reference bridge. */
  httpAvailable?: boolean;
  databaseTransportAvailable?: boolean;
}
export interface RuntimeProfileEditor {
  readonly ready: Promise<void>;
  submission(includeUnchanged?: boolean): RuntimeProfileSubmission | undefined;
  usesHttp(): boolean;
  setBusy(busy: boolean): void;
  dispose(): void;
}

/** No configuration SQL and no secret-to-params conversion. Storage and transient tests are backend bindings. */
export function createRuntimeProfileEditor(options: RuntimeProfileEditorOptions): RuntimeProfileEditor {
  let previous: RuntimeProfileView | null = null;
  let loading = Boolean(options.editing && options.bindings), busy = false, disposed = false;
  let loadError: string | null = null;
  let edited = false, passwordEdited = false, passphraseEdited = false, bearerEdited = false;
  const databaseAvailable = options.databaseTransportAvailable !== false;
  const mode = document.createElement("select");
  const addOption = (select: HTMLSelectElement, value: string, label: string) => {
    const option = document.createElement("option"); option.value = value; option.textContent = label; select.append(option);
  };
  for (const [value, label] of [["", "不新增本机 TLS 设置（保留附加参数）"], ["disable", "disable（禁用）"], ["prefer", "prefer（允许明文回退）"], ["require", "require（不验证身份）"], ["verify-ca", "verify-ca（验证 CA）"], ["verify-full", "verify-full（验证原主机名）"]]) addOption(mode, value, label);
  mode.value = "";
  const ca = textInput("", 280); ca.placeholder = "CA PEM 路径；空时使用驱动信任库";
  const cert = textInput("", 280); cert.placeholder = "客户端证书 PEM 路径（与私钥成对）";
  const key = textInput("", 280); key.placeholder = "客户端私钥 PEM 路径（与证书成对）";
  const sshHost = textInput("", 280); sshHost.placeholder = "堡垒机地址（填写后启用 SSH）";
  const sshPort = numberInput(22, 100); sshPort.min = "1"; sshPort.max = "65535";
  const sshUser = textInput("", 280);
  const authentication = document.createElement("select"); addOption(authentication, "password", "密码"); addOption(authentication, "private_key", "OpenSSH 私钥"); authentication.value = "password";
  const sshPassword = passwordInput("", 280); sshPassword.autocomplete = "new-password"; sshPassword.placeholder = "新 SSH 密码；留空保留本机已保存秘密";
  const sshKey = textInput("", 280); sshKey.placeholder = "OpenSSH 私钥路径";
  const sshPassphrase = passwordInput("", 280); sshPassphrase.autocomplete = "new-password"; sshPassphrase.placeholder = "私钥口令；留空保留本机已保存秘密";
  const clearPassword = checkbox("清除本机 SSH 密码", false);
  const clearPasswordInput = clearPassword.querySelector<HTMLInputElement>("input")!;
  const clearPassphrase = checkbox("清除本机私钥口令", false);
  const clearPassphraseInput = clearPassphrase.querySelector<HTMLInputElement>("input")!;
  const knownHosts = textInput("", 280); knownHosts.placeholder = "必填：预先核实的 known_hosts 文件";
  const rebind = checkbox("更改端点后，将此本机通道配置明确重新绑定到新端点", false);
  const rebindInput = rebind.querySelector<HTMLInputElement>("input")!;
  rebindInput.setAttribute("aria-label", "将本机通道配置重新绑定到新端点");
  const enableHttp = checkbox("启用本机 HTTP SQL 通道（H2 参考服务）", false);
  const enableHttpInput = enableHttp.querySelector<HTMLInputElement>("input")!;
  enableHttpInput.setAttribute("aria-label", "启用 HTTP SQL 通道");
  const httpEndpoint = textInput("", 280); httpEndpoint.placeholder = "https://bridge.example:8443/（只填 origin）";
  const httpServerId = textInput("", 280); httpServerId.placeholder = "预先核实的服务 UUID";
  const httpTargetId = textInput("", 280); httpTargetId.placeholder = "预先核实的目标 UUID";
  const httpAlias = textInput("", 280); httpAlias.placeholder = "服务端预配 alias；不会自动发现目标";
  const httpTimeout = numberInput(10_000, 120); httpTimeout.min = "1000"; httpTimeout.max = "60000";
  const httpCa = textInput("", 280); httpCa.placeholder = "外层 HTTPS CA PEM；空时使用系统信任库";
  const httpCert = textInput("", 280); httpCert.placeholder = "外层 HTTPS 客户端证书 PEM（可选）";
  const httpKey = textInput("", 280); httpKey.placeholder = "外层 HTTPS 客户端私钥 PEM（与证书成对）";
  const httpBearer = passwordInput("", 280); httpBearer.autocomplete = "new-password"; httpBearer.placeholder = "新 Bearer；留空保留本机已保存令牌";
  const clearBearer = checkbox("清除本机 HTTP Bearer", false);
  const clearBearerInput = clearBearer.querySelector<HTMLInputElement>("input")!;
  clearBearerInput.setAttribute("aria-label", "清除本机 HTTP Bearer");
  const tlsSwitch = checkbox("启用 TLS（SSL/TLS 加密连接）", false);
  tlsSwitch.classList.add("conn-switch");
  const tlsSwitchInput = tlsSwitch.querySelector<HTMLInputElement>("input")!;
  const sshSwitch = checkbox("使用 SSH 隧道（经堡垒机转发）", false);
  sshSwitch.classList.add("conn-switch");
  const sshSwitchInput = sshSwitch.querySelector<HTMLInputElement>("input")!;
  const controls = [mode, ca, cert, key, sshHost, sshPort, sshUser, authentication, sshPassword, sshKey, sshPassphrase, clearPasswordInput, clearPassphraseInput, knownHosts, rebindInput, enableHttpInput, httpEndpoint, httpServerId, httpTargetId, httpAlias, httpTimeout, httpCa, httpCert, httpKey, httpBearer, clearBearerInput, tlsSwitchInput, sshSwitchInput];
  const databaseControls = new Set<HTMLElement>([mode, ca, cert, key, sshHost, sshPort, sshUser, authentication, sshPassword, sshKey, sshPassphrase, clearPasswordInput, clearPassphraseInput, knownHosts]);
  const httpControls = new Set<HTMLElement>([httpEndpoint, httpServerId, httpTargetId, httpAlias, httpTimeout, httpCa, httpCert, httpKey, httpBearer, clearBearerInput]);
  const tlsFields = document.createElement("div");
  const sshFields = document.createElement("div");
  let tlsEnabled = false, sshEnabled = false, tlsSavedMode = "";
  const tlsBackup = { ca: "", cert: "", key: "" };
  const applyChannelVisibility = () => {
    tlsFields.hidden = !tlsEnabled; sshFields.hidden = !sshEnabled;
    tlsSwitchInput.checked = tlsEnabled; sshSwitchInput.checked = sshEnabled;
  };
  const setTlsEnabled = (on: boolean) => {
    if (on) {
      if (mode.value === "" || mode.value === "disable") mode.value = tlsSavedMode && tlsSavedMode !== "disable" ? tlsSavedMode : "require";
      ca.value = tlsBackup.ca; cert.value = tlsBackup.cert; key.value = tlsBackup.key;
    } else {
      if (mode.value && mode.value !== "disable") tlsSavedMode = mode.value;
      tlsBackup.ca = ca.value; tlsBackup.cert = cert.value; tlsBackup.key = key.value;
      ca.value = ""; cert.value = ""; key.value = ""; mode.value = "disable";
    }
    tlsEnabled = on; applyChannelVisibility();
  };
  const setSshEnabled = (on: boolean) => { sshEnabled = on; applyChannelVisibility(); };
  const buttons: HTMLButtonElement[] = [], databaseButtons: HTMLButtonElement[] = [], httpButtons: HTMLButtonElement[] = [];
  const namedField = (name: string, control: HTMLElement): HTMLElement => {
    control.setAttribute("aria-label", name.replace(/[:：]$/, "")); control.setAttribute("aria-describedby", options.status.id);
    return field(name, control);
  };
  const pick = (input: HTMLInputElement, label: string, group: HTMLButtonElement[]): HTMLElement => {
    const row = document.createElement("div"); row.style.cssText = "display:flex;gap:8px;align-items:center";
    input.setAttribute("aria-label", label); input.setAttribute("aria-describedby", options.status.id);
    const button = document.createElement("button"); button.type = "button"; button.className = "btn"; button.textContent = "浏览…"; button.setAttribute("aria-label", `浏览${label}`); buttons.push(button); group.push(button);
    button.addEventListener("click", async () => {
      if (button.disabled || loading || busy || disposed) return;
      try { const path = await options.pickFile(); if (typeof path === "string" && !disposed) { input.value = path; edited = true; } }
      catch { if (!disposed) options.status.textContent = "无法读取本机证书路径；输入已保留。"; }
    });
    row.append(input, button); return row;
  };
  const makeHint = (short: string, long: string): HTMLElement => {
    const wrap = document.createElement("div"); wrap.className = "conn-hint";
    const line = document.createElement("div"); line.className = "conn-hint-line"; line.textContent = "ⓘ " + short;
    const detail = document.createElement("details"); detail.className = "conn-hint-detail";
    const summary = document.createElement("summary"); summary.textContent = "查看说明";
    const body = document.createElement("p"); body.textContent = long;
    detail.append(summary, body); wrap.append(line, detail); return wrap;
  };
  const tlsHint = makeHint("证书路径仅本机加密保存，不上传数据源配置", "本机 TLS 设置与路径独立加密保存，不上传数据源配置；附加参数原文仍保留。材料缺验证模式、损坏或不配对均拒绝连接。这里只是配置，不代表已经验证握手。");
  const sshHint = makeHint("SSH 秘密只存本机加密配置，绝不写入参数", "SSH 秘密只送到本机加密配置服务，绝不拼入 params。known_hosts 必填；支持哈希主机名、显式多密钥及非默认端口，未知/变更密钥拒绝，不自动 TOFU。当前 SSH + TLS 原主机名组合未实现，SSH 必须明确选择 TLS disable。");
  const tlsSection = document.createElement("div"); tlsSection.hidden = !databaseAvailable;
  tlsSection.append(tlsSwitch, tlsFields);
  tlsFields.append(namedField("SSL/TLS 模式:", mode), field("CA 证书:", pick(ca, "CA 证书", databaseButtons)), field("客户端证书:", pick(cert, "客户端证书", databaseButtons)), field("客户端私钥:", pick(key, "客户端私钥", databaseButtons)), tlsHint);
  const sshSection = document.createElement("div"); sshSection.hidden = !databaseAvailable;
  sshSection.append(sshSwitch, sshFields);
  sshFields.append(namedField("SSH 主机:", sshHost), namedField("SSH 端口:", sshPort), namedField("SSH 用户名:", sshUser), namedField("SSH 认证方式:", authentication), namedField("SSH 密码:", sshPassword), clearPassword,
    namedField("SSH 私钥:", sshKey), namedField("私钥口令:", sshPassphrase), clearPassphrase, namedField("known_hosts:", knownHosts), sshHint);
  const httpSection = document.createElement("div"); httpSection.hidden = !options.httpAvailable;
  const httpFields = document.createElement("div"); httpFields.id = `${options.status.id || options.connectionId}-http-fields`; httpFields.hidden = true;
  enableHttpInput.setAttribute("aria-controls", httpFields.id); enableHttpInput.setAttribute("aria-expanded", "false");
  const httpHint = makeHint("协议 navkit-sql-tunnel/1；H2 参考桥接须预配 alias 与服务/目标 ID", "协议 navkit-sql-tunnel/1。当前参考桥接服务只支持 H2 2.3.232 REGULAR；请先在服务端预配 alias，并单独核实服务 ID 与目标 ID。数据库密码保留在桥接服务，测试时不会上传常规页数据库密码。外层 HTTPS 固定 VerifyFull；它与服务至数据库的下游 TLS 证据分别核验。");
  httpFields.append(namedField("HTTP HTTPS origin:", httpEndpoint), namedField("HTTP 服务 ID:", httpServerId), namedField("HTTP 目标 ID:", httpTargetId), namedField("HTTP 目标 alias:", httpAlias), namedField("HTTP 连接期限（毫秒）:", httpTimeout),
    field("HTTP 外层 CA 证书:", pick(httpCa, "HTTP 外层 CA 证书", httpButtons)), field("HTTP 外层客户端证书:", pick(httpCert, "HTTP 外层客户端证书", httpButtons)), field("HTTP 外层客户端私钥:", pick(httpKey, "HTTP 外层客户端私钥", httpButtons)), namedField("HTTP Bearer:", httpBearer), clearBearer, httpHint);
  httpSection.append(enableHttp, httpFields);
  (options.containers?.tls ?? options.container).append(tlsSection);
  (options.containers?.ssh ?? options.container).append(sshSection);
  if (options.httpAvailable) (options.containers?.http ?? options.container).append(httpSection);
  (options.containers?.advanced ?? options.container).append(rebind);
  for (const control of controls) {
    control.setAttribute("aria-describedby", options.status.id);
    control.addEventListener("input", () => { edited = true; }); control.addEventListener("change", () => { edited = true; });
  }
  clearPasswordInput.setAttribute("aria-label", "清除本机 SSH 密码"); clearPassphraseInput.setAttribute("aria-label", "清除本机私钥口令");
  sshPassword.addEventListener("input", () => { passwordEdited = true; }); sshPassphrase.addEventListener("input", () => { passphraseEdited = true; }); httpBearer.addEventListener("input", () => { bearerEdited = true; });
  const updateBusy = () => {
    for (const control of controls) control.disabled = loading || busy || disposed || databaseControls.has(control) && !databaseAvailable || (control === enableHttpInput || httpControls.has(control)) && !options.httpAvailable || httpControls.has(control) && !enableHttpInput.checked;
    for (const button of buttons) button.disabled = loading || busy || disposed || databaseButtons.includes(button) && !databaseAvailable || httpButtons.includes(button) && (!options.httpAvailable || !enableHttpInput.checked);
    tlsSwitchInput.disabled = loading || busy || disposed || !databaseAvailable;
    sshSwitchInput.disabled = loading || busy || disposed || !databaseAvailable;
    httpFields.hidden = !enableHttpInput.checked; enableHttpInput.setAttribute("aria-expanded", String(enableHttpInput.checked));
    tlsFields.hidden = !tlsEnabled; sshFields.hidden = !sshEnabled;
  };
  tlsSwitchInput.addEventListener("change", () => { edited = true; setTlsEnabled(tlsSwitchInput.checked); updateBusy(); });
  sshSwitchInput.addEventListener("change", () => { edited = true; setSshEnabled(sshSwitchInput.checked); updateBusy(); });
  enableHttpInput.addEventListener("change", updateBusy);
  applyChannelVisibility();
  updateBusy();
  const ready = loading ? options.bindings!.load(options.connectionId).then(view => {
    if (disposed) return;
    assertProfileView(view, options.connectionId); previous = view;
    if (view && !edited) {
      mode.value = tlsModeToUi(view.tls?.mode); ca.value = view.tls?.ca ?? ""; cert.value = view.tls?.cert ?? ""; key.value = view.tls?.key ?? "";
      sshHost.value = view.ssh?.host ?? ""; sshPort.value = String(view.ssh?.port ?? 22); sshUser.value = view.ssh?.user ?? ""; authentication.value = view.ssh?.authentication ?? "password"; sshKey.value = view.ssh?.key_file ?? ""; knownHosts.value = view.ssh?.known_hosts ?? "";
      sshPassword.placeholder = view.ssh_password_present ? "本机已保存 SSH 密码；留空保持不变" : "请输入 SSH 密码";
      sshPassphrase.placeholder = view.ssh_key_passphrase_present ? "本机已保存私钥口令；留空保持不变" : "输入私钥口令（可选）";
      enableHttpInput.checked = Boolean(view.http); httpEndpoint.value = view.http?.endpoint ?? ""; httpServerId.value = view.http?.expected_server_id ?? ""; httpTargetId.value = view.http?.expected_target_id ?? ""; httpAlias.value = view.http?.target_alias ?? ""; httpTimeout.value = String(view.http?.connect_timeout_ms ?? 10_000);
      httpCa.value = view.http?.tls.ca ?? ""; httpCert.value = view.http?.tls.cert ?? ""; httpKey.value = view.http?.tls.key ?? "";
      httpBearer.placeholder = view.http_bearer_present ? "本机已保存 HTTP Bearer；留空保持不变" : "请输入桥接服务预配 Bearer";
      if (view.http && !options.httpAvailable || !databaseAvailable && (view.ssh || view.tls)) throw new Error("当前数据源不支持已保存的本机通道，不能覆盖未知配置。");
      tlsEnabled = Boolean(mode.value) && mode.value !== "disable";
      sshEnabled = Boolean(sshHost.value.trim());
      applyChannelVisibility();
    }
  }).catch(() => {
    loadError = "本机通道配置读取失败、协议不支持或回执身份不符；不能覆盖未知配置，输入已保留。";
    if (!disposed) options.status.textContent = loadError;
  }).finally(() => { loading = false; updateBusy(); }) : Promise.resolve();
  return {
    ready,
    usesHttp() { return Boolean(enableHttpInput.checked); },
    submission(includeUnchanged = false) {
      if (loading) throw new Error("正在读取本机通道配置；未发送测试或保存。");
      if (loadError) throw new Error(loadError);
      const raw = options.rawParameters();
      const keys = raw.trim() ? runtimeParameterKeys(raw) : [];
      if (hasPlaintextRuntimeSecret(keys)) throw new Error("附加参数含明文秘密；必须先显式加密迁移，原文不会被静默删除。");
      if (keys.includes("runtime_profile_id") && !previous) throw new Error("连接引用的本机通道配置尚未读取或不存在；不能按普通连接回退，原参数与输入已保留。");
      if ((previous || edited) && keys.some(key => isRuntimeParameter(key) && !(key === "runtime_profile_id" && previous))) throw new Error("本机通道设置不能与附加参数的 TLS/SSH/HTTP 设置叠加；请先显式迁移旧设置，原文与输入已保留。");
      if (!edited && (!previous || !includeUnchanged)) return undefined;
      if (!options.bindings) throw new Error("本机加密通道配置服务尚未接线；不会把通道秘密写入 params，输入已保留。");
      const tlsMode = tlsModeFromUi(mode.value);
      if (!tlsMode && (ca.value || cert.value || key.value)) throw new Error("证书材料需要明确验证模式；不会默认 prefer 降级。");
      if (!["password", "private_key"].includes(authentication.value)) throw new Error("SSH 认证方式无效；不会切换认证重试。");
      const draft: RuntimeProfileDraft = {
        ssh: sshEnabled && sshHost.value.trim() ? { host: sshHost.value.trim(), port: Number(sshPort.value), user: sshUser.value.trim(), authentication: authentication.value as "password" | "private_key", key_file: sshKey.value.trim() || null, known_hosts: knownHosts.value.trim() } : null,
        tls: tlsMode ? { mode: tlsMode, ca: ca.value.trim() || null, cert: cert.value.trim() || null, key: key.value.trim() || null } : null,
        ssh_password: clearPasswordInput.checked || passwordEdited && !sshPassword.value ? { action: "clear" } : passwordEdited ? { action: "set", value: sshPassword.value } : { action: "keep" },
        ssh_key_passphrase: clearPassphraseInput.checked || passphraseEdited && !sshPassphrase.value ? { action: "clear" } : passphraseEdited ? { action: "set", value: sshPassphrase.value } : { action: "keep" },
      };
      if (options.httpAvailable || previous?.http !== undefined) {
        draft.http = enableHttpInput.checked ? { endpoint: httpEndpoint.value, protocol: "navkit-sql-tunnel/1", expected_server_id: httpServerId.value, expected_target_id: httpTargetId.value, target_alias: httpAlias.value, tls: { mode: "verify_full", ca: httpCa.value || null, cert: httpCert.value || null, key: httpKey.value || null }, connect_timeout_ms: Number(httpTimeout.value) } : null;
        draft.http_bearer = clearBearerInput.checked || bearerEdited && !httpBearer.value ? { action: "clear" } : bearerEdited ? { action: "set", value: httpBearer.value } : { action: "keep" };
      }
      validateRuntimeDraft(draft, previous, rebindInput.checked);
      return { draft, expected_revision: previous?.revision ?? null, rebind_endpoint: rebindInput.checked };
    },
    setBusy(value) { busy = value; updateBusy(); },
    dispose() { disposed = true; sshPassword.value = ""; sshPassphrase.value = ""; httpBearer.value = ""; updateBusy(); },
  };
}