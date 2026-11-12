//! Complete comparison semantics layered on the existing type-aware filter compiler.
//! The original filter source and all its tests remain unchanged.
use anyhow::{anyhow, Result};
use std::collections::BTreeMap;
use sqlx::Row;
use crate::{db::{self,Db}, models::{DbKind, FilterCondition, FilterLine, PageRequest, FilterLogic}};
use super::{table_schema, typed_table_filter::{self, FilterType, column_type}};

fn condition(kind: &DbKind, filter: &FilterCondition, ty: FilterType) -> Result<String> {
    let op=filter.operator.trim();
    if ![">", ">=", "<", "<="].contains(&op) { return typed_table_filter::condition(kind, filter, ty); }
    // Equality already certifies type/dialect/collation and safely quotes the value.
    // Change only its actual operator token, never an '=' inside an identifier/value.
    let mut comparison=filter.clone(); comparison.operator="=".into();
    let sql=typed_table_filter::condition(kind, &comparison, ty)?;
    // A fixed safe probe value leaves the real quoted column/collation prefix
    // unchanged. Its last equality delimiter cannot be inside a user identifier
    // or value, including SQL Server's escaped ]] bracket identifiers.
    let mut probe=comparison.clone();probe.value="__navkit_comparison_probe__".into();
    let probe_sql=typed_table_filter::condition(kind,&probe,ty)?;
    let operator=probe_sql.rfind(" = ").ok_or_else(||anyhow!("FILTER_COMPARISON_OPERATOR_MISSING"))?+1;
    if sql.get(operator..operator+1)!=Some("=") {return Err(anyhow!("FILTER_COMPARISON_OPERATOR_MISMATCH"));}
    Ok(format!("{}{}{}", &sql[..operator], op, &sql[operator+1..]))
}

pub(super) fn clause(kind: &DbKind, lines: &[FilterLine], types: &BTreeMap<String, FilterType>) -> Result<String> {
    let mut groups=Vec::new();
    for line in lines {
        if line.conds.is_empty() { continue; }
        let mut group=String::new();
        for (index, filter) in line.conds.iter().enumerate() {
            if index>0 { group.push_str(match filter.logic.unwrap_or(FilterLogic::And) { FilterLogic::And=>" AND ", FilterLogic::Or=>" OR " }); }
            let ty=*types.get(&filter.column).ok_or_else(||anyhow!("FILTER_COLUMN_METADATA_MISSING: {}",filter.column))?;
            group.push_str(&condition(kind,filter,ty)?);
        }
        groups.push((line.logic,group,line.conds.len()>1));
    }
    if groups.is_empty() { return Ok(String::new()); }
    let multiple=groups.len()>1; let mut sql=String::new();
    for (index,(logic,group,bracket)) in groups.iter().enumerate() {
        if index>0 { sql.push_str(match logic { FilterLogic::And=>" AND ", FilterLogic::Or=>" OR " }); }
        if multiple && *bracket { sql.push('('); sql.push_str(group); sql.push(')'); } else { sql.push_str(group); }
    }
    Ok(format!(" WHERE ({sql})"))
}

pub(super) async fn typed_clause(db: &Db, kind: &DbKind, request: &PageRequest) -> Result<String> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if request.filters.iter().all(|line|line.conds.is_empty()) { return Ok(String::new()); }
    let mut types=BTreeMap::new();
    if let Db::Postgres(pool)=db {
        // Preserve the existing recursive domain/citext metadata certification.
        let rows=crate::sql_history::query("WITH RECURSIVE types AS (SELECT a.attname,t.oid,t.typbasetype,t.typcategory,t.typname,0 AS depth FROM pg_attribute a JOIN pg_class c ON c.oid=a.attrelid JOIN pg_namespace n ON n.oid=c.relnamespace JOIN pg_type t ON t.oid=a.atttypid WHERE n.nspname=$1 AND c.relname=$2 AND a.attnum>0 AND NOT a.attisdropped UNION ALL SELECT x.attname,t.oid,t.typbasetype,t.typcategory,t.typname,x.depth+1 FROM types x JOIN pg_type t ON t.oid=x.typbasetype WHERE x.typbasetype<>0 AND x.depth<16) SELECT attname,typname,typcategory::text AS category FROM types WHERE typbasetype=0").bind(&request.schema).bind(&request.table).fetch_all(pool).await?;
        for row in rows { let name:String=row.try_get("attname")?; let ty:String=row.try_get("typname")?; let category:String=row.try_get("category")?; types.insert(name,if category=="S" {FilterType::Text} else {column_type(&ty)}); }
    } else {
        let metadata=table_schema(db,kind.clone(),&request.schema,&request.table).await?;
        for column in metadata.columns {
            let ty=if kind.is_sqlite() {
                let affinity=column.data_type.to_ascii_uppercase();
                if affinity.contains("CHAR")||affinity.contains("CLOB")||affinity.contains("TEXT") {FilterType::Text}
                else if affinity.contains("INT")||affinity.contains("REAL")||affinity.contains("FLOA")||affinity.contains("DOUB")||affinity.contains("NUM")||affinity.contains("DEC") {FilterType::Scalar}
                else {column_type(&column.data_type)}
            } else {column_type(&column.data_type)};
            types.insert(column.name,ty);
        }
    }
    clause(kind,&request.filters,&types)

    }).await
}

#[cfg(test)] mod tests {
    use super::*;
    fn filter(column: &str, operator: &str, value: &str) -> FilterCondition { FilterCondition{column:column.into(),operator:operator.into(),value:value.into(),case_sensitive:true,logic:None} }
    #[tokio::test] async fn sqlite_sensitive_ranges_override_nocase_and_preserve_numeric_semantics() {
        let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        crate::sql_history::query("CREATE TABLE t(x TEXT COLLATE NOCASE,n INTEGER); INSERT INTO t VALUES('B',2),('b',10),('a',1)").execute(&pool).await.unwrap();
        let kind=DbKind::of("sqlite");
        for (op,expected) in [("<",vec!["B"]),("<=",vec!["B","a"]),(">",vec!["b"]),(">=",vec!["a","b"])] {
            let request:PageRequest=serde_json::from_value(serde_json::json!({"connection_id":"isolated","schema":"main","table":"t","offset":0,"limit":100,"filters":[{"conds":[filter("x",op,"a")]}]})).unwrap();
            let sql=format!("SELECT x FROM t{} ORDER BY x COLLATE BINARY",typed_clause(&Db::Sqlite(pool.clone()),&kind,&request).await.unwrap());
            let rows:Vec<String>=crate::sql_history::query_scalar(&sql).fetch_all(&pool).await.unwrap(); assert_eq!(rows,expected,"{sql}");
        }
        let sql=condition(&kind,&filter("n","<","10"),FilterType::Scalar).unwrap();assert!(!sql.contains("COLLATE"));
        let rows:Vec<i64>=crate::sql_history::query_scalar(&format!("SELECT n FROM t WHERE {sql} ORDER BY n")).fetch_all(&pool).await.unwrap();assert_eq!(rows,vec![1,2]);
        let unchanged:i64=crate::sql_history::query_scalar("SELECT count(*) FROM t WHERE x<'a'").fetch_one(&pool).await.unwrap();assert_eq!(unchanged,0);
        pool.close().await;
    }
    #[test] fn certified_mysql_pg_mssql_ranges_and_unknown_types() {
        for operator in [">",">=","<","<="] {
            for (kind,prefix) in [("mysql","CONVERT(`x` USING utf8mb4) COLLATE utf8mb4_bin"),("postgresql","CAST(\"x\" AS TEXT) COLLATE \"C\""),("sqlserver","[x] COLLATE Latin1_General_100_BIN2")] {
                let k=DbKind::of(kind);let sql=condition(&k,&filter("x",operator,"a"),FilterType::Text).unwrap();assert!(sql.starts_with(prefix),"{sql}");assert!(sql.contains(&format!(" {operator} ")));
                let numeric=condition(&k,&filter("n",operator,"10"),FilterType::Scalar).unwrap();assert!(!numeric.contains("COLLATE")&&!numeric.contains("LOWER"));
                assert!(condition(&k,&filter("x",operator,"a"),FilterType::Unknown).is_err());
            }
        }
    }
    #[test] fn replaces_only_comparison_operator_not_equals_inside_identifier_or_value() {
        for kind in ["sqlite","mysql","postgresql","sqlserver"] {
            for column in ["name = quoted","name ] = quoted","name \" = quoted","name ` = quoted"] {
                let k=DbKind::of(kind);let sql=condition(&k,&filter(column,"<=","a = b'"),FilterType::Text).unwrap();
                assert!(sql.contains(&db::quote_ident(&k,column)),"{sql}");assert!(sql.contains("a = b''"),"{sql}");assert!(sql.contains(" <= "),"{sql}");
            }
        }
    }
}
