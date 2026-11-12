//! PL-17 local HTTP configuration only. No discovery, network I/O, SQL or session authorization.
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;
use crate::tls_options::{self, TlsMode, TlsOptions};
use super::TlsProfile;

pub const HTTP_PROTOCOL: &str = "navkit-sql-tunnel/1";
pub const HTTP_BEARER_LIMIT: usize = 16 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpProfile {
    pub endpoint: String,
    pub protocol: String,
    pub expected_server_id: String,
    pub expected_target_id: String,
    pub target_alias: String,
    /// Outer HTTPS policy. This is NOT evidence of TLS between the bridge and its database.
    pub tls: TlsProfile,
    pub connect_timeout_ms: u32,
}

fn canonical_uuid(value: &str) -> Result<()> {
    let id = uuid::Uuid::parse_str(value).map_err(|_| anyhow::anyhow!("HTTP_RUNTIME_ID_INVALID: 服务和目标ID必须是规范UUID"))?;
    if id.to_string() != value { bail!("HTTP_RUNTIME_ID_INVALID: 服务和目标ID必须是小写、带连字符的规范UUID"); }
    Ok(())
}

pub fn validate_endpoint(value: &str) -> Result<url::Url> {
    if value.is_empty() || value.len() > 4096 || value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        bail!("HTTP_RUNTIME_ENDPOINT_INVALID: HTTP隧道端点必须是不含空白或控制字符的HTTPS origin");
    }
    // Check the raw path as well: URL parsers normalize '/a/../' and backslashes.
    let (scheme, rest) = value.split_once("://").ok_or_else(|| anyhow::anyhow!("HTTP_RUNTIME_ENDPOINT_INVALID: HTTP隧道要求HTTPS origin"))?;
    let authority = rest.strip_suffix('/').unwrap_or(rest);
    if !scheme.eq_ignore_ascii_case("https") || authority.is_empty() || authority.contains(['/', '\\', '@', '?', '#']) {
        bail!("HTTP_RUNTIME_ENDPOINT_INVALID: HTTPS origin不能携带userinfo、路径、查询或片段");
    }
    let parsed = url::Url::parse(value).map_err(|_| anyhow::anyhow!("HTTP_RUNTIME_ENDPOINT_INVALID: HTTPS origin无效"))?;
    if parsed.scheme() != "https" || parsed.host_str().is_none() || !parsed.username().is_empty() || parsed.password().is_some() || parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some() {
        bail!("HTTP_RUNTIME_ENDPOINT_INVALID: HTTPS origin不能携带userinfo、路径、查询或片段");
    }
    Ok(parsed)
}

pub fn validate_bearer(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > HTTP_BEARER_LIMIT || value.bytes().any(|b| !(0x21..=0x7e).contains(&b)) {
        bail!("HTTP_RUNTIME_BEARER_INVALID: Bearer令牌必须为1字节至16KiB的可见ASCII字符，不允许空白、CRLF或控制字符");
    }
    Ok(())
}

impl HttpProfile {
    pub fn validate(&self) -> Result<url::Url> {
        let endpoint = validate_endpoint(&self.endpoint)?;
        if self.protocol != HTTP_PROTOCOL { bail!("HTTP_RUNTIME_PROTOCOL_UNSUPPORTED: 只支持navkit-sql-tunnel/1，不会按普通REST回退"); }
        canonical_uuid(&self.expected_server_id)?;
        canonical_uuid(&self.expected_target_id)?;
        if self.target_alias.is_empty() || self.target_alias.len() > 200 || self.target_alias.trim() != self.target_alias || self.target_alias.chars().any(char::is_control) {
            bail!("HTTP_RUNTIME_ALIAS_INVALID: 必须明确填写桥接服务预配alias（1至200字节），不能自动发现或猜测目标");
        }
        if !(1000..=60_000).contains(&self.connect_timeout_ms) { bail!("HTTP_RUNTIME_TIMEOUT_INVALID: 连接期限必须为1000至60000毫秒"); }
        if self.tls.mode != TlsMode::VerifyFull { bail!("HTTP_RUNTIME_TLS_REQUIRED: 外层HTTPS必须VerifyFull验证CA与原主机名，不允许明文或跳过身份校验"); }
        Ok(endpoint)
    }

    /// Transport identity changes must not reuse an old token without an explicit CAS rebind.
    pub fn same_target(&self, other: &Self) -> Result<bool> {
        Ok(self.validate()?.as_str() == other.validate()?.as_str()
            && self.protocol == other.protocol
            && self.expected_server_id == other.expected_server_id
            && self.expected_target_id == other.expected_target_id
            && self.target_alias == other.target_alias)
    }
}

/// NOT Serialize. Only the trusted local network caller receives bearer and immutable PEM bytes.
pub struct ResolvedHttpRuntime {
    pub profile: HttpProfile,
    pub endpoint: url::Url,
    pub tls: TlsOptions,
    pub bearer: Zeroizing<String>,
    pub fingerprint: String,
}
impl std::fmt::Debug for ResolvedHttpRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ResolvedHttpRuntime")
            .field("protocol", &self.profile.protocol)
            .field("bearer_present", &!self.bearer.is_empty())
            .field("tls", &self.tls)
            .finish_non_exhaustive()
    }
}
fn hash_value(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}
pub fn resolve(profile: &HttpProfile, bearer: &str) -> Result<ResolvedHttpRuntime> {
    let endpoint = profile.validate()?;
    validate_bearer(bearer)?;
    let tls = tls_options::parse(&profile.tls.parameters()?, true)?;
    let mut digest = Sha256::new();
    digest.update(b"navkit:local-http-runtime:v1\0");
    hash_value(&mut digest, endpoint.as_str().as_bytes());
    hash_value(&mut digest, &serde_json::to_vec(profile)?);
    hash_value(&mut digest, tls.fingerprint().as_bytes());
    // Bind the actual validated private-key bytes as well as CA/certificate content.
    match &tls.material().key {
        Some(key) => { digest.update([1]); hash_value(&mut digest, key.as_slice()); },
        None => digest.update([0]),
    }
    hash_value(&mut digest, bearer.as_bytes());
    Ok(ResolvedHttpRuntime { profile: profile.clone(), endpoint, tls, bearer: Zeroizing::new(bearer.to_owned()), fingerprint: format!("{:x}", digest.finalize()) })
}
