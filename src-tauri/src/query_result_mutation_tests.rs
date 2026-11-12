use super::*;
use crate::query_lineage::{capture_sqlite_select,CaptureAttempt,CaptureLimits};
async fn fixture()->(Db,ResultStore,ResultBinding,crate::query_result_store::RegisteredResult) {
    let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
    sqlx::query("CREATE TABLE t(id INTEGER PRIMARY KEY,v TEXT NOT NULL,b BLOB)").execute(&pool).await.unwrap();sqlx::query("INSERT INTO t VALUES(9223372036854775807,'old',X'00FF')").execute(&pool).await.unwrap();
    let mut conn=pool.acquire().await.unwrap();let CaptureAttempt::Executed(c)=capture_sqlite_select(&mut conn,"SELECT * FROM t",CaptureLimits::default(),|_,_,_|Ok(())).await.unwrap() else{panic!()};drop(conn);
    let binding=ResultBinding{principal:"main".into(),owner_id:"page".into(),connection_id:"conn".into(),endpoint_fingerprint:"a".repeat(64),source_execution_id:uuid::Uuid::new_v4().to_string(),result_index:0,generation:uuid::Uuid::new_v4().to_string()};let store=ResultStore::default();let registered=store.register(binding.clone(),&c).unwrap();(Db::Sqlite(pool),store,binding,registered)
}
fn request(result:&crate::query_result_store::RegisteredResult)->QueryRowChange {QueryRowChange{result_token:result.editability.result_token.clone().unwrap(),row_token:Some(result.rows[0].row_token.clone()),operation:"update".into(),changes:vec![(result.editability.columns[1].result_column_id.clone(),DbValue::Text("new'); DELETE FROM t; --".into()))]}}
#[test]
fn ipc_refuses_client_sql_table_keys_and_old_values() {
    for field in ["sql","table","keys","original_values","endpoint_fingerprint","source"] {let mut value=serde_json::json!({"result_token":"r","row_token":"row","operation":"update","changes":[]});value[field]=serde_json::json!("injected");assert!(serde_json::from_value::<QueryRowChange>(value).is_err());}
}
#[tokio::test]
async fn c2_plan_uses_server_key_and_typed_old_values_with_source_for_atomic_validation() {
    let(db,store,binding,result)=fixture().await;let req=request(&result);let prepared=prepare_row_plan(&db,&store,&binding,&req,false).await.unwrap();
    assert_eq!(prepared.plan.keys,vec![("id".into(),DbValue::Integer("9223372036854775807".into()))]);assert!(prepared.plan.conditions.contains(&("v".into(),DbValue::Text("old".into()))));assert!(!prepared.plan.sql.contains("DELETE FROM t"));assert!(prepared.plan.sql.contains('?'));assert_eq!(prepared.source.source.schema,"main");validate_context(&store,&prepared.source,&binding).unwrap();
    // Under unchanged metadata this is the existing C2 same-connection CAS/rowcount/readback path.
    // Production caller must attach prepared.source before Registry::put (the parent owns that hook).
    let receipt=mutation::run_plan(&db,&DbKind::of("sqlite"),&prepared.plan,&prepared.plan.base_revision).await.unwrap();assert!(receipt.verified);assert_eq!(receipt.affected,1);
    let Db::Sqlite(pool)=db else{panic!()};let v:String=sqlx::query_scalar("SELECT v FROM t").fetch_one(&pool).await.unwrap();assert_eq!(v,"new'); DELETE FROM t; --");
}
#[tokio::test]
async fn data_changed_after_capture_is_a_real_c2_conflict_with_no_write() {
    let(db,store,binding,result)=fixture().await;let Db::Sqlite(pool)=&db else{panic!()};sqlx::query("UPDATE t SET v='external'").execute(pool).await.unwrap();let prepared=prepare_row_plan(&db,&store,&binding,&request(&result),false).await.unwrap();let receipt=mutation::run_plan(&db,&DbKind::of("sqlite"),&prepared.plan,&prepared.plan.base_revision).await.unwrap();assert_eq!(receipt.status,"conflict");assert_eq!(receipt.affected,0);let v:String=sqlx::query_scalar("SELECT v FROM t").fetch_one(pool).await.unwrap();assert_eq!(v,"external");
}
#[tokio::test]
async fn readonly_wrong_row_changed_generation_and_key_column_are_refused() {
    let(db,store,binding,result)=fixture().await;let req=request(&result);assert!(prepare_row_plan(&db,&store,&binding,&req,true).await.is_err());let mut wrong=binding.clone();wrong.generation=uuid::Uuid::new_v4().to_string();assert!(prepare_row_plan(&db,&store,&wrong,&req,false).await.is_err());let mut req=req;req.row_token=Some("row-index-0".into());assert!(prepare_row_plan(&db,&store,&binding,&req,false).await.is_err());req.row_token=Some(result.rows[0].row_token.clone());req.changes=vec![(result.editability.columns[0].result_column_id.clone(),DbValue::Integer("9".into()))];assert!(prepare_row_plan(&db,&store,&binding,&req,false).await.is_err());
}
#[tokio::test]
async fn same_ddl_aba_is_refused_at_preview_and_delete_needs_full_old_projection() {
    let(db,store,binding,result)=fixture().await;let Db::Sqlite(pool)=&db else{panic!()};sqlx::query("DROP TABLE t").execute(pool).await.unwrap();sqlx::query("CREATE TABLE t(id INTEGER PRIMARY KEY,v TEXT NOT NULL,b BLOB)").execute(pool).await.unwrap();sqlx::query("INSERT INTO t VALUES(9223372036854775807,'old',X'00FF')").execute(pool).await.unwrap();assert!(prepare_row_plan(&db,&store,&binding,&request(&result),false).await.is_err());
    let mut conn=pool.acquire().await.unwrap();let CaptureAttempt::Executed(c)=capture_sqlite_select(&mut conn,"SELECT id,v FROM t",CaptureLimits::default(),|_,_,_|Ok(())).await.unwrap() else{panic!()};drop(conn);let mut next=binding;next.generation=uuid::Uuid::new_v4().to_string();let partial=store.register(next.clone(),&c).unwrap();assert!(!partial.editability.actions.delete.supported);let mut req=request(&partial);req.operation="delete".into();req.changes.clear();assert!(prepare_row_plan(&db,&store,&next,&req,false).await.is_err());
}
