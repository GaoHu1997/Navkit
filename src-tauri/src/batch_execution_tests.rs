//! 所有用例只打开 UUID 独立临时目录/SQLite；不访问保存连接或用户数据。
use super::*;
use crate::execution::{Binding, Registry};
use crate::execution_contracts::ExecutionState;
use std::path::PathBuf;

struct Fixture { root:PathBuf, pool:sqlx::SqlitePool, db:Arc<Db> }
impl Fixture {
    async fn new()->Self {
        Self::with_connections(2).await
    }
    async fn with_connections(max_connections:u32)->Self {
        let root=std::env::temp_dir().join(format!("navkit-typed-batch-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let options=sqlx::sqlite::SqliteConnectOptions::new().filename(root.join("isolated.sqlite")).create_if_missing(true).journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
        let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(max_connections).connect_with(options).await.unwrap();
        sqlx::query("CREATE TABLE generated(id INTEGER PRIMARY KEY AUTOINCREMENT,name TEXT NOT NULL UNIQUE,amount REAL NOT NULL CHECK(amount>=0),active INTEGER NOT NULL,payload TEXT NOT NULL,twice REAL GENERATED ALWAYS AS(amount*2)); CREATE TABLE items(id INTEGER PRIMARY KEY,text TEXT)").execute(&pool).await.unwrap();
        Self { root,pool:pool.clone(),db:Arc::new(Db::Sqlite(pool)) }
    }
    fn rows(&self)->GeneratedRowsRequest {
        GeneratedRowsRequest{connection_id:"isolated".into(),schema:"main".into(),table:"generated".into(),columns:["name","amount","active","payload"].map(String::from).to_vec(),rows:(1..=25).map(|i|vec![Some(format!("sensitive_generated_{i}")),Some(format!("{i}.25")),Some((i%2).to_string()),Some(format!("{{\"i\":{i}}}"))]).collect()}
    }
    fn file(&self,name:&str,sql:&str)->Payload {
        let path=self.root.join(name);std::fs::write(&path,sql).unwrap();
        Payload::File{source:SourceFile::open(path.to_str().unwrap()).unwrap(),encoding:"utf-8".into(),stop_on_error:false}
    }
    async fn count(&self,table:&str)->i64 { sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}")).fetch_one(&self.pool).await.unwrap() }
    async fn cleanup(self) {
        let Self { root, pool, db } = self;
        drop(db);
        // 先收回全部物理会话并等待 SQLite worker 关闭回执；不把池计数归零
        // 当作 Windows 原生文件句柄已经释放的证明。
        let mut physical=Vec::new();
        for _ in 0..pool.size() { physical.push(pool.acquire().await.unwrap()); }
        for conn in physical { conn.close().await.unwrap(); }
        pool.close().await;
        drop(pool);
        // root 由 with_connections 创建为 UUID 独立临时目录；只对 Windows
        // sharing/lock violation 最多尝试 20 次（共等待不超过 950ms）。
        for attempt in 1..=20 {
            match std::fs::remove_dir_all(&root) {
                Ok(()) => return,
                Err(error) if cfg!(windows) && matches!(error.raw_os_error(), Some(32 | 33)) && attempt < 20 => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => panic!("failed to remove isolated batch fixture {} after {attempt} attempt(s): {error}", root.display()),
            }
        }
    }
}
fn record(payload:&Payload,timeout_ms:u64)->Arc<BatchRecord> {
    let registry=Registry::default();
    let binding=Binding{principal:"batch-test".into(),owner:uuid::Uuid::new_v4().to_string(),connection:"isolated".into(),namespace:"main".into(),fingerprint:"independent-temporary-native-sqlite".into()};
    let task=match payload{Payload::Generated(rows)=>BatchTask::GeneratedRows{request:rows.clone()},Payload::File{source,encoding,stop_on_error}=>BatchTask::SqlFile{path:source.identity().canonical_path.clone(),encoding:encoding.clone(),stop_on_error:*stop_on_error}};
    let request=BatchExecutionRequest{connection_id:binding.connection.clone(),namespace:binding.namespace.clone(),owner_id:binding.owner.clone(),timeout_ms:Some(timeout_ms),task};
    let scope=scope(&request,payload).unwrap();
    let grant=registry.grant_typed_scope(binding.clone(),&scope).unwrap();
    let entry=registry.register_typed_scope(&grant.execution_id,&grant.token,binding.clone(),&scope,Some(timeout_ms),30).unwrap();
    let (kind,total_rows,source_bytes)=match payload{Payload::Generated(rows)=>(BatchTaskKind::GeneratedRows,Some(rows.rows.len() as u64),None),Payload::File{source,..}=>(BatchTaskKind::SqlFile,None,Some(source.identity().bytes))};
    Arc::new(BatchRecord{ticket:BatchExecutionTicket{context:BatchContext{execution_id:entry.id.clone(),connection_id:binding.connection,namespace:binding.namespace,owner_id:binding.owner},task_kind:kind,execution_grant:grant.token,expires_in_ms:grant.expires_in_ms,summary:"独立临时测试".into(),total_rows,source_bytes},entry,payload:Mutex::new(None),ordinary:Mutex::new(None),started:AtomicBool::new(true),wake:tokio::sync::Notify::new(),registered:Instant::now(),progress:Mutex::new(Progress{scanned:0,executed:0,errors:0,scan_complete:false,parse_error:None,affected:0,transaction:"none".into(),logs:VecDeque::new(),bytes:0,next:0,dropped:0,outcome:None})})
}
async fn run(f:&Fixture,payload:Payload,record:&Arc<BatchRecord>)->BatchExecutionSnapshot {
    record.entry.start();
    let(result,confirmed)=run_pinned(f.db.clone(),DbKind::of("sqlite"),record.clone(),payload).await;
    record.publish(result.map_err(|e|safe_error(&e)),confirmed);record.snapshot(0)
}
#[tokio::test]
async fn batch_generated_real_commit_count_binding_and_log_redaction(){
    let f=Fixture::new().await;let payload=Payload::Generated(f.rows());let r=record(&payload,0);let result=run(&f,payload,&r).await;
    assert_eq!(result.status.state,ExecutionState::Succeeded,"{:?}",result.outcome.as_ref().unwrap().error);
    assert_eq!(result.transaction_state,"committed");assert_eq!((result.metrics.scanned,result.metrics.executed,result.metrics.errors,result.affected_rows),(25,25,0,25));
    let(count,keys,sum,generated):(i64,i64,f64,f64)=sqlx::query_as("SELECT COUNT(*),COUNT(DISTINCT id),SUM(amount),SUM(twice) FROM generated").fetch_one(&f.pool).await.unwrap();
    assert_eq!((count,keys),(25,25));assert!((sum-331.25).abs()<0.00001);assert!((generated-662.5).abs()<0.00001);
    let logs=serde_json::to_string(&result.logs).unwrap();assert!(logs.contains("INSERT INTO"));assert!(logs.contains('?'));assert!(!logs.contains("sensitive_generated_"));assert!(!logs.contains("331.25"));
    println!("typed batch real SQLite rows={count} keys={keys} sum={sum} generated_sum={generated} transaction={}",result.transaction_state);
    f.cleanup().await;
}
#[tokio::test]
async fn batch_generated_constraint_failure_rolls_back_entire_batch(){
    let f=Fixture::new().await;let mut rows=f.rows();rows.rows[5][1]=Some("-1".into());let payload=Payload::Generated(rows);let r=record(&payload,0);let result=run(&f,payload,&r).await;
    assert_eq!(result.status.state,ExecutionState::Failed);assert_eq!(result.transaction_state,"rolled_back");assert_eq!(result.outcome.unwrap().effects,"no_effect");assert_eq!((result.metrics.executed,result.metrics.errors,result.affected_rows),(6,1,0));assert_eq!(f.count("generated").await,0);
    f.cleanup().await;
}
#[tokio::test]
async fn batch_generated_true_vm_cancel_and_auto_rollback_are_confirmed(){
    let f=Fixture::new().await;
    sqlx::query("CREATE TRIGGER slow BEFORE INSERT ON generated BEGIN SELECT (WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n); END").execute(&f.pool).await.unwrap();
    let payload=Payload::Generated(f.rows());let r=record(&payload,0);r.entry.start();let db=f.db.clone();let working=r.clone();
    let task=tokio::spawn(send_work(Box::pin(run_pinned(db,DbKind::of("sqlite"),working,payload))));
    tokio::time::timeout(Duration::from_secs(5),async { while r.snapshot(0).metrics.scanned!=25 { tokio::time::sleep(Duration::from_millis(1)).await; } }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(40)).await;assert!(r.entry.request_stop(false).accepted);
    let(result,confirmed)=tokio::time::timeout(Duration::from_secs(10),task).await.unwrap().unwrap();assert!(confirmed,"{:?}",result);
    r.publish(result.map_err(|e|safe_error(&e)),confirmed);let result=r.snapshot(0);
    assert_eq!(result.status.state,ExecutionState::Cancelled,"{:?}",result.outcome.as_ref().unwrap().error);assert_eq!(result.transaction_state,"rolled_back");assert!(!result.status.resources_pending);assert_eq!((result.metrics.executed,result.metrics.errors),(1,1));assert_eq!(f.count("generated").await,0);
    f.cleanup().await;
}
#[tokio::test]
async fn batch_file_same_physical_session_and_malformed_tail_keep_prefix(){
    let f=Fixture::new().await;let payload=f.file("stream.sql","CREATE TEMP TABLE local_counter(v INTEGER); INSERT INTO local_counter VALUES(7); INSERT INTO main.items SELECT v,'prefix-secret' FROM local_counter; SELECT 'unterminated");
    let r=record(&payload,0);let result=run(&f,payload,&r).await;
    assert_eq!(result.status.state,ExecutionState::Failed);assert_eq!((result.metrics.scanned,result.metrics.executed,result.metrics.errors),(3,3,0));assert!(!result.scan_complete);assert!(result.parse_error.is_some());
    assert_eq!(f.count("items").await,1);assert_eq!(sqlx::query_scalar::<_,i64>("SELECT id FROM items").fetch_one(&f.pool).await.unwrap(),7);
    assert!(!serde_json::to_string(&result.logs).unwrap().contains("prefix-secret"));
    f.cleanup().await;
}
#[tokio::test]
async fn batch_file_execution_error_is_not_parse_error_and_continues_once(){
    let f=Fixture::new().await;let payload=f.file("errors.sql","INSERT INTO main.items VALUES(1,'one'); INSERT INTO main.items VALUES(1,'duplicate'); INSERT INTO main.items VALUES(2,'two');");let r=record(&payload,0);let result=run(&f,payload,&r).await;
    assert_eq!(result.status.state,ExecutionState::Failed);assert_eq!((result.metrics.scanned,result.metrics.executed,result.metrics.errors),(3,3,1));assert!(result.scan_complete);assert!(result.parse_error.is_none());assert_eq!(result.affected_rows,2);assert_eq!(f.count("items").await,2);
    f.cleanup().await;
}
#[tokio::test]
async fn batch_file_true_cancel_preserves_committed_prefix_without_replay(){
    let f=Fixture::new().await;
    let mut sql="INSERT INTO main.items VALUES(1,'prefix'); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n;".to_string();
    for id in 2..=10000 { sql.push_str(&format!("INSERT INTO main.items VALUES({id},'never');\n")); }
    assert!(sql.len()>64*1024);
    let payload=f.file("cancel.sql",&sql);let r=record(&payload,0);r.entry.start();let working=r.clone();let db=f.db.clone();
    let task=tokio::spawn(send_work(Box::pin(run_pinned(db,DbKind::of("sqlite"),working,payload))));
    tokio::time::timeout(Duration::from_secs(5),async { while r.snapshot(0).metrics.executed<1 { tokio::time::sleep(Duration::from_millis(1)).await; } }).await.unwrap();
    let prefix=r.snapshot(0);assert!(!prefix.scan_complete);assert!(prefix.metrics.scanned<10001);
    tokio::time::sleep(Duration::from_millis(50)).await;assert!(r.entry.request_stop(false).accepted);
    let(result,confirmed)=tokio::time::timeout(Duration::from_secs(10),task).await.unwrap().unwrap();assert!(confirmed);r.publish(result.map_err(|e|safe_error(&e)),confirmed);let result=r.snapshot(0);
    assert_eq!(result.status.state,ExecutionState::Cancelled);assert_eq!(result.transaction_state,"none");assert_eq!(f.count("items").await,1);assert_eq!(result.metrics.executed,2);assert_eq!(result.metrics.errors,1);assert_eq!(result.outcome.unwrap().effects,"possibly_applied");
    f.cleanup().await;
}
#[tokio::test]
async fn batch_file_monotonic_timeout_interrupts_real_vm(){
    let f=Fixture::new().await;let payload=f.file("timeout.sql","WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n;");let r=record(&payload,25);let result=run(&f,payload,&r).await;
    assert_eq!(result.status.state,ExecutionState::TimedOut,"{:?}",result.outcome.as_ref().unwrap().error);assert!(result.status.deadline.deadline_reached);assert!(result.status.deadline.stop_confirmed);assert!(!result.status.resources_pending);f.cleanup().await;
}
#[tokio::test]
async fn batch_typed_scope_binds_kind_values_owner_target_and_rejects_replay(){
    let f=Fixture::new().await;let rows=f.rows();let payload=Payload::Generated(rows.clone());let mut request=BatchExecutionRequest{connection_id:"isolated".into(),namespace:"main".into(),owner_id:"owner-a".into(),timeout_ms:Some(0),task:BatchTask::GeneratedRows{request:rows.clone()}};
    let original=scope(&request,&payload).unwrap();let mut changed=rows;changed.rows[0][0]=Some("changed-value".into());assert_ne!(original,scope(&request,&Payload::Generated(changed)).unwrap());request.owner_id="owner-b".into();assert_ne!(original,scope(&request,&payload).unwrap());request.owner_id="owner-a".into();request.connection_id="other-real-target".into();assert_ne!(original,scope(&request,&payload).unwrap());
    let registry=Registry::default();let binding=Binding{principal:"window-a".into(),owner:"owner-a".into(),connection:"isolated".into(),namespace:"main".into(),fingerprint:f.root.to_string_lossy().into()};let grant=registry.grant_typed_scope(binding.clone(),&original).unwrap();registry.register_typed_scope(&grant.execution_id,&grant.token,binding.clone(),&original,Some(0),30).unwrap();assert!(registry.register_typed_scope(&grant.execution_id,&grant.token,binding.clone(),&original,Some(0),30).is_err());let mut other=binding;other.owner="owner-b".into();assert!(registry.authorized(&grant.execution_id,&other).is_err());f.cleanup().await;
}
#[tokio::test]
async fn batch_log_ring_is_bounded_and_cursor_is_explicit(){
    let f=Fixture::new().await;let payload=Payload::Generated(f.rows());let r=record(&payload,0);for i in 0..1200{r.log("info","execute",Some(i),None,&"界".repeat(800),Some(&"?".repeat(3000)));}
    let result=r.snapshot(0);assert!(result.logs.len()<=LOG_LINES);assert!(r.progress.lock().bytes<=LOG_BYTES);assert!(result.dropped_before_sequence>0);assert_eq!(result.next_sequence,1200);let cursor=result.logs.last().unwrap().sequence;assert!(r.snapshot(cursor).logs.is_empty());
    for i in 1..=1200 {file_ack(&r,i,"INSERT INTO main.items VALUES(1,'秘密-value');",Dialect::Sqlite,&Ok(1),1);}
    let result=r.snapshot(0);assert!(result.logs.len()<=LOG_LINES);assert!(r.progress.lock().bytes<=LOG_BYTES);assert_eq!(result.next_sequence,2400);assert!(result.dropped_before_sequence>=1200);assert!(result.logs.iter().all(|log|log.message.contains("SHA-256:c16805567f4a12924fb21a1e7aab76cd0048447a02f658f34df825df1ec44e05; UTF-8:48 bytes")));assert!(!serde_json::to_string(&result.logs).unwrap().contains("秘密-value"));f.cleanup().await;
}
#[tokio::test]
async fn batch_unknown_keeps_owned_lease_until_late_drain_and_never_republishes(){
    struct Lease(Arc<std::sync::atomic::AtomicUsize>);impl Drop for Lease{fn drop(&mut self){self.0.fetch_add(1,Ordering::SeqCst);}}
    let f=Fixture::new().await;let payload=Payload::Generated(f.rows());let r=record(&payload,0);let released=Arc::new(std::sync::atomic::AtomicUsize::new(0));*r.ordinary.lock()=Some(Box::new(Lease(released.clone())));r.entry.start();r.entry.request_stop(false);
    let working=r.clone();let(result,confirmed)=execution::supervise_with_grace(r.entry.clone(),async move{tokio::time::sleep(Duration::from_millis(180)).await;working.ordinary.lock().take();(Ok(working.result()),false)},Duration::from_millis(10)).await;r.publish(result,confirmed);
    assert_eq!(r.snapshot(0).status.state,ExecutionState::Unknown);assert!(r.snapshot(0).status.resources_pending);assert_eq!(released.load(Ordering::SeqCst),0);
    tokio::time::sleep(Duration::from_millis(220)).await;assert_eq!(released.load(Ordering::SeqCst),1);assert_eq!(r.snapshot(0).status.state,ExecutionState::Unknown);assert!(!r.snapshot(0).status.resources_pending);assert_eq!(r.snapshot(0).outcome.unwrap().state,ExecutionState::Unknown);f.cleanup().await;
}

#[tokio::test]
async fn batch_missing_receipts_do_not_count_but_real_database_errors_do() {
    let f=Fixture::new().await;
    let payload=Payload::Generated(f.rows());let r=record(&payload,0);
    let missing=Err(sqlx::Error::Io(std::io::Error::new(std::io::ErrorKind::UnexpectedEof,"simulated missing receipt")));
    generated_row(&r,1,"INSERT INTO main.items VALUES(?)",&missing,1);
    file_ack(&r,1,"INSERT INTO main.items VALUES(1,'secret-transport')",Dialect::Sqlite,&missing,1);
    assert_eq!((r.snapshot(0).metrics.executed,r.snapshot(0).metrics.errors),(0,0));
    sqlx::query("INSERT INTO main.items VALUES(1,'one')").execute(&f.pool).await.unwrap();
    let rejected=sqlx::query("INSERT INTO main.items VALUES(1,'duplicate')").execute(&f.pool).await.map(|q|q.rows_affected());
    assert!(rejected.as_ref().unwrap_err().as_database_error().is_some());
    file_ack(&r,2,"INSERT INTO main.items VALUES(1,'duplicate')",Dialect::Sqlite,&rejected,1);
    let result=r.snapshot(0);assert_eq!((result.metrics.executed,result.metrics.errors),(1,1));
    let logs=serde_json::to_string(&result.logs).unwrap();assert!(logs.contains("缺少完整数据库回执"));assert!(!logs.contains("secret-transport"));assert!(!logs.contains("duplicate"));assert!(logs.contains("UNIQUE constraint failed"));assert!(logs.contains("INSERT"));
    for log in &result.logs {assert!(log.sql_template.as_ref().is_none_or(|template|template.len()<=2048));}
    f.cleanup().await;
}

#[tokio::test]
async fn batch_file_stop_on_database_error_never_sends_tail() {
    let f=Fixture::new().await;
    let mut payload=f.file("stop-on-error.sql","INSERT INTO main.items VALUES(1,'prefix'); INSERT INTO main.items VALUES(1,'duplicate'); INSERT INTO main.items VALUES(2,'never');");
    if let Payload::File{stop_on_error,..}=&mut payload { *stop_on_error=true; }
    let r=record(&payload,0);let result=run(&f,payload,&r).await;
    assert_eq!(result.status.state,ExecutionState::Failed);
    assert_eq!((result.metrics.executed,result.metrics.errors,result.affected_rows),(2,1,1));
    assert_eq!(f.count("items").await,1);
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT id FROM items").fetch_one(&f.pool).await.unwrap(),1);
    let audit:Vec<_>=result.logs.iter().filter(|log|log.phase=="execute").collect();assert_eq!(audit.len(),2);
    assert!(audit[0].message.contains("SHA-256:b3838651ce153a0c7468c260e53e9257e62c67d6918dabc43eb504a0acc78780; UTF-8:42 bytes"));
    assert!(audit[1].message.contains("SHA-256:dd16a85e22cedc564283284d668b77d5031ebc6e53361a3d0f7fb2d641adcfcf; UTF-8:46 bytes"));
    let logs=serde_json::to_string(&result.logs).unwrap();assert!(!logs.contains("4d6c9b1932d937350e67d8aef3a8516c0b2897e1d8c386a21346535a6978da0b"));for secret in ["prefix","duplicate","never"] {assert!(!logs.contains(secret));}
    f.cleanup().await;
}

#[tokio::test]
async fn batch_generated_main_target_cannot_be_shadowed_by_temp_table() {
    let f=Fixture::with_connections(1).await;
    sqlx::query("CREATE TEMP TABLE generated(name TEXT,amount REAL,active INTEGER,payload TEXT)").execute(&f.pool).await.unwrap();
    let payload=Payload::Generated(f.rows());let r=record(&payload,0);let result=run(&f,payload,&r).await;
    assert_eq!(result.status.state,ExecutionState::Succeeded);
    assert_eq!(f.count("main.generated").await,25);
    // run_pinned 销毁执行物理连接，其 temp 对象也随之消失。
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM sqlite_temp_master WHERE name='generated'").fetch_one(&f.pool).await.unwrap(),0);
    f.cleanup().await;
}

#[tokio::test]
async fn batch_generated_metadata_rejects_generated_missing_null_and_duplicate_keys() {
    let f=Fixture::new().await;
    let mut generated=f.rows();generated.columns.push("twice".into());for row in &mut generated.rows {row.push(Some("1".into()));}
    let mut required=f.rows();required.columns.remove(0);for row in &mut required.rows {row.remove(0);}
    let mut null=f.rows();null.rows[0][0]=None;
    let mut duplicate=f.rows();duplicate.columns.insert(0,"id".into());for row in &mut duplicate.rows {row.insert(0,Some("1".into()));}
    for rows in [generated,required,null,duplicate] {
        let payload=Payload::Generated(rows);let r=record(&payload,0);let result=run(&f,payload,&r).await;
        assert_eq!(result.status.state,ExecutionState::Failed);
        assert_eq!(result.transaction_state,"rolled_back");
        assert_eq!((result.metrics.executed,result.metrics.errors),(0,0));
        assert_eq!(f.count("generated").await,0);
    }
    f.cleanup().await;
}

#[tokio::test]
async fn batch_legacy_data_generator_file_fixture_preserves_real_commit_and_binding() {
    let f=Fixture::new().await;let kind=DbKind::of("sqlite");let rows=f.rows();
    assert_eq!(data_generator::insert_batch(f.db.as_ref(),&kind,&rows).await.unwrap(),25);
    let(count,keys,sum):(i64,i64,f64)=sqlx::query_as("SELECT COUNT(*),COUNT(DISTINCT id),SUM(amount) FROM main.generated").fetch_one(&f.pool).await.unwrap();
    assert_eq!((count,keys),(25,25));assert!((sum-331.25).abs()<0.00001);
    let mut attack=f.rows();attack.rows.truncate(1);attack.rows[0][0]=Some("x'); DROP TABLE generated; --".into());
    assert_eq!(data_generator::insert_batch(f.db.as_ref(),&kind,&attack).await.unwrap(),1);
    assert_eq!(f.count("generated").await,26);
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM main.generated WHERE name=?").bind(attack.rows[0][0].as_deref()).fetch_one(&f.pool).await.unwrap(),1);
    attack.columns[0]="name\") VALUES ('x'); DROP TABLE generated; --".into();
    assert!(data_generator::insert_batch(f.db.as_ref(),&kind,&attack).await.is_err());
    assert_eq!(f.count("generated").await,26);f.cleanup().await;
}

#[tokio::test]
async fn batch_legacy_data_generator_file_fixture_preserves_constraint_rollback_and_limits() {
    let f=Fixture::new().await;let kind=DbKind::of("sqlite");let mut rows=f.rows();rows.rows[5][1]=Some("-1".into());
    let error=data_generator::insert_batch(f.db.as_ref(),&kind,&rows).await.unwrap_err();
    assert!(error.to_string().contains("第 6 行"));assert_eq!(f.count("generated").await,0);
    let mut null=f.rows();null.rows[0][0]=None;
    let mut missing=f.rows();missing.columns.remove(0);for row in &mut missing.rows {row.remove(0);}
    let mut too_many=f.rows();too_many.rows=vec![too_many.rows[0].clone();data_generator::MAX_ROWS+1];
    for invalid in [null,missing,too_many] {
        assert!(data_generator::insert_batch(f.db.as_ref(),&kind,&invalid).await.is_err());
        assert_eq!(f.count("generated").await,0);
    }
    f.cleanup().await;
}

#[tokio::test]
async fn batch_file_audit_hashes_decoded_utf8_statement_not_gbk_source_bytes(){
    let f=Fixture::new().await;let sql="INSERT INTO main.items VALUES(1,'秘密-value');";
    let(encoded,_,unmappable)=encoding_rs::GBK.encode(sql);assert!(!unmappable);let path=f.root.join("digest-gbk.sql");std::fs::write(&path,encoded.as_ref()).unwrap();
    let source_hash=format!("{:x}",Sha256::digest(encoded.as_ref()));assert_ne!(source_hash,"c16805567f4a12924fb21a1e7aab76cd0048447a02f658f34df825df1ec44e05");
    let payload=Payload::File{source:SourceFile::open(path.to_str().unwrap()).unwrap(),encoding:"gbk".into(),stop_on_error:true};let r=record(&payload,0);let result=run(&f,payload,&r).await;
    assert_eq!(result.status.state,ExecutionState::Succeeded);assert_eq!((result.metrics.scanned,result.metrics.executed,result.metrics.errors),(1,1,0));assert_eq!(sqlx::query_scalar::<_,String>("SELECT text FROM items WHERE id=1").fetch_one(&f.pool).await.unwrap(),"秘密-value");
    let audit:Vec<_>=result.logs.iter().filter(|log|log.phase=="execute").collect();assert_eq!(audit.len(),1);assert_eq!(audit[0].statement,Some(1));
    assert_eq!(audit[0].message,"SQL 执行并排空结果收到数据库回执；解码后语句 SHA-256:c16805567f4a12924fb21a1e7aab76cd0048447a02f658f34df825df1ec44e05; UTF-8:48 bytes");
    let logs=serde_json::to_string(&result.logs).unwrap();assert!(!logs.contains("秘密-value"));assert!(!logs.contains(&source_hash));
    let template=audit[0].sql_template.as_ref().unwrap();
    assert_eq!(template.split_whitespace().collect::<String>(),"INSERTINTOmain.itemsVALUES(?,?);");
    assert!(template.contains('?'));f.cleanup().await;
}

#[tokio::test]
async fn batch_file_audit_unknown_contains_attempt_prefix_without_unsent_tail(){
    let f=Fixture::new().await;let payload=f.file("unknown-audit.sql","INSERT INTO items VALUES(3,'unsent-secret');");let r=record(&payload,0);drop(payload);r.entry.start();
    file_ack(&r,1,"INSERT INTO main.items VALUES(1,'prefix');",Dialect::Sqlite,&Ok(1),1);
    let missing=Err(sqlx::Error::Io(std::io::Error::new(std::io::ErrorKind::UnexpectedEof,"simulated secret server error")));
    file_ack(&r,2,"INSERT INTO main.items VALUES(1,'secret-transport')",Dialect::Sqlite,&missing,1);
    r.publish(Err(AppError::coded("EXECUTION_EFFECTS_UNKNOWN","完整回执缺失；停止尾部且禁止重放")),false);let result=r.snapshot(0);
    assert_eq!(result.status.state,ExecutionState::Unknown);assert_eq!((result.metrics.executed,result.metrics.errors),(1,0));assert!(!result.scan_complete);
    let audit:Vec<_>=result.logs.iter().filter(|log|log.phase=="execute").collect();assert_eq!(audit.len(),2);assert_eq!((audit[0].statement,audit[1].statement),(Some(1),Some(2)));
    assert!(audit[0].message.contains("SHA-256:b3838651ce153a0c7468c260e53e9257e62c67d6918dabc43eb504a0acc78780; UTF-8:42 bytes"));
    assert!(audit[1].message.contains("缺少完整数据库回执"));assert!(audit[1].message.contains("SHA-256:7b91ad4b5af51737b9acaa59ed0b99477ecd8fff95fb044763a747ad5a3e3cb3; UTF-8:51 bytes"));
    let logs=serde_json::to_string(&result.logs).unwrap();for secret in ["prefix","secret-transport","unsent-secret"] {assert!(!logs.contains(secret));}assert!(logs.contains("simulated secret server error"));assert_eq!(f.count("items").await,0);f.cleanup().await;
}

fn queued(payload:Payload,ttl:u64)->Arc<BatchRecord> {
    let mut r=record(&payload,0);let inner=Arc::get_mut(&mut r).unwrap();
    inner.ticket.expires_in_ms=ttl;inner.started.store(false,Ordering::Release);*inner.payload.lock()=Some(payload);r
}
#[tokio::test]
async fn batch_queued_cancel_before_and_after_watchdog_wait_releases_payload_without_sql(){
    let f=Fixture::new().await;
    for wait in [false,true] {for file in [false,true] {
        let payload=if file{f.file("queued.sql","INSERT INTO main.items VALUES(99,'never')")}else{Payload::Generated(f.rows())};
        let r=queued(payload,60000);
        if !wait {r.entry.request_stop(false);}
        let task=tokio::spawn(queued_watchdog(r.clone()));
        if wait {tokio::time::sleep(Duration::from_millis(10)).await;r.entry.request_stop(false);}
        tokio::time::timeout(Duration::from_secs(2),task).await.unwrap().unwrap();
        let s=r.snapshot(0);assert_eq!(s.status.state,ExecutionState::Cancelled);assert_eq!(s.outcome.unwrap().effects,"no_effect");assert!(!s.status.resources_pending);assert_eq!(s.transaction_state,"none");assert_eq!((s.metrics.scanned,s.metrics.executed,s.metrics.errors),(0,0,0));assert!(r.payload.lock().is_none());assert!(claim_payload(&r).is_err());
        if file{std::fs::write(f.root.join("queued.sql"),"source handle released").unwrap();}
    }}
    assert_eq!(f.count("items").await,0);assert_eq!(f.count("generated").await,0);f.cleanup().await;
}
#[tokio::test]
async fn batch_queued_expiry_and_run_cancel_race_have_one_finish_and_no_replay(){
    let f=Fixture::new().await;let r=queued(f.file("expired.sql","INSERT INTO items VALUES(1,'never')"),5);
    queued_watchdog(r.clone()).await;assert_eq!(r.snapshot(0).outcome.unwrap().effects,"no_effect");assert!(claim_payload(&r).is_err());assert!(r.payload.lock().is_none());
    for _ in 0..20 {
        let r=queued(Payload::Generated(f.rows()),60000);let watch=tokio::spawn(queued_watchdog(r.clone()));
        r.entry.request_stop(false);
        match claim_payload(&r) {Ok(payload)=>{assert!(r.entry.stopping());drop(payload);r.entry.work_settled();r.transaction("none");r.publish(Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND","未发送")),true);},Err(_)=>{}}
        watch.await.unwrap();let s=r.snapshot(0);assert_eq!(s.status.state,ExecutionState::Cancelled);assert_eq!(s.outcome.unwrap().effects,"no_effect");assert_eq!(s.logs.iter().filter(|l|l.phase=="finish").count(),1);assert!(claim_payload(&r).is_err());
    }
    assert_eq!(f.count("generated").await,0);assert_eq!(f.count("items").await,0);f.cleanup().await;
}
#[tokio::test]
async fn batch_dump_guards_use_fixed_sqlite_session_and_do_not_pollute_pool(){
    let f=Fixture::new().await;let original: i64=sqlx::query_scalar("PRAGMA foreign_keys").fetch_one(&f.pool).await.unwrap();assert_eq!(original,1);
    let sql="PRAGMA /* ordinary header */ foreign_keys = OFF; CREATE TABLE parent(id INTEGER PRIMARY KEY); CREATE TABLE child(pid INTEGER REFERENCES parent(id)); INSERT INTO child VALUES(999); PRAGMA foreign_keys = ON; INSERT INTO child VALUES(998); INSERT INTO parent VALUES(1);";
    let payload=f.file("dump.sql",sql);let r=record(&payload,0);let s=run(&f,payload,&r).await;
    assert_eq!((s.metrics.scanned,s.metrics.executed,s.metrics.errors),(7,7,1));assert_eq!(s.status.state,ExecutionState::Failed);assert_eq!(f.count("child").await,1);assert_eq!(f.count("parent").await,1);
    assert_eq!(sqlx::query_scalar::<_,i64>("PRAGMA foreign_keys").fetch_one(&f.pool).await.unwrap(),original);
    let logs=serde_json::to_string(&s.logs).unwrap();assert!(logs.contains("PRAGMA"));assert!(logs.contains("INSERT"));
    let audit:Vec<_>=s.logs.iter().filter(|log|log.phase=="execute").collect();assert_eq!(audit.len(),7);
    let mut reader=Reader::new(sql.as_bytes(),Dialect::Sqlite,None);
    // 检查可能泄露SQL字面量的文字字段；ts_ms/elapsed_ms本来就可能包含999，
    // 不能用偶然时间戳数字判定泄漏。摘要仍逐条按实际语句完整核验并只移除一次。
    let mut logs_without_verified_digests=s.logs.iter().map(|log|format!("{} {}",log.message,log.sql_template.as_deref().unwrap_or(""))).collect::<Vec<_>>().join("\n");
    for (index,log) in audit.iter().enumerate() {
        let statement=reader.next_statement().unwrap().unwrap();let digest=format!("{:x}",Sha256::digest(statement.sql.as_bytes()));
        assert_eq!(log.statement,Some(index as u64+1));
        assert!(log.message.ends_with(&format!("；解码后语句 SHA-256:{digest}; UTF-8:{} bytes",statement.sql.len())),"{}",log.message);
        assert!(!log.sql_template.as_ref().unwrap().contains("999"));
        // 仅排除本条完整实际语句已经核验的摘要；例如建表语句的合法摘要含 9999。
        let marker=format!("SHA-256:{digest};");assert_eq!(log.message.matches(marker.as_str()).count(),1);
        logs_without_verified_digests=logs_without_verified_digests.replacen(marker.as_str(),"SHA-256:<verified>;",1);
    }
    assert!(reader.next_statement().unwrap().is_none());assert!(!logs_without_verified_digests.contains("999"));
    let payload=f.file("complete-dump.sql","PRAGMA foreign_keys=OFF; CREATE TABLE dump_parent(id INTEGER PRIMARY KEY); CREATE TABLE dump_child(pid INTEGER REFERENCES dump_parent(id)); INSERT INTO dump_child VALUES(77); INSERT INTO dump_parent VALUES(77); PRAGMA foreign_keys=ON;");
    let r=record(&payload,0);let s=run(&f,payload,&r).await;assert_eq!(s.status.state,ExecutionState::Succeeded);assert_eq!((s.metrics.scanned,s.metrics.executed,s.metrics.errors),(6,6,0));assert_eq!(f.count("dump_child").await,1);assert_eq!(f.count("dump_parent").await,1);
    assert_eq!(sqlx::query_scalar::<_,i64>("PRAGMA foreign_keys").fetch_one(&f.pool).await.unwrap(),original);
    let payload=f.file("unsafe.sql","PRAGMA foreign_keys=OFF+0; INSERT INTO items VALUES(1,'never');");let r=record(&payload,0);let s=run(&f,payload,&r).await;assert!(s.parse_error.is_some());assert_eq!(s.metrics.executed,0);assert_eq!(f.count("items").await,0);f.cleanup().await;
}
#[tokio::test]
async fn batch_file_raw_result_receipts_are_native_and_result_rows_are_discarded() {
    let f=Fixture::new().await;
    let payload=f.file("native-raw-receipts.sql","INSERT INTO main.items VALUES(21,'receipt-secret'); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000) SELECT x FROM n; INSERT INTO main.items VALUES(22,'after-result-drain');");
    let r=record(&payload,0);let result=run(&f,payload,&r).await;
    assert_eq!(result.status.state,ExecutionState::Succeeded);
    assert_eq!((result.metrics.executed,result.metrics.errors),(3,0));
    // SQLite's native changes() is still 1 on SELECT; it is a driver receipt,
    // not a count of selected rows or a net count of writes in this file.
    assert_eq!(result.affected_rows,3);assert_eq!(f.count("items").await,2);
    let receipts:Vec<_>=result.logs.iter().filter(|l|l.phase=="execute").collect();
    assert_eq!(receipts.len(),3);assert!(receipts.iter().all(|l|l.affected_rows==Some(1)));
    assert!(!serde_json::to_string(&result.logs).unwrap().contains("receipt-secret"));
    f.cleanup().await;
}

