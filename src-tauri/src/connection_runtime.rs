//! Server-owned runtime admission, material fingerprints and pool generations.
//! No credential, file content or client-supplied fingerprint is returned in this module's IPC.
use anyhow::{bail, Result};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, path::Path, sync::{Arc, Weak}};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};
use crate::{connection_health::{Generation, KeepAlivePolicy, Registry as HealthRegistry}, models::{ConnectionConfig, ConnectionRequest, Protocol}, ssh_tunnel::{self, SshConfig, SshTunnel, WeakTunnel}, tls_options::{self, TlsOptions}};
pub use ssh_tunnel::profiles::{RuntimeProfileDraft, RuntimeProfileView};

pub struct PreparedRuntime {
    pub fingerprint: String,
    pub tls: Option<TlsOptions>,
    pub ssh: Option<SshConfig>,
    pub http: Option<ssh_tunnel::profiles::ResolvedHttpRuntime>,
    pub profile_revision: Option<String>,
}
impl std::fmt::Debug for PreparedRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("PreparedRuntime").field("fingerprint", &self.fingerprint).field("profile_revision", &self.profile_revision).field("ssh_present", &self.ssh.is_some()).field("tls_present", &self.tls.is_some()).field("http_present", &self.http.is_some()).finish() }
}
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSubmission {
    pub draft: RuntimeProfileDraft,
    pub expected_revision: Option<String>,
    /// Reusing a local profile at a different endpoint requires a distinct, explicit human choice.
    #[serde(default)] pub rebind_endpoint: bool,
}

fn hash_parts(parts: &[&[u8]]) -> String {
    let mut hash = Sha256::new();
    for part in parts { hash.update((part.len() as u64).to_be_bytes()); hash.update(part); }
    format!("{:x}", hash.finalize())
}
pub fn endpoint_fingerprint(cfg: &ConnectionConfig) -> Result<String> {
    if cfg.id.is_empty() || cfg.id.len() > 128 || cfg.id.chars().any(char::is_control) { bail!("RUNTIME_PROFILE_ID_INVALID"); }
    let protocol = cfg.kind.protocol();
    let host = cfg.host.to_ascii_lowercase();
    let port = if cfg.port == 0 { cfg.kind.meta().map(|meta| meta.port).unwrap_or(0) } else { cfg.port };
    let user = if cfg.user.is_empty() { match protocol { Protocol::Mysql => "root", Protocol::Postgres => "postgres", _ => "" } } else { cfg.user.as_str() };
    let encoded = serde_json::to_vec(&(&cfg.kind, host, port, user, &cfg.database, &cfg.file))?;
    Ok(hash_parts(&[b"navkit-runtime-endpoint-v1", &encoded]))
}
fn config_digest(cfg: &ConnectionConfig) -> Result<String> {
    let mut identity = cfg.clone(); identity.name.clear(); identity.group.clear();
    Ok(hash_parts(&[b"navkit-runtime-config-v1", &serde_json::to_vec(&identity)?]))
}
/// The driver's structured configuration is authoritative. Never feed it to a SQLx/SSH key-value parser.
pub fn transport_parameters(parameters: &str) -> Result<Option<String>> {
    if crate::driver_manager::parse_params(parameters)?.is_some() { return Ok(None); }
    let (_, extra) = crate::driver_manager::split_version_selection(parameters)?;
    if crate::driver_manager::parse_params(&extra)?.is_some() { bail!("RUNTIME_DRIVER_CONFIG_CONFLICT: 驱动版本选择不能嵌套另一驱动配置"); }
    Ok(Some(extra))
}
pub fn profile_reference(parameters: &str) -> Result<Option<String>> {
    let Some(parameters) = transport_parameters(parameters)? else { return Ok(None) };
    // Opaque REST/native/JDBC payloads are not SQLx key-value parameters. Parse only if a local reference is actually present.
    if !parameters.to_ascii_lowercase().contains("runtime_profile") { return Ok(None); }
    let mut revision = None;
    for (key, value) in tls_options::params::parse(&parameters)? {
        if key == "runtime_profile_id" {
            if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) { bail!("RUNTIME_PROFILE_REFERENCE_INVALID: 本机配置引用版本无效"); }
            if revision.replace(value).is_some() { bail!("RUNTIME_PROFILE_REFERENCE_CONFLICT: 本机配置引用重复"); }
        }
    }
    Ok(revision)
}
/// A persisted profile is never optional once its revision is referenced by the connection configuration.
pub fn prepare_at(cfg: &ConnectionConfig, root: &Path) -> Result<PreparedRuntime> {
    let config_hash = config_digest(cfg)?;
    let profile_store = ssh_tunnel::profiles::Store::new(root);
    let reference = profile_reference(&cfg.params)?;
    let view = profile_store.view(&cfg.id)?;
    match (&reference, &view) {
        (Some(revision), Some(view)) if revision == &view.revision => {},
        (Some(_), None) => bail!("RUNTIME_PROFILE_MISSING: 连接引用的本机配置缺失；禁止回退直连或默认TLS"),
        (Some(_), Some(_)) => bail!("RUNTIME_PROFILE_REFERENCE_CHANGED: 连接与本机配置版本不一致；需先恢复或重新加载"),
        (None, Some(_)) => bail!("RUNTIME_PROFILE_REFERENCE_REQUIRED: 存在未绑定的本机配置；必须完成一致保存或显式迁移"),
        (None, None) => {},
    }
    let Some(parameters) = transport_parameters(&cfg.params)? else {
        if view.is_some() { bail!("RUNTIME_DRIVER_TRANSPORT_UNSUPPORTED: 自定义驱动尚未证明SSH/TLS身份分离；不会忽略本机配置直连"); }
        return Ok(PreparedRuntime { fingerprint: config_hash, tls: None, ssh: None, http: None, profile_revision: None });
    };
    let mut tls = None; let mut ssh = None; let mut http = None;
    let mut material = Vec::new();
    if view.is_some() {
        assert_profile_parameters(&parameters)?;
        let resolved = profile_store.resolve(&cfg.id, &endpoint_fingerprint(cfg)?)?.ok_or_else(|| anyhow::anyhow!("RUNTIME_PROFILE_MISSING"))?;
        validate_resolved_protocol(cfg, resolved.http.is_some())?;
        material.extend_from_slice(resolved.effective_fingerprint.as_bytes());
        tls = resolved.tls; ssh = resolved.ssh; http = resolved.http;
    } else {
        match cfg.kind.protocol() {
            Protocol::Mysql | Protocol::Postgres => {
                tls = Some(tls_options::parse(&parameters, true)?);
                ssh = ssh_tunnel::parse(&parameters)?;
                if ssh.is_some() { tls.as_ref().unwrap().ensure_ssh_compatible()?; }
            },
            _ => {
                if parameters.to_ascii_lowercase().contains("ssh_") && ssh_tunnel::parse(&parameters)?.is_some() { bail!("SSH_PROTOCOL_UNSUPPORTED: 此协议尚未安全接入SSH；禁止忽略配置直连"); }
                // Native Kafka/MQTT perform their own verified TLS setup; content changes still invalidate their binding.
                if matches!(cfg.kind.id(), "kafka" | "mqtt") {
                    let native = crate::native_tls::parse(&parameters)?;
                    material.extend_from_slice(hash_parts(&[format!("{:?}:{:?}", native.requested_mode, native.enforced_mode).as_bytes(), native.ca.as_deref().unwrap_or_default(), native.cert.as_deref().unwrap_or_default(), native.key.as_ref().map(|key| key.as_slice()).unwrap_or_default()]).as_bytes());
                }
            }
        }
    }
    if let Some(options) = &tls { material.extend_from_slice(options.fingerprint().as_bytes()); }
    if let Some(options) = &ssh { material.extend_from_slice(options.material_fingerprint().as_bytes()); }
    if http.is_none() && matches!(cfg.kind.protocol(), Protocol::Mysql) {
        material.extend_from_slice(crate::mysql_options::parse(&parameters)?.fingerprint().as_bytes());
    }
    let _policy = keepalive_policy(cfg)?;
    Ok(PreparedRuntime { fingerprint: hash_parts(&[config_hash.as_bytes(), &material]), tls, ssh, http, profile_revision: reference })
}
pub fn binding_fingerprint(cfg: &ConnectionConfig) -> Result<String> { binding_fingerprint_at(cfg, &crate::storage::data_dir()) }
pub fn binding_fingerprint_at(cfg: &ConnectionConfig, root: &Path) -> Result<String> { Ok(prepare_at(cfg, root)?.fingerprint) }
/// A draft is separate local I/O, never converted to portable parameters or saved as part of a connection test.
pub fn prepare_transient(cfg: &ConnectionConfig, root: &Path, submission: &RuntimeSubmission) -> Result<PreparedRuntime> {
    let parameters = transport_parameters(&cfg.params)?.ok_or_else(|| anyhow::anyhow!("RUNTIME_DRIVER_TRANSPORT_UNSUPPORTED"))?;
    assert_profile_parameters(&parameters)?;
    let store = ssh_tunnel::profiles::Store::new(root);
    let resolved = store.resolve_draft_rebinding(&cfg.id, &endpoint_fingerprint(cfg)?, &submission.draft, submission.expected_revision.as_deref(), submission.rebind_endpoint)?;
    validate_resolved_protocol(cfg, resolved.http.is_some())?;
    let fingerprint = hash_parts(&[config_digest(cfg)?.as_bytes(), resolved.effective_fingerprint.as_bytes()]);
    Ok(PreparedRuntime { fingerprint, tls: resolved.tls, ssh: resolved.ssh, http: resolved.http, profile_revision: Some(resolved.revision) })
}
fn validate_resolved_protocol(cfg: &ConnectionConfig, http: bool) -> Result<()> {
    if http {
        if cfg.kind.id() != "h2" { bail!("HTTP_TUNNEL_TARGET_UNCERTIFIED: 当前参考隧道仅已验证H2，不会忽略配置直连"); }
    } else if !matches!(cfg.kind.protocol(), Protocol::Mysql | Protocol::Postgres) {
        bail!("RUNTIME_PROFILE_PROTOCOL_UNSUPPORTED: 该驱动未接入此本机配置，不会忽略配置直连");
    }
    Ok(())
}
pub fn assert_profile_parameters(parameters: &str) -> Result<()> {
    for (key, _) in tls_options::params::parse(parameters)? {
        if key.starts_with("ssh_") || tls_options::params::tls_key(&key).is_some() { bail!("RUNTIME_PROFILE_CONFLICTING_PARAMS: 本机配置不能与旧SSH/TLS参数叠加；原文已保留，需显式迁移"); }
    }
    Ok(())
}
pub fn keepalive_policy(cfg: &ConnectionConfig) -> Result<KeepAlivePolicy> {
    let parameters = transport_parameters(&cfg.params)?.unwrap_or_default();
    if matches!(cfg.kind.protocol(), Protocol::Mysql | Protocol::Postgres) || parameters.to_ascii_lowercase().contains("keepalive") || parameters.to_ascii_lowercase().contains("keep_alive") {
        KeepAlivePolicy::try_parse(&parameters, cfg.keep_alive)
    } else { KeepAlivePolicy::try_parse("", false) }
}
pub fn request_config(id: &str, request: &ConnectionRequest, read_only: bool, keep_alive: bool) -> ConnectionConfig {
    ConnectionConfig { id: id.into(), name: String::new(), kind: request.kind.clone(), host: request.host.clone(), port: request.port, user: request.user.clone(), password: String::new(), database: request.database.clone(), file: request.file.clone(), params: request.params.clone(), group: String::new(), read_only, keep_alive }
}

struct Record { fingerprint: String, db: Weak<crate::db::Db>, tunnel: Option<WeakTunnel>, generation: Option<Generation> }
#[derive(Default)]
struct Inner { records: HashMap<String, Record>, closing: HashMap<String, ()> }
#[derive(Default, Clone)]
pub struct RuntimeRegistry { inner: Arc<Mutex<Inner>>, gates: Arc<Mutex<HashMap<String, Weak<AsyncMutex<()>>>>>, pub health: HealthRegistry }
impl RuntimeRegistry {
    pub fn require_openable(&self, id: &str) -> Result<()> {
        let inner = self.inner.lock();
        if inner.closing.keys().any(|base| id == base || id.starts_with(&format!("{base}@"))) { bail!("CONNECTION_RESOURCES_PENDING: 原物理连接仍在关闭；禁止并发重新打开或探测"); }
        Ok(())
    }
    pub async fn lock(&self, id: &str) -> Result<OwnedMutexGuard<()>> {
        self.require_openable(id)?;
        let gate = {
            let mut gates = self.gates.lock(); gates.retain(|_, value| value.strong_count() != 0);
            if let Some(gate) = gates.get(id).and_then(Weak::upgrade) { gate } else { let gate = Arc::new(AsyncMutex::new(())); gates.insert(id.into(), Arc::downgrade(&gate)); gate }
        };
        let guard = gate.lock_owned().await; self.require_openable(id)?; Ok(guard)
    }
    pub fn matches(&self, id: &str, fingerprint: &str, db: &Arc<crate::db::Db>) -> bool {
        let inner = self.inner.lock();
        inner.records.get(id).map(|record| record.fingerprint == fingerprint && record.db.upgrade().map(|old| Arc::ptr_eq(&old, db)).unwrap_or(false) && record.tunnel.as_ref().map(|weak| weak.upgrade().map(|tunnel| tunnel.is_available()).unwrap_or(false)).unwrap_or(true)).unwrap_or(false)
    }
    pub fn publish(&self, id: &str, fingerprint: &str, db: &Arc<crate::db::Db>, tunnel: Option<&SshTunnel>, policy: KeepAlivePolicy) -> Result<()> {
        self.require_openable(id)?;
        let supported = matches!(db.as_ref(), crate::db::Db::Mysql(_) | crate::db::Db::Postgres(_));
        let generation = self.health.register_connection(id, fingerprint, policy, supported)?;
        self.inner.lock().records.insert(id.into(), Record { fingerprint: fingerprint.into(), db: Arc::downgrade(db), tunnel: tunnel.map(SshTunnel::downgrade), generation: Some(generation) });
        Ok(())
    }
    pub fn publish_derived(&self, id: &str, fingerprint: &str, db: &Arc<crate::db::Db>, tunnel: Option<&SshTunnel>) -> Result<()> {
        self.require_openable(id)?;
        self.inner.lock().records.insert(id.into(), Record { fingerprint: fingerprint.into(), db: Arc::downgrade(db), tunnel: tunnel.map(SshTunnel::downgrade), generation: None }); Ok(())
    }
    pub fn tunnel(&self, id: &str) -> Option<SshTunnel> { self.inner.lock().records.get(id).and_then(|record| record.tunnel.as_ref()).and_then(WeakTunnel::upgrade) }
    pub fn generation(&self, id: &str) -> Option<Generation> { self.inner.lock().records.get(id).and_then(|record| record.generation.clone()) }
    pub fn remove(&self, id: &str) {
        let prefix = format!("{id}@");
        let mut inner = self.inner.lock();
        if let Some(generation) = inner.records.get(id).and_then(|record| record.generation.as_ref()) { self.health.clear_generation(generation); }
        inner.records.retain(|key, _| key != id && !key.starts_with(&prefix));
    }
    pub fn begin_close(&self, id: &str) -> Result<ClosingToken> {
        self.begin_close_many(&[id.to_owned()])?.pop().ok_or_else(|| anyhow::anyhow!("CONNECTION_ID_INVALID"))
    }
    /// All close markers are established atomically, after callers acquire the base gates.
    pub fn begin_close_many(&self, ids: &[String]) -> Result<Vec<ClosingToken>> {
        let mut ids = ids.to_vec(); ids.sort(); ids.dedup();
        let mut inner = self.inner.lock();
        for id in &ids {
            if inner.closing.keys().any(|base| id == base || id.starts_with(&format!("{base}@"))) {
                bail!("CONNECTION_RESOURCES_PENDING: 原物理连接仍在关闭；批量关闭未改变其他连接");
            }
        }
        let mut tokens = Vec::with_capacity(ids.len());
        for id in ids {
            let prefix = format!("{id}@");
            if let Some(generation) = inner.records.get(&id).and_then(|record| record.generation.as_ref()) { self.health.clear_generation(generation); }
            inner.records.retain(|key, _| key != &id && !key.starts_with(&prefix));
            inner.closing.insert(id.clone(), ());
            tokens.push(ClosingToken { inner: self.inner.clone(), id });
        }
        Ok(tokens)
    }
}
/// Dropping an unfinished token does NOT claim closure: panic or lost cleanup remains quarantined.
pub struct ClosingToken { inner: Arc<Mutex<Inner>>, id: String }
impl ClosingToken { pub fn complete(self) { self.inner.lock().closing.remove(&self.id); } }

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> ConnectionConfig { request_config("c1", &ConnectionRequest { kind: crate::models::DbKind::of("sqlite"), host: String::new(), port: 0, user: String::new(), password: String::new(), database: String::new(), file: ":memory:".into(), params: String::new(), timeout_secs: 1 }, false, false) }
    #[test] fn display_name_and_group_do_not_change_binding() { let mut cfg = config(); let before = config_digest(&cfg).unwrap(); cfg.name = "renamed".into(); cfg.group = "other".into(); assert_eq!(before, config_digest(&cfg).unwrap()); cfg.read_only = true; assert_ne!(before, config_digest(&cfg).unwrap()); }
    #[test] fn endpoint_binds_database_and_default_port() { let mut cfg = config(); cfg.kind = crate::models::DbKind::of("postgresql"); let original = endpoint_fingerprint(&cfg).unwrap(); cfg.port = cfg.kind.meta().unwrap().port; assert_eq!(original, endpoint_fingerprint(&cfg).unwrap()); cfg.database = "other".into(); assert_ne!(original, endpoint_fingerprint(&cfg).unwrap()); }
    #[test] fn reference_is_strict_not_a_client_fingerprint() { assert!(profile_reference("runtime_profile_id=anything").is_err()); assert_eq!(profile_reference(&format!("runtime_profile_id={}", "a".repeat(64))).unwrap(), Some("a".repeat(64))); }
    #[tokio::test] async fn unfinished_close_blocks_reopen_and_derived_connections() { let registry = RuntimeRegistry::default(); let token = registry.begin_close("c1").unwrap(); assert!(registry.require_openable("c1").is_err()); assert!(registry.require_openable("c1@db").is_err()); token.complete(); assert!(registry.require_openable("c1").is_ok()); }
    #[tokio::test] async fn dropped_close_token_stays_quarantined() { let registry = RuntimeRegistry::default(); drop(registry.begin_close("c1").unwrap()); assert!(registry.lock("c1").await.is_err()); }
    #[test] fn batch_close_is_atomic_and_duplicate_ids_are_one_token() {
        let registry = RuntimeRegistry::default();
        let existing = registry.begin_close("c2").unwrap();
        assert!(registry.begin_close_many(&["c1".into(), "c2".into()]).is_err());
        assert!(registry.require_openable("c1").is_ok());
        existing.complete();
        let tokens = registry.begin_close_many(&["c2".into(), "c1".into(), "c1".into()]).unwrap();
        assert_eq!(tokens.len(), 2);
        assert!(registry.require_openable("c1@namespace").is_err());
        for token in tokens { token.complete(); }
        assert!(registry.require_openable("c1").is_ok());
        assert!(registry.require_openable("c2").is_ok());
    }
    #[test] fn custom_driver_is_not_fed_to_transport_parser() { let params = r#"@navkit-driver:{"version":1,"driver_id":"driver","connection_string":"jdbc:h2:mem:test"}"#; assert!(transport_parameters(params).unwrap().is_none()); }
}
