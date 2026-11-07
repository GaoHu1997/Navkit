//! 导入 / 导出 / 数据传输 / SQL 转储。

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::{Column, Row, TypeInfo, ValueRef};

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
    let enc = text_encoding(encoding)?;
    let bytes = if encoding.eq_ignore_ascii_case("latin1") || encoding.eq_ignore_ascii_case("iso-8859-1") {
        content.chars().map(|c| u8::try_from(c as u32).map_err(|_| anyhow!("内容不能以 ISO-8859-1 无损保存"))).collect::<Result<Vec<_>>>()?
    } else {
        let (bytes, _, errors) = enc.encode(content);
        if errors { bail!("内容不能以 {} 无损保存", encoding); }
        bytes.into_owned()
    };
    ensure_writable(path)?;
    // Write beside the destination, then replace it only after a complete successful export.
    let temp = Path::new(path).with_file_name(format!(".navkit-export-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut f = File::options().write(true).create_new(true).open(&temp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
        std::fs::rename(&temp, path).context("无法替换导出文件（文件可能正在使用）")?;
        Ok(())
    })();
    if result.is_err() { let _ = std::fs::remove_file(&temp); }
    result
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
            .map(|v| match v {
                Some(s) if s == "\\N" => "\\\\N".to_string(),
                Some(s) => csv_escape(s, delim),
                None => "\\N".to_string(),
            })
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
    run_import_data(db, &kind, opts, &[], None).await
}
pub async fn import_json(db: &Db, kind: DbKind, opts: &ImportOptions) -> Result<u64> {
    run_import_data(db, &kind, opts, &[], None).await
}
pub async fn import_sql(db: &Db, kind: DbKind, opts: &ImportOptions) -> Result<u64> {
    run_import_data(db, &kind, opts, &[], None).await
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
        create_target: opts.create_target, columns: vec![], where_clause: opts.where_clause.clone(),
    };
    Ok(crate::sync::synchronize(source, source_kind, target, target_kind, &sync_opts).await?.rows)
}

/// 生成 SQL 转储文件。
pub async fn dump(
    db: &Db,
    kind: &DbKind,
    opts: &DumpOptions,
) -> Result<u64> {
    if opts.structure_only && opts.data_only { bail!("仅结构与仅数据不能同时选择"); }
    protect_database_path(db, &opts.file).await?;
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

    let mut count = 0u64;
    if !opts.structure_only {
        let (names, rows) = crate::schema::fetch_all_rows(
            db,
            &kind,
            &opts.schema,
            &opts.table,
            None,
            None,
        )
        .await?;
        let cols: Vec<String> = names.iter().map(|n| quote_ident(&kind, n)).collect();
        for row in &rows {
            let vals: Vec<String> = row
                .iter()
                .map(|v| match v {
                    Some(s) => crate::db::quote_literal(s),
                    None => "NULL".to_string(),
                })
                .collect();
            out.push_str(&format!(
                "INSERT INTO {} ({}) VALUES ({});\r\n",
                full,
                cols.join(", "),
                vals.join(", ")
            ));
            count += 1;
        }
    }

    write_text(&opts.file, "utf-8", &out)?;
    Ok(count)
}

/// 整库备份：把 schema 下全部（或指定）表 / 视图转储为单个 SQL 文件。
///
/// 与单表 dump() 不同，备份文件按流式写入（BufWriter），
/// 结构部分：MySQL 用 SHOW CREATE TABLE 的真实 DDL，
/// SQLite 用 sqlite_master 里的原始 SQL，PostgreSQL 综合字段信息生成；
/// 数据部分按每 100 行一条多值 INSERT 输出。
/// 返回 (数据行数, 文件字节数)。
pub async fn dump_database(
    db: &Db,
    kind: &DbKind,
    schema: &str,
    tables: &[String],
    include_structure: bool,
    include_data: bool,
    path: &str,
    comment: &str,
) -> Result<(u64, u64)> {
    use std::io::BufWriter;

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

    let file = File::create(path).with_context(|| format!("无法创建文件: {}", path))?;
    let mut w = BufWriter::new(file);
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
                chunk.push_str(&format!(
                    "DROP {} IF EXISTS {};\r\n",
                    if is_view { "VIEW" } else { "TABLE" },
                    full
                ));
                chunk.push_str(&sql);
                if !sql.trim_end().ends_with(';') {
                    chunk.push(';');
                }
                chunk.push_str("\r\n\r\n");
                w.write_all(chunk.as_bytes())?;
                bytes += chunk.len() as u64;
            }
        }

        if include_data && !is_view {
            let (names, rows) = crate::schema::fetch_all_rows(db, &kind, schema, name, None, None).await?;
            if !names.is_empty() {
                let cols: Vec<String> = names.iter().map(|n| quote_ident(&kind, n)).collect();
                let mut buf = String::new();
                for chunk in rows.chunks(100) {
                    let mut vals_all = Vec::with_capacity(chunk.len());
                    for row in chunk {
                        let vals: Vec<String> = row
                            .iter()
                            .map(|v| match v {
                                Some(s) => crate::db::quote_literal(s),
                                None => "NULL".to_string(),
                            })
                            .collect();
                        vals_all.push(format!("({})", vals.join(", ")));
                    }
                    buf.push_str(&format!(
                        "INSERT INTO {} ({}) VALUES {};\r\n",
                        full,
                        cols.join(", "),
                        vals_all.join(",\r\n  ")
                    ));
                    if buf.len() > 1 << 20 {
                        w.write_all(buf.as_bytes())?;
                        bytes += buf.len() as u64;
                        buf.clear();
                    }
                }
                if !buf.is_empty() {
                    w.write_all(buf.as_bytes())?;
                    bytes += buf.len() as u64;
                }
                w.write_all(b"\r\n")?;
                bytes += 2;
                rows_total += rows.len() as u64;
            }
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
    w.flush()?;

    Ok((rows_total, bytes))
}

/// 取单个表 / 视图的 CREATE 语句（已含结尾分号前的全部内容）。
async fn table_ddl(
    db: &Db,
    kind: &DbKind,
    schema: &str,
    name: &str,
    is_view: bool,
) -> Result<Option<String>> {
    match db {
        Db::Mssql(_) => {
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
            let rows = sqlx::query(
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
            let rows = sqlx::query(&sql).fetch_all(pool).await?;
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
                let rows = sqlx::query(&sql).fetch_all(pool).await?;
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
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("无法读取备份文件: {}", path))?;
    let stmts: Vec<String> = crate::db::split_statements(&content)
        .into_iter()
        .map(|s| crate::db::strip_leading_comments(&s))
        .filter(|s| !s.is_empty())
        .collect();

    let mut steps: Vec<RestoreStep> = Vec::with_capacity(stmts.len());
    let mut succeeded = 0usize;
    let mut failed = 0usize;

    for (i, t) in stmts.iter().enumerate() {
        let mut sql_short: String = t.chars().take(500).collect();
        if t.chars().count() > 500 {
            sql_short.push_str(" …");
        }
        let sql_short = sql_short.trim().to_string();
        let outcome = execute_one(db, t).await;
        match outcome {
            Ok(affected) => {
                succeeded += 1;
                steps.push(RestoreStep {
                    index: i + 1,
                    sql: sql_short,
                    ok: true,
                    affected,
                    error: None,
                    skipped: false,
                });
            }
            Err(err) => {
                if is_fk_guard_stmt(t) {
                    steps.push(RestoreStep {
                        index: i + 1,
                        sql: sql_short,
                        ok: true,
                        affected: None,
                        error: Some(format!("守卫语句已跳过：{err}")),
                        skipped: true,
                    });
                    continue;
                }
                failed += 1;
                steps.push(RestoreStep {
                    index: i + 1,
                    sql: sql_short,
                    ok: false,
                    affected: None,
                    error: Some(err.to_string()),
                    skipped: false,
                });
                // 失败即中止，避免半套数据落库
                break;
            }
        }
    }

    Ok(RestoreReport {
        total: stmts.len(),
        succeeded,
        failed,
        steps,
    })
}

/// 单条语句执行（按数据源选择驱动），返回影响行数。
async fn execute_one(db: &Db, sql: &str) -> Result<Option<u64>> {
    match db {
        Db::Sqlite(pool) => {
            let mut conn = pool.acquire().await?;
            let r = sqlx::query(sql).execute(&mut *conn).await?;
            Ok(Some(r.rows_affected()))
        }
        Db::Mysql(pool) => {
            let mut conn = pool.acquire().await?;
            let r = sqlx::query(sql).execute(&mut *conn).await?;
            Ok(Some(r.rows_affected()))
        }
        Db::Postgres(pool) => {
            let mut conn = pool.acquire().await?;
            let r = sqlx::query(sql).execute(&mut *conn).await?;
            Ok(Some(r.rows_affected()))
        }
        Db::Mssql(_) | Db::ClickHouse(_) | Db::Odbc(_) => Ok(Some(db.generic_exec(sql).await?)),
        Db::Redis(_) | Db::Mongo(_) | Db::Http(_) => {
            Err(anyhow!("该数据源类型不支持 SQL 脚本还原"))
        }
    }
}

/// 执行 SQL 文件中的全部语句（还原备份的共用实现），返回执行的语句数。
///
/// 全部语句在**同一连接**上执行：外键守卫（SET FOREIGN_KEY_CHECKS /
/// PRAGMA foreign_keys / SET session_replication_role）是会话级设置，
/// 走连接池会让"关外键"与"还原数据"落在不同会话，守卫形同虚设。
/// 守卫语句失败时跳过（如 PG 普通用户不能设置 session_replication_role）。
pub async fn restore_sql_file(db: &Db, _kind: &DbKind, path: &str) -> Result<u64> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("无法读取备份文件: {}", path))?;
    // 剥离纯注释块并过滤空语句
    let stmts: Vec<String> = crate::db::split_statements(&content)
        .into_iter()
        .map(|s| crate::db::strip_leading_comments(&s))
        .filter(|s| !s.is_empty())
        .collect();

    let mut count = 0u64;
    match db {
        Db::Sqlite(pool) => {
            let mut conn = pool.acquire().await?;
            let _ = sqlx::query("PRAGMA foreign_keys = OFF").execute(&mut *conn).await;
            for t in &stmts {
                match sqlx::query(t).execute(&mut *conn).await {
                    Ok(_) => count += 1,
                    Err(e) => {
                        if is_fk_guard_stmt(t) {
                            continue;
                        }
                        let _ = sqlx::query("PRAGMA foreign_keys = ON").execute(&mut *conn).await;
                        return Err(anyhow::Error::from(e).context(format!("还原在第 {} 条语句失败", count + 1)));
                    }
                }
            }
            let _ = sqlx::query("PRAGMA foreign_keys = ON").execute(&mut *conn).await;
        }
        Db::Mysql(pool) => {
            let mut conn = pool.acquire().await?;
            let _ = sqlx::query("SET FOREIGN_KEY_CHECKS = 0").execute(&mut *conn).await;
            for t in &stmts {
                match sqlx::query(t).execute(&mut *conn).await {
                    Ok(_) => count += 1,
                    Err(e) => {
                        if is_fk_guard_stmt(t) {
                            continue;
                        }
                        let _ = sqlx::query("SET FOREIGN_KEY_CHECKS = 1").execute(&mut *conn).await;
                        return Err(anyhow::Error::from(e).context(format!("还原在第 {} 条语句失败", count + 1)));
                    }
                }
            }
            let _ = sqlx::query("SET FOREIGN_KEY_CHECKS = 1").execute(&mut *conn).await;
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
                match sqlx::query(t).execute(&mut *conn).await {
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
            t.progress.status = if result.is_ok() { "completed" } else if self.cancel.load(Ordering::Relaxed) { "cancelled" } else { "failed" }.into();
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
    let total = statements.iter().filter(|(_, row)| *row).count() as u64;
    macro_rules! run {
        ($pool:expr) => {{
            let mut tx = $pool.begin().await?;
            let mut count = 0u64;
            for (i, (sql, is_row)) in statements.iter().enumerate() {
                if let Some(t) = task { if let Err(e) = t.check() { tx.rollback().await?; return Err(e.context("任务已取消，未提交的目标修改已回滚")); } }
                match sqlx::query(sql).execute(&mut *tx).await {
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
}
pub async fn run_import_task(db: &Db, kind: &DbKind, opts: &ImportTaskOptions) -> Result<u64> {
    let task = TaskControl::begin(&opts.task_id)?;
    let result = run_import_data(db, kind, &opts.options, &opts.column_mapping, Some(&task)).await;
    task.finish(&result); result
}
async fn run_import_data(db: &Db, kind: &DbKind, opts: &ImportOptions, mapping: &[ImportColumnMapping], task: Option<&TaskControl>) -> Result<u64> {
    validate_names(&[opts.table.clone()])?;
    if opts.sheet.as_deref().is_some_and(|s| !s.is_empty()) { bail!("CSV/JSON/SQL 导入不支持工作表参数"); }
    let content = read_text(&opts.file, &opts.encoding)?;
    if let Some(t) = task { t.check()?; t.update("parse", 0, 0, "正在解析文件"); }
    if opts.format.eq_ignore_ascii_case("sql") {
        if opts.truncate || opts.create_table || !mapping.is_empty() { bail!("SQL 脚本按原语句执行，不支持清空、自动建表或列映射"); }
        let stmts = crate::db::split_statements(&content).into_iter().map(|s| crate::db::strip_leading_comments(&s)).filter(|s| !s.is_empty()).collect::<Vec<_>>();
        for s in &stmts {
            let word = s.split_whitespace().next().unwrap_or("").to_ascii_uppercase();
            if matches!(word.as_str(), "BEGIN" | "COMMIT" | "ROLLBACK" | "END" | "SAVEPOINT" | "RELEASE" | "START" | "ATTACH" | "DETACH" | "VACUUM" | "PRAGMA" | "SET" | "USE") { bail!("SQL 导入不允许会话/事务控制语句: {}", word); }
        }
        return execute_atomic(db, &stmts.into_iter().map(|s| { let row = s.trim_start().to_ascii_uppercase().starts_with("INSERT"); (s, row) }).collect::<Vec<_>>(), task).await;
    }
    let (names, rows): (Vec<String>, Vec<Vec<Option<String>>>) = match opts.format.to_ascii_lowercase().as_str() {
        "csv" => { let (names, rows) = parse_csv(&content, validate_delimiter(&opts.delimiter)?, opts.has_header)?;
            (names, rows.into_iter().map(|r| r.into_iter().map(|s| if s == "\\N" { None } else { Some(if s == "\\\\N" { "\\N".into() } else { s }) }).collect()).collect()) },
        "json" => {
            let v: serde_json::Value = serde_json::from_str(&content).context("JSON 格式无效")?;
            let items = match v { serde_json::Value::Array(a) => a, serde_json::Value::Object(_) => vec![v], _ => bail!("JSON 必须是对象或对象数组") };
            let mut names = Vec::new();
            for item in &items {
                let map = item.as_object().ok_or_else(|| anyhow!("JSON 每一行都必须是对象"))?;
                for key in map.keys() { if !names.contains(key) { names.push(key.clone()); } }
            }
            validate_names(&names)?;
            let rows = items.iter().map(|item| names.iter().map(|n| match item.get(n) { None | Some(serde_json::Value::Null) => None, Some(serde_json::Value::String(s)) => Some(s.clone()), Some(v) => Some(v.to_string()) }).collect()).collect();
            (names, rows)
        },
        _ => bail!("不支持的导入格式: {}", opts.format),
    };
    if names.is_empty() { bail!("导入文件没有列，未修改目标"); }
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
    let mut stmts = Vec::new();
    if !exists { stmts.push((format!("CREATE TABLE {} ({})", full, cols.iter().map(|c| format!("{} TEXT", quote_ident(kind, c))).collect::<Vec<_>>().join(", ")), false)); }
    if opts.truncate { stmts.push((format!("DELETE FROM {}", full), false)); }
    let quoted = cols.iter().map(|c| quote_ident(kind, c)).collect::<Vec<_>>().join(", ");
    for row in rows {
        let vals = pairs.iter().map(|(i, name)| {
            let ty = target_meta.as_ref().and_then(|s| s.columns.iter().find(|c| c.name == *name)).map(|c| c.data_type.as_str()).unwrap_or("TEXT");
            crate::sync::value_sql(row.get(*i).and_then(Option::as_deref), kind, ty, ty)
        }).collect::<Result<Vec<_>>>()?;
        stmts.push((format!("INSERT INTO {} ({}) VALUES ({})", full, quoted, vals.join(", ")), true));
    }
    execute_atomic(db, &stmts, task).await
}

/// Keep empty table headers and binary values, unlike schema::fetch_all_rows.
pub async fn fetch_transfer_rows(db: &Db, kind: &DbKind, schema: &str, table: &str, clause: Option<&str>, limit: Option<u64>) -> Result<(Vec<String>, Vec<Vec<Option<String>>>)> {
    validate_where(clause)?; validate_names(&[table.to_string()])?;
    let sql = format!("SELECT * FROM {}{}{}", qualified(kind, schema, table), clause.filter(|s| !s.trim().is_empty()).map(|s| format!(" WHERE {}", s)).unwrap_or_default(), limit.map(|l| format!(" LIMIT {}", l)).unwrap_or_default());
    let names = crate::schema::table_schema(db, kind.clone(), schema, table).await?.columns.into_iter().map(|c| c.name).collect::<Vec<_>>();
    macro_rules! fetch {
        ($pool:expr, $convert:path) => {{
            let fetched = sqlx::query(&sql).fetch_all($pool).await?;
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
        // 通用通道（SQL Server / ClickHouse / ODBC 关系型）：适配器直接返回文本行；
        // 结果列为空（0 行且驱动未给列名）时回退到元数据层列名。
        Db::Mssql(_) | Db::ClickHouse(_) | Db::Odbc(_) => {
            let (mut names, rows) = db.generic_rows(&sql).await?;
            if names.is_empty() {
                names = crate::schema::table_schema(db, kind.clone(), schema, table).await?.columns.into_iter().map(|c| c.name).collect();
            }
            Ok((names, rows))
        }
        _ => bail!("该数据源不支持导入导出") }
}
pub async fn run_export_task(db: &Db, kind: &DbKind, opts: &ExportTaskOptions) -> Result<u64> {
    let task = TaskControl::begin(&opts.task_id)?;
    let result = async {
        let o = &opts.options;
        validate_export(o)?; protect_database_path(db, &o.file).await?; task.check()?;
        task.update("read", 0, 0, "正在读取源数据");
        let (names, rows) = fetch_transfer_rows(db, kind, &o.schema, &o.table, o.where_clause.as_deref(), o.limit).await?;
        task.check()?; task.update("export", 0, rows.len() as u64, "正在生成导出文件");
        let types = if o.format.eq_ignore_ascii_case("sql") { crate::schema::table_schema(db, kind.clone(), &o.schema, &o.table).await?.columns.into_iter().map(|c| c.data_type).collect::<Vec<_>>() } else { vec![] };
        let temp = Path::new(&o.file).with_file_name(format!(".navkit-task-{}.tmp", uuid::Uuid::new_v4()));
        let temp_string = temp.to_string_lossy().to_string();
        let format = o.format.to_ascii_lowercase(); let encoding = o.encoding.clone(); let delimiter = o.delimiter.clone();
        let header = o.has_header; let k = kind.clone(); let schema = o.schema.clone(); let table = o.table.clone();
        let write_result = tokio::task::spawn_blocking(move || match format.as_str() {
            "csv" => export_csv(&temp_string, &names, &rows, &delimiter, &encoding, header),
            "json" => export_json(&temp_string, &names, &rows, &encoding),
            "sql" => export_sql_typed(&temp_string, &k, &schema, &table, &names, &rows, &types, &encoding),
            _ => Err(anyhow!("不支持的导出格式")),
        }).await?;
        let complete = (|| -> Result<u64> {
            let count = write_result?; task.check()?;
            std::fs::rename(&temp, &o.file).context("无法替换导出文件")?;
            Ok(count)
        })();
        if complete.is_err() { let _ = std::fs::remove_file(&temp); }
        let count = complete?;
        task.update("export", count, count, "文件已保存"); Ok(count)
    }.await;
    task.finish(&result); result
}
pub async fn protect_database_path(db: &Db, output: &str) -> Result<()> {
    if let Db::Sqlite(pool) = db {
        for row in sqlx::query("PRAGMA database_list").fetch_all(pool).await? {
            let path: String = row.try_get("file")?;
            if !path.is_empty() && std::fs::canonicalize(&path).ok() == std::fs::canonicalize(output).ok() && Path::new(output).exists() { bail!("输出文件不能覆盖当前 SQLite 数据库"); }
        }
    }
    if ["db", "sqlite", "sqlite3", "db3"].contains(&Path::new(output).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str()) { bail!("导出文件不能使用数据库文件扩展名"); }
    Ok(())
}
