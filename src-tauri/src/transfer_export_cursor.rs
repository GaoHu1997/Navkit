//! 同一物理连接上的 describe + 单 SELECT 游标；不使用 OFFSET 或 fetch_all。
use super::{Db, DbKind, ExportOptions, ExportSink, TaskControl};
use super::transfer_output::StagedOutput;
use crate::models::Protocol;
use futures::TryStreamExt;
use sqlx::{Column, Executor, Row, TypeInfo, ValueRef};
use anyhow::{anyhow, bail, Context, Result};
#[path = "transfer_mysql_export.rs"] mod mysql;

fn select_sql(kind: &DbKind, options: &ExportOptions) -> Result<String> {
    if options.limit.is_some_and(|limit| limit > i64::MAX as u64) { bail!("导出行数上限超过当前原生游标可表示的范围"); }
    if kind.protocol() == Protocol::Mssql { crate::object_identity::SqlServerObject::parse(&options.schema, &options.table)?; }
    let top = if kind.protocol() == Protocol::Mssql { options.limit.map(|limit| format!("TOP ({limit}) ")).unwrap_or_default() } else { String::new() };
    let mut sql = format!("SELECT {top}* FROM {}", crate::db::qualified(kind, &options.schema, &options.table));
    if let Some(clause) = options.where_clause.as_deref().filter(|clause| !clause.trim().is_empty()) { sql.push_str(&format!(" WHERE ({clause})")); }
    if kind.protocol() != Protocol::Mssql { if let Some(limit) = options.limit { sql.push_str(&format!(" LIMIT {limit}")); } }
    Ok(sql)
}

async fn sqlite_session(pool: &sqlx::SqlitePool, kind: &DbKind, options: &ExportOptions) -> Result<(sqlx::pool::PoolConnection<sqlx::Sqlite>, Option<String>, bool)> {
    let mut connection = pool.acquire().await?;
    let ddl = if options.format.eq_ignore_ascii_case("sql") {
        // SQLite 的 schema 只能是 main/temp/attached；调用方可能传入数据库文件名，
        // 此时必须回退到 main，否则会查询不存在的 <name>.sqlite_schema（BUG-R1-028）。
        let attached: Vec<String> = crate::sql_history::query("PRAGMA database_list").fetch_all(&mut *connection).await?
            .iter().filter_map(|row| crate::values::sqlite_row_values(row, 3).get(1).cloned().flatten()).collect();
        let schema = if options.schema.is_empty() || !attached.iter().any(|n| n == &options.schema) { "main" } else { options.schema.as_str() };
        crate::sql_history::query_scalar::<_, Option<String>>(&format!("SELECT sql FROM {}.sqlite_schema WHERE type = 'view' AND name = ?", crate::db::quote_ident(kind, schema)))
            .bind(&options.table).fetch_optional(&mut *connection).await?.flatten()
    } else { None };
    Ok((connection, ddl, false))
}

async fn pg_session(pool: &sqlx::PgPool, kind: &DbKind, options: &ExportOptions) -> Result<(sqlx::pool::PoolConnection<sqlx::Postgres>, Option<String>, bool)> {
    let mut connection = pool.acquire().await?;
    let ddl = if options.format.eq_ignore_ascii_case("sql") {
        let row = crate::sql_history::query("SELECT c.relkind::text, pg_get_viewdef(c.oid, true) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = COALESCE(NULLIF($1, ''), current_schema()) AND c.relname = $2 AND c.relkind IN ('v','m')")
            .bind(&options.schema).bind(&options.table).fetch_optional(&mut *connection).await?;
        row.map(|row| -> Result<String> { let ty: String = row.try_get(0)?; let definition: String = row.try_get(1)?; Ok(format!("CREATE {}VIEW {} AS {}", if ty == "m" { "MATERIALIZED " } else { "" }, crate::db::qualified(kind, &options.schema, &options.table), definition.trim().trim_end_matches(';'))) }).transpose()?
    } else { None };
    Ok((connection, ddl, false))
}

/// 旧 SQL 转储入口的视图钩子：在固定会话中确认类型并读取真实定义，不读取视图数据。
pub(super) async fn view_definition(db:&Db, kind:&DbKind, schema:&str, name:&str)->Result<Option<String>> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    match db {
        Db::Sqlite(pool) => {
            let mut connection=pool.acquire().await?;let schema=if schema.is_empty(){"main"}else{schema};
            Ok(crate::sql_history::query_scalar::<_,Option<String>>(&format!("SELECT sql FROM {}.sqlite_schema WHERE type = 'view' AND name = ?",crate::db::quote_ident(kind,schema))).bind(name).fetch_optional(&mut *connection).await?.flatten())
        },
        Db::Mysql(pool) => {
            let mut connection=pool.acquire().await?;
            let namespace=if schema.is_empty(){crate::sql_history::query_scalar::<_,Option<String>>("SELECT DATABASE()").fetch_one(&mut *connection).await?.ok_or_else(||anyhow!("请选择导出数据库"))?}else{schema.to_owned()};
            let row=crate::sql_history::query("SELECT TABLE_TYPE, ENGINE FROM information_schema.tables WHERE table_schema = ? AND table_name = ?").bind(&namespace).bind(name).fetch_optional(&mut *connection).await?;
            // information_schema.TABLE_TYPE 在 MySQL 8 返回 BINARY 排序规则，按 String 直接解码会失败。
            if !row.is_some_and(|row|crate::schema::mysql_get(&row,"TABLE_TYPE").is_some_and(|ty|ty.eq_ignore_ascii_case("VIEW"))){return Ok(None);}
            let row=crate::sql_history::query(&format!("SHOW CREATE VIEW {}",crate::db::qualified(kind,&namespace,name))).fetch_one(&mut *connection).await?;
            Ok(Some(row.try_get::<String,_>(1).context("读取真实视图定义需要 SHOW VIEW 权限")?))
        },
        Db::Postgres(pool) => {
            let mut connection=pool.acquire().await?;
            let row=crate::sql_history::query("SELECT c.relkind::text, pg_get_viewdef(c.oid, true) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = COALESCE(NULLIF($1, ''), current_schema()) AND c.relname = $2 AND c.relkind IN ('v','m')").bind(schema).bind(name).fetch_optional(&mut *connection).await?;
            row.map(|row|->Result<String>{let ty:String=row.try_get(0)?;let definition:String=row.try_get(1)?;Ok(format!("CREATE {}VIEW {} AS {}",if ty=="m"{"MATERIALIZED "}else{""},crate::db::qualified(kind,schema,name),definition.trim().trim_end_matches(';')))}).transpose()
        },
        Db::Mssql(database) => {
            let identity=crate::object_identity::SqlServerObject::parse(schema,name)?;let mut client=database.export_session().await;
            let rows=tds_metadata(&mut client,&identity.type_query(kind)).await?;
            if rows.first().and_then(|row|row.first()).and_then(Option::as_deref).map(str::trim)!=Some("V"){return Ok(None);}
            let rows=tds_metadata(&mut client,&identity.view_query(kind)).await?;
            Ok(Some(rows.first().and_then(|row|row.first()).cloned().flatten().filter(|ddl|!ddl.trim().is_empty()).ok_or_else(||anyhow!("视图定义不可见或已加密，需要 VIEW DEFINITION 权限"))?))
        },
        _=>Ok(None),
    }

    }).await
}

/// 旧转储入口复用同一个有界数据游标，写入调用者已固定的 stage；不发布中间文件。
pub(super) async fn append_dump_data(db: &Db, kind: &DbKind, schema: &str, table: &str, stage: &StagedOutput) -> Result<u64> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let task = TaskControl::begin(&format!("dump_{}", uuid::Uuid::new_v4().simple()))?;
    let options = ExportOptions { connection_id: String::new(), schema: schema.into(), table: table.into(), file: String::new(), format: "sql".into(), delimiter: ",".into(), encoding: "utf8".into(), has_header: true, where_clause: None, limit: None };
    let result = export_cursor(db, kind, &options, &task, stage).await;
    task.finish(&result);
    result

    }).await
}

pub(super) async fn export_cursor(db: &Db, kind: &DbKind, options: &ExportOptions, task: &TaskControl, stage: &StagedOutput) -> Result<u64> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let sql = select_sql(kind, options)?;
    let limits = crate::transfer_stream::TransferLimits::default();
    macro_rules! read {
        ($session:expr, $convert:path) => {{
            task.check()?;
            let (mut connection, view, transaction) = $session.await?;
            let result: Result<u64> = async {
                if let Some(definition) = view {
                    task.check()?;
                    ExportSink::view(stage, kind, &options.schema, &definition, &options.encoding)?;
                    return Ok(0);
                }
                let description = crate::sql_history::describe((&mut *connection),&sql).await?;
                let names = description.columns().iter().map(|column| column.name().to_owned()).collect::<Vec<_>>();
                let types = description.columns().iter().map(|column| column.type_info().name().to_owned()).collect::<Vec<_>>();
                let upper_types = types.iter().map(|ty| ty.to_ascii_uppercase()).collect::<Vec<_>>();
                let mut sink = ExportSink::create(stage, &options.format.to_ascii_lowercase(), kind, &options.schema, &options.table, &names, &types, &options.delimiter, options.has_header, &options.encoding)?;
                // 一个 SELECT 对应一个 statement snapshot，空结果仍有 describe 的完整列元数据。
                let mut stream = crate::sql_history::query(&sql).fetch(&mut *connection);
                let mut count = 0u64;
                loop {
                    task.check()?;
                    let Some(row) = stream.try_next().await? else { break };
                    let mut values = $convert(&row, row.len());
                    if values.len() != names.len() || row.len() != names.len() { bail!("导出游标返回的列数与准备时元数据不一致"); }
                    let mut record_bytes = 0u64;
                    for (index, column) in row.columns().iter().enumerate() {
                        if column.name() != names[index] || column.type_info().name() != types[index] { bail!("导出游标列元数据发生变化，目标未发布"); }
                        let raw = row.try_get_raw(index)?;
                        if raw.is_null() { values[index] = None; continue; }
                        let ty = &upper_types[index];
                        if ty.contains("BLOB") || ty.contains("BINARY") || ty == "BYTEA" {
                            let bytes: Vec<u8> = row.try_get(index).context("二进制列不能无损读取，未将其伪造为 NULL")?;
                            let encoded_bytes = bytes.len().checked_mul(2).and_then(|length| length.checked_add(2)).ok_or_else(|| anyhow!("二进制列长度溢出"))?;
                            if u64::try_from(encoded_bytes).map_or(true, |length| length > limits.max_record_bytes.saturating_sub(record_bytes)) { bail!("导出单条记录超过上限"); }
                            let mut value = String::with_capacity(encoded_bytes); value.push_str("0x");
                            const HEX: &[u8; 16] = b"0123456789ABCDEF";
                            for byte in bytes { value.push(HEX[(byte >> 4) as usize] as char); value.push(HEX[(byte & 15) as usize] as char); }
                            values[index] = Some(value);
                        }
                        let value = values[index].as_ref().ok_or_else(|| anyhow!("导出列 {}（{}）尚不能无损解码，未将非 NULL 值伪造为 NULL", names[index], ty))?;
                        record_bytes = record_bytes.checked_add(u64::try_from(value.len())?).ok_or_else(|| anyhow!("导出记录长度溢出"))?;
                        if record_bytes > limits.max_record_bytes { bail!("导出单条记录超过上限"); }
                    }
                    count = count.checked_add(sink.write_rows(std::slice::from_ref(&values))?).ok_or_else(|| anyhow!("导出行数溢出"))?;
                    // 取消仍逐行检查；仅降低进度全局锁/字符串分配频率。
                    if count % 100 == 0 {
                        task.update("export", count, options.limit.unwrap_or(0), "正在读取固定会话游标并生成目标编码文件");
                        tokio::task::yield_now().await;
                    }
                }
                drop(stream);
                task.check()?;
                sink.finish()?;
                task.update("export", count, options.limit.unwrap_or(0), "固定会话游标已读取完成，目标编码文件已生成");
                Ok(count)
            }.await;
            let result = match result {
                Ok(count) if transaction => crate::sql_history::query("ROLLBACK").execute(&mut *connection).await.map(|_| count).map_err(anyhow::Error::from),
                other => other,
            };
            match result {
                Ok(count) => Ok(count),
                Err(error) => {
                    // 取消只在 try_next 完成后发生。销毁专用连接并真实等待关闭，不让未排空会话回池。
                    match connection.close().await {
                        Ok(()) => Err(error),
                        Err(close) => Err(error.context(format!("TRANSFER_EFFECTS_UNKNOWN: 导出读取连接关闭未确认；目标未发布，需要核对连接资源: {close}"))),
                    }
                }
            }
        }};
    }
    match db {
        Db::Sqlite(pool) if kind.protocol() == Protocol::Sqlite => read!(sqlite_session(pool, kind, options), crate::values::sqlite_row_values),
        Db::Mysql(pool) if kind.protocol() == Protocol::Mysql => read!(mysql::session(pool, kind, options, task), crate::values::my_row_values),
        Db::Postgres(pool) if kind.protocol() == Protocol::Postgres => read!(pg_session(pool, kind, options), crate::values::pg_row_values),
        Db::Mssql(client) if kind.protocol() == Protocol::Mssql => mssql_cursor(client, kind, options, task, stage, &sql).await,
        _ => bail!("当前通道尚未验证单物理连接的流式导出游标，没有发送导出 SELECT；未退回全量读取或 OFFSET 分页"),
    }

    }).await
}

type TdsClient = tiberius::Client<tokio_util::compat::Compat<tokio::net::TcpStream>>;
/// 仅元数据可组成有界集合；数据行直接写 sink，绝不汇聚对象结果集。
async fn tds_metadata(client: &mut TdsClient, sql: &str) -> Result<Vec<Vec<Option<String>>>> {
    let mut stream = client.simple_query(sql).await?; let mut rows = Vec::new(); let mut bytes = 0usize;
    while let Some(item) = stream.try_next().await? {
        if let tiberius::QueryItem::Row(row) = item {
            if rows.len() >= 4096 { bail!("SQL Server 导出列元数据超过上限"); }
            let mut values = Vec::with_capacity(row.len());
            for (_, cell) in row.cells() { let value = crate::mssql::MssqlDb::export_cell(cell, crate::transfer_stream::DEFAULT_MAX_RECORD_BYTES as usize - bytes)?; bytes = bytes.checked_add(value.as_ref().map_or(0, String::len)).ok_or_else(|| anyhow!("元数据长度溢出"))?; values.push(value); }
            rows.push(values);
        }
    }
    Ok(rows)
}

async fn mssql_cursor(database: &crate::mssql::MssqlDb, kind: &DbKind, options: &ExportOptions, task: &TaskControl, stage: &StagedOutput, sql: &str) -> Result<u64> {
    task.check()?; let identity = crate::object_identity::SqlServerObject::parse(&options.schema, &options.table)?;
    let mut client = database.export_session().await;
    // 同一个锁贯穿真实 sessionRef、目录描述、数据 SELECT 和 drain/事务恢复。
    let rows = tds_metadata(&mut client, "SELECT CAST(@@SPID AS nvarchar(32)), CAST(@@TRANCOUNT AS nvarchar(32)), CAST(transaction_isolation_level AS nvarchar(32)) FROM sys.dm_exec_sessions WHERE session_id = @@SPID").await?;
    let session = rows.first().and_then(|row| row.first()).and_then(Option::as_deref).ok_or_else(|| anyhow!("未取得真实 SQL Server sessionRef"))?.to_owned();
    let transaction = rows.first().and_then(|row| row.get(1)).and_then(Option::as_deref);
    if transaction != Some("0") { bail!("SQL Server sessionRef={session} 已存在用户事务，导出不会借用或回滚该事务"); }
    let isolation = match rows.first().and_then(|row| row.get(2)).and_then(Option::as_deref) {
        Some("1") => "READ UNCOMMITTED", Some("2") => "READ COMMITTED", Some("3") => "REPEATABLE READ", Some("4") => "SERIALIZABLE", Some("5") => "SNAPSHOT", _ => bail!("SQL Server 原会话隔离级别未知，未改变会话"),
    };
    let result: Result<u64> = async {
        // SERIALIZABLE 保障一致读取，可能持有读锁；不宣称未启用的 MVCC 快照。
        client.execute("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE; BEGIN TRANSACTION", &[]).await?;
        let object_rows = tds_metadata(&mut client, &identity.type_query(kind)).await?;
        let object_type = object_rows.first().and_then(|row| row.first()).and_then(Option::as_deref).map(str::trim).ok_or_else(|| anyhow!("SQL Server 表或视图不存在，或对象目录不可见"))?;
        if object_type != "U" && object_type != "V" { bail!("SQL Server 当前对象不是表或视图"); }
        if options.format.eq_ignore_ascii_case("sql") && object_type == "V" {
            let rows = tds_metadata(&mut client, &identity.view_query(kind)).await?;
            let definition = rows.first().and_then(|row| row.first()).and_then(Option::as_deref).filter(|definition| !definition.trim().is_empty()).ok_or_else(|| anyhow!("SQL Server 视图定义不可见或已加密；需要 VIEW DEFINITION 权限，不生成伪表或 INSERT 视图"))?;
            task.check()?; ExportSink::view(stage, kind, &identity.catalog, definition, &options.encoding)?; return Ok(0);
        }
        let columns = tds_metadata(&mut client, &identity.columns_query(kind)).await?;
        if columns.is_empty() { bail!("SQL Server 对象列目录不可见；需要相应元数据权限，未将其伪造为空表"); }
        let names = columns.iter().map(|row| row.first().cloned().flatten().filter(|name| !name.is_empty()).ok_or_else(|| anyhow!("SQL Server 实际列名缺失"))).collect::<Result<Vec<_>>>()?;
        let types = columns.iter().map(|row| row.get(1).cloned().flatten().filter(|ty| !ty.is_empty()).ok_or_else(|| anyhow!("SQL Server 实际列类型缺失"))).collect::<Result<Vec<_>>>()?;
        let mut stream = client.simple_query(sql).await?; let mut sink = None; let mut description = None; let mut failure = None; let mut count = 0u64;
        while let Some(item) = stream.try_next().await? {
            if failure.is_some() { continue; }
            let consumed: Result<()> = (|| {
                task.check()?;
                match item {
                    tiberius::QueryItem::Metadata(metadata) => {
                        if description.is_some() || metadata.result_index() != 0 { bail!("SQL Server 导出返回额外结果集"); }
                        if metadata.columns().len() != names.len() || metadata.columns().iter().zip(&names).any(|(column, name)| column.name() != name) { bail!("SQL Server 流元数据与真实目录不一致，未发布目标"); }
                        description = Some(metadata.columns().to_vec());
                        sink = Some(ExportSink::create(stage, &options.format.to_ascii_lowercase(), kind, &options.schema, &options.table, &names, &types, &options.delimiter, options.has_header, &options.encoding)?);
                    },
                    tiberius::QueryItem::Row(row) => {
                        let metadata = description.as_ref().ok_or_else(|| anyhow!("SQL Server 数据行之前未返回列元数据"))?;
                        if row.len() != metadata.len() || row.result_index() != 0 || row.columns().iter().zip(metadata).any(|(actual, expected)| actual.name() != expected.name() || actual.column_type() != expected.column_type()) { bail!("SQL Server 导出行的列元数据改变"); }
                        let mut budget = crate::transfer_stream::DEFAULT_MAX_RECORD_BYTES as usize; let mut values = Vec::with_capacity(row.len());
                        for (_, cell) in row.cells() { let value = crate::mssql::MssqlDb::export_cell(cell, budget)?; budget -= value.as_ref().map_or(0, String::len); values.push(value); }
                        count = count.checked_add(sink.as_mut().ok_or_else(|| anyhow!("SQL Server 导出 sink 未准备"))?.write_rows(std::slice::from_ref(&values))?).ok_or_else(|| anyhow!("导出行数溢出"))?;
                        task.update("export", count, options.limit.unwrap_or(0), &format!("固定 SQL Server sessionRef={session} 正在一致读取；可能持有读锁"));
                    },
                }
                Ok(())
            })();
            if let Err(error) = consumed { failure = Some(error); task.update("drain", count, 0, "停止写入，正在排空当前 SQL Server 对象游标；尚未确认停止"); }
            if count % 100 == 0 { tokio::task::yield_now().await; }
        }
        drop(stream);
        if let Some(error) = failure { return Err(error); }
        task.check()?; sink.ok_or_else(|| anyhow!("SQL Server 空结果未返回列元数据"))?.finish()?; Ok(count)
    }.await;
    // simple_query 会先真实 flush 未读协议数据，再恢复我们自己创建的只读事务。
    let cleanup = tds_metadata(&mut client, &format!("IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION; SET TRANSACTION ISOLATION LEVEL {isolation}; SELECT CAST(@@SPID AS nvarchar(32)), CAST(@@TRANCOUNT AS nvarchar(32))")).await;
    match cleanup {
        Ok(rows) if rows.last().is_some_and(|row| row.first().and_then(Option::as_deref) == Some(session.as_str()) && row.get(1).and_then(Option::as_deref) == Some("0")) => result.map_err(|error| error.context(format!("SQL Server sessionRef={session}；当前对象协议已排空，导出事务已回滚，目标未发布"))),
        other => { let detail = other.err().map(|error| format!("{error:#}")).unwrap_or_else(|| "会话或事务读回不一致".into()); Err(anyhow!("TRANSFER_EFFECTS_UNKNOWN: SQL Server sessionRef={session} 的游标排空/隔离级别恢复未确认；目标未发布，需核对连接资源：{detail}")) },
    }
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn sqlserver_export_uses_segmented_identity_top_and_zero_limit() {
        let options = ExportOptions { connection_id:"fixture".into(), schema:"catalog.with.dot".into(), table:"[sales].[order]]detail]".into(), file:String::new(), format:"csv".into(), delimiter:",".into(), encoding:"utf8".into(), has_header:true, where_clause:Some("id > 0".into()), limit:Some(0) };
        assert_eq!(select_sql(&DbKind::of("sqlserver"), &options).unwrap(), "SELECT TOP (0) * FROM [catalog.with.dot].[sales].[order]]detail] WHERE (id > 0)");
    }
    #[test] fn export_paths_refuse_empty_or_relative_directory_before_creating_anything() {
        let relative=format!("navkit-export-relative-{}",uuid::Uuid::new_v4());
        for path in [String::new()," ".into(),format!("{relative}.csv"),format!("{relative}/out.csv"),"C:out.csv".into(),format!("{}/",std::env::temp_dir().display())] {
            assert!(crate::transfer::ensure_writable(&path).is_err(),"必须拒绝：{path}");
        }
        assert!(!std::path::Path::new(&relative).exists());assert!(!std::path::Path::new(&format!("{relative}.csv")).exists());
        let absolute=std::env::temp_dir().join(format!("{relative}.csv"));crate::transfer::ensure_writable(absolute.to_str().unwrap()).unwrap();assert!(!absolute.exists());
    }
}
#[cfg(test)] #[path = "transfer_export_protocol_tests.rs"] mod protocol_tests;
