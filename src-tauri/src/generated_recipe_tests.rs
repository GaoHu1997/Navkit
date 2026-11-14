//! Actual pinned-session atomic recipe tests; only UUID-isolated SQLite fixtures.
use super::*;
use super::tests::{Fixture, record, run};
use crate::generated_recipe::{Recipe, Rule, Mode};
use crate::execution_contracts::ExecutionState;
#[path = "performance_rss_probe.rs"] mod performance_rss_probe;
fn recipe(total:u64)->Recipe {
    Recipe{version:1,connection_id:"isolated".into(),schema:"main".into(),table:"items".into(),total,seed:"确定性".into(),frozen_time:"2026-11-13T08:00:00Z".into(),schema_fingerprint:String::new(),transaction_mode:"atomic".into(),fields:vec![Rule{name:"id".into(),mode:Mode::Sequence,value:"10".into()},Rule{name:"text".into(),mode:Mode::Auto,value:String::new()}]}
}
fn payload(recipe:Recipe,preview:bool)->(Payload,Arc<Mutex<Option<crate::generated_recipe::Preview>>>) {
    let sample=Arc::new(Mutex::new(None));(Payload::Recipe(RecipeWork{recipe,preview,sample:sample.clone(),lease:None}),sample)
}
async fn preview(f:&Fixture,recipe:Recipe)->crate::generated_recipe::Preview {
    let(p,sample)=payload(recipe,true);let r=record(&p,0);let s=run(f,p,&r).await;
    assert_eq!(s.status.state,ExecutionState::Succeeded,"{:?}",s.outcome);assert_eq!(s.metrics.sent,0);assert_eq!(s.transaction_state,"none");
    let preview=sample.lock().take().unwrap();preview
}
#[tokio::test]
async fn recipe_preview_completion_accepts_success_without_stop_and_no_writes() {
    let f=Fixture::new().await;let(p,sample)=payload(recipe(1000),true);let r=record(&p,0);
    assert!(r.entry.start());
    let(result,stop_confirmed)=run_pinned(f.db.clone(),DbKind::of("sqlite"),r.clone(),p).await;
    assert!(result.is_ok(),"{result:?}");assert!(!stop_confirmed,"no stop was requested");
    finish_recipe_preview(&r,result,stop_confirmed).unwrap();
    let snapshot=r.snapshot(0);assert_eq!(snapshot.status.state,ExecutionState::Succeeded);
    assert!(!snapshot.status.resources_pending);assert_eq!(snapshot.transaction_state,"none");assert_eq!(snapshot.metrics.sent,0);
    assert_eq!(sample.lock().as_ref().unwrap().rows.len(),20);assert_eq!(f.count("items").await,0);
    f.cleanup().await;
}
#[test]
fn recipe_preview_completion_preserves_failure_and_uncertain_cleanup() {
    for uncertain in [false,true] {
        let(p,_)=payload(recipe(1000),true);let r=record(&p,0);r.entry.start();r.entry.work_settled();r.transaction(if uncertain{"unknown"}else{"none"});
        let code=if uncertain{"EXECUTION_EFFECTS_UNKNOWN"}else{"RECIPE_PREVIEW_TEST_FAILED"};
        let error=finish_recipe_preview(&r,Err(AppError::coded(code,"original preview/close diagnostic").into()),true).unwrap_err();
        assert_eq!(error.code,code);assert!(error.message.contains("original preview/close diagnostic"));
        assert_eq!(r.snapshot(0).status.state,if uncertain{ExecutionState::Unknown}else{ExecutionState::Failed});
    }
}
#[test]
fn recipe_preview_completion_rejects_cancel_timeout_and_unconfirmed_stop() {
    for timeout in [false,true] {for confirmed in [false,true] {
        let(p,_)=payload(recipe(1000),true);let r=record(&p,0);r.entry.start();assert!(r.entry.request_stop(timeout).accepted);r.entry.work_settled();r.transaction("none");
        let error=finish_recipe_preview(&r,Err(stop_error()),confirmed).unwrap_err();
        let expected=if !confirmed{ExecutionState::Unknown}else if timeout{ExecutionState::TimedOut}else{ExecutionState::Cancelled};
        assert_eq!(r.snapshot(0).status.state,expected);
        assert_eq!(error.code,if !confirmed{"EXECUTION_EFFECTS_UNKNOWN"}else if timeout{"QUERY_TIMEOUT"}else{"CANCELLED"});
    }}
}
#[test]
fn recipe_preview_completion_rejects_pending_resources_or_writes() {
    for guard in 0..3 {
        let(p,_)=payload(recipe(1000),true);let r=record(&p,0);r.entry.start();r.entry.work_settled();r.transaction("none");
        match guard {0=>r.entry.quarantine(),1=>r.transaction("committed"),_=>r.progress.lock().sent=1};
        let error=finish_recipe_preview(&r,Ok(r.result()),false).unwrap_err();
        assert_eq!(error.code,"EXECUTION_EFFECTS_UNKNOWN");
        if guard==0 {assert!(r.snapshot(0).status.resources_pending);}
    }
}
#[test]
fn recipe_preview_public_scope_passes_real_registry_and_binds_payload() {
    let draft=recipe(1000);let original=recipe_preview_scope(&draft).unwrap();
    assert!(original.starts_with("typed/v1:recipe-preview:"));
    let registry=execution::Registry::default();
    let binding=execution::Binding{principal:"native-window".into(),owner:"preview-owner".into(),connection:"isolated".into(),namespace:"main".into(),fingerprint:"physical-fixture".into()};
    let grant=registry.grant_typed_scope(binding.clone(),&original).unwrap();
    for change in 0..4 {
        let mut other=draft.clone();match change {0=>other.total+=1,1=>other.seed.push('x'),2=>other.fields[0].value="99".into(),_=>other.transaction_mode="segmented".into()};
        let scope=recipe_preview_scope(&other).unwrap();assert_ne!(original,scope);
        assert_eq!(registry.register_typed_scope(&grant.execution_id,&grant.token,binding.clone(),&scope,Some(0),30).err().unwrap().code,"EXECUTION_GRANT_INVALID");
    }
    let entry=registry.register_typed_scope(&grant.execution_id,&grant.token,binding.clone(),&original,Some(0),30).unwrap();
    assert_eq!(entry.status().state,ExecutionState::Queued);
    assert!(registry.register_typed_scope(&grant.execution_id,&grant.token,binding,&original,Some(0),30).is_err());
}
#[tokio::test]
async fn recipe_native_preview_multiblock_atomic_commit_exact_values() {
    let f=Fixture::new().await;let p=preview(&f,recipe(2501)).await;assert_eq!(p.rows.len(),20);assert_eq!(f.count("items").await,0);
    let(task,_)=payload(p.recipe,false);let r=record(&task,0);let s=run(&f,task,&r).await;
    assert_eq!(s.status.state,ExecutionState::Succeeded,"{:?}",s.outcome);assert_eq!(s.transaction_state,"committed");
    assert_eq!((s.metrics.generated,s.metrics.sent,s.metrics.acknowledged,s.metrics.committed,s.affected_rows),(2501,2501,2501,2501,2501));
    let actual:Vec<(i64,String)>=sqlx::query_as("SELECT id,text FROM items ORDER BY id LIMIT 20").fetch_all(&f.pool).await.unwrap();
    for(i,(id,text))in actual.iter().enumerate(){assert_eq!(p.rows[i],vec![Some(id.to_string()),Some(text.clone())]);}
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT MAX(id) FROM items").fetch_one(&f.pool).await.unwrap(),2510);
    f.cleanup().await;
}
#[tokio::test]
async fn recipe_schema_drift_stops_before_first_bound_row() {
    let f=Fixture::new().await;let p=preview(&f,recipe(1000)).await;
    sqlx::query("CREATE TRIGGER changed AFTER INSERT ON items BEGIN SELECT 1; END").execute(&f.pool).await.unwrap();
    let(task,_)=payload(p.recipe,false);let r=record(&task,0);let s=run(&f,task,&r).await;
    assert_eq!(s.status.state,ExecutionState::Failed);assert_eq!(s.transaction_state,"rolled_back");assert_eq!(s.metrics.sent,0);assert_eq!(s.metrics.committed,0);assert_eq!(f.count("items").await,0);
    assert_eq!(s.outcome.unwrap().error.unwrap().code,"RECIPE_SCHEMA_CHANGED");f.cleanup().await;
}
#[tokio::test]
async fn recipe_unique_failure_rolls_back_all_acknowledged_rows() {
    let f=Fixture::new().await;let mut draft=recipe(1000);draft.fields[0].mode=Mode::Constant;draft.fields[0].value="10".into();let p=preview(&f,draft).await;
    let(task,_)=payload(p.recipe,false);let r=record(&task,0);let s=run(&f,task,&r).await;
    assert_eq!(s.status.state,ExecutionState::Failed);assert_eq!(s.transaction_state,"rolled_back");assert_eq!((s.metrics.sent,s.metrics.acknowledged,s.metrics.errors,s.metrics.committed),(2,2,1,0));assert_eq!(s.affected_rows,0);assert_eq!(f.count("items").await,0);f.cleanup().await;
}
#[tokio::test]
async fn recipe_cancel_drains_native_vm_and_rolls_back() {
    let f=Fixture::new().await;
    sqlx::query("CREATE TRIGGER slow BEFORE INSERT ON items BEGIN SELECT (WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n); END").execute(&f.pool).await.unwrap();
    let p=preview(&f,recipe(1000000)).await;let(task,_)=payload(p.recipe,false);let r=record(&task,0);r.entry.start();let working=r.clone();let db=f.db.clone();
    let task=tokio::spawn(send_work(Box::pin(run_pinned(db,DbKind::of("sqlite"),working,task))));
    tokio::time::timeout(Duration::from_secs(5),async{while r.snapshot(0).metrics.sent<1{tokio::time::sleep(Duration::from_millis(1)).await;}}).await.unwrap();r.entry.request_stop(false);
    let(result,confirmed)=tokio::time::timeout(Duration::from_secs(10),task).await.unwrap().unwrap();assert!(confirmed);r.publish(result.map_err(|e|safe_error(&e)),confirmed);
    let s=r.snapshot(0);assert_eq!(s.status.state,ExecutionState::Cancelled);assert_eq!(s.transaction_state,"rolled_back");assert_eq!(s.metrics.committed,0);assert!(!s.status.resources_pending);assert_eq!(f.count("items").await,0);f.cleanup().await;
}
#[tokio::test]
async fn recipe_scope_binds_rules_seed_total_time_fingerprint_and_owner() {
    let f=Fixture::new().await;let recipe=preview(&f,recipe(1000)).await.recipe;let(p,_)=payload(recipe.clone(),false);
    let mut request=BatchExecutionRequest{connection_id:"isolated".into(),namespace:"main".into(),owner_id:"a".into(),timeout_ms:None,task:BatchTask::GeneratedRecipe{recipe:recipe.clone()}};let original=scope(&request,&p).unwrap();
    for which in 0..6 {let mut changed=recipe.clone();match which{0=>changed.seed.push('x'),1=>changed.total+=1,2=>changed.frozen_time="2026-11-14T08:00:00Z".into(),3=>changed.schema_fingerprint="b".repeat(64),4=>changed.transaction_mode="segmented".into(),_=>changed.fields[0].value="99".into()};let(other,_)=payload(changed,false);assert_ne!(scope(&request,&other).unwrap(),original);}
    request.owner_id="b".into();assert_ne!(scope(&request,&p).unwrap(),original);f.cleanup().await;
}
async fn benchmark(total:u64) {
    let memory=performance_rss_probe::Probe::start();
    let f=Fixture::new().await;let p=preview(&f,recipe(total)).await;let(task,_)=payload(p.recipe,false);let r=record(&task,0);let start=Instant::now();let s=run(&f,task,&r).await;
    assert_eq!(s.status.state,ExecutionState::Succeeded,"{:?}",s.outcome);assert_eq!(s.metrics.committed,total);assert_eq!(f.count("items").await,total as i64);
    let distinct:i64=sqlx::query_scalar("SELECT COUNT(DISTINCT id) FROM items").fetch_one(&f.pool).await.unwrap();assert_eq!(distinct,total as i64);
    println!("actual pinned SQLite recipe total={total} elapsed_ms={} generated={} sent={} acknowledged={} committed={} affected={} (one final COMMIT, row budget=64KiB, no speedup claim)",start.elapsed().as_millis(),s.metrics.generated,s.metrics.sent,s.metrics.acknowledged,s.metrics.committed,s.affected_rows);
    println!("PROCESS_RSS recipe_rows={total} {}",memory.finish());f.cleanup().await;
}
#[test]
fn recipe_readonly_scope_and_owner_permissions_fail_closed() {
    let mut draft=recipe(1000);draft.schema_fingerprint="a".repeat(64);
    let cfg=ConnectionConfig{id:"isolated".into(),name:"test".into(),kind:DbKind::of("sqlite"),host:String::new(),port:0,user:String::new(),password:String::new(),database:String::new(),file:String::new(),params:String::new(),group:String::new(),read_only:true,keep_alive:false};
    let request=BatchExecutionRequest{connection_id:"isolated".into(),namespace:"main".into(),owner_id:"a".into(),timeout_ms:None,task:BatchTask::GeneratedRecipe{recipe:draft.clone()}};
    assert_eq!(require_request(&request,&cfg).unwrap_err().code,"READ_ONLY_VIOLATION");
    let registry=execution::Registry::default();let binding=execution::Binding{principal:"window-a".into(),owner:"a".into(),connection:"isolated".into(),namespace:"main".into(),fingerprint:"isolated-physical".into()};
    let(p,_)=payload(draft,false);let scope=scope(&request,&p).unwrap();let grant=registry.grant_typed_scope(binding.clone(),&scope).unwrap();
    registry.register_typed_scope(&grant.execution_id,&grant.token,binding.clone(),&scope,Some(0),30).unwrap();
    assert!(registry.register_typed_scope(&grant.execution_id,&grant.token,binding.clone(),&scope,Some(0),30).is_err());
    let mut foreign=binding.clone();foreign.owner="b".into();assert!(registry.authorized(&grant.execution_id,&foreign).is_err());foreign=binding;foreign.principal="window-b".into();assert!(registry.authorized(&grant.execution_id,&foreign).is_err());
}
fn segmented_recipe(total:u64)->Recipe {let mut r=recipe(total);r.transaction_mode="segmented".into();r}
#[tokio::test]
async fn recipe_segmented_native_2501_rows_three_confirmed_commits() {
    let f=Fixture::new().await;let p=preview(&f,segmented_recipe(2501)).await;
    let(task,_)=payload(p.recipe,false);let r=record(&task,0);let s=run(&f,task,&r).await;
    assert_eq!(s.status.state,ExecutionState::Succeeded,"{:?}",s.outcome);assert_eq!(s.transaction_state,"committed");
    assert_eq!((s.metrics.generated,s.metrics.sent,s.metrics.acknowledged,s.metrics.committed,s.affected_rows),(2501,2501,2501,2501,2501));
    assert_eq!(s.logs.iter().filter(|l|l.phase=="commit"&&l.level=="success").count(),3);
    assert_eq!(f.count("items").await,2501);
    let actual:Vec<(i64,String)>=sqlx::query_as("SELECT id,text FROM items ORDER BY id LIMIT 20").fetch_all(&f.pool).await.unwrap();
    for(i,(id,text))in actual.iter().enumerate(){assert_eq!(p.rows[i],vec![Some(id.to_string()),Some(text.clone())]);}
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT text FROM items WHERE id=1010").fetch_one(&f.pool).await.unwrap(),"确定性_1001_text");
    f.cleanup().await;
}
#[tokio::test]
async fn recipe_segmented_native_unique_failures_1001_and_1501_keep_1000() {
    for failing_row in [1001u64,1501] {
        let f=Fixture::new().await;
        sqlx::query("INSERT INTO items VALUES(?,'preexisting')").bind((failing_row+9) as i64).execute(&f.pool).await.unwrap();
        let p=preview(&f,segmented_recipe(2501)).await;let(task,_)=payload(p.recipe,false);let r=record(&task,0);let s=run(&f,task,&r).await;
        assert_eq!(s.status.state,ExecutionState::Failed);assert_eq!(s.transaction_state,"rolled_back");
        assert_eq!((s.metrics.sent,s.metrics.acknowledged,s.metrics.errors,s.metrics.committed,s.affected_rows),(failing_row,failing_row,1,1000,1000));
        assert_eq!(s.outcome.as_ref().unwrap().effects,"possibly_partial");assert!(s.outcome.as_ref().unwrap().error.as_ref().unwrap().message.contains("已确认提交前缀 1000"));
        assert_eq!(f.count("items").await,1001);assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM items WHERE id BETWEEN 10 AND 1009").fetch_one(&f.pool).await.unwrap(),1000);
        assert_eq!(s.logs.iter().filter(|l|l.phase=="commit"&&l.level=="success").count(),1);
        f.cleanup().await;
    }
}
#[tokio::test]
async fn recipe_segmented_native_cancel_current_segment_keeps_prefix_and_lease() {
    let f=Fixture::new().await;
    sqlx::query("CREATE TRIGGER slow BEFORE INSERT ON items WHEN NEW.id=1010 BEGIN SELECT (WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n); END").execute(&f.pool).await.unwrap();
    let p=preview(&f,segmented_recipe(2501)).await;let(mut owned,_)=payload(p.recipe,false);
    let target=uuid::Uuid::new_v4().to_string();if let Payload::Recipe(work)=&mut owned{work.lease=Some(crate::generated_recipe::Lease::acquire(target.clone()).unwrap());}
    let r=record(&owned,0);r.entry.start();let working=r.clone();let db=f.db.clone();
    let task=tokio::spawn(send_work(Box::pin(run_pinned(db,DbKind::of("sqlite"),working,owned))));
    tokio::time::timeout(Duration::from_secs(10),async{while r.snapshot(0).metrics.sent<1001{tokio::time::sleep(Duration::from_millis(1)).await;}}).await.unwrap();
    assert_eq!(r.snapshot(0).metrics.committed,1000);assert!(crate::generated_recipe::Lease::acquire(target.clone()).is_err());assert!(r.entry.request_stop(false).accepted);
    let(result,confirmed)=tokio::time::timeout(Duration::from_secs(10),task).await.unwrap().unwrap();assert!(confirmed,"{result:?}");r.publish(result.map_err(|e|safe_error(&e)),confirmed);
    let s=r.snapshot(0);assert_eq!(s.status.state,ExecutionState::Cancelled);assert_eq!(s.transaction_state,"rolled_back");assert_eq!((s.metrics.committed,s.affected_rows),(1000,1000));assert_eq!(s.outcome.unwrap().effects,"possibly_partial");assert!(!s.status.resources_pending);assert_eq!(f.count("items").await,1000);
    assert!(crate::generated_recipe::Lease::acquire(target).is_ok());f.cleanup().await;
}
#[tokio::test]
async fn recipe_segmented_native_ddl_between_segments_revalidates_inside_write_lock() {
    let f=Fixture::new().await;let p=preview(&f,segmented_recipe(2501)).await;let(task,_)=payload(p.recipe,false);let r=record(&task,0);
    let pool=f.pool.clone();recipe_segment_hooks().lock().insert(r.entry.id.clone(),Box::new(move||Box::pin(async move{
        sqlx::query("CREATE TRIGGER changed AFTER INSERT ON items BEGIN SELECT 1; END").execute(&pool).await.unwrap();
    })));
    let s=run(&f,task,&r).await;
    assert_eq!(s.status.state,ExecutionState::Failed);assert_eq!(s.transaction_state,"rolled_back");assert_eq!((s.metrics.sent,s.metrics.committed,s.affected_rows),(1000,1000,1000));
    assert_eq!(s.outcome.unwrap().error.unwrap().code,"RECIPE_SCHEMA_CHANGED");assert_eq!(f.count("items").await,1000);f.cleanup().await;
}
#[tokio::test]
async fn recipe_segmented_native_stop_between_segments_has_confirmed_prefix_not_success() {
    let f=Fixture::new().await;let p=preview(&f,segmented_recipe(2501)).await;let(task,_)=payload(p.recipe,false);let r=record(&task,0);
    let entry=r.entry.clone();recipe_segment_hooks().lock().insert(r.entry.id.clone(),Box::new(move||Box::pin(async move{entry.request_stop(false);})));let s=run(&f,task,&r).await;
    assert_eq!(s.status.state,ExecutionState::Cancelled);assert_eq!(s.transaction_state,"committed");assert_eq!((s.metrics.sent,s.metrics.committed,s.affected_rows),(1000,1000,1000));
    assert!(s.outcome.unwrap().result.is_none());assert_eq!(f.count("items").await,1000);f.cleanup().await;
}
#[tokio::test]
async fn recipe_atomic_native_second_segment_position_failure_still_rolls_back_everything() {
    let f=Fixture::new().await;sqlx::query("INSERT INTO items VALUES(1510,'preexisting')").execute(&f.pool).await.unwrap();
    let p=preview(&f,recipe(2501)).await;let(task,_)=payload(p.recipe,false);let r=record(&task,0);let s=run(&f,task,&r).await;
    assert_eq!(s.status.state,ExecutionState::Failed);assert_eq!((s.metrics.sent,s.metrics.committed,s.affected_rows),(1501,0,0));assert_eq!(s.outcome.unwrap().effects,"no_effect");assert_eq!(f.count("items").await,1);f.cleanup().await;
}
#[tokio::test]
async fn recipe_segmented_native_byte_budget_precedes_1000_rows() {
    let f=Fixture::new().await;sqlx::query("ALTER TABLE items ADD COLUMN a TEXT; ALTER TABLE items ADD COLUMN b TEXT; ALTER TABLE items ADD COLUMN c TEXT").execute(&f.pool).await.unwrap();
    let mut draft=segmented_recipe(400);draft.fields[1].mode=Mode::Constant;draft.fields[1].value="x".repeat(12000);
    for name in ["a","b","c"]{draft.fields.push(Rule{name:name.into(),mode:Mode::Constant,value:"y".repeat(12000)});}
    let p=preview(&f,draft).await;let first_bytes=recipe_bound_bytes(&p.rows[0]);let per_segment=(RECIPE_SEGMENT_BYTES/first_bytes) as u64;
    assert!(per_segment<1000);let(task,_)=payload(p.recipe,false);let r=record(&task,0);let s=run(&f,task,&r).await;
    assert_eq!(s.status.state,ExecutionState::Succeeded,"{:?}",s.outcome);assert_eq!(s.metrics.committed,400);assert_eq!(f.count("items").await,400);
    assert_eq!(s.logs.iter().filter(|l|l.phase=="commit"&&l.level=="success").count(),3);f.cleanup().await;
}
#[tokio::test]
async fn recipe_segmented_unknown_commit_state_simulation_does_not_rollback_or_erase_prefix() {
    // Explicit state simulation, NOT a claim of native transport fault injection.
    let f=Fixture::new().await;let(task,_)=payload(segmented_recipe(2501),false);let r=record(&task,0);r.entry.start();
    let mut conn=f.pool.acquire().await.unwrap();sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await.unwrap();
    {let mut p=r.progress.lock();p.committed_prefix=Some((1000,1000));p.affected=1500;}r.transaction("active");
    let unknown=AppError::coded("EXECUTION_EFFECTS_UNKNOWN","simulated missing COMMIT receipt");
    let(result,confirmed)=generated_cleanup_sqlite(&mut conn,&r,(Err(unknown.into()),false)).await;assert!(!confirmed);
    let active={let mut handle=conn.lock_handle().await.unwrap();unsafe{libsqlite3_sys::sqlite3_get_autocommit(handle.as_raw_handle().as_ptr())==0}};assert!(active,"unknown must not replay COMMIT or issue a claimed rollback");
    r.publish(result.map_err(|e|safe_error(&e)),confirmed);let s=r.snapshot(0);assert_eq!(s.status.state,ExecutionState::Unknown);assert_eq!(s.transaction_state,"unknown");assert_eq!((s.metrics.committed,s.affected_rows),(1000,1000));assert_eq!(s.outcome.unwrap().effects,"possibly_applied");
    sqlx::query("ROLLBACK").execute(&mut *conn).await.unwrap();conn.close().await.unwrap();f.cleanup().await;
}
#[test]
fn recipe_segmented_pg_mysql_rejected_before_native_sql() {
    for kind in ["postgresql","mysql"] {
        let cfg=ConnectionConfig{id:"isolated".into(),name:"test".into(),kind:DbKind::of(kind),host:String::new(),port:0,user:String::new(),password:String::new(),database:String::new(),file:String::new(),params:String::new(),group:String::new(),read_only:false,keep_alive:false};
        let mut recipe=segmented_recipe(2501);recipe.schema_fingerprint="a".repeat(64);
        let request=BatchExecutionRequest{connection_id:"isolated".into(),namespace:"main".into(),owner_id:"a".into(),timeout_ms:None,task:BatchTask::GeneratedRecipe{recipe}};
        assert_eq!(require_request(&request,&cfg).unwrap_err().code,"RECIPE_SEGMENTED_UNSUPPORTED");
    }
}

#[tokio::test]
async fn recipe_sqlite_desc_primary_key_is_not_database_generated_rowid() {
    let f=Fixture::new().await;
    sqlx::raw_sql("CREATE TABLE descending(id INTEGER PRIMARY KEY DESC,text TEXT)").execute(&f.pool).await.unwrap();
    let mut draft=recipe(3);draft.table="descending".into();draft.fields[0].mode=Mode::Omit;draft.fields[0].value.clear();
    for preview_mode in [true,false] {
        let mut candidate=draft.clone();if !preview_mode {candidate.schema_fingerprint="a".repeat(64);}
        let(task,sample)=payload(candidate,preview_mode);let r=record(&task,0);let s=run(&f,task,&r).await;
        assert_eq!(s.status.state,ExecutionState::Failed,"inline DESC primary key cannot generate id: {:?}",s.outcome);
        assert_eq!(s.transaction_state,"rolled_back");assert_eq!(s.metrics.sent,0);assert_eq!(s.metrics.committed,0);
        assert!(sample.lock().is_none());assert_eq!(f.count("descending").await,0);
        assert!(s.outcome.unwrap().error.unwrap().message.contains("缺少非空/主键字段 id"));
    }
    // Ordinary INTEGER PRIMARY KEY still accepts omission and creates real ids.
    draft.table="items".into();let p=preview(&f,draft).await;
    assert!(p.database_decided.contains(&"id".into()));
    let(task,_)=payload(p.recipe,false);let r=record(&task,0);let s=run(&f,task,&r).await;
    assert_eq!(s.status.state,ExecutionState::Succeeded,"{:?}",s.outcome);assert_eq!(s.metrics.committed,3);
    let ids:Vec<i64>=sqlx::query_scalar("SELECT id FROM items ORDER BY id").fetch_all(&f.pool).await.unwrap();
    assert_eq!(ids,vec![1,2,3]);f.cleanup().await;
}

#[tokio::test]#[ignore="actual pinned SQLite 100k benchmark"]async fn recipe_benchmark_100k(){benchmark(100000).await;}
#[tokio::test]#[ignore="actual pinned SQLite million benchmark"]async fn recipe_benchmark_million(){benchmark(1000000).await;}
#[tokio::test]#[ignore="explicit ten-million-row SQLite capacity/RSS probe; not a speedup claim"]async fn recipe_benchmark_ten_million(){benchmark(10000000).await;}
