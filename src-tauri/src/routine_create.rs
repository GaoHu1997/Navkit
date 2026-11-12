//! 新例程创建是独立的不存在基线，不冒充既有 RoutineSnapshot。
//! 原生写入仅由 execution_bound 的 C1 owned connection 调用；从不 DROP/OR REPLACE。
use std::{collections::{BTreeMap,HashMap},sync::{Arc,OnceLock},time::{Duration,Instant}};
use futures::FutureExt as _;
use anyhow::{anyhow,bail,Result};
use parking_lot::Mutex;
use serde::{Deserialize,Serialize};
use sqlx::{Column,Row};
use crate::{db::{self,Db},error::{AppError,CmdResult},execution::{Binding,Entry},models::{DbKind,MultiResult,Protocol},routine_design::{self,RoutineObjectId,RoutineSnapshot,RoutineStep,RoutineStepResult}};

const TTL:Duration=Duration::from_secs(120);
const MAX_BASELINES:usize=64;
const MAX_SQL:usize=2*1024*1024;
#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct RoutineCreationBaseline {
    pub baseline_id:String,pub object:RoutineObjectId,
    #[serde(default)] pub source_fingerprint:Option<String>,
    pub base_revision:String,pub completeness:String,pub absent:bool,
    pub expires_in_ms:u64,pub strategy:String,pub warnings:Vec<String>,
}
#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct RoutineCreationDraft {
    pub baseline_id:String,pub object:RoutineObjectId,
    #[serde(default)] pub source_fingerprint:Option<String>,
    #[serde(default)] pub base_revision:String,
    pub definition:String,
}
#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct RoutineCreationPreview {
    pub baseline_id:String,pub object:RoutineObjectId,pub source_fingerprint:Option<String>,pub base_revision:String,
    pub steps:Vec<RoutineStep>,pub request_sql:String,pub strategy:String,pub executable:bool,
    pub expires_in_ms:u64,pub warnings:Vec<String>,
}
#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct RoutineCreationReceipt {
    pub object:RoutineObjectId,pub source_fingerprint:Option<String>,pub baseline_id:String,
    pub status:String,pub verified:bool,pub steps:Vec<RoutineStepResult>,pub transaction:String,
    pub snapshot:RoutineSnapshot,pub warnings:Vec<String>,
}
#[derive(Debug,Clone)]struct Proof {context:BTreeMap<String,String>,revision:String}
#[derive(Clone)]struct Stored {binding:Binding,baseline:RoutineCreationBaseline,proof:Proof,kind:DbKind,expires:Instant}
#[derive(Default)]pub struct Registry {baselines:Mutex<HashMap<String,Stored>>}
#[derive(Clone)]pub(crate) struct CreationExecution {stored:Stored,draft:RoutineCreationDraft,compiled:Compiled}
#[derive(Clone,Debug)]struct Compiled {sql:String,body:String,language:String,result_type:Option<String>,deterministic:String,data_access:String,security:String}
pub fn global()->&'static Registry {static VALUE:OnceLock<Registry>=OnceLock::new();VALUE.get_or_init(Registry::default)}
fn source_object(object:&RoutineObjectId,binding:&Binding)->CmdResult<()> {
    routine_design::require_source_fingerprint(Some(&binding.fingerprint))?;
    if binding.principal.is_empty()||binding.owner.is_empty()||object.connection_id!=binding.connection||object.schema!=binding.namespace {
        return Err(AppError::coded("ROUTINE_CREATION_SCOPE_MISMATCH","创建对象必须与真实窗口/页面/连接/命名空间一致，没有发送SQL"));
    }
    if !matches!(object.kind.as_str(),"function"|"procedure")||object.name.is_empty()||object.name.len()>256||object.schema.is_empty()||object.name.contains('\0')||object.schema.contains('\0') {
        return Err(AppError::coded("ROUTINE_CREATION_ID_INVALID","创建必须提供完整函数/过程身份"));
    }
    if !object.identity_arguments.is_empty(){return Err(AppError::coded("ROUTINE_CREATION_SIGNATURE_UNSUPPORTED","当前安全创建只支持零参数新名字；带参创建需要单独认证"));}
    if object.schema.starts_with("pg_")||matches!(object.schema.as_str(),"information_schema"|"mysql"|"sys"|"performance_schema") {
        return Err(AppError::coded("ROUTINE_SYSTEM_OBJECT","禁止在系统命名空间创建例程"));
    }
    Ok(())
}
fn same_binding(a:&Binding,b:&Binding)->bool {a.principal==b.principal&&a.owner==b.owner&&a.connection==b.connection&&a.namespace==b.namespace&&a.fingerprint==b.fingerprint}
fn valid_id(id:&str)->CmdResult<()> {let parsed=uuid::Uuid::parse_str(id).map_err(|_|AppError::coded("ROUTINE_CREATION_BASELINE_REQUIRED","缺少规范创建基线"))?;if parsed.is_nil()||parsed.to_string()!=id{return Err(AppError::coded("ROUTINE_CREATION_BASELINE_REQUIRED","创建基线必须为规范非空UUID"));}Ok(())}
impl Registry {
    fn insert(&self,object:&RoutineObjectId,binding:&Binding,kind:&DbKind,proof:Proof)->Result<RoutineCreationBaseline> {
        let mut entries=self.baselines.lock();entries.retain(|_,v|v.expires>Instant::now());
        if entries.len()>=MAX_BASELINES{bail!("ROUTINE_CREATION_CAPACITY: 创建基线数量已达上限，请关闭旧创建页或等待过期");}
        let baseline=RoutineCreationBaseline{baseline_id:uuid::Uuid::new_v4().to_string(),object:object.clone(),source_fingerprint:Some(binding.fingerprint.clone()),base_revision:proof.revision.clone(),completeness:"complete".into(),absent:true,expires_in_ms:TTL.as_millis() as u64,strategy:if kind.protocol()==Protocol::Postgres{"create_transactional"}else{"create_implicit_commit"}.into(),warnings:vec!["仅零参数且该命名空间整个同名例程组不存在；过期后必须显式重新取创建基线，不自动重放".into(),"PostgreSQL/MySQL专用实库尚未认证；此原生能力是受限创建，不是DROP重建或完整对象管理".into()]};
        entries.insert(baseline.baseline_id.clone(),Stored{binding:binding.clone(),baseline:baseline.clone(),proof,kind:kind.clone(),expires:Instant::now()+TTL});Ok(baseline)
    }
    fn checked(&self,draft:&RoutineCreationDraft,binding:&Binding)->CmdResult<Stored> {
        routine_design::validate_source_fingerprint(draft.source_fingerprint.as_deref(),&binding.fingerprint)?;source_object(&draft.object,binding)?;valid_id(&draft.baseline_id)?;
        if draft.base_revision.is_empty(){return Err(AppError::coded("ROUTINE_CREATION_BASELINE_REQUIRED","不能把创建草稿作为已存在例程；必须保留打开时不存在基线"));}
        let entries=self.baselines.lock();let stored=entries.get(&draft.baseline_id).ok_or_else(||AppError::coded("ROUTINE_CREATION_BASELINE_EXPIRED","创建基线已过期或被消费，必须显式重新核对不存在状态"))?;
        if stored.expires<=Instant::now(){return Err(AppError::coded("ROUTINE_CREATION_BASELINE_EXPIRED","创建基线已过期，未执行SQL"));}
        if !same_binding(&stored.binding,binding)||stored.baseline.object!=draft.object||stored.baseline.base_revision!=draft.base_revision||stored.baseline.source_fingerprint!=draft.source_fingerprint {
            return Err(AppError::coded("ROUTINE_CREATION_BASELINE_MISMATCH","草稿对象/来源/不存在基线与原始创建页面不同，未执行SQL"));
        }
        if draft.definition.len()>MAX_SQL||draft.definition.contains('\0'){return Err(AppError::coded("ROUTINE_CREATION_SQL_INVALID","例程定义超过2MiB或包含NUL"));}
        Ok(stored.clone())
    }
    pub fn validate_draft(&self,draft:&RoutineCreationDraft,binding:&Binding)->CmdResult<()> {let stored=self.checked(draft,binding)?;compile(&draft.object,&stored.kind,&draft.definition).map_err(AppError::from)?;Ok(())}
    pub(crate) fn take(&self,draft:&RoutineCreationDraft,binding:&Binding,kind:&DbKind)->Result<CreationExecution> {
        let stored=self.checked(draft,binding)?;
        if &stored.kind!=kind{bail!("ROUTINE_CREATION_KIND_CHANGED: 创建基线驱动已变化");}
        let compiled=compile(&draft.object,kind,&draft.definition)?;
        let mut entries=self.baselines.lock();let current=entries.get(&draft.baseline_id).ok_or_else(||anyhow!("ROUTINE_CREATION_BASELINE_EXPIRED: 创建基线已被消费"))?;
        if current.expires<=Instant::now()||!same_binding(&current.binding,binding){bail!("ROUTINE_CREATION_BASELINE_EXPIRED: 创建基线已过期或来源不匹配");}
        entries.remove(&draft.baseline_id);Ok(CreationExecution{stored,draft:draft.clone(),compiled})
    }
}
/// 给父 ControlledWork.validate 的零SQL守门，实际一次性消费由 bound 执行器完成。
pub fn validate_draft(draft:&RoutineCreationDraft,binding:&Binding)->CmdResult<()> {global().validate_draft(draft,binding)}
pub fn request_sql(draft:&RoutineCreationDraft)->CmdResult<String> {
    routine_design::require_source_fingerprint(draft.source_fingerprint.as_deref())?;valid_id(&draft.baseline_id)?;
    if draft.base_revision.is_empty()||draft.definition.is_empty()||draft.definition.len()>MAX_SQL||draft.definition.contains('\0'){return Err(AppError::coded("ROUTINE_CREATION_BASELINE_REQUIRED","创建工作必须包含固定不存在基线与完整定义"));}
    Ok(format!("ROUTINE_CREATION {}",crate::object_catalog::revision(draft)))
}

#[derive(Clone,Debug,PartialEq,Eq)]enum TokenKind {Word,Identifier,Literal,Dollar,Symbol}
#[derive(Clone,Debug)]struct Token {text:String,start:usize,end:usize,kind:TokenKind}
impl Token {fn is(&self,word:&str)->bool {self.kind==TokenKind::Word&&self.text.eq_ignore_ascii_case(word)}}
fn lex(sql:&str,pg:bool)->Result<Vec<Token>> {
    if sql.len()>MAX_SQL||sql.contains('\0'){bail!("ROUTINE_CREATION_SQL_INVALID: 定义过大或包含NUL");}
    let bytes=sql.as_bytes();let mut at=0;let mut tokens=Vec::new();
    while at<bytes.len(){
        if bytes[at].is_ascii_whitespace(){at+=1;continue;}
        if bytes[at..].starts_with(b"--") {
            if !pg&&!bytes.get(at+2).map_or(true, |b|b.is_ascii_whitespace()){bail!("ROUTINE_CREATION_SQL_UNSUPPORTED: MySQL非空格双横线语义未认证");}
            while at<bytes.len()&&bytes[at]!=b'\n'{at+=1;}continue;
        }
        if bytes[at..].starts_with(b"/*") {
            if bytes.get(at+2).is_some_and(|b|matches!(b,b'!'|b'+'))||bytes.get(at+2..at+4)==Some(b"M!"){bail!("ROUTINE_CREATION_SQL_UNSUPPORTED: 可执行版本/提示注释不允许");}
            at+=2;let mut depth=1;while at<bytes.len()&&depth>0{if bytes[at..].starts_with(b"/*"){if !pg{bail!("MySQL嵌套注释未认证");}depth+=1;at+=2;}else if bytes[at..].starts_with(b"*/"){depth-=1;at+=2;}else{at+=1;}}
            if depth!=0{bail!("注释未闭合");}continue;
        }
        let start=at;let kind;
        if matches!(bytes[at],b'\''|b'"'|b'`') {
            let quote=bytes[at];kind=if quote==b'\''{TokenKind::Literal}else{TokenKind::Identifier};at+=1;let mut closed=false;
            while at<bytes.len(){if bytes[at]==quote{at+=1;if bytes.get(at)==Some(&quote){at+=1;continue;}closed=true;break;}if bytes[at]==b'\\'{bail!("ROUTINE_CREATION_SQL_UNSUPPORTED: 引用内反斜线依赖会话SQL模式，安全创建请用双写引号/dollar quote");}at+=1;}
            if !closed{bail!("引用未闭合");}
        }else if pg&&bytes[at]==b'$'{let mut end=at+1;while end<bytes.len()&&(bytes[end].is_ascii_alphanumeric()||bytes[end]==b'_'){end+=1;}if bytes.get(end)!=Some(&b'$'){bail!("未知dollar quote");}let tag=&sql[at..=end];at=end+1;let rest=sql[at..].find(tag).ok_or_else(||anyhow!("dollar quote未闭合"))?;at+=rest+tag.len();kind=TokenKind::Dollar;
        }else if bytes[at].is_ascii_alphanumeric()||bytes[at]==b'_'||bytes[at]>=128 {at+=1;while at<bytes.len()&&(bytes[at].is_ascii_alphanumeric()||bytes[at]==b'_'||bytes[at]==b'$'||bytes[at]>=128){at+=1;}kind=TokenKind::Word;
        }else{at+=1;kind=TokenKind::Symbol;}
        tokens.push(Token{text:sql[start..at].into(),start,end:at,kind});
    }Ok(tokens)
}
fn identifier(token:&Token,pg:bool)->Result<String> {
    match token.kind {TokenKind::Word=>Ok(if pg{token.text.to_ascii_lowercase()}else{token.text.clone()}),TokenKind::Identifier=>{let quote=token.text.as_bytes()[0];if pg&&quote!=b'"'||!pg&&quote!=b'`'{bail!("标识符引用模式未认证");}let q=quote as char;Ok(token.text[1..token.text.len()-1].replace(&format!("{q}{q}"),&q.to_string()))},_=>bail!("标识符非法")}
}
fn normalized(tokens:&[Token])->String {tokens.iter().map(|t|{let value=if t.kind==TokenKind::Word{t.text.to_ascii_lowercase()}else{t.text.clone()};format!("{}:{value}",value.len())}).collect::<Vec<_>>().join("|")}
fn pg_return(tokens:&[Token])->Result<String> {
    let value=tokens.iter().map(|t|t.text.to_ascii_lowercase()).collect::<Vec<_>>().join(" ");
    let value=value.strip_prefix("pg_catalog . ").unwrap_or(&value);
    Ok(match value {"int"|"int4"|"integer"=>"integer","int2"|"smallint"=>"smallint","int8"|"bigint"=>"bigint","bool"|"boolean"=>"boolean","float4"|"real"=>"real","float8"|"double precision"=>"double precision","decimal"|"numeric"=>"numeric","timestamp"|"timestamp without time zone"=>"timestamp without time zone","timestamptz"|"timestamp with time zone"=>"timestamp with time zone","time"|"time without time zone"=>"time without time zone","timetz"|"time with time zone"=>"time with time zone","text"=>"text","bytea"=>"bytea","uuid"=>"uuid","json"=>"json","jsonb"=>"jsonb","date"=>"date","void"=>"void",_=>bail!("ROUTINE_CREATION_RETURN_UNSUPPORTED: PostgreSQL创建只支持已知无typmod内置标量返回类型")}.into())
}
fn mysql_return(tokens:&[Token])->Result<String> {
    if tokens.is_empty(){bail!("缺少函数返回类型");}
    let first=tokens[0].text.to_ascii_lowercase();let integer=matches!(first.as_str(),"tinyint"|"smallint"|"mediumint"|"int"|"integer"|"bigint");
    let mut suffix=String::new();let mut i=1;
    if tokens.get(i).is_some_and(|t|t.text=="(") {let start=i;i+=1;while i<tokens.len()&&tokens[i].text!=")"{if tokens[i].kind!=TokenKind::Word&&tokens[i].text!=","{bail!("返回类型修饰非法");}if tokens[i].kind==TokenKind::Word&&!tokens[i].text.bytes().all(|b|b.is_ascii_digit()){bail!("返回类型长度只能数字");}i+=1;}if i==tokens.len(){bail!("返回类型括号未闭合");}i+=1;if !integer{suffix=tokens[start..i].iter().map(|t|t.text.as_str()).collect::<String>();}}
    let unsigned=if tokens.get(i).is_some_and(|t|t.is("UNSIGNED")){i+=1;" unsigned"}else{""};
    if i!=tokens.len()||!matches!(first.as_str(),"tinyint"|"smallint"|"mediumint"|"int"|"integer"|"bigint"|"decimal"|"numeric"|"float"|"double"|"char"|"varchar"|"text"|"tinytext"|"mediumtext"|"longtext"|"binary"|"varbinary"|"blob"|"tinyblob"|"mediumblob"|"longblob"|"date"|"time"|"datetime"|"timestamp"){bail!("ROUTINE_CREATION_RETURN_UNSUPPORTED: MySQL仅已知内置标量返回类型，字符集/排序规则等修饰须单独认证");}
    Ok(format!("{}{}{}",match first.as_str(){"integer"=>"int","numeric"=>"decimal",other=>other},suffix,unsigned))
}
fn parse(object:&RoutineObjectId,kind:&DbKind,sql:&str,readback:bool)->Result<Compiled> {
    let pg=kind.protocol()==Protocol::Postgres;if !pg&&kind.protocol()!=Protocol::Mysql{bail!("ROUTINE_CREATION_ADAPTER_UNSUPPORTED");}
    let mut tokens=lex(sql,pg)?;if tokens.last().is_some_and(|t|t.text==";"){tokens.pop();}
    let mut i=0;if !tokens.get(i).is_some_and(|t|t.is("CREATE")){bail!("创建必须是单独CREATE FUNCTION/PROCEDURE");}i+=1;
    if readback&&pg&&tokens.get(i).is_some_and(|t|t.is("OR")){if !tokens.get(i+1).is_some_and(|t|t.is("REPLACE")){bail!("读回CREATE修饰未知");}i+=2;}
    if readback&&!pg&&tokens.get(i).is_some_and(|t|t.is("DEFINER")){i+=1;if tokens.get(i).map_or(true, |t|t.text!="="){bail!("DEFINER读回语法未知");}i+=1;let start=i;while i<tokens.len()&&!tokens[i].is("FUNCTION")&&!tokens[i].is("PROCEDURE"){if i-start>6||tokens[i].text==";"{bail!("DEFINER读回语法未知");}i+=1;}}
    let word=if object.kind=="function"{"FUNCTION"}else if object.kind=="procedure"{"PROCEDURE"}else{bail!("创建类型未知");};
    if !tokens.get(i).is_some_and(|t|t.is(word)){bail!("ROUTINE_CREATION_TARGET_MISMATCH: 创建类型/REPLACE/DEFINER不允许");}i+=1;
    let first=tokens.get(i).ok_or_else(||anyhow!("缺少对象名称"))?;let name_start=first.start;let mut name=identifier(first,pg)?;i+=1;let mut schema=None;
    if tokens.get(i).is_some_and(|t|t.text=="."){schema=Some(name);i+=1;name=identifier(tokens.get(i).ok_or_else(||anyhow!("缺少限定对象名"))?,pg)?;i+=1;}
    let name_end=tokens[i-1].end;
    if name!=object.name||schema.as_ref().is_some_and(|s|s!=&object.schema){bail!("ROUTINE_CREATION_TARGET_MISMATCH: CREATE精确对象名/命名空间与固定基线不一致");}
    if !tokens.get(i).is_some_and(|t|t.text=="(")||!tokens.get(i+1).is_some_and(|t|t.text==")"){bail!("ROUTINE_CREATION_SIGNATURE_UNSUPPORTED: 只支持零参数创建，不能改签名");}i+=2;
    let mut result_type=None;
    if object.kind=="function" {
        if !tokens.get(i).is_some_and(|t|t.is("RETURNS")){bail!("函数缺少RETURNS");}i+=1;let start=i;
        while i<tokens.len()&&!tokens[i].is("AS")&&!tokens[i].is("LANGUAGE")&&!tokens[i].is("SQL")&&!tokens[i].is("DETERMINISTIC")&&!tokens[i].is("NOT")&&!tokens[i].is("CONTAINS")&&!tokens[i].is("NO")&&!tokens[i].is("READS")&&!tokens[i].is("MODIFIES")&&!tokens[i].is("BEGIN")&&!tokens[i].is("RETURN"){i+=1;}
        result_type=Some(if pg{pg_return(&tokens[start..i])?}else{mysql_return(&tokens[start..i])?});
    }
    let mut language=None;let mut body=None;let mut deterministic="NO".to_string();let mut data_access="CONTAINS SQL".to_string();let mut security=if pg{"INVOKER"}else{"DEFINER"}.to_string();let mut seen=std::collections::HashSet::new();
    while i<tokens.len(){
        if tokens[i].is("LANGUAGE"){if !seen.insert("language"){bail!("重复LANGUAGE");}i+=1;let value=tokens.get(i).ok_or_else(||anyhow!("LANGUAGE缺失"))?;if value.kind!=TokenKind::Word{bail!("LANGUAGE必须是已知原生语言");}let value=value.text.to_ascii_lowercase();if pg&&!matches!(value.as_str(),"sql"|"plpgsql")||!pg&&value!="sql"{bail!("ROUTINE_CREATION_LANGUAGE_UNSUPPORTED");}language=Some(value);i+=1;
        }else if pg&&tokens[i].is("AS"){if body.is_some(){bail!("重复AS body");}i+=1;let token=tokens.get(i).ok_or_else(||anyhow!("AS缺少body"))?;if token.kind!=TokenKind::Dollar{bail!("ROUTINE_CREATION_BODY_UNSUPPORTED: PostgreSQL新建仅独立dollar body");}let tag=token.text[1..].find('$').unwrap()+2;body=Some(token.text[tag..token.text.len()-tag].to_string());i+=1;
        }else if !pg&&tokens[i].is("DETERMINISTIC"){if !seen.insert("deterministic"){bail!("重复DETERMINISTIC");}deterministic="YES".into();i+=1;
        }else if !pg&&tokens[i].is("NOT")&&tokens.get(i+1).is_some_and(|t|t.is("DETERMINISTIC")){if !seen.insert("deterministic"){bail!("重复DETERMINISTIC");}deterministic="NO".into();i+=2;
        }else if !pg&&tokens[i].is("SQL")&&tokens.get(i+1).is_some_and(|t|t.is("SECURITY")){if !seen.insert("security"){bail!("重复SQL SECURITY");}i+=2;let value=tokens.get(i).ok_or_else(||anyhow!("SQL SECURITY缺失"))?;if !value.is("INVOKER")&&!value.is("DEFINER"){bail!("SQL SECURITY未知");}security=value.text.to_ascii_uppercase();i+=1;
        }else if !pg&&["CONTAINS","NO","READS","MODIFIES"].iter().any(|v|tokens[i].is(v)){if !seen.insert("data_access"){bail!("重复SQL DATA访问说明");}let start=i;let has_data=tokens[i].is("READS")||tokens[i].is("MODIFIES");i+=1;if !tokens.get(i).is_some_and(|t|t.is("SQL")){bail!("SQL DATA说明非法");}i+=1;if has_data{if !tokens.get(i).is_some_and(|t|t.is("DATA")){bail!("SQL DATA说明非法");}i+=1;}data_access=tokens[start..i].iter().map(|t|t.text.to_ascii_uppercase()).collect::<Vec<_>>().join(" ");
        }else if !pg&&(tokens[i].is("BEGIN")||tokens[i].is("RETURN")){
            let start=i;
            if tokens[i].is("BEGIN"){let mut depth=1;i+=1;while i<tokens.len()&&depth>0{if ["CASE","IF","LOOP","WHILE","REPEAT"].iter().any(|k|tokens[i].is(k)){bail!("ROUTINE_CREATION_BODY_UNSUPPORTED: MySQL复杂控制流未认证，当前仅简单BEGIN块");}if tokens[i].is("BEGIN"){depth+=1;}else if tokens[i].is("END"){depth-=1;}i+=1;}if depth!=0{bail!("BEGIN未闭合");}if i!=tokens.len(){bail!("ROUTINE_CREATION_MULTIPLE_SQL: END之后存在额外SQL");}
            }else{if object.kind!="function"{bail!("过程不能使用RETURN表达式body");}if tokens[i..].iter().any(|t|t.text==";"){bail!("ROUTINE_CREATION_MULTIPLE_SQL: RETURN后有顶层额外SQL");}i=tokens.len();}
            body=Some(normalized(&tokens[start..i]));
        }else{bail!("ROUTINE_CREATION_SQL_UNSUPPORTED: 未认证修饰/顶层额外SQL/SECURITY属性，不能普通创建");}
    }
    let body=body.ok_or_else(||anyhow!("缺少可核验body"))?;
    if pg&&language.is_none(){bail!("必须明确LANGUAGE sql/plpgsql");}
    let mut qualified=sql.to_string();qualified.replace_range(name_start..name_end,&db::qualified(kind,&object.schema,&object.name));
    Ok(Compiled{sql:qualified,body,language:language.unwrap_or_else(||"sql".into()),result_type,deterministic,data_access,security})
}
fn compile(object:&RoutineObjectId,kind:&DbKind,sql:&str)->Result<Compiled> {if !object.identity_arguments.is_empty(){bail!("ROUTINE_CREATION_SIGNATURE_UNSUPPORTED");}parse(object,kind,sql,false)}

fn proof(object:&RoutineObjectId,context:BTreeMap<String,String>)->Proof {let revision=crate::object_catalog::revision(&(object,&context,"complete_absent_name_group_v1"));Proof{context,revision}}
async fn pg_proof(conn:&mut sqlx::PgConnection,object:&RoutineObjectId)->Result<Proof> {
    let row=crate::sql_history::query("SELECT current_database() AS database,current_user::text AS current_user,n.oid::text AS schema_oid,n.nspowner::text AS schema_owner,(SELECT oid::text FROM pg_catalog.pg_roles WHERE rolname=current_user) AS role_oid,current_setting('server_version_num') AS version,pg_catalog.has_schema_privilege(n.oid,'CREATE') AS can_create FROM pg_catalog.pg_namespace n WHERE n.nspname=$1").bind(&object.schema).fetch_optional(&mut *conn).await?.ok_or_else(||anyhow!("ROUTINE_CREATION_SCHEMA_MISSING: 命名空间不存在"))?;
    if !row.try_get::<bool,_>("can_create")?{bail!("ROUTINE_CREATION_PERMISSION_UNKNOWN: 当前角色没有已证明的schema CREATE权限");}
    // pg_proc是非按例程权限过滤的真实目录；查询拒绝时不能宣称不存在。
    let existing=crate::sql_history::query("SELECT p.oid::text FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname=$1 AND p.proname=$2 LIMIT 1").bind(&object.schema).bind(&object.name).fetch_optional(&mut *conn).await?;
    if existing.is_some(){bail!("ROUTINE_CREATION_ALREADY_EXISTS: 同命名空间已有同名例程，不能创建或替换整组重载");}
    let mut context=BTreeMap::new();for key in ["database","current_user","schema_oid","schema_owner","role_oid","version"]{context.insert(key.into(),row.try_get::<String,_>(key)?);}context.insert("can_create".into(),"true".into());Ok(proof(object,context))
}
fn direct_mysql_permissions(grants:&[String],schema:&str)->Result<()> {
    let mut visible=false;let mut create=false;
    for grant in grants {
        let tokens=lex(grant,false)?;
        if tokens.first().is_some_and(|t|t.is("REVOKE")){bail!("ROUTINE_CREATION_PERMISSION_UNKNOWN: 直接权限包含局部撤销，不能证明目录完整可见");}
        if !tokens.first().is_some_and(|t|t.is("GRANT")){bail!("ROUTINE_CREATION_PERMISSION_UNKNOWN: SHOW GRANTS返回未知权限记录");}
        let Some(on)=tokens.iter().position(|t|t.is("ON")) else {continue;};
        let Some(to)=tokens.iter().position(|t|t.is("TO")) else {bail!("SHOW GRANTS缺少TO");};
        let level=&tokens[on+1..to];if level.len()!=3||level[1].text!="."||level[2].text!="*"{continue;}
        let global=level[0].text=="*";let same_schema=!global&&identifier(&level[0],false).is_ok_and(|name|name==schema);
        let privileges=tokens[1..on].split(|t|t.text==",").map(|part|part.iter().map(|t|t.text.to_ascii_uppercase()).collect::<Vec<_>>().join(" ")).collect::<Vec<_>>();
        let all=privileges.iter().any(|v|v=="ALL PRIVILEGES"||v=="ALL");
        if global&&(all||privileges.iter().any(|v|v=="SELECT")){visible=true;}
        if (global||same_schema)&&(all||privileges.iter().any(|v|v=="CREATE ROUTINE")){create=true;}
    }
    if !visible||!create{bail!("ROUTINE_CREATION_PERMISSION_UNKNOWN: MySQL新建要求直接global SELECT/ALL证明全目录可见和直接CREATE ROUTINE权限；角色/过滤空目录不是不存在证明");}Ok(())
}
async fn mysql_proof(conn:&mut sqlx::MySqlConnection,object:&RoutineObjectId)->Result<Proof> {
    let row=crate::sql_history::query("SELECT CURRENT_USER() AS current_user,DATABASE() AS database_name,@@version AS version,@@sql_mode AS sql_mode,@@character_set_client AS character_set_client,@@collation_connection AS collation_connection").fetch_one(&mut *conn).await?;
    let rows=crate::sql_history::query("SHOW GRANTS FOR CURRENT_USER()").fetch_all(&mut *conn).await?;
    let mut grants=Vec::new();for row in rows{grants.push(mysql_text(&row,0)?.ok_or_else(||anyhow!("SHOW GRANTS记录不可读"))?);}direct_mysql_permissions(&grants,&object.schema)?;
    let schema=crate::sql_history::query_scalar::<_,String>("SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME=?").bind(&object.schema).fetch_optional(&mut *conn).await?;
    if schema.as_deref()!=Some(&object.schema){bail!("ROUTINE_CREATION_SCHEMA_MISSING: 命名空间不存在或不可见");}
    let existing=crate::sql_history::query("SELECT ROUTINE_NAME FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA=? AND ROUTINE_NAME=? LIMIT 1").bind(&object.schema).bind(&object.name).fetch_optional(&mut *conn).await?;
    if existing.is_some(){bail!("ROUTINE_CREATION_ALREADY_EXISTS: 同命名空间已有同名函数/过程，没有发送DDL");}
    let mut context=BTreeMap::new();for key in ["current_user","version","sql_mode","character_set_client","collation_connection"]{context.insert(key.into(),row.try_get::<String,_>(key)?);}context.insert("database".into(),row.try_get::<Option<String>,_>("database_name")?.unwrap_or_default());context.insert("grants_revision".into(),crate::object_catalog::revision(&grants));
    let mode=&context["sql_mode"];if mode.split(',').any(|v|matches!(v.trim(),"ANSI_QUOTES"|"NO_BACKSLASH_ESCAPES")){bail!("ROUTINE_CREATION_SQL_MODE_UNSUPPORTED: 当前引用解释模式未经创建解析器认证");}Ok(proof(object,context))
}
fn mysql_text(row:&sqlx::mysql::MySqlRow,index:usize)->Result<Option<String>> {if let Ok(value)=row.try_get::<Option<String>,_>(index){return Ok(value);}let value:Option<Vec<u8>>=row.try_get(index)?;value.map(String::from_utf8).transpose().map_err(Into::into)}
fn mysql_named(row:&sqlx::mysql::MySqlRow,name:&str)->Result<Option<String>> {let index=row.columns().iter().position(|c|c.name().eq_ignore_ascii_case(name)).ok_or_else(||anyhow!("ROUTINE_CREATION_METADATA_MISSING: {name}"))?;mysql_text(row,index)}
async fn read_proof(db:&Db,object:&RoutineObjectId)->Result<Proof> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    match db {Db::Postgres(pool)=>{let mut conn=pool.acquire().await?;pg_proof(&mut conn,object).await},Db::Mysql(pool)=>{let mut conn=pool.acquire().await?;mysql_proof(&mut conn,object).await},_=>bail!("ROUTINE_CREATION_ADAPTER_UNSUPPORTED: 尚无该协议真实例程创建目录通道")}

    }).await
}
/// 父必须在调用前后重验保存/运行端点FP；Binding必须来自真实window，而非前端来源。
pub async fn baseline(db:&Db,kind:&DbKind,object:&RoutineObjectId,binding:&Binding,read_only:bool)->Result<RoutineCreationBaseline> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if read_only{return Err(AppError::coded("READ_ONLY_ROUTINE","只读连接禁止创建例程，未查询/发送SQL").into());}source_object(object,binding)?;
    if !matches!((db,kind.protocol()),(Db::Postgres(_),Protocol::Postgres)|(Db::Mysql(_),Protocol::Mysql)){bail!("ROUTINE_CREATION_ADAPTER_UNSUPPORTED: 仅PostgreSQL/MySQL原生协议能取创建基线");}
    let proof=read_proof(db,object).await?;global().insert(object,binding,kind,proof)

    }).await
}
pub async fn preview(db:&Db,kind:&DbKind,draft:&RoutineCreationDraft,binding:&Binding,read_only:bool)->Result<RoutineCreationPreview> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if read_only{return Err(AppError::coded("READ_ONLY_ROUTINE","只读连接禁止创建预览，未发送SQL").into());}
    let stored=global().checked(draft,binding)?;if &stored.kind!=kind{bail!("ROUTINE_CREATION_KIND_CHANGED");}let compiled=compile(&draft.object,kind,&draft.definition)?;
    let current=read_proof(db,&draft.object).await?;if current.revision!=stored.proof.revision{bail!("ROUTINE_CREATION_STALE: 不存在状态/namespace身份/权限/会话创建上下文已变化，未发送DDL");}
    let remaining=stored.expires.saturating_duration_since(Instant::now()).as_millis() as u64;if remaining==0{bail!("ROUTINE_CREATION_BASELINE_EXPIRED");}
    Ok(RoutineCreationPreview{baseline_id:draft.baseline_id.clone(),object:draft.object.clone(),source_fingerprint:draft.source_fingerprint.clone(),base_revision:draft.base_revision.clone(),steps:vec![RoutineStep{label:format!("创建{}",draft.object.kind),sql:compiled.sql}],request_sql:request_sql(draft)?,strategy:stored.baseline.strategy,executable:true,expires_in_ms:remaining,warnings:stored.baseline.warnings})

    }).await
}
fn validate_execution(work:&CreationExecution,entry:&Arc<Entry>)->CmdResult<()> {routine_design::validate_source_fingerprint(work.draft.source_fingerprint.as_deref(),&entry.binding.fingerprint)?;source_object(&work.draft.object,&entry.binding)?;if !same_binding(&work.stored.binding,&entry.binding){return Err(AppError::coded("ROUTINE_CREATION_SCOPE_MISMATCH","已消费创建基线与执行窗口/页面不同"));}Ok(())}
fn stopped()->anyhow::Error {AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND","创建发送前停止，没有发送DDL").into()}
fn unknown(message:impl Into<String>)->anyhow::Error {AppError::coded("EXECUTION_EFFECTS_UNKNOWN",message).into()}
async fn pg_rollback(conn:&mut sqlx::PgConnection)->Result<()> {crate::sql_history::query("ROLLBACK").execute(conn).await.map_err(|e|unknown(format!("创建事务回滚未确认；禁止自动重放：{e}")))?;Ok(())}
/// 仅execution_bound调用：从来不从Pool重新取连接，不签发grant/终态。
pub(crate) async fn pg_create(conn:&mut sqlx::PgConnection,work:&CreationExecution,entry:&Arc<Entry>)->Result<(MultiResult,RoutineCreationReceipt)> {
    validate_execution(work,entry)?;if entry.stopping(){return Err(stopped());}
    crate::sql_history::query("BEGIN").execute(&mut *conn).await?;
    let attempted:Result<RoutineSnapshot>=async {
        let current=pg_proof(conn,&work.draft.object).await?;if current.revision!=work.stored.proof.revision{bail!("ROUTINE_CREATION_STALE: 事务内不存在/权限基线已变化，没有发送DDL");}
        if entry.stopping(){return Err(stopped());}
        crate::sql_history::query(&work.compiled.sql).execute(&mut *conn).await?;
        let row=crate::sql_history::query("SELECT n.oid::text AS schema_oid,p.proowner::text AS owner_oid,p.prosrc,p.pronargs::int AS nargs,p.prosecdef,p.proconfig::text AS config,p.proisstrict,p.provolatile::text AS volatility,p.proparallel::text AS parallel,l.lanname AS language,pg_catalog.pg_get_function_result(p.oid) AS result_type FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid=p.pronamespace JOIN pg_catalog.pg_language l ON l.oid=p.prolang WHERE n.nspname=$1 AND p.proname=$2 AND p.prokind::text=$3 AND p.pronargs=0").bind(&work.draft.object.schema).bind(&work.draft.object.name).bind(if work.draft.object.kind=="function"{"f"}else{"p"}).fetch_optional(&mut *conn).await?.ok_or_else(||anyhow!("ROUTINE_CREATION_VERIFICATION_FAILED: 创建后精确对象不存在"))?;
        let verified=row.try_get::<String,_>("schema_oid")?==work.stored.proof.context["schema_oid"]&&row.try_get::<String,_>("owner_oid")?==work.stored.proof.context["role_oid"]&&row.try_get::<String,_>("prosrc")?==work.compiled.body&&row.try_get::<i32,_>("nargs")?==0&&!row.try_get::<bool,_>("prosecdef")?&&row.try_get::<Option<String>,_>("config")?.is_none()&&!row.try_get::<bool,_>("proisstrict")?&&row.try_get::<String,_>("volatility")?=="v"&&row.try_get::<String,_>("parallel")?=="u"&&row.try_get::<String,_>("language")?==work.compiled.language&&row.try_get::<Option<String>,_>("result_type")?==work.compiled.result_type;
        if !verified{bail!("ROUTINE_CREATION_VERIFICATION_FAILED: body/签名/返回型/语言/安全属性/namespace或owner读回不一致");}
        let mut snapshot=routine_design::pg_snapshot(conn,&work.draft.object).await?;snapshot.source_fingerprint=work.draft.source_fingerprint.clone();Ok(snapshot)
    }.await;
    let snapshot=match attempted {Ok(value)=>value,Err(error)=>{pg_rollback(conn).await?;return Err(AppError::coded("ROUTINE_CREATION_ROLLED_BACK",format!("创建未提交，事务已实际回滚：{error}")).into());}};
    if entry.stopping(){pg_rollback(conn).await?;return Err(AppError::coded("ROUTINE_STOPPED_ROLLED_BACK","创建被停止，新增例程事务已真实回滚").into());}
    crate::sql_history::query("COMMIT").execute(&mut *conn).await.map_err(|e|unknown(format!("创建COMMIT响应未知，禁止重放/自动恢复：{e}")))?;
    Ok(created(work,snapshot,"committed"))
}
async fn mysql_readback(conn:&mut sqlx::MySqlConnection,work:&CreationExecution)->Result<RoutineSnapshot> {
    let object=&work.draft.object;let word=if object.kind=="function"{"FUNCTION"}else{"PROCEDURE"};
    let create=crate::sql_history::query(&format!("SHOW CREATE {word} {}",db::qualified(&work.stored.kind,&object.schema,&object.name))).fetch_one(&mut *conn).await?;
    let column=if object.kind=="function"{"Create Function"}else{"Create Procedure"};let definition=mysql_named(&create,column)?.filter(|v|!v.is_empty()).ok_or_else(||anyhow!("ROUTINE_CREATION_VERIFICATION_FAILED: SHOW CREATE定义不可读"))?;
    let actual=parse(object,&work.stored.kind,&definition,true)?;
    let row=crate::sql_history::query("SELECT DTD_IDENTIFIER AS result_type,IS_DETERMINISTIC AS deterministic,SQL_DATA_ACCESS AS data_access,SECURITY_TYPE AS security,DEFINER AS definer,SQL_MODE AS sql_mode FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA=? AND ROUTINE_NAME=? AND ROUTINE_TYPE=?").bind(&object.schema).bind(&object.name).bind(word).fetch_one(&mut *conn).await?;
    let return_type=row.try_get::<Option<String>,_>("result_type")?.map(|v|mysql_return(&lex(&v,false)?)).transpose()?;
    if actual.body!=work.compiled.body||actual.result_type!=work.compiled.result_type||return_type!=work.compiled.result_type||actual.language!=work.compiled.language||row.try_get::<String,_>("deterministic")?!=work.compiled.deterministic||row.try_get::<String,_>("data_access")?!=work.compiled.data_access||row.try_get::<String,_>("security")?!=work.compiled.security||row.try_get::<String,_>("definer")?!=work.stored.proof.context["current_user"]||row.try_get::<String,_>("sql_mode")?!=work.stored.proof.context["sql_mode"]{bail!("ROUTINE_CREATION_VERIFICATION_FAILED: body/返回型/创建SQL_MODE/权限上下文读回不符");}
    let parameters=crate::sql_history::query_scalar::<_,i64>("SELECT COUNT(*) FROM information_schema.PARAMETERS WHERE SPECIFIC_SCHEMA=? AND SPECIFIC_NAME=? AND ORDINAL_POSITION>0").bind(&object.schema).bind(&object.name).fetch_one(&mut *conn).await?;
    if parameters!=0{bail!("ROUTINE_CREATION_VERIFICATION_FAILED: 零参数创建读回出现了参数");}
    let mut context=BTreeMap::new();for column_info in create.columns(){if !column_info.name().eq_ignore_ascii_case(column){if let Some(value)=mysql_named(&create,column_info.name())?{context.insert(column_info.name().to_string(),value);}}}
    context.insert("definer".into(),row.try_get::<String,_>("definer")?);context.insert("security_type".into(),row.try_get::<String,_>("security")?);
    let revision=crate::object_catalog::revision(&(object,&definition,&context));
    Ok(RoutineSnapshot{object:object.clone(),definition,revision,overloads:1,completeness:"complete".into(),source_fingerprint:work.draft.source_fingerprint.clone(),backend_object_id:None,creation_context:context,security_completeness:"unknown".into(),replacement_reason:"MySQL恢复旧ACL/DEFINER未认证，创建成功不授予DROP+CREATE替换能力".into()})
}
pub(crate) async fn mysql_create(conn:&mut sqlx::MySqlConnection,pool:&sqlx::MySqlPool,work:&CreationExecution,entry:&Arc<Entry>)->Result<(MultiResult,RoutineCreationReceipt)> {
    let conn = &mut *conn; // 借用重绑定：后续以同一个较短生命周期使用连接
    validate_execution(work,entry)?;if entry.stopping(){return Err(stopped());}
    let current=mysql_proof(conn,&work.draft.object).await?;if current.revision!=work.stored.proof.revision{bail!("ROUTINE_CREATION_STALE: MySQL不存在/创建上下文/权限已变化，没有发送DDL");}
    if entry.stopping(){return Err(stopped());}
    // parser已将完整输入限制为单独CREATE；绝不DROP/OR REPLACE，body内部语句只是定义数据。
    // 复合体（BEGIN..END）只能走 raw_sql 多语句；&mut 连接作为 Executor 的条件 Send
    // 实现会触发 rustc #102211「Executor<'0> 不够通用」，因此改由同一连接池执行
    // （池 Executor 编译可过）：CREATE 是 DDL，效果与会话无关，隐式提交语义一致；
    // 停止确认仍作用于本固定连接。
    crate::sql_history::raw_sql(&work.compiled.sql).execute(pool).await.map_err(|e|unknown(format!("MySQL CREATE执行结果/隐式提交未完整确认，禁止自动重放：{e}")))?;
    let snapshot=mysql_readback(conn,work).await.map_err(|e|unknown(format!("MySQL CREATE已ACK但读回未完整核验，可能已创建，禁止重放：{e}")))?;
    if entry.stopping(){return Err(unknown("MySQL CREATE可能已应用，停止不能冒充事务回滚；请人工重读对象"));}
    Ok(created(work,snapshot,"implicit_commit_acknowledged"))
}
fn created(work:&CreationExecution,snapshot:RoutineSnapshot,transaction:&str)->(MultiResult,RoutineCreationReceipt) {
    let receipt=RoutineCreationReceipt{object:work.draft.object.clone(),source_fingerprint:work.draft.source_fingerprint.clone(),baseline_id:work.draft.baseline_id.clone(),status:"applied".into(),verified:true,steps:vec![RoutineStepResult{label:format!("创建{}",work.draft.object.kind),sql:work.compiled.sql.clone(),status:"applied".into(),error:None}],transaction:transaction.into(),snapshot,warnings:work.stored.baseline.warnings.clone()};
    (MultiResult{sets:vec![],messages:vec!["新例程已按固定不存在基线CREATE并真实读回；没有删除或替换既有对象".into()],elapsed_ms:0,spool_id:None},receipt)
}

#[cfg(test)]mod tests {
    use super::*;
    fn object()->RoutineObjectId {RoutineObjectId{connection_id:"c".into(),schema:"public".into(),name:"fresh".into(),kind:"function".into(),identity_arguments:String::new()}}
    fn binding()->Binding {Binding{principal:"window".into(),owner:"page".into(),connection:"c".into(),namespace:"public".into(),fingerprint:"a".repeat(64)}}
    fn draft(base:&RoutineCreationBaseline)->RoutineCreationDraft {RoutineCreationDraft{baseline_id:base.baseline_id.clone(),object:base.object.clone(),source_fingerprint:base.source_fingerprint.clone(),base_revision:base.base_revision.clone(),definition:"CREATE FUNCTION fresh() RETURNS int AS $$ SELECT 'Keep Case' $$ LANGUAGE sql".into()}}
    #[test]fn pg_single_create_qualified_zero_signature_and_body_bytes(){let compiled=compile(&object(),&DbKind::of("postgresql"),"CREATE FUNCTION fresh() RETURNS integer AS $b$ SELECT 'Keep Case; DROP' $b$ LANGUAGE sql;").unwrap();assert!(compiled.sql.starts_with("CREATE FUNCTION \"public\".\"fresh\"()"));assert!(!compiled.sql.contains("OR REPLACE"));assert_eq!(compiled.body," SELECT 'Keep Case; DROP' ");assert_eq!(compiled.result_type,Some("integer".into()));}
    #[test]fn create_cannot_replace_drop_change_target_or_add_top_level_sql(){for sql in ["CREATE OR REPLACE FUNCTION fresh() RETURNS int AS $$ SELECT 1 $$ LANGUAGE sql","DROP FUNCTION fresh()","CREATE FUNCTION other() RETURNS int AS $$ SELECT 1 $$ LANGUAGE sql","CREATE FUNCTION evil.fresh() RETURNS int AS $$ SELECT 1 $$ LANGUAGE sql","CREATE FUNCTION fresh(x int) RETURNS int AS $$ SELECT 1 $$ LANGUAGE sql","CREATE FUNCTION fresh() RETURNS int AS $$ SELECT 1 $$ LANGUAGE sql; DROP TABLE x","CREATE FUNCTION fresh() RETURNS int AS $$ SELECT 1 $$ LANGUAGE sql SECURITY DEFINER","CREATE FUNCTION fresh() RETURNS int AS 'SELECT 1' LANGUAGE sql"]{assert!(compile(&object(),&DbKind::of("postgresql"),sql).is_err(),"{sql}");}}
    #[test]fn mysql_simple_compound_body_does_not_allow_trailing_ddl(){let mut target=object();target.kind="procedure".into();let kind=DbKind::of("mysql");let sql="CREATE PROCEDURE fresh() BEGIN SELECT 'a; Keep Case'; SELECT 2; END;";let compiled=compile(&target,&kind,sql).unwrap();assert!(compiled.sql.starts_with("CREATE PROCEDURE `public`.`fresh`()"));assert!(compiled.body.contains("Keep Case"));for extra in [" DROP TABLE x","; CREATE TABLE x(id int)"]{assert!(compile(&target,&kind,&format!("{sql}{extra}")).is_err());}assert!(compile(&target,&kind,"CREATE DEFINER=`root`@`%` PROCEDURE fresh() BEGIN END").is_err());}
    #[test]fn executable_comments_and_ambiguous_escapes_are_rejected(){for sql in ["/*! CREATE FUNCTION fresh() RETURNS int RETURN 1 */","/*M! CREATE FUNCTION fresh() RETURNS int RETURN 1 */","CREATE FUNCTION fresh() RETURNS int RETURN 'a\\b'"]{assert!(compile(&object(),&DbKind::of("mysql"),sql).is_err());}}
    #[test]fn mysql_filtered_catalog_or_roles_are_not_absence_proof(){for grants in [vec!["GRANT EXECUTE ON *.* TO `u`@`%`".into()],vec!["GRANT `administrator`@`%` TO `u`@`%`".into()],vec!["GRANT ALL PRIVILEGES ON *.* TO `u`@`%`".into(),"REVOKE SELECT ON `public`.* FROM `u`@`%`".into()]]{assert!(direct_mysql_permissions(&grants,"public").is_err());}assert!(direct_mysql_permissions(&["GRANT ALL PRIVILEGES ON *.* TO `u`@`%` WITH GRANT OPTION".into()],"public").is_ok());assert!(direct_mysql_permissions(&["GRANT SELECT ON *.* TO `u`@`%`".into(),"GRANT CREATE ROUTINE ON `public`.* TO `u`@`%`".into()],"public").is_ok());}
    #[test]fn immutable_absence_baseline_has_full_window_scope_and_single_use(){let registry=Registry::default();let target=object();let actor=binding();let proof=proof(&target,BTreeMap::new());let base=registry.insert(&target,&actor,&DbKind::of("postgresql"),proof).unwrap();let draft=draft(&base);registry.validate_draft(&draft,&actor).unwrap();for field in 0..5 { let mut altered=actor.clone(); match field { 0=>altered.principal.push('x'),1=>altered.owner.push('x'),2=>altered.connection.push('x'),3=>altered.namespace.push('x'),_=>altered.fingerprint="b".repeat(64) }; assert!(registry.validate_draft(&draft,&altered).is_err()); }let mut changed=draft.clone();changed.source_fingerprint=Some("b".repeat(64));assert!(registry.validate_draft(&changed,&actor).is_err());changed.source_fingerprint=None;assert_eq!(registry.validate_draft(&changed,&actor).unwrap_err().code,"ROUTINE_SOURCE_REQUIRED");let marker=request_sql(&draft).unwrap();let mut literal=draft.clone();literal.definition=literal.definition.replace("Keep Case","keep case");assert_ne!(marker,request_sql(&literal).unwrap());assert!(!marker.contains("SELECT"));registry.take(&draft,&actor,&DbKind::of("postgresql")).unwrap();assert!(registry.take(&draft,&actor,&DbKind::of("postgresql")).is_err());}
    #[test]fn baseline_expiry_and_fake_existing_snapshot_cannot_create(){let registry=Registry::default();let target=object();let actor=binding();let base=registry.insert(&target,&actor,&DbKind::of("postgresql"),proof(&target,BTreeMap::new())).unwrap();let mut input=draft(&base);input.baseline_id="snapshot-revision".into();assert!(registry.validate_draft(&input,&actor).is_err());input=draft(&base);registry.baselines.lock().get_mut(&base.baseline_id).unwrap().expires=Instant::now()-Duration::from_secs(1);assert!(registry.validate_draft(&input,&actor).is_err());}
    #[tokio::test]async fn readonly_or_wrong_endpoint_baseline_is_zero_sql_even_with_busy_sqlite_pool(){let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();let mut held=pool.acquire().await.unwrap();crate::sql_history::query("CREATE TABLE untouched(id int)").execute(&mut *held).await.unwrap();let db=Db::Sqlite(pool);let result=tokio::time::timeout(Duration::from_millis(200),baseline(&db,&DbKind::of("sqlite"),&object(),&binding(),true)).await.unwrap();assert_eq!(result.unwrap_err().downcast_ref::<AppError>().unwrap().code,"READ_ONLY_ROUTINE");let mut actor=binding();actor.namespace="other".into();let result=tokio::time::timeout(Duration::from_millis(200),baseline(&db,&DbKind::of("sqlite"),&object(),&actor,false)).await.unwrap();assert!(result.is_err());assert_eq!(crate::sql_history::query_scalar::<_,i64>("SELECT COUNT(*) FROM sqlite_schema WHERE name='untouched'").fetch_one(&mut *held).await.unwrap(),1);}
}
