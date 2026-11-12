//! T6/T7/D2 typed 批任务：真实 grant、独占物理会话、后台生命周期和有界审计。
//! 生成行只执行一次参数绑定事务；文件不预读正文、不将文件扮成 dummy SQL。
use std::{collections::{HashMap, VecDeque}, sync::{Arc, OnceLock, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use anyhow::{anyhow, bail, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Connection, Executor, Row};
use tauri::{Manager, State, WebviewWindow};
use crate::{batch_sql_reader::{Dialect, Reader, SourceFile, Statement}, commands::{AppState, execution_binding, saved_config}, data_generator::{self, GeneratedRowsRequest}, db::{self, Db}, error::{AppError, CmdResult}, execution::{self, Entry}, execution_contracts::{ExecutionOutcome, ExecutionStatus}, models::{ColumnInfo, ConnectionConfig, DbKind, MultiResult, TableSchema}, transfer_stream::DecodeReader};

const LOG_LINES: usize = 256;
const LOG_BYTES: usize = 256 * 1024;
const RECORD_LIMIT: usize = 2048;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BatchTaskKind { GeneratedRows, SqlFile }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BatchTask {
    GeneratedRows { request: GeneratedRowsRequest },
    SqlFile { path: String, #[serde(default = "utf8")] encoding: String, stop_on_error: bool },
}
fn utf8() -> String { "utf-8".into() }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchExecutionRequest { pub connection_id:String, #[serde(default)] pub namespace:String, pub owner_id:String, #[serde(default)] pub timeout_ms:Option<u64>, pub task:BatchTask }
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BatchContext { pub execution_id:String, pub connection_id:String, pub namespace:String, pub owner_id:String }
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BatchExecutionTicket { #[serde(flatten)] pub context:BatchContext, pub task_kind:BatchTaskKind, pub execution_grant:String, pub expires_in_ms:u64, pub summary:String, pub total_rows:Option<u64>, pub source_bytes:Option<u64> }
#[derive(Debug, Clone, Serialize)]
pub struct BatchMetrics { pub scanned:u64, pub executed:u64, pub errors:u64, pub elapsed_ms:u64 }
#[derive(Debug, Clone, Serialize)]
pub struct BatchLog { pub sequence:u64, pub level:String, pub phase:String, pub statement:Option<u64>, pub row:Option<u64>, pub message:String, pub sql_template:Option<String>, pub ts_ms:u64, pub elapsed_ms:Option<u64>, pub affected_rows:Option<u64>, pub error_code:Option<String> }
#[derive(Debug, Clone, Serialize)]
pub struct BatchExecutionSnapshot { pub context:BatchContext, pub kind:BatchTaskKind, pub status:ExecutionStatus, pub metrics:BatchMetrics, pub scan_complete:bool, pub parse_error:Option<String>, pub affected_rows:u64, pub total_rows:Option<u64>, pub transaction_state:String, pub logs:Vec<BatchLog>, pub next_sequence:u64, pub dropped_before_sequence:u64, pub outcome:Option<ExecutionOutcome> }
enum Payload { Generated(GeneratedRowsRequest), File { source:SourceFile, encoding:String, stop_on_error:bool } }
struct Progress { scanned:u64, executed:u64, errors:u64, scan_complete:bool, parse_error:Option<String>, affected:u64, transaction:String, logs:VecDeque<BatchLog>, bytes:usize, next:u64, dropped:u64, outcome:Option<ExecutionOutcome> }
struct BatchRecord { ticket:BatchExecutionTicket, entry:Arc<Entry>, payload:Mutex<Option<Payload>>, ordinary:Mutex<Option<Box<dyn Send>>>, started:AtomicBool, wake:tokio::sync::Notify, registered:Instant, progress:Mutex<Progress> }
static RECORDS:OnceLock<Mutex<HashMap<String,Arc<BatchRecord>>>>=OnceLock::new();
fn records()->&'static Mutex<HashMap<String,Arc<BatchRecord>>> { RECORDS.get_or_init(||Mutex::new(HashMap::new())) }
fn clipped(text:&str, limit:usize)->String { text.chars().take(limit).collect() }
fn now_ms()->u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0,|d|d.as_millis() as u64) }
/// 原生错误码作为独立字段；原始错误文本另行保留在 message。
fn db_error_code(result:&std::result::Result<u64,sqlx::Error>)->Option<String> {
    let code=result.as_ref().err()?.as_database_error()?.code()?.into_owned();
    let digits:String=code.chars().filter(|c|c.is_ascii_alphanumeric()).take(16).collect();
    if digits.is_empty() { None } else { Some(digits) }
}
impl BatchRecord {
    fn log(&self, level:&str, phase:&str, statement:Option<u64>, row:Option<u64>, message:&str, sql:Option<&str>) {
        self.log_detail(level,phase,statement,row,message,sql,None,None,None);
    }
    /// 带单条执行耗时/影响行/错误码的日志；非执行回执传 None。
    fn log_detail(&self, level:&str, phase:&str, statement:Option<u64>, row:Option<u64>, message:&str, sql:Option<&str>, elapsed_ms:Option<u64>, affected_rows:Option<u64>, error_code:Option<String>) {
        let message=clipped(message,4096); let sql_template=sql.map(crate::batch_sql_reader::bounded_template);
        self.push_log(BatchLog{sequence:0,level:level.into(),phase:phase.into(),statement,row,message,sql_template,ts_ms:now_ms(),elapsed_ms,affected_rows,error_code});
    }
    fn push_log(&self, log:BatchLog) {
        let size=log.message.len()+log.sql_template.as_ref().map_or(0,String::len)+log.error_code.as_ref().map_or(0,String::len)+128;
        let mut p=self.progress.lock(); p.next+=1; let sequence=p.next;
        p.logs.push_back(BatchLog{sequence,..log}); p.bytes+=size;
        while p.logs.len()>LOG_LINES||p.bytes>LOG_BYTES { if let Some(old)=p.logs.pop_front() { p.bytes=p.bytes.saturating_sub(old.message.len()+old.sql_template.as_ref().map_or(0,String::len)+old.error_code.as_ref().map_or(0,String::len)+128); p.dropped=old.sequence; } }
    }
    fn snapshot(&self, after:u64)->BatchExecutionSnapshot {
        let status=self.entry.status(); let p=self.progress.lock();
        BatchExecutionSnapshot{context:self.ticket.context.clone(),kind:self.ticket.task_kind.clone(),metrics:BatchMetrics{scanned:p.scanned,executed:p.executed,errors:p.errors,elapsed_ms:status.deadline.elapsed_ms},scan_complete:p.scan_complete,parse_error:p.parse_error.clone(),affected_rows:p.affected,total_rows:self.ticket.total_rows,transaction_state:p.transaction.clone(),logs:p.logs.iter().filter(|l|l.sequence>after).cloned().collect(),next_sequence:p.next,dropped_before_sequence:p.dropped,status,outcome:p.outcome.clone()}
    }
    fn transaction(&self, state:&str) { self.progress.lock().transaction=state.into(); if matches!(state,"committed"|"rolled_back"|"none"|"unknown") { self.entry.record_remote_transaction(state); } }
    fn result(&self)->MultiResult { let p=self.progress.lock(); MultiResult{sets:vec![],messages:vec![format!("已扫描 {}，执行回执 {}，错误 {}，影响行 {}",p.scanned,p.executed,p.errors,p.affected)],elapsed_ms:self.entry.status().deadline.elapsed_ms,spool_id:None} }
    fn publish(&self, result:CmdResult<MultiResult>, confirmed:bool) {
        let generated=self.ticket.task_kind==BatchTaskKind::GeneratedRows;
        let outcome=self.entry.finish(result,confirmed,generated);
        // 最终 Unknown 只能保留/排空，不接受迟到成功覆盖。事务状态来自真实 COMMIT/ROLLBACK 回执。
        if outcome.transaction_state=="unknown" { self.progress.lock().transaction="unknown".into(); }
        let message=match outcome.state { crate::execution_contracts::ExecutionState::Succeeded=>"任务完成",crate::execution_contracts::ExecutionState::Cancelled=>"停止已确认；请查看真实事务/已有写入状态",crate::execution_contracts::ExecutionState::TimedOut=>"执行时限已到且停止已确认",crate::execution_contracts::ExecutionState::Unknown=>"写入/排空结果未知，禁止重放；资源仍由后台任务持有",_=>"任务失败" };
        let message=outcome.error.as_ref().map_or_else(||message.to_owned(),|error|format!("{message}；原因：{}",error.message));
        self.log_detail(if outcome.error.is_some(){"error"}else{"success"},"finish",None,None,&message,None,Some(outcome.deadline.elapsed_ms),None,outcome.error.as_ref().map(|error|error.code.clone()));
        self.progress.lock().outcome=Some(outcome);
    }
}
fn require_request(request:&BatchExecutionRequest,cfg:&ConnectionConfig)->CmdResult<()> {
    if request.owner_id.trim().is_empty()||request.owner_id.len()>256||request.namespace.len()>1024||request.namespace.contains('\0') { return Err(AppError::coded("EXECUTION_CONTEXT_INVALID","批任务 owner/命名空间无效")); }
    if cfg.read_only { return Err(AppError::coded("READ_ONLY_VIOLATION","批写入/SQL 文件执行必须使用已确认的可写连接")); }
    Dialect::from_kind(&cfg.kind).map_err(AppError::from)?;
    if request.timeout_ms.is_some_and(|ms|ms>86400000) { return Err(AppError::coded("EXECUTION_BUDGET_INVALID","执行预算超出范围")); }
    if cfg.kind.is_sqlite()&&!matches!(request.namespace.as_str(),""|"main") { return Err(AppError::coded("SQLITE_NAMESPACE_UNSUPPORTED","受控批任务只允许固定 SQLite main 数据库")); }
    match &request.task {
        BatchTask::GeneratedRows{request:rows}=>{ data_generator::validate_shape(rows).map_err(AppError::from)?; if rows.connection_id!=request.connection_id||rows.schema!=request.namespace||rows.table.len()>1024||rows.columns.iter().any(|c|c.len()>1024) { return Err(AppError::coded("BATCH_PAYLOAD_SCOPE_MISMATCH","生成批次连接/表/字段与授权目标不一致或超出上限")); } }
        BatchTask::SqlFile{path,encoding,..}=>{ if path.is_empty()||path.len()>32768||!matches!(encoding.as_str(),"utf-8"|"gbk"|"gb18030"|"big5"|"latin1") { return Err(AppError::coded("BATCH_SOURCE_INVALID","SQL 文件路径/编码无效")); } }
    } Ok(())
}
fn scope(request:&BatchExecutionRequest,payload:&Payload)->CmdResult<String> {
    let detail=match payload { Payload::Generated(rows)=>serde_json::to_value(rows)?,Payload::File{source,encoding,stop_on_error}=>serde_json::json!({"source":source.identity(),"encoding":encoding,"stop_on_error":stop_on_error}) };
    let bytes=serde_json::to_vec(&serde_json::json!({"task_kind":match payload{Payload::Generated(_)=>"generated_rows",Payload::File{..}=>"sql_file"},"connection_id":request.connection_id,"namespace":request.namespace,"owner":request.owner_id,"timeout_ms":request.timeout_ms,"policy":"native-pinned/v2;generated-one-bound-transaction;file-exact-dump-guards/v1;no-replay;logs-lexical/v1","payload":detail}))?;
    Ok(format!("typed/v1:batch:{:x}",Sha256::digest(bytes)))
}
fn authorized(state:&AppState,principal:&str,context:&BatchContext)->CmdResult<Arc<BatchRecord>> {
    let cfg=saved_config(state,&context.connection_id)?;
    let binding=execution_binding(state,&cfg,principal,&context.owner_id,&context.namespace)?;
    let entry=state.executions.authorized(&context.execution_id,&binding)?;
    let record=records().lock().get(&context.execution_id).cloned().ok_or_else(||AppError::coded("BATCH_NOT_FOUND","批任务不存在或记录已回收"))?;
    if !Arc::ptr_eq(&entry,&record.entry)||record.ticket.context!=*context { return Err(AppError::coded("EXECUTION_CONTEXT_MISMATCH","批任务上下文不匹配")); } Ok(record)
}
#[tauri::command]
pub async fn register_batch_execution(window:WebviewWindow,state:State<'_,AppState>,request:BatchExecutionRequest)->CmdResult<BatchExecutionTicket> {
    let activity=state.activity.enter()?; let cfg=saved_config(&state,&request.connection_id)?; require_request(&request,&cfg)?;
    let prepared=crate::db::runtime::prepare_at(&cfg,state.store.data_root())?;
    if prepared.http.is_some()||cfg.params.starts_with(crate::driver_manager::PARAMS_PREFIX) { return Err(AppError::coded("BATCH_NATIVE_REQUIRED","批任务要求具备真实取消及排空能力的原生物理会话")); }
    let binding=execution_binding(&state,&cfg,window.label(),&request.owner_id,&request.namespace)?;
    let (payload,kind,total_rows,source_bytes,summary)=match &request.task {
        BatchTask::GeneratedRows{request:rows}=>(Payload::Generated(rows.clone()),BatchTaskKind::GeneratedRows,Some(rows.rows.len() as u64),None,format!("参数绑定生成 {} 行 / {} 字段到 {}",rows.rows.len(),rows.columns.len(),rows.table)),
        BatchTask::SqlFile{path,encoding,stop_on_error}=>{ let path=path.clone(); let source=tokio::task::spawn_blocking(move||SourceFile::open(&path)).await.map_err(|_|AppError::coded("BATCH_SOURCE_INVALID","SQL 源句柄固定任务异常"))??; let bytes=source.identity().bytes; (Payload::File{source,encoding:encoding.clone(),stop_on_error:*stop_on_error},BatchTaskKind::SqlFile,None,Some(bytes),format!("流式执行 SQL 文件（{} 字节，{}）",bytes,encoding)) }
    };
    let grant=state.executions.grant_typed_scope(binding.clone(),&scope(&request,&payload)?)?;
    let entry=state.query_results.register_execution(&binding,&grant.execution_id,||state.executions.register_typed_scope(&grant.execution_id,&grant.token,binding.clone(),&scope(&request,&payload)?,request.timeout_ms,state.store.settings().query_timeout_secs))?;
    entry.own_activity(activity);
    let ticket=BatchExecutionTicket{context:BatchContext{execution_id:grant.execution_id,connection_id:request.connection_id,namespace:request.namespace,owner_id:request.owner_id},task_kind:kind,execution_grant:grant.token,expires_in_ms:grant.expires_in_ms,summary,total_rows,source_bytes};
    let record=Arc::new(BatchRecord{ticket:ticket.clone(),entry,payload:Mutex::new(Some(payload)),ordinary:Mutex::new(None),started:AtomicBool::new(false),wake:tokio::sync::Notify::new(),registered:Instant::now(),progress:Mutex::new(Progress{scanned:0,executed:0,errors:0,scan_complete:false,parse_error:None,affected:0,transaction:"none".into(),logs:VecDeque::new(),bytes:0,next:0,dropped:0,outcome:None})});
    { let mut map=records().lock(); map.retain(|_,r| { let s=r.entry.status(); !s.state.terminal()||s.resources_pending||r.registered.elapsed()<Duration::from_secs(300) }); if map.len()>=RECORD_LIMIT { let oldest=map.iter().filter(|(_,r)| { let s=r.entry.status(); s.state.terminal()&&!s.resources_pending }).min_by_key(|(_,r)|r.registered).map(|(id,_)|id.clone()); if let Some(id)=oldest { map.remove(&id); } } map.insert(ticket.context.execution_id.clone(),record.clone()); }
    record.log("info","queued",None,None,"已注册冻结的 typed 批次；尚未提交数据库 SQL",None);
    tauri::async_runtime::spawn(queued_watchdog(record.clone())); Ok(ticket)
}
#[tauri::command]
pub async fn run_batch_execution(window:WebviewWindow,state:State<'_,AppState>,ticket:BatchExecutionTicket)->CmdResult<BatchExecutionSnapshot> {
    let record=authorized(&state,window.label(),&ticket.context)?;
    if ticket!=record.ticket { return Err(AppError::coded("EXECUTION_GRANT_INVALID","批任务票据被改写")); }
    let payload=claim_payload(&record)?;
    let app=window.app_handle().clone(); let running=record.clone();
    tauri::async_runtime::spawn(async move {
        running.entry.start(); let entry=running.entry.clone();
        let (result,confirmed)=execution::supervise(entry,run_owned(app,running.clone(),payload)).await;
        running.publish(result,confirmed);
    }); Ok(record.snapshot(0))
}
#[tauri::command]
pub async fn batch_execution_status(window:WebviewWindow,state:State<'_,AppState>,context:BatchContext,after_sequence:Option<u64>)->CmdResult<BatchExecutionSnapshot> { Ok(authorized(&state,window.label(),&context)?.snapshot(after_sequence.unwrap_or(0))) }
// 注册与测试共用拥有任务；CAS 决定唯一发布者。
async fn queued_watchdog(record:Arc<BatchRecord>) {
    let remaining=Duration::from_millis(record.ticket.expires_in_ms).saturating_sub(record.registered.elapsed());
    tokio::select! { _=record.wake.notified()=>{},_=record.entry.wait_stop()=>{},_=tokio::time::sleep(remaining)=>{} }
    if record.started.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_ok() {
        record.payload.lock().take();record.entry.work_settled();record.transaction("none");
        record.publish(Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND","注册已停止或过期；没有发送数据库 SQL")),true);
    }
}
fn claim_payload(record:&BatchRecord)->CmdResult<Payload> {
    if record.registered.elapsed()>=Duration::from_millis(record.ticket.expires_in_ms) { return Err(AppError::coded("EXECUTION_GRANT_EXPIRED","批任务注册已过期")); }
    if record.started.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err() { return Err(AppError::coded("EXECUTION_GRANT_REPLAYED","批任务只能运行一次")); }
    record.wake.notify_one();
    record.payload.lock().take().ok_or_else(||AppError::coded("BATCH_PAYLOAD_MISSING","冻结批次已释放"))
}
fn stop_error()->anyhow::Error { AppError::coded("BATCH_STOPPED","批任务已停止；不再提交下一条 SQL").into() }
// 在组合进拥有任务前单独校验 Send，避免 rustc #102211 的高阶生命周期推断问题。
fn send_work<F: std::future::Future + Send>(future:F)->F { future }
fn safe_error(error:&anyhow::Error)->AppError {
    // 仅分类，保留错误原文以及 anyhow 的完整原因链。
    let message=format!("{error:#}");
    if let Some(app)=error.downcast_ref::<AppError>() { return AppError { message, ..app.clone() }; }
    if let Some(sql)=error.downcast_ref::<sqlx::Error>() {
        if sql.as_database_error().is_none() { return AppError::coded("EXECUTION_EFFECTS_UNKNOWN",format!("数据库完整回执缺失；不能确认写入/排空结果，禁止重放；原因：{message}")); }
        return AppError::coded("BATCH_SQL_FAILED",message);
    }
    AppError::coded("BATCH_FAILED",message)
}
// Erase runtime admission's borrowing opaque future before it enters C1's owned task.
// These inputs are owned; the actual managed State guard survives every admission await.
fn batch_connection(app:tauri::AppHandle,id:String,namespace:String,pg:bool)->futures::future::BoxFuture<'static,CmdResult<Arc<Db>>> {
    Box::pin(async move {
        let state=app.state::<AppState>();
        let admitted:futures::future::BoxFuture<'_,CmdResult<Arc<Db>>>=if pg {
            let derived:futures::future::BoxFuture<'_,CmdResult<(String,Arc<Db>)>>=Box::pin(crate::runtime_commands::derived(&state,&id,&namespace,true));
            Box::pin(async move { Ok(derived.await?.1) })
        } else { Box::pin(crate::commands::scoped_db(&state,&id,&namespace)) };
        let result=admitted.await;
        result
    })
}
fn run_owned(app:tauri::AppHandle,record:Arc<BatchRecord>,payload:Payload)->futures::future::BoxFuture<'static,(CmdResult<MultiResult>,bool)> {
    Box::pin(async move {
    let worker=record.clone();
    let result=async move {
        let record=worker;
        let state=app.state::<AppState>(); let context=record.ticket.context.clone();
        if record.entry.stopping() { record.transaction("none"); return Ok((Err(stop_error()),true)); }
        let cfg=saved_config(&state,&context.connection_id)?;
        let binding=execution_binding(&state,&cfg,&record.entry.binding.principal,&context.owner_id,&context.namespace)?;
        if binding!=record.entry.binding||cfg.read_only { return Err(AppError::coded("EXECUTION_CONTEXT_MISMATCH","连接真实目标/只读策略在运行前变化")); }
        let ids=[context.connection_id.as_str()];
        let ordinary=tokio::select! { biased; _=record.entry.wait_stop()=>{ record.transaction("none"); return Ok((Err(stop_error()),true)); },lease=state.manual_transactions.ordinary(&ids)=>lease? };
        *record.ordinary.lock()=Some(Box::new(ordinary));
        state.executions.require_quiescent(&context.connection_id)?;
        let conn=batch_connection(app.clone(),context.connection_id.clone(),context.namespace.clone(),cfg.kind.is_pg_family()&&!context.namespace.is_empty()).await?;
        let latest=saved_config(&state,&context.connection_id)?;
        if execution_binding(&state,&latest,&record.entry.binding.principal,&context.owner_id,&context.namespace)?!=record.entry.binding { return Err(AppError::coded("EXECUTION_CONTEXT_MISMATCH","固定连接的运行目标已变化")); }
        let pinned=send_work(Box::pin(run_pinned(conn,cfg.kind,record.clone(),payload)));
        Ok(pinned.await)
    }.await;
    // ordinary lease 必须留在拥有工作任务中；supervise 返回 Unknown 不会提前释放。
    record.ordinary.lock().take();
    match result { Ok((result,confirmed))=>(result.map_err(|e|safe_error(&e)),confirmed),Err(error)=>(Err(error),true) }
    })
}
async fn acquire_sqlite(pool:&sqlx::SqlitePool,entry:&Arc<Entry>)->Result<sqlx::pool::PoolConnection<sqlx::Sqlite>> { tokio::select! { biased; _=entry.wait_stop()=>Err(stop_error()),conn=pool.acquire()=>Ok(conn?) } }
async fn acquire_mysql(pool:&sqlx::MySqlPool,entry:&Arc<Entry>)->Result<sqlx::pool::PoolConnection<sqlx::MySql>> { tokio::select! { biased; _=entry.wait_stop()=>Err(stop_error()),conn=pool.acquire()=>Ok(conn?) } }
async fn acquire_pg(pool:&sqlx::PgPool,entry:&Arc<Entry>)->Result<sqlx::pool::PoolConnection<sqlx::Postgres>> { tokio::select! { biased; _=entry.wait_stop()=>Err(stop_error()),conn=pool.acquire()=>Ok(conn?) } }
async fn run_pinned(db:Arc<Db>,kind:DbKind,record:Arc<BatchRecord>,payload:Payload)->(Result<MultiResult>,bool) {
    let __sql_history_context = crate::sql_history::db_context(db.as_ref());
    crate::sql_history::scope(__sql_history_context, async {
    let entry=&record.entry;
    match db.as_ref() {
        Db::Sqlite(pool)=>{
            let mut conn=match acquire_sqlite(pool,entry).await { Ok(c)=>c,Err(e)=>return(Err(e),true) };
            conn.close_on_drop();
            if let Err(e)=execution::install_sqlite(&mut conn,entry,false).await { let _cleared=execution::clear_sqlite(&mut conn).await; if conn.close().await.is_err(){return(Err(AppError::coded("EXECUTION_EFFECTS_UNKNOWN","SQLite 控制安装失败且会话关闭未确认").into()),false);}return(Err(e),true); }
            let (mut result,mut confirmed)=match payload {
                Payload::Generated(rows)=>{ let work=send_work(Box::pin(generate_sqlite(&mut conn,&kind,&record,&rows))); let pair=execution::drive(work,entry,||async{Ok(())}).await; match execution::clear_sqlite(&mut conn).await { Ok(())=>generated_cleanup_sqlite(&mut conn,&record,pair).await,Err(_)=>{record.transaction("unknown");(Err(AppError::coded("EXECUTION_EFFECTS_UNKNOWN","SQLite 控制解除未确认；不能把后续回滚声称为成功").into()),false)} } },
                Payload::File{source,encoding,stop_on_error}=>{ let work=send_work(Box::pin(file_sqlite(&mut conn,record.clone(),source,encoding,stop_on_error))); let pair=execution::drive(work,entry,||async{Ok(())}).await; match execution::clear_sqlite(&mut conn).await { Ok(())=>file_cleanup(pair,&record),Err(_)=>(Err(AppError::coded("EXECUTION_EFFECTS_UNKNOWN","SQLite 控制解除未确认；等待物理会话关闭，禁止重放已有前缀").into()),false) } }
            };
            if let Err(e)=conn.close().await { if record.progress.lock().transaction=="active" { record.transaction("unknown"); } result=Err(AppError::coded("EXECUTION_EFFECTS_UNKNOWN",format!("固定 SQLite 会话关闭失败；不可重放；原因：{e}")).into()); confirmed=false; }
            (result,confirmed)
        }
        Db::Mysql(pool)=>{
            let mut conn=match acquire_mysql(pool,entry).await {Ok(c)=>c,Err(e)=>return(Err(e),true)}; conn.close_on_drop();
            let id:u64=match crate::sql_history::query_scalar("SELECT CONNECTION_ID()").fetch_one(&mut *conn).await {Ok(id)=>id,Err(e)=>{if conn.close().await.is_err(){return(Err(AppError::coded("EXECUTION_EFFECTS_UNKNOWN","MySQL 会话身份读取及关闭未确认").into()),false);}return(Err(e.into()),true);}};
            let opts=pool.connect_options(); let control=move||async move { let mut c=sqlx::MySqlConnection::connect_with(&opts).await?; let outcome=crate::sql_history::query(&format!("KILL QUERY {}",id)).execute(&mut c).await.map(|_|()); let close=c.close().await; match(outcome,close){(Ok(()),Ok(()))=>Ok(()),(Err(e),_)|(_,Err(e))=>Err(e.into())} };
            let (mut result,mut confirmed)=match payload { Payload::Generated(rows)=>{ let pair=execution::drive(Box::pin(generate_mysql(&mut conn,&kind,&record,&rows)),entry,control).await; generated_cleanup_mysql(&mut conn,&record,pair).await },Payload::File{source,encoding,stop_on_error}=>file_cleanup(execution::drive(Box::pin(file_mysql(&mut conn,record.clone(),source,encoding,stop_on_error)),entry,control).await,&record) };
            if conn.close().await.is_err() { result=Err(AppError::coded("EXECUTION_EFFECTS_UNKNOWN","固定 MySQL 会话关闭失败；不可重放").into()); confirmed=false; } (result,confirmed)
        }
        Db::Postgres(pool)=>{
            let mut conn=match acquire_pg(pool,entry).await {Ok(c)=>c,Err(e)=>return(Err(e),true)}; conn.close_on_drop();
            let id:i32=match crate::sql_history::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *conn).await {Ok(id)=>id,Err(e)=>{if conn.close().await.is_err(){return(Err(AppError::coded("EXECUTION_EFFECTS_UNKNOWN","PostgreSQL 会话身份读取及关闭未确认").into()),false);}return(Err(e.into()),true);}};
            let opts=pool.connect_options(); let control=move||async move { let mut c=sqlx::PgConnection::connect_with(&opts).await?; let outcome=crate::sql_history::query_scalar::<_,bool>("SELECT pg_cancel_backend($1)").bind(id).fetch_one(&mut c).await; let close=c.close().await; match(outcome,close){(Ok(true),Ok(()))=>Ok(()),(Ok(false),Ok(()))=>Err(anyhow!("服务器没有确认定向取消")),(Err(e),_)|(_,Err(e))=>Err(e.into())} };
            let (mut result,mut confirmed)=match payload { Payload::Generated(rows)=>{ let pair=execution::drive(Box::pin(generate_pg(&mut conn,&kind,&record,&rows)),entry,control).await; generated_cleanup_pg(&mut conn,&record,pair).await },Payload::File{source,encoding,stop_on_error}=>file_cleanup(execution::drive(Box::pin(file_pg(&mut conn,record.clone(),source,encoding,stop_on_error)),entry,control).await,&record) };
            if conn.close().await.is_err() { result=Err(AppError::coded("EXECUTION_EFFECTS_UNKNOWN","固定 PostgreSQL 会话关闭失败；不可重放").into()); confirmed=false; } (result,confirmed)
        }
        _=>(Err(AppError::coded("BATCH_NATIVE_REQUIRED","当前适配器没有受控单会话取消与排空契约").into()),true)
    }

    }).await
}
fn column(name:String,ty:String,nullable:bool,default:Option<String>,pk:bool,auto:bool)->ColumnInfo { ColumnInfo{name,data_type:ty,nullable,default,primary_key:pk,auto_increment:auto,comment:None,length:None,precision:None,scale:None} }
fn table_info(req:&GeneratedRowsRequest,columns:Vec<ColumnInfo>)->TableSchema { TableSchema{table:req.table.clone(),database:req.schema.clone(),columns,indexes:vec![],foreign_keys:vec![],comment:None,engine:None,collation:None,row_count:None} }
fn generated_validated(record:&BatchRecord,req:&GeneratedRowsRequest,sql:&str) { {let mut p=record.progress.lock();p.scanned=req.rows.len() as u64;p.scan_complete=true;} record.log("info","execute",None,None,"真实字段/生成列/必填/主键及参数宽度校验通过；一个固定物理会话的单批事务",Some(sql)); }
fn received_ack(result:&std::result::Result<u64,sqlx::Error>)->bool {
    result.as_ref().map_or_else(|error|error.as_database_error().is_some(),|_|true)
}
fn count_ack(record:&BatchRecord,result:&std::result::Result<u64,sqlx::Error>) {
    if !received_ack(result) { return; }
    let mut p=record.progress.lock();p.executed+=1;
    match result { Ok(n)=>p.affected+=n,Err(_)=>p.errors+=1 }
}
fn generated_row(record:&BatchRecord,row:u64,sql:&str,result:&std::result::Result<u64,sqlx::Error>,elapsed_ms:u64) {
    count_ack(record,result);
    // 一行一条成功回执会淹没语句本身：成功行只计入 metrics，真正失败的绑定才落一条错误记录。
    if result.is_ok() { return; }
    let summary=if received_ack(result){"参数绑定行收到数据库错误回执；准备排空后回滚整批"}else{"参数绑定行缺少完整数据库回执；不计执行/错误次数，写入结果待确认"};
    let message=format!("{summary}；原因：{}",result.as_ref().unwrap_err());
    record.log_detail("error","execute",Some(1),Some(row),&message,Some(sql),Some(elapsed_ms),None,db_error_code(result));
}
async fn generate_sqlite(conn:&mut sqlx::SqliteConnection,kind:&DbKind,record:&BatchRecord,req:&GeneratedRowsRequest)->Result<MultiResult> {
    data_generator::validate_shape(req)?;
    if record.entry.stopping(){return Err(stop_error());}
    crate::sql_history::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;record.transaction("active");
    // 事务锁取得后读取 main 元数据，避免验证与写入之间被 DDL 替换。
    let definition:Option<String>=crate::sql_history::query_scalar("SELECT sql FROM main.sqlite_master WHERE type='table' AND name=?").bind(&req.table).fetch_optional(&mut *conn).await?;
    let definition=definition.ok_or_else(||anyhow!("目标必须为固定 main 中的真实表"))?;
    let metadata=crate::sql_history::query(&format!("PRAGMA main.table_xinfo({})",db::quote_ident(kind,&req.table))).fetch_all(&mut *conn).await?;
    let mut cols=vec![]; for row in metadata { let name:String=row.try_get("name")?;let hidden:i64=row.try_get("hidden")?;if hidden!=0 {if req.columns.contains(&name){bail!("生成/隐藏字段不可写入");}continue;}cols.push(column(name,row.try_get("type")?,row.try_get::<i64,_>("notnull")?==0,row.try_get("dflt_value")?,row.try_get::<i64,_>("pk")?>0,false)); }
    let primary=cols.iter().filter(|c|c.primary_key).count(); if primary==1&&!definition.to_ascii_uppercase().contains("WITHOUT ROWID"){for c in &mut cols{if c.primary_key&&c.data_type.eq_ignore_ascii_case("INTEGER")&&!req.columns.contains(&c.name){c.auto_increment=true;}}}
    data_generator::validate_columns(req,&table_info(req,cols))?;
    let unqualified=data_generator::insert_sql(kind,req,&[]);let table=db::quote_ident(kind,&req.table);let sql=unqualified.replacen(&format!("INSERT INTO {}",table),&format!("INSERT INTO \"main\".{}",table),1);
    generated_validated(record,req,&sql); if record.entry.stopping(){return Err(stop_error());}
    for(index,row)in req.rows.iter().enumerate(){if record.entry.stopping(){return Err(stop_error());}let mut query=crate::sql_history::query(&sql);for value in row{query=query.bind(value);}let began=Instant::now();let result=query.execute(&mut *conn).await.map(|r|r.rows_affected());generated_row(record,index as u64+1,&sql,&result,began.elapsed().as_millis() as u64);result?;}
    if record.entry.stopping(){return Err(stop_error());}let batch_rows=record.progress.lock().affected;let batch_elapsed=record.entry.status().deadline.elapsed_ms;record.log_detail("success","execute",Some(1),None,"参数绑定批量写入收到数据库回执（尚待整批提交）",Some(&sql),Some(batch_elapsed),Some(batch_rows),None);record.log("info","commit",None,None,"等待整批 COMMIT 回执",None);crate::sql_history::query("COMMIT").execute(&mut *conn).await.map_err(|e|AppError::coded("EXECUTION_EFFECTS_UNKNOWN",format!("COMMIT 回执失败；整批提交结果未知，禁止重放；原因：{e}")))?;record.transaction("committed");record.log("success","commit",None,None,"整批 COMMIT 已确认",None);Ok(record.result())
}
async fn generate_mysql(conn:&mut sqlx::MySqlConnection,kind:&DbKind,record:&BatchRecord,req:&GeneratedRowsRequest)->Result<MultiResult> {
    if record.entry.stopping(){return Err(stop_error());}
    // START TRANSACTION 不在 MySQL 预处理语句白名单内，服务端返回 1295；必须走文本协议。
    (&mut *conn).execute("START TRANSACTION").await?;record.transaction("active");
    // 零行实体表访问持有事务级 MDL，先固定对象，再校验 InnoDB/真实字段。
    crate::sql_history::query(&format!("SELECT 1 FROM {} LIMIT 0",db::qualified(kind,&req.schema,&req.table))).execute(&mut *conn).await?;
    let table=crate::sql_history::query("SELECT TABLE_TYPE,ENGINE FROM information_schema.TABLES WHERE TABLE_SCHEMA=? AND TABLE_NAME=?").bind(&req.schema).bind(&req.table).fetch_optional(&mut *conn).await?.ok_or_else(||anyhow!("表不存在"))?;
    if crate::schema::mysql_get(&table,"TABLE_TYPE").as_deref()!=Some("BASE TABLE")||!crate::schema::mysql_get(&table,"ENGINE").is_some_and(|v|v.eq_ignore_ascii_case("InnoDB")){bail!("生成必须为真实 InnoDB 事务表");}
    let metadata=crate::sql_history::query("SELECT COLUMN_NAME,COLUMN_TYPE,IS_NULLABLE,COLUMN_DEFAULT,COLUMN_KEY,EXTRA FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY ORDINAL_POSITION").bind(&req.schema).bind(&req.table).fetch_all(&mut *conn).await?;
    let mut cols=vec![];for row in metadata{let get=|name|crate::schema::mysql_get(&row,name);let name=get("COLUMN_NAME").unwrap_or_default();let extra=get("EXTRA").unwrap_or_default();if data_generator::mysql_generated_column(&extra){if req.columns.contains(&name){bail!("生成字段不可写入");}continue;}cols.push(column(name,get("COLUMN_TYPE").unwrap_or_default(),get("IS_NULLABLE").as_deref()==Some("YES"),get("COLUMN_DEFAULT"),get("COLUMN_KEY").as_deref()==Some("PRI"),extra.to_ascii_lowercase().contains("auto_increment")));}
    data_generator::validate_columns(req,&table_info(req,cols))?;let sql=data_generator::insert_sql(kind,req,&[]);generated_validated(record,req,&sql);if record.entry.stopping(){return Err(stop_error());}
    for(index,row)in req.rows.iter().enumerate(){if record.entry.stopping(){return Err(stop_error());}let mut query=crate::sql_history::query(&sql);for value in row{query=query.bind(value);}let began=Instant::now();let result=query.execute(&mut *conn).await.map(|r|r.rows_affected());generated_row(record,index as u64+1,&sql,&result,began.elapsed().as_millis() as u64);result?;}
    if record.entry.stopping(){return Err(stop_error());}let batch_rows=record.progress.lock().affected;let batch_elapsed=record.entry.status().deadline.elapsed_ms;record.log_detail("success","execute",Some(1),None,"参数绑定批量写入收到数据库回执（尚待整批提交）",Some(&sql),Some(batch_elapsed),Some(batch_rows),None);record.log("info","commit",None,None,"等待整批 COMMIT 回执",None);crate::sql_history::query("COMMIT").execute(&mut *conn).await.map_err(|e|AppError::coded("EXECUTION_EFFECTS_UNKNOWN",format!("COMMIT 回执失败；提交结果未知，禁止重放；原因：{e}")))?;record.transaction("committed");Ok(record.result())
}
async fn generate_pg(conn:&mut sqlx::PgConnection,kind:&DbKind,record:&BatchRecord,req:&GeneratedRowsRequest)->Result<MultiResult> {
    execution::prepare_pg_deadline(conn,&record.entry).await?;if record.entry.stopping(){return Err(stop_error());}
    crate::sql_history::query("BEGIN").execute(&mut *conn).await?;record.transaction("active");
    crate::sql_history::query(&format!("LOCK TABLE {} IN ROW EXCLUSIVE MODE",db::qualified(kind,&req.schema,&req.table))).execute(&mut *conn).await?;
    let metadata=crate::sql_history::query("SELECT a.attname,tn.nspname AS type_schema,t.typname,a.attnotnull,a.attgenerated::text AS generated,a.attidentity::text AS identity,pg_get_expr(d.adbin,d.adrelid) AS default_value,EXISTS(SELECT 1 FROM pg_index i WHERE i.indrelid=c.oid AND i.indisprimary AND a.attnum=ANY(i.indkey)) AS is_pk FROM pg_attribute a JOIN pg_class c ON c.oid=a.attrelid JOIN pg_namespace n ON n.oid=c.relnamespace JOIN pg_type t ON t.oid=a.atttypid JOIN pg_namespace tn ON tn.oid=t.typnamespace LEFT JOIN pg_attrdef d ON d.adrelid=c.oid AND d.adnum=a.attnum WHERE n.nspname=$1 AND c.relname=$2 AND c.relkind IN ('r','p') AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum").bind(&req.schema).bind(&req.table).fetch_all(&mut *conn).await?;
    if metadata.is_empty(){bail!("目标必须为真实表");}let mut cols=vec![];let mut types=HashMap::new();
    for row in metadata{let name:String=row.try_get("attname")?;let generated:String=row.try_get("generated")?;let identity:String=row.try_get("identity")?;if (!generated.is_empty()||!identity.is_empty())&&req.columns.contains(&name){bail!("生成/标识字段不可写入");}if !generated.is_empty(){continue;}let ty:String=row.try_get("typname")?;let ns:String=row.try_get("type_schema")?;types.insert(name.clone(),format!("{}.{}",db::quote_ident(kind,&ns),db::quote_ident(kind,&ty)));let default:Option<String>=row.try_get("default_value")?;let auto=!identity.is_empty()||default.as_ref().is_some_and(|v|v.starts_with("nextval("));cols.push(column(name,ty,!row.try_get::<bool,_>("attnotnull")?,default,row.try_get("is_pk")?,auto));}
    data_generator::validate_columns(req,&table_info(req,cols))?;let types:Vec<String>=req.columns.iter().map(|n|types.get(n).cloned().ok_or_else(||anyhow!("字段类型不存在"))).collect::<Result<_>>()?;let sql=data_generator::insert_sql(kind,req,&types);generated_validated(record,req,&sql);execution::prepare_pg_deadline(conn,&record.entry).await?;if record.entry.stopping(){return Err(stop_error());}
    for(index,row)in req.rows.iter().enumerate(){if record.entry.stopping(){return Err(stop_error());}let mut query=crate::sql_history::query(&sql);for value in row{query=query.bind(value);}let began=Instant::now();let result=query.execute(&mut *conn).await.map(|r|r.rows_affected());generated_row(record,index as u64+1,&sql,&result,began.elapsed().as_millis() as u64);result?;}
    if record.entry.stopping(){return Err(stop_error());}let batch_rows=record.progress.lock().affected;let batch_elapsed=record.entry.status().deadline.elapsed_ms;record.log_detail("success","execute",Some(1),None,"参数绑定批量写入收到数据库回执（尚待整批提交）",Some(&sql),Some(batch_elapsed),Some(batch_rows),None);record.log("info","commit",None,None,"等待整批 COMMIT 回执",None);crate::sql_history::query("COMMIT").execute(&mut *conn).await.map_err(|e|AppError::coded("EXECUTION_EFFECTS_UNKNOWN",format!("COMMIT 回执失败；提交结果未知，禁止重放；原因：{e}")))?;record.transaction("committed");Ok(record.result())
}
macro_rules! generated_cleanup { ($name:ident,$conn:ty)=>{
    async fn $name(conn:&mut $conn,record:&BatchRecord,(result,mut confirmed):(Result<MultiResult>,bool))->(Result<MultiResult>,bool){
        let tx=record.progress.lock().transaction.clone();
        if tx=="active" {
            if result.as_ref().err().and_then(|e|e.downcast_ref::<AppError>()).is_some_and(|e|e.code=="EXECUTION_EFFECTS_UNKNOWN"){record.transaction("unknown");return(result,false);}
            record.log("info","rollback",None,None,"所有执行/取消控制已排空，等待整批 ROLLBACK 回执",None);
            match crate::sql_history::query("ROLLBACK").execute(conn).await{Ok(_)=>{record.transaction("rolled_back");record.progress.lock().affected=0;confirmed=true;record.log("success","rollback",None,None,"整批 ROLLBACK 已确认；没有该批持久写入",None);},Err(e)=>{record.transaction("unknown");let original=result.as_ref().err().map(|error|format!("；原执行错误：{error:#}")).unwrap_or_default();return(Err(AppError::coded("EXECUTION_EFFECTS_UNKNOWN",format!("ROLLBACK 回执失败；事务结果未知，禁止重放；原因：{e}{original}")).into()),false);}}
        }else if tx=="none" {record.transaction("none");if record.entry.stopping(){confirmed=true;}}
        (result,confirmed)
    }
}; }
generated_cleanup!(generated_cleanup_sqlite_generic,sqlx::SqliteConnection);
async fn generated_cleanup_sqlite(conn:&mut sqlx::SqliteConnection,record:&BatchRecord,pair:(Result<MultiResult>,bool))->(Result<MultiResult>,bool){
    let active=record.progress.lock().transaction=="active";
    let known_failure=pair.0.as_ref().err().is_some_and(|e|!e.downcast_ref::<AppError>().is_some_and(|a|a.code=="EXECUTION_EFFECTS_UNKNOWN"));
    if active&&known_failure {
        let autocommit={ let mut handle=match conn.lock_handle().await{Ok(h)=>h,Err(e)=>return(Err(e.into()),false)};
            // SAFETY: 固定存活 SQLite 物理连接，持有 SQLx worker handle 锁，只读取事务状态。
            unsafe{libsqlite3_sys::sqlite3_get_autocommit(handle.as_raw_handle().as_ptr())!=0} };
        if autocommit {record.transaction("rolled_back");record.progress.lock().affected=0;record.log("success","rollback",None,None,"SQLite VM 已中断并自动回滚；固定句柄 autocommit 状态确认整批结束",None);return(pair.0,true);}
    }
    generated_cleanup_sqlite_generic(conn,record,pair).await
}
generated_cleanup!(generated_cleanup_mysql,sqlx::MySqlConnection);
generated_cleanup!(generated_cleanup_pg,sqlx::PgConnection);

fn file_cleanup((result,confirmed):(Result<MultiResult>,bool),record:&BatchRecord)->(Result<MultiResult>,bool){
    let between=record.entry.stopping()&&result.as_ref().err().and_then(|e|e.downcast_ref::<AppError>()).is_some_and(|e|matches!(e.code.as_str(),"BATCH_STOPPED"|"BATCH_SCAN_STOPPED"));
    // 已完整排空且尚未提交下一语句的停止可以确认；成功前缀仍不可推断回滚。
    (result,confirmed||between)
}
enum Scan { Statement(Statement), End, Error(AppError) }
fn scanner(record:Arc<BatchRecord>,source:SourceFile,encoding:String,dialect:Dialect)->(tokio::sync::mpsc::Receiver<Scan>,tokio::task::JoinHandle<()>) {
    // 最多两个完整语句排队；不聚合完整文件/结果集。句柄始终由该 blocking 任务拥有。
    let(tx,rx)=tokio::sync::mpsc::channel(2);
    let task=tokio::task::spawn_blocking(move||{
        let decoder=match DecodeReader::new(source,&encoding){Ok(d)=>d,Err(e)=>{let _=tx.blocking_send(Scan::Error(safe_error(&e)));return;}};
        let mut reader=Reader::new(decoder,dialect,Some(record.entry.clone()));
        loop { match reader.next_statement(){Ok(Some(statement))=>{record.progress.lock().scanned+=1;if tx.blocking_send(Scan::Statement(statement)).is_err(){return;}},Ok(None)=>{record.progress.lock().scan_complete=true;let _=tx.blocking_send(Scan::End);return;},Err(error)=>{if record.entry.stopping(){return;}let error=AppError::coded("BATCH_SQL_PARSE_FAILED",clipped(&error.to_string(),512));record.progress.lock().parse_error=Some(error.message.clone());record.log("error","scan",None,None,&error.message,None);let _=tx.blocking_send(Scan::Error(error));return;}} }
    });(rx,task)
}
fn file_ack(record:&BatchRecord,index:u64,sql:&str,dialect:Dialect,result:&std::result::Result<u64,sqlx::Error>,elapsed_ms:u64){
    count_ack(record,result);
    let summary=if result.is_ok(){"SQL 执行并排空结果收到数据库回执"}else if received_ack(result){"SQL 执行收到错误回执（不计为扫描错误）"}else{"SQL 缺少完整数据库回执；不计执行/错误次数，停止尾部并禁止重放"};
    let message=match result { Ok(_)=>summary.to_owned(),Err(error)=>format!("{summary}；原因：{error}") };
    // 摘要覆盖实际发送的解码后完整语句；不是原始 GBK 等源文件字节的 hash。
    let message=format!("{message}；解码后语句 SHA-256:{:x}; UTF-8:{} bytes",Sha256::digest(sql.as_bytes()),sql.len());
    let audit=crate::batch_sql_reader::redacted_template(sql, dialect);record.log_detail(if result.is_ok(){"success"}else{"error"},"execute",Some(index),None,&message,Some(&audit),Some(elapsed_ms),result.as_ref().ok().copied(),db_error_code(result));
}
macro_rules! file_worker {($name:ident,$conn:ty,$dialect:expr,$pg:expr)=>{
    async fn $name(conn:&mut $conn,record:Arc<BatchRecord>,source:SourceFile,encoding:String,stop_on_error:bool)->Result<MultiResult>{
        let(mut rx,task)=scanner(record.clone(),source,encoding,$dialect);let mut index=0u64;
        let result=async {
            loop{if record.entry.stopping(){return Err(stop_error());}let next=tokio::select!{biased;_=record.entry.wait_stop()=>return Err(stop_error()),item=rx.recv()=>item};match next{
                Some(Scan::Statement(statement))=>{if record.entry.stopping(){return Err(stop_error());}index+=1;$pg(conn,&record.entry).await?;let began=Instant::now();let mut sql_history=crate::sql_history::Guard::new(&statement.sql,Vec::new());let result=sqlx::Executor::execute(&mut *conn,sqlx::raw_sql(&statement.sql)).await.map(|r|r.rows_affected());sql_history.result(&result,|n|*n as i64);file_ack(&record,index,&statement.sql,$dialect,&result,began.elapsed().as_millis() as u64);if let Err(error)=result{if error.as_database_error().is_none(){return Err(AppError::coded("EXECUTION_EFFECTS_UNKNOWN","SQL 完整回执缺失；停止执行尾部，既有写入未知，禁止重放").into());}if record.entry.stopping()||stop_on_error{return Err(error.into());}}},
                Some(Scan::End)=>{if record.progress.lock().errors>0{return Err(AppError::coded("BATCH_SQL_ERRORS","文件扫描完成但部分执行回执失败；已成功写入不自动回滚/重放").into());}return Ok(record.result());},Some(Scan::Error(error))=>return Err(error.into()),None=>{if record.entry.stopping(){return Err(stop_error());}return Err(AppError::coded("BATCH_SCAN_INCOMPLETE","扫描任务在 EOF 回执前结束；已执行前缀不可重放").into());}
            }}
        }.await;
        drop(rx);if task.await.is_err(){return Err(AppError::coded("EXECUTION_EFFECTS_UNKNOWN","SQL 固定源扫描任务异常；资源清理无法证明，禁止重放").into());}result
    }
}; }
#[cfg(test)]
#[path="batch_execution_tests.rs"]
mod tests;
async fn no_deadline<C>(_conn:&mut C,_entry:&Arc<Entry>)->Result<()> { Ok(()) }
file_worker!(file_sqlite,sqlx::SqliteConnection,Dialect::Sqlite,no_deadline);
file_worker!(file_mysql,sqlx::MySqlConnection,Dialect::Mysql,no_deadline);
file_worker!(file_pg,sqlx::PgConnection,Dialect::Postgres,execution::prepare_pg_deadline);
