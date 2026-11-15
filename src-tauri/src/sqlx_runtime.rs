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
// Report local dial/auth failures without changing address, credentials, or TLS.
fn mysql_connect_error(host: &str, tunneled: bool, error: sqlx::Error) -> anyhow::Error {
    if tunneled || !(host.eq_ignore_ascii_case("localhost") || matches!(host, "::1" | "127.0.0.1")) {
        return error.into();
    }
    let authentication = match &error {
        sqlx::Error::Database(database) => database.code().is_some_and(|code| code.starts_with("28"))
            || database.try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>()
                .is_some_and(|error| matches!(error.number(), 1044 | 1045 | 1130 | 1698)),
        _ => false,
    };
    let transport = matches!(&error, sqlx::Error::Io(error) if matches!(error.kind(),
        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotConnected
        | std::io::ErrorKind::AddrNotAvailable | std::io::ErrorKind::TimedOut));
    let (code, guidance) = if authentication {
        ("MYSQL_AUTH_REJECTED", "服务器已响应但拒绝认证或授权。请检查密码及 IPv4/IPv6 主机授权；Access denied 不会触发自动换地址重试。")
    } else if transport {
        ("MYSQL_CONNECT_FAILED", "请检查本机服务、端口及 IPv4/IPv6 监听地址。可在连接属性中明确选择 127.0.0.1 或 ::1 后重新测试；不会自动切换地址。")
    } else if matches!(&error, sqlx::Error::PoolTimedOut) {
        ("POOL_ACQUIRE_TIMEOUT", "建立或获取本机连接超时；连接池未返回最初的网络错误，不能据此判断为密码错误或 IPv6 故障。请检查服务、端口及 IPv4/IPv6 监听；如需指定地址，请在连接属性中明确选择后重新测试。")
    } else {
        return error.into();
    };
    crate::error::AppError::coded(code, format!("{code}: {guidance} TLS 配置保持不变；启用主机名校验时，所选地址必须匹配服务器证书。\n{error}")).into()
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
                let pool = mysql_options(timeout, read_only, advanced, tls.clone(), tunnel.clone()).connect_with(options).await
                    .map_err(|error| mysql_connect_error(&req.host, tunnel.is_some(), error))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn request(port: u16) -> ConnectionRequest {
        ConnectionRequest { kind: crate::models::DbKind::of("mysql"), host: "127.0.0.1".into(), port,
            user: "test".into(), password: String::new(), database: String::new(), file: String::new(),
            params: "sslmode=disable".into(), timeout_secs: 1 }
    }

    #[test]
    fn loopback_connection_guidance_is_scoped_and_preserves_unknown_tls_errors() {
        for host in ["localhost", "LOCALHOST", "127.0.0.1", "::1"] {
            let error = mysql_connect_error(host, false, std::io::Error::from(std::io::ErrorKind::ConnectionRefused).into());
            let app = crate::error::AppError::from(error);
            assert_eq!(app.code, "MYSQL_CONNECT_FAILED");
            assert!(app.message.contains("127.0.0.1") && app.message.contains("::1"));
            assert!(!app.retryable);
        }
        for (host, tunneled) in [("db.example", false), ("localhost.example", false), ("localhost", true)] {
            let error = mysql_connect_error(host, tunneled, std::io::Error::from(std::io::ErrorKind::ConnectionRefused).into());
            assert!(matches!(error.downcast_ref::<sqlx::Error>(), Some(sqlx::Error::Io(_))));
        }
        for error in [sqlx::Error::Tls(std::io::Error::other("certificate mismatch").into()),
            sqlx::Error::Protocol("TLS_EVIDENCE_UNAVAILABLE".into()),
            std::io::Error::from(std::io::ErrorKind::ConnectionReset).into()] {
            let original = error.to_string();
            let error = mysql_connect_error("localhost", false, error);
            assert!(error.downcast_ref::<sqlx::Error>().is_some());
            assert_eq!(error.to_string(), original);
        }
    }

    #[tokio::test]
    async fn refused_loopback_connection_is_not_authentication_failure() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let req = request(listener.local_addr().unwrap().port());
        drop(listener);
        let result = open_legacy(&req, Duration::from_millis(500), false).await;
        let error = result.err().expect("closed port must fail");
        let app = crate::error::AppError::from(error);
        assert!(matches!(app.code.as_str(), "MYSQL_CONNECT_FAILED" | "POOL_ACQUIRE_TIMEOUT"), "{}", app.message);
        assert!(!app.message.contains("MYSQL_AUTH_REJECTED"));
    }

    #[tokio::test]
    async fn access_denied_is_authentication_failure_and_never_retried() {
        use tokio::io::AsyncWriteExt;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let req = request(listener.local_addr().unwrap().port());
        let attempts = Arc::new(AtomicUsize::new(0));
        let count = attempts.clone();
        let (stop, mut stopped) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = tokio::select! {
                    _ = &mut stopped => break,
                    accepted = listener.accept() => accepted.unwrap(),
                };
                count.fetch_add(1, Ordering::SeqCst);
                // A real SQLx-decoded ERR packet, not a string-matched mock error.
                let mut payload = vec![0xff, 0x15, 0x04];
                payload.extend_from_slice(b"#28000Access denied for user 'test'@'::1' (using password: NO)");
                let len = payload.len();
                socket.write_all(&[len as u8, (len >> 8) as u8, (len >> 16) as u8, 0]).await.unwrap();
                socket.write_all(&payload).await.unwrap();
            }
        });
        let result = tokio::time::timeout(Duration::from_secs(2), open_legacy(&req, Duration::from_secs(1), false)).await;
        let _ = stop.send(());
        server.await.unwrap();
        let error = result.expect("auth rejection must be immediate").err().expect("auth must fail");
        let app = crate::error::AppError::from(error);
        assert_eq!(app.code, "MYSQL_AUTH_REJECTED", "{}", app.message);
        assert!(app.message.contains("1045") && app.message.contains("Access denied") && app.message.contains("::1"));
        assert!(!app.message.contains("MYSQL_CONNECT_FAILED"));
        assert!(!app.retryable);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}
