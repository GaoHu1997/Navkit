//! Bounded diff preview. No cross-database ordering/collation assumptions: JSON keys
//! retain the old row_key contract and are indexed as BLOBs in a private disk database.
//! Each side has its own read snapshot (NOT a distributed/atomic two-database snapshot).
//! SQLite: one-row driver buffer + native connection length limit + disk budget.
//! PG/MySQL: preserve small-table compatibility, but preflight and cap legacy buffers;
//! large-table/network-packet bounds and PG MONEY streaming are not yet verified.
use super::*;
use anyhow::Context;
use futures::TryStreamExt;
use sqlx::{Column, ConnectOptions, Connection, Executor, TypeInfo, ValueRef};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use std::path::PathBuf;

#[path = "sync_diff_store.rs"] mod sync_diff_store;
use sync_diff_store::Store;

const ROW_BYTES: usize = 8 * 1024 * 1024;
const RECORD_BYTES: usize = 16 * 1024 * 1024;
const SPILL_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_COLUMNS: usize = 1024;
const LEGACY_ROWS: u64 = 2048;
const LEGACY_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ROWS: u64 = 10_000_000;

fn where_sql(clause: Option<&str>) -> String {
    clause.filter(|s| !s.trim().is_empty()).map(|s| format!(" WHERE ({s})")).unwrap_or_default()
}
fn sqlite_table(table: &str) -> String { format!("main.{}", quote_ident(&DbKind::of("sqlite"), table)) }
fn checked_count(n: i64) -> Result<u64> { u64::try_from(n).context("差异预览收到无效负行数") }

// Only this disposable connection gets native limits/authorizer; no user index,
// persistent database PRAGMA or pool configuration is changed.
unsafe extern "C" fn read_authorizer(_: *mut std::ffi::c_void, action: i32,
    a: *const std::ffi::c_char, b: *const std::ffi::c_char,
    database: *const std::ffi::c_char, _: *const std::ffi::c_char) -> i32 {
    let bytes = |p: *const std::ffi::c_char| if p.is_null() { &[][..] } else { unsafe { std::ffi::CStr::from_ptr(p) }.to_bytes() };
    match action {
        libsqlite3_sys::SQLITE_SELECT | libsqlite3_sys::SQLITE_RECURSIVE | libsqlite3_sys::SQLITE_TRANSACTION => libsqlite3_sys::SQLITE_OK,
        libsqlite3_sys::SQLITE_READ if bytes(database) == b"main" => libsqlite3_sys::SQLITE_OK,
        libsqlite3_sys::SQLITE_PRAGMA if [b"table_info".as_slice(), b"table_list"].contains(&bytes(a)) => libsqlite3_sys::SQLITE_OK,
        libsqlite3_sys::SQLITE_FUNCTION => {
            let name = bytes(if b.is_null() { a } else { b });
            if [b"load_extension".as_slice(), b"writefile", b"readfile", b"eval", b"shell", b"fts3_tokenizer"].iter().any(|f| name.eq_ignore_ascii_case(f)) { libsqlite3_sys::SQLITE_DENY } else { libsqlite3_sys::SQLITE_OK }
        }
        _ => libsqlite3_sys::SQLITE_DENY,
    }
}

async fn sqlite_reader(pool: &sqlx::SqlitePool) -> Result<sqlx::SqliteConnection> {
    sqlite_reader_buffered(pool,1).await
}
async fn sqlite_reader_buffered(pool: &sqlx::SqlitePool, buffer_rows: usize) -> Result<sqlx::SqliteConnection> {
    // Keep the pool anchor alive until the dedicated connection has opened, including
    // generated SQLx named shared-memory databases. Private :memory: cannot be reopened.
    let mut anchor = pool.acquire().await?;
    let filename: String = crate::sql_history::query_scalar("SELECT file FROM pragma_database_list WHERE name='main'").fetch_one(&mut *anchor).await?;
    let options = pool.connect_options();
    if filename.is_empty() { bail!("内存 SQLite 必须使用原池的固定会话，不重开可能不同的数据库"); }
    let options = options.as_ref().clone().create_if_missing(false).read_only(!filename.is_empty())
        .row_buffer_size(buffer_rows).statement_cache_capacity(8).optimize_on_close(false,None);
    let mut reader = sqlx::SqliteConnection::connect_with(&options).await?;
    drop(anchor);
    let setup: Result<()> = async {
        let mut handle = reader.lock_handle().await?;
        unsafe {
            libsqlite3_sys::sqlite3_limit(handle.as_raw_handle().as_ptr(), libsqlite3_sys::SQLITE_LIMIT_LENGTH, ROW_BYTES as i32);
            let rc = libsqlite3_sys::sqlite3_set_authorizer(handle.as_raw_handle().as_ptr(), Some(read_authorizer), std::ptr::null_mut());
            if rc != libsqlite3_sys::SQLITE_OK { bail!("无法建立差异预览只读角色"); }
        }
        Ok(())
    }.await;
    if let Err(error) = setup { let close=reader.close().await; return Err(error.context(format!("只读角色初始化reader_close={}",close.is_ok()))); }
    Ok(reader)
}

async fn sqlite_metadata(connection: &mut sqlx::SqliteConnection, table: &str) -> Result<Option<crate::models::TableSchema>> {
    // First catalog SELECT establishes this side's snapshot before metadata/count/data.
    let actual: Option<String> = crate::sql_history::query_scalar("SELECT name FROM main.sqlite_schema WHERE name=? COLLATE BINARY AND type IN ('table','view')")
        .bind(table).fetch_optional(&mut *connection).await?;
    let Some(actual) = actual else { return Ok(None); };
    let kind: String = crate::sql_history::query(&format!("PRAGMA main.table_list({})", quote_ident(&DbKind::of("sqlite"), &actual)))
        .fetch_one(&mut *connection).await?.try_get("type")?;
    if kind != "table" && kind != "view" { bail!("差异预览只验证 SQLite 普通表/视图，virtual/shadow 外部存储快照尚未验证"); }
    // Deliberately matches schema::table_schema's table_info contract (not xinfo).
    let sql = format!("PRAGMA main.table_info({})", quote_ident(&DbKind::of("sqlite"), &actual));
    let mut stream = crate::sql_history::query(&sql).fetch(&mut *connection);
    let mut columns = Vec::new(); let mut bytes = 0usize;
    while let Some(row) = stream.try_next().await? {
        let column = ColumnInfo { name: row.try_get("name")?, data_type: row.try_get("type")?, nullable: row.try_get::<i64, _>("notnull")? == 0,
            default: row.try_get("dflt_value")?, primary_key: row.try_get::<i64, _>("pk")? > 0,
            auto_increment: false, comment: None, length: None, precision: None, scale: None };
        bytes = bytes.checked_add(column.name.len() + column.data_type.len() + column.default.as_ref().map_or(0, String::len)).ok_or_else(|| anyhow!("元数据长度溢出"))?;
        if columns.len() >= MAX_COLUMNS || bytes > ROW_BYTES { bail!("差异预览元数据超过预算"); }
        columns.push(column);
    }
    Ok(Some(crate::models::TableSchema { table: actual, database: "main".into(), columns,
        indexes: Vec::new(), foreign_keys: Vec::new(), comment: None, engine: None, collation: None, row_count: None }))
}

fn verify_metadata(expected: &crate::models::TableSchema, actual: &crate::models::TableSchema) -> Result<()> {
    if expected.columns.len() != actual.columns.len() || expected.columns.iter().zip(&actual.columns)
        .any(|(a,b)| a.name != b.name || a.data_type != b.data_type || a.primary_key != b.primary_key) {
        bail!("差异预览结构已变化，请重新预览；没有用新结构悄悄替换旧映射");
    }
    Ok(())
}

fn validate_record(names: &[String], row: &[Option<String>], meta: &crate::models::TableSchema) -> Result<()> {
    if names.len() > MAX_COLUMNS || row.len() != names.len() { bail!("差异预览列数超过预算或列值缺失"); }
    let bytes = row.iter().try_fold(0usize, |bytes, value| bytes.checked_add(value.as_ref().map_or(0, String::len)).ok_or_else(||anyhow!("行长度溢出")))?;
    if bytes > ROW_BYTES { bail!("差异预览单行超过8MiB预算"); }
    // Preserve normalized_cell exactly within a declared range. Reject, never declare
    // equal/fall back, when decimal display expansion could exceed bounded memory.
    for (name, value) in names.iter().zip(row) {
        if meta.columns.iter().any(|c| c.name == *name && numeric_family(&c.data_type)) {
            if let Some(value) = value {
                if value.len() > 16384 { bail!("差异预览数值文本超过16384字节归一化预算"); }
                if let Some((_, exponent)) = value.trim().rsplit_once(['e','E']) {
                    if exponent.parse::<i64>().is_ok_and(|n| n.unsigned_abs() > 16384) { bail!("差异预览数值指数超过精确归一化预算"); }
                }
            }
        }
    }
    Ok(())
}

fn sqlite_values(row: &sqlx::sqlite::SqliteRow) -> Result<Vec<Option<String>>> {
    let mut values = crate::values::sqlite_row_values(row, row.len());
    for (index, value) in values.iter_mut().enumerate() {
        let raw = row.try_get_raw(index)?;
        if raw.is_null() { continue; }
        if raw.type_info().name() == "REAL" && !row.try_get::<f64,_>(index)?.is_finite() { bail!("差异预览不接受非有限SQLite REAL，未伪造equal"); }
        if raw.type_info().name() == "BLOB" {
            let bytes: Vec<u8> = row.try_get(index)?;
            *value = Some(format!("0x{}", bytes.iter().map(|b| format!("{b:02X}")).collect::<String>()));
        }
        if value.is_none() { bail!("差异预览不能无损解码第{}列，未伪造 NULL/equal", index + 1); }
    }
    Ok(values)
}

// Adaptive decoder prechecks borrowed native lengths before allocating cells.
// The driver/raw row itself is outside the accounted-heap contract, guarded by
// the disposable native 8MiB length limit and one-row SQLx buffer.
fn sqlite_values_budgeted(row: &sqlx::sqlite::SqliteRow) -> Result<Vec<Option<String>>> {
    use std::fmt::Write;
    let mut values=Vec::new();
    values.try_reserve_exact(row.len()).map_err(|e|anyhow!("MEMORY_ALLOCATION_FAILED: 行容器申请失败: {e}"))?;
    let mut decoded=0usize;
    for index in 0..row.len() {
        let raw=row.try_get_raw(index)?;
        if raw.is_null() {values.push(None);continue;}
        let info=raw.type_info();
        let (length,blob,text)=match info.name() {
            "TEXT"=> {let text: &str=row.try_get(index)?;(text.len(),None,Some(text))},
            "BLOB"=> {let blob: &[u8]=row.try_get(index)?;let n=blob.len().checked_mul(2).and_then(|n|n.checked_add(2)).ok_or_else(||anyhow!("BLOB转换长度溢出"))?;(n,Some(blob),None)},
            "INTEGER"=>(32,None,None),
            // Rust's finite f64 Display expansion (including subnormal exponent)
            // is bounded here before formatting, not grown on demand.
            "REAL"=>(768,None,None),
            _=> {bail!("差异预览不能在预算内认证SQLite第{}列存储类{}，未伪造NULL/equal",index+1,info.name());}
        };
        decoded=decoded.checked_add(length).ok_or_else(||anyhow!("行解码容量溢出"))?;
        if decoded>ROW_BYTES {bail!("差异预览解码容量超过8MiB单行预算，分配前拒绝");}
        let mut value=String::new();value.try_reserve_exact(length).map_err(|e|anyhow!("MEMORY_ALLOCATION_FAILED: 单元格申请失败: {e}"))?;
        if let Some(text)=text {value.push_str(text);}
        else if let Some(blob)=blob {
            const HEX:&[u8;16]=b"0123456789ABCDEF";value.push_str("0x");
            for byte in blob {value.push(HEX[(byte>>4) as usize] as char);value.push(HEX[(byte&15) as usize] as char);}
        } else if info.name()=="INTEGER" {write!(&mut value,"{}",row.try_get::<i64,_>(index)?)?;}
        else {
            let number=row.try_get::<f64,_>(index)?;
            if !number.is_finite() {bail!("差异预览不接受非有限SQLite REAL，未伪造equal");}
            if number.fract()==0.0 && number.abs()<1e15 {write!(&mut value,"{}",number as i64)?;}
            else {write!(&mut value,"{number}")?;}
        }
        values.push(Some(value));
    }
    Ok(values)
}

struct Scratch { directory: PathBuf, path: PathBuf }
impl Scratch {
    fn new() -> Result<Self> {
        let directory = std::env::temp_dir().join(format!("navkit-sync-diff-{}", uuid::Uuid::new_v4()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)] { use std::os::unix::fs::DirBuilderExt; builder.mode(0o700); }
        builder.create(&directory)?;
        let path = directory.join("rows.sqlite");
        let mut options = std::fs::OpenOptions::new(); options.read(true).write(true).create_new(true);
        #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        if let Err(error) = options.open(&path) { let _ = std::fs::remove_dir(&directory); return Err(error.into()); }
        Ok(Self { directory, path })
    }
    fn clean(&self) -> Result<()> {
        if self.path.exists() { std::fs::remove_file(&self.path)?; }
        if self.directory.exists() { std::fs::remove_dir(&self.directory)?; }
        Ok(())
    }
}
impl Drop for Scratch { fn drop(&mut self) { let _ = self.clean(); } }
struct PendingRow { source: bool, sequence: i64, key: String, body: Vec<u8> }
struct Index {
    connection: sqlx::SqliteConnection, path: PathBuf, bytes: u64,
    source_rows: u64, target_rows: u64, pending: u64,
    rows: Vec<PendingRow>, row_bytes: usize,
}
impl Index {
    async fn open(scratch: &Scratch) -> Result<Self> {
        // Only our private scratch file is configured; indexes are never created in users' DBs.
        let options = SqliteConnectOptions::new().filename(&scratch.path).create_if_missing(false)
            .journal_mode(SqliteJournalMode::Off).row_buffer_size(1).statement_cache_capacity(8)
            .pragma("cache_size", "-2048").pragma("temp_store", "FILE").pragma("page_size", "4096")
            .pragma("max_page_count", "1048576");
        let mut connection = sqlx::SqliteConnection::connect_with(&options).await?;
        let setup=crate::sql_history::query("CREATE TABLE s(seq INTEGER PRIMARY KEY,k BLOB NOT NULL UNIQUE,body BLOB NOT NULL); CREATE TABLE t(seq INTEGER PRIMARY KEY,k BLOB NOT NULL UNIQUE,body BLOB NOT NULL); BEGIN")
            .execute(&mut connection).await;
        if let Err(error)=setup {
            let close=connection.close().await;let cleanup=scratch.clean();
            return Err(anyhow!(error).context(format!("spill初始化失败；close={} cleanup={}",close.is_ok(),cleanup.is_ok())));
        }
        Ok(Self { connection, path: scratch.path.clone(), bytes: 0, source_rows: 0, target_rows: 0, pending: 0,
            rows: Vec::with_capacity(128), row_bytes: 0 })
    }
    fn check_disk(&self) -> Result<()> {
        let mut bytes=0u64;
        for suffix in ["", "-journal", "-wal", "-shm"] {
            let path=PathBuf::from(format!("{}{suffix}",self.path.to_string_lossy()));
            match std::fs::metadata(path) {
                Ok(meta)=>{bytes=bytes.checked_add(meta.len()).ok_or_else(||anyhow!("spill实际文件长度溢出"))?;}
                Err(error) if error.kind()==std::io::ErrorKind::NotFound=>{},
                Err(error)=>return Err(error.into()),
            }
        }
        if bytes>SPILL_BYTES{bail!("差异预览spill实际文件/sidecar超过4GiB预算");}Ok(())
    }
    async fn push(&mut self, source: bool, names: &[String], row: &[Option<String>], keys: &[String], meta: &crate::models::TableSchema) -> Result<()> {
        validate_record(names, row, meta)?;
        let label = if source { "源表" } else { "目标表" };
        for name in keys {
            let index = names.iter().position(|n| n == name).ok_or_else(||anyhow!("{label}主键列缺失: {name}"))?;
            if row[index].is_none() { bail!("{label}主键 {name} 为 NULL，无法安全进行增量比较"); }
        }
        let key = row_key(names, row, keys).ok_or_else(||anyhow!("{label}无法读取完整主键值"))?;
        let body = serde_json::to_vec(row)?;
        if key.len() > RECORD_BYTES || body.len() > RECORD_BYTES { bail!("差异预览编码记录超过16MiB预算"); }
        self.push_encoded(source,key,body).await
    }
    async fn push_budgeted(&mut self, source: bool, names: &[String], row: &[Option<String>], keys: &[String], meta: &crate::models::TableSchema) -> Result<()> {
        validate_record(names,row,meta)?;
        let key=sync_diff_store::budget_key(names,row,keys)?;
        let body=sync_diff_store::budget_body(row)?;
        self.push_encoded(source,key,body).await
    }
    async fn push_encoded(&mut self, source: bool, key: String, body: Vec<u8>) -> Result<()> {
        let label=if source {"源表"} else {"目标表"};
        // Row table payload + unique-index key + B-tree/internal key allowance,
        // with 256B record/header/slack (NOT a whole page per row). Real file bytes
        // are checked each commit; native max_page_count independently caps growth.
        let cost = (key.len() as u64).checked_mul(3).and_then(|n|n.checked_add(body.len() as u64 + 256)).ok_or_else(||anyhow!("spill预算溢出"))?;
        self.bytes = self.bytes.checked_add(cost).ok_or_else(||anyhow!("spill预算溢出"))?;
        if self.bytes > SPILL_BYTES { bail!("差异预览私有spill超过4GiB预算，未返回不完整报告"); }
        let count = if source { self.source_rows } else { self.target_rows };
        if count >= MAX_ROWS { bail!("差异预览单侧行数超过1000万预算"); }
        if self.rows.iter().any(|pending| pending.source == source && pending.key == key) {
            bail!("{label}存在重复主键值，无法安全进行增量比较");
        }
        let row_bytes = key.len().saturating_add(body.len()).saturating_add(64);
        if !self.rows.is_empty() && self.row_bytes.saturating_add(row_bytes) > 1024 * 1024 { self.flush().await?; }
        self.rows.push(PendingRow { source, sequence: count as i64, key, body });
        self.row_bytes += row_bytes; self.pending += 1;
        if source { self.source_rows += 1; } else { self.target_rows += 1; }
        if self.rows.len() >= 128 || self.row_bytes >= 1024 * 1024 { self.flush().await?; }
        Ok(())
    }
    async fn flush(&mut self) -> Result<()> {
        if self.rows.is_empty() { return Ok(()); }
        let rows = std::mem::take(&mut self.rows);
        // Scratch is private and has no triggers/defaults. 128 rows use <=384
        // bindings: batching it does not change any user database transaction.
        for source in [true, false] {
            let selected = rows.iter().filter(|row| row.source == source).collect::<Vec<_>>();
            if selected.is_empty() { continue; }
            let sql = if source { "INSERT INTO s(seq,k,body) " } else { "INSERT INTO t(seq,k,body) " };
            let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(sql);
            query.push_values(selected, |mut bind, row| {
                bind.push_bind(row.sequence).push_bind(row.key.as_bytes()).push_bind(&row.body);
            });
            if let Err(error) = query.build().execute(&mut self.connection).await {
                if error.as_database_error().is_some_and(|error| error.is_unique_violation()) {
                    bail!("{}存在重复主键值，无法安全比较",if source { "源表" } else { "目标表" });
                }
                return Err(error.into());
            }
        }
        self.row_bytes = 0; self.pending = 0;
        crate::sql_history::query("COMMIT; BEGIN").execute(&mut self.connection).await?;
        self.check_disk()?;
        self.rows = Vec::with_capacity(128);
        Ok(())
    }
    async fn report(&mut self, snames: &[String], tnames: &[String], cols: &[(&SyncColumnMapping,&ColumnInfo)], pk: &[String], target_meta: &crate::models::TableSchema, report: &mut SyncDiffReport) -> Result<()> {
        self.flush().await?;
        crate::sql_history::query("COMMIT").execute(&mut self.connection).await?;
        self.check_disk()?;
        report.source_rows = self.source_rows; report.target_rows = self.target_rows; report.keyed = true;
        report.missing_in_target = checked_count(crate::sql_history::query_scalar("SELECT COUNT(*) FROM s WHERE NOT EXISTS(SELECT 1 FROM t WHERE t.k=s.k)").fetch_one(&mut self.connection).await?)?;
        report.extra_in_target = checked_count(crate::sql_history::query_scalar("SELECT COUNT(*) FROM t WHERE NOT EXISTS(SELECT 1 FROM s WHERE s.k=t.k)").fetch_one(&mut self.connection).await?)?;
        // BLOB equality in ONE local index, not a merge of unverified server collations.
        // Indexes are used for point joins; no full-table ORDER BY/temp sort is required.
        // Target numeric affinity also normalizes source TEXT in row_differs.
        let numeric_meta = crate::models::TableSchema { columns: cols.iter().map(|(m,c)| {
            let mut column = (**c).clone();
            if target_meta.columns.iter().any(|tc|tc.name==m.target && numeric_family(&tc.data_type)) { column.data_type="NUMERIC".into(); }
            column
        }).collect(), ..target_meta.clone() };
        let mut target_numeric_meta=target_meta.clone();
        for column in &mut target_numeric_meta.columns {
            if cols.iter().any(|(m,c)|m.target==column.name && numeric_family(&c.data_type)) { column.data_type="NUMERIC".into(); }
        }
        let mut stream = crate::sql_history::query("SELECT s.body AS sb,t.body AS tb FROM s JOIN t ON t.k=s.k").fetch(&mut self.connection);
        while let Some(row) = stream.try_next().await? {
            let source: Vec<Option<String>> = serde_json::from_slice(row.try_get::<&[u8],_>("sb")?)?;
            let target: Vec<Option<String>> = serde_json::from_slice(row.try_get::<&[u8],_>("tb")?)?;
            validate_record(snames, &source, &numeric_meta)?;
            validate_record(tnames, &target, &target_numeric_meta)?;
            if row_differs(snames,&source,tnames,&target,cols,Some(target_meta))? {
                report.changed += 1;
                if report.changed_samples.len() < 5 {
                    let sample = pk.iter().map(|key| {
                        snames.iter().position(|name|name==key).and_then(|i|source.get(i)).and_then(Option::as_deref).unwrap_or("NULL").to_owned()
                    }).collect::<Vec<_>>().join(", ");
                    report.changed_samples.push(sample);
                }
            }
        }
        Ok(())
    }
}

async fn sqlite_read_on(reader: &mut sqlx::SqliteConnection, table: &str, clause: Option<&str>, expected: Option<&crate::models::TableSchema>, keys: &[String], source: bool, mut index: Option<&mut Store>, legacy: bool) -> Result<u64> {
    let current = sqlite_metadata(reader,table).await?;
    let Some(expected) = expected else { if current.is_some() { bail!("目标表在预览期间创建，请重新预览"); } return Ok(0); };
    let current = current.ok_or_else(||anyhow!("表在预览期间消失，请重新预览"))?;
    verify_metadata(expected,&current)?;
    let full = sqlite_table(table);
    if index.is_none() {
        let sql = format!("SELECT COUNT(*) FROM {full}{}",where_sql(clause));
        return checked_count(crate::sql_history::query_scalar(&sql).fetch_one(&mut *reader).await?);
    }
    let names = expected.columns.iter().map(|c|c.name.clone()).collect::<Vec<_>>();
    if legacy {
        // A fixed pooled connection is the only valid identity for private :memory:.
        // No pool/native limits are mutated: raw driver buffers are instead bounded
        // by total bytes/rows preflight in this SAME transaction before SELECT.
        let size=names.iter().map(|n|format!("COALESCE(length(CAST({} AS BLOB)),0)",quote_ident(&DbKind::of("sqlite"),n))).collect::<Vec<_>>().join(" + ");
        let sql=format!("SELECT COUNT(*) AS n,COALESCE(SUM({size}),0) AS bytes,COALESCE(MAX({size}),0) AS biggest FROM {full}{}",where_sql(clause));
        let row=crate::sql_history::query(&sql).fetch_one(&mut *reader).await?;
        if checked_count(row.try_get("n")?)?>LEGACY_ROWS || checked_count(row.try_get("bytes")?)?>LEGACY_BYTES || checked_count(row.try_get("biggest")?)?>ROW_BYTES as u64 {
            bail!("私有SQLite :memory:兼容预览仅验证每侧2048行/16MiB/单行8MiB；大数据请使用文件库/shared-memory单行快照");
        }
    }
    // table_info intentionally excludes generated columns, as the old mapping did.
    // Project its known columns instead of changing results/rejecting generated tables.
    let projection=names.iter().map(|name|quote_ident(&DbKind::of("sqlite"),name)).collect::<Vec<_>>().join(", ");
    let sql=format!("SELECT {projection} FROM {full}{}",where_sql(clause));
    let mut stream=crate::sql_history::query(&sql).fetch(&mut *reader);
    let mut count=0u64;
    while let Some(row)=stream.try_next().await? {
        if row.len()!=names.len() || row.columns().iter().zip(&names).any(|(c,n)|c.name()!=n){bail!("SQLite游标列身份与快照元数据不一致");}
        let store = index.as_deref_mut().ok_or_else(||anyhow!("spill未初始化"))?;
        if store.is_adaptive() {
            if count % 128 == 0 { store.batch_boundary().await?; }
            let values = sqlite_values_budgeted(&row)?;
            store.push_owned(source,&names,values,keys,expected).await?;
        } else {
            let values=sqlite_values(&row)?;
            store.push(source,&names,&values,keys,expected).await?;
        }
        count+=1;
    }
    Ok(count)
}

async fn sqlite_side(pool: &sqlx::SqlitePool, table: &str, clause: Option<&str>, expected: Option<&crate::models::TableSchema>, keys: &[String], source: bool, index: Option<&mut Store>) -> Result<u64> {
    // SQLx 0.8 can panic serializing hand-constructed :memory: connect options.
    // Inspect the real connection instead: any memory database uses its existing
    // identity and the bounded compatibility path, never a reconstructed URL.
    let mut anchor = pool.acquire().await?;
    let filename: String = crate::sql_history::query_scalar("SELECT file FROM pragma_database_list WHERE name='main'")
        .fetch_one(&mut *anchor).await?;
    if filename.is_empty() {
        let mut transaction=anchor.begin().await?;
        let result=sqlite_read_on(&mut transaction,table,clause,expected,keys,source,index,true).await;
        let rollback=transaction.rollback().await;
        // SQLx Transaction Drop also rolls back on future cancellation. The original
        // pool anchor is NOT closed (closing a private memory DB would destroy data).
        rollback.context("私有SQLite差异快照回滚未确认")?;
        return result;
    }
    drop(anchor);
    let mut reader=sqlite_reader(pool).await?;
    let result:Result<u64>=async{
        crate::sql_history::query("BEGIN").execute(&mut reader).await?;
        sqlite_read_on(&mut reader,table,clause,expected,keys,source,index,false).await
    }.await;
    let rollback=crate::sql_history::query("ROLLBACK").execute(&mut reader).await;
    let close=reader.close().await;
    match(result,rollback,close){(Ok(n),Ok(_),Ok(()))=>Ok(n),(result,rollback,close)=>Err(result.err().unwrap_or_else(||anyhow!("差异快照释放失败")).context(format!("只读快照未返回报告；rollback={} close={}",rollback.is_ok(),close.is_ok())))}
}

// Small compatibility path uses a server-side byte/row preflight IN THE SAME
// read-only snapshot as the bounded fetch. It is deliberately not called streaming:
// SQLx network packets/metadata can allocate before the application sees a row.
// The legacy fetch is capped at 2049 rows even if a non-MVCC engine changes mid-read.
async fn legacy_side(db: &Db, kind: &DbKind, schema: &str, table: &str, clause: Option<&str>, meta: &crate::models::TableSchema, keys: &[String], source: bool, mut index: Option<&mut Store>) -> Result<u64> {
    let names=meta.columns.iter().map(|c|c.name.clone()).collect::<Vec<_>>();
    if names.len()>MAX_COLUMNS { bail!("差异预览列数超过1024预算"); }
    let full=qualified(kind,schema,table); let filter=where_sql(clause);
    let sql=format!("SELECT * FROM {full}{filter} LIMIT {}",LEGACY_ROWS+1);
    let bytes=names.iter().map(|n| match kind.protocol() {
        Protocol::Postgres=>format!("COALESCE(octet_length(CAST({} AS text)),0)",quote_ident(kind,n)),
        _=>format!("COALESCE(OCTET_LENGTH(CAST({} AS BINARY)),0)",quote_ident(kind,n)),
    }).collect::<Vec<_>>().join(" + ");
    let preflight=format!("SELECT COUNT(*) AS n,COALESCE(SUM({bytes}),0) AS total,COALESCE(MAX({bytes}),0) AS biggest FROM {full}{filter}");
    let mut values:Vec<Vec<Option<String>>>=Vec::new();
    macro_rules! gate { ($row:expr, $decode:path) => {{
        let row=$row; let decoded=$decode(&row,3);
        let n=decoded[0].as_deref().ok_or_else(||anyhow!("行数无法解码"))?.parse::<u64>()?;
        let total=decoded[1].as_deref().ok_or_else(||anyhow!("字节总数无法解码"))?.parse::<sqlx::types::BigDecimal>()?;
        let biggest=decoded[2].as_deref().ok_or_else(||anyhow!("单行字节数无法解码"))?.parse::<sqlx::types::BigDecimal>()?;
        if n>LEGACY_ROWS || total>sqlx::types::BigDecimal::from(LEGACY_BYTES) || biggest>sqlx::types::BigDecimal::from(ROW_BYTES as u64) {
            bail!("PG/MySQL 差异预览 legacy 内存预算：每侧最多2048行、16MiB原始字节、8MiB单行；大型原生游标/驱动包尚未验证，请缩小源where范围或使用SQLite有界路径");
        } n
    }}; }
    macro_rules! decode { ($row:expr, $convert:path) => {{
        let row=$row;
        if row.len()!=names.len() || row.columns().iter().zip(&names).any(|(c,n)|c.name()!=n) { bail!("差异游标列身份改变，请重新预览"); }
        let mut vals=$convert(&row,names.len());
        for i in 0..names.len() {
            let raw=row.try_get_raw(i)?;
            if raw.is_null(){continue;}
            let ty=raw.type_info().name().to_ascii_uppercase();
            if ty.contains("BLOB") || ty.contains("BINARY") || ty=="BYTEA" { vals[i]=row.try_get::<Option<Vec<u8>>,_>(i)?.map(|b|format!("0x{}",b.iter().map(|b|format!("{b:02X}")).collect::<String>())); }
            if vals[i].is_none() { bail!("差异预览列{}无法无损解码，未伪造NULL/equal",names[i]); }
        }
        vals
    }}; }
    match db {
        Db::Postgres(pool)=>{
            let mut connection=pool.acquire().await?;
            connection.close_on_drop();
            let result:Result<u64>=async {
                crate::sql_history::query("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY").execute(&mut *connection).await?;
                if index.is_none() { return checked_count(crate::sql_history::query_scalar(&format!("SELECT COUNT(*) FROM {full}{filter}")).fetch_one(&mut *connection).await?); }
                let n=gate!(crate::sql_history::query(&preflight).fetch_one(&mut *connection).await?,crate::values::pg_row_values);
                let rows=crate::sql_history::query(&sql).fetch_all(&mut *connection).await?;
                if rows.len() as u64 != n { bail!("差异预览快照行数改变，未返回不完整报告"); }
                let mut used=0usize;
                for row in rows {
                    let mut vals=crate::values::pg_row_values(&row,names.len());
                    crate::values::pg_resolve_money_values(&mut connection,&row,&mut vals).await?;
                    // MONEY resolution is deliberately on this exact session after the
                    // bounded fetch; the generic decode must not erase its resolved text.
                    for i in 0..names.len() {
                        let raw=row.try_get_raw(i)?;if raw.is_null(){continue;}
                        if raw.type_info().name()=="BYTEA" { vals[i]=row.try_get::<Option<Vec<u8>>,_>(i)?.map(|b|format!("0x{}",b.iter().map(|b|format!("{b:02X}")).collect::<String>())); }
                        if vals[i].is_none(){bail!("PG差异列{}无法无损解码，未伪造NULL",names[i]);}
                    }
                    if row.len()!=names.len() || row.columns().iter().zip(&names).any(|(c,n)|c.name()!=n){bail!("PG差异游标列身份改变");}
                    validate_record(&names,&vals,meta)?;
                    used=used.checked_add(serde_json::to_vec(&vals)?.len()).ok_or_else(||anyhow!("内存预算溢出"))?;
                    if used>LEGACY_BYTES as usize { bail!("PG差异预览转换后内存超过16MiB预算"); }
                    values.push(vals);
                }
                Ok(n)
            }.await;
            let close=connection.close().await;
            let n=result?;close?;
            if index.is_none(){return Ok(n);}
        }
        Db::Mysql(pool)=>{
            let mut connection=pool.acquire().await?;
            connection.close_on_drop();
            let result:Result<u64>=async {
                // TiDB 的 READ ONLY 只是 noop 占位（默认 1235），按服务端版本降级为普通事务。
                let (version,comment):(String,String)=crate::sql_history::query_as("SELECT @@version, @@version_comment").fetch_one(&mut *connection).await?;
                if crate::db::mysql_read_only_unsupported(&version,&comment){(&mut *connection).execute("START TRANSACTION").await?;}else{crate::sql_history::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ").execute(&mut *connection).await?;(&mut *connection).execute("START TRANSACTION READ ONLY").await?;}
                // Nontransactional tables cannot promise the same preflight/data snapshot.
                let namespace=if schema.is_empty(){crate::sql_history::query_scalar::<_,String>("SELECT DATABASE()").fetch_one(&mut *connection).await?}else{schema.to_owned()};
                let engine=crate::sql_history::query("SELECT ENGINE,TABLE_TYPE FROM information_schema.tables WHERE table_schema=? AND table_name=?")
                    .bind(&namespace).bind(table).fetch_optional(&mut *connection).await?.ok_or_else(||anyhow!("MySQL差异表引擎不可见"))?;
                if !crate::schema::mysql_get(&engine,"ENGINE").is_some_and(|e|e.eq_ignore_ascii_case("InnoDB")) { bail!("MySQL差异只验证InnoDB普通表单快照；视图/非事务引擎未验证"); }
                if index.is_none(){return checked_count(crate::sql_history::query_scalar(&format!("SELECT COUNT(*) FROM {full}{filter}")).fetch_one(&mut *connection).await?);}
                let n=gate!(crate::sql_history::query(&preflight).fetch_one(&mut *connection).await?,crate::values::my_row_values);
                let mut stream=crate::sql_history::query(&sql).fetch(&mut *connection);
                let mut used=0usize;
                while let Some(row)=stream.try_next().await? {
                    let vals=decode!(row,crate::values::my_row_values);
                    validate_record(&names,&vals,meta)?;
                    used=used.checked_add(serde_json::to_vec(&vals)?.len()).ok_or_else(||anyhow!("内存预算溢出"))?;
                    if used>LEGACY_BYTES as usize || values.len() as u64>=LEGACY_ROWS {bail!("MySQL差异预览转换后内存/行数超过预算");}
                    values.push(vals);
                }
                if values.len() as u64!=n{bail!("MySQL差异快照行数改变");}Ok(n)
            }.await;
            let close=connection.close().await;
            let n=result?;close?;
            if index.is_none(){return Ok(n);}
        }
        _=>bail!("该协议的有界差异预览/快照尚未验证；未进入无上限legacy全表读取"),
    }
    for row in values { index.as_deref_mut().ok_or_else(||anyhow!("spill未初始化"))?.push(source,&names,&row,keys,meta).await?; }
    Ok(if source{index.as_deref().unwrap().source_rows}else{index.as_deref().unwrap().target_rows})
}

async fn side(db:&Db,kind:&DbKind,schema:&str,table:&str,clause:Option<&str>,meta:Option<&crate::models::TableSchema>,keys:&[String],source:bool,index:Option<&mut Store>)->Result<u64>{
    match db {
        Db::Sqlite(pool)=>sqlite_side(pool,table,clause,meta,keys,source,index).await,
        _=>{let Some(meta)=meta else{return Ok(0)};legacy_side(db,kind,schema,table,clause,meta,keys,source,index).await}
    }
}

pub(super) async fn compare(source:&Db,source_kind:&DbKind,target:&Db,target_kind:&DbKind,req:&SyncDiffRequest,
    source_meta:&crate::models::TableSchema,target_meta:Option<&crate::models::TableSchema>,cols:&[(&SyncColumnMapping,&ColumnInfo)],mut report:SyncDiffReport)->Result<SyncDiffReport>{
    if req.where_clause.as_ref().is_some_and(|clause|clause.len()>ROW_BYTES) || source_meta.columns.len()>MAX_COLUMNS || target_meta.is_some_and(|meta|meta.columns.len()>MAX_COLUMNS) { bail!("差异预览SQL/列元数据超过已验证预算"); }
    let pk_source=cols.iter().filter(|(_,c)|c.primary_key).map(|(m,_)|m.source.clone()).collect::<Vec<_>>();
    let pk_target=cols.iter().filter(|(_,c)|c.primary_key).map(|(m,_)|m.target.clone()).collect::<Vec<_>>();
    if target_meta.is_none() || pk_source.is_empty(){
        report.source_rows=side(source,source_kind,&req.source_schema,&req.source_table,req.where_clause.as_deref(),Some(source_meta),&[],true,None).await?;
        report.target_rows=side(target,target_kind,&req.target_schema,&req.target_table,None,target_meta,&[],false,None).await?;
        if target_meta.is_none(){report.missing_in_target=report.source_rows;report.keyed=!pk_source.is_empty();report.target_missing_columns=cols.iter().map(|(m,_)|m.target.clone()).collect();report.message=Some("目标表不存在：全部源行都将作为缺失行写入（结构模式或启用建表时自动创建）。".into());}
        else{report.message=Some("源表没有主键：仅统计行数与结构差异；缺失/多出/内容差异需要主键才能比较。".into());}
        return Ok(report);
    }
    let target_meta=target_meta.ok_or_else(||anyhow!("目标元数据缺失"))?;
    let mut index = Store::new();
    let result:Result<SyncDiffReport>=async{
        side(source,source_kind,&req.source_schema,&req.source_table,req.where_clause.as_deref(),Some(source_meta),&pk_source,true,Some(&mut index)).await?;
        side(target,target_kind,&req.target_schema,&req.target_table,None,Some(target_meta),&pk_target,false,Some(&mut index)).await?;
        let snames=source_meta.columns.iter().map(|c|c.name.clone()).collect::<Vec<_>>();
        let tnames=target_meta.columns.iter().map(|c|c.name.clone()).collect::<Vec<_>>();
        index.report(&snames,&tnames,cols,&pk_source,target_meta,&mut report).await?;Ok(report)
    }.await;
    let close = index.close().await;
    match (result, close) {
        (Ok(report), Ok(())) => Ok(report),
        (result, close) => Err(result.err().unwrap_or_else(|| anyhow!("差异索引清理未确认"))
            .context(format!("未发布预览报告；索引close/cleanup={}",close.is_ok()))),
    }
}

// New path never touches legacy/network drivers or reopens a memory database.
pub(super) async fn validate_memory_database(db: &Db, kind: &DbKind) -> Result<()> {
    let Db::Sqlite(pool) = db else { bail!("MEMORY_BUDGET_UNSUPPORTED: 新预算比较仅认证原生文件SQLite；其它协议请使用旧API"); };
    if kind.protocol() != Protocol::Sqlite { bail!("MEMORY_BUDGET_UNSUPPORTED: 需要原生SQLite协议"); }
    let filename: String = crate::sql_history::query_scalar("SELECT file FROM pragma_database_list WHERE name='main'").fetch_one(pool).await?;
    if filename.is_empty() { bail!("MEMORY_BUDGET_UNSUPPORTED: 新预算比较不认证私有/shared-memory SQLite，仅支持原生文件库"); }
    Ok(())
}

pub(super) async fn compare_with_memory(source: &sqlx::SqlitePool, source_kind: &DbKind, target: &sqlx::SqlitePool, target_kind: &DbKind,
    req: &SyncDiffRequest, memory: &DiffMemoryOptions, controller: &std::sync::Arc<super::diff_memory_budget::Controller>) -> Result<SyncDiffReport> {
    validate_diff_memory_shape(req)?;
    transfer::validate_where(req.where_clause.as_deref())?;
    if req.where_clause.as_ref().is_some_and(|c|c.len()>ROW_BYTES) { bail!("差异预览SQL超过预算"); }
    let mut budget = controller.start(memory)?;
    let buffer_rows = budget.reader_buffer_rows();
    let mut store = Store::with_budget(budget);
    let mut source_reader = match sqlite_reader_buffered(source,buffer_rows).await {
        Ok(reader) => reader,
        Err(error) => { store.close().await?; return Err(error); }
    };
    let mut target_reader = match sqlite_reader_buffered(target,buffer_rows).await {
        Ok(reader) => reader,
        Err(error) => { let close = source_reader.close().await; let cleanup = store.close().await; return Err(error.context(format!("reader_close={} scratch_cleanup={}",close.is_ok(),cleanup.is_ok()))); }
    };
    let result: Result<SyncDiffReport> = async {
        crate::sql_history::query("BEGIN").execute(&mut source_reader).await?;
        crate::sql_history::query("BEGIN").execute(&mut target_reader).await?;
        // Catalog SELECTs fix both sides independently BEFORE mappings/count/data.
        let sm = sqlite_metadata(&mut source_reader,&req.source_table).await?.ok_or_else(||anyhow!("源表不存在或没有可比较的列"))?;
        let tm = sqlite_metadata(&mut target_reader,&req.target_table).await?;
        if sm.columns.is_empty() { bail!("源表不存在或没有可比较的列"); }
        let metadata_bytes = std::iter::once(&sm).chain(tm.iter()).flat_map(|m|&m.columns)
            .map(|c|c.name.capacity()+c.data_type.capacity()+c.default.as_ref().map_or(0,String::capacity)+128).sum::<usize>();
        if metadata_bytes > 1024*1024 { bail!("MEMORY_BUDGET_METADATA_TOO_LARGE: 元数据超出已预留工作区"); }
        let mapping = if req.columns.is_empty() { sm.columns.iter().map(|c|SyncColumnMapping{source:c.name.clone(),target:c.name.clone(),target_type:None}).collect::<Vec<_>>() } else { req.columns.clone() };
        let mapping_bytes = mapping.iter().map(|m|m.source.capacity()+m.target.capacity()+m.target_type.as_ref().map_or(0,String::capacity)+96).sum::<usize>();
        if mapping.len()>MAX_COLUMNS || mapping_bytes>1024*1024 { bail!("MEMORY_BUDGET_METADATA_TOO_LARGE: 列映射超过工作区"); }
        let cols = mapping.iter().map(|m|sm.columns.iter().find(|c|c.name==m.source).map(|c|(m,c)).ok_or_else(||anyhow!("源列不存在: {}",m.source))).collect::<Result<Vec<_>>>()?;
        validate_key_mapping(&sm,&mapping,&[])?;
        let pk_source = cols.iter().filter(|(_,c)|c.primary_key).map(|(m,_)|m.source.clone()).collect::<Vec<_>>();
        let pk_target = cols.iter().filter(|(_,c)|c.primary_key).map(|(m,_)|m.target.clone()).collect::<Vec<_>>();
        let mut report = SyncDiffReport { source_rows:0,target_rows:0,keyed:false,missing_in_target:0,extra_in_target:0,changed:0,changed_samples:Vec::new(),target_missing_columns:Vec::new(),target_extra_columns:Vec::new(),message:None,memory:None };
        if let Some(tm) = &tm {
            report.target_missing_columns=cols.iter().filter(|(m,_)|!tm.columns.iter().any(|c|c.name==m.target)).map(|(m,_)|m.target.clone()).collect();
            report.target_extra_columns=tm.columns.iter().filter(|c|!cols.iter().any(|(m,_)|m.target==c.name)).map(|c|c.name.clone()).collect();
        }
        if tm.is_none() || pk_source.is_empty() {
            report.source_rows=sqlite_read_on(&mut source_reader,&req.source_table,req.where_clause.as_deref(),Some(&sm),&[],true,None,false).await?;
            report.target_rows=sqlite_read_on(&mut target_reader,&req.target_table,None,tm.as_ref(),&[],false,None,false).await?;
            if tm.is_none() {
                report.missing_in_target=report.source_rows;report.keyed=!pk_source.is_empty();report.target_missing_columns=cols.iter().map(|(m,_)|m.target.clone()).collect();
                report.message=Some("目标表不存在：全部源行都将作为缺失行写入（结构模式或启用建表时自动创建）。".into());
            } else { report.message=Some("源表没有主键：仅统计行数与结构差异；缺失/多出/内容差异需要主键才能比较。".into()); }
            report.memory=store.memory_stats("count_only");
            return Ok(report);
        }
        let tm=tm.as_ref().ok_or_else(||anyhow!("目标元数据缺失"))?;
        sqlite_read_on(&mut source_reader,&req.source_table,req.where_clause.as_deref(),Some(&sm),&pk_source,true,Some(&mut store),false).await?;
        sqlite_read_on(&mut target_reader,&req.target_table,None,Some(tm),&pk_target,false,Some(&mut store),false).await?;
        store.batch_boundary().await?;
        let snames=sm.columns.iter().map(|c|c.name.clone()).collect::<Vec<_>>();
        let tnames=tm.columns.iter().map(|c|c.name.clone()).collect::<Vec<_>>();
        store.report(&snames,&tnames,&cols,&pk_source,tm,&mut report).await?;
        report.memory=store.memory_stats(store.backend());
        Ok(report)
    }.await;
    // Readers and scratch really close BEFORE the lease and owned maps are dropped.
    let source_rollback=crate::sql_history::query("ROLLBACK").execute(&mut source_reader).await;
    let target_rollback=crate::sql_history::query("ROLLBACK").execute(&mut target_reader).await;
    let source_close=source_reader.close().await; let target_close=target_reader.close().await;
    let store_close=store.close().await;
    let clean=source_rollback.is_ok()&&target_rollback.is_ok()&&source_close.is_ok()&&target_close.is_ok()&&store_close.is_ok();
    match result {
        Ok(mut report) if clean => {
            // Every reader/scratch/store owner has really settled; report the final
            // released ledger, while retaining its actual accounted peak snapshot.
            if let Some(memory)=&mut report.memory { memory.used_bytes=0; }
            Ok(report)
        },
        result => Err(result.err().unwrap_or_else(||anyhow!("差异快照/索引清理未确认")).context(format!("未发布预览报告；source_rollback={} target_rollback={} source_close={} target_close={} scratch_close={}",source_rollback.is_ok(),target_rollback.is_ok(),source_close.is_ok(),target_close.is_ok(),store_close.is_ok()))),
    }
}

#[cfg(test)] #[path="sync_diff_tests.rs"] mod tests;
