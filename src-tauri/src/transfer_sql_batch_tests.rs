use super::*;
use super::super::{run_export_task,export_table_streaming,ExportOptions,task_progress};
use sqlx::Executor;
use std::path::PathBuf;
struct Directory(PathBuf);
impl Directory{fn new()->Self{let p=std::env::temp_dir().join(format!("navkit-sql-batch-{}",uuid::Uuid::new_v4()));std::fs::create_dir(&p).unwrap();Self(p)}fn file(&self,name:&str)->String{self.0.join(name).to_string_lossy().into_owned()}}
impl Drop for Directory{fn drop(&mut self){let _=std::fs::remove_dir_all(&self.0);}}
fn opts(dir:&Directory,file:&str,format:&str,rows:u32,bytes:u64)->ExportTaskOptions{ExportTaskOptions{options:ExportOptions{connection_id:"isolated".into(),schema:"main".into(),table:"result".into(),file:dir.file(file),format:format.into(),has_header:true,delimiter:",".into(),encoding:"utf8".into(),where_clause:None,limit:None},task_id:uuid::Uuid::new_v4().to_string(),sql_batch_rows:rows,sql_batch_bytes:bytes}}
async fn fixture(dir:&Directory)->(Db,sqlx::SqlitePool){let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(dir.file("source.sqlite")).create_if_missing(true)).await.unwrap();sqlx::query("CREATE TABLE result(id INTEGER,payload TEXT,binary BLOB,nullable TEXT)").execute(&pool).await.unwrap();(Db::Sqlite(pool.clone()),pool)}
#[tokio::test]
async fn sql_batch_task_local_is_scoped_and_does_not_inherit_into_spawn(){
 let policy=Policy::checked(256,1024*1024).unwrap();let metrics=Arc::new(Mutex::new(Metrics::default()));
 scoped(Some(policy),metrics,async { assert!(current().is_some());assert!(tokio::spawn(async {current().is_none()}).await.unwrap());assert!(current().is_some()); }).await;
 assert!(current().is_none());
}
#[test]
fn sql_batch_exact_encoding_accounting_and_policy_bounds(){
 let text="Aé中文🦀\r\n";assert_eq!(encoded_len(text,"utf8").unwrap(),text.len() as u64);assert_eq!(encoded_len(text,"utf-16le").unwrap(),text.encode_utf16().count() as u64*2);assert!(encoded_len(text,"latin1").is_err());
 for(label,encoding)in[("gbk",encoding_rs::GBK),("gb18030",encoding_rs::GB18030),("big5",encoding_rs::BIG5)]{let text="中文，quote'";let(encoded,_,bad)=encoding.encode(text);assert!(!bad);assert_eq!(encoded_len(text,label).unwrap(),encoded.len() as u64);}
 assert!(Policy::checked(1,1024).is_err());assert!(Policy::checked(1001,1024).is_err());assert!(Policy::checked(256,MAX_BATCH_BYTES+1).is_err());
}
#[tokio::test]
async fn sql_batch_native_cursor_roundtrip_cross_page_finish_and_null_blob_int64(){
 let dir=Directory::new();let(db,pool)=fixture(&dir).await;
 for i in 0..513i64{sqlx::query("INSERT INTO result VALUES(?,?,?,NULL)").bind(i64::MAX-i).bind(format!("中文🦀 '\\ \nrow{i}")).bind(vec![0u8,255,39,128]).execute(&pool).await.unwrap();}
 let options=opts(&dir,"multi.sql","sql",256,1024*1024);assert_eq!(run_export_task(&db,&DbKind::of("sqlite"),&options).await.unwrap(),513);
 let sql=std::fs::read_to_string(&options.options.file).unwrap();assert_eq!(sql.matches("INSERT INTO").count(),3);assert!(sql.ends_with(";\r\n"));
 let target=sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();sqlx::query("CREATE TABLE result(id INTEGER,payload TEXT,binary BLOB,nullable TEXT)").execute(&target).await.unwrap();sqlx::raw_sql(&sql).execute(&target).await.unwrap();
 let query="SELECT id,payload,binary,nullable FROM result ORDER BY id";let source:Vec<(i64,String,Vec<u8>,Option<String>)>=sqlx::query_as(query).fetch_all(&pool).await.unwrap();let restored:Vec<(i64,String,Vec<u8>,Option<String>)>=sqlx::query_as(query).fetch_all(&target).await.unwrap();assert_eq!(source,restored);
 let progress=task_progress(&options.task_id).unwrap();assert!(progress.message.contains("3 语句"));assert!(!progress.message.contains("row0"));target.close().await;pool.close().await;
}
#[tokio::test]
async fn sql_batch_parallel_task_scope_no_contamination_legacy_single_and_formats_unchanged(){
 let dir=Directory::new();let(db,pool)=fixture(&dir).await;for i in 0..10{sqlx::query("INSERT INTO result VALUES(?,'payload',X'00FF',NULL)").bind(i).execute(&pool).await.unwrap();}
 let multi=opts(&dir,"multi.sql","sql",4,1024*1024);let single=opts(&dir,"legacy.sql","sql",0,0);
 let kind=DbKind::of("sqlite");let(a,b)=tokio::join!(run_export_task(&db,&kind,&multi),export_table_streaming(&db,&kind,&single.options));assert_eq!(a.unwrap(),10);assert_eq!(b.unwrap(),10);
 assert_eq!(std::fs::read_to_string(&multi.options.file).unwrap().matches("INSERT INTO").count(),3);assert_eq!(std::fs::read_to_string(&single.options.file).unwrap().matches("INSERT INTO").count(),10);assert!(current().is_none());
 for format in["csv","json"]{let normal=opts(&dir,&format!("default.{format}"),format,0,0);assert_eq!(run_export_task(&db,&DbKind::of("sqlite"),&normal).await.unwrap(),10);let invalid=opts(&dir,&format!("invalid.{format}"),format,256,1024*1024);assert!(run_export_task(&db,&DbKind::of("sqlite"),&invalid).await.is_err());assert!(!std::path::Path::new(&invalid.options.file).exists());}
 pool.close().await;
}
#[tokio::test]
async fn sql_batch_encoded_size_and_oversize_record_are_hard_limits_no_publication(){
 let dir=Directory::new();let(db,pool)=fixture(&dir).await;for i in 0..9{sqlx::query("INSERT INTO result VALUES(?,?,NULL,NULL)").bind(i).bind("'中文".repeat(70)).execute(&pool).await.unwrap();}
 let options=opts(&dir,"bounded.sql","sql",256,1024);assert_eq!(run_export_task(&db,&DbKind::of("sqlite"),&options).await.unwrap(),9);let sql=std::fs::read_to_string(&options.options.file).unwrap();for statement in sql.split_inclusive(";\r\n"){assert!(statement.len()<=1024);}
 sqlx::raw_sql("DELETE FROM result; INSERT INTO result VALUES(1,printf('%2048s','x'),NULL,NULL)").execute(&pool).await.unwrap();let oversize=opts(&dir,"oversize.sql","sql",256,1024);assert!(run_export_task(&db,&DbKind::of("sqlite"),&oversize).await.is_err());assert!(!std::path::Path::new(&oversize.options.file).exists());
 let invalid=opts(&dir,"unsupported.sql","sql",256,1024);assert!(Policy::for_task(&db,&DbKind::of("mssql"),&invalid).is_err());
 sqlx::query("CREATE VIEW result_view AS SELECT * FROM result").execute(&pool).await.unwrap();let mut view=opts(&dir,"view.sql","sql",256,1024);view.options.table="result_view".into();assert!(run_export_task(&db,&DbKind::of("sqlite"),&view).await.is_err());assert!(!std::path::Path::new(&view.options.file).exists());pool.close().await;
}
#[tokio::test]
async fn sql_batch_statement_trigger_semantics_are_explicitly_different_not_equivalent(){
 // SQLite only has row triggers; a statement side-effect still exposes statement boundaries.
 // PostgreSQL statement-level triggers/default statement_timestamp require the same explicit opt-in.
 let dir=Directory::new();let(db,pool)=fixture(&dir).await;for i in 0..3{sqlx::query("INSERT INTO result VALUES(?,'v',NULL,NULL)").bind(i).execute(&pool).await.unwrap();}
 let multi=opts(&dir,"multi.sql","sql",256,1024);let single=opts(&dir,"single.sql","sql",0,0);run_export_task(&db,&DbKind::of("sqlite"),&multi).await.unwrap();run_export_task(&db,&DbKind::of("sqlite"),&single).await.unwrap();
 let multi=std::fs::read_to_string(multi.options.file).unwrap();let single=std::fs::read_to_string(single.options.file).unwrap();assert_eq!(multi.matches("INSERT INTO").count(),1);assert_eq!(single.matches("INSERT INTO").count(),3);
 let setup="CREATE TABLE result(id INTEGER,payload TEXT,binary BLOB,nullable TEXT); CREATE TABLE audit(previous_changes INTEGER); CREATE TRIGGER capture AFTER INSERT ON result BEGIN INSERT INTO audit VALUES(changes()); END;";
 let a=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();let b=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
 for target in[&a,&b]{sqlx::raw_sql(setup).execute(target).await.unwrap();}
 sqlx::raw_sql(&multi).execute(&a).await.unwrap();sqlx::raw_sql(&single).execute(&b).await.unwrap();
 let observed_a:Vec<i64>=sqlx::query_scalar("SELECT previous_changes FROM audit ORDER BY rowid").fetch_all(&a).await.unwrap();let observed_b:Vec<i64>=sqlx::query_scalar("SELECT previous_changes FROM audit ORDER BY rowid").fetch_all(&b).await.unwrap();assert_eq!(observed_a.len(),3);assert_eq!(observed_b.len(),3);assert_ne!(observed_a,observed_b,"真实触发器可以观察语句边界：批量不等价于逐行");a.close().await;b.close().await;pool.close().await;
}
