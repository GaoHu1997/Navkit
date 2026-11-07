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
}

// ---------------------------------------------------------------- 设置

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> AppSettings {
    state.store.settings()
}

#[tauri::command]
pub fn set_settings(state: State<'_, AppState>, settings: AppSettings) -> CmdResult<()> {
    state.store.set_settings(settings);
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
) -> CmdResult<Vec<ConnectionConfig>> {
    let _activity_guard = state.activity.exclusive()?;
    let _manual_guard = state.manual_transactions.ordinary(&[&config.id]).await?;
    let mut cfg = config;
    if cfg.id.is_empty() { return Err(AppError::new("连接 ID 不能为空")); }
    let supplied_password = password.is_some();
    if let Some(p) = password {
        cfg.password = state.store.encrypt_password(&p)?;
    }
    let changed_id = cfg.id.clone();
    let (conns, reopen) = state.store.update_connections(|conns, groups| {
        let reopen = match conns.iter_mut().find(|c| c.id == cfg.id) {
            Some(slot) => {
                if !supplied_password { cfg.password = slot.password.clone(); }
                let reopen = slot.requires_reopen(&cfg);
                *slot = cfg;
                reopen
            }
            None => { conns.push(cfg); false }
        };
        for c in conns.iter().filter(|c| !c.group.is_empty()) {
            if !groups.contains(&c.group) { groups.push(c.group.clone()); }
        }
        groups.sort();
        Ok((conns.clone(), reopen))
    }).map_err(AppError::new)?;
    if reopen { db::close_with_derived(&state.registry, &changed_id); }
    Ok(conns)
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
    let _activity_guard = state.activity.exclusive()?;
    let _manual_guard = state.manual_transactions.ordinary(&[&id]).await?;
    let conns = state.store.update_connections(|conns, _| {
        conns.retain(|c| c.id != id);
        Ok(conns.clone())
    }).map_err(AppError::new)?;
    db::close_with_derived(&state.registry, &id);
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

/// 测试连接。
#[tauri::command]
pub async fn test_connection(req: ConnectionRequest) -> CmdResult<TestResult> {
    match db::open(&req).await {
        Ok(conn) => {
            let mut version = String::new();
            let mut charset = String::new();
            match &conn {
                db::Db::Sqlite(pool) => {
                    if let Ok(row) =
                        sqlx::query("SELECT sqlite_version() AS v").fetch_one(pool).await
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
                    if let Ok(row) = sqlx::query("SELECT VERSION() AS v").fetch_one(pool).await {
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
                    if let Ok(row) = sqlx::query("SELECT version() AS v").fetch_one(pool).await {
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
                        sqlx::query("SHOW server_encoding").fetch_one(pool).await
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
    let conns = state.store.connections();
    let cfg = conns
        .iter()
        .find(|c| c.id == id)
        .ok_or_else(|| anyhow!("连接不存在"))?;
    let password = decrypt_or_err(&state.store, cfg)?;
    let mut req = db::request_from_config(cfg, &password);
    req.timeout_secs = state.store.settings().query_timeout_secs;
    // 重新打开前先清掉旧池（含按库派生池），保证用最新凭据
    db::close_with_derived(&state.registry, &id);
    db::open_and_register(&state.registry, &id, &req).await?;
    crate::storage::log_line("connection", &format!("打开连接 {}", id));
    Ok(TestResult {
        ok: true,
        message: "连接成功".into(),
        version: String::new(),
        charset: String::new(),
    })
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
    if let db::Db::Sqlite(pool) = conn.as_ref() { sqlx::query("SELECT 1").fetch_one(pool).await?; }
    Ok(connection_success("数据库/模式可用"))
}

/// 关闭连接。
#[tauri::command]
pub async fn close_connection(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let _activity_guard = state.activity.enter()?;
    let mut lease = state.manual_transactions.lease(&id).await;
    lease.close().await?;
    db::close_with_derived(&state.registry, &id);
    crate::storage::log_line("connection", &format!("关闭连接 {}", id));
    Ok(())
}

/// 关闭全部连接。
#[tauri::command]
pub async fn close_all_connections(state: State<'_, AppState>) -> CmdResult<()> {
    let _activity_guard = state.activity.enter()?;
    let _lease = state.manual_transactions.close_all().await?;
    db::close_all(&state.registry);
    Ok(())
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
) -> CmdResult<TableSchema> {
    let _activity_guard = state.activity.enter()?;
    let db = scoped_db(&state, &id, &schema_name).await?;
    schema::table_schema(db.as_ref(), kind_of(&state, &id)?, &schema_name, &table).await.map_err(AppError::from)
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
    schema::page_table(db.as_ref(), kind_of(&state, &id)?, &req).await.map_err(AppError::from)
}

/// 记录 SQL 执行日志（「历史日志」的 SQL 执行日志来源）；记录失败静默，不影响执行结果。
fn record_sql_log(
    state: &State<'_, AppState>,
    id: &str,
    database: &str,
    sql: &str,
    outcome: Result<&MultiResult, &AppError>,
    origin: &str,
) {
    let conns = state.store.connections();
    let Some(cfg) = conns.iter().find(|c| c.id == id) else { return };
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let (ok, elapsed_ms, rows, message) = match outcome {
        Ok(result) => {
            let (mut affected, mut returned) = (0u64, 0u64);
            for set in &result.sets {
                if set.ok {
                    if set.has_rows { returned += set.rows.len() as u64 } else { affected += set.affected }
                }
            }
            (true, result.elapsed_ms, if returned > 0 { returned as i64 } else { affected as i64 }, String::new())
        }
        Err(error) => (false, 0u64, -1i64, error.to_string().chars().take(500).collect()),
    };
    crate::storage::append_sql_log(&crate::models::SqlLogEntry {
        ts: now.to_string(),
        connection_id: id.to_string(),
        connection_name: cfg.name.clone(),
        kind: cfg.kind.0.clone(),
        database: database.to_string(),
        sql: sql.chars().take(20_000).collect(),
        ok,
        elapsed_ms,
        rows,
        message,
        origin: origin.to_string(),
    });
}

/// 执行 SQL。MySQL 按库缓存连接；PG 选项是 schema，使用隔离的
/// search_path 连接池，但始终连接到保存配置指定的真实数据库。
/// 每次执行（成功或失败）都会写入 SQL 执行日志；origin 标记调用来源。
#[tauri::command]
pub async fn execute_sql(
    state: State<'_, AppState>,
    id: String,
    sql: String,
    database: Option<String>,
    transaction_owner: Option<String>,
    origin: Option<String>,
) -> CmdResult<MultiResult> {
    let _activity_guard = state.activity.enter()?;
    let namespace = database.as_deref().map(str::trim).unwrap_or("");
    let origin = origin.as_deref().unwrap_or("tool").to_string();
    let mut lease = state.manual_transactions.lease(&id).await;
    match lease.execute(transaction_owner.as_deref(), namespace, &sql).await {
        Err(error) => {
            let error = AppError::from(error);
            record_sql_log(&state, &id, namespace, &sql, Err(&error), &origin);
            return Err(error);
        }
        Ok(Some(result)) => {
            record_sql_log(&state, &id, namespace, &sql, Ok(&result), &origin);
            return Ok(result);
        }
        Ok(None) => {}
    }
    let kind = kind_of(&state, &id)?;
    let (key, _conn) = query_db(&state, &id, Some(namespace)).await?;
    match db::execute(&state.registry, &key, &sql, kind).await {
        Ok(result) => {
            record_sql_log(&state, &id, namespace, &sql, Ok(&result), &origin);
            Ok(result)
        }
        Err(error) => {
            let error = AppError::from(error);
            record_sql_log(&state, &id, namespace, &sql, Err(&error), &origin);
            Err(error)
        }
    }
}

/// 读取 SQL 执行日志（最新优先；默认最多 2000 条，30 天内的记录）。
#[tauri::command]
pub fn list_sql_log(limit: Option<u16>) -> Vec<crate::models::SqlLogEntry> {
    crate::storage::read_sql_log(limit.unwrap_or(crate::storage::SQL_LOG_MAX_ENTRIES as u16) as usize)
}

/// 清空 SQL 执行日志。
#[tauri::command]
pub fn clear_sql_log(state: State<'_, AppState>) -> Result<(), String> {
    let _activity_guard = state.activity.enter().map_err(|e| e.to_string())?;
    crate::storage::log_line("sql-log", "用户清空了 SQL 执行日志");
    crate::storage::clear_sql_log();
    Ok(())
}

#[tauri::command]
pub async fn manual_transaction_start(state: State<'_, AppState>, id: String, schema: String, owner: String) -> CmdResult<crate::manual_transaction::ManualTransactionStatus> {
    let _activity_guard = state.activity.enter()?;
    start_manual_transaction(&state, &id, &schema, &owner).await
}

async fn start_manual_transaction(state: &AppState, id: &str, schema: &str, owner: &str) -> CmdResult<crate::manual_transaction::ManualTransactionStatus> {
    let mut lease = state.manual_transactions.lease(id).await;
    lease.require_inactive()?;
    require_writable_connection(state, id)?;
    let cfg = saved_config(state, id)?;
    if !matches!(cfg.kind.protocol(), Protocol::Sqlite | Protocol::Mysql | Protocol::Postgres) || cfg.params.starts_with(crate::driver_manager::PARAMS_PREFIX) {
        return Err(AppError::new("手动查询事务仅支持 SQLx 原生 SQLite / MySQL / PostgreSQL，当前驱动不支持"));
    }
    let schema = schema.trim();
    let (_, conn) = query_db(state, id, Some(schema)).await?;
    lease.start(conn.as_ref(), cfg.kind, schema.into(), owner.into()).await.map_err(AppError::from)
}

#[tauri::command]
pub async fn manual_transaction_status(state: State<'_, AppState>, id: String) -> CmdResult<crate::manual_transaction::ManualTransactionStatus> {
    let _activity_guard = state.activity.enter()?;
    saved_config(&state, &id)?;
    Ok(state.manual_transactions.lease(&id).await.status())
}

#[tauri::command]
pub async fn manual_transaction_finish(state: State<'_, AppState>, id: String, owner: String, commit: bool) -> CmdResult<crate::manual_transaction::ManualTransactionStatus> {
    let _activity_guard = state.activity.enter()?;
    state.manual_transactions.lease(&id).await.finish(&owner, commit).await.map_err(AppError::from)
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

/// 执行 DDL（表结构修改）。
#[tauri::command]
pub async fn execute_ddl(
    state: State<'_, AppState>,
    id: String,
    sql: String,
) -> CmdResult<u64> {
    let _activity_guard = state.activity.enter()?;
    let _manual_guard = state.manual_transactions.ordinary(&[&id]).await?;
    require_writable_connection(&state, &id)?;
    let kind = kind_of(&state, &id)?;
    let db = ensure_db(&state, &id).await?;
    schema::exec_affected(db.as_ref(), &kind, &sql).await.map_err(AppError::from)
}

// ---------------------------------------------------------------- 收藏 / 最近

#[tauri::command]
pub fn list_favorites(state: State<'_, AppState>) -> Vec<Favorite> {
    state.store.favorites()
}

#[tauri::command]
pub fn add_favorite(state: State<'_, AppState>, favorite: Favorite) -> CmdResult<Vec<Favorite>> {
    let mut items = state.store.favorites();
    items.retain(|f| f.id != favorite.id);
    items.insert(0, favorite);
    state.store.save_favorites(items.clone());
    Ok(items)
}

#[tauri::command]
pub fn remove_favorite(state: State<'_, AppState>, id: String) -> CmdResult<Vec<Favorite>> {
    let mut items = state.store.favorites();
    items.retain(|f| f.id != id);
    state.store.save_favorites(items.clone());
    Ok(items)
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
    let (names, rows) = crate::transfer::fetch_transfer_rows(
        db.as_ref(),
        &kind,
        &opts.schema,
        &opts.table,
        opts.where_clause.as_deref(),
        opts.limit,
    )
    .await
    .map_err(AppError::from)?;
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

#[tauri::command]
pub async fn run_sync(
    state: State<'_, AppState>,
    opts: crate::sync::SyncOptions,
) -> CmdResult<crate::sync::SyncReport> {
    let _activity_guard = state.activity.enter()?;
    let _manual_guard = state.manual_transactions.ordinary(&[&opts.source_connection, &opts.target_connection]).await?;
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
    crate::sync::synchronize(source.as_ref(), &source_kind, target.as_ref(), &target_kind, &opts)
        .await.map_err(AppError::from)
}

/// 比较源表与目标表的差异（行数/结构/按主键的缺失、多出与内容差异）。只读，不修改数据。
#[tauri::command]
pub async fn sync_diff(
    state: State<'_, AppState>,
    req: crate::sync::SyncDiffRequest,
) -> CmdResult<crate::sync::SyncDiffReport> {
    let _activity_guard = state.activity.enter()?;
    let source_kind = kind_of(&state, &req.source_connection)?;
    let target_kind = kind_of(&state, &req.target_connection)?;
    if !crate::sync::supported(&source_kind) || !crate::sync::supported(&target_kind) {
        return Err(AppError::new("差异比较仅支持 SQLite、MySQL/MariaDB 与 PostgreSQL 协议数据库"));
    }
    let source = scoped_db(&state, &req.source_connection, &req.source_schema).await?;
    let target = scoped_db(&state, &req.target_connection, &req.target_schema).await?;
    crate::sync::diff(source.as_ref(), &source_kind, target.as_ref(), &target_kind, &req).await.map_err(AppError::from)
}

/// 整库同步（库到库）：逐表独立事务，单表失败继续并在报告中记录。
#[tauri::command]
pub async fn run_sync_database(
    state: State<'_, AppState>,
    req: crate::sync::DbSyncRequest,
) -> CmdResult<Vec<crate::sync::SyncTableReport>> {
    let _activity_guard = state.activity.enter()?;
    let _manual_guard = state.manual_transactions.ordinary(&[&req.source_connection, &req.target_connection]).await?;
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
    let result = crate::sync::synchronize_database(source.as_ref(), &source_kind, target.as_ref(), &target_kind, &req).await;
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
    result.map_err(AppError::from)
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
            // 注释写在文件头 "-- 注释     : xxx"
            let comment = std::fs::read_to_string(&p)
                .unwrap_or_default()
                .lines()
                .take(12)
                .find_map(|l| l.trim().strip_prefix("-- 注释"))
                .map(|s| {
                    s.trim_start_matches(|c: char| c == ' ' || c == ':' || c == '：')
                        .trim()
                        .to_string()
                })
                .unwrap_or_default();
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

/// 创建整库备份（转储为 SQL 文件），返回 (数据行数, 文件字节数)。
#[tauri::command]
pub async fn create_backup(state: State<'_, AppState>, opts: BackupOptions) -> CmdResult<(u64, u64)> {
    let _activity_guard = state.activity.enter()?;
    let id = opts.connection_id.clone();
    let kind = kind_of(&state, &id)?;
    let db = scoped_db(&state, &id, &opts.schema).await?;
    let dir = storage::backup_dir(&id, &opts.schema);
    let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let file = dir.join(format!("{}_{}.sql", opts.schema, stamp));
    let (rows, bytes) = crate::transfer::dump_database(
        db.as_ref(),
        &kind,
        &opts.schema,
        &opts.tables,
        opts.include_structure,
        opts.include_data,
        &file.to_string_lossy(),
        &opts.comment,
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
            rows,
            bytes
        ),
    );
    Ok((rows, bytes))
}

/// 还原备份：逐条执行备份文件里的 SQL 语句，返回每条语句的执行结果
/// （供前端「还原备份」对话框展示执行过程 / 语句 / 结果）。
#[tauri::command]
pub async fn restore_backup_detailed(
    state: State<'_, AppState>,
    connection_id: String,
    schema_name: String,
    file: String,
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
    let report =
        crate::transfer::restore_sql_file_detailed(db.as_ref(), &kind, &path.to_string_lossy())
            .await
            .map_err(AppError::from)?;
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
            if let Ok(row) = sqlx::query("SELECT sqlite_version() AS v").fetch_one(pool).await {
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
            if let Ok(rows) = sqlx::query(
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
            if let Ok(row) = sqlx::query("SELECT version() AS v").fetch_one(pool).await {
                let cols = row.columns();
                if let Some(i) = cols.iter().position(|c| c.name() == "v") {
                    info.version = crate::values::pg_row_values(&row, cols.len())
                        .get(i)
                        .cloned()
                        .flatten()
                        .unwrap_or_default();
                }
            }
            if let Ok(row) = sqlx::query("SHOW server_encoding").fetch_one(pool).await {
                let cols = row.columns();
                if let Some(i) = cols.iter().position(|c| c.name() == "server_encoding") {
                    info.charset = crate::values::pg_row_values(&row, cols.len())
                        .get(i)
                        .cloned()
                        .flatten()
                        .unwrap_or_default();
                }
            }
            if let Ok(row) = sqlx::query("SHOW server_version").fetch_one(pool).await {
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
    let rows = sqlx::query(
        "SELECT user, host, ssl_type, max_questions, max_updates, max_connections, \
         max_user_connections, Super_priv FROM mysql.user ORDER BY user, host",
    )
    .fetch_all(pool)
    .await
    .map_err(AppError::from)?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let cols = r.columns();
        let vals = crate::values::my_row_values(r, cols.len());
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
            let full = db::qualified(&kind, &schema_name, &name);
            let head = if routine_type == "procedure" { "PROCEDURE" } else { "FUNCTION" };
            let rows = sqlx::query(&format!("SHOW CREATE {} {}", head, full))
                .fetch_all(pool)
                .await
                .map_err(AppError::from)?;
            let Some(r) = rows.first() else {
                return Err(AppError::new("对象不存在"));
            };
            let cols = r.columns();
            let vals = crate::values::my_row_values(r, cols.len());
            let ddl = vals.get(2).cloned().flatten().unwrap_or_default();
            if ddl.is_empty() {
                Err(AppError::new("对象定义获取失败"))
            } else {
                Ok(ddl)
            }
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
    let definitions: Vec<String> = sqlx::query_scalar(
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

/// 立即执行一个任务：打开连接 → 执行 SQL → 记录状态与历史。
#[tauri::command]
pub async fn run_job_now(
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<(u64, String)> {
    let _activity_guard = state.activity.enter()?;
    let mut jobs: Vec<ScheduleJob> = storage::read_json(&storage::jobs_path()).unwrap_or_default();
    let Some(job) = jobs.iter().find(|j| j.id == id).cloned() else {
        return Err(AppError::new("任务不存在"));
    };
    let started = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let start = std::time::Instant::now();

    // 确保连接可用（未打开则先打开）
    let result = (|| async {
        let _manual_guard = state.manual_transactions.ordinary(&[&job.connection_id]).await?;
        let kind = kind_of(&state, &job.connection_id)?;
        let (target_key, _conn) = query_db(&state, &job.connection_id, Some(&job.database)).await?;
        db::execute(&state.registry, &target_key, &job.sql, kind)
            .await
            .map_err(AppError::from)
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
#[tauri::command]
pub async fn completion_data(
    state: State<'_, AppState>,
    id: String,
    schema_name: String,
) -> CmdResult<HashMap<String, Vec<String>>> {
    let _activity_guard = state.activity.enter()?;
    let kind = kind_of(&state, &id)?;
    let db = scoped_db(&state, &id, &schema_name).await?;
    load_completion_data(db.as_ref(), kind, &schema_name).await
}

async fn load_completion_data(
    db: &db::Db,
    kind: DbKind,
    schema_name: &str,
) -> CmdResult<HashMap<String, Vec<String>>> {
    use futures::StreamExt;

    let tables = schema::list_tables(db, kind.clone(), schema_name).await?;
    // 保留 __tables__ 包含全部可查询对象的旧协议，另提供视图类型。
    let names = tables.iter().map(|(name, _)| name.clone()).collect();
    let views = tables.iter().filter(|(_, ty)| matches!(ty, ObjectType::View))
        .map(|(name, _)| name.clone()).collect();
    let mut out = HashMap::new();
    let jobs: Vec<_> = tables.into_iter().map(|(name, _): (String, ObjectType)| {
        let kind = kind.clone();
        async move {
            let result = schema::table_schema(db, kind, schema_name, &name).await;
            (name, result)
        }
    }).collect();
    let fields = futures::stream::iter(jobs).buffer_unordered(4).collect::<Vec<_>>().await;
    for (name, result) in fields {
        match result {
            Ok(info) => {
                out.insert(name.clone(), info.columns.iter().map(|c| c.name.clone()).collect());
            }
            Err(error) => {
                // 一张表无权限时仍提示其它表，并留下可诊断的真实错误。
                storage::log_line("completion", &format!("读取 {}.{} 字段失败：{}", schema_name, name, error));
            }
        }
    }
    out.insert("__tables__".into(), names);
    out.insert("__views__".into(), views);
    out.insert("__schemas__".into(), if schema_name.is_empty() { vec![] } else { vec![schema_name.to_string()] });
    Ok(out)
}

#[cfg(test)]
mod completion_tests {
    use super::*;

    #[tokio::test]
    async fn completion_uses_real_table_and_view_columns() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1)
            .connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE users (id INTEGER PRIMARY KEY, display_name TEXT)")
            .execute(&pool).await.unwrap();
        sqlx::query("CREATE VIEW active_users AS SELECT id, display_name FROM users")
            .execute(&pool).await.unwrap();
        let data = load_completion_data(&db::Db::Sqlite(pool), DbKind::of("sqlite"), "main").await.unwrap();
        assert_eq!(data["__tables__"], vec!["active_users", "users"]);
        assert_eq!(data["__views__"], vec!["active_users"]);
        assert_eq!(data["__schemas__"], vec!["main"]);
        assert_eq!(data["users"], vec!["id", "display_name"]);
        assert_eq!(data["active_users"], vec!["id", "display_name"]);
    }
}

// ---------------------------------------------------------------- 工具

fn connection_success(message: &str) -> TestResult {
    TestResult { ok: true, message: message.into(), version: String::new(), charset: String::new() }
}

fn saved_config(state: &AppState, id: &str) -> CmdResult<ConnectionConfig> {
    state.store.connections().into_iter().find(|c| c.id == id)
        .ok_or_else(|| AppError::new("连接不存在"))
}

fn require_writable_connection(state: &AppState, id: &str) -> CmdResult<()> {
    if saved_config(state, id)?.read_only {
        return Err(AppError::new("目标连接为只读连接，不能写入数据或修改结构"));
    }
    Ok(())
}

/// Validate saved credentials even when registered. A concurrent ensure may open a
/// spare pool, but entry() never replaces the pool another command is using.
async fn ensure_db(state: &AppState, id: &str) -> CmdResult<Arc<db::Db>> {
    let cfg = saved_config(state, id)?;
    let password = decrypt_or_err(&state.store, &cfg)?;
    if let Ok(conn) = db::get_db(&state.registry, id) {
        return Ok(conn);
    }
    let mut req = db::request_from_config(&cfg, &password);
    req.timeout_secs = state.store.settings().query_timeout_secs;
    let conn = Arc::new(db::open(&req).await?);
    Ok(state.registry.write().entry(id.to_string()).or_insert(conn).clone())
}

/// Metadata/transfer schema is a PG namespace, but a MySQL database. Never put
/// a PG tree namespace into ConnectionRequest.database.
pub(crate) async fn scoped_db(state: &AppState, id: &str, namespace: &str) -> CmdResult<Arc<db::Db>> {
    let base = ensure_db(state, id).await?;
    if namespace.is_empty() {
        return Ok(base);
    }
    if namespace.contains('\0') {
        return Err(AppError::new("数据库/模式名包含空字符"));
    }
    match base.as_ref() {
        db::Db::Postgres(pool) => {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = $1 AND has_schema_privilege(oid, 'USAGE'))"
            ).bind(namespace).fetch_one(pool).await?;
            if !exists { return Err(AppError::new("PostgreSQL 模式不存在或没有访问权限")); }
            Ok(base)
        }
        db::Db::Mysql(_) => {
            let key = format!("{}@{}", id, namespace);
            if let Ok(conn) = db::get_db(&state.registry, &key) {
                // Opening an existing pool still validates that the server is reachable.
                if let db::Db::Mysql(pool) = conn.as_ref() {
                    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?")
                        .bind(namespace).fetch_one(pool).await?;
                    if exists == 0 { return Err(AppError::new("MySQL 数据库不存在或没有访问权限")); }
                }
                return Ok(conn);
            }
            let cfg = saved_config(state, id)?;
            let password = decrypt_or_err(&state.store, &cfg)?;
            let mut req = db::request_from_config(&cfg, &password);
            req.database = namespace.to_string();
            req.timeout_secs = state.store.settings().query_timeout_secs;
            let conn = Arc::new(db::open(&req).await?);
            Ok(state.registry.write().entry(key).or_insert(conn).clone())
        }
        _ => {
            let names = schema::list_databases(base.as_ref(), saved_config(state, id)?.kind).await?;
            if !names.iter().any(|(name, _)| name == namespace) {
                return Err(AppError::new("数据库/模式不存在或没有访问权限"));
            }
            Ok(base)
        }
    }
}

fn pg_startup_search_path(kind: &DbKind, namespace: &str) -> String {
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
    let Some(namespace) = namespace.filter(|s| !s.is_empty()) else {
        return Ok((id.into(), ensure_db(state, id).await?));
    };
    let conn = scoped_db(state, id, namespace).await?;
    match conn.as_ref() {
        db::Db::Postgres(pool) => {
            let key = format!("{}@schema:{}", id, namespace);
            if let Ok(conn) = db::get_db(&state.registry, &key) { return Ok((key, conn)); }
            let opts = pool.connect_options().as_ref().clone()
                .options([("search_path", pg_startup_search_path(&saved_config(state, id)?.kind, namespace))]);
            let pg = sqlx::postgres::PgPoolOptions::new().max_connections(5)
                .acquire_timeout(std::time::Duration::from_secs(state.store.settings().query_timeout_secs.max(1)))
                .connect_with(opts).await?;
            let conn = state.registry.write().entry(key.clone())
                .or_insert_with(|| Arc::new(db::Db::Postgres(pg))).clone();
            Ok((key, conn))
        }
        db::Db::Mysql(_) => Ok((format!("{}@{}", id, namespace), conn)),
        db::Db::Sqlite(_) => Ok((id.into(), conn)),
        db::Db::Http(http) if http.shares_session_namespace() => {
            // A tree namespace must never replace the saved JDBC database/URL.
            // The sidecar serializes namespace selection with each SQL request.
            let key = format!("{}@{}", id, namespace);
            let view = http.with_namespace(namespace);
            let conn = state.registry.write().entry(key.clone())
                .or_insert_with(|| Arc::new(db::Db::Http(view))).clone();
            Ok((key, conn))
        }
        _ => {
            // Preserve selected-container behavior for other native/HTTP/ODBC drivers.
            let key = format!("{}@{}", id, namespace);
            if let Ok(conn) = db::get_db(&state.registry, &key) { return Ok((key, conn)); }
            let cfg = saved_config(state, id)?;
            let password = decrypt_or_err(&state.store, &cfg)?;
            let mut req = db::request_from_config(&cfg, &password);
            req.database = namespace.to_string();
            req.timeout_secs = state.store.settings().query_timeout_secs;
            let conn = Arc::new(db::open(&req).await?);
            let conn = state.registry.write().entry(key.clone()).or_insert(conn).clone();
            Ok((key, conn))
        }
    }
}

#[cfg(test)]
#[path = "commands_h2_namespace_tests.rs"]
mod h2_namespace_tests;

#[cfg(test)]
#[path = "commands_jdbc_namespace_tests.rs"]
mod jdbc_namespace_tests;

async fn protect_export_path(state: &AppState, output: &str, conn: &db::Db) -> CmdResult<()> {
    crate::transfer::protect_database_path(conn, output).await?;
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
    crate::transfer::validate_names(&[table.to_string()])?;
    let objects = schema::list_tables(conn, kind.clone(), namespace).await?;
    match objects.iter().find(|(name, _)| name == table) {
        Some((_, ObjectType::Table)) => Ok(()),
        Some(_) => Err(AppError::new("目标必须是数据表，不能写入视图或其它对象")),
        None if allow_create => Ok(()),
        None => Err(AppError::new("目标表不存在，请启用建表或选择现有表")),
    }
}

/// 取连接的数据库类型。
fn kind_of(state: &State<'_, AppState>, id: &str) -> CmdResult<DbKind> {
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
                sqlx::query(&format!("CREATE SCHEMA {}", db::quote_ident(&kind, schema))).execute(&pool).await?;
            }
            sqlx::query(&format!("CREATE FUNCTION {}(arg integer) RETURNS integer LANGUAGE sql AS $$ SELECT arg + 1 $$", full)).execute(&pool).await?;
            sqlx::query(&format!("CREATE FUNCTION {}(arg text) RETURNS text LANGUAGE sql AS $$ SELECT arg || ':ok' $$", full)).execute(&pool).await?;
            sqlx::query(&format!("CREATE PROCEDURE {}(arg boolean) LANGUAGE plpgsql AS $$ BEGIN NULL; END; $$", full)).execute(&pool).await?;
            sqlx::query(&format!("CREATE FUNCTION {}(arg integer) RETURNS integer LANGUAGE sql AS $$ SELECT 999 $$", other_full)).execute(&pool).await?;

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
            sqlx::query(&format!("DROP FUNCTION {}(integer)", full)).execute(&pool).await?;
            sqlx::query(&format!("DROP FUNCTION {}(text)", full)).execute(&pool).await?;
            sqlx::raw_sql(&functions).execute(&pool).await?;
            sqlx::query(&format!("DROP PROCEDURE {}(boolean)", full)).execute(&pool).await?;
            sqlx::raw_sql(&procedure).execute(&pool).await?;
            let integer: i32 = sqlx::query_scalar(&format!("SELECT {}(41)", full)).fetch_one(&pool).await?;
            let text: String = sqlx::query_scalar(&format!("SELECT {}('hello'::text)", full)).fetch_one(&pool).await?;
            anyhow::ensure!(integer == 42 && text == "hello:ok", "recreated overload bodies changed");
            sqlx::query(&format!("CALL {}(TRUE)", full)).execute(&pool).await?;
            Ok(())
        }.await;
        let mut cleanup = Ok(());
        for schema in [&namespace, &other_namespace] {
            if let Err(error) = sqlx::query(&format!("DROP SCHEMA IF EXISTS {} CASCADE", db::quote_ident(&kind, schema))).execute(&pool).await {
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
        let state = AppState { store: Store::load(), registry: Arc::new(DbRegistry::default()), manual_transactions: Default::default(), activity: Default::default() };
        let sqlite_cfg = config("sqlite-test", "sqlite", &root.join("test.sqlite"));
        state.store.save_connections(vec![sqlite_cfg.clone()], vec!["group-a".into(), "group-b".into()]).unwrap();
        let conn = ensure_db(&state, "sqlite-test").await.unwrap();
        let db::Db::Sqlite(pool) = conn.as_ref() else { panic!("SQLite expected") };
        sqlx::query("CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT)").execute(pool).await.unwrap();
        sqlx::query("CREATE VIEW item_view AS SELECT * FROM items").execute(pool).await.unwrap();
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
            file: csv.to_string_lossy().into(), format: "csv".into(), has_header: false, delimiter: ",".into(), encoding: "utf-8".into(), create_table: false, truncate: false, sheet: None };
        assert_eq!(crate::transfer::import_csv(conn.as_ref(), sqlite_cfg.kind.clone(), &import).await.unwrap(), 2);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items WHERE id = 1 AND name = 'first'").fetch_one(pool).await.unwrap();
        assert_eq!(count, 1);
        sqlx::query("CREATE TABLE empty_table(id INTEGER, name TEXT)").execute(pool).await.unwrap();
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
                sqlx::query(&format!("CREATE SCHEMA {}", db::quote_ident(&cfg.kind, namespace))).execute(admin).await.unwrap();
                sqlx::query(&format!("CREATE TABLE {} (value INTEGER)", db::qualified(&cfg.kind, namespace, "items"))).execute(admin).await.unwrap();
                sqlx::query(&format!("INSERT INTO {} VALUES ({})", db::qualified(&cfg.kind, namespace, "items"), i + 1)).execute(admin).await.unwrap();
            }
            let original: String = sqlx::query_scalar("SHOW search_path").fetch_one(admin).await.unwrap();
            for (i, namespace) in spaces.iter().enumerate() {
                let scoped = scoped_db(&state, &cfg.id, namespace).await.unwrap(); assert!(Arc::ptr_eq(&conn, &scoped));
                let (_, query) = query_db(&state, &cfg.id, Some(namespace)).await.unwrap();
                let db::Db::Postgres(pool) = query.as_ref() else { unreachable!() };
                let value: i32 = sqlx::query_scalar("SELECT value FROM items").fetch_one(pool).await.unwrap(); assert_eq!(value, i as i32 + 1);
                let actual_database: String = sqlx::query_scalar("SELECT current_database()").fetch_one(pool).await.unwrap(); assert_eq!(actual_database, "postgres");
                let (_, cached) = query_db(&state, &cfg.id, Some(namespace)).await.unwrap(); assert!(Arc::ptr_eq(&query, &cached));
            }
            assert!(scoped_db(&state, &cfg.id, "navkit_cmd_missing_schema").await.is_err());
            assert_eq!(sqlx::query_scalar::<_, String>("SHOW search_path").fetch_one(admin).await.unwrap(), original);
            for namespace in &spaces { sqlx::query(&format!("DROP SCHEMA {} CASCADE", db::quote_ident(&cfg.kind, namespace))).execute(admin).await.unwrap(); }
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
                sqlx::query(&format!("CREATE DATABASE {}", db::quote_ident(&cfg.kind, namespace))).execute(admin).await.unwrap();
                let query = scoped_db(&state, &cfg.id, namespace).await.unwrap();
                let db::Db::Mysql(pool) = query.as_ref() else { unreachable!() };
                sqlx::query("CREATE TABLE items(value INTEGER)").execute(pool).await.unwrap();
                sqlx::query(&format!("INSERT INTO items VALUES ({})", i + 1)).execute(pool).await.unwrap();
                let value: i32 = sqlx::query_scalar("SELECT value FROM items").fetch_one(pool).await.unwrap(); assert_eq!(value, i as i32 + 1);
                let actual_database: String = sqlx::query_scalar("SELECT DATABASE()").fetch_one(pool).await.unwrap(); assert_eq!(&actual_database, namespace);
                let cached = scoped_db(&state, &cfg.id, namespace).await.unwrap(); assert!(Arc::ptr_eq(&query, &cached));
            }
            for namespace in &spaces { sqlx::query(&format!("DROP DATABASE {}", db::quote_ident(&cfg.kind, namespace))).execute(admin).await.unwrap(); }
            let pools = state.registry.write().drain().map(|(_, conn)| conn).collect::<Vec<_>>();
            for conn in pools { if let db::Db::Mysql(pool) = conn.as_ref() { pool.close().await; } }
        }
    }
}
