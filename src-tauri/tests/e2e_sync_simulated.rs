//! Explicit opt-in ACTUAL synchronize/diff calls over stateful protocol simulations.
//! NOT vendor databases/certification. No user data/, stored connections, or secrets.
//! Run this target serially; reports checkpoint before every case and after cleanup.
#[path = "support/sync_simulated/mod.rs"]
mod support;

use anyhow::{ensure, Result};
use futures::FutureExt;
use navkit_lib::{models::Protocol, sync::{self, SyncColumnMapping, SyncOptions}, transfer};
use serde_json::{json, Value};
use support::{Endpoint, Report, World};

async fn synchronize(source: &Endpoint, target: &Endpoint, opts: &mut SyncOptions) -> Result<sync::SyncReport> {
    opts.task_id = Some(uuid::Uuid::new_v4().to_string());
    sync::synchronize(source.db.as_ref(), &source.kind, target.db.as_ref(), &target.kind, opts).await
}

/// Empty/structure/data/both/recreate, filtered mapping and exact complete readback.
/// CH replace is a separate desired-behavior regression, not hidden in a baseline.
async fn full_modes(source: &Endpoint, target: &Endpoint, nullable: bool) -> Result<Value> {
    let src = support::table();
    let dst = support::table();
    let expected = support::seed(source, &src, nullable).await?;
    ensure!(expected.len() == 2 && expected[0][0].as_deref() == Some("1"), "fixture must hold real sentinel rows");
    let mut opts = support::options(source, &src, target, &dst);
    opts.mode = "structure".into();
    let structure = synchronize(source, target, &mut opts).await?;
    ensure!(structure.created && structure.rows == 0 && support::read(target, &dst).await?.is_empty(), "structure-only changed row state");
    opts.mode = "data".into();
    opts.create_target = false;
    let data = synchronize(source, target, &mut opts).await?;
    let copied = support::read(target, &dst).await?;
    ensure!(data.rows == 2 && copied == expected, "data readback mismatch: expected={expected:?}, actual={copied:?}");
    let compared = sync::diff(source.db.as_ref(), &source.kind, target.db.as_ref(), &target.kind, &support::diff_request(&opts)).await?;
    ensure!(compared.source_rows == 2 && compared.target_rows == 2 && compared.missing_in_target == 0 && compared.extra_in_target == 0 && compared.changed == 0, "diff disagrees with exact complete readback: {compared:?}");
    let mut tested = vec!["structure", "data", "both/recreate", "where+column mapping", "diff+exact complete readback"];
    if target.kind.protocol() != Protocol::Clickhouse {
        opts.write_mode = "replace".into();
        let replace = synchronize(source, target, &mut opts).await?;
        ensure!(replace.rows == 2 && support::read(target, &dst).await? == expected, "replace did not retain exact source state");
        tested.push("data/replace");
    }
    opts.mode = "both".into();
    opts.write_mode = "recreate".into();
    opts.create_target = true;
    let recreated = synchronize(source, target, &mut opts).await?;
    ensure!(recreated.created && recreated.rows == 2 && support::read(target, &dst).await? == expected, "recreate failed exact readback");
    let mapped = support::table();
    let mut mapping = support::options(source, &src, target, &mapped);
    mapping.where_clause = Some(format!("{} >= 2", navkit_lib::db::quote_ident(&source.kind, "ID")));
    mapping.columns = vec![SyncColumnMapping {source: "ID".into(), target: "KEY".into(), target_type: None}, SyncColumnMapping {source: "PAYLOAD".into(), target: "TITLE".into(), target_type: None}];
    let mapped_report = synchronize(source, target, &mut mapping).await?;
    let projected = vec![vec![expected[1][0].clone(), expected[1][1].clone()]];
    let mapped_rows = support::read(target, &mapped).await?;
    ensure!(mapped_report.rows == 1 && mapped_rows == projected, "mapped/filter readback mismatch: expected={projected:?}, actual={mapped_rows:?}");
    Ok(json!({"source_kind": source.kind.id(), "target_kind": target.kind.id(), "source_table": src, "target_table": dst, "tested": tested, "source_rows": expected, "target_rows": copied, "diff": compared, "mapping_rows": mapped_rows, "source_has_null": nullable}))
}

async fn keyed_modes(source: &Endpoint, target: &Endpoint) -> Result<Value> {
    let baseline = full_modes(source, target, true).await?;
    let src = support::table();
    let dst = support::table();
    let original = support::seed(source, &src, true).await?;
    let mut opts = support::options(source, &src, target, &dst);
    synchronize(source, target, &mut opts).await?;
    let source_full = support::full(source, &src);
    let target_full = support::full(target, &dst);
    let changed = format!("updated:{}:中文🙂", uuid::Uuid::new_v4());
    support::execute(source, &format!("UPDATE {source_full} SET {}={} WHERE {}=1", navkit_lib::db::quote_ident(&source.kind, "PAYLOAD"), support::literal(source, &changed), navkit_lib::db::quote_ident(&source.kind, "ID"))).await?;
    support::execute(source, &format!("INSERT INTO {source_full} VALUES (3,{},NULL)", support::literal(source, "third"))).await?;
    support::execute(target, &format!("INSERT INTO {target_full} VALUES (9,{},NULL)", support::literal(target, "extra"))).await?;
    let before = sync::diff(source.db.as_ref(), &source.kind, target.db.as_ref(), &target.kind, &support::diff_request(&opts)).await?;
    ensure!(before.keyed && before.missing_in_target == 1 && before.extra_in_target == 1 && before.changed == 1, "keyed diff expected missing/extra/changed=1: {before:?}");
    opts.mode = "data".into();
    opts.sync_mode = "incremental".into();
    opts.create_target = false;
    let insert = synchronize(source, target, &mut opts).await?;
    let inserted_rows = support::read(target, &dst).await?;
    let mut expected_insert = original;
    expected_insert.push(vec![Some("3".into()), Some("third".into()), None]);
    expected_insert.push(vec![Some("9".into()), Some("extra".into()), None]);
    expected_insert.sort();
    ensure!(insert.rows == 1 && inserted_rows == expected_insert, "incremental insert must preserve changed row and extra row: {inserted_rows:?}");
    opts.incremental_update = true;
    let update = synchronize(source, target, &mut opts).await?;
    let after_rows = support::read(target, &dst).await?;
    let mut expected_after = support::read(source, &src).await?;
    expected_after.push(vec![Some("9".into()), Some("extra".into()), None]);
    expected_after.sort();
    ensure!(update.rows == 1 && after_rows == expected_after, "incremental update must exactly update one row and preserve extra: {after_rows:?}");
    let after = sync::diff(source.db.as_ref(), &source.kind, target.db.as_ref(), &target.kind, &support::diff_request(&opts)).await?;
    ensure!(after.keyed && after.missing_in_target == 0 && after.changed == 0 && after.extra_in_target == 1, "post-incremental diff mismatch: {after:?}");
    let repeated = synchronize(source, target, &mut opts).await?;
    ensure!(repeated.rows == 0 && support::read(target, &dst).await? == expected_after, "incremental repetition not idempotent");
    Ok(json!({"baseline": baseline, "before_diff": before, "insert_rows": insert.rows, "insert_readback": inserted_rows, "update_rows": update.rows, "update_readback": after_rows, "after_diff": after, "idempotent_rows": repeated.rows}))
}

async fn generic_partial_failure(source: &Endpoint, target: &Endpoint) -> Result<Value> {
    let src = support::table();
    let dst = support::table();
    support::seed(source, &src, true).await?;
    let full = support::full(target, &dst);
    let payload = if target.kind.protocol() == Protocol::Mssql {"NVARCHAR(MAX)"} else {"VARCHAR(1000)"};
    support::execute(target, &format!("CREATE TABLE {full} (ID BIGINT NOT NULL PRIMARY KEY CHECK(ID<>2),PAYLOAD {payload} NOT NULL,OPTIONAL {payload})")).await?;
    let mut opts = support::options(source, &src, target, &dst);
    opts.mode = "data".into();
    opts.create_target = false;
    let result = synchronize(source, target, &mut opts).await;
    let rows = support::read(target, &dst).await?;
    let progress = transfer::task_progress(opts.task_id.as_deref().unwrap())?;
    ensure!(result.is_err() && rows.len() == 1 && rows[0][0].as_deref() == Some("1") && progress.status == "failed", "generic target must expose non-atomic earlier write, not falsely claim rollback: result={result:?}, rows={rows:?}, progress={progress:?}");
    Ok(json!({"sync_error": format!("{:#}", result.unwrap_err()), "committed_rows_after_error": rows, "task": progress, "atomicity": "NON_TRANSACTIONAL_EARLIER_WRITE_REMAINS"}))
}

async fn tds_binary(source: &Endpoint, target: &Endpoint) -> Result<Value> {
    let src = support::table();
    let dst = support::table();
    let full = support::full(source, &src);
    if source.kind.protocol() == Protocol::Mssql {
        support::execute(source, &format!("CREATE TABLE {full} (ID BIGINT NOT NULL PRIMARY KEY,PAYLOAD VARBINARY(MAX) NOT NULL)")).await?;
        support::execute(source, &format!("INSERT INTO {full} VALUES (1,0x00FF1080)")).await?;
    } else {
        support::execute(source, &format!("CREATE TABLE {full} (ID INTEGER NOT NULL PRIMARY KEY,PAYLOAD BLOB NOT NULL)")).await?;
        support::execute(source, &format!("INSERT INTO {full} VALUES (1,X'00FF1080')")).await?;
    }
    let before = support::read(source, &src).await?;
    let mut opts = support::options(source, &src, target, &dst);
    let result = synchronize(source, target, &mut opts).await;
    let report = result.map_err(|e| anyhow::anyhow!("lossless binary synchronization failed: {e:#}; source_readback={before:?}"))?;
    let copied = support::read(target, &dst).await?;
    ensure!(report.rows == 1 && copied.len() == 1 && copied[0][1].as_deref().map(str::to_ascii_uppercase).as_deref() == Some("0X00FF1080"), "binary bytes must survive NUL/non-UTF8: source={before:?}, target={copied:?}");
    Ok(json!({"source_rows": before, "target_rows": copied, "expected_bytes_hex": "00FF1080", "sync": report}))
}

async fn odbc_binary_four_values(source: &Endpoint, target: &Endpoint) -> Result<Value> {
    let src = support::table();
    let dst = support::table();
    let source_full = support::full(source, &src);
    support::execute(source, &format!("CREATE TABLE {source_full} (ID INTEGER NOT NULL PRIMARY KEY,PAYLOAD BLOB)")).await?;
    let small = if source.kind.protocol() == Protocol::Odbc {"from_hex('00FF1080')"} else {"X'00FF1080'"};
    let empty = if source.kind.protocol() == Protocol::Odbc {"from_hex('')"} else {"X''"};
    let mixed = if source.kind.protocol() == Protocol::Odbc {"from_hex('00017F80FEFF41C3A9F09F9982')"} else {"X'00017F80FEFF41C3A9F09F9982'"};
    // from_hex is fixture-only DuckDB setup, never evidence of vendor SQL support.
    support::execute(source, &format!("INSERT INTO {source_full} VALUES (1,{small}),(2,NULL),(3,{empty}),(4,{mixed})")).await?;
    let expected = vec![
        vec![Some("1".to_string()), Some("0X00FF1080".to_string())],
        vec![Some("2".to_string()), None],
        vec![Some("3".to_string()), Some("0X".to_string())],
        vec![Some("4".to_string()), Some("0X00017F80FEFF41C3A9F09F9982".to_string())],
    ];
    let normalize_hex = |rows: &[Vec<Option<String>>]| rows.iter().map(|row| {
        row.iter().enumerate().map(|(i, value)| if i == 1 { value.as_ref().map(|v| v.to_ascii_uppercase()) } else { value.clone() }).collect::<Vec<_>>()
    }).collect::<Vec<_>>();
    let before = support::read(source, &src).await?;
    ensure!(normalize_hex(&before) == expected, "source binary readback must preserve raw bytes/NULL/empty with 0x prefix, not driver text escapes: actual={before:?}, expected={expected:?}");
    let mut opts = support::options(source, &src, target, &dst);
    let result = synchronize(source, target, &mut opts).await;
    let report = result.map_err(|e| anyhow::anyhow!("four-value binary synchronization failed: {e:#}; source_readback={before:?}"))?;
    let copied = support::read(target, &dst).await?;
    ensure!(report.rows == 4 && normalize_hex(&copied) == expected, "four-value binary readback mismatch: source={before:?}, target={copied:?}, expected={expected:?}");
    let storage_evidence = match target.db.as_ref() {
        navkit_lib::db::Db::Sqlite(pool) => {
            use sqlx::Row;
            let physical = sqlx::query(&format!("SELECT ID,typeof(PAYLOAD) AS STORAGE_CLASS,PAYLOAD FROM {} ORDER BY ID", support::full(target, &dst))).fetch_all(pool).await?;
            let expected_bytes = vec![Some(vec![0x00u8,0xFF,0x10,0x80]), None, Some(Vec::new()), Some(vec![0x00u8,0x01,0x7F,0x80,0xFE,0xFF,0x41,0xC3,0xA9,0xF0,0x9F,0x99,0x82])];
            let mut decoded = Vec::new();
            let mut classes = Vec::new();
            for row in &physical {
                classes.push(row.try_get::<String,_>("STORAGE_CLASS")?);
                decoded.push(row.try_get::<Option<Vec<u8>>,_>("PAYLOAD")?);
            }
            ensure!(classes == vec!["blob", "null", "blob", "blob"] && decoded == expected_bytes, "SQLite storage must contain actual BLOB bytes, never a text hex surrogate: storage={classes:?}, bytes={decoded:?}");
            json!({"storage_classes": classes, "decoded_bytes": decoded})
        }
        navkit_lib::db::Db::Odbc(_) => {
            let (_, byte_rows) = target.db.generic_rows(&format!("SELECT ID,typeof(PAYLOAD),hex(PAYLOAD) FROM {} ORDER BY ID", support::full(target, &dst))).await?;
            let expected_physical: Vec<Vec<Option<String>>> = vec![
                vec![Some("1".into()),Some("BLOB".into()),Some("00FF1080".into())],
                vec![Some("2".into()),Some("BLOB".into()),None],
                vec![Some("3".into()),Some("BLOB".into()),Some("".into())],
                vec![Some("4".into()),Some("BLOB".into()),Some("00017F80FEFF41C3A9F09F9982".into())],
            ];
            ensure!(byte_rows == expected_physical, "actual DuckDB BLOB/hex mismatch: {byte_rows:?}");
            json!({"fixture_backend_type_and_hex": byte_rows})
        }
        _ => unreachable!("four-value ODBC regression is limited to SQLite/ODBC"),
    };
    Ok(json!({"source_kind": source.kind.id(), "target_kind": target.kind.id(), "source_rows": before, "target_rows": copied, "expected_rows": expected, "cases": ["NUL_and_non_UTF8", "NULL", "empty_BLOB", "mixed_binary_UTF8_bytes"], "physical_storage": storage_evidence, "sync": report}))
}

async fn tds_cancellation(world: &World, source: &Endpoint, target: &Endpoint, last: bool) -> Result<Value> {
    let src = support::table();
    let dst = support::table();
    support::seed(source, &src, true).await?;
    if !last { support::execute(source, &format!("INSERT INTO {} VALUES (3,'third',NULL)", support::full(source, &src))).await?; }
    support::execute(target, &format!("CREATE TABLE {} (ID BIGINT NOT NULL PRIMARY KEY,PAYLOAD NVARCHAR(MAX) NOT NULL,OPTIONAL NVARCHAR(MAX))", support::full(target, &dst))).await?;
    let mut opts = support::options(source, &src, target, &dst);
    opts.mode = "data".into();
    opts.create_target = false;
    let task = opts.task_id.clone().unwrap();
    let directory = world.hook(&dst, 2, "pause")?;
    // Do not renew task here: the hook cancels this exact id while write is in flight.
    let (result, cancel) = tokio::join!(sync::synchronize(source.db.as_ref(), &source.kind, target.db.as_ref(), &target.kind, &opts), support::task_cancel_at_hook(&directory, &dst, &task));
    cancel?;
    let rows = support::read(target, &dst).await?;
    let progress = transfer::task_progress(&task)?;
    ensure!(result.is_err() && progress.status == "cancelled" && rows.len() == 2, "cancel during {} statement must not become completed or invent rollback: result={result:?}, task={progress:?}, committed_rows={rows:?}", if last {"last"} else {"middle"});
    Ok(json!({"cancelled_in_flight_ordinal": 2, "last_statement": last, "sync_error": format!("{:#}", result.unwrap_err()), "task": progress, "committed_rows": rows, "atomicity": "NON_TRANSACTIONAL_CURRENT_AND_EARLIER_WRITE_REMAIN"}))
}

async fn ch_incremental(source: &Endpoint, target: &Endpoint) -> Result<Value> {
    let src = support::table();
    let dst = support::table();
    support::seed(source, &src, false).await?;
    let mut opts = support::options(source, &src, target, &dst);
    synchronize(source, target, &mut opts).await?;
    support::execute(source, &format!("INSERT INTO {} VALUES (3,'third','present')", support::full(source, &src))).await?;
    opts.mode = "data".into();
    opts.sync_mode = "incremental".into();
    opts.create_target = false;
    let inserted = synchronize(source, target, &mut opts).await?;
    let before = support::read(target, &dst).await?;
    ensure!(inserted.rows == 1 && before == support::read(source, &src).await?, "CH incremental insertion failed exact readback");
    support::execute(source, &format!("UPDATE {} SET PAYLOAD='changed' WHERE ID=1", support::full(source, &src))).await?;
    // A missing row would be queued before UPDATE in the old generic path. Keep
    // both changes pending so rejecting UPDATE must also prevent that INSERT.
    support::execute(source, &format!("INSERT INTO {} VALUES (4,'must-not-insert','present')", support::full(source, &src))).await?;
    let before_diff = sync::diff(source.db.as_ref(), &source.kind, target.db.as_ref(), &target.kind, &support::diff_request(&opts)).await?;
    ensure!(before_diff.keyed && before_diff.missing_in_target == 1 && before_diff.changed == 1, "CH rejection fixture must contain both a missing row and a changed row: {before_diff:?}");
    opts.incremental_update = true;
    let result = synchronize(source, target, &mut opts).await;
    let after = support::read(target, &dst).await?;
    let after_diff = sync::diff(source.db.as_ref(), &source.kind, target.db.as_ref(), &target.kind, &support::diff_request(&opts)).await?;
    let progress = transfer::task_progress(opts.task_id.as_deref().unwrap())?;
    // Current generic UPDATE is not a CH statement. Explicit safe rejection is valid;
    // pretending it executed, or inserting the missing row before rejecting, is not.
    ensure!(result.is_err() && before == after && after_diff.missing_in_target == 1 && after_diff.changed == 1 && progress.processed == 0, "CH unsupported UPDATE must reject before any write, preserving the missing row as well: result={result:?}, before={before:?}, after={after:?}, after_diff={after_diff:?}, task={progress:?}");
    Ok(json!({"incremental_insert": inserted, "insert_readback": before, "rejected_mixed_batch": {"missing_rows": 1, "changed_rows": 1}, "before_rejection_diff": before_diff, "incremental_update_status": "EXPLICIT_REJECTION_NO_TARGET_CHANGE", "update_error": format!("{:#}", result.unwrap_err()), "readback_after_rejection": after, "after_rejection_diff": after_diff, "task": progress}))
}

async fn ch_replace(source: &Endpoint, target: &Endpoint) -> Result<Value> {
    let src = support::table();
    let dst = support::table();
    support::seed(source, &src, false).await?;
    let mut opts = support::options(source, &src, target, &dst);
    synchronize(source, target, &mut opts).await?;
    support::execute(source, &format!("UPDATE {} SET PAYLOAD='replaced' WHERE ID=1", support::full(source, &src))).await?;
    opts.mode = "data".into();
    opts.write_mode = "replace".into();
    let result = synchronize(source, target, &mut opts).await;
    let readback = support::read(target, &dst).await?;
    let copied = result.map_err(|e| anyhow::anyhow!("advertised CH replace requires valid lightweight DELETE WHERE: {e:#}; target_readback={readback:?}"))?;
    ensure!(copied.rows == 2 && readback == support::read(source, &src).await?, "CH replace exact readback mismatch");
    Ok(json!({"sync": copied, "target_rows": readback}))
}

async fn no_key_source_incremental(source: &Endpoint, target: &Endpoint, nullable: bool) -> Result<Value> {
    let src = support::table();
    let dst = support::table();
    support::seed(source, &src, nullable).await?;
    let mut opts = support::options(source, &src, target, &dst);
    synchronize(source, target, &mut opts).await?;
    let before = support::read(target, &dst).await?;
    let compared = sync::diff(source.db.as_ref(), &source.kind, target.db.as_ref(), &target.kind, &support::diff_request(&opts)).await?;
    ensure!(!compared.keyed, "fixture route unexpectedly provided source primary-key metadata");
    opts.mode = "data".into();
    opts.sync_mode = "incremental".into();
    opts.create_target = false;
    let result = synchronize(source, target, &mut opts).await;
    let after = support::read(target, &dst).await?;
    ensure!(result.is_err() && after == before, "missing source key metadata must reject non-empty incremental before writes: result={result:?}, rows={after:?}");
    Ok(json!({"source_metadata_keyed": compared.keyed, "diff": compared, "incremental_status": "EXPLICIT_REJECTION_NO_TARGET_CHANGE", "error": format!("{:#}", result.unwrap_err()), "target_rows": after}))
}

async fn finish(report: &mut Report, world: &mut World, outcome: std::result::Result<Result<()>, Box<dyn std::any::Any + Send>>) -> Result<()> {
    match outcome {Ok(Ok(())) => {}, Ok(Err(error)) => report.fatal("setup/orchestration", format!("{error:#}")), Err(payload) => report.fatal("setup/orchestration", support::panic_text(payload))}
    let (cleanup, wire) = world.cleanup().await;
    report.finish(cleanup, wire)
}

#[tokio::test]
#[ignore = "Explicit loopback stateful TDS simulation; never a vendor SQL Server"]
async fn sync_simulated_tds() -> Result<()> {
    let mut report = Report::new("tds", "sqlserver")?;
    let mut world = World::new()?;
    report.fixture(&world.root)?;
    let outcome = std::panic::AssertUnwindSafe(async {
        let request = world.start_server("tds").await?;
        let tds = world.connect(request, "dbo").await?;
        let sqlite = world.sqlite().await?;
        report.case("sqlite_to_tds_full_diff_incremental", keyed_modes(&sqlite, &tds)).await?;
        report.case("tds_to_sqlite_full_diff_incremental", keyed_modes(&tds, &sqlite)).await?;
        report.case("tds_to_tds_full_diff_incremental", keyed_modes(&tds, &tds)).await?;
        report.case("tds_nontransactional_failure_after_first_write", generic_partial_failure(&sqlite, &tds)).await?;
        report.case("sqlite_to_tds_lossless_binary", tds_binary(&sqlite, &tds)).await?;
        report.case("tds_to_sqlite_lossless_binary", tds_binary(&tds, &sqlite)).await?;
        report.case("tds_cancel_middle_inflight_write", tds_cancellation(&world, &sqlite, &tds, false)).await?;
        report.case("tds_cancel_last_inflight_write_must_not_complete", tds_cancellation(&world, &sqlite, &tds, true)).await?;
        Ok::<_, anyhow::Error>(())
    }).catch_unwind().await;
    finish(&mut report, &mut world, outcome).await
}

#[tokio::test]
#[ignore = "Explicit actual Windows ODBC manager + existing test-only facade + DuckDB; no vendor"]
async fn sync_simulated_odbc() -> Result<()> {
    let kind = std::env::var("NAVKIT_SYNC_SIM_ODBC_KIND").unwrap_or_else(|_| "odbc".into());
    ensure!(!kind.is_empty() && kind.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'), "safe report kind required");
    let mut report = Report::new("odbc", &kind)?;
    let mut world = World::new()?;
    report.fixture(&world.root)?;
    let outcome = std::panic::AssertUnwindSafe(async {
        let odbc = world.odbc(&kind).await?;
        let odbc_other = world.odbc(&kind).await?;
        let sqlite = world.sqlite().await?;
        report.doc["limitations"].as_array_mut().unwrap().push(json!("Existing proxy translates a finite family-dialect subset to actual DuckDB; not Oracle/Dameng/Db2 vendor syntax/type certification"));
        report.case("sqlite_to_odbc_full_diff_incremental", keyed_modes(&sqlite, &odbc)).await?;
        if matches!(kind.as_str(), "oracle" | "dameng") {
            report.case("odbc_to_sqlite_full_diff_incremental", keyed_modes(&odbc, &sqlite)).await?;
            report.case("odbc_to_odbc_full_diff_incremental", keyed_modes(&odbc, &odbc_other)).await?;
        } else {
            report.case("odbc_to_sqlite_full_diff", full_modes(&odbc, &sqlite, true)).await?;
            report.case("odbc_to_odbc_full_diff", full_modes(&odbc, &odbc_other, true)).await?;
            report.case("odbc_missing_pk_metadata_incremental_safely_rejected", no_key_source_incremental(&odbc, &sqlite, true)).await?;
        }
        report.case("odbc_nontransactional_failure_after_first_write", generic_partial_failure(&sqlite, &odbc)).await?;
        report.case("sqlite_to_odbc_binary_4values_exact_bytes_null_empty", odbc_binary_four_values(&sqlite, &odbc)).await?;
        report.case("odbc_to_sqlite_binary_4values_exact_bytes_null_empty", odbc_binary_four_values(&odbc, &sqlite)).await?;
        Ok::<_, anyhow::Error>(())
    }).catch_unwind().await;
    finish(&mut report, &mut world, outcome).await
}

#[tokio::test]
#[ignore = "Explicit loopback mutable ClickHouse HTTP SQL subset; no vendor engine"]
async fn sync_simulated_clickhouse() -> Result<()> {
    let mut report = Report::new("clickhouse", "http_sql_subset")?;
    let mut world = World::new()?;
    report.fixture(&world.root)?;
    let outcome = std::panic::AssertUnwindSafe(async {
        let request = world.start_server("clickhouse").await?;
        let namespace = request.database.clone();
        let ch = world.connect(request, &namespace).await?;
        let sqlite = world.sqlite().await?;
        report.doc["limitations"].as_array_mut().unwrap().push(json!("SQLite storage does not certify ClickHouse engine/ordering/async-mutation/decimal precision/backslash-literal semantics; non-NULL baseline separated from Nullable regression"));
        report.case("sqlite_to_clickhouse_full_diff_nonnull", full_modes(&sqlite, &ch, false)).await?;
        report.case("clickhouse_to_sqlite_full_diff_nonnull", full_modes(&ch, &sqlite, false)).await?;
        report.case("clickhouse_to_clickhouse_full_diff_nonnull", full_modes(&ch, &ch, false)).await?;
        report.case("sqlite_to_clickhouse_nullable_lossless", full_modes(&sqlite, &ch, true)).await?;
        report.case("clickhouse_to_sqlite_nullable_lossless", full_modes(&ch, &sqlite, true)).await?;
        report.case("clickhouse_incremental_insert_and_mixed_batch_prewrite_rejection", ch_incremental(&sqlite, &ch)).await?;
        report.case("clickhouse_missing_pk_metadata_incremental_safely_rejected", no_key_source_incremental(&ch, &sqlite, false)).await?;
        report.case("clickhouse_replace_requires_valid_delete_where", ch_replace(&sqlite, &ch)).await?;
        Ok::<_, anyhow::Error>(())
    }).catch_unwind().await;
    finish(&mut report, &mut world, outcome).await
}
