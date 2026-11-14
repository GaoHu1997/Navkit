//! 性能基准：量化 navkit 原生 SQLite 查询热路径各阶段耗时。
//!
//! 只使用本地临时库（`Fixture`），不连接任何用户数据库。
//! 运行：`cargo test --lib perf_sqlite_phase_breakdown -- --nocapture`
//! 可用环境变量 `NAVKIT_BENCH_ROWS` 调整行数（默认 200000）。
use super::*;
use futures::TryStreamExt;
use std::time::Instant;

const COLS: usize = 8;
const SQL: &str = "SELECT id, name, amount, flag, tag, score, note, grp FROM big";

async fn seed(f: &Fixture, n: usize) {
    let sql = format!(
        "CREATE TABLE big(id INTEGER PRIMARY KEY, name TEXT, amount REAL, flag INTEGER, tag TEXT, score REAL, note TEXT, grp INTEGER);\n\
         WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<{n})\n\
         INSERT INTO big SELECT x, 'name-'||x, x*1.5, x%2, 'tag'||(x%100), x*0.25, 'note;with,comma '||x, x%10 FROM n"
    );
    sqlx::query(&sql).execute(&f.observer).await.unwrap();
}

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[tokio::test]
async fn perf_sqlite_phase_breakdown() {
    let f = Fixture::new(false).await;
    let n: usize = std::env::var("NAVKIT_BENCH_ROWS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200_000);
    seed(&f, n).await;

    // 0) prepare/describe 往返（每条语句一次）
    let describe_ms;
    let described_cols;
    {
        let mut c = f.observer.acquire().await.unwrap();
        let t = Instant::now();
        let d = sqlx::Executor::describe(&mut *c, SQL).await.unwrap();
        describe_ms = ms(t.elapsed());
        described_cols = d.columns().len();
    }

    // 1) 纯驱动 fetch：只取行、不转换
    let fetch_ms;
    let fetched;
    {
        let mut c = f.observer.acquire().await.unwrap();
        let t = Instant::now();
        let mut stream = sqlx::query(SQL).fetch(&mut *c);
        let mut count = 0usize;
        while let Some(_row) = stream.try_next().await.unwrap() {
            count += 1;
        }
        fetch_ms = ms(t.elapsed());
        fetched = count;
    }

    // 2) fetch + 逐行转换（当前 values.rs 的阶梯实现）
    let fetch_convert_ms;
    let converted;
    {
        let mut c = f.observer.acquire().await.unwrap();
        let t = Instant::now();
        let mut stream = sqlx::query(SQL).fetch(&mut *c);
        let mut count = 0usize;
        while let Some(row) = stream.try_next().await.unwrap() {
            count += crate::values::sqlite_row_values(&row, COLS).len();
        }
        fetch_convert_ms = ms(t.elapsed());
        converted = count;
    }

    // 3) 已取到内存的行，只计转换成本（隔离 fetch）
    let fetch_all_ms;
    let convert_only_ms;
    let acc;
    {
        let mut c = f.observer.acquire().await.unwrap();
        let t = Instant::now();
        let all: Vec<sqlx::sqlite::SqliteRow> =
            sqlx::query(SQL).fetch_all(&mut *c).await.unwrap();
        fetch_all_ms = ms(t.elapsed());
        let t = Instant::now();
        let mut a = 0usize;
        for row in &all {
            a += crate::values::sqlite_row_values(row, COLS).len();
        }
        convert_only_ms = ms(t.elapsed());
        acc = a;
    }

    // 4) serde 逐行序列化（模拟 query_spool 每行 to_writer 的固定成本）
    let sample: Vec<Option<String>> = (0..COLS).map(|i| Some(format!("value-{i}-中文"))).collect();
    let t = Instant::now();
    let mut sink: Vec<u8> = Vec::with_capacity(256);
    for _ in 0..n {
        sink.clear();
        serde_json::to_writer(&mut sink, &sample).unwrap();
    }
    let serde_ms = ms(t.elapsed());
    std::hint::black_box(&sink);

    // 5) 生产完整路径（describe + fetch + 转换 + spool 逐行落盘 + 预览 + 服务编排）
    let t = Instant::now();
    let outcome = f.execute(SQL, 0).await;
    let full_ms = ms(t.elapsed());
    assert_eq!(outcome.state, ExecutionState::Succeeded, "{:?}", outcome.error);
    let total;
    {
        let result = outcome.result.as_ref().unwrap();
        total = result.sets[0].total;
        if let Some(id) = result.spool_id.clone() {
            crate::query_spool::drop_spool(&id);
        }
    }

    eprintln!("\n==== navkit SQLite 热路径基准 rows={n} cols={COLS} ====");
    eprintln!("describe(prepare)     : {describe_ms:>9.1} ms  (cols={described_cols})");
    eprintln!("fetch-only            : {fetch_ms:>9.1} ms  ({:.0} ns/row)", fetch_ms * 1e6 / fetched as f64);
    eprintln!(
        "fetch+convert         : {fetch_convert_ms:>9.1} ms  ({:.0} ns/row, n={converted})",
        fetch_convert_ms * 1e6 / fetched.max(1) as f64
    );
    eprintln!("fetch_all             : {fetch_all_ms:>9.1} ms");
    eprintln!(
        "convert-only(内存行)  : {convert_only_ms:>9.1} ms  ({:.0} ns/row, n={acc})",
        convert_only_ms * 1e6 / n as f64
    );
    eprintln!("serde to_writer x n   : {serde_ms:>9.1} ms  ({:.0} ns/row)", serde_ms * 1e6 / n as f64);
    eprintln!("FULL production       : {full_ms:>9.1} ms  (total={total})");
    eprintln!(
        "分工估算: raw fetch {:.0}% · convert {:.0}% · serde {:.0}% · 其它(spool/预览/编排) {:.0}%",
        100.0 * fetch_ms.min(full_ms) / full_ms,
        100.0 * convert_only_ms / full_ms,
        100.0 * serde_ms / full_ms,
        100.0 * (full_ms - fetch_ms - convert_only_ms - serde_ms).max(0.0) / full_ms
    );
    f.cleanup().await;
}
