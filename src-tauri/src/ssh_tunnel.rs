//! Explicit known-host pins, local-only secrets, and owned SSH forwarding lifecycle.
use anyhow::{bail, Result};
use futures::FutureExt;
use russh::{client::{self, Handler}, keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate}};
use serde::Serialize;
use std::{net::{Shutdown, TcpStream as StdTcpStream}, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::Duration};
use tokio::{io::AsyncWriteExt, net::{TcpListener, TcpStream}, sync::{Mutex, Notify, oneshot}, task::JoinSet};
use zeroize::Zeroizing;
#[path = "ssh_known_hosts.rs"] pub mod known_hosts;
#[path = "runtime_profiles.rs"] pub mod profiles;

#[derive(Clone)]
pub struct SshConfig {
    pub host: String, pub port: u16, pub user: String,
    pub key_file: Option<String>, pub known_hosts: String,
    password: Option<Zeroizing<String>>, private_key: Option<Arc<PrivateKey>>,
    pins: Arc<known_hosts::KnownHosts>, material_fingerprint: String,
}
impl std::fmt::Debug for SshConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshConfig").field("port", &self.port).field("password_present", &self.password.is_some()).field("private_key_present", &self.private_key.is_some()).finish()
    }
}
impl SshConfig {
    pub fn from_profile(spec: &profiles::SshProfile, password: Option<&str>, passphrase: Option<&str>) -> Result<Self> {
        known_hosts::canonical_host(&spec.host, spec.port)?;
        if spec.user.is_empty() || spec.user.len() > 256 || spec.user.chars().any(char::is_control) { bail!("SSH_USER_INVALID: SSH用户名无效"); }
        let pins = Arc::new(known_hosts::KnownHosts::load(&spec.known_hosts)?);
        let mut credential = None; let mut private_key = None;
        match spec.authentication {
            profiles::SshAuthentication::Password => {
                if spec.key_file.is_some() || passphrase.is_some() { bail!("SSH_AUTH_CONFLICT: 密码认证不能同时配置私钥或私钥口令"); }
                let password = password.filter(|value| !value.is_empty()).ok_or_else(|| anyhow::anyhow!("SSH_PASSWORD_REQUIRED: 本机加密配置中没有SSH密码"))?;
                if password.len() > 16 * 1024 { bail!("SSH_SECRET_LIMIT: SSH密码超过16KiB限制"); }
                credential = Some(Zeroizing::new(password.to_owned()));
            }
            profiles::SshAuthentication::PrivateKey => {
                if password.is_some() { bail!("SSH_AUTH_CONFLICT: 私钥认证不能隐式回退密码认证"); }
                let bytes = crate::tls_options::material::read_file(spec.key_file.as_deref().ok_or_else(|| anyhow::anyhow!("SSH_KEY_REQUIRED: 私钥文件未指定"))?)
                    .map_err(|_| anyhow::anyhow!("SSH_KEY_INVALID: SSH私钥文件不存在、不可读或超限"))?;
                let mut key = PrivateKey::from_openssh(bytes.as_slice()).map_err(|_| anyhow::anyhow!("SSH_KEY_INVALID: SSH私钥不是有效OpenSSH格式"))?;
                if key.is_encrypted() {
                    let passphrase = passphrase.filter(|value| !value.is_empty()).ok_or_else(|| anyhow::anyhow!("SSH_KEY_PASSPHRASE_REQUIRED: 加密SSH私钥需要本机加密口令"))?;
                    key = key.decrypt(passphrase).map_err(|_| anyhow::anyhow!("SSH_KEY_DECRYPT_FAILED: SSH私钥口令无效或格式不支持"))?;
                } else if passphrase.is_some() { bail!("SSH_AUTH_CONFLICT: 未加密私钥不能静默忽略已配置的口令"); }
                private_key = Some(Arc::new(key));
            }
        }
        use sha2::{Digest, Sha256};
        let public = private_key.as_ref().map(|key| key.public_key().fingerprint(HashAlg::Sha256).to_string());
        let data = serde_json::to_vec(&(&spec.host, spec.port, &spec.user, &spec.authentication, &pins.revision, public))?;
        Ok(Self { host: spec.host.clone(), port: spec.port, user: spec.user.clone(), key_file: spec.key_file.clone(), known_hosts: spec.known_hosts.clone(), password: credential, private_key, pins, material_fingerprint: format!("{:x}", Sha256::digest(data)) })
    }
    pub fn material_fingerprint(&self) -> &str { &self.material_fingerprint }
}
/// Legacy nonsecret key-only settings remain readable. Plaintext secrets require explicit encrypted migration.
pub fn parse(parameters: &str) -> Result<Option<SshConfig>> {
    let pairs = crate::tls_options::params::parse(parameters)?;
    let mut values = std::collections::BTreeMap::new();
    for (name, value) in pairs {
        if !name.starts_with("ssh_") { continue; }
        if matches!(name.as_str(), "ssh_password" | "ssh_key_passphrase") { bail!("RUNTIME_PROFILE_MIGRATION_REQUIRED: 旧SSH明文秘密必须先迁移至本机加密配置；原值未删除"); }
        if !matches!(name.as_str(), "ssh_host" | "ssh_port" | "ssh_user" | "ssh_key_file" | "ssh_known_hosts") { bail!("SSH_OPTION_UNSUPPORTED: SSH参数{}不支持", name); }
        if values.insert(name, value.clone()).map(|old| old != value).unwrap_or(false) { bail!("SSH_OPTION_CONFLICT: SSH参数存在冲突重复键"); }
    }
    if values.is_empty() { return Ok(None); }
    crate::tls_options::parse(parameters, true)?.ensure_ssh_compatible()?;
    let required = |key: &str| values.get(key).filter(|value| !value.is_empty()).cloned().ok_or_else(|| anyhow::anyhow!("SSH_OPTION_REQUIRED: {}必须明确提供", key));
    let port = match values.get("ssh_port") { Some(value) => value.parse::<u16>().ok().filter(|value| *value > 0).ok_or_else(|| anyhow::anyhow!("SSH_PORT_INVALID: SSH端口必须为1至65535"))?, None => 22 };
    SshConfig::from_profile(&profiles::SshProfile { host: required("ssh_host")?, port, user: required("ssh_user")?, authentication: profiles::SshAuthentication::PrivateKey, key_file: Some(required("ssh_key_file")?), known_hosts: required("ssh_known_hosts")? }, None, None).map(Some)
}

struct TunnelHandler { host: String, port: u16, pins: Arc<known_hosts::KnownHosts>, control: Arc<Control> }
impl Handler for TunnelHandler {
    type Error = russh::Error;
    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> std::result::Result<bool, Self::Error> {
        if key.certificate().is_some() { self.control.fail("SSH_HOST_CERTIFICATE_UNSUPPORTED"); return Ok(false); }
        match self.pins.verify(&self.host, self.port, &key.public_key()) {
            Ok(evidence) => { *self.control.host_key.lock() = Some(evidence.key_fingerprint); Ok(true) }
            Err(error) => {
                let text = error.to_string();
                let code = if text.starts_with("SSH_HOST_KEY_REVOKED") { "SSH_HOST_KEY_REVOKED" } else if text.starts_with("SSH_HOST_KEY_CHANGED") { "SSH_HOST_KEY_CHANGED" } else { "SSH_HOST_KEY_UNKNOWN" };
                self.control.fail(code); Ok(false)
            }
        }
    }
    async fn disconnected(&mut self, reason: client::DisconnectReason<Self::Error>) -> std::result::Result<(), Self::Error> {
        if !self.control.stopped.load(Ordering::Acquire) { self.control.fail("SSH_SESSION_CLOSED"); }
        self.control.stop();
        match reason { client::DisconnectReason::ReceivedDisconnect(_) => Ok(()), client::DisconnectReason::Error(error) => Err(error) }
    }
}
struct Control {
    stopped: AtomicBool, drained: AtomicBool, stop_notify: Notify, drain_notify: Notify,
    socket: parking_lot::Mutex<Option<StdTcpStream>>, failure: parking_lot::Mutex<Option<String>>,
    host_key: parking_lot::Mutex<Option<String>>,
}
impl Control {
    fn new() -> Arc<Self> { Arc::new(Self { stopped: AtomicBool::new(false), drained: AtomicBool::new(false), stop_notify: Notify::new(), drain_notify: Notify::new(), socket: parking_lot::Mutex::new(None), failure: parking_lot::Mutex::new(None), host_key: parking_lot::Mutex::new(None) }) }
    fn fail(&self, code: &str) { let mut failure = self.failure.lock(); if failure.is_none() { *failure = Some(code.into()); } }
    fn stop(&self) { self.stopped.store(true, Ordering::Release); if let Some(socket) = self.socket.lock().as_ref() { let _ = socket.shutdown(Shutdown::Both); } self.stop_notify.notify_waiters(); }
    async fn cancelled(&self) { loop { let notified = self.stop_notify.notified(); tokio::pin!(notified); notified.as_mut().enable(); if self.stopped.load(Ordering::Acquire) { return; } notified.await; } }
    async fn wait_drained(&self) { loop { let notified = self.drain_notify.notified(); tokio::pin!(notified); notified.as_mut().enable(); if self.drained.load(Ordering::Acquire) { return; } notified.await; } }
    fn finish(&self) { self.socket.lock().take(); self.drained.store(true, Ordering::Release); self.drain_notify.notify_waiters(); }
}
struct Owner { control: Arc<Control> }
impl Drop for Owner { fn drop(&mut self) { self.control.stop(); } }
struct StopOnDrop(Arc<Control>);
impl Drop for StopOnDrop { fn drop(&mut self) { self.0.stop(); } }
#[derive(Clone)]
pub struct SshTunnel { pub local_port: u16, owner: Arc<Owner> }
/// Registry metadata must not keep a tunnel alive after the last physical pool/derived pool disappears.
pub struct WeakTunnel { local_port: u16, owner: std::sync::Weak<Owner> }
impl WeakTunnel { pub fn upgrade(&self) -> Option<SshTunnel> { self.owner.upgrade().map(|owner| SshTunnel { local_port: self.local_port, owner }) } }
#[derive(Debug, Clone, Serialize)]
pub struct TunnelCloseReceipt { pub drained: bool, pub state: String, pub error_code: Option<String> }
impl SshTunnel {
    pub fn downgrade(&self) -> WeakTunnel { WeakTunnel { local_port: self.local_port, owner: Arc::downgrade(&self.owner) } }
    pub fn is_available(&self) -> bool { !self.owner.control.stopped.load(Ordering::Acquire) && !self.is_drained() }
    pub fn request_close(&self) { self.owner.control.stop(); }
    pub fn is_drained(&self) -> bool { self.owner.control.drained.load(Ordering::Acquire) }
    /// An owning cleanup worker uses this without dropping the still-running session on timeout.
    pub async fn wait_closed(&self) { self.owner.control.wait_drained().await; }
    pub fn host_key_fingerprint(&self) -> Option<String> { self.owner.control.host_key.lock().clone() }
    /// Timeout does not abort workers or claim cleanup. The worker retains all handles until actual drain.
    pub async fn close(&self, timeout: Duration) -> TunnelCloseReceipt {
        self.request_close();
        let drained = tokio::time::timeout(timeout.min(Duration::from_secs(60)), self.owner.control.wait_drained()).await.is_ok();
        TunnelCloseReceipt { drained, state: if drained { "closed" } else { "resources_pending" }.into(), error_code: self.owner.control.failure.lock().clone() }
    }
}
/// Database/pool MUST own this returned guard for its complete lifetime, including failed opens.
pub async fn start(cfg: &SshConfig, target_host: &str, target_port: u16) -> Result<SshTunnel> {
    known_hosts::canonical_host(target_host, target_port)?;
    let control = Control::new(); let owner = Arc::new(Owner { control: control.clone() });
    let (ready, receive) = oneshot::channel(); let worker_control = control.clone(); let cfg = cfg.clone(); let target = target_host.to_owned();
    tokio::spawn(async move {
        supervise_worker(run_worker(cfg, target, target_port, worker_control.clone(), ready), worker_control).await;
    });
    match tokio::time::timeout(Duration::from_secs(30), receive).await {
        Ok(Ok(Ok(local_port))) if !control.stopped.load(Ordering::Acquire) => Ok(SshTunnel { local_port, owner }),
        Ok(Ok(Err(code))) => { control.stop(); bail!("{}: SSH连接失败；认证秘密和文件路径未写入错误", code); }
        _ => { control.stop(); bail!("SSH_CONNECT_TIMEOUT: SSH启动超时或已取消；后台仍持有资源直至真实排空"); }
    }
}
async fn supervise_worker<F: std::future::Future<Output = ()>>(future: F, control: Arc<Control>) {
    let _stop = StopOnDrop(control.clone());
    match std::panic::AssertUnwindSafe(future).catch_unwind().await {
        Ok(()) => control.finish(),
        Err(_) => { control.fail("SSH_WORKER_PANICKED"); control.stop(); }
    }
}
async fn settled_stage<F: std::future::Future>(future: F, control: &Control, duration: Duration) -> F::Output {
    tokio::pin!(future);
    tokio::select! { value = &mut future => value, _ = control.cancelled() => { control.stop(); future.await }, _ = tokio::time::sleep(duration) => { control.fail("SSH_STAGE_TIMEOUT"); control.stop(); future.await } }
}
async fn run_worker(cfg: SshConfig, target: String, target_port: u16, control: Arc<Control>, ready: oneshot::Sender<std::result::Result<u16, String>>) {
    let stream = tokio::select! { result = TcpStream::connect((cfg.host.as_str(), cfg.port)) => result, _ = control.cancelled() => { let _ = ready.send(Err("SSH_CONNECT_CANCELLED".into())); return; }, _ = tokio::time::sleep(Duration::from_secs(15)) => { let _ = ready.send(Err("SSH_CONNECT_TIMEOUT".into())); return; } };
    let standard = match stream.and_then(TcpStream::into_std) { Ok(value) => value, Err(_) => { let _ = ready.send(Err("SSH_CONNECT_FAILED".into())); return; } };
    let breaker = match standard.try_clone() { Ok(value) => value, Err(_) => { let _ = ready.send(Err("SSH_SOCKET_FAILED".into())); return; } };
    *control.socket.lock() = Some(breaker);
    let stream = match TcpStream::from_std(standard) { Ok(value) => value, Err(_) => { let _ = ready.send(Err("SSH_SOCKET_FAILED".into())); return; } };
    let handler = TunnelHandler { host: cfg.host.clone(), port: cfg.port, pins: cfg.pins.clone(), control: control.clone() };
    let config = client::Config { keepalive_interval: Some(Duration::from_secs(30)), keepalive_max: 2, inactivity_timeout: Some(Duration::from_secs(600)), ..Default::default() };
    let mut session = match settled_stage(client::connect_stream(Arc::new(config), stream, handler), &control, Duration::from_secs(15)).await {
        Ok(value) => value,
        Err(_) => { let code = control.failure.lock().clone().unwrap_or_else(|| "SSH_HANDSHAKE_FAILED".into()); let _ = ready.send(Err(code)); return; }
    };
    let authenticated = if control.stopped.load(Ordering::Acquire) { false }
    else if let Some(password) = &cfg.password { settled_stage(session.authenticate_password(cfg.user.clone(), password.as_str()), &control, Duration::from_secs(15)).await.map(|value| value.success()).unwrap_or(false) }
    else if let Some(key) = &cfg.private_key { settled_stage(session.authenticate_publickey(cfg.user.clone(), PrivateKeyWithHashAlg::new(key.clone(), Some(HashAlg::Sha256))), &control, Duration::from_secs(15)).await.map(|value| value.success()).unwrap_or(false) }
    else { false };
    if !authenticated || control.stopped.load(Ordering::Acquire) { control.fail("SSH_AUTHENTICATION_FAILED"); control.stop(); let _ = ready.send(Err(control.failure.lock().clone().unwrap_or_else(|| "SSH_AUTHENTICATION_FAILED".into()))); let _ = session.await; return; }
    let listener = match TcpListener::bind(("127.0.0.1", 0)).await { Ok(value) => value, Err(_) => { control.stop(); let _ = ready.send(Err("SSH_LISTENER_FAILED".into())); let _ = session.await; return; } };
    let local_port = match listener.local_addr() { Ok(value) => value.port(), Err(_) => { control.stop(); let _ = ready.send(Err("SSH_LISTENER_FAILED".into())); let _ = session.await; return; } };
    let session = Arc::new(Mutex::new(Some(session)));
    if ready.send(Ok(local_port)).is_err() { control.stop(); }
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            _ = control.cancelled() => break,
            Some(result) = tasks.join_next(), if !tasks.is_empty() => { if result.is_err() { control.fail("SSH_FORWARD_TASK_FAILED"); control.stop(); } },
            accepted = listener.accept() => {
                let (mut inbound, peer) = match accepted { Ok(value) => value, Err(_) => { control.fail("SSH_LISTENER_FAILED"); break; } };
                if tasks.len() >= 64 { let _ = inbound.shutdown().await; continue; }
                let session = session.clone(); let control = control.clone(); let target = target.clone();
                tasks.spawn(async move {
                    let channel = {
                        let locked = session.lock().await;
                        match locked.as_ref() { Some(handle) => settled_stage(handle.channel_open_direct_tcpip(target, u32::from(target_port), peer.ip().to_string(), u32::from(peer.port())), &control, Duration::from_secs(15)).await.ok(), None => None }
                    };
                    if let Some(channel) = channel {
                        let mut stream = channel.into_stream();
                        tokio::select! { _ = tokio::io::copy_bidirectional(&mut inbound, &mut stream) => {}, _ = control.cancelled() => {} }
                        let _ = inbound.shutdown().await; let _ = stream.shutdown().await;
                    }
                });
            }
        }
    }
    drop(listener); control.stop();
    while let Some(result) = tasks.join_next().await { if result.is_err() { control.fail("SSH_FORWARD_TASK_FAILED"); } }
    let handle = { session.lock().await.take() };
    if let Some(handle) = handle { let _ = handle.await; }
}
pub fn strip_params_checked(parameters: &str) -> Result<String> { crate::tls_options::params::filter(parameters, |key| !key.starts_with("ssh_") && key != "runtime_profile_id") }
/// Compatibility only. Invalid input remains invalid instead of being silently dropped.
pub fn strip_params(parameters: &str) -> String { strip_params_checked(parameters).unwrap_or_else(|_| parameters.to_owned()) }
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture_pins() -> Arc<known_hosts::KnownHosts> {
        use base64::Engine;
        let mut wire = Vec::new();
        wire.extend_from_slice(&11_u32.to_be_bytes()); wire.extend_from_slice(b"ssh-ed25519");
        wire.extend_from_slice(&32_u32.to_be_bytes()); wire.extend_from_slice(&[1_u8; 32]);
        Arc::new(known_hosts::KnownHosts::parse(&format!("localhost ssh-ed25519 {}", base64::engine::general_purpose::STANDARD.encode(wire))).unwrap())
    }
    #[tokio::test] async fn disconnected_server_stops_local_listener_before_drain() { let control = Control::new(); let mut handler = TunnelHandler { host: "localhost".into(), port: 22, pins: fixture_pins(), control: control.clone() }; assert!(handler.disconnected(client::DisconnectReason::Error(russh::Error::Disconnect)).await.is_err()); assert!(control.stopped.load(Ordering::Acquire)); assert!(!control.drained.load(Ordering::Acquire)); assert_eq!(control.failure.lock().as_deref(), Some("SSH_SESSION_CLOSED")); }
    #[tokio::test] async fn unexpected_worker_panic_quarantines_instead_of_claiming_drain() { let control = Control::new(); supervise_worker(async { panic!("controlled test panic") }, control.clone()).await; assert!(control.stopped.load(Ordering::Acquire)); assert!(!control.drained.load(Ordering::Acquire)); assert_eq!(control.failure.lock().as_deref(), Some("SSH_WORKER_PANICKED")); }
    #[test] fn plaintext_secret_requires_non_destructive_migration() { for parameter in ["ssh_password=secret", "ssh_key_passphrase=secret"] { let error = parse(parameter).err().unwrap().to_string(); assert!(error.contains("MIGRATION_REQUIRED")); assert!(!error.contains("=secret")); } }
    #[test] fn incomplete_ssh_does_not_become_direct_connection() { assert!(parse("ssh_port=0").is_err()); assert!(parse("ssh_user=user").is_err()); assert!(parse("ssh_host=h;ssh_user=u;sslmode=disable").is_err()); }
    #[test] fn transport_filter_preserves_json_and_unknown_nontransport_parameters() { assert_eq!(strip_params_checked("{\"ssh_host\":\"h\",\"sslmode\":\"disable\",\"application_name\":\"navkit\"}").unwrap(), "{\"application_name\":\"navkit\",\"sslmode\":\"disable\"}"); }
    #[tokio::test] async fn last_owner_stops_control_without_static_keepalive() { let control = Control::new(); let first = SshTunnel { local_port: 1234, owner: Arc::new(Owner { control: control.clone() }) }; let second = first.clone(); drop(first); assert!(!control.stopped.load(Ordering::Acquire)); drop(second); assert!(control.stopped.load(Ordering::Acquire)); }
    #[tokio::test] async fn close_timeout_does_not_claim_drain_or_abort_worker() { let control = Control::new(); let tunnel = SshTunnel { local_port: 1234, owner: Arc::new(Owner { control: control.clone() }) }; let result = tunnel.close(Duration::from_millis(1)).await; assert!(!result.drained); assert_eq!(result.state, "resources_pending"); control.finish(); assert!(tunnel.close(Duration::from_millis(1)).await.drained); }
    #[tokio::test] async fn cancellation_notification_does_not_get_lost() { let control = Control::new(); control.stop(); tokio::time::timeout(Duration::from_secs(1), control.cancelled()).await.unwrap(); }
    #[tokio::test] async fn stage_deadline_keeps_future_owned_until_it_settles() { let control = Control::new(); let marker = Arc::new(AtomicBool::new(false)); let observed = marker.clone(); let value = settled_stage(async move { tokio::time::sleep(Duration::from_millis(20)).await; observed.store(true, Ordering::Release); 7 }, &control, Duration::from_millis(1)).await; assert_eq!(value, 7); assert!(marker.load(Ordering::Acquire)); assert!(control.stopped.load(Ordering::Acquire)); }
}
