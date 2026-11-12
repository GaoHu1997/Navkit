//! SQLx physical connections keep the SSH owner and verify actual session configuration.
use super::*;
use crate::{mysql_options::MysqlAdvancedOptions, ssh_tunnel::{SshConfig, SshTunnel}, tls_options::TlsOptions};

fn check_tunnel(tunnel: Option<&SshTunnel>) -> std::result::Result<(), sqlx::Error> {
    if tunnel.map(|tunnel| !tunnel.is_available()).unwrap_or(false) { return Err(sqlx::Error::Protocol("SSH_SESSION_CLOSED: SSH通道已关闭；不能把旧连接当作有效".into())); }
    Ok(())
}
fn mysql_options(timeout: Duration, read_only: bool, advanced: MysqlAdvancedOptions, tls: TlsOptions, tunnel: Option<SshTunnel>) -> MySqlPoolOptions {
    MySqlPoolOptions::new().max_connections(5).acquire_timeout(timeout).after_connect(move |connection, _| {
        let advanced = advanced.clone(); let tls = tls.clone(); let tunnel = tunnel.clone();
        Box::pin(async move {
            check_tunnel(tunnel.as_ref())?;
            crate::sql_history::query("SET sql_mode=CONCAT(@@session.sql_mode, ',NO_BACKSLASH_ESCAPES')").execute(&mut *connection).await?;
            advanced.verify_connection(&mut *connection).await.map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
            tls.verify_mysql_connection(&mut *connection).await.map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
            if read_only {
                crate::sql_history::query("SET SESSION TRANSACTION READ ONLY").execute(&mut *connection).await?;
                let flag = match crate::sql_history::query_scalar::<_, i64>("SELECT @@session.transaction_read_only").fetch_one(&mut *connection).await {
                    Ok(flag) => flag,
                    Err(_) => crate::sql_history::query_scalar::<_, i64>("SELECT @@session.tx_read_only").fetch_one(&mut *connection).await?,
                };
                if flag != 1 { return Err(sqlx::Error::Protocol("MySQL只读事务默认值未建立".into())); }
            }
            check_tunnel(tunnel.as_ref())?;
            Ok(())
        })
    })
}
fn pg_options(timeout: Duration, read_only: bool, tls: TlsOptions, tunnel: Option<SshTunnel>) -> PgPoolOptions {
    PgPoolOptions::new().max_connections(5).acquire_timeout(timeout).after_connect(move |connection, _| {
        let tls = tls.clone(); let tunnel = tunnel.clone();
        Box::pin(async move {
            check_tunnel(tunnel.as_ref())?;
            tls.verify_pg_connection(&mut *connection).await.map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
            if read_only {
                crate::sql_history::query("SET default_transaction_read_only=on").execute(&mut *connection).await?;
                let flag: String = crate::sql_history::query_scalar("SHOW default_transaction_read_only").fetch_one(&mut *connection).await?;
                if flag != "on" { return Err(sqlx::Error::Protocol("PostgreSQL只读事务默认值未建立".into())); }
            }
            check_tunnel(tunnel.as_ref())?;
            Ok(())
        })
    })
}
async fn close_failed(tunnel: Option<&SshTunnel>) {
    if let Some(tunnel) = tunnel { tunnel.request_close(); tunnel.wait_closed().await; }
}
pub async fn open(req: &ConnectionRequest, timeout: Duration, read_only: bool, tls: TlsOptions, ssh: Option<SshConfig>) -> Result<OpenedConnection> {
    let (_, parameters) = crate::driver_manager::split_version_selection(&req.params)?;
    let mut local = req.clone(); local.params = parameters;
    let tunnel = if let Some(ssh) = ssh {
        tls.ensure_ssh_compatible()?;
        let target_port = if req.port == 0 { match req.kind.protocol() { Protocol::Mysql => 3306, Protocol::Postgres => 5432, _ => return Err(anyhow!("SSH_PROTOCOL_UNSUPPORTED")) } } else { req.port };
        let guard = crate::ssh_tunnel::start(&ssh, &req.host, target_port).await?;
        local.host = "127.0.0.1".into(); local.port = guard.local_port;
        Some(guard)
    } else { None };
    let opening = async {
        match req.kind.protocol() {
            Protocol::Mysql => {
                let advanced = crate::mysql_options::parse(&local.params)?;
                let port = if local.port == 0 { 3306 } else { local.port };
                let user = if local.user.is_empty() { "root" } else { local.user.as_str() };
                let mut options = MySqlConnectOptions::new().host(&local.host).port(port).username(user).password(&local.password);
                options = tls.apply_mysql(advanced.apply(options));
                if !local.database.is_empty() { options = options.database(&local.database); }
                let pool = mysql_options(timeout, read_only, advanced, tls.clone(), tunnel.clone()).connect_with(options).await?;
                Ok(Db::Mysql(pool))
            },
            Protocol::Postgres => {
                let port = if local.port == 0 { 5432 } else { local.port };
                let user = if local.user.is_empty() { "postgres" } else { local.user.as_str() };
                let database = if local.database.is_empty() { "postgres" } else { local.database.as_str() };
                let mut options = tls.apply_pg(PgConnectOptions::new().host(&local.host).port(port).username(user).password(&local.password).database(database).application_name("navkit"));
                let startup = crate::tls_options::server_startup_params(&local.params)?;
                for (key, value) in crate::tls_options::params::parse(&startup)? { options = options.options([(key, value)]); }
                let pool = pg_options(timeout, read_only, tls.clone(), tunnel.clone()).connect_with(options).await?;
                Ok(Db::Postgres(pool))
            },
            _ => Err(anyhow!("SQLX_RUNTIME_PROTOCOL_UNSUPPORTED")),
        }
    }.await;
    match opening {
        Ok(db) if tunnel.as_ref().map(|tunnel| !tunnel.is_available()).unwrap_or(false) => { super::shutdown_db(&db).await; close_failed(tunnel.as_ref()).await; Err(anyhow!("SSH_SESSION_CLOSED: 数据库握手期间SSH已断开；不会登记旧物理连接")) },
        Ok(db) => Ok(OpenedConnection { db, tunnel }),
        Err(error) => { close_failed(tunnel.as_ref()).await; Err(error) },
    }
}
pub async fn open_legacy(req: &ConnectionRequest, timeout: Duration, read_only: bool) -> Result<Db> {
    let parameters = runtime::transport_parameters(&req.params)?.ok_or_else(|| anyhow!("RUNTIME_DRIVER_CONFIG_CONFLICT"))?;
    if runtime::profile_reference(&parameters)?.is_some() { return Err(anyhow!("RUNTIME_PROFILE_SAVED_CONTEXT_REQUIRED: 本机配置引用必须经已保存连接或独立临时配置解析，不能被普通测试忽略")); }
    let tls = crate::tls_options::parse(&parameters, true)?;
    let ssh = crate::ssh_tunnel::parse(&parameters)?;
    let opened = open(req, timeout, read_only, tls, ssh).await?;
    Ok(opened.db)
}
pub async fn derive_mysql(pool: &sqlx::MySqlPool, namespace: &str, parameters: &str, tls: TlsOptions, tunnel: Option<SshTunnel>, read_only: bool, timeout: Duration) -> Result<Db> {
    check_tunnel(tunnel.as_ref())?;
    let advanced = crate::mysql_options::parse(parameters)?;
    let options = pool.connect_options().as_ref().clone().database(namespace);
    let derived = mysql_options(timeout, read_only, advanced, tls, tunnel).connect_with(options).await?;
    Ok(Db::Mysql(derived))
}
pub async fn derive_pg(pool: &sqlx::PgPool, search_path: &str, tls: TlsOptions, tunnel: Option<SshTunnel>, read_only: bool, timeout: Duration) -> Result<Db> {
    check_tunnel(tunnel.as_ref())?;
    let options = pool.connect_options().as_ref().clone().options([("search_path", search_path)]);
    let derived = pg_options(timeout, read_only, tls, tunnel).connect_with(options).await?;
    Ok(Db::Postgres(derived))
}
