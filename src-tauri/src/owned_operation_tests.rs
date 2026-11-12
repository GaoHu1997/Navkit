//! The production owned worker retains a real transaction and its guards even if its IPC waiter disappears.
use super::owned_tool_task;
use std::sync::Arc;
use tokio::sync::{oneshot, Mutex, OwnedMutexGuard};

struct ReleaseNotice {
    guard: Option<OwnedMutexGuard<()>>,
    done: Option<oneshot::Sender<()>>,
}
impl Drop for ReleaseNotice {
    fn drop(&mut self) {
        drop(self.guard.take());
        if let Some(done) = self.done.take() { let _ = done.send(()); }
    }
}
async fn pool() -> sqlx::SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1)
        .connect("sqlite::memory:").await.unwrap();
    let mut setup = pool.acquire().await.unwrap();
    sqlx::query("CREATE TABLE effects(value INTEGER NOT NULL)").execute(&mut *setup).await.unwrap();
    setup.return_to_pool().await;
    pool
}
async fn count(pool: &sqlx::SqlitePool) -> i64 {
    let mut observer = pool.acquire().await.unwrap();
    let value = sqlx::query_scalar("SELECT COUNT(*) FROM effects").fetch_one(&mut *observer).await.unwrap();
    observer.return_to_pool().await;
    value
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_waiter_keeps_transaction_and_lease_until_real_commit() {
    let pool = pool().await;
    let work_pool = pool.clone();
    let lock = Arc::new(Mutex::new(()));
    let (started, observed_start) = oneshot::channel();
    let (finish, observed_finish) = oneshot::channel();
    let (released, observed_release) = oneshot::channel();
    let guard = ReleaseNotice { guard: Some(lock.clone().lock_owned().await), done: Some(released) };
    let waiter = tokio::spawn(owned_tool_task(async move {
        let mut owned = work_pool.acquire().await?;
        let mut transaction = sqlx::Connection::begin(&mut *owned).await?;
        sqlx::query("INSERT INTO effects VALUES(1)").execute(&mut *transaction).await?;
        let _ = started.send(());
        observed_finish.await?;
        transaction.commit().await?;
        owned.return_to_pool().await;
        Ok::<_, anyhow::Error>(1_u64)
    }, guard));
    tokio::time::timeout(std::time::Duration::from_secs(3), observed_start).await.unwrap().unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    assert!(lock.try_lock().is_err(), "a lost IPC waiter must not release the transaction lease");
    finish.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), observed_release).await.unwrap().unwrap();
    assert!(lock.try_lock().is_ok());
    assert_eq!(count(&pool).await, 1, "the actual sent transaction was committed exactly once");
    pool.close().await;
    // 关键不变量：关闭态拒绝新连接（物理连接数受其它 Arc 持有者释放时序影响）。
    assert!(pool.is_closed());
    assert!(pool.acquire().await.is_err());
}

#[tokio::test]
async fn rollback_and_guard_release_finish_before_error_acknowledgement() {
    let pool = pool().await;
    let work_pool = pool.clone();
    let lock = Arc::new(Mutex::new(()));
    let (released, mut observed_release) = oneshot::channel();
    let guard = ReleaseNotice { guard: Some(lock.clone().lock_owned().await), done: Some(released) };
    let result = owned_tool_task(async move {
        let mut owned = work_pool.acquire().await?;
        let mut transaction = sqlx::Connection::begin(&mut *owned).await?;
        sqlx::query("INSERT INTO effects VALUES(1)").execute(&mut *transaction).await?;
        transaction.rollback().await?;
        owned.return_to_pool().await;
        Err::<u64, _>(anyhow::anyhow!("explicit rollback fixture error"))
    }, guard).await;
    assert!(result.is_err());
    observed_release.try_recv().expect("the receipt cannot precede releasing its actual guards");
    assert!(lock.try_lock().is_ok());
    assert_eq!(count(&pool).await, 0);
    pool.close().await;
    // 关键不变量：关闭态拒绝新连接（物理连接数受其它 Arc 持有者释放时序影响）。
    assert!(pool.is_closed());
    assert!(pool.acquire().await.is_err());
}
