//! Persistent query transactions: one owned SQLx transaction, never a pooled BEGIN.
use std::{collections::HashMap, sync::Arc};
use anyhow::{anyhow, bail, Result};
use serde::Serialize;
use sqlx::{Column, Executor, TypeInfo};
use tokio::sync::{Mutex, OwnedMutexGuard, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};
use crate::{db::{self, Db}, models::{DbKind, MultiResult, ResultColumn, ResultSet}};

#[derive(Debug, Clone, Default, Serialize)]
pub struct ManualTransactionStatus {
    pub active: bool,
    pub schema: String,
    pub owner: String,
    pub failed: bool,
}

enum Handle {
    Sqlite(sqlx::Transaction<'static, sqlx::Sqlite>),
    Mysql(sqlx::Transaction<'static, sqlx::MySql>),
    Postgres(sqlx::Transaction<'static, sqlx::Postgres>),
}
struct Session { history:crate::sql_history::Context, status: ManualTransactionStatus, handle: Handle, kind: DbKind, principal: Option<String>, unknown: bool, mysql_pool: Option<sqlx::MySqlPool>, pg_pool: Option<sqlx::PgPool> }

#[derive(Default)]
pub struct Registry {
    slots: parking_lot::Mutex<HashMap<String, Arc<Mutex<Option<Session>>>>>,
    lifecycle: Arc<RwLock<()>>,
}
/// Hold this across the whole ordinary query/direct write to serialize against BEGIN.
pub struct Lease {
    _lifecycle: OwnedRwLockReadGuard<()>,
    slot: OwnedMutexGuard<Option<Session>>,
}
pub struct OrdinaryLease {
    _lifecycle: OwnedRwLockReadGuard<()>,
    _slots: Vec<OwnedMutexGuard<Option<Session>>>,
}
impl Registry {
    /// 同步只检查状态并持有生命周期独占锁，不提交、回滚或取走事务。
    pub(crate) fn inactive_for_sync(&self, ids: &[String]) -> Result<OwnedRwLockWriteGuard<()>> {
        let lifecycle = self.lifecycle.clone().try_write_owned()
            .map_err(|_| anyhow!("数据库或手动事务操作仍在运行，请稍后重新提交同步"))?;
        let slots = self.slots.lock();
        for id in ids {
            if let Some(slot) = slots.get(id) {
                let guard = slot.try_lock().map_err(|_| anyhow!("手动事务操作仍在运行"))?;
                if guard.is_some() { bail!("受影响连接存在手动查询事务，请明确提交或回滚后重新预览；同步不会结束事务"); }
            }
        }
        Ok(lifecycle)
    }
    pub async fn lease(&self, id: &str) -> Lease {
        let lifecycle = self.lifecycle.clone().read_owned().await;
        let slot = self.slots.lock().entry(id.into()).or_default().clone();
        Lease { _lifecycle: lifecycle, slot: slot.lock_owned().await }
    }
    pub async fn ordinary(&self, ids: &[&str]) -> Result<OrdinaryLease> {
        let lifecycle = self.lifecycle.clone().read_owned().await;
        let mut ids = ids.to_vec(); ids.sort_unstable(); ids.dedup();
        let mut guards = Vec::new();
        for id in ids {
            let slot = self.slots.lock().entry(id.into()).or_default().clone();
            let guard = slot.lock_owned().await;
            if guard.is_some() { bail!("此连接存在手动查询事务，请先提交或回滚事务再执行数据工具或连接操作"); }
            guards.push(guard);
        }
        Ok(OrdinaryLease { _lifecycle: lifecycle, _slots: guards })
    }
    /// 保活只尝试取得现有空闲租约；竞争或手动事务存在时立即跳过。
    /// 取得部分连接后若失败，所有已持有 guard 随返回释放，不排队探测。
    pub async fn try_ordinary(&self, ids: &[&str]) -> Result<Option<OrdinaryLease>> {
        let lifecycle = match self.lifecycle.clone().try_read_owned() {
            Ok(guard) => guard,
            Err(_) => return Ok(None),
        };
        let mut ids = ids.to_vec(); ids.sort_unstable(); ids.dedup();
        let mut guards = Vec::with_capacity(ids.len());
        for id in ids {
            let slot = self.slots.lock().entry(id.into()).or_default().clone();
            let guard = match slot.try_lock_owned() {
                Ok(guard) => guard,
                Err(_) => return Ok(None),
            };
            if guard.is_some() { return Ok(None); }
            guards.push(guard);
        }
        Ok(Some(OrdinaryLease { _lifecycle: lifecycle, _slots: guards }))
    }
    pub async fn close_all(&self) -> Result<OwnedRwLockWriteGuard<()>> {
        let lifecycle = self.lifecycle.clone().write_owned().await;
        let slots: Vec<_> = self.slots.lock().values().cloned().collect();
        let mut error = None;
        for slot in slots {
            let mut slot = slot.lock().await;
            if let Some(session) = slot.take() {
                if let Err(e) = close_session(session).await { error = Some(e); }
            }
        }
        if let Some(error) = error { return Err(error); }
        Ok(lifecycle)
    }
}
impl Lease {
    pub fn status(&self) -> ManualTransactionStatus {
        self.slot.as_ref().map(|s| s.status.clone()).unwrap_or_default()
    }
    /// 当前会话的连接类型（会话不存在时 None）；用于按方言构造行变更 SQL。
    pub fn db_kind(&self) -> Option<crate::models::DbKind> {
        self.slot.as_ref().map(|s| s.kind.clone())
    }
    pub fn require_inactive(&self) -> Result<()> {
        if self.slot.is_some() { bail!("此连接存在手动查询事务，请先提交或回滚事务再执行其它查询、数据工具或连接操作"); }
        Ok(())
    }
    pub async fn start(&mut self, conn: &Db, kind: DbKind, schema: String, owner: String) -> Result<ManualTransactionStatus> {
    let __sql_history_context = crate::sql_history::db_context(conn);
    crate::sql_history::scope(__sql_history_context, async {
        self.require_inactive()?;
        if owner.trim().is_empty() { bail!("手动事务必须绑定查询标签页所有者"); }
        let handle = match conn {
            Db::Sqlite(pool) => {
                if !schema.is_empty() && schema != "main" { bail!("SQLite 手动查询事务仅支持 main"); }
                let mut connection = pool.acquire().await?;
                let attached = crate::sql_history::query("PRAGMA database_list").fetch_all(&mut *connection).await?;
                if attached.iter().any(|row| sqlx::Row::get::<String, _>(row, "name") != "main" && sqlx::Row::get::<String, _>(row, "name") != "temp") {
                    bail!("SQLite 手动事务连接包含附加数据库，无法安全绑定 main");
                }
                let temporary: i64 = crate::sql_history::query_scalar("SELECT COUNT(*) FROM temp.sqlite_master").fetch_one(&mut *connection).await?;
                if temporary != 0 { bail!("SQLite 手动事务连接包含临时对象，请清理临时对象或重开连接"); }
                // 预审拒绝不销毁既有池状态；只有真实事务拥有的连接才隔离复用。
                connection.close_on_drop();
                let tx = sqlx::Transaction::begin(connection, None).await?;
                Handle::Sqlite(tx)
            }
            Db::Mysql(pool) => {
                let mut connection = pool.acquire().await?; connection.close_on_drop();
                let mut tx = sqlx::Transaction::begin(connection, None).await?;
                // Non-transactional tables make a rollback promise false; fail closed.
                let unsupported: i64 = crate::sql_history::query_scalar("SELECT COUNT(*) FROM information_schema.TABLES WHERE TABLE_SCHEMA = DATABASE() AND TABLE_TYPE = 'BASE TABLE' AND COALESCE(ENGINE,'') <> 'InnoDB'")
                    .fetch_one(&mut *tx).await?;
                if unsupported != 0 { bail!("MySQL 选定数据库包含非 InnoDB 数据表，无法保证手动事务回滚"); }
                Handle::Mysql(tx)
            }
            Db::Postgres(pool) => {
                let mut connection = pool.acquire().await?; connection.close_on_drop();
                Handle::Postgres(sqlx::Transaction::begin(connection, None).await?)
            },
            _ => bail!("手动查询事务仅支持 SQLx 原生 SQLite / MySQL / PostgreSQL 协议连接，其它驱动不支持"),
        };
        let status = ManualTransactionStatus { active: true, schema, owner, failed: false };
        *self.slot = Some(Session { history:crate::sql_history::current_context(), status: status.clone(), handle, kind, principal: None, unknown: false,
            mysql_pool: if let Db::Mysql(pool) = conn { Some(pool.clone()) } else { None },
            pg_pool: if let Db::Postgres(pool) = conn { Some(pool.clone()) } else { None } });
        Ok(status)
    
    }).await
}
    pub fn bind_principal(&mut self, principal: &str) { if let Some(s) = self.slot.as_mut() { s.principal = Some(principal.into()); } }
    pub fn require_principal(&self, principal: &str) -> Result<()> {
        if self.slot.as_ref().is_some_and(|s| s.principal.as_deref().is_some_and(|p| p != principal)) { bail!("手动事务属于其它调用窗口"); } Ok(())
    }
    pub async fn execute_controlled(&mut self, owner: Option<&str>, schema: &str, sql: &str, entry: &Arc<crate::execution::Entry>) -> Result<Option<(crate::error::CmdResult<MultiResult>, bool)>> {
        self.require_principal(&entry.binding.principal)?;
        let Some(session) = self.slot.as_mut() else {
            if owner.is_some() { bail!("该查询的手动事务已经结束，请刷新事务状态"); } return Ok(None);
        };
        // SQLite 的空命名空间等价 main：前端表页归一为 main 后，执行请求可能仍带空串
        // （旧调用点/受控执行默认值），两者必须视为同一命名空间（BUG-R1-014）。
        let same_namespace = schema == session.status.schema
            || (session.kind.protocol() == crate::models::Protocol::Sqlite
                && matches!((schema, session.status.schema.as_str()),
                    ("" | "main", "main") | ("main", "")));
        if owner != Some(session.status.owner.as_str()) || !same_namespace {
            bail!(format!("执行页/命名空间与手动事务不匹配（页面 schema={schema:?}，事务 schema={:?}，owner={owner:?}/{:?}，same_ns={same_namespace}）", session.status.schema, session.status.owner));
        }
        if session.status.failed { bail!("手动事务已失败，只允许明确回滚"); }
        let mut context=session.history.clone();context.operation_id=Some(entry.id.clone());context.origin="query".into();
        crate::sql_history::scope(context,async {
        let statements = checked_statements(sql)?;
        if session.kind.is_mysql_family() { validate_mysql_targets(&mut session.handle, &statements).await?; }
        let sql = statements.join(";\n");
        let (result, confirmed) = match &mut session.handle {
            Handle::Sqlite(tx) => crate::execution::sqlite_run(&mut **tx, &sql, entry, false).await,
            Handle::Mysql(tx) => crate::execution::mysql_run(&mut **tx, session.mysql_pool.as_ref().expect("MySQL pool"), &sql, entry).await,
            Handle::Postgres(tx) => crate::execution::pg_run(&mut **tx, session.pg_pool.as_ref().expect("PG pool"), &sql, entry, true).await,
        };
        if result.is_err() || entry.stopping() { session.status.failed = true; }
        if entry.stopping() && !confirmed { session.unknown = true; }
        Ok(Some((result.map_err(crate::error::AppError::from), confirmed)))
        }).await
    }
    pub async fn execute(&mut self, owner: Option<&str>, schema: &str, sql: &str) -> Result<Option<MultiResult>> {
        let Some(session) = self.slot.as_mut() else {
            if owner.is_some() { bail!("该查询的手动事务已经结束，请刷新事务状态"); }
            return Ok(None);
        };
        if owner != Some(session.status.owner.as_str()) { bail!("此连接的手动事务属于其它查询标签页，请先结束事务"); }
        // 与 execute_controlled 一致：SQLite 空命名空间等价 main（BUG-R1-014）。
        let same_namespace = schema == session.status.schema
            || (session.kind.protocol() == crate::models::Protocol::Sqlite
                && matches!((schema, session.status.schema.as_str()),
                    ("" | "main", "main") | ("main", "")));
        if !same_namespace { bail!("手动事务期间不能切换数据库/模式"); }
        if session.status.failed { bail!("手动事务已失败，只能回滚，不能继续执行或提交"); }
        let result = crate::sql_history::scope(session.history.clone(),async {
            let statements = checked_statements(sql)?;
            if session.kind.is_mysql_family() { validate_mysql_targets(&mut session.handle, &statements).await?; }
            execute_handle(&mut session.handle, &statements).await
        }).await;
        match result {
            Ok(result) => Ok(Some(result)),
            Err(error) => { session.status.failed = true; Err(error) }
        }
    }
    pub async fn finish(&mut self, owner: &str, commit: bool) -> Result<ManualTransactionStatus> {
        let session = self.slot.as_ref().ok_or_else(|| anyhow!("手动事务已经结束，不能重复提交或回滚"))?;
        if owner != session.status.owner { bail!("只能由手动事务所属查询标签页提交或回滚"); }
        if commit && session.status.failed { bail!("手动事务已失败，只能回滚，不能提交"); }
        let session = self.slot.take().expect("validated transaction");
        if session.unknown {
            // 应用不再显式提交/回滚；SQLx Drop 可能尽力回滚，close_on_drop 禁止回池。
            // 没有回滚回执，销毁不能当作已回滚。
            drop(session);
            bail!("停止未确认，异常事务物理连接已隔离/销毁；回滚效果未知，请验证数据库状态");
        }
        finish_handle(session.handle, commit).await?;
        Ok(ManualTransactionStatus::default())
    }
    pub async fn close(&mut self) -> Result<()> {
        if let Some(session) = self.slot.take() { close_session(session).await?; }
        Ok(())
    }
}
async fn close_session(session: Session) -> Result<()> {
    if session.unknown {
        // 离页/全局退出与显式 finish 相同：不追加应用回滚；SQLx Drop 尽力清理且不回池。
        drop(session);
        bail!("停止未确认，异常事务物理连接已隔离/销毁；回滚效果未知，请验证数据库状态");
    }
    finish_handle(session.handle, false).await
}
async fn finish_handle(handle: Handle, commit: bool) -> Result<()> {
    macro_rules! finish { ($tx:expr) => { if commit { $tx.commit().await?; } else { $tx.rollback().await?; } }; }
    match handle { Handle::Sqlite(tx) => finish!(tx), Handle::Mysql(tx) => finish!(tx), Handle::Postgres(tx) => finish!(tx) }
    Ok(())
}

/// SQL lexer used only as a fail-closed transaction policy, not an SQL rewriting parser.
/// Preserve quoted identifiers for target checks; hide strings/comments; split outside them.
pub(crate) fn tokenize(sql: &str) -> Result<Vec<Vec<String>>> {
    let chars: Vec<char> = sql.chars().collect();
    let mut all = Vec::new(); let mut tokens = Vec::new(); let mut token = String::new(); let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '-' && chars.get(i+1) == Some(&'-') || c == '#' {
            if !token.is_empty() { tokens.push(std::mem::take(&mut token)); }
            while i < chars.len() && chars[i] != '\n' { i += 1; } continue;
        }
        if c == '/' && chars.get(i+1) == Some(&'*') {
            if !token.is_empty() { tokens.push(std::mem::take(&mut token)); }
            if chars.get(i+2) == Some(&'!') || chars.get(i+2) == Some(&'M') && chars.get(i+3) == Some(&'!') { bail!("手动事务禁止可执行注释"); }
            i += 2; let mut depth = 1;
            while i < chars.len() && depth > 0 {
                if chars[i] == '/' && chars.get(i+1) == Some(&'*') { depth += 1; i += 2; }
                else if chars[i] == '*' && chars.get(i+1) == Some(&'/') { depth -= 1; i += 2; } else { i += 1; }
            }
            if depth != 0 { bail!("SQL 注释未结束"); } continue;
        }
        if c == '\'' || c == '"' || c == '`' || c == '[' {
            if !token.is_empty() { tokens.push(std::mem::take(&mut token)); }
            let close = if c == '[' { ']' } else { c }; let mut quoted = String::new(); i += 1; let mut ended = false;
            while i < chars.len() {
                if chars[i] == close {
                    if chars.get(i+1) == Some(&close) { quoted.push(close); i += 2; continue; }
                    ended = true; i += 1; break;
                }
                if chars[i] == '\\' && c == '\'' { i += 2; continue; }
                quoted.push(chars[i]); i += 1;
            }
            if !ended { bail!("SQL 引号未结束"); }
            if c != '\'' { tokens.push(format!("@{}", quoted)); } else { tokens.push("@literal".into()); }
            continue;
        }
        if c == '$' {
            let mut end = i+1;
            while end < chars.len() && (chars[end].is_ascii_alphanumeric() || chars[end] == '_') { end += 1; }
            if chars.get(end) == Some(&'$') {
                let delimiter: String = chars[i..=end].iter().collect(); let remaining: String = chars[end+1..].iter().collect();
                let Some(at) = remaining.find(&delimiter) else { bail!("SQL dollar 引号未结束"); };
                if !token.is_empty() { tokens.push(std::mem::take(&mut token)); }
                i = end+1 + remaining[..at].chars().count() + delimiter.chars().count(); tokens.push("@literal".into()); continue;
            }
        }
        if c == ';' || c.is_whitespace() || "(),.=".contains(c) {
            if !token.is_empty() { tokens.push(std::mem::take(&mut token)); }
            if c == ';' { if !tokens.is_empty() { all.push(std::mem::take(&mut tokens)); } }
            else if !c.is_whitespace() { tokens.push(c.to_string()); }
        } else { token.push(c.to_ascii_uppercase()); }
        i += 1;
    }
    if !token.is_empty() { tokens.push(token); }
    if !tokens.is_empty() { all.push(tokens); }
    Ok(all)
}
fn checked_statements(sql: &str) -> Result<Vec<String>> {
    let tokens = tokenize(sql)?;
    if tokens.is_empty() { bail!("SQL 为空"); }
    for statement in &tokens {
        if !matches!(statement.first().map(String::as_str), Some("SELECT" | "WITH" | "INSERT" | "UPDATE" | "DELETE" | "REPLACE")) {
            bail!("手动事务仅允许查询和 DML，禁止事务控制、DDL、数据库切换、SET/LOCK、过程与维护命令");
        }
        // Also rejects controls hidden in a CTE or a malicious multi-statement fragment.
        if statement.iter().any(|t| matches!(t.as_str(), "BEGIN" | "COMMIT" | "ROLLBACK" | "SAVEPOINT" | "RELEASE" | "START" | "ATTACH" | "DETACH" | "USE" | "LOCK" | "UNLOCK" | "CREATE" | "ALTER" | "DROP" | "TRUNCATE" | "GRANT" | "REVOKE" | "CALL" | "VACUUM" | "PRAGMA")) {
            bail!("SQL 包含不允许的事务控制或隐式提交命令");
        }
        if statement.iter().any(|t| {
            let name = t.strip_prefix('@').unwrap_or(t).to_ascii_uppercase();
            matches!(name.as_str(), "SET_CONFIG" | "GET_LOCK" | "RELEASE_LOCK" | "RELEASE_ALL_LOCKS" | "LOAD_EXTENSION" | "LO_IMPORT" | "LO_EXPORT") || name.starts_with("PG_ADVISORY_") || name.starts_with("DBLINK")
        }) {
            bail!("手动事务禁止修改会话、外部连接或不可回滚副作用函数");
        }
        if statement.first().map(String::as_str) == Some("SELECT") && statement.iter().any(|t| t == "INTO") { bail!("手动事务禁止 SELECT INTO 创建对象或文件"); }
        if statement.windows(2).any(|w| w[0] == "INTO" && matches!(w[1].as_str(), "OUTFILE" | "DUMPFILE")) { bail!("手动事务禁止文件输出"); }
    }
    let statements: Vec<_> = db::split_statements(sql).into_iter().filter(|s| db::is_executable(s)).collect();
    // The existing splitter does not support dollar-quoted semicolons. Fail closed.
    if statements.len() != tokens.len() { bail!("此 SQL 的引用或语句边界不受手动事务支持，请拆分后执行"); }
    Ok(statements)
}
async fn validate_mysql_targets(handle: &mut Handle, statements: &[String]) -> Result<()> {
    let Handle::Mysql(tx) = handle else { return Ok(()); };
    for sql in statements {
        let blocks = tokenize(sql)?;
        let tokens = &blocks[0];
        // WITH writes/multiple-table UPDATE/DELETE are deliberately unsupported here.
        if tokens[0] == "WITH" && tokens.iter().any(|t| matches!(t.as_str(), "INSERT" | "UPDATE" | "DELETE" | "REPLACE")) { bail!("MySQL 手动事务不支持 WITH 写入"); }
        if !matches!(tokens[0].as_str(), "INSERT" | "UPDATE" | "DELETE" | "REPLACE") { continue; }
        let mut at = 1;
        while tokens.get(at).is_some_and(|t| matches!(t.as_str(), "LOW_PRIORITY" | "IGNORE" | "DELAYED" | "INTO" | "FROM")) { at += 1; }
        let target = tokens.get(at).ok_or_else(|| anyhow!("缺少事务写入目标表"))?;
        let name = target.strip_prefix('@').unwrap_or(target).to_string();
        if tokens.get(at+1).is_some_and(|t| matches!(t.as_str(), "." | "," | "JOIN")) { bail!("MySQL 手动事务仅支持当前库单表写入，不支持跨库或多表修改"); }
        if tokens[0] == "DELETE" && tokens.get(1).map(String::as_str) != Some("FROM") { bail!("MySQL 手动事务仅支持 DELETE FROM 单表形式"); }
        if tokens.iter().any(|t| t == "JOIN") || tokens[0] == "UPDATE" && tokens[..tokens.iter().position(|t| t == "SET").unwrap_or(tokens.len())].iter().any(|t| t == ",") { bail!("MySQL 手动事务不支持多表修改"); }
        let unsafe_targets: i64 = crate::sql_history::query_scalar("SELECT COUNT(*) FROM information_schema.TABLES WHERE TABLE_SCHEMA = DATABASE() AND LOWER(TABLE_NAME) = LOWER(?) AND (TABLE_TYPE <> 'BASE TABLE' OR COALESCE(ENGINE,'') <> 'InnoDB')")
            .bind(&name).fetch_one(&mut **tx).await?;
        let existing: i64 = crate::sql_history::query_scalar("SELECT COUNT(*) FROM information_schema.TABLES WHERE TABLE_SCHEMA = DATABASE() AND LOWER(TABLE_NAME) = LOWER(?) AND TABLE_TYPE = 'BASE TABLE'")
            .bind(&name).fetch_one(&mut **tx).await?;
        let triggers: i64 = crate::sql_history::query_scalar("SELECT COUNT(*) FROM information_schema.TRIGGERS WHERE EVENT_OBJECT_SCHEMA = DATABASE() AND LOWER(EVENT_OBJECT_TABLE) = LOWER(?)")
            .bind(&name).fetch_one(&mut **tx).await?;
        if existing == 0 || unsafe_targets != 0 || triggers != 0 { bail!("MySQL 手动事务写入目标必须是无触发器的已验证 InnoDB 单表"); }
    }
    Ok(())
}
fn columns(names: Vec<String>, types: Vec<String>) -> Vec<ResultColumn> {
    names.into_iter().enumerate().map(|(i, name)| ResultColumn { name, data_type: types.get(i).cloned().unwrap_or_default(), nullable: true, primary_key: false, auto_increment: false }).collect()
}

/// 手动事务内的图形化行变更（网格「开始事务」后的可视化编辑提交）。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ManualMutationOp {
    Update {
        table: String,
        changes: Vec<(String, Option<String>)>,
        keys: Vec<(String, Option<String>)>,
        old_values: Vec<(String, Option<String>)>,
    },
    Insert {
        table: String,
        columns: Vec<String>,
        values: Vec<Option<String>>,
    },
    Delete {
        table: String,
        keys: Vec<(String, Option<String>)>,
        old_values: Vec<(String, Option<String>)>,
    },
}

/// 把结构化行变更编译为方言 SQL（标识符与字面量统一走 db 层引用，杜绝拼接注入）。
pub fn build_manual_mutation_sql(kind: &DbKind, schema: &str, ops: &[ManualMutationOp]) -> Result<Vec<String>> {
    use crate::schema::where_condition;
    let mut statements = Vec::with_capacity(ops.len());
    for op in ops {
        match op {
            ManualMutationOp::Update { table, changes, keys, old_values } => {
                let full = validate_table(kind, schema, table)?;
                if changes.is_empty() { bail!("更新缺少变更列"); }
                let sets: Vec<String> = changes.iter()
                    .map(|(col, val)| format!("{} = {}", db::quote_ident(kind, col), match val { Some(s) => db::quote_literal(s), None => "NULL".into() }))
                    .collect();
                let mut conds: Vec<String> = keys.iter().map(|(c, v)| where_condition(kind, c, v)).collect();
                if conds.is_empty() {
                    conds = old_values.iter().map(|(c, v)| where_condition(kind, c, v)).collect();
                }
                if conds.is_empty() { bail!("缺少主键条件，无法在事务中更新该行"); }
                statements.push(format!("UPDATE {} SET {} WHERE {}", full, sets.join(", "), conds.join(" AND ")));
            }
            ManualMutationOp::Insert { table, columns, values } => {
                let full = validate_table(kind, schema, table)?;
                // 空列插入使用 DEFAULT VALUES（SQLite/PG 支持；MySQL 用 () VALUES ()）
                if columns.is_empty() {
                    statements.push(match kind.protocol() {
                        crate::models::Protocol::Mysql => format!("INSERT INTO {} () VALUES ()", full),
                        _ => format!("INSERT INTO {} DEFAULT VALUES", full),
                    });
                    continue;
                }
                if columns.len() != values.len() { bail!("插入列与值数量不一致"); }
                let cols: Vec<String> = columns.iter().map(|c| db::quote_ident(kind, c)).collect();
                let vals: Vec<String> = values.iter().map(|v| match v { Some(s) => db::quote_literal(s), None => "NULL".into() }).collect();
                statements.push(format!("INSERT INTO {} ({}) VALUES ({})", full, cols.join(", "), vals.join(", ")));
            }
            ManualMutationOp::Delete { table, keys, old_values } => {
                let full = validate_table(kind, schema, table)?;
                let mut conds: Vec<String> = keys.iter().map(|(c, v)| where_condition(kind, c, v)).collect();
                if conds.is_empty() {
                    conds = old_values.iter().map(|(c, v)| where_condition(kind, c, v)).collect();
                }
                if conds.is_empty() { bail!("缺少主键条件，无法在事务中删除该行"); }
                statements.push(format!("DELETE FROM {} WHERE {}", full, conds.join(" AND ")));
            }
        }
    }
    Ok(statements)
}

/// 校验并限定行变更的目标表名（拒绝空名 / 分号 / NUL，统一加方言限定）。
fn validate_table(kind: &DbKind, schema: &str, table: &str) -> Result<String> {
    if table.trim().is_empty() || table.contains([';', '\0']) {
        bail!("行变更目标表名无效");
    }
    Ok(db::qualified(kind, schema, table))
}
async fn execute_handle(handle: &mut Handle, statements: &[String]) -> Result<MultiResult> {
    let start = std::time::Instant::now(); let mut sets = Vec::new(); let mut messages = Vec::new();
    macro_rules! execute {
        ($tx:expr, $values:path) => {
            for statement in statements {
                let conn = &mut **$tx;
                let describe = crate::sql_history::describe(&mut *conn,statement).await?;
                let names: Vec<_> = describe.columns().iter().map(|c| c.name().to_string()).collect();
                let types: Vec<_> = describe.columns().iter().map(|c| c.type_info().name().to_string()).collect();
                if !names.is_empty() {
                    let rows = crate::sql_history::query(statement).fetch_all(&mut *conn).await?;
                    let rows: Vec<_> = rows.iter().map(|row| $values(row, names.len())).collect();
                    let total = rows.len() as u64;
                    sets.push(ResultSet { columns: columns(names, types), rows, affected: 0, elapsed_ms: 0, ok: true, message: String::new(), has_rows: true, primary_keys: vec![], total });
                } else {
                    let affected = crate::sql_history::query(statement).execute(&mut *conn).await?.rows_affected();
                    messages.push(format!("{} 行受影响（尚未提交）", affected));
                    sets.push(ResultSet { columns: vec![], rows: vec![], affected, elapsed_ms: 0, ok: true, message: String::new(), has_rows: false, primary_keys: vec![], total: 0 });
                }
            }
        }
    }
    match handle { Handle::Sqlite(tx) => execute!(tx, crate::values::sqlite_row_values), Handle::Mysql(tx) => execute!(tx, crate::values::my_row_values), Handle::Postgres(tx) => execute!(tx, crate::values::pg_row_values) }
    Ok(MultiResult { sets, messages, elapsed_ms: start.elapsed().as_millis() as u64, spool_id: None })
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    struct Fixture { db: Db, observer: sqlx::SqlitePool, path: PathBuf, registry: Registry }
    impl Fixture {
        async fn new() -> Self {
            let path = std::env::temp_dir().join(format!("navkit-manual-{}.sqlite", uuid::Uuid::new_v4()));
            let options = sqlx::sqlite::SqliteConnectOptions::new().filename(&path).create_if_missing(true)
                .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal).busy_timeout(std::time::Duration::from_secs(2));
            let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(3).connect_with(options.clone()).await.unwrap();
            crate::sql_history::query("CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT NOT NULL)").execute(&pool).await.unwrap();
            let observer = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(options).await.unwrap();
            Self { db: Db::Sqlite(pool), observer, path, registry: Registry::default() }
        }
        async fn start(&self) -> ManualTransactionStatus {
            self.registry.lease("db").await.start(&self.db, DbKind::of("sqlite"), "main".into(), "tab-a".into()).await.unwrap()
        }
        async fn execute(&self, sql: &str) -> Result<MultiResult> {
            self.registry.lease("db").await.execute(Some("tab-a"), "main", sql).await.map(Option::unwrap)
        }
        async fn count(&self) -> i64 { crate::sql_history::query_scalar("SELECT COUNT(*) FROM items").fetch_one(&self.observer).await.unwrap() }
        async fn cleanup(self) {
            drop(self.registry.close_all().await.unwrap());
            if let Db::Sqlite(pool) = self.db { pool.close().await; }
            self.observer.close().await;
            // Windows 下连接池句柄释放可能滞后于 close().await；清理失败不应伪装成断言失败。
            let _ = std::fs::remove_file(&self.path);
        }
    }
    #[tokio::test]
    async fn idle_probe_lease_never_waits_and_releases_partial_acquisition() {
        let registry = Registry::default();
        let busy = registry.ordinary(&["b"]).await.unwrap();
        assert!(registry.try_ordinary(&["b", "a", "a"]).await.unwrap().is_none());
        let partial = registry.try_ordinary(&["a"]).await.unwrap().expect("a must not remain locked after b was busy");
        drop(partial); drop(busy);
        let acquired = registry.try_ordinary(&["a", "b"]).await.unwrap().unwrap();
        assert!(registry.try_ordinary(&["a"]).await.unwrap().is_none());
        drop(acquired);
        let exclusive = registry.lifecycle.clone().write_owned().await;
        assert!(registry.try_ordinary(&["a"]).await.unwrap().is_none());
        drop(exclusive);
        assert!(registry.try_ordinary(&["a"]).await.unwrap().is_some());
    }
    #[tokio::test]
    async fn idle_probe_skips_active_transaction_without_touching_it() {
        let f = Fixture::new().await;
        f.start().await;
        f.execute("INSERT INTO items VALUES(1,'pending')").await.unwrap();
        assert!(f.registry.try_ordinary(&["db"]).await.unwrap().is_none());
        assert!(f.registry.lease("db").await.status().active);
        assert_eq!(f.count().await, 0);
        f.registry.lease("db").await.finish("tab-a", false).await.unwrap();
        assert!(f.registry.try_ordinary(&["db"]).await.unwrap().is_some());
        assert_eq!(f.count().await, 0);
        f.cleanup().await;
    }
    #[tokio::test]
    async fn manual_transaction_cross_requests_read_own_writes_commit_and_visibility() {
        let f = Fixture::new().await; assert!(f.start().await.active);
        f.execute("INSERT INTO items VALUES(1, '中文;COMMIT --仅文本')").await.unwrap();
        assert_eq!(f.count().await, 0);
        let result = f.execute("SELECT name FROM items WHERE id=1; SELECT name FROM items WHERE id=99;").await.unwrap();
        assert_eq!(result.sets[0].rows, vec![vec![Some("中文;COMMIT --仅文本".into())]]);
        assert_eq!(result.sets[1].columns[0].name, "name");
        assert!(result.sets[1].rows.is_empty());
        let done = f.registry.lease("db").await.finish("tab-a", true).await.unwrap();
        assert!(!done.active); assert!(done.schema.is_empty() && done.owner.is_empty());
        assert_eq!(f.count().await, 1);
        f.cleanup().await;
    }
    #[tokio::test]
    async fn manual_transaction_rollback_and_close_are_real_zero_write() {
        let f = Fixture::new().await;
        f.start().await; f.execute("INSERT INTO items VALUES(1,'rollback')").await.unwrap();
        f.registry.lease("db").await.finish("tab-a", false).await.unwrap(); assert_eq!(f.count().await, 0);
        f.start().await; f.execute("INSERT INTO items VALUES(1,'close')").await.unwrap();
        f.registry.lease("db").await.close().await.unwrap(); assert_eq!(f.count().await, 0);
        f.start().await; f.execute("INSERT INTO items VALUES(1,'close all')").await.unwrap();
        drop(f.registry.close_all().await.unwrap()); assert_eq!(f.count().await, 0);
        f.cleanup().await;
    }
    #[tokio::test]
    async fn manual_transaction_error_marks_failed_and_commit_is_rejected() {
        let f = Fixture::new().await; f.start().await;
        f.execute("INSERT INTO items VALUES(1,'first')").await.unwrap();
        assert!(f.execute("INSERT INTO items VALUES(1,'duplicate')").await.is_err());
        assert!(f.registry.lease("db").await.status().failed);
        assert!(f.registry.lease("db").await.finish("tab-a", true).await.is_err());
        assert!(f.execute("SELECT * FROM items").await.is_err());
        f.registry.lease("db").await.finish("tab-a", false).await.unwrap(); assert_eq!(f.count().await, 0);
        f.cleanup().await;
    }
    #[tokio::test]
    async fn manual_transaction_owner_namespace_and_direct_write_guards_fail_closed() {
        let f = Fixture::new().await; f.start().await;
        for (owner, schema) in [(None,"main"),(Some("tab-b"),"main"),(Some("tab-a"),"other")] {
            assert!(f.registry.lease("db").await.execute(owner,schema,"INSERT INTO items VALUES(1,'bad')").await.is_err());
        }
        assert!(!f.registry.lease("db").await.status().failed, "foreign requests must not poison the owner transaction");
        assert!(f.registry.ordinary(&["db"]).await.is_err());
        assert!(f.registry.ordinary(&["target","db"]).await.is_err());
        assert!(f.registry.lease("db").await.finish("tab-b", false).await.is_err());
        assert_eq!(f.count().await, 0);
        f.registry.lease("db").await.finish("tab-a",false).await.unwrap();
        assert!(f.registry.lease("db").await.finish("tab-a",false).await.is_err());
        assert!(f.registry.lease("db").await.execute(Some("tab-a"),"main","SELECT 1").await.is_err());
        assert!(f.registry.lease("db").await.execute(None,"main","SELECT 1").await.unwrap().is_none());
        f.cleanup().await;
    }
    #[tokio::test]
    async fn manual_transaction_concurrent_start_finish_and_direct_operation_serialization() {
        let f = Fixture::new().await;
        let start = || async { f.registry.lease("db").await.start(&f.db,DbKind::of("sqlite"),"main".into(),"tab-a".into()).await };
        let (a,b) = tokio::join!(start(),start()); assert_eq!(usize::from(a.is_ok())+usize::from(b.is_ok()),1);
        f.execute("INSERT INTO items VALUES(1,'pending')").await.unwrap();
        let finish = || async { f.registry.lease("db").await.finish("tab-a",true).await };
        let (a,b) = tokio::join!(finish(),finish()); assert_eq!(usize::from(a.is_ok())+usize::from(b.is_ok()),1);
        assert_eq!(f.count().await,1);
        {
            let ordinary = f.registry.ordinary(&["db"]).await.unwrap();
            let begin = f.registry.lease("db"); tokio::pin!(begin);
            assert!(tokio::time::timeout(std::time::Duration::from_millis(20), &mut begin).await.is_err());
            drop(ordinary); drop(begin.await);
        }
        f.cleanup().await;
    }
    #[tokio::test]
    async fn manual_transaction_refuses_pooled_temporary_and_attached_objects() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        crate::sql_history::query("CREATE TEMP TABLE shadow(id INTEGER)").execute(&pool).await.unwrap();
        let db = Db::Sqlite(pool.clone()); let registry = Registry::default();
        assert!(registry.lease("db").await.start(&db,DbKind::of("sqlite"),"main".into(),"tab".into()).await.is_err());
        crate::sql_history::query("DROP TABLE temp.shadow").execute(&pool).await.unwrap();
        crate::sql_history::query("ATTACH DATABASE ':memory:' AS auxiliary").execute(&pool).await.unwrap();
        assert!(registry.lease("db").await.start(&db,DbKind::of("sqlite"),"main".into(),"tab".into()).await.is_err());
        pool.close().await;
    }
    #[test]
    fn manual_transaction_script_policy_rejects_controls_implicit_commit_and_file_output() {
        for sql in ["BEGIN", "SELECT 1; COMMIT", "ROLLBACK", "SAVEPOINT a", "RELEASE a", "CREATE TABLE x(id INT)",
            "ALTER TABLE x ADD id INT", "TRUNCATE x", "SET autocommit=1", "LOCK TABLES x WRITE", "USE other", "CALL p()", "ATTACH DATABASE 'other' AS other", "SELECT * INTO x FROM y", "SELECT * INTO OUTFILE '/tmp/x' FROM y", "/*!40101 COMMIT */ SELECT 1", "/*M! COMMIT */ SELECT 1", "SELECT set_config('search_path','other',false)", "SELECT \"set_config\"('search_path','other',false)", "SELECT pg_advisory_lock(1)", "SELECT load_extension('external')"] {
            assert!(checked_statements(sql).is_err(), "allowed unsafe script: {sql}");
        }
        for sql in ["UPDATE items SET name='COMMIT' WHERE id=1", "-- COMMIT\nSELECT 'BEGIN;ROLLBACK'", "/* DROP TABLE x */ INSERT INTO items VALUES(1,'a')", "SELECT \"commit\" FROM items", "WITH x AS (SELECT 1) SELECT * FROM x"] {
            assert!(checked_statements(sql).is_ok(), "rejected safe script: {sql}");
        }
    }
    #[tokio::test]
    async fn manual_transaction_drop_rolls_back_and_sqlite_namespace_is_not_faked() {
        let f = Fixture::new().await;
        assert!(f.registry.lease("db").await.start(&f.db,DbKind::of("sqlite"),"temp".into(),"tab-a".into()).await.is_err());
        assert!(f.registry.lease("db").await.start(&f.db,DbKind::of("sqlite"),"main".into(),String::new()).await.is_err());
        let registry = Registry::default();
        registry.lease("owned").await.start(&f.db,DbKind::of("sqlite"),"main".into(),"tab".into()).await.unwrap();
        registry.lease("owned").await.execute(Some("tab"),"main","INSERT INTO items VALUES(1,'drop')").await.unwrap();
        drop(registry);
        // Acquiring/using a pooled connection flushes SQLx's drop-rollback queue.
        if let Db::Sqlite(pool) = &f.db { crate::sql_history::query("SELECT 1").fetch_one(pool).await.unwrap(); }
        assert_eq!(f.count().await,0);
        f.cleanup().await;
    }
}
