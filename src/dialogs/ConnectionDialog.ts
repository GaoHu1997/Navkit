import { open as openFile } from "@tauri-apps/plugin-dialog";
import { driverApi, encodeDriverParams, parseDriverParams, encodeVersionParams, parseVersionSelection, type DriverDefinition } from "../core/api";
import { currentDriverStatuses, ensureDriverStatuses } from "../core/driverStatus";
import { kindLabel, kindMeta, kindIcon } from "../core/dbcatalog";
import type { ConnectionConfig, ConnectionRequest, DbKind } from "../core/types";
import { checkbox, Dialog, field, numberInput, passwordInput, textInput } from "../ui/Dialog";
import type { RuntimeProfileBindings, RuntimeProfileSubmission } from "../core/runtime-profile";
import { iconSvgByName } from "../ui/icons";
import { createRuntimeProfileEditor } from "../ui/RuntimeProfileEditor";

export function newConnectionId(): string {
  return `c_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 8)}`;
}
export interface ConnectionDialogResult {
  config: ConnectionConfig;
  /** undefined preserves ciphertext; empty string deliberately removes it. */
  password?: string;
  /** Persist with the backend local profile service; never merge this draft into params. */
  runtimeProfile?: RuntimeProfileSubmission;
}
export function connectionField(label: string, control: HTMLElement): HTMLElement {
  control.setAttribute("aria-label", label.replace(/[:：]$/, ""));
  return field(label, control);
}
/** Local accessibility enhancement; does not modify the shared dialog implementation. */
export function prepareConnectionDialog(dlg: Dialog): void {
  dlg.el.setAttribute("role", "dialog"); dlg.el.setAttribute("aria-modal", "true");
  dlg.el.style.maxWidth = "calc(100vw - 24px)"; dlg.el.style.maxHeight = "calc(100vh - 24px)";
  const title = dlg.el.querySelector<HTMLElement>(".dialog-title-text");
  if (title) { title.id ||= newConnectionId(); dlg.el.setAttribute("aria-labelledby", title.id); }
  const close = dlg.el.querySelector<HTMLElement>(".dialog-title-close");
  if (close) {
    close.tabIndex = 0; close.setAttribute("role", "button"); close.setAttribute("aria-label", "关闭");
    close.addEventListener("keydown", (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); dlg.close(); } });
  }
  dlg.el.addEventListener("keydown", (e) => {
    if (e.key === "Escape") { e.preventDefault(); dlg.close(); }
    if (e.key !== "Tab") return;
    const elements = [...dlg.el.querySelectorAll<HTMLElement>("button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex='0']")]
      .filter((element) => element.getClientRects().length > 0);
    const first = elements[0], last = elements[elements.length - 1];
    if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last?.focus(); }
    else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first?.focus(); }
  });
}

/** Compatible groups argument intentionally unused: grouping belongs to the connection tree. */
export function openConnectionDialog(
  existing: ConnectionConfig | null, _groups: string[],
  onSaved: (result: ConnectionDialogResult) => void | Promise<void>,
  defaultKind: DbKind = "mysql", selectedDriver?: DriverDefinition, runtimeProfiles?: RuntimeProfileBindings,
): Dialog {
  const previousFocus = document.activeElement as HTMLElement | null;
  // 空 ID 表示复制/新建；一次表单会话使用稳定 ID，保存失败重试不会创建多个副本。
  const connectionId = existing?.id || newConnectionId();
  const editing = Boolean(existing?.id);
  let closed = false;
  const kind = selectedDriver ? "odbc" : existing?.kind ?? defaultKind;
  const meta = kindMeta(kind);
  let driver = selectedDriver;
  const savedParams = parseDriverParams(existing?.params ?? "");
  const driverId = selectedDriver?.id ?? savedParams?.driver_id;
  const fileKind = ["sqlite", "access", "duckdb"].includes(kind);
  const isOdbc = meta?.protocol === "odbc" || kind === "odbc";
  const defaultUser = ["bigquery", "spanner", "salesforce"].includes(kind) ? "token" : kind === "h2" ? "sa" : meta?.protocol === "postgres" ? "postgres" : meta?.protocol === "mysql" ? "root" : "";
  const name = textInput(existing?.name ?? driver?.name ?? kindLabel(kind), 280);
  name.required = true; name.autocomplete = "off";
  const host = textInput(existing?.host ?? (kind === "bigquery" ? "bigquery.googleapis.com" : kind === "spanner" ? "spanner.googleapis.com" : kind === "salesforce" ? "" : "localhost"), 280);
  if (kind === "salesforce") host.placeholder = "your-org.my.salesforce.com";
  const port = numberInput(existing?.port ?? meta?.port ?? 0, 100); port.min = "0"; port.max = "65535";
  const user = textInput(existing?.user ?? defaultUser, 280); user.autocomplete = "username";
  const password = passwordInput("", 280); password.autocomplete = "new-password";
  password.placeholder = existing?.password ? "已保存密码；留空保持不变" : ["bigquery", "spanner", "salesforce"].includes(kind) ? "预生成 OAuth 访问令牌" : "输入连接密码";
  let passwordChanged = false;
  password.addEventListener("input", () => { passwordChanged = true; });
  const savePassword = checkbox("保存密码（加密保存在本地）", existing ? Boolean(existing.password) : true);
  const saveCheckbox = savePassword.querySelector<HTMLInputElement>("input")!;
  // 不把空密码当作非法认证；常见账号连接须由用户明确选择，避免漏填后直接发起请求。
  const confirmEmptyPassword = !driverId && ["mysql", "postgres", "mssql"].includes(meta?.protocol ?? "");
  const emptyPassword = checkbox("使用空密码 / 无密码认证", false);
  const emptyPasswordCheckbox = emptyPassword.querySelector<HTMLInputElement>("input")!;
  emptyPasswordCheckbox.setAttribute("aria-label", "使用空密码 / 无密码认证");
  const updateEmptyPassword = () => {
    emptyPassword.hidden = !confirmEmptyPassword || password.value !== "" || Boolean(existing?.password) && !passwordChanged;
    if (emptyPassword.hidden) emptyPasswordCheckbox.checked = false;
  };
  updateEmptyPassword();
  password.addEventListener("input", updateEmptyPassword);
  const database = textInput(existing?.database ?? "", 280);
  database.placeholder = kind === "bigquery" ? "project 或 project/dataset" : kind === "spanner" ? "project/instance/database" : kind === "salesforce" ? "API 版本（默认 v59.0）" : kind === "hbase" ? "namespace（默认 default）" : kind === "odbc" && !driverId ? "ODBC DSN 名称" : "数据库名称";
  const file = textInput(existing?.file ?? "", 280); file.placeholder = kind === "h2" ? "可选：文件库路径；留空使用内存库或 jdbc_url" : "数据库文件绝对路径";
  // 已保存的托管驱动版本选择（@navkit-driver-version: 前缀）：版本回填下拉，附加参数回填文本框。
  const savedVersion = parseVersionSelection(existing?.params ?? "");
  const params = document.createElement("textarea"); params.rows = 5;
  params.style.cssText = "width:100%;resize:vertical";
  params.value = driverId ? savedParams?.template ?? "" : savedVersion ? savedVersion.extra : existing?.params ?? "";
  params.placeholder = driverId ? "留空使用驱动默认模板" : isOdbc ? "DRIVER={驱动名称};SERVER=...;UID=...（密码填常规页）" : "例如 sslmode=require";
  const status = document.createElement("div"); status.id = newConnectionId(); status.setAttribute("role", "status"); status.setAttribute("aria-live", "polite");
  for (const control of [name, host, port, user, password, emptyPasswordCheckbox, file, params]) control.setAttribute("aria-describedby", status.id);
  status.style.cssText = "min-height:24px;padding-top:10px";
  const summary = document.createElement("div");
  summary.style.cssText = "display:flex;align-items:center;gap:12px;padding:4px 0 16px;border-bottom:1px solid var(--line);margin-bottom:16px";
  const icon = document.createElement("span"); icon.setAttribute("aria-hidden", "true"); icon.innerHTML = iconSvgByName(kindIcon(kind));
  const description = document.createElement("div");
  const updateSummary = () => {
    description.textContent = driver ? `${driver.name} · ${driver.protocol === "jdbc" ? "自定义 JDBC 驱动" : "自定义 ODBC 驱动"}` : driverId ? "正在读取自定义驱动…" : `${kindLabel(kind)} · ${meta?.protocol.toUpperCase() ?? "ODBC"}`;
    params.placeholder = driver?.template ?? params.placeholder;
  };
  updateSummary(); summary.append(icon, description);
  const general = document.createElement("div"); general.append(summary, connectionField("连接名称:", name));
  if (!fileKind) {
    general.append(connectionField("主机 / 地址:", host), connectionField("端口:", port), connectionField(kind === "dynamodb" ? "Access Key ID:" : "用户名:", user), connectionField(kind === "dynamodb" ? "Secret Access Key:" : ["bigquery", "spanner", "salesforce"].includes(kind) ? "OAuth 访问令牌:" : "密码:", password));
    general.append(field("", savePassword));
    if (confirmEmptyPassword) general.append(field("", emptyPassword));
    // One input, one panel owner. Never move the database input between tabs.
    general.append(connectionField(kind === "bigquery" ? "项目 / 数据集:" : kind === "spanner" ? "项目 / 实例 / 库:" : kind === "salesforce" ? "REST API 版本:" : kind === "hbase" ? "Namespace:" : kind === "odbc" && !driverId ? "数据库 / DSN:" : "数据库:", database));
  }
  if (fileKind || driverId || kind === "h2") {
    const pathControl = document.createElement("div"); pathControl.style.cssText = "display:flex;gap:8px;align-items:center";
    const browse = document.createElement("button"); browse.className = "btn"; browse.textContent = "浏览…";
    browse.addEventListener("click", async () => {
      try { const path = await openFile({ multiple: false, directory: false, title: "选择数据库文件" }); if (!closed && typeof path === "string") { file.value = path; file.dispatchEvent(new Event("input", { bubbles: true })); } }
      catch (error) { if (!closed) status.textContent = String(error); }
    });
    pathControl.append(file, browse); file.setAttribute("aria-label", "数据库文件"); general.append(connectionField("数据库文件:", pathControl));
  }
  const advanced = document.createElement("div");
  const hint = document.createElement("div"); hint.className = "conn-hint";
  const hintLine = document.createElement("div"); hintLine.className = "conn-hint-line"; hintLine.textContent = "ⓘ 驱动、附加参数与兼容性说明";
  const hintDetail = document.createElement("details"); hintDetail.className = "conn-hint-detail";
  const hintSummary = document.createElement("summary"); hintSummary.textContent = "查看说明";
  const hintBody = document.createElement("p");
  hintDetail.append(hintSummary, hintBody); hint.append(hintLine, hintDetail);
  const hintText = (text: string) => { hintBody.textContent = text; };
  hintText(driverId
    ? driver?.protocol === "jdbc"
      ? "自定义 JDBC 驱动通过本地 .jar 与 Java 11+ 连接。留空沿用驱动 URL 模板；可用 {{host}}、{{port}}、{{database}}、{{file}}、{{user}}、{{password}}。不要在模板里保存明文密码。"
      : "自定义驱动通过原生 ODBC 连接。留空沿用驱动模板；可用 {{host}}、{{port}}、{{database}}、{{file}}、{{user}}、{{password}}、{{driver}}、{{dsn}}。替换值自动转义，不要额外添加花括号。"
    : isOdbc ? "可填写完整 ODBC 连接串；不要在连接串保存明文 PWD/Password，密码应填写在常规页并加密保存。留空时数据库字段作为 DSN。" : "填写数据库适配器支持的附加参数。未实现的 SSL / SSH / HTTP 通道不在此展示。");
  if (meta?.protocol === "native") {
    hintText(`${meta.note} 附加参数支持 JSON 对象或 key=value;key=value；凭据请填常规页，不要放在未加密参数中。`);
    params.placeholder = kind === "h2" ? '{"jdbc_url":"jdbc:h2:mem:navkit"}' : kind === "dynamodb" ? '{"region":"us-east-1"}' : '{"ssl":false}';
  }
  if (kind === "bigquery" && !driverId) {
    hintText("使用官方完整 BigQuery JDBC 与 Java 11+；用户名为 token，令牌填常规页并加密保存。云端固定 bigquery.googleapis.com:443 和正常 HTTPS，项目格式为 project 或 project/dataset。令牌过期后需更新；ADC、刷新令牌及其它认证请显式配置自定义驱动。");
    params.placeholder = "云端留空使用正常 HTTPS";
  }
  if (kind === "spanner" && !driverId) {
    hintText("使用完整 Google Spanner JDBC 与 Java 11+；数据库填 project/instance/database。云端固定 spanner.googleapis.com:443 和正常 TLS，用户名 token，预生成令牌填常规页。令牌过期后需更新。仅连接本地模拟器时，填写字面回环地址和端口，清空用户名及令牌，再设置 ssl=false；令牌不会发送到明文端点。服务账户、ADC、刷新令牌和其它端点请配置自定义驱动。");
    params.placeholder = "云端留空；无凭据本地模拟器显式填 ssl=false";
  }
  if (kind === "salesforce" && !driverId) {
    hintText("内置 Salesforce REST，不需要另装驱动。主机填写组织 instance 域名（如 your-org.my.salesforce.com），密码填预生成 OAuth token；API 版本默认 v59.0。支持 SOQL 和 JSON create/update/delete；云端 HTTPS 严格验证证书，令牌过期需手动更新。自定义 JDBC/ODBC 绑定仍可使用。");
    params.placeholder = "云端留空；可设 api_version=v59.0";
  }
  if (kind === "hbase" && !driverId) {
    hintText("内置 HBase Stargate REST，默认端口 8080（不是 Thrift 9090/Phoenix）。数据库填 namespace；使用 JSON scan/get/put/delete，二进制可用 *_base64。无认证端点可用 HTTP；Basic 凭据要求 HTTPS 或字面回环地址，ssl=true 启用验证证书的 HTTPS。Kerberos/SPNEGO 请显式配置自定义驱动。");
    params.placeholder = "无认证 REST 可留空；HTTPS 填 ssl=true";
  }
  if (kind === "argodb" && !driverId) {
    hintText("使用星环官方 Quark 完整 JDBC 与 Java 11+。主机填 ArgoDB Quark Gateway，端口通常 10000；当前托管路线明确 auth=noSasl，用户名和密码均留空。LDAP/NONE/Kerberos/TLS 请配置显式自定义 JDBC；不将该路线标记为这些认证已验证。");
    params.placeholder = "普通 noSasl 留空；其它认证使用自定义 JDBC";
  }
  hint.id = newConnectionId();
  if (["bigquery", "spanner", "salesforce", "hbase", "argodb"].includes(kind) && !driverId) {
    for (const control of [host, user, password, database]) control.setAttribute("aria-describedby", `${status.id} ${hint.id}`);
  }
  params.setAttribute("aria-describedby", `${status.id} ${hint.id}`);
  // 托管驱动版本选择（官方驱动多版本；仅在目录中有该数据源版本时显示）。
  const versionSelect = document.createElement("select"); versionSelect.setAttribute("aria-label", "数据库驱动版本");
  const versionField = connectionField("数据库驱动版本:", versionSelect);
  versionField.style.display = "none";
  advanced.append(hint, versionField, connectionField(driverId ? "连接串模板:" : "附加参数:", params));
  const option = (label: string, value: string): HTMLOptionElement => { const item = document.createElement("option"); item.textContent = label; item.value = value; return item; };
  const populateVersions = (rows: { kind: string; version: string; status: string; can_download: boolean }[]) => {
    if (driverId) return;
    const versions = rows.filter(r => r.kind === kind && r.version).map(r => ({ ...r }));
    // 已保存的版本不在当前已核验目录中时保留选项，避免打开旧连接即改坏配置。
    if (savedVersion?.kind === kind && !versions.some(r => r.version === savedVersion.version)) {
      versions.unshift({ kind, version: savedVersion.version, status: "missing", can_download: false });
    }
    if (!versions.length) return;
    versionSelect.replaceChildren(option("推荐版本（默认）", ""), ...versions.map(r => option(`${r.version}${r.status === "ready" ? "（已下载）" : "（未下载）"}`, r.version)));
    if (savedVersion?.kind === kind) versionSelect.value = savedVersion.version;
    versionField.style.display = "";
  };
  const cachedStatuses = currentDriverStatuses();
  if (cachedStatuses) populateVersions(cachedStatuses);
  else void ensureDriverStatuses().then(populateVersions).catch(() => undefined);
  // 本机 TLS/SSH/HTTP 配置独立加密保存。旧参数不在打开表单时重写或迁移。
  const tlsCapable = !driverId && (meta?.protocol === "mysql" || meta?.protocol === "postgres");
  const httpCapable = !driverId && kind === "h2";
  const tlsPanel = document.createElement("div");
  const sshPanel = document.createElement("div");
  const httpPanel = document.createElement("div");
  const runtimeEditor = tlsCapable || httpCapable ? createRuntimeProfileEditor({
    container: advanced, containers: { advanced, tls: tlsPanel, ssh: sshPanel, http: httpPanel }, status, connectionId, editing, bindings: runtimeProfiles,
    httpAvailable: httpCapable, databaseTransportAvailable: tlsCapable,
    rawParameters: () => params.value,
    pickFile: async () => { const path = await openFile({ multiple: false, directory: false, title: "选择证书或私钥文件" }); return typeof path === "string" ? path : null; },
  }) : null;
  const buildParams = (): string => {
    if (driverId) return encodeDriverParams(driverId, params.value.trim());
    const version = versionSelect.value;
    const raw = params.value;
    if (version) return encodeVersionParams(kind, version, raw);
    return raw;
  };
  const request = (): ConnectionRequest => ({ kind, host: host.value.trim(), port: Number(port.value) || 0,
    user: user.value.trim(), password: runtimeEditor?.usesHttp() ? "" : password.value, database: database.value.trim(), file: file.value.trim(),
    params: buildParams(), timeout_secs: 30 });
  let showTab = (_tab: string) => {};
  let tabExists = (_label: string) => false;
  const validate = (forTest = false): boolean => {
    for (const control of [name, host, port, file, password, params]) control.removeAttribute("aria-invalid");
    const invalid = (control: HTMLElement, message: string, tab = "常规"): false => {
      status.textContent = message; control.setAttribute("aria-invalid", "true"); showTab(tab); control.focus(); return false;
    };
    if (!name.value.trim()) return invalid(name, "请输入连接名称。");
    if (!port.value.trim() || !port.checkValidity()) return invalid(port, "端口必须是 0–65535 之间的整数。");
    if (confirmEmptyPassword && !runtimeEditor?.usesHttp()) {
      if (!host.value.trim()) return invalid(host, "请输入主机 / 地址。");
      if (Number(port.value) < 1) return invalid(port, "网络连接端口必须在 1–65535 之间。");
      if (forTest && !password.value && !(existing?.password && !passwordChanged) && !emptyPasswordCheckbox.checked) {
        return invalid(password, "请输入密码；若服务器允许无密码认证，请勾选“使用空密码 / 无密码认证”后再测试。");
      }
    }
    if (fileKind && !file.value.trim()) return invalid(file, "请选择数据库文件。");
    if (/(?:^|;)\s*(?:pwd|password)\s*=\s*(?!\{\{password\}\}(?:;|$))[^;\s]+/i.test(params.value)) {
      status.textContent = "请移除连接串里的明文密码，使用常规页密码字段。"; showTab("高级"); params.focus(); return false;
    }
    if (driverId && !driver) { status.textContent = "自定义驱动未载入或已删除，无法保存连接。"; return false; }
    try { runtimeEditor?.submission(); }
    catch (error) {
      const message = error instanceof Error ? error.message : "本机通道配置无效；输入已保留。";
      status.textContent = message;
      const tab = /SSH/.test(message) ? "SSH" : /HTTP/.test(message) ? "HTTP" : /TLS|证书|主机名/.test(message) ? "SSL/TLS" : "高级";
      if (tabExists(tab)) showTab(tab);
      return false;
    }
    return true;
  };
  let testing = false;
  let saving = false;
  let stopWaiting: (() => void) | undefined;
  const setBusy = (busy: boolean): void => {
    if (closed) return;
    if (busy) dlg.el.setAttribute("aria-busy", "true");
    else dlg.el.removeAttribute("aria-busy");
    for (const button of dlg.footer.querySelectorAll<HTMLButtonElement>("button")) {
      if (button.textContent === "停止等待") { button.hidden = !testing; button.disabled = !testing; }
      else if (button.textContent !== "取消") button.disabled = busy;
    }
    for (const control of [name, host, port, user, password, emptyPasswordCheckbox, database, file, params, versionSelect, saveCheckbox]) control.disabled = busy;
    for (const button of general.querySelectorAll<HTMLButtonElement>("button")) button.disabled = busy;
    runtimeEditor?.setBusy(busy);
  };
  const dlg = new Dialog({ title: `${editing ? "连接属性" : "新建连接"} (${driver?.name ?? kindLabel(kind)})`, width: 580, height: 570,
    render: (body) => {
      body.style.cssText = "display:flex;flex-direction:column;overflow:hidden";
      const tabs = document.createElement("div"); tabs.className = "tabs"; tabs.style.cssText = "flex:1;min-height:0";
      const header = document.createElement("div"); header.className = "tabs-header"; header.setAttribute("role", "tablist");
      const content = document.createElement("div"); content.className = "tabs-body"; content.style.overflow = "auto";
      const defs: [string, HTMLElement][] = [["常规", general], ["高级", advanced]];
      if (tlsCapable) defs.push(["SSL/TLS", tlsPanel], ["SSH", sshPanel]);
      if (httpCapable) defs.push(["HTTP", httpPanel]);
      const panels = new Map(defs); const buttons = new Map<string, HTMLButtonElement>();
      const order = defs.map(([label]) => label);
      tabExists = (label) => panels.has(label);
      for (const [label, panel] of panels) {
        panel.id = newConnectionId(); panel.setAttribute("role", "tabpanel");
        const button = document.createElement("button"); button.type = "button"; button.className = "tabs-tab"; button.textContent = label; button.id = newConnectionId();
        button.setAttribute("role", "tab"); button.setAttribute("aria-controls", panel.id); panel.setAttribute("aria-labelledby", button.id);
        button.addEventListener("click", () => showTab(label));
        button.addEventListener("keydown", (e) => { if (e.key !== "ArrowRight" && e.key !== "ArrowLeft") return; e.preventDefault(); const at = order.indexOf(label); const next = order[(at + (e.key === "ArrowRight" ? 1 : order.length - 1)) % order.length]; showTab(next); buttons.get(next)?.focus(); });
        buttons.set(label, button); header.append(button); content.append(panel);
      }
      showTab = (label) => {
        for (const [key, panel] of panels) panel.hidden = key !== label;
        for (const [key, button] of buttons) { button.classList.toggle("active", key === label); button.setAttribute("aria-selected", String(key === label)); button.tabIndex = key === label ? 0 : -1; }
      };
      showTab("常规"); tabs.append(header, content); body.append(tabs, status);
    },
    buttons: [
      { label: "测试连接", alignLeft: true, onClick: async () => {
        if (closed || testing || saving || !validate(true)) return false;
        const submission = runtimeEditor?.submission(true);
        const capturedRequest = request();
        const useSavedPassword = !runtimeEditor?.usesHttp() && Boolean(existing?.password) && !passwordChanged;
        // HTTP 使用其显式连接期限；IPC 多留 2 秒兜底，诊断也纳入同一次等待。
        const waitMs = (submission?.draft.http?.connect_timeout_ms ?? capturedRequest.timeout_secs * 1000) + 2000;
        let interrupted = false;
        let timer: ReturnType<typeof setTimeout> | undefined;
        const interruption = new Promise<string>((resolve) => {
          stopWaiting = () => { interrupted = true; resolve("已停止等待本次测试；后台连接可能仍在结束，迟到结果将忽略。可修改后重试。"); };
          timer = setTimeout(() => { interrupted = true; resolve(`测试连接超时（${waitMs / 1000} 秒），请检查地址、端口、认证及网络后重试；输入已保留。`); }, waitMs);
        });
        testing = true; status.textContent = `正在测试连接（最多等待 ${waitMs / 1000} 秒）…`; setBusy(true);
        try {
          const work = async (): Promise<string> => {
            const result = submission
              ? await runtimeProfiles!.test(capturedRequest, connectionId, submission, useSavedPassword)
              : await driverApi.testConnection(capturedRequest, existing?.id, useSavedPassword);
            if (interrupted || closed) return "";
            if (!result.ok) return `连接失败：${result.message || "服务端未提供错误详情，请检查连接配置。"}`;
            let diag = "";
            if (existing?.id && !submission) {
              try {
                const items = await driverApi.options(existing.id);
                const notable = items.filter((d) => d.status !== "applied");
                diag = notable.length ? "\n" + notable.map((d) => `· ${d.key}（${d.status}）：${d.message}`).join("\n") : "";
              } catch (error) { diag = `\n未能验证高级参数：${String(error)}`; }
            }
            return `连接成功${result.version ? ` — ${result.version}` : ""}${diag}`;
          };
          const message = await Promise.race([work(), interruption]);
          if (!closed) status.textContent = message;
        } catch (error) { if (!closed) status.textContent = `连接失败：${String(error)}`; }
        finally {
          clearTimeout(timer); stopWaiting = undefined; testing = false; setBusy(false);
        }
        return false;
      } },
      { label: "停止等待", alignLeft: true, hidden: true, onClick: () => { stopWaiting?.(); return false; } },
      { label: "保存", primary: true, onClick: async () => {
        if (closed || testing || saving || !validate()) return false;
        const req = request();
        const submission = runtimeEditor?.submission();
        const config: ConnectionConfig = { ...req, id: connectionId, name: name.value.trim(),
          password: runtimeEditor?.usesHttp() ? existing?.password ?? "" : saveCheckbox.checked ? existing?.password ?? "" : "", group: existing?.group ?? "",
          read_only: existing?.read_only ?? false, keep_alive: existing?.keep_alive ?? false };
        const savedPassword = runtimeEditor?.usesHttp() ? undefined : !saveCheckbox.checked ? "" : existing?.password && !passwordChanged ? undefined : password.value;
        saving = true; status.textContent = "正在保存连接…"; setBusy(true);
        try {
          await onSaved({ config, password: savedPassword, ...(submission ? { runtimeProfile: submission } : {}) });
        } catch (error) {
          if (!closed) status.textContent = `保存失败：${String(error)}`;
          return false;
        } finally {
          saving = false; setBusy(false);
        }
      } }, { label: "取消" },
    ],
  });
  prepareConnectionDialog(dlg); dlg.onClose(() => {
    closed = true; stopWaiting?.(); runtimeEditor?.dispose();
    if (previousFocus?.isConnected) previousFocus.focus();
  });
  // 结果只属于提交时的输入快照；修改表单后不要继续展示旧的“连接成功”。
  for (const event of ["input", "change"]) dlg.body.addEventListener(event, (e) => {
    if (testing || saving || closed) return;
    (e.target as HTMLElement).removeAttribute("aria-invalid");
    status.textContent = "连接配置已更改，请重新测试。";
  });
  if (driverId && !driver) void driverApi.list().then((drivers) => {
    if (closed) return;
    driver = drivers.find((item) => item.id === driverId); updateSummary();
    if (!driver) status.textContent = "此连接使用的自定义驱动已删除，请在驱动管理中恢复。";
    else dlg.setTitle(`${editing ? "连接属性" : "新建连接"} (${driver.name})`);
  }).catch((error) => { if (!closed) status.textContent = `读取驱动失败：${String(error)}`; });
  return dlg;
}