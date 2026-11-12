//! MySQL/MariaDB高级参数：验证、真实驱动配置和同一物理会话读回。
use anyhow::{bail, Result};
use serde::Serialize;
use sqlx::mysql::{MySqlConnectOptions, MySqlConnection};
use sqlx::Row;

pub const DEFAULT_CHARSET: &str = "utf8mb4";
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OptionDiagnostic { pub key: String, pub status: String, pub message: String }
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MysqlAdvancedOptions {
    pub charset: Option<String>, pub collation: Option<String>, pub socket: Option<String>,
    pub diagnostics: Vec<OptionDiagnostic>,
}
#[derive(Debug, Clone, Serialize)]
pub struct MysqlSessionEvidence {
    pub character_set_client: String, pub character_set_connection: String,
    pub character_set_results: String, pub collation_connection: String,
    pub status: String,
}
fn valid_ident(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
fn normal_charset(value: &str) -> &str { if value == "utf8" { "utf8mb3" } else { value } }

pub fn parse(params: &str) -> Result<MysqlAdvancedOptions> {
    let mut out = MysqlAdvancedOptions::default();
    let mut seen: Vec<(String, String)> = Vec::new();
    for (key, value) in crate::tls_options::params::parse(params)? {
        let canonical = match key.as_str() {
            "charset" | "character_set" | "character-set" => "charset",
            "collation" => "collation",
            "socket" | "unix_socket" | "unix-socket" => "socket",
            _ if crate::tls_options::params::tls_key(&key).is_some() || key.starts_with("ssh_") || key == "runtime_profile_id" || crate::tls_options::params::keepalive_key(&key) => continue,
            _ => bail!("MYSQL_OPTION_UNSUPPORTED: 附加参数{}不能由此MySQL驱动应用；连接已拒绝", key),
        };
        if let Some((_, previous)) = seen.iter().find(|(name, _)| name == canonical) {
            if previous != &value { bail!("MYSQL_OPTION_CONFLICT: 参数{}有冲突别名，连接已拒绝", canonical); }
            continue;
        }
        seen.push((canonical.into(), value.clone()));
        match canonical {
            "charset" => { if !valid_ident(&value) { bail!("MYSQL_CHARSET_INVALID: charset格式无效"); } out.charset = Some(value.to_ascii_lowercase()); }
            "collation" => { if !valid_ident(&value) { bail!("MYSQL_COLLATION_INVALID: collation格式无效"); } out.collation = Some(value.to_ascii_lowercase()); }
            "socket" => {
                if value.is_empty() || value.len() > 32768 || value.chars().any(char::is_control) { bail!("MYSQL_SOCKET_INVALID: socket路径无效"); }
                #[cfg(windows)] { bail!("MYSQL_SOCKET_UNSUPPORTED: 此Windows驱动不支持Unix socket；不会回退TCP"); }
                #[cfg(not(windows))] { out.socket = Some(value); }
            }
            _ => unreachable!(),
        }
        out.diagnostics.push(OptionDiagnostic { key: canonical.into(), status: "configured".into(), message: "已写入驱动配置；实际会话尚未读回验证".into() });
    }
    if let Some(collation) = &out.collation {
        let charset = out.charset.as_deref().unwrap_or(DEFAULT_CHARSET);
        let prefix = collation.split('_').next().unwrap_or("");
        if normal_charset(prefix) != normal_charset(charset) { bail!("MYSQL_COLLATION_CONFLICT: collation不属于所选charset"); }
    }
    Ok(out)
}
impl MysqlAdvancedOptions {
    /// Form inspection only: these diagnostics cannot substitute same-session evidence.
    pub fn diagnostics_for_ui(&self) -> Vec<OptionDiagnostic> { self.diagnostics.clone() }
    pub fn apply(&self, mut options: MySqlConnectOptions) -> MySqlConnectOptions {
        options = options.charset(self.charset.as_deref().unwrap_or(DEFAULT_CHARSET));
        if let Some(value) = &self.collation { options = options.collation(value); }
        if let Some(value) = &self.socket { options = options.socket(value); }
        options
    }
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let data = serde_json::to_vec(&(&self.charset, &self.collation, &self.socket)).expect("string option serialization");
        format!("{:x}", Sha256::digest(data))
    }
    /// Call from SQLx after_connect for EVERY physical session, not a different pooled connection.
    pub async fn verify_connection(&self, connection: &mut MySqlConnection) -> Result<MysqlSessionEvidence> {
        let row = crate::sql_history::query("SELECT @@character_set_client AS c1, @@character_set_connection AS c2, @@character_set_results AS c3, @@collation_connection AS co")
            .fetch_one(&mut *connection).await?;
        let evidence = MysqlSessionEvidence {
            character_set_client: row.try_get("c1")?, character_set_connection: row.try_get("c2")?,
            character_set_results: row.try_get("c3")?, collation_connection: row.try_get("co")?, status: "verified".into(),
        };
        let wanted = normal_charset(self.charset.as_deref().unwrap_or(DEFAULT_CHARSET));
        if [&evidence.character_set_client, &evidence.character_set_connection, &evidence.character_set_results].iter().any(|value| normal_charset(value) != wanted) {
            bail!("MYSQL_SESSION_MISMATCH: 实际字符集会话与请求不一致；连接不能作为已验证连接使用");
        }
        if self.collation.as_ref().map(|value| value != &evidence.collation_connection).unwrap_or(false) {
            bail!("MYSQL_SESSION_MISMATCH: 实际collation与请求不一致");
        }
        Ok(evidence)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn defaults_and_builder_are_not_session_evidence() { let value = parse("").unwrap(); let options = value.apply(MySqlConnectOptions::new()); assert_eq!(options.get_host(), "localhost"); assert!(value.diagnostics.is_empty()); }
    #[test] fn charset_and_collation_are_accepted_as_configured_only() { let value = parse("charset=utf8mb4;collation=utf8mb4_bin").unwrap(); assert!(value.diagnostics.iter().all(|item| item.status == "configured")); }
    #[test] fn conflicting_aliases_are_rejected_without_values() { let e = parse("charset=utf8mb4;character_set=latin1").unwrap_err().to_string(); assert!(e.contains("MYSQL_OPTION_CONFLICT")); assert!(!e.contains("latin1")); }
    #[test] fn unknown_or_bare_legacy_options_stop_connection() { assert!(parse("whatever=value").is_err()); assert!(parse("charset=utf8mb4;whatever").is_err()); }
    #[test] fn empty_and_injectable_identifiers_are_rejected() { for params in ["charset=", "collation=", "charset=a-b", "charset='utf8mb4'", "charset=utf8mb4 --"] { assert!(parse(params).is_err()); } }
    #[test] fn default_charset_also_validates_collation() { assert!(parse("collation=latin1_bin").is_err()); assert!(parse("charset=utf8;collation=utf8mb3_bin").is_ok()); }
    #[test] fn same_alias_and_key_case_are_deterministic() { assert_eq!(parse("CHARSET=utf8mb4;character_set=utf8mb4").unwrap().charset.as_deref(), Some("utf8mb4")); }
    #[test] fn duplicate_json_is_not_last_wins() { assert!(parse("{\"charset\":\"utf8mb4\",\"charset\":\"latin1\"}").is_err()); }
    #[test] fn delegated_tls_and_keepalive_never_claim_applied() { assert!(parse("sslmode=verify-full;ssl_ca=x;keepalive_interval=30").unwrap().diagnostics.is_empty()); }
    #[test] fn fingerprint_is_full_and_changes_with_effective_options() { assert_eq!(parse("").unwrap().fingerprint().len(), 64); assert_ne!(parse("").unwrap().fingerprint(), parse("charset=latin1").unwrap().fingerprint()); }
    #[cfg(windows)] #[test] fn windows_socket_does_not_fall_back_to_tcp() { assert!(parse("socket=C:/tmp/mysql.sock").unwrap_err().to_string().contains("MYSQL_SOCKET_UNSUPPORTED")); }
}
