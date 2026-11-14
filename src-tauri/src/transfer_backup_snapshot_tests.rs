use super::*;
struct Directory(std::path::PathBuf);
impl Directory {
    fn new() -> Self { let dir=std::env::temp_dir().join(format!("navkit-backup-snapshot-{}",uuid::Uuid::new_v4()));std::fs::create_dir(&dir).unwrap();Self(dir) }
    fn path(&self,name:&str)->String { self.0.join(name).to_string_lossy().into_owned() }
}
impl Drop for Directory { fn drop(&mut self) { let _=std::fs::remove_dir_all(&self.0); } }
async fn fixture(dir:&Directory)->sqlx::SqlitePool {
    let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(dir.path("source.sqlite")).create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)).await.unwrap();
    sqlx::raw_sql("CREATE TABLE a(id INTEGER PRIMARY KEY,v TEXT,b BLOB); CREATE TABLE b(id INTEGER PRIMARY KEY,v TEXT); CREATE VIEW displayed AS SELECT id,v FROM a; INSERT INTO a VALUES(1,'old-中文',X'00FF80'); INSERT INTO b VALUES(1,'old-pair');").execute(&pool).await.unwrap();pool
}
#[tokio::test]
async fn backup_snapshot_catalog_and_all_tables_keep_same_read_version() {
    let dir=Directory::new();let pool=fixture(&dir).await;let kind=DbKind::of("sqlite");
    let mut connection=open(&pool).await.unwrap();assert_eq!(catalog(&mut connection,&[]).await.unwrap().len(),3);
    // Update a related pair in a committed writer transaction after snapshot acquisition.
    sqlx::raw_sql("BEGIN;UPDATE a SET v='new';UPDATE b SET v='new-pair';ALTER TABLE b ADD COLUMN later TEXT;COMMIT;").execute(&pool).await.unwrap();
    let path=dir.path("snapshot.sql");let stage=StagedOutput::new(&path).unwrap();
    let report=write_snapshot(&mut connection,&kind,"main",&[],true,true,&stage,"same read version",false).await.unwrap();
    assert_eq!(report.rows,2);connection.execute("ROLLBACK").await.unwrap();connection.close().await.unwrap();stage.publish(||Ok(())).unwrap();
    let sql=std::fs::read_to_string(&path).unwrap();assert!(sql.contains("old-中文"));assert!(sql.contains("old-pair"));assert!(!sql.contains("new-pair"));assert!(!sql.contains("later"));
    let target=sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();sqlx::raw_sql(&sql).execute(&target).await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT v FROM a").fetch_one(&target).await.unwrap(),"old-中文");
    assert_eq!(sqlx::query_scalar::<_,Vec<u8>>("SELECT b FROM a").fetch_one(&target).await.unwrap(),vec![0,255,128]);
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT v FROM displayed").fetch_one(&target).await.unwrap(),"old-中文");
    target.close().await;pool.close().await;
}
#[tokio::test]
async fn backup_snapshot_actual_database_entry_selection_and_create_only() {
    let dir=Directory::new();let pool=fixture(&dir).await;let kind=DbKind::of("sqlite");let db=Db::Sqlite(pool.clone());
    let path=dir.path("selected.sql");
    let report=super::super::dump_database_with_policy(&db,&kind,"main",&["a".into()],&[],true,true,&path,"selected",false).await.unwrap();
    assert_eq!(report.rows,1);assert_eq!(report.bytes,std::fs::metadata(&path).unwrap().len());
    let text=std::fs::read_to_string(&path).unwrap();assert!(text.contains("同一只读事务"));assert!(!text.contains("CREATE TABLE b"));
    assert!(super::super::dump_database(&db,&kind,"main",&[],&[],true,true,&path,"").await.is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(),text);
    let empty=dir.path("structure.sql");assert_eq!(run(&pool,&kind,"main",&["displayed".into()],true,false,&empty,"",false).await.unwrap().rows,0);
    assert!(std::fs::read_to_string(empty).unwrap().contains("CREATE VIEW"));
    pool.close().await;
}
#[tokio::test]
async fn backup_snapshot_failure_has_no_published_file_or_open_writer_lock() {
    let dir=Directory::new();let pool=fixture(&dir).await;let kind=DbKind::of("sqlite");
    sqlx::raw_sql("INSERT INTO a VALUES(2,CAST(X'FF' AS TEXT),NULL)").execute(&pool).await.unwrap();
    let path=dir.path("failed.sql");assert!(run(&pool,&kind,"main",&[],true,true,&path,"",false).await.is_err());
    assert!(!std::path::Path::new(&path).exists());
    sqlx::raw_sql("UPDATE b SET v='still-writable'").execute(&pool).await.unwrap();
    assert!(!std::fs::read_dir(&dir.0).unwrap().any(|entry|entry.unwrap().file_name().to_string_lossy().starts_with(".navkit-export-")));
    pool.close().await;
}

#[tokio::test]
async fn backup_snapshot_generated_columns_roundtrip_preserves_base_values_and_defaults() {
    let dir = Directory::new(); let pool = fixture(&dir).await; let kind = DbKind::of("sqlite");
    sqlx::raw_sql("CREATE TABLE generated(id INTEGER PRIMARY KEY, base INTEGER DEFAULT 5, virtual_value INTEGER GENERATED ALWAYS AS (base * 2) VIRTUAL, stored_value TEXT GENERATED ALWAYS AS ('v=' || base) STORED); INSERT INTO generated(id,base) VALUES(1,7); INSERT INTO generated(id) VALUES(2);").execute(&pool).await.unwrap();
    let path = dir.path("generated.sql");
    let report = run(&pool,&kind,"main",&["generated".into()],true,true,&path,"",false).await.unwrap();
    assert_eq!(report.rows, 2);
    let sql = std::fs::read_to_string(&path).unwrap();
    assert!(sql.contains("GENERATED ALWAYS"));
    for line in sql.lines().filter(|line| line.starts_with("INSERT INTO")) {
        assert!(!line.contains("\"virtual_value\"")); assert!(!line.contains("\"stored_value\""));
        assert!(line.contains("\"base\""), "ordinary defaulted values must not be omitted");
    }
    let target = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
    sqlx::raw_sql(&sql).execute(&target).await.unwrap();
    let rows: Vec<(i64,i64,i64,String)> = sqlx::query_as("SELECT id,base,virtual_value,stored_value FROM generated ORDER BY id").fetch_all(&target).await.unwrap();
    assert_eq!(rows, vec![(1,7,14,"v=7".into()),(2,5,10,"v=5".into())]);
    sqlx::query("INSERT INTO generated(id) VALUES(3)").execute(&target).await.unwrap();
    assert_eq!(sqlx::query_as::<_,(i64,i64,String)>("SELECT base,virtual_value,stored_value FROM generated WHERE id=3").fetch_one(&target).await.unwrap(),(5,10,"v=5".into()));
    // Also exercise the public detailed restore lifecycle, not only raw SQL syntax.
    let restored=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(dir.path("restored.sqlite")).create_if_missing(true)).await.unwrap();
    let restored_db=Db::Sqlite(restored.clone());
    let receipt=super::super::restore_sql_file_detailed(&restored_db,&kind,&path).await.unwrap();
    assert_eq!(receipt.failed,0,"{:?}",receipt.error);assert!(receipt.error.is_none());
    assert!(!receipt.effects_unknown);assert!(receipt.cleanup_error.is_none());
    assert_eq!(receipt.committed,receipt.total);assert_eq!(receipt.succeeded,receipt.total);
    let restored_rows:Vec<(i64,i64,i64,String)>=sqlx::query_as("SELECT id,base,virtual_value,stored_value FROM generated ORDER BY id").fetch_all(&restored).await.unwrap();
    assert_eq!(restored_rows,rows);
    sqlx::query("INSERT INTO generated(id) VALUES(3)").execute(&restored).await.unwrap();
    assert_eq!(sqlx::query_as::<_,(i64,i64,String)>("SELECT base,virtual_value,stored_value FROM generated WHERE id=3").fetch_one(&restored).await.unwrap(),(5,10,"v=5".into()));
    restored.close().await;target.close().await;pool.close().await;
}

#[tokio::test]
async fn backup_snapshot_missing_selected_object_fails_without_partial_or_replaced_output() {
    let dir = Directory::new(); let pool = fixture(&dir).await; let kind = DbKind::of("sqlite");
    let db = Db::Sqlite(pool.clone());
    for (file, selection) in [("only-missing.sql", vec!["missing".into()]), ("partly-missing.sql", vec!["a".into(), "missing".into()])] {
        let path = dir.path(file);
        let error = super::super::dump_database_with_policy(&db,&kind,"main",&selection,&[],true,true,&path,"",false).await.unwrap_err();
        assert!(format!("{error:#}").contains("BACKUP_OBJECT_MISSING"));
        assert!(!std::path::Path::new(&path).exists());
        std::fs::write(&path, "existing backup").unwrap();
        assert!(super::super::dump_database_with_policy(&db,&kind,"main",&selection,&[],true,true,&path,"",false).await.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "existing backup");
    }
    assert!(!std::fs::read_dir(&dir.0).unwrap().any(|entry|entry.unwrap().file_name().to_string_lossy().starts_with(".navkit-export-")));
    pool.close().await;
}

#[tokio::test]
async fn backup_snapshot_literal_sqlite_prefix_and_attached_ddl_roundtrip_without_trigger_replay() {
    let dir=Directory::new();let pool=fixture(&dir).await;let kind=DbKind::of("sqlite");
    sqlx::raw_sql("CREATE TABLE sqliteXevents(id INTEGER PRIMARY KEY,v TEXT); CREATE TABLE audit(v TEXT); CREATE UNIQUE INDEX unique_events ON sqliteXevents(v); CREATE TRIGGER audit_events AFTER INSERT ON sqliteXevents BEGIN INSERT INTO audit VALUES(NEW.v); END; INSERT INTO sqliteXevents VALUES(1,'original');").execute(&pool).await.unwrap();
    let path=dir.path("attached.sql");let db=Db::Sqlite(pool.clone());
    let dump=super::super::dump_database_with_policy(&db,&kind,"main",&[],&[],true,true,&path,"",false).await.unwrap();
    assert_eq!(dump.rows,4,"a, b, sqliteXevents and historical audit row must all survive");
    let text=std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("CREATE TABLE sqliteXevents"));assert!(text.contains("CREATE UNIQUE INDEX unique_events"));assert!(text.contains("CREATE TRIGGER audit_events"));
    let last_insert=text.rfind("\nINSERT INTO ").unwrap();assert!(text.find("CREATE TRIGGER audit_events").unwrap()>last_insert);assert!(text.find("CREATE UNIQUE INDEX unique_events").unwrap()>last_insert);
    let restored=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(dir.path("attached-restored.sqlite")).create_if_missing(true)).await.unwrap();
    let receipt=super::super::restore_sql_file_detailed(&Db::Sqlite(restored.clone()),&kind,&path).await.unwrap();
    assert_eq!(receipt.failed,0,"{:?}",receipt.error);assert!(receipt.error.is_none()&&receipt.cleanup_error.is_none());assert!(!receipt.effects_unknown);assert_eq!(receipt.committed,receipt.total);
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT v FROM sqliteXevents WHERE id=1").fetch_one(&restored).await.unwrap(),"original");
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM audit").fetch_one(&restored).await.unwrap(),1,"restoring original rows must not replay their trigger effects");
    assert!(sqlx::query("INSERT INTO sqliteXevents VALUES(2,'original')").execute(&restored).await.is_err(),"restored explicit unique index must reject duplicates");
    sqlx::query("INSERT INTO sqliteXevents VALUES(2,'new')").execute(&restored).await.unwrap();
    let audit:Vec<String>=sqlx::query_scalar("SELECT v FROM audit ORDER BY rowid").fetch_all(&restored).await.unwrap();assert_eq!(audit,vec!["original","new"]);
    restored.close().await;pool.close().await;
}

#[tokio::test]
async fn backup_snapshot_skipped_table_does_not_leave_attached_index_or_trigger() {
    let dir=Directory::new();let pool=fixture(&dir).await;let kind=DbKind::of("sqlite");
    sqlx::raw_sql("INSERT INTO a VALUES(2,CAST(X'FF' AS TEXT),NULL); CREATE INDEX skipped_index ON a(v); CREATE TRIGGER skipped_trigger AFTER INSERT ON a BEGIN INSERT INTO b VALUES(NEW.id,NEW.v); END;").execute(&pool).await.unwrap();
    let path=dir.path("skipped.sql");let dump=run(&pool,&kind,"main",&["a".into(),"b".into()],true,true,&path,"",true).await.unwrap();
    assert_eq!(dump.rows,1);assert_eq!(dump.warnings.len(),1);
    let text=std::fs::read_to_string(&path).unwrap();assert!(!text.contains("CREATE TABLE a"));assert!(!text.contains("skipped_index"));assert!(!text.contains("skipped_trigger"));
    let restored=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(dir.path("skipped-restored.sqlite")).create_if_missing(true)).await.unwrap();
    let receipt=super::super::restore_sql_file_detailed(&Db::Sqlite(restored.clone()),&kind,&path).await.unwrap();
    assert_eq!(receipt.failed,0,"{:?}",receipt.error);assert!(!receipt.effects_unknown);assert_eq!(receipt.committed,receipt.total);
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT v FROM b").fetch_one(&restored).await.unwrap(),"old-pair");
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM sqlite_schema WHERE name IN ('a','skipped_index','skipped_trigger')").fetch_one(&restored).await.unwrap(),0);
    restored.close().await;pool.close().await;
}

#[tokio::test]
async fn backup_snapshot_attached_ddl_uses_same_snapshot_and_combined_catalog_budget() {
    let dir=Directory::new();let pool=fixture(&dir).await;let kind=DbKind::of("sqlite");
    sqlx::raw_sql("CREATE INDEX old_index ON a(v); CREATE TRIGGER old_trigger AFTER INSERT ON a BEGIN SELECT 1; END;").execute(&pool).await.unwrap();
    let mut connection=open(&pool).await.unwrap();let objects=catalog(&mut connection,&["a".into()]).await.unwrap();
    sqlx::raw_sql("DROP INDEX old_index; DROP TRIGGER old_trigger; CREATE INDEX new_index ON a(v); CREATE TRIGGER new_trigger AFTER INSERT ON a BEGIN SELECT 2; END;").execute(&pool).await.unwrap();
    let attached=attached_catalog(&mut connection,&objects).await.unwrap();assert_eq!(attached.len(),2);assert!(attached.iter().any(|(_,name,_,_)|name=="old_index"));assert!(attached.iter().any(|(_,name,_,_)|name=="old_trigger"));
    let full=vec![("a".into(),"table".into(),"x".repeat(16*1024*1024-6))];
    assert!(attached_catalog(&mut connection,&full).await.unwrap_err().to_string().contains("目录"),"attached DDL must share table metadata's 16MiB ceiling");
    let path=dir.path("catalog-snapshot.sql");let stage=StagedOutput::new(&path).unwrap();
    write_snapshot(&mut connection,&kind,"main",&["a".into()],true,true,&stage,"",false).await.unwrap();
    connection.execute("ROLLBACK").await.unwrap();connection.close().await.unwrap();stage.publish(||Ok(())).unwrap();
    let text=std::fs::read_to_string(&path).unwrap();assert!(text.contains("CREATE INDEX old_index"));assert!(text.contains("CREATE TRIGGER old_trigger"));assert!(!text.contains("new_index"));assert!(!text.contains("new_trigger"));
    pool.close().await;
}
