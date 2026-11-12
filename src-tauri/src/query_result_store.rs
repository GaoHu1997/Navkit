//! Bounded, ephemeral source evidence. This is not an SQL authorization registry.
//! Row identities/old typed values originate only in the already-authorized C1 capture.
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use anyhow::{anyhow, bail, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use crate::mutation::DbValue;
use crate::query_lineage::{CapturedSelect, OriginColumn, ProvenSelect};

pub const RESULT_TTL: Duration = Duration::from_secs(120);
pub const RESULT_LIMIT: usize = 64;
pub const RESULT_CACHE_BYTES: usize = 64 * 1024 * 1024;
pub const SINGLE_RESULT_BYTES: usize = 20 * 1024 * 1024;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ResultBinding {
    pub principal: String, pub owner_id: String, pub connection_id: String, pub endpoint_fingerprint: String,
    pub source_execution_id: String, pub result_index: usize, pub generation: String,
}
impl ResultBinding {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.principal.is_empty() || self.owner_id.is_empty() || self.connection_id.is_empty() || self.principal.len()>512 || self.owner_id.len()>512 || self.connection_id.len()>512
            || uuid::Uuid::parse_str(&self.source_execution_id).is_err() || uuid::Uuid::parse_str(&self.generation).is_err()
            || self.endpoint_fingerprint.len()!=64 || !self.endpoint_fingerprint.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) { bail!("QUERY_CONTEXT_INVALID：来源绑定不完整"); } Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ActionCapability { pub supported: bool, pub reason: String }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ResultActions { pub update: ActionCapability, pub delete: ActionCapability, pub insert: ActionCapability }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct QueryEditability {
    pub source_execution_id: String, pub result_index: usize, pub generation: String,
    pub result_token: Option<String>, pub mode: String, pub reason: String,
    pub schema: Option<String>, pub table: Option<String>, pub metadata_revision: Option<String>,
    pub columns: Vec<OriginColumn>, pub key_column_ids: Vec<String>, pub actions: ResultActions,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TokenizedRow { pub token: String, pub row_token: String, pub values: Vec<DbValue> }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RegisteredResult { pub editability: QueryEditability, pub rows: Vec<TokenizedRow>, pub loaded_rows: usize, pub total_rows: u64 }
#[derive(Debug, Clone, Serialize)]
struct Evidence { binding: ResultBinding, proof: ProvenSelect, rows: HashMap<String,Vec<DbValue>>, row_order: Vec<String>, total_rows: u64 }
struct Entry { evidence: Evidence, created: Instant, bytes: usize }
#[derive(Default)]
pub(crate) struct ResultStore { entries: Mutex<HashMap<String,Entry>> }
#[derive(Debug, Clone)]
pub(crate) struct ResolvedResult {
    pub binding: ResultBinding, pub proof: ProvenSelect, pub values: Option<Vec<DbValue>>,
}
fn cap(supported: bool, reason: &str) -> ActionCapability { ActionCapability { supported, reason: if supported { String::new() } else { reason.into() } } }
pub(crate) fn readonly(binding: &ResultBinding, reason: &str) -> QueryEditability {
    let reason = if reason.is_empty() { "查询来源证据不可用，结果保持只读" } else { reason };
    QueryEditability { source_execution_id: binding.source_execution_id.clone(), result_index: binding.result_index, generation: binding.generation.clone(), result_token: None, mode: "read_only".into(), reason: reason.into(), schema: None, table: None, metadata_revision: None, columns: Vec::new(), key_column_ids: Vec::new(), actions: ResultActions { update: cap(false,reason), delete: cap(false,reason), insert: cap(false,reason) } }
}
fn make_editability(binding:&ResultBinding,proof:&ProvenSelect,token:&str)->QueryEditability {
    let update=proof.columns.iter().any(|c| c.writable);
    QueryEditability { source_execution_id:binding.source_execution_id.clone(),result_index:binding.result_index,generation:binding.generation.clone(),result_token:Some(token.into()),mode:"single_table".into(),reason:String::new(),schema:Some(proof.source.schema.clone()),table:Some(proof.source.table.clone()),metadata_revision:Some(proof.source.metadata_revision.clone()),columns:proof.columns.clone(),key_column_ids:proof.key_column_ids.clone(),actions:ResultActions { update:cap(update,"结果没有已验证的可写投影列"),delete:cap(proof.delete_supported,"删除需要完整原行投影；请明确重新查询基表全部列"),insert:cap(proof.insert_supported,"新增所需的必填基表列未全部投影") } }
}
impl ResultStore {
    /// Only server-created captures are accepted. No caller-supplied table/key/old values.
    /// Returned data is detached; mutating it never changes stored evidence.
    pub(crate) fn register(&self, binding: ResultBinding, capture: &CapturedSelect) -> Result<RegisteredResult> {
        binding.validate()?;
        {
            let mut entries=self.entries.lock(); Self::prune(&mut entries);
            // Even a new read-only result replaces the old editable generation of this slot.
            entries.retain(|_,entry| !(entry.evidence.binding.principal==binding.principal && entry.evidence.binding.owner_id==binding.owner_id && entry.evidence.binding.result_index==binding.result_index));
        }
        let Some(proof) = &capture.proof else { return Ok(RegisteredResult { editability: readonly(&binding,&capture.read_only_reason), rows: Vec::new(), loaded_rows: 0, total_rows: capture.total }); };
        if capture.typed_rows.len()>crate::query_lineage::TOKEN_PREFIX_ROWS || capture.typed_rows.len() as u64>capture.total || proof.source.schema!="main" || proof.columns.len()!=capture.names.len() || proof.key_column_ids.is_empty()
            || proof.columns.iter().map(|c| &c.result_column_id).collect::<HashSet<_>>().len()!=proof.columns.len()
            || proof.key_column_ids.iter().collect::<HashSet<_>>().len()!=proof.key_column_ids.len() { bail!("QUERY_PROOF_INVALID：来源快照不完整"); }
        let mut keys = HashSet::new(); let mut stored = HashMap::new(); let mut rows=Vec::new();
        let key_indexes = proof.key_column_ids.iter().map(|id| proof.columns.iter().position(|c| c.result_column_id==*id).ok_or_else(|| anyhow!("来源键缺失"))).collect::<Result<Vec<_>>>()?;
        for values in &capture.typed_rows {
            if values.len()!=proof.columns.len() { bail!("来源行宽度不一致"); }
            let key: Vec<_> = key_indexes.iter().map(|i| &values[*i]).collect();
            if key.iter().any(|v| matches!(v,DbValue::Null)) || !keys.insert(serde_json::to_vec(&key)?) { bail!("来源定位键含 NULL 或重复，未签发行令牌"); }
            let token=uuid::Uuid::new_v4().to_string(); stored.insert(token.clone(),values.clone()); rows.push(TokenizedRow { token: token.clone(), row_token: token, values: values.clone() });
        }
        let row_order=rows.iter().map(|row| row.row_token.clone()).collect();
        let token=uuid::Uuid::new_v4().to_string(); let evidence=Evidence { binding: binding.clone(), proof: proof.clone(), rows: stored, row_order, total_rows:capture.total };
        let bytes=serde_json::to_vec(&evidence)?.len();
        if bytes>SINGLE_RESULT_BYTES { return Ok(RegisteredResult { editability: readonly(&binding,"来源证据超过单结果 20 MiB 限额，保持只读；请缩小查询范围"), rows: Vec::new(), loaded_rows: 0, total_rows: capture.total }); }
        let mut entries=self.entries.lock(); Self::prune(&mut entries);
        // Retest the slot immediately before publishing, under the same lock as eviction/insertion.
        entries.retain(|_,entry| !(entry.evidence.binding.principal==binding.principal && entry.evidence.binding.owner_id==binding.owner_id && entry.evidence.binding.result_index==binding.result_index));
        while entries.len()>=RESULT_LIMIT || entries.values().fold(bytes,|n,e| n.saturating_add(e.bytes))>RESULT_CACHE_BYTES {
            let Some(oldest)=entries.iter().min_by_key(|(_,e)| e.created).map(|(id,_)| id.clone()) else { break; }; entries.remove(&oldest);
        }
        entries.insert(token.clone(),Entry { evidence,created:Instant::now(),bytes });
        let editability=make_editability(&binding,proof,&token);
        Ok(RegisteredResult { editability, loaded_rows:rows.len(),rows,total_rows:capture.total })
    }
    fn prune(entries: &mut HashMap<String,Entry>) { entries.retain(|_,entry| entry.created.elapsed()<RESULT_TTL); }
    /// Caller supplies its *current server-side* binding, never an IPC binding object.
    pub(crate) fn resolve(&self,token:&str,row_token:Option<&str>,current:&ResultBinding)->Result<ResolvedResult> {
        current.validate()?; let mut entries=self.entries.lock(); Self::prune(&mut entries);
        let entry=entries.get(token).ok_or_else(|| anyhow!("QUERY_RESULT_EXPIRED：来源令牌已过期/释放，请重新执行"))?;
        if entry.evidence.binding!=*current { bail!("QUERY_CONTEXT_CHANGED：令牌不属于当前窗口/页面/端点/执行/结果代际"); }
        let values=match row_token { Some(row)=>Some(entry.evidence.rows.get(row).ok_or_else(|| anyhow!("QUERY_ROW_TOKEN_INVALID：行令牌不属于当前结果"))?.clone()),None=>None };
        Ok(ResolvedResult { binding:entry.evidence.binding.clone(),proof:entry.evidence.proof.clone(),values })
    }
    /// A short-lived IPC copy only. Entry keeps a small slot, never another permanent typed cache.
    /// Row ordering is the original capture order, not HashMap iteration or a client row number.
    pub(crate) fn snapshot(&self,result_token:&str,current:&ResultBinding)->Result<RegisteredResult> {
        current.validate()?; let mut entries=self.entries.lock(); Self::prune(&mut entries);
        let entry=entries.get(result_token).ok_or_else(||anyhow!("QUERY_RESULT_EXPIRED：来源令牌已过期/释放，请重新执行"))?;
        let evidence=&entry.evidence;
        if evidence.binding!=*current { bail!("QUERY_CONTEXT_CHANGED：令牌不属于当前窗口/页面/端点/执行/结果代际"); }
        let rows=evidence.row_order.iter().map(|token| {
            let values=evidence.rows.get(token).ok_or_else(||anyhow!("QUERY_PROOF_INVALID：缓存行序与原值证据不一致"))?;
            Ok(TokenizedRow { token:token.clone(),row_token:token.clone(),values:values.clone() })
        }).collect::<Result<Vec<_>>>()?;
        Ok(RegisteredResult { editability:make_editability(&evidence.binding,&evidence.proof,result_token),loaded_rows:rows.len(),rows,total_rows:evidence.total_rows })
    }
    pub(crate) fn clear_owner(&self,principal:&str,owner:&str) { self.entries.lock().retain(|_,e| e.evidence.binding.principal!=principal || e.evidence.binding.owner_id!=owner); }
    pub(crate) fn clear_connection(&self,id:&str) { self.entries.lock().retain(|_,e| e.evidence.binding.connection_id!=id); }
    pub(crate) fn invalidate(&self,token:&str,current:&ResultBinding)->Result<()> { let mut entries=self.entries.lock(); if let Some(entry)=entries.get(token) { if entry.evidence.binding!=*current { bail!("来源令牌不属于当前窗口/页面"); } } entries.remove(token); Ok(()) }
}

#[cfg(test)]
#[path="query_result_store_tests.rs"]
mod tests;
