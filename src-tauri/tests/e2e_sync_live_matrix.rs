//! Real-server table synchronization acceptance. Explicit opt-in; no unavailable endpoint is a pass.
//! NAVKIT_MYSQL_PASSWORD is required, including an explicitly empty value for an empty-password fixture.
//! Run: cargo test --test e2e_sync_live_matrix -- --ignored --nocapture
use anyhow::{anyhow, ensure, Context, Result};
use navkit_lib::{
    db::{self, Db},
    models::{ConnectionRequest, DbKind},
    schema,
    sync::{self, DbSyncRequest, DbSyncTable, SyncColumnMapping, SyncDiffRequest, SyncOptions},
    transfer,
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::{Path, PathBuf}, str::FromStr, time::{Duration, Instant}};

struct Endpoint {
    db: Db,
    kind: DbKind,
    namespace: String,
    id: String,
    version: String,
    owned_namespace: bool,
}
impl Endpoint {
    fn full(&self, table: &str) -> String { db::qualified(&self.kind, &self.namespace, table) }
    async fn exec(&self, sql: &str) -> Result<()> {
        match &self.db {
            Db::Sqlite(p) => { sqlx::query(sql).execute(p).await?; }
            Db::Mysql(p) => { sqlx::query(sql).execute(p).await?; }
            Db::Postgres(p) => { sqlx::query(sql).execute(p).await?; }
            _ => unreachable!(),
        }
        Ok(())
    }
    async fn close(&self) {
        match &self.db { Db::Sqlite(p) => p.close().await, Db::Mysql(p) => p.close().await, Db::Postgres(p) => p.close().await, _ => {} }
    }
    async fn cleanup(&self) -> Result<()> {
        if !self.owned_namespace { return Ok(()); }
        ensure!(self.namespace.starts_with("navkit_sync_live_"), "refuse to remove unowned namespace");
        let quoted = db::quote_ident(&self.kind, &self.namespace);
        let sql = if self.kind.is_mysql_family() { format!("DROP DATABASE {quoted}") } else { format!("DROP SCHEMA {quoted} CASCADE") };
        self.exec(&sql).await?;
        let exists: i64 = match &self.db {
            Db::Mysql(p) => sqlx::query_scalar("SELECT COUNT(*) FROM information_schema.schemata WHERE schema_name=?").bind(&self.namespace).fetch_one(p).await?,
            Db::Postgres(p) => sqlx::query_scalar("SELECT COUNT(*) FROM information_schema.schemata WHERE schema_name=$1").bind(&self.namespace).fetch_one(p).await?,
            _ => 0,
        };
        ensure!(exists == 0, "owned namespace remains after cleanup");
        Ok(())
    }
}
fn env(name: &str, default: &str) -> String { std::env::var(name).unwrap_or_else(|_| default.into()) }
async fn provision(kind: &str, role: &str, root: &Path, token: &str) -> Result<Endpoint> {
    let mut req = ConnectionRequest { kind: DbKind::of(kind), host: String::new(), port: 0, user: String::new(), password: String::new(), database: String::new(), file: String::new(), params: String::new(), timeout_secs: 8 };
    match kind {
        "sqlite" => req.file = root.join(format!("{role}.sqlite3")).to_string_lossy().into_owned(),
        "mysql" => {
            req.host = env("NAVKIT_MYSQL_HOST", "127.0.0.1"); req.port = env("NAVKIT_MYSQL_PORT", "3306").parse()?;
            req.user = env("NAVKIT_MYSQL_USER", "root"); req.password = std::env::var("NAVKIT_MYSQL_PASSWORD").context("NAVKIT_MYSQL_PASSWORD is mandatory for live synchronization")?;
        }
        "postgresql" => {
            req.host = env("NAVKIT_PG_HOST", "127.0.0.1"); req.port = env("NAVKIT_PG_PORT", "5433").parse()?;
            req.user = env("NAVKIT_PG_USER", "postgres"); req.password = env("NAVKIT_PG_PASSWORD", ""); req.database = env("NAVKIT_PG_DATABASE", "postgres");
        }
        _ => unreachable!(),
    }
    // Use the production connection factory, including its per-session SQL settings.
    let db = db::open(&req).await?;
    let namespace = if kind == "sqlite" { "main".into() } else { format!("navkit_sync_live_{token}_{role}") };
    let mut endpoint = Endpoint { db, kind: req.kind, namespace, id: format!("{kind}-{role}"), version: String::new(), owned_namespace: false };
    let setup: Result<()> = async {
        endpoint.version = match &endpoint.db {
            Db::Sqlite(p) => sqlx::query_scalar::<_, String>("SELECT sqlite_version()").fetch_one(p).await?,
            Db::Mysql(p) => sqlx::query_scalar::<_, String>("SELECT VERSION()").fetch_one(p).await?,
            Db::Postgres(p) => sqlx::query_scalar::<_, String>("SHOW server_version").fetch_one(p).await?,
            _ => unreachable!(),
        };
        if kind != "sqlite" {
            let quoted = db::quote_ident(&endpoint.kind, &endpoint.namespace);
            let sql = if kind == "mysql" { format!("CREATE DATABASE {quoted} CHARACTER SET utf8mb4") } else { format!("CREATE SCHEMA {quoted}") };
            endpoint.exec(&sql).await?;
            endpoint.owned_namespace = true;
        }
        Ok(())
    }.await;
    if let Err(error) = setup { let _ = endpoint.cleanup().await; endpoint.close().await; return Err(error); }
    Ok(endpoint)
}
fn opts(source: &Endpoint, target: &Endpoint, table: &str, target_table: &str) -> SyncOptions {
    SyncOptions { task_id: Some(uuid::Uuid::new_v4().to_string()), source_connection: source.id.clone(), source_schema: source.namespace.clone(), source_table: table.into(), target_connection: target.id.clone(), target_schema: target.namespace.clone(), target_table: target_table.into(), mode: "both".into(), write_mode: "append".into(), sync_mode: "full".into(), incremental_update: false, create_target: true, columns: vec![], key_columns: Vec::new(), delete_missing: false, where_clause: None }
}
async fn run_sync(source: &Endpoint, target: &Endpoint, options: &mut SyncOptions) -> Result<sync::SyncReport> {
    options.task_id = Some(uuid::Uuid::new_v4().to_string());
    let report = sync::synchronize(&source.db, &source.kind, &target.db, &target.kind, options).await?;
    let progress = transfer::task_progress(options.task_id.as_deref().unwrap())?;
    ensure!(progress.status == "completed", "successful sync has incorrect task status: {}", progress.status);
    Ok(report)
}
async fn compare(source: &Endpoint, target: &Endpoint, options: &SyncOptions) -> Result<sync::SyncDiffReport> {
    sync::diff(&source.db, &source.kind, &target.db, &target.kind, &SyncDiffRequest { source_connection: source.id.clone(), source_schema: source.namespace.clone(), source_table: options.source_table.clone(), target_connection: target.id.clone(), target_schema: target.namespace.clone(), target_table: options.target_table.clone(), where_clause: options.where_clause.clone(), columns: options.columns.clone() }).await
}
fn hex(bytes: &[u8]) -> String { format!("0x{}", bytes.iter().map(|b| format!("{b:02X}")).collect::<String>()) }
fn expected() -> Vec<Vec<Option<String>>> {
    let rows = [
        ("1", Some("中文🙂;O'Reilly\\尾\r\nline\tend"), None, vec![0, 255, 65, 92, 39], "1", "12345.625", "2024-02-29 12:34:56.123456"),
        ("2", Some(""), Some(""), vec![], "0", "-0.125", "2024-01-01 00:00:00"),
        ("3", None, Some(" NULL "), vec![65, 66], "1", "0", "2025-12-31 23:59:59.999999"),
        ("4", Some("  padded  "), Some("\\N"), vec![0, 1, 2, 127, 128], "0", "42.5", "2024-06-15 08:00:00"),
    ];
    rows.into_iter().map(|(id, name, note, bytes, active, amount, stamp)| vec![Some(id.into()), name.map(String::from), note.map(String::from), Some(hex(&bytes)), Some(active.into()), Some(amount.into()), Some(stamp.into())]).collect()
}
async fn seed(endpoint: &Endpoint, table: &str) -> Result<()> {
    let blob = if endpoint.kind.is_pg_family() { "BYTEA" } else { "BLOB" };
    let timestamp = if endpoint.kind.is_mysql_family() { "DATETIME(6)" } else if endpoint.kind.is_sqlite() { "TEXT" } else { "TIMESTAMP(6)" };
    endpoint.exec(&format!("CREATE TABLE {} (id BIGINT NOT NULL PRIMARY KEY, name VARCHAR(240), note TEXT, payload {blob} NOT NULL, active BOOLEAN NOT NULL, amount DECIMAL(22,6) NOT NULL, stamp {timestamp} NOT NULL)", endpoint.full(table))).await?;
    // Bind setup values directly; the oracle does not use the sync SQL literal writer.
    for row in expected() {
        let id: i64 = row[0].as_ref().unwrap().parse()?;
        let raw = row[3].as_ref().unwrap().trim_start_matches("0x");
        let bytes = (0..raw.len()).step_by(2).map(|i| u8::from_str_radix(&raw[i..i+2],16)).collect::<std::result::Result<Vec<_>, _>>()?;
        let flag = row[4].as_deref() == Some("1");
        let decimal = sqlx::types::BigDecimal::from_str(row[5].as_deref().unwrap())?;
        let stamp = chrono::NaiveDateTime::parse_from_str(row[6].as_deref().unwrap(), "%Y-%m-%d %H:%M:%S%.f")?;
        let full = endpoint.full(table);
        match &endpoint.db {
            Db::Sqlite(p) => { sqlx::query(&format!("INSERT INTO {full} VALUES (?,?,?,?,?,?,?)")).bind(id).bind(&row[1]).bind(&row[2]).bind(bytes).bind(flag).bind(row[5].as_ref()).bind(row[6].as_ref()).execute(p).await?; }
            Db::Mysql(p) => { sqlx::query(&format!("INSERT INTO {full} VALUES (?,?,?,?,?,?,?)")).bind(id).bind(&row[1]).bind(&row[2]).bind(bytes).bind(flag).bind(decimal).bind(stamp).execute(p).await?; }
            Db::Postgres(p) => { sqlx::query(&format!("INSERT INTO {full} VALUES ($1,$2,$3,$4,$5,$6,$7)")).bind(id).bind(&row[1]).bind(&row[2]).bind(bytes).bind(flag).bind(decimal).bind(stamp).execute(p).await?; }
            _ => unreachable!(),
        }
    }
    ensure_rows(endpoint, table, &expected()).await.context("source fixture must independently match expected values")
}
fn canonical(mut rows: Vec<Vec<Option<String>>>) -> Result<Vec<Vec<Option<String>>>> {
    for row in &mut rows {
        ensure!(row.len() == 7, "expected seven mapped columns, got {}", row.len());
        if let Some(v) = &row[4] { row[4] = Some(match v.to_ascii_lowercase().as_str() { "true" | "t" | "1" => "1", "false" | "f" | "0" => "0", _ => return Err(anyhow!("boolean corrupted: {v}")) }.into()); }
        if let Some(v) = &row[5] { row[5] = Some(sqlx::types::BigDecimal::from_str(v)?.normalized().to_string()); }
        if let Some(v) = &row[6] {
            let dt = chrono::NaiveDateTime::parse_from_str(v, "%Y-%m-%d %H:%M:%S%.f").with_context(|| format!("invalid datetime readback: {v}"))?;
            row[6] = Some(dt.format("%Y-%m-%d %H:%M:%S%.f").to_string());
        }
    }
    rows.sort_by_key(|r| r[0].as_deref().unwrap_or("").parse::<i64>().unwrap_or(-1));
    Ok(rows)
}
async fn read_rows(endpoint: &Endpoint, table: &str) -> Result<Vec<Vec<Option<String>>>> {
    // SQL execution and result decoding are real. The independent oracle above checks every cell.
    let (names, rows) = transfer::fetch_transfer_rows(&endpoint.db, &endpoint.kind, &endpoint.namespace, table, None, None).await?;
    let fields = ["id", "name", "note", "payload", "active", "amount", "stamp"];
    let indexes = fields.iter().map(|field| names.iter().position(|name| name == field).ok_or_else(||anyhow!("missing column {field}"))).collect::<Result<Vec<_>>>()?;
    canonical(rows.into_iter().map(|row| indexes.iter().map(|&i| row[i].clone()).collect()).collect())
}
async fn ensure_rows(endpoint: &Endpoint, table: &str, expected: &[Vec<Option<String>>]) -> Result<()> {
    let actual = read_rows(endpoint, table).await?;
    let expected = canonical(expected.to_vec())?;
    ensure!(actual == expected, "{} exact row/cell readback mismatch: actual={actual:?}, expected={expected:?}", endpoint.kind.id());
    Ok(())
}
async fn scenario(source: &Endpoint, target: &Endpoint, sequence: usize, case: &str) -> Result<Value> {
    let table = format!("case_{sequence}_{}", source.kind.id());
    let target_table = format!("out_{sequence}_{}", source.kind.id());
    seed(source, &table).await?;
    let mut options = opts(source, target, &table, &target_table);
    let mut evidence = json!({"source_rows":4,"oracle":"every cell: NULL/empty/Unicode/whitespace/binary/boolean/decimal/microsecond timestamp"});
    match case {
        "full_create" => {
            let report = run_sync(source, target, &mut options).await?;
            ensure!(report.rows == 4 && report.created, "initial full sync report mismatch");
            ensure_rows(target, &target_table, &expected()).await?;
            let meta = schema::table_schema(&target.db, target.kind.clone(), &target.namespace, &target_table).await?;
            ensure!(meta.columns.len() == 7 && meta.columns.iter().any(|c| c.name == "id" && c.primary_key), "created metadata lost columns or primary key");
            let diff = compare(source, target, &options).await?;
            ensure!(diff.missing_in_target == 0 && diff.extra_in_target == 0 && diff.changed == 0, "diff after full sync is not empty: {diff:?}");
            evidence["warnings"] = json!(report.warnings);
        }
        "structure_then_data" => {
            options.mode = "structure".into();
            ensure!(run_sync(source, target, &mut options).await?.rows == 0, "structure-only wrote rows");
            ensure_rows(target, &target_table, &[]).await?;
            options.mode = "data".into();
            ensure!(run_sync(source, target, &mut options).await?.rows == 4, "data-only count mismatch");
            ensure_rows(target, &target_table, &expected()).await?;
        }
        "replace" | "recreate" => {
            run_sync(source, target, &mut options).await?;
            target.exec(&format!("UPDATE {} SET name='stale'", target.full(&target_table))).await?;
            target.exec(&format!("INSERT INTO {} SELECT 99,name,note,payload,active,amount,stamp FROM {} WHERE id=1", target.full(&target_table), target.full(&target_table))).await?;
            options.write_mode = case.into();
            let report = run_sync(source, target, &mut options).await?;
            ensure!(report.rows == 4 && report.created == (case == "recreate"), "replace/recreate report mismatch");
            ensure_rows(target, &target_table, &expected()).await?;
        }
        "incremental_insert" | "incremental_update" => {
            run_sync(source, target, &mut options).await?;
            target.exec(&format!("DELETE FROM {} WHERE id>=3", target.full(&target_table))).await?;
            target.exec(&format!("UPDATE {} SET name='stale' WHERE id=1", target.full(&target_table))).await?;
            target.exec(&format!("INSERT INTO {} SELECT 99,name,note,payload,active,amount,stamp FROM {} WHERE id=2", target.full(&target_table), target.full(&target_table))).await?;
            let diff = compare(source, target, &options).await?;
            ensure!((diff.missing_in_target,diff.extra_in_target,diff.changed) == (2,1,1), "pre-incremental diff mismatch: {diff:?}");
            options.sync_mode = "incremental".into(); options.incremental_update = case == "incremental_update";
            let wanted_count = if options.incremental_update { 3 } else { 2 };
            ensure!(run_sync(source,target,&mut options).await?.rows == wanted_count, "incremental write count mismatch");
            let mut wanted = expected(); if !options.incremental_update { wanted[0][1] = Some("stale".into()); }
            let mut extra = wanted[1].clone(); extra[0] = Some("99".into()); wanted.push(extra);
            ensure_rows(target,&target_table,&wanted).await?;
            ensure!(run_sync(source,target,&mut options).await?.rows == 0, "repeating incremental sync was not idempotent");
            evidence["inserted"] = json!(2); evidence["updated"] = json!(u8::from(options.incremental_update)); evidence["extra_target_row_preserved"] = json!(true);
        }
        "exact_precision_and_whitespace_incremental" => {
            let special=format!("precision_{sequence}");let out=format!("precision_out_{sequence}");
            let decimal=if source.kind.is_sqlite(){"TEXT"}else{"DECIMAL(38,20)"};
            source.exec(&format!("CREATE TABLE {}(id BIGINT PRIMARY KEY,v_int BIGINT,v_decimal {decimal},v_text VARCHAR(240))",source.full(&special))).await?;
            let decimal=if target.kind.is_sqlite(){"TEXT"}else{"DECIMAL(38,20)"};
            target.exec(&format!("CREATE TABLE {}(id BIGINT PRIMARY KEY,v_int BIGINT,v_decimal {decimal},v_text VARCHAR(240))",target.full(&out))).await?;
            source.exec(&format!("INSERT INTO {} VALUES (1,9007199254740993,'1.00000000000000000001','same'),(2,1,'123456789012345678.12345678901234567890','same'),(3,1,'1',' padded ')",source.full(&special))).await?;
            target.exec(&format!("INSERT INTO {} VALUES (1,9007199254740992,'1.00000000000000000001','same'),(2,1,'123456789012345678.12345678901234567891','same'),(3,1,'1','padded')",target.full(&out))).await?;
            options=opts(source,target,&special,&out);options.mode="data".into();options.sync_mode="incremental".into();options.incremental_update=true;
            let before=compare(source,target,&options).await?;ensure!(before.changed==3,"precise integer/decimal/whitespace diff must find 3 changed rows: {before:?}");
            ensure!(run_sync(source,target,&mut options).await?.rows==3,"precise incremental comparison did not update all 3 rows");
            let (_,mut actual)=transfer::fetch_transfer_rows(&target.db,&target.kind,&target.namespace,&out,None,None).await?;actual.sort_by(|a,b|a[0].cmp(&b[0]));
            let wanted=[(1_i64,9007199254740993_i64,"1.00000000000000000001","same"),(2,1,"123456789012345678.12345678901234567890","same"),(3,1,"1"," padded ")];
            ensure!(actual.len()==3,"precision target row count mismatch");
            for (row,(id,integer,decimal,text)) in actual.iter().zip(wanted) {
                ensure!(row[0].as_deref().unwrap().parse::<i64>()?==id&&row[1].as_deref().unwrap().parse::<i64>()?==integer&&sqlx::types::BigDecimal::from_str(row[2].as_deref().unwrap())?==sqlx::types::BigDecimal::from_str(decimal)?&&row[3].as_deref()==Some(text),"precise typed readback mismatch: {row:?}");
            }
            let before_repeat=compare(source,target,&options).await?;
            let (_,source_raw)=transfer::fetch_transfer_rows(&source.db,&source.kind,&source.namespace,&special,None,None).await?;
            let target_meta=schema::table_schema(&target.db,target.kind.clone(),&target.namespace,&out).await?;
            let repeated=run_sync(source,target,&mut options).await?;
            ensure!(repeated.rows==0,"repeating precise heterogeneous incremental comparison must be idempotent: repeat={repeated:?}, diff={before_repeat:?}, source={source_raw:?}, target={actual:?}, target_schema={target_meta:?}");
            evidence=json!({"source_rows":3,"changed_rows":3,"updated_rows":3,"repeat_writes":0,"oracle":"independent i64 and BigDecimal equality plus exact whitespace-bearing TEXT"});
        }
        "composite_unicode_key_incremental" => {
            let special=format!("composite_{sequence}");let out=format!("composite_out_{sequence}");
            source.exec(&format!("CREATE TABLE {}(tenant BIGINT NOT NULL,region VARCHAR(240) NOT NULL,value VARCHAR(240),PRIMARY KEY(tenant,region))",source.full(&special))).await?;
            let plain="中文🙂;O'Reilly\\尾";let padded=format!(" {plain} ");
            source.exec(&format!("INSERT INTO {} VALUES(1,{},'new-first'),(1,{},'new-second')",source.full(&special),db::quote_literal(plain),db::quote_literal(&padded))).await?;
            options=opts(source,target,&special,&out);options.columns=vec![SyncColumnMapping{source:"tenant".into(),target:"tenant_id".into(),target_type:None},SyncColumnMapping{source:"region".into(),target:"region_key".into(),target_type:Some("VARCHAR(240)".into())},SyncColumnMapping{source:"value".into(),target:"mapped_value".into(),target_type:Some("VARCHAR(240)".into())}];
            run_sync(source,target,&mut options).await?;
            target.exec(&format!("DELETE FROM {} WHERE {}={}",target.full(&out),db::quote_ident(&target.kind,"region_key"),db::quote_literal(&padded))).await?;
            target.exec(&format!("UPDATE {} SET {}='old-first'",target.full(&out),db::quote_ident(&target.kind,"mapped_value"))).await?;
            let before=compare(source,target,&options).await?;ensure!(before.missing_in_target==1&&before.changed==1,"composite Unicode keys collided or failed diff: {before:?}");
            options.sync_mode="incremental".into();options.incremental_update=true;ensure!(run_sync(source,target,&mut options).await?.rows==2,"composite mapped key insert/update count mismatch");
            let (_,actual)=transfer::fetch_transfer_rows(&target.db,&target.kind,&target.namespace,&out,None,None).await?;
            ensure!(actual.len()==2&&actual.iter().any(|r|r[1].as_deref()==Some(plain)&&r[2].as_deref()==Some("new-first"))&&actual.iter().any(|r|r[1].as_deref()==Some(padded.as_str())&&r[2].as_deref()==Some("new-second")),"composite mapped key exact target mismatch: {actual:?}");
            evidence=json!({"source_rows":2,"inserted_rows":1,"updated_rows":1,"oracle":"complete mapped composite primary key with Unicode, quote, backslash and leading/trailing whitespace"});
        }
        "filter_and_mapping" => {
            options.where_clause = Some("id >= 2 AND id <= 3".into());
            options.columns = vec![SyncColumnMapping { source:"id".into(),target:"mapped_id".into(),target_type:Some("BIGINT".into()) }, SyncColumnMapping { source:"name".into(),target:"mapped_name".into(),target_type:Some("VARCHAR(240)".into()) }];
            ensure!(run_sync(source,target,&mut options).await?.rows == 2,"filtered mapping count mismatch");
            let (names,mut rows) = transfer::fetch_transfer_rows(&target.db,&target.kind,&target.namespace,&target_table,None,None).await?;
            rows.sort_by(|a,b|a[0].cmp(&b[0]));
            ensure!(names == ["mapped_id","mapped_name"] && rows == vec![vec![Some("2".into()),Some("".into())],vec![Some("3".into()),None]],"filter/mapping exact readback mismatch: {names:?} {rows:?}");
        }
        "constraint_failure_rollback" => {
            // Existing CHECK makes the third source row fail after DELETE and two INSERTs.
            let blob = if target.kind.is_pg_family(){"BYTEA"}else{"BLOB"};
            let stamp = if target.kind.is_mysql_family(){"DATETIME(6)"}else if target.kind.is_sqlite(){"TEXT"}else{"TIMESTAMP(6)"};
            target.exec(&format!("CREATE TABLE {} (id BIGINT PRIMARY KEY CHECK(id<>3),name VARCHAR(240),note TEXT,payload {blob} NOT NULL,active BOOLEAN NOT NULL,amount DECIMAL(22,6),stamp {stamp})",target.full(&target_table))).await?;
            options.mode="data".into(); options.where_clause=Some("id=1".into()); run_sync(source,target,&mut options).await?;
            let before=read_rows(target,&target_table).await?;
            options.where_clause=None;options.write_mode="replace".into(); options.task_id=Some(uuid::Uuid::new_v4().to_string());
            let error=sync::synchronize(&source.db,&source.kind,&target.db,&target.kind,&options).await.err().ok_or_else(||anyhow!("CHECK violation incorrectly succeeded"))?;
            ensure!(format!("{error:#}").contains("回滚"),"transaction error lost rollback context: {error:#}");
            ensure!(read_rows(target,&target_table).await? == before,"failed replace changed committed target data");
            ensure!(transfer::task_progress(options.task_id.as_deref().unwrap())?.status=="failed","failed sync task status mismatch");
            evidence["expected_error"]=json!(format!("{error:#}"));evidence["rollback_verified"]=json!(true);
        }
        "whole_database_partial_failure_and_view" => {
            let view=format!("view_{sequence}_{}",source.kind.id());
            source.exec(&format!("CREATE VIEW {} AS SELECT id,name,note,payload,active,amount,stamp FROM {} WHERE id<=2",source.full(&view),source.full(&table))).await?;
            let request=DbSyncRequest {task_id:Some(uuid::Uuid::new_v4().to_string()),source_connection:source.id.clone(),source_schema:source.namespace.clone(),target_connection:target.id.clone(),target_schema:target.namespace.clone(),mode:"both".into(),write_mode:"append".into(),sync_mode:"full".into(),incremental_update:false,tables:vec![DbSyncTable{source:format!("missing_{sequence}"),target:format!("missing_out_{sequence}"),view:false},DbSyncTable{source:table.clone(),target:target_table.clone(),view:false},DbSyncTable{source:view,target:format!("view_out_{sequence}_{}",source.kind.id()),view:true}],where_clause:None};
            let reports=sync::synchronize_database(&source.db,&source.kind,&target.db,&target.kind,&request).await?;
            ensure!(reports.len()==3&&reports[0].error.is_some()&&reports[1].error.is_none()&&reports[2].error.is_none(),"whole database did not isolate table errors: {reports:?}");
            ensure_rows(target,&target_table,&expected()).await?;ensure_rows(target,&request.tables[2].target,&expected()[..2]).await?;
            ensure!(reports[2].warnings.iter().any(|s|s.contains("物化")),"view materialization warning missing");
            evidence["per_table_reports"]=serde_json::to_value(reports)?;
        }
        "invalid_options_preserve_target" => {
            run_sync(source,target,&mut options).await?;
            options.mode="data".into();options.write_mode="replace".into();options.where_clause=Some("1=1; DROP TABLE t".into());
            ensure!(sync::synchronize(&source.db,&source.kind,&target.db,&target.kind,&options).await.is_err(),"unsafe WHERE accepted");
            options.where_clause=None;options.sync_mode="incremental".into();
            ensure!(sync::synchronize(&source.db,&source.kind,&target.db,&target.kind,&options).await.is_err(),"incremental replace accepted");
            ensure_rows(target,&target_table,&expected()).await?;
        }
        _ => unreachable!(),
    }
    ensure_rows(source,&table,&expected()).await.context("synchronization modified source data")?;
    Ok(evidence)
}
fn executed_binary() -> Result<Value> {
    use sha2::Digest;
    let path=std::env::current_exe()?;
    let hash=format!("{:x}",sha2::Sha256::digest(std::fs::read(&path)?));
    Ok(json!({"path":path,"sha256":hash}))
}
fn source_hashes() -> Result<Value> {
    use sha2::Digest;
    let root=Path::new(env!("CARGO_MANIFEST_DIR"));let mut hashes=serde_json::Map::new();
    for path in ["src/sync.rs","src/transfer.rs","src/mssql.rs","src/odbc_db.rs","src/schema.rs","src/db.rs","tests/e2e_sync_live_matrix.rs"] {
        hashes.insert(format!("src-tauri/{path}"),json!(format!("{:x}",sha2::Sha256::digest(std::fs::read(root.join(path))?))));
    }
    Ok(Value::Object(hashes))
}
fn remove_owned(root: &Path) -> Result<()> {
    let path=root.canonicalize()?;
    ensure!(path.file_name().and_then(|s|s.to_str()).is_some_and(|s|s.starts_with("navkit-sync-live-")),"refuse unowned directory removal");
    let deadline=Instant::now()+Duration::from_secs(3);
    loop {
        match std::fs::remove_dir_all(&path) {
            Ok(())=>return Ok(()),
            Err(error) if (error.kind()==std::io::ErrorKind::PermissionDenied || matches!(error.raw_os_error(),Some(32|33))) && Instant::now()<deadline => std::thread::sleep(Duration::from_millis(25)),
            Err(error)=>return Err(error.into()),
        }
    }
}
#[tokio::test]
#[ignore="requires explicit live MySQL/PostgreSQL endpoints; run with --ignored; no silent skips"]
async fn real_sqlite_mysql_postgres_sync_matrix() {
    let initial_hashes=source_hashes().unwrap();
    let token=uuid::Uuid::new_v4().simple().to_string();
    let root=std::env::temp_dir().join(format!("navkit-sync-live-{token}"));std::fs::create_dir(&root).unwrap();
    let kinds=["sqlite","mysql","postgresql"];
    let cases=["full_create","structure_then_data","replace","recreate","incremental_insert","incremental_update","exact_precision_and_whitespace_incremental","composite_unicode_key_incremental","filter_and_mapping","constraint_failure_rollback","whole_database_partial_failure_and_view","invalid_options_preserve_target"];
    let mut endpoints=BTreeMap::new();let mut setup=vec![];let mut rows=vec![];
    for kind in kinds { for role in ["source","target"] {
        match provision(kind,role,&root,&token).await {
            Ok(endpoint)=>{setup.push(json!({"kind":kind,"role":role,"status":"LIVE_CONNECTED","version":endpoint.version}));endpoints.insert(format!("{kind}-{role}"),endpoint);}
            Err(error)=>setup.push(json!({"kind":kind,"role":role,"status":"FAIL","error":format!("{error:#}")})),
        }
    }}
    let mut sequence=0;
    for source_kind in kinds { for target_kind in kinds { for case in cases {
        sequence+=1;let started=Instant::now();
        let mut row=json!({"source":source_kind,"target":target_kind,"case":case,"evidence_type":"LIVE_DATABASE_SYNC","homogeneous":source_kind==target_kind});
        if let (Some(source),Some(target))=(endpoints.get(&format!("{source_kind}-source")),endpoints.get(&format!("{target_kind}-target"))) {
            // Keep cleanup outside the scenario deadline; an error never skips later directions.
            let result=tokio::time::timeout(Duration::from_secs(45),scenario(source,target,sequence,case)).await.unwrap_or_else(|_|Err(anyhow!("live synchronization scenario exceeded 45 seconds")));
            match result {Ok(details)=>{row["status"]=json!("PASS");row["details"]=details;},Err(error)=>{row["status"]=json!("FAIL");row["error"]=json!(format!("{error:#}"));}}
        } else {row["status"]=json!("NOT_RUN");row["error"]=json!("required live endpoint failed setup; not counted as pass");}
        row["elapsed_ms"]=json!(started.elapsed().as_millis());println!("{source_kind} -> {target_kind} {case}: {} {}",row["status"],row.get("error").unwrap_or(&Value::Null));rows.push(row);
    }}}
    let mut cleanup=vec![];
    for endpoint in endpoints.values() {
        let result=endpoint.cleanup().await;cleanup.push(json!({"endpoint":endpoint.id,"success":result.is_ok(),"error":result.err().map(|e|format!("{e:#}"))}));endpoint.close().await;
    }
    let removed=remove_owned(&root);cleanup.push(json!({"endpoint":"owned_sqlite_directory","success":removed.is_ok(),"error":removed.err().map(|e|format!("{e:#}"))}));
    let passed=rows.iter().filter(|r|r["status"]=="PASS").count();let setup_failed=setup.iter().any(|r|r["status"]=="FAIL");let cleanup_failed=cleanup.iter().any(|r|r["success"]!=true);
    let report=json!({"schema_version":1,"suite":"real-sqlite-mysql-postgres-synchronization","run_id":token,"timestamp_utc":chrono::Utc::now().to_rfc3339(),"executed_binary":executed_binary().unwrap(),"source_hashes":initial_hashes,"source_unchanged_during_run":source_hashes().unwrap()==initial_hashes,"counts":{"directions":9,"homogeneous_directions":3,"heterogeneous_directions":6,"cases":rows.len(),"pass":passed,"fail_or_not_run":rows.len()-passed},"setup":setup,"rows":rows,"cleanup":cleanup,"limitations":["Homogeneous MySQL/PostgreSQL use separate UUID namespaces and separate production pools on the same local server, not separate machines.","Incremental synchronization means key comparison; it is not binlog/WAL CDC and does not delete target-only rows.","All data checks call the actual Rust synchronization engine; this suite does not assert GUI click paths."]});
    let output=std::env::var("NAVKIT_SYNC_LIVE_REPORT").map(PathBuf::from).unwrap_or_else(|_|Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/acceptance/sync/live-report.json"));
    if let Some(parent)=output.parent(){std::fs::create_dir_all(parent).unwrap();}std::fs::write(&output,serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    assert!(passed==kinds.len()*kinds.len()*cases.len()&&!setup_failed&&!cleanup_failed&&source_hashes().unwrap()==initial_hashes,"live synchronization matrix failed; see {}",output.display());
}
