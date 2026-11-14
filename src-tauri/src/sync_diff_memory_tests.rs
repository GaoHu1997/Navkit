//! One measured comparison per fresh test process. Never calls the materializing oracle in bounded mode.
use super::*;
#[tokio::test]
#[ignore="explicit independent-process RSS and elapsed comparison; no external database"]
async fn sqlite_diff_independent_process_rss_benchmark() {
    let mode=std::env::var("NAVKIT_DIFF_RSS_MODE").expect("explicit default or bounded mode");
    assert!(matches!(mode.as_str(),"default"|"bounded"));
    let rows=std::env::var("NAVKIT_DIFF_RSS_ROWS").unwrap().parse::<u64>().unwrap();
    assert!(matches!(rows,50|100000));
    let(_dir,s,t)=fixtures().await;
    let create=format!("CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT,b BLOB);WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<{rows}) INSERT INTO items SELECT x,printf('payload-%09d',x),X'00FF80' FROM n");
    for pool in [&s,&t]{sqlx::raw_sql(&create).execute(pool).await.unwrap();}
    if rows==100000 {sqlx::raw_sql("UPDATE items SET v='changed' WHERE id IN(1,50000,99990);DELETE FROM items WHERE id>=99991;INSERT INTO items VALUES(100001,'extra',X'00FF80')").execute(&t).await.unwrap();}
    let kind=DbKind::of("sqlite");let req=request();
    let source=Db::Sqlite(s.clone());let target=Db::Sqlite(t.clone());
    let memory=process_memory::Probe::start();let start=std::time::Instant::now();
    let report=if mode=="default" {crate::sync::diff(&source,&kind,&target,&kind,&req).await} else {crate::sync::diff_bounded(&source,&kind,&target,&kind,&req).await}.unwrap();
    let seconds=start.elapsed().as_secs_f64();let observation=memory.finish();
    let expected=if rows==100000 {(100000,99991,10,1,3)}else{(50,50,0,0,0)};
    assert_eq!((report.source_rows,report.target_rows,report.missing_in_target,report.extra_in_target,report.changed),expected);
    assert!(report.keyed);assert!(report.target_missing_columns.is_empty()&&report.target_extra_columns.is_empty());
    assert_eq!(report.changed_samples.len(),if rows==100000{3}else{0});
    if rows==100000 {let mut samples=report.changed_samples.clone();samples.sort();assert_eq!(samples,["1","50000","99990"]);}
    println!("DIFF_PROCESS_AB mode={mode} rows={rows} seconds={seconds:.6} missing={} extra={} changed={} {observation}",report.missing_in_target,report.extra_in_target,report.changed);
    s.close().await;t.close().await;
}
