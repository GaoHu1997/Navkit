//! 可复现真实 SQLite 文件库微基准；调用产品 synchronize 入口，计时不含造数/清理。
use crate::{db::Db,models::DbKind,sync::{SyncOptions,synchronize}};
use sqlx::{Row,sqlite::{SqliteConnectOptions,SqlitePoolOptions,SqliteJournalMode}};
use std::{path::PathBuf,time::Instant};
struct Directory(PathBuf);
impl Drop for Directory {fn drop(&mut self){let _=std::fs::remove_dir_all(&self.0);}}
fn options(mode:&str)->SyncOptions{SyncOptions{task_id:None,source_connection:"perf-source".into(),source_schema:"main".into(),source_table:"items".into(),target_connection:"perf-target".into(),target_schema:"main".into(),target_table:"items".into(),mode:"data".into(),write_mode:if mode=="full"{"replace"}else{"append"}.into(),sync_mode:mode.into(),incremental_update:true,create_target:false,columns:vec![],key_columns:vec![],delete_missing:false,where_clause:None}}
async fn database(path:&std::path::Path)->sqlx::SqlitePool{SqlitePoolOptions::new().max_connections(1).connect_with(SqliteConnectOptions::new().filename(path).create_if_missing(true).journal_mode(SqliteJournalMode::Delete).foreign_keys(true)).await.unwrap()}
async fn run_case(mode:&str,rows:i64,rounds:usize){
 let dir=Directory(std::env::temp_dir().join(format!("navkit-sync-perf-{}",uuid::Uuid::new_v4())));std::fs::create_dir(&dir.0).unwrap();let source=database(&dir.0.join("source.sqlite")).await;let target=database(&dir.0.join("target.sqlite")).await;
 let ddl="CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT,payload BLOB,amount TEXT)";sqlx::query(ddl).execute(&source).await.unwrap();sqlx::query(ddl).execute(&target).await.unwrap();
 sqlx::query("WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<?) INSERT INTO items SELECT x,printf('row-%08d',x)||char(0)||'尾',CAST(printf('%08d',x) AS BLOB),'12345678901234567890.12345678901234567890' FROM n").bind(rows).execute(&source).await.unwrap();
 let kind=DbKind::of("sqlite");let src=Db::Sqlite(source.clone());let dst=Db::Sqlite(target.clone());let opts=options(mode);let mut samples=Vec::new();
 for round in 0..=rounds {sqlx::query("DELETE FROM items").execute(&target).await.unwrap();if mode=="incremental"{sqlx::query("WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<?) INSERT INTO items SELECT x,'old',X'00','old' FROM n").bind(rows/2).execute(&target).await.unwrap();}
 let start=Instant::now();let report=synchronize(&src,&kind,&dst,&kind,&opts).await.unwrap();let elapsed=start.elapsed().as_secs_f64()*1000.0;assert_eq!(report.rows,rows as u64);
 let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM items").fetch_one(&target).await.unwrap();assert_eq!(count,rows);let row=sqlx::query("SELECT v,payload,amount FROM items WHERE id=?").bind(rows).fetch_one(&target).await.unwrap();assert_eq!(row.get::<String,_>("v"),format!("row-{rows:08}\0尾"));assert_eq!(row.get::<Vec<u8>,_>("payload"),format!("{rows:08}").as_bytes());assert_eq!(row.get::<String,_>("amount"),"12345678901234567890.12345678901234567890");
 if round>0{samples.push(elapsed);} }
 let mut sorted=samples.clone();sorted.sort_by(f64::total_cmp);println!("{}",serde_json::json!({"label":std::env::var("NAVKIT_SYNC_PERF_LABEL").unwrap_or_default(),"engine":"SQLite file DELETE journal FK ON","path":"sync::synchronize","mode":mode,"rows":rows,"warmup":1,"rounds":rounds,"ms":samples,"median_ms":sorted[sorted.len()/2]}));source.close().await;target.close().await;
}
#[tokio::test]
#[ignore = "显式性能测试：设置 NAVKIT_SYNC_PERF_ROWS/ROUNDS 并使用 --ignored"]
async fn sqlite_real_product_full_and_incremental(){let rows=std::env::var("NAVKIT_SYNC_PERF_ROWS").unwrap_or_else(|_|"20000".into()).parse().unwrap();let rounds=std::env::var("NAVKIT_SYNC_PERF_ROUNDS").unwrap_or_else(|_|"3".into()).parse().unwrap();assert!(rows>=2000&&rounds>=3);let mode=std::env::var("NAVKIT_SYNC_PERF_MODE").unwrap_or_else(|_|"all".into());if mode=="all"||mode=="full"{run_case("full",rows,rounds).await;}if mode=="all"||mode=="incremental"{run_case("incremental",rows,rounds).await;}}

#[tokio::test]
#[ignore = "显式真实MySQL基准：harness从仓库fixture读取本机测试凭据"]
async fn mysql_real_product_full(){
    use anyhow::{Result,bail};
    use futures::FutureExt;
    let url=std::env::var("NAVKIT_SYNC_MYSQL_URL").expect("必须显式配置本机fixture URL");
    let admin=sqlx::MySqlPool::connect(&url).await.expect("本机MySQL必须可连接");
    let namespace=format!("navkit_perf_{}",uuid::Uuid::new_v4().simple());assert!(namespace.starts_with("navkit_perf_")&&namespace.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'_'));
    println!("fixture_database={namespace}");
    sqlx::raw_sql(&format!("CREATE DATABASE {namespace} CHARACTER SET utf8mb4")).execute(&admin).await.unwrap();
    let result=std::panic::AssertUnwindSafe(async {
        for table in ["source_items","target_items"]{sqlx::raw_sql(&format!("CREATE TABLE {namespace}.{table}(id BIGINT PRIMARY KEY,v VARCHAR(128),payload BLOB,amount VARCHAR(128)) ENGINE=InnoDB")).execute(&admin).await?;}
        let rows:i64=std::env::var("NAVKIT_SYNC_PERF_ROWS").unwrap_or_else(|_|"20000".into()).parse()?;let rounds:usize=std::env::var("NAVKIT_SYNC_PERF_ROUNDS").unwrap_or_else(|_|"3".into()).parse()?;
        if rows<2000||rounds<3{bail!("基准行数/轮次过小");}
        for first in (1..=rows).step_by(500){let end=(first+499).min(rows);let mut builder=sqlx::QueryBuilder::<sqlx::MySql>::new(format!("INSERT INTO {namespace}.source_items "));builder.push_values(first..=end,|mut row,id|{row.push_bind(id).push_bind(format!("row-{id:08}-尾")).push_bind(format!("{id:08}").into_bytes()).push_bind("12345678901234567890.12345678901234567890");});builder.build().execute(&admin).await?;}
        let kind=DbKind::of("mysql");let db=Db::Mysql(admin.clone());let mut opts=options("full");opts.source_schema=namespace.clone();opts.target_schema=namespace.clone();opts.source_table="source_items".into();opts.target_table="target_items".into();let mut samples=Vec::new();
        for round in 0..=rounds{let start=Instant::now();let report=synchronize(&db,&kind,&db,&kind,&opts).await?;let ms=start.elapsed().as_secs_f64()*1000.0;if report.rows!=rows as u64{bail!("真实同步影响行数不符");}let count:i64=sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {namespace}.target_items")).fetch_one(&admin).await?;if count!=rows{bail!("目标总行数不符");}let row=sqlx::query(&format!("SELECT v,payload,amount FROM {namespace}.target_items WHERE id=?")).bind(rows).fetch_one(&admin).await?;if row.try_get::<String,_>("v")?!=format!("row-{rows:08}-尾")||row.try_get::<Vec<u8>,_>("payload")?!=format!("{rows:08}").as_bytes()||row.try_get::<String,_>("amount")?!="12345678901234567890.12345678901234567890"{bail!("目标类型/内容不符");}if round>0{samples.push(ms);}}
        let mut sorted=samples.clone();sorted.sort_by(f64::total_cmp);println!("{}",serde_json::json!({"label":std::env::var("NAVKIT_SYNC_PERF_LABEL").unwrap_or_default(),"engine":"MySQL 8 local InnoDB","path":"sync::synchronize","mode":"full","rows":rows,"warmup":1,"rounds":rounds,"ms":samples,"median_ms":sorted[sorted.len()/2]}));Ok::<(),anyhow::Error>(())
    }).catch_unwind().await;
    let cleanup=sqlx::raw_sql(&format!("DROP DATABASE {namespace}")).execute(&admin).await;admin.close().await;cleanup.expect("必须清理本次唯一新建数据库");println!("fixture_database_cleaned={namespace}");result.expect("同步测试panic但已清理临时库").unwrap();
}
