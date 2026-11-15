//! End-to-end JSON export A/B through the real cursor, sink, fsync and publication.
//! Timing assertions deliberately omitted: repeated observations decide rollout.
use super::*;
use anyhow::{ensure, Context};
use sha2::{Digest, Sha256};
use std::{io::Read, path::PathBuf, time::Instant};
#[path = "performance_rss_probe.rs"] mod memory;

struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let root = std::env::temp_dir().join(format!("navkit-json-ab-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir(&root)?; Ok(Self(root))
    }
}
impl Drop for Directory { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

fn digest(file: &Path) -> Result<Vec<u8>> {
    let mut input = std::fs::File::open(file)?; let mut hash = Sha256::new(); let mut bytes = [0u8; 65536];
    loop { let count = input.read(&mut bytes)?; if count == 0 { break; } hash.update(&bytes[..count]); }
    Ok(hash.finalize().to_vec())
}

async fn measure(db: &Db, kind: &DbKind, schema: &str, directory: &Directory, rows: u64, rounds: usize) -> Result<()> {
    let mut old = Vec::new(); let mut new = Vec::new(); let mut expected = None;
    for round in 0..=rounds {
        for legacy in if round % 2 == 0 { [true, false] } else { [false, true] } {
            let file = directory.0.join(format!("round-{round}-{}.json", if legacy { "old" } else { "new" }));
            let options = ExportOptions { connection_id: "json-ab-isolated".into(), schema: schema.into(), table: "result".into(), file: file.to_string_lossy().into(), format: "json".into(), delimiter: ",".into(), encoding: "utf8".into(), has_header: true, where_clause: None, limit: None };
            let rss = memory::Probe::start();
            let started = Instant::now();
            let count = transfer_json_row::comparison(legacy, export_table_streaming(db, kind, &options)).await?;
            let seconds = started.elapsed().as_secs_f64(); let rss = rss.finish();
            ensure!(count == rows, "export row count mismatch");
            let actual = digest(&file)?;
            if let Some(expected) = &expected { ensure!(&actual == expected, "JSON output bytes differ"); } else { expected = Some(actual); }
            let bytes = std::fs::metadata(&file)?.len();
            println!("JSON_EXPORT_AB engine={} round={round} legacy={legacy} rows={rows} bytes={bytes} seconds={seconds:.6} {rss}", kind.id());
            if round > 0 { if legacy { old.push(seconds); } else { new.push(seconds); } }
            std::fs::remove_file(file)?;
        }
    }
    old.sort_by(f64::total_cmp); new.sort_by(f64::total_cmp);
    println!("JSON_EXPORT_MEDIAN engine={} rows={rows} rounds={rounds} old_seconds={:.6} new_seconds={:.6} ratio={:.4} old_min={:.6} old_max={:.6} new_min={:.6} new_max={:.6}", kind.id(), old[old.len()/2], new[new.len()/2], old[old.len()/2]/new[new.len()/2], old[0], old[old.len()-1], new[0], new[new.len()-1]);
    Ok(())
}

fn config() -> Result<(u64, usize)> {
    let rows = std::env::var("NAVKIT_JSON_AB_ROWS").unwrap_or_else(|_| "100000".into()).parse()?;
    let rounds = std::env::var("NAVKIT_JSON_AB_ROUNDS").unwrap_or_else(|_| "5".into()).parse()?;
    ensure!((1000..=2_500_000).contains(&rows) && (3..=9).contains(&rounds), "invalid benchmark size/rounds");
    Ok((rows, rounds))
}

#[tokio::test]
#[ignore = "isolated full JSON export A/B benchmark; no external database"]
async fn sqlite_json_export_end_to_end_ab() -> Result<()> {
    let (rows, rounds) = config()?; let directory = Directory::new()?;
    let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(directory.0.join("source.sqlite")).create_if_missing(true)).await?;
    sqlx::query("CREATE TABLE result(z INTEGER PRIMARY KEY,payload TEXT,nullable TEXT,amount TEXT)").execute(&pool).await?;
    let mut tx = pool.begin().await?;
    for row in 0..rows { sqlx::query("INSERT INTO result VALUES(?,?,NULL,?)").bind(row as i64).bind(format!("中文,{row}\nquote'\"slash\\{}", "wide".repeat(16))).bind(format!("{row}.12345678901234567890")).execute(&mut *tx).await?; }
    tx.commit().await?;
    let result = measure(&Db::Sqlite(pool.clone()), &DbKind::of("sqlite"), "main", &directory, rows, rounds).await;
    pool.close().await; result
}

#[tokio::test]
#[ignore = "requires explicit disposable NAVKIT_PERF_MYSQL_URL; local MySQL only"]
async fn mysql_json_export_end_to_end_ab() -> Result<()> {
    let (rows, rounds) = config()?; let directory = Directory::new()?;
    let url = std::env::var("NAVKIT_PERF_MYSQL_URL").context("explicit local MySQL URL required")?;
    let options: sqlx::mysql::MySqlConnectOptions = url.parse()?;
    ensure!(matches!(options.get_host(), "127.0.0.1" | "localhost" | "::1"), "benchmark is local only");
    let admin = sqlx::mysql::MySqlPoolOptions::new().max_connections(1).connect_with(options.clone()).await?;
    let schema = format!("navkit_r4_json_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE `{schema}` CHARACTER SET utf8mb4")).execute(&admin).await?;
    let result: Result<()> = async {
        let pool = sqlx::mysql::MySqlPoolOptions::new().max_connections(1).after_connect(|conn, _| Box::pin(async move {sqlx::query("SET SESSION sql_mode=CONCAT(@@session.sql_mode,',NO_BACKSLASH_ESCAPES')").execute(conn).await?; Ok(())})).connect_with(options.database(&schema)).await?;
        let result: Result<()> = async {
            sqlx::query("CREATE TABLE result(z BIGINT PRIMARY KEY,payload TEXT,nullable TEXT,amount DECIMAL(40,20)) ENGINE=InnoDB").execute(&pool).await?;
            let mut tx = pool.begin().await?;
            for start in (0..rows).step_by(256) {
                let mut sql = String::from("INSERT INTO result VALUES ");
                for row in start..(start + 256).min(rows) {
                    if row != start { sql.push(','); }
                    let payload = crate::db::quote_literal(&format!("中文,{row}\nquote'\"slash\\{}", "wide".repeat(16)));
                    sql.push_str(&format!("({row},{payload},NULL,'{row}.12345678901234567890')"));
                }
                sqlx::query(&sql).execute(&mut *tx).await?;
            }
            tx.commit().await?;
            measure(&Db::Mysql(pool.clone()), &DbKind::of("mysql"), &schema, &directory, rows, rounds).await
        }.await;
        pool.close().await; result
    }.await;
    let cleanup = sqlx::query(&format!("DROP DATABASE `{schema}`")).execute(&admin).await;
    admin.close().await; cleanup?; result
}
