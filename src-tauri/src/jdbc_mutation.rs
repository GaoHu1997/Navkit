//! PL-20 adapter-side DTOs. This is not a second authorization/plan registry.
//! Values are the C2 `DbValue`; only the application's already-authorized plan calls this API.

use std::collections::BTreeMap;
use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::mutation::DbValue;

pub const PROTOCOL_VERSION: u32 = 2;
pub const FRAME_LIMIT: usize = 48 * 1024 * 1024;
pub const PAYLOAD_LIMIT: usize = 32 * 1024 * 1024;
pub const JAVA_SUPPORT: &str = include_str!("JdbcMutationSupport.java");
#[path = "jdbc_mutation_contracts.rs"]
mod contracts;
pub use contracts::{decode_json, exact_fields, validate_result, validate_scalar};

/// Separate catalog/schema/name; namespace display strings are not object identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JdbcObjectId {
    pub catalog: String,
    pub schema: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JdbcCapabilities {
    pub protocol_version: u32,
    pub session_id: String,
    pub adapter_id: String,
    pub product: String,
    pub product_version: String,
    pub driver_name: String,
    pub driver_version: String,
    pub catalog: String,
    pub schema: String,
    pub read_only: bool,
    pub transactions: bool,
    pub metadata: bool,
    pub row_write: bool,
    pub ddl_write: bool,
    pub bound_types: Vec<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JdbcColumnMetadata {
    pub name: String,
    pub jdbc_type: i32,
    pub type_name: String,
    pub ordinal: u32,
    pub precision: u32,
    pub scale: i32,
    pub nullable: Option<bool>,
    pub auto_increment: Option<bool>,
    pub generated: Option<bool>,
    pub read_only: bool,
    pub default_value: Option<String>,
    pub generation_expression: Option<String>,
    pub writable_type: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JdbcMetadataSnapshot {
    pub object: JdbcObjectId,
    pub session_id: String,
    pub backend_object_id: String,
    pub revision: String,
    pub table_type: String,
    pub definition: Option<String>,
    pub columns: Vec<JdbcColumnMetadata>,
    pub primary_key: Vec<String>,
    pub unique_keys: Vec<Vec<String>>,
    pub indexes: Value,
    pub foreign_keys: Value,
    pub constraints: Value,
    pub triggers: Value,
    pub completeness: BTreeMap<String, String>,
}

/// An adapter operation compiled from a server-owned C2 plan, NEVER frontend SQL.
/// `read_only` must be resolved by the caller from the saved effective connection;
/// neither an omitted field nor driver support can waive the application read-only gate.
#[derive(Debug, Clone, Serialize)]
pub struct JdbcRowPlan {
    pub operation_id: String,
    pub session_id: String,
    pub object: JdbcObjectId,
    pub base_revision: String,
    pub kind: String,
    pub keys: Vec<(String, DbValue)>,
    pub original_values: Vec<(String, DbValue)>,
    pub changes: Vec<(String, DbValue)>,
    pub read_only: bool,
    pub timeout_secs: u64,
}

/// Driver evidence for the single public C2 receipt. Unknown counts remain null.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JdbcMutationOutcome {
    pub operation_id: String,
    pub status: String,
    pub affected_rows: Option<String>,
    pub matched_rows: Option<String>,
    pub verified: bool,
    pub transaction: String,
    pub connection_poisoned: bool,
    pub mismatches: Vec<String>,
    pub readback: Vec<(String, DbValue)>,
    pub generated_keys: Vec<(String, DbValue)>,
}

pub fn source_with_support(main: &str) -> String {
    format!("{main}\n{JAVA_SUPPORT}\n")
}

pub fn request(action: &str, session: &str, payload: Value) -> Result<(String, String)> {
    let request_id = uuid::Uuid::new_v4().to_string();
    let bytes = serde_json::to_vec(&serde_json::json!({
        "protocol": PROTOCOL_VERSION, "request_id": request_id,
        "session_id": session, "action": action, "payload": payload,
    }))?;
    if bytes.len() > PAYLOAD_LIMIT { bail!("JDBC v2 request exceeds 32 MiB; no SQL sent"); }
    let line = format!("v2\t{}\n", STANDARD.encode(bytes));
    if line.len() > FRAME_LIMIT { bail!("JDBC v2 frame exceeds 48 MiB; no SQL sent"); }
    Ok((request_id, line))
}

/// Validate every envelope BEFORE inspecting results. A protocol/ID mismatch is
/// transport corruption: the caller must poison/kill the sidecar, never reuse it.
pub fn decode_envelope(line: &str, request_id: &str, session: &str) -> Result<Value> {
    if line.len() > FRAME_LIMIT { bail!("JDBC v2 response exceeds frame limit"); }
    let value = decode_json(line).context("JDBC v2 invalid response JSON")?;
    if value.get("protocol").and_then(Value::as_u64) != Some(PROTOCOL_VERSION as u64)
        || value.get("request_id").and_then(Value::as_str) != Some(request_id)
        || value.get("session_id").and_then(Value::as_str).map_or(true, |id|
            if session.is_empty() { uuid::Uuid::parse_str(id).is_err() } else { id != session }) {
        bail!("JDBC v2 protocol/request/session identity mismatch; reconnect required");
    }
    if value.get("result").is_some() == value.get("error").is_some() {
        bail!("JDBC v2 response must contain exactly one result or error");
    }
    if let Some(error) = value.get("error") {
        exact_fields(&value, &["protocol", "request_id", "session_id", "error"])?;
        exact_fields(error, &["code", "sql_state", "message", "effects", "connection_poisoned"])?;
        if ["code", "sql_state", "message", "effects"].into_iter().any(|key| !error.get(key).is_some_and(Value::is_string))
            || !error.get("connection_poisoned").is_some_and(Value::is_boolean)
            || !matches!(error.get("effects").and_then(Value::as_str), Some("none" | "unknown"))
            || error.get("effects").and_then(Value::as_str) == Some("unknown") && error.get("connection_poisoned").and_then(Value::as_bool) != Some(true) {
            bail!("JDBC v2 error evidence is malformed or contradictory");
        }
    } else { exact_fields(&value, &["protocol", "request_id", "session_id", "result"])?; }
    Ok(value)
}

pub fn decode_result<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T> {
    if let Some(error) = value.get("error") {
        return Err(anyhow!("JDBC {} [{}]: {} (effects={}; reconnect={})",
            error.get("code").and_then(Value::as_str).unwrap_or("DRIVER_ERROR"),
            error.get("sql_state").and_then(Value::as_str).unwrap_or(""),
            error.get("message").and_then(Value::as_str).unwrap_or("operation failed"),
            error.get("effects").and_then(Value::as_str).unwrap_or("unknown"),
            error.get("connection_poisoned").and_then(Value::as_bool).unwrap_or(true)));
    }
    serde_json::from_value(value.get("result").cloned().ok_or_else(|| anyhow!("JDBC v2 missing result"))?)
        .context("JDBC v2 result contract mismatch; reconnect required")
}

pub fn poisoned(value: &Value) -> bool {
    value.get("error").or_else(|| value.get("result"))
        .and_then(|part| part.get("connection_poisoned"))
        .and_then(Value::as_bool).unwrap_or(false)
}

/// A dropped/timeout transport future destroys the physical sidecar. Its next
/// stdout frame can never be misattributed to a later query or a retried write.
pub async fn exchange_line(
    child: &mut tokio::process::Child,
    input: &mut Option<tokio::process::ChildStdin>,
    output: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
    line: &str,
    timeout: std::time::Duration,
) -> Result<String> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    struct Admission<'a> {
        child: &'a mut tokio::process::Child,
        input: &'a mut Option<tokio::process::ChildStdin>,
        complete: bool,
    }
    impl Drop for Admission<'_> {
        fn drop(&mut self) {
            if !self.complete {
                self.input.take();
                let _ = self.child.start_kill();
            }
        }
    }
    if line.len() > FRAME_LIMIT { bail!("JDBC sidecar request exceeds 48 MiB; no SQL sent"); }
    let mut admission = Admission { child, input, complete: false };
    let response = tokio::time::timeout(timeout, async {
        let input = admission.input.as_mut().ok_or_else(|| anyhow!("JDBC physical sidecar is closed; reconnect required"))?;
        input.write_all(line.as_bytes()).await?;
        input.flush().await?;
        let mut bytes = Vec::new();
        loop {
            let buffer = output.fill_buf().await?;
            if buffer.is_empty() { bail!("JDBC sidecar exited or returned a partial frame; outcome unknown, do not replay"); }
            let end = buffer.iter().position(|b| *b == b'\n').map(|n| n + 1);
            let count = end.unwrap_or(buffer.len());
            if bytes.len().saturating_add(count) > FRAME_LIMIT { bail!("JDBC sidecar response exceeds 48 MiB; outcome unknown, do not replay"); }
            bytes.extend_from_slice(&buffer[..count]);
            output.consume(count);
            if end.is_some() { break; }
        }
        String::from_utf8(bytes).context("JDBC sidecar response is not UTF-8; outcome unknown, do not replay")
    }).await.context("JDBC request timed out; physical sidecar discarded; outcome unknown, do not replay")??;
    admission.complete = true;
    Ok(response)
}

#[cfg(test)]
#[path = "jdbc_mutation_real_tests.rs"]
mod real_tests;
#[cfg(test)]
#[path = "jdbc_mutation_transport_tests.rs"]
mod transport_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn v2_identity_mismatches_are_rejected() {
        let request_id = uuid::Uuid::new_v4().to_string();
        let session_id = uuid::Uuid::new_v4().to_string();
        let valid = serde_json::json!({"protocol":2,"request_id":request_id,"session_id":session_id,"result":{}});
        assert!(decode_envelope(&valid.to_string(), &request_id, &session_id).is_ok());
        for (key, bad) in [("protocol", serde_json::json!(1)), ("request_id", serde_json::json!("wrong")), ("session_id", serde_json::json!("wrong"))] {
            let mut value = valid.clone(); value[key] = bad;
            assert!(decode_envelope(&value.to_string(), &request_id, &session_id).is_err());
        }
        assert!(decode_envelope(r#"{"columns":[],"rows":[]}"#, &request_id, &session_id).is_err());
    }
    #[test]
    fn counts_and_exact_values_are_not_coerced() {
        let result = serde_json::json!({"operation_id":"one","status":"unknown","affected_rows":null,"matched_rows":"1","verified":false,
            "transaction":"unknown","connection_poisoned":true,"mismatches":["commit ack missing"],
            "readback":[["id",{"kind":"integer","value":"9223372036854775807"}],["d",{"kind":"decimal","value":"12345678901234567890.123456789012"}]],"generated_keys":[]});
        let outcome: JdbcMutationOutcome = decode_result(serde_json::json!({"result": result})).unwrap();
        assert!(outcome.affected_rows.is_none()); assert!(!outcome.verified);
        assert_eq!(outcome.readback[0].1, DbValue::Integer("9223372036854775807".into()));
        assert_eq!(outcome.readback[1].1, DbValue::Decimal("12345678901234567890.123456789012".into()));
    }
}
