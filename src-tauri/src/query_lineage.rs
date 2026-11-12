//! PL21: a deliberately small, fully consumed SELECT AST plus real SQLite origin evidence.
//! Called only inside the already authorized C1 execution. Never retries or executes a second SELECT.
use std::collections::HashSet;
use std::ffi::{CStr, CString};
use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Connection, Row, TypeInfo, ValueRef};
use crate::models::{ColumnInfo, IndexInfo, TableSchema};
use crate::mutation::DbValue;

pub const TOKEN_PREFIX_ROWS: usize = 10_000;
pub const TOKEN_PREFIX_BYTES: usize = 16 * 1024 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SqliteSource {
    pub schema: String, pub table: String, pub schema_cookie: i64,
    pub rootpage: i64, pub metadata_revision: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct OriginColumn {
    pub result_column_id: String, pub result_index: usize, pub name: String,
    pub base_column: String, pub data_type: String, pub writable: bool,
    pub key: bool, pub generated: bool, pub reason: String,
    /// Insert metadata is copied from the same physical schema snapshot, independently of update/key flags.
    pub insertable: bool, pub nullable: bool, pub auto_increment: bool, pub default_value: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ProvenSelect {
    pub source: SqliteSource, pub metadata: TableSchema, pub columns: Vec<OriginColumn>,
    pub key_column_ids: Vec<String>, pub delete_supported: bool, pub insert_supported: bool,
}
#[derive(Debug, Clone)]
pub(crate) struct CaptureLimits { pub preview_rows: usize, pub preview_bytes: usize, pub token_rows: usize, pub token_bytes: usize }
impl Default for CaptureLimits {
    fn default() -> Self { Self { preview_rows: 10_000, preview_bytes: 50 * 1024 * 1024, token_rows: TOKEN_PREFIX_ROWS, token_bytes: TOKEN_PREFIX_BYTES } }
}
#[derive(Debug)]
pub(crate) struct CapturedSelect {
    pub names: Vec<String>, pub types: Vec<String>, pub display_rows: Vec<Vec<Option<String>>>,
    /// Exactly the loaded prefix; never decoded from display strings.
    pub typed_rows: Vec<Vec<DbValue>>, pub total: u64, pub proof: Option<ProvenSelect>, pub read_only_reason: String,
}
#[derive(Debug)]
pub(crate) enum CaptureAttempt { NotEligible(String), Executed(CapturedSelect) }
#[derive(Debug, Clone)]
pub(crate) struct SelectAst { pub table: String, pub alias: Option<String>, projection: Vec<Projection>, refs: Vec<Vec<String>> }
#[derive(Debug, Clone)]
enum Projection { Star(Option<String>), Column(Vec<String>) }
#[derive(Debug, Clone, PartialEq)]
enum Token { Word(String, bool), Literal, Number(String), Symbol(String) }
struct Parser { tokens: Vec<Token>, i: usize, refs: Vec<Vec<String>>, depth: usize }
fn keyword(token: Option<&Token>, word: &str) -> bool { matches!(token, Some(Token::Word(s, false)) if s.eq_ignore_ascii_case(word)) }
fn reserved(s: &str) -> bool {
    ["SELECT","FROM","WHERE","ORDER","BY","LIMIT","OFFSET","AS","AND","OR","NOT","IS","NULL","TRUE","FALSE","ASC","DESC","NULLS","FIRST","LAST","IN","BETWEEN","LIKE","GLOB","ESCAPE","JOIN","LEFT","RIGHT","INNER","OUTER","CROSS","NATURAL","ON","USING","GROUP","HAVING","WINDOW","UNION","INTERSECT","EXCEPT","RETURNING","DISTINCT","ALL","COLLATE","INDEXED","WITH"].iter().any(|v| s.eq_ignore_ascii_case(v))
}
fn lex(sql: &str) -> Result<Vec<Token>> {
    if sql.len() > 1024 * 1024 || sql.contains('\0') { bail!("SQL 超过来源解析限额或含 NUL"); }
    let chars: Vec<char> = sql.chars().collect(); let mut i = 0; let mut tokens = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() { i += 1; continue; }
        if c == '-' && chars.get(i + 1) == Some(&'-') { i += 2; while i < chars.len() && chars[i] != '\n' { i += 1; } continue; }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2; while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') { i += 1; }
            if i + 1 >= chars.len() { bail!("未结束的 SQL 注释"); } i += 2; continue;
        }
        if matches!(c, '\'' | '"' | '`' | '[') {
            let end = if c == '[' { ']' } else { c }; let mut value = String::new(); i += 1; let mut closed = false;
            while i < chars.len() {
                if chars[i] == end {
                    if c != '[' && chars.get(i + 1) == Some(&end) { value.push(end); i += 2; continue; }
                    i += 1; closed = true; break;
                }
                value.push(chars[i]); i += 1;
            }
            if !closed { bail!("未结束的 SQL 引号"); }
            tokens.push(if c == '\'' { Token::Literal } else { Token::Word(value, true) });
        } else if c.is_ascii_digit() {
            let start = i; i += 1; while i < chars.len() && chars[i].is_ascii_digit() { i += 1; }
            if chars.get(i) == Some(&'.') { i += 1; while i < chars.len() && chars[i].is_ascii_digit() { i += 1; } }
            if matches!(chars.get(i), Some('e' | 'E')) { i += 1; if matches!(chars.get(i), Some('+' | '-')) { i += 1; } let start_exp = i; while i < chars.len() && chars[i].is_ascii_digit() { i += 1; } if start_exp == i { bail!("不完整的数字指数"); } }
            tokens.push(Token::Number(chars[start..i].iter().collect()));
        } else if c == '_' || c.is_alphabetic() || !c.is_ascii() {
            let start = i; i += 1; while i < chars.len() && (chars[i] == '_' || chars[i] == '$' || chars[i].is_alphanumeric() || !chars[i].is_ascii()) { i += 1; }
            tokens.push(Token::Word(chars[start..i].iter().collect(), false));
        } else {
            let mut symbol = c.to_string(); i += 1;
            if matches!(c, '<' | '>' | '!' | '=') && chars.get(i).is_some_and(|n| *n == '=' || c == '<' && *n == '>') { symbol.push(chars[i]); i += 1; }
            if ![".",",","*","(",")",";","=","==","!=","<>","<",">","<=",">=","+","-"].contains(&symbol.as_str()) { bail!("来源解析未认证该 SQL 运算符"); }
            tokens.push(Token::Symbol(symbol));
        }
        if tokens.len() > 16_384 { bail!("SQL 来源 AST 超过节点限额"); }
    }
    Ok(tokens)
}
impl Parser {
    fn kw(&mut self, s: &str) -> bool { if keyword(self.tokens.get(self.i), s) { self.i += 1; true } else { false } }
    fn sym(&mut self, s: &str) -> bool { if self.tokens.get(self.i) == Some(&Token::Symbol(s.into())) { self.i += 1; true } else { false } }
    fn need(&mut self, s: &str) -> Result<()> { if self.kw(s) { Ok(()) } else { bail!("只认证单表直接投影 SELECT，缺少 {s}") } }
    fn ident(&mut self) -> Result<String> {
        if let Some(Token::Word(s, quoted)) = self.tokens.get(self.i) { if *quoted || !reserved(s) { let s = s.clone(); self.i += 1; if !s.is_empty() { return Ok(s); } } }
        bail!("SQL 需要明确的标识符，表达式/函数/子查询未认证")
    }
    fn path(&mut self) -> Result<Vec<String>> { let mut v = vec![self.ident()?]; while self.sym(".") { v.push(self.ident()?); if v.len() > 3 { bail!("标识符层级过多"); } } Ok(v) }
    fn operand(&mut self) -> Result<()> {
        if self.kw("NULL") || self.kw("TRUE") || self.kw("FALSE") { return Ok(()); }
        let signed = self.sym("+") || self.sym("-");
        if matches!(self.tokens.get(self.i), Some(Token::Number(_))) { self.i += 1; return Ok(()); }
        if signed { bail!("仅数字文字允许正负号"); }
        if self.tokens.get(self.i) == Some(&Token::Literal) { self.i += 1; return Ok(()); }
        let path = self.path()?; self.refs.push(path); Ok(())
    }
    fn predicate(&mut self) -> Result<()> {
        self.depth += 1; if self.depth > 32 { bail!("SQL 布尔表达式过深"); }
        if self.kw("NOT") { self.predicate()?; }
        else if self.sym("(") { self.boolean()?; if !self.sym(")") { bail!("布尔括号未闭合"); } }
        else {
            self.operand()?;
            if self.kw("IS") { self.kw("NOT"); if !self.kw("NULL") { self.operand()?; } }
            else if ["=","==","!=","<>","<",">","<=",">="].iter().any(|s| self.sym(s)) { self.operand()?; }
            else {
                let negated = self.kw("NOT");
                if self.kw("BETWEEN") { self.operand()?; self.need("AND")?; self.operand()?; }
                else if self.kw("IN") { if !self.sym("(") { bail!("IN 只认证文字列表"); } self.operand()?; while self.sym(",") { self.operand()?; } if !self.sym(")") { bail!("IN 列表未闭合"); } }
                else if self.kw("LIKE") || self.kw("GLOB") { self.operand()?; if self.kw("ESCAPE") { self.operand()?; } }
                else if negated { bail!("NOT 后缺少已认证运算符"); }
            }
        }
        self.depth -= 1; Ok(())
    }
    fn conjunction(&mut self) -> Result<()> { self.predicate()?; while self.kw("AND") { self.predicate()?; } Ok(()) }
    fn boolean(&mut self) -> Result<()> { self.conjunction()?; while self.kw("OR") { self.conjunction()?; } Ok(()) }
    fn limit(&mut self) -> Result<()> { if let Some(Token::Number(s)) = self.tokens.get(self.i) { if s.parse::<u64>().is_ok() { self.i += 1; return Ok(()); } } bail!("LIMIT/OFFSET 仅认证非负整数文字") }
}
pub(crate) fn classify(sql: &str) -> Result<SelectAst, String> {
    (|| -> Result<SelectAst> {
        let mut p = Parser { tokens: lex(sql)?, i: 0, refs: Vec::new(), depth: 0 }; p.need("SELECT")?;
        let mut projection = Vec::new();
        loop {
            if p.sym("*") { projection.push(Projection::Star(None)); }
            else {
                let first = p.ident()?;
                if p.sym(".") {
                    if p.sym("*") { projection.push(Projection::Star(Some(first))); }
                    else { let mut path = vec![first, p.ident()?]; if p.sym(".") { path.push(p.ident()?); } projection.push(Projection::Column(path)); }
                } else { projection.push(Projection::Column(vec![first])); }
                if p.kw("AS") { p.ident()?; } else if matches!(p.tokens.get(p.i), Some(Token::Word(s, q)) if *q || !reserved(s)) { p.ident()?; }
            }
            if !p.sym(",") { break; }
        }
        p.need("FROM")?; let path = p.path()?;
        let table = match path.as_slice() { [t] => t.clone(), [schema, t] if schema.eq_ignore_ascii_case("main") => t.clone(), _ => bail!("仅认证 main 实体表，附加库/多层对象保持只读") };
        let alias = if p.kw("AS") || matches!(p.tokens.get(p.i), Some(Token::Word(s, q)) if *q || !reserved(s)) { Some(p.ident()?) } else { None };
        if p.kw("WHERE") { p.boolean()?; }
        if p.kw("ORDER") { p.need("BY")?; loop { let path = p.path()?; p.refs.push(path); if !p.kw("ASC") { p.kw("DESC"); } if p.kw("NULLS") && !(p.kw("FIRST") || p.kw("LAST")) { bail!("NULLS 缺少位置"); } if !p.sym(",") { break; } } }
        if p.kw("LIMIT") { p.limit()?; if p.sym(",") || p.kw("OFFSET") { p.limit()?; } }
        p.sym(";"); if p.i != p.tokens.len() { bail!("查询含 JOIN/聚合/表达式/CTE/组合结果或未消费语法，保持只读"); }
        let valid_qualifier = |parts: &[String]| match parts { [_] => true, [q, _] => q.eq_ignore_ascii_case(alias.as_deref().unwrap_or(&table)), [db, t, _] => db.eq_ignore_ascii_case("main") && alias.is_none() && t.eq_ignore_ascii_case(&table), _ => false };
        for item in &projection { match item { Projection::Column(path) if !valid_qualifier(path) => bail!("投影限定符与实际基表不一致"), Projection::Star(Some(q)) if !q.eq_ignore_ascii_case(alias.as_deref().unwrap_or(&table)) => bail!("星号限定符与实际基表不一致"), _ => {} } }
        if p.refs.iter().any(|path| !valid_qualifier(path)) { bail!("筛选或排序来源不是同一基表"); }
        Ok(SelectAst { table, alias, projection, refs: p.refs })
    })().map_err(|error| error.to_string())
}
fn qi(s: &str) -> String { format!("\"{}\"", s.replace('"', "\"\"")) }
async fn source(conn: &mut sqlx::SqliteConnection, table: &str) -> Result<SqliteSource> {
    let cookie: i64 = crate::sql_history::query_scalar("PRAGMA main.schema_version").fetch_one(&mut *conn).await?;
    let row: (String, i64, String) = crate::sql_history::query_as("SELECT type,rootpage,sql FROM main.sqlite_master WHERE name=? AND type IN ('table','view')").bind(table).fetch_one(&mut *conn).await?;
    if row.0 != "table" || row.1 <= 0 || table.to_ascii_lowercase().starts_with("sqlite_") || row.2.to_ascii_uppercase().contains("VIRTUAL TABLE") { bail!("视图/虚表/系统表没有可写的实体表来源"); }
    let rows = crate::sql_history::query("SELECT type, name, sql FROM main.sqlite_master WHERE (tbl_name = ? OR name = ?) AND type IN ('table','index','trigger') ORDER BY type, name").bind(table).bind(table).fetch_all(&mut *conn).await?;
    let values: Vec<_> = rows.iter().map(|r| crate::values::sqlite_row_values(r, 3)).collect();
    Ok(SqliteSource { schema: "main".into(), table: table.into(), schema_cookie: cookie, rootpage: row.1, metadata_revision: format!("{:x}", Sha256::digest(serde_json::to_vec(&values)?)) })
}
/// Must be called on the C2 owned transaction after its schema read lock, before any DML.
/// A separate pool read does not establish this guarantee.
pub(crate) async fn validate_source(conn: &mut sqlx::SqliteConnection, expected: &SqliteSource) -> Result<()> {
    if expected.schema != "main" || source(conn, &expected.table).await? != *expected { bail!("QUERY_SOURCE_CHANGED：查询来源的 schema generation/rootpage/结构版本已变化，未发送写入"); } Ok(())
}
async fn table_metadata(conn: &mut sqlx::SqliteConnection, table: &str) -> Result<(TableSchema, HashSet<String>)> {
    let rows = crate::sql_history::query(&format!("PRAGMA main.table_xinfo({})", qi(table))).fetch_all(&mut *conn).await?;
    if rows.is_empty() || rows.len() > 1024 { bail!("实体表列元数据缺失或超过限额"); }
    let index_rows = crate::sql_history::query(&format!("PRAGMA main.index_list({})", qi(table))).fetch_all(&mut *conn).await?;
    let has_primary_index = index_rows.iter().any(|row| row.try_get::<String,_>("origin").ok().as_deref() == Some("pk"));
    let primary_count = rows.iter().filter(|r| r.try_get::<i64,_>("pk").unwrap_or(0) > 0).count();
    let mut generated = HashSet::new(); let mut columns = Vec::new(); let mut primary = Vec::new();
    for row in rows {
        let name: String = row.try_get("name")?; let data_type: String = row.try_get("type")?; let pk: i64 = row.try_get("pk")?; let hidden: i64 = row.try_get("hidden")?;
        if hidden != 0 { generated.insert(name.clone()); }
        // INTEGER PRIMARY KEY DESC has a separate index and is not a rowid/auto-generated alias.
        let auto = primary_count == 1 && pk > 0 && data_type.eq_ignore_ascii_case("INTEGER") && !has_primary_index;
        if pk > 0 { primary.push((pk, name.clone())); }
        columns.push(ColumnInfo { name, data_type, nullable: row.try_get::<i64,_>("notnull")? == 0 && !auto, default: row.try_get("dflt_value")?, primary_key: pk > 0, auto_increment: auto, comment: None, length: None, precision: None, scale: None });
    }
    let mut indexes = Vec::new(); primary.sort_by_key(|(ordinal,_)| *ordinal);
    if !primary.is_empty() { indexes.push(IndexInfo { name: "__query_primary".into(), columns: primary.into_iter().map(|(_,name)| name).collect(), unique: true, primary: true, method: None }); }
    for row in index_rows {
        if row.try_get::<i64,_>("unique")? == 0 || row.try_get::<i64,_>("partial")? != 0 { continue; }
        let name: String = row.try_get("name")?; let parts = crate::sql_history::query(&format!("PRAGMA main.index_xinfo({})", qi(&name))).fetch_all(&mut *conn).await?;
        let mut cols = Vec::new(); let mut valid = true;
        for part in parts { if part.try_get::<i64,_>("key")? == 0 { continue; } let cid: i64 = part.try_get("cid")?; let col: Option<String> = part.try_get("name")?; let coll: Option<String> = part.try_get("coll")?;
            if cid < 0 || col.is_none() || coll.as_deref() != Some("BINARY") { valid = false; break; } cols.push(col.expect("checked")); }
        if valid && !cols.is_empty() { indexes.push(IndexInfo { name, columns: cols, unique: true, primary: false, method: None }); }
    }
    Ok((TableSchema { table: table.into(), database: "main".into(), columns, indexes, foreign_keys: Vec::new(), comment: None, engine: None, collation: None, row_count: None }, generated))
}
struct PreparedOrigins { names: Vec<String>, types: Vec<String>, origins: Vec<(String,String,String)> }
async fn prepared_origins(conn: &mut sqlx::SqliteConnection, sql: &str) -> Result<PreparedOrigins> {
    let mut handle = conn.lock_handle().await.context("来源 handle 获取失败；禁止另执行查询来恢复证据")?;
    let sql = CString::new(sql)?;
    // SAFETY: SQLx worker is locked; stmt is finalized synchronously before releasing it.
    unsafe {
        let db = handle.as_raw_handle().as_ptr(); let mut stmt = std::ptr::null_mut(); let mut tail = std::ptr::null();
        let rc = libsqlite3_sys::sqlite3_prepare_v2(db, sql.as_ptr(), -1, &mut stmt, &mut tail);
        if rc != libsqlite3_sys::SQLITE_OK || stmt.is_null() { if !stmt.is_null() { libsqlite3_sys::sqlite3_finalize(stmt); } bail!("SQLite 无法 prepare 原 SELECT，查询未执行；禁止回退重跑"); }
        struct Finalize(*mut libsqlite3_sys::sqlite3_stmt); impl Drop for Finalize { fn drop(&mut self) { unsafe { libsqlite3_sys::sqlite3_finalize(self.0); } } }
        let _guard = Finalize(stmt); if libsqlite3_sys::sqlite3_stmt_readonly(stmt) == 0 { bail!("来源语句不是 SQLite 认证的只读 SELECT"); }
        let count = libsqlite3_sys::sqlite3_column_count(stmt); if !(1..=1024).contains(&count) { bail!("来源结果列数超出限额"); }
        let string = |ptr: *const std::ffi::c_char| -> Result<String> { if ptr.is_null() { Ok(String::new()) } else { Ok(CStr::from_ptr(ptr).to_str()?.into()) } };
        let mut result = PreparedOrigins { names: Vec::new(), types: Vec::new(), origins: Vec::new() };
        for index in 0..count {
            result.names.push(string(libsqlite3_sys::sqlite3_column_name(stmt,index))?);
            result.types.push(string(libsqlite3_sys::sqlite3_column_decltype(stmt,index))?);
            result.origins.push((string(libsqlite3_sys::sqlite3_column_database_name(stmt,index))?, string(libsqlite3_sys::sqlite3_column_table_name(stmt,index))?, string(libsqlite3_sys::sqlite3_column_origin_name(stmt,index))?));
        }
        Ok(result)
    }
}
fn make_proof(ast: &SelectAst, prepared: &PreparedOrigins, source: SqliteSource, metadata: TableSchema, generated: &HashSet<String>) -> Result<ProvenSelect> {
    let mut bases = HashSet::new(); let mut columns = Vec::new();
    for (index, (database, table, origin)) in prepared.origins.iter().enumerate() {
        if database != "main" || !table.eq_ignore_ascii_case(&ast.table) || origin.is_empty() || !bases.insert(origin.to_ascii_lowercase()) { bail!("SQLite 实际来源含视图/附加库/表达式/重复投影，不能编辑"); }
        let col = metadata.columns.iter().find(|col| col.name == *origin).ok_or_else(|| anyhow!("实际列来源不在同一元数据快照"))?;
        let generated = generated.contains(origin); let supported = writable_decl(&col.data_type);
        columns.push(OriginColumn { result_column_id: uuid::Uuid::new_v4().to_string(), result_index: index, name: prepared.names[index].clone(), base_column: origin.clone(), data_type: col.data_type.clone(), writable: !generated && supported, key: false, generated, reason: if generated { "生成列保持只读".into() } else if !supported { "该 SQLite 声明类型未验证无损写入".into() } else { String::new() }, insertable: !generated && !col.auto_increment && supported, nullable: col.nullable, auto_increment: col.auto_increment, default_value: col.default.clone() });
    }
    let key = metadata.indexes.iter().filter(|key| key.unique).find(|key| key.columns.iter().all(|name| columns.iter().any(|col| col.base_column == *name && writable_decl(&col.data_type)))).ok_or_else(|| anyhow!("结果未投影完整主键/候选唯一键；需明确确认后重新查询，不能追加隐藏 SELECT 猜测原行"))?;
    let mut ids = Vec::new(); for name in &key.columns { let col = columns.iter_mut().find(|c| c.base_column == *name).expect("selected key"); col.key = true; col.writable = false; col.reason = "定位键在查询结果中保持只读".into(); ids.push(col.result_column_id.clone()); }
    let complete = metadata.columns.iter().all(|base| writable_decl(&base.data_type) && columns.iter().any(|col| col.base_column == base.name));
    let insert = metadata.columns.iter().filter(|col| !generated.contains(&col.name) && !col.nullable && col.default.is_none() && !col.auto_increment).all(|base| columns.iter().any(|col| col.base_column == base.name && writable_decl(&col.data_type)));
    Ok(ProvenSelect { source, metadata, columns, key_column_ids: ids, delete_supported: complete, insert_supported: insert })
}
fn writable_decl(s: &str) -> bool { let s = s.to_ascii_lowercase(); let base = s.split('(').next().unwrap_or("").trim(); matches!(base,"integer"|"int"|"bigint"|"smallint"|"tinyint"|"int2"|"int4"|"int8"|"real"|"float"|"double"|"double precision"|"blob"|"binary"|"varbinary") || ["text","char","clob"].iter().any(|part| base.contains(part)) }
fn typed_row(row: &sqlx::sqlite::SqliteRow, count: usize) -> Result<Vec<DbValue>> {
    (0..count).map(|index| {
        let raw = row.try_get_raw(index)?; if raw.is_null() { return Ok(DbValue::Null); }
        Ok(match raw.type_info().name() {
            "INTEGER" => DbValue::Integer(row.try_get::<i64,_>(index)?.to_string()),
            "REAL" => { let n = row.try_get::<f64,_>(index)?; if !n.is_finite() { bail!("非有限浮点结果不能签发行令牌"); } DbValue::Float(n) },
            "TEXT" => DbValue::Text(row.try_get::<String,_>(index)?),
            "BLOB" => DbValue::Binary(STANDARD.encode(row.try_get::<Vec<u8>,_>(index)?)),
            _ => bail!("结果存储类型尚未认证；保持只读，禁止从显示文本反推原值"),
        })
    }).collect()
}
/// NotEligible means *no* user statement was sent and the caller may use its ordinary C1 reader.
/// Executed/Err must NEVER fall back/retry. Metadata failure still reads the original SELECT once.
/// Callback receives every display row, including rows after both bounded prefixes; use existing spool.
pub(crate) async fn capture_sqlite_select<F>(conn: &mut sqlx::SqliteConnection, sql: &str, limits: CaptureLimits, mut on_row: F) -> Result<CaptureAttempt>
where F: FnMut(&[String], &[String], &[Option<String>]) -> Result<()> {
    let ast = match classify(sql) { Ok(ast) => ast, Err(reason) => return Ok(CaptureAttempt::NotEligible(reason)) };
    if limits.preview_rows > 10_000 || limits.preview_bytes > 50 * 1024 * 1024 || limits.token_rows > TOKEN_PREFIX_ROWS || limits.token_bytes > TOKEN_PREFIX_BYTES { bail!("查询捕获前缀超出服务端限额"); }
    // SQLx transaction RAII queues rollback on cancellation; C1 supervise must retain the future/connection until cleanup.
    let mut tx = conn.begin().await?;
    let metadata = async { let before = source(&mut *tx, &ast.table).await?; let (metadata, generated) = table_metadata(&mut *tx, &ast.table).await?; Ok::<_,anyhow::Error>((before, metadata, generated)) }.await;
    let prepared = prepared_origins(&mut *tx, sql).await?;
    let mut reason = String::new();
    let mut proof = match metadata { Ok((source,metadata,generated)) => match make_proof(&ast,&prepared,source,metadata,&generated) { Ok(proof) => Some(proof), Err(e) => { reason=e.to_string(); None } }, Err(e) => { reason=format!("来源元数据不可验证：{e}"); None } };
    let mut display_rows = Vec::new(); let mut typed_rows = Vec::new(); let mut display_bytes = 0usize; let mut token_bytes = 0usize; let mut token_full = false; let mut display_full = false; let mut total = 0u64;
    {
        let mut rows = crate::sql_history::query(sql).fetch(&mut *tx);
        while let Some(row) = rows.try_next().await? {
            total = total.checked_add(1).ok_or_else(|| anyhow!("查询行数溢出"))?;
            let typed = if proof.is_some() && !token_full { Some(typed_row(&row,prepared.names.len())) } else { None };
            let display = crate::values::sqlite_row_values(&row, prepared.names.len()); on_row(&prepared.names,&prepared.types,&display)?;
            if !display_full { let bytes = serde_json::to_vec(&display)?.len(); if display_rows.len() >= limits.preview_rows || display_bytes.saturating_add(bytes) > limits.preview_bytes { display_full=true; } else { display_bytes += bytes; display_rows.push(display); } }
            if let Some(typed) = typed {
                match typed {
                    Ok(values) => { let bytes=serde_json::to_vec(&values)?.len(); if typed_rows.len() >= limits.token_rows || token_bytes.saturating_add(bytes)>limits.token_bytes { token_full=true; } else { token_bytes+=bytes; typed_rows.push(values); } },
                    Err(e) => { proof=None; typed_rows.clear(); reason=e.to_string(); }
                }
            }
            if total % 128 == 0 { tokio::task::yield_now().await; }
        }
    }
    if let Some(candidate) = &proof {
        let key_indexes: Vec<_> = candidate.key_column_ids.iter().map(|id| candidate.columns.iter().position(|c| c.result_column_id==*id).expect("proof key")).collect();
        if typed_rows.iter().any(|row| key_indexes.iter().any(|i| matches!(row[*i],DbValue::Null))) { proof=None; typed_rows.clear(); reason="定位键含 NULL，不能签发唯一行令牌".into(); }
    }
    if let Some(candidate) = &proof { if validate_source(&mut *tx,&candidate.source).await.is_err() { proof=None; typed_rows.clear(); reason="读取前后 schema generation 已变化，结果保持只读".into(); } }
    tx.rollback().await.context("查询捕获读事务清理失败；连接必须隔离且禁止重新执行原查询")?;
    Ok(CaptureAttempt::Executed(CapturedSelect { names: prepared.names, types: prepared.types, display_rows, typed_rows, total, proof, read_only_reason: reason }))
}

#[cfg(test)]
#[path="query_lineage_tests.rs"]
mod tests;
