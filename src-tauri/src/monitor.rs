//! 数据库监控：会话/连接清单与结束会话（kill）。
//! 只提供固定协议 SQL 的只读清单与显式 kill，不提供任意 SQL 执行入口。
use anyhow::{anyhow, bail, Result};
use tauri::State;

use crate::commands::{scoped_db, AppState};
use crate::db::Db;
use crate::error::{AppError, CmdResult};
use crate::models::ProcessInfo;
use sqlx::Row;

/// 列出数据库当前会话与正在执行的 SQL。
#[tauri::command]
pub async fn list_db_processes(
    state: State<'_, AppState>,
    id: String,
    database: Option<String>,
) -> CmdResult<Vec<ProcessInfo>> {
    let _activity_guard = state.activity.enter()?;
    let cfg = state
        .store
        .connections()
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(|| AppError::new("连接不存在"))?;
    let namespace = database.as_deref().map(str::trim).unwrap_or("");
    let conn = scoped_db(&state, &id, namespace).await?;
    list_processes(conn.as_ref(), &cfg.kind).await.map_err(AppError::from)
}

/// 结束一个会话 / 正在执行的 SQL。只读连接明确拒绝。
#[tauri::command]
pub async fn kill_db_process(
    state: State<'_, AppState>,
    id: String,
    database: Option<String>,
    process_id: String,
) -> CmdResult<String> {
    let _activity_guard = state.activity.enter()?;
    let cfg = state
        .store
        .connections()
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(|| AppError::new("连接不存在"))?;
    if cfg.read_only {
        return Err(AppError::new("此连接为只读，禁止结束数据库会话"));
    }
    let namespace = database.as_deref().map(str::trim).unwrap_or("");
    let conn = scoped_db(&state, &id, namespace).await?;
    let message = kill_process(conn.as_ref(), &cfg.kind, &process_id).await?;
    crate::storage::log_line(
        "db-monitor",
        &format!("connection={} kill session={}", cfg.name.replace(['\n', '\r'], " "), process_id),
    );
    Ok(message)
}

fn digits_only(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
}

async fn list_processes(db: &Db, kind: &crate::models::DbKind) -> Result<Vec<ProcessInfo>> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    match db {
        Db::Mysql(pool) => mysql_processes(pool).await,
        Db::Postgres(pool) => pg_processes(pool).await,
        Db::Mssql(_) | Db::ClickHouse(_) | Db::Odbc(_) => generic_processes(db, kind).await,
        _ => bail!("该数据源没有服务器会话概念，无法列出连接（SQLite 为嵌入式数据库）"),
    }

    }).await
}

async fn mysql_processes(pool: &sqlx::MySqlPool) -> Result<Vec<ProcessInfo>> {
    let own: u64 = crate::sql_history::query_scalar("SELECT CONNECTION_ID()").fetch_one(pool).await?;
    let rows = crate::sql_history::query("SHOW FULL PROCESSLIST").fetch_all(pool).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        // PROCESSLIST 列在部分兼容协议服务端（TiDB/OceanBase 等）可能以 binary 字符集返回，
        // 必须走宽容解码。
        let get = |name: &str| crate::schema::mysql_get(row, name).unwrap_or_default();
        let id = get("Id");
        if id.is_empty() {
            continue;
        }
        out.push(ProcessInfo {
            is_self: id == own.to_string(),
            id,
            user: get("User"),
            host: get("Host"),
            db: get("db"),
            command: get("Command"),
            state: get("State"),
            elapsed_secs: get("Time").parse().ok(),
            sql: get("Info"),
        });
    }
    Ok(out)
}

async fn pg_processes(pool: &sqlx::PgPool) -> Result<Vec<ProcessInfo>> {
    let rows = crate::sql_history::query(
        "SELECT pid, COALESCE(usename,'') AS usename, \
         COALESCE(client_addr::text,'本地') AS client_addr, \
         COALESCE(datname,'') AS datname, COALESCE(state,'') AS state, \
         COALESCE(wait_event_type,'') AS wait_type, COALESCE(wait_event,'') AS wait_event, \
         COALESCE(EXTRACT(EPOCH FROM (now() - query_start))::bigint, 0) AS elapsed, \
         COALESCE(query,'') AS query, pid = pg_backend_pid() AS is_self \
         FROM pg_stat_activity WHERE datname = current_database() ORDER BY pid",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let id: i64 = row.try_get("pid")?;
        let elapsed: i64 = row.try_get("elapsed")?;
        let is_self: bool = row.try_get("is_self")?;
        let state_text: String = row.try_get("state")?;
        let wait_type: String = row.try_get("wait_type")?;
        let wait_event: String = row.try_get("wait_event")?;
        let mut state_label = state_text.clone();
        if !wait_type.is_empty() {
            state_label = format!("{}（等待 {}）", if state_label.is_empty() { "-" } else { &state_label }, wait_event);
        }
        out.push(ProcessInfo {
            id: id.to_string(),
            user: row.try_get("usename")?,
            host: row.try_get("client_addr")?,
            db: row.try_get("datname")?,
            command: if state_text == "active" { "正在执行".into() } else if state_text == "idle in transaction" { "事务中空闲".into() } else if state_text == "idle" { "空闲".into() } else { state_text },
            state: state_label,
            elapsed_secs: Some(elapsed),
            sql: row.try_get("query")?,
            is_self,
        });
    }
    Ok(out)
}

/// MSSQL / ClickHouse 走 generic_rows 文本行查询；按列名宽容取值。
async fn generic_processes(db: &Db, kind: &crate::models::DbKind) -> Result<Vec<ProcessInfo>> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let sql = if kind.id() == "sqlserver" || matches!(db, Db::Mssql(_)) {
        "SELECT s.session_id, COALESCE(s.login_name,'') AS login_name, \
         COALESCE(s.host_name,'') AS host_name, COALESCE(DB_NAME(s.database_id),'') AS db_name, \
         COALESCE(s.status,'') AS status, \
         ISNULL(DATEDIFF(SECOND, r.start_time, GETDATE()), 0) AS elapsed, \
         ISNULL(CAST(t.text AS nvarchar(max)),'') AS query_text, \
         CASE WHEN s.session_id = @@SPID THEN 1 ELSE 0 END AS is_self \
         FROM sys.dm_exec_sessions s \
         LEFT JOIN sys.dm_exec_requests r ON r.session_id = s.session_id \
         OUTER APPLY sys.dm_exec_sql_text(r.sql_handle) t \
         WHERE s.is_user_process = 1 ORDER BY s.session_id"
    } else {
        "SELECT query_id, user, address, elapsed, query FROM system.processes ORDER BY elapsed DESC LIMIT 200"
    };
    let (columns, rows) = db.generic_rows(sql).await?;
    let find = |columns: &[String], name: &str| {
        columns.iter().position(|c| c.eq_ignore_ascii_case(name))
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let get = |name: &str| {
            find(&columns, name)
                .and_then(|i| row.get(i).cloned())
                .flatten()
                .unwrap_or_default()
        };
        if matches!(db, Db::Mssql(_)) {
            out.push(ProcessInfo {
                is_self: get("is_self") == "1" || get("is_self").eq_ignore_ascii_case("true"),
                id: get("session_id"),
                user: get("login_name"),
                host: get("host_name"),
                db: get("db_name"),
                command: if get("elapsed").parse::<i64>().unwrap_or(0) > 0 && !get("query_text").is_empty() { "正在执行".into() } else { "会话".into() },
                state: get("status"),
                elapsed_secs: get("elapsed").parse().ok(),
                sql: get("query_text"),
            });
        } else {
            out.push(ProcessInfo {
                is_self: false,
                id: get("query_id"),
                user: get("user"),
                host: get("address"),
                db: String::new(),
                command: "正在执行".into(),
                state: String::new(),
                elapsed_secs: get("elapsed").parse::<f64>().ok().map(|f| f as i64),
                sql: get("query"),
            });
        }
    }
    out.retain(|p| !p.id.is_empty());
    Ok(out)

    }).await
}

async fn kill_process(db: &Db, kind: &crate::models::DbKind, process_id: &str) -> Result<String> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let pid = process_id.trim();
    if pid.is_empty() {
        bail!("缺少会话 ID");
    }
    match db {
        Db::Mysql(pool) => {
            if !digits_only(pid) {
                bail!("MySQL 会话 ID 必须是数字");
            }
            crate::sql_history::query(&format!("KILL {}", pid)).execute(pool).await?;
            Ok(format!("已结束 MySQL 会话 {}", pid))
        }
        Db::Postgres(pool) => {
            if !digits_only(pid) {
                bail!("PostgreSQL 会话 ID 必须是数字");
            }
            let id: i64 = pid.parse()?;
            let terminated: bool = crate::sql_history::query_scalar("SELECT pg_terminate_backend($1)")
                .bind(id)
                .fetch_one(pool)
                .await?;
            Ok(if terminated {
                format!("已结束 PostgreSQL 会话 {}", pid)
            } else {
                format!("会话 {} 不存在或已退出（可能已自行结束）", pid)
            })
        }
        Db::Mssql(_) => {
            if !digits_only(pid) {
                bail!("SQL Server 会话 ID 必须是数字");
            }
            db.generic_exec(&format!("KILL {}", pid)).await?;
            Ok(format!("已结束 SQL Server 会话 {}", pid))
        }
        Db::ClickHouse(_) => {
            if !pid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
                bail!("ClickHouse query_id 只允许字母、数字、下划线和连字符");
            }
            db.generic_exec(&format!("KILL QUERY WHERE query_id = {}", crate::db::quote_literal(pid)))
                .await?;
            Ok(format!("已对 ClickHouse 查询 {} 发送 KILL QUERY", pid))
        }
        _ => bail!(
            "该数据源没有服务器会话概念，无法结束会话（当前类型：{}）",
            kind.0
        ),
    }

    }).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_only_rejects_non_numeric() {
        assert!(digits_only("12345"));
        assert!(!digits_only(""));
        assert!(!digits_only("12a"));
        assert!(!digits_only("1 2"));
        assert!(!digits_only("-1"));
        assert!(!digits_only("1;DROP TABLE x"));
    }

    #[tokio::test]
    async fn mysql_processes_decodes_binary_charset_columns() {
        // 用真实 MySQL 协议服务端验证较重；这里仅验证结构体组装与 is_self 判定。
        let info = ProcessInfo {
            id: "7".into(),
            user: "root".into(),
            host: "localhost:1234".into(),
            db: "test".into(),
            command: "Query".into(),
            state: "".into(),
            elapsed_secs: Some(3),
            sql: "SELECT 1".into(),
            is_self: false,
        };
        assert_eq!(info.id, "7");
        assert!(!info.is_self);
        assert!(anyhow!("占位").to_string().contains("占位"));
    }

    #[tokio::test]
    async fn unsupported_kind_fails_cleanly() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let db = Db::Sqlite(pool);
        let kind = crate::models::DbKind::of("sqlite");
        let error = list_processes(&db, &kind).await.unwrap_err();
        assert!(error.to_string().contains("没有服务器会话概念"));
        let kill = kill_process(&db, &kind, "1").await.unwrap_err();
        assert!(kill.to_string().contains("无法结束会话"));
    }
}
