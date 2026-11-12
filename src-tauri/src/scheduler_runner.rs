//! Owned scheduler workers. SQL/tool futures and their leases remain owned until actual settlement.
use super::{clock::*,contracts::*,offline::HeadlessScheduleRequest,store::SchedulerStore};
use anyhow::{anyhow,ensure,Result};
use async_trait::async_trait;
use std::{collections::BTreeMap,path::PathBuf,sync::{Arc,Mutex},time::Duration};
use tokio::{sync::{oneshot,watch},task::JoinHandle,time::Instant};

#[derive(Debug,Clone,Copy,PartialEq,Eq)]
pub enum SchedulerStopReason {CancelRequested,Deadline,LeaseLost,Shutdown}
#[derive(Clone)]
pub struct ActionExecutionContext {
    pub run_id:String, pub action_id:String, pub owner_id:String, pub instance:SchedulerOwner,
    pub timeout_ms:u64, pub origin:&'static str, pub mode:RunnerMode,
    control:watch::Receiver<Option<SchedulerStopReason>>,
}
impl ActionExecutionContext {
    pub fn stop_reason(&self)->Option<SchedulerStopReason>{*self.control.borrow()}
    pub fn subscribe_stop(&self)->watch::Receiver<Option<SchedulerStopReason>>{self.control.clone()}
    /// Scope resolution only: this context must never authorize execution or be used to issue a SQL grant.
    pub fn for_review(owner:&SchedulerOwner,plan_id:&str,action_id:&str,timeout_ms:u64)->Self {
        let (_,control)=watch::channel(None);
        Self {run_id:plan_id.into(),action_id:action_id.into(),owner_id:format!("review:{plan_id}/{action_id}"),instance:owner.clone(),timeout_ms,origin:"job",mode:owner.mode,control}
    }
}
/// resolve_scope uses the real endpoint, permissions, file digests and tool preview, and sends no user writes.
/// execute atomically rechecks this scope immediately before dispatch through existing C1/tool services.
/// It listens to subscribe_stop, requests real cancellation, never drops a sent future, and reports exact effects/drain.
/// No retries, shell engine, implicit permission, plaintext-secret fallback or compensation is allowed.
#[async_trait]
pub trait SchedulerExecutor:Send+Sync+'static {
    async fn resolve_scope(&self,template:&ActionTemplate,context:&ActionExecutionContext)->Result<ActionScope>;
    async fn execute(&self,template:ActionTemplate,scope:ActionScope,context:ActionExecutionContext)->Result<ActionReceipt>;
    async fn resources_drained(&self,_context:&ActionExecutionContext,_receipt:&ActionReceipt)->Result<bool>{Ok(false)}
}
type RunTasks=Arc<Mutex<BTreeMap<String,JoinHandle<Result<ScheduledRun>>>>>;
struct IntervalEpoch {revision:String,wall:chrono::DateTime<chrono::Utc>,mono:u64}
#[derive(Clone)]
pub struct SchedulerRunner {
    pub store:SchedulerStore, pub owner:SchedulerOwner,
    clock:Arc<dyn SchedulerClock>, executor:Arc<dyn SchedulerExecutor>,
    headless:Option<(HeadlessScheduleRequest,PathBuf)>, tasks:RunTasks, cancel_grace:Duration,
    interval_epochs:Arc<Mutex<BTreeMap<String,IntervalEpoch>>>,
}
impl SchedulerRunner {
    pub fn new(store:SchedulerStore,owner:SchedulerOwner,clock:Arc<dyn SchedulerClock>,executor:Arc<dyn SchedulerExecutor>)->Self {
        Self {store,owner,clock,executor,headless:None,tasks:Arc::new(Mutex::new(BTreeMap::new())),cancel_grace:Duration::from_secs(10),interval_epochs:Arc::new(Mutex::new(BTreeMap::new()))}
    }
    #[cfg(test)] pub(crate) fn with_cancel_grace(mut self, grace:Duration)->Self {self.cancel_grace=grace;self}
    pub fn with_headless_request(mut self,request:HeadlessScheduleRequest,executable:PathBuf)->Result<Self> {
        ensure!(self.owner.mode==RunnerMode::Headless,"SCHEDULE_RUNNER_MODE");self.store.validate_headless_request(&request,&executable)?;self.headless=Some((request,executable));Ok(self)
    }
    pub fn claim_due(&self)->Result<Vec<ClaimedRun>> {
        let only_job=if self.owner.mode==RunnerMode::Headless {
            let (request,exe)=self.headless.as_ref().ok_or_else(||anyhow!("SCHEDULE_HEADLESS_ARGUMENTS: headless 必须验证精确任务版本"))?;
            self.store.validate_headless_request(request,exe)?;Some(request.job_id.as_str())
        }else{None};
        let now=self.clock.now_utc();let mono=self.clock.monotonic_millis();
        let records=self.store.list_jobs()?;
        let mut epochs=self.interval_epochs.lock().map_err(|_|anyhow!("SCHEDULE_CLOCK_UNAVAILABLE"))?;
        epochs.retain(|id,_|records.iter().any(|r|&r.job.id==id && r.job.enabled && matches!(&r.job.schedule,ScheduleSpec::Interval {..})));
        let mut cutoffs=BTreeMap::new();
        for record in records.iter().filter(|r|r.job.enabled && matches!(&r.job.schedule,ScheduleSpec::Interval {..})) {
            let epoch=epochs.entry(record.job.id.clone()).or_insert_with(||IntervalEpoch {revision:record.schedule_revision.clone(),wall:now,mono});
            if epoch.revision!=record.schedule_revision {*epoch=IntervalEpoch {revision:record.schedule_revision.clone(),wall:now,mono};}
            ensure!(mono>=epoch.mono,"SCHEDULE_MONOTONIC_REWIND");
            let elapsed=i64::try_from(mono-epoch.mono).map_err(|_|anyhow!("SCHEDULE_CLOCK_RANGE"))?;
            let cutoff=epoch.wall.checked_add_signed(chrono::Duration::milliseconds(elapsed)).ok_or_else(||anyhow!("SCHEDULE_CLOCK_RANGE"))?;
            cutoffs.insert(record.job.id.clone(),(record.schedule_revision.clone(),cutoff));
        }
        drop(epochs);
        self.store.claim_due_clocked(&self.owner,now,only_job,Some(&cutoffs))
    }
    /// All manual and timer workers share the same owned task registry.
    pub fn spawn_claimed(self:Arc<Self>,claim:ClaimedRun)->Result<()> {
        let _receiver=self.spawn_owned(claim)?;Ok(())
    }
    fn spawn_owned(self:Arc<Self>,claim:ClaimedRun)->Result<oneshot::Receiver<Result<ScheduledRun>>> {
        let mut tasks=self.tasks.lock().map_err(|_|anyhow!("SCHEDULE_SERVICE_UNAVAILABLE"))?;
        ensure!(!tasks.contains_key(&claim.run.id),"SCHEDULE_RUN_DUPLICATE");
        let id=claim.run.id.clone();let child=self.clone();let (send,receive)=oneshot::channel();
        tasks.insert(id,crate::sql_history::spawn(async move {
            let result=child.run_owned_claim(claim).await;
            let response=result.as_ref().map(Clone::clone).map_err(|error|anyhow!("{error:#}"));
            let _=send.send(response);result
        }));Ok(receive)
    }
    /// Dropping an IPC caller drops only its receiver, never the lease heartbeat or action monitor.
    pub async fn run_claimed(self:Arc<Self>,claim:ClaimedRun)->Result<ScheduledRun> {
        self.spawn_owned(claim)?.await.map_err(|_|anyhow!("SCHEDULE_RUN_ACK_UNKNOWN"))?
    }
    async fn run_owned_claim(self:Arc<Self>,claim:ClaimedRun)->Result<ScheduledRun> {
        let run=claim.run;let token=run.lease.token.clone();
        let started=match self.store.start_run(&run.id,&self.owner,&token,self.clock.now_utc()) {
            Ok(run)=>run,Err(error)=>{let _=self.store.complete_run(&run.id,&self.owner,&token,RunStatus::Failed,self.clock.now_utc());return Err(error)}
        };
        let order=validate_job(&started.job_snapshot)?;let started_mono=self.clock.monotonic_millis();let real_started=Instant::now();
        let mut terminal=RunStatus::Succeeded;let mut committed=false;
        for id in order {
            let action=started.job_snapshot.actions.iter().find(|a|a.id==id).cloned().ok_or_else(||anyhow!("SCHEDULE_ACTION_MISSING"))?;
            let spent=self.clock.monotonic_millis().saturating_sub(started_mono).max(real_started.elapsed().as_millis().min(u64::MAX as u128) as u64);
            if spent>=started.job_snapshot.timeout_ms {terminal=RunStatus::TimedOut;break;}
            match self.store.heartbeat(&run.id,&self.owner,&token,self.clock.now_utc()) {
                Ok(true)=>{terminal=RunStatus::Cancelled;break;},Ok(false)=>{},Err(error)=>{let _=self.store.record_uncertain(&run.id,&self.owner,&token,false,"SCHEDULE_LEASE_LOST",self.clock.now_utc());return Err(error)}
            }
            let (stop_tx,stop_rx)=watch::channel(None);
            let context=ActionExecutionContext {run_id:run.id.clone(),action_id:id.clone(),owner_id:format!("job:{}/{}/{}",run.job_id,run.id,id),instance:self.owner.clone(),timeout_ms:started.job_snapshot.timeout_ms-spent,origin:"job",mode:self.owner.mode,control:stop_rx};
            let child=self.clone();let worker_run=started.clone();let worker_context=context.clone();let worker_token=token.clone();
            let worker=crate::sql_history::spawn(async move {child.perform_action(worker_run,action,worker_token,worker_context).await});
            let monitored=self.monitor_action(worker,stop_tx,context.clone(),&token).await;
            let receipt=monitored.receipt;
            // Start and receipt persistence are separate: the worker never rewrites a watchdog Unknown.
            let saved=self.store.finish_action(&run.id,&id,&self.owner,&token,receipt.clone(),self.clock.now_utc());
            let needs_reap=monitored.reaper.is_some() || receipt.resources_pending;
            if needs_reap {
                self.clone().reap(&run.id,monitored.reaper,context.clone(),receipt.clone());
            }
            if let Err(error)=saved {
                if !needs_reap {self.clone().reap(&run.id,None,context.clone(),receipt.clone());}
                let _=self.store.record_uncertain(&run.id,&self.owner,&token,receipt.resources_pending,"SCHEDULE_JOURNAL_WRITE_UNKNOWN",self.clock.now_utc());
                return Err(error);
            }
            committed|=receipt.effects==ActionEffects::Committed;
            terminal=classify_receipt(&receipt);
            if terminal!=RunStatus::Succeeded {break;}
        }
        if committed && !matches!(terminal,RunStatus::Succeeded|RunStatus::Unknown){terminal=RunStatus::Partial;}
        match self.store.complete_run(&run.id,&self.owner,&token,terminal,self.clock.now_utc()) {
            Ok(outcome)=>Ok(outcome),
            Err(error)=>{let _=self.store.record_uncertain(&run.id,&self.owner,&token,false,"SCHEDULE_COMPLETION_UNCONFIRMED",self.clock.now_utc());Err(error)}
        }
    }
    async fn perform_action(&self,run:ScheduledRun,action:ScheduledAction,token:String,context:ActionExecutionContext)->Result<ActionReceipt> {
        let template=match self.store.load_template(&action.template_ref){Ok(value)=>value,Err(_)=>return Ok(confirmation("SCHEDULE_TEMPLATE_UNAVAILABLE"))};
        let Some(expected)=run.approvals.get(&action.id) else {return Ok(confirmation("SCHEDULE_AWAITING_APPROVAL"))};
        if expected.scope.template_revision!=template.revision || expected.scope.template_digest!=digest(&template)? || expected.scope_revision!=action.approved_scope_revision {return Ok(confirmation("SCHEDULE_TEMPLATE_CHANGED"));}
        let scope=match self.executor.resolve_scope(&template,&context).await{Ok(value)=>value,Err(_)=>return Ok(confirmation("SCHEDULE_PREFLIGHT_REJECTED"))};
        if super::service::ensure_scope_matches_template(&scope,&template).is_err() || scope!=expected.scope || scope.revision()?!=expected.scope_revision {return Ok(confirmation("SCHEDULE_SCOPE_CHANGED"));}
        if let Some(reason)=context.stop_reason(){return Ok(stopped_before_send(reason));}
        match self.store.start_action(&run.id,&action.id,&self.owner,&token,self.clock.now_utc()) {
            Ok(true)=>{},Ok(false)=>return Ok(stopped_before_send(SchedulerStopReason::CancelRequested)),Err(_)=>return Ok(confirmation("SCHEDULE_DISPATCH_REJECTED"))
        }
        // Cancellation may have arrived while fsync was in progress. No new send after the stop flag.
        if let Some(reason)=context.stop_reason(){return Ok(stopped_before_send(reason));}
        Ok(match self.executor.execute(template,scope,context).await {
            Ok(receipt) if receipt.validate().is_ok()=>receipt,_=>unknown("SCHEDULE_EXECUTOR_ACK_UNKNOWN")
        })
    }
    async fn monitor_action(&self,mut worker:JoinHandle<Result<ActionReceipt>>,stop:watch::Sender<Option<SchedulerStopReason>>,context:ActionExecutionContext,token:&str)->MonitoredAction {
        let started=Instant::now();let started_mono=self.clock.monotonic_millis();
        let mut polling=tokio::time::interval(Duration::from_millis(100));polling.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut stopping_at:Option<Instant>=None;let mut heartbeat_at=Instant::now();
        loop {tokio::select!{
            biased;
            result=&mut worker=>{let receipt=match result{Ok(Ok(receipt)) if receipt.validate().is_ok()=>receipt,_=>unknown("SCHEDULE_EXECUTOR_ACK_UNKNOWN")};return MonitoredAction {receipt,reaper:None}},
            _=polling.tick()=>{
                let elapsed=self.clock.monotonic_millis().saturating_sub(started_mono).max(started.elapsed().as_millis().min(u64::MAX as u128) as u64);
                if elapsed>=context.timeout_ms && stop.borrow().is_none(){stop.send_replace(Some(SchedulerStopReason::Deadline));stopping_at=Some(Instant::now());}
                if heartbeat_at.elapsed()>=Duration::from_secs(1) {
                    let reason=match self.store.heartbeat(&context.run_id,&self.owner,token,self.clock.now_utc()) {Ok(true)=>Some(SchedulerStopReason::CancelRequested),Ok(false)=>None,Err(_)=>Some(SchedulerStopReason::LeaseLost)};
                    if let Some(reason)=reason {if stop.borrow().is_none(){stop.send_replace(Some(reason));stopping_at=Some(Instant::now());}}
                    heartbeat_at=Instant::now();
                }
                if stopping_at.is_some_and(|at|at.elapsed()>=self.cancel_grace) {return MonitoredAction {receipt:unknown("SCHEDULE_STOP_UNCONFIRMED"),reaper:Some(worker)};}
            }
        }}
    }
    fn reap(self:Arc<Self>,run_id:&str,worker:Option<JoinHandle<Result<ActionReceipt>>>,context:ActionExecutionContext,receipt:ActionReceipt) {
        let run_id=run_id.to_string();
        // Spawned owner keeps the JoinHandle and all underlying SQL/tool leases, even if IPC/UI disappears.
        crate::sql_history::spawn(async move {
            let actual=if let Some(worker)=worker {match worker.await{Ok(Ok(r)) if r.validate().is_ok()=>r,_=>return}}else{receipt};
            loop {
                let drained=if !actual.resources_pending {true}else{
                    let executor=self.executor.clone();let ctx=context.clone();let r=actual.clone();
                    let check=crate::sql_history::spawn(async move {executor.resources_drained(&ctx,&r).await});
                    match check.await {Ok(Ok(value))=>value,_=>false}
                };
                if drained {
                    // A late result cannot alter terminal state. Retry only the local bookkeeping write.
                    let evidence=digest(&actual).unwrap_or_default();
                    if self.store.mark_drained(&run_id,&evidence,self.clock.now_utc()).is_ok(){break;}
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
    }
    pub fn start(self:Arc<Self>)->SchedulerServiceHandle {
        let (shutdown,mut receiver)=watch::channel(false);let jobs=self.tasks.clone();let runner=self.clone();
        let task=crate::sql_history::spawn(async move {
            let mut tick=tokio::time::interval(Duration::from_millis(250));tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {tokio::select!{
                changed=receiver.changed()=>{if changed.is_err() || *receiver.borrow(){break;}},
                _=tick.tick()=>{
                    if let Ok(claims)=runner.claim_due(){for claim in claims{let _=runner.clone().spawn_claimed(claim);}}
                    let finished:Vec<_>=runner.tasks.lock().map(|m|m.iter().filter(|(_,h)|h.is_finished()).map(|(id,_)|id.clone()).collect()).unwrap_or_default();
                    for id in finished {let handle=runner.tasks.lock().ok().and_then(|mut m|m.remove(&id));if let Some(handle)=handle{let _=handle.await;}}
                }
            }}
            let ids:Vec<_>=runner.tasks.lock().map(|m|m.keys().cloned().collect()).unwrap_or_default();
            for id in ids {let _=runner.store.request_cancel(&id,runner.clock.now_utc());}
            let handles=runner.tasks.lock().map(|mut m|std::mem::take(&mut *m)).unwrap_or_default();
            for (_,handle) in handles {let _=handle.await;}
        });SchedulerServiceHandle {shutdown,task:Some(task),jobs}
    }
}
struct MonitoredAction {receipt:ActionReceipt,reaper:Option<JoinHandle<Result<ActionReceipt>>>}
fn classify_receipt(receipt:&ActionReceipt)->RunStatus {
    if receipt.resources_pending || !receipt.verified || matches!(receipt.effects,ActionEffects::Possible|ActionEffects::Unknown) || receipt.status==ActionStatus::Unknown {return RunStatus::Unknown;}
    match receipt.status {ActionStatus::Succeeded=>RunStatus::Succeeded,ActionStatus::AwaitingConfirmation=>RunStatus::AwaitingConfirmation,ActionStatus::Partial=>RunStatus::Partial,ActionStatus::Cancelled=>RunStatus::Cancelled,ActionStatus::TimedOut=>RunStatus::TimedOut,_=>RunStatus::Failed}
}
fn unknown(code:&str)->ActionReceipt {ActionReceipt {status:ActionStatus::Unknown,verified:false,effects:ActionEffects::Unknown,resources_pending:true,execution_id:None,affected_rows:None,error_code:Some(safe_code(code))}}
fn confirmation(code:&str)->ActionReceipt {ActionReceipt {status:ActionStatus::AwaitingConfirmation,verified:true,effects:ActionEffects::None,resources_pending:false,execution_id:None,affected_rows:None,error_code:Some(safe_code(code))}}
fn stopped_before_send(reason:SchedulerStopReason)->ActionReceipt {ActionReceipt {status:if reason==SchedulerStopReason::Deadline{ActionStatus::TimedOut}else{ActionStatus::Cancelled},verified:true,effects:ActionEffects::None,resources_pending:false,execution_id:None,affected_rows:Some("0".into()),error_code:Some("SCHEDULE_STOPPED_BEFORE_SEND".into())}}
pub struct SchedulerServiceHandle {shutdown:watch::Sender<bool>,task:Option<JoinHandle<()>>,jobs:RunTasks}
impl SchedulerServiceHandle {
    pub fn request_stop(&self){self.shutdown.send_replace(true);}
    pub fn active_runs(&self)->usize{self.jobs.lock().map(|m|m.len()).unwrap_or(0)}
    pub async fn shutdown(mut self){self.request_stop();if let Some(task)=self.task.take(){let _=task.await;}}
}
impl Drop for SchedulerServiceHandle {
    fn drop(&mut self){self.request_stop();}
}
