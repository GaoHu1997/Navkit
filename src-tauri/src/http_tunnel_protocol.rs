//! PL-17 wire contract only. This module never authorizes or executes database SQL.
//! Frames are useful only after the C1 caller verifies the expected service/target
//! and binds the opened session to its actual runtime resource fingerprint.
use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const PROTOCOL: &str = "navkit-sql-tunnel/1";
pub const PROTOCOL_HEADER: &str = "X-Navkit-Protocol";
pub const OWNER_HEADER: &str = "X-Navkit-Owner";
pub const CAPABILITIES_PATH: &str = "/v1/capabilities";
pub const SESSIONS_PATH: &str = "/v1/sessions";
pub const JSON_TYPE: &str = "application/json";
pub const STREAM_TYPE: &str = "application/x-ndjson";
pub const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_VALUE_BYTES: usize = 1024 * 1024;
pub const MAX_REQUEST_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_RESULT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_ROWS: u64 = 10_000;
pub const MAX_COLUMNS: usize = 1024;

fn uuid(value: &str) -> Result<()> { if Uuid::parse_str(value)?.to_string() != value { bail!("Noncanonical tunnel UUID"); } Ok(()) }
fn nonempty(value: &str, max: usize) -> Result<()> { if value.is_empty() || value.len() > max || value.chars().any(|c| c == '\0' || c.is_control()) { bail!("Invalid tunnel identity"); } Ok(()) }
fn revision(value: &str) -> Result<()> { if value.len()!=64 || !value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) { bail!("Invalid tunnel revision"); } Ok(()) }
pub fn decimal_count(value: &str) -> Result<u64> {
    if value.is_empty() || value.len()>20 || value.len()>1 && value.starts_with('0') || !value.bytes().all(|b| b.is_ascii_digit()) { bail!("Noncanonical tunnel count"); }
    Ok(value.parse().context("Tunnel count exceeds u64")?)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SessionBinding {
    pub server_id: String, pub server_generation: String,
    pub target_id: String, pub target_generation: String,
    pub session_id: String, pub session_generation: String,
    pub owner_id: String, pub principal: String, pub namespace: String,
    pub read_only: bool, pub capability_revision: String,
}
impl SessionBinding {
    pub fn validate(&self) -> Result<()> {
        for value in [&self.server_id,&self.server_generation,&self.target_id,&self.target_generation,&self.session_id,&self.session_generation,&self.owner_id] { uuid(value)?; }
        nonempty(&self.principal,200)?; nonempty(&self.namespace,200)?; revision(&self.capability_revision)
    }
}

/// A sequence is reserved atomically BEFORE sending SQL. The physical session's
/// watermark never decreases, even after the bounded status tombstone expires.
/// The execution UUID is a deterministic namespace of session generation+sequence,
/// so a removed global status entry cannot be replayed in another session either.
pub fn execution_id(binding: &SessionBinding, sequence: &str) -> Result<String> {
    binding.validate()?; if decimal_count(sequence)? == 0 { bail!("Execution sequence starts at one"); }
    let mut hash=Sha256::new(); hash.update(b"navkit-sql-tunnel/1\0"); hash.update(binding.session_id.as_bytes()); hash.update([0]); hash.update(binding.session_generation.as_bytes()); hash.update([0]); hash.update(sequence.as_bytes());
    let digest=hash.finalize(); let mut bytes=[0u8;16]; bytes.copy_from_slice(&digest[..16]); bytes[6]=(bytes[6]&0x0f)|0x80; bytes[8]=(bytes[8]&0x3f)|0x80;
    Ok(Uuid::from_bytes(bytes).to_string())
}
pub fn executions_path(session: &str) -> Result<String> { uuid(session)?; Ok(format!("{SESSIONS_PATH}/{session}/executions")) }
pub fn catalog_path(session: &str) -> Result<String> { uuid(session)?; Ok(format!("{SESSIONS_PATH}/{session}/catalog")) }
pub fn metadata_path(session: &str) -> Result<String> { uuid(session)?; Ok(format!("{SESSIONS_PATH}/{session}/metadata")) }
pub fn session_path(session: &str) -> Result<String> { uuid(session)?; Ok(format!("{SESSIONS_PATH}/{session}")) }
pub fn status_path(execution: &str) -> Result<String> { uuid(execution)?; Ok(format!("/v1/executions/{execution}")) }
pub fn cancel_path(execution: &str) -> Result<String> { Ok(format!("{}/cancel",status_path(execution)?)) }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag="kind",content="value",rename_all="snake_case",deny_unknown_fields)]
pub enum TunnelValue { Null, Text(String), Integer(String), Decimal(String), Float(f64), Bool(bool), Binary(String), Date(String), Time(String), Timestamp(String), Timestamptz(String), Uuid(String) }
impl TunnelValue {
    pub fn validate(&self) -> Result<()> {
        use chrono::{NaiveDate,NaiveTime,NaiveDateTime,DateTime};
        match self {
            Self::Null|Self::Bool(_)=>{},
            Self::Float(value)=>if !value.is_finite(){bail!("Nonfinite tunnel float");},
            Self::Text(value)=>{if value.len()>MAX_VALUE_BYTES{bail!("Tunnel text exceeds quota");}},
            Self::Integer(value)=>{if value.parse::<i64>().map(|v|v.to_string()).ok().as_deref()!=Some(value.as_str()){bail!("Tunnel integer is not an exact canonical signed BIGINT");}},
            Self::Decimal(value)=>{if value.len()>1000 || !decimal_literal(value){bail!("Tunnel decimal must be bounded exact decimal text");}},
            Self::Binary(value)=>{let bytes=STANDARD.decode(value)?;if bytes.len()>MAX_VALUE_BYTES || STANDARD.encode(bytes)!=*value {bail!("Noncanonical or excessive tunnel binary");}},
            Self::Date(value)=>{if value.len()!=10 || !value.as_bytes().get(0).map_or(false,u8::is_ascii_digit) {bail!("Invalid tunnel date");}NaiveDate::parse_from_str(value,"%Y-%m-%d")?;},
            Self::Time(value)=>{if value.len()>18{bail!("Excessive tunnel time precision");}NaiveTime::parse_from_str(value,"%H:%M:%S%.f").or_else(|_|NaiveTime::parse_from_str(value,"%H:%M"))?;},
            Self::Timestamp(value)=>{if value.len()>29 || value.len()<16 || !value.is_ascii(){bail!("Invalid tunnel timestamp");}NaiveDateTime::parse_from_str(value,"%Y-%m-%dT%H:%M:%S%.f").or_else(|_|NaiveDateTime::parse_from_str(value,"%Y-%m-%dT%H:%M"))?;},
            Self::Timestamptz(value)=>{if value.len()>35 || !value.is_ascii(){bail!("Invalid tunnel offset timestamp");}let date=DateTime::parse_from_rfc3339(value)?;if date.offset().local_minus_utc().abs()>18*3600 || date.offset().local_minus_utc()%60!=0 {bail!("Unsupported tunnel timezone offset");}},
            Self::Uuid(value)=>uuid(value)?,
        } Ok(())
    }
}
fn decimal_literal(value:&str)->bool {
    let raw=value.strip_prefix('-').unwrap_or(value);let mut parts=raw.split('.');let whole=parts.next().unwrap_or("");let fraction=parts.next();
    !whole.is_empty() && whole.bytes().all(|b|b.is_ascii_digit()) && !(whole.len()>1 && whole.starts_with('0'))
        && fraction.map_or(true,|part|!part.is_empty() && part.bytes().all(|b|b.is_ascii_digit())) && parts.next().is_none()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TunnelCapabilities { pub query: bool, pub namespace: bool, pub metadata: bool, pub sql_write: bool, pub cancel: bool, pub status: bool, pub drain: bool, pub row_mutation: bool, pub ddl: bool, pub manual_transaction: bool }
impl TunnelCapabilities {
    pub fn validate(&self)->Result<()> { if !self.query || !self.namespace || !self.metadata || !self.cancel || !self.status || !self.drain {bail!("Tunnel lacks required control/metadata capabilities");} Ok(()) }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TunnelLimits { pub request_bytes:u64, pub frame_bytes:u64, pub result_bytes:u64, pub rows:u64, pub sessions:u32, pub executions:u32, pub status_ttl_ms:u64, pub session_idle_ms:u64, pub max_timeout_ms:u32 }
impl TunnelLimits {
    fn validate(&self)->Result<()> { if self.request_bytes==0 || self.request_bytes>MAX_REQUEST_BYTES as u64 || self.frame_bytes==0 || self.frame_bytes>MAX_FRAME_BYTES as u64 || self.result_bytes==0 || self.result_bytes>MAX_RESULT_BYTES as u64 || self.rows==0 || self.rows>MAX_ROWS || self.sessions==0 || self.sessions>1024 || self.executions==0 || self.executions>4096 || self.status_ttl_ms==0 || self.session_idle_ms==0 || self.max_timeout_ms==0 || self.max_timeout_ms>300_000 {bail!("Unbounded or unsupported tunnel quotas");} Ok(()) }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetDescriptor { pub alias:String,pub target_id:String,pub target_generation:String,pub product:String,pub product_version:String,pub namespaces:Vec<String> }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityResponse { pub protocol:String,pub server_id:String,pub server_generation:String,pub capability_revision:String,pub principal:String,pub implementation:String,pub targets:Vec<TargetDescriptor>,pub capabilities:TunnelCapabilities,pub limits:TunnelLimits }
impl CapabilityResponse {
    pub fn validate(&self,expected_server:&str)->Result<()> {
        if self.protocol!=PROTOCOL || self.server_id!=expected_server {bail!("Tunnel service/protocol identity differs from configured target");} uuid(&self.server_id)?;uuid(&self.server_generation)?;revision(&self.capability_revision)?;nonempty(&self.principal,200)?;nonempty(&self.implementation,200)?;self.capabilities.validate()?;self.limits.validate()?;
        if self.targets.is_empty() || self.targets.len()>64 {bail!("Tunnel has no bounded authorized target aliases");}let mut aliases=std::collections::BTreeSet::new();let mut ids=std::collections::BTreeSet::new();
        for target in &self.targets {nonempty(&target.alias,200)?;uuid(&target.target_id)?;uuid(&target.target_generation)?;nonempty(&target.product,200)?;nonempty(&target.product_version,200)?;if !aliases.insert(&target.alias) || !ids.insert(&target.target_id) || target.namespaces.is_empty() || target.namespaces.len()>64 {bail!("Ambiguous tunnel target identity");}let mut schemas=std::collections::BTreeSet::new();for ns in &target.namespaces {nonempty(ns,200)?;if !schemas.insert(ns){bail!("Duplicate tunnel namespace");}}}Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenSessionRequest { pub protocol:String,pub request_id:String,pub owner_id:String,pub target_alias:String,pub expected_server_id:String,pub expected_target_id:String,pub namespace:String,pub read_only:bool }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenSessionResponse { pub protocol:String,pub request_id:String,pub binding:SessionBinding,pub target_alias:String,pub catalog:String,pub product:String,pub product_version:String,pub watermark:String,pub capabilities:TunnelCapabilities,pub limits:TunnelLimits }
impl OpenSessionResponse {
    pub fn validate(&self,request:&OpenSessionRequest,capabilities:&CapabilityResponse)->Result<()> {
        self.binding.validate()?;uuid(&self.request_id)?;if self.protocol!=PROTOCOL || self.request_id!=request.request_id || self.binding.server_id!=request.expected_server_id || self.binding.owner_id!=request.owner_id || self.binding.target_id!=request.expected_target_id || self.target_alias!=request.target_alias || self.binding.namespace!=request.namespace || request.read_only && !self.binding.read_only || self.binding.principal!=capabilities.principal || self.binding.capability_revision!=capabilities.capability_revision || self.binding.server_generation!=capabilities.server_generation || decimal_count(&self.watermark)?!=0 {bail!("Opened tunnel session has changed identity or policy");}
        let target=capabilities.targets.iter().find(|target|target.alias==request.target_alias && target.target_id==request.expected_target_id).ok_or_else(||anyhow!("Requested tunnel alias was not negotiated"))?;
        if self.binding.target_generation!=target.target_generation || self.product!=target.product || self.product_version!=target.product_version || !target.namespaces.contains(&request.namespace) || self.capabilities!=capabilities.capabilities || self.limits!=capabilities.limits {bail!("Opened tunnel target/capabilities changed during negotiation");}self.capabilities.validate()?;self.limits.validate()?;nonempty(&self.catalog,200)?;Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecuteRequest { pub protocol:String,pub request_id:String,pub binding:SessionBinding,pub execution_id:String,pub sequence:String,pub sql:String,pub binds:Vec<TunnelValue>,pub timeout_ms:u32 }
impl ExecuteRequest {
    pub fn validate(&self)->Result<()> {self.binding.validate()?;uuid(&self.request_id)?;if self.protocol!=PROTOCOL || self.execution_id!=execution_id(&self.binding,&self.sequence)? || self.sql.is_empty() || self.sql.len()>1024*1024 || self.sql.contains('\0') || self.binds.len()>1024 || self.timeout_ms==0 || self.timeout_ms>300_000 {bail!("Invalid tunnel execution request");}for value in &self.binds {value.validate()?;}if serde_json::to_vec(self)?.len()>MAX_REQUEST_BYTES {bail!("Tunnel request exceeds quota");}Ok(())}
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundRequest { pub protocol:String,pub request_id:String,pub binding:SessionBinding }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogTable { pub schema:String, pub name:String, pub kind:String }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogResponse { pub protocol:String, pub request_id:String, pub binding:SessionBinding, pub completeness:String, pub tables:Vec<CatalogTable> }
impl CatalogResponse {
    pub fn validate(&self, request:&BoundRequest)->Result<()> {
        request.binding.validate()?;uuid(&request.request_id)?;
        if request.protocol!=PROTOCOL || self.protocol!=PROTOCOL || self.request_id!=request.request_id || self.binding!=request.binding || self.completeness!="known" || self.tables.len()>512 {bail!("Tunnel catalog identity/completeness/quota mismatch");}
        let mut names=std::collections::BTreeSet::new();
        for table in &self.tables {nonempty(&table.schema,200)?;nonempty(&table.name,200)?;if table.schema!=self.binding.namespace || !["table","view"].contains(&table.kind.as_str()) || !names.insert((&table.schema,&table.name)) {bail!("Tunnel catalog escaped the authorized namespace or is ambiguous");}}Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectId { pub catalog:String,pub schema:String,pub name:String }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataRequest { pub protocol:String,pub request_id:String,pub binding:SessionBinding,pub object:ObjectId }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataResponse { pub protocol:String,pub request_id:String,pub binding:SessionBinding,pub snapshot:crate::http_db::relational::JdbcMetadataSnapshot }
impl MetadataResponse {
    pub fn validate(&self,request:&MetadataRequest)->Result<()> {if self.protocol!=PROTOCOL || self.request_id!=request.request_id || self.binding!=request.binding || self.snapshot.session_id!=request.binding.session_id || self.snapshot.object.catalog!=request.object.catalog || self.snapshot.object.schema!=request.object.schema || self.snapshot.object.name!=request.object.name || self.snapshot.object.schema!=self.binding.namespace {bail!("Tunnel metadata belongs to another session/target");}
        let value=serde_json::json!({"session_id":self.binding.session_id,"result":self.snapshot});let payload=serde_json::json!({"object":self.snapshot.object});crate::http_db::relational::validate_result("metadata",&payload,&value)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultColumn { pub name:String,pub data_type:String,pub jdbc_type:i32,pub precision:u32,pub scale:i32,pub nullable:Option<bool> }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameIdentity { pub protocol:String,pub request_id:String,pub binding:SessionBinding,pub execution_id:String,pub sequence:String }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag="kind",rename_all="snake_case")]
pub enum StreamFrame {
    Header { #[serde(flatten)] identity:FrameIdentity, columns:Vec<ResultColumn> },
    Row { #[serde(flatten)] identity:FrameIdentity, ordinal:String, values:Vec<TunnelValue> },
    Progress { #[serde(flatten)] identity:FrameIdentity, rows_read:String },
    Final { #[serde(flatten)] identity:FrameIdentity, state:String, transaction:String, affected_rows:Option<String>, rows_read:String, connection_poisoned:bool, code:Option<String>, message:Option<String> },
}
/// State machine checks the COMPLETE stream. EOF without a final frame is always
/// unknown to the caller after submission, even if a prefix of rows looked valid.
pub struct StreamValidator { expected:FrameIdentity, columns:Option<usize>, rows:u64, bytes:usize, terminal:bool }
impl StreamValidator {
    pub fn new(request:&ExecuteRequest)->Result<Self> {request.validate()?;Ok(Self{expected:FrameIdentity{protocol:PROTOCOL.into(),request_id:request.request_id.clone(),binding:request.binding.clone(),execution_id:request.execution_id.clone(),sequence:request.sequence.clone()},columns:None,rows:0,bytes:0,terminal:false})}
    pub fn push_json(&mut self,line:&str)->Result<StreamFrame> {
        if self.terminal || line.len()>MAX_FRAME_BYTES {bail!("Unexpected or excessive tunnel frame");}self.bytes=self.bytes.checked_add(line.len()).ok_or_else(||anyhow!("Tunnel stream quota overflow"))?;if self.bytes>MAX_RESULT_BYTES{bail!("Tunnel result exceeds quota");}
        let value=decode_json(line)?;
        let kind=value.get("kind").and_then(Value::as_str).ok_or_else(||anyhow!("Tunnel frame kind missing"))?;
        let mut fields=vec!["kind","protocol","request_id","binding","execution_id","sequence"];
        match kind {"header"=>fields.push("columns"),"row"=>fields.extend(["ordinal","values"]),"progress"=>fields.push("rows_read"),"final"=>fields.extend(["state","transaction","affected_rows","rows_read","connection_poisoned","code","message"]),_=>bail!("Unknown tunnel frame")};
        crate::http_db::relational::exact_fields(&value,&fields)?;
        let frame:StreamFrame=serde_json::from_value(value).context("Malformed tunnel frame")?;
        let identity=match &frame {StreamFrame::Header{identity,..}|StreamFrame::Row{identity,..}|StreamFrame::Progress{identity,..}|StreamFrame::Final{identity,..}=>identity};
        if identity.protocol!=self.expected.protocol || identity.request_id!=self.expected.request_id || identity.binding!=self.expected.binding || identity.execution_id!=self.expected.execution_id || identity.sequence!=self.expected.sequence {bail!("Tunnel frame identity changed within a stream");}
        match &frame {
            StreamFrame::Header{columns,..}=>{if self.columns.is_some() || columns.len()>MAX_COLUMNS{bail!("Duplicate or excessive tunnel header");}for column in columns {nonempty(&column.name,4096)?;nonempty(&column.data_type,200)?;}self.columns=Some(columns.len());},
            StreamFrame::Row{ordinal,values,..}=>{if self.columns!=Some(values.len()) || decimal_count(ordinal)?!=self.rows || self.rows>=MAX_ROWS {bail!("Tunnel row order/width/quota mismatch");}for value in values{value.validate()?;}self.rows+=1;},
            StreamFrame::Progress{rows_read,..}=>if self.columns.is_none() || decimal_count(rows_read)?!=self.rows {bail!("Tunnel progress is not the received row count");},
            StreamFrame::Final{state,transaction,affected_rows,rows_read,connection_poisoned,code,message,..}=>{if decimal_count(rows_read)?!=self.rows {bail!("Tunnel final row count differs from stream");}validate_terminal(state,transaction,affected_rows.as_deref(),*connection_poisoned,code.as_deref(),message.as_deref())?;if state=="succeeded" && self.columns.is_none(){bail!("Successful tunnel result has no header");}self.terminal=true;},
        }Ok(frame)
    }
    pub fn finish(&self)->Result<()> {if !self.terminal {bail!("MUTATION_EFFECTS_UNKNOWN: tunnel stream ended without a validated final frame; do not replay");}Ok(())}
}
fn validate_terminal(state:&str,transaction:&str,affected:Option<&str>,poisoned:bool,code:Option<&str>,message:Option<&str>)->Result<()> {
    match state {
        "succeeded"=>{decimal_count(affected.ok_or_else(||anyhow!("Missing affected count"))?)?;if !["committed","rolled_back"].contains(&transaction) || poisoned || code.is_some() || message.is_some(){bail!("Contradictory successful tunnel outcome");}},
        "failed"|"cancelled"=>{if affected!=Some("0") || !["rolled_back","none"].contains(&transaction) || code.map_or(true,str::is_empty) || message.map_or(true,str::is_empty){bail!("Unverified failed/cancelled tunnel outcome");}},
        "unknown"=>{if affected.is_some() || transaction!="unknown" || !poisoned || code.map_or(true,str::is_empty) || message.map_or(true,str::is_empty){bail!("Unknown tunnel outcome cannot have a fabricated count or reusable session");}},
        _=>bail!("Unknown tunnel terminal state"),
    }Ok(())
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusResponse { pub protocol:String,pub request_id:String,pub binding:SessionBinding,pub execution_id:String,pub sequence:String,pub state:String,pub transaction:String,pub affected_rows:Option<String>,pub rows_read:String,pub connection_poisoned:bool,pub code:Option<String>,pub message:Option<String> }
impl StatusResponse {
    pub fn validate(&self,request:&ExecuteRequest)->Result<()> {if self.protocol!=PROTOCOL || self.binding!=request.binding || self.execution_id!=request.execution_id || self.sequence!=request.sequence {bail!("Tunnel status belongs to a different operation");}uuid(&self.request_id)?;decimal_count(&self.rows_read)?;if self.state=="running" {if self.affected_rows.is_some() || self.transaction!="active" || self.connection_poisoned || self.code.is_some() || self.message.is_some(){bail!("Running tunnel status fabricates completion");}Ok(())}else{validate_terminal(&self.state,&self.transaction,self.affected_rows.as_deref(),self.connection_poisoned,self.code.as_deref(),self.message.as_deref())}}
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelResponse { pub protocol:String,pub request_id:String,pub binding:SessionBinding,pub execution_id:String,pub sequence:String,pub requested:bool,pub state:String }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrainResponse { pub protocol:String,pub request_id:String,pub binding:SessionBinding,pub state:String,pub pending_execution_ids:Vec<String>,pub connection_poisoned:bool }
impl DrainResponse {
    pub fn validate(&self,request:&BoundRequest)->Result<()> {if self.protocol!=PROTOCOL || self.request_id!=request.request_id || self.binding!=request.binding {bail!("Tunnel drain identity mismatch");}for eid in &self.pending_execution_ids {uuid(eid)?;}match self.state.as_str(){"closed"=>if !self.pending_execution_ids.is_empty(){bail!("Tunnel close has undrained executions");},"pending"=>if self.pending_execution_ids.is_empty(){bail!("Tunnel pending close lacks owned execution");},"unknown"=>if !self.connection_poisoned {bail!("Unknown tunnel close must quarantine the resource");},_=>bail!("Unknown tunnel drain state")};Ok(())}
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorResponse {pub protocol:String,pub request_id:Option<String>,pub code:String,pub message:String,pub effects:String,pub retryable:bool}

pub fn decode_json(text:&str)->Result<Value> {if text.len()>MAX_FRAME_BYTES {bail!("Tunnel JSON exceeds frame limit");}crate::http_db::relational::decode_json(text)}
pub fn decode_http_json(status:u16,content_type:&str,body:&[u8])->Result<Value> {
    if !(200..300).contains(&status) {bail!("Tunnel HTTP {status}: redirects/auth/errors are never followed or treated as SQL success");}if content_type.split(';').next().unwrap_or("").trim()!=JSON_TYPE || body.len()>MAX_FRAME_BYTES {bail!("Tunnel returned HTML or an unexpected/oversized media type");}let value=decode_json(std::str::from_utf8(body)?)?;if value.get("protocol").and_then(Value::as_str)!=Some(PROTOCOL){bail!("Tunnel protocol mismatch");}Ok(value)
}
pub fn validate_stream_content_type(status:u16,content_type:&str)->Result<()> {if status!=200 || content_type.split(';').next().unwrap_or("").trim()!=STREAM_TYPE {bail!("Tunnel execution did not return a v1 NDJSON stream");}Ok(())}
pub fn validate_https_endpoint(endpoint:&str)->Result<url::Url> {let parsed=url::Url::parse(endpoint)?;if parsed.scheme()!="https" || parsed.host_str().is_none() || !parsed.username().is_empty() || parsed.password().is_some() || parsed.query().is_some() || parsed.fragment().is_some() || parsed.path()!="/" {bail!("Tunnel endpoint must be an HTTPS origin without credentials/query/path/fragment");}Ok(parsed)}
/// Callers may add their resolved local CA/client identity before building, but
/// must retain HTTPS-only and redirect=none; bearer credentials never cross 3xx.
pub fn client_builder()->reqwest::ClientBuilder {reqwest::Client::builder().https_only(true).redirect(reqwest::redirect::Policy::none())}

#[cfg(test)]
mod tests {
    use super::*;
    fn binding()->SessionBinding {SessionBinding{server_id:Uuid::new_v4().to_string(),server_generation:Uuid::new_v4().to_string(),target_id:Uuid::new_v4().to_string(),target_generation:Uuid::new_v4().to_string(),session_id:Uuid::new_v4().to_string(),session_generation:Uuid::new_v4().to_string(),owner_id:Uuid::new_v4().to_string(),principal:"alice".into(),namespace:"PUBLIC".into(),read_only:false,capability_revision:"a".repeat(64)}}
    fn request()->ExecuteRequest {let binding=binding();ExecuteRequest{protocol:PROTOCOL.into(),request_id:Uuid::new_v4().to_string(),execution_id:execution_id(&binding,"1").unwrap(),binding,sequence:"1".into(),sql:"SELECT 1".into(),binds:vec![],timeout_ms:10_000}}
    fn frame(req:&ExecuteRequest,kind:&str)->Value {serde_json::json!({"kind":kind,"protocol":PROTOCOL,"request_id":req.request_id,"binding":req.binding,"execution_id":req.execution_id,"sequence":req.sequence})}
    #[test] fn execution_ids_are_session_sequence_namespaces_and_canonical(){let r=request();r.validate().unwrap();let mut next=r.binding.clone();next.session_generation=Uuid::new_v4().to_string();assert_ne!(r.execution_id,execution_id(&next,"1").unwrap());assert_ne!(r.execution_id,execution_id(&r.binding,"2").unwrap());for invalid in ["0","01","-1","18446744073709551616"]{assert!(execution_id(&r.binding,invalid).is_err());}}
    #[test] fn https_origin_and_http_media_auth_redirect_checks(){for bad in ["http://example.com/","https://alice:secret@example.com/","https://example.com/v1","https://example.com/?token=x","https://example.com/#x"]{assert!(validate_https_endpoint(bad).is_err());}validate_https_endpoint("https://localhost:4443/").unwrap();assert!(decode_http_json(302,JSON_TYPE,b"{}").is_err());assert!(decode_http_json(200,"text/html",b"<html>").is_err());assert!(decode_http_json(200,JSON_TYPE,b"{\"protocol\":\"wrong\"}").is_err());}
    #[test] fn exact_values_never_round_bigint_decimal_or_binary(){for good in [TunnelValue::Integer(i64::MAX.to_string()),TunnelValue::Decimal("12345678901234567890.123456789012".into()),TunnelValue::Binary("AP8BgA==".into()),TunnelValue::Timestamptz("2026-11-10T12:34:56.123456789+05:30".into())]{good.validate().unwrap();}for bad in [TunnelValue::Integer("9223372036854775808".into()),TunnelValue::Decimal("1e30".into()),TunnelValue::Binary("AA".into()),TunnelValue::Float(f64::INFINITY)]{assert!(bad.validate().is_err());}assert!(serde_json::from_value::<TunnelValue>(serde_json::json!({"kind":"integer","value":9007199254740993u64})).is_err());}
    #[test] fn duplicates_and_unknown_fields_fail_before_identity_decode(){assert!(decode_json("{\"binding\":{\"owner_id\":\"a\",\"owner_id\":\"b\"}}").is_err());let mut r=serde_json::to_value(request()).unwrap();r["host"]=Value::String("localhost".into());assert!(serde_json::from_value::<ExecuteRequest>(r).is_err());}
    #[test] fn stream_requires_full_identity_order_exact_count_and_final(){let r=request();let mut s=StreamValidator::new(&r).unwrap();let mut header=frame(&r,"header");header["columns"]=serde_json::json!([{"name":"N","data_type":"BIGINT","jdbc_type":-5,"precision":64,"scale":0,"nullable":false}]);s.push_json(&header.to_string()).unwrap();let mut row=frame(&r,"row");row["ordinal"]=Value::String("0".into());row["values"]=serde_json::json!([{"kind":"integer","value":"9223372036854775807"}]);s.push_json(&row.to_string()).unwrap();assert!(s.finish().is_err());let mut end=frame(&r,"final");for(k,v)in [("state",serde_json::json!("succeeded")),("transaction",serde_json::json!("rolled_back")),("affected_rows",serde_json::json!("0")),("rows_read",serde_json::json!("1")),("connection_poisoned",serde_json::json!(false)),("code",Value::Null),("message",Value::Null)]{end[k]=v;}s.push_json(&end.to_string()).unwrap();s.finish().unwrap();assert!(s.push_json(&end.to_string()).is_err());}
    #[test] fn stream_foreign_owner_or_synthetic_unknown_zero_is_rejected(){let r=request();let mut h=frame(&r,"header");h["columns"]=serde_json::json!([]);h["binding"]["owner_id"]=Value::String(Uuid::new_v4().to_string());assert!(StreamValidator::new(&r).unwrap().push_json(&h.to_string()).is_err());assert!(validate_terminal("unknown","unknown",Some("0"),true,Some("ACK_LOST"),Some("do not replay")).is_err());assert!(validate_terminal("unknown","unknown",None,false,Some("ACK_LOST"),Some("do not replay")).is_err());validate_terminal("unknown","unknown",None,true,Some("ACK_LOST"),Some("do not replay")).unwrap();}
    #[test] fn catalog_is_complete_bounded_and_exactly_session_scoped(){let request=BoundRequest{protocol:PROTOCOL.into(),request_id:Uuid::new_v4().to_string(),binding:binding()};let valid=CatalogResponse{protocol:PROTOCOL.into(),request_id:request.request_id.clone(),binding:request.binding.clone(),completeness:"known".into(),tables:vec![CatalogTable{schema:"PUBLIC".into(),name:"T_%中".into(),kind:"table".into()}]};valid.validate(&request).unwrap();let mut missing=valid.clone();missing.completeness="unknown".into();assert!(missing.validate(&request).is_err());let mut foreign=valid.clone();foreign.tables[0].schema="OTHER".into();assert!(foreign.validate(&request).is_err());let mut duplicate=valid.clone();duplicate.tables.push(duplicate.tables[0].clone());assert!(duplicate.validate(&request).is_err());let mut huge=valid.clone();huge.tables=(0..513).map(|i|CatalogTable{schema:"PUBLIC".into(),name:format!("T{i}"),kind:"table".into()}).collect();assert!(huge.validate(&request).is_err());let mut wrong=valid.clone();wrong.binding.owner_id=Uuid::new_v4().to_string();assert!(wrong.validate(&request).is_err());let mut field=serde_json::to_value(valid).unwrap();field["tables"][0]["host"]=serde_json::json!("evil");assert!(serde_json::from_value::<CatalogResponse>(field).is_err());}
    #[test] fn drain_pending_never_means_closed(){let request=BoundRequest{protocol:PROTOCOL.into(),request_id:Uuid::new_v4().to_string(),binding:binding()};let mut reply=DrainResponse{protocol:PROTOCOL.into(),request_id:request.request_id.clone(),binding:request.binding.clone(),state:"pending".into(),pending_execution_ids:vec![Uuid::new_v4().to_string()],connection_poisoned:false};reply.validate(&request).unwrap();reply.state="closed".into();assert!(reply.validate(&request).is_err());}
}
