//! PL-25 的独立调度模块。数据库动作只通过 SchedulerExecutor 回调进入现有 C1/工具服务。
//! 不注册系统任务、不启动第二数据库引擎；GUI 与 headless 必须共享同一 SchedulerStore。
#[path = "scheduler_contracts.rs"] mod contracts;
#[path = "scheduler_clock.rs"] mod clock;
#[path = "scheduler_store.rs"] mod store;
#[path = "scheduler_runner.rs"] mod runner;
#[path = "scheduler_service.rs"] mod service;
#[path = "scheduler_migration.rs"] mod migration;
#[path = "scheduler_offline.rs"] mod offline;
pub use contracts::*;
pub use clock::*;
pub use store::*;
pub use runner::*;
pub use service::*;
pub use offline::*;
pub use migration::{LegacyMigrationRegistry, LegacyMigrationPreview, LegacyMigrationReceipt, LegacyMigrationItem, LegacyMigrationStatus};
#[cfg(test)] #[path = "scheduler_tests.rs"] mod tests;
