//! Owned opening, namespace derivation and physical resource disposal.
//! A deadline never drops an in-flight handshake: a worker closes late-created pools before releasing ownership.
use super::*;
use crate::ssh_tunnel::SshTunnel;

pub async fn open_prepared(req: &ConnectionRequest, read_only: bool, settings: &crate::models::AppSettings, prepared: runtime::PreparedRuntime) -> Result<OpenedConnection> {
    settings.validate_execution_settings()?;
    if let Some(http) = prepared.http {
        if req.kind.id() != "h2" || prepared.ssh.is_some() || prepared.tls.is_some() { return Err(anyhow!("HTTP_TUNNEL_TRANSPORT_CONFLICT")); }
        let config = crate::http_tunnel::TunnelConfig { endpoint: http.endpoint.to_string(), expected_server_id: http.profile.expected_server_id, expected_target_id: http.profile.expected_target_id, target_alias: http.profile.target_alias, connect_timeout_ms: http.profile.connect_timeout_ms, tls: http.tls, bearer: http.bearer };
        let db = crate::http_tunnel_adapter::HttpTunnelAdapter::connect(req, config, read_only).await?;
        return Ok(OpenedConnection { db: Db::Http(db), tunnel: None });
    }
    let native = runtime::transport_parameters(&req.params)?.is_some() && matches!(req.kind.protocol(), Protocol::Mysql | Protocol::Postgres);
    if native {
        let parameters = runtime::transport_parameters(&req.params)?.unwrap_or_default();
        let tls = match prepared.tls { Some(value) => value, None => crate::tls_options::parse(&parameters, true)? };
        return sqlx_runtime::open(req, Duration::from_secs(settings.pool_acquire_timeout_secs), read_only, tls, prepared.ssh).await;
    }
    if prepared.ssh.is_some() || prepared.tls.is_some() || prepared.profile_revision.is_some() {
        return Err(anyhow!("RUNTIME_PROTOCOL_UNSUPPORTED: 此驱动尚未证明本机通道身份和生命周期；不会忽略配置直连"));
    }
    let db = open_saved_inner(req, read_only, settings).await?;
    Ok(OpenedConnection { db, tunnel: None })
}

pub async fn open_saved_legacy(req: &ConnectionRequest, read_only: bool, settings: &crate::models::AppSettings) -> Result<Db> {
    settings.validate_execution_settings()?;
    // This lower-level compatibility entry has no saved profile owner. A reference cannot be silently ignored.
    if runtime::profile_reference(&req.params)?.is_some() { return Err(anyhow!("RUNTIME_PROFILE_SAVED_CONTEXT_REQUIRED")); }
    let request = req.clone(); let settings = settings.clone();
    let timeout = Duration::from_secs(settings.connect_timeout_secs);
    let (send, receive) = tokio::sync::oneshot::channel();
    crate::sql_history::spawn(async move {
        let result = open_saved_inner(&request, read_only, &settings).await;
        if let Err(result) = send.send(result) {
            if let Ok(db) = result { shutdown_db(&db).await; drop(db); }
        }
    });
    match tokio::time::timeout(timeout, receive).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(anyhow!("CONNECTION_OPEN_UNKNOWN: 连接工作任务失联；未登记连接，不自动重放")),
        Err(_) => Err(anyhow!("CONNECTION_OPEN_TIMEOUT: 连接阶段超时；后台保留握手并关闭任何迟到物理连接")),
    }
}

async fn open_saved_inner(req: &ConnectionRequest, read_only: bool, settings: &crate::models::AppSettings) -> Result<Db> {
    let acquire = Duration::from_secs(settings.pool_acquire_timeout_secs);
    if req.params.trim().starts_with(crate::driver_manager::PARAMS_PREFIX) { return super::open(req).await; }
    if req.kind.is_sqlite() {
        let file = sqlite_file(req)?;
        let options = SqliteConnectOptions::new().filename(&file).read_only(read_only).create_if_missing(!read_only)
            .busy_timeout(Duration::from_secs(settings.sqlite_busy_timeout_secs)).foreign_keys(true);
        let pool = SqlitePoolOptions::new().max_connections(4).acquire_timeout(acquire).connect_with(options).await?;
        return Ok(Db::Sqlite(pool));
    }
    match req.kind.protocol() {
        Protocol::Mysql => sqlx_runtime::open_legacy(req, acquire, read_only).await,
        Protocol::Postgres => sqlx_runtime::open_legacy(req, acquire, read_only).await,
        _ => super::open(req).await,
    }
}

/// Native SQLx pools close ALL physical sessions, including derived pools and checked-out sessions.
/// Other adapters retain their existing last-owner Drop contract; this function does not claim remote drain evidence for them.
pub async fn shutdown_db(db: &Db) {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    match db { Db::Sqlite(pool) => pool.close().await, Db::Mysql(pool) => pool.close().await, Db::Postgres(pool) => pool.close().await, Db::Http(http) => http.shutdown().await, _ => {} }

    }).await
}
pub fn take_with_derived(registry: &DbRegistry, id: &str) -> Vec<(String, Arc<Db>)> {
    let prefix = format!("{id}@"); let mut registry = registry.write();
    let keys: Vec<String> = registry.keys().filter(|key| key.as_str() == id || key.starts_with(&prefix)).cloned().collect();
    keys.into_iter().filter_map(|key| registry.remove(&key).map(|db| (key, db))).collect()
}
pub async fn shutdown_owned(databases: Vec<Arc<Db>>, tunnels: Vec<SshTunnel>) {
    // Shut the forwarding listener first: no new database sockets can enter a closing pool.
    for tunnel in &tunnels { tunnel.request_close(); }
    for db in &databases { shutdown_db(db).await; }
    // JDBC/H2 sidecar Drop is blocking and may wait for EOF/flush; never block the async executor thread.
    let dropped = tokio::task::spawn_blocking(move || drop(databases)).await;
    if dropped.is_err() { std::future::pending::<()>().await; }
    for tunnel in &tunnels { tunnel.wait_closed().await; }
}

pub async fn derive_mysql(base: &Arc<Db>, schema: &str, cfg: &ConnectionConfig, prepared: runtime::PreparedRuntime, tunnel: Option<SshTunnel>, settings: &crate::models::AppSettings) -> Result<Db> {
    let Db::Mysql(pool) = base.as_ref() else { return Err(anyhow!("MYSQL_NAMESPACE_BASE_INVALID")); };
    let parameters = runtime::transport_parameters(&cfg.params)?.ok_or_else(|| anyhow!("RUNTIME_DRIVER_CONFIG_CONFLICT"))?;
    let tls = prepared.tls.unwrap_or(crate::tls_options::parse(&parameters, true)?);
    sqlx_runtime::derive_mysql(pool, schema, &parameters, tls, tunnel, cfg.read_only, Duration::from_secs(settings.pool_acquire_timeout_secs)).await
}
pub async fn derive_pg(base: &Arc<Db>, schema: &str, cfg: &ConnectionConfig, prepared: runtime::PreparedRuntime, tunnel: Option<SshTunnel>, settings: &crate::models::AppSettings) -> Result<Db> {
    let Db::Postgres(pool) = base.as_ref() else { return Err(anyhow!("PG_NAMESPACE_BASE_INVALID")); };
    let parameters = runtime::transport_parameters(&cfg.params)?.ok_or_else(|| anyhow!("RUNTIME_DRIVER_CONFIG_CONFLICT"))?;
    let tls = prepared.tls.unwrap_or(crate::tls_options::parse(&parameters, true)?);
    let search_path = crate::commands::pg_startup_search_path(&cfg.kind, schema);
    sqlx_runtime::derive_pg(pool, &search_path, tls, tunnel, cfg.read_only, Duration::from_secs(settings.pool_acquire_timeout_secs)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn settings() -> crate::models::AppSettings { crate::models::AppSettings::default() }
    fn request() -> ConnectionRequest { ConnectionRequest { kind: DbKind::of("sqlite"), host: String::new(), port: 0, user: String::new(), password: String::new(), database: String::new(), file: ":memory:".into(), params: String::new(), timeout_secs: 1 } }
    #[tokio::test] async fn shutdown_closes_real_sqlite_pool_and_all_clones() {
        let Db::Sqlite(pool) = open_saved_inner(&request(), false, &settings()).await.unwrap() else { panic!("SQLite expected") };
        let clone = pool.clone(); shutdown_db(&Db::Sqlite(pool)).await;
        assert!(clone.is_closed()); assert!(crate::sql_history::query("SELECT 1").execute(&clone).await.is_err());
    }
    #[tokio::test] async fn take_derived_removes_only_full_connection_identity() {
        let registry = DbRegistry::default(); let db = Arc::new(open_saved_inner(&request(), false, &settings()).await.unwrap());
        registry.write().insert("c1".into(), db.clone()); registry.write().insert("c1@main".into(), db.clone()); registry.write().insert("c10".into(), db);
        assert_eq!(take_with_derived(&registry, "c1").len(), 2); assert!(registry.read().contains_key("c10"));
    }
    #[tokio::test] async fn ordinary_test_cannot_ignore_saved_profile_reference() {
        let mut req = request(); req.params = format!("runtime_profile_id={}", "a".repeat(64));
        assert!(open_saved_legacy(&req, false, &settings()).await.err().unwrap().to_string().contains("SAVED_CONTEXT_REQUIRED"));
    }
}
