//! 真实 SQLite 产品备份路径回归，不通过测试替身读取数据。
use anyhow::Result;
use navkit_lib::{db::Db, models::{DbKind, DumpOptions}, transfer};
use sqlx::Row;

struct Directory(std::path::PathBuf);
impl Directory {
    fn new() -> Self {
        let path=std::env::temp_dir().join(format!("navkit-backup-regression-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap(); Self(path)
    }
    fn file(&self,name:&str)->String {self.0.join(name).to_string_lossy().into_owned()}
    fn no_stage(&self) {assert!(!std::fs::read_dir(&self.0).unwrap().any(|entry|entry.unwrap().file_name().to_string_lossy().starts_with(".navkit-")));}
}
impl Drop for Directory {fn drop(&mut self){let _=std::fs::remove_dir_all(&self.0);}}
async fn pool()->sqlx::SqlitePool {sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap()}

#[tokio::test]
async fn sqlite_large_dump_database_roundtrips_types_views_and_order_without_clobber()->Result<()> {
    let dir=Directory::new();let source=pool().await;let target=pool().await;
    sqlx::raw_sql("CREATE TABLE items(id INTEGER PRIMARY KEY, amount TEXT, note TEXT, payload BLOB);\
        WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<50003)\
        INSERT INTO items SELECT x,'123456789012345678901234567890.12345678901234567890',\
        CASE WHEN x%3=0 THEN NULL ELSE '中文,quote''line' END,X'00FF8027' FROM n;\
        CREATE TABLE edge(id INTEGER,note TEXT,payload BLOB);\
        INSERT INTO edge VALUES(9223372036854775807,CAST(X'610062' AS TEXT),X''),(-9223372036854775808,'',X'00FF80');\
        CREATE TABLE z_empty(id INTEGER);CREATE VIEW a_view AS SELECT * FROM items;")
        .execute(&source).await?;
    let db=Db::Sqlite(source.clone());let kind=DbKind::of("sqlite");let file=dir.file("database.sql");
    let (rows,bytes)=transfer::dump_database(&db,&kind,"main",&[],&[],true,true,&file,"typed roundtrip").await?;
    assert_eq!(rows,50005);assert_eq!(bytes,std::fs::metadata(&file)?.len());
    let text=std::fs::read_to_string(&file)?;
    assert!(text.find("CREATE TABLE items").unwrap()<text.find("CREATE VIEW a_view").unwrap());
    assert!(!text.contains("INSERT INTO \"a_view\""));
    transfer::restore_sql_file(&Db::Sqlite(target.clone()),&kind,&file).await?;
    let row=sqlx::query("SELECT COUNT(*) AS total,SUM(note IS NULL) AS nulls,SUM(typeof(payload)='blob' AND hex(payload)='00FF8027') AS binary,SUM(amount='123456789012345678901234567890.12345678901234567890') AS exact FROM items").fetch_one(&target).await?;
    assert_eq!(row.try_get::<i64,_>("total")?,50003);assert_eq!(row.try_get::<i64,_>("nulls")?,16667);assert_eq!(row.try_get::<i64,_>("binary")?,50003);assert_eq!(row.try_get::<i64,_>("exact")?,50003);
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM edge WHERE id=9223372036854775807 AND hex(note)='610062' AND typeof(note)='text' AND typeof(payload)='blob' AND length(payload)=0").fetch_one(&target).await?,1);
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM a_view").fetch_one(&target).await?,50003);
    assert!(transfer::dump_database(&db,&kind,"main",&[],&[],true,true,&file,"").await.is_err());assert_eq!(std::fs::read_to_string(&file)?,text);dir.no_stage();
    target.close().await;source.close().await;Ok(())
}

#[tokio::test]
async fn sqlite_backup_export_cursor_cancellation_never_publishes_partial_file()->Result<()> {
    let dir=Directory::new();let source=pool().await;
    sqlx::raw_sql("CREATE TABLE items(id INTEGER,note TEXT);WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000) INSERT INTO items SELECT x,'cancel boundary' FROM n").execute(&source).await?;
    let db=Db::Sqlite(source.clone());let kind=DbKind::of("sqlite");let file=dir.file("cancel.sql");let id=format!("backup_cancel_{}",uuid::Uuid::new_v4().simple());
    let options=transfer::ExportTaskOptions {sql_batch_rows:0,sql_batch_bytes:0,task_id:id.clone(),options:navkit_lib::models::ExportOptions {connection_id:"fixture".into(),schema:"main".into(),table:"items".into(),file:file.clone(),format:"sql".into(),encoding:"utf8".into(),delimiter:",".into(),has_header:true,where_clause:None,limit:None}};
    let cancel=async {
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);
        loop {
            if transfer::task_progress(&id).is_ok_and(|p|p.processed>=100) {transfer::cancel_task(&id).unwrap();break;}
            assert!(std::time::Instant::now()<deadline,"等待真实游标进度超时");tokio::task::yield_now().await;
        }
    };
    let (result,())=tokio::join!(transfer::run_export_task(&db,&kind,&options),cancel);
    assert!(format!("{:#}",result.unwrap_err()).contains("取消"));assert_eq!(transfer::task_progress(&id)?.status,"cancelled");
    assert!(!std::path::Path::new(&file).exists());dir.no_stage();
    // 失败会话必须关闭，不将未排空的流放回池；池仍可建立干净连接。
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT 1").fetch_one(&source).await?,1);source.close().await;Ok(())
}

#[tokio::test]
async fn sqlite_single_dump_options_and_serialized_record_budget_remain_compatible()->Result<()> {
    let dir=Directory::new();let source=pool().await;let target=pool().await;
    sqlx::raw_sql("CREATE TABLE items(id INTEGER,payload BLOB,note TEXT);INSERT INTO items VALUES(1,X'00FF80','quote''line'),(2,X'',NULL)").execute(&source).await?;
    let db=Db::Sqlite(source.clone());let kind=DbKind::of("sqlite");
    let mut opts=DumpOptions{connection_id:"fixture".into(),schema:"main".into(),table:"items".into(),file:dir.file("single.sql"),structure_only:false,data_only:false,include_drop:true};
    assert_eq!(transfer::dump(&db,&kind,&opts).await?,2);
    transfer::restore_sql_file(&Db::Sqlite(target.clone()),&kind,&opts.file).await?;
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM items WHERE typeof(payload)='blob'").fetch_one(&target).await?,2);
    opts.file=dir.file("structure.sql");opts.structure_only=true;assert_eq!(transfer::dump(&db,&kind,&opts).await?,0);assert!(!std::fs::read_to_string(&opts.file)?.contains("INSERT INTO"));
    opts.file=dir.file("data.sql");opts.structure_only=false;opts.data_only=true;assert_eq!(transfer::dump(&db,&kind,&opts).await?,2);assert!(!std::fs::read_to_string(&opts.file)?.contains("CREATE TABLE"));
    // 输入小于16MiB但引号转义后大于16MiB，必须失败且清理stage，不能静默截断。
    sqlx::query("INSERT INTO items VALUES(3,X'00',?)").bind("'".repeat(9*1024*1024)).execute(&source).await?;
    opts.file=dir.file("too-large.sql");let error=transfer::dump(&db,&kind,&opts).await.unwrap_err();assert!(format!("{error:#}").contains("超过上限"));assert!(!std::path::Path::new(&opts.file).exists());dir.no_stage();
    source.close().await;target.close().await;Ok(())
}
