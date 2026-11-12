use super::*;
use sqlx::Connection;
use crate::query_lineage::{capture_sqlite_select,CaptureAttempt,CaptureLimits,CapturedSelect};
async fn capture()->CapturedSelect {
    let mut conn=sqlx::SqliteConnection::connect("sqlite::memory:").await.unwrap();
    sqlx::query("CREATE TABLE t(id INTEGER PRIMARY KEY,v TEXT,b BLOB)").execute(&mut conn).await.unwrap();sqlx::query("INSERT INTO t VALUES(1,'old',X'00FF'),(2,'',NULL)").execute(&mut conn).await.unwrap();
    let CaptureAttempt::Executed(c)=capture_sqlite_select(&mut conn,"SELECT * FROM t",CaptureLimits::default(),|_,_,_|Ok(())).await.unwrap() else{panic!()};c
}
fn binding()->ResultBinding {ResultBinding{principal:"main".into(),owner_id:"page".into(),connection_id:"conn".into(),endpoint_fingerprint:"a".repeat(64),source_execution_id:uuid::Uuid::new_v4().to_string(),result_index:0,generation:uuid::Uuid::new_v4().to_string()}}
#[tokio::test]
async fn tokens_bind_all_identity_fields_and_never_accept_foreign_row_or_display_values() {
    let store=ResultStore::default();let binding=binding();let c=capture().await;let mut registered=store.register(binding.clone(),&c).unwrap();let token=registered.editability.result_token.clone().unwrap();let row=registered.rows[0].row_token.clone();
    registered.rows[0].values[1]=DbValue::Text("tampered".into());assert_eq!(store.resolve(&token,Some(&row),&binding).unwrap().values.unwrap()[1],DbValue::Text("old".into()));
    for field in 0..8 {let mut wrong=binding.clone();match field{0=>wrong.principal.push('x'),1=>wrong.owner_id.push('x'),2=>wrong.connection_id.push('x'),3=>wrong.endpoint_fingerprint="b".repeat(64),4=>wrong.source_execution_id=uuid::Uuid::new_v4().to_string(),5=>wrong.result_index+=1,6=>wrong.generation=uuid::Uuid::new_v4().to_string(),_=>wrong.principal.clear()}assert!(store.resolve(&token,Some(&row),&wrong).is_err());}
    assert!(store.resolve(&token,Some("row-index-0"),&binding).is_err());assert!(store.resolve("not-a-result",Some(&row),&binding).is_err());
    assert!(store.resolve(&token,Some(&row),&binding).is_ok(),"foreign attempt did not consume owner evidence");
}
#[tokio::test]
async fn rerun_expiry_close_and_other_owner_isolation_are_real_cache_events() {
    let store=ResultStore::default();let mut binding=binding();let c=capture().await;let first=store.register(binding.clone(),&c).unwrap().editability.result_token.unwrap();
    let mut other=binding.clone();other.owner_id="other".into();let other_token=store.register(other.clone(),&c).unwrap().editability.result_token.unwrap();
    binding.generation=uuid::Uuid::new_v4().to_string();let next=store.register(binding.clone(),&c).unwrap().editability.result_token.unwrap();assert!(store.resolve(&first,None,&binding).is_err());assert!(store.resolve(&other_token,None,&other).is_ok());
    store.entries.lock().get_mut(&next).unwrap().created=Instant::now()-Duration::from_secs(121);assert!(store.resolve(&next,None,&binding).is_err());
    store.clear_owner(&other.principal,&other.owner_id);assert!(store.resolve(&other_token,None,&other).is_err());
    let token=store.register(binding.clone(),&c).unwrap().editability.result_token.unwrap();store.clear_connection(&binding.connection_id);assert!(store.resolve(&token,None,&binding).is_err());
}
#[tokio::test]
async fn bounded_cache_eviction_counts_full_typed_evidence_and_all_actions_are_explicit() {
    let store=ResultStore::default();let mut b=binding();let c=capture().await;let first=store.register(b.clone(),&c).unwrap();let token=first.editability.result_token.unwrap();assert!(first.editability.actions.update.supported);assert!(first.editability.actions.delete.supported);assert!(first.editability.actions.insert.supported);
    for i in 0..RESULT_LIMIT {b.owner_id=format!("owner-{i}");store.register(b.clone(),&c).unwrap();}
    assert_eq!(store.entries.lock().len(),RESULT_LIMIT);assert!(!store.entries.lock().contains_key(&token));assert!(store.entries.lock().values().map(|e|e.bytes).sum::<usize>()<=RESULT_CACHE_BYTES);
    let mut c=c; c.proof=None;c.read_only_reason="缺少完整定位键".into();let readonly=store.register(b,&c).unwrap();assert!(readonly.editability.result_token.is_none());assert!(!readonly.editability.actions.update.supported);assert!(readonly.rows.is_empty());assert_eq!(readonly.total_rows,2);
}
#[tokio::test]
async fn readonly_replacement_also_invalidates_old_generation_but_not_another_result_slot() {
    let store=ResultStore::default();let b=binding();let mut c=capture().await;
    let token=store.register(b.clone(),&c).unwrap().editability.result_token.unwrap();
    let mut other=b.clone();other.result_index=1;let other_token=store.register(other.clone(),&c).unwrap().editability.result_token.unwrap();
    c.proof=None;c.typed_rows.clear();c.read_only_reason="新查询缺少定位键".into();
    let mut replacement=b.clone();replacement.generation=uuid::Uuid::new_v4().to_string();
    let readonly=store.register(replacement,&c).unwrap();assert_eq!(readonly.editability.mode,"read_only");assert!(readonly.editability.result_token.is_none());
    assert!(store.resolve(&token,None,&b).is_err());assert!(store.resolve(&other_token,None,&other).is_ok());
}
#[tokio::test]
async fn snapshot_preserves_real_capture_order_is_detached_budgeted_and_refuses_foreign_or_expired() {
    let mut conn=sqlx::SqliteConnection::connect("sqlite::memory:").await.unwrap();
    sqlx::query("CREATE TABLE ordered(id INTEGER PRIMARY KEY,v TEXT)").execute(&mut conn).await.unwrap();
    sqlx::query("INSERT INTO ordered VALUES(2,'two'),(1,'one'),(3,'three')").execute(&mut conn).await.unwrap();
    let CaptureAttempt::Executed(c)=capture_sqlite_select(&mut conn,"SELECT * FROM ordered ORDER BY id DESC",CaptureLimits::default(),|_,_,_|Ok(())).await.unwrap() else{panic!()};
    let store=ResultStore::default();let b=binding();let registered=store.register(b.clone(),&c).unwrap();let token=registered.editability.result_token.clone().unwrap();
    let expected_tokens=registered.rows.iter().map(|row|row.row_token.clone()).collect::<Vec<_>>();drop(registered);
    let mut snapshot=store.snapshot(&token,&b).unwrap();assert_eq!(snapshot.loaded_rows,3);assert_eq!(snapshot.total_rows,3);assert_eq!(snapshot.editability.result_token.as_deref(),Some(token.as_str()));
    assert_eq!(snapshot.rows.iter().map(|row|row.row_token.clone()).collect::<Vec<_>>(),expected_tokens);
    assert_eq!(snapshot.rows.iter().map(|row|row.values.clone()).collect::<Vec<_>>(),c.typed_rows);
    assert_eq!(snapshot.rows[0].values[0],DbValue::Integer("3".into()));assert_eq!(snapshot.rows[2].values[0],DbValue::Integer("1".into()));
    snapshot.rows[0].values[1]=DbValue::Text("client changed copy".into());snapshot.editability.columns[0].base_column="foreign".into();
    let fresh=store.snapshot(&token,&b).unwrap();assert_eq!(fresh.rows[0].values[1],DbValue::Text("three".into()));assert_eq!(fresh.editability.columns[0].base_column,"id");
    {let entries=store.entries.lock();let entry=entries.get(&token).unwrap();assert_eq!(entry.evidence.row_order,expected_tokens);assert_eq!(entry.evidence.rows.len(),3);assert_eq!(entry.bytes,serde_json::to_vec(&entry.evidence).unwrap().len());}
    let mut wrong=b.clone();wrong.owner_id="foreign".into();assert!(store.snapshot(&token,&wrong).is_err());wrong=b.clone();wrong.generation=uuid::Uuid::new_v4().to_string();assert!(store.snapshot(&token,&wrong).is_err());
    assert!(store.snapshot("row-number-0",&b).is_err());assert!(store.snapshot(&token,&b).is_ok());
    store.entries.lock().get_mut(&token).unwrap().created=Instant::now()-Duration::from_secs(121);assert!(store.snapshot(&token,&b).is_err());assert!(!store.entries.lock().contains_key(&token));
}
#[tokio::test]
async fn duplicate_null_keys_and_tampered_row_width_are_never_signed() {
    let store=ResultStore::default();let b=binding();let mut c=capture().await;c.typed_rows[1][0]=c.typed_rows[0][0].clone();assert!(store.register(b.clone(),&c).is_err());c.typed_rows[1][0]=DbValue::Null;assert!(store.register(b.clone(),&c).is_err());c.typed_rows[1].pop();assert!(store.register(b,&c).is_err());
}
