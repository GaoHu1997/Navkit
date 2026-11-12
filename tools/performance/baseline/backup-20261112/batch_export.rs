//! Explicit table/view batch export. Empty selections are errors, never whole-database backups.
use std::{collections::HashSet, fs::File, io::{Read, Write}, path::{Path, PathBuf}};
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use tauri::State;
use crate::{commands::AppState, db::Db, error::{AppError, CmdResult}, models::{DbKind, ExportOptions, ObjectType}, transfer::{ExportTaskOptions, TaskControl}};
#[path = "transfer_output.rs"] mod output;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BatchObjectType { Table, View }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchExportObject { pub name: String, pub object_type: BatchObjectType }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum BatchDestination { Files { base_file: String }, SqlFile { file: String } }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchExportRequest {
    pub connection_id: String, pub namespace: String, pub objects: Vec<BatchExportObject>,
    pub format: String, pub destination: BatchDestination, pub encoding: String,
    pub delimiter: String, pub has_header: bool, pub task_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchExportItemResult { pub object: BatchExportObject, pub file: Option<String>, pub rows: u64, pub status: String, pub error: Option<String> }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchExportResult { pub items: Vec<BatchExportItemResult>, pub combined_file: Option<String>, pub message: String }

fn file_part(value: &str) -> String {
    let value: String = value.chars().map(|ch| if ch.is_control() || "<>:\"/\\|?*".contains(ch) { '_' } else { ch }).take(80).collect();
    let value = value.trim_end_matches(['.', ' ']); if value.is_empty() { "object".into() } else { value.into() }
}
fn output_files(request: &BatchExportRequest) -> Result<Vec<String>> {
    if request.objects.is_empty() || request.objects.len() > 256 { bail!("请明确选择 1–256 个表或视图，空集合不会解释为全库"); }
    if !matches!(request.format.as_str(), "csv" | "json" | "sql") { bail!("批量导出仅支持 CSV、JSON、SQL"); }
    let mut names = HashSet::new();
    for object in &request.objects {
        if object.name.is_empty() || object.name.contains('\0') || !names.insert(object.name.clone()) { bail!("对象身份不能为空、重复或含空字符"); }
    }
    match &request.destination {
        BatchDestination::SqlFile { file } => {
            if !Path::new(file).is_absolute() { bail!("导出文件必须使用完整绝对路径"); }
            if request.format != "sql" { bail!("只有 SQL 可以合并为单文件"); }
            output::checked_absolute(Path::new(file))?;
            Ok(vec![file.clone(); request.objects.len()])
        }
        BatchDestination::Files { base_file } => {
            if !Path::new(base_file).is_absolute() { bail!("导出文件必须使用完整绝对路径"); }
            let base = output::checked_absolute(Path::new(base_file))?;
            let parent = base.parent().ok_or_else(|| anyhow!("缺少导出目录"))?;
            let stem = base.file_stem().and_then(|value| value.to_str()).ok_or_else(|| anyhow!("文件名无效"))?;
            Ok(request.objects.iter().enumerate().map(|(index, object)| parent.join(format!("{}_{:03}_{}.{}", file_part(stem), index+1, file_part(&object.name), request.format)).to_string_lossy().into_owned()).collect())
        }
    }
}
async fn validate_objects(db: &Db, kind: &DbKind, request: &BatchExportRequest) -> Result<()> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let catalog = crate::schema::list_tables(db, kind.clone(), &request.namespace).await?;
    for object in &request.objects {
        if !catalog.iter().any(|(name, ty)| name == &object.name && matches!((&object.object_type, ty), (BatchObjectType::Table, ObjectType::Table) | (BatchObjectType::View, ObjectType::View))) {
            bail!("所选对象已不存在或类型变化：{}；请刷新清单后重试", object.name);
        }
    }
    Ok(())

    }).await
}
struct InternalFile(PathBuf);
impl Drop for InternalFile { fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); } }
async fn run_batch(db: &Db, kind: &DbKind, request: &BatchExportRequest, paths: Vec<String>) -> Result<BatchExportResult> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let task = TaskControl::begin(&request.task_id)?;
    let result = async {
        validate_objects(db, kind, request).await?;
        let combined = match &request.destination { BatchDestination::SqlFile { file } => Some(output::StagedOutput::new(file)?), _ => None };
        let mut combined_output = combined.as_ref().map(|stage| stage.file_clone()).transpose()?;
        let mut items = Vec::with_capacity(request.objects.len());
        let mut abort = false;
        for (index, object) in request.objects.iter().enumerate() {
            let file = paths[index].clone();
            let mut item = BatchExportItemResult { object: object.clone(), file: None, rows: 0, status: "failed".into(), error: None };
            let cancelled = task.check().is_err();
            if abort || cancelled { abort = true; item.status = "skipped".into(); item.error = Some(if cancelled { "已在对象边界停止；当前对象之前完成的独立文件保留，不再读取剩余对象" } else { "合并 SQL 前一对象失败，未发布目标文件" }.into()); items.push(item); continue; }
            let internal = combined.as_ref().map(|_| InternalFile(Path::new(&file).parent().unwrap().join(format!(".navkit-batch-{}.sql", uuid::Uuid::new_v4()))));
            let target = internal.as_ref().map(|file| file.0.to_string_lossy().into_owned()).unwrap_or_else(|| file.clone());
            let child = ExportTaskOptions { task_id: format!("batch_{}_{}", uuid::Uuid::new_v4().simple(), index), options: ExportOptions {
                connection_id: request.connection_id.clone(), schema: request.namespace.clone(), table: object.name.clone(), file: target,
                format: request.format.clone(), has_header: request.has_header, delimiter: request.delimiter.clone(), encoding: request.encoding.clone(), where_clause: None, limit: None,
            }};
            let exported = async {
                task.check()?;
                let rows = crate::transfer::run_export_task(db, kind, &child).await?;
                if let (Some(writer), Some(internal)) = (combined_output.as_mut(), internal.as_ref()) {
                    // Bounded copy; no whole-table or whole-file allocation. A cancelled batch never publishes its SQL stage.
                    let mut source = File::open(&internal.0)?; let mut buffer = [0u8; 64 * 1024];
                    loop { task.check()?; let count = source.read(&mut buffer)?; if count == 0 { break; } writer.write_all(&buffer[..count])?; }
                }
                Ok::<_, anyhow::Error>(rows)
            }.await;
            match exported {
                Ok(rows) => { item.rows = rows; item.status = if combined.is_some() { "staged" } else { "completed" }.into(); if combined.is_none() { item.file = Some(file); } }
                Err(error) => { let error = format!("{error:#}"); item.status = if error.contains("TRANSFER_EFFECTS_UNKNOWN") { "unknown" } else { "failed" }.into(); item.error = Some(error); if combined.is_some() { abort = true; } }
            }
            items.push(item);
            task.update("batch_export", (index+1) as u64, 0, &format!("已处理 {} / {} 个对象；取消在当前对象结束后生效，已完成的独立文件保留",index+1,request.objects.len()));
        }
        let mut combined_file = None;
        if let Some(stage) = combined {
            if !abort {
                drop(combined_output.take());
                match stage.publish(|| task.check()) {
                    Ok(()) => { combined_file = paths.first().cloned(); for item in &mut items { item.status = "completed".into(); item.file = combined_file.clone(); } }
                    Err(error) => { let error = format!("{error:#}"); for item in &mut items { item.status = if error.contains("TRANSFER_EFFECTS_UNKNOWN") { "unknown" } else { "failed" }.into(); item.error = Some(error.clone()); } }
                }
            } else { for item in items.iter_mut().filter(|item| item.status == "staged") { item.status = "failed".into(); item.error = Some("其他对象导出失败，组合文件未发布".into()); } }
        }
        let message = if task.check().is_err() { "已在对象边界停止；已完成的独立文件保留，组合 SQL 未发布。请按对象结果核对。" } else { "每个对象独立一致读取（SQL Server 可能持有读锁）；已存在文件不会覆盖。SQL 视图只写真实定义，需先具备依赖对象。失败和未知结果请按对象核对，禁止自动重放。" };
        Ok::<_, anyhow::Error>(BatchExportResult { items, combined_file, message: message.into() })
    }.await;
    let progress = result.as_ref().map_err(|error| anyhow!("{error:#}")).and_then(|receipt| {
        if receipt.items.iter().any(|item| item.status == "unknown") { bail!("TRANSFER_EFFECTS_UNKNOWN: 部分对象结果或连接排空未知，需按逐对象回执核对，禁止自动重放"); }
        if receipt.items.iter().any(|item| item.status == "failed") { bail!("部分对象导出失败；已完成文件保留，请查看逐对象回执"); }
        Ok(())
    });
    let progress = if progress.is_ok() { task.check() } else { progress };
    task.finish(&progress); result

    }).await
}
#[tauri::command]
pub async fn run_batch_export(state: State<'_, AppState>, request: BatchExportRequest) -> CmdResult<BatchExportResult> {
    let _activity = state.activity.enter()?;
    let paths = output_files(&request).map_err(AppError::from)?;
    let kind = state.store.connections().into_iter().find(|cfg| cfg.id == request.connection_id).ok_or_else(|| AppError::new("连接不存在"))?.kind;
    let db = crate::commands::scoped_db(&state, &request.connection_id, &request.namespace).await?;
    for path in &paths { crate::commands::protect_export_path(&state, path, db.as_ref()).await?; }
    run_batch(db.as_ref(), &kind, &request, paths).await.map_err(AppError::from)
}
#[cfg(test)] mod tests {
    use super::*;
    fn request(directory: &Path) -> BatchExportRequest { BatchExportRequest { connection_id: "fixture".into(), namespace: "main".into(), objects: vec![BatchExportObject { name: "one".into(), object_type: BatchObjectType::Table }, BatchExportObject { name: "two".into(), object_type: BatchObjectType::View }], format: "csv".into(), destination: BatchDestination::Files { base_file: directory.join("chosen.csv").to_string_lossy().into_owned() }, encoding: "utf-8".into(), delimiter: ",".into(), has_header: true, task_id: uuid::Uuid::new_v4().to_string() } }
    #[test] fn explicit_collection_and_destination_contracts() {
        let dir = std::env::temp_dir(); let mut req = request(&dir);
        let files = output_files(&req).unwrap(); assert!(files[0].ends_with("chosen_001_one.csv")); assert!(files[1].ends_with("chosen_002_two.csv"));
        req.objects.clear(); assert!(output_files(&req).is_err());
        req = request(&dir); req.objects[1].name = "one".into(); assert!(output_files(&req).is_err());
        req = request(&dir); req.destination = BatchDestination::SqlFile { file: dir.join("all.sql").to_string_lossy().into_owned() }; assert!(output_files(&req).is_err());
        req.format = "sql".into(); assert!(output_files(&req).is_ok());
    }
    #[tokio::test] async fn sqlite_real_batch_is_bounded_and_combined_sql_is_only_published_when_complete() {
        let dir = std::env::temp_dir().join(format!("navkit-batch-test-{}", uuid::Uuid::new_v4())); std::fs::create_dir(&dir).unwrap();
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        crate::sql_history::query("CREATE TABLE one (id INTEGER, text TEXT)").execute(&pool).await.unwrap();
        crate::sql_history::query("INSERT INTO one VALUES (1,'alpha'),(2,'beta')").execute(&pool).await.unwrap();
        crate::sql_history::query("CREATE VIEW two AS SELECT * FROM one").execute(&pool).await.unwrap();
        let db = Db::Sqlite(pool.clone()); let kind = DbKind::of("sqlite"); let mut req = request(&dir);
        let result = run_batch(&db, &kind, &req, output_files(&req).unwrap()).await.unwrap(); assert_eq!(result.items.len(),2); assert!(result.items.iter().all(|item| item.status=="completed" && item.rows==2));
        req.task_id = uuid::Uuid::new_v4().to_string(); req.format="sql".into(); let combined=dir.join("all.sql"); req.destination=BatchDestination::SqlFile {file:combined.to_string_lossy().into_owned()};
        let result=run_batch(&db,&kind,&req,output_files(&req).unwrap()).await.unwrap(); assert!(result.combined_file.is_some()); let sql=std::fs::read_to_string(&combined).unwrap(); assert!(sql.contains("CREATE VIEW two AS SELECT * FROM one")); assert!(!sql.contains("INSERT INTO \"main\".\"two\"") && !sql.contains("CREATE TABLE two")); assert_eq!(result.items[1].rows,0);
        let restored = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        crate::sql_history::query("CREATE TABLE one (id INTEGER, text TEXT)").execute(&restored).await.unwrap();
        crate::sql_history::raw_sql(&sql).execute(&restored).await.unwrap();
        assert_eq!(crate::sql_history::query_scalar::<_,i64>("SELECT COUNT(*) FROM two").fetch_one(&restored).await.unwrap(),2); restored.close().await;
        req.task_id=uuid::Uuid::new_v4().to_string(); assert!(run_batch(&db,&kind,&req,output_files(&req).unwrap()).await.is_err()); assert_eq!(std::fs::read_to_string(&combined).unwrap(),sql);
        pool.close().await; std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test] async fn sqlite_batch_reports_partial_files_without_overwriting_existing_target() {
        let dir = std::env::temp_dir().join(format!("navkit-batch-partial-{}",uuid::Uuid::new_v4())); std::fs::create_dir(&dir).unwrap();
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        crate::sql_history::raw_sql("CREATE TABLE one(id INTEGER, payload BLOB); INSERT INTO one VALUES(1,x'00FF8027'); CREATE VIEW two AS SELECT * FROM one").execute(&pool).await.unwrap();
        let db=Db::Sqlite(pool.clone()); let kind=DbKind::of("sqlite"); let req=request(&dir); let paths=output_files(&req).unwrap();
        std::fs::write(&paths[0],b"user owned original").unwrap();
        let result=run_batch(&db,&kind,&req,paths.clone()).await.unwrap();
        assert_eq!(result.items[0].status,"failed"); assert!(result.items[0].file.is_none()); assert_eq!(std::fs::read(&paths[0]).unwrap(),b"user owned original");
        assert_eq!(result.items[1].status,"completed"); assert_eq!(result.items[1].rows,1); assert!(std::fs::read_to_string(&paths[1]).unwrap().contains("0x00FF8027"));
        assert!(!std::fs::read_dir(&dir).unwrap().any(|entry| entry.unwrap().file_name().to_string_lossy().starts_with(".navkit-")));
        pool.close().await; std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test] async fn sqlite_batch_cancel_finishes_current_object_and_skips_remaining_objects() {
        let dir=std::env::temp_dir().join(format!("navkit-batch-cancel-{}",uuid::Uuid::new_v4()));std::fs::create_dir(&dir).unwrap();
        let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        crate::sql_history::raw_sql("CREATE TABLE one(id INTEGER); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000) INSERT INTO one SELECT x FROM n; CREATE VIEW two AS SELECT * FROM one").execute(&pool).await.unwrap();
        let db=Db::Sqlite(pool.clone());let kind=DbKind::of("sqlite");let req=request(&dir);let paths=output_files(&req).unwrap();
        let cancel=async {
            let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);
            loop {
                if std::fs::read_dir(&dir).unwrap().any(|entry|entry.unwrap().file_name().to_string_lossy().starts_with(".navkit-export-")) {crate::transfer::cancel_task(&req.task_id).unwrap();break;}
                assert!(std::time::Instant::now()<deadline,"必须在真实对象 stage 出现后取消");tokio::task::yield_now().await;
            }
        };
        let (result,())=tokio::join!(run_batch(&db,&kind,&req,paths.clone()),cancel);let result=result.unwrap();
        assert_eq!((result.items[0].status.as_str(),result.items[0].rows),("completed",10000));assert_eq!(result.items[1].status,"skipped");
        assert!(Path::new(&paths[0]).exists());assert!(!Path::new(&paths[1]).exists());assert_eq!(crate::transfer::task_progress(&req.task_id).unwrap().status,"cancelled");
        assert!(!std::fs::read_dir(&dir).unwrap().any(|entry|entry.unwrap().file_name().to_string_lossy().starts_with(".navkit-")));
        pool.close().await;std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test] async fn sqlite_legacy_dump_streams_large_typed_data_and_restores_without_clobber() {
        let dir=std::env::temp_dir().join(format!("navkit-dump-stream-{}",uuid::Uuid::new_v4()));std::fs::create_dir(&dir).unwrap();
        let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        crate::sql_history::raw_sql("CREATE TABLE one(id INTEGER, payload BLOB, note TEXT); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<12005) INSERT INTO one SELECT x, x'00FF8027', CASE WHEN x%2=0 THEN NULL ELSE '中文,quote''line' END FROM n").execute(&pool).await.unwrap();
        let db=Db::Sqlite(pool.clone());let kind=DbKind::of("sqlite");
        let options=crate::models::DumpOptions {connection_id:"fixture".into(),schema:"main".into(),table:"one".into(),file:dir.join("one.sql").to_string_lossy().into_owned(),structure_only:false,data_only:false,include_drop:true};
        assert_eq!(crate::transfer::dump(&db,&kind,&options).await.unwrap(),12005);
        let restored=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        let text=std::fs::read_to_string(&options.file).unwrap();assert!(text.contains("X'00FF8027'"));crate::sql_history::raw_sql(&text).execute(&restored).await.unwrap();
        assert_eq!(crate::sql_history::query_scalar::<_,i64>("SELECT COUNT(*) FROM one WHERE typeof(payload)='blob' AND hex(payload)='00FF8027'").fetch_one(&restored).await.unwrap(),12005);
        assert_eq!(crate::sql_history::query_scalar::<_,i64>("SELECT COUNT(*) FROM one WHERE note IS NULL").fetch_one(&restored).await.unwrap(),6002);
        let file=dir.join("database.sql");let (rows,bytes)=crate::transfer::dump_database(&db,&kind,"main",&[],&[],true,true,file.to_str().unwrap(),"").await.unwrap();assert_eq!(rows,12005);assert_eq!(bytes,std::fs::metadata(&file).unwrap().len());
        crate::sql_history::raw_sql(&std::fs::read_to_string(&file).unwrap()).execute(&restored).await.unwrap();assert_eq!(crate::sql_history::query_scalar::<_,i64>("SELECT COUNT(*) FROM one WHERE note='中文,quote''line'").fetch_one(&restored).await.unwrap(),6003);
        assert!(crate::transfer::dump(&db,&kind,&options).await.is_err());assert_eq!(std::fs::read_to_string(&options.file).unwrap(),text);
        assert!(!std::fs::read_dir(&dir).unwrap().any(|entry|entry.unwrap().file_name().to_string_lossy().starts_with(".navkit-")));restored.close().await;pool.close().await;std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test] async fn sqlite_legacy_view_and_database_dumps_use_real_ddl_and_never_clobber() {
        let dir=std::env::temp_dir().join(format!("navkit-dump-views-{}",uuid::Uuid::new_v4()));std::fs::create_dir(&dir).unwrap();
        let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        crate::sql_history::raw_sql("CREATE TABLE one(id INTEGER); INSERT INTO one VALUES(1),(2); CREATE VIEW two AS SELECT * FROM one").execute(&pool).await.unwrap();
        let db=Db::Sqlite(pool.clone());let kind=DbKind::of("sqlite");let file=dir.join("view.sql");
        let mut options=crate::models::DumpOptions {connection_id:"fixture".into(),schema:"main".into(),table:"two".into(),file:file.to_string_lossy().into_owned(),structure_only:false,data_only:false,include_drop:true};
        assert_eq!(crate::transfer::dump(&db,&kind,&options).await.unwrap(),0);let text=std::fs::read_to_string(&file).unwrap();assert!(text.contains("DROP VIEW IF EXISTS")&&text.contains("CREATE VIEW two AS SELECT * FROM one"));assert!(!text.contains("CREATE TABLE")&&!text.contains("INSERT INTO"));
        assert!(crate::transfer::dump(&db,&kind,&options).await.is_err());assert_eq!(std::fs::read_to_string(&file).unwrap(),text);
        options.file=dir.join("data-only.sql").to_string_lossy().into_owned();options.data_only=true;assert!(crate::transfer::dump(&db,&kind,&options).await.is_err());assert!(!Path::new(&options.file).exists());
        let full=dir.join("database.sql");let result=crate::transfer::dump_database(&db,&kind,"main",&[],&[],true,true,full.to_str().unwrap(),"").await.unwrap();assert_eq!(result.0,2);
        let restored=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();let text=std::fs::read_to_string(&full).unwrap();crate::sql_history::raw_sql(&text).execute(&restored).await.unwrap();assert_eq!(crate::sql_history::query_scalar::<_,i64>("SELECT COUNT(*) FROM two").fetch_one(&restored).await.unwrap(),2);
        assert!(crate::transfer::dump_database(&db,&kind,"main",&[],&[],true,true,full.to_str().unwrap(),"").await.is_err());assert_eq!(std::fs::read_to_string(&full).unwrap(),text);
        assert!(!std::fs::read_dir(&dir).unwrap().any(|entry|entry.unwrap().file_name().to_string_lossy().starts_with(".navkit-")));restored.close().await;pool.close().await;std::fs::remove_dir_all(dir).unwrap();
    }
}
