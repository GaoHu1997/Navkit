//! 测试公共工具：显式关闭连接池（Windows 下必须先 close 才能 remove_dir_all）、
//! 真实 MySQL 的可用性探测。

use navkit_lib::db::{Db, DbRegistry};
use std::sync::Arc;

/// 排空注册表并显式关闭全部连接池。
///
/// sqlx 池 drop 后仍有后台任务持有 SQLite 文件句柄，
/// Windows 上不 close 就 remove_dir_all 会静默失败（遗留临时目录）。
pub async fn shutdown_registry(reg: &DbRegistry) {
    let dbs: Vec<Arc<Db>> = reg.write().drain().map(|(_, v)| v).collect();
    for db in dbs {
        close_db(&db).await;
    }
}

/// 关闭单个连接（对 Arc 克隆也有效：close 作用于共享池实例）。
pub async fn close_db(db: &Db) {
    match db {
        Db::Sqlite(p) => p.close().await,
        Db::Mysql(p) => p.close().await,
        Db::Postgres(p) => p.close().await,
        // ClickHouse / ODBC 无显式连接池关闭语义
        Db::ClickHouse(_) | Db::Odbc(_) | Db::Http(_) | Db::Mssql(_) | Db::Redis(_) | Db::Mongo(_) => {}
    }
}

/// 本机 127.0.0.1:port 是否可建立 TCP 连接（用于跳过依赖真实服务器的测试）。
pub fn tcp_probe(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(800),
    )
    .is_ok()
}

/// 环境变量提供的真实 MySQL（host:port:user:pass:db），
/// 形如 `127.0.0.1:3306:root:secret:testdb`。未设置或服务器不可达时跳过。
pub fn mysql_env_creds() -> Option<(String, u16, String, String, String)> {
    let v = std::env::var("NAVKIT_TEST_MYSQL").ok()?;
    let parts: Vec<&str> = v.split(':').collect();
    if parts.len() != 5 {
        return None;
    }
    Some((
        parts[0].to_string(),
        parts[1].parse().ok()?,
        parts[2].to_string(),
        parts[3].to_string(),
        parts[4].to_string(),
    ))
}
