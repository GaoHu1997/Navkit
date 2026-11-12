//! 表级 CHECK/触发器的真实读取：精确 schema+parent，不把拒权限或解析失败当空集合。
//! SQLite 的匿名 CHECK 显式标记 generated_name；它不是可删除的真实约束名。

use anyhow::{anyhow, Result};
use serde::Serialize;
use sqlx::{Column, Row};

use crate::db::{self, Db};
use crate::models::{DbKind, Protocol};

#[derive(Debug, Clone, Serialize)]
pub struct ConstraintInfo {
    pub name: String,
    pub definition: String,
    pub generated_name: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TriggerInfo {
    pub name: String,
    pub timing: Option<String>,
    pub event: Option<String>,
    pub statement: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ConstraintSet {
    pub checks: Vec<ConstraintInfo>,
    pub triggers: Vec<TriggerInfo>,
    /// complete | partial | unknown；成功返回不代表目录完整。
    pub completeness: String,
    pub limitations: Vec<String>,
    /// SQLite schema 原文不随 row count/sequence 值变化，保留未建模特征的 CAS 证据。
    pub table_definition: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenKind { Word, Identifier, Literal, Symbol }
#[derive(Debug)]
pub(crate) struct Token<'a> { pub(crate) text: &'a str, pub(crate) start: usize, pub(crate) end: usize, pub(crate) kind: TokenKind }
impl Token<'_> {
    pub(crate) fn word(&self, word: &str) -> bool { self.kind == TokenKind::Word && self.text.eq_ignore_ascii_case(word) }
    pub(crate) fn symbol(&self, symbol: &str) -> bool { self.kind == TokenKind::Symbol && self.text == symbol }
}

/// SQLite 字符串/带引号身份/注释内的 CHECK 与括号都不是语法。
pub(crate) fn tokens(ddl: &str) -> Result<Vec<Token<'_>>> {
    if ddl.len() > 8 * 1024 * 1024 { return Err(anyhow!("TABLE_OBJECT_METADATA_TOO_LARGE: 表定义超过 8 MiB，未发布空 CHECK")); }
    let bytes = ddl.as_bytes();
    let mut i = 0;
    let mut result = Vec::new();
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() { i += 1; continue; }
        if bytes[i..].starts_with(b"--") { while i < bytes.len() && bytes[i] != b'\n' { i += 1; } continue; }
        if bytes[i..].starts_with(b"/*") {
            i += 2;
            while i + 1 < bytes.len() && !bytes[i..].starts_with(b"*/") { i += 1; }
            if i + 1 >= bytes.len() { return Err(anyhow!("TABLE_OBJECT_DDL_INVALID: 未闭合注释，CHECK metadata 未知")); }
            i += 2;
            continue;
        }
        let start = i;
        let (kind, end) = match bytes[i] {
            b'\'' | b'"' | b'`' | b'[' => {
                let opening = bytes[i];
                let closing = if opening == b'[' { b']' } else { opening };
                i += 1;
                loop {
                    if i >= bytes.len() { return Err(anyhow!("TABLE_OBJECT_DDL_INVALID: 未闭合字符串/标识符，CHECK metadata 未知")); }
                    if bytes[i] == closing {
                        i += 1;
                        if opening != b'[' && i < bytes.len() && bytes[i] == closing { i += 1; continue; }
                        break;
                    }
                    i += 1;
                }
                (if opening == b'\'' { TokenKind::Literal } else { TokenKind::Identifier }, i)
            }
            b'(' | b')' | b',' | b';' => { i += 1; (TokenKind::Symbol, i) }
            _ => {
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() && !matches!(bytes[i], b'\'' | b'"' | b'`' | b'[' | b'(' | b')' | b',' | b';') && !bytes[i..].starts_with(b"--") && !bytes[i..].starts_with(b"/*") { i += 1; }
                if i == start { return Err(anyhow!("TABLE_OBJECT_DDL_INVALID: 无法识别的 metadata token")); }
                (TokenKind::Word, i)
            }
        };
        result.push(Token { text: &ddl[start..end], start, end, kind });
        if result.len() > 262144 { return Err(anyhow!("TABLE_OBJECT_METADATA_TOO_LARGE: metadata token 数量超过上限")); }
    }
    Ok(result)
}

pub(crate) fn identifier(token: &Token<'_>) -> Result<String> {
    match token.kind {
        TokenKind::Word => Ok(token.text.to_string()),
        TokenKind::Identifier => {
            let first = token.text.as_bytes()[0];
            let inner = &token.text[1..token.text.len() - 1];
            Ok(match first { b'"' => inner.replace("\"\"", "\""), b'`' => inner.replace("``", "`"), b'[' => inner.to_string(), _ => return Err(anyhow!("TABLE_OBJECT_IDENTIFIER_UNKNOWN")) })
        }
        _ => Err(anyhow!("TABLE_OBJECT_IDENTIFIER_UNKNOWN: CHECK 约束名不是标识符")),
    }
}

pub(crate) fn extract_checks(ddl: &str) -> Result<Vec<ConstraintInfo>> {
    let tokens = tokens(ddl)?;
    let mut total_depth = 0i32;
    for token in &tokens {
        if token.symbol("(") { total_depth += 1; }
        if token.symbol(")") { total_depth -= 1; if total_depth < 0 { return Err(anyhow!("TABLE_OBJECT_DDL_INVALID: DDL 括号未平衡")); } }
    }
    if total_depth != 0 { return Err(anyhow!("TABLE_OBJECT_DDL_INVALID: DDL 括号未闭合，metadata 未知")); }
    let mut result = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if !tokens[i].word("CHECK") { i += 1; continue; }
        if !tokens.get(i + 1).is_some_and(|token| token.symbol("(")) { return Err(anyhow!("TABLE_OBJECT_DDL_INVALID: CHECK 缺少括号，metadata 未知")); }
        let named = i >= 2 && tokens[i - 2].word("CONSTRAINT");
        let name = if named { identifier(&tokens[i - 1])? } else { format!("check_{}", result.len() + 1) };
        let mut depth = 0;
        let mut end = None;
        for (index, token) in tokens.iter().enumerate().skip(i + 1) {
            if token.symbol("(") { depth += 1; }
            if token.symbol(")") { depth -= 1; if depth == 0 { end = Some(index); break; } }
        }
        let end = end.ok_or_else(|| anyhow!("TABLE_OBJECT_DDL_INVALID: CHECK 括号未闭合，metadata 未知"))?;
        result.push(ConstraintInfo { name, definition: ddl[tokens[i].start..tokens[end].end].to_string(), generated_name: !named });
        if result.len() > 4096 { return Err(anyhow!("TABLE_OBJECT_METADATA_TOO_LARGE: CHECK 数量超过 4096")); }
        i = end + 1;
    }
    Ok(result)
}

fn scope(schema: &str, table: &str) -> Result<()> {
    if schema.is_empty() || table.is_empty() || schema.contains(char::from(0)) || table.contains(char::from(0)) { return Err(anyhow!("TABLE_OBJECT_SCOPE_REQUIRED: 必须给精确 schema/table，不能跨 schema 按同名合并")); }
    Ok(())
}

const PG_TRIGGER_SQL: &str = "SELECT t.tgname, pg_get_triggerdef(t.oid) FROM pg_trigger t JOIN pg_class c ON c.oid = t.tgrelid JOIN pg_namespace n ON n.oid = c.relnamespace WHERE c.relname = $1 AND n.nspname = $2 AND NOT t.tgisinternal ORDER BY t.tgname";

/// 与父 C1 owned SQLite 事务共用同一个物理连接；不自行 BEGIN/授权/提交。
pub async fn list_sqlite(conn: &mut sqlx::SqliteConnection, schema: &str, table: &str) -> Result<ConstraintSet> {
    scope(schema, table)?;
    if schema != "main" { return Err(anyhow!("TABLE_OBJECT_NAMESPACE_UNSUPPORTED: 当前 SQLite metadata reader 只认证 main，未读取其它 schema")); }
    let row: Option<(Option<String>,)> = crate::sql_history::query_as("SELECT sql FROM main.sqlite_schema WHERE type = 'table' AND name = ?").bind(table).fetch_optional(&mut *conn).await?;
    let ddl = row.ok_or_else(|| anyhow!("TABLE_OBJECT_NOT_FOUND: 精确 main/table 不存在"))?.0.ok_or_else(|| anyhow!("TABLE_OBJECT_DEFINITION_UNKNOWN: CREATE TABLE metadata 是 NULL"))?;
    let checks = extract_checks(&ddl)?;
    let rows: Vec<(String, Option<String>)> = crate::sql_history::query_as("SELECT name, sql FROM main.sqlite_schema WHERE type = 'trigger' AND tbl_name = ? ORDER BY name").bind(table).fetch_all(&mut *conn).await?;
    if rows.len() > 4096 { return Err(anyhow!("TABLE_OBJECT_METADATA_TOO_LARGE: 触发器数量超过 4096")); }
    let triggers = rows.into_iter().map(|(name, sql)| Ok(TriggerInfo { name, timing: None, event: None, statement: sql.ok_or_else(|| anyhow!("TABLE_OBJECT_DEFINITION_UNKNOWN: trigger 定义为 NULL，不返回空字符串"))? })).collect::<Result<Vec<_>>>()?;
    Ok(ConstraintSet { checks, triggers, completeness: "complete".into(), limitations: Vec::new(), table_definition: Some(ddl) })
}

pub async fn list(db: &Db, kind: &DbKind, schema: &str, table: &str) -> Result<ConstraintSet> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    scope(schema, table)?;
    match (db, kind.protocol()) {
        (Db::Sqlite(pool), Protocol::Sqlite) => {
            let mut conn = pool.acquire().await?;
            list_sqlite(&mut conn, schema, table).await
        }
        (Db::Mysql(pool), Protocol::Mysql) => {
            // 权限/不存在错误保留。SHOW CREATE 成功证明表可读，不证明 TRIGGERS 目录完整。
            let row = crate::sql_history::query(&format!("SHOW CREATE TABLE {}", db::qualified(kind, schema, table))).fetch_one(pool).await?;
            let column = row.columns().iter().position(|column| column.name().eq_ignore_ascii_case("Create Table")).ok_or_else(|| anyhow!("TABLE_OBJECT_DEFINITION_UNKNOWN: SHOW CREATE TABLE 返回列不含 Create Table"))?;
            let definition = row.try_get::<Option<String>, _>(column)?.ok_or_else(|| anyhow!("TABLE_OBJECT_DEFINITION_UNKNOWN: SHOW CREATE TABLE 定义不可见"))?;
            if definition.len() > 8 * 1024 * 1024 { return Err(anyhow!("TABLE_OBJECT_METADATA_TOO_LARGE")); }
            let checks: Vec<(String, Option<String>)> = crate::sql_history::query_as("SELECT tc.CONSTRAINT_NAME, cc.CHECK_CLAUSE FROM information_schema.TABLE_CONSTRAINTS tc JOIN information_schema.CHECK_CONSTRAINTS cc ON cc.CONSTRAINT_SCHEMA = tc.CONSTRAINT_SCHEMA AND cc.CONSTRAINT_NAME = tc.CONSTRAINT_NAME WHERE tc.TABLE_SCHEMA = ? AND tc.TABLE_NAME = ? AND tc.CONSTRAINT_TYPE = 'CHECK' ORDER BY tc.CONSTRAINT_NAME").bind(schema).bind(table).fetch_all(pool).await?;
            let triggers: Vec<(String, String, String, String)> = crate::sql_history::query_as("SELECT TRIGGER_NAME, ACTION_TIMING, EVENT_MANIPULATION, ACTION_STATEMENT FROM information_schema.TRIGGERS WHERE EVENT_OBJECT_SCHEMA = ? AND EVENT_OBJECT_TABLE = ? ORDER BY TRIGGER_NAME").bind(schema).bind(table).fetch_all(pool).await?;
            if checks.len() > 4096 || triggers.len() > 4096 { return Err(anyhow!("TABLE_OBJECT_METADATA_TOO_LARGE")); }
            // 不猜测角色/局部 GRANT；仅当前账户可证的全局 TRIGGER 可确认为完整。
            let privileges: Vec<String> = crate::sql_history::query_scalar("SELECT PRIVILEGE_TYPE FROM information_schema.USER_PRIVILEGES WHERE GRANTEE = CONCAT(QUOTE(SUBSTRING_INDEX(CURRENT_USER(), '@', 1)), '@', QUOTE(SUBSTRING_INDEX(CURRENT_USER(), '@', -1)))").fetch_all(pool).await?;
            let complete = privileges.iter().any(|privilege| privilege.eq_ignore_ascii_case("TRIGGER"));
            Ok(ConstraintSet {
                checks: checks.into_iter().map(|(name, clause)| Ok(ConstraintInfo { name, definition: clause.ok_or_else(|| anyhow!("TABLE_OBJECT_DEFINITION_UNKNOWN: CHECK_CLAUSE 不可见，不能当空"))?, generated_name: false })).collect::<Result<Vec<_>>>()?,
                triggers: triggers.into_iter().map(|(name, timing, event, statement)| TriggerInfo { name, timing: Some(timing), event: Some(event), statement }).collect(),
                completeness: if complete { "complete" } else { "unknown" }.into(),
                limitations: if complete { Vec::new() } else { vec!["MySQL TRIGGERS 可见性受权限过滤；当前角色/局部权限尚未证明全目录完整，可见空集合不是没有触发器".into()] },
                // AUTO_INCREMENT 下一值随写入变化，不能把 SHOW CREATE 的运行期计数放入结构 revision。
                table_definition: None,
            })
        }
        (Db::Postgres(pool), Protocol::Postgres) => {
            let exists: bool = crate::sql_history::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE c.relname = $1 AND n.nspname = $2 AND c.relkind IN ('r','p'))").bind(table).bind(schema).fetch_one(pool).await?;
            if !exists { return Err(anyhow!("TABLE_OBJECT_NOT_FOUND: 精确 PostgreSQL schema/table 不存在或不是基表")); }
            let checks: Vec<(String, String)> = crate::sql_history::query_as("SELECT con.conname, pg_get_constraintdef(con.oid) FROM pg_constraint con JOIN pg_class c ON c.oid = con.conrelid JOIN pg_namespace n ON n.oid = c.relnamespace WHERE con.contype = 'c' AND c.relname = $1 AND n.nspname = $2 ORDER BY con.conname").bind(table).bind(schema).fetch_all(pool).await?;
            let triggers: Vec<(String, String)> = crate::sql_history::query_as(PG_TRIGGER_SQL).bind(table).bind(schema).fetch_all(pool).await?;
            if checks.len() > 4096 || triggers.len() > 4096 { return Err(anyhow!("TABLE_OBJECT_METADATA_TOO_LARGE")); }
            Ok(ConstraintSet { checks: checks.into_iter().map(|(name, definition)| ConstraintInfo { name, definition, generated_name: false }).collect(), triggers: triggers.into_iter().map(|(name, statement)| TriggerInfo { name, timing: None, event: None, statement }).collect(), completeness: "complete".into(), limitations: Vec::new(), table_definition: None })
        }
        (_, Protocol::Sqlite | Protocol::Mysql | Protocol::Postgres) => Err(anyhow!("TABLE_OBJECT_ADAPTER_UNSUPPORTED: 此连接不是相应原生 adapter，未查询，不能 unreachable/panic")),
        _ => Err(anyhow!("当前数据源尚未接入检查约束与触发器读取；没有执行任何查询")),
    }

    }).await
}

/// 生成删除触发器的方言 SQL；此函数只编译，不授权执行。
pub fn drop_trigger_sql(kind: &DbKind, schema: &str, table: &str, name: &str) -> Result<String> {
    scope(schema, table)?;
    if name.is_empty() || name.contains(char::from(0)) { return Err(anyhow!("TABLE_OBJECT_NAME_REQUIRED")); }
    match kind.protocol() {
        Protocol::Sqlite if schema == "main" => Ok(format!("DROP TRIGGER {}", db::quote_ident(kind, name))),
        Protocol::Sqlite => Err(anyhow!("TABLE_OBJECT_NAMESPACE_UNSUPPORTED")),
        Protocol::Mysql => Ok(format!("DROP TRIGGER {}", db::qualified(kind, schema, name))),
        Protocol::Postgres => Ok(format!("DROP TRIGGER {} ON {}", db::quote_ident(kind, name), db::qualified(kind, schema, table))),
        _ => Err(anyhow!("当前数据源不支持删除触发器；已拒绝执行")),
    }
}

/// 兼容执行 helper；父必须先持 C1/只读/事务 gate。验证给定 trigger 属于这个 parent。
pub async fn drop_trigger(db: &Db, kind: &DbKind, schema: &str, table: &str, name: &str) -> Result<u64> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let sql = drop_trigger_sql(kind, schema, table, name)?;
    let objects = list(db, kind, schema, table).await?;
    if !objects.triggers.iter().any(|trigger| trigger.name == name) { return Err(anyhow!("TABLE_TRIGGER_PARENT_CHANGED: trigger 不属于精确 schema/table，未 DROP")); }
    let affected = match db {
        Db::Sqlite(pool) => crate::sql_history::query(&format!("DROP TRIGGER {}.{}", db::quote_ident(kind, "main"), db::quote_ident(kind, name))).execute(pool).await?.rows_affected(),
        Db::Mysql(pool) => crate::sql_history::query(&sql).execute(pool).await?.rows_affected(),
        Db::Postgres(pool) => crate::sql_history::query(&sql).execute(pool).await?.rows_affected(),
        _ => return Err(anyhow!("当前数据源不支持删除触发器；已拒绝执行")),
    };
    Ok(affected)

    }).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_balanced_checks() {
        let ddl = "CREATE TABLE t (id integer, qty integer CHECK (qty > 0), name text CHECK (length(name) > 1))";
        let checks = extract_checks(ddl).unwrap();
        assert_eq!(checks.len(), 2);
        assert!(checks[0].definition.contains("qty > 0"));
        assert!(checks[1].definition.contains("length(name) > 1"));
    }

    #[test]
    fn drop_trigger_is_dialect_specific() {
        let sqlite = drop_trigger_sql(&DbKind::of("sqlite"), "main", "t", "tr").unwrap();
        assert_eq!(sqlite, "DROP TRIGGER \"tr\"");
        let pg = drop_trigger_sql(&DbKind::of("postgresql"), "public", "t", "tr").unwrap();
        assert!(pg.contains("ON \"public\".\"t\""));
    }

    #[test]
    fn rejects_unknown_protocol_drop() { assert!(drop_trigger_sql(&DbKind::of("mssql"), "dbo", "t", "tr").is_err()); }

    #[test]
    fn check_literals_quotes_comments_and_named_identity_are_lossless() {
        let ddl = "CREATE TABLE t (\"check(dummy)\" TEXT DEFAULT 'CHECK(nope)', note TEXT CONSTRAINT \"约束 A\" CHECK (note IN ('a)b', '中 CHECK(值)')) /* CHECK(false) */, n INT CHECK (n > 0 -- ) CHECK(fake)\n))";
        let checks = extract_checks(ddl).unwrap();
        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].name, "约束 A");
        assert!(!checks[0].generated_name);
        assert!(checks[0].definition.contains("'a)b'"));
        assert!(checks[0].definition.contains("'中 CHECK(值)'"));
        assert!(checks[1].generated_name);
    }

    #[test]
    fn malformed_check_is_error_not_known_empty() {
        assert!(extract_checks("CREATE TABLE t (n CHECK (n > 0)").is_err());
        assert!(extract_checks("CREATE TABLE t (n CHECK (n > 0").is_err());
        assert!(extract_checks("CREATE TABLE t (s DEFAULT 'CHECK(n)").is_err());
        assert!(extract_checks("CREATE TABLE t (n CHECK /* unterminated").is_err());
        assert!(extract_checks("CREATE TABLE t (n CHECKING > 0)").unwrap().is_empty());
    }

    #[test]
    fn postgres_trigger_query_requires_namespace_and_parent() {
        assert!(PG_TRIGGER_SQL.contains("JOIN pg_namespace n ON n.oid = c.relnamespace"));
        assert!(PG_TRIGGER_SQL.contains("c.relname = $1 AND n.nspname = $2"));
        assert!(drop_trigger_sql(&DbKind::of("postgresql"), "", "t", "tr").is_err());
    }

    #[tokio::test]
    async fn sqlite_real_check_and_trigger_parent_metadata() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        crate::sql_history::query("CREATE TABLE t (id INTEGER, note TEXT CONSTRAINT ck_note CHECK (note <> 'A )B'))").execute(&pool).await.unwrap();
        crate::sql_history::query("CREATE TABLE other (id INTEGER)").execute(&pool).await.unwrap();
        crate::sql_history::query("CREATE TRIGGER tr_t AFTER INSERT ON t BEGIN SELECT 1; END").execute(&pool).await.unwrap();
        let db = Db::Sqlite(pool.clone());
        let objects = list(&db, &DbKind::of("sqlite"), "main", "t").await.unwrap();
        assert_eq!(objects.completeness, "complete");
        assert_eq!(objects.checks[0].name, "ck_note");
        assert!(objects.checks[0].definition.contains("'A )B'"));
        assert_eq!(objects.triggers.len(), 1);
        assert_eq!(objects.triggers[0].name, "tr_t");
        assert!(list(&db, &DbKind::of("sqlite"), "main", "other").await.unwrap().triggers.is_empty());
        assert!(drop_trigger(&db, &DbKind::of("sqlite"), "main", "other", "tr_t").await.is_err());
        assert!(list(&db, &DbKind::of("sqlite"), "aux", "t").await.is_err());
        assert!(list(&db, &DbKind::of("mysql"), "main", "t").await.is_err());
        assert!(list(&db, &DbKind::of("sqlite"), "main", "missing").await.is_err());
        let count: i64 = crate::sql_history::query_scalar("SELECT count(*) FROM main.sqlite_schema WHERE type='trigger' AND name='tr_t'").fetch_one(&pool).await.unwrap();
        assert_eq!(count, 1);
        pool.close().await;
    }
}
