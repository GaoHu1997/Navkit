//! T1/T11 的完整表基线、受控 SQLite DDL 与读回；授权/取消沿用 C1。
use anyhow::{anyhow, ensure, Result};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection, SqlitePool};
use futures::TryStreamExt;
use sha2::{Digest, Sha256};
use std::{collections::{BTreeMap,BTreeSet}, sync::Arc, time::Instant};
use crate::{commands::{self,AppState}, db::Db, design::{self,TableDesign,ColumnDesign,IndexDesign,ForeignKeyDesign,CheckDesign,TriggerDesign,TableOptionsDesign,DesignStep,DesignStepResult,DesignReceipt}, error::{AppError,CmdResult}, execution::{self,Entry,Binding}, execution_contracts::{ExecutionRequest,ExecutionOutcome}, manual_transaction::OrdinaryLease, models::{ColumnInfo,IndexInfo,ForeignKeyInfo,TableSchema,DbKind,MultiResult}, table_copy::{self,q}, table_objects::{self,ConstraintSet}};
use crate::structure_execution::metadata;

#[derive(Debug,Clone,Serialize)] pub struct FeatureCapability { pub supported:bool,pub reason:Option<String> }
#[derive(Debug,Clone,Serialize)] pub struct TableCapabilities {pub checks:FeatureCapability,pub triggers:FeatureCapability,pub options:FeatureCapability,pub rebuild:FeatureCapability,pub clear:FeatureCapability,pub copy_structure:FeatureCapability,pub copy_data:FeatureCapability,pub supported_options:Vec<String>,pub limitations:Vec<String>}
#[derive(Debug,Clone,Serialize)] pub struct TableDesignSnapshot {pub schema:TableSchema,pub objects:ConstraintSet,pub options:TableOptionsDesign,pub design:TableDesign,pub revision:String,pub source_fingerprint:String,pub owner_id:String,pub completeness:String,pub capabilities:TableCapabilities}
#[derive(Debug,Clone,Serialize)] pub struct TablePreview {pub request_sql:String,pub steps:Vec<DesignStep>,pub destructive:bool,pub revision:String,pub capabilities:TableCapabilities,pub limitations:Vec<String>}
#[derive(Debug,Clone,Serialize,Deserialize)] pub struct TableOperation {pub connection_id:String,pub schema:String,pub table:String,pub kind:String,#[serde(default)]pub target_table:Option<String>,pub base_revision:String,pub source_fingerprint:String}
#[derive(Debug,Clone,Serialize)] pub struct OperationPreview {#[serde(flatten)]pub preview:TablePreview,pub operation:TableOperation}
#[derive(Debug,Clone,Serialize)] pub struct TableDesignReceipt {#[serde(flatten)]pub receipt:DesignReceipt,pub snapshot:Option<TableDesignSnapshot>}
#[derive(Debug,Clone,Serialize)] pub struct TableOperationReceipt {pub kind:String,pub source_table:String,pub target_table:Option<String>,pub affected_rows:u64,pub copied_rows:u64,pub verified:bool,pub mismatches:Vec<String>,pub transaction:String,pub steps:Vec<DesignStepResult>,pub snapshot:Option<TableDesignSnapshot>}
#[derive(Serialize)] pub struct TableDesignResponse {pub outcome:ExecutionOutcome,pub receipt:Option<TableDesignReceipt>}
#[derive(Serialize)] pub struct TableOperationResponse {pub outcome:ExecutionOutcome,pub receipt:Option<TableOperationReceipt>}
#[derive(Clone)] pub(crate) enum TableWork {Design(TableDesign),Operation(TableOperation)}
pub(crate) struct BoundResult {pub result:CmdResult<MultiResult>,pub confirmed:bool,pub design:Option<TableDesignReceipt>,pub operation:Option<TableOperationReceipt>}
pub(crate) struct Stored {pub(crate) public:TableDesignSnapshot,pub(crate) ddl:String,pub(crate) indexes:Vec<(String,String)>,pub(crate) columns:Vec<(String,i64)>,pub(crate) rowid:Option<String>,pub(crate) sequence:Option<i64>,pub(crate) base:String,pub(crate) kind:DbKind,pub(crate) native:Option<crate::table_workflows_native::NativeMetadata>}
fn unknown(message:impl Into<String>)->AppError {AppError::coded("EXECUTION_EFFECTS_UNKNOWN",message)}
fn cap(supported:bool,reason:Option<&str>)->FeatureCapability {FeatureCapability{supported,reason:reason.map(str::to_string)}}
fn capabilities(readonly:bool)->TableCapabilities {let why=readonly.then_some("保存的连接为只读，禁止表修改");let c=cap(!readonly,why);TableCapabilities{checks:c.clone(),triggers:c.clone(),options:c.clone(),rebuild:c.clone(),clear:c.clone(),copy_structure:c.clone(),copy_data:c,supported_options:vec!["strict".into(),"without_rowid".into()],limitations:vec!["SQLite 没有 engine/charset/全表 collation/表注释；这些选项明确拒绝".into(),"未知列修改或含歧义表引用的触发器复制拒绝；未修改原始 DDL 逐字保留".into()]}}
fn validate_identity(schema:&str,table:&str,owner:&str)->Result<()> {ensure!(!schema.is_empty()&&!schema.contains('\0'),"TABLE_NAMESPACE_REQUIRED");q(table)?;ensure!(!table.starts_with("sqlite_"),"TABLE_SYSTEM_OBJECT_FORBIDDEN");ensure!(!owner.trim().is_empty()&&owner.len()<=128&&!owner.contains('\0'),"TABLE_OWNER_REQUIRED");Ok(())}
fn equal<T:Serialize>(a:&T,b:&T)->bool {serde_json::to_value(a).ok()==serde_json::to_value(b).ok()}
pub(crate) fn snapshot_design(schema:&TableSchema,objects:&ConstraintSet,options:&TableOptionsDesign)->Result<TableDesign> {
    let columns=schema.columns.iter().map(|c|ColumnDesign{name:c.name.clone(),orig_name:Some(c.name.clone()),data_type:c.data_type.clone(),length:None,scale:None,nullable:c.nullable,default_value:c.default.clone(),auto_increment:c.auto_increment,comment:c.comment.clone(),unsigned:false,zerofill:false}).collect();
    let indexes=schema.indexes.iter().filter(|i|!i.primary).map(|i|IndexDesign{name:i.name.clone(),columns:i.columns.clone(),unique:i.unique,method:i.method.clone(),descending:Vec::new()}).collect();
    let foreign_keys=schema.foreign_keys.iter().map(|f|ForeignKeyDesign{name:f.name.clone(),columns:f.columns.clone(),ref_table:f.ref_table.clone(),ref_schema:f.ref_schema.clone(),ref_columns:f.ref_columns.clone(),on_update:f.on_update.clone(),on_delete:f.on_delete.clone()}).collect();
    let checks=objects.checks.iter().map(|c|Ok(CheckDesign{name:c.name.clone(),orig_name:Some(c.name.clone()),expression:table_copy::check_expression(&c.definition)?,generated_name:c.generated_name})).collect::<Result<Vec<_>>>()?;
    let triggers=objects.triggers.iter().map(|t|{let ts=table_objects::tokens(&t.statement)?;let timing=ts.iter().find(|t|t.word("BEFORE")||t.word("AFTER")||t.word("INSTEAD")).map(|t|t.text.to_string()).unwrap_or_default();let event=ts.iter().find(|t|t.word("INSERT")||t.word("UPDATE")||t.word("DELETE")).map(|t|t.text.to_string()).unwrap_or_default();let begin=ts.iter().position(|t|t.word("BEGIN"));let end=ts.iter().rposition(|t|t.word("END"));let body=match(begin,end){(Some(b),Some(e))if b<e=>t.statement[ts[b].end..ts[e].start].trim().trim_end_matches(';').to_string(),_=>String::new()};Ok(TriggerDesign{name:t.name.clone(),orig_name:Some(t.name.clone()),timing,event,statement:body,definition:Some(t.statement.clone())})}).collect::<Result<Vec<_>>>()?;
    Ok(TableDesign{schema:schema.database.clone(),table:schema.table.clone(),columns,primary_key:design::ordered_primary_key(schema)?,comment:schema.comment.clone(),indexes:Some(indexes),foreign_keys:Some(foreign_keys),checks:Some(checks),triggers:Some(triggers),options:Some(options.clone()),base_revision:None,source_fingerprint:None})
}
async fn snapshot(conn:&mut SqliteConnection,pool:&SqlitePool,binding:&Binding,table:&str,read_only:bool)->Result<Stored> {
    ensure!(binding.namespace=="main","TABLE_NAMESPACE_UNSUPPORTED: SQLite 只认证 main");
    validate_identity(&binding.namespace,table,&binding.owner)?;
    let options=pool.connect_options();let physical=metadata::physical(conn,pool,&options).await?;
    let version:i64=crate::sql_history::query_scalar("PRAGMA main.schema_version").fetch_one(&mut *conn).await?;
    let raw:Option<(String,i64,Option<String>)>=crate::sql_history::query_as("SELECT type,rootpage,sql FROM main.sqlite_schema WHERE name=?").bind(table).fetch_optional(&mut *conn).await?;
    let (kind,root,ddl)=raw.ok_or_else(||anyhow!("TABLE_NOT_FOUND"))?;ensure!(kind=="table"&&root>0,"TABLE_BASE_TABLE_REQUIRED: 不接受视图/虚表");let ddl=ddl.ok_or_else(||anyhow!("TABLE_DEFINITION_UNKNOWN"))?;table_copy::table_parts(&ddl)?;
    let table_flags:Option<(i64,i64)>=crate::sql_history::query_as("SELECT wr,strict FROM pragma_table_list WHERE schema='main' AND name=? AND type='table'").bind(table).fetch_optional(&mut *conn).await?;
    let (wr,strict)=table_flags.ok_or_else(||anyhow!("TABLE_OPTIONS_UNKNOWN"))?;
    let options=TableOptionsDesign{strict:Some(strict!=0),without_rowid:Some(wr!=0),..Default::default()};
    let mut objects=table_objects::list_sqlite(conn,"main",table).await?;
    let rows=crate::sql_history::query(&format!("PRAGMA main.table_xinfo({})",q(table)?)).fetch_all(&mut *conn).await?;
    ensure!(!rows.is_empty()&&rows.len()<=4096,"TABLE_COLUMNS_UNKNOWN_OR_QUOTA");let mut columns=Vec::new();let mut hidden=Vec::new();let mut pk=Vec::new();
    let autoincrement=table_objects::tokens(&ddl)?.iter().any(|t|t.word("AUTOINCREMENT"));
    for row in rows {let name:String=row.try_get("name")?;let ty:String=row.try_get("type")?;let order:i64=row.try_get("pk")?;let generated:i64=row.try_get("hidden")?;ensure!([0,2,3].contains(&generated),"TABLE_HIDDEN_COLUMN_UNKNOWN");if order>0{pk.push((order,name.clone()));}hidden.push((name.clone(),generated));columns.push(ColumnInfo{name,data_type:ty.clone(),nullable:row.try_get::<i64,_>("notnull")?==0,default:row.try_get("dflt_value")?,primary_key:order>0,auto_increment:autoincrement&&order>0&&ty.eq_ignore_ascii_case("INTEGER"),comment:None,length:None,precision:None,scale:None});}
    pk.sort_by_key(|p|p.0);let pk:Vec<String>=pk.into_iter().map(|p|p.1).collect();
    let mut indexes=Vec::new();if !pk.is_empty(){indexes.push(IndexInfo{name:"PRIMARY KEY".into(),columns:pk,unique:true,primary:true,method:None});}
    let mut raw_indexes=Vec::new();let index_rows=crate::sql_history::query(&format!("PRAGMA main.index_list({})",q(table)?)).fetch_all(&mut *conn).await?;ensure!(index_rows.len()<=512,"TABLE_INDEX_QUOTA");
    for row in index_rows {let name:String=row.try_get("name")?;let origin:String=row.try_get("origin")?;if origin=="pk"{continue;}let members=crate::sql_history::query(&format!("PRAGMA main.index_xinfo({})",q(&name)?)).fetch_all(&mut *conn).await?;let mut names=Vec::new();for m in members {if m.try_get::<i64,_>("key")?==1{if let Some(n)=m.try_get::<Option<String>,_>("name")?{names.push(n);}}}indexes.push(IndexInfo{name:name.clone(),columns:names,unique:row.try_get::<i64,_>("unique")?!=0,primary:false,method:None});let sql:Option<String>=crate::sql_history::query_scalar("SELECT sql FROM main.sqlite_schema WHERE type='index' AND name=?").bind(&name).fetch_one(&mut *conn).await?;if let Some(sql)=sql{raw_indexes.push((name,sql));}}
    indexes.sort_by(|a,b|a.name.cmp(&b.name));raw_indexes.sort_by(|a,b|a.0.cmp(&b.0));
    let fkrows=crate::sql_history::query(&format!("PRAGMA main.foreign_key_list({})",q(table)?)).fetch_all(&mut *conn).await?;let mut foreign=BTreeMap::<i64,ForeignKeyInfo>::new();
    for row in fkrows {let id:i64=row.try_get("id")?;let fk=foreign.entry(id).or_insert(ForeignKeyInfo{name:format!("FK_{table}_{id}"),columns:Vec::new(),ref_table:row.try_get("table")?,ref_schema:Some("main".into()),ref_columns:Vec::new(),on_update:Some(row.try_get("on_update")?),on_delete:Some(row.try_get("on_delete")?)});fk.columns.push(row.try_get("from")?);fk.ref_columns.push(row.try_get::<Option<String>,_>("to")?.unwrap_or_default());}
    objects.completeness="complete".into();let schema=TableSchema{table:table.into(),database:"main".into(),columns,indexes,foreign_keys:foreign.into_values().collect(),comment:None,engine:None,collation:None,row_count:None};
    let mut design=snapshot_design(&schema,&objects,&options)?;
    let base=crate::object_catalog::revision(&(&ddl,&raw_indexes,&objects,&hidden,&options));
    let revision=crate::object_catalog::revision(&(&base,&physical,root,version,binding));design.base_revision=Some(revision.clone());design.source_fingerprint=Some(binding.fingerprint.clone());
    let sequence=if autoincrement {crate::sql_history::query_scalar("SELECT seq FROM main.sqlite_sequence WHERE name=?").bind(table).fetch_optional(&mut *conn).await?}else{None};
    let rowid=if wr==0 {Some(["rowid","_rowid_","oid"].into_iter().find(|name|!schema.columns.iter().any(|c|c.name.eq_ignore_ascii_case(name))).ok_or_else(||anyhow!("TABLE_ROWID_SHADOWED: 不能证明可无损复制隐藏 rowid"))?.to_string())}else{None};
    let public=TableDesignSnapshot{schema,objects,options,design,revision,source_fingerprint:binding.fingerprint.clone(),owner_id:binding.owner.clone(),completeness:"complete".into(),capabilities:capabilities(read_only)};
    Ok(Stored{public,ddl,indexes:raw_indexes,columns:hidden,rowid,sequence,base,kind:DbKind::of("sqlite"),native:None})
}
pub(crate) fn require_baseline(snapshot:&Stored,revision:Option<&str>,fingerprint:Option<&str>,binding:&Binding)->Result<()> {ensure!(revision==Some(snapshot.public.revision.as_str()),"TABLE_BASELINE_CHANGED: 表结构或 schema generation 已改变，请重新载入完整基线");ensure!(fingerprint==Some(binding.fingerprint.as_str()),"TABLE_SOURCE_CHANGED: 保存端点凭据/配置已改变");Ok(())}
pub(crate) fn marker(work:&TableWork)->CmdResult<String>{let (kind,value)=match work{TableWork::Design(d)=>("TABLE_DESIGN",serde_json::to_value(d)),TableWork::Operation(o)=>("TABLE_OPERATION",serde_json::to_value(o))};let value=value.map_err(AppError::from)?;let bytes=serde_json::to_vec(&value).map_err(AppError::from)?;if bytes.len()>4*1024*1024{return Err(AppError::coded("TABLE_PAYLOAD_QUOTA","表操作草稿超过上限"));}Ok(format!("{kind} {}",crate::object_catalog::revision(&value)))}
pub(crate) fn validate_work(work:&TableWork,binding:&Binding)->CmdResult<()> {let (schema,table,fp)=match work{TableWork::Design(d)=>(d.schema.as_str(),d.table.as_str(),d.source_fingerprint.as_deref()),TableWork::Operation(o)=>{if o.connection_id!=binding.connection{return Err(AppError::coded("TABLE_CONNECTION_CHANGED","草稿连接与真实窗口授权不同"));}(o.schema.as_str(),o.table.as_str(),Some(o.source_fingerprint.as_str()))}};validate_identity(schema,table,&binding.owner).map_err(AppError::from)?;if schema!=binding.namespace||fp!=Some(binding.fingerprint.as_str()){return Err(AppError::coded("TABLE_BINDING_CHANGED","命名空间/端点指纹与实际授权不同"));}Ok(())}
pub(crate) fn strip_unchanged(d:&TableDesign,b:&TableDesign)->TableDesign {let mut out=d.clone();if out.indexes.as_ref().is_some_and(|v|equal(&Some(v.clone()),&b.indexes)){out.indexes=None;}if out.foreign_keys.as_ref().is_some_and(|v|equal(&Some(v.clone()),&b.foreign_keys)){out.foreign_keys=None;}out.checks=None;out.triggers=None;out.options=None;out}
fn changed_checks(d:&TableDesign,b:&TableDesign)->bool {d.checks.as_ref().is_some_and(|c|!equal(&Some(c.clone()),&b.checks))}
fn changed_triggers(d:&TableDesign,b:&TableDesign)->bool {d.triggers.as_ref().is_some_and(|t|!equal(&Some(t.clone()),&b.triggers))}
fn options_target(before:&TableOptionsDesign,wanted:Option<&TableOptionsDesign>)->Result<TableOptionsDesign>{let mut out=before.clone();if let Some(w)=wanted {ensure!(w.engine.is_none()&&w.charset.is_none()&&w.collation.is_none()&&w.row_format.is_none()&&w.fillfactor.is_none(),"TABLE_OPTION_UNSUPPORTED: SQLite 没有 engine/charset/表collation/row_format/fillfactor");if w.strict.is_some(){out.strict=w.strict;}if w.without_rowid.is_some(){out.without_rowid=w.without_rowid;}}Ok(out)}
fn validate_column(c:&ColumnDesign)->Result<()> {q(&c.name)?;ensure!(!c.data_type.is_empty()&&c.data_type.len()<=1024,"TABLE_COLUMN_TYPE_INVALID");let ts=table_objects::tokens(&c.data_type)?;ensure!(!ts.iter().any(|t|t.symbol(";")||["CONSTRAINT","CHECK","REFERENCES","DEFAULT","PRIMARY","UNIQUE","NULL","GENERATED"].iter().any(|w|t.word(w))),"TABLE_COLUMN_TYPE_ESCAPE");ensure!(ts.iter().all(|t|t.kind==table_objects::TokenKind::Word||t.symbol("(")||t.symbol(")")||t.symbol(",")),"TABLE_COLUMN_TYPE_UNSUPPORTED");for n in [&c.length,&c.scale].into_iter().flatten(){ensure!(!n.is_empty()&&n.bytes().all(|b|b.is_ascii_digit()),"TABLE_COLUMN_SIZE_INVALID");}Ok(())}
fn rebuild_definition(before:&Stored,d:&TableDesign)->Result<String>{
    let (_,mut parts,suffix)=table_copy::table_parts(&before.ddl)?;let b=&before.public.design;
    if let Some(alias)=&before.rowid {ensure!(!d.columns.iter().any(|column|column.name.eq_ignore_ascii_case(alias)),"TABLE_REBUILD_ROWID_SHADOWED: 新字段会遮蔽用于无损复制与读回校验的隐藏rowid，拒绝发送重建DDL");}
    ensure!(d.primary_key==b.primary_key,"TABLE_REBUILD_PRIMARY_KEY_UNSUPPORTED: 此重建只认证保留原有主键");
    ensure!(d.foreign_keys.is_none()||equal(&d.foreign_keys,&b.foreign_keys),"TABLE_REBUILD_FOREIGN_KEY_EDIT_UNSUPPORTED: 未修改外键会完整保留");
    ensure!(d.columns.len()>=b.columns.len(),"TABLE_REBUILD_DROP_COLUMN_UNSUPPORTED: 不能证明依赖列删除无损");
    for old in &b.columns {let desired=d.columns.iter().find(|c|c.orig_name.as_deref()==Some(&old.name)).ok_or_else(||anyhow!("TABLE_REBUILD_COLUMN_MAPPING_REQUIRED"))?;ensure!(desired.name==old.name,"TABLE_REBUILD_RENAME_UNSUPPORTED: 请单独执行原生安全重命名");if equal(desired,old){continue;}validate_column(desired)?;ensure!(!old.auto_increment&&!b.primary_key.contains(&old.name),"TABLE_REBUILD_KEY_COLUMN_EDIT_UNSUPPORTED");let p=parts.iter_mut().find(|p|table_objects::tokens(p).ok().and_then(|t|t.first().and_then(|t|table_objects::identifier(t).ok())).as_deref()==Some(old.name.as_str())).ok_or_else(||anyhow!("TABLE_REBUILD_COLUMN_DEFINITION_UNKNOWN"))?;let ts=table_objects::tokens(p)?;ensure!(!ts.iter().skip(1).any(|t|["COLLATE","GENERATED","AS","REFERENCES","UNIQUE","PRIMARY","CHECK","CONSTRAINT","AUTOINCREMENT"].iter().any(|w|t.word(w))),"TABLE_REBUILD_COLUMN_HAS_UNMODELED_FEATURES: 该列包含未建模定义，拒绝覆盖");*p=design::column_definition(&DbKind::of("sqlite"),desired,false)?;}
    // SQLite requires column definitions before table constraints. Reconstruct the
    // column section in draft order without rewriting preserved raw definitions.
    let mut definitions=BTreeMap::new();let mut constraints=Vec::new();
    for part in parts {let ts=table_objects::tokens(&part)?;let first=ts.first().ok_or_else(||anyhow!("TABLE_REBUILD_PARTS_UNKNOWN"))?;
        if ["CONSTRAINT","PRIMARY","UNIQUE","CHECK","FOREIGN"].iter().any(|word|first.word(word)){constraints.push(part);}
        else{let name=table_objects::identifier(first)?;ensure!(b.columns.iter().any(|c|c.name==name),"TABLE_REBUILD_COLUMN_DEFINITION_UNKNOWN");ensure!(definitions.insert(name,part).is_none(),"TABLE_COLUMN_DUPLICATE");}}
    let mut parts=Vec::new();
    for desired in &d.columns {if let Some(original)=&desired.orig_name{parts.push(definitions.remove(original).ok_or_else(||anyhow!("TABLE_REBUILD_COLUMN_MAPPING_REQUIRED"))?);}
        else{validate_column(desired)?;ensure!(!b.columns.iter().any(|c|c.name==desired.name),"TABLE_COLUMN_DUPLICATE");parts.push(design::column_definition(&DbKind::of("sqlite"),desired,false)?);}}
    ensure!(definitions.is_empty(),"TABLE_REBUILD_DROP_COLUMN_UNSUPPORTED");parts.extend(constraints);
    let wanted=options_target(&before.public.options,d.options.as_ref())?;
    let suffix=if wanted==before.public.options{suffix}else{format!("){}{}",if wanted.without_rowid==Some(true){" WITHOUT ROWID"}else{""},if wanted.strict==Some(true){if wanted.without_rowid==Some(true){", STRICT"}else{" STRICT"}}else{""})};
    let mut ddl=format!("CREATE TABLE {}({}{}",q(&d.table)?,parts.join(", "),suffix);
    if changed_checks(d,b){ddl=table_copy::with_checks(&ddl,d.checks.as_ref().unwrap())?;}
    Ok(ddl)
}
fn design_steps(before:&Stored,d:&TableDesign)->Result<(Vec<DesignStep>,bool)> {
    if !before.kind.is_sqlite(){return crate::table_workflows_native::design_steps(before,d);}
    ensure!(d.table==before.public.schema.table&&d.schema=="main","TABLE_DESIGN_SCOPE_CHANGED");ensure!(d.comment.as_deref().unwrap_or("").is_empty(),"TABLE_COMMENT_UNSUPPORTED: SQLite 不支持表注释");
    let mut names=BTreeSet::new();for c in &d.columns{ensure!(names.insert(c.name.clone()),"TABLE_COLUMN_DUPLICATE");validate_column(c)?;}
    let b=&before.public.design;let options=options_target(&before.public.options,d.options.as_ref())?;
    let inserts_before_existing=d.columns.iter().enumerate().any(|(index,column)|column.orig_name.is_none()&&d.columns[index+1..].iter().any(|later|later.orig_name.is_some()));
    let old_order:Vec<_>=d.columns.iter().filter_map(|column|column.orig_name.as_deref()).collect();
    let baseline_order:Vec<_>=b.columns.iter().map(|column|column.name.as_str()).collect();
    let must_rebuild=changed_checks(d,b)||options!=before.public.options||inserts_before_existing||old_order!=baseline_order;
    let sanitized=strip_unchanged(d,b);
    let native=if must_rebuild{None}else{design::compile(&DbKind::of("sqlite"),&before.public.schema,&sanitized).ok()};
    let rebuild=must_rebuild||native.is_none();let mut steps=if rebuild {let definition=rebuild_definition(before,d)?;let tmp=format!("__navkit_rebuild_{}",&crate::object_catalog::revision(&(&before.public.revision,d))[..16]);let create=table_copy::rename_object(&definition,"TABLE",&tmp,&d.table,&d.table,false)?;let map:Vec<_>=d.columns.iter().filter_map(|c|c.orig_name.as_ref().map(|o|(c.name.clone(),o.clone()))).filter(|(n,_)|before.columns.iter().any(|(c,h)|c==n&&*h==0)).collect();let mut dest=map.iter().map(|(n,_)|q(n)).collect::<Result<Vec<_>>>()?;let mut src=map.iter().map(|(_,n)|q(n)).collect::<Result<Vec<_>>>()?;if let Some(rowid)=&before.rowid{dest.push(q(rowid)?);src.push(q(rowid)?);}ensure!(!dest.is_empty(),"TABLE_REBUILD_NO_PRESERVABLE_COLUMNS");vec![DesignStep{label:"创建完整定义的替换表".into(),sql:create},DesignStep{label:"复制原数据与隐藏rowid".into(),sql:format!("INSERT INTO {} ({}) SELECT {} FROM main.{}",q(&tmp)?,dest.join(","),src.join(","),q(&d.table)?)},DesignStep{label:"替换原表（事务内）".into(),sql:format!("DROP TABLE main.{}",q(&d.table)?)},DesignStep{label:"恢复原表身份".into(),sql:format!("ALTER TABLE main.{} RENAME TO {}",q(&tmp)?,q(&d.table)?)}]}else{native.unwrap()};
    if rebuild {for (_,sql) in &before.indexes {steps.push(DesignStep{label:"恢复完整索引定义".into(),sql:sql.clone()});}if !changed_triggers(d,b){for t in &before.public.objects.triggers{steps.push(DesignStep{label:format!("恢复触发器 {}",t.name),sql:t.statement.clone()});}}}
    if changed_triggers(d,b){let target=d.triggers.as_ref().unwrap();let mut names=BTreeSet::new();for t in target{ensure!(names.insert(t.name.clone()),"TABLE_TRIGGER_DUPLICATE");}if !rebuild{for t in &before.public.objects.triggers{steps.push(DesignStep{label:format!("更新触发器 {}",t.name),sql:format!("DROP TRIGGER main.{}",q(&t.name)?)});}}for t in target {let original=b.triggers.as_ref().unwrap().iter().find(|o|o.name==t.name&&equal(*o,t));let sql=if let Some(original)=original{original.definition.clone().ok_or_else(||anyhow!("TABLE_TRIGGER_DEFINITION_UNKNOWN"))?}else{table_copy::trigger_sql(&d.table,t)?};steps.push(DesignStep{label:format!("保存触发器 {}",t.name),sql});}}
    if rebuild {ensure!(d.indexes.is_none()||equal(&d.indexes,&b.indexes),"TABLE_REBUILD_COMBINED_INDEX_EDIT_UNSUPPORTED: 请分开保存索引与重建");}
    Ok((steps,rebuild))
}
fn operation_steps(before:&Stored,o:&TableOperation)->Result<Vec<DesignStep>> {
    if !before.kind.is_sqlite(){return crate::table_workflows_native::operation_steps(before,o);}
    if o.kind=="clear"{ensure!(o.target_table.is_none(),"TABLE_CLEAR_TARGET_FORBIDDEN");return Ok(vec![DesignStep{label:"清空表（保留所有定义）".into(),sql:format!("DELETE FROM main.{}",q(&o.table)?)}]);}
    ensure!(["copy_structure","copy_data"].contains(&o.kind.as_str()),"TABLE_OPERATION_UNSUPPORTED");let target=o.target_table.as_deref().ok_or_else(||anyhow!("TABLE_COPY_TARGET_REQUIRED"))?;q(target)?;ensure!(target!=o.table&&!target.starts_with("sqlite_"),"TABLE_COPY_TARGET_INVALID");
    let mut steps=vec![DesignStep{label:"复制完整CREATE TABLE定义".into(),sql:table_copy::rename_object(&before.ddl,"TABLE",target,&o.table,target,true)?}];
    if o.kind=="copy_data"{let mut names=before.columns.iter().filter(|(_,h)|*h==0).map(|(n,_)|q(n)).collect::<Result<Vec<_>>>()?;if let Some(rowid)=&before.rowid{names.push(q(rowid)?);}ensure!(!names.is_empty(),"TABLE_COPY_COLUMNS_UNKNOWN");steps.push(DesignStep{label:"复制数据（排除生成列，保留rowid）".into(),sql:format!("INSERT INTO main.{} ({}) SELECT {} FROM main.{}",q(target)?,names.join(","),names.join(","),q(&o.table)?)});}
    for (name,sql) in &before.indexes{let new=format!("{}_{}",target,&crate::object_catalog::revision(&(target,name))[..12]);steps.push(DesignStep{label:format!("复制索引 {name}"),sql:table_copy::rename_object(sql,"INDEX",&new,&o.table,target,true)?});}
    // 数据先写入再安装 trigger，避免导入时执行审计/副作用，完成后触发器定义完整存在。
    for trigger in &before.public.objects.triggers{let new=format!("{}_{}",target,&crate::object_catalog::revision(&(target,&trigger.name))[..12]);steps.push(DesignStep{label:format!("复制触发器 {}",trigger.name),sql:table_copy::rename_object(&trigger.statement,"TRIGGER",&new,&o.table,target,true)?});}
    Ok(steps)
}
async fn target_absent(conn:&mut SqliteConnection,target:&str)->Result<()> {let exists:i64=crate::sql_history::query_scalar("SELECT count(*) FROM main.sqlite_schema WHERE name=? COLLATE NOCASE").bind(target).fetch_one(&mut *conn).await?;ensure!(exists==0,"TABLE_COPY_TARGET_EXISTS: 目标已存在，禁止覆盖");Ok(())}
fn desired_verification(before:&Stored,after:&Stored,d:&TableDesign)->Result<()> {
    let mut native=d.clone();native.checks=None;native.triggers=None;native.options=None;native.foreign_keys=None;
    let mismatches=design::verify(&after.public.schema,&native,&DbKind::of("sqlite"));ensure!(mismatches.is_empty(),"TABLE_DESIGN_READBACK_MISMATCH: {}",mismatches.join("；"));
    let checks=d.checks.as_ref().unwrap_or(before.public.design.checks.as_ref().unwrap());let actual=after.public.design.checks.as_ref().unwrap();ensure!(checks.len()==actual.len(),"TABLE_CHECK_READBACK_COUNT_MISMATCH");for c in checks {let found=actual.iter().find(|a|a.name==c.name || c.generated_name&&a.expression==c.expression).ok_or_else(||anyhow!("TABLE_CHECK_READBACK_MISSING: {}",c.name))?;ensure!(table_copy::expression(&c.expression)?==found.expression,"TABLE_CHECK_READBACK_MISMATCH: {}",c.name);}
    let triggers=d.triggers.as_ref().unwrap_or(before.public.design.triggers.as_ref().unwrap());ensure!(triggers.len()==after.public.objects.triggers.len(),"TABLE_TRIGGER_READBACK_COUNT_MISMATCH");for t in triggers{let actual=after.public.objects.triggers.iter().find(|a|a.name==t.name).ok_or_else(||anyhow!("TABLE_TRIGGER_READBACK_MISSING"))?;let original=before.public.design.triggers.as_ref().unwrap().iter().find(|o|equal(*o,t));if let Some(original)=original{ensure!(Some(&actual.statement)==original.definition.as_ref(),"TABLE_TRIGGER_PRESERVATION_MISMATCH");}else{ensure!(actual.statement==table_copy::trigger_sql(&d.table,t)?,"TABLE_TRIGGER_READBACK_MISMATCH");}}
    ensure!(after.public.options==options_target(&before.public.options,d.options.as_ref())?,"TABLE_OPTIONS_READBACK_MISMATCH");Ok(())
}

async fn data_digest(conn:&mut SqliteConnection, table:&str, before:&Stored)->Result<String> {
    let mut names:Vec<String>=before.columns.iter().map(|(n,_)|n.clone()).collect();
    let order=if let Some(rowid)=&before.rowid {names.push(rowid.clone());vec![rowid.clone()]}else{design::ordered_primary_key(&before.public.schema)?};
    ensure!(!names.is_empty()&&!order.is_empty(),"TABLE_DATA_ORDER_UNKNOWN");
    let fields=names.iter().map(|n|{let n=q(n)?;Ok(format!("typeof({n}), hex(CAST({n} AS BLOB))"))}).collect::<Result<Vec<_>>>()?;
    let sql=format!("SELECT {} FROM main.{} ORDER BY {}",fields.join(","),q(table)?,order.iter().map(|n|q(n)).collect::<Result<Vec<_>>>()?.join(","));
    let mut rows=crate::sql_history::query(&sql).fetch(&mut *conn);let mut hash=Sha256::new();let mut count=0u64;
    while let Some(row)=rows.try_next().await? {for i in 0..names.len()*2 {let s:String=row.try_get(i)?;hash.update((s.len() as u64).to_le_bytes());hash.update(s.as_bytes());}count+=1;}
    hash.update(count.to_le_bytes());Ok(format!("{:x}",hash.finalize()))
}
async fn dependent_definition_digest(conn:&mut SqliteConnection,table:&str)->Result<String> {
    let rows:Vec<(String,String,String,Option<String>)>=crate::sql_history::query_as("SELECT type,name,tbl_name,sql FROM main.sqlite_schema WHERE tbl_name!=? AND name!=? ORDER BY type,name").bind(table).bind(table).fetch_all(&mut *conn).await?;
    Ok(crate::object_catalog::revision(&rows))
}
fn verify_copy_objects(before:&Stored,after:&Stored,target:&str)->Result<()> {
    ensure!(before.indexes.len()==after.indexes.len()&&before.public.objects.triggers.len()==after.public.objects.triggers.len(),"TABLE_COPY_OBJECT_COUNT_MISMATCH");
    for (name,sql) in &before.indexes {let new=format!("{}_{}",target,&crate::object_catalog::revision(&(target,name))[..12]);let expected=table_copy::rename_object(sql,"INDEX",&new,&before.public.schema.table,target,true)?;ensure!(after.indexes.iter().any(|(name,sql)|name==&new&&sql==&expected),"TABLE_COPY_INDEX_READBACK_MISMATCH: {name}");}
    for trigger in &before.public.objects.triggers {let new=format!("{}_{}",target,&crate::object_catalog::revision(&(target,&trigger.name))[..12]);let expected=table_copy::rename_object(&trigger.statement,"TRIGGER",&new,&before.public.schema.table,target,true)?;ensure!(after.public.objects.triggers.iter().any(|t|t.name==new&&t.statement==expected),"TABLE_COPY_TRIGGER_READBACK_MISMATCH: {}",trigger.name);}
    Ok(())
}

async fn apply_sqlite(conn:&mut SqliteConnection,pool:&SqlitePool,binding:&Binding,entry:&Arc<Entry>,work:&TableWork,output:&parking_lot::Mutex<(Option<TableDesignReceipt>,Option<TableOperationReceipt>)>)->CmdResult<MultiResult> {
    metadata::ensure_autocommit(conn).await.map_err(AppError::from)?;
    let fk:i64=crate::sql_history::query_scalar("PRAGMA foreign_keys").fetch_one(&mut *conn).await.map_err(AppError::from)?;let legacy:i64=crate::sql_history::query_scalar("PRAGMA legacy_alter_table").fetch_one(&mut *conn).await.map_err(AppError::from)?;
    // SQLite 官方 rebuild 流程：在 BEGIN 前关闭FK，保留引用身份，在COMMIT前全库foreign_key_check。
    let setup: Result<()> = async { crate::sql_history::query("PRAGMA foreign_keys=OFF").execute(&mut *conn).await?; crate::sql_history::query("PRAGMA legacy_alter_table=ON").execute(&mut *conn).await?; Ok(()) }.await;
    if let Err(e) = setup { execution::clear_sqlite(conn).await.map_err(|e|unknown(format!("TABLE_CONTROL_CLEANUP_UNKNOWN: {e:#}")))?; for sql in [format!("PRAGMA foreign_keys={fk}"),format!("PRAGMA legacy_alter_table={legacy}")] {crate::sql_history::query(&sql).execute(&mut *conn).await.map_err(|e|unknown(format!("TABLE_CONNECTION_RESTORE_UNKNOWN: {e}")))?;} return Err(AppError::from(e)); }
    let mut transaction=false;let mut committed=false;let started=Instant::now();let mut receipts=Vec::new();
    let result:Result<MultiResult>=async {
        ensure!(!entry.stopping(),"TABLE_STOPPED_BEFORE_BEGIN");crate::sql_history::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;transaction=true;
        let table=match work{TableWork::Design(d)=>&d.table,TableWork::Operation(o)=>&o.table};let before=snapshot(conn,pool,binding,table,false).await?;
        let (steps,rebuild)=match work{TableWork::Design(d)=>{require_baseline(&before,d.base_revision.as_deref(),d.source_fingerprint.as_deref(),binding)?;design_steps(&before,d)?},TableWork::Operation(o)=>{require_baseline(&before,Some(&o.base_revision),Some(&o.source_fingerprint),binding)?;if o.kind!="clear"{target_absent(conn,o.target_table.as_deref().ok_or_else(||anyhow!("TABLE_COPY_TARGET_REQUIRED"))?).await?;}(operation_steps(&before,o)?,false)}};
        let source_count:i64=crate::sql_history::query_scalar(&format!("SELECT count(*) FROM main.{}",q(table)?)).fetch_one(&mut *conn).await?;
        let preserve=rebuild||matches!(work,TableWork::Operation(o) if o.kind=="copy_data");
        let data_before=if preserve{Some(data_digest(conn,table,&before).await?)}else{None};
        let dependencies_before=if rebuild{Some(dependent_definition_digest(conn,table).await?)}else{None};
        let mut affected=0;
        for step in &steps{ensure!(!entry.stopping(),"TABLE_STOPPED_ROLLED_BACK");let result=crate::sql_history::query(&step.sql).execute(&mut *conn).await;match result{Ok(r)=>{if step.sql.starts_with("DELETE "){affected=r.rows_affected();}receipts.push(DesignStepResult{label:step.label.clone(),sql:step.sql.clone(),status:"applied".into(),error:None});},Err(e)=>{receipts.push(DesignStepResult{label:step.label.clone(),sql:step.sql.clone(),status:"failed".into(),error:Some(e.to_string())});return Err(e.into());}}}
        if rebuild||matches!(work,TableWork::Operation(o) if o.kind=="copy_data"){if let Some(seq)=before.sequence{let dest=match work{TableWork::Design(d)=>&d.table,TableWork::Operation(o)=>o.target_table.as_ref().unwrap()};crate::sql_history::query("UPDATE main.sqlite_sequence SET seq = max(seq,?) WHERE name=?").bind(seq).bind(dest).execute(&mut *conn).await?;crate::sql_history::query("INSERT INTO main.sqlite_sequence(name,seq) SELECT ?,? WHERE NOT EXISTS (SELECT 1 FROM main.sqlite_sequence WHERE name=?)").bind(dest).bind(seq).bind(dest).execute(&mut *conn).await?;}}
        ensure!(!entry.stopping(),"TABLE_STOPPED_ROLLED_BACK");
        let after_table=match work{TableWork::Design(d)=>&d.table,TableWork::Operation(o)=>o.target_table.as_ref().unwrap_or(&o.table)};let after=snapshot(conn,pool,binding,after_table,false).await?;
        let count:i64=crate::sql_history::query_scalar(&format!("SELECT count(*) FROM main.{}",q(after_table)?)).fetch_one(&mut *conn).await?;
        if let Some(expected)=data_before {ensure!(data_digest(conn,after_table,&before).await?==expected,"TABLE_DATA_PRESERVATION_MISMATCH: 行值、storage class或隐藏rowid改变");}
        if let Some(expected)=dependencies_before {ensure!(dependent_definition_digest(conn,table).await?==expected,"TABLE_DEPENDENT_DEFINITION_PRESERVATION_MISMATCH");ensure!(before.indexes==after.indexes,"TABLE_INDEX_PRESERVATION_MISMATCH");}
        match work {TableWork::Design(d)=>{desired_verification(&before,&after,d)?;if rebuild{ensure!(count==source_count,"TABLE_REBUILD_ROW_COUNT_MISMATCH");}},TableWork::Operation(o) if o.kind=="clear"=>{ensure!(count==0&&before.base==after.base,"TABLE_CLEAR_READBACK_MISMATCH");},TableWork::Operation(o)=>{let expected=table_copy::rename_object(&before.ddl,"TABLE",after_table,&o.table,after_table,true)?;ensure!(after.ddl==expected,"TABLE_COPY_DEFINITION_READBACK_MISMATCH");verify_copy_objects(&before,&after,after_table)?;ensure!(count==if o.kind=="copy_data"{source_count}else{0},"TABLE_COPY_ROW_COUNT_MISMATCH");let source_after=snapshot(conn,pool,binding,&o.table,false).await?;ensure!(before.base==source_after.base,"TABLE_COPY_SOURCE_CHANGED");}}
        if matches!(work,TableWork::Operation(o) if o.kind=="copy_data") && before.sequence.is_some(){ensure!(after.sequence==before.sequence,"TABLE_COPY_SEQUENCE_READBACK_MISMATCH");}
        let violation=crate::sql_history::query("PRAGMA main.foreign_key_check").fetch_optional(&mut *conn).await?;ensure!(violation.is_none(),"TABLE_FOREIGN_KEY_READBACK_VIOLATION");ensure!(!entry.stopping(),"TABLE_STOPPED_ROLLED_BACK");
        crate::sql_history::query("COMMIT").execute(&mut *conn).await.map_err(|e|unknown(format!("TABLE_COMMIT_UNKNOWN: {e}")))?;transaction=false;committed=true;entry.record_remote_transaction("committed");
        if entry.stopping(){return Err(unknown("TABLE_COMMIT_STOP_RACE: COMMIT已发送，不能声称回滚").into());}
        let public=after.public;
        match work{TableWork::Design(_)=>output.lock().0=Some(TableDesignReceipt{receipt:DesignReceipt{steps:receipts.clone(),verified:true,unverified:false,mismatches:Vec::new(),transaction:if steps.is_empty(){"none"}else{"committed"}.into(),schema:Some(public.schema.clone())},snapshot:Some(public)}),TableWork::Operation(o)=>output.lock().1=Some(TableOperationReceipt{kind:o.kind.clone(),source_table:o.table.clone(),target_table:o.target_table.clone(),affected_rows:affected,copied_rows:if o.kind=="copy_data"{count as u64}else{0},verified:true,mismatches:Vec::new(),transaction:"committed".into(),steps:receipts.clone(),snapshot:Some(public)})};
        Ok(MultiResult{sets:Vec::new(),messages:vec!["表操作已提交，完整定义与对象已在同一 owned 事务内读回核验".into()],elapsed_ms:started.elapsed().as_millis() as u64,spool_id:None})
    }.await;
    // 进度停止不能中断必须确认的清理和 ROLLBACK；停止后不再发送业务SQL。
    if let Err(e)=execution::clear_sqlite(conn).await{return Err(unknown(format!("TABLE_CONTROL_CLEANUP_UNKNOWN: {e:#}")));}
    let uncertain=result.as_ref().err().is_some_and(|e|e.downcast_ref::<AppError>().is_some_and(|e|e.code=="EXECUTION_EFFECTS_UNKNOWN"));
    if uncertain{return Err(unknown(format!("{}",result.unwrap_err())));}
    if transaction {if let Err(e)=crate::sql_history::query("ROLLBACK").execute(&mut *conn).await{return Err(unknown(format!("TABLE_ROLLBACK_UNKNOWN: {e}")));}metadata::ensure_autocommit(conn).await.map_err(|e|unknown(format!("TABLE_ROLLBACK_UNKNOWN: {e:#}")))?;entry.record_remote_transaction("rolled_back");}
    for sql in [format!("PRAGMA foreign_keys={fk}"),format!("PRAGMA legacy_alter_table={legacy}")] {crate::sql_history::query(&sql).execute(&mut *conn).await.map_err(|e|unknown(format!("TABLE_CONNECTION_RESTORE_UNKNOWN: {e}")))?;}
    match result {Ok(r)=>Ok(r),Err(e) if committed=>Err(unknown(format!("TABLE_POST_COMMIT_UNKNOWN: {e:#}"))),Err(e)=>{let code=if entry.stopping(){"TABLE_STOPPED_ROLLED_BACK"}else{"TABLE_FAILED_ROLLED_BACK"};Err(AppError::coded(code,format!("未提交表修改，ROLLBACK已确认：{e:#}")))}}
}
pub(crate) async fn run(db:Arc<Db>,kind:DbKind,entry:Arc<Entry>,ordinary:OrdinaryLease,work:TableWork,read_only:bool)->BoundResult {
    let __sql_history_context = crate::sql_history::db_context(db.as_ref());
    crate::sql_history::scope(__sql_history_context, async {
    if matches!(db.as_ref(),Db::Mysql(_)|Db::Postgres(_)){return crate::table_workflows_native::run(db,entry,ordinary,work,read_only).await;}
    if read_only||!kind.is_sqlite()||!matches!(db.as_ref(),Db::Sqlite(_)){return BoundResult{result:Err(AppError::coded("TABLE_CONTROLLED_ADAPTER_UNSUPPORTED","受控完整表工作流当前认证原生SQLite；其它协议明确不发送DDL")),confirmed:false,design:None,operation:None};}
    if let Err(e)=validate_work(&work,&entry.binding){return BoundResult{result:Err(e),confirmed:false,design:None,operation:None};}
    let pool=match db.as_ref(){Db::Sqlite(p)=>p.clone(),_=>unreachable!()};let published=Arc::new(parking_lot::Mutex::new((None,None)));let output=published.clone();let working=entry.clone();
    let (result,confirmed)=execution::supervise(entry.clone(),async move{let _ordinary=ordinary;
        let mut conn=match tokio::select!{biased;_=working.wait_stop()=>Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND","池等待已停止，没有发送SQL")),c=pool.acquire()=>c.map_err(AppError::from)}{Ok(c)=>c,Err(e)=>return(Err(e),true)};
        if working.stopping()||!working.start(){conn.return_to_pool().await;return(Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND","发送前已停止")),true);}
        if let Err(e)=execution::install_sqlite(&mut conn,&working,false).await{conn.close_on_drop();let _=conn.close().await;return(Err(unknown(format!("TABLE_CONTROL_INSTALL_UNKNOWN: {e:#}"))),false);}
        let (result,proof)=execution::drive(async{apply_sqlite(&mut conn,&pool,&working.binding,&working,&work,&output).await.map_err(anyhow::Error::from)},&working,||async{Ok(())}).await;
        let mut result=result.map_err(AppError::from);let cleanup=execution::clear_sqlite(&mut conn).await;
        let destroy=result.as_ref().err().is_some_and(|e|e.code=="EXECUTION_EFFECTS_UNKNOWN")||cleanup.is_err();
        if destroy{conn.close_on_drop();if let Err(e)=conn.close().await{result=Err(unknown(format!("TABLE_CONNECTION_CLOSE_UNKNOWN: {e}")));}}else{conn.return_to_pool().await;}
        if let Err(e)=cleanup{result=Err(unknown(format!("TABLE_CONTROL_CLEANUP_UNKNOWN: {e:#}")));}
        let proof=proof||result.as_ref().err().is_some_and(|e|e.code=="TABLE_STOPPED_ROLLED_BACK");(result,proof)
    }).await;
    let (design,operation)=if result.is_ok()&&!entry.stopping(){published.lock().clone()}else{(None,None)};BoundResult{result,confirmed,design,operation}

    }).await
}
async fn read_owned(db:Arc<Db>,binding:Binding,table:String,read_only:bool,ordinary:OrdinaryLease)->CmdResult<Stored>{
    let __sql_history_context = crate::sql_history::db_context(db.as_ref());
    crate::sql_history::scope(__sql_history_context, async {
    if matches!(db.as_ref(),Db::Mysql(_)|Db::Postgres(_)){return crate::table_workflows_native::read_owned(db,binding,table,read_only,ordinary).await;}
    // 即使IPC future消失，独立任务也持有lease与连接直到读事务真正排空。
    crate::sql_history::spawn(async move{let _ordinary=ordinary;let pool=match db.as_ref(){Db::Sqlite(p)=>p,_=>return Err(AppError::coded("TABLE_METADATA_ADAPTER_UNSUPPORTED","完整表设计工作流当前认证原生SQLite"))};let mut conn=pool.acquire().await.map_err(AppError::from)?;metadata::ensure_autocommit(&mut conn).await.map_err(AppError::from)?;crate::sql_history::query("BEGIN").execute(&mut *conn).await.map_err(AppError::from)?;let result=snapshot(&mut conn,pool,&binding,&table,read_only).await;let cleanup=crate::sql_history::query("ROLLBACK").execute(&mut *conn).await;if cleanup.is_err(){conn.close_on_drop();let _=conn.close().await;return Err(unknown("TABLE_METADATA_ROLLBACK_UNKNOWN"));}conn.return_to_pool().await;result.map_err(AppError::from)}).await.map_err(|e| AppError::coded("TABLE_METADATA_TASK_INTERRUPTED",format!("owned 表目录读取任务中断，读事务/连接清理未确认；未发布元数据：{e}")))?

    }).await
}
async fn read_context(state:&AppState,id:&str,schema:&str,table:&str,owner:&str,principal:&str,writable:bool)->CmdResult<Stored>{
    validate_identity(schema,table,owner).map_err(AppError::from)?;let cfg=commands::saved_config(state,id)?;if writable&&cfg.read_only{return Err(AppError::coded("READ_ONLY_VIOLATION","只读连接禁止表操作"));}let binding=commands::execution_binding(state,&cfg,principal,owner,schema)?;let ordinary=state.manual_transactions.ordinary(&[id]).await?;state.executions.require_quiescent(id)?;let db=commands::scoped_db(state,id,schema).await?;
    // MySQL 的库级读取返回 `id@database` 派生池；用基础 id 比对会误判端点已变。
    let key=commands::physical_key(id,schema,db.as_ref());
    if !state.connection_runtime.matches(&key,&binding.fingerprint,&db){return Err(AppError::coded("TABLE_RUNTIME_CHANGED","实际物理池与已保存端点不符"));}
    let saved=read_owned(db,binding.clone(),table.into(),cfg.read_only,ordinary).await?;let current=commands::saved_config(state,id)?;ensure_binding(state,&current,&binding,principal)?;Ok(saved)
}
fn ensure_binding(state:&AppState,cfg:&crate::models::ConnectionConfig,binding:&Binding,principal:&str)->CmdResult<()> {if commands::execution_binding(state,cfg,principal,&binding.owner,&binding.namespace)?!=*binding{return Err(AppError::coded("TABLE_CONTEXT_CHANGED","读取期间端点配置改变，请重新载入"));}Ok(())}
#[tauri::command]pub async fn read_table_design_snapshot(window:tauri::WebviewWindow,state:tauri::State<'_,AppState>,id:String,schema:String,table:String,owner_id:String)->CmdResult<TableDesignSnapshot>{let _activity=state.activity.enter()?;Ok(read_context(&state,&id,&schema,&table,&owner_id,window.label(),false).await?.public)}
#[tauri::command]pub async fn preview_table_design(window:tauri::WebviewWindow,state:tauri::State<'_,AppState>,id:String,design:TableDesign,owner_id:String)->CmdResult<TablePreview>{let _activity=state.activity.enter()?;let before=read_context(&state,&id,&design.schema,&design.table,&owner_id,window.label(),true).await?;let cfg=commands::saved_config(&state,&id)?;let binding=commands::execution_binding(&state,&cfg,window.label(),&owner_id,&design.schema)?;require_baseline(&before,design.base_revision.as_deref(),design.source_fingerprint.as_deref(),&binding).map_err(AppError::from)?;let (steps,rebuild)=design_steps(&before,&design).map_err(AppError::from)?;Ok(TablePreview{request_sql:marker(&TableWork::Design(design))?,steps,destructive:rebuild,revision:before.public.revision,capabilities:before.public.capabilities,limitations:Vec::new()})}
#[tauri::command]pub async fn preview_table_operation(window:tauri::WebviewWindow,state:tauri::State<'_,AppState>,operation:TableOperation,owner_id:String)->CmdResult<OperationPreview>{let _activity=state.activity.enter()?;let before=read_context(&state,&operation.connection_id,&operation.schema,&operation.table,&owner_id,window.label(),true).await?;let cfg=commands::saved_config(&state,&operation.connection_id)?;let binding=commands::execution_binding(&state,&cfg,window.label(),&owner_id,&operation.schema)?;require_baseline(&before,Some(&operation.base_revision),Some(&operation.source_fingerprint),&binding).map_err(AppError::from)?;let steps=operation_steps(&before,&operation).map_err(AppError::from)?;Ok(OperationPreview{preview:TablePreview{request_sql:marker(&TableWork::Operation(operation.clone()))?,steps,destructive:operation.kind=="clear",revision:before.public.revision,capabilities:before.public.capabilities,limitations:Vec::new()},operation})}
#[tauri::command]pub async fn execute_table_design(window:tauri::WebviewWindow,state:tauri::State<'_,AppState>,request:ExecutionRequest,design:TableDesign)->CmdResult<TableDesignResponse>{let response=crate::controlled_execution::execute(&state,request,window.label(),crate::controlled_execution::ControlledWork::Table{work:TableWork::Design(design)}).await?;Ok(TableDesignResponse{outcome:response.outcome,receipt:response.table_design})}
#[tauri::command]pub async fn execute_table_operation(window:tauri::WebviewWindow,state:tauri::State<'_,AppState>,request:ExecutionRequest,operation:TableOperation)->CmdResult<TableOperationResponse>{let response=crate::controlled_execution::execute(&state,request,window.label(),crate::controlled_execution::ControlledWork::Table{work:TableWork::Operation(operation)}).await?;Ok(TableOperationResponse{outcome:response.outcome,receipt:response.table_operation})}

#[cfg(test)]
mod tests {
    include!("table_workflows_tests.rs");
    include!("table_workflows_regression_tests.rs");
}
