//! Real SQLite execution regressions for difficult incremental keys, values, cancellation and bulk writes.
//! Every case uses new isolated database files; expected assertions are independent of sync helpers.
use anyhow::{anyhow, ensure, Result};
use navkit_lib::{db::Db, models::DbKind, sync::{self, DbSyncRequest, DbSyncTable, SyncColumnMapping, SyncDiffRequest, SyncOptions}, transfer};
use serde_json::{json, Value};
use sqlx::{sqlite::{SqliteConnectOptions, SqlitePoolOptions}, Row, SqlitePool};
use std::{path::{Path, PathBuf}, time::{Duration, Instant}};

struct Fixture { root: PathBuf, source: Db, target: Db }
async fn open(path: &Path) -> Result<SqlitePool> {
    Ok(SqlitePoolOptions::new().max_connections(2).connect_with(SqliteConnectOptions::new().filename(path).create_if_missing(true)).await?)
}
impl Fixture {
    async fn new() -> Result<Self> {
        let root=std::env::temp_dir().join(format!("navkit-sync-regression-{}",uuid::Uuid::new_v4().simple()));std::fs::create_dir(&root)?;
        Ok(Self { source:Db::Sqlite(open(&root.join("source.sqlite3")).await?),target:Db::Sqlite(open(&root.join("target.sqlite3")).await?),root })
    }
    fn src(&self) -> &SqlitePool { if let Db::Sqlite(p)=&self.source {p}else{unreachable!()} }
    fn dst(&self) -> &SqlitePool { if let Db::Sqlite(p)=&self.target {p}else{unreachable!()} }
    async fn cleanup(&self) -> Result<()> {
        self.src().close().await;self.dst().close().await;
        let root=self.root.canonicalize()?;
        ensure!(root.file_name().and_then(|n|n.to_str()).is_some_and(|n|n.starts_with("navkit-sync-regression-")),"refuse non-owned cleanup");
        let deadline=Instant::now()+Duration::from_secs(3);
        loop {match std::fs::remove_dir_all(&root) {
            Ok(())=>return Ok(()),
            Err(e) if (e.kind()==std::io::ErrorKind::PermissionDenied || matches!(e.raw_os_error(),Some(32|33))) && Instant::now()<deadline=>std::thread::sleep(Duration::from_millis(25)),
            Err(e)=>return Err(e.into()),
        }}
    }
}
fn kind() -> DbKind { DbKind::of("sqlite") }
fn options() -> SyncOptions {
    SyncOptions {task_id:Some(uuid::Uuid::new_v4().to_string()),source_connection:"source".into(),source_schema:"main".into(),source_table:"t".into(),target_connection:"target".into(),target_schema:"main".into(),target_table:"t".into(),mode:"data".into(),write_mode:"append".into(),sync_mode:"incremental".into(),incremental_update:true,create_target:false,columns:vec![],where_clause:None}
}
async fn exec(pool: &SqlitePool, sql: &str) -> Result<()> {sqlx::query(sql).execute(pool).await?;Ok(())}
async fn both(f: &Fixture, sql: &str) -> Result<()> {exec(f.src(),sql).await?;exec(f.dst(),sql).await}
async fn diff(f: &Fixture, opts: &SyncOptions) -> Result<sync::SyncDiffReport> {
    sync::diff(&f.source,&kind(),&f.target,&kind(),&SyncDiffRequest {source_connection:opts.source_connection.clone(),source_schema:opts.source_schema.clone(),source_table:opts.source_table.clone(),target_connection:opts.target_connection.clone(),target_schema:opts.target_schema.clone(),target_table:opts.target_table.clone(),where_clause:opts.where_clause.clone(),columns:opts.columns.clone()}).await
}
async fn synchronize(f: &Fixture, opts: &SyncOptions) -> Result<sync::SyncReport> {sync::synchronize(&f.source,&kind(),&f.target,&kind(),opts).await}
async fn cancel_after_observed_writes(task_id: &str) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(15),async {
        loop {
            if let Ok(p)=transfer::task_progress(task_id) {
                ensure!(p.status=="running","writer completed before cooperative cancellation could be observed");
                if p.phase=="write" && p.processed>=5 {transfer::cancel_task(task_id)?;return Ok(());}
            }
            tokio::task::yield_now().await;
        }
    }).await.map_err(|_|anyhow!("no actual writes observed before cancellation deadline"))?
}
async fn seed_bulk(pool: &SqlitePool, table: &str, count: i64) -> Result<()> {
    exec(pool,&format!("CREATE TABLE {table} (id INTEGER PRIMARY KEY,v TEXT NOT NULL)")).await?;
    let mut tx=pool.begin().await?;
    for id in 1..=count {sqlx::query(&format!("INSERT INTO {table} VALUES (?,?)")).bind(id).bind(format!("{id}:中文🙂;O'Reilly\\尾")).execute(&mut *tx).await?;}
    tx.commit().await?;Ok(())
}
async fn run_case(case: &str, f: &Fixture) -> Result<Value> {
    let mut opts=options();
    match case {
        "bigint_precision" => {
            both(f,"CREATE TABLE t(id INTEGER PRIMARY KEY,v BIGINT NOT NULL)").await?;
            exec(f.src(),"INSERT INTO t VALUES(1,9007199254740993)").await?;exec(f.dst(),"INSERT INTO t VALUES(1,9007199254740992)").await?;
            ensure!(diff(f,&opts).await?.changed==1,"diff lost a BIGINT difference above 2^53");
            ensure!(synchronize(f,&opts).await?.rows==1,"BIGINT difference was not updated");
            let value:i64=sqlx::query_scalar("SELECT v FROM t WHERE id=1").fetch_one(f.dst()).await?;ensure!(value==9007199254740993,"BIGINT update lost exact integer");
        }
        "decimal_precision" => {
            // TEXT affinity keeps the exact decimal lexemes in SQLite. DECIMAL in the declared
            // type exercises numeric comparison without SQLite's own REAL storage rounding.
            both(f,"CREATE TABLE t(id INTEGER PRIMARY KEY,v 'DECIMAL TEXT' NOT NULL)").await?;
            let source="9007199254740992.0000000000000001";let target="9007199254740992.0000000000000002";
            sqlx::query("INSERT INTO t VALUES(1,?)").bind(source).execute(f.src()).await?;sqlx::query("INSERT INTO t VALUES(1,?)").bind(target).execute(f.dst()).await?;
            ensure!(diff(f,&opts).await?.changed==1,"diff rounded unequal exact decimals to the same f64");
            ensure!(synchronize(f,&opts).await?.rows==1,"exact decimal difference was not updated");
            let value:String=sqlx::query_scalar("SELECT v FROM t").fetch_one(f.dst()).await?;ensure!(value==source,"decimal lexeme was not preserved");
        }
        "text_whitespace" => {
            both(f,"CREATE TABLE t(id INTEGER PRIMARY KEY,v TEXT)").await?;
            exec(f.src(),"INSERT INTO t VALUES(1,' padded ')").await?;exec(f.dst(),"INSERT INTO t VALUES(1,'padded')").await?;
            ensure!(diff(f,&opts).await?.changed==1,"meaningful TEXT whitespace was trimmed away");
            ensure!(synchronize(f,&opts).await?.rows==1,"TEXT whitespace difference was not updated");
            let value:String=sqlx::query_scalar("SELECT v FROM t").fetch_one(f.dst()).await?;ensure!(value==" padded ","TEXT whitespace lost in target");
        }
        "text_key_whitespace" => {
            both(f,"CREATE TABLE t(id TEXT PRIMARY KEY NOT NULL,v TEXT)").await?;
            exec(f.src(),"INSERT INTO t VALUES('x','one'),(' x ','two')").await?;exec(f.dst(),"INSERT INTO t VALUES('x','one')").await?;
            ensure!(diff(f,&opts).await?.missing_in_target==1,"distinct whitespace-bearing keys collided");
            ensure!(synchronize(f,&opts).await?.rows==1,"distinct TEXT primary key was not inserted");
            let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM t").fetch_one(f.dst()).await?;ensure!(count==2,"TEXT PK collision dropped a row");
        }
        "composite_key_delimiter" => {
            both(f,"CREATE TABLE t(a TEXT NOT NULL,b TEXT NOT NULL,v TEXT,PRIMARY KEY(a,b))").await?;
            let first=("a\u{1}sb","c");let second=("a","b\u{1}sc");
            for (a,b) in [first,second] {sqlx::query("INSERT INTO t VALUES(?,?,'same')").bind(a).bind(b).execute(f.src()).await?;}
            sqlx::query("INSERT INTO t VALUES(?,?,'same')").bind(first.0).bind(first.1).execute(f.dst()).await?;
            ensure!(diff(f,&opts).await?.missing_in_target==1,"composite primary key delimiter collision hid a missing row");
            ensure!(synchronize(f,&opts).await?.rows==1,"composite primary key collision dropped a row");
            let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM t").fetch_one(f.dst()).await?;ensure!(count==2,"composite PK target row count mismatch");
        }
        "incomplete_composite_key_mapping" => {
            exec(f.src(),"CREATE TABLE t(a INTEGER NOT NULL,b INTEGER NOT NULL,v TEXT,PRIMARY KEY(a,b))").await?;exec(f.src(),"INSERT INTO t VALUES(1,1,'first'),(1,2,'second')").await?;
            exec(f.dst(),"CREATE TABLE t(a INTEGER PRIMARY KEY,v TEXT)").await?;exec(f.dst(),"INSERT INTO t VALUES(1,'original')").await?;
            opts.columns=vec![SyncColumnMapping {source:"a".into(),target:"a".into(),target_type:None},SyncColumnMapping {source:"v".into(),target:"v".into(),target_type:None}];
            let error=synchronize(f,&opts).await.err().ok_or_else(||anyhow!("incremental sync accepted incomplete composite primary key mapping"))?;
            ensure!(format!("{error:#}").contains("主键"),"missing PK error lacks key context");
            let value:String=sqlx::query_scalar("SELECT v FROM t").fetch_one(f.dst()).await?;ensure!(value=="original","rejected incomplete PK mapping modified target");
            ensure!(diff(f,&opts).await.is_err(),"diff must reject incomplete composite PK instead of reporting misleading keyed counts");
        }
        "binary_primary_key_update" => {
            both(f,"CREATE TABLE t(id BLOB PRIMARY KEY NOT NULL,v TEXT)").await?;
            exec(f.src(),"INSERT INTO t VALUES(X'00FF4142','new')").await?;exec(f.dst(),"INSERT INTO t VALUES(X'00FF4142','old')").await?;
            ensure!(diff(f,&opts).await?.changed==1,"binary PK diff did not find changed row");
            ensure!(synchronize(f,&opts).await?.rows==1,"binary PK UPDATE predicate did not match real BLOB key");
            let value:String=sqlx::query_scalar("SELECT v FROM t WHERE id=X'00FF4142'").fetch_one(f.dst()).await?;ensure!(value=="new","binary PK row stayed unchanged");
        }
        "nullable_primary_key_rejected" => {
            both(f,"CREATE TABLE t(id TEXT PRIMARY KEY,v TEXT)").await?;
            exec(f.src(),"INSERT INTO t VALUES(NULL,'new')").await?;exec(f.dst(),"INSERT INTO t VALUES('x','existing')").await?;
            ensure!(synchronize(f,&opts).await.is_err(),"NULL key cannot safely identify an incremental row");
            let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM t").fetch_one(f.dst()).await?;ensure!(count==1,"NULL key rejection modified target");
        }
        "duplicate_target_key_rejected" => {
            exec(f.src(),"CREATE TABLE t(id INTEGER PRIMARY KEY,v TEXT)").await?;exec(f.src(),"INSERT INTO t VALUES(1,'new')").await?;
            exec(f.dst(),"CREATE TABLE t(id INTEGER,v TEXT)").await?;exec(f.dst(),"INSERT INTO t VALUES(1,'old-a'),(1,'old-b')").await?;
            let error=synchronize(f,&opts).await.err().ok_or_else(||anyhow!("duplicate target key was silently overwritten in the comparison map"))?;
            ensure!(format!("{error:#}").contains("重复"),"duplicate-key rejection lacks context");
            let rows:Vec<String>=sqlx::query_scalar("SELECT v FROM t ORDER BY v").fetch_all(f.dst()).await?;ensure!(rows==["old-a","old-b"],"ambiguous target key rejection modified rows");
            ensure!(diff(f,&opts).await.is_err(),"diff silently collapsed duplicate target keys");
        }
        "synchronize_mid_write_cancel" => {
            seed_bulk(f.src(),"t",2000).await?;exec(f.dst(),"CREATE TABLE t(id INTEGER PRIMARY KEY,v TEXT)").await?;exec(f.dst(),"INSERT INTO t VALUES(9000,'keep-committed')").await?;
            opts.sync_mode="full".into();opts.write_mode="replace".into();let task_id=opts.task_id.as_ref().unwrap();
            let (result,cancel)=tokio::join!(synchronize(f,&opts),cancel_after_observed_writes(task_id));cancel?;
            ensure!(result.is_err(),"synchronize reported success after mid-write cancellation");
            ensure!(transfer::task_progress(task_id)?.status=="cancelled","cancelled synchronize task status mismatch");
            let rows=sqlx::query("SELECT id,v FROM t").fetch_all(f.dst()).await?;ensure!(rows.len()==1&&rows[0].get::<i64,_>(0)==9000&&rows[0].get::<String,_>(1)=="keep-committed","mid-write cancellation did not roll back DELETE/INSERT");
            return Ok(json!({"writes_observed_before_cancel":5,"source_rows":2000,"target_transaction_rolled_back":true}));
        }
        "whole_database_last_table_cancel" => {
            seed_bulk(f.src(),"first",1).await?;seed_bulk(f.src(),"bulk",2000).await?;exec(f.dst(),"CREATE TABLE bulk(id INTEGER PRIMARY KEY,v TEXT)").await?;exec(f.dst(),"INSERT INTO bulk VALUES(9000,'keep-committed')").await?;
            let task_id=uuid::Uuid::new_v4().to_string();
            let request=DbSyncRequest {task_id:Some(task_id.clone()),source_connection:"source".into(),source_schema:"main".into(),target_connection:"target".into(),target_schema:"main".into(),mode:"both".into(),write_mode:"replace".into(),sync_mode:"full".into(),incremental_update:false,tables:vec![DbSyncTable{source:"first".into(),target:"first".into(),view:false},DbSyncTable{source:"bulk".into(),target:"bulk".into(),view:false}],where_clause:None};
            let sqlite=kind();let writer=sync::synchronize_database(&f.source,&sqlite,&f.target,&sqlite,&request);
            let (result,cancel)=tokio::join!(writer,cancel_after_observed_writes(&task_id));cancel?;
            ensure!(result.is_err(),"whole-database sync swallowed cancellation in its final table and returned success");
            ensure!(transfer::task_progress(&task_id)?.status=="cancelled","last-table cancellation became completed");
            let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM first").fetch_one(f.dst()).await?;ensure!(count==1,"earlier independent table commit should remain");
            let value:String=sqlx::query_scalar("SELECT v FROM bulk WHERE id=9000").fetch_one(f.dst()).await?;ensure!(value=="keep-committed","last table cancellation did not restore committed data");
        }
        "bulk_2000_rows_roundtrip" => {
            seed_bulk(f.src(),"t",2000).await?;
            opts.mode="both".into();opts.sync_mode="full".into();opts.create_target=true;
            ensure!(synchronize(f,&opts).await?.rows==2000,"bulk full sync truncated rows");
            let before=sqlx::query("SELECT id,v FROM t ORDER BY id").fetch_all(f.dst()).await?;
            ensure!(before.len()==2000,"bulk target row count mismatch");
            for (i,row) in before.iter().enumerate(){let id=(i+1) as i64;ensure!(row.get::<i64,_>(0)==id&&row.get::<String,_>(1)==format!("{id}:中文🙂;O'Reilly\\尾"),"bulk exact row readback mismatch at {id}");}
            exec(f.src(),"UPDATE t SET v='changed中文' WHERE id=1").await?;exec(f.src(),"INSERT INTO t VALUES(2001,'new中文')").await?;
            opts.task_id=Some(uuid::Uuid::new_v4().to_string());opts.sync_mode="incremental".into();
            ensure!(synchronize(f,&opts).await?.rows==2,"bulk incremental insert/update mismatch");
            let wanted:Vec<(i64,String)>=sqlx::query_as("SELECT id,v FROM t ORDER BY id").fetch_all(f.src()).await?;
            let actual:Vec<(i64,String)>=sqlx::query_as("SELECT id,v FROM t ORDER BY id").fetch_all(f.dst()).await?;ensure!(actual==wanted,"bulk incremental exact readback mismatch");
            return Ok(json!({"full_rows":2000,"incremental_writes":2,"verified_cells":4002}));
        }
        "same_file_connection_alias_guard" => {
            seed_bulk(f.src(),"t",1).await?;let alias=Db::Sqlite(open(&f.root.join("source.sqlite3")).await?);
            opts.mode="both".into();opts.sync_mode="full".into();opts.write_mode="recreate".into();opts.create_target=true;
            let result=sync::synchronize(&f.source,&kind(),&alias,&kind(),&opts).await;
            if let Db::Sqlite(pool)=&alias {pool.close().await;}
            ensure!(result.is_err(),"same-file connection alias was allowed to recreate source table");
            let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM t").fetch_one(f.src()).await?;ensure!(count==1,"source alias guard lost data");
        }
        _ => unreachable!(),
    }
    Ok(json!({"actual_rust_synchronize_or_diff":true,"independent_sql_readback":true}))
}
#[tokio::test]
async fn sqlite_incremental_safety_and_precision_regressions() {
    let cases=["bigint_precision","decimal_precision","text_whitespace","text_key_whitespace","composite_key_delimiter","incomplete_composite_key_mapping","binary_primary_key_update","nullable_primary_key_rejected","duplicate_target_key_rejected","synchronize_mid_write_cancel","whole_database_last_table_cancel","bulk_2000_rows_roundtrip","same_file_connection_alias_guard"];
    let mut rows=vec![];
    for case in cases {
        let started=Instant::now();let fixture=Fixture::new().await.unwrap();
        let result=run_case(case,&fixture).await;let cleanup=fixture.cleanup().await;
        let mut row=json!({"case":case,"evidence_type":"REAL_SQLITE_FILE_EXECUTION","cleanup_success":cleanup.is_ok(),"elapsed_ms":started.elapsed().as_millis()});
        match result {Ok(details) if cleanup.is_ok()=>{row["status"]=json!("PASS");row["details"]=details;},Ok(_)=>{row["status"]=json!("FAIL");row["error"]=json!(format!("cleanup failed: {:#}",cleanup.unwrap_err()));},Err(error)=>{row["status"]=json!("FAIL");row["error"]=json!(format!("{error:#}"));}}
        println!("{case}: {} {}",row["status"],row.get("error").unwrap_or(&Value::Null));rows.push(row);
    }
    let pass=rows.iter().filter(|r|r["status"]=="PASS").count();
    let report=json!({"schema_version":1,"suite":"incremental-safety-and-precision-regressions","timestamp_utc":chrono::Utc::now().to_rfc3339(),"counts":{"total":rows.len(),"pass":pass,"fail":rows.len()-pass},"rows":rows,"limitations":["Decimal lexeme regression uses SQLite TEXT affinity to avoid SQLite's own NUMERIC-to-REAL rounding. Live MySQL/PostgreSQL decimal coverage is reported separately.","The 2000-row case is bounded bulk correctness coverage, not a throughput benchmark or large-scale memory guarantee."]});
    let output=std::env::var("NAVKIT_SYNC_REGRESSION_REPORT").map(PathBuf::from).unwrap_or_else(|_|Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/acceptance/sync/regressions-report.json"));
    if let Some(parent)=output.parent(){std::fs::create_dir_all(parent).unwrap();}std::fs::write(&output,serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    assert_eq!(pass,cases.len(),"actual execution regressions failed; see {}",output.display());
}
