//! MySQL 协议族的用户编辑（参考 通用数据库客户端「编辑用户」）：读取账号常规/高级属性与权限，
//! 生成变更 SQL 预览，并在用户确认后按条目执行。
//!
//! 只有 MySQL 协议族（mysql.user 与 information_schema 权限视图）支持；其它后端返回错误。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sqlx::mysql::MySqlRow;
use tauri::State;

use crate::commands::{ensure_db, kind_of, require_writable_connection, AppState};
use crate::db::Db;
use crate::error::{AppError, CmdResult};

/// 服务器级静态权限（顺序与参考程序一致；动态权限按实际授予追加在后）。
const STATIC_SERVER_PRIVILEGES: [&str; 27] = [
    "Alter", "Alter Routine", "Create", "Create Routine", "Create Temporary Tables",
    "Create User", "Create View", "Delete", "Drop", "Event", "Execute", "File", "Index",
    "Insert", "Lock Tables", "Process", "References", "Reload", "Replication Client",
    "Replication Slave", "Select", "Show Databases", "Show View", "Shutdown", "Super",
    "Trigger", "Update",
];

/// 账号常规 / 高级属性（对应「常规」「高级」两个标签页）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DbUserAccount {
    pub user: String,
    pub host: String,
    /// 认证插件；MariaDB 等可能为空。
    pub plugin: String,
    pub password_expired: bool,
    /// None 表示 PASSWORD EXPIRE DEFAULT；Some(0) 表示 NEVER；Some(n) 表示 INTERVAL n DAY。
    pub password_lifetime_days: Option<i64>,
    pub account_locked: bool,
    pub max_questions: i64,
    pub max_updates: i64,
    pub max_connections: i64,
    pub max_user_connections: i64,
    /// 空字符串表示未要求 SSL；ANY / X509 / SPECIFIED。
    pub ssl_type: String,
    pub ssl_cipher: String,
    pub x509_issuer: String,
    pub x509_subject: String,
}

/// 一项服务器级权限。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DbUserPrivilege {
    pub name: String,
    pub granted: bool,
    pub grantable: bool,
}

/// 一行对象级权限：name 为 "*" 表示整库范围。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DbUserObjectPrivilege {
    pub database: String,
    pub name: String,
    pub privileges: Vec<String>,
    pub grantable: bool,
}

/// 角色（「成员属于」）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DbUserRole {
    pub role: String,
    pub host: String,
}

/// 成员（当前角色被授予给哪些账号）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DbUserMember {
    pub user: String,
    pub host: String,
}

/// 编辑器加载的完整用户详情。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DbUserDetail {
    pub account: DbUserAccount,
    pub server_privileges: Vec<DbUserPrivilege>,
    pub object_privileges: Vec<DbUserObjectPrivilege>,
    pub roles: Vec<DbUserRole>,
    pub members: Vec<DbUserMember>,
    pub supports_roles: bool,
    pub warnings: Vec<String>,
}

/// 对象级授权草稿行。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DbUserGrantDraft {
    pub database: String,
    pub name: String,
    pub privileges: Vec<String>,
}

/// 编辑器提交的完整期望状态（与加载时的 current 做差量）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DbUserDraft {
    /// 当前身份（编辑前）。
    pub user: String,
    pub host: String,
    /// 改名后的身份；为空表示不改。
    pub new_user: String,
    pub new_host: String,
    /// 新密码；None 表示不改密码。
    pub password: Option<String>,
    pub plugin: String,
    /// default / never / interval。
    pub password_expiry: String,
    pub password_lifetime_days: i64,
    pub account_locked: bool,
    pub max_questions: i64,
    pub max_updates: i64,
    pub max_connections: i64,
    pub max_user_connections: i64,
    pub ssl_type: String,
    pub ssl_cipher: String,
    pub x509_issuer: String,
    pub x509_subject: String,
    pub server_privileges: Vec<String>,
    pub object_grants: Vec<DbUserGrantDraft>,
    pub roles: Vec<DbUserRole>,
}

/// 预览结果：将执行的语句与提示。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DbUserPlan {
    pub statements: Vec<String>,
    pub warnings: Vec<String>,
}

/// 应用结果。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DbUserApplyResult {
    pub statements: Vec<String>,
    pub executed: usize,
}

// ------------------------------------------------------------------ 小工具

fn cells(row: &MySqlRow) -> Vec<Option<String>> {
    crate::schema::mysql_meta_row_values(row)
}

fn text(values: &[Option<String>], index: usize) -> String {
    values.get(index).and_then(|value| value.clone()).unwrap_or_default()
}

fn parse_i64(value: &str) -> i64 {
    value.trim().parse::<i64>().unwrap_or(0)
}

/// 反引号标识符。
fn quote_ident(name: &str) -> String {
    format!("\u{0060}{}\u{0060}", name.replace('\u{0060}', "\u{0060}\u{0060}"))
}

/// 单引号字面量。
fn quote_user(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn principal(user: &str, host: &str) -> String {
    format!("{}@{}", quote_user(user), quote_user(host))
}

/// 权限比较键：忽略空格/下划线差异并大写。
fn privilege_key(name: &str) -> String {
    name.split(|ch: char| ch.is_whitespace() || ch == '_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_")
        .to_ascii_uppercase()
}

/// 权限显示名：ALTER_ROUTINE -> Alter Routine。
fn display_privilege(name: &str) -> String {
    name.split(|ch: char| ch.is_whitespace() || ch == '_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str().to_ascii_lowercase()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn object_ref(database: &str, name: &str) -> String {
    if name.is_empty() || name == "*" {
        format!("{}.*", quote_ident(database))
    } else {
        format!("{}.{}", quote_ident(database), quote_ident(name))
    }
}

fn join_names(names: &BTreeSet<String>) -> String {
    names.iter().cloned().collect::<Vec<_>>().join(", ")
}

// ------------------------------------------------------------------ 读取

async fn user_table_columns(pool: &sqlx::MySqlPool) -> anyhow::Result<BTreeSet<String>> {
    let rows = crate::sql_history::query(
        "SELECT COLUMN_NAME AS column_name FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = 'mysql' AND TABLE_NAME = 'user'",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(|row| text(&cells(row), 0).to_ascii_lowercase()).collect())
}

async fn read_account(
    pool: &sqlx::MySqlPool,
    columns: &BTreeSet<String>,
    user: &str,
    host: &str,
) -> anyhow::Result<DbUserAccount> {
    let mut selected: Vec<&str> = vec!["user", "host"];
    for optional in [
        "plugin", "password_expired", "password_lifetime", "account_locked", "max_questions",
        "max_updates", "max_connections", "max_user_connections", "ssl_type", "ssl_cipher",
        "x509_issuer", "x509_subject",
    ] {
        if columns.contains(optional) {
            selected.push(optional);
        }
    }
    let sql = format!(
        "SELECT {} FROM mysql.user WHERE user = ? AND host = ?",
        selected.join(", ")
    );
    let rows = crate::sql_history::query(&sql).bind(user).bind(host).fetch_all(pool).await?;
    let row = rows
        .first()
        .ok_or_else(|| anyhow::anyhow!("用户 {}@{} 不存在", user, host))?;
    let values = cells(row);
    let get = |name: &str| -> Option<String> {
        selected.iter().position(|item| *item == name).and_then(|index| values.get(index).and_then(|value| value.clone()))
    };
    let flag = |name: &str| get(name).map(|value| value.eq_ignore_ascii_case("Y") || value.eq_ignore_ascii_case("YES")).unwrap_or(false);
    Ok(DbUserAccount {
        user: get("user").unwrap_or_else(|| user.to_string()),
        host: get("host").unwrap_or_else(|| host.to_string()),
        plugin: get("plugin").unwrap_or_default(),
        password_expired: flag("password_expired"),
        password_lifetime_days: get("password_lifetime").and_then(|value| value.trim().parse::<i64>().ok()),
        account_locked: flag("account_locked"),
        max_questions: parse_i64(&get("max_questions").unwrap_or_default()),
        max_updates: parse_i64(&get("max_updates").unwrap_or_default()),
        max_connections: parse_i64(&get("max_connections").unwrap_or_default()),
        max_user_connections: parse_i64(&get("max_user_connections").unwrap_or_default()),
        ssl_type: get("ssl_type").unwrap_or_default(),
        ssl_cipher: get("ssl_cipher").unwrap_or_default(),
        x509_issuer: get("x509_issuer").unwrap_or_default(),
        x509_subject: get("x509_subject").unwrap_or_default(),
    })
}

async fn read_server_privileges(
    pool: &sqlx::MySqlPool,
    user: &str,
    host: &str,
) -> anyhow::Result<Vec<DbUserPrivilege>> {
    let rows = crate::sql_history::query(
        "SELECT PRIVILEGE_TYPE AS privilege_type, IS_GRANTABLE AS is_grantable \
         FROM information_schema.USER_PRIVILEGES WHERE GRANTEE = ?",
    )
    .bind(principal(user, host))
    .fetch_all(pool)
    .await?;
    let mut granted: BTreeMap<String, (String, bool)> = BTreeMap::new();
    for row in &rows {
        let values = cells(row);
        let name = text(&values, 0);
        if name.is_empty() {
            continue;
        }
        let grantable = text(&values, 1).eq_ignore_ascii_case("YES");
        granted.insert(privilege_key(&name), (name, grantable));
    }
    let mut any_grantable = false;
    let mut out = Vec::new();
    for name in STATIC_SERVER_PRIVILEGES {
        let key = privilege_key(name);
        let entry = granted.remove(&key);
        let grantable = entry.as_ref().map(|(_, value)| *value).unwrap_or(false);
        any_grantable |= grantable;
        out.push(DbUserPrivilege { name: name.to_string(), granted: entry.is_some(), grantable });
    }
    for (key, (name, grantable)) in granted {
        if key == "GRANT_OPTION" {
            any_grantable |= grantable;
            continue;
        }
        any_grantable |= grantable;
        out.push(DbUserPrivilege { name, granted: true, grantable });
    }
    out.push(DbUserPrivilege { name: "Grant Option".to_string(), granted: any_grantable, grantable: any_grantable });
    Ok(out)
}

async fn read_object_privileges(
    pool: &sqlx::MySqlPool,
    user: &str,
    host: &str,
) -> anyhow::Result<Vec<DbUserObjectPrivilege>> {
    let grantee = principal(user, host);
    let mut rows_by_key: BTreeMap<(String, String), (BTreeSet<String>, bool)> = BTreeMap::new();
    let schema_rows = crate::sql_history::query(
        "SELECT TABLE_SCHEMA AS table_schema, PRIVILEGE_TYPE AS privilege_type, IS_GRANTABLE AS is_grantable \
         FROM information_schema.SCHEMA_PRIVILEGES WHERE GRANTEE = ?",
    )
    .bind(&grantee)
    .fetch_all(pool)
    .await?;
    for row in &schema_rows {
        let values = cells(row);
        let database = text(&values, 0);
        let privilege = text(&values, 1);
        if database.is_empty() || privilege.is_empty() {
            continue;
        }
        let entry = rows_by_key.entry((database, "*".to_string())).or_default();
        entry.0.insert(display_privilege(&privilege));
        entry.1 |= text(&values, 2).eq_ignore_ascii_case("YES");
    }
    let table_rows = crate::sql_history::query(
        "SELECT TABLE_SCHEMA AS table_schema, TABLE_NAME AS table_name, PRIVILEGE_TYPE AS privilege_type, IS_GRANTABLE AS is_grantable \
         FROM information_schema.TABLE_PRIVILEGES WHERE GRANTEE = ?",
    )
    .bind(&grantee)
    .fetch_all(pool)
    .await?;
    for row in &table_rows {
        let values = cells(row);
        let database = text(&values, 0);
        let name = text(&values, 1);
        let privilege = text(&values, 2);
        if database.is_empty() || name.is_empty() || privilege.is_empty() {
            continue;
        }
        let entry = rows_by_key.entry((database, name)).or_default();
        entry.0.insert(display_privilege(&privilege));
        entry.1 |= text(&values, 3).eq_ignore_ascii_case("YES");
    }
    Ok(rows_by_key
        .into_iter()
        .map(|((database, name), (privileges, grantable))| DbUserObjectPrivilege {
            database,
            name,
            privileges: privileges.into_iter().collect(),
            grantable,
        })
        .collect())
}

async fn read_roles(
    pool: &sqlx::MySqlPool,
    user: &str,
    host: &str,
) -> anyhow::Result<(Vec<DbUserRole>, Vec<DbUserMember>, bool)> {
    let count_rows = crate::sql_history::query(
        "SELECT COUNT(*) AS table_count FROM information_schema.TABLES WHERE TABLE_SCHEMA = 'mysql' AND TABLE_NAME = 'role_edges'",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let supports_roles = count_rows
        .first()
        .map(|row| parse_i64(&text(&cells(row), 0)) > 0)
        .unwrap_or(false);
    if !supports_roles {
        return Ok((Vec::new(), Vec::new(), false));
    }
    let rows = crate::sql_history::query(
        "SELECT FROM_USER AS from_user, FROM_HOST AS from_host, TO_USER AS to_user, TO_HOST AS to_host FROM mysql.role_edges",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let mut roles = Vec::new();
    let mut members = Vec::new();
    for row in &rows {
        let values = cells(row);
        let from_user = text(&values, 0);
        let from_host = text(&values, 1);
        let to_user = text(&values, 2);
        let to_host = text(&values, 3);
        if to_user == user && to_host == host {
            roles.push(DbUserRole { role: from_user.clone(), host: from_host.clone() });
        }
        if from_user == user && from_host == host {
            members.push(DbUserMember { user: to_user, host: to_host });
        }
    }
    Ok((roles, members, true))
}

async fn load_detail(pool: &sqlx::MySqlPool, user: &str, host: &str) -> anyhow::Result<DbUserDetail> {
    let columns = user_table_columns(pool).await?;
    let account = read_account(pool, &columns, user, host).await?;
    let server_privileges = read_server_privileges(pool, user, host).await?;
    let object_privileges = read_object_privileges(pool, user, host).await?;
    let mut warnings = Vec::new();
    let (roles, members, supports_roles) = read_roles(pool, user, host).await?;
    if !supports_roles {
        warnings.push("当前服务器没有 mysql.role_edges（MySQL 8.0 以下或 MariaDB），「成员属于 / 成员」不可用。".to_string());
    }
    Ok(DbUserDetail {
        account,
        server_privileges,
        object_privileges,
        roles,
        members,
        supports_roles,
        warnings,
    })
}

// ------------------------------------------------------------------ 差量生成

/// 根据加载到的当前状态与提交的期望状态，生成变更语句（纯函数，可单测）。
pub fn build_user_plan(current: &DbUserDetail, draft: &DbUserDraft) -> DbUserPlan {
    let mut statements = Vec::new();
    let mut warnings = Vec::new();

    let old_user = current.account.user.clone();
    let old_host = current.account.host.clone();
    let new_user = if draft.new_user.trim().is_empty() { old_user.clone() } else { draft.new_user.trim().to_string() };
    let new_host = if draft.new_host.trim().is_empty() { old_host.clone() } else { draft.new_host.trim().to_string() };
    let target = principal(&new_user, &new_host);

    if new_user != old_user || new_host != old_host {
        statements.push(format!(
            "RENAME USER {} TO {}",
            principal(&old_user, &old_host),
            target
        ));
    }

    // 认证方式 / 密码。
    if let Some(password) = draft.password.as_ref() {
        let plugin = draft.plugin.trim();
        if plugin.is_empty() {
            statements.push(format!("ALTER USER {} IDENTIFIED BY {}", target, quote_user(password)));
        } else {
            statements.push(format!(
                "ALTER USER {} IDENTIFIED WITH {} BY {}",
                target,
                quote_ident(plugin),
                quote_user(password)
            ));
        }
    }

    // 资源限制。
    if draft.max_questions != current.account.max_questions
        || draft.max_updates != current.account.max_updates
        || draft.max_connections != current.account.max_connections
        || draft.max_user_connections != current.account.max_user_connections
    {
        statements.push(format!(
            "ALTER USER {} WITH MAX_QUERIES_PER_HOUR {} MAX_UPDATES_PER_HOUR {} MAX_CONNECTIONS_PER_HOUR {} MAX_USER_CONNECTIONS {}",
            target,
            draft.max_questions.max(0),
            draft.max_updates.max(0),
            draft.max_connections.max(0),
            draft.max_user_connections.max(0)
        ));
    }

    // SSL 要求。
    let current_ssl = current.account.ssl_type.trim().to_ascii_uppercase();
    let desired_ssl = draft.ssl_type.trim().to_ascii_uppercase();
    if desired_ssl != current_ssl
        || draft.ssl_cipher != current.account.ssl_cipher
        || draft.x509_issuer != current.account.x509_issuer
        || draft.x509_subject != current.account.x509_subject
    {
        let mut clauses: Vec<String> = Vec::new();
        match desired_ssl.as_str() {
            "ANY" => clauses.push("REQUIRE SSL".to_string()),
            "X509" => clauses.push("REQUIRE X509".to_string()),
            "SPECIFIED" => {
                if !draft.x509_issuer.trim().is_empty() {
                    clauses.push(format!("ISSUER {}", quote_user(draft.x509_issuer.trim())));
                }
                if !draft.x509_subject.trim().is_empty() {
                    clauses.push(format!("SUBJECT {}", quote_user(draft.x509_subject.trim())));
                }
                if !draft.ssl_cipher.trim().is_empty() {
                    clauses.push(format!("CIPHER {}", quote_user(draft.ssl_cipher.trim())));
                }
                if clauses.is_empty() {
                    clauses.push("REQUIRE NONE".to_string());
                } else {
                    clauses.insert(0, "REQUIRE".to_string());
                }
            }
            _ => clauses.push("REQUIRE NONE".to_string()),
        }
        statements.push(format!("ALTER USER {} {}", target, clauses.join(" ")));
    }

    // 密码过期策略。
    let desired_lifetime = match draft.password_expiry.as_str() {
        "never" => Some(0i64),
        "interval" => Some(draft.password_lifetime_days.max(1)),
        _ => None,
    };
    if desired_lifetime != current.account.password_lifetime_days {
        let clause = match desired_lifetime {
            Some(0) => "PASSWORD EXPIRE NEVER".to_string(),
            Some(days) => format!("PASSWORD EXPIRE INTERVAL {} DAY", days),
            None => "PASSWORD EXPIRE DEFAULT".to_string(),
        };
        statements.push(format!("ALTER USER {} {}", target, clause));
    }

    // 账户锁定。
    if draft.account_locked != current.account.account_locked {
        statements.push(format!(
            "ALTER USER {} ACCOUNT {}",
            target,
            if draft.account_locked { "LOCK" } else { "UNLOCK" }
        ));
    }

    // 服务器权限差量。
    let mut current_granted: BTreeSet<String> = BTreeSet::new();
    let mut current_grant_option = false;
    for privilege in &current.server_privileges {
        if privilege_key(&privilege.name) == "GRANT_OPTION" {
            current_grant_option = privilege.granted;
        } else if privilege.granted {
            current_granted.insert(privilege_key(&privilege.name));
        }
    }
    let mut desired_granted: BTreeSet<String> = BTreeSet::new();
    let mut desired_grant_option = false;
    for name in &draft.server_privileges {
        if privilege_key(name) == "GRANT_OPTION" {
            desired_grant_option = true;
        } else {
            desired_granted.insert(privilege_key(name));
        }
    }
    let grant: BTreeSet<String> = desired_granted.difference(&current_granted).cloned().collect();
    let revoke: BTreeSet<String> = current_granted.difference(&desired_granted).cloned().collect();
    if !grant.is_empty() {
        let list = grant.iter().map(|key| display_privilege(key).to_ascii_uppercase()).collect::<Vec<_>>().join(", ");
        let with_option = if desired_grant_option && !current_grant_option { " WITH GRANT OPTION" } else { "" };
        statements.push(format!("GRANT {} ON *.* TO {}{}", list, target, with_option));
    } else if desired_grant_option && !current_grant_option {
        statements.push(format!("GRANT USAGE ON *.* TO {} WITH GRANT OPTION", target));
    }
    if !revoke.is_empty() {
        let list = revoke.iter().map(|key| display_privilege(key).to_ascii_uppercase()).collect::<Vec<_>>().join(", ");
        statements.push(format!("REVOKE {} ON *.* FROM {}", list, target));
    }
    if current_grant_option && !desired_grant_option {
        statements.push(format!("REVOKE GRANT OPTION ON *.* FROM {}", target));
    }

    // 对象级权限差量。
    let mut current_objects: BTreeMap<(String, String), (BTreeSet<String>, bool)> = BTreeMap::new();
    for row in &current.object_privileges {
        let entry = current_objects.entry((row.database.clone(), row.name.clone())).or_default();
        entry.0 = row.privileges.iter().map(|name| privilege_key(name)).collect();
        entry.1 = row.grantable;
    }
    let mut desired_objects: BTreeMap<(String, String), (BTreeSet<String>, bool)> = BTreeMap::new();
    for row in &draft.object_grants {
        if row.database.trim().is_empty() {
            continue;
        }
        let name = if row.name.trim().is_empty() { "*".to_string() } else { row.name.trim().to_string() };
        let entry = desired_objects.entry((row.database.trim().to_string(), name)).or_default();
        entry.0 = row.privileges.iter().map(|name| privilege_key(name)).collect();
    }
    for ((database, name), (desired, _)) in &desired_objects {
        let reference = object_ref(database, name);
        let (current_set, current_grantable) = current_objects
            .get(&(database.clone(), name.clone()))
            .cloned()
            .unwrap_or_default();
        let granted: BTreeSet<String> = desired.difference(&current_set).cloned().collect();
        let revoked: BTreeSet<String> = current_set.difference(desired).cloned().collect();
        if !granted.is_empty() {
            let list = granted.iter().map(|key| display_privilege(key).to_ascii_uppercase()).collect::<Vec<_>>().join(", ");
            let with_option = if !current_grantable { "" } else { "" };
            statements.push(format!("GRANT {} ON {} TO {}{}", list, reference, target, with_option));
        }
        if !revoked.is_empty() {
            let list = revoked.iter().map(|key| display_privilege(key).to_ascii_uppercase()).collect::<Vec<_>>().join(", ");
            statements.push(format!("REVOKE {} ON {} FROM {}", list, reference, target));
        }
    }
    for ((database, name), (current_set, _)) in &current_objects {
        if desired_objects.contains_key(&(database.clone(), name.clone())) {
            continue;
        }
        if current_set.is_empty() {
            continue;
        }
        let list = current_set.iter().map(|key| display_privilege(key).to_ascii_uppercase()).collect::<Vec<_>>().join(", ");
        statements.push(format!("REVOKE {} ON {} FROM {}", list, object_ref(database, name), target));
    }

    // 角色差量（成员属于）。
    let current_roles: BTreeSet<String> = current.roles.iter().map(|role| format!("{}@{}", role.role, role.host)).collect();
    let desired_roles: BTreeSet<String> = draft.roles.iter().map(|role| format!("{}@{}", role.role, role.host)).collect();
    for role in desired_roles.difference(&current_roles) {
        let parts: Vec<&str> = role.rsplitn(2, '@').collect();
        statements.push(format!("GRANT {} TO {}", principal(parts[1], parts[0]), target));
    }
    for role in current_roles.difference(&desired_roles) {
        let parts: Vec<&str> = role.rsplitn(2, '@').collect();
        statements.push(format!("REVOKE {} FROM {}", principal(parts[1], parts[0]), target));
    }

    if statements.is_empty() {
        warnings.push("没有需要应用的变更。".to_string());
    }
    DbUserPlan { statements, warnings }
}

// ------------------------------------------------------------------ 命令

/// 读取用户完整详情（常规 / 高级 / 权限 / 成员属于）。
#[tauri::command]
pub async fn read_db_user(
    state: State<'_, AppState>,
    id: String,
    user: String,
    host: String,
) -> CmdResult<DbUserDetail> {
    let _activity_guard = state.activity.enter()?;
    if !kind_of(&state, &id)?.is_mysql_family() {
        return Err(AppError::new("仅 MySQL 协议族数据库支持用户编辑"));
    }
    let db = ensure_db(&state, &id).await?;
    let Db::Mysql(pool) = db.as_ref() else {
        return Err(AppError::new("仅 MySQL 协议族数据库支持用户编辑"));
    };
    load_detail(pool, &user.trim(), &host.trim()).await.map_err(AppError::from)
}

/// 预览编辑用户将执行的 SQL。
#[tauri::command]
pub async fn preview_db_user_changes(
    state: State<'_, AppState>,
    id: String,
    draft: DbUserDraft,
) -> CmdResult<DbUserPlan> {
    let _activity_guard = state.activity.enter()?;
    if !kind_of(&state, &id)?.is_mysql_family() {
        return Err(AppError::new("仅 MySQL 协议族数据库支持用户编辑"));
    }
    let db = ensure_db(&state, &id).await?;
    let Db::Mysql(pool) = db.as_ref() else {
        return Err(AppError::new("仅 MySQL 协议族数据库支持用户编辑"));
    };
    let current = load_detail(pool, &draft.user.trim(), &draft.host.trim())
        .await
        .map_err(AppError::from)?;
    Ok(build_user_plan(&current, &draft))
}

/// 应用编辑用户的变更（逐条执行；失败时报告已执行条数）。
#[tauri::command]
pub async fn apply_db_user_changes(
    state: State<'_, AppState>,
    id: String,
    draft: DbUserDraft,
) -> CmdResult<DbUserApplyResult> {
    let _activity_guard = state.activity.enter()?;
    let _manual_guard = state.manual_transactions.ordinary(&[&id]).await?;
    require_writable_connection(&state, &id)?;
    let kind = kind_of(&state, &id)?;
    if !kind.is_mysql_family() {
        return Err(AppError::new("仅 MySQL 协议族数据库支持用户编辑"));
    }
    let db = ensure_db(&state, &id).await?;
    let Db::Mysql(pool) = db.as_ref() else {
        return Err(AppError::new("仅 MySQL 协议族数据库支持用户编辑"));
    };
    let current = load_detail(pool, &draft.user.trim(), &draft.host.trim())
        .await
        .map_err(AppError::from)?;
    let plan = build_user_plan(&current, &draft);
    let mut executed = 0usize;
    for statement in &plan.statements {
        crate::schema::exec_affected(db.as_ref(), &kind, statement)
            .await
            .map_err(|error| {
                AppError::coded(
                    "DB_USER_APPLY_FAILED",
                    format!("第 {} 条语句执行失败（已执行 {} 条）：{}", executed + 1, executed, error),
                )
            })?;
        executed += 1;
    }
    crate::storage::log_line(
        "user",
        &format!("编辑用户 {}@{}：执行 {} 条语句", draft.user, draft.host, executed),
    );
    Ok(DbUserApplyResult { statements: plan.statements, executed })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn privilege(name: &str, granted: bool) -> DbUserPrivilege {
        DbUserPrivilege { name: name.to_string(), granted, grantable: false }
    }

    fn detail() -> DbUserDetail {
        DbUserDetail {
            account: DbUserAccount {
                user: "app".to_string(),
                host: "%".to_string(),
                plugin: "caching_sha2_password".to_string(),
                password_expired: false,
                password_lifetime_days: None,
                account_locked: false,
                max_questions: 0,
                max_updates: 0,
                max_connections: 0,
                max_user_connections: 0,
                ssl_type: String::new(),
                ssl_cipher: String::new(),
                x509_issuer: String::new(),
                x509_subject: String::new(),
            },
            server_privileges: vec![privilege("Select", true), privilege("Grant Option", false)],
            object_privileges: vec![DbUserObjectPrivilege {
                database: "shop".to_string(),
                name: "*".to_string(),
                privileges: vec!["Select".to_string()],
                grantable: false,
            }],
            roles: vec![DbUserRole { role: "readonly".to_string(), host: "%".to_string() }],
            members: Vec::new(),
            supports_roles: true,
            warnings: Vec::new(),
        }
    }

    fn draft() -> DbUserDraft {
        DbUserDraft {
            user: "app".to_string(),
            host: "%".to_string(),
            new_user: "app".to_string(),
            new_host: "%".to_string(),
            password: None,
            plugin: "caching_sha2_password".to_string(),
            password_expiry: "default".to_string(),
            password_lifetime_days: 0,
            account_locked: false,
            max_questions: 0,
            max_updates: 0,
            max_connections: 0,
            max_user_connections: 0,
            ssl_type: String::new(),
            ssl_cipher: String::new(),
            x509_issuer: String::new(),
            x509_subject: String::new(),
            server_privileges: vec!["Select".to_string(), "Insert".to_string()],
            object_grants: vec![DbUserGrantDraft {
                database: "shop".to_string(),
                name: "*".to_string(),
                privileges: vec!["Select".to_string(), "Update".to_string()],
            }],
            roles: vec![
                DbUserRole { role: "readonly".to_string(), host: "%".to_string() },
                DbUserRole { role: "writer".to_string(), host: "%".to_string() },
            ],
        }
    }

    #[test]
    fn diff_generates_grant_and_revoke() {
        let plan = build_user_plan(&detail(), &draft());
        assert!(plan.statements.iter().any(|item| item == "GRANT INSERT ON *.* TO 'app'@'%'"));
        assert!(plan.statements.iter().any(|item| item == "GRANT UPDATE ON \u{0060}shop\u{0060}.* TO 'app'@'%'"));
        // MySQL role accounts use the same quoted user@host syntax as principals.
        assert!(plan.statements.iter().any(|item| item == "GRANT 'writer'@'%' TO 'app'@'%'"));
    }

    #[test]
    fn rename_password_ssl_lock_and_expiry() {
        let mut desired = draft();
        desired.new_user = "newapp".to_string();
        desired.password = Some("p'w".to_string());
        desired.ssl_type = "ANY".to_string();
        desired.account_locked = true;
        desired.password_expiry = "never".to_string();
        let plan = build_user_plan(&detail(), &desired);
        assert!(plan.statements[0].starts_with("RENAME USER 'app'@'%' TO 'newapp'@'%'"));
        assert!(plan.statements.iter().any(|item| item.contains("IDENTIFIED WITH") && item.contains("'p''w'")));
        assert!(plan.statements.iter().any(|item| item.ends_with("REQUIRE SSL")));
        assert!(plan.statements.iter().any(|item| item.ends_with("ACCOUNT LOCK")));
        assert!(plan.statements.iter().any(|item| item.ends_with("PASSWORD EXPIRE NEVER")));
    }

    #[test]
    fn unchanged_state_produces_no_statements() {
        let mut desired = draft();
        desired.server_privileges = vec!["Select".to_string()];
        desired.object_grants = vec![DbUserGrantDraft {
            database: "shop".to_string(),
            name: "*".to_string(),
            privileges: vec!["Select".to_string()],
        }];
        desired.roles = vec![DbUserRole { role: "readonly".to_string(), host: "%".to_string() }];
        let plan = build_user_plan(&detail(), &desired);
        assert!(plan.statements.is_empty(), "statements: {:?}", plan.statements);
    }
}

