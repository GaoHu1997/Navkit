//! 本机调度事务存储：单一内核文件锁保护 CAS、claim、lease 和 journal，密文原子替换。
//! 不静默修复损坏文件；失联执行永远不据 lease 到期自动重放。
use super::{clock::*, contracts::*};
use anyhow::{anyhow, ensure, Result};
use chrono::{DateTime, Duration, Utc};
use std::{collections::{BTreeMap, BTreeSet}, fs::{self, File, OpenOptions}, path::{Path, PathBuf}};

struct StoreLock { _file: File }
impl StoreLock {
    fn acquire(dir:&Path) -> Result<Self> {
        safe_directory(dir)?;
        let path=dir.join("scheduler.lock"); reject_link(&path)?;
        let mut options=OpenOptions::new(); options.read(true).write(true).create(true);
        #[cfg(windows)] {use std::os::windows::fs::OpenOptionsExt; options.share_mode(0);}
        let file=options.open(path).map_err(|_|anyhow!("SCHEDULE_LOCKED: 另一实例正在更新调度，原状态保留"))?;
        #[cfg(unix)] {use std::os::fd::AsRawFd; extern "C" {fn flock(fd:std::os::raw::c_int,op:std::os::raw::c_int)->std::os::raw::c_int;} ensure!(unsafe {flock(file.as_raw_fd(),2|4)}==0,"SCHEDULE_LOCKED: 另一实例正在更新调度");}
        #[cfg(not(any(windows,unix)))] anyhow::bail!("SCHEDULE_LOCK_UNSUPPORTED: 当前平台缺少可靠文件锁");
        Ok(Self {_file:file})
    }
}
fn reject_link(path:&Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta)=>{ensure!(!meta.file_type().is_symlink(),"SCHEDULE_PATH_INVALID: 调度路径不能是符号链接"); #[cfg(windows)] {use std::os::windows::fs::MetadataExt; ensure!(meta.file_attributes() & 0x400 == 0,"SCHEDULE_PATH_INVALID: 调度路径不能是重解析点");}},
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{}, Err(e)=>return Err(e.into()),
    }
    Ok(())
}
fn safe_directory(dir:&Path) -> Result<()> {reject_link(dir)?; fs::create_dir_all(dir)?; ensure!(dir.is_dir(),"SCHEDULE_PATH_INVALID: 调度目录无效"); Ok(())}
fn absent_file(error:&std::io::Error)->bool{error.kind()==std::io::ErrorKind::NotFound}

#[derive(Clone)]
pub struct SchedulerStore { root:PathBuf }
impl SchedulerStore {
    pub fn new(root:PathBuf) -> Self {Self {root}}
    fn dir(&self)->PathBuf{self.root.join("scheduler")}
    fn state_path(&self)->PathBuf{self.dir().join("state.enc")}
    fn template_path(&self,id:&str)->Result<PathBuf>{canonical_uuid(id)?;Ok(self.dir().join(format!("template-{id}.enc")))}
    fn ensure_key(&self)->Result<()> {
        let key=self.root.join("key.bin");reject_link(&self.root)?;reject_link(&key)?;
        let meta=fs::metadata(&key).map_err(|_|anyhow!("SCHEDULE_KEY_UNAVAILABLE: 必须先初始化或恢复应用原本机密钥；调度器不创建替代密钥"))?;
        ensure!(meta.is_file() && meta.len()==32,"SCHEDULE_KEY_INVALID: 本机密钥损坏，调度数据保持原状");Ok(())
    }
    fn read_encrypted<T:serde::de::DeserializeOwned>(&self,path:&Path,aad:&str,limit:usize)->Result<Option<T>> {
        reject_link(path)?;
        let encrypted=match crate::storage::read_bytes_limited(path,limit+28){Ok(b)=>b,Err(e) if absent_file(&e)=>{ensure!(!path.with_extension("enc.bak").exists(),"SCHEDULE_RECOVERY_REQUIRED: 主文件缺失但备份存在，拒绝当作空数据覆盖");return Ok(None)},Err(_)=>return Err(anyhow!("SCHEDULE_STORAGE_READ: 无法读取本机调度文件，未修改原状态"))};
        self.ensure_key()?;
        let plain=crate::storage::decrypt_bytes_at(&self.root,&encrypted,aad.as_bytes()).map_err(|_|anyhow!("SCHEDULE_DECRYPT_FAILED: 密文或本机密钥不匹配，原文件保留"))?;
        let value=serde_json::from_slice(&plain).map_err(|_|anyhow!("SCHEDULE_CORRUPT: 本机调度数据损坏或版本不支持，拒绝覆盖"))?;
        Ok(Some(value))
    }
    fn write_encrypted<T:serde::Serialize>(&self,path:&Path,aad:&str,value:&T,limit:usize)->Result<()> {
        self.ensure_key()?; reject_link(path)?;
        let plain=zeroize::Zeroizing::new(serde_json::to_vec(value)?);ensure!(plain.len()<=limit,"SCHEDULE_STORAGE_QUOTA: 调度持久化数据超过上限");
        let encrypted=crate::storage::encrypt_bytes_at(&self.root,&plain,aad.as_bytes())?;
        ensure!(crate::storage::decrypt_bytes_at(&self.root,&encrypted,aad.as_bytes())?.as_slice()==plain.as_slice(),"SCHEDULE_VERIFY_FAILED: 写入前加密校验失败");
        let old=match crate::storage::read_bytes_limited(path,limit+28){Ok(b)=>Some(b),Err(e) if absent_file(&e)=>None,Err(e)=>return Err(e.into())};
        if let Some(old)=&old {let bak=path.with_extension("enc.bak");reject_link(&bak)?;crate::storage::atomic_write(&bak,old)?;}
        crate::storage::atomic_write(path,&encrypted).map_err(|_|anyhow!("SCHEDULE_STORAGE_WRITE: 原子写入失败；未确认新状态，禁止执行动作"))?;
        let checked=crate::storage::read_bytes_limited(path,limit+28)?;
        ensure!(checked.as_slice()==encrypted.as_slice(),"SCHEDULE_VERIFY_FAILED: 写后读回不一致，保留备份并停止执行");Ok(())
    }
    fn read_unlocked(&self)->Result<SchedulerState> {
        let state:SchedulerState=self.read_encrypted(&self.state_path(),"navkit:scheduler:v2:state",MAX_STATE_BYTES)?.unwrap_or_default();
        ensure!(state.version==SCHEDULER_VERSION && state.consumed_manual_requests.len()<=16384 && state.jobs.len()<=MAX_JOBS && state.runs.len()<=MAX_HISTORY+MAX_JOBS,"SCHEDULE_CORRUPT: 版本/容量无效，拒绝当作空任务表");
        canonical_uuid(&state.revision)?;
        for (id,record) in &state.jobs {ensure!(*id==record.job.id && is_digest(&record.schedule_revision),"SCHEDULE_CORRUPT: 任务身份/版本损坏");validate_job(&record.job)?;ensure!(schedule_revision(&record.job)?==record.schedule_revision,"SCHEDULE_CORRUPT: 任务内容与计划版本不一致");}
        let mut ids=BTreeSet::new();for run in &state.runs {canonical_uuid(&run.id)?;ensure!(ids.insert(&run.id) && run.actions.len()<=MAX_ACTIONS && run.journal.len()<=MAX_JOURNAL_EVENTS,"SCHEDULE_CORRUPT: 运行日志身份/容量无效");}
        Ok(state)
    }
    fn transaction<T>(&self,operation:impl FnOnce(&mut SchedulerState)->Result<T>)->Result<T> {
        let _lock=StoreLock::acquire(&self.dir())?;let mut state=self.read_unlocked()?;
        let before=digest(&state)?; let out=operation(&mut state)?;trim_history(&mut state);
        if digest(&state)?!=before { state.revision=uuid::Uuid::new_v4().to_string();
            self.write_encrypted(&self.state_path(),"navkit:scheduler:v2:state",&state,MAX_STATE_BYTES)?;
        }
        Ok(out)
    }
    pub fn snapshot(&self)->Result<SchedulerState>{let _lock=StoreLock::acquire(&self.dir())?;self.read_unlocked()}
    pub fn list_jobs(&self)->Result<Vec<JobRecord>>{Ok(self.snapshot()?.jobs.into_values().collect())}
    pub fn history(&self,job_id:Option<&str>)->Result<Vec<ScheduledRun>>{let mut runs:Vec<_>=self.snapshot()?.runs.into_iter().filter(|r|job_id.map_or(true,|id|r.job_id==id)).collect();runs.sort_by_key(|r|std::cmp::Reverse(r.claimed_at_utc));runs.truncate(MAX_HISTORY);Ok(runs)}
    pub fn list_templates(&self)->Result<Vec<ActionTemplateSummary>> {
        let _lock=StoreLock::acquire(&self.dir())?; self.read_unlocked()?;
        let mut result=vec![];
        for entry in fs::read_dir(self.dir())? {
            let entry=entry?; let name=entry.file_name(); let name=name.to_string_lossy();
            if let Some(id)=name.strip_prefix("template-").and_then(|s|s.strip_suffix(".enc")) {
                ensure!(result.len()<MAX_TEMPLATES,"SCHEDULE_TEMPLATE_QUOTA");
                result.push(ActionTemplateSummary::from(self.template_unlocked(id)?));
            }
        }
        result.sort_by(|a,b|a.name.cmp(&b.name).then_with(||a.id.cmp(&b.id))); Ok(result)
    }
    pub fn load_template(&self,id:&str)->Result<ActionTemplate>{let _lock=StoreLock::acquire(&self.dir())?;self.template_unlocked(id)}
    fn template_unlocked(&self,id:&str)->Result<ActionTemplate>{let t:ActionTemplate=self.read_encrypted(&self.template_path(id)?,&format!("navkit:scheduler:v2:template:{id}"),MAX_TEMPLATE_BYTES)?.ok_or_else(||anyhow!("SCHEDULE_TEMPLATE_MISSING: 模板已删除或不可用"))?;validate_template(&t)?;ensure!(t.id==id && !t.revision.is_empty(),"SCHEDULE_TEMPLATE_INVALID: 模板文件身份不一致");Ok(t)}
    pub fn save_template(&self,mut template:ActionTemplate,expected_revision:Option<&str>)->Result<ActionTemplate> {
        validate_template(&template)?;let _lock=StoreLock::acquire(&self.dir())?;
        // 在写任何模板前读取并验证状态；损坏状态不能留下已更改的模板。
        let mut state=self.read_unlocked()?;
        let path=self.template_path(&template.id)?;let existing:Option<ActionTemplate>=self.read_encrypted(&path,&format!("navkit:scheduler:v2:template:{}",template.id),MAX_TEMPLATE_BYTES)?;
        ensure!(existing.as_ref().map(|t|t.revision.as_str())==expected_revision && template.revision==expected_revision.unwrap_or(""),"SCHEDULE_TEMPLATE_CONFLICT: 模板已被另一实例修改，拒绝覆盖");
        if existing.is_none(){let count=fs::read_dir(self.dir())?.filter_map(|e|e.ok()).filter(|e|{let n=e.file_name();let n=n.to_string_lossy();n.starts_with("template-")&&n.ends_with(".enc")}).count();ensure!(count<MAX_TEMPLATES,"SCHEDULE_TEMPLATE_QUOTA: 模板已达到上限");}
        template.revision=uuid::Uuid::new_v4().to_string();validate_template(&template)?;
        self.write_encrypted(&path,&format!("navkit:scheduler:v2:template:{}",template.id),&template,MAX_TEMPLATE_BYTES)?;
        // 若随后的状态写失败，执行仍须重新核对模板 revision；旧批准绝不会适用于新模板。
        for record in state.jobs.values_mut().filter(|r|r.job.actions.iter().any(|a|a.template_ref==template.id)){record.paused_reason=Some("SCHEDULE_TEMPLATE_CHANGED".into());record.queued=None;}
        state.revision=uuid::Uuid::new_v4().to_string();self.write_encrypted(&self.state_path(),"navkit:scheduler:v2:state",&state,MAX_STATE_BYTES)?;Ok(template)
    }
    pub fn save_job(&self,mut job:ScheduledJobV2,expected_revision:Option<&str>,now:DateTime<Utc>)->Result<JobRecord> {
        validate_job(&job)?;
        self.transaction(|state|{
            let old=state.jobs.get(&job.id).cloned();ensure!(old.as_ref().map(|r|r.job.revision.as_str())==expected_revision && job.revision==expected_revision.unwrap_or(""),"SCHEDULE_CONFLICT: 任务版本已经变化，未覆盖另一实例修改");
            ensure!(old.is_some() || state.jobs.len()<MAX_JOBS,"SCHEDULE_JOB_QUOTA: 本机任务已达上限");
            let safety_unchanged=old.as_ref().is_some_and(|r|schedule_revision(&job).ok().as_ref()==Some(&r.schedule_revision));
            let mut approvals=BTreeMap::new();
            for action in &mut job.actions {
                let template=self.template_unlocked(&action.template_ref)?;ensure!(template.kind==action.kind,"SCHEDULE_TEMPLATE_KIND: 动作与模板种类不一致");
                let retained=old.as_ref().filter(|_|safety_unchanged).and_then(|r|r.job.actions.iter().find(|a|a.id==action.id && a.kind==action.kind && a.template_ref==action.template_ref && a.depends_on==action.depends_on).and_then(|_|r.approvals.get(&action.id))).filter(|a|a.scope.template_revision==template.revision && a.scope.template_digest==digest(&template).unwrap_or_default());
                action.approved_scope_revision=retained.map(|a|a.scope_revision.clone()).unwrap_or_default();if let Some(a)=retained{approvals.insert(action.id.clone(),a.clone());}
            }
            let revision=schedule_revision(&job)?;let same=old.as_ref().is_some_and(|r|r.schedule_revision==revision);
            job.revision=uuid::Uuid::new_v4().to_string();
            let next=if !job.enabled {None}else if same {old.as_ref().and_then(|r|r.next_run_utc)}else{next_run_after(&job.schedule,now)?};
            let record=JobRecord {job:job.clone(),schedule_revision:revision,next_run_utc:next,queued:if same {old.as_ref().and_then(|r|r.queued.clone())}else{None},blocked_run_id:old.as_ref().and_then(|r|r.blocked_run_id.clone()),paused_reason:if approvals.len()!=job.actions.len(){Some("SCHEDULE_AWAITING_APPROVAL".into())}else{None},approvals,legacy:old.as_ref().and_then(|r|r.legacy.clone()),last_occurrence_utc:old.as_ref().and_then(|r|r.last_occurrence_utc)};
            state.jobs.insert(job.id.clone(),record.clone());state.offline_plans.remove(&job.id);Ok(record)
        })
    }
    /// 仅主进程在展示真实模板/目标/破坏性范围后调用；不要把 scopes 直接作为 IPC 客户端参数。
    pub fn approve_job(&self,job_id:&str,expected_revision:&str,scopes:Vec<(String,ActionScope)>,now:DateTime<Utc>)->Result<JobRecord> {
        self.transaction(|state|{
            let record=state.jobs.get_mut(job_id).ok_or_else(||anyhow!("SCHEDULE_JOB_MISSING"))?;ensure!(record.job.revision==expected_revision,"SCHEDULE_CONFLICT: 批准前任务版本已改变");ensure!(record.blocked_run_id.is_none(),"SCHEDULE_UNRESOLVED: 必须先核对未知执行");
            ensure!(scopes.len()==record.job.actions.len(),"SCHEDULE_SCOPE_INVALID: 必须核对每个动作");let mut approvals=BTreeMap::new();
            for (action_id,scope) in scopes {
                let action=record.job.actions.iter_mut().find(|a|a.id==action_id).ok_or_else(||anyhow!("SCHEDULE_SCOPE_INVALID: 动作身份不匹配"))?;
                let template=self.template_unlocked(&action.template_ref)?;scope.validate()?;
                ensure!(scope.template_ref==template.id && scope.template_revision==template.revision && scope.template_digest==digest(&template)? && scope.connection_id==template.connection_id && scope.namespace==template.namespace && scope.read_only==template.read_only,"SCHEDULE_SCOPE_INVALID: 批准范围与真实模板不一致");
                ensure!(!approvals.contains_key(&action_id),"SCHEDULE_SCOPE_INVALID: 重复动作批准");let rev=scope.revision()?;action.approved_scope_revision=rev.clone();approvals.insert(action_id.clone(),ScopeApproval {action_id,scope,scope_revision:rev,approved_at_utc:now});
            }
            record.approvals=approvals;record.paused_reason=None;record.job.revision=uuid::Uuid::new_v4().to_string();Ok(record.clone())
        })
    }
    pub fn pause_job(&self,id:&str,expected_revision:&str,now:DateTime<Utc>)->Result<JobRecord>{let state=self.snapshot()?;let mut job=state.jobs.get(id).ok_or_else(||anyhow!("SCHEDULE_JOB_MISSING"))?.job.clone();job.enabled=false;self.save_job(job,Some(expected_revision),now)}
    pub fn delete_job(&self,id:&str,expected_revision:&str)->Result<()> {self.transaction(|s|{let old=s.jobs.get(id).ok_or_else(||anyhow!("SCHEDULE_JOB_MISSING"))?;ensure!(old.job.revision==expected_revision,"SCHEDULE_CONFLICT");ensure!(!s.runs.iter().any(|r|r.job_id==id && (!r.status.terminal() || r.resources_pending)),"SCHEDULE_RUN_ACTIVE: 请先停止并排空运行");s.jobs.remove(id);s.offline_plans.remove(id);Ok(())})}
    pub fn invalidate_connection(&self,id:&str)->Result<()> {self.transaction(|s|{for record in s.jobs.values_mut().filter(|r|r.approvals.values().any(|a|a.scope.connection_ids.iter().any(|c|c==id))){record.paused_reason=Some("SCHEDULE_CONNECTION_CHANGED".into());record.queued=None;}Ok(())})}
    pub fn claim_due(&self,owner:&SchedulerOwner,now:DateTime<Utc>)->Result<Vec<ClaimedRun>> {self.claim_due_scoped(owner,now,None)}
    pub fn claim_due_scoped(&self,owner:&SchedulerOwner,now:DateTime<Utc>,only_job:Option<&str>)->Result<Vec<ClaimedRun>> {
        self.claim_due_clocked(owner, now, only_job, None)
    }
    /// Calendar jobs follow real UTC; a live runner supplies a revision-bound monotonic cutoff for intervals.
    pub(crate) fn claim_due_clocked(&self,owner:&SchedulerOwner,now:DateTime<Utc>,only_job:Option<&str>,intervals:Option<&BTreeMap<String,(String,DateTime<Utc>)>>)->Result<Vec<ClaimedRun>> {
        owner.validate()?;self.transaction(|state|{
            recover_expired(state,now);let mut claims=vec![];let ids:Vec<_>=state.jobs.keys().filter(|id|only_job.map_or(true,|only|id.as_str()==only)).cloned().collect();
            for id in ids {
                let mut record=state.jobs.get(&id).expect("known job").clone();
                if !record.job.enabled || record.paused_reason.is_some() || record.blocked_run_id.is_some() || (owner.mode==RunnerMode::Headless && record.job.execution_mode!=ExecutionMode::OfflineOptIn){continue;}
                if owner.mode==RunnerMode::Headless && !state.offline_plans.get(&id).is_some_and(|p|p.confirmed && p.installed_receipt.is_some() && p.schedule_revision==record.schedule_revision && p.job_revision==record.job.revision){continue;}
                let active=state.runs.iter().any(|r|r.job_id==id && (!r.status.terminal()||r.resources_pending));
                let mut candidate=if !active {record.queued.take().filter(|q|q.schedule_revision==record.schedule_revision).map(|q|(q.scheduled_at_utc,q.request_id))}else{None};
                let schedule_now=if matches!(&record.job.schedule,ScheduleSpec::Interval {..}) {
                    if let Some(cutoffs)=intervals {
                        match cutoffs.get(&id).filter(|(revision,_)|revision==&record.schedule_revision) {Some((_,cutoff))=>*cutoff,None=>continue}
                    } else {now}
                } else {now};
                if let Some(due)=record.next_run_utc.filter(|due|*due<=schedule_now){let decision=decide_due(&record.job.schedule,due,schedule_now,record.job.misfire)?;record.next_run_utc=decision.next_run_utc;
                    if let Some(at)=decision.run_at {if record.last_occurrence_utc.map_or(true,|last|at>last){if active {if record.job.overlap==OverlapPolicy::QueueOne {record.queued=Some(QueuedOccurrence {schedule_revision:record.schedule_revision.clone(),scheduled_at_utc:at,request_id:None});}}else if candidate.is_none(){candidate=Some((at,None));}else if record.job.overlap==OverlapPolicy::QueueOne{record.queued=Some(QueuedOccurrence {schedule_revision:record.schedule_revision.clone(),scheduled_at_utc:at,request_id:None});}}}
                }
                if let Some((at,request))=candidate {
                    let connections=connections_for(&record)?;
                    if capacity_available(state,&connections) {if let Some(run)=claim_one(state,&record,owner,now,at,request)?{record.last_occurrence_utc=Some(record.last_occurrence_utc.map_or(at,|last|last.max(at)));claims.push(ClaimedRun {run});}}
                    else if record.job.overlap==OverlapPolicy::QueueOne {record.queued=Some(QueuedOccurrence {schedule_revision:record.schedule_revision.clone(),scheduled_at_utc:at,request_id:request});}
                }
                state.jobs.insert(id,record);
            }
            Ok(claims)
        })
    }
    pub fn claim_manual(&self,id:&str,expected_revision:&str,request_id:&str,owner:&SchedulerOwner,now:DateTime<Utc>)->Result<Option<ClaimedRun>> {
        canonical_uuid(request_id)?;owner.validate()?;self.transaction(|state|{
            recover_expired(state,now);let mut record=state.jobs.get(id).cloned().ok_or_else(||anyhow!("SCHEDULE_JOB_MISSING"))?;ensure!(record.job.revision==expected_revision,"SCHEDULE_CONFLICT: 手动执行前任务变化");ensure!(record.blocked_run_id.is_none() && record.paused_reason.is_none(),"SCHEDULE_AWAITING_CONFIRMATION: 任务需要重新批准或核对未知运行");
            ensure!(owner.mode==RunnerMode::Gui,"SCHEDULE_OFFLINE_MANUAL_FORBIDDEN: headless 只执行已批准到期计划");
            let request_key=digest(&(&record.job.id,"manual",request_id))?;
            if state.consumed_manual_requests.contains(&request_key) {return Ok(None);}
            let connections=connections_for(&record)?;let active=state.runs.iter().any(|r|r.job_id==id && (!r.status.terminal()||r.resources_pending));
            if active||!capacity_available(state,&connections){if record.job.overlap==OverlapPolicy::QueueOne {record.queued=Some(QueuedOccurrence {schedule_revision:record.schedule_revision.clone(),scheduled_at_utc:now,request_id:Some(request_id.into())});state.jobs.insert(id.into(),record);}return Ok(None);}
            Ok(claim_one(state,&record,owner,now,now,Some(request_id.into()))?.map(|run|ClaimedRun {run}))
        })
    }
    pub fn heartbeat(&self,id:&str,owner:&SchedulerOwner,token:&str,now:DateTime<Utc>)->Result<bool>{self.transaction(|s|{let run=owned_run(s,id,owner,token,now)?;run.lease.expires_at_utc=now+Duration::seconds(LEASE_SECONDS);Ok(run.cancel_requested)})}
    pub fn start_run(&self,id:&str,owner:&SchedulerOwner,token:&str,now:DateTime<Utc>)->Result<ScheduledRun>{self.transaction(|s|{ensure_current_job(s,id)?;let run=owned_run(s,id,owner,token,now)?;ensure!(run.status==RunStatus::Claimed,"SCHEDULE_RUN_STATE: 运行已启动/终结");run.status=RunStatus::Running;run.started_at_utc=Some(now);push_event(run,now,"started",None,None);Ok(run.clone())})}
    /// started 是发送前的写前日志；保存失败必须保证零数据库调用。
    pub fn start_action(&self,id:&str,action:&str,owner:&SchedulerOwner,token:&str,now:DateTime<Utc>)->Result<bool>{self.transaction(|s|{
        ensure_current_job(s,id)?;let run=owned_run(s,id,owner,token,now)?;ensure!(run.status==RunStatus::Running,"SCHEDULE_RUN_STATE");if run.cancel_requested{return Ok(false);}
        let spec=run.job_snapshot.actions.iter().find(|a|a.id==action).ok_or_else(||anyhow!("SCHEDULE_ACTION_MISSING"))?;
        ensure!(spec.depends_on.iter().all(|dep|run.actions.iter().any(|a|a.action_id==*dep && a.status==ActionStatus::Succeeded && a.receipt.as_ref().is_some_and(|r|r.verified && !r.resources_pending))),"SCHEDULE_DEPENDENCY_BLOCKED: 上游没有确认成功");
        let journal=run.actions.iter_mut().find(|a|a.action_id==action).ok_or_else(||anyhow!("SCHEDULE_ACTION_MISSING"))?;ensure!(journal.status==ActionStatus::Pending,"SCHEDULE_ACTION_REPLAY: 动作只能启动一次");journal.status=ActionStatus::Started;journal.started_at_utc=Some(now);push_event(run,now,"action_started",Some(action),None);Ok(true)
    })}
    pub fn finish_action(&self,id:&str,action:&str,owner:&SchedulerOwner,token:&str,receipt:ActionReceipt,now:DateTime<Utc>)->Result<()> {receipt.validate()?;self.transaction(|s|{let run=owned_run(s,id,owner,token,now)?;let journal=run.actions.iter_mut().find(|a|a.action_id==action).ok_or_else(||anyhow!("SCHEDULE_ACTION_MISSING"))?;ensure!(matches!(journal.status,ActionStatus::Started|ActionStatus::Pending),"SCHEDULE_ACTION_REPLAY");journal.status=receipt.status;journal.completed_at_utc=Some(now);journal.receipt=Some(receipt.clone());run.resources_pending|=receipt.resources_pending;push_event(run,now,"action_completed",Some(action),receipt.error_code.as_deref());Ok(())})}
    pub fn complete_run(&self,id:&str,owner:&SchedulerOwner,token:&str,status:RunStatus,now:DateTime<Utc>)->Result<ScheduledRun>{ensure!(status.terminal(),"SCHEDULE_RUN_STATE: 缺少终态");self.transaction(|s|{
        let run=owned_run(s,id,owner,token,now)?;
        let all_ok=run.actions.iter().all(|a|a.status==ActionStatus::Succeeded && a.receipt.as_ref().is_some_and(|r|r.verified && !r.resources_pending));ensure!(status!=RunStatus::Succeeded || all_ok,"SCHEDULE_RUN_STATE: 只有全部动作确认成功才能结束成功");
        for action in &mut run.actions {if action.status==ActionStatus::Pending {action.status=ActionStatus::Blocked;action.completed_at_utc=Some(now);}}
        run.status=status;run.completed_at_utc=Some(now);push_event(run,now,"completed",None,None);let out=run.clone();
        if matches!(status,RunStatus::Unknown|RunStatus::Partial|RunStatus::AwaitingConfirmation) || out.resources_pending {if let Some(job)=s.jobs.get_mut(&out.job_id){job.blocked_run_id=Some(out.id.clone());job.queued=None;}}
        Ok(out)
    })}
    pub fn request_cancel(&self,id:&str,now:DateTime<Utc>)->Result<ScheduledRun>{self.transaction(|s|{let run=s.runs.iter_mut().find(|r|r.id==id).ok_or_else(||anyhow!("SCHEDULE_RUN_MISSING"))?;if !run.status.terminal(){run.cancel_requested=true;push_event(run,now,"cancel_requested",None,None);}Ok(run.clone())})}
    /// 只有真实 C1/工具排空证据允许解除资源占用；Unknown 终态保持不改写。
    pub fn mark_drained(&self,id:&str,evidence_digest:&str,now:DateTime<Utc>)->Result<()> {ensure!(is_digest(evidence_digest),"SCHEDULE_DRAIN_EVIDENCE_INVALID");self.transaction(|s|{let run=s.runs.iter_mut().find(|r|r.id==id).ok_or_else(||anyhow!("SCHEDULE_RUN_MISSING"))?;ensure!(run.status.terminal(),"SCHEDULE_RUN_ACTIVE");run.resources_pending=false;push_event(run,now,"resources_drained",None,None);Ok(())})}
    /// 宿主必须先展示 journal 并由用户明确核对；只允许从未来重新调度，绝不重放旧 occurrence。
    pub fn acknowledge_run(&self,id:&str,expected_job_revision:&str,evidence_digest:&str,now:DateTime<Utc>)->Result<JobRecord>{ensure!(is_digest(evidence_digest),"SCHEDULE_RECONCILIATION_INVALID");self.transaction(|s|{
        let run=s.runs.iter().find(|r|r.id==id).ok_or_else(||anyhow!("SCHEDULE_RUN_MISSING"))?;ensure!(run.status.terminal()&&!run.resources_pending,"SCHEDULE_RESOURCES_PENDING: 资源尚未确认排空");let job=s.jobs.get_mut(&run.job_id).ok_or_else(||anyhow!("SCHEDULE_JOB_MISSING"))?;ensure!(job.job.revision==expected_job_revision && job.blocked_run_id.as_deref()==Some(id),"SCHEDULE_CONFLICT");job.blocked_run_id=None;job.queued=None;job.paused_reason=Some("SCHEDULE_AWAITING_APPROVAL".into());job.approvals.clear();for a in &mut job.job.actions{a.approved_scope_revision.clear();}job.job.revision=uuid::Uuid::new_v4().to_string();job.next_run_utc=if job.job.enabled{next_run_after(&job.job.schedule,now)?}else{None};Ok(job.clone())
    })}
    /// Only the original owned worker may conservatively freeze an uncertain run after lease/write failure.
    /// This never authorizes another send and never turns an existing Unknown into success.
    pub(crate) fn record_uncertain(&self,id:&str,owner:&SchedulerOwner,token:&str,pending:bool,code:&str,now:DateTime<Utc>)->Result<ScheduledRun> {
        self.transaction(|state| {
            let run=state.runs.iter_mut().find(|r|r.id==id).ok_or_else(||anyhow!("SCHEDULE_RUN_MISSING"))?;
            ensure!(run.lease.owner==*owner && run.lease.token==token,"SCHEDULE_LEASE_LOST");
            if !run.status.terminal() {run.status=RunStatus::Unknown;run.completed_at_utc=Some(now);}
            run.resources_pending|=pending;
            for action in &mut run.actions {if action.status==ActionStatus::Started {action.status=ActionStatus::Unknown;}else if action.status==ActionStatus::Pending {action.status=ActionStatus::Blocked;}}
            push_event(run,now,"uncertain",None,Some(code));let out=run.clone();
            if let Some(job)=state.jobs.get_mut(&out.job_id) {job.blocked_run_id=Some(out.id.clone());job.queued=None;}
            Ok(out)
        })
    }
    pub fn recover(&self,now:DateTime<Utc>)->Result<()> {self.transaction(|s|{recover_expired(s,now);Ok(())})}
    pub(crate) fn data_root(&self) -> &Path { &self.root }
    /// Only an authenticated opaque plan reaches here; migration cannot dispatch database work.
    pub(crate) fn import_legacy_prepared(&self, prepared: &[super::migration::PreparedLegacyJob], expected_target: &str, source_check: impl Fn() -> Result<()>) -> Result<Vec<JobRecord>> {
        self.transaction(|state| {
            source_check()?;
            ensure!(super::migration::legacy_target_revision(state)? == expected_target, "SCHEDULE_LEGACY_TARGET_CHANGED");
            ensure!(!prepared.is_empty() && state.jobs.len() + prepared.len() <= MAX_JOBS, "SCHEDULE_JOB_QUOTA");
            ensure!(state.imported_legacy_digests.len() + prepared.len() <= 16_384, "SCHEDULE_LEGACY_QUOTA");
            let mut ids = BTreeSet::new(); let mut writes = Vec::new(); let mut records = Vec::new();
            // Preflight all collisions before writing even one template.
            for item in prepared {
                validate_job(&item.job)?; validate_template(&item.template)?;
                ensure!(!item.job.enabled && item.job.execution_mode == ExecutionMode::AppOnline && item.job.actions.len() == 1
                    && item.job.actions[0].approved_scope_revision.is_empty() && item.job.actions[0].template_ref == item.template.id
                    && item.job.actions[0].kind == item.template.kind, "SCHEDULE_LEGACY_PAYLOAD_INVALID");
                ensure!(ids.insert(item.job.id.clone()) && !state.jobs.contains_key(&item.job.id)
                    && !state.imported_legacy_digests.contains(&item.legacy.source_digest), "SCHEDULE_LEGACY_TARGET_CHANGED");
                let path = self.template_path(&item.template.id)?; let aad = format!("navkit:scheduler:v2:template:{}", item.template.id);
                if let Some(existing) = self.read_encrypted::<ActionTemplate>(&path, &aad, MAX_TEMPLATE_BYTES)? {
                    validate_template(&existing)?;
                    ensure!(super::migration::legacy_template_equal(&existing, &item.template), "SCHEDULE_LEGACY_TEMPLATE_CONFLICT");
                } else { writes.push((path, aad, item.template.clone())); }
                records.push(JobRecord { job: item.job.clone(), schedule_revision: schedule_revision(&item.job)?, next_run_utc: None,
                    queued: None, blocked_run_id: None, paused_reason: Some("SCHEDULE_LEGACY_REQUIRES_ENDPOINT_APPROVAL".into()),
                    approvals: BTreeMap::new(), legacy: Some(item.legacy.clone()), last_occurrence_utc: None });
            }
            let mut count = 0usize;
            for entry in fs::read_dir(self.dir())? { let entry = entry?; let name = entry.file_name(); let name = name.to_string_lossy();
                if name.starts_with("template-") && name.ends_with(".enc") { reject_link(&entry.path())?; count += 1; ensure!(count <= MAX_TEMPLATES, "SCHEDULE_TEMPLATE_QUOTA"); }
            }
            ensure!(count + writes.len() <= MAX_TEMPLATES, "SCHEDULE_TEMPLATE_QUOTA"); source_check()?;
            for (path, aad, template) in writes { self.write_encrypted(&path, &aad, &template, MAX_TEMPLATE_BYTES)?;
                ensure!(self.template_unlocked(&template.id)? == template, "SCHEDULE_LEGACY_VERIFY_FAILED"); }
            source_check()?;
            for (record, item) in records.iter().zip(prepared) { state.jobs.insert(record.job.id.clone(), record.clone()); state.imported_legacy_digests.insert(item.legacy.source_digest.clone()); }
            // A failed state commit leaves only non-executable orphan templates, never enabled jobs.
            Ok(records)
        })
    }
    pub(crate) fn update_state<T>(&self,operation:impl FnOnce(&mut SchedulerState)->Result<T>)->Result<T>{self.transaction(operation)}
}
fn connections_for(record:&JobRecord)->Result<Vec<String>> {ensure!(record.approvals.len()==record.job.actions.len(),"SCHEDULE_AWAITING_APPROVAL: 任务动作未全部批准");let mut ids=BTreeSet::new();for a in &record.job.actions {let approved=record.approvals.get(&a.id).ok_or_else(||anyhow!("SCHEDULE_AWAITING_APPROVAL"))?;ensure!(approved.scope_revision==a.approved_scope_revision&&approved.scope.revision()?==approved.scope_revision,"SCHEDULE_SCOPE_INVALID");for id in &approved.scope.connection_ids{ids.insert(id.clone());}}Ok(ids.into_iter().collect())}
fn capacity_available(state:&SchedulerState,connections:&[String])->bool{let active:Vec<_>=state.runs.iter().filter(|r|!r.status.terminal()||r.resources_pending).collect();active.len()<2 && active.iter().all(|r|!r.connection_ids.iter().any(|id|connections.contains(id)))}
fn claim_one(state:&mut SchedulerState,record:&JobRecord,owner:&SchedulerOwner,now:DateTime<Utc>,at:DateTime<Utc>,request_id:Option<String>)->Result<Option<ScheduledRun>> {
    let key=if let Some(ref request)=request_id {digest(&(&record.job.id,"manual",request))?}else{digest(&(&record.job.id,&record.schedule_revision,at))?};
    if state.runs.iter().any(|r|r.occurrence_key==key) || state.consumed_manual_requests.contains(&key){return Ok(None);}
    if request_id.is_some() {
        ensure!(state.consumed_manual_requests.len()<16384,"SCHEDULE_REQUEST_QUOTA: 手动领取去重账本已满；请保留历史并处理存储维护");
        state.consumed_manual_requests.insert(key.clone());
    }
    let run=ScheduledRun{id:uuid::Uuid::new_v4().to_string(),job_id:record.job.id.clone(),job_revision:record.job.revision.clone(),schedule_revision:record.schedule_revision.clone(),occurrence_key:key,scheduled_at_utc:at,claimed_at_utc:now,started_at_utc:None,completed_at_utc:None,status:RunStatus::Claimed,lease:RunLease {owner:owner.clone(),token:uuid::Uuid::new_v4().to_string(),expires_at_utc:now+Duration::seconds(LEASE_SECONDS)},cancel_requested:false,resources_pending:false,connection_ids:connections_for(record)?,actions:record.job.actions.iter().map(|a|ActionJournal {action_id:a.id.clone(),status:ActionStatus::Pending,started_at_utc:None,completed_at_utc:None,receipt:None}).collect(),journal:vec![JournalEvent {at_utc:now,kind:"claimed".into(),action_id:None,code:None}],job_snapshot:record.job.clone(),approvals:record.approvals.clone()};state.runs.push(run.clone());Ok(Some(run))
}
fn ensure_current_job(state:&SchedulerState,id:&str)->Result<()>{let run=state.runs.iter().find(|r|r.id==id).ok_or_else(||anyhow!("SCHEDULE_RUN_MISSING"))?;let job=state.jobs.get(&run.job_id).ok_or_else(||anyhow!("SCHEDULE_JOB_MISSING"))?;ensure!(job.job.revision==run.job_revision && job.schedule_revision==run.schedule_revision && job.paused_reason.is_none() && job.blocked_run_id.is_none(),"SCHEDULE_JOB_CHANGED: 任务已暂停、编辑或重新批准，旧排队执行失效");Ok(())}
fn owned_run<'a>(state:&'a mut SchedulerState,id:&str,owner:&SchedulerOwner,token:&str,now:DateTime<Utc>)->Result<&'a mut ScheduledRun>{let run=state.runs.iter_mut().find(|r|r.id==id).ok_or_else(||anyhow!("SCHEDULE_RUN_MISSING"))?;ensure!(run.lease.owner==*owner && run.lease.token==token && run.lease.expires_at_utc>now && !run.status.terminal(),"SCHEDULE_LEASE_LOST: 当前实例失去运行租约，禁止继续发送动作");Ok(run)}
fn push_event(run:&mut ScheduledRun,now:DateTime<Utc>,kind:&str,action:Option<&str>,code:Option<&str>){if run.journal.len()>=MAX_JOURNAL_EVENTS{run.journal.remove(1);}run.journal.push(JournalEvent{at_utc:now,kind:kind.into(),action_id:action.map(str::to_owned),code:code.map(safe_code)});}
fn recover_expired(state:&mut SchedulerState,now:DateTime<Utc>){for run in &mut state.runs{if !run.status.terminal() && run.lease.expires_at_utc<=now{let maybe_sent=run.status==RunStatus::Running||run.actions.iter().any(|a|a.status!=ActionStatus::Pending);run.status=if maybe_sent{RunStatus::Unknown}else{RunStatus::Cancelled};run.resources_pending=maybe_sent;run.completed_at_utc=Some(now);for a in &mut run.actions{if a.status==ActionStatus::Started{a.status=ActionStatus::Unknown;}else if a.status==ActionStatus::Pending{a.status=ActionStatus::Blocked;}}push_event(run,now,"lease_expired",None,Some(if maybe_sent{"SCHEDULE_EXECUTOR_LOST"}else{"SCHEDULE_EXPIRED_BEFORE_START"}));if maybe_sent{if let Some(job)=state.jobs.get_mut(&run.job_id){job.blocked_run_id=Some(run.id.clone());job.queued=None;}}}}}
fn trim_history(state:&mut SchedulerState){let mut removable:Vec<_>=state.runs.iter().filter(|r|r.status.terminal()&&!r.resources_pending&&!state.jobs.values().any(|j|j.blocked_run_id.as_deref()==Some(r.id.as_str()))).map(|r|(r.claimed_at_utc,r.id.clone())).collect();removable.sort_unstable();let remove:BTreeSet<_>=removable.iter().take(removable.len().saturating_sub(MAX_HISTORY)).map(|(_,id)|id.clone()).collect();state.runs.retain(|r|!remove.contains(&r.id));}
