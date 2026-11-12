use navkit_lib::{db::Db, models::{DbKind, ImportOptions, ExportOptions}, transfer::{self, ImportTaskOptions, ExportTaskOptions}, sync::{self, SyncOptions, SyncColumnMapping}};
use sqlx::{Row, sqlite::{SqliteConnectOptions, SqlitePoolOptions}};
use std::path::Path;

async fn sqlite(path: &Path) -> Db {
    Db::Sqlite(SqlitePoolOptions::new().max_connections(3).connect_with(SqliteConnectOptions::new().filename(path).create_if_missing(true)).await.unwrap())
}
async fn exec(db: &Db, sql: &str) { if let Db::Sqlite(p) = db { sqlx::query(sql).execute(p).await.unwrap(); } }
async fn rows(db: &Db, table: &str) -> Vec<(i64, Option<String>, Vec<u8>)> {
    if let Db::Sqlite(p) = db { return sqlx::query(&format!("SELECT id,name,payload FROM \"{}\" ORDER BY id",table)).fetch_all(p).await.unwrap().iter().map(|r| (r.get(0),r.get(1),r.get(2))).collect(); } unreachable!()
}
fn import(file: &Path, table: &str, format: &str) -> ImportTaskOptions {
    ImportTaskOptions { task_id: uuid::Uuid::new_v4().to_string(), column_mapping: vec![], options: ImportOptions { connection_id: "target".into(), schema: "main".into(), table: table.into(), file: file.to_string_lossy().into(), format: format.into(), has_header: true, delimiter: ",".into(), encoding: "utf-8".into(), create_table: false, truncate: false, sheet: None, on_conflict: "error".into(), conflict_columns: Vec::new() } }
}
fn export(file: &Path, table: &str, format: &str) -> ExportTaskOptions {
    ExportTaskOptions { task_id: uuid::Uuid::new_v4().to_string(), options: ExportOptions { connection_id: "source".into(), schema: "main".into(), table: table.into(), file: file.to_string_lossy().into(), format: format.into(), has_header: true, delimiter: ",".into(), encoding: "utf-8".into(), where_clause: None, limit: None } }
}
fn sync_opts(table: &str, mode: &str) -> SyncOptions {
    SyncOptions { task_id: Some(uuid::Uuid::new_v4().to_string()), source_connection:"source".into(), source_schema:"main".into(), source_table:"items".into(), target_connection:"target".into(), target_schema:"main".into(), target_table:table.into(), mode:mode.into(), write_mode:"append".into(), sync_mode:"full".into(), incremental_update:false, create_target:true, columns:vec![], key_columns: Vec::new(), delete_missing: false, where_clause:None }
}
#[tokio::test]
async fn isolated_sqlite_transfer_and_sync_end_to_end() {
    let root = std::env::temp_dir().join(format!("navkit-transfer-{}", uuid::Uuid::new_v4())); std::fs::create_dir(&root).unwrap();
    let source = sqlite(&root.join("source.sqlite")).await; let target = sqlite(&root.join("target.sqlite")).await; let kind = DbKind::of("sqlite");
    exec(&source, "CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT DEFAULT 'fallback', payload BLOB NOT NULL)").await;
    exec(&source, "CREATE UNIQUE INDEX uq_items_name ON items(name)").await;
    exec(&source, "INSERT INTO items VALUES (1,'逗号,引号''换行\ntext',X'00FF4142'),(2,NULL,X'4142'),(3,'',X''),(4,'\\N',X'0123')").await;
    let expected = rows(&source,"items").await;
    for format in ["csv", "json"] {
        let file = root.join(format!("items.{}",format)); let out = export(&file,"items",format);
        assert_eq!(transfer::run_export_task(&source,&kind,&out).await.unwrap(),4);
        assert_eq!(transfer::task_progress(&out.task_id).unwrap().status,"completed");
        exec(&target,&format!("CREATE TABLE {}(id INTEGER PRIMARY KEY,name TEXT,payload BLOB NOT NULL)",format)).await;
        let input = import(&file,format,format);
        assert_eq!(transfer::run_import_task(&target,&kind,&input).await.unwrap(),4);
        assert_eq!(rows(&target,format).await,expected);
        // 审定行为：已存在的导出目标必须拒绝覆盖；失败编码同样不得改动既有文件。
        let before = std::fs::read(&file).unwrap();
        let existing = export(&file,"items",format);
        assert!(transfer::run_export_task(&source,&kind,&existing).await.is_err(), "已存在的导出目标必须拒绝覆盖");
        assert_eq!(std::fs::read(&file).unwrap(),before, "拒绝覆盖不得改动既有文件");
        let fresh = root.join(format!("retry.{}",format));
        let mut bad = export(&fresh,"items",format); bad.options.encoding="latin1".into();
        let result = transfer::run_export_task(&source,&kind,&bad).await;
        assert!(result.is_err() || !fresh.exists(), "失败编码不得留下目标文件");
    }
    // No-header mode must preserve the first row; unmatched target columns use DEFAULT.
    let noheader=root.join("noheader.csv"); std::fs::write(&noheader,"10,first\r\n11,second\r\n").unwrap();
    exec(&target,"CREATE TABLE positional(id INTEGER,name TEXT,extra TEXT DEFAULT 'default')").await;
    let mut opts=import(&noheader,"positional","csv"); opts.options.has_header=false;
    assert_eq!(transfer::run_import_task(&target,&kind,&opts).await.unwrap(),2);
    if let Db::Sqlite(p)=&target { let r=sqlx::query("SELECT name,extra FROM positional WHERE id=10").fetch_one(p).await.unwrap(); assert_eq!(r.get::<String,_>(0),"first"); assert_eq!(r.get::<String,_>(1),"default"); }
    let createfile=root.join("new.csv"); std::fs::write(&createfile,"a,b\n1,first\n2,second\n").unwrap(); let mut create=import(&createfile,"created","csv"); create.options.create_table=true;
    assert_eq!(transfer::run_import_task(&target,&kind,&create).await.unwrap(),2);
    let mut replace=create.clone(); replace.task_id=uuid::Uuid::new_v4().to_string(); replace.options.truncate=true;
    assert_eq!(transfer::run_import_task(&target,&kind,&replace).await.unwrap(),2);
    // Truncate + failed insert is fully rolled back (including deleted rows).
    let badfile=root.join("bad.csv"); std::fs::write(&badfile,"id,name,payload\n5,new,0x00\n5,duplicate,0xFF\n").unwrap();
    let mut bad=import(&badfile,"csv","csv"); bad.options.truncate=true; assert!(transfer::run_import_task(&target,&kind,&bad).await.is_err()); assert_eq!(rows(&target,"csv").await,expected);
    let malformed=root.join("malformed.csv"); std::fs::write(&malformed,"id,name\n1,a,b\n").unwrap(); assert!(transfer::run_import_task(&target,&kind,&import(&malformed,"csv","csv")).await.is_err());
    // Empty exports retain headers.
    exec(&source,"CREATE TABLE empty(id INTEGER, name TEXT)").await; let empty=root.join("empty.csv"); assert_eq!(transfer::run_export_task(&source,&kind,&export(&empty,"empty","csv")).await.unwrap(),0); assert_eq!(std::fs::read_to_string(empty).unwrap(),"id,name\r\n");
    // SQL export/import uses the target dialect and transaction, not one pool query per statement.
    let sqlfile=root.join("items.sql"); assert_eq!(transfer::run_export_task(&source,&kind,&export(&sqlfile,"items","sql")).await.unwrap(),4);
    exec(&target,"CREATE TABLE items(id INTEGER PRIMARY KEY,name TEXT,payload BLOB NOT NULL)").await;
    assert_eq!(transfer::run_import_task(&target,&kind,&import(&sqlfile,"items","sql")).await.unwrap(),4);
    assert_eq!(rows(&target,"items").await,expected);
    let structure=sync_opts("structure_only","structure"); let report=sync::synchronize(&source,&kind,&target,&kind,&structure).await.unwrap(); assert_eq!(report.rows,0); assert!(report.created); assert!(rows(&target,"structure_only").await.is_empty());
    let data=sync_opts("structure_only","data"); assert_eq!(sync::synchronize(&source,&kind,&target,&kind,&data).await.unwrap().rows,4); assert_eq!(rows(&target,"structure_only").await,expected);
    let both=sync_opts("both","both"); assert_eq!(sync::synchronize(&source,&kind,&target,&kind,&both).await.unwrap().rows,4); assert_eq!(rows(&target,"both").await,expected);
    let mut mapped=sync_opts("mapped","both"); mapped.columns=vec![SyncColumnMapping{source:"id".into(),target:"key".into(),target_type:Some("BIGINT".into())},SyncColumnMapping{source:"name".into(),target:"title".into(),target_type:Some("TEXT".into())}]; assert_eq!(sync::synchronize(&source,&kind,&target,&kind,&mapped).await.unwrap().rows,4);
    let mut unsafe_type=sync_opts("never_created","both"); unsafe_type.columns=vec![SyncColumnMapping{source:"id".into(),target:"id".into(),target_type:Some("TEXT); DROP TABLE both;--".into())}]; assert!(sync::synchronize(&source,&kind,&target,&kind,&unsafe_type).await.is_err());
    let mut unsafe_where=sync_opts("both","data"); unsafe_where.where_clause=Some("1=1; DROP TABLE items".into()); unsafe_where.write_mode="replace".into(); assert!(sync::synchronize(&source,&kind,&target,&kind,&unsafe_where).await.is_err()); assert_eq!(rows(&target,"both").await,expected);
    let mut invalid=sync_opts("both","invalid"); assert!(sync::synchronize(&source,&kind,&target,&kind,&invalid).await.is_err()); invalid.mode="data".into(); invalid.write_mode="recreate".into(); assert!(sync::synchronize(&source,&kind,&target,&kind,&invalid).await.is_err());
    let mut self_copy=sync_opts("items","both"); self_copy.target_connection="another-alias".into(); assert!(sync::synchronize(&source,&kind,&source,&kind,&self_copy).await.is_err());
    let overwrite=export(&root.join("source.sqlite"),"items","csv"); assert!(transfer::run_export_task(&source,&kind,&overwrite).await.is_err()); assert_eq!(rows(&source,"items").await,expected);
    // Cancellation requested during a write rolls back the whole target transaction.
    let taskid=uuid::Uuid::new_v4().to_string(); let task=transfer::TaskControl::begin(&taskid).unwrap(); transfer::cancel_task(&taskid).unwrap();
    assert!(transfer::execute_atomic(&target,&[("DELETE FROM both".into(),false)],Some(&task)).await.is_err()); assert_eq!(rows(&target,"both").await,expected);
    // Actual cooperative cancellation after the transaction starts.
    let taskid=uuid::Uuid::new_v4().to_string(); let task=transfer::TaskControl::begin(&taskid).unwrap();
    let statements=(0..10000).map(|i|(format!("INSERT INTO positional VALUES ({},'cancel','x')",i+1000),true)).collect::<Vec<_>>();
    let writer=transfer::execute_atomic(&target,&statements,Some(&task)); let cancel=async { tokio::time::sleep(std::time::Duration::from_millis(5)).await; transfer::cancel_task(&taskid).unwrap(); };
    let (result,_)=tokio::join!(writer,cancel); assert!(result.is_err());
    if let Db::Sqlite(p)=&target { assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM positional").fetch_one(p).await.unwrap(),2); }
    assert_eq!(sync::map_type("BYTEA",&DbKind::of("mysql")),"LONGBLOB"); assert_eq!(sync::map_type("DATETIME",&DbKind::of("postgresql")),"TIMESTAMP"); assert!(sync::validate_target_type("VARCHAR(0)",&kind).is_err());
    if let Db::Sqlite(p)=source { p.close().await; } if let Db::Sqlite(p)=target { p.close().await; }
    std::fs::remove_dir_all(&root).unwrap();
}

#[tokio::test]
#[ignore = "requires explicit NAVKIT_MYSQL_PASSWORD and live MySQL/PostgreSQL; connection failures are failures"]
async fn isolated_live_mysql_pg_heterogeneous_sync() {
    use sqlx::{mysql::MySqlConnectOptions, postgres::PgConnectOptions};
    let password = std::env::var("NAVKIT_MYSQL_PASSWORD").expect("NAVKIT_MYSQL_PASSWORD must explicitly configure the live fixture");
    let mysql = sqlx::mysql::MySqlPoolOptions::new().max_connections(3).after_connect(|conn,_| Box::pin(async move {
        sqlx::query("SET sql_mode=CONCAT(@@session.sql_mode, ',NO_BACKSLASH_ESCAPES')").execute(conn).await?; Ok(())
    })).connect_with(MySqlConnectOptions::new()
        .host(&std::env::var("NAVKIT_MYSQL_HOST").unwrap_or_else(|_| "127.0.0.1".into()))
        .port(std::env::var("NAVKIT_MYSQL_PORT").unwrap_or_else(|_| "3306".into()).parse().expect("MySQL port"))
        .username(&std::env::var("NAVKIT_MYSQL_USER").unwrap_or_else(|_| "root".into()))
        .password(&password)).await.expect("live MySQL must be available");
    let pg = sqlx::postgres::PgPoolOptions::new().max_connections(3).connect_with(PgConnectOptions::new()
        .host(&std::env::var("NAVKIT_PG_HOST").unwrap_or_else(|_| "127.0.0.1".into()))
        .port(std::env::var("NAVKIT_PG_PORT").unwrap_or_else(|_| "5433".into()).parse().expect("PostgreSQL port"))
        .username(&std::env::var("NAVKIT_PG_USER").unwrap_or_else(|_| "postgres".into()))
        .password(&std::env::var("NAVKIT_PG_PASSWORD").unwrap_or_default())
        .database(&std::env::var("NAVKIT_PG_DATABASE").unwrap_or_else(|_| "postgres".into())))
        .await.expect("live PostgreSQL must be available");
    let suffix=uuid::Uuid::new_v4().simple().to_string(); let namespace=format!("navkit_sync_{}",suffix);
    // Never touch existing acceptance/product tables: unique namespace created for this run.
    sqlx::query(&format!("CREATE DATABASE `{}`",namespace)).execute(&mysql).await.unwrap();
    sqlx::query(&format!("CREATE SCHEMA \"{}\"",namespace)).execute(&pg).await.unwrap();
    let root=std::env::temp_dir().join(format!("navkit-cross-{}",suffix)); std::fs::create_dir(&root).unwrap(); let sqlite=sqlite(&root.join("source.sqlite")).await;
    exec(&sqlite,"CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT, payload BLOB NOT NULL, active BOOLEAN)").await;
    exec(&sqlite,"INSERT INTO items VALUES(1,'异构,quote''',X'00FF4142',1),(2,NULL,X'4142',0)").await;
    let source_kind=DbKind::of("sqlite"); let my_kind=DbKind::of("mysql"); let pg_kind=DbKind::of("postgresql"); let mydb=Db::Mysql(mysql.clone()); let pgdb=Db::Postgres(pg.clone());
    let result: anyhow::Result<()> = async {
        let mut options=sync_opts("from_sqlite","both"); options.target_schema=namespace.clone();
        anyhow::ensure!(sync::synchronize(&sqlite,&source_kind,&mydb,&my_kind,&options).await?.rows==2,"SQLite to MySQL count mismatch");
        options.task_id=Some(uuid::Uuid::new_v4().to_string()); options.target_table="structure_only".into(); options.mode="structure".into(); anyhow::ensure!(sync::synchronize(&sqlite,&source_kind,&pgdb,&pg_kind,&options).await?.rows==0,"structure-only wrote data");
        options.task_id=Some(uuid::Uuid::new_v4().to_string()); options.mode="data".into(); anyhow::ensure!(sync::synchronize(&sqlite,&source_kind,&pgdb,&pg_kind,&options).await?.rows==2,"data-only count mismatch");
        let mut cross=sync_opts("from_mysql","both"); cross.source_schema=namespace.clone(); cross.source_table="from_sqlite".into(); cross.target_schema=namespace.clone();
        anyhow::ensure!(sync::synchronize(&mydb,&my_kind,&pgdb,&pg_kind,&cross).await?.rows==2,"MySQL to PostgreSQL count mismatch");
        let (names,a)=transfer::fetch_transfer_rows(&sqlite,&source_kind,"main","items",None,None).await?; let (_,b)=transfer::fetch_transfer_rows(&pgdb,&pg_kind,&namespace,"from_mysql",None,None).await?;
        anyhow::ensure!(a==b&&names[2]=="payload","all live readback rows/cells must match, including NULL/binary/boolean: source={a:?},target={b:?}");
        for (db,kind,table) in [(&mydb,&my_kind,"from_sqlite"),(&pgdb,&pg_kind,"from_mysql")] {
            let file=root.join(format!("{}.json",kind.id())); let mut out=export(&file,table,"json"); out.options.schema=namespace.clone(); anyhow::ensure!(transfer::run_export_task(db,kind,&out).await?==2,"live export count mismatch");
            let mut input=import(&file,table,"json"); input.options.schema=namespace.clone(); input.options.truncate=true; anyhow::ensure!(transfer::run_import_task(db,kind,&input).await?==2,"live import count mismatch");
        }
        Ok(())
    }.await;
    // Always clean unique namespaces even if an assertion-equivalent operation errors.
    sqlx::query(&format!("DROP SCHEMA \"{}\" CASCADE",namespace)).execute(&pg).await.unwrap();
    sqlx::query(&format!("DROP DATABASE `{}`",namespace)).execute(&mysql).await.unwrap();
    if let Db::Sqlite(p)=sqlite {p.close().await;} mysql.close().await; pg.close().await; std::fs::remove_dir_all(root).unwrap();
    result.unwrap();
}
