//! Local gates/native-plan tests. Ignored tests require ONLY explicit disposable performance URLs.
use super::*;

struct TestDirectory(std::path::PathBuf);
impl TestDirectory {
    fn new() -> Result<Self> {
        let path = std::env::temp_dir().join(format!("navkit-backup-native-{}",uuid::Uuid::new_v4().simple()));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
    fn path(&self) -> &std::path::Path { &self.0 }
}
impl Drop for TestDirectory { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

fn report() -> DumpDatabaseReport { DumpDatabaseReport { rows: 0, bytes: 0, warnings: vec![] } }
fn check_staged_values(stage: &StagedOutput, expected: &str, count: usize) -> Result<()> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = stage.file_clone()?; file.seek(SeekFrom::Start(0))?;
    let mut text = String::new(); file.read_to_string(&mut text)?;
    let inserts: Vec<_> = text.lines().filter(|line|line.starts_with("INSERT INTO")).collect();
    anyhow::ensure!(inserts.len()==count && inserts.iter().all(|line|line.ends_with(expected)),"cross-table snapshot values/row count differ");
    Ok(())
}

fn object(name: &str, ty: &str, engine: &str) -> Object {
    Object { name: name.into(), identity: String::new(), ty: ty.into(), engine: engine.into() }
}

#[test]
fn policy_default_is_compatible_and_protocol_profiles_fail_closed() {
    for kind in ["sqlite", "mysql", "postgresql", "tidb", "mariadb"] {
        assert_eq!(mode(&DbKind::of(kind),None).unwrap(),Mode::Compat);
        assert_eq!(mode(&DbKind::of(kind),Some("compat")).unwrap(),Mode::Compat);
    }
    assert_eq!(mode(&DbKind::of("postgresql"),Some("pg_snapshot")).unwrap(),Mode::PgSnapshot);
    assert_eq!(mode(&DbKind::of("mysql"),Some("mysql_locked")).unwrap(),Mode::MysqlLocked);
    for (kind, policy) in [("tidb","mysql_locked"),("mariadb","mysql_locked"),("postgresql","mysql_locked"),("mysql","pg_snapshot"),("sqlite","pg_snapshot"),("mysql",""),("mysql","snapshot")] {
        assert!(mode(&DbKind::of(kind),Some(policy)).is_err());
    }
    let opts: crate::models::BackupOptions = serde_json::from_value(serde_json::json!({"connection_id":"c","schema":"s"})).unwrap();
    assert!(opts.source_consistency.is_none());
    assert!(opts.include_structure && opts.include_data && !opts.skip_incompatible_tables);
}

#[test]
fn mysql_native_plan_one_lock_statement_quotes_all_tables_and_rejects_dependencies() {
    let objects = vec![object("a`b","BASE TABLE","InnoDB"),object("c","BASE TABLE","InnoDB")];
    assert_eq!(mysql_lock_sql(&DbKind::of("mysql"),"s",&objects).unwrap(),"LOCK TABLES `s`.`a``b` READ, `s`.`c` READ");
    for objects in [vec![],vec![object("v","VIEW","")],vec![object("t","BASE TABLE","MyISAM")]] {
        assert!(mysql_lock_sql(&DbKind::of("mysql"),"s",&objects).is_err());
    }
    assert!(selected(vec![object("t","BASE TABLE","InnoDB")],&["missing".into()]).is_err());
    assert!(mysql_version("8.0.40","MySQL Community Server - GPL").is_ok());
    assert!(mysql_version("8.4.3","MySQL Enterprise Server - Commercial").is_ok());
    for (version,comment) in [("5.7.44","MySQL Community Server - GPL"),("8.0.11-TiDB-v8.5.0","TiDB Server"),("10.11.2-MariaDB","MariaDB Server"),("8.0.40","unknown proxy"),("9.0.0","MySQL Community Server - GPL")] {
        assert!(mysql_version(version,comment).is_err());
    }
    assert_eq!(pg_lock_sql(&DbKind::of("postgresql"),"s",&[object("a\"b","r","p"),object("c","r","p")]).unwrap(),"LOCK TABLE ONLY \"s\".\"a\"\"b\", ONLY \"s\".\"c\" IN ACCESS SHARE MODE NOWAIT");
    assert!(pg_lock_sql(&DbKind::of("postgresql"),"s",&[object("v","v","p")]).is_err());
    assert_eq!(PG_BEGIN,"BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY");
    assert_eq!(MYSQL_UNLOCK,"UNLOCK TABLES");
}

#[test]
fn native_startup_sequence_is_consumed_once_before_readers() {
    let nonce = uuid::Uuid::new_v4().to_string();
    let pg_lock = pg_lock_sql(&DbKind::of("postgresql"),"s",&[object("a","r","p"),object("b","r","p")]).unwrap();
    let pg = startup_plan(Mode::PgSnapshot,&pg_lock,&nonce).unwrap();
    assert_eq!(pg,vec![PG_BEGIN.to_string(),format!("SET LOCAL navkit.backup_token = '{nonce}'"),pg_lock]);
    let my_lock = mysql_lock_sql(&DbKind::of("mysql"),"s",&[object("a","BASE TABLE","InnoDB"),object("b","BASE TABLE","InnoDB")]).unwrap();
    let mysql = startup_plan(Mode::MysqlLocked,&my_lock,&nonce).unwrap();
    assert_eq!(mysql,vec![my_lock,format!("SET @navkit_backup_token = '{nonce}'")]);
    assert!(!mysql.iter().any(|sql|sql.contains("START TRANSACTION") || sql.contains("SET TRANSACTION") || sql.contains("FLUSH")));
    assert!(startup_plan(Mode::Compat,"unused",&nonce).is_err());
    assert!(startup_plan(Mode::MysqlLocked,"unused","'; UNLOCK TABLES; --").is_err());
}

#[tokio::test]
async fn dedicated_pool_plan_never_recycles_by_age_or_idle() {
    let pg = sqlx::postgres::PgPoolOptions::new().connect_lazy("postgres://unused:unused@127.0.0.1:1/unused").unwrap();
    let my = sqlx::mysql::MySqlPoolOptions::new().connect_lazy("mysql://unused:unused@127.0.0.1:1/unused").unwrap();
    let dedicated_pg = pg_pool(&pg); let dedicated_my = mysql_pool(&my);
    assert_eq!(dedicated_pg.options().get_max_connections(),1);
    assert_eq!(dedicated_my.options().get_max_connections(),1);
    assert!(dedicated_pg.options().get_max_lifetime().is_none() && dedicated_pg.options().get_idle_timeout().is_none());
    assert!(dedicated_my.options().get_max_lifetime().is_none() && dedicated_my.options().get_idle_timeout().is_none());
    dedicated_pg.close().await; dedicated_my.close().await; pg.close().await; my.close().await;
}

#[test]
fn missing_changed_nonce_cleanup_or_lost_reader_prevents_stage_publication() {
    let directory = TestDirectory::new().unwrap();
    let scope = Scope::new(Mode::MysqlLocked,"s",vec!["a".into(),"b".into()]);
    let failures = [verify_nonce(None,&scope.nonce),verify_nonce(Some("changed"),&scope.nonce),scope.verify_readers(true)];
    for (index, failure) in failures.into_iter().enumerate() {
        let target = directory.path().join(format!("failed-{index}.sql"));
        let stage = StagedOutput::new(target.to_str().unwrap()).unwrap();
        assert!(finish_stage(stage,failure.map(|_|report()),Ok(())).is_err());
        assert!(!target.exists());
    }
    let target = directory.path().join("cleanup-failed.sql");
    let stage = StagedOutput::new(target.to_str().unwrap()).unwrap();
    let error = finish_stage(stage,Ok(report()),Err(anyhow!("mock physical close/rollback/unlock failed"))).unwrap_err();
    assert!(format!("{error:#}").contains("TRANSFER_EFFECTS_UNKNOWN"));
    assert!(!target.exists());
    let target = directory.path().join("verified.sql");
    let stage = StagedOutput::new(target.to_str().unwrap()).unwrap();
    let mut writer = stage.encoder("utf8").unwrap(); writer.write_all(b"-- verified").unwrap(); writer.finish().unwrap().get_ref().sync_all().unwrap();
    assert!(finish_stage(stage,Ok(report()),Ok(())).is_ok());
    assert!(target.exists());
}

#[tokio::test]
async fn mysql_shared_reader_accounting_detects_scope_loss_and_wrong_object() {
    let scope = Arc::new(Scope::new(Mode::MysqlLocked,"s",vec!["a".into(),"b".into()]));
    assert!(consistency_label().is_none());
    BACKUP_SCOPE.scope(scope.clone(),async {
        assert!(consistency_label().unwrap().contains("隐式锁关联表"));
        assert!(consistency_label().unwrap().contains("阻塞业务读写"));
        assert!(scope.check_reader("other","a").is_err());
        assert!(scope.check_reader("s","view").is_err());
        scope.check_reader("s","a").unwrap();
        // The second reader accidentally used the compatibility branch: no scope counter increment.
        assert!(scope.verify_readers(true).is_err());
        scope.check_reader("s","b").unwrap();
        assert!(scope.verify_readers(true).is_ok());
        scope.check_reader("s","b").unwrap();
        assert!(scope.verify_readers(true).is_err());
    }).await;
    assert!(consistency_label().is_none());
}

#[tokio::test]
async fn mismatched_native_profile_rejects_without_connecting_or_publishing() {
    let directory = TestDirectory::new().unwrap(); let file = directory.path().join("never.sql");
    let pool = sqlx::mysql::MySqlPoolOptions::new().connect_lazy("mysql://unused:unused@127.0.0.1:1/unused").unwrap();
    let db = Db::Mysql(pool.clone());
    let error = crate::transfer::dump_database_with_consistency(&db,&DbKind::of("tidb"),"s",&[],&[],true,true,file.to_str().unwrap(),"",false,Some("mysql_locked")).await.unwrap_err();
    assert!(format!("{error:#}").contains("BACKUP_CONSISTENCY_UNSUPPORTED"));
    assert!(!file.exists()); assert_eq!(pool.size(),0); pool.close().await;
}

#[tokio::test]
async fn default_and_explicit_compat_keep_sqlite_snapshot_roundtrip() -> Result<()> {
    let directory = TestDirectory::new()?;
    let source = directory.path().join("source.sqlite");
    let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(&source).create_if_missing(true)).await?;
    pool.execute("CREATE TABLE t(id INTEGER PRIMARY KEY,v TEXT,b BLOB); INSERT INTO t VALUES(1,'中文',X'00FF80')").await?;
    let db = Db::Sqlite(pool.clone()); let kind = DbKind::of("sqlite");
    for (index,policy) in [None,Some("compat")].into_iter().enumerate() {
        let file = directory.path().join(format!("compat-{index}.sql"));
        let report = crate::transfer::dump_database_with_consistency(&db,&kind,"main",&[],&[],true,true,file.to_str().unwrap(),"",false,policy).await?;
        assert_eq!(report.rows,1); assert_eq!(report.bytes,std::fs::metadata(&file)?.len());
        let text = std::fs::read_to_string(file)?;
        assert!(text.contains("SQLite 同一只读事务"));
        let target = sqlx::SqlitePool::connect("sqlite::memory:").await?;
        target.execute(text.as_str()).await?;
        assert_eq!(crate::sql_history::query_scalar::<_,Vec<u8>>("SELECT b FROM t").fetch_one(&target).await?,vec![0,255,128]);
        assert_eq!(crate::sql_history::query_scalar::<_,String>("SELECT v FROM t").fetch_one(&target).await?,"中文");
        target.close().await;
    }
    pool.close().await; Ok(())
}

#[tokio::test]
#[ignore = "requires explicit NAVKIT_PERF_PG_URL; validates nondefault storage/compression rejection"]
async fn live_pg_nondefault_storage_is_rejected_instead_of_silently_restored() -> Result<()> {
    let url=std::env::var("NAVKIT_PERF_PG_URL").context("explicit isolated PostgreSQL URL required")?;
    let admin=sqlx::PgPool::connect(&url).await?;
    let schema=format!("navkit_storage_{}",uuid::Uuid::new_v4().simple());let kind=DbKind::of("postgresql");
    let table=qualified(&kind,&schema,"t");let directory=TestDirectory::new()?;
    admin.execute(format!("CREATE SCHEMA {}; CREATE TABLE {table}(v text)",quote_ident(&kind,&schema)).as_str()).await?;
    let result:Result<()>=async {
        let control=directory.path().join("default.sql");
        crate::transfer::dump_database_with_consistency(&Db::Postgres(admin.clone()),&kind,&schema,&["t".into()],&[],true,true,control.to_str().unwrap(),"",false,Some("pg_snapshot")).await?;
        for (index,alter) in ["SET STORAGE EXTERNAL","SET COMPRESSION pglz"].into_iter().enumerate() {
            admin.execute(format!("ALTER TABLE {table} ALTER COLUMN v {alter}").as_str()).await?;
            let output=directory.path().join(format!("rejected-{index}.sql"));
            anyhow::ensure!(crate::transfer::dump_database_with_consistency(&Db::Postgres(admin.clone()),&kind,&schema,&["t".into()],&[],true,true,output.to_str().unwrap(),"",false,Some("pg_snapshot")).await.is_err(),"nondefault {alter} must not be declared lossless");
            anyhow::ensure!(!output.exists(),"unsupported structure must not be published");
            admin.execute(format!("ALTER TABLE {table} ALTER COLUMN v SET STORAGE EXTENDED").as_str()).await?;
        }
        Ok(())
    }.await;
    let cleanup=admin.execute(format!("DROP SCHEMA {} CASCADE",quote_ident(&kind,&schema)).as_str()).await;admin.close().await;cleanup?;result
}

#[tokio::test]
#[ignore = "requires explicit NAVKIT_PERF_MYSQL_URL; validates known trigger/FK lock expansion gate"]
async fn live_mysql_related_trigger_is_rejected_before_locking_source() -> Result<()> {
    let url=std::env::var("NAVKIT_PERF_MYSQL_URL").context("explicit isolated MySQL URL required")?;
    let admin=sqlx::mysql::MySqlPoolOptions::new().max_connections(2).connect(&url).await?;
    let schema=format!("navkit_locks_{}",uuid::Uuid::new_v4().simple());let kind=DbKind::of("mysql");let directory=TestDirectory::new()?;
    admin.execute(format!("CREATE DATABASE {}",quote_ident(&kind,&schema)).as_str()).await?;
    let result:Result<()>=async {
        admin.execute(format!("CREATE TABLE {}(v int) ENGINE=InnoDB; CREATE TABLE {}(v int) ENGINE=InnoDB; CREATE TRIGGER {} AFTER INSERT ON {} FOR EACH ROW INSERT INTO {} VALUES(NEW.v)",qualified(&kind,&schema,"t"),qualified(&kind,&schema,"audit"),qualified(&kind,&schema,"tr"),qualified(&kind,&schema,"t"),qualified(&kind,&schema,"audit")).as_str()).await?;
        let output=directory.path().join("no-implicit-locks.sql");
        let error=crate::transfer::dump_database_with_consistency(&Db::Mysql(admin.clone()),&kind,&schema,&["t".into()],&[],true,true,output.to_str().unwrap(),"",false,Some("mysql_locked")).await.unwrap_err();
        anyhow::ensure!(format!("{error:#}").contains("BACKUP_LOCK_DEPENDENCY_UNSUPPORTED"),"expected dependency gate, not another failure");
        anyhow::ensure!(!output.exists(),"unsafe source locks must not publish");
        let mut second=admin.acquire().await?;second.execute("SET SESSION lock_wait_timeout=1").await?;
        second.execute(format!("INSERT INTO {} VALUES(1)",qualified(&kind,&schema,"audit")).as_str()).await?;
        let n:i64=crate::sql_history::query_scalar(&format!("SELECT COUNT(*) FROM {}",qualified(&kind,&schema,"audit"))).fetch_one(&mut *second).await?;
        anyhow::ensure!(n==1,"related audit remains writable/readable after rejection");drop(second);Ok(())
    }.await;
    let cleanup=admin.execute(format!("DROP DATABASE {}",quote_ident(&kind,&schema)).as_str()).await;admin.close().await;cleanup?;result
}

// Native acceptance is intentionally ignored. These tests create only UUID resources
// in NAVKIT_PERF_*_URL, never saved connection configs or inferred local/production URLs.
#[tokio::test]
#[ignore = "requires explicit NAVKIT_PERF_PG_URL disposable database; not local acceptance"]
async fn live_pg_same_snapshot_concurrent_write_ddl_and_nonce_loss() -> Result<()> {
    let url = std::env::var("NAVKIT_PERF_PG_URL").context("set NAVKIT_PERF_PG_URL to an authorized disposable DB")?;
    let admin = sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await?;
    let schema = format!("navkit_backup_{}",uuid::Uuid::new_v4().simple());
    let kind = DbKind::of("postgresql");
    admin.execute(format!("CREATE SCHEMA {}",quote_ident(&kind,&schema)).as_str()).await?;
    let result: Result<()> = async {
        for table in ["a","b"] {
            admin.execute(format!("CREATE TABLE {} (id integer NOT NULL, v integer NOT NULL); INSERT INTO {} SELECT i,0 FROM generate_series(1,128) i",qualified(&kind,&schema,table),qualified(&kind,&schema,table)).as_str()).await?;
        }
        let pool = pg_pool(&admin); let db = Db::Postgres(pool.clone());
        let body: Result<()> = async {
            let discovered = pg_catalog(&pool,&schema,&[]).await?;
            pool.execute(PG_BEGIN).await?;
            let scope = Arc::new(Scope::new(Mode::PgSnapshot,&schema,Vec::new()));
            pool.execute(format!("SET LOCAL navkit.backup_token='{}'",scope.nonce).as_str()).await?;
            pool.execute(pg_lock_sql(&kind,&schema,&discovered)?.as_str()).await?;
            let objects = pg_catalog(&pool,&schema,&[]).await?;
            anyhow::ensure!(objects == discovered,"actual locked catalog drifted");
            pg_lock_and_prove(&pool,&kind,&schema,&objects,true).await?;
            // A concurrent transaction updates both tables AFTER the backup snapshot/locks.
            admin.execute(format!("BEGIN; UPDATE {} SET v=1; UPDATE {} SET v=1; COMMIT",qualified(&kind,&schema,"a"),qualified(&kind,&schema,"b")).as_str()).await?;
            let mut ddl = admin.acquire().await?;
            ddl.execute("SET lock_timeout='100ms'").await?;
            anyhow::ensure!(ddl.execute(format!("ALTER TABLE {} ADD COLUMN concurrent integer",qualified(&kind,&schema,"a")).as_str()).await.is_err(),"concurrent DDL must be blocked");
            ddl.close().await?;
            let directory = TestDirectory::new()?; let file = directory.path().join("snapshot.sql");
            let stage = StagedOutput::new(file.to_str().unwrap())?;
            let report = BACKUP_SCOPE.scope(scope.clone(),super::super::dump_database_to_stage(&db,&kind,&schema,&[],&[],true,true,"",false,&stage)).await?;
            anyhow::ensure!(report.rows==256,"expected both complete tables");
            check_staged_values(&stage,", 0);",256)?;
            let nonce: Option<String> = crate::sql_history::query_scalar("SELECT current_setting('navkit.backup_token',true)").fetch_one(&pool).await?;
            verify_nonce(nonce.as_deref(),&scope.nonce)?;
            anyhow::ensure!(pg_catalog(&pool,&schema,&[]).await? == objects,"locked catalog changed");
            // Ending the transaction must invalidate SET LOCAL, even on the same socket.
            pool.execute("ROLLBACK").await?;
            let nonce: Option<String> = crate::sql_history::query_scalar("SELECT current_setting('navkit.backup_token',true)").fetch_one(&pool).await?;
            anyhow::ensure!(finish_stage(stage,verify_nonce(nonce.as_deref(),&scope.nonce).map(|_|report),Ok(())).is_err(),"ended transaction must fail publication");
            anyhow::ensure!(!file.exists(),"failed backup was published");
            // Exercise the actual explicit wrapper and successful publish, now at v=1.
            let file = directory.path().join("published.sql");
            let report = crate::transfer::dump_database_with_consistency(&Db::Postgres(admin.clone()),&kind,&schema,&[],&[],true,true,file.to_str().unwrap(),"",false,Some("pg_snapshot")).await?;
            anyhow::ensure!(report.rows==256,"expected both complete tables");
            let text = std::fs::read_to_string(file)?;
            anyhow::ensure!(text.lines().filter(|line|line.starts_with("INSERT INTO")).all(|line|line.ends_with(", 1);")),"published values differ");
            Ok(())
        }.await;
        // Test resources must be released even if an assertion/query fails.
        let _ = pool.execute("ROLLBACK").await; pool.close().await; body
    }.await;
    let cleanup = admin.execute(format!("DROP SCHEMA {} CASCADE",quote_ident(&kind,&schema)).as_str()).await;
    admin.close().await;
    cleanup?; result
}

#[tokio::test]
#[ignore = "requires explicit NAVKIT_PERF_MYSQL_URL disposable database; not local acceptance"]
async fn live_mysql_lock_blocks_writes_ddl_and_preserves_one_reader_session() -> Result<()> {
    let url = std::env::var("NAVKIT_PERF_MYSQL_URL").context("set NAVKIT_PERF_MYSQL_URL to an authorized disposable DB")?;
    let admin = sqlx::mysql::MySqlPoolOptions::new().max_connections(2).connect(&url).await?;
    let schema = format!("navkit_backup_{}",uuid::Uuid::new_v4().simple()); let kind = DbKind::of("mysql");
    admin.execute(format!("CREATE DATABASE {}",quote_ident(&kind,&schema)).as_str()).await?;
    let result: Result<()> = async {
        for table in ["a","b"] {
            admin.execute(format!("CREATE TABLE {} (id INT PRIMARY KEY,v INT NOT NULL) ENGINE=InnoDB",qualified(&kind,&schema,table)).as_str()).await?;
            admin.execute(format!("INSERT INTO {} VALUES(1,0)",qualified(&kind,&schema,table)).as_str()).await?;
        }
        let pool = mysql_pool(&admin); let db = Db::Mysql(pool.clone());
        let body: Result<()> = async {
            let row = crate::sql_history::query("SELECT @@version,@@version_comment").fetch_one(&pool).await?;
            mysql_version(&row.try_get::<String,_>(0)?,&row.try_get::<String,_>(1)?)?;
            let objects = mysql_catalog(&pool,&schema,&[]).await?;
            let scope = Arc::new(Scope::new(Mode::MysqlLocked,&schema,objects.iter().map(|o|o.name.clone()).collect()));
            let mut source = pool.acquire().await?;
            source.execute(mysql_lock_sql(&kind,&schema,&objects)?.as_str()).await?;
            crate::sql_history::query("SET @navkit_backup_token=?").bind(&scope.nonce).execute(&mut *source).await?;
            drop(source);
            let mut writer = admin.acquire().await?;
            writer.execute("SET SESSION lock_wait_timeout=1").await?;
            anyhow::ensure!(writer.execute(format!("UPDATE {} SET v=1",qualified(&kind,&schema,"a")).as_str()).await.is_err(),"source writer must be blocked");
            anyhow::ensure!(writer.execute(format!("ALTER TABLE {} ADD COLUMN concurrent INT",qualified(&kind,&schema,"b")).as_str()).await.is_err(),"source DDL must be blocked");
            writer.close().await?;
            let directory = TestDirectory::new()?; let file = directory.path().join("locked.sql"); let stage = StagedOutput::new(file.to_str().unwrap())?;
            let report = BACKUP_SCOPE.scope(scope.clone(),super::super::dump_database_to_stage(&db,&kind,&schema,&[],&[],true,true,"",false,&stage)).await?;
            anyhow::ensure!(report.rows==2,"expected both complete tables"); scope.verify_readers(true)?;
            check_staged_values(&stage,", 0);",2)?;
            let token: Option<String> = crate::sql_history::query_scalar("SELECT CAST(@navkit_backup_token AS CHAR)").fetch_one(&pool).await?;
            verify_nonce(token.as_deref(),&scope.nonce)?;
            // Simulate an errored cursor that really closed the source socket. Replacement must fail final verification.
            pool.acquire().await?.close().await?;
            let token: Option<String> = crate::sql_history::query_scalar("SELECT CAST(@navkit_backup_token AS CHAR)").fetch_one(&pool).await?;
            anyhow::ensure!(finish_stage(stage,verify_nonce(token.as_deref(),&scope.nonce).map(|_|report),Ok(())).is_err(),"reconnected source must fail publication");
            anyhow::ensure!(!file.exists(),"failed backup was published");
            let file = directory.path().join("published.sql");
            let report = crate::transfer::dump_database_with_consistency(&Db::Mysql(admin.clone()),&kind,&schema,&[],&[],true,true,file.to_str().unwrap(),"",false,Some("mysql_locked")).await?;
            anyhow::ensure!(report.rows==2,"expected both complete tables");
            anyhow::ensure!(std::fs::read_to_string(file)?.lines().filter(|line|line.starts_with("INSERT INTO")).all(|line|line.ends_with(", 0);")),"published values differ");
            Ok(())
        }.await;
        let _ = pool.execute(MYSQL_UNLOCK).await; pool.close().await; body
    }.await;
    let cleanup = admin.execute(format!("DROP DATABASE {}",quote_ident(&kind,&schema)).as_str()).await;
    admin.close().await;
    cleanup?; result
}
