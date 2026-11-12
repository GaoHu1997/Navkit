//! PL-09: 单对象外层语法验证与编辑基线 CAS。旧按名整组定义命令保留。
//! 旧无 C1 Entry 的 apply 拒绝执行；新执行入口经 execution_bound 保持真实 lease/控制/终态。
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Column, Row};
use crate::{db::{self, Db}, models::{DbKind, Protocol}};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineObjectId {
    #[serde(default)] pub connection_id: String,
    #[serde(default)] pub schema: String,
    pub name: String,
    pub kind: String,
    #[serde(default)] pub identity_arguments: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutineSnapshot {
    pub object: RoutineObjectId, pub definition: String, pub revision: String, pub overloads: usize,
    pub completeness: String,
    /// 由可信IPC在读取前后端点CAS后填写；原生读取不生成来源证明。
    #[serde(default)] pub source_fingerprint: Option<String>,
    pub backend_object_id: Option<String>,
    pub creation_context: std::collections::BTreeMap<String,String>,
    pub security_completeness: String,
    pub replacement_reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutineReplacementDraft {
    pub object: RoutineObjectId,
    #[serde(default)] pub base_revision: String,
    /// 固定编辑器加载时来源，旧草稿缺失时必须拒绝。
    #[serde(default)] pub source_fingerprint: Option<String>,
    pub definition: String,
    #[serde(default)] pub allow_drop_recreate: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct RoutineStep { pub label: String, pub sql: String }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct RoutineStepResult { pub label: String, pub sql: String, pub status: String, #[serde(default)] pub error: Option<String> }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutineReceipt {
    pub steps: Vec<RoutineStepResult>, pub verified: bool, pub mismatches: Vec<String>, pub strategy: String,
    pub transaction: String, pub revision: String, pub definition: String,
    pub status: String, pub original_revision: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutineReplacementPreview {
    pub object: RoutineObjectId, pub base_revision: String, pub backend_object_id: Option<String>,
    #[serde(default)] pub source_fingerprint: Option<String>,
    pub steps: Vec<RoutineStep>, pub strategy: String, pub executable: bool, pub warnings: Vec<String>,
    /// 给C1签发/执行的工作标识；前端不能自行散列SQL或替换草稿。
    pub request_sql: String,
}
/// 缺失/旧格式来源不能在保存时用当前端点补齐。此检查不执行数据库操作。
pub fn require_source_fingerprint(source: Option<&str>) -> crate::error::CmdResult<&str> {
    source.filter(|value| value.len()==64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .ok_or_else(|| crate::error::AppError::coded("ROUTINE_SOURCE_REQUIRED", "缺少有效的加载时端点指纹，请重新读取例程后合并草稿；没有发送SQL"))
}
pub fn validate_source_fingerprint(source: Option<&str>, expected: &str) -> crate::error::CmdResult<()> {
    let source=require_source_fingerprint(source)?;
    let expected=require_source_fingerprint(Some(expected))?;
    if source!=expected {return Err(crate::error::AppError::coded("ROUTINE_SOURCE_CHANGED", "连接端点与例程加载时不同，请重新读取并核对；没有发送SQL"));}
    Ok(())
}
#[derive(Clone, Debug, PartialEq, Eq)] enum TokenKind { Word, Identifier, Literal, Dollar, Symbol }
#[derive(Clone, Debug)] struct Token { text: String, start: usize, end: usize, kind: TokenKind }
impl Token { fn is(&self,s:&str)->bool { self.kind==TokenKind::Word && self.text.eq_ignore_ascii_case(s) } }
/// 按字节定位保留源 SQL，不切 body 内部的分号。可执行 MySQL 版本注释明确拒绝。
fn lex(sql:&str)->Result<Vec<Token>> {
    if sql.len()>2*1024*1024 || sql.contains('\0') { bail!("例程定义超过2MiB或包含NUL"); }
    let b=sql.as_bytes(); let mut i=0; let mut out=Vec::new();
    while i<b.len() {
        if b[i].is_ascii_whitespace() { i+=1; continue; }
        if b[i..].starts_with(b"--") { while i<b.len() && b[i]!=b'\n' { i+=1; } continue; }
        if b[i..].starts_with(b"/*") {
            if b.get(i+2)==Some(&b'!') || b.get(i+2)==Some(&b'+') { bail!("可执行/提示注释未认证，拒绝例程保存"); }
            i+=2; let mut depth=1;
            while i<b.len() && depth>0 { if b[i..].starts_with(b"/*") {depth+=1;i+=2;}else if b[i..].starts_with(b"*/"){depth-=1;i+=2;}else{i+=1;} }
            if depth!=0 { bail!("例程定义注释未闭合"); } continue;
        }
        let start=i; let kind;
        if matches!(b[i],b'\''|b'"'|b'`') {
            let quote=b[i]; kind=if quote==b'\''{TokenKind::Literal}else{TokenKind::Identifier}; i+=1; let mut closed=false;
            while i<b.len() { if b[i]==quote { i+=1; if b.get(i)==Some(&quote){i+=1;continue;} closed=true;break; } if b[i]==b'\\' && quote==b'\'' { i+=1; if i<b.len(){i+=1;} }else{i+=1;} }
            if !closed { bail!("例程定义引用未闭合"); }
        } else if b[i]==b'$' {
            let mut tag_end=i+1; while tag_end<b.len() && (b[tag_end].is_ascii_alphanumeric()||b[tag_end]==b'_'){tag_end+=1;}
            if b.get(tag_end)==Some(&b'$') {
                let tag=&sql[i..=tag_end]; i=tag_end+1;
                let end=sql[i..].find(tag).ok_or_else(||anyhow!("dollar quote未闭合"))?; i+=end+tag.len(); kind=TokenKind::Dollar;
            }else{ i+=1;kind=TokenKind::Symbol; }
        } else if b[i].is_ascii_alphanumeric()||b[i]==b'_'||b[i]>=128 {
            i+=1; while i<b.len() && (b[i].is_ascii_alphanumeric()||b[i]==b'_'||b[i]==b'$'||b[i]>=128){i+=1;} kind=TokenKind::Word;
        } else { i+=1;kind=TokenKind::Symbol; }
        out.push(Token {text:sql[start..i].into(),start,end:i,kind});
    }
    Ok(out)
}
fn token_normal(t:&Token)->String {
    match t.kind {
        TokenKind::Word=>t.text.to_ascii_lowercase(),
        TokenKind::Dollar=>{ let n=t.text[1..].find('$').unwrap()+2; format!("DOLLAR:{}",&t.text[n..t.text.len()-n]) },
        _=>t.text.clone(),
    }
}
/// 仅外层 token 空白/关键字大小写规范化；引用标识符、literal、body字节完全保留。
fn normalize(sql:&str)->String {
    match lex(sql) { Ok(mut tokens)=> { if tokens.last().is_some_and(|t|t.text==";"){tokens.pop();} tokens.iter().map(|t| {let s=token_normal(t);format!("{}:{s}",s.len())}).collect::<Vec<_>>().join("|") }, Err(_)=>sql.to_string() }
}
fn revision_of(definitions:&[String])->String {
    let mut h=Sha256::new(); for d in definitions { let n=normalize(d);h.update(n.len().to_le_bytes());h.update(n.as_bytes()); } format!("{:x}",h.finalize())
}
fn routine_word(kind:&str)->Result<&'static str>{match kind{"function"=>Ok("FUNCTION"),"procedure"=>Ok("PROCEDURE"),_=>bail!("仅支持函数或存储过程，收到未知类型：{kind}")}}
fn identifier(token:&Token,pg:bool)->Result<String>{
    match token.kind {
        TokenKind::Identifier=>{let q=token.text.chars().next().unwrap();Ok(token.text[1..token.text.len()-1].replace(&format!("{q}{q}"),&q.to_string()))},
        TokenKind::Word=>Ok(if pg{token.text.to_ascii_lowercase()}else{token.text.clone()}),
        _=>bail!("对象标识符非法"),
    }
}
struct Header { tokens:Vec<Token>, name_start:usize, name_end:usize, schema:Option<String>, name:String, kind:String, arguments:String, returns:Option<String>, create_end:usize, replace:bool, body_index:Option<usize> }
fn header(sql:&str,pg:bool)->Result<Header>{
    let tokens=lex(sql)?; if tokens.is_empty(){bail!("定义不能为空");}
    if !tokens[0].is("CREATE"){bail!("定义必须以CREATE语句开始");}
    let create_end=tokens[0].end; let mut i=1; let mut replace=false;
    if tokens.get(i).is_some_and(|t|t.is("OR")){if !tokens.get(i+1).is_some_and(|t|t.is("REPLACE")){bail!("未知CREATE修饰");}replace=true;i+=2;}
    if !pg && tokens.get(i).is_some_and(|t|t.is("DEFINER")) {
        i+=1; if !tokens.get(i).is_some_and(|t|t.text=="="){bail!("DEFINER语法非法");} i+=1;
        if tokens.get(i).is_some_and(|t|t.is("CURRENT_USER")) { i+=1; if tokens.get(i).is_some_and(|t|t.text=="("){if !tokens.get(i+1).is_some_and(|t|t.text==")"){bail!("DEFINER非法");}i+=2;} }
        else { if !tokens.get(i).is_some_and(|t|matches!(t.kind,TokenKind::Word|TokenKind::Identifier|TokenKind::Literal)){bail!("DEFINER非法");}i+=1;if !tokens.get(i).is_some_and(|t|t.text=="@"){bail!("DEFINER缺主机");}i+=2; }
    }
    let kind=if tokens.get(i).is_some_and(|t|t.is("FUNCTION")){"function"}else if tokens.get(i).is_some_and(|t|t.is("PROCEDURE")){"procedure"}else{bail!("一次只能替换一个例程，必须是CREATE FUNCTION/PROCEDURE");}; i+=1;
    let first=tokens.get(i).ok_or_else(||anyhow!("缺少对象名称"))?; let name_start=first.start; let mut name=identifier(first,pg)?; i+=1; let mut schema=None;
    if tokens.get(i).is_some_and(|t|t.text=="."){schema=Some(name);i+=1;name=identifier(tokens.get(i).ok_or_else(||anyhow!("对象名缺失"))?,pg)?;i+=1;}
    let name_end=tokens.get(i-1).unwrap().end;
    if !tokens.get(i).is_some_and(|t|t.text=="("){bail!("例程必须带参数括号");}
    let arg_start=i+1; i+=1;let mut depth=1;
    while i<tokens.len() && depth>0 { if tokens[i].text=="("{depth+=1;}else if tokens[i].text==")"{depth-=1;} i+=1; }
    if depth!=0{bail!("参数括号未闭合");} let arguments=tokens[arg_start..i-1].iter().map(token_normal).collect::<Vec<_>>().join("|");
    let returns=if tokens.get(i).is_some_and(|t|t.is("RETURNS")){let start=i+1;let mut end=start;while end<tokens.len() && !["AS","LANGUAGE","BEGIN","RETURN","IMMUTABLE","STABLE","VOLATILE","SECURITY","DETERMINISTIC","NOT","CONTAINS","READS","MODIFIES","COMMENT"].iter().any(|k|tokens[end].is(k)){end+=1;}Some(tokens[start..end].iter().map(token_normal).collect::<Vec<_>>().join("|"))}else{None};
    // PostgreSQL body仅接受引用的AS单元；未知SQL BEGIN ATOMIC不冒充可靠解析。
    let mut body_index=None;
    if pg {
        let as_index=(i..tokens.len()).find(|n|tokens[*n].is("AS")).ok_or_else(||anyhow!("PostgreSQL定义必须带可验证AS body；BEGIN ATOMIC尚未支持"))?;
        let literal_index=if tokens.get(as_index+1).is_some_and(|t|t.is("E")){as_index+2}else{as_index+1};
        if !tokens.get(literal_index).is_some_and(|t|matches!(t.kind,TokenKind::Literal|TokenKind::Dollar)){bail!("PostgreSQL body必须是独立引用单元");}
        body_index=Some(literal_index);
        // 顶层分号只能是末尾。body中的所有分号已经是literal/dollar token内部数据。
        if tokens.iter().enumerate().any(|(n,t)|t.text==";" && n+1<tokens.len()) {bail!("一次只能替换一个例程；检测到额外顶层SQL");}
        if tokens.iter().skip(i).any(|t|t.is("CREATE")||t.is("DROP")||t.is("ALTER")){bail!("一次只能替换一个例程；检测到额外DDL");}
    } else {
        let body=(i..tokens.len()).find(|n|tokens[*n].is("BEGIN")||tokens[*n].is("RETURN")).ok_or_else(||anyhow!("MySQL例程body必须是可验证BEGIN...END或RETURN单元"))?;
        if tokens[body].is("BEGIN") {
            let mut level=1;let mut n=body+1;let mut end=None;
            while n<tokens.len(){if tokens[n].is("BEGIN"){level+=1;}if tokens[n].is("END") && !tokens.get(n+1).is_some_and(|t|["IF","LOOP","CASE","WHILE","REPEAT"].iter().any(|k|t.is(k))){level-=1;if level==0{end=Some(n);break;}}n+=1;}
            let end=end.ok_or_else(||anyhow!("BEGIN...END未闭合；复杂控制流请使用专门SQL批处理"))?;
            if tokens[end+1..].iter().any(|t|t.text!=";") || tokens[end+1..].len()>1{bail!("一次只能替换一个例程；END之后存在额外SQL");}
        } else if tokens[body..].iter().enumerate().any(|(n,t)|t.text==";" && body+n+1<tokens.len()){bail!("一次只能替换一个例程；RETURN之后存在额外SQL");}
    }
    Ok(Header{tokens,name_start,name_end,schema,name,kind:kind.into(),arguments,returns,create_end,replace,body_index})
}
fn outer_key(h:&Header)->String {
    h.tokens.iter().enumerate().filter_map(|(i,t)| {
        if t.start>=h.name_start&&t.end<=h.name_end{return (t.start==h.name_start).then(||"OBJECT".to_string());}
        if h.body_index==Some(i){return Some("BODY".into());}
        if i==1&&t.is("OR")||i==2&&h.tokens.get(1).is_some_and(|t|t.is("OR"))&&t.is("REPLACE")||t.text==";"{return None;}
        Some(token_normal(t))
    }).collect::<Vec<_>>().join("|")
}
pub(crate) fn definition_parameter_count(sql:&str,pg:bool)->Result<usize>{
    let h=header(sql,pg)?;let mut depth=0;let mut count=0;let mut started=false;
    for t in h.tokens.iter().skip_while(|t|t.end<=h.name_end) {
        if t.text=="(" {depth+=1;if depth==1{continue;}}
        if t.text==")" {depth-=1;if depth==0{break;}}
        if depth==1&&t.text==","{count+=1;}else if depth>0{started=true;}
    }
    Ok(if started{count+1}else{0})
}
fn body_only(current:&Header,next:&Header)->Result<()> {
    if outer_key(current)!=outer_key(next){bail!("ROUTINE_ATTRIBUTES_UNSUPPORTED: 当前原生安全保存仅支持修改body；LANGUAGE/SECURITY/配置/其它属性变更须专门预览和核验");}
    if !next.body_index.is_some_and(|i|next.tokens[i].kind==TokenKind::Dollar){bail!("ROUTINE_BODY_UNSUPPORTED: 安全替换body要求dollar quote，避免SQL string模式解释差异");}
    Ok(())
}
fn match_target(h:&Header,object:&RoutineObjectId)->Result<()> {
    if h.kind!=object.kind || h.name!=object.name || h.schema.as_ref().is_some_and(|s|s!=&object.schema){bail!("定义中的完整对象身份与所选对象不一致；改名/改签名需独立新建删除流程");} Ok(())
}
pub(crate) fn compile(kind:&DbKind,current:&[String],draft:&RoutineReplacementDraft)->Result<(Vec<RoutineStep>,String)> {
    let pg=kind.protocol()==Protocol::Postgres; let h=header(&draft.definition,pg)?; match_target(&h,&draft.object)?; routine_word(&draft.object.kind)?;
    if current.len()>1{bail!("一次只能替换一个例程；不能对整组重载脚本保存");}
    if let Some(old)=current.first(){let old=header(old,pg)?;match_target(&old,&draft.object)?;if old.arguments!=h.arguments{bail!("例程参数身份/名称/默认值变化不能通过普通保存完成；请单独预览签名变更");}if pg && old.returns!=h.returns{bail!("PostgreSQL函数返回类型变化不能通过CREATE OR REPLACE完成");}}
    let word=routine_word(&draft.object.kind)?;
    let full=db::qualified(kind,&draft.object.schema,&draft.object.name);
    let mut sql=draft.definition.clone(); sql.replace_range(h.name_start..h.name_end,&full);
    match kind.protocol() {
        Protocol::Postgres=>{if !h.replace{sql.insert_str(h.create_end," OR REPLACE");}Ok((vec![RoutineStep{label:format!("原生替换{word}"),sql}],"create_or_replace".into()))},
        Protocol::Mysql=>{if h.replace{bail!("MySQL/MariaDB CREATE OR REPLACE未经版本认证");}if !current.is_empty()&&!draft.allow_drop_recreate{bail!("MySQL既有例程替换需要DROP+CREATE显式确认");}let mut steps=Vec::new();if !current.is_empty(){steps.push(RoutineStep{label:format!("删除旧{word}"),sql:format!("DROP {word} {full}")});}steps.push(RoutineStep{label:format!("创建{word}"),sql});Ok((steps,if current.is_empty(){"create"}else{"drop_recreate"}.into()))},
        _=>bail!("当前数据源尚未接入例程安全替换；没有执行任何SQL"),
    }
}
fn validate_object(object:&RoutineObjectId)->Result<()> {
    routine_word(&object.kind)?;
    if object.connection_id.is_empty()||object.schema.is_empty()||object.name.is_empty()||object.name.contains('\0')||object.schema.contains('\0'){bail!("ROUTINE_ID_INVALID: 必须提供完整连接/命名空间/对象身份");}
    if object.schema.starts_with("pg_")||object.schema=="information_schema"||object.schema=="mysql" {bail!("ROUTINE_SYSTEM_OBJECT: 禁止图形化修改系统例程");} Ok(())
}
pub(crate) async fn pg_snapshot(conn:&mut sqlx::PgConnection,object:&RoutineObjectId)->Result<RoutineSnapshot>{
    validate_object(object)?;
    let row=crate::sql_history::query("SELECT p.oid::text AS oid, pg_catalog.pg_get_functiondef(p.oid) AS definition, pg_get_userbyid(p.proowner) AS owner,p.prosecdef::text AS security_definer,p.proacl::text AS acl,p.proconfig::text AS config FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname=$1 AND p.proname=$2 AND p.prokind::text=$3 AND pg_catalog.pg_get_function_identity_arguments(p.oid)=$4").bind(&object.schema).bind(&object.name).bind(if object.kind=="procedure"{"p"}else{"f"}).bind(&object.identity_arguments).fetch_optional(conn).await?.ok_or_else(||anyhow!("ROUTINE_NOT_FOUND: 精确签名不存在，不能自动选首个同名重载"))?;
    let definition:String=row.try_get("definition")?;let oid:String=row.try_get("oid")?; let mut context=std::collections::BTreeMap::new();
    for key in ["owner","security_definer","acl","config"] {if let Some(v)=row.try_get::<Option<String>,_>(key)?{context.insert(key.into(),v);}}
    let revision=crate::object_catalog::revision(&(object,&oid,&definition,&context));
    Ok(RoutineSnapshot{object:object.clone(),source_fingerprint:None,definition,revision,overloads:1,completeness:"complete".into(),backend_object_id:Some(oid),creation_context:context,security_completeness:"partial".into(),replacement_reason:"PostgreSQL仅原生替换；依赖/角色恢复完整性尚未认证，禁止DROP重建".into()})
}
pub async fn snapshot(db:&Db,kind:&DbKind,object:RoutineObjectId)->Result<RoutineSnapshot>{
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    validate_object(&object)?;
    match db {
        Db::Postgres(pool)=>{let mut conn=pool.acquire().await?;pg_snapshot(&mut conn,&object).await},
        Db::Mysql(pool)=>{
            if !object.identity_arguments.is_empty(){bail!("ROUTINE_SIGNATURE_INVALID: MySQL没有按签名重载");}
            let word=routine_word(&object.kind)?;let row=crate::sql_history::query(&format!("SHOW CREATE {word} {}",db::qualified(kind,&object.schema,&object.name))).fetch_one(pool).await?;
            let definition_column=if object.kind=="function"{"Create Function"}else{"Create Procedure"};
            let text=|name:&str|->Result<Option<String>>{let idx=row.columns().iter().position(|c|c.name().eq_ignore_ascii_case(name)).ok_or_else(||anyhow!("ROUTINE_METADATA_COLUMN_MISSING: {name}"))?;if let Ok(s)=row.try_get::<Option<String>,_>(idx){return Ok(s);}let bytes:Option<Vec<u8>>=row.try_get(idx)?;bytes.map(String::from_utf8).transpose().map_err(Into::into)};
            let definition=text(definition_column)?.filter(|s|!s.is_empty()).ok_or_else(||anyhow!("ROUTINE_DEFINITION_UNREADABLE: SHOW CREATE定义为空/无权限"))?;
            let mut context=std::collections::BTreeMap::new();for c in row.columns(){if !c.name().eq_ignore_ascii_case(definition_column){if let Some(v)=text(c.name())?{context.insert(c.name().into(),v);}}}
            let revision=crate::object_catalog::revision(&(&object,&definition,&context));
            Ok(RoutineSnapshot{object,source_fingerprint:None,definition,revision,overloads:1,completeness:"complete".into(),backend_object_id:None,creation_context:context,security_completeness:"unknown".into(),replacement_reason:"MySQL DROP+CREATE恢复ACL/DEFINER/创建权限与同会话SQL_MODE尚未认证，拒绝破坏性应用".into()})
        },
        _=>bail!("当前数据源尚未接入例程定义读取；没有执行任何查询"),
    }

    }).await
}
pub async fn preview(db:&Db,kind:&DbKind,draft:&RoutineReplacementDraft)->Result<RoutineReplacementPreview>{
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    require_source_fingerprint(draft.source_fingerprint.as_deref())?;
    if draft.base_revision.is_empty(){bail!("ROUTINE_BASELINE_REQUIRED: 必须使用编辑器打开时的基线revision，不允许保存时重读新基线");}
    let current=snapshot(db,kind,draft.object.clone()).await?;
    if current.revision!=draft.base_revision{bail!("ROUTINE_STALE: 对象定义/身份/上下文已变化；保存被拒绝，请重新读取后合并");}
    let (steps,strategy)=compile(kind,&[current.definition.clone()],draft)?;
    if kind.protocol()==Protocol::Postgres{body_only(&header(&current.definition,true)?,&header(&draft.definition,true)?)?;}
    Ok(RoutineReplacementPreview{object:draft.object.clone(),source_fingerprint:draft.source_fingerprint.clone(),base_revision:draft.base_revision.clone(),backend_object_id:current.backend_object_id,steps,strategy,request_sql:crate::execution_bound::replacement_request_sql(draft)?,executable:kind.protocol()==Protocol::Postgres,warnings:vec![current.replacement_reason,"外部DDL可能在读基线与替换之间竞争；原生替换不会删除重载/权限，不提供未经证明的原子CAS承诺".into()]})

    }).await
}
/// 兼容旧命令的fail-closed入口；任何裸execute都不能绕过C1注册表。
pub async fn apply(_db:&Db,_kind:&DbKind,_draft:&RoutineReplacementDraft)->Result<RoutineReceipt>{
    let __sql_history_context = crate::sql_history::db_context(_db);
    crate::sql_history::scope(__sql_history_context, async {bail!("ROUTINE_CONTROLLED_EXECUTION_REQUIRED: 请使用预览计划和C1授权的受控执行入口；没有执行SQL")
    }).await
}
/// 仅由 execution_bound 调用：同一pinned连接上重查基线、执行、读回；事务与unknown如实记录。
pub(crate) async fn pg_apply(conn:&mut sqlx::PgConnection,kind:&DbKind,draft:&RoutineReplacementDraft,entry:&std::sync::Arc<crate::execution::Entry>)->Result<(crate::models::MultiResult,RoutineReceipt)>{
    validate_source_fingerprint(draft.source_fingerprint.as_deref(),&entry.binding.fingerprint)?;
    if draft.base_revision.is_empty(){bail!("ROUTINE_BASELINE_REQUIRED");}
    let current=pg_snapshot(conn,&draft.object).await?;
    if current.revision!=draft.base_revision{bail!("ROUTINE_STALE: 执行前对象已变化，没有发送DDL");}
    let (steps,strategy)=compile(kind,&[current.definition.clone()],draft)?;
    body_only(&header(&current.definition,true)?,&header(&draft.definition,true)?)?;
    if entry.stopping(){return Err(crate::error::AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND","例程基线读取期间停止，没有发送DDL").into());}
    crate::sql_history::query("BEGIN").execute(&mut *conn).await?;
    if entry.stopping(){crate::sql_history::query("ROLLBACK").execute(&mut *conn).await.map_err(|_|crate::error::AppError::coded("EXECUTION_EFFECTS_UNKNOWN","发送前停止但回滚未确认"))?;return Err(crate::error::AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND","没有发送DDL，BEGIN已回滚").into());}
    let result=crate::sql_history::query(&steps[0].sql).execute(&mut *conn).await;
    if let Err(error)=result {let rolled=crate::sql_history::query("ROLLBACK").execute(&mut *conn).await;if let Err(rollback)=rolled{return Err(crate::error::AppError::coded("EXECUTION_EFFECTS_UNKNOWN",format!("例程失败后回滚未确认：{rollback}")).into());}return Err(error.into());}
    let after=match pg_snapshot(conn,&draft.object).await {Ok(v)=>v,Err(error)=>{let rolled=crate::sql_history::query("ROLLBACK").execute(&mut *conn).await;if rolled.is_err(){return Err(crate::error::AppError::coded("EXECUTION_EFFECTS_UNKNOWN","例程读回失败且回滚未确认").into());}return Err(error);}};
    // canonical pg_get_functiondef允许格式变化，但literal/body仍严格比较；不能凭旧定义假成功。
    let expected=header(&draft.definition,true)?;let actual=header(&after.definition,true)?;
    let expected_body=expected.body_index.map(|i|token_normal(&expected.tokens[i]));
    let actual_body=actual.body_index.map(|i|token_normal(&actual.tokens[i]));
    let verified=expected_body==actual_body && outer_key(&expected)==outer_key(&actual) && current.creation_context==after.creation_context && current.backend_object_id==after.backend_object_id;
    if !verified {crate::sql_history::query("ROLLBACK").execute(&mut *conn).await.map_err(|_|crate::error::AppError::coded("EXECUTION_EFFECTS_UNKNOWN","读回不符且回滚未确认"))?;bail!("ROUTINE_VERIFICATION_FAILED: 定义/参数/返回类型/OID读回不一致，已回滚");}
    if entry.stopping(){crate::sql_history::query("ROLLBACK").execute(&mut *conn).await.map_err(|_|crate::error::AppError::coded("EXECUTION_EFFECTS_UNKNOWN","停止后回滚未确认"))?;return Err(crate::error::AppError::coded("ROUTINE_STOPPED_ROLLED_BACK","已停止，DDL事务已真实回滚").into());}
    crate::sql_history::query("COMMIT").execute(&mut *conn).await.map_err(|e|crate::error::AppError::coded("EXECUTION_EFFECTS_UNKNOWN",format!("例程提交响应未知；禁止自动重试或恢复：{e}")))?;
    let receipt=RoutineReceipt{steps:vec![RoutineStepResult{label:steps[0].label.clone(),sql:steps[0].sql.clone(),status:"applied".into(),error:None}],verified:true,mismatches:vec![],strategy,transaction:"committed".into(),revision:after.revision,definition:after.definition,status:"applied".into(),original_revision:current.revision};
    Ok((crate::models::MultiResult{sets:vec![],messages:vec!["已按选中身份原生替换并读回核验；未删除其它重载".into()],elapsed_ms:0,spool_id:None},receipt))
}

#[cfg(test)] mod tests {
    use super::*;
    fn object(kind:&str)->RoutineObjectId{RoutineObjectId{connection_id:"c".into(),schema:"public".into(),name:"p_total".into(),kind:kind.into(),identity_arguments:String::new()}}
    fn draft(def:&str,allow:bool)->RoutineReplacementDraft{RoutineReplacementDraft{object:object("function"),source_fingerprint:Some("a".repeat(64)),base_revision:String::new(),definition:def.into(),allow_drop_recreate:allow}}
    #[test] fn mysql_requires_explicit_drop_recreate(){let err=compile(&DbKind::of("mysql"),&["CREATE FUNCTION p_total() RETURNS int RETURN 1".into()],&draft("CREATE FUNCTION p_total() RETURNS int RETURN 2",false)).unwrap_err();assert!(err.to_string().contains("DROP+CREATE"));}
    #[test] fn mysql_drop_then_create_steps(){let (steps,strategy)=compile(&DbKind::of("mysql"),&["CREATE FUNCTION p_total() RETURNS int RETURN 1".into()],&draft("CREATE FUNCTION p_total() RETURNS int RETURN 2",true)).unwrap();assert_eq!(strategy,"drop_recreate");assert_eq!(steps.len(),2);assert!(steps[0].sql.starts_with("DROP FUNCTION"));assert!(steps[1].sql.starts_with("CREATE FUNCTION"));}
    #[test] fn postgres_adds_or_replace(){let (steps,strategy)=compile(&DbKind::of("postgresql"),&["CREATE FUNCTION p_total() RETURNS integer AS $$ SELECT 1 $$ LANGUAGE sql".into()],&draft("CREATE FUNCTION p_total() RETURNS integer AS $$ SELECT 2 $$ LANGUAGE sql",false)).unwrap();assert_eq!(strategy,"create_or_replace");assert!(steps[0].sql.contains("CREATE OR REPLACE FUNCTION"));assert!(steps[0].sql.contains("\"public\".\"p_total\""));}
    #[test] fn postgres_rejects_return_type_change(){let err=compile(&DbKind::of("postgresql"),&["CREATE FUNCTION p_total() RETURNS integer AS $$ SELECT 1 $$ LANGUAGE sql".into()],&draft("CREATE FUNCTION p_total() RETURNS text AS $$ SELECT 'x' $$ LANGUAGE sql",false)).unwrap_err();assert!(err.to_string().contains("返回类型变化"));}
    #[test] fn rejects_multiple_routines(){let err=compile(&DbKind::of("postgresql"),&[],&draft("CREATE FUNCTION p_total() RETURNS int AS $$ SELECT 1 $$ LANGUAGE sql; CREATE FUNCTION other() RETURNS int AS $$ SELECT 2 $$ LANGUAGE sql",false)).unwrap_err();assert!(err.to_string().contains("一次只能替换一个例程"));}
    #[test] fn rejects_empty_definition(){assert!(compile(&DbKind::of("postgresql"),&[],&draft("   ",false)).is_err());}
    #[test] fn rejects_unknown_protocol(){assert!(compile(&DbKind::of("mssql"),&[],&draft("CREATE FUNCTION p_total() RETURNS int",false)).is_err());}
    #[test] fn rejects_mismatched_object_name(){let err=compile(&DbKind::of("postgresql"),&[],&draft("CREATE FUNCTION other() RETURNS int AS $$ SELECT 1 $$ LANGUAGE sql",false)).unwrap_err();assert!(err.to_string().contains("不一致"));}
    #[test] fn normalize_ignores_whitespace_and_semicolon(){assert_eq!(normalize("CREATE  FUNCTION f () RETURNS int;"),normalize("create function f() returns int"));}
    #[test] fn revision_changes_with_definition(){assert_ne!(revision_of(&["CREATE FUNCTION f() RETURNS int".into()]),revision_of(&["CREATE FUNCTION f() RETURNS text".into()]));}
    #[test] fn literal_case_and_whitespace_are_not_collisions(){for b in ["'ab'","'A B'","'a  b'"]{assert_ne!(revision_of(&["SELECT 'a b'".into()]),revision_of(&[format!("SELECT {b}")]));}assert_ne!(normalize("AS $$SELECT 'x'$$"),normalize("AS $$SELECT 'X'$$"));}
    #[test] fn dollar_quote_and_comment_do_not_count_body_ddl(){let d=draft("/* CREATE FUNCTION other() */ CREATE FUNCTION public.p_total() RETURNS text AS $body$ BEGIN RETURN 'CREATE FUNCTION other();'; END; $body$ LANGUAGE plpgsql;",false);assert!(compile(&DbKind::of("postgresql"),&[],&d).is_ok());}
    #[test] fn selected_name_in_literal_cannot_change_target(){assert!(compile(&DbKind::of("postgresql"),&[],&draft("CREATE FUNCTION other() RETURNS text AS $$ SELECT 'p_total' $$ LANGUAGE sql",false)).is_err());}
    #[test] fn schema_and_kind_mismatch_are_rejected(){for s in ["CREATE FUNCTION other.p_total() RETURNS int AS $$ SELECT 1 $$ LANGUAGE sql","CREATE PROCEDURE public.p_total() AS $$ SELECT 1 $$ LANGUAGE sql"]{assert!(compile(&DbKind::of("postgresql"),&[],&draft(s,false)).is_err());}}
    #[test] fn signature_change_is_not_replace(){assert!(compile(&DbKind::of("postgresql"),&["CREATE FUNCTION p_total(x integer) RETURNS int AS $$SELECT 1$$ LANGUAGE sql".into()],&draft("CREATE FUNCTION p_total(x text) RETURNS int AS $$SELECT 1$$ LANGUAGE sql",false)).is_err());}
    #[test] fn mysql_single_compound_body_and_trailing_injection(){let mut d=draft("CREATE PROCEDURE p_total() BEGIN SELECT 'END; CREATE FUNCTION x()'; SELECT 2; END;",true);d.object.kind="procedure".into();assert!(compile(&DbKind::of("mysql"),&[],&d).is_ok());d.definition.push_str(" DROP TABLE victim");assert!(compile(&DbKind::of("mysql"),&[],&d).is_err());}
    #[test] fn executable_comment_rejected(){assert!(lex("/*!50003 CREATE FUNCTION f() */").is_err());}
    #[test] fn body_only_preserves_outer_attributes_and_ignores_default_literal(){let old=header("CREATE FUNCTION public.p_total(x text DEFAULT 'Keep literal') RETURNS text AS $old$ SELECT x $old$ LANGUAGE sql SECURITY INVOKER",true).unwrap();let next=header("CREATE OR REPLACE FUNCTION \"public\".\"p_total\"(x text DEFAULT 'Keep literal') RETURNS text AS $new$ SELECT x || 'changed' $new$ LANGUAGE sql SECURITY INVOKER",true).unwrap();assert_eq!(old.tokens[old.body_index.unwrap()].kind,TokenKind::Dollar);assert!(body_only(&old,&next).is_ok());for sql in ["CREATE FUNCTION public.p_total(x text DEFAULT 'Keep literal') RETURNS text AS $$ SELECT x $$ LANGUAGE plpgsql SECURITY INVOKER","CREATE FUNCTION public.p_total(x text DEFAULT 'Keep literal') RETURNS text AS $$ SELECT x $$ LANGUAGE sql SECURITY DEFINER","CREATE FUNCTION public.p_total(x text DEFAULT 'Changed literal') RETURNS text AS $$ SELECT x $$ LANGUAGE sql SECURITY INVOKER"]{assert!(body_only(&old,&header(sql,true).unwrap()).is_err());}}
    #[test] fn body_only_rejects_sql_string_interpretation_changes(){let old=header("CREATE FUNCTION p_total() RETURNS int AS $$ SELECT 1 $$ LANGUAGE sql",true).unwrap();let next=header("CREATE FUNCTION p_total() RETURNS int AS ' SELECT 2 ' LANGUAGE sql",true).unwrap();assert!(body_only(&old,&next).unwrap_err().to_string().contains("ROUTINE_BODY_UNSUPPORTED"));}
    #[test] fn real_definition_parameter_count_keeps_nested_literals(){assert_eq!(definition_parameter_count("CREATE PROCEDURE p_total(IN x DECIMAL(20,8), IN y VARCHAR(30)) BEGIN SELECT 'a,b)'; END",false).unwrap(),2);assert_eq!(definition_parameter_count("CREATE FUNCTION p_total() RETURNS int RETURN 1",false).unwrap(),0);assert_eq!(definition_parameter_count("CREATE FUNCTION p_total(x text DEFAULT 'a,b', y numeric(20,8) DEFAULT 2) RETURNS text AS $$ SELECT x $$ LANGUAGE sql",true).unwrap(),2);}
    #[test] fn source_fingerprint_requires_loaded_proof_and_exact_endpoint(){for source in [None,Some(""),Some("endpoint"),Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")]{assert_eq!(validate_source_fingerprint(source,&"a".repeat(64)).unwrap_err().code,"ROUTINE_SOURCE_REQUIRED");}assert!(validate_source_fingerprint(Some(&"a".repeat(64)),&"a".repeat(64)).is_ok());assert_eq!(validate_source_fingerprint(Some(&"a".repeat(64)),&"b".repeat(64)).unwrap_err().code,"ROUTINE_SOURCE_CHANGED");}
    #[tokio::test] async fn legacy_draft_missing_source_is_rejected_before_snapshot(){let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();let _held=pool.acquire().await.unwrap();let mut json=serde_json::to_value(draft("CREATE FUNCTION p_total() RETURNS int AS $$ SELECT 1 $$ LANGUAGE sql",false)).unwrap();json.as_object_mut().unwrap().remove("source_fingerprint");let old:RoutineReplacementDraft=serde_json::from_value(json).unwrap();assert!(old.source_fingerprint.is_none());let db=Db::Sqlite(pool);let error=tokio::time::timeout(std::time::Duration::from_millis(200),preview(&db,&DbKind::of("sqlite"),&old)).await.unwrap().unwrap_err();assert_eq!(error.downcast_ref::<crate::error::AppError>().unwrap().code,"ROUTINE_SOURCE_REQUIRED");}
    #[tokio::test] async fn legacy_apply_cannot_execute_without_c1(){let pool=sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();let error=apply(&Db::Sqlite(pool),&DbKind::of("sqlite"),&draft("CREATE FUNCTION f()",false)).await.unwrap_err();assert!(error.to_string().contains("CONTROLLED_EXECUTION_REQUIRED"));}
}
