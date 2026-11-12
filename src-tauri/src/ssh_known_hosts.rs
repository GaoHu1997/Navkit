//! OpenSSH known_hosts verification with explicit pins; no TOFU and no substring matching.
use anyhow::{bail, Result};
use base64::Engine;
use hmac::{Hmac, Mac};
use russh::keys::ssh_key::{HashAlg, PublicKey};
use sha1::Sha1;
use sha2::{Digest, Sha256};

#[derive(Clone)]
struct Entry { hosts: String, key: PublicKey, marker: Marker }
#[derive(Clone, Copy, PartialEq, Eq)]
enum Marker { Normal, Revoked, CertificateAuthority }
#[derive(Clone)]
pub struct KnownHosts { entries: Vec<Entry>, pub revision: String }
#[derive(Debug, Clone)]
pub struct KnownHostEvidence { pub key_fingerprint: String, pub file_revision: String }

pub fn canonical_host(host: &str, port: u16) -> Result<String> {
    if host.is_empty() || host.len() > 1024 || port == 0 || host.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '/' | '\\' | '@' | '*' | '?' | ',' | '!' | '|')) { bail!("SSH_HOST_INVALID: SSH主机或端口格式无效"); }
    let host = host.strip_prefix('[').and_then(|value| value.strip_suffix(']')).unwrap_or(host).to_ascii_lowercase();
    Ok(if port == 22 { host } else { format!("[{host}]:{port}") })
}
fn wildcard(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes(); let value = value.as_bytes();
    let (mut p, mut v, mut star, mut matched) = (0, 0, None, 0);
    while v < value.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p].eq_ignore_ascii_case(&value[v])) { p += 1; v += 1; }
        else if p < pattern.len() && pattern[p] == b'*' { star = Some(p); p += 1; matched = v; }
        else if let Some(position) = star { matched += 1; v = matched; p = position + 1; }
        else { return false; }
    }
    while p < pattern.len() && pattern[p] == b'*' { p += 1; }
    p == pattern.len()
}
fn hashed_parts(pattern: &str) -> Result<(Vec<u8>, Vec<u8>)> {
    let pieces: Vec<_> = pattern.split('|').collect();
    if pieces.len() != 4 || !pieces[0].is_empty() || pieces[1] != "1" { bail!("SSH_KNOWN_HOSTS_INVALID: 不支持或无效的hashed-host格式"); }
    let salt = base64::engine::general_purpose::STANDARD.decode(pieces[2]).map_err(|_| anyhow::anyhow!("SSH_KNOWN_HOSTS_INVALID: hashed-host盐无效"))?;
    let digest = base64::engine::general_purpose::STANDARD.decode(pieces[3]).map_err(|_| anyhow::anyhow!("SSH_KNOWN_HOSTS_INVALID: hashed-host摘要无效"))?;
    if salt.len() != 20 || digest.len() != 20 { bail!("SSH_KNOWN_HOSTS_INVALID: hashed-host盐和SHA1摘要必须各20字节"); }
    Ok((salt, digest))
}
fn hosts_match(patterns: &str, host: &str) -> Result<bool> {
    if patterns.starts_with('|') {
        let (salt, digest) = hashed_parts(patterns)?;
        let mut mac = Hmac::<Sha1>::new_from_slice(&salt).map_err(|_| anyhow::anyhow!("SSH_KNOWN_HOSTS_INVALID"))?;
        mac.update(host.as_bytes());
        return Ok(mac.verify_slice(&digest).is_ok());
    }
    let mut positive = false;
    for pattern in patterns.split(',') {
        let (negated, pattern) = pattern.strip_prefix('!').map(|value| (true, value)).unwrap_or((false, pattern));
        if wildcard(pattern, host) { if negated { return Ok(false); } positive = true; }
    }
    Ok(positive)
}
impl KnownHosts {
    pub fn load(path: &str) -> Result<Self> {
        let bytes = crate::tls_options::material::read_file(path).map_err(|_| anyhow::anyhow!("SSH_KNOWN_HOSTS_REQUIRED: 必须提供可读的known_hosts普通文件；不会自动信任或写入新主机"))?;
        let text = std::str::from_utf8(&bytes).map_err(|_| anyhow::anyhow!("SSH_KNOWN_HOSTS_INVALID: known_hosts不是UTF8文本"))?;
        Self::parse(text)
    }
    pub fn parse(text: &str) -> Result<Self> {
        if text.len() > crate::tls_options::material::MAX_MATERIAL_BYTES { bail!("SSH_KNOWN_HOSTS_LIMIT: known_hosts超过4MiB限制"); }
        let mut entries = Vec::new();
        for (number, line) in text.lines().enumerate() {
            let line = line.trim(); if line.is_empty() || line.starts_with('#') { continue; }
            if line.len() > 65536 || line.chars().any(|c| c.is_control() && c != '\t') { bail!("SSH_KNOWN_HOSTS_INVALID: known_hosts第{}行无效", number + 1); }
            let fields: Vec<_> = line.split_whitespace().collect();
            let (marker, start) = match fields.first().copied() {
                Some("@revoked") => (Marker::Revoked, 1), Some("@cert-authority") => (Marker::CertificateAuthority, 1),
                Some(value) if value.starts_with('@') => bail!("SSH_KNOWN_HOSTS_INVALID: known_hosts第{}行marker不支持", number + 1),
                _ => (Marker::Normal, 0),
            };
            if fields.len() < start + 3 { bail!("SSH_KNOWN_HOSTS_INVALID: known_hosts第{}行字段不足", number + 1); }
            let hosts = fields[start];
            if hosts.starts_with('|') { hashed_parts(hosts)?; }
            else if hosts.split(',').any(|pattern| pattern.is_empty() || pattern == "!" || pattern.contains('|')) { bail!("SSH_KNOWN_HOSTS_INVALID: 主机模式无效"); }
            let algorithm = fields[start + 1];
            let key = PublicKey::from_openssh(&format!("{} {}", algorithm, fields[start + 2])).map_err(|_| anyhow::anyhow!("SSH_KNOWN_HOSTS_INVALID: known_hosts第{}行公钥wire或算法无效", number + 1))?;
            if key.algorithm().as_str() != algorithm { bail!("SSH_KNOWN_HOSTS_INVALID: 公钥算法与wire声明不一致"); }
            entries.push(Entry { hosts: hosts.into(), key, marker });
            if entries.len() > 16384 { bail!("SSH_KNOWN_HOSTS_LIMIT: known_hosts超过16384条记录"); }
        }
        if entries.is_empty() { bail!("SSH_KNOWN_HOSTS_REQUIRED: known_hosts没有可用主机公钥，不能自动TOFU"); }
        Ok(Self { entries, revision: format!("{:x}", Sha256::digest(text.as_bytes())) })
    }
    pub fn verify(&self, host: &str, port: u16, key: &PublicKey) -> Result<KnownHostEvidence> {
        let host = canonical_host(host, port)?;
        let (mut same_algorithm, mut exact) = (false, false);
        for entry in &self.entries {
            if !hosts_match(&entry.hosts, &host)? { continue; }
            let same = entry.key.key_data() == key.key_data();
            if entry.marker == Marker::Revoked && same { bail!("SSH_HOST_KEY_REVOKED: 当前SSH主机公钥已明确撤销"); }
            if entry.marker != Marker::Normal { continue; }
            if entry.key.algorithm() == key.algorithm() { same_algorithm = true; }
            if same { exact = true; }
        }
        if !exact {
            if same_algorithm { bail!("SSH_HOST_KEY_CHANGED: SSH主机公钥已改变，认证与转发已拒绝"); }
            bail!("SSH_HOST_KEY_UNKNOWN: SSH主机/端口/算法没有显式公钥pin，认证与转发已拒绝");
        }
        Ok(KnownHostEvidence { key_fingerprint: key.fingerprint(HashAlg::Sha256).to_string(), file_revision: self.revision.clone() })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn key(seed: u8) -> (PublicKey, String) {
        let mut wire = Vec::new(); wire.extend_from_slice(&11_u32.to_be_bytes()); wire.extend_from_slice(b"ssh-ed25519"); wire.extend_from_slice(&32_u32.to_be_bytes()); wire.extend_from_slice(&[seed; 32]);
        let line = format!("ssh-ed25519 {}", base64::engine::general_purpose::STANDARD.encode(wire));
        (PublicKey::from_openssh(&line).unwrap(), line)
    }
    fn hash(host: &str) -> String {
        let salt = [7; 20]; let mut mac = Hmac::<Sha1>::new_from_slice(&salt).unwrap(); mac.update(host.as_bytes());
        format!("|1|{}|{}", base64::engine::general_purpose::STANDARD.encode(salt), base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
    }
    #[test] fn no_file_or_empty_records_never_tofu() { assert!(KnownHosts::load("missing-private-known-hosts-file").is_err()); assert!(KnownHosts::parse("# comment\n").is_err()); }
    #[test] fn tabs_comments_and_exact_public_key_work() { let (public, text) = key(1); let file = KnownHosts::parse(&format!("example\t{text} comment\n")).unwrap(); assert!(file.verify("EXAMPLE", 22, &public).is_ok()); }
    #[test] fn unknown_and_changed_keys_fail_before_authentication() { let (public, text) = key(1); let file = KnownHosts::parse(&format!("example {text}")).unwrap(); assert!(file.verify("other", 22, &public).unwrap_err().to_string().contains("UNKNOWN")); assert!(file.verify("example", 22, &key(2).0).unwrap_err().to_string().contains("CHANGED")); }
    #[test] fn explicit_multiple_rotation_entries_do_not_early_reject() { let (public, text) = key(2); let file = KnownHosts::parse(&format!("example {}\nexample {text}", key(1).1)).unwrap(); assert!(file.verify("example", 22, &public).is_ok()); }
    #[test] fn nondefault_ports_are_distinct_identities() { let (public, text) = key(1); let file = KnownHosts::parse(&format!("[example]:2222 {text}")).unwrap(); assert!(file.verify("example", 2222, &public).is_ok()); assert!(file.verify("example", 22, &public).is_err()); }
    #[test] fn hash_covers_canonical_port_and_host() { let (public, text) = key(1); let file = KnownHosts::parse(&format!("{} {text}", hash("[example]:2222"))).unwrap(); assert!(file.verify("example", 2222, &public).is_ok()); assert!(file.verify("other", 2222, &public).is_err()); }
    #[test] fn negation_scopes_wildcards() { let (public, text) = key(1); let file = KnownHosts::parse(&format!("*.example,!admin.example {text}")).unwrap(); assert!(file.verify("db.example", 22, &public).is_ok()); assert!(file.verify("admin.example", 22, &public).is_err()); }
    #[test] fn revoked_pin_wins_even_if_normal_pin_is_present() { let (public, text) = key(1); let file = KnownHosts::parse(&format!("example {text}\n@revoked example {text}")).unwrap(); assert!(file.verify("example", 22, &public).unwrap_err().to_string().contains("REVOKED")); }
    #[test] fn authority_key_is_not_a_plain_host_pin() { let (public, text) = key(1); let file = KnownHosts::parse(&format!("@cert-authority example {text}")).unwrap(); assert!(file.verify("example", 22, &public).is_err()); }
    #[test] fn algorithm_wire_mismatch_is_invalid() { assert!(KnownHosts::parse(&format!("example {}", key(1).1.replace("ssh-ed25519 ", "ssh-rsa "))).is_err()); }
    #[test] fn malformed_hash_and_marker_are_not_ignored() { assert!(KnownHosts::parse(&format!("|1|YQ==|YQ== {}", key(1).1)).is_err()); assert!(KnownHosts::parse(&format!("@other example {}", key(1).1)).is_err()); }
    #[test] fn public_revision_and_fingerprint_are_stable() { let (public, text) = key(1); let file = KnownHosts::parse(&format!("example {text}")).unwrap(); assert_eq!(file.revision.len(), 64); assert!(file.verify("example", 22, &public).unwrap().key_fingerprint.starts_with("SHA256:")); }
}
