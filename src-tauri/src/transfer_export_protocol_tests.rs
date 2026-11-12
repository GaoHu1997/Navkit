//! 真实 SQLx/Tiberius 驱动 + 本机 wire 模拟。不是厂商数据库验收。
use super::*;
use std::{path::{Path, PathBuf}, process::{Child, Command, Stdio}, time::{Duration, Instant}};

struct Fixture { directory: PathBuf, child: Child, port: u16 }
impl Fixture {
    async fn start(protocol: &str) -> Self {
        let directory=std::env::temp_dir().join(format!("navkit-export-simulation-{}",uuid::Uuid::new_v4()));std::fs::create_dir(&directory).unwrap();
        let root=Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let mut command=Command::new("python");
        command.arg(root.join("tools/tests/database-export-protocol-fixture.py")).arg(&directory).arg(protocol)
            .env("NAVKIT_TDS_USER","fixture").env("NAVKIT_TDS_PASSWORD","fixture").env("NAVKIT_TDS_DATABASE","catalog.with.dot")
            .stdout(Stdio::null()).stderr(Stdio::inherit());
        #[cfg(windows)] { use std::os::windows::process::CommandExt; command.creation_flags(0x08000000); }
        let mut child=command.spawn().expect("必须能够启动明确的本机协议模拟；不会跳过或连接用户数据库");
        let deadline=Instant::now()+Duration::from_secs(10);
        let port=loop {
            if let Ok(bytes)=std::fs::read(directory.join("ready.json")) { if let Ok(value)=serde_json::from_slice::<serde_json::Value>(&bytes) { break value["port"].as_u64().unwrap() as u16; } }
            assert!(child.try_wait().unwrap().is_none(),"协议模拟启动失败，不当作厂商通过");assert!(Instant::now()<deadline,"协议模拟启动超时");tokio::time::sleep(Duration::from_millis(10)).await;
        };
        Self {directory,child,port}
    }
    fn file(&self,name:&str)->String {self.directory.join(name).to_string_lossy().into_owned()}
    fn wire(&self)->Vec<serde_json::Value> {std::fs::read_to_string(self.directory.join("wire.jsonl")).unwrap_or_default().lines().map(|line|serde_json::from_str(line).unwrap()).collect()}
    fn clean_stages(&self) {assert!(!std::fs::read_dir(&self.directory).unwrap().any(|entry|entry.unwrap().file_name().to_string_lossy().starts_with(".navkit-export-")));}
}
impl Drop for Fixture {fn drop(&mut self){let _=self.child.kill();let _=self.child.wait();let _=std::fs::remove_dir_all(&self.directory);}}
fn options(fixture:&Fixture,namespace:&str,table:&str,name:&str,format:&str,limit:Option<u64>)->crate::transfer::ExportTaskOptions {
    crate::transfer::ExportTaskOptions { task_id:uuid::Uuid::new_v4().to_string(),options:ExportOptions {connection_id:"isolated-protocol-simulation".into(),schema:namespace.into(),table:table.into(),file:fixture.file(name),format:format.into(),delimiter:",".into(),encoding:"utf8".into(),has_header:true,where_clause:None,limit} }
}
fn data_select(row:&serde_json::Value)->bool {let sql=row["sql"].as_str().unwrap_or("").to_ascii_lowercase();(row["event"]=="EXECUTE"||row["event"]=="QUERY"||row["event"]=="SQL")&&(sql.starts_with("select * from ")||sql.starts_with("select top ("))}
fn dump_options(fixture:&Fixture,namespace:&str,table:&str)->crate::models::DumpOptions {
    crate::models::DumpOptions {connection_id:"protocol-simulation".into(),schema:namespace.into(),table:table.into(),file:fixture.file("legacy-view.sql"),structure_only:false,data_only:false,include_drop:true}
}

async fn cancel_after_rows(task_id:&str) {
    let deadline=Instant::now()+Duration::from_secs(10);
    loop {
        if crate::transfer::task_progress(task_id).is_ok_and(|progress|progress.phase=="export"&&progress.processed>0) {crate::transfer::cancel_task(task_id).unwrap();return;}
        assert!(Instant::now()<deadline,"必须等到真实游标行进度再取消");tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn mysql_wire_simulation_exports_views_without_weakening_innodb_and_keeps_one_data_select() {
    let fixture=Fixture::start("mysql").await;
    let pool=sqlx::mysql::MySqlPoolOptions::new().max_connections(1).connect_with(sqlx::mysql::MySqlConnectOptions::new().host("127.0.0.1").port(fixture.port).username("fixture").password("fixture").database("fixture").ssl_mode(sqlx::mysql::MySqlSslMode::Disabled)).await.unwrap();
    let db=Db::Mysql(pool.clone());let kind=DbKind::of("mysql");
    for (table,format,count) in [("base","csv",2),("selected_view","json",2),("empty_view","csv",0),("selected_view","sql",0),("base","sql",2)] {
        let before=fixture.wire().len();let request=options(&fixture,"fixture",table,&format!("{table}.{format}"),format,None);
        assert_eq!(crate::transfer::run_export_task(&db,&kind,&request).await.unwrap(),count);
        let text=std::fs::read_to_string(&request.options.file).unwrap();
        if table=="selected_view"&&format=="sql" {assert!(text.contains("CREATE VIEW `fixture`.`selected_view`"));assert!(!text.contains("INSERT INTO"));}
        if table=="empty_view" {assert!(text.starts_with("id,payload,nullable,note\r\n"));}
        if format=="json" {let rows:serde_json::Value=serde_json::from_str(&text).unwrap();assert_eq!(rows[0]["id"],"9223372036854775807");assert_eq!(rows[0]["payload"],"0x00FF8027");assert!(rows[0]["nullable"].is_null());assert_eq!(rows[1]["nullable"],"");}
        if table=="base"&&format=="sql" {assert!(text.contains("X'00FF8027'"));}
        let wire=fixture.wire();let data:Vec<_>=wire[before..].iter().filter(|row|data_select(row)).collect();
        assert_eq!(data.len(),if table=="selected_view"&&format=="sql" {0}else{1});
        for select in data {let session=&select["session"];assert!(wire.iter().any(|row|row["event"]=="PREPARE"&&row["sql"]==select["sql"]&&row["session"]==*session));assert!(wire[before..].iter().filter(|row|row["event"]=="EXECUTE"||row["event"]=="PREPARE").all(|row|row["session"]==*session));}
    }
    let before=fixture.wire().len();let mut dump=dump_options(&fixture,"fixture","selected_view");assert_eq!(crate::transfer::dump(&db,&kind,&dump).await.unwrap(),0);
    let sql=std::fs::read_to_string(&dump.file).unwrap();assert!(sql.contains("DROP VIEW IF EXISTS `fixture`.`selected_view`;")&&sql.contains("CREATE VIEW"));assert!(!sql.contains("INSERT INTO")&&!fixture.wire()[before..].iter().any(data_select));
    assert!(crate::transfer::dump(&db,&kind,&dump).await.is_err());assert_eq!(std::fs::read_to_string(&dump.file).unwrap(),sql);
    dump.data_only=true;dump.file=fixture.file("view-data-only.sql");assert!(crate::transfer::dump(&db,&kind,&dump).await.is_err());assert!(!Path::new(&dump.file).exists());
    let mut data_dump=dump_options(&fixture,"fixture","base");data_dump.data_only=true;data_dump.file=fixture.file("legacy-data.sql");
    let before=fixture.wire().len();assert_eq!(crate::transfer::dump(&db,&kind,&data_dump).await.unwrap(),2);let sql=std::fs::read_to_string(&data_dump.file).unwrap();assert!(sql.contains("X'00FF8027'"));assert_eq!(fixture.wire()[before..].iter().filter(|row|data_select(row)).count(),1);
    let before=fixture.wire().len();let file=fixture.file("database-data.sql");let (rows,bytes)=crate::transfer::dump_database(&db,&kind,"fixture",&["base".into()],&[],false,true,&file,"").await.unwrap();assert_eq!(rows,2);assert_eq!(bytes,std::fs::metadata(&file).unwrap().len());assert_eq!(fixture.wire()[before..].iter().filter(|row|data_select(row)).count(),1);
    for table in ["unsafe","mixed_view"] {
        let before=fixture.wire().len();let request=options(&fixture,"fixture",table,&format!("{table}.csv"),"csv",None);
        let error=crate::transfer::run_export_task(&db,&kind,&request).await.unwrap_err();assert!(format!("{error:#}").contains("InnoDB"));assert!(!Path::new(&request.options.file).exists());assert!(!fixture.wire()[before..].iter().any(data_select));
    }
    for (table,message) in [("hidden_view","SHOW VIEW"),("hidden_dependency_view","依赖") ] {
        let before=fixture.wire().len();let request=options(&fixture,"fixture",table,&format!("{table}.csv"),"csv",None);
        let error=crate::transfer::run_export_task(&db,&kind,&request).await.unwrap_err();assert!(format!("{error:#}").contains(message));assert!(!Path::new(&request.options.file).exists());assert!(!fixture.wire()[before..].iter().any(data_select));
    }
    let request=options(&fixture,"fixture","slow","cancelled.csv","csv",None);
    let (result,())=tokio::join!(crate::transfer::run_export_task(&db,&kind,&request),cancel_after_rows(&request.task_id));
    assert!(result.is_err());assert!(!Path::new(&request.options.file).exists());assert_eq!(crate::transfer::task_progress(&request.task_id).unwrap().status,"cancelled");
    // 查询恢复必须取得新连接；取消的游标不允许回池复用。
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT DATABASE()").fetch_one(&pool).await.unwrap(),"fixture");
    assert!(fixture.wire().iter().filter(|row|row["event"]=="LOGIN").count()>=2);
    // 事务控制语句不在 MySQL 预处理白名单：必须以 COM_QUERY 发送，否则服务端返回 1295。
    let wire=fixture.wire();
    assert!(!wire.iter().any(|row|row["event"]=="PREPARE_REJECTED"),"事务/表锁控制语句不得走预处理协议");
    assert!(wire.iter().any(|row|row["event"]=="QUERY"&&row["sql"].as_str().is_some_and(|sql|sql.starts_with("START TRANSACTION"))),"START TRANSACTION 必须以 COM_QUERY 发送");
    fixture.clean_stages();pool.close().await;
    eprintln!("MySQL wire simulation 已验证真实 SQLx describe/stream、VIEW ENGINE=NULL、InnoDB 门禁、事务控制走文本协议；厂商 MySQL NOT_RUN，无默认 3306 连接");
}

#[tokio::test]
async fn tds_wire_simulation_preserves_fixed_session_metadata_top_binary_rowversion_and_view_ddl() {
    let fixture=Fixture::start("tds").await;
    let request:crate::models::ConnectionRequest=serde_json::from_value(serde_json::json!({"kind":"sqlserver","host":"127.0.0.1","port":fixture.port,"user":"fixture","password":"fixture","database":"catalog.with.dot","file":"","params":"encrypt=false","timeout_secs":10})).unwrap();
    let database=crate::mssql::MssqlDb::connect(DbKind::of("sqlserver"),&request).await.unwrap();let db=Db::Mssql(database);let kind=DbKind::of("sqlserver");
    for (table,format,limit,count,name) in [("[sales].[order]]detail]","csv",None,2,"table.csv"),("[sales].[order]]detail]","json",None,2,"table.json"),("[sales].[order]]detail]","sql",None,2,"table.sql"),("[sales].[order]]detail]","csv",Some(0),0,"zero.csv"),("[sales].[selected_view]","json",None,2,"view.json"),("[sales].[selected_view]","sql",None,0,"view.sql")] {
        let before=fixture.wire().len();let options=options(&fixture,"catalog.with.dot",table,name,format,limit);
        assert_eq!(crate::transfer::run_export_task(&db,&kind,&options).await.unwrap(),count);
        let text=std::fs::read_to_string(&options.options.file).unwrap();
        if format=="json" {let rows:serde_json::Value=serde_json::from_str(&text).unwrap();assert_eq!(rows[0]["id"],"9223372036854775807");assert_eq!(rows[0]["payload"],"0x00FF8027");assert_eq!(rows[0]["rv"],"0x0000000000000001");assert!(rows[0]["nullable"].is_null());assert_eq!(rows[0]["date_value"],"2026-11-11");assert_eq!(rows[0]["time_value"],"12:00:00.1234567");assert_eq!(rows[0]["offset_value"],"2026-11-11 12:00:00.1234567+08:00");}
        if name=="zero.csv" {assert_eq!(text,"id,payload,rv,nullable,date_value,time_value,moment,offset_value\r\n");}
        if name=="table.sql" {assert!(text.contains("INSERT INTO [catalog.with.dot].[sales].[order]]detail]"));assert!(text.contains("0x00FF8027")&&text.contains("0x0000000000000001"));assert!(!text.contains("N'0x"));}
        if name=="view.sql" {assert!(text.contains("CREATE VIEW [sales].[selected_view]"));assert!(text.contains("USE [catalog.with.dot];\r\nGO"));assert!(!text.contains("INSERT INTO"));}
        let wire=fixture.wire();assert_eq!(wire[before..].iter().filter(|row|data_select(row)).count(),if name=="view.sql" {0}else{1});
        let progress=crate::transfer::task_progress(&options.task_id).unwrap();assert_eq!(progress.status,"completed");
        assert!(wire[before..].iter().any(|row|row["sql"].as_str().is_some_and(|sql|sql.starts_with("IF @@TRANCOUNT > 0 ROLLBACK"))));
    }
    let before=fixture.wire().len();let dump=dump_options(&fixture,"catalog.with.dot","[sales].[selected_view]");assert_eq!(crate::transfer::dump(&db,&kind,&dump).await.unwrap(),0);
    let sql=std::fs::read_to_string(&dump.file).unwrap();assert!(sql.contains("USE [catalog.with.dot];\r\nGO\r\nDROP VIEW IF EXISTS [sales].[selected_view];\r\nGO\r\nCREATE VIEW"));assert!(!sql.contains("INSERT INTO")&&!fixture.wire()[before..].iter().any(data_select));
    let database_dump=fixture.file("database-views.sql");assert_eq!(crate::transfer::dump_database(&db,&kind,"catalog.with.dot",&["[sales].[selected_view]".into()],&[],true,true,&database_dump,"simulation").await.unwrap().0,0);let sql=std::fs::read_to_string(&database_dump).unwrap();assert!(sql.contains("GO\r\nCREATE VIEW")&&!sql.contains("INSERT INTO"));
    let mut data_dump=dump_options(&fixture,"catalog.with.dot","[sales].[order]]detail]");data_dump.data_only=true;data_dump.file=fixture.file("legacy-data.sql");
    let before=fixture.wire().len();assert_eq!(crate::transfer::dump(&db,&kind,&data_dump).await.unwrap(),2);let sql=std::fs::read_to_string(&data_dump.file).unwrap();assert!(sql.contains("0x00FF8027")&&sql.contains("0x0000000000000001"));assert!(!sql.contains("N'0x"));assert_eq!(fixture.wire()[before..].iter().filter(|row|data_select(row)).count(),1);
    let before=fixture.wire().len();let file=fixture.file("database-data.sql");let (rows,bytes)=crate::transfer::dump_database(&db,&kind,"catalog.with.dot",&["[sales].[order]]detail]".into()],&[],false,true,&file,"").await.unwrap();assert_eq!(rows,2);assert_eq!(bytes,std::fs::metadata(&file).unwrap().len());assert_eq!(fixture.wire()[before..].iter().filter(|row|data_select(row)).count(),1);
    let hidden=options(&fixture,"catalog.with.dot","[sales].[hidden_view]","hidden.sql","sql",None);
    let error=crate::transfer::run_export_task(&db,&kind,&hidden).await.unwrap_err();assert!(format!("{error:#}").contains("VIEW DEFINITION"));assert!(!Path::new(&hidden.options.file).exists());
    let mut invalid=options(&fixture,"catalog.with.dot","[sales].[encoding_failure]","encoding.csv","csv",None);invalid.options.encoding="latin1".into();
    let error=crate::transfer::run_export_task(&db,&kind,&invalid).await.unwrap_err();assert!(format!("{error:#}").contains("协议已排空"));assert!(!Path::new(&invalid.options.file).exists());
    let cancelled=options(&fixture,"catalog.with.dot","[sales].[slow]","cancelled.csv","csv",None);
    let (result,())=tokio::join!(crate::transfer::run_export_task(&db,&kind,&cancelled),cancel_after_rows(&cancelled.task_id));
    let error=result.unwrap_err();assert!(format!("{error:#}").contains("sessionRef=")&&format!("{error:#}").contains("协议已排空"));assert_eq!(crate::transfer::task_progress(&cancelled.task_id).unwrap().status,"cancelled");assert!(!Path::new(&cancelled.options.file).exists());
    let recovered=options(&fixture,"catalog.with.dot","[sales].[order]]detail]","recovered.json","json",None);assert_eq!(crate::transfer::run_export_task(&db,&kind,&recovered).await.unwrap(),2);
    fixture.clean_stages();drop(db);
    eprintln!("TDS wire simulation 已验证真实 Tiberius sessionRef、TOP(0)、列元数据、binary/rowversion、datetimeoffset、视图 DDL；厂商 SQL Server NOT_RUN");
}
