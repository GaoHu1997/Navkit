//! Typed, reviewed scheduler tool actions. Reuses the same transfer/sync/design services as IPC.
//! Output files are unique per run. Import bytes are pinned to the reviewed digest in a private copy.
use std::{collections::HashMap, fs::{self, File, OpenOptions}, io::{Read, Write}, path::{Path, PathBuf}, sync::{Arc, OnceLock, atomic::{AtomicBool, Ordering}}, time::Duration};
use anyhow::{anyhow, ensure, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::Manager;
use crate::{commands::{self, AppState}, models::*, scheduler::*};
use crate::scheduler_desktop::state_access::SchedulerStateAccess;
#[path = "scheduler_structure.rs"]
pub(crate) mod structure;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag="kind", content="options", rename_all="snake_case", deny_unknown_fields)]
pub enum ToolDefinition {
    Backup(BackupOptions), Export(ExportOptions), Import(ImportOptions),
    DataSync(crate::sync::SyncOptions), StructureSync(crate::structure_sync::StructureSyncRequest),
}
impl ToolDefinition {
    fn kind(&self) -> ActionKind { match self { Self::Backup(_)=>ActionKind::Backup,Self::Export(_)=>ActionKind::Export,Self::Import(_)=>ActionKind::Import,Self::DataSync(_)=>ActionKind::DataSync,Self::StructureSync(_)=>ActionKind::StructureSync } }
    fn main(&self) -> (&str,&str) { match self { Self::Backup(o)=>(&o.connection_id,&o.schema),Self::Export(o)=>(&o.connection_id,&o.schema),Self::Import(o)=>(&o.connection_id,&o.schema),Self::DataSync(o)=>(&o.target_connection,&o.target_schema),Self::StructureSync(o)=>(&o.target_connection,&o.target_schema) } }
    fn resources(&self) -> Vec<(String,String)> {
        let (id,ns)=self.main(); let mut resources=vec![(id.to_owned(),ns.to_owned())];
        match self { Self::DataSync(o)=>resources.push((o.source_connection.clone(),o.source_schema.clone())),Self::StructureSync(o)=>resources.push((o.source_connection.clone(),o.source_schema.clone())),_=>{} }
        resources.sort();resources.dedup();resources
    }
    fn writes_database(&self) -> bool { matches!(self,Self::Import(_)|Self::DataSync(_)|Self::StructureSync(_)) }
}
fn parse(template:&ActionTemplate)->Result<ToolDefinition> {
    validate_template(template)?;
    let TemplateConfiguration::Tool { options_json }=&template.configuration else { return Err(anyhow!("SCHEDULE_TEMPLATE_KIND")); };
    let definition:ToolDefinition=serde_json::from_str(options_json).map_err(|_|anyhow!("SCHEDULE_TOOL_OPTIONS_INVALID"))?;
    ensure!(definition.kind()==template.kind && definition.main()==(template.connection_id.as_str(),template.namespace.as_str()),"SCHEDULE_TOOL_SCOPE_INVALID");
    ensure!(!definition.writes_database() || !template.read_only,"READ_ONLY_VIOLATION");
    match &definition {
        ToolDefinition::Import(o)=>{
            ensure!(!o.create_table && !o.truncate && matches!(o.format.to_ascii_lowercase().as_str(),"csv"|"json"),"SCHEDULE_IMPORT_PLAN_REQUIRED: 自动导入仅接受已有表和结构化数据，不执行隐式建表、清表或 SQL 文件");
            ensure!(matches!(o.on_conflict.as_str(),""|"error"|"skip"|"update"),"SCHEDULE_IMPORT_POLICY_INVALID");
        }
        ToolDefinition::DataSync(o)=>{
            ensure!(o.mode=="data" && o.write_mode=="append" && !o.create_target && !o.delete_missing,"SCHEDULE_DESTRUCTIVE_PLAN_REQUIRED: 自动数据同步必须保留目标额外数据和结构");
            ensure!(o.task_id.is_none(),"SCHEDULE_TOOL_ID_INVALID");
        }
        ToolDefinition::StructureSync(o)=>ensure!(!o.allow_drop && o.source_revision.is_empty() && o.target_revision.is_empty() && o.review_digest.is_empty(),"SCHEDULE_DESTRUCTIVE_PLAN_REQUIRED"),
        ToolDefinition::Backup(o)=>ensure!(o.routines.is_empty(),"SCHEDULE_ROUTINE_IDENTITY_REQUIRED: 备份例程须有完整对象身份"),
        _=>{},
    }
    Ok(definition)
}
pub(crate) fn validate_template_options(template: &ActionTemplate) -> Result<()> { parse(template).map(|_| ()) }
fn reject_link(path:&Path)->Result<()> {
    let metadata=fs::symlink_metadata(path)?;
    ensure!(!metadata.file_type().is_symlink(),"SCHEDULE_PATH_UNPROVEN");
    #[cfg(windows)] { use std::os::windows::fs::MetadataExt;ensure!(metadata.file_attributes()&0x400==0,"SCHEDULE_PATH_UNPROVEN"); }
    Ok(())
}
fn canonical_regular(path:&Path)->Result<PathBuf> {
    reject_link(path)?;let canonical=fs::canonicalize(path)?;ensure!(canonical.is_file(),"SCHEDULE_FILE_INVALID");Ok(canonical)
}
fn output_pattern(path:&str)->Result<PathBuf> {
    let path=Path::new(path);let file=path.file_name().ok_or_else(||anyhow!("SCHEDULE_OUTPUT_INVALID"))?;
    let parent=path.parent().filter(|p|!p.as_os_str().is_empty()).unwrap_or_else(||Path::new("."));
    reject_link(parent)?;let parent=fs::canonicalize(parent)?;ensure!(parent.is_dir(),"SCHEDULE_OUTPUT_INVALID");Ok(parent.join(file))
}
fn hash_reader(reader:&mut File,mut output:Option<&mut File>)->Result<String> {
    let mut digest=Sha256::new();let mut buffer=[0u8;64*1024];let mut total=0u64;
    loop {let count=reader.read(&mut buffer)?;if count==0 {break;}total=total.checked_add(count as u64).ok_or_else(||anyhow!("SCHEDULE_FILE_QUOTA"))?;
        ensure!(total<=16*1024*1024*1024,"SCHEDULE_FILE_QUOTA: 导入文件超过单次 16 GiB 上限");digest.update(&buffer[..count]);if let Some(file)=output.as_deref_mut(){file.write_all(&buffer[..count])?;}}
    Ok(format!("{:x}",digest.finalize()))
}
fn file_proof(path:&str)->Result<(PathBuf,String)> {
    let canonical=canonical_regular(Path::new(path))?;let mut file=File::open(&canonical)?;let before=file.metadata()?;
    let digest=hash_reader(&mut file,None)?;let after=file.metadata()?;
    ensure!(before.len()==after.len() && before.modified()?==after.modified()?,"SCHEDULE_INPUT_CHANGED");Ok((canonical,digest))
}
struct InputSnapshot(PathBuf);
impl Drop for InputSnapshot { fn drop(&mut self){let _=fs::remove_file(&self.0);} }
fn pin_input(root:&Path,input:&str,expected:&str,run_id:&str)->Result<InputSnapshot> {
    canonical_uuid(run_id)?;
    let source=canonical_regular(Path::new(input))?;
    // Temp files use random create_new names; they are not placed in the model/template directories.
    let dir=root.join("cache").join("scheduled-imports");fs::create_dir_all(&dir)?;reject_link(&dir)?;
    let path=dir.join(format!("{}-{}.data",run_id,uuid::Uuid::new_v4()));
    let snapshot=InputSnapshot(path.clone());let mut writer=OpenOptions::new().write(true).create_new(true).open(&path)?;
    let mut reader=File::open(source)?;let got=hash_reader(&mut reader,Some(&mut writer))?;writer.sync_all()?;
    ensure!(got==expected,"SCHEDULE_INPUT_CHANGED: 文件内容已变化，没有导入任何数据");Ok(snapshot)
}
fn unique_output(pattern:&str,run_id:&str)->Result<String> {
    canonical_uuid(run_id)?;let path=output_pattern(pattern)?;
    let stem=path.file_stem().and_then(|s|s.to_str()).ok_or_else(||anyhow!("SCHEDULE_OUTPUT_INVALID"))?;
    let suffix=path.extension().and_then(|s|s.to_str()).unwrap_or("data");
    let path=path.with_file_name(format!("{stem}-{run_id}.{suffix}"));
    ensure!(!path.try_exists()?,"SCHEDULE_OUTPUT_EXISTS: 不覆盖已有运行输出");Ok(path.to_string_lossy().into_owned())
}
async fn metadata_proof(state:&AppState,id:&str,namespace:&str,table:&str)->Result<String> {
    let cfg=commands::saved_config(state,id)?;let db=commands::scoped_db(state,id,namespace).await?;
    crate::sync::validate_sqlite_namespace(db.as_ref(),namespace,table).await?;
    crate::mutation::table_revision(db.as_ref(),&cfg.kind,namespace,table).await
}
struct StagedOutput { directory: PathBuf, file: PathBuf, destination: PathBuf }
impl StagedOutput {
    fn new(destination: &Path) -> Result<Self> {
        ensure!(!destination.try_exists()?, "SCHEDULE_OUTPUT_EXISTS");
        let parent = destination.parent().ok_or_else(|| anyhow!("SCHEDULE_OUTPUT_INVALID"))?;
        let directory = parent.join(format!(".navkit-scheduled-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory)?;
        let file = directory.join(destination.file_name().ok_or_else(|| anyhow!("SCHEDULE_OUTPUT_INVALID"))?);
        Ok(Self { directory, file, destination: destination.to_owned() })
    }
    fn file_name(&self) -> Result<String> {
        self.file.to_str().map(str::to_owned).ok_or_else(|| anyhow!("SCHEDULE_OUTPUT_INVALID"))
    }
    fn publish(&self) -> Result<()> {
        let source = canonical_regular(&self.file)?;
        // Windows：紧接写入后的 open 可能被拒绝（写句柄释放/扫描窗口），带退避重试刷新。
        // 刷新失败不改变发布语义（hard_link 仍原子且拒绝已存在目标），但必须显式报告而非静默跳过。
        let mut last_error = None;
        for attempt in 0..10 {
            // sync_all 需要可写句柄；只读 open 在部分 Windows 文件系统上返回 access denied。
            let attempt_open = fs::OpenOptions::new().read(true).write(true).open(&source);
            match attempt_open.and_then(|handle| handle.sync_all()) {
                Ok(()) => { last_error = None; break; }
                Err(error) => {
                    last_error = Some(error);
                    std::thread::sleep(std::time::Duration::from_millis(10 * (attempt + 1)));
                }
            }
        }
        if let Some(error) = last_error { return Err(anyhow!("SCHEDULE_OUTPUT_SYNC_FAILED: {error}")); }
        // The staging directory is beside the destination, on the same filesystem.
        // hard_link creates the destination atomically and refuses every existing entry,
        // including a symlink created after the preview. No rename may overwrite it.
        fs::hard_link(&source, &self.destination).map_err(|_| anyhow!("SCHEDULE_OUTPUT_PUBLISH_CONFLICT"))?;
        Ok(())
    }
}
impl Drop for StagedOutput {
    fn drop(&mut self) { let _ = fs::remove_file(&self.file); let _ = fs::remove_dir(&self.directory); }
}
struct ToolProof {
    scope: ActionScope,
    input_digest: Option<String>,
    backup_tables: Vec<String>,
}
pub async fn resolve_scope(app:&tauri::AppHandle,template:&ActionTemplate)->Result<ActionScope> {
    let state=app.state::<AppState>(); resolve_scope_state(&state,template).await
}
pub(crate) async fn resolve_scope_state(state:&AppState,template:&ActionTemplate)->Result<ActionScope> {
    let definition=parse(template)?;
    if let ToolDefinition::StructureSync(options) = &definition {
        return Box::pin(structure::resolve_scope(state, template, options)).await;
    }
    let _activity=state.activity.enter()?;
    let resources=definition.resources();let ids:Vec<_>=resources.iter().map(|(id,_)|id.as_str()).collect();
    let _lease=state.manual_transactions.ordinary(&ids).await?;
    Ok(resolve_proof(&state,template,&definition).await?.scope)
}
// Both callers already own the activity and ordinary leases; no release/reacquire between
// the final approval comparison, pinning the import bytes and dispatching the tool.
async fn resolve_proof(state:&AppState,template:&ActionTemplate,definition:&ToolDefinition)->Result<ToolProof> {
    let resources=definition.resources();
    let mut endpoints=vec![];
    for (id,namespace) in &resources {let cfg=commands::saved_config(&state,id)?;
        ensure!(matches!(cfg.kind.protocol(),Protocol::Sqlite|Protocol::Mysql|Protocol::Postgres) && !cfg.params.starts_with(crate::driver_manager::PARAMS_PREFIX),"SCHEDULE_TOOL_CHANNEL_UNPROVEN");
        if definition.writes_database() && id==&template.connection_id {ensure!(!cfg.read_only,"READ_ONLY_VIOLATION");}
        endpoints.push((id.clone(),namespace.clone(),crate::db::runtime::binding_fingerprint_at(&cfg,state.store.data_root())?));
    }
    let mut metadata=vec![];let mut files=vec![];
    let mut input_digest=None;let mut backup_tables=vec![];
    match &definition {
        ToolDefinition::Export(o)=>{crate::transfer::validate_where(o.where_clause.as_deref())?;metadata.push(metadata_proof(&state,&o.connection_id,&o.schema,&o.table).await?);files.push(digest(&output_pattern(&o.file)?)?);}
        ToolDefinition::Import(o)=>{metadata.push(metadata_proof(&state,&o.connection_id,&o.schema,&o.table).await?);
            let file=o.file.clone();let proof=tokio::task::spawn_blocking(move||file_proof(&file)).await??;
            files.push(digest(&proof)?);input_digest=Some(proof.1);}
        ToolDefinition::Backup(o)=>{let cfg=commands::saved_config(&state,&o.connection_id)?;let db=commands::scoped_db(&state,&o.connection_id,&o.schema).await?;
            let tables=crate::schema::list_tables(db.as_ref(),cfg.kind.clone(),&o.schema).await?;
            let selected:Vec<_>=if o.tables.is_empty(){tables.iter().filter(|(_,kind)|*kind==ObjectType::Table).map(|(name,_)|name.clone()).collect()}else{o.tables.clone()};
            ensure!(!selected.is_empty() && selected.len()<=512,"SCHEDULE_METADATA_QUOTA: 备份必须有明确的非空表集合");for table in &selected {ensure!(tables.iter().any(|(name,kind)|name==table && *kind==ObjectType::Table),"SCHEDULE_OBJECT_MISSING");metadata.push(digest(&(table,metadata_proof(&state,&o.connection_id,&o.schema,table).await?))?);}backup_tables=selected;}
        ToolDefinition::DataSync(o)=>{metadata.push(metadata_proof(&state,&o.source_connection,&o.source_schema,&o.source_table).await?);metadata.push(metadata_proof(&state,&o.target_connection,&o.target_schema,&o.target_table).await?);}
        ToolDefinition::StructureSync(_)=>return Err(anyhow!("SCHEDULE_STRUCTURE_CONTROLLED_EXECUTION_REQUIRED")),
    }
    // Material is re-read after metadata/file work, never replaced by a fingerprint from the template.
    for (id,_,expected) in &endpoints {let cfg=commands::saved_config(&state,id)?;ensure!(&crate::db::runtime::binding_fingerprint_at(&cfg,state.store.data_root())?==expected,"SCHEDULE_SCOPE_CHANGED");}
    let mut connection_ids:Vec<_>=resources.into_iter().map(|(id,_)|id).collect();connection_ids.sort();connection_ids.dedup();
    let permission_revision=digest(&(definition,&metadata,&files,&endpoints))?;
    let destructive_limit=if definition.writes_database() {Some(DestructiveLimit{plan_revision:permission_revision.clone(),max_delete_rows:None,allow_overwrite:matches!(definition,ToolDefinition::Import(_)),allow_drop:false})}else{None};
    let scope=ActionScope {template_ref:template.id.clone(),template_revision:template.revision.clone(),template_digest:digest(template)?,
        connection_id:template.connection_id.clone(),connection_ids,endpoint_fingerprint:digest(&endpoints)?,namespace:template.namespace.clone(),
        read_only:template.read_only,permission_revision,destructive_limit};scope.validate()?;
    Ok(ToolProof {scope,input_digest,backup_tables})
}
fn pending()->&'static parking_lot::Mutex<HashMap<String,Arc<AtomicBool>>> {static PENDING:OnceLock<parking_lot::Mutex<HashMap<String,Arc<AtomicBool>>>>=OnceLock::new();PENDING.get_or_init(Default::default)}
fn key(context:&ActionExecutionContext)->String {format!("{}/{}",context.run_id,context.action_id)}
pub async fn resources_drained(_app:&tauri::AppHandle,context:&ActionExecutionContext,receipt:&ActionReceipt)->Result<bool> {
    resources_drained_state(context,receipt).await
}
pub(crate) async fn resources_drained_state(context:&ActionExecutionContext,receipt:&ActionReceipt)->Result<bool> {
    if let Some(drained) = structure::resources_drained(context, receipt)? { return Ok(drained); }
    let id=key(context);let mut all=pending().lock();let Some(done)=all.get(&id)else{return Ok(!receipt.resources_pending)};
    let drained=done.load(Ordering::Acquire);if drained {all.remove(&id);}Ok(drained)
}
async fn follow<T,F>(task_id:&str,context:&ActionExecutionContext,work:F)->Result<T>
where F:std::future::Future<Output=Result<T>> {
    tokio::pin!(work);let mut tick=tokio::time::interval(Duration::from_millis(50));tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {tokio::select!{result=&mut work=>return result,_=tick.tick()=>{if context.stop_reason().is_some(){let _=crate::transfer::cancel_task(task_id);}}}}
}
pub async fn execute(app:&tauri::AppHandle,template:ActionTemplate,approved:ActionScope,context:ActionExecutionContext)->Result<ActionReceipt> {
    execute_state(SchedulerStateAccess::Desktop(app.clone()),template,approved,context).await
}
pub(crate) async fn execute_state(access:SchedulerStateAccess,template:ActionTemplate,approved:ActionScope,context:ActionExecutionContext)->Result<ActionReceipt> {
    let state=access.get(); execute_with_state(&state,access.clone(),template,approved,context).await
}
async fn execute_with_state(state:&AppState,access:SchedulerStateAccess,template:ActionTemplate,approved:ActionScope,context:ActionExecutionContext)->Result<ActionReceipt> {
    let definition=parse(&template)?;
    if let ToolDefinition::StructureSync(options) = &definition {
        return Box::pin(structure::execute(access, template.clone(), options.clone(), approved, context)).await;
    }
    if context.stop_reason().is_some(){return Ok(stopped());}
    let activity=state.activity.enter()?;let resources=definition.resources();
    let ids:Vec<_>=resources.iter().map(|(id,_)|id.as_str()).collect();
    let ordinary=state.manual_transactions.ordinary(&ids).await?;
    let proof=match resolve_proof(&state,&template,&definition).await {
        Ok(proof) if proof.scope==approved=>proof,
        _=>{let mut r=ActionReceipt::failed("SCHEDULE_SCOPE_CHANGED");r.status=ActionStatus::AwaitingConfirmation;return Ok(r);}
    };
    let task_id=uuid::Uuid::new_v4().to_string();
    let input=if let ToolDefinition::Import(o)=&definition {
        let file=o.file.clone();let run=context.run_id.clone();let root=state.store.data_root().to_owned();
        let expected=proof.input_digest.clone().ok_or_else(||anyhow!("SCHEDULE_INPUT_UNAVAILABLE"))?;
        match tokio::task::spawn_blocking(move||pin_input(&root,&file,&expected,&run)).await {
            Ok(Ok(snapshot))=>Some(snapshot),
            _=>{let mut r=ActionReceipt::failed("SCHEDULE_INPUT_CHANGED");r.status=ActionStatus::AwaitingConfirmation;return Ok(r);}
        }
    }else{None};
    // File copying can take time. Check both the stop flag and the complete reviewed
    // endpoint/schema proof again, retaining the same activity and connection leases.
    if context.stop_reason().is_some(){return Ok(stopped());}
    match resolve_proof(&state,&template,&definition).await {
        Ok(current) if current.scope==approved=>{},
        _=>{let mut r=ActionReceipt::failed("SCHEDULE_SCOPE_CHANGED");r.status=ActionStatus::AwaitingConfirmation;return Ok(r);}
    }
    let work=async {
        match definition {
            ToolDefinition::Export(mut o)=>{
                let destination=PathBuf::from(unique_output(&o.file,&context.run_id)?);
                let cfg=commands::saved_config(&state,&o.connection_id)?;let db=commands::scoped_db(&state,&o.connection_id,&o.schema).await?;
                commands::protect_export_path(&state,&destination.to_string_lossy(),db.as_ref()).await?;
                let output=StagedOutput::new(&destination)?;o.file=output.file_name()?;
                let rows=crate::transfer::run_export_task(db.as_ref(),&cfg.kind,&crate::transfer::ExportTaskOptions {options:o,task_id:task_id.clone()}).await?;
                ensure!(context.stop_reason().is_none(),"SCHEDULE_CANCELLED_BEFORE_PUBLICATION");
                output.publish()?;Ok(rows)
            }
            ToolDefinition::Import(mut o)=>{o.file=input.as_ref().ok_or_else(||anyhow!("SCHEDULE_INPUT_UNAVAILABLE"))?.0.to_string_lossy().into_owned();let cfg=commands::saved_config(&state,&o.connection_id)?;ensure!(!cfg.read_only,"READ_ONLY_VIOLATION");let db=commands::scoped_db(&state,&o.connection_id,&o.schema).await?;
                commands::validate_table_target(db.as_ref(),&cfg.kind,&o.schema,&o.table,false).await?;crate::transfer::run_import_task(db.as_ref(),&cfg.kind,&crate::transfer::ImportTaskOptions {options:o,task_id:task_id.clone(),column_mapping:vec![]}).await}
            ToolDefinition::Backup(o)=>{let cfg=commands::saved_config(&state,&o.connection_id)?;let db=commands::scoped_db(&state,&o.connection_id,&o.schema).await?;let dir=crate::storage::backup_dir(&o.connection_id,&o.schema);
                let file=dir.join(format!("scheduled-{}.sql",context.run_id));
                commands::protect_export_path(&state,&file.to_string_lossy(),db.as_ref()).await?;
                let output=StagedOutput::new(&file)?;
                let (rows,_)=crate::transfer::dump_database(db.as_ref(),&cfg.kind,&o.schema,&proof.backup_tables,&o.routines,o.include_structure,o.include_data,&output.file_name()?,&o.comment).await?;
                ensure!(context.stop_reason().is_none(),"SCHEDULE_CANCELLED_BEFORE_PUBLICATION");
                output.publish()?;Ok(rows)}
            ToolDefinition::DataSync(mut o)=>{o.task_id=Some(task_id.clone());let source_cfg=commands::saved_config(&state,&o.source_connection)?;let target_cfg=commands::saved_config(&state,&o.target_connection)?;ensure!(!target_cfg.read_only,"READ_ONLY_VIOLATION");
                let source=commands::scoped_db(&state,&o.source_connection,&o.source_schema).await?;let target=commands::scoped_db(&state,&o.target_connection,&o.target_schema).await?;
                crate::sync::synchronize(source.as_ref(),&source_cfg.kind,target.as_ref(),&target_cfg.kind,&o).await.map(|r|r.rows)}
            ToolDefinition::StructureSync(_)=>Err(anyhow!("SCHEDULE_STRUCTURE_CONTROLLED_EXECUTION_REQUIRED")),
        }
    };
    let result=follow(&task_id,&context,work).await;
    drop(input);
    match result {
        Ok(count)=>{drop(ordinary);drop(activity);Ok(ActionReceipt {status:ActionStatus::Succeeded,verified:true,effects:ActionEffects::Committed,resources_pending:false,execution_id:None,affected_rows:Some(count.to_string()),error_code:None})}
        Err(_)=>{
            // The cleanup owner retains every lease even if gate admission or physical drain
            // exceeds the IPC budget. A fresh connection alone is not evidence of drainage.
            let done=Arc::new(AtomicBool::new(false));pending().lock().insert(key(&context),done.clone());
            let connection_ids=approved.connection_ids.clone();
            crate::sql_history::spawn(async move {
                let state=access.get();let _owned_guards=(ordinary,activity);
                loop {
                    match crate::runtime_commands::retire_many(&state,connection_ids.clone(),()).await {
                        Ok(())=>break,
                        Err(_)=>{
                            let all_closing=connection_ids.iter().all(|id|state.connection_runtime.require_openable(id).is_err());
                            if all_closing {
                                while connection_ids.iter().any(|id|state.connection_runtime.require_openable(id).is_err()) {tokio::time::sleep(Duration::from_millis(100)).await;}
                                break;
                            }
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                    }
                }
                done.store(true,Ordering::Release);
            });
            Ok(ActionReceipt {status:ActionStatus::Unknown,verified:false,effects:ActionEffects::Unknown,resources_pending:true,execution_id:None,affected_rows:None,error_code:Some("SCHEDULE_TOOL_EFFECTS_UNKNOWN".into())})
        }
    }
}
fn stopped()->ActionReceipt {let mut receipt=ActionReceipt::failed("SCHEDULE_CANCELLED_BEFORE_SEND");receipt.status=ActionStatus::Cancelled;receipt}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn unique_output_never_targets_existing_pattern(){let dir=std::env::temp_dir().join(format!("navkit-scheduled-file-{}",uuid::Uuid::new_v4()));fs::create_dir_all(&dir).unwrap();let pattern=dir.join("out.csv");fs::write(&pattern,b"keep").unwrap();let actual=unique_output(pattern.to_str().unwrap(),&uuid::Uuid::new_v4().to_string()).unwrap();assert_ne!(Path::new(&actual),pattern);assert_eq!(fs::read(&pattern).unwrap(),b"keep");fs::remove_dir_all(dir).unwrap();}
    #[test]
    fn publication_refuses_a_destination_created_after_staging() {
        let root=std::env::temp_dir().join(format!("navkit-publication-{}",uuid::Uuid::new_v4()));fs::create_dir(&root).unwrap();
        let destination=root.join("out.csv");
        { let output=StagedOutput::new(&destination).unwrap();fs::write(&output.file,b"new").unwrap();fs::write(&destination,b"keep").unwrap();
          assert!(output.publish().is_err());assert_eq!(fs::read(&destination).unwrap(),b"keep"); }
        assert_eq!(fs::read_dir(&root).unwrap().count(),1);fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn publication_keeps_only_the_completed_output() {
        let root=std::env::temp_dir().join(format!("navkit-publication-{}",uuid::Uuid::new_v4()));fs::create_dir(&root).unwrap();
        let destination=root.join("out.csv");
        { let output=StagedOutput::new(&destination).unwrap();fs::write(&output.file,b"complete").unwrap();output.publish().unwrap(); }
        assert_eq!(fs::read(&destination).unwrap_or_else(|e| panic!("读取发布文件失败 {destination:?}: {e}；目录项={:?}", fs::read_dir(&root).map(|d| d.filter_map(|x| x.ok().map(|e| e.file_name())).collect::<Vec<_>>()))),b"complete");
        // Windows 下 Drop 的目录删除可能滞后于句柄释放；重试读取并在失败时明确报错（不静默跳过）。
        let entries = { let mut last = None; let mut value = None;
            for _ in 0..20 { match fs::read_dir(&root) { Ok(iter) => { value = Some(iter.count()); break; } Err(e) => { last = Some(e); std::thread::sleep(std::time::Duration::from_millis(25)); } } }
            value.unwrap_or_else(|| panic!("读取发布目录失败：{:?}", last)) };
        assert_eq!(entries,1,"发布目录应只保留一个完成文件");
        assert!(StagedOutput::new(&destination).is_err());fs::remove_dir_all(root).unwrap();
    }
    #[test] fn pin_import_rejects_changed_bytes_without_deleting_source(){let id=uuid::Uuid::new_v4().to_string();let path=std::env::temp_dir().join(format!("navkit-scheduled-input-{id}"));fs::write(&path,b"changed").unwrap();assert!(pin_input(&std::env::temp_dir(),path.to_str().unwrap(),&"a".repeat(64),&id).is_err());assert_eq!(fs::read(&path).unwrap(),b"changed");fs::remove_file(path).unwrap();}
}
