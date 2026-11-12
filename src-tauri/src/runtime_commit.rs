//! Checked local write-ahead commit across a connection and its encrypted runtime profile.
//! Journals and profile checkpoints are encrypted with the ORIGINAL key. Recovery never executes SQL.
use super::*;
use crate::db::runtime::{self, RuntimeSubmission};
use crate::models::ConnectionConfig;
use base64::{engine::general_purpose::STANDARD, Engine};
const JOURNAL_LIMIT: usize = 4 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    format: String, version: u32, id: String,
    before: Option<ConnectionConfig>, after: Option<ConnectionConfig>,
    profile_before: Option<String>, profile_after: Option<String>,
}
pub struct CommitResult { pub connections: Vec<ConnectionConfig>, pub reopened: bool }
fn journal_path(store: &Store, id: &str) -> PathBuf { store.directory().join(format!("{:x}.pending", Sha256::digest(id.as_bytes()))) }
fn journal_aad(id: &str) -> Vec<u8> { format!("navkit:connection-runtime-commit:v1:{id}").into_bytes() }
fn journal_aad_v2(hash: &str) -> Vec<u8> { format!("navkit:connection-runtime-commit:v2:{hash}").into_bytes() }
fn journal_hash(id: &str) -> String { format!("{:x}", Sha256::digest(id.as_bytes())) }
fn seal(key: &[u8;32], plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| anyhow::anyhow!("RUNTIME_COMMIT_CRYPTO_INVALID"))?;
    let mut nonce = [0;12]; rand::rngs::OsRng.fill_bytes(&mut nonce);
    let mut bytes = nonce.to_vec(); bytes.extend(cipher.encrypt(Nonce::from_slice(&nonce), Payload { msg: plaintext, aad }).map_err(|_| anyhow::anyhow!("RUNTIME_COMMIT_ENCRYPT_FAILED"))?); Ok(bytes)
}
fn unseal(key: &[u8;32], bytes: &[u8], aad: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    if bytes.len() < 28 { bail!("RUNTIME_COMMIT_CORRUPT: 加密恢复记录损坏；已保留，不会重建密钥或覆盖配置"); }
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| anyhow::anyhow!("RUNTIME_COMMIT_CRYPTO_INVALID"))?;
    Ok(Zeroizing::new(cipher.decrypt(Nonce::from_slice(&bytes[..12]), Payload { msg: &bytes[12..], aad }).map_err(|_| anyhow::anyhow!("RUNTIME_COMMIT_DECRYPT_FAILED: 原密钥不符或恢复记录损坏；不能覆盖"))?))
}
fn raw_file(path: &Path, limit: usize) -> Result<Option<Vec<u8>>> {
    let metadata = match fs::symlink_metadata(path) { Ok(value) => value, Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None), Err(_) => bail!("RUNTIME_COMMIT_UNREADABLE") };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > limit as u64 { bail!("RUNTIME_COMMIT_PATH_INVALID: 恢复和配置必须是有界普通文件"); }
    Ok(Some(crate::storage::read_bytes_limited(path, limit)?.to_vec()))
}
fn decode_bytes(value: &Option<String>) -> Result<Option<Vec<u8>>> { value.as_ref().map(|value| STANDARD.decode(value).map_err(|_| anyhow::anyhow!("RUNTIME_COMMIT_CORRUPT"))).transpose() }
fn write_profile(store: &Store, id: &str, bytes: &Option<Vec<u8>>) -> Result<()> {
    let path = store.path(id);
    // Refuse a changed symlink/reparse-like file rather than overwriting an unrelated target.
    let _ = raw_file(&path, LIMIT)?;
    match bytes { Some(bytes) => crate::storage::atomic_write(&path, bytes)?, None => match fs::remove_file(&path) { Ok(()) => {}, Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}, Err(error) => return Err(error.into()) } }
    if raw_file(&path, LIMIT)? != *bytes { bail!("RUNTIME_COMMIT_EFFECTS_UNKNOWN: 本机配置写后读回不一致"); } Ok(())
}
pub fn has_pending(root: &Path, id: &str) -> Result<bool> { valid_id(id)?; Ok(raw_file(&journal_path(&Store::new(root), id), JOURNAL_LIMIT)?.is_some()) }
fn read_journal(store: &Store, id: &str, key: &[u8;32]) -> Result<Option<Journal>> {
    let Some(bytes) = raw_file(&journal_path(store, id), JOURNAL_LIMIT)? else { return Ok(None) };
    let plaintext = unseal(key, &bytes, &journal_aad_v2(&journal_hash(id))).or_else(|_| unseal(key, &bytes, &journal_aad(id)))?;
    let journal: Journal = serde_json::from_slice(&plaintext).map_err(|_| anyhow::anyhow!("RUNTIME_COMMIT_CORRUPT"))?;
    if journal.id != id || !matches!(journal.version, 1 | 2) || journal.format != "navkit-connection-runtime-commit" || journal.before.as_ref().map(|cfg| cfg.id != id).unwrap_or(false) || journal.after.as_ref().map(|cfg| cfg.id != id).unwrap_or(false) { bail!("RUNTIME_COMMIT_IDENTITY_INVALID"); }
    for checkpoint in [&journal.profile_before, &journal.profile_after] {
        if let Some(bytes) = decode_bytes(checkpoint)? {
            if bytes.len() > LIMIT { bail!("RUNTIME_COMMIT_LIMIT"); }
            let plaintext = unseal(key, &bytes, &aad(id))?;
            let profile: StoredProfile = serde_json::from_slice(&plaintext).map_err(|_| anyhow::anyhow!("RUNTIME_COMMIT_CORRUPT"))?;
            if profile.connection_id != id || profile.format != "navkit-runtime-profile" || profile.version != 1 { bail!("RUNTIME_COMMIT_IDENTITY_INVALID"); }
        }
    }
    Ok(Some(journal))
}
fn update_config(storage: &crate::storage::Store, reservation: &crate::storage::ConnectionReservation, connections: &mut Vec<ConnectionConfig>, groups: &mut Vec<String>, id: &str, after: &Option<ConnectionConfig>) -> Result<()> {
    match after {
        Some(cfg) => { if let Some(slot) = connections.iter_mut().find(|value| value.id == id) { *slot = cfg.clone(); } else { connections.push(cfg.clone()); } if !cfg.group.is_empty() && !groups.contains(&cfg.group) { groups.push(cfg.group.clone()); groups.sort(); } },
        None => connections.retain(|cfg| cfg.id != id),
    }
    storage.save_reserved(reservation, connections.clone(), groups.clone()).map_err(anyhow::Error::msg)
}
/// Redoes an already authorized LOCAL save/delete after a crash, only when BOTH old/new checkpoints match.
/// Unknown external configuration or ciphertext is retained; no guess, blank profile or destructive rollback.
pub fn recover(storage: &crate::storage::Store, root: &Path, id: &str) -> Result<bool> {
    if !has_pending(root, id)? { return Ok(false); }
    let store = Store::new(root); let _lock = store.lock()?; let key = store.existing_key()?;
    let Some(journal) = read_journal(&store, id, &key)? else { return Ok(false) };
    let snapshot = storage.snapshot_checked().map_err(anyhow::Error::msg)?;
    let reservation = storage.reserve_connections(snapshot.revision).map_err(anyhow::Error::msg)?;
    let current = snapshot.connections.iter().find(|cfg| cfg.id == id).cloned();
    if current != journal.before && current != journal.after { bail!("RUNTIME_COMMIT_CONFLICT: 恢复期间连接配置被其他实例修改；旧记录和草稿均已保留"); }
    let before = decode_bytes(&journal.profile_before)?; let after = decode_bytes(&journal.profile_after)?;
    let actual = raw_file(&store.path(id), LIMIT)?;
    if actual != before && actual != after { bail!("RUNTIME_COMMIT_CONFLICT: 恢复期间本机密文被其他实例修改；不会覆盖"); }
    if *store.existing_key()? != *key { bail!("RUNTIME_PROFILE_KEY_CHANGED"); }
    write_profile(&store, id, &after)?;
    let mut connections = snapshot.connections; let mut groups = snapshot.groups;
    update_config(storage, &reservation, &mut connections, &mut groups, id, &journal.after)?;
    let check = storage.snapshot_checked().map_err(anyhow::Error::msg)?;
    if check.connections.iter().find(|cfg| cfg.id == id).cloned() != journal.after || raw_file(&store.path(id), LIMIT)? != after || *store.existing_key()? != *key { bail!("RUNTIME_COMMIT_EFFECTS_UNKNOWN: 恢复写后核验失败；仍保留恢复记录"); }
    fs::remove_file(journal_path(&store, id))?; Ok(true)
}
#[derive(Default)]
pub struct RecoveryReport { pub recovered: usize, pub blocked: usize }
/// Discover interrupted local commits, including a new connection absent from connections.json.
/// Version 2 AAD binds the encrypted identity to its hashed filename; legacy journals use known IDs.
pub fn recover_all(storage: &crate::storage::Store, root: &Path) -> Result<RecoveryReport> {
    let store = Store::new(root);
    let metadata = match fs::symlink_metadata(store.directory()) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(RecoveryReport::default()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() { bail!("RUNTIME_COMMIT_PATH_INVALID"); }
    #[cfg(windows)] { use std::os::windows::fs::MetadataExt; if metadata.file_attributes() & 0x400 != 0 { bail!("RUNTIME_COMMIT_PATH_INVALID"); } }
    let snapshot = storage.snapshot_checked().map_err(anyhow::Error::msg)?;
    let known: std::collections::BTreeMap<String, String> = snapshot.connections.into_iter().map(|cfg| (journal_hash(&cfg.id), cfg.id)).collect();
    let mut pending = Vec::new();
    for (index, entry) in fs::read_dir(store.directory())?.enumerate() {
        if index >= 32768 { bail!("RUNTIME_COMMIT_LIMIT"); }
        let entry = entry?;
        if entry.path().extension().and_then(|suffix| suffix.to_str()) == Some("pending") { pending.push(entry.path()); }
        if pending.len() > 1024 { bail!("RUNTIME_COMMIT_LIMIT"); }
    }
    pending.sort();
    let mut report = RecoveryReport::default();
    for path in pending {
        let discovered = (|| -> Result<String> {
            let hash = path.file_stem().and_then(|stem| stem.to_str()).ok_or_else(|| anyhow::anyhow!("RUNTIME_COMMIT_PATH_INVALID"))?;
            valid_fingerprint(hash)?;
            let key = store.existing_key()?;
            let bytes = raw_file(&path, JOURNAL_LIMIT)?.ok_or_else(|| anyhow::anyhow!("RUNTIME_COMMIT_MISSING"))?;
            match unseal(&key, &bytes, &journal_aad_v2(hash)) {
                Ok(plaintext) => {
                    let journal: Journal = serde_json::from_slice(&plaintext).map_err(|_| anyhow::anyhow!("RUNTIME_COMMIT_CORRUPT"))?;
                    valid_id(&journal.id)?;
                    if journal.version != 2 || journal_hash(&journal.id) != hash { bail!("RUNTIME_COMMIT_IDENTITY_INVALID"); }
                    Ok(journal.id)
                },
                Err(error) => known.get(hash).cloned().ok_or(error),
            }
        })();
        match discovered.and_then(|id| recover(storage, root, &id)) {
            Ok(true) => report.recovered += 1,
            Ok(false) => {},
            Err(_) => report.blocked += 1,
        }
    }
    Ok(report)
}

fn set_reference(parameters: &str, revision: Option<&str>) -> Result<String> {
    if revision.is_none() && runtime::profile_reference(parameters)?.is_none() { return Ok(parameters.into()); }
    let (selection, extra) = crate::driver_manager::split_version_selection(parameters)?;
    if crate::driver_manager::parse_params(&extra)?.is_some() { if revision.is_some() { bail!("RUNTIME_DRIVER_TRANSPORT_UNSUPPORTED"); } return Ok(parameters.into()); }
    let filtered = tls_options::params::filter(&extra, |key| key != "runtime_profile_id")?;
    let updated = if let Some(revision) = revision {
        valid_fingerprint(revision)?;
        if filtered.trim().starts_with('{') {
            let mut map: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> = serde_json::from_str(&filtered)?;
            map.insert("runtime_profile_id".into(), serde_json::value::RawValue::from_string(serde_json::to_string(revision)?)?); serde_json::to_string(&map)?
        } else { format!("{}{}runtime_profile_id={revision}", filtered, if filtered.trim().is_empty() { "" } else { ";" }) }
    } else { filtered };
    if let Some(mut selection) = selection { selection.extra = updated; Ok(format!("{}{}", crate::driver_manager::VERSION_PARAMS_PREFIX, serde_json::to_string(&selection)?)) } else { Ok(updated) }
}
fn validate_options(cfg: &ConnectionConfig, profile: Option<&ResolvedRuntime>) -> Result<()> {
    let Some(parameters) = runtime::transport_parameters(&cfg.params)? else { if profile.is_some() { bail!("RUNTIME_DRIVER_TRANSPORT_UNSUPPORTED"); } return Ok(()) };
    runtime::keepalive_policy(cfg)?;
    if let Some(profile) = profile {
        runtime::assert_profile_parameters(&parameters)?;
        if profile.http.is_some() {
            if cfg.kind.id() != "h2" || profile.ssh.is_some() || profile.tls.is_some() { bail!("RUNTIME_PROFILE_PROTOCOL_UNSUPPORTED"); }
        } else if !matches!(cfg.kind.protocol(), crate::models::Protocol::Mysql | crate::models::Protocol::Postgres) { bail!("RUNTIME_PROFILE_PROTOCOL_UNSUPPORTED"); }
    } else {
        match cfg.kind.protocol() {
            crate::models::Protocol::Mysql | crate::models::Protocol::Postgres => { let tls = tls_options::parse(&parameters, true)?; if super::super::parse(&parameters)?.is_some() { tls.ensure_ssh_compatible()?; } },
            _ => { if parameters.to_ascii_lowercase().contains("ssh_") && super::super::parse(&parameters)?.is_some() { bail!("SSH_PROTOCOL_UNSUPPORTED"); } if matches!(cfg.kind.id(), "kafka" | "mqtt") { crate::native_tls::parse(&parameters)?; } },
        }
    }
    if cfg.kind.protocol() == crate::models::Protocol::Mysql { crate::mysql_options::parse(&parameters)?; }
    Ok(())
}
#[derive(Clone, Copy, PartialEq)]
enum StopAfter { Never, Journal, Profile, Config }
fn apply(storage: &crate::storage::Store, root: &Path, journal: Journal, before_bytes: Option<Vec<u8>>, after_bytes: Option<Vec<u8>>, reservation: &crate::storage::ConnectionReservation, mut connections: Vec<ConnectionConfig>, mut groups: Vec<String>, key: &[u8;32], stop: StopAfter) -> Result<Vec<ConnectionConfig>> {
    let store = Store::new(root); let id = &journal.id;
    let plaintext = Zeroizing::new(serde_json::to_vec(&journal)?);
    if plaintext.len() > JOURNAL_LIMIT - 28 { bail!("RUNTIME_COMMIT_LIMIT"); }
    if *store.existing_key()? != *key { bail!("RUNTIME_PROFILE_KEY_CHANGED"); }
    let bytes = seal(key, &plaintext, &journal_aad_v2(&journal_hash(id)))?;
    crate::storage::atomic_write(&journal_path(&store, id), &bytes)?;
    if raw_file(&journal_path(&store, id), JOURNAL_LIMIT)?.as_ref() != Some(&bytes) { bail!("RUNTIME_COMMIT_EFFECTS_UNKNOWN"); }
    if stop == StopAfter::Journal { bail!("RUNTIME_COMMIT_TEST_CRASH_AFTER_JOURNAL"); }
    let written = write_profile(&store, id, &after_bytes).and_then(|_| {
        if stop == StopAfter::Profile { bail!("RUNTIME_COMMIT_TEST_CRASH_AFTER_PROFILE"); }
        update_config(storage, reservation, &mut connections, &mut groups, id, &journal.after)
    });
    if let Err(error) = written {
        if stop == StopAfter::Profile { return Err(error); }
        // A known config failure restores only our own exact ciphertext, never an external change.
        let current = storage.snapshot_checked().ok().and_then(|snapshot| snapshot.connections.into_iter().find(|cfg| cfg.id == *id));
        let actual = raw_file(&store.path(id), LIMIT)?;
        if current == journal.before && (actual == before_bytes || actual == after_bytes) && *store.existing_key()? == *key {
            write_profile(&store, id, &before_bytes)?; fs::remove_file(journal_path(&store, id))?;
            return Err(anyhow::anyhow!("RUNTIME_COMMIT_FAILED_NO_CHANGE: 保存失败，原连接和本机配置已核验保留：{error}"));
        }
        bail!("RUNTIME_COMMIT_EFFECTS_UNKNOWN: 保存回执不明；已保留加密恢复记录，必须核对或重启恢复，不自动重放数据库操作");
    }
    if stop == StopAfter::Config { bail!("RUNTIME_COMMIT_TEST_CRASH_AFTER_CONFIG"); }
    let check = storage.snapshot_checked().map_err(anyhow::Error::msg)?;
    if check.connections.iter().find(|cfg| cfg.id == *id).cloned() != journal.after || raw_file(&store.path(id), LIMIT)? != after_bytes || *store.existing_key()? != *key { bail!("RUNTIME_COMMIT_EFFECTS_UNKNOWN: 保存写后核验失败；仍保留加密恢复记录"); }
    fs::remove_file(journal_path(&store, id))?; Ok(connections)
}
pub fn save_connection(storage: &crate::storage::Store, root: &Path, cfg: ConnectionConfig, password: Option<&str>, submission: Option<&RuntimeSubmission>) -> Result<CommitResult> {
    save_with_stop(storage, root, cfg, password, submission, StopAfter::Never)
}
fn save_with_stop(storage: &crate::storage::Store, root: &Path, mut cfg: ConnectionConfig, password: Option<&str>, submission: Option<&RuntimeSubmission>, stop: StopAfter) -> Result<CommitResult> {
    valid_id(&cfg.id)?; recover(storage, root, &cfg.id)?;
    let store = Store::new(root); let _lock = store.lock()?;
    let snapshot = storage.snapshot_checked().map_err(anyhow::Error::msg)?;
    let reservation = storage.reserve_connections(snapshot.revision).map_err(anyhow::Error::msg)?;
    let before = snapshot.connections.iter().find(|old| old.id == cfg.id).cloned();
    let previous_bytes = raw_file(&store.path(&cfg.id), LIMIT)?;
    // Existing encrypted material can NEVER be treated as a fresh-install key initialization.
    if previous_bytes.is_some() || snapshot.connections.iter().any(|old| !old.password.is_empty()) || root.join("key.bin").exists() { let _key = store.existing_key()?; }
    cfg.password = if let Some(password) = password { storage.encrypt_password(password).map_err(anyhow::Error::msg)? } else { before.as_ref().map(|old| old.password.clone()).unwrap_or_default() };
    let (after_bytes, after_revision) = if let Some(submission) = submission {
        let key = store.existing_key()?;
        let previous = store.read(&cfg.id, &key)?;
        let profile = store.prepare_draft_rebinding(&cfg.id, &runtime::endpoint_fingerprint(&cfg)?, &submission.draft, previous.as_ref(), submission.expected_revision.as_deref(), submission.rebind_endpoint)?;
        let resolved = store.resolve_stored(&profile)?; validate_options(&cfg, Some(&resolved))?;
        let bytes = seal(&key, &Zeroizing::new(serde_json::to_vec(&profile)?), &aad(&cfg.id))?;
        (Some(bytes), Some(profile.revision))
    } else {
        let revision = if previous_bytes.is_some() { let key = store.existing_key()?; let profile = store.read(&cfg.id, &key)?.ok_or_else(|| anyhow::anyhow!("RUNTIME_PROFILE_MISSING"))?; if profile.endpoint_fingerprint != runtime::endpoint_fingerprint(&cfg)? { bail!("RUNTIME_PROFILE_TARGET_CHANGED: 修改端点需要显式重新绑定本机配置"); } let resolved = store.resolve_stored(&profile)?; validate_options(&cfg, Some(&resolved))?; Some(profile.revision) } else { validate_options(&cfg, None)?; if runtime::profile_reference(&cfg.params)?.is_some() { bail!("RUNTIME_PROFILE_MISSING"); } None };
        (previous_bytes.clone(), revision)
    };
    cfg.params = set_reference(&cfg.params, after_revision.as_deref())?;
    let reopened = before.as_ref().map(|old| old.requires_reopen(&cfg)).unwrap_or(false) || submission.is_some();
    let mut connections = snapshot.connections; let mut groups = snapshot.groups;
    if submission.is_none() { update_config(storage, &reservation, &mut connections, &mut groups, &cfg.id, &Some(cfg.clone()))?; return Ok(CommitResult { connections, reopened }); }
    let key = store.existing_key()?;
    let journal = Journal { format: "navkit-connection-runtime-commit".into(), version: 2, id: cfg.id.clone(), before, after: Some(cfg), profile_before: previous_bytes.as_ref().map(|bytes| STANDARD.encode(bytes)), profile_after: after_bytes.as_ref().map(|bytes| STANDARD.encode(bytes)) };
    let connections = apply(storage, root, journal, previous_bytes, after_bytes, &reservation, connections, groups, &key, stop)?;
    Ok(CommitResult { connections, reopened })
}
pub fn delete_connection(storage: &crate::storage::Store, root: &Path, id: &str) -> Result<Vec<ConnectionConfig>> {
    valid_id(id)?; recover(storage, root, id)?;
    let store = Store::new(root); let _lock = store.lock()?;
    let snapshot = storage.snapshot_checked().map_err(anyhow::Error::msg)?;
    let reservation = storage.reserve_connections(snapshot.revision).map_err(anyhow::Error::msg)?;
    let before = snapshot.connections.iter().find(|cfg| cfg.id == id).cloned(); let bytes = raw_file(&store.path(id), LIMIT)?;
    let mut connections = snapshot.connections; let mut groups = snapshot.groups;
    if bytes.is_none() { update_config(storage, &reservation, &mut connections, &mut groups, id, &None)?; return Ok(connections); }
    let key = store.existing_key()?; store.read(id, &key)?;
    let journal = Journal { format: "navkit-connection-runtime-commit".into(), version: 2, id: id.into(), before, after: None, profile_before: bytes.as_ref().map(|bytes| STANDARD.encode(bytes)), profile_after: None };
    apply(storage, root, journal, bytes, None, &reservation, connections, groups, &key, StopAfter::Never)
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Temp(PathBuf); impl Temp { fn new() -> Self { let root = std::env::temp_dir().join(format!("navkit-runtime-commit-{}",uuid::Uuid::new_v4())); fs::create_dir_all(&root).unwrap(); fs::write(root.join("key.bin"),[8;32]).unwrap(); Self(root) } } impl Drop for Temp { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
    fn config() -> ConnectionConfig { runtime::request_config("c1", &crate::models::ConnectionRequest { kind: crate::models::DbKind::of("postgresql"), host:"localhost".into(),port:5432,user:"user".into(),password:String::new(),database:"db".into(),file:String::new(),params:"application_name=navkit".into(),timeout_secs:1 },false,false) }
    fn submission() -> RuntimeSubmission { RuntimeSubmission { draft: RuntimeProfileDraft { ssh:None,tls:Some(TlsProfile{mode:TlsMode::Disable,ca:None,cert:None,key:None}),http:None,http_bearer:SecretPatch::Keep,ssh_password:SecretPatch::Keep,ssh_key_passphrase:SecretPatch::Keep },expected_revision:None,rebind_endpoint:false } }
    #[test] fn profile_and_connection_revision_are_saved_together() { let root=Temp::new();let storage=crate::storage::Store::load_at(&root.0);let result=save_connection(&storage,&root.0,config(),None,Some(&submission())).unwrap();let profile=Store::new(&root.0).view("c1").unwrap().unwrap();assert_eq!(runtime::profile_reference(&result.connections[0].params).unwrap().as_deref(),Some(profile.revision.as_str()));assert!(!has_pending(&root.0,"c1").unwrap()); }
    #[test] fn journal_recovery_finishes_each_local_crash_boundary() { for boundary in [StopAfter::Journal,StopAfter::Profile,StopAfter::Config] {let root=Temp::new();let storage=crate::storage::Store::load_at(&root.0);assert!(save_with_stop(&storage,&root.0,config(),None,Some(&submission()),boundary).is_err());let reopened=crate::storage::Store::load_at(&root.0);assert!(recover(&reopened,&root.0,"c1").unwrap());let cfg=reopened.snapshot_checked().unwrap().connections.remove(0);let profile=Store::new(&root.0).view("c1").unwrap().unwrap();assert_eq!(runtime::profile_reference(&cfg.params).unwrap(),Some(profile.revision));assert!(!has_pending(&root.0,"c1").unwrap());} }
    #[test] fn external_ciphertext_blocks_recovery_instead_of_being_overwritten() {let root=Temp::new();let storage=crate::storage::Store::load_at(&root.0);assert!(save_with_stop(&storage,&root.0,config(),None,Some(&submission()),StopAfter::Profile).is_err());let store=Store::new(&root.0);fs::write(store.path("c1"),b"external").unwrap();assert!(recover(&storage,&root.0,"c1").unwrap_err().to_string().contains("CONFLICT"));assert_eq!(fs::read(store.path("c1")).unwrap(),b"external");assert!(has_pending(&root.0,"c1").unwrap());}
    #[test] fn recovery_does_not_rollback_unrelated_connections_or_groups() {let root=Temp::new();let storage=crate::storage::Store::load_at(&root.0);assert!(save_with_stop(&storage,&root.0,config(),None,Some(&submission()),StopAfter::Profile).is_err());let mut other=config();other.id="c2".into();storage.update_connections(|connections,groups|{connections.push(other);groups.push("external-group".into());Ok(())}).unwrap();assert!(recover(&storage,&root.0,"c1").unwrap());let snapshot=storage.snapshot_checked().unwrap();assert_eq!(snapshot.connections.len(),2);assert!(snapshot.groups.contains(&"external-group".into()));}
    #[test] fn wrong_key_or_corrupt_journal_is_retained_not_recreated() {let root=Temp::new();let storage=crate::storage::Store::load_at(&root.0);assert!(save_with_stop(&storage,&root.0,config(),None,Some(&submission()),StopAfter::Journal).is_err());let path=journal_path(&Store::new(&root.0),"c1");let bytes=fs::read(&path).unwrap();fs::write(root.0.join("key.bin"),[4;32]).unwrap();assert!(recover(&storage,&root.0,"c1").is_err());assert_eq!(bytes,fs::read(path).unwrap());assert_eq!(fs::read(root.0.join("key.bin")).unwrap(),[4;32]);}
    #[test] fn parameters_keep_large_numbers_and_driver_selection() {let parameters=r#"{"value":1844674407370955161512345,"label":"a;b"}"#;let updated=set_reference(parameters,Some(&"a".repeat(64))).unwrap();assert!(updated.contains("1844674407370955161512345"));assert!(updated.contains("a;b"));}
    #[test] fn startup_discovers_new_connection_commit_before_config_was_written() {
        for boundary in [StopAfter::Journal, StopAfter::Profile, StopAfter::Config] {
            let root=Temp::new(); let storage=crate::storage::Store::load_at(&root.0);
            assert!(save_with_stop(&storage,&root.0,config(),None,Some(&submission()),boundary).is_err());
            let reopened=crate::storage::Store::load_at(&root.0);
            let report=recover_all(&reopened,&root.0).unwrap(); assert_eq!(report.recovered,1); assert_eq!(report.blocked,0);
            let cfg=reopened.snapshot_checked().unwrap().connections.remove(0); let profile=Store::new(&root.0).view("c1").unwrap().unwrap();
            assert_eq!(runtime::profile_reference(&cfg.params).unwrap(),Some(profile.revision)); assert!(!has_pending(&root.0,"c1").unwrap());
            assert_eq!(recover_all(&reopened,&root.0).unwrap().recovered,0);
        }
    }
    #[test] fn startup_keeps_corrupt_or_relocated_journals_and_original_key() {
        let root=Temp::new(); let storage=crate::storage::Store::load_at(&root.0);
        assert!(save_with_stop(&storage,&root.0,config(),None,Some(&submission()),StopAfter::Journal).is_err());
        let store=Store::new(&root.0); let original=journal_path(&store,"c1"); let bytes=fs::read(&original).unwrap();
        let moved=journal_path(&store,"different-id"); fs::rename(&original,&moved).unwrap();
        let report=recover_all(&storage,&root.0).unwrap(); assert_eq!(report.recovered,0); assert_eq!(report.blocked,1);
        assert_eq!(fs::read(&moved).unwrap(),bytes); assert!(storage.connections().is_empty()); assert_eq!(fs::read(root.0.join("key.bin")).unwrap(),[8;32]);
    }
    #[test] fn startup_recovers_legacy_journal_only_for_known_connection() {
        let root=Temp::new(); let storage=crate::storage::Store::load_at(&root.0);
        let result=save_connection(&storage,&root.0,config(),None,Some(&submission())).unwrap();
        let store=Store::new(&root.0); let key=store.existing_key().unwrap(); let profile=fs::read(store.path("c1")).unwrap();
        let cfg=result.connections[0].clone(); let journal=Journal {format:"navkit-connection-runtime-commit".into(),version:1,id:"c1".into(),before:Some(cfg.clone()),after:Some(cfg),profile_before:Some(STANDARD.encode(&profile)),profile_after:Some(STANDARD.encode(&profile))};
        fs::write(journal_path(&store,"c1"),seal(&key,&serde_json::to_vec(&journal).unwrap(),&journal_aad("c1")).unwrap()).unwrap();
        let report=recover_all(&storage,&root.0).unwrap(); assert_eq!(report.recovered,1); assert_eq!(report.blocked,0);
    }
    #[test] fn delete_removes_profile_and_reference_in_one_local_commit() {let root=Temp::new();let storage=crate::storage::Store::load_at(&root.0);save_connection(&storage,&root.0,config(),None,Some(&submission())).unwrap();assert!(delete_connection(&storage,&root.0,"c1").unwrap().is_empty());assert!(Store::new(&root.0).view("c1").unwrap().is_none());assert!(!has_pending(&root.0,"c1").unwrap());}
}
