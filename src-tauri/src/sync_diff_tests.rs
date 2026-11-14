//! Correctness oracle is intentionally the old materializing algorithm, ONLY in tests.
//! Production previews must not call this oracle or allocate its whole-table maps.
use super::*;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
#[path = "performance_rss_probe.rs"] mod process_memory;
#[path = "sync_diff_memory_tests.rs"] mod memory_tests;
#[path = "sync_adaptive_memory_bench.rs"] mod adaptive_performance;

struct Directory(PathBuf);
impl Drop for Directory { fn drop(&mut self) { let _=std::fs::remove_dir_all(&self.0); } }
async fn fixtures()->(Directory,sqlx::SqlitePool,sqlx::SqlitePool) {
    let directory=Directory(std::env::temp_dir().join(format!("navkit-sync-diff-test-{}",uuid::Uuid::new_v4())));
    std::fs::create_dir(&directory.0).unwrap();
    let source=SqlitePoolOptions::new().max_connections(1).connect_with(SqliteConnectOptions::new().filename(directory.0.join("s.sqlite")).create_if_missing(true).journal_mode(SqliteJournalMode::Wal)).await.unwrap();
    let target=SqlitePoolOptions::new().max_connections(1).connect_with(SqliteConnectOptions::new().filename(directory.0.join("t.sqlite")).create_if_missing(true).journal_mode(SqliteJournalMode::Wal)).await.unwrap();
    (directory,source,target)
}
fn request()->SyncDiffRequest { SyncDiffRequest { source_connection:"source".into(), source_schema:"main".into(), source_table:"items".into(),target_connection:"target".into(),target_schema:"main".into(),target_table:"items".into(),where_clause:None,columns:vec![] } }
async fn run(source:&sqlx::SqlitePool,target:&sqlx::SqlitePool,req:&SyncDiffRequest)->Result<SyncDiffReport> {
    let kind=DbKind::of("sqlite");diff_bounded(&Db::Sqlite(source.clone()),&kind,&Db::Sqlite(target.clone()),&kind,req).await
}
async fn old_report(source:&Db,kind:&DbKind,target:&Db,req:&SyncDiffRequest)->Result<SyncDiffReport> {
    let sm=crate::schema::table_schema(source,kind.clone(),&req.source_schema,&req.source_table).await?;
    let mapping=if req.columns.is_empty(){sm.columns.iter().map(|c|SyncColumnMapping{source:c.name.clone(),target:c.name.clone(),target_type:None}).collect::<Vec<_>>()}else{req.columns.clone()};
    validate_key_mapping(&sm,&mapping,&[])?;
    let cols=mapping.iter().map(|m|(m,sm.columns.iter().find(|c|c.name==m.source).unwrap())).collect::<Vec<_>>();
    let (sn,sr)=transfer::fetch_transfer_rows(source,kind,&req.source_schema,&req.source_table,req.where_clause.as_deref(),None).await?;
    let mut report=SyncDiffReport{source_rows:sr.len() as u64,target_rows:0,keyed:false,missing_in_target:0,extra_in_target:0,changed:0,changed_samples:vec![],target_missing_columns:vec![],target_extra_columns:vec![],message:None,memory:None};
    let exists=crate::schema::list_tables(target,kind.clone(),&req.target_schema).await?.iter().any(|(n,_)|n==&req.target_table);
    if !exists {report.missing_in_target=report.source_rows;report.keyed=cols.iter().any(|(_,c)|c.primary_key);report.target_missing_columns=cols.iter().map(|(m,_)|m.target.clone()).collect();report.message=Some("目标表不存在：全部源行都将作为缺失行写入（结构模式或启用建表时自动创建）。".into());return Ok(report);}
    let tm=crate::schema::table_schema(target,kind.clone(),&req.target_schema,&req.target_table).await?;
    report.target_missing_columns=cols.iter().filter(|(m,_)|!tm.columns.iter().any(|c|c.name==m.target)).map(|(m,_)|m.target.clone()).collect();
    report.target_extra_columns=tm.columns.iter().filter(|c|!cols.iter().any(|(m,_)|m.target==c.name)).map(|c|c.name.clone()).collect();
    let(tn,tr)=transfer::fetch_transfer_rows(target,kind,&req.target_schema,&req.target_table,None,None).await?;
    report.target_rows=tr.len() as u64;
    let sk=cols.iter().filter(|(_,c)|c.primary_key).map(|(m,_)|m.source.clone()).collect::<Vec<_>>();
    let tk=cols.iter().filter(|(_,c)|c.primary_key).map(|(m,_)|m.target.clone()).collect::<Vec<_>>();
    if sk.is_empty(){report.message=Some("源表没有主键：仅统计行数与结构差异；缺失/多出/内容差异需要主键才能比较。".into());return Ok(report);}
    report.keyed=true;
    let s=keyed_rows(&sn,&sr,&sk,"源表")?;let t=keyed_rows(&tn,&tr,&tk,"目标表")?;
    report.missing_in_target=s.keys().filter(|k|!t.contains_key(*k)).count() as u64;
    report.extra_in_target=t.keys().filter(|k|!s.contains_key(*k)).count() as u64;
    for (key,row) in s {if let Some(target)=t.get(&key){if row_differs(&sn,row,&tn,target,&cols,Some(&tm))?{report.changed+=1;if report.changed_samples.len()<5{report.changed_samples.push(sk.iter().map(|key|row[sn.iter().position(|n|n==key).unwrap()].as_deref().unwrap_or("NULL").to_owned()).collect::<Vec<_>>().join(", "));}}}}
    Ok(report)
}
fn canonical(mut report:SyncDiffReport)->serde_json::Value {report.changed_samples.sort();report.memory=None;serde_json::to_value(report).unwrap()}
struct StableAvailable;
impl super::super::diff_memory_budget::AvailableMemory for StableAvailable { fn available(&self)->Result<u64> {Ok(4*1024*1024*1024)} }
async fn adaptive_run(source:&sqlx::SqlitePool,target:&sqlx::SqlitePool,req:&SyncDiffRequest,mode:&str,limit:Option<u64>)->Result<SyncDiffReport> {
    let controller=super::super::diff_memory_budget::Controller::new(std::sync::Arc::new(StableAvailable));
    let kind=DbKind::of("sqlite");let opts=DiffMemoryOptions{mode:mode.into(),limit_bytes:limit};
    let result=compare_with_memory(source,&kind,target,&kind,req,&opts,&controller).await;
    assert_eq!(controller.used(),0,"actual readers/scratch closed before releasing all accounted reservations");
    if let Ok(report)=&result {assert_eq!(report.memory.as_ref().unwrap().used_bytes,0);}
    result
}
async fn assert_ab(source:&sqlx::SqlitePool,target:&sqlx::SqlitePool,req:&SyncDiffRequest) {
    let kind=DbKind::of("sqlite");let expected=old_report(&Db::Sqlite(source.clone()),&kind,&Db::Sqlite(target.clone()),req).await.unwrap();
    let actual=run(source,target,req).await.unwrap();
    let compatible=diff(&Db::Sqlite(source.clone()),&kind,&Db::Sqlite(target.clone()),&kind,req).await.unwrap();
    assert_eq!(canonical(compatible),canonical(expected.clone()));
    assert_eq!(canonical(actual),canonical(expected.clone()));
    let source_file:String=crate::sql_history::query_scalar("SELECT file FROM pragma_database_list WHERE name='main'").fetch_one(source).await.unwrap();
    let target_file:String=crate::sql_history::query_scalar("SELECT file FROM pragma_database_list WHERE name='main'").fetch_one(target).await.unwrap();
    // The new pipeline deliberately rejects memory databases. This shared oracle
    // helper must still exercise old default/bounded memory compatibility unchanged.
    if !source_file.is_empty() && !target_file.is_empty() {
        for (mode,limit) in [("auto",None),("manual",Some(64*1024*1024)),("manual",Some(512*1024*1024)),("speed",None)] {
            let adaptive=adaptive_run(source,target,req,mode,limit).await.unwrap();
            assert!(adaptive.memory.is_some());assert_eq!(canonical(adaptive),canonical(expected.clone()));
        }
    }
}

#[tokio::test]
async fn sqlite_report_ab_mapping_where_decimal_null_binary_and_structure() {
    let(_dir,s,t)=fixtures().await;
    sqlx::raw_sql("CREATE TABLE items(id INTEGER PRIMARY KEY,n TEXT,b BLOB,v TEXT,gone TEXT);INSERT INTO items VALUES(1,'9007199254740993.000',X'00FF80','same',NULL),(2,'9007199254740994',X'00FF',' tail ',NULL),(3,NULL,NULL,'x',NULL),(4,'1e-20',X'AB','null',NULL),(5,'0',X'00','filtered',NULL),(6,'1',X'01','missing',NULL)").execute(&s).await.unwrap();
    sqlx::raw_sql("CREATE TABLE items(k INTEGER PRIMARY KEY,n DECIMAL,b BLOB,label TEXT,extra TEXT);INSERT INTO items VALUES(1,'9007199254740993',X'00FF80','same',NULL),(2,'9007199254740995',X'00FF','tail',NULL),(3,NULL,X'','x',NULL),(4,'0.00000000000000000001',X'AB',NULL,NULL),(99,'0',X'00','extra',NULL)").execute(&t).await.unwrap();
    let mut req=request();req.where_clause=Some("id <> 5".into());
    req.columns=[("id","k"),("n","n"),("b","b"),("v","label"),("gone","gone")].into_iter().map(|(s,t)|SyncColumnMapping{source:s.into(),target:t.into(),target_type:None}).collect();
    assert_ab(&s,&t,&req).await;let report=run(&s,&t,&req).await.unwrap();assert_eq!((report.source_rows,report.target_rows,report.missing_in_target,report.extra_in_target,report.changed),(5,5,1,1,3));
    s.close().await;t.close().await;
}

#[tokio::test]
async fn sqlite_no_key_and_missing_target_count_only_even_oversized_values() {
    for primary in [false,true] {
        let(_dir,s,t)=fixtures().await;
        let ddl=format!("CREATE TABLE items(id INTEGER{},payload BLOB);INSERT INTO items VALUES(1,zeroblob({})),(2,zeroblob({}))",if primary{" PRIMARY KEY"}else{""},ROW_BYTES+1,ROW_BYTES+1);
        sqlx::raw_sql(&ddl).execute(&s).await.unwrap();
        if !primary{sqlx::raw_sql("CREATE TABLE items(id INTEGER,payload BLOB);INSERT INTO items VALUES(9,zeroblob(9000000))").execute(&t).await.unwrap();}
        let mut req=request();req.where_clause=Some("id=2".into());let report=run(&s,&t,&req).await.unwrap();
        assert_eq!(report.source_rows,1);assert_eq!(report.keyed,primary);assert_eq!(report.missing_in_target,if primary{1}else{0});assert_eq!(report.target_rows,if primary{0}else{1});
        let adaptive=adaptive_run(&s,&t,&req,"manual",Some(64*1024*1024)).await.unwrap();assert_eq!(adaptive.memory.as_ref().unwrap().backend,"count_only");assert_eq!(canonical(adaptive),canonical(report));
        s.close().await;t.close().await;
    }
}

#[tokio::test]
async fn sqlite_report_ab_counts_and_missing_target() {
    let(_dir,s,t)=fixtures().await;
    sqlx::raw_sql("CREATE TABLE items(id INTEGER,v TEXT);INSERT INTO items VALUES(1,NULL),(2,'x');").execute(&s).await.unwrap();
    assert_ab(&s,&t,&request()).await;
    sqlx::raw_sql("CREATE TABLE items(id INTEGER,extra TEXT);INSERT INTO items VALUES(3,'z')").execute(&t).await.unwrap();
    assert_ab(&s,&t,&request()).await;s.close().await;t.close().await;
}

#[tokio::test]
async fn sqlite_null_and_duplicate_keys_fail_like_old_algorithm() {
    for null in [false,true] {
        let(_dir,s,t)=fixtures().await;
        sqlx::raw_sql("CREATE TABLE items(id TEXT PRIMARY KEY,v TEXT);INSERT INTO items VALUES('x','one')").execute(&s).await.unwrap();
        if null{sqlx::raw_sql("INSERT INTO items VALUES(NULL,'bad')").execute(&s).await.unwrap();}
        sqlx::raw_sql("CREATE TABLE items(id TEXT,v TEXT);INSERT INTO items VALUES('x','one'),('x','two')").execute(&t).await.unwrap();
        let kind=DbKind::of("sqlite");assert!(old_report(&Db::Sqlite(s.clone()),&kind,&Db::Sqlite(t.clone()),&request()).await.is_err());assert!(run(&s,&t,&request()).await.is_err());
        assert!(adaptive_run(&s,&t,&request(),"manual",Some(512*1024*1024)).await.is_err());
        s.close().await;t.close().await;
    }
}

#[tokio::test]
async fn sqlite_keys_keep_binary_json_identity_not_server_collation_or_numeric_merge() {
    let(_dir,s,t)=fixtures().await;
    sqlx::raw_sql("CREATE TABLE items(id TEXT COLLATE BINARY PRIMARY KEY,v TEXT);INSERT INTO items VALUES('a','x'),('A','y'),('01','z'),('1','q')").execute(&s).await.unwrap();
    sqlx::raw_sql("CREATE TABLE items(id TEXT COLLATE NOCASE,v TEXT);INSERT INTO items VALUES('a','x'),('A','y'),('1','q')").execute(&t).await.unwrap();
    assert_ab(&s,&t,&request()).await;assert_eq!(run(&s,&t,&request()).await.unwrap().missing_in_target,1);s.close().await;t.close().await;
}

#[tokio::test]
async fn sqlite_invalid_utf8_row_limit_and_numeric_expansion_fail_not_equal() {
    for sql in ["INSERT INTO items VALUES(1,CAST(X'FF' AS TEXT))".to_owned(),format!("INSERT INTO items VALUES(1,zeroblob({}))",ROW_BYTES+1),"INSERT INTO items VALUES(1,'1e999999999')".into()] {
        let(_dir,s,t)=fixtures().await;sqlx::raw_sql("CREATE TABLE items(id INTEGER PRIMARY KEY,v NUMERIC)").execute(&s).await.unwrap();
        sqlx::raw_sql(&sql).execute(&s).await.unwrap();sqlx::raw_sql("CREATE TABLE items(id INTEGER PRIMARY KEY,v NUMERIC);INSERT INTO items VALUES(1,NULL)").execute(&t).await.unwrap();
        assert!(run(&s,&t,&request()).await.is_err());
        assert!(adaptive_run(&s,&t,&request(),"manual",Some(512*1024*1024)).await.is_err());s.close().await;t.close().await;
    }
}

#[tokio::test]
async fn sqlite_scratch_budget_duplicate_cleanup_and_index_plan() {
    let scratch=Scratch::new().unwrap();let path=scratch.path.clone();let mut index=Index::open(&scratch).await.unwrap();
    let column=ColumnInfo{name:"id".into(),data_type:"TEXT".into(),nullable:false,default:None,primary_key:true,auto_increment:false,comment:None,length:None,precision:None,scale:None};
    let meta=crate::models::TableSchema{table:"items".into(),database:"main".into(),columns:vec![column],indexes:vec![],foreign_keys:vec![],comment:None,engine:None,collation:None,row_count:None};
    let names=vec!["id".to_owned()];let row=vec![Some("x".to_owned())];
    index.push(true,&names,&row,&names,&meta).await.unwrap();assert!(index.push(true,&names,&row,&names,&meta).await.is_err());
    index.bytes=SPILL_BYTES;assert!(index.push(false,&names,&row,&names,&meta).await.is_err());
    let plan=sqlx::query("EXPLAIN QUERY PLAN SELECT s.body,t.body FROM s JOIN t ON t.k=s.k").fetch_all(&mut index.connection).await.unwrap();
    assert!(plan.iter().any(|r|r.get::<String,_>("detail").contains("INDEX")));
    index.connection.close().await.unwrap();scratch.clean().unwrap();assert!(!path.exists());
}

#[tokio::test]
async fn sqlite_single_side_snapshot_stays_stable_across_concurrent_writer() {
    let(_dir,s,t)=fixtures().await;sqlx::raw_sql("CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);INSERT INTO items VALUES(1,'old')").execute(&s).await.unwrap();
    let mut reader=sqlite_reader(&s).await.unwrap();sqlx::query("BEGIN").execute(&mut reader).await.unwrap();
    sqlite_metadata(&mut reader,"items").await.unwrap();
    sqlx::query("INSERT INTO items VALUES(2,'new')").execute(&s).await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM main.items").fetch_one(&mut reader).await.unwrap(),1);
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT v FROM main.items WHERE id=1").fetch_one(&mut reader).await.unwrap(),"old");
    reader.close().await.unwrap();s.close().await;t.close().await;
}

#[tokio::test]
async fn sqlite_changed_samples_are_capped_at_five() {
    let(_dir,s,t)=fixtures().await;
    sqlx::raw_sql("CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<20) INSERT INTO items SELECT x,'source' FROM n").execute(&s).await.unwrap();
    sqlx::raw_sql("CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<20) INSERT INTO items SELECT x,'target' FROM n").execute(&t).await.unwrap();
    let r=run(&s,&t,&request()).await.unwrap();assert_eq!(r.changed,20);assert_eq!(r.changed_samples.len(),5);s.close().await;t.close().await;
}

#[tokio::test]
async fn adaptive_declared_boolean_date_and_extreme_real_keep_old_exact_values() {
    let(_dir,s,t)=fixtures().await;
    let ddl="CREATE TABLE items(id INTEGER PRIMARY KEY,flag BOOLEAN,at DATETIME,n REAL,b BLOB);INSERT INTO items VALUES(1,true,'2026-01-01T10:20:30',1.7976931348623157e308,X'00FF80'),(2,false,'2026-01-02',4.9406564584124654e-324,X''),(3,true,NULL,-0.0,NULL)";
    sqlx::raw_sql(ddl).execute(&s).await.unwrap();sqlx::raw_sql(ddl).execute(&t).await.unwrap();assert_ab(&s,&t,&request()).await;
    s.close().await;t.close().await;
}
#[tokio::test]
async fn adaptive_boolean_integer_and_abnormal_storage_classes_match_old_oracle() {
    let(_dir,s,t)=fixtures().await;
    sqlx::raw_sql("CREATE TABLE items(id INTEGER PRIMARY KEY,flag BOOLEAN,at DATE,n TEXT);INSERT INTO items VALUES(1,true,'2026-01-01','9007199254740993.000'),(2,false,123,'1e-20'),(3,'true',X'00FF','3'),(4,2,NULL,'0.000'),(5,'001','tail ',NULL)").execute(&s).await.unwrap();
    sqlx::raw_sql("CREATE TABLE items(id INTEGER PRIMARY KEY,flag INTEGER,at DATETIME,n DECIMAL);INSERT INTO items VALUES(1,1,'2026-01-01','9007199254740993'),(2,0,123,'0.00000000000000000001'),(3,1,X'00FF',3),(4,2,NULL,'0'),(5,1,'tail ',NULL)").execute(&t).await.unwrap();
    assert_ab(&s,&t,&request()).await;s.close().await;t.close().await;
}
#[tokio::test]
async fn adaptive_rejects_non_file_sqlite_or_non_native_protocol_and_old_json_stays_optional() {
    let pool=SqlitePoolOptions::new().max_connections(1).connect(":memory:").await.unwrap();let db=Db::Sqlite(pool.clone());
    let opts=DiffMemoryOptions{mode:"manual".into(),limit_bytes:Some(64*1024*1024)};
    let error=diff_with_memory(&db,&DbKind::of("sqlite"),&db,&DbKind::of("sqlite"),&request(),&opts).await.unwrap_err();assert!(error.to_string().contains("MEMORY_BUDGET_UNSUPPORTED"));
    let error=diff_with_memory(&db,&DbKind::of("postgres"),&db,&DbKind::of("postgres"),&request(),&opts).await.unwrap_err();assert!(error.to_string().contains("MEMORY_BUDGET_UNSUPPORTED"));
    let json=serde_json::json!({"source_rows":0,"target_rows":0,"keyed":false,"missing_in_target":0,"extra_in_target":0,"changed":0,"changed_samples":[],"target_missing_columns":[],"target_extra_columns":[],"message":null});
    let old:SyncDiffReport=serde_json::from_value(json.clone()).unwrap();assert!(old.memory.is_none());assert_eq!(serde_json::to_value(old).unwrap(),json);pool.close().await;
}

#[tokio::test]
async fn adaptive_oversized_borrowed_mapping_rejects_before_owned_copy_or_budget_start() {
    let(_dir,s,t)=fixtures().await;
    let controller=super::super::diff_memory_budget::Controller::new(std::sync::Arc::new(StableAvailable));
    let kind=DbKind::of("sqlite");
    let options=DiffMemoryOptions{mode:"manual".into(),limit_bytes:Some(64*1024*1024)};
    let mut req=request();
    req.columns=vec![SyncColumnMapping{source:"id".into(),target:"id".into(),target_type:Some("x".repeat(2*1024*1024))}];
    let error=compare_with_memory(&s,&kind,&t,&kind,&req,&options,&controller).await.unwrap_err();
    assert!(error.to_string().contains("复制前拒绝")); assert_eq!(controller.used(),0);
    let entry=diff_with_memory(&Db::Sqlite(s.clone()),&kind,&Db::Sqlite(t.clone()),&kind,&req,&options).await.unwrap_err();
    assert!(entry.to_string().contains("复制前拒绝"));
    req.columns=vec![SyncColumnMapping{source:"id".into(),target:"id".into(),target_type:None};1025];
    assert!(compare_with_memory(&s,&kind,&t,&kind,&req,&options,&controller).await.unwrap_err().to_string().contains("1024列"));
    assert_eq!(controller.used(),0);
    let invalid_options=DiffMemoryOptions{mode:"x".repeat(2*1024*1024),limit_bytes:None};
    let invalid=diff_with_memory(&Db::Sqlite(s.clone()),&kind,&Db::Sqlite(t.clone()),&kind,&request(),&invalid_options).await.unwrap_err();
    assert!(invalid.to_string().contains("复制前拒绝"));
    s.close().await;t.close().await;
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn adaptive_sqlite_100k_auto_manual_oracle_and_owned_waiter_drop() {
    let(_dir,s,t)=fixtures().await;
    sqlx::raw_sql("CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT,b BLOB);WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000) INSERT INTO items SELECT x,printf('payload-%09d',x),X'00FF80' FROM n").execute(&s).await.unwrap();
    sqlx::raw_sql("CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT,b BLOB);WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000) INSERT INTO items SELECT x,CASE WHEN x IN(1,50000,99990) THEN 'changed' ELSE printf('payload-%09d',x) END,X'00FF80' FROM n").execute(&t).await.unwrap();
    let kind=DbKind::of("sqlite");let req=request();
    let expected=old_report(&Db::Sqlite(s.clone()),&kind,&Db::Sqlite(t.clone()),&req).await.unwrap();
    let automatic=adaptive_run(&s,&t,&req,"auto",None).await.unwrap();
    assert_eq!(automatic.memory.as_ref().unwrap().backend,"memory");assert_eq!(automatic.memory.as_ref().unwrap().spilled_bytes,0);assert_eq!(canonical(automatic),canonical(expected.clone()));
    let controller=super::super::diff_memory_budget::Controller::new(std::sync::Arc::new(StableAvailable));
    let c=controller.clone();let source=s.clone();let target=t.clone();let (finished,receive)=tokio::sync::oneshot::channel();
    let waiter=crate::sql_history::spawn(async move {
        let result=compare_with_memory(&source,&kind,&target,&kind,&req,&DiffMemoryOptions{mode:"manual".into(),limit_bytes:Some(512*1024*1024)},&c).await;
        let _=finished.send(result);
    });
    // Drop ONLY the waiter, not/never abort the owner future holding native readers.
    let deadline=tokio::time::Instant::now()+std::time::Duration::from_secs(10);
    while controller.used()==0 && !waiter.is_finished() && tokio::time::Instant::now()<deadline {tokio::task::yield_now().await;}
    assert!(controller.used()>0,"comparison began and still owns its working reservations");drop(waiter);
    assert!(controller.used()>0,"dropping the waiter did not drop a live reader/store lease");
    let actual=receive.await.unwrap().unwrap();assert_eq!(controller.used(),0,"real reader close and scratch cleanup settled");
    let stats=actual.memory.as_ref().unwrap();assert_eq!(stats.backend,"memory");assert_eq!(stats.spilled_bytes,0);assert!(stats.peak_bytes<=512*1024*1024);
    assert_eq!(canonical(actual),canonical(expected));s.close().await;t.close().await;
}

#[tokio::test]
#[ignore="real 100k-row diff A/B timing; run explicitly in release with --nocapture"]
async fn sqlite_actual_diff_100k_report_ab_benchmark() {
    let(_dir,s,t)=fixtures().await;
    sqlx::raw_sql("CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT,b BLOB);WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000) INSERT INTO items SELECT x,printf('payload-%09d',x),X'00FF80' FROM n").execute(&s).await.unwrap();
    sqlx::raw_sql("CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT,b BLOB);WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000) INSERT INTO items SELECT x,CASE WHEN x IN(1,50000,99990) THEN 'changed' ELSE printf('payload-%09d',x) END,X'00FF80' FROM n;DELETE FROM items WHERE id>=99991;INSERT INTO items VALUES(100001,'extra',X'00FF80')").execute(&t).await.unwrap();
    let kind=DbKind::of("sqlite");let start=std::time::Instant::now();
    let expected=old_report(&Db::Sqlite(s.clone()),&kind,&Db::Sqlite(t.clone()),&request()).await.unwrap();let old=start.elapsed();
    let start=std::time::Instant::now();let actual=run(&s,&t,&request()).await.unwrap();let bounded=start.elapsed();
    assert_eq!((actual.source_rows,actual.target_rows,actual.missing_in_target,actual.extra_in_target,actual.changed),(100000,99991,10,1,3));
    assert_eq!(canonical(actual),canonical(expected));
    eprintln!("actual diff rows/side=100000; old={old:?}; disk-index={bounded:?}; sample_limit=5; SQLite driver row_buffer=1; row_limit={ROW_BYTES}; spill_limit={SPILL_BYTES}. Timings are local observations, not asserted speedups/RSS.");
    s.close().await;t.close().await;
}

#[tokio::test]
async fn sqlite_private_memory_and_view_compatibility_keeps_original_pool_alive() {
    let options=SqliteConnectOptions::new().filename(":memory:").in_memory(true).shared_cache(false);
    let s=SqlitePoolOptions::new().max_connections(1).connect_with(options.clone()).await.unwrap();
    let t=SqlitePoolOptions::new().max_connections(1).connect_with(options).await.unwrap();
    sqlx::raw_sql("CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);INSERT INTO items VALUES(1,'a'),(2,'b');CREATE VIEW viewed AS SELECT * FROM items").execute(&s).await.unwrap();
    sqlx::raw_sql("CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);INSERT INTO items VALUES(1,'a'),(2,'changed');CREATE VIEW viewed AS SELECT * FROM items").execute(&t).await.unwrap();
    assert_ab(&s,&t,&request()).await;
    let mut req=request();req.source_table="viewed".into();req.target_table="viewed".into();assert_ab(&s,&t,&req).await;
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM items").fetch_one(&s).await.unwrap(),2);
    s.close().await;t.close().await;
}

#[tokio::test]
async fn sqlite_generated_columns_preserve_old_mapping_report() {
    let(_dir,s,t)=fixtures().await;
    sqlx::raw_sql("CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT,generated TEXT GENERATED ALWAYS AS(v||'!') VIRTUAL);INSERT INTO items(id,v) VALUES(1,'a'),(2,'b')").execute(&s).await.unwrap();
    sqlx::raw_sql("CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT,generated TEXT GENERATED ALWAYS AS(v||'?') VIRTUAL);INSERT INTO items(id,v) VALUES(1,'a'),(2,'changed')").execute(&t).await.unwrap();
    assert_ab(&s,&t,&request()).await;s.close().await;t.close().await;
}

#[tokio::test]
#[ignore="small-table actual diff latency A/B; no performance claim without observations"]
async fn sqlite_actual_diff_small_ab_benchmark() {
    let(_dir,s,t)=fixtures().await;
    for pool in [&s,&t]{sqlx::raw_sql("CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<50) INSERT INTO items SELECT x,'same' FROM n").execute(pool).await.unwrap();}
    let kind=DbKind::of("sqlite");let start=std::time::Instant::now();
    let expected=old_report(&Db::Sqlite(s.clone()),&kind,&Db::Sqlite(t.clone()),&request()).await.unwrap();let old=start.elapsed();
    let start=std::time::Instant::now();let actual=run(&s,&t,&request()).await.unwrap();let bounded=start.elapsed();
    assert_eq!(canonical(actual),canonical(expected));eprintln!("actual small diff rows/side=50; old={old:?}; bounded={bounded:?}; private scratch initialization may regress small-table latency; not asserted faster");
    s.close().await;t.close().await;
}

// Only explicitly provided disposable databases are touched. Tables use unique names
// and are dropped even when assertions fail; ignored by default (no implicit network).
#[tokio::test]
#[ignore="set NAVKIT_SYNC_DIFF_PG_URL to a disposable PostgreSQL database"]
async fn live_postgres_small_decimal_null_binary_report_ab_and_row_budget()->Result<()> {
    let url=std::env::var("NAVKIT_SYNC_DIFF_PG_URL").context("provide a disposable PG database")?;
    let pool=sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await?;
    let tag=uuid::Uuid::new_v4().simple().to_string();let s=format!("navkit_diff_s_{tag}");let t=format!("navkit_diff_t_{tag}");
    let result:Result<()>=async{
        for name in [&s,&t]{sqlx::query(&format!("CREATE TABLE public.{name}(id BIGINT PRIMARY KEY,n NUMERIC(65,20),b BYTEA)")).execute(&pool).await?;}
        sqlx::query(&format!("INSERT INTO public.{s} VALUES(1,9007199254740993.12345678901234567890,decode('00FF80','hex')),(2,NULL,NULL),(4,0,decode('00','hex'))")).execute(&pool).await?;
        sqlx::query(&format!("INSERT INTO public.{t} VALUES(1,9007199254740993.12345678901234567891,decode('00FF80','hex')),(2,NULL,decode('','hex')),(3,0,decode('00','hex'))")).execute(&pool).await?;
        let db=Db::Postgres(pool.clone());let kind=DbKind::of("postgres");let mut req=request();req.source_schema="public".into();req.target_schema="public".into();req.source_table=s.clone();req.target_table=t.clone();
        let expected=old_report(&db,&kind,&db,&req).await?;let actual=diff_bounded(&db,&kind,&db,&kind,&req).await?;
        assert_eq!((actual.changed,actual.missing_in_target,actual.extra_in_target),(2,1,1));assert_eq!(canonical(actual),canonical(expected));
        sqlx::query(&format!("INSERT INTO public.{t} SELECT x,0,NULL FROM generate_series(100,2200) AS x")).execute(&pool).await?;
        assert!(diff_bounded(&db,&kind,&db,&kind,&req).await.is_err());Ok(())
    }.await;
    for name in [&s,&t]{sqlx::query(&format!("DROP TABLE IF EXISTS public.{name}")).execute(&pool).await?;}pool.close().await;result
}

#[tokio::test]
#[ignore="set NAVKIT_SYNC_DIFF_MYSQL_URL to a disposable MySQL/InnoDB database"]
async fn live_mysql_small_decimal_null_binary_report_ab_and_row_budget()->Result<()> {
    let url=std::env::var("NAVKIT_SYNC_DIFF_MYSQL_URL").context("provide a disposable MySQL database")?;
    let pool=sqlx::mysql::MySqlPoolOptions::new().max_connections(2).connect(&url).await?;
    let tag=uuid::Uuid::new_v4().simple().to_string();let s=format!("navkit_diff_s_{tag}");let t=format!("navkit_diff_t_{tag}");
    let result:Result<()>=async{
        for name in [&s,&t]{sqlx::query(&format!("CREATE TABLE {name}(id BIGINT PRIMARY KEY,n DECIMAL(65,20),b BLOB) ENGINE=InnoDB")).execute(&pool).await?;}
        sqlx::query(&format!("INSERT INTO {s} VALUES(1,9007199254740993.12345678901234567890,X'00FF80'),(2,NULL,NULL),(4,0,X'00')")).execute(&pool).await?;
        sqlx::query(&format!("INSERT INTO {t} VALUES(1,9007199254740993.12345678901234567891,X'00FF80'),(2,NULL,X''),(3,0,X'00')")).execute(&pool).await?;
        let db=Db::Mysql(pool.clone());let kind=DbKind::of("mysql");let mut req=request();req.source_schema=String::new();req.target_schema=String::new();req.source_table=s.clone();req.target_table=t.clone();
        let expected=old_report(&db,&kind,&db,&req).await?;let actual=diff_bounded(&db,&kind,&db,&kind,&req).await?;
        assert_eq!((actual.changed,actual.missing_in_target,actual.extra_in_target),(2,1,1));assert_eq!(canonical(actual),canonical(expected));
        let mut transaction=pool.begin().await?;
        for id in 100..2201{sqlx::query(&format!("INSERT INTO {t} VALUES(?,0,NULL)")).bind(id).execute(&mut *transaction).await?;}transaction.commit().await?;
        assert!(diff_bounded(&db,&kind,&db,&kind,&req).await.is_err());Ok(())
    }.await;
    for name in [&s,&t]{sqlx::query(&format!("DROP TABLE IF EXISTS {name}")).execute(&pool).await?;}pool.close().await;result
}
