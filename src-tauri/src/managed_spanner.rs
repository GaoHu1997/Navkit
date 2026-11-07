//! Original Google Spanner all-JAR. Cloud token and explicit local emulator are distinct routes.
use super::*;

pub(super) fn vendor() -> Vendor {
    Vendor {
        kind: "spanner", kinds: &["spanner"], version: "2.47.0", class: "com.google.cloud.spanner.jdbc.JdbcDriver", prefix: "cloudspanner",
        manifest_version: "2.47.0", class_major: 52,
        probe_url: "jdbc:cloudspanner://127.0.0.1:1/projects/probe-project/instances/probe-instance/databases/probe-db;usePlainText=true;autoConfigEmulator=false;enableEndToEndTracing=false;minSessions=0;maxSessions=1;numChannels=1",
        jar: spec("https://repo.maven.apache.org/maven2/com/google/cloud/google-cloud-spanner-jdbc/2.47.0/google-cloud-spanner-jdbc-2.47.0-single-jar-with-dependencies.jar", 69_051_102, "defe87db99e3d547c95f956662698e358d342a9a3ad5ca0361193099fcda77c8"),
        pom: Some(spec("https://repo.maven.apache.org/maven2/com/google/cloud/google-cloud-spanner-jdbc/2.47.0/google-cloud-spanner-jdbc-2.47.0.pom", 78_594, "1eca532527fb05ff07c75f66337fb0d1bcafd275ee54fe02b4619111b3f74596")),
        license: None, entries: 31_787, expanded: 171_343_958, max_expanded: 256 * 1024 * 1024,
        limits: "官方 Google Spanner JDBC 2.47.0 原始 single-jar-with-dependencies 与 POM，包含必需运行依赖及原许可；需 Java 11+。数据库填 project/instance/database；云端固定 spanner.googleapis.com:443、正常 TLS，用户名 token、密码为预生成 OAuth 令牌，不搜索 ADC 或隐式加载凭据文件。显式字面回环地址与 ssl=false 仅用于无凭据本地模拟器，不把令牌发送到明文端点。原驱动 gRPC 认证拒绝验证不是 Google 云端 SQL/TLS 验收；令牌刷新、服务账户、PGAdapter、Omni 和自定义端点请显式配置自定义 JDBC。",
    }
}
fn resource(value: &str) -> Result<String> {
    let parts: Vec<_> = value.trim().split('/').collect();
    let ids = match parts.as_slice() {
        [project, instance, database] => [*project, *instance, *database],
        ["projects", project, "instances", instance, "databases", database] => [*project, *instance, *database],
        _ => bail!("Spanner 数据库必须为 project/instance/database 或完整 projects/.../instances/.../databases/..."),
    };
    for id in ids {
        if id.is_empty() || id.len() > 128 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)) {
            bail!("Spanner 项目/实例/数据库标识符无效")
        }
    }
    Ok(format!("projects/{}/instances/{}/databases/{}", ids[0], ids[1], ids[2]))
}
pub(super) fn connection_url(req: &ConnectionRequest, pairs: &[(String, String)]) -> Result<String> {
    if pairs.iter().any(|(key, _)| !["ssl", "tls", "sslmode", "ssl-mode", "ssl_verify"].contains(&key.as_str())) {
        bail!("Spanner 扩展属性须显式配置自定义 JDBC，不会忽略 catalog/schema/认证设置")
    }
    let host = validated_host(req)?;
    let literal = host.trim_start_matches('[').trim_end_matches(']');
    let loopback = literal.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback());
    let plain = matches!(tls_mode(pairs)?, Some(TlsMode::Plain));
    if loopback {
        if !plain || req.port == 0 { bail!("Spanner 本地模拟器须显式字面回环地址、端口及 ssl=false") }
        if !req.user.is_empty() || !req.password.is_empty() { bail!("Spanner 明文本地模拟器不接收用户名或 OAuth 令牌；不会泄露凭据") }
    } else {
        if host != "spanner.googleapis.com" || !matches!(req.port, 0 | 443) || plain { bail!("Spanner 云端托管连接只允许 spanner.googleapis.com:443 与正常 TLS") }
        if req.user != "token" || req.password.is_empty() || !req.password.bytes().all(|b| b.is_ascii_alphanumeric() || b"._~+/=-".contains(&b)) {
            bail!("Spanner 云端须用户名 token 和预生成 OAuth 访问令牌；不隐式搜索 ADC")
        }
    }
    let authority = server_authority(req, 443)?;
    Ok(format!("jdbc:cloudspanner://{authority}/{};usePlainText={plain};autoConfigEmulator=false;enableEndToEndTracing=false;minSessions=0;maxSessions=10;numChannels=1", resource(&req.database)?))
}
