//! Ordered native key metadata. Query errors or unrepresentable keys are not empty metadata.
use anyhow::{anyhow, ensure, Result};
use sqlx::{MySqlPool, PgPool, Row, SqlitePool};
use std::collections::BTreeMap;
use crate::db::quote_ident;
use crate::models::{DbKind, ForeignKeyInfo, IndexInfo};

fn pragma(namespace: &str, name: &str, object: &str) -> String {
    let namespace = if namespace.is_empty() { "main" } else { namespace };
    format!("PRAGMA {}.{}({})", quote_ident(&DbKind::of("sqlite"), namespace), name, quote_ident(&DbKind::of("sqlite"), object))
}
fn action(value: &str) -> Result<String> {
    match value { "NO ACTION" | "RESTRICT" | "CASCADE" | "SET NULL" | "SET DEFAULT" => Ok(value.into()),
        _ => Err(anyhow!("STRUCTURE_METADATA_ACTION_UNPROVEN")) }
}
fn pg_action(value: &str) -> Result<String> {
    action(match value { "a" => "NO ACTION", "r" => "RESTRICT", "c" => "CASCADE", "n" => "SET NULL", "d" => "SET DEFAULT",
        _ => return Err(anyhow!("STRUCTURE_METADATA_ACTION_UNPROVEN")) })
}
fn mysql_text(row: &sqlx::mysql::MySqlRow, name: &str) -> Result<String> {
    super::mysql_get(row, name).filter(|s| !s.is_empty()).ok_or_else(|| anyhow!("STRUCTURE_METADATA_FIELD_UNPROVEN: {name}"))
}

pub(super) async fn sqlite_indexes(pool: &SqlitePool, namespace: &str, table: &str) -> Result<Vec<IndexInfo>> {
    let rows = crate::sql_history::query(&pragma(namespace, "index_list", table)).fetch_all(pool).await?;
    ensure!(rows.len() <= 512, "STRUCTURE_METADATA_INDEX_QUOTA");
    let mut indexes = Vec::new();
    for row in rows {
        let origin: String = row.try_get("origin")?;
        if origin == "pk" { continue; } // The ordered table PK is already returned by table_info.
        let name: String = row.try_get("name")?;
        ensure!(row.try_get::<i64, _>("partial")? == 0, "STRUCTURE_METADATA_UNREPRESENTABLE_INDEX: partial {name}");
        let key_rows = crate::sql_history::query(&pragma(namespace, "index_xinfo", &name)).fetch_all(pool).await?;
        let mut keys = Vec::new();
        for key in key_rows {
            if key.try_get::<i64, _>("key")? == 0 { continue; }
            let column: Option<String> = key.try_get("name")?;
            ensure!(key.try_get::<i64, _>("cid")? >= 0 && key.try_get::<i64, _>("desc")? == 0,
                "STRUCTURE_METADATA_UNREPRESENTABLE_INDEX: expression or descending {name}");
            let coll: Option<String> = key.try_get("coll")?;
            ensure!(coll.as_deref().map_or(true, |c| c == "BINARY"), "STRUCTURE_METADATA_UNREPRESENTABLE_INDEX: collation {name}");
            keys.push((key.try_get::<i64, _>("seqno")?, column.ok_or_else(|| anyhow!("STRUCTURE_METADATA_FIELD_UNPROVEN"))?));
        }
        keys.sort_by_key(|k| k.0);
        ensure!(!keys.is_empty() && keys.iter().enumerate().all(|(i, k)| k.0 == i as i64), "STRUCTURE_METADATA_KEY_ORDER_UNPROVEN");
        indexes.push(IndexInfo { name, columns: keys.into_iter().map(|k| k.1).collect(), unique: row.try_get::<i64, _>("unique")? != 0,
            primary: false, method: None });
    }
    indexes.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(indexes)
}

pub(super) async fn sqlite_foreign_keys(pool: &SqlitePool, namespace: &str, table: &str) -> Result<Vec<ForeignKeyInfo>> {
    let rows = crate::sql_history::query(&pragma(namespace, "foreign_key_list", table)).fetch_all(pool).await?;
    ensure!(rows.len() <= 4096, "STRUCTURE_METADATA_KEY_QUOTA");
    type Part = (i64, String, Option<String>);
    let mut groups: BTreeMap<i64, (ForeignKeyInfo, Vec<Part>)> = BTreeMap::new();
    for row in rows {
        let id: i64 = row.try_get("id")?;
        let target: String = row.try_get("table")?;
        let update = action(&row.try_get::<String, _>("on_update")?)?;
        let delete = action(&row.try_get::<String, _>("on_delete")?)?;
        let entry = groups.entry(id).or_insert_with(|| (ForeignKeyInfo { name: format!("FK_{table}_{id}"), columns: Vec::new(),
            ref_table: target.clone(), ref_schema: Some(if namespace.is_empty() { "main" } else { namespace }.into()), ref_columns: Vec::new(),
            on_update: Some(update.clone()), on_delete: Some(delete.clone()) }, Vec::new()));
        ensure!(entry.0.ref_table == target && entry.0.on_update.as_ref() == Some(&update) && entry.0.on_delete.as_ref() == Some(&delete),
            "STRUCTURE_METADATA_KEY_INCONSISTENT");
        entry.1.push((row.try_get("seq")?, row.try_get("from")?, row.try_get("to")?));
    }
    let mut result = Vec::new();
    for (_, (mut fk, mut parts)) in groups {
        parts.sort_by_key(|p| p.0);
        ensure!(parts.iter().enumerate().all(|(i, p)| p.0 == i as i64), "STRUCTURE_METADATA_KEY_ORDER_UNPROVEN");
        fk.columns = parts.iter().map(|p| p.1.clone()).collect();
        if parts.iter().all(|p| p.2.is_none()) {
            let target = crate::sql_history::query(&pragma(namespace, "table_info", &fk.ref_table)).fetch_all(pool).await?;
            let mut pk = Vec::new();
            for row in target { let n: i64 = row.try_get("pk")?; if n > 0 { pk.push((n, row.try_get::<String, _>("name")?)); } }
            pk.sort_by_key(|p| p.0);
            ensure!(pk.len() == parts.len() && pk.iter().enumerate().all(|(i, p)| p.0 == i as i64 + 1), "STRUCTURE_METADATA_REFERENCE_UNPROVEN");
            fk.ref_columns = pk.into_iter().map(|p| p.1).collect();
        } else {
            fk.ref_columns = parts.into_iter().map(|p| p.2.ok_or_else(|| anyhow!("STRUCTURE_METADATA_REFERENCE_UNPROVEN"))).collect::<Result<_>>()?;
        }
        result.push(fk);
    }
    Ok(result)
}

pub(super) async fn mysql_keys(pool: &MySqlPool, namespace: &str, table: &str) -> Result<(Vec<IndexInfo>, Vec<ForeignKeyInfo>)> {
    let rows = crate::sql_history::query("SELECT INDEX_NAME, COLUMN_NAME, NON_UNIQUE, SEQ_IN_INDEX, INDEX_TYPE, SUB_PART, COLLATION FROM information_schema.STATISTICS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY INDEX_NAME, SEQ_IN_INDEX LIMIT 8193")
        .bind(namespace).bind(table).fetch_all(pool).await?;
    ensure!(rows.len() <= 8192, "STRUCTURE_METADATA_KEY_QUOTA");
    let mut indexes: BTreeMap<String, IndexInfo> = BTreeMap::new();
    for row in rows {
        let name = mysql_text(&row, "INDEX_NAME")?;
        let column = mysql_text(&row, "COLUMN_NAME")?;
        let ordinal: usize = mysql_text(&row, "SEQ_IN_INDEX")?.parse()?;
        let method = mysql_text(&row, "INDEX_TYPE")?.to_ascii_lowercase();
        let unique = match mysql_text(&row, "NON_UNIQUE")?.as_str() { "0" => true, "1" => false, _ => return Err(anyhow!("STRUCTURE_METADATA_FIELD_UNPROVEN")) };
        ensure!(super::mysql_get(&row, "SUB_PART").is_none() && super::mysql_get(&row, "COLLATION").as_deref() != Some("D"),
            "STRUCTURE_METADATA_UNREPRESENTABLE_INDEX: prefix or descending {name}");
        let index = indexes.entry(name.clone()).or_insert_with(|| IndexInfo { name: name.clone(), columns: Vec::new(), unique, primary: name == "PRIMARY", method: Some(method.clone()) });
        ensure!(ordinal == index.columns.len() + 1 && index.unique == unique && index.method.as_ref() == Some(&method), "STRUCTURE_METADATA_KEY_ORDER_UNPROVEN");
        index.columns.push(column);
    }
    let rows = crate::sql_history::query("SELECT k.CONSTRAINT_NAME, k.COLUMN_NAME, k.ORDINAL_POSITION, k.REFERENCED_TABLE_SCHEMA AS REF_SCHEMA, k.REFERENCED_TABLE_NAME AS REF_TABLE, k.REFERENCED_COLUMN_NAME AS REF_COLUMN, r.UPDATE_RULE, r.DELETE_RULE FROM information_schema.KEY_COLUMN_USAGE k JOIN information_schema.REFERENTIAL_CONSTRAINTS r ON r.CONSTRAINT_SCHEMA = k.CONSTRAINT_SCHEMA AND r.CONSTRAINT_NAME = k.CONSTRAINT_NAME AND r.TABLE_NAME = k.TABLE_NAME WHERE k.TABLE_SCHEMA = ? AND k.TABLE_NAME = ? AND k.REFERENCED_TABLE_NAME IS NOT NULL ORDER BY k.CONSTRAINT_NAME, k.ORDINAL_POSITION LIMIT 8193")
        .bind(namespace).bind(table).fetch_all(pool).await?;
    ensure!(rows.len() <= 8192, "STRUCTURE_METADATA_KEY_QUOTA");
    let mut foreign: BTreeMap<String, ForeignKeyInfo> = BTreeMap::new();
    for row in rows {
        let name = mysql_text(&row, "CONSTRAINT_NAME")?;
        let target = mysql_text(&row, "REF_TABLE")?; let target_schema = mysql_text(&row, "REF_SCHEMA")?;
        let update = action(&mysql_text(&row, "UPDATE_RULE")?)?; let delete = action(&mysql_text(&row, "DELETE_RULE")?)?;
        let ordinal: usize = mysql_text(&row, "ORDINAL_POSITION")?.parse()?;
        let fk = foreign.entry(name.clone()).or_insert_with(|| ForeignKeyInfo { name, columns: Vec::new(), ref_table: target.clone(), ref_schema: Some(target_schema.clone()),
            ref_columns: Vec::new(), on_update: Some(update.clone()), on_delete: Some(delete.clone()) });
        ensure!(ordinal == fk.columns.len() + 1 && fk.ref_table == target && fk.ref_schema.as_ref() == Some(&target_schema)
            && fk.on_update.as_ref() == Some(&update) && fk.on_delete.as_ref() == Some(&delete), "STRUCTURE_METADATA_KEY_ORDER_UNPROVEN");
        fk.columns.push(mysql_text(&row, "COLUMN_NAME")?); fk.ref_columns.push(mysql_text(&row, "REF_COLUMN")?);
    }
    Ok((indexes.into_values().collect(), foreign.into_values().collect()))
}

const PG_INDEX_SQL: &str = "SELECT idx.relname::text AS index_name, x.indisprimary AS is_primary, x.indisunique AS is_unique, am.amname::text AS method, a.attname::text AS column_name, k.ordinal::bigint AS ordinal, opt.bits::int AS option_bits, x.indisvalid, x.indisready, x.indnatts::int AS all_columns, COALESCE((to_json(x)->>'indnkeyatts')::int, x.indnatts::int) AS key_columns, pg_get_expr(x.indpred,x.indrelid) AS predicate, pg_get_expr(x.indexprs,x.indrelid) AS expressions FROM pg_index x JOIN pg_class c ON c.oid = x.indrelid JOIN pg_namespace n ON n.oid = c.relnamespace JOIN pg_class idx ON idx.oid = x.indexrelid JOIN pg_am am ON am.oid = idx.relam JOIN LATERAL unnest(x.indkey::smallint[]) WITH ORDINALITY k(attnum,ordinal) ON true LEFT JOIN LATERAL unnest(x.indoption::smallint[]) WITH ORDINALITY opt(bits,ordinal) ON opt.ordinal = k.ordinal LEFT JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum = k.attnum WHERE n.nspname = $1 AND c.relname = $2 AND k.ordinal <= COALESCE((to_json(x)->>'indnkeyatts')::int,x.indnatts::int) ORDER BY idx.relname,k.ordinal LIMIT 8193";
const PG_FK_SQL: &str = "SELECT tc.conname::text AS constraint_name, a.attname::text AS local_column, c2.relname::text AS ref_table, n2.nspname::text AS ref_schema, af.attname::text AS ref_column, local_key.ordinal::bigint AS ordinal, tc.confupdtype::text AS update_action, tc.confdeltype::text AS delete_action FROM pg_constraint tc JOIN pg_class c ON c.oid = tc.conrelid JOIN pg_namespace n ON n.oid = c.relnamespace JOIN pg_class c2 ON c2.oid = tc.confrelid JOIN pg_namespace n2 ON n2.oid = c2.relnamespace JOIN LATERAL unnest(tc.conkey) WITH ORDINALITY local_key(attnum,ordinal) ON true JOIN LATERAL unnest(tc.confkey) WITH ORDINALITY ref_key(attnum,ordinal) ON ref_key.ordinal = local_key.ordinal JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum = local_key.attnum JOIN pg_attribute af ON af.attrelid = c2.oid AND af.attnum = ref_key.attnum WHERE tc.contype = 'f' AND n.nspname = $1 AND c.relname = $2 ORDER BY tc.conname,local_key.ordinal LIMIT 8193";

pub(super) async fn pg_keys(pool: &PgPool, namespace: &str, table: &str) -> Result<(Vec<IndexInfo>, Vec<ForeignKeyInfo>)> {
    let rows = crate::sql_history::query(PG_INDEX_SQL).bind(namespace).bind(table).fetch_all(pool).await?;
    ensure!(rows.len() <= 8192, "STRUCTURE_METADATA_KEY_QUOTA");
    let mut indexes: BTreeMap<String, IndexInfo> = BTreeMap::new();
    for row in rows {
        let name: String = row.try_get("index_name")?;
        let column: Option<String> = row.try_get("column_name")?;
        ensure!(row.try_get::<bool, _>("indisvalid")? && row.try_get::<bool, _>("indisready")?
            && row.try_get::<i32, _>("all_columns")? == row.try_get::<i32, _>("key_columns")?
            && row.try_get::<Option<String>, _>("predicate")?.is_none() && row.try_get::<Option<String>, _>("expressions")?.is_none()
            && row.try_get::<Option<i32>, _>("option_bits")?.unwrap_or(0) == 0, "STRUCTURE_METADATA_UNREPRESENTABLE_INDEX: {name}");
        let ordinal: i64 = row.try_get("ordinal")?;
        let unique: bool = row.try_get("is_unique")?; let primary: bool = row.try_get("is_primary")?; let method: String = row.try_get("method")?;
        let index = indexes.entry(name.clone()).or_insert_with(|| IndexInfo { name, columns: Vec::new(), unique, primary, method: Some(method.clone()) });
        ensure!(ordinal == index.columns.len() as i64 + 1 && index.primary == primary && index.unique == unique && index.method.as_ref() == Some(&method), "STRUCTURE_METADATA_KEY_ORDER_UNPROVEN");
        index.columns.push(column.ok_or_else(|| anyhow!("STRUCTURE_METADATA_FIELD_UNPROVEN"))?);
    }
    let rows = crate::sql_history::query(PG_FK_SQL).bind(namespace).bind(table).fetch_all(pool).await?;
    ensure!(rows.len() <= 8192, "STRUCTURE_METADATA_KEY_QUOTA");
    let mut foreign: BTreeMap<String, ForeignKeyInfo> = BTreeMap::new();
    for row in rows {
        let name: String = row.try_get("constraint_name")?; let target: String = row.try_get("ref_table")?; let schema: String = row.try_get("ref_schema")?;
        let update = pg_action(&row.try_get::<String, _>("update_action")?)?; let delete = pg_action(&row.try_get::<String, _>("delete_action")?)?;
        let ordinal: i64 = row.try_get("ordinal")?;
        let fk = foreign.entry(name.clone()).or_insert_with(|| ForeignKeyInfo { name, columns: Vec::new(), ref_table: target.clone(), ref_schema: Some(schema.clone()),
            ref_columns: Vec::new(), on_update: Some(update.clone()), on_delete: Some(delete.clone()) });
        ensure!(ordinal == fk.columns.len() as i64 + 1 && fk.ref_table == target && fk.ref_schema.as_ref() == Some(&schema)
            && fk.on_update.as_ref() == Some(&update) && fk.on_delete.as_ref() == Some(&delete), "STRUCTURE_METADATA_KEY_ORDER_UNPROVEN");
        fk.columns.push(row.try_get("local_column")?); fk.ref_columns.push(row.try_get("ref_column")?);
    }
    Ok((indexes.into_values().collect(), foreign.into_values().collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn pool() -> SqlitePool { sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap() }
    async fn exec(pool: &SqlitePool, sql: &str) { crate::sql_history::query(sql).execute(pool).await.unwrap(); }
    #[tokio::test] async fn sqlite_fk_order_actions_and_implicit_parent_key_are_exact() {
        let p = pool().await; exec(&p,"CREATE TABLE parent(a INT, b INT, PRIMARY KEY(b,a))").await;
        exec(&p,"CREATE TABLE child(x INT,y INT,FOREIGN KEY(x,y) REFERENCES parent ON UPDATE CASCADE ON DELETE SET NULL)").await;
        let keys = sqlite_foreign_keys(&p,"main","child").await.unwrap(); assert_eq!(keys.len(),1);
        assert_eq!(keys[0].columns,vec!["x","y"]); assert_eq!(keys[0].ref_columns,vec!["b","a"]);
        assert_eq!(keys[0].ref_schema.as_deref(),Some("main")); assert_eq!(keys[0].on_update.as_deref(),Some("CASCADE")); assert_eq!(keys[0].on_delete.as_deref(),Some("SET NULL")); p.close().await;
    }
    #[tokio::test] async fn sqlite_regular_and_unique_indexes_preserve_order_and_names() {
        let p=pool().await; exec(&p,"CREATE TABLE t(a INT,b INT,UNIQUE(b,a))").await; exec(&p,"CREATE INDEX ordered ON t(b,a)").await;
        let indexes=sqlite_indexes(&p,"main","t").await.unwrap(); assert_eq!(indexes.len(),2);
        let ordinary=indexes.iter().find(|i|i.name=="ordered").unwrap(); assert_eq!(ordinary.columns,vec!["b","a"]); assert!(!ordinary.unique);
        assert!(indexes.iter().any(|i|i.unique&&i.columns==vec!["b","a"])); p.close().await;
    }
    #[tokio::test] async fn sqlite_attached_namespace_and_quoted_names_are_not_main_aliases() {
        let p=pool().await; exec(&p,"ATTACH DATABASE ':memory:' AS aux").await;
        exec(&p,"CREATE TABLE main.parent(a INT PRIMARY KEY)").await; exec(&p,"CREATE TABLE aux.parent(b INT PRIMARY KEY)").await;
        exec(&p,"CREATE TABLE aux.\"child\"\"name\"(x INT REFERENCES parent)").await;
        let keys=sqlite_foreign_keys(&p,"aux","child\"name").await.unwrap(); assert_eq!(keys[0].ref_columns,vec!["b"]); assert_eq!(keys[0].ref_schema.as_deref(),Some("aux")); p.close().await;
    }
    #[tokio::test] async fn sqlite_partial_or_expression_indexes_are_not_false_empty_metadata() {
        let p=pool().await; exec(&p,"CREATE TABLE t(a INT)").await; exec(&p,"CREATE INDEX partial ON t(a) WHERE a>0").await;
        assert!(sqlite_indexes(&p,"main","t").await.unwrap_err().to_string().contains("UNREPRESENTABLE_INDEX"));
        exec(&p,"DROP INDEX partial").await; exec(&p,"CREATE INDEX expr ON t(a+1)").await;
        assert!(sqlite_indexes(&p,"main","t").await.unwrap_err().to_string().contains("UNREPRESENTABLE_INDEX")); p.close().await;
    }
    #[tokio::test] async fn sqlite_missing_implicit_parent_does_not_invent_key_columns() {
        let p=pool().await; exec(&p,"CREATE TABLE t(a INT REFERENCES missing)").await;
        assert!(sqlite_foreign_keys(&p,"main","t").await.unwrap_err().to_string().contains("REFERENCE_UNPROVEN")); p.close().await;
    }
    #[test] fn postgres_sql_pairs_ordinals_and_never_uses_cartesian_any_join() {
        assert!(PG_FK_SQL.contains("ref_key.ordinal = local_key.ordinal")); assert!(PG_FK_SQL.contains("n2.nspname"));
        assert!(PG_INDEX_SQL.contains("WITH ORDINALITY")); assert!(!PG_FK_SQL.contains("ANY(")); assert!(!PG_INDEX_SQL.contains("ANY("));
        for (code,expected) in [("a","NO ACTION"),("r","RESTRICT"),("c","CASCADE"),("n","SET NULL"),("d","SET DEFAULT")] { assert_eq!(pg_action(code).unwrap(),expected); }
        assert!(pg_action("unknown").is_err());
    }
    #[test] fn postgres_index_sql_avoids_to_jsonb_for_opengauss() {
        // openGauss 7.0（9.2.4 内核）没有 to_jsonb(record)，会 42883；to_json 在 PG 9.x/10/11+/openGauss 均存在。
        // 保留 COALESCE 回退：PG 10 无 indnkeyatts 时 to_json(x)->>'indnkeyatts' 为 NULL，退回 indnatts 仍正确。
        assert!(!PG_INDEX_SQL.contains("to_jsonb"), "openGauss 不支持 to_jsonb");
        assert!(PG_INDEX_SQL.contains("to_json(x)->>'indnkeyatts'"));
        assert!(PG_INDEX_SQL.contains("COALESCE((to_json(x)->>'indnkeyatts')::int, x.indnatts::int)"));
    }
}
