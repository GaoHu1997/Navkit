import type { RuntimeHttpProfile, RuntimeProfileDraft, RuntimeProfileView, RuntimeSecretPatch, RuntimeTlsMode } from "./runtime-profile";

/** Inspection only: never reserialize or trim a saved option value while opening a form. */
export function runtimeParameterKeys(raw: string): string[] {
  const text = raw.trim();
  if (!text || text.startsWith("@navkit-driver")) return [];
  if (text.startsWith("{")) {
    const value: unknown = JSON.parse(text);
    if (value === null || typeof value !== "object" || Array.isArray(value)) throw new Error("附加参数必须是 JSON 对象。");
    return Object.keys(value).map(key => key.toLowerCase());
  }
  return text.split(/[;&]/).filter(part => part.trim()).map(part => {
    const at = part.indexOf("=");
    if (at <= 0) throw new Error("每个附加参数必须是 key=value；原文已保留。");
    return part.slice(0, at).trim().toLowerCase();
  });
}
export function isRuntimeParameter(key: string): boolean {
  return key.startsWith("ssh_") || key.startsWith("http_") || key === "runtime_profile_id" || /^(ssl|tls)(?:$|[_-])/.test(key) || ["sslmode", "sslrootcert", "cacert", "cafile", "ca", "cert", "key", "root-cert", "root_cert", "client-cert", "client_cert", "client-key", "client_key", "insecure", "server_name", "trust-server-certificate", "authorization", "bearer", "access_token"].includes(key);
}
export function hasPlaintextRuntimeSecret(keys: readonly string[]): boolean { return keys.some(key => ["ssh_password", "ssh_key_passphrase", "pwd", "password", "http_bearer", "http_token", "authorization", "bearer", "access_token"].includes(key)); }
export function tlsModeToUi(mode: RuntimeTlsMode | undefined): string { return mode?.replaceAll("_", "-") ?? ""; }
export function tlsModeFromUi(mode: string): RuntimeTlsMode | null {
  if (!mode) return null;
  if (!["disable", "prefer", "require", "verify-ca", "verify-full"].includes(mode)) throw new Error("TLS 模式无效；不会回退明文。");
  return mode.replaceAll("-", "_") as RuntimeTlsMode;
}
const tlsModes: readonly string[] = ["disable", "prefer", "require", "verify_ca", "verify_full"];
const shape = (value: unknown, keys: readonly string[]): value is Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value) && Object.keys(value).every(key => keys.includes(key));
const cleanText = (value: unknown, limit = 4096): value is string => typeof value === "string" && value.length <= limit && !/[\u0000-\u001f\u007f]/.test(value);
const nullablePath = (value: unknown): boolean => value === null || cleanText(value);
const utf8Length = (value: string): number => new TextEncoder().encode(value).length;
const canonicalUuid = (value: unknown): boolean => typeof value === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value);
function validateSecretPatch(patch: RuntimeSecretPatch): void {
  if (!shape(patch, ["action", "value"]) || !["keep", "clear", "set"].includes(patch.action)) throw new Error("秘密修改方式无效；不会删除已保存秘密。");
  if (patch.action === "set" && (typeof patch.value !== "string" || !patch.value || utf8Length(patch.value) > 16 * 1024)) throw new Error("新秘密必须为 1 字节至 16KiB；删除请明确选择清除。");
  if (patch.action !== "set" && "value" in patch) throw new Error("保留或清除秘密不能同时携带新秘密。");
}
export function validateHttpEndpoint(value: string): URL {
  if (!cleanText(value) || !value || /\s/.test(value)) throw new Error("HTTP 端点必须是不含空白或控制字符的 HTTPS origin。");
  const raw = value.match(/^https:\/\/([^/\\@?#]+)\/?$/i);
  if (!raw) throw new Error("HTTP 端点只能是 HTTPS origin，不能包含账号、路径、查询或片段。");
  let endpoint: URL;
  try { endpoint = new URL(value); } catch { throw new Error("HTTP 端点必须是有效的 HTTPS origin。"); }
  if (endpoint.protocol !== "https:" || !endpoint.hostname || endpoint.username || endpoint.password || endpoint.pathname !== "/" || endpoint.search || endpoint.hash) throw new Error("HTTP 端点只能是 HTTPS origin，不能包含账号、路径、查询或片段。");
  return endpoint;
}
export function validateHttpProfile(http: RuntimeHttpProfile): void {
  if (!shape(http, ["endpoint", "protocol", "expected_server_id", "expected_target_id", "target_alias", "tls", "connect_timeout_ms"])) throw new Error("HTTP 本机配置包含无效字段。");
  validateHttpEndpoint(http.endpoint);
  if (http.protocol !== "navkit-sql-tunnel/1") throw new Error("HTTP 通道只支持 navkit-sql-tunnel/1，不会按普通 REST 回退。");
  if (!canonicalUuid(http.expected_server_id) || !canonicalUuid(http.expected_target_id)) throw new Error("请明确填写桥接服务 ID 和目标 ID，必须是小写、带连字符的规范 UUID。");
  if (!cleanText(http.target_alias, 200) || !http.target_alias || http.target_alias.trim() !== http.target_alias || utf8Length(http.target_alias) > 200) throw new Error("请填写桥接服务预配 alias（1 至 200 字节）；不会自动发现或猜测目标。");
  if (!Number.isInteger(http.connect_timeout_ms) || http.connect_timeout_ms < 1000 || http.connect_timeout_ms > 60_000) throw new Error("HTTP 连接期限必须为 1000–60000 毫秒。");
  if (!shape(http.tls, ["mode", "ca", "cert", "key"]) || http.tls.mode !== "verify_full" || ![http.tls.ca, http.tls.cert, http.tls.key].every(nullablePath)) throw new Error("HTTP 外层 HTTPS 必须 VerifyFull 验证 CA 和原主机名，证书路径必须有效。");
  if (Boolean(http.tls.cert) !== Boolean(http.tls.key)) throw new Error("HTTP 客户端证书与私钥必须成对提供。");
}
export function httpProfileSameTarget(left: RuntimeHttpProfile, right: RuntimeHttpProfile): boolean {
  validateHttpProfile(left); validateHttpProfile(right);
  return validateHttpEndpoint(left.endpoint).origin === validateHttpEndpoint(right.endpoint).origin && left.protocol === right.protocol && left.expected_server_id === right.expected_server_id && left.expected_target_id === right.expected_target_id && left.target_alias === right.target_alias;
}
export function assertProfileView(view: RuntimeProfileView | null, connectionId: string): void {
  if (view === null) return;
  if (!shape(view, ["connection_id", "endpoint_fingerprint", "revision", "ssh", "tls", "ssh_password_present", "ssh_key_passphrase_present", "http", "http_bearer_present"]) || view.connection_id !== connectionId || !/^[0-9a-f]{64}$/i.test(view.revision) || !/^[0-9a-f]{64}$/i.test(view.endpoint_fingerprint) || typeof view.ssh_password_present !== "boolean" || typeof view.ssh_key_passphrase_present !== "boolean") throw new Error("本机通道配置回执与连接身份不符；输入已保留。");
  if (view.ssh !== null && !shape(view.ssh, ["host", "port", "user", "authentication", "key_file", "known_hosts"]) || view.tls !== null && !shape(view.tls, ["mode", "ca", "cert", "key"])) throw new Error("本机通道配置回执包含无效字段；不会覆盖未知配置。");
  const hasHttp = Object.prototype.hasOwnProperty.call(view, "http"), hasPresence = Object.prototype.hasOwnProperty.call(view, "http_bearer_present");
  if (hasHttp !== hasPresence || hasHttp && (typeof view.http_bearer_present !== "boolean" || view.http === undefined)) throw new Error("HTTP 本机配置回执缺少明确的令牌存在状态。");
  validateRuntimeDraft({ ssh: view.ssh, tls: view.tls, ssh_password: { action: "keep" }, ssh_key_passphrase: { action: "keep" }, http: view.http, http_bearer: { action: "keep" } }, view);
}
export function validateRuntimeDraft(draft: RuntimeProfileDraft, previous: RuntimeProfileView | null, rebindEndpoint = false): void {
  validateSecretPatch(draft.ssh_password); validateSecretPatch(draft.ssh_key_passphrase);
  const bearer = draft.http_bearer ?? { action: "keep" };
  validateSecretPatch(bearer);
  if (bearer.action === "set" && /[^\x21-\x7e]/.test(bearer.value)) throw new Error("HTTP Bearer 只接受可见 ASCII 字符；不允许空白、CRLF 或控制字符。");
  const http = draft.http;
  if (http) {
    validateHttpProfile(http);
    if (draft.ssh || draft.tls) throw new Error("HTTP 外层通道与 SSH、数据库 TLS 配置互斥；请明确选择连接方式。");
    if (draft.ssh_password.action === "set" || draft.ssh_key_passphrase.action === "set") throw new Error("HTTP 通道不能同时提交新的 SSH 秘密。");
    if (bearer.action !== "set" && !(bearer.action === "keep" && previous?.http_bearer_present === true)) throw new Error("HTTP 通道需要 Bearer 令牌；关闭通道后才能明确清除令牌，不会直连回退。");
    if (bearer.action === "keep" && previous?.http_bearer_present && !rebindEndpoint && (!previous.http || !httpProfileSameTarget(previous.http, http))) throw new Error("HTTP 服务或目标已改变；保留旧 Bearer 需要显式重新绑定并匹配原配置版本。");
  } else if (bearer.action === "set") throw new Error("填写 HTTP Bearer 前必须启用完整 HTTP 通道配置。");
  const tls = draft.tls;
  if (tls) {
    if (!tlsModes.includes(tls.mode) || ![tls.ca, tls.cert, tls.key].every(nullablePath)) throw new Error("TLS 模式或证书路径无效；不会降级明文。");
    if (Boolean(tls.cert) !== Boolean(tls.key)) throw new Error("客户端证书与私钥必须成对提供。");
    if ((tls.ca || tls.cert || tls.key) && !["verify_ca", "verify_full"].includes(tls.mode)) throw new Error("证书材料要求明确选择 verify-ca 或 verify-full；不会默认 prefer 降级。");
  }
  const ssh = draft.ssh;
  if (!ssh) {
    if (draft.ssh_password.action === "set" || draft.ssh_key_passphrase.action === "set") throw new Error("填写 SSH 秘密前必须启用 SSH；不会静默保存未使用秘密。");
    return;
  }
  if (!cleanText(ssh.host, 253) || !ssh.host.trim() || !cleanText(ssh.user, 128) || !ssh.user.trim()) throw new Error("启用 SSH 后必须填写合法主机和用户名。");
  if (!Number.isInteger(ssh.port) || ssh.port < 1 || ssh.port > 65535) throw new Error("SSH 端口必须为 1–65535。");
  if (!cleanText(ssh.known_hosts) || !ssh.known_hosts.trim() || !nullablePath(ssh.key_file)) throw new Error("SSH 必须指定合法 known_hosts 路径；不会自动信任首次主机密钥。");
  if (tls?.mode !== "disable") throw new Error("当前驱动的 SSH 与 TLS 原主机名尚不能安全组合；SSH 必须明确选择 TLS disable。");
  if (!["password", "private_key"].includes(ssh.authentication)) throw new Error("SSH 认证方式无效；不会切换认证重试。");
  if (ssh.authentication === "password") {
    if (ssh.key_file) throw new Error("SSH 密码认证不能同时指定私钥。");
    if (draft.ssh_password.action !== "set" && !(draft.ssh_password.action === "keep" && previous?.ssh_password_present)) throw new Error("请输入 SSH 密码或保留本机已保存密码。");
  } else if (!ssh.key_file) throw new Error("SSH 私钥认证必须提供 OpenSSH 私钥路径。");
}
