//! Wire-simulated ClickHouse covers the generic path and its real SQL history hooks.
//! This is not a live ClickHouse vendor/performance certification.
use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn page_generic_metadata_and_typed_filter_never_count_when_disabled() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let wire = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let captured = wire.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0u8; 4096];
            let (start, length) = loop {
                let n = stream.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let length = headers.lines().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
                    }).unwrap();
                    break (end + 4, length);
                }
            };
            while bytes.len() < start + length {
                let n = stream.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
            }
            let sql = String::from_utf8(bytes[start..start + length].to_vec()).unwrap();
            captured.lock().unwrap().push(sql.clone());
            let upper = sql.to_uppercase();
            let body = if upper.starts_with("DESCRIBE TABLE") {
                "[\"name\",\"type\",\"default_type\",\"default_expression\",\"comment\"]\n[\"String\",\"String\",\"String\",\"String\",\"String\"]\n[\"id\",\"Int64\",\"\",\"\",\"\"]\n[\"name\",\"Nullable(String)\",\"\",\"\",\"\"]\n"
            } else if upper.contains("COUNT(") {
                "[\"count\"]\n[\"UInt64\"]\n[3]\n"
            } else if upper.contains("1=0") {
                "[\"id\",\"name\"]\n[\"Int64\",\"Nullable(String)\"]\n"
            } else {
                assert!(upper.starts_with("SELECT * FROM"), "{sql}");
                "[\"id\",\"name\"]\n[\"Int64\",\"Nullable(String)\"]\n[1,\"alpha\"]\n[2,null]\n[3,\"gamma\"]\n"
            };
            let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });
    let f = Fixture::new().await;
    let connection = crate::models::ConnectionRequest {
        kind: DbKind::of("clickhouse"), host: "127.0.0.1".into(), port,
        user: "default".into(), password: String::new(), database: "main".into(),
        file: String::new(), params: String::new(), timeout_secs: 5,
    };
    let db = Db::ClickHouse(crate::clickhouse::ClickHouseClient::connect(&connection).unwrap());
    let context = Context { path: Some(f.path.clone()), kind: "clickhouse".into(), ..Context::default() };
    crate::sql_history::scope(context, async {
        let metadata = crate::schema::table_schema_for_page(&db, connection.kind.clone(), "main", "items").await.unwrap();
        assert_eq!(metadata.columns.len(), 2);
        assert_eq!(metadata.row_count, None);
        let ddl = crate::schema::table_ddl(&db, connection.kind.clone(), "main", "items").await.unwrap();
        assert_eq!(ddl, "CREATE TABLE `main`.`items` (\n  `id` Int64 NOT NULL,\n  `name` String\n)");
        let mut req = request("items", 0, 2, false);
        req.filters = serde_json::from_value(serde_json::json!([{"conds":[{"column":"id","operator":">","value":"0"}]}])).unwrap();
        let result = crate::schema::page_table(&db, connection.kind.clone(), &req).await.unwrap();
        assert_eq!((result.rows.len(), result.total, result.total_exact, result.has_more), (2, 3, false, Some(true)));
        assert_eq!(result.rows[1][1], None);
        let logs = no_count(&f);
        assert!(logs.iter().any(|e| e.sql.starts_with("DESCRIBE TABLE")));
        assert!(logs.iter().any(|e| e.sql.starts_with("SELECT * FROM") && e.sql.contains("WHERE") && e.sql.contains("LIMIT 3 OFFSET 0") && e.rows == 3));
        assert!(wire.lock().unwrap().iter().all(|sql| !sql.to_uppercase().contains("COUNT(")));
        // Legacy full schema keeps its row count; the optional fast path is not a
        // global change to designers or other table_schema consumers.
        crate::storage::clear_sql_log_at(&f.path);
        let legacy = crate::schema::table_schema(&db, connection.kind.clone(), "main", "items").await.unwrap();
        assert_eq!(legacy.row_count, Some(3));
        assert_eq!(f.logs().iter().filter(|e| e.sql.to_uppercase().contains("COUNT(")).count(), 1);
    }).await;
    server.abort();
    let _ = server.await;
}
