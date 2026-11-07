//! 数据库访问层：统一封装 SQLite / MySQL(MariaDB) / PostgreSQL / SQL Server /
//! Redis / MongoDB / ClickHouse / HTTP 类数据源 / ODBC。
//! 所有连接以 `kind://...` 的 URL 形式交给 sqlx，SQLite 走 rusqlite。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Column, Executor, Row, TypeInfo};

use crate::models::{DbKind, Protocol, ConnectionConfig, ConnectionRequest};

/// 一个活跃的数据库连接。
pub enum Db {
    Sqlite(sqlx::SqlitePool),
    Mysql(sqlx::MySqlPool),
    Postgres(sqlx::PgPool),
    /// SQL Server 原生客户端（tiberius）。
    Mssql(crate::mssql::MssqlDb),
    /// Redis 原生客户端（redis-rs）。
    Redis(crate::redis_db::RedisDb),
    /// MongoDB 原生客户端。
    Mongo(crate::mongo_db::MongoDb),
    /// ClickHouse 原生 HTTP 客户端。
    ClickHouse(crate::clickhouse::ClickHouseClient),
    /// HTTP REST 类数据源（检索 / 向量 / 时序 / MQ / 注册配置）。
    Http(crate::http_db::HttpDb),
    /// ODBC 连接（达梦 / Hive / Oracle 等，需安装厂商 ODBC 驱动）。
    Odbc(Arc<crate::odbc_db::OdbcDb>),
}

/// 全局连接池表：connection_id -> Arc<Db>。
///
/// 使用 Arc 是为了让命令可以先把连接克隆出来再释放锁，
/// 这样 await 期间不会持有锁（也不会跨线程持有 guard）。
pub type DbRegistry = parking_lot::RwLock<HashMap<String, Arc<Db>>>;

/// 连接附加参数：`k=v;k=v` 或 `k=v&k=v` 形式。
pub fn extra_params(params: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for pair in params.split(['&', ';']) {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        if let Some((k, v)) = pair.split_once('=') {
            let (k, v) = (k.trim().to_string(), v.trim().to_string());
            if !k.is_empty() {
                out.push((k, v));
            }
        }
    }
    out
}

/// 打开 MySQL 连接：显式传递用户名与密码（此前版本密码从未发送，导致所有
/// 需要认证的服务器一律 Access denied）。
async fn open_mysql(req: &ConnectionRequest, timeout: Duration) -> Result<Db> {
    let port = if req.port == 0 { 3306 } else { req.port };
    let user = if req.user.is_empty() { "root" } else { req.user.as_str() };
    let mut opts = MySqlConnectOptions::new()
        .host(req.host.as_str())
        .port(port)
        .username(user)
        .password(req.password.as_str())
        .charset("utf8mb4")
        .collation("utf8mb4_general_ci");
    // 不选库连接时保持会话默认库为空，避免强行落到 mysql 系统库。
    if !req.database.is_empty() {
        opts = opts.database(req.database.as_str());
    }
    for (k, v) in extra_params(&req.params) {
        let kl = k.to_ascii_lowercase();
        match kl.as_str() {
            "ssl-mode" | "sslmode" => {
                opts = opts.ssl_mode(match v.as_str() {
                    "disabled" | "disable" | "none" => sqlx::mysql::MySqlSslMode::Disabled,
                    "preferred" => sqlx::mysql::MySqlSslMode::Preferred,
                    "required" | "require" => sqlx::mysql::MySqlSslMode::Required,
                    "verify_ca" | "verify-ca" => sqlx::mysql::MySqlSslMode::VerifyCa,
                    "verify_identity" | "verify-identity" | "verify-full" => {
                        sqlx::mysql::MySqlSslMode::VerifyIdentity
                    }
                    other => return Err(anyhow!("未知的 ssl-mode: {}", other)),
                });
            }
            _ => {
                // 其它连接参数忽略（不支持 unix socket 等高级选项）。
            }
        }
    }
    let pool = MySqlPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(timeout)
        .after_connect(|conn, _| {
            Box::pin(async move {
                // 关闭反斜杠转义：统一以 '' 转义字符串字面量（与 PG/SQLite 一致）。
                sqlx::query(
                    "SET sql_mode=CONCAT(@@session.sql_mode, ',NO_BACKSLASH_ESCAPES')",
                )
                .execute(conn)
                .await?;
                Ok(())
            })
        })
        .connect_with(opts)
        .await
        .with_context(|| format!("无法连接到 {} 服务器 {}:{}", req.kind.label(), req.host, port))?;
    Ok(Db::Mysql(pool))
}

/// 打开 PostgreSQL 连接：显式传递用户名与密码。
async fn open_pg(req: &ConnectionRequest, timeout: Duration) -> Result<Db> {
    let port = if req.port == 0 { 5432 } else { req.port };
    let user = if req.user.is_empty() { "postgres" } else { req.user.as_str() };
    let database = if req.database.is_empty() { "postgres" } else { req.database.as_str() };
    let mut opts = PgConnectOptions::new()
        .host(req.host.as_str())
        .port(port)
        .username(user)
        .password(req.password.as_str())
        .application_name("navkit");
    if !database.is_empty() {
        opts = opts.database(database);
    }
    let mut extra_options: Vec<(String, String)> = Vec::new();
    for (k, v) in extra_params(&req.params) {
        let kl = k.to_ascii_lowercase();
        match kl.as_str() {
            "sslmode" | "ssl-mode" => {
                opts = opts.ssl_mode(match v.as_str() {
                    "disable" | "disabled" | "none" => sqlx::postgres::PgSslMode::Disable,
                    "prefer" | "preferred" => sqlx::postgres::PgSslMode::Prefer,
                    "require" | "required" => sqlx::postgres::PgSslMode::Require,
                    "verify-ca" | "verify_ca" => sqlx::postgres::PgSslMode::VerifyCa,
                    "verify-full" | "verify_identity" => sqlx::postgres::PgSslMode::VerifyFull,
                    other => return Err(anyhow!("未知的 sslmode: {}", other)),
                });
            }
            _ => extra_options.push((k, v)),
        }
    }
    for (k, v) in &extra_options {
        opts = opts.options([(k.as_str(), v.as_str())]);
    }
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(timeout)
        .connect_with(opts)
        .await
        .with_context(|| format!("无法连接到 {} 服务器 {}:{}", req.kind.label(), req.host, port))?;
    Ok(Db::Postgres(pool))
}

/// 由配置生成 SQLite 文件路径。
fn sqlite_file(req: &ConnectionRequest) -> Result<String> {
    if req.file.trim().is_empty() {
        return Err(anyhow!("SQLite 连接必须指定数据库文件"));
    }
    Ok(req.file.clone())
}

/// 打开一个连接（不放入注册表）。
///
/// 按数据源协议族选择驱动：MySQL / PostgreSQL 协议族由 sqlx 驱动；
/// SQL Server / Redis / MongoDB 为原生客户端；ClickHouse 走 HTTP；
/// HTTP REST 类由 http_db 按适配器分发；其余走系统 ODBC。
pub async fn open(req: &ConnectionRequest) -> Result<Db> {
    let timeout = Duration::from_secs(req.timeout_secs.max(1));
    if req.kind.is_planned() {
        return Err(anyhow!(
            "{} 的适配器尚未提供（{}）",
            req.kind.label(),
            req.kind
                .meta()
                .map(|m| m.note)
                .unwrap_or("该类型暂未支持")
        ));
    }
    // New driver-free REST defaults retain every explicit legacy JDBC/ODBC binding.
    if matches!(req.kind.id(), "salesforce" | "hbase") {
        if !explicit_legacy_binding(&req.params) {
            return Ok(Db::Http(crate::native_db::connect(req).await?));
        }
        return open_odbc_or_jdbc(req).await;
    }
    match req.kind.protocol() {
        Protocol::Sqlite => {
            let file = sqlite_file(req)?;
            let opts = SqliteConnectOptions::new()
                .filename(&file)
                .create_if_missing(true)
                .busy_timeout(timeout)
                .foreign_keys(true);
            let pool = SqlitePoolOptions::new()
                .max_connections(4)
                .acquire_timeout(timeout)
                .connect_with(opts)
                .await
                .with_context(|| format!("无法打开 SQLite 文件: {}", file))?;
            Ok(Db::Sqlite(pool))
        }
        Protocol::Mysql => open_mysql(req, timeout).await,
        Protocol::Postgres => open_pg(req, timeout).await,
        Protocol::Mssql => {
            let client = crate::mssql::MssqlDb::connect(req.kind.clone(), req).await?;
            Ok(Db::Mssql(client))
        }
        Protocol::Redis => {
            let client = crate::redis_db::RedisDb::connect(req.kind.clone(), req).await?;
            Ok(Db::Redis(client))
        }
        Protocol::Mongo => {
            let client = crate::mongo_db::MongoDb::connect(req.kind.clone(), req).await?;
            Ok(Db::Mongo(client))
        }
        Protocol::Clickhouse => {
            let client = crate::clickhouse::ClickHouseClient::connect(req)?;
            // 真实建连验证
            client
                .scalar("SELECT 1")
                .await
                .with_context(|| format!("无法连接到 {} 服务器", req.kind.label()))?;
            Ok(Db::ClickHouse(client))
        }
        Protocol::Http => {
            let client = crate::http_db::HttpDb::connect(req.kind.clone(), req).await?;
            Ok(Db::Http(client))
        }
        Protocol::Native => Ok(Db::Http(crate::native_db::connect(req).await?)),
        Protocol::Odbc => open_odbc_or_jdbc(req).await,
    }
}

pub(crate) fn explicit_legacy_binding(params: &str) -> bool {
    let raw = params.trim();
    raw.starts_with("@navkit-driver:") || raw.starts_with("@navkit-driver-version:")
        || (!raw.starts_with('{') && raw.split(';').any(|part| part.split_once('=').is_some_and(|(key, _)| {
            matches!(key.trim().to_ascii_lowercase().as_str(), "driver" | "dsn")
        })))
}

async fn open_odbc_or_jdbc(req: &ConnectionRequest) -> Result<Db> {
    // Explicit custom bindings or a fully verified product-specific managed JDBC.
    // No generic compatibility guessing and no fallback after a managed failure.
    if let Some(res) = crate::driver_manager::resolve_jdbc_driver(req)? {
        let jdbc = crate::jdbc_db::JdbcDb::connect(req, &res.target, &res.url, &res.driver_class)
            .await.with_context(|| format!("无法通过 JDBC 连接到 {} 服务器", req.kind.label()))?;
        return Ok(Db::Http(crate::http_db::HttpDb::from_adapter(req, Box::new(jdbc)).await?));
    }
    let odb = crate::odbc_db::OdbcDb::connect(req.kind.clone(), req)
        .with_context(|| format!("无法连接到 {} 服务器", req.kind.label()))?;
    Ok(Db::Odbc(Arc::new(odb)))
}

impl Db {
    /// 通用文本行查询（ClickHouse / ODBC / SQL Server 用；sqlx 变体不支持）。
    pub async fn generic_rows(
        &self,
        sql: &str,
    ) -> Result<(Vec<String>, Vec<Vec<Option<String>>>)> {
        match self {
            Db::ClickHouse(c) => {
                let r = c.query_rows(sql).await?;
                Ok((r.columns, r.rows))
            }
            Db::Odbc(o) => {
                let r = o.query_rows(sql).await?;
                Ok((r.columns, r.rows))
            }
            Db::Mssql(c) => {
                let r = c.query_rows(sql).await?;
                Ok((r.columns, r.rows))
            }
            _ => Err(anyhow!("该连接类型不支持通用查询")),
        }
    }

    /// 通用语句执行（ClickHouse / ODBC / SQL Server 用）。
    pub async fn generic_exec(&self, sql: &str) -> Result<u64> {
        match self {
            Db::ClickHouse(c) => c.execute(sql).await,
            Db::Odbc(o) => o.execute(sql).await,
            Db::Mssql(c) => c.execute(sql).await,
            _ => Err(anyhow!("该连接类型不支持通用执行")),
        }
    }

    /// 单值查询（ClickHouse / ODBC / SQL Server 用）。
    pub async fn generic_scalar(&self, sql: &str) -> Result<Option<String>> {
        match self {
            Db::ClickHouse(c) => c.scalar(sql).await,
            Db::Odbc(o) => o.scalar(sql).await,
            Db::Mssql(c) => c.scalar(sql).await,
            _ => Err(anyhow!("该连接类型不支持单值查询")),
        }
    }
}

/// 由已保存的连接配置构造请求。
pub fn request_from_config(cfg: &ConnectionConfig, password: &str) -> ConnectionRequest {
    ConnectionRequest {
        kind: cfg.kind.clone(),
        host: cfg.host.clone(),
        port: cfg.port,
        user: cfg.user.clone(),
        password: password.to_string(),
        database: cfg.database.clone(),
        file: cfg.file.clone(),
        params: cfg.params.clone(),
        timeout_secs: 30,
    }
}

/// 打开并登记一个连接。
pub async fn open_and_register(
    registry: &DbRegistry,
    id: &str,
    req: &ConnectionRequest,
) -> Result<()> {
    let db = open(req).await?;
    registry.write().insert(id.to_string(), Arc::new(db));
    Ok(())
}

/// 关闭并注销一个连接。
pub fn close(registry: &DbRegistry, id: &str) {
    registry.write().remove(id);
}

/// 关闭一个连接及其全部按库派生池（key = `{id}` 或 `{id}@{db}`）。
pub fn close_with_derived(registry: &DbRegistry, id: &str) {
    let prefix = format!("{}@", id);
    registry
        .write()
        .retain(|k, _| !(k == id || k.starts_with(&prefix)));
}

/// 关闭全部连接。
pub fn close_all(registry: &DbRegistry) {
    registry.write().clear();
}

/// 从注册表取出连接的共享引用（克隆后即释放锁）。
pub fn get_db(registry: &DbRegistry, id: &str) -> Result<Arc<Db>> {
    registry
        .read()
        .get(id)
        .cloned()
        .ok_or_else(|| anyhow!("连接未打开或已关闭"))
}

/// 从 sqlx 行中取出所有列的值（按列名通用解码）。
fn row_values(
    db: &Db,
    kind: &DbKind,
    columns: &[sqlx::mysql::MySqlColumn],
    row: &sqlx::mysql::MySqlRow,
) -> Vec<Option<String>> {
    let _ = (db, kind, columns);
    crate::values::my_row_values(row, columns.len())
}

/// 标识符引用（防注入）。
pub fn quote_ident(kind: &DbKind, name: &str) -> String {
    match kind.protocol() {
        crate::models::Protocol::Sqlite | crate::models::Protocol::Postgres => {
            format!("\"{}\"", name.replace('"', "\"\""))
        }
        crate::models::Protocol::Mysql | crate::models::Protocol::Clickhouse => {
            format!("`{}`", name.replace('`', "``"))
        }
        crate::models::Protocol::Mssql => format!("[{}]", name.replace(']', "]]")),
        // Redis / MongoDB / HTTP 类不走 SQL 标识符；ODBC 按方言退化
        crate::models::Protocol::Redis
        | crate::models::Protocol::Mongo
        | crate::models::Protocol::Http | crate::models::Protocol::Native => format!("\"{}\"", name.replace('"', "\"\"")),
        crate::models::Protocol::Odbc => match kind.id() {
            "hive" | "inceptor" | "argodb" => format!("`{}`", name.replace('`', "``")),
            "mysql" => format!("`{}`", name.replace('`', "``")),
            // Oracle/DB2/达梦 等以大写存储未加引号的对象名：同步建表统一大写，
            // 否则小写带引号对象在对象树里不可见。
            "oracle" | "dameng" | "oscar" | "sundb" | "yashandb" | "gbase" | "db2" | "informix" => {
                format!("\"{}\"", name.to_ascii_uppercase().replace('"', "\"\""))
            }
            _ => format!("\"{}\"", name.replace('"', "\"\"")),
        },
    }
}

/// 字符串字面量转义。
pub fn quote_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// 表名全名（schema.table 或 table）。
///
/// SQLite 的文件连接只有一个库（main），且本应用不使用 ATTACH，
/// 因此这里对 SQLite 一律忽略 schema —— 否则 `"demo"."t"` 会报 no such table。
pub fn qualified(kind: &DbKind, schema: &str, table: &str) -> String {
    if kind.is_sqlite() || schema.is_empty() {
        quote_ident(&kind, table)
    } else {
        format!(
            "{}.{}",
            quote_ident(&kind, schema),
            quote_ident(&kind, table)
        )
    }
}


/// 用列名 + 类型构造 ResultColumn。
fn make_columns(names: &[String], types: &[String]) -> Vec<crate::models::ResultColumn> {
    names
        .iter()
        .zip(types.iter())
        .map(|(n, t)| crate::models::ResultColumn {
            name: n.clone(),
            data_type: t.clone(),
            nullable: true,
            primary_key: false,
            auto_increment: false,
        })
        .collect()
}

/// 空结果集：通过 describe 取列名，保证网格始终有表头。
fn empty_set(names: Vec<String>, types: Vec<String>) -> crate::models::ResultSet {
    crate::models::ResultSet {
        columns: make_columns(&names, &types),
        rows: Vec::new(),
        affected: 0,
        elapsed_ms: 0,
        ok: true,
        message: String::new(),
        has_rows: true,
        primary_keys: Vec::new(),
        total: 0,
    }
}

/// SQLite 空结果集兜底：PRAGMA 无 describe，直接空列。
async fn sqlite_describe(pool: &sqlx::SqlitePool, sql: &str) -> (Vec<String>, Vec<String>) {
    match pool.describe(sql).await {
        Ok(d) => (
            d.columns().iter().map(|c| c.name().to_string()).collect(),
            d.columns()
                .iter()
                .map(|c| c.type_info().name().to_string())
                .collect(),
        ),
        Err(_) => (Vec::new(), Vec::new()),
    }
}

async fn mysql_describe(
    conn: &mut sqlx::mysql::MySqlConnection,
    sql: &str,
) -> (Vec<String>, Vec<String>) {
    match conn.describe(sql).await {
        Ok(d) => (
            d.columns().iter().map(|c| c.name().to_string()).collect(),
            d.columns()
                .iter()
                .map(|c| c.type_info().name().to_string())
                .collect(),
        ),
        Err(_) => (Vec::new(), Vec::new()),
    }
}

async fn pg_describe(
    conn: &mut sqlx::postgres::PgConnection,
    sql: &str,
) -> (Vec<String>, Vec<String>) {
    match conn.describe(sql).await {
        Ok(d) => (
            d.columns().iter().map(|c| c.name().to_string()).collect(),
            d.columns()
                .iter()
                .map(|c| c.type_info().name().to_string())
                .collect(),
        ),
        Err(_) => (Vec::new(), Vec::new()),
    }
}
pub async fn execute(
    registry: &DbRegistry,
    id: &str,
    sql: &str,
    kind: DbKind,
) -> Result<crate::models::MultiResult> {
    let start = std::time::Instant::now();
    let db = get_db(registry, id)?;
    let db = db.as_ref();
    let mut sets = Vec::new();
    let mut messages = Vec::new();

    match db {
        Db::Sqlite(pool) => {
            // SQLite: 逐条执行语句，收集结果。
            let mut stmts: Vec<String> = Vec::new();
            for part in split_sqlite(sql) {
                stmts.push(part);
            }
            for stmt in stmts {
                let trimmed = stmt.trim();
                if !is_executable(trimmed) {
                    continue; // 纯注释块不执行（否则 MySQL 报 ER_EMPTY_QUERY）
                }
                if is_query(trimmed) {
                    let rows = sqlx::query(trimmed).fetch_all(pool).await?;
                    let (names, types, out_rows) = if let Some(first) = rows.first() {
                        let cols = first.columns();
                        let names: Vec<String> =
                            cols.iter().map(|c| c.name().to_string()).collect();
                        let types: Vec<String> = cols
                            .iter()
                            .map(|c| c.type_info().name().to_string())
                            .collect();
                        let mut out = Vec::with_capacity(rows.len());
                        for r in &rows {
                            out.push(crate::values::sqlite_row_values(r, names.len()));
                        }
                        (names, types, out)
                    } else {
                        // 0 行结果：describe 取列名，网格仍应有表头。
                        let (n, t) = sqlite_describe(pool, trimmed).await;
                        (n, t, Vec::new())
                    };
                    let n = out_rows.len() as u64;
                    sets.push(crate::models::ResultSet {
                        columns: make_columns(&names, &types),
                        rows: out_rows,
                        affected: 0,
                        elapsed_ms: 0,
                        ok: true,
                        message: String::new(),
                        has_rows: true,
                        primary_keys: Vec::new(),
                        total: n,
                    });
                } else {
                    let res = sqlx::query(trimmed).execute(pool).await?;
                    messages.push(format!("{} 行受影响", res.rows_affected()));
                    sets.push(crate::models::ResultSet {
                        columns: Vec::new(),
                        rows: Vec::new(),
                        affected: res.rows_affected(),
                        elapsed_ms: 0,
                        ok: true,
                        message: String::new(),
                        has_rows: false,
                        primary_keys: Vec::new(),
                        total: 0,
                    });
                }
            }
        }
        Db::Mysql(pool) => {
            let mut conn = pool.acquire().await?;
            for stmt in split_statements(sql) {
                let trimmed = stmt.trim();
                if !is_executable(trimmed) {
                    continue; // 纯注释块不执行（否则 MySQL 报 ER_EMPTY_QUERY）
                }
                if is_query(trimmed) {
                    let rows = sqlx::query(trimmed).fetch_all(&mut *conn).await?;
                    if let Some(first) = rows.first() {
                        let cols = first.columns();
                        let names: Vec<String> = cols
                            .iter()
                            .map(|c| c.name().to_string())
                            .collect();
                        let types: Vec<String> = cols
                            .iter()
                            .map(|c| c.type_info().name().to_string())
                            .collect();
                        let mut out = Vec::with_capacity(rows.len());
                        for r in &rows {
                            out.push(row_values(db, &kind, &cols, r));
                        }
                        let n = out.len() as u64;
                        sets.push(crate::models::ResultSet {
                            columns: make_columns(&names, &types),
                            rows: out,
                            affected: 0,
                            elapsed_ms: 0,
                            ok: true,
                            message: String::new(),
                            has_rows: true,
                            primary_keys: Vec::new(),
                            total: n,
                        });
                    } else {
                        // 0 行结果：describe 取列名，结果页不再凭空消失。
                        let (n, t) = mysql_describe(&mut conn, trimmed).await;
                        sets.push(empty_set(n, t));
                    }
                } else {
                    let res = sqlx::query(trimmed).execute(&mut *conn).await?;
                    messages.push(format!("{} 行受影响", res.rows_affected()));
                    sets.push(crate::models::ResultSet {
                        columns: Vec::new(),
                        rows: Vec::new(),
                        affected: res.rows_affected(),
                        elapsed_ms: 0,
                        ok: true,
                        message: String::new(),
                        has_rows: false,
                        primary_keys: Vec::new(),
                        total: 0,
                    });
                }
            }
        }
        Db::Postgres(pool) => {
            let mut conn = pool.acquire().await?;
            for stmt in split_statements(sql) {
                let trimmed = stmt.trim();
                if !is_executable(trimmed) {
                    continue; // 纯注释块不执行（否则 MySQL 报 ER_EMPTY_QUERY）
                }
                if is_query(trimmed) {
                    let rows = sqlx::query(trimmed).fetch_all(&mut *conn).await?;
                    if let Some(first) = rows.first() {
                        let cols = first.columns();
                        let names: Vec<String> = cols
                            .iter()
                            .map(|c| c.name().to_string())
                            .collect();
                        let types: Vec<String> = cols
                            .iter()
                            .map(|c| c.type_info().name().to_string())
                            .collect();
                        let mut out = Vec::with_capacity(rows.len());
                        for r in &rows {
                            let vals: Vec<Option<String>> =
                                crate::values::pg_row_values(r, cols.len());
                            out.push(vals);
                        }
                        let n = out.len() as u64;
                        sets.push(crate::models::ResultSet {
                            columns: make_columns(&names, &types),
                            rows: out,
                            affected: 0,
                            elapsed_ms: 0,
                            ok: true,
                            message: String::new(),
                            has_rows: true,
                            primary_keys: Vec::new(),
                            total: n,
                        });
                    } else {
                        // 0 行结果：describe 取列名，结果页不再凭空消失。
                        let (n, t) = pg_describe(&mut conn, trimmed).await;
                        sets.push(empty_set(n, t));
                    }
                } else {
                    let res = sqlx::query(trimmed).execute(&mut *conn).await?;
                    messages.push(format!("{} 行受影响", res.rows_affected()));
                    sets.push(crate::models::ResultSet {
                        columns: Vec::new(),
                        rows: Vec::new(),
                        affected: res.rows_affected(),
                        elapsed_ms: 0,
                        ok: true,
                        message: String::new(),
                        has_rows: false,
                        primary_keys: Vec::new(),
                        total: 0,
                    });
                }
            }
        }
        // 通用 SQL 数据源：ClickHouse / ODBC / SQL Server 经 generic_* 文本化执行
        Db::Mssql(_) | Db::ClickHouse(_) | Db::Odbc(_) => {
            for stmt in split_statements(sql) {
                let trimmed = stmt.trim();
                if !is_executable(trimmed) {
                    continue;
                }
                if is_query(trimmed) {
                    let (names, rows) = db.generic_rows(trimmed).await?;
                    let n = rows.len() as u64;
                    sets.push(crate::models::ResultSet {
                        columns: names
                            .into_iter()
                            .map(|c| crate::models::ResultColumn {
                                name: c,
                                data_type: String::new(),
                                nullable: true,
                                primary_key: false,
                                auto_increment: false,
                            })
                            .collect(),
                        rows,
                        affected: 0,
                        elapsed_ms: 0,
                        ok: true,
                        message: String::new(),
                        has_rows: true,
                        primary_keys: Vec::new(),
                        total: n,
                    });
                } else {
                    let n = db.generic_exec(trimmed).await?;
                    messages.push(format!("{} 行受影响", n));
                    sets.push(crate::models::ResultSet {
                        columns: Vec::new(),
                        rows: Vec::new(),
                        affected: n,
                        elapsed_ms: 0,
                        ok: true,
                        message: String::new(),
                        has_rows: false,
                        primary_keys: Vec::new(),
                        total: 0,
                    });
                }
            }
        }
        // HTTP REST 类：语句交给适配器执行（ES DSL / Mango / Cypher / SQL 等）
        Db::Http(h) => {
            // JSON/DSL/protocol commands are not SQL: do not reinterpret escaped quotes or semicolons.
            let stmts = http_command_statements(h.kind.id(), sql);
            for stmt in stmts {
                let trimmed = stmt.trim();
                if !is_executable(trimmed) {
                    continue;
                }
                let r = h.run(&h.database, "", trimmed).await?;
                let n = r.rows.len() as u64;
                sets.push(crate::models::ResultSet {
                    columns: r
                        .columns
                        .into_iter()
                        .map(|c| crate::models::ResultColumn {
                            name: c,
                            data_type: "string".into(),
                            nullable: true,
                            primary_key: false,
                            auto_increment: false,
                        })
                        .collect(),
                    rows: r.rows,
                    affected: 0,
                    elapsed_ms: 0,
                    ok: true,
                    message: String::new(),
                    has_rows: true,
                    primary_keys: Vec::new(),
                    total: n,
                });
            }
        }
        // Redis：每条语句为一条命令（如 GET key / KEYS *）
        Db::Redis(r) => {
            for stmt in split_protocol_commands(sql) {
                let trimmed = stmt.trim();
                if !is_executable(trimmed) {
                    continue;
                }
                let rows = r.run_command(r.database, trimmed).await?;
                let n = rows.len() as u64;
                sets.push(crate::models::ResultSet {
                    columns: vec![crate::models::ResultColumn {
                        name: "result".into(),
                        data_type: "string".into(),
                        nullable: true,
                        primary_key: false,
                        auto_increment: false,
                    }],
                    rows,
                    affected: 0,
                    elapsed_ms: 0,
                    ok: true,
                    message: String::new(),
                    has_rows: true,
                    primary_keys: Vec::new(),
                    total: n,
                });
            }
        }
        // MongoDB：`集合 {JSON条件}` 执行 find；`count 集合 {JSON条件}` 计数
        Db::Mongo(m) => {
            if m.database.is_empty() {
                return Err(anyhow!(
                    "MongoDB 需要先在运行条选择数据库才能执行查询"
                ));
            }
            for stmt in split_protocol_commands(sql) {
                let trimmed = stmt.trim();
                if !is_executable(trimmed) {
                    continue;
                }
                let (first, rest) = match trimmed.split_once(char::is_whitespace) {
                    Some((a, b)) => (a, b.trim()),
                    None => (trimmed, ""),
                };
                if first.eq_ignore_ascii_case("count") {
                    let (coll, filter) = mongo_split(rest);
                    let n = m.count(&m.database, &coll, filter).await?;
                    messages.push(format!("{} 条文档", n));
                    sets.push(crate::models::ResultSet {
                        columns: vec![crate::models::ResultColumn {
                            name: "count".into(),
                            data_type: "int".into(),
                            nullable: false,
                            primary_key: false,
                            auto_increment: false,
                        }],
                        rows: vec![vec![Some(n.to_string())]],
                        affected: 0,
                        elapsed_ms: 0,
                        ok: true,
                        message: String::new(),
                        has_rows: true,
                        primary_keys: Vec::new(),
                        total: 1,
                    });
                } else {
                    let (cols, rows) = m.find_docs(&m.database, first, rest, 0, 200).await?;
                    let n = rows.len() as u64;
                    sets.push(crate::models::ResultSet {
                        columns: cols
                            .into_iter()
                            .map(|c| crate::models::ResultColumn {
                                name: c,
                                data_type: "string".into(),
                                nullable: true,
                                primary_key: false,
                                auto_increment: false,
                            })
                            .collect(),
                        rows,
                        affected: 0,
                        elapsed_ms: 0,
                        ok: true,
                        message: String::new(),
                        has_rows: true,
                        primary_keys: Vec::new(),
                        total: n,
                    });
                }
            }
        }
    }

    Ok(crate::models::MultiResult {
        sets,
        messages,
        elapsed_ms: start.elapsed().as_millis() as u64,
    })
}


/// 剥离语句块开头的前导独立注释行（`--` 整行 / `/* ... */` 独立块），
/// 返回剩余 SQL；纯注释返回空串。不碰行内注释与字符串字面量。
pub fn strip_leading_comments(stmt: &str) -> String {
    let mut s = stmt.trim_start();
    loop {
        if s.starts_with("--") {
            s = s.split_once('\n').map(|(_, r)| r.trim_start()).unwrap_or("");
            continue;
        }
        if s.starts_with("/*") {
            match s.find("*/") {
                Some(i) => s = s[i + 2..].trim_start(),
                None => return String::new(),
            }
            continue;
        }
        break;
    }
    s.trim().to_string()
}

/// 判断一段语句剥离注释后是否还有可执行内容。
pub fn is_executable(stmt: &str) -> bool {
    !strip_leading_comments(stmt).is_empty()
}

/// 判断 SQL 是否返回数据。
///
/// 跳过前置空白、行/块注释与若干层括号后再看关键字，
/// 这样 `(SELECT 1)`、`/* 注释 */ SELECT 1` 也能被正确识别为查询。
pub fn is_query(sql: &str) -> bool {
    let mut s = sql.trim_start();
    loop {
        // 空白
        let before = s;
        s = s.trim_start();
        if s.is_empty() {
            return false;
        }
        // 行注释
        if s.starts_with("--") {
            s = s.split_once('\n').map(|(_, rest)| rest).unwrap_or("");
            if before == s {
                return false;
            }
            continue;
        }
        // 块注释
        if s.starts_with("/*") {
            s = match s.find("*/") {
                Some(i) => &s[i + 2..],
                None => "",
            };
            continue;
        }
        // 括号
        if s.starts_with('(') {
            s = &s[1..];
            continue;
        }
        break;
    }
    // 取首词（连续 ASCII 字母）比对关键字，兼容 SELECT*FROM 等紧凑写法。
    let word: String = s
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    matches!(
        word.as_str(),
        "SELECT" | "SHOW" | "DESCRIBE" | "DESC" | "EXPLAIN" | "PRAGMA" | "WITH" | "VALUES"
            | "TABLE" | "CALL"
    )
}

/// Only actual SQL adapters consume SQL statement batches. JSON/DSL text is one command.
fn http_command_statements(kind: &str, command: &str) -> Vec<String> {
    if matches!(kind, "h2" | "druid" | "rqlite" | "cassandra" | "nebulagraph" | "tdengine" | "iotdb" | "influxdb" | "turso" | "d1" | "solr") {
        split_statements(command)
    } else {
        vec![command.to_string()]
    }
}

/// Redis CLI / Mongo JSON use backslash escapes, unlike NO_BACKSLASH_ESCAPES SQL.
/// Split only outside quoted text and balanced JSON; preserve malformed input for real parsers.
fn split_protocol_commands(command: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut brackets = Vec::new();
    for ch in command.chars() {
        if let Some(q) = quote {
            current.push(ch);
            if escaped { escaped = false; }
            else if ch == '\\' { escaped = true; }
            else if ch == q { quote = None; }
            continue;
        }
        match ch {
            '\'' | '"' => { quote = Some(ch); current.push(ch); }
            '{' | '[' => { brackets.push(ch); current.push(ch); }
            '}' | ']' => {
                let expected = if ch == '}' { '{' } else { '[' };
                if brackets.last() == Some(&expected) { brackets.pop(); }
                else { brackets.push(ch); } // malformed JSON cannot turn into a second command
                current.push(ch);
            }
            ';' if brackets.is_empty() => {
                if !current.trim().is_empty() { statements.push(std::mem::take(&mut current)); }
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() { statements.push(current); }
    statements
}

#[cfg(test)]
#[path = "db_protocol_dispatch_tests.rs"]
mod protocol_dispatch_tests;

/// 按分号切分 SQL（尊重字符串字面量与注释）。
pub fn split_statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut quote = '\0';
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        if in_line_comment {
            cur.push(c);
            if c == '\n' {
                in_line_comment = false;
            }
            continue;
        }
        if in_block_comment {
            cur.push(c);
            if c == '*' && chars.peek() == Some(&'/') {
                cur.push(chars.next().unwrap());
                in_block_comment = false;
            }
            continue;
        }
        if in_str {
            cur.push(c);
            if c == quote {
                if chars.peek() == Some(&'\'') {
                    cur.push(chars.next().unwrap());
                } else {
                    in_str = false;
                }
            }
            continue;
        }
        match c {
            '\'' | '"' | '`' => {
                in_str = true;
                quote = c;
                cur.push(c);
            }
            '-' if chars.peek() == Some(&'-') => {
                in_line_comment = true;
                cur.push(c);
            }
            '/' if chars.peek() == Some(&'*') => {
                in_block_comment = true;
                cur.push(c);
            }
            ';' => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// SQLite 专用切分（同样尊重引号）。
pub fn split_sqlite(sql: &str) -> Vec<String> {
    split_statements(sql)
}

/// 拆分 Mongo 查询语法 "集合 {JSON 条件}" → (集合名, 条件 JSON)。
fn mongo_split(s: &str) -> (String, &str) {
    match s.find(char::is_whitespace) {
        Some(i) => (s[..i].to_string(), s[i..].trim()),
        None => (s.trim().to_string(), ""),
    }
}
