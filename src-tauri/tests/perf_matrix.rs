//! 真实 MySQL/PostgreSQL 性能基准：直接调用产品入口（运行SQL文件/备份/还原/同步）。
//! 仅显式 opt-in：需要 NAVKIT_PERF_MYSQL_URL / NAVKIT_PERF_PG_URL。测试自建隔离库并清理。
//! 运行：cargo test --test perf_matrix --release -- --ignored --nocapture
use anyhow::{Context, Result, ensure};
use navkit_lib::{
    db::{self, Db},
    models::{ConnectionRequest, DbKind},
    sync::{self, SyncOptions},
    transfer,
};
use std::time::Instant;

const TICK: char = '\u{60}';

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

#[derive(Clone, Copy, Debug)]
enum Engine { Mysql, Postgres }

impl Engine {
    fn id(self) -> &'static str { match self { Engine::Mysql => "mysql", Engine::Postgres => "postgresql" } }
}

fn request(engine: Engine, url: &str, database: &str) -> Result<ConnectionRequest> {
    let rest = url.splitn(2, "://").nth(1).context("URL 缺少 scheme")?;
    let (authority, _) = rest.split_once('/').unwrap_or((rest, ""));
    let (credential, host_port) = authority.rsplit_once('@').unwrap_or(("", authority));
    let (user, password) = credential.split_once(':').unwrap_or((credential, ""));
    let (host, port) = host_port.rsplit_once(':').unwrap_or((host_port, ""));
    Ok(ConnectionRequest {
        kind: DbKind::of(engine.id()),
        host: if host.is_empty() { "127.0.0.1".into() } else { host.into() },
        port: port.parse().context("端口无效")?,
        user: user.into(),
        password: password.into(),
        database: database.into(),
        file: String::new(),
        params: String::new(),
        timeout_secs: 30,
    })
}

fn opts(namespace: &str, source: &str, target: &str, mode: &str, write: &str) -> SyncOptions {
    SyncOptions {
        task_id: None,
        source_connection: "source".into(), source_schema: namespace.into(), source_table: source.into(),
        target_connection: "target".into(), target_schema: namespace.into(), target_table: target.into(),
        mode: "data".into(), write_mode: write.into(), sync_mode: mode.into(),
        incremental_update: false, create_target: false, columns: vec![], key_columns: vec![],
        delete_missing: false, where_clause: None,
    }
}

fn emit(case: &str, engine: Engine, rows: u64, ms: &[f64], extra: serde_json::Value) {
    let mut sorted = ms.to_vec();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[sorted.len() / 2];
    println!("{}", serde_json::json!({
        "case": case, "engine": engine.id(), "rows": rows, "ms": ms,
        "median_ms": median, "min_ms": sorted[0],
        "rows_per_s": rows as f64 / (median / 1000.0), "extra": extra
    }));
}

async fn exec(db: &Db, sql: &str) -> Result<()> {
    match db {
        Db::Mysql(p) => { sqlx::raw_sql(sql).execute(p).await?; }
        Db::Postgres(p) => { sqlx::raw_sql(sql).execute(p).await?; }
        Db::Sqlite(p) => { sqlx::raw_sql(sql).execute(p).await?; }
        _ => anyhow::bail!("不支持的引擎"),
    }
    Ok(())
}

async fn seed(db: &Db, table: &str, rows: u64) -> Result<()> {
    let mut done = 0u64;
    while done < rows {
        let n = (rows - done).min(2000);
        let mut sql = format!("INSERT INTO {table}(id,v,amount,note) VALUES ");
        for i in 0..n {
            if i > 0 { sql.push(','); }
            let id = done + i + 1;
            sql.push_str(&format!("({id},'row-{id:08}-中文尾','{}.{:04}',NULL)", id, id % 10000));
        }
        exec(db, &sql).await?;
        done += n;
    }
    Ok(())
}

fn insert_script(table: &str, rows: u64, mysql: bool) -> String {
    let mut s = String::with_capacity(rows as usize * 80);
    if mysql { s.push_str("SET NAMES utf8mb4;\n"); }
    for id in 1..=rows {
        s.push_str(&format!("INSERT INTO {table}(id,v,amount,note) VALUES ({id},'row-{id:08}-中文尾','{}.{:04}',NULL);\n", id, id % 10000));
    }
    s
}

fn create_database(engine: Engine, namespace: &str) -> String {
    match engine {
        Engine::Mysql => format!("CREATE DATABASE {TICK}{namespace}{TICK} CHARACTER SET utf8mb4"),
        Engine::Postgres => format!("CREATE SCHEMA \"{namespace}\""),
    }
}

fn drop_database(engine: Engine, namespace: &str) -> String {
    match engine {
        Engine::Mysql => format!("DROP DATABASE IF EXISTS {TICK}{namespace}{TICK}"),
        Engine::Postgres => format!("DROP SCHEMA IF EXISTS \"{namespace}\" CASCADE"),
    }
}

async fn run_engine(engine: Engine, base: &str, rows: u64, rounds: usize, workdir: &std::path::Path) -> Result<()> {
    let admin_db = match engine { Engine::Mysql => "", Engine::Postgres => "postgres" };
    let admin = db::open(&request(engine, base, admin_db)?).await?;
    let token = uuid::Uuid::new_v4().simple().to_string();
    let namespace = format!("navkit_perf_{}", &token[..12]);
    ensure!(namespace.starts_with("navkit_perf_"), "隔离库名不合法");
    exec(&admin, &create_database(engine, &namespace)).await?;
    println!("fixture={namespace} engine={}", engine.id());

    let scratch = workdir.join(format!("{}-{}", engine.id(), &token[..8]));
    std::fs::create_dir_all(&scratch)?;
    let result: Result<()> = async {
        let connect_database = match engine { Engine::Mysql => namespace.clone(), Engine::Postgres => "postgres".to_string() };
        let db = db::open(&request(engine, base, &connect_database)?).await?;
        let kind = DbKind::of(engine.id());
        let decimal = if matches!(engine, Engine::Mysql) { "DECIMAL(20,4)" } else { "NUMERIC(20,4)" };
        // PostgreSQL 连接库是 postgres，目标 schema 不在 search_path 上，必须显式限定表名。
        let q = if matches!(engine, Engine::Postgres) { format!("\"{namespace}\".") } else { String::new() };
        for table in ["sync_src", "sync_dst", "file_dst"] {
            exec(&db, &format!("CREATE TABLE {q}{table}(id BIGINT PRIMARY KEY, v VARCHAR(200) NOT NULL, amount {decimal} NOT NULL, note TEXT)")).await?;
        }
        let seed_start = Instant::now();
        seed(&db, &format!("{q}sync_src"), rows).await?;
        println!("seed_ms={:.0} rows={rows}", seed_start.elapsed().as_secs_f64() * 1000.0);

        let mut samples = Vec::new();
        for round in 0..=rounds {
            exec(&db, &format!("DELETE FROM {q}sync_dst")).await?;
            let started = Instant::now();
            let report = sync::synchronize(&db, &kind, &db, &kind, &opts(&namespace, "sync_src", "sync_dst", "full", "append")).await?;
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            ensure!(report.rows == rows, "同步行数不符: {} != {rows}", report.rows);
            if round > 0 { samples.push(ms); }
        }
        emit("sync_full", engine, rows, &samples, serde_json::json!({"table":"sync_src -> sync_dst"}));

        let script = scratch.join("inserts.sql");
        std::fs::write(&script, insert_script(&format!("{q}file_dst"), rows, matches!(engine, Engine::Mysql)))?;
        let bytes = std::fs::metadata(&script)?.len();
        let mut samples = Vec::new();
        for round in 0..=rounds {
            exec(&db, &format!("TRUNCATE TABLE {q}file_dst")).await?;
            let started = Instant::now();
            let report = transfer::run_sql_file_detailed(&db, &script.to_string_lossy(), true).await?;
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            ensure!(report.failed == 0, "SQL 文件执行失败: {:?}", report.steps.iter().find(|s| !s.ok).map(|s| &s.error));
            ensure!(report.succeeded as u64 >= rows, "执行语句数不足: {} < {}", report.succeeded, rows);
            if round > 0 { samples.push(ms); }
        }
        emit("run_sql_file", engine, rows, &samples, serde_json::json!({"bytes":bytes}));

        let dump = scratch.join("backup.sql");
        let mut samples = Vec::new();
        let mut dump_bytes = 0u64;
        for round in 0..=rounds {
            let _ = std::fs::remove_file(&dump);
            let started = Instant::now();
            let (dumped, written) = transfer::dump_database(&db, &kind, &namespace, &["sync_dst".into()], &[], true, true, &dump.to_string_lossy(), "perf").await?;
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            ensure!(dumped == rows, "备份行数不符: {dumped}");
            dump_bytes = written;
            if round > 0 { samples.push(ms); }
        }
        emit("dump_backup", engine, rows, &samples, serde_json::json!({"dump_bytes":dump_bytes}));

        let mut samples = Vec::new();
        for round in 0..=rounds {
            exec(&db, &format!("DROP TABLE IF EXISTS {q}sync_dst")).await?;
            let started = Instant::now();
            let report = transfer::restore_sql_file_detailed(&db, &kind, &dump.to_string_lossy()).await?;
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            ensure!(report.failed == 0, "还原失败: {:?}", report.steps.iter().find(|s| !s.ok).map(|s| &s.error));
            if round > 0 { samples.push(ms); }
        }
        emit("restore_backup", engine, rows, &samples, serde_json::json!({"dump_bytes":dump_bytes}));

        // 5) 大表全量同步：验证键集分页在大表上不会因 OFFSET 深页退化。
        if let Ok(big) = std::env::var("NAVKIT_PERF_BIG_ROWS").unwrap_or_default().parse::<u64>() {
            if big > rows {
                for table in ["big_src", "big_dst"] {
                    exec(&db, &format!("CREATE TABLE {q}{table}(id BIGINT PRIMARY KEY, v VARCHAR(200) NOT NULL, amount {decimal} NOT NULL, note TEXT)")).await?;
                }
                let seed_started = Instant::now();
                seed(&db, &format!("{q}big_src"), big).await?;
                println!("big_seed_ms={:.0} rows={big}", seed_started.elapsed().as_secs_f64() * 1000.0);
                let mut samples = Vec::new();
                for round in 0..=rounds {
                    exec(&db, &format!("DELETE FROM {q}big_dst")).await?;
                    let started = Instant::now();
                    let report = sync::synchronize(&db, &kind, &db, &kind, &opts(&namespace, "big_src", "big_dst", "full", "append")).await?;
                    let ms = started.elapsed().as_secs_f64() * 1000.0;
                    ensure!(report.rows == big, "大表同步行数不符: {} != {big}", report.rows);
                    if round > 0 { samples.push(ms); }
                }
                emit("sync_full_bigtable", engine, big, &samples, serde_json::json!({"pages": (big + 1999) / 2000}));
            }
        }
        let _ = std::fs::remove_dir_all(&scratch);
        Ok(())
    }.await;

    let cleanup = exec(&admin, &drop_database(engine, &namespace)).await;
    cleanup.expect("必须清理本次唯一新建的隔离库");
    println!("fixture_cleaned={namespace}");
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "显式真实库性能基准：需要 NAVKIT_PERF_MYSQL_URL / NAVKIT_PERF_PG_URL"]
async fn real_database_performance_matrix() -> Result<()> {
    let rows: u64 = env("NAVKIT_PERF_ROWS").unwrap_or_else(|| "20000".into()).parse()?;
    let rounds: usize = env("NAVKIT_PERF_ROUNDS").unwrap_or_else(|| "3".into()).parse()?;
    ensure!(rows >= 1000 && rounds >= 1, "行数/轮次过小");
    let label = env("NAVKIT_PERF_LABEL").unwrap_or_default();
    let workdir = std::env::temp_dir().join(format!("navkit-perf-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&workdir)?;
    println!("label={label} rows={rows} rounds={rounds}");
    if let Some(url) = env("NAVKIT_PERF_MYSQL_URL") { run_engine(Engine::Mysql, &url, rows, rounds, &workdir).await?; }
    if let Some(url) = env("NAVKIT_PERF_PG_URL") { run_engine(Engine::Postgres, &url, rows, rounds, &workdir).await?; }
    let _ = std::fs::remove_dir_all(&workdir);
    Ok(())
}
