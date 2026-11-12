//! Local-only runtime profiles. Encryption reads the ORIGINAL key once; it can never create a replacement key.
use anyhow::{bail, Result};
use aes_gcm::{aead::{Aead, KeyInit, Payload}, Aes256Gcm, Nonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, io::{Read, Write}, path::{Path, PathBuf}};
use zeroize::{Zeroize, Zeroizing};
use crate::tls_options::{self, TlsMode, TlsOptions};
const LIMIT: usize = 1024 * 1024;
const SECRET_LIMIT: usize = 16 * 1024;
#[path = "runtime_commit.rs"] pub mod commit;
#[path = "http_runtime_profile.rs"] pub mod http;
pub use http::{HttpProfile, ResolvedHttpRuntime};
#[cfg(test)] #[path = "http_runtime_profile_tests.rs"] mod http_profile_tests;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all="snake_case")]
pub enum SshAuthentication { Password, PrivateKey }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshProfile {
    pub host: String, pub port: u16, pub user: String, pub authentication: SshAuthentication,
    pub key_file: Option<String>, pub known_hosts: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TlsProfile { pub mode: TlsMode, pub ca: Option<String>, pub cert: Option<String>, pub key: Option<String> }
impl TlsProfile {
    pub fn parameters(&self) -> Result<String> {
        let mode = match self.mode { TlsMode::Disable => "disable", TlsMode::Prefer => "prefer", TlsMode::Require => "require", TlsMode::VerifyCa => "verify-ca", TlsMode::VerifyFull => "verify-full" };
        let mut map = serde_json::Map::new(); map.insert("sslmode".into(), mode.into());
        for (key, value) in [("ssl_ca", &self.ca), ("ssl_cert", &self.cert), ("ssl_key", &self.key)] { if let Some(value) = value { map.insert(key.into(), value.clone().into()); } }
        Ok(serde_json::to_string(&map)?)
    }
}
#[derive(Clone, Deserialize)]
#[serde(tag="action", content="value", rename_all="snake_case")]
pub enum SecretPatch { Keep, Clear, Set(String) }
impl Default for SecretPatch { fn default() -> Self { Self::Keep } }
impl std::fmt::Debug for SecretPatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(match self { Self::Keep => "Keep", Self::Clear => "Clear", Self::Set(_) => "Set([REDACTED])" }) }
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeProfileDraft {
    pub ssh: Option<SshProfile>, pub tls: Option<TlsProfile>,
    #[serde(default)] pub http: Option<HttpProfile>,
    #[serde(default)] pub http_bearer: SecretPatch,
    #[serde(default)] pub ssh_password: SecretPatch,
    #[serde(default)] pub ssh_key_passphrase: SecretPatch,
}
impl Drop for RuntimeProfileDraft {
    fn drop(&mut self) { for patch in [&mut self.ssh_password, &mut self.ssh_key_passphrase, &mut self.http_bearer] { if let SecretPatch::Set(value) = patch { value.zeroize(); } } }
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Secrets { password: Option<String>, key_passphrase: Option<String>, #[serde(default)] http_bearer: Option<String> }
impl Drop for Secrets { fn drop(&mut self) { if let Some(value) = &mut self.password { value.zeroize(); } if let Some(value) = &mut self.key_passphrase { value.zeroize(); } if let Some(value) = &mut self.http_bearer { value.zeroize(); } } }
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredProfile {
    format: String, version: u32, connection_id: String, endpoint_fingerprint: String, revision: String,
    ssh: Option<SshProfile>, tls: Option<TlsProfile>, #[serde(default)] http: Option<HttpProfile>, secrets: Secrets,
}
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeProfileView {
    pub connection_id: String, pub endpoint_fingerprint: String, pub revision: String,
    pub ssh: Option<SshProfile>, pub tls: Option<TlsProfile>, pub ssh_password_present: bool, pub ssh_key_passphrase_present: bool,
    pub http: Option<HttpProfile>, pub http_bearer_present: bool,
}
impl StoredProfile {
    fn view(&self) -> RuntimeProfileView { RuntimeProfileView { connection_id: self.connection_id.clone(), endpoint_fingerprint: self.endpoint_fingerprint.clone(), revision: self.revision.clone(), ssh: self.ssh.clone(), tls: self.tls.clone(), ssh_password_present: self.secrets.password.is_some(), ssh_key_passphrase_present: self.secrets.key_passphrase.is_some(), http: self.http.clone(), http_bearer_present: self.secrets.http_bearer.is_some() } }
}
/// Deliberately NOT Serialize: decrypted credentials cannot accidentally enter IPC, config export or logs.
pub struct ResolvedRuntime {
    pub ssh: Option<super::SshConfig>, pub tls: Option<TlsOptions>, pub http: Option<ResolvedHttpRuntime>, pub revision: String, pub effective_fingerprint: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct LegacyMigrationReceipt {
    pub profile: RuntimeProfileView, pub cleaned_params: String,
    /// The caller MUST CAS-save its connection config before removing legacy plaintext; this function never edits config.
    pub connection_config_updated: bool,
}
#[derive(Clone)]
pub struct Store { root: PathBuf }
struct Lock { file: fs::File }
impl Drop for Lock {
    fn drop(&mut self) {
        #[cfg(unix)] unsafe { use std::os::fd::AsRawFd; let _ = flock(self.file.as_raw_fd(), 8); }
    }
}
#[cfg(unix)] extern "C" { fn flock(fd: i32, operation: i32) -> i32; }
fn valid_id(id: &str) -> Result<()> { if id.is_empty() || id.len() > 128 || id.chars().any(char::is_control) { bail!("RUNTIME_PROFILE_ID_INVALID: 连接ID无效"); } Ok(()) }
fn valid_fingerprint(value: &str) -> Result<()> { if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) { bail!("RUNTIME_PROFILE_BINDING_INVALID: 端点指纹必须是64位SHA256"); } Ok(()) }
fn patch_value(patch: &SecretPatch, previous: &Option<String>) -> Result<Option<String>> {
    match patch { SecretPatch::Keep => Ok(previous.clone()), SecretPatch::Clear => Ok(None), SecretPatch::Set(value) => {
        if value.is_empty() || value.len() > SECRET_LIMIT { bail!("RUNTIME_SECRET_INVALID: 新秘密必须为1字节至16KiB；删除请明确Clear"); }
        Ok(Some(value.clone()))
    } }
}
fn aad(id: &str) -> Vec<u8> { format!("navkit:runtime-profile:v1:{id}").into_bytes() }
impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self { Self { root: root.into() } }
    fn directory(&self) -> PathBuf { self.root.join("runtime-profiles") }
    fn path(&self, id: &str) -> PathBuf { self.directory().join(format!("{:x}.enc", Sha256::digest(id.as_bytes()))) }
    fn existing_key(&self) -> Result<Zeroizing<[u8; 32]>> {
        let path = self.root.join("key.bin");
        let metadata = fs::symlink_metadata(&path).map_err(|_| anyhow::anyhow!("RUNTIME_PROFILE_KEY_MISSING: 原本机key.bin缺失；已拒绝创建替代密钥"))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() != 32 { bail!("RUNTIME_PROFILE_KEY_INVALID: 原本机key.bin损坏或不是普通文件；不会覆盖"); }
        let mut file = fs::File::open(path).map_err(|_| anyhow::anyhow!("RUNTIME_PROFILE_KEY_INVALID: 原本机密钥不可读"))?;
        let mut key = Zeroizing::new([0; 32]); file.read_exact(key.as_mut()).map_err(|_| anyhow::anyhow!("RUNTIME_PROFILE_KEY_INVALID: 原本机密钥读取失败"))?;
        let mut extra = [0; 1]; if file.read(&mut extra)? != 0 { bail!("RUNTIME_PROFILE_KEY_INVALID: 原本机密钥长度已变化"); }
        Ok(key)
    }
    fn lock(&self) -> Result<Lock> {
        for path in [&self.root, &self.directory()] {
            if let Ok(metadata) = fs::symlink_metadata(path) { if metadata.file_type().is_symlink() || !metadata.is_dir() { bail!("RUNTIME_PROFILE_PATH_INVALID: 本机配置目录不能是符号链接或文件"); } }
        }
        fs::create_dir_all(self.directory())?;
        let lock_path = self.directory().join("profiles.lock");
        if let Ok(metadata) = fs::symlink_metadata(&lock_path) { if metadata.file_type().is_symlink() || !metadata.is_file() { bail!("RUNTIME_PROFILE_PATH_INVALID: 锁文件不能是符号链接"); } }
        let mut options = fs::OpenOptions::new(); options.read(true).write(true).create(true);
        #[cfg(windows)] { use std::os::windows::fs::OpenOptionsExt; options.share_mode(0); }
        // 有界重试：同实例并发（保活探测 vs 用户操作）与其它进程的瞬时占用不应误报 BUSY（BUG-R1-013）。
        // 注意：重试只发生在 lock() 唯一入口内部，不引入可重入互斥体（Windows share_mode(0) 不可重入）。
        let mut attempt: u32 = 0;
        loop {
            let file = match options.open(&lock_path) {
                Ok(file) => file,
                Err(_) if attempt < 4 => { attempt += 1; std::thread::sleep(std::time::Duration::from_millis(40u64 * attempt as u64)); continue; }
                Err(_) => bail!("RUNTIME_PROFILE_BUSY: 本机配置被其它进程或并发操作长期占用（已自动重试）"),
            };
            #[cfg(unix)] unsafe { use std::os::fd::AsRawFd; if flock(file.as_raw_fd(), 2 | 4) != 0 {
                if attempt < 4 { attempt += 1; std::thread::sleep(std::time::Duration::from_millis(40u64 * attempt as u64)); continue; }
                bail!("RUNTIME_PROFILE_BUSY: 本机配置被其它进程或并发操作长期占用（已自动重试）");
            } }
            return Ok(Lock { file });
        }
    }
    fn read(&self, id: &str, key: &[u8; 32]) -> Result<Option<StoredProfile>> {
        let path = self.path(id);
        let metadata = match fs::symlink_metadata(&path) { Ok(value) => value, Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None), Err(_) => bail!("RUNTIME_PROFILE_UNREADABLE: 本机配置文件不可读") };
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > LIMIT as u64 { bail!("RUNTIME_PROFILE_INVALID: 本机配置必须是有界普通文件"); }
        let bytes = crate::storage::read_bytes_limited(&path, LIMIT)?;
        if bytes.len() < 28 { bail!("RUNTIME_PROFILE_INVALID: 本机配置密文损坏"); }
        let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| anyhow::anyhow!("RUNTIME_PROFILE_CRYPTO_INVALID"))?;
        let plaintext = Zeroizing::new(cipher.decrypt(Nonce::from_slice(&bytes[..12]), Payload { msg: &bytes[12..], aad: &aad(id) }).map_err(|_| anyhow::anyhow!("RUNTIME_PROFILE_DECRYPT_FAILED: 原密钥不匹配、文件身份不符或密文损坏；不会覆盖"))?);
        let profile: StoredProfile = serde_json::from_slice(&plaintext).map_err(|_| anyhow::anyhow!("RUNTIME_PROFILE_INVALID: 本机配置数据结构无效"))?;
        if profile.format != "navkit-runtime-profile" || profile.version != 1 || profile.connection_id != id { bail!("RUNTIME_PROFILE_INVALID: 本机配置版本或连接身份不符"); }
        valid_fingerprint(&profile.endpoint_fingerprint)?; valid_fingerprint(&profile.revision)?;
        Ok(Some(profile))
    }
    fn read_existing(&self, id: &str) -> Result<Option<StoredProfile>> {
        let _lock = self.lock()?;
        match fs::symlink_metadata(self.path(id)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => bail!("RUNTIME_PROFILE_UNREADABLE: 本机配置文件不可读"),
            Ok(_) => {}
        }
        let key = self.existing_key()?;
        self.read(id, &key)
    }
    pub fn view(&self, id: &str) -> Result<Option<RuntimeProfileView>> {
        valid_id(id)?;
        Ok(self.read_existing(id)?.map(|profile| profile.view()))
    }
    pub fn resolve(&self, id: &str, fingerprint: &str) -> Result<Option<ResolvedRuntime>> {
        valid_id(id)?; valid_fingerprint(fingerprint)?;
        let profile = match self.read_existing(id)? { Some(profile) => profile, None => return Ok(None) };
        if profile.endpoint_fingerprint != fingerprint { bail!("RUNTIME_PROFILE_TARGET_CHANGED: 本机配置绑定的端点已变化，不能按同名连接复用凭据"); }
        self.resolve_stored(&profile).map(Some)
    }
    fn prepare_draft(&self, id: &str, fingerprint: &str, draft: &RuntimeProfileDraft, previous: Option<&StoredProfile>, expected_revision: Option<&str>) -> Result<StoredProfile> {
        self.prepare_draft_inner(id, fingerprint, draft, previous, expected_revision, false)
    }
    fn prepare_draft_inner(&self, id: &str, fingerprint: &str, draft: &RuntimeProfileDraft, previous: Option<&StoredProfile>, expected_revision: Option<&str>, allow_http_rebind: bool) -> Result<StoredProfile> {
        if previous.map(|profile| profile.revision.as_str()) != expected_revision { bail!("RUNTIME_PROFILE_CONFLICT: 本机配置已被其他窗口修改；保留草稿并重新加载"); }
        if previous.map(|profile| profile.endpoint_fingerprint.as_str() != fingerprint).unwrap_or(false) { bail!("RUNTIME_PROFILE_TARGET_CHANGED: 端点已变化；必须显式重新绑定新的本机配置"); }
        if draft.ssh.is_none() && (matches!(&draft.ssh_password, SecretPatch::Set(_)) || matches!(&draft.ssh_key_passphrase, SecretPatch::Set(_))) { bail!("RUNTIME_PROFILE_DRAFT_INCOMPLETE: 新SSH秘密必须绑定完整SSH目标；不会静默保存或删除"); }
        if draft.http.is_some() && (draft.ssh.is_some() || draft.tls.is_some()) { bail!("HTTP_RUNTIME_TRANSPORT_CONFLICT: HTTP外层通道与SSH/数据库TLS配置互斥，不能静默选路"); }
        if draft.http.is_none() && matches!(&draft.http_bearer, SecretPatch::Set(_)) { bail!("HTTP_RUNTIME_DRAFT_INCOMPLETE: 新Bearer必须绑定完整HTTP目标，不会写入params或保存未使用秘密"); }
        if let SecretPatch::Set(value) = &draft.http_bearer { http::validate_bearer(value)?; }
        let empty = Secrets::default(); let old_secrets = previous.map(|profile| &profile.secrets).unwrap_or(&empty);
        if let Some(next_http) = &draft.http {
            next_http.validate()?;
            if matches!(&draft.http_bearer, SecretPatch::Keep) && old_secrets.http_bearer.is_some() && !allow_http_rebind {
                let same_target = previous.and_then(|value| value.http.as_ref()).map(|old| old.same_target(next_http)).transpose()?.unwrap_or(false);
                if !same_target { bail!("HTTP_RUNTIME_REBIND_REQUIRED: HTTP端点/服务/目标/alias已改变，保留旧Bearer需要显式重新绑定和原版本CAS"); }
            }
        }
        Ok(StoredProfile {
            format: "navkit-runtime-profile".into(), version: 1, connection_id: id.into(), endpoint_fingerprint: fingerprint.into(),
            revision: format!("{:x}", Sha256::digest(uuid::Uuid::new_v4().as_bytes())), ssh: draft.ssh.clone(), tls: draft.tls.clone(), http: draft.http.clone(),
            secrets: Secrets { password: patch_value(&draft.ssh_password, &old_secrets.password)?, key_passphrase: patch_value(&draft.ssh_key_passphrase, &old_secrets.key_passphrase)?, http_bearer: patch_value(&draft.http_bearer, &old_secrets.http_bearer)? },
        })
    }
    /// Validates transient test inputs without writing a profile or creating a master key. Keep reads only the matching existing profile.
    pub fn resolve_draft(&self, id: &str, fingerprint: &str, draft: &RuntimeProfileDraft, expected_revision: Option<&str>) -> Result<ResolvedRuntime> {
        valid_id(id)?; valid_fingerprint(fingerprint)?;
        let previous = self.read_existing(id)?;
        let profile = self.prepare_draft(id, fingerprint, draft, previous.as_ref(), expected_revision)?;
        self.resolve_stored(&profile)
    }
    /// Only an explicit rebind request may carry matching existing secrets to a changed server endpoint.
    fn prepare_draft_rebinding(&self, id: &str, fingerprint: &str, draft: &RuntimeProfileDraft, previous: Option<&StoredProfile>, expected_revision: Option<&str>, rebind_endpoint: bool) -> Result<StoredProfile> {
        if !rebind_endpoint { return self.prepare_draft(id, fingerprint, draft, previous, expected_revision); }
        let previous = previous.ok_or_else(|| anyhow::anyhow!("RUNTIME_PROFILE_REBIND_MISSING: 没有已有本机配置可重新绑定"))?;
        if Some(previous.revision.as_str()) != expected_revision { bail!("RUNTIME_PROFILE_CONFLICT: 重新绑定仍须匹配原配置版本"); }
        let mut rebound = previous.clone(); rebound.endpoint_fingerprint = fingerprint.into();
        self.prepare_draft_inner(id, fingerprint, draft, Some(&rebound), expected_revision, true)
    }
    pub fn resolve_draft_rebinding(&self, id: &str, fingerprint: &str, draft: &RuntimeProfileDraft, expected_revision: Option<&str>, rebind_endpoint: bool) -> Result<ResolvedRuntime> {
        valid_id(id)?; valid_fingerprint(fingerprint)?;
        let previous = self.read_existing(id)?;
        let profile = self.prepare_draft_rebinding(id, fingerprint, draft, previous.as_ref(), expected_revision, rebind_endpoint)?;
        self.resolve_stored(&profile)
    }
    fn resolve_stored(&self, profile: &StoredProfile) -> Result<ResolvedRuntime> {
        if profile.http.is_some() && (profile.ssh.is_some() || profile.tls.is_some()) { bail!("HTTP_RUNTIME_TRANSPORT_CONFLICT: 密文本机HTTP通道不能与SSH/数据库TLS叠加"); }
        let http = profile.http.as_ref().map(|spec| {
            let bearer = profile.secrets.http_bearer.as_deref().ok_or_else(|| anyhow::anyhow!("HTTP_RUNTIME_BEARER_REQUIRED: HTTP通道缺少本机Bearer，不能直连回退"))?;
            http::resolve(spec, bearer)
        }).transpose()?;
        let ssh = profile.ssh.as_ref().map(|spec| super::SshConfig::from_profile(spec, profile.secrets.password.as_deref(), profile.secrets.key_passphrase.as_deref())).transpose()?;
        let tls = profile.tls.as_ref().map(|spec| spec.parameters().and_then(|params| tls_options::parse(&params, true))).transpose()?;
        if ssh.is_some() { tls.as_ref().ok_or_else(|| anyhow::anyhow!("SSH_TLS_IDENTITY_UNSUPPORTED: SSH必须明确选择TLS disable；不会静默覆盖默认prefer"))?.ensure_ssh_compatible()?; }
        let mut digest = Sha256::new();
        for value in [&profile.endpoint_fingerprint, &profile.revision] { digest.update((value.len() as u64).to_be_bytes()); digest.update(value); }
        if let Some(ssh) = &ssh { digest.update(ssh.material_fingerprint()); }
        if let Some(tls) = &tls { digest.update(tls.fingerprint()); }
        if let Some(http) = &http { digest.update(http.fingerprint.as_bytes()); }
        Ok(ResolvedRuntime { ssh, tls, http, revision: profile.revision.clone(), effective_fingerprint: format!("{:x}", digest.finalize()) })
    }
    pub fn save(&self, id: &str, fingerprint: &str, draft: &RuntimeProfileDraft, expected_revision: Option<&str>) -> Result<RuntimeProfileView> {
        valid_id(id)?; valid_fingerprint(fingerprint)?;
        let key = self.existing_key()?; let previous = self.read(id, &key)?;
        let profile = self.prepare_draft(id, fingerprint, draft, previous.as_ref(), expected_revision)?;
        self.resolve_stored(&profile)?; // All paths, host pins, TLS material and authentication are validated BEFORE replacing ciphertext.
        let plaintext = Zeroizing::new(serde_json::to_vec(&profile)?);
        if plaintext.len() > LIMIT - 28 { bail!("RUNTIME_PROFILE_LIMIT: 本机配置超过1MiB限制"); }
        if *self.existing_key()? != *key { bail!("RUNTIME_PROFILE_KEY_CHANGED: 原密钥在保存前变化；本次未写入"); }
        let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| anyhow::anyhow!("RUNTIME_PROFILE_CRYPTO_INVALID"))?;
        let mut nonce = [0; 12]; rand::rngs::OsRng.fill_bytes(&mut nonce);
        let ciphertext = cipher.encrypt(Nonce::from_slice(&nonce), Payload { msg: &plaintext, aad: &aad(id) }).map_err(|_| anyhow::anyhow!("RUNTIME_PROFILE_ENCRYPT_FAILED"))?;
        let mut bytes = nonce.to_vec(); bytes.extend_from_slice(&ciphertext);
        crate::storage::atomic_write(&self.path(id), &bytes)?;
        let verify_key = self.existing_key()?;
        if *verify_key != *key { bail!("RUNTIME_PROFILE_EFFECTS_UNKNOWN: 原密钥在落盘期间变化；配置结果未知，请保留原密钥和草稿核对"); }
        let verified = self.read(id, &verify_key)?.ok_or_else(|| anyhow::anyhow!("RUNTIME_PROFILE_EFFECTS_UNKNOWN: 本机配置写后读回缺失"))?;
        if verified.revision != profile.revision { bail!("RUNTIME_PROFILE_EFFECTS_UNKNOWN: 本机配置写后读回版本不一致"); }
        Ok(verified.view())
    }
    pub fn delete(&self, id: &str, fingerprint: &str, expected_revision: &str) -> Result<bool> {
        valid_id(id)?; valid_fingerprint(fingerprint)?; let key = self.existing_key()?; let _lock = self.lock()?;
        let profile = match self.read(id, &key)? { Some(value) => value, None => return Ok(false) };
        if profile.endpoint_fingerprint != fingerprint { bail!("RUNTIME_PROFILE_TARGET_CHANGED"); }
        if profile.revision != expected_revision { bail!("RUNTIME_PROFILE_CONFLICT"); }
        fs::remove_file(self.path(id))?;
        Ok(true)
    }
    /// Encrypts first. A failed call NEVER removes secrets from the caller's original params/config.
    pub fn migrate_legacy(&self, id: &str, fingerprint: &str, parameters: &str, expected_revision: Option<&str>, tls_capable: bool) -> Result<LegacyMigrationReceipt> {
        let pairs = tls_options::params::parse(parameters)?;
        if pairs.iter().any(|(name, _)| ["http_bearer", "http_token", "access_token", "authorization", "bearer"].contains(&name.as_str())) { bail!("HTTP_RUNTIME_MIGRATION_UNSUPPORTED: 不允许隐式迁移或删除附加参数中的HTTP秘密，请通过本机HTTP表单显式配置"); }
        let mut ssh_values = std::collections::BTreeMap::new();
        for (name, value) in &pairs {
            if name.starts_with("ssh_") {
                if ssh_values.insert(name.clone(), value.clone()).map(|old| old != *value).unwrap_or(false) { bail!("RUNTIME_PROFILE_MIGRATION_CONFLICT: 旧SSH参数有重复冲突；原值已保留"); }
            }
        }
        let password = ssh_values.remove("ssh_password"); let passphrase = ssh_values.remove("ssh_key_passphrase");
        let mut draft = RuntimeProfileDraft { ssh: None, tls: None, http: None, http_bearer: SecretPatch::Keep, ssh_password: password.map(SecretPatch::Set).unwrap_or_default(), ssh_key_passphrase: passphrase.map(SecretPatch::Set).unwrap_or_default() };
        if let Some(host) = ssh_values.remove("ssh_host") {
            let port = match ssh_values.remove("ssh_port") { Some(value) => value.parse::<u16>().ok().filter(|value| *value != 0).ok_or_else(|| anyhow::anyhow!("SSH_PORT_INVALID"))?, None => 22 };
            let user = ssh_values.remove("ssh_user").filter(|value| !value.is_empty()).ok_or_else(|| anyhow::anyhow!("SSH_USER_REQUIRED"))?;
            let known_hosts = ssh_values.remove("ssh_known_hosts").filter(|value| !value.is_empty()).ok_or_else(|| anyhow::anyhow!("SSH_KNOWN_HOSTS_REQUIRED: 迁移必须明确提供known_hosts；原密码未删除"))?;
            let key_file = ssh_values.remove("ssh_key_file").filter(|value| !value.is_empty());
            let has_password = matches!(&draft.ssh_password, SecretPatch::Set(_));
            if has_password && key_file.is_some() { bail!("SSH_AUTH_CONFLICT: 旧配置同时指定密码和私钥，必须明确选择后再迁移"); }
            let authentication = if has_password { SshAuthentication::Password } else { SshAuthentication::PrivateKey };
            draft.ssh = Some(SshProfile { host, port, user, authentication, key_file, known_hosts });
        }
        if draft.ssh.is_none() && (matches!(&draft.ssh_password, SecretPatch::Set(_)) || matches!(&draft.ssh_key_passphrase, SecretPatch::Set(_))) { bail!("RUNTIME_PROFILE_MIGRATION_INCOMPLETE: 旧SSH秘密缺少完整SSH目标；原值已保留，不会写入空配置或移除秘密"); }
        if !ssh_values.is_empty() { bail!("SSH_OPTION_UNSUPPORTED: 旧SSH参数不完整或有未知键，原值已保留"); }
        let tls_parameters = tls_options::params::filter(parameters, |key| tls_options::params::tls_key(key).is_some())?;
        if !tls_options::params::parse(&tls_parameters)?.is_empty() {
            let tls = tls_options::parse(&tls_parameters, tls_capable)?;
            draft.tls = Some(TlsProfile { mode: tls.mode, ca: tls.ca.clone(), cert: tls.cert.clone(), key: tls.key.clone() });
        }
        if draft.ssh.is_none() && draft.tls.is_none() { bail!("RUNTIME_PROFILE_MIGRATION_EMPTY: 没有可迁移的SSH/TLS配置"); }
        let profile = self.save(id, fingerprint, &draft, expected_revision)?;
        let cleaned_params = tls_options::params::filter(parameters, |key| !key.starts_with("ssh_") && tls_options::params::tls_key(key).is_none())?;
        Ok(LegacyMigrationReceipt { profile, cleaned_params, connection_config_updated: false })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Temp(PathBuf); impl Temp { fn new() -> Self { let path = std::env::temp_dir().join(format!("navkit-runtime-test-{}", uuid::Uuid::new_v4())); fs::create_dir_all(&path).unwrap(); Self(path) } }
    impl Drop for Temp { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
    fn draft() -> RuntimeProfileDraft { RuntimeProfileDraft { ssh: None, tls: Some(TlsProfile { mode: TlsMode::Disable, ca: None, cert: None, key: None }), http: None, http_bearer: SecretPatch::Keep, ssh_password: SecretPatch::Keep, ssh_key_passphrase: SecretPatch::Keep } }
    fn fingerprint() -> String { "a".repeat(64) }
    fn store() -> (Temp, Store) { let root = Temp::new(); fs::write(root.0.join("key.bin"), [9; 32]).unwrap(); let store = Store::new(&root.0); (root, store) }
    #[test] fn no_new_or_replacement_key_is_ever_created() { let root = Temp::new(); let store = Store::new(&root.0); assert!(store.save("c_legacy-id", &fingerprint(), &draft(), None).is_err()); assert!(!root.0.join("key.bin").exists()); }
    #[test] fn non_uuid_connection_ids_use_safe_hashed_paths() { let (root, store) = store(); let id = "c_old-session-123"; let saved = store.save(id, &fingerprint(), &draft(), None).unwrap(); assert_eq!(saved.connection_id, id); assert!(store.path(id).starts_with(root.0.join("runtime-profiles"))); }
    #[test] fn cas_conflict_preserves_existing_ciphertext() { let (_root, store) = store(); let view = store.save("c1", &fingerprint(), &draft(), None).unwrap(); let before = fs::read(store.path("c1")).unwrap(); assert!(store.save("c1", &fingerprint(), &draft(), None).is_err()); assert_eq!(before, fs::read(store.path("c1")).unwrap()); let saved = store.save("c1", &fingerprint(), &draft(), Some(&view.revision)).unwrap(); assert_ne!(view.revision, saved.revision); }
    #[test] fn same_name_or_id_does_not_bypass_endpoint_binding() { let (_root, store) = store(); store.save("c1", &fingerprint(), &draft(), None).unwrap(); assert!(store.resolve("c1", &"b".repeat(64)).is_err()); }
    #[test] fn ciphertext_is_bound_to_connection_identity() { let (_root, store) = store(); store.save("c1", &fingerprint(), &draft(), None).unwrap(); fs::copy(store.path("c1"), store.path("c2")).unwrap(); assert!(store.view("c2").is_err()); }
    #[test] fn damaged_key_is_retained_not_overwritten() { let (root, store) = store(); fs::write(root.0.join("key.bin"), b"broken").unwrap(); assert!(store.save("c1", &fingerprint(), &draft(), None).is_err()); assert_eq!(fs::read(root.0.join("key.bin")).unwrap(), b"broken"); }
    #[test] fn damaged_ciphertext_does_not_become_empty_profile() { let (_root, store) = store(); store.save("c1", &fingerprint(), &draft(), None).unwrap(); fs::write(store.path("c1"), b"broken").unwrap(); assert!(store.view("c1").is_err()); assert!(store.save("c1", &fingerprint(), &draft(), None).is_err()); }
    #[test] fn views_and_drafts_never_serialize_secrets() { let (_root, store) = store(); let view = store.save("c1", &fingerprint(), &draft(), None).unwrap(); let json = serde_json::to_string(&view).unwrap(); assert!(!json.contains("key_passphrase\":")); assert!(!format!("{:?}", SecretPatch::Set("PRIVATE-PASSWORD".into())).contains("PRIVATE-PASSWORD")); }
    #[test] fn migration_failure_never_deletes_old_plaintext_or_creates_key() { let root = Temp::new(); let store = Store::new(&root.0); let params = "ssh_host=h;ssh_user=u;ssh_password=retained"; assert!(store.migrate_legacy("c1", &fingerprint(), params, None, true).is_err()); assert!(params.contains("retained")); assert!(!root.0.join("key.bin").exists()); }
    #[test] fn orphan_legacy_secret_is_never_removed_when_tls_exists() { let (_root, store) = store(); let parameters = "ssh_password=retained;sslmode=disable"; assert!(store.migrate_legacy("c1", &fingerprint(), parameters, None, true).unwrap_err().to_string().contains("MIGRATION_INCOMPLETE")); assert!(parameters.contains("retained")); assert!(store.view("c1").unwrap().is_none()); }
    #[test] fn cross_instance_lock_is_real_not_only_memory_mutex() { let (_root, store) = store(); let _guard = store.lock().unwrap(); assert!(store.lock().is_err()); }
    #[test] fn deletion_is_cas_bound() { let (_root, store) = store(); let view = store.save("c1", &fingerprint(), &draft(), None).unwrap(); assert!(store.delete("c1", &fingerprint(), "stale").is_err()); assert!(store.delete("c1", &fingerprint(), &view.revision).unwrap()); }
    #[test] fn absent_profile_does_not_require_or_create_master_key() { let root = Temp::new(); let store = Store::new(&root.0); assert!(store.view("c1").unwrap().is_none()); assert!(store.resolve("c1", &fingerprint()).unwrap().is_none()); assert!(!root.0.join("key.bin").exists()); }
    #[test] fn transient_test_does_not_save_profile_or_initialize_key() { let root = Temp::new(); let store = Store::new(&root.0); let runtime = store.resolve_draft("c1", &fingerprint(), &draft(), None).unwrap(); assert!(runtime.tls.is_some()); assert!(!store.path("c1").exists()); assert!(!root.0.join("key.bin").exists()); }
    #[test] fn transient_test_preserves_revision_and_ciphertext() { let (_root, store) = store(); let view = store.save("c1", &fingerprint(), &draft(), None).unwrap(); let bytes = fs::read(store.path("c1")).unwrap(); store.resolve_draft("c1", &fingerprint(), &draft(), Some(&view.revision)).unwrap(); assert_eq!(bytes, fs::read(store.path("c1")).unwrap()); assert_eq!(view.revision, store.view("c1").unwrap().unwrap().revision); }
    #[test] fn keep_never_implicitly_clears_secret_when_ssh_is_disabled() { let (_root, store) = store(); let mut old = store.prepare_draft("c1", &fingerprint(), &draft(), None, None).unwrap(); old.secrets.password = Some("retained-password".into()); let next = store.prepare_draft("c1", &fingerprint(), &draft(), Some(&old), Some(&old.revision)).unwrap(); assert_eq!(next.secrets.password.as_deref(), Some("retained-password")); let mut clear = draft(); clear.ssh_password = SecretPatch::Clear; assert!(store.prepare_draft("c1", &fingerprint(), &clear, Some(&old), Some(&old.revision)).unwrap().secrets.password.is_none()); }
    #[test] fn endpoint_rebinding_requires_explicit_choice_and_original_revision() { let (_root, store) = store(); let original = store.save("c1", &fingerprint(), &draft(), None).unwrap(); let changed = "b".repeat(64); assert!(store.resolve_draft_rebinding("c1", &changed, &draft(), Some(&original.revision), false).is_err()); assert!(store.resolve_draft_rebinding("c1", &changed, &draft(), Some("stale"), true).is_err()); assert!(store.resolve_draft_rebinding("c1", &changed, &draft(), Some(&original.revision), true).is_ok()); assert_eq!(store.view("c1").unwrap().unwrap().endpoint_fingerprint, fingerprint()); }
    #[test] fn new_orphan_secret_is_rejected_not_discarded() { let (_root, store) = store(); let mut incomplete = draft(); incomplete.ssh_password = SecretPatch::Set("not-discarded".into()); assert!(store.save("c1", &fingerprint(), &incomplete, None).unwrap_err().to_string().contains("DRAFT_INCOMPLETE")); assert!(!store.path("c1").exists()); }
}
