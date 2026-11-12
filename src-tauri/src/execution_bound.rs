//! 新例程功能只是C1的调用者：不签发授权，不另造取消/预算/终态引擎。
//! 调用方必须在commands服务中用真实window+保存配置注册Entry，并移动ordinary lease。
use std::sync::Arc;
use anyhow::{anyhow,Result};
use crate::routine_create::{self, CreationExecution, RoutineCreationDraft, RoutineCreationReceipt};
use sqlx::Connection;
use crate::{db::Db,error::{AppError,CmdResult},execution::{self,Entry},manual_transaction::OrdinaryLease,models::{DbKind,MultiResult},routine_call::{self,CallExecution,RoutineCallDetails},routine_design::{self,RoutineReceipt,RoutineReplacementDraft}};

pub enum RoutineWork {
    Call { plan_id:String },
    Replace { draft:RoutineReplacementDraft },
    Create { draft:RoutineCreationDraft },
}
enum ResolvedWork { Call(CallExecution),Replace(RoutineReplacementDraft),Create(CreationExecution) }
#[derive(Default)] struct Details {call:Option<RoutineCallDetails>,replacement:Option<RoutineReceipt>,creation:Option<RoutineCreationReceipt>}
pub struct BoundResult {
    pub result:CmdResult<MultiResult>,
    pub confirmed:bool,
    pub call:Option<RoutineCallDetails>,
    pub replacement:Option<RoutineReceipt>,
    pub creation:Option<RoutineCreationReceipt>,
}
impl BoundResult {
    fn error(error:AppError,confirmed:bool)->Self {Self{result:Err(error),confirmed,call:None,replacement:None,creation:None}}
}
/// 用于C1请求的已授权工作标识。仅父commands可信服务构造，不接受前端SQL。
pub fn call_request_sql(plan_id:&str)->CmdResult<String>{
    let id=uuid::Uuid::parse_str(plan_id).map_err(|_|AppError::coded("ROUTINE_PLAN_INVALID","计划身份无效"))?;
    if id.to_string()!=plan_id||id.is_nil(){return Err(AppError::coded("ROUTINE_PLAN_INVALID","计划身份必须规范非空UUID"));}
    Ok(format!("ROUTINE_CALL_PLAN {plan_id}"))
}
pub fn replacement_request_sql(draft:&RoutineReplacementDraft)->CmdResult<String>{
    routine_design::require_source_fingerprint(draft.source_fingerprint.as_deref())?;
    if draft.base_revision.is_empty(){return Err(AppError::coded("ROUTINE_BASELINE_REQUIRED","必须保留编辑时基线"));}
    Ok(format!("ROUTINE_REPLACEMENT {}",crate::object_catalog::revision(draft)))
}
/// Entry只由既有C1 Registry创建。本函数从未调用grant/register/finish。
/// commands应验证request.sql为上方对应工作标识、拒绝transaction_owner，再获取ordinary lease。
/// 返回后commands只能调用entry.finish一次；unknown的迟到Details永不发布。
pub async fn run(db:&Db,kind:&DbKind,entry:Arc<Entry>,ordinary:OrdinaryLease,work:RoutineWork,read_only:bool)->BoundResult {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if read_only{return BoundResult::error(AppError::coded("READ_ONLY_ROUTINE","只读连接禁止可能有副作用的例程调用/保存"),false);}
    let resolved=match work {
        RoutineWork::Call{plan_id}=>match routine_call::global().take(&plan_id,&entry.binding){Ok(work)=>ResolvedWork::Call(work),Err(error)=>return BoundResult::error(AppError::from(error),false)},
        RoutineWork::Replace{draft}=>{
            if let Err(error)=routine_design::validate_source_fingerprint(draft.source_fingerprint.as_deref(),&entry.binding.fingerprint){return BoundResult::error(error,false);}
            if draft.object.connection_id!=entry.binding.connection||draft.object.schema!=entry.binding.namespace{return BoundResult::error(AppError::coded("ROUTINE_SCOPE_MISMATCH","授权窗口/连接/命名空间与草稿不匹配"),false);}
            if !matches!(db,Db::Postgres(_)){return BoundResult::error(AppError::coded("ROUTINE_REPLACEMENT_UNSUPPORTED","仅PostgreSQL原生body替换已经接入C1；MySQL恢复证据未完整，未执行DDL"),false);}
            ResolvedWork::Replace(draft)
        },
        RoutineWork::Create{draft}=>{
            if !matches!(db,Db::Postgres(_) | Db::Mysql(_)){return BoundResult::error(AppError::coded("ROUTINE_CREATION_ADAPTER_UNSUPPORTED","当前驱动没有受控例程创建通道，没有发送DDL"),false);}
            match routine_create::global().take(&draft,&entry.binding,kind){Ok(work)=>ResolvedWork::Create(work),Err(error)=>return BoundResult::error(AppError::from(error),false)}
        },
    };
    let owned=match db {Db::Mysql(p)=>Db::Mysql(p.clone()),Db::Postgres(p)=>Db::Postgres(p.clone()),_=>return BoundResult::error(AppError::coded("ROUTINE_ADAPTER_UNSUPPORTED","该驱动没有已接入的受控typed例程执行器"),false)};
    if entry.stopping(){return BoundResult::error(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND","等待期间停止，没有发送SQL"),true);}
    let details=Arc::new(parking_lot::Mutex::new(Details::default()));let output=details.clone();let working=entry.clone();let kind=kind.clone();
    // ordinary 租约不进入被监督的 worker future：PoolConnection 的物理排空 future 与
    // OwnedMutexGuard 组合会触发 rustc「Send 不够通用」的推断缺陷（#102211）。
    // 改为在本函数内持有租约跨越 supervise —— 门禁窗口等价于真实工作期间，
    // supervise 返回即工作已定态，租约随其释放。
    let _ordinary = ordinary;
    let (result,confirmed)=execution::supervise(entry,async move {
        run_pinned(&owned,&kind,&working,resolved,&output).await
    }).await;
    drop(_ordinary);
    let mut details=details.lock();
    if result.is_ok(){BoundResult{result,confirmed,call:details.call.take(),replacement:details.replacement.take(),creation:details.creation.take()}}else{BoundResult{result,confirmed,call:None,replacement:None,creation:None}}

    }).await
}
// 池获取按数据库类型具体化（泛型 PoolConnection<DB> 会触发 rustc「Send 不够通用」
// 的自动 trait 推断缺陷；两个具体实现体完全一致，仅类型不同）。
async fn acquire_pg(pool:&sqlx::PgPool,entry:&Arc<Entry>)->Result<sqlx::pool::PoolConnection<sqlx::Postgres>> {
    tokio::select!{biased;_ =entry.wait_stop()=>Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND","池等待已取消，没有发送SQL").into()),result=pool.acquire()=>result.map_err(Into::into)}
}
async fn acquire_mysql(pool:&sqlx::MySqlPool,entry:&Arc<Entry>)->Result<sqlx::pool::PoolConnection<sqlx::MySql>> {
    tokio::select!{biased;_ =entry.wait_stop()=>Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND","池等待已取消，没有发送SQL").into()),result=pool.acquire()=>result.map_err(Into::into)}
}
fn before_start(entry:&Arc<Entry>)->Result<()> {
    if entry.stopping()||!entry.start(){return Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND","工作发送前停止，没有发送SQL").into());} Ok(())
}
fn confirmed_without_database_error(result:&Result<MultiResult>)->bool {
    result.as_ref().err().and_then(|e|e.downcast_ref::<AppError>()).is_some_and(|e|matches!(e.code.as_str(),"EXECUTION_CANCELLED_BEFORE_SEND"|"ROUTINE_STOPPED_ROLLED_BACK"|"ROUTINE_CREATION_ROLLED_BACK"))
}
/// 例程工作以具名 async fn 传给 drive：内联 async move 块捕获 &mut 连接时
/// 会触发 rustc「Send 不够通用」的自动 trait 推断缺陷（#102211）。
async fn pg_work(conn:&mut sqlx::PgConnection,kind:&DbKind,entry:&Arc<Entry>,work:ResolvedWork,output:&Arc<parking_lot::Mutex<Details>>)->Result<MultiResult>{
    if entry.stopping(){return Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND","读取控制身份后停止，没有发送例程SQL").into());}
    match work {
        ResolvedWork::Call(work)=>{let (result,details)=routine_call::pg_execute(conn,&work,entry).await?;output.lock().call=Some(details);Ok(result)},
        ResolvedWork::Replace(draft)=>{let (result,receipt)=routine_design::pg_apply(conn,kind,&draft,entry).await?;output.lock().replacement=Some(receipt);Ok(result)},
        ResolvedWork::Create(work)=>{let (result,receipt)=routine_create::pg_create(conn,&work,entry).await?;output.lock().creation=Some(receipt);Ok(result)},
    }
}
async fn mysql_work(conn:&mut sqlx::MySqlConnection,pool:&sqlx::MySqlPool,kind:&DbKind,entry:&Arc<Entry>,work:ResolvedWork,output:&Arc<parking_lot::Mutex<Details>>)->Result<MultiResult>{
    if entry.stopping(){return Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND","读取控制身份后停止，没有发送例程SQL").into());}
    match work{ResolvedWork::Call(work)=>{let (result,details)=routine_call::mysql_execute(conn,kind,&work,entry).await?;output.lock().call=Some(details);Ok(result)},ResolvedWork::Replace(_)=>Err(anyhow!("ROUTINE_REPLACEMENT_UNSUPPORTED")),ResolvedWork::Create(work)=>{let (result,receipt)=routine_create::mysql_create(conn,pool,&work,entry).await?;output.lock().creation=Some(receipt);Ok(result)}}
}
async fn run_pinned(db:&Db,kind:&DbKind,entry:&Arc<Entry>,work:ResolvedWork,output:&Arc<parking_lot::Mutex<Details>>)->(CmdResult<MultiResult>,bool){
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let outcome = match db {
        Db::Postgres(pool) => match acquire_pg(pool, entry).await { Ok(mut c) => {
            c.close_on_drop(); let outcome = send_work(Box::pin(pg_run(&mut c, pool, kind, entry, work, output))).await;
            if let Err(e) = c.close().await { return (Err(AppError::coded("EXECUTION_EFFECTS_UNKNOWN", format!("PostgreSQL固定连接未确认销毁：{e}"))), false); }
            outcome
        }, Err(e) => { let unsent = e.downcast_ref::<AppError>().is_some_and(|e| e.code == "EXECUTION_CANCELLED_BEFORE_SEND"); (Err(e), unsent) } },
        Db::Mysql(pool) => match acquire_mysql(pool, entry).await { Ok(mut c) => {
            c.close_on_drop(); let outcome = send_work(Box::pin(mysql_run(&mut c, pool, kind, entry, work, output))).await;
            if let Err(e) = c.close().await { return (Err(AppError::coded("EXECUTION_EFFECTS_UNKNOWN", format!("MySQL固定连接未确认销毁：{e}"))), false); }
            outcome
        }, Err(e) => { let unsent = e.downcast_ref::<AppError>().is_some_and(|e| e.code == "EXECUTION_CANCELLED_BEFORE_SEND"); (Err(e), unsent) } },
        _ => return (Err(AppError::coded("ROUTINE_ADAPTER_UNSUPPORTED", "该驱动没有已接入的受控typed例程执行器")), false),
    };
    let confirmed = outcome.1 || confirmed_without_database_error(&outcome.0);
    (outcome.0.map_err(AppError::from), confirmed)

    }).await
}
async fn pg_run(conn:&mut sqlx::PgConnection,pool:&sqlx::PgPool,kind:&DbKind,entry:&Arc<Entry>,work:ResolvedWork,output:&Arc<parking_lot::Mutex<Details>>)->(Result<MultiResult>,bool){
    if let Err(e)=before_start(entry){return (Err(e),true);}
    let target=match crate::sql_history::query_scalar::<_,i32>("SELECT pg_catalog.pg_backend_pid()").fetch_one(&mut *conn).await{Ok(id)=>id,Err(e)=>return (Err(e.into()),false)};
    let opts=pool.connect_options();
    execution::drive(pg_work(conn,kind,entry,work,output),entry,||async move{
        let mut control=sqlx::PgConnection::connect_with(&opts).await?;
        let stopped=crate::sql_history::query_scalar::<_,bool>("SELECT pg_catalog.pg_cancel_backend($1)").bind(target).fetch_one(&mut control).await;
        let closed=control.close().await;let stopped=stopped?;closed?;if !stopped{return Err(anyhow!("PostgreSQL没有确认定向取消"));}Ok(())
    }).await
}
async fn mysql_run(conn:&mut sqlx::MySqlConnection,pool:&sqlx::MySqlPool,kind:&DbKind,entry:&Arc<Entry>,work:ResolvedWork,output:&Arc<parking_lot::Mutex<Details>>)->(Result<MultiResult>,bool){
    if let Err(e)=before_start(entry){return (Err(e),true);}
    let target=match crate::sql_history::query_scalar::<_,u64>("SELECT CONNECTION_ID()").fetch_one(&mut *conn).await{Ok(id)=>id,Err(e)=>return (Err(e.into()),false)};
    let opts=pool.connect_options();
    execution::drive(mysql_work(conn,pool,kind,entry,work,output),entry,||async move{
        let mut control=sqlx::MySqlConnection::connect_with(&opts).await?;
        let stopped=crate::sql_history::query(&format!("KILL QUERY {target}")).execute(&mut control).await;let closed=control.close().await;stopped?;closed?;Ok(())
    }).await
}



/// 显式 Send 检查点：例程执行 future 在组合进 supervise 前先单独通过 Send 验证，
/// 绕开 rustc #102211「Send 不够通用」的推断缺陷（检查点是中性包装，不改变语义）。
fn send_work<F: std::future::Future + Send>(f: F) -> F { f }
#[cfg(test)]mod tests{
    use super::*;
    use crate::{execution::{Binding,Registry},execution_contracts::{ExecutionRequest,ExecutionState}};
    fn registered(cancel:bool)->(Arc<Entry>,ExecutionRequest){let registry=Registry::default();let binding=Binding{principal:"window".into(),owner:"page".into(),connection:"c".into(),namespace:"public".into(),fingerprint:"a".repeat(64)};let mut req=ExecutionRequest{execution_id:String::new(),connection_id:"c".into(),namespace:"public".into(),owner_id:"page".into(),sql:"work".into(),transaction_owner:None,timeout_ms:Some(1000),origin:Some("routine".into()),execution_grant:None,cancel_before_send:cancel};registry.grant_request(&mut req,binding.clone()).unwrap();let entry=registry.register(&req,binding,1).unwrap();(entry,req)}
    #[test]fn marker_canonical_uuid_required(){assert!(call_request_sql("client-token").is_err());let id=uuid::Uuid::new_v4().to_string();assert_eq!(call_request_sql(&id).unwrap(),format!("ROUTINE_CALL_PLAN {id}"));}
    #[test]fn replacement_marker_binds_body_signature_and_editor_baseline(){let draft=RoutineReplacementDraft{object:crate::routine_design::RoutineObjectId{connection_id:"c".into(),schema:"public".into(),name:"f".into(),kind:"function".into(),identity_arguments:"x text".into()},source_fingerprint:Some("a".repeat(64)),base_revision:"opening-revision".into(),definition:"CREATE FUNCTION f(x text) RETURNS text AS $$ SELECT 'Keep Case' $$ LANGUAGE sql".into(),allow_drop_recreate:false};let marker=replacement_request_sql(&draft).unwrap();assert!(marker.starts_with("ROUTINE_REPLACEMENT "));assert!(!marker.contains("SELECT"));let mut changed=draft.clone();changed.definition=changed.definition.replace("Keep Case","keep case");assert_ne!(marker,replacement_request_sql(&changed).unwrap());let mut changed=draft.clone();changed.base_revision="new-baseline".into();assert_ne!(marker,replacement_request_sql(&changed).unwrap());let mut changed=draft.clone();changed.object.identity_arguments="x integer".into();assert_ne!(marker,replacement_request_sql(&changed).unwrap());changed.base_revision.clear();assert!(replacement_request_sql(&changed).is_err());let mut changed=draft.clone();changed.source_fingerprint=Some("b".repeat(64));assert_ne!(marker,replacement_request_sql(&changed).unwrap());changed.source_fingerprint=None;assert_eq!(replacement_request_sql(&changed).unwrap_err().code,"ROUTINE_SOURCE_REQUIRED");}
    #[test]fn only_c1_can_publish_terminal(){let (entry,_)=registered(true);assert!(before_start(&entry).is_err());assert_eq!(entry.status().state,ExecutionState::CancelRequested);let outcome=entry.finish(Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND","未执行")),true,false);assert_eq!(outcome.state,ExecutionState::Cancelled);assert_eq!(outcome.effects,"no_effect");}
    #[tokio::test]async fn old_or_changed_endpoint_replacement_is_rejected_before_pool_work(){let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();let mut held=pool.acquire().await.unwrap();crate::sql_history::query("CREATE TABLE untouched (id INTEGER)").execute(&mut *held).await.unwrap();let db=Db::Sqlite(pool);let registry=crate::manual_transaction::Registry::default();for (source,code) in [(None,"ROUTINE_SOURCE_REQUIRED"),(Some("b".repeat(64)),"ROUTINE_SOURCE_CHANGED")]{let(entry,_)=registered(false);let draft=RoutineReplacementDraft{object:crate::routine_design::RoutineObjectId{connection_id:"c".into(),schema:"public".into(),name:"f".into(),kind:"function".into(),identity_arguments:String::new()},source_fingerprint:source,base_revision:"same-definition".into(),definition:"CREATE FUNCTION f() RETURNS int AS $$ SELECT 2 $$ LANGUAGE sql".into(),allow_drop_recreate:false};let lease=registry.ordinary(&["c"]).await.unwrap();let result=tokio::time::timeout(std::time::Duration::from_millis(200),run(&db,&DbKind::of("sqlite"),entry.clone(),lease,RoutineWork::Replace{draft},false)).await.unwrap();assert_eq!(result.result.unwrap_err().code,code);assert_eq!(entry.status().state,ExecutionState::Queued);assert!(registry.ordinary(&["c"]).await.is_ok());}assert_eq!(crate::sql_history::query_scalar::<_,i64>("SELECT COUNT(*) FROM sqlite_schema WHERE name='untouched'").fetch_one(&mut *held).await.unwrap(),1);}
    #[tokio::test]async fn readonly_fail_closed_never_acquires_native_pool(){let pool=sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();let (entry,_)=registered(false);let registry=crate::manual_transaction::Registry::default();let lease=registry.ordinary(&["c"]).await.unwrap();let outcome=run(&Db::Sqlite(pool),&DbKind::of("sqlite"),entry.clone(),lease,RoutineWork::Call{plan_id:"not_used".into()},true).await;assert_eq!(outcome.result.unwrap_err().code,"READ_ONLY_ROUTINE");assert_eq!(entry.status().state,ExecutionState::Queued);}
}
