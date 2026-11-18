//! 结构化操作只编排既有 C1 Entry；不接受客户端 SQL、不另建授权/取消引擎。
use crate::{
    commands::{self, AppState},
    error::{AppError, CmdResult},
    execution::Binding,
    execution_contracts::{ExecutionOutcome, ExecutionRequest, ExecutionState},
    execution_bound::{self, RoutineWork},
    models::MultiResult,
    routine_call::RoutineCallDetails,
    routine_create::{self, RoutineCreationDraft, RoutineCreationReceipt},
    routine_design::{RoutineReceipt, RoutineReplacementDraft},
};

pub(crate) enum ControlledWork {
    Call { plan_id: String },
    Replace { draft: RoutineReplacementDraft },
    Create { draft: RoutineCreationDraft },
    Table { work: crate::table_workflows::TableWork },
    Object { work: crate::object_mutation::ObjectMutationWork },
    Structure { request: crate::structure_sync::StructureSyncRequest, binding: crate::structure_sync::StructureReviewBinding },
}
impl ControlledWork {
    fn marker(&self, binding: &Binding) -> CmdResult<String> {
        match self {
            Self::Call { plan_id } => execution_bound::call_request_sql(plan_id),
            Self::Replace { draft } => execution_bound::replacement_request_sql(draft),
            Self::Create { draft } => routine_create::request_sql(draft),
            Self::Table { work } => crate::table_workflows::marker(work),
            Self::Object { work } => crate::object_mutation::request_sql(work, binding),
            Self::Structure { request, .. } => crate::structure_sync::review_request_sql(request).map_err(AppError::from),
        }
    }
    fn validate(&self, request: &ExecutionRequest, binding: &Binding) -> CmdResult<()> {
        if request.sql != self.marker(binding)? {
            return Err(AppError::coded("EXECUTION_PAYLOAD_MISMATCH", "已授权工作标识与完整操作草稿不一致，没有发送 SQL"));
        }
        match self {
            Self::Call { plan_id } => crate::routine_call::global().validate_plan(plan_id, binding).map_err(AppError::from)?,
            Self::Replace { draft } => crate::routine_design::validate_source_fingerprint(draft.source_fingerprint.as_deref(), &binding.fingerprint)?,
            Self::Create { draft } => routine_create::validate_draft(draft, binding)?,
            Self::Table { work } => crate::table_workflows::validate_work(work, binding)?,
            Self::Object { work } => crate::object_mutation::validate_request(work, binding, &request.sql)?,
            Self::Structure { request, binding: reviewed } => {
                crate::structure_commands::validate_target(reviewed, binding)?;
                crate::structure_sync::validate_review_request(request, reviewed).map_err(AppError::from)?;
            },
        }
        let matches = match self {
            Self::Call { .. } => true, // 这里只读校验；一次性消费仍由持有租约的执行器完成。
            Self::Table { .. } => true, // validate_work 已验证真实端点/namespace。
            Self::Object { .. } => true, // 后端一次性计划已核对完整窗口、页面、对象和端点。
            Self::Replace { draft } => draft.object.connection_id == binding.connection && draft.object.schema == binding.namespace,
            Self::Create { draft } => draft.object.connection_id == binding.connection && draft.object.schema == binding.namespace,
            Self::Structure { request, .. } => request.target_connection == binding.connection && request.target_schema == binding.namespace,
        };
        if !matches {
            return Err(AppError::coded("EXECUTION_PAYLOAD_SCOPE_MISMATCH", "草稿的连接/端点/命名空间与窗口授权不一致，没有发送 SQL"));
        }
        Ok(())
    }
}

#[derive(serde::Serialize)]
pub(crate) struct ControlledResponse {
    pub outcome: ExecutionOutcome,
    pub call: Option<RoutineCallDetails>,
    pub replacement: Option<RoutineReceipt>,
    pub creation: Option<RoutineCreationReceipt>,
    pub structure: Option<crate::structure_sync::StructureSyncReceipt>,
    pub table_design: Option<crate::table_workflows::TableDesignReceipt>,
    pub table_operation: Option<crate::table_workflows::TableOperationReceipt>,
    pub object_mutation: Option<crate::object_mutation::ObjectMutationReceipt>,
}
struct WorkReport {
    result: CmdResult<MultiResult>,
    confirmed: bool,
    call: Option<RoutineCallDetails>,
    replacement: Option<RoutineReceipt>,
    creation: Option<RoutineCreationReceipt>,
    structure: Option<crate::structure_sync::StructureSyncReceipt>,
}
impl WorkReport {
    fn error(error: AppError) -> Self {
        Self { result: Err(error), confirmed: false, call: None, replacement: None, creation: None, structure: None }
    }
    fn cancelled() -> Self {
        let mut report = Self::error(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "排队已停止，没有发送 SQL"));
        report.confirmed = true;
        report
    }
}

/// Invalid/expired payloads must not consume a grant or invalidate an existing query source.
/// Keep admission behind validation; the registered binding is checked again before dispatch.
fn validated_registration<T>(work: &ControlledWork, request: &ExecutionRequest, binding: &Binding, register: impl FnOnce() -> CmdResult<T>) -> CmdResult<T> {
    work.validate(request, binding)?;
    register()
}

/// principal 必须由外层 Tauri WebviewWindow.label() 提供，绝不取客户端 origin。
pub(crate) async fn execute(state: &AppState, request: ExecutionRequest, principal: &str, work: ControlledWork) -> CmdResult<ControlledResponse> {
    // 与普通 SQL 同一注册函数：消费一次性 grant，Entry 持有 activity 到真实 drain。
    let saved = commands::saved_config(state, &request.connection_id)?;
    let observed_binding = commands::execution_binding(state, &saved, principal, &request.owner_id, &request.namespace)?;
    let (cfg, binding, entry) = validated_registration(&work, &request, &observed_binding,
        || commands::register_execution(state, &request, principal))?;
    let transactional = matches!(&work, ControlledWork::Replace { .. } | ControlledWork::Create { .. } | ControlledWork::Structure { .. } | ControlledWork::Table { .. } | ControlledWork::Object { .. });
    let mut dispatched = false;
    let table_report = std::sync::Arc::new(parking_lot::Mutex::new((None, None)));
    let object_report = std::sync::Arc::new(parking_lot::Mutex::new(None));
    let history_context=crate::sql_history::Context { connection_id:cfg.id.clone(),connection_name:cfg.name.clone(),kind:cfg.kind.0.clone(),database:cfg.database.clone(),namespace:request.namespace.clone(),path:Some(state.store.data_root().join("logs/sql_log.jsonl")),origin:request.origin.clone().unwrap_or_else(||"tool".into()),operation_id:Some(request.execution_id.clone()) };
    let report: CmdResult<WorkReport> = crate::sql_history::scope(history_context,async {
        work.validate(&request, &binding)?;
        if entry.stopping() { return Ok(WorkReport::cancelled()); }
        if cfg.read_only {
            return Err(AppError::coded("READ_ONLY_VIOLATION", "只读连接禁止结构化写入与可能有副作用的例程操作，没有发送 SQL"));
        }
        if request.transaction_owner.is_some() {
            return Err(AppError::coded("CONTROLLED_MANUAL_TRANSACTION_UNSUPPORTED", "结构化操作尚未接入手动事务；请先明确提交或回滚，没有发送 SQL"));
        }
        let mut connection_ids = vec![&cfg.id[..]];
        if let ControlledWork::Structure { request, .. } = &work { connection_ids.push(&request.source_connection); }
        let ordinary = tokio::select! {
            biased;
            _ = entry.wait_stop() => return Ok(WorkReport::cancelled()),
            lease = state.manual_transactions.ordinary(&connection_ids) => lease.map_err(AppError::from)?,
        };
        for id in &connection_ids { state.executions.require_quiescent(id)?; }
        if entry.stopping() { return Ok(WorkReport::cancelled()); }
        // Object plans only support native SQLite main. Avoid an unowned namespace
        // probe here; their owned worker proves main before sending any object SQL.
        let namespace = if matches!(&work, ControlledWork::Object { .. }) && cfg.kind.is_sqlite() { "" } else { &request.namespace };
        let db = commands::scoped_db(state, &cfg.id, namespace).await?;
        // 与登记时一致的物理池键：MySQL 库级读取登记为 `id@database`，其余复用基础池。
        let physical = commands::physical_key(&cfg.id, namespace, db.as_ref());
        let current = commands::saved_config(state, &cfg.id)?;
        if commands::execution_binding(&state, &current, principal, &request.owner_id, &request.namespace)? != binding {
            return Err(AppError::coded("EXECUTION_CONTEXT_CHANGED", "排队期间连接配置已改变，没有发送 SQL"));
        }
        if entry.stopping() { return Ok(WorkReport::cancelled()); }
        dispatched = true;
        // 所有下层工作拥有物理连接+ordinary lease；supervise 不丢弃已发送 future。
        match work {
            ControlledWork::Call { plan_id } => {
                let r = execution_bound::run(db.as_ref(), &cfg.kind, entry.clone(), ordinary, RoutineWork::Call { plan_id }, cfg.read_only).await;
                Ok(WorkReport { result: r.result, confirmed: r.confirmed, call: r.call, replacement: r.replacement, creation: r.creation, structure: None })
            }
            ControlledWork::Replace { draft } => {
                let r = execution_bound::run(db.as_ref(), &cfg.kind, entry.clone(), ordinary, RoutineWork::Replace { draft }, cfg.read_only).await;
                Ok(WorkReport { result: r.result, confirmed: r.confirmed, call: r.call, replacement: r.replacement, creation: r.creation, structure: None })
            }
            ControlledWork::Create { draft } => {
                let r = execution_bound::run(db.as_ref(), &cfg.kind, entry.clone(), ordinary, RoutineWork::Create { draft }, cfg.read_only).await;
                Ok(WorkReport { result: r.result, confirmed: r.confirmed, call: None, replacement: None, creation: r.creation, structure: None })
            }
            ControlledWork::Table { work } => {
                if !state.connection_runtime.matches(&physical, &binding.fingerprint, &db) { return Err(AppError::coded("TABLE_RUNTIME_CHANGED", "实际物理池不属于已授权端点")); }
                let r = crate::table_workflows::run(db, cfg.kind.clone(), entry.clone(), ordinary, work, cfg.read_only).await;
                *table_report.lock() = (r.design, r.operation);
                Ok(WorkReport { result: r.result, confirmed: r.confirmed, call: None, replacement: None, creation: None, structure: None })
            }
            ControlledWork::Object { work } => {
                if !state.connection_runtime.matches(&physical, &binding.fingerprint, &db) { return Err(AppError::coded("OBJECT_MUTATION_RUNTIME_CHANGED", "实际物理池不属于对象计划的已授权端点")); }
                let r = crate::object_mutation::run(db, cfg.kind.clone(), entry.clone(), ordinary, work, request.sql.clone(), cfg.read_only).await;
                *object_report.lock() = Some((r.receipt, r.no_effect, r.transaction));
                Ok(WorkReport { result: r.result, confirmed: r.confirmed, call: None, replacement: None, creation: None, structure: None })
            }
            ControlledWork::Structure { request: sync_request, binding: reviewed } => {
                let source_cfg = commands::saved_config(state, &sync_request.source_connection)?;
                let source = commands::scoped_db(state, &sync_request.source_connection, &sync_request.source_schema).await?;
                crate::structure_commands::recheck_runtime(state, &sync_request, &reviewed, &source, &db)?;
                let r = if crate::structure_commands::server::native_pair(&source, &source_cfg.kind, &db, &cfg.kind) {
                    crate::structure_commands::server::run(source, source_cfg.kind, db, cfg.kind.clone(), entry.clone(), ordinary, sync_request, reviewed, cfg.read_only).await
                } else {
                    crate::structure_execution::run(source, source_cfg.kind, db, cfg.kind.clone(), entry.clone(), ordinary, sync_request, reviewed, cfg.read_only).await
                };
                Ok(WorkReport { result: r.result, confirmed: r.confirmed, call: None, replacement: None, creation: None, structure: r.receipt })
            }
        }
    }).await;
    let mut report = report.unwrap_or_else(WorkReport::error);
    // Entry.finish 是整个调用的唯一终态写入；迟到 worker 不再发布结果。
    let rolled_back = report.result.as_ref().err().is_some_and(|error| matches!(error.code.as_str(), "ROUTINE_STOPPED_ROLLED_BACK" | "ROUTINE_CREATION_ROLLED_BACK"));
    let mut outcome = entry.finish(report.result, report.confirmed, false);
    if !dispatched && !outcome.resources_pending {
        outcome.effects = "no_effect".into();
        outcome.transaction_state = "none".into();
    }
    if outcome.state == ExecutionState::Unknown || outcome.resources_pending {
        if dispatched && transactional { outcome.transaction_state = "unknown".into(); }
        report.call = None; report.replacement = None; report.creation = None; report.structure = None;
    } else {
        if rolled_back { outcome.effects = "no_effect".into(); outcome.transaction_state = "rolled_back".into(); }
        if outcome.state != ExecutionState::Succeeded { report.call = None; report.replacement = None; report.creation = None; }
        if let Some(receipt) = &report.structure {
            outcome.transaction_state = receipt.receipt.transaction.clone();
            if receipt.receipt.transaction == "rolled_back" { outcome.effects = "no_effect".into(); }
        } else if let Some(receipt) = &report.creation {
            outcome.transaction_state = receipt.transaction.clone();
        } else if let Some(receipt) = &report.replacement {
            outcome.transaction_state = receipt.transaction.clone();
        }
    }
    let mut object_mutation = None;
    if outcome.state != ExecutionState::Unknown && !outcome.resources_pending {
        if let Some((receipt, no_effect, transaction)) = object_report.lock().clone() {
            outcome.transaction_state = transaction;
            if no_effect { outcome.effects = "no_effect".into(); }
            if outcome.state == ExecutionState::Succeeded { object_mutation = receipt; }
        }
    }
    // Do not persist request.sql: controlled work may contain an operation marker, not SQL.
    let (table_design, table_operation) = if outcome.state == ExecutionState::Succeeded && !outcome.resources_pending { table_report.lock().clone() } else { (None, None) };
    Ok(ControlledResponse { outcome, call: report.call, replacement: report.replacement, creation: report.creation, structure: report.structure, table_design, table_operation, object_mutation })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(sql: String) -> ExecutionRequest {
        ExecutionRequest { execution_id: String::new(), connection_id: "c".into(), namespace: " a ".into(), owner_id: "tab".into(), sql, transaction_owner: None, timeout_ms: Some(1000), origin: None, execution_grant: None, cancel_before_send: false }
    }
    fn binding() -> Binding { Binding { principal: "actual-window".into(), owner: "tab".into(), connection: "c".into(), namespace: " a ".into(), fingerprint: "a".repeat(64) } }
    #[test]
    fn call_marker_requires_canonical_non_nil_plan() {
        for id in ["", "0", "00000000-0000-0000-0000-000000000000"] { assert!(ControlledWork::Call { plan_id: id.into() }.marker(&binding()).is_err()); }
        let work = ControlledWork::Call { plan_id: uuid::Uuid::new_v4().to_string() };
        assert_eq!(work.marker(&binding()).unwrap(), match &work { ControlledWork::Call { plan_id } => format!("ROUTINE_CALL_PLAN {plan_id}"), _ => unreachable!() });
        assert!(work.validate(&request(work.marker(&binding()).unwrap()), &binding()).is_err()); // UUID并不能替代后端已绑定计划。
    }
    #[test]
    fn authorized_arbitrary_sql_cannot_be_substituted_for_bound_work() {
        let work = ControlledWork::Call { plan_id: uuid::Uuid::new_v4().to_string() };
        assert_eq!(work.validate(&request("DROP TABLE items".into()), &binding()).unwrap_err().code, "EXECUTION_PAYLOAD_MISMATCH");
    }
    #[test]
    fn invalid_payload_does_not_consume_execution_grant() {
        let registry = crate::execution::Registry::default();
        let binding = binding();
        let mut req = request("DROP TABLE sentinel".into());
        registry.grant_request(&mut req, binding.clone()).unwrap();
        let work = ControlledWork::Call { plan_id: uuid::Uuid::new_v4().to_string() };
        let mut attempted = false;
        let rejected = validated_registration(&work, &req, &binding, || {
            attempted = true;
            registry.register(&req, binding.clone(), 1)
        });
        assert_eq!(rejected.err().unwrap().code, "EXECUTION_PAYLOAD_MISMATCH");
        assert!(!attempted, "payload rejection must precede registration/query-source invalidation");
        let entry = registry.register(&req, binding.clone(), 1).unwrap();
        assert_eq!(entry.id, req.execution_id, "rejected payload must leave the original grant unconsumed");
        entry.finish(Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "fixture never sent SQL")), true, false);
    }
    #[test]
    fn replacement_preserves_exact_namespace_binding() {
        let draft = RoutineReplacementDraft { object: crate::routine_design::RoutineObjectId { connection_id: "c".into(), schema: " a ".into(), name: "f".into(), kind: "function".into(), identity_arguments: "".into() }, base_revision: "baseline".into(), source_fingerprint: Some("a".repeat(64)), definition: "CREATE FUNCTION f() RETURNS int AS $$SELECT 1$$ LANGUAGE SQL".into(), allow_drop_recreate: false };
        let work = ControlledWork::Replace { draft };
        let req = request(work.marker(&binding()).unwrap());
        assert!(work.validate(&req, &binding()).is_ok());
        let mut trimmed = binding(); trimmed.namespace = "a".into();
        assert_eq!(work.validate(&req, &trimmed).unwrap_err().code, "EXECUTION_PAYLOAD_SCOPE_MISMATCH");
    }
}
