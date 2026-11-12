//! C0/C1 的唯一序列化契约。权限主体来自 Tauri 窗口，不来自 origin/UUID。
use serde::{Deserialize, Serialize};
use crate::{error::AppError, models::MultiResult};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExecutionRequest {
    pub execution_id: String,
    pub connection_id: String,
    #[serde(default)] pub namespace: String,
    pub owner_id: String,
    pub sql: String,
    #[serde(default)] pub transaction_owner: Option<String>,
    #[serde(default)] pub timeout_ms: Option<u64>,
    #[serde(default)] pub origin: Option<String>,
    #[serde(default)] pub execution_grant: Option<String>,
    #[serde(default)] pub cancel_before_send: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionGrant { pub execution_id: String, pub token: String, pub expires_in_ms: u64 }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionState { Queued, Running, CancelRequested, Succeeded, Failed, Cancelled, TimedOut, Unknown }
impl ExecutionState {
    pub fn terminal(self) -> bool { !matches!(self, Self::Queued | Self::Running | Self::CancelRequested) }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadlineInfo {
    pub execution_id: String,
    pub budget_ms: Option<u64>,
    pub elapsed_ms: u64,
    pub deadline_reached: bool,
    pub stop_confirmed: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionOutcome {
    pub execution_id: String,
    pub state: ExecutionState,
    pub result: Option<MultiResult>,
    pub error: Option<AppError>,
    pub deadline: DeadlineInfo,
    /// 已完成步骤可能有写入；不能据 cancelled/timed_out 推断回滚。
    pub effects: String,
    pub transaction_state: String,
    pub resources_pending: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelAcknowledgement {
    pub execution_id: String,
    pub accepted: bool,
    pub state: ExecutionState,
    pub message: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionStatus { pub execution_id: String, pub state: ExecutionState, pub deadline: DeadlineInfo, pub resources_pending: bool }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability { pub level: String, pub reason: String }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    pub version: u32,
    pub protocol: String,
    pub read_only: bool,
    pub arbitrary_sql: Capability,
    pub cancel: Capability,
    pub deadline: Capability,
    pub manual_transaction: Capability,
    pub safe_browse: Capability,
}
