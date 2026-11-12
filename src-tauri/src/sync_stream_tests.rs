//! 真实 SQLite 回归；此文件尚未由本子代理编译/执行，父代理统一 fresh。
use super::*;
use std::path::{Path, PathBuf};
use std::time::Duration;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self { let path=std::env::temp_dir().join(format!("navkit-sync-stream-test-{}",uuid::Uuid::new_v4()));std::fs::create_dir(&path).unwrap();Self(path) }
    fn path(&self,name:&str)->PathBuf{self.0.join(name)}
}
impl Drop for Directory {fn drop(&mut self){let _=std::fs::remove_dir_all(&self.0);}}
async fn database(path:&Path,wal:bool)->SqlitePool {
    SqlitePoolOptions::new().max_connections(1).connect_with(SqliteConnectOptions::new().filename(path).create_if_missing(true).journal_mode(if wal{SqliteJournalMode::Wal}else{SqliteJournalMode::Delete}).busy_timeout(Duration::from_secs(15))).await.unwrap()
}
async fn execute(pool:&SqlitePool,sql:&str){sqlx::query(sql).execute(pool).await.unwrap();}
async fn count(pool:&SqlitePool,table:&str)->i64{sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {}",super::table(table))).fetch_one(pool).await.unwrap()}
async fn table_exists(pool:&SqlitePool,name:&str)->bool{sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM main.sqlite_schema WHERE type='table' AND name=?").bind(name).fetch_one(pool).await.unwrap()!=0}
fn options(source_table:&str,target_table:&str)->SyncOptions {
    SyncOptions{task_id:None,source_connection:"source-window-alias".into(),source_schema:"main".into(),source_table:source_table.into(),target_connection:"target-window-alias".into(),target_schema:"main".into(),target_table:target_table.into(),mode:"data".into(),write_mode:"append".into(),sync_mode:"incremental".into(),incremental_update:true,create_target:false,columns:Vec::new(),key_columns:Vec::new(),delete_missing:false,where_clause:None}
}
async fn synchronize(source:&SqlitePool,target:&SqlitePool,options:&SyncOptions)->Result<SyncReport>{sqlite_incremental(source,target,options,None,"test").await}
async fn public_synchronize(source:&SqlitePool,target:&SqlitePool,options:&SyncOptions)->Result<SyncReport>{
    let kind=crate::models::DbKind::of("sqlite");
    crate::sync::synchronize(&crate::db::Db::Sqlite(source.clone()),&kind,&crate::db::Db::Sqlite(target.clone()),&kind,options).await
}

#[tokio::test]
async fn public_incremental_entry_uses_business_keys_and_the_exact_typed_transaction(){
    let dir=Directory::new();let source=database(&dir.path("source.sqlite"),false).await;let target=database(&dir.path("target.sqlite"),false).await;
    execute(&source,"CREATE TABLE items(sid INTEGER PRIMARY KEY,k TEXT,payload BLOB,amount TEXT);INSERT INTO items VALUES(1,'business',X'00FF80','12345678901234567890.12345678901234567890')").await;
    execute(&target,"CREATE TABLE items(tid INTEGER PRIMARY KEY,k TEXT,payload BLOB,amount TEXT,local TEXT);INSERT INTO items VALUES(900,'business',X'11','old','owned');INSERT INTO items VALUES(901,'target-only',NULL,NULL,'retain')").await;
    let mut opts=options("items","items");opts.key_columns=vec!["k".into()];opts.columns=["k","payload","amount"].into_iter().map(|name|SyncColumnMapping{source:name.into(),target:name.into(),target_type:None}).collect();
    let report=public_synchronize(&source,&target,&opts).await.unwrap();assert_eq!(report.rows,1);assert_eq!(count(&target,"items").await,2);
    let row=sqlx::query("SELECT tid,payload,amount,local FROM items WHERE k='business'").fetch_one(&target).await.unwrap();assert_eq!(row.get::<i64,_>("tid"),900);assert_eq!(row.get::<Vec<u8>,_>("payload"),[0,255,128]);assert_eq!(row.get::<String,_>("amount"),"12345678901234567890.12345678901234567890");assert_eq!(row.get::<String,_>("local"),"owned");
    let error=public_synchronize(&source,&source,&options("items","ITEMS")).await.unwrap_err();assert!(format!("{error:#}").contains("同一物理表"));assert_eq!(count(&source,"items").await,1);
    source.close().await;target.close().await;
}

#[tokio::test]
async fn public_delete_missing_flag_is_rejected_in_every_mode_before_any_pool_io_or_ddl(){
    let dir=Directory::new();let source=database(&dir.path("source.sqlite"),false).await;let target=database(&dir.path("target.sqlite"),false).await;
    execute(&source,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);INSERT INTO items VALUES(1,'source')").await;execute(&target,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);INSERT INTO items VALUES(99,'preserve')").await;
    let schema_before:i64=sqlx::query_scalar("PRAGMA main.schema_version").fetch_one(&target).await.unwrap();
    // Both single-connection pools stay leased: a metadata or DDL attempt would block.
    let source_lease=source.acquire().await.unwrap();let target_lease=target.acquire().await.unwrap();
    for mode in ["data","both","structure"]{for write in ["append","replace","recreate"]{for sync_mode in ["","full","incremental"]{for table in ["items","absent"]{
        let mut opts=options("items",table);opts.mode=mode.into();opts.write_mode=write.into();opts.sync_mode=sync_mode.into();opts.create_target=true;opts.delete_missing=true;
        let error=tokio::time::timeout(Duration::from_secs(2),public_synchronize(&source,&target,&opts)).await.expect("delete flag must fail before attempting a pooled connection").unwrap_err();assert!(format!("{error:#}").contains("legacy delete_missing=true"),"{mode}/{write}/{sync_mode}: {error:#}");
    }}}}
    drop(source_lease);drop(target_lease);assert!(!table_exists(&target,"absent").await);assert_eq!(count(&target,"items").await,1);assert_eq!(sqlx::query_scalar::<_,String>("SELECT v FROM items WHERE id=99").fetch_one(&target).await.unwrap(),"preserve");assert_eq!(sqlx::query_scalar::<_,i64>("PRAGMA main.schema_version").fetch_one(&target).await.unwrap(),schema_before);
    source.close().await;target.close().await;
}

#[tokio::test]
async fn public_invalid_incremental_combinations_cannot_reach_structure_or_full_replacement(){
    let dir=Directory::new();let source=database(&dir.path("source.sqlite"),false).await;let target=database(&dir.path("target.sqlite"),false).await;
    execute(&target,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);INSERT INTO items VALUES(99,'preserve')").await;
    let source_lease=source.acquire().await.unwrap();let target_lease=target.acquire().await.unwrap();
    for (mode,write,sync_mode,expected) in [("structure","append","incremental","必须包含数据"),("both","replace","incremental","只支持"),("both","recreate","incremental","只支持"),("both","append","incremental ","同步方式无效")]{
        let mut opts=options("missing_source","items");opts.mode=mode.into();opts.write_mode=write.into();opts.sync_mode=sync_mode.into();opts.create_target=true;
        let error=tokio::time::timeout(Duration::from_secs(2),public_synchronize(&source,&target,&opts)).await.expect("invalid incremental options must be rejected before metadata").unwrap_err();assert!(format!("{error:#}").contains(expected),"{error:#}");
    }
    drop(source_lease);drop(target_lease);assert_eq!(count(&target,"items").await,1);assert_eq!(sqlx::query_scalar::<_,String>("SELECT v FROM items WHERE id=99").fetch_one(&target).await.unwrap(),"preserve");source.close().await;target.close().await;
}

#[tokio::test]
async fn public_unverified_incremental_channels_and_kind_mismatches_fail_before_database_io(){
    let dir=Directory::new();let source=database(&dir.path("source.sqlite"),false).await;let target=database(&dir.path("target.sqlite"),false).await;
    // Lazy pool has no network connection; unsupported routing must never acquire it.
    let remote=sqlx::mysql::MySqlPoolOptions::new().max_connections(1).acquire_timeout(Duration::from_secs(1)).connect_lazy("mysql://sync_fixture:unused@127.0.0.1:1/sync_fixture").unwrap();
    let source_lease=source.acquire().await.unwrap();let target_lease=target.acquire().await.unwrap();
    let cases=vec![(crate::db::Db::Mysql(remote.clone()),"mysql",crate::db::Db::Sqlite(target.clone()),"sqlite"),(crate::db::Db::Sqlite(source.clone()),"sqlite",crate::db::Db::Mysql(remote.clone()),"mysql"),(crate::db::Db::Mysql(remote.clone()),"mysql",crate::db::Db::Mysql(remote.clone()),"mysql"),(crate::db::Db::Sqlite(source.clone()),"postgresql",crate::db::Db::Sqlite(target.clone()),"sqlite")];
    for (source_db,source_kind,target_db,target_kind) in cases{
        let mut opts=options("missing_source","absent");opts.mode="both".into();opts.create_target=true;
        let error=tokio::time::timeout(Duration::from_secs(2),crate::sync::synchronize(&source_db,&crate::models::DbKind::of(source_kind),&target_db,&crate::models::DbKind::of(target_kind),&opts)).await.expect("unverified channel must not acquire a database connection").unwrap_err();assert!(format!("{error:#}").contains("有界快照/typed事务"),"{error:#}");
    }
    drop(source_lease);drop(target_lease);assert!(!table_exists(&target,"absent").await);remote.close().await;source.close().await;target.close().await;
}

#[tokio::test]
async fn public_nonincremental_append_replace_recreate_and_structure_paths_are_preserved(){
    let dir=Directory::new();let source=database(&dir.path("source.sqlite"),false).await;let target=database(&dir.path("target.sqlite"),false).await;
    execute(&source,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);INSERT INTO items VALUES(1,'source')").await;
    for (sync_mode,write) in [("","append"),("full","replace"),("full","recreate")]{
        execute(&target,"DROP TABLE IF EXISTS items;CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);INSERT INTO items VALUES(99,'old')").await;
        let mut opts=options("items","items");opts.sync_mode=sync_mode.into();opts.write_mode=write.into();opts.mode=if write=="recreate"{"both".into()}else{"data".into()};opts.create_target=true;
        let report=public_synchronize(&source,&target,&opts).await.unwrap();assert_eq!(report.rows,1);assert_eq!(report.created,write=="recreate");assert_eq!(count(&target,"items").await,if write=="append"{2}else{1});assert_eq!(sqlx::query_scalar::<_,String>("SELECT v FROM items WHERE id=1").fetch_one(&target).await.unwrap(),"source");
    }
    let mut opts=options("items","schema_only");opts.sync_mode="full".into();opts.mode="structure".into();opts.create_target=true;
    let report=public_synchronize(&source,&target,&opts).await.unwrap();assert!(report.created);assert_eq!(report.rows,0);assert!(table_exists(&target,"schema_only").await);assert_eq!(count(&target,"schema_only").await,0);source.close().await;target.close().await;
}

#[tokio::test]
async fn public_database_sync_checks_incremental_channel_before_skip_and_routes_each_table(){
    let dir=Directory::new();let source=database(&dir.path("source.sqlite"),false).await;let target=database(&dir.path("target.sqlite"),false).await;
    execute(&source,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);INSERT INTO items VALUES(1,'source')").await;execute(&target,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);INSERT INTO items VALUES(1,'old')").await;
    let mut req=crate::sync::DbSyncRequest{task_id:None,source_connection:"source".into(),source_schema:"main".into(),target_connection:"target".into(),target_schema:"main".into(),mode:"both".into(),write_mode:"append".into(),sync_mode:"incremental".into(),incremental_update:true,tables:vec![crate::sync::DbSyncTable{source:"items".into(),target:"items".into(),view:false}],where_clause:None};
    let source_db=crate::db::Db::Sqlite(source.clone());let target_db=crate::db::Db::Sqlite(target.clone());let kind=crate::models::DbKind::of("sqlite");
    let source_lease=source.acquire().await.unwrap();let target_lease=target.acquire().await.unwrap();
    for write in ["append","skip"]{
        req.write_mode=write.into();let error=tokio::time::timeout(Duration::from_secs(2),crate::sync::synchronize_database(&source_db,&crate::models::DbKind::of("mysql"),&target_db,&kind,&req)).await.expect("database incremental guard must precede metadata/skip").unwrap_err();assert!(format!("{error:#}").contains("有界快照/typed事务"),"{error:#}");
    }
    req.mode="structure".into();req.write_mode="append".into();let error=tokio::time::timeout(Duration::from_secs(2),crate::sync::synchronize_database(&source_db,&kind,&target_db,&kind,&req)).await.expect("structure-only incremental must not acquire either pool").unwrap_err();assert!(format!("{error:#}").contains("必须包含数据"));
    drop(source_lease);drop(target_lease);req.mode="both".into();let reports=crate::sync::synchronize_database(&source_db,&kind,&target_db,&kind,&req).await.unwrap();assert_eq!(reports.len(),1);assert!(reports[0].error.is_none(),"{:?}",reports[0].error);assert_eq!(reports[0].rows,1);assert_eq!(sqlx::query_scalar::<_,String>("SELECT v FROM items WHERE id=1").fetch_one(&target).await.unwrap(),"source");
    source.close().await;target.close().await;
}

struct TestTask {id:String,control:TaskControl}
impl TestTask{fn new()->Self{let id=uuid::Uuid::new_v4().to_string();let control=TaskControl::begin(&id).unwrap();Self{id,control}}}
impl Drop for TestTask{fn drop(&mut self){let result:Result<()>=Err(anyhow!("test fixture released"));self.control.finish(&result);}}
async fn wait_read(task:&TestTask,worker:&tokio::task::JoinHandle<Result<SyncReport>>,minimum:u64)->Result<crate::transfer::TaskProgress>{
    tokio::time::timeout(Duration::from_secs(10),async{loop{let status=crate::transfer::task_progress(&task.id)?;if status.phase=="read"&&status.processed>=minimum{return Ok(status);}if worker.is_finished(){bail!("同步在预期源游标观察点前已结束: {}",status.message);}tokio::time::sleep(Duration::from_millis(1)).await;}}).await.map_err(|_|anyhow!("等待源快照观察点超过10秒"))?
}

#[tokio::test]
async fn sqlite_business_key_updates_the_matching_row_and_preserves_exact_values_and_local_columns(){
    let dir=Directory::new();let source=database(&dir.path("source.sqlite"),false).await;let target=database(&dir.path("target.sqlite"),false).await;
    execute(&source,"CREATE TABLE items(sid INTEGER PRIMARY KEY,business TEXT NOT NULL,payload BLOB,note TEXT,amount TEXT,big INTEGER)").await;
    execute(&target,"CREATE TABLE items(tid INTEGER PRIMARY KEY,business TEXT,payload BLOB,note TEXT,amount TEXT,big INTEGER,local TEXT NOT NULL DEFAULT 'keep');INSERT INTO items VALUES(900,' 001 ',X'11','old','0',1,'owned');INSERT INTO items VALUES(901,'obsolete',NULL,NULL,NULL,NULL,'retain')").await;
    let amount="12345678901234567890.123456789012345678901234567890";
    sqlx::query("INSERT INTO items VALUES(10,' 001 ',?,NULL,?,?)").bind(&b"\0\xff\x80"[..]).bind(amount).bind(i64::MAX).execute(&source).await.unwrap();
    sqlx::query("INSERT INTO items VALUES(11,'002',?,?,?,?)").bind(&b""[..]).bind("literal NULL\0 text").bind("0.000000000000000000000000000001").bind(i64::MIN).execute(&source).await.unwrap();
    let mut opts=options("items","items");opts.key_columns=vec!["business".into()];opts.columns=["business","payload","note","amount","big"].into_iter().map(|name|SyncColumnMapping{source:name.into(),target:name.into(),target_type:None}).collect();
    let report=synchronize(&source,&target,&opts).await.unwrap();assert_eq!(report.rows,2);assert!(!report.created);assert_eq!(count(&target,"items").await,3);
    let row=sqlx::query("SELECT * FROM items WHERE business=' 001 '").fetch_one(&target).await.unwrap();assert_eq!(row.get::<i64,_>("tid"),900);assert_eq!(row.get::<Vec<u8>,_>("payload"),b"\0\xff\x80");assert_eq!(row.get::<Option<String>,_>("note"),None);assert_eq!(row.get::<String,_>("amount"),amount);assert_eq!(row.get::<i64,_>("big"),i64::MAX);assert_eq!(row.get::<String,_>("local"),"owned");
    let row=sqlx::query("SELECT * FROM items WHERE business='002'").fetch_one(&target).await.unwrap();assert_eq!(row.get::<Vec<u8>,_>("payload"),Vec::<u8>::new());assert_eq!(row.get::<String,_>("note"),"literal NULL\0 text");assert_eq!(row.get::<i64,_>("big"),i64::MIN);assert_eq!(row.get::<String,_>("local"),"keep");
    assert_eq!(synchronize(&source,&target,&opts).await.unwrap().rows,0);
    execute(&source,"UPDATE items SET note='new text' WHERE business=' 001 '").await;opts.incremental_update=false;assert_eq!(synchronize(&source,&target,&opts).await.unwrap().rows,0);
    assert_eq!(sqlx::query_scalar::<_,Option<String>>("SELECT note FROM items WHERE tid=900").fetch_one(&target).await.unwrap(),None);
    source.close().await;target.close().await;
}

#[tokio::test]
async fn sqlite_same_pool_one_connection_different_tables_is_supported_but_same_table_alias_is_rejected(){
    let dir=Directory::new();let pool=database(&dir.path("same.sqlite"),false).await;execute(&pool,"CREATE TABLE source(id INTEGER PRIMARY KEY,v TEXT);CREATE TABLE target(id INTEGER PRIMARY KEY,v TEXT);INSERT INTO source VALUES(1,'payload')").await;
    assert!(same_sqlite_database(&pool,&pool).await.unwrap());assert_eq!(synchronize(&pool,&pool,&options("source","target")).await.unwrap().rows,1);assert_eq!(count(&pool,"target").await,1);
    let alias=database(&dir.path("same.sqlite"),false).await;assert!(same_sqlite_database(&pool,&alias).await.unwrap());let error=synchronize(&pool,&alias,&options("source","SOURCE")).await.unwrap_err();assert!(format!("{error:#}").contains("同一物理表"));assert_eq!(count(&pool,"source").await,1);
    alias.close().await;pool.close().await;
}

#[tokio::test]
async fn sqlite_hard_link_alias_with_distinct_table_names_is_rejected_before_write(){
    let dir=Directory::new();let original=dir.path("original.sqlite");let linked=dir.path("linked.sqlite");let source=database(&original,false).await;execute(&source,"CREATE TABLE source(id INTEGER PRIMARY KEY,v TEXT);CREATE TABLE target(id INTEGER PRIMARY KEY,v TEXT);INSERT INTO source VALUES(1,'payload')").await;
    std::fs::hard_link(&original,&linked).expect("temporary test directory must support same-volume hard links");let target=database(&linked,false).await;assert!(same_sqlite_database(&source,&target).await.unwrap());let error=synchronize(&source,&target,&options("source","target")).await.unwrap_err();assert!(format!("{error:#}").contains("硬链接别名"));assert_eq!(count(&target,"target").await,0);assert_eq!(count(&source,"source").await,1);
    target.close().await;source.close().await;
}

#[tokio::test]
async fn sqlite_shared_memory_pools_keep_distinct_identity_and_same_pool_anchor(){
    let source=SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();let target=SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();assert!(!same_sqlite_database(&source,&target).await.unwrap());
    execute(&source,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);INSERT INTO items VALUES(1,'memory');CREATE TABLE target(id INTEGER PRIMARY KEY,v TEXT)").await;execute(&target,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT)").await;
    assert_eq!(synchronize(&source,&target,&options("items","items")).await.unwrap().rows,1);assert_eq!(synchronize(&source,&source,&options("items","target")).await.unwrap().rows,1);assert_eq!(count(&source,"items").await,1);
    source.close().await;target.close().await;
}

#[tokio::test]
async fn sqlite_null_or_duplicate_source_keys_fail_before_create_target(){
    for values in ["(NULL,'bad')","('duplicate','a'),('duplicate','b')"]{
        let dir=Directory::new();let source=database(&dir.path("source.sqlite"),false).await;let target=database(&dir.path("target.sqlite"),false).await;execute(&source,&format!("CREATE TABLE items(k TEXT,v TEXT);INSERT INTO items VALUES {values}")).await;
        let mut opts=options("items","items");opts.key_columns=vec!["k".into()];opts.mode="both".into();opts.create_target=true;let error=synchronize(&source,&target,&opts).await.unwrap_err();let message=format!("{error:#}");assert!(message.contains("NULL")||message.contains("键重复"),"{message}");assert!(!table_exists(&target,"items").await);
        source.close().await;target.close().await;
    }
}

#[tokio::test]
async fn sqlite_duplicate_or_null_target_keys_fail_before_any_data_change(){
    for values in ["('duplicate','a'),('duplicate','b')","(NULL,'unrelated')"]{
        let dir=Directory::new();let source=database(&dir.path("source.sqlite"),false).await;let target=database(&dir.path("target.sqlite"),false).await;execute(&source,"CREATE TABLE items(k TEXT,v TEXT);INSERT INTO items VALUES('new','payload')").await;execute(&target,&format!("CREATE TABLE items(k TEXT,v TEXT);INSERT INTO items VALUES {values}")).await;
        let before=count(&target,"items").await;let mut opts=options("items","items");opts.key_columns=vec!["k".into()];let error=synchronize(&source,&target,&opts).await.unwrap_err();let message=format!("{error:#}");assert!(message.contains("NULL")||message.contains("键重复"),"{message}");assert_eq!(count(&target,"items").await,before);assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM items WHERE k='new'").fetch_one(&target).await.unwrap(),0);
        source.close().await;target.close().await;
    }
}

#[tokio::test]
async fn sqlite_source_key_collisions_after_target_affinity_conversion_fail_before_ddl(){
    let dir=Directory::new();let source=database(&dir.path("source.sqlite"),false).await;let target=database(&dir.path("target.sqlite"),false).await;execute(&source,"CREATE TABLE items(k TEXT PRIMARY KEY,v TEXT);INSERT INTO items VALUES('1','one'),('01','leading zero')").await;
    let mut opts=options("items","items");opts.mode="both".into();opts.create_target=true;opts.columns=vec![SyncColumnMapping{source:"k".into(),target:"k".into(),target_type:Some("NUMERIC".into())},SyncColumnMapping{source:"v".into(),target:"v".into(),target_type:None}];let error=synchronize(&source,&target,&opts).await.unwrap_err();assert!(format!("{error:#}").contains("转换后发生碰撞"));assert!(!table_exists(&target,"items").await);
    source.close().await;target.close().await;
}

#[tokio::test]
async fn sqlite_lossy_decimal_and_integer_affinity_conversions_are_rejected_without_rounding(){
    for (source_type,target_type,value) in [("TEXT","NUMERIC",Cell::Text("12345678901234567890.12345678901234567890".into())),("INTEGER","REAL",Cell::Integer(i64::MAX))]{
        let dir=Directory::new();let source=database(&dir.path("source.sqlite"),false).await;let target=database(&dir.path("target.sqlite"),false).await;execute(&source,&format!("CREATE TABLE items(k INTEGER PRIMARY KEY,v {source_type})")).await;execute(&target,&format!("CREATE TABLE items(k INTEGER PRIMARY KEY,v {target_type});INSERT INTO items VALUES(99,1)")).await;
        bind(crate::sql_history::query("INSERT INTO items VALUES(?,?)"),&[Cell::Integer(1),value]).execute(&source).await.unwrap();let error=synchronize(&source,&target,&options("items","items")).await.unwrap_err();assert!(format!("{error:#}").contains("不能无损存储"));assert_eq!(count(&target,"items").await,1);assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM items WHERE k=1").fetch_one(&target).await.unwrap(),0);
        source.close().await;target.close().await;
    }
}

#[tokio::test]
async fn sqlite_invalid_utf8_is_not_exported_as_null_or_replacement_text(){
    let dir=Directory::new();let source=database(&dir.path("source.sqlite"),false).await;let target=database(&dir.path("target.sqlite"),false).await;execute(&source,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);INSERT INTO items VALUES(1,CAST(X'FF' AS TEXT))").await;execute(&target,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT)").await;
    let error=synchronize(&source,&target,&options("items","items")).await.unwrap_err();assert!(format!("{error:#}").contains("UTF-8"));assert_eq!(count(&target,"items").await,0);source.close().await;target.close().await;
}

#[tokio::test]
async fn sqlite_constraint_failure_rolls_back_prior_verified_rows(){
    let dir=Directory::new();let source=database(&dir.path("source.sqlite"),false).await;let target=database(&dir.path("target.sqlite"),false).await;execute(&source,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);INSERT INTO items VALUES(1,'valid'),(2,'bad')").await;execute(&target,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT CHECK(v<>'bad'));INSERT INTO items VALUES(99,'keep')").await;
    let task=TestTask::new();let result=sqlite_incremental(&source,&target,&options("items","items"),Some(&task.control),"test").await;let error=result.unwrap_err();assert!(format!("{error:#}").contains("已确认回滚"));let progress=crate::transfer::task_progress(&task.id).unwrap();assert_eq!(progress.phase,"write");assert_eq!(progress.processed,1);assert_eq!(count(&target,"items").await,1);assert_eq!(sqlx::query_scalar::<_,String>("SELECT v FROM items WHERE id=99").fetch_one(&target).await.unwrap(),"keep");source.close().await;target.close().await;
}

#[tokio::test]
async fn sqlite_trigger_readback_mismatch_and_zero_affected_rows_roll_back(){
    for trigger in ["CREATE TRIGGER change AFTER INSERT ON items BEGIN UPDATE items SET v='triggered' WHERE id=NEW.id;END","CREATE TRIGGER ignore_row BEFORE INSERT ON items BEGIN SELECT RAISE(IGNORE);END"]{
        let dir=Directory::new();let source=database(&dir.path("source.sqlite"),false).await;let target=database(&dir.path("target.sqlite"),false).await;execute(&source,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);INSERT INTO items VALUES(1,'source')").await;execute(&target,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT)").await;execute(&target,trigger).await;
        let error=synchronize(&source,&target,&options("items","items")).await.unwrap_err();let message=format!("{error:#}");assert!(message.contains("读回")||message.contains("不是唯一命中1行"),"{message}");assert!(message.contains("已确认回滚"));assert_eq!(count(&target,"items").await,0);source.close().await;target.close().await;
    }
}

#[tokio::test]
async fn sqlite_trigger_change_to_unmapped_column_is_not_silently_accepted(){
    let dir=Directory::new();let source=database(&dir.path("source.sqlite"),false).await;let target=database(&dir.path("target.sqlite"),false).await;execute(&source,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);INSERT INTO items VALUES(1,'new')").await;execute(&target,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT,local TEXT);INSERT INTO items VALUES(1,'old','owned');CREATE TRIGGER side_effect AFTER UPDATE OF v ON items BEGIN UPDATE items SET local='changed' WHERE id=NEW.id;END").await;
    let error=synchronize(&source,&target,&options("items","items")).await.unwrap_err();assert!(format!("{error:#}").contains("未映射字段"));let row=sqlx::query("SELECT v,local FROM items WHERE id=1").fetch_one(&target).await.unwrap();assert_eq!(row.get::<String,_>("v"),"old");assert_eq!(row.get::<String,_>("local"),"owned");source.close().await;target.close().await;
}

#[tokio::test]
async fn sqlite_filtered_empty_source_preserves_target_and_legacy_delete_requires_review(){
    let dir=Directory::new();let source=database(&dir.path("source.sqlite"),false).await;let target=database(&dir.path("target.sqlite"),false).await;execute(&source,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);INSERT INTO items VALUES(1,'source')").await;execute(&target,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);INSERT INTO items VALUES(2,'retain')").await;
    let mut opts=options("items","items");opts.where_clause=Some("id<0".into());assert_eq!(synchronize(&source,&target,&opts).await.unwrap().rows,0);assert_eq!(count(&target,"items").await,1);opts.delete_missing=true;let error=synchronize(&source,&target,&opts).await.unwrap_err();assert!(format!("{error:#}").contains("删除目标多余行需要完整范围"));assert_eq!(count(&target,"items").await,1);
    opts.target_table="missing".into();opts.mode="both".into();opts.create_target=true;assert!(synchronize(&source,&target,&opts).await.is_err());assert!(!table_exists(&target,"missing").await);source.close().await;target.close().await;
}

#[tokio::test]
async fn sqlite_bad_scope_or_read_only_source_cannot_execute_where_side_effects(){
    let dir=Directory::new();let source=database(&dir.path("source.sqlite"),false).await;let target=database(&dir.path("target.sqlite"),false).await;execute(&source,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);INSERT INTO items VALUES(1,'source')").await;execute(&target,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT)").await;
    let mut opts=options("items","items");opts.source_schema="temp".into();assert!(synchronize(&source,&target,&opts).await.is_err());opts.source_schema="main".into();opts.where_clause=Some("load_extension('not-an-extension') IS NULL".into());assert!(synchronize(&source,&target,&opts).await.is_err());assert_eq!(count(&target,"items").await,0);assert_eq!(count(&source,"items").await,1);
    opts.where_clause=Some("1); DELETE FROM items; SELECT (1".into());assert!(synchronize(&source,&target,&opts).await.is_err());assert_eq!(count(&source,"items").await,1);source.close().await;target.close().await;
}

#[tokio::test]
async fn sqlite_missing_target_is_created_transactionally_only_when_explicitly_enabled(){
    let dir=Directory::new();let source=database(&dir.path("source.sqlite"),false).await;let target=database(&dir.path("target.sqlite"),false).await;execute(&source,"CREATE TABLE items(a INTEGER,b TEXT,v BLOB,PRIMARY KEY(b,a));INSERT INTO items VALUES(7,'key',X'00FF')").await;
    assert!(synchronize(&source,&target,&options("items","items")).await.is_err());let mut opts=options("items","items");opts.mode="both".into();opts.create_target=true;let report=synchronize(&source,&target,&opts).await.unwrap();assert!(report.created);assert_eq!(report.rows,1);let columns=sqlx::query("PRAGMA table_info(items)").fetch_all(&target).await.unwrap();assert_eq!(columns[0].get::<i64,_>("pk"),2);assert_eq!(columns[1].get::<i64,_>("pk"),1);let row=sqlx::query("SELECT * FROM items").fetch_one(&target).await.unwrap();assert_eq!(row.get::<Vec<u8>,_>("v"),b"\0\xff");assert!(report.warnings.iter().any(|message|message.contains("外键、CHECK、触发器和索引未跨表复制")));source.close().await;target.close().await;
}

#[tokio::test]
async fn sqlite_pre_send_cancel_has_zero_ddl_and_no_resources_left_in_target(){
    let dir=Directory::new();let source=database(&dir.path("source.sqlite"),false).await;let target=database(&dir.path("target.sqlite"),false).await;execute(&source,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT)").await;let mut opts=options("items","missing");opts.mode="both".into();opts.create_target=true;
    let task=TestTask::new();crate::transfer::cancel_task(&task.id).unwrap();let result=sqlite_incremental(&source,&target,&opts,Some(&task.control),"test").await;assert!(result.is_err());task.control.finish(&result);assert_eq!(crate::transfer::task_progress(&task.id).unwrap().status,"cancelled");assert!(!table_exists(&target,"missing").await);execute(&target,"BEGIN IMMEDIATE;ROLLBACK").await;source.close().await;target.close().await;
}

#[tokio::test]
async fn sqlite_cancel_during_cursor_holds_work_until_source_drains_and_target_remains_unchanged(){
    let dir=Directory::new();let source=database(&dir.path("source.sqlite"),false).await;let target=database(&dir.path("target.sqlite"),false).await;execute(&source,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000) INSERT INTO items SELECT x,printf('%08d',x) FROM n").await;execute(&target,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT)").await;
    let task=TestTask::new();let worker={let source=source.clone();let target=target.clone();let control=task.control.clone();tokio::spawn(async move{sqlite_incremental(&source,&target,&options("items","items"),Some(&control),"test").await})};
    if let Err(error)=wait_read(&task,&worker,1).await{let _=crate::transfer::cancel_task(&task.id);let _=worker.await;panic!("{error:#}");}crate::transfer::cancel_task(&task.id).unwrap();let result=worker.await.unwrap();assert!(result.is_err());assert!(format!("{:#}",result.as_ref().unwrap_err()).contains("目标尚未修改"));task.control.finish(&result);assert_eq!(crate::transfer::task_progress(&task.id).unwrap().status,"cancelled");assert_eq!(count(&target,"items").await,0);execute(&source,"BEGIN IMMEDIATE;ROLLBACK").await;execute(&target,"BEGIN IMMEDIATE;ROLLBACK").await;source.close().await;target.close().await;
}

#[tokio::test]
async fn sqlite_active_wal_source_cursor_keeps_a_single_snapshot_across_more_than_eight_mib(){
    const ROWS:i64=4096;let dir=Directory::new();let source=database(&dir.path("source.sqlite"),true).await;let target=database(&dir.path("target.sqlite"),true).await;execute(&source,&format!("CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<{ROWS}) INSERT INTO items SELECT x,printf('%08d-',x)||hex(zeroblob(1024)) FROM n")).await;execute(&target,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT)").await;
    let mut external=SqliteConnection::connect_with(&source.connect_options()).await.unwrap();let task=TestTask::new();let worker={let source=source.clone();let target=target.clone();let control=task.control.clone();tokio::spawn(async move{sqlite_incremental(&source,&target,&options("items","items"),Some(&control),"test").await})};
    if let Err(error)=wait_read(&task,&worker,1).await{let _=crate::transfer::cancel_task(&task.id);let _=worker.await;external.close().await.unwrap();panic!("{error:#}");}
    sqlx::query("UPDATE items SET v='changed after cursor start' WHERE id=?").bind(ROWS).execute(&mut external).await.unwrap();let during=crate::transfer::task_progress(&task.id).unwrap();assert_eq!(during.phase,"read");assert!(during.processed<ROWS as u64,"external commit must happen before the original cursor drains");external.close().await.unwrap();
    let result=worker.await.unwrap();task.control.finish(&result);let report=result.unwrap();assert_eq!(report.rows,ROWS as u64);assert_eq!(count(&target,"items").await,ROWS);let last:String=sqlx::query_scalar("SELECT v FROM items WHERE id=?").bind(ROWS).fetch_one(&target).await.unwrap();assert!(last.starts_with("00004096-"));assert_eq!(last.len(),2057);let bytes:i64=sqlx::query_scalar("SELECT SUM(length(CAST(v AS BLOB))) FROM items").fetch_one(&target).await.unwrap();assert!(bytes>8*1024*1024);assert_eq!(sqlx::query_scalar::<_,String>("SELECT v FROM items WHERE id=?").bind(ROWS).fetch_one(&source).await.unwrap(),"changed after cursor start");source.close().await;target.close().await;
}

#[tokio::test]
async fn sqlite_old_target_baseline_detects_identical_ddl_drop_recreate_while_waiting_for_writer(){
    let dir=Directory::new();let source=database(&dir.path("source.sqlite"),true).await;let target=database(&dir.path("target.sqlite"),true).await;execute(&source,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<200) INSERT INTO items SELECT x,'source' FROM n").await;execute(&target,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);INSERT INTO items VALUES(999,'old baseline')").await;
    let mut blocker=SqliteConnection::connect_with(&target.connect_options()).await.unwrap();sqlx::query("BEGIN IMMEDIATE").execute(&mut blocker).await.unwrap();let task=TestTask::new();let worker={let source=source.clone();let target=target.clone();let control=task.control.clone();tokio::spawn(async move{sqlite_incremental(&source,&target,&options("items","items"),Some(&control),"test").await})};
    if let Err(error)=wait_read(&task,&worker,200).await{let _=sqlx::query("ROLLBACK").execute(&mut blocker).await;let _=crate::transfer::cancel_task(&task.id);let _=worker.await;blocker.close().await.unwrap();panic!("{error:#}");}
    sqlx::query("DROP TABLE items;CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);INSERT INTO items VALUES(777,'external replacement');COMMIT").execute(&mut blocker).await.unwrap();blocker.close().await.unwrap();let result=worker.await.unwrap();task.control.finish(&result);let error=result.unwrap_err();assert!(format!("{error:#}").contains("目标结构CAS冲突"));assert_eq!(count(&target,"items").await,1);let row=sqlx::query("SELECT id,v FROM items").fetch_one(&target).await.unwrap();assert_eq!(row.get::<i64,_>("id"),777);assert_eq!(row.get::<String,_>("v"),"external replacement");source.close().await;target.close().await;
}

#[tokio::test]
async fn sqlite_view_or_virtual_source_is_not_misreported_as_a_verified_table_snapshot(){
    let dir=Directory::new();let source=database(&dir.path("source.sqlite"),false).await;let target=database(&dir.path("target.sqlite"),false).await;execute(&source,"CREATE TABLE base(id INTEGER PRIMARY KEY,v TEXT);INSERT INTO base VALUES(1,'source');CREATE VIEW items AS SELECT * FROM base").await;execute(&target,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT)").await;let error=synchronize(&source,&target,&options("items","items")).await.unwrap_err();assert!(format!("{error:#}").contains("普通表"));assert_eq!(count(&target,"items").await,0);source.close().await;target.close().await;
}

#[tokio::test]
async fn sqlite_later_trigger_cannot_corrupt_an_already_verified_row_before_commit(){
    let dir=Directory::new();let source=database(&dir.path("source.sqlite"),false).await;let target=database(&dir.path("target.sqlite"),false).await;
    execute(&source,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);INSERT INTO items VALUES(1,'first'),(2,'second')").await;
    execute(&target,"CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT);CREATE TRIGGER corrupt_previous AFTER INSERT ON items BEGIN UPDATE items SET v='corrupted by later row' WHERE id<>NEW.id;END").await;
    let error=synchronize(&source,&target,&options("items","items")).await.unwrap_err();let message=format!("{error:#}");assert!(message.contains("最终读回"),"{message}");assert!(message.contains("已确认回滚"),"{message}");assert_eq!(count(&target,"items").await,0);
    source.close().await;target.close().await;
}

#[tokio::test]
async fn sqlite_spill_authentication_rejects_modified_source_and_final_receipt_and_cleans_up(){
    let mut spill=Spill::new().await.unwrap();let path=spill.test_path();let cells=vec![Cell::Integer(i64::MAX),Cell::Blob(vec![0,255,128]),Cell::Text("exact decimal 12345678901234567890.123456789".into()),Cell::Null];let keys=[Cell::Integer(i64::MAX)];
    spill.push_source(&cells,&keys).await.unwrap();spill.seal().await.unwrap();assert_eq!(spill.source(0).await.unwrap().0,cells);
    spill.push_verified(&keys,&cells).await.unwrap();spill.verify_receipts().await.unwrap();assert_eq!(spill.verified(0).await.unwrap().1,cells);
    let mut external=SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path).create_if_missing(false)).await.unwrap();
    sqlx::query("UPDATE source_rows SET body=X'00' WHERE seq=0;UPDATE verified_rows SET body=X'00' WHERE seq=0").execute(&mut external).await.unwrap();external.close().await.unwrap();
    assert!(format!("{:#}",spill.source(0).await.unwrap_err()).contains("认证失败"));assert!(format!("{:#}",spill.verify_receipts().await.unwrap_err()).contains("认证失败"));spill.close().await.unwrap();assert!(!path.exists());
}

#[tokio::test]
async fn sqlite_spill_budget_is_checked_before_writing_and_does_not_require_an_eight_gib_allocation(){
    let mut spill=Spill::new().await.unwrap();let path=spill.test_path();let remaining=usize::try_from(spill::SPILL_BYTES-spill.test_bytes()-1024).unwrap();spill.test_charge(remaining).unwrap();assert_eq!(spill.test_bytes(),spill::SPILL_BYTES);
    let error=spill.push_source(&[Cell::Integer(1)],&[Cell::Integer(1)]).await.unwrap_err();assert!(format!("{error:#}").contains("8GiB预算"));assert_eq!(spill.rows,0);
    let metadata=std::fs::metadata(&path).unwrap();assert!(metadata.len()<1024*1024,"budget regression must not actually allocate the budget");spill.close().await.unwrap();assert!(!path.exists());
}
