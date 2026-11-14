//! 安全批量生成写入。值一律参数绑定；表/字段须匹配真实元数据；一个批次一个事务。
//! lib.rs 接线：mod data_generator; generate_handler![..., data_generator::insert_generated_rows]
use std::collections::HashSet;
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use tauri::State;
use crate::{commands::AppState, db::{self, Db}, error::{AppError, CmdResult}, models::{DbKind, TableSchema}, schema, storage};

pub const MAX_ROWS: usize = 500;
/// DEFAULT_GENERATED is a writable default expression, not a generated column.
/// Match the complete MySQL/MariaDB generated-storage markers, not substrings.
pub(crate) fn mysql_generated_column(extra: &str) -> bool {
    let words: Vec<_> = extra.split_ascii_whitespace().collect();
    words.windows(2).any(|pair| ["VIRTUAL", "STORED", "PERSISTENT"].iter().any(|storage| pair[0].eq_ignore_ascii_case(storage)) && pair[1].eq_ignore_ascii_case("GENERATED"))
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GeneratedRowsRequest {
    pub connection_id: String,
    pub schema: String,
    pub table: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
}

pub(crate) fn validate_shape(req: &GeneratedRowsRequest) -> Result<()> {
    if req.table.is_empty() || req.table.contains('\0') || req.schema.contains('\0') { bail!("表/模式名称无效"); }
    if req.rows.is_empty() || req.rows.len() > MAX_ROWS { bail!("每批只允许 1–{} 行", MAX_ROWS); }
    if req.columns.len() > 128 { bail!("每批最多 128 个字段"); }
    let mut columns = HashSet::new();
    if req.columns.iter().any(|c| c.is_empty() || c.contains('\0') || !columns.insert(c)) { bail!("字段名称为空、无效或重复"); }
    let mut bytes = 0usize;
    for row in &req.rows {
        if row.len() != req.columns.len() { bail!("行值数量与字段数量不一致"); }
        for value in row.iter().flatten() {
            if value.len() > 16384 { bail!("单个值不能超过 16 KB"); }
            bytes = bytes.saturating_add(value.len());
        }
    }
    if bytes > 2 * 1024 * 1024 { bail!("每批数据不能超过 2 MB"); }
    Ok(())
}
pub(crate) fn validate_columns(req: &GeneratedRowsRequest, info: &TableSchema) -> Result<()> {
    if info.columns.is_empty() { bail!("没有读取到真实表结构"); }
    for name in &req.columns {
        let col = info.columns.iter().find(|c| &c.name == name).ok_or_else(|| anyhow!("字段 {} 不属于目标表", name))?;
        if col.auto_increment { bail!("自增/标识字段 {} 必须交给数据库生成", name); }
    }
    for col in &info.columns {
        if let Some(index) = req.columns.iter().position(|c| c == &col.name) {
            if (!col.nullable || col.primary_key) && req.rows.iter().any(|r| r[index].is_none()) { bail!("字段 {} 不允许 NULL", col.name); }
        } else if !col.auto_increment && col.default.is_none() && (!col.nullable || col.primary_key) {
            bail!("缺少非空/主键字段 {}（无数据库默认值）", col.name);
        }
    }
    // 只检查完整、已提供的主键；既有键/唯一索引/外键/CHECK 交给真实数据库事务。
    let primary: Vec<usize> = info.columns.iter().filter(|c| c.primary_key).filter_map(|c| req.columns.iter().position(|n| n == &c.name)).collect();
    if !primary.is_empty() && primary.len() == info.columns.iter().filter(|c| c.primary_key).count() {
        let mut keys = HashSet::new();
        for row in &req.rows {
            let key: Vec<_> = primary.iter().map(|i| row[*i].clone()).collect();
            if !keys.insert(key) { bail!("批次内有重复主键"); }
        }
    }
    Ok(())
}
pub(crate) fn insert_sql(kind: &DbKind, req: &GeneratedRowsRequest, pg_types: &[String]) -> String {
    let full = db::qualified(kind, &req.schema, &req.table);
    if req.columns.is_empty() {
        return if kind.is_mysql_family() { format!("INSERT INTO {} () VALUES ()", full) } else { format!("INSERT INTO {} DEFAULT VALUES", full) };
    }
    let columns = req.columns.iter().map(|n| db::quote_ident(kind, n)).collect::<Vec<_>>().join(", ");
    let values = req.columns.iter().enumerate().map(|(i, _)| if kind.is_pg_family() {
        // 显式 text 再转换为数据库返回的真实类型；类型的模式/名字也引用，禁止 SQL 注入。
        format!("${}::text::{}", i + 1, pg_types[i])
    } else { "?".into() }).collect::<Vec<_>>().join(", ");
    format!("INSERT INTO {} ({}) VALUES ({})", full, columns, values)
}

pub async fn insert_batch(db: &Db, kind: &DbKind, req: &GeneratedRowsRequest) -> Result<u64> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    validate_shape(req)?;
    crate::sync::validate_sqlite_namespace(db, &req.schema, &req.table).await?;
    if !matches!(db, Db::Sqlite(_) | Db::Mysql(_) | Db::Postgres(_)) { bail!("该协议尚不支持安全事务批量生成"); }
    let mut info = schema::table_schema(db, kind.clone(), &req.schema, &req.table).await?;
    match db {
        Db::Sqlite(pool) => {
            let definition: Option<String> = crate::sql_history::query_scalar("SELECT sql FROM sqlite_master WHERE type='table' AND name=?")
                .bind(&req.table).fetch_optional(pool).await?;
            let definition = definition.ok_or_else(|| anyhow!("目标必须为真实表，不能是视图"))?;
            // SQLite table_info 不暴露生成列；table_xinfo hidden=2/3 必须省略。
            let hidden = crate::sql_history::query(&format!("PRAGMA table_xinfo({})", db::quote_ident(kind, &req.table))).fetch_all(pool).await?;
            for row in hidden {
                let name: String = row.try_get("name")?;
                let generated: i64 = row.try_get("hidden")?;
                if generated != 0 && req.columns.contains(&name) { bail!("生成/隐藏字段 {} 不可写入", name); }
            }
            let primary_count = info.columns.iter().filter(|c| c.primary_key).count();
            if primary_count == 1 && !definition.to_ascii_uppercase().contains("WITHOUT ROWID") {
                // Inline INTEGER PRIMARY KEY DESC has its own PK index: it is
                // not a rowid alias and omission would silently insert NULL ids.
                let indexes = crate::sql_history::query(&format!("PRAGMA main.index_list({})", db::quote_ident(kind, &req.table))).fetch_all(pool).await?;
                let separate_primary = indexes.iter().try_fold(false, |found, row| -> std::result::Result<bool, sqlx::Error> { Ok(found || row.try_get::<String, _>("origin")? == "pk") })?;
                if !separate_primary {
                    for col in &mut info.columns {
                        // True INTEGER rowid aliases may be generated when omitted;
                        // explicitly supplied values still pass constraint validation.
                        if col.primary_key && col.data_type.eq_ignore_ascii_case("INTEGER") && !req.columns.contains(&col.name) { col.auto_increment = true; }
                    }
                }
            }
            validate_columns(req, &info)?;
            let sql = insert_sql(kind, req, &[]);
            let mut tx = pool.begin().await?;
            let mut affected = 0;
            for (index, row) in req.rows.iter().enumerate() {
                let mut query = crate::sql_history::query(&sql);
                for value in row { query = query.bind(value); }
                match query.execute(&mut *tx).await {
                    Ok(result) => affected += result.rows_affected(),
                    Err(error) => { tx.rollback().await?; return Err(anyhow!("第 {} 行失败；整批已回滚：{}", index + 1, error)); }
                }
            }
            tx.commit().await?; Ok(affected)
        }
        Db::Mysql(pool) => {
            // information_schema 列在部分兼容协议服务端（TiDB/OceanBase/Doris 等）会以
            // binary 字符集返回，sqlx 会判为 BINARY；必须走宽容解码而非 try_get::<String>。
            let table = crate::sql_history::query("SELECT TABLE_TYPE, ENGINE FROM information_schema.TABLES WHERE TABLE_SCHEMA=? AND TABLE_NAME=?")
                .bind(&req.schema).bind(&req.table).fetch_optional(pool).await?.ok_or_else(|| anyhow!("表不存在"))?;
            let ty = crate::schema::mysql_get(&table, "TABLE_TYPE").unwrap_or_default();
            let engine = crate::schema::mysql_get(&table, "ENGINE");
            if ty != "BASE TABLE" { bail!("目标必须为真实表"); }
            if !engine.as_deref().is_some_and(|e| e.eq_ignore_ascii_case("InnoDB")) { bail!("安全批量生成仅支持 InnoDB 事务表，当前引擎无法保证失败全部回滚"); }
            let generated = crate::sql_history::query("SELECT COLUMN_NAME, EXTRA FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=? AND TABLE_NAME=?")
                .bind(&req.schema).bind(&req.table).fetch_all(pool).await?;
            for row in generated {
                let extra = crate::schema::mysql_get(&row, "EXTRA").unwrap_or_default();
                if !mysql_generated_column(&extra) { continue; }
                let name = crate::schema::mysql_get(&row, "COLUMN_NAME").unwrap_or_default();
                if req.columns.contains(&name) { bail!("生成字段 {} 不可写入", name); }
                info.columns.retain(|c| c.name != name);
            }
            validate_columns(req, &info)?;
            let sql = insert_sql(kind, req, &[]);
            let mut tx = pool.begin().await?; let mut affected = 0;
            for (index, row) in req.rows.iter().enumerate() {
                let mut query = crate::sql_history::query(&sql);
                for value in row { query = query.bind(value); }
                match query.execute(&mut *tx).await {
                    Ok(result) => affected += result.rows_affected(),
                    Err(error) => { tx.rollback().await?; return Err(anyhow!("第 {} 行失败；整批已回滚：{}", index + 1, error)); }
                }
            }
            tx.commit().await?; Ok(affected)
        }
        Db::Postgres(pool) => {
            let metadata = crate::sql_history::query("SELECT a.attname, tn.nspname AS type_schema, t.typname, a.attgenerated::text AS attgenerated, a.attidentity::text AS attidentity FROM pg_attribute a JOIN pg_class c ON c.oid=a.attrelid JOIN pg_namespace n ON n.oid=c.relnamespace JOIN pg_type t ON t.oid=a.atttypid JOIN pg_namespace tn ON tn.oid=t.typnamespace WHERE n.nspname=$1 AND c.relname=$2 AND c.relkind IN ('r','p') AND a.attnum>0 AND NOT a.attisdropped")
                .bind(&req.schema).bind(&req.table).fetch_all(pool).await?;
            if metadata.is_empty() { bail!("目标必须为真实表"); }
            let mut types = Vec::new();
            for row in &metadata {
                let name: String = row.try_get("attname")?;
                let generated: String = row.try_get("attgenerated")?;
                let identity: String = row.try_get("attidentity")?;
                if (!generated.is_empty() || !identity.is_empty()) && req.columns.contains(&name) { bail!("生成/标识字段 {} 不可写入", name); }
                if !generated.is_empty() { info.columns.retain(|c| c.name != name); }
                if !identity.is_empty() { if let Some(c) = info.columns.iter_mut().find(|c| c.name == name) { c.auto_increment = true; } }
            }
            for name in &req.columns {
                let row = metadata.iter().find(|r| r.try_get::<String, _>("attname").ok().as_deref() == Some(name.as_str())).ok_or_else(|| anyhow!("字段 {} 不存在", name))?;
                let namespace: String = row.try_get("type_schema")?; let ty: String = row.try_get("typname")?;
                types.push(format!("{}.{}", db::quote_ident(kind, &namespace), db::quote_ident(kind, &ty)));
            }
            validate_columns(req, &info)?;
            let sql = insert_sql(kind, req, &types); let mut tx = pool.begin().await?; let mut affected = 0;
            for (index, row) in req.rows.iter().enumerate() {
                let mut query = crate::sql_history::query(&sql);
                for value in row { query = query.bind(value); }
                match query.execute(&mut *tx).await {
                    Ok(result) => affected += result.rows_affected(),
                    Err(error) => { tx.rollback().await?; return Err(anyhow!("第 {} 行失败；整批已回滚：{}", index + 1, error)); }
                }
            }
            tx.commit().await?; Ok(affected)
        }
        _ => unreachable!(),
    }

    }).await
}

#[tauri::command]
pub async fn insert_generated_rows(state: State<'_, AppState>, req: GeneratedRowsRequest) -> CmdResult<u64> {
    let _activity_guard = state.activity.enter()?;
    let _manual_guard = state.manual_transactions.ordinary(&[&req.connection_id]).await?;
    validate_shape(&req)?;
    let cfg = state.store.connections().into_iter().find(|c| c.id == req.connection_id).ok_or_else(|| AppError::new("连接不存在"))?;
    if cfg.read_only { return Err(AppError::new("此连接为只读，禁止数据生成写入")); }
    let db = crate::commands::scoped_db(&state, &req.connection_id, &req.schema).await?;
    crate::commands::validate_table_target(&db, &cfg.kind, &req.schema, &req.table, false).await?;
    let result = insert_batch(db.as_ref(), &cfg.kind, &req).await;
    match result {
        Ok(rows) => { storage::log_line("data-generator", &format!("connection={} table={}.{} committed_rows={}", cfg.name.replace(['\n','\r'], " "), req.schema.replace(['\n','\r'], " "), req.table.replace(['\n','\r'], " "), rows)); Ok(rows) }
        Err(error) => { storage::log_line("data-generator", "生成批次失败，未提交（详细数据库错误只返回当前窗口，日志不记录数据值）"); Err(AppError::from(error)) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn fixture() -> (Db, DbKind) {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        crate::sql_history::query("CREATE TABLE generated(id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL UNIQUE, amount REAL NOT NULL CHECK(amount>=0), active INTEGER NOT NULL, payload TEXT NOT NULL)").execute(&pool).await.unwrap();
        (Db::Sqlite(pool), DbKind::of("sqlite"))
    }
    fn request() -> GeneratedRowsRequest {
        GeneratedRowsRequest { connection_id: "test".into(), schema: "main".into(), table: "generated".into(), columns: ["name", "amount", "active", "payload"].map(String::from).into(),
            rows: (1..=25).map(|i| vec![Some(format!("generated_{}",i)),Some(format!("{}.25",i)),Some((i%2).to_string()),Some(format!("{{\"i\":{}}}",i))]).collect() }
    }
    #[test]
    fn mysql_default_generated_is_writable_but_real_generated_columns_are_not() {
        for extra in ["", "auto_increment", "DEFAULT_GENERATED", "DEFAULT_GENERATED on update CURRENT_TIMESTAMP", "default_generated", "DEFAULT_GENERATED INVISIBLE"] { assert!(!mysql_generated_column(extra), "{extra}"); }
        for extra in ["VIRTUAL GENERATED", "STORED GENERATED", "PERSISTENT GENERATED", "virtual\tgenerated", "STORED GENERATED INVISIBLE", "DEFAULT_GENERATED STORED GENERATED"] { assert!(mysql_generated_column(extra), "{extra}"); }
        let mut info=TableSchema{table:"events".into(),database:"isolated".into(),columns:vec![crate::models::ColumnInfo{name:"ts".into(),data_type:"timestamp".into(),nullable:false,default:Some("CURRENT_TIMESTAMP".into()),primary_key:false,auto_increment:false,comment:None,length:None,precision:None,scale:None}],indexes:vec![],foreign_keys:vec![],comment:None,engine:Some("InnoDB".into()),collation:None,row_count:None};
        if mysql_generated_column("DEFAULT_GENERATED") { info.columns.clear(); }
        let req=GeneratedRowsRequest{connection_id:"isolated".into(),schema:"isolated".into(),table:"events".into(),columns:vec!["ts".into()],rows:vec![vec![Some("2026-11-11 08:00:00".into())]]};
        assert!(validate_columns(&req,&info).is_ok(), "An explicitly supplied ordinary default-expression column must remain writable");
    }
    #[tokio::test]
    async fn sqlite_generated_rows_are_real_and_counted() {
        let (db, kind) = fixture().await; let req = request();
        assert_eq!(insert_batch(&db, &kind, &req).await.unwrap(), 25);
        let Db::Sqlite(pool) = &db else { unreachable!() };
        let (count, keys, amount): (i64,i64,f64) = crate::sql_history::query_as("SELECT COUNT(*),COUNT(DISTINCT id),SUM(amount) FROM generated").fetch_one(pool).await.unwrap();
        assert_eq!((count, keys), (25,25)); assert!((amount-331.25).abs()<0.00001);
        let required: i64 = crate::sql_history::query_scalar("SELECT COUNT(*) FROM generated WHERE name IS NULL OR amount IS NULL OR active IS NULL OR payload IS NULL").fetch_one(pool).await.unwrap(); assert_eq!(required,0);
        println!("actual SQLite generated rows={} distinct_primary_keys={} sum_amount={} null_required={}", count, keys, amount, required);
    }
    #[tokio::test]
    async fn sqlite_constraint_failure_rolls_back_entire_batch() {
        let (db, kind) = fixture().await; let mut req=request(); req.rows[5][1]=Some("-1".into());
        assert!(insert_batch(&db,&kind,&req).await.unwrap_err().to_string().contains("第 6 行"));
        let Db::Sqlite(pool)=&db else { unreachable!() };
        assert_eq!(crate::sql_history::query_scalar::<_,i64>("SELECT COUNT(*) FROM generated").fetch_one(pool).await.unwrap(),0);
    }
    #[tokio::test]
    async fn sqlite_values_and_identifiers_cannot_inject_sql() {
        let (db,kind)=fixture().await; let mut req=request(); req.rows.truncate(1);
        let attack="x'); DROP TABLE generated; --".to_string(); req.rows[0][0]=Some(attack.clone());
        assert_eq!(insert_batch(&db,&kind,&req).await.unwrap(),1);
        let Db::Sqlite(pool)=&db else { unreachable!() };
        assert_eq!(crate::sql_history::query_scalar::<_,String>("SELECT name FROM generated").fetch_one(pool).await.unwrap(),attack);
        req.columns[0]="name\") VALUES ('x'); DROP TABLE generated; --".into(); assert!(insert_batch(&db,&kind,&req).await.is_err());
        assert_eq!(crate::sql_history::query_scalar::<_,i64>("SELECT COUNT(*) FROM generated").fetch_one(pool).await.unwrap(),1);
    }
    #[tokio::test]
    async fn sqlite_rejects_attached_namespace_and_temp_shadow_without_writes() {
        let (db, kind) = fixture().await;
        let Db::Sqlite(pool) = &db else { unreachable!() };
        crate::sql_history::query("ATTACH DATABASE ':memory:' AS aux").execute(pool).await.unwrap();
        crate::sql_history::query("CREATE TABLE aux.generated(id INTEGER PRIMARY KEY, name TEXT)").execute(pool).await.unwrap();
        let mut req = request(); req.schema = "aux".into();
        assert!(insert_batch(&db, &kind, &req).await.unwrap_err().to_string().contains("main"));
        assert_eq!(crate::sql_history::query_scalar::<_,i64>("SELECT COUNT(*) FROM main.generated").fetch_one(pool).await.unwrap(), 0);
        assert_eq!(crate::sql_history::query_scalar::<_,i64>("SELECT COUNT(*) FROM aux.generated").fetch_one(pool).await.unwrap(), 0);
        crate::sql_history::query("CREATE TEMP TABLE generated(id INTEGER PRIMARY KEY, name TEXT)").execute(pool).await.unwrap();
        req.schema = "main".into();
        assert!(insert_batch(&db, &kind, &req).await.unwrap_err().to_string().contains("临时对象"));
        assert_eq!(crate::sql_history::query_scalar::<_,i64>("SELECT COUNT(*) FROM main.generated").fetch_one(pool).await.unwrap(), 0);
        assert_eq!(crate::sql_history::query_scalar::<_,i64>("SELECT COUNT(*) FROM temp.generated").fetch_one(pool).await.unwrap(), 0);
    }
    #[tokio::test]
    async fn sqlite_generated_desc_primary_key_requires_explicit_value() {
        let (db,kind)=fixture().await;
        let Db::Sqlite(pool)=&db else { unreachable!() };
        sqlx::raw_sql("CREATE TABLE descending(id INTEGER PRIMARY KEY DESC,text TEXT); CREATE TABLE rowid_alias(id INTEGER PRIMARY KEY,text TEXT)").execute(pool).await.unwrap();
        let mut req=GeneratedRowsRequest{connection_id:"test".into(),schema:"main".into(),table:"descending".into(),columns:vec!["text".into()],rows:vec![vec![Some("one".into())],vec![Some("two".into())]]};
        let error=insert_batch(&db,&kind,&req).await.unwrap_err();
        assert!(error.to_string().contains("缺少非空/主键字段 id"));
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM descending").fetch_one(pool).await.unwrap(),0);
        req.table="rowid_alias".into();assert_eq!(insert_batch(&db,&kind,&req).await.unwrap(),2);
        let ids:Vec<i64>=sqlx::query_scalar("SELECT id FROM rowid_alias ORDER BY id").fetch_all(pool).await.unwrap();assert_eq!(ids,vec![1,2]);
        req.table="descending".into();req.columns=vec!["id".into(),"text".into()];
        req.rows=vec![vec![Some("10".into()),Some("one".into())],vec![Some("11".into()),Some("two".into())]];
        assert_eq!(insert_batch(&db,&kind,&req).await.unwrap(),2);
        let ids:Vec<i64>=sqlx::query_scalar("SELECT id FROM descending ORDER BY id").fetch_all(pool).await.unwrap();assert_eq!(ids,vec![10,11]);
        pool.close().await;
    }

    #[tokio::test]
    async fn sqlite_rejects_missing_required_fields_null_and_excess_rows() {
        let (db,kind)=fixture().await; let mut req=request(); req.rows[0][0]=None; assert!(insert_batch(&db,&kind,&req).await.is_err());
        let mut req=request(); req.columns.remove(0); for row in &mut req.rows {row.remove(0);} assert!(insert_batch(&db,&kind,&req).await.is_err());
        let mut req=request(); req.rows=vec![req.rows[0].clone();MAX_ROWS+1]; assert!(insert_batch(&db,&kind,&req).await.is_err());
    }
}
