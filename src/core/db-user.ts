/** 用户编辑（MySQL 协议族）的前端模型与纯逻辑；与 src-tauri/src/db_users.rs 对应。 */

export interface DbUserAccount {
  user: string;
  host: string;
  plugin: string;
  password_expired: boolean;
  password_lifetime_days: number | null;
  account_locked: boolean;
  max_questions: number;
  max_updates: number;
  max_connections: number;
  max_user_connections: number;
  ssl_type: string;
  ssl_cipher: string;
  x509_issuer: string;
  x509_subject: string;
}

export interface DbUserPrivilege {
  name: string;
  granted: boolean;
  grantable: boolean;
}

export interface DbUserObjectPrivilege {
  database: string;
  name: string;
  privileges: string[];
  grantable: boolean;
}

export interface DbUserRole {
  role: string;
  host: string;
}

export interface DbUserMember {
  user: string;
  host: string;
}

export interface DbUserDetail {
  account: DbUserAccount;
  server_privileges: DbUserPrivilege[];
  object_privileges: DbUserObjectPrivilege[];
  roles: DbUserRole[];
  members: DbUserMember[];
  supports_roles: boolean;
  warnings: string[];
}

export interface DbUserGrantDraft {
  database: string;
  name: string;
  privileges: string[];
}

export interface DbUserDraft {
  user: string;
  host: string;
  new_user: string;
  new_host: string;
  password: string | null;
  plugin: string;
  password_expiry: string;
  password_lifetime_days: number;
  account_locked: boolean;
  max_questions: number;
  max_updates: number;
  max_connections: number;
  max_user_connections: number;
  ssl_type: string;
  ssl_cipher: string;
  x509_issuer: string;
  x509_subject: string;
  server_privileges: string[];
  object_grants: DbUserGrantDraft[];
  roles: DbUserRole[];
}

export interface DbUserPlan {
  statements: string[];
  warnings: string[];
}

export interface DbUserApplyResult {
  statements: string[];
  executed: number;
}

/** 认证插件候选（可手输其它插件）。 */
export const AUTH_PLUGIN_OPTIONS = [
  "caching_sha2_password",
  "mysql_native_password",
  "sha256_password",
  "mysql_clear_password",
  "auth_socket",
];

/** 密码过期策略。 */
export const PASSWORD_EXPIRY_OPTIONS = [
  { value: "default", label: "默认（DEFAULT）" },
  { value: "never", label: "永不过期（NEVER）" },
  { value: "interval", label: "按天过期（INTERVAL N DAY）" },
];

/** SSL 类型。 */
export const SSL_TYPE_OPTIONS = [
  { value: "", label: "无" },
  { value: "ANY", label: "需要 SSL" },
  { value: "X509", label: "需要 X509 证书" },
  { value: "SPECIFIED", label: "指定 ISSUER / SUBJECT / CIPHER" },
];

/** 对象级权限表格的列（与参考截图一致）。 */
export const OBJECT_PRIVILEGE_COLUMNS = [
  "Select",
  "Insert",
  "Update",
  "Delete",
  "Create",
  "Drop",
  "Alter",
  "Index",
  "Execute",
  "References",
  "Create View",
  "Show View",
  "Trigger",
];

/** 解析 user@host（用户名本身可能含 @，取最后一个 @）。 */
export function splitAccountName(account: string): { user: string; host: string } {
  const at = account.lastIndexOf("@");
  if (at < 0) return { user: account, host: "" };
  return { user: account.slice(0, at), host: account.slice(at + 1) };
}

/** 权限比较键：忽略空白差异并大写。 */
export function privilegeKey(name: string): string {
  return name.trim().split(/\s+/).filter(Boolean).join("_").toUpperCase();
}

/** 由账号的 password_lifetime_days 推导过期策略。 */
export function expiryFromAccount(account: DbUserAccount): string {
  if (account.password_lifetime_days === null || account.password_lifetime_days === undefined) return "default";
  return account.password_lifetime_days === 0 ? "never" : "interval";
}

/** 从加载到的详情构造初始草稿。 */
export function draftFromDetail(detail: DbUserDetail): DbUserDraft {
  const account = detail.account;
  return {
    user: account.user,
    host: account.host,
    new_user: account.user,
    new_host: account.host,
    password: null,
    plugin: account.plugin,
    password_expiry: expiryFromAccount(account),
    password_lifetime_days: account.password_lifetime_days ?? 0,
    account_locked: account.account_locked,
    max_questions: account.max_questions,
    max_updates: account.max_updates,
    max_connections: account.max_connections,
    max_user_connections: account.max_user_connections,
    ssl_type: account.ssl_type || "",
    ssl_cipher: account.ssl_cipher,
    x509_issuer: account.x509_issuer,
    x509_subject: account.x509_subject,
    server_privileges: detail.server_privileges.filter((item) => item.granted).map((item) => item.name),
    object_grants: detail.object_privileges.map((row) => ({
      database: row.database,
      name: row.name,
      privileges: row.privileges.slice(),
    })),
    roles: detail.roles.map((role) => ({ role: role.role, host: role.host })),
  };
}

/** 对象级权限行的稳定键。 */
export function objectGrantKey(database: string, name: string): string {
  return database + "\u0000" + name;
}

/** 当前角色集合转 user@host 数组。 */
export function roleAccount(role: DbUserRole): string {
  return role.role + "@" + role.host;
}
