//! Native MySQL 8 / PostgreSQL structure synchronization, owned C1 caller only.
//! No grant/register/finish; no pool metadata queries while holding an owned pair.
#[path = "structure_server_metadata.rs"] mod metadata;
#[path = "structure_server_plan.rs"] mod plan;
#[cfg(test)] #[path = "structure_server_tests.rs"] mod tests;

use std::{collections::BTreeMap, sync::Arc, time::Instant};
use anyhow::{anyhow, ensure, Result};
use sqlx::{Connection, Executor, Row, Column, ValueRef};
use crate::{db::{self, Db}, design::{self, DesignReceipt, DesignStepResult}, error::{AppError, CmdResult},
    execution::{self, Entry}, manual_transaction::OrdinaryLease, models::{DbKind, MultiResult, Protocol},
    structure_execution::{BoundResult, metadata::StructurePreflight},
    structure_sync::{self, StructureDiffReport, StructureReviewBinding, StructureSyncReceipt, StructureSyncRequest}};
type Record = BTreeMap<String, Option<String>>;
fn get<'a>(row: &'a Record, key: &str) -> Result<&'a str> { row.get(key).and_then(|v| v.as_deref()).ok_or_else(|| anyhow!("STRUCTURE_METADATA_FIELD_UNKNOWN: {key}")) }
fn opt(row: &Record, key: &str) -> Result<Option<String>> { row.get(key).cloned().ok_or_else(|| anyhow!("STRUCTURE_METADATA_FIELD_MISSING: {key}")) }
fn unknown(message: impl Into<String>) -> AppError { AppError::coded("EXECUTION_EFFECTS_UNKNOWN", message) }
fn transport(error: &anyhow::Error) -> bool { error.downcast_ref::<sqlx::Error>().is_some_and(|e| !matches!(e, sqlx::Error::Database(_))) || error.downcast_ref::<AppError>().is_some_and(|e| e.code == "EXECUTION_EFFECTS_UNKNOWN") }
fn stopped() -> anyhow::Error { AppError::coded("STRUCTURE_STOPPED_BEFORE_COMMIT", "已停止：不发送其余业务 SQL，等待确认回滚/原表保留").into() }
fn check_stop(entry: &Entry) -> Result<()> { if entry.stopping() { Err(stopped()) } else { Ok(()) } }

/// This checks *both* configured dialect and actual adapter; compatible/custom adapters are not relabelled.
pub(crate) fn native_pair(source: &Db, source_kind: &DbKind, target: &Db, target_kind: &DbKind) -> bool {
    matches!((source, source_kind.id(), target, target_kind.id()),
        (Db::Mysql(_), "mysql", Db::Mysql(_), "mysql") | (Db::Postgres(_), "postgresql", Db::Postgres(_), "postgresql"))
}
fn validate(request: &StructureSyncRequest, binding: &StructureReviewBinding) -> Result<()> {
    for name in [&request.source_schema, &request.source_table, &request.target_schema, &request.target_table] { plan::ident(name)?; }
    ensure!(request.columns.len() <= 1024, "STRUCTURE_MAPPING_QUOTA");
    for (from, to) in &request.columns { plan::ident(from)?; plan::ident(to)?; }
    let fp = |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    ensure!(!binding.principal.is_empty() && !binding.owner.is_empty() && fp(&binding.source_fingerprint) && fp(&binding.target_fingerprint), "STRUCTURE_BINDING_REQUIRED");
    ensure!(request.source_connection == binding.source_connection_id && request.target_connection == binding.target_connection_id
        && request.source_schema == binding.source_schema && request.target_schema == binding.target_schema
        && request.source_table == binding.source_table && request.target_table == binding.target_table, "STRUCTURE_BINDING_CHANGED");
    Ok(())
}

enum Session { Mysql(sqlx::pool::PoolConnection<sqlx::MySql>, Option<String>), Pg(sqlx::pool::PoolConnection<sqlx::Postgres>) }
impl Session {
    fn pg(&self) -> bool { matches!(self, Self::Pg(_)) }
    fn kind(&self) -> DbKind { DbKind::of(if self.pg() { "postgresql" } else { "mysql" }) }
    async fn acquire(db: &Db, entry: Option<&Arc<Entry>>) -> Result<Self> {
        match db {
            Db::Mysql(pool) => { let c = if let Some(e) = entry { tokio::select! { biased; _ = e.wait_stop() => return Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "池等待停止，未发送 DDL").into()), c = pool.acquire() => c? } } else { pool.acquire().await? }; Ok(Self::Mysql(c, None)) },
            Db::Postgres(pool) => { let c = if let Some(e) = entry { tokio::select! { biased; _ = e.wait_stop() => return Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "池等待停止，未发送 DDL").into()), c = pool.acquire() => c? } } else { pool.acquire().await? }; Ok(Self::Pg(c)) },
            _ => Err(anyhow!("STRUCTURE_NATIVE_ADAPTER_UNSUPPORTED")),
        }
    }
    async fn rows(&mut self, sql: &str) -> Result<Vec<Record>> {
        macro_rules! read { ($c:expr) => {{
            let rows = crate::sql_history::query(sql).fetch_all(&mut **$c).await?;
            ensure!(rows.len() <= 16384, "STRUCTURE_METADATA_QUOTA"); let mut out = Vec::new();
            for row in rows { let mut r = Record::new(); for (i, col) in row.columns().iter().enumerate() {
                let value = if row.try_get_raw(i)?.is_null() { None } else { Some(row.try_get::<String, _>(i)?) };
                ensure!(value.as_ref().map_or(true, |v| v.len() <= 8 * 1024 * 1024), "STRUCTURE_METADATA_BYTES_QUOTA");
                ensure!(r.insert(col.name().into(), value).is_none(), "STRUCTURE_METADATA_DUPLICATE_FIELD");
            } out.push(r); } Ok(out)
        }}; }
        match self { Self::Mysql(c, _) => read!(c), Self::Pg(c) => read!(c) }
    }
    async fn scalar(&mut self, sql: &str) -> Result<String> {
        let rows = self.rows(sql).await?; ensure!(rows.len() == 1 && rows[0].len() == 1, "STRUCTURE_METADATA_SCALAR_UNKNOWN");
        rows[0].values().next().unwrap().clone().ok_or_else(|| anyhow!("STRUCTURE_METADATA_NULL_SCALAR"))
    }
    async fn execute(&mut self, sql: &str) -> Result<u64> {
        // LOCK TABLES/UNLOCK TABLES are not supported by MySQL's prepared protocol.
        match self { Self::Mysql(c, _) => Ok((&mut **c).execute(sql).await?.rows_affected()), Self::Pg(c) => Ok(crate::sql_history::query(sql).execute(&mut **c).await?.rows_affected()) }
    }
    async fn setup(&mut self) -> Result<()> {
        if let Self::Mysql(c, slot) = self {
            // autocommit=1 does NOT rule out an explicit START TRANSACTION. MySQL 8 has no
            // MariaDB @@in_transaction variable, so prove actual transaction events on THIS
            // server thread. If instrumentation is absent, fail before SET/BEGIN/ROLLBACK.
            let enabled: String = crate::sql_history::query_scalar("SELECT CAST((SELECT count(*) FROM performance_schema.setup_instruments WHERE NAME='transaction' AND ENABLED='YES') + (SELECT count(*) FROM performance_schema.setup_consumers WHERE NAME='events_transactions_current' AND ENABLED='YES') AS CHAR)").fetch_one(&mut **c).await?;
            ensure!(enabled == "2", "STRUCTURE_MYSQL_TRANSACTION_STATE_UNPROVEN: transaction instrumentation/current consumer 未开启");
            let active: String = crate::sql_history::query_scalar("SELECT CAST(count(*) AS CHAR) FROM performance_schema.events_transactions_current e JOIN performance_schema.threads t ON t.THREAD_ID=e.THREAD_ID WHERE t.PROCESSLIST_ID=CONNECTION_ID() AND e.STATE='ACTIVE' AND e.AUTOCOMMIT='NO'").fetch_one(&mut **c).await?;
            ensure!(active == "0" && crate::sql_history::query_scalar::<_, i64>("SELECT @@session.autocommit").fetch_one(&mut **c).await? == 1, "STRUCTURE_SESSION_TRANSACTION_ACTIVE: 不接管/回滚已存在用户事务");
            let mode: String = crate::sql_history::query_scalar("SELECT @@session.sql_mode").fetch_one(&mut **c).await?;
            *slot = Some(mode.clone()); let mut wanted = mode.split(',').filter(|s| !s.is_empty()).map(str::to_owned).collect::<Vec<_>>();
            for flag in ["NO_BACKSLASH_ESCAPES", "STRICT_ALL_TABLES"] { if !wanted.iter().any(|s| s == flag) { wanted.push(flag.into()); } }
            (&mut **c).execute(format!("SET SESSION sql_mode={}", db::quote_literal(&wanted.join(","))).as_str()).await?;
            ensure!(crate::sql_history::query_scalar::<_, i64>("SELECT @@session.autocommit").fetch_one(&mut **c).await? == 1, "STRUCTURE_SESSION_TRANSACTION_ACTIVE");
        }
        Ok(())
    }
    async fn release(self, destroy: bool) -> Result<()> {
        match self {
            Self::Mysql(mut c, mode) => { if destroy { c.close_on_drop(); c.close().await?; } else {
                if let Some(mode) = mode { if let Err(e) = (&mut *c).execute(format!("SET SESSION sql_mode={}", db::quote_literal(&mode)).as_str()).await { c.close_on_drop(); let _ = c.close().await; return Err(e.into()); } }
                c.return_to_pool().await;
            } },
            Self::Pg(mut c) => { if destroy { c.close_on_drop(); c.close().await?; } else { c.return_to_pool().await; } },
        } Ok(())
    }
}
struct Pair { target: Session, source: Option<Session>, source_physical: metadata::Physical, target_physical: metadata::Physical, target_tx: bool, source_tx: bool, locked: bool }
impl Pair {
    async fn acquire(source: &Db, target: &Db, request: &StructureSyncRequest, entry: Option<&Arc<Entry>>) -> Result<Self> {
        let same_pool = match (source, target) { (Db::Mysql(a), Db::Mysql(b)) => Arc::ptr_eq(&a.connect_options(), &b.connect_options()), (Db::Postgres(a), Db::Postgres(b)) => Arc::ptr_eq(&a.connect_options(), &b.connect_options()), _ => false };
        let mut t = Session::acquire(target, entry).await?;
        let outcome = async { t.setup().await?; let tp = metadata::physical(&mut t).await?;
            if same_pool { return Ok::<_, anyhow::Error>((None, tp.clone(), tp)); }
            let mut s = Session::acquire(source, entry).await?;
            let sp = match async { s.setup().await?; let physical = metadata::physical(&mut s).await?;
                metadata::snapshot(&mut s, &physical, &request.source_schema, &request.source_table).await?;
                let source_full = db::qualified(&s.kind(), &request.source_schema, &request.source_table);
                s.rows(&format!("SELECT * FROM {source_full} LIMIT 0")).await?;
                Ok::<_, anyhow::Error>(physical)
            }.await { Ok(p) => p, Err(e) => { s.release(true).await?; return Err(e); } };
            // MySQL can read another database on this same server using fully qualified names.
            // PG cannot switch databases; only merge a proven same-cluster/same-database pair.
            if sp.server == tp.server && (!t.pg() || sp.database == tp.database) { s.release(false).await?; Ok((None, sp, tp)) } else { Ok((Some(s), sp, tp)) }
        }.await;
        match outcome { Ok((source, source_physical, target_physical)) => Ok(Self { target: t, source, source_physical, target_physical, target_tx: false, source_tx: false, locked: false }), Err(e) => { t.release(true).await?; Err(e) } }
    }
    async fn source_snapshot(&mut self, request: &StructureSyncRequest) -> Result<metadata::Snapshot> {
        if let Some(s) = &mut self.source { metadata::snapshot(s, &self.source_physical, &request.source_schema, &request.source_table).await }
        else { metadata::snapshot(&mut self.target, &self.source_physical, &request.source_schema, &request.source_table).await }
    }
    async fn snapshots(&mut self, request: &StructureSyncRequest) -> Result<(metadata::Snapshot, metadata::Snapshot)> {
        let s = self.source_snapshot(request).await?;
        let t = metadata::snapshot(&mut self.target, &self.target_physical, &request.target_schema, &request.target_table).await?;
        // Real OID/TABLE_ID catches table-name case aliases and connection aliases, not saved-config hashes.
        ensure!(s.physical_table != t.physical_table, "STRUCTURE_SAME_PHYSICAL_TABLE: 双端真实 database/table OID 或 UUID/TABLE_ID 相同");
        Ok((s, t))
    }
    async fn begin_read(&mut self, request: &StructureSyncRequest) -> Result<()> {
        let pg = self.target.pg(); let begin = if pg { "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY" } else { "START TRANSACTION READ ONLY" };
        self.target_tx = true; self.target.execute(begin).await?;
        let full = db::qualified(&self.target.kind(), &request.target_schema, &request.target_table);
        self.target.rows(&format!("SELECT * FROM {full} LIMIT 0")).await?;
        if let Some(s) = &mut self.source { self.source_tx = true; s.execute(begin).await?; }
        let source_full = db::qualified(&self.target.kind(), &request.source_schema, &request.source_table);
        if let Some(s) = &mut self.source { s.rows(&format!("SELECT * FROM {source_full} LIMIT 0")).await?; }
        else { self.target.rows(&format!("SELECT * FROM {source_full} LIMIT 0")).await?; } Ok(())
    }
    async fn cleanup(mut self, destroy: bool) -> Result<()> {
        let mut failure = None;
        if self.target_tx { if let Err(e) = self.target.execute("ROLLBACK").await { failure = Some(e); } }
        if self.source_tx { if let Some(s) = &mut self.source { if let Err(e) = s.execute("ROLLBACK").await { failure = Some(e); } } }
        if self.locked { if let Err(e) = self.target.execute("UNLOCK TABLES").await { failure = Some(e); } if let Some(s) = &mut self.source { if let Err(e) = s.execute("UNLOCK TABLES").await { failure = Some(e); } } }
        let discard = destroy || failure.is_some(); if let Err(e) = self.target.release(discard).await { failure = Some(e); }
        if let Some(s) = self.source { if let Err(e) = s.release(discard).await { failure = Some(e); } }
        if let Some(e) = failure { Err(e) } else { Ok(()) }
    }
}

fn report(source: &metadata::Snapshot, target: &metadata::Snapshot, kind: &DbKind, request: &StructureSyncRequest, binding: &StructureReviewBinding) -> Result<StructureDiffReport> {
    let mut report = structure_sync::preview_from_schemas(&source.schema, Some(&target.schema), kind, request, &source.revision, &target.revision)?;
    report.unsupported.extend(source.unsupported.iter().map(|s| format!("源：{s}"))); report.unsupported.extend(target.unsupported.iter().map(|s| format!("目标：{s}")));
    if let Err(e) = plan::complete(&mut report, source, target, kind, request, binding) { report.unsupported.push(format!("{e:#}")); }
    report.alterable = report.unsupported.is_empty();
    if !report.alterable { report.steps.clear(); report.review_digest.clear(); report.summary = "服务端依赖/类型无法无损保留：预览不可执行".into(); }
    Ok(report)
}
/// Same ABI as SQLite metadata::preflight, including real non-SQLite physical identities.
pub async fn preflight(source: Arc<Db>, source_kind: DbKind, target: Arc<Db>, target_kind: DbKind, ordinary: OrdinaryLease,
    request: StructureSyncRequest, binding: StructureReviewBinding, read_only: bool) -> CmdResult<StructurePreflight> {
    let _ordinary = ordinary;
    let operation = async { validate(&request, &binding)?;
        ensure!(native_pair(&source, &source_kind, &target, &target_kind), "STRUCTURE_NATIVE_DIALECT_UNSUPPORTED: 只认证同方言原生 MySQL 8 / PostgreSQL，不把兼容驱动或跨方言当 SQLite");
        let mut pair = Pair::acquire(&source, &target, &request, None).await?;
        let result = async { pair.begin_read(&request).await?; let (s, t) = pair.snapshots(&request).await?;
            let (sa, ta) = pair.snapshots(&request).await?; ensure!(s.revision == sa.revision && t.revision == ta.revision, "STRUCTURE_PREVIEW_STALE");
            let mut report = report(&s, &t, &target_kind, &request, &binding)?;
            if read_only { report.alterable = false; report.unsupported.push("目标只读：没有执行资格".into()); report.steps.clear(); }
            let mut request_sql = String::new();
            if report.alterable && !report.steps.is_empty() { structure_sync::bind_review(&request, &mut report, &binding)?;
                let mut reviewed = request.clone(); reviewed.source_revision = report.source_revision.clone(); reviewed.target_revision = report.target_revision.clone(); reviewed.review_digest = report.review_digest.clone(); request_sql = structure_sync::review_request_sql(&reviewed)?; }
            let same_database = s.identity.database.identity == t.identity.database.identity;
            let observation_digest = crate::object_catalog::revision(&("STRUCTURE_SERVER_READ_ONLY_OBSERVATION", &binding, &report, &s.identity, &t.identity));
            Ok::<_, anyhow::Error>(StructurePreflight { report, source_identity: s.identity, target_identity: Some(t.identity), same_database,
                source_consistency: if same_database { "same_owned_read_snapshot" } else { "two_owned_read_snapshots_not_cross_database_atomic" }.into(), observation_only: request_sql.is_empty(), request_sql, observation_digest, observation_binding: binding.clone() })
        }.await;
        pair.cleanup(false).await.map_err(|e| unknown(format!("STRUCTURE_PREVIEW_CLEANUP_UNKNOWN: {e:#}")))?; result
    }.await; operation.map_err(AppError::from)
}

#[derive(Default)]
struct Work { report: Option<StructureDiffReport>, before: Option<metadata::Snapshot>, source_before: Option<metadata::Snapshot>, after: Option<metadata::Snapshot>, steps: Vec<DesignStepResult>, shadow_created: bool, copy_verified: bool, original_data: Option<(u64, String)> }
impl Session {
    async fn digest(&mut self, namespace: &str, table: &str, columns: &[String]) -> Result<(u64, String)> {
        use futures::TryStreamExt; use sha2::{Digest, Sha256};
        ensure!(!columns.is_empty(), "STRUCTURE_COPY_COLUMNS_UNKNOWN"); let kind = self.kind();
        let fields = columns.iter().enumerate().map(|(i, n)| { let n = db::quote_ident(&kind, n); format!("CASE WHEN {n} IS NULL THEN 'N' ELSE CONCAT('V',HEX(CAST({n} AS BINARY))) END AS __navkit_v{i}") }).collect::<Vec<_>>();
        let sql = format!("SELECT {} FROM {} ORDER BY {}", fields.join(","), db::qualified(&kind, namespace, table), (0..fields.len()).map(|i| format!("BINARY __navkit_v{i}")).collect::<Vec<_>>().join(","));
        let Self::Mysql(c, _) = self else { return Err(anyhow!("STRUCTURE_DIGEST_DIALECT_UNSUPPORTED")); };
        let mut hash = Sha256::new(); let mut count = 0u64; let mut stream = crate::sql_history::query(&sql).fetch(&mut **c);
        while let Some(row) = stream.try_next().await? { for i in 0..fields.len() { let value: String = row.try_get(i)?; hash.update((value.len() as u64).to_le_bytes()); hash.update(value.as_bytes()); } count += 1; }
        hash.update(count.to_le_bytes()); Ok((count, format!("{:x}", hash.finalize())))
    }
}
async fn mysql_lock_proof(session: &mut Session, namespace: &str, table: &str, write: bool) -> Result<()> {
    let mode = if write { "'SHARED_NO_READ_WRITE'" } else { "'SHARED_READ_ONLY','SHARED_NO_WRITE','SHARED_NO_READ_WRITE'" };
    let sql = format!("SELECT CAST(count(*) AS CHAR) FROM performance_schema.metadata_locks m JOIN performance_schema.threads t ON t.THREAD_ID=m.OWNER_THREAD_ID WHERE t.PROCESSLIST_ID=CONNECTION_ID() AND m.OBJECT_TYPE='TABLE' AND m.OBJECT_SCHEMA={} AND m.OBJECT_NAME={} AND m.LOCK_STATUS='GRANTED' AND m.LOCK_DURATION='EXPLICIT' AND m.LOCK_TYPE IN ({mode})", db::quote_literal(namespace), db::quote_literal(table));
    let n = session.scalar(&sql).await.map_err(|e| anyhow!("STRUCTURE_MYSQL_MDL_UNPROVEN: performance_schema 的真实显式锁不可验证：{e:#}"))?;
    ensure!(n.parse::<u64>()? > 0, "STRUCTURE_MYSQL_MDL_UNPROVEN: 表 {}.{} 没有已授予 owned {} 锁；禁止切换", namespace, table, if write { "WRITE" } else { "READ" }); Ok(())
}
async fn mysql_relock(pair: &mut Pair, request: &StructureSyncRequest, binding: &StructureReviewBinding, work: &Work) -> Result<()> {
    let kind = pair.target.kind(); let (shadow, _) = plan::artifacts(request, binding, &request.source_revision, &request.target_revision);
    let source_full = db::qualified(&kind, &request.source_schema, &request.source_table); let target_full = db::qualified(&kind, &request.target_schema, &request.target_table); let shadow_full = db::qualified(&kind, &request.target_schema, &shadow);
    // ALTER TABLE may replace/release table-level metadata locks across implicit commits, depending
    // on server version. Never assume they survived: reacquire ALL locks, then compare locked
    // double CAS and the FULL original row-value digest across that gap. A concurrent change
    // aborts with original untouched by this operation; shadow is retained for recovery.
    if let Some(s) = &mut pair.source { s.execute(&format!("LOCK TABLES {source_full} READ")).await?; pair.target.execute(&format!("LOCK TABLES {target_full} WRITE, {shadow_full} WRITE")).await?; }
    else { pair.target.execute(&format!("LOCK TABLES {source_full} READ, {target_full} WRITE, {shadow_full} WRITE")).await?; }
    mysql_lock_proof(&mut pair.target, &request.target_schema, &request.target_table, true).await?;
    mysql_lock_proof(&mut pair.target, &request.target_schema, &shadow, true).await?;
    if let Some(s) = &mut pair.source { mysql_lock_proof(s, &request.source_schema, &request.source_table, false).await?; }
    else { mysql_lock_proof(&mut pair.target, &request.source_schema, &request.source_table, false).await?; }
    let (source, target) = pair.snapshots(request).await?;
    ensure!(Some(&source.revision) == work.source_before.as_ref().map(|s| &s.revision) && Some(&target.revision) == work.before.as_ref().map(|s| &s.revision), "STRUCTURE_MYSQL_RELOCK_CAS_CHANGED");
    let columns = target.schema.columns.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
    ensure!(Some(pair.target.digest(&request.target_schema, &request.target_table, &columns).await?) == work.original_data, "STRUCTURE_MYSQL_RELOCK_DATA_CHANGED: 隐式 DDL 锁间隙原表发生变化，不切换/覆盖"); Ok(())
}
async fn apply_steps(pair: &mut Pair, entry: &Arc<Entry>, request: &StructureSyncRequest, binding: &StructureReviewBinding, work: &mut Work, skip_first: bool, skip_last: bool) -> Result<()> {
    let steps = work.report.as_ref().ok_or_else(|| anyhow!("STRUCTURE_PLAN_MISSING"))?.steps.clone();
    let start = usize::from(skip_first); let end = steps.len().saturating_sub(usize::from(skip_last));
    for (ordinal, step) in steps.iter().enumerate().take(end).skip(start) {
        check_stop(entry)?;
        match pair.target.execute(&step.sql).await { Ok(_) => { work.steps.push(DesignStepResult { label: step.label.clone(), sql: step.sql.clone(), status: "applied".into(), error: None }); }, Err(e) => {
            work.steps.push(DesignStepResult { label: step.label.clone(), sql: step.sql.clone(), status: "failed".into(), error: Some(format!("{e:#}")) });
            work.steps.extend(steps.iter().skip(ordinal+1).map(|s| DesignStepResult { label: s.label.clone(), sql: s.sql.clone(), status: "skipped".into(), error: None })); return Err(e);
        } }
        if !pair.target.pg() {
            if step.sql.starts_with("INSERT INTO ") { ensure!(pair.target.scalar("SELECT CAST(@@warning_count AS CHAR)").await? == "0", "STRUCTURE_COPY_WARNINGS: 复制有截断/转换警告，禁止切换"); }
            mysql_relock(pair, request, binding, work).await?;
        }
        #[cfg(test)] if entry.binding.owner == "test-stop-after-ddl" && ordinal == start { entry.request_stop(false); }
        #[cfg(test)] if entry.binding.owner == "test-fail-after-ddl" && ordinal == start { return Err(anyhow!("STRUCTURE_TEST_INJECTED_DDL_FAILURE")); }
    }
    Ok(())
}
async fn pg_work(pair: &mut Pair, entry: &Arc<Entry>, request: &StructureSyncRequest, binding: &StructureReviewBinding, work: &mut Work) -> Result<MultiResult> {
    check_stop(entry)?; pair.target_tx = true; pair.target.execute("BEGIN").await?;
    if let Some(s) = &mut pair.source { pair.source_tx = true; s.execute("BEGIN").await?; }
    let kind = pair.target.kind(); let target_full = db::qualified(&kind, &request.target_schema, &request.target_table); let source_full = db::qualified(&kind, &request.source_schema, &request.source_table);
    if let Some(s) = &mut pair.source { s.execute(&format!("LOCK TABLE {source_full} IN ACCESS SHARE MODE")).await?; pair.target.execute(&format!("LOCK TABLE {target_full} IN ACCESS EXCLUSIVE MODE")).await?; }
    else { pair.target.execute(&format!("LOCK TABLE {source_full}, {target_full} IN ACCESS EXCLUSIVE MODE")).await?; }
    check_stop(entry)?; let (source, target) = pair.snapshots(request).await?;
    let mut current = report(&source, &target, &kind, request, binding)?; structure_sync::validate_review(request, &current, binding)?; structure_sync::bind_review(request, &mut current, binding)?;
    ensure!(!current.steps.is_empty(), "STRUCTURE_NO_CHANGES");
    work.report = Some(current); work.before = Some(target.clone()); work.source_before = Some(source.clone());
    apply_steps(pair, entry, request, binding, work, false, false).await?; check_stop(entry)?;
    let after = metadata::snapshot(&mut pair.target, &pair.target_physical, &request.target_schema, &request.target_table).await?;
    let desired = work.report.as_ref().unwrap().final_design.as_ref().unwrap();
    ensure!(after.identity == target.identity && after.unsupported.is_empty(), "STRUCTURE_READBACK_IDENTITY_OR_DEPENDENCY_CHANGED");
    let mismatch = design::verify(&after.schema, desired, &kind); ensure!(mismatch.is_empty(), "STRUCTURE_READBACK_MISMATCH: {}", mismatch.join("；"));
    let source_after = pair.source_snapshot(request).await?; ensure!(source.revision == source_after.revision && source.identity == source_after.identity, "STRUCTURE_SOURCE_CHANGED_BEFORE_COMMIT");
    check_stop(entry)?; work.after = Some(after); Ok(MultiResult { sets: Vec::new(), messages: Vec::new(), elapsed_ms: 0, spool_id: None })
}
async fn mysql_work(pair: &mut Pair, entry: &Arc<Entry>, request: &StructureSyncRequest, binding: &StructureReviewBinding, work: &mut Work) -> Result<MultiResult> {
    check_stop(entry)?;
    // Read/CAS before even CREATE SHADOW, then recheck under metadata WRITE/READ locks.
    pair.begin_read(request).await?; let (source_initial, target_initial) = pair.snapshots(request).await?; let initial_report = report(&source_initial, &target_initial, &pair.target.kind(), request, binding)?;
    structure_sync::validate_review(request, &initial_report, binding)?; ensure!(!initial_report.steps.is_empty(), "STRUCTURE_NO_CHANGES");
    pair.target.execute("ROLLBACK").await?; pair.target_tx = false;
    if let Some(s) = &mut pair.source { s.execute("ROLLBACK").await?; pair.source_tx = false; }
    check_stop(entry)?; let first = &initial_report.steps[0];
    pair.target.execute(&first.sql).await?; work.shadow_created = true; work.report = Some(initial_report.clone());
    work.steps.push(DesignStepResult { label: first.label.clone(), sql: first.sql.clone(), status: "applied".into(), error: None });
    let kind = pair.target.kind(); let (shadow, _) = plan::artifacts(request, binding, &source_initial.revision, &target_initial.revision);
    let target_full = db::qualified(&kind, &request.target_schema, &request.target_table); let source_full = db::qualified(&kind, &request.source_schema, &request.source_table); let shadow_full = db::qualified(&kind, &request.target_schema, &shadow);
    pair.locked = true;
    if let Some(s) = &mut pair.source { s.execute(&format!("LOCK TABLES {source_full} READ")).await?; pair.target.execute(&format!("LOCK TABLES {target_full} WRITE, {shadow_full} WRITE")).await?; }
    else { pair.target.execute(&format!("LOCK TABLES {source_full} READ, {target_full} WRITE, {shadow_full} WRITE")).await?; }
    check_stop(entry)?; let (source, target) = pair.snapshots(request).await?;
    let mut locked_report = report(&source, &target, &kind, request, binding)?;
    structure_sync::validate_review(request, &locked_report, binding)?; structure_sync::bind_review(request, &mut locked_report, binding)?;
    ensure!(locked_report.steps == initial_report.steps, "STRUCTURE_LOCKED_RECOMPILE_CHANGED");
    // CREATE LIKE was outside table locks. Verify the cloned table still exactly matches locked original before any ALTER.
    let clone = metadata::snapshot(&mut pair.target, &pair.target_physical, &request.target_schema, &shadow).await?;
    let mut clone_schema = clone.schema.clone(); clone_schema.table = target.schema.table.clone();
    ensure!(serde_json::to_value(&clone_schema)? == serde_json::to_value(&target.schema)? && clone.unsupported.is_empty(), "STRUCTURE_SHADOW_CLONE_STALE");
    work.before = Some(target.clone()); work.source_before = Some(source.clone()); work.report = Some(locked_report);
    let desired = work.report.as_ref().unwrap().final_design.clone().unwrap(); let retained = plan::retained(&desired, &target.schema);
    let all_columns = target.schema.columns.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
    work.original_data = Some(pair.target.digest(&request.target_schema, &request.target_table, &all_columns).await?);
    mysql_relock(pair, request, binding, work).await?;
    let original_digest = pair.target.digest(&request.target_schema, &request.target_table, &retained).await?;
    apply_steps(pair, entry, request, binding, work, true, true).await?;
    // Strict mode still can produce warnings (date coercion etc.). Any copy warning is a hard failure.
    let warnings = pair.target.scalar("SELECT CAST(@@warning_count AS CHAR)").await?; ensure!(warnings == "0", "STRUCTURE_COPY_WARNINGS: 不切换可能截断/转换的数据");
    check_stop(entry)?;
    let after = metadata::snapshot(&mut pair.target, &pair.target_physical, &request.target_schema, &shadow).await?;
    let mut shadow_design = desired; shadow_design.table = shadow.clone();
    let mismatch = design::verify(&after.schema, &shadow_design, &kind); ensure!(mismatch.is_empty() && after.unsupported.is_empty(), "STRUCTURE_SHADOW_READBACK_MISMATCH: {}", mismatch.join("；"));
    ensure!(pair.target.digest(&request.target_schema, &shadow, &retained).await? == original_digest, "STRUCTURE_COPY_DATA_CHANGED: 保留列字节/行数不等，不发送 RENAME");
    ensure!(pair.target.digest(&request.target_schema, &request.target_table, &retained).await? == original_digest, "STRUCTURE_ORIGINAL_DATA_CHANGED");
    let source_after = pair.source_snapshot(request).await?; ensure!(source.revision == source_after.revision && source.identity == source_after.identity, "STRUCTURE_SOURCE_CHANGED_BEFORE_SWAP");
    let target_after = metadata::snapshot(&mut pair.target, &pair.target_physical, &request.target_schema, &request.target_table).await?; ensure!(target.revision == target_after.revision, "STRUCTURE_TARGET_CHANGED_BEFORE_SWAP");
    check_stop(entry)?; work.after = Some(after); work.copy_verified = true;
    // Atomic RENAME is deliberately sent only AFTER drive joins any directed cancellation.
    Ok(MultiResult { sets: Vec::new(), messages: Vec::new(), elapsed_ms: 0, spool_id: None })
}
async fn cancel(db: &Db, id: &str) -> Result<()> {
    match db { Db::Postgres(pool) => { let mut c = sqlx::PgConnection::connect_with(&pool.connect_options()).await?;
        let result = crate::sql_history::query_scalar::<_, bool>("SELECT pg_catalog.pg_cancel_backend($1)").bind(id.parse::<i32>()?).fetch_one(&mut c).await; let closed = c.close().await;
        ensure!(result?, "STRUCTURE_CANCEL_NOT_ACKNOWLEDGED"); closed?; },
        Db::Mysql(pool) => { let mut c = sqlx::MySqlConnection::connect_with(&pool.connect_options()).await?;
            let id = id.parse::<u64>()?; let result = crate::sql_history::query(&format!("KILL QUERY {id}")).execute(&mut c).await; let closed = c.close().await; result?; closed?; },
        _ => return Err(anyhow!("STRUCTURE_CANCEL_ADAPTER_UNSUPPORTED")),
    } Ok(())
}
fn receipt(work: &Work, transaction: &str, verified: bool, message: Option<String>, schema: Option<crate::models::TableSchema>) -> Option<StructureSyncReceipt> {
    work.report.clone().map(|report| StructureSyncReceipt { report, receipt: DesignReceipt { steps: work.steps.iter().cloned().map(|mut s| { if transaction == "rolled_back" && s.status == "applied" { s.status = "rolled_back".into(); } s }).collect(), verified, unverified: false, mismatches: message.into_iter().collect(), transaction: transaction.into(), schema } })
}
async fn finish_pg(pair: &mut Pair, entry: &Arc<Entry>, work: &Work, result: Result<MultiResult>) -> (Result<MultiResult>, Option<StructureSyncReceipt>, bool, bool) {
    let stopped = entry.stopping(); let uncertain = result.as_ref().err().is_some_and(transport);
    if stopped && !pair.target_tx && work.before.is_none() { entry.record_remote_transaction("none"); return (Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "停止发生在结构事务/DDL发送前，未修改表").into()), None, true, false); }
    if uncertain { return (Err(unknown("STRUCTURE_PG_WORK_UNKNOWN: 工作连接/DDL 未确认，不发布 receipt、不重放").into()), None, false, true); }
    if result.is_ok() && !stopped {
        if let Err(e) = pair.target.execute("COMMIT").await { return (Err(unknown(format!("STRUCTURE_COMMIT_ACK_UNKNOWN: {e:#}")).into()), None, false, true); }
        pair.target_tx = false; entry.record_remote_transaction("committed");
        #[cfg(test)] if entry.binding.owner == "test-lose-commit-ack" { return (Err(unknown("STRUCTURE_COMMIT_ACK_UNKNOWN: 测试丢弃真实 COMMIT ACK").into()), None, false, true); }
        return (result, receipt(work, "committed", true, None, work.after.as_ref().map(|s| s.schema.clone())), false, false);
    }
    if pair.target_tx { if let Err(e) = pair.target.execute("ROLLBACK").await { return (Err(unknown(format!("STRUCTURE_ROLLBACK_ACK_UNKNOWN: {e:#}")).into()), None, false, true); } pair.target_tx = false; }
    entry.record_remote_transaction("rolled_back");
    let original = format!("{}", result.as_ref().err().map(|e| format!("{e:#}")).unwrap_or_else(|| "COMMIT 前停止".into()));
    let error = if stopped { AppError::coded("STRUCTURE_STOPPED_ROLLED_BACK", format!("PostgreSQL ROLLBACK 已确认：{original}")) } else { AppError::coded("STRUCTURE_APPLY_FAILED_ROLLED_BACK", format!("PostgreSQL ROLLBACK 已确认：{original}")) };
    (Err(error.into()), receipt(work, "rolled_back", false, Some(original), work.before.as_ref().map(|s| s.schema.clone())), stopped, false)
}
async fn finish_mysql(pair: &mut Pair, entry: &Arc<Entry>, request: &StructureSyncRequest, binding: &StructureReviewBinding, work: &mut Work, result: Result<MultiResult>) -> (Result<MultiResult>, Option<StructureSyncReceipt>, bool, bool) {
    if result.as_ref().err().is_some_and(transport) { entry.record_remote_transaction("unknown"); return (Err(unknown("STRUCTURE_MYSQL_WORK_UNKNOWN: 影子 DDL/复制或连接结果未确认；原表/影子由操作者重新核验，不声称回滚").into()), None, false, true); }
    let stopped = entry.stopping();
    if stopped && !work.shadow_created { entry.record_remote_transaction("none"); return (Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "停止发生在影子 DDL 发送前，没有修改目标表").into()), None, true, false); }
    if result.is_err() || stopped {
        let message = result.as_ref().err().map(|e| format!("{e:#}")).unwrap_or_else(|| "原子切换前停止".into());
        // No RENAME has been sent. Original remains original; shadow artifacts are retained, not deleted.
        entry.record_remote_transaction(if work.shadow_created { "unknown" } else { "none" });
        let code = if stopped { "STRUCTURE_STOPPED_ORIGINAL_RETAINED" } else { "STRUCTURE_SHADOW_FAILED_ORIGINAL_RETAINED" };
        let r = receipt(work, "shadow_failed_original_retained", false, Some(message.clone()), work.before.as_ref().map(|s| s.schema.clone()));
        return (Err(AppError::coded(code, format!("MySQL 未发送原子 RENAME；原表保留，影子可能存在并保留，不是 ROLLBACK：{message}")).into()), r, stopped, false);
    }
    if let Err(e) = ensure_swap(work) { return (Err(e), None, false, true); }
    let report = work.report.as_ref().unwrap(); let rename = report.steps.last().unwrap().clone();
    match pair.target.execute(&rename.sql).await {
        Err(e) if transport(&e) => { entry.record_remote_transaction("unknown"); return (Err(unknown(format!("STRUCTURE_RENAME_ACK_UNKNOWN: 不重放/盲目恢复：{e:#}")).into()), None, false, true); },
        Err(e) => { work.steps.push(DesignStepResult { label: rename.label, sql: rename.sql, status: "failed".into(), error: Some(format!("{e:#}")) });
            return (Err(AppError::coded("STRUCTURE_SHADOW_FAILED_ORIGINAL_RETAINED", format!("原子 RENAME 被服务器拒绝；原表与影子保留：{e:#}")).into()), receipt(work, "shadow_failed_original_retained", false, Some(format!("{e:#}")), work.before.as_ref().map(|s| s.schema.clone())), false, false); },
        Ok(_) => work.steps.push(DesignStepResult { label: rename.label, sql: rename.sql, status: "applied".into(), error: None }),
    }
    #[cfg(test)] if entry.binding.owner == "test-lose-rename-ack" { return (Err(unknown("STRUCTURE_RENAME_ACK_UNKNOWN: 测试丢弃真实原子 RENAME ACK").into()), None, false, true); }
    let after = metadata::snapshot(&mut pair.target, &pair.target_physical, &request.target_schema, &request.target_table).await;
    let verified = after.as_ref().is_ok_and(|s| s.identity.rootpage == work.after.as_ref().unwrap().identity.rootpage && s.unsupported.is_empty() && design::verify(&s.schema, work.report.as_ref().unwrap().final_design.as_ref().unwrap(), &pair.target.kind()).is_empty());
    #[cfg(test)] let verified = verified && entry.binding.owner != "test-force-swap-readback-mismatch";
    if verified { entry.record_remote_transaction("committed"); let after = after.unwrap();
        return (result, receipt(work, "shadow_swapped_backup_retained", true, None, Some(after.schema)), false, false); }
    // An acknowledged RENAME followed by a deterministic readback mismatch can be reversed only under
    // the still-held WRITE locks and after proving BOTH real table IDs. A transport error is Unknown.
    if after.as_ref().err().is_some_and(transport) { return (Err(unknown("STRUCTURE_SWAP_READBACK_UNKNOWN: 切换 ACK 已收但读回未确认，备份保留，不盲目恢复").into()), None, false, true); }
    let (shadow, backup) = plan::artifacts(request, binding, &request.source_revision, &request.target_revision); let kind = pair.target.kind();
    let original_backup = metadata::snapshot(&mut pair.target, &pair.target_physical, &request.target_schema, &backup).await;
    if !after.as_ref().is_ok_and(|s| s.identity.rootpage == work.after.as_ref().unwrap().identity.rootpage) || !original_backup.as_ref().is_ok_and(|s| s.identity.rootpage == work.before.as_ref().unwrap().identity.rootpage) { return (Err(unknown("STRUCTURE_RECOVERY_IDENTITY_UNPROVEN: 不自动恢复，不改终态成功").into()), None, false, true); }
    let recovery = format!("RENAME TABLE {} TO {}, {} TO {}", db::qualified(&kind, &request.target_schema, &request.target_table), db::qualified(&kind, &request.target_schema, &shadow), db::qualified(&kind, &request.target_schema, &backup), db::qualified(&kind, &request.target_schema, &request.target_table));
    if let Err(e) = pair.target.execute(&recovery).await { return (Err(unknown(format!("STRUCTURE_RECOVERY_RENAME_UNKNOWN: {e:#}")).into()), None, false, true); }
    work.steps.push(DesignStepResult { label: "切换读回失败后原子恢复原表，保留失败影子".into(), sql: recovery, status: "applied".into(), error: None });
    let restored = metadata::snapshot(&mut pair.target, &pair.target_physical, &request.target_schema, &request.target_table).await;
    let columns = work.before.as_ref().unwrap().schema.columns.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
    let restored_data = pair.target.digest(&request.target_schema, &request.target_table, &columns).await;
    if !restored.as_ref().is_ok_and(|s| s.identity == work.before.as_ref().unwrap().identity && s.revision == work.before.as_ref().unwrap().revision) || restored_data.ok().as_ref() != work.original_data.as_ref() { return (Err(unknown("STRUCTURE_RECOVERY_READBACK_UNKNOWN: 原表身份/结构/完整数据恢复读回未确认").into()), None, false, true); }
    entry.record_remote_transaction("unknown");
    (Err(AppError::coded("STRUCTURE_SHADOW_RESTORED", "切换后核验失败，原子恢复原表已确认；失败影子保留，不是假称事务回滚").into()), receipt(work, "shadow_restored_backup_retained", false, Some("原表恢复已确认；新影子保留供人工检查".into()), restored.ok().map(|s| s.schema)), false, false)
}
fn ensure_swap(work: &Work) -> Result<()> { ensure!(work.copy_verified && work.before.is_some() && work.after.is_some() && work.report.is_some(), "STRUCTURE_SWAP_NOT_VERIFIED"); Ok(()) }
/// Same ABI as SQLite structure_execution::run. The caller alone finishes the C1 entry.
pub async fn run(source: Arc<Db>, source_kind: DbKind, target: Arc<Db>, target_kind: DbKind, entry: Arc<Entry>, ordinary: OrdinaryLease,
    request: StructureSyncRequest, binding: StructureReviewBinding, read_only: bool) -> BoundResult {
    let valid = async { if read_only { return Err(AppError::coded("STRUCTURE_READ_ONLY", "目标只读，未取 pool/发送 DDL")); }
        validate(&request, &binding).map_err(AppError::from)?; structure_sync::validate_review_request(&request, &binding).map_err(AppError::from)?;
        crate::structure_commands::validate_target(&binding, &entry.binding)?;
        if !native_pair(&source, &source_kind, &target, &target_kind) { return Err(AppError::coded("STRUCTURE_NATIVE_DIALECT_UNSUPPORTED", "实际适配器/配置方言未认证")); } Ok(()) }.await;
    if let Err(error) = valid { return BoundResult { result: Err(error), confirmed: false, receipt: None }; }
    let published = Arc::new(parking_lot::Mutex::new(None)); let output = published.clone(); let worker = entry.clone();
    let (result, confirmed) = execution::supervise(entry.clone(), async move {
        let _ordinary = ordinary; let started = Instant::now();
        if worker.stopping() || !worker.start() { return (Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "排队停止，没有发送 DDL")), true); }
        let mut pair = match Pair::acquire(&source, &target, &request, Some(&worker)).await { Ok(p) => p, Err(e) => { let unsent = e.downcast_ref::<AppError>().is_some_and(|e| e.code == "EXECUTION_CANCELLED_BEFORE_SEND"); return (Err(AppError::from(e)), unsent); } };
        let ids = async { let sql = if pair.target.pg() { "SELECT pg_backend_pid()::text" } else { "SELECT CAST(CONNECTION_ID() AS CHAR)" };
            let target_id = pair.target.scalar(sql).await?; let source_id = if let Some(s) = &mut pair.source { Some(s.scalar(sql).await?) } else { None }; Ok::<_, anyhow::Error>((target_id, source_id)) }.await;
        let (target_id, source_id) = match ids { Ok(v) => v, Err(e) => { let _ = pair.cleanup(true).await; return (Err(AppError::from(e)), false); } };
        let pg = pair.target.pg(); let mut work = Work::default();
        let (result, _) = execution::drive(async { if pg { pg_work(&mut pair, &worker, &request, &binding, &mut work).await } else { mysql_work(&mut pair, &worker, &request, &binding, &mut work).await } }, &worker, || async {
            let target_cancel = cancel(&target, &target_id);
            let source_cancel = async { if let Some(id) = &source_id { cancel(&source, id).await } else { Ok(()) } };
            let (t, s) = tokio::join!(target_cancel, source_cancel); t?; s?; Ok(())
        }).await;
        let (mut result, receipt, confirmed, destroy) = if pg { finish_pg(&mut pair, &worker, &work, result).await } else { finish_mysql(&mut pair, &worker, &request, &binding, &mut work, result).await };
        if let Err(e) = pair.cleanup(destroy).await { return (Err(unknown(format!("STRUCTURE_OWNED_CLEANUP_UNKNOWN: {e:#}"))), false); }
        if let Ok(r) = &mut result { r.elapsed_ms = started.elapsed().as_millis() as u64; r.messages.push(if pg { "PostgreSQL 结构 DDL 已在 owned 锁内读回核验并收到 COMMIT ACK" } else { "MySQL owned 影子复制与保留数据已核验，原子 RENAME 已确认，原表备份永久保留；不是事务回滚" }.into()); }
        *output.lock() = receipt; (result.map_err(AppError::from), confirmed)
    }).await;
    let publish = result.is_ok() || result.as_ref().err().is_some_and(|e| ["STRUCTURE_APPLY_FAILED_ROLLED_BACK", "STRUCTURE_STOPPED_ROLLED_BACK", "STRUCTURE_SHADOW_FAILED_ORIGINAL_RETAINED", "STRUCTURE_STOPPED_ORIGINAL_RETAINED", "STRUCTURE_SHADOW_RESTORED"].contains(&e.code.as_str()));
    let receipt = if publish { published.lock().take() } else { None };
    BoundResult { result, confirmed, receipt }
}
