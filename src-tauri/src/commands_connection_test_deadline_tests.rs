//! Focused deadline tests: no external database, driver, or credentials required.
use super::*;
use std::{sync::atomic::{AtomicBool, Ordering}, time::Duration};

fn request(kind: &str) -> ConnectionRequest {
    ConnectionRequest {
        kind: DbKind::of(kind), host: "127.0.0.1".into(), port: 0,
        user: "test".into(), password: String::new(), database: String::new(),
        file: ":memory:".into(), params: String::new(), timeout_secs: 1,
    }
}

#[tokio::test]
async fn blocking_worker_does_not_block_deadline_and_finishes_after_timeout() {
    let finished = Arc::new(AtomicBool::new(false));
    let worker_finished = finished.clone();
    let (release, wait) = std::sync::mpsc::channel();
    let (completed, completion) = tokio::sync::oneshot::channel();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(50);
    // This deliberately has no await before blocking, just like SQLDriverConnect.
    let result = tokio::time::timeout(Duration::from_secs(1), run_connection_test(deadline, async move {
        wait.recv().unwrap();
        worker_finished.store(true, Ordering::Release);
        let _ = completed.send(());
        Ok(connection_success("late result"))
    })).await;
    // Always release the worker even if an assertion below fails.
    release.send(()).unwrap();
    let result = result.expect("a blocking driver must not stall the IPC deadline").unwrap();
    assert!(!result.ok);
    assert!(result.message.contains("CONNECT_TIMEOUT"));
    tokio::time::timeout(Duration::from_secs(1), completion).await.unwrap().unwrap();
    assert!(finished.load(Ordering::Acquire), "timeout must not abort the resource owner");
}

#[tokio::test]
async fn asynchronous_probe_uses_the_same_overall_deadline() {
    let (release, wait) = tokio::sync::oneshot::channel();
    let (completed, completion) = tokio::sync::oneshot::channel();
    let result = run_connection_test(tokio::time::Instant::now() + Duration::from_millis(30), async move {
        let _ = wait.await;
        let _ = completed.send(());
        Ok(connection_success("late metadata"))
    }).await.unwrap();
    assert!(!result.ok);
    assert!(result.message.contains("CONNECT_TIMEOUT"));
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), completion).await.unwrap().unwrap();
}

#[tokio::test]
async fn empty_password_and_unresponsive_mysql_handshake_return_within_budget() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut req = request("mysql");
    req.port = listener.local_addr().unwrap().port();
    let (release, wait) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        let _ = wait.await;
    });
    let result = tokio::time::timeout(Duration::from_secs(3), test_connection(req)).await;
    let _ = release.send(());
    server.await.unwrap();
    let result = result.expect("silent server must not leave test pending").unwrap();
    assert!(!result.ok);
    assert!(!result.message.is_empty());
}

#[tokio::test]
async fn empty_password_authentication_error_is_not_reported_as_timeout() {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut req = request("mysql");
    req.port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        // MySQL permits an initial ERR packet instead of the normal greeting.
        let mut payload = vec![0xff, 0x15, 0x04]; // ER_ACCESS_DENIED_ERROR (1045).
        payload.extend_from_slice(b"#28000Access denied for user test (using password: NO)");
        let len = payload.len();
        socket.write_all(&[len as u8, (len >> 8) as u8, (len >> 16) as u8, 0]).await.unwrap();
        socket.write_all(&payload).await.unwrap();
    });
    let result = tokio::time::timeout(Duration::from_secs(2), test_connection(req)).await.unwrap().unwrap();
    server.await.unwrap();
    assert!(!result.ok);
    assert!(result.message.contains("Access denied"), "{}", result.message);
    assert!(!result.message.contains("CONNECT_TIMEOUT"));
}

#[tokio::test]
async fn sqlite_success_still_returns_metadata() {
    let result = test_connection(request("sqlite")).await.unwrap();
    assert!(result.ok, "{}", result.message);
    assert!(!result.version.is_empty());
    assert_eq!(result.charset, "UTF-8");
}
