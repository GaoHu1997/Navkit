//! Real-store and owned-runner regressions. These use the app's AES-GCM helpers and OS lock.
use super::*;
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime,Duration,Utc};
use std::{path::PathBuf,sync::{Arc,Mutex,atomic::{AtomicBool,AtomicUsize,Ordering}},time::Duration as StdDuration};
use tokio::sync::Semaphore;

fn at(text:&str)->DateTime<Utc> {DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)}
struct Fixture {root:PathBuf,store:SchedulerStore}
impl Fixture {
    fn new()->Self {
        let root=std::env::temp_dir().join(format!("navkit-scheduler-test-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();std::fs::write(root.join("key.bin"),[7u8;32]).unwrap();
        let store=SchedulerStore::new(root.clone());Self {root,store}
    }
    fn template(&self,connection:&str)->ActionTemplate {
        self.store.save_template(ActionTemplate {id:uuid::Uuid::new_v4().to_string(),revision:String::new(),name:"real SQL template".into(),kind:ActionKind::Sql,connection_id:connection.into(),namespace:" main ".into(),read_only:false,configuration:TemplateConfiguration::Sql {sql:"INSERT INTO items(value) VALUES ('private fixture value')".into()}},None).unwrap()
    }
    fn job(&self,template:&ActionTemplate,now:DateTime<Utc>)->JobRecord {
        self.store.save_job(ScheduledJobV2 {version:2,id:uuid::Uuid::new_v4().to_string(),revision:String::new(),name:"job".into(),enabled:false,execution_mode:ExecutionMode::AppOnline,schedule:ScheduleSpec::Manual,misfire:MisfirePolicy::LatestOnce,overlap:OverlapPolicy::Skip,timeout_ms:5000,actions:vec![ScheduledAction{id:uuid::Uuid::new_v4().to_string(),kind:template.kind,depends_on:vec![],template_ref:template.id.clone(),approved_scope_revision:String::new()}]},None,now).unwrap()
    }
    fn approve(&self,record:&JobRecord,now:DateTime<Utc>)->JobRecord {
        let scopes=record.job.actions.iter().map(|a|{let t=self.store.load_template(&a.template_ref).unwrap();(a.id.clone(),scope(&t,0))}).collect();
        self.store.approve_job(&record.job.id,&record.job.revision,scopes,now).unwrap()
    }
}
impl Drop for Fixture {fn drop(&mut self){let _=std::fs::remove_dir_all(&self.root);}}
fn scope(t:&ActionTemplate,epoch:usize)->ActionScope {
    ActionScope {template_ref:t.id.clone(),template_revision:t.revision.clone(),template_digest:digest(t).unwrap(),connection_id:t.connection_id.clone(),connection_ids:vec![t.connection_id.clone(),format!("source-for-{}",t.connection_id)],endpoint_fingerprint:"a".repeat(64),namespace:t.namespace.clone(),read_only:t.read_only,permission_revision:digest(&epoch).unwrap(),destructive_limit:None}
}
fn success()->ActionReceipt {ActionReceipt {status:ActionStatus::Succeeded,verified:true,effects:ActionEffects::Committed,resources_pending:false,execution_id:Some(uuid::Uuid::new_v4().to_string()),affected_rows:Some("1".into()),error_code:None}}
struct Executor {epoch:AtomicUsize,resolves:AtomicUsize,calls:Mutex<Vec<String>>,fail_at:AtomicUsize,block:AtomicBool,finished:AtomicBool,dropped:AtomicBool,release:Semaphore}
impl Default for Executor {
    fn default() -> Self { Self { epoch:AtomicUsize::new(0),resolves:AtomicUsize::new(0),calls:Mutex::new(Vec::new()),fail_at:AtomicUsize::new(0),block:AtomicBool::new(false),finished:AtomicBool::new(false),dropped:AtomicBool::new(false),release:Semaphore::new(0) } }
}
struct DropProbe<'a> {flag:&'a AtomicBool,finished:bool}
impl Drop for DropProbe<'_> {fn drop(&mut self){if !self.finished{self.flag.store(true,Ordering::SeqCst);}}}
#[async_trait]
impl SchedulerExecutor for Executor {
    async fn resolve_scope(&self,t:&ActionTemplate,_:&ActionExecutionContext)->Result<ActionScope> {self.resolves.fetch_add(1,Ordering::SeqCst);Ok(scope(t,self.epoch.load(Ordering::SeqCst)))}
    async fn execute(&self,t:ActionTemplate,_:ActionScope,context:ActionExecutionContext)->Result<ActionReceipt> {
        assert_eq!(context.origin,"job");assert!(context.owner_id.starts_with("job:"));
        let mut probe=DropProbe {flag:&self.dropped,finished:false};
        let count={let mut calls=self.calls.lock().unwrap();calls.push(t.id);calls.len()};
        if self.block.load(Ordering::SeqCst) {let permit=self.release.acquire().await.unwrap();permit.forget();}
        probe.finished=true;self.finished.store(true,Ordering::SeqCst);
        Ok(if self.fail_at.load(Ordering::SeqCst)==count {ActionReceipt::failed("TEST_UPSTREAM_FAILED")}else{success()})
    }
    async fn resources_drained(&self,_:&ActionExecutionContext,_:&ActionReceipt)->Result<bool> {Ok(self.finished.load(Ordering::SeqCst))}
}

#[test]
fn real_aesgcm_store_cas_and_template_changes_invalidate_approval() {
    let f=Fixture::new();let now=at("2026-01-01T00:00:00Z");let t=f.template("target");let r=f.approve(&f.job(&t,now),now);
    let encrypted=std::fs::read(f.root.join("scheduler/state.enc")).unwrap();
    assert!(!String::from_utf8_lossy(&encrypted).contains("private fixture value"));
    let mut changed=r.job.clone();changed.name="new name".into();let saved=f.store.save_job(changed.clone(),Some(&r.job.revision),now).unwrap();
    assert!(!saved.approvals.is_empty());assert!(f.store.save_job(changed,Some(&r.job.revision),now).is_err());
    let mut edited=t.clone();edited.configuration=TemplateConfiguration::Sql {sql:"SELECT 2".into()};f.store.save_template(edited,Some(&t.revision)).unwrap();
    assert_eq!(f.store.list_jobs().unwrap()[0].paused_reason.as_deref(),Some("SCHEDULE_TEMPLATE_CHANGED"));
}
#[test]
fn schedule_or_offline_mode_change_requires_fresh_approval() {
    let f=Fixture::new();let now=Utc::now();let t=f.template("target");let r=f.approve(&f.job(&t,now),now);
    let mut changed=r.job.clone();changed.execution_mode=ExecutionMode::OfflineOptIn;
    let saved=f.store.save_job(changed,Some(&r.job.revision),now).unwrap();assert!(saved.approvals.is_empty());assert!(saved.paused_reason.is_some());
}
#[test]
fn state_corruption_rejects_template_write_and_preserves_existing_ciphertext() {
    let f=Fixture::new();let t=f.template("target");let path=f.root.join(format!("scheduler/template-{}.enc",t.id));let original=std::fs::read(&path).unwrap();
    std::fs::write(f.root.join("scheduler/state.enc"),b"broken state").unwrap();let mut changed=t.clone();changed.name="must not save".into();
    assert!(f.store.save_template(changed,Some(&t.revision)).is_err());assert_eq!(std::fs::read(path).unwrap(),original);
}
#[test]
fn missing_original_key_never_generates_a_replacement() {
    let f=Fixture::new();let t=f.template("target");std::fs::remove_file(f.root.join("key.bin")).unwrap();
    assert!(f.store.load_template(&t.id).is_err());assert!(!f.root.join("key.bin").exists());
}
#[test]
fn os_lock_and_cas_keep_one_of_two_concurrent_saves() {
    let f=Fixture::new();let now=Utc::now();let t=f.template("target");let record=f.job(&t,now);let barrier=Arc::new(std::sync::Barrier::new(2));
    let handles:Vec<_>=(0..2).map(|index|{let store=f.store.clone();let mut job=record.job.clone();job.name=format!("writer {index}");let expected=record.job.revision.clone();let barrier=barrier.clone();std::thread::spawn(move||{barrier.wait();store.save_job(job,Some(&expected),now).is_ok()})}).collect();
    let successes=handles.into_iter().map(|h|h.join().unwrap()).filter(|success|*success).count();assert_eq!(successes,1);assert_eq!(f.store.list_jobs().unwrap().len(),1);
}
#[test]
fn gui_and_headless_style_owners_cannot_claim_twice_or_share_secondary_connection() {
    let f=Fixture::new();let now=Utc::now();let t=f.template("target");let r=f.approve(&f.job(&t,now),now);let first=SchedulerOwner::new(RunnerMode::Gui);let second=SchedulerOwner::new(RunnerMode::Gui);
    let request=uuid::Uuid::new_v4().to_string();let claim=f.store.claim_manual(&r.job.id,&r.job.revision,&request,&first,now).unwrap().unwrap();
    assert!(f.store.claim_manual(&r.job.id,&r.job.revision,&request,&second,now).unwrap().is_none());
    let other=f.template("source-for-target");let other=f.approve(&f.job(&other,now),now);
    assert!(f.store.claim_manual(&other.job.id,&other.job.revision,&uuid::Uuid::new_v4().to_string(),&second,now).unwrap().is_none());
    assert_eq!(claim.run.connection_ids,vec!["source-for-target".to_string(),"target".to_string()]);
}
#[test]
fn expired_started_lease_is_unknown_and_never_automatically_replayed() {
    let f=Fixture::new();let now=Utc::now();let t=f.template("target");let r=f.approve(&f.job(&t,now),now);let owner=SchedulerOwner::new(RunnerMode::Gui);
    let claim=f.store.claim_manual(&r.job.id,&r.job.revision,&uuid::Uuid::new_v4().to_string(),&owner,now).unwrap().unwrap();
    f.store.start_run(&claim.run.id,&owner,&claim.run.lease.token,now).unwrap();f.store.start_action(&claim.run.id,&r.job.actions[0].id,&owner,&claim.run.lease.token,now).unwrap();
    f.store.recover(now+Duration::minutes(1)).unwrap();let state=f.store.snapshot().unwrap();assert_eq!(state.runs[0].status,RunStatus::Unknown);assert!(state.runs[0].resources_pending);
    assert!(f.store.claim_manual(&r.job.id,&r.job.revision,&uuid::Uuid::new_v4().to_string(),&owner,now+Duration::minutes(2)).is_err());
    assert!(f.store.claim_due(&owner,now+Duration::days(10)).unwrap().is_empty());
}
#[test]
fn manual_idempotency_survives_two_hundred_row_journal_retention() {
    let f=Fixture::new();let now=Utc::now();let t=f.template("target");let r=f.approve(&f.job(&t,now),now);let owner=SchedulerOwner::new(RunnerMode::Gui);let first=uuid::Uuid::new_v4().to_string();
    for i in 0..202 {let request=if i==0{first.clone()}else{uuid::Uuid::new_v4().to_string()};let claim=f.store.claim_manual(&r.job.id,&r.job.revision,&request,&owner,now).unwrap().unwrap();let token=&claim.run.lease.token;
        f.store.start_run(&claim.run.id,&owner,token,now).unwrap();f.store.start_action(&claim.run.id,&r.job.actions[0].id,&owner,token,now).unwrap();
        f.store.finish_action(&claim.run.id,&r.job.actions[0].id,&owner,token,success(),now).unwrap();f.store.complete_run(&claim.run.id,&owner,token,RunStatus::Succeeded,now).unwrap();}
    assert_eq!(f.store.history(None).unwrap().len(),200);assert!(f.store.claim_manual(&r.job.id,&r.job.revision,&first,&owner,now).unwrap().is_none());
}
#[tokio::test]
async fn approval_binds_window_re_resolves_scope_and_is_consumed_once() {
    let f=Fixture::new();let now=Utc::now();let t=f.template("target");let r=f.job(&t,now);let clock=Arc::new(ControlledSchedulerClock::new(now));let executor=Arc::new(Executor::default());
    let service=SchedulerService::new(f.store.clone(),SchedulerOwner::new(RunnerMode::Gui),clock,executor.clone());
    let preview=service.prepare_approval(&r.job.id,&r.job.revision,"main").await.unwrap();assert_eq!(executor.resolves.load(Ordering::SeqCst),1);
    assert!(service.approve_job(&preview.plan_id,&preview.review_digest,"other-window").await.is_err());
    let saved=service.approve_job(&preview.plan_id,&preview.review_digest,"main").await.unwrap();assert_eq!(executor.resolves.load(Ordering::SeqCst),2);assert!(saved.paused_reason.is_none());
    assert!(service.approve_job(&preview.plan_id,&preview.review_digest,"main").await.is_err());assert!(executor.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn changed_actual_scope_or_monotonic_review_expiry_cannot_approve() {
    let f=Fixture::new();let now=Utc::now();let t=f.template("target");let r=f.job(&t,now);let clock=Arc::new(ControlledSchedulerClock::new(now));let executor=Arc::new(Executor::default());
    let service=SchedulerService::new(f.store.clone(),SchedulerOwner::new(RunnerMode::Gui),clock.clone(),executor.clone());
    let preview=service.prepare_approval(&r.job.id,&r.job.revision,"main").await.unwrap();executor.epoch.store(1,Ordering::SeqCst);assert!(service.approve_job(&preview.plan_id,&preview.review_digest,"main").await.is_err());
    let preview=service.prepare_approval(&r.job.id,&r.job.revision,"main").await.unwrap();clock.advance(Duration::hours(-2),120_001);assert!(service.approve_job(&preview.plan_id,&preview.review_digest,"main").await.is_err());
    assert!(f.store.list_jobs().unwrap()[0].approvals.is_empty());
}
#[tokio::test]
async fn failed_upstream_blocks_dependents_and_records_partial_commit() {
    let f=Fixture::new();let now=Utc::now();let t=f.template("target");let r=f.job(&t,now);let mut job=r.job.clone();
    for _ in 0..2 {let previous=job.actions.last().unwrap().id.clone();job.actions.push(ScheduledAction {id:uuid::Uuid::new_v4().to_string(),kind:ActionKind::Sql,depends_on:vec![previous],template_ref:t.id.clone(),approved_scope_revision:String::new()});}
    let r=f.store.save_job(job,Some(&r.job.revision),now).unwrap();let r=f.approve(&r,now);let owner=SchedulerOwner::new(RunnerMode::Gui);let executor=Arc::new(Executor::default());executor.fail_at.store(2,Ordering::SeqCst);
    let claim=f.store.claim_manual(&r.job.id,&r.job.revision,&uuid::Uuid::new_v4().to_string(),&owner,now).unwrap().unwrap();
    let runner=Arc::new(SchedulerRunner::new(f.store.clone(),owner,Arc::new(SystemSchedulerClock::default()),executor.clone()));
    let run=tokio::time::timeout(StdDuration::from_secs(5),runner.run_claimed(claim)).await.unwrap().unwrap();
    assert_eq!(executor.calls.lock().unwrap().len(),2);assert_eq!(run.status,RunStatus::Partial);assert_eq!(run.actions[2].status,ActionStatus::Blocked);assert!(f.store.list_jobs().unwrap()[0].blocked_run_id.is_some());
    let public:SchedulerRunSummary=run.clone().into();let json=serde_json::to_string(&public).unwrap();assert!(!json.contains(&run.lease.token));assert!(!json.contains("private fixture value"));
}
#[tokio::test]
async fn timed_out_worker_is_retained_and_late_success_only_drains_unknown() {
    let f=Fixture::new();let now=Utc::now();let t=f.template("target");let r=f.job(&t,now);let mut job=r.job.clone();job.timeout_ms=1;
    let r=f.store.save_job(job,Some(&r.job.revision),now).unwrap();let r=f.approve(&r,now);let owner=SchedulerOwner::new(RunnerMode::Gui);let executor=Arc::new(Executor::default());executor.block.store(true,Ordering::SeqCst);
    let claim=f.store.claim_manual(&r.job.id,&r.job.revision,&uuid::Uuid::new_v4().to_string(),&owner,now).unwrap().unwrap();
    let runner=Arc::new(SchedulerRunner::new(f.store.clone(),owner.clone(),Arc::new(SystemSchedulerClock::default()),executor.clone()).with_cancel_grace(StdDuration::from_millis(20)));
    let run=tokio::time::timeout(StdDuration::from_secs(3),runner.run_claimed(claim)).await.unwrap().unwrap();
    assert_eq!(run.status,RunStatus::Unknown);assert!(run.resources_pending);assert!(!executor.dropped.load(Ordering::SeqCst));assert!(!executor.finished.load(Ordering::SeqCst));
    executor.release.add_permits(1);
    tokio::time::timeout(StdDuration::from_secs(3),async {loop {let state=f.store.snapshot().unwrap();if !state.runs[0].resources_pending {assert_eq!(state.runs[0].status,RunStatus::Unknown);break;}tokio::time::sleep(StdDuration::from_millis(10)).await;}}).await.unwrap();
    assert!(executor.finished.load(Ordering::SeqCst));assert!(!executor.dropped.load(Ordering::SeqCst));assert!(f.store.claim_due(&owner,Utc::now()+Duration::days(1)).unwrap().is_empty());
}

#[test]
fn live_interval_ignores_forward_and_backward_wall_clock_adjustment() {
    let f=Fixture::new();let now=at("2026-01-01T00:00:00Z");let t=f.template("interval-target");let record=f.job(&t,now);
    let mut job=record.job.clone();job.enabled=true;job.schedule=ScheduleSpec::Interval {seconds:60,anchor_utc:now};
    let record=f.store.save_job(job,Some(&record.job.revision),now).unwrap();let record=f.approve(&record,now);
    let clock=Arc::new(ControlledSchedulerClock::new(now));let runner=SchedulerRunner::new(f.store.clone(),SchedulerOwner::new(RunnerMode::Gui),clock.clone(),Arc::new(Executor::default()));
    assert!(runner.claim_due().unwrap().is_empty());
    clock.advance(Duration::hours(3),1000);assert!(runner.claim_due().unwrap().is_empty());
    assert_eq!(f.store.list_jobs().unwrap()[0].next_run_utc,Some(now+Duration::seconds(60)));
    clock.advance(Duration::hours(-5),59_000);let claims=runner.claim_due().unwrap();
    assert_eq!(claims.len(),1);assert_eq!(claims[0].run.job_id,record.job.id);assert_eq!(claims[0].run.scheduled_at_utc,now+Duration::seconds(60));
    assert_eq!(f.store.list_jobs().unwrap()[0].next_run_utc,Some(now+Duration::seconds(120)));
}

#[test]
fn calendar_jobs_still_follow_real_utc_when_interval_clock_is_pinned() {
    let f=Fixture::new();let now=at("2026-01-01T00:00:00Z");let t=f.template("calendar-target");let record=f.job(&t,now);let mut job=record.job.clone();job.enabled=true;
    job.schedule=ScheduleSpec::Calendar {cron:"0 * * * *".into(),timezone:"UTC".into(),dst_gap:DstGap::Skip,dst_fold:DstFold::First};
    let record=f.store.save_job(job,Some(&record.job.revision),now).unwrap();let record=f.approve(&record,now);
    let clock=Arc::new(ControlledSchedulerClock::new(now));let runner=SchedulerRunner::new(f.store.clone(),SchedulerOwner::new(RunnerMode::Gui),clock.clone(),Arc::new(Executor::default()));
    assert!(runner.claim_due().unwrap().is_empty());clock.advance(Duration::hours(3),1000);
    let claims=runner.claim_due().unwrap();assert_eq!(claims.len(),1);assert_eq!(claims[0].run.job_id,record.job.id);assert_eq!(claims[0].run.scheduled_at_utc,now+Duration::hours(3));
}

#[tokio::test]
async fn dropping_run_caller_keeps_owned_monitor_and_completion_journal() {
    let f=Fixture::new();let now=Utc::now();let t=f.template("target");let record=f.approve(&f.job(&t,now),now);let owner=SchedulerOwner::new(RunnerMode::Gui);
    let claim=f.store.claim_manual(&record.job.id,&record.job.revision,&uuid::Uuid::new_v4().to_string(),&owner,now).unwrap().unwrap();let run_id=claim.run.id.clone();
    let executor=Arc::new(Executor::default());executor.block.store(true,Ordering::SeqCst);
    let runner=Arc::new(SchedulerRunner::new(f.store.clone(),owner,Arc::new(SystemSchedulerClock::default()),executor.clone()));
    let caller=tokio::spawn(runner.clone().run_claimed(claim));
    tokio::time::timeout(StdDuration::from_secs(3),async {while executor.calls.lock().unwrap().is_empty(){tokio::time::sleep(StdDuration::from_millis(10)).await;}}).await.unwrap();
    caller.abort();assert!(caller.await.unwrap_err().is_cancelled());
    assert!(!executor.dropped.load(Ordering::SeqCst));assert_eq!(f.store.snapshot().unwrap().runs.iter().find(|r|r.id==run_id).unwrap().status,RunStatus::Running);
    executor.release.add_permits(1);
    tokio::time::timeout(StdDuration::from_secs(3),async {loop {let state=f.store.snapshot().unwrap();let run=state.runs.iter().find(|r|r.id==run_id).unwrap();if run.status.terminal(){assert_eq!(run.status,RunStatus::Succeeded);assert!(!run.resources_pending);break;}tokio::time::sleep(StdDuration::from_millis(10)).await;}}).await.unwrap();
    assert!(executor.finished.load(Ordering::SeqCst));assert!(!executor.dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn dropping_service_handle_stops_polling_and_releases_runner() {
    let f=Fixture::new();let runner=Arc::new(SchedulerRunner::new(f.store.clone(),SchedulerOwner::new(RunnerMode::Gui),Arc::new(SystemSchedulerClock::default()),Arc::new(Executor::default())));
    let weak=Arc::downgrade(&runner);let handle=runner.start();drop(handle);
    tokio::time::timeout(StdDuration::from_secs(3),async {while weak.upgrade().is_some(){tokio::time::sleep(StdDuration::from_millis(10)).await;}}).await.unwrap();
}
