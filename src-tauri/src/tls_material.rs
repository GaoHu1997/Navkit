//! Bounded immutable TLS material; no filename is reopened after validation.
use anyhow::{bail, Result};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::io::{BufReader, Cursor, Read};
use std::path::Path;
use zeroize::Zeroizing;

pub const MAX_MATERIAL_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_CERTIFICATES: usize = 128;
#[derive(Clone, Default)]
pub struct TlsMaterial {
    pub ca: Option<Vec<u8>>,
    pub cert: Option<Vec<u8>>,
    pub key: Option<Zeroizing<Vec<u8>>>,
}
impl std::fmt::Debug for TlsMaterial {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("TlsMaterial").field("ca_present", &self.ca.is_some()).field("client_cert_present", &self.cert.is_some()).field("private_key_present", &self.key.is_some()).finish()
    }
}

pub fn read_file(path: &str) -> Result<Zeroizing<Vec<u8>>> {
    if path.is_empty() || path.len() > 32768 || path.chars().any(char::is_control) { bail!("TLS_PATH_INVALID: TLS材料路径无效"); }
    let path = Path::new(path);
    let metadata = std::fs::symlink_metadata(path).map_err(|_| anyhow::anyhow!("TLS_FILE_UNREADABLE: TLS材料文件不存在或不可读"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_MATERIAL_BYTES as u64 {
        bail!("TLS_FILE_INVALID: TLS材料必须是1字节至4MiB的普通文件，不接受符号链接");
    }
    let file = std::fs::File::open(path).map_err(|_| anyhow::anyhow!("TLS_FILE_UNREADABLE: TLS材料文件不可读"))?;
    let mut bytes = Zeroizing::new(Vec::new());
    file.take((MAX_MATERIAL_BYTES + 1) as u64).read_to_end(&mut bytes).map_err(|_| anyhow::anyhow!("TLS_FILE_UNREADABLE: TLS材料读取失败"))?;
    if bytes.is_empty() || bytes.len() > MAX_MATERIAL_BYTES { bail!("TLS_FILE_LIMIT: TLS材料读取超过4MiB限制"); }
    Ok(bytes)
}

pub fn certificates(bytes: &[u8]) -> Result<Vec<CertificateDer<'static>>> {
    if bytes.is_empty() || bytes.len() > MAX_MATERIAL_BYTES { bail!("TLS_CERT_INVALID: 证书材料为空或超限"); }
    let mut reader = BufReader::new(Cursor::new(bytes));
    let mut output = Vec::new();
    for item in rustls_pemfile::read_all(&mut reader) {
        match item.map_err(|_| anyhow::anyhow!("TLS_CERT_INVALID: 证书PEM无法解析"))? {
            rustls_pemfile::Item::X509Certificate(cert) => {
                let mut validator = rustls::RootCertStore::empty();
                validator.add(cert.clone()).map_err(|_| anyhow::anyhow!("TLS_CERT_INVALID: 证书DER不是有效X.509证书"))?;
                output.push(cert);
                if output.len() > MAX_CERTIFICATES { bail!("TLS_CERT_LIMIT: 证书链超过128张限制"); }
            }
            _ => bail!("TLS_CERT_INVALID: 证书文件包含非证书PEM材料"),
        }
    }
    if output.is_empty() { bail!("TLS_CERT_INVALID: 文件不含X.509证书"); }
    Ok(output)
}

pub fn private_key(bytes: &[u8]) -> Result<PrivateKeyDer<'static>> {
    if bytes.is_empty() || bytes.len() > MAX_MATERIAL_BYTES { bail!("TLS_KEY_INVALID: 私钥材料为空或超限"); }
    let mut reader = BufReader::new(Cursor::new(bytes));
    let mut key = None;
    for item in rustls_pemfile::read_all(&mut reader) {
        let item = item.map_err(|_| anyhow::anyhow!("TLS_KEY_INVALID: 私钥PEM无法解析；不支持加密PEM私钥"))?;
        let value = match item {
            rustls_pemfile::Item::Pkcs1Key(value) => PrivateKeyDer::Pkcs1(value),
            rustls_pemfile::Item::Pkcs8Key(value) => PrivateKeyDer::Pkcs8(value),
            rustls_pemfile::Item::Sec1Key(value) => PrivateKeyDer::Sec1(value),
            _ => bail!("TLS_KEY_INVALID: 私钥文件包含非私钥PEM材料"),
        };
        if key.replace(value).is_some() { bail!("TLS_KEY_INVALID: 私钥文件必须恰好包含一个私钥"); }
    }
    key.ok_or_else(|| anyhow::anyhow!("TLS_KEY_INVALID: 文件不含可用私钥；不支持加密PEM私钥"))
}

impl TlsMaterial {
    pub fn load(ca: Option<&str>, cert: Option<&str>, key: Option<&str>) -> Result<Self> {
        if cert.is_some() != key.is_some() { bail!("TLS_CLIENT_PAIR_REQUIRED: 客户端证书和私钥必须成对指定"); }
        let material = Self {
            ca: ca.map(read_file).transpose()?.map(|value| value.to_vec()),
            cert: cert.map(read_file).transpose()?.map(|value| value.to_vec()),
            key: key.map(read_file).transpose()?,
        };
        if let Some(bytes) = &material.ca { certificates(bytes)?; }
        if let (Some(cert), Some(key)) = (&material.cert, &material.key) {
            let certificate_chain = certificates(cert)?;
            let key = private_key(key)?;
            let certified = rustls::sign::CertifiedKey::from_der(certificate_chain, key, &rustls::crypto::ring::default_provider())
                .map_err(|_| anyhow::anyhow!("TLS_CLIENT_KEY_MISMATCH: 客户端证书私钥无效或不匹配"))?;
            certified.keys_match().map_err(|_| anyhow::anyhow!("TLS_CLIENT_KEY_MISMATCH: 无法证明客户端证书与私钥匹配"))?;
        }
        Ok(material)
    }
    pub fn public_fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        for value in [&self.ca, &self.cert] {
            match value { Some(value) => { digest.update([1]); digest.update((value.len() as u64).to_be_bytes()); digest.update(value); }, None => digest.update([0]) }
        }
        format!("{:x}", digest.finalize())
    }
    pub fn roots(&self) -> Result<rustls::RootCertStore> {
        let mut roots = rustls::RootCertStore::empty();
        if let Some(bytes) = &self.ca {
            for cert in certificates(bytes)? { roots.add(cert).map_err(|_| anyhow::anyhow!("TLS_CA_INVALID: CA证书无效"))?; }
        }
        Ok(roots)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn pem_delimiters_do_not_prove_valid_x509() { assert!(certificates(b"-----BEGIN CERTIFICATE-----\nZmFrZQ==\n-----END CERTIFICATE-----\n").is_err()); }
    #[test] fn empty_and_arbitrary_data_are_not_certificates() { for data in [b"".as_slice(), b"hello", b"-----BEGIN CERTIFICATE-----"] { assert!(certificates(data).is_err()); } }
    #[test] fn private_key_must_be_real_and_single() { assert!(private_key(b"-----BEGIN PRIVATE KEY-----\nZmFrZQ==\n-----END PRIVATE KEY-----\n").is_err() || rustls::crypto::ring::sign::any_supported_type(&private_key(b"-----BEGIN PRIVATE KEY-----\nZmFrZQ==\n-----END PRIVATE KEY-----\n").unwrap()).is_err()); }
    #[test] fn client_auth_is_not_optional_half_pair() { assert!(TlsMaterial::load(None, Some("missing"), None).is_err()); }
    #[test] fn secret_debug_is_redacted() { let material = TlsMaterial { key: Some(Zeroizing::new(b"secret-private-key".to_vec())), ..Default::default() }; assert!(!format!("{material:?}").contains("secret-private-key")); }
    #[test] fn missing_files_do_not_echo_private_paths() { let error = read_file("DO-NOT-ECHO-private-path.pem").unwrap_err().to_string(); assert!(!error.contains("DO-NOT-ECHO")); }
}
