//! Actual database-bound statements, never operation markers or reconstructed SQL.
//! SQL remains untouched; parameter values are separate typed Debug representations.
//! Connection credentials/options are deliberately never formatted or persisted here.
use crate::{
    db::Db,
    models::{ConnectionConfig, SqlLogEntry, SqlLogParameter},
};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    future::Future,
    path::PathBuf,
    sync::{Arc, OnceLock, Weak},
    time::Instant,
};

#[derive(Clone, Default)]
pub struct Context {
    pub connection_id: String,
    pub connection_name: String,
    pub kind: String,
    pub database: String,
    pub namespace: String,
    pub path: Option<PathBuf>,
    pub origin: String,
    pub operation_id: Option<String>,
}
tokio::task_local! { static CURRENT: Context; }
pub fn current_context() -> Context {
    CURRENT.try_with(Clone::clone).unwrap_or_default()
}
pub fn scope<F: Future>(context: Context, work: F) -> impl Future<Output = F::Output> {
    CURRENT.scope(context, Box::pin(work))
}
pub fn spawn<F>(work: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(scope(current_context(), work))
}
fn registry() -> &'static Mutex<HashMap<usize, (Weak<Db>, Context)>> {
    static MAP: OnceLock<Mutex<HashMap<usize, (Weak<Db>, Context)>>> = OnceLock::new();
    MAP.get_or_init(Default::default)
}
fn key(db: &Db) -> usize {
    db as *const Db as usize
}
pub fn register(db: &Arc<Db>, cfg: &ConnectionConfig, namespace: &str, root: &std::path::Path) {
    let mut context = Context {
        connection_id: cfg.id.clone(),
        connection_name: cfg.name.clone(),
        kind: cfg.kind.0.clone(),
        database: cfg.database.clone(),
        namespace: if matches!(db.as_ref(), Db::Postgres(_)) {
            namespace
                .strip_prefix("schema:")
                .unwrap_or(namespace)
                .into()
        } else {
            namespace.into()
        },
        path: Some(root.join("logs").join("sql_log.jsonl")),
        origin: "tool".into(),
        operation_id: None,
    };
    match db.as_ref() {
        Db::Sqlite(p) => {
            context.database = p
                .connect_options()
                .get_filename()
                .to_string_lossy()
                .into_owned()
        }
        Db::Mysql(p) => context.database = p.connect_options().get_database().unwrap_or("").into(),
        Db::Postgres(p) => {
            context.database = p
                .connect_options()
                .get_database()
                .unwrap_or("postgres")
                .into()
        }
        _ => {}
    }
    let mut map = registry().lock();
    map.retain(|_, (db, _)| db.strong_count() > 0);
    map.insert(key(db), (Arc::downgrade(db), context));
}
pub fn db_context(db: &Db) -> Context {
    let active = current_context();
    let mut c = registry()
        .lock()
        .get(&key(db))
        .and_then(|(weak, c)| weak.upgrade().map(|_| c.clone()))
        .unwrap_or_else(|| active.clone());
    if active.operation_id.is_some() {
        c.operation_id = active.operation_id;
    }
    if !active.origin.is_empty() {
        c.origin = active.origin;
    }
    c
}
pub async fn with_db<F: Future>(db: &Db, work: F) -> F::Output {
    scope(db_context(db), work).await
}

/// Logging never creates data_dir in tests. An explicit sink is required even for integration tests.
fn persist_direct(entry: &SqlLogEntry, path: &std::path::Path) {
    persist_batch(std::slice::from_ref(entry), path);
}
fn persist_batch(entries: &[SqlLogEntry], path: &std::path::Path) {
    static WRITE: Mutex<()> = Mutex::new(());
    let _write = WRITE.lock();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    crate::storage::append_sql_logs_at(path, entries);
}
enum WriteEvent {
    Entry(PathBuf, SqlLogEntry),
    Flush(std::sync::mpsc::Sender<()>),
}
/// Only coalesce already queued entries for the same sink. Never delay the first
/// entry to fill a batch, reorder sinks, or consume a Flush before its writes.
fn run_writer(receive: std::sync::mpsc::Receiver<WriteEvent>) {
    const MAX_ENTRIES: usize = 128;
    const MAX_BYTES: usize = 256 * 1024;
    fn payload(entry: &SqlLogEntry) -> usize {
        entry.sql.len().saturating_add(entry.message.len()).saturating_add(512)
            .saturating_add(entry.parameters.iter().map(|p| p.value.len().saturating_add(p.type_name.len()).saturating_add(64)).fold(0usize, usize::saturating_add))
    }
    let mut pending = None;
    loop {
        let event = match pending.take() { Some(event) => event, None => match receive.recv() { Ok(event) => event, Err(_) => break } };
        match event {
            WriteEvent::Entry(path, entry) => {
                let mut bytes = payload(&entry);
                let mut batch = vec![entry];
                while batch.len() < MAX_ENTRIES && bytes < MAX_BYTES {
                    match receive.try_recv() {
                        Ok(WriteEvent::Entry(next_path, next)) if next_path == path && payload(&next) <= MAX_BYTES.saturating_sub(bytes) => {
                            bytes = bytes.saturating_add(payload(&next)); batch.push(next);
                        }
                        Ok(next) => { pending = Some(next); break; }
                        Err(_) => break,
                    }
                }
                persist_batch(&batch, &path);
            }
            WriteEvent::Flush(done) => { let _ = done.send(()); }
        }
    }
}
#[cfg(not(test))]
fn writer() -> Option<&'static std::sync::mpsc::Sender<WriteEvent>> {
    static WRITER: OnceLock<Option<std::sync::mpsc::Sender<WriteEvent>>> = OnceLock::new();
    WRITER
        .get_or_init(|| {
            let (send, receive) = std::sync::mpsc::channel();
            std::thread::Builder::new()
                .name("sql-history".into())
                .spawn(move || run_writer(receive))
                .ok()
                .map(|_| send)
        })
        .as_ref()
}
fn persist(entry: &SqlLogEntry, context: &Context) {
    let Some(path) = &context.path else { return };
    #[cfg(test)]
    persist_direct(entry, path);
    #[cfg(not(test))]
    if let Some(writer) = writer() {
        let _ = writer.send(WriteEvent::Entry(path.clone(), entry.clone()));
    }
}
/// A history read/clear can wait for queued persistence; database futures never do.
pub fn flush() {
    #[cfg(not(test))]
    if let Some(writer) = writer() {
        let (send, receive) = std::sync::mpsc::channel();
        if writer.send(WriteEvent::Flush(send)).is_ok() {
            let _ = receive.recv_timeout(std::time::Duration::from_secs(2));
        }
    }
}
#[derive(Clone, Default)]
pub struct Meta {
    pub parameters: Vec<SqlLogParameter>,
}
impl Meta {
    fn bind<T: std::fmt::Debug>(&mut self, value: &T) {
        self.parameters.push(SqlLogParameter {
            ordinal: self.parameters.len() + 1,
            type_name: std::any::type_name::<T>().into(),
            value: format!("{value:?}"),
            redacted: false,
        });
    }
}
pub struct Guard {
    entry: Option<SqlLogEntry>,
    context: Context,
    started: Instant,
}
impl Guard {
    pub fn new(sql: &str, parameters: Vec<SqlLogParameter>) -> Self {
        let c = current_context();
        Self::in_context(sql, parameters, c)
    }
    pub fn in_context(sql: &str, parameters: Vec<SqlLogParameter>, c: Context) -> Self {
        Self {
            entry: Some(SqlLogEntry {
                ts: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                connection_id: c.connection_id.clone(),
                connection_name: c.connection_name.clone(),
                kind: c.kind.clone(),
                database: c.database.clone(),
                namespace: c.namespace.clone(),
                parameters,
                sql: sql.into(),
                ok: false,
                elapsed_ms: 0,
                rows: -1,
                message: String::new(),
                origin: if c.origin.is_empty() {
                    "tool".into()
                } else {
                    c.origin.clone()
                },
                operation_id: c.operation_id.clone(),
                execution_state: None,
            }),
            context: c,
            started: Instant::now(),
        }
    }
    pub fn finish(&mut self, ok: bool, rows: i64, message: String) {
        if let Some(mut entry) = self.entry.take() {
            entry.ok = ok;
            entry.rows = rows;
            entry.message = message;
            entry.elapsed_ms = self.started.elapsed().as_millis().min(u64::MAX as u128) as u64;
            entry.execution_state = Some(if ok {
                crate::execution_contracts::ExecutionState::Succeeded
            } else {
                crate::execution_contracts::ExecutionState::Failed
            });
            persist(&entry, &self.context);
        }
    }
    pub fn result<T, E: std::fmt::Display>(
        &mut self,
        result: &Result<T, E>,
        rows: impl FnOnce(&T) -> i64,
    ) {
        match result {
            Ok(value) => self.finish(true, rows(value), String::new()),
            Err(error) => self.finish(false, -1, error.to_string()),
        }
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(mut entry) = self.entry.take() {
            entry.elapsed_ms = self.started.elapsed().as_millis().min(u64::MAX as u128) as u64;
            entry.execution_state = Some(crate::execution_contracts::ExecutionState::Unknown);
            entry.message="Database execution/response was not fully observed; dropping the future or stream does not confirm cancellation or rollback".into();
            persist(&entry, &self.context);
        }
    }
}

/// Physical pool options are Arc-shared by clones; unlike pointer-to-Pool, identity survives moves.
pub trait ExecutorContext {
    fn sql_context(&self) -> Context;
}
macro_rules! pool_context {
    ($db:ty,$variant:ident) => {
        impl ExecutorContext for &sqlx::Pool<$db> {
            fn sql_context(&self) -> Context {
                let options = self.connect_options();
                let active = current_context();
                for (_, (weak, context)) in registry().lock().iter() {
                    if let Some(db) = weak.upgrade() {
                        if let Db::$variant(pool) = db.as_ref() {
                            if Arc::ptr_eq(&options, &pool.connect_options()) {
                                let mut c = context.clone();
                                c.origin = active.origin.clone();
                                c.operation_id = active.operation_id.clone();
                                return c;
                            }
                        }
                    }
                }
                active
            }
        }
    };
}
pool_context!(sqlx::Sqlite, Sqlite);
pool_context!(sqlx::MySql, Mysql);
pool_context!(sqlx::Postgres, Postgres);
impl ExecutorContext for &mut sqlx::SqliteConnection {
    fn sql_context(&self) -> Context {
        current_context()
    }
}
impl ExecutorContext for &mut sqlx::MySqlConnection {
    fn sql_context(&self) -> Context {
        current_context()
    }
}
impl ExecutorContext for &mut sqlx::PgConnection {
    fn sql_context(&self) -> Context {
        current_context()
    }
}

#[path = "sql_history_sqlx.rs"]
mod queries;
pub use queries::*;
#[cfg(test)]
#[path = "sql_history_tests.rs"]
mod tests;
