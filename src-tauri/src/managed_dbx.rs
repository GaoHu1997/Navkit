//! Fixed original drivers hosted by the user-requested dbx reference; explicitly third-party provenance.
//! Original bytes/notices are retained. dbx's license is not applied to proprietary driver contents.
use super::*;
const SOURCE: &str = "来源为用户指定参考项目 dbx 的固定提交 71ce03490dad3c9c3eaaa3bd62863b9a6fcdbb9a 原 JAR，并非厂商官方下载；原包许可/签名保留，不推断再分发授权。需 Java 11+；普通 TCP 用户名/密码路线已验证，厂商服务器 SQL/认证和 TLS 仍 UNVERIFIED。TLS/证书/集群扩展请显式配置自定义 JDBC，不会静默降级。";
pub(super) fn vendor(kind: &str) -> Option<Vendor> {
    match kind {
        "cache" => Some(Vendor {
            kind: "cache", kinds: &["cache"], version: "2010.2.3.702", class: "com.intersys.jdbc.CacheDriver", prefix: "Cache",
            manifest_version: "2010.2.3.702", class_major: 50, probe_url: "jdbc:Cache://127.0.0.1:1/USER",
            jar: spec("https://raw.githubusercontent.com/t8y2/dbx/71ce03490dad3c9c3eaaa3bd62863b9a6fcdbb9a/agents/drivers/cache/libs/CacheDB.jar", 2_233_648, "2b12827c4e03ce043f10c9651f12206e97bc6fb73d7556451ae97b9d3eb7a29c"),
            pom: None, license: None, entries: 987, expanded: 5_203_333, max_expanded: 16 * 1024 * 1024, limits: SOURCE,
        }),
        "sundb" => Some(Vendor {
            kind: "sundb", kinds: &["sundb"], version: "8-1.0.2", class: "csii.sundb.jdbc.SundbDriver", prefix: "sundb",
            manifest_version: "", class_major: 52, probe_url: "jdbc:sundb://127.0.0.1:1/test",
            jar: spec("https://raw.githubusercontent.com/t8y2/dbx/71ce03490dad3c9c3eaaa3bd62863b9a6fcdbb9a/agents/drivers/sundb/libs/sundb8-1.0.2.jar", 567_314, "d654aa28331888e5b2079f7736a67d3c963ec80140b4775df4af4799da04c733"),
            pom: None, license: None, entries: 405, expanded: 1_102_757, max_expanded: 8 * 1024 * 1024, limits: SOURCE,
        }),
        "oscar" => Some(Vendor {
            kind: "oscar", kinds: &["oscar"], version: "4.1.167x20260107145220", class: "com.oscar.Driver", prefix: "oscar",
            manifest_version: "", class_major: 52, probe_url: "jdbc:oscar://127.0.0.1:1/OSRDB",
            jar: spec("https://raw.githubusercontent.com/t8y2/dbx/71ce03490dad3c9c3eaaa3bd62863b9a6fcdbb9a/agents/drivers/oscar/libs/oscarJDBC8.jar", 3_639_760, "91a05d96da16d57b906cd488cc8c90066fa3878ef17bbf48b88a9d083bafb7be"),
            pom: None, license: None, entries: 897, expanded: 9_302_879, max_expanded: 16 * 1024 * 1024, limits: SOURCE,
        }),
        _ => None,
    }
}
pub(super) fn connection_url(req: &ConnectionRequest, kind: &str, pairs: &[(String, String)]) -> Result<String> {
    if matches!(tls_mode(pairs)?, Some(TlsMode::Require | TlsMode::VerifyCa | TlsMode::VerifyFull)) {
        bail!("dbx 原件托管连接未核验 TLS，不会静默降级；请配置自定义 JDBC")
    }
    if pairs.iter().any(|(key, _)| !["ssl", "tls", "sslmode", "ssl-mode", "ssl_verify"].contains(&key.as_str())) {
        bail!("dbx 原件的扩展认证或命名空间请显式配置自定义 JDBC，不会忽略 catalog/schema")
    }
    let (prefix, port, default) = match kind {
        "cache" => ("Cache", 1972, "USER"), "sundb" => ("sundb", 22581, "test"), "oscar" => ("oscar", 2003, "OSRDB"),
        _ => bail!("未知 dbx 原件产品"),
    };
    let database = if req.database.trim().is_empty() { default } else { plain_name(req.database.trim(), "厂商数据库/namespace")? };
    Ok(format!("jdbc:{prefix}://{}/{database}", server_authority(req, port)?))
}
