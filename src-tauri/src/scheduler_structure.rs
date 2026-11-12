//! Scheduler adapter for the existing owned SQLite structure service and C1 executor.
//! Registered by scheduler_tools; all SQL still goes through the same controlled C1 service.
//! Approval hashes contain business evidence only; transient reviews and execution IDs never enter them.
use std::{collections::HashMap, sync::OnceLock, time::Duration};
use anyhow::{anyhow, ensure, Result};
use parking_lot::Mutex;
use crate::{
    commands::{self, AppState}, controlled_execution::{self, ControlledWork},
    execution::Binding, execution_contracts::{ExecutionOutcome, ExecutionRequest, ExecutionState},
    scheduler::*, scheduler_desktop::state_access::SchedulerStateAccess,
    structure_commands, structure_execution::metadata::{self, StructurePreflight},
    structure_sync::{StructureReviewBinding, StructureSyncRequest},
};

struct Observed { preflight: StructurePreflight, binding: StructureReviewBinding }
#[derive(Clone)]
struct ExecutionProof { access: SchedulerStateAccess, id: String, binding: Binding }
fn proofs() -> &'static Mutex<HashMap<String, ExecutionProof>> {
    static PROOFS: OnceLock<Mutex<HashMap<String, ExecutionProof>>> = OnceLock::new();
    PROOFS.get_or_init(Default::default)
}
fn key(context: &ActionExecutionContext) -> String { format!("{}/{}", context.run_id, context.action_id) }

fn validate_options(template: &ActionTemplate, options: &StructureSyncRequest) -> Result<()> {
    validate_template(template)?;
    ensure!(template.kind == ActionKind::StructureSync && !template.read_only
        && template.connection_id == options.target_connection && template.namespace == options.target_schema,
        "SCHEDULE_STRUCTURE_SCOPE_INVALID");
    ensure!(!options.allow_drop && options.source_revision.is_empty() && options.target_revision.is_empty()
        && options.review_digest.is_empty(), "SCHEDULE_STRUCTURE_TEMPLATE_INVALID: 模板不得携带临时 review 或旧 CAS");
    let TemplateConfiguration::Tool { options_json } = &template.configuration else { return Err(anyhow!("SCHEDULE_TEMPLATE_KIND")); };
    let value: serde_json::Value = serde_json::from_str(options_json)?;
    let stored: StructureSyncRequest = serde_json::from_value(value.get("options").cloned()
        .ok_or_else(|| anyhow!("SCHEDULE_STRUCTURE_TEMPLATE_INVALID"))?)?;
    ensure!(value.get("kind").and_then(|v| v.as_str()) == Some("structure_sync")
        && serde_json::to_value(&stored)? == serde_json::to_value(options)?, "SCHEDULE_STRUCTURE_TEMPLATE_CHANGED");
    Ok(())
}

/// The original report remains available to C1. Only this projection enters approval digests.
/// In particular review_binding/digest, observation fields and request_sql are deliberately absent.
fn semantic_revision(options: &StructureSyncRequest, observed: &Observed) -> Result<String> {
    let p = &observed.preflight; let r = &p.report;
    ensure!(r.target_exists && p.target_identity.is_some() && r.alterable && r.unsupported.is_empty(),
        "SCHEDULE_STRUCTURE_PLAN_UNPROVEN: 目标不存在、元数据未知或不在已认证原生子集");
    ensure!(is_digest(&r.source_revision) && is_digest(&r.target_revision), "SCHEDULE_STRUCTURE_REVISION_REQUIRED");
    ensure!(r.changes.iter().all(|c| c.risk == "safe" && matches!(c.kind.as_str(), "add_column" | "index")),
        "SCHEDULE_DESTRUCTIVE_PLAN_REQUIRED: 结构任务只批准新增列和普通索引");
    ensure!((r.steps.is_empty() && r.changes.is_empty() && p.request_sql.is_empty() && p.observation_only)
        || (!r.steps.is_empty() && !p.request_sql.is_empty() && !p.observation_only), "SCHEDULE_STRUCTURE_REVIEW_INCOMPLETE");
    digest(&("SCHEDULE_STRUCTURE_BUSINESS_V1", options, &r.source_revision, &r.target_revision,
        &p.source_identity, &p.target_identity, p.same_database, &p.source_consistency,
        (&r.target_exists, &r.changes, &r.warnings, r.alterable, &r.unsupported, &r.summary),
        &r.final_design, &r.steps))
}
fn scope(template: &ActionTemplate, options: &StructureSyncRequest, observed: &Observed) -> Result<ActionScope> {
    validate_options(template, options)?;
    let permission_revision = semantic_revision(options, observed)?;
    let b = &observed.binding;
    let mut endpoints = vec![(b.source_connection_id.clone(), b.source_schema.clone(), b.source_fingerprint.clone()),
        (b.target_connection_id.clone(), b.target_schema.clone(), b.target_fingerprint.clone())];
    endpoints.sort(); endpoints.dedup();
    let mut connection_ids = vec![b.source_connection_id.clone(), b.target_connection_id.clone()];
    connection_ids.sort(); connection_ids.dedup();
    let result = ActionScope { template_ref: template.id.clone(), template_revision: template.revision.clone(),
        template_digest: digest(template)?, connection_id: options.target_connection.clone(), connection_ids,
        endpoint_fingerprint: digest(&endpoints)?, namespace: options.target_schema.clone(), read_only: false,
        permission_revision, destructive_limit: None };
    result.validate()?; Ok(result)
}

async fn observe(state: &AppState, options: &StructureSyncRequest, principal: &str, owner: &str) -> Result<Observed> {
    let binding = structure_commands::review_binding(state, options, principal, owner)?;
    let source_cfg = commands::saved_config(state, &options.source_connection)?;
    let target_cfg = commands::saved_config(state, &options.target_connection)?;
    ensure!(!target_cfg.read_only, "READ_ONLY_VIOLATION");
    // ordinary() owns a mutex for each sorted resource. It is not reentrant; preflight
    // consumes and releases it before the controlled service obtains its own lease.
    let ordinary = state.manual_transactions.ordinary(&[&options.source_connection, &options.target_connection]).await?;
    state.executions.require_quiescent(&options.source_connection)?;
    state.executions.require_quiescent(&options.target_connection)?;
    let source = commands::scoped_db(state, &options.source_connection, &options.source_schema).await?;
    let target = commands::scoped_db(state, &options.target_connection, &options.target_schema).await?;
    structure_commands::recheck_runtime(state, options, &binding, &source, &target)?;
    let preflight = metadata::preflight(source.clone(), source_cfg.kind, target.clone(), target_cfg.kind,
        ordinary, options.clone(), binding.clone(), target_cfg.read_only).await?;
    structure_commands::recheck_runtime(state, options, &binding, &source, &target)?;
    Ok(Observed { preflight, binding })
}
pub(crate) async fn resolve_scope(state: &AppState, template: &ActionTemplate, options: &StructureSyncRequest) -> Result<ActionScope> {
    validate_options(template, options)?;
    let _activity = state.activity.enter()?;
    let observed = observe(state, options, "scheduler:structure-review", "review:scheduler-structure").await?;
    scope(template, options, &observed)
}
fn confirmation() -> ActionReceipt {
    let mut result = ActionReceipt::failed("SCHEDULE_SCOPE_CHANGED");
    result.status = ActionStatus::AwaitingConfirmation; result
}
fn stopped() -> ActionReceipt {
    let mut result = ActionReceipt::failed("SCHEDULE_CANCELLED_BEFORE_SEND"); result.status = ActionStatus::Cancelled; result
}
fn project(outcome: &ExecutionOutcome, verified: bool) -> ActionReceipt {
    let proved_none = matches!(outcome.effects.as_str(), "none" | "no_effect") || outcome.transaction_state == "rolled_back";
    let unknown = outcome.state == ExecutionState::Unknown || outcome.resources_pending
        || (outcome.state == ExecutionState::Succeeded && !verified)
        || (outcome.state != ExecutionState::Succeeded && !proved_none);
    let status = if unknown { ActionStatus::Unknown } else { match outcome.state {
        ExecutionState::Succeeded => ActionStatus::Succeeded, ExecutionState::Cancelled => ActionStatus::Cancelled,
        ExecutionState::TimedOut => ActionStatus::TimedOut, _ => ActionStatus::Failed,
    }};
    ActionReceipt { status, verified: !unknown, effects: if unknown { ActionEffects::Unknown }
        else if proved_none { ActionEffects::None } else { ActionEffects::Committed }, resources_pending: outcome.resources_pending,
        execution_id: Some(outcome.execution_id.clone()), affected_rows: None,
        error_code: outcome.error.as_ref().map(|error| safe_code(&error.code)) }
}

/// Delegates to the same register/Entry/owned-worker/finish path used by the structure IPC.
/// The fresh preflight may form a new runtime review only AFTER its full semantic scope
/// equals the original approval. That exact preflight's two opaque versions remain frozen.
pub(crate) async fn execute(access: SchedulerStateAccess, template: ActionTemplate, options: StructureSyncRequest,
    approved: ActionScope, context: ActionExecutionContext) -> Result<ActionReceipt> {
    ensure!(context.origin == "job" && context.mode == RunnerMode::Gui && context.instance.mode == RunnerMode::Gui
        && context.owner_id.starts_with("job:") && !context.owner_id.starts_with("review:")
        && context.owner_id.ends_with(&format!("/{}/{}", context.run_id, context.action_id)), "SCHEDULE_EXECUTION_CONTEXT_INVALID");
    canonical_uuid(&context.run_id)?; canonical_uuid(&context.action_id)?;
    validate_options(&template, &options)?;
    if context.stop_reason().is_some() { return Ok(stopped()); }
    let state = access.get(); let _activity = state.activity.enter()?;
    let principal = format!("scheduler:{}", context.instance.instance_id);
    let observed = match observe(&state, &options, &principal, &context.owner_id).await {
        Ok(observed) => observed, Err(_) => return Ok(confirmation()),
    };
    let current = match scope(&template, &options, &observed) { Ok(scope) => scope, Err(_) => return Ok(confirmation()) };
    if current != approved { return Ok(confirmation()); }
    if context.stop_reason().is_some() { return Ok(stopped()); }
    if observed.preflight.report.steps.is_empty() {
        // A reviewed complete no-change observation sends no DDL and creates no C1 execution.
        return Ok(ActionReceipt { status: ActionStatus::Succeeded, verified: true, effects: ActionEffects::None,
            resources_pending: false, execution_id: None, affected_rows: Some("0".into()), error_code: None });
    }
    let mut reviewed = options;
    reviewed.source_revision = observed.preflight.report.source_revision.clone();
    reviewed.target_revision = observed.preflight.report.target_revision.clone();
    reviewed.review_digest = observed.preflight.report.review_digest.clone();
    let marker = crate::structure_sync::review_request_sql(&reviewed)?;
    ensure!(marker == observed.preflight.request_sql, "SCHEDULE_STRUCTURE_MARKER_CHANGED");
    let mut request = ExecutionRequest { execution_id: String::new(), connection_id: reviewed.target_connection.clone(),
        namespace: reviewed.target_schema.clone(), owner_id: context.owner_id.clone(), sql: marker,
        transaction_owner: None, timeout_ms: Some(context.timeout_ms), origin: Some("job".into()),
        execution_grant: None, cancel_before_send: context.stop_reason().is_some() };
    commands::grant_execution_request(&state, &mut request, &principal)?;
    let cfg = commands::saved_config(&state, &reviewed.target_connection)?;
    let binding = commands::execution_binding(&state, &cfg, &principal, &context.owner_id, &reviewed.target_schema)?;
    structure_commands::validate_target(&observed.binding, &binding)?;
    let id = request.execution_id.clone(); let proof_key = key(&context);
    proofs().lock().insert(proof_key.clone(), ExecutionProof { access: access.clone(), id: id.clone(), binding: binding.clone() });
    // No outer OrdinaryLease remains here. The worker rechecks the original versions and
    // complete runtime review under BEGIN IMMEDIATE/source locks; no fresh fallback exists.
    let work = Box::pin(controlled_execution::execute(&state, request, &principal,
        ControlledWork::Structure { request: reviewed, binding: observed.binding }));
    tokio::pin!(work);
    let mut tick = tokio::time::interval(Duration::from_millis(25));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let result = loop { tokio::select! {
        value = &mut work => break value,
        _ = tick.tick() => {
            if let Some(reason) = context.stop_reason() {
                if let Ok(entry) = state.executions.authorized(&id, &binding) {
                    entry.request_stop(reason == SchedulerStopReason::Deadline);
                }
            }
        }
    }};
    let result = match result {
        Ok(response) => {
            let verified = response.structure.as_ref().is_some_and(|value| value.receipt.verified
                && !value.receipt.unverified && value.receipt.mismatches.is_empty());
            let receipt = project(&response.outcome, verified);
            if !receipt.resources_pending { proofs().lock().remove(&proof_key); }
            receipt
        }
        Err(error) => match state.executions.authorized(&id, &binding) {
            Err(missing) if missing.code == "EXECUTION_NOT_FOUND" => {
                proofs().lock().remove(&proof_key); ActionReceipt::failed(&safe_code(&error.code))
            }
            _ => ActionReceipt { status: ActionStatus::Unknown, verified: false, effects: ActionEffects::Unknown,
                resources_pending: true, execution_id: Some(id), affected_rows: None, error_code: Some(safe_code(&error.code)) },
        },
    };
    // Entry, not this adapter, continues to own Activity/dual ordinary/physical work when pending.
    result.validate()?; Ok(result)
}

/// Some(true/false) means this action belongs to this adapter. None lets the caller
/// delegate SQL and other tool receipts to their existing proof owners.
pub(crate) fn resources_drained(context: &ActionExecutionContext, receipt: &ActionReceipt) -> Result<Option<bool>> {
    let proof_key = key(context);
    let proof = proofs().lock().get(&proof_key).cloned();
    let Some(proof) = proof else { return Ok(None); };
    ensure!(receipt.execution_id.as_deref() == Some(proof.id.as_str()), "SCHEDULE_STRUCTURE_EXECUTION_MISMATCH");
    let state = proof.access.get();
    let Ok(entry) = state.executions.authorized(&proof.id, &proof.binding) else { return Ok(Some(false)); };
    let status = entry.status(); let drained = status.state.terminal() && !status.resources_pending;
    if drained { proofs().lock().remove(&proof_key); }
    Ok(Some(drained))
}

#[cfg(test)]
#[path = "scheduler_structure_tests.rs"]
mod tests;
