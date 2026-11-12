//! HTTP local-profile regressions. No network service or database is used by these tests.
use super::*;

struct Temp(PathBuf);
impl Temp {
    fn new(with_key: bool) -> Self {
        let root = std::env::temp_dir().join(format!("navkit-http-profile-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        if with_key { fs::write(root.join("key.bin"), [7; 32]).unwrap(); }
        Self(root)
    }
}
impl Drop for Temp { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
fn profile() -> HttpProfile {
    HttpProfile { endpoint: "https://bridge.example:8443/".into(), protocol: http::HTTP_PROTOCOL.into(), expected_server_id: "11111111-1111-4111-8111-111111111111".into(), expected_target_id: "22222222-2222-4222-8222-222222222222".into(), target_alias: "h2-reference".into(), tls: TlsProfile { mode: TlsMode::VerifyFull, ca: None, cert: None, key: None }, connect_timeout_ms: 10_000 }
}
fn draft() -> RuntimeProfileDraft {
    RuntimeProfileDraft { ssh: None, tls: None, http: Some(profile()), http_bearer: SecretPatch::Set("LOCAL-BEARER-DO-NOT-LOG".into()), ssh_password: SecretPatch::Keep, ssh_key_passphrase: SecretPatch::Keep }
}
fn fp() -> String { "a".repeat(64) }
fn store() -> (Temp, Store) { let root = Temp::new(true); let store = Store::new(&root.0); (root, store) }

#[test]
fn http_profiles_require_https_origin_without_credentials_or_normalized_paths() {
    for bad in ["http://bridge.example/", "https://u:private@bridge.example/", "https://bridge.example/v1", "https://bridge.example/a/../", "https://bridge.example//", "https://bridge.example/?token=private", "https://bridge.example/#private", "https://bridge.example/\\", "https://bridge.example\n/"] {
        let error = http::validate_endpoint(bad).unwrap_err().to_string();
        assert!(!error.contains("private"));
    }
    assert_eq!(http::validate_endpoint("https://BRIDGE.example:443").unwrap().as_str(), "https://bridge.example/");
}
#[test]
fn http_profile_ids_protocol_alias_and_timeout_are_not_guessed() {
    let mut value = profile(); value.validate().unwrap();
    value.protocol = "http-rest".into(); assert!(value.validate().is_err());
    value = profile(); value.expected_server_id = "not-uuid".into(); assert!(value.validate().is_err());
    value = profile(); value.expected_target_id = "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA".into(); assert!(value.validate().is_err());
    for alias in ["", " ambiguous ", "x\ny"] { value = profile(); value.target_alias = alias.into(); assert!(value.validate().is_err()); }
    for timeout in [0, 999, 60_001, u32::MAX] { value = profile(); value.connect_timeout_ms = timeout; assert!(value.validate().is_err()); }
    for timeout in [1000, 60_000] { value = profile(); value.connect_timeout_ms = timeout; value.validate().unwrap(); }
}
#[test]
fn http_outer_tls_never_accepts_plaintext_prefer_require_or_verify_ca() {
    for mode in [TlsMode::Disable, TlsMode::Prefer, TlsMode::Require, TlsMode::VerifyCa] { let mut value = profile(); value.tls.mode = mode; assert!(http::resolve(&value, "token").is_err()); }
    http::resolve(&profile(), "token").unwrap();
}
#[test]
fn bearer_is_bounded_header_safe_and_errors_never_echo_its_contents() {
    for token in ["".to_string(), "private\r\nInjected: yes".into(), "private\0".into(), " private ".into(), "中文".into(), "X".repeat(http::HTTP_BEARER_LIMIT + 1)] {
        let error = http::validate_bearer(&token).unwrap_err().to_string(); assert!(!error.contains("private"));
    }
    http::validate_bearer(&"X".repeat(http::HTTP_BEARER_LIMIT)).unwrap();
}
#[test]
fn old_profile_and_draft_deserialize_with_default_http_fields() {
    let old = serde_json::json!({"format":"navkit-runtime-profile","version":1,"connection_id":"c1","endpoint_fingerprint":fp(),"revision":"b".repeat(64),"ssh":null,"tls":null,"secrets":{"password":null,"key_passphrase":null}});
    let decoded: StoredProfile = serde_json::from_value(old).unwrap(); assert!(decoded.http.is_none()); assert!(decoded.secrets.http_bearer.is_none());
    let old_draft: RuntimeProfileDraft = serde_json::from_str(r#"{"ssh":null,"tls":null,"ssh_password":{"action":"keep"},"ssh_key_passphrase":{"action":"keep"}}"#).unwrap();
    assert!(old_draft.http.is_none()); assert!(matches!(&old_draft.http_bearer, SecretPatch::Keep));
}
#[test]
fn http_token_is_encrypted_and_view_returns_presence_only() {
    let (_root, store) = store(); let saved = store.save("c1", &fp(), &draft(), None).unwrap();
    assert!(saved.http_bearer_present); assert!(saved.http.is_some()); assert!(saved.tls.is_none()); assert!(saved.ssh.is_none());
    let bytes = fs::read(store.path("c1")).unwrap(); assert!(!bytes.windows(b"LOCAL-BEARER-DO-NOT-LOG".len()).any(|value| value == b"LOCAL-BEARER-DO-NOT-LOG"));
    let view_json = serde_json::to_string(&saved).unwrap(); assert!(!view_json.contains("LOCAL-BEARER")); assert!(!view_json.contains("\"http_bearer\":"));
    let resolved = store.resolve("c1", &fp()).unwrap().unwrap().http.unwrap(); assert_eq!(resolved.bearer.as_str(), "LOCAL-BEARER-DO-NOT-LOG");
    assert!(!format!("{resolved:?}").contains("LOCAL-BEARER")); assert!(!format!("{:?}", draft()).contains("LOCAL-BEARER"));
}
#[test]
fn transient_http_draft_never_writes_a_profile_or_initializes_a_key() {
    let root = Temp::new(false); let store = Store::new(&root.0);
    let runtime = store.resolve_draft("c1", &fp(), &draft(), None).unwrap(); assert!(runtime.http.is_some());
    assert!(!store.path("c1").exists()); assert!(!root.0.join("key.bin").exists());
    assert!(store.save("c1", &fp(), &draft(), None).is_err()); assert!(!root.0.join("key.bin").exists());
}
#[test]
fn http_transport_cannot_coexist_with_database_tls_or_ssh() {
    let (_root, store) = store(); let mut value = draft(); value.tls = Some(profile().tls); assert!(store.resolve_draft("c1", &fp(), &value, None).is_err());
    value.tls = None; value.ssh = Some(SshProfile { host: "bastion.example".into(), port: 22, user: "alice".into(), authentication: SshAuthentication::Password, key_file: None, known_hosts: "missing".into() });
    assert!(store.resolve_draft("c1", &fp(), &value, None).is_err());
}
#[test]
fn missing_or_cleared_http_token_does_not_fall_back_or_replace_ciphertext() {
    let (_root, store) = store(); let mut value = draft(); value.http_bearer = SecretPatch::Keep;
    let error = store.resolve_draft("c1", &fp(), &value, None).err().unwrap().to_string(); assert!(error.contains("BEARER_REQUIRED"));
    let original = store.save("c1", &fp(), &draft(), None).unwrap(); let bytes = fs::read(store.path("c1")).unwrap();
    value.http_bearer = SecretPatch::Clear; assert!(store.save("c1", &fp(), &value, Some(&original.revision)).is_err()); assert_eq!(bytes, fs::read(store.path("c1")).unwrap());
    value.http = None; let cleared = store.save("c1", &fp(), &value, Some(&original.revision)).unwrap(); assert!(!cleared.http_bearer_present);
}
#[test]
fn keep_preserves_bearer_and_cas_never_replaces_newer_http_profile() {
    let (_root, store) = store(); let original = store.save("c1", &fp(), &draft(), None).unwrap(); let mut value = draft(); value.http_bearer = SecretPatch::Keep;
    let before = fs::read(store.path("c1")).unwrap(); let transient = store.resolve_draft("c1", &fp(), &value, Some(&original.revision)).unwrap(); assert_eq!(transient.http.unwrap().bearer.as_str(), "LOCAL-BEARER-DO-NOT-LOG"); assert_eq!(before, fs::read(store.path("c1")).unwrap());
    store.save("c1", &fp(), &value, Some(&original.revision)).unwrap(); let after = fs::read(store.path("c1")).unwrap(); assert!(store.save("c1", &fp(), &value, Some(&original.revision)).is_err()); assert_eq!(after, fs::read(store.path("c1")).unwrap());
}
#[test]
fn changing_http_identity_with_keep_requires_explicit_rebind_and_original_revision() {
    let (_root, store) = store(); let original = store.save("c1", &fp(), &draft(), None).unwrap(); let mut value = draft(); value.http_bearer = SecretPatch::Keep;
    value.http.as_mut().unwrap().expected_target_id = uuid::Uuid::new_v4().to_string();
    assert!(store.resolve_draft("c1", &fp(), &value, Some(&original.revision)).is_err());
    assert!(store.resolve_draft_rebinding("c1", &fp(), &value, Some("stale"), true).is_err());
    store.resolve_draft_rebinding("c1", &fp(), &value, Some(&original.revision), true).unwrap();
    assert_eq!(store.view("c1").unwrap().unwrap().http.unwrap().expected_target_id, profile().expected_target_id);
}
#[test]
fn token_and_timeout_changes_invalidate_the_resolved_http_fingerprint() {
    let first = http::resolve(&profile(), "token-first").unwrap(); let second = http::resolve(&profile(), "token-second").unwrap(); assert_ne!(first.fingerprint, second.fingerprint);
    let mut changed = profile(); changed.connect_timeout_ms += 1000; assert_ne!(first.fingerprint, http::resolve(&changed, "token-first").unwrap().fingerprint);
    let mut changed = profile(); changed.target_alias = "other".into(); assert_ne!(first.fingerprint, http::resolve(&changed, "token-first").unwrap().fingerprint);
}
#[test]
fn malformed_http_certificate_never_overwrites_an_existing_profile() {
    let (root, store) = store(); let original = store.save("c1", &fp(), &draft(), None).unwrap(); let before = fs::read(store.path("c1")).unwrap();
    let path = root.0.join("invalid-ca.pem"); fs::write(&path, b"not-a-certificate").unwrap(); let mut value = draft(); value.http.as_mut().unwrap().tls.ca = Some(path.to_string_lossy().into_owned());
    assert!(store.save("c1", &fp(), &value, Some(&original.revision)).is_err()); assert_eq!(before, fs::read(store.path("c1")).unwrap());
}
#[test]
fn orphan_bearer_or_legacy_http_secret_never_gets_implicitly_saved_or_migrated() {
    let (_root, store) = store(); let mut value = draft(); value.http = None; assert!(store.save("c1", &fp(), &value, None).is_err()); assert!(!store.path("c1").exists());
    let parameters = "sslmode=disable;http_bearer=retained-secret";
    assert!(store.migrate_legacy("c1", &fp(), parameters, None, true).is_err()); assert!(parameters.contains("retained-secret")); assert!(!store.path("c1").exists());
}
