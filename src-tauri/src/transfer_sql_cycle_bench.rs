//! Same-transaction SQL export + restore A/B. File formatting is explicit; no speed assertions.
use super::*;
use std::path::PathBuf;
struct Directory(PathBuf);
impl Directory { fn new()->Self {let p=std::env::temp_dir().join(format!("navkit-sql-cycle-{}",uuid::Uuid::new_v4()));std::fs::create_dir(&p).unwrap();Self(p)} fn file(&self,name:&str)->PathBuf{self.0.join(name)} }
impl Drop for Directory {fn drop(&mut self){let _=std::fs::remove_dir_all(&self.0);}}
#[tokio::test]
#[ignore="explicit isolated file-backed export+atomic restore A/B"]
async fn sql_export_restore_actual_cycle_ab() {
    let dir=Directory::new();
    let total=std::env::var("NAVKIT_SQL_CYCLE_ROWS").ok().and_then(|value|value.parse::<u64>().ok()).unwrap_or(10000);
    assert!((1..=100000).contains(&total));
    let source=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(dir.file("source.sqlite")).create_if_missing(true)).await.unwrap();
    sqlx::raw_sql("CREATE TABLE result(id INTEGER PRIMARY KEY,v TEXT,b BLOB,n TEXT)").execute(&source).await.unwrap();
    let mut tx=source.begin().await.unwrap();
    for id in 0..total {sqlx::query("INSERT INTO result VALUES(?,?,X'00FF80',?)").bind(id as i64).bind(format!("row'中-{id},\nline\\value")).bind(if id%3==0{None}else{Some("value")}).execute(&mut *tx).await.unwrap();}tx.commit().await.unwrap();
    let mut singles=Vec::new();let mut groups=Vec::new();
    for round in 0..6 {
        let modes=if round%2==0{[1,256]}else{[256,1]};
        for batch in modes {
            let file=dir.file(&format!("{round}-{batch}.sql"));
            let request=ExportTaskOptions{options:ExportOptions{connection_id:"isolated".into(),schema:"main".into(),table:"result".into(),file:file.to_string_lossy().into_owned(),format:"sql".into(),has_header:true,delimiter:",".into(),encoding:"utf8".into(),where_clause:None,limit:None},task_id:uuid::Uuid::new_v4().to_string(),sql_batch_rows:batch,sql_batch_bytes:1024*1024};
            let target=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(
                sqlx::sqlite::SqliteConnectOptions::new().filename(dir.file(&format!("target-{round}-{batch}.sqlite"))).create_if_missing(true)).await.unwrap();
            sqlx::raw_sql("CREATE TABLE result(id INTEGER PRIMARY KEY,v TEXT,b BLOB,n TEXT)").execute(&target).await.unwrap();
            let start=std::time::Instant::now();
            assert_eq!(run_export_task(&Db::Sqlite(source.clone()),&DbKind::of("sqlite"),&request).await.unwrap(),total);
            let export_seconds=start.elapsed().as_secs_f64();
            let import:ImportOptions=serde_json::from_value(serde_json::json!({"connection_id":"isolated","schema":"main","table":"result","format":"sql","file":file,"encoding":"utf8"})).unwrap();
            let restore=std::time::Instant::now();
            assert_eq!(run_import_data(&Db::Sqlite(target.clone()),&DbKind::of("sqlite"),&import,&[],None).await.unwrap(),total);
            let restore_seconds=restore.elapsed().as_secs_f64();let cycle=export_seconds+restore_seconds;
            assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM result").fetch_one(&target).await.unwrap(),total as i64);
            assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM result WHERE n IS NULL").fetch_one(&target).await.unwrap(),((total+2)/3) as i64);
            assert_eq!(sqlx::query_scalar::<_,Vec<u8>>("SELECT b FROM result WHERE id=0").fetch_one(&target).await.unwrap(),vec![0,255,128]);
            assert_eq!(sqlx::query_scalar::<_,String>("SELECT v FROM result WHERE id=?").bind((total-1) as i64).fetch_one(&target).await.unwrap(),format!("row'中-{},\nline\\value",total-1));
            println!("SQL_CYCLE_AB round={round} batch_rows={batch} rows={total} export_s={export_seconds:.6} restore_s={restore_seconds:.6} cycle_s={cycle:.6} file_bytes={} transaction=one_atomic_target",std::fs::metadata(&file).unwrap().len());
            if round>0 {if batch==1{singles.push(cycle)}else{groups.push(cycle)}}
            target.close().await;
        }
    }
    singles.sort_by(f64::total_cmp);groups.sort_by(f64::total_cmp);
    println!("SQL_CYCLE_MEDIAN rows={total} single_seconds={:.6} grouped_seconds={:.6} ratio={:.3}",singles[2],groups[2],singles[2]/groups[2]);
    source.close().await;
}
