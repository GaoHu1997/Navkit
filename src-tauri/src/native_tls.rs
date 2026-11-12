//! Native Kafka/MQTT TLS: strict modes, real PEM validation, no silent plaintext.
use anyhow::{bail, Result};
use crate::tls_options::{self, TlsMode};
use zeroize::Zeroizing;
#[derive(Clone)]
pub struct NativeTls {
    pub enabled: bool, pub requested_mode: TlsMode, pub enforced_mode: TlsMode,
    pub ca: Option<Vec<u8>>, pub cert: Option<Vec<u8>>, pub key: Option<Zeroizing<Vec<u8>>>,
}
impl std::fmt::Debug for NativeTls {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("NativeTls").field("enabled", &self.enabled).field("requested_mode", &self.requested_mode).field("enforced_mode", &self.enforced_mode).field("ca_present", &self.ca.is_some()).field("cert_present", &self.cert.is_some()).field("key_present", &self.key.is_some()).finish()
    }
}
impl Default for NativeTls {
    fn default() -> Self { Self { enabled: false, requested_mode: TlsMode::Disable, enforced_mode: TlsMode::Disable, ca: None, cert: None, key: None } }
}
impl NativeTls {
    pub fn parse(parameters: &str) -> Result<Self> {
        let pairs = tls_options::params::parse(parameters)?;
        let mut requested = None;
        let mut material = serde_json::Map::new();
        for (name, value) in pairs {
            let canonical = if matches!(name.as_str(), "security.protocol" | "security_protocol") { Some("security_protocol") } else { tls_options::params::tls_key(&name) };
            let canonical = match canonical { Some(value) => value, None => {
                if name.starts_with("tls_") || name.starts_with("ssl_") || name.starts_with("tls-") || name.starts_with("ssl-") { bail!("NATIVE_TLS_OPTION_UNSUPPORTED: TLS选项{}不受支持", name); }
                continue;
            } };
            match canonical {
                "mode" | "security_protocol" => {
                    let mode = if canonical == "security_protocol" {
                        match value.to_ascii_uppercase().as_str() { "SSL" => TlsMode::VerifyFull, "PLAINTEXT" => TlsMode::Disable, _ => bail!("NATIVE_TLS_AUTH_UNSUPPORTED: security_protocol模式未实现，不会回退明文") }
                    } else { tls_options::mode(&value)? };
                    if requested.map(|old| old != mode).unwrap_or(false) { bail!("NATIVE_TLS_OPTION_CONFLICT: TLS模式别名冲突"); }
                    requested = Some(mode);
                }
                "ca" | "cert" | "key" => {
                    if value.is_empty() { bail!("TLS_PATH_INVALID: 原生TLS材料路径不能为空"); }
                    if material.get(canonical).and_then(serde_json::Value::as_str).map(|old| old != value).unwrap_or(false) { bail!("NATIVE_TLS_OPTION_CONFLICT: TLS材料路径别名冲突"); }
                    material.insert(canonical.into(), value.into());
                }
                "insecure" => { if tls_options::params::bool_value(&value, &name)? { bail!("TLS_INSECURE_FORBIDDEN: 原生TLS不允许绕过服务器验证"); } }
                "server_name" => bail!("TLS_NAME_OVERRIDE_UNSUPPORTED: 原生TLS必须绑定原端点名称"),
                _ => unreachable!(),
            }
        }
        if !material.is_empty() && requested.is_none() { bail!("TLS_MODE_REQUIRED: 有原生TLS材料必须明确开启TLS"); }
        let requested = requested.unwrap_or(TlsMode::Disable);
        if requested == TlsMode::Disable {
            if !material.is_empty() { bail!("TLS_MATERIAL_MODE_CONFLICT: 关闭TLS时不能静默忽略CA或客户端密钥"); }
            return Ok(Self::default());
        }
        if requested == TlsMode::Prefer { bail!("NATIVE_TLS_MODE_UNSUPPORTED: 原生TLS不支持prefer明文回退；请明确verify-full或disable"); }
        material.insert("sslmode".into(), "verify-full".into());
        let validated = tls_options::parse(&serde_json::to_string(&material)?, true)?;
        let material = validated.material();
        Ok(Self { enabled: true, requested_mode: requested, enforced_mode: TlsMode::VerifyFull, ca: material.ca.clone(), cert: material.cert.clone(), key: material.key.clone() })
    }
    pub fn mqtt(&self) -> Result<rumqttc::TlsConfiguration> {
        if !self.enabled { bail!("NATIVE_TLS_DISABLED: 此配置未开启TLS"); }
        let ca = self.ca.clone().ok_or_else(|| anyhow::anyhow!("MQTT_TLS_CA_REQUIRED: 此MQTT客户端的显式TLS配置需要CA文件"))?;
        let client_auth = match (&self.cert, &self.key) { (Some(cert), Some(key)) => Some((cert.clone(), key.to_vec())), (None, None) => None, _ => bail!("TLS_CLIENT_PAIR_REQUIRED: 客户端证书密钥缺失") };
        Ok(rumqttc::TlsConfiguration::Simple { ca, alpn: None, client_auth })
    }
    pub fn kafka(&self) -> Result<std::sync::Arc<rustls::ClientConfig>> {
        if !self.enabled { bail!("NATIVE_TLS_DISABLED: 此配置未开启TLS"); }
        let material = tls_options::material::TlsMaterial { ca: self.ca.clone(), cert: self.cert.clone(), key: self.key.clone() };
        let mut roots = material.roots()?;
        if self.ca.is_none() {
            for certificate in rustls_native_certs::load_native_certs().certs { let _ = roots.add(certificate); }
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        }
        if roots.is_empty() { bail!("TLS_CA_REQUIRED: 没有可用的系统或自定义CA"); }
        let builder = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions().map_err(|_| anyhow::anyhow!("TLS_PROTOCOL_INVALID: TLS协议配置无效"))?.with_root_certificates(roots);
        let config = match (&self.cert, &self.key) {
            (Some(cert), Some(key)) => builder.with_client_auth_cert(tls_options::material::certificates(cert)?, tls_options::material::private_key(key)?)
                .map_err(|_| anyhow::anyhow!("TLS_CLIENT_KEY_MISMATCH: 客户端证书私钥配置不匹配"))?,
            (None, None) => builder.with_no_client_auth(),
            _ => bail!("TLS_CLIENT_PAIR_REQUIRED: 客户端证书密钥缺失"),
        };
        Ok(std::sync::Arc::new(config))
    }
}
pub fn parse(parameters: &str) -> Result<NativeTls> { NativeTls::parse(parameters) }

/// Unsupported native protocols MUST call this before any socket operation.
pub fn no_tls(parameters: &str, protocol: &str) -> Result<()> {
    let tls = NativeTls::parse(parameters)?;
    if tls.enabled { bail!("NATIVE_TLS_UNSUPPORTED: {}的原生TLS尚未接入；连接已拒绝", protocol); }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn invalid_boolean_mode_never_becomes_plaintext() { for value in ["tls=typo", "sslmode=banana", "tls=", "tls=true;ssl=false"] { assert!(NativeTls::parse(value).is_err()); } }
    #[test] fn native_defaults_and_explicit_off_are_clear() { assert!(!NativeTls::parse("").unwrap().enabled); assert!(!NativeTls::parse("{\"tls\":false}").unwrap().enabled); }
    #[test] fn false_with_material_is_not_ignored() { assert!(NativeTls::parse("tls=false;ca=missing.pem").is_err()); assert!(NativeTls::parse("ca=missing.pem").is_err()); }
    #[test] fn prefer_is_not_a_hidden_downgrade() { assert!(NativeTls::parse("sslmode=prefer").is_err()); }
    #[test] fn enabled_transport_enforces_full_identity() { let value = NativeTls::parse("tls=true").unwrap(); assert_eq!(value.enforced_mode, TlsMode::VerifyFull); assert!(value.kafka().is_ok()); assert!(value.mqtt().is_err()); }
    #[test] fn require_is_explicitly_enforced_more_strictly() { let value = NativeTls::parse("sslmode=require").unwrap(); assert_eq!(value.requested_mode, TlsMode::Require); assert_eq!(value.enforced_mode, TlsMode::VerifyFull); }
    #[test] fn unsupported_native_protocol_does_not_dial_plaintext() { assert!(no_tls("tls=true", "Cassandra").is_err()); assert!(no_tls("tls=typo", "ZooKeeper").is_err()); assert!(no_tls("tls=false", "Nebula").is_ok()); }
    #[test] fn client_key_is_not_dumped_in_debug() { let tls = NativeTls { key: Some(Zeroizing::new(b"never-echo-private-key".to_vec())), ..Default::default() }; assert!(!format!("{tls:?}").contains("never-echo-private-key")); }
    #[test] fn security_protocol_does_not_fake_sasl_tls_support() { assert!(NativeTls::parse("security_protocol=SASL_SSL").is_err()); assert!(NativeTls::parse("security_protocol=SSL;tls=false").is_err()); }
}
