//! Strict, lossless transport option parsing. This module never silently drops a token.
use anyhow::{bail, Result};
use serde::de::{self, MapAccess, Visitor};
use serde::Deserialize;
use std::fmt;

pub const MAX_PARAM_BYTES: usize = 128 * 1024;
pub const MAX_PARAM_PAIRS: usize = 512;

struct JsonPairs(Vec<(String, String)>);
impl<'de> Deserialize<'de> for JsonPairs {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct ObjectVisitor;
        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = JsonPairs;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result { formatter.write_str("a connection option object") }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> std::result::Result<JsonPairs, M::Error> {
                let mut pairs: Vec<(String, String)> = Vec::new();
                while let Some(key) = map.next_key::<String>()? {
                    if pairs.len() >= MAX_PARAM_PAIRS { return Err(de::Error::custom("too many options")); }
                    let canonical = key.to_ascii_lowercase();
                    if pairs.iter().any(|(old, _)| old == &canonical) { return Err(de::Error::custom("duplicate JSON option")); }
                    let value: Box<serde_json::value::RawValue> = map.next_value()?;
                    let raw = value.get();
                    let value = if raw.starts_with('"') { serde_json::from_str::<String>(raw).map_err(de::Error::custom)? } else { raw.to_owned() };
                    pairs.push((canonical, value));
                }
                Ok(JsonPairs(pairs))
            }
        }
        deserializer.deserialize_map(ObjectVisitor)
    }
}

pub fn parse(params: &str) -> Result<Vec<(String, String)>> {
    if params.len() > MAX_PARAM_BYTES { bail!("CONNECTION_PARAM_LIMIT: 附加参数超过128KiB限制"); }
    let raw = params.trim();
    let pairs = if raw.is_empty() { Vec::new() }
    else if raw.starts_with('{') {
        serde_json::from_str::<JsonPairs>(raw).map_err(|_| anyhow::anyhow!("CONNECTION_PARAM_INVALID: JSON附加参数无效或有重复键"))?.0
    } else {
        if raw.starts_with('[') || raw.starts_with('@') { bail!("CONNECTION_PARAM_INVALID: 此入口不接受驱动模板或数组参数"); }
        let mut pairs = Vec::new();
        for part in raw.split([';', '&']) {
            let part = part.trim();
            if part.is_empty() { continue; }
            let (key, value) = part.split_once('=').ok_or_else(|| anyhow::anyhow!("CONNECTION_PARAM_INVALID: 每个附加参数必须是key=value"))?;
            pairs.push((key.trim().to_ascii_lowercase(), value.trim().to_owned()));
            if pairs.len() > MAX_PARAM_PAIRS { bail!("CONNECTION_PARAM_LIMIT: 附加参数数量超过512项"); }
        }
        pairs
    };
    for (key, value) in &pairs {
        if key.is_empty() || key.len() > 128 || !key.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')) {
            bail!("CONNECTION_PARAM_INVALID: 附加参数名格式无效");
        }
        if value.chars().any(char::is_control) { bail!("CONNECTION_PARAM_INVALID: 参数{}含控制字符", key); }
    }
    Ok(pairs)
}

pub fn tls_key(key: &str) -> Option<&'static str> {
    match key.to_ascii_lowercase().as_str() {
        "ssl" | "tls" | "sslmode" | "ssl-mode" | "ssl_mode" | "tls-mode" | "tls_mode" => Some("mode"),
        "ssl-ca" | "ssl_ca" | "ssl-root-cert" | "ssl_root_cert" | "root-cert" | "root_cert" | "sslrootcert" | "cacert" | "ca" | "cafile" => Some("ca"),
        "ssl-cert" | "ssl_cert" | "client-cert" | "client_cert" | "cert" => Some("cert"),
        "ssl-key" | "ssl_key" | "client-key" | "client_key" | "key" => Some("key"),
        "ssl_insecure" | "tls_insecure" | "insecure" | "trust-server-certificate" => Some("insecure"),
        "tls_server_name" | "ssl_server_name" | "server_name" => Some("server_name"),
        _ => None,
    }
}

pub fn bool_value(value: &str, key: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => bail!("CONNECTION_PARAM_INVALID: 参数{}必须是明确的布尔值", key),
    }
}

pub fn keepalive_key(key: &str) -> bool {
    matches!(key, "keepalive" | "keep_alive" | "keepalive_interval" | "keepalive_interval_secs" | "keepalive_timeout" | "keepalive_timeout_secs" | "keepalive_failures")
}

/// Delimited output is only for validated nonsecret values. JSON shape is preserved.
pub fn filter(params: &str, retain: impl Fn(&str) -> bool) -> Result<String> {
    let pairs = parse(params)?;
    if params.trim().starts_with('{') {
        let raw: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> = serde_json::from_str(params).map_err(|_| anyhow::anyhow!("CONNECTION_PARAM_INVALID"))?;
        let mut map = std::collections::BTreeMap::new();
        for (key, _value) in pairs.into_iter().filter(|(key, _)| retain(key)) {
            let item = raw.iter().find(|(name, _)| name.eq_ignore_ascii_case(&key)).map(|(_, value)| value.clone()).ok_or_else(|| anyhow::anyhow!("CONNECTION_PARAM_INVALID"))?;
            map.insert(key, item);
        }
        Ok(serde_json::to_string(&map)?)
    } else {
        Ok(pairs.into_iter().filter(|(key, _)| retain(key)).map(|(key, value)| format!("{key}={value}")).collect::<Vec<_>>().join(";"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn no_bare_token_is_lost() { assert!(parse("charset=utf8mb4;broken").is_err()); }
    #[test] fn malformed_json_never_falls_back() { assert!(parse("{\"tls\": true,").is_err()); }
    #[test] fn duplicate_json_keys_and_case_alias_are_rejected() { assert!(parse("{\"tls\":true,\"TLS\":false}").is_err()); }
    #[test] fn exact_json_numbers_remain_text() { assert_eq!(parse("{\"n\":18446744073709551615}").unwrap()[0].1, "18446744073709551615"); }
    #[test] fn nul_and_control_are_rejected_without_value_echo() { let e = parse("charset=secret\0password").unwrap_err().to_string(); assert!(!e.contains("secret")); }
    #[test] fn oversized_and_excessive_pairs_are_rejected() { assert!(parse(&"x".repeat(MAX_PARAM_BYTES + 1)).is_err()); assert!(parse(&vec!["x=1"; MAX_PARAM_PAIRS + 1].join(";")).is_err()); }
    #[test] fn filtering_preserves_json_and_non_tls_fields() { assert_eq!(filter("{\"ssl\":true,\"port\":9092}", |key| tls_key(key).is_none()).unwrap(), "{\"port\":9092}"); }
    #[test] fn filter_never_rounds_json_numbers_or_retypes_strings() { let input = r#"{"ssh_host":"h","n":1844674407370955161512345,"s":"18446744073709551615","nested":{"n":1844674407370955161512345}}"#; let output = filter(input, |key| !key.starts_with("ssh_")).unwrap(); let exact: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> = serde_json::from_str(&output).unwrap(); assert_eq!(exact["n"].get(), "1844674407370955161512345"); assert_eq!(exact["s"].get(), "\"18446744073709551615\""); assert_eq!(exact["nested"].get(), r#"{"n":1844674407370955161512345}"#); }
    #[test] fn booleans_are_not_truthy_typos() { assert!(bool_value("tru", "tls").is_err()); assert!(!bool_value("false", "tls").unwrap()); }
}
