//! Read-only ER metadata. Deliberately independent of editable structure validation.
use crate::db::{quote_ident, Db};
use crate::models::{ColumnInfo, DbKind, ForeignKeyInfo, IndexInfo, ObjectType, TableSchema};
use anyhow::{anyhow, ensure, Result};
use futures::{stream, StreamExt};
use serde::Serialize;
use sqlx::{MySqlPool, PgPool, Row, SqlitePool};
use std::collections::BTreeMap;

#[derive(Debug, Serialize)]
pub struct DiagramFailure {
    pub table: String,
    pub error: String,
}
#[derive(Debug, Default, Serialize)]
pub struct DiagramMetadata {
    pub tables: Vec<TableSchema>,
    pub failures: Vec<DiagramFailure>,
    pub limitations: Vec<String>,
    #[serde(skip)]
    table_positions: BTreeMap<String, usize>,
    #[serde(skip)]
    failed_phases: std::collections::BTreeSet<(String, String)>,
    #[serde(skip)]
    failure_keys: std::collections::BTreeSet<(String, String)>,
}

fn shell(namespace: &str, table: &str) -> TableSchema {
    TableSchema {
        table: table.into(),
        database: namespace.into(),
        columns: vec![],
        indexes: vec![],
        foreign_keys: vec![],
        comment: None,
        engine: None,
        collation: None,
        row_count: None,
    }
}
fn column(
    name: String,
    data_type: String,
    nullable: bool,
    default: Option<String>,
    primary_key: bool,
) -> ColumnInfo {
    ColumnInfo {
        name,
        data_type,
        nullable,
        default,
        primary_key,
        auto_increment: false,
        comment: None,
        length: None,
        precision: None,
        scale: None,
    }
}
impl DiagramMetadata {
    fn failure(&mut self, table: &str, phase: &str, error: impl std::fmt::Display) {
        self.failed_phases.insert((table.into(), phase.into()));
        let error = format!("{phase}: {error}");
        if self.failure_keys.insert((table.into(), error.clone())) {
            self.failures.push(DiagramFailure {
                table: table.into(),
                error,
            });
        }
    }
    fn batch_failure(&mut self, phase: &str, error: impl std::fmt::Display) {
        let error = error.to_string();
        for table in self
            .tables
            .iter()
            .map(|t| t.table.clone())
            .collect::<Vec<_>>()
        {
            self.failure(&table, phase, &error);
        }
    }
    fn limitation(&mut self, reason: impl Into<String>) {
        let reason = reason.into();
        if !self.limitations.contains(&reason) {
            self.limitations.push(reason);
        }
    }
    fn check_columns(&mut self) {
        for table in &mut self.tables {
            // Successful columns may be only a prefix of a composite primary key.
            // A separately decoded, complete primary index remains usable.
            if self
                .failed_phases
                .contains(&(table.table.clone(), "columns".into()))
            {
                for column in &mut table.columns {
                    column.primary_key = false;
                }
            }
        }
        let empty = self
            .tables
            .iter()
            .filter(|t| t.columns.is_empty())
            .map(|t| t.table.clone())
            .collect::<Vec<_>>();
        for table in empty {
            if !self
                .failed_phases
                .contains(&(table.clone(), "columns".into()))
            {
                self.failure(&table, "columns", "DIAGRAM_COLUMNS_UNAVAILABLE");
            }
        }
    }
    fn table_mut(&mut self, name: &str) -> Option<&mut TableSchema> {
        if self.table_positions.len() != self.tables.len() {
            self.table_positions = self
                .tables
                .iter()
                .enumerate()
                .map(|(i, t)| (t.table.clone(), i))
                .collect();
        }
        self.table_positions
            .get(name)
            .copied()
            .and_then(|i| self.tables.get_mut(i))
    }
}

pub async fn load(db: &Db, kind: DbKind, namespace: &str) -> Result<DiagramMetadata> {
    crate::sql_history::with_db(db, async {
        match db {
            Db::Sqlite(pool) => sqlite(pool, namespace).await,
            Db::Mysql(pool) => mysql(pool, namespace).await,
            Db::Postgres(pool) => postgres(pool, namespace).await,
            _ => fallback(db, kind, namespace).await,
        }
    })
    .await
}

async fn fallback(db: &Db, kind: DbKind, namespace: &str) -> Result<DiagramMetadata> {
    let names = crate::schema::list_tables(db, kind.clone(), namespace).await?;
    let results = stream::iter(
        names
            .into_iter()
            .filter(|(_, ty)| matches!(ty, ObjectType::Table)),
    )
    .map(|(name, _)| {
        let kind = kind.clone();
        async move {
            let result = crate::schema::table_schema_for_page(db, kind, namespace, &name).await;
            (name, result)
        }
    })
    .buffer_unordered(4)
    .collect::<Vec<_>>()
    .await;
    let mut out = DiagramMetadata::default();
    out.limitation(
        "当前适配器未提供关系图专用外键目录；空外键不表示已证明不存在外键，未推断关系。",
    );
    for (name, result) in results {
        match result {
            Ok(mut table) => {
                table.row_count = None;
                out.tables.push(table);
            }
            Err(error) => {
                out.tables.push(shell(namespace, &name));
                out.failure(&name, "columns", error);
            }
        }
    }
    out.tables.sort_by(|a, b| a.table.cmp(&b.table));
    out.check_columns();
    Ok(out)
}

#[derive(Debug, Clone)]
struct ForeignPart {
    table: String,
    name: String,
    ordinal: i64,
    column: String,
    ref_schema: String,
    ref_table: String,
    ref_column: String,
    on_update: Option<String>,
    on_delete: Option<String>,
}
fn foreign_groups(parts: Vec<ForeignPart>, out: &mut DiagramMetadata) {
    let mut groups: BTreeMap<(String, String), Vec<ForeignPart>> = BTreeMap::new();
    for part in parts {
        groups
            .entry((part.table.clone(), part.name.clone()))
            .or_default()
            .push(part);
    }
    for ((table, name), mut parts) in groups {
        // A decoded prefix of a broken compound key is not a real foreign key.
        if out
            .failed_phases
            .contains(&(table.clone(), "foreign_keys".into()))
        {
            continue;
        }
        parts.sort_by_key(|p| p.ordinal);
        let first = &parts[0];
        let valid = parts.iter().enumerate().all(|(i, p)| {
            p.ordinal == i as i64 + 1
                && !p.column.is_empty()
                && !p.ref_column.is_empty()
                && p.ref_schema == first.ref_schema
                && p.ref_table == first.ref_table
                && p.on_update == first.on_update
                && p.on_delete == first.on_delete
        });
        if !valid {
            out.failure(
                &table,
                "foreign_keys",
                format!("DIAGRAM_KEY_ORDER_UNPROVEN: {name}"),
            );
            continue;
        }
        let fk = ForeignKeyInfo {
            name,
            columns: parts.iter().map(|p| p.column.clone()).collect(),
            ref_schema: Some(first.ref_schema.clone()),
            ref_table: first.ref_table.clone(),
            ref_columns: parts.iter().map(|p| p.ref_column.clone()).collect(),
            on_update: first.on_update.clone(),
            on_delete: first.on_delete.clone(),
        };
        if let Some(t) = out.table_mut(&table) {
            t.foreign_keys.push(fk);
        }
    }
}
#[derive(Debug)]
struct IndexPart {
    table: String,
    name: String,
    ordinal: i64,
    column: Option<String>,
    unique: bool,
    primary: bool,
    method: Option<String>,
    simple: bool,
}
fn index_groups(parts: Vec<IndexPart>, out: &mut DiagramMetadata) {
    let mut groups: BTreeMap<(String, String), Vec<IndexPart>> = BTreeMap::new();
    for part in parts {
        groups
            .entry((part.table.clone(), part.name.clone()))
            .or_default()
            .push(part);
    }
    for ((table, name), mut parts) in groups {
        // Never turn failed trailing key metadata into a smaller unique index.
        if out
            .failed_phases
            .contains(&(table.clone(), "indexes".into()))
        {
            continue;
        }
        parts.sort_by_key(|p| p.ordinal);
        let first = &parts[0];
        if !parts.iter().enumerate().all(|(i, p)| {
            p.simple
                && p.column.as_ref().is_some_and(|c| !c.is_empty())
                && p.ordinal == i as i64 + 1
                && p.unique == first.unique
                && p.primary == first.primary
        }) {
            out.limitation("关系图仅采用完整、普通列的可靠索引判断唯一性；复杂索引已忽略，不影响字段和外键显示。");
            continue;
        }
        let index = IndexInfo {
            name,
            columns: parts.iter().filter_map(|p| p.column.clone()).collect(),
            unique: first.unique,
            primary: first.primary,
            method: first.method.clone(),
        };
        if let Some(t) = out.table_mut(&table) {
            t.indexes.push(index);
        }
    }
}
fn mysql_required(row: &sqlx::mysql::MySqlRow, name: &str) -> Result<String> {
    crate::schema::mysql_get(row, name)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("DIAGRAM_METADATA_FIELD_MISSING: {name}"))
}

const MY_TABLES: &str = "SELECT TABLE_NAME, TABLE_COMMENT, ENGINE, TABLE_COLLATION FROM information_schema.TABLES WHERE TABLE_SCHEMA = ? AND TABLE_TYPE = 'BASE TABLE' ORDER BY TABLE_NAME";
const MY_COLUMNS: &str = "SELECT TABLE_NAME, COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, COLUMN_KEY, EXTRA, COLUMN_COMMENT, CHARACTER_MAXIMUM_LENGTH, NUMERIC_PRECISION, NUMERIC_SCALE FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = ? ORDER BY TABLE_NAME, ORDINAL_POSITION";
const MY_INDEXES: &str = "SELECT TABLE_NAME, INDEX_NAME, COLUMN_NAME, NON_UNIQUE, SEQ_IN_INDEX, INDEX_TYPE, SUB_PART, COLLATION FROM information_schema.STATISTICS WHERE TABLE_SCHEMA = ? ORDER BY TABLE_NAME, INDEX_NAME, SEQ_IN_INDEX";
// As in DBX: MySQL 5.7 information_schema joins materialize the entire catalog.
const MY_FK: &str = "SELECT TABLE_NAME, CONSTRAINT_NAME, COLUMN_NAME, ORDINAL_POSITION, REFERENCED_TABLE_SCHEMA, REFERENCED_TABLE_NAME, REFERENCED_COLUMN_NAME FROM information_schema.KEY_COLUMN_USAGE WHERE TABLE_SCHEMA = ? AND REFERENCED_TABLE_NAME IS NOT NULL ORDER BY TABLE_NAME, CONSTRAINT_NAME, ORDINAL_POSITION";
const MY_RULES: &str = "SELECT TABLE_NAME, CONSTRAINT_NAME, UPDATE_RULE, DELETE_RULE FROM information_schema.REFERENTIAL_CONSTRAINTS WHERE CONSTRAINT_SCHEMA = ?";
async fn mysql(pool: &MySqlPool, namespace: &str) -> Result<DiagramMetadata> {
    let mut out = DiagramMetadata::default();
    for row in crate::sql_history::query(MY_TABLES)
        .bind(namespace)
        .fetch_all(pool)
        .await?
    {
        let name = mysql_required(&row, "TABLE_NAME")?;
        let mut t = shell(namespace, &name);
        t.comment = crate::schema::mysql_get(&row, "TABLE_COMMENT");
        t.engine = crate::schema::mysql_get(&row, "ENGINE");
        t.collation = crate::schema::mysql_get(&row, "TABLE_COLLATION");
        out.tables.push(t);
    }
    if out.tables.is_empty() {
        return Ok(out);
    }
    match crate::sql_history::query(MY_COLUMNS)
        .bind(namespace)
        .fetch_all(pool)
        .await
    {
        Err(e) => out.batch_failure("columns", e),
        Ok(rows) => {
            for row in rows {
                let table = match mysql_required(&row, "TABLE_NAME") {
                    Ok(table) => table,
                    Err(e) => {
                        out.batch_failure("columns", e);
                        continue;
                    }
                };
                let parsed = (|| -> Result<ColumnInfo> {
                    let mut c = column(
                        mysql_required(&row, "COLUMN_NAME")?,
                        mysql_required(&row, "COLUMN_TYPE")?,
                        mysql_required(&row, "IS_NULLABLE")?.eq_ignore_ascii_case("YES"),
                        crate::schema::mysql_get(&row, "COLUMN_DEFAULT"),
                        crate::schema::mysql_get(&row, "COLUMN_KEY").as_deref() == Some("PRI"),
                    );
                    c.auto_increment = crate::schema::mysql_get(&row, "EXTRA")
                        .unwrap_or_default()
                        .to_ascii_lowercase()
                        .contains("auto_increment");
                    c.comment = crate::schema::mysql_get(&row, "COLUMN_COMMENT");
                    c.length = crate::schema::mysql_get(&row, "CHARACTER_MAXIMUM_LENGTH")
                        .and_then(|s| s.parse().ok());
                    c.precision = crate::schema::mysql_get(&row, "NUMERIC_PRECISION")
                        .and_then(|s| s.parse().ok());
                    c.scale = crate::schema::mysql_get(&row, "NUMERIC_SCALE")
                        .and_then(|s| s.parse().ok());
                    Ok(c)
                })();
                match parsed {
                    Ok(c) => {
                        if let Some(t) = out.table_mut(&table) {
                            t.columns.push(c);
                        }
                    }
                    Err(e) => out.failure(&table, "columns", e),
                }
            }
        }
    }
    match crate::sql_history::query(MY_INDEXES)
        .bind(namespace)
        .fetch_all(pool)
        .await
    {
        Err(e) => out.batch_failure("indexes", e),
        Ok(rows) => {
            let mut parts = vec![];
            for row in rows {
                let table = match mysql_required(&row, "TABLE_NAME") {
                    Ok(table) => table,
                    Err(e) => {
                        out.batch_failure("indexes", e);
                        continue;
                    }
                };
                let parsed = (|| -> Result<IndexPart> {
                    let name = mysql_required(&row, "INDEX_NAME")?;
                    Ok(IndexPart {
                        table: table.clone(),
                        primary: name == "PRIMARY",
                        name,
                        ordinal: mysql_required(&row, "SEQ_IN_INDEX")?.parse()?,
                        column: crate::schema::mysql_get(&row, "COLUMN_NAME"),
                        unique: mysql_required(&row, "NON_UNIQUE")? == "0",
                        method: crate::schema::mysql_get(&row, "INDEX_TYPE"),
                        simple: crate::schema::mysql_get(&row, "SUB_PART").is_none()
                            && crate::schema::mysql_get(&row, "COLLATION").as_deref() != Some("D"),
                    })
                })();
                match parsed {
                    Ok(p) => parts.push(p),
                    Err(e) => out.failure(&table, "indexes", e),
                }
            }
            index_groups(parts, &mut out);
        }
    }
    match crate::sql_history::query(MY_FK)
        .bind(namespace)
        .fetch_all(pool)
        .await
    {
        Err(e) => out.batch_failure("foreign_keys", e),
        Ok(rows) => {
            let mut rules = BTreeMap::new();
            if !rows.is_empty() {
                match crate::sql_history::query(MY_RULES)
                    .bind(namespace)
                    .fetch_all(pool)
                    .await
                {
                    Err(e) => out.batch_failure("foreign_key_actions", e),
                    Ok(rows) => {
                        for row in rows {
                            let table = match mysql_required(&row, "TABLE_NAME") {
                                Ok(table) => table,
                                Err(e) => {
                                    out.batch_failure("foreign_key_actions", e);
                                    continue;
                                }
                            };
                            let name = match mysql_required(&row, "CONSTRAINT_NAME") {
                                Ok(name) => name,
                                Err(e) => {
                                    out.failure(&table, "foreign_key_actions", e);
                                    continue;
                                }
                            };
                            rules.insert(
                                (table, name),
                                (
                                    crate::schema::mysql_get(&row, "UPDATE_RULE"),
                                    crate::schema::mysql_get(&row, "DELETE_RULE"),
                                ),
                            );
                        }
                    }
                }
            }
            let mut parts = vec![];
            for row in rows {
                let table = match mysql_required(&row, "TABLE_NAME") {
                    Ok(table) => table,
                    Err(e) => {
                        out.batch_failure("foreign_keys", e);
                        continue;
                    }
                };
                let parsed = (|| -> Result<ForeignPart> {
                    let name = mysql_required(&row, "CONSTRAINT_NAME")?;
                    if !rules.contains_key(&(table.clone(), name.clone())) {
                        out.failure(
                            &table,
                            "foreign_key_actions",
                            format!("DIAGRAM_ACTIONS_UNAVAILABLE: {name}"),
                        );
                    }
                    let (on_update, on_delete) = rules
                        .get(&(table.clone(), name.clone()))
                        .cloned()
                        .unwrap_or_default();
                    Ok(ForeignPart {
                        table: table.clone(),
                        name,
                        ordinal: mysql_required(&row, "ORDINAL_POSITION")?.parse()?,
                        column: mysql_required(&row, "COLUMN_NAME")?,
                        ref_schema: mysql_required(&row, "REFERENCED_TABLE_SCHEMA")?,
                        ref_table: mysql_required(&row, "REFERENCED_TABLE_NAME")?,
                        ref_column: mysql_required(&row, "REFERENCED_COLUMN_NAME")?,
                        on_update,
                        on_delete,
                    })
                })();
                match parsed {
                    Ok(p) => parts.push(p),
                    Err(e) => out.failure(&table, "foreign_keys", e),
                }
            }
            foreign_groups(parts, &mut out);
        }
    }
    out.check_columns();
    Ok(out)
}

const PG_TABLES: &str = "SELECT c.relname::text AS table_name, obj_description(c.oid)::text AS comment FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relkind IN ('r','p') ORDER BY c.relname";
const PG_COLUMNS: &str = "SELECT c.relname::text AS table_name, a.attname::text AS column_name, format_type(a.atttypid,a.atttypmod)::text AS data_type, a.attnotnull, pg_get_expr(ad.adbin,ad.adrelid)::text AS default_value, (a.attidentity <> '' OR COALESCE(pg_get_expr(ad.adbin,ad.adrelid) LIKE 'nextval(%',false)) AS auto_increment, col_description(c.oid,a.attnum)::text AS comment, EXISTS(SELECT 1 FROM pg_index pk WHERE pk.indrelid=c.oid AND pk.indisprimary AND a.attnum=ANY(pk.indkey)) AS primary_key FROM pg_attribute a JOIN pg_class c ON c.oid=a.attrelid JOIN pg_namespace n ON n.oid=c.relnamespace LEFT JOIN pg_attrdef ad ON ad.adrelid=a.attrelid AND ad.adnum=a.attnum WHERE n.nspname=$1 AND c.relkind IN ('r','p') AND a.attnum>0 AND NOT a.attisdropped ORDER BY c.relname,a.attnum";
const PG_INDEXES: &str = "SELECT c.relname::text AS table_name, idx.relname::text AS index_name, x.indisprimary AS is_primary, x.indisunique AS is_unique, am.amname::text AS method, a.attname::text AS column_name, k.ordinal::bigint AS ordinal, (x.indisvalid AND x.indisready AND x.indpred IS NULL AND x.indexprs IS NULL AND x.indnatts=COALESCE((to_jsonb(x)->>'indnkeyatts')::int,x.indnatts) AND COALESCE(opt.bits,0)=0) AS simple FROM pg_index x JOIN pg_class c ON c.oid=x.indrelid JOIN pg_namespace n ON n.oid=c.relnamespace JOIN pg_class idx ON idx.oid=x.indexrelid JOIN pg_am am ON am.oid=idx.relam JOIN LATERAL unnest(x.indkey::smallint[]) WITH ORDINALITY k(attnum,ordinal) ON true LEFT JOIN LATERAL unnest(x.indoption::smallint[]) WITH ORDINALITY opt(bits,ordinal) ON opt.ordinal=k.ordinal LEFT JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum=k.attnum WHERE n.nspname=$1 AND c.relkind IN ('r','p') ORDER BY c.relname,idx.relname,k.ordinal";
const PG_FK: &str = "SELECT c.relname::text AS table_name, tc.conname::text AS constraint_name, a.attname::text AS local_column, c2.relname::text AS ref_table, n2.nspname::text AS ref_schema, af.attname::text AS ref_column, local_key.ordinal::bigint AS ordinal, tc.confupdtype::text AS update_action, tc.confdeltype::text AS delete_action FROM pg_constraint tc JOIN pg_class c ON c.oid=tc.conrelid JOIN pg_namespace n ON n.oid=c.relnamespace JOIN pg_class c2 ON c2.oid=tc.confrelid JOIN pg_namespace n2 ON n2.oid=c2.relnamespace JOIN LATERAL unnest(tc.conkey) WITH ORDINALITY local_key(attnum,ordinal) ON true JOIN LATERAL unnest(tc.confkey) WITH ORDINALITY ref_key(attnum,ordinal) ON ref_key.ordinal = local_key.ordinal JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum=local_key.attnum JOIN pg_attribute af ON af.attrelid=c2.oid AND af.attnum=ref_key.attnum WHERE tc.contype='f' AND n.nspname=$1 ORDER BY c.relname,tc.conname,local_key.ordinal";
fn pg_action(code: &str) -> Option<String> {
    match code {
        "a" => Some("NO ACTION".into()),
        "r" => Some("RESTRICT".into()),
        "c" => Some("CASCADE".into()),
        "n" => Some("SET NULL".into()),
        "d" => Some("SET DEFAULT".into()),
        _ => None,
    }
}
async fn postgres(pool: &PgPool, namespace: &str) -> Result<DiagramMetadata> {
    let mut out = DiagramMetadata::default();
    for row in crate::sql_history::query(PG_TABLES)
        .bind(namespace)
        .fetch_all(pool)
        .await?
    {
        let mut t = shell(namespace, &row.try_get::<String, _>("table_name")?);
        t.comment = row.try_get("comment")?;
        out.tables.push(t);
    }
    if out.tables.is_empty() {
        return Ok(out);
    }
    match crate::sql_history::query(PG_COLUMNS)
        .bind(namespace)
        .fetch_all(pool)
        .await
    {
        Err(e) => out.batch_failure("columns", e),
        Ok(rows) => {
            for row in rows {
                let table = match row.try_get::<String, _>("table_name") {
                    Ok(table) => table,
                    Err(e) => {
                        out.batch_failure("columns", e);
                        continue;
                    }
                };
                let parsed = (|| -> Result<ColumnInfo> {
                    let mut c = column(
                        row.try_get("column_name")?,
                        row.try_get("data_type")?,
                        !row.try_get::<bool, _>("attnotnull")?,
                        row.try_get("default_value")?,
                        row.try_get("primary_key")?,
                    );
                    c.auto_increment = row.try_get("auto_increment")?;
                    c.comment = row.try_get("comment")?;
                    Ok(c)
                })();
                match parsed {
                    Ok(c) => {
                        if let Some(t) = out.table_mut(&table) {
                            t.columns.push(c);
                        }
                    }
                    Err(e) => out.failure(&table, "columns", e),
                }
            }
        }
    }
    match crate::sql_history::query(PG_INDEXES)
        .bind(namespace)
        .fetch_all(pool)
        .await
    {
        Err(e) => out.batch_failure("indexes", e),
        Ok(rows) => {
            let mut parts = vec![];
            for row in rows {
                let table = match row.try_get::<String, _>("table_name") {
                    Ok(table) => table,
                    Err(e) => {
                        out.batch_failure("indexes", e);
                        continue;
                    }
                };
                let parsed = (|| -> Result<IndexPart> {
                    Ok(IndexPart {
                        table: table.clone(),
                        name: row.try_get("index_name")?,
                        ordinal: row.try_get("ordinal")?,
                        column: row.try_get("column_name")?,
                        unique: row.try_get("is_unique")?,
                        primary: row.try_get("is_primary")?,
                        method: row.try_get("method")?,
                        simple: row.try_get("simple")?,
                    })
                })();
                match parsed {
                    Ok(p) => parts.push(p),
                    Err(e) => out.failure(&table, "indexes", e),
                }
            }
            index_groups(parts, &mut out);
        }
    }
    match crate::sql_history::query(PG_FK)
        .bind(namespace)
        .fetch_all(pool)
        .await
    {
        Err(e) => out.batch_failure("foreign_keys", e),
        Ok(rows) => {
            let mut parts = vec![];
            for row in rows {
                let table = match row.try_get::<String, _>("table_name") {
                    Ok(table) => table,
                    Err(e) => {
                        out.batch_failure("foreign_keys", e);
                        continue;
                    }
                };
                let parsed = (|| -> Result<ForeignPart> {
                    Ok(ForeignPart {
                        table: table.clone(),
                        name: row.try_get("constraint_name")?,
                        ordinal: row.try_get("ordinal")?,
                        column: row.try_get("local_column")?,
                        ref_schema: row.try_get("ref_schema")?,
                        ref_table: row.try_get("ref_table")?,
                        ref_column: row.try_get("ref_column")?,
                        on_update: pg_action(&row.try_get::<String, _>("update_action")?),
                        on_delete: pg_action(&row.try_get::<String, _>("delete_action")?),
                    })
                })();
                match parsed {
                    Ok(p) => parts.push(p),
                    Err(e) => out.failure(&table, "foreign_keys", e),
                }
            }
            foreign_groups(parts, &mut out);
        }
    }
    out.check_columns();
    Ok(out)
}

fn pragma(namespace: &str, name: &str, object: &str) -> String {
    let kind = DbKind::of("sqlite");
    format!(
        "PRAGMA {}.{}({})",
        quote_ident(&kind, namespace),
        name,
        quote_ident(&kind, object)
    )
}
async fn sqlite(pool: &SqlitePool, namespace: &str) -> Result<DiagramMetadata> {
    // Keep attached databases and all PRAGMAs on the same physical connection.
    let namespace = if namespace.is_empty() {
        "main"
    } else {
        namespace
    };
    let mut conn = pool.acquire().await?;
    let attached = crate::sql_history::query("PRAGMA database_list")
        .fetch_all(&mut *conn)
        .await?;
    ensure!(
        namespace == "temp"
            || attached
                .iter()
                .any(|r| r.try_get::<String, _>("name").ok().as_deref() == Some(namespace)),
        "DIAGRAM_NAMESPACE_UNKNOWN: {namespace}"
    );
    let sql = format!("SELECT name FROM {}.sqlite_master WHERE type='table' AND name NOT GLOB 'sqlite_*' ORDER BY name",quote_ident(&DbKind::of("sqlite"),namespace));
    let mut out = DiagramMetadata::default();
    for row in crate::sql_history::query(&sql)
        .fetch_all(&mut *conn)
        .await?
    {
        out.tables
            .push(shell(namespace, &row.try_get::<String, _>("name")?));
    }
    let names = out
        .tables
        .iter()
        .map(|t| t.table.clone())
        .collect::<Vec<_>>();
    let canonical_tables = names
        .iter()
        .map(|name| (name.to_ascii_lowercase(), name.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut primary: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for table in &names {
        let parsed = async {
            let rows = crate::sql_history::query(&pragma(namespace, "table_xinfo", table))
                .fetch_all(&mut *conn)
                .await?;
            let mut columns = vec![];
            let mut pk = vec![];
            for row in rows {
                if row.try_get::<i64, _>("hidden")? == 1 {
                    continue;
                }
                let name: String = row.try_get("name")?;
                let order: i64 = row.try_get("pk")?;
                if order > 0 {
                    pk.push((order, name.clone()));
                }
                columns.push(column(
                    name,
                    row.try_get("type")?,
                    row.try_get::<i64, _>("notnull")? == 0,
                    row.try_get("dflt_value")?,
                    order > 0,
                ));
            }
            ensure!(!columns.is_empty(), "DIAGRAM_COLUMNS_UNAVAILABLE");
            pk.sort_by_key(|p| p.0);
            Ok::<_, anyhow::Error>((columns, pk.into_iter().map(|p| p.1).collect::<Vec<_>>()))
        }
        .await;
        match parsed {
            Err(e) => out.failure(table, "columns", e),
            Ok((columns, pk)) => {
                if let Some(t) = out.table_mut(table) {
                    t.columns = columns;
                    if !pk.is_empty() {
                        t.indexes.push(IndexInfo {
                            name: "PRIMARY KEY".into(),
                            columns: pk.clone(),
                            unique: true,
                            primary: true,
                            method: None,
                        });
                    }
                }
                primary.insert(table.clone(), pk);
            }
        }
    }
    let canonical_columns = out
        .tables
        .iter()
        .map(|table| {
            (
                table.table.clone(),
                table
                    .columns
                    .iter()
                    .map(|column| (column.name.to_ascii_lowercase(), column.name.clone()))
                    .collect::<BTreeMap<_, _>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let actual_column = |owner: &str, name: String| {
        canonical_columns
            .get(owner)
            .and_then(|columns| columns.get(&name.to_ascii_lowercase()))
            .cloned()
            .unwrap_or(name)
    };
    for table in &names {
        match crate::sql_history::query(&pragma(namespace, "index_list", table))
            .fetch_all(&mut *conn)
            .await
        {
            Err(e) => out.failure(table, "indexes", e),
            Ok(rows) => {
                for row in rows {
                    let parsed = async {
                        if row.try_get::<String, _>("origin")? == "pk" {
                            return Ok(None);
                        }
                        let name: String = row.try_get("name")?;
                        if row.try_get::<i64, _>("partial")? != 0 {
                            return Ok(None);
                        }
                        let keys =
                            crate::sql_history::query(&pragma(namespace, "index_xinfo", &name))
                                .fetch_all(&mut *conn)
                                .await?;
                        let mut columns = vec![];
                        for key in keys {
                            if key.try_get::<i64, _>("key")? == 0 {
                                continue;
                            }
                            if key.try_get::<i64, _>("cid")? < 0
                                || key.try_get::<i64, _>("desc")? != 0
                            {
                                return Ok(None);
                            }
                            columns.push((
                                key.try_get::<i64, _>("seqno")?,
                                key.try_get::<String, _>("name")?,
                            ));
                        }
                        columns.sort_by_key(|p| p.0);
                        ensure!(
                            !columns.is_empty()
                                && columns.iter().enumerate().all(|(i, p)| p.0 == i as i64),
                            "DIAGRAM_KEY_ORDER_UNPROVEN"
                        );
                        Ok::<_, anyhow::Error>(Some(IndexInfo {
                            name,
                            columns: columns.into_iter().map(|p| p.1).collect(),
                            unique: row.try_get::<i64, _>("unique")? != 0,
                            primary: false,
                            method: None,
                        }))
                    }
                    .await;
                    match parsed {
                        Ok(Some(index)) => {
                            if let Some(t) = out.table_mut(table) {
                                t.indexes.push(index);
                            }
                        }
                        Ok(None) => {
                            if row.try_get::<String, _>("origin").ok().as_deref() != Some("pk") {
                                out.limitation(
                                    "关系图忽略部分、表达式或降序索引；字段和外键仍显示。",
                                );
                            }
                        }
                        Err(e) => out.failure(table, "indexes", e),
                    }
                }
            }
        }
        match crate::sql_history::query(&pragma(namespace, "foreign_key_list", table))
            .fetch_all(&mut *conn)
            .await
        {
            Err(e) => out.failure(table, "foreign_keys", e),
            Ok(rows) => {
                let mut groups: BTreeMap<i64, Vec<sqlx::sqlite::SqliteRow>> = BTreeMap::new();
                for row in rows {
                    match row.try_get::<i64, _>("id") {
                        Ok(id) => groups.entry(id).or_default().push(row),
                        Err(e) => {
                            out.failure(table, "foreign_keys", e);
                            groups.clear();
                            break;
                        }
                    }
                }
                for (id, mut rows) in groups {
                    rows.sort_by_key(|r| r.try_get::<i64, _>("seq").unwrap_or(-1));
                    let parsed = (|| -> Result<ForeignKeyInfo> {
                        let first = &rows[0];
                        let raw_target: String = first.try_get("table")?;
                        // SQLite identifiers compare ASCII case-insensitively. Resolve through
                        // its actual catalog; never lowercase identities exposed to the UI.
                        let target = canonical_tables
                            .get(&raw_target.to_ascii_lowercase())
                            .cloned()
                            .unwrap_or(raw_target.clone());
                        let mut columns = vec![];
                        let mut refs = vec![];
                        for (i, row) in rows.iter().enumerate() {
                            ensure!(
                                row.try_get::<i64, _>("seq")? == i as i64
                                    && row.try_get::<String, _>("table")? == raw_target,
                                "DIAGRAM_KEY_ORDER_UNPROVEN"
                            );
                            columns.push(actual_column(table, row.try_get::<String, _>("from")?));
                            refs.push(
                                row.try_get::<Option<String>, _>("to")?
                                    .map(|name| actual_column(&target, name)),
                            );
                        }
                        let ref_columns = if refs.iter().all(Option::is_none) {
                            let pk = primary
                                .get(&target)
                                .ok_or_else(|| anyhow!("DIAGRAM_REFERENCE_UNPROVEN: {target}"))?;
                            ensure!(
                                pk.len() == refs.len(),
                                "DIAGRAM_REFERENCE_UNPROVEN: {target}"
                            );
                            pk.clone()
                        } else {
                            refs.into_iter()
                                .map(|c| c.ok_or_else(|| anyhow!("DIAGRAM_REFERENCE_UNPROVEN")))
                                .collect::<Result<Vec<_>>>()?
                        };
                        Ok(ForeignKeyInfo {
                            name: format!("FK_{table}_{id}"),
                            columns,
                            ref_table: target,
                            ref_schema: Some(namespace.into()),
                            ref_columns,
                            on_update: Some(first.try_get("on_update")?),
                            on_delete: Some(first.try_get("on_delete")?),
                        })
                    })();
                    match parsed {
                        Ok(fk) => {
                            if let Some(t) = out.table_mut(table) {
                                t.foreign_keys.push(fk);
                            }
                        }
                        Err(e) => out.failure(table, "foreign_keys", e),
                    }
                }
            }
        }
    }
    out.check_columns();
    Ok(out)
}

#[cfg(test)]
#[path = "schema_diagram_tests.rs"]
mod tests;
