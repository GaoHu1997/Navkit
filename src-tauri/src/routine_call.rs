//! PL-22: 真实参数元数据、一次性后端计划和类型绑定。执行只由execution_bound/C1承担。
use std::{collections::{BTreeMap,HashMap},str::FromStr,sync::OnceLock,time::{Duration,Instant}};
use anyhow::{anyhow,bail,Result};
use base64::{engine::general_purpose::STANDARD,Engine};
use serde::{Deserialize,Serialize};
use sqlx::{Column,Row,TypeInfo,ValueRef};
use futures::TryStreamExt;
use crate::{db::{self,Db},execution::Binding,execution_contracts::Capability,models::{DbKind,Protocol,ResultColumn,ResultSet,MultiResult},mutation::DbValue,object_catalog::{DatabaseObjectId,revision},routine_design::RoutineObjectId};

#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct RoutineParameter { pub ordinal:i32,pub name:Option<String>,pub mode:String,pub data_type:String,pub type_schema:Option<String>,pub type_name:String,pub has_default:Option<bool>,pub variadic:bool }
#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct RoutineCallMetadata { #[serde(default)] pub source_fingerprint:Option<String>, pub object:DatabaseObjectId,pub backend_object_id:Option<String>,pub revision:String,pub parameters:Vec<RoutineParameter>,pub result_type:Option<String>,pub definition_revision:String,pub completeness:String,pub call:Capability,pub notes:Vec<String> }
#[derive(Debug,Clone,Serialize,Deserialize)]
#[serde(tag="mode",content="value",rename_all="snake_case")]
pub enum RoutineArgumentValue { Value(DbValue),Default }
#[derive(Debug,Clone,Serialize,Deserialize)] pub struct RoutineArgument {pub ordinal:i32,pub argument:RoutineArgumentValue}
#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct RoutineCallDraft { #[serde(default)] pub source_fingerprint:Option<String>, pub object:DatabaseObjectId,pub metadata_revision:String,pub arguments:Vec<RoutineArgument> }
#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct RoutineCallPreview {#[serde(default)] pub source_fingerprint:Option<String>,pub plan_id:String,pub metadata_revision:String,pub object:DatabaseObjectId,pub sql_preview:String,pub parameter_count:usize,pub expires_in_ms:u64,pub warnings:Vec<String>}
#[derive(Debug,Clone,Serialize,Deserialize)] pub struct TypedResultColumn {pub name:String,pub data_type:String}
#[derive(Debug,Clone,Serialize,Deserialize)] pub struct TypedResultSet {pub columns:Vec<TypedResultColumn>,pub rows:Vec<Vec<DbValue>>,pub affected:u64}
#[derive(Debug,Clone,Serialize,Deserialize)] pub struct RoutineOutput {pub ordinal:i32,pub name:String,pub value:DbValue,pub data_type:String}
#[derive(Debug,Clone,Default,Serialize,Deserialize)] pub struct RoutineCallDetails {pub sets:Vec<TypedResultSet>,pub outputs:Vec<RoutineOutput>,pub warnings:Vec<String>}
#[derive(Clone)] pub(crate) struct CallExecution {pub binding:Binding,pub metadata:RoutineCallMetadata,pub draft:RoutineCallDraft}
struct Cached {expires:Instant,bytes:usize,work:CallExecution}
#[derive(Default)] pub struct Registry {plans:parking_lot::Mutex<HashMap<String,Cached>>}
pub fn global()->&'static Registry {static REGISTRY:OnceLock<Registry>=OnceLock::new();REGISTRY.get_or_init(Registry::default)}
fn routine_id(object:&DatabaseObjectId)->Result<RoutineObjectId>{
    if !matches!(object.kind.as_str(),"function"|"procedure")||object.parent.is_some(){bail!("ROUTINE_OBJECT_INVALID: 必须选择完整函数或过程身份");}
    let identity_arguments=object.identity_arguments.clone().ok_or_else(||anyhow!("ROUTINE_IDENTITY_REQUIRED: 未确定签名不等于零参数"))?;
    Ok(RoutineObjectId{connection_id:object.connection_id.clone(),schema:object.schema.clone(),name:object.name.clone(),kind:object.kind.clone(),identity_arguments})
}
fn finish_metadata(mut m:RoutineCallMetadata,read_only:bool,pg:bool)->RoutineCallMetadata {
    let unsupported=m.parameters.iter().any(|p|!supported_type(&p.type_name,pg)||p.type_schema.as_deref().is_some_and(|s|pg&&s!="pg_catalog")||p.variadic);
    let result_bad=m.object.kind=="function"&&m.result_type.as_ref().is_some_and(|t|!supported_type(t,pg));
    let unsupported=unsupported||m.parameters.len()>1024||m.parameters.iter().any(|p|!matches!(p.mode.as_str(),"in"|"out"|"inout"));
    let allowed=!read_only&&!unsupported&&!result_bad&&m.completeness=="complete";
    m.call=Capability{level:if allowed{"limited"}else{"unsupported"}.into(),reason:if read_only{"只读连接禁止调用可能有副作用的例程"}else if unsupported||result_bad{"自定义类型/数组/variadic/结果类型尚未证明可无损绑定，拒绝调用"}else{"真实metadata+后端typed计划；手动事务调用尚未接入，多结果集需逐驱动实库认证"}.into()};
    m.revision=revision(&(&m.object,&m.backend_object_id,&m.definition_revision,&m.parameters,&m.result_type));m
}
pub async fn metadata(db:&Db,kind:&DbKind,object:&DatabaseObjectId,read_only:bool)->Result<RoutineCallMetadata>{
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let id=routine_id(object)?;
    match db {
        Db::Postgres(pool)=>{let mut c=pool.acquire().await?;pg_metadata(&mut c,object,read_only).await},
        Db::Mysql(pool)=>{let mut c=pool.acquire().await?;mysql_metadata(&mut c,kind,object,read_only).await},
        _=>{let _=id;bail!("ROUTINE_ADAPTER_UNSUPPORTED: 当前协议没有已接入的例程参数读取器");},
    }

    }).await
}
pub(crate) async fn pg_metadata(conn:&mut sqlx::PgConnection,object:&DatabaseObjectId,read_only:bool)->Result<RoutineCallMetadata>{
    let id=routine_id(object)?;
    if let Some(catalog)=&object.catalog {let current:String=crate::sql_history::query_scalar("SELECT current_database()").fetch_one(&mut *conn).await?;if &current!=catalog{bail!("ROUTINE_CATALOG_MISMATCH");}}
    let snapshot=crate::routine_design::pg_snapshot(conn,&id).await?;
    let oid=snapshot.backend_object_id.clone().ok_or_else(||anyhow!("ROUTINE_OID_REQUIRED"))?;
    let parameters=crate::sql_history::query("SELECT q.ordinal::int AS ordinal,p.proargnames[q.ordinal::int] AS name,COALESCE(p.proargmodes[q.ordinal::int]::text,'i') AS mode,pg_catalog.format_type(q.type_oid,NULL) AS data_type,n.nspname AS type_schema,t.typname AS type_name,p.pronargs::int AS input_count,p.pronargdefaults::int AS default_count FROM pg_catalog.pg_proc p CROSS JOIN LATERAL unnest(COALESCE(p.proallargtypes,p.proargtypes::oid[])) WITH ORDINALITY AS q(type_oid,ordinal) JOIN pg_catalog.pg_type t ON t.oid=q.type_oid JOIN pg_catalog.pg_namespace n ON n.oid=t.typnamespace WHERE p.oid=$1::bigint::oid ORDER BY q.ordinal").bind(oid.parse::<i64>()?).fetch_all(&mut *conn).await?;
    let mut result=Vec::new();let mut input_position=0;
    for row in parameters {let mode:String=row.try_get("mode")?;let input=matches!(mode.as_str(),"i"|"b"|"v");if input{input_position+=1;}let input_count:i32=row.try_get("input_count")?;let default_count:i32=row.try_get("default_count")?;
        result.push(RoutineParameter{ordinal:row.try_get("ordinal")?,name:row.try_get("name")?,mode:match mode.as_str(){"i"|"v"=>"in","o"|"t"=>"out","b"=>"inout",_=>bail!("ROUTINE_PARAMETER_MODE_UNKNOWN")}.into(),data_type:row.try_get("data_type")?,type_schema:Some(row.try_get("type_schema")?),type_name:row.try_get("type_name")?,has_default:Some(input&&input_position>input_count-default_count),variadic:mode=="v"});
    }
    let result_type:Option<String>=crate::sql_history::query_scalar("SELECT t.typname FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_type t ON t.oid=p.prorettype WHERE p.oid=$1::bigint::oid").bind(oid.parse::<i64>()?).fetch_optional(&mut *conn).await?;
    Ok(finish_metadata(RoutineCallMetadata{object:object.clone(),source_fingerprint:None,backend_object_id:Some(oid),revision:String::new(),parameters:result,result_type,definition_revision:snapshot.revision,completeness:"complete".into(),call:Capability{level:String::new(),reason:String::new()},notes:vec!["默认值只允许省略尾部IN参数；NULL/空字符串必须显式区分".into()]},read_only,true))
}
pub(crate) async fn mysql_metadata(conn:&mut sqlx::MySqlConnection,kind:&DbKind,object:&DatabaseObjectId,read_only:bool)->Result<RoutineCallMetadata>{
    let id=routine_id(object)?;
    if !id.identity_arguments.is_empty()||object.catalog.as_deref().is_some_and(|s|s!=object.schema){bail!("ROUTINE_MYSQL_SCOPE_INVALID");}
    let word=if object.kind=="function"{"FUNCTION"}else{"PROCEDURE"};
    let row=crate::sql_history::query(&format!("SHOW CREATE {word} {}",db::qualified(kind,&object.schema,&object.name))).fetch_one(&mut *conn).await?;
    let name=if object.kind=="function"{"Create Function"}else{"Create Procedure"};let definition=mysql_text(&row,name)?.ok_or_else(||anyhow!("ROUTINE_DEFINITION_UNREADABLE: 无权限读取定义，不能信任空参数列表"))?;
    let mut context=BTreeMap::new();for column in row.columns(){if let Some(value)=mysql_text(&row,column.name())?{context.insert(column.name().to_string(),value);}}
    let rows=crate::sql_history::query("SELECT CAST(ORDINAL_POSITION AS SIGNED) AS ordinal,PARAMETER_NAME AS name,PARAMETER_MODE AS mode,DATA_TYPE AS data_type,DTD_IDENTIFIER AS type_details FROM information_schema.PARAMETERS WHERE SPECIFIC_SCHEMA=? AND SPECIFIC_NAME=? ORDER BY ORDINAL_POSITION").bind(&object.schema).bind(&object.name).fetch_all(&mut *conn).await?;
    let mut parameters=Vec::new();let mut result_type=None;
    for row in rows {let ordinal:i32=row.try_get("ordinal")?;let ty=mysql_text(&row,"data_type")?.ok_or_else(||anyhow!("ROUTINE_PARAMETER_TYPE_UNKNOWN"))?;
        if ordinal==0{result_type=Some(ty);continue;}
        parameters.push(RoutineParameter{ordinal,name:mysql_text(&row,"name")?,mode:mysql_text(&row,"mode")?.ok_or_else(||anyhow!("ROUTINE_PARAMETER_MODE_UNKNOWN"))?.to_ascii_lowercase(),data_type:mysql_text(&row,"type_details")?.unwrap_or_else(||ty.clone()),type_schema:None,type_name:ty,has_default:Some(false),variadic:false});
    }
    if crate::routine_design::definition_parameter_count(&definition,false)?!=parameters.len(){bail!("ROUTINE_METADATA_INCOMPLETE: 真实定义和参数目录数量不同，权限隐藏不能当零参数");}
    // SHOW CREATE已经有真实定义证据，但隐藏parameter行的权限语义尚未逐版本认证。
    if object.kind=="function" && result_type.is_none(){bail!("ROUTINE_METADATA_INCOMPLETE: 函数返回参数不可见");}
    Ok(finish_metadata(RoutineCallMetadata{object:object.clone(),source_fingerprint:None,backend_object_id:None,revision:String::new(),parameters,result_type,definition_revision:revision(&(&id,&definition,&context)),completeness:"complete".into(),call:Capability{level:String::new(),reason:String::new()},notes:vec!["MySQL OUT/INOUT通过同一pinned session的专用临时变量读取；连接结束时销毁，不复用状态".into(),"MySQL参数目录与多结果行为尚无专用实库验收".into()]},read_only,false))
}
fn mysql_text(row:&sqlx::mysql::MySqlRow,name:&str)->Result<Option<String>>{let i=row.columns().iter().position(|c|c.name().eq_ignore_ascii_case(name)).ok_or_else(||anyhow!("ROUTINE_METADATA_COLUMN_MISSING: {name}"))?;if let Ok(v)=row.try_get::<Option<String>,_>(i){return Ok(v);}let b:Option<Vec<u8>>=row.try_get(i)?;b.map(String::from_utf8).transpose().map_err(Into::into)}
fn supported_type(t:&str,pg:bool)->bool{if pg{matches!(t,"int2"|"int4"|"int8"|"numeric"|"float4"|"float8"|"bool"|"text"|"varchar"|"bpchar"|"bytea"|"date"|"time"|"timestamp"|"timestamptz"|"uuid"|"json"|"jsonb"|"void")}else{matches!(t.to_ascii_lowercase().as_str(),"tinyint"|"smallint"|"mediumint"|"int"|"integer"|"bigint"|"decimal"|"numeric"|"float"|"double"|"real"|"bool"|"boolean"|"char"|"varchar"|"text"|"tinytext"|"mediumtext"|"longtext"|"binary"|"varbinary"|"blob"|"tinyblob"|"mediumblob"|"longblob"|"date"|"time"|"datetime"|"timestamp"|"json")}}
fn validate_value(p:&RoutineParameter,value:&DbValue,pg:bool)->Result<()> {
    let t=p.type_name.as_str(); let valid=match value {
        DbValue::Null=>true,
        DbValue::Int(n)=>{if !(-9_007_199_254_740_991..=9_007_199_254_740_991).contains(n){bail!("ROUTINE_INTEGER_UNSAFE: 使用integer字符串");}matches!(t,"int2"|"int4"|"int8"|"tinyint"|"smallint"|"mediumint"|"int"|"integer"|"bigint")},
        DbValue::Integer(s)=>{if s.len()>21||s.is_empty()||s.trim()!=s||!s.trim_start_matches(['-','+']).bytes().all(|b|b.is_ascii_digit()){bail!("ROUTINE_INTEGER_INVALID");}if pg{s.parse::<i64>()?;}else if s.parse::<i64>().is_err(){s.parse::<u64>()?;}matches!(t,"int2"|"int4"|"int8"|"tinyint"|"smallint"|"mediumint"|"int"|"integer"|"bigint")},
        DbValue::Decimal(s)=>{decimal(s)?;matches!(t,"numeric"|"decimal")},
        DbValue::Float(v)=>v.is_finite()&&matches!(t,"float4"|"float8"|"float"|"double"|"real"),
        DbValue::Bool(_)=>matches!(t,"bool"|"boolean"),
        DbValue::Text(_)=>matches!(t,"text"|"varchar"|"bpchar"|"char"|"tinytext"|"mediumtext"|"longtext"),
        DbValue::Binary(s)=>{STANDARD.decode(s)?;matches!(t,"bytea"|"binary"|"varbinary"|"blob"|"tinyblob"|"mediumblob"|"longblob")},
        DbValue::Date(s)=>{chrono::NaiveDate::parse_from_str(s,"%Y-%m-%d")?;t=="date"},
        DbValue::Time(s)=>{chrono::NaiveTime::parse_from_str(s,"%H:%M:%S%.f")?;t=="time"},
        DbValue::Timestamp(s)=>{timestamp(s)?;matches!(t,"timestamp"|"datetime")},
        DbValue::Timestamptz(s)=>{chrono::DateTime::parse_from_rfc3339(s)?;pg&&t=="timestamptz"},
        DbValue::Uuid(s)=>{uuid::Uuid::parse_str(s)?;pg&&t=="uuid"},
        DbValue::Json(s)=>{serde_json::value::RawValue::from_string(s.clone())?;matches!(t,"json"|"jsonb")},
    };
    if !valid{bail!("ROUTINE_PARAMETER_TYPE_MISMATCH: 第{}参数要求{}",p.ordinal,p.data_type);}
    if serde_json::to_vec(value)?.len()>16*1024*1024{bail!("ROUTINE_VALUE_TOO_LARGE");} Ok(())
}
fn decimal(s:&str)->Result<sqlx::types::BigDecimal>{if s.is_empty()||s.len()>1024||s.trim()!=s||!s.bytes().all(|b|b.is_ascii_digit()||matches!(b,b'+'|b'-'|b'.'|b'e'|b'E')){bail!("ROUTINE_DECIMAL_INVALID");}Ok(sqlx::types::BigDecimal::from_str(s)?)}
fn timestamp(s:&str)->Result<chrono::NaiveDateTime>{chrono::NaiveDateTime::parse_from_str(s,"%Y-%m-%d %H:%M:%S%.f").or_else(|_|chrono::NaiveDateTime::parse_from_str(s,"%Y-%m-%dT%H:%M:%S%.f")).map_err(Into::into)}
fn checked_arguments(m:&RoutineCallMetadata,d:&RoutineCallDraft,pg:bool)->Result<Vec<(RoutineParameter,Option<DbValue>)>>{
    if m.call.level=="unsupported"||m.completeness!="complete"{bail!("ROUTINE_CALL_UNSUPPORTED: {}",m.call.reason);}
    if m.object!=d.object||m.revision!=d.metadata_revision{bail!("ROUTINE_METADATA_STALE: 参数/定义/签名已变化，请重新读取");}
    let mut args=BTreeMap::new();for arg in &d.arguments{if args.insert(arg.ordinal,&arg.argument).is_some(){bail!("ROUTINE_PARAMETER_DUPLICATE");}}
    let mut result=Vec::new();let mut default_started=false;
    for p in &m.parameters {if p.mode=="out"{if args.contains_key(&p.ordinal){bail!("ROUTINE_OUT_INPUT_REJECTED: OUT不接受用户输入");}result.push((p.clone(),None));continue;}
        let arg=args.remove(&p.ordinal).ok_or_else(||anyhow!("ROUTINE_PARAMETER_REQUIRED: 第{}参数未输入",p.ordinal))?;
        match arg {RoutineArgumentValue::Default=>{if !pg||p.mode!="in"||p.has_default!=Some(true){bail!("ROUTINE_DEFAULT_UNSUPPORTED");}default_started=true;result.push((p.clone(),None));},RoutineArgumentValue::Value(value)=>{if default_started{bail!("ROUTINE_DEFAULT_SUFFIX_REQUIRED: 只能省略尾部IN参数");}validate_value(p,value,pg)?;result.push((p.clone(),Some(value.clone())));}}
    }
    if !args.is_empty(){bail!("ROUTINE_PARAMETER_UNKNOWN");}
    if default_started&&m.object.kind=="procedure"&&m.parameters.iter().any(|p|p.mode=="out"){bail!("ROUTINE_DEFAULT_WITH_OUT_UNSUPPORTED: 带OUT的CALL省略默认参数未认证");} Ok(result)
}
impl Registry {
    pub fn prepare(&self,binding:Binding,metadata:RoutineCallMetadata,draft:RoutineCallDraft,kind:&DbKind)->Result<RoutineCallPreview>{
        crate::routine_design::validate_source_fingerprint(draft.source_fingerprint.as_deref(),&binding.fingerprint)?;
        crate::routine_design::validate_source_fingerprint(metadata.source_fingerprint.as_deref(),&binding.fingerprint)?;
        if binding.connection!=draft.object.connection_id||binding.namespace!=draft.object.schema{bail!("ROUTINE_SCOPE_MISMATCH");}
        let values=checked_arguments(&metadata,&draft,kind.protocol()==Protocol::Postgres)?;
        let size=serde_json::to_vec(&draft)?.len();if size>32*1024*1024{bail!("ROUTINE_PLAN_TOO_LARGE");}
        let sql_preview=call_sql(&metadata,&values,kind,"preview")?.0;
        let id=uuid::Uuid::new_v4().to_string();let mut plans=self.plans.lock();plans.retain(|_,p|p.expires>Instant::now());if plans.len()>=64||plans.values().map(|p|p.bytes).sum::<usize>()+size>64*1024*1024{bail!("ROUTINE_PLAN_CAPACITY");}
        plans.insert(id.clone(),Cached{expires:Instant::now()+Duration::from_secs(120),bytes:size,work:CallExecution{binding,metadata:metadata.clone(),draft}});
        Ok(RoutineCallPreview{source_fingerprint:metadata.source_fingerprint.clone(),plan_id:id,metadata_revision:metadata.revision,object:metadata.object,sql_preview,parameter_count:values.iter().filter(|(_,v)|v.is_some()).count(),expires_in_ms:120000,warnings:vec!["调用可能写入数据；取消/超时不保证回滚，unknown禁止重试".into(),"手动事务中的参数调用尚未接入；请先明确提交或回滚".into()]})
    }
    /// 供父ControlledWork在连接/查询之前校验计划来源；不消费计划，不执行SQL。
    pub fn validate_plan(&self,id:&str,binding:&Binding)->Result<()> {
        let plans=self.plans.lock();let p=plans.get(id).ok_or_else(||anyhow!("ROUTINE_PLAN_NOT_FOUND: 计划已经使用/不存在"))?;
        if &p.work.binding!=binding{bail!("ROUTINE_PLAN_FORBIDDEN: 窗口/页面/端点/命名空间不匹配");}
        validate_execution_source(&p.work,binding)?;
        if p.expires<=Instant::now(){bail!("ROUTINE_PLAN_EXPIRED");}
        Ok(())
    }
    pub(crate) fn take(&self,id:&str,binding:&Binding)->Result<CallExecution>{let mut plans=self.plans.lock();let p=plans.get(id).ok_or_else(||anyhow!("ROUTINE_PLAN_NOT_FOUND: 计划已经使用/不存在"))?;if &p.work.binding!=binding{bail!("ROUTINE_PLAN_FORBIDDEN: 窗口/页面/端点/命名空间不匹配");}validate_execution_source(&p.work,binding)?;if p.expires<=Instant::now(){plans.remove(id);bail!("ROUTINE_PLAN_EXPIRED");}Ok(plans.remove(id).unwrap().work)}
}
pub(crate) fn validate_execution_source(work:&CallExecution,binding:&Binding)->crate::error::CmdResult<()> {
    if &work.binding!=binding{return Err(crate::error::AppError::coded("ROUTINE_PLAN_FORBIDDEN","计划加载时的窗口/页面/端点/命名空间已变化；没有发送SQL"));}
    crate::routine_design::validate_source_fingerprint(work.metadata.source_fingerprint.as_deref(),&binding.fingerprint)?;
    crate::routine_design::validate_source_fingerprint(work.draft.source_fingerprint.as_deref(),&binding.fingerprint)
}
fn pg_cast(p:&RoutineParameter)->Result<String>{if p.type_schema.as_deref()!=Some("pg_catalog")||!supported_type(&p.type_name,true){bail!("ROUTINE_TYPE_UNSUPPORTED");}Ok(format!("pg_catalog.{}",p.type_name))}
fn call_sql(m:&RoutineCallMetadata,args:&[(RoutineParameter,Option<DbValue>)],kind:&DbKind,suffix:&str)->Result<(String,Vec<DbValue>,Vec<(i32,String)>)>{
    let pg=kind.protocol()==Protocol::Postgres;let mut placeholders=Vec::new();let mut values=Vec::new();let mut outputs=Vec::new();
    for (p,v) in args {if pg {
        if p.mode=="out" && m.object.kind=="function"{continue;}
        if let Some(v)=v {values.push(v.clone());placeholders.push(format!("${}::{}",values.len(),pg_cast(p)?));}else if p.mode=="out"{placeholders.push(format!("NULL::{}",pg_cast(p)?));}
    } else if p.mode=="out"||p.mode=="inout"{let var=format!("@navkit_{suffix}_{}",p.ordinal);placeholders.push(var.clone());outputs.push((p.ordinal,var));}else{placeholders.push("?".into());values.push(v.clone().ok_or_else(||anyhow!("ROUTINE_DEFAULT_UNSUPPORTED"))?);}}
    let full=db::qualified(kind,&m.object.schema,&m.object.name);let args=placeholders.join(", ");
    let sql=if m.object.kind=="procedure"{format!("CALL {full}({args})")}else if pg{format!("SELECT * FROM {full}({args})")}else{format!("SELECT {full}({args}) AS result")};Ok((sql,values,outputs))
}
pub(crate) fn bind_pg<'q>(q:crate::sql_history::Query<'q,sqlx::Postgres,sqlx::postgres::PgArguments>,v:&DbValue)->Result<crate::sql_history::Query<'q,sqlx::Postgres,sqlx::postgres::PgArguments>>{Ok(match v{
    DbValue::Null=>q.bind(None::<String>),DbValue::Text(s)=>q.bind(s.clone()),DbValue::Int(n)=>q.bind(*n),DbValue::Integer(s)=>q.bind(s.parse::<i64>()?),DbValue::Decimal(s)=>q.bind(decimal(s)?),DbValue::Float(v)=>q.bind(*v),DbValue::Bool(v)=>q.bind(*v),DbValue::Binary(s)=>q.bind(STANDARD.decode(s)?),DbValue::Date(s)=>q.bind(chrono::NaiveDate::parse_from_str(s,"%Y-%m-%d")?),DbValue::Time(s)=>q.bind(chrono::NaiveTime::parse_from_str(s,"%H:%M:%S%.f")?),DbValue::Timestamp(s)=>q.bind(timestamp(s)?),DbValue::Timestamptz(s)=>q.bind(chrono::DateTime::parse_from_rfc3339(s)?.with_timezone(&chrono::Utc)),DbValue::Uuid(s)=>q.bind(uuid::Uuid::parse_str(s)?),DbValue::Json(s)=>q.bind(sqlx::types::Json(serde_json::value::RawValue::from_string(s.clone())?)),})}
pub(crate) fn bind_mysql<'q>(q:crate::sql_history::Query<'q,sqlx::MySql,sqlx::mysql::MySqlArguments>,v:&DbValue)->Result<crate::sql_history::Query<'q,sqlx::MySql,sqlx::mysql::MySqlArguments>>{Ok(match v{
    DbValue::Null=>q.bind(None::<String>),DbValue::Text(s)|DbValue::Uuid(s)=>q.bind(s.clone()),DbValue::Int(n)=>q.bind(*n),DbValue::Integer(s)=>match s.parse::<i64>(){Ok(v)=>q.bind(v),Err(_)=>q.bind(s.parse::<u64>()?)},DbValue::Decimal(s)=>q.bind(decimal(s)?),DbValue::Float(v)=>q.bind(*v),DbValue::Bool(v)=>q.bind(*v),DbValue::Binary(s)=>q.bind(STANDARD.decode(s)?),DbValue::Date(s)=>q.bind(chrono::NaiveDate::parse_from_str(s,"%Y-%m-%d")?),DbValue::Time(s)=>q.bind(chrono::NaiveTime::parse_from_str(s,"%H:%M:%S%.f")?),DbValue::Timestamp(s)=>q.bind(timestamp(s)?),DbValue::Timestamptz(_)=>bail!("MySQL带时区值不能无损绑定"),DbValue::Json(s)=>q.bind(sqlx::types::Json(serde_json::value::RawValue::from_string(s.clone())?)),})}
fn pg_values(row:&sqlx::postgres::PgRow)->Result<Vec<DbValue>>{row.columns().iter().enumerate().map(|(i,c)|{if row.try_get_raw(i)?.is_null(){return Ok(DbValue::Null);}Ok(match c.type_info().name(){
    "INT2"=>DbValue::Integer(row.try_get::<i16,_>(i)?.to_string()),"INT4"=>DbValue::Integer(row.try_get::<i32,_>(i)?.to_string()),"INT8"=>DbValue::Integer(row.try_get::<i64,_>(i)?.to_string()),"NUMERIC"=>DbValue::Decimal(row.try_get::<sqlx::types::BigDecimal,_>(i)?.to_string()),"FLOAT4"=>DbValue::Float(row.try_get::<f32,_>(i)? as f64),"FLOAT8"=>DbValue::Float(row.try_get(i)?),"BOOL"=>DbValue::Bool(row.try_get(i)?),"TEXT"|"VARCHAR"|"BPCHAR"=>DbValue::Text(row.try_get(i)?),"BYTEA"=>DbValue::Binary(STANDARD.encode(row.try_get::<Vec<u8>,_>(i)?)),"DATE"=>DbValue::Date(row.try_get::<chrono::NaiveDate,_>(i)?.to_string()),"TIME"=>DbValue::Time(row.try_get::<chrono::NaiveTime,_>(i)?.to_string()),"TIMESTAMP"=>DbValue::Timestamp(row.try_get::<chrono::NaiveDateTime,_>(i)?.to_string()),"TIMESTAMPTZ"=>DbValue::Timestamptz(row.try_get::<chrono::DateTime<chrono::Utc>,_>(i)?.to_rfc3339()),"UUID"=>DbValue::Uuid(row.try_get::<uuid::Uuid,_>(i)?.to_string()),"JSON"|"JSONB"=>DbValue::Json(row.try_get::<sqlx::types::Json<Box<serde_json::value::RawValue>>,_>(i)?.0.get().to_string()),t=>bail!("ROUTINE_OUTPUT_TYPE_UNSUPPORTED: {t}，没有把解码错误当NULL"),})}).collect()}
fn mysql_values(row:&sqlx::mysql::MySqlRow)->Result<Vec<DbValue>>{row.columns().iter().enumerate().map(|(i,c)|{if row.try_get_raw(i)?.is_null(){return Ok(DbValue::Null);}let ty=c.type_info().name();Ok(match ty{
    "TINYINT"|"SMALLINT"|"MEDIUMINT"|"INT"|"BIGINT"=>DbValue::Integer(row.try_get::<i64,_>(i)?.to_string()),"TINYINT UNSIGNED"|"SMALLINT UNSIGNED"|"MEDIUMINT UNSIGNED"|"INT UNSIGNED"|"BIGINT UNSIGNED"=>DbValue::Integer(row.try_get::<u64,_>(i)?.to_string()),"DECIMAL"|"NEWDECIMAL"=>DbValue::Decimal(row.try_get::<sqlx::types::BigDecimal,_>(i)?.to_string()),"FLOAT"|"DOUBLE"=>DbValue::Float(row.try_get::<f64,_>(i)?),"VARCHAR"|"VAR_STRING"|"STRING"|"TEXT"=>DbValue::Text(row.try_get(i)?),"BLOB"|"TINYBLOB"|"MEDIUMBLOB"|"LONGBLOB"|"BINARY"|"VARBINARY"=>DbValue::Binary(STANDARD.encode(row.try_get::<Vec<u8>,_>(i)?)),"DATE"=>DbValue::Date(row.try_get::<chrono::NaiveDate,_>(i)?.to_string()),"TIME"=>DbValue::Time(row.try_get::<chrono::NaiveTime,_>(i)?.to_string()),"DATETIME"|"TIMESTAMP"=>DbValue::Timestamp(row.try_get::<chrono::NaiveDateTime,_>(i)?.to_string()),"JSON"=>DbValue::Json(row.try_get::<sqlx::types::Json<Box<serde_json::value::RawValue>>,_>(i)?.0.get().to_string()),t=>bail!("ROUTINE_OUTPUT_TYPE_UNSUPPORTED: {t}"),})}).collect()}
fn push_row(sets:&mut Vec<TypedResultSet>,names:Vec<TypedResultColumn>,values:Vec<DbValue>,rows:&mut usize,bytes:&mut usize)->Result<()> {
    if values.iter().any(|v|matches!(v,DbValue::Float(f) if !f.is_finite())){bail!("ROUTINE_OUTPUT_NONFINITE: 非有限浮点结果不能伪装成NULL");}
    *rows+=1;*bytes+=serde_json::to_vec(&values)?.len();if *rows>10_000||*bytes>50*1024*1024{return Err(crate::error::AppError::coded("EXECUTION_EFFECTS_UNKNOWN","例程结果超过10000行/50MiB；固定连接将销毁，写入结果不可推断").into());}
    if sets.last().map_or(true,|s|s.columns.len()!=names.len()||s.columns.iter().zip(&names).any(|(a,b)|a.name!=b.name||a.data_type!=b.data_type)){sets.push(TypedResultSet{columns:names,rows:vec![],affected:0});}sets.last_mut().unwrap().rows.push(values);Ok(())
}
fn multi(details:&RoutineCallDetails)->MultiResult{MultiResult{sets:details.sets.iter().map(|s|ResultSet{columns:s.columns.iter().map(|c|ResultColumn{name:c.name.clone(),data_type:c.data_type.clone(),nullable:true,primary_key:false,auto_increment:false}).collect(),rows:s.rows.iter().map(|r|r.iter().map(|v|match v{DbValue::Null=>None,DbValue::Text(s)|DbValue::Integer(s)|DbValue::Decimal(s)|DbValue::Binary(s)|DbValue::Date(s)|DbValue::Time(s)|DbValue::Timestamp(s)|DbValue::Timestamptz(s)|DbValue::Uuid(s)|DbValue::Json(s)=>Some(s.clone()),DbValue::Int(v)=>Some(v.to_string()),DbValue::Float(v)=>Some(v.to_string()),DbValue::Bool(v)=>Some(v.to_string())}).collect()).collect(),affected:s.affected,elapsed_ms:0,ok:true,message:String::new(),has_rows:!s.columns.is_empty(),primary_keys:vec![],total:s.rows.len() as u64}).collect(),messages:details.warnings.clone(),elapsed_ms:0,spool_id:None}}
fn output_values(metadata:&RoutineCallMetadata,ordinals:&[i32],values:&[DbValue],pg:bool)->Result<Vec<RoutineOutput>> {
    if ordinals.len()!=values.len(){return Err(crate::error::AppError::coded("EXECUTION_EFFECTS_UNKNOWN",format!("OUT/INOUT返回{}值，参数元数据要求{}值；调用可能已写入，禁止重放",values.len(),ordinals.len())).into());}
    ordinals.iter().zip(values).map(|(ordinal,value)|{
        let parameter=metadata.parameters.iter().find(|p|p.ordinal==*ordinal&&matches!(p.mode.as_str(),"out"|"inout")).ok_or_else(||crate::error::AppError::coded("EXECUTION_EFFECTS_UNKNOWN","OUT身份与参数元数据不符；调用可能已写入"))?;
        validate_value(parameter,value,pg).map_err(|error|crate::error::AppError::coded("EXECUTION_EFFECTS_UNKNOWN",format!("第{ordinal}个OUT/INOUT类型无法核验：{error}；没有把结果当作NULL")))?;
        Ok(RoutineOutput{ordinal:*ordinal,name:parameter.name.clone().unwrap_or_else(||format!("参数{ordinal}")),value:value.clone(),data_type:parameter.data_type.clone()})
    }).collect()
}
fn pg_outputs(metadata:&RoutineCallMetadata,sets:&[TypedResultSet])->Result<Vec<RoutineOutput>> {
    let ordinals=metadata.parameters.iter().filter(|p|matches!(p.mode.as_str(),"out"|"inout")).map(|p|p.ordinal).collect::<Vec<_>>();
    if ordinals.is_empty(){return Ok(vec![]);}
    if sets.len()!=1||sets[0].rows.len()!=1||sets[0].columns.len()!=ordinals.len(){return Err(crate::error::AppError::coded("EXECUTION_EFFECTS_UNKNOWN","PostgreSQL OUT/INOUT结果行/列形状与参数元数据不符；没有遗漏参数或把它们当NULL，禁止重放").into());}
    output_values(metadata,&ordinals,&sets[0].rows[0],true)
}
pub(crate) async fn pg_execute(conn:&mut sqlx::PgConnection,work:&CallExecution,entry:&std::sync::Arc<crate::execution::Entry>)->Result<(MultiResult,RoutineCallDetails)>{
    validate_execution_source(work,&entry.binding)?;
    let current=pg_metadata(conn,&work.metadata.object,false).await?;let args=checked_arguments(&current,&work.draft,true)?;
    if entry.stopping(){return Err(crate::error::AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND","参数读取期间停止，没有发送例程调用").into());}
    let (sql,values,_)=call_sql(&current,&args,&DbKind::of("postgresql"),"unused")?;let mut query=crate::sql_history::query(&sql);for v in &values{query=bind_pg(query,v)?;}
    let mut sets=Vec::new();let mut rows=0;let mut bytes=0;let mut stream=query.fetch(conn);
    while let Some(row)=stream.try_next().await?{let columns=row.columns().iter().map(|c|TypedResultColumn{name:c.name().into(),data_type:c.type_info().name().into()}).collect();push_row(&mut sets,columns,pg_values(&row)?,&mut rows,&mut bytes)?;}
    let mut details=RoutineCallDetails{sets,outputs:vec![],warnings:vec!["调用可能已经写入；取消/超时不自动回滚".into()]};
    details.outputs=pg_outputs(&current,&details.sets)?;
    if details.sets.is_empty(){details.warnings.push("PostgreSQL空结果的列元数据当前未读取；不据此宣称没有返回定义".into());}
    Ok((multi(&details),details))
}
#[allow(deprecated)]
pub(crate) async fn mysql_execute(conn:&mut sqlx::MySqlConnection,kind:&DbKind,work:&CallExecution,entry:&std::sync::Arc<crate::execution::Entry>)->Result<(MultiResult,RoutineCallDetails)>{
    validate_execution_source(work,&entry.binding)?;
    let current=mysql_metadata(conn,kind,&work.metadata.object,false).await?;let args=checked_arguments(&current,&work.draft,false)?;let suffix=uuid::Uuid::new_v4().simple().to_string();let (sql,values,outputs)=call_sql(&current,&args,kind,&suffix)?;
    if entry.stopping(){return Err(crate::error::AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND","参数读取期间停止，没有发送例程调用").into());}
    for (p,v) in &args {if p.mode=="out"||p.mode=="inout"{let var=outputs.iter().find(|(n,_)|n==&p.ordinal).unwrap().1.clone();let setup=format!("SET {var} = ?");let query=bind_mysql(crate::sql_history::query(&setup),v.as_ref().unwrap_or(&DbValue::Null))?;query.execute(&mut *conn).await?;}}
    if entry.stopping(){return Err(crate::error::AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND","临时OUT变量准备期间停止，没有发送例程调用").into());}
    let mut query=crate::sql_history::query(&sql);for v in &values{query=bind_mysql(query,v)?;}
    let mut sets=Vec::new();let mut rows=0;let mut bytes=0;// Box<dyn Stream + Send> 在单一具体生命周期上强制 Send 检查：
// 裸 MySqlStream 的条件 Send 实现在组合 future 里会触发 rustc
// 「Send 不够通用」推断缺陷（#102211），dyn 擦除后即可通过。
    let mut stream: std::pin::Pin<Box<dyn futures::Stream<Item = std::result::Result<sqlx::Either<sqlx::mysql::MySqlQueryResult, sqlx::mysql::MySqlRow>, sqlx::Error>> + Send + '_>> = Box::pin(query.fetch_many(&mut *conn));let mut boundary=true;
    while let Some(item)=stream.try_next().await? {if sets.len()>1024{return Err(crate::error::AppError::coded("EXECUTION_EFFECTS_UNKNOWN","多结果集超过1024组，固定连接将销毁；禁止重放").into());}match item {sqlx::Either::Right(row)=>{let names=row.columns().iter().map(|c|TypedResultColumn{name:c.name().into(),data_type:c.type_info().name().into()}).collect();if boundary{sets.push(TypedResultSet{columns:row.columns().iter().map(|c|TypedResultColumn{name:c.name().into(),data_type:c.type_info().name().into()}).collect(),rows:vec![],affected:0});boundary=false;}push_row(&mut sets,names,mysql_values(&row)?,&mut rows,&mut bytes)?;},sqlx::Either::Left(result)=>{if boundary{sets.push(TypedResultSet{columns:vec![],rows:vec![],affected:result.rows_affected()});}else{sets.last_mut().unwrap().affected=result.rows_affected();}boundary=true;}}}drop(stream);
    let mut details=RoutineCallDetails{sets,outputs:vec![],warnings:vec!["所有结果已在同一session读取；空结果集的列metadata当前SQLx API不保证可见".into()]};
    if !outputs.is_empty(){let select=format!("SELECT {}",outputs.iter().map(|(ordinal,var)|format!("{var} AS navkit_out_{ordinal}")).collect::<Vec<_>>().join(","));let row=crate::sql_history::query(&select).fetch_one(conn).await?;let vals=mysql_values(&row)?;let ordinals=outputs.iter().map(|(ordinal,_)|*ordinal).collect::<Vec<_>>();details.outputs=output_values(&current,&ordinals,&vals,false)?;}
    Ok((multi(&details),details))
}

#[cfg(test)] mod tests {
    use super::*;
    fn meta()->RoutineCallMetadata{let object=DatabaseObjectId{connection_id:"c".into(),catalog:None,schema:"public".into(),kind:"function".into(),name:"f".into(),identity_arguments:Some("x bigint, y text".into()),parent:None};let parameters=vec![RoutineParameter{ordinal:1,name:Some("x".into()),mode:"in".into(),data_type:"bigint".into(),type_schema:Some("pg_catalog".into()),type_name:"int8".into(),has_default:Some(false),variadic:false},RoutineParameter{ordinal:2,name:Some("y".into()),mode:"in".into(),data_type:"text".into(),type_schema:Some("pg_catalog".into()),type_name:"text".into(),has_default:Some(true),variadic:false}];finish_metadata(RoutineCallMetadata{object,source_fingerprint:Some("a".repeat(64)),backend_object_id:Some("42".into()),revision:String::new(),parameters,result_type:Some("text".into()),definition_revision:"def".into(),completeness:"complete".into(),call:Capability{level:String::new(),reason:String::new()},notes:vec![]},false,true)}
    fn draft(m:&RoutineCallMetadata)->RoutineCallDraft{RoutineCallDraft{source_fingerprint:m.source_fingerprint.clone(),object:m.object.clone(),metadata_revision:m.revision.clone(),arguments:vec![RoutineArgument{ordinal:1,argument:RoutineArgumentValue::Value(DbValue::Integer("9007199254740993".into()))},RoutineArgument{ordinal:2,argument:RoutineArgumentValue::Value(DbValue::Text("x'); DROP TABLE t; --".into()))}]}}
    #[test] fn sql_never_contains_input_values(){let m=meta();let d=draft(&m);let a=checked_arguments(&m,&d,true).unwrap();let (sql,v,_)=call_sql(&m,&a,&DbKind::of("postgresql"),"safe").unwrap();assert!(sql.contains("$1::pg_catalog.int8"));assert!(!sql.contains("9007199254740993"));assert!(!sql.contains("DROP TABLE"));assert_eq!(v.len(),2);}
    #[test] fn null_empty_and_default_remain_distinct(){let m=meta();for value in [RoutineArgumentValue::Value(DbValue::Null),RoutineArgumentValue::Value(DbValue::Text("".into())),RoutineArgumentValue::Default]{let mut d=draft(&m);d.arguments[1].argument=value.clone();let args=checked_arguments(&m,&d,true).unwrap();let (_,v,_)=call_sql(&m,&args,&DbKind::of("postgresql"),"safe").unwrap();assert_eq!(v.len(),if matches!(value,RoutineArgumentValue::Default){1}else{2});}}
    #[test] fn stale_signature_duplicate_unknown_and_wrong_type_rejected(){let m=meta();let mut d=draft(&m);d.metadata_revision="bad".into();assert!(checked_arguments(&m,&d,true).is_err());let mut d=draft(&m);d.arguments.push(d.arguments[0].clone());assert!(checked_arguments(&m,&d,true).is_err());let mut d=draft(&m);d.arguments[0].argument=RoutineArgumentValue::Value(DbValue::Text("42".into()));assert!(checked_arguments(&m,&d,true).is_err());}
    #[test] fn registry_binds_window_endpoint_namespace_and_consumes_once(){let m=meta();let b=Binding{principal:"window".into(),owner:"page".into(),connection:"c".into(),namespace:"public".into(),fingerprint:"a".repeat(64)};let r=Registry::default();let p=r.prepare(b.clone(),m.clone(),draft(&m),&DbKind::of("postgresql" )).unwrap();assert!(r.validate_plan(&p.plan_id,&b).is_ok());assert!(r.validate_plan(&p.plan_id,&b).is_ok());let mut wrong=b.clone();wrong.principal="other".into();assert!(r.take(&p.plan_id,&wrong).is_err());let mut wrong=b.clone();wrong.fingerprint="b".repeat(64);assert!(r.validate_plan(&p.plan_id,&wrong).is_err());assert!(r.take(&p.plan_id,&wrong).is_err());assert!(r.take(&p.plan_id,&b).is_ok());assert!(r.validate_plan(&p.plan_id,&b).is_err());assert!(r.take(&p.plan_id,&b).is_err());}
    #[test] fn readonly_and_custom_types_refuse_calls(){let mut m=meta();m.parameters[0].type_schema=Some("user_types".into());assert_eq!(finish_metadata(m,false,true).call.level,"unsupported");assert_eq!(finish_metadata(meta(),true,true).call.level,"unsupported");}
    #[test] fn numeric_binary_and_zone_validation_is_lossless(){let mut p=meta().parameters.remove(0);p.type_name="numeric".into();assert!(validate_value(&p,&DbValue::Decimal("12345678901234567890.12345678901234567890".into()),true).is_ok());assert!(validate_value(&p,&DbValue::Decimal("NaN".into()),true).is_err());p.type_name="bytea".into();assert!(validate_value(&p,&DbValue::Binary("AP8=".into()),true).is_ok());p.type_name="timestamptz".into();assert!(validate_value(&p,&DbValue::Timestamptz("2026-11-09T10:00:00+08:00".into()),true).is_ok());assert!(validate_value(&p,&DbValue::Timestamptz("2026-11-09T10:00:00".into()),true).is_err());}
    #[test] fn output_shape_cannot_silently_drop_or_fabricate_parameters(){let mut m=meta();m.object.kind="procedure".into();m.parameters[0].mode="inout".into();m.parameters[1].mode="out".into();let values=vec![DbValue::Integer("9007199254740993".into()),DbValue::Text(String::new())];let columns=vec![TypedResultColumn{name:"x".into(),data_type:"INT8".into()},TypedResultColumn{name:"y".into(),data_type:"TEXT".into()}];let set=TypedResultSet{columns,rows:vec![values.clone()],affected:0};let out=pg_outputs(&m,&[set.clone()]).unwrap();assert_eq!(out.len(),2);assert_eq!(out[0].ordinal,1);assert_eq!(out[0].value,values[0]);assert_eq!(out[1].value,values[1]);for sets in [vec![],vec![TypedResultSet{rows:vec![],..set.clone()}],vec![TypedResultSet{rows:vec![vec![values[0].clone()]],..set.clone()}],vec![set.clone(),set.clone()]]{let error=pg_outputs(&m,&sets).unwrap_err();assert_eq!(error.downcast_ref::<crate::error::AppError>().unwrap().code,"EXECUTION_EFFECTS_UNKNOWN");}}
    #[test] fn output_type_and_nonfinite_are_unknown_not_null(){let mut m=meta();m.parameters[0].mode="out".into();m.parameters[0].type_name="float8".into();for values in [vec![DbValue::Float(f64::NAN)],vec![DbValue::Text("not a float".into())],vec![],vec![DbValue::Float(1.0),DbValue::Null]]{assert_eq!(output_values(&m,&[1],&values,true).unwrap_err().downcast_ref::<crate::error::AppError>().unwrap().code,"EXECUTION_EFFECTS_UNKNOWN");}assert_eq!(output_values(&m,&[1],&[DbValue::Null],true).unwrap()[0].value,DbValue::Null);}
    #[test] fn function_inout_is_also_returned_as_typed_output(){let mut m=meta();m.parameters[0].mode="inout".into();let set=TypedResultSet{columns:vec![TypedResultColumn{name:"f".into(),data_type:"INT8".into()}],rows:vec![vec![DbValue::Integer("9223372036854775807".into())]],affected:0};let outputs=pg_outputs(&m,&[set]).unwrap();assert_eq!(outputs.len(),1);assert_eq!(outputs[0].name,"x");assert_eq!(outputs[0].value,DbValue::Integer("9223372036854775807".into()));}
    #[test] fn identical_definition_on_new_endpoint_cannot_rebase_loaded_arguments(){let old=meta();let mut current=old.clone();current.source_fingerprint=Some("b".repeat(64));current=finish_metadata(current,false,true);assert_eq!(old.revision,current.revision);let binding=Binding{principal:"window".into(),owner:"page".into(),connection:"c".into(),namespace:"public".into(),fingerprint:"b".repeat(64)};let registry=Registry::default();let error=registry.prepare(binding,current,draft(&old),&DbKind::of("postgresql")).unwrap_err();assert_eq!(error.downcast_ref::<crate::error::AppError>().unwrap().code,"ROUTINE_SOURCE_CHANGED");assert!(registry.plans.lock().is_empty());}
    #[test] fn absent_source_on_metadata_or_legacy_draft_never_creates_plan(){let m=meta();let binding=Binding{principal:"window".into(),owner:"page".into(),connection:"c".into(),namespace:"public".into(),fingerprint:"a".repeat(64)};let registry=Registry::default();let mut legacy=serde_json::to_value(draft(&m)).unwrap();legacy.as_object_mut().unwrap().remove("source_fingerprint");let old:RoutineCallDraft=serde_json::from_value(legacy).unwrap();assert!(old.source_fingerprint.is_none());for (meta,draft) in [(m.clone(),old),({let mut value=m.clone();value.source_fingerprint=None;value},draft(&m))]{let error=registry.prepare(binding.clone(),meta,draft,&DbKind::of("postgresql")).unwrap_err();assert_eq!(error.downcast_ref::<crate::error::AppError>().unwrap().code,"ROUTINE_SOURCE_REQUIRED");}assert!(registry.plans.lock().is_empty());}
    #[test] fn native_reread_without_ipc_source_preserves_parameter_revision(){let loaded=meta();let mut native=loaded.clone();native.source_fingerprint=None;let native=finish_metadata(native,false,true);assert_eq!(loaded.revision,native.revision);assert!(checked_arguments(&native,&draft(&loaded),true).is_ok());let mut other=loaded.clone();other.source_fingerprint=Some("b".repeat(64));assert_eq!(loaded.revision,finish_metadata(other,false,true).revision);}
    #[test] fn unknown_signature_not_assumed_zero_parameters(){let mut o=meta().object;o.identity_arguments=None;assert!(routine_id(&o).is_err());o.identity_arguments=Some("".into());assert!(routine_id(&o).is_ok());}
}
