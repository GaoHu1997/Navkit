//! Explicit import/export producer admission; not a process RSS limit.
//! Legacy task logs, sync coordinators, dump children and recipe's separate budget
//! are deliberately NOT automatically classified as producers by task id.
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};

/// Starting benchmark candidate, not an empirically optimal worker count.
pub const DEFAULT_MAX_PRODUCERS: usize = 4;
const MIB: u64 = 1024 * 1024;

/// A finite reservation envelope for one bounded import/export pipeline.
/// Weight = 3 record copies + a worst-case 6x escaped/encoded record +
/// (queue + current batch) * (batch target + one oversized record) + 1 MiB
/// fixed buffers. Defaults: record 16 MiB, batch 4 MiB, queue 2 => 205 MiB.
/// This is an admission ledger, NOT measured/allocator-enforced memory: driver
/// buffers, metadata/Vec overhead, database server memory and unbounded legacy
/// paths remain outside it. Callers must use the actual normalized limits and
/// must NOT launch additional independent producers inside a nested membership.
#[derive(Debug, Clone, Copy)]
pub struct TransferAdmissionProfile { pub reserved_bytes: u64 }
impl TransferAdmissionProfile {
    pub fn bounded(record: u64, batch: u64, queue: u32) -> Result<Self, AdmissionError> {
        if record == 0 || batch == 0 || queue == 0 { return Err(AdmissionError::InvalidBudget); }
        let bytes = record.checked_mul(9)
            .and_then(|copies| batch.checked_add(record)
                .and_then(|block| block.checked_mul(u64::from(queue) + 1))
                .and_then(|blocks| copies.checked_add(blocks)))
            .and_then(|bytes| bytes.checked_add(MIB))
            .ok_or(AdmissionError::InvalidBudget)?;
        Ok(Self { reserved_bytes: bytes })
    }
}
impl Default for TransferAdmissionProfile {
    fn default() -> Self { Self::bounded(16 * MIB, 4 * MIB, 2).expect("finite default transfer profile") }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionError { InvalidBudget, DuplicateId, Capacity, MissingParent, ForeignController }
impl fmt::Display for AdmissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidBudget => "TRANSFER_ADMISSION_INVALID_BUDGET: 预留字节必须为正且不溢出",
            Self::DuplicateId => "TRANSFER_ADMISSION_DUPLICATE_ID: 任务已持有准入配额",
            Self::Capacity => "TRANSFER_ADMISSION_BUSY: 传输生产者或预留字节达到限额，请在现有任务结束后重试（未排队）",
            Self::MissingParent => "TRANSFER_ADMISSION_PARENT_FINISHED: 父任务已结束或不持有准入配额",
            Self::ForeignController => "TRANSFER_ADMISSION_FOREIGN_PARENT: 父任务属于其他准入控制器",
        })
    }
}
impl std::error::Error for AdmissionError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionUsage { pub producers: usize, pub reserved_bytes: u64, pub members: usize }
struct Member { token: u64, root: u64 }
struct Root { bytes: u64, members: usize }
#[derive(Default)]
struct State { next_token: u64, bytes: u64, members: HashMap<String, Member>, roots: HashMap<u64, Root> }

/// Isolated instances make unit tests independent of the process-global ledger.
pub struct AdmissionController { max_producers: usize, max_bytes: u64, state: Mutex<State> }
impl AdmissionController {
    pub fn new(max_producers: usize, max_bytes: u64) -> Arc<Self> {
        Arc::new(Self { max_producers, max_bytes, state: Mutex::new(State::default()) })
    }
    /// Immediate rejection: no waiting future, database lock or uncancellable queue.
    pub fn try_begin(self: &Arc<Self>, id: &str, bytes: u64) -> Result<AdmissionPermit, AdmissionError> {
        if bytes == 0 { return Err(AdmissionError::InvalidBudget); }
        let mut state = self.state.lock().expect("transfer admission lock poisoned; fail closed");
        if state.members.contains_key(id) { return Err(AdmissionError::DuplicateId); }
        let total = state.bytes.checked_add(bytes).ok_or(AdmissionError::Capacity)?;
        if state.roots.len() >= self.max_producers || total > self.max_bytes { return Err(AdmissionError::Capacity); }
        let token = state.next_token.checked_add(1).ok_or(AdmissionError::Capacity)?;
        state.next_token = token; state.bytes = total;
        state.roots.insert(token, Root { bytes, members: 1 });
        state.members.insert(id.to_owned(), Member { token, root: token });
        Ok(AdmissionPermit { controller: self.clone(), id: id.to_owned(), token })
    }
    /// Same producer's serial sub-operation/cursor: reuse its root reservation.
    /// Parent may finish first, but bytes stay reserved until ALL members settle.
    /// This is not permission to start another independent producer for free.
    pub fn try_nested(self: &Arc<Self>, id: &str, parent: &AdmissionPermit) -> Result<AdmissionPermit, AdmissionError> {
        if !Arc::ptr_eq(self, &parent.controller) { return Err(AdmissionError::ForeignController); }
        let mut state = self.state.lock().expect("transfer admission lock poisoned; fail closed");
        if state.members.contains_key(id) { return Err(AdmissionError::DuplicateId); }
        let root = state.members.get(&parent.id).filter(|member| member.token == parent.token)
            .map(|member| member.root).ok_or(AdmissionError::MissingParent)?;
        let token = state.next_token.checked_add(1).ok_or(AdmissionError::Capacity)?;
        let count = state.roots.get(&root).expect("root must remain until last member finishes").members
            .checked_add(1).ok_or(AdmissionError::Capacity)?;
        state.next_token = token;
        state.roots.get_mut(&root).unwrap().members = count;
        state.members.insert(id.to_owned(), Member { token, root });
        Ok(AdmissionPermit { controller: self.clone(), id: id.to_owned(), token })
    }
    pub fn usage(&self) -> AdmissionUsage {
        let state = self.state.lock().expect("transfer admission lock poisoned; fail closed");
        AdmissionUsage { producers: state.roots.len(), reserved_bytes: state.bytes, members: state.members.len() }
    }
}

/// Clones share task ownership, not another producer. Deliberately NO releasing
/// Drop: dropping a future/handle says nothing about a detached worker settling.
/// Only finish AFTER worker join/drain/cleanup releases a member. Missing finish
/// fails closed (reservation survives in the controller, including global one).
#[derive(Clone)]
pub struct AdmissionPermit { controller: Arc<AdmissionController>, id: String, token: u64 }
impl AdmissionPermit {
    pub fn nested(&self, id: &str) -> Result<Self, AdmissionError> { self.controller.try_nested(id, self) }
    /// Returns false for repeated/stale finish, never releases a reused task id.
    pub fn finish(&self) -> bool {
        let mut state = self.controller.state.lock().expect("transfer admission lock poisoned; fail closed");
        let Some(root) = state.members.get(&self.id).filter(|member| member.token == self.token).map(|member| member.root) else { return false; };
        state.members.remove(&self.id);
        let reservation = state.roots.get_mut(&root).expect("live member requires root reservation");
        reservation.members -= 1;
        if reservation.members == 0 {
            let reservation = state.roots.remove(&root).unwrap();
            state.bytes -= reservation.bytes;
        }
        true
    }
}

pub fn global() -> &'static Arc<AdmissionController> {
    static GLOBAL: OnceLock<Arc<AdmissionController>> = OnceLock::new();
    GLOBAL.get_or_init(|| AdmissionController::new(DEFAULT_MAX_PRODUCERS,
        TransferAdmissionProfile::default().reserved_bytes * DEFAULT_MAX_PRODUCERS as u64))
}

#[cfg(test)]
#[path = "transfer_admission_tests.rs"]
mod tests;
