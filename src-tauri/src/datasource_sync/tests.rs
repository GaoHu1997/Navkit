//! 全部测试仅使用临时目录、合成连接和 127.0.0.1 mock WebDAV。
use super::*;
use crate::{
    db::{Db, DbRegistry},
    models::DbKind,
};
use std::{fs, path::PathBuf, sync::OnceLock};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Notify,
};

const PASSPHRASE: &str = "synthetic-sync-passphrase";
const DB_SECRET: &str = "synthetic-db-password";
const PARAM_SECRET: &str = "synthetic-param-token";

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("navkit-datasource-sync-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn store(&self) -> Store {
        Store::empty_at(&self.0)
    }
    fn state(&self) -> AppState {
        AppState {
            store: self.store(),
            registry: Arc::new(DbRegistry::default()),
            manual_transactions: Default::default(),
            activity: Default::default(),
        }
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        #[cfg(windows)]
        for name in ["connections.json", "datasource_sync.enc"] {
            let path = self.0.join(name);
            if let Ok(mut permissions) = fs::metadata(&path).map(|m| m.permissions()) {
                permissions.set_readonly(false);
                let _ = fs::set_permissions(path, permissions);
            }
        }
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn cfg(id: &str, group: &str) -> ConnectionConfig {
    ConnectionConfig {
        id: id.into(),
        name: format!("连接-{id}"),
        kind: DbKind::of("sqlite"),
        host: "synthetic.example".into(),
        port: 0,
        user: "synthetic-user".into(),
        password: DB_SECRET.into(),
        database: "main".into(),
        file: String::new(),
        params: format!("token={PARAM_SECRET}"),
        group: group.into(),
        read_only: false,
        keep_alive: true,
    }
}
fn snapshot(ids: &[&str]) -> Snapshot {
    Snapshot::new(
        ids.iter().map(|id| cfg(id, "folder")).collect(),
        vec!["folder".into(), "empty".into()],
        BTreeSet::new(),
    )
}
fn fixture() -> Snapshot {
    let mut s = snapshot(&["11111111-1111-4111-8111-111111111111"]);
    s.connections[0].params = format!("@navkit-driver:{{\"version\":1,\"driver_id\":\"synthetic-driver\",\"template\":\"\",\"token\":\"{PARAM_SECRET}\"}}");
    s
}
fn sealed_fixture() -> Vec<u8> {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES
        .get_or_init(|| crypto::seal(&fixture(), PASSPHRASE).unwrap())
        .clone()
}
fn encrypted_empty() -> Vec<u8> {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES
        .get_or_init(|| crypto::seal(&Snapshot::empty(), PASSPHRASE).unwrap())
        .clone()
}
fn local_save(store: &Store, s: &Snapshot) {
    let mut connections = s.connections.clone();
    for c in &mut connections {
        c.password = store.encrypt_password(&c.password).unwrap();
    }
    store
        .save_connections(connections, s.groups.clone())
        .unwrap();
}
fn input(url: &str, remember: bool) -> ConfigInput {
    ConfigInput {
        url: url.into(),
        user: "synthetic-dav-user".into(),
        auto_every: -1,
        remember_password: remember,
        password: Some("synthetic-webdav-password".into()),
        passphrase: Some(PASSPHRASE.into()),
        headers: Some("X-Token: synthetic-header-token".into()),
    }
}
fn configured(store: &Store, url: &str) -> Service {
    let service = Service::new(store);
    service.set_config(store, input(url, true)).unwrap();
    service
}
fn set_baseline(service: &Service, store: &Store, base: Snapshot) {
    let mut runtime = service.lock().unwrap();
    runtime.stored.baseline = Some(base);
    persist_state(store.root(), &runtime.stored).unwrap();
}
fn message<T>(result: Result<T, String>) -> String {
    match result {
        Ok(_) => panic!("预期安全拒绝，但操作成功"),
        Err(e) => e,
    }
}
fn choices(view: &Preview, choice: Choice) -> BTreeMap<String, Choice> {
    view.conflicts
        .iter()
        .map(|c| (c.key.clone(), choice))
        .collect()
}

#[derive(Clone)]
struct Request {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}
struct MockState {
    data: Option<Vec<u8>>,
    version: u32,
    dirs: BTreeSet<String>,
    requests: Vec<Request>,
    get_status: Option<u16>,
    directory_status: Option<u16>,
    mkcol_status: Option<u16>,
    no_etag: bool,
    weak_etag: bool,
    force_412: bool,
    html: bool,
    truncated_response: bool,
    corrupt_put: bool,
    truncate_put: bool,
    redirect: Option<String>,
    put_redirect: bool,
    retry_after: Option<String>,
    pause_put: Option<(Arc<Notify>, Arc<Notify>)>,
    put_hook: Option<Arc<dyn Fn() + Send + Sync>>,
}
impl Default for MockState {
    fn default() -> Self {
        Self {
            data: None,
            version: 1,
            dirs: ["/".into(), "/dav/".into()].into(),
            requests: vec![],
            get_status: None,
            directory_status: None,
            mkcol_status: None,
            no_etag: false,
            weak_etag: false,
            force_412: false,
            html: false,
            truncated_response: false,
            corrupt_put: false,
            truncate_put: false,
            redirect: None,
            put_redirect: false,
            retry_after: None,
            pause_put: None,
            put_hook: None,
        }
    }
}
struct Mock {
    url: String,
    state: Arc<Mutex<MockState>>,
    task: tokio::task::JoinHandle<()>,
}
impl Mock {
    async fn new(data: Option<Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/dav/", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(MockState {
            data,
            ..Default::default()
        }));
        let shared = state.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let state = shared.clone();
                tokio::spawn(async move {
                    serve(stream, state).await;
                });
            }
        });
        Self { url, state, task }
    }
    fn edit(&self, edit: impl FnOnce(&mut MockState)) {
        edit(&mut self.state.lock().unwrap());
    }
    fn requests(&self) -> Vec<Request> {
        self.state.lock().unwrap().requests.clone()
    }
    fn data(&self) -> Option<Vec<u8>> {
        self.state.lock().unwrap().data.clone()
    }
    fn dav(&self) -> Dav {
        Dav::new(&self.url, "u", "p", "", Arc::new(Mutex::new(None))).unwrap()
    }
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
        if let Some((_, release)) = &self.state.lock().unwrap().pause_put {
            release.notify_waiters();
        }
    }
}

async fn serve(mut stream: tokio::net::TcpStream, state: Arc<Mutex<MockState>>) {
    let mut bytes = vec![];
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        let n = match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        bytes.extend_from_slice(&chunk[..n]);
        if bytes.len() > crypto::MAX_REMOTE_BYTES + 65536 {
            return;
        }
        if let Some(at) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&bytes[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let method = first.next().unwrap_or("").to_string();
    let path = first.next().unwrap_or("").to_string();
    let mut headers = BTreeMap::<String, String>::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers
                .entry(name.to_ascii_lowercase())
                .and_modify(|v| {
                    v.push(',');
                    v.push_str(value.trim());
                })
                .or_insert_with(|| value.trim().into());
        }
    }
    let length = headers
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    if length > crypto::MAX_REMOTE_BYTES {
        return;
    }
    while bytes.len() < head_end + length {
        let n = match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        bytes.extend_from_slice(&chunk[..n]);
    }
    let request = Request {
        method: method.clone(),
        path: path.clone(),
        headers,
        body: bytes[head_end..head_end + length].to_vec(),
    };
    let (status, extra, body, incomplete, pause, hook) = {
        let mut s = state.lock().unwrap();
        s.requests.push(request.clone());
        let etag = format!("\"v{}\"", s.version);
        let mut extra: Vec<(String, String)> = vec![];
        let mut body = vec![];
        let mut incomplete = false;
        let mut pause = None;
        let mut hook = None;
        let status = match method.as_str() {
            "GET" if path == "/login" => {
                body = b"<html><form>login</form></html>".to_vec();
                200
            }
            "GET" if s.redirect.is_some() => {
                extra.push(("Location".into(), s.redirect.clone().unwrap()));
                302
            }
            "GET" if s.get_status.is_some() => {
                if let Some(value) = &s.retry_after {
                    extra.push(("Retry-After".into(), value.clone()));
                }
                s.get_status.unwrap()
            }
            "GET" if s.html => {
                body = b"<!doctype html><html>login</html>".to_vec();
                200
            }
            "GET" => {
                if let Some(data) = &s.data {
                    body = data.clone();
                    incomplete = s.truncated_response;
                    if !s.no_etag {
                        extra.push((
                            "ETag".into(),
                            if s.weak_etag {
                                format!("W/{etag}")
                            } else {
                                etag.clone()
                            },
                        ));
                    }
                    200
                } else {
                    404
                }
            }
            "PROPFIND" if s.directory_status.is_some() => s.directory_status.unwrap(),
            "PROPFIND" => {
                if s.dirs.contains(&path) {
                    body = format!("<?xml version=\"1.0\"?><d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>{path}</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>").into_bytes();
                    extra.push(("Content-Type".into(), "application/xml".into()));
                    207
                } else {
                    404
                }
            }
            "MKCOL" if s.mkcol_status.is_some() => s.mkcol_status.unwrap(),
            "MKCOL" => {
                let parent = path
                    .trim_end_matches('/')
                    .rsplit_once('/')
                    .map(|(p, _)| format!("{p}/"))
                    .unwrap_or_else(|| "/".into());
                if s.dirs.contains(&path) {
                    405
                } else if !s.dirs.contains(&parent) {
                    409
                } else {
                    s.dirs.insert(path.clone());
                    201
                }
            }
            "PUT" if s.put_redirect => {
                extra.push(("Location".into(), "/elsewhere".into()));
                307
            }
            "PUT" if s.force_412 => 412,
            "PUT" => {
                let valid = if s.data.is_some() {
                    request.headers.get("if-match") == Some(&etag)
                } else {
                    request.headers.get("if-none-match").map(String::as_str) == Some("*")
                };
                if !valid {
                    412
                } else {
                    let mut data = request.body.clone();
                    if s.corrupt_put && !data.is_empty() {
                        data[0] ^= 1;
                    }
                    if s.truncate_put {
                        data.truncate(data.len() / 2);
                    }
                    s.data = Some(data);
                    s.version += 1;
                    pause = s.pause_put.clone();
                    hook = s.put_hook.clone();
                    extra.push(("ETag".into(), format!("\"v{}\"", s.version)));
                    201
                }
            }
            _ => 405,
        };
        (status, extra, body, incomplete, pause, hook)
    };
    if let Some(hook) = hook {
        hook();
    }
    if let Some((arrived, release)) = pause {
        arrived.notify_one();
        release.notified().await;
    }
    let length = body.len();
    let mut response =
        format!("HTTP/1.1 {status} Mock\r\nContent-Length: {length}\r\nConnection: close\r\n");
    for (name, value) in extra {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str("\r\n");
    let _ = stream.write_all(response.as_bytes()).await;
    let body = if incomplete {
        &body[..body.len() / 2]
    } else {
        &body
    };
    let _ = stream.write_all(body).await;
    let _ = stream.shutdown().await;
}

#[test]
fn headers_validate_names_values_duplicates_and_reserved_protocol_headers() {
    let headers =
        webdav::parse_headers("Authorization: Bearer synthetic\r\nX-Test: value:with:colons\n")
            .unwrap();
    assert_eq!(headers["authorization"], "Bearer synthetic");
    assert_eq!(headers["x-test"], "value:with:colons");
    assert!(webdav::parse_headers(" \n\r\n").unwrap().is_empty());
    for invalid in [
        "Bad Name: v",
        "X: a\nx: b",
        "X: a\ry: b",
        "X: a\n folded: b",
        "no-colon",
        ": value",
        "X: a\0b",
    ] {
        assert!(webdav::parse_headers(invalid).is_err(), "应拒绝非法 header");
    }
    for name in [
        "Host",
        "Content-Length",
        "Content-Type",
        "Accept-Encoding",
        "If-Match",
        "If-None-Match",
        "Depth",
        "Destination",
        "Overwrite",
        "Range",
        "Connection",
        "Transfer-Encoding",
    ] {
        assert!(webdav::parse_headers(&format!("{name}: override")).is_err());
    }
}

#[test]
fn urls_reject_credentials_query_fragment_and_non_http() {
    for url in [
        "https://user:pw@example.test/dav",
        "https://@example.test/",
        "https://example.test/?token=x",
        "https://example.test/#x",
        "ftp://example.test/",
        "file:///tmp/x",
        "https://example.test/\r\nX: y",
    ] {
        assert!(webdav::normalize_url(url).is_err());
    }
    assert_eq!(
        webdav::normalize_url("https://EXAMPLE.test/dav").unwrap(),
        "https://example.test/dav/"
    );
    assert!(webdav::normalize_url("").unwrap().is_empty());
}

#[test]
fn crypto_roundtrip_full_connection_fields_and_no_plaintext_in_envelope() {
    let bytes = sealed_fixture();
    let open = crypto::open(&bytes, PASSPHRASE).unwrap();
    assert!(open == fixture());
    let text = String::from_utf8(bytes).unwrap();
    for secret in [DB_SECRET, PARAM_SECRET, PASSPHRASE, "synthetic-driver"] {
        assert!(!text.contains(secret));
    }
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["iterations"], 600000);
    assert_eq!(value["version"], 1);
    assert_eq!(value["kdf"], "pbkdf2-sha256");
}

#[test]
fn crypto_rejects_wrong_passphrase_ciphertext_and_authenticated_metadata_tampering() {
    let bytes = sealed_fixture();
    assert!(crypto::open(&bytes, "wrong-password").is_err());
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let data = value["data"].as_str().unwrap();
    use base64::Engine;
    let mut sealed = base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap();
    sealed[0] ^= 1;
    value["data"] = base64::engine::general_purpose::STANDARD
        .encode(sealed)
        .into();
    assert!(crypto::open(&serde_json::to_vec(&value).unwrap(), PASSPHRASE).is_err());
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["salt"] = base64::engine::general_purpose::STANDARD
        .encode([42u8; 16])
        .into();
    assert!(crypto::open(&serde_json::to_vec(&value).unwrap(), PASSPHRASE).is_err());
}

#[test]
fn crypto_rejects_untrusted_kdf_size_format_and_truncation_before_derivation() {
    let bytes = sealed_fixture();
    let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    for (key, value) in [
        ("iterations", serde_json::json!(1)),
        ("iterations", serde_json::json!(10000000)),
        ("version", serde_json::json!(2)),
        ("nonce", serde_json::json!("bad")),
        ("salt", serde_json::json!("bad")),
        ("format", serde_json::json!("other-app")),
    ] {
        let mut v = original.clone();
        v[key] = value;
        assert!(crypto::open(&serde_json::to_vec(&v).unwrap(), PASSPHRASE).is_err());
    }
    assert!(crypto::open(&bytes[..bytes.len() / 2], PASSPHRASE).is_err());
    assert!(crypto::inspect_envelope(&vec![0; crypto::MAX_REMOTE_BYTES + 1]).is_err());
}

#[test]
fn first_sync_keeps_distinct_ids_and_warns_same_names() {
    let mut l = snapshot(&["a"]);
    let mut r = snapshot(&["b"]);
    l.connections[0].name = "same".into();
    r.connections[0].name = "same".into();
    let result = merge::merge(None, &l, &r, &BTreeMap::new()).unwrap();
    assert_eq!(result.snapshot.connections.len(), 2);
    assert!(result.conflicts.is_empty());
    assert!(!result.warnings.is_empty());
    assert!(result.snapshot.connections.iter().any(|c| c.id == "a"));
    assert!(result.snapshot.connections.iter().any(|c| c.id == "b"));
}

#[test]
fn three_way_merges_one_sided_changes_and_preserves_empty_groups() {
    let base = snapshot(&["a", "b"]);
    let mut l = base.clone();
    let mut r = base.clone();
    l.connections[0].host = "local.example".into();
    r.connections[1].database = "remote-db".into();
    let result = merge::merge(Some(&base), &l, &r, &BTreeMap::new()).unwrap();
    assert!(result.conflicts.is_empty());
    assert_eq!(result.snapshot.connections[0].host, "local.example");
    assert_eq!(result.snapshot.connections[1].database, "remote-db");
    assert!(result.snapshot.groups.contains(&"empty".into()));
}

#[test]
fn concurrent_same_connection_edits_require_a_choice_without_secret_summaries() {
    let base = snapshot(&["a"]);
    let mut l = base.clone();
    let mut r = base.clone();
    l.connections[0].password = "local-password".into();
    r.connections[0].params = "password=remote-parameter-password".into();
    let result = merge::merge(Some(&base), &l, &r, &BTreeMap::new()).unwrap();
    assert_eq!(result.conflicts.len(), 1);
    let summary = serde_json::to_string(&result.conflicts).unwrap();
    for secret in [
        DB_SECRET,
        "local-password",
        "remote-parameter-password",
        PARAM_SECRET,
    ] {
        assert!(!summary.contains(secret));
    }
    let choose = [("connection:a".into(), Choice::Remote)].into();
    let selected = merge::merge(Some(&base), &l, &r, &choose).unwrap();
    assert_eq!(
        selected.snapshot.connections[0].params,
        r.connections[0].params
    );
}

#[test]
fn one_sided_deletion_produces_tombstone_and_delete_edit_conflict_is_explicit() {
    let base = snapshot(&["a"]);
    let mut l = base.clone();
    l.connections.clear();
    l.tombstones.insert("a".into());
    let result = merge::merge(Some(&base), &l, &base, &BTreeMap::new()).unwrap();
    assert!(result.snapshot.connections.is_empty());
    assert!(result.snapshot.tombstones.contains("a"));
    assert!(result.conflicts.is_empty());
    let mut r = base.clone();
    r.connections[0].host = "changed".into();
    let result = merge::merge(Some(&base), &l, &r, &BTreeMap::new()).unwrap();
    assert_eq!(result.conflicts.len(), 1);
    let result = merge::merge(
        Some(&base),
        &l,
        &r,
        &[("connection:a".into(), Choice::Remote)].into(),
    )
    .unwrap();
    assert_eq!(result.snapshot.connections[0].host, "changed");
    assert!(!result.snapshot.tombstones.contains("a"));
}

#[test]
fn no_baseline_offline_device_cannot_silently_revive_a_tombstoned_id() {
    let l = snapshot(&["a"]);
    let mut r = Snapshot::empty();
    r.tombstones.insert("a".into());
    let result = merge::merge(None, &l, &r, &BTreeMap::new()).unwrap();
    assert!(result.snapshot.connections.is_empty());
    assert_eq!(result.conflicts.len(), 1);
    let result = merge::merge(
        None,
        &l,
        &r,
        &[("connection:a".into(), Choice::Local)].into(),
    )
    .unwrap();
    assert_eq!(result.snapshot.connections.len(), 1);
    assert!(!result.snapshot.tombstones.contains("a"));
}

#[test]
fn concurrent_reorders_in_same_group_conflict_and_choice_preserves_order() {
    let base = snapshot(&["a", "b", "c"]);
    let mut l = base.clone();
    let mut r = base.clone();
    l.connections.swap(0, 1);
    r.connections.swap(1, 2);
    let result = merge::merge(Some(&base), &l, &r, &BTreeMap::new()).unwrap();
    assert!(result
        .conflicts
        .iter()
        .any(|c| c.key == merge::order_key("folder")));
    let result = merge::merge(
        Some(&base),
        &l,
        &r,
        &[(merge::order_key("folder"), Choice::Remote)].into(),
    )
    .unwrap();
    assert_eq!(
        result
            .snapshot
            .connections
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "c", "b"]
    );
}

#[test]
fn independent_group_orders_and_distinct_insertions_merge_without_losing_ids() {
    let mut base = snapshot(&["a", "b", "c", "d"]);
    base.groups.push("second".into());
    base.connections[2].group = "second".into();
    base.connections[3].group = "second".into();
    let mut l = base.clone();
    let mut r = base.clone();
    l.connections.swap(0, 1);
    r.connections.swap(2, 3);
    let result = merge::merge(Some(&base), &l, &r, &BTreeMap::new()).unwrap();
    assert!(result.conflicts.is_empty());
    assert_eq!(
        result
            .snapshot
            .connections
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        vec!["b", "a", "d", "c"]
    );
    let base = snapshot(&["a", "b"]);
    let mut l = base.clone();
    let mut r = base.clone();
    l.connections.insert(1, cfg("x", "folder"));
    r.connections.insert(1, cfg("y", "folder"));
    let result = merge::merge(Some(&base), &l, &r, &BTreeMap::new()).unwrap();
    assert!(result.conflicts.is_empty());
    assert_eq!(
        result
            .snapshot
            .connections
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "x", "y", "b"]
    );
}

#[test]
fn rename_and_related_remote_edit_raise_conservative_organization_conflict() {
    let base = snapshot(&["a"]);
    let mut l = base.clone();
    let mut r = base.clone();
    l.groups[0] = "renamed".into();
    l.connections[0].group = "renamed".into();
    r.connections[0].host = "remote-change".into();
    let result = merge::merge(Some(&base), &l, &r, &BTreeMap::new()).unwrap();
    assert!(result.conflicts.iter().any(|c| c.key == "organization"));
    let result = merge::merge(
        Some(&base),
        &l,
        &r,
        &[
            ("organization".into(), Choice::Local),
            ("connection:a".into(), Choice::Remote),
        ]
        .into(),
    )
    .unwrap();
    assert_eq!(result.snapshot.connections[0].host, "remote-change");
    assert_eq!(result.snapshot.connections[0].group, "renamed");
    assert!(!result.snapshot.groups.contains(&"folder".into()));
}

#[test]
fn empty_group_concurrent_renames_conflict_but_independent_additions_are_kept() {
    let base = snapshot(&[]);
    let mut l = base.clone();
    let mut r = base.clone();
    l.groups[1] = "local-empty".into();
    r.groups[1] = "remote-empty".into();
    assert!(merge::merge(Some(&base), &l, &r, &BTreeMap::new())
        .unwrap()
        .conflicts
        .iter()
        .any(|c| c.key == "organization"));
    let mut l = base.clone();
    let mut r = base.clone();
    l.groups.push("local-add".into());
    r.groups.push("remote-add".into());
    let result = merge::merge(Some(&base), &l, &r, &BTreeMap::new()).unwrap();
    assert!(result.conflicts.is_empty());
    assert_eq!(result.snapshot.groups.len(), 4);
}

#[test]
fn snapshot_rejects_duplicate_ids_invalid_group_references_and_tombstone_overlap() {
    let mut s = snapshot(&["a", "a"]);
    assert!(s.validate().is_err());
    s = snapshot(&["a"]);
    s.groups.clear();
    assert!(s.validate().is_err());
    s = snapshot(&["a"]);
    s.tombstones.insert("a".into());
    assert!(s.validate().is_err());
}

#[test]
fn local_revision_and_freeze_prevent_stale_or_parallel_connection_writes() {
    let temp = Temp::new();
    let store = temp.store();
    local_save(&store, &snapshot(&["a"]));
    let first = store.snapshot_checked().unwrap();
    store
        .update_connections(|c, _| {
            c[0].name = "edited".into();
            Ok(())
        })
        .unwrap();
    assert!(store.reserve_connections(first.revision).is_err());
    let current = store.snapshot_checked().unwrap();
    let reservation = store.reserve_connections(current.revision).unwrap();
    assert!(store
        .update_connections(|c, _| {
            c.clear();
            Ok(())
        })
        .is_err());
    assert_eq!(store.connections().len(), 1);
    drop(reservation);
    store
        .update_connections(|c, _| {
            c.clear();
            Ok(())
        })
        .unwrap();
    assert!(store.connections().is_empty());
}

#[test]
fn atomic_write_failure_and_external_disk_change_do_not_advance_memory_revision() {
    let temp = Temp::new();
    let blocked = temp.0.join("blocked");
    fs::create_dir(&blocked).unwrap();
    assert!(storage::atomic_write(&blocked, b"synthetic").is_err());
    assert!(blocked.is_dir());
    assert!(!fs::read_dir(&temp.0).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".tmp")));
    let store = temp.store();
    local_save(&store, &snapshot(&["a"]));
    let revision = store.snapshot_checked().unwrap().revision;
    fs::write(
        temp.0.join("connections.json"),
        b"external synthetic change",
    )
    .unwrap();
    assert!(store
        .update_connections(|c, _| {
            c.clear();
            Ok(())
        })
        .is_err());
    assert_eq!(store.connections().len(), 1);
    assert!(store.reserve_connections(revision).is_err());
}

#[test]
fn local_decryption_failure_never_becomes_empty_password_or_creates_replacement_key() {
    let temp = Temp::new();
    let store = temp.store();
    assert_eq!(store.decrypt_password("").unwrap(), "");
    assert!(store.decrypt_password("invalid-base64").is_err());
    assert!(!temp.0.join("key.bin").exists());
    let password = store.encrypt_password(DB_SECRET).unwrap();
    assert_eq!(store.decrypt_password(&password).unwrap(), DB_SECRET);
    fs::write(temp.0.join("key.bin"), [1u8; 32]).unwrap();
    assert!(store.decrypt_password(&password).is_err());
    fs::write(temp.0.join("key.bin"), b"corrupt").unwrap();
    assert!(store.encrypt_password("new").is_err());
    assert_eq!(fs::read(temp.0.join("key.bin")).unwrap(), b"corrupt");
}

#[test]
fn local_config_and_baseline_are_encrypted_and_secret_flags_are_only_booleans() {
    let temp = Temp::new();
    let store = temp.store();
    let service = configured(&store, "https://synthetic.example/dav/");
    set_baseline(&service, &store, fixture());
    let bytes = fs::read(temp.0.join("datasource_sync.enc")).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    for secret in [
        "synthetic-webdav-password",
        "synthetic-header-token",
        PASSPHRASE,
        DB_SECRET,
        PARAM_SECRET,
    ] {
        assert!(!text.contains(secret));
    }
    let loaded = load_state(&temp.0).unwrap();
    assert_eq!(loaded.config.passphrase, PASSPHRASE);
    assert!(loaded.baseline.unwrap() == fixture());
    let view = serde_json::to_value(service.get_config().unwrap()).unwrap();
    assert_eq!(view["password_set"], true);
    assert_eq!(view["headers_set"], true);
    assert!(view.get("password").is_none());
    assert!(view.get("passphrase").is_none());
    assert!(view.get("headers").is_none());
}

#[test]
fn remember_false_keeps_every_secret_only_in_session_and_discards_baseline_after_restart() {
    let temp = Temp::new();
    let store = temp.store();
    let service = Service::new(&store);
    service
        .set_config(&store, input("https://synthetic.example/dav/", false))
        .unwrap();
    set_baseline(&service, &store, fixture());
    assert!(service.get_config().unwrap().password_set);
    assert!(service.get_config().unwrap().passphrase_set);
    assert!(service.get_config().unwrap().headers_set);
    let restarted = Service::new(&store);
    let view = restarted.get_config().unwrap();
    assert!(!view.password_set && !view.passphrase_set && !view.headers_set);
    assert!(restarted.lock().unwrap().stored.baseline.is_none());
    assert!(restarted.lock().unwrap().stored.pending.is_none());
    assert!(view.last_sync.is_none());
}

#[test]
fn endpoint_changes_isolate_secrets_and_none_preserves_but_empty_explicitly_clears() {
    let temp = Temp::new();
    let store = temp.store();
    let service = configured(&store, "https://synthetic.example/dav/");
    let mut config = input("https://synthetic.example/dav/", true);
    config.password = None;
    config.passphrase = None;
    config.headers = None;
    let view = service.set_config(&store, config).unwrap();
    assert!(view.password_set && view.passphrase_set && view.headers_set);
    set_baseline(&service, &store, fixture());
    let mut config = input("https://another-synthetic.example/dav/", true);
    config.password = None;
    config.passphrase = None;
    config.headers = None;
    let view = service.set_config(&store, config).unwrap();
    assert!(!view.password_set && !view.passphrase_set && !view.headers_set);
    assert!(service.lock().unwrap().stored.baseline.is_none());
    service
        .set_config(
            &store,
            input("https://another-synthetic.example/dav/", true),
        )
        .unwrap();
    let mut config = input("https://another-synthetic.example/dav/", true);
    config.user = "another-user".into();
    config.password = None;
    config.passphrase = None;
    config.headers = None;
    let view = service.set_config(&store, config).unwrap();
    assert!(!view.password_set && !view.passphrase_set && !view.headers_set);
    let mut config = input("", false);
    config.password = Some("".into());
    config.passphrase = Some("".into());
    config.headers = Some("".into());
    assert!(service.set_config(&store, config).unwrap().url.is_empty());
}

#[test]
fn config_write_failure_and_corrupt_state_refuse_overwrite() {
    let temp = Temp::new();
    let store = temp.store();
    let service = configured(&store, "https://synthetic.example/dav/");
    fs::remove_file(temp.0.join("datasource_sync.enc")).unwrap();
    fs::create_dir(temp.0.join("datasource_sync.enc")).unwrap();
    assert!(service
        .set_config(&store, input("https://new-synthetic.example/dav/", true))
        .is_err());
    assert_eq!(
        service.get_config().unwrap().url,
        "https://synthetic.example/dav/"
    );
    fs::remove_dir(temp.0.join("datasource_sync.enc")).unwrap();
    fs::write(temp.0.join("datasource_sync.enc"), b"corrupt").unwrap();
    let restarted = Service::new(&store);
    assert!(restarted.get_config().is_err());
    assert!(restarted.set_config(&store, input("", false)).is_err());
    assert_eq!(
        fs::read(temp.0.join("datasource_sync.enc")).unwrap(),
        b"corrupt"
    );
}

#[tokio::test]
async fn dav_test_is_read_only_and_needs_no_passphrase() {
    let mock = Mock::new(None).await;
    let temp = Temp::new();
    let store = temp.store();
    let service = configured(&store, &mock.url);
    let mut config = input(&mock.url, true);
    config.passphrase = Some("".into());
    service.set_config(&store, config).unwrap();
    assert!(service.test().await.unwrap().contains("只读"));
    assert!(service.preview(&store).await.is_err());
    assert!(mock
        .requests()
        .iter()
        .all(|r| r.method == "GET" || r.method == "PROPFIND"));
    assert!(mock.data().is_none());
}

#[tokio::test]
async fn explicit_authorization_overrides_basic_and_identity_is_sent() {
    let mock = Mock::new(Some(b"encrypted synthetic body".to_vec())).await;
    let dav = Dav::new(
        &mock.url,
        "u",
        "p",
        "Authorization: Bearer synthetic-token",
        Arc::new(Mutex::new(None)),
    )
    .unwrap();
    dav.read().await.unwrap();
    let requests = mock.requests();
    assert_eq!(
        requests[0].headers["authorization"],
        "Bearer synthetic-token"
    );
    assert_eq!(requests[0].headers["accept-encoding"], "identity");
    assert!(!requests[0].headers["authorization"].contains("Basic"));
}

#[tokio::test]
async fn dav_first_write_makes_only_validated_missing_dirs_and_uses_if_none_match() {
    let mock = Mock::new(None).await;
    mock.edit(|s| {
        s.dirs.remove("/dav/");
    });
    let dav = mock.dav();
    assert!(dav.test().await.is_err());
    let observed = dav.read().await.unwrap();
    assert_eq!(observed.missing_dirs.len(), 2);
    let kept = dav.put_verified(&observed, b"encrypted-new").await.unwrap();
    assert!(kept.etag.is_some());
    let requests = mock.requests();
    let mkcol: Vec<_> = requests
        .iter()
        .filter(|r| r.method == "MKCOL")
        .map(|r| r.path.as_str())
        .collect();
    assert_eq!(mkcol, vec!["/dav/", "/dav/navkit-sync/"]);
    let put = requests.iter().find(|r| r.method == "PUT").unwrap();
    assert_eq!(put.headers["if-none-match"], "*");
    assert!(!put.headers.contains_key("if-match"));
}

#[tokio::test]
async fn dav_existing_write_requires_strong_if_match_and_verifies_content() {
    let mock = Mock::new(Some(b"encrypted-old".to_vec())).await;
    let dav = mock.dav();
    let observed = dav.read().await.unwrap();
    dav.put_verified(&observed, b"encrypted-new").await.unwrap();
    let requests = mock.requests();
    let put = requests.iter().find(|r| r.method == "PUT").unwrap();
    assert_eq!(put.headers["if-match"], "\"v1\"");
    assert!(!put.headers.contains_key("if-none-match"));
    assert_eq!(mock.data().unwrap(), b"encrypted-new");
}

#[tokio::test]
async fn dav_412_never_overwrites_or_retries_unconditionally() {
    let mock = Mock::new(Some(b"original".to_vec())).await;
    mock.edit(|s| s.force_412 = true);
    let dav = mock.dav();
    let observed = dav.read().await.unwrap();
    assert!(message(dav.put_verified(&observed, b"attempt").await).contains("412"));
    assert_eq!(mock.data().unwrap(), b"original");
    assert_eq!(
        mock.requests().iter().filter(|r| r.method == "PUT").count(),
        1
    );
}

#[tokio::test]
async fn dav_missing_or_weak_etag_refuses_put() {
    for weak in [false, true] {
        let mock = Mock::new(Some(b"original".to_vec())).await;
        mock.edit(|s| {
            s.no_etag = !weak;
            s.weak_etag = weak;
        });
        let dav = mock.dav();
        let observed = dav.read().await.unwrap();
        assert!(observed.etag.is_none());
        assert!(dav.put_verified(&observed, b"attempt").await.is_err());
        assert!(mock.requests().iter().all(|r| r.method != "PUT"));
    }
}

#[tokio::test]
async fn dav_same_length_corruption_and_truncated_put_fail_hash_verification() {
    for truncate in [false, true] {
        let mock = Mock::new(Some(b"old".to_vec())).await;
        mock.edit(|s| {
            s.corrupt_put = !truncate;
            s.truncate_put = truncate;
        });
        let dav = mock.dav();
        let observed = dav.read().await.unwrap();
        assert!(
            message(dav.put_verified(&observed, b"encrypted-upload").await).contains("SHA-256")
        );
        assert_eq!(
            mock.requests().iter().filter(|r| r.method == "PUT").count(),
            1
        );
    }
}

#[tokio::test]
async fn dav_truncated_get_html_and_http_permissions_are_distinguished() {
    let mock = Mock::new(Some(b"encrypted-content".to_vec())).await;
    mock.edit(|s| s.truncated_response = true);
    assert!(mock.dav().read().await.is_err());
    let html = Mock::new(None).await;
    html.edit(|s| s.html = true);
    assert!(message(html.dav().read().await).contains("HTML"));
    for code in [401, 403, 409, 410] {
        let mock = Mock::new(None).await;
        mock.edit(|s| s.get_status = Some(code));
        assert!(message(mock.dav().read().await).contains(&code.to_string()));
        assert!(mock.requests().iter().all(|r| r.method != "MKCOL"));
    }
    let mock = Mock::new(None).await;
    mock.edit(|s| s.directory_status = Some(403));
    assert!(message(mock.dav().read().await).contains("403"));
    assert!(mock.requests().iter().all(|r| r.method != "MKCOL"));
}

#[tokio::test]
async fn dav_mkcol_permission_denial_is_never_interpreted_as_missing_parent() {
    let mock = Mock::new(None).await;
    mock.edit(|s| s.mkcol_status = Some(403));
    let dav = mock.dav();
    let observed = dav.read().await.unwrap();
    assert!(message(dav.put_verified(&observed, b"encrypted-upload").await).contains("403"));
    assert!(mock.requests().iter().all(|r| r.method != "PUT"));
    assert!(mock.data().is_none());
}

#[tokio::test]
async fn dav_cross_origin_redirect_sends_no_credentials_to_destination_and_login_html_is_rejected()
{
    let destination = Mock::new(None).await;
    let source = Mock::new(None).await;
    source.edit(|s| s.redirect = Some(destination.url.clone()));
    assert!(message(source.dav().read().await).contains("origin"));
    assert!(destination.requests().is_empty());
    source.edit(|s| s.redirect = Some("/login".into()));
    assert!(message(source.dav().read().await).contains("HTML"));
    let mock = Mock::new(Some(b"old".to_vec())).await;
    mock.edit(|s| s.put_redirect = true);
    let dav = mock.dav();
    let observed = dav.read().await.unwrap();
    assert!(dav.put_verified(&observed, b"new").await.is_err());
    assert_eq!(mock.data().unwrap(), b"old");
}

#[tokio::test]
async fn retry_after_429_and_503_prevent_immediate_retries() {
    for status in [429, 503] {
        let mock = Mock::new(None).await;
        mock.edit(|s| {
            s.get_status = Some(status);
            s.retry_after = Some("120".into());
        });
        let dav = mock.dav();
        let error = message(dav.read().await);
        assert!(error.contains(&status.to_string()) && error.contains("120"));
        assert!(message(dav.read().await).contains("Retry-After"));
        assert_eq!(mock.requests().len(), 1);
    }
}

#[test]
fn propstat_404_is_not_resource_404_and_directory_response_must_match_target() {
    // 本测试走 Dav 的真实 parser，通过模块内测试辅助入口。
    let url = url::Url::parse("http://127.0.0.1/dav/").unwrap();
    let invalid = b"<d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>/dav/</d:href><d:propstat><d:prop><d:resourcetype/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response></d:multistatus>";
    assert!(webdav::validate_collection_for_test(invalid, &url).is_err());
    let wrong = b"<multistatus><response><href>/another/</href><status>HTTP/1.1 404 Not Found</status></response></multistatus>";
    assert!(webdav::validate_collection_for_test(wrong, &url).is_err());
    let malformed = b"<multistatus><response>";
    assert!(webdav::validate_collection_for_test(malformed, &url).is_err());
}

#[tokio::test]
async fn preview_returns_only_safe_shape_is_read_only_and_cancel_or_expiry_drops_plan() {
    let mock = Mock::new(Some(sealed_fixture())).await;
    let temp = Temp::new();
    let store = temp.store();
    let service = configured(&store, &mock.url);
    let before = fs::read(temp.0.join("datasource_sync.enc")).unwrap();
    let view = service.preview(&store).await.unwrap();
    assert!(view.remote_exists && view.requires_confirmation && view.has_changes);
    assert_eq!(view.summary.added, 1);
    assert!(view.conflicts.is_empty());
    let json = serde_json::to_string(&view).unwrap();
    for secret in [
        DB_SECRET,
        PARAM_SECRET,
        PASSPHRASE,
        "synthetic-header-token",
        "synthetic-webdav-password",
    ] {
        assert!(!json.contains(secret));
    }
    assert!(
        !json.contains("\"connections\"")
            && !json.contains("\"params\"")
            && !json.contains("\"password\"")
    );
    assert_eq!(
        fs::read(temp.0.join("datasource_sync.enc")).unwrap(),
        before
    );
    assert!(!temp.0.join("connections.json").exists());
    assert!(mock.requests().iter().all(|r| r.method == "GET"));
    service.cancel(&view.plan_id).unwrap();
    assert!(service.lock().unwrap().plan.is_none());
    let view = service.preview(&store).await.unwrap();
    service.lock().unwrap().plan.as_mut().unwrap().created = Instant::now() - PLAN_TTL;
    assert!(message(
        service
            .commit(&temp.state(), &view.plan_id, BTreeMap::new(), false)
            .await
    )
    .contains("过期"));
    assert!(service.lock().unwrap().plan.is_none());
}

#[tokio::test]
async fn first_existing_remote_and_conflicts_cannot_be_auto_committed_even_with_choices() {
    let mock = Mock::new(Some(sealed_fixture())).await;
    let temp = Temp::new();
    let state = temp.state();
    let service = configured(&state.store, &mock.url);
    let view = service.preview(&state.store).await.unwrap();
    assert!(message(
        service
            .commit(&state, &view.plan_id, BTreeMap::new(), true)
            .await
    )
    .contains("自动"));
    let mut local = fixture();
    local.connections[0].host = "local-change".into();
    local_save(&state.store, &local);
    let view = service.preview(&state.store).await.unwrap();
    assert!(!view.conflicts.is_empty());
    assert!(service
        .commit(&state, &view.plan_id, choices(&view, Choice::Remote), true)
        .await
        .is_err());
    assert!(mock.requests().iter().all(|r| r.method != "PUT"));
    assert!(service.get_config().unwrap().last_sync.is_none());
}

#[tokio::test]
async fn successful_commit_uploads_only_encrypted_datasources_and_returns_local_ciphertext() {
    let mock = Mock::new(None).await;
    let temp = Temp::new();
    let state = temp.state();
    local_save(&state.store, &fixture());
    let original_password = state.store.connections()[0].password.clone();
    let service = configured(&state.store, &mock.url);
    let view = service.preview(&state.store).await.unwrap();
    assert!(!view.requires_confirmation);
    let result = service
        .commit(&state, &view.plan_id, BTreeMap::new(), true)
        .await
        .unwrap();
    assert_eq!(result.connections.len(), 1);
    assert_eq!(result.connections[0].password, original_password);
    assert_eq!(
        state
            .store
            .decrypt_password(&result.connections[0].password)
            .unwrap(),
        DB_SECRET
    );
    assert!(result.groups.contains(&"empty".into()));
    assert_eq!(
        service.get_config().unwrap().last_sync.as_deref(),
        Some(result.last_sync.as_str())
    );
    assert!(service.lock().unwrap().stored.pending.is_none());
    let remote = mock.data().unwrap();
    assert!(crypto::open(&remote, PASSPHRASE).unwrap() == fixture());
    let text = String::from_utf8_lossy(&remote);
    assert!(!text.contains(DB_SECRET) && !text.contains(PARAM_SECRET));
    let body = serde_json::to_value(result).unwrap();
    assert!(
        body.get("changed_ids").is_some()
            && body.get("deleted_ids").is_some()
            && body.get("last_sync").is_some()
    );
}

#[tokio::test]
async fn preview_local_revision_or_remote_version_change_rejects_commit_without_put() {
    let mock = Mock::new(None).await;
    let temp = Temp::new();
    let state = temp.state();
    local_save(&state.store, &fixture());
    let service = configured(&state.store, &mock.url);
    let view = service.preview(&state.store).await.unwrap();
    state
        .store
        .update_connections(|c, _| {
            c[0].name = "changed after preview".into();
            Ok(())
        })
        .unwrap();
    assert!(message(
        service
            .commit(&state, &view.plan_id, BTreeMap::new(), false)
            .await
    )
    .contains("预览"));
    let view = service.preview(&state.store).await.unwrap();
    mock.edit(|s| {
        s.data = Some(sealed_fixture());
        s.version += 1;
    });
    assert!(message(
        service
            .commit(&state, &view.plan_id, BTreeMap::new(), false)
            .await
    )
    .contains("远端"));
    assert!(mock.requests().iter().all(|r| r.method != "PUT"));
}

#[tokio::test]
async fn busy_database_gate_rejects_commit_without_clearing_registry_or_writing_remote() {
    let mock = Mock::new(None).await;
    let temp = Temp::new();
    let state = temp.state();
    local_save(&state.store, &fixture());
    let service = configured(&state.store, &mock.url);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    state
        .registry
        .write()
        .insert("unrelated".into(), Arc::new(Db::Sqlite(pool)));
    let view = service.preview(&state.store).await.unwrap();
    let activity = state.activity.enter().unwrap();
    assert!(message(
        service
            .commit(&state, &view.plan_id, BTreeMap::new(), false)
            .await
    )
    .contains("运行"));
    assert!(state.registry.read().contains_key("unrelated"));
    assert!(mock.requests().iter().all(|r| r.method != "PUT"));
    drop(activity);
}

#[tokio::test]
async fn active_manual_transaction_is_left_active_and_never_implicitly_finished() {
    let mock = Mock::new(Some(sealed_fixture())).await;
    let temp = Temp::new();
    let state = temp.state();
    let mut local = fixture();
    local.connections[0].host = "local-host".into();
    local_save(&state.store, &local);
    let service = configured(&state.store, &mock.url);
    let id = local.connections[0].id.clone();
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(2)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let db = Db::Sqlite(pool);
    {
        let mut lease = state.manual_transactions.lease(&id).await;
        lease
            .start(&db, DbKind::of("sqlite"), "main".into(), "owner".into())
            .await
            .unwrap();
    }
    let view = service.preview(&state.store).await.unwrap();
    assert!(message(
        service
            .commit(&state, &view.plan_id, choices(&view, Choice::Remote), false)
            .await
    )
    .contains("事务"));
    let mut lease = state.manual_transactions.lease(&id).await;
    assert!(lease.status().active);
    lease.finish("owner", false).await.unwrap();
    assert!(mock.requests().iter().all(|r| r.method != "PUT"));
}

#[tokio::test]
async fn remote_disappearance_and_known_deletion_require_confirmation_in_backend() {
    let mock = Mock::new(None).await;
    let temp = Temp::new();
    let state = temp.state();
    local_save(&state.store, &fixture());
    let service = configured(&state.store, &mock.url);
    set_baseline(&service, &state.store, fixture());
    let view = service.preview(&state.store).await.unwrap();
    assert!(view.conflicts.iter().any(|c| c.key == "remote_missing"));
    assert!(view.requires_confirmation);
    assert!(service
        .commit(&state, &view.plan_id, choices(&view, Choice::Local), true)
        .await
        .is_err());
    mock.edit(|s| s.data = Some(encrypted_empty()));
    let view = service.preview(&state.store).await.unwrap();
    assert!(view.requires_confirmation && view.summary.deleted == 1);
    assert!(service
        .commit(&state, &view.plan_id, BTreeMap::new(), true)
        .await
        .is_err());
    assert!(mock.requests().iter().all(|r| r.method != "PUT"));
    assert_eq!(state.store.connections().len(), 1);
}

#[tokio::test]
async fn wrong_passphrase_or_broken_local_password_stops_before_any_write() {
    let mock = Mock::new(Some(sealed_fixture())).await;
    let temp = Temp::new();
    let state = temp.state();
    let service = configured(&state.store, &mock.url);
    let mut config = input(&mock.url, true);
    config.passphrase = Some("incorrect-but-long".into());
    service.set_config(&state.store, config).unwrap();
    assert!(service.preview(&state.store).await.is_err());
    assert!(service.lock().unwrap().plan.is_none());
    let mut c = fixture().connections[0].clone();
    c.password = "broken-ciphertext".into();
    state
        .store
        .save_connections(vec![c], fixture().groups.clone())
        .unwrap();
    service
        .set_config(&state.store, input(&mock.url, true))
        .unwrap();
    assert!(service.preview(&state.store).await.is_err());
    assert!(mock
        .requests()
        .iter()
        .all(|r| r.method != "PUT" && r.method != "MKCOL"));
}

#[tokio::test]
async fn conflict_choices_must_cover_exact_preview_keys_and_config_changes_invalidate_plan() {
    let mock = Mock::new(Some(sealed_fixture())).await;
    let temp = Temp::new();
    let state = temp.state();
    let mut local = fixture();
    local.connections[0].host = "local-change".into();
    local_save(&state.store, &local);
    let service = configured(&state.store, &mock.url);
    let view = service.preview(&state.store).await.unwrap();
    assert!(service
        .commit(&state, &view.plan_id, BTreeMap::new(), false)
        .await
        .is_err());
    let view = service.preview(&state.store).await.unwrap();
    let mut selected = choices(&view, Choice::Remote);
    selected.insert("unknown-key".into(), Choice::Local);
    assert!(service
        .commit(&state, &view.plan_id, selected, false)
        .await
        .is_err());
    let view = service.preview(&state.store).await.unwrap();
    service
        .set_config(&state.store, input(&mock.url, true))
        .unwrap();
    assert!(service
        .commit(&state, &view.plan_id, choices(&view, Choice::Remote), false)
        .await
        .is_err());
    assert!(mock.requests().iter().all(|r| r.method != "PUT"));
}

#[tokio::test]
async fn single_flight_rejects_another_preview_and_configuration_change() {
    let mock = Mock::new(None).await;
    let temp = Temp::new();
    let store = temp.store();
    let service = configured(&store, &mock.url);
    let guard = service.operation.lock().await;
    assert!(service.preview(&store).await.is_err());
    assert!(service.test().await.is_err());
    assert!(service.set_config(&store, input(&mock.url, true)).is_err());
    assert!(mock.requests().is_empty());
    drop(guard);
}

#[tokio::test]
async fn commit_holds_activity_and_connection_freeze_for_the_entire_put_phase() {
    let mock = Mock::new(None).await;
    let temp = Temp::new();
    let state = Arc::new(temp.state());
    local_save(&state.store, &fixture());
    let service = Arc::new(configured(&state.store, &mock.url));
    let view = service.preview(&state.store).await.unwrap();
    let arrived = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    mock.edit(|s| s.pause_put = Some((arrived.clone(), release.clone())));
    let commit_state = state.clone();
    let commit_service = service.clone();
    let task = tokio::spawn(async move {
        commit_service
            .commit(&commit_state, &view.plan_id, BTreeMap::new(), false)
            .await
    });
    tokio::time::timeout(Duration::from_secs(60), arrived.notified())
        .await
        .unwrap();
    assert!(state.activity.enter().is_err());
    assert!(state
        .store
        .update_connections(|c, _| {
            c.clear();
            Ok(())
        })
        .is_err());
    release.notify_one();
    assert!(task.await.unwrap().is_ok());
    assert_eq!(state.store.connections().len(), 1);
}

#[tokio::test]
async fn organization_only_changes_keep_base_and_derived_pools_and_ciphertext_password() {
    let mock = Mock::new(Some(sealed_fixture())).await;
    let temp = Temp::new();
    let state = temp.state();
    let mut local = fixture();
    local.connections[0].group = "old-folder".into();
    local.groups[0] = "old-folder".into();
    local_save(&state.store, &local);
    let service = configured(&state.store, &mock.url);
    set_baseline(&service, &state.store, local.clone());
    let id = local.connections[0].id.clone();
    let derived = format!("{id}@main");
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let db = Arc::new(Db::Sqlite(pool));
    state.registry.write().insert(id.clone(), db.clone());
    state.registry.write().insert(derived.clone(), db.clone());
    let password = state.store.connections()[0].password.clone();
    let view = service.preview(&state.store).await.unwrap();
    assert!(view.conflicts.is_empty());
    let result = service
        .commit(&state, &view.plan_id, BTreeMap::new(), false)
        .await
        .unwrap();
    assert_eq!(result.connections[0].group, "folder");
    assert_eq!(result.connections[0].password, password);
    assert!(Arc::ptr_eq(&db, state.registry.read().get(&id).unwrap()));
    assert!(state.registry.read().contains_key(&derived));
}

#[cfg(windows)]
#[tokio::test]
async fn remote_written_local_atomic_failure_preserves_baseline_and_recovers_on_fresh_preview() {
    let mock = Mock::new(Some(sealed_fixture())).await;
    let temp = Temp::new();
    let state = temp.state();
    local_save(&state.store, &snapshot(&["local-only"]));
    let service = configured(&state.store, &mock.url);
    let view = service.preview(&state.store).await.unwrap();
    assert!(view.conflicts.is_empty());
    let revision = state.store.snapshot_checked().unwrap().revision;
    let path = temp.0.join("connections.json");
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions).unwrap();
    let error = message(
        service
            .commit(&state, &view.plan_id, BTreeMap::new(), false)
            .await,
    );
    assert!(error.contains("恢复"));
    assert_eq!(state.store.connections().len(), 1);
    assert!(service.lock().unwrap().stored.baseline.is_none());
    assert!(service.lock().unwrap().stored.pending.is_some());
    assert!(service.get_config().unwrap().last_sync.is_none());
    assert_eq!(state.store.snapshot_checked().unwrap().revision, revision);
    assert_eq!(
        crypto::open(&mock.data().unwrap(), PASSPHRASE)
            .unwrap()
            .connections
            .len(),
        2
    );
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(false);
    fs::set_permissions(&path, permissions).unwrap();
    state
        .store
        .update_connections(|c, _| {
            c[0].name = "失败后继续编辑".into();
            Ok(())
        })
        .unwrap();
    // 模拟重启：从临时连接文件与加密日志重新加载，而不是仅依赖内存恢复。
    let state = AppState {
        store: Store::load_at(&temp.0),
        registry: Arc::new(DbRegistry::default()),
        manual_transactions: Default::default(),
        activity: Default::default(),
    };
    let service = Service::new(&state.store);
    let view = service.preview(&state.store).await.unwrap();
    assert!(view.requires_confirmation);
    assert!(view.conflicts.is_empty());
    let result = service
        .commit(&state, &view.plan_id, BTreeMap::new(), false)
        .await
        .unwrap();
    assert_eq!(result.connections.len(), 2);
    assert!(service.lock().unwrap().stored.pending.is_none());
    assert_eq!(
        result
            .connections
            .iter()
            .find(|c| c.id == "local-only")
            .unwrap()
            .name,
        "失败后继续编辑"
    );
}

#[cfg(windows)]
#[tokio::test]
async fn success_baseline_write_failure_retains_journal_and_recovers_without_losing_applied_changes(
) {
    let mock = Mock::new(Some(sealed_fixture())).await;
    let temp = Temp::new();
    let state = temp.state();
    local_save(&state.store, &snapshot(&["local-only"]));
    let service = configured(&state.store, &mock.url);
    let view = service.preview(&state.store).await.unwrap();
    let path = temp.0.join("datasource_sync.enc");
    let hook_path = path.clone();
    mock.edit(|s| {
        s.put_hook = Some(Arc::new(move || {
            let mut permissions = fs::metadata(&hook_path).unwrap().permissions();
            permissions.set_readonly(true);
            fs::set_permissions(&hook_path, permissions).unwrap();
        }))
    });
    let applied = service
        .commit(&state, &view.plan_id, BTreeMap::new(), false)
        .await
        .unwrap();
    assert!(applied.recovery_required);
    assert!(applied.last_sync.is_empty());
    assert_eq!(applied.connections.len(), 2);
    assert!(applied.warnings.iter().any(|w| w.contains("基线")));
    assert_eq!(state.store.connections().len(), 2);
    assert!(service.lock().unwrap().stored.baseline.is_none());
    assert!(service.lock().unwrap().stored.pending.is_some());
    assert!(service.get_config().unwrap().last_sync.is_none());
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(false);
    fs::set_permissions(&path, permissions).unwrap();
    mock.edit(|s| s.put_hook = None);
    let view = service.preview(&state.store).await.unwrap();
    let result = service
        .commit(&state, &view.plan_id, BTreeMap::new(), false)
        .await
        .unwrap();
    assert_eq!(result.connections.len(), 2);
    assert!(service.lock().unwrap().stored.pending.is_none());
}

#[tokio::test]
async fn local_journal_write_failure_prevents_any_remote_write() {
    let mock = Mock::new(None).await;
    let temp = Temp::new();
    let state = temp.state();
    local_save(&state.store, &fixture());
    let service = configured(&state.store, &mock.url);
    let view = service.preview(&state.store).await.unwrap();
    fs::remove_file(temp.0.join("datasource_sync.enc")).unwrap();
    fs::create_dir(temp.0.join("datasource_sync.enc")).unwrap();
    assert!(service
        .commit(&state, &view.plan_id, BTreeMap::new(), false)
        .await
        .is_err());
    assert!(mock
        .requests()
        .iter()
        .all(|r| r.method != "PUT" && r.method != "MKCOL"));
    assert_eq!(state.store.connections().len(), 1);
    assert!(service.get_config().unwrap().last_sync.is_none());
}

#[tokio::test]
async fn put_verification_failure_retains_original_baseline_and_pending_recovery() {
    let mock = Mock::new(None).await;
    mock.edit(|s| s.corrupt_put = true);
    let temp = Temp::new();
    let state = temp.state();
    local_save(&state.store, &fixture());
    let service = configured(&state.store, &mock.url);
    let view = service.preview(&state.store).await.unwrap();
    assert!(message(
        service
            .commit(&state, &view.plan_id, BTreeMap::new(), false)
            .await
    )
    .contains("SHA-256"));
    assert!(service.lock().unwrap().stored.baseline.is_none());
    assert!(service.lock().unwrap().stored.pending.is_some());
    assert!(service.get_config().unwrap().last_sync.is_none());
    assert_eq!(state.store.connections().len(), 1);
    assert!(service.preview(&state.store).await.is_err());
}

#[test]
fn dependency_warnings_never_create_files_or_disclose_params() {
    let temp = Temp::new();
    let mut s = fixture();
    let missing = temp.0.join("missing.sqlite");
    s.connections[0].file = missing.to_string_lossy().into_owned();
    let warnings = dependency_warnings(&temp.0, &s);
    assert!(warnings.iter().any(|w| w.contains("文件不存在")));
    assert!(warnings.iter().any(|w| w.contains("驱动")));
    assert!(!warnings.join(" ").contains(PARAM_SECRET));
    assert!(!missing.exists());
}

#[test]
fn independent_sync_passphrase_is_required_and_explicit_empty_clears_same_endpoint_secrets() {
    let temp = Temp::new();
    let store = temp.store();
    let service = configured(&store, "https://synthetic.example/dav/");
    let mut config = service.config().unwrap();
    config.passphrase = "short".into();
    assert!(config.validate_sync().is_err());
    config.passphrase = config.password.clone();
    assert!(config.validate_sync().is_err());
    config.password.clear();
    config.passphrase = PASSPHRASE.into();
    config.headers = format!("Authorization: Bearer {PASSPHRASE}");
    assert!(config.validate_sync().is_err());
    use base64::Engine;
    config.headers = format!(
        "Authorization: Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("u:{PASSPHRASE}"))
    );
    assert!(config.validate_sync().is_err());
    let mut input = input("https://synthetic.example/dav/", true);
    input.password = Some("".into());
    input.passphrase = Some("".into());
    input.headers = Some("".into());
    let view = service.set_config(&store, input).unwrap();
    assert!(!view.password_set && !view.passphrase_set && !view.headers_set);
    assert!(crypto::seal(&Snapshot::empty(), "short").is_err());
}

#[test]
fn conflict_display_scrubs_parameter_secrets_even_when_copied_into_name_or_host() {
    let base = snapshot(&["a"]);
    let mut local = base.clone();
    let mut remote = base.clone();
    local.connections[0].name = format!("name-{PARAM_SECRET}");
    remote.connections[0].host = "https://u:uri-password@synthetic.example/path?token=other".into();
    let result = merge::merge(Some(&base), &local, &remote, &BTreeMap::new()).unwrap();
    let json = serde_json::to_string(&result.conflicts).unwrap();
    assert!(!json.contains(PARAM_SECRET) && !json.contains("uri-password"));
}

#[test]
fn shared_uri_and_device_paths_are_kept_as_text_without_dependency_network_access() {
    for path in [
        "\\\\synthetic-server\\share\\db.sqlite",
        "//synthetic-server/share/db.sqlite",
        "\\\\?\\C:\\db.sqlite",
        "http://synthetic.example/db.sqlite",
    ] {
        assert_eq!(local_file_available(path), None);
    }
    let temp = Temp::new();
    let mut s = fixture();
    s.connections[0].file = "\\\\synthetic-server\\share\\db.sqlite".into();
    let warnings = dependency_warnings(&temp.0, &s);
    assert!(warnings.iter().any(|w| w.contains("仅保留路径文本")));
    fs::write(temp.0.join("drivers.json"), br#"[{"id":"synthetic-driver","source":"file","target":"\\\\synthetic-server\\share\\driver.dll"}]"#).unwrap();
    assert!(dependency_warnings(&temp.0, &s)
        .iter()
        .any(|w| w.contains("缺少")));
}

#[tokio::test]
async fn backend_refuses_existing_remote_without_strong_etag_even_for_manual_commit() {
    let mock = Mock::new(Some(sealed_fixture())).await;
    mock.edit(|s| s.no_etag = true);
    let temp = Temp::new();
    let state = temp.state();
    let service = configured(&state.store, &mock.url);
    let view = service.preview(&state.store).await.unwrap();
    assert!(view.requires_confirmation);
    assert!(message(
        service
            .commit(&state, &view.plan_id, BTreeMap::new(), false)
            .await
    )
    .contains("ETag"));
    assert!(state.store.connections().is_empty());
    assert!(service.lock().unwrap().stored.baseline.is_none());
    assert!(mock.requests().iter().all(|r| r.method != "PUT"));
}

#[tokio::test]
async fn backend_412_leaves_baseline_and_local_state_unchanged_and_requires_new_preview() {
    let mock = Mock::new(None).await;
    mock.edit(|s| s.force_412 = true);
    let temp = Temp::new();
    let state = temp.state();
    local_save(&state.store, &fixture());
    let service = configured(&state.store, &mock.url);
    let view = service.preview(&state.store).await.unwrap();
    assert!(message(
        service
            .commit(&state, &view.plan_id, BTreeMap::new(), false)
            .await
    )
    .contains("412"));
    assert!(service.get_config().unwrap().last_sync.is_none());
    assert!(service.lock().unwrap().stored.baseline.is_none());
    assert!(service.lock().unwrap().stored.pending.is_some());
    assert!(service
        .commit(&state, &view.plan_id, BTreeMap::new(), false)
        .await
        .is_err());
    assert_eq!(
        mock.requests().iter().filter(|r| r.method == "PUT").count(),
        1
    );
}

#[test]
fn empty_group_deletion_is_recorded_and_offline_no_baseline_device_cannot_revive_it() {
    let base = snapshot(&[]);
    let mut local = base.clone();
    local.groups.retain(|g| g != "empty");
    let deleted = merge::merge(Some(&base), &local, &base, &BTreeMap::new()).unwrap();
    assert!(deleted.conflicts.is_empty());
    assert!(deleted.snapshot.group_tombstones.contains("empty"));
    let stale = merge::merge(None, &base, &deleted.snapshot, &BTreeMap::new()).unwrap();
    assert!(!stale.snapshot.groups.contains(&"empty".into()));
    assert!(stale
        .conflicts
        .iter()
        .any(|c| c.key == merge::group_key("empty")));
    let recreate = merge::merge(
        None,
        &base,
        &deleted.snapshot,
        &[(merge::group_key("empty"), Choice::Local)].into(),
    )
    .unwrap();
    assert!(recreate.snapshot.groups.contains(&"empty".into()));
    assert!(!recreate.snapshot.group_tombstones.contains("empty"));
}

#[test]
fn deleting_empty_group_while_other_side_adds_connection_requires_organization_choice() {
    let base = snapshot(&[]);
    let mut local = base.clone();
    let mut remote = base.clone();
    local.groups.retain(|g| g != "empty");
    remote.connections.push(cfg("new", "empty"));
    let preview = merge::merge(Some(&base), &local, &remote, &BTreeMap::new()).unwrap();
    assert!(preview.conflicts.iter().any(|c| c.key == "organization"));
    let selected = merge::merge(
        Some(&base),
        &local,
        &remote,
        &[("organization".into(), Choice::Local)].into(),
    )
    .unwrap();
    assert_eq!(selected.snapshot.connections.len(), 1);
    assert!(selected.snapshot.connections[0].group.is_empty());
    assert!(!selected.snapshot.groups.contains(&"empty".into()));
}

#[test]
fn capture_retains_connection_and_empty_group_deletion_records_from_baseline() {
    let temp = Temp::new();
    let store = temp.store();
    let base = snapshot(&["a"]);
    local_save(&store, &base);
    store
        .update_connections(|c, g| {
            c.clear();
            g.retain(|name| name != "empty");
            Ok(())
        })
        .unwrap();
    let (captured, _) = Snapshot::capture(&store, Some(&base)).unwrap();
    assert!(captured.tombstones.contains("a"));
    assert!(captured.group_tombstones.contains("empty"));
}

#[test]
fn baseline_group_delete_edit_has_one_organization_choice_and_honors_both_sides() {
    let base = snapshot(&[]);
    let mut local = base.clone();
    let mut remote = base.clone();
    local.groups.retain(|g| g != "empty");
    local.group_tombstones.insert("empty".into());
    remote.connections.push(cfg("new", "empty"));
    let preview = merge::merge(Some(&base), &local, &remote, &BTreeMap::new()).unwrap();
    assert_eq!(preview.conflicts.len(), 1);
    assert_eq!(preview.conflicts[0].key, "organization");
    let selected = merge::merge(
        Some(&base),
        &local,
        &remote,
        &[("organization".into(), Choice::Remote)].into(),
    )
    .unwrap();
    assert_eq!(selected.snapshot.connections[0].group, "empty");
    assert!(selected.snapshot.groups.contains(&"empty".into()));
    assert!(!selected.snapshot.group_tombstones.contains("empty"));
}

#[test]
fn separate_group_recreation_and_organization_choices_both_apply() {
    let local = Snapshot::new(
        vec![cfg("a", "stale")],
        vec!["one".into(), "two".into(), "stale".into()],
        BTreeSet::new(),
    );
    let mut remote = Snapshot::new(vec![], vec!["two".into(), "one".into()], BTreeSet::new());
    remote.group_tombstones.insert("stale".into());
    let selected = merge::merge(
        None,
        &local,
        &remote,
        &[
            (merge::group_key("stale"), Choice::Local),
            ("organization".into(), Choice::Remote),
        ]
        .into(),
    )
    .unwrap();
    assert_eq!(selected.conflicts.len(), 2);
    assert!(selected.snapshot.groups.contains(&"stale".into()));
    assert_eq!(selected.snapshot.connections[0].group, "stale");
    assert!(!selected.snapshot.group_tombstones.contains("stale"));
    assert_eq!(&selected.snapshot.groups[..2], &["two", "one"]);
}

#[test]
fn group_and_order_conflict_keys_are_opaque_and_labels_do_not_disclose_secrets() {
    let mut base = snapshot(&["a", "b", "c"]);
    base.groups[0] = PARAM_SECRET.into();
    for c in &mut base.connections {
        c.group = PARAM_SECRET.into();
    }
    let mut local = base.clone();
    let mut remote = base.clone();
    local.connections.swap(0, 1);
    remote.connections.swap(1, 2);
    let preview = merge::merge(Some(&base), &local, &remote, &BTreeMap::new()).unwrap();
    assert!(preview
        .conflicts
        .iter()
        .any(|c| c.key == merge::order_key(PARAM_SECRET)));
    assert!(!serde_json::to_string(&preview.conflicts)
        .unwrap()
        .contains(PARAM_SECRET));
    let mut remote = Snapshot::empty();
    remote.group_tombstones.insert(PARAM_SECRET.into());
    let preview = merge::merge(None, &base, &remote, &BTreeMap::new()).unwrap();
    assert!(preview
        .conflicts
        .iter()
        .any(|c| c.key == merge::group_key(PARAM_SECRET)));
    assert!(!serde_json::to_string(&preview.conflicts)
        .unwrap()
        .contains(PARAM_SECRET));
}

#[tokio::test]
async fn missing_base_directory_requires_confirmation_before_automatic_creation() {
    let mock = Mock::new(None).await;
    mock.edit(|s| {
        s.dirs.remove("/dav/");
    });
    let temp = Temp::new();
    let state = temp.state();
    local_save(&state.store, &fixture());
    let service = configured(&state.store, &mock.url);
    let view = service.preview(&state.store).await.unwrap();
    assert!(view.requires_confirmation);
    assert!(service
        .commit(&state, &view.plan_id, BTreeMap::new(), true)
        .await
        .is_err());
    assert!(mock
        .requests()
        .iter()
        .all(|r| r.method != "MKCOL" && r.method != "PUT"));
}

#[tokio::test]
async fn network_stage_timeout_after_put_does_not_retry_or_report_success() {
    let mock = Mock::new(Some(b"old".to_vec())).await;
    let mut dav = mock.dav();
    let observed = dav.read().await.unwrap();
    dav.set_total_timeout_for_test(Duration::from_secs(2));
    let arrived = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    mock.edit(|s| s.pause_put = Some((arrived.clone(), release.clone())));
    let task = tokio::spawn(async move { dav.put_verified(&observed, b"new").await });
    tokio::time::timeout(Duration::from_secs(5), arrived.notified())
        .await
        .unwrap();
    assert!(message(task.await.unwrap()).contains("超时"));
    release.notify_one();
    assert_eq!(
        mock.requests().iter().filter(|r| r.method == "PUT").count(),
        1
    );
}

#[test]
fn authorization_identity_uses_effective_header_and_ignores_ordinary_headers() {
    let fingerprint = |password: &str, headers: &str| {
        webdav::identity_fingerprint("https://synthetic.example/dav/", "u", password, headers).unwrap()
    };
    assert_ne!(fingerprint("one", ""), fingerprint("two", ""));
    assert_eq!(fingerprint("one", "X-Tag: first"), fingerprint("one", "X-Tag: second\n# comment"));
    assert_eq!(fingerprint("one", "Authorization: Bearer account-one"), fingerprint("two", "authorization: Bearer account-one"));
    assert_ne!(fingerprint("one", "Authorization: Bearer account-one"), fingerprint("one", "Authorization: Bearer account-two"));
}

#[test]
fn changing_effective_authentication_resets_baseline_pending_and_success_time() {
    let temp = Temp::new();
    let store = temp.store();
    let service = configured(&store, "https://synthetic.example/dav/");
    for (before, after) in [("", ""), ("Authorization: Bearer account-one", "Authorization: Bearer account-two")] {
        let mut initial = input("https://synthetic.example/dav/", true);
        initial.headers = Some(before.into());
        service.set_config(&store, initial).unwrap();
        set_baseline(&service, &store, fixture());
        {
            let mut runtime = service.lock().unwrap();
            runtime.stored.config.last_sync = Some("previous-success".into());
            runtime.stored.pending = Some(Recovery {
                identity: runtime.stored.config.identity().unwrap(),
                original_local: fixture(), target: fixture(), written_hash: "a".repeat(64),
            });
        }
        let mut changed = input("https://synthetic.example/dav/", true);
        changed.password = Some("different-basic-password".into());
        changed.headers = Some(after.into());
        let view = service.set_config(&store, changed).unwrap();
        assert!(view.last_sync.is_none());
        let runtime = service.lock().unwrap();
        assert!(runtime.stored.baseline.is_none() && runtime.stored.pending.is_none());
    }
}

#[test]
fn explicit_authentication_preserves_baseline_when_unused_basic_or_non_auth_headers_change() {
    let temp = Temp::new();
    let store = temp.store();
    let service = configured(&store, "https://synthetic.example/dav/");
    let mut initial = input("https://synthetic.example/dav/", true);
    initial.headers = Some("Authorization: Bearer account-one\nX-Tag: first".into());
    service.set_config(&store, initial).unwrap();
    set_baseline(&service, &store, fixture());
    let mut changed = input("https://synthetic.example/dav/", true);
    changed.password = Some("unused-basic-changed".into());
    changed.headers = Some("authorization: Bearer account-one\nX-Tag: second".into());
    service.set_config(&store, changed).unwrap();
    assert!(service.lock().unwrap().stored.baseline.is_some());
    assert!(load_state(&temp.0).unwrap().baseline.is_some());
}

#[test]
fn legacy_endpoint_only_state_is_readable_but_cannot_reuse_the_old_baseline() {
    let temp = Temp::new();
    let store = temp.store();
    let service = configured(&store, "https://synthetic.example/dav/");
    set_baseline(&service, &store, fixture());
    let mut old = service.lock().unwrap().stored.clone();
    old.identity = None;
    old.config.last_sync = Some("old-success".into());
    persist_state(&temp.0, &old).unwrap();
    let loaded = load_state(&temp.0).unwrap();
    assert!(loaded.baseline.is_none() && loaded.pending.is_none());
    assert!(loaded.config.last_sync.is_none());
    assert_eq!(loaded.config.passphrase, PASSPHRASE);
}

#[tokio::test]
async fn switching_bearer_account_consumes_preview_and_requires_first_remote_confirmation() {
    let mock = Mock::new(Some(sealed_fixture())).await;
    let temp = Temp::new();
    let state = temp.state();
    local_save(&state.store, &fixture());
    let service = configured(&state.store, &mock.url);
    let mut initial = input(&mock.url, true);
    initial.headers = Some("Authorization: Bearer account-one".into());
    service.set_config(&state.store, initial).unwrap();
    set_baseline(&service, &state.store, fixture());
    let old_view = service.preview(&state.store).await.unwrap();
    let mut changed = input(&mock.url, true);
    changed.headers = Some("Authorization: Bearer account-two".into());
    service.set_config(&state.store, changed).unwrap();
    assert!(service.commit(&state, &old_view.plan_id, BTreeMap::new(), false).await.is_err());
    let view = service.preview(&state.store).await.unwrap();
    assert!(view.requires_confirmation);
    assert!(service.commit(&state, &view.plan_id, BTreeMap::new(), true).await.is_err());
    assert!(mock.requests().iter().all(|r| r.method != "PUT"));
}

#[tokio::test]
async fn commit_rechecks_driver_dependencies_after_preview() {
    let mock = Mock::new(Some(sealed_fixture())).await;
    let temp = Temp::new();
    let state = temp.state();
    fs::write(temp.0.join("drivers.json"), br#"[{"id":"synthetic-driver","source":"url","target":"synthetic"}]"#).unwrap();
    let service = configured(&state.store, &mock.url);
    let view = service.preview(&state.store).await.unwrap();
    assert!(view.warnings.iter().all(|w| !w.contains("缺少本机自定义驱动")));
    fs::write(temp.0.join("drivers.json"), b"[]").unwrap();
    let result = service.commit(&state, &view.plan_id, BTreeMap::new(), false).await.unwrap();
    assert!(result.warnings.iter().any(|w| w.contains("缺少本机自定义驱动")));
    assert!(!result.recovery_required);
}

#[tokio::test]
async fn missing_directory_depth_is_bounded_before_any_mkcol() {
    let mock = Mock::new(None).await;
    let url = format!("{}{}", mock.url, "missing/".repeat(20));
    let dav = Dav::new(&url, "u", "p", "", Arc::new(Mutex::new(None))).unwrap();
    assert!(message(dav.read().await).contains("层级过深"));
    assert!(mock
        .requests()
        .iter()
        .all(|r| r.method != "MKCOL" && r.method != "PUT"));
}
