//! `transfer_http_export::export` 的端到端测试。
//!
//! 用真实的 `HttpDb`（经 `HttpDb::from_adapter` 注入一个内存假适配器）驱动分页，
//! 覆盖：跨页动态列、短页不是 EOF、服务端返回行数超过请求上限时的截断、以及
//! 通过 `run_export_task` 的真实发布/不发布与清理行为。不连接任何用户数据库。

use std::collections::VecDeque;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{bail, Result};
use async_trait::async_trait;

use crate::db::Db;
use crate::http_db::{HttpAdapter, HttpDb, HttpExportPage, HttpRows};
use crate::models::{ConnectionRequest, DbKind, ExportOptions};
use crate::transfer::transfer_output::StagedOutput;
use crate::transfer::{run_export_task, ExportTaskOptions, TaskControl};
use crate::transfer_stream::TransferLimits;

/// 一页的静态描述（可克隆，便于每轮召唤时重建 `HttpExportPage`）。
#[derive(Clone)]
struct PageSpec {
    columns: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
    next: Option<String>,
}

/// 内存假适配器：按顺序逐页返回 `pages`；`fail` 为真时首次导出即报错。
struct MockAdapter {
    pages: Mutex<VecDeque<PageSpec>>,
    fail: bool,
    cleanup: Option<Arc<Mutex<Vec<Option<String>>>>>,
    cleanup_fails: bool,
}

impl MockAdapter {
    fn new(pages: Vec<PageSpec>) -> Self {
        Self { pages: Mutex::new(pages.into()), fail: false, cleanup: None, cleanup_fails: false }
    }

    fn failing() -> Self {
        Self { pages: Mutex::new(VecDeque::new()), fail: true, cleanup: None, cleanup_fails: false }
    }
}

#[async_trait]
impl HttpAdapter for MockAdapter {
    async fn ping(&self) -> Result<String> {
        Ok("mock-http".into())
    }

    async fn containers(&self) -> Result<Vec<(String, String)>> {
        Ok(vec![])
    }

    async fn objects(&self, _container: &str) -> Result<Vec<String>> {
        Ok(vec![])
    }

    async fn rows(
        &self,
        _container: &str,
        _object: &str,
        _offset: u64,
        _limit: u64,
    ) -> Result<HttpRows> {
        bail!("测试适配器只实现 export_page，不实现 browse rows")
    }

    async fn run(&self, _container: &str, _object: &str, _sql: &str) -> Result<HttpRows> {
        bail!("测试适配器不支持自由执行")
    }

    async fn finish_export(&self, token: Option<&str>) -> Result<()> {
        if let Some(cleanup) = &self.cleanup {
            cleanup.lock().unwrap().push(token.map(str::to_owned));
        }
        if self.cleanup_fails { bail!("mock cleanup failed"); }
        Ok(())
    }

    async fn export_page(
        &self,
        _container: &str,
        _object: &str,
        _token: Option<&str>,
        _limit: u64,
    ) -> Result<HttpExportPage> {
        if self.fail {
            bail!("mock 导出失败（用于验证错误不发布）");
        }
        let spec = self.pages.lock().unwrap().pop_front();
        match spec {
            Some(spec) => Ok(HttpExportPage {
                data: HttpRows { columns: spec.columns, rows: spec.rows },
                next: spec.next,
            }),
            None => Ok(HttpExportPage {
                data: HttpRows { columns: Vec::new(), rows: Vec::new() },
                next: None,
            }),
        }
    }
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("navkit-http-e2e-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self { path }
    }

    fn file(&self, name: &str) -> String {
        self.path.join(name).to_str().unwrap().to_string()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn request() -> ConnectionRequest {
    ConnectionRequest {
        kind: DbKind::of("elasticsearch"),
        host: "127.0.0.1".into(),
        port: 1,
        user: String::new(),
        password: String::new(),
        database: String::new(),
        file: String::new(),
        params: String::new(),
        timeout_secs: 5,
    }
}

async fn db_with(adapter: MockAdapter) -> HttpDb {
    HttpDb::from_adapter(&request(), Box::new(adapter)).await.unwrap()
}

fn options(file: String, format: &str, limit: Option<u64>) -> ExportOptions {
    ExportOptions {
        connection_id: "t".into(),
        schema: "db".into(),
        table: "obj".into(),
        file,
        format: format.into(),
        has_header: true,
        delimiter: ",".into(),
        encoding: "utf-8".into(),
        where_clause: None,
        limit,
    }
}

fn test_task() -> TaskControl {
    TaskControl::begin(&format!("http-e2e-{}", uuid::Uuid::new_v4().simple())).unwrap()
}

fn read_stage(stage: &StagedOutput) -> String {
    let mut file = stage.file_clone().unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut text = String::new();
    file.read_to_string(&mut text).unwrap();
    text
}

#[tokio::test]
async fn csv_merges_dynamic_columns_and_does_not_treat_short_page_as_eof() {
    let dir = TempDir::new();
    let pages = vec![
        PageSpec {
            columns: vec!["a".into(), "b".into()],
            rows: vec![
                vec![Some("1".into()), Some("2".into())],
                vec![Some("3".into()), Some("4".into())],
            ],
            next: Some("t1".into()),
        },
        // 只有 1 行，但仍有下一页游标：短页绝不是 EOF。
        PageSpec {
            columns: vec!["a".into(), "b".into(), "c".into()],
            rows: vec![vec![Some("5".into()), Some("6".into()), Some("7".into())]],
            next: Some("t2".into()),
        },
        PageSpec {
            columns: vec!["a".into(), "c".into()],
            rows: vec![vec![Some("8".into()), Some("9".into())]],
            next: None,
        },
    ];
    let http = db_with(MockAdapter::new(pages)).await;
    let kind = DbKind::of("elasticsearch");
    let opts = options(dir.file("out.csv"), "csv", None);
    let stage = StagedOutput::new(&opts.file).unwrap();
    let task = test_task();

    let written = super::export(&http, &kind, &opts, &task, &stage).await.unwrap();
    assert_eq!(written, 4);
    assert_eq!(
        read_stage(&stage),
        "a,b,c\r\n1,2,\\N\r\n3,4,\\N\r\n5,6,7\r\n8,\\N,9\r\n"
    );
    drop(stage);
    let leftovers = std::fs::read_dir(&dir.path)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(".navkit-export-"))
        .count();
    assert_eq!(leftovers, 0, "导出结束后 spool/stage 临时文件必须清理");
    task.finish(&Ok::<u64, anyhow::Error>(written));
}

#[tokio::test]
async fn csv_truncates_when_server_returns_more_rows_than_requested_limit() {
    let dir = TempDir::new();
    let pages = vec![PageSpec {
        columns: vec!["v".into()],
        rows: vec![
            vec![Some("1".into())],
            vec![Some("2".into())],
            vec![Some("3".into())],
            vec![Some("4".into())],
            vec![Some("5".into())],
        ],
        // 即使还有游标，达到总 limit 后不应再追下一页。
        next: Some("t1".into()),
    }];
    let http = db_with(MockAdapter::new(pages)).await;
    let kind = DbKind::of("elasticsearch");
    let opts = options(dir.file("out.csv"), "csv", Some(3));
    let stage = StagedOutput::new(&opts.file).unwrap();
    let task = test_task();

    let written = super::export(&http, &kind, &opts, &task, &stage).await.unwrap();
    assert_eq!(written, 3, "服务端多返回的行必须被截断到请求上限");
    assert_eq!(read_stage(&stage), "v\r\n1\r\n2\r\n3\r\n");
    task.finish(&Ok::<u64, anyhow::Error>(written));
}

#[tokio::test]
async fn run_export_task_publishes_json_and_leaves_no_target_on_error() {
    let dir = TempDir::new();

    // 1) 成功发布 JSON。
    let ok_pages = vec![PageSpec {
        columns: vec!["a".into(), "b".into()],
        rows: vec![vec![Some("1".into()), None]],
        next: None,
    }];
    let ok_http = db_with(MockAdapter::new(ok_pages)).await;
    let ok_kind = DbKind::of("elasticsearch");
    let ok_opts = options(dir.file("ok.json"), "json", None);
    let ok_task = ExportTaskOptions {
        sql_batch_rows: 0, sql_batch_bytes: 0,
        options: ok_opts.clone(),
        task_id: format!("http-ok-{}", uuid::Uuid::new_v4().simple()),
    };
    let ok_db = Db::Http(ok_http);
    let count = run_export_task(&ok_db, &ok_kind, &ok_task).await.unwrap();
    assert_eq!(count, 1);
    let published = std::fs::read_to_string(&ok_opts.file).unwrap();
    assert!(published.starts_with('['), "JSON 应以数组开头: {published}");
    assert!(published.contains("\"a\":\"1\""), "缺少 a=1: {published}");
    assert!(published.contains("\"b\":null"), "缺少 b=null: {published}");

    // 2) 出错时不发布目标文件，也不残留临时文件。
    let fail_http = db_with(MockAdapter::failing()).await;
    let fail_kind = DbKind::of("elasticsearch");
    let fail_opts = options(dir.file("fail.csv"), "csv", None);
    let fail_file = fail_opts.file.clone();
    let fail_task = ExportTaskOptions {
        sql_batch_rows: 0, sql_batch_bytes: 0,
        options: fail_opts,
        task_id: format!("http-fail-{}", uuid::Uuid::new_v4().simple()),
    };
    let fail_db = Db::Http(fail_http);
    let result = run_export_task(&fail_db, &fail_kind, &fail_task).await;
    assert!(result.is_err(), "导出失败必须返回错误");
    assert!(!std::path::Path::new(&fail_file).exists(), "失败时不得发布目标文件");
    let leftovers = std::fs::read_dir(&dir.path)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(".navkit-export-"))
        .count();
    assert_eq!(leftovers, 0, "失败后不得残留导出 stage 临时文件");
}

/// 无总 limit 时，即使服务器在一页里 overreturn 超过请求批量，也不能按请求批量裁剪；
/// 该页必须全部写出，最后一行存在。
#[tokio::test]
async fn csv_writes_all_rows_when_page_overreturns_without_total_limit() {
    let dir = TempDir::new();
    let n = TransferLimits::default().batch_rows as usize + 2;
    let rows: Vec<Vec<Option<String>>> =
        (1..=n).map(|i| vec![Some(i.to_string())]).collect();
    let pages = vec![PageSpec { columns: vec!["v".into()], rows, next: None }];
    let http = db_with(MockAdapter::new(pages)).await;
    let kind = DbKind::of("elasticsearch");
    let opts = options(dir.file("out.csv"), "csv", None);
    let stage = StagedOutput::new(&opts.file).unwrap();
    let task = test_task();

    let written = super::export(&http, &kind, &opts, &task, &stage).await.unwrap();
    assert_eq!(written, n as u64, "无总 limit 时必须写出整页全部行");
    let text = read_stage(&stage);
    assert!(text.starts_with("v\r\n"), "缺少表头");
    assert_eq!(text.lines().count(), n + 1, "表头 + 全部数据行");
    assert!(text.ends_with(&format!("{n}\r\n")), "最后一行缺失或截断");
    task.finish(&Ok::<u64, anyhow::Error>(written));
}

#[tokio::test]
async fn json_writes_all_rows_when_page_overreturns_without_total_limit() {
    let dir = TempDir::new();
    let n = TransferLimits::default().batch_rows as usize + 2;
    let rows: Vec<Vec<Option<String>>> =
        (1..=n).map(|i| vec![Some(i.to_string())]).collect();
    let pages = vec![PageSpec { columns: vec!["v".into()], rows, next: None }];
    let http = db_with(MockAdapter::new(pages)).await;
    let kind = DbKind::of("elasticsearch");
    let opts = options(dir.file("out.json"), "json", None);
    let stage = StagedOutput::new(&opts.file).unwrap();
    let task = test_task();

    let written = super::export(&http, &kind, &opts, &task, &stage).await.unwrap();
    assert_eq!(written, n as u64, "无总 limit 时必须写出整页全部行");
    let text = read_stage(&stage);
    assert!(text.starts_with('['), "JSON 应以数组开头");
    assert!(text.ends_with(']'), "JSON 应以数组结尾");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&text).unwrap().as_array().unwrap().len(), n);
    assert_eq!(text.matches("\"v\":").count(), n, "JSON 记录数应等于全部行");
    assert!(text.contains(&format!("\"v\":\"{n}\"")), "最后一条记录缺失");
    task.finish(&Ok::<u64, anyhow::Error>(written));
}

#[tokio::test]
async fn latest_resource_token_is_cleaned_on_limit_validation_and_cleanup_error() {
    for format in ["csv", "json"] {
        for mode in ["limit", "invalid-row", "cleanup-error", "invalid-row-cleanup-error"] {
            let dir = TempDir::new();
            let cleanup = Arc::new(Mutex::new(Vec::new()));
            let row = if mode.starts_with("invalid-row") { vec![] } else { vec![Some("1".into())] };
            let mut adapter = MockAdapter::new(vec![PageSpec {
                columns: vec!["v".into()], rows: vec![row], next: Some("latest-resource".into()),
            }]);
            adapter.cleanup = Some(cleanup.clone());
            adapter.cleanup_fails = mode.ends_with("cleanup-error");
            let http = db_with(adapter).await;
            let kind = DbKind::of("elasticsearch");
            let opts = options(dir.file(&format!("{mode}.{format}")), format, Some(1));
            let target = opts.file.clone();
            let task = ExportTaskOptions { sql_batch_rows: 0, sql_batch_bytes: 0, options: opts, task_id: format!("cleanup-{}", uuid::Uuid::new_v4().simple()) };
            let result = run_export_task(&Db::Http(http), &kind, &task).await;
            assert_eq!(cleanup.lock().unwrap().as_slice(), &[Some("latest-resource".into())]);
            if mode == "limit" { assert_eq!(result.unwrap(), 1); }
            else {
                let error = result.unwrap_err();
                if mode == "invalid-row-cleanup-error" {
                    let message = format!("{error:#}");
                    assert!(message.contains("列数不一致"), "read error must be retained: {message}");
                    assert!(message.contains("mock cleanup failed"), "cleanup failure must be reported: {message}");
                }
                assert!(!std::path::Path::new(&target).exists());
            }
            assert!(std::fs::read_dir(&dir.path).unwrap().all(|entry| !entry.unwrap().file_name().to_string_lossy().starts_with(".navkit-export-")));
        }
    }
}

#[test]
fn pagination_token_over_limit_is_rejected() {
    // 超过 64 KiB 的分页游标必须在进入有界历史前被拒绝。
    let mut chain = super::TokenChain::new();
    let big = "x".repeat(super::MAX_TOKEN_BYTES + 1);
    let err = super::advance_token(None, &big, &mut chain).unwrap_err();
    assert!(err.to_string().contains("游标超过上限"), "unexpected error: {err}");
}
