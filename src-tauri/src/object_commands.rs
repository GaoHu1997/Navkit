//! PL-19/22 IPC：对象身份与元数据由实际连接读取；执行只走 controlled_execution/C1。
use crate::object_mutation::{self, ObjectMutationBaseline, ObjectMutationDraft, ObjectMutationPreview, ObjectMutationReceipt, ObjectMutationWork};
use crate::routine_create::{RoutineCreationBaseline, RoutineCreationDraft, RoutineCreationPreview, RoutineCreationReceipt};
use tauri::State;
use crate::{
    commands::{self, AppState},
    controlled_execution::{self, ControlledWork},
    error::{AppError, CmdResult},
    execution::Binding,
    execution_contracts::{ExecutionOutcome, ExecutionRequest},
    models::ConnectionConfig,
    object_catalog::{DatabaseObjectId, ObjectCatalogPage, ObjectDefinition},
    routine_call::{RoutineCallDraft, RoutineCallMetadata, RoutineCallPreview, RoutineCallDetails},
    routine_design::{RoutineObjectId, RoutineSnapshot, RoutineReplacementDraft, RoutineReplacementPreview, RoutineReceipt},
};

fn owner(owner: &str) -> CmdResult<()> {
    if owner.trim().is_empty() || owner.len() > 128 || owner.contains('\0') {
        return Err(AppError::coded("EXECUTION_OWNER_INVALID", "必须归属有效的真实页面身份"));
    }
    Ok(())
}
fn require_write(cfg: &ConnectionConfig) -> CmdResult<()> {
    if cfg.read_only { Err(AppError::coded("READ_ONLY_VIOLATION", "只读连接禁止可能有副作用的例程操作")) } else { Ok(()) }
}
/// 读出的数据只能属于读前的真实配置；名称/分组不参与端点指纹。
fn recheck_endpoint(state: &AppState, binding: &Binding) -> CmdResult<()> {
    let current = commands::saved_config(state, &binding.connection)?;
    let after = commands::execution_binding(&state, &current, &binding.principal, &binding.owner, &binding.namespace)?;
    if after != *binding {
        return Err(AppError::coded("EXECUTION_CONTEXT_CHANGED", "读取期间连接端点或权限已变化，结果未发布，请重新读取实际对象"));
    }
    Ok(())
}
fn exact_routine(object: &DatabaseObjectId) -> CmdResult<RoutineObjectId> {
    if !matches!(object.kind.as_str(), "function" | "procedure") || object.parent.is_some()
        || object.connection_id.is_empty() || object.schema.is_empty() || object.name.is_empty()
    {
        return Err(AppError::coded("OBJECT_ID_INVALID", "必须选择完整的单例程身份"));
    }
    let signature = object.identity_arguments.clone().ok_or_else(|| AppError::coded("OBJECT_SIGNATURE_REQUIRED", "未知签名不能按零参数例程读取，请从真实目录选择精确重载"))?;
    Ok(RoutineObjectId { connection_id: object.connection_id.clone(), schema: object.schema.clone(), name: object.name.clone(), kind: object.kind.clone(), identity_arguments: signature })
}

#[tauri::command]
pub async fn list_object_catalog(state: State<'_, AppState>, id: String, catalog: Option<String>, schema: String, kinds: Vec<String>, cursor: Option<String>) -> CmdResult<ObjectCatalogPage> {
    let _activity = state.activity.enter()?;
    let ordinary = state.manual_transactions.ordinary(&[&id]).await?;
    let cfg = commands::saved_config(&state, &id)?;
    let binding = commands::execution_binding(&state, &cfg, "metadata", "catalog", &schema)?;
    let db = commands::scoped_db(&state, &id, if cfg.kind.is_sqlite() { "" } else { &schema }).await?;
    let page = if cfg.kind.is_sqlite() && matches!(db.as_ref(), crate::db::Db::Sqlite(_)) {
        if !state.connection_runtime.matches(&id, &binding.fingerprint, &db) {
            return Err(AppError::coded("OBJECT_MUTATION_RUNTIME_CHANGED", "目录实际物理池与保存端点不符"));
        }
        let worker_db = db.clone(); let worker_cfg = cfg.clone(); let worker_binding = binding.clone();
        let worker_activity = state.activity.enter()?;
        crate::sql_history::spawn(async move {
            let _ordinary = ordinary; let _activity = worker_activity;
            let mut page = crate::object_catalog::list(worker_db.as_ref(), &worker_cfg.kind, &id, catalog.as_deref(), &schema, &kinds, cursor.as_deref(), &worker_binding.fingerprint, worker_cfg.read_only).await.map_err(AppError::from)?;
            let pool = match worker_db.as_ref() { crate::db::Db::Sqlite(pool) => pool, _ => unreachable!() };
            let parents: Vec<String> = crate::sql_history::query_scalar("SELECT s.name FROM main.sqlite_schema s JOIN pragma_table_list p ON p.schema='main' AND p.name=s.name COLLATE BINARY WHERE s.type='table' AND p.type='table' AND s.rootpage>0 AND s.sql IS NOT NULL LIMIT 4097")
                .fetch_all(pool).await?;
            if parents.len() <= 4096 { apply_trigger_drop_capabilities(&mut page, &worker_binding, worker_cfg.read_only, &parents); }
            Ok::<_, AppError>(page)
        }).await.map_err(|e| AppError::coded("OBJECT_MUTATION_METADATA_TASK_FAILED", format!("对象目录拥有任务未返回完整结果：{e}")))??
    } else {
        let _ordinary = ordinary;
        crate::object_catalog::list(db.as_ref(), &cfg.kind, &id, catalog.as_deref(), &schema, &kinds, cursor.as_deref(), &binding.fingerprint, cfg.read_only).await.map_err(AppError::from)?
    };
    recheck_endpoint(&state, &binding)?;
    if cfg.kind.is_sqlite() && !state.connection_runtime.matches(&cfg.id, &binding.fingerprint, &db) {
        return Err(AppError::coded("OBJECT_MUTATION_RUNTIME_CHANGED", "读取期间目录实际物理池发生变化"));
    }
    // 能力只是打开受控基线的入口；不改变catalog的目录CAS/分页revision。
    Ok(page)
}

fn apply_trigger_drop_capabilities(page: &mut ObjectCatalogPage, binding: &Binding, read_only: bool, parents: &[String]) {
    for descriptor in &mut page.objects {
        if !read_only && descriptor.definition_readable && object_mutation::validate_object(&descriptor.object, binding).is_ok()
            && descriptor.object.parent.as_ref().is_some_and(|parent| parents.contains(&parent.name)) {
            descriptor.actions.insert("drop".into(), crate::execution_contracts::Capability {
                level: "limited".into(), reason: "仅原生SQLite/main用户触发器；须读取原基线、后端预览与一次明确确认，经统一C1授权后同连接事务验证DROP".into(),
            });
        }
    }
}

#[tauri::command]
pub async fn get_object_definition(state: State<'_, AppState>, object: DatabaseObjectId) -> CmdResult<ObjectDefinition> {
    let _activity = state.activity.enter()?;
    let _ordinary = state.manual_transactions.ordinary(&[&object.connection_id]).await?;
    let cfg = commands::saved_config(&state, &object.connection_id)?;
    let binding = commands::execution_binding(&state, &cfg, "metadata", "object-definition", &object.schema)?;
    let db = commands::scoped_db(&state, &cfg.id, &object.schema).await?;
    let definition = crate::object_catalog::definition(db.as_ref(), &cfg.kind, &object).await.map_err(AppError::from)?;
    recheck_endpoint(&state, &binding)?;
    Ok(definition)
}

/// 新编辑入口保留catalog和精确签名；旧按名称聚合的只读命令不作为编辑基线。
#[tauri::command]
pub async fn read_exact_routine_design(state: State<'_, AppState>, object: DatabaseObjectId) -> CmdResult<RoutineSnapshot> {
    let routine = exact_routine(&object)?;
    let _activity = state.activity.enter()?;
    let _ordinary = state.manual_transactions.ordinary(&[&object.connection_id]).await?;
    let cfg = commands::saved_config(&state, &object.connection_id)?;
    let binding = commands::execution_binding(&state, &cfg, "metadata", "routine-design", &object.schema)?;
    let db = commands::scoped_db(&state, &cfg.id, &object.schema).await?;
    // definition核对实际catalog和完整身份，snapshot读取外层属性及revision。
    let definition = crate::object_catalog::definition(db.as_ref(), &cfg.kind, &object).await.map_err(AppError::from)?;
    let mut snapshot = crate::routine_design::snapshot(db.as_ref(), &cfg.kind, routine).await.map_err(AppError::from)?;
    if definition.backend_object_id != snapshot.backend_object_id {
        return Err(AppError::coded("OBJECT_BASELINE_CHANGED", "读取期间例程物理身份已变化，请重新读取"));
    }
    recheck_endpoint(&state, &binding)?;
    snapshot.source_fingerprint = Some(binding.fingerprint);
    Ok(snapshot)
}

#[tauri::command]
pub async fn get_routine_call_metadata(state: State<'_, AppState>, object: DatabaseObjectId) -> CmdResult<RoutineCallMetadata> {
    let _activity = state.activity.enter()?;
    let _ordinary = state.manual_transactions.ordinary(&[&object.connection_id]).await?;
    let cfg = commands::saved_config(&state, &object.connection_id)?;
    let binding = commands::execution_binding(&state, &cfg, "metadata", "routine-parameters", &object.schema)?;
    let db = commands::scoped_db(&state, &cfg.id, &object.schema).await?;
    let mut metadata = crate::routine_call::metadata(db.as_ref(), &cfg.kind, &object, cfg.read_only).await.map_err(AppError::from)?;
    recheck_endpoint(&state, &binding)?;
    metadata.source_fingerprint = Some(binding.fingerprint);
    Ok(metadata)
}

#[tauri::command]
pub async fn preview_routine_call(window: tauri::WebviewWindow, state: State<'_, AppState>, draft: RoutineCallDraft, owner_id: String) -> CmdResult<RoutineCallPreview> {
    owner(&owner_id)?;
    let _activity = state.activity.enter()?;
    let _ordinary = state.manual_transactions.ordinary(&[&draft.object.connection_id]).await?;
    let cfg = commands::saved_config(&state, &draft.object.connection_id)?;
    let binding = commands::execution_binding(&state, &cfg, window.label(), &owner_id, &draft.object.schema)?;
    crate::routine_design::validate_source_fingerprint(draft.source_fingerprint.as_deref(), &binding.fingerprint)?;
    require_write(&cfg)?;
    let db = commands::scoped_db(&state, &cfg.id, &draft.object.schema).await?;
    let mut metadata = crate::routine_call::metadata(db.as_ref(), &cfg.kind, &draft.object, cfg.read_only).await.map_err(AppError::from)?;
    recheck_endpoint(&state, &binding)?;
    metadata.source_fingerprint = Some(binding.fingerprint.clone());
    // 只填充fresh metadata来源；绝不补draft.source或替换编辑时metadata_revision。
    crate::routine_call::global().prepare(binding, metadata, draft, &cfg.kind).map_err(AppError::from)
}

#[derive(serde::Serialize)]
pub struct RoutineCallResponse { pub outcome: ExecutionOutcome, pub call: Option<RoutineCallDetails> }
#[tauri::command]
pub async fn execute_routine_call(window: tauri::WebviewWindow, state: State<'_, AppState>, plan_id: String, request: ExecutionRequest) -> CmdResult<RoutineCallResponse> {
    let response = controlled_execution::execute(&state, request, window.label(), ControlledWork::Call { plan_id }).await?;
    Ok(RoutineCallResponse { outcome: response.outcome, call: response.call })
}

#[tauri::command]
pub async fn preview_routine_replacement(state: State<'_, AppState>, draft: RoutineReplacementDraft) -> CmdResult<RoutineReplacementPreview> {
    let _activity = state.activity.enter()?;
    let _ordinary = state.manual_transactions.ordinary(&[&draft.object.connection_id]).await?;
    let cfg = commands::saved_config(&state, &draft.object.connection_id)?;
    let binding = commands::execution_binding(&state, &cfg, "metadata", "routine-replacement-preview", &draft.object.schema)?;
    crate::routine_design::validate_source_fingerprint(draft.source_fingerprint.as_deref(), &binding.fingerprint)?;
    require_write(&cfg)?;
    let db = commands::scoped_db(&state, &cfg.id, &draft.object.schema).await?;
    let preview = crate::routine_design::preview(db.as_ref(), &cfg.kind, &draft).await.map_err(AppError::from)?;
    recheck_endpoint(&state, &binding)?;
    Ok(preview)
}
#[derive(serde::Serialize)]
pub struct RoutineReplacementResponse { pub outcome: ExecutionOutcome, pub receipt: Option<RoutineReceipt> }
#[tauri::command]
pub async fn execute_routine_replacement(window: tauri::WebviewWindow, state: State<'_, AppState>, draft: RoutineReplacementDraft, request: ExecutionRequest) -> CmdResult<RoutineReplacementResponse> {
    let response = controlled_execution::execute(&state, request, window.label(), ControlledWork::Replace { draft }).await?;
    Ok(RoutineReplacementResponse { outcome: response.outcome, receipt: response.replacement })
}

/// 创建页面固定完整的“不存在”证明；主体和页面均由真实IPC上下文绑定。
#[tauri::command]
pub async fn read_routine_creation_baseline(window: tauri::WebviewWindow, state: State<'_, AppState>, object: RoutineObjectId, owner_id: String) -> CmdResult<RoutineCreationBaseline> {
    owner(&owner_id)?;
    let _activity = state.activity.enter()?;
    let _ordinary = state.manual_transactions.ordinary(&[&object.connection_id]).await?;
    let cfg = commands::saved_config(&state, &object.connection_id)?;
    require_write(&cfg)?;
    let binding = commands::execution_binding(&state, &cfg, window.label(), &owner_id, &object.schema)?;
    let db = commands::scoped_db(&state, &cfg.id, &object.schema).await?;
    let baseline = crate::routine_create::baseline(db.as_ref(), &cfg.kind, &object, &binding, cfg.read_only).await.map_err(AppError::from)?;
    recheck_endpoint(&state, &binding)?;
    Ok(baseline)
}
#[tauri::command]
pub async fn preview_routine_creation(window: tauri::WebviewWindow, state: State<'_, AppState>, draft: RoutineCreationDraft, owner_id: String) -> CmdResult<RoutineCreationPreview> {
    owner(&owner_id)?;
    let _activity = state.activity.enter()?;
    let _ordinary = state.manual_transactions.ordinary(&[&draft.object.connection_id]).await?;
    let cfg = commands::saved_config(&state, &draft.object.connection_id)?;
    require_write(&cfg)?;
    let binding = commands::execution_binding(&state, &cfg, window.label(), &owner_id, &draft.object.schema)?;
    crate::routine_create::validate_draft(&draft, &binding)?;
    let db = commands::scoped_db(&state, &cfg.id, &draft.object.schema).await?;
    let preview = crate::routine_create::preview(db.as_ref(), &cfg.kind, &draft, &binding, cfg.read_only).await.map_err(AppError::from)?;
    recheck_endpoint(&state, &binding)?;
    Ok(preview)
}
#[derive(serde::Serialize)]
pub struct RoutineCreationResponse { pub outcome: ExecutionOutcome, pub receipt: Option<RoutineCreationReceipt> }
#[tauri::command]
pub async fn execute_routine_creation(window: tauri::WebviewWindow, state: State<'_, AppState>, draft: RoutineCreationDraft, request: ExecutionRequest) -> CmdResult<RoutineCreationResponse> {
    let response = controlled_execution::execute(&state, request, window.label(), ControlledWork::Create { draft }).await?;
    Ok(RoutineCreationResponse { outcome: response.outcome, receipt: response.creation })
}

/// 对象DROP只认证原生SQLite/main。基础池读取不能先在IPC future中执行池元数据SQL。
fn mutation_binding(state: &AppState, object: &DatabaseObjectId, principal: &str, owner_id: &str) -> CmdResult<(ConnectionConfig, Binding)> {
    owner(owner_id)?;
    let cfg = commands::saved_config(state, &object.connection_id)?;
    require_write(&cfg)?;
    if !cfg.kind.is_sqlite() {
        return Err(AppError::coded("OBJECT_MUTATION_ADAPTER_UNSUPPORTED", "仅原生SQLite/main用户触发器DROP已认证"));
    }
    let binding = commands::execution_binding(state, &cfg, principal, owner_id, &object.schema)?;
    object_mutation::validate_object(object, &binding)?;
    Ok((cfg, binding))
}
fn recheck_mutation(state: &AppState, binding: &Binding, db: &std::sync::Arc<crate::db::Db>) -> CmdResult<()> {
    recheck_endpoint(state, binding)?;
    let cfg = commands::saved_config(state, &binding.connection)?;
    require_write(&cfg)?;
    if !cfg.kind.is_sqlite() || !matches!(db.as_ref(), crate::db::Db::Sqlite(_))
        || !state.connection_runtime.matches(&binding.connection, &binding.fingerprint, db) {
        return Err(AppError::coded("OBJECT_MUTATION_RUNTIME_CHANGED", "实际原生物理池不属于此窗口/页面的已保存端点，未发布对象计划"));
    }
    Ok(())
}

/// IPC与真实SQLite集成测试共用同一生产服务；principal只能来自实际window.label()。
pub(crate) async fn read_object_mutation_baseline_service(state: &AppState, principal: &str, object: DatabaseObjectId, owner_id: &str) -> CmdResult<ObjectMutationBaseline> {
    let _publication_activity = state.activity.enter()?;
    let (cfg, binding) = mutation_binding(state, &object, principal, owner_id)?;
    let ordinary = state.manual_transactions.ordinary(&[&cfg.id]).await?;
    state.executions.require_quiescent(&cfg.id)?;
    let db = commands::scoped_db(state, &cfg.id, "").await?;
    recheck_mutation(state, &binding, &db)?;
    // 另一个activity和ordinary移动到内部spawn；IPC失联也持有到真实读事务排空。
    let baseline = object_mutation::baseline(db.clone(), cfg.kind, binding.clone(), object,
        ordinary, state.activity.enter()?, cfg.read_only).await?;
    recheck_mutation(state, &binding, &db)?;
    Ok(baseline)
}
pub(crate) async fn preview_object_mutation_service(state: &AppState, principal: &str, draft: ObjectMutationDraft, owner_id: &str) -> CmdResult<ObjectMutationPreview> {
    let _publication_activity = state.activity.enter()?;
    let (cfg, binding) = mutation_binding(state, &draft.object, principal, owner_id)?;
    object_mutation::validate_draft(&draft, &binding)?;
    let ordinary = state.manual_transactions.ordinary(&[&cfg.id]).await?;
    state.executions.require_quiescent(&cfg.id)?;
    let db = commands::scoped_db(state, &cfg.id, "").await?;
    recheck_mutation(state, &binding, &db)?;
    let preview = object_mutation::preflight(db.clone(), cfg.kind, binding.clone(), draft,
        ordinary, state.activity.enter()?, cfg.read_only).await?;
    recheck_mutation(state, &binding, &db)?;
    Ok(preview)
}
#[derive(serde::Serialize)]
pub struct ObjectMutationResponse { pub outcome: ExecutionOutcome, pub receipt: Option<ObjectMutationReceipt> }
pub(crate) async fn execute_object_mutation_service(state: &AppState, principal: &str, work: ObjectMutationWork, request: ExecutionRequest) -> CmdResult<ObjectMutationResponse> {
    // 完整窗口/页面/命名空间/已签发marker先校验，不能先消费grant或注册Entry。
    let cfg = commands::saved_config(state, &request.connection_id)?;
    let binding = commands::execution_binding(state, &cfg, principal, &request.owner_id, &request.namespace)?;
    object_mutation::validate_request(&work, &binding, &request.sql)?;
    let response = controlled_execution::execute(state, request, principal, ControlledWork::Object { work }).await?;
    Ok(ObjectMutationResponse { outcome: response.outcome, receipt: response.object_mutation })
}

#[tauri::command]
pub async fn read_object_mutation_baseline(window: tauri::WebviewWindow, state: State<'_, AppState>, object: DatabaseObjectId, owner_id: String) -> CmdResult<ObjectMutationBaseline> {
    read_object_mutation_baseline_service(&state, window.label(), object, &owner_id).await
}
#[tauri::command]
pub async fn preview_object_mutation(window: tauri::WebviewWindow, state: State<'_, AppState>, draft: ObjectMutationDraft, owner_id: String) -> CmdResult<ObjectMutationPreview> {
    preview_object_mutation_service(&state, window.label(), draft, &owner_id).await
}
fn mutation_rejection(error: AppError, registered: bool) -> AppError {
    if registered { return AppError::coded("EXECUTION_EFFECTS_UNKNOWN", format!("{}: {}；此执行 ID 已登记，拒绝不能证明原执行未发送，禁止重放", error.code, error.message)).operation(error.operation_id.as_deref().unwrap_or("")); }
    if matches!(error.code.as_str(), "EXECUTION_ID_DUPLICATE" | "EXECUTION_FORBIDDEN" | "EXECUTION_EFFECTS_UNKNOWN" | "EXECUTION_CLEANUP_PENDING") { return error; }
    AppError::coded("OBJECT_MUTATION_NOT_REGISTERED", format!("{}: {}；执行尚未注册，没有发送删除 SQL", error.code, error.message))
}
#[tauri::command]
pub async fn execute_object_mutation(window: tauri::WebviewWindow, state: State<'_, AppState>, work: ObjectMutationWork, request: ExecutionRequest) -> CmdResult<ObjectMutationResponse> {
    let execution_id = request.execution_id.clone();
    execute_object_mutation_service(&state, window.label(), work, request).await.map_err(|error|
        mutation_rejection(error, state.executions.contains_execution(&execution_id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn object() -> DatabaseObjectId {
        DatabaseObjectId { connection_id: "c".into(), catalog: Some("physical-db".into()), schema: " a ".into(), kind: "function".into(), name: "f".into(), identity_arguments: Some(String::new()), parent: None }
    }
    #[test]
    fn duplicate_or_registered_execution_is_not_a_no_send_proof() {
        for code in ["EXECUTION_ID_DUPLICATE", "EXECUTION_FORBIDDEN", "EXECUTION_EFFECTS_UNKNOWN"] {
            assert_eq!(mutation_rejection(AppError::coded(code, "uncertain"), false).code, code);
        }
        assert_eq!(mutation_rejection(AppError::coded("OBJECT_MUTATION_PLAN_EXPIRED", "expired"), true).code, "EXECUTION_EFFECTS_UNKNOWN");
        assert_eq!(mutation_rejection(AppError::coded("EXECUTION_CONTEXT_INVALID", "invalid"), true).code, "EXECUTION_EFFECTS_UNKNOWN");
        assert_eq!(mutation_rejection(AppError::coded("OBJECT_MUTATION_PLAN_EXPIRED", "expired"), false).code, "OBJECT_MUTATION_NOT_REGISTERED");
    }
    #[tokio::test]
    async fn real_sqlite_catalog_only_certifies_readable_user_trigger_drop_entry() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        crate::sql_history::raw_sql("CREATE TABLE parent(id INT); CREATE VIEW kept_view AS SELECT id FROM parent; CREATE TRIGGER trig AFTER INSERT ON parent BEGIN SELECT 1; END; CREATE TRIGGER view_trig INSTEAD OF INSERT ON kept_view BEGIN SELECT 1; END")
            .execute(&pool).await.unwrap();
        let db = crate::db::Db::Sqlite(pool.clone());
        let binding = Binding { principal: "metadata".into(), owner: "catalog".into(), connection: "c".into(), namespace: "main".into(), fingerprint: "a".repeat(64) };
        let original = crate::object_catalog::list(&db, &crate::models::DbKind::of("sqlite"), "c", None, "main", &[], None, &binding.fingerprint, false).await.unwrap();
        let mut page = original.clone();
        apply_trigger_drop_capabilities(&mut page, &binding, false, &["parent".into()]);
        for descriptor in &page.objects {
            assert_eq!(descriptor.actions["drop"].level, if descriptor.object.name == "trig" { "limited" } else { "unsupported" });
            for action in ["rename", "enable", "disable", "restart"] { assert_eq!(descriptor.actions[action].level, "unsupported"); }
        }
        let mut readonly = original.clone(); apply_trigger_drop_capabilities(&mut readonly, &binding, true, &["parent".into()]);
        assert!(readonly.objects.iter().all(|d| d.actions["drop"].level == "unsupported"));
        let mut unreadable = original.clone(); for descriptor in &mut unreadable.objects { descriptor.definition_readable = false; }
        apply_trigger_drop_capabilities(&mut unreadable, &binding, false, &["parent".into()]);
        assert!(unreadable.objects.iter().all(|d| d.actions["drop"].level == "unsupported"));
        assert_eq!(page.revision, original.revision); assert_eq!(page.next_cursor, original.next_cursor);
        drop(db); pool.close().await; assert!(pool.is_closed());
    }

    #[test] fn exact_routine_requires_known_signature_and_keeps_empty_signature() {
        let mut value = object(); assert_eq!(exact_routine(&value).unwrap().identity_arguments, "");
        value.identity_arguments = None; assert_eq!(exact_routine(&value).unwrap_err().code, "OBJECT_SIGNATURE_REQUIRED");
    }
    #[test] fn exact_routine_rejects_trigger_parent_and_preserves_namespace_spaces() {
        let mut value = object(); assert_eq!(exact_routine(&value).unwrap().schema, " a ");
        value.parent = Some(crate::object_catalog::ObjectParent { schema: " a ".into(), kind: "table".into(), name: "t".into() });
        assert_eq!(exact_routine(&value).unwrap_err().code, "OBJECT_ID_INVALID");
    }
}
