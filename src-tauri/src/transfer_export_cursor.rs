//! 同一物理连接上的 describe + 单 SELECT 游标；不使用 OFFSET 或 fetch_all。
use super::{Db, DbKind, ExportOptions, ExportSink, TaskControl};
use super::transfer_output::StagedOutput;
use crate::models::Protocol;
use futures::TryStreamExt;
use sqlx::{Column, Executor, Row, TypeInfo, ValueRef};
use anyhow::{anyhow, bail, Context, Result};
#[path = "transfer_mysql_export.rs"] mod mysql;
#[path = "transfer_mongo_export.rs"] mod mongo;
#[path = "transfer_http_export.rs"] mod http_export;

fn select_sql(kind: &DbKind, options: &ExportOptions) -> Result<String> {
    select_sql_projection(kind, options, "*")
}

fn select_sql_projection(kind: &DbKind, options: &ExportOptions, projection: &str) -> Result<String> {
    if kind.protocol() != Protocol::Odbc && options.limit.is_some_and(|limit| limit > i64::MAX as u64) { bail!("导出行数上限超过当前原生游标可表示的范围"); }
    if kind.protocol() == Protocol::Mssql { crate::object_identity::SqlServerObject::parse(&options.schema, &options.table)?; }
    let top = if kind.protocol() == Protocol::Mssql { options.limit.map(|limit| format!("TOP ({limit}) ")).unwrap_or_default() } else { String::new() };
    let mut sql = format!("SELECT {top}{projection} FROM {}", crate::db::qualified(kind, &options.schema, &options.table));
    if let Some(clause) = options.where_clause.as_deref().filter(|clause| !clause.trim().is_empty()) { sql.push_str(&format!(" WHERE ({clause})")); }
    // ODBC 驱动覆盖多个版本和方言；不假定 LIMIT/FETCH FIRST，统一在接收游标时控制行数。
    if !matches!(kind.protocol(), Protocol::Mssql | Protocol::Odbc) {
        if let Some(limit) = options.limit { sql.push_str(&format!(" LIMIT {limit}")); }
    }
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
        Db::Odbc(odb) if kind.protocol()==Protocol::Odbc && kind.id()=="oracle" => oracle_view_definition(odb.as_ref(), kind, schema, name).await,
        _=>Ok(None),
    }

    }).await
}

/// 旧转储入口复用同一个有界数据游标，写入调用者已固定的 stage；不发布中间文件。
pub(super) async fn append_dump_data(db: &Db, kind: &DbKind, schema: &str, table: &str, stage: &StagedOutput) -> Result<u64> {
    append_dump_data_to_schema(db, kind, schema, table, stage, None).await
}

/// Append table data while optionally rewriting only the SQL INSERT target.
/// The source schema remains the read scope; a target schema is used solely for
/// cross-database restore archives, never by the SELECT or metadata query.
pub(super) async fn append_dump_data_to_schema(db: &Db, kind: &DbKind, schema: &str, table: &str, stage: &StagedOutput, target_schema: Option<&str>) -> Result<u64> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let task = TaskControl::begin(&format!("dump_{}", uuid::Uuid::new_v4().simple()))?;
    let options = ExportOptions { connection_id: String::new(), schema: schema.into(), table: table.into(), file: String::new(), format: "sql".into(), delimiter: ",".into(), encoding: "utf8".into(), has_header: true, where_clause: None, limit: None };
    let result = export_cursor_with_sql_schema(db, kind, &options, &task, stage, target_schema).await;
    task.finish(&result);
    result

    }).await
}

pub(super) async fn export_cursor(db: &Db, kind: &DbKind, options: &ExportOptions, task: &TaskControl, stage: &StagedOutput) -> Result<u64> {
    export_cursor_with_sql_schema(db, kind, options, task, stage, None).await
}

async fn export_cursor_with_sql_schema(db: &Db, kind: &DbKind, options: &ExportOptions, task: &TaskControl, stage: &StagedOutput, target_schema: Option<&str>) -> Result<u64> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let sql = if matches!(db, Db::Mongo(_) | Db::Http(_) | Db::Redis(_)) { String::new() } else { select_sql(kind, options)? };
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
                // MONEY's wire integer carries no lc_monetary scale. The server
                // resolves its numeric text in this physical session. Logical types
                // remain MONEY for the sink; wire types are checked separately.
                let execution_sql = if kind.protocol() == Protocol::Postgres && upper_types.iter().any(|ty| ty == "MONEY") {
                    let projection = names.iter().zip(&upper_types).map(|(name, ty)| {
                        let name = crate::db::quote_ident(kind, name);
                        if ty == "MONEY" { format!("({name}::numeric)::text AS {name}") } else { name }
                    }).collect::<Vec<_>>().join(", ");
                    select_sql_projection(kind, options, &projection)?
                } else { sql.clone() };
                let wire_types = if execution_sql != sql {
                    let actual = crate::sql_history::describe((&mut *connection), &execution_sql).await?;
                    if actual.columns().len() != names.len() || actual.columns().iter().zip(&names).any(|(column, name)| column.name() != name) { bail!("MONEY 文本投影与原始对象列元数据不一致"); }
                    actual.columns().iter().map(|column| column.type_info().name().to_owned()).collect::<Vec<_>>()
                } else { types.clone() };
                let mut sink = ExportSink::create_with_sql_schema(stage, &options.format.to_ascii_lowercase(), kind, &options.schema, &options.table, &names, &types, &options.delimiter, options.has_header, &options.encoding, target_schema)?;
                // 一个 SELECT 对应一个 statement snapshot，空结果仍有 describe 的完整列元数据。
                let mut stream = crate::sql_history::query(&execution_sql).fetch(&mut *connection);
                let mut count = 0u64;
                loop {
                    task.check()?;
                    let Some(row) = stream.try_next().await? else { break };
                    let mut values = $convert(&row, row.len());
                    if values.len() != names.len() || row.len() != names.len() { bail!("导出游标返回的列数与准备时元数据不一致"); }
                    // values.rs 用显式标记表示“非 NULL 但无法无损解码”，不能当作真实文本写盘。
                    for (index, value) in values.iter().enumerate() {
                        if value.as_deref().is_some_and(crate::values::is_unsupported_value) {
                            bail!(super::UnsupportedExportValue { column: names[index].clone(), data_type: upper_types[index].clone() });
                        }
                    }
                    let mut record_bytes = 0u64;
                    for (index, column) in row.columns().iter().enumerate() {
                        if column.name() != names[index] || column.type_info().name() != wire_types[index] { bail!("导出游标列元数据发生变化，目标未发布"); }
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
                        let value = values[index].as_ref().ok_or_else(|| anyhow::Error::new(super::UnsupportedExportValue {
                            column: names[index].clone(), data_type: ty.clone(),
                        }))?;
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
        Db::ClickHouse(client) if kind.protocol() == Protocol::Clickhouse => clickhouse_cursor(client, kind, options, task, stage, &sql).await,
        Db::Odbc(odb) if kind.protocol() == Protocol::Odbc => odbc_cursor(odb, kind, options, task, stage, &sql).await,
        Db::Mongo(mongo) if kind.protocol() == Protocol::Mongo => mongo::export(mongo, kind, options, task, stage).await,
        Db::Http(http) if matches!(kind.protocol(), Protocol::Http | Protocol::Native | Protocol::Odbc) => http_export::export(http, kind, options, task, stage).await,
        Db::Redis(_) if kind.protocol() == Protocol::Redis => bail!("Redis 是键值存储，没有表结构，无法按表导出；请使用键浏览或命令行导出"),
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

fn is_binary_odbc_type(ty: &str) -> bool {
    let ty = ty.to_ascii_uppercase();
    ty.contains("BLOB") || ty.contains("BINARY")
}

/// Oracle 视图真实定义（LONG 文本）：只在 SQL 格式导出时调用；找不到视图即视为表。
async fn oracle_view_definition(odb: &crate::odbc_db::OdbcDb, kind: &DbKind, schema: &str, table: &str) -> Result<Option<String>> {
    let needle = table.to_ascii_uppercase().replace('\'', "''");
    let sql = if schema.trim().is_empty() {
        format!("SELECT TEXT FROM USER_VIEWS WHERE VIEW_NAME = '{}'", needle)
    } else {
        let owner = schema.to_ascii_uppercase().replace('\'', "''");
        format!("SELECT TEXT FROM ALL_VIEWS WHERE OWNER = '{}' AND VIEW_NAME = '{}'", owner, needle)
    };
    let result = odb.query_rows(&sql).await?;
    let Some(text) = result.rows.first().and_then(|row| row.first().cloned().flatten()) else { return Ok(None) };
    let text = text.trim();
    if text.is_empty() { bail!("Oracle 视图定义为空，未生成伪造 DDL"); }
    Ok(Some(format!("CREATE OR REPLACE VIEW {} AS {}", crate::db::qualified(kind, schema, table), text.trim_end_matches(';'))))
}

/// ODBC（Oracle/达梦/Oscar/DB2/GBase 等）导出：先 describe 建 sink，再在单一 blocking 游标上逐行转换写盘；不 fetch_all。
async fn odbc_cursor(odb: &std::sync::Arc<crate::odbc_db::OdbcDb>, kind: &DbKind, options: &ExportOptions, task: &TaskControl, stage: &StagedOutput, sql: &str) -> Result<u64> {
    task.check()?;
    if options.format.eq_ignore_ascii_case("sql") && kind.id() == "oracle" {
        if let Some(definition) = oracle_view_definition(odb.as_ref(), kind, &options.schema, &options.table).await? {
            task.check()?;
            ExportSink::view(stage, kind, &options.schema, &definition, &options.encoding)?;
            return Ok(0);
        }
    }
    if options.format.eq_ignore_ascii_case("sql") && kind.id() != "oracle" {
        let object_type = odb.export_object_type(&options.schema, &options.table).await?;
        task.check()?;
        if object_type != "TABLE" { bail!("当前 ODBC 驱动尚不能读取该视图的真实 DDL；请使用 CSV/JSON 导出视图数据，未生成不可恢复的 INSERT"); }
    }
    // 空结果集只做 describe 取回列名与真实类型，据此建立有界 sink；不读取任何数据行。
    let describe_sql = format!("SELECT * FROM {} WHERE 1=0", crate::db::qualified(kind, &options.schema, &options.table));
    let meta = odb.query_rows(&describe_sql).await?;
    if meta.columns.is_empty() { bail!("ODBC 对象列目录不可见；未将其伪造为空表"); }
    let names = meta.columns.clone();
    let types: Vec<String> = meta.column_info.iter().map(|column| column.data_type.clone()).collect();
    if types.len() != names.len() { bail!("ODBC 列类型元数据与列数不一致"); }
    let binary: Vec<bool> = types.iter().map(|ty| is_binary_odbc_type(ty)).collect();
    let mut sink = ExportSink::create(stage, &options.format.to_ascii_lowercase(), kind, &options.schema, &options.table, &names, &types, &options.delimiter, options.has_header, &options.encoding)?;
    if options.limit == Some(0) { sink.finish()?; return Ok(0); }
    let max_record_bytes = crate::transfer_stream::TransferLimits::default().max_record_bytes;
    let names_owned = names.clone();
    let stream_sql = sql.to_owned();
    // 有界通道（64 行）解耦阻塞的 ODBC 游标与异步 sink，绝不无界缓存整表。
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<Option<String>>>(64);
    let handle = odb.spawn_row_stream(stream_sql, names_owned, binary, max_record_bytes, tx);
    let mut count = 0u64;
    let mut write_error: Option<anyhow::Error> = None;
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(100));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            maybe = rx.recv() => match maybe {
                Some(values) => match sink.write_rows(std::slice::from_ref(&values)) {
                    Ok(written) => {
                        count = match count.checked_add(written) { Some(total) => total, None => { write_error = Some(anyhow!("ODBC 导出行数溢出")); break; } };
                        if count % 100 == 0 { task.update("export", count, options.limit.unwrap_or(0), "正在流式读取 ODBC 游标并生成目标编码文件"); }
                        if options.limit.is_some_and(|max| count >= max) { break; }
                    }
                    Err(error) => { write_error = Some(error); break; }
                },
                None => break,
            },
            _ = ticker.tick() => {
                if let Err(error) = task.check() { write_error = Some(error); break; }
            }
        }
    }
    drop(rx);
    let produced = handle.await.map_err(|e| anyhow!("ODBC 任务失败: {e}"))?;
    if let Some(error) = write_error { return Err(error); }
    let produced = produced?;
    if !options.limit.is_some_and(|max| count >= max) && produced != count { bail!("ODBC 导出返回行数与写入行数不一致，目标未发布"); }
    task.check()?;
    sink.finish()?;
    Ok(count)
}

/// 只解析定义开头的关键字，不把列注释/字符串里的 VIEW 错判成视图。
fn clickhouse_is_view(definition: &str) -> Result<bool> {
    let mut words = definition.split_whitespace();
    if !words.next().is_some_and(|word| word.eq_ignore_ascii_case("CREATE")) { bail!("ClickHouse 返回的定义不是 CREATE 语句"); }
    let mut object = words.next().ok_or_else(|| anyhow!("ClickHouse 对象定义不完整"))?;
    if object.eq_ignore_ascii_case("OR") {
        if !words.next().is_some_and(|word| word.eq_ignore_ascii_case("REPLACE")) { bail!("ClickHouse 对象定义前缀不可识别"); }
        object = words.next().ok_or_else(|| anyhow!("ClickHouse 对象定义不完整"))?;
    }
    if ["MATERIALIZED", "LIVE", "WINDOW"].iter().any(|prefix| object.eq_ignore_ascii_case(prefix)) {
        object = words.next().ok_or_else(|| anyhow!("ClickHouse 视图定义不完整"))?;
        if !object.eq_ignore_ascii_case("VIEW") { bail!("ClickHouse 视图定义前缀不可识别"); }
        return Ok(true);
    }
    if object.eq_ignore_ascii_case("VIEW") { return Ok(true); }
    if object.eq_ignore_ascii_case("TABLE") { return Ok(false); }
    bail!("ClickHouse 对象定义类型不可识别，未退回 INSERT 导出")
}

/// ClickHouse 导出：SQL 格式对视图输出真实定义；否则 describe 建 sink，再流式逐行写盘。
async fn clickhouse_cursor(ch: &crate::clickhouse::ClickHouseClient, kind: &DbKind, options: &ExportOptions, task: &TaskControl, stage: &StagedOutput, sql: &str) -> Result<u64> {
    task.check()?;
    let qualified = crate::db::qualified(kind, &options.schema, &options.table);
    if options.format.eq_ignore_ascii_case("sql") {
        let ddl = ch.export_definition(&format!("SHOW CREATE TABLE {}", qualified)).await?;
        if clickhouse_is_view(&ddl)? {
            task.check()?;
            ExportSink::view(stage, kind, &options.schema, &ddl, &options.encoding)?;
            return Ok(0);
        }
    }
    let (names, types) = ch.describe(&format!("SELECT * FROM {} LIMIT 0", qualified)).await?;
    if names.is_empty() { bail!("ClickHouse 对象列目录不可见；未将其伪造为空表"); }
    if types.len() != names.len() { bail!("ClickHouse 列类型元数据与列数不一致"); }
    let mut sink = ExportSink::create(stage, &options.format.to_ascii_lowercase(), kind, &options.schema, &options.table, &names, &types, &options.delimiter, options.has_header, &options.encoding)?;
    if options.limit == Some(0) { sink.finish()?; return Ok(0); }
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<Option<String>>>(64);
    let max_record_bytes = crate::transfer_stream::TransferLimits::default().max_record_bytes;
    let handle = ch.spawn_row_stream(sql.to_owned(), names.clone(), types.clone(), max_record_bytes, tx);
    let mut count = 0u64;
    let mut write_error: Option<anyhow::Error> = None;
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(100));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            maybe = rx.recv() => match maybe {
                Some(values) => match sink.write_rows(std::slice::from_ref(&values)) {
                    Ok(written) => {
                        count = match count.checked_add(written) { Some(total) => total, None => { write_error = Some(anyhow!("ClickHouse 导出行数溢出")); break; } };
                        if count % 100 == 0 { task.update("export", count, options.limit.unwrap_or(0), "正在流式读取 ClickHouse 结果并生成目标编码文件"); }
                    }
                    Err(error) => { write_error = Some(error); break; }
                },
                None => break,
            },
            _ = ticker.tick() => {
                if let Err(error) = task.check() { write_error = Some(error); break; }
            }
        }
    }
    drop(rx);
    let produced = handle.await.map_err(|error| anyhow!("ClickHouse 任务失败: {error}"))?;
    if let Some(error) = write_error { return Err(error); }
    let produced = produced?;
    if produced != count { bail!("ClickHouse 导出返回行数与写入行数不一致，目标未发布"); }
    task.check()?;
    sink.finish()?;
    Ok(count)
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn sqlserver_export_uses_segmented_identity_top_and_zero_limit() {
        let options = ExportOptions { connection_id:"fixture".into(), schema:"catalog.with.dot".into(), table:"[sales].[order]]detail]".into(), file:String::new(), format:"csv".into(), delimiter:",".into(), encoding:"utf8".into(), has_header:true, where_clause:Some("id > 0".into()), limit:Some(0) };
        assert_eq!(select_sql(&DbKind::of("sqlserver"), &options).unwrap(), "SELECT TOP (0) * FROM [catalog.with.dot].[sales].[order]]detail] WHERE (id > 0)");
    }
    #[test] fn oracle_export_uses_receiver_limit() {
        let base = |limit: Option<u64>| ExportOptions { connection_id:"fixture".into(), schema:"app".into(), table:"orders".into(), file:String::new(), format:"csv".into(), delimiter:",".into(), encoding:"utf8".into(), has_header:true, where_clause:None, limit };
        // ODBC 驱动覆盖多个版本和方言，不假定 LIMIT/FETCH FIRST；行数上限由接收游标控制。
        assert_eq!(select_sql(&DbKind::of("oracle"), &base(Some(10))).unwrap(), "SELECT * FROM \"APP\".\"ORDERS\"");
        // 无上限时不得残留任何 LIMIT/FETCH 子句。
        assert_eq!(select_sql(&DbKind::of("oracle"), &base(None)).unwrap(), "SELECT * FROM \"APP\".\"ORDERS\"");
        let sql = select_sql(&DbKind::of("oracle"), &base(Some(10))).unwrap().to_ascii_uppercase();
        assert!(!sql.contains("LIMIT") && !sql.contains("FETCH"));
    }
    #[test] fn clickhouse_and_odbc_export_limits_use_dialect_syntax() {
        let base = |limit: Option<u64>| ExportOptions { connection_id:"fixture".into(), schema:"app".into(), table:"orders".into(), file:String::new(), format:"csv".into(), delimiter:",".into(), encoding:"utf8".into(), has_header:true, where_clause:None, limit };
        // ClickHouse 用反引号标识符 + LIMIT。
        assert_eq!(select_sql(&DbKind::of("clickhouse"), &base(Some(5))).unwrap(), "SELECT * FROM `app`.`orders` LIMIT 5");
        // DB2/Informix 等 ODBC 族不追加任何行数后缀。
        assert_eq!(select_sql(&DbKind::of("db2"), &base(Some(5))).unwrap(), "SELECT * FROM \"APP\".\"ORDERS\"");
        assert_eq!(select_sql(&DbKind::of("informix"), &base(Some(5))).unwrap(), "SELECT * FROM \"APP\".\"ORDERS\"");
        // Hive 属 ODBC 族，同样不追加 LIMIT/FETCH FIRST，行数由接收游标控制。
        assert_eq!(select_sql(&DbKind::of("hive"), &base(Some(5))).unwrap(), "SELECT * FROM `app`.`orders`");
    }
    #[test] fn clickhouse_is_view_does_not_misread_view_literal_in_create_table() {
        // CREATE TABLE 的列名、默认值或注释里出现 VIEW 字面量，不能被误判成视图。
        assert!(!clickhouse_is_view("CREATE TABLE `app`.`t` (`view` String, note String DEFAULT 'CREATE VIEW') ENGINE = MergeTree ORDER BY tuple()").unwrap());
        assert!(!clickhouse_is_view("CREATE TABLE app.t AS SELECT 'CREATE VIEW v AS SELECT 1' AS s").unwrap());
        assert!(!clickhouse_is_view("create table app.t (id UInt64) engine=Memory").unwrap());
    }
    #[test] fn clickhouse_is_view_accepts_materialized_live_window_and_or_replace() {
        for ddl in [
            "CREATE VIEW app.v AS SELECT 1",
            "CREATE OR REPLACE VIEW app.v AS SELECT 1",
            "CREATE MATERIALIZED VIEW app.mv ENGINE = MergeTree ORDER BY tuple() AS SELECT 1",
            "CREATE OR REPLACE MATERIALIZED VIEW app.mv ENGINE = MergeTree ORDER BY tuple() AS SELECT 1",
            "CREATE LIVE VIEW app.lv AS SELECT 1",
            "CREATE WINDOW VIEW app.wv AS SELECT 1",
        ] {
            assert!(clickhouse_is_view(ddl).unwrap(), "应识别为视图: {ddl}");
        }
    }
    #[test] fn clickhouse_is_view_rejects_unknown_definition() {
        for ddl in [
            "SELECT 1",
            "CREATE DICTIONARY app.d (id UInt64) PRIMARY KEY id SOURCE(CLICKHOUSE(TABLE 't')) LAYOUT(FLAT()) LIFETIME(0)",
            "CREATE OR REPLACE DICTIONARY app.d (id UInt64) PRIMARY KEY id SOURCE(CLICKHOUSE(TABLE 't')) LAYOUT(FLAT()) LIFETIME(0)",
            "CREATE FUNCTION f AS (x) -> x + 1",
        ] {
            assert!(clickhouse_is_view(ddl).is_err(), "未知定义必须拒绝: {ddl}");
        }
    }
    #[test] fn odbc_binary_type_detection() {
        assert!(is_binary_odbc_type("BLOB"));
        assert!(is_binary_odbc_type("VARBINARY"));
        assert!(is_binary_odbc_type("BINARY(16)"));
        assert!(!is_binary_odbc_type("VARCHAR"));
        assert!(!is_binary_odbc_type("NUMBER(10,2)"));
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
#[cfg(test)] #[path = "transfer_clickhouse_export_tests.rs"] mod clickhouse_protocol_tests;
