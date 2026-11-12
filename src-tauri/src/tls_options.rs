//! Strict SQLx TLS modes and validated, immutable certificate material.
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use sqlx::{mysql::{MySqlConnectOptions, MySqlConnection, MySqlSslMode}, postgres::{PgConnectOptions, PgConnection, PgSslMode}, Row};
use crate::mysql_options::OptionDiagnostic;
#[path = "connection_params.rs"] pub mod params;
#[path = "tls_material.rs"] pub mod material;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TlsMode { Disable, Prefer, Require, VerifyCa, VerifyFull }
#[derive(Clone)]
pub struct TlsOptions {
    pub mode: TlsMode, pub ca: Option<String>, pub cert: Option<String>, pub key: Option<String>,
    pub diagnostics: Vec<OptionDiagnostic>, material: material::TlsMaterial,
}
impl std::fmt::Debug for TlsOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("TlsOptions").field("mode", &self.mode).field("material", &self.material).finish()
    }
}
impl Default for TlsOptions {
    fn default() -> Self { Self { mode: TlsMode::Prefer, ca: None, cert: None, key: None, diagnostics: Vec::new(), material: Default::default() } }
}
#[derive(Debug, Clone, Serialize)]
pub struct TlsSessionEvidence {
    pub requested_mode: TlsMode, pub encrypted: bool, pub cipher: Option<String>,
    /// Configured driver policy, NOT a claim that server-side SQL exposes the certificate subject.
    pub identity_policy: String,
}

pub fn mode(value: &str) -> Result<TlsMode> {
    match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
        "disable" | "disabled" | "false" | "0" | "no" | "off" => Ok(TlsMode::Disable),
        "prefer" | "preferred" => Ok(TlsMode::Prefer),
        "require" | "required" => Ok(TlsMode::Require),
        "verify-ca" => Ok(TlsMode::VerifyCa),
        "verify-full" | "verify-identity" | "true" | "1" | "yes" | "on" => Ok(TlsMode::VerifyFull),
        _ => bail!("TLS_MODE_INVALID: TLS模式无效；不会回退明文"),
    }
}

pub fn parse(parameters: &str, tls_capable: bool) -> Result<TlsOptions> {
    let pairs = params::parse(parameters)?;
    let mut selected = None;
    let mut ca: Option<String> = None;
    let mut cert: Option<String> = None;
    let mut key: Option<String> = None;
    for (name, value) in pairs {
        let canonical = match params::tls_key(&name) {
            Some(value) => value,
            None => {
                if name.starts_with("ssl_") || name.starts_with("ssl-") || name.starts_with("tls_") || name.starts_with("tls-") { bail!("TLS_OPTION_UNSUPPORTED: TLS参数{}不支持", name); }
                continue;
            }
        };
        match canonical {
            "mode" => {
                let new = mode(&value)?;
                if selected.map(|old| old != new).unwrap_or(false) { bail!("TLS_OPTION_CONFLICT: TLS模式别名冲突；不会last-wins"); }
                selected = Some(new);
            }
            "ca" | "cert" | "key" => {
                if value.is_empty() { bail!("TLS_PATH_INVALID: 显式TLS材料路径不能为空"); }
                let slot = match canonical { "ca" => &mut ca, "cert" => &mut cert, _ => &mut key };
                if slot.as_ref().map(|old| old != &value).unwrap_or(false) { bail!("TLS_OPTION_CONFLICT: TLS材料路径别名冲突"); }
                *slot = Some(value);
            }
            "insecure" => { if params::bool_value(&value, &name)? { bail!("TLS_INSECURE_FORBIDDEN: 不允许通过insecure跳过TLS校验"); } }
            "server_name" => bail!("TLS_NAME_OVERRIDE_UNSUPPORTED: 驱动未提供独立拨号地址和TLS身份的安全绑定，不能覆盖校验名称"),
            _ => unreachable!(),
        }
    }
    let has_material = ca.is_some() || cert.is_some() || key.is_some();
    if has_material && selected.is_none() { bail!("TLS_MODE_REQUIRED: 有证书材料时必须明确选择verify-ca或verify-full；不会默认prefer降级"); }
    let selected = selected.unwrap_or(if tls_capable { TlsMode::Prefer } else { TlsMode::Disable });
    if !tls_capable && (selected != TlsMode::Disable || has_material) { bail!("TLS_UNSUPPORTED: 此协议没有已接入的TLS能力；连接已拒绝"); }
    if has_material && !matches!(selected, TlsMode::VerifyCa | TlsMode::VerifyFull) { bail!("TLS_MATERIAL_MODE_CONFLICT: TLS材料不能在disable/prefer/require模式下被静默忽略"); }
    let material = material::TlsMaterial::load(ca.as_deref(), cert.as_deref(), key.as_deref())?;
    let message = match selected {
        TlsMode::Disable => "TLS已明确关闭；不会声称加密",
        TlsMode::Prefer => "兼容prefer允许明文回退；尚未发生握手，不代表TLS已验证",
        TlsMode::Require => "要求加密但不验证服务器身份；尚未发生握手",
        TlsMode::VerifyCa => "要求CA验证但不校验hostname；尚未发生握手",
        TlsMode::VerifyFull => "要求CA及原目标hostname验证；尚未发生握手",
    };
    Ok(TlsOptions { mode: selected, ca, cert, key, material, diagnostics: vec![OptionDiagnostic { key: "tls".into(), status: "configured".into(), message: message.into() }] })
}

impl TlsOptions {
    /// Form inspection only: never report configured policy as a verified TLS handshake.
    pub fn diagnostics_for_ui(&self) -> Vec<OptionDiagnostic> { self.diagnostics.clone() }
    pub fn apply_mysql(&self, mut options: MySqlConnectOptions) -> MySqlConnectOptions {
        options = options.ssl_mode(match self.mode { TlsMode::Disable => MySqlSslMode::Disabled, TlsMode::Prefer => MySqlSslMode::Preferred, TlsMode::Require => MySqlSslMode::Required, TlsMode::VerifyCa => MySqlSslMode::VerifyCa, TlsMode::VerifyFull => MySqlSslMode::VerifyIdentity });
        if let Some(bytes) = &self.material.ca { options = options.ssl_ca_from_pem(bytes.clone()); }
        if let Some(bytes) = &self.material.cert { options = options.ssl_client_cert_from_pem(bytes); }
        if let Some(bytes) = &self.material.key { options = options.ssl_client_key_from_pem(bytes.as_slice()); }
        options
    }
    pub fn apply_pg(&self, mut options: PgConnectOptions) -> PgConnectOptions {
        options = options.ssl_mode(match self.mode { TlsMode::Disable => PgSslMode::Disable, TlsMode::Prefer => PgSslMode::Prefer, TlsMode::Require => PgSslMode::Require, TlsMode::VerifyCa => PgSslMode::VerifyCa, TlsMode::VerifyFull => PgSslMode::VerifyFull });
        if let Some(bytes) = &self.material.ca { options = options.ssl_root_cert_from_pem(bytes.clone()); }
        if let Some(bytes) = &self.material.cert { options = options.ssl_client_cert_from_pem(bytes); }
        if let Some(bytes) = &self.material.key { options = options.ssl_client_key_from_pem(bytes.as_slice()); }
        options
    }
    /// SQLx does not safely separate TCP dial-host from TLS SNI/certificate identity here.
    pub fn ensure_ssh_compatible(&self) -> Result<()> {
        if self.mode != TlsMode::Disable { bail!("SSH_TLS_IDENTITY_UNSUPPORTED: SSH本地拨号地址与TLS原hostname不能安全分离；不会改验127.0.0.1或降级TLS"); }
        Ok(())
    }
    pub fn material(&self) -> &material::TlsMaterial { &self.material }
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(serde_json::to_vec(&(self.mode, self.material.public_fingerprint())).expect("TLS policy serialization")))
    }
    fn evidence(&self, encrypted: bool, cipher: Option<String>) -> Result<TlsSessionEvidence> {
        if (self.mode == TlsMode::Disable && encrypted) || (matches!(self.mode, TlsMode::Require | TlsMode::VerifyCa | TlsMode::VerifyFull) && !encrypted) { bail!("TLS_NEGOTIATION_MISMATCH: 实际会话加密状态不符合请求；不会报告握手成功"); }
        Ok(TlsSessionEvidence { requested_mode: self.mode, encrypted, cipher, identity_policy: match self.mode { TlsMode::VerifyFull => "driver_ca_and_original_hostname", TlsMode::VerifyCa => "driver_ca_without_hostname", _ => "server_identity_not_verified" }.into() })
    }
    pub async fn verify_mysql_connection(&self, connection: &mut MySqlConnection) -> Result<TlsSessionEvidence> {
        let row = crate::sql_history::query("SHOW SESSION STATUS LIKE 'Ssl_cipher'").fetch_optional(&mut *connection).await.map_err(|_| anyhow::anyhow!("TLS_EVIDENCE_UNAVAILABLE: 无法读取同一MySQL会话加密证据"))?
            .ok_or_else(|| anyhow::anyhow!("TLS_EVIDENCE_UNAVAILABLE: MySQL未返回会话加密证据"))?;
        let cipher: String = row.try_get(1).map_err(|_| anyhow::anyhow!("TLS_EVIDENCE_UNAVAILABLE: MySQL会话加密证据格式无效"))?;
        self.evidence(!cipher.is_empty(), if cipher.is_empty() { None } else { Some(cipher) })
    }
    pub async fn verify_pg_connection(&self, connection: &mut PgConnection) -> Result<TlsSessionEvidence> {
        let row = crate::sql_history::query("SELECT ssl, cipher FROM pg_catalog.pg_stat_ssl WHERE pid=pg_backend_pid()").fetch_optional(&mut *connection).await.map_err(|_| anyhow::anyhow!("TLS_EVIDENCE_UNAVAILABLE: 无法读取同一PostgreSQL会话加密证据"))?
            .ok_or_else(|| anyhow::anyhow!("TLS_EVIDENCE_UNAVAILABLE: PostgreSQL未返回会话加密证据"))?;
        self.evidence(row.try_get("ssl")?, row.try_get("cipher")?)
    }
}
/// Must run before PgConnectOptions.options; local transport/TLS paths are NOT server startup GUCs.
pub fn server_startup_params(parameters: &str) -> Result<String> {
    params::filter(parameters, |key| params::tls_key(key).is_none() && !key.starts_with("ssh_") && key != "runtime_profile_id" && !params::keepalive_key(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn modes_have_explicit_semantics() { assert_eq!(parse("sslmode=require", true).unwrap().mode, TlsMode::Require); assert_eq!(parse("ssl=true", true).unwrap().mode, TlsMode::VerifyFull); assert_eq!(parse("ssl=false;sslmode=disable", true).unwrap().mode, TlsMode::Disable); }
    #[test] fn invalid_or_conflicting_mode_does_not_become_plaintext() { for value in ["sslmode=typo", "ssl=typo", "ssl=true;sslmode=disable", "sslmode="] { assert!(parse(value, true).is_err()); } }
    #[test] fn absent_mode_with_certificate_does_not_allow_fallback() { assert!(parse("ssl_ca=missing.pem", true).unwrap_err().to_string().contains("TLS_MODE_REQUIRED")); }
    #[test] fn material_in_disable_prefer_require_is_not_ignored() { for mode in ["disable", "prefer", "require"] { assert!(parse(&format!("sslmode={mode};ssl_ca=missing.pem"), true).is_err()); } }
    #[test] fn client_certificate_requires_key() { assert!(parse("sslmode=verify-full;ssl_cert=missing.pem", true).is_err()); }
    #[test] fn unsupported_protocol_rejects_tls_without_dial() { assert!(parse("sslmode=require", false).is_err()); assert!(parse("sslmode=disable", false).is_ok()); assert!(parse("", false).is_ok()); }
    #[test] fn ca_and_key_aliases_do_not_last_win() { assert!(parse("sslmode=verify-full;ssl_ca=a;ca=b", true).unwrap_err().to_string().contains("TLS_OPTION_CONFLICT")); }
    #[test] fn prefer_is_not_reported_verified() { let value = parse("", true).unwrap(); assert_eq!(value.diagnostics[0].status, "configured"); assert!(value.diagnostics[0].message.contains("明文")); }
    #[test] fn no_insecure_or_arbitrary_sni_override() { assert!(parse("tls=true;tls_insecure=true", true).is_err()); assert!(parse("sslmode=verify-full;tls_server_name=other", true).is_err()); }
    #[test] fn ssh_plus_tls_preserves_identity_by_refusing_unsupported_dial() { assert!(parse("sslmode=verify-full", true).unwrap().ensure_ssh_compatible().is_err()); assert!(parse("", true).unwrap().ensure_ssh_compatible().is_err()); assert!(parse("sslmode=disable", true).unwrap().ensure_ssh_compatible().is_ok()); }
    #[test] fn real_session_evidence_cannot_claim_required_plaintext() { assert!(parse("sslmode=require", true).unwrap().evidence(false, None).is_err()); assert!(!parse("", true).unwrap().evidence(false, None).unwrap().encrypted); }
    #[test] fn pg_startup_never_receives_local_tls_or_ssh_fields() { assert_eq!(server_startup_params("sslmode=verify-full;ssl_ca=private.pem;ssh_host=h;ssh_key_file=k;keepalive_interval=30;application_name=navkit").unwrap(), "application_name=navkit"); }
    #[test] fn garbage_pem_is_not_certificate_validation() { assert!(material::certificates(b"-----BEGIN CERTIFICATE-----\nZmFrZQ==\n-----END CERTIFICATE-----").is_err()); }
}
