//! SQLite logical backup of catalog + every selected table from one read transaction.
//! No changes to a user's pool settings or persistent PRAGMAs. Other drivers remain separate.
use super::*;
use sqlx::{Connection, Executor};

async fn open(pool: &sqlx::SqlitePool) -> Result<sqlx::SqliteConnection> {
    let mut anchor = pool.acquire().await?;
    let file: String = crate::sql_history::query_scalar("SELECT file FROM pragma_database_list WHERE name='main'")
        .fetch_one(&mut *anchor).await?;
    if file.is_empty() { bail!("私有内存库不能重开成备份快照；使用现有兼容路径"); }
    let options = (*pool.connect_options()).clone().create_if_missing(false).read_only(true)
        .row_buffer_size(1).statement_cache_capacity(8);
    let mut connection = sqlx::SqliteConnection::connect_with(&options).await?;
    drop(anchor);
    {
        let mut handle = connection.lock_handle().await?;
        // Limit one native row before driver buffering/hex encoding. This is a
        // disposable connection, never an altered pool-wide sqlite setting.
        unsafe { libsqlite3_sys::sqlite3_limit(handle.as_raw_handle().as_ptr(), libsqlite3_sys::SQLITE_LIMIT_LENGTH, 8 * 1024 * 1024); }
    }
    connection.execute("PRAGMA query_only=ON; BEGIN").await?;
    Ok(connection)
}

async fn catalog(connection: &mut sqlx::SqliteConnection, tables: &[String]) -> Result<Vec<(String,String,String)>> {
    let mut result = Vec::new(); let mut bytes = 0usize;
    let mut stream = crate::sql_history::query("SELECT name,type,sql FROM main.sqlite_schema WHERE type IN ('table','view') AND name NOT GLOB 'sqlite_*' ORDER BY CASE type WHEN 'table' THEN 0 ELSE 1 END,name").fetch(&mut *connection);
    while let Some(row) = stream.try_next().await? {
        let name: String = row.try_get("name")?;
        if !tables.is_empty() && !tables.contains(&name) { continue; }
        let ty: String = row.try_get("type")?; let ddl: String = row.try_get("sql")?;
        bytes = bytes.checked_add(name.len()+ty.len()+ddl.len()).ok_or_else(||anyhow!("备份目录字节溢出"))?;
        if result.len() >= 4096 || bytes > 16 * 1024 * 1024 { bail!("SQLite备份目录超出4096对象/16MiB预算"); }
        result.push((name,ty,ddl));
    }
    drop(stream);
    for name in tables {
        if !result.iter().any(|(actual, _, _)| actual == name) {
            bail!("BACKUP_OBJECT_MISSING: 所选 SQLite 对象不存在或不可见：{name}；未发布备份");
        }
    }
    Ok(result)
}

async fn attached_catalog(connection: &mut sqlx::SqliteConnection, objects: &[(String,String,String)]) -> Result<Vec<(String,String,String,String)>> {
    // sqlite_schema.sql is NULL for implicit constraint indexes already present
    // in CREATE TABLE. Capture only explicit DDL belonging to selected objects.
    let mut count = objects.len();
    let mut bytes = objects.iter().try_fold(0usize, |sum,(name,ty,ddl)| sum.checked_add(name.len()+ty.len()+ddl.len()).ok_or_else(||anyhow!("备份目录字节溢出")))?;
    let mut result = Vec::new();
    let mut stream = crate::sql_history::query("SELECT tbl_name,name,type,sql FROM main.sqlite_schema WHERE type IN ('index','trigger') AND sql IS NOT NULL AND trim(sql)<>'' AND name NOT GLOB 'sqlite_*' ORDER BY CASE type WHEN 'index' THEN 0 ELSE 1 END,name").fetch(&mut *connection);
    while let Some(row) = stream.try_next().await? {
        let owner: String = row.try_get("tbl_name")?;
        if !objects.iter().any(|(name,_,_)| name == &owner) { continue; }
        let name: String = row.try_get("name")?; let ty: String = row.try_get("type")?; let ddl: String = row.try_get("sql")?;
        bytes = bytes.checked_add(owner.len()+name.len()+ty.len()+ddl.len()).ok_or_else(||anyhow!("备份目录字节溢出"))?;
        if count >= 4096 || bytes > 16 * 1024 * 1024 { bail!("SQLite备份目录（含索引/触发器）超出4096对象/16MiB预算"); }
        count += 1; result.push((owner,name,ty,ddl));
    }
    Ok(result)
}

async fn insertable_projection(connection: &mut sqlx::SqliteConnection, kind: &DbKind, table: &str) -> Result<String> {
    // Generated (hidden=2/3) and virtual-table hidden columns cannot accept
    // explicit INSERT values. Keep every ordinary column, including defaults:
    // generated values are reconstructed by the original catalog DDL.
    let sql = format!("PRAGMA main.table_xinfo({})", quote_ident(kind, table));
    let mut stream = crate::sql_history::query(&sql).fetch(&mut *connection);
    let mut names = Vec::new(); let mut bytes = 0usize;
    while let Some(row) = stream.try_next().await? {
        if row.try_get::<i64, _>("hidden")? != 0 { continue; }
        let name: String = row.try_get("name")?;
        bytes = bytes.checked_add(name.len()).ok_or_else(||anyhow!("备份列目录字节溢出"))?;
        if names.len() >= 1024 || bytes > 1024 * 1024 { bail!("SQLite备份列目录超出预算"); }
        names.push(quote_ident(kind, &name));
    }
    if names.is_empty() { bail!("SQLite备份对象没有可插入的普通列：{table}"); }
    Ok(names.join(", "))
}

async fn write_snapshot(connection: &mut sqlx::SqliteConnection, kind: &DbKind, schema: &str,
    selected: &[String], include_structure: bool, include_data: bool, stage: &StagedOutput,
    comment: &str, skip_incompatible: bool) -> Result<DumpDatabaseReport> {
    // This first catalog read establishes the snapshot used by DDL AND data below.
    let objects = catalog(connection, selected).await?;
    let attached = if include_structure { attached_catalog(connection, &objects).await? } else { Vec::new() };
    let mut skipped_tables = HashSet::new();
    let mut writer = stage.encoder("utf8")?;
    write!(writer, "-- ------------------------------------------------------------\r\n-- NavKit 备份文件\r\n--\r\n-- 源数据库 : {}\r\n-- 备份时间 : {}\r\n-- 一致性 : SQLite 同一只读事务（结构与所选数据）\r\n",
        schema.replace(['\r','\n'], " "), chrono::Local::now().format("%Y-%m-%d %H:%M:%S"))?;
    if !comment.trim().is_empty() { writeln!(writer,"-- 注释     : {}",comment.replace(['\r','\n']," "))?; }
    writer.write_all(b"-- ------------------------------------------------------------\r\n\r\nPRAGMA foreign_keys = OFF;\r\n\r\n")?;
    let mut rows = 0u64; let mut warnings = Vec::new();
    for (name, ty, ddl) in objects {
        writer.flush()?; let start = stage.file_clone()?.metadata()?.len();
        let is_view = ty == "view";
        if include_structure {
            write!(writer,"-- ---------------- {} {} ----------------\r\nDROP {} IF EXISTS {};\r\n{};\r\n\r\n",
                if is_view {"视图"} else {"表"}, name.replace(['\r','\n']," "), if is_view {"VIEW"}else{"TABLE"},
                format!("main.{}",quote_ident(kind,&name)),ddl.trim_end().trim_end_matches(';'))?;
        }
        if include_data && !is_view {
            writer.flush()?;
            let result: Result<u64> = async {
                let projection = insertable_projection(connection, kind, &name).await?;
                let sql = format!("SELECT {projection} FROM main.{}",quote_ident(kind,&name));
                let description = crate::sql_history::describe(&mut *connection,&sql).await?;
                let names = description.columns().iter().map(|column|column.name().to_owned()).collect::<Vec<_>>();
                let types = description.columns().iter().map(|column|column.type_info().name().to_owned()).collect::<Vec<_>>();
                let mut sink = ExportSink::create(stage,"sql",kind,schema,&name,&names,&types,",",true,"utf8")?;
                let mut stream = crate::sql_history::query(&sql).fetch(&mut *connection);
                let mut count = 0u64;
                while let Some(row) = stream.try_next().await? {
                    let mut values = crate::values::sqlite_row_values(&row,row.len());
                    let mut bytes = 0usize;
                    for index in 0..row.len() {
                        let raw = row.try_get_raw(index)?;
                        if raw.is_null() { values[index] = None; continue; }
                        if raw.type_info().name() == "BLOB" {
                            let binary: Vec<u8> = row.try_get(index)?;
                            let length = binary.len().checked_mul(2).and_then(|n| n.checked_add(2)).ok_or_else(||anyhow!("备份二进制长度溢出"))?;
                            if length > TransferLimits::default().max_record_bytes as usize { bail!("备份二进制记录超过上限"); }
                            let mut text = String::with_capacity(length); text.push_str("0x");
                            const HEX: &[u8] = b"0123456789ABCDEF";
                            for byte in binary { text.push(HEX[(byte>>4) as usize] as char); text.push(HEX[(byte&15) as usize] as char); }
                            values[index] = Some(text);
                        }
                        let value = values[index].as_deref().filter(|value| !crate::values::is_unsupported_value(value))
                            .ok_or_else(||anyhow::Error::new(UnsupportedExportValue{column:names[index].clone(),data_type:types[index].clone()}))?;
                        bytes = bytes.checked_add(value.len()).ok_or_else(||anyhow!("备份记录长度溢出"))?;
                        if bytes > TransferLimits::default().max_record_bytes as usize { bail!("备份单条记录超过上限"); }
                    }
                    count += sink.write_rows(std::slice::from_ref(&values))?;
                    if count % 128 == 0 { tokio::task::yield_now().await; }
                }
                drop(stream); sink.finish()?; Ok(count)
            }.await;
            match result {
                Ok(count) => rows = rows.checked_add(count).ok_or_else(||anyhow!("备份行数溢出"))?,
                Err(error) if skip_incompatible && error.downcast_ref::<UnsupportedExportValue>().is_some() => {
                    super::rollback_dump_table(stage,start)?;
                    let warning = format!("已跳过表 {name}：{error}；依赖对象需手动核对");
                    writeln!(writer,"-- 警告：{}",warning.replace(['\r','\n']," "))?;
                    skipped_tables.insert(name.clone());
                    warnings.push(warning); continue;
                }
                Err(error) => return Err(error),
            }
            writer.write_all(b"\r\n")?;
        }
    }
    // Install triggers only AFTER every table's data: the archive already includes
    // their historical effects, which must not run again during restoration.
    for (owner,name,ty,ddl) in attached {
        if skipped_tables.contains(&owner) { continue; }
        write!(writer,"-- ---------------- {} {} ----------------\r\n{};\r\n\r\n",
            if ty == "index" {"索引"} else {"触发器"}, name.replace(['\r','\n'], " "), ddl.trim_end().trim_end_matches(';'))?;
    }
    writer.write_all(b"PRAGMA foreign_keys = ON;\r\n")?;
    writer.finish()?.get_ref().sync_all()?;
    let bytes = stage.file_clone()?.metadata()?.len();
    Ok(DumpDatabaseReport{rows,bytes,warnings})
}

#[cfg(test)]
#[path = "transfer_backup_snapshot_tests.rs"]
mod tests;

pub(super) async fn run(pool: &sqlx::SqlitePool, kind:&DbKind, schema:&str, tables:&[String],
    include_structure:bool, include_data:bool, path:&str, comment:&str, skip:bool) -> Result<DumpDatabaseReport> {
    if !matches!(schema,""|"main") { bail!("SQLite一致性逻辑备份只支持main；未读取其他附加库"); }
    ensure_writable(path)?;
    let stage = StagedOutput::new(path)?;
    let mut connection = open(pool).await?;
    let result = write_snapshot(&mut connection,kind,schema,tables,include_structure,include_data,&stage,comment,skip).await;
    let rollback = connection.execute("ROLLBACK").await;
    let close = connection.close().await;
    match (result,rollback,close) {
        (Ok(report),Ok(_),Ok(())) => { stage.publish(||Ok(()))?; Ok(report) },
        (result,rollback,close) => Err(result.err().unwrap_or_else(||anyhow!("备份只读会话清理未确认"))
            .context(format!("未发布备份；rollback={} close={}",rollback.is_ok(),close.is_ok()))),
    }
}
