//! 元数据层：对象树、表结构、分页读取、行编辑、导入导出。

use anyhow::{anyhow, Result};
#[path = "schema_relational_keys.rs"]
mod relational_keys;
use sqlx::mysql::MySqlRow;
use sqlx::postgres::PgRow;
use sqlx::{Column, Row, TypeInfo, ValueRef};

use crate::db::{quote_ident, qualified, Db};
use crate::models::{
    ColumnInfo, DbKind, ForeignKeyInfo, IndexInfo, ObjectInfo, ObjectType, PageRequest,
    PageResult, ResultColumn, RowInsert, RowUpdate, TableSchema, TreeNode,
};

/// 从一行中按列名取字符串（元数据专用解码）。
/// information_schema / mysql.user 的 VARBINARY 列（COLUMN_TYPE、COLUMN_KEY、User、Host 等）
/// 按文本展示：合法 UTF-8 字节解码为文本；非法字节保留 0x 十六进制（与数据路径一致，可往返）。
/// 不得改 values.rs 数据路径：0x 前缀是二进制数据同步/编辑往返契约的一部分（BUG-R1-012）。
pub(crate) fn mysql_get(row: &MySqlRow, name: &str) -> Option<String> {
    let idx = row.columns().iter().position(|c| c.name() == name)?;
    let binary = row
        .try_get_raw(idx)
        .ok()
        .map(|v| crate::values::is_binary_type(v.type_info().name()))
        .unwrap_or(false);
    if binary {
        if let Ok(Some(bytes)) = row.try_get::<Option<Vec<u8>>, _>(idx) {
            return Some(match std::str::from_utf8(&bytes) {
                Ok(text) => text.to_string(),
                Err(_) => crate::values::format_binary(&bytes),
            });
        }
    }
    crate::values::my_row_values(row, row.columns().len())
        .get(idx)
        .cloned()
        .flatten()
}

/// 元数据行解码：与 mysql_get 相同的「合法 UTF-8 则文本」策略，按整行返回。
pub(crate) fn mysql_meta_row_values(row: &MySqlRow) -> Vec<Option<String>> {
    let count = row.columns().len();
    (0..count)
        .map(|i| {
            let name = row.columns()[i].name().to_string();
            mysql_get(row, &name)
        })
        .collect()
}

/// 从 PostgreSQL 行中按列名取字符串。
fn pg_get(row: &PgRow, name: &str) -> Option<String> {
    let idx = row.columns().iter().position(|c| c.name() == name)?;
    crate::values::pg_row_values(row, row.columns().len())
        .get(idx)
        .cloned()
        .flatten()
}

/// 列出该连接可见的数据库 / 模式。
pub async fn list_databases(db: &Db, kind: DbKind) -> Result<Vec<(String, String)>> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if let Db::Http(h) = db {
        return h.containers().await;
    }
    if let Db::Redis(r) = db {
        return r.databases().await;
    }
    if let Db::Mongo(m) = db {
        return m.databases().await;
    }
    if matches!(db, Db::ClickHouse(_) | Db::Odbc(_) | Db::Mssql(_)) {
        return generic_list_databases(db, &kind).await;
    }
    let mut out = Vec::new();
    match db {
        Db::Sqlite(_pool) => {
            // SQLite 单库：固定返回 main（此前误把表/视图当数据库列出）。
            out.push(("main".to_string(), "main".to_string()));
        }
        Db::Mysql(pool) => {
            let rows = crate::sql_history::query("SHOW DATABASES").fetch_all(pool).await?;
            for r in &rows {
                let name = mysql_get(r, "Database").unwrap_or_default();
                if !name.is_empty() {
                    out.push((name.clone(), name));
                }
            }
        }
        Db::Postgres(pool) => {
            let rows = crate::sql_history::query(
                "SELECT nspname FROM pg_namespace \
                 WHERE nspname NOT LIKE 'pg_%' AND nspname <> 'information_schema' \
                 ORDER BY nspname",
            )
            .fetch_all(pool)
            .await?;
            for r in &rows {
                let name = pg_get(r, "nspname").unwrap_or_default();
                if !name.is_empty() {
                    out.push((name.clone(), name));
                }
            }
        }
        _ => {}
    }
    let _ = kind;
    Ok(out)

    }).await
}

/// 列出某个数据库 / 模式下的表与视图。
pub async fn list_tables(
    db: &Db,
    kind: DbKind,
    schema: &str,
) -> Result<Vec<(String, ObjectType)>> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if let Db::Http(h) = db {
        // 支持 HTTP/JDBC 类关系源时按类型分类（H2 / JDBC 已实现），视图不再混入表。
        return Ok(h.objects_with_types(schema).await?);
    }
    if let Db::Redis(r) = db {
        // 库节点形如 db0：扫描键作为「表」展示
        let dbn = schema
            .strip_prefix("db")
            .and_then(|n| n.parse::<i64>().ok())
            .unwrap_or(0);
        let keys = r.keys(dbn, "*", 500).await?;
        return Ok(keys.into_iter().map(|k| (k, ObjectType::Table)).collect());
    }
    if let Db::Mongo(m) = db {
        let colls = m.collections(schema).await?;
        return Ok(colls.into_iter().map(|n| (n, ObjectType::Table)).collect());
    }
    if matches!(db, Db::ClickHouse(_) | Db::Odbc(_) | Db::Mssql(_)) {
        return generic_list_tables(db, &kind, schema).await;
    }
    let mut out = Vec::new();
    match db {
        Db::Sqlite(pool) => {
            let rows = crate::sql_history::query(
                "SELECT name, type FROM sqlite_master \
                 WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .fetch_all(pool)
            .await?;
            for r in &rows {
                let vals = crate::values::sqlite_row_values(r, 2);
                let name = vals.first().cloned().flatten().unwrap_or_default();
                let ty = vals.get(1).cloned().flatten().unwrap_or_default();
                out.push((
                    name,
                    if ty == "view" { ObjectType::View } else { ObjectType::Table },
                ));
            }
        }
        Db::Mysql(pool) => {
            let sql = format!(
                "SELECT TABLE_NAME, TABLE_TYPE FROM information_schema.TABLES \
                 WHERE TABLE_SCHEMA = {} ORDER BY TABLE_NAME",
                crate::db::quote_literal(schema)
            );
            let rows = crate::sql_history::query(&sql).fetch_all(pool).await?;
            for r in &rows {
                let name = mysql_get(r, "TABLE_NAME").unwrap_or_default();
                let ty = mysql_get(r, "TABLE_TYPE").unwrap_or_default();
                out.push((
                    name,
                    if ty.eq_ignore_ascii_case("VIEW") {
                        ObjectType::View
                    } else {
                        ObjectType::Table
                    },
                ));
            }
        }
        Db::Postgres(pool) => {
            let sql = format!(
                "SELECT c.relname, c.relkind::text AS relkind FROM pg_class c \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE n.nspname = {} AND c.relkind IN ('r','v','m','p') \
                 ORDER BY c.relname",
                crate::db::quote_literal(schema)
            );
            let rows = crate::sql_history::query(&sql).fetch_all(pool).await?;
            for r in &rows {
                let name = pg_get(r, "relname").unwrap_or_default();
                let kindc = pg_get(r, "relkind").unwrap_or_default();
                out.push((
                    name,
                    if kindc == "r" || kindc == "p" {
                        ObjectType::Table
                    } else {
                        ObjectType::View
                    },
                ));
            }
        }
        _ => {}
    }
    let _ = kind;
    Ok(out)

    }).await
}

/// 列出某模式下的对象及其统计信息（对象标签页网格数据）。
pub async fn list_objects(
    db: &Db,
    kind: DbKind,
    schema: &str,
) -> Result<Vec<ObjectInfo>> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if let Db::Http(_) | Db::Redis(_) | Db::Mongo(_) = db {
        let tables = list_tables(db, kind, schema).await?;
        return Ok(tables
            .into_iter()
            .map(|(name, obj_type)| ObjectInfo {
                name,
                obj_type,
                auto_increment: 0,
                update_time: String::new(),
                data_length: 0,
                engine: String::new(),
                rows: 0,
                comment: String::new(),
            })
            .collect());
    }
    if matches!(db, Db::ClickHouse(_) | Db::Odbc(_) | Db::Mssql(_)) {
        // 由表清单构造对象列表（行数/大小随表结构读取补充）
        let tables = generic_list_tables(db, &kind, schema).await?;
        let mut out = Vec::new();
        for (name, ty) in tables {
            let schema_info = generic_table_schema(db, &kind, schema, &name).await.ok();
            out.push(ObjectInfo {
                name,
                obj_type: ty,
                auto_increment: 0,
                update_time: String::new(),
                data_length: 0,
                engine: String::new(),
                rows: schema_info.as_ref().and_then(|s| s.row_count).unwrap_or(0),
                comment: String::new(),
            });
        }
        return Ok(out);
    }
    let mut out: Vec<ObjectInfo> = Vec::new();
    match db {
        Db::Sqlite(pool) => {
            let rows = crate::sql_history::query(
                "SELECT name, type FROM sqlite_master                  WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .fetch_all(pool)
            .await?;
            for r in &rows {
                let vals = crate::values::sqlite_row_values(r, 2);
                let name = vals.first().cloned().flatten().unwrap_or_default();
                let ty = vals.get(1).cloned().flatten().unwrap_or_default();
                let n: i64 = crate::sql_history::query_scalar(&format!(
                    "SELECT COUNT(*) FROM {}",
                    quote_ident(&kind, &name)
                ))
                .fetch_one(pool)
                .await
                .unwrap_or(0);
                out.push(ObjectInfo {
                    name,
                    obj_type: if ty == "view" { ObjectType::View } else { ObjectType::Table },
                    auto_increment: 0,
                    update_time: String::new(),
                    data_length: 0,
                    engine: String::new(),
                    rows: n,
                    comment: String::new(),
                });
            }
        }
        Db::Mysql(pool) => {
            let sql = format!(
                "SELECT TABLE_NAME, TABLE_TYPE, AUTO_INCREMENT, UPDATE_TIME, DATA_LENGTH,                  ENGINE, TABLE_ROWS, TABLE_COMMENT FROM information_schema.TABLES                  WHERE TABLE_SCHEMA = {} ORDER BY TABLE_NAME",
                crate::db::quote_literal(schema)
            );
            let rows = crate::sql_history::query(&sql).fetch_all(pool).await?;
            for r in &rows {
                let name = mysql_get(r, "TABLE_NAME").unwrap_or_default();
                let ty = mysql_get(r, "TABLE_TYPE").unwrap_or_default();
                // information_schema 统计列均为 bigint unsigned，i64 解码必失败
                let ai = r.try_get::<Option<u64>, _>("AUTO_INCREMENT").unwrap_or(None).unwrap_or(0) as i64;
                let dl = r.try_get::<Option<u64>, _>("DATA_LENGTH").unwrap_or(None).unwrap_or(0) as i64;
                let tr = r.try_get::<Option<u64>, _>("TABLE_ROWS").unwrap_or(None).unwrap_or(0) as i64;
                let engine = mysql_get(r, "ENGINE").unwrap_or_default();
                let comment = mysql_get(r, "TABLE_COMMENT").unwrap_or_default();
                let upd = mysql_get(r, "UPDATE_TIME").unwrap_or_default();
                out.push(ObjectInfo {
                    name,
                    obj_type: if ty.eq_ignore_ascii_case("VIEW") { ObjectType::View } else { ObjectType::Table },
                    auto_increment: ai,
                    update_time: upd,
                    data_length: dl,
                    engine,
                    rows: tr,
                    comment,
                });
            }
        }
        Db::Postgres(pool) => {
            let sql = format!(
                "SELECT c.relname, c.relkind::text AS relkind, c.reltuples::bigint AS tab_rows,                  pg_total_relation_size(c.oid) AS total_bytes, obj_description(c.oid) AS descr                  FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace                  WHERE n.nspname = {} AND c.relkind IN ('r','v','m','p') ORDER BY c.relname",
                crate::db::quote_literal(schema)
            );
            let rows = crate::sql_history::query(&sql).fetch_all(pool).await?;
            for r in &rows {
                let name = pg_get(r, "relname").unwrap_or_default();
                let kindc = pg_get(r, "relkind").unwrap_or_default();
                let tr: i64 = r.try_get("tab_rows").unwrap_or(0);
                let dl: i64 = r.try_get("total_bytes").unwrap_or(0);
                let descr = pg_get(r, "descr").unwrap_or_default();
                out.push(ObjectInfo {
                    name,
                    obj_type: if kindc == "r" || kindc == "p" { ObjectType::Table } else { ObjectType::View },
                    auto_increment: 0,
                    update_time: String::new(),
                    data_length: dl,
                    engine: String::new(),
                    rows: if tr < 0 { 0 } else { tr },
                    comment: descr,
                });
            }
        }
        _ => {}
    }
    let _ = kind;
    Ok(out)

    }).await
}

/// 列出某个模式下的存储过程 / 函数 / 触发器 / 事件。
pub async fn list_routines(
    db: &Db,
    kind: DbKind,
    schema: &str,
) -> Result<Vec<(ObjectType, String)>> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if let Db::Http(_) | Db::Redis(_) | Db::Mongo(_) = db {
        return Ok(Vec::new());
    }
    if matches!(db, Db::ClickHouse(_) | Db::Odbc(_) | Db::Mssql(_)) {
        // ClickHouse / Hive 无存储过程；达梦从 ALL_OBJECTS 读取
        if kind.id() == "dameng" {
            let (_, rows) = db
                .generic_rows(
                    "SELECT OBJECT_NAME, OBJECT_TYPE FROM ALL_OBJECTS \n                     WHERE OWNER = USER AND OBJECT_TYPE IN ('PROCEDURE', 'FUNCTION') \n                     ORDER BY OBJECT_NAME",
                )
                .await?;
            return Ok(rows
                .into_iter()
                .filter_map(|r| {
                    let name = r.first()?.clone()?;
                    let ty = r.get(1).cloned().flatten().unwrap_or_default();
                    let t = if ty.eq_ignore_ascii_case("PROCEDURE") {
                        ObjectType::Procedure
                    } else {
                        ObjectType::Function
                    };
                    Some((t, name))
                })
                .collect());
        }
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    match db {
        Db::Sqlite(pool) => {
            let rows = crate::sql_history::query(
                "SELECT name FROM sqlite_master WHERE type = 'trigger' ORDER BY name",
            )
            .fetch_all(pool)
            .await?;
            for r in &rows {
                let name = crate::values::sqlite_row_values(r, 1)
                    .first()
                    .cloned()
                    .flatten()
                    .unwrap_or_default();
                out.push((ObjectType::Trigger, name));
            }
        }
        Db::Mysql(pool) => {
            let sql = format!(
                "SELECT ROUTINE_NAME, ROUTINE_TYPE FROM information_schema.ROUTINES \
                 WHERE ROUTINE_SCHEMA = {} ORDER BY ROUTINE_NAME",
                crate::db::quote_literal(schema)
            );
            if let Ok(rows) = crate::sql_history::query(&sql).fetch_all(pool).await {
                for r in &rows {
                    let name = mysql_get(r, "ROUTINE_NAME").unwrap_or_default();
                    let ty = mysql_get(r, "ROUTINE_TYPE").unwrap_or_default();
                    let t = if ty.eq_ignore_ascii_case("FUNCTION") {
                        ObjectType::Function
                    } else {
                        ObjectType::Procedure
                    };
                    out.push((t, name));
                }
            }
            let tsql = format!(
                "SELECT TRIGGER_NAME FROM information_schema.TRIGGERS \
                 WHERE TRIGGER_SCHEMA = {} ORDER BY TRIGGER_NAME",
                crate::db::quote_literal(schema)
            );
            if let Ok(rows) = crate::sql_history::query(&tsql).fetch_all(pool).await {
                for r in &rows {
                    out.push((
                        ObjectType::Trigger,
                        mysql_get(r, "TRIGGER_NAME").unwrap_or_default(),
                    ));
                }
            }
        }
        Db::Postgres(pool) => {
            let sql = format!(
                "SELECT p.proname, p.prokind::text AS prokind FROM pg_proc p \
                 JOIN pg_namespace n ON n.oid = p.pronamespace \
                 WHERE n.nspname = {} ORDER BY p.proname",
                crate::db::quote_literal(schema)
            );
            if let Ok(rows) = crate::sql_history::query(&sql).fetch_all(pool).await {
                for r in &rows {
                    let name = pg_get(r, "proname").unwrap_or_default();
                    let k = pg_get(r, "prokind").unwrap_or_default();
                    let t = if k == "f" { ObjectType::Function } else { ObjectType::Procedure };
                    out.push((t, name));
                }
            }
            let tsql = format!(
                "SELECT tgname FROM pg_trigger t JOIN pg_class c ON c.oid = t.tgrelid \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE n.nspname = {} AND NOT tgisinternal ORDER BY tgname",
                crate::db::quote_literal(schema)
            );
            if let Ok(rows) = crate::sql_history::query(&tsql).fetch_all(pool).await {
                for r in &rows {
                    out.push((
                        ObjectType::Trigger,
                        pg_get(r, "tgname").unwrap_or_default(),
                    ));
                }
            }
        }
        _ => {}
    }
    let _ = kind;
    Ok(out)

    }).await
}

/// 构造对象树节点（供前端构建树时复用）。
#[allow(dead_code)]
pub fn make_node(
    id: &str,
    label: &str,
    kind: ObjectType,
    icon: &str,
    expandable: bool,
) -> TreeNode {
    TreeNode {
        id: id.to_string(),
        label: label.to_string(),
        kind,
        icon: icon.to_string(),
        children: Vec::new(),
        expandable,
        has_children: expandable,
        expanded: false,
    }
}

/// 读取表的完整结构。
pub async fn table_schema(
    db: &Db,
    kind: DbKind,
    schema: &str,
    table: &str,
) -> Result<TableSchema> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if let Db::Http(h) = db {
        return http_table_schema(h, kind, schema, table).await;
    }
    if let Db::Redis(r) = db {
        return redis_table_schema(r, kind, schema, table).await;
    }
    if let Db::Mongo(m) = db {
        return mongo_table_schema(m, kind, schema, table).await;
    }
    if matches!(db, Db::ClickHouse(_) | Db::Odbc(_) | Db::Mssql(_)) {
        return generic_table_schema(db, &kind, schema, table).await;
    }
    let mut columns = Vec::new();
    let mut indexes = Vec::new();
    let mut foreign_keys = Vec::new();
    let mut comment = None;
    let mut engine = None;
    let mut collation = None;

    match db {
        Db::Sqlite(pool) => {
            // SQLite 的 schema 只能是 main / temp / attached 名称。调用方（整库备份、导入等）
            // 可能把「数据库文件名」当作 schema 传入；此时必须回退到 main，否则 PRAGMA 会报
            // "no such table: <name>.sqlite_schema"（BUG-R1-028）。
            let sqlite_namespace = {
                let attached: Vec<String> = crate::sql_history::query("PRAGMA database_list")
                    .fetch_all(pool).await?
                    .iter().filter_map(|r| crate::values::sqlite_row_values(r, 3).get(1).cloned().flatten()).collect();
                if schema.is_empty() || !attached.iter().any(|n| n == schema) { "main" } else { schema }
            };
            let table_pragma = format!("PRAGMA {}.table_info({})", quote_ident(&DbKind::of("sqlite"), sqlite_namespace), quote_ident(&DbKind::of("sqlite"), table));
            let rows = crate::sql_history::query(&table_pragma)
                .fetch_all(pool)
                .await?;
            let mut pk_idx: Vec<(i64, String)> = Vec::new();
            for r in &rows {
                let vals = crate::values::sqlite_row_values(r, 6);
                let name = vals.get(1).cloned().flatten().unwrap_or_default();
                let ctype = vals.get(2).cloned().flatten().unwrap_or_default();
                let notnull = vals
                    .get(3)
                    .cloned()
                    .flatten()
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(0);
                let dflt = vals.get(4).cloned().flatten();
                let pk = vals
                    .get(5)
                    .cloned()
                    .flatten()
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(0);
                columns.push(ColumnInfo {
                    name: name.clone(),
                    data_type: ctype,
                    nullable: notnull == 0,
                    default: dflt,
                    primary_key: pk > 0,
                    auto_increment: false,
                    comment: None,
                    length: None,
                    precision: None,
                    scale: None,
                });
                if pk > 0 {
                    pk_idx.push((pk, name));
                }
            }
            pk_idx.sort();
            if !pk_idx.is_empty() {
                let names: Vec<String> = pk_idx.into_iter().map(|(_, n)| n).collect();
                indexes.push(IndexInfo {
                    name: "PRIMARY KEY".into(),
                    columns: names,
                    unique: true,
                    primary: true,
                    method: None,
                });
            }
            indexes.extend(relational_keys::sqlite_indexes(pool, sqlite_namespace, table).await?);
            foreign_keys = relational_keys::sqlite_foreign_keys(pool, sqlite_namespace, table).await?;
        }
        Db::Mysql(pool) => {
            let sql = format!(
                "SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, \
                 COLUMN_KEY, EXTRA, COLUMN_COMMENT, CHARACTER_MAXIMUM_LENGTH, \
                 NUMERIC_PRECISION, NUMERIC_SCALE \
                 FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = {} AND TABLE_NAME = {} \
                 ORDER BY ORDINAL_POSITION",
                crate::db::quote_literal(schema),
                crate::db::quote_literal(table)
            );
            let rows = crate::sql_history::query(&sql).fetch_all(pool).await?;
            for r in &rows {
                let name = mysql_get(r, "COLUMN_NAME").unwrap_or_default();
                let ctype = mysql_get(r, "COLUMN_TYPE").unwrap_or_default();
                let nullable = mysql_get(r, "IS_NULLABLE").unwrap_or_default();
                let dflt = mysql_get(r, "COLUMN_DEFAULT");
                let key = mysql_get(r, "COLUMN_KEY").unwrap_or_default();
                let extra = mysql_get(r, "EXTRA").unwrap_or_default();
                let cmt = mysql_get(r, "COLUMN_COMMENT");
                let len = mysql_get(r, "CHARACTER_MAXIMUM_LENGTH").and_then(|v| v.parse().ok());
                let prec = mysql_get(r, "NUMERIC_PRECISION").and_then(|v| v.parse().ok());
                let scale = mysql_get(r, "NUMERIC_SCALE").and_then(|v| v.parse().ok());
                columns.push(ColumnInfo {
                    name,
                    data_type: ctype,
                    nullable: nullable.eq_ignore_ascii_case("YES"),
                    default: dflt,
                    primary_key: key.eq_ignore_ascii_case("PRI"),
                    auto_increment: extra.to_ascii_lowercase().contains("auto_increment"),
                    comment: cmt,
                    length: len,
                    precision: prec,
                    scale,
                });
            }
            let (actual_indexes, actual_foreign_keys) = relational_keys::mysql_keys(pool, schema, table).await?;
            indexes = actual_indexes;
            foreign_keys = actual_foreign_keys;
            let csql = format!(
                "SELECT TABLE_COMMENT, ENGINE, TABLE_COLLATION FROM information_schema.TABLES \
                 WHERE TABLE_SCHEMA = {} AND TABLE_NAME = {}",
                crate::db::quote_literal(schema),
                crate::db::quote_literal(table)
            );
            let rows = crate::sql_history::query(&csql).fetch_all(pool).await?;
            if let Some(r) = rows.first() {
                comment = mysql_get(r, "TABLE_COMMENT");
                engine = mysql_get(r, "ENGINE");
                collation = mysql_get(r, "TABLE_COLLATION");
            } else {
                return Err(anyhow!("METADATA_OBJECT_UNKNOWN: 表选项目录未返回目标对象"));
            }
        }
        Db::Postgres(pool) => {
            let sql = format!(
                "SELECT a.attname, format_type(a.atttypid, a.atttypmod), a.attnotnull, \
                 pg_get_expr(ad.adbin, ad.adrelid), \
                 (a.attidentity <> '' OR COALESCE(pg_get_expr(ad.adbin, ad.adrelid) LIKE 'nextval(%', false)) AS auto_increment, \
                 col_description(c.oid, a.attnum) \
                 FROM pg_attribute a \
                 JOIN pg_class c ON c.oid = a.attrelid \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 LEFT JOIN pg_attrdef ad ON ad.adrelid = a.attrelid AND ad.adnum = a.attnum \
                 WHERE n.nspname = {} AND c.relname = {} AND a.attnum > 0 AND NOT a.attisdropped \
                 ORDER BY a.attnum",
                crate::db::quote_literal(schema),
                crate::db::quote_literal(table)
            );
            let rows = crate::sql_history::query(&sql).fetch_all(pool).await?;
            for r in &rows {
                let name = pg_get(r, "attname").unwrap_or_default();
                let ctype = pg_get(r, "format_type").unwrap_or_default();
                let notnull: bool = r.try_get("attnotnull").unwrap_or(false);
                let dflt = pg_get(r, "pg_get_expr");
                let auto_increment: bool = r.try_get("auto_increment").unwrap_or(false);
                let cmt = pg_get(r, "col_description");
                columns.push(ColumnInfo {
                    name,
                    data_type: ctype,
                    nullable: !notnull,
                    default: dflt,
                    primary_key: false,
                    auto_increment,
                    comment: cmt,
                    length: None,
                    precision: None,
                    scale: None,
                });
            }
            let (actual_indexes, actual_foreign_keys) = relational_keys::pg_keys(pool, schema, table).await?;
            indexes = actual_indexes;
            foreign_keys = actual_foreign_keys;
            for column in &mut columns {
                column.primary_key = indexes.iter().filter(|index| index.primary).any(|index| index.columns.contains(&column.name));
            }
            let csql = format!(
                "SELECT obj_description(c.oid) FROM pg_class c \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE n.nspname = {} AND c.relname = {}",
                crate::db::quote_literal(schema),
                crate::db::quote_literal(table)
            );
            let rows = crate::sql_history::query(&csql).fetch_all(pool).await?;
            if let Some(r) = rows.first() {
                comment = pg_get(r, "obj_description");
            } else {
                return Err(anyhow!("METADATA_OBJECT_UNKNOWN: 对象描述目录未返回目标对象"));
            }
        }
            _ => {}
    }

    Ok(TableSchema {
        table: table.to_string(),
        database: schema.to_string(),
        columns,
        indexes,
        foreign_keys,
        comment,
        engine,
        collation,
        row_count: None,
    })

    }).await
}

#[path="table_workflows_filter.rs"]
mod typed_table_filter;
#[path="table_workflows_filter_comparisons.rs"]
mod sensitive_table_filter;

// 仅静态方言测试使用明确的文本 fixture；实际分页必须读取真实列目录。
#[cfg(test)]
fn filter_clause(kind: &DbKind, lines: &[crate::models::FilterLine]) -> Result<String> {
    let types = lines.iter().flat_map(|l| &l.conds).map(|c| (c.column.clone(), typed_table_filter::FilterType::Text)).collect();
    sensitive_table_filter::clause(kind, lines, &types)
}

/// 兼容旧单列排序；显式空数组表示清除，绝不回退旧字段。
fn order_clause(kind: &DbKind, req: &PageRequest) -> String {
    let rules = req.order_rules.clone().unwrap_or_else(|| {
        req.order_by.as_ref().filter(|c| !c.is_empty()).map(|c| vec![crate::models::SortRule {
            column: c.clone(), ascending: req.ascending,
        }]).unwrap_or_default()
    });
    let parts: Vec<String> = rules.iter().filter(|r| !r.column.is_empty()).map(|r| {
        format!("{} {}", quote_ident(kind, &r.column), if r.ascending { "ASC" } else { "DESC" })
    }).collect();
    if parts.is_empty() { String::new() } else { format!(" ORDER BY {}", parts.join(", ")) }
}

/// 分页读取表数据。
///
/// 列结构一律取自表元数据（table_schema），而不是从返回行推导 ——
/// 否则空表 / 筛选无结果时网格没有表头，也无法插入数据。
pub async fn page_table(
    db: &Db,
    kind: DbKind,
    req: &PageRequest,
) -> Result<PageResult> {
    let mut __sql_history_context = crate::sql_history::db_context(db);
    __sql_history_context.origin="page".into();
    crate::sql_history::scope(__sql_history_context, async {
    let start = std::time::Instant::now();
    if let Db::Http(h) = db {
        let mut r = http_page_table(h, req).await?;
        r.elapsed_ms = start.elapsed().as_millis() as u64;
        return Ok(r);
    }
    if let Db::Redis(r) = db {
        let mut r = redis_key_page(r, req).await?;
        r.elapsed_ms = start.elapsed().as_millis() as u64;
        return Ok(r);
    }
    if let Db::Mongo(m) = db {
        let mut r = mongo_doc_page(m, req).await?;
        r.elapsed_ms = start.elapsed().as_millis() as u64;
        return Ok(r);
    }
    if matches!(db, Db::ClickHouse(_) | Db::Odbc(_) | Db::Mssql(_)) {
        let mut r = generic_page_table(db, &kind, req).await?;
        r.elapsed_ms = start.elapsed().as_millis() as u64;
        return Ok(r);
    }
    let full = qualified(&kind, &req.schema, &req.table);
    let where_sql = sensitive_table_filter::typed_clause(db, &kind, req).await?;
    let order_sql = order_clause(&kind, req);
    let limit = req.limit.clamp(1, 100_000);
    let offset = req.offset;

    // 表元数据：列定义 + 主键（失败则退回按行推导）。
    let meta = table_schema(db, kind.clone(), &req.schema, &req.table).await.ok();
    let meta_cols: Vec<ResultColumn> = meta
        .as_ref()
        .map(|m| {
            m.columns
                .iter()
                .map(|c| ResultColumn {
                    name: c.name.clone(),
                    data_type: c.data_type.clone(),
                    nullable: c.nullable,
                    primary_key: c.primary_key,
                    auto_increment: c.auto_increment,
                })
                .collect()
        })
        .unwrap_or_default();
    let meta_pks: Vec<String> = meta.as_ref().map(crate::design::ordered_primary_key).transpose()?.unwrap_or_default();
    let col_count = meta_cols.len();

    let (rows, total, pks) = match db {
        Db::Sqlite(pool) => {
            let count_sql = format!("SELECT COUNT(*) FROM {}{}", full, where_sql);
            let total: i64 = crate::sql_history::query_scalar(&count_sql).fetch_one(pool).await?;
            let sql = format!(
                "SELECT * FROM {}{}{} LIMIT {} OFFSET {}",
                full, where_sql, order_sql, limit, offset
            );
            let fetched = crate::sql_history::query(&sql).fetch_all(pool).await?;
            let out_rows = if let Some(first) = fetched.first() {
                let n = first.columns().len();
                let mut out = Vec::with_capacity(fetched.len());
                for r in &fetched {
                    out.push(crate::values::sqlite_row_values(r, n));
                }
                out
            } else {
                Vec::new()
            };
            // 主键（元数据缺失时退回 PRAGMA）
            let mut pks = meta_pks.clone();
            if pks.is_empty() {
                let namespace = if req.schema.is_empty() { "main" } else { &req.schema };
                let pk_sql = format!("PRAGMA {}.table_info({})", quote_ident(&kind, namespace), quote_ident(&kind, &req.table));
                let pkrows = crate::sql_history::query(&pk_sql).fetch_all(pool).await?;
                let mut ordered = Vec::new();
                for row in pkrows {
                    let ordinal: i64 = row.try_get("pk")?;
                    if ordinal > 0 { ordered.push((ordinal, row.try_get::<String, _>("name")?)); }
                }
                ordered.sort_by_key(|part| part.0);
                anyhow::ensure!(ordered.iter().enumerate().all(|(index, part)| part.0 == index as i64 + 1), "STRUCTURE_METADATA_KEY_ORDER_UNPROVEN");
                pks = ordered.into_iter().map(|part| part.1).collect();
            }
            (out_rows, total as u64, pks)
        }
        Db::Mysql(pool) => {
            let count_sql = format!("SELECT COUNT(*) FROM {}{}", full, where_sql);
            let total: i64 = crate::sql_history::query_scalar(&count_sql).fetch_one(pool).await?;
            let sql = format!(
                "SELECT * FROM {}{}{} LIMIT {} OFFSET {}",
                full, where_sql, order_sql, limit, offset
            );
            let rows = crate::sql_history::query(&sql).fetch_all(pool).await?;
            let out_rows = if let Some(first) = rows.first() {
                let n = first.columns().len();
                let mut out = Vec::with_capacity(rows.len());
                for r in &rows {
                    out.push(crate::values::my_row_values(r, n));
                }
                out
            } else {
                Vec::new()
            };
            let pks = if meta_pks.is_empty() {
                primary_keys_mysql(pool, &req.schema, &req.table).await?
            } else {
                meta_pks
            };
            (out_rows, total as u64, pks)
        }
        Db::Postgres(pool) => {
            let count_sql = format!("SELECT COUNT(*) FROM {}{}", full, where_sql);
            let total: i64 = crate::sql_history::query_scalar(&count_sql).fetch_one(pool).await?;
            let sql = format!(
                "SELECT * FROM {}{}{} LIMIT {} OFFSET {}",
                full, where_sql, order_sql, limit, offset
            );
            let rows = crate::sql_history::query(&sql).fetch_all(pool).await?;
            let out_rows = if let Some(first) = rows.first() {
                let n = first.columns().len();
                let mut out = Vec::with_capacity(rows.len());
                for r in &rows {
                    out.push(crate::values::pg_row_values(r, n));
                }
                out
            } else {
                Vec::new()
            };
            let pks = if meta_pks.is_empty() {
                primary_keys_pg(pool, &req.schema, &req.table).await?
            } else {
                meta_pks
            };
            (out_rows, total as u64, pks)
        }
            _ => (Vec::new(), 0, Vec::new()),
    };

    // 列结构：优先元数据；没有元数据时按行推导（兜底）。
    let columns = if !meta_cols.is_empty() {
        meta_cols
    } else if let Some(first) = rows.first() {
        first
            .iter()
            .enumerate()
            .map(|(i, _)| ResultColumn {
                name: format!("列{}", i + 1),
                data_type: String::new(),
                nullable: true,
                primary_key: false,
                auto_increment: false,
            })
            .collect()
    } else {
        Vec::new()
    };
    // 行宽对齐列数（元数据比 SELECT * 多出表达式列时不至于越界）。
    let width = columns.len().max(col_count);
    let rows = rows
        .into_iter()
        .map(|mut r| {
            if width > r.len() {
                r.resize(width, None);
            }
            r
        })
        .collect();

    Ok(PageResult {
        columns,
        rows,
        total,
        primary_keys: pks,
        elapsed_ms: start.elapsed().as_millis() as u64,
    })

    }).await
}

/// MySQL 主键列。
async fn primary_keys_mysql(
    pool: &sqlx::MySqlPool,
    schema: &str,
    table: &str,
) -> Result<Vec<String>> {
    let rows = crate::sql_history::query("SELECT COLUMN_NAME FROM information_schema.KEY_COLUMN_USAGE WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? AND CONSTRAINT_NAME = 'PRIMARY' ORDER BY ORDINAL_POSITION")
        .bind(schema).bind(table).fetch_all(pool).await?;
    rows.iter().map(|row| mysql_get(row, "COLUMN_NAME").filter(|name| !name.is_empty()).ok_or_else(|| anyhow!("STRUCTURE_METADATA_FIELD_UNPROVEN"))).collect()
}

/// PostgreSQL 主键列。
async fn primary_keys_pg(
    pool: &sqlx::PgPool,
    schema: &str,
    table: &str,
) -> Result<Vec<String>> {
    let rows = crate::sql_history::query("SELECT a.attname::text AS column_name FROM pg_index i JOIN pg_class c ON c.oid = i.indrelid JOIN pg_namespace n ON n.oid = c.relnamespace JOIN LATERAL unnest(i.indkey::smallint[]) WITH ORDINALITY k(attnum,ordinal) ON true JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum = k.attnum WHERE n.nspname = $1 AND c.relname = $2 AND i.indisprimary ORDER BY k.ordinal")
        .bind(schema).bind(table).fetch_all(pool).await?;
    rows.iter().map(|row| row.try_get::<String, _>("column_name").map_err(Into::into)).collect()
}

/// 生成 `col = value` / `col IS NULL` 的 WHERE 条件（NULL 用 IS NULL，否则永远不匹配）。
pub(crate) fn where_condition(kind: &DbKind, col: &str, val: &Option<String>) -> String {
    let ident = quote_ident(&kind, col);
    match val {
        Some(s) => format!("{} = {}", ident, crate::db::quote_literal(s)),
        None => format!("{} IS NULL", ident),
    }
}

/// 更新一行。
pub async fn update_row(db: &Db, kind: DbKind, req: &RowUpdate) -> Result<u64> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if let Db::Http(_) | Db::Redis(_) | Db::Mongo(_) = db {
        return Err(anyhow::anyhow!(
            "该数据源类型不支持行编辑，请在查询标签页执行对应命令"
        ));
    }
    if req.changes.is_empty() {
        return Ok(0);
    }
    let full = qualified(&kind, &req.schema, &req.table);
    let mut sets = Vec::new();
    for (col, val) in &req.changes {
        let v = match val {
            Some(s) => crate::db::quote_literal(s),
            None => "NULL".to_string(),
        };
        sets.push(format!("{} = {}", quote_ident(&kind, col), v));
    }
    let mut conds = Vec::new();
    for (col, val) in &req.primary_keys {
        conds.push(where_condition(&kind, col, val));
    }
    if conds.is_empty() {
        // 无主键时按「旧行全列匹配」定位（与常见客户端一致）。
        for (col, val) in &req.old_values {
            conds.push(where_condition(&kind, col, val));
        }
    }
    if conds.is_empty() {
        return Err(anyhow!("缺少主键条件，无法更新该行"));
    }
    let sql = format!(
        "UPDATE {} SET {} WHERE {}",
        full,
        sets.join(", "),
        conds.join(" AND ")
    );
    exec_affected(db, &kind, &sql).await

    }).await
}

/// 插入一行。
pub async fn insert_row(db: &Db, kind: DbKind, req: &RowInsert) -> Result<u64> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if let Db::Http(_) | Db::Redis(_) | Db::Mongo(_) = db {
        return Err(anyhow::anyhow!(
            "该数据源类型不支持行编辑，请在查询标签页执行对应命令"
        ));
    }
    let full = qualified(&kind, &req.schema, &req.table);
    let cols: Vec<String> = req.columns.iter().map(|c| quote_ident(&kind, c)).collect();
    let vals: Vec<String> = req
        .values
        .iter()
        .map(|v| match v {
            Some(s) => crate::db::quote_literal(s),
            None => "NULL".to_string(),
        })
        .collect();
    if cols.is_empty() {
        return Err(anyhow!("没有可插入的列"));
    }
    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        full,
        cols.join(", "),
        vals.join(", ")
    );
    exec_affected(db, &kind, &sql).await

    }).await
}

/// 删除一行。
pub async fn delete_row(
    db: &Db,
    kind: DbKind,
    schema: &str,
    table: &str,
    primary_keys: &[(String, Option<String>)],
) -> Result<u64> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if let Db::Http(_) | Db::Redis(_) | Db::Mongo(_) = db {
        return Err(anyhow::anyhow!(
            "该数据源类型不支持行编辑，请在查询标签页执行对应命令"
        ));
    }
    let full = qualified(&kind, schema, table);
    let mut conds = Vec::new();
    for (col, val) in primary_keys {
        conds.push(where_condition(&kind, col, val));
    }
    if conds.is_empty() {
        return Err(anyhow!("缺少主键条件，无法删除该行"));
    }
    let sql = format!("DELETE FROM {} WHERE {}", full, conds.join(" AND "));
    exec_affected(db, &kind, &sql).await

    }).await
}

/// 执行 DDL / DML 并返回影响行数。
/// 读取视图定义（完整的 CREATE VIEW 语句）。
pub async fn view_definition(db: &Db, kind: DbKind, schema: &str, view: &str) -> Result<String> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if let Db::Http(_) | Db::Redis(_) | Db::Mongo(_) = db {
        return Err(anyhow::anyhow!("该数据源类型不支持视图定义查看"));
    }
    if matches!(db, Db::ClickHouse(_) | Db::Odbc(_) | Db::Mssql(_)) {
        return generic_view_definition(db, &kind, schema, view).await;
    }
    match db {
        Db::Sqlite(pool) => {
            let rows = crate::sql_history::query(
                "SELECT sql FROM sqlite_master WHERE type = 'view' AND name = ?",
            )
            .bind(view)
            .fetch_all(pool)
            .await?;
            rows.first()
                .and_then(|r| crate::values::sqlite_row_values(r, 1).first().cloned().flatten())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("视图不存在"))
        }
        Db::Mysql(pool) => {
            let full = crate::db::qualified(&kind, schema, view);
            let rows = crate::sql_history::query(&format!("SHOW CREATE VIEW {}", full)).fetch_all(pool).await?;
            let r = rows.first().ok_or_else(|| anyhow::anyhow!("视图不存在"))?;
            let cols = r.columns();
            let vals = crate::values::my_row_values(r, cols.len());
            // SHOW CREATE VIEW 列序：View / Create View / character_set_client / collation_connection
            let ddl = vals.get(1).cloned().flatten().unwrap_or_default();
            if ddl.is_empty() {
                return Err(anyhow::anyhow!("视图定义为空"));
            }
            Ok(crate::transfer::strip_definer(&ddl))
        }
        Db::Postgres(pool) => {
            let full = crate::db::qualified(&kind, schema, view);
            let sql = format!(
                "SELECT pg_get_viewdef('{}'::regclass, true) AS def",
                full.replace('\'', "''")
            );
            let rows = crate::sql_history::query(&sql).fetch_all(pool).await?;
            let def = rows
                .first()
                .and_then(|r| crate::values::pg_row_values(r, 1).first().cloned().flatten())
                .unwrap_or_default();
            if def.is_empty() {
                return Err(anyhow::anyhow!("视图不存在"));
            }
            Ok(format!(
                "CREATE OR REPLACE VIEW {} AS {}",
                full,
                def.trim_end_matches(';').trim()
            ))
        }
            _ => Err(anyhow::anyhow!("不支持的连接类型")),
    }

    }).await
}

fn mysql_table_status_sql(kind: &DbKind, schema: &str, table: &str) -> String {
    format!("SHOW TABLE STATUS FROM {} WHERE Name = {}", quote_ident(kind, schema), crate::db::quote_literal(table))
}

/// 表状态信息（信息面板「表信息」）：行 / 引擎 / 各类长度 / 时间 / 排序规则等。
///
/// 返回 (键, 值) 对，键为 SHOW TABLE STATUS 的英文列名（Rows / Engine /
/// Data_length …），前端按固定顺序映射中文展示；不适用的项由后端省略。
pub async fn table_status(
    db: &Db,
    kind: DbKind,
    schema: &str,
    table: &str,
) -> Result<Vec<(String, String)>> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    match db {
        Db::Mysql(pool) => {
            let sql = mysql_table_status_sql(&kind, schema, table);
            let rows = crate::sql_history::query(&sql).fetch_all(pool).await?;
            let r = rows.first().ok_or_else(|| anyhow::anyhow!("表不存在"))?;
            let cols = r.columns();
            let vals = crate::values::my_row_values(r, cols.len());
            Ok(cols
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    (
                        c.name().to_string(),
                        vals.get(i).cloned().flatten().unwrap_or_default(),
                    )
                })
                .collect())
        }
        Db::Postgres(pool) => {
            let sql = format!(
                "SELECT \
                 c.reltuples::bigint AS \"Rows\", \
                 pg_size_pretty(pg_relation_size(c.oid)) AS \"Data_length\", \
                 pg_relation_size(c.oid) AS \"data_bytes\", \
                 pg_size_pretty((SELECT COALESCE(SUM(pg_relation_size(i.indexrelid)), 0) \
                    FROM pg_index i WHERE i.indrelid = c.oid)) AS \"Index_length\", \
                 pg_size_pretty(pg_total_relation_size(c.oid)) AS \"total_pretty\", \
                 pg_total_relation_size(c.oid) AS \"total_bytes\", \
                 obj_description(c.oid) AS \"Comment\", \
                 (SELECT datcollate FROM pg_database WHERE datname = current_database()) AS \"Collation\", \
                 pg_get_userbyid(c.relowner) AS \"owner\" \
                 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE n.nspname = {} AND c.relname = {}",
                crate::db::quote_literal(schema),
                crate::db::quote_literal(table)
            );
            let rows = crate::sql_history::query(&sql).fetch_all(pool).await?;
            let r = rows.first().ok_or_else(|| anyhow::anyhow!("表不存在"))?;
            let cols = r.columns();
            let vals = crate::values::pg_row_values(r, cols.len());
            let get = |name: &str| {
                cols.iter()
                    .position(|c| c.name() == name)
                    .and_then(|i| vals.get(i).cloned().flatten())
                    .unwrap_or_default()
            };
            let mut out = vec![
                ("Rows".into(), get("Rows")),
                ("Data_length".into(), get("Data_length")),
                ("Index_length".into(), get("Index_length")),
                ("total_bytes".into(), get("total_bytes")),
                ("Collation".into(), get("Collation")),
                ("Comment".into(), get("Comment")),
            ];
            if let Some(i) = cols.iter().position(|c| c.name() == "owner") {
                if let Some(v) = vals.get(i).cloned().flatten() {
                    out.push(("Create_options".into(), format!("owner={}", v)));
                }
            }
            Ok(out)
        }
        Db::Sqlite(pool) => {
            let page_count: i64 =
                crate::sql_history::query_scalar("PRAGMA page_count").fetch_one(pool).await?;
            let page_size: i64 =
                crate::sql_history::query_scalar("PRAGMA page_size").fetch_one(pool).await?;
            let bytes = page_count.saturating_mul(page_size);
            Ok(vec![
                ("Data_length".into(), bytes.to_string()),
                (
                    "Collation".into(),
                    "UTF-8".into(),
                ),
                (
                    "Create_options".into(),
                    format!("{} pages × {} bytes", page_count, page_size),
                ),
            ])
        }
        _ => Err(anyhow::anyhow!("该数据源类型不支持表信息查看")),
    }

    }).await
}

/// 表的建表语句（信息面板「DDL」）。
pub async fn table_ddl(
    db: &Db,
    kind: DbKind,
    schema: &str,
    table: &str,
) -> Result<String> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    match db {
        Db::Mysql(pool) => {
            let full = crate::db::qualified(&kind, schema, table);
            let rows = crate::sql_history::query(&format!("SHOW CREATE TABLE {}", full))
                .fetch_all(pool)
                .await?;
            let r = rows.first().ok_or_else(|| anyhow::anyhow!("表不存在"))?;
            let cols = r.columns();
            let vals = crate::values::my_row_values(r, cols.len());
            // SHOW CREATE TABLE 列序：Table / Create Table
            vals.get(1)
                .cloned()
                .flatten()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("表定义获取失败"))
        }
        Db::Sqlite(pool) => {
            let rows = crate::sql_history::query(
                "SELECT sql FROM sqlite_master WHERE type IN ('table','view') AND name = ?",
            )
            .bind(table)
            .fetch_all(pool)
            .await?;
            rows.first()
                .and_then(|r| crate::values::sqlite_row_values(r, 1).first().cloned().flatten())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("表不存在"))
        }
        _ => {
            // 其余类型由表结构生成 CREATE TABLE（PG / ClickHouse / ODBC / MSSQL）
            let s = table_schema(db, kind.clone(), schema, table).await?;
            if s.columns.is_empty() {
                return Err(anyhow::anyhow!("表不存在"));
            }
            let q = |name: &str| crate::db::quote_ident(&kind, name);
            let full = crate::db::qualified(&kind, schema, table);
            let mut cols_sql: Vec<String> = s
                .columns
                .iter()
                .map(|c| {
                    let mut seg = format!("  {} {}", q(&c.name), c.data_type);
                    if !c.nullable {
                        seg.push_str(" NOT NULL");
                    }
                    if let Some(d) = &c.default {
                        seg.push_str(&format!(" DEFAULT {}", d));
                    }
                    seg
                })
                .collect();
            let pks: Vec<String> = s
                .columns
                .iter()
                .filter(|c| c.primary_key)
                .map(|c| q(&c.name))
                .collect();
            if !pks.is_empty() {
                cols_sql.push(format!("  PRIMARY KEY ({})", pks.join(", ")));
            }
            Ok(format!(
                "CREATE TABLE {} (\n{}\n)",
                full,
                cols_sql.join(",\n")
            ))
        }
    }

    }).await
}

pub async fn exec_affected(db: &Db, _kind: &DbKind, sql: &str) -> Result<u64> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if matches!(db, Db::ClickHouse(_) | Db::Odbc(_)) {
        return db.generic_exec(sql).await;
    }
    match db {
        Db::Sqlite(pool) => {
            let res = crate::sql_history::query(sql).execute(pool).await?;
            Ok(res.rows_affected())
        }
        Db::Mysql(pool) => {
            let res = crate::sql_history::query(sql).execute(pool).await?;
            Ok(res.rows_affected())
        }
        Db::Postgres(pool) => {
            let res = crate::sql_history::query(sql).execute(pool).await?;
            Ok(res.rows_affected())
        }
        Db::Mssql(_) | Db::Http(_) => db.generic_exec(sql).await,
        _ => Err(anyhow!("当前适配器不支持此 SQL 写入操作，没有执行")),
    }

    }).await
}

/// 读取表的原始行（导出用，不分页）。
pub async fn fetch_all_rows(
    db: &Db,
    kind: &DbKind,
    schema: &str,
    table: &str,
    where_clause: Option<&str>,
    limit: Option<u64>,
) -> Result<(Vec<String>, Vec<Vec<Option<String>>>)> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if matches!(db, Db::ClickHouse(_) | Db::Odbc(_)) {
        let full = qualified(&kind, schema, table);
        let where_sql = match where_clause {
            Some(w) if !w.trim().is_empty() => format!(" WHERE {}", w),
            _ => String::new(),
        };
        let limit_sql = match limit {
            Some(l) => format!(" LIMIT {}", l),
            None => String::new(),
        };
        return db
            .generic_rows(&format!("SELECT * FROM {}{}{}", full, where_sql, limit_sql))
            .await;
    }
    let full = qualified(&kind, schema, table);
    let where_sql = match where_clause {
        Some(w) if !w.trim().is_empty() => format!(" WHERE {}", w),
        _ => String::new(),
    };
    let limit_sql = match limit {
        Some(l) => format!(" LIMIT {}", l),
        None => String::new(),
    };
    let sql = format!("SELECT * FROM {}{}{}", full, where_sql, limit_sql);
    match db {
        Db::Sqlite(pool) => {
            let fetched = crate::sql_history::query(&sql).fetch_all(pool).await?;
            let mut names = Vec::new();
            let mut rows_out = Vec::new();
            if let Some(first) = fetched.first() {
                names = first.columns().iter().map(|c| c.name().to_string()).collect();
            }
            for r in &fetched {
                rows_out.push(crate::values::sqlite_row_values(r, names.len()));
            }
            Ok((names, rows_out))
        }
        Db::Mysql(pool) => {
            let rows = crate::sql_history::query(&sql).fetch_all(pool).await?;
            let mut out = Vec::new();
            let names: Vec<String> = rows
                .first()
                .map(|first| first.columns().iter().map(|c| c.name().to_string()).collect())
                .unwrap_or_default();
            for r in &rows {
                out.push(crate::values::my_row_values(r, names.len()));
            }
            Ok((names, out))
        }
        Db::Postgres(pool) => {
            let rows = crate::sql_history::query(&sql).fetch_all(pool).await?;
            let mut out = Vec::new();
            let names: Vec<String> = rows
                .first()
                .map(|first| first.columns().iter().map(|c| c.name().to_string()).collect())
                .unwrap_or_default();
            for r in &rows {
                out.push(crate::values::pg_row_values(r, names.len()));
            }
            Ok((names, out))
        }
            _ => Ok((Vec::new(), Vec::new())),
    }

    }).await
}

// ================================================================ 通用 SQL 实现（ClickHouse / ODBC / SQL Server 等）
// 所有数据都经 db.generic_* 文本化取出，再按方言组织目录查询。


#[path = "schema_namespaces.rs"]
mod namespaces;

/// 列出真实可见目录：查询失败向上传播，成功的空目录仍为空。
async fn generic_list_databases(db: &Db, kind: &DbKind) -> Result<Vec<(String, String)>> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    namespaces::list_with(kind, |sql| db.generic_rows(sql)).await

    }).await
}

/// 列出表与视图：方言优先，其余走 information_schema 兜底。
async fn generic_list_tables(
    db: &Db,
    kind: &DbKind,
    schema: &str,
) -> Result<Vec<(String, ObjectType)>> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let mut out: Vec<(String, ObjectType)> = Vec::new();
    match kind.id() {
        "clickhouse" => {
            let q = if schema.is_empty() {
                "SHOW TABLES".to_string()
            } else {
                format!("SHOW TABLES FROM {}", crate::db::quote_ident(&kind, schema))
            };
            let (_, rows) = db.generic_rows(&q).await?;
            for r in rows {
                if let Some(name) = r.into_iter().next().flatten() {
                    out.push((name, ObjectType::Table));
                }
            }
        }
        "hive" | "inceptor" | "argodb" | "spark" | "impala" | "kyuubi" => {
            let q = if schema.is_empty() {
                "SHOW TABLES".to_string()
            } else {
                format!("SHOW TABLES IN {}", schema)
            };
            let (_, rows) = db.generic_rows(&q).await?;
            for r in rows {
                if let Some(name) = r.into_iter().next().flatten() {
                    out.push((name, ObjectType::Table));
                }
            }
        }
        "sqlserver" | "mssql" => {
            let query = crate::object_identity::sqlserver_list_query(kind, schema)?;
            let (_, rows) = db.generic_rows(&query).await?;
            for row in rows {
                let (Some(namespace), Some(name), Some(ty)) = (row.first().cloned().flatten(), row.get(1).cloned().flatten(), row.get(2).cloned().flatten()) else { continue; };
                out.push((crate::object_identity::sqlserver_object_label(kind, &namespace, &name), if ty == "V" { ObjectType::View } else { ObjectType::Table }));
            }
        }
        "oracle" | "dameng" => {
            // Oracle 风格目录：达梦同样兼容 USER_TABLES / USER_VIEWS
            let (_, rows) = db
                .generic_rows(
                    "SELECT TABLE_NAME, 'TABLE' FROM USER_TABLES \
                     UNION ALL SELECT VIEW_NAME, 'VIEW' FROM USER_VIEWS ORDER BY 1",
                )
                .await?;
            for r in rows {
                let name = r.first().cloned().flatten().unwrap_or_default();
                let ty = r.get(1).cloned().flatten().unwrap_or_default();
                if !name.is_empty() {
                    out.push((
                        name,
                        if ty.eq_ignore_ascii_case("VIEW") {
                            ObjectType::View
                        } else {
                            ObjectType::Table
                        },
                    ));
                }
            }
        }
        "db2" => {
            let (_, rows) = db
                .generic_rows(
                    "SELECT tabname, type FROM syscat.tables \
                     WHERE tabschema = CURRENT SCHEMA ORDER BY tabname",
                )
                .await?;
            for r in rows {
                let name = r.first().cloned().flatten().unwrap_or_default();
                let ty = r.get(1).cloned().flatten().unwrap_or_default();
                if !name.is_empty() {
                    out.push((
                        name,
                        if ty.eq_ignore_ascii_case("V") {
                            ObjectType::View
                        } else {
                            ObjectType::Table
                        },
                    ));
                }
            }
        }
        "teradata" => {
            let (_, rows) = db
                .generic_rows(&format!(
                    "SELECT TableName, TableKind FROM dbc.Tables \
                     WHERE DatabaseName = '{}' ORDER BY TableName",
                    schema.replace('\'', "''")
                ))
                .await?;
            for r in rows {
                let name = r.first().cloned().flatten().unwrap_or_default();
                let ty = r.get(1).cloned().flatten().unwrap_or_default();
                if !name.is_empty() {
                    out.push((
                        name,
                        if ty.eq_ignore_ascii_case("V") {
                            ObjectType::View
                        } else {
                            ObjectType::Table
                        },
                    ));
                }
            }
        }
        _ => {
            // 其余 ODBC 数据源：尝试 information_schema（部分驱动不支持 CURRENT_SCHEMA，退化不过滤）
            let rows = match db
                .generic_rows(
                    "SELECT table_name, table_type FROM information_schema.tables \
                     WHERE table_schema = CURRENT_SCHEMA ORDER BY table_name",
                )
                .await
            {
                Ok((_, rows)) => rows,
                Err(_) => db
                    .generic_rows(
                        "SELECT table_name, table_type FROM information_schema.tables ORDER BY table_name",
                    )
                    .await
                    .map(|(_, rows)| rows)
                    .unwrap_or_default(),
            };
            for r in rows {
                let name = r.first().cloned().flatten().unwrap_or_default();
                let ty = r.get(1).cloned().flatten().unwrap_or_default();
                if !name.is_empty() {
                    out.push((
                        name,
                        if ty.to_ascii_uppercase().contains("VIEW") {
                            ObjectType::View
                        } else {
                            ObjectType::Table
                        },
                    ));
                }
            }
        }
    }
    Ok(out)

    }).await
}

/// 读取表结构：方言优先，其余以 SELECT ... WHERE 1=0 取列名兜底。
async fn generic_table_schema(
    db: &Db,
    kind: &DbKind,
    schema: &str,
    table: &str,
) -> Result<TableSchema> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let mut out = TableSchema {
        table: table.to_string(),
        database: schema.to_string(),
        columns: Vec::new(),
        indexes: Vec::new(),
        foreign_keys: Vec::new(),
        comment: None,
        engine: None,
        collation: None,
        row_count: None,
    };
    match kind.id() {
        "clickhouse" => {
            let full = crate::db::qualified(&kind, schema, table);
            let (cols, rows) = db
                .generic_rows(&format!("DESCRIBE TABLE {}", full))
                .await?;
            let idx = |n: &str| cols.iter().position(|c| c.eq_ignore_ascii_case(n));
            let i_name = idx("name").unwrap_or(0);
            let i_type = idx("type").unwrap_or(1);
            let i_dt = idx("default_type");
            let i_de = idx("default_expression");
            let i_cm = idx("comment");
            for r in &rows {
                let name = r.get(i_name).cloned().flatten().unwrap_or_default();
                let ty = r.get(i_type).cloned().flatten().unwrap_or_default();
                if name.is_empty() {
                    continue;
                }
                let default = match (
                    r.get(i_dt.unwrap_or(2)).cloned().flatten().unwrap_or_default(),
                    i_de.and_then(|i| r.get(i)).cloned().flatten(),
                ) {
                    (dt, Some(de)) if dt.eq_ignore_ascii_case("DEFAULT") => Some(de),
                    _ => None,
                };
                out.columns.push(ColumnInfo {
                    name,
                    data_type: ty
                        .trim_start_matches("Nullable(")
                        .trim_end_matches(')')
                        .to_string(),
                    nullable: ty.starts_with("Nullable("),
                    default,
                    primary_key: false,
                    auto_increment: false,
                    comment: i_cm
                        .and_then(|i| r.get(i))
                        .cloned()
                        .flatten()
                        .filter(|s| !s.is_empty()),
                    length: None,
                    precision: None,
                    scale: None,
                });
            }
            let cnt = db
                .generic_scalar(&format!("SELECT count() FROM {}", full))
                .await?;
            out.row_count = cnt.and_then(|s| s.trim().parse().ok());
        }
        "sqlserver" | "mssql" => {
            let identity = crate::object_identity::SqlServerObject::parse(schema, table)?;
            let (_cols, rows) = db.generic_rows(&identity.columns_query(kind)).await?;
            for r in &rows {
                let name = r.first().cloned().flatten().unwrap_or_default();
                if name.is_empty() {
                    continue;
                }
                out.columns.push(ColumnInfo {
                    name,
                    data_type: r.get(1).cloned().flatten().unwrap_or_default(),
                    nullable: r
                        .get(2)
                        .cloned()
                        .flatten()
                        .map(|v| v.eq_ignore_ascii_case("YES"))
                        .unwrap_or(true),
                    default: r
                        .get(3)
                        .cloned()
                        .flatten()
                        .filter(|s| !s.trim().is_empty()),
                    primary_key: false,
                    auto_increment: false,
                    comment: None,
                    length: None,
                    precision: None,
                    scale: None,
                });
            }
            // 主键
            let (_, pk_rows) = db
                .generic_rows(&identity.primary_key_query(kind))
                .await?;
            let pk_names: Vec<String> = pk_rows
                .iter()
                .filter_map(|r| r.first().cloned().flatten())
                .collect();
            for c in out.columns.iter_mut() {
                if pk_names.iter().any(|p| p.eq_ignore_ascii_case(&c.name)) {
                    c.primary_key = true;
                }
            }
            let full = crate::db::qualified(&kind, schema, table);
            let cnt = db
                .generic_scalar(&format!("SELECT COUNT(*) FROM {}", full))
                .await?;
            out.row_count = cnt.and_then(|s| s.trim().parse().ok());
        }
        "oracle" | "dameng" => {
            let upper = table.to_uppercase().replace('\'', "''");
            let (_cols, rows) = db
                .generic_rows(&format!(
                    "SELECT COLUMN_NAME, DATA_TYPE, NULLABLE, DATA_DEFAULT, CHAR_LENGTH \
                     FROM ALL_TAB_COLUMNS WHERE OWNER = USER AND TABLE_NAME = '{}' \
                     ORDER BY COLUMN_ID",
                    upper
                ))
                .await?;
            for r in &rows {
                let name = r.first().cloned().flatten().unwrap_or_default();
                if name.is_empty() {
                    continue;
                }
                out.columns.push(ColumnInfo {
                    name,
                    data_type: r.get(1).cloned().flatten().unwrap_or_default(),
                    nullable: r
                        .get(2)
                        .cloned()
                        .flatten()
                        .map(|v| v.eq_ignore_ascii_case("Y"))
                        .unwrap_or(true),
                    default: r
                        .get(3)
                        .cloned()
                        .flatten()
                        .filter(|s| !s.trim().is_empty()),
                    primary_key: false,
                    auto_increment: false,
                    comment: None,
                    length: r
                        .get(4)
                        .cloned()
                        .flatten()
                        .and_then(|s| s.trim().parse().ok()),
                    precision: None,
                    scale: None,
                });
            }
            // 主键
            let (_, pk_rows) = db
                .generic_rows(&format!(
                    "SELECT cc.COLUMN_NAME FROM ALL_CONSTRAINTS c \
                     JOIN ALL_CONS_COLUMNS cc ON c.CONSTRAINT_NAME = cc.CONSTRAINT_NAME \
                     AND c.OWNER = cc.OWNER \
                     WHERE c.CONSTRAINT_TYPE = 'P' AND c.OWNER = USER AND c.TABLE_NAME = '{}' \
                     ORDER BY cc.POSITION",
                    upper
                ))
                .await
                .unwrap_or((Vec::new(), Vec::new()));
            let pk_names: Vec<String> = pk_rows
                .iter()
                .filter_map(|r| r.first().cloned().flatten())
                .collect();
            for c in out.columns.iter_mut() {
                if pk_names.iter().any(|p| p.eq_ignore_ascii_case(&c.name)) {
                    c.primary_key = true;
                }
            }
            let full = crate::db::qualified(&kind, schema, table);
            let cnt = db
                .generic_scalar(&format!("SELECT COUNT(*) FROM {}", full))
                .await?;
            out.row_count = cnt.and_then(|s| s.trim().parse().ok());
        }
        "hive" | "inceptor" | "argodb" | "spark" | "impala" | "kyuubi" => {
            let full = crate::db::qualified(&kind, schema, table);
            let (_cols, rows) = db.generic_rows(&format!("DESCRIBE {}", full)).await?;
            for r in &rows {
                let name = r.first().cloned().flatten().unwrap_or_default();
                if name.is_empty() || name.starts_with('#') {
                    continue;
                }
                out.columns.push(ColumnInfo {
                    name,
                    data_type: r.get(1).cloned().flatten().unwrap_or_default(),
                    nullable: true,
                    default: None,
                    primary_key: false,
                    auto_increment: false,
                    comment: r.get(2).cloned().flatten().filter(|s| !s.is_empty()),
                    length: None,
                    precision: None,
                    scale: None,
                });
            }
            let cnt = db
                .generic_scalar(&format!("SELECT COUNT(*) FROM {}", full))
                .await?;
            out.row_count = cnt.and_then(|s| s.trim().parse().ok());
        }
        _ => {
            // 通用兜底：SELECT * WHERE 1=0 只取列名（几乎所有 SQL 数据源都支持）
            let full = crate::db::qualified(&kind, schema, table);
            if let Db::Odbc(odbc) = db {
                // 从已动态验证的 SQLDescribeCol 类型码取结构；主键未知仍保持未知。
                // 否则源 BLOB 虽已正确读成 HEX，反向同步仍会错误创建 TEXT。
                let result = match odbc.query_rows(&format!("SELECT * FROM {} WHERE 1 = 0", full)).await {
                    Ok(result) => result,
                    Err(_) => odbc.query_rows(&format!("SELECT * FROM {}", full)).await?,
                };
                out.columns = result.column_info;
            } else {
                let (cols, _rows) = match db
                    .generic_rows(&format!("SELECT * FROM {} WHERE 1 = 0", full))
                    .await
                {
                    Ok(x) => x,
                    Err(_) => match db.generic_rows(&format!("SELECT * FROM {}", full)).await {
                        Ok((c, _)) => (c, Vec::new()),
                        Err(e) => return Err(e),
                    },
                };
                for name in cols {
                    out.columns.push(ColumnInfo {
                        name,
                        data_type: String::new(),
                        nullable: true,
                        default: None,
                        primary_key: false,
                        auto_increment: false,
                        comment: None,
                        length: None,
                        precision: None,
                        scale: None,
                    });
                }
            }
            let cnt = db
                .generic_scalar(&format!("SELECT COUNT(*) FROM {}", full))
                .await?;
            out.row_count = cnt.and_then(|s| s.trim().parse().ok());
        }
    }
    Ok(out)

    }).await
}

/// 分页读取表数据。
async fn generic_page_table(db: &Db, kind: &DbKind, req: &PageRequest) -> Result<PageResult> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if kind.protocol() == crate::models::Protocol::Mssql {
        crate::object_identity::SqlServerObject::parse(&req.schema, &req.table)?;
    }
    let full = crate::db::qualified(&kind, &req.schema, &req.table);
    let where_sql = sensitive_table_filter::typed_clause(db, kind, req).await?;
    let order_sql = order_clause(kind, req);
    let total: u64 = db
        .generic_scalar(&format!("SELECT COUNT(*) FROM {}{}", full, where_sql))
        .await?
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let limit = req.limit.clamp(1, 100_000);
    let hive_family = matches!(
        kind.id(),
        "hive" | "inceptor" | "argodb" | "spark" | "impala" | "kyuubi"
    );
    // Access and InterSystems TOP has no OFFSET; fetch through offset + limit and
    // skip only after the database has applied the requested filters and ordering.
    let top_family = matches!(kind.id(), "access" | "iris" | "cache");
    let rows_sql = if req.limit == 0 {
        // Keep actual column metadata while returning no records, including dialects
        // where FETCH NEXT 0 / FIRST 0 is invalid. Do not silently turn zero into one.
        let empty_where = if where_sql.is_empty() { " WHERE 1=0".to_string() } else { format!("{} AND 1=0", where_sql) };
        format!("SELECT * FROM {}{}", full, empty_where)
    } else if matches!(kind.id(), "sqlserver" | "mssql" | "oracle" | "dameng" | "db2") {
        let order = if order_sql.is_empty() && matches!(kind.id(), "sqlserver" | "mssql") {
            " ORDER BY (SELECT NULL)"
        } else {
            &order_sql
        };
        format!(
            "SELECT * FROM {}{}{} OFFSET {} ROWS FETCH NEXT {} ROWS ONLY",
            full, where_sql, order, req.offset, limit
        )
    } else if kind.id() == "firebird" {
        format!("SELECT FIRST {} SKIP {} * FROM {}{}{}", limit, req.offset, full, where_sql, order_sql)
    } else if kind.id() == "informix" {
        format!("SELECT SKIP {} FIRST {} * FROM {}{}{}", req.offset, limit, full, where_sql, order_sql)
    } else if top_family {
        format!("SELECT TOP {} * FROM {}{}{}", req.offset.saturating_add(limit), full, where_sql, order_sql)
    } else if hive_family {
        // HiveQL 无 OFFSET：排序/筛选在取行之前执行，再跳过 offset 行。
        format!("SELECT * FROM {}{}{} LIMIT {}", full, where_sql, order_sql, req.offset.saturating_add(limit))
    } else {
        format!(
            "SELECT * FROM {}{}{} LIMIT {} OFFSET {}",
            full, where_sql, order_sql, limit, req.offset
        )
    };
    let (cols, mut rows) = db.generic_rows(&rows_sql).await?;
    if (hive_family || top_family) && req.offset > 0 {
        rows = rows.into_iter().skip(req.offset as usize).collect();
    }
    let mssql_columns = if kind.protocol() == crate::models::Protocol::Mssql {
        let identity = crate::object_identity::SqlServerObject::parse(&req.schema, &req.table)?;
        let (_, metadata) = db.generic_rows(&identity.columns_query(kind)).await?;
        metadata.into_iter().map(|row| {
            let name = row.first().cloned().flatten().ok_or_else(|| anyhow::anyhow!("SQL Server 列目录缺少名称"))?;
            let data_type = row.get(1).cloned().flatten().filter(|ty| !ty.is_empty()).ok_or_else(|| anyhow::anyhow!("SQL Server 列目录缺少真实类型"))?;
            let nullable = row.get(2).and_then(Option::as_deref).map(|value| value.eq_ignore_ascii_case("YES")).unwrap_or(true);
            Ok(ResultColumn { name, data_type, nullable, primary_key: false, auto_increment: false })
        }).collect::<Result<Vec<_>>>()?
    } else { Vec::new() };
    let columns = cols
        .into_iter()
        .map(|c| {
            if kind.protocol() == crate::models::Protocol::Mssql {
                return mssql_columns.iter().find(|column| column.name == c).cloned().ok_or_else(|| anyhow::anyhow!("SQL Server 结果列与目录不一致：{}", c));
            }
            Ok(ResultColumn { name: c, data_type: String::new(), nullable: true, primary_key: false, auto_increment: false })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(PageResult {
        columns,
        rows,
        total,
        primary_keys: Vec::new(),
        elapsed_ms: 0,
    })

    }).await
}

/// 视图定义。
async fn generic_view_definition(
    db: &Db,
    kind: &DbKind,
    schema: &str,
    view: &str,
) -> Result<String> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    match kind.id() {
        "clickhouse" => {
            let full = crate::db::qualified(&kind, schema, view);
            let (_, rows) = db
                .generic_rows(&format!("SHOW CREATE TABLE {}", full))
                .await?;
            rows.first()
                .and_then(|r| r.first().cloned().flatten())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("视图不存在"))
        }
        "hive" | "inceptor" | "argodb" | "spark" | "impala" | "kyuubi" => {
            let full = crate::db::qualified(&kind, schema, view);
            let (_, rows) = db
                .generic_rows(&format!("SHOW CREATE TABLE {}", full))
                .await?;
            let text = rows
                .iter()
                .filter_map(|r| r.first().cloned().flatten())
                .collect::<Vec<_>>()
                .join("\r\n");
            if text.is_empty() {
                Err(anyhow::anyhow!("视图不存在"))
            } else {
                Ok(text)
            }
        }
        "sqlserver" | "mssql" => {
            let identity = crate::object_identity::SqlServerObject::parse(schema, view)?;
            let (_, rows) = db.generic_rows(&identity.view_query(kind)).await?;
            rows.first()
                .and_then(|r| r.first().cloned().flatten())
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| anyhow::anyhow!("视图不存在"))
        }
        "dameng" => {
            let upper = view.to_uppercase().replace('\'', "''");
            let (_, rows) = db
                .generic_rows(&format!(
                    "SELECT TEXT FROM ALL_VIEWS WHERE VIEW_NAME = '{}'",
                    upper
                ))
                .await?;
            let text = rows
                .first()
                .and_then(|r| r.first().cloned().flatten())
                .unwrap_or_default();
            if text.is_empty() {
                Err(anyhow::anyhow!("视图不存在"))
            } else {
                Ok(format!(
                    "CREATE VIEW {} AS {}",
                    crate::db::quote_ident(&kind, view),
                    text
                ))
            }
        }
        _ => Err(anyhow::anyhow!("该数据源类型不支持视图定义查看")),
    }

    }).await
}

// ================================================================ HTTP / Redis / MongoDB 树与数据页适配

/// HTTP 类数据源：优先用适配器真实列元数据（如 JDBC），否则由前几行数据推导。
async fn http_table_schema(
    h: &crate::http_db::HttpDb,
    kind: DbKind,
    schema: &str,
    table: &str,
) -> Result<TableSchema> {
    let mut out = TableSchema {
        table: table.to_string(),
        database: schema.to_string(),
        columns: Vec::new(),
        indexes: Vec::new(),
        foreign_keys: Vec::new(),
        comment: None,
        engine: None,
        collation: None,
        row_count: None,
    };
    if let Some(columns) = h.table_columns(schema, table).await? {
        for col in columns {
            out.columns.push(ColumnInfo {
                name: col.name,
                data_type: col.data_type,
                nullable: col.nullable,
                default: None,
                primary_key: col.primary_key,
                auto_increment: false,
                comment: None,
                length: None,
                precision: None,
                scale: None,
            });
        }
    } else {
        let r = h.rows(schema, table, 0, 20).await?;
        for name in r.columns {
            out.columns.push(ColumnInfo {
                name,
                data_type: "string".to_string(),
                nullable: true,
                default: None,
                primary_key: false,
                auto_increment: false,
                comment: None,
                length: None,
                precision: None,
                scale: None,
            });
        }
    }
    let _ = kind;
    Ok(out)
}

/// HTTP 类数据源：分页浏览。
async fn http_page_table(h: &crate::http_db::HttpDb, req: &PageRequest) -> Result<PageResult> {
    let r = h.rows(&req.schema, &req.table, req.offset, req.limit).await?;
    let columns: Vec<ResultColumn> = r
        .columns
        .into_iter()
        .map(|c| ResultColumn {
            name: c,
            data_type: "string".to_string(),
            nullable: true,
            primary_key: false,
            auto_increment: false,
        })
        .collect();
    // 总数未知：整页填满时多给 1 条额度以启用「下一页」
    let n = r.rows.len() as u64;
    let more = if n >= req.limit { 1 } else { 0 };
    Ok(PageResult {
        columns,
        rows: r.rows,
        total: req.offset + n + more,
        primary_keys: Vec::new(),
        elapsed_ms: 0,
    })
}

/// Redis：键详情作为「表结构」。
async fn redis_table_schema(
    r: &crate::redis_db::RedisDb,
    kind: DbKind,
    schema: &str,
    table: &str,
) -> Result<TableSchema> {
    let dbn = schema
        .strip_prefix("db")
        .and_then(|n| n.parse::<i64>().ok())
        .unwrap_or(0);
    let (ty, ttl, value) = r.key_info(dbn, table).await?;
    let mut out = TableSchema {
        table: table.to_string(),
        database: schema.to_string(),
        columns: Vec::new(),
        indexes: Vec::new(),
        foreign_keys: Vec::new(),
        comment: None,
        engine: None,
        collation: None,
        row_count: Some(1),
    };
    for name in ["key", "type", "ttl", "value"] {
        out.columns.push(ColumnInfo {
            name: name.to_string(),
            data_type: if name == "ttl" { "int".to_string() } else { "string".to_string() },
            nullable: false,
            default: None,
            primary_key: name == "key",
            auto_increment: false,
            comment: None,
            length: None,
            precision: None,
            scale: None,
        });
    }
    out.comment = Some(format!("type={} ttl={} value={} 字符", ty, ttl, value.chars().count()));
    let _ = kind;
    Ok(out)
}

/// Redis：单键数据页。
async fn redis_key_page(r: &crate::redis_db::RedisDb, req: &PageRequest) -> Result<PageResult> {
    let dbn = req
        .schema
        .strip_prefix("db")
        .and_then(|n| n.parse::<i64>().ok())
        .unwrap_or(0);
    let (ty, ttl, value) = r.key_info(dbn, &req.table).await?;
    Ok(PageResult {
        columns: vec![
            ResultColumn {
                name: "key".into(),
                data_type: "string".into(),
                nullable: false,
                primary_key: true,
                auto_increment: false,
            },
            ResultColumn {
                name: "type".into(),
                data_type: "string".into(),
                nullable: false,
                primary_key: false,
                auto_increment: false,
            },
            ResultColumn {
                name: "ttl".into(),
                data_type: "int".into(),
                nullable: true,
                primary_key: false,
                auto_increment: false,
            },
            ResultColumn {
                name: "value".into(),
                data_type: "string".into(),
                nullable: true,
                primary_key: false,
                auto_increment: false,
            },
        ],
        rows: vec![vec![
            Some(req.table.clone()),
            Some(ty),
            Some(ttl.to_string()),
            Some(value),
        ]],
        total: 1,
        primary_keys: vec!["key".into()],
        elapsed_ms: 0,
    })
}

/// MongoDB：由文档页推导表结构。
async fn mongo_table_schema(
    m: &crate::mongo_db::MongoDb,
    kind: DbKind,
    schema: &str,
    table: &str,
) -> Result<TableSchema> {
    let (cols, _) = m.find_docs(schema, table, "", 0, 20).await?;
    let mut out = TableSchema {
        table: table.to_string(),
        database: schema.to_string(),
        columns: Vec::new(),
        indexes: Vec::new(),
        foreign_keys: Vec::new(),
        comment: None,
        engine: None,
        collation: None,
        row_count: None,
    };
    for name in cols {
        let pk = name == "_id";
        out.columns.push(ColumnInfo {
            name,
            data_type: "string".to_string(),
            nullable: true,
            default: None,
            primary_key: pk,
            auto_increment: false,
            comment: None,
            length: None,
            precision: None,
            scale: None,
        });
    }
    let cnt = m.count(schema, table, "").await?;
    out.row_count = Some(cnt as i64);
    let _ = kind;
    Ok(out)
}

/// MongoDB：文档分页。
async fn mongo_doc_page(m: &crate::mongo_db::MongoDb, req: &PageRequest) -> Result<PageResult> {
    // MongoDB interprets a wire limit of zero as unlimited. Sample at most one document
    // for inferred columns on an empty UI page, then discard that bounded sample.
    let query_limit = if req.limit == 0 { 1 } else { req.limit };
    let (cols, mut rows) = m
        .find_docs(&req.schema, &req.table, "", req.offset, query_limit)
        .await?;
    if req.limit == 0 { rows.clear(); }
    let total = m.count(&req.schema, &req.table, "").await?;
    let columns: Vec<ResultColumn> = cols
        .into_iter()
        .map(|c| ResultColumn {
            name: c,
            data_type: "string".to_string(),
            nullable: true,
            primary_key: false,
            auto_increment: false,
        })
        .collect();
    Ok(PageResult {
        columns,
        rows,
        total,
        primary_keys: vec!["_id".into()],
        elapsed_ms: 0,
    })
}

#[cfg(test)]
mod filter_sort_tests {
    use super::*;
    use crate::models::{FilterCondition, FilterLine, FilterLogic, SortRule};

    fn filter(column: &str, operator: &str, value: &str, case_sensitive: bool) -> FilterCondition {
        FilterCondition {
            column: column.into(),
            operator: operator.into(),
            value: value.into(),
            case_sensitive,
            logic: None,
        }
    }

    /// 单条件行（最常见：一行一条）。
    fn one(c: FilterCondition) -> FilterLine {
        FilterLine { logic: FilterLogic::And, conds: vec![c] }
    }

    fn request() -> PageRequest {
        serde_json::from_value(serde_json::json!({
            "connection_id": "test", "schema": "", "table": "items", "offset": 0, "limit": 20,
            "order_by": "id", "ascending": true
        })).unwrap()
    }

    #[test]
    fn compatible_request_defaults_and_explicit_clear() {
        let mut req = request();
        assert_eq!(req.filter_logic, FilterLogic::And);
        assert!(req.order_rules.is_none());
        // 旧请求（无逐条 logic）仍可反序列化，行内连接方式缺省为 None
        assert!(serde_json::from_value::<FilterCondition>(serde_json::json!({
            "column": "a", "operator": "=", "value": "1"
        })).unwrap().logic.is_none());
        // 条件行缺省 logic = AND
        assert_eq!(serde_json::from_value::<FilterLine>(serde_json::json!({
            "conds": [{ "column": "a", "operator": "=", "value": "1" }]
        })).unwrap().logic, FilterLogic::And);
        assert_eq!(order_clause(&DbKind::of("sqlite"), &req), " ORDER BY \"id\" ASC");
        req.order_rules = Some(vec![]);
        assert_eq!(order_clause(&DbKind::of("sqlite"), &req), "");
        assert!(serde_json::from_value::<FilterLogic>(serde_json::json!("OR 1=1")).is_err());
    }

    #[test]
    fn line_groups_follow_sql_bracket_text() {
        let sqlite = DbKind::of("sqlite");
        // 行与行默认 AND 连接
        let lines = vec![one(filter("a", "=", "1", false)), one(filter("b", "=", "2", false))];
        assert_eq!(filter_clause(&sqlite, &lines).unwrap(),
            " WHERE (\"a\" COLLATE NOCASE = '1' AND \"b\" COLLATE NOCASE = '2')");
        // 行间 OR
        let or_lines = vec![
            one(filter("a", "=", "1", false)),
            FilterLine { logic: FilterLogic::Or, conds: vec![filter("b", "=", "2", false)] },
        ];
        assert_eq!(filter_clause(&sqlite, &or_lines).unwrap(),
            " WHERE (\"a\" COLLATE NOCASE = '1' OR \"b\" COLLATE NOCASE = '2')");
        // 用户示例：a = 1 AND (b = 2 OR c = 3) AND d = 4 → 三行，中一行两条 OR 括起
        let group = vec![
            one(filter("a", "=", "1", false)),
            FilterLine {
                logic: FilterLogic::And,
                conds: vec![
                    filter("b", "=", "2", false),
                    FilterCondition { logic: Some(FilterLogic::Or), ..filter("c", "=", "3", false) },
                ],
            },
            one(filter("d", "=", "4", false)),
        ];
        assert_eq!(filter_clause(&sqlite, &group).unwrap(),
            " WHERE (\"a\" COLLATE NOCASE = '1' AND (\"b\" COLLATE NOCASE = '2' OR \"c\" COLLATE NOCASE = '3') AND \"d\" COLLATE NOCASE = '4')");
        // 行内 AND 也可以：(b AND c) OR d
        let group_and = vec![
            FilterLine {
                logic: FilterLogic::And,
                conds: vec![
                    filter("b", "=", "2", false),
                    FilterCondition { logic: Some(FilterLogic::And), ..filter("c", "=", "3", false) },
                ],
            },
            FilterLine { logic: FilterLogic::Or, conds: vec![filter("d", "=", "4", false)] },
        ];
        assert_eq!(filter_clause(&sqlite, &group_and).unwrap(),
            " WHERE ((\"b\" COLLATE NOCASE = '2' AND \"c\" COLLATE NOCASE = '3') OR \"d\" COLLATE NOCASE = '4')");
        // 行内 logic 缺省回退 AND
        let legacy = vec![FilterLine {
            logic: FilterLogic::And,
            conds: vec![filter("a", "=", "1", false), filter("b", "=", "2", false)],
        }];
        assert_eq!(filter_clause(&sqlite, &legacy).unwrap(),
            " WHERE (\"a\" COLLATE NOCASE = '1' AND \"b\" COLLATE NOCASE = '2')");
    }

    #[test]
    fn empty_lines_are_skipped() {
        let sqlite = DbKind::of("sqlite");
        let lines = vec![
            FilterLine { logic: FilterLogic::And, conds: vec![] },
            one(filter("a", "=", "1", false)),
        ];
        assert_eq!(filter_clause(&sqlite, &lines).unwrap(), " WHERE (\"a\" COLLATE NOCASE = '1')");
        let all_empty = vec![FilterLine { logic: FilterLogic::And, conds: vec![] }];
        assert_eq!(filter_clause(&sqlite, &all_empty).unwrap(), "");
    }

    #[test]
    fn sql_dialects_and_safe_quoting() {
        let pg = DbKind::of("postgresql");
        // 目录实际 PostgreSQL id 为 postgresql 或 postgres；协议通过目录决定。
        let pg = if pg.is_pg_family() { pg } else { DbKind::of("postgres") };
        let conditions = vec![FilterLine {
            logic: FilterLogic::And,
            conds: vec![
                filter("name", "LIKE", "Ab%", false),
                FilterCondition { logic: Some(FilterLogic::Or), ..filter("note", "is null", "ignored", false) },
            ],
        }];
        let sql = filter_clause(&pg, &conditions).unwrap();
        assert!(sql.contains(" ILIKE 'Ab%' ESCAPE '\\' OR \"note\" IS NULL"), "{sql}");
        assert!(sql.starts_with(" WHERE (") && sql.ends_with(')'));
        let mysql = filter_clause(&DbKind::of("mysql"), &[one(filter("na`me", "=", "O'Reilly", true))]).unwrap();
        assert_eq!(mysql, " WHERE (CONVERT(`na``me` USING utf8mb4) COLLATE utf8mb4_bin = 'O''Reilly')");
        assert!(filter_clause(&pg, &[one(filter("name", "= 1 OR", "x", false))]).is_err());
        let mut req = request();
        req.order_rules = Some(vec![SortRule { column: "group".into(), ascending: false }, SortRule { column: "id\"quoted".into(), ascending: true }]);
        assert_eq!(order_clause(&DbKind::of("sqlite"), &req), " ORDER BY \"group\" DESC, \"id\"\"quoted\" ASC");
    }

    #[test]
    fn mysql_table_status_uses_database_identifier_not_missing_db_column() {
        assert_eq!(mysql_table_status_sql(&DbKind::of("mysql"), "db`name", "O'Reilly"),
            "SHOW TABLE STATUS FROM `db``name` WHERE Name = 'O''Reilly'");
    }

    async fn fixture() -> Db {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        crate::sql_history::query("CREATE TABLE items (id INTEGER PRIMARY KEY, team TEXT, score INTEGER, name TEXT COLLATE NOCASE, note TEXT)").execute(&pool).await.unwrap();
        crate::sql_history::query("INSERT INTO items VALUES (1,'a',1,'Abc',NULL),(2,'a',2,'abc','x'),(3,'b',1,'a*c','x'),(4,'a',2,'','x')").execute(&pool).await.unwrap();
        Db::Sqlite(pool)
    }

    #[tokio::test]
    async fn sqlite_and_or_multi_sort_count_and_pagination() {
        let db = fixture().await;
        let mut req = request();
        // 单行两条 AND：team='b' AND score>1 → 空
        req.filters = vec![FilterLine {
            logic: FilterLogic::And,
            conds: vec![
                filter("team", "=", "b", false),
                FilterCondition { logic: Some(FilterLogic::And), ..filter("score", ">", "1", false) },
            ],
        }];
        let and = page_table(&db, DbKind::of("sqlite"), &req).await.unwrap();
        assert_eq!(and.total, 0);
        assert_eq!(and.columns.len(), 5, "零结果仍有列结构");
        // 两行 OR：team='b' OR score>1 → 3 条
        req.filters = vec![
            one(filter("team", "=", "b", false)),
            FilterLine { logic: FilterLogic::Or, conds: vec![filter("score", ">", "1", false)] },
        ];
        req.order_rules = Some(vec![SortRule { column: "score".into(), ascending: false }, SortRule { column: "id".into(), ascending: false }]);
        req.limit = 2;
        let first = page_table(&db, DbKind::of("sqlite"), &req).await.unwrap();
        assert_eq!(first.total, 3);
        assert_eq!(first.rows.iter().map(|r| r[0].as_deref().unwrap()).collect::<Vec<_>>(), vec!["4", "2"]);
        req.offset = 2;
        let next = page_table(&db, DbKind::of("sqlite"), &req).await.unwrap();
        assert_eq!(next.total, 3);
        assert_eq!(next.rows[0][0].as_deref(), Some("3"));
    }

    #[tokio::test]
    async fn sqlite_case_sensitive_like_equality_not_like_null_and_empty() {
        let db = fixture().await;
        let mut req = request();
        for (op, value, sensitive, expected) in [
            ("like", "Ab%", true, 1), ("like", "Ab%", false, 2),
            ("=", "Abc", true, 1), ("=", "Abc", false, 2),
            ("like", "a*c", true, 1), ("not like", "Ab%", true, 3),
            ("=", "", false, 1), ("is null", "ignored", false, 0),
        ] {
            req.filters = vec![one(filter("name", op, value, sensitive))];
            let result = page_table(&db, DbKind::of("sqlite"), &req).await.unwrap();
            assert_eq!(result.total, expected, "{op} {value} sensitive={sensitive}");
        }
        req.filters = vec![one(filter("note", "is null", "", false))];
        assert_eq!(page_table(&db, DbKind::of("sqlite"), &req).await.unwrap().total, 1);
    }

    /// Run explicitly with NAVKIT_TEST_PG_URL; saved product connections are never used.
    #[tokio::test]
    #[ignore = "requires an explicitly supplied isolated PostgreSQL test connection"]
    async fn real_pg_catalog_classifies_internal_char_and_column_flags() -> Result<()> {
        let url = std::env::var("NAVKIT_TEST_PG_URL")
            .map_err(|_| anyhow!("set NAVKIT_TEST_PG_URL to an isolated PostgreSQL test connection"))?;
        let pool = sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(&url).await?;
        let kind = DbKind::of("postgresql");
        let kind = if kind.is_pg_family() { kind } else { DbKind::of("postgres") };
        let schema = format!("navkit_catalog_test_{}", uuid::Uuid::new_v4().simple());
        let quoted = quote_ident(&kind, &schema);
        // Exactly one pool connection keeps isolated DDL and catalog reads in the same transaction.
        crate::sql_history::query("BEGIN").execute(&pool).await?;
        let db = Db::Postgres(pool.clone());
        let result: Result<()> = async {
            crate::sql_history::query(&format!("CREATE SCHEMA {quoted}")).execute(&pool).await?;
            crate::sql_history::query(&format!("CREATE TABLE {quoted}.table_fixture (id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY, serial_id serial, required text NOT NULL, optional text, computed integer GENERATED ALWAYS AS (id + 1) STORED)"))
                .execute(&pool).await?;
            crate::sql_history::query(&format!("CREATE TABLE {quoted}.partition_fixture (id integer) PARTITION BY RANGE (id)"))
                .execute(&pool).await?;
            crate::sql_history::query(&format!("CREATE VIEW {quoted}.view_fixture AS SELECT id FROM {quoted}.table_fixture"))
                .execute(&pool).await?;
            crate::sql_history::query(&format!("CREATE MATERIALIZED VIEW {quoted}.materialized_fixture AS SELECT id FROM {quoted}.table_fixture"))
                .execute(&pool).await?;
            crate::sql_history::query(&format!("CREATE FUNCTION {quoted}.function_fixture() RETURNS integer LANGUAGE sql AS 'SELECT 1'"))
                .execute(&pool).await?;
            crate::sql_history::query(&format!("CREATE PROCEDURE {quoted}.procedure_fixture() LANGUAGE sql AS 'SELECT 1'"))
                .execute(&pool).await?;
            let tables = list_tables(&db, kind.clone(), &schema).await?;
            for (name, expected) in [("table_fixture", ObjectType::Table), ("partition_fixture", ObjectType::Table),
                ("view_fixture", ObjectType::View), ("materialized_fixture", ObjectType::View)] {
                anyhow::ensure!(tables.iter().any(|(n, t)| n == name && *t == expected), "list_tables misclassified {name}: {tables:?}");
            }
            let objects = list_objects(&db, kind.clone(), &schema).await?;
            anyhow::ensure!(objects.iter().any(|o| o.name == "table_fixture" && o.obj_type == ObjectType::Table), "list_objects misclassified table");
            anyhow::ensure!(objects.iter().any(|o| o.name == "view_fixture" && o.obj_type == ObjectType::View), "list_objects misclassified view");
            let routines = list_routines(&db, kind.clone(), &schema).await?;
            anyhow::ensure!(routines.iter().any(|(t, n)| n == "function_fixture" && *t == ObjectType::Function), "list_routines misclassified function: {routines:?}");
            anyhow::ensure!(routines.iter().any(|(t, n)| n == "procedure_fixture" && *t == ObjectType::Procedure), "list_routines misclassified procedure: {routines:?}");
            let meta = table_schema(&db, kind.clone(), &schema, "table_fixture").await?;
            for name in ["id", "serial_id"] {
                anyhow::ensure!(meta.columns.iter().any(|c| c.name == name && c.auto_increment), "{name} must be auto_increment");
            }
            anyhow::ensure!(meta.columns.iter().any(|c| c.name == "computed" && !c.auto_increment), "generated expression must not be auto_increment");
            anyhow::ensure!(meta.columns.iter().any(|c| c.name == "required" && !c.nullable), "NOT NULL must remain non-nullable");
            anyhow::ensure!(meta.columns.iter().any(|c| c.name == "optional" && c.nullable), "nullable column must remain nullable");
            Ok(())
        }.await;
        // Cleanup runs on both success and assertion/query error; verify the UUID schema vanished.
        crate::sql_history::query("ROLLBACK").execute(&pool).await?;
        let exists: bool = crate::sql_history::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = $1)")
            .bind(&schema).fetch_one(&pool).await?;
        anyhow::ensure!(!exists, "temporary test schema leaked after rollback: {schema}");
        pool.close().await;
        result
    }

    #[tokio::test]
    async fn sqlite_text_and_numeric_affinity_survive_case_collation_and_mixed_logic() -> Result<()> {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await?;
        crate::sql_history::query("CREATE TABLE items (id INTEGER PRIMARY KEY, amount INTEGER, code TEXT, label TEXT COLLATE NOCASE)")
            .execute(&pool).await?;
        crate::sql_history::query("INSERT INTO items VALUES (1,1,'01','Abc'),(2,2,'1','abc'),(3,10,'10','Other')")
            .execute(&pool).await?;
        let db = Db::Sqlite(pool);
        let mut req = request();
        for (column, op, value, sensitive, expected_ids) in [
            ("amount", "=", "01", false, vec!["1"]),
            ("amount", "=", "1.0", true, vec!["1"]),
            ("amount", ">", "2", false, vec!["3"]),
            ("code", "=", "01", false, vec!["1"]),
            ("code", "=", "1", true, vec!["2"]),
            ("label", "=", "Abc", false, vec!["1", "2"]),
            ("label", "=", "Abc", true, vec!["1"]),
        ] {
            req.filters = vec![one(filter(column, op, value, sensitive))];
            let result = page_table(&db, DbKind::of("sqlite"), &req).await?;
            let ids: Vec<&str> = result.rows.iter().map(|r| r[0].as_deref().unwrap()).collect();
            anyhow::ensure!(ids == expected_ids, "{column} {op} {value} sensitive={sensitive}: {ids:?}");
        }
        let mut text = filter("label", "=", "ABC", false);
        text.logic = Some(FilterLogic::And);
        // (code='01' AND label='ABC') OR amount>2 → 行 1、3
        req.filters = vec![
            FilterLine { logic: FilterLogic::And, conds: vec![filter("code", "=", "01", false), text] },
            FilterLine { logic: FilterLogic::Or, conds: vec![filter("amount", ">", "2", false)] },
        ];
        let result = page_table(&db, DbKind::of("sqlite"), &req).await?;
        anyhow::ensure!(result.total == 2 && result.rows[0][0].as_deref() == Some("1") && result.rows[1][0].as_deref() == Some("3"), "mixed text/numeric groups did not retain affinity");
        Ok(())
    }

    #[tokio::test]
    async fn sqlite_line_groups_match_sql_semantics() {
        let db = fixture().await;
        let mut req = request();
        // team='a' AND (score>1 OR note IS NULL) → (id 2,4) OR (id 1) → 1,2,4
        let mut or_note = filter("note", "is null", "", false);
        or_note.logic = Some(FilterLogic::Or);
        req.filters = vec![
            one(filter("team", "=", "a", false)),
            FilterLine { logic: FilterLogic::And, conds: vec![filter("score", ">", "1", false), or_note] },
        ];
        let result = page_table(&db, DbKind::of("sqlite"), &req).await.unwrap();
        assert_eq!(result.total, 3);
        assert_eq!(result.rows.iter().map(|r| r[0].as_deref().unwrap()).collect::<Vec<_>>(), vec!["1", "2", "4"]);
        // 行内两条改成 AND 连接 → team='a' AND (score>1 AND note IS NULL) → 空
        req.filters[1].conds[1].logic = Some(FilterLogic::And);
        let result = page_table(&db, DbKind::of("sqlite"), &req).await.unwrap();
        assert_eq!(result.total, 0);
    }
}
