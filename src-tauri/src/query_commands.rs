//! PL21 reads C1-owned evidence and produces the same one-shot C2 plan as table editing.
//! No IPC accepts a source table, old values, lineage proof, generation, or SQL.
use std::sync::Arc;
use serde::Deserialize;
use tauri::State;
use crate::commands::{self, AppState};
use crate::error::{AppError, CmdResult};
use crate::execution::{Binding, Entry};
use crate::execution_contracts::ExecutionState;
use crate::mutation::{MutationPlan, PlanInternal};
use crate::query_execution::{QueryCapture, QuerySlot, QueryWriteGuard};
use crate::query_result_mutation::{self, QueryRowChange};
use crate::query_result_store::RegisteredResult;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QueryContext {
    pub execution_id: String,
    pub connection_id: String,
    pub owner_id: String,
    pub namespace: String,
    pub result_index: usize,
}
fn execution_binding(state: &AppState, principal: &str, context: &QueryContext) -> CmdResult<Binding> {
    if context.result_index >= 128 || context.owner_id.is_empty() || context.owner_id.len() > 256
        || context.connection_id.is_empty() || context.namespace.contains('\0') {
        return Err(AppError::coded("QUERY_CONTEXT_INVALID", "查询页面上下文无效"));
    }
    let cfg = commands::saved_config(state, &context.connection_id)?;
    commands::execution_binding(&state, &cfg, principal, &context.owner_id, &context.namespace)
}
fn authorized_capture(state: &AppState, principal: &str, context: &QueryContext) -> CmdResult<(Arc<Entry>, Arc<QueryCapture>, QuerySlot)> {
    let binding = execution_binding(state, principal, context)?;
    let entry = state.executions.authorized(&context.execution_id, &binding)?;
    let status = entry.status();
    if status.state != ExecutionState::Succeeded || status.resources_pending {
        return Err(AppError::coded("QUERY_RESULT_NOT_READY", "执行尚未确认成功或资源仍在隔离；没有发布行写入证据"));
    }
    let capture = entry.query_capture().ok_or_else(|| AppError::coded("QUERY_RESULT_READ_ONLY", "本次结果没有已验证的单表来源，保留只读显示"))?;
    let slot = capture.slot(context.result_index).map_err(AppError::from)?;
    if slot.binding.principal != principal || slot.binding.owner_id != context.owner_id
        || slot.binding.connection_id != context.connection_id || slot.binding.endpoint_fingerprint != binding.fingerprint
        || slot.binding.source_execution_id != context.execution_id || slot.binding.result_index != context.result_index {
        return Err(AppError::coded("QUERY_CONTEXT_CHANGED", "来源证据与当前窗口、页面或实际端点不一致"));
    }
    state.query_results.require_current(&slot.binding).map_err(AppError::from)?;
    Ok((entry, capture, slot))
}

pub(crate) fn snapshot_service(state: &AppState, principal: &str, context: &QueryContext) -> CmdResult<RegisteredResult> {
    let _activity = state.activity.enter()?;
    let (_, _, slot) = authorized_capture(state, principal, context)?;
    if let Some(token) = &slot.result_token {
        state.query_results.store.snapshot(token, &slot.binding).map_err(AppError::from)
    } else {
        Ok(RegisteredResult { editability: crate::query_result_store::readonly(&slot.binding, &slot.read_only_reason),
            rows: Vec::new(), loaded_rows: 0, total_rows: slot.total_rows })
    }
}
#[tauri::command]
pub fn query_result_snapshot(window: tauri::WebviewWindow, state: State<'_, AppState>, context: QueryContext) -> CmdResult<RegisteredResult> {
    snapshot_service(&state, window.label(), &context)
}

pub(crate) async fn preview_service(state: &AppState, principal: &str, context: &QueryContext, change: &QueryRowChange) -> CmdResult<MutationPlan> {
    let _activity = state.activity.enter()?;
    let _ordinary = state.manual_transactions.ordinary(&[&context.connection_id]).await?;
    let (entry, _, slot) = authorized_capture(state, principal, context)?;
    if slot.result_token.as_deref() != Some(change.result_token.as_str()) {
        return Err(AppError::coded("QUERY_CONTEXT_CHANGED", "写入请求的结果令牌不属于当前已认证页面"));
    }
    let cfg = commands::saved_config(state, &context.connection_id)?;
    if cfg.read_only { return Err(AppError::coded("READ_ONLY_VIOLATION", "保存连接为只读；没有生成行写入计划")); }
    state.executions.require_quiescent(&context.connection_id)?;
    let db = commands::scoped_db(state, &context.connection_id, &context.namespace).await?;
    if !matches!(db.as_ref(), crate::db::Db::Sqlite(_))
        || !state.connection_runtime.matches(&context.connection_id, &slot.binding.endpoint_fingerprint, &db) {
        return Err(AppError::coded("QUERY_CONTEXT_CHANGED", "实际物理适配器或连接已变化；没有生成计划"));
    }
    let mut prepared = query_result_mutation::prepare_row_plan(db.as_ref(), &state.query_results.store, &slot.binding, change, cfg.read_only)
        .await.map_err(AppError::from)?;
    // No fresh binding replaces the reviewed source: compare the same server slot after every await.
    let (_, _, current) = authorized_capture(state, principal, context)?;
    let final_cfg = commands::saved_config(state, &context.connection_id)?;
    if current.binding != slot.binding || current.result_token != slot.result_token || final_cfg.read_only
        || !state.connection_runtime.matches(&context.connection_id, &slot.binding.endpoint_fingerprint, &db) {
        return Err(AppError::coded("QUERY_CONTEXT_CHANGED", "预览期间来源、权限或实际物理连接已变化"));
    }
    prepared.source.execution_namespace = entry.binding.namespace.clone();
    query_result_mutation::validate_context(&state.query_results.store, &prepared.source, &current.binding).map_err(AppError::from)?;
    let mut plan = prepared.plan;
    plan.query_source = Some(prepared.source);
    let mut preview = MutationPlan { plan_id: String::new(), sql: plan.sql.clone(), binds: plan.binds.clone(),
        base_revision: plan.base_revision.clone(), expected_rows: plan.expected_rows,
        summary: format!("{} {}.{} 的 1 行；按原查询行令牌定位并在同一事务中校验原值和读回", plan.kind, plan.schema, plan.table) };
    preview.plan_id = state.mutations.put(plan).map_err(AppError::from)?;
    Ok(preview)
}
#[tauri::command]
pub async fn preview_query_row_mutation(window: tauri::WebviewWindow, state: State<'_, AppState>, context: QueryContext, change: QueryRowChange) -> CmdResult<MutationPlan> {
    preview_service(&state, window.label(), &context, &change).await
}

/// Called by the existing C2 execute command, after consuming the existing one-shot plan.
/// This guard also prevents a concurrent same-page query from replacing the old values during DML.
pub(crate) fn pin_plan(state: &AppState, plan: &PlanInternal, principal: &str, owner: &str) -> CmdResult<Option<QueryWriteGuard>> {
    let Some(source) = &plan.query_source else { return Ok(None); };
    if plan.jdbc.is_some() || plan.principal != principal || plan.owner_id != owner
        || source.binding.principal != principal || source.binding.owner_id != owner
        || source.binding.connection_id != plan.connection_id || source.binding.endpoint_fingerprint != plan.endpoint_fingerprint
        || source.source.schema != plan.schema || source.source.table != plan.table || source.source.metadata_revision != plan.base_revision {
        return Err(AppError::coded("QUERY_CONTEXT_CHANGED", "冻结计划的查询来源与实际 C2 上下文不一致"));
    }
    let context = QueryContext { execution_id: source.binding.source_execution_id.clone(), connection_id: plan.connection_id.clone(),
        owner_id: owner.into(), namespace: source.execution_namespace.clone(), result_index: source.binding.result_index };
    let (_, _, slot) = authorized_capture(state, principal, &context)?;
    if slot.binding != source.binding || slot.result_token.as_deref() != Some(source.result_token.as_str()) {
        return Err(AppError::coded("QUERY_CONTEXT_CHANGED", "查询已替换或释放；没有发送 DML"));
    }
    let guard = state.query_results.pin_write(&slot.binding).map_err(AppError::from)?;
    query_result_mutation::validate_context(&state.query_results.store, source, &slot.binding).map_err(AppError::from)?;
    Ok(Some(guard))
}
pub(crate) fn invalidate_plan(state: &AppState, plan: &PlanInternal) {
    if let Some(source) = &plan.query_source {
        let _ = state.query_results.store.invalidate(&source.result_token, &source.binding);
        let binding = Binding { principal: source.binding.principal.clone(), owner: source.binding.owner_id.clone(),
            connection: source.binding.connection_id.clone(), fingerprint: source.binding.endpoint_fingerprint.clone(), namespace: source.execution_namespace.clone() };
        if let Ok(entry) = state.executions.authorized(&source.binding.source_execution_id, &binding) {
            if let Some(capture) = entry.query_capture() { capture.release(); }
        }
    }
}
/// Release is idempotent for an old result and cannot revoke a newer execution on the same page.
pub(crate) fn release_service(state: &AppState, principal: &str, context: &QueryContext) -> CmdResult<()> {
    let _activity = state.activity.enter()?;
    let binding = execution_binding(state, principal, context)?;
    let entry = state.executions.authorized(&context.execution_id, &binding)?;
    state.query_results.release_execution(&binding, &context.execution_id).map_err(AppError::from)?;
    if let Some(capture) = entry.query_capture() { capture.release(); }
    Ok(())
}
#[tauri::command]
pub fn release_query_result(window: tauri::WebviewWindow, state: State<'_, AppState>, context: QueryContext) -> CmdResult<()> {
    release_service(&state, window.label(), &context)
}

#[cfg(test)]
#[path="query_commands_tests.rs"]
mod tests;
