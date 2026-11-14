//! ClickHouse 导出的端到端测试。
//!
//! 用真实的 `ClickHouseClient` 连接一个本机 `TcpListener` 假服务器，经
//! `run_export_task` 走完整的导出、发布与失败清理路径；不连接任何用户数据库。
//! 假服务器按 SQL 返回 `JSONCompactEachRowWithNamesAndTypes` 响应，小响应使用
//! 1~7 字节的 chunked 分片，专门覆盖 UTF-8 多字节字符被网络分片切断的情况。

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::clickhouse::ClickHouseClient;
use crate::db::Db;
use crate::models::{ConnectionRequest, DbKind, ExportOptions};
use crate::transfer::{run_export_task, ExportTaskOptions};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("navkit-ch-e2e-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
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

/// 本机 ClickHouse HTTP 假服务器。每个连接串行处理一次请求，响应后关闭连接。
struct MockClickHouse {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    requests: Arc<Mutex<Vec<String>>>,
}

impl MockClickHouse {
    fn new(ddl: &str, columns: &[&str], types: &[&str], rows: &[String]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();

        let names_line = serde_json::to_string(&columns.iter().map(|s| (*s).to_string()).collect::<Vec<_>>()).unwrap();
        let types_line = serde_json::to_string(&types.iter().map(|s| (*s).to_string()).collect::<Vec<_>>()).unwrap();
        let describe = format!("{}\n{}\n", names_line, types_line).into_bytes();
        let mut data = describe.clone();
        for row in rows {
            data.extend_from_slice(row.as_bytes());
            data.push(b'\n');
        }

        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let thread_stop = stop.clone();
        let thread_requests = requests.clone();
        let ddl = ddl.to_string();
        let handle = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        // 监听器是非阻塞的；accept 得到的连接必须显式恢复阻塞模式。
                        let _ = socket.set_nonblocking(false);
                        let _ = socket.set_read_timeout(Some(Duration::from_secs(10)));
                        let _ = socket.set_write_timeout(Some(Duration::from_secs(10)));
                        let _ = handle_connection(&mut socket, &ddl, &describe, &data, thread_requests.as_ref());
                    }
                    Err(ref error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });
        Self { addr, stop, handle: Some(handle), requests }
    }

    fn client(&self) -> ClickHouseClient {
        ClickHouseClient::connect(&ConnectionRequest {
            kind: DbKind::of("clickhouse"),
            host: "127.0.0.1".into(),
            port: self.addr.port(),
            user: String::new(),
            password: String::new(),
            database: "default".into(),
            file: String::new(),
            params: String::new(),
            timeout_secs: 10,
        })
        .unwrap()
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for MockClickHouse {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|window| window == b"\r\n\r\n")
}

fn header_value(head: &str, name: &str) -> Option<String> {
    head.lines().skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim().to_string())
    })
}

fn read_request(socket: &mut TcpStream) -> std::io::Result<Option<Vec<u8>>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        let read = socket.read(&mut chunk)?;
        if read == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..read]);
        if let Some(position) = find_header_end(&buf) {
            break position;
        }
        if buf.len() > 1024 * 1024 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "请求头过大"));
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let content_length = header_value(&head, "content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < content_length {
        let read = socket.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);
    Ok(Some(body))
}

fn response_for(sql: &str, ddl: &str, describe: &[u8], data: &[u8]) -> Vec<u8> {
    let upper = sql.trim().to_ascii_uppercase();
    if upper.starts_with("SHOW CREATE TABLE") {
        let statement = serde_json::to_string(ddl).unwrap();
        // JSONCompactEachRowWithNamesAndTypes 的每一行都必须是 JSON 数组：
        // 第三行也是数据行，必须写成 [string]，不能直接塞一个裸 JSON 字符串。
        return format!("[\"statement\"]\n[\"String\"]\n[{}]\n", statement).into_bytes();
    }
    if upper.starts_with("SELECT") {
        // LIMIT 0 只读列名/类型；其余 SELECT 才是真正读取数据行。
        return if upper.contains("LIMIT 0") { describe.to_vec() } else { data.to_vec() };
    }
    b"mock: unexpected query\n".to_vec()
}

fn handle_connection(
    socket: &mut TcpStream,
    ddl: &str,
    describe: &[u8],
    data: &[u8],
    requests: &Mutex<Vec<String>>,
) -> std::io::Result<()> {
    let Some(body) = read_request(socket)? else { return Ok(()) };
    let body = String::from_utf8_lossy(&body);
    let sql = body.lines().next().unwrap_or("").trim().to_string();
    requests.lock().unwrap().push(sql.clone());
    let response = response_for(&sql, ddl, describe, data);
    if response.len() <= 256 * 1024 {
        socket.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        )?;
        let sizes = [1usize, 2, 3, 5, 7, 4];
        let mut offset = 0;
        let mut index = 0;
        while offset < response.len() {
            let size = sizes[index % sizes.len()].min(response.len() - offset);
            socket.write_all(format!("{:X}\r\n", size).as_bytes())?;
            socket.write_all(&response[offset..offset + size])?;
            socket.write_all(b"\r\n")?;
            offset += size;
            index += 1;
        }
        socket.write_all(b"0\r\n\r\n")?;
    } else {
        socket.write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response.len()
            )
            .as_bytes(),
        )?;
        socket.write_all(&response)?;
    }
    socket.flush()
}

fn options(file: String, format: &str, limit: Option<u64>) -> ExportOptions {
    ExportOptions {
        connection_id: "mock".into(),
        schema: "default".into(),
        table: "events".into(),
        file,
        format: format.into(),
        has_header: true,
        delimiter: ",".into(),
        encoding: "utf-8".into(),
        where_clause: None,
        limit,
    }
}

fn task_options(options: ExportOptions) -> ExportTaskOptions {
    ExportTaskOptions {
        sql_batch_rows: 0, sql_batch_bytes: 0,
        options,
        task_id: format!("ch-e2e-{}", uuid::Uuid::new_v4().simple()),
    }
}

fn is_data_select(sql: &str) -> bool {
    let upper = sql.trim().to_ascii_uppercase();
    upper.starts_with("SELECT") && !upper.contains("LIMIT 0")
}

#[tokio::test]
async fn json_csv_sql_exports_preserve_utf8_and_large_numbers_over_chunked_http() {
    let dir = TempDir::new();
    let mock = MockClickHouse::new(
        "CREATE TABLE default.events (`id` UInt64, `name` String, `amount` Decimal(38, 0), `note` Nullable(String)) ENGINE = MergeTree ORDER BY id",
        &["id", "name", "amount", "note"],
        &["UInt64", "String", "Decimal(38, 0)", "Nullable(String)"],
        &[
            r#"[18446744073709551615,"中文-🚀","12345678901234567890123456789012345678",null]"#.to_string(),
            r#"[0,"x","0","ok"]"#.to_string(),
        ],
    );
    let db = Db::ClickHouse(mock.client());
    let kind = DbKind::of("clickhouse");

    // JSON：必须读回合法 JSON，换行是真实换行而不是字面量反斜杠 n。
    let json_path = dir.file("events.json");
    let count = run_export_task(&db, &kind, &task_options(options(json_path.clone(), "json", None)))
        .await
        .unwrap();
    assert_eq!(count, 2);
    let text = std::fs::read_to_string(&json_path).unwrap();
    assert!(text.starts_with("[\n"), "JSON 开头必须是真实换行: {text:?}");
    assert!(text.ends_with("\n]\n"), "JSON 结尾必须是真实换行: {text:?}");
    assert!(!text.contains("\\n"), "JSON 导出不能把换行写成字面量反斜杠 n: {text:?}");
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    let rows = value.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    // 假服务器按 JSONCompact 返回裸十进制 18446744073709551615（UInt64::MAX），
    // 导出 JSON 时不得舍入或截断；这里同时容忍导出层把单元格落成 JSON 字符串，
    // 或保留为 JSON number（serde 的 u64 可无损表示 UInt64 最大值）。
    let id_text = match &rows[0]["id"] {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Number(number) => number.to_string(),
        other => panic!("id 必须保留 UInt64 的十进制值: {other:?}"),
    };
    assert_eq!(id_text, "18446744073709551615");
    assert_eq!(rows[0]["name"].as_str(), Some("中文-🚀"));
    let amount_text = match &rows[0]["amount"] {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Number(number) => number.to_string(),
        other => panic!("amount 必须保留 38 位十进制值: {other:?}"),
    };
    assert_eq!(amount_text, "12345678901234567890123456789012345678");
    assert_eq!(rows[0]["note"], serde_json::Value::Null);
    assert_eq!(rows[1]["name"].as_str(), Some("x"));

    // CSV：NULL 编码为 \N，UTF-8 与 38 位十进制不被截断/舍入。
    let csv_path = dir.file("events.csv");
    let count = run_export_task(&db, &kind, &task_options(options(csv_path.clone(), "csv", None)))
        .await
        .unwrap();
    assert_eq!(count, 2);
    assert_eq!(
        std::fs::read_to_string(&csv_path).unwrap(),
        "id,name,amount,note\r\n18446744073709551615,中文-🚀,12345678901234567890123456789012345678,\\N\r\n0,x,0,ok\r\n"
    );

    // SQL：数值保持裸字面量，字符串正确引用，NULL 保持 NULL。
    let sql_path = dir.file("events.sql");
    let count = run_export_task(&db, &kind, &task_options(options(sql_path.clone(), "sql", None)))
        .await
        .unwrap();
    assert_eq!(count, 2);
    let sql = std::fs::read_to_string(&sql_path).unwrap();
    let lines: Vec<&str> = sql.lines().collect();
    assert_eq!(lines.len(), 2, "每行必须是一条独立 INSERT: {sql:?}");
    assert_eq!(
        lines[0],
        "INSERT INTO `default`.`events` (`id`, `name`, `amount`, `note`) VALUES (18446744073709551615, '中文-🚀', 12345678901234567890123456789012345678, NULL);"
    );
    assert_eq!(
        lines[1],
        "INSERT INTO `default`.`events` (`id`, `name`, `amount`, `note`) VALUES (0, 'x', 0, 'ok');"
    );
}

#[tokio::test]
async fn view_sql_and_limit_zero_never_request_data_rows() {
    let dir = TempDir::new();
    let view_ddl = "CREATE MATERIALIZED VIEW default.mv ENGINE = Memory AS SELECT 1 AS id";
    let mock = MockClickHouse::new(
        view_ddl,
        &["id"],
        &["UInt64"],
        &[r#"[1]"#.to_string()],
    );
    let db = Db::ClickHouse(mock.client());
    let kind = DbKind::of("clickhouse");

    // SQL 导出视图：只输出真实 DDL，不向视图请求数据行。
    let view_path = dir.file("view.sql");
    let count = run_export_task(&db, &kind, &task_options(options(view_path.clone(), "sql", None)))
        .await
        .unwrap();
    assert_eq!(count, 0);
    let text = std::fs::read_to_string(&view_path).unwrap();
    assert!(text.contains(view_ddl), "必须保留真实视图定义: {text:?}");
    assert!(!text.contains("INSERT INTO"), "视图 SQL 不得生成数据 INSERT: {text:?}");
    assert!(
        mock.requests().iter().all(|sql| !is_data_select(sql)),
        "视图 SQL 导出不得发起数据 SELECT: {:?}",
        mock.requests()
    );

    // limit 0：建立空文件，但绝不发起数据 SELECT。
    let empty_path = dir.file("empty.json");
    let count = run_export_task(&db, &kind, &task_options(options(empty_path.clone(), "json", Some(0))))
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(std::fs::read_to_string(&empty_path).unwrap(), "[\n\n]\n");
    let requests = mock.requests();
    assert!(
        requests.iter().all(|sql| !is_data_select(sql)),
        "limit 0 不得发起数据 SELECT: {requests:?}"
    );
    assert!(
        requests.iter().any(|sql| sql.contains("LIMIT 0")),
        "limit 0 仍应读取列元数据: {requests:?}"
    );
}

#[tokio::test]
async fn oversized_record_fails_without_publishing_or_leaving_temp_files() {
    let dir = TempDir::new();
    let huge = "x".repeat(16 * 1024 * 1024 + 1);
    let row = format!(r#"[1,"{}"]"#, huge);
    let mock = MockClickHouse::new(
        "CREATE TABLE default.events (`id` UInt64, `payload` String) ENGINE = Memory",
        &["id", "payload"],
        &["UInt64", "String"],
        &[row],
    );
    let db = Db::ClickHouse(mock.client());
    let kind = DbKind::of("clickhouse");
    let target = dir.file("big.csv");

    let result = run_export_task(&db, &kind, &task_options(options(target.clone(), "csv", None))).await;
    let error = format!("{}", result.unwrap_err());
    assert!(error.contains("超过上限"), "应因单条记录超限失败: {error}");
    // 必须真的发出了数据 SELECT 并在解析该行时触发上限，而不是元数据阶段提前失败。
    let requests = mock.requests();
    assert!(
        requests.iter().any(|sql| is_data_select(sql)),
        "必须实际读取数据行后才触发单条记录超限: {requests:?}"
    );
    assert!(!Path::new(&target).exists(), "失败时不得发布目标文件");

    let leftovers: Vec<String> = std::fs::read_dir(&dir.path)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    assert!(leftovers.is_empty(), "失败后不得残留目标或临时文件: {leftovers:?}");
}
