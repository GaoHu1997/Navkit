//! Contract tests plus opt-in real-server tests. No user's existing tables are modified.
use super::*;
fn request(namespace: &str) -> StructureSyncRequest { StructureSyncRequest { source_connection: "source".into(), source_schema: namespace.into(), source_table: "src".into(), target_connection: "target".into(), target_schema: namespace.into(), target_table: "dst".into(), columns: Vec::new(), allow_drop: false, source_revision: String::new(), target_revision: String::new(), review_digest: String::new() } }
fn binding(namespace: &str, owner: &str) -> StructureReviewBinding { StructureReviewBinding { principal: "real-test-window".into(), owner: owner.into(), source_connection_id: "source".into(), source_fingerprint: "a".repeat(64), source_schema: namespace.into(), source_table: "src".into(), target_connection_id: "target".into(), target_fingerprint: "b".repeat(64), target_schema: namespace.into(), target_table: "dst".into() } }
fn entry(request: &StructureSyncRequest, binding: &StructureReviewBinding) -> Arc<Entry> {
    let registry = execution::Registry::default(); let b = execution::Binding { principal: binding.principal.clone(), owner: binding.owner.clone(), connection: binding.target_connection_id.clone(), namespace: binding.target_schema.clone(), fingerprint: binding.target_fingerprint.clone() };
    let mut r = crate::execution_contracts::ExecutionRequest { execution_id: String::new(), connection_id: "target".into(), namespace: binding.target_schema.clone(), owner_id: binding.owner.clone(), sql: structure_sync::review_request_sql(request).unwrap(), transaction_owner: None, timeout_ms: Some(30_000), origin: Some("structure_sync".into()), execution_grant: None, cancel_before_send: false };
    registry.grant_request(&mut r, b.clone()).unwrap(); registry.register(&r, b, 0).unwrap()
}
#[test] fn safe_metadata_is_not_a_sql_injection_channel() {
    for t in ["integer", "character varying(120)", "numeric(18,2)", "timestamp without time zone"] { assert!(plan::safe_type(t, true).is_ok(), "{t}"); }
    for t in ["int unsigned", "varchar(128)", "decimal(20,4)", "datetime(6)"] { assert!(plan::safe_type(t, false).is_ok(), "{t}"); }
    for t in ["INT); DROP TABLE x;--", "custom_type", "integer[]", "varchar(20) COLLATE evil", "int DEFAULT evil()"] { assert!(plan::safe_type(t, true).is_err()); assert!(plan::safe_type(t, false).is_err()); }
    for d in ["'Keep  Case'::text", "42", "now()", "'It''s literal'::character varying"] { assert!(plan::safe_default(d, true).is_ok(), "{d}"); }
    for d in ["dangerous()", "'x' || evil()", "'x'::evil", "NULL; DROP TABLE t", "'unclosed"] { assert!(plan::safe_default(d, true).is_err()); }
}
#[test] fn postgres_only_proven_lossless_widen_is_executable() {
    for (a, b) in [("smallint", "integer"), ("integer", "bigint"), ("character varying(40)", "character varying(120)"), ("character varying(40)", "text"), ("numeric(10,2)", "numeric(20,2)")] { assert!(plan::pg_lossless_type(a,b), "{a} → {b}"); }
    for (a, b) in [("bigint", "integer"), ("character varying(120)", "character varying(40)"), ("numeric(20,4)", "numeric(20,2)"), ("numeric", "numeric(20,2)"), ("double precision", "real"), ("integer", "text"), ("timestamp without time zone", "timestamp(2) without time zone")] { assert!(!plan::pg_lossless_type(a,b), "{a} → {b}"); }
}
#[test] fn artifacts_are_bound_to_real_owner_material_and_both_cas() {
    let r = request("isolated"); let b = binding("isolated", "page");
    let base = plan::artifacts(&r, &b, &"c".repeat(64), &"d".repeat(64));
    assert!(base.0.starts_with("__navkit_sync_")); assert!(base.1.starts_with("__navkit_backup_")); assert_ne!(base.0, base.1); assert!(base.1.len() < 64);
    let mut changed = b.clone(); changed.owner = "other-page".into(); assert_ne!(base, plan::artifacts(&r, &changed, &"c".repeat(64), &"d".repeat(64)));
    changed = b; changed.source_fingerprint = "e".repeat(64); assert_ne!(base, plan::artifacts(&r, &changed, &"c".repeat(64), &"d".repeat(64)));
    assert_ne!(base, plan::artifacts(&r, &changed, &"f".repeat(64), &"d".repeat(64)));
}
#[test] fn full_native_plans_include_add_alter_drop_columns_and_plain_indexes() {
    use crate::models::{ColumnInfo,IndexInfo,TableSchema}; use crate::structure_execution::metadata::{PhysicalDatabaseIdentity,PhysicalTableIdentity};
    let col = |name: &str, ty: &str| ColumnInfo { name: name.into(), data_type: ty.into(), nullable: true, default: None, primary_key: false, auto_increment: false, comment: None, length: None, precision: None, scale: None };
    for pg in [false,true] {
        let kind = DbKind::of(if pg { "postgresql" } else { "mysql" }); let r = { let mut r=request("isolated"); r.allow_drop=true; r }; let b=binding("isolated","page");
        let schema = |name: &str, source: bool| TableSchema { table:name.into(), database:"isolated".into(), columns: if source { vec![col("id",if pg {"bigint"} else {"bigint"}),col("note",if pg {"character varying(120)"} else {"varchar(120)"}),col("added","text")] } else { vec![col("id",if pg {"integer"} else {"int"}),col("note",if pg {"character varying(40)"} else {"varchar(40)"}),col("legacy","text")] }, indexes:vec![IndexInfo {name:if source {"idx_source_note"} else {"idx_legacy"}.into(),columns:vec![if source {"note"} else {"legacy"}.into()],unique:false,primary:false,method:Some("btree".into())}],foreign_keys:vec![],comment:None,engine:None,collation:None,row_count:None };
        let snap = |schema: TableSchema, id: i64, revision: &str| metadata::Snapshot { identity:PhysicalTableIdentity {database:PhysicalDatabaseIdentity {kind:if pg {"postgres-server"} else {"mysql-server"}.into(),identity:"real-test-server:database:isolated".into(),canonical_path:None},schema:"isolated".into(),table:schema.table.clone(),rootpage:id},schema,revision:revision.repeat(64),physical_table:format!("table:{id}"),unsupported:vec![],evidence:vec![] };
        let source=snap(schema("src",true),1,"c"); let target=snap(schema("dst",false),2,"d"); let report=report(&source,&target,&kind,&r,&b).unwrap();
        assert!(report.alterable,"{:?}",report.unsupported); for change in ["add_column","alter_column","drop_column","index","drop_index"] { assert!(report.changes.iter().any(|c|c.kind==change),"missing {change}"); }
        assert!(report.steps.iter().any(|s|s.sql.contains("ADD COLUMN"))); assert!(report.steps.iter().any(|s|s.sql.contains("DROP COLUMN"))); assert!(report.steps.iter().any(|s|s.sql.contains("DROP INDEX")));
        if pg { assert!(report.steps.iter().any(|s|s.sql.contains("ALTER COLUMN")&&s.sql.contains(" TYPE "))); assert!(report.steps.iter().any(|s|s.sql.contains("__navkit_idx_"))); }
        else { assert!(report.steps[0].sql.starts_with("CREATE TABLE")&&report.steps[0].sql.contains(" LIKE ")); assert!(report.steps.last().unwrap().sql.starts_with("RENAME TABLE")); assert!(report.steps.iter().filter(|s|s.sql.starts_with("ALTER TABLE")).all(|s|s.sql.contains("__navkit_sync_"))); }
        let mut bound=report; structure_sync::bind_review(&r,&mut bound,&b).unwrap(); assert_eq!(bound.review_digest.len(),64);
        let mut reviewed=r; reviewed.source_revision=bound.source_revision.clone(); reviewed.target_revision=bound.target_revision.clone(); reviewed.review_digest=bound.review_digest.clone(); structure_sync::validate_review(&reviewed,&bound,&b).unwrap();
        let mut tampered=bound; tampered.steps[0].sql.push_str("; DROP TABLE user_data"); assert!(structure_sync::validate_review(&reviewed,&tampered,&b).is_err());
    }
}
#[tokio::test] async fn read_only_and_adapter_mismatch_do_not_borrow_a_held_pool() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap(); let held = pool.acquire().await.unwrap(); let db = Arc::new(Db::Sqlite(pool.clone()));
    let mut r = request("main"); r.source_revision = "c".repeat(64); r.target_revision = "d".repeat(64); r.review_digest = "e".repeat(64); let b = binding("main", "page"); let gates = crate::manual_transaction::Registry::default();
    let result = tokio::time::timeout(std::time::Duration::from_millis(300), run(db.clone(), DbKind::of("mysql"), db.clone(), DbKind::of("mysql"), entry(&r, &b), gates.ordinary(&["source", "target"]).await.unwrap(), r.clone(), b.clone(), true)).await.unwrap();
    assert_eq!(result.result.unwrap_err().code, "STRUCTURE_READ_ONLY"); assert!(result.receipt.is_none());
    let result = run(db.clone(), DbKind::of("mysql"), db, DbKind::of("mysql"), entry(&r, &b), gates.ordinary(&["source", "target"]).await.unwrap(), r, b, false).await;
    assert_eq!(result.result.unwrap_err().code, "STRUCTURE_NATIVE_DIALECT_UNSUPPORTED"); drop(held); pool.close().await;
}
#[tokio::test] #[ignore = "requires explicit NAVKIT_STRUCTURE_POSTGRES_URL; isolated real PostgreSQL only"] async fn real_postgres_isolated_schema() { real_server(true).await; }
#[tokio::test] #[ignore = "requires explicit NAVKIT_STRUCTURE_MYSQL_URL; isolated real MySQL 8 only"] async fn real_mysql_isolated_database() { real_server(false).await; }
async fn review_fixture(database: &Arc<Db>, kind: &DbKind, gates: &crate::manual_transaction::Registry, namespace: &str, owner: &str, allow_drop: bool) -> Result<(StructureSyncRequest, StructureReviewBinding, StructurePreflight)> {
    let mut r = request(namespace); r.allow_drop = allow_drop; let b = binding(namespace, owner);
    let preview = preflight(database.clone(), kind.clone(), database.clone(), kind.clone(), gates.ordinary(&["source", "target"]).await?, r.clone(), b.clone(), false).await?;
    ensure!(preview.report.alterable && !preview.request_sql.is_empty(), "真实预览不可执行：{:?}", preview.report.unsupported);
    r.source_revision = preview.report.source_revision.clone(); r.target_revision = preview.report.target_revision.clone(); r.review_digest = preview.report.review_digest.clone(); Ok((r,b,preview))
}
async fn inspect(database: &Arc<Db>, namespace: &str, table: &str) -> Result<metadata::Snapshot> {
    let mut s = Session::acquire(database, None).await?; s.setup().await?; let physical = metadata::physical(&mut s).await?;
    let result = metadata::snapshot(&mut s, &physical, namespace, table).await; s.release(false).await?; result
}
async fn real_server(pg: bool) {
    let variable = if pg { "NAVKIT_STRUCTURE_POSTGRES_URL" } else { "NAVKIT_STRUCTURE_MYSQL_URL" };
    let url = std::env::var(variable).expect("真实服务器测试必须显式配置授权隔离 URL（值不输出）");
    let database = if pg { Db::Postgres(sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(&url).await.expect("真实测试连接失败（不显示URL）")) }
        else { Db::Mysql(sqlx::mysql::MySqlPoolOptions::new().max_connections(1).connect(&url).await.expect("真实测试连接失败（不显示URL）")) };
    let database = Arc::new(database); let kind = DbKind::of(if pg { "postgresql" } else { "mysql" });
    let namespace = format!("navkit_structure_test_{}", uuid::Uuid::new_v4().simple()); assert!(namespace.starts_with("navkit_structure_test_"));
    let quoted = db::quote_ident(&kind, &namespace); let create = format!("CREATE {} {quoted}", if pg { "SCHEMA" } else { "DATABASE" });
    let mut s = Session::acquire(&database, None).await.unwrap(); s.setup().await.unwrap(); s.execute(&create).await.unwrap();
    if !pg {
        let before_mode = s.scalar("SELECT @@session.sql_mode").await.unwrap();
        s.execute("START TRANSACTION").await.unwrap();
        assert!(s.setup().await.is_err(), "autocommit=1 的既有显式事务必须拒绝接管");
        assert_eq!(s.scalar("SELECT @@session.sql_mode").await.unwrap(), before_mode, "事务拒绝前不可修改session policy");
        // This test created and owns this transaction; product code did not roll it back.
        s.execute("ROLLBACK").await.unwrap();
    }
    let result = async {
        let src = db::qualified(&kind, &namespace, "src"); let dst = db::qualified(&kind, &namespace, "dst");
        s.execute(&format!("CREATE TABLE {src}(id BIGINT NOT NULL PRIMARY KEY,note VARCHAR(120),added TEXT)")).await?;
        s.execute(&format!("CREATE TABLE {dst}(id INT NOT NULL PRIMARY KEY,note VARCHAR(40),legacy TEXT)")).await?;
        s.execute(&format!("CREATE INDEX {} ON {src}(note)", db::quote_ident(&kind, "idx_source_note"))).await?;
        s.execute(&format!("CREATE INDEX {} ON {dst}(id,note)", db::quote_ident(&kind, "idx_legacy"))).await?;
        s.execute(&format!("INSERT INTO {dst}(id,note,legacy) VALUES(1,'Keep  Case','original')")).await?;
        s.release(false).await?;
        let gates = crate::manual_transaction::Registry::default(); let mut r = request(&namespace); let b = binding(&namespace, "page");
        let preview = preflight(database.clone(), kind.clone(), database.clone(), kind.clone(), gates.ordinary(&["source", "target"]).await?, r.clone(), b.clone(), false).await?;
        ensure!(preview.report.alterable && !preview.request_sql.is_empty(), "真实服务器预览不可执行：{:?}", preview.report.unsupported);
        ensure!(preview.source_identity.database.kind == if pg { "postgres-server" } else { "mysql-server" }, "真实方言身份错误");
        r.source_revision = preview.report.source_revision; r.target_revision = preview.report.target_revision; r.review_digest = preview.report.review_digest;
        let e = entry(&r, &b); let executed = run(database.clone(), kind.clone(), database.clone(), kind.clone(), e.clone(), gates.ordinary(&["source", "target"]).await?, r.clone(), b.clone(), false).await;
        ensure!(executed.result.is_ok(), "真实同步失败：{:?}", executed.result.as_ref().err()); let receipt = executed.receipt.as_ref().ok_or_else(|| anyhow!("缺少receipt"))?;
        ensure!(receipt.receipt.verified && receipt.receipt.transaction == if pg { "committed" } else { "shadow_swapped_backup_retained" }, "事务/影子证据错误");
        e.finish(executed.result, executed.confirmed, false);
        let mut observer = Session::acquire(&database, None).await?; let row = observer.rows(&format!("SELECT CAST(id AS {}) AS id,note,legacy,added FROM {dst}", if pg { "TEXT" } else { "CHAR" })).await?;
        ensure!(row.len() == 1 && get(&row[0], "id")? == "1" && get(&row[0], "note")? == "Keep  Case" && get(&row[0], "legacy")? == "original" && opt(&row[0], "added")?.is_none(), "数据保留失败");
        observer.execute(&format!("ALTER TABLE {src} ADD COLUMN pending TEXT")).await?; observer.release(false).await?;
        for owner in ["test-fail-after-ddl", "test-stop-after-ddl"] {
            let (reviewed, bind, _) = review_fixture(&database, &kind, &gates, &namespace, owner, false).await?;
            let before = inspect(&database, &namespace, "dst").await?;
            let e = entry(&reviewed, &bind); let applied = run(database.clone(), kind.clone(), database.clone(), kind.clone(), e.clone(), gates.ordinary(&["source", "target"]).await?, reviewed, bind, false).await;
            ensure!(applied.result.is_err(), "故障/取消不可成功");
            let receipt = applied.receipt.as_ref().ok_or_else(|| anyhow!("故障/取消没有明确receipt"))?;
            ensure!(receipt.receipt.transaction == if pg { "rolled_back" } else { "shadow_failed_original_retained" }, "错误恢复语义");
            ensure!(inspect(&database, &namespace, "dst").await?.revision == before.revision, "失败/取消改变原目标表");
            let outcome = e.finish(applied.result, applied.confirmed, false);
            ensure!(outcome.state == if owner.contains("stop") { crate::execution_contracts::ExecutionState::Cancelled } else { crate::execution_contracts::ExecutionState::Failed }, "C1故障/停止状态未确认");
        }
        if !pg {
            let (reviewed, bind, _) = review_fixture(&database, &kind, &gates, &namespace, "test-force-swap-readback-mismatch", false).await?;
            let before = inspect(&database, &namespace, "dst").await?; let e = entry(&reviewed, &bind);
            let applied = run(database.clone(), kind.clone(), database.clone(), kind.clone(), e.clone(), gates.ordinary(&["source", "target"]).await?, reviewed, bind, false).await;
            ensure!(applied.result.is_err() && applied.receipt.as_ref().is_some_and(|r| r.receipt.transaction == "shadow_restored_backup_retained"), "原子恢复未确认");
            ensure!(inspect(&database, &namespace, "dst").await?.identity == before.identity && inspect(&database, &namespace, "dst").await?.revision == before.revision, "恢复没有保持真实原表ID和目录");
            e.finish(applied.result, applied.confirmed, false);
        }
        let (reviewed, bind, preview) = review_fixture(&database, &kind, &gates, &namespace, "destructive-page", true).await?;
        ensure!(preview.report.changes.iter().any(|c| c.kind == "drop_column" && c.object == "legacy") && preview.report.changes.iter().any(|c| c.kind == "drop_index" && c.object == "idx_legacy"), "删除列/索引未逐项进入preview");
        let e = entry(&reviewed, &bind); let applied = run(database.clone(), kind.clone(), database.clone(), kind.clone(), e.clone(), gates.ordinary(&["source", "target"]).await?, reviewed, bind, false).await;
        ensure!(applied.result.is_ok(), "显式删除同步失败：{:?}", applied.result.as_ref().err()); let final_schema = applied.receipt.as_ref().unwrap().receipt.schema.as_ref().unwrap();
        ensure!(!final_schema.columns.iter().any(|c| c.name == "legacy") && !final_schema.indexes.iter().any(|i| i.name == "idx_legacy") && final_schema.columns.iter().any(|c| c.name == "pending"), "删除列/普通索引/新增列读回失败"); e.finish(applied.result, applied.confirmed, false);
        let mut observer = Session::acquire(&database, None).await?; observer.execute(&format!("ALTER TABLE {src} ADD COLUMN ack_marker TEXT")).await?; observer.release(false).await?;
        let owner = if pg { "test-lose-commit-ack" } else { "test-lose-rename-ack" };
        let (reviewed, bind, _) = review_fixture(&database, &kind, &gates, &namespace, owner, false).await?; let e = entry(&reviewed, &bind);
        let applied = run(database.clone(), kind.clone(), database.clone(), kind.clone(), e.clone(), gates.ordinary(&["source", "target"]).await?, reviewed, bind, false).await;
        ensure!(applied.result.as_ref().err().is_some_and(|e| e.code == "EXECUTION_EFFECTS_UNKNOWN") && applied.receipt.is_none(), "丢失真实提交ACK必须Unknown且无receipt");
        ensure!(e.finish(applied.result, applied.confirmed, false).state == crate::execution_contracts::ExecutionState::Unknown, "C1丢ACK终态被改成成功");
        ensure!(inspect(&database, &namespace, "dst").await?.schema.columns.iter().any(|c| c.name == "ack_marker"), "独立观察应证明真实提交发生，但不得升级原Unknown");
        let mut observer = Session::acquire(&database, None).await?; observer.execute(&format!("ALTER TABLE {src} ADD COLUMN concurrent TEXT")).await?; observer.release(false).await?;
        let e = entry(&r, &b); let stale = run(database.clone(), kind.clone(), database.clone(), kind.clone(), e.clone(), gates.ordinary(&["source", "target"]).await?, r, b, false).await;
        ensure!(stale.result.is_err() && stale.receipt.is_none(), "stale 旧审核未拒绝"); e.finish(stale.result, stale.confirmed, false);
        let mut alias = request(&namespace); alias.source_table = "dst".into(); let mut alias_binding = binding(&namespace, "alias-page"); alias_binding.source_table = "dst".into();
        let alias_error = preflight(database.clone(), kind.clone(), database.clone(), kind.clone(), gates.ordinary(&["source", "target"]).await?, alias, alias_binding, false).await;
        ensure!(alias_error.is_err(), "同真实表别名没有拒绝");
        let view = db::qualified(&kind, &namespace, "dependent_view"); let mut observer = Session::acquire(&database, None).await?;
        observer.execute(&format!("CREATE VIEW {view} AS SELECT id FROM {dst}")).await?; observer.release(false).await?;
        let blocked = preflight(database.clone(), kind.clone(), database.clone(), kind.clone(), gates.ordinary(&["source", "target"]).await?, request(&namespace), binding(&namespace, "dependency-page"), false).await?;
        ensure!(!blocked.report.alterable && blocked.request_sql.is_empty() && blocked.report.steps.is_empty(), "依赖视图不可假称可同步");
        let mut observer = Session::acquire(&database, None).await?; observer.execute(&format!("DROP VIEW {view}")).await?;
        let child = db::qualified(&kind, &namespace, "incoming_fk"); observer.execute(&format!("CREATE TABLE {child}(ref BIGINT,FOREIGN KEY(ref) REFERENCES {dst}(id))")).await?; observer.release(false).await?;
        let blocked = preflight(database.clone(), kind.clone(), database.clone(), kind.clone(), gates.ordinary(&["source", "target"]).await?, request(&namespace), binding(&namespace, "fk-page"), false).await?;
        ensure!(!blocked.report.alterable && blocked.request_sql.is_empty(), "入站外键不可丢失/CASCADE");
        Ok::<_, anyhow::Error>(())
    }.await;
    // Only this freshly generated namespace is removed; no user's pre-existing object is touched.
    let mut cleanup = Session::acquire(&database, None).await.unwrap();
    let drop_sql = format!("DROP {} {quoted}{}", if pg { "SCHEMA" } else { "DATABASE" }, if pg { " CASCADE" } else { "" });
    let dropped = cleanup.execute(&drop_sql).await; cleanup.release(false).await.unwrap();
    match database.as_ref() { Db::Postgres(p) => p.close().await, Db::Mysql(p) => p.close().await, _ => unreachable!() }
    assert!(dropped.is_ok(), "隔离测试schema清理失败"); assert!(result.is_ok(), "真实服务端测试失败：{result:?}");
}
