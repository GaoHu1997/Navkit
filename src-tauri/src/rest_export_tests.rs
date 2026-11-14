//! 本机 mock 真实 wire 接口，验证 HBase/Salesforce 的 `export_page` 真分页、
//! 跨页行合并、limit=0 元数据、错误/提前停止时的清理，而不依赖厂商部署。
use super::connect as hbase_connect;
use crate::http_db::HttpAdapter;
use crate::models::{ConnectionRequest, DbKind};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::Duration;

enum HbBatch {
    Rows(Vec<Value>),
    Fail(u16),
}

struct State {
    token: String,
    actions: Vec<(String, String)>,
    hb_batches: VecDeque<HbBatch>,
    hb_scanners: HashMap<String, String>,
    hb_serial: u64,
    sf_mode: bool,
    sf_total: usize,
    sf_pages: VecDeque<Vec<Value>>,
    sf_cursors: HashMap<String, VecDeque<Vec<Value>>>,
    sf_serial: u64,
    sf_last_query: String,
    sf_bad_next: bool,
}

struct Reply {
    code: u16,
    body: Vec<u8>,
    headers: Vec<(String, String)>,
}

fn reply(code: u16, value: Value) -> Reply {
    Reply { code, body: serde_json::to_vec(&value).unwrap(), headers: vec![] }
}
fn empty(code: u16) -> Reply {
    Reply { code, body: Vec::new(), headers: vec![] }
}

struct Mock {
    port: u16,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Mock {
    fn hbase(batches: Vec<HbBatch>) -> Self {
        Self::start(false, batches, Vec::new(), 0)
    }
    fn salesforce(chunks: Vec<Vec<Value>>, total: usize) -> Self {
        Self::start(true, Vec::new(), chunks, total)
    }
    fn start(sf_mode: bool, hb: Vec<HbBatch>, sf_chunks: Vec<Vec<Value>>, sf_total: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(Mutex::new(State {
            token: "test-token".into(),
            actions: Vec::new(),
            hb_batches: hb.into(),
            hb_scanners: HashMap::new(),
            hb_serial: 0,
            sf_mode,
            sf_total,
            sf_pages: sf_chunks.into(),
            sf_cursors: HashMap::new(),
            sf_serial: 0,
            sf_last_query: String::new(),
            sf_bad_next: false,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let shared = state.clone();
        let done = stop.clone();
        let worker = thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                        let response = match read_request(&mut stream) {
                            Ok((method, target, headers)) => {
                                handle(&mut shared.lock().unwrap(), &method, &target, &headers)
                            }
                            Err(_) => empty(400),
                        };
                        write_reply(&mut stream, response);
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::park_timeout(Duration::from_millis(2))
                    }
                    Err(_) => break,
                }
            }
        });
        Self { port, state, stop, worker: Some(worker) }
    }
    fn req(&self, kind: &str) -> ConnectionRequest {
        ConnectionRequest {
            kind: DbKind::of(kind),
            host: "127.0.0.1".into(),
            port: self.port,
            user: if kind == "salesforce" { "token".into() } else { "user".into() },
            password: "test-token".into(),
            database: if kind == "salesforce" { "v59.0".into() } else { "default".into() },
            file: String::new(),
            params: "ssl=false".into(),
            timeout_secs: 15,
        }
    }
    fn actions(&self) -> Vec<(String, String)> {
        self.state.lock().unwrap().actions.clone()
    }
    fn scanners(&self) -> usize {
        self.state.lock().unwrap().hb_scanners.len()
    }
    fn set_bad_next(&self) {
        self.state.lock().unwrap().sf_bad_next = true;
    }
    fn last_query(&self) -> String {
        self.state.lock().unwrap().sf_last_query.clone()
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.worker.take() {
            t.thread().unpark();
            let _ = t.join();
        }
    }
}

fn read_request(stream: &mut std::net::TcpStream) -> Result<(String, String, HashMap<String, String>), ()> {
    let mut data = Vec::new();
    let end = loop {
        let mut block = [0u8; 4096];
        let n = stream.read(&mut block).map_err(|_| ())?;
        if n == 0 {
            return Err(());
        }
        data.extend_from_slice(&block[..n]);
        if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if data.len() > 1 << 20 {
            return Err(());
        }
    };
    let header = std::str::from_utf8(&data[..end]).map_err(|_| ())?;
    let mut lines = header.lines();
    let mut first = lines.next().ok_or(())?.split_whitespace();
    let method = first.next().ok_or(())?.to_string();
    let target = first.next().ok_or(())?.to_string();
    let headers: HashMap<String, String> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let len: usize = headers.get("content-length").and_then(|s| s.parse().ok()).unwrap_or(0);
    while data.len() < end + len {
        let mut block = [0u8; 4096];
        let n = stream.read(&mut block).map_err(|_| ())?;
        if n == 0 {
            return Err(());
        }
        data.extend_from_slice(&block[..n]);
    }
    Ok((method, target, headers))
}

fn write_reply(stream: &mut std::net::TcpStream, reply: Reply) {
    let reason = match reply.code {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Status",
    };
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.code,
        reason,
        reply.body.len()
    );
    for (k, v) in reply.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&reply.body);
    let _ = stream.flush();
}

fn handle(s: &mut State, method: &str, target: &str, headers: &HashMap<String, String>) -> Reply {
    s.actions.push((method.to_string(), target.to_string()));
    let expected = if s.sf_mode {
        format!("Bearer {}", s.token)
    } else {
        format!("Basic {}", STANDARD.encode(format!("user:{}", s.token)))
    };
    if headers.get("authorization") != Some(&expected) {
        return reply(401, json!({"error": "invalid credentials"}));
    }
    let url = match reqwest::Url::parse(&format!("http://127.0.0.1{target}")) {
        Ok(u) => u,
        Err(_) => return empty(400),
    };
    if s.sf_mode {
        salesforce(s, method, &url)
    } else {
        hbase(s, method, &url)
    }
}

fn salesforce(s: &mut State, method: &str, url: &reqwest::Url) -> Reply {
    let segs: Vec<&str> = url.path_segments().map(|p| p.collect()).unwrap_or_default();
    if segs.len() < 4 || segs[0] != "services" || segs[1] != "data" || segs[2] != "v59.0" {
        return reply(404, json!({"error": "unknown API"}));
    }
    match (segs[3], method) {
        ("sobjects", "GET") if segs.len() == 6 && segs[4] == "Account" && segs[5] == "describe" => {
            reply(200, json!({"name":"Account","fields":[
                {"name":"Id","type":"id","nillable":false},
                {"name":"Name","type":"string","nillable":false},
                {"name":"Nullable","type":"string","nillable":true}]}))
        }
        ("query", "GET") if segs.len() == 5 => {
            let id = segs[4].to_string();
            let mut queue = match s.sf_cursors.remove(&id) {
                Some(q) => q,
                None => return reply(404, json!({"error": "unknown cursor"})),
            };
            let chunk = queue.pop_front().unwrap_or_default();
            if queue.is_empty() {
                return reply(200, json!({"totalSize": s.sf_total, "done": true, "records": chunk}));
            }
            s.sf_serial += 1;
            let next_id = format!("q{}", s.sf_serial);
            s.sf_cursors.insert(next_id.clone(), queue);
            reply(200, json!({"totalSize": s.sf_total, "done": false, "records": chunk,
                "nextRecordsUrl": format!("/services/data/v59.0/query/{next_id}")}))
        }
        ("query", "GET") if segs.len() == 4 => {
            s.sf_last_query = url
                .query_pairs()
                .find(|(k, _)| k == "q")
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default();
            let mut pages = s.sf_pages.clone();
            let first = pages.pop_front().unwrap_or_default();
            if s.sf_bad_next {
                return reply(200, json!({"totalSize": s.sf_total, "done": false, "records": first,
                    "nextRecordsUrl": "http://127.0.0.1:1/stolen"}));
            }
            if pages.is_empty() {
                return reply(200, json!({"totalSize": s.sf_total, "done": true, "records": first}));
            }
            s.sf_serial += 1;
            let id = format!("q{}", s.sf_serial);
            s.sf_cursors.insert(id.clone(), pages);
            reply(200, json!({"totalSize": s.sf_total, "done": false, "records": first,
                "nextRecordsUrl": format!("/services/data/v59.0/query/{id}")}))
        }
        _ => reply(404, json!({"error": "unknown Salesforce route"})),
    }
}

fn hbase(s: &mut State, method: &str, url: &reqwest::Url) -> Reply {
    let segs: Vec<&str> = url.path_segments().map(|p| p.collect()).unwrap_or_default();
    if method == "GET" && segs.len() == 2 && segs[1] == "schema" {
        return reply(200, json!({"name": segs[0], "ColumnSchema": [{"name": "cf"}]}));
    }
    if method == "PUT" && segs.len() == 2 && segs[1] == "scanner" {
        s.hb_serial += 1;
        let id = format!("scan{}", s.hb_serial);
        s.hb_scanners.insert(id.clone(), segs[0].to_string());
        return Reply {
            code: 201,
            body: Vec::new(),
            headers: vec![("Location".into(), format!("/{}/scanner/{}", segs[0], id))],
        };
    }
    if segs.len() == 3 && segs[1] == "scanner" {
        let id = segs[2].to_string();
        if !s.hb_scanners.contains_key(&id) {
            return reply(404, json!({"error": "unknown scanner"}));
        }
        match method {
            "GET" => match s.hb_batches.pop_front() {
                None => empty(204),
                Some(HbBatch::Fail(code)) => reply(code, json!({"error": "scanner failed"})),
                Some(HbBatch::Rows(rows)) => reply(200, json!({"Row": rows})),
            },
            "DELETE" => {
                s.hb_scanners.remove(&id);
                empty(200)
            }
            _ => reply(404, json!({"error": "unsupported scanner method"})),
        }
    } else {
        reply(404, json!({"error": "unknown HBase route"}))
    }
}

fn hb_cell(column: &str, value: &str, ts: u64) -> Value {
    json!({
        "column": STANDARD.encode(format!("cf:{column}").as_bytes()),
        "$": STANDARD.encode(value.as_bytes()),
        "timestamp": ts,
    })
}
fn hb_row(key: &str, cells: Vec<Value>) -> Value {
    json!({"key": STANDARD.encode(key.as_bytes()), "Cell": cells})
}

async fn hbase_adapter(mock: &Mock) -> Box<dyn HttpAdapter> {
    hbase_connect(&mock.req("hbase")).await.unwrap()
}
async fn salesforce_adapter(mock: &Mock) -> Box<dyn HttpAdapter> {
    crate::salesforce_rest::connect(&mock.req("salesforce")).await.unwrap()
}

fn account(i: usize) -> Value {
    json!({
        "attributes": {"type": "Account"},
        "Id": format!("00100000000000{i}"),
        "Name": format!("name-{i}"),
        "Nullable": Value::Null,
    })
}

#[tokio::test]
async fn salesforce_export_paginates_beyond_browse_window_and_selects_all_columns() {
    let records: Vec<Value> = (0..2505).map(account).collect();
    let chunks = records.chunks(1000).map(|chunk|chunk.to_vec()).collect();
    let mock = Mock::salesforce(chunks, records.len());
    let adapter = salesforce_adapter(&mock).await;

    // 浏览窗口照样受限，但导出必须越过它。
    assert!(adapter.rows("v59.0", "Account", 2001, 10).await.is_err());
    let describe_before = mock.actions().iter().filter(|(_, path)| path.ends_with("/describe")).count();

    let mut collected = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let page = adapter.export_page("v59.0", "Account", token.as_deref(), 1000).await.unwrap();
        collected.extend(page.data.rows.iter().cloned());
        token = page.next;
        pages += 1;
        if token.is_none() || pages > 10 {
            break;
        }
    }
    assert_eq!(collected.len(), 2505);
    assert_eq!(collected[0][1].as_deref(), Some("name-0"));
    assert_eq!(collected[2504][1].as_deref(), Some("name-2504"));
    assert!(collected.iter().all(|row|row[2].is_none()));
    assert_eq!(mock.actions().iter().filter(|(_,path)|path.ends_with("/describe")).count() - describe_before,1,"冻结首次 describe 字段，不得每页改变结构");
    assert!(pages >= 3, "expected real multi-page pagination, got {pages}");

    let query = mock.last_query();
    assert!(query.contains("SELECT Id,Name,Nullable FROM Account"), "{query}");
    assert!(!query.contains("LIMIT") && !query.contains("OFFSET"), "{query}");
}

#[tokio::test]
async fn salesforce_limit_zero_returns_metadata_without_query() {
    let mock = Mock::salesforce(vec![vec![account(0)]], 1);
    let adapter = salesforce_adapter(&mock).await;
    let page = adapter.export_page("v59.0", "Account", None, 0).await.unwrap();
    assert_eq!(page.data.columns, vec!["Id", "Name", "Nullable"]);
    assert!(page.data.rows.is_empty());
    assert!(page.next.is_none());
    let queried = mock.actions().iter().any(|(_, p)| p.contains("/query"));
    assert!(!queried, "limit=0 不应发起 SOQL 查询");
}

#[tokio::test]
async fn salesforce_rejects_cross_origin_next_records_url() {
    let mock = Mock::salesforce(vec![vec![account(0)], vec![account(1)]], 2);
    mock.set_bad_next();
    let adapter = salesforce_adapter(&mock).await;
    let error = adapter.export_page("v59.0", "Account", None, 1000).await.err().expect("应当拒绝跨域游标");
    let text = format!("{error:#}");
    assert!(text.contains("nextRecordsUrl") || text.contains("origin"), "{text}");
}

#[tokio::test]
async fn hbase_export_merges_cross_page_rows_and_cleans_scanner() {
    let batches = vec![
        HbBatch::Rows(vec![
            hb_row("r1", vec![hb_cell("a", "1", 1)]),
            hb_row("r2", vec![hb_cell("a", "2", 1)]),
        ]),
        HbBatch::Rows(vec![
            hb_row("r2", vec![hb_cell("b", "3", 2)]),
            hb_row("r3", vec![hb_cell("a", "4", 1)]),
        ]),
    ];
    let mock = Mock::hbase(batches);
    let adapter = hbase_adapter(&mock).await;

    assert!(adapter.rows("default", "t1", 100001, 10).await.is_err());

    let mut merged: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let page = adapter.export_page("default", "t1", token.as_deref(), 1000).await.unwrap();
        let columns = page.data.columns.clone();
        for row in &page.data.rows {
            let key = row[0].clone().unwrap();
            let mut cells = BTreeMap::new();
            for (index, column) in columns.iter().enumerate().skip(1) {
                if let Some(value) = &row[index] {
                    cells.insert(column.clone(), value.clone());
                }
            }
            merged.insert(key, cells);
        }
        token = page.next;
        pages += 1;
        if token.is_none() || pages > 10 {
            break;
        }
    }
    assert_eq!(merged["r1"]["cf:a"].as_str(), "1");
    assert_eq!(merged["r2"]["cf:a"].as_str(), "2");
    assert_eq!(merged["r2"]["cf:b"].as_str(), "3", "跨 scanner 页的同一 row cells 必须合并");
    assert_eq!(merged["r3"]["cf:a"].as_str(), "4");
    assert_eq!(mock.scanners(), 0, "EOF 后必须删除 scanner");
}

#[tokio::test]
async fn hbase_finish_export_deletes_scanner_on_early_stop() {
    let batches = vec![
        HbBatch::Rows(vec![hb_row("a", vec![hb_cell("a", "1", 1)]), hb_row("b", vec![hb_cell("a", "2", 1)])]),
        HbBatch::Rows(vec![hb_row("c", vec![hb_cell("a", "3", 1)])]),
    ];
    let mock = Mock::hbase(batches);
    let adapter = hbase_adapter(&mock).await;
    let page = adapter.export_page("default", "t1", None, 1000).await.unwrap();
    let token = page.next.expect("应返回下一页游标");
    assert_eq!(mock.scanners(), 1);
    adapter.finish_export(Some(&token)).await.unwrap();
    assert_eq!(mock.scanners(), 0, "提前停止必须删除 scanner");
    assert!(adapter.export_page("default", "t1", Some(&token), 1000).await.is_err());
}

#[tokio::test]
async fn hbase_export_error_deletes_scanner() {
    let mock = Mock::hbase(vec![HbBatch::Fail(500)]);
    let adapter = hbase_adapter(&mock).await;
    let error = adapter.export_page("default", "t1", None, 1000).await.err().expect("scanner 读取失败应报错");
    assert!(format!("{error:#}").contains("REST HTTP 500"));
    assert_eq!(mock.scanners(), 0, "错误后必须删除 scanner");
    assert!(mock.actions().iter().any(|(m, p)| m == "DELETE" && p.contains("/scanner/")));
}

#[tokio::test]
async fn hbase_limit_zero_and_oversized_token_rejected() {
    let mock = Mock::hbase(Vec::new());
    let adapter = hbase_adapter(&mock).await;
    let page = adapter.export_page("default", "t1", None, 0).await.unwrap();
    assert_eq!(page.data.columns, vec!["row", "cf"]);
    assert!(page.data.rows.is_empty());
    assert!(page.next.is_none());
    assert!(
        !mock.actions().iter().any(|(m, _)| m == "PUT"),
        "limit=0 不应创建 scanner"
    );
    assert_eq!(mock.scanners(), 0);

    let oversized = "x".repeat(64 * 1024 + 1);
    assert!(adapter.export_page("default", "t1", Some(&oversized), 10).await.is_err());
}

#[tokio::test]
async fn salesforce_missing_or_unknown_fields_are_not_silently_lost() {
    for missing in [true,false] {
        let mut row=account(0);
        if missing {row.as_object_mut().unwrap().remove("Name");}
        else {row.as_object_mut().unwrap().insert("Undescribed".into(),json!("lost"));}
        let mock=Mock::salesforce(vec![vec![row]],1);
        let adapter=salesforce_adapter(&mock).await;
        assert!(adapter.export_page("v59.0","Account",None,1000).await.is_err());
    }
}

#[tokio::test]
async fn salesforce_done_cannot_hide_missing_records() {
    let mock=Mock::salesforce(vec![vec![account(0)]],2);
    let adapter=salesforce_adapter(&mock).await;
    let error=adapter.export_page("v59.0","Account",None,1000).await.err().unwrap();
    assert!(format!("{error:#}").contains("totalSize"));
}

#[tokio::test]
async fn salesforce_cursor_is_bounded_and_object_bound_without_extra_requests() {
    let mock=Mock::salesforce(vec![vec![account(0)],vec![account(1)]],2);
    let adapter=salesforce_adapter(&mock).await;
    let first=adapter.export_page("v59.0","Account",None,1).await.unwrap();
    let before=mock.actions().len();
    assert!(adapter.export_page("v59.0","Contact",first.next.as_deref(),1).await.is_err());
    assert!(adapter.export_page("v59.0","Account",Some(&"x".repeat(64*1024+1)),1).await.is_err());
    assert_eq!(mock.actions().len(),before);
    // Salesforce query locators have no DELETE contract. Stopping is local and
    // must not consume pages merely to dispose the locator (server TTL handles it).
    adapter.finish_export(first.next.as_deref()).await.unwrap();
    assert_eq!(mock.actions().len(),before);
}

#[tokio::test]
async fn salesforce_response_over_16_mib_is_an_error() {
    let mut row=account(0);row["Name"]=json!("x".repeat(16*1024*1024+1));
    let mock=Mock::salesforce(vec![vec![row]],1);
    let adapter=salesforce_adapter(&mock).await;
    let error=adapter.export_page("v59.0","Account",None,1000).await.err().unwrap();
    let text = format!("{error:#}");
    assert!(text.contains("REST 响应超过 16 MiB；未截断为成功结果"), "{text}");
}

#[tokio::test]
async fn hbase_stale_and_wrong_table_tokens_do_not_advance_or_delete_scanner() {
    let mock=Mock::hbase(vec![
        HbBatch::Rows(vec![hb_row("a",vec![hb_cell("a","1",1)]),hb_row("b",vec![hb_cell("a","2",1)])]),
        HbBatch::Rows(vec![hb_row("c",vec![hb_cell("a","3",1)])]),
    ]);
    let adapter=hbase_adapter(&mock).await;
    let first=adapter.export_page("default","t1",None,1).await.unwrap();
    let before=mock.actions().len();
    assert!(adapter.export_page("default","t2",first.next.as_deref(),1).await.is_err());
    assert_eq!(mock.actions().len(),before);
    let second=adapter.export_page("default","t1",first.next.as_deref(),1).await.unwrap();
    let before=mock.actions().len();
    assert!(adapter.export_page("default","t1",first.next.as_deref(),1).await.is_err());
    assert!(adapter.finish_export(first.next.as_deref()).await.is_err());
    assert_eq!(mock.actions().len(),before);
    assert_eq!(mock.scanners(),1);
    adapter.finish_export(second.next.as_deref()).await.unwrap();
    adapter.finish_export(second.next.as_deref()).await.unwrap();
    assert_eq!(mock.scanners(),0);
}

#[tokio::test]
async fn hbase_does_not_discard_overreturned_rows_for_page_limit() {
    let mock=Mock::hbase(vec![HbBatch::Rows((0..101).map(|i|hb_row(&format!("r{i:04}"),vec![hb_cell("a",&i.to_string(),1)])).collect())]);
    let adapter=hbase_adapter(&mock).await;
    let first=adapter.export_page("default","t1",None,1).await.unwrap();
    assert_eq!(first.data.rows.len(),100,"请求大小不是全表截断边界");
    let last=adapter.export_page("default","t1",first.next.as_deref(),1).await.unwrap();
    assert_eq!(last.data.rows.len(),1);
    assert_eq!(last.data.rows[0][0].as_deref(),Some("r0100"));
    assert!(last.next.is_none());assert_eq!(mock.scanners(),0);
}

#[tokio::test]
async fn hbase_bad_cell_and_nonadvancing_row_delete_scanner() {
    let invalid=json!({"column":STANDARD.encode(b"cf:a"),"$":"!invalid base64!","timestamp":1});
    let cases=vec![
        vec![hb_row("a",vec![invalid])],
        vec![hb_row("b",vec![hb_cell("a","1",1)]),hb_row("a",vec![hb_cell("a","2",1)])],
        vec![hb_row("a",vec![hb_cell("a","1",1),hb_cell("a","2",1)])],
    ];
    for rows in cases {
        let mock=Mock::hbase(vec![HbBatch::Rows(rows)]);
        let adapter=hbase_adapter(&mock).await;
        assert!(adapter.export_page("default","t1",None,1000).await.is_err());
        assert_eq!(mock.scanners(),0);
    }
}

#[tokio::test]
async fn hbase_response_over_16_mib_deletes_scanner() {
    let value="x".repeat(13*1024*1024);
    let mock=Mock::hbase(vec![HbBatch::Rows(vec![hb_row("a",vec![hb_cell("a",&value,1)])])]);
    let adapter=hbase_adapter(&mock).await;
    let error=adapter.export_page("default","t1",None,1000).await.err().unwrap();
    let text = format!("{error:#}");
    assert!(text.contains("REST 响应超过 16 MiB；未截断为成功结果"), "{text}");
    assert_eq!(mock.scanners(),0);
}

#[tokio::test]
async fn hbase_cross_page_pending_row_obeys_16_mib_bound() {
    let value="x".repeat(6*1024*1024);
    let mock=Mock::hbase((0..3).map(|i|HbBatch::Rows(vec![hb_row("a",vec![hb_cell(&format!("c{i}"),&value,1)])])).collect());
    let adapter=hbase_adapter(&mock).await;
    let error=adapter.export_page("default","t1",None,1000).await.err().unwrap();
    assert!(format!("{error:#}").contains("单行"));assert_eq!(mock.scanners(),0);
}

#[tokio::test]
async fn hbase_sparse_expansion_obeys_serialized_page_bound() {
    // Each wire batch is tiny, but 1200 distinct sparse columns expanded into
    // 1200 full rows exceed 16 MiB. Reject before allocating that matrix.
    let rows=(0..1201).map(|i|hb_row(&format!("r{i:04}"),vec![hb_cell(&format!("c{i}"),"x",1)])).collect();
    let mock=Mock::hbase(vec![HbBatch::Rows(rows)]);
    let adapter=hbase_adapter(&mock).await;
    let result=adapter.export_page("default","t1",None,1000).await;
    // 1200^2 nulls still fit (~7 MiB); a larger page is tested below.
    let page=result.unwrap();adapter.finish_export(page.next.as_deref()).await.unwrap();
    assert_eq!(mock.scanners(),0);
    let rows=(0..2001).map(|i|hb_row(&format!("r{i:04}"),vec![hb_cell(&format!("c{i}"),"x",1)])).collect();
    let mock=Mock::hbase(vec![HbBatch::Rows(rows)]);
    let adapter=hbase_adapter(&mock).await;
    assert!(adapter.export_page("default","t1",None,1000).await.is_err());
    assert_eq!(mock.scanners(),0);
}

