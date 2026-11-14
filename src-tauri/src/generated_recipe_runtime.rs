// Recipe additions to the existing batch module: same owned physical session and drain lifecycle.
fn prepare_recipe(work:&RecipeWork,info:&TableSchema,fingerprint:String)->Result<crate::generated_recipe::Engine> {
    if !work.preview && work.recipe.schema_fingerprint!=fingerprint {return Err(AppError::coded("RECIPE_SCHEMA_CHANGED","授权后的目标结构/default/约束/trigger 已变化；当前段没有发送生成行，已确认提交前缀不受影响，请重新预览授权").into());}
    let engine=crate::generated_recipe::Engine::new(&work.recipe,info)?;
    if work.preview {*work.sample.lock()=Some(engine.preview(fingerprint,info)?);}
    Ok(engine)
}
fn recipe_digest(info:&TableSchema,metadata:&serde_json::Value)->Result<String> {
    let bytes=serde_json::to_vec(&(info,metadata))?;
    if bytes.len()>1024*1024 {bail!("目标 recipe 元数据超过 1 MiB 支持预算");}
    Ok(format!("{:x}",Sha256::digest(bytes)))
}
async fn recipe_fingerprint_sqlite(conn:&mut sqlx::SqliteConnection,_kind:&DbKind,req:&GeneratedRowsRequest,info:&TableSchema)->Result<String> {
    let version:i64=crate::sql_history::query_scalar("PRAGMA main.schema_version").fetch_one(&mut *conn).await?;
    let objects=crate::sql_history::query("SELECT type,name,sql FROM main.sqlite_master WHERE tbl_name=? ORDER BY type,name").bind(&req.table).fetch_all(&mut *conn).await?;
    let mut definitions=vec![];for row in objects {definitions.push((row.try_get::<String,_>("type")?,row.try_get::<String,_>("name")?,row.try_get::<Option<String>,_>("sql")?));}
    // DDL covers CHECK/FK/collation/generated/default and all indexes/triggers. schema_version
    // also prevents silently authorizing a drop/recreate with identical DDL (conservative).
    recipe_digest(info,&serde_json::json!({"schema_version":version,"definitions":definitions}))
}
async fn recipe_fingerprint_mysql(conn:&mut sqlx::MySqlConnection,kind:&DbKind,req:&GeneratedRowsRequest,info:&TableSchema)->Result<String> {
    let ddl=crate::sql_history::query(&format!("SHOW CREATE TABLE {}",db::qualified(kind,&req.schema,&req.table))).fetch_one(&mut *conn).await?;
    let ddl=crate::schema::mysql_get(&ddl,"Create Table").ok_or_else(||anyhow!("无法读取完整目标 DDL，拒绝授权 recipe"))?;
    let triggers=crate::sql_history::query("SELECT TRIGGER_NAME,ACTION_TIMING,EVENT_MANIPULATION,ACTION_STATEMENT,SQL_MODE,CHARACTER_SET_CLIENT,COLLATION_CONNECTION,DATABASE_COLLATION FROM information_schema.TRIGGERS WHERE EVENT_OBJECT_SCHEMA=? AND EVENT_OBJECT_TABLE=? ORDER BY TRIGGER_NAME").bind(&req.schema).bind(&req.table).fetch_all(&mut *conn).await?;
    let mut definitions=vec![];for row in triggers {let mut values=vec![];for key in ["TRIGGER_NAME","ACTION_TIMING","EVENT_MANIPULATION","ACTION_STATEMENT","SQL_MODE","CHARACTER_SET_CLIENT","COLLATION_CONNECTION","DATABASE_COLLATION"]{values.push(crate::schema::mysql_get(&row,key));}definitions.push(values);}
    recipe_digest(info,&serde_json::json!({"ddl":ddl,"triggers":definitions}))
}
async fn recipe_fingerprint_pg(conn:&mut sqlx::PgConnection,_kind:&DbKind,req:&GeneratedRowsRequest,info:&TableSchema)->Result<String> {
    let objects=crate::sql_history::query_scalar::<_,String>("SELECT descriptor FROM (SELECT 'table:'||c.oid::text||':'||c.relkind::text||':'||c.relrowsecurity::text||':'||c.relforcerowsecurity::text AS descriptor FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2 UNION ALL SELECT 'column:'||a.attnum::text||':'||a.atttypid::text||':'||a.atttypmod::text||':'||a.attcollation::text||':'||a.attgenerated::text||':'||a.attidentity::text FROM pg_attribute a JOIN pg_class c ON c.oid=a.attrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2 AND a.attnum>0 AND NOT a.attisdropped UNION ALL SELECT 'constraint:'||co.oid::text||':'||pg_get_constraintdef(co.oid,true) FROM pg_constraint co JOIN pg_class c ON c.oid=co.conrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2 UNION ALL SELECT 'index:'||i.indexrelid::text||':'||pg_get_indexdef(i.indexrelid) FROM pg_index i JOIN pg_class c ON c.oid=i.indrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2 UNION ALL SELECT 'trigger:'||t.oid::text||':'||t.tgenabled::text||':'||pg_get_triggerdef(t.oid,true)||':'||pg_get_functiondef(t.tgfoid) FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2 AND NOT t.tgisinternal UNION ALL SELECT 'policy:'||p.polname||':'||p.polcmd::text||':'||COALESCE(pg_get_expr(p.polqual,p.polrelid),'')||':'||COALESCE(pg_get_expr(p.polwithcheck,p.polrelid),'') FROM pg_policy p JOIN pg_class c ON c.oid=p.polrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2) objects ORDER BY descriptor").bind(&req.schema).bind(&req.table).fetch_all(&mut *conn).await?;
    recipe_digest(info,&serde_json::json!({"objects":objects}))
}
fn recipe_preview_scope(recipe:&crate::generated_recipe::Recipe)->CmdResult<String> {
    let bytes=serde_json::to_vec(recipe)?;
    Ok(format!("typed/v1:recipe-preview:{:x}",Sha256::digest(bytes)))
}
/// Finish the owned preview after run_pinned has drained work and closed its session.
/// stop_confirmed is cancellation evidence, not a success/close flag.
fn finish_recipe_preview(record:&BatchRecord,result:Result<MultiResult>,stop_confirmed:bool)->CmdResult<()> {
    record.publish(result.map_err(|error|safe_error(&error)),stop_confirmed);
    let snapshot=record.snapshot(0);
    let outcome=snapshot.outcome.ok_or_else(||AppError::coded("RECIPE_PREVIEW_OUTCOME_MISSING","样本任务缺少完成回执"))?;
    if outcome.state!=crate::execution_contracts::ExecutionState::Succeeded {
        return Err(outcome.error.unwrap_or_else(||AppError::coded("EXECUTION_EFFECTS_UNKNOWN","样本任务没有确认成功")));
    }
    if outcome.resources_pending||snapshot.status.resources_pending||snapshot.transaction_state!="none"||snapshot.metrics.sent!=0 {
        return Err(AppError::coded("EXECUTION_EFFECTS_UNKNOWN","样本物理会话排空/回滚未确认或出现写入；不能返回预览"));
    }
    Ok(())
}
/// Samples are produced under the same native metadata/object locks as execution, then rolled back.
/// This command never returns an execution grant; the normalized recipe is separately confirmed
/// through register_batch_execution's existing principal/owner binding and one-time CAS.
#[tauri::command]
pub async fn preview_generated_recipe(window:WebviewWindow,state:State<'_,AppState>,mut recipe:crate::generated_recipe::Recipe,owner_id:String)->CmdResult<crate::generated_recipe::Preview> {
    let _activity=state.activity.enter()?;recipe.validate(true).map_err(AppError::from)?;
    let cfg=saved_config(&state,&recipe.connection_id)?;
    let mut checked=recipe.clone();checked.schema_fingerprint="0".repeat(64);
    let request=BatchExecutionRequest{connection_id:recipe.connection_id.clone(),namespace:recipe.schema.clone(),owner_id:owner_id.clone(),timeout_ms:None,task:BatchTask::GeneratedRecipe{recipe:checked}};
    require_request(&request,&cfg)?;
    let prepared=crate::db::runtime::prepare_at(&cfg,state.store.data_root())?;
    if prepared.http.is_some()||cfg.params.starts_with(crate::driver_manager::PARAMS_PREFIX){return Err(AppError::coded("BATCH_NATIVE_REQUIRED","recipe 预览要求真实原生物理会话"));}
    let binding=execution_binding(&state,&cfg,window.label(),&owner_id,&recipe.schema)?;
    let lease=crate::generated_recipe::Lease::acquire("native-recipe-single-writer".into()).map_err(AppError::from)?;
    let _ordinary=state.manual_transactions.ordinary(&[recipe.connection_id.as_str()]).await?;
    state.executions.require_quiescent(&recipe.connection_id)?;
    let conn=batch_connection(window.app_handle().clone(),recipe.connection_id.clone(),recipe.schema.clone(),cfg.kind.is_pg_family()&&!recipe.schema.is_empty()).await?;
    let latest=saved_config(&state,&recipe.connection_id)?;
    if execution_binding(&state,&latest,window.label(),&owner_id,&recipe.schema)?!=binding||latest.read_only{return Err(AppError::coded("EXECUTION_CONTEXT_MISMATCH","预览前连接目标或只读策略变化"));}
    // A local registry is sufficient for a read-only sample; no persistent write ticket is exposed.
    let registry=execution::Registry::default();let preview_scope=recipe_preview_scope(&recipe)?;
    let grant=registry.grant_typed_scope(binding.clone(),&preview_scope)?;
    let entry=registry.register_typed_scope(&grant.execution_id,&grant.token,binding.clone(),&preview_scope,None,state.store.settings().query_timeout_secs)?;
    recipe.schema_fingerprint.clear();let sample=Arc::new(Mutex::new(None));
    let payload=Payload::Recipe(RecipeWork{recipe:recipe.clone(),preview:true,sample:sample.clone(),lease:Some(lease)});
    let ticket=BatchExecutionTicket{context:BatchContext{execution_id:entry.id.clone(),connection_id:binding.connection,namespace:binding.namespace,owner_id:binding.owner},task_kind:BatchTaskKind::GeneratedRecipe,execution_grant:grant.token,expires_in_ms:grant.expires_in_ms,summary:"原生 recipe 只读样本".into(),total_rows:Some(recipe.total),source_bytes:None};
    let record=Arc::new(BatchRecord{ticket,entry,payload:Mutex::new(None),ordinary:Mutex::new(None),started:AtomicBool::new(true),wake:tokio::sync::Notify::new(),registered:Instant::now(),progress:Mutex::new(Progress{scanned:0,sent:0,executed:0,errors:0,scan_complete:false,parse_error:None,affected:0,committed_prefix:None,transaction:"none".into(),logs:VecDeque::new(),bytes:0,next:0,dropped:0,outcome:None})});
    record.entry.start();let(result,stop_confirmed)=run_pinned(conn,cfg.kind,record.clone(),payload).await;
    finish_recipe_preview(&record,result,stop_confirmed)?;
    let result=sample.lock().take().ok_or_else(||AppError::coded("RECIPE_PREVIEW_MISSING","原生样本未生成"));result
}

#[cfg(test)]
#[path="generated_recipe_tests.rs"]
mod recipe_tests;
