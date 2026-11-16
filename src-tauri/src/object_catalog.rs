//! PL-19: 完整身份、真实目录及四分元数据状态。旧 list_tables/list_routines 不变。
//! 系统目录可见性不等于全库对象完备性；MySQL 空列表若无覆盖权限证明仍为 unknown。
use std::collections::{BTreeMap, BTreeSet};
use anyhow::{anyhow, bail, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Column, Row};
use crate::{db::{self, Db}, execution_contracts::Capability, models::{DbKind, Protocol}};

const SECTION_LIMIT: usize = 2000;
const PAGE_LIMIT: usize = 256;
const KINDS: [&str; 7] = ["table", "view", "materialized_view", "function", "procedure", "trigger", "sequence"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectParent { pub schema: String, pub name: String, pub kind: String }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseObjectId {
    pub connection_id: String,
    #[serde(default)] pub catalog: Option<String>,
    pub schema: String,
    pub kind: String,
    pub name: String,
    /// None 是未确定身份；Some("") 是已确定的零参数例程。
    #[serde(default)] pub identity_arguments: Option<String>,
    #[serde(default)] pub parent: Option<ObjectParent>,
}
pub fn object_key(object: &DatabaseObjectId) -> String {
    let parent = object.parent.as_ref().map(|p| (&p.schema, &p.kind, &p.name));
    serde_json::to_string(&(&object.connection_id, &object.catalog, &object.schema, &object.kind, &object.name, &object.identity_arguments, parent)).expect("object identity serialization")
}
pub fn revision<T: Serialize>(value: &T) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(value).expect("metadata serialization")))
}
fn capability(level: &str, reason: &str) -> Capability { Capability { level: level.into(), reason: reason.into() } }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectDescriptor {
    pub object: DatabaseObjectId,
    pub object_key: String,
    pub display_name: String,
    pub backend_object_id: Option<String>,
    pub owner: Option<String>,
    pub enabled: Option<bool>,
    pub definition_readable: bool,
    pub actions: BTreeMap<String, Capability>,
    pub properties: BTreeMap<String, String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataSection { pub state: String, pub reason: Option<String> }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectCatalogPage {
    pub objects: Vec<ObjectDescriptor>,
    pub completeness: String,
    pub sections: BTreeMap<String, MetadataSection>,
    pub revision: String,
    pub next_cursor: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Cursor { fingerprint: String, scope: String, revision: String, offset: usize }

fn descriptor(object: DatabaseObjectId, oid: Option<String>, owner: Option<String>, enabled: Option<bool>, readable: bool, properties: BTreeMap<String, String>, read_only: bool) -> ObjectDescriptor {
    let system = object.schema.starts_with("pg_") || object.schema == "information_schema" || object.name.starts_with("sqlite_");
    let mut actions = BTreeMap::new();
    actions.insert("read_definition".into(), capability(if readable { "supported" } else { "unsupported" }, if readable { "按完整对象身份读取；权限错误不会伪装为空" } else { "该对象定义读取器尚未实现或定义不可读" }));
    let routine = matches!(object.kind.as_str(), "function" | "procedure");
    let reason = if read_only { "只读连接禁止可能有副作用的调用或DDL" } else if system { "系统对象禁止图形化变更" } else { "需后端元数据/CAS及统一执行授权；目录支持不是写入认证" };
    actions.insert("call".into(), capability(if routine && !read_only && !system { "limited" } else { "unsupported" }, reason));
    actions.insert("replace".into(), capability(if routine && readable && !read_only && !system { "limited" } else { "unsupported" }, reason));
    for action in ["drop", "rename", "enable", "disable", "restart"] { actions.insert(action.into(), capability("unsupported", "对象写计划/依赖检查尚未接入；没有执行SQL")); }
    let display_name = if let Some(signature) = &object.identity_arguments { format!("{}({signature})", object.name) } else if let Some(parent) = &object.parent { format!("{} — {}.{}", object.name, parent.schema, parent.name) } else { object.name.clone() };
    ObjectDescriptor { object_key: object_key(&object), object, display_name, backend_object_id: oid, owner, enabled, definition_readable: readable, actions, properties }
}
fn identity(connection: &str, catalog: Option<&str>, schema: &str, kind: &str, name: &str) -> DatabaseObjectId {
    DatabaseObjectId { connection_id: connection.into(), catalog: catalog.map(str::to_owned), schema: schema.into(), kind: kind.into(), name: name.into(), identity_arguments: None, parent: None }
}
fn section(state: &str, reason: Option<String>) -> MetadataSection { MetadataSection { state: state.into(), reason } }
fn validate_id(object: &DatabaseObjectId) -> Result<()> {
    if object.connection_id.is_empty() || object.schema.is_empty() || object.name.is_empty() || [&object.schema, &object.name].iter().any(|s| s.contains('\0') || s.len() > 1024) { bail!("OBJECT_ID_INVALID: 缺少完整连接/命名空间/名称"); }
    if object.kind == "trigger" && object.parent.is_none() { bail!("OBJECT_ID_INVALID: trigger 必须带父表身份"); }
    if matches!(object.kind.as_str(), "function" | "procedure") && object.identity_arguments.is_none() { bail!("OBJECT_SIGNATURE_REQUIRED: 必须选择具体重载，零参数请传空签名"); }
    Ok(())
}

/// fingerprint 必须来自保存配置的后端摘要，不得从前端接受。游标绑定此次目录的全量revision。
/// 单类别至多2000条；超过上限返回partial+unknown，而不是声称完整分页。
pub async fn list(db: &Db, kind: &DbKind, connection: &str, catalog: Option<&str>, schema: &str, requested: &[String], cursor: Option<&str>, fingerprint: &str, read_only: bool) -> Result<ObjectCatalogPage> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if connection.is_empty() || schema.is_empty() || schema.contains('\0') || fingerprint.is_empty() { bail!("OBJECT_SCOPE_INVALID: 缺少后端目录上下文"); }
    let requested: BTreeSet<String> = if requested.is_empty() { KINDS.iter().map(|s| s.to_string()).chain((kind.protocol() == Protocol::Mysql).then(|| "event".to_string())).collect() } else { requested.iter().cloned().collect() };
    if requested.len() > 8 { bail!("OBJECT_FILTER_INVALID: 对象类别过多"); }
    let actual_catalog = match db {
        Db::Postgres(p) => Some(crate::sql_history::query_scalar::<_, String>("SELECT current_database()").fetch_one(p).await?),
        Db::Mysql(_) => Some(schema.to_string()),
        Db::Sqlite(_) => { if schema != "main" { bail!("OBJECT_NAMESPACE_UNSUPPORTED: SQLite目录目前仅main"); } None },
        _ => catalog.map(str::to_owned),
    };
    if let Some(catalog) = catalog { if actual_catalog.as_deref() != Some(catalog) { bail!("OBJECT_CATALOG_MISMATCH: 当前物理连接catalog与对象不一致"); } }
    let mut objects = Vec::new(); let mut sections = BTreeMap::new();
    for object_kind in &requested {
        let result = match db {
            Db::Sqlite(p) if matches!(object_kind.as_str(), "table" | "view" | "trigger") => sqlite_section(p, connection, schema, object_kind, read_only).await,
            Db::Postgres(p) if KINDS.contains(&object_kind.as_str()) => pg_section(p, connection, actual_catalog.as_deref(), schema, object_kind, read_only).await,
            Db::Mysql(p) if matches!(object_kind.as_str(), "table" | "view" | "function" | "procedure" | "trigger" | "event") => mysql_section(p, connection, schema, object_kind, read_only).await,
            _ => { sections.insert(object_kind.clone(), section("unsupported", Some("当前适配器未实现该对象类别目录；不是空对象列表".into()))); continue; }
        };
        match result {
            Ok(mut rows) => {
                let truncated = rows.len() > SECTION_LIMIT; rows.truncate(SECTION_LIMIT); objects.extend(rows);
                let visibility = matches!(db, Db::Mysql(_));
                sections.insert(object_kind.clone(), if truncated { section("unknown", Some("目录超过2000条资源上限，请缩小命名空间；已列出部分真实对象".into())) } else if visibility { section("unknown", Some("已读取当前账户可见对象；information_schema隐藏无权限对象，尚不能证明空列表/目录覆盖完整".into())) } else { section("known", None) });
            }
            Err(error) => { sections.insert(object_kind.clone(), section("unknown", Some(format!("METADATA_READ_FAILED: {error}")))); }
        }
    }
    objects.sort_by(|a,b| a.object_key.cmp(&b.object_key));
    let directory_revision = revision(&(&objects, &sections));
    let scope = revision(&(&connection, &actual_catalog, &schema, &requested));
    let offset = if let Some(cursor) = cursor {
        if cursor.len() > 8192 { bail!("OBJECT_CURSOR_INVALID: 游标过长"); }
        let c: Cursor = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(cursor).map_err(|_| anyhow!("OBJECT_CURSOR_INVALID"))?)?;
        if c.fingerprint != fingerprint || c.scope != scope || c.revision != directory_revision || c.offset > objects.len() { bail!("OBJECT_CURSOR_STALE: 目录/端点/筛选已变化，请重新读取"); } c.offset
    } else { 0 };
    let end = (offset + PAGE_LIMIT).min(objects.len());
    let next_cursor = if end < objects.len() { Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&Cursor { fingerprint: fingerprint.into(), scope, revision: directory_revision.clone(), offset: end })?)) } else { None };
    let completeness = if sections.values().all(|s| s.state == "known") { "complete" } else { "partial" }.into();
    Ok(ObjectCatalogPage { objects: objects[offset..end].to_vec(), sections, completeness, revision: directory_revision, next_cursor })

    }).await
}
async fn sqlite_section(pool: &sqlx::SqlitePool, connection: &str, schema: &str, kind: &str, read_only: bool) -> Result<Vec<ObjectDescriptor>> {
    let rows = crate::sql_history::query("SELECT name,tbl_name,sql FROM main.sqlite_schema WHERE type=? AND name NOT LIKE 'sqlite_%' ORDER BY name LIMIT 2001").bind(kind).fetch_all(pool).await?;
    rows.into_iter().map(|r| {
        let name: String = r.try_get("name")?; let parent: String = r.try_get("tbl_name")?;
        let mut object = identity(connection,None,schema,kind,&name);
        if kind == "trigger" { object.parent = Some(ObjectParent { schema: schema.into(), name: parent, kind: "table".into() }); }
        let sql: Option<String> = r.try_get("sql")?;
        Ok(descriptor(object,None,None,None,sql.is_some(),BTreeMap::new(),read_only))
    }).collect()
}
async fn pg_section(pool: &sqlx::PgPool, connection: &str, catalog: Option<&str>, schema: &str, kind: &str, read_only: bool) -> Result<Vec<ObjectDescriptor>> {
    let sql = match kind {
        "function" | "procedure" => "SELECT p.oid::text AS oid,p.proname AS name,pg_get_userbyid(p.proowner) AS owner,pg_get_function_identity_arguments(p.oid) AS signature, NULL::text AS parent, NULL::bool AS enabled, pg_get_function_result(p.oid) AS summary FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname=$1 AND p.prokind::text=$2 ORDER BY p.proname,p.oid LIMIT 2001",
        "trigger" => "SELECT t.oid::text AS oid,t.tgname AS name,pg_get_userbyid(c.relowner) AS owner,NULL::text AS signature,c.relname AS parent,(t.tgenabled::text <> 'D') AS enabled,pg_get_triggerdef(t.oid) AS summary FROM pg_catalog.pg_trigger t JOIN pg_catalog.pg_class c ON c.oid=t.tgrelid JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND NOT t.tgisinternal AND $2='t' ORDER BY c.relname,t.tgname LIMIT 2001",
        _ => "SELECT c.oid::text AS oid,c.relname AS name,pg_get_userbyid(c.relowner) AS owner,NULL::text AS signature,NULL::text AS parent,NULL::bool AS enabled,c.relkind::text AS summary FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relkind::text=ANY(string_to_array($2,',')) ORDER BY c.relname,c.oid LIMIT 2001",
    };
    let filter = match kind { "function"=>"f", "procedure"=>"p", "table"=>"r,p", "view"=>"v", "materialized_view"=>"m", "sequence"=>"S", "trigger"=>"t", _=>unreachable!() };
    // openGauss/GaussDB 等内核没有 pg_sequence 目录（42883/42P01），序列属性改用标准 information_schema.sequences。
    // 只探测一次，避免每个序列重复失败。
    let has_pg_sequence = kind == "sequence"
        && crate::sql_history::query("SELECT 1 AS probe FROM pg_catalog.pg_sequence LIMIT 0").execute(pool).await.is_ok();
    let rows = crate::sql_history::query(sql).bind(schema).bind(filter).fetch_all(pool).await?;
    let mut out = Vec::new();
    for row in rows {
        let name: String = row.try_get("name")?;
        let mut object = identity(connection,catalog,schema,kind,&name);
        object.identity_arguments = row.try_get("signature")?;
        if let Some(parent) = row.try_get::<Option<String>,_>("parent")? { object.parent = Some(ObjectParent { schema: schema.into(),name:parent,kind:"table".into() }); }
        let mut properties = BTreeMap::new();
        if let Some(summary) = row.try_get::<Option<String>,_>("summary")? { properties.insert("summary".into(),summary); }
        if kind == "sequence" {
            let oid: String = row.try_get("oid")?;
            let seq = if has_pg_sequence {
                crate::sql_history::query("SELECT seqstart::text AS start,seqincrement::text AS increment,seqmin::text AS min,seqmax::text AS max,seqcache::text AS cache,seqcycle::text AS cycle FROM pg_catalog.pg_sequence WHERE seqrelid=$1::bigint::oid").bind(oid.parse::<i64>()?).fetch_one(pool).await?
            } else {
                // information_schema.sequences 没有 cache 列，用默认值 '1' 占位（读回时不伪造具体 cache）。
                crate::sql_history::query("SELECT COALESCE(s.start_value::text,'1') AS start,COALESCE(s.increment::text,'1') AS increment,COALESCE(s.minimum_value::text,'1') AS min,COALESCE(s.maximum_value::text,'9223372036854775807') AS max,'1' AS cache,COALESCE(s.cycle_option::text,'NO') AS cycle FROM information_schema.sequences s WHERE s.sequence_schema=$1 AND s.sequence_name=$2").bind(schema).bind(name.as_str()).fetch_one(pool).await?
            };
            for key in ["start","increment","min","max","cache","cycle"] { properties.insert(key.into(), seq.try_get::<String,_>(key)?); }
            let dependencies: Vec<String> = crate::sql_history::query_scalar("SELECT n.nspname || '.' || c.relname || '.' || a.attname FROM pg_catalog.pg_depend d JOIN pg_catalog.pg_class c ON c.oid=d.refobjid JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace JOIN pg_catalog.pg_attribute a ON a.attrelid=c.oid AND a.attnum=d.refobjsubid WHERE d.objid=$1::bigint::oid AND d.deptype IN ('a','i') ORDER BY n.nspname,c.relname,a.attname").bind(oid.parse::<i64>()?).fetch_all(pool).await?;
            properties.insert("owned_by".into(), serde_json::to_string(&dependencies)?);
        }
        let readable = matches!(kind,"function"|"procedure"|"view"|"materialized_view"|"trigger"|"sequence");
        out.push(descriptor(object,Some(row.try_get("oid")?),Some(row.try_get("owner")?),row.try_get("enabled")?,readable,properties,read_only));
    }
    Ok(out)
}
/// 按真实列类型逐级解码。SHOW CREATE 的元数据行混有 TIMESTAMP/DATETIME 等非文本列，
/// 固定 String→BLOB 顺序会把整行读取变成 IpcError（column N: BLOB vs TIMESTAMP）；
/// 无法识别的类型返回 None（该列不展示），不阻断目录与定义读取。
fn my_text(row: &sqlx::mysql::MySqlRow, name: &str) -> Result<Option<String>> {
    let idx = row.columns().iter().position(|c| c.name().eq_ignore_ascii_case(name)).ok_or_else(|| anyhow!("METADATA_COLUMN_MISSING: {name}"))?;
    if let Ok(v) = row.try_get::<Option<String>, _>(idx) { return Ok(v); }
    if let Ok(v) = row.try_get::<Option<Vec<u8>>, _>(idx) { return v.map(String::from_utf8).transpose().map_err(Into::into); }
    if let Ok(v) = row.try_get::<Option<chrono::NaiveDateTime>, _>(idx) { return Ok(v.map(|v| v.format("%Y-%m-%d %H:%M:%S").to_string())); }
    if let Ok(v) = row.try_get::<Option<chrono::NaiveDate>, _>(idx) { return Ok(v.map(|v| v.to_string())); }
    if let Ok(v) = row.try_get::<Option<chrono::NaiveTime>, _>(idx) { return Ok(v.map(|v| v.to_string())); }
    if let Ok(v) = row.try_get::<Option<i64>, _>(idx) { return Ok(v.map(|v| v.to_string())); }
    if let Ok(v) = row.try_get::<Option<u64>, _>(idx) { return Ok(v.map(|v| v.to_string())); }
    if let Ok(v) = row.try_get::<Option<f64>, _>(idx) { return Ok(v.map(|v| v.to_string())); }
    Ok(None)
}
async fn mysql_section(pool: &sqlx::MySqlPool, connection: &str, schema: &str, kind: &str, read_only: bool) -> Result<Vec<ObjectDescriptor>> {
    let sql = match kind {
        "function"|"procedure" => "SELECT ROUTINE_NAME AS name,DEFINER AS owner,ROUTINE_DEFINITION AS definition,SQL_DATA_ACCESS AS summary FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA=? AND ROUTINE_TYPE=? ORDER BY ROUTINE_NAME LIMIT 2001",
        "trigger" => "SELECT TRIGGER_NAME AS name,DEFINER AS owner,EVENT_OBJECT_TABLE AS parent,ACTION_STATEMENT AS definition,CONCAT(ACTION_TIMING,' ',EVENT_MANIPULATION) AS summary FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA=? AND ?='TRIGGER' ORDER BY EVENT_OBJECT_TABLE,TRIGGER_NAME LIMIT 2001",
        "event" => "SELECT EVENT_NAME AS name,DEFINER AS owner,EVENT_DEFINITION AS definition,STATUS AS enabled,TIME_ZONE AS time_zone,EVENT_TYPE AS event_type,CAST(EXECUTE_AT AS CHAR) AS execute_at,INTERVAL_VALUE AS interval_value,INTERVAL_FIELD AS interval_field,ON_COMPLETION AS on_completion FROM information_schema.EVENTS WHERE EVENT_SCHEMA=? AND ?='EVENT' ORDER BY EVENT_NAME LIMIT 2001",
        _ => "SELECT TABLE_NAME AS name,TABLE_TYPE AS summary FROM information_schema.TABLES WHERE TABLE_SCHEMA=? AND TABLE_TYPE=? ORDER BY TABLE_NAME LIMIT 2001",
    };
    let filter = match kind { "function"=>"FUNCTION", "procedure"=>"PROCEDURE", "trigger"=>"TRIGGER", "event"=>"EVENT", "table"=>"BASE TABLE", "view"=>"VIEW", _=>unreachable!() };
    let rows = crate::sql_history::query(sql).bind(schema).bind(filter).fetch_all(pool).await?;
    let mut out = Vec::new();
    for row in rows {
        let name = my_text(&row,"name")?.ok_or_else(|| anyhow!("METADATA_NAME_NULL"))?;
        let mut object = identity(connection,Some(schema),schema,kind,&name);
        if matches!(kind,"function"|"procedure") { object.identity_arguments = Some(String::new()); }
        if kind == "trigger" { object.parent = Some(ObjectParent { schema:schema.into(),name:my_text(&row,"parent")?.ok_or_else(|| anyhow!("METADATA_PARENT_NULL"))?,kind:"table".into() }); }
        let has = |name:&str| row.columns().iter().any(|c| c.name().eq_ignore_ascii_case(name));
        let owner = if has("owner") { my_text(&row,"owner")? } else { None };
        let readable = has("definition") && my_text(&row,"definition")?.is_some();
        let enabled = if has("enabled") { my_text(&row,"enabled")?.map(|v| v == "ENABLED") } else { None };
        let mut properties = BTreeMap::new();
        for key in ["summary","time_zone","event_type","execute_at","interval_value","interval_field","on_completion"] { if has(key) { if let Some(v) = my_text(&row,key)? { properties.insert(key.into(),v); } } }
        out.push(descriptor(object,None,owner,enabled,readable,properties,read_only));
    }
    Ok(out)
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectDefinition { pub object: DatabaseObjectId, pub backend_object_id: Option<String>, pub definition: String, pub revision: String, pub properties: BTreeMap<String,String>, pub completeness: String }
/// trigger 必须传parent；PG routine必须传签名。没有按名称自动选首个重载的路径。
pub async fn definition(db: &Db, kind: &DbKind, object: &DatabaseObjectId) -> Result<ObjectDefinition> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    validate_id(object)?;
    let (definition,oid,properties) = match db {
        Db::Sqlite(p) => {
            if object.schema != "main" || !matches!(object.kind.as_str(),"table"|"view"|"trigger") { bail!("OBJECT_DEFINITION_UNSUPPORTED"); }
            let parent = object.parent.as_ref().map(|p| p.name.as_str());
            let value: Option<Option<String>> = crate::sql_history::query_scalar("SELECT sql FROM main.sqlite_schema WHERE type=? AND name=? AND (? IS NULL OR tbl_name=?)").bind(&object.kind).bind(&object.name).bind(parent).bind(parent).fetch_optional(p).await?;
            (value.flatten().ok_or_else(|| anyhow!("OBJECT_NOT_FOUND_OR_UNREADABLE"))?,None,BTreeMap::new())
        }
        Db::Postgres(p) => {
            let actual: String = crate::sql_history::query_scalar("SELECT current_database()").fetch_one(p).await?;
            if object.catalog.as_deref().is_some_and(|c| c != actual) { bail!("OBJECT_CATALOG_MISMATCH"); }
            if matches!(object.kind.as_str(),"function"|"procedure") {
                let prokind = if object.kind == "procedure" { "p" } else { "f" };
                let row = crate::sql_history::query("SELECT p.oid::text AS oid,pg_catalog.pg_get_functiondef(p.oid) AS definition FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname=$1 AND p.proname=$2 AND p.prokind::text=$3 AND pg_catalog.pg_get_function_identity_arguments(p.oid)=$4").bind(&object.schema).bind(&object.name).bind(prokind).bind(object.identity_arguments.as_ref().unwrap()).fetch_optional(p).await?.ok_or_else(|| anyhow!("OBJECT_NOT_FOUND: 精确重载不存在"))?;
                (row.try_get("definition")?,Some(row.try_get("oid")?),BTreeMap::new())
            } else if object.kind == "trigger" {
                let parent = object.parent.as_ref().unwrap();
                if parent.schema != object.schema || parent.kind != "table" { bail!("OBJECT_PARENT_MISMATCH"); }
                let row = crate::sql_history::query("SELECT t.oid::text AS oid,pg_catalog.pg_get_triggerdef(t.oid) AS definition FROM pg_catalog.pg_trigger t JOIN pg_catalog.pg_class c ON c.oid=t.tgrelid JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2 AND t.tgname=$3 AND NOT t.tgisinternal").bind(&object.schema).bind(&parent.name).bind(&object.name).fetch_optional(p).await?.ok_or_else(|| anyhow!("OBJECT_NOT_FOUND"))?;
                (row.try_get("definition")?,Some(row.try_get("oid")?),BTreeMap::new())
            } else if matches!(object.kind.as_str(),"view"|"materialized_view") {
                let row = crate::sql_history::query("SELECT c.oid::text AS oid,pg_catalog.pg_get_viewdef(c.oid,true) AS definition FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2 AND c.relkind::text=$3").bind(&object.schema).bind(&object.name).bind(if object.kind == "view" {"v"}else{"m"}).fetch_optional(p).await?.ok_or_else(|| anyhow!("OBJECT_NOT_FOUND"))?;
                (row.try_get("definition")?,Some(row.try_get("oid")?),BTreeMap::new())
            } else if object.kind == "sequence" {
                let rows = pg_section(p,&object.connection_id,Some(&actual),&object.schema,"sequence",true).await?;
                let found = rows.into_iter().find(|v| v.object.name == object.name).ok_or_else(|| anyhow!("OBJECT_NOT_FOUND_OR_DIRECTORY_LIMIT"))?;
                let definition = serde_json::to_string_pretty(&found.properties)?;
                (definition,found.backend_object_id,found.properties)
            } else { bail!("OBJECT_DEFINITION_UNSUPPORTED: 尚未实现该对象完整DDL读取"); }
        }
        Db::Mysql(p) => {
            if object.catalog.as_deref().is_some_and(|c| c != object.schema) { bail!("OBJECT_CATALOG_MISMATCH"); }
            let word = match object.kind.as_str() { "function"=>"FUNCTION", "procedure"=>"PROCEDURE", "trigger"=>"TRIGGER", "event"=>"EVENT", "view"=>"VIEW", "table"=>"TABLE", _=>bail!("OBJECT_DEFINITION_UNSUPPORTED") };
            if object.kind == "trigger" {
                let parent = object.parent.as_ref().unwrap();
                let actual: Option<String> = crate::sql_history::query_scalar("SELECT EVENT_OBJECT_TABLE FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA=? AND TRIGGER_NAME=?").bind(&object.schema).bind(&object.name).fetch_optional(p).await?;
                if actual.as_deref() != Some(&parent.name) || parent.schema != object.schema { bail!("OBJECT_PARENT_MISMATCH"); }
            }
            let row = crate::sql_history::query(&format!("SHOW CREATE {word} {}",db::qualified(kind,&object.schema,&object.name))).fetch_one(p).await?;
            let column = match object.kind.as_str() { "view"=>"Create View", "table"=>"Create Table", "function"=>"Create Function", "procedure"=>"Create Procedure", "trigger"=>"SQL Original Statement", "event"=>"Create Event", _=>unreachable!() };
            let def = my_text(&row,column)?.filter(|d| !d.is_empty()).ok_or_else(|| anyhow!("OBJECT_DEFINITION_UNREADABLE: SHOW CREATE未返回可读定义"))?;
            let mut props = BTreeMap::new();
            for col in row.columns() { if !col.name().eq_ignore_ascii_case(column) { if let Some(v) = my_text(&row,col.name())? { props.insert(col.name().into(),v); } } }
            (def,None,props)
        }
        _ => bail!("OBJECT_DEFINITION_UNSUPPORTED: 该适配器没有定义读取器"),
    };
    let revision = revision(&(object,&oid,&definition,&properties));
    Ok(ObjectDefinition { object:object.clone(), backend_object_id:oid,definition,revision,properties,completeness:"complete".into() })

    }).await
}

#[cfg(test)] mod tests {
    use super::*;
    fn id() -> DatabaseObjectId { identity("c",None,"main","function","f") }
    #[test] fn keys_preserve_scope_and_signature() { let a=id(); let mut b=a.clone(); b.schema="other".into(); assert_ne!(object_key(&a),object_key(&b)); b=a.clone(); b.identity_arguments=Some("integer".into()); assert_ne!(object_key(&a),object_key(&b)); b=a.clone(); b.identity_arguments=Some("".into()); assert_ne!(object_key(&a),object_key(&b)); }
    #[test] fn key_delimiters_do_not_collide() { let mut a=id(); let mut b=id(); a.connection_id="a:b".into(); a.schema="c".into(); b.connection_id="a".into(); b.schema="b:c".into(); assert_ne!(object_key(&a),object_key(&b)); }
    #[test] fn trigger_parent_is_identity() { let mut a=id(); a.kind="trigger".into(); assert!(validate_id(&a).is_err()); a.parent=Some(ObjectParent {schema:"main".into(),kind:"table".into(),name:"a".into()}); let mut b=a.clone(); b.parent.as_mut().unwrap().name="b".into(); assert_ne!(object_key(&a),object_key(&b)); }
    #[tokio::test] async fn sqlite_reads_real_objects_and_parent_definition() {
        let pool=sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        crate::sql_history::query("CREATE TABLE items(id INTEGER); CREATE VIEW item_view AS SELECT id FROM items; CREATE TRIGGER item_insert AFTER INSERT ON items BEGIN UPDATE items SET id=NEW.id WHERE id IS NULL; END;").execute(&pool).await.unwrap();
        let db=Db::Sqlite(pool);
        let page=list(&db,&DbKind::of("sqlite"),"c",None,"main",&["table".into(),"view".into(),"trigger".into(),"function".into()],None,"endpoint",false).await.unwrap();
        assert_eq!(page.objects.len(),3); assert_eq!(page.sections["table"].state,"known"); assert_eq!(page.sections["function"].state,"unsupported"); assert_eq!(page.completeness,"partial");
        let trigger=page.objects.iter().find(|d|d.object.kind=="trigger").unwrap();
        assert_eq!(trigger.object.parent.as_ref().unwrap().name,"items");
        let def=definition(&db,&DbKind::of("sqlite"),&trigger.object).await.unwrap(); assert!(def.definition.contains("AFTER INSERT"));
        let mut wrong=trigger.object.clone(); wrong.parent.as_mut().unwrap().name="other".into(); assert!(definition(&db,&DbKind::of("sqlite"),&wrong).await.is_err());
    }
    #[tokio::test] async fn sqlite_zero_is_known_and_cursor_binds_revision() {
        let pool=sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap(); let db=Db::Sqlite(pool.clone());
        let first=list(&db,&DbKind::of("sqlite"),"c",None,"main",&["table".into()],None,"ep",false).await.unwrap(); assert_eq!(first.sections["table"].state,"known"); assert!(first.objects.is_empty());
        for i in 0..257 { crate::sql_history::query(&format!("CREATE TABLE t{i}(id INTEGER)")).execute(&pool).await.unwrap(); }
        let page=list(&db,&DbKind::of("sqlite"),"c",None,"main",&["table".into()],None,"ep",false).await.unwrap(); assert_eq!(page.objects.len(),256);
        let cursor=page.next_cursor.as_deref().unwrap(); let next=list(&db,&DbKind::of("sqlite"),"c",None,"main",&["table".into()],Some(cursor),"ep",false).await.unwrap(); assert_eq!(next.objects.len(),1);
        assert!(list(&db,&DbKind::of("sqlite"),"c",None,"main",&["table".into()],Some(cursor),"changed",false).await.is_err());
        crate::sql_history::query("CREATE TABLE new_object(id INTEGER)").execute(&pool).await.unwrap(); assert!(list(&db,&DbKind::of("sqlite"),"c",None,"main",&["table".into()],Some(cursor),"ep",false).await.is_err());
    }
}
