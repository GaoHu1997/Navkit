//! 导入 / 导出 / 数据传输 / SQL 转储。

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::{Column, Row, TypeInfo, ValueRef};

use crate::models::Protocol;
use crate::transfer_stream::{CsvStream, JsonStream, TransferLimits};
use futures::TryStreamExt;

#[path = "transfer_output.rs"] mod transfer_output;
#[path = "transfer_export_cursor.rs"] mod transfer_export_cursor;
#[path = "transfer_sql_reader.rs"] mod transfer_sql_reader;
use transfer_output::{FileEncoder, StagedOutput};
use transfer_sql_reader::PreparedScript;

use crate::db::{qualified, quote_ident, Db};
use crate::models::{DbKind, DumpOptions, ExportOptions, ImportOptions, TransferOptions};

/// 按编码解码文件内容（支持 utf-8 / gbk / latin1）。
fn read_text(path: &str, encoding: &str) -> Result<String> {
    let meta = std::fs::metadata(path).with_context(|| format!("无法打开文件: {}", path))?;
    if !meta.is_file() || meta.len() > 256 * 1024 * 1024 { bail!("导入文件必须是普通文件且不超过 256 MiB"); }
    let bytes = std::fs::read(path)?;
    let enc = text_encoding(encoding)?;
    if encoding.eq_ignore_ascii_case("latin1") || encoding.eq_ignore_ascii_case("iso-8859-1") {
        return Ok(bytes.iter().map(|b| char::from(*b)).collect());
    }
    let (text, _, errors) = enc.decode(&bytes);
    if errors { bail!("文件包含无效的 {} 字节，请选择正确编码", encoding); }
    Ok(text.trim_start_matches('\u{feff}').to_string())
}

/// 写文本文件（按编码）。
fn text_encoding(encoding: &str) -> Result<&'static encoding_rs::Encoding> {
    Ok(match encoding.to_ascii_lowercase().as_str() {
        "" | "utf-8" | "utf8" => encoding_rs::UTF_8,
        "gbk" | "gb2312" | "gb18030" => encoding_rs::GB18030,
        "big5" => encoding_rs::BIG5,
        "latin1" | "iso-8859-1" => encoding_rs::WINDOWS_1252,
        _ => bail!("不支持的编码: {}", encoding),
    })
}

fn write_text(path: &str, encoding: &str, content: &str) -> Result<()> {
    ensure_writable(path)?;
    let stage=StagedOutput::new(path)?;let mut writer=stage.encoder(encoding)?;
    writer.write_all(content.as_bytes())?;writer.finish()?.get_ref().sync_all()?;
    stage.publish(||Ok(()))
}

/// CSV 字段转义。
fn csv_escape(value: &str, delimiter: char) -> String {
    let needs = value.contains(delimiter)
        || value.contains('"')
        || value.contains('\n')
        || value.contains('\r');
    if needs {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// NULL 使用 \\N；文本的首个反斜线再转义一次，避免与字面值混淆。
fn csv_cell(value: Option<&str>, delimiter: char) -> String {
    match value {
        None => csv_escape("\\N", delimiter),
        Some(s) if s.starts_with('\\') => csv_escape(&format!("\\{s}"), delimiter),
        Some(s) => csv_escape(s, delimiter),
    }
}

fn csv_value(value: String) -> Option<String> {
    if value == "\\N" { None }
    else if value.starts_with("\\\\") { Some(value[1..].to_owned()) }
    else { Some(value) }
}

/// 导出表数据为 CSV。
pub fn export_csv(
    path: &str,
    names: &[String],
    rows: &[Vec<Option<String>>],
    delimiter: &str,
    encoding: &str,
    has_header: bool,
) -> Result<u64> {
    validate_names(names)?;
    if rows.iter().any(|r| r.len() != names.len()) { bail!("导出行的列数与表头不一致"); }
    let delim = validate_delimiter(delimiter)?;
    let mut out = String::new();
    if has_header {
        let cells: Vec<String> = names.iter().map(|n| csv_escape(n, delim)).collect();
        out.push_str(&cells.join(&delim.to_string()));
        out.push_str("\r\n");
    }
    let mut count = 0u64;
    for row in rows {
        let cells: Vec<String> = row
            .iter()
            .map(|v| csv_cell(v.as_deref(), delim))
            .collect();
        out.push_str(&cells.join(&delim.to_string()));
        out.push_str("\r\n");
        count += 1;
    }
    write_text(path, encoding, &out)?;
    Ok(count)
}

/// 导出表数据为 JSON。
pub fn export_json(
    path: &str,
    names: &[String],
    rows: &[Vec<Option<String>>],
    encoding: &str,
) -> Result<u64> {
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let mut obj = serde_json::Map::new();
        for (i, name) in names.iter().enumerate() {
            let v = row.get(i).cloned().flatten();
            obj.insert(name.clone(), serde_json::Value::from(v));
        }
        items.push(serde_json::Value::Object(obj));
    }
    let text = serde_json::to_string_pretty(&items)?;
    write_text(path, encoding, &text)?;
    Ok(rows.len() as u64)
}

/// 导出表数据为 SQL INSERT 语句。
pub fn export_sql(
    path: &str,
    kind: DbKind,
    schema: &str,
    table: &str,
    names: &[String],
    rows: &[Vec<Option<String>>],
    encoding: &str,
) -> Result<u64> {
    export_sql_typed(path, &kind, schema, table, names, rows, &[], encoding)
}
pub fn export_sql_typed(path: &str, kind: &DbKind, schema: &str, table: &str, names: &[String], rows: &[Vec<Option<String>>], types: &[String], encoding: &str) -> Result<u64> {
    validate_names(names)?;
    if rows.iter().any(|r| r.len() != names.len()) { bail!("导出行列数不一致"); }
    let full = qualified(kind, schema, table);
    let cols = names.iter().map(|n| quote_ident(kind, n)).collect::<Vec<_>>().join(", ");
    let mut out = String::new();
    for row in rows {
        let vals = row.iter().enumerate().map(|(i, v)| {
            let ty = types.get(i).map(String::as_str).unwrap_or("TEXT");
            crate::sync::value_sql(v.as_deref(), kind, ty, ty)
        }).collect::<Result<Vec<_>>>()?;
        out.push_str(&format!("INSERT INTO {} ({}) VALUES ({});\r\n", full, cols, vals.join(", ")));
    }
    write_text(path, encoding, &out)?;
    Ok(rows.len() as u64)
}

/// 解析 CSV 内容为表头与行。
fn parse_csv(content: &str, delimiter: char, has_header: bool) -> Result<(Vec<String>, Vec<Vec<String>>)> {
    let mut rdr = csv::ReaderBuilder::new().delimiter(delimiter as u8)
        .has_headers(false).flexible(false).from_reader(content.trim_start_matches('\u{feff}').as_bytes());
    let mut names = Vec::new();
    let mut rows = Vec::new();
    for (i, rec) in rdr.records().enumerate() {
        let vals = rec.with_context(|| format!("CSV 第 {} 条记录无效", i + 1))?.iter().map(str::to_string).collect::<Vec<_>>();
        if i == 0 && has_header { names = vals; } else { rows.push(vals); }
    }
    if !has_header { names = (0..rows.first().map_or(0, Vec::len)).map(|i| format!("column{}", i + 1)).collect(); }
    validate_names(&names)?;
    Ok((names, rows))
}

/// CSV / JSON / SQL imports share validation, mapping and a single target transaction.
pub async fn import_csv(db: &Db, kind: DbKind, opts: &ImportOptions) -> Result<u64> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    run_import_data(db, &kind, opts, &[], None).await

    }).await
}
pub async fn import_json(db: &Db, kind: DbKind, opts: &ImportOptions) -> Result<u64> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    run_import_data(db, &kind, opts, &[], None).await

    }).await
}
pub async fn import_sql(db: &Db, kind: DbKind, opts: &ImportOptions) -> Result<u64> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    run_import_data(db, &kind, opts, &[], None).await

    }).await
}

/// 数据传输：把源表的结构与数据复制到目标。
pub async fn transfer(
    source: &Db,
    source_kind: &DbKind,
    target: &Db,
    target_kind: &DbKind,
    opts: &TransferOptions,
) -> Result<u64> {
    let mode = match (opts.copy_structure, opts.copy_data) {
        (true, true) => "both", (true, false) => "structure", (false, true) => "data",
        _ => bail!("至少选择同步结构或同步数据"),
    };
    let sync_opts = crate::sync::SyncOptions {
        task_id: None, source_connection: opts.source_connection.clone(),
        source_schema: opts.source_schema.clone(), source_table: opts.source_table.clone(),
        target_connection: opts.target_connection.clone(), target_schema: opts.target_schema.clone(),
        target_table: opts.target_table.clone(), mode: mode.into(),
        write_mode: if opts.drop_existing { "recreate" } else { "append" }.into(),
        sync_mode: "full".into(), incremental_update: false,
        create_target: opts.create_target, columns: vec![], key_columns: Vec::new(), delete_missing: false,
        where_clause: opts.where_clause.clone(),
    };
    Ok(crate::sync::synchronize(source, source_kind, target, target_kind, &sync_opts).await?.rows)
}

/// 生成 SQL 转储文件。
pub async fn dump(
    db: &Db,
    kind: &DbKind,
    opts: &DumpOptions,
) -> Result<u64> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if opts.structure_only && opts.data_only { bail!("仅结构与仅数据不能同时选择"); }
    protect_database_path(db, &opts.file).await?;
    if let Some(definition)=transfer_export_cursor::view_definition(db,kind,&opts.schema,&opts.table).await? {
        if opts.data_only {bail!("视图 SQL 转储只支持真实定义，不向视图生成 INSERT；导出视图数据请使用 CSV 或 JSON");}
        ensure_writable(&opts.file)?;let stage=StagedOutput::new(&opts.file)?;
        let definition=if opts.include_drop {
            let full=if kind.protocol()==Protocol::Mssql {let identity=crate::object_identity::SqlServerObject::parse(&opts.schema,&opts.table)?;format!("{}.{}",quote_ident(kind,&identity.schema),quote_ident(kind,&identity.name))}else{qualified(kind,&opts.schema,&opts.table)};
            format!("DROP VIEW IF EXISTS {full};\r\n{}{}",if kind.protocol()==Protocol::Mssql{"GO\r\n"}else{""},definition)
        }else{definition};
        ExportSink::view(&stage,kind,&opts.schema,&definition,"utf8")?;stage.publish(||Ok(()))?;return Ok(0);
    }
    if opts.data_only {
        ensure_writable(&opts.file)?;
        let stage = StagedOutput::new(&opts.file)?;
        let count = transfer_export_cursor::append_dump_data(db, kind, &opts.schema, &opts.table, &stage).await?;
        stage.publish(|| Ok(()))?;
        return Ok(count);
    }
    let schema = crate::schema::table_schema(db, kind.clone(), &opts.schema, &opts.table).await?;
    let full = qualified(&kind, &opts.schema, &opts.table);
    let mut out = String::new();

    if opts.include_drop && !opts.data_only {
        out.push_str(&format!("DROP TABLE IF EXISTS {};\r\n", full));
    }

    if !opts.data_only {
        let mut cols_sql = Vec::new();
        for c in &schema.columns {
            let mut seg = format!("  {} {}", quote_ident(&kind, &c.name), c.data_type);
            if !c.nullable {
                seg.push_str(" NOT NULL");
            }
            if let Some(d) = &c.default {
                seg.push_str(&format!(" DEFAULT {}", d));
            }
            // AUTO_INCREMENT / COMMENT 是 MySQL 方言，PG/SQLite 输出即非法。
            if kind.is_mysql_family() {
                if c.auto_increment {
                    seg.push_str(" AUTO_INCREMENT");
                }
                if let Some(cm) = &c.comment {
                    seg.push_str(&format!(" COMMENT {}", crate::db::quote_literal(cm)));
                }
            }
            cols_sql.push(seg);
        }
        let pks: Vec<String> = schema
            .columns
            .iter()
            .filter(|c| c.primary_key)
            .map(|c| quote_ident(&kind, &c.name))
            .collect();
        // 主键必须写在列定义括号内。
        if !pks.is_empty() {
            cols_sql.push(format!("  PRIMARY KEY ({})", pks.join(", ")));
        }
        out.push_str(&format!(
            "CREATE TABLE {} (\r\n{}\r\n)",
            full,
            cols_sql.join(",\r\n")
        ));
        out.push_str(";\r\n\r\n");
        for idx in &schema.indexes {
            if idx.primary {
                continue;
            }
            out.push_str(&format!(
                "CREATE {} INDEX {} ON {} ({});\r\n",
                if idx.unique { "UNIQUE" } else { "" },
                quote_ident(&kind, &idx.name),
                full,
                idx.columns
                    .iter()
                    .map(|c| quote_ident(&kind, c))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for fk in &schema.foreign_keys {
            out.push_str(&format!(
                "-- FOREIGN KEY {} ({}) REFERENCES {} ({})\r\n",
                fk.name,
                fk.columns.join(", "),
                fk.ref_table,
                fk.ref_columns.join(", ")
            ));
        }
        out.push_str("\r\n");
    }

    ensure_writable(&opts.file)?;
    let stage = StagedOutput::new(&opts.file)?;
    let mut writer = stage.encoder("utf8")?;
    writer.write_all(out.as_bytes())?;
    writer.finish()?;
    let count = if opts.structure_only { 0 } else {
        transfer_export_cursor::append_dump_data(db, kind, &opts.schema, &opts.table, &stage).await?
    };
    stage.publish(|| Ok(()))?;
    Ok(count)

    }).await
}

/// 整库备份：把 schema 下全部（或指定）表 / 视图转储为单个 SQL 文件。
///
/// 与单表 dump() 不同，备份文件按流式写入（BufWriter），
/// 结构部分：MySQL 用 SHOW CREATE TABLE 的真实 DDL，
/// SQLite 用 sqlite_master 里的原始 SQL，PostgreSQL 综合字段信息生成；
/// 数据部分复用固定会话单 SELECT 游标，逐行有界写入同目录 stage。
/// 返回 (数据行数, 文件字节数)。
pub async fn dump_database(
    db: &Db,
    kind: &DbKind,
    schema: &str,
    tables: &[String],
    routines: &[String],
    include_structure: bool,
    include_data: bool,
    path: &str,
    comment: &str,
) -> Result<(u64, u64)> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    ensure_writable(path)?;
    let all = crate::schema::list_tables(db, kind.clone(), schema).await?;
    let wanted: Vec<&(String, crate::models::ObjectType)> = if tables.is_empty() {
        // 表在前、视图在后，保证还原顺序
        let mut t: Vec<&(String, crate::models::ObjectType)> =
            all.iter().filter(|(_, ty)| *ty == crate::models::ObjectType::Table).collect();
        let v: Vec<&(String, crate::models::ObjectType)> =
            all.iter().filter(|(_, ty)| *ty != crate::models::ObjectType::Table).collect();
        t.extend(v);
        t
    } else {
        all.iter()
            .filter(|(n, _)| tables.iter().any(|x| x == n))
            .collect()
    };

    let stage=StagedOutput::new(path)?;
    let mut w=stage.encoder("utf8")?;
    let mut bytes = 0u64;
    let mut rows_total = 0u64;

    let mut head = String::new();
    head.push_str("-- ------------------------------------------------------------\r\n");
    head.push_str("-- NavKit 备份文件\r\n--\r\n");
    head.push_str(&format!("-- 源数据库 : {}\r\n", schema));
    head.push_str(&format!(
        "-- 备份时间 : {}\r\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    ));
    if !comment.trim().is_empty() {
        head.push_str(&format!("-- 注释     : {}\r\n", comment.replace('\n', " ")));
    }
    head.push_str("-- ------------------------------------------------------------\r\n\r\n");
    match kind.protocol() {
        crate::models::Protocol::Mysql => {
            head.push_str("SET NAMES utf8mb4;\r\nSET FOREIGN_KEY_CHECKS = 0;\r\n\r\n")
        }
        crate::models::Protocol::Mssql
        | crate::models::Protocol::Redis
        | crate::models::Protocol::Mongo
        | crate::models::Protocol::Http
        | crate::models::Protocol::Native => {}
        crate::models::Protocol::Postgres => {
            head.push_str("SET session_replication_role = replica;\r\n\r\n")
        }
        crate::models::Protocol::Sqlite => head.push_str("PRAGMA foreign_keys = OFF;\r\n\r\n"),
        // ClickHouse / 达梦 / Hive 无外键守卫语法
        crate::models::Protocol::Clickhouse | crate::models::Protocol::Odbc => {}
    }
    w.write_all(head.as_bytes())?;
    bytes += head.len() as u64;

    for (name, ty) in &wanted {
        let full = qualified(&kind, schema, name);
        let is_view = *ty != crate::models::ObjectType::Table;

        if include_structure {
            let ddl = table_ddl(db, &kind, schema, name, is_view).await?;
            if let Some(sql) = ddl {
                let mut chunk = String::new();
                chunk.push_str(&format!(
                    "-- ---------------- {} {} ----------------\r\n",
                    if is_view { "视图" } else { "表" },
                    name
                ));
                if is_view {chunk.push_str("-- 真实视图定义：还原前请先具备依赖对象；不向视图生成 INSERT。\r\n");}
                if is_view && kind.protocol()==Protocol::Mssql {
                    let identity=crate::object_identity::SqlServerObject::parse(schema,name)?;
                    chunk.push_str(&format!("USE {};\r\nGO\r\nDROP VIEW IF EXISTS {}.{};\r\nGO\r\n",quote_ident(kind,&identity.catalog),quote_ident(kind,&identity.schema),quote_ident(kind,&identity.name)));
                }else{chunk.push_str(&format!("DROP {} IF EXISTS {};\r\n",if is_view{"VIEW"}else{"TABLE"},full));}
                chunk.push_str(&sql);
                if !sql.trim_end().ends_with(';') {
                    chunk.push(';');
                }
                chunk.push_str("\r\n\r\n");
                if is_view && kind.protocol()==Protocol::Mssql {chunk.push_str("GO\r\n");}
                w.write_all(chunk.as_bytes())?;
                bytes += chunk.len() as u64;
            }
        }

        if include_data && !is_view {
            // try_clone 与原 writer 共享文件位置；先排空结构缓冲，再由游标接续写入。
            w.flush()?;
            let before = stage.file_clone()?.metadata()?.len();
            let rows = transfer_export_cursor::append_dump_data(db, kind, schema, name, &stage).await?;
            let added = stage.file_clone()?.metadata()?.len().checked_sub(before).ok_or_else(|| anyhow!("转储 stage 长度发生倒退"))?;
            bytes = bytes.checked_add(added).ok_or_else(|| anyhow!("转储文件长度溢出"))?;
            rows_total = rows_total.checked_add(rows).ok_or_else(|| anyhow!("转储行数溢出"))?;
            w.write_all(b"\r\n")?;
            bytes += 2;
        }
    }

    // 例程身份先查真实目录；重复的名字只导出一次，同名函数/过程各自导出。
    let mut exported_routines = HashSet::new();
    for name in routines {
        if name.is_empty() { bail!("ROUTINE_NAME_INVALID: 备份选择包含空例程名"); }
        if !exported_routines.insert(name) { continue; }
        for (keyword, ddl) in routine_ddls(db, kind, schema, name).await? {
            let chunk = routine_dump_chunk(kind, schema, name, &keyword, &ddl)?;
            w.write_all(chunk.as_bytes())?;
            bytes += chunk.len() as u64;
        }
    }

    let mut tail = String::new();
    match kind.protocol() {
        crate::models::Protocol::Mysql => tail.push_str("SET FOREIGN_KEY_CHECKS = 1;\r\n"),
        crate::models::Protocol::Postgres => {
            tail.push_str("SET session_replication_role = DEFAULT;\r\n")
        }
        crate::models::Protocol::Sqlite => tail.push_str("PRAGMA foreign_keys = ON;\r\n"),
        crate::models::Protocol::Clickhouse | crate::models::Protocol::Odbc => {}
        crate::models::Protocol::Mssql
        | crate::models::Protocol::Redis
        | crate::models::Protocol::Mongo
        | crate::models::Protocol::Http
        | crate::models::Protocol::Native => {}
    }
    w.write_all(tail.as_bytes())?;
    bytes += tail.len() as u64;
    w.finish()?.get_ref().sync_all()?;
    stage.publish(||Ok(()))?;

    Ok((rows_total, bytes))

    }).await
}

/// 取例程真实类型与定义；不存在/权限错误绝不当成成功的空备份。
async fn routine_ddls(db: &Db, kind: &DbKind, schema: &str, name: &str) -> Result<Vec<(String, String)>> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    match db {
        Db::Mysql(pool) => crate::mysql_routines::definitions(pool, kind, schema, name).await,
        Db::Postgres(pool) => {
            let rows = crate::sql_history::query("SELECT pg_get_functiondef(p.oid) AS ddl, CASE WHEN p.prokind = 'p' THEN 'PROCEDURE' ELSE 'FUNCTION' END AS kind FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace WHERE n.nspname = $1 AND p.proname = $2 AND p.prokind IN ('f','p') ORDER BY p.oid")
                .bind(schema).bind(name).fetch_all(pool).await?;
            if rows.is_empty() { bail!("ROUTINE_NOT_FOUND: 例程 {schema}.{name} 不存在或不可见，备份未完成"); }
            if rows.len() != 1 { bail!("ROUTINE_IDENTITY_REQUIRED: 同名 PostgreSQL 重载不能通过仅名字的备份契约完整还原，未导出随机对象"); }
            let row = &rows[0];
            let ddl: String = row.try_get("ddl")?;
            if ddl.trim().is_empty() { bail!("ROUTINE_DEFINITION_UNREADABLE: 例程定义不可读取，备份未完成"); }
            Ok(vec![(row.try_get("kind")?, ddl)])
        }
        _ => bail!("ROUTINE_ADAPTER_UNSUPPORTED: 当前协议没有可验证的例程备份读取器"),
    }

    }).await
}

fn routine_dump_chunk(kind: &DbKind, schema: &str, name: &str, keyword: &str, ddl: &str) -> Result<String> {
    let routine = crate::mysql_routines::RoutineKind::parse(keyword)?;
    if ddl.trim().is_empty() { bail!("ROUTINE_DEFINITION_UNREADABLE: 不允许生成空例程备份"); }
    let mut chunk = format!("-- ---------------- 例程 {} ----------------\r\nDROP {} IF EXISTS {};\r\n", name.replace(['\r','\n'], " "), routine.keyword(), qualified(kind, schema, name));
    if kind.is_mysql_family() {
        // BEGIN/END 的内部分号不是分句边界；只用不出现在正文中的标点 delimiter，兼容客户端与还原词法器。
        let mut n = 0u64;
        let delimiter = loop {
            let alphabet = b"!$%&?@^"; let mut ordinal = n; let mut suffix = String::new();
            loop { suffix.push(alphabet[(ordinal % alphabet.len() as u64) as usize] as char); ordinal /= alphabet.len() as u64; if ordinal == 0 { break; } }
            let value = format!("$${suffix}$$");
            if value.len() > 16 { bail!("ROUTINE_DELIMITER_UNAVAILABLE: 无法生成有界、无冲突的例程分隔符"); }
            if !ddl.contains(&value) { break value; } n += 1;
        };
        let statement = ddl.trim_end().strip_suffix(';').unwrap_or(ddl.trim_end());
        chunk.push_str(&format!("DELIMITER {delimiter}\r\n{statement}{delimiter}\r\nDELIMITER ;\r\n\r\n"));
    } else {
        chunk.push_str(ddl);
        if !ddl.trim_end().ends_with(';') { chunk.push(';'); }
        chunk.push_str("\r\n\r\n");
    }
    Ok(chunk)
}

#[cfg(test)]
mod routine_dump_tests {
    use super::*;
    use super::transfer_sql_reader::{ScriptDialect, SqlStatementReader};
    #[test]
    fn mysql_function_and_compound_procedure_are_complete_restore_statements() {
        for (word, ddl) in [("FUNCTION", "CREATE FUNCTION fn_add10(x INT) RETURNS INT DETERMINISTIC RETURN x + 10"), ("PROCEDURE", "CREATE PROCEDURE p() BEGIN SELECT 'a;b'; SELECT 2; END;")] {
            let text = routine_dump_chunk(&DbKind::of("mysql"), "test", "fixture", word, ddl).unwrap();
            let mut parser = SqlStatementReader::new(std::io::Cursor::new(text), ScriptDialect::Mysql, 1024 * 1024);
            let mut statements = Vec::new();
            while let Some(statement) = parser.next_statement().unwrap() { statements.push(crate::db::strip_leading_comments(&statement.sql).trim().trim_end_matches(';').trim().to_string()); }
            assert_eq!(statements.len(), 2);
            assert!(statements[0].starts_with(&format!("DROP {word} IF EXISTS")));
            assert_eq!(statements[1], ddl.trim_end_matches(';'));
        }
    }
    #[tokio::test]
    async fn routine_restore_parser_keeps_compound_ddl_and_rejects_invalid_input_before_send() {
        let pool = sqlx::mysql::MySqlPoolOptions::new().connect_lazy("mysql://localhost/regression").unwrap();
        let database = Db::Mysql(pool);
        let text = routine_dump_chunk(&DbKind::of("mysql"), "regression", "p", "PROCEDURE", "CREATE PROCEDURE p() BEGIN SELECT 'a;b'; SELECT 2; END").unwrap();
        let statements = script_statements(&database, &text).unwrap();
        assert_eq!(statements.len(), 2);
        assert!(statements[1].contains("SELECT 'a;b'; SELECT 2; END"));
        assert!(!statements.iter().any(|sql| sql.starts_with("DELIMITER")));
        assert!(script_statements(&database, "DELIMITER INVALID
CREATE PROCEDURE p() BEGIN ENDINVALID").is_err());
        assert_eq!(script_statements(&database, "SET NAMES utf8mb4; SET FOREIGN_KEY_CHECKS = 0; SET FOREIGN_KEY_CHECKS = 1;").unwrap().len(), 3);
        for sql in ["SET autocommit=1", "USE other", "SET FOREIGN_KEY_CHECKS = 0; COMMIT", "SET NAMES utf8mb4 COLLATE evil"] { assert!(script_statements(&database, sql).is_err()); }
        if let Db::Mysql(pool) = database { pool.close().await; }
    }
    #[test]
    fn routine_delimiter_cannot_split_a_literal_or_emit_invalid_kind() {
        let text = routine_dump_chunk(&DbKind::of("mariadb"), "test", "p", "PROCEDURE", "CREATE PROCEDURE p() BEGIN SELECT '$$!$$'; END").unwrap();
        assert!(text.contains("DELIMITER $$$$$"));
        assert!(routine_dump_chunk(&DbKind::of("mysql"), "test", "x", "TRIGGER", "SELECT 1").is_err());
        assert!(routine_dump_chunk(&DbKind::of("mysql"), "test", "x", "FUNCTION", " ").is_err());
    }
}

/// 取单个表 / 视图的 CREATE 语句（已含结尾分号前的全部内容）。
async fn table_ddl(
    db: &Db,
    kind: &DbKind,
    schema: &str,
    name: &str,
    is_view: bool,
) -> Result<Option<String>> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    match db {
        Db::Mssql(_) => {
            if is_view {return transfer_export_cursor::view_definition(db,kind,schema,name).await;}
            let needle = name.replace("'", "''");
            let (_, rows) = db
                .generic_rows(&format!(
                    "SELECT OBJECT_DEFINITION(OBJECT_ID('{}'))",
                    needle
                ))
                .await?;
            return Ok(rows
                .first()
                .and_then(|r| r.first().cloned().flatten())
                .filter(|x| !x.trim().is_empty()));
        }
        Db::Sqlite(pool) => {
            let rows = crate::sql_history::query(
                "SELECT sql FROM sqlite_master WHERE name = ? AND sql IS NOT NULL",
            )
            .bind(name)
            .fetch_all(pool)
            .await?;
            Ok(rows
                .first()
                .and_then(|r| crate::values::sqlite_row_values(r, 1).first().cloned().flatten()))
        }
        Db::Mysql(pool) => {
            let full = qualified(&kind, schema, name);
            let sql = if is_view {
                format!("SHOW CREATE VIEW {}", full)
            } else {
                format!("SHOW CREATE TABLE {}", full)
            };
            let rows = crate::sql_history::query(&sql).fetch_all(pool).await?;
            let Some(r) = rows.first() else {
                return Ok(None);
            };
            let cols = r.columns();
            let vals = crate::values::my_row_values(r, cols.len());
            // SHOW CREATE TABLE: 第 2 列 "Create Table"；SHOW CREATE VIEW: 第 2 列 "Create View"
            let mut ddl = vals.get(1).cloned().flatten().unwrap_or_default();
            if ddl.is_empty() {
                return Ok(None);
            }
            if is_view {
                // 去掉 DEFINER 子句，避免还原时因用户不存在而失败
                ddl = strip_definer(&ddl);
            }
            Ok(Some(ddl))
        }
        Db::ClickHouse(client) => {
            let full = qualified(&kind, schema, name);
            // SHOW CREATE TABLE 对表与视图均适用
            let res = client.query_rows(&format!("SHOW CREATE TABLE {}", full)).await?;
            Ok(res
                .rows
                .first()
                .and_then(|r| r.first().cloned().flatten())
                .filter(|s| !s.is_empty()))
        }
        Db::Odbc(o) => {
            let ddl: Option<String> = match o.kind.id() {
            "hive" | "inceptor" | "argodb" => {
                let full = qualified(&kind, schema, name);
                let res = o.query_rows(&format!("SHOW CREATE TABLE {}", full)).await?;
                // SHOW CREATE TABLE 每行一段文本
                let text = res
                    .rows
                    .iter()
                    .filter_map(|r| r.first().cloned().flatten())
                    .collect::<Vec<_>>()
                    .join("\r\n");
                if text.is_empty() { None } else { Some(text) }
            }
            "dameng" => {
                let upper = name.to_uppercase();
                if is_view {
                    let res = o
                        .query_rows(&format!(
                            "SELECT TEXT FROM ALL_VIEWS WHERE VIEW_NAME = '{}'",
                            upper.replace('\'', "''")
                        ))
                        .await?;
                    let text = res
                        .rows
                        .first()
                        .and_then(|r| r.first().cloned().flatten())
                        .unwrap_or_default();
                    if text.is_empty() {
                        None
                    } else {
                        Some(format!("CREATE VIEW {} AS {}", quote_ident(&kind, name), text))
                    }
                } else {
                    let res = o
                        .query_rows(&format!(
                            "SELECT DBMS_METADATA.GET_DDL('TABLE', '{}') AS DDL FROM DUAL",
                            upper.replace('\'', "''")
                        ))
                        .await?;
                    res.rows
                        .first()
                        .and_then(|r| r.first().cloned().flatten())
                        .filter(|s| !s.trim().is_empty())
                }
            }
            _ => None,
            };
            Ok(ddl)
        }
        Db::Postgres(pool) => {
            if is_view {
                // pg_get_viewdef 返回裸 SELECT，需要包上 CREATE VIEW；
                // regclass 字面量里用双引号标识符，兼容带 schema / 大小写的名字
                let full = qualified(&kind, schema, name);
                let sql = format!(
                    "SELECT pg_get_viewdef('{}'::regclass, true) AS def",
                    full.replace('\'', "''")
                );
                let rows = crate::sql_history::query(&sql).fetch_all(pool).await?;
                let def = rows.first().and_then(|r| {
                    crate::values::pg_row_values(r, 1).first().cloned().flatten()
                });
                return Ok(def.map(|d| {
                    format!("CREATE OR REPLACE VIEW {} AS {}", full, d.trim_end_matches(';').trim())
                }));
            }
            let s = crate::schema::table_schema(db, kind.clone(), schema, name).await?;
            let mut cols_sql = Vec::new();
            for c in &s.columns {
                let mut seg = format!("{} {}", quote_ident(&kind, &c.name), c.data_type);
                if !c.nullable {
                    seg.push_str(" NOT NULL");
                }
                if let Some(d) = &c.default {
                    seg.push_str(&format!(" DEFAULT {}", d));
                }
                cols_sql.push(seg);
            }
            if cols_sql.is_empty() {
                return Ok(None);
            }
            let pks: Vec<String> = s
                .columns
                .iter()
                .filter(|c| c.primary_key)
                .map(|c| quote_ident(&kind, &c.name))
                .collect();
            // 主键必须写在列定义括号内。
            if !pks.is_empty() {
                cols_sql.push(format!("PRIMARY KEY ({})", pks.join(", ")));
            }
            let ddl = format!(
                "CREATE TABLE {} (\r\n  {}\r\n)",
                qualified(&kind, schema, name),
                cols_sql.join(",\r\n  ")
            );
            Ok(Some(ddl))
        }
        Db::Redis(_) | Db::Mongo(_) | Db::Http(_) => Ok(None),
    }

    }).await
}

/// 去掉 MySQL 视图 DDL 中的 DEFINER 子句。
pub fn strip_definer(sql: &str) -> String {
    if let Some(start) = sql.find("DEFINER=") {
        let rest = &sql[start..];
        let end = rest
            .find(|c| c == ' ' || c == '\n')
            .map(|i| start + i)
            .unwrap_or(sql.len());
        return format!("{}{}", &sql[..start], &sql[end..]);
    }
    sql.to_string()
}

/// 是否为外键守卫语句（还原失败可跳过）。
pub fn is_fk_guard_stmt(sql: &str) -> bool {
    let up = sql.trim_start().to_ascii_uppercase();
    up.starts_with("SET FOREIGN_KEY_CHECKS")
        || up.starts_with("SET SESSION_REPLICATION_ROLE")
        || up.starts_with("PRAGMA FOREIGN_KEYS")
}

/// 单条还原语句的执行结果（供「还原备份」进度对话框展示）。
#[derive(Debug, Clone, Serialize)]
pub struct RestoreStep {
    /// 语句序号（从 1 开始）。
    pub index: usize,
    /// 语句原文（截断到 500 字符，避免超长 INSERT 撑爆界面）。
    pub sql: String,
    /// 是否成功（外键守卫语句失败亦记为跳过成功）。
    pub ok: bool,
    /// 影响行数（驱动未返回时为 None）。
    pub affected: Option<u64>,
    /// 失败原因 / 跳过说明。
    pub error: Option<String>,
    /// 是否为被跳过的守卫语句。
    pub skipped: bool,
}

/// 还原结果汇总。
#[derive(Debug, Clone, Serialize)]
pub struct RestoreReport {
    pub total: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub steps: Vec<RestoreStep>,
}

/// 单批事务的语句条数上限与字节预算：达到任一上限即提交，避免一条长事务压住全部 DML。
const SCRIPT_BATCH_STATEMENTS: usize = 1000;
const SCRIPT_BATCH_BYTES: usize = 8 * 1024 * 1024;

/// 只有连续 DML 进入批事务；DDL/SET/事务控制语句保持逐条自动提交，
/// 这样「每条 DDL 立即生效」的既有可见语义不变，而大文件里成片的 INSERT 不再逐条刷盘。
fn batchable_dml(sql: &str) -> bool {
    let head: String = sql.trim_start().chars().take(8).collect::<String>().to_ascii_uppercase();
    ["INSERT ", "INSERT(", "UPDATE ", "DELETE ", "REPLACE "].iter().any(|prefix| head.starts_with(prefix))
}

fn step_sql_short(statement: &str) -> String {
    let mut short: String = statement.chars().take(500).collect();
    if statement.chars().nth(500).is_some() { short.push_str(" …"); }
    short.trim().to_string()
}

/// 在固定物理会话上逐条执行整段脚本：连续 DML 攒成批事务（每 1000 条 / 8 MiB 提交），
/// 其它语句保持自动提交。这样既消除每条 INSERT 一次的隐式事务刷盘，又保留「DDL 立即生效」
/// 的既有语义（MySQL DDL 会隐式结束事务，所以遇到非 DML 先提交，下一段 DML 再显式开启）。
/// 批内失败回滚该批并写明回滚条数，已提交的更早批次保持原样。
macro_rules! script_tx_body {
    ($conn:ident, $begin:literal, $stmts:expr, $on_error:expr, $steps:ident) => {{
        use sqlx::Executor;
        let mut in_tx = false;
        let mut batch_rows = 0usize;
        let mut batch_bytes = 0usize;
        let mut stopped = false;
        for (i, statement) in $stmts.iter().enumerate() {
            if stopped {
                $steps.push(RestoreStep { index: i + 1, sql: String::new(), ok: false, affected: None, error: Some("前序语句失败，已终止".into()), skipped: true });
                continue;
            }
            let short = step_sql_short(statement);
            let dml = batchable_dml(statement);
            if !dml && in_tx {
                if let Err(error) = sqlx::Executor::execute(&mut $conn, sqlx::raw_sql("COMMIT")).await {
                    return Err(anyhow!(error).context("脚本分批提交失败；提交结果未知，请核对目标数据后再重试"));
                }
                in_tx = false; batch_rows = 0; batch_bytes = 0;
            }
            if dml && (!in_tx || batch_rows >= SCRIPT_BATCH_STATEMENTS || batch_bytes.saturating_add(statement.len()) > SCRIPT_BATCH_BYTES) {
                if in_tx {
                    if let Err(error) = sqlx::Executor::execute(&mut $conn, sqlx::raw_sql("COMMIT")).await {
                        return Err(anyhow!(error).context("脚本分批提交失败；提交结果未知，请核对目标数据后再重试"));
                    }
                }
                if let Err(error) = sqlx::Executor::execute(&mut $conn, sqlx::raw_sql($begin)).await {
                    return Err(anyhow!(error).context("无法为脚本 DML 开启批事务"));
                }
                in_tx = true; batch_rows = 0; batch_bytes = 0;
            }
            match sqlx::Executor::execute(&mut $conn, sqlx::raw_sql(statement.as_str())).await {
                Ok(receipt) => {
                    if in_tx && dml { batch_rows += 1; batch_bytes = batch_bytes.saturating_add(statement.len()); }
                    $steps.push(RestoreStep { index: i + 1, sql: short, ok: true, affected: Some(receipt.rows_affected()), error: None, skipped: false });
                }
                Err(error) => {
                    if is_fk_guard_stmt(statement) {
                        $steps.push(RestoreStep { index: i + 1, sql: short, ok: true, affected: None, error: Some(format!("守卫语句已跳过：{error}")), skipped: true });
                        continue;
                    }
                    let rolled_back = if in_tx {
                        let rows = batch_rows + 1;
                        in_tx = false;
                        sqlx::Executor::execute(&mut $conn, sqlx::raw_sql("ROLLBACK")).await.ok().map(|_| rows)
                    } else { None };
                    batch_rows = 0; batch_bytes = 0;
                    let message = match rolled_back {
                        Some(rows) => format!("{error}（本批 {rows} 条 DML 已确认回滚；更早批次早已提交）"),
                        None => format!("{error}"),
                    };
                    $steps.push(RestoreStep { index: i + 1, sql: short, ok: false, affected: None, error: Some(message), skipped: false });
                    if $on_error { stopped = true; }
                }
            }
        }
        if in_tx {
            sqlx::Executor::execute(&mut $conn, sqlx::raw_sql("COMMIT")).await
                .map_err(|error| anyhow!(error).context("脚本末尾提交失败；提交结果未知，请核对目标数据后再重试"))?;
        }
    }};
}

/// 取一条专用物理连接执行脚本，结束后关闭；会话级守卫因此固定在同一个会话。
macro_rules! script_with_connection {
    ($pool:expr, $conn_ty:ty, $begin:literal, $stmts:expr, $on_error:expr, $steps:ident) => {{
        use sqlx::Connection;
        let options = (*$pool.connect_options()).clone();
        let mut conn = <$conn_ty as Connection>::connect_with(&options).await?;
        script_tx_body!(conn, $begin, $stmts, $on_error, $steps);
        conn.close().await?;
    }};
}

/// 其余通道（SQL Server / ClickHouse / ODBC）：逐条自动提交并如实说明不会回滚。
macro_rules! script_loop_serial {
    ($db:expr, $stmts:expr, $on_error:expr, $steps:ident) => {{
        let mut stopped = false;
        for (i, statement) in $stmts.iter().enumerate() {
            if stopped {
                $steps.push(RestoreStep { index: i + 1, sql: String::new(), ok: false, affected: None, error: Some("前序语句失败，已终止".into()), skipped: true });
                continue;
            }
            let short = step_sql_short(statement);
            match execute_one($db, statement).await {
                Ok(affected) => $steps.push(RestoreStep { index: i + 1, sql: short, ok: true, affected, error: None, skipped: false }),
                Err(err) => {
                    if is_fk_guard_stmt(statement) {
                        $steps.push(RestoreStep { index: i + 1, sql: short, ok: true, affected: None, error: Some(format!("守卫语句已跳过：{err}")), skipped: true });
                        continue;
                    }
                    $steps.push(RestoreStep { index: i + 1, sql: short, ok: false, affected: None, error: Some(err.to_string()), skipped: false });
                    if $on_error { stopped = true; }
                }
            }
        }
    }};
}


/// 逐条执行 SQL 文件并收集每条语句结果（还原备份的详细版）。
///
/// 与 `restore_sql_file` 使用同一套外键守卫语义，区别是：
///  · 不中途返回 Err，而是把失败记进 steps 并停止后续执行（保持数据一致性）
///  · 返回每条语句的执行结果，供前端展示执行过程
pub async fn restore_sql_file_detailed(
    db: &Db,
    _kind: &DbKind,
    path: &str,
) -> Result<RestoreReport> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("无法读取备份文件: {}", path))?;
    let stmts = script_statements(db, &content)?;

    let mut steps: Vec<RestoreStep> = Vec::with_capacity(stmts.len());
    match db {
        Db::Sqlite(pool) => script_with_connection!(pool, sqlx::SqliteConnection, "BEGIN", &stmts, true, steps),
        Db::Mysql(pool) => script_with_connection!(pool, sqlx::MySqlConnection, "START TRANSACTION", &stmts, true, steps),
        Db::Postgres(pool) => script_with_connection!(pool, sqlx::PgConnection, "BEGIN", &stmts, true, steps),
        _ => script_loop_serial!(db, &stmts, true, steps),
    }
    let succeeded = steps.iter().filter(|step| step.ok && !step.skipped).count();
    let failed = steps.iter().filter(|step| !step.ok && !step.skipped).count();

    Ok(RestoreReport { total: stmts.len(), succeeded, failed, steps })

    }).await
}

/// 运行 SQL 文件（任意路径，数据库右键「运行 SQL 文件」入口）。
///
/// 逐条执行并收集结果；`stop_on_error` 控制遇错终止（默认，保持数据一致性）
/// 还是跳过错误继续执行后续语句。守卫语句（外键开关）失败时总是跳过。
pub async fn run_sql_file_detailed(
    db: &Db,
    path: &str,
    stop_on_error: bool,
) -> Result<RestoreReport> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("无法读取 SQL 文件: {}", path))?;
    let stmts = script_statements(db, &content)?;

    let mut steps: Vec<RestoreStep> = Vec::with_capacity(stmts.len());
    match db {
        Db::Sqlite(pool) => script_with_connection!(pool, sqlx::SqliteConnection, "BEGIN", &stmts, stop_on_error, steps),
        Db::Mysql(pool) => script_with_connection!(pool, sqlx::MySqlConnection, "START TRANSACTION", &stmts, stop_on_error, steps),
        Db::Postgres(pool) => script_with_connection!(pool, sqlx::PgConnection, "BEGIN", &stmts, stop_on_error, steps),
        _ => script_loop_serial!(db, &stmts, stop_on_error, steps),
    }
    let succeeded = steps.iter().filter(|step| step.ok && !step.skipped).count();
    let failed = steps.iter().filter(|step| !step.ok && !step.skipped).count();

    Ok(RestoreReport { total: stmts.len(), succeeded, failed, steps })

    }).await
}

/// MySQL 客户端 DELIMITER 只控制本地分句，绝不发送给服务器。预读完整脚本，失败不发送 SQL。
fn script_statements(db: &Db, content: &str) -> Result<Vec<String>> {
    if matches!(db, Db::Mysql(_)) {
        use transfer_sql_reader::{ScriptDialect, SqlStatementReader};
        let mut reader = SqlStatementReader::for_backup_restore(std::io::Cursor::new(content), ScriptDialect::Mysql, 50 * 1024 * 1024);
        let mut statements = Vec::new();
        while let Some(statement) = reader.next_statement()? { statements.push(crate::db::strip_leading_comments(&statement.sql).trim().trim_end_matches(';').trim().to_string()); }
        Ok(statements)
    } else {
        Ok(crate::db::split_statements(content).into_iter().map(|s| crate::db::strip_leading_comments(&s)).filter(|s| !s.is_empty()).collect())
    }
}

/// 单条语句执行（按数据源选择驱动），返回影响行数。
async fn execute_one(db: &Db, sql: &str) -> Result<Option<u64>> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {    match db {
        Db::Sqlite(pool) => {
            let mut conn = pool.acquire().await?;
            let r = crate::sql_history::query(sql).execute(&mut *conn).await?;
            Ok(Some(r.rows_affected()))
        }
        Db::Mysql(pool) => {
            // 没有绑定参数的完整脚本使用 COM_QUERY，例程 DDL 不支持 prepared protocol。
            let r = crate::sql_history::raw_sql(sql).execute(pool).await?;
            Ok(Some(r.rows_affected()))
        }
        Db::Postgres(pool) => {
            let mut conn = pool.acquire().await?;
            let r = crate::sql_history::query(sql).execute(&mut *conn).await?;
            Ok(Some(r.rows_affected()))
        }
        Db::Mssql(_) | Db::ClickHouse(_) | Db::Odbc(_) => Ok(Some(db.generic_exec(sql).await?)),
        Db::Redis(_) | Db::Mongo(_) | Db::Http(_) => {
            Err(anyhow!("该数据源类型不支持 SQL 脚本还原"))
        }
    }

    }).await
}

/// 执行 SQL 文件中的全部语句（还原备份的共用实现），返回执行的语句数。
///
/// 全部语句在**同一连接**上执行：外键守卫（SET FOREIGN_KEY_CHECKS /
/// PRAGMA foreign_keys / SET session_replication_role）是会话级设置，
/// 走连接池会让"关外键"与"还原数据"落在不同会话，守卫形同虚设。
/// 守卫语句失败时跳过（如 PG 普通用户不能设置 session_replication_role）。
pub async fn restore_sql_file(db: &Db, _kind: &DbKind, path: &str) -> Result<u64> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("无法读取备份文件: {}", path))?;
    // 剥离纯注释块并过滤空语句
    let stmts = script_statements(db, &content)?;

    let mut count = 0u64;
    match db {
        Db::Sqlite(pool) => {
            let mut conn = pool.acquire().await?;
            let _ = crate::sql_history::query("PRAGMA foreign_keys = OFF").execute(&mut *conn).await;
            for t in &stmts {
                match crate::sql_history::query(t).execute(&mut *conn).await {
                    Ok(_) => count += 1,
                    Err(e) => {
                        if is_fk_guard_stmt(t) {
                            continue;
                        }
                        let _ = crate::sql_history::query("PRAGMA foreign_keys = ON").execute(&mut *conn).await;
                        return Err(anyhow::Error::from(e).context(format!("还原在第 {} 条语句失败", count + 1)));
                    }
                }
            }
            let _ = crate::sql_history::query("PRAGMA foreign_keys = ON").execute(&mut *conn).await;
        }
        Db::Mysql(pool) => {
            let mut conn = pool.acquire().await?;
            let _ = crate::sql_history::query("SET FOREIGN_KEY_CHECKS = 0").execute(&mut *conn).await;
            for t in &stmts {
                match {let mut history=crate::sql_history::Guard::new(t,Vec::new());let result=sqlx::Executor::execute(&mut *conn,t.as_str()).await;history.result(&result,|r|r.rows_affected() as i64);result} {
                    Ok(_) => count += 1,
                    Err(e) => {
                        if is_fk_guard_stmt(t) {
                            continue;
                        }
                        let _ = crate::sql_history::query("SET FOREIGN_KEY_CHECKS = 1").execute(&mut *conn).await;
                        return Err(anyhow::Error::from(e).context(format!("还原在第 {} 条语句失败", count + 1)));
                    }
                }
            }
            let _ = crate::sql_history::query("SET FOREIGN_KEY_CHECKS = 1").execute(&mut *conn).await;
        }
        Db::Mssql(_) => {
            // 无会话级外键守卫语法，逐条执行；守卫语句失败跳过
            for t in &stmts {
                match db.generic_exec(t).await {
                    Ok(_) => count += 1,
                    Err(e) => {
                        if is_fk_guard_stmt(t) {
                            continue;
                        }
                        return Err(anyhow::Error::from(e).context(format!("还原在第 {} 条语句失败", count + 1)));
                    }
                }
            }
        }
        Db::Redis(_) | Db::Mongo(_) | Db::Http(_) => {
            return Err(anyhow!("该数据源类型不支持 SQL 脚本还原"));
        }
        Db::ClickHouse(_) | Db::Odbc(_) => {
            // 无会话级外键守卫语法，逐条执行；守卫语句失败跳过
            for t in &stmts {
                match db.generic_exec(t).await {
                    Ok(_) => count += 1,
                    Err(e) => {
                        if is_fk_guard_stmt(t) {
                            continue;
                        }
                        return Err(e.context(format!("还原在第 {} 条语句失败", count + 1)));
                    }
                }
            }
        }
        Db::Postgres(pool) => {
            let mut conn = pool.acquire().await?;
            for t in &stmts {
                match crate::sql_history::query(t).execute(&mut *conn).await {
                    Ok(_) => count += 1,
                    Err(e) => {
                        if is_fk_guard_stmt(t) {
                            continue;
                        }
                        return Err(anyhow::Error::from(e).context(format!("还原在第 {} 条语句失败", count + 1)));
                    }
                }
            }
        }
    }
    Ok(count)

    }).await
}

/// 导出整个查询结果为 CSV（查询页导出）。
pub fn export_query_csv(
    path: &str,
    names: &[String],
    rows: &[Vec<Option<String>>],
    delimiter: &str,
    encoding: &str,
) -> Result<u64> {
    export_csv(path, names, rows, delimiter, encoding, true)
}

/// 读取 CSV 文件的表头（供导入向导预览）。
pub fn preview_csv(path: &str, delimiter: &str, encoding: &str, limit: usize) -> Result<(Vec<String>, Vec<Vec<String>>)> {
    let content = read_text(path, encoding)?;
    let (names, mut rows) = parse_csv(&content, validate_delimiter(delimiter)?, true)?;
    rows.truncate(limit.min(1000));
    Ok((names, rows))
}

/// 检查路径可写。
pub fn ensure_writable(path: &str) -> Result<()> {
    let p = Path::new(path);
    if path.trim().is_empty() || path.contains('\0') || !p.is_absolute() || path.ends_with('/') || path.ends_with('\\')
        || p.file_name().is_none() || p.components().any(|part| matches!(part, std::path::Component::ParentDir)) {
        bail!("导出文件必须使用包含目录和完整文件名的绝对路径");
    }
    if let Some(parent) = p.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }
    Ok(())
}

/// 简单的文本校验，用于导入向导的错误提示。
pub fn validate_delimiter(delimiter: &str) -> Result<char> {
    let mut chars = delimiter.chars();
    let c = chars
        .next()
        .ok_or_else(|| anyhow!("分隔符不能为空"))?;
    if chars.next().is_some() || !c.is_ascii() || matches!(c, '\n' | '\r' | '\0' | '"') {
        return Err(anyhow!("分隔符必须是一个非换行、非引号的 ASCII 字符"));
    }
    Ok(c)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportColumnMapping { pub source: String, pub target: String }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportTaskOptions {
    #[serde(flatten)] pub options: ImportOptions,
    pub task_id: String,
    #[serde(default)] pub column_mapping: Vec<ImportColumnMapping>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportTaskOptions { #[serde(flatten)] pub options: ExportOptions, pub task_id: String }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskProgress {
    pub task_id: String, pub status: String, pub phase: String,
    pub processed: u64, pub total: u64, pub message: String,
}
struct TaskEntry { cancel: Arc<AtomicBool>, progress: TaskProgress }
fn tasks() -> &'static parking_lot::Mutex<HashMap<String, TaskEntry>> {
    static TASKS: OnceLock<parking_lot::Mutex<HashMap<String, TaskEntry>>> = OnceLock::new();
    TASKS.get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
}
#[derive(Clone)]
pub struct TaskControl { id: String, cancel: Arc<AtomicBool> }
impl TaskControl {
    pub fn begin(id: &str) -> Result<Self> {
        if id.is_empty() || id.len() > 128 || !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')) { bail!("任务标识无效"); }
        let mut all = tasks().lock();
        if all.contains_key(id) { bail!("任务标识已使用，请创建新的任务"); }
        if all.len() >= 128 { all.retain(|_, t| t.progress.status == "running"); }
        if all.len() >= 128 { bail!("同时执行的任务过多"); }
        let cancel = Arc::new(AtomicBool::new(false));
        all.insert(id.into(), TaskEntry { cancel: cancel.clone(), progress: TaskProgress {
            task_id: id.into(), status: "running".into(), phase: "validate".into(), processed: 0, total: 0, message: "正在校验参数".into(),
        }});
        Ok(Self { id: id.into(), cancel })
    }
    // 取消本身不承诺回滚：是否回滚由执行通道（事务/逐条提交）说明。
    pub fn check(&self) -> Result<()> { if self.cancel.load(Ordering::Relaxed) { bail!("任务已取消"); } Ok(()) }
    pub fn update(&self, phase: &str, processed: u64, total: u64, message: &str) {
        if let Some(t) = tasks().lock().get_mut(&self.id) {
            t.progress.phase = phase.into(); t.progress.processed = processed; t.progress.total = total; t.progress.message = message.into();
        }
    }
    pub fn finish<T>(&self, result: &Result<T>) {
        if let Some(t) = tasks().lock().get_mut(&self.id) {
            let unknown = result.as_ref().err().is_some_and(|error| format!("{error:#}").contains("TRANSFER_EFFECTS_UNKNOWN"));
            t.progress.status = if result.is_ok() { "completed" } else if unknown { "unknown" } else if self.cancel.load(Ordering::Relaxed) { "cancelled" } else { "failed" }.into();
            t.progress.message = result.as_ref().err().map(|e| format!("{:#}", e)).unwrap_or_else(|| "执行完成".into());
        }
    }
}
pub fn task_progress(id: &str) -> Result<TaskProgress> {
    tasks().lock().get(id).map(|t| t.progress.clone()).ok_or_else(|| anyhow!("任务尚未启动或已过期"))
}
pub fn cancel_task(id: &str) -> Result<()> {
    let mut all = tasks().lock();
    let entry = all.get_mut(id).ok_or_else(|| anyhow!("任务不存在"))?;
    if entry.progress.status == "running" { entry.cancel.store(true, Ordering::Relaxed); entry.progress.message = "正在取消，等待当前数据库操作返回".into(); }
    Ok(())
}
pub fn validate_names(names: &[String]) -> Result<()> {
    let mut set = HashSet::new();
    for n in names {
        if n.trim().is_empty() || n.contains('\0') || n.len() > 256 { bail!("列名/表名不能为空、包含空字符或超过 256 字节"); }
        if !set.insert(n.to_ascii_lowercase()) { bail!("重复列名: {}", n); }
    }
    Ok(())
}
pub fn validate_where(clause: Option<&str>) -> Result<()> {
    if let Some(c) = clause {
        if c.len() > 16_384 || c.contains(';') || c.contains('\0') || c.contains("--") || c.contains("/*") || c.contains("*/") { bail!("筛选条件只能是单个 WHERE 表达式，不能包含分号或注释"); }
    }
    Ok(())
}
pub fn validate_export(opts: &ExportOptions) -> Result<()> {
    validate_names(&[opts.table.clone()])?; validate_where(opts.where_clause.as_deref())?;
    text_encoding(&opts.encoding)?;
    if !matches!(opts.format.to_ascii_lowercase().as_str(), "csv" | "json" | "sql") { bail!("不支持的导出格式"); }
    if opts.format.eq_ignore_ascii_case("csv") { validate_delimiter(&opts.delimiter)?; }
    ensure_writable(&opts.file)
}

/// Dedicated transaction: BEGIN/INSERT must not go through different pool sessions.
/// MySQL DDL commits implicitly; cancellation/errors still roll back pending DML.
pub async fn execute_atomic(db: &Db, statements: &[(String, bool)], task: Option<&TaskControl>) -> Result<u64> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let total = statements.iter().filter(|(_, row)| *row).count() as u64;
    macro_rules! run {
        ($pool:expr) => {{
            let mut tx = $pool.begin().await?;
            let mut count = 0u64;
            for (i, (sql, is_row)) in statements.iter().enumerate() {
                if let Some(t) = task { if let Err(e) = t.check() { tx.rollback().await?; return Err(e.context("任务已取消，未提交的目标修改已回滚")); } }
                match crate::sql_history::query(sql).execute(&mut *tx).await {
                    Ok(r) => if *is_row { count += r.rows_affected(); },
                    Err(e) => { let _ = tx.rollback().await; return Err(anyhow!(e).context(format!("第 {} 条目标语句执行失败，未提交数据已回滚", i + 1))); }
                }
                if let Some(t) = task { t.update("write", count, total, "正在写入目标（提交前可取消）"); }
                if i % 100 == 0 { tokio::task::yield_now().await; }
            }
            if let Some(t) = task { if let Err(e) = t.check() { tx.rollback().await?; return Err(e.context("任务已取消，未提交的目标修改已回滚")); } }
            tx.commit().await?; Ok(count)
        }};
    }
    match db { Db::Sqlite(p) => run!(p), Db::Mysql(p) => run!(p), Db::Postgres(p) => run!(p),
        // 通用通道（SQL Server / ClickHouse / ODBC）：无跨语句事务，逐条自动提交；
        // 失败时如实说明已执行部分不会回滚（UI 侧有对应警告）。
        Db::Mssql(_) | Db::ClickHouse(_) | Db::Odbc(_) => {
            let mut count = 0u64;
            for (i, (sql, is_row)) in statements.iter().enumerate() {
                if let Some(t) = task { t.check().map_err(|e| e.context("任务已取消；该目标通道不支持事务，已执行的语句不会自动回滚"))?; }
                match db.generic_exec(sql).await {
                    Ok(n) => if *is_row { count += n.max(1); },
                    Err(e) => return Err(anyhow!(e).context(format!("第 {} 条目标语句执行失败；该目标通道不支持事务，失败前已执行的语句不会自动回滚", i + 1))),
                }
                if let Some(t) = task {
                    t.update("write", count, total, "正在写入目标（非事务通道，逐条提交）");
                    // 最后一条语句执行期间也可能收到取消，不能将其覆盖成 completed。
                    t.check().map_err(|e| e.context("任务已取消；该目标通道不支持事务，已执行的语句不会自动回滚"))?;
                }
                if i % 100 == 0 { tokio::task::yield_now().await; }
            }
            Ok(count)
        }
        _ => bail!("当前导入/同步只支持 SQLite、MySQL/MariaDB、PostgreSQL、SQL Server、ClickHouse 与 ODBC 关系型数据库") }

    }).await
}
pub async fn run_import_task(db: &Db, kind: &DbKind, opts: &ImportTaskOptions) -> Result<u64> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let task = TaskControl::begin(&opts.task_id)?;
    let result = run_import_data(db, kind, &opts.options, &opts.column_mapping, Some(&task)).await;
    task.finish(&result); result

    }).await
}
/// 生成单行导入 INSERT 语句。`strategy` 为 error/skip/update；
/// 非事务方言（SQL Server/ClickHouse/ODBC）不支持冲突策略，由调用方提前拒绝。
fn import_insert_statement(
    full: &str,
    quoted_cols: &str,
    values: &str,
    kind: &DbKind,
    strategy: &str,
    conflict_cols: &[String],
    all_cols: &[String],
) -> Result<String> {
    let qconflict = conflict_cols.iter().map(|c| quote_ident(kind, c)).collect::<Vec<_>>().join(", ");
    match strategy {
        "error" => Ok(format!("INSERT INTO {} ({}) VALUES ({})", full, quoted_cols, values)),
        "skip" => match kind.protocol() {
            Protocol::Mysql => Ok(format!("INSERT IGNORE INTO {} ({}) VALUES ({})", full, quoted_cols, values)),
            Protocol::Sqlite | Protocol::Postgres => Ok(format!(
                "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) DO NOTHING",
                full, quoted_cols, values, qconflict
            )),
            _ => bail!("当前目标通道尚未接入导入冲突处理"),
        },
        "update" => {
            let non_key: Vec<&String> = all_cols.iter().filter(|c| !conflict_cols.iter().any(|k| k == *c)).collect();
            if non_key.is_empty() { bail!("更新冲突需要至少一个非键列用于更新"); }
            match kind.protocol() {
                Protocol::Mysql => {
                    let assigns = non_key
                        .iter()
                        .map(|c| { let q = quote_ident(kind, c); format!("{} = VALUES({})", q, q) })
                        .collect::<Vec<_>>()
                        .join(", ");
                    Ok(format!("INSERT INTO {} ({}) VALUES ({}) ON DUPLICATE KEY UPDATE {}", full, quoted_cols, values, assigns))
                }
                Protocol::Sqlite | Protocol::Postgres => {
                    let assigns = non_key
                        .iter()
                        .map(|c| { let q = quote_ident(kind, c); format!("{} = excluded.{}", q, q) })
                        .collect::<Vec<_>>()
                        .join(", ");
                    Ok(format!(
                        "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) DO UPDATE SET {}",
                        full, quoted_cols, values, qconflict, assigns
                    ))
                }
                _ => bail!("当前目标通道尚未接入导入冲突处理"),
            }
        }
        other => bail!("导入冲突策略无效: {other}"),
    }
}

/// 流式导入的数据来源：CSV 或 JSON 数组。
enum RowFeeder {
    Csv(CsvStream),
    Json(JsonStream),
}

impl RowFeeder {
    fn names(&self) -> &[String] {
        match self {
            RowFeeder::Csv(s) => &s.names,
            RowFeeder::Json(s) => &s.names,
        }
    }

    fn next_batch(&mut self, limits: &TransferLimits) -> Result<Option<Vec<Vec<Option<String>>>>> {
        match self {
            RowFeeder::Csv(s) => Ok(s.next_batch(limits)?.map(|b| {
                b.rows
                    .into_iter()
                    .map(|r| {
                        r.into_iter()
                            .map(csv_value)
                            .collect()
                    })
                    .collect()
            })),
            RowFeeder::Json(s) => s.next_batch(limits),
        }
    }
}

struct ImportMapper {
    pairs: Vec<(usize, String)>,
    kind: DbKind,
    full: String,
    quoted: String,
    strategy: String,
    conflict_cols: Vec<String>,
    cols: Vec<String>,
    types: Vec<String>,
}

impl ImportMapper {
    fn statements(&self, rows: &[Vec<Option<String>>]) -> Result<Vec<(String, bool)>> {
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let vals = self
                .pairs
                .iter()
                .zip(self.types.iter())
                .map(|((i, _), ty)| crate::sync::value_sql(row.get(*i).and_then(Option::as_deref), &self.kind, ty, ty))
                .collect::<Result<Vec<_>>>()?;
            out.push((import_insert_statement(&self.full, &self.quoted, &vals.join(", "), &self.kind, &self.strategy, &self.conflict_cols, &self.cols)?, true));
        }
        Ok(out)
    }
}

/// 分批执行导入语句：事务型通道整批一个事务，非事务通道逐条自动提交。
/// 每批之间检查取消信号并更新进度，保证大文件不会先全部读入再假装分批。
async fn execute_import_batches(db: &Db, prefix: &[(String, bool)], feeder: &mut RowFeeder, mapper: &ImportMapper, limits: &TransferLimits, task: Option<&TaskControl>) -> Result<u64> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    macro_rules! run_tx {
        ($pool:expr) => {{
            let mut count = 0u64;
            let mut tx = $pool.begin().await?;
            for (sql, is_row) in prefix {
                match crate::sql_history::query(sql).execute(&mut *tx).await {
                    Ok(r) => if *is_row { count += r.rows_affected(); },
                    Err(e) => { let _ = tx.rollback().await; return Err(anyhow!(e).context("导入前置语句执行失败，未提交数据已回滚")); }
                }
            }
            let mut batches = 0u64;
            loop {
                if let Some(t) = task { t.check().map_err(|e| e.context("任务已取消，未提交的目标修改已回滚"))?; }
                let Some(rows) = feeder.next_batch(limits)? else { break };
                let stmts = mapper.statements(&rows)?;
                for (sql, is_row) in &stmts {
                    match crate::sql_history::query(sql).execute(&mut *tx).await {
                        Ok(r) => if *is_row { count += r.rows_affected(); },
                        Err(e) => { let _ = tx.rollback().await; return Err(anyhow!(e).context(format!("第 {} 批导入语句执行失败，未提交数据已回滚", batches + 1))); }
                    }
                }
                batches += 1;
                if let Some(t) = task { t.update("write", count, 0, "正在分批写入目标（提交前可取消）"); }
                tokio::task::yield_now().await;
            }
            tx.commit().await?;
            Ok(count)
        }};
    }
    match db {
        Db::Sqlite(p) => run_tx!(p),
        Db::Mysql(p) => run_tx!(p),
        Db::Postgres(p) => run_tx!(p),
        Db::Mssql(_) | Db::ClickHouse(_) | Db::Odbc(_) => {
            let mut count = 0u64;
            for (sql, is_row) in prefix {
                if let Some(t) = task { t.check().map_err(|e| e.context("任务已取消；该目标通道不支持事务，已执行的语句不会自动回滚"))?; }
                let n = db.generic_exec(sql).await.map_err(|e| anyhow!(e).context("导入前置语句执行失败；该目标通道不支持事务，失败前已执行的语句不会自动回滚"))?;
                if *is_row { count += n.max(1); }
            }
            let mut batches = 0u64;
            loop {
                if let Some(t) = task { t.check().map_err(|e| e.context("任务已取消；该目标通道不支持事务，已执行的语句不会自动回滚"))?; }
                let Some(rows) = feeder.next_batch(limits)? else { break };
                for (sql, is_row) in mapper.statements(&rows)? {
                    let n = db.generic_exec(&sql).await.map_err(|e| anyhow!(e).context(format!("第 {} 批导入失败；该目标通道不支持事务，已执行的语句不会自动回滚", batches + 1)))?;
                    if is_row { count += n.max(1); }
                }
                batches += 1;
                if let Some(t) = task { t.update("write", count, 0, "正在分批写入目标（非事务通道，逐条提交）"); }
                tokio::task::yield_now().await;
            }
            Ok(count)
        }
        _ => bail!("当前导入/同步只支持 SQLite、MySQL/MariaDB、PostgreSQL、SQL Server、ClickHouse 与 ODBC 关系型数据库"),
    }

    }).await
}

async fn execute_prepared_script(db: &Db, script: &mut PreparedScript, task: Option<&TaskControl>) -> Result<u64> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    macro_rules! run {
        ($pool:expr, $implicit:expr) => {{
            let mut tx = $pool.begin().await?;
            let result: Result<u64> = async {
                let mut count = 0u64;
                let mut statements = 0u64;
                while let Some(sql) = script.next_statement()? {
                    if let Some(t) = task { t.check()?; }
                    // 当前数据库 future 一直 await 到真实结束；取消只停止下一条，不丢弃已发送操作。
                    let receipt = crate::sql_history::query(&sql).execute(&mut *tx).await?;
                    if script.counts_rows() { count = count.checked_add(receipt.rows_affected()).ok_or_else(|| anyhow!("导入影响行数溢出"))?; }
                    statements += 1;
                    if let Some(t) = task { t.update("write", statements, script.count(), if $implicit { "逐语句写入（MySQL DDL/非事务引擎可能已提交）" } else { "逐语句写入（同物理事务，提交前可取消）" }); }
                    if statements % 100 == 0 { tokio::task::yield_now().await; }
                }
                if let Some(t) = task { t.check()?; }
                Ok(count)
            }.await;
            match result {
                Ok(count) => {
                    tx.commit().await.map_err(|error| anyhow!("TRANSFER_EFFECTS_UNKNOWN: SQL 提交确认失败；可能已生效，必须核对后再操作: {error}"))?;
                    Ok(count)
                }
                Err(error) => match tx.rollback().await {
                    Ok(()) if !$implicit => Err(error.context("SQL 脚本失败；本次物理事务的回滚已确认")),
                    Ok(()) => Err(error.context("TRANSFER_EFFECTS_UNKNOWN: MySQL 待提交 DML 已回滚，但 DDL/非事务引擎可能已生效，不能宣称全部回滚或自动重放")),
                    Err(rollback) => Err(error.context(format!("TRANSFER_EFFECTS_UNKNOWN: SQL 回滚未确认；可能已生效，必须核对后再操作: {rollback}"))),
                },
            }
        }};
    }
    match db { Db::Sqlite(pool) => run!(pool, false), Db::Mysql(pool) => run!(pool, true), Db::Postgres(pool) => run!(pool, false),
        _ => bail!("当前通道尚未验证流式 SQL 执行；没有发送 SQL"), }

    }).await
}

async fn run_import_data(db: &Db, kind: &DbKind, opts: &ImportOptions, mapping: &[ImportColumnMapping], task: Option<&TaskControl>) -> Result<u64> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    validate_names(&[opts.table.clone()])?;
    if opts.sheet.as_deref().is_some_and(|s| !s.is_empty()) { bail!("CSV/JSON/SQL 导入不支持工作表参数"); }
    let limits = TransferLimits::default();
    if opts.format.eq_ignore_ascii_case("sql") {
        if opts.truncate || opts.create_table || !mapping.is_empty() || !opts.on_conflict.is_empty() && opts.on_conflict != "error" || !opts.conflict_columns.is_empty() {
            bail!("SQL 脚本按原语句执行，不支持清空、自动建表、列映射或 CSV/JSON 冲突选项");
        }
        if let Some(t) = task { t.check()?; t.update("parse", 0, 0, "正在完整预扫描 SQL（成功前不会修改目标）"); }
        let file = opts.file.clone(); let encoding = opts.encoding.clone(); let kind = kind.clone(); let control = task.cloned();
        let mut script = tokio::task::spawn_blocking(move || PreparedScript::prepare(&file, &encoding, &kind,
            usize::try_from(limits.max_record_bytes).context("SQL 单条语句上限超过当前平台可表示的内存范围")?, limits.spill_budget_bytes.as_deref(), || control.as_ref().map_or(Ok(()), TaskControl::check))).await??;
        return execute_prepared_script(db, &mut script, task).await;
    }
    if let Some(t) = task { t.check()?; t.update("parse", 0, 0, "正在打开并解析文件"); }
    let mut feeder = match opts.format.to_ascii_lowercase().as_str() {
        "csv" => {
            let delim = validate_delimiter(&opts.delimiter)?;
            RowFeeder::Csv(CsvStream::open(&opts.file, &opts.encoding, delim, opts.has_header)?)
        }
        "json" => {
            let names = JsonStream::scan_names(&opts.file, &opts.encoding, limits.max_record_bytes)?;
            validate_names(&names)?;
            let mut s = JsonStream::open(&opts.file, &opts.encoding, limits.max_record_bytes)?;
            s.names = names;
            RowFeeder::Json(s)
        }
        _ => bail!("不支持的导入格式: {}", opts.format),
    };
    let names = feeder.names().to_vec();
    if names.is_empty() { bail!("导入文件没有列，未修改目标"); }
    validate_names(&names)?;
    let exists = crate::schema::list_tables(db, kind.clone(), &opts.schema).await?.iter().any(|(n, _)| n == &opts.table);
    if !exists && !opts.create_table { bail!("目标表不存在，请启用自动建表"); }
    let target_meta = if exists { Some(crate::schema::table_schema(db, kind.clone(), &opts.schema, &opts.table).await?) } else { None };
    let target_cols = target_meta.as_ref().map(|s| s.columns.iter().map(|c| c.name.clone()).collect::<Vec<_>>()).unwrap_or_else(|| names.clone());
    let pairs: Vec<(usize, String)> = if !mapping.is_empty() {
        mapping.iter().map(|m| {
            let i = names.iter().position(|n| n == &m.source).ok_or_else(|| anyhow!("源列不存在: {}", m.source))?;
            if exists && !target_cols.contains(&m.target) { bail!("目标列不存在: {}", m.target); }
            Ok((i, m.target.clone()))
        }).collect::<Result<_>>()?
    } else if opts.format.eq_ignore_ascii_case("csv") && !opts.has_header && exists {
        if names.len() > target_cols.len() { bail!("无表头 CSV 列数超过目标表列数"); }
        target_cols.iter().take(names.len()).enumerate().map(|(i, c)| (i, c.clone())).collect()
    } else {
        names.iter().enumerate().filter_map(|(i, n)| target_cols.iter().find(|c| c.eq_ignore_ascii_case(n)).map(|c| (i, c.clone()))).collect()
    };
    let cols = pairs.iter().map(|(_, c)| c.clone()).collect::<Vec<_>>();
    validate_names(&cols)?;
    if pairs.is_empty() { bail!("没有匹配的列，未修改目标"); }
    let full = qualified(kind, &opts.schema, &opts.table);
    let strategy = { let v = opts.on_conflict.to_ascii_lowercase(); if v.is_empty() { "error".to_string() } else { v } };
    if !matches!(strategy.as_str(), "error" | "skip" | "update") { bail!("导入冲突策略无效：仅支持 error/skip/update"); }
    let conflict_cols: Vec<String> = if strategy == "error" {
        Vec::new()
    } else {
        if !exists { bail!("目标表不存在：自动建表不支持冲突处理"); }
        if !matches!(kind.protocol(), Protocol::Sqlite | Protocol::Mysql | Protocol::Postgres) { bail!("当前目标通道尚未接入导入冲突处理；请在 SQLite/MySQL/PostgreSQL 使用"); }
        let keys: Vec<String> = if opts.conflict_columns.is_empty() {
            target_meta.as_ref().map(|m| m.columns.iter().filter(|c| c.primary_key).map(|c| c.name.clone()).collect()).unwrap_or_default()
        } else { opts.conflict_columns.clone() };
        if keys.is_empty() { bail!("冲突处理需要目标表主键或显式指定冲突键列"); }
        for k in &keys {
            if !cols.contains(k) { bail!("冲突键列必须参与导入列映射: {}", k); }
            if exists && !target_cols.contains(k) { bail!("目标表不存在冲突键列: {}", k); }
        }
        keys
    };
    let mut prefix: Vec<(String, bool)> = Vec::new();
    if !exists { prefix.push((format!("CREATE TABLE {} ({})", full, cols.iter().map(|c| format!("{} TEXT", quote_ident(kind, c))).collect::<Vec<_>>().join(", ")), false)); }
    if opts.truncate { prefix.push((format!("DELETE FROM {}", full), false)); }
    let quoted = cols.iter().map(|c| quote_ident(kind, c)).collect::<Vec<_>>().join(", ");
    let types = pairs.iter().map(|(_, name)| target_meta.as_ref().and_then(|s| s.columns.iter().find(|c| c.name == *name)).map(|c| c.data_type.clone()).unwrap_or_else(|| "TEXT".to_string())).collect::<Vec<_>>();
    let mapper = ImportMapper { pairs, kind: kind.clone(), full, quoted, strategy, conflict_cols, cols, types };
    execute_import_batches(db, &prefix, &mut feeder, &mapper, &limits, task).await

    }).await
}

/// Keep empty table headers and binary values, unlike schema::fetch_all_rows.
pub async fn fetch_transfer_rows(db: &Db, kind: &DbKind, schema: &str, table: &str, clause: Option<&str>, limit: Option<u64>) -> Result<(Vec<String>, Vec<Vec<Option<String>>>)> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    validate_where(clause)?; validate_names(&[table.to_string()])?;
    let sql = format!("SELECT * FROM {}{}{}", qualified(kind, schema, table), clause.filter(|s| !s.trim().is_empty()).map(|s| format!(" WHERE {}", s)).unwrap_or_default(), limit.map(|l| format!(" LIMIT {}", l)).unwrap_or_default());
    let names = crate::schema::table_schema(db, kind.clone(), schema, table).await?.columns.into_iter().map(|c| c.name).collect::<Vec<_>>();
    fetch_rows_sql(db, kind, schema, table, &sql, names).await

    }).await
}

/// 键集分页读取：与 OFFSET 分页等价，但用 (k1,k2) > (上一页末行键) 定位，
/// 深页不再需要扫描并丢弃前 offset 行（OFFSET 在大表上是 O(n²)）。
pub async fn fetch_transfer_page_after(
    db: &Db, kind: &DbKind, schema: &str, table: &str, clause: Option<&str>,
    key_cols: &[String], key_types: &[String], after: Option<&[Option<String>]>, limit: u64,
) -> Result<(Vec<String>, Vec<Vec<Option<String>>>)> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    validate_where(clause)?; validate_names(&[table.to_string()])?;
    for c in key_cols { validate_names(&[c.clone()])?; }
    if key_cols.is_empty() || key_cols.len() != key_types.len() { bail!("键集分页需要键列及其类型"); }
    let quoted = key_cols.iter().map(|c| quote_ident(kind, c)).collect::<Vec<_>>().join(", ");
    let mut predicates: Vec<String> = Vec::new();
    if let Some(c) = clause.filter(|s| !s.trim().is_empty()) { predicates.push(format!("({})", c)); }
    if let Some(values) = after {
        if values.len() != key_cols.len() { bail!("键集分页的续页键列数量不一致"); }
        let literals = values.iter().zip(key_types.iter())
            .map(|(value, ty)| crate::sync::value_sql(value.as_deref(), kind, ty, ty))
            .collect::<Result<Vec<_>>>()?;
        if values.iter().any(|value| value.is_none()) { bail!("键集分页键值为 NULL，无法安全续页"); }
        predicates.push(format!("({}) > ({})", quoted, literals.join(", ")));
    }
    let mut sql = format!("SELECT * FROM {}", qualified(kind, schema, table));
    if !predicates.is_empty() { sql.push_str(&format!(" WHERE {}", predicates.join(" AND "))); }
    sql.push_str(&format!(" ORDER BY {} LIMIT {}", quoted, limit));
    let names = crate::schema::table_schema(db, kind.clone(), schema, table).await?.columns.into_iter().map(|c| c.name).collect::<Vec<_>>();
    fetch_rows_sql(db, kind, schema, table, &sql, names).await

    }).await
}

/// 分页读取：只用于只读导出与快照读取，不作为镜像删除的完整性依据。
pub async fn fetch_transfer_page(db: &Db, kind: &DbKind, schema: &str, table: &str, clause: Option<&str>, order_cols: &[String], offset: u64, limit: u64) -> Result<(Vec<String>, Vec<Vec<Option<String>>>)> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    validate_where(clause)?; validate_names(&[table.to_string()])?;
    for c in order_cols { validate_names(&[c.clone()])?; }
    let mut sql = format!("SELECT * FROM {}", qualified(kind, schema, table));
    if let Some(c) = clause.filter(|s| !s.trim().is_empty()) { sql.push_str(&format!(" WHERE {}", c)); }
    if !order_cols.is_empty() {
        sql.push_str(" ORDER BY ");
        sql.push_str(&order_cols.iter().map(|c| quote_ident(kind, c)).collect::<Vec<_>>().join(", "));
    }
    sql.push_str(&format!(" LIMIT {} OFFSET {}", limit, offset));
    let names = crate::schema::table_schema(db, kind.clone(), schema, table).await?.columns.into_iter().map(|c| c.name).collect::<Vec<_>>();
    fetch_rows_sql(db, kind, schema, table, &sql, names).await

    }).await
}

async fn fetch_rows_sql(db: &Db, kind: &DbKind, schema: &str, table: &str, sql: &str, names: Vec<String>) -> Result<(Vec<String>, Vec<Vec<Option<String>>>)> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    macro_rules! fetch {
        ($pool:expr, $convert:path) => {{
            let fetched = crate::sql_history::query(sql).fetch_all($pool).await?;
            let names = fetched.first().map(|r| r.columns().iter().map(|c| c.name().to_string()).collect::<Vec<_>>()).unwrap_or(names);
            let mut rows = Vec::with_capacity(fetched.len());
            for row in fetched {
                let mut vals = $convert(&row, names.len());
                for i in 0..names.len() {
                    let ty = row.try_get_raw(i)?.type_info().name().to_ascii_uppercase();
                    if ty.contains("BLOB") || ty.contains("BINARY") || ty == "BYTEA" {
                        vals[i] = row.try_get::<Option<Vec<u8>>, _>(i)?.map(|b| format!("0x{}", b.iter().map(|b| format!("{:02X}", b)).collect::<String>()));
                    }
                }
                rows.push(vals);
            }
            Ok((names, rows))
        }};
    }
    match db { Db::Sqlite(p) => fetch!(p, crate::values::sqlite_row_values), Db::Mysql(p) => fetch!(p, crate::values::my_row_values), Db::Postgres(p) => fetch!(p, crate::values::pg_row_values),
        Db::Mssql(_) | Db::ClickHouse(_) | Db::Odbc(_) => {
            let (mut names, rows) = db.generic_rows(sql).await?;
            if names.is_empty() {
                names = crate::schema::table_schema(db, kind.clone(), schema, table).await?.columns.into_iter().map(|c| c.name).collect();
            }
            Ok((names, rows))
        }
        _ => bail!("该数据源不支持导入导出") }

    }).await
}

/// 流式导出写入器：有界序列化写入临时文件，完整成功后无覆盖发布目标。
pub(crate) enum ExportSink {
    Csv { out: FileEncoder, delim: char, columns: usize },
    Json { out: FileEncoder, first: bool, names: Vec<String> },
    Sql { out: FileEncoder, kind: DbKind, prefix: String, types: Vec<String>, buffer: Vec<u8> },
}

impl ExportSink {
    fn view(stage: &StagedOutput, kind: &DbKind, schema: &str, definition: &str, encoding: &str) -> Result<()> {
        if definition.trim().is_empty() || definition.len() > TransferLimits::default().max_record_bytes as usize { bail!("真实视图定义为空或超过导出上限"); }
        let mut out = stage.encoder(encoding)?;
        out.write_all("-- 视图只导出真实定义，不向视图写入 INSERT。请先创建其依赖对象；本文件的表部分仅包含数据。\r\n".as_bytes())?;
        if kind.protocol() == crate::models::Protocol::Mssql { write!(&mut out, "USE {};\r\nGO\r\n", quote_ident(kind, schema))?; }
        out.write_all(definition.trim().trim_end_matches(';').as_bytes())?;
        out.write_all(b";\r\n")?;
        if kind.protocol() == crate::models::Protocol::Mssql { out.write_all(b"GO\r\n")?; }
        out.finish()?.get_ref().sync_all()?;
        Ok(())
    }

    fn create(stage: &StagedOutput, format: &str, kind: &DbKind, schema: &str, table: &str, names: &[String], types: &[String], delimiter: &str, has_header: bool, encoding: &str) -> Result<Self> {
        validate_names(names)?;
        if !types.is_empty() && types.len() != names.len() { bail!("导出类型与列数不一致"); }
        if !names.iter().try_fold(0u64, |sum, name| u64::try_from(name.len()).ok().and_then(|bytes| sum.checked_add(bytes))).is_some_and(|bytes| bytes <= TransferLimits::default().max_record_bytes) { bail!("导出表头超过单条记录上限"); }
        let mut out = stage.encoder(encoding)?;
        match format {
            "csv" => {
                let delim = validate_delimiter(delimiter)?;
                if has_header {
                    let cells: Vec<String> = names.iter().map(|n| csv_escape(n, delim)).collect();
                    out.write_all((cells.join(&delim.to_string()) + "\r\n").as_bytes())?;
                }
                Ok(ExportSink::Csv { out, delim, columns: names.len() })
            }
            "json" => {
                out.write_all(b"[\n")?;
                Ok(ExportSink::Json { out, first: true, names: names.to_vec() })
            }
            "sql" => {
                let full = qualified(kind, schema, table);
                let cols = names.iter().map(|n| quote_ident(kind, n)).collect::<Vec<_>>().join(", ");
                if kind.protocol() == crate::models::Protocol::Mssql && types.iter().any(|ty| ty.eq_ignore_ascii_case("rowversion") || ty.eq_ignore_ascii_case("timestamp")) {
                    out.write_all("-- SQL Server rowversion 为服务器生成的只读值；下方保留原始 0xHEX，恢复归档数据时该目标列须为 BINARY(8)，不能显式写入真实 rowversion 列。\r\n".as_bytes())?;
                }
                Ok(ExportSink::Sql { out, kind: kind.clone(), prefix: format!("INSERT INTO {} ({}) VALUES (", full, cols), types: if types.is_empty() { vec!["TEXT".into(); names.len()] } else { types.to_vec() }, buffer: Vec::with_capacity(1024) })
            }
            _ => bail!("不支持的导出格式"),
        }
    }

    pub(crate) fn write_rows(&mut self, rows: &[Vec<Option<String>>]) -> Result<u64> {
        let limits = TransferLimits::default();
        let columns = match self { Self::Csv { columns, .. } => *columns, Self::Json { names, .. } => names.len(), Self::Sql { types, .. } => types.len() };
        if u64::try_from(rows.len()).map_or(true, |rows| rows > limits.batch_rows) { bail!("导出批次行数超过上限"); }
        for row in rows {
            if row.len() != columns { bail!("导出行列数与实际列元数据不一致"); }
            if !row.iter().try_fold(0usize, |sum, v| sum.checked_add(v.as_ref().map_or(0, String::len))).is_some_and(|bytes| u64::try_from(bytes).is_ok_and(|bytes| bytes <= limits.max_record_bytes)) { bail!("导出单条记录超过上限"); }
        }
        let mut count = 0u64;
        match self {
            ExportSink::Csv { out, delim, .. } => {
                for row in rows {
                    let mut record = LimitedRecordWriter { inner: out, bytes: 0, max_bytes: limits.max_record_bytes };
                    for (i, value) in row.iter().enumerate() {
                        if i > 0 { record.write_all(delim.to_string().as_bytes())?; }
                        record.write_all(csv_cell(value.as_deref(), *delim).as_bytes())?;
                    }
                    record.write_all(b"\r\n")?;
                    count += 1;
                }
            }
            ExportSink::Json { out, first, names } => {
                for row in rows {
                    let mut obj = serde_json::Map::new();
                    for (i, name) in names.iter().enumerate() {
                        obj.insert(name.clone(), serde_json::Value::from(row.get(i).cloned().flatten()));
                    }
                    if *first { *first = false; } else { out.write_all(b",\n")?; }
                    let record = LimitedRecordWriter { inner: out, bytes: 0, max_bytes: limits.max_record_bytes };
                    serde_json::to_writer(record, &serde_json::Value::Object(obj))?;
                    count += 1;
                }
            }
            ExportSink::Sql { out, kind, prefix, types, buffer } => {
                for row in rows {
                    // 单条 INSERT 结构/顺序不变；复用有界序列化缓冲，编码器每行只处理一次。
                    // 不攒整表，不合并多行 INSERT，转义扩张仍在写入缓冲前核对上限。
                    buffer.clear();
                    let mut record = LimitedRecordWriter { inner: &mut *buffer, bytes: 0, max_bytes: limits.max_record_bytes };
                    record.write_all(prefix.as_bytes())?;
                    for (i, value) in row.iter().enumerate() {
                        let ty = &types[i];
                        let literal_type = if kind.protocol() == crate::models::Protocol::Mssql && (ty.eq_ignore_ascii_case("rowversion") || ty.eq_ignore_ascii_case("timestamp")) { "BINARY(8)" } else { ty };
                        let value = crate::sync::value_sql(value.as_deref(), kind, literal_type, literal_type)?;
                        if i > 0 { record.write_all(b", ")?; }
                        record.write_all(value.as_bytes())?;
                    }
                    record.write_all(b");\r\n")?;
                    out.write_all(buffer)?;
                    count += 1;
                }
            }
        }
        Ok(count)
    }

    pub(crate) fn finish(self) -> Result<()> {
        let out = match self {
            ExportSink::Csv { out, .. } | ExportSink::Sql { out, .. } => out,
            ExportSink::Json { mut out, .. } => { out.write_all(b"\n]\n")?; out },
        };
        out.finish()?.get_ref().sync_all()?;
        Ok(())
    }
}

/// 限制的是序列化后的单条记录字节数；转义扩张也不能突破上限。
struct LimitedRecordWriter<'a, W: Write> { inner: &'a mut W, bytes: u64, max_bytes: u64 }
impl<W: Write> Write for LimitedRecordWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if u64::try_from(bytes.len()).map_or(true, |len| len > self.max_bytes.saturating_sub(self.bytes)) { return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "导出序列化记录超过上限")); }
        let written = self.inner.write(bytes)?; self.bytes += written as u64; Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> { self.inner.flush() }
}

/// 把一次执行缓存下来的完整结果集流式写成文件。
///
/// 读取的是执行时落盘的行，不会重跑用户 SQL；写入先落在目标旁边的临时文件，
/// 完整成功后无覆盖原子发布；已有目标保持不变，临时文件由 RAII 回收。
pub fn export_spool_set(set: &crate::query_spool::SpoolSet, opts: &ExportOptions, kind: &DbKind, names: &[String], types: &[String]) -> Result<u64> {
    validate_export(opts)?;
    if names != set.names() || types != set.types() { bail!("导出列元数据与查询快照不一致"); }
    set.ensure_complete()?;
    if opts.limit.is_some_and(|limit| limit < set.total()) { bail!("完整查询快照导出不接受截断行数上限；请清空 limit 后重试"); }
    let stage = StagedOutput::new(&opts.file)?;
    let mut sink = ExportSink::create(&stage, &opts.format.to_ascii_lowercase(), kind, &opts.schema, &opts.table, names, types, &opts.delimiter, opts.has_header, &opts.encoding)?;
    let mut count = 0u64;
    set.stream(256, |rows| {
        count = count.checked_add(sink.write_rows(&rows)?).ok_or_else(|| anyhow!("导出行数溢出"))?;
        Ok(())
    })?;
    sink.finish()?;
    if count != set.total() { bail!("导出快照行数不一致，没有发布文件"); }
    stage.publish(|| set.ensure_complete())?;
    Ok(count)
}

/// 转码测试及受控 UTF-8 来源复用同一个严格有界编码器，不允许覆盖既有目标。
fn transcode_file(src: &Path, dst: &Path, encoding: &str) -> Result<()> {
    let src = transfer_output::checked_absolute(src)?;
    let mut options = std::fs::OpenOptions::new(); options.read(true);
    #[cfg(windows)] { use std::os::windows::fs::OpenOptionsExt; options.share_mode(1); }
    let mut input = options.open(src)?;
    if !input.metadata()?.is_file() { bail!("转码来源必须是普通文件"); }
    let stage = StagedOutput::new(dst.to_str().ok_or_else(|| anyhow!("导出路径必须能无损表示为 UTF-8"))?)?;
    let mut output = stage.encoder(encoding)?;
    std::io::copy(&mut input, &mut output)?;
    let output = output.finish()?; output.get_ref().sync_all()?; drop(output);
    stage.publish(|| Ok(()))
}

pub async fn run_export_task(db: &Db, kind: &DbKind, opts: &ExportTaskOptions) -> Result<u64> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let task = TaskControl::begin(&opts.task_id)?;
    let result = async {
        let o = &opts.options;
        validate_export(o)?; protect_database_path(db, &o.file).await?; task.check()?;
        task.update("read", 0, 0, "正在读取源数据");

        // 所有编码复用有界写入器和受控 stage，编码/读取/取消失败均不发布目标。
        let stage = StagedOutput::new(&o.file)?;
        let count = transfer_export_cursor::export_cursor(db, kind, o, &task, &stage).await?;
        stage.publish(|| task.check())?;
        task.update("export", count, count, "文件已保存"); Ok(count)
    }.await;
    task.finish(&result); result

    }).await
}
pub async fn protect_database_path(db: &Db, output: &str) -> Result<()> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if let Db::Sqlite(pool) = db {
        for row in crate::sql_history::query("PRAGMA database_list").fetch_all(pool).await? {
            let path: String = row.try_get("file")?;
            if !path.is_empty() && std::fs::canonicalize(&path).ok() == std::fs::canonicalize(output).ok() && Path::new(output).exists() { bail!("输出文件不能覆盖当前 SQLite 数据库"); }
        }
    }
    if ["db", "sqlite", "sqlite3", "db3"].contains(&Path::new(output).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str()) { bail!("导出文件不能使用数据库文件扩展名"); }
    Ok(())

    }).await
}

#[cfg(test)]
#[path = "transfer_spool_tests.rs"]
mod transfer_spool_tests;

#[cfg(test)]
#[path = "transfer_streaming_tests.rs"]
mod transfer_streaming_tests;

#[cfg(test)]
mod import_conflict_tests {
    use super::*;

    fn cols() -> Vec<String> { vec!["id".into(), "name".into()] }

    #[test]
    fn error_strategy_is_plain_insert() {
        let sql = import_insert_statement("\"t\"", "\"id\", \"name\"", "1, 'a'", &DbKind::of("sqlite"), "error", &[], &cols()).unwrap();
        assert_eq!(sql, "INSERT INTO \"t\" (\"id\", \"name\") VALUES (1, 'a')");
    }

    #[test]
    fn sqlite_skip_and_update_use_on_conflict() {
        let skip = import_insert_statement("\"t\"", "\"id\", \"name\"", "1, 'a'", &DbKind::of("sqlite"), "skip", &["id".into()], &cols()).unwrap();
        assert!(skip.contains("ON CONFLICT (\"id\") DO NOTHING"));
        let upd = import_insert_statement("\"t\"", "\"id\", \"name\"", "1, 'a'", &DbKind::of("sqlite"), "update", &["id".into()], &cols()).unwrap();
        assert!(upd.contains("DO UPDATE SET \"name\" = excluded.\"name\""));
    }

    #[test]
    fn mysql_uses_ignore_and_duplicate_key() {
        let skip = import_insert_statement("`t`", "`id`, `name`", "1, 'a'", &DbKind::of("mysql"), "skip", &["id".into()], &cols()).unwrap();
        assert!(skip.starts_with("INSERT IGNORE INTO"));
        let upd = import_insert_statement("`t`", "`id`, `name`", "1, 'a'", &DbKind::of("mysql"), "update", &["id".into()], &cols()).unwrap();
        assert!(upd.contains("ON DUPLICATE KEY UPDATE"));
    }

    #[test]
    fn update_requires_non_key_column() {
        assert!(import_insert_statement("\"t\"", "\"id\"", "1", &DbKind::of("sqlite"), "update", &["id".into()], &["id".into()]).is_err());
    }

    #[test]
    fn unknown_strategy_rejected() {
        assert!(import_insert_statement("\"t\"", "\"id\"", "1", &DbKind::of("sqlite"), "merge", &[], &["id".into()]).is_err());
    }
}
