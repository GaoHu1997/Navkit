//! Real Rust -> verified HTTPS -> unmodified Java 11/H2 acceptance test.
//! Run only through tools/tests/http-tunnel-client.test.mjs. The fixture owns every
//! listener, certificate, bearer, database and child process; no user DB is used.
use super::*;
use crate::execution::{Binding, Registry as ExecutionRegistry};
use crate::execution_contracts::{ExecutionOutcome, ExecutionRequest, ExecutionState};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    version: u32,
    endpoint: String,
    ca_path: String,
    server_id: String,
    target_id: String,
    bearer: String,
    gateways: BTreeMap<String, String>,
}
impl Fixture {
    fn load() -> Self {
        let path = std::env::var("NAVKIT_HTTP_TUNNEL_CLIENT_FIXTURE")
            .expect("Use the dedicated Node runner; no external/default endpoint is accepted");
        let path = Path::new(&path);
        assert!(path.is_absolute());
        assert_eq!(path.file_name().and_then(|v| v.to_str()), Some("client-fixture.json"));
        assert!(path.parent().and_then(Path::file_name).and_then(|v| v.to_str())
            .is_some_and(|v| v.starts_with("navkit-http-tunnel-client-")));
        let meta = std::fs::symlink_metadata(path).expect("temporary fixture manifest");
        assert!(meta.is_file() && !meta.file_type().is_symlink() && meta.len() < 65536);
        let fixture: Self = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(fixture.version, 1);
        for endpoint in std::iter::once(&fixture.endpoint).chain(fixture.gateways.values()) {
            let url = wire::validate_https_endpoint(endpoint).unwrap();
            assert!(matches!(url.host_str(), Some("localhost" | "127.0.0.1")));
        }
        fixture
    }
    fn config(&self, endpoint: &str) -> TunnelConfig {
        let tls = crate::tls_options::parse(&serde_json::json!({
            "sslmode": "verify-full", "ssl_ca": self.ca_path
        }).to_string(), true).unwrap();
        TunnelConfig {
            endpoint: endpoint.into(), expected_server_id: self.server_id.clone(),
            expected_target_id: self.target_id.clone(), target_alias: "main".into(),
            connect_timeout_ms: 5000, tls, bearer: Zeroizing::new(self.bearer.clone()),
        }
    }
    fn endpoint(&self, fault: &str) -> &str {
        self.gateways.get(fault).unwrap_or_else(|| panic!("missing fixture fault endpoint: {fault}"))
    }
    async fn connect(&self, endpoint: &str) -> Arc<TunnelRuntime> {
        TunnelRuntime::connect(self.config(endpoint)).await.expect("verified temporary HTTPS reference service")
    }
}
fn one_cell(result: &MultiResult) -> Option<&str> {
    assert_eq!(result.sets.len(), 1);
    assert_eq!(result.sets[0].rows.len(), 1);
    assert_eq!(result.sets[0].rows[0].len(), 1);
    result.sets[0].rows[0][0].as_deref()
}
async fn count(session: &RemoteSession, id: i64) -> String {
    let result = session.execute("SELECT COUNT(*) FROM EVENTS WHERE ID = ?".into(),
        vec![wire::TunnelValue::Integer(id.to_string())], 10000).await.unwrap();
    one_cell(&result).unwrap().to_owned()
}
fn request_for_status(session: &RemoteSession, sequence: u64, sql: &str) -> wire::ExecuteRequest {
    let sequence = sequence.to_string();
    let request = wire::ExecuteRequest {
        protocol: wire::PROTOCOL.into(), request_id: Uuid::new_v4().to_string(),
        binding: session.binding.clone(), execution_id: wire::execution_id(&session.binding, &sequence).unwrap(),
        sequence, sql: sql.into(), binds: vec![], timeout_ms: 10000,
    };
    request.validate().unwrap();
    request
}
fn registered(
    registry: &ExecutionRegistry, runtime: &TunnelRuntime, session: &RemoteSession, endpoint: &str, sql: &str,
) -> (ExecutionRequest, Arc<Entry>) {
    // The fixture supplies a real immutable endpoint/session resource fingerprint,
    // never a client-supplied grant or a grantless Registry::register bypass.
    let fingerprint = format!("{:x}", Sha256::digest(serde_json::to_vec(&(
        endpoint, &runtime.capabilities.server_id, &runtime.capabilities.server_generation,
        &session.binding, "verify-full",
    )).unwrap()));
    let mut request = ExecutionRequest {
        execution_id: String::new(), connection_id: format!("https-fixture:{}", session.binding.session_id),
        namespace: "PUBLIC".into(), owner_id: format!("page:{}", Uuid::new_v4()), sql: sql.into(),
        transaction_owner: None, timeout_ms: Some(10000), origin: Some("query".into()),
        execution_grant: None, cancel_before_send: false,
    };
    let binding = Binding {
        principal: "main".into(), owner: request.owner_id.clone(), connection: request.connection_id.clone(),
        namespace: request.namespace.clone(), fingerprint,
    };
    registry.grant_request(&mut request, binding.clone()).unwrap();
    let mut changed = request.clone(); changed.sql.push_str(" ");
    assert!(registry.register(&changed, binding.clone(), 30).is_err(), "grant must bind the authorized SQL");
    let entry = registry.register(&request, binding.clone(), 30).unwrap();
    for changed in [
        Binding { principal: "another-window".into(), ..binding.clone() },
        Binding { owner: "another-page".into(), ..binding.clone() },
        Binding { namespace: "OTHER".into(), ..binding.clone() },
        Binding { fingerprint: "another-endpoint-generation".into(), ..binding.clone() },
    ] { assert!(registry.authorized(&request.execution_id, &changed).is_err()); }
    assert!(registry.register(&request, binding, 30).is_err(), "the grant/ID is once-only");
    (request, entry)
}
async fn c1_execute(runtime: Arc<TunnelRuntime>, session: &RemoteSession, endpoint: &str, sql: &str) -> ExecutionOutcome {
    let registry = ExecutionRegistry::default();
    let leases = crate::manual_transaction::Registry::default();
    let (request, entry) = registered(&registry, &runtime, session, endpoint, sql);
    let activity = Arc::new(tokio::sync::RwLock::new(()));
    entry.own_activity(activity.clone().read_owned().await);
    let lease = leases.lease(&request.connection_id).await;
    let (result, confirmed) = runtime.run_owned(request.namespace.clone(), request.sql.clone(), entry.clone(), false, lease).await;
    let outcome = entry.finish(result, confirmed, false);
    assert!(!outcome.resources_pending, "reference service acknowledged actual close/drain");
    assert!(!registry.has_active(&request.connection_id));
    assert!(activity.try_write().is_ok(), "activity guard released after work and control drain");
    assert!(leases.try_ordinary(&[&request.connection_id]).await.unwrap().is_some());
    outcome
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Dedicated Node harness must start temporary trusted HTTPS Java 11/H2; no default/user endpoint"]
async fn real_https_java_h2_rust_client_contract() {
    let fixture = Fixture::load();
    let runtime = fixture.connect(&fixture.endpoint).await;
    assert_eq!(runtime.capabilities.implementation, "navkit-h2-reference/1");
    assert!(runtime.capabilities.capabilities.sql_write);
    assert!(!runtime.capabilities.capabilities.row_mutation && !runtime.capabilities.capabilities.ddl
        && !runtime.capabilities.capabilities.manual_transaction);
    assert_eq!(runtime.namespaces(), vec!["PUBLIC".to_owned(), "OTHER".to_owned()]);
    let session = runtime.session("PUBLIC", false).await.unwrap();
    assert_eq!(session.binding.target_id, fixture.target_id);
    assert_eq!(session.binding.server_id, fixture.server_id);
    assert!(!session.binding.read_only);
    assert!(Arc::ptr_eq(&session, &runtime.session("PUBLIC", false).await.unwrap()));
    assert!(runtime.session("NOT_AUTHORIZED", false).await.is_err());

    // Real PreparedStatement typed DML and SELECT. String display must not round
    // BIGINT/DECIMAL, confuse NULL and empty, or drop binary/NUL/non-BMP text.
    use wire::TunnelValue as V;
    let text = "中文; O'Reilly 🙂\0尾";
    let binds = vec![V::Integer("1".into()), V::Text(text.into()), V::Integer(i64::MAX.to_string()),
        V::Decimal("12345678901234567890.123456789012".into()), V::Null, V::Text(String::new()),
        V::Binary("AP8BgA==".into()), V::Date("2026-02-28".into()), V::Time("12:34:56.123456789".into()),
        V::Timestamp("2026-11-10T12:34:56.123456789".into()), V::Timestamptz("2026-11-10T12:34:56.123456789+05:30".into())];
    let inserted = session.execute("INSERT INTO EVENTS(ID,TXT,BIG,AMT,NIL,EMPTY,BIN,D,TM,TS,TZ) VALUES(?,?,?,?,?,?,?,?,?,?,?)".into(), binds, 10000).await.unwrap();
    assert_eq!(inserted.sets.len(), 1); assert_eq!(inserted.sets[0].affected, 1); assert!(!inserted.sets[0].has_rows);
    let selected = session.execute("SELECT ID,TXT,BIG,AMT,NIL,EMPTY,BIN,D,TM,TS,TZ FROM EVENTS WHERE ID = ?".into(), vec![V::Integer("1".into())], 10000).await.unwrap();
    assert_eq!(selected.sets[0].total, 1);
    assert_eq!(selected.sets[0].rows, vec![vec![Some("1".into()), Some(text.into()), Some(i64::MAX.to_string()),
        Some("12345678901234567890.123456789012".into()), None, Some(String::new()), Some("0x00FF0180".into()),
        Some("2026-02-28".into()), Some("12:34:56.123456789".into()), Some("2026-11-10T12:34:56.123456789".into()),
        Some("2026-11-10T12:34:56.123456789+05:30".into())]]);
    let other = runtime.session("OTHER", true).await.unwrap();
    assert_ne!(session.binding.session_id, other.binding.session_id);
    assert_eq!(one_cell(&other.execute("SELECT TXT FROM EVENTS WHERE ID = ?".into(), vec![V::Integer("7".into())], 10000).await.unwrap()), Some("other-namespace"));
    let read = runtime.session("PUBLIC", true).await.unwrap();
    assert!(read.binding.read_only);
    assert!(read.execute("INSERT INTO EVENTS(ID,TXT) VALUES(?,?)".into(), vec![V::Integer("2".into()), V::Text("forbidden".into())], 10000).await.is_err());
    assert_eq!(count(&session, 2).await, "0");
    eprintln!("HTTP_CLIENT_EVIDENCE typed_prepared_dml_and_select exact_values namespace read_only");

    // Control requests use the independent HTTPS client while the SQL mutex is
    // deliberately held by the outstanding prepared stream.
    const LONG: &str = "SELECT COUNT(*) FROM WORK AS A, WORK AS B, WORK AS C";
    let prepared = session.prepare(LONG.into(), vec![], 10000, session.sequence.lock().await).await.unwrap();
    let request = prepared.request.clone(); let observed = Mutex::new(None);
    let started = Instant::now();
    let control = async {
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut saw_running = false;
        while Instant::now() < deadline {
            if let Ok(status) = session.status(&request).await {
                if status.state == "running" { saw_running = true; break; }
                if status.state != "running" { break; }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let cancelled = session.cancel(&request).await;
        (saw_running, cancelled, started.elapsed())
    };
    let (result, (saw_running, cancelled, elapsed)) = tokio::join!(session.stream(&request, None, &observed), control);
    cancelled.unwrap(); assert!(saw_running); assert!(elapsed < Duration::from_secs(5)); assert!(result.is_err());
    let final_state = observed.lock().clone().expect("strict EOF published a verified final");
    assert_eq!(final_state.state, "cancelled"); assert_eq!(final_state.transaction, "rolled_back"); assert!(!final_state.poisoned);
    let status = session.status(&request).await.unwrap();
    assert_eq!(status.state, "cancelled"); assert_eq!(status.affected_rows.as_deref(), Some("0"));
    drop(prepared);
    assert_eq!(one_cell(&session.execute("SELECT 42".into(), vec![], 10000).await.unwrap()), Some("42"));
    eprintln!("HTTP_CLIENT_EVIDENCE independent_status_cancel real_statement_drained");

    let c1 = c1_execute(runtime.clone(), &session, &fixture.endpoint, "INSERT INTO EVENTS(ID,TXT) VALUES(20,'c1 once')").await;
    assert_eq!(c1.state, ExecutionState::Succeeded); assert_eq!(c1.effects, "completed");
    assert_eq!(c1.result.unwrap().sets[0].affected, 1); assert_eq!(count(&session, 20).await, "1");

    let registry = ExecutionRegistry::default(); let leases = crate::manual_transaction::Registry::default();
    let (c1_request, entry) = registered(&registry, &runtime, &session, &fixture.endpoint, LONG);
    let activity = Arc::new(tokio::sync::RwLock::new(())); entry.own_activity(activity.clone().read_owned().await);
    let expected = request_for_status(&session, *session.sequence.lock().await + 1, LONG);
    let lease = leases.lease(&c1_request.connection_id).await;
    let stop = async {
        let deadline = Instant::now() + Duration::from_secs(4); let mut running = false;
        while Instant::now() < deadline {
            if let Ok(status) = session.status(&expected).await { if status.state == "running" { running = true; break; } }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let lease_held = leases.try_ordinary(&[&c1_request.connection_id]).await.unwrap().is_none();
        let activity_held = activity.try_write().is_err();
        (running, lease_held, activity_held, entry.request_stop(false))
    };
    let ((result, confirmed), (running, lease_held, activity_held, ack)) = tokio::join!(
        runtime.clone().run_owned("PUBLIC".into(), LONG.into(), entry.clone(), false, lease), stop);
    assert!(running && lease_held && activity_held && ack.accepted);
    let outcome = entry.finish(result, confirmed, false);
    assert_eq!(outcome.state, ExecutionState::Cancelled); assert!(outcome.deadline.stop_confirmed);
    assert!(outcome.result.is_none()); assert!(!outcome.resources_pending);
    assert!(activity.try_write().is_ok()); assert!(!registry.has_active(&c1_request.connection_id));
    assert!(leases.try_ordinary(&[&c1_request.connection_id]).await.unwrap().is_some());
    assert_eq!(session.status(&expected).await.unwrap().state, "cancelled");
    eprintln!("HTTP_CLIENT_EVIDENCE c1_real_grant_owned_future_cancel_activity_and_lease");

    // Every fault gateway first negotiates the real Java service; only its
    // selected response is damaged. Writes are observed on a separate session.
    for (index, fault) in ["stream_missing_final", "stream_truncated_final", "stream_final_then_bad",
        "stream_foreign_binding", "stream_target_generation", "stream_redirect", "stream_html"].iter().enumerate() {
        let endpoint = fixture.endpoint(fault);
        let broken = fixture.connect(endpoint).await;
        let remote = broken.session("PUBLIC", false).await.unwrap();
        let id = 100 + index as i64;
        let sql = format!("INSERT INTO EVENTS(ID,TXT) VALUES({id},'must not replay')");
        let outcome = c1_execute(broken.clone(), &remote, endpoint, &sql).await;
        assert_eq!(outcome.state, ExecutionState::Unknown, "{fault} must not publish SQL success");
        assert!(outcome.result.is_none()); assert_eq!(outcome.effects, "possibly_applied");
        assert_eq!(outcome.error.as_ref().unwrap().code, "EXECUTION_EFFECTS_UNKNOWN");
        assert!(remote.poisoned.load(Ordering::Acquire) && remote.closed.load(Ordering::Acquire));
        let retry = remote.execute(sql, vec![], 10000).await.unwrap_err();
        assert!(retry.to_string().contains("QUARANTINED"));
        assert!(broken.session("PUBLIC", false).await.is_err());
        let actual = count(&session, id).await;
        assert_eq!(actual, if matches!(*fault, "stream_redirect" | "stream_html") { "0" } else { "1" }, "{fault}: real independent committed-row witness");
        broken.shutdown().await;
        eprintln!("HTTP_CLIENT_EVIDENCE {fault} unknown_no_replay");
    }
    for fault in ["cap_redirect", "cap_html"] {
        assert!(TunnelRuntime::connect(fixture.config(fixture.endpoint(fault))).await.is_err(), "{fault}");
        eprintln!("HTTP_CLIENT_EVIDENCE {fault} negotiation_rejected");
    }
    for fault in ["open_foreign_binding", "open_target_generation"] {
        let changed = fixture.connect(fixture.endpoint(fault)).await;
        assert!(changed.session("PUBLIC", false).await.is_err(), "{fault}: changed identity may not execute");
        changed.shutdown().await;
        eprintln!("HTTP_CLIENT_EVIDENCE {fault} open_rejected");
    }
    let status_runtime = fixture.connect(fixture.endpoint("control_foreign_status")).await;
    let status_session = status_runtime.session("PUBLIC", false).await.unwrap();
    let prepared = status_session.prepare("SELECT 1".into(), vec![], 10000, status_session.sequence.lock().await).await.unwrap();
    let observed = Mutex::new(None);
    status_session.stream(&prepared.request, None, &observed).await.unwrap();
    assert!(status_session.status(&prepared.request).await.is_err(), "foreign target-generation status is never completion evidence");
    drop(prepared); status_runtime.shutdown().await;
    eprintln!("HTTP_CLIENT_EVIDENCE control_foreign_status rejected");
    runtime.shutdown().await;
    assert!(session.closed.load(Ordering::Acquire));
    let observer_runtime = fixture.connect(&fixture.endpoint).await;
    let observer = observer_runtime.session("PUBLIC", true).await.unwrap();
    assert_eq!(count(&observer, 1).await, "1");
    observer_runtime.shutdown().await;
    assert!(observer.closed.load(Ordering::Acquire));
    assert!(runtime.session("PUBLIC", false).await.is_err());
    eprintln!("HTTP_CLIENT_EVIDENCE complete");
}
