//! One actual public diff route per fresh process. Setup/validation excluded from elapsed.
//! RSS includes the Rust process/SQLite cache; accounted budgets are separate diagnostics.
use super::*;

#[tokio::test]
#[ignore = "explicit isolated default/fixed-low/auto/manual performance+RSS comparison"]
async fn adaptive_diff_independent_process_benchmark() {
    let mode=std::env::var("NAVKIT_ADAPTIVE_BENCH_MODE").expect("explicit benchmark mode");
    assert!(matches!(mode.as_str(),"default"|"fixed_low"|"auto"|"manual_fit"|"manual_limited"|"speed"));
    let rows=std::env::var("NAVKIT_ADAPTIVE_BENCH_ROWS").unwrap().parse::<u64>().unwrap();
    let width=std::env::var("NAVKIT_ADAPTIVE_BENCH_WIDTH").unwrap_or("0".into()).parse::<usize>().unwrap();
    let round=std::env::var("NAVKIT_ADAPTIVE_BENCH_ROUND").unwrap_or("1".into());
    assert!(matches!(rows,50|1000|100000|1000000));assert!(matches!(width,0|256|4096));
    let(_directory,s,t)=fixtures().await;
    let sql=format!("CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT,b BLOB);WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<{rows}) INSERT INTO items SELECT x,printf('payload-%09d',x)||?,X'00FF80' FROM n");
    let tail=if width==0 {String::new()}else{format!("中🙂'\n\\{}","x".repeat(width))};
    for pool in [&s,&t]{sqlx::raw_sql("CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT,b BLOB)").execute(pool).await.unwrap();
        let insert=sql.split_once(';').unwrap().1;
        sqlx::query(insert).bind(&tail).execute(pool).await.unwrap();}
    if rows>=100000 {
        let change=format!("UPDATE items SET v='changed' WHERE id IN(1,{});UPDATE items SET b=X'01FF80' WHERE id={};DELETE FROM items WHERE id>{};INSERT INTO items VALUES({},'extra',X'00FF80')",rows-10,rows/2,rows-10,rows+1);
        sqlx::raw_sql(&change).execute(&t).await.unwrap();
    }
    let sqlite:String=sqlx::query_scalar("SELECT sqlite_version()").fetch_one(&s).await.unwrap();
    let kind=DbKind::of("sqlite");let req=request();let source=Db::Sqlite(s.clone());let target=Db::Sqlite(t.clone());
    let options=if matches!(mode.as_str(),"auto"|"manual_fit"|"manual_limited"|"speed") {
        let policy=if matches!(mode.as_str(),"manual_fit"|"manual_limited"){"manual"}else{mode.as_str()};
        let limit=if policy=="manual"{Some(std::env::var("NAVKIT_ADAPTIVE_LIMIT_MIB").ok().map(|v|v.parse::<u64>().unwrap()).unwrap_or(if mode=="manual_fit"{2048}else{64})*1024*1024)}else{None};
        Some(crate::sync::DiffMemoryOptions{mode:policy.into(),limit_bytes:limit})
    }else{None};
    let probe=process_memory::Probe::start();let start=std::time::Instant::now();
    let report=match mode.as_str(){
        "default"=>crate::sync::diff(&source,&kind,&target,&kind,&req).await,
        "fixed_low"=>crate::sync::diff_bounded(&source,&kind,&target,&kind,&req).await,
        _=>crate::sync::diff_with_memory(&source,&kind,&target,&kind,&req,options.as_ref().unwrap()).await,
    }.unwrap();
    let seconds=start.elapsed().as_secs_f64();let rss=probe.finish();
    let expected=if rows>=100000{(rows,rows-9,10,1,3)}else{(rows,rows,0,0,0)};
    assert_eq!((report.source_rows,report.target_rows,report.missing_in_target,report.extra_in_target,report.changed),expected);
    assert!(report.keyed);assert!(report.target_missing_columns.is_empty()&&report.target_extra_columns.is_empty());
    if rows>=100000{let mut actual=report.changed_samples.clone();actual.sort();let mut samples=vec!["1".to_owned(),(rows/2).to_string(),(rows-10).to_string()];samples.sort();assert_eq!(actual,samples);}else{assert!(report.changed_samples.is_empty());}
    if mode=="manual_fit"&&rows<=100000{assert_eq!(report.memory.as_ref().unwrap().spilled_bytes,0,"sufficient budget must not spill merely due to row count");}
    if options.is_none(){assert!(report.memory.is_none(),"legacy JSON shape retains no new budget report");}
    let metrics=serde_json::to_string(&report.memory).unwrap();
    println!("ADAPTIVE_DIFF_AB mode={mode} rows={rows} extra_width={width} round={round} seconds={seconds:.6} sqlite={sqlite} missing={} extra={} changed={} memory={metrics}",report.missing_in_target,report.extra_in_target,report.changed);
    println!("ADAPTIVE_DIFF_RSS mode={mode} rows={rows} extra_width={width} round={round} {rss}");
    s.close().await;t.close().await;
}
