//! SQLite 增量同步：一个源快照游标 -> 认证 spill -> 同物理目标事务内的键预检/CAS/绑定/读回。
//! 不支持仅凭 delete_missing bool 删除；危险删除须由后续 reviewed-run 契约明确授权。
#[path="sync_stream_spill.rs"] mod spill;
use spill::{Cell, Pin, Spill, bind, decode_row};
use super::{SyncOptions, SyncReport, SyncColumnMapping, TaskControl};
use anyhow::{anyhow, bail, Context, Result};
use futures::TryStreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{Column, ConnectOptions, Connection, Row, Sqlite, SqliteConnection, SqlitePool};
use std::collections::HashSet;
use std::sync::Arc;

fn ident(name:&str)->String {crate::db::quote_ident(&crate::models::DbKind::of("sqlite"),name)}
fn table(name:&str)->String {format!("main.{}",ident(name))}
fn check(task:Option<&TaskControl>)->Result<()> {if let Some(task)=task {task.check()?;}Ok(())}
fn progress(task:Option<&TaskControl>,phase:&str,rows:u64,total:u64,label:&str){if let Some(task)=task{task.update(phase,rows,total,label);}}

#[derive(Clone,Debug,Serialize)] struct Col {name:String,ty:String,not_null:bool,default:Option<String>,pk:i64,hidden:i64}
#[derive(Clone,Debug,Serialize)] struct Meta {name:String,kind:String,columns:Vec<Col>,objects:Vec<(String,String,Option<String>)>,strict:bool,schema_cookie:i64,hash:String}
async fn metadata(connection:&mut SqliteConnection,name:&str)->Result<Option<Meta>> {
    let row=crate::sql_history::query("SELECT name,type FROM main.sqlite_schema WHERE name=? COLLATE NOCASE AND type IN ('table','view')").bind(name).fetch_optional(&mut *connection).await?;
    let Some(row)=row else{return Ok(None)};let actual:String=row.try_get("name")?;
    let mut columns=Vec::new();let mut objects=Vec::new();let mut total=0usize;
    let sql=format!("PRAGMA main.table_xinfo({})",ident(&actual));let mut stream=crate::sql_history::query(&sql).fetch(&mut *connection);
    while let Some(row)=stream.try_next().await? {let column=Col{name:row.try_get("name")?,ty:row.try_get("type")?,not_null:row.try_get::<i64,_>("notnull")?!=0,default:row.try_get("dflt_value")?,pk:row.try_get("pk")?,hidden:row.try_get("hidden")?};
        total=total.checked_add(column.name.len()+column.ty.len()+column.default.as_ref().map_or(0,String::len)).ok_or_else(||anyhow!("元数据长度溢出"))?;
        if columns.len()>=4096 || total>spill::RECORD_BYTES {bail!("SQLite元数据超过已验证界限");}columns.push(column);
    }drop(stream);
    let mut stream=crate::sql_history::query("SELECT type,name,sql FROM main.sqlite_schema WHERE tbl_name=? COLLATE NOCASE ORDER BY type,name").bind(&actual).fetch(&mut *connection);
    while let Some(row)=stream.try_next().await? {let sql:Option<String>=row.try_get("sql")?;total=total.checked_add(sql.as_ref().map_or(0,String::len)).ok_or_else(||anyhow!("元数据长度溢出"))?;if objects.len()>=4096 || total>spill::RECORD_BYTES{bail!("SQLite对象元数据超过已验证界限");}objects.push((row.try_get("type")?,row.try_get("name")?,sql));}drop(stream);
    let row=crate::sql_history::query(&format!("PRAGMA main.table_list({})",ident(&actual))).fetch_optional(&mut *connection).await?.ok_or_else(||anyhow!("SQLite表清单与结构不一致"))?;
    if row.try_get::<String,_>("name")?!=actual || row.try_get::<String,_>("schema")?!="main"{bail!("SQLite表清单返回错误对象身份");}
    let kind:String=row.try_get("type")?;let strict:bool=row.try_get::<i64,_>("strict")?!=0;
    let schema_cookie:i64=crate::sql_history::query_scalar("PRAGMA main.schema_version").fetch_one(&mut *connection).await?;
    let mut meta=Meta{name:actual,kind,columns,objects,strict,schema_cookie,hash:String::new()};meta.hash=format!("{:x}",Sha256::digest(serde_json::to_vec(&meta)?));Ok(Some(meta))
}
#[derive(Clone,Debug,PartialEq,Eq)] enum DatabaseId {File(spill::FileId),NamedMemory(String),PoolMemory(usize)}
struct Owned {connection:Option<sqlx::pool::PoolConnection<Sqlite>>,dedicated:Option<SqliteConnection>,pin:Option<Pin>,identity:DatabaseId}
impl Owned {
    async fn open(pool:&SqlitePool)->Result<Self> {
        let mut connection=pool.acquire().await?;
        let filename:String=crate::sql_history::query_scalar("SELECT file FROM pragma_database_list WHERE name='main'").fetch_one(&mut *connection).await?;
        let options=pool.connect_options();
        let (pin,identity)=if filename.is_empty(){
            let filename=options.get_filename().to_string_lossy().into_owned();
            let url=options.as_ref().clone().filename("navkit-sync-options-probe.sqlite").to_url_lossy();let shared=url.query_pairs().any(|(key,value)|key=="cache"&&value=="shared");let memory=url.query_pairs().any(|(key,value)|key=="mode"&&value=="memory");
            let identity=if memory&&shared&&filename.strip_prefix("file:sqlx-in-memory-").is_some_and(|suffix|!suffix.is_empty()&&suffix.bytes().all(|byte|byte.is_ascii_digit())) {DatabaseId::NamedMemory(filename)}else{DatabaseId::PoolMemory(Arc::as_ptr(&options) as usize)};
            (None,identity)
        }else{let pin=Pin::open(std::path::Path::new(&filename),false)?;pin.verify_database(&mut connection).await?;let identity=DatabaseId::File(pin.identity());(Some(pin),identity)};
        Ok(Self{connection:Some(connection),dedicated:None,pin,identity})
    }
    fn connection(&mut self)->Result<&mut SqliteConnection>{if let Some(connection)=self.dedicated.as_mut(){return Ok(connection);}self.connection.as_deref_mut().ok_or_else(||anyhow!("SQLite同步连接已释放"))}
    async fn verify(&mut self)->Result<()> {if let Some(pin)=&self.pin {let connection=if let Some(connection)=self.dedicated.as_mut(){connection}else{self.connection.as_deref_mut().ok_or_else(||anyhow!("SQLite同步连接已释放"))?};pin.verify_database(connection).await?;}Ok(())}
    async fn prepare_writer(&mut self,pool:&SqlitePool)->Result<()>{
        if matches!(self.identity,DatabaseId::PoolMemory(_)){bail!("私有:memory:尚未验证独立目标事务连接，请使用文件库或SQLx shared-memory池");}
        let options=(*pool.connect_options()).clone().create_if_missing(false).row_buffer_size(1).statement_cache_capacity(8);
        self.dedicated=Some(SqliteConnection::connect_with(&options).await?);
        {let mut handle=self.connection()?.lock_handle().await?;unsafe{libsqlite3_sys::sqlite3_limit(handle.as_raw_handle().as_ptr(),libsqlite3_sys::SQLITE_LIMIT_LENGTH,spill::RECORD_BYTES as i32);}}
        self.verify().await
    }
    async fn destroy(&mut self)->Result<()> {
        let dedicated=if let Some(connection)=self.dedicated.take(){connection.close().await}else{Ok(())};
        let pooled=if let Some(connection)=self.connection.take(){connection.close().await}else{Ok(())};dedicated?;pooled?;Ok(())
    }
    async fn finish(&mut self)->Result<()> {if let Some(connection)=self.dedicated.take(){connection.close().await?;}Ok(())}
}
pub(crate) async fn same_sqlite_database(source:&SqlitePool,target:&SqlitePool)->Result<bool>{
    if Arc::ptr_eq(&source.connect_options(),&target.connect_options()){return Ok(true);}
    let source=Owned::open(source).await?;let source_id=source.identity.clone();drop(source);
    let target=Owned::open(target).await?;Ok(source_id==target.identity)
}

// 独立源读取连接设 row_buffer_size(1)，保留原池锚点以防 shared-memory 消失。
// 源角色只读，不能借新连接绕过主门禁执行写 SQL/附加库/加载扩展或文件 I/O 函数。
unsafe extern "C" fn source_authorizer(_: *mut std::ffi::c_void,action:i32,arg1:*const std::ffi::c_char,arg2:*const std::ffi::c_char,database:*const std::ffi::c_char,_origin:*const std::ffi::c_char)->i32 {
    let string=|pointer:*const std::ffi::c_char|if pointer.is_null(){&[][..]}else{unsafe{std::ffi::CStr::from_ptr(pointer)}.to_bytes()};
    match action {
        libsqlite3_sys::SQLITE_SELECT | libsqlite3_sys::SQLITE_RECURSIVE | libsqlite3_sys::SQLITE_TRANSACTION => libsqlite3_sys::SQLITE_OK,
        libsqlite3_sys::SQLITE_READ if database.is_null() || string(database)==b"main" => libsqlite3_sys::SQLITE_OK,
        libsqlite3_sys::SQLITE_PRAGMA if [b"table_xinfo".as_slice(),b"table_list",b"index_list",b"index_xinfo",b"foreign_key_list",b"database_list",b"schema_version"].contains(&string(arg1)) && (string(arg1)!=b"schema_version"||arg2.is_null()) => libsqlite3_sys::SQLITE_OK,
        libsqlite3_sys::SQLITE_FUNCTION => {let name=string(if arg2.is_null(){arg1}else{arg2});if [b"load_extension".as_slice(),b"writefile",b"readfile",b"eval",b"shell",b"fts3_tokenizer"].iter().any(|forbidden|name.eq_ignore_ascii_case(forbidden)){libsqlite3_sys::SQLITE_DENY}else{libsqlite3_sys::SQLITE_OK}},
        _ => libsqlite3_sys::SQLITE_DENY,
    }
}
async fn source_connection(pool:&SqlitePool,anchor:&Owned)->Result<SqliteConnection>{
    let memory=!matches!(anchor.identity,DatabaseId::File(_));
    if memory && matches!(anchor.identity,DatabaseId::PoolMemory(_)){bail!("独立私有:memory:连接尚未验证单行缓冲共享快照；请使用文件库或SQLx shared-memory池");}
    let options=(*pool.connect_options()).clone().create_if_missing(false).read_only(!memory).row_buffer_size(1).statement_cache_capacity(8);
    let mut connection=SqliteConnection::connect_with(&options).await?;
    crate::sql_history::query("PRAGMA query_only=ON").execute(&mut connection).await?;
    {let mut locked=connection.lock_handle().await?;let result=unsafe{libsqlite3_sys::sqlite3_limit(locked.as_raw_handle().as_ptr(),libsqlite3_sys::SQLITE_LIMIT_LENGTH,spill::RECORD_BYTES as i32);libsqlite3_sys::sqlite3_set_authorizer(locked.as_raw_handle().as_ptr(),Some(source_authorizer),std::ptr::null_mut())};if result!=libsqlite3_sys::SQLITE_OK{bail!("无法建立同步源只读角色");}}
    if let Some(pin)=&anchor.pin{pin.verify_database(&mut connection).await?;}Ok(connection)
}
#[derive(Clone)] struct Mapping {source:String,target:String,source_index:usize}
fn mappings(source:&Meta,options:&SyncOptions)->Result<(Vec<Mapping>,Vec<usize>)>{
    let requested=if options.columns.is_empty(){source.columns.iter().filter(|column|column.hidden==0).map(|column|SyncColumnMapping{source:column.name.clone(),target:column.name.clone(),target_type:None}).collect::<Vec<_>>()}else{options.columns.clone()};
    if requested.is_empty() || requested.len()>4096 {bail!("同步映射为空或超过界限");}let mut sources=HashSet::new();let mut targets=HashSet::new();let mut mappings=Vec::new();
    for column in &requested{crate::transfer::validate_names(std::slice::from_ref(&column.source))?;crate::transfer::validate_names(std::slice::from_ref(&column.target))?;if !sources.insert(column.source.clone())||!targets.insert(column.target.to_ascii_lowercase()){bail!("同步映射源/目标列重复");}let source_index=source.columns.iter().position(|source|source.name==column.source&&source.hidden!=1).ok_or_else(||anyhow!("源映射列不存在: {}",column.source))?;mappings.push(Mapping{source:column.source.clone(),target:column.target.clone(),source_index});}
    let names=if options.key_columns.is_empty(){let mut columns=source.columns.iter().filter(|column|column.pk>0).collect::<Vec<_>>();columns.sort_by_key(|column|column.pk);columns.into_iter().map(|column|column.name.clone()).collect::<Vec<_>>()}else{options.key_columns.clone()};
    if names.is_empty()||names.len()>8||names.iter().collect::<HashSet<_>>().len()!=names.len(){bail!("增量同步需要1至8列完整且不重复的业务键或主键");}
    let positions=names.iter().map(|name|mappings.iter().position(|mapping|&mapping.source==name).ok_or_else(||anyhow!("业务键/完整主键必须参与映射: {name}"))).collect::<Result<Vec<_>>>()?;Ok((mappings,positions))
}
fn numeric(ty:&str)->bool{let ty=ty.to_ascii_uppercase();ty.contains("INT")||ty.contains("REAL")||ty.contains("FLOA")||ty.contains("DOUB")||ty.contains("NUMERIC")||ty.contains("DECIMAL")||ty.contains("NUMBER")}
fn affinity(column:&Col,strict:bool)->&'static str {let ty=column.ty.to_ascii_uppercase();if strict&&ty=="ANY"{return "BLOB";}if ty.contains("INT"){"INTEGER"}else if ty.contains("CHAR")||ty.contains("CLOB")||ty.contains("TEXT"){"TEXT"}else if ty.is_empty()||ty.contains("BLOB"){"BLOB"}else if ty.contains("REAL")||ty.contains("FLOA")||ty.contains("DOUB"){"REAL"}else{"NUMERIC"}}
fn equivalent(expected:&Cell,actual:&Cell,column:&Col)->bool {
    if expected==actual{return true;}
    if numeric(&column.ty){if let (Some(expected),Some(actual))=(expected.decimal(),actual.decimal()){return expected==actual;}}
    if affinity(column,false)=="TEXT" {match (expected,actual){(Cell::Integer(expected),Cell::Text(actual))=>return actual.parse::<i64>().ok()==Some(*expected),(Cell::Real(expected),Cell::Text(actual))=>return actual.parse::<f64>().ok().map(f64::to_bits)==Some(*expected),_=>{}}}
    false
}
fn planned_target(source:&Meta,mappings:&[Mapping],options:&SyncOptions)->Result<Meta>{
    let kind=crate::models::DbKind::of("sqlite");let mut columns=Vec::new();
    for mapping in mappings {let original=&source.columns[mapping.source_index];let custom=options.columns.iter().find(|column|column.source==mapping.source).and_then(|column|column.target_type.as_deref());let ty=custom.map(|ty|super::validate_target_type(ty,&kind)).transpose()?.unwrap_or_else(||super::map_type(&original.ty,&kind));columns.push(Col{name:mapping.target.clone(),ty,not_null:original.not_null||original.pk>0,default:None,pk:original.pk,hidden:0});}
    let complete_pk=source.columns.iter().filter(|column|column.pk>0).all(|column|mappings.iter().any(|mapping|mapping.source==column.name));if !complete_pk{for column in &mut columns{column.pk=0;}}
    Ok(Meta{name:options.target_table.clone(),kind:"table".into(),columns,objects:Vec::new(),strict:false,schema_cookie:0,hash:String::new()})
}
fn create_target(meta:&Meta)->String {
    let mut definitions=meta.columns.iter().map(|column|format!("{} {}{}",ident(&column.name),column.ty,if column.not_null{" NOT NULL"}else{""})).collect::<Vec<_>>();let mut pk=meta.columns.iter().filter(|column|column.pk>0).collect::<Vec<_>>();pk.sort_by_key(|column|column.pk);if !pk.is_empty(){definitions.push(format!("PRIMARY KEY ({})",pk.iter().map(|column|ident(&column.name)).collect::<Vec<_>>().join(", ")));}format!("CREATE TABLE {} ({})",table(&meta.name),definitions.join(", "))
}
fn positions(meta:&Meta,mappings:&[Mapping])->Result<Vec<usize>> {mappings.iter().map(|mapping|meta.columns.iter().position(|column|column.name==mapping.target&&column.hidden==0).ok_or_else(||anyhow!("目标映射列不存在或是生成/隐藏列: {}",mapping.target))).collect()}
fn key_values(row:&[Cell],positions:&[usize])->Result<Vec<Cell>>{positions.iter().map(|position|row.get(*position).cloned().ok_or_else(||anyhow!("完整键列值缺失"))).collect()}
async fn lookup(connection:&mut SqliteConnection,meta:&Meta,key_columns:&[usize],values:&[Cell])->Result<Option<Vec<Cell>>>{
    let sql=format!("SELECT {} FROM {} WHERE {} LIMIT 2",meta.columns.iter().map(|column|ident(&column.name)).collect::<Vec<_>>().join(", "),table(&meta.name),key_columns.iter().map(|position|format!("{} COLLATE BINARY IS ?",ident(&meta.columns[*position].name))).collect::<Vec<_>>().join(" AND "));
    let mut stream=bind(crate::sql_history::query(&sql),values).fetch(connection);let mut first=None;let mut error=None;let mut count=0u8;
    while let Some(row)=stream.try_next().await? {count+=1;if count==1 {match decode_row(&row){Ok(row)=>first=Some(row),Err(failure)=>error=Some(failure)}}}
    if let Some(error)=error{return Err(error);}if count>1 {bail!("目标业务键命中多行，目标未提交");}Ok(first)
}
async fn rollback(connection:&mut SqliteConnection,error:anyhow::Error)->anyhow::Error {
    match crate::sql_history::query("ROLLBACK").execute(&mut *connection).await {Ok(_)=>error.context("SQLite目标事务已确认回滚；没有提交本次目标修改"),Err(failure)=>error.context(format!("TRANSFER_EFFECTS_UNKNOWN: SQLite回滚未确认，需要核对目标和连接: {failure}"))}
}

pub(crate) async fn sqlite_incremental(source:&SqlitePool,target:&SqlitePool,options:&SyncOptions,task:Option<&TaskControl>,label:&str)->Result<SyncReport>{
    check(task)?;crate::transfer::validate_where(options.where_clause.as_deref())?;
    crate::transfer::validate_names(std::slice::from_ref(&options.source_table))?;crate::transfer::validate_names(std::slice::from_ref(&options.target_table))?;
    if options.sync_mode!="incremental" || options.write_mode!="append" || !matches!(options.mode.as_str(),"data"|"both"){bail!("SQLite增量流仅接受data/both+incremental+append");}
    if [&options.source_schema,&options.target_schema].iter().any(|namespace|!namespace.is_empty()&&!namespace.eq_ignore_ascii_case("main")){bail!("SQLite增量流仅验证main，不允许temp/附加库错靶");}
    if options.delete_missing {bail!("删除目标多余行需要完整范围/预览/CAS授权；legacy delete_missing=true已在任何DDL前拒绝，未静默忽略删除");}
    let mut baseline=Owned::open(target).await?;
    if let Err(error)=baseline.prepare_writer(target).await{let close=baseline.finish().await;return Err(if let Err(close)=close{error.context(format!("TRANSFER_EFFECTS_UNKNOWN: 目标基线连接初始化后关闭未确认: {close}"))}else{error});}
    crate::sql_history::query("BEGIN").execute(baseline.connection()?).await?;
    let before=metadata(baseline.connection()?,&options.target_table).await;
    let baseline_rollback=crate::sql_history::query("ROLLBACK").execute(baseline.connection()?).await;
    if let Err(error)=baseline_rollback{let _=baseline.destroy().await;return Err(anyhow!(error).context("TRANSFER_EFFECTS_UNKNOWN: 目标结构基线只读事务未确认释放"));}
    baseline.finish().await.context("TRANSFER_EFFECTS_UNKNOWN: 目标基线专用连接关闭未确认")?;
    let target_baseline=before?;let target_id=baseline.identity.clone();let target_pin=baseline.pin.take();drop(baseline);
    if target_baseline.is_none()&&(options.mode=="data"||!options.create_target){bail!("目标表不存在；仅数据模式要求已有表，结构+数据模式需显式启用建表");}
    if target_baseline.as_ref().is_some_and(|meta|meta.kind!="table"){bail!("SQLite增量目标必须是真实表，不接受视图");}
    let anchor=Owned::open(source).await?;let source_id=anchor.identity.clone();
    if source_id==target_id&&options.source_table.eq_ignore_ascii_case(&options.target_table){bail!("源表与目标表是同一物理表/连接别名，未发送同步写入");}
    if let (Some(source),Some(target))=(&anchor.pin,&target_pin){if source.same(target)&&source.path!=target.path{bail!("SQLite硬链接别名的独立journal/WAL尚未验证，拒绝错靶同步");}}
    let mut spill=Spill::new().await?;
    let mut reader=match source_connection(source,&anchor).await{Ok(reader)=>reader,Err(error)=>{let cleanup=spill.close().await;return Err(if let Err(cleanup)=cleanup{error.context(format!("TRANSFER_EFFECTS_UNKNOWN: 源连接初始化失败且spill清理未确认: {cleanup}"))}else{error});}};
    let read_result:Result<(Meta,Vec<Mapping>,Vec<usize>,String)>=async {
        if let Some(task)=task.cloned(){reader.lock_handle().await?.set_progress_handler(1000,move||task.check().is_ok());}
        crate::sql_history::query("BEGIN").execute(&mut reader).await?;
        let source_meta=metadata(&mut reader,&options.source_table).await?.ok_or_else(||anyhow!("源表不存在"))?;
        if source_meta.kind!="table"{bail!("SQLite增量源目前只验证普通表的单语句快照；视图/virtual/shadow未冒充快照");}
        let (mappings,keys)=mappings(&source_meta,options)?;
        let mut sql=format!("SELECT {} FROM {}",mappings.iter().map(|mapping|ident(&mapping.source)).collect::<Vec<_>>().join(", "),table(&source_meta.name));if let Some(clause)=options.where_clause.as_deref().filter(|clause|!clause.trim().is_empty()){sql.push_str(&format!(" WHERE ({clause})"));}
        let mut stream=crate::sql_history::query(&sql).fetch(&mut reader);let mut failure=None;
        while let Some(row)=stream.try_next().await? {
            if failure.is_none(){if let Err(error)=check(task){failure=Some(error);}else{let result:Result<()>=async{if row.len()!=mappings.len()||row.columns().iter().zip(&mappings).any(|(column,mapping)|column.name()!=mapping.source){bail!("源游标列身份与快照映射不一致");}let cells=decode_row(&row)?;let key_cells=key_values(&cells,&keys)?;spill.push_source(&cells,&key_cells).await?;Ok(())}.await;if let Err(error)=result{failure=Some(error);}}}
            // 即使取消/坏记录也保留并排空已经发送的同一SELECT，父监督负责有界Unknown。
            progress(task,"read",spill.rows,0,&format!("{label} · 同一源快照已处理{}行，目标尚未修改",spill.rows));tokio::task::yield_now().await;
        }drop(stream);if let Some(error)=failure{return Err(error);}check(task)?;
        if metadata(&mut reader,&options.source_table).await?.as_ref().map(|meta|&meta.hash)!=Some(&source_meta.hash){bail!("源定义快照发生变化");}
        if let Some(pin)=&anchor.pin{pin.verify_database(&mut reader).await?;}
        let digest=spill.seal().await?;Ok((source_meta,mappings,keys,digest))
    }.await;
    let clear=async{reader.lock_handle().await?.remove_progress_handler();Ok::<_,sqlx::Error>(())}.await;
    let source_rollback=crate::sql_history::query("ROLLBACK").execute(&mut reader).await;let source_close=reader.close().await;drop(anchor);
    let source_result=match (read_result,clear,source_rollback,source_close){(Ok(value),Ok(()),Ok(_),Ok(()))=>Ok(value),(result,clear,rollback,close)=>{let error=result.err().unwrap_or_else(||anyhow!("源快照未确认排空"));if clear.is_err()||rollback.is_err()||close.is_err(){Err(error.context("TRANSFER_EFFECTS_UNKNOWN: 源快照释放/读取连接关闭未确认，目标未提交"))}else{Err(error.context("源快照已确认释放，目标尚未修改"))}}};
    let target_result:Result<SyncReport>=async {
    let (source_meta,mappings,keys,digest)=source_result?;
    check(task).map_err(|error|error.context("源完整快照已排空，取消发生在目标事务前"))?;
    let mut writer=Owned::open(target).await?;if writer.identity!=target_id {bail!("目标物理身份与源扫描前基线不同，未发送目标DDL");}
    let write_result:Result<SyncReport>=async {
        writer.prepare_writer(target).await?;writer.verify().await?;if let Some(pin)=&target_pin{pin.check()?;}
        crate::sql_history::query("BEGIN IMMEDIATE").execute(writer.connection()?).await?;
        let result:Result<SyncReport>=async {
            let current=metadata(writer.connection()?,&options.target_table).await?;
            if current.as_ref().map(|meta|&meta.hash)!=target_baseline.as_ref().map(|meta|&meta.hash){bail!("目标结构CAS冲突，没有用新的读取替换旧基线");}
            let planned=if let Some(meta)=current{meta}else{planned_target(&source_meta,&mappings,options)?};let mapped=positions(&planned,&mappings)?;let target_keys=keys.iter().map(|position|mapped[*position]).collect::<Vec<_>>();
            spill.configure_probe(&mapped.iter().map(|position|affinity(&planned.columns[*position],planned.strict)).collect::<Vec<_>>()).await?;
            // 全源键及所有转换先完成认证/无损预检；尚未CREATE/INSERT/UPDATE目标。
            for sequence in 0..spill.rows {check(task)?;let (cells,_,_,_)=spill.source(sequence).await?;let converted=spill.probe(&cells).await?;for (index,position) in mapped.iter().enumerate(){if !equivalent(&cells[index],&converted[index],&planned.columns[*position]){bail!("目标列{}不能无损存储源值，未发送目标DDL/DML",planned.columns[*position].name);}}spill.push_mapped_key(sequence,&key_values(&converted,&keys)?).await?;}
            spill.verify_mapped_keys().await?;
            if target_baseline.is_some(){
                let sql=format!("SELECT {} FROM {}",target_keys.iter().map(|position|ident(&planned.columns[*position].name)).collect::<Vec<_>>().join(", "),table(&planned.name));let mut stream=crate::sql_history::query(&sql).fetch(writer.connection()?);let mut failure=None;
                while let Some(row)=stream.try_next().await?{if failure.is_none(){let result:Result<()>=async{check(task)?;spill.push_target_key(&decode_row(&row)?).await?;Ok(())}.await;if let Err(error)=result{failure=Some(error);}}}drop(stream);if let Some(error)=failure{return Err(error);}spill.verify_target_keys().await?;
            }
            check(task)?;let created=target_baseline.is_none();if created {crate::sql_history::query(&create_target(&planned)).execute(writer.connection()?).await?;}
            let actual=metadata(writer.connection()?,&options.target_table).await?.ok_or_else(||anyhow!("目标创建/定义读回失败"))?;let mapped=positions(&actual,&mappings)?;let target_keys=keys.iter().map(|position|mapped[*position]).collect::<Vec<_>>();let mut affected=0u64;
            for sequence in 0..spill.rows {
                check(task)?;let (cells,_,_,_)=spill.source(sequence).await?;let values=spill.probe(&cells).await?;let key_cells=key_values(&values,&keys)?;
                let original=lookup(writer.connection()?,&actual,&target_keys,&key_cells).await?;
                let should_update=original.as_ref().is_some_and(|row|options.incremental_update&&mapped.iter().enumerate().any(|(index,position)|!equivalent(&values[index],&row[*position],&actual.columns[*position])));
                if original.is_some()&&!should_update{continue;}
                let (sql,parameters)=if let Some(row)=&original{
                    let changes=mapped.iter().enumerate().filter(|(index,_)|!keys.contains(index)).map(|(index,position)|(index,*position)).collect::<Vec<_>>();if changes.is_empty(){continue;}
                    let sql=format!("UPDATE {} SET {} WHERE {}",table(&actual.name),changes.iter().map(|(_,position)|format!("{} = ?",ident(&actual.columns[*position].name))).collect::<Vec<_>>().join(", "),actual.columns.iter().map(|column|format!("{} COLLATE BINARY IS ?",ident(&column.name))).collect::<Vec<_>>().join(" AND "));
                    let mut parameters=changes.iter().map(|(index,_)|values[*index].clone()).collect::<Vec<_>>();parameters.extend(row.iter().cloned());(sql,parameters)
                }else{(format!("INSERT INTO {} ({}) VALUES ({})",table(&actual.name),mapped.iter().map(|position|ident(&actual.columns[*position].name)).collect::<Vec<_>>().join(", "),vec!["?";mapped.len()].join(", ")),values.clone())};
                let count=bind(crate::sql_history::query(&sql),&parameters).execute(writer.connection()?).await?.rows_affected();if count!=1{bail!("SQLite同步影响行数为{count}，不是唯一命中1行，已停止提交");}
                let readback=lookup(writer.connection()?,&actual,&target_keys,&key_cells).await?.ok_or_else(||anyhow!("写入后的唯一键读回缺失"))?;
                for (index,position) in mapped.iter().enumerate(){if !equivalent(&values[index],&readback[*position],&actual.columns[*position]){bail!("写后读回与目标typed值不符: {}",actual.columns[*position].name);}}
                if let Some(original)=original {for (index,cell) in original.iter().enumerate(){if !mapped.contains(&index)&&cell!=&readback[index]{bail!("触发器改变未映射字段，读回验证失败");}}}
                spill.push_verified(&key_cells,&readback).await?;
                affected=affected.checked_add(count).ok_or_else(||anyhow!("同步影响行数溢出"))?;progress(task,"write",affected,spill.rows,&format!("{label} · 目标事务尚未提交"));tokio::task::yield_now().await;
            }
            // 后续行的触发器可能再改先前已读回的行；在提交前复核全部认证回执。
            spill.verify_receipts().await?;
            for sequence in 0..spill.verified_rows{check(task)?;let (keys,expected)=spill.verified(sequence).await?;let actual=lookup(writer.connection()?,&actual,&target_keys,&keys).await?.ok_or_else(||anyhow!("目标最终读回缺少已写行"))?;if actual!=expected{bail!("目标最终读回已被后续语句/触发器修改，拒绝提交");}}
            spill.verify_source().await?;writer.verify().await?;check(task)?;
            Ok(SyncReport{rows:affected,created,warnings:vec![format!("源定义快照SHA256: {}",source_meta.hash),format!("源typed记录快照SHA256: {digest}"),"SQLite业务键/原值校验、绑定写入和读回在同一目标事务；删除未启用".into(),if created{"已物化字段/主键；自增、外键、CHECK、触发器和索引未跨表复制".into()}else{"保留未映射列及现有结构".into()}]})
        }.await;
        match result{Ok(mut report)=>{match crate::sql_history::query("COMMIT").execute(writer.connection()?).await{Ok(_)=>{if let Err(error)=writer.verify().await{return Err(error.context("TRANSFER_EFFECTS_UNKNOWN: 提交已确认但目标物理身份复核失败"));}if task.is_some_and(|task|task.check().is_err()){report.warnings.push("提交已确认；提交期间收到的停止请求不能撤销已提交数据".into());}Ok(report)},Err(error)=>{let _=crate::sql_history::query("ROLLBACK").execute(writer.connection()?).await;let _=writer.destroy().await;Err(anyhow!(error).context("TRANSFER_EFFECTS_UNKNOWN: SQLite提交未确认；未把后续ROLLBACK当作没有生效的证明"))}}},Err(error)=>{let error=rollback(writer.connection()?,error).await;if error.to_string().contains("TRANSFER_EFFECTS_UNKNOWN"){let _=writer.destroy().await;}Err(error)}}
    }.await;
    let close=writer.finish().await;
    match (write_result,close){(Ok(report),Ok(()))=>Ok(report),(Err(error),Ok(()))=>Err(error),(result,Err(error))=>Err(result.err().unwrap_or_else(||anyhow!("目标提交已确认")).context(format!("TRANSFER_EFFECTS_UNKNOWN: 目标专用连接关闭未确认: {error}")))}
    }.await;
    let cleanup=spill.close().await;
    match (target_result,cleanup){(Ok(report),Ok(()))=>Ok(report),(result,Err(cleanup))=>Err(result.err().unwrap_or_else(||anyhow!("目标提交已确认但临时资源未清理")).context(format!("TRANSFER_EFFECTS_UNKNOWN: 同步spill关闭/清理未确认: {cleanup}"))),(Err(error),Ok(()))=>Err(error)}
}

#[cfg(test)] #[path="sync_stream_tests.rs"] mod tests;
#[cfg(test)] #[path="sync_stream_performance_tests.rs"] mod performance_tests;
