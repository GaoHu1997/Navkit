//! 真实 SQLx SQLite + C1 owned 通道重建回归；不冒充 UI/IPC 逐项确认验收。
use super::*;
use crate::{execution::{Binding, Registry}, execution_contracts::{ExecutionRequest, ExecutionState}, manual_transaction};
use sqlx::{sqlite::{SqliteConnectOptions, SqlitePoolOptions}, SqlitePool};

fn request(drop: bool) -> StructureSyncRequest {
    StructureSyncRequest { source_connection: "source".into(), source_schema: "main".into(), source_table: "src".into(), target_connection: "target".into(), target_schema: "main".into(), target_table: "dst".into(), columns: Vec::new(), allow_drop: drop, source_revision: String::new(), target_revision: String::new(), review_digest: String::new() }
}
fn binding() -> StructureReviewBinding {
    StructureReviewBinding { principal: "window".into(), owner: "page".into(), source_connection_id: "source".into(), source_fingerprint: "a".repeat(64), source_schema: "main".into(), source_table: "src".into(), target_connection_id: "target".into(), target_fingerprint: "b".repeat(64), target_schema: "main".into(), target_table: "dst".into() }
}
async fn pool() -> SqlitePool { SqlitePoolOptions::new().max_connections(1).connect_with(SqliteConnectOptions::new().in_memory(true).foreign_keys(true)).await.unwrap() }
async fn exec(pool: &SqlitePool, sql: &str) { crate::sql_history::query(sql).execute(pool).await.unwrap(); }
async fn catalog(pool: &SqlitePool) -> Vec<(String,String,String)> {
    crate::sql_history::query_as("SELECT type,name,sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY type,name").fetch_all(pool).await.unwrap()
}
async fn preview_pair(source: &SqlitePool, target: &SqlitePool, drop: bool) -> metadata::StructurePreflight {
    let gates = manual_transaction::Registry::default(); let ordinary = gates.ordinary(&["source","target"]).await.unwrap();
    metadata::preflight(Arc::new(Db::Sqlite(source.clone())), DbKind::of("sqlite"), Arc::new(Db::Sqlite(target.clone())), DbKind::of("sqlite"), ordinary, request(drop), binding(), false).await.unwrap()
}
async fn reviewed(source: &SqlitePool, target: &SqlitePool, drop: bool) -> (StructureSyncRequest, StructureDiffReport) {
    let preview = preview_pair(source,target,drop).await;
    assert!(preview.report.alterable, "{:?}", preview.report.unsupported); assert!(!preview.request_sql.is_empty());
    let mut request = request(drop); request.source_revision = preview.report.source_revision.clone(); request.target_revision = preview.report.target_revision.clone(); request.review_digest = preview.report.review_digest.clone();
    (request, preview.report)
}
fn entry(request: &StructureSyncRequest) -> Arc<Entry> {
    let binding = Binding { principal: "window".into(), owner: "page".into(), connection: "target".into(), namespace: "main".into(), fingerprint: "b".repeat(64) };
    let registry = Registry::default();
    let mut execution = ExecutionRequest { execution_id: String::new(), connection_id: "target".into(), namespace: "main".into(), owner_id: "page".into(), sql: structure_sync::review_request_sql(request).unwrap(), transaction_owner: None, timeout_ms: Some(10_000), origin: Some("structure_sync".into()), execution_grant: None, cancel_before_send: false };
    registry.grant_request(&mut execution,binding.clone()).unwrap(); registry.register(&execution,binding,0).unwrap()
}
async fn call(source: &SqlitePool, target: &SqlitePool, request: StructureSyncRequest, mode: TestMode) -> (Arc<Entry>, BoundResult) {
    let gates = manual_transaction::Registry::default(); let ordinary = gates.ordinary(&["source","target"]).await.unwrap(); let entry = entry(&request);
    let result = run_mode(Arc::new(Db::Sqlite(source.clone())),DbKind::of("sqlite"),Arc::new(Db::Sqlite(target.clone())),DbKind::of("sqlite"),entry.clone(),ordinary,request,binding(),false,mode).await;
    (entry,result)
}
async fn rich_fixture() -> SqlitePool {
    let pool = pool().await;
    exec(&pool,"CREATE TABLE src(a INTEGER NOT NULL DEFAULT 9,added TEXT DEFAULT 'New  Case')").await;
    exec(&pool,"CREATE TABLE dst(a TEXT CHECK(a>=0),extra BLOB DEFAULT X'00')").await;
    exec(&pool,"CREATE INDEX extra_idx ON dst(extra,a)").await;
    exec(&pool,"CREATE TABLE audit(v TEXT)").await;
    exec(&pool,"CREATE TRIGGER dst_audit AFTER INSERT ON dst BEGIN INSERT INTO audit(v) VALUES('Trigger  Case'); END").await;
    exec(&pool,"CREATE VIEW dst_view AS SELECT a FROM dst").await;
    exec(&pool,"CREATE VIEW chained_view AS SELECT a FROM dst_view").await;
    exec(&pool,"CREATE TRIGGER audit_update AFTER UPDATE ON audit BEGIN UPDATE dst SET a=a WHERE a=NEW.v; END").await;
    exec(&pool,"INSERT INTO dst(rowid,a,extra) VALUES(42,'7',X'0001FF')").await;
    pool
}

#[test]
fn ddl_parser_preserves_references_but_not_old_following_null_default() {
    let constraints = sqlite_rebuild::ordinary_constraints("a INT REFERENCES parent(id) ON DELETE CASCADE NOT NULL DEFAULT 3 CHECK(a>0)",true).unwrap();
    assert_eq!(constraints,vec!["REFERENCES parent(id) ON DELETE CASCADE","CHECK(a>0)"]);
}

#[tokio::test]
async fn rebuild_changes_type_null_default_adds_column_and_preserves_rowid_blob_check_triggers_views() {
    let pool = rich_fixture().await;
    let (request,report) = reviewed(&pool,&pool,false).await;
    assert!(report.steps.iter().any(|s| s.sql.starts_with("DROP TABLE")));
    assert!(report.changes.iter().any(|c| c.kind=="alter_column" && c.risk=="scan_required"));
    let dependencies: Vec<_> = catalog(&pool).await.into_iter().filter(|r| r.0=="view" || r.0=="trigger").collect();
    let (entry,result) = call(&pool,&pool,request,TestMode::Normal).await;
    assert!(result.result.is_ok(),"{:?}",result.result.as_ref().err());
    let receipt = result.receipt.as_ref().unwrap(); assert!(receipt.receipt.verified); assert_eq!(receipt.receipt.transaction,"committed");
    let row: (i64,i64,Vec<u8>,String) = crate::sql_history::query_as("SELECT rowid,a,extra,added FROM dst").fetch_one(&pool).await.unwrap();
    assert_eq!(row,(42,7,vec![0,1,255],"New  Case".into()));
    let audit: i64 = crate::sql_history::query_scalar("SELECT count(*) FROM audit").fetch_one(&pool).await.unwrap(); assert_eq!(audit,1,"复制不执行原触发器");
    assert_eq!(catalog(&pool).await.into_iter().filter(|r|r.0=="view" || r.0=="trigger").collect::<Vec<_>>(),dependencies);
    let view: i64 = crate::sql_history::query_scalar("SELECT a FROM chained_view").fetch_one(&pool).await.unwrap(); assert_eq!(view,7);
    assert!(crate::sql_history::query("INSERT INTO dst(a) VALUES(-1)").execute(&pool).await.is_err());
    exec(&pool,"INSERT INTO dst(extra) VALUES(X'01')").await;
    let default: i64 = crate::sql_history::query_scalar("SELECT a FROM dst WHERE rowid!=42").fetch_one(&pool).await.unwrap(); assert_eq!(default,9);
    assert_eq!(entry.finish(result.result,result.confirmed,false).state,ExecutionState::Succeeded);
}

#[tokio::test]
async fn drop_extra_column_and_index_are_explicit_review_changes_and_preserve_retained_views() {
    let pool = pool().await;
    exec(&pool,"CREATE TABLE src(a INT)").await; exec(&pool,"CREATE TABLE dst(a INT,extra TEXT)").await;
    exec(&pool,"CREATE INDEX extra_idx ON dst(extra)").await; exec(&pool,"CREATE VIEW kept_view AS SELECT a FROM dst").await;
    exec(&pool,"INSERT INTO dst(rowid,a,extra) VALUES(99,5,'remove')").await;
    let preview = preview_pair(&pool,&pool,false).await; assert!(preview.report.changes.is_empty()); assert!(preview.report.steps.is_empty());
    let (request,report) = reviewed(&pool,&pool,true).await;
    for kind in ["drop_column","drop_index"] { assert!(report.changes.iter().any(|c|c.kind==kind && c.risk=="destructive")); }
    let (_,result) = call(&pool,&pool,request,TestMode::Normal).await; assert!(result.result.is_ok(),"{:?}",result.result.as_ref().err());
    let row: (i64,i64) = crate::sql_history::query_as("SELECT rowid,a FROM dst").fetch_one(&pool).await.unwrap(); assert_eq!(row,(99,5));
    let idx: i64 = crate::sql_history::query_scalar("SELECT count(*) FROM sqlite_schema WHERE name='extra_idx'").fetch_one(&pool).await.unwrap(); assert_eq!(idx,0);
    let view: i64 = crate::sql_history::query_scalar("SELECT a FROM kept_view").fetch_one(&pool).await.unwrap(); assert_eq!(view,5);
}

#[tokio::test]
async fn index_only_drop_is_reviewed_without_table_rebuild() {
    let pool = pool().await; exec(&pool,"CREATE TABLE src(a INT)").await; exec(&pool,"CREATE TABLE dst(a INT)").await; exec(&pool,"CREATE INDEX unwanted ON dst(a)").await;
    let (request,report) = reviewed(&pool,&pool,true).await; assert_eq!(report.changes.len(),1); assert_eq!(report.changes[0].kind,"drop_index"); assert_eq!(report.steps.len(),1);
    let (_,result) = call(&pool,&pool,request,TestMode::Normal).await; assert!(result.result.is_ok(),"{:?}",result.result.as_ref().err());
}

#[tokio::test]
async fn unique_autoindex_only_changes_are_explicitly_unsupported_before_ddl() {
    for (source,target) in [("CREATE TABLE src(a TEXT)","CREATE TABLE dst(a TEXT UNIQUE)"),("CREATE TABLE src(a TEXT UNIQUE)","CREATE TABLE dst(a TEXT)")] {
        let pool=pool().await; exec(&pool,source).await; exec(&pool,target).await;
        let before=catalog(&pool).await; let preview=preview_pair(&pool,&pool,true).await;
        assert!(!preview.report.alterable); assert!(preview.report.steps.is_empty()); assert!(preview.request_sql.is_empty());
        assert!(preview.report.unsupported.join(" ").contains("CONSTRAINT_DEPENDENCY") || preview.report.unsupported.join(" ").contains("AUTO_INDEX_UNSUPPORTED")); assert_eq!(catalog(&pool).await,before);
    }
}

#[tokio::test]
async fn ordered_primary_key_change_requires_drop_approval_and_preserves_rowid() {
    let pool = pool().await; exec(&pool,"CREATE TABLE src(a TEXT NOT NULL,b TEXT NOT NULL,PRIMARY KEY(b,a))").await;
    exec(&pool,"CREATE TABLE dst(a TEXT NOT NULL,b TEXT NOT NULL,PRIMARY KEY(a,b))").await; exec(&pool,"INSERT INTO dst(rowid,a,b) VALUES(51,'A','B')").await;
    assert!(!preview_pair(&pool,&pool,false).await.report.alterable);
    let (request,_) = reviewed(&pool,&pool,true).await; let (_,result) = call(&pool,&pool,request,TestMode::Normal).await; assert!(result.result.is_ok(),"{:?}",result.result.as_ref().err());
    let schema = result.receipt.unwrap().receipt.schema.unwrap(); assert_eq!(design::ordered_primary_key(&schema).unwrap(),vec!["b","a"]);
    let rowid: i64 = crate::sql_history::query_scalar("SELECT rowid FROM dst").fetch_one(&pool).await.unwrap(); assert_eq!(rowid,51);
}

#[tokio::test]
async fn integer_primary_key_must_equal_old_rowid_and_duplicate_pk_is_atomic_failure() {
    for (source,rows,success) in [
        ("CREATE TABLE src(a INTEGER PRIMARY KEY)","INSERT INTO dst(rowid,a) VALUES(7,7)",true),
        ("CREATE TABLE src(a INTEGER PRIMARY KEY)","INSERT INTO dst(rowid,a) VALUES(7,8)",false),
        ("CREATE TABLE src(a INT PRIMARY KEY)","INSERT INTO dst(rowid,a) VALUES(7,8),(9,8)",false),
    ] {
        let pool=pool().await; exec(&pool,source).await; exec(&pool,"CREATE TABLE dst(a INT)").await; exec(&pool,rows).await;
        let before=catalog(&pool).await; let (request,_) = reviewed(&pool,&pool,false).await; let (_,result)=call(&pool,&pool,request,TestMode::Normal).await;
        assert_eq!(result.result.is_ok(),success,"{:?}",result.result.as_ref().err());
        if !success { assert_eq!(catalog(&pool).await,before); assert_eq!(result.receipt.unwrap().receipt.transaction,"rolled_back"); }
    }
}

#[tokio::test]
async fn dependency_deletion_rejected_before_any_ddl() {
    for dependency in [
        "CREATE VIEW dependent AS SELECT extra FROM dst",
        "CREATE VIEW dependent AS SELECT * FROM dst",
        "CREATE TRIGGER dependent AFTER UPDATE ON dst BEGIN SELECT NEW.extra; END",
        "CREATE TABLE child(id INT REFERENCES dst(a) ON DELETE CASCADE)",
    ] {
        let pool=pool().await; exec(&pool,"CREATE TABLE src(a INT PRIMARY KEY)").await; exec(&pool,"CREATE TABLE dst(a INT PRIMARY KEY,extra TEXT)").await; exec(&pool,dependency).await;
        exec(&pool,"INSERT INTO dst(rowid,a,extra) VALUES(17,3,'Keep')").await;
        let before=catalog(&pool).await; let preview=preview_pair(&pool,&pool,true).await;
        assert!(!preview.report.alterable,"{dependency}"); assert!(preview.report.steps.is_empty()); assert!(preview.request_sql.is_empty());
        assert!(preview.report.unsupported.join(" ").contains("DEPENDENCY") || preview.report.unsupported.join(" ").contains("FOREIGN_KEY")); assert_eq!(catalog(&pool).await,before);
    }
}

#[tokio::test]
async fn check_and_outgoing_fk_dependency_deletion_rejected() {
    for ddl in ["CREATE TABLE dst(a INT,extra INT CHECK(extra>=0))","CREATE TABLE dst(a INT,extra INT REFERENCES parent(id))"] {
        let pool=pool().await; exec(&pool,"CREATE TABLE parent(id INT PRIMARY KEY)").await; exec(&pool,"CREATE TABLE src(a INT)").await; exec(&pool,ddl).await;
        let before=catalog(&pool).await; let preview=preview_pair(&pool,&pool,true).await;
        assert!(!preview.report.alterable); assert!(preview.report.steps.is_empty()); assert_eq!(catalog(&pool).await,before);
    }
}

#[tokio::test]
async fn outgoing_fk_and_following_check_preserved_while_null_default_change() {
    let pool=pool().await; exec(&pool,"CREATE TABLE parent(id INT PRIMARY KEY)").await; exec(&pool,"INSERT INTO parent VALUES(1)").await;
    exec(&pool,"CREATE TABLE src(a INT DEFAULT 1)").await; exec(&pool,"CREATE TABLE dst(a INT REFERENCES parent(id) ON DELETE CASCADE NOT NULL DEFAULT 2 CHECK(a>0))").await;
    exec(&pool,"INSERT INTO dst(rowid,a) VALUES(10,1)").await;
    let (request,_) = reviewed(&pool,&pool,false).await; let (_,result)=call(&pool,&pool,request,TestMode::Normal).await; assert!(result.result.is_ok(),"{:?}",result.result.as_ref().err());
    let schema=result.receipt.unwrap().receipt.schema.unwrap(); assert!(schema.columns[0].nullable); assert_eq!(schema.columns[0].default.as_deref(),Some("1")); assert_eq!(schema.foreign_keys.len(),1); assert_eq!(schema.foreign_keys[0].on_delete.as_deref(),Some("CASCADE"));
    assert!(crate::sql_history::query("INSERT INTO dst(a) VALUES(999)").execute(&pool).await.is_err());
}

#[tokio::test]
async fn invalid_or_lossy_conversions_nulls_and_length_never_commit_or_truncate() {
    for (old_type,new_type,value) in [
        ("TEXT","INTEGER","'12x'"), ("TEXT","INTEGER","'01'"), ("TEXT","INTEGER","' 1 '"),
        ("INTEGER","REAL","9007199254740993"), ("REAL","INTEGER","9223372036854775808.0"),
        ("REAL","INTEGER","1.5"), ("REAL","TEXT","1.2345678901234567"),
        ("TEXT","VARCHAR(2)","'long'"), ("NUMERIC","DECIMAL(3,1)","1.25"), ("TEXT","TEXT NOT NULL","NULL"),
    ] {
        let pool=pool().await; exec(&pool,&format!("CREATE TABLE src(a {new_type})")).await; exec(&pool,&format!("CREATE TABLE dst(a {old_type})")).await;
        exec(&pool,&format!("INSERT INTO dst(rowid,a) VALUES(71,{value})")).await;
        let before=catalog(&pool).await; let original: (i64,String,String) = crate::sql_history::query_as("SELECT rowid,typeof(a),quote(a) FROM dst").fetch_one(&pool).await.unwrap();
        let (request,_) = reviewed(&pool,&pool,false).await; let (_,result)=call(&pool,&pool,request,TestMode::Normal).await;
        assert!(result.result.is_err(),"{old_type}->{new_type} {value}"); assert_eq!(result.receipt.unwrap().receipt.transaction,"rolled_back"); assert_eq!(catalog(&pool).await,before);
        let after: (i64,String,String) = crate::sql_history::query_as("SELECT rowid,typeof(a),quote(a) FROM dst").fetch_one(&pool).await.unwrap(); assert_eq!(after,original);
    }
}

#[tokio::test]
async fn current_timestamp_addition_and_decimal_blob_defaults_rebuild_safely() {
    let pool=pool().await; exec(&pool,"CREATE TABLE src(a REAL DEFAULT 1.5,added TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP)").await;
    exec(&pool,"CREATE TABLE dst(a REAL DEFAULT 2.5,extra BLOB DEFAULT X'00FF')").await; exec(&pool,"INSERT INTO dst(rowid,a) VALUES(29,2.5)").await;
    let (request,_) = reviewed(&pool,&pool,false).await; let (_,result)=call(&pool,&pool,request,TestMode::Normal).await;
    assert!(result.result.is_ok(),"{:?}",result.result.as_ref().err());
    let row: (i64,f64,Vec<u8>,String) = crate::sql_history::query_as("SELECT rowid,a,extra,added FROM dst").fetch_one(&pool).await.unwrap();
    assert_eq!((row.0,row.1,row.2),(29,2.5,vec![0,255])); assert_eq!(row.3.len(),19);
}

#[tokio::test]
async fn unchanged_numeric_text_blob_storage_and_unique_constraint_survive_default_rebuild() {
    let pool=pool().await; exec(&pool,"CREATE TABLE src(a NUMERIC DEFAULT 5)").await; exec(&pool,"CREATE TABLE dst(a NUMERIC UNIQUE)").await;
    exec(&pool,"INSERT INTO dst(rowid,a) VALUES(12,'not numeric'),(13,X'00FF')").await;
    let (request,_) = reviewed(&pool,&pool,false).await; let (_,result)=call(&pool,&pool,request,TestMode::Normal).await; assert!(result.result.is_ok(),"{:?}",result.result.as_ref().err());
    let rows: Vec<(i64,String,String)> = crate::sql_history::query_as("SELECT rowid,typeof(a),quote(a) FROM dst ORDER BY rowid").fetch_all(&pool).await.unwrap();
    assert_eq!(rows,vec![(12,"text".into(),"'not numeric'".into()),(13,"blob".into(),"X'00FF'".into())]);
}

#[tokio::test]
async fn inline_primary_key_preserves_unique_autoindex_identity_during_default_change() {
    let pool=pool().await; exec(&pool,"CREATE TABLE src(a TEXT PRIMARY KEY DEFAULT 'new',b TEXT)").await;
    exec(&pool,"CREATE TABLE dst(a TEXT PRIMARY KEY DEFAULT 'old',b TEXT UNIQUE)").await; exec(&pool,"INSERT INTO dst(rowid,a,b) VALUES(33,'keep','unique')").await;
    let (request,_) = reviewed(&pool,&pool,false).await; let (_,result)=call(&pool,&pool,request,TestMode::Normal).await;
    assert!(result.result.is_ok(),"{:?}",result.result.as_ref().err());
    let index: String = crate::sql_history::query_scalar("SELECT name FROM pragma_index_list('dst') WHERE origin='u'").fetch_one(&pool).await.unwrap(); assert_eq!(index,"sqlite_autoindex_dst_2");
}

#[tokio::test]
async fn rebuild_preserves_trigger_registration_order_on_target_and_external_parent() {
    let pool=pool().await; exec(&pool,"CREATE TABLE src(a INT DEFAULT 9)").await; exec(&pool,"CREATE TABLE dst(a INT DEFAULT 8)").await;
    exec(&pool,"CREATE TABLE audit(v TEXT)").await; exec(&pool,"CREATE TABLE input(v INT)").await;
    exec(&pool,"CREATE TRIGGER z_first AFTER INSERT ON dst BEGIN INSERT INTO audit(v) VALUES('z_first'); END").await;
    exec(&pool,"CREATE TRIGGER a_last AFTER INSERT ON dst BEGIN INSERT INTO audit(v) VALUES('a_last'); END").await;
    exec(&pool,"CREATE TRIGGER z_dependent AFTER INSERT ON input BEGIN INSERT INTO dst(a) VALUES(NEW.v); END").await;
    exec(&pool,"CREATE TRIGGER a_independent AFTER INSERT ON input BEGIN INSERT INTO audit(v) VALUES('independent'); END").await;
    exec(&pool,"INSERT INTO dst(a) VALUES(1)").await; exec(&pool,"INSERT INTO input(v) VALUES(2)").await;
    let before: Vec<String> = crate::sql_history::query_scalar("SELECT v FROM audit ORDER BY rowid").fetch_all(&pool).await.unwrap();
    let (request,_) = reviewed(&pool,&pool,false).await; let (_,result)=call(&pool,&pool,request,TestMode::Normal).await;
    assert!(result.result.is_ok(),"{:?}",result.result.as_ref().err());
    let copy_audit: Vec<String> = crate::sql_history::query_scalar("SELECT v FROM audit ORDER BY rowid").fetch_all(&pool).await.unwrap(); assert_eq!(copy_audit,before);
    exec(&pool,"DELETE FROM audit").await; exec(&pool,"INSERT INTO dst(a) VALUES(1)").await; exec(&pool,"INSERT INTO input(v) VALUES(2)").await;
    let after: Vec<String> = crate::sql_history::query_scalar("SELECT v FROM audit ORDER BY rowid").fetch_all(&pool).await.unwrap(); assert_eq!(after,before);
}

#[tokio::test]
async fn stop_and_failure_after_drop_rename_roll_back_all_data_and_dependency_catalog() {
    for mode in [TestMode::FailAfterTableReplacement,TestMode::StopAfterTableReplacement] {
        let pool=rich_fixture().await; let before=catalog(&pool).await; let (request,_) = reviewed(&pool,&pool,false).await;
        let (entry,result)=call(&pool,&pool,request,mode).await; assert!(result.result.is_err());
        if matches!(mode,TestMode::StopAfterTableReplacement) { assert!(result.confirmed); }
        assert_eq!(result.receipt.as_ref().unwrap().receipt.transaction,"rolled_back"); assert_eq!(catalog(&pool).await,before);
        let row: (i64,String,Vec<u8>) = crate::sql_history::query_as("SELECT rowid,a,extra FROM dst").fetch_one(&pool).await.unwrap(); assert_eq!(row,(42,"7".into(),vec![0,1,255]));
        let outcome=entry.finish(result.result,result.confirmed,false); assert_eq!(outcome.state,if matches!(mode,TestMode::StopAfterTableReplacement) {ExecutionState::Cancelled} else {ExecutionState::Failed}); assert!(!outcome.resources_pending);
    }
}

#[tokio::test]
async fn changed_review_mapping_allow_drop_and_catalog_cannot_reuse_old_rebuild_digest() {
    let pool=rich_fixture().await; let (request,report) = reviewed(&pool,&pool,false).await;
    let mut changed=request.clone(); changed.allow_drop=true;
    assert!(structure_sync::validate_review(&changed,&report,&binding()).is_err());
    let mut changed=request.clone(); changed.columns=vec![("a".into(),"extra".into())]; assert!(structure_sync::validate_review(&changed,&report,&binding()).is_err());
    exec(&pool,"CREATE VIEW concurrent_view AS SELECT a FROM dst").await;
    let before=catalog(&pool).await; let (_,result)=call(&pool,&pool,request,TestMode::Normal).await; assert!(result.result.is_err()); assert!(result.receipt.is_none()); assert_eq!(catalog(&pool).await,before);
}

#[tokio::test]
async fn check_disabled_connection_cannot_bypass_guard_or_modify_table() {
    let pool=pool().await; exec(&pool,"CREATE TABLE src(a INTEGER)").await; exec(&pool,"CREATE TABLE dst(a TEXT)").await; exec(&pool,"INSERT INTO dst VALUES('bad')").await;
    let (request,_) = reviewed(&pool,&pool,false).await; let before=catalog(&pool).await; exec(&pool,"PRAGMA ignore_check_constraints=ON").await;
    let (_,result)=call(&pool,&pool,request,TestMode::Normal).await; assert!(result.result.is_err()); assert!(result.receipt.is_none()); assert_eq!(catalog(&pool).await,before);
}

#[tokio::test]
async fn real_file_rebuild_commit_lost_ack_is_unknown_and_not_replayed() {
    let dir=std::env::temp_dir().join(format!("navkit_rebuild_unknown_{}",uuid::Uuid::new_v4())); assert!(dir.is_absolute()); std::fs::create_dir_all(&dir).unwrap();
    let source=SqlitePoolOptions::new().max_connections(1).connect_with(SqliteConnectOptions::new().filename(dir.join("source.sqlite")).create_if_missing(true).foreign_keys(true)).await.unwrap();
    let target=SqlitePoolOptions::new().max_connections(1).connect_with(SqliteConnectOptions::new().filename(dir.join("target.sqlite")).create_if_missing(true).foreign_keys(true)).await.unwrap();
    exec(&source,"CREATE TABLE src(a INTEGER)").await; exec(&target,"CREATE TABLE dst(a TEXT)").await; exec(&target,"INSERT INTO dst(rowid,a) VALUES(81,'6')").await;
    let (request,_) = reviewed(&source,&target,false).await; let (entry,result)=call(&source,&target,request,TestMode::LoseCommitAcknowledgement).await;
    assert_eq!(result.result.as_ref().unwrap_err().code,"EXECUTION_EFFECTS_UNKNOWN"); assert!(result.receipt.is_none()); assert_eq!(target.size(),0); assert_eq!(source.size(),0);
    assert_eq!(entry.finish(result.result,result.confirmed,true).state,ExecutionState::Unknown);
    let observer=SqlitePoolOptions::new().max_connections(1).connect_with(SqliteConnectOptions::new().filename(dir.join("target.sqlite")).foreign_keys(true)).await.unwrap();
    let row: (i64,i64,String) = crate::sql_history::query_as("SELECT rowid,a,typeof(a) FROM dst").fetch_one(&observer).await.unwrap(); assert_eq!(row,(81,6,"integer".into()));
    let leftover: i64 = crate::sql_history::query_scalar("SELECT count(*) FROM sqlite_schema WHERE name LIKE '__navkit_%'").fetch_one(&observer).await.unwrap(); assert_eq!(leftover,0);
    observer.close().await; source.close().await; target.close().await; std::fs::remove_dir_all(dir).unwrap();
}
