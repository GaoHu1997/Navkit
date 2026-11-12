//! Saved connection admission and shutdown. Workers own admission/activity leases through real I/O drain.
use std::{future::Future, sync::Arc, time::Duration};
use tokio::sync::OwnedMutexGuard;
use crate::{commands::{AppState, saved_config}, db::{self, runtime::{self, PreparedRuntime, RuntimeSubmission}}, error::{AppError, CmdResult}, models::{ConnectionConfig, ConnectionRequest, TestResult}};

fn pending(message: &str) -> AppError { AppError::coded("CONNECTION_RESOURCES_PENDING", message) }
fn fingerprint(state: &AppState, cfg: &ConnectionConfig) -> CmdResult<String> {
    runtime::binding_fingerprint_at(cfg, state.store.data_root()).map_err(AppError::from)
}
fn password(state: &AppState, cfg: &ConnectionConfig) -> CmdResult<String> { state.store.decrypt_password(&cfg.password).map_err(AppError::from) }

/// If a worker panics while it owns physical work, keep the endpoint quarantined.
struct AdmissionGuard { runtime: runtime::RuntimeRegistry, id: String, settled: bool }
impl Drop for AdmissionGuard { fn drop(&mut self) { if !self.settled { let _ = self.runtime.begin_close(&self.id); } } }

async fn admit<F>(state: &AppState, cfg: ConnectionConfig, key: String, expected: String, gate: OwnedMutexGuard<()>, work: F, base: bool, inherited_tunnel: Option<crate::ssh_tunnel::SshTunnel>) -> CmdResult<Arc<db::Db>>
where F: Future<Output = anyhow::Result<db::OpenedConnection>> + Send + 'static {
    let activity = state.activity.enter()?;
    let registry = state.registry.clone();
    let runtime = state.connection_runtime.clone();
    let root = state.store.data_root().to_path_buf();
    let timeout = Duration::from_secs(state.store.settings().connect_timeout_secs);
    let (send, receive) = tokio::sync::oneshot::channel();
    crate::sql_history::spawn(async move {
        let _gate = gate; let _activity = activity;
        let mut ownership = AdmissionGuard { runtime: runtime.clone(), id: cfg.id.clone(), settled: false };
        tokio::pin!(work);
        let opened = tokio::select! {
            result = &mut work => result,
            _ = tokio::time::sleep(timeout) => {
                let _ = send.send(Err(AppError::coded("CONNECT_TIMEOUT", "连接阶段超时；后台仍持有租约，关闭迟到物理连接后才解除隔离")));
                if let Ok(opened) = work.await {
                    db::shutdown_owned(vec![Arc::new(opened.db)], opened.tunnel.into_iter().collect()).await;
                }
                ownership.settled = true;
                return;
            }
        };
        let mut opened_tunnel = None;
        let result = match opened {
            Err(error) => Err(AppError::from(error)),
            Ok(opened) => {
                let conn = Arc::new(opened.db);
                opened_tunnel = opened.tunnel.clone();
                let tunnel = opened.tunnel.or(inherited_tunnel);
                let verified = runtime::binding_fingerprint_at(&cfg, &root).map_err(AppError::from).and_then(|actual| {
                    if actual != expected { return Err(AppError::coded("CONNECTION_CONTEXT_CHANGED", "握手期间配置或证书材料改变；迟到连接不会登记")); }
                    if base { runtime.publish(&key, &expected, &conn, tunnel.as_ref(), runtime::keepalive_policy(&cfg).map_err(AppError::from)?) }
                    else { runtime.publish_derived(&key, &expected, &conn, tunnel.as_ref()) }.map_err(AppError::from)
                });
                if let Err(error) = verified {
                    db::shutdown_owned(vec![conn], opened_tunnel.take().into_iter().collect()).await;
                    Err(error)
                } else if send.is_closed() {
                    runtime.remove(&key);
                    db::shutdown_owned(vec![conn], opened_tunnel.take().into_iter().collect()).await;
                    ownership.settled = true;
                    return;
                } else {
                    crate::sql_history::register(&conn, &cfg, key.strip_prefix(&format!("{}@",cfg.id)).unwrap_or(""), &root);
                    registry.write().insert(key.clone(), conn.clone());
                    Ok(conn)
                }
            }
        };
        if let Err(Ok(conn)) = send.send(result) {
            // Cancellation can race is_closed. The gate/activity survive this physical cleanup.
            let removed = {
                let mut map = registry.write();
                if map.get(&key).map(|current| Arc::ptr_eq(current, &conn)).unwrap_or(false) { map.remove(&key); true } else { false }
            };
            if removed { runtime.remove(&key); }
            db::shutdown_owned(vec![conn], opened_tunnel.into_iter().collect()).await;
        }
        ownership.settled = true;
    });
    receive.await.map_err(|_| AppError::coded("CONNECTION_OPEN_UNKNOWN", "连接工作任务失联，结果未知；目标已隔离，不能自动重试"))?
}

pub(crate) async fn ensure(state: &AppState, id: &str) -> CmdResult<Arc<db::Db>> {
    state.executions.require_quiescent(id)?;
    let _activity = state.activity.enter()?;
    let cfg = saved_config(state, id)?;
    if crate::ssh_tunnel::profiles::commit::has_pending(state.store.data_root(), id)? {
        return Err(AppError::coded("RUNTIME_COMMIT_PENDING", "本机配置有未完成的保存记录；需先恢复一致状态，不能直连"));
    }
    let prepared = runtime::prepare_at(&cfg, state.store.data_root())?;
    let expected = prepared.fingerprint.clone();
    // Credential integrity is checked even for an already admitted pool.
    let secret = password(state, &cfg)?;
    let gate = tokio::time::timeout(Duration::from_secs(state.store.settings().connect_timeout_secs), state.connection_runtime.lock(id)).await
        .map_err(|_| pending("连接仍在握手或关闭；未启动第二次握手"))??;
    if let Ok(conn) = db::get_db(&state.registry, id) {
        if state.connection_runtime.matches(id, &expected, &conn) { return Ok(conn); }
        drop(gate);
        retire(state, id, ()).await?;
        return Err(AppError::coded("CONNECTION_CONTEXT_CHANGED", "原连接的配置、证书材料或隧道已失效；旧资源已关闭，请明确重新连接"));
    }
    let settings = state.store.settings(); settings.validate_execution_settings()?;
    let mut req = db::request_from_config(&cfg, &secret); req.timeout_secs = settings.connect_timeout_secs;
    let read_only = cfg.read_only;
    admit(state, cfg, id.into(), expected, gate, async move { db::open_prepared(&req, read_only, &settings, prepared).await }, true, None).await
}

/// Namespace pools clone the admitted physical options, TLS verification and tunnel owner.
pub(crate) async fn derived(state: &AppState, id: &str, namespace: &str, pg: bool) -> CmdResult<(String, Arc<db::Db>)> {
    if namespace.is_empty() || namespace.chars().any(|c| c == '\0') { return Err(AppError::coded("NAMESPACE_INVALID", "数据库/模式名为空或包含空字符")); }
    let base = ensure(state, id).await?;
    let _activity = state.activity.enter()?;
    let cfg = saved_config(state, id)?;
    let prepared = runtime::prepare_at(&cfg, state.store.data_root())?;
    let expected = prepared.fingerprint.clone();
    let key = if pg { format!("{id}@schema:{namespace}") } else { format!("{id}@{namespace}") };
    // The base gate orders namespace admission against base retirement.
    let gate = tokio::time::timeout(Duration::from_secs(state.store.settings().connect_timeout_secs), state.connection_runtime.lock(id)).await
        .map_err(|_| pending("命名空间连接的源连接尚在握手或关闭"))??;
    state.connection_runtime.require_openable(&key)?;
    if !state.connection_runtime.matches(id, &expected, &base) { return Err(AppError::coded("CONNECTION_CONTEXT_CHANGED", "派生连接的实际源连接已变化")); }
    if let Ok(conn) = db::get_db(&state.registry, &key) {
        if state.connection_runtime.matches(&key, &expected, &conn) { return Ok((key, conn)); }
        return Err(AppError::coded("CONNECTION_CONTEXT_CHANGED", "旧命名空间池未经当前运行配置验证；请关闭连接后重新打开"));
    }
    let tunnel = state.connection_runtime.tunnel(id);
    let inherited = tunnel.clone(); let settings = state.store.settings();
    let config = cfg.clone(); let schema = namespace.to_owned();
    let conn = admit(state, cfg, key.clone(), expected, gate, async move {
        let db = if pg { db::derive_pg(&base, &schema, &config, prepared, tunnel, &settings).await? }
            else { db::derive_mysql(&base, &schema, &config, prepared, tunnel, &settings).await? };
        Ok(db::OpenedConnection { db, tunnel: None })
    }, false, inherited).await?;
    Ok((key, conn))
}

/// All guards supplied by a close/save caller survive a timeout until physical drain.
pub(crate) async fn retire<G: Send + 'static>(state: &AppState, id: &str, guards: G) -> CmdResult<()> {
    let timeout = Duration::from_secs(state.store.settings().connect_timeout_secs);
    // gate 获取与 ensure/derived 同样受超时约束：握手中断时关闭操作不得无限等待（评审4-F4 附加项）。
    let gate = tokio::time::timeout(timeout, state.connection_runtime.lock(id)).await
        .map_err(|_| pending("连接仍在握手或关闭；关闭请求已按预算放弃等待，可稍后重试"))??;
    let tunnel = state.connection_runtime.tunnel(id);
    let token = state.connection_runtime.begin_close(id)?;
    let databases = db::take_with_derived(&state.registry, id).into_iter().map(|(_, db)| db).collect();
    state.mutations.clear_connection(id);
    state.query_results.clear_connection(id);
    crate::object_mutation::global().clear_connection(id);
    let (send, receive) = tokio::sync::oneshot::channel();
    crate::sql_history::spawn(async move {
        let _guards = guards; let _gate = gate;
        db::shutdown_owned(databases, tunnel.into_iter().collect()).await;
        token.complete(); let _ = send.send(());
    });
    match tokio::time::timeout(timeout, receive).await {
        Ok(Ok(())) => Ok(()),
        _ => Err(pending("物理连接仍在关闭；后台继续持有租约，排空完成前禁止重开和保活探测")),
    }
}

/// Acquire every distinct base gate before atomically marking or removing any connection.
pub(crate) async fn retire_many<G: Send + 'static>(state: &AppState, mut ids: Vec<String>, guards: G) -> CmdResult<()> {
    ids.sort(); ids.dedup();
    let timeout = Duration::from_secs(state.store.settings().connect_timeout_secs);
    let gates = tokio::time::timeout(timeout, async {
        let mut gates = Vec::with_capacity(ids.len());
        for id in &ids { gates.push(state.connection_runtime.lock(id).await?); }
        Ok::<_, anyhow::Error>(gates)
    }).await.map_err(|_| pending("连接仍在握手或关闭；批量关闭未改变其他连接"))??;
    let tunnels = ids.iter().filter_map(|id| state.connection_runtime.tunnel(id)).collect();
    let tokens = state.connection_runtime.begin_close_many(&ids)?;
    let mut databases = Vec::new();
    for id in &ids {
        databases.extend(db::take_with_derived(&state.registry, id).into_iter().map(|(_, db)| db));
        state.mutations.clear_connection(id);
        state.query_results.clear_connection(id);
        crate::object_mutation::global().clear_connection(id);
    }
    let (send, receive) = tokio::sync::oneshot::channel();
    crate::sql_history::spawn(async move {
        let _guards = guards; let _gates = gates;
        db::shutdown_owned(databases, tunnels).await;
        for token in tokens { token.complete(); }
        let _ = send.send(());
    });
    match tokio::time::timeout(timeout, receive).await {
        Ok(Ok(())) => Ok(()),
        _ => Err(pending("物理连接仍在关闭；后台继续持有租约，排空完成前禁止重开和保活探测")),
    }
}

/// HTTP/ODBC 等适配器的容器级命名空间连接（树命名空间 → 独立连接或共享会话视图）。
///
/// · shares_session_namespace 适配器（H2 / JDBC sidecar）：以 `with_namespace`
///   生成共享会话视图登记到注册表，树命名空间绝不覆盖保存的 JDBC 数据库/URL；
///   sidecar 逐条 SQL 串行切换命名空间。
/// · 其它适配器：以 namespace 打开真实新连接（key = "{id}@{namespace}"），走
///   admit 派生登记（含运行指纹校验与超时隔离）。
pub(crate) async fn container(state: &AppState, id: &str, namespace: &str, base: Arc<db::Db>) -> CmdResult<(String, Arc<db::Db>)> {
    let __sql_history_context = crate::sql_history::db_context(base.as_ref());
    crate::sql_history::scope(__sql_history_context, async {
    if namespace.is_empty() || namespace.chars().any(|c| c == '\0') {
        return Err(AppError::coded("NAMESPACE_INVALID", "数据库/模式名为空或包含空字符"));
    }
    let cfg = saved_config(state, id)?;
    let prepared = runtime::prepare_at(&cfg, state.store.data_root())?;
    let expected = prepared.fingerprint.clone();
    let key = format!("{id}@{namespace}");
    let gate = tokio::time::timeout(Duration::from_secs(state.store.settings().connect_timeout_secs), state.connection_runtime.lock(id)).await
        .map_err(|_| pending("命名空间连接的源连接尚在握手或关闭"))??;
    state.connection_runtime.require_openable(&key)?;
    if !state.connection_runtime.matches(id, &expected, &base) { return Err(AppError::coded("CONNECTION_CONTEXT_CHANGED", "派生连接的实际源连接已变化")); }
    if let Ok(conn) = db::get_db(&state.registry, &key) {
        if state.connection_runtime.matches(&key, &expected, &conn) { return Ok((key, conn)); }
        return Err(AppError::coded("CONNECTION_CONTEXT_CHANGED", "旧命名空间连接未经当前运行配置验证；请关闭连接后重新打开"));
    }
    if let db::Db::Http(http) = base.as_ref() {
        if http.shares_session_namespace() {
            let view = http.with_namespace(namespace);
            let conn = Arc::new(db::Db::Http(view));
            state.connection_runtime.publish_derived(&key, &expected, &conn, None).map_err(AppError::from)?;
            if let Some(previous) = state.registry.write().insert(key.clone(), conn.clone()) {
                drop(previous);
            }
            return Ok((key, conn));
        }
    }
    let settings = state.store.settings(); settings.validate_execution_settings()?;
    let secret = password(state, &cfg)?;
    let mut req = db::request_from_config(&cfg, &secret);
    req.database = namespace.to_string();
    req.timeout_secs = settings.connect_timeout_secs;
    let read_only = cfg.read_only;
    let conn = admit(state, cfg, key.clone(), expected, gate, async move {
        db::open_prepared(&req, read_only, &settings, prepared).await
    }, false, None).await?;
    Ok((key, conn))

    }).await
}

pub(crate) async fn health(state: &AppState, id: &str, principal: &str, owner: &str) -> CmdResult<crate::connection_health::ConnectionHealthView> {
    let activity = state.activity.enter()?;
    let cfg = saved_config(state, id)?;
    let reg = &state.connection_runtime.health;
    let Some(generation) = state.connection_runtime.generation(id) else { return Ok(reg.view(id, runtime::keepalive_policy(&cfg)?.interval_secs)); };
    let conn = db::get_db(&state.registry, id)?;
    let actual = fingerprint(state, &cfg)?;
    if !state.connection_runtime.matches(id, &actual, &conn) || generation.fingerprint != actual {
        return Err(AppError::coded("CONNECTION_CONTEXT_CHANGED", "保活目标已变化；没有发送探测SQL"));
    }
    if state.executions.has_active(id) { reg.record_busy(&generation, "execution_pending"); return reg.view_generation(&generation).map_err(AppError::from); }
    let Some(lease) = state.manual_transactions.try_ordinary(&[id]).await? else { reg.record_busy(&generation, "manual_transaction"); return reg.view_generation(&generation).map_err(AppError::from); };
    // The held slot lease excludes a query/manual transaction entering after this check.
    if state.executions.has_active(id) { reg.record_busy(&generation, "execution_pending"); return reg.view_generation(&generation).map_err(AppError::from); }
    let Some(ticket) = reg.try_begin(&generation, principal, owner)? else { return reg.view_generation(&generation).map_err(AppError::from); };
    Ok(crate::connection_health::probe_owned(conn, ticket, lease, activity).await)
}

#[tauri::command]
pub fn runtime_profile_view(state: tauri::State<'_, AppState>, id: String) -> CmdResult<Option<runtime::RuntimeProfileView>> {
    let _activity = state.activity.enter()?;
    if id.is_empty() || id.len() > 128 { return Err(AppError::coded("CONNECTION_ID_INVALID", "连接ID无效")); }
    crate::ssh_tunnel::profiles::Store::new(state.store.data_root()).view(&id).map_err(AppError::from)
}

#[tauri::command]
pub async fn test_runtime_connection(state: tauri::State<'_, AppState>, mut req: ConnectionRequest, id: String, submission: RuntimeSubmission, preserve_database_password: bool) -> CmdResult<TestResult> {
    let activity = state.activity.enter()?;
    let previous = state.store.connections().into_iter().find(|cfg| cfg.id == id);
    let mut cfg = runtime::request_config(&id, &req, previous.as_ref().map(|c| c.read_only).unwrap_or(false), false);
    if preserve_database_password {
        let saved = previous.as_ref().ok_or_else(|| AppError::coded("CONNECTION_NOT_FOUND", "没有可保留密码的已保存连接"))?;
        req.password = password(&state, saved)?; cfg.password = saved.password.clone();
    }
    let prepared: PreparedRuntime = runtime::prepare_transient(&cfg, state.store.data_root(), &submission)?;
    let settings = state.store.settings(); settings.validate_execution_settings()?;
    req.timeout_secs = settings.connect_timeout_secs;
    let (send, receive) = tokio::sync::oneshot::channel();
    let timeout = Duration::from_secs(settings.connect_timeout_secs);
    crate::sql_history::spawn(async move {
        let _activity = activity;
        let result = db::open_prepared(&req, cfg.read_only, &settings, prepared).await;
        let result = match result {
            Ok(opened) => { db::shutdown_owned(vec![Arc::new(opened.db)], opened.tunnel.into_iter().collect()).await; Ok(TestResult { ok: true, message: "连接成功；临时连接已关闭，本机配置未保存".into(), version: String::new(), charset: String::new() }) },
            Err(error) => Err(AppError::from(error)),
        };
        let _ = send.send(result);
    });
    match tokio::time::timeout(timeout, receive).await {
        Ok(Ok(result)) => result,
        _ => Err(AppError::coded("CONNECT_TIMEOUT", "测试超时；后台继续关闭迟到连接，未保存配置")),
    }
}
