//! Actual official all-JAR and OAuth HTTP transport, not Google-server/TLS certification.
use super::*;

fn request() -> ConnectionRequest {
    ConnectionRequest {
        kind: crate::models::DbKind::of("bigquery"), host: "127.0.0.1".into(), port: 1,
        user: "token".into(), password: "LOCAL_TEST_TOKEN_NOT_A_GOOGLE_CREDENTIAL.123-abc".into(),
        database: "probe-project/probe_dataset".into(), file: String::new(),
        params: "ssl=false".into(), timeout_secs: 5,
    }
}

#[test]
fn bigquery_url_never_absorbs_credentials_or_arbitrary_endpoints() {
    let mut req = request();
    let v = vendor("bigquery").unwrap();
    let url = connection_url(&req, &v, &params(&v, &req).unwrap().unwrap()).unwrap();
    assert!(url.contains("EndpointOverrides=BIGQUERY=http://127.0.0.1:1,OAUTH2=http://127.0.0.1:1/token,READ_API=127.0.0.1:1,STS=http://127.0.0.1:1/sts"));
    assert!(url.contains(";OAuthType=2;OAuthAccessTokenReadonly=0"));
    assert!(url.contains(";LogLevel=0;"));
    assert!(!url.contains(&req.password));
    for host in ["localhost", "evil.example", "bigquery.googleapis.com.evil.example"] {
        req.host = host.into();
        assert!(connection_url(&req, &v, &params(&v, &req).unwrap().unwrap()).is_err());
    }
    req.host = "bigquery.googleapis.com".into(); req.port = 443; req.params.clear();
    let cloud = connection_url(&req, &v, &[]).unwrap();
    assert!(cloud.starts_with("jdbc:bigquery://bigquery.googleapis.com:443;"));
    assert!(!cloud.contains("EndpointOverrides")); assert!(!cloud.contains(&req.password));
    req.params = "ssl=false".into();
    assert!(connection_url(&req, &v, &params(&v, &req).unwrap().unwrap()).is_err());
    req.host = "127.0.0.1".into(); req.params.clear();
    assert!(connection_url(&req, &v, &[]).is_err());
    req.params = "ssl=false".into();
    for bad in ["bad;LogLevel=8", "has space", "has\nnewline", "has,comma"] {
        req.password = bad.into();
        assert!(connection_url(&req, &v, &params(&v, &req).unwrap().unwrap()).is_err());
    }
    req.password = "LOCAL_SYNTHETIC_TOKEN".into(); req.user.clear();
    assert!(connection_url(&req, &v, &[]).is_err());
    req.user = "token".into(); req.database = "probe-project;EndpointOverrides=x".into();
    assert!(connection_url(&req, &v, &[]).is_err());
    for option in ["LogLevel=8", "LogPath=stolen", "EndpointOverrides=BIGQUERY=https://evil.example"] {
        req.params = option.into();
        assert!(params(&v, &req).unwrap().is_none(), "custom settings must not be silently absorbed");
    }
}

fn stage_original(root: &Path) -> PathBuf {
    let v = vendor("bigquery").unwrap();
    let dir = package_dir_for(&v); fs::create_dir_all(&dir).unwrap();
    fs::copy(root.join("cloud-iris-20261107/artifacts/google-cloud-bigquery-jdbc-1.9.0-all.jar"), dir.join("driver.jar")).unwrap();
    fs::copy(root.join("cloud-final-20261107/evidence/google-cloud-bigquery-jdbc-1.9.0.pom"), dir.join("official.pom")).unwrap();
    fs::write(dir.join("manifest.json"), serde_json::to_vec(&Manifest {
        format: 1, kind: v.kind.into(), version: v.version.into(), class: v.class.into(),
    }).unwrap()).unwrap();
    let jar = verify_package(&dir, &v).unwrap();
    assert_eq!(digest_file(&jar).unwrap(), v.jar.sha256);
    jar
}

struct RejectionServer {
    port: u16,
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl RejectionServer {
    fn start(secret: String, status: u16) -> Self {
        use std::{io::{Read, Write}, net::TcpListener, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}, time::Instant};
        assert!(matches!(status, 401 | 403));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stopped = Arc::new(AtomicBool::new(false)); let stop = stopped.clone();
        let seen = Arc::new(Mutex::new(Vec::new())); let capture = seen.clone();
        let thread = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(25);
            while !stop.load(Ordering::SeqCst) && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                        stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                        let mut bytes = Vec::new(); let mut buf = [0u8; 4096];
                        let mut expected = None;
                        while bytes.len() < 128 * 1024 {
                            match stream.read(&mut buf) {
                                Ok(0) | Err(_) => break,
                                Ok(n) => {
                                    bytes.extend_from_slice(&buf[..n]);
                                    if expected.is_none() {
                                        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                                            let headers = String::from_utf8_lossy(&bytes[..end]);
                                            let size = headers.lines().find_map(|line| {
                                                let (key, value) = line.split_once(':')?;
                                                key.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().ok()).flatten()
                                            }).unwrap_or(0);
                                            expected = Some(end + 4 + size);
                                        }
                                    }
                                    if expected.is_some_and(|size| bytes.len() >= size) { break; }
                                }
                            }
                        }
                        capture.lock().unwrap().push(String::from_utf8_lossy(&bytes).into_owned());
                        let reason = if status == 401 { "authError" } else { "accessDenied" };
                        let body = serde_json::json!({"error": {
                            "code": status, "message": format!("Synthetic loopback denial {secret}"),
                            "errors": [{"reason": reason, "message": format!("Synthetic loopback denial {secret}")}],
                            "status": if status == 401 { "UNAUTHENTICATED" } else { "PERMISSION_DENIED" },
                        }}).to_string();
                        let label = if status == 401 { "Unauthorized" } else { "Forbidden" };
                        let response = format!("HTTP/1.1 {status} {label}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                        let _ = stream.write_all(response.as_bytes());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(5)),
                    Err(_) => break,
                }
            }
        });
        Self { port, stopped, seen, thread: Some(thread) }
    }
}
impl Drop for RejectionServer {
    fn drop(&mut self) {
        self.stopped.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(thread) = self.thread.take() { thread.join().unwrap(); }
    }
}
struct LogEnvironment;
impl Drop for LogEnvironment {
    fn drop(&mut self) {
        std::env::remove_var("BIGQUERY_JDBC_LOG_LEVEL");
        std::env::remove_var("BIGQUERY_JDBC_LOG_PATH");
    }
}

#[tokio::test]
#[ignore = "original BigQuery all-JAR and Java; copied isolated test executable only"]
async fn bigquery_original_package_oauth_properties_lazy_query_and_log_suppression_are_real() {
    let portable = crate::storage::portable_root();
    assert!(portable.starts_with(std::env::temp_dir()) && portable.file_name().unwrap().to_string_lossy().starts_with("navkit-jdbc-command-tests-"));
    let root = PathBuf::from(std::env::var_os("NAVKIT_JDBC_RESEARCH_ROOT").expect("original official research artifacts"));
    let jar = stage_original(&root); let v = vendor("bigquery").unwrap();
    let ready = status_at(&v, jar.parent().unwrap(), "bigquery");
    assert_eq!(ready.status, "ready", "{}", ready.message);
    assert_eq!(std::env::split_paths(&classpath_for(&jar).unwrap()).count(), 1);
    let forbidden_log = portable.join("forbidden-inherited-driver-logs");
    // Only this unique copied process gets these environment variables.
    std::env::set_var("BIGQUERY_JDBC_LOG_LEVEL", "8");
    std::env::set_var("BIGQUERY_JDBC_LOG_PATH", &forbidden_log);
    let _environment = LogEnvironment;
    for status in [401, 403] {
        let mut req = request(); let server = RejectionServer::start(req.password.clone(), status); req.port = server.port;
        let route = resolve_default(&req).unwrap().expect("actual default JDBC route");
        assert_eq!(route.target, jar.to_string_lossy()); assert_eq!(route.driver_class, v.class);
        assert!(!route.url.contains(&req.password));
        let result = tokio::time::timeout(Duration::from_secs(20), crate::db::open(&req)).await.expect("bounded actual query rejection");
        let error = match result {
            Ok(_) => panic!("a lazy BigQuery connection must not pass when its real query is denied"),
            Err(error) => format!("{error:#}"),
        };
        assert!(error.contains("JDBC"), "{error}");
        if status == 401 {
            assert!(error.contains("BigQuery OAuth authentication [28000]") && error.contains("replace the token and reconnect"), "{error}");
        } else {
            assert!(error.contains("Synthetic loopback denial") && error.contains("[REDACTED]"), "{error}");
        }
        for bad in [&req.password, "ClassNotFoundException", "NoClassDefFoundError", "无效 JSON", "sidecar 已退出"] {
            assert!(!error.contains(bad), "secret/driver/protocol failure escaped");
        }
        assert!(!forbidden_log.exists(), "inherited log path must not create any driver log");
        let captured = server.seen.lock().unwrap();
        assert!(!captured.is_empty(), "actual driver must issue a request, not just create a lazy Connection");
        assert!(captured.iter().any(|request| request.lines().any(|line| line.eq_ignore_ascii_case(&format!("authorization: Bearer {}", req.password)))), "actual OAuth Authorization header missing");
        assert!(captured.iter().any(|request| request.starts_with("POST ") && request.contains("/queries")), "real BigQuery REST query missing");
        assert!(captured.iter().all(|request| !request.lines().next().unwrap_or("").contains(&req.password)), "token entered request URL");
        println!("bigquery HTTP {status}: actual OAuth header/query and failed connection verified; credential redacted; driver logs absent");
        drop(captured); drop(server);
    }
    assert_eq!(digest_file(&jar).unwrap(), v.jar.sha256, "original all-JAR is unchanged");
    println!("bigquery: complete original package/runtime/default JDBC OAuth properties/real lazy-query HTTP denial/redaction/inherited-log suppression PASS; server and TLS UNVERIFIED");
}
