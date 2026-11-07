//! 同步包专用信封。远端只能得到密文；严格限制 KDF 与文件大小。
use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use pbkdf2::pbkdf2_hmac;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::merge::Snapshot;

pub(super) const MAX_REMOTE_BYTES: usize = 8 * 1024 * 1024;
const MAX_PLAIN_BYTES: usize = 4 * 1024 * 1024;
const FORMAT: &str = "navkit-datasource-sync";
const ITERATIONS: u32 = 600_000;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    format: String,
    version: u32,
    kdf: String,
    iterations: u32,
    salt: String,
    nonce: String,
    data: String,
}

impl Envelope {
    fn aad(&self) -> Vec<u8> {
        format!(
            "{}/{}/{}/{}/{}/{}",
            self.format, self.version, self.kdf, self.iterations, self.salt, self.nonce
        )
        .into_bytes()
    }
}

fn inspect(bytes: &[u8]) -> Result<Envelope, String> {
    if bytes.len() > MAX_REMOTE_BYTES {
        return Err("远端同步文件超过 8 MiB，已拒绝读取".into());
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let envelope: Envelope = serde_json::from_slice(bytes)
        .map_err(|_| "远端不是完整的 NavKit 数据源同步信封".to_string())?;
    if envelope.format != FORMAT || envelope.version != 1 || envelope.kdf != "pbkdf2-sha256" {
        return Err("远端同步文件格式或版本不受支持，已拒绝覆盖".into());
    }
    // 固定版本只接受固定成本，不能由不可信远端指定廉价 KDF 或无限 CPU 消耗。
    if envelope.iterations != ITERATIONS {
        return Err("远端同步文件的 KDF 参数无效".into());
    }
    if envelope.salt.len() != 24 || envelope.nonce.len() != 16 {
        return Err("远端同步文件的 salt 或 nonce 长度无效".into());
    }
    if B64
        .decode(&envelope.salt)
        .map_err(|_| "同步 salt 损坏")?
        .len()
        != 16
        || B64
            .decode(&envelope.nonce)
            .map_err(|_| "同步 nonce 损坏")?
            .len()
            != 12
    {
        return Err("远端同步文件的 salt 或 nonce 无效".into());
    }
    let data = B64.decode(&envelope.data).map_err(|_| "同步密文损坏")?;
    if data.len() <= 16 || data.len() > MAX_PLAIN_BYTES + 16 {
        return Err("同步密文为空、被截断或过大".into());
    }
    Ok(envelope)
}

/// 连接测试无需同步口令，但仍拒绝 HTML、非本应用文件或损坏的信封。
pub(super) fn inspect_envelope(bytes: &[u8]) -> Result<(), String> {
    inspect(bytes).map(|_| ())
}

pub(super) fn seal(snapshot: &Snapshot, passphrase: &str) -> Result<Vec<u8>, String> {
    if passphrase.trim().chars().count() < 8 {
        return Err("同步加密需要至少 8 个字符的独立强口令".into());
    }
    snapshot.validate()?;
    let plain = Zeroizing::new(serde_json::to_vec(snapshot).map_err(|_| "序列化同步数据源失败")?);
    if plain.len() > MAX_PLAIN_BYTES {
        return Err("数据源同步包超过 4 MiB".into());
    }
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut salt);
    rand::thread_rng().fill_bytes(&mut nonce);
    let mut envelope = Envelope {
        format: FORMAT.into(),
        version: 1,
        kdf: "pbkdf2-sha256".into(),
        iterations: ITERATIONS,
        salt: B64.encode(salt),
        nonce: B64.encode(nonce),
        data: String::new(),
    };
    let mut key = Zeroizing::new([0u8; 32]);
    pbkdf2_hmac::<Sha256>(passphrase.as_bytes(), &salt, ITERATIONS, key.as_mut());
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| "同步加密初始化失败")?;
    let sealed = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &plain,
                aad: &envelope.aad(),
            },
        )
        .map_err(|_| "同步加密失败")?;
    envelope.data = B64.encode(sealed);
    serde_json::to_vec(&envelope).map_err(|_| "编码同步信封失败".into())
}

pub(super) fn open(bytes: &[u8], passphrase: &str) -> Result<Snapshot, String> {
    let envelope = inspect(bytes)?;
    let salt = B64.decode(&envelope.salt).map_err(|_| "同步 salt 损坏")?;
    let nonce = B64.decode(&envelope.nonce).map_err(|_| "同步 nonce 损坏")?;
    let data = B64.decode(&envelope.data).map_err(|_| "同步密文损坏")?;
    let mut key = Zeroizing::new([0u8; 32]);
    pbkdf2_hmac::<Sha256>(passphrase.as_bytes(), &salt, ITERATIONS, key.as_mut());
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| "同步解密初始化失败")?;
    let plain = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &data,
                    aad: &envelope.aad(),
                },
            )
            .map_err(|_| "同步口令不正确或快照已被篡改，已拒绝同步".to_string())?,
    );
    let snapshot: Snapshot =
        serde_json::from_slice(&plain).map_err(|_| "解密后的数据源快照损坏，已拒绝同步")?;
    snapshot.validate()?;
    Ok(snapshot)
}

pub(super) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
