//! Catalog evidence read exclusively on the supplied owned connection.
use super::*;
use serde::Serialize;
use crate::{models::{ColumnInfo, IndexInfo, TableSchema}, structure_execution::metadata::{PhysicalDatabaseIdentity, PhysicalTableIdentity}};
#[derive(Clone)] pub(super) struct Physical { pub server: String, pub database: String, pub kind: String }
#[derive(Clone, Serialize)] pub(super) struct Snapshot {
    pub schema: TableSchema, pub identity: PhysicalTableIdentity, pub revision: String,
    pub physical_table: String, pub unsupported: Vec<String>, pub evidence: Vec<Vec<Record>>,
}
pub(super) async fn physical(s: &mut Session) -> Result<Physical> {
    if s.pg() {
        // OIDs alone are only cluster-local. A real cluster system identifier prevents false alias proof.
        let rows = s.rows("SELECT (SELECT system_identifier::text FROM pg_catalog.pg_control_system()) AS server, pg_catalog.pg_postmaster_start_time()::text AS instance, d.oid::text AS database FROM pg_catalog.pg_database d WHERE d.datname=current_database()").await
            .map_err(|e| anyhow!("STRUCTURE_PG_IDENTITY_UNPROVEN: 需要 pg_control_system 的真实 cluster 身份权限，不用 saved FP 冒充物理库：{e:#}"))?;
        ensure!(rows.len() == 1, "STRUCTURE_PG_DATABASE_IDENTITY_UNKNOWN");
        Ok(Physical { server: format!("postgres-cluster:{}:instance:{}", get(&rows[0], "server")?, get(&rows[0], "instance")?), database: get(&rows[0], "database")?.into(), kind: "postgres-server".into() })
    } else {
        let rows = s.rows("SELECT CAST(@@server_uuid AS CHAR) AS server, DATABASE() AS database, CAST(@@version AS CHAR) AS version, CAST(@@version_comment AS CHAR) AS comment").await
            .map_err(|e| anyhow!("STRUCTURE_MYSQL_UUID_UNPROVEN: 仅 MySQL 8 server_uuid 认证，MariaDB/TiDB 不降级伪造身份：{e:#}"))?;
        ensure!(rows.len() == 1, "STRUCTURE_MYSQL_IDENTITY_UNKNOWN"); let r = &rows[0];
        let uuid = uuid::Uuid::parse_str(get(r, "server")?).map_err(|_| anyhow!("STRUCTURE_MYSQL_UUID_UNPROVEN"))?;
        ensure!(!uuid.is_nil() && get(r, "version")?.starts_with("8.") && !get(r, "comment")?.to_ascii_lowercase().contains("tidb"), "STRUCTURE_MYSQL_VERSION_UNSUPPORTED: 只认证 MySQL 8 的 InnoDB/LOCK TABLES/原子 RENAME");
        // information_schema can silently hide views/triggers/FKs for restricted users.
        // Fail closed unless global metadata visibility is proven, never call hidden dependencies empty.
        let visible = s.scalar("SELECT CAST(count(*) AS CHAR) FROM mysql.user WHERE CONCAT(User,'@',Host)=CURRENT_USER() AND Select_priv='Y' AND Show_view_priv='Y' AND Trigger_priv='Y' AND Process_priv='Y'").await
            .map_err(|e| anyhow!("STRUCTURE_MYSQL_METADATA_VISIBILITY_UNPROVEN: 依赖扫描需要全局 SELECT/SHOW VIEW/TRIGGER/PROCESS 可见性：{e:#}"))?;
        ensure!(visible == "1", "STRUCTURE_MYSQL_METADATA_VISIBILITY_UNPROVEN: 无法证明入站外键/视图/触发器全集可见，预览不可执行");
        let mdl = s.scalar("SELECT CAST(count(*) AS CHAR) FROM performance_schema.setup_instruments WHERE NAME='wait/lock/metadata/sql/mdl' AND ENABLED='YES'").await?;
        ensure!(mdl == "1", "STRUCTURE_MYSQL_MDL_UNPROVEN: metadata lock instrumentation 未启用，不能认证跨隐式 DDL 锁与原子切换");
        Ok(Physical { server: format!("mysql-uuid:{uuid}"), database: get(r, "database")?.into(), kind: "mysql-server".into() })
    }
}
fn column(name: String, data_type: String, nullable: bool, default: Option<String>, comment: Option<String>) -> ColumnInfo {
    ColumnInfo { name, data_type, nullable, default, primary_key: false, auto_increment: false, comment, length: None, precision: None, scale: None }
}
fn finish(physical: &Physical, namespace: &str, table: &str, table_id: i64, schema: TableSchema, evidence: Vec<Vec<Record>>, unsupported: Vec<String>, actual_database: &str) -> Result<Snapshot> {
    ensure!(table_id > 0 && !schema.columns.is_empty() && schema.columns.len() <= 4096 && schema.indexes.len() <= 512, "STRUCTURE_METADATA_QUOTA_OR_UNKNOWN");
    let database = PhysicalDatabaseIdentity { kind: physical.kind.clone(), identity: format!("{}:database:{}", physical.server, actual_database), canonical_path: None };
    // The wire-compatible rootpage slot carries actual PG table OID / MySQL InnoDB TABLE_ID, never SQLite rootpage.
    let identity = PhysicalTableIdentity { database, schema: namespace.into(), table: table.into(), rootpage: table_id };
    let physical_table = format!("{}:table:{table_id}", identity.database.identity);
    let revision = crate::object_catalog::revision(&(&identity, &schema, &evidence, &unsupported));
    Ok(Snapshot { schema, identity, revision, physical_table, unsupported, evidence })
}
pub(super) async fn snapshot(s: &mut Session, physical: &Physical, namespace: &str, table: &str) -> Result<Snapshot> {
    plan::ident(namespace)?; plan::ident(table)?;
    if s.pg() { pg_snapshot(s, physical, namespace, table).await } else { mysql_snapshot(s, physical, namespace, table).await }
}
async fn pg_snapshot(s: &mut Session, physical: &Physical, namespace: &str, table: &str) -> Result<Snapshot> {
    let ns = db::quote_literal(namespace); let name = db::quote_literal(table);
    let tables = s.rows(&format!("SELECT c.oid::text AS oid,n.oid::text AS namespace,c.relkind::text AS kind,c.relpersistence::text AS persistence,c.relrowsecurity::text AS rls,c.relforcerowsecurity::text AS force_rls,COALESCE(c.reloptions::text,'') AS options,COALESCE(pg_catalog.obj_description(c.oid,'pg_class'),'') AS comment,pg_catalog.pg_get_userbyid(c.relowner) AS owner,c.relispartition::text AS partition FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname={ns} AND c.relname={name}")).await?;
    ensure!(tables.len() == 1, "STRUCTURE_TABLE_NOT_FOUND_OR_AMBIGUOUS"); let t = &tables[0]; let oid = get(t, "oid")?.parse::<i64>()?;
    ensure!(get(t, "kind")? == "r", "STRUCTURE_TABLE_UNSUPPORTED: 不是 PostgreSQL 普通实体表");
    let mut unsupported = Vec::new();
    if get(t, "persistence")? != "p" || get(t, "rls")? != "false" || get(t, "force_rls")? != "false" || get(t, "partition")? != "false" || !get(t, "options")?.is_empty() { unsupported.push("临时/unlogged/分区/RLS/自定义表选项未能证明无损保留".into()); }
    let columns = s.rows(&format!("SELECT a.attnum::text AS ordinal,a.attname AS name,pg_catalog.format_type(a.atttypid,a.atttypmod) AS type,a.attnotnull::text AS not_null,pg_catalog.pg_get_expr(d.adbin,d.adrelid) AS default,a.attidentity::text AS identity,a.attgenerated::text AS generated,COALESCE(pg_catalog.col_description(a.attrelid,a.attnum),'') AS comment,ty.typtype::text AS type_kind,tn.nspname AS type_schema,a.attcollation::text AS collation,ty.typcollation::text AS default_collation,COALESCE(a.attoptions::text,'') AS options FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_type ty ON ty.oid=a.atttypid JOIN pg_catalog.pg_namespace tn ON tn.oid=ty.typnamespace LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum WHERE a.attrelid={oid} AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum")).await?;
    let mut cols = Vec::new();
    for r in &columns {
        let name = get(r, "name")?.to_owned(); plan::ident(&name)?;
        let data_type = get(r, "type")?.to_owned(); let default = opt(r, "default")?;
        if !get(r, "identity")?.is_empty() || !get(r, "generated")?.is_empty() || get(r, "type_schema")? != "pg_catalog" || get(r, "type_kind")? != "b" || get(r, "collation")? != get(r, "default_collation")? || !get(r, "options")?.is_empty() { unsupported.push(format!("列 {name} 有 identity/generated/custom type/collation/options，不能自动保留")); }
        if let Err(e) = plan::safe_type(&data_type, true) { unsupported.push(format!("列 {name}：{e:#}")); }
        if let Some(d) = &default { if let Err(e) = plan::safe_default(d, true) { unsupported.push(format!("列 {name}：{e:#}")); } }
        cols.push(column(name, data_type, get(r, "not_null")? == "false", default, opt(r, "comment")?.filter(|s| !s.is_empty())));
    }
    let indexes = s.rows(&format!("SELECT ci.relname AS name,i.indexrelid::text AS oid,i.indisunique::text AS unique,i.indisprimary::text AS primary,i.indisvalid::text AS valid,i.indisready::text AS ready,i.indnkeyatts::text AS keys,i.indnatts::text AS total,i.indkey::text AS key_numbers,i.indoption::text AS options,am.amname AS method,pg_catalog.pg_get_indexdef(i.indexrelid) AS definition,pg_catalog.pg_get_expr(i.indpred,i.indrelid) AS predicate,pg_catalog.pg_get_expr(i.indexprs,i.indrelid) AS expression,COALESCE(con.contype::text,'') AS constraint_type,COALESCE(con.conname,'') AS constraint_name FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class ci ON ci.oid=i.indexrelid JOIN pg_catalog.pg_am am ON am.oid=ci.relam LEFT JOIN pg_catalog.pg_constraint con ON con.conindid=i.indexrelid WHERE i.indrelid={oid} ORDER BY ci.relname")).await?;
    let by_num = columns.iter().map(|r| Ok((get(r, "ordinal")?.to_owned(), get(r, "name")?.to_owned()))).collect::<Result<BTreeMap<_, _>>>()?;
    let mut ix = Vec::new();
    for r in &indexes {
        let name = get(r, "name")?.to_owned(); let primary = get(r, "primary")? == "true";
        let keys = get(r, "key_numbers")?.split_ascii_whitespace().map(|n| by_num.get(n).cloned().ok_or_else(|| anyhow!("STRUCTURE_INDEX_EXPRESSION_UNSUPPORTED"))).collect::<Result<Vec<_>>>();
        let normal = keys.is_ok() && get(r, "valid")? == "true" && get(r, "ready")? == "true" && get(r, "keys")? == get(r, "total")? && get(r, "options")?.split_ascii_whitespace().all(|v| v == "0") && get(r, "method")? == "btree" && opt(r, "predicate")?.is_none() && opt(r, "expression")?.is_none() && (get(r, "constraint_type")?.is_empty() || primary);
        if !normal { unsupported.push(format!("索引 {name} 有表达式/partial/include/排序/unique约束或未知方法，不生成不完整普通索引")); }
        if let Ok(keys) = keys { if primary { for c in &mut cols { c.primary_key = keys.contains(&c.name); } }
            ix.push(IndexInfo { name: if primary { get(r, "constraint_name")?.into() } else { name }, columns: keys, unique: get(r, "unique")? == "true", primary, method: Some(get(r, "method")?.into()) }); }
    }
    let constraints = s.rows(&format!("SELECT oid::text AS oid,conname AS name,contype::text AS type,conrelid::text AS relation,confrelid::text AS reference,pg_catalog.pg_get_constraintdef(oid,true) AS definition FROM pg_catalog.pg_constraint WHERE (conrelid={oid} AND contype<>'p') OR confrelid={oid} ORDER BY oid")).await?;
    if !constraints.is_empty() { unsupported.push("CHECK/外键(含入站)/排除/unique约束无法保证无损保留，明确不可执行，不 CASCADE".into()); }
    let triggers = s.rows(&format!("SELECT oid::text AS oid,tgname AS name,pg_catalog.pg_get_triggerdef(oid,true) AS definition FROM pg_catalog.pg_trigger WHERE tgrelid={oid} AND NOT tgisinternal ORDER BY oid")).await?;
    let views = s.rows(&format!("SELECT DISTINCT c.oid::text AS oid,n.nspname AS schema,c.relname AS name,pg_catalog.pg_get_viewdef(c.oid,true) AS definition FROM pg_catalog.pg_depend d JOIN pg_catalog.pg_rewrite r ON r.oid=d.objid JOIN pg_catalog.pg_class c ON c.oid=r.ev_class JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE d.refobjid={oid} AND d.classid='pg_catalog.pg_rewrite'::regclass AND c.relkind IN ('v','m') ORDER BY oid")).await?;
    let others = s.rows(&format!("SELECT inhrelid::text AS child,inhparent::text AS parent FROM pg_catalog.pg_inherits WHERE inhrelid={oid} OR inhparent={oid}")).await?;
    let rules = s.rows(&format!("SELECT oid::text AS oid,rulename AS name,pg_catalog.pg_get_ruledef(oid,true) AS definition FROM pg_catalog.pg_rewrite WHERE ev_class={oid}")).await?;
    if !triggers.is_empty() || !views.is_empty() || !others.is_empty() || !rules.is_empty() { unsupported.push("触发器/依赖视图/继承/规则未能保留，预览不可执行".into()); }
    // Custom operator classes/collations are included by pg_get_indexdef but not in IndexInfo.
    for r in &indexes { if get(r, "definition")?.contains(" COLLATE ") || get(r, "definition")?.contains("_ops") || get(r, "definition")?.contains(" WITH (") { unsupported.push("自定义索引 opclass/collation/options 未完整建模".into()); } }
    let schema = TableSchema { table: table.into(), database: namespace.into(), columns: cols, indexes: ix, foreign_keys: Vec::new(), comment: opt(t, "comment")?.filter(|s| !s.is_empty()), engine: None, collation: None, row_count: None };
    finish(physical, namespace, table, oid, schema, vec![tables, columns, indexes, constraints, triggers, views, others, rules], unsupported, &physical.database)
}
async fn mysql_snapshot(s: &mut Session, physical: &Physical, namespace: &str, table: &str) -> Result<Snapshot> {
    let ns = db::quote_literal(namespace); let name = db::quote_literal(table);
    let tables = s.rows(&format!("SELECT TABLE_SCHEMA AS schema_name,TABLE_NAME AS table_name,TABLE_TYPE AS type,ENGINE AS engine,TABLE_COLLATION AS collation,CREATE_OPTIONS AS options,TABLE_COMMENT AS comment FROM information_schema.TABLES WHERE TABLE_SCHEMA={ns} AND TABLE_NAME={name}")).await?;
    ensure!(tables.len() == 1, "STRUCTURE_TABLE_NOT_FOUND_OR_AMBIGUOUS"); let t = &tables[0];
    ensure!(get(t, "schema_name")? == namespace && get(t, "table_name")? == table && get(t, "type")? == "BASE TABLE" && get(t, "engine")? == "InnoDB", "STRUCTURE_MYSQL_TABLE_UNSUPPORTED: 仅准确身份的普通 InnoDB 表");
    let ids = s.rows(&format!("SELECT CAST(TABLE_ID AS CHAR) AS id,NAME AS name FROM information_schema.INNODB_TABLES WHERE NAME=CONCAT({ns},'/',{name})")).await?;
    ensure!(ids.len() == 1, "STRUCTURE_MYSQL_TABLE_ID_UNPROVEN: 表名编码/分区或 TABLE_ID 不可证明"); let id = get(&ids[0], "id")?.parse::<i64>()?;
    let ddl = s.rows(&format!("SHOW CREATE TABLE {}", db::qualified(&s.kind(), namespace, table))).await?;
    ensure!(ddl.len() == 1, "STRUCTURE_MYSQL_DDL_UNKNOWN"); let mut unsupported = Vec::new();
    if !get(t, "options")?.is_empty() { unsupported.push("自定义 CREATE_OPTIONS 未能无损保留".into()); }
    let columns = s.rows(&format!("SELECT COLUMN_NAME AS name,COLUMN_TYPE AS type,IS_NULLABLE AS nullable,COLUMN_DEFAULT AS default_value,EXTRA AS extra,COLUMN_COMMENT AS comment,GENERATION_EXPRESSION AS generated,COLLATION_NAME AS collation,CAST(ORDINAL_POSITION AS CHAR) AS ordinal FROM information_schema.COLUMNS WHERE TABLE_SCHEMA={ns} AND TABLE_NAME={name} ORDER BY ORDINAL_POSITION")).await?;
    let mut cols = Vec::new();
    for r in &columns {
        let n = get(r, "name")?.to_owned(); let ty = get(r, "type")?.to_owned(); let raw = opt(r, "default_value")?;
        let extra = get(r, "extra")?;
        if !get(r, "generated")?.is_empty() || !extra.is_empty() { unsupported.push(format!("列 {n} generated/auto_increment/on update/default expression 未认证，拒绝影子替换")); }
        if opt(r, "collation")?.as_deref().is_some_and(|c| Some(c) != opt(t, "collation").ok().flatten().as_deref()) { unsupported.push(format!("列 {n} 自定义 collation 未完整保留")); }
        if let Err(e) = plan::safe_type(&ty, false) { unsupported.push(format!("列 {n}：{e:#}")); }
        // information_schema MySQL COLUMN_DEFAULT is a *value*, not SQL literal.
        let numeric = ty.split(['(', ' ']).next().is_some_and(|v| ["tinyint", "smallint", "mediumint", "int", "integer", "bigint", "decimal", "numeric", "float", "double", "real"].contains(&v.to_ascii_lowercase().as_str()));
        let default = raw.map(|d| if numeric { d } else if ["CURRENT_TIMESTAMP", "CURRENT_TIMESTAMP()"].iter().any(|v| d.eq_ignore_ascii_case(v)) && ty.starts_with("timestamp") { "CURRENT_TIMESTAMP".into() } else { db::quote_literal(&d) });
        if let Some(d) = &default { if let Err(e) = plan::safe_default(d, false) { unsupported.push(format!("列 {n}：{e:#}")); } }
        cols.push(column(n, ty, get(r, "nullable")? == "YES", default, opt(r, "comment")?.filter(|s| !s.is_empty())));
    }
    let indexes = s.rows(&format!("SELECT INDEX_NAME AS name,CAST(NON_UNIQUE AS CHAR) AS non_unique,COLUMN_NAME AS column_name,CAST(SEQ_IN_INDEX AS CHAR) AS ordinal,INDEX_TYPE AS method,COLLATION AS ordering,CAST(SUB_PART AS CHAR) AS prefix,EXPRESSION AS expression,IS_VISIBLE AS visible,INDEX_COMMENT AS comment FROM information_schema.STATISTICS WHERE TABLE_SCHEMA={ns} AND TABLE_NAME={name} ORDER BY INDEX_NAME,SEQ_IN_INDEX")).await?;
    let mut map = BTreeMap::<String, IndexInfo>::new();
    for r in &indexes { let n = get(r, "name")?.to_owned();
        if get(r, "method")? != "BTREE" || opt(r, "ordering")?.as_deref() != Some("A") || opt(r, "prefix")?.is_some() || opt(r, "expression")?.is_some() || get(r, "visible")? != "YES" || !get(r, "comment")?.is_empty() { unsupported.push(format!("索引 {n} 表达式/前缀/降序/不可见/注释/非BTREE未能保留")); }
        let Some(c) = opt(r, "column_name")? else { continue; };
        let ix = map.entry(n.clone()).or_insert_with(|| IndexInfo { name: n.clone(), columns: Vec::new(), unique: get(r, "non_unique").ok() == Some("0"), primary: n == "PRIMARY", method: Some("BTREE".into()) });
        ensure!(get(r, "ordinal")?.parse::<usize>()? == ix.columns.len() + 1, "STRUCTURE_INDEX_ORDER_UNKNOWN"); ix.columns.push(c);
    }
    let ix = map.into_values().collect::<Vec<_>>(); if let Some(pk) = ix.iter().find(|i| i.primary) { for c in &mut cols { c.primary_key = pk.columns.contains(&c.name); } }
    let constraints = s.rows(&format!("SELECT CONSTRAINT_SCHEMA AS schema_name,TABLE_NAME AS table_name,CONSTRAINT_NAME AS name,CONSTRAINT_TYPE AS type FROM information_schema.TABLE_CONSTRAINTS WHERE TABLE_SCHEMA={ns} AND TABLE_NAME={name} ORDER BY CONSTRAINT_NAME")).await?;
    let refs = s.rows(&format!("SELECT CONSTRAINT_SCHEMA AS schema_name,TABLE_NAME AS table_name,CONSTRAINT_NAME AS name,COLUMN_NAME AS column_name,REFERENCED_TABLE_SCHEMA AS ref_schema,REFERENCED_TABLE_NAME AS ref_table,REFERENCED_COLUMN_NAME AS ref_column,CAST(ORDINAL_POSITION AS CHAR) AS ordinal FROM information_schema.KEY_COLUMN_USAGE WHERE (TABLE_SCHEMA={ns} AND TABLE_NAME={name} AND REFERENCED_TABLE_NAME IS NOT NULL) OR (REFERENCED_TABLE_SCHEMA={ns} AND REFERENCED_TABLE_NAME={name}) ORDER BY CONSTRAINT_SCHEMA,TABLE_NAME,CONSTRAINT_NAME,ORDINAL_POSITION")).await?;
    let triggers = s.rows(&format!("SELECT TRIGGER_SCHEMA AS schema_name,TRIGGER_NAME AS name,ACTION_TIMING AS timing,EVENT_MANIPULATION AS event,ACTION_STATEMENT AS statement,SQL_MODE AS mode,DEFINER AS definer FROM information_schema.TRIGGERS WHERE EVENT_OBJECT_SCHEMA={ns} AND EVENT_OBJECT_TABLE={name} ORDER BY TRIGGER_SCHEMA,TRIGGER_NAME")).await?;
    let views = s.rows(&format!("SELECT VIEW_SCHEMA AS schema_name,VIEW_NAME AS name FROM information_schema.VIEW_TABLE_USAGE WHERE TABLE_SCHEMA={ns} AND TABLE_NAME={name} ORDER BY VIEW_SCHEMA,VIEW_NAME")).await?;
    let partitions = s.rows(&format!("SELECT PARTITION_NAME AS name,PARTITION_METHOD AS method FROM information_schema.PARTITIONS WHERE TABLE_SCHEMA={ns} AND TABLE_NAME={name} AND PARTITION_NAME IS NOT NULL ORDER BY PARTITION_ORDINAL_POSITION")).await?;
    if !refs.is_empty() || constraints.iter().any(|r| get(r, "type").ok() == Some("CHECK")) || !triggers.is_empty() || !views.is_empty() || !partitions.is_empty() { unsupported.push("CHECK/外键(含入站)/触发器/依赖视图/分区无法无损影子保留：预览不可执行，不 CASCADE".into()); }
    let schema = TableSchema { table: table.into(), database: namespace.into(), columns: cols, indexes: ix, foreign_keys: Vec::new(), comment: opt(t, "comment")?.filter(|s| !s.is_empty()), engine: opt(t, "engine")?, collation: opt(t, "collation")?, row_count: None };
    finish(physical, namespace, table, id, schema, vec![tables, ids, ddl, columns, indexes, constraints, refs, triggers, views, partitions], unsupported, namespace)
}
