//! Turn opaque PL21 source tokens into the existing C2 row plan; no SQL execution/retry here.
//! Parent must store `source` with PlanInternal and call query_lineage::validate_source on the
//! actual SQLite owned write transaction before DML. A preview pool read alone is insufficient.
use std::collections::HashSet;
use anyhow::{anyhow,bail,Result};
use serde::{Deserialize,Serialize};
use sqlx::Connection;
use crate::db::Db;
use crate::models::DbKind;
use crate::mutation::{self,DbValue,MutationDraft,PlanInternal};
use crate::query_lineage::SqliteSource;
use crate::query_result_store::{ResultBinding,ResultStore};

#[derive(Debug,Clone,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QueryRowChange {
    pub result_token:String, pub row_token:Option<String>, pub operation:String,
    /// Opaque result column IDs only. The IPC request never supplies table/key/old values/SQL.
    pub changes:Vec<(String,DbValue)>,
}
#[derive(Debug,Clone,Serialize)]
pub(crate) struct QueryMutationSource {
    pub binding:ResultBinding, pub source:SqliteSource, pub result_token:String,
    /// Filled from the authorized C1 entry; SQLite's base schema may be main while C1 used an empty namespace.
    pub execution_namespace:String,
}
#[derive(Debug,Clone,Serialize)]
pub(crate) struct PreparedQueryMutation { pub plan:PlanInternal, pub source:QueryMutationSource }

pub(crate) async fn prepare_row_plan(db:&Db,store:&ResultStore,current:&ResultBinding,request:&QueryRowChange,effective_read_only:bool)->Result<PreparedQueryMutation> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if effective_read_only { bail!("READ_ONLY_VIOLATION：当前保存连接为只读连接"); }
    if request.result_token.is_empty() || !["insert","update","delete"].contains(&request.operation.as_str()) || request.changes.len()>1024 || serde_json::to_vec(request)?.len()>16*1024*1024 { bail!("QUERY_MUTATION_INVALID：结果写入请求不完整或超限"); }
    let resolved=store.resolve(&request.result_token,request.row_token.as_deref(),current)?;
    if request.operation=="insert" && request.row_token.is_some() || request.operation!="insert" && resolved.values.is_none() { bail!("QUERY_ROW_TOKEN_INVALID：新增不接受旧行令牌，更新/删除必须提供旧行令牌"); }
    let proof=&resolved.proof;
    if request.operation=="delete" && !proof.delete_supported { bail!("QUERY_DELETE_UNSUPPORTED：查询没有投影完整可比较原行"); }
    if request.operation=="insert" && !proof.insert_supported { bail!("QUERY_INSERT_UNSUPPORTED：必填基表列没有全部投影"); }
    if request.operation=="update" && request.changes.is_empty() || request.operation=="delete" && !request.changes.is_empty() { bail!("QUERY_MUTATION_INVALID：更新必须有改动，删除不能携带列改动"); }
    let mut seen=HashSet::new(); let mut changes=Vec::new();
    for (id,value) in &request.changes {
        if !seen.insert(id) { bail!("QUERY_COLUMN_INVALID：列 ID 重复"); }
        let column=proof.columns.iter().find(|col| col.result_column_id==*id).ok_or_else(||anyhow!("QUERY_COLUMN_INVALID：列不属于当前投影"))?;
        let base=proof.metadata.columns.iter().find(|base| base.name==column.base_column).ok_or_else(||anyhow!("来源列元数据缺失"))?;
        if column.generated || request.operation=="update" && !column.writable || request.operation=="insert" && (!column.writable && !column.key || base.auto_increment) { bail!("QUERY_COLUMN_READ_ONLY：生成列/定位键/未认证列不可通过该操作修改"); }
        changes.push((column.base_column.clone(),value.clone()));
    }
    let mut keys=Vec::new(); let mut original=Vec::new();
    if let Some(values)=&resolved.values {
        if values.len()!=proof.columns.len() { bail!("来源原值宽度不一致"); }
        for (index,column) in proof.columns.iter().enumerate() {
            if column.key { if matches!(values[index],DbValue::Null) { bail!("QUERY_KEY_INVALID：来源键含 NULL"); } keys.push((column.base_column.clone(),values[index].clone())); }
            // Update CAS covers every changed column and every key. Delete requires all original fields.
            if request.operation=="delete" || column.key || changes.iter().any(|(name,_)| *name==column.base_column) { original.push((column.base_column.clone(),values[index].clone())); }
        }
    }
    let draft=MutationDraft { connection_id:current.connection_id.clone(),owner_id:current.owner_id.clone(),schema:proof.source.schema.clone(),table:proof.source.table.clone(),kind:request.operation.clone(),changes,keys,original_values:original };
    let Db::Sqlite(pool)=db else { bail!("UNSUPPORTED_QUERY_MUTATION：当前仅认证 SQLite 同连接来源；其他实际适配器保持只读"); };
    let mut conn=pool.acquire().await?;
    let mut tx=conn.begin().await?;
    crate::query_lineage::validate_source(&mut *tx,&proof.source).await?;
    let plan=mutation::build_plan(&draft,&DbKind::of("sqlite"),&proof.metadata,proof.source.metadata_revision.clone(),&current.principal,current.endpoint_fingerprint.clone())?;
    tx.rollback().await?;
    Ok(PreparedQueryMutation { plan,source:QueryMutationSource { binding:resolved.binding,source:proof.source.clone(),result_token:request.result_token.clone(),execution_namespace:proof.source.schema.clone() } })

    }).await
}
/// Recheck current server identity and token lifetime inside the command's ordinary/runtime lease.
/// Parent invokes this after taking the one-shot C2 plan, then validates the source in the write tx.
pub(crate) fn validate_context(store:&ResultStore,source:&QueryMutationSource,current:&ResultBinding)->Result<()> {
    if source.binding!=*current { bail!("QUERY_CONTEXT_CHANGED：页面/执行/代际/端点已变化"); }
    let actual=store.resolve(&source.result_token,None,current)?;
    if actual.proof.source!=source.source { bail!("QUERY_SOURCE_CHANGED：来源证据与冻结计划不一致"); } Ok(())
}

#[cfg(test)]
#[path="query_result_mutation_tests.rs"]
mod tests;
