//! PL-10 的原生 SQLite owned 元数据。所有 PRAGMA/目录读取都在调用方持有的同一连接上。
//! 不借 pool 读表结构，不把权限错误、缺列、表达式索引或未知特征降级为空元数据。

use std::{collections::{BTreeMap, BTreeSet}, path::{Path, PathBuf}, sync::Arc};
use anyhow::{anyhow, ensure, Result};
use serde::Serialize;
use sqlx::{ConnectOptions, Row, Sqlite, SqliteConnection, SqlitePool, pool::PoolConnection, sqlite::SqliteConnectOptions};
use crate::{db::Db, error::{AppError, CmdResult}, execution::Entry, manual_transaction::OrdinaryLease,
    models::{ColumnInfo, DbKind, ForeignKeyInfo, IndexInfo, Protocol, TableSchema},
    structure_sync::{self, StructureDiffReport, StructureReviewBinding, StructureSyncRequest}, table_objects::{self, ConstraintSet}};

const MAX_COLUMNS: usize = 4096;
const MAX_INDEXES: usize = 512;
const MAX_KEY_PARTS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PhysicalDatabaseIdentity {
    pub kind: String,
    pub identity: String,
    pub canonical_path: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PhysicalTableIdentity {
    pub database: PhysicalDatabaseIdentity,
    pub schema: String,
    pub table: String,
    pub rootpage: i64,
}
#[derive(Debug, Clone, Serialize)]
pub struct OwnedTableSnapshot {
    pub schema: TableSchema,
    pub objects: ConstraintSet,
    /// opaque CAS：真实表证据 + 物理库身份/rootpage/schema generation；不含 row count。
    pub revision: String,
    pub identity: PhysicalTableIdentity,
    pub schema_version: i64,
    /// 同文件目标 DDL 合法改变全库 schema_version 时，用此值另验源表本身没有改变。
    #[serde(skip)]
    pub base_revision: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct StructurePreflight {
    #[serde(flatten)]
    pub report: StructureDiffReport,
    pub source_identity: PhysicalTableIdentity,
    pub target_identity: Option<PhysicalTableIdentity>,
    pub same_database: bool,
    pub source_consistency: String,
    /// 仅非空可执行 review 有 C1 marker；读回观察绝不伪造空 DDL 执行计划。
    pub request_sql: String,
    pub observation_digest: String,
    pub observation_binding: StructureReviewBinding,
    pub observation_only: bool,
}

pub(crate) fn ident(name: &str) -> Result<String> {
    ensure!(!name.is_empty() && name.len() <= 1024 && !name.contains(char::from(0)), "STRUCTURE_SCOPE_INVALID: 名称为空/过长/含 NUL");
    Ok(format!("\"{}\"", name.replace('"', "\"\"")))
}
fn pragma(operation: &str, object: &str) -> Result<String> {
    Ok(format!("PRAGMA main.{operation}({})", ident(object)?))
}
pub(crate) fn validate_scopes(request: &StructureSyncRequest, binding: &StructureReviewBinding) -> Result<()> {
    ensure!(request.source_schema == "main" && request.target_schema == "main", "STRUCTURE_NAMESPACE_UNSUPPORTED: 仅认证 main，不将 attached/temp 当作 main");
    let _ = ident(&request.source_table)?; let _ = ident(&request.target_table)?;
    let valid_fp = |v: &str| v.len() == 64 && v.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    ensure!(!binding.principal.is_empty() && !binding.owner.is_empty() && valid_fp(&binding.source_fingerprint) && valid_fp(&binding.target_fingerprint), "STRUCTURE_BINDING_REQUIRED");
    ensure!(request.source_connection == binding.source_connection_id && request.target_connection == binding.target_connection_id
        && request.source_schema == binding.source_schema && request.source_table == binding.source_table
        && request.target_schema == binding.target_schema && request.target_table == binding.target_table, "STRUCTURE_BINDING_CHANGED");
    ensure!(request.columns.len() <= 1024, "STRUCTURE_MAPPING_QUOTA");
    for (from, to) in &request.columns { let _ = ident(from)?; let _ = ident(to)?; }
    Ok(())
}
pub(crate) fn sqlite_pool<'a>(db: &'a Db, kind: &DbKind) -> Result<&'a SqlitePool> {
    match (db, kind.protocol()) { (Db::Sqlite(pool), Protocol::Sqlite) => Ok(pool), _ => Err(anyhow!("STRUCTURE_NATIVE_METADATA_UNSUPPORTED: 受控 owned 结构同步目前只认证原生 SQLite；PG/MySQL 未验证")) }
}

#[cfg(windows)]
fn win_file_identity(handle: *mut std::ffi::c_void) -> Result<String> {
    use std::ffi::c_void;
    #[repr(C)] struct FileInfo { attributes: u32, creation: [u32; 2], access: [u32; 2], write: [u32; 2], volume: u32, size_high: u32, size_low: u32, links: u32, index_high: u32, index_low: u32 }
    #[link(name = "kernel32")]
    extern "system" { fn GetFileInformationByHandle(handle: *mut c_void, information: *mut FileInfo) -> i32; }
    let mut info = std::mem::MaybeUninit::<FileInfo>::uninit();
    // SAFETY: 调用者在期间持有 std File 或 SQLx sqlite handle 锁，HANDLE 有效；输出布局与 Win32 一致。
    let ok = unsafe { GetFileInformationByHandle(handle, info.as_mut_ptr()) };
    ensure!(ok != 0, "STRUCTURE_PHYSICAL_IDENTITY_UNPROVEN: {}", std::io::Error::last_os_error());
    // SAFETY: Win32 成功调用初始化了整个输出结构。
    let info = unsafe { info.assume_init() };
    Ok(format!("sqlite-file:{:08x}:{:08x}{:08x}:{:08x}{:08x}", info.volume, info.index_high, info.index_low, info.creation[1], info.creation[0]))
}
#[cfg(unix)]
fn file_identity(path: &Path) -> Result<String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path)?;
    Ok(format!("sqlite-file:{}:{}", meta.dev(), meta.ino()))
}
#[cfg(not(any(windows, unix)))]
fn file_identity(_path: &Path) -> Result<String> { Err(anyhow!("STRUCTURE_PHYSICAL_IDENTITY_UNPROVEN: 此平台没有文件身份实现")) }

#[cfg(windows)]
fn file_identity(path: &Path) -> Result<String> {
    use std::os::windows::io::AsRawHandle;
    let file = std::fs::File::open(path)?;
    win_file_identity(file.as_raw_handle())
}
#[cfg(windows)]
async fn owned_file_identity(conn: &mut SqliteConnection, _path: &Path) -> Result<String> {
    let mut locked = conn.lock_handle().await?;
    let mut handle: *mut std::ffi::c_void = std::ptr::null_mut();
    // SAFETY: 固定 native SQLite 连接，main 名称 NUL 终止，GetHandle 的输出是 HANDLE 指针。
    let rc = unsafe { libsqlite3_sys::sqlite3_file_control(locked.as_raw_handle().as_ptr(), b"main\0".as_ptr().cast(), libsqlite3_sys::SQLITE_FCNTL_WIN32_GET_HANDLE, (&mut handle as *mut *mut std::ffi::c_void).cast()) };
    ensure!(rc == libsqlite3_sys::SQLITE_OK && !handle.is_null(), "STRUCTURE_PHYSICAL_IDENTITY_UNPROVEN: 未取得 SQLite 实际 Win32 文件 HANDLE");
    win_file_identity(handle)
}
#[cfg(unix)]
async fn owned_file_identity(conn: &mut SqliteConnection, path: &Path) -> Result<String> {
    let identity = file_identity(path)?;
    let mut locked = conn.lock_handle().await?; let mut moved: i32 = 0;
    // SAFETY: 锁固定内建 unix VFS，HAS_MOVED 输出整型检查路径是否仍对应实际 open 文件 inode。
    let rc = unsafe { libsqlite3_sys::sqlite3_file_control(locked.as_raw_handle().as_ptr(), b"main\0".as_ptr().cast(), libsqlite3_sys::SQLITE_FCNTL_HAS_MOVED, (&mut moved as *mut i32).cast()) };
    ensure!(rc == libsqlite3_sys::SQLITE_OK && moved == 0, "STRUCTURE_PHYSICAL_IDENTITY_CHANGED: open inode 与路径不一致或 VFS 不可证明");
    Ok(identity)
}
#[cfg(not(any(windows, unix)))]
async fn owned_file_identity(_conn: &mut SqliteConnection, _path: &Path) -> Result<String> { Err(anyhow!("STRUCTURE_PHYSICAL_IDENTITY_UNPROVEN")) }

pub(crate) async fn ensure_autocommit(conn: &mut SqliteConnection) -> Result<()> {
    let mut handle = conn.lock_handle().await?;
    // SAFETY: SQLx handle 锁固定存活连接；只读取 SQLite autocommit 状态。
    let active = unsafe { libsqlite3_sys::sqlite3_get_autocommit(handle.as_raw_handle().as_ptr()) == 0 };
    ensure!(!active, "STRUCTURE_CONNECTION_TRANSACTION_ACTIVE: 不接管/回滚未知已有事务");
    Ok(())
}
pub(crate) async fn physical(conn: &mut SqliteConnection, pool: &SqlitePool, options: &Arc<SqliteConnectOptions>) -> Result<PhysicalDatabaseIdentity> {
    // SQLx 0.8.6 会把 :memory:/file:... 当作 URL host，转换时可能 panic。
    // 仅在真实 options 的副本上替换序列化文件名以读取 flags；不打开 probe，也不改变连接/身份。
    let url = options.as_ref().clone().filename("navkit-structure-options-probe.sqlite").to_url_lossy();
    ensure!(!url.query_pairs().any(|(k, v)| k == "vfs" || k == "immutable" && v == "true"), "STRUCTURE_PHYSICAL_IDENTITY_UNPROVEN: 自定义 VFS/immutable 连接没有认证 fresh CAS");
    let databases: Vec<(i64, String, String)> = crate::sql_history::query_as("PRAGMA database_list").fetch_all(&mut *conn).await?;
    ensure!(databases.iter().all(|(_, name, _)| name == "main" || name == "temp"), "STRUCTURE_ATTACHED_DATABASE_UNSUPPORTED");
    let temp_count: i64 = crate::sql_history::query_scalar("SELECT count(*) FROM temp.sqlite_schema").fetch_one(&mut *conn).await?;
    ensure!(temp_count == 0, "STRUCTURE_TEMP_SHADOW_UNSUPPORTED: TEMP 对象可能遮蔽编译器未限定 main 的 DDL");
    let file = databases.iter().find(|(_, name, _)| name == "main").ok_or_else(|| anyhow!("STRUCTURE_PHYSICAL_IDENTITY_UNPROVEN: main 不可见"))?.2.clone();
    if !file.is_empty() {
        let (path, identity) = tokio::task::spawn_blocking(move || {
            let path = std::fs::canonicalize(PathBuf::from(file))?;
            let identity = file_identity(&path)?;
            Ok::<_, anyhow::Error>((path, identity))
        }).await??;
        let owned_identity = owned_file_identity(conn, &path).await?;
        ensure!(owned_identity == identity, "STRUCTURE_PHYSICAL_IDENTITY_CHANGED: 同路径文件被替换，不能把新文件身份赋给旧 pinned 连接");
        return Ok(PhysicalDatabaseIdentity { kind: "sqlite-file".into(), identity: owned_identity, canonical_path: Some(path.to_string_lossy().into_owned()) });
    }
    // memory 身份只接受此实际 pool 持有的 options Arc，不能借调用方构造的同名副本证明同库。
    ensure!(Arc::ptr_eq(options, &pool.connect_options()), "STRUCTURE_MEMORY_IDENTITY_UNPROVEN: options 并非实际 pool 的选项");
    let filename = options.get_filename().to_str().ok_or_else(|| anyhow!("STRUCTURE_MEMORY_IDENTITY_UNPROVEN"))?;
    let generated = filename.strip_prefix("file:sqlx-in-memory-").is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()));
    let shared = url.query_pairs().any(|(k, v)| k == "cache" && v == "shared");
    let memory_mode = url.query_pairs().any(|(k, v)| k == "mode" && v == "memory");
    let identity = if generated && shared && memory_mode {
        // SQLx 原生解析器生成的进程内共享 URI：克隆 pool/同 URI 的 pool 指向相同实际库。
        format!("sqlite-memory:process:{}:generated:{}", std::process::id(), crate::object_catalog::revision(&filename))
    } else if filename == ":memory:" && pool.options().get_max_connections() == 1 {
        // 私有 :memory: 限单连接，保留 options Arc 到 owned operation 结束，避免 ABA/reuse。
        format!("sqlite-memory:process:{}:pool:{:x}", std::process::id(), Arc::as_ptr(options) as usize)
    } else { return Err(anyhow!("STRUCTURE_MEMORY_IDENTITY_UNPROVEN: named/shared 或多连接私有 memory 不能仅凭名称证明同库")); };
    Ok(PhysicalDatabaseIdentity { kind: "sqlite-memory".into(), identity, canonical_path: None })
}

#[derive(Debug)]
struct DdlToken { text: String, word: bool }
fn ddl_tokens(sql: &str) -> Result<Vec<DdlToken>> {
    ensure!(sql.len() <= 8 * 1024 * 1024, "STRUCTURE_METADATA_DDL_QUOTA");
    let b = sql.as_bytes(); let mut i = 0; let mut out = Vec::new();
    while i < b.len() {
        if b[i].is_ascii_whitespace() { i += 1; continue; }
        if b[i..].starts_with(b"--") { while i < b.len() && b[i] != b'\n' { i += 1; } continue; }
        if b[i..].starts_with(b"/*") { i += 2; while i + 1 < b.len() && !b[i..].starts_with(b"*/") { i += 1; } ensure!(i + 1 < b.len(), "STRUCTURE_DDL_PARSE_UNKNOWN"); i += 2; continue; }
        let start = i;
        if matches!(b[i], b'\'' | b'"' | b'`' | b'[') {
            let opening = b[i]; let closing = if opening == b'[' { b']' } else { opening }; i += 1;
            loop { ensure!(i < b.len(), "STRUCTURE_DDL_PARSE_UNKNOWN"); if b[i] == closing { i += 1; if opening != b'[' && i < b.len() && b[i] == closing { i += 1; continue; } break; } i += 1; }
            out.push(DdlToken { text: sql[start..i].into(), word: false });
        } else if matches!(b[i], b'(' | b')' | b',' | b';' | b'.') { i += 1; out.push(DdlToken { text: sql[start..i].into(), word: false }); }
        else {
            while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'\'' | b'"' | b'`' | b'[' | b'(' | b')' | b',' | b';' | b'.') && !b[i..].starts_with(b"--") && !b[i..].starts_with(b"/*") { i += 1; }
            ensure!(i > start, "STRUCTURE_DDL_PARSE_UNKNOWN"); out.push(DdlToken { text: sql[start..i].into(), word: true });
        }
        ensure!(out.len() <= 262144, "STRUCTURE_METADATA_DDL_QUOTA");
    }
    Ok(out)
}
fn ensure_representable_ddl(sql: &str) -> Result<()> {
    for t in ddl_tokens(sql)? {
        if t.word && ["VIRTUAL", "COLLATE", "GENERATED", "AUTOINCREMENT", "DEFERRABLE", "INITIALLY", "WITHOUT", "STRICT"].iter().any(|w| t.text.eq_ignore_ascii_case(w)) {
            return Err(anyhow!("STRUCTURE_METADATA_UNREPRESENTABLE: {} 没有完整建模，不能生成假安全 ALTER", t.text));
        }
    }
    Ok(())
}
fn action(value: String) -> Result<String> {
    ensure!(["NO ACTION", "RESTRICT", "CASCADE", "SET NULL", "SET DEFAULT"].contains(&value.as_str()), "STRUCTURE_METADATA_ACTION_UNKNOWN"); Ok(value)
}
async fn ordered_columns(conn: &mut SqliteConnection, table: &str) -> Result<(Vec<ColumnInfo>, Vec<String>)> {
    let rows = crate::sql_history::query(&pragma("table_xinfo", table)?).fetch_all(&mut *conn).await?;
    ensure!(!rows.is_empty() && rows.len() <= MAX_COLUMNS, "STRUCTURE_METADATA_COLUMNS_UNKNOWN_OR_QUOTA");
    let mut columns = Vec::new(); let mut pk = Vec::new(); let mut seen = BTreeSet::new();
    for (ordinal, row) in rows.into_iter().enumerate() {
        let cid: i64 = row.try_get("cid")?; let name: String = row.try_get("name")?; let data_type: String = row.try_get("type")?;
        ensure!(cid == ordinal as i64 && row.try_get::<i64, _>("hidden")? == 0, "STRUCTURE_METADATA_UNREPRESENTABLE: generated/hidden/列顺序未知");
        let _ = ident(&name)?; ensure!(seen.insert(name.clone()) && !data_type.is_empty() && data_type.len() <= 1024, "STRUCTURE_METADATA_COLUMN_UNKNOWN");
        let not_null: i64 = row.try_get("notnull")?; let order: i64 = row.try_get("pk")?;
        ensure!((not_null == 0 || not_null == 1) && order >= 0, "STRUCTURE_METADATA_COLUMN_UNKNOWN");
        if order > 0 { pk.push((order, name.clone())); }
        columns.push(ColumnInfo { name, data_type, nullable: not_null == 0, default: row.try_get("dflt_value")?, primary_key: order > 0,
            auto_increment: false, comment: None, length: None, precision: None, scale: None });
    }
    pk.sort_by_key(|p| p.0); ensure!(pk.iter().enumerate().all(|(i, p)| p.0 == i as i64 + 1), "STRUCTURE_METADATA_KEY_ORDER_UNKNOWN");
    Ok((columns, pk.into_iter().map(|p| p.1).collect()))
}
async fn indexes(conn: &mut SqliteConnection, table: &str, columns: &[ColumnInfo], pk: &[String]) -> Result<Vec<IndexInfo>> {
    let rows = crate::sql_history::query(&pragma("index_list", table)?).fetch_all(&mut *conn).await?;
    ensure!(rows.len() <= MAX_INDEXES, "STRUCTURE_METADATA_INDEX_QUOTA");
    let mut result = Vec::new(); let mut primary_seen = false; let mut total_key_parts = 0usize;
    for row in rows {
        let name: String = row.try_get("name")?; let origin: String = row.try_get("origin")?; let unique: i64 = row.try_get("unique")?;
        let _ = ident(&name)?; ensure!(["c", "u", "pk"].contains(&origin.as_str()) && (unique == 0 || unique == 1) && row.try_get::<i64, _>("partial")? == 0, "STRUCTURE_METADATA_UNREPRESENTABLE_INDEX: partial/origin {name}");
        let rows = crate::sql_history::query(&pragma("index_xinfo", &name)?).fetch_all(&mut *conn).await?; ensure!(rows.len() <= MAX_KEY_PARTS, "STRUCTURE_METADATA_INDEX_QUOTA");
        let mut keys = Vec::new();
        for key in rows {
            let member: i64 = key.try_get("key")?; ensure!(member == 0 || member == 1, "STRUCTURE_METADATA_INDEX_UNKNOWN"); if member == 0 { continue; }
            let col: Option<String> = key.try_get("name")?; let cid: i64 = key.try_get("cid")?; let coll: Option<String> = key.try_get("coll")?;
            ensure!(cid >= 0 && key.try_get::<i64, _>("desc")? == 0 && coll.as_deref() == Some("BINARY"), "STRUCTURE_METADATA_UNREPRESENTABLE_INDEX: expression/descending/collation {name}");
            let col = col.ok_or_else(|| anyhow!("STRUCTURE_METADATA_INDEX_UNKNOWN"))?;
            ensure!(columns.iter().any(|c| c.name == col), "STRUCTURE_METADATA_INDEX_UNKNOWN"); keys.push((key.try_get::<i64, _>("seqno")?, col));
        }
        keys.sort_by_key(|p| p.0); ensure!(!keys.is_empty() && keys.iter().enumerate().all(|(i, p)| p.0 == i as i64), "STRUCTURE_METADATA_KEY_ORDER_UNKNOWN");
        total_key_parts += keys.len(); ensure!(total_key_parts <= MAX_KEY_PARTS, "STRUCTURE_METADATA_INDEX_QUOTA");
        let keys: Vec<String> = keys.into_iter().map(|p| p.1).collect();
        if origin == "pk" { ensure!(!primary_seen && unique == 1 && keys == pk, "STRUCTURE_METADATA_PRIMARY_KEY_UNKNOWN"); primary_seen = true; }
        else { result.push(IndexInfo { name, columns: keys, unique: unique == 1, primary: false, method: None }); }
    }
    result.sort_by(|a, b| a.name.cmp(&b.name));
    if !pk.is_empty() { result.insert(0, IndexInfo { name: "PRIMARY KEY".into(), columns: pk.to_vec(), unique: true, primary: true, method: None }); }
    Ok(result)
}
async fn foreign_keys(conn: &mut SqliteConnection, table: &str, columns: &[ColumnInfo]) -> Result<Vec<ForeignKeyInfo>> {
    let rows = crate::sql_history::query(&pragma("foreign_key_list", table)?).fetch_all(&mut *conn).await?;
    ensure!(rows.len() <= MAX_KEY_PARTS, "STRUCTURE_METADATA_KEY_QUOTA");
    type Part = (i64, String, Option<String>);
    let mut groups: BTreeMap<i64, (ForeignKeyInfo, Vec<Part>)> = BTreeMap::new();
    for row in rows {
        let id: i64 = row.try_get("id")?; let target: String = row.try_get("table")?; let _ = ident(&target)?;
        let update = action(row.try_get("on_update")?)?; let delete = action(row.try_get("on_delete")?)?;
        let entry = groups.entry(id).or_insert_with(|| (ForeignKeyInfo { name: format!("FK_{table}_{id}"), columns: Vec::new(), ref_table: target.clone(), ref_schema: Some("main".into()),
            ref_columns: Vec::new(), on_update: Some(update.clone()), on_delete: Some(delete.clone()) }, Vec::new()));
        ensure!(id >= 0 && entry.0.ref_table == target && entry.0.on_update.as_ref() == Some(&update) && entry.0.on_delete.as_ref() == Some(&delete), "STRUCTURE_METADATA_FK_UNKNOWN");
        let from: String = row.try_get("from")?; ensure!(columns.iter().any(|c| c.name == from), "STRUCTURE_METADATA_FK_UNKNOWN");
        entry.1.push((row.try_get("seq")?, from, row.try_get("to")?));
    }
    let mut result = Vec::new();
    for (_, (mut fk, mut parts)) in groups {
        parts.sort_by_key(|p| p.0); ensure!(!parts.is_empty() && parts.iter().enumerate().all(|(i, p)| p.0 == i as i64), "STRUCTURE_METADATA_KEY_ORDER_UNKNOWN");
        let (parent_columns, parent_pk) = ordered_columns(conn, &fk.ref_table).await.map_err(|e| anyhow!("STRUCTURE_METADATA_REFERENCE_UNKNOWN: {e:#}"))?;
        fk.columns = parts.iter().map(|p| p.1.clone()).collect();
        if parts.iter().all(|p| p.2.is_none()) { ensure!(parts.len() == parent_pk.len(), "STRUCTURE_METADATA_REFERENCE_UNKNOWN"); fk.ref_columns = parent_pk; }
        else { fk.ref_columns = parts.into_iter().map(|p| p.2.ok_or_else(|| anyhow!("STRUCTURE_METADATA_REFERENCE_UNKNOWN"))).collect::<Result<_>>()?; }
        ensure!(fk.ref_columns.iter().all(|name| parent_columns.iter().any(|c| &c.name == name)), "STRUCTURE_METADATA_REFERENCE_UNKNOWN");
        result.push(fk);
    }
    Ok(result)
}

/// 调用方持快照/写事务。此函数不 BEGIN/COMMIT，也不借 pool，更不修改数据或 schema。
pub async fn snapshot(conn: &mut SqliteConnection, physical: &PhysicalDatabaseIdentity, schema: &str, table: &str) -> Result<OwnedTableSnapshot> {
    ensure!(schema == "main", "STRUCTURE_NAMESPACE_UNSUPPORTED"); let _ = ident(table)?;
    let before: i64 = crate::sql_history::query_scalar("PRAGMA main.schema_version").fetch_one(&mut *conn).await?;
    let row: Option<(String, i64, Option<String>)> = crate::sql_history::query_as("SELECT type, rootpage, sql FROM main.sqlite_schema WHERE name = ?").bind(table).fetch_optional(&mut *conn).await?;
    let (kind, rootpage, sql) = row.ok_or_else(|| anyhow!("STRUCTURE_TABLE_NOT_FOUND: {table}"))?;
    ensure!(kind == "table" && rootpage > 0, "STRUCTURE_TABLE_UNREPRESENTABLE: 不把 view/virtual 当实体表");
    let sql = sql.ok_or_else(|| anyhow!("STRUCTURE_TABLE_DEFINITION_UNKNOWN"))?; ensure_representable_ddl(&sql)?;
    let table_rows = crate::sql_history::query("SELECT name,type,wr,strict FROM pragma_table_list WHERE schema='main' AND name=? LIMIT 2").bind(table).fetch_all(&mut *conn).await?;
    ensure!(table_rows.len() == 1, "STRUCTURE_TABLE_LIST_UNKNOWN");
    let table_row = &table_rows[0];
    ensure!(table_row.try_get::<String, _>("name")? == table, "STRUCTURE_TABLE_LIST_UNKNOWN");
    ensure!(table_row.try_get::<String, _>("type")? == "table" && table_row.try_get::<i64, _>("wr")? == 0 && table_row.try_get::<i64, _>("strict")? == 0, "STRUCTURE_METADATA_UNREPRESENTABLE: strict/without-rowid/virtual 没有完整建模");
    let (objects_count, objects_bytes): (i64, i64) = crate::sql_history::query_as("SELECT count(*), COALESCE(sum(length(sql)),0) FROM main.sqlite_schema WHERE name=? OR tbl_name=?").bind(table).bind(table).fetch_one(&mut *conn).await?;
    ensure!(objects_count <= 4096 && objects_bytes <= 16 * 1024 * 1024, "STRUCTURE_METADATA_OBJECT_QUOTA: 在读取触发器/DDL 文本前拒绝超量目录");
    let (columns, pk) = ordered_columns(conn, table).await?;
    let indexes = indexes(conn, table, &columns, &pk).await?;
    let foreign_keys = foreign_keys(conn, table, &columns).await?;
    let objects = table_objects::list_sqlite(conn, schema, table).await?;
    ensure!(objects.table_definition.as_deref() == Some(sql.as_str()), "STRUCTURE_METADATA_CHANGED: CREATE TABLE 读回不同");
    let after: i64 = crate::sql_history::query_scalar("PRAGMA main.schema_version").fetch_one(&mut *conn).await?;
    ensure!(before == after, "STRUCTURE_METADATA_CHANGED: 目录读取跨 generation");
    let metadata = TableSchema { table: table.into(), database: schema.into(), columns, indexes, foreign_keys, comment: None, engine: None, collation: None, row_count: None };
    let identity = PhysicalTableIdentity { database: physical.clone(), schema: schema.into(), table: table.into(), rootpage };
    let base_revision = structure_sync::revision_for_schema(&metadata, &objects, schema, table)?;
    let revision = crate::object_catalog::revision(&(&base_revision, &identity, before));
    Ok(OwnedTableSnapshot { schema: metadata, objects, revision, identity, schema_version: before, base_revision })
}

pub(crate) struct OwnedPair {
    pub target: PoolConnection<Sqlite>,
    pub source: Option<PoolConnection<Sqlite>>,
    pub source_physical: PhysicalDatabaseIdentity,
    pub target_physical: PhysicalDatabaseIdentity,
    pub target_tx: bool,
    pub source_tx: bool,
    callbacks: bool,
    _source_options: Arc<SqliteConnectOptions>,
    _target_options: Arc<SqliteConnectOptions>,
}
async fn acquire(pool: &SqlitePool, entry: Option<&Arc<Entry>>) -> Result<PoolConnection<Sqlite>> {
    if let Some(entry) = entry {
        tokio::select! { biased; _ = entry.wait_stop() => Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "排队停止，没有发送结构 DDL").into()), result = pool.acquire() => Ok(result?) }
    } else { Ok(pool.acquire().await?) }
}
async fn release_unstarted(mut conn: PoolConnection<Sqlite>, callbacks: bool) -> Result<()> {
    let mut valid = ensure_autocommit(&mut conn).await;
    if callbacks { if let Err(error) = crate::execution::clear_sqlite(&mut conn).await { valid = Err(error); } }
    if let Err(error) = valid { conn.close_on_drop(); let _ = conn.close().await; return Err(error); }
    conn.return_to_pool().await; Ok(())
}
/// 在锁之前拿到两端实际身份。同 pool clone/同 file 别名合并为 target 一条连接，避免 max_connections=1 自锁。
pub(crate) async fn acquire_pair(source_pool: &SqlitePool, target_pool: &SqlitePool, entry: Option<&Arc<Entry>>) -> Result<OwnedPair> {
    let source_options = source_pool.connect_options(); let target_options = target_pool.connect_options();
    let mut target = acquire(target_pool, entry).await?;
    let outcome = async {
        ensure_autocommit(&mut target).await?;
        if let Some(entry) = entry { crate::execution::install_sqlite(&mut target, entry, false).await?; }
        let target_physical = physical(&mut target, target_pool, &target_options).await?;
        if Arc::ptr_eq(&source_options, &target_options) {
            return Ok::<_, anyhow::Error>((None, target_physical.clone(), target_physical));
        }
        let mut source = acquire(source_pool, entry).await?;
        let source_result = async {
            ensure_autocommit(&mut source).await?;
            if let Some(entry) = entry { crate::execution::install_sqlite(&mut source, entry, false).await?; }
            physical(&mut source, source_pool, &source_options).await
        }.await;
        let source_physical = match source_result {
            Ok(value) => value,
            Err(error) => { if let Err(cleanup) = release_unstarted(source, entry.is_some()).await { return Err(anyhow!("STRUCTURE_ACQUIRE_CLEANUP_UNKNOWN: {error:#}；{cleanup:#}")); } return Err(error); }
        };
        if source_physical.identity == target_physical.identity {
            if entry.is_some() { if let Err(error) = crate::execution::clear_sqlite(&mut source).await { source.close_on_drop(); let _ = source.close().await; return Err(error); } }
            source.return_to_pool().await;
            Ok((None, source_physical, target_physical))
        } else { Ok((Some(source), source_physical, target_physical)) }
    }.await;
    match outcome {
        Ok((source, source_physical, target_physical)) => Ok(OwnedPair { target, source, source_physical, target_physical, target_tx: false, source_tx: false, callbacks: entry.is_some(), _source_options: source_options, _target_options: target_options }),
        Err(error) => { if let Err(cleanup) = release_unstarted(target, entry.is_some()).await { return Err(anyhow!("STRUCTURE_ACQUIRE_CLEANUP_UNKNOWN: {error:#}；{cleanup:#}")); } Err(error) }
    }
}
impl OwnedPair {
    pub fn same_database(&self) -> bool { self.source.is_none() }
    pub fn validate_paths(&self) -> Result<()> {
        for (options, expected) in [(&self._source_options, &self.source_physical), (&self._target_options, &self.target_physical)] {
            if expected.canonical_path.is_some() {
                let path = std::fs::canonicalize(options.get_filename()).map_err(|error| anyhow!("STRUCTURE_PHYSICAL_IDENTITY_CHANGED: 声明路径不可解析：{error}"))?;
                ensure!(file_identity(&path)? == expected.identity, "STRUCTURE_PHYSICAL_IDENTITY_CHANGED: 源/目标 alias 被重定向或替换，不将旧 open 连接当作新文件");
            }
        }
        Ok(())
    }
    pub async fn begin(&mut self, write: bool) -> Result<()> {
        if write {
            let query_only: i64 = crate::sql_history::query_scalar("PRAGMA query_only").fetch_one(&mut *self.target).await?;
            let foreign_keys: i64 = crate::sql_history::query_scalar("PRAGMA foreign_keys").fetch_one(&mut *self.target).await?;
            let legacy: i64 = crate::sql_history::query_scalar("PRAGMA legacy_alter_table").fetch_one(&mut *self.target).await?;
            ensure!(query_only == 0 && foreign_keys == 1 && legacy == 0, "STRUCTURE_CONNECTION_POLICY_UNPROVEN: 不关闭 query_only/外键检查，也不静默修改 legacy_alter_table");
        }
        let begin = if write { "BEGIN IMMEDIATE" } else { "BEGIN DEFERRED" };
        let target_begin = crate::sql_history::query(begin).execute(&mut *self.target).await;
        // 即使 BEGIN ack 失败，也用实际 autocommit 状态发现可能已经建立的事务，cleanup 才能回滚。
        self.target_tx = ensure_autocommit(&mut self.target).await.is_err();
        target_begin?; ensure!(self.target_tx, "STRUCTURE_TRANSACTION_UNPROVEN");
        if let Some(source) = self.source.as_mut() {
            let source_begin = crate::sql_history::query(begin).execute(&mut **source).await;
            self.source_tx = ensure_autocommit(source).await.is_err();
            source_begin?; ensure!(self.source_tx, "STRUCTURE_SOURCE_TRANSACTION_UNPROVEN");
        }
        Ok(())
    }
    pub async fn snapshots(&mut self, request: &StructureSyncRequest) -> Result<(OwnedTableSnapshot, Option<OwnedTableSnapshot>)> {
        self.validate_paths()?;
        let source = if let Some(conn) = self.source.as_mut() { snapshot(conn, &self.source_physical, &request.source_schema, &request.source_table).await? }
            else { snapshot(&mut self.target, &self.target_physical, &request.source_schema, &request.source_table).await? };
        let target_exists: i64 = crate::sql_history::query_scalar("SELECT count(*) FROM main.sqlite_schema WHERE name = ?").bind(&request.target_table).fetch_one(&mut *self.target).await?;
        let target = if target_exists == 0 { None } else { Some(snapshot(&mut self.target, &self.target_physical, &request.target_schema, &request.target_table).await?) };
        if let Some(target) = &target {
            structure_sync::validate_distinct_physical_tables(&source.identity.database.identity, &source.identity.schema, &source.identity.table,
                &target.identity.database.identity, &target.identity.schema, &target.identity.table)?;
        }
        Ok((source, target))
    }
    /// 正常完成也必须等待 callbacks 清除、ROLLBACK 确认与 SQLx return_to_pool 真正完成。
    pub async fn cleanup(mut self, destroy: bool) -> Result<()> {
        let mut error: Option<anyhow::Error> = None;
        if self.callbacks {
            if let Err(e) = crate::execution::clear_sqlite(&mut self.target).await { error = Some(e); }
            if let Some(source) = self.source.as_mut() { if let Err(e) = crate::execution::clear_sqlite(source).await { error = Some(e); } }
        }
        if self.target_tx {
            if let Err(e) = crate::sql_history::query("ROLLBACK").execute(&mut *self.target).await { error = Some(e.into()); }
            else if let Err(e) = ensure_autocommit(&mut self.target).await { error = Some(e); }
        }
        if self.source_tx { if let Some(source) = self.source.as_mut() {
            if let Err(e) = crate::sql_history::query("ROLLBACK").execute(&mut **source).await { error = Some(e.into()); }
            else if let Err(e) = ensure_autocommit(source).await { error = Some(e); }
        } }
        let destroy = destroy || error.is_some();
        if destroy {
            self.target.close_on_drop(); if let Err(e) = self.target.close().await { error = Some(e.into()); }
            if let Some(mut source) = self.source { source.close_on_drop(); if let Err(e) = source.close().await { error = Some(e.into()); } }
        } else { self.target.return_to_pool().await; if let Some(mut source) = self.source { source.return_to_pool().await; } }
        match error { Some(e) => Err(e), None => Ok(()) }
    }
}

/// native 预览只读，不签 C1 grant。调用方持真实 activity + 双端 ordinary，且读前后复验 saved/runtime FP。
pub async fn preflight(source: Arc<Db>, source_kind: DbKind, target: Arc<Db>, target_kind: DbKind, ordinary: OrdinaryLease,
    request: StructureSyncRequest, binding: StructureReviewBinding, read_only: bool) -> CmdResult<StructurePreflight> {
    let _ordinary = ordinary;
    let operation = async {
        validate_scopes(&request, &binding)?;
        let source_pool = sqlite_pool(&source, &source_kind)?; let target_pool = sqlite_pool(&target, &target_kind)?;
        let mut pair = acquire_pair(source_pool, target_pool, None).await?;
        let result = async {
            pair.begin(false).await?;
            let (source_before, target_before) = pair.snapshots(&request).await?;
            let (source_after, target_after) = pair.snapshots(&request).await?;
            ensure!(source_before.revision == source_after.revision && target_before.as_ref().map(|s| &s.revision) == target_after.as_ref().map(|s| &s.revision), "STRUCTURE_PREVIEW_STALE: owned 读取跨目录 generation");
            let mut report = structure_sync::preview_from_schemas(&source_before.schema, target_before.as_ref().map(|s| &s.schema), &target_kind, &request,
                &source_before.revision, target_before.as_ref().map(|s| s.revision.as_str()).unwrap_or(""))?;
            if read_only { report.alterable = false; report.unsupported.push("目标保存连接是只读；预览保留，但没有写入资格".into()); }
            if request.allow_drop { report.alterable = false; report.unsupported.push("原生 SQLite 受控流程当前仅认证非破坏增列/新增普通索引；不 DROP/CASCADE/整表重建".into()); }
            if let Some(target_snapshot) = target_before.as_ref() {
                if let (Some(final_design), true) = (report.final_design.as_ref(), report.alterable) {
                    if let Err(error) = super::safe_steps(target_snapshot, final_design, &report.steps) {
                        report.alterable = false; report.unsupported.push(format!("原生安全 DDL 子集拒绝：{error:#}"));
                    }
                }
            } else {
                report.alterable = false; report.unsupported.push("目标表不存在：当前 owned 子集未认证新建表的 absence 基线，不把缺表当可增列".into());
            }
            let mut request_sql = String::new();
            if report.alterable && !report.steps.is_empty() {
                structure_sync::bind_review(&request, &mut report, &binding)?;
                let mut reviewed = request.clone(); reviewed.source_revision = report.source_revision.clone(); reviewed.target_revision = report.target_revision.clone(); reviewed.review_digest = report.review_digest.clone();
                request_sql = structure_sync::review_request_sql(&reviewed)?;
            }
            let same_database = pair.same_database();
            // 独立 read-only 证据 domain，与执行 review 不兼容；它只证明当前两个 owned 读快照，绝非先前 COMMIT ACK。
            let observation_digest = crate::object_catalog::revision(&("STRUCTURE_READ_ONLY_OBSERVATION", &binding, &report, &source_before.identity, target_before.as_ref().map(|s| &s.identity), same_database));
            let observation_only = request_sql.is_empty();
            Ok::<_, anyhow::Error>(StructurePreflight { report, source_identity: source_before.identity, target_identity: target_before.map(|s| s.identity), same_database,
                source_consistency: if same_database { "same_owned_read_snapshot" } else { "two_owned_read_snapshots_not_cross_database_atomic" }.into(),
                request_sql, observation_digest, observation_binding: binding.clone(), observation_only })
        }.await;
        let cleanup = pair.cleanup(false).await;
        match cleanup { Ok(()) => result, Err(error) => Err(anyhow!("STRUCTURE_PREVIEW_CLEANUP_UNKNOWN: {error:#}")) }
    }.await;
    operation.map_err(AppError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manual_transaction;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    fn inputs() -> (StructureSyncRequest, StructureReviewBinding) {
        let request = StructureSyncRequest { source_connection: "source".into(), source_schema: "main".into(), source_table: "src".into(), target_connection: "target".into(), target_schema: "main".into(), target_table: "dst".into(), columns: vec![], allow_drop: false, source_revision: String::new(), target_revision: String::new(), review_digest: String::new() };
        let binding = StructureReviewBinding { principal: "window".into(), owner: "page".into(), source_connection_id: "source".into(), source_fingerprint: "a".repeat(64), source_schema: "main".into(), source_table: "src".into(), target_connection_id: "target".into(), target_fingerprint: "b".repeat(64), target_schema: "main".into(), target_table: "dst".into() };
        (request, binding)
    }
    async fn memory() -> SqlitePool {
        SqlitePoolOptions::new().max_connections(1).connect_with(SqliteConnectOptions::new().in_memory(true).foreign_keys(true)).await.unwrap()
    }
    async fn exec(pool: &SqlitePool, sql: &str) { crate::sql_history::query(sql).execute(pool).await.unwrap(); }
    async fn equal_tables(pool: &SqlitePool) {
        exec(pool, "CREATE TABLE src(a TEXT NOT NULL,b TEXT NOT NULL,c TEXT DEFAULT 'Keep  Case',PRIMARY KEY(b,a))").await;
        exec(pool, "CREATE TABLE dst(a TEXT NOT NULL,b TEXT NOT NULL,c TEXT DEFAULT 'Keep  Case',PRIMARY KEY(b,a))").await;
    }
    async fn preview(pool: &SqlitePool, request: StructureSyncRequest, binding: StructureReviewBinding, read_only: bool) -> StructurePreflight {
        let gates = manual_transaction::Registry::default(); let db = Arc::new(Db::Sqlite(pool.clone())); let lease = gates.ordinary(&["source", "target"]).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), preflight(db.clone(), DbKind::of("sqlite"), db, DbKind::of("sqlite"), lease, request, binding, read_only)).await.unwrap().unwrap()
    }
    #[test]
    fn ddl_lexer_keeps_quoted_keywords_literals_and_malformed_metadata_distinct() {
        assert!(ensure_representable_ddl("CREATE TABLE t(\"GENERATED\" TEXT DEFAULT 'COLLATE AUTOINCREMENT STRICT')").is_ok());
        for sql in ["CREATE TABLE t(c TEXT COLLATE NOCASE)", "CREATE TABLE t(a INT,c INT GENERATED ALWAYS AS(a+1))", "CREATE TABLE t(c INT) WITHOUT ROWID", "CREATE TABLE t(c INT) STRICT", "CREATE TABLE t(c TEXT DEFAULT 'unterminated)"] { assert!(ensure_representable_ddl(sql).is_err(), "{sql}"); }
    }
    #[test]
    fn identifiers_are_quoted_without_case_fold_and_scopes_do_not_alias_namespaces() {
        assert_eq!(ident("space \"Column").unwrap(), "\"space \"\"Column\""); assert!(ident("").is_err()); assert!(ident("bad\0name").is_err());
        let (mut request, binding) = inputs(); request.source_schema = "attached".into(); assert!(validate_scopes(&request, &binding).is_err());
    }
    #[tokio::test]
    async fn owned_snapshot_reads_ordered_pk_unique_and_fk_with_real_reference_schema() {
        let pool = memory().await;
        exec(&pool,"CREATE TABLE parent(a TEXT NOT NULL,b TEXT NOT NULL,PRIMARY KEY(b,a))").await;
        exec(&pool,"CREATE TABLE src(a TEXT NOT NULL,b TEXT NOT NULL,c TEXT,PRIMARY KEY(b,a),CONSTRAINT fk_pair FOREIGN KEY(b,a) REFERENCES parent(b,a) ON UPDATE CASCADE ON DELETE RESTRICT)").await;
        exec(&pool,"CREATE UNIQUE INDEX ux_pair ON src(c,a)").await;
        let mut conn = pool.acquire().await.unwrap(); let options = pool.connect_options(); let actual = physical(&mut conn,&pool,&options).await.unwrap();
        let value = snapshot(&mut conn,&actual,"main","src").await.unwrap();
        assert_eq!(crate::design::ordered_primary_key(&value.schema).unwrap(),vec!["b","a"]);
        let index = value.schema.indexes.iter().find(|i|i.name=="ux_pair").unwrap(); assert!(index.unique); assert_eq!(index.columns,vec!["c","a"]);
        assert_eq!(value.schema.foreign_keys.len(),1); let fk=&value.schema.foreign_keys[0]; assert_eq!(fk.columns,vec!["b","a"]); assert_eq!(fk.ref_columns,vec!["b","a"]); assert_eq!(fk.ref_schema.as_deref(),Some("main")); assert_eq!(fk.on_update.as_deref(),Some("CASCADE")); assert_eq!(fk.on_delete.as_deref(),Some("RESTRICT"));
        assert_eq!(value.objects.completeness,"complete"); assert_eq!(value.revision.len(),64); assert_ne!(value.revision,value.base_revision); assert!(value.identity.rootpage>0); conn.return_to_pool().await;
    }
    #[tokio::test]
    async fn revision_excludes_dml_but_detects_real_trigger_body_and_schema_generation() {
        let pool=memory().await; exec(&pool,"CREATE TABLE src(a INT)").await; exec(&pool,"CREATE TABLE audit(value TEXT)").await;
        exec(&pool,"CREATE TRIGGER src_audit AFTER INSERT ON src BEGIN INSERT INTO audit(value) VALUES('Keep  Case'); END").await;
        let mut conn=pool.acquire().await.unwrap(); let options=pool.connect_options(); let actual=physical(&mut conn,&pool,&options).await.unwrap();
        let first=snapshot(&mut conn,&actual,"main","src").await.unwrap(); crate::sql_history::query("INSERT INTO src(a) VALUES(1)").execute(&mut *conn).await.unwrap(); let after_dml=snapshot(&mut conn,&actual,"main","src").await.unwrap(); assert_eq!(first.revision,after_dml.revision);
        crate::sql_history::query("DROP TRIGGER src_audit").execute(&mut *conn).await.unwrap(); crate::sql_history::query("CREATE TRIGGER src_audit AFTER INSERT ON src BEGIN INSERT INTO audit(value) VALUES('keep case'); END").execute(&mut *conn).await.unwrap();
        let changed=snapshot(&mut conn,&actual,"main","src").await.unwrap(); assert_ne!(first.revision,changed.revision); assert_ne!(first.base_revision,changed.base_revision); assert_ne!(first.schema_version,changed.schema_version); conn.return_to_pool().await;
    }
    #[tokio::test]
    async fn same_pool_nochange_and_readonly_return_bound_observation_not_execution_review() {
        let pool=memory().await; equal_tables(&pool).await; let (request,binding)=inputs();
        let first=preview(&pool,request.clone(),binding.clone(),false).await; assert!(first.same_database); assert!(first.report.steps.is_empty()); assert!(first.request_sql.is_empty()); assert!(first.report.review_digest.is_empty()); assert!(first.observation_only); assert_eq!(first.observation_digest.len(),64); assert_eq!(first.observation_binding,binding);
        let readonly=preview(&pool,request.clone(),binding.clone(),true).await; assert!(!readonly.report.alterable); assert!(readonly.request_sql.is_empty()); assert!(readonly.observation_only); assert_eq!(readonly.observation_digest.len(),64);
        let mut observational=request; observational.source_revision=first.report.source_revision.clone(); observational.target_revision=first.report.target_revision.clone(); observational.review_digest=first.observation_digest.clone(); assert!(structure_sync::validate_review(&observational,&first.report,&binding).is_err());
        assert!(pool.num_idle()>0);
    }
    #[tokio::test]
    async fn expression_index_unknown_does_not_become_empty_complete_and_memory_survives() {
        let pool=memory().await; equal_tables(&pool).await; exec(&pool,"CREATE INDEX idx_expression ON src(lower(c))").await;
        let gates=manual_transaction::Registry::default(); let (request,binding)=inputs(); let db=Arc::new(Db::Sqlite(pool.clone())); let lease=gates.ordinary(&["source","target"]).await.unwrap();
        let error=preflight(db.clone(),DbKind::of("sqlite"),db,DbKind::of("sqlite"),lease,request,binding,false).await.unwrap_err(); assert!(error.message.contains("INDEX") || error.message.contains("索引"),"{}",error.message);
        let mut connection = pool.acquire().await.unwrap();
        let count:i64=crate::sql_history::query_scalar("SELECT count(*) FROM main.sqlite_schema WHERE name='src'").fetch_one(&mut *connection).await.unwrap(); assert_eq!(count,1);
        // 预审失败后仍复用同一条实际连接，不销毁唯一 private-memory 库；明确等待 observer 归还。
        let recovered: i64 = crate::sql_history::query_scalar("SELECT count(*) FROM main.sqlite_schema WHERE name='src'").fetch_one(&mut *connection).await.unwrap();
        assert_eq!(recovered,1, "预审失败后内存库仍必须保留源表");
        connection.return_to_pool().await; assert!(pool.num_idle() > 0);
    }
    #[tokio::test]
    async fn same_physical_table_alias_is_rejected_but_connection_is_not_destroyed() {
        let pool=memory().await; equal_tables(&pool).await; let gates=manual_transaction::Registry::default(); let (mut request,mut binding)=inputs(); request.target_table="src".into(); binding.target_table="src".into(); let db=Arc::new(Db::Sqlite(pool.clone())); let lease=gates.ordinary(&["source","target"]).await.unwrap();
        let error=preflight(db.clone(),DbKind::of("sqlite"),db,DbKind::of("sqlite"),lease,request,binding,false).await.unwrap_err(); assert!(error.message.contains("SAME_PHYSICAL_TABLE"),"{}",error.message); let count:i64=crate::sql_history::query_scalar("SELECT count(*) FROM main.sqlite_schema WHERE name='src'").fetch_one(&pool).await.unwrap(); assert_eq!(count,1);
    }
    #[tokio::test]
    async fn hardlink_alias_uses_actual_file_identity_and_target_connection_for_source() {
        let dir=std::env::temp_dir().join(format!("navkit_structure_alias_{}",uuid::Uuid::new_v4())); assert!(dir.is_absolute()); std::fs::create_dir_all(&dir).unwrap(); let path=dir.join("original.sqlite"); let alias=dir.join("alias.sqlite");
        let source=SqlitePoolOptions::new().max_connections(1).connect_with(SqliteConnectOptions::new().filename(&path).create_if_missing(true).foreign_keys(true)).await.unwrap(); equal_tables(&source).await; std::fs::hard_link(&path,&alias).unwrap();
        let target=SqlitePoolOptions::new().max_connections(1).connect_with(SqliteConnectOptions::new().filename(&alias).foreign_keys(true)).await.unwrap(); let gates=manual_transaction::Registry::default(); let (request,binding)=inputs(); let lease=gates.ordinary(&["source","target"]).await.unwrap();
        let result=tokio::time::timeout(std::time::Duration::from_secs(5),preflight(Arc::new(Db::Sqlite(source.clone())),DbKind::of("sqlite"),Arc::new(Db::Sqlite(target.clone())),DbKind::of("sqlite"),lease,request,binding,false)).await.unwrap().unwrap();
        assert!(result.same_database); assert_eq!(result.source_identity.database.identity,result.target_identity.as_ref().unwrap().database.identity); assert_ne!(result.source_identity.rootpage,result.target_identity.as_ref().unwrap().rootpage); assert!(result.request_sql.is_empty()); assert!(result.observation_only);
        source.close().await; target.close().await; std::fs::remove_dir_all(&dir).unwrap();
    }
}
