//! Production acceptance: encrypted local profile -> saved admission -> HTTPS adapter -> C1.
//! The parent registers this as a child of http_tunnel and runs only this exact ignored
//! test through its existing temporary HTTPS/Java 11/H2 fixture. No service is started
//! here; no local H2, network mock, synthetic grant or production data root is used.
use super::*;
use crate::{
    commands::{self, AppState},
    db::{self, runtime},
    execution_contracts::{ExecutionOutcome, ExecutionRequest, ExecutionState},
    models::{ConnectionConfig, ConnectionRequest, MultiResult},
    ssh_tunnel::profiles::{self, HttpProfile, RuntimeProfileDraft, SecretPatch, TlsProfile},
    storage::Store,
    tls_options::TlsMode,
};
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, io::Write, path::{Path, PathBuf}};
use zeroize::Zeroize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProductionFixture {
    version: u32,
    endpoint: String,
    ca_path: String,
    server_id: String,
    target_id: String,
    bearer: String,
    gateways: BTreeMap<String, String>,
}
impl Drop for ProductionFixture {
    fn drop(&mut self) { self.bearer.zeroize(); }
}
impl ProductionFixture {
    fn load() -> (Self, PathBuf) {
        let path = PathBuf::from(std::env::var("NAVKIT_HTTP_TUNNEL_CLIENT_FIXTURE")
            .expect("Only the dedicated temporary HTTPS fixture may run this test"));
        assert!(path.is_absolute(), "No relative or default fixture is accepted");
        assert_eq!(path.file_name().and_then(|name| name.to_str()), Some("client-fixture.json"));
        let parent = path.parent().expect("temporary fixture parent");
        assert!(parent.file_name().and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("navkit-http-tunnel-client-")));
        let parent_meta = fs::symlink_metadata(parent).expect("fixture directory exists");
        assert!(parent_meta.is_dir() && !parent_meta.file_type().is_symlink());
        let root = fs::canonicalize(parent).expect("canonical owned fixture root");
        assert!(root.starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()),
            "The test may use only an OS-temporary fixture directory");
        let meta = fs::symlink_metadata(&path).expect("temporary manifest exists");
        assert!(meta.is_file() && !meta.file_type().is_symlink() && meta.len() < 65_536);
        let fixture: Self = serde_json::from_slice(&fs::read(&path).unwrap())
            .expect("bounded manifest must match the dedicated fixture contract");
        assert_eq!(fixture.version, 1);
        for endpoint in std::iter::once(&fixture.endpoint).chain(fixture.gateways.values()) {
            let endpoint = wire::validate_https_endpoint(endpoint).unwrap();
            assert!(matches!(endpoint.host_str(), Some("localhost" | "127.0.0.1")),
                "No external endpoint, user database or fallback endpoint is allowed");
        }
        let ca = Path::new(&fixture.ca_path);
        assert!(ca.is_absolute());
        assert_eq!(ca.file_name().and_then(|name| name.to_str()), Some("ca.pem"));
        assert_eq!(fs::canonicalize(ca.parent().unwrap()).unwrap(), root,
            "CA material must belong to this exact temporary HTTPS fixture");
        let ca_meta = fs::symlink_metadata(ca).unwrap();
        assert!(ca_meta.is_file() && !ca_meta.file_type().is_symlink()
            && ca_meta.len() > 0 && ca_meta.len() <= 4 * 1024 * 1024);
        (fixture, root)
    }
    fn draft(&self) -> RuntimeProfileDraft {
        RuntimeProfileDraft {
            ssh: None, tls: None,
            http: Some(HttpProfile {
                endpoint: self.endpoint.clone(), protocol: "navkit-sql-tunnel/1".into(),
                expected_server_id: self.server_id.clone(), expected_target_id: self.target_id.clone(),
                target_alias: "main".into(), connect_timeout_ms: 5_000,
                tls: TlsProfile { mode: TlsMode::VerifyFull, ca: Some(self.ca_path.clone()), cert: None, key: None },
            }),
            http_bearer: SecretPatch::Set(self.bearer.clone()),
            ssh_password: SecretPatch::Keep, ssh_key_passphrase: SecretPatch::Keep,
        }
    }
}

/// The only data root this test owns. Failure leaves it for the parent's fixture
/// cleanup instead of pretending a still-running physical resource has drained.
struct ProductionData { root: PathBuf, drained: bool }
impl ProductionData {
    fn new(fixture_root: &Path) -> Self {
        let root = fixture_root.join(format!("production-profile-state-{}", Uuid::new_v4()));
        fs::create_dir(&root).expect("create-only independent Store root");
        let global_root = crate::storage::data_dir();
        assert!(global_root.is_absolute());
        assert_ne!(root, global_root,
            "C1 must read this Store's root, never global portable data beside the test executable");
        // Do not create a global root just to canonicalize it: only this fixture's
        // independent Store may be touched, even beside a brand-new copied executable.
        if global_root.exists() {
            assert_ne!(fs::canonicalize(&root).unwrap(), fs::canonicalize(&global_root).unwrap());
        }
        let mut key = Zeroizing::new([0_u8; 32]);
        rand::thread_rng().fill_bytes(key.as_mut());
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(root.join("key.bin"))
            .expect("first and original key in a proven empty temporary data root");
        file.write_all(key.as_ref()).unwrap(); file.sync_all().unwrap();
        Self { root, drained: false }
    }
}
impl Drop for ProductionData {
    fn drop(&mut self) {
        if self.drained { let _ = fs::remove_dir_all(&self.root); }
    }
}

fn saved_profile(
    fixture: &ProductionFixture, storage: &Store, root: &Path, id: &str, read_only: bool,
) -> ConnectionConfig {
    // These are deliberately NOT the bridge's JDBC URL/user/password. If ensure
    // drops the HTTP profile and enters the local H2 adapter, it cannot reach the
    // seeded EVENTS table or pass the remote-resource assertions below.
    let unavailable = root.join("never-open-local-h2").to_string_lossy().replace('\\', "/");
    let request = ConnectionRequest {
        kind: crate::models::DbKind::of("h2"), host: "127.0.0.1".into(), port: 1,
        user: "not-the-bridge-database-user".into(), password: String::new(),
        database: format!("jdbc:h2:file:{unavailable};IFEXISTS=TRUE;MODE=REGULAR"),
        file: String::new(), params: String::new(), timeout_secs: 10,
    };
    let mut cfg = runtime::request_config(id, &request, read_only, false);
    cfg.name = "temporary production HTTPS profile".into();
    let endpoint = runtime::endpoint_fingerprint(&cfg).unwrap();
    let submission = runtime::RuntimeSubmission {
        draft: fixture.draft(), expected_revision: None, rebind_endpoint: false,
    };
    let committed = profiles::commit::save_connection(storage, root, cfg, None, Some(&submission))
        .expect("real production atomic profile/config save; no manual reference or plaintext params");
    let cfg = committed.connections.into_iter().find(|cfg| cfg.id == id)
        .expect("atomic commit publishes the saved connection configuration");
    let store = profiles::Store::new(root);
    let saved = store.view(id).unwrap().expect("atomically saved profile exists");
    assert!(saved.http.is_some() && saved.http_bearer_present);
    assert!(saved.ssh.is_none() && saved.tls.is_none());
    assert_eq!(runtime::profile_reference(&cfg.params).unwrap().as_deref(), Some(saved.revision.as_str()),
        "the PRODUCTION commit, not the test, must link the encrypted profile's exact revision");
    assert!(!profiles::commit::has_pending(root, id).unwrap());
    let view = serde_json::to_vec(&saved).unwrap();
    assert!(!view.windows(fixture.bearer.len()).any(|bytes| bytes == fixture.bearer.as_bytes()),
        "the view must disclose only Bearer presence");
    let encrypted = root.join("runtime-profiles").join(format!("{:x}.enc", Sha256::digest(id.as_bytes())));
    let bytes = fs::read(&encrypted).unwrap();
    assert!(bytes.len() >= 28);
    assert!(!bytes.windows(fixture.bearer.len()).any(|bytes| bytes == fixture.bearer.as_bytes()),
        "persisted profile must not contain a plaintext Bearer");
    // Ordinary repeat-save carries no new secret patch. It must retain the exact
    // encrypted material and the originally committed revision, not invent a new
    // profile or clear/replace the Bearer while saving unrelated configuration.
    let repeated = profiles::commit::save_connection(storage, root, cfg.clone(), None, None)
        .expect("unchanged production save preserves the existing encrypted HTTP profile");
    let repeated_cfg = repeated.connections.into_iter().find(|cfg| cfg.id == id).unwrap();
    assert_eq!(repeated_cfg, cfg);
    assert_eq!(store.view(id).unwrap().unwrap().revision, saved.revision);
    assert!(fs::read(&encrypted).unwrap() == bytes, "unchanged save must preserve ciphertext exactly");
    assert!(!profiles::commit::has_pending(root, id).unwrap());
    let resolved = store.resolve(id, &endpoint).unwrap().expect("saved local profile resolves");
    assert!(resolved.http.as_ref().is_some_and(|http| http.bearer.as_str() == fixture.bearer),
        "the trusted network path receives the decrypted original token, not a substitute");
    cfg
}

async fn c1_dispatch(
    state: &AppState, id: &str, sql: String,
) -> (ExecutionRequest, ExecutionOutcome) {
    let mut request = ExecutionRequest {
        execution_id: String::new(), connection_id: id.into(), namespace: "PUBLIC".into(),
        owner_id: format!("production-page-{}", Uuid::new_v4()), sql,
        transaction_owner: None, timeout_ms: Some(10_000), origin: Some("query".into()),
        execution_grant: None, cancel_before_send: false,
    };
    // Use the ACTUAL saved-config/local-profile/window binding. No fixture grant,
    // direct Registry.register or bespoke remote execution helper is permitted.
    commands::grant_execution_request(state, &mut request, "main")
        .expect("production grant must resolve the independent Store root");
    assert!(request.execution_grant.as_ref().is_some_and(|grant| !grant.is_empty()));
    assert!(Uuid::parse_str(&request.execution_id).is_ok());
    let signed = request.clone();
    let outcome = tokio::time::timeout(Duration::from_secs(25),
        commands::execute_sql_service(state, request, "main")).await
        .expect("bounded production C1 test; no request is replayed on timeout")
        .expect("production SQL service consumes the real one-use grant");
    assert_eq!(outcome.execution_id, signed.execution_id);
    assert!(outcome.state.terminal());
    assert!(!outcome.resources_pending, "normal reference service work/control must actually drain");
    assert!(!state.executions.has_active(id));
    assert!(state.manual_transactions.try_ordinary(&[id]).await.unwrap().is_some());
    (signed, outcome)
}
fn completed(outcome: &ExecutionOutcome) -> &MultiResult {
    assert_eq!(outcome.state, ExecutionState::Succeeded, "production C1 error: {:?}", outcome.error);
    assert!(outcome.error.is_none());
    outcome.result.as_ref().expect("only a successful verified C1 result is readable")
}
fn scalar(result: &MultiResult) -> &str {
    assert_eq!(result.sets.len(), 1); assert_eq!(result.sets[0].rows.len(), 1);
    assert_eq!(result.sets[0].rows[0].len(), 1);
    result.sets[0].rows[0][0].as_deref().expect("COUNT value cannot be NULL")
}
async fn retire_verified(state: &AppState, id: &str) {
    state.executions.require_quiescent(id).unwrap();
    let activity = state.activity.enter().unwrap();
    let ids = [id];
    let lease = state.manual_transactions.ordinary(&ids).await.unwrap();
    tokio::time::timeout(Duration::from_secs(25),
        crate::runtime_commands::retire(state, id, (activity, lease))).await
        .expect("reference session DELETE/drain must finish within the fixture's bound")
        .expect("retirement must report actual drain, never resources_pending as success");
    assert!(db::get_db(&state.registry, id).is_err());
    assert!(!state.registry.read().keys().any(|key| key.starts_with(&format!("{id}@"))));
    state.connection_runtime.require_openable(id).unwrap();
    assert!(state.manual_transactions.try_ordinary(&[id]).await.unwrap().is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Parent's dedicated temporary HTTPS Java 11/H2 fixture only; no default service or user data"]
async fn production_profile_to_c1_uses_remote_resource() {
    let (fixture, fixture_root) = ProductionFixture::load();
    let mut data = ProductionData::new(&fixture_root);
    let write_id = format!("http-production-write-{}", Uuid::new_v4());
    let read_id = format!("http-production-read-{}", Uuid::new_v4());
    let storage = Store::load_at(&data.root);
    let writable = saved_profile(&fixture, &storage, &data.root, &write_id, false);
    let readonly = saved_profile(&fixture, &storage, &data.root, &read_id, true);
    // Both connections and their encrypted profiles were saved ONLY through the
    // production atomic commit. No manual registry publication or cfg reference
    // construction is allowed to make an otherwise disconnected profile appear valid.
    assert_eq!(storage.connections(), vec![writable.clone(), readonly.clone()]);
    let state = AppState {
        store: storage, registry: Arc::new(db::DbRegistry::default()),
        manual_transactions: Default::default(), activity: Default::default(),
        executions: Default::default(), mutations: Default::default(),
        connection_runtime: Default::default(), query_results: Default::default(),
    };
    assert_ne!(state.store.data_root(), crate::storage::data_dir().as_path());
    assert!(state.registry.read().is_empty(), "admission must cold-open the actual production adapter");
    for cfg in [&writable, &readonly] {
        let profile = profiles::Store::new(&data.root).view(&cfg.id).unwrap().unwrap();
        assert_eq!(runtime::profile_reference(&cfg.params).unwrap(), Some(profile.revision));
        assert!(!profiles::commit::has_pending(&data.root, &cfg.id).unwrap());
    }
    eprintln!("HTTP_PRODUCTION_EVIDENCE atomic_profile_config_save unchanged_ciphertext independent_store_root");

    let admitted = crate::runtime_commands::ensure(&state, &write_id).await
        .expect("real saved profile admission must not fall back to local H2");
    let http = match admitted.as_ref() {
        db::Db::Http(http) => http,
        _ => panic!("production HTTPS profile was silently routed to a non-HTTP database"),
    };
    let remote = http.tunnel_runtime().expect("ordinary REST/local H2 is not the certified SQL resource");
    let expected = runtime::binding_fingerprint_at(&writable, &data.root).unwrap();
    assert!(state.connection_runtime.matches(&write_id, &expected, &admitted));
    assert_eq!(remote.capabilities.server_id, fixture.server_id);
    assert_eq!(remote.capabilities.implementation, "navkit-h2-reference/1");
    assert!(remote.capabilities.capabilities.sql_write);
    let session = remote.session("PUBLIC", false).await.unwrap();
    assert_eq!(session.binding.server_id, fixture.server_id);
    assert_eq!(session.binding.target_id, fixture.target_id);
    assert_eq!(session.binding.namespace, "PUBLIC"); assert!(!session.binding.read_only);
    assert_eq!(session.binding.server_generation, remote.capabilities.server_generation);
    assert_eq!(session.binding.capability_revision, remote.capabilities.capability_revision);
    assert!(Arc::ptr_eq(&admitted, &crate::runtime_commands::ensure(&state, &write_id).await.unwrap()));
    let caps = crate::execution::capabilities(Some(admitted.as_ref()), &writable.kind, false, false);
    assert_eq!(caps.protocol, "navkit-sql-tunnel/1"); assert_ne!(caps.cancel.level, "unsupported");
    eprintln!("HTTP_PRODUCTION_EVIDENCE ensure_db_http actual_server_target_session");

    // Seeded only on the fixture's SERVER H2, never in this independent client Store.
    let objects = http.objects("PUBLIC").await.unwrap();
    assert!(objects.iter().any(|name| name == "EVENTS"));
    let catalog = session.catalog().await.unwrap();
    assert!(catalog.iter().any(|object| object.schema == "PUBLIC" && object.name == "EVENTS" && object.kind == "table"));
    let metadata = session.metadata(wire::ObjectId {
        catalog: session.catalog.clone(), schema: "PUBLIC".into(), name: "EVENTS".into(),
    }).await.unwrap();
    assert_eq!(metadata.session_id, session.binding.session_id);
    assert_eq!(metadata.object.catalog, session.catalog);
    assert_eq!(metadata.object.schema, "PUBLIC"); assert_eq!(metadata.object.name, "EVENTS");
    assert!(!metadata.backend_object_id.is_empty() && !metadata.revision.is_empty());
    assert_eq!(metadata.primary_key, vec!["ID".to_string()]);
    assert!(metadata.columns.iter().any(|column| column.name == "ID" && column.jdbc_type == -5));
    assert!(metadata.columns.iter().any(|column| column.name == "TXT"));
    let columns = http.table_columns("PUBLIC", "EVENTS").await.unwrap().unwrap();
    assert!(columns.iter().any(|column| column.name == "ID" && column.primary_key));
    eprintln!("HTTP_PRODUCTION_EVIDENCE real_remote_catalog_metadata");

    let row_id = 700_000_000_u64 + u64::from_be_bytes(Uuid::new_v4().as_bytes()[..8].try_into().unwrap()) % 1_000_000_000;
    let denied_id = row_id + 1;
    let marker = format!("profile-c1-{}", Uuid::new_v4());
    let (_, before) = c1_dispatch(&state, &write_id,
        format!("SELECT COUNT(*) FROM EVENTS WHERE ID = {row_id}")).await;
    assert_eq!(scalar(completed(&before)), "0");
    let (one_use, insert) = c1_dispatch(&state, &write_id,
        format!("INSERT INTO EVENTS(ID,TXT) VALUES({row_id},'{marker}')")).await;
    assert_eq!(completed(&insert).sets.len(), 1);
    assert_eq!(completed(&insert).sets[0].affected, 1);
    assert_eq!(insert.transaction_state, "committed");
    assert!(commands::execute_sql_service(&state, one_use, "main").await.is_err(),
        "replaying the consumed real C1 grant must never send a second INSERT");
    let (_, after) = c1_dispatch(&state, &write_id,
        format!("SELECT COUNT(*) FROM EVENTS WHERE ID = {row_id}")).await;
    assert_eq!(scalar(completed(&after)), "1");
    let (_, selected) = c1_dispatch(&state, &write_id,
        format!("SELECT ID,TXT FROM EVENTS WHERE ID = {row_id}")).await;
    assert_eq!(completed(&selected).sets[0].rows,
        vec![vec![Some(row_id.to_string()), Some(marker.clone())]]);
    assert!(state.registry.read().contains_key(&format!("{write_id}@PUBLIC")),
        "C1 must traverse the production namespace view rather than a fixture-injected registry entry");
    eprintln!("HTTP_PRODUCTION_EVIDENCE actual_c1_grant_remote_count_insert_select_once");

    let read_admitted = crate::runtime_commands::ensure(&state, &read_id).await.unwrap();
    let read_http = match read_admitted.as_ref() {
        db::Db::Http(http) => http,
        _ => panic!("read-only HTTPS profile was silently routed to local H2"),
    };
    let read_remote = read_http.tunnel_runtime().expect("read-only production tunnel exists");
    let read_session = read_remote.session("PUBLIC", true).await.unwrap();
    assert!(read_session.binding.read_only);
    assert_eq!(read_session.binding.target_id, fixture.target_id);
    let (_, read_count) = c1_dispatch(&state, &read_id,
        format!("SELECT COUNT(*) FROM EVENTS WHERE ID = {row_id}")).await;
    assert_eq!(scalar(completed(&read_count)), "1", "read-only config must still read the REAL server data");
    let (_, denied) = c1_dispatch(&state, &read_id,
        format!("INSERT INTO EVENTS(ID,TXT) VALUES({denied_id},'read-only-must-not-write')")).await;
    assert_ne!(denied.state, ExecutionState::Succeeded);
    assert!(denied.result.is_none() && denied.error.is_some());
    // A non-stream rejection may conservatively be Unknown, not a synthetic
    // rolled_back success. Independent server readback is the zero-write proof.
    let (_, no_write) = c1_dispatch(&state, &write_id,
        format!("SELECT COUNT(*) FROM EVENTS WHERE ID = {denied_id}")).await;
    assert_eq!(scalar(completed(&no_write)), "0");
    eprintln!("HTTP_PRODUCTION_EVIDENCE saved_read_only_remote_zero_writes");

    let old_session_id = session.binding.session_id.clone();
    retire_verified(&state, &write_id).await;
    assert!(remote.closing.load(Ordering::Acquire));
    assert!(session.closed.load(Ordering::Acquire), "retire must await verified remote session drain");
    assert!(remote.session("PUBLIC", false).await.is_err());
    assert!(session.catalog().await.is_err());
    assert!(session.execute("SELECT COUNT(*) FROM EVENTS".into(), vec![], 10_000).await.is_err(),
        "retained old session capability cannot execute after actual retire");
    let reopened = crate::runtime_commands::ensure(&state, &write_id).await.unwrap();
    let reopened_http = match reopened.as_ref() { db::Db::Http(http) => http, _ => panic!("explicit reopen must remain HTTPS") };
    let fresh = reopened_http.tunnel_runtime().unwrap();
    assert!(!Arc::ptr_eq(&fresh, &remote));
    let fresh_session = fresh.session("PUBLIC", false).await.unwrap();
    assert_ne!(fresh_session.binding.session_id, old_session_id);
    let (_, durable) = c1_dispatch(&state, &write_id,
        format!("SELECT COUNT(*) FROM EVENTS WHERE ID = {row_id}")).await;
    assert_eq!(scalar(completed(&durable)), "1", "committed row is durable in a new REMOTE session");
    retire_verified(&state, &write_id).await;
    retire_verified(&state, &read_id).await;
    assert!(fresh_session.closed.load(Ordering::Acquire));
    assert!(read_session.closed.load(Ordering::Acquire));
    assert!(state.registry.read().is_empty());
    assert!(state.activity.exclusive().is_ok(), "all work/control/admission guards released only after drain");
    assert!(!data.root.join("never-open-local-h2.mv.db").exists(), "no client-side H2 file was opened");
    eprintln!("HTTP_PRODUCTION_EVIDENCE retire_real_drain_old_capability_unusable explicit_remote_reopen complete");
    drop(state);
    data.drained = true;
}
