//! Loopback HTTP wire tests for actual ApiDb export requests; no vendor server required.
use super::*;
use crate::models::DbKind;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}},
    thread,
    time::Duration,
};

#[derive(Clone)]
struct Wire { path: String, headers: String, body: Value }
struct Server {
    port: u16,
    requests: Arc<Mutex<Vec<Wire>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn start(mut handler: impl FnMut(&Wire) -> Value + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let shared = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let worker = thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Windows can inherit FIONBIO from the nonblocking listener.
                        // The fixture's read_wire uses blocking reads, including fragmented requests.
                        stream.set_nonblocking(false).unwrap();
                        stream.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
                        stream.set_write_timeout(Some(Duration::from_secs(15))).unwrap();
                        let wire = read_wire(&mut stream);
                        let response = serde_json::to_vec(&handler(&wire)).unwrap();
                        shared.lock().unwrap().push(wire);
                        let header = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", response.len());
                        let _ = stream.write_all(header.as_bytes());
                        let _ = stream.write_all(&response);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::park_timeout(Duration::from_millis(2));
                    }
                    Err(error) => panic!("fixture accept failed: {error}"),
                }
            }
        });
        Self { port, requests, stop, worker: Some(worker) }
    }
    async fn db(&self, kind: &str) -> ApiDb {
        ApiDb::connect(&ConnectionRequest {
            kind: DbKind::of(kind), host: "127.0.0.1".into(), port: self.port,
            user: if kind == "dynamodb" { "test-access".into() } else { String::new() },
            password: if kind == "dynamodb" { "test-secret".into() } else { String::new() },
            database: String::new(), file: String::new(), params: "ssl=false".into(),
            timeout_secs: 15,
        }).await.unwrap()
    }
    fn requests(&self) -> Vec<Wire> { self.requests.lock().unwrap().clone() }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            if let Err(failure) = worker.join() {
                // Preserve a fixture assertion as a normal test failure, but do not
                // abort the entire test executable by panicking during an existing unwind.
                if !thread::panicking() { std::panic::resume_unwind(failure); }
            }
        }
    }
}
fn read_wire(stream: &mut TcpStream) -> Wire {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut block = [0u8; 4096];
        let n = stream.read(&mut block).unwrap();
        assert_ne!(n, 0, "fixture request ended before headers");
        bytes.extend_from_slice(&block[..n]);
        assert!(bytes.len() <= 1024 * 1024, "fixture request too large");
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") { break end + 4; }
    };
    let headers = std::str::from_utf8(&bytes[..header_end]).unwrap().to_string();
    let length = headers.lines().find_map(|line| {
        line.split_once(':').filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
            .map(|(_, value)| value.trim().parse::<usize>().unwrap())
    }).unwrap_or(0);
    while bytes.len() < header_end + length {
        let mut block = [0u8; 4096];
        let n = stream.read(&mut block).unwrap();
        assert_ne!(n, 0, "fixture request ended before body");
        bytes.extend_from_slice(&block[..n]);
    }
    Wire {
        path: headers.lines().next().unwrap().split_whitespace().nth(1).unwrap().into(),
        headers,
        body: serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap(),
    }
}

#[test]
fn native_export_fixture_waits_for_fragmented_request_on_accepted_socket() {
    let server = Server::start(|wire| {
        assert_eq!(wire.path, "/fragmented");
        assert_eq!(wire.body, json!({"key":"AP+A"}));
        json!({"ok":true})
    });
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    let mut response = [0u8; 256];
    // No request bytes exist yet: the server must wait instead of treating
    // a nonblocking accept's WouldBlock as a failed request and closing it.
    let error = stream.read(&mut response).unwrap_err();
    assert!(matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut), "{error}");
    let body = br#"{"key":"AP+A"}"#;
    write!(stream, "POST /fragmented HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
    let error = stream.read(&mut response).unwrap_err();
    assert!(matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut), "{error}");
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    stream.write_all(&body[..5]).unwrap();
    stream.write_all(&body[5..]).unwrap();
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).unwrap();
    assert!(std::str::from_utf8(&reply).unwrap().starts_with("HTTP/1.1 200 OK"));
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn native_export_fixture_worker_panic_is_propagated_without_double_unwind_abort() {
    fn failed_server() -> Server {
        Server {
            port: 0,
            requests: Arc::new(Mutex::new(Vec::new())),
            stop: Arc::new(AtomicBool::new(false)),
            worker: Some(thread::spawn(|| panic!("fixture assertion failure"))),
        }
    }
    // A worker's assertions must still fail a test when there is no other panic.
    let worker_failure = std::panic::catch_unwind(|| drop(failed_server())).unwrap_err();
    assert_eq!(worker_failure.downcast_ref::<&str>(), Some(&"fixture assertion failure"));
    // If the request caller is already unwinding, keep that original failure
    // rather than invoking a second panic from the fixture destructor.
    let request_failure = std::panic::catch_unwind(|| {
        let _server = failed_server();
        panic!("request assertion failure");
    }).unwrap_err();
    assert_eq!(request_failure.downcast_ref::<&str>(), Some(&"request assertion failure"));
}

#[tokio::test]
async fn dynamo_export_scans_beyond_10000_without_replaying_browse_window() {
    let total = 10_005usize;
    let mut expected = 0usize;
    let server = Server::start(move |wire| {
        assert_eq!(wire.path, "/");
        assert!(wire.headers.to_ascii_lowercase().contains("x-amz-target: dynamodb_20120810.scan"));
        assert!(wire.headers.contains("AWS4-HMAC-SHA256"));
        assert_eq!(wire.body["TableName"], "records");
        assert_eq!(wire.body["Limit"], 1000);
        let offset = wire.body.pointer("/ExclusiveStartKey/id/N")
            .and_then(Value::as_str).map(|v| v.parse::<usize>().unwrap()).unwrap_or(0);
        assert_eq!(offset, expected, "Scan must continue exactly from previous LEK");
        let end = (offset + 1000).min(total);
        let items: Vec<Value> = (offset..end).map(|id| json!({
            "id":{"N":id.to_string()}, "large":{"N":"12345678901234567890123456789012345678"},
            "binary":{"B":"AP8="}, "nullable":{"NULL":true}
        })).collect();
        expected = end;
        let mut response = json!({"Items":items});
        if end < total { response["LastEvaluatedKey"] = json!({"id":{"N":end.to_string()}}); }
        response
    });
    let db = server.db("dynamodb").await;
    let mut token = None;
    let mut rows = 0usize;
    let mut last = String::new();
    loop {
        let page = db.export_page("us-east-1", "records", token.as_deref(), 1000).await.unwrap();
        let id_column = page.data.columns.iter().position(|c| c == "id").unwrap();
        let binary_column = page.data.columns.iter().position(|c| c == "binary").unwrap();
        let number_column = page.data.columns.iter().position(|c| c == "large").unwrap();
        for row in page.data.rows {
            assert_eq!(row[binary_column].as_deref(), Some("{\"B\":\"AP8=\"}"));
            assert_eq!(row[number_column].as_deref(), Some("{\"N\":\"12345678901234567890123456789012345678\"}"));
            last = row[id_column].clone().unwrap();
            rows += 1;
        }
        token = page.next;
        if token.is_none() { break; }
    }
    assert_eq!(rows, total);
    assert_eq!(last, "{\"N\":\"10004\"}");
    assert_eq!(server.requests().len(), 11, "no whole-table replay requests");
}

#[tokio::test]
async fn dynamo_empty_items_with_lek_is_not_eof() {
    let key = json!({"id":{"N":"10001"}});
    let continued_key = key.clone();
    let server = Server::start(move |wire| {
        if wire.body.get("ExclusiveStartKey").is_none() {
            json!({"Items":[],"LastEvaluatedKey":continued_key})
        } else {
            assert_eq!(wire.body["ExclusiveStartKey"], continued_key);
            json!({"Items":[{"id":{"N":"10002"}}],"LastEvaluatedKey":{}})
        }
    });
    let db = server.db("dynamodb").await;
    let first = db.export_page("us-east-1", "records", None, 1000).await.unwrap();
    assert!(first.data.rows.is_empty());
    let token = first.next.expect("empty evaluated page must preserve LEK");
    assert_eq!(scan_start_key(&token, "records").unwrap(), key);
    let second = db.export_page("us-east-1", "records", Some(&token), 1000).await.unwrap();
    assert_eq!(second.data.rows.len(), 1);
    assert!(second.next.is_none());
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn native_export_limit_zero_never_reads_data() {
    let server = Server::start(|_| panic!("limit=0 must not issue an HTTP request"));
    for kind in ["dynamodb", "etcd"] {
        let db = server.db(kind).await;
        let page = db.export_page("/", "records", None, 0).await.unwrap();
        assert!(page.data.rows.is_empty());
        assert!(page.next.is_none());
        if kind == "etcd" { assert_eq!(page.data.columns, etcd_export_columns()); }
    }
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn etcd_exports_only_named_key_and_preserves_binary_value() {
    let server = Server::start(|wire| {
        assert_eq!(wire.path, "/v3/kv/range");
        assert_eq!(wire.body, json!({"key":b64("/应用/键".as_bytes()),"limit":"1"}));
        json!({"header":{"revision":"42"},"kvs":[{
            "key":b64("/应用/键".as_bytes()),"value":b64(&[0,255,128]),
            "version":"3","mod_revision":"42"
        }]})
    });
    let db = server.db("etcd").await;
    let page = db.export_page("/", "/应用/键", None, 1000).await.unwrap();
    assert_eq!(page.data.columns, etcd_export_columns());
    assert_eq!(page.data.rows, vec![vec![Some("/应用/键".into()), Some("base64:AP+A".into()), Some("3".into()), Some("42".into())]]);
    assert!(page.next.is_none());
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn etcd_binary_catalog_key_and_absent_key_have_no_fake_history() {
    let server = Server::start(|wire| {
        assert_eq!(wire.body["key"], b64(&[255,0,128]));
        assert!(wire.body.get("range_end").is_none());
        json!({"header":{"revision":"42"},"kvs":[]})
    });
    let db = server.db("etcd").await;
    let object = format!("base64:{}", b64(&[255,0,128]));
    let page = db.export_page("/", &object, None, 1000).await.unwrap();
    assert!(page.data.rows.is_empty());
    assert!(page.next.is_none());
    assert_eq!(page.data.columns, etcd_export_columns());
}

#[tokio::test]
async fn dynamo_rejects_nonadvancing_or_wrong_table_cursor() {
    let key = json!({"id":{"N":"10001"}});
    let reply_key = key.clone();
    let server = Server::start(move |_| json!({"Items":[],"LastEvaluatedKey":reply_key}));
    let db = server.db("dynamodb").await;
    let token = scan_next_token("records", Some(&key)).unwrap().unwrap();
    let error = db.export_page("us-east-1", "records", Some(&token), 1000).await.err().unwrap();
    assert!(error.to_string().contains("未推进"));
    assert!(db.export_page("us-east-1", "other", Some(&token), 1000).await.is_err());
    assert_eq!(server.requests().len(), 1, "wrong-table token rejected before network");
    assert!(scan_next_token("records", Some(&Value::Null)).is_err());
    assert!(scan_start_key(&"x".repeat(EXPORT_TOKEN_MAX_BYTES + 1), "records").is_err());
    assert!(scan_start_key(r#"{"table":"records","key":{}}"#, "records").is_err());
}

#[tokio::test]
async fn etcd_rejects_mismatched_key_or_missing_value() {
    for missing_value in [false, true] {
        let server = Server::start(move |_| {
            let mut kv = json!({"key":b64(b"requested"),"value":b64(b"value")});
            if missing_value { kv.as_object_mut().unwrap().remove("value"); }
            else { kv["key"] = Value::String(b64(b"different")); }
            json!({"header":{},"kvs":[kv]})
        });
        let db = server.db("etcd").await;
        let error = db.export_page("/", "requested", None, 1000).await.err().unwrap();
        assert!(error.to_string().contains(if missing_value { "缺少 base64 value" } else { "不同 key" }), "{error}");
    }
}
