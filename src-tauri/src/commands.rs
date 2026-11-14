//! Tauri 命令层：所有前端可调用的接口。

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::anyhow;

use crate::error::{AppError, CmdResult};
use sqlx::{Column, Row};
use tauri::State;

use crate::db::{self, quote_literal, DbRegistry};
use crate::models::*;
use crate::schema;
use crate::storage::{self, Store};

/// Result 区分合法空密码与解密失败；不创建或替换解密密钥。
fn decrypt_or_err(store: &Store, cfg: &ConnectionConfig) -> anyhow::Result<String> {
    store.decrypt_password(&cfg.password)
}

/// 应用全局状态。
pub struct AppState {
    pub store: Store,
    pub registry: Arc<DbRegistry>,
    pub manual_transactions: crate::manual_transaction::Registry,
    pub activity: crate::activity::ActivityGate,
    pub executions: crate::execution::Registry,
    pub mutations: crate::mutation::Registry,
    pub connection_runtime: crate::db::runtime::RuntimeRegistry,
    pub query_results: Arc<crate::query_execution::QueryPages>,
}

// ---------------------------------------------------------------- 设置

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> AppSettings {
    state.store.settings()
}

#[tauri::command]
pub fn set_settings(state: State<'_, AppState>, settings: AppSettings) -> CmdResult<()> {
    settings.validate_execution_settings()?;
    state.store.set_settings(settings).map_err(AppError::new)?;
    Ok(())
}

// ---------------------------------------------------------------- 连接

/// 列出全部连接（含分组）。
#[tauri::command]
pub fn list_connections(state: State<'_, AppState>) -> Vec<ConnectionConfig> {
    state.store.connections()
}

/// 列出分组。
#[tauri::command]
pub fn list_groups(state: State<'_, AppState>) -> Vec<String> {
    state.store.groups()
}

/// 新建分组（空分组也持久化，供连接树展示）。
#[tauri::command]
pub fn add_group(state: State<'_, AppState>, name: String) -> CmdResult<Vec<String>> {
    let _activity_guard = state.activity.enter()?;
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::new("分组名不能为空"));
    }
    state.store.update_connections(|_, groups| {
        if !groups.contains(&name) { groups.push(name); }
        groups.sort();
        Ok(groups.clone())
    }).map_err(AppError::new)
}

/// 保存连接（新增或更新）。
#[tauri::command]
pub async fn save_connection(
    state: State<'_, AppState>,
    config: ConnectionConfig,
    password: Option<String>,
    runtime_profile: Option<crate::db::runtime::RuntimeSubmission>,
) -> CmdResult<Vec<ConnectionConfig>> {
    let activity = state.activity.exclusive()?;
    let manual = state.manual_transactions.ordinary(&[&config.id]).await?;
    let changed_id = config.id.clone();
    if runtime_profile.is_some() { state.store.ensure_encryption_key()?; }
    let result = crate::ssh_tunnel::profiles::commit::save_connection(&state.store, state.store.data_root(), config, password.as_deref(), runtime_profile.as_ref())?;
    if result.reopened {
        crate::runtime_commands::retire(&state, &changed_id, (activity, manual)).await?;
    }
    Ok(result.connections)
}

/// 移动/重排仅更新持久树顺序；连接池与按库派生池保持不变。
#[tauri::command]
pub fn move_connection(
    state: State<'_, AppState>,
    id: String,
    group: String,
    target_id: Option<String>,
    position: Option<ConnectionMovePosition>,
) -> CmdResult<Vec<ConnectionConfig>> {
    let _activity_guard = state.activity.enter()?;
    state.store.move_connection(&id, &group, target_id.as_deref(), position).map_err(AppError::new)
}

#[cfg(test)]
fn move_connection_config(state: &AppState, id: &str, group: &str) -> CmdResult<Vec<ConnectionConfig>> {
    state.store.move_connection(id, group, None, None).map_err(AppError::new)
}

/// 删除连接。
#[tauri::command]
pub async fn delete_connection(state: State<'_, AppState>, id: String) -> CmdResult<Vec<ConnectionConfig>> {
    let activity = state.activity.exclusive()?;
    let manual = state.manual_transactions.ordinary(&[&id]).await?;
    let conns = crate::ssh_tunnel::profiles::commit::delete_connection(&state.store, state.store.data_root(), &id)?;
    crate::runtime_commands::retire(&state, &id, (activity, manual)).await?;
    Ok(conns)
}

/// 重命名分组。
#[tauri::command]
pub fn rename_group(
    state: State<'_, AppState>,
    old_name: String,
    new_name: String,
) -> CmdResult<Vec<ConnectionConfig>> {
    let _activity_guard = state.activity.enter()?;
    let new_name = new_name.trim().to_string();
    if old_name.is_empty() || new_name.is_empty() { return Err(AppError::new("分组名不能为空")); }
    state.store.update_connections(|conns, groups| {
        if !groups.contains(&old_name) && !conns.iter().any(|c| c.group == old_name) { return Err("原分组不存在".into()); }
        for c in conns.iter_mut().filter(|c| c.group == old_name) { c.group = new_name.clone(); }
        for g in groups.iter_mut().filter(|g| **g == old_name) { *g = new_name.clone(); }
        groups.push(new_name);
        groups.sort();
        groups.dedup();
        Ok(conns.clone())
    }).map_err(AppError::new)
}

/// 检视连接高级参数的真实应用情况（目前覆盖 MySQL/MariaDB）。
#[tauri::command]
pub fn validate_connection_options(state: State<'_, AppState>, id: String) -> CmdResult<Vec<crate::mysql_options::OptionDiagnostic>> {
    let config = saved_config(&state, &id)?;
    let mut out = Vec::new();
    match config.kind.protocol() {
        crate::models::Protocol::Mysql => {
            out.extend(crate::mysql_options::parse(&config.params)?.diagnostics_for_ui());
            out.extend(crate::tls_options::parse(&config.params, true)?.diagnostics_for_ui());
        }
        crate::models::Protocol::Postgres => {
            out.extend(crate::tls_options::parse(&config.params, true)?.diagnostics_for_ui());
        }
        other => out.push(crate::mysql_options::OptionDiagnostic {
            key: "params".into(),
            status: "unsupported".into(),
            message: format!("当前数据源（{:?}）没有可验证的高级连接参数", other),
        }),
    }
    Ok(out)
}

/// 测试连接：握手、版本探测及清理共用一个期限；同步驱动不能阻塞 IPC 执行器。
#[tauri::command]
pub async fn test_connection(req: ConnectionRequest) -> CmdResult<TestResult> {
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_secs(req.timeout_secs.clamp(1, 86400));
    run_connection_test(deadline, test_connection_inner(req, deadline)).await
}

fn connection_test_timeout() -> TestResult {
    TestResult {
        ok: false,
        message: "CONNECT_TIMEOUT: 测试连接超时；后台继续清理迟到连接，未保存配置".into(),
        version: String::new(),
        charset: String::new(),
    }
}

async fn run_connection_test<F>(deadline: tokio::time::Instant, work: F) -> CmdResult<TestResult>
where
    F: std::future::Future<Output = CmdResult<TestResult>> + Send + 'static,
{
    let runtime = tokio::runtime::Handle::current();
    let context = crate::sql_history::current_context();
    let (send, receive) = tokio::sync::oneshot::channel();
    // ODBC/driver discovery may block before its first await. A plain async spawn
    // would still occupy the runtime; never abort this owner on caller timeout.
    tokio::task::spawn_blocking(move || {
        let result = runtime.block_on(crate::sql_history::scope(context, work));
        let _ = send.send(result);
    });
    match tokio::time::timeout_at(deadline, receive).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(AppError::coded("CONNECTION_TEST_UNKNOWN", "测试连接工作任务异常退出；未保存配置")),
        Err(_) => Ok(connection_test_timeout()),
    }
}

#[cfg(test)]
#[path = "commands_connection_test_deadline_tests.rs"]
mod connection_test_deadline_tests;

async fn test_connection_inner(req: ConnectionRequest, deadline: tokio::time::Instant) -> CmdResult<TestResult> {
    match db::open(&req).await {
        Ok(conn) => {
            let result = tokio::time::timeout_at(deadline, async {
            let mut version = String::new();
            let mut charset = String::new();
            match &conn {
                db::Db::Sqlite(pool) => {
                    if let Ok(row) =
                        crate::sql_history::query("SELECT sqlite_version() AS v").fetch_one(pool).await
                    {
                        let cols = row.columns();
                        if let Some(i) = cols.iter().position(|c| c.name() == "v") {
                            version = crate::values::sqlite_row_values(&row, cols.len())
                                .get(i)
                                .cloned()
                                .flatten()
                                .unwrap_or_default();
                        }
                    }
                    charset = "UTF-8".into();
                }
                db::Db::Mysql(pool) => {
                    if let Ok(row) = crate::sql_history::query("SELECT VERSION() AS v").fetch_one(pool).await {
                        let cols = row.columns();
                        if let Some(i) = cols.iter().position(|c| c.name() == "v") {
                            version = crate::values::my_row_values(&row, cols.len())
                                .get(i)
                                .cloned()
                                .flatten()
                                .unwrap_or_default();
                        }
                    }
                    charset = "utf8mb4".into();
                }
                db::Db::Mssql(c) => {
                    if let Ok(v) = c.scalar("SELECT @@VERSION").await {
                        version = v.unwrap_or_default();
                    }
                    charset = "数据库默认".into();
                }
                db::Db::Redis(c) => {
                    if let Ok(v) = c.version().await {
                        version = v;
                    }
                    charset = "UTF-8".into();
                }
                db::Db::Mongo(c) => {
                    if let Ok(v) = c.version().await {
                        version = v;
                    }
                    charset = "UTF-8".into();
                }
                db::Db::Http(c) => {
                    version = c.version.clone();
                    charset = "UTF-8".into();
                }
                db::Db::Postgres(pool) => {
                    if let Ok(row) = crate::sql_history::query("SELECT version() AS v").fetch_one(pool).await {
                        let cols = row.columns();
                        if let Some(i) = cols.iter().position(|c| c.name() == "v") {
                            version = crate::values::pg_row_values(&row, cols.len())
                                .get(i)
                                .cloned()
                                .flatten()
                                .unwrap_or_default();
                        }
                    }
                    if let Ok(row) =
                        crate::sql_history::query("SHOW server_encoding").fetch_one(pool).await
                    {
                        let cols = row.columns();
                        if let Some(i) = cols.iter().position(|c| c.name() == "server_encoding") {
                            charset = crate::values::pg_row_values(&row, cols.len())
                                .get(i)
                                .cloned()
                                .flatten()
                                .unwrap_or_default();
                        }
                    }
                }
                            _ => {}
}
            Ok(TestResult {
                ok: true,
                message: "连接成功".into(),
                version,
                charset,
            })
            }).await.unwrap_or_else(|_| Ok(connection_test_timeout()));
            // Always dispose the owned test connection, including a late handshake
            // or timed-out metadata probe; no test connection is registered.
            db::shutdown_db(&conn).await;
            drop(conn);
            result
        }
        Err(e) => Ok(TestResult {
            ok: false,
            message: format!("{:#}", e),
            version: String::new(),
            charset: String::new(),
        }),
    }
}

/// 打开连接。
#[tauri::command]
pub async fn open_connection(
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<TestResult> {
    let _activity_guard = state.activity.enter()?;
    let _manual_guard = state.manual_transactions.ordinary(&[&id]).await?;
    ensure_db(&state, &id).await?;
    crate::storage::log_line("connection", &format!("打开连接 {}", id));
    Ok(connection_success("连接成功"))
}

/// 确保已保存的连接已打开；已有连接绝不关闭或替换。
#[tauri::command]
pub async fn ensure_connection(
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<TestResult> {
    let _activity_guard = state.activity.enter()?;
    ensure_db(&state, &id).await?;
    Ok(connection_success("连接可用"))
}

/// 打开并实际校验树中的库/模式。PG 的树节点是 schema，而非数据库。
#[tauri::command]
pub async fn open_database(
    state: State<'_, AppState>,
    id: String,
    database: String,
) -> CmdResult<TestResult> {
    let _activity_guard = state.activity.enter()?;
    let _manual_guard = state.manual_transactions.ordinary(&[&id]).await?;
    if database.trim().is_empty() {
        return Err(AppError::new("数据库/模式不能为空"));
    }
    let conn = scoped_db(&state, &id, &database).await?;
    if let db::Db::Sqlite(pool) = conn.as_ref() { crate::sql_history::query("SELECT 1").fetch_one(pool).await?; }
    Ok(connection_success("数据库/模式可用"))
}

/// 关闭连接。
#[tauri::command]
pub async fn close_connection(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let activity = state.activity.enter()?;
    let mut lease = state.manual_transactions.lease(&id).await;
    lease.close().await?;
    crate::runtime_commands::retire(&state, &id, (activity, lease)).await?;
    crate::storage::log_line("connection", &format!("关闭连接 {}", id));
    Ok(())
}

/// 关闭全部连接。
#[tauri::command]
pub async fn close_all_connections(state: State<'_, AppState>) -> CmdResult<()> {
    let activity = state.activity.enter()?;
    let lease = state.manual_transactions.close_all().await?;
    let mut ids: Vec<String> = state.store.connections().into_iter().map(|cfg| cfg.id).collect();
    ids.extend(state.registry.read().keys().filter(|key| !key.contains('@')).cloned());
    crate::runtime_commands::retire_many(&state, ids, (activity, lease)).await
}

/// 列出某连接下的数据库 / 模式。
#[tauri::command]
pub async fn list_databases(
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<Vec<(String, String)>> {
    let _activity_guard = state.activity.enter()?;
    let db = ensure_db(&state, &id).await?;
    schema::list_databases(db.as_ref(), kind_of(&state, &id)?).await.map_err(AppError::from)
}

/// 表设计器「选项」页的真实候选项（引擎 / 字符集 / 排序规则 / 行格式）。
#[tauri::command]
pub async fn table_option_choices(
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<schema::TableOptionChoices> {
    let _activity_guard = state.activity.enter()?;
    let db = ensure_db(&state, &id).await?;
    schema::table_option_choices(db.as_ref(), kind_of(&state, &id)?).await.map_err(AppError::from)
}

/// 列出某数据库下的表与视图。
#[tauri::command]
pub async fn list_tables(
    state: State<'_, AppState>,
    id: String,
    schema_name: String,
) -> CmdResult<Vec<(String, ObjectType)>> {
    let _activity_guard = state.activity.enter()?;
    let db = scoped_db(&state, &id, &schema_name).await?;
    schema::list_tables(db.as_ref(), kind_of(&state, &id)?, &schema_name).await.map_err(AppError::from)
}

/// 列出某数据库下的对象与统计（对象标签页）。
#[tauri::command]
pub async fn list_objects(
    state: State<'_, AppState>,
    id: String,
    schema_name: String,
) -> CmdResult<Vec<crate::models::ObjectInfo>> {
    let _activity_guard = state.activity.enter()?;
    let db = scoped_db(&state, &id, &schema_name).await?;
    schema::list_objects(db.as_ref(), kind_of(&state, &id)?, &schema_name)
        .await
        .map_err(AppError::from)
}

/// 列出存储过程 / 函数 / 触发器。
#[tauri::command]
pub async fn list_routines(
    state: State<'_, AppState>,
    id: String,
    schema_name: String,
) -> CmdResult<Vec<(ObjectType, String)>> {
    let _activity_guard = state.activity.enter()?;
    let db = scoped_db(&state, &id, &schema_name).await?;
    schema::list_routines(db.as_ref(), kind_of(&state, &id)?, &schema_name).await.map_err(AppError::from)
}

/// 读取表结构。
#[tauri::command]
pub async fn get_table_schema(
    state: State<'_, AppState>,
    id: String,
    schema_name: String,
    table: String,
    include_row_count: Option<bool>,
) -> CmdResult<TableSchema> {
    let _activity_guard = state.activity.enter()?;
    let db = scoped_db(&state, &id, &schema_name).await?;
    let kind = kind_of(&state, &id)?;
    if include_row_count == Some(false) {
        schema::table_schema_for_page(db.as_ref(), kind, &schema_name, &table).await.map_err(AppError::from)
    } else {
        schema::table_schema(db.as_ref(), kind, &schema_name, &table).await.map_err(AppError::from)
    }
}

/// 只读关系图目录：不复用完整编辑结构验证，不统计表行数。
#[tauri::command]
pub async fn get_diagram_metadata(
    state: State<'_, AppState>,
    id: String,
    schema_name: String,
) -> CmdResult<crate::schema_diagram::DiagramMetadata> {
    let _activity_guard = state.activity.enter()?;
    let db = scoped_db(&state, &id, &schema_name).await?;
    crate::schema_diagram::load(db.as_ref(), kind_of(&state, &id)?, &schema_name).await.map_err(AppError::from)
}

/// 分页读取表数据。
#[tauri::command]
pub async fn page_table(
    state: State<'_, AppState>,
    req: PageRequest,
) -> CmdResult<PageResult> {
    let _activity_guard = state.activity.enter()?;
    let id = req.connection_id.clone();
    let db = scoped_db(&state, &id, &req.schema).await?;
    let kind = kind_of(&state, &id)?;
    schema::page_table(db.as_ref(), kind, &req).await.map_err(AppError::from)
}

#[cfg(test)]
fn sql_log_entry(
    cfg: &ConnectionConfig,
    database: &str,
    sql: &str,
    outcome: &crate::execution_contracts::ExecutionOutcome,
    origin: &str,
) -> SqlLogEntry {
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let ok = outcome.state == crate::execution_contracts::ExecutionState::Succeeded;
    let rows = match outcome.result.as_ref().filter(|_| ok) {
        Some(result) => {
            let (mut affected, mut returned) = (0u64, 0u64);
            for set in &result.sets {
                if set.ok {
                    if set.has_rows { returned += set.rows.len() as u64 } else { affected += set.affected }
                }
            }
            if returned > 0 { returned as i64 } else { affected as i64 }
        }
        None => -1,
    };
    let message = format!("state={:?}; stop_confirmed={}; effects={}; code={}", outcome.state, outcome.deadline.stop_confirmed, outcome.effects, outcome.error.as_ref().map_or("", |e| e.code.as_str()));
    SqlLogEntry {
        ts: now.to_string(),
        connection_id: cfg.id.clone(),
        connection_name: cfg.name.clone(),
        kind: cfg.kind.0.clone(),
        database: database.to_string(),
        sql: sql.to_owned(),
        namespace: database.to_string(),
        parameters: Vec::new(),
        ok,
        elapsed_ms: outcome.deadline.elapsed_ms,
        rows,
        message,
        origin: if matches!(origin, "query" | "monitor" | "job" | "page") { origin } else { "tool" }.into(),
        operation_id: Some(outcome.execution_id.clone()),
        execution_state: Some(outcome.state),
    }
}

/// 执行 SQL。MySQL 按库缓存连接；PG 选项是 schema，使用隔离的
/// search_path 连接池，但始终连接到保存配置指定的真实数据库。
/// 每次执行（成功或失败）都会写入 SQL 执行日志；origin 标记调用来源。
#[tauri::command]
pub async fn execute_sql(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    id: String,
    sql: String,
    database: Option<String>,
    transaction_owner: Option<String>,
    origin: Option<String>,
) -> CmdResult<MultiResult> {
    let mut request = crate::execution_contracts::ExecutionRequest { execution_id: uuid::Uuid::new_v4().to_string(), connection_id: id,
        namespace: database.unwrap_or_default(), owner_id: transaction_owner.clone().unwrap_or_else(|| "legacy-tool".into()), sql,
        transaction_owner, timeout_ms: None, origin, execution_grant: None, cancel_before_send: false };
    grant_execution_request(&state, &mut request, window.label())?;
    legacy_outcome(execute_sql_service(&state, request, window.label()).await?)
}

fn legacy_outcome(outcome: crate::execution_contracts::ExecutionOutcome) -> CmdResult<MultiResult> {
    if outcome.state == crate::execution_contracts::ExecutionState::Succeeded {
        return outcome.result.ok_or_else(|| AppError::coded("EXECUTION_RESULT_MISSING", "执行缺少结果"));
    }
    Err(outcome.error.unwrap_or_else(|| AppError::coded("EXECUTION_INCOMPLETE", format!("执行终态 {:?}；不能推断已回滚", outcome.state)).operation(&outcome.execution_id)))
}
pub(crate) fn execution_binding(state: &AppState, cfg: &ConnectionConfig, principal: &str, owner: &str, namespace: &str) -> CmdResult<crate::execution::Binding> {
    // 保存配置、密文配置版本与证书/SSH材料共同绑定；读取失败绝不退回空或旧指纹。
    let fingerprint = crate::db::runtime::binding_fingerprint_at(cfg, state.store.data_root()).map_err(AppError::from)?;
    Ok(crate::execution::Binding { principal: principal.into(), owner: owner.into(), connection: cfg.id.clone(), namespace: namespace.into(), fingerprint })
}
/// Only the legacy internal call sites may sign immediately before dispatch.
/// v2 must obtain its grant first and retain the exact returned execution identity.
pub(crate) fn grant_execution_request(state: &AppState, request: &mut crate::execution_contracts::ExecutionRequest, principal: &str) -> CmdResult<()> {
    if request.owner_id.trim().is_empty() || request.owner_id.len() > 128 || request.namespace.len() > 1024 || request.sql.len() > 16 * 1024 * 1024 {
        return Err(AppError::coded("EXECUTION_REQUEST_INVALID", "执行页面、命名空间或 SQL 长度无效；没有发送 SQL"));
    }
    let cfg = saved_config(state, &request.connection_id)?;
    let binding = execution_binding(&state, &cfg, principal, &request.owner_id, &request.namespace)?;
    state.executions.grant_request(request, binding)
}

/// Issue a sealed one-use execution identity to the actual calling window.
/// No database query is sent by this command.
#[tauri::command]
pub async fn issue_execution_grant(window: tauri::WebviewWindow, state: State<'_, AppState>, mut request: crate::execution_contracts::ExecutionRequest) -> CmdResult<crate::execution_contracts::ExecutionRequest> {
    let _activity = state.activity.enter()?;
    if request.execution_grant.is_some() {
        return Err(AppError::coded("EXECUTION_GRANT_INVALID", "不能续签或替换已有授权；请明确创建新执行"));
    }
    grant_execution_request(&state, &mut request, window.label())?;
    Ok(request)
}

/// SQL、例程与模型工程共用的唯一注册/授权消费入口。主体只取真实窗口。
pub(crate) fn register_execution(state: &AppState, request: &crate::execution_contracts::ExecutionRequest, principal: &str) -> CmdResult<(ConnectionConfig, crate::execution::Binding, Arc<crate::execution::Entry>)> {
    let activity = state.activity.enter()?;
    let cfg = saved_config(state, &request.connection_id)?;
    let binding = execution_binding(&state, &cfg, principal, &request.owner_id, &request.namespace)?;
    // Missing/expired/replayed v2 grants are rejected, never silently re-signed.
    let entry = state.query_results.register_execution(&binding, &request.execution_id,
        || state.executions.register(request, binding.clone(), state.store.settings().query_timeout_secs))?;
    entry.own_activity(activity);
    Ok((cfg, binding, entry))
}

pub(crate) async fn execute_sql_service(state: &AppState, request: crate::execution_contracts::ExecutionRequest, principal: &str) -> CmdResult<crate::execution_contracts::ExecutionOutcome> {
    let registered = register_execution(state, &request, principal)?;
    execute_registered_sql_service(state, request, principal, registered).await
}

pub(crate) async fn execute_registered_sql_service(
    state: &AppState, request: crate::execution_contracts::ExecutionRequest, principal: &str,
    registered: (ConnectionConfig, crate::execution::Binding, Arc<crate::execution::Entry>),
) -> CmdResult<crate::execution_contracts::ExecutionOutcome> {
    let (cfg, binding, entry) = registered;
    let mut transaction = false;
    let history_context=crate::sql_history::Context { connection_id:cfg.id.clone(),connection_name:cfg.name.clone(),kind:cfg.kind.0.clone(),database:cfg.database.clone(),namespace:request.namespace.clone(),path:Some(state.store.data_root().join("logs/sql_log.jsonl")),origin:request.origin.clone().unwrap_or_else(||"tool".into()),operation_id:Some(request.execution_id.clone()) };
    let outcome = crate::sql_history::scope(history_context,async {
        if entry.stopping() { return Ok((Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "停止发生在发送前，没有执行 SQL")), true)); }
        if cfg.read_only {
            let verified_tunnel_profile = db::runtime::prepare_at(&cfg, state.store.data_root())?.http.is_some();
            if !verified_tunnel_profile {
                if !cfg.kind.is_sqlite() || cfg.params.starts_with(crate::driver_manager::PARAMS_PREFIX) { return Err(AppError::coded("READ_ONLY_UNPROVEN", "当前驱动无法证明只读任意 SQL 安全；请使用结构化浏览")); }
                crate::execution_policy::readonly_batch(&request.sql)?;
            }
        }
        // 这里还没有用户 SQL；取消等待互斥 lease 不会丢弃数据库执行 future。
        let mut lease = tokio::select! {
            lease = state.manual_transactions.lease(&cfg.id) => lease,
            _ = entry.wait_stop() => return Ok((Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "排队已取消，没有发送 SQL")), true)),
        };
        state.executions.require_quiescent(&cfg.id)?;
        if cfg.read_only && lease.status().active { return Err(AppError::coded("READ_ONLY_TRANSACTION", "只读连接不能继续手动写事务")); }
        if !lease.status().active && matches!(cfg.kind.protocol(), Protocol::Sqlite | Protocol::Mysql | Protocol::Postgres) {
            crate::execution_policy::ordinary_native_batch(&request.sql)?;
        }
        if lease.status().active {
            transaction = true;
            entry.attach_spool(crate::query_spool::create(&cfg.id, &request.owner_id, &request.namespace, principal));
            let working_request = request.clone();
            let working_entry = entry.clone();
            return Ok(crate::execution::supervise(entry.clone(), async move {
                // The owning task retains the physical transaction and both lifecycle leases.
                if !working_entry.start() {
                    return (Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "手动事务执行在发送前已停止")), true);
                }
                match lease.execute_controlled(working_request.transaction_owner.as_deref(), &working_request.namespace, &working_request.sql, &working_entry).await {
                    Ok(Some(result)) => result,
                    Ok(None) => (Err(AppError::coded("MANUAL_TRANSACTION_ENDED", "手动事务已经结束，没有发送 SQL")), false),
                    Err(error) => (Err(AppError::from(error)), false),
                }
            }).await);
        }
        if request.transaction_owner.is_some() {
            return Err(AppError::coded("MANUAL_TRANSACTION_ENDED", "该查询的手动事务已结束，请刷新事务状态；没有发送 SQL"));
        }
        let (key, conn) = query_db(state, &cfg.id, Some(&request.namespace)).await?;
        if entry.stream().is_some() && !matches!(conn.as_ref(), db::Db::Sqlite(_) | db::Db::Mysql(_) | db::Db::Postgres(_)) {
            return Err(AppError::coded("STREAM_ADAPTER_UNSUPPORTED", "实际物理适配器不是已认证的原生流式驱动；SQL 没有发送"));
        }
        if execution_binding(&state, &saved_config(state, &cfg.id)?, principal, &request.owner_id, &request.namespace)? != binding {
            return Err(AppError::coded("EXECUTION_CONTEXT_CHANGED", "排队期间保存连接已改变，SQL 没有发送"));
        }
        if entry.stopping() { return Ok((Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "排队期间停止，没有执行 SQL")), true)); }
        if let db::Db::Http(http) = conn.as_ref() {
            if let Some(tunnel) = http.tunnel_runtime() {
                if !state.connection_runtime.matches(&key, &binding.fingerprint, &conn) { return Err(AppError::coded("EXECUTION_CONTEXT_CHANGED", "隧道物理资源与实际连接配置不符；SQL未发送")); }
                entry.attach_spool(crate::query_spool::create(&cfg.id, &request.owner_id, &request.namespace, principal));
                return Ok(tunnel.run_owned(request.namespace.clone(), request.sql.clone(), entry.clone(), cfg.read_only, lease).await);
            }
        }
        if matches!(conn.as_ref(), db::Db::Sqlite(_) | db::Db::Mysql(_) | db::Db::Postgres(_)) {
            // 原生流可以逐行落盘：界面仍只收有界预览，但这次执行的完整结果留在本机缓存里，
            // 「加载更多」与「导出全部」都读同一份快照，不重跑用户 SQL。
            entry.attach_spool(crate::query_spool::create(&cfg.id, &request.owner_id, &request.namespace, principal));
            if matches!(conn.as_ref(), db::Db::Sqlite(_)) && !cfg.read_only {
                entry.attach_query_capture(crate::query_execution::QueryCapture::new(state.query_results.clone(), binding.clone(), request.execution_id.clone()));
            }
            let working_entry = entry.clone();
            let working_sql = request.sql.clone();
            let working_kind = cfg.kind.clone();
            // C1's owned worker retains the ordinary/manual gate through uncertain cleanup.
            Ok(crate::execution::run_owned(conn, working_kind, working_sql, working_entry, cfg.read_only, lease).await)
        } else {
            // Legacy adapters still retain their owning lease/worker on an uncertain deadline.
            let registry = state.registry.clone();
            let sql = request.sql.clone();
            let kind = cfg.kind.clone();
            let working_entry = entry.clone();
            Ok(crate::execution::supervise(entry.clone(), async move {
                let _ordinary_lease = lease;
                if !working_entry.start() {
                    return (Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "发送前已停止，没有执行 SQL")), true);
                }
                let result = db::execute(&registry, &key, &sql, kind).await.map_err(|e| AppError::coded("EXECUTION_EFFECTS_UNKNOWN", format!("适配器没有可验证的取消/写入回执：{e:#}")));
                working_entry.stopping();
                working_entry.work_settled();
                (result, false)
            }).await)
        }
    }).await;
    let (result, confirmed) = outcome.unwrap_or_else(|e| (Err(e), false));
    let outcome = entry.finish(result, confirmed, transaction);
    // Actual submitted statements are recorded at the driver boundary, not this request summary.
    Ok(outcome)
}
#[tauri::command]
pub async fn execute_sql_v2(window: tauri::WebviewWindow, state: State<'_, AppState>, request: crate::execution_contracts::ExecutionRequest) -> CmdResult<crate::execution_contracts::ExecutionOutcome> {
    execute_sql_service(&state, request, window.label()).await
}
/// Opt-in native streaming. Reuses the reviewed one-use SQL grant unchanged.
/// Unsupported adapters are rejected before registration; use execute_sql_v2 there.
pub(crate) fn register_streaming_execution(
    state: &AppState, request: &crate::execution_contracts::ExecutionRequest, principal: &str,
    options: crate::execution_stream::StreamOptions,
) -> CmdResult<(ConnectionConfig, crate::execution::Binding, Arc<crate::execution::Entry>)> {
    options.validate()?;
    let cfg = saved_config(state, &request.connection_id)?;
    if db::runtime::prepare_at(&cfg, state.store.data_root())?.http.is_some() {
        return Err(AppError::coded("STREAM_ADAPTER_UNSUPPORTED", "HTTP 隧道尚未认证首屏流式返回；没有发送 SQL"));
    }
    if !matches!(cfg.kind.protocol(), Protocol::Mysql | Protocol::Postgres | Protocol::Sqlite)
        || db::explicit_legacy_binding(&cfg.params) {
        return Err(AppError::coded("STREAM_ADAPTER_UNSUPPORTED", "当前适配器尚未认证首屏流式返回；请使用原查询接口，没有发送 SQL"));
    }
    let registered = register_execution(state, request, principal)?;
    registered.2.attach_stream(crate::execution_stream::StreamProgress::new(options));
    Ok(registered)
}

#[tauri::command]
pub async fn execute_sql_streaming(
    app: tauri::AppHandle, window: tauri::WebviewWindow, state: State<'_, AppState>,
    request: crate::execution_contracts::ExecutionRequest, options: crate::execution_stream::StreamOptions,
) -> CmdResult<crate::execution_stream::StreamSnapshot> {
    use tauri::Manager;
    let principal = window.label().to_owned();
    let registered = register_streaming_execution(&state, &request, &principal, options)?;
    let entry = registered.2.clone();
    let progress = entry.stream().expect("stream registered");
    crate::execution_stream::launch(entry.clone(), async move {
        let state = app.state::<AppState>();
        execute_registered_sql_service(&state, request, &principal, registered).await
    });
    // Dropping this IPC waiter does NOT drop/abort the owning database worker.
    Ok(progress.wait_first(&entry).await)
}

pub(crate) fn stream_snapshot_service(
    state: &AppState, principal: &str, execution_id: &str, connection_id: &str, owner_id: &str, namespace: &str,
) -> CmdResult<crate::execution_stream::StreamSnapshot> {
    if uuid::Uuid::parse_str(execution_id).is_err() || connection_id.is_empty() || connection_id.len() > 512
        || owner_id.is_empty() || owner_id.len() > 256 || namespace.len() > 1024 || namespace.contains('\0') {
        return Err(AppError::coded("EXECUTION_CONTEXT_INVALID", "流式快照上下文无效"));
    }
    let cfg = saved_config(state, connection_id)?;
    let binding = execution_binding(state, &cfg, principal, owner_id, namespace)?;
    let entry = state.executions.authorized(execution_id, &binding)?;
    let progress = entry.stream().ok_or_else(|| AppError::coded("EXECUTION_NOT_STREAMING", "此执行未使用流式接口"))?;
    Ok(progress.snapshot(&entry))
}

#[tauri::command]
pub fn execution_stream_snapshot(
    window: tauri::WebviewWindow, state: State<'_, AppState>,
    execution_id: String, connection_id: String, owner_id: String, namespace: String,
) -> CmdResult<crate::execution_stream::StreamSnapshot> {
    stream_snapshot_service(&state, window.label(), &execution_id, &connection_id, &owner_id, &namespace)
}

#[tauri::command]
pub fn cancel_execution(window: tauri::WebviewWindow, state: State<'_, AppState>, execution_id: String, connection_id: String, owner_id: String, namespace: String) -> CmdResult<crate::execution_contracts::CancelAcknowledgement> {
    let cfg = saved_config(&state, &connection_id)?;
    let binding = execution_binding(&state, &cfg, window.label(), &owner_id, &namespace)?;
    let entry = state.executions.authorized(&execution_id, &binding)?;
    let conn = db::get_db(&state.registry, &connection_id).ok();
    if entry.status().state != crate::execution_contracts::ExecutionState::Queued && crate::execution::capabilities(conn.as_deref(), &cfg.kind, cfg.read_only, !db::explicit_legacy_binding(&cfg.params)).cancel.level == "unsupported" {
        return Err(AppError::coded("CANCEL_UNSUPPORTED", "该适配器没有已验证的独立取消通道"));
    }
    Ok(entry.request_stop(false))
}
#[tauri::command]
pub fn execution_status(window: tauri::WebviewWindow, state: State<'_, AppState>, execution_id: String, connection_id: String, owner_id: String, namespace: String) -> CmdResult<crate::execution_contracts::ExecutionStatus> {
    let cfg = saved_config(&state, &connection_id)?;
    Ok(state.executions.authorized(&execution_id, &execution_binding(&state, &cfg, window.label(), &owner_id, &namespace)?)?.status())
}
#[tauri::command]
pub async fn execution_capabilities(state: State<'_, AppState>, connection_id: String) -> CmdResult<crate::execution_contracts::Capabilities> {
    let cfg = saved_config(&state, &connection_id)?;
    // 读取能力不主动连接数据库。
    let conn = db::get_db(&state.registry, &connection_id).ok();
    Ok(crate::execution::capabilities(conn.as_deref(), &cfg.kind, cfg.read_only, !db::explicit_legacy_binding(&cfg.params)))
}

/// 读取 SQL 执行日志（最新优先；默认最多 2000 条，30 天内的记录）。
#[tauri::command]
pub fn list_sql_log(limit: Option<u16>) -> Vec<crate::models::SqlLogEntry> {
    crate::sql_history::flush();
    crate::storage::read_sql_log(limit.unwrap_or(crate::storage::SQL_LOG_MAX_ENTRIES as u16) as usize)
}

/// 清空 SQL 执行日志。
#[tauri::command]
pub fn clear_sql_log(state: State<'_, AppState>) -> Result<(), String> {
    let _activity_guard = state.activity.enter().map_err(|e| e.to_string())?;
    crate::storage::log_line("sql-log", "用户清空了 SQL 执行日志");
    crate::sql_history::flush();
    crate::storage::clear_sql_log();
    Ok(())
}

#[tauri::command]
pub async fn manual_transaction_start(window: tauri::WebviewWindow, state: State<'_, AppState>, id: String, schema: String, owner: String) -> CmdResult<crate::manual_transaction::ManualTransactionStatus> {
    let _activity_guard = state.activity.enter()?;
    start_manual_transaction(&state, &id, &schema, &owner, window.label()).await
}

async fn start_manual_transaction(state: &AppState, id: &str, schema: &str, owner: &str, principal: &str) -> CmdResult<crate::manual_transaction::ManualTransactionStatus> {
    state.executions.require_quiescent(id)?;
    let mut lease = state.manual_transactions.lease(id).await;
    lease.require_inactive()?;
    require_writable_connection(state, id)?;
    let cfg = saved_config(state, id)?;
    if !matches!(cfg.kind.protocol(), Protocol::Sqlite | Protocol::Mysql | Protocol::Postgres) || cfg.params.starts_with(crate::driver_manager::PARAMS_PREFIX) {
        return Err(AppError::new("手动查询事务仅支持 SQLx 原生 SQLite / MySQL / PostgreSQL，当前驱动不支持"));
    }
    let (_, conn) = query_db(state, id, Some(schema)).await?;
    let status = lease.start(conn.as_ref(), cfg.kind, schema.into(), owner.into()).await?;
    lease.bind_principal(principal);
    Ok(status)
}

#[tauri::command]
pub async fn manual_transaction_status(state: State<'_, AppState>, id: String) -> CmdResult<crate::manual_transaction::ManualTransactionStatus> {
    let _activity_guard = state.activity.enter()?;
    saved_config(&state, &id)?;
    Ok(state.manual_transactions.lease(&id).await.status())
}

#[tauri::command]
pub async fn manual_transaction_finish(window: tauri::WebviewWindow, state: State<'_, AppState>, id: String, owner: String, commit: bool) -> CmdResult<crate::manual_transaction::ManualTransactionStatus> {
    let _activity_guard = state.activity.enter()?;
    let mut lease = state.manual_transactions.lease(&id).await;
    lease.require_principal(window.label())?;
    if commit { require_writable_connection(&state, &id)?; }
    lease.finish(&owner, commit).await.map_err(AppError::from)
}

/// 手动事务内执行图形化行变更（表数据页「开始事务」后的可视化编辑提交）。
/// 全部变更在同一事务连接上执行：任一语句失败则事务标记失败，只能回滚。
#[tauri::command]
pub async fn manual_transaction_mutate(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    id: String,
    owner: String,
    schema: String,
    ops: Vec<crate::manual_transaction::ManualMutationOp>,
) -> CmdResult<crate::models::MultiResult> {
    let _activity_guard = state.activity.enter()?;
    let cfg = saved_config(&state, &id)?;
    if cfg.read_only {
        return Err(AppError::new("只读连接不能在事务中执行写入"));
    }
    let mut lease = state.manual_transactions.lease(&id).await;
    lease.require_principal(window.label())?;
    let kind = lease.db_kind().ok_or_else(|| AppError::new("手动事务已经结束，请先重新开始事务"))?;
    let status = lease.status();
    if !status.active {
        return Err(AppError::new("手动事务已经结束，请先重新开始事务"));
    }
    // SQLite 空命名空间等价 main（与 manual_transaction 内部一致，BUG-R1-014）
    let same_namespace = status.schema == schema
        || (kind.protocol() == crate::models::Protocol::Sqlite
            && matches!((schema.as_str(), status.schema.as_str()), ("" | "main", "main") | ("main", "")));
    if status.owner != owner || !same_namespace {
        return Err(AppError::new("事务由其它页面或模式持有，请返回事务所属页面操作"));
    }
    if status.failed {
        return Err(AppError::new("手动事务已失败，只能回滚，不能继续执行"));
    }
    let statements = crate::manual_transaction::build_manual_mutation_sql(&kind, &schema, &ops)
        .map_err(AppError::from)?;
    let sql = statements.join(";\n");
    match lease.execute(Some(&owner), &schema, &sql).await {
        Ok(Some(result)) => Ok(result),
        Ok(None) => Err(AppError::new("手动事务已经结束，没有发送变更")),
        Err(error) => Err(AppError::from(error)),
    }
}

/// 更新一行。
#[tauri::command]
pub async fn update_row(state: State<'_, AppState>, req: RowUpdate) -> CmdResult<u64> {
    let _activity_guard = state.activity.enter()?;
    let _manual_guard = state.manual_transactions.ordinary(&[&req.connection_id]).await?;
    let id = req.connection_id.clone();
    require_writable_connection(&state, &id)?;
    let db = scoped_db(&state, &id, &req.schema).await?;
    schema::update_row(db.as_ref(), kind_of(&state, &id)?, &req).await.map_err(AppError::from)
}

/// 插入一行。
#[tauri::command]
pub async fn insert_row(state: State<'_, AppState>, req: RowInsert) -> CmdResult<u64> {
    let _activity_guard = state.activity.enter()?;
    let _manual_guard = state.manual_transactions.ordinary(&[&req.connection_id]).await?;
    let id = req.connection_id.clone();
    require_writable_connection(&state, &id)?;
    let db = scoped_db(&state, &id, &req.schema).await?;
    schema::insert_row(db.as_ref(), kind_of(&state, &id)?, &req).await.map_err(AppError::from)
}

/// 删除一行。
#[tauri::command]
pub async fn delete_row(
    state: State<'_, AppState>,
    id: String,
    schema_name: String,
    table: String,
    primary_keys: Vec<(String, Option<String>)>,
) -> CmdResult<u64> {
    let _activity_guard = state.activity.enter()?;
    let _manual_guard = state.manual_transactions.ordinary(&[&id]).await?;
    require_writable_connection(&state, &id)?;
    let db = scoped_db(&state, &id, &schema_name).await?;
    schema::delete_row(
        db.as_ref(),
        kind_of(&state, &id)?,
        &schema_name,
        &table,
        &primary_keys,
    )
    .await
    .map_err(AppError::from)
}

/// 读取表上的检查约束与触发器（只读）。
#[tauri::command]
pub async fn list_table_constraints(
    state: State<'_, AppState>,
    id: String,
    schema: String,
    table: String,
) -> CmdResult<crate::table_objects::ConstraintSet> {
    let _activity_guard = state.activity.enter()?;
    let db = scoped_db(&state, &id, &schema).await?;
    let kind = kind_of(&state, &id)?;
    crate::table_objects::list(db.as_ref(), &kind, &schema, &table)
        .await
        .map_err(AppError::from)
}

/// 删除一个触发器；受只读与事务门禁保护。
#[tauri::command]
pub async fn drop_table_trigger(
    state: State<'_, AppState>,
    id: String,
    schema: String,
    table: String,
    name: String,
) -> CmdResult<u64> {
    let _activity_guard = state.activity.enter()?;
    let _manual_guard = state.manual_transactions.ordinary(&[&id]).await?;
    require_writable_connection(&state, &id)?;
    let kind = kind_of(&state, &id)?;
    let db = scoped_db(&state, &id, &schema).await?;
    crate::table_objects::drop_trigger(db.as_ref(), &kind, &schema, &table, &name)
        .await
        .map_err(AppError::from)
}

/// 预览结构差异同步：只读比对源/目标结构，不执行任何 DDL。
#[tauri::command]
pub async fn preview_structure_sync(
    state: State<'_, AppState>,
    request: crate::structure_sync::StructureSyncRequest,
) -> CmdResult<crate::structure_sync::StructureDiffReport> {
    let _activity_guard = state.activity.enter()?;
    let source_kind = kind_of(&state, &request.source_connection)?;
    let target_kind = kind_of(&state, &request.target_connection)?;
    if !crate::sync::supported(&source_kind) || !crate::sync::supported(&target_kind) {
        return Err(AppError::new("结构同步仅支持 SQLite、MySQL/PostgreSQL 协议族等关系型数据源"));
    }
    let source = scoped_db(&state, &request.source_connection, &request.source_schema).await?;
    let target = scoped_db(&state, &request.target_connection, &request.target_schema).await?;
    crate::structure_sync::preview(source.as_ref(), &source_kind, target.as_ref(), &target_kind, &request)
        .await
        .map_err(AppError::from)
}

/// 应用结构差异：按预览结果生成 ALTER，执行后读回校验。
#[tauri::command]
pub async fn apply_structure_sync(
    state: State<'_, AppState>,
    request: crate::structure_sync::StructureSyncRequest,
) -> CmdResult<crate::structure_sync::StructureSyncReceipt> {
    let _activity_guard = state.activity.enter()?;
    let _manual_guard = state
        .manual_transactions
        .ordinary(&[&request.source_connection, &request.target_connection])
        .await?;
    require_writable_connection(&state, &request.target_connection)?;
    let source_kind = kind_of(&state, &request.source_connection)?;
    let target_kind = kind_of(&state, &request.target_connection)?;
    if !crate::sync::supported(&source_kind) || !crate::sync::supported(&target_kind) {
        return Err(AppError::new("结构同步仅支持 SQLite、MySQL/PostgreSQL 协议族等关系型数据源"));
    }
    let source = scoped_db(&state, &request.source_connection, &request.source_schema).await?;
    let target = scoped_db(&state, &request.target_connection, &request.target_schema).await?;
    crate::structure_sync::apply(source.as_ref(), &source_kind, target.as_ref(), &target_kind, &request)
        .await
        .map_err(AppError::from)
}

/// 预览图形化行写入计划；不执行任何写操作。
#[tauri::command]
pub async fn preview_row_mutation(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    draft: crate::mutation::MutationDraft,
) -> CmdResult<crate::mutation::MutationPlan> {
    let _activity_guard = state.activity.enter()?;
    let id = draft.connection_id.clone();
    let _manual_guard = state.manual_transactions.ordinary(&[&id]).await?;
    require_writable_connection(&state, &id)?;
    let cfg = saved_config(&state, &id)?;
    let kind = cfg.kind.clone();
    let binding = execution_binding(&state, &cfg, window.label(), &draft.owner_id, &draft.schema)?;
    let db = scoped_db(&state, &id, &draft.schema).await?;
    let physical_key = self::physical_key(&id, &draft.schema, db.as_ref());
    let scoped_cfg = saved_config(&state, &id)?;
    if scoped_cfg.read_only { return Err(AppError::coded("READ_ONLY_VIOLATION", "目标连接已变为只读，没有生成写入计划")); }
    if execution_binding(&state, &scoped_cfg, window.label(), &draft.owner_id, &draft.schema)? != binding
        || !state.connection_runtime.matches(&physical_key, &binding.fingerprint, &db) {
        return Err(AppError::coded("MUTATION_CONTEXT_CHANGED", "当前实际物理连接与保存端点/凭据/权限绑定不一致，没有生成计划"));
    }
    // Dispatch on the actual physical adapter, never an application kind label.
    let plan = crate::mutation::prepare_plan(db.as_ref(), &kind, &draft, window.label(), binding.fingerprint.clone())
        .await.map_err(AppError::from)?;
    let final_cfg = saved_config(&state, &id)?;
    if final_cfg.read_only { return Err(AppError::coded("READ_ONLY_VIOLATION", "读取元数据期间目标变为只读，没有生成计划")); }
    if execution_binding(&state, &final_cfg, window.label(), &draft.owner_id, &draft.schema)? != binding
        || !state.connection_runtime.matches(&physical_key, &binding.fingerprint, &db) {
        return Err(AppError::coded("MUTATION_CONTEXT_CHANGED", "预览期间实际端点或物理连接已改变，没有生成计划"));
    }
    let sql = plan.sql.clone();
    let binds = plan.binds.clone();
    let expected_rows = plan.expected_rows;
    let revision = plan.base_revision.clone();
    let plan_id = state.mutations.put(plan).map_err(AppError::from)?;
    Ok(crate::mutation::MutationPlan {
        plan_id, sql, binds, base_revision: revision, expected_rows,
        summary: format!("{} {} 的 1 行；冲突与读回失败均在事务内回滚", draft.kind, draft.table),
    })
}

/// 一次性消费属于当前窗口/页面的计划；端点或结构变化时不发送 DML。
#[tauri::command]
pub async fn execute_row_mutation(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    plan_id: String,
    owner_id: String,
) -> CmdResult<crate::mutation::MutationReceipt> {
    execute_row_mutation_service(&state, window.label(), &plan_id, &owner_id).await
}

pub(crate) async fn execute_row_mutation_service(state: &AppState, principal: &str, plan_id: &str, owner_id: &str) -> CmdResult<crate::mutation::MutationReceipt> {
    let _activity_guard = state.activity.enter()?;
    let plan = state.mutations.take(plan_id, principal, owner_id)
        .map_err(|e| AppError::coded("MUTATION_PLAN_REJECTED", e.to_string()))?;
    let id = plan.connection_id.clone();
    let _manual_guard = state.manual_transactions.ordinary(&[&id]).await?;
    require_writable_connection(&state, &id)?;
    let cfg = saved_config(&state, &id)?;
    let binding = execution_binding(&state, &cfg, principal, owner_id, &plan.schema)?;
    if binding.fingerprint != plan.endpoint_fingerprint {
        return Err(AppError::coded("MUTATION_CONTEXT_CHANGED", "计划绑定的端点/凭据/权限已变化，没有发送 DML"));
    }
    let kind = cfg.kind.clone();
    let db = scoped_db(&state, &id, &plan.schema).await?;
    let physical_key = self::physical_key(&id, &plan.schema, db.as_ref());
    let scoped_cfg = saved_config(&state, &id)?;
    if scoped_cfg.read_only { return Err(AppError::coded("READ_ONLY_VIOLATION", "目标连接已变为只读，没有发送 DML")); }
    if execution_binding(&state, &scoped_cfg, principal, owner_id, &plan.schema)? != binding
        || !state.connection_runtime.matches(&physical_key, &binding.fingerprint, &db) {
        return Err(AppError::coded("MUTATION_CONTEXT_CHANGED", "排队期间实际物理连接或保存端点已变化，没有发送 DML"));
    }
    let _query_write = crate::query_commands::pin_plan(&state, &plan, principal, owner_id)?;
    crate::mutation::validate_plan_context(db.as_ref(), &plan).await.map_err(AppError::from)?;
    let final_cfg = saved_config(&state, &id)?;
    if final_cfg.read_only { return Err(AppError::coded("READ_ONLY_VIOLATION", "校验结构期间目标已变为只读，没有发送 DML")); }
    if execution_binding(&state, &final_cfg, principal, owner_id, &plan.schema)? != binding
        || !state.connection_runtime.matches(&physical_key, &binding.fingerprint, &db) {
        return Err(AppError::coded("MUTATION_CONTEXT_CHANGED", "校验期间实际端点或物理连接已变化，没有发送 DML"));
    }
    let settings = state.store.settings();
    settings.validate_execution_settings()?;
    // Keep activity, ordinary lease and the page write guard in the task that actually
    // executes and drains the transaction. An abandoned IPC waiter cannot release them.
    let capture = plan.query_source.as_ref().and_then(|source| {
        let source_binding = crate::execution::Binding {
            principal: source.binding.principal.clone(), owner: source.binding.owner_id.clone(),
            connection: source.binding.connection_id.clone(), fingerprint: source.binding.endpoint_fingerprint.clone(),
            namespace: source.execution_namespace.clone(),
        };
        state.executions.authorized(&source.binding.source_execution_id, &source_binding).ok()?.query_capture()
    });
    if plan.query_source.is_some() && capture.is_none() {
        return Err(AppError::coded("QUERY_CONTEXT_CHANGED", "原查询证据已释放，没有发送 DML"));
    }
    let plan_id = plan_id.to_owned();
    owned_tool_task(Box::pin(async move {
        let result = crate::mutation::execute_plan(db.as_ref(), &kind, &plan_id, &plan, final_cfg.read_only, settings.query_timeout_secs).await;
        // Invalidate even when the original waiter was lost or commit acknowledgement is unknown.
        if let Some(capture) = capture { capture.release(); }
        result
    }), (_activity_guard, _manual_guard, _query_write)).await
}

/// 保存表结构设计：后端编译方言 SQL、执行并读回比对。
///
/// 前端只提交目标结构，不提交 SQL；不支持的能力返回明确错误而不是假成功。
#[tauri::command]
pub async fn apply_table_design(
    state: State<'_, AppState>,
    id: String,
    design: crate::design::TableDesign,
) -> CmdResult<crate::design::DesignReceipt> {
    let _activity_guard = state.activity.enter()?;
    let _manual_guard = state.manual_transactions.ordinary(&[&id]).await?;
    require_writable_connection(&state, &id)?;
    let db = scoped_db(&state, &id, &design.schema).await?;
    let kind = kind_of(&state, &id)?;
    crate::design::apply(db.as_ref(), &kind, &design).await.map_err(AppError::from)
}

/// 连接保活：空闲时执行一次无副作用探测，返回健康状态（PL-27）。
#[tauri::command]
pub async fn check_connection_health(window: tauri::WebviewWindow, state: State<'_, AppState>, id: String, force: Option<bool>) -> CmdResult<crate::connection_health::ConnectionHealthView> {
    let _ = force; // 用户探测也遵守已保存策略和间隔，不绕过关闭或禁用状态。
    crate::runtime_commands::health(&state, &id, window.label(), "connection-health").await
}

#[tauri::command]
pub fn get_connection_health(state: State<'_, AppState>, id: String) -> CmdResult<crate::connection_health::ConnectionHealthView> {
    let _activity = state.activity.enter()?;
    let cfg = saved_config(&state, &id)?;
    let policy = crate::db::runtime::keepalive_policy(&cfg)?;
    let reg = &state.connection_runtime.health;
    if let Some(generation) = state.connection_runtime.generation(&id) {
        return reg.view_generation(&generation).map_err(AppError::from);
    }
    Ok(reg.view(&id, policy.interval_secs))
}

/// 执行 DDL（表结构修改）。
#[tauri::command]
pub async fn execute_ddl(
    state: State<'_, AppState>,
    id: String,
    sql: String,
    schema_name: Option<String>,
    replace_view: Option<String>,
) -> CmdResult<u64> {
    let _activity_guard = state.activity.enter()?;
    let _manual_guard = state.manual_transactions.ordinary(&[&id]).await?;
    require_writable_connection(&state, &id)?;
    let kind = kind_of(&state, &id)?;
    let db = scoped_db(&state, &id, schema_name.as_deref().unwrap_or("")).await?;
    if let Some(view) = replace_view {
        if let db::Db::Sqlite(pool) = db.as_ref() {
            return replace_sqlite_view(pool, schema_name.as_deref().unwrap_or("main"), &view, &sql).await;
        }
        return Err(AppError::new("原子视图重建仅用于原生 SQLite"));
    }
    schema::exec_affected(db.as_ref(), &kind, &sql).await.map_err(AppError::from)
}

async fn replace_sqlite_view(pool: &sqlx::SqlitePool, namespace: &str, view: &str, sql: &str) -> CmdResult<u64> {
    if !["", "main"].contains(&namespace) { return Err(AppError::new("SQLite 视图设计仅支持 main 数据库")); }
    let quote = |name: &str| format!("\"{}\"", name.replace('"', "\"\""));
    let mut transaction = pool.begin().await?;
    crate::sql_history::query(&format!("DROP VIEW {}.{}", quote("main"), quote(view))).execute(&mut *transaction).await?;
    let result = crate::sql_history::query(sql).execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok(result.rows_affected())
}

#[cfg(test)]
mod object_browser_tests {
    use super::*;
    #[tokio::test]
    async fn sqlite_view_replacement_is_atomic_and_quoted() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        crate::sql_history::query("CREATE VIEW \"v\"\"name\" AS SELECT 1 AS value").execute(&pool).await.unwrap();
        assert!(replace_sqlite_view(&pool, "main", "v\"name", "CREATE VIEW invalid syntax").await.is_err());
        assert_eq!(crate::sql_history::query_scalar::<_, i64>("SELECT value FROM \"v\"\"name\"").fetch_one(&pool).await.unwrap(), 1);
        replace_sqlite_view(&pool, "main", "v\"name", "CREATE VIEW \"v\"\"name\" AS SELECT 2 AS value").await.unwrap();
        assert_eq!(crate::sql_history::query_scalar::<_, i64>("SELECT value FROM \"v\"\"name\"").fetch_one(&pool).await.unwrap(), 2);
    }
}

// ---------------------------------------------------------------- 收藏 / 最近

#[tauri::command]
pub fn list_favorites(state: State<'_, AppState>) -> CmdResult<Vec<Favorite>> {
    state.store.checked_favorites().map_err(AppError::new)
}

#[tauri::command]
pub fn add_favorite(state: State<'_, AppState>, favorite: Favorite) -> CmdResult<Vec<Favorite>> {
    state.store.add_favorite(favorite).map_err(AppError::new)
}

#[tauri::command]
pub fn remove_favorite(state: State<'_, AppState>, id: String) -> CmdResult<Vec<Favorite>> {
    state.store.remove_favorite(&id).map_err(AppError::new)
}

#[tauri::command]
pub fn list_recent(state: State<'_, AppState>) -> Vec<RecentItem> {
    state.store.recent()
}

#[tauri::command]
pub fn push_recent(state: State<'_, AppState>, item: RecentItem) -> CmdResult<Vec<RecentItem>> {
    let max = state.store.settings().max_recent;
    state.store.push_recent(item, max);
    Ok(state.store.recent())
}

#[tauri::command]
pub fn clear_recent(state: State<'_, AppState>) -> CmdResult<Vec<RecentItem>> {
    state.store.clear_recent();
    Ok(state.store.recent())
}

// ---------------------------------------------------------------- 导入导出

#[tauri::command]
pub async fn run_import(
    state: State<'_, AppState>,
    opts: ImportOptions,
) -> CmdResult<u64> {
    let _activity_guard = state.activity.enter()?;
    let _manual_guard = state.manual_transactions.ordinary(&[&opts.connection_id]).await?;
    require_writable_connection(&state, &opts.connection_id)?;
    if opts.format.eq_ignore_ascii_case("csv") { crate::transfer::validate_delimiter(&opts.delimiter)?; }
    let id = opts.connection_id.clone();
    let kind = kind_of(&state, &id)?;
    let (_, db) = query_db(&state, &id, Some(&opts.schema)).await?;
    validate_table_target(db.as_ref(), &kind, &opts.schema, &opts.table, opts.create_table).await?;
    match opts.format.to_ascii_lowercase().as_str() {
        "csv" => crate::transfer::import_csv(db.as_ref(), kind, &opts)
            .await
            .map_err(AppError::from),
        "json" => crate::transfer::import_json(db.as_ref(), kind, &opts)
            .await
            .map_err(AppError::from),
        "sql" => crate::transfer::import_sql(db.as_ref(), kind, &opts)
            .await
            .map_err(AppError::from),
        other => Err(AppError::new(format!("不支持的导入格式: {}", other))),
    }
}

#[tauri::command]
pub async fn run_export(
    state: State<'_, AppState>,
    opts: ExportOptions,
) -> CmdResult<u64> {
    let _activity_guard = state.activity.enter()?;
    crate::transfer::validate_export(&opts)?;
    let id = opts.connection_id.clone();
    let kind = kind_of(&state, &id)?;
    let db = scoped_db(&state, &id, &opts.schema).await?;
    protect_export_path(&state, &opts.file, db.as_ref()).await?;
    // 与交互导出同一条流式管线：游标读取 + 有界写入 + 原子发布，避免整表物化。
    crate::transfer::export_table_streaming(db.as_ref(), &kind, &opts)
        .await
        .map_err(AppError::from)
}

/// 导出查询结果（结果网格导出）。
#[tauri::command]
pub async fn export_query_result(
    state: State<'_, AppState>,
    opts: ExportOptions,
    names: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
) -> CmdResult<u64> {
    let _activity_guard = state.activity.enter()?;
    let kind = kind_of(&state, &opts.connection_id)?;
    crate::transfer::validate_export(&opts)?;
    crate::transfer::validate_names(&names)?;
    if rows.iter().any(|r| r.len() != names.len()) { return Err(AppError::new("结果列数与数据行不一致")); }
    let conn = ensure_db(&state, &opts.connection_id).await?;
    protect_export_path(&state, &opts.file, conn.as_ref()).await?;
    match opts.format.to_ascii_lowercase().as_str() {
        "csv" => crate::transfer::export_csv(
            &opts.file,
            &names,
            &rows,
            &opts.delimiter,
            &opts.encoding,
            opts.has_header,
        )
        .map_err(AppError::from),
        "json" => crate::transfer::export_json(&opts.file, &names, &rows, &opts.encoding)
            .map_err(AppError::from),
        "sql" => crate::transfer::export_sql(
            &opts.file,
            kind,
            &opts.schema,
            &opts.table,
            &names,
            &rows,
            &opts.encoding,
        )
        .map_err(AppError::from),
        other => Err(AppError::new(format!("不支持的导出格式: {}", other))),
    }
}

/// 分页结果集的缓存页。
#[derive(serde::Serialize)]
pub struct SpoolPage {
    pub rows: Vec<Vec<Option<String>>>,
    pub total: u64,
    pub truncated: bool,
    pub message: Option<String>,
}

/// 读取一次执行已缓存的结果行（「加载更多」）。不发送任何 SQL，也不重跑查询。
#[tauri::command]
pub async fn fetch_query_rows(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    spool_id: String,
    connection_id: String,
    owner_id: String,
    namespace: String,
    index: usize,
    offset: u64,
    limit: u64,
) -> CmdResult<SpoolPage> {
    let _activity_guard = state.activity.enter()?;
    let spool = crate::query_spool::authorize(&spool_id, &connection_id, &owner_id, &namespace, window.label())?;
    let set = spool.set(index)?;
    tokio::task::spawn_blocking(move || {
        let rows = set.fetch(offset, limit.clamp(1, 5_000))?;
        Ok(SpoolPage { rows, total: set.total(), truncated: set.truncated(), message: set.stop_reason() })
    }).await.map_err(|e| AppError::new(format!("读取结果任务意外结束：{e}")))?
}

/// 把一次执行的完整结果导出为文件；读的是执行时缓存的行，不会重跑用户 SQL。
#[tauri::command]
pub async fn export_query_spooled(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    opts: ExportOptions,
    spool_id: String,
    owner_id: String,
    namespace: String,
    index: usize,
) -> CmdResult<u64> {
    export_query_spooled_service(&state, opts, &spool_id, &owner_id, &namespace, index, window.label()).await
}

pub(crate) async fn export_query_spooled_service(
    state: &AppState, opts: ExportOptions, spool_id: &str, owner_id: &str,
    namespace: &str, index: usize, principal: &str,
) -> CmdResult<u64> {
    let _activity_guard = state.activity.enter()?;
    let kind = state.store.connections().into_iter().find(|c| c.id == opts.connection_id)
        .map(|c| c.kind)
        .ok_or_else(|| AppError::coded("CONNECTION_NOT_FOUND", "连接不存在"))?;
    crate::transfer::validate_export(&opts)?;
    let spool = crate::query_spool::authorize(spool_id, &opts.connection_id, owner_id, namespace, principal)?;
    let set = spool.set(index)?;
    let names = set.names();
    crate::transfer::validate_names(&names)?;
    // 缓存导出离线可用；校验保存的数据库路径即可，不建立数据库连接或再次执行查询。
    protect_saved_export_path(state, &opts.file)?;
    let types = set.types();
    let result = tokio::task::spawn_blocking(move || {
        crate::transfer::export_spool_set(&set, &opts, &kind, &names, &types)
    })
    .await
    .map_err(|e| AppError::new(format!("导出任务意外结束：{e}")))?;
    result.map_err(AppError::from)
}

/// 释放结果页不再使用的快照；重复释放无副作用。
#[tauri::command]
pub fn drop_query_spool(
    window: tauri::WebviewWindow, state: State<'_, AppState>, spool_id: String,
    connection_id: String, owner_id: String, namespace: String,
) -> CmdResult<()> {
    let _activity_guard = state.activity.enter()?;
    if crate::query_spool::exists(&spool_id) {
        crate::query_spool::authorize(&spool_id, &connection_id, &owner_id, &namespace, window.label())?;
        crate::query_spool::drop_spool(&spool_id);
    }
    Ok(())
}

/// 异步导入任务；opts 的基础导入字段通过 serde(flatten) 平铺。
#[tauri::command]
pub async fn run_import_task(
    state: State<'_, AppState>,
    opts: crate::transfer::ImportTaskOptions,
) -> CmdResult<u64> {
    let _activity_guard = state.activity.enter()?;
    let _manual_guard = state.manual_transactions.ordinary(&[&opts.options.connection_id]).await?;
    let o = &opts.options;
    require_writable_connection(&state, &o.connection_id)?;
    let kind = kind_of(&state, &o.connection_id)?;
    let (_, conn) = query_db(&state, &o.connection_id, Some(&o.schema)).await?;
    validate_table_target(conn.as_ref(), &kind, &o.schema, &o.table, o.create_table).await?;
    crate::transfer::run_import_task(conn.as_ref(), &kind, &opts).await.map_err(AppError::from)
}

#[tauri::command]
pub async fn run_export_task(
    state: State<'_, AppState>,
    opts: crate::transfer::ExportTaskOptions,
) -> CmdResult<u64> {
    let _activity_guard = state.activity.enter()?;
    let o = &opts.options;
    let kind = kind_of(&state, &o.connection_id)?;
    let conn = scoped_db(&state, &o.connection_id, &o.schema).await?;
    protect_export_path(&state, &o.file, conn.as_ref()).await?;
    crate::transfer::run_export_task(conn.as_ref(), &kind, &opts).await.map_err(AppError::from)
}

#[tauri::command]
pub fn transfer_task_progress(task_id: String) -> CmdResult<crate::transfer::TaskProgress> {
    crate::transfer::task_progress(&task_id).map_err(AppError::from)
}

#[tauri::command]
pub fn cancel_transfer_task(task_id: String) -> CmdResult<()> {
    crate::transfer::cancel_task(&task_id).map_err(AppError::from)
}

#[cfg(test)]
#[path = "owned_operation_tests.rs"]
mod owned_operation_tests;

/// Once an action owns its connection leases, losing the IPC waiter must not drop
/// an in-flight database transaction or release its activity gates ahead of drain.
async fn owned_tool_task<T, F, G>(work: F, guards: G) -> CmdResult<T>
where
    T: Send + 'static,
    F: std::future::Future<Output = anyhow::Result<T>> + Send + 'static,
    G: Send + 'static,
{
    let (send, receive) = tokio::sync::oneshot::channel();
    crate::sql_history::spawn(async move {
        let _guards = guards;
        let result = work.await.map_err(AppError::from);
        drop(_guards);
        let _ = send.send(result);
    });
    receive.await.map_err(|_| AppError::coded("TOOL_EFFECTS_UNKNOWN", "工具执行任务失联；请保留任务证据并核对实际结果，禁止自动重放"))?
}

#[tauri::command]
pub async fn run_sync(
    state: State<'_, AppState>,
    opts: crate::sync::SyncOptions,
) -> CmdResult<crate::sync::SyncReport> {
    if opts.delete_missing { return Err(AppError::coded("SYNC_DELETE_REVIEW_REQUIRED", "删除目标多余行需要完整范围与已确认的预览；未访问数据库")); }
    let activity_guard = state.activity.enter()?;
    let manual_guard = state.manual_transactions.ordinary(&[&opts.source_connection, &opts.target_connection]).await?;
    require_writable_connection(&state, &opts.target_connection)?;
    let source_kind = kind_of(&state, &opts.source_connection)?;
    let target_kind = kind_of(&state, &opts.target_connection)?;
    if !crate::sync::supported(&source_kind) || !crate::sync::supported(&target_kind) {
        return Err(AppError::new("同步支持 SQLite、MySQL/PostgreSQL 协议族、SQL Server、ClickHouse 与 ODBC 关系型数据源；Redis、Elasticsearch、Kafka 等非关系型不支持"));
    }
    crate::transfer::validate_names(&[opts.source_table.clone()])?;
    crate::transfer::validate_where(opts.where_clause.as_deref())?;
    let source = scoped_db(&state, &opts.source_connection, &opts.source_schema).await?;
    let target = scoped_db(&state, &opts.target_connection, &opts.target_schema).await?;
    validate_table_target(target.as_ref(), &target_kind, &opts.target_schema, &opts.target_table,
        opts.create_target && opts.mode != "data").await?;
    owned_tool_task(Box::pin(async move {
        crate::sync::synchronize(source.as_ref(), &source_kind, target.as_ref(), &target_kind, &opts).await
    }), (activity_guard, manual_guard)).await
}

/// 比较源表与目标表的差异（行数/结构/按主键的缺失、多出与内容差异）。只读，不修改数据。
#[tauri::command]
pub async fn sync_diff(
    state: State<'_, AppState>,
    req: crate::sync::SyncDiffRequest,
    bounded: Option<bool>,
    memory: Option<crate::sync::DiffMemoryOptions>,
) -> CmdResult<crate::sync::SyncDiffReport> {
    let _activity_guard = state.activity.enter()?;
    if memory.is_some() && bounded.unwrap_or(false) {
        return Err(AppError::new("MEMORY_BUDGET_INVALID: memory与bounded=true互斥，请仅选择一种比较策略"));
    }
    let source_kind = kind_of(&state, &req.source_connection)?;
    let target_kind = kind_of(&state, &req.target_connection)?;
    if !crate::sync::supported(&source_kind) || !crate::sync::supported(&target_kind) {
        return Err(AppError::new("差异比较仅支持 SQLite、MySQL/MariaDB 与 PostgreSQL 协议数据库"));
    }
    let source = scoped_db(&state, &req.source_connection, &req.source_schema).await?;
    let target = scoped_db(&state, &req.target_connection, &req.target_schema).await?;
    if let Some(memory) = memory {
        owned_tool_task(Box::pin(async move {
            crate::sync::diff_with_memory(source.as_ref(), &source_kind, target.as_ref(), &target_kind, &req, &memory).await
        }), _activity_guard).await
    } else if bounded.unwrap_or(false) {
        crate::sync::diff_bounded(source.as_ref(), &source_kind, target.as_ref(), &target_kind, &req).await.map_err(AppError::from)
    } else {
        crate::sync::diff(source.as_ref(), &source_kind, target.as_ref(), &target_kind, &req).await.map_err(AppError::from)
    }
}

/// Only OS resource sampling; execution revalidates and never trusts this preview.
#[tauri::command]
pub async fn preview_diff_memory(
    state: State<'_, AppState>,
    memory: Option<crate::sync::DiffMemoryOptions>,
) -> CmdResult<crate::sync::DiffMemoryPreview> {
    let _activity_guard = state.activity.enter()?;
    crate::sync::preview_diff_memory(memory).map_err(AppError::from)
}

/// 整库同步（库到库）：逐表独立事务，单表失败继续并在报告中记录。
#[tauri::command]
pub async fn run_sync_database(
    state: State<'_, AppState>,
    req: crate::sync::DbSyncRequest,
) -> CmdResult<Vec<crate::sync::SyncTableReport>> {
    let activity_guard = state.activity.enter()?;
    let manual_guard = state.manual_transactions.ordinary(&[&req.source_connection, &req.target_connection]).await?;
    require_writable_connection(&state, &req.target_connection)?;
    let source_kind = kind_of(&state, &req.source_connection)?;
    let target_kind = kind_of(&state, &req.target_connection)?;
    if !crate::sync::supported(&source_kind) || !crate::sync::supported(&target_kind) {
        return Err(AppError::new("同步支持 SQLite、MySQL/PostgreSQL 协议族、SQL Server、ClickHouse 与 ODBC 关系型数据源；Redis、Elasticsearch、Kafka 等非关系型不支持"));
    }
    if req.tables.is_empty() {
        return Err(AppError::new("请至少选择一张要同步的表"));
    }
    crate::transfer::validate_where(req.where_clause.as_deref())?;
    let source = scoped_db(&state, &req.source_connection, &req.source_schema).await?;
    let target = scoped_db(&state, &req.target_connection, &req.target_schema).await?;
    let work_request = req.clone();
    let result = owned_tool_task(Box::pin(async move {
        crate::sync::synchronize_database(source.as_ref(), &source_kind, target.as_ref(), &target_kind, &work_request).await
    }), (activity_guard, manual_guard)).await;
    match &result {
        Ok(reports) => {
            let ok = reports.iter().filter(|r| r.error.is_none() && !r.skipped).count();
            let skipped = reports.iter().filter(|r| r.skipped).count();
            let failed = reports.iter().filter(|r| r.error.is_some()).count();
            storage::log_line("data-sync", &format!(
                "整库同步 source={} schema={} target={} schema={} 完成 {} 表（跳过 {}，失败 {}）",
                req.source_connection.replace(['\n', '\r'], " "), req.source_schema.replace(['\n', '\r'], " "),
                req.target_connection.replace(['\n', '\r'], " "), req.target_schema.replace(['\n', '\r'], " "),
                ok, skipped, failed));
        }
        Err(_) => storage::log_line("data-sync", "整库同步未完成（详细错误只返回当前窗口）"),
    }
    result
}

#[tauri::command]
pub async fn sync_type_mapping(
    state: State<'_, AppState>,
    source_connection: String,
    source_schema: String,
    source_table: String,
    target_connection: String,
) -> CmdResult<Vec<crate::sync::SyncTypeMapping>> {
    let _activity_guard = state.activity.enter()?;
    let source_kind = kind_of(&state, &source_connection)?;
    let target_kind = kind_of(&state, &target_connection)?;
    crate::transfer::validate_names(&[source_table.clone()])?;
    let source = scoped_db(&state, &source_connection, &source_schema).await?;
    ensure_db(&state, &target_connection).await?;
    crate::sync::type_mapping(source.as_ref(), &source_kind, &source_schema, &source_table, &target_kind)
        .await.map_err(AppError::from)
}

/// 预览 CSV 文件（导入向导第一步）。
#[tauri::command]
pub fn preview_csv(
    path: String,
    delimiter: String,
    encoding: String,
    limit: Option<usize>,
) -> CmdResult<(Vec<String>, Vec<Vec<String>>)> {
    crate::transfer::preview_csv(&path, &delimiter, &encoding, limit.unwrap_or(20))
        .map_err(AppError::from)
}

/// 数据传输。
#[tauri::command]
pub async fn run_transfer(
    state: State<'_, AppState>,
    opts: TransferOptions,
) -> CmdResult<u64> {
    let _activity_guard = state.activity.enter()?;
    let _manual_guard = state.manual_transactions.ordinary(&[&opts.source_connection, &opts.target_connection]).await?;
    let source_id = opts.source_connection.clone();
    let target_id = opts.target_connection.clone();
    let source_kind = kind_of(&state, &source_id)?;
    let target_kind = kind_of(&state, &target_id)?;
    require_writable_connection(&state, &target_id)?;
    crate::transfer::validate_where(opts.where_clause.as_deref())?;
    let source = scoped_db(&state, &source_id, &opts.source_schema).await?;
    let target = scoped_db(&state, &target_id, &opts.target_schema).await?;
    validate_table_target(target.as_ref(), &target_kind, &opts.target_schema, &opts.target_table, opts.create_target).await?;
    crate::transfer::transfer(source.as_ref(), &source_kind, target.as_ref(), &target_kind, &opts)
        .await
        .map_err(AppError::from)
}

/// SQL 转储。
#[tauri::command]
pub async fn run_dump(state: State<'_, AppState>, opts: DumpOptions) -> CmdResult<u64> {
    let _activity_guard = state.activity.enter()?;
    let id = opts.connection_id.clone();
    let kind = kind_of(&state, &id)?;
    let db = scoped_db(&state, &id, &opts.schema).await?;
    protect_export_path(&state, &opts.file, db.as_ref()).await?;
    crate::transfer::dump(db.as_ref(), &kind, &opts)
        .await
        .map_err(AppError::from)
}

// ---------------------------------------------------------------- 备份 / 还原

/// 列出某连接某数据库下的备份文件。
#[tauri::command]
pub fn list_backups(connection_id: String, schema_name: String) -> CmdResult<Vec<BackupInfo>> {
    let dir = storage::backup_dir(&connection_id, &schema_name);
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            let name = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            if !name.to_ascii_lowercase().ends_with(".sql") {
                continue;
            }
            let meta = e.metadata()?;
            let modified = meta
                .modified()
                .ok()
                .map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_default();
            // 注释写在文件头 "-- 注释     : xxx"；只读取有界前缀，绝不为前 12 行加载整个备份。
            let comment = storage::read_backup_comment(&p).unwrap_or_default();
            out.push(BackupInfo {
                name,
                modified,
                size: meta.len(),
                comment,
            });
        }
    }
    out.sort_by(|a, b| b.name.cmp(&a.name));
    Ok(out)
}

/// 已发布备份的完整路径；显示失败不改变已经成功落盘的备份结果。
fn backup_saved_path(file: &std::path::Path) -> String {
    let resolved = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let path = resolved.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{}", unc);
        }
        if let Some(local) = path.strip_prefix(r"\\?\") {
            return local.to_owned();
        }
    }
    path.into_owned()
}

/// 创建整库备份（转储为 SQL 文件），返回行数、字节数、实际路径与跳过警告。
#[tauri::command]
pub async fn create_backup(state: State<'_, AppState>, opts: BackupOptions) -> CmdResult<crate::models::BackupResult> {
    let _activity_guard = state.activity.enter()?;
    let id = opts.connection_id.clone();
    let kind = kind_of(&state, &id)?;
    let db = scoped_db(&state, &id, &opts.schema).await?;
    let dir = storage::backup_dir(&id, &opts.schema);
    // 自定义文件名（可含合法文件名字符，不含扩展名时补 .sql）；否则按时间戳命名。
    let file = match opts.file_name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(name) => {
            let safe = name.trim_end_matches(".sql").trim_end_matches('.');
            if safe.is_empty() || safe.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
                return Err(AppError::new("备份文件名包含非法字符或为空"));
            }
            dir.join(format!("{}.sql", safe))
        }
        None => {
            let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
            dir.join(format!("{}_{}.sql", opts.schema, stamp))
        }
    };
    let report = crate::transfer::dump_database_with_consistency(
        db.as_ref(),
        &kind,
        &opts.schema,
        &opts.tables,
        &opts.routines,
        opts.include_structure,
        opts.include_data,
        &file.to_string_lossy(),
        &opts.comment,
        opts.skip_incompatible_tables,
        opts.source_consistency.as_deref(),
    )
    .await
    .map_err(AppError::from)?;
    storage::log_line(
        "backup",
        &format!(
            "备份 {}@{} → {}（{} 行，{} 字节）",
            opts.schema,
            id,
            file.file_name().map(|s| s.to_string_lossy()).unwrap_or_default(),
            report.rows,
            report.bytes
        ),
    );
    Ok(crate::models::BackupResult {
        rows: report.rows,
        bytes: report.bytes,
        path: backup_saved_path(&file),
        warnings: report.warnings,
    })
}

/// 数据库右键「运行 SQL 文件」：逐条执行选中的 SQL 文件，返回每条语句的执行报告。
/// `stop_on_error` 为 true 时遇错终止（保持数据一致性），false 时跳过错误继续。
#[tauri::command]
pub async fn run_sql_file(
    state: State<'_, AppState>,
    id: String,
    schema_name: String,
    path: String,
    stop_on_error: bool,
) -> CmdResult<crate::transfer::RestoreReport> {
    let _activity_guard = state.activity.enter()?;
    let db = scoped_db(&state, &id, &schema_name).await?;
    crate::transfer::run_sql_file_detailed(db.as_ref(), &path, stop_on_error)
        .await
        .map_err(AppError::from)
}

/// 数据库右键「转储 SQL 文件」：把整个数据库（表 + 视图）转储为单个 SQL 文件，
/// 保存到用户指定位置，返回 (数据行数, 文件字节数)。
#[tauri::command]
pub async fn dump_database_to_file(
    state: State<'_, AppState>,
    id: String,
    schema_name: String,
    file: String,
    include_structure: bool,
    include_data: bool,
) -> CmdResult<(u64, u64)> {
    let _activity_guard = state.activity.enter()?;
    let kind = kind_of(&state, &id)?;
    let db = scoped_db(&state, &id, &schema_name).await?;
    if file.trim().is_empty() {
        return Err(AppError::new("请指定输出文件"));
    }
    crate::transfer::dump_database(
        db.as_ref(),
        &kind,
        &schema_name,
        &[],
        &[],
        include_structure,
        include_data,
        &file,
        "",
    )
    .await
    .map_err(AppError::from)
}

/// 重命名备份文件（备份列表右键）。仅允许改名到同一备份目录内。
#[tauri::command]
pub async fn rename_backup(
    state: State<'_, AppState>,
    connection_id: String,
    schema_name: String,
    file: String,
    new_name: String,
) -> CmdResult<()> {
    let _activity_guard = state.activity.enter()?;
    let dir = storage::backup_dir(&connection_id, &schema_name);
    let old_name = std::path::Path::new(&file)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| file.clone());
    let old_path = dir.join(&old_name);
    let safe = new_name.trim().trim_end_matches(".sql").trim_end_matches('.').to_string();
    if safe.is_empty() || safe.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
        return Err(AppError::new("备份文件名包含非法字符或为空"));
    }
    let new_path = dir.join(format!("{}.sql", safe));
    if !old_path.exists() {
        return Err(AppError::new("备份文件不存在"));
    }
    if new_path.exists() && new_path != old_path {
        return Err(AppError::new("同名备份已存在"));
    }
    std::fs::rename(&old_path, &new_path)
        .map_err(|e| AppError::new(format!("重命名失败：{}", e)))?;
    storage::log_line("backup", &format!("重命名备份 {} → {}", old_name, safe));
    Ok(())
}

/// 还原备份：逐条执行备份文件里的 SQL 语句，返回每条语句的执行结果
/// （供前端「还原备份」对话框展示执行过程 / 语句 / 结果）。
#[tauri::command]
pub async fn restore_backup_detailed(
    state: State<'_, AppState>,
    connection_id: String,
    schema_name: String,
    file: String,
    task_id: Option<String>,
) -> CmdResult<crate::transfer::RestoreReport> {
    let _activity_guard = state.activity.enter()?;
    let _manual_guard = state.manual_transactions.ordinary(&[&connection_id]).await?;
    // 只允许备份目录内的文件名，防路径穿越
    if file.contains('/') || file.contains('\\') || file.contains("..") {
        return Err(AppError::new("非法的备份文件名"));
    }
    let path = storage::backup_dir(&connection_id, &schema_name).join(&file);
    let kind = kind_of(&state, &connection_id)?;
    require_writable_connection(&state, &connection_id)?;
    let (_, db) = query_db(&state, &connection_id, Some(&schema_name)).await?;
    let report = if let Some(task_id) = task_id.as_deref() {
        crate::transfer::restore_sql_file_task(db.as_ref(), &path.to_string_lossy(), task_id).await
    } else {
        crate::transfer::restore_sql_file_detailed(db.as_ref(), &kind, &path.to_string_lossy()).await
    }.map_err(AppError::from)?;
    storage::log_line(
        "backup",
        &format!(
            "还原 {}@{} ← {}（成功 {} / 失败 {} / 共 {} 条）",
            schema_name, connection_id, file, report.succeeded, report.failed, report.total
        ),
    );
    Ok(report)
}

/// 还原备份：执行备份文件里的全部 SQL 语句，返回执行的语句数。
#[tauri::command]
pub async fn restore_backup(
    state: State<'_, AppState>,
    connection_id: String,
    schema_name: String,
    file: String,
) -> CmdResult<u64> {
    let _activity_guard = state.activity.enter()?;
    let _manual_guard = state.manual_transactions.ordinary(&[&connection_id]).await?;
    // 只允许备份目录内的文件名，防路径穿越
    if file.contains('/') || file.contains('\\') || file.contains("..") {
        return Err(AppError::new("非法的备份文件名"));
    }
    let path = storage::backup_dir(&connection_id, &schema_name).join(&file);
    let kind = kind_of(&state, &connection_id)?;
    require_writable_connection(&state, &connection_id)?;
    let (_, db) = query_db(&state, &connection_id, Some(&schema_name)).await?;
    let count = crate::transfer::restore_sql_file(db.as_ref(), &kind, &path.to_string_lossy())
        .await
        .map_err(AppError::from)?;
    storage::log_line("backup", &format!("还原 {}@{} ← {}（{} 条语句）", schema_name, connection_id, file, count));
    Ok(count)
}

/// 删除备份文件。
#[tauri::command]
pub fn delete_backup(connection_id: String, schema_name: String, file: String) -> CmdResult<()> {
    if file.contains('/') || file.contains('\\') || file.contains("..") {
        return Err(AppError::new("非法的备份文件名"));
    }
    let path = storage::backup_dir(&connection_id, &schema_name).join(&file);
    std::fs::remove_file(&path).map_err(|e| AppError::new(format!("删除失败: {}", e)))?;
    storage::log_line("backup", &format!("删除备份 {}", path.display()));
    Ok(())
}

/// 用系统文件管理器打开备份所在文件夹，返回目录路径。
#[tauri::command]
pub fn open_backup_folder(connection_id: String, schema_name: String) -> CmdResult<String> {
    let dir = storage::backup_dir(&connection_id, &schema_name);
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&dir)
            .spawn()
            .map_err(|e| AppError::new(format!("无法打开文件夹: {}", e)))?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        #[cfg(target_os = "macos")]
        let prog = "open";
        #[cfg(all(unix, not(target_os = "macos")))]
        let prog = "xdg-open";
        std::process::Command::new(prog)
            .arg(&dir)
            .spawn()
            .map_err(|e| AppError::new(format!("无法打开文件夹: {}", e)))?;
    }
    Ok(dir.to_string_lossy().to_string())
}

/// 用系统文件管理器打开任意本地路径（文件 → 选中，目录 → 进入）。
#[tauri::command]
pub fn open_path_in_explorer(path: String) -> CmdResult<()> {
    let target = std::path::PathBuf::from(&path);
    if !target.exists() {
        return Err(AppError::new("路径不存在或已被删除"));
    }
    #[cfg(target_os = "windows")]
    {
        if target.is_dir() {
            std::process::Command::new("explorer")
                .arg(&target)
                .spawn()
                .map_err(|e| AppError::new(format!("无法打开文件夹: {}", e)))?;
        } else {
            // /select, 带逗号参数定位到文件
            std::process::Command::new("explorer")
                .arg(format!("/select,{}", target.display()))
                .spawn()
                .map_err(|e| AppError::new(format!("无法打开文件夹: {}", e)))?;
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        #[cfg(target_os = "macos")]
        let prog: &str = "open";
        #[cfg(all(unix, not(target_os = "macos")))]
        let prog: &str = "xdg-open";
        let dir = if target.is_dir() { target } else { target.parent().map(|p| p.to_path_buf()).unwrap_or(target) };
        std::process::Command::new(prog)
            .arg(&dir)
            .spawn()
            .map_err(|e| AppError::new(format!("无法打开文件夹: {}", e)))?;
    }
    Ok(())
}

// ---------------------------------------------------------------- 服务器信息

#[tauri::command]
pub async fn server_info(
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<ServerInfo> {
    let _activity_guard = state.activity.enter()?;
    let kind = kind_of(&state, &id)?;
    let conns = state.store.connections();
    let cfg = conns.iter().find(|c| c.id == id);
    let (host, port) = cfg
        .map(|c| (c.host.clone(), c.port))
        .unwrap_or_else(|| (String::new(), 0));
    let db = ensure_db(&state, &id).await?;
    let mut info = ServerInfo {
        product: kind.label().to_string(),
        version: String::new(),
        host,
        port,
        charset: String::new(),
        collation: String::new(),
        uptime: None,
        extra: Vec::new(),
    };
    match db.as_ref() {
        db::Db::Sqlite(pool) => {
            if let Ok(row) = crate::sql_history::query("SELECT sqlite_version() AS v").fetch_one(pool).await {
                let cols = row.columns();
                if let Some(i) = cols.iter().position(|c| c.name() == "v") {
                    info.version = crate::values::sqlite_row_values(&row, cols.len())
                        .get(i)
                        .cloned()
                        .flatten()
                        .unwrap_or_default();
                }
            }
            info.charset = "UTF-8".into();
        }
        db::Db::Mysql(pool) => {
            if let Ok(rows) = crate::sql_history::query(
                "SHOW VARIABLES WHERE Variable_name IN \
                 ('version','character_set_server','collation_server','uptime')",
            )
            .fetch_all(pool)
            .await
            {
                for r in &rows {
                    let cols = r.columns();
                    let vals = crate::values::my_row_values(r, cols.len());
                    let mut name = String::new();
                    let mut value = String::new();
                    if let Some(i) = cols.iter().position(|c| c.name() == "Variable_name") {
                        name = vals.get(i).cloned().flatten().unwrap_or_default();
                    }
                    if let Some(i) = cols.iter().position(|c| c.name() == "Value") {
                        value = vals.get(i).cloned().flatten().unwrap_or_default();
                    }
                    match name.as_str() {
                        "version" => info.version = value.clone(),
                        "character_set_server" => info.charset = value.clone(),
                        "collation_server" => info.collation = value.clone(),
                        "uptime" => info.uptime = Some(value.clone()),
                        _ => {}
                    }
                    if !name.is_empty() {
                        info.extra.push((name, value));
                    }
                }
            }
        }
        db::Db::Postgres(pool) => {
            if let Ok(row) = crate::sql_history::query("SELECT version() AS v").fetch_one(pool).await {
                let cols = row.columns();
                if let Some(i) = cols.iter().position(|c| c.name() == "v") {
                    info.version = crate::values::pg_row_values(&row, cols.len())
                        .get(i)
                        .cloned()
                        .flatten()
                        .unwrap_or_default();
                }
            }
            if let Ok(row) = crate::sql_history::query("SHOW server_encoding").fetch_one(pool).await {
                let cols = row.columns();
                if let Some(i) = cols.iter().position(|c| c.name() == "server_encoding") {
                    info.charset = crate::values::pg_row_values(&row, cols.len())
                        .get(i)
                        .cloned()
                        .flatten()
                        .unwrap_or_default();
                }
            }
            if let Ok(row) = crate::sql_history::query("SHOW server_version").fetch_one(pool).await {
                let cols = row.columns();
                if let Some(i) = cols.iter().position(|c| c.name() == "server_version") {
                    if let Some(v) = crate::values::pg_row_values(&row, cols.len())
                        .get(i)
                        .cloned()
                        .flatten()
                    {
                        info.extra.push(("服务器版本".into(), v));
                    }
                }
            }
        }
        _ => return Err(AppError::new("不支持的连接类型")),
}
    Ok(info)
}

// ---------------------------------------------------------------- 用户

/// 列出数据库用户（MySQL 协议族读 mysql.user 真实数据；其它后端返回空表）。
#[tauri::command]
pub async fn list_db_users(state: State<'_, AppState>, id: String) -> CmdResult<Vec<Vec<String>>> {
    let _activity_guard = state.activity.enter()?;
    if !kind_of(&state, &id)?.is_mysql_family() {
        return Ok(Vec::new());
    }
    let db = ensure_db(&state, &id).await?;
    let crate::db::Db::Mysql(pool) = db.as_ref() else {
        return Ok(Vec::new());
    };
    let rows = crate::sql_history::query(
        "SELECT user, host, ssl_type, max_questions, max_updates, max_connections, \
         max_user_connections, Super_priv FROM mysql.user ORDER BY user, host",
    )
    .fetch_all(pool)
    .await
    .map_err(AppError::from)?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let cols = r.columns();
        // mysql.user 的 User/Host 为 VARBINARY：走元数据解码，显示可读文本（BUG-R1-012）。
        let vals = crate::schema::mysql_meta_row_values(r);
        let get = |i: usize| vals.get(i).cloned().flatten().unwrap_or_default();
        let name = format!("{}@{}", get(0), get(1));
        // Super_priv 权限列是 'Y'/'N' 枚举（MySQL 5.7/8.0 通用；旧版 Super 列 8.0 已移除）
        let su = match get(7).to_ascii_uppercase().as_str() {
            "Y" => "是".to_string(),
            _ => "否".to_string(),
        };
        out.push(vec![
            name,
            get(2),
            get(3),
            get(4),
            get(5),
            get(6),
            su,
        ]);
    }
    Ok(out)
}

// ---------------------------------------------------------------- 用户管理（MySQL 协议族）

/// 引用用户 / 主机名（MySQL 标识符内的引号转义）。
fn quote_user(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// 新建用户（CREATE USER ... IDENTIFIED BY）。
#[tauri::command]
pub async fn create_db_user(
    state: State<'_, AppState>,
    id: String,
    user: String,
    host: String,
    password: String,
) -> CmdResult<()> {
    let _activity_guard = state.activity.enter()?;
    let _manual_guard = state.manual_transactions.ordinary(&[&id]).await?;
    require_writable_connection(&state, &id)?;
    let kind = kind_of(&state, &id)?;
    if !kind.is_mysql_family() {
        return Err(AppError::new("仅 MySQL 协议族数据库支持用户管理"));
    }
    if user.trim().is_empty() {
        return Err(AppError::new("用户名不能为空"));
    }
    let db = ensure_db(&state, &id).await?;
    let host = if host.trim().is_empty() { "%" } else { host.trim() };
    let sql = format!(
        "CREATE USER {}@{} IDENTIFIED BY {}",
        quote_user(user.trim()),
        quote_user(host),
        quote_literal(&password)
    );
    crate::schema::exec_affected(db.as_ref(), &kind, &sql)
        .await
        .map_err(AppError::from)?;
    storage::log_line("user", &format!("新建用户 {}@{}", user, host));
    Ok(())
}

/// 删除用户。
#[tauri::command]
pub async fn drop_db_user(
    state: State<'_, AppState>,
    id: String,
    user: String,
    host: String,
) -> CmdResult<()> {
    let _activity_guard = state.activity.enter()?;
    let _manual_guard = state.manual_transactions.ordinary(&[&id]).await?;
    require_writable_connection(&state, &id)?;
    let kind = kind_of(&state, &id)?;
    if !kind.is_mysql_family() {
        return Err(AppError::new("仅 MySQL 协议族数据库支持用户管理"));
    }
    let db = ensure_db(&state, &id).await?;
    let sql = format!("DROP USER {}@{}", quote_user(&user), quote_user(&host));
    crate::schema::exec_affected(db.as_ref(), &kind, &sql)
        .await
        .map_err(AppError::from)?;
    storage::log_line("user", &format!("删除用户 {}@{}", user, host));
    Ok(())
}

/// 修改用户密码。
#[tauri::command]
pub async fn alter_db_user_password(
    state: State<'_, AppState>,
    id: String,
    user: String,
    host: String,
    password: String,
) -> CmdResult<()> {
    let _activity_guard = state.activity.enter()?;
    let _manual_guard = state.manual_transactions.ordinary(&[&id]).await?;
    require_writable_connection(&state, &id)?;
    let kind = kind_of(&state, &id)?;
    if !kind.is_mysql_family() {
        return Err(AppError::new("仅 MySQL 协议族数据库支持用户管理"));
    }
    let db = ensure_db(&state, &id).await?;
    let sql = format!(
        "ALTER USER {}@{} IDENTIFIED BY {}",
        quote_user(&user),
        quote_user(&host),
        quote_literal(&password)
    );
    crate::schema::exec_affected(db.as_ref(), &kind, &sql)
        .await
        .map_err(AppError::from)?;
    storage::log_line("user", &format!("修改用户 {}@{} 的密码", user, host));
    Ok(())
}

// ---------------------------------------------------------------- 视图 / 函数定义

/// 读取视图定义（完整的 CREATE VIEW 语句）。
#[tauri::command]
pub async fn get_view_definition(
    state: State<'_, AppState>,
    id: String,
    schema_name: String,
    view: String,
) -> CmdResult<String> {
    let _activity_guard = state.activity.enter()?;
    let kind = kind_of(&state, &id)?;
    let db = scoped_db(&state, &id, &schema_name).await?;
    crate::schema::view_definition(db.as_ref(), kind, &schema_name, &view)
        .await
        .map_err(AppError::from)
}

/// 读取存储过程 / 函数源码。
#[tauri::command]
pub async fn get_routine_definition(
    state: State<'_, AppState>,
    id: String,
    schema_name: String,
    name: String,
    routine_type: String,
) -> CmdResult<String> {
    let _activity_guard = state.activity.enter()?;
    let kind = kind_of(&state, &id)?;
    if kind.protocol() == crate::models::Protocol::Sqlite {
        return Err(AppError::new("SQLite 不支持存储过程 / 函数"));
    }
    let db = ensure_db(&state, &id).await?;
    match db.as_ref() {
        db::Db::Mysql(pool) => {
            let routine = crate::mysql_routines::RoutineKind::parse(&routine_type).map_err(AppError::from)?;
            crate::mysql_routines::definition(pool, &kind, &schema_name, &name, routine).await.map_err(AppError::from)
        }
        db::Db::Postgres(pool) => {
            pg_routine_definition(pool, &schema_name, &name, &routine_type).await
        }
        db::Db::Sqlite(_) => Err(AppError::new("SQLite 不支持存储过程 / 函数")),
            _ => Err(AppError::new("不支持的连接类型")),
    }
}

/// A routine name is not an OID and can identify several overloads. Return
/// every matching CREATE definition in a stable signature order; each remains
/// executable SQL and includes its own argument signature.
async fn pg_routine_definition(
    pool: &sqlx::PgPool,
    schema_name: &str,
    name: &str,
    routine_type: &str,
) -> CmdResult<String> {
    let prokind = match routine_type {
        "function" => "f",
        "procedure" => "p",
        _ => return Err(AppError::new("PostgreSQL 定义仅支持函数或存储过程")),
    };
    let definitions: Vec<String> = crate::sql_history::query_scalar(
        "SELECT pg_catalog.pg_get_functiondef(p.oid) \
         FROM pg_catalog.pg_proc p \
         JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace \
         WHERE n.nspname = $1 AND p.proname = $2 AND p.prokind::text = $3 \
         ORDER BY pg_catalog.pg_get_function_identity_arguments(p.oid), p.oid"
    )
    .bind(schema_name)
    .bind(name)
    .bind(prokind)
    .fetch_all(pool)
    .await?;
    if definitions.is_empty() {
        return Err(AppError::new("对象不存在"));
    }
    if definitions.iter().any(|ddl| ddl.trim().is_empty()) {
        return Err(AppError::new("对象定义获取失败"));
    }
    // pg_get_functiondef omits the outer statement terminator on some server
    // versions. Each overload needs one before the next CREATE can be parsed.
    Ok(definitions.into_iter().map(|ddl| {
        let ddl = ddl.trim_end();
        if ddl.ends_with(';') { ddl.to_string() } else { format!("{};", ddl) }
    }).collect::<Vec<_>>().join("\n\n"))
}

/// 读取单个例程的完整身份与当前定义（供安全编辑使用）。
#[tauri::command]
pub async fn read_routine_design(
    state: State<'_, AppState>,
    id: String,
    schema_name: String,
    name: String,
    routine_type: String,
    identity_arguments: Option<String>,
) -> CmdResult<crate::routine_design::RoutineSnapshot> {
    let _activity_guard = state.activity.enter()?;
    let kind = kind_of(&state, &id)?;
    let db = scoped_db(&state, &id, &schema_name).await?;
    crate::routine_design::snapshot(
        db.as_ref(),
        &kind,
        crate::routine_design::RoutineObjectId {
            connection_id: id,
            schema: schema_name,
            name,
            kind: routine_type,
            identity_arguments: identity_arguments.unwrap_or_default(),
        },
    )
    .await
    .map_err(AppError::from)
}

/// 应用例程替换计划：定义 revision CAS、方言编译、执行与读回校验。
#[tauri::command]
pub async fn apply_routine_replacement(
    state: State<'_, AppState>,
    id: String,
    draft: crate::routine_design::RoutineReplacementDraft,
) -> CmdResult<crate::routine_design::RoutineReceipt> {
    let _activity_guard = state.activity.enter()?;
    let _manual_guard = state.manual_transactions.ordinary(&[&id]).await?;
    require_writable_connection(&state, &id)?;
    if !draft.object.connection_id.is_empty() && draft.object.connection_id != id {
        return Err(AppError::new("提交的连接与目标连接不一致"));
    }
    let schema = draft.object.schema.clone();
    let db = scoped_db(&state, &id, &schema).await?;
    let kind = kind_of(&state, &id)?;
    crate::routine_design::apply(db.as_ref(), &kind, &draft).await.map_err(AppError::from)
}

// ---------------------------------------------------------------- 表信息 / DDL

/// 表状态信息（信息面板「表信息」页）。
#[tauri::command]
pub async fn table_status(
    state: State<'_, AppState>,
    id: String,
    schema_name: String,
    table: String,
) -> CmdResult<Vec<(String, String)>> {
    let _activity_guard = state.activity.enter()?;
    let db = scoped_db(&state, &id, &schema_name).await?;
    schema::table_status(db.as_ref(), kind_of(&state, &id)?, &schema_name, &table)
        .await
        .map_err(AppError::from)
}

/// 表的建表语句（信息面板「DDL」页）。
#[tauri::command]
pub async fn get_table_ddl(
    state: State<'_, AppState>,
    id: String,
    schema_name: String,
    table: String,
) -> CmdResult<String> {
    let _activity_guard = state.activity.enter()?;
    let db = scoped_db(&state, &id, &schema_name).await?;
    schema::table_ddl(db.as_ref(), kind_of(&state, &id)?, &schema_name, &table)
        .await
        .map_err(AppError::from)
}

// ---------------------------------------------------------------- 自动运行

/// 列出自动运行任务。
#[tauri::command]
pub fn list_jobs() -> Vec<ScheduleJob> {
    storage::read_json(&storage::jobs_path()).unwrap_or_default()
}

/// 保存（新增或更新）自动运行任务。
#[tauri::command]
pub fn save_job(job: ScheduleJob) -> CmdResult<Vec<ScheduleJob>> {
    let mut jobs: Vec<ScheduleJob> = storage::read_json(&storage::jobs_path()).unwrap_or_default();
    let mut job = job;
    if let Some(at) = job.daily_at.as_mut() {
        let trimmed = at.trim().to_string();
        if !valid_daily_at(&trimmed) { return Err(AppError::coded("JOB_SCHEDULE_INVALID", "每天执行时刻必须是 24 小时制 HH:MM")); }
        *at = trimmed;
        job.interval_secs = 0;
    }
    if job.id.is_empty() {
        job.id = uuid::Uuid::new_v4().to_string();
    }
    match jobs.iter_mut().find(|j| j.id == job.id) {
        Some(slot) => *slot = job.clone(),
        None => jobs.push(job.clone()),
    }
    storage::write_json(&storage::jobs_path(), &jobs);
    Ok(jobs)
}

fn valid_daily_at(at: &str) -> bool {
    let Some((h, m)) = at.split_once(':') else { return false };
    if h.len() > 2 || m.len() != 2 { return false; }
    matches!((h.parse::<u32>(), m.parse::<u32>()), (Ok(h), Ok(m)) if h < 24 && m < 60)
}

/// 删除自动运行任务。
#[tauri::command]
pub fn delete_job(id: String) -> CmdResult<Vec<ScheduleJob>> {
    let mut jobs: Vec<ScheduleJob> = storage::read_json(&storage::jobs_path()).unwrap_or_default();
    jobs.retain(|j| j.id != id);
    storage::write_json(&storage::jobs_path(), &jobs);
    Ok(jobs)
}

/// 追加一条运行历史并裁剪到最近 200 条。
fn push_history(entry: JobHistoryEntry) {
    let mut list: Vec<JobHistoryEntry> =
        storage::read_json(&storage::job_history_path()).unwrap_or_default();
    list.insert(0, entry);
    list.truncate(200);
    storage::write_json(&storage::job_history_path(), &list);
}

/// 列出自动运行历史。
#[tauri::command]
pub fn list_job_history() -> Vec<JobHistoryEntry> {
    storage::read_json(&storage::job_history_path()).unwrap_or_default()
}

/// 前端派发的任务型自动运行（备份 / 导出 / 导入 / 数据同步 / 数据生成）
/// 统一写入同一份运行历史，与后端 SQL 任务共用。
#[tauri::command]
pub fn record_job_history(
    job_id: String,
    job_name: String,
    ok: bool,
    elapsed_ms: u64,
    detail: String,
) -> CmdResult<()> {
    let started = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    push_history(JobHistoryEntry {
        job_id: job_id.clone(),
        job_name,
        started_at: started.clone(),
        elapsed_ms,
        ok,
        detail,
    });
    // 任务型执行（备份 / 导出 / 导入 / 数据同步 / 数据生成 / 数据字典）
    // 与后端 SQL 任务一样回写列表的上次运行 / 状态列。
    let mut jobs: Vec<ScheduleJob> =
        storage::read_json(&storage::jobs_path()).unwrap_or_default();
    if let Some(slot) = jobs.iter_mut().find(|j| j.id == job_id) {
        slot.last_run = Some(started);
        slot.last_status = Some(if ok { "成功".into() } else { "失败".into() });
        storage::write_json(&storage::jobs_path(), &jobs);
    }
    Ok(())
}

/// 立即执行一个任务：打开连接 → 执行 SQL → 记录状态与历史。
#[tauri::command]
pub async fn run_job_now(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<(u64, String)> {
    let mut jobs: Vec<ScheduleJob> = storage::read_json(&storage::jobs_path()).unwrap_or_default();
    let Some(job) = jobs.iter().find(|j| j.id == id).cloned() else {
        return Err(AppError::new("任务不存在"));
    };
    let started = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let start = std::time::Instant::now();

    // 确保连接可用（未打开则先打开）
    let result = (|| async {
        let mut request = crate::execution_contracts::ExecutionRequest {
            execution_id: uuid::Uuid::new_v4().to_string(), connection_id: job.connection_id.clone(), namespace: job.database.clone(),
            owner_id: format!("job:{}", job.id), sql: job.sql.clone(), transaction_owner: None, timeout_ms: None, origin: Some("job".into()),
            execution_grant: None, cancel_before_send: false,
        };
        grant_execution_request(&state, &mut request, window.label())?;
        legacy_outcome(execute_sql_service(&state, request, window.label()).await?)
    })()
    .await;

    let (ok, detail, _messages) = match result {
        Ok(mr) => {
            let msgs = mr.messages.clone();
            let sets_info = mr
                .sets
                .iter()
                .map(|s| {
                    if s.has_rows {
                        format!("结果集 {} 行", s.total)
                    } else {
                        format!("{} 行受影响", s.affected)
                    }
                })
                .collect::<Vec<_>>()
                .join("；");
            let detail = if sets_info.is_empty() {
                msgs.join("；")
            } else if msgs.is_empty() {
                sets_info
            } else {
                format!("{}；{}", sets_info, msgs.join("；"))
            };
            (true, if detail.is_empty() { "执行成功".into() } else { detail }, msgs)
        }
        Err(e) => (false, format!("{:#}", e), Vec::new()),
    };
    let elapsed = start.elapsed().as_millis() as u64;

    // 更新任务状态
    if let Some(slot) = jobs.iter_mut().find(|j| j.id == id) {
        slot.last_run = Some(started.clone());
        slot.last_status = Some(if ok { "成功".into() } else { "失败".into() });
    }
    storage::write_json(&storage::jobs_path(), &jobs);
    push_history(JobHistoryEntry {
        job_id: id,
        job_name: job.name.clone(),
        started_at: started,
        elapsed_ms: elapsed,
        ok,
        detail: detail.clone(),
    });
    storage::log_line("schedule", &format!("任务「{}」{}（{} ms）", job.name, if ok { "执行成功" } else { "执行失败" }, elapsed));
    Ok((elapsed, detail))
}

// ---------------------------------------------------------------- 自动补全

/// 获取某连接的表名与列名，用于 SQL 编辑器自动补全。
///
/// 保持扁平协议（对象名 → 列名，外加 `__tables__`/`__views__`/`__schemas__`），
/// 另以 `__meta__` 携带真实读取到的表/字段备注与字段类型；未读取到的值保持 null，
/// 由前端决定不渲染，而不是写占位文案。
#[tauri::command]
pub async fn completion_data(
    state: State<'_, AppState>,
    id: String,
    schema_name: String,
) -> CmdResult<serde_json::Value> {
    let _activity_guard = state.activity.enter()?;
    let kind = kind_of(&state, &id)?;
    let db = scoped_db(&state, &id, &schema_name).await?;
    load_completion_data(db.as_ref(), kind, &schema_name).await
}

async fn load_completion_data(
    db: &db::Db,
    kind: DbKind,
    schema_name: &str,
) -> CmdResult<serde_json::Value> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    use futures::StreamExt;

    let tables = schema::list_tables(db, kind.clone(), schema_name).await?;
    // 保留 __tables__ 包含全部可查询对象的旧协议，另提供视图类型。
    let names: Vec<String> = tables.iter().map(|(name, _)| name.clone()).collect();
    let views: Vec<String> = tables.iter().filter(|(_, ty)| matches!(ty, ObjectType::View))
        .map(|(name, _)| name.clone()).collect();
    let mut out = serde_json::Map::new();
    let mut relations = serde_json::Map::new();
    let mut column_meta = serde_json::Map::new();
    let jobs: Vec<_> = tables.into_iter().map(|(name, ty): (String, ObjectType)| {
        let kind = kind.clone();
        async move {
            let result = schema::table_schema(db, kind, schema_name, &name).await;
            (name, ty, result)
        }
    }).collect();
    let fields = futures::stream::iter(jobs).buffer_unordered(4).collect::<Vec<_>>().await;
    for (name, ty, result) in fields {
        let class = if matches!(ty, ObjectType::View) { "view" } else { "table" };
        match result {
            Ok(info) => {
                out.insert(name.clone(), serde_json::json!(
                    info.columns.iter().map(|c| c.name.clone()).collect::<Vec<_>>()));
                let mut columns = serde_json::Map::new();
                for column in &info.columns {
                    columns.insert(column.name.clone(), serde_json::json!({
                        "type": column.data_type,
                        "comment": column.comment,
                    }));
                }
                column_meta.insert(name.clone(), serde_json::Value::Object(columns));
                relations.insert(name, serde_json::json!({
                    "kind": class,
                    "comment": info.comment,
                    "columns_known": true,
                }));
            }
            Err(error) => {
                // 一张表无权限时仍提示其它表；该表的列标记为未读取，
                // 前端保留表候选但不假装“没有备注”。
                relations.insert(name.clone(), serde_json::json!({
                    "kind": class,
                    "comment": null,
                    "columns_known": false,
                }));
                storage::log_line("completion", &format!("读取 {}.{} 字段失败：{}", schema_name, name, error));
            }
        }
    }
    out.insert("__tables__".into(), serde_json::json!(names));
    out.insert("__views__".into(), serde_json::json!(views));
    out.insert("__schemas__".into(), serde_json::json!(if schema_name.is_empty() { vec![] } else { vec![schema_name.to_string()] }));
    out.insert("__meta__".into(), serde_json::json!({
        "version": 2,
        "schema": schema_name,
        "relations": relations,
        "columns": column_meta,
    }));
    Ok(serde_json::Value::Object(out))

    }).await
}

#[cfg(test)]
mod completion_tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn completion_uses_real_table_and_view_columns() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1)
            .connect("sqlite::memory:").await.unwrap();
        crate::sql_history::query("CREATE TABLE users (id INTEGER PRIMARY KEY, display_name TEXT)")
            .execute(&pool).await.unwrap();
        crate::sql_history::query("CREATE VIEW active_users AS SELECT id, display_name FROM users")
            .execute(&pool).await.unwrap();
        let data = load_completion_data(&db::Db::Sqlite(pool), DbKind::of("sqlite"), "main").await.unwrap();
        assert_eq!(data["__tables__"], json!(["active_users", "users"]));
        assert_eq!(data["__views__"], json!(["active_users"]));
        assert_eq!(data["__schemas__"], json!(["main"]));
        assert_eq!(data["users"], json!(["id", "display_name"]));
        assert_eq!(data["active_users"], json!(["id", "display_name"]));
        // SQLite 不提供表/字段备注：元数据仍给出真实类型，备注为 null 而非伪造文案。
        assert_eq!(data["__meta__"]["relations"]["users"]["columns_known"], json!(true));
        assert_eq!(data["__meta__"]["columns"]["users"]["id"]["type"], json!("INTEGER"));
        assert!(data["__meta__"]["columns"]["users"]["id"]["comment"].is_null());
    }
}

// ---------------------------------------------------------------- 工具

fn connection_success(message: &str) -> TestResult {
    TestResult { ok: true, message: message.into(), version: String::new(), charset: String::new() }
}

pub(crate) fn saved_config(state: &AppState, id: &str) -> CmdResult<ConnectionConfig> {
    state.store.connections().into_iter().find(|c| c.id == id)
        .ok_or_else(|| AppError::new("连接不存在"))
}

pub(crate) fn require_writable_connection(state: &AppState, id: &str) -> CmdResult<()> {
    if saved_config(state, id)?.read_only {
        return Err(AppError::coded("READ_ONLY_VIOLATION", "目标连接为只读连接，不能写入数据或修改结构"));
    }
    Ok(())
}

/// Validate saved credentials even when registered. A concurrent ensure may open a
/// spare pool, but entry() never replaces the pool another command is using.
pub(crate) async fn ensure_db(state: &AppState, id: &str) -> CmdResult<Arc<db::Db>> {
    crate::runtime_commands::ensure(state, id).await
}

/// The registry key a physical pool was admitted under. A MySQL scoped read returns
/// the database-level derived pool published as `id@database`, so callers must not
/// compare it against the base `id` record; every other dialect reuses the base pool.
pub(crate) fn physical_key(id: &str, namespace: &str, db: &db::Db) -> String {
    if matches!(db, db::Db::Mysql(_)) && !namespace.is_empty() { format!("{id}@{namespace}") } else { id.to_string() }
}

/// Metadata/transfer schema is a PG namespace, but a MySQL database. Never put
/// a PG tree namespace into ConnectionRequest.database.
pub(crate) async fn scoped_db(state: &AppState, id: &str, namespace: &str) -> CmdResult<Arc<db::Db>> {
    state.executions.require_quiescent(id)?;
    let base = ensure_db(state, id).await?;
    if namespace.is_empty() {
        return Ok(base);
    }
    if namespace.contains('\0') {
        return Err(AppError::new("数据库/模式名包含空字符"));
    }
    match base.as_ref() {
        db::Db::Postgres(pool) => {
            let exists: bool = crate::sql_history::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_namespace WHERE nspname = $1 AND pg_catalog.has_schema_privilege(oid, 'USAGE'))"
            ).bind(namespace).fetch_one(pool).await?;
            if !exists { return Err(AppError::new("PostgreSQL 模式不存在或没有访问权限")); }
            Ok(base)
        }
        db::Db::Mysql(_) => Ok(crate::runtime_commands::derived(state, id, namespace, false).await?.1),
        _ => {
            let names = schema::list_databases(base.as_ref(), saved_config(state, id)?.kind).await?;
            if !names.iter().any(|(name, _)| name == namespace) {
                return Err(AppError::new("数据库/模式不存在或没有访问权限"));
            }
            Ok(base)
        }
    }
}

pub(crate) fn pg_startup_search_path(kind: &DbKind, namespace: &str) -> String {
    // PostgreSQL splits startup `options` on whitespace (quotes do not group it).
    // Escape that layer separately from SQL identifier quoting.
    db::quote_ident(kind, namespace).chars().flat_map(|c| {
        if c.is_whitespace() || c == '\\' { vec!['\\', c] } else { vec![c] }
    }).collect()
}

/// Unqualified PG SQL needs an isolated startup search_path. Clone the original
/// pool's connection options (including saved real database/SSL), not a shared
/// session SET. Each namespace has its own cached pool cleaned by close_with_derived.
async fn query_db(state: &AppState, id: &str, namespace: Option<&str>) -> CmdResult<(String, Arc<db::Db>)> {
    state.executions.require_quiescent(id)?;
    let Some(namespace) = namespace.filter(|s| !s.is_empty()) else {
        return Ok((id.into(), ensure_db(state, id).await?));
    };
    let conn = scoped_db(state, id, namespace).await?;
    match conn.as_ref() {
        db::Db::Postgres(_) => crate::runtime_commands::derived(state, id, namespace, true).await,
        db::Db::Mysql(_) => Ok((format!("{}@{}", id, namespace), conn)),
        db::Db::Sqlite(_) => Ok((id.into(), conn)),
        db::Db::Http(http) if http.shares_session_namespace() => crate::runtime_commands::container(state, id, namespace, conn.clone()).await,
        _ => crate::runtime_commands::container(state, id, namespace, conn.clone()).await,
    }
}

#[cfg(test)]
#[path = "commands_h2_namespace_tests.rs"]
mod h2_namespace_tests;

#[cfg(test)]
#[path = "commands_jdbc_namespace_tests.rs"]
mod jdbc_namespace_tests;

#[cfg(test)]
#[path = "commands_physical_key_tests.rs"]
mod physical_key_tests;

pub(crate) async fn protect_export_path(state: &AppState, output: &str, conn: &db::Db) -> CmdResult<()> {
    let __sql_history_context = crate::sql_history::db_context(conn);
    crate::sql_history::scope(__sql_history_context, async {
    crate::transfer::protect_database_path(conn, output).await?;
    protect_saved_export_path(state, output)

    }).await
}

fn protect_saved_export_path(state: &AppState, output: &str) -> CmdResult<()> {
    if ["db", "sqlite", "sqlite3", "db3"].contains(&std::path::Path::new(output).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str()) {
        return Err(AppError::new("导出文件不能使用数据库文件扩展名"));
    }
    // Protect every saved SQLite database, not just the exporting connection.
    // Include WAL/SHM/journal siblings: overwriting those can corrupt a live DB.
    let output = std::path::Path::new(output);
    let output_path = std::fs::canonicalize(output).unwrap_or_else(|_| {
        let parent = output.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| std::path::Path::new("."));
        let parent = std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
        output.file_name().map(|name| parent.join(name)).unwrap_or_else(|| output.to_path_buf())
    });
    for cfg in state.store.connections().into_iter().filter(|c| c.kind.is_sqlite() && !c.file.is_empty()) {
        let database = std::fs::canonicalize(&cfg.file).unwrap_or_else(|_| std::path::PathBuf::from(&cfg.file));
        let output_name = output_path.to_string_lossy().to_ascii_lowercase();
        let database_name = database.to_string_lossy().to_ascii_lowercase();
        if output_name == database_name || ["-wal", "-shm", "-journal"].iter().any(|s| output_name == format!("{}{}", database_name, s)) {
            return Err(AppError::new("输出文件不能覆盖已保存的 SQLite 数据库或日志文件"));
        }
    }
    Ok(())
}

pub(crate) async fn validate_table_target(conn: &db::Db, kind: &DbKind, namespace: &str, table: &str, allow_create: bool) -> CmdResult<()> {
    let __sql_history_context = crate::sql_history::db_context(conn);
    crate::sql_history::scope(__sql_history_context, async {
    crate::transfer::validate_names(&[table.to_string()])?;
    let objects = schema::list_tables(conn, kind.clone(), namespace).await?;
    match objects.iter().find(|(name, _)| name == table) {
        Some((_, ObjectType::Table)) => Ok(()),
        Some(_) => Err(AppError::new("目标必须是数据表，不能写入视图或其它对象")),
        None if allow_create => Ok(()),
        None => Err(AppError::new("目标表不存在，请启用建表或选择现有表")),
    }

    }).await
}

/// 取连接的数据库类型。
pub(crate) fn kind_of(state: &State<'_, AppState>, id: &str) -> CmdResult<DbKind> {
    let conns = state.store.connections();
    conns.iter()
        .find(|c| c.id == id)
        .map(|c| c.kind.clone())
        .ok_or_else(|| AppError::new("连接不存在"))
}

/// 读取 UTF-8 SQL 文件（仅 .sql、普通文件、最多 16 MiB，支持 UTF-8 BOM）。
#[tauri::command]
pub fn read_sql_file(path: String) -> CmdResult<String> {
    use std::io::Read;
    const MAX_SQL_BYTES: u64 = 16 * 1024 * 1024;
    let path = std::path::Path::new(&path);
    if !path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("sql")) {
        return Err(AppError::new("只能读取 .sql 文件"));
    }
    let file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() { return Err(AppError::new("SQL 路径必须是普通文件")); }
    if metadata.len() > MAX_SQL_BYTES { return Err(AppError::new("SQL 文件不能超过 16 MiB")); }
    // Re-check a bounded read in case the file grows after metadata().
    let mut bytes = Vec::new();
    file.take(MAX_SQL_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SQL_BYTES { return Err(AppError::new("SQL 文件不能超过 16 MiB")); }
    let content = String::from_utf8(bytes).map_err(|_| AppError::new("SQL 文件必须使用 UTF-8 编码"))?;
    Ok(content.strip_prefix('\u{feff}').unwrap_or(&content).to_string())
}

/// 读取数据目录下的日志（供“日志”窗口使用）。
#[tauri::command]
pub fn read_log() -> Vec<String> {
    let path = storage::log_dir().join("navkit.log");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .rev()
        .take(500)
        .map(|s| s.to_string())
        .collect()
}

/// 数据源类型目录（新建连接选择器数据源：id / 名称 / 端口 / 图标 / 分类 /
/// 驱动提示 / 说明 / 是否规划中）。
#[tauri::command]
pub fn db_kinds() -> Vec<crate::models::KindInfo> {
    crate::catalog::CATALOG
        .iter()
        .map(|m| crate::models::KindInfo {
            id: m.id.to_string(),
            label: m.label.to_string(),
            port: m.port,
            category: m.category.to_string(),
            protocol: format!("{:?}", m.protocol).to_ascii_lowercase(),
            driver: m.driver.to_string(),
            note: m.note.to_string(),
            planned: m.planned,
        })
        .collect()
}

/// 数据源分类目录（顺序即展示顺序）。
#[tauri::command]
pub fn db_categories() -> Vec<(String, String)> {
    vec![
        ("relational".into(), "关系型数据库".into()),
        ("analytics".into(), "数仓分析".into()),
        ("domestic".into(), "国产数据库".into()),
        ("lightweight".into(), "轻量数据库".into()),
        ("nosql".into(), "文档/缓存/检索".into()),
        ("graph".into(), "图谱/向量".into()),
        ("timeseries".into(), "时序数据库".into()),
        ("mq".into(), "消息队列".into()),
        ("registry".into(), "注册与配置".into()),
    ]
}

/// 关于对话框信息。
#[tauri::command]
pub fn about_info() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("product".into(), "NavKit".into());
    m.insert("version".into(), env!("CARGO_PKG_VERSION").into());
    m.insert("copyright".into(), "Copyright © 2026 NavKit Contributors".into());
    m.insert("publisher".into(), "NavKit".into());
    m.insert("url".into(), String::new());
    m.insert("support".into(), String::new());
    m.insert("dataDir".into(), storage::data_dir().to_string_lossy().to_string());
    m
}

// ---------------------------------------------------------------- 数据字典导出

/// 存储过程 / 函数源码读取的内部实现（字典导出与命令共用）。
pub(crate) async fn routine_definition_inner(
    state: &AppState,
    id: &str,
    schema_name: &str,
    name: &str,
    routine_type: &str,
) -> CmdResult<String> {
    let conns = state.store.connections();
    let kind = conns
        .iter()
        .find(|c| c.id == id)
        .map(|c| c.kind.clone())
        .ok_or_else(|| AppError::new("连接不存在"))?;
    if kind.protocol() == crate::models::Protocol::Sqlite {
        return Err(AppError::new("SQLite 不支持存储过程 / 函数"));
    }
    let db = ensure_db(state, id).await?;
    match db.as_ref() {
        db::Db::Mysql(pool) => {
            let routine = crate::mysql_routines::RoutineKind::parse(routine_type).map_err(AppError::from)?;
            crate::mysql_routines::definition(pool, &kind, schema_name, name, routine).await.map_err(AppError::from)
        }
        db::Db::Postgres(pool) => pg_routine_definition(pool, schema_name, name, routine_type).await,
        db::Db::Sqlite(_) => Err(AppError::new("SQLite 不支持存储过程 / 函数")),
        _ => Err(AppError::new("不支持的连接类型")),
    }
}

/// 采集数据字典元数据（表 / 视图 / 存储过程 + 列定义），供导出器生成文档。
#[tauri::command]
pub async fn dictionary_metadata(
    state: State<'_, AppState>,
    id: String,
    schema_name: String,
) -> CmdResult<crate::data_dictionary::DictDatabase> {
    let _activity_guard = state.activity.enter()?;
    let kind = kind_of(&state, &id)?;
    let db_type = kind.label().to_string();
    let db = scoped_db(&state, &id, &schema_name).await?;

    let mut tables = Vec::new();
    let mut views = Vec::new();
    for (name, ty) in schema::list_tables(db.as_ref(), kind.clone(), &schema_name)
        .await
        .map_err(AppError::from)?
    {
        if ty == ObjectType::View {
            // 视图：定义 + 说明（多数后端 table_schema 可读视图注释；失败降级为空）
            let script = crate::schema::view_definition(db.as_ref(), kind.clone(), &schema_name, &name)
                .await
                .unwrap_or_default();
            let comment = schema::table_schema(db.as_ref(), kind.clone(), &schema_name, &name)
                .await
                .ok()
                .and_then(|s| s.comment)
                .unwrap_or_default();
            views.push(crate::data_dictionary::DictObject { name, comment, script, db_type: db_type.clone() });
        } else {
            let ts = schema::table_schema(db.as_ref(), kind.clone(), &schema_name, &name)
                .await
                .map_err(AppError::from)?;
            let mut columns = Vec::new();
            for (i, col) in ts.columns.iter().enumerate() {
                let length = match (col.length, col.precision) {
                    (Some(l), _) => l.to_string(),
                    (None, Some(p)) => match col.scale {
                        Some(s) => format!("{p},{s}"),
                        None => p.to_string(),
                    },
                    _ => String::new(),
                };
                columns.push(crate::data_dictionary::DictColumn {
                    order: i + 1,
                    name: col.name.clone(),
                    type_name: col.data_type.clone(),
                    length,
                    primary_key: col.primary_key,
                    auto_increment: col.auto_increment,
                    nullable: col.nullable,
                    default_value: col.default.clone().unwrap_or_default(),
                    comment: col.comment.clone().unwrap_or_default(),
                });
            }
            tables.push(crate::data_dictionary::DictTable {
                name,
                comment: ts.comment.unwrap_or_default(),
                db_type: db_type.clone(),
                columns,
            });
        }
    }

    let mut routines = Vec::new();
    if !matches!(kind.protocol(), crate::models::Protocol::Sqlite) {
        for (ty, name) in schema::list_routines(db.as_ref(), kind.clone(), &schema_name)
            .await
            .map_err(AppError::from)?
        {
            let routine_type = if ty == ObjectType::Procedure { "procedure" } else { "function" };
            let script = routine_definition_inner(&state, &id, &schema_name, &name, routine_type)
                .await
                .unwrap_or_default();
            let comment = String::new();
            routines.push(crate::data_dictionary::DictObject { name, comment, script, db_type: db_type.clone() });
        }
    }

    Ok(crate::data_dictionary::DictDatabase {
        doc_title: format!("{schema_name}数据库设计文档"),
        name: schema_name,
        tables,
        views,
        routines,
    })
}

/// 数据字典导出到文件（生成器在 data_dictionary 模块；元数据由 dictionary_metadata 提供）。
#[tauri::command]
pub async fn dictionary_export(
    dto: crate::data_dictionary::DictDatabase,
    opts: crate::data_dictionary::DictExportOptions,
    file_path: String,
) -> CmdResult<u64> {
    // 导出是纯 CPU/IO 操作：放到阻塞线程池，避免卡住命令运行时
    let result = tokio::task::spawn_blocking(move || {
        crate::data_dictionary::export_to_file(&dto, &opts, std::path::Path::new(&file_path))
    })
    .await
    .map_err(|e| AppError::new(format!("导出任务失败: {e}")))?;
    result.map_err(AppError::from)
}

#[cfg(test)]
mod command_tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn temporary_dir() -> PathBuf {
        let root = std::env::temp_dir().join(format!("navkit-command-files-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        root
    }

    fn config(id: &str, kind: &str, file: &Path) -> ConnectionConfig {
        ConnectionConfig { id: id.into(), name: id.into(), kind: DbKind::of(kind), host: "127.0.0.1".into(),
            port: 0, user: String::new(), password: String::new(), database: String::new(),
            file: file.to_string_lossy().into(), params: String::new(), group: String::new(), read_only: false, keep_alive: false }
    }

    #[test]
    fn sql_log_preserves_original_sql_and_execution_metadata_on_disk() {
        use crate::execution_contracts::{DeadlineInfo, ExecutionOutcome, ExecutionState};

        let root = temporary_dir();
        let path = root.join("sql_log.jsonl");
        let cfg = config("历史连接", "mysql", Path::new(""));
        let sql = format!(
            " \t-- 保留注释、缩进和空行\r\nSELECT '单引号''与双引号\"', 'C:\\路径', '<script>&中文';\r\n\r\n/* {} */\nSELECT 2;  ",
            "长 SQL 正文".repeat(6000),
        );
        for (state, origin) in [
            (ExecutionState::Succeeded, "query"),
            (ExecutionState::Failed, "monitor"),
            (ExecutionState::Cancelled, "job"),
            (ExecutionState::TimedOut, "tool"),
        ] {
            let ok = state == ExecutionState::Succeeded;
            let execution_id = uuid::Uuid::new_v4().to_string();
            let outcome = ExecutionOutcome {
                execution_id: execution_id.clone(),
                state,
                result: ok.then(|| MultiResult { sets: vec![], messages: vec![], elapsed_ms: 42, spool_id: None }),
                error: (!ok).then(|| AppError::coded("TEST_EXECUTION_ERROR", "测试失败原因")),
                deadline: DeadlineInfo {
                    execution_id: execution_id.clone(), budget_ms: Some(100), elapsed_ms: 42,
                    deadline_reached: state == ExecutionState::TimedOut, stop_confirmed: !ok,
                },
                effects: "none".into(), transaction_state: "none".into(), resources_pending: false,
            };
            storage::append_sql_log_at(&path, &sql_log_entry(&cfg, "业务库", &sql, &outcome, origin));
            let records = storage::read_sql_log_at(&path, 1);
            let restored = &records[0];
            assert_eq!(restored.sql.as_bytes(), sql.as_bytes(), "SQL 原文丢失：{state:?}");
            assert_eq!(restored.connection_id, cfg.id);
            assert_eq!(restored.connection_name, cfg.name);
            assert_eq!(restored.kind, "mysql");
            assert_eq!(restored.database, "业务库");
            assert_eq!(restored.origin, origin);
            assert_eq!(restored.ok, ok);
            assert_eq!(restored.elapsed_ms, 42);
            assert_eq!(restored.rows, if ok { 0 } else { -1 });
            assert_eq!(restored.operation_id.as_deref(), Some(execution_id.as_str()));
            assert_eq!(restored.execution_state, Some(state));
        }
        assert_eq!(storage::read_sql_log_at(&path, 10).len(), 4);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sql_file_limits_bom_and_utf8() {
        let root = temporary_dir();
        let sql = root.join("input.SQL");
        std::fs::write(&sql, b"\xEF\xBB\xBFSELECT 1;").unwrap();
        assert_eq!(read_sql_file(sql.to_string_lossy().into()).unwrap(), "SELECT 1;");
        std::fs::write(&sql, [0xff, 0xfe]).unwrap();
        assert!(read_sql_file(sql.to_string_lossy().into()).is_err());
        let file = std::fs::File::create(&sql).unwrap();
        file.set_len(16 * 1024 * 1024 + 1).unwrap(); drop(file);
        assert!(read_sql_file(sql.to_string_lossy().into()).is_err());
        let other = root.join("input.txt"); std::fs::write(&other, "SELECT 1").unwrap();
        assert!(read_sql_file(other.to_string_lossy().into()).is_err());
        let directory = root.join("directory.sql"); std::fs::create_dir(&directory).unwrap();
        assert!(read_sql_file(directory.to_string_lossy().into()).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pg_startup_namespace_escaping() {
        let kind = DbKind::of("postgresql");
        assert_eq!(pg_startup_search_path(&kind, "public"), "\"public\"");
        assert_eq!(pg_startup_search_path(&kind, "a b\\c\"d"), "\"a\\ b\\\\c\"\"d\"");
        assert_eq!(pg_startup_search_path(&kind, "s -c role=evil"), "\"s\\ -c\\ role=evil\"");
    }

    /// Opt-in real PostgreSQL regression: all objects live in UUID schemas and
    /// are dropped before surfacing any assertion/query error. No Store/user data.
    #[tokio::test]
    async fn pg_routine_definitions_overloads_and_procedures() {
        if std::env::var("NAVKIT_COMMAND_TEST_PG").as_deref() != Ok("1") { return; }
        let pool = sqlx::postgres::PgPoolOptions::new().max_connections(3)
            .connect_with(sqlx::postgres::PgConnectOptions::new()
                .host("127.0.0.1").port(5433).username("postgres").database("postgres"))
            .await.unwrap();
        let kind = DbKind::of("postgresql");
        let namespace = format!("navkit_routine_{} space'\"", uuid::Uuid::new_v4().simple());
        let other_namespace = format!("navkit_routine_other_{}", uuid::Uuid::new_v4().simple());
        let name = "overloaded 'routine\"";
        let full = db::qualified(&kind, &namespace, name);
        let other_full = db::qualified(&kind, &other_namespace, name);
        let result: anyhow::Result<()> = async {
            for schema in [&namespace, &other_namespace] {
                crate::sql_history::query(&format!("CREATE SCHEMA {}", db::quote_ident(&kind, schema))).execute(&pool).await?;
            }
            crate::sql_history::query(&format!("CREATE FUNCTION {}(arg integer) RETURNS integer LANGUAGE sql AS $$ SELECT arg + 1 $$", full)).execute(&pool).await?;
            crate::sql_history::query(&format!("CREATE FUNCTION {}(arg text) RETURNS text LANGUAGE sql AS $$ SELECT arg || ':ok' $$", full)).execute(&pool).await?;
            crate::sql_history::query(&format!("CREATE PROCEDURE {}(arg boolean) LANGUAGE plpgsql AS $$ BEGIN NULL; END; $$", full)).execute(&pool).await?;
            crate::sql_history::query(&format!("CREATE FUNCTION {}(arg integer) RETURNS integer LANGUAGE sql AS $$ SELECT 999 $$", other_full)).execute(&pool).await?;

            let functions = pg_routine_definition(&pool, &namespace, name, "function").await?;
            anyhow::ensure!(functions.matches("CREATE OR REPLACE FUNCTION").count() == 2, "must return both function overloads");
            anyhow::ensure!(functions.contains("arg integer") && functions.contains("arg text"), "both signatures must be preserved");
            anyhow::ensure!(!functions.contains("CREATE OR REPLACE PROCEDURE") && !functions.contains("999"), "function lookup leaked another kind/schema");
            anyhow::ensure!(functions == pg_routine_definition(&pool, &namespace, name, "function").await?, "overload ordering must be stable");
            let procedure = pg_routine_definition(&pool, &namespace, name, "procedure").await?;
            anyhow::ensure!(procedure.matches("CREATE OR REPLACE PROCEDURE").count() == 1 && procedure.contains("arg boolean"), "procedure definition/signature missing");
            anyhow::ensure!(!procedure.contains("CREATE OR REPLACE FUNCTION"), "procedure lookup returned function overloads");
            anyhow::ensure!(pg_routine_definition(&pool, &namespace, "missing", "function").await.is_err(), "missing routine should fail");
            anyhow::ensure!(pg_routine_definition(&pool, &other_namespace, name, "procedure").await.is_err(), "wrong prokind should fail");
            anyhow::ensure!(pg_routine_definition(&pool, "navkit_missing_routine_schema", name, "function").await.is_err(), "missing schema should fail");
            anyhow::ensure!(pg_routine_definition(&pool, &namespace, name, "trigger").await.is_err(), "unsupported object kind should fail");

            // Verify joined definitions really are executable CREATE SQL, rather
            // than only signature labels or an arbitrary first overload.
            crate::sql_history::query(&format!("DROP FUNCTION {}(integer)", full)).execute(&pool).await?;
            crate::sql_history::query(&format!("DROP FUNCTION {}(text)", full)).execute(&pool).await?;
            crate::sql_history::raw_sql(&functions).execute(&pool).await?;
            crate::sql_history::query(&format!("DROP PROCEDURE {}(boolean)", full)).execute(&pool).await?;
            crate::sql_history::raw_sql(&procedure).execute(&pool).await?;
            let integer: i32 = crate::sql_history::query_scalar(&format!("SELECT {}(41)", full)).fetch_one(&pool).await?;
            let text: String = crate::sql_history::query_scalar(&format!("SELECT {}('hello'::text)", full)).fetch_one(&pool).await?;
            anyhow::ensure!(integer == 42 && text == "hello:ok", "recreated overload bodies changed");
            crate::sql_history::query(&format!("CALL {}(TRUE)", full)).execute(&pool).await?;
            Ok(())
        }.await;
        let mut cleanup = Ok(());
        for schema in [&namespace, &other_namespace] {
            if let Err(error) = crate::sql_history::query(&format!("DROP SCHEMA IF EXISTS {} CASCADE", db::quote_ident(&kind, schema))).execute(&pool).await {
                cleanup = Err(error);
            }
        }
        pool.close().await;
        cleanup.unwrap();
        result.unwrap();
    }

    /// Store resolves data relative to the executable. Run a copied test binary
    /// from a fresh temp directory to exercise persistence without user data.
    #[tokio::test]
    async fn isolated_connection_and_namespace_commands() {
        if std::env::var("NAVKIT_COMMAND_ISOLATED").as_deref() != Ok("1") { return; }
        let root = storage::portable_root();
        assert!(root.starts_with(std::env::temp_dir()));
        assert!(root.file_name().unwrap().to_string_lossy().starts_with("navkit-command-tests-"));
        let state = AppState { store: Store::load(), registry: Arc::new(DbRegistry::default()), manual_transactions: Default::default(), activity: Default::default(), executions: Default::default(), mutations: Default::default(), query_results: Default::default(), connection_runtime: Default::default() };
        let sqlite_cfg = config("sqlite-test", "sqlite", &root.join("test.sqlite"));
        state.store.save_connections(vec![sqlite_cfg.clone()], vec!["group-a".into(), "group-b".into()]).unwrap();
        let conn = ensure_db(&state, "sqlite-test").await.unwrap();
        let db::Db::Sqlite(pool) = conn.as_ref() else { panic!("SQLite expected") };
        crate::sql_history::query("CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT)").execute(pool).await.unwrap();
        crate::sql_history::query("CREATE VIEW item_view AS SELECT * FROM items").execute(pool).await.unwrap();
        let again = ensure_db(&state, "sqlite-test").await.unwrap();
        assert!(Arc::ptr_eq(&conn, &again));
        state.registry.write().insert("sqlite-test@derived".into(), conn.clone());
        let configs = move_connection_config(&state, "sqlite-test", "group-b").unwrap();
        assert_eq!(configs[0].group, "group-b");
        assert!(Arc::ptr_eq(&conn, &db::get_db(&state.registry, "sqlite-test").unwrap()));
        assert!(state.registry.read().contains_key("sqlite-test@derived"));
        assert!(move_connection_config(&state, "sqlite-test", "missing").is_err());
        assert!(move_connection_config(&state, "missing", "group-a").is_err());
        assert!(ensure_db(&state, "missing").await.is_err());
        assert!(scoped_db(&state, "sqlite-test", "missing").await.is_err());
        scoped_db(&state, "sqlite-test", "main").await.unwrap();
        assert!(validate_table_target(conn.as_ref(), &sqlite_cfg.kind, "main", "item_view", true).await.is_err());
        let mut readonly = sqlite_cfg.clone(); readonly.read_only = true;
        state.store.save_connections(vec![readonly], state.store.groups()).unwrap();
        assert!(require_writable_connection(&state, "sqlite-test").is_err());
        let mut bad_password = sqlite_cfg.clone(); bad_password.password = "invalid-ciphertext".into();
        state.store.save_connections(vec![bad_password], state.store.groups()).unwrap();
        assert!(ensure_db(&state, "sqlite-test").await.is_err());
        state.store.save_connections(vec![sqlite_cfg.clone()], state.store.groups()).unwrap();
        assert!(protect_export_path(&state, &sqlite_cfg.file, conn.as_ref()).await.is_err());
        assert!(protect_export_path(&state, &format!("{}-wal", sqlite_cfg.file), conn.as_ref()).await.is_err());
        let csv = root.join("no-header.csv"); std::fs::write(&csv, "1,first\n2,second\n").unwrap();
        let import = ImportOptions { connection_id: sqlite_cfg.id.clone(), schema: "main".into(), table: "items".into(),
            file: csv.to_string_lossy().into(), format: "csv".into(), has_header: false, delimiter: ",".into(), encoding: "utf-8".into(), create_table: false, truncate: false, sheet: None, on_conflict: String::new(), conflict_columns: Vec::new() };
        assert_eq!(crate::transfer::import_csv(conn.as_ref(), sqlite_cfg.kind.clone(), &import).await.unwrap(), 2);
        let count: i64 = crate::sql_history::query_scalar("SELECT COUNT(*) FROM items WHERE id = 1 AND name = 'first'").fetch_one(pool).await.unwrap();
        assert_eq!(count, 1);
        crate::sql_history::query("CREATE TABLE empty_table(id INTEGER, name TEXT)").execute(pool).await.unwrap();
        let (names, rows) = crate::transfer::fetch_transfer_rows(conn.as_ref(), &sqlite_cfg.kind, "main", "empty_table", None, None).await.unwrap();
        assert_eq!(names, vec!["id", "name"]); assert!(rows.is_empty());
        let export = ExportOptions { connection_id: sqlite_cfg.id.clone(), schema: "main".into(), table: "items".into(),
            file: root.join("export.csv").to_string_lossy().into(), format: "csv".into(), has_header: false,
            delimiter: ",".into(), encoding: "utf-8".into(), where_clause: Some("1=1; DELETE FROM items".into()), limit: None };
        assert!(crate::transfer::validate_export(&export).is_err());
        db::close_with_derived(&state.registry, "sqlite-test");
        assert!(state.registry.read().is_empty());
        pool.close().await;

        if std::env::var("NAVKIT_COMMAND_TEST_PG").as_deref() == Ok("1") {
            let mut cfg = config("pg-test", "postgresql", Path::new(""));
            cfg.port = 5433; cfg.user = "postgres".into(); cfg.database = "postgres".into();
            state.store.save_connections(vec![cfg.clone()], state.store.groups()).unwrap();
            let conn = ensure_db(&state, &cfg.id).await.unwrap();
            let db::Db::Postgres(admin) = conn.as_ref() else { panic!("Postgres expected") };
            let prefix = format!("navkit_cmd_{}", uuid::Uuid::new_v4().simple());
            let spaces = [format!("{}_a", prefix), format!("{}_b space\\\"", prefix)];
            for (i, namespace) in spaces.iter().enumerate() {
                crate::sql_history::query(&format!("CREATE SCHEMA {}", db::quote_ident(&cfg.kind, namespace))).execute(admin).await.unwrap();
                crate::sql_history::query(&format!("CREATE TABLE {} (value INTEGER)", db::qualified(&cfg.kind, namespace, "items"))).execute(admin).await.unwrap();
                crate::sql_history::query(&format!("INSERT INTO {} VALUES ({})", db::qualified(&cfg.kind, namespace, "items"), i + 1)).execute(admin).await.unwrap();
            }
            let original: String = crate::sql_history::query_scalar("SHOW search_path").fetch_one(admin).await.unwrap();
            for (i, namespace) in spaces.iter().enumerate() {
                let scoped = scoped_db(&state, &cfg.id, namespace).await.unwrap(); assert!(Arc::ptr_eq(&conn, &scoped));
                let (_, query) = query_db(&state, &cfg.id, Some(namespace)).await.unwrap();
                let db::Db::Postgres(pool) = query.as_ref() else { unreachable!() };
                let value: i32 = crate::sql_history::query_scalar("SELECT value FROM items").fetch_one(pool).await.unwrap(); assert_eq!(value, i as i32 + 1);
                let actual_database: String = crate::sql_history::query_scalar("SELECT current_database()").fetch_one(pool).await.unwrap(); assert_eq!(actual_database, "postgres");
                let (_, cached) = query_db(&state, &cfg.id, Some(namespace)).await.unwrap(); assert!(Arc::ptr_eq(&query, &cached));
            }
            assert!(scoped_db(&state, &cfg.id, "navkit_cmd_missing_schema").await.is_err());
            assert_eq!(crate::sql_history::query_scalar::<_, String>("SHOW search_path").fetch_one(admin).await.unwrap(), original);
            for namespace in &spaces { crate::sql_history::query(&format!("DROP SCHEMA {} CASCADE", db::quote_ident(&cfg.kind, namespace))).execute(admin).await.unwrap(); }
            let pools = state.registry.write().drain().map(|(_, conn)| conn).collect::<Vec<_>>();
            for conn in pools { if let db::Db::Postgres(pool) = conn.as_ref() { pool.close().await; } }
        }

        if std::env::var("NAVKIT_COMMAND_TEST_MYSQL").as_deref() == Ok("1") {
            let mut cfg = config("mysql-test", "mysql", Path::new(""));
            cfg.port = 3306; cfg.user = "root".into(); cfg.password = storage::encrypt_password("123456").unwrap();
            state.store.save_connections(vec![cfg.clone()], state.store.groups()).unwrap();
            let conn = ensure_db(&state, &cfg.id).await.unwrap();
            let db::Db::Mysql(admin) = conn.as_ref() else { panic!("MySQL expected") };
            let prefix = format!("navkit_cmd_{}", uuid::Uuid::new_v4().simple());
            let spaces = [format!("{}_a", prefix), format!("{}_b", prefix)];
            for (i, namespace) in spaces.iter().enumerate() {
                crate::sql_history::query(&format!("CREATE DATABASE {}", db::quote_ident(&cfg.kind, namespace))).execute(admin).await.unwrap();
                let query = scoped_db(&state, &cfg.id, namespace).await.unwrap();
                let db::Db::Mysql(pool) = query.as_ref() else { unreachable!() };
                crate::sql_history::query("CREATE TABLE items(value INTEGER)").execute(pool).await.unwrap();
                crate::sql_history::query(&format!("INSERT INTO items VALUES ({})", i + 1)).execute(pool).await.unwrap();
                let value: i32 = crate::sql_history::query_scalar("SELECT value FROM items").fetch_one(pool).await.unwrap(); assert_eq!(value, i as i32 + 1);
                let actual_database: String = crate::sql_history::query_scalar("SELECT DATABASE()").fetch_one(pool).await.unwrap(); assert_eq!(&actual_database, namespace);
                let cached = scoped_db(&state, &cfg.id, namespace).await.unwrap(); assert!(Arc::ptr_eq(&query, &cached));
            }
            for namespace in &spaces { crate::sql_history::query(&format!("DROP DATABASE {}", db::quote_ident(&cfg.kind, namespace))).execute(admin).await.unwrap(); }
            let pools = state.registry.write().drain().map(|(_, conn)| conn).collect::<Vec<_>>();
            for conn in pools { if let db::Db::Mysql(pool) = conn.as_ref() { pool.close().await; } }
        }
    }
}

#[cfg(test)]
mod job_schedule_tests {
    use super::valid_daily_at;

    #[test]
    fn daily_at_accepts_24h_times() {
        assert!(valid_daily_at("00:00"));
        assert!(valid_daily_at("9:05"));
        assert!(valid_daily_at("23:59"));
        assert!(!valid_daily_at("24:00"));
        assert!(!valid_daily_at("12:60"));
        assert!(!valid_daily_at("12:5"));
        assert!(!valid_daily_at("noon"));
    }
}
