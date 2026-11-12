//! Windows 离线模式的可审查安装计划与 headless 参数验证。
//! 此模块不调用 schtasks/COM/PowerShell，也不自动注册系统任务。宿主显式批准后才可调用系统适配器。
use super::{contracts::*, store::SchedulerStore};
use anyhow::{anyhow, ensure, Result};
use chrono::{DateTime,Utc};
use serde::{Deserialize,Serialize};
use sha2::{Digest,Sha256};
use std::{fs,io::Read,path::{Path,PathBuf}};

#[derive(Debug,Clone,Serialize,Deserialize,PartialEq,Eq)]
#[serde(deny_unknown_fields)]
pub struct OfflineInstallPlan {
    pub id:String,
    pub job_id:String,
    pub job_revision:String,
    pub schedule_revision:String,
    pub executable:String,
    pub executable_digest:String,
    pub argv:Vec<String>,
    pub task_name:String,
    pub account_label:String,
    pub wake_device:bool,
    pub run_when_logged_out:bool,
    pub prepared_at_utc:DateTime<Utc>,
    pub expires_at_utc:DateTime<Utc>,
    pub consent_digest:String,
    pub confirmed:bool,
    pub installed_receipt:Option<OfflineRegistrationReceipt>,
}
#[derive(Debug,Clone,Serialize,Deserialize,PartialEq,Eq)]
pub struct OfflineRegistrationReceipt {pub task_name:String,pub installed_at_utc:DateTime<Utc>,pub executable_digest:String,pub configuration_digest:String}
#[derive(Debug,Clone,Serialize,Deserialize,PartialEq,Eq)]
pub struct OfflineOptions {pub account_label:String,pub wake_device:bool,pub run_when_logged_out:bool}
#[derive(Debug,Clone,Serialize,Deserialize,PartialEq,Eq)]
pub struct OfflineDiagnostic {pub code:String,pub ok:bool,pub requires_user_action:bool}
#[derive(Debug,Clone,PartialEq,Eq)]
pub struct HeadlessScheduleRequest {pub job_id:String,pub schedule_revision:String}
impl HeadlessScheduleRequest {
    /// 只接收 exe 之后的 argv；不接受 SQL、连接串、文件路径或秘密参数。
    pub fn parse(argv:&[String])->Result<Self>{ensure!(argv.len()==5 && argv[0]=="--headless-schedule" && argv[1]=="--job-id" && argv[3]=="--schedule-revision","SCHEDULE_HEADLESS_ARGUMENTS: headless 参数必须是固定模板，不接受额外参数");canonical_uuid(&argv[2])?;ensure!(is_digest(&argv[4]),"SCHEDULE_HEADLESS_ARGUMENTS: schedule revision 无效");Ok(Self {job_id:argv[2].clone(),schedule_revision:argv[4].clone()})}
    pub fn argv(&self)->Vec<String>{vec!["--headless-schedule".into(),"--job-id".into(),self.job_id.clone(),"--schedule-revision".into(),self.schedule_revision.clone()]}
}
fn executable_digest(path:&Path)->Result<(PathBuf,String)> {
    let canonical=path.canonicalize().map_err(|_|anyhow!("SCHEDULE_EXE_MISSING: 当前应用执行文件不存在或已移动"))?;
    ensure!(canonical.is_file() && canonical.extension().is_some_and(|v|v.to_string_lossy().eq_ignore_ascii_case("exe")),"SCHEDULE_EXE_INVALID: Windows 离线模式需要真实应用 exe");
    let mut file=fs::File::open(&canonical).map_err(|_|anyhow!("SCHEDULE_EXE_UNREADABLE"))?;let mut hash=Sha256::new();let mut buffer=[0u8;65536];loop{let n=file.read(&mut buffer)?;if n==0{break;}hash.update(&buffer[..n]);}
    Ok((canonical,format!("{:x}",hash.finalize())))
}
fn consent_digest(plan:&OfflineInstallPlan)->Result<String>{digest(&(&plan.id,&plan.job_id,&plan.job_revision,&plan.schedule_revision,&plan.executable,&plan.executable_digest,&plan.argv,&plan.task_name,&plan.account_label,plan.wake_device,plan.run_when_logged_out,plan.prepared_at_utc,plan.expires_at_utc))}
impl SchedulerStore {
    /// 显式预览；enabled+offline_opt_in 是用户已在任务配置中选择该模式的必要条件。
    pub fn prepare_offline_install(&self,id:&str,expected_job_revision:&str,executable:&Path,options:OfflineOptions,now:DateTime<Utc>)->Result<OfflineInstallPlan>{
        ensure!(cfg!(windows),"SCHEDULE_OFFLINE_UNSUPPORTED: 当前平台尚未实现系统调度适配器");
        ensure!(!options.account_label.trim().is_empty() && options.account_label.len()<=256 && !options.account_label.chars().any(char::is_control),"SCHEDULE_OFFLINE_ACCOUNT: 必须展示明确运行账号");let (exe,exe_digest)=executable_digest(executable)?;
        self.update_state(|state|{let record=state.jobs.get(id).ok_or_else(||anyhow!("SCHEDULE_JOB_MISSING"))?;ensure!(record.job.revision==expected_job_revision && record.job.enabled && record.job.execution_mode==ExecutionMode::OfflineOptIn,"SCHEDULE_OFFLINE_NOT_OPTED_IN: 必须明确启用应用关闭后的执行模式");ensure!(record.paused_reason.is_none() && record.blocked_run_id.is_none() && record.approvals.len()==record.job.actions.len(),"SCHEDULE_AWAITING_CONFIRMATION: 任务尚未得到完整批准");
            let request=HeadlessScheduleRequest {job_id:id.into(),schedule_revision:record.schedule_revision.clone()};
            let mut plan=OfflineInstallPlan {id:uuid::Uuid::new_v4().to_string(),job_id:id.into(),job_revision:record.job.revision.clone(),schedule_revision:record.schedule_revision.clone(),executable:exe.to_string_lossy().into_owned(),executable_digest:exe_digest.clone(),argv:request.argv(),task_name:format!("NavKit-Schedule-{id}"),account_label:options.account_label.clone(),wake_device:options.wake_device,run_when_logged_out:options.run_when_logged_out,prepared_at_utc:now,expires_at_utc:now+chrono::Duration::minutes(10),consent_digest:String::new(),confirmed:false,installed_receipt:None};plan.consent_digest=consent_digest(&plan)?;state.offline_plans.insert(id.into(),plan.clone());Ok(plan)
        })
    }
    /// 只把明确用户同意记录为 confirmed，尚未声称系统任务已经安装。
    pub fn confirm_offline_install(&self,job_id:&str,plan_id:&str,reviewed_digest:&str,now:DateTime<Utc>)->Result<OfflineInstallPlan>{self.update_state(|state|{let plan=state.offline_plans.get_mut(job_id).ok_or_else(||anyhow!("SCHEDULE_OFFLINE_PLAN_MISSING"))?;let record=state.jobs.get(job_id).ok_or_else(||anyhow!("SCHEDULE_JOB_MISSING"))?;ensure!(plan.id==plan_id && plan.consent_digest==reviewed_digest && consent_digest(plan)?==reviewed_digest && plan.expires_at_utc>now && record.job.revision==plan.job_revision && record.schedule_revision==plan.schedule_revision && record.job.enabled && record.job.execution_mode==ExecutionMode::OfflineOptIn,"SCHEDULE_OFFLINE_CONSENT_STALE: 安装计划或批准内容已改变，请重新预览");let (_,hash)=executable_digest(Path::new(&plan.executable))?;ensure!(hash==plan.executable_digest,"SCHEDULE_EXE_CHANGED: 执行文件已改变，请重新核对");plan.confirmed=true;Ok(plan.clone())})}
    /// 仅供真实系统适配器在 register 返回后记录；从不根据“已确认”伪造安装成功。
    pub fn record_offline_registration(&self,job_id:&str,plan_id:&str,receipt:OfflineRegistrationReceipt)->Result<()> {self.update_state(|state|{let plan=state.offline_plans.get_mut(job_id).ok_or_else(||anyhow!("SCHEDULE_OFFLINE_PLAN_MISSING"))?;ensure!(plan.confirmed && plan.id==plan_id && receipt.task_name==plan.task_name && receipt.executable_digest==plan.executable_digest && receipt.configuration_digest==plan.consent_digest,"SCHEDULE_OFFLINE_RECEIPT_INVALID");plan.installed_receipt=Some(receipt);Ok(())})}
    pub fn offline_diagnostics(&self,id:&str)->Result<Vec<OfflineDiagnostic>>{let state=self.snapshot()?;let record=state.jobs.get(id).ok_or_else(||anyhow!("SCHEDULE_JOB_MISSING"))?;let mut out=vec![OfflineDiagnostic {code:if cfg!(windows){"SCHEDULE_OFFLINE_WINDOWS"}else{"SCHEDULE_OFFLINE_UNSUPPORTED"}.into(),ok:cfg!(windows),requires_user_action:!cfg!(windows)}];let Some(plan)=state.offline_plans.get(id) else {out.push(OfflineDiagnostic {code:"SCHEDULE_OFFLINE_NOT_INSTALLED".into(),ok:false,requires_user_action:true});return Ok(out)};let matching=plan.job_revision==record.job.revision && plan.schedule_revision==record.schedule_revision && record.job.enabled;out.push(OfflineDiagnostic {code:"SCHEDULE_OFFLINE_REVISION".into(),ok:matching,requires_user_action:!matching});let executable_ok=executable_digest(Path::new(&plan.executable)).is_ok_and(|(_,hash)|hash==plan.executable_digest);out.push(OfflineDiagnostic {code:"SCHEDULE_OFFLINE_EXECUTABLE".into(),ok:executable_ok,requires_user_action:!executable_ok});out.push(OfflineDiagnostic {code:"SCHEDULE_OFFLINE_REGISTRATION".into(),ok:plan.confirmed&&plan.installed_receipt.is_some(),requires_user_action:!(plan.confirmed&&plan.installed_receipt.is_some())});Ok(out)}
    /// 返回精确待删除系统任务名，调用者显式确认后交给 OS 适配器；此方法不执行系统命令。
    pub fn prepare_offline_removal(&self,id:&str)->Result<Option<String>>{Ok(self.snapshot()?.offline_plans.get(id).map(|p|p.task_name.clone()))}
    pub fn record_offline_removal(&self,id:&str,expected_plan_id:&str)->Result<()> {self.update_state(|state|{ensure!(state.offline_plans.get(id).is_some_and(|p|p.id==expected_plan_id),"SCHEDULE_OFFLINE_CONFLICT");state.offline_plans.remove(id);Ok(())})}
    /// GUI 与 headless 都必须初始化相同本机 data_dir/密钥/RuntimeProfile 与执行服务后再调用。
    pub fn validate_headless_request(&self,request:&HeadlessScheduleRequest,actual_executable:&Path)->Result<JobRecord>{let state=self.snapshot()?;let record=state.jobs.get(&request.job_id).ok_or_else(||anyhow!("SCHEDULE_JOB_MISSING"))?;let plan=state.offline_plans.get(&request.job_id).ok_or_else(||anyhow!("SCHEDULE_OFFLINE_NOT_OPTED_IN"))?;ensure!(record.job.enabled && record.job.execution_mode==ExecutionMode::OfflineOptIn && record.schedule_revision==request.schedule_revision && plan.schedule_revision==request.schedule_revision && plan.job_revision==record.job.revision && plan.confirmed && plan.installed_receipt.is_some() && record.paused_reason.is_none() && record.blocked_run_id.is_none(),"SCHEDULE_OFFLINE_APPROVAL_INVALID: 任务已暂停、变更、删除或尚未安装");let (exe,hash)=executable_digest(actual_executable)?;ensure!(exe==PathBuf::from(&plan.executable) && hash==plan.executable_digest,"SCHEDULE_EXE_CHANGED: 便携执行文件已移动或更换");Ok(record.clone())}
}
#[cfg(test)]mod tests{use super::*;#[test]fn fixed_headless_argv_rejects_extra_secrets(){let r=HeadlessScheduleRequest{job_id:uuid::Uuid::new_v4().to_string(),schedule_revision:"a".repeat(64)};assert_eq!(HeadlessScheduleRequest::parse(&r.argv()).unwrap(),r);let mut values=r.argv();values.push("--password=p".into());assert!(HeadlessScheduleRequest::parse(&values).is_err());assert!(!r.argv().join(" ").contains("sql"));}}
