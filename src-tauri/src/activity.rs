//! 数据库操作全程持有读门禁；配置应用尝试独占，绝不靠丢弃 Arc 取消任务。
use std::sync::Arc;

use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

use crate::error::{AppError, CmdResult};

#[derive(Default)]
pub struct ActivityGate(Arc<RwLock<()>>);

impl ActivityGate {
    pub fn enter(&self) -> CmdResult<OwnedRwLockReadGuard<()>> {
        self.0
            .clone()
            .try_read_owned()
            .map_err(|_| AppError::new("数据源配置正在应用，请稍后重试此操作"))
    }

    pub fn exclusive(&self) -> CmdResult<OwnedRwLockWriteGuard<()>> {
        self.0
            .clone()
            .try_write_owned()
            .map_err(|_| AppError::new("数据库操作仍在运行，请结束操作后重新提交数据源配置"))
    }
}

#[cfg(test)]
mod tests {
    use super::ActivityGate;

    #[test]
    fn mutation_lease_defers_sync_until_it_is_released() {
        let gate = ActivityGate::default();
        let mutation = gate.enter().unwrap();
        assert!(gate.exclusive().is_err());
        drop(mutation);
        assert!(gate.exclusive().is_ok());
    }

    #[test]
    fn sync_commit_rejects_new_driver_or_database_activity() {
        let gate = ActivityGate::default();
        let commit = gate.exclusive().unwrap();
        assert!(gate.enter().is_err());
        assert!(gate.exclusive().is_err());
        drop(commit);
        assert!(gate.enter().is_ok());
    }
}
