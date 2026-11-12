//! 过程密钥认证的一次性 grant。单调消费窗口在退休后仍拒绝重放，不存无限 ID tombstone。
use std::collections::BTreeSet;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Serialize, Deserialize};
use sha2::Sha256;
use crate::{error::{AppError, CmdResult}, execution::Binding, execution_contracts::ExecutionGrant};
const TTL: u64 = 60_000;
const WINDOW: u64 = 2048;
#[derive(Serialize, Deserialize)]
struct Claim { id: String, sequence: u64, issued: u64, expires: u64, binding: Binding, scope: String }
pub(crate) struct Grants { key: [u8;32], issued: u64, floor: u64, spent: BTreeSet<u64> }
impl Default for Grants {
    fn default() -> Self { let mut key=[0;32]; rand::thread_rng().fill_bytes(&mut key); Self { key, issued:0, floor:0, spent:Default::default() } }
}
impl Grants {
    pub fn issue(&mut self, binding: Binding, scope: &str, now: u64) -> CmdResult<ExecutionGrant> {
        self.issued=self.issued.checked_add(1).ok_or_else(|| AppError::coded("EXECUTION_GRANT_EXHAUSTED", "过程序号耗尽"))?;
        let claim=Claim { id:uuid::Uuid::new_v4().to_string(), sequence:self.issued, issued:now, expires:now.saturating_add(TTL), binding, scope: scope.into() };
        let bytes=serde_json::to_vec(&claim).expect("grant serialization");
        let mut mac=Hmac::<Sha256>::new_from_slice(&self.key).expect("key"); mac.update(&bytes);
        Ok(ExecutionGrant { execution_id:claim.id, token:format!("{}.{}", URL_SAFE_NO_PAD.encode(&bytes), URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())), expires_in_ms:TTL })
    }
    pub fn consume(&mut self, id: &str, token: Option<&str>, binding: &Binding, scope: &str, now: u64) -> CmdResult<()> {
        let denied=|| AppError::coded("EXECUTION_GRANT_INVALID", "执行授权无效、过期或已撤销；没有发送 SQL");
        let token=token.ok_or_else(denied)?;
        if token.len()>4096 { return Err(denied()); }
        let (body,signature)=token.split_once('.').ok_or_else(denied)?;
        let bytes=URL_SAFE_NO_PAD.decode(body).map_err(|_| denied())?;
        let signature=URL_SAFE_NO_PAD.decode(signature).map_err(|_| denied())?;
        let mut mac=Hmac::<Sha256>::new_from_slice(&self.key).expect("key"); mac.update(&bytes); mac.verify_slice(&signature).map_err(|_| denied())?;
        let claim:Claim=serde_json::from_slice(&bytes).map_err(|_| denied())?;
        if claim.id!=id || &claim.binding!=binding || claim.scope!=scope || now<claim.issued || now>=claim.expires || claim.expires!=claim.issued.saturating_add(TTL) || claim.sequence>self.issued || claim.sequence<=self.floor { return Err(denied()); }
        if !self.spent.insert(claim.sequence) { return Err(AppError::coded("EXECUTION_ID_DUPLICATE", "此执行授权已消费，禁止重放")); }
        self.floor=self.floor.max(claim.sequence.saturating_sub(WINDOW));
        self.spent.retain(|seq| *seq>self.floor);
        Ok(())
    }
}
