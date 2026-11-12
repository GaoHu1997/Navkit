//! Real temporary SQLite: production C1 capture -> evidence IPC -> existing one-shot C2 service.
use super::*;
#[path = "query_cleanup_debug.rs"]
mod cleanup_debug;
use std::{path::PathBuf, time::Duration};
use crate::{commands, db::{self, Db}, execution_contracts::{ExecutionOutcome, ExecutionRequest},
    models::{ConnectionConfig, DbKind}, mutation::DbValue, storage::Store};
const PRINCIPAL: &str = "query-test-window";
const OWNER: &str = "query-test-page";
const TABLE: &str = "CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT NOT NULL,b BLOB,n INTEGER DEFAULT 7)";
struct Fixture { state: Arc<AppState>, observer: sqlx::SqlitePool, root: PathBuf }
impl Fixture {
    async fn new(read_only: bool) -> Self {
        let root = std::env::temp_dir().join(format!("navkit-query-service-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let options = sqlx::sqlite::SqliteConnectOptions::new().filename(root.join("test.sqlite"))
            .create_if_missing(true).journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
        let observer = sqlx::sqlite::SqlitePoolOptions::new().max_connections(2).connect_with(options).await.unwrap();
        let mut setup = observer.acquire().await.unwrap();
        sqlx::query(TABLE).execute(&mut *setup).await.unwrap();
        sqlx::query("INSERT INTO items VALUES(9223372036854775807,'原值',X'0000FF',NULL)").execute(&mut *setup).await.unwrap();
        setup.return_to_pool().await;
        let store = Store::empty_at(&root);
        let cfg = ConnectionConfig { id:"query-connection".into(), name:"临时查询服务测试".into(), kind:DbKind::of("sqlite"),
            host:String::new(),port:0,user:String::new(),password:String::new(),database:"main".into(),
            file:root.join("test.sqlite").to_string_lossy().into(),params:String::new(),group:String::new(),read_only,keep_alive:false };
        store.save_connections(vec![cfg.clone()],vec![]).unwrap();
        let state=Arc::new(AppState { store,registry:Arc::new(db::DbRegistry::default()),manual_transactions:Default::default(),
            activity:Default::default(),executions:Default::default(),mutations:Default::default(),connection_runtime:Default::default(),query_results:Default::default() });
        let conn=Arc::new(db::open_saved(&db::request_from_config(&cfg,""),read_only,&state.store.settings()).await.unwrap());
        let fingerprint=crate::db::runtime::binding_fingerprint_at(&cfg,state.store.data_root()).unwrap();
        state.connection_runtime.publish(&cfg.id,&fingerprint,&conn,None,crate::db::runtime::keepalive_policy(&cfg).unwrap()).unwrap();
        state.registry.write().insert(cfg.id,conn);
        Self{state,observer,root}
    }
    fn request(&self, sql:&str, namespace:&str) -> ExecutionRequest {
        let mut request=ExecutionRequest { execution_id:String::new(), connection_id:"query-connection".into(),namespace:namespace.into(),owner_id:OWNER.into(),
            sql:sql.into(),transaction_owner:None,timeout_ms:Some(5_000),origin:Some("query".into()),execution_grant:None,cancel_before_send:false };
        commands::grant_execution_request(&self.state,&mut request,PRINCIPAL).unwrap(); request
    }
    async fn execute(&self, sql:&str, namespace:&str) -> (QueryContext,ExecutionOutcome) {
        let request=self.request(sql,namespace);
        let context=QueryContext { execution_id:request.execution_id.clone(),connection_id:request.connection_id.clone(),owner_id:request.owner_id.clone(),namespace:request.namespace.clone(),result_index:0 };
        let outcome=tokio::time::timeout(Duration::from_secs(60),commands::execute_sql_service(&self.state,request,PRINCIPAL)).await.expect("C1 capture did not drain").unwrap();
        (context,outcome)
    }
    async fn select(&self, namespace:&str) -> (QueryContext,RegisteredResult) {
        let(context,outcome)=self.execute("SELECT * FROM items",namespace).await;
        assert_eq!(outcome.state,ExecutionState::Succeeded,"{:?}",outcome.error); assert!(!outcome.resources_pending);
        let result=snapshot_service(&self.state,PRINCIPAL,&context).unwrap();
        assert_eq!(result.editability.mode,"single_table"); assert_eq!(result.loaded_rows,1);
        (context,result)
    }
    fn update(result:&RegisteredResult,value:&str) -> QueryRowChange {
        let id=result.editability.columns.iter().find(|column|column.base_column=="v").unwrap().result_column_id.clone();
        QueryRowChange { result_token:result.editability.result_token.clone().unwrap(),row_token:Some(result.rows[0].row_token.clone()),operation:"update".into(),changes:vec![(id,DbValue::Text(value.into()))] }
    }
    async fn value(&self) -> String {
        let mut observer = self.observer.acquire().await.unwrap();
        let value = sqlx::query_scalar("SELECT v FROM items WHERE id=9223372036854775807").fetch_one(&mut *observer).await.unwrap();
        observer.return_to_pool().await;
        value
    }
    async fn cleanup(self) {
        let Self { state, observer, root } = self;
        drop(state.manual_transactions.close_all().await.unwrap());
        let conns:Vec<_>=state.registry.write().drain().map(|(_,db)|db).collect();
        for conn in conns { if let Db::Sqlite(pool)=conn.as_ref() {
            pool.close().await;
            // 关键不变量：池进入关闭态后拒绝新连接。物理连接数取决于 fixture 中其它 Arc 持有者
            // 的释放时序（Windows sqlx 异步回收），不作为此清理步骤的断言。
            assert!(pool.is_closed(), "production pool did not enter the closed state");
            assert!(pool.acquire().await.is_err(), "closed pool must reject new acquisitions");
        } }
        observer.close().await;
        assert!(observer.is_closed());
        assert_eq!(observer.size(),0,"observer still owns SQLite connections after close");
        drop(observer);
        drop(state);
        if let Err(error)=std::fs::remove_dir_all(&root) {
            let remaining=std::fs::read_dir(&root).map(|entries|entries.filter_map(|entry|entry.ok().map(|entry|entry.file_name())).collect::<Vec<_>>());
            let resources = [root.join("test.sqlite"), root.join("test.sqlite-shm"), root.join("test.sqlite-wal")];
            let owners = cleanup_debug::describe_file_users(&resources);
            panic!("closed query fixture cleanup failed at {}: {error:?}; remaining={remaining:?}; {owners}",root.display());
        }
    }
}
#[tokio::test]
async fn production_capture_to_c2_is_lossless_bound_and_consumed_once() {
    let f=Fixture::new(false).await;let(context,mut result)=f.select("main").await;
    assert_eq!(result.rows[0].values[0],DbValue::Integer("9223372036854775807".into()));
    assert_eq!(result.rows[0].values[2],DbValue::Binary("AAD/".into()));assert_eq!(result.rows[0].values[3],DbValue::Null);
    let change=Fixture::update(&result,"new'); DELETE FROM items; --");
    // A detached IPC snapshot must not become the source of old values.
    result.rows[0].values[1]=DbValue::Text("forged old value".into());
    let plan=preview_service(&f.state,PRINCIPAL,&context,&change).await.unwrap();
    assert_eq!(f.value().await,"原值");assert!(!plan.sql.contains("DELETE FROM items"));
    let receipt=commands::execute_row_mutation_service(&f.state,PRINCIPAL,&plan.plan_id,OWNER).await.unwrap();
    assert_eq!(receipt.plan_id,plan.plan_id);assert_eq!(receipt.status,"applied");assert_eq!(receipt.transaction,"committed");assert!(receipt.verified);assert_eq!(receipt.affected,1);
    assert_eq!(f.value().await,"new'); DELETE FROM items; --");
    assert!(commands::execute_row_mutation_service(&f.state,PRINCIPAL,&plan.plan_id,OWNER).await.is_err());
    assert!(snapshot_service(&f.state,PRINCIPAL,&context).is_err());
    assert!(preview_service(&f.state,PRINCIPAL,&context,&change).await.is_err());f.cleanup().await;
}
#[tokio::test]
async fn empty_execution_namespace_does_not_replace_proved_main_schema() {
    let f=Fixture::new(false).await;let(context,result)=f.select("").await;
    assert_eq!(result.editability.schema.as_deref(),Some("main"));
    let plan=preview_service(&f.state,PRINCIPAL,&context,&Fixture::update(&result,"空命名空间")).await.unwrap();
    let receipt=commands::execute_row_mutation_service(&f.state,PRINCIPAL,&plan.plan_id,OWNER).await.unwrap();
    assert!(receipt.verified);assert_eq!(receipt.transaction,"committed");assert_eq!(f.value().await,"空命名空间");f.cleanup().await;
}
#[tokio::test]
async fn foreign_window_owner_namespace_and_row_tokens_never_send_dml() {
    let f=Fixture::new(false).await;let(context,result)=f.select("main").await;let change=Fixture::update(&result,"forged");
    assert!(snapshot_service(&f.state,"foreign-window",&context).is_err());
    let mut wrong=context.clone();wrong.owner_id="foreign-page".into();assert!(preview_service(&f.state,PRINCIPAL,&wrong,&change).await.is_err());
    wrong=context.clone();wrong.namespace="".into();assert!(snapshot_service(&f.state,PRINCIPAL,&wrong).is_err());
    let mut row=change.clone();row.row_token=Some("row-index-0".into());assert!(preview_service(&f.state,PRINCIPAL,&context,&row).await.is_err());
    let plan=preview_service(&f.state,PRINCIPAL,&context,&change).await.unwrap();
    assert!(commands::execute_row_mutation_service(&f.state,"foreign-window",&plan.plan_id,OWNER).await.is_err());
    assert!(commands::execute_row_mutation_service(&f.state,PRINCIPAL,&plan.plan_id,"foreign-page").await.is_err());
    assert_eq!(f.value().await,"原值");
    assert!(commands::execute_row_mutation_service(&f.state,PRINCIPAL,&plan.plan_id,OWNER).await.unwrap().verified);f.cleanup().await;
}
#[tokio::test]
async fn same_ddl_aba_after_preview_is_rejected_inside_the_real_write_transaction() {
    let f=Fixture::new(false).await;let(context,result)=f.select("main").await;
    let plan=preview_service(&f.state,PRINCIPAL,&context,&Fixture::update(&result,"must not write")).await.unwrap();
    let mut observer = f.observer.acquire().await.unwrap();
    sqlx::query("DROP TABLE items").execute(&mut *observer).await.unwrap();sqlx::query(TABLE).execute(&mut *observer).await.unwrap();
    sqlx::query("INSERT INTO items VALUES(9223372036854775807,'原值',X'0000FF',NULL)").execute(&mut *observer).await.unwrap();
    observer.return_to_pool().await;
    assert!(commands::execute_row_mutation_service(&f.state,PRINCIPAL,&plan.plan_id,OWNER).await.is_err());
    assert_eq!(f.value().await,"原值");assert!(commands::execute_row_mutation_service(&f.state,PRINCIPAL,&plan.plan_id,OWNER).await.is_err());f.cleanup().await;
}
#[tokio::test]
async fn newer_readonly_query_revokes_old_rows_and_old_release_cannot_revoke_the_new_result() {
    let f=Fixture::new(false).await;let(old,result)=f.select("main").await;
    let plan=preview_service(&f.state,PRINCIPAL,&old,&Fixture::update(&result,"stale")).await.unwrap();
    let(_,outcome)=f.execute("SELECT 1 AS read_only_result","main").await;assert_eq!(outcome.state,ExecutionState::Succeeded);
    assert!(snapshot_service(&f.state,PRINCIPAL,&old).is_err());
    assert!(commands::execute_row_mutation_service(&f.state,PRINCIPAL,&plan.plan_id,OWNER).await.is_err());
    let(new,latest)=f.select("main").await;release_service(&f.state,PRINCIPAL,&old).unwrap();
    let current=snapshot_service(&f.state,PRINCIPAL,&new).unwrap();assert_eq!(current.editability.result_token,latest.editability.result_token);
    assert_eq!(f.value().await,"原值");f.cleanup().await;
}
#[tokio::test]
async fn page_write_guard_blocks_same_page_replacement_without_consuming_the_new_grant() {
    let f=Fixture::new(false).await;let(context,result)=f.select("main").await;
    let preview=preview_service(&f.state,PRINCIPAL,&context,&Fixture::update(&result,"unused")).await.unwrap();
    let plan=f.state.mutations.take(&preview.plan_id,PRINCIPAL,OWNER).unwrap();
    let guard=pin_plan(&f.state,&plan,PRINCIPAL,OWNER).unwrap().unwrap();
    let request=f.request("SELECT 42","main");
    let error=commands::execute_sql_service(&f.state,request.clone(),PRINCIPAL).await.unwrap_err();assert_eq!(error.code,"QUERY_MUTATION_IN_PROGRESS");
    assert!(release_service(&f.state,PRINCIPAL,&context).is_err());
    drop(guard);
    let outcome=commands::execute_sql_service(&f.state,request,PRINCIPAL).await.unwrap();assert_eq!(outcome.state,ExecutionState::Succeeded);
    assert_eq!(f.value().await,"原值");f.cleanup().await;
}
#[tokio::test]
async fn readonly_saved_permission_and_actual_connection_retirement_revoke_capability() {
    let f=Fixture::new(false).await;let(context,result)=f.select("main").await;
    let preview=preview_service(&f.state,PRINCIPAL,&context,&Fixture::update(&result,"denied")).await.unwrap();
    let mut configs=f.state.store.connections();configs[0].read_only=true;f.state.store.save_connections(configs,vec![]).unwrap();
    assert!(preview_service(&f.state,PRINCIPAL,&context,&Fixture::update(&result,"denied")).await.is_err());
    assert!(commands::execute_row_mutation_service(&f.state,PRINCIPAL,&preview.plan_id,OWNER).await.is_err());assert_eq!(f.value().await,"原值");f.cleanup().await;
    let f=Fixture::new(false).await;let(context,_)=f.select("main").await;
    crate::runtime_commands::retire(&f.state,"query-connection",()).await.unwrap();
    assert!(snapshot_service(&f.state,PRINCIPAL,&context).is_err());assert_eq!(f.value().await,"原值");f.cleanup().await;
}
#[tokio::test]
async fn non_single_table_and_readonly_queries_keep_original_display_without_edit_evidence() {
    let f=Fixture::new(false).await;
    for sql in ["SELECT count(*) FROM items","SELECT a.id,a.v FROM items a JOIN items b ON a.id=b.id","SELECT v FROM items"] {
        let(context,outcome)=f.execute(sql,"main").await;assert_eq!(outcome.state,ExecutionState::Succeeded,"{:?}",outcome.error);
        assert_eq!(outcome.result.as_ref().unwrap().sets[0].total,1);
        if let Ok(snapshot)=snapshot_service(&f.state,PRINCIPAL,&context) {assert_eq!(snapshot.editability.mode,"read_only");assert!(snapshot.editability.result_token.is_none());assert!(snapshot.rows.is_empty());}
    }
    f.cleanup().await;
    let f=Fixture::new(true).await;let(context,outcome)=f.execute("SELECT * FROM items","main").await;
    assert_eq!(outcome.state,ExecutionState::Succeeded);assert_eq!(outcome.result.unwrap().sets[0].rows[0][1].as_deref(),Some("原值"));
    assert!(snapshot_service(&f.state,PRINCIPAL,&context).is_err());f.cleanup().await;
}
