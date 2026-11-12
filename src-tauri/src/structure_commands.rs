//! Reviewed structure synchronization over the existing C1 registry and owned workers.
use std::sync::Arc;
use tauri::State;
use crate::{
    commands::{self, AppState}, controlled_execution::{self, ControlledWork}, db::Db,
    error::{AppError, CmdResult}, execution::Binding, execution_contracts::{ExecutionOutcome, ExecutionRequest},
    structure_execution::metadata::{self, StructurePreflight},
    structure_sync::{self, StructureReviewBinding, StructureSyncReceipt, StructureSyncRequest},
};

fn validate_input(request: &StructureSyncRequest, owner: &str) -> CmdResult<()> {
    if owner.trim().is_empty() || owner.len() > 128 || owner.contains('\0') {
        return Err(AppError::coded("EXECUTION_OWNER_INVALID", "结构同步必须归属有效页面"));
    }
    let parts = [&request.source_connection, &request.source_schema, &request.source_table,
        &request.target_connection, &request.target_schema, &request.target_table];
    if parts.iter().any(|value| value.is_empty() || value.len() > 1024 || value.contains('\0'))
        || request.columns.len() > 4096 || serde_json::to_vec(request).map_err(AppError::from)?.len() > 1024 * 1024 {
        return Err(AppError::coded("STRUCTURE_CONTEXT_INVALID", "双端对象身份/列映射缺失或超限，没有发送 DDL"));
    }
    Ok(())
}

/// Both fingerprints are resolved from current saved credentials/material by the server.
pub(crate) fn review_binding(state: &AppState, request: &StructureSyncRequest, principal: &str, owner: &str) -> CmdResult<StructureReviewBinding> {
    validate_input(request, owner)?;
    let source = commands::saved_config(state, &request.source_connection)?;
    let target = commands::saved_config(state, &request.target_connection)?;
    let source = commands::execution_binding(&state, &source, principal, owner, &request.source_schema)?;
    let target = commands::execution_binding(&state, &target, principal, owner, &request.target_schema)?;
    Ok(StructureReviewBinding { principal: principal.into(), owner: owner.into(),
        source_connection_id: source.connection, source_fingerprint: source.fingerprint,
        source_schema: source.namespace, source_table: request.source_table.clone(),
        target_connection_id: target.connection, target_fingerprint: target.fingerprint,
        target_schema: target.namespace, target_table: request.target_table.clone() })
}

pub(crate) fn validate_target(binding: &StructureReviewBinding, actual: &Binding) -> CmdResult<()> {
    if binding.principal != actual.principal || binding.owner != actual.owner || binding.target_connection_id != actual.connection
        || binding.target_schema != actual.namespace || binding.target_fingerprint != actual.fingerprint {
        return Err(AppError::coded("STRUCTURE_CONTEXT_CHANGED", "结构目标与当前窗口/页面/授权端点不同，没有发送 DDL"));
    }
    Ok(())
}

pub(crate) fn recheck_runtime(state: &AppState, request: &StructureSyncRequest, expected: &StructureReviewBinding, source: &Arc<Db>, target: &Arc<Db>) -> CmdResult<()> {
    if review_binding(state, request, &expected.principal, &expected.owner)? != *expected {
        return Err(AppError::coded("STRUCTURE_CONTEXT_CHANGED", "读取或排队期间双端配置/凭据材料改变，请明确重新预览"));
    }
    // These owned workers currently prove SQLite only. Never label an overridden/custom adapter as SQLite.
    if !matches!(source.as_ref(), Db::Sqlite(_)) || !matches!(target.as_ref(), Db::Sqlite(_)) {
        return Err(AppError::coded("STRUCTURE_CONTROLLED_DIALECT_UNSUPPORTED", "当前 owned 结构同步已认证 SQLite；此实际适配器尚未接入事务与来源校验"));
    }
    if !state.connection_runtime.matches(&request.source_connection, &expected.source_fingerprint, source)
        || !state.connection_runtime.matches(&request.target_connection, &expected.target_fingerprint, target) {
        return Err(AppError::coded("STRUCTURE_RUNTIME_CHANGED", "实际物理池不属于已审核双端配置，没有发送 DDL"));
    }
    Ok(())
}

pub type ControlledStructurePreview = StructurePreflight;

#[tauri::command]
pub async fn preview_controlled_structure_sync(window: tauri::WebviewWindow, state: State<'_, AppState>, request: StructureSyncRequest, owner_id: String) -> CmdResult<ControlledStructurePreview> {
    let _activity = state.activity.enter()?;
    let binding = review_binding(&state, &request, window.label(), &owner_id)?;
    let ordinary = state.manual_transactions.ordinary(&[&request.source_connection, &request.target_connection]).await?;
    state.executions.require_quiescent(&request.source_connection)?;
    state.executions.require_quiescent(&request.target_connection)?;
    let source_cfg = commands::saved_config(&state, &request.source_connection)?;
    let target_cfg = commands::saved_config(&state, &request.target_connection)?;
    let source = commands::scoped_db(&state, &request.source_connection, &request.source_schema).await?;
    let target = commands::scoped_db(&state, &request.target_connection, &request.target_schema).await?;
    recheck_runtime(&state, &request, &binding, &source, &target)?;
    let preflight = metadata::preflight(source.clone(), source_cfg.kind, target.clone(), target_cfg.kind, ordinary, request.clone(), binding.clone(), target_cfg.read_only).await?;
    recheck_runtime(&state, &request, &binding, &source, &target)?;
    Ok(preflight)
}

#[derive(serde::Serialize)]
pub struct StructureExecutionResponse { pub outcome: ExecutionOutcome, pub receipt: Option<StructureSyncReceipt> }
#[tauri::command]
pub async fn execute_structure_sync(window: tauri::WebviewWindow, state: State<'_, AppState>, request: ExecutionRequest, sync_request: StructureSyncRequest) -> CmdResult<StructureExecutionResponse> {
    let binding = review_binding(&state, &sync_request, window.label(), &request.owner_id)?;
    let response = controlled_execution::execute(&state, request, window.label(), ControlledWork::Structure { request: sync_request, binding }).await?;
    Ok(StructureExecutionResponse { outcome: response.outcome, receipt: response.structure })
}

#[cfg(test)]
#[path = "structure_integration_tests.rs"]
mod integration_tests;

#[cfg(test)]
mod tests {
    use super::*;
    fn binding() -> StructureReviewBinding { StructureReviewBinding { principal: "window".into(), owner: "page".into(), source_connection_id: "source".into(), source_fingerprint: "a".repeat(64), source_schema: "main".into(), source_table: " a ".into(), target_connection_id: "target".into(), target_fingerprint: "b".repeat(64), target_schema: "main".into(), target_table: " a ".into() } }
    #[test] fn target_requires_real_owner_window_namespace_and_material() {
        let expected = binding(); let target = Binding { principal: "window".into(), owner: "page".into(), connection: "target".into(), namespace: "main".into(), fingerprint: "b".repeat(64) };
        validate_target(&expected, &target).unwrap();
        for field in 0..5 { let mut actual = target.clone(); match field { 0 => actual.principal = "other".into(), 1 => actual.owner = "other".into(), 2 => actual.connection = "other".into(), 3 => actual.namespace = " main".into(), _ => actual.fingerprint = "c".repeat(64) } assert!(validate_target(&expected, &actual).is_err()); }
    }
    #[test] fn input_preserves_exact_names_and_rejects_missing_owner_or_oversize_mapping() {
        let mut request = StructureSyncRequest { source_connection: "source".into(), source_schema: "main".into(), source_table: " a ".into(), target_connection: "target".into(), target_schema: "main".into(), target_table: " a ".into(), columns: Vec::new(), allow_drop: false, source_revision: String::new(), target_revision: String::new(), review_digest: String::new() };
        validate_input(&request, "page").unwrap(); assert_eq!(request.source_table, " a "); assert!(validate_input(&request, " ").is_err()); request.columns = vec![("a".into(), "b".into()); 4097]; assert!(validate_input(&request, "page").is_err());
    }
}
