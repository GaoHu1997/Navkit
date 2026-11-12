//! Transport ownership tests use this test executable as a private stdin/stdout
//! fixture, so Rust regression tests require neither Node nor an H2/Java install.
use super::*;
use crate::http_db::{HttpAdapter, HttpRows};
use std::io::Write;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

#[test]
#[ignore = "private subprocess fixture; executed only by the ownership tests below"]
fn driver_transport_fixture() {
    let mode = std::env::var("NAVKIT_PL20_TRANSPORT_FIXTURE").expect("private fixture mode");
    // libtest writes `test <name> ... ` without a newline before calling us.
    // Put readiness on its own line; don't confuse that harness prefix with a
    // missing/partial driver response or relax the exact readiness check.
    println!("\nNAVKIT_PL20_FIXTURE_READY"); std::io::stdout().flush().unwrap();
    let mut request = String::new(); std::io::stdin().read_line(&mut request).unwrap();
    match mode.as_str() {
        "partial" => { print!("partial"); std::io::stdout().flush().unwrap(); std::process::exit(0); }
        "invalid-utf8" => { std::io::stdout().write_all(&[0xff, b'\n']).unwrap(); std::io::stdout().flush().unwrap(); std::process::exit(0); }
        "complete" => { println!("{{}}"); std::io::stdout().flush().unwrap(); }
        "silent" => { }
        _ => panic!("unknown private fixture mode"),
    }
    // A managed fixture waits without CPU polling; every caller owns and kills it.
    loop { std::thread::park(); }
}

async fn fixture(mode: &str) -> (Child, Option<ChildStdin>, BufReader<ChildStdout>) {
    let mut child = crate::background_process::tokio_command(std::env::current_exe().unwrap())
        .args(["--exact", "http_db::relational::transport_tests::driver_transport_fixture", "--ignored", "--nocapture", "--test-threads=1"])
        .env("NAVKIT_PL20_TRANSPORT_FIXTURE", mode)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).kill_on_drop(true).spawn().unwrap();
    let input = child.stdin.take();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let mut line = String::new();
            assert_ne!(output.read_line(&mut line).await.unwrap(), 0, "private fixture exited before admission");
            if line.trim() == "NAVKIT_PL20_FIXTURE_READY" { break; }
        }
    }).await.expect("private fixture startup deadline");
    (child, input, output)
}
async fn stopped(child: &mut Child) {
    tokio::time::timeout(Duration::from_secs(5), child.wait()).await.expect("discarded sidecar must terminate").unwrap();
}

#[tokio::test]
async fn timeout_discards_physical_connection_and_never_accepts_a_second_request() {
    let (mut child, mut input, mut output) = fixture("silent").await;
    let error = exchange_line(&mut child, &mut input, &mut output, "first\n", Duration::from_millis(30)).await.unwrap_err();
    assert!(error.to_string().contains("timed out")); assert!(input.is_none());
    stopped(&mut child).await;
    let error = exchange_line(&mut child, &mut input, &mut output, "second\n", Duration::from_secs(1)).await.unwrap_err();
    assert!(error.to_string().contains("closed"), "a cancelled write cannot be replayed on the old lease");
}
#[tokio::test]
async fn externally_dropped_transport_future_owns_sidecar_destruction() {
    let (mut child, mut input, mut output) = fixture("silent").await;
    let cancelled = tokio::time::timeout(Duration::from_millis(30), exchange_line(
        &mut child, &mut input, &mut output, "first\n", Duration::from_secs(10))).await;
    assert!(cancelled.is_err()); assert!(input.is_none()); stopped(&mut child).await;
}
#[tokio::test]
async fn eof_partial_and_invalid_utf8_frames_cannot_poison_next_request() {
    for mode in ["partial", "invalid-utf8"] {
        let (mut child, mut input, mut output) = fixture(mode).await;
        assert!(exchange_line(&mut child, &mut input, &mut output, "first\n", Duration::from_secs(3)).await.is_err());
        assert!(input.is_none(), "{mode} must discard admission, not leave a stale stdout response");
        stopped(&mut child).await;
    }
}
#[tokio::test]
async fn a_complete_frame_keeps_a_clean_physical_connection() {
    let (mut child, mut input, mut output) = fixture("complete").await;
    assert_eq!(exchange_line(&mut child, &mut input, &mut output, "first\n", Duration::from_secs(3)).await.unwrap(), "{}\n");
    assert!(input.is_some()); child.start_kill().unwrap(); stopped(&mut child).await;
}

struct OrdinaryRest;
#[async_trait::async_trait]
impl HttpAdapter for OrdinaryRest {
    async fn ping(&self) -> anyhow::Result<String> { Ok("ordinary REST".into()) }
    async fn containers(&self) -> anyhow::Result<Vec<(String, String)>> { Ok(vec![]) }
    async fn objects(&self, _: &str) -> anyhow::Result<Vec<String>> { Ok(vec![]) }
    async fn rows(&self, _: &str, _: &str, _: u64, _: u64) -> anyhow::Result<HttpRows> { Ok(HttpRows { columns: vec![], rows: vec![] }) }
    async fn run(&self, _: &str, _: &str, _: &str) -> anyhow::Result<HttpRows> { panic!("typed row plans must never fall back to the ordinary REST text/SQL route") }
}
#[tokio::test]
async fn shared_http_trait_never_infers_relational_write_or_metadata_capability() {
    let adapter = OrdinaryRest;
    assert!(adapter.relational_capabilities().await.unwrap_err().to_string().contains("UNSUPPORTED_CAPABILITY"));
    let object = JdbcObjectId { catalog: "db".into(), schema: "schema".into(), name: "table".into() };
    assert!(adapter.relational_metadata(&object).await.unwrap_err().to_string().contains("UNSUPPORTED_CAPABILITY"));
    let plan = JdbcRowPlan { operation_id: uuid::Uuid::new_v4().to_string(), session_id: uuid::Uuid::new_v4().to_string(), object,
        base_revision: "a".repeat(64), kind: "insert".into(), keys: vec![], original_values: vec![], changes: vec![("value".into(), DbValue::Text("not SQL".into()))], read_only: false, timeout_secs: 1 };
    assert!(adapter.apply_row_plan(&plan).await.unwrap_err().to_string().contains("UNSUPPORTED_CAPABILITY"));
    assert_eq!(adapter.ping().await.unwrap(), "ordinary REST");
}
