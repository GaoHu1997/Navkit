//! 详细还原：完整固定源预扫后，专用物理会话、顺序 raw_sql、有界报告。
//! 与普通导入整文件事务不同：1000 条/8 MiB 连续 DML 软阈值，非 DML 前提交。
use std::future::Future;
use std::pin::Pin;
use anyhow::{anyhow, Result};
use sqlx::Connection;
use super::{Db, RestoreReport, RestoreStep, TaskControl, SCRIPT_BATCH_BYTES, SCRIPT_BATCH_STATEMENTS, step_sql_short};
use super::transfer_sql_reader::{PreparedScript, ScriptDialect};

const REPORT_LIMIT: usize = 2048;
const STATEMENT_LIMIT: usize = 50 * 1024 * 1024;

struct Failure { message: String, certain: bool }
impl From<sqlx::Error> for Failure {
    fn from(error: sqlx::Error) -> Self { let certain = matches!(&error, sqlx::Error::Database(_)); Self { message: error.to_string(), certain } }
}
type Receipt<'a> = Pin<Box<dyn Future<Output = std::result::Result<u64, Failure>> + Send + 'a>>;
trait Session {
    fn execute<'a>(&'a mut self, sql: &'a str) -> Receipt<'a>;
    fn transactional(&self) -> bool { true }
    fn rollback_proven(&self) -> bool { true }
    fn begin_sql(&self) -> &'static str { "BEGIN" }
}
enum NativeSession { Sqlite(sqlx::SqliteConnection), Mysql(sqlx::MySqlConnection), Postgres(sqlx::PgConnection) }
impl Session for NativeSession {
    fn execute<'a>(&'a mut self, sql: &'a str) -> Receipt<'a> {
        Box::pin(async move {
            macro_rules! exec { ($conn:expr) => { sqlx::Executor::execute($conn, sqlx::raw_sql(sql)).await.map(|r| r.rows_affected()).map_err(Failure::from) }; }
            match self { Self::Sqlite(c) => exec!(c), Self::Mysql(c) => exec!(c), Self::Postgres(c) => exec!(c) }
        })
    }
    // MySQL 未验证全部目标存储引擎/触发器；ROLLBACK 回执不能证明非事务副作用消失。
    fn rollback_proven(&self) -> bool { !matches!(self, Self::Mysql(_)) }
    fn begin_sql(&self) -> &'static str { if matches!(self, Self::Mysql(_)) { "START TRANSACTION" } else { "BEGIN" } }
}
impl NativeSession {
    async fn open(db: &Db) -> Result<Self> {
        match db {
            Db::Sqlite(p) => {
                // A private memory database cannot be reopened from connect options:
                // the new connection may own a different database and falsely report
                // successful restoration. Do not close the original pool anchor either.
                let mut anchor = p.acquire().await?;
                let file: String = crate::sql_history::query_scalar("SELECT file FROM pragma_database_list WHERE name='main'")
                    .fetch_one(&mut *anchor).await?;
                if file.is_empty() {
                    return Err(anyhow!("RESTORE_MEMORY_TARGET_UNSUPPORTED: 详细还原需要固定文件 SQLite；内存库不能安全重开为专用物理会话，未发送还原 SQL、未修改原库"));
                }
                let options = (*p.connect_options()).clone().create_if_missing(false);
                let connection = sqlx::SqliteConnection::connect_with(&options).await?;
                drop(anchor);
                Ok(Self::Sqlite(connection))
            },
            Db::Mysql(p) => Ok(Self::Mysql(sqlx::MySqlConnection::connect_with(&(*p.connect_options()).clone()).await?)),
            Db::Postgres(p) => Ok(Self::Postgres(sqlx::PgConnection::connect_with(&(*p.connect_options()).clone()).await?)),
            _ => Err(anyhow!("不是 native 还原通道")),
        }
    }
    async fn close(self) -> Result<()> {
        match self { Self::Sqlite(c) => c.close().await?, Self::Mysql(c) => c.close().await?, Self::Postgres(c) => c.close().await? }
        Ok(())
    }
}
struct GenericSession<'a>(&'a Db);
impl Session for GenericSession<'_> {
    fn execute<'a>(&'a mut self, sql: &'a str) -> Receipt<'a> {
        Box::pin(async move { super::execute_one(self.0, sql).await.map(|n| n.unwrap_or(0)).map_err(|e| Failure { message: e.to_string(), certain: false }) })
    }
    fn transactional(&self) -> bool { false }
}

struct Audit { report: RestoreReport, pending: usize, pending_bytes: usize, pending_start: usize, in_tx: bool, next: usize }
impl Audit {
    fn new(total: usize) -> Self {
        Self { report: RestoreReport { total, succeeded: 0, failed: 0, steps: Vec::with_capacity(total.min(REPORT_LIMIT)), acked: 0, committed: 0, rolled_back: 0, skipped: 0, unknown: 0, attempted: 0, first_failed_index: None, omitted_steps: 0, effects_unknown: false, error: None, cleanup_error: None, warnings: Vec::new() }, pending: 0, pending_bytes: 0, pending_start: 0, in_tx: false, next: 1 }
    }
    fn push(&mut self, index: usize, sql: &str, ok: bool, affected: Option<u64>, error: Option<String>, skipped: bool, effect: &str) {
        let step = RestoreStep { index, sql: step_sql_short(sql), ok, affected, error: error.map(|s| s.chars().take(2048).collect()), skipped, effect: effect.into() };
        if self.report.steps.len() < REPORT_LIMIT { self.report.steps.push(step); }
        else if !ok && !skipped { self.report.steps[REPORT_LIMIT - 1] = step; }
    }
    fn settle(&mut self, effect: &str) {
        match effect { "committed" => self.report.committed += self.pending, "rolled_back" => self.report.rolled_back += self.pending, _ => self.report.unknown += self.pending }
        for step in &mut self.report.steps { if step.index >= self.pending_start && step.effect == "acked" { step.effect = effect.into(); step.ok = effect == "committed"; } }
        self.pending = 0; self.pending_bytes = 0;
        self.report.succeeded = self.report.committed;
    }
    fn unknown(&mut self, message: String) { self.report.effects_unknown = true; self.report.error = Some(format!("TRANSFER_EFFECTS_UNKNOWN: {}；禁止自动重放，须核对目标", message.chars().take(2048).collect::<String>())); }
    fn finish(&mut self) {
        let skipped = self.report.total.saturating_sub(self.next - 1);
        self.report.skipped += skipped;
        // 小脚本继续返回所有后续 skipped；大脚本绝不按总量迭代或保留。
        let room = REPORT_LIMIT.saturating_sub(self.report.steps.len());
        for index in self.next..self.next + room.min(skipped) { self.push(index, "", false, None, Some("前序语句失败，已终止".into()), true, "skipped"); }
        self.report.omitted_steps = self.report.total.saturating_sub(self.report.steps.len());
        self.report.succeeded = self.report.committed;
    }
}
async fn commit<S: Session>(session: &mut S, audit: &mut Audit) -> bool {
    if !audit.in_tx { return true; }
    match session.execute("COMMIT").await {
        Ok(_) => { audit.settle("committed"); audit.in_tx = false; true },
        Err(e) => { audit.in_tx = false; audit.settle("unknown"); audit.unknown(format!("COMMIT 回执未确认：{}", e.message)); false },
    }
}
async fn rollback<S: Session>(session: &mut S, audit: &mut Audit) -> bool {
    if !audit.in_tx { return true; }
    match session.execute("ROLLBACK").await {
        Ok(_) => {
            audit.in_tx = false;
            if session.rollback_proven() { audit.settle("rolled_back"); true }
            else { audit.settle("unknown"); audit.unknown("MySQL ROLLBACK 已回执，但非事务引擎/触发器副作用未证明已回滚".into()); false }
        },
        Err(e) => { audit.in_tx = false; audit.settle("unknown"); audit.unknown(format!("ROLLBACK 回执未确认：{}", e.message)); false },
    }
}
async fn execute<S: Session>(session: &mut S, script: &mut PreparedScript, stop_on_error: bool, task: Option<&TaskControl>) -> RestoreReport {
    let mut audit = Audit::new(script.count() as usize);
    loop {
        if let Some(control) = task { if let Err(e) = control.check() { audit.report.error = Some(format!("任务已取消，停止下一条；已提交前缀保留：{e}")); rollback(session, &mut audit).await; break; } }
        let statement = match script.next_statement() {
            Ok(Some(sql)) => sql,
            Ok(None) => { commit(session, &mut audit).await; break; },
            Err(e) => { audit.report.error = Some(format!("SQL 快照验证失败，本条及后续未发送：{e}")); rollback(session, &mut audit).await; break; },
        };
        let index = audit.next;
        let dml = script.counts_rows();
        let threshold = audit.pending >= SCRIPT_BATCH_STATEMENTS || audit.pending_bytes.saturating_add(statement.len()) > SCRIPT_BATCH_BYTES;
        if audit.in_tx && (!dml || threshold) && !commit(session, &mut audit).await { break; }
        if dml && session.transactional() && !audit.in_tx {
            let begin = session.begin_sql();
            if let Err(e) = session.execute(begin).await {
                audit.report.error = Some(format!("批事务 BEGIN 失败，本条及后续未发送：{}", e.message));
                if !e.certain { audit.unknown(format!("BEGIN 回执未确认：{}", e.message)); }
                break;
            }
            audit.in_tx = true; audit.pending_start = index;
        }
        audit.next += 1; audit.report.attempted += 1;
        let sql = crate::db::strip_leading_comments(&statement);
        match session.execute(&statement).await {
            Ok(affected) => {
                audit.report.acked += 1;
                if audit.in_tx { audit.pending += 1; audit.pending_bytes = audit.pending_bytes.saturating_add(statement.len()); audit.push(index, &statement, true, Some(affected), None, false, "acked"); }
                else { audit.report.committed += 1; audit.push(index, &statement, true, Some(affected), None, false, "committed"); }
            },
            Err(e) if e.certain && super::is_fk_guard_stmt(&sql) => {
                audit.report.skipped += 1;
                audit.push(index, &statement, true, None, Some(format!("守卫语句已跳过：{}", e.message)), true, "skipped");
            },
            Err(e) => {
                audit.report.failed += 1;
                audit.report.first_failed_index.get_or_insert(index);
                let uncertain = !e.certain || !session.rollback_proven();
                if uncertain { audit.report.unknown += 1; }
                audit.push(index, &statement, false, None, Some(e.message.clone()), false, if uncertain { "unknown" } else { "failed" });
                let rolled_back = rollback(session, &mut audit).await;
                if uncertain && !audit.report.effects_unknown { audit.unknown(format!("第 {index} 条结果未证明，执行回执丢失或非事务副作用可能保留：{}", e.message)); }
                if stop_on_error || uncertain || !rolled_back { break; }
            },
        }
        if let Some(control) = task { control.update("restore", index as u64, script.count(), "顺序还原；已回执不等于已提交"); }
        if index % 100 == 0 { tokio::task::yield_now().await; }
    }
    // 未完成的正常事务必须确认回滚；未知 COMMIT/ROLLBACK 已直接转 finally-close，不重放控制命令。
    if audit.in_tx { rollback(session, &mut audit).await; }
    audit.finish(); audit.report
}

pub(super) async fn run(db: &Db, path: &str, stop_on_error: bool) -> Result<RestoreReport> {
    run_with_control(db, path, stop_on_error, None).await
}
// 当前公开详细 API 没有 task id；内部接缝支持取消，但不会声称关闭对话框等同取消。
pub(super) async fn run_with_control(db: &Db, path: &str, stop_on_error: bool, task: Option<&TaskControl>) -> Result<RestoreReport> {
    let dialect = match db { Db::Sqlite(_) => ScriptDialect::Sqlite, Db::Mysql(_) => ScriptDialect::Mysql, Db::Postgres(_) => ScriptDialect::Postgres, _ => ScriptDialect::Generic };
    if let Some(task) = task { task.update("scan",0,0,"正在完整预扫描并冻结 SQL；未发送数据库写入"); }
    let file = path.to_string(); let control = task.cloned();
    let mut script = tokio::task::spawn_blocking(move || PreparedScript::prepare_restore(&file, dialect, STATEMENT_LIMIT, None, || control.as_ref().map_or(Ok(()), TaskControl::check))).await??;
    if let Some(task) = task { task.update("restore",0,script.count(),"预扫描已完成；停止将等待当前操作回执，保留已提交前缀"); }
    let mut report;
    if dialect == ScriptDialect::Generic {
        report = execute(&mut GenericSession(db), &mut script, stop_on_error, task).await;
        report.warnings.push("非 native 有界兼容模式：逐条原通道执行，无任务级专用物理连接/事务保证；仅支持分号和标准引号脚本。".into());
    } else {
        // 预扫描结束后才建立专用物理会话；任何执行结果都经过同一个 finally-close。
        let mut session = match NativeSession::open(db).await {
            Ok(s) => s,
            Err(e) => { return match script.cleanup() { Ok(()) => Err(e), Err(cleanup) => Err(e.context(format!("连接建立失败且 SQL 私有快照清理失败：{cleanup}"))) }; },
        };
        report = execute(&mut session, &mut script, stop_on_error, task).await;
        if let Err(e) = session.close().await {
            report.effects_unknown = true;
            report.error = Some(format!("{}TRANSFER_EFFECTS_UNKNOWN: 专用连接 close 未确认：{}；禁止自动重放", report.error.as_ref().map(|s| format!("{s}\n")).unwrap_or_default(), e.to_string().chars().take(2048).collect::<String>()));
        }
    }
    if let Err(e) = script.cleanup() { report.cleanup_error = Some(format!("SQL 私有快照清理失败：{}；目标执行结果不变", e.to_string().chars().take(2048).collect::<String>())); }
    Ok(report)
}

#[cfg(test)]
#[path = "transfer_restore_stream_tests.rs"]
mod tests;
