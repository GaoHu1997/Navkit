//! 原生表工作流。目录、CAS、DDL、验证始终使用同一物理会话。
use anyhow::{anyhow, ensure, Result};
use sqlx::{Row, Column, ValueRef, Connection, Executor};
use std::{collections::BTreeMap, sync::Arc, time::Instant};
use sha2::{Digest, Sha256};
use futures::TryStreamExt;
use crate::{db::{self,Db}, models::{DbKind,MultiResult}, execution::{self,Binding,Entry}, manual_transaction::OrdinaryLease, error::{AppError,CmdResult}, design::{self,TableDesign,DesignStep,DesignStepResult,DesignReceipt}, table_workflows::{self as workflow,Stored,TableWork,BoundResult,TableDesignReceipt,TableOperationReceipt}};
#[path="table_workflows_native_metadata.rs"] mod metadata;
#[path="table_workflows_native_plan.rs"] mod plan;
pub(crate) type Record=BTreeMap<String,Option<String>>;
pub(crate) fn get<'a>(r:&'a Record,k:&str)->Result<&'a str>{r.get(k).and_then(|v|v.as_deref()).ok_or_else(||anyhow!("TABLE_METADATA_REQUIRED: {k}"))}
pub(crate) fn opt(r:&Record,k:&str)->Result<Option<String>>{r.get(k).cloned().ok_or_else(||anyhow!("TABLE_METADATA_FIELD_MISSING: {k}"))}
pub(crate) enum Session {Mysql(sqlx::pool::PoolConnection<sqlx::MySql>,Option<String>),Pg(sqlx::pool::PoolConnection<sqlx::Postgres>)}
impl Session {
    pub(crate) fn kind(&self)->DbKind{DbKind::of(if matches!(self,Self::Mysql(..)){"mysql"}else{"postgresql"})}
    pub(crate) async fn acquire(db:&Db)->Result<Self>{
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {match db{Db::Mysql(p)=>Ok(Self::Mysql(p.acquire().await?,None)),Db::Postgres(p)=>Ok(Self::Pg(p.acquire().await?)),_=>Err(anyhow!("TABLE_NATIVE_ADAPTER_UNSUPPORTED"))}
    }).await
}
    async fn setup(&mut self)->Result<()>{if let Self::Mysql(c,mode)=self{let old:String=crate::sql_history::query_scalar("SELECT @@session.sql_mode").fetch_one(&mut **c).await?;*mode=Some(old.clone());if !old.split(',').any(|v|v=="NO_BACKSLASH_ESCAPES"){let wanted=if old.is_empty(){"NO_BACKSLASH_ESCAPES".into()}else{format!("{old},NO_BACKSLASH_ESCAPES")};crate::sql_history::query("SET SESSION sql_mode=?").bind(wanted).execute(&mut **c).await?;}}Ok(())}
    pub(crate) async fn rows(&mut self,sql:&str)->Result<Vec<Record>>{
        macro_rules! read {($conn:expr,$extra:ty)=>{{let rows=crate::sql_history::query(sql).fetch_all(&mut **$conn).await?;let mut out=Vec::new();for row in rows{let mut r=Record::new();for (i,c) in row.columns().iter().enumerate(){let v=if row.try_get_raw(i)?.is_null(){None}else if let Ok(s)=row.try_get::<String,_>(i){Some(s)}else if let Ok(bytes)=row.try_get::<Vec<u8>,_>(i){Some(String::from_utf8(bytes).map_err(|_|anyhow!("TABLE_METADATA_UTF8_INVALID: {}",c.name()))?)}else if let Ok(n)=row.try_get::<i64,_>(i){Some(n.to_string())}else if let Ok(n)=row.try_get::<i32,_>(i){Some(n.to_string())}else if let Ok(n)=row.try_get::<$extra,_>(i){Some(n.to_string())}else if let Ok(n)=row.try_get::<bool,_>(i){Some(if n{"1"}else{"0"}.into())}else if let Ok(n)=row.try_get::<chrono::NaiveDateTime,_>(i){Some(n.to_string())}else if let Ok(n)=row.try_get::<chrono::DateTime<chrono::Utc>,_>(i){Some(n.naive_utc().to_string())}else{return Err(anyhow!("TABLE_METADATA_DECODE_FAILED: {}",c.name()));};ensure!(r.insert(c.name().to_string(),v).is_none(),"TABLE_METADATA_DUPLICATE_FIELD");}out.push(r);}Ok(out)}};}
        match self{Self::Mysql(c,_)=>read!(c,u64),Self::Pg(c)=>read!(c,i16)}
    }
    /// MySQL 侧走文本协议：START TRANSACTION / LOCK TABLES / UNLOCK TABLES 不在服务端预处理白名单内（1295）。
    pub(crate) async fn execute(&mut self,sql:&str)->Result<u64>{match self{Self::Mysql(c,_)=>Ok((&mut **c).execute(sql).await?.rows_affected()),Self::Pg(c)=>Ok(crate::sql_history::query(sql).execute(&mut **c).await?.rows_affected())}}
    pub(crate) async fn scalar(&mut self,sql:&str)->Result<String>{let r=self.rows(sql).await?;ensure!(r.len()==1&&r[0].len()==1,"TABLE_METADATA_SCALAR_REQUIRED");r[0].values().next().unwrap().clone().ok_or_else(||anyhow!("TABLE_METADATA_NULL_SCALAR"))}
    pub(crate) async fn release(self,destroy:bool)->Result<()>{match self{Self::Mysql(mut c,mode)=>{if destroy{c.close_on_drop();c.close().await?;}else{if let Some(mode)=mode{if let Err(e)=crate::sql_history::query("SET SESSION sql_mode=?").bind(&mode).execute(&mut *c).await{c.close_on_drop();let _=c.close().await;return Err(e.into());}}c.return_to_pool().await;}},Self::Pg(mut c)=>{if destroy{c.close_on_drop();c.close().await?;}else{c.return_to_pool().await;}}}Ok(())}
    async fn digest(&mut self,before:&Stored,table:&str)->Result<(u64,String)>{
        let kind=self.kind();let full=db::qualified(&kind,&before.public.schema.database,table);
        let mut fields=Vec::new();
        for c in &before.public.schema.columns{let n=db::quote_ident(&kind,&c.name);fields.push(if kind.is_mysql_family(){format!("CASE WHEN {n} IS NULL THEN 'N' ELSE CONCAT('V',HEX(CAST({n} AS BINARY))) END")}else{format!("CASE WHEN {n} IS NULL THEN 'N' ELSE 'V'||encode(convert_to({n}::text,'UTF8'),'hex') END COLLATE \"C\"")});}
        let sql=format!("SELECT {} FROM {full} ORDER BY {}",fields.join(","),(1..=fields.len()).map(|i|i.to_string()).collect::<Vec<_>>().join(","));
        let mut hash=Sha256::new();let mut count=0u64;
        macro_rules! stream {($c:expr)=>{{let mut rows=crate::sql_history::query(&sql).fetch(&mut **$c);while let Some(row)=rows.try_next().await?{for i in 0..fields.len(){let s:String=row.try_get(i)?;hash.update((s.len() as u64).to_le_bytes());hash.update(s.as_bytes());}count+=1;}}};}
        match self{Self::Mysql(c,_)=>stream!(c),Self::Pg(c)=>stream!(c)}hash.update(count.to_le_bytes());Ok((count,format!("{:x}",hash.finalize())))
    }
}
#[derive(Clone,serde::Serialize)] pub(crate) struct NativeMetadata {
    pub identity:String,pub physical:String,pub columns:Vec<Record>,pub indexes:Vec<Record>,pub constraints:Vec<Record>,pub triggers:Vec<Record>,pub table:Record,pub sequences:Vec<Record>,
}
pub(crate) async fn snapshot(s:&mut Session,b:&Binding,table:&str,readonly:bool)->Result<Stored>{metadata::snapshot(s,b,table,readonly).await}
pub(crate) fn design_steps(b:&Stored,d:&TableDesign)->Result<(Vec<DesignStep>,bool)>{plan::design_steps(b,d)}
pub(crate) fn operation_steps(b:&Stored,o:&workflow::TableOperation)->Result<Vec<DesignStep>>{plan::operation_steps(b,o)}
pub(crate) async fn read_owned(db:Arc<Db>,b:Binding,table:String,readonly:bool,ordinary:OrdinaryLease)->CmdResult<Stored>{
    let __sql_history_context = crate::sql_history::db_context(db.as_ref());
    crate::sql_history::scope(__sql_history_context, async {
    crate::sql_history::spawn(async move{let _lease=ordinary;let mut s=Session::acquire(&db).await.map_err(AppError::from)?;let pg=matches!(s,Session::Pg(_));
        let result:Result<Stored>=async{s.setup().await?;if pg{s.execute("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY").await?;}else{ensure!(s.scalar("SELECT @@session.autocommit").await?=="1","TABLE_SESSION_TRANSACTION_ACTIVE");s.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ").await?;s.execute("START TRANSACTION READ ONLY").await?;}
            // MySQL MDL 在事务存续期间固定对象，防止 SHOW CREATE 与目录混代。
            s.rows(&format!("SELECT * FROM {} LIMIT 0",db::qualified(&s.kind(),&b.namespace,&table))).await?;snapshot(&mut s,&b,&table,readonly).await}.await;
        let rollback=s.execute("ROLLBACK").await;let destroy=rollback.is_err();s.release(destroy).await.map_err(|e|unknown(format!("TABLE_METADATA_CLOSE_UNKNOWN: {e:#}")))?;rollback.map_err(|e|unknown(format!("TABLE_METADATA_ROLLBACK_UNKNOWN: {e:#}")))?;result.map_err(AppError::from)
    }).await.map_err(|e|unknown(format!("TABLE_METADATA_TASK_INTERRUPTED: {e}")))?

    }).await
}
fn unknown(m:impl Into<String>)->AppError{AppError::coded("EXECUTION_EFFECTS_UNKNOWN",m)}
fn mysql_remote_state(verified:bool,applied:bool,uncertain:bool,stopped:bool)->&'static str{if uncertain||stopped&&applied{"unknown"}else if verified{"committed"}else if applied{"unknown"}else{"none"}}
fn is_unknown(e:&anyhow::Error)->bool{e.downcast_ref::<AppError>().is_some_and(|e|e.code=="EXECUTION_EFFECTS_UNKNOWN")||e.downcast_ref::<sqlx::Error>().is_some_and(|e|!matches!(e,sqlx::Error::Database(_)))}

// 仅新建复制目标及触发器 DDL 需要退出 LOCK TABLES；列名、字符串中的 TRIGGER 不算 DDL 头部。
fn mysql_step_requires_unlock(copy:bool,position:usize,sql:&str)->Result<bool>{
    let tokens=crate::table_objects::tokens(sql)?;
    let create=tokens.first().is_some_and(|t|t.word("CREATE"));
    let drop=tokens.first().is_some_and(|t|t.word("DROP"));
    let creates_target=copy&&position==0&&create&&tokens.get(1).is_some_and(|t|t.word("TABLE"));
    let header=tokens.iter().skip(1).take_while(|t|!t.word("ON")&&!t.symbol("("));
    let trigger_ddl=(create||drop)&&header.clone().any(|t|t.word("TRIGGER"))
        &&!header.into_iter().any(|t|t.word("TABLE")||t.word("INDEX")||t.word("VIEW"));
    Ok(creates_target||trigger_ddl)
}

// 清理在 drive 返回后执行，避免迟到的定向取消击中 COMMIT/ROLLBACK 或归还后的连接。
async fn apply(s:&mut Session,b:&Binding,entry:&Arc<Entry>,work:&TableWork,steps_out:&mut Vec<DesignStepResult>)->Result<(Stored,u64,u64)>{
    let table=match work{TableWork::Design(d)=>&d.table,TableWork::Operation(o)=>&o.table};let kind=s.kind();let full=db::qualified(&kind,&b.namespace,table);
    if kind.is_pg_family(){s.execute("BEGIN").await?;s.execute(&format!("LOCK TABLE {full} IN ACCESS EXCLUSIVE MODE")).await?;}else{
        ensure!(s.scalar("SELECT @@session.autocommit").await?=="1","TABLE_SESSION_TRANSACTION_ACTIVE");
        let lock=plan::lock_name(b,table);ensure!(s.scalar(&format!("SELECT GET_LOCK({},10)",db::quote_literal(&lock))).await?=="1","TABLE_CAS_LOCK_BUSY");
        // LOCK TABLES 跨隐式提交持有服务端元数据写锁；同会话每条 DDL 后重新核验完整定义。
        s.execute(&format!("LOCK TABLES {full} WRITE")).await?;
    }
    ensure!(!entry.stopping(),"TABLE_STOPPED_BEFORE_DDL");let before=snapshot(s,b,table,false).await?;
    let steps=match work{TableWork::Design(d)=>{workflow::require_baseline(&before,d.base_revision.as_deref(),d.source_fingerprint.as_deref(),b)?;design_steps(&before,d)?.0},TableWork::Operation(o)=>{workflow::require_baseline(&before,Some(&o.base_revision),Some(&o.source_fingerprint),b)?;if o.kind!="clear"{let target=o.target_table.as_deref().ok_or_else(||anyhow!("TABLE_COPY_TARGET_REQUIRED"))?;let exists=if kind.is_pg_family(){format!("SELECT count(*)::text FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname={} AND c.relname={}",db::quote_literal(&b.namespace),db::quote_literal(target))}else{format!("SELECT CAST(count(*) AS CHAR) FROM information_schema.TABLES WHERE TABLE_SCHEMA={} AND TABLE_NAME={}",db::quote_literal(&b.namespace),db::quote_literal(target))};ensure!(s.scalar(&exists).await?=="0","TABLE_COPY_TARGET_EXISTS");}operation_steps(&before,o)?}};
    let source_digest=if matches!(work,TableWork::Operation(o) if o.kind=="copy_data"){Some(s.digest(&before,table).await?)}else{None};let mut affected=0;
    let mut expected=before.public.revision.clone();
    let copy=matches!(work,TableWork::Operation(o) if o.kind!="clear");
    for (position,step) in steps.iter().enumerate(){ensure!(!entry.stopping(),"TABLE_STOPPED_DURING_DDL");
        if kind.is_mysql_family(){ensure!(snapshot(s,b,table,false).await?.public.revision==expected,"TABLE_CONCURRENT_DDL_DETECTED");}
        // LOCK TABLES 只放行已加锁对象：新建复制目标表、以及对触发器做 CREATE/DROP，都会命中 1100（Table ... was not locked with LOCK TABLES）。
        // 只在这两处边界释放物理表锁；GET_LOCK 仍由本会话持有，执行后立刻重新加锁并复验源定义，外部并发改写会在提交前被拒绝。
        let released=kind.is_mysql_family()&&mysql_step_requires_unlock(copy,position,&step.sql)?;
        if released{s.execute("UNLOCK TABLES").await?;ensure!(!entry.stopping(),"TABLE_STOPPED_DURING_DDL");}
        match s.execute(&step.sql).await{Ok(n)=>{if step.sql.starts_with("DELETE "){affected=n;}steps_out.push(DesignStepResult{label:step.label.clone(),sql:step.sql.clone(),status:"applied".into(),error:None});},Err(e)=>{steps_out.push(DesignStepResult{label:step.label.clone(),sql:step.sql.clone(),status:"failed".into(),error:Some(e.to_string())});return Err(e);}}
        if kind.is_mysql_family(){
            // 新建目标后把源/目标一起固定；触发器 DDL 后把源表重新固定。LOCK TABLES 只在同一 owned 会话更新。
            if copy{let target=match work{TableWork::Operation(o)=>o.target_table.as_deref().ok_or_else(||anyhow!("TABLE_COPY_TARGET_REQUIRED"))?,_=>return Err(anyhow!("TABLE_OPERATION_UNSUPPORTED"))};let dest=db::qualified(&kind,&b.namespace,target);s.execute(&format!("LOCK TABLES {full} WRITE, {dest} WRITE")).await?;}else if released{s.execute(&format!("LOCK TABLES {full} WRITE")).await?;}
            let source_after=snapshot(s,b,table,false).await?;
            if copy{ensure!(source_after.base==before.base&&source_after.native.as_ref().map(|m|&m.identity)==before.native.as_ref().map(|m|&m.identity),"TABLE_COPY_SOURCE_CHANGED");if released{if let Some(digest)=&source_digest{ensure!(s.digest(&before,table).await?==*digest,"TABLE_COPY_SOURCE_DATA_CHANGED");}}}
            expected=source_after.public.revision;
        }
    }
    let target=match work{TableWork::Operation(o)=>o.target_table.as_deref().unwrap_or(table),_=>table};
    if let TableWork::Operation(o)=work{if o.kind=="copy_data"{plan::restore_sequences(s,&before,target).await?;}}
    let after=snapshot(s,b,target,false).await?;let copied=match work{
        TableWork::Design(d)=>{let mut verified_design=d.clone();if kind.is_pg_family(){plan::canonical_checks(s,&after,&mut verified_design).await?;}plan::verify_design(&before,&after,&verified_design)?;0},
        TableWork::Operation(o) if o.kind=="clear"=>{ensure!(s.scalar(&format!("SELECT count(*) FROM {full}")).await?=="0"&&before.base==after.base,"TABLE_CLEAR_READBACK_MISMATCH");0},
        TableWork::Operation(o)=>{plan::verify_copy(&before,&after,target)?;let (count,digest)=s.digest(&before,target).await?;if o.kind=="copy_data"{let expected=source_digest.as_ref().unwrap();ensure!(count==expected.0&&digest==expected.1,"TABLE_COPY_DATA_DIGEST_MISMATCH");ensure!(s.digest(&before,table).await?==*expected,"TABLE_COPY_SOURCE_DATA_CHANGED");}else{ensure!(count==0,"TABLE_COPY_STRUCTURE_HAS_ROWS");}let source_after=snapshot(s,b,table,false).await?;ensure!(source_after.base==before.base&&source_after.native.as_ref().map(|m|&m.identity)==before.native.as_ref().map(|m|&m.identity),"TABLE_COPY_SOURCE_CHANGED");count}
    };ensure!(!entry.stopping(),"TABLE_STOPPED_BEFORE_COMMIT");Ok((after,affected,copied))
}
pub(crate) async fn run(db:Arc<Db>,entry:Arc<Entry>,ordinary:OrdinaryLease,work:TableWork,readonly:bool)->BoundResult{
    let __sql_history_context = crate::sql_history::db_context(db.as_ref());
    crate::sql_history::scope(__sql_history_context, async {
    if readonly{return BoundResult{result:Err(AppError::coded("READ_ONLY_VIOLATION","只读连接禁止表修改")),confirmed:false,design:None,operation:None};}
    if let Err(e)=workflow::validate_work(&work,&entry.binding){return BoundResult{result:Err(e),confirmed:false,design:None,operation:None};}
    let published=Arc::new(parking_lot::Mutex::new((None,None)));let output=published.clone();let working=entry.clone();
    let (result,confirmed)=execution::supervise(entry.clone(),async move{let _lease=ordinary;let started=Instant::now();let mut s=match tokio::select!{biased;_=working.wait_stop()=>Err(anyhow!("TABLE_STOPPED_BEFORE_SEND")),s=Session::acquire(&db)=>s}{Ok(s)=>s,Err(e)=>return(Err(AppError::from(e)),true)};
        if working.stopping()||!working.start(){let _=s.release(false).await;return(Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND","发送前已停止")),true);}
        if let Err(e)=s.setup().await{let close=s.release(true).await;return(Err(unknown(format!("TABLE_SESSION_SETUP_UNKNOWN: {e:#}; close={close:?}"))),false);}
        let pg=matches!(s,Session::Pg(_));let mut receipts=Vec::new();let mut verified=None;
        let target=match s.scalar(if pg{"SELECT pg_backend_pid()::text"}else{"SELECT CAST(CONNECTION_ID() AS CHAR)"}).await{Ok(n)=>n,Err(e)=>{let _=s.release(true).await;return(Err(AppError::from(e)),false)}};
        let (result,_)=execution::drive(async{let r=apply(&mut s,&working.binding,&working,&work,&mut receipts).await?;verified=Some(r);Ok(MultiResult{sets:vec![],messages:vec![],elapsed_ms:0,spool_id:None})},&working,||async{
            match db.as_ref(){Db::Postgres(p)=>{let mut c=sqlx::PgConnection::connect_with(&p.connect_options()).await?;let r=crate::sql_history::query_scalar::<_,bool>("SELECT pg_cancel_backend($1)").bind(target.parse::<i32>()?).fetch_one(&mut c).await;let close=c.close().await;ensure!(r?,"TABLE_CANCEL_NOT_ACKNOWLEDGED");close?;},Db::Mysql(p)=>{let id:u64=target.parse()?;let mut c=sqlx::MySqlConnection::connect_with(&p.connect_options()).await?;let r=crate::sql_history::query(&format!("KILL QUERY {id}")).execute(&mut c).await;let close=c.close().await;r?;close?;},_=>unreachable!()}Ok(())
        }).await;
        let mut uncertain=result.as_ref().err().is_some_and(is_unknown);let mut result=result.map_err(AppError::from);let mut transaction=if pg{"rolled_back"}else{"implicit"};let stopped=working.stopping();
        if pg{if result.is_ok()&&!stopped{match s.execute("COMMIT").await{Ok(_)=>{transaction="committed";working.record_remote_transaction("committed");},Err(e)=>{uncertain=true;result=Err(unknown(format!("TABLE_COMMIT_ACK_UNKNOWN: {e:#}")));}}}else if !uncertain{if let Err(e)=s.execute("ROLLBACK").await{uncertain=true;result=Err(unknown(format!("TABLE_ROLLBACK_ACK_UNKNOWN: {e:#}")));}else{working.record_remote_transaction("rolled_back");}}}
        else{working.record_remote_transaction(mysql_remote_state(result.is_ok(),receipts.iter().any(|r|r.status=="applied"),uncertain,stopped));if let Err(e)=s.execute("UNLOCK TABLES").await{uncertain=true;result=Err(unknown(format!("TABLE_UNLOCK_UNKNOWN: {e:#}")));}let table=match &work{TableWork::Design(d)=>&d.table,TableWork::Operation(o)=>&o.table};let lock=plan::lock_name(&working.binding,table);match s.scalar(&format!("SELECT RELEASE_LOCK({})",db::quote_literal(&lock))).await{Ok(v)if v=="1"=>{},Ok(_)=>{uncertain=true;result=Err(unknown("TABLE_RELEASE_LOCK_NOT_CONFIRMED"));},Err(e)=>{uncertain=true;result=Err(unknown(format!("TABLE_RELEASE_LOCK_UNKNOWN: {e:#}")));}}}
        let partial=!pg&&!receipts.is_empty()&&result.is_err();if partial&&!uncertain{result=Err(AppError::coded("TABLE_IMPLICIT_PARTIAL_UNVERIFIED",format!("MySQL DDL 已隐式提交 {} 步，未验证完整目标，保留草稿；{}",receipts.iter().filter(|r|r.status=="applied").count(),result.as_ref().unwrap_err().message)));}
        if uncertain{result=Err(unknown(format!("TABLE_NATIVE_EFFECTS_UNKNOWN: {}",result.as_ref().err().map(|e|e.message.as_str()).unwrap_or("连接效果无法确认"))));}
        if let Err(e)=s.release(uncertain||stopped||pg&&result.is_err()).await{result=Err(unknown(format!("TABLE_CONNECTION_CLOSE_UNKNOWN: {e:#}")));}
        if result.is_ok()&&!stopped{if let Some((after,affected,copied))=verified{let public=after.public;match &work{TableWork::Design(_)=>output.lock().0=Some(TableDesignReceipt{receipt:DesignReceipt{steps:receipts,verified:true,unverified:false,mismatches:vec![],transaction:transaction.into(),schema:Some(public.schema.clone())},snapshot:Some(public)}),TableWork::Operation(o)=>output.lock().1=Some(TableOperationReceipt{kind:o.kind.clone(),source_table:o.table.clone(),target_table:o.target_table.clone(),affected_rows:affected,copied_rows:copied,verified:true,mismatches:vec![],transaction:transaction.into(),steps:receipts,snapshot:Some(public)})};}}
        if stopped&&result.is_ok(){result=Err(unknown("TABLE_STOP_COMMIT_RACE: 已提交或隐式提交，不能宣称取消回滚"));}
        if let Ok(r)=&mut result{r.elapsed_ms=started.elapsed().as_millis() as u64;r.messages.push(format!("表定义、约束、触发器及数据已在固定会话读回；{transaction}"));}
        let confirmed=stopped&&pg&&!uncertain&&transaction=="rolled_back";(result,confirmed)
    }).await;
    let (design,operation)=if result.is_ok()&&!entry.stopping(){published.lock().clone()}else{(None,None)};BoundResult{result,confirmed,design,operation}

    }).await
}

#[cfg(test)]
#[path="table_workflows_native_tests.rs"] mod tests;
