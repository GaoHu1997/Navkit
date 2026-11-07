//! Fixed, product-specific original JDBC packages with explicit provenance. No JAR is bundled in NavKit.
//!
//! 参考 dbx（https://github.com/t8y2/dbx）的分层支持策略：协议兼容数据库走原生驱动、
//! 其余数据库走具体产品 JDBC。每个产品只允许固定的 Maven/厂商或明确标源的 dbx 原始制品
//! （现经由自有镜像仓库 GaoHu1997/Navkit-drivers 字节级托管；jar/zip/pom 的
//! 大小/SHA-256/条目数/展开大小全部固定），所有 URL 拼写均经真实驱动 `acceptsURL`/
//! `getPropertyInfo`/死端口连接验证；不会被验证支持的 TLS 形态一律显式拒绝而不是静默明文。
//!
//! This module lives below driver_manager::downloads so its network, semaphore and
//! staging helpers are the existing bounded/signed driver-download chain.
use super::{digest_file, safe_relative, DownloadSpec, DriverStatus, Staging, DOWNLOAD_GATE};
#[path = "jdbc_dependencies.rs"]
mod dependencies;
#[path = "managed_vendor_assets.rs"]
mod vendor_assets;
#[path = "managed_spanner.rs"]
mod spanner;
#[path = "managed_dbx.rs"]
mod dbx;
use crate::{
    driver_manager::{DriverResolution, OdbcInventory, PARAMS_PREFIX},
    models::ConnectionRequest,
};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

pub(crate) struct Vendor {
    /// 产品/包目录 id：manifest.kind 与 package_dir 使用；兼容产品共享同一包。
    kind: &'static str,
    /// 映射到本产品的数据源类型（kind 本身 + 协议兼容的目录项）。
    kinds: &'static [&'static str],
    version: &'static str,
    class: &'static str,
    prefix: &'static str,
    /// JAR Manifest 的 Implementation-Version 前缀。空串表示该产品 manifest 无版本行
    /// （如 IBM jcc / Exasol），此时整个 JAR 的摘要固定就是产品身份契约。
    manifest_version: &'static str,
    class_major: u16,
    probe_url: &'static str,
    jar: DownloadSpec,
    // Dremio's original all-in-one release has no published POM; never fabricate one.
    pom: Option<DownloadSpec>,
    license: Option<DownloadSpec>,
    entries: usize,
    expanded: u64,
    max_expanded: u64,
    limits: &'static str,
}
const MAVEN: &str = "https://repo.maven.apache.org/maven2/";
const fn spec(path: &'static str, size: u64, sha256: &'static str) -> DownloadSpec {
    DownloadSpec { url: path, size, sha256 }
}
impl Vendor {
    /// 固定原包版本号（如 "435"、"2.3.232"）；字段对外只读。
    pub(crate) fn version(&self) -> &'static str { self.version }
}
fn vendor(kind: &str) -> Option<Vendor> {
    match kind {
        "trino" => Some(Vendor {
            kind: "trino", kinds: &["trino"], version: "435", class: "io.trino.jdbc.TrinoDriver", prefix: "trino",
            manifest_version: "435", class_major: 52, probe_url: "jdbc:trino://127.0.0.1:8080",
            jar: spec("https://repo.maven.apache.org/maven2/io/trino/trino-jdbc/435/trino-jdbc-435.jar", 10281807, "75fd7373cd5d7d7d1fe272d936d436e4dca024ab3d901382277fcdfc46cad476"),
            pom: Some(spec("https://repo.maven.apache.org/maven2/io/trino/trino-jdbc/435/trino-jdbc-435.pom", 38754, "98742be070f75a239fdf270c1aa07c48e922815bf60b3a48f9bd77d5af77f5e8")),
            license: Some(spec("https://raw.githubusercontent.com/trinodb/trino/435/LICENSE", 11358, "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30")),
            entries: 6476, expanded: 24052415, max_expanded: 64 * 1024 * 1024,
            limits: "仅官方 Trino JDBC 435（Java 8 字节码，NavKit source launcher 需 Java 11+）；不是 PrestoDB/PrestoSQL、Starburst 商业驱动或 Hive/Spark 通用驱动。服务器版本/认证扩展仍须实际验收。",
        }),
        "prestosql" => Some(Vendor {
            kind: "prestosql", kinds: &["prestosql"], version: "350", class: "io.prestosql.jdbc.PrestoDriver", prefix: "presto",
            manifest_version: "350", class_major: 52, probe_url: "jdbc:presto://127.0.0.1:8080",
            jar: spec("https://repo.maven.apache.org/maven2/io/prestosql/presto-jdbc/350/presto-jdbc-350.jar", 6656599, "664a23e517c120c1d5a9bb88b215612b117415b9f1649bc22c48298eaf3f979b"),
            pom: Some(spec("https://repo.maven.apache.org/maven2/io/prestosql/presto-jdbc/350/presto-jdbc-350.pom", 21007, "20da3aac314d2bd773c8ed7a2ad9dcd26d5e471329cd3b7074ac76403ca0d9f0")),
            license: None, entries: 4383, expanded: 15518270, max_expanded: 64 * 1024 * 1024,
            limits: "仅更名前的 PrestoSQL 350 官方 io.prestosql JDBC；不是 com.facebook.presto 的 PrestoDB，也不宣称支持任意 Trino/Starburst/Hive/Spark。NavKit 需 Java 11+，服务器版本/认证扩展仍须实际验收。",
        }),
        "databricks" => Some(Vendor {
            kind: "databricks", kinds: &["databricks"], version: "3.4.4", class: "com.databricks.client.jdbc.Driver", prefix: "databricks",
            manifest_version: "3.4.4", class_major: 55,
            probe_url: "jdbc:databricks://127.0.0.1:443/default;httpPath=/sql/1.0/warehouses/navkit-probe;AuthMech=3;SSL=1",
            jar: spec("https://repo.maven.apache.org/maven2/com/databricks/databricks-jdbc/3.4.4/databricks-jdbc-3.4.4.jar", 42987921, "1f2e2c5474f2bfbf914f9647319d0d69d8eeb2b1ebb891b05044d79539a359e7"),
            pom: Some(spec("https://repo.maven.apache.org/maven2/com/databricks/databricks-jdbc/3.4.4/databricks-jdbc-3.4.4.pom", 1428, "0beeefc2ff7865b01394e704435a0677b5d92b237984b6eadd439334fe99fffa")),
            license: None, entries: 27592, expanded: 102656765, max_expanded: 128 * 1024 * 1024,
            limits: "仅官方 Databricks JDBC 3.4.4（Apache 2.0、自包含 JAR、Java 11+）；需参数 httpPath，用户名 token、密码为 PAT，仅支持 AuthMech=3 和严格 TLS。OAuth/自定义信任库请显式配置自定义 JDBC；不是 Simba ODBC、Hive/Spark 通用驱动。数据库名可填 catalog/schema；服务器与认证仍须实际验收。",
        }),
        "db2" => Some(Vendor {
            kind: "db2", kinds: &["db2"], version: "12.1.5.0", class: "com.ibm.db2.jcc.DB2Driver", prefix: "db2",
            manifest_version: "", class_major: 52, probe_url: "jdbc:db2://127.0.0.1:50000/navkit",
            jar: spec("https://repo.maven.apache.org/maven2/com/ibm/db2/jcc/12.1.5.0/jcc-12.1.5.0.jar", 6630167, "04150111a29370247d162c3f05b8e938bf1314f19a63ad9a5302d88dd478e8e8"),
            pom: Some(spec("https://repo.maven.apache.org/maven2/com/ibm/db2/jcc/12.1.5.0/jcc-12.1.5.0.pom", 1423, "9995113be8dee9413cd0a3e1b8631656df49503a49ac9814fc996f6143ee0feb")),
            license: None, entries: 2367, expanded: 14882066, max_expanded: 64 * 1024 * 1024,
            limits: "仅官方 IBM Data Server Driver for JDBC（com.ibm.db2:jcc 12.1.5.0，适用 DB2 LUW/z/OS/i）；TLS 用 :sslConnection=true; URL 属性。服务器版本/认证/LDAP 扩展仍须实际验收。",
        }),
        "informix" => Some(Vendor {
            kind: "informix", kinds: &["informix"], version: "15.0.1.4", class: "com.informix.jdbc.IfxDriver", prefix: "informix-sqli",
            manifest_version: "15.0.1.4", class_major: 52, probe_url: "jdbc:informix-sqli://127.0.0.1:9088/navkit:INFORMIXSERVER=navkit_probe",
            jar: spec("https://repo.maven.apache.org/maven2/com/ibm/informix/jdbc/15.0.1.4/jdbc-15.0.1.4.jar", 1195494, "152fe3380e414261266d7bde6bacae348c94b6db0cf16f969d1094368449cec7"),
            pom: Some(spec("https://repo.maven.apache.org/maven2/com/ibm/informix/jdbc/15.0.1.4/jdbc-15.0.1.4.pom", 1469, "db7f2965194c57f276872180c2f80bec09785adb67658a426b82e3d416aa8dd5")),
            license: None, entries: 457, expanded: 2731790, max_expanded: 64 * 1024 * 1024,
            limits: "仅官方 IBM Informix JDBC 15.0.1.4；需要参数 informixserver（sqlhosts 服务器名）。TLS 由服务器端 sqlhosts/环境决定，驱动无经核验的 URL TLS 参数，需要 TLS 请用自定义 JDBC。",
        }),
        "saphana" => Some(Vendor {
            kind: "saphana", kinds: &["saphana"], version: "2.30.11", class: "com.sap.db.jdbc.Driver", prefix: "sap",
            manifest_version: "2.30.11-", class_major: 52, probe_url: "jdbc:sap://127.0.0.1:30015/?databaseName=HXE&encrypt=true&validateCertificate=true",
            jar: spec("https://repo.maven.apache.org/maven2/com/sap/cloud/db/jdbc/ngdbc/2.30.11/ngdbc-2.30.11.jar", 1588075, "38654a00df1ea9616f9f4cea745f1f8a5a923a1c98acbd10ce4962ba4a2c1fb8"),
            pom: Some(spec("https://repo.maven.apache.org/maven2/com/sap/cloud/db/jdbc/ngdbc/2.30.11/ngdbc-2.30.11.pom", 1298, "cec71f04c46f5772e6925624a53559299a1957de436d4de8e766ec0db3ec2616")),
            license: None, entries: 704, expanded: 3727638, max_expanded: 64 * 1024 * 1024,
            limits: "仅官方 SAP HANA JDBC（com.sap.cloud.db.jdbc:ngdbc 2.30.11）；TLS 用 encrypt=true&validateCertificate=true URL 参数。SSO/Kerberos 等扩展与服务器版本仍须实际验收。",
        }),
        "teradata" => Some(Vendor {
            kind: "teradata", kinds: &["teradata"], version: "20.00.00.62", class: "com.teradata.jdbc.TeraDriver", prefix: "teradata",
            manifest_version: "20.00.00.62", class_major: 52, probe_url: "jdbc:teradata://127.0.0.1/DBS_PORT=1025,DATABASE=navkit",
            jar: spec("https://repo.maven.apache.org/maven2/com/teradata/jdbc/terajdbc/20.00.00.62/terajdbc-20.00.00.62.jar", 1305372, "e3264b00355f8e4030f21459e9ef635582b2dd91ddf62e0c5cb0f7fbe6873544"),
            pom: Some(spec("https://repo.maven.apache.org/maven2/com/teradata/jdbc/terajdbc/20.00.00.62/terajdbc-20.00.00.62.pom", 1315, "9f860c17c1b488fb67f3eeff4f2d126065414c5d3e51ce41d5f2321c53d504c3")),
            license: None, entries: 499, expanded: 3057957, max_expanded: 64 * 1024 * 1024,
            limits: "仅官方 Teradata JDBC 20.00.00.62；非 TLS 走 DBS_PORT，TLS（SSLMode=REQUIRE/VERIFY-CA/VERIFY-FULL）实测强制 https_port，NavKit 显式写入端口。服务器/认证扩展仍须实际验收。",
        }),
        "vertica" => Some(Vendor {
            kind: "vertica", kinds: &["vertica"], version: "25.3.0-0", class: "com.vertica.jdbc.Driver", prefix: "vertica",
            manifest_version: "25.03.0000", class_major: 52, probe_url: "jdbc:vertica://127.0.0.1:5433/navkit",
            jar: spec("https://repo.maven.apache.org/maven2/com/vertica/jdbc/vertica-jdbc/25.3.0-0/vertica-jdbc-25.3.0-0.jar", 1850793, "f43c4ed61a9639d95214e00c888fd21c4ea283d60ca2ce06c6855a774ff5e583"),
            pom: Some(spec("https://repo.maven.apache.org/maven2/com/vertica/jdbc/vertica-jdbc/25.3.0-0/vertica-jdbc-25.3.0-0.pom", 1254, "77e480e1e358490e0781cd9808279f976d8d0d2e8bb50bda6cad08f77210a513")),
            license: None, entries: 1519, expanded: 3859476, max_expanded: 64 * 1024 * 1024,
            limits: "仅官方 Vertica JDBC 25.3.0-0；TLS 用 tlsmode=require/verify-ca/verify-full URL 参数。服务器版本/认证扩展仍须实际验收。",
        }),
        "exasol" => Some(Vendor {
            kind: "exasol", kinds: &["exasol"], version: "26.2.10", class: "com.exasol.jdbc.EXADriver", prefix: "exa",
            manifest_version: "", class_major: 52, probe_url: "jdbc:exa:127.0.0.1:8563",
            jar: spec("https://repo.maven.apache.org/maven2/com/exasol/exasol-jdbc/26.2.10/exasol-jdbc-26.2.10.jar", 9525280, "91b11bd846b0c447a1e9fed86541bc2c1b4c9272f9a2d0f169e9ff0bca222d19"),
            pom: Some(spec("https://repo.maven.apache.org/maven2/com/exasol/exasol-jdbc/26.2.10/exasol-jdbc-26.2.10.pom", 1455, "c4a571600f0a10fe2f41dcc3608413e7cf4c86a779e47cd405bf89bd4bedad20")),
            license: None, entries: 8168, expanded: 19444103, max_expanded: 64 * 1024 * 1024,
            limits: "仅官方 Exasol JDBC 26.2.10；URL 仅验证过 encryption=true/false 开关（validateservercertificate 的取值解析与文档不符，不写入）；证书校验由驱动默认策略决定，需要指定指纹请用自定义 JDBC。",
        }),
        "dameng" => Some(Vendor {
            kind: "dameng", kinds: &["dameng"], version: "8.1.3.140", class: "dm.jdbc.driver.DmDriver", prefix: "dm",
            manifest_version: "- 8.1.3.140 - Production", class_major: 52, probe_url: "jdbc:dm://127.0.0.1:5236",
            jar: spec("https://repo.maven.apache.org/maven2/com/dameng/DmJdbcDriver18/8.1.3.140/DmJdbcDriver18-8.1.3.140.jar", 1376925, "9af4ff4d6ed15948507f528a18ab9b7196b3600d9169ad7998c19869031a3c6f"),
            pom: Some(spec("https://repo.maven.apache.org/maven2/com/dameng/DmJdbcDriver18/8.1.3.140/DmJdbcDriver18-8.1.3.140.pom", 974, "f0aa320655b016e92ef2c147f47f8555cbae20f044affb801fac9224914f72d9")),
            license: None, entries: 429, expanded: 3897730, max_expanded: 64 * 1024 * 1024,
            limits: "仅 Maven Central 官方 com.dameng:DmJdbcDriver18 8.1.3.140（达梦 JDBC，适用 DM8）；TLS 需要 sslFilesPath 客户端证书参数，请用自定义 JDBC。服务器版本/集群扩展仍须实际验收。",
        }),
        "firebird" => Some(Vendor {
            kind: "firebird", kinds: &["firebird"], version: "5.0.13.java11", class: "org.firebirdsql.jdbc.FBDriver", prefix: "firebirdsql",
            manifest_version: "5.0.13.java11", class_major: 55, probe_url: "jdbc:firebirdsql://127.0.0.1:3050/navkit",
            jar: spec("https://repo.maven.apache.org/maven2/org/firebirdsql/jdbc/jaybird/5.0.13.java11/jaybird-5.0.13.java11.jar", 1583226, "de0d6c0a2dcd05a5622f13d1074b9a2b6de7df827bf245e799bba6b3f8774d4c"),
            pom: Some(spec("https://repo.maven.apache.org/maven2/org/firebirdsql/jdbc/jaybird/5.0.13.java11/jaybird-5.0.13.java11.pom", 5888, "cc681363ea8f8366baf22450033d38386ec0d415ccb77e80ac8a40e3193f910e")),
            license: None, entries: 1072, expanded: 3396863, max_expanded: 64 * 1024 * 1024,
            limits: "仅官方 Jaybird 5.0.13 Java 11 变体（org.firebirdsql.jdbc:jaybird）；wireCrypt=REQUIRED/DISABLED，无证书校验模式。数据库名填服务器端数据库路径或别名；Firebird 3+ 需要服务端开启 wire compression/wireCrypt 兼容。服务器与认证仍须实际验收。",
        }),
        "oracle" => Some(Vendor {
            kind: "oracle", kinds: &["oracle"], version: "23.26.4.0.0", class: "oracle.jdbc.OracleDriver", prefix: "oracle",
            manifest_version: "23.26.4.0.0", class_major: 55, probe_url: "jdbc:oracle:thin:@//127.0.0.1:1521/NAVKIT",
            jar: spec("https://repo.maven.apache.org/maven2/com/oracle/database/jdbc/ojdbc11/23.26.4.0.0/ojdbc11-23.26.4.0.0.jar", 8828929, "df977e9291499cd4ceee7d577462c285987dd2ad0216637c0865eb98956a9e6e"),
            pom: Some(spec("https://repo.maven.apache.org/maven2/com/oracle/database/jdbc/ojdbc11/23.26.4.0.0/ojdbc11-23.26.4.0.0.pom", 1477, "8ba46a7f433686c309271ac059f98023e14a6d04d89a31aaedb31523592b996a")),
            license: None, entries: 4020, expanded: 21203174, max_expanded: 64 * 1024 * 1024,
            limits: "仅官方 Oracle JDBC thin（com.oracle.database.jdbc:ojdbc11 23.26.4.0.0，Java 11+，与托管 ODBC 客户端同版本线）；默认按 host:port/service 连接，SID 须 params.sid 明确指定；TCPS 需要 wallet，请用自定义 JDBC 或托管 ODBC。服务器与认证仍须实际验收。",
        }),
        "hive" | "kyuubi" | "impala" | "spark" => Some(Vendor {
            kind: "hive", kinds: &["hive", "kyuubi", "impala", "spark"], version: "3.1.3", class: "org.apache.hive.jdbc.HiveDriver", prefix: "hive2",
            manifest_version: "3.1.3", class_major: 52, probe_url: "jdbc:hive2://127.0.0.1:10000/navkit",
            jar: spec("https://repo.maven.apache.org/maven2/org/apache/hive/hive-jdbc/3.1.3/hive-jdbc-3.1.3-standalone.jar", 72843348, "2f99f0912136d2d3ee72b26c5fa628c1edf3aac24908da21d3f5a92926074ec0"),
            pom: Some(spec("https://repo.maven.apache.org/maven2/org/apache/hive/hive-jdbc/3.1.3/hive-jdbc-3.1.3.pom", 17203, "f5f972d376958115c0a3fc893ee3f3b81630bb95cc9251c475db27cd2eb79bd8")),
            license: None, entries: 45244, expanded: 167250658, max_expanded: 192 * 1024 * 1024,
            limits: "仅官方 Apache Hive 3.1.3 standalone JDBC（Java 8 字节码）；Kyuubi、Impala、Spark 仅提供 HiveServer2 兼容端点连接方式，需服务端开放相应协议并使用本驱动支持的认证。驱动 ready 不代表这些厂商服务器已验收；不作为星环专有 SDK 的替代。仅支持 ssl=true，自定义信任库、Kerberos/HTTP transport 请用自定义 JDBC。", 
        }),
        "kylin" => Some(Vendor {
            kind: "kylin", kinds: &["kylin"], version: "5.0.5", class: "org.apache.kylin.jdbc.Driver", prefix: "kylin",
            manifest_version: "5.0.5", class_major: 52, probe_url: "jdbc:kylin://127.0.0.1:7070/navkit",
            jar: spec("https://repo.maven.apache.org/maven2/org/apache/kylin/kylin-jdbc/5.0.5/kylin-jdbc-5.0.5.jar", 10530561, "e4ec598dac5a05b46c0f9f7032068bde6a55fec46a6ed3a35a21880ef14b3c87"),
            pom: Some(spec("https://repo.maven.apache.org/maven2/org/apache/kylin/kylin-jdbc/5.0.5/kylin-jdbc-5.0.5.pom", 10449, "24c6e00faa4638c567c01b7ec0acf3a3ef0b13c8aee4323056b930050c0d779e")),
            license: None, entries: 5498, expanded: 30061853, max_expanded: 64 * 1024 * 1024,
            limits: "仅官方 Apache Kylin JDBC 5.0.5；数据库名填 project。未核验 HTTPS/TLS URL 参数，TLS 部署请用自定义 JDBC。服务器与认证仍须实际验收。",
        }),
        "snowflake" => Some(Vendor {
            kind: "snowflake", kinds: &["snowflake"], version: "4.4.1", class: "net.snowflake.client.api.driver.SnowflakeDriver", prefix: "snowflake",
            manifest_version: "4.4.1", class_major: 52, probe_url: "jdbc:snowflake://org-account.snowflakecomputing.com/?db=NAVKIT",
            jar: spec("https://repo.maven.apache.org/maven2/net/snowflake/snowflake-jdbc/4.4.1/snowflake-jdbc-4.4.1.jar", 98932562, "cb225d78ac10336669797dfef8588cffb7e3dae689f14120935831b1f6ba2709"),
            pom: Some(spec("https://repo.maven.apache.org/maven2/net/snowflake/snowflake-jdbc/4.4.1/snowflake-jdbc-4.4.1.pom", 1672, "dc9eaaa58833b8fcb45b54d85629c0a20336575efed55d40256719f97895c4e4")),
            license: None, entries: 47957, expanded: 279876110, max_expanded: 320 * 1024 * 1024,
            limits: "仅官方 Snowflake JDBC 4.4.1（自包含包约 98MB，强制 TLS，4.x 驱动类为 net.snowflake.client.api.driver.SnowflakeDriver）；host 填账户 URL（形如 org-account.snowflakecomputing.com），数据库填 db 或 db/schema；角色/仓库/OAuth 等扩展请用自定义 JDBC。服务器与认证仍须实际验收。",
        }),
        "dremio" => Some(Vendor {
            kind: "dremio", kinds: &["dremio"], version: "26.0.5-202509091642240013-f5051a07",
            class: "com.dremio.jdbc.Driver", prefix: "dremio", class_major: 55,
            manifest_version: "26.0.5-202509091642240013-f5051a07", probe_url: "jdbc:dremio:direct=127.0.0.1:31010",
            jar: spec("https://download.dremio.com/jdbc-driver/26.0.5-202509091642240013-f5051a07/dremio-jdbc-driver-26.0.5-202509091642240013-f5051a07.jar", 48621894, "57bb472dd287f83adc4074ae9aba800cb7684aaaffd6fdaaad88dc364570a42c"),
            pom: None, license: None, entries: 24971, expanded: 124324023, max_expanded: 128 * 1024 * 1024,
            limits: "官方 Dremio legacy direct RPC all-in-one JDBC 26.0.5；不是 Arrow Flight SQL，后者需独立端点/版本。原包含全部 shaded 依赖与 LICENSE/NOTICE；未发布 POM，不伪造。Java 11/17 需 java.nio 官方 opens 参数，已自动添加。legacy 已弃用；服务器仍须实际验收。TLS/自定义信任库请显式配置自定义 JDBC。",
        }),
        "ignite" => Some(Vendor {
            kind: "ignite", kinds: &["ignite"], version: "2.18.0", class: "org.apache.ignite.IgniteJdbcThinDriver", prefix: "ignite",
            manifest_version: "2.18.0", class_major: 55, probe_url: "jdbc:ignite:thin://127.0.0.1:10800/PUBLIC",
            jar: spec("https://repo.maven.apache.org/maven2/org/apache/ignite/ignite-core/2.18.0/ignite-core-2.18.0.jar", 14379054, "f12296bc612ac53a318d89aa8eebd45433b845a1eaedb651548c307e3200b03b"),
            pom: Some(spec("https://repo.maven.apache.org/maven2/org/apache/ignite/ignite-core/2.18.0/ignite-core-2.18.0.pom", 5610, "3ed15577b56b95c0e8169b327c9b6686cb384751a8612678a3bf8997bb8a06f3")),
            license: None, entries: 7472, expanded: 33922209, max_expanded: 64 * 1024 * 1024,
            limits: "仅官方 Apache Ignite 2.18.0 thin JDBC，非 Ignite 3；完整包为 ignite-core 与六项固定必需依赖，保留各原始 POM/许可（cache-api 1.0.0 为其原 JSR-107 许可，不统一改名）。仅已核验 Java 11/17，自动添加各版本完整官方模块参数。TLS/证书/集群扩展请自定义 JDBC；驱动 ready 不等于服务器已验收。",
        }),
        "access" => Some(Vendor {
            kind: "access", kinds: &["access"], version: "5.3.0", class: "net.ucanaccess.jdbc.UcanaccessDriver", prefix: "ucanaccess",
            manifest_version: "5.3.0", class_major: 55, probe_url: "jdbc:ucanaccess://navkit-runtime-probe.accdb",
            jar: spec("https://repo.maven.apache.org/maven2/io/github/spannm/ucanaccess/5.3.0/ucanaccess-5.3.0-uber.jar", 3531077, "64da615521487819b9f6d295cad4c7c3dc136299d6fc6fe1a0178a98398c9129"),
            pom: Some(spec("https://repo.maven.apache.org/maven2/io/github/spannm/ucanaccess/5.3.0/ucanaccess-5.3.0.pom", 34727, "1706372ab654f91c32201a2bcbf4b5ee6e7c570ee8abf1dd7e5c762e76ed0dc2")),
            license: None, entries: 1803, expanded: 9526677, max_expanded: 32 * 1024 * 1024,
            limits: "官方 UCanAccess 5.3.0 uber 原包，包含 shaded Jackcess/HSQLDB 及原 LICENSE/NOTICE；Java 11+，直接读写本地 .mdb/.accdb，不需要 Microsoft Access 或 ODBC。仅普通 SQL/事务/文件重开经过验证；高级 OLE/Office/加密可选依赖不在此完整包范围。新建文件须明确参数 newDatabaseVersion=V2010；文件路径不能含 JDBC 属性分隔符。",
        }),
        "yashandb" => Some(Vendor {
            kind: "yashandb", kinds: &["yashandb"], version: "1.10.7", class: "com.yashandb.jdbc.Driver", prefix: "yasdb",
            manifest_version: "", class_major: 52, probe_url: "jdbc:yasdb://127.0.0.1:1688/",
            jar: spec("https://repo.maven.apache.org/maven2/com/yashandb/yashandb-jdbc/1.10.7/yashandb-jdbc-1.10.7.jar", 535712, "0814694886721c050f2c4a3103dd41733a0426f8cbc18f6a46e9b356c639f5c4"),
            pom: Some(spec("https://repo.maven.apache.org/maven2/com/yashandb/yashandb-jdbc/1.10.7/yashandb-jdbc-1.10.7.pom", 1215, "024f27151a1bfd2d95459094f08e3eb40ec8b4cec8e006a783f9971c3e8f8623")),
            license: None, entries: 359, expanded: 1130754, max_expanded: 16 * 1024 * 1024,
            limits: "官方 YashanDB JDBC 1.10.7，原 JAR/POM/Apache-2.0 许可完整保留；固定实际 JAR 摘要与官方 .sha256 相符，不使用不一致的 Gradle .module 记录。Java 11/17 实例/SPI/闭回环传输已验；服务器 SQL/认证/TLS/TLCP/UKey 未验，TLS 需求须自定义 JDBC，不会降级。",
        }),
        "xugu" => Some(Vendor {
            kind: "xugu", kinds: &["xugu"], version: "12.3.9", class: "com.xugu.cloudjdbc.Driver", prefix: "xugu",
            manifest_version: "", class_major: 52, probe_url: "jdbc:xugu://127.0.0.1:5138/SYSTEM",
            jar: spec("https://repo.maven.apache.org/maven2/com/xugudb/xugu-jdbc/12.3.9/xugu-jdbc-12.3.9.jar", 713400, "eab7e0ad4ea8c462f76dee11f0c0513dbff04da40005ec582d626ffca7eb3ced"),
            pom: Some(spec("https://repo.maven.apache.org/maven2/com/xugudb/xugu-jdbc/12.3.9/xugu-jdbc-12.3.9.pom", 14215, "4decc44bc1d2d4bc064fc374c72b1aedf72d86b16933f8c918ce47c004de858e")),
            license: None, entries: 193, expanded: 1635249, max_expanded: 16 * 1024 * 1024,
            limits: "官方虚谷 JDBC 12.3.9 原包与固定 Checker Qual 3.5.0 必需运行依赖；保留原 POM，厂商声明 Proprietary，不改签名或伪标开源许可。普通 classpath 模式不加载 OSGi 容器；Java 11/17 已验证类加载、几何序列化与真实回环传输。TLS/native/厂商认证和 SQL 仍须实库验收，TLS 需求请用自定义 JDBC。",
        }),
        "iris" => Some(Vendor {
            kind: "iris", kinds: &["iris"], version: "3.11.0", class: "com.intersystems.jdbc.IRISDriver", prefix: "IRIS",
            manifest_version: "3.11.0", class_major: 52, probe_url: "jdbc:IRIS://127.0.0.1:1972/USER",
            jar: spec("https://repo.maven.apache.org/maven2/com/intersystems/intersystems-jdbc/3.11.0/intersystems-jdbc-3.11.0.jar", 607851, "08469804eadbfe62f8139b42cd81af814180300f006d5132dd1ab11c8f9d80ce"),
            pom: Some(spec("https://repo.maven.apache.org/maven2/com/intersystems/intersystems-jdbc/3.11.0/intersystems-jdbc-3.11.0.pom", 1565, "f3ae5a2c532118b9558d5acec29b6f418d9b79e80a191546a241f63a5c6ecbe7")),
            license: None, entries: 181, expanded: 1289727, max_expanded: 16 * 1024 * 1024,
            limits: "官方 InterSystems IRIS JDBC 3.11.0 原始签名 JAR/POM，保留原 IERTU 条款链接；不是 Caché 驱动，不推断再分发授权。Java 11/17 已验证完整包与真实 TCP 回环传输；数据库名填 IRIS namespace，空值使用 USER。SharedMemory/TLS/客户端证书等扩展请用自定义 JDBC；厂商服务器 SQL/认证仍须实库验收。",
        }),
        "bigquery" => Some(Vendor {
            kind: "bigquery", kinds: &["bigquery"], version: "1.9.0", class: "com.google.cloud.bigquery.jdbc.BigQueryDriver", prefix: "bigquery",
            manifest_version: "1.9.0", class_major: 52,
            probe_url: "jdbc:bigquery://bigquery.googleapis.com:443;ProjectId=navkit-offline-probe;OAuthType=2;OAuthAccessTokenReadonly=0;EnableDiagnosticTelemetry=0;EnableHighThroughputAPI=0;EnableWriteAPI=0;EnableProjectDiscovery=0;enableGcpTraceExporter=false;enableGcpLogExporter=false;useGlobalOpenTelemetry=false",
            jar: spec("https://repo.maven.apache.org/maven2/com/google/cloud/google-cloud-bigquery-jdbc/1.9.0/google-cloud-bigquery-jdbc-1.9.0-all.jar", 68007731, "023298ec36f9814e74a9a850c06e4e40ed455663fbee6637fa758d06f23c62e9"),
            pom: Some(spec("https://repo.maven.apache.org/maven2/com/google/cloud/google-cloud-bigquery-jdbc/1.9.0/google-cloud-bigquery-jdbc-1.9.0.pom", 97953, "5b33097c8437652aa24afec9a0c4dd5de74662ff486012eef638cfc1bd64e9d0")),
            license: None, entries: 29085, expanded: 191570473, max_expanded: 256 * 1024 * 1024,
            limits: "官方 Google BigQuery JDBC 1.9.0 完整 all JAR（原包包含运行依赖与许可），需 Java 11+。数据库填 project 或 project/dataset，用户名 token、密码填写预生成 OAuth 访问令牌，令牌只走 stdin Properties，不进入 URL。默认固定 Google HTTPS；OAuth 刷新、服务账户、ADC、任意端点扩展请用自定义 JDBC。离线加载/真实闭回环查询失败已验证，Google 账户权限、云端查询与 TLS 握手仍 UNVERIFIED。",
        }),
        "spanner" => Some(spanner::vendor()),
        _ => vendor_assets::vendor(kind).or_else(|| dbx::vendor(kind)),
    }
}
pub fn supports(kind: &str) -> bool {
    vendor(kind).is_some()
}
pub fn package_dir(kind: &str) -> PathBuf {
    let package_kind = vendor(kind).map(|v| v.kind).unwrap_or(kind);
    crate::storage::data_dir()
        .join("managed-drivers")
        .join(package_kind)
        .join("jdbc")
}
/// 推荐版本之外的已核验历史版本目录（推荐版本由 vendor() 提供，排在最前）。
/// 新增条目必须有完整核验值（URL/大小/SHA-256/条目数/展开大小），与 vendor() 同一标准。
fn extra_versions(kind: &str) -> Vec<Vendor> {
    match kind {
        "trino" => vec![Vendor {
            kind: "trino", kinds: &["trino"], version: "476", class: "io.trino.jdbc.TrinoDriver", prefix: "trino",
            manifest_version: "476", class_major: 55, probe_url: "jdbc:trino://127.0.0.1:8080",
            jar: spec("https://repo.maven.apache.org/maven2/io/trino/trino-jdbc/476/trino-jdbc-476.jar", 12766107, "a3736d21609a5a317d2ce88b3835e312e9b6f41393c1d4b09f2ad6e7aeb3bb90"),
            pom: Some(spec("https://repo.maven.apache.org/maven2/io/trino/trino-jdbc/476/trino-jdbc-476.pom", 42721, "c12cb645c0991d3e4bbad14da78f46fedbb06df25706d4e98034b11918da00e1")),
            license: Some(spec("https://raw.githubusercontent.com/trinodb/trino/476/LICENSE", 11358, "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30")),
            entries: 6139, expanded: 30261292, max_expanded: 64 * 1024 * 1024,
            limits: "仅官方 Trino JDBC 476（Java 11 字节码，NavKit 需 Java 11+ source launcher）；不是 PrestoDB/PrestoSQL、Starburst 商业驱动或 Hive/Spark 通用驱动。服务器版本/认证扩展仍须实际验收。",
        }],
        _ => Vec::new(),
    }
}
/// 该产品的全部已核验版本（推荐版本在最前）。驱动管理按版本展示/下载/删除。
pub fn versions(kind: &str) -> Vec<Vendor> {
    let mut all = extra_versions(kind);
    if let Some(v) = vendor(kind) { all.insert(0, v); }
    all
}
/// 按版本号查找已核验且明确来源的原包定义。
pub fn vendor_version(kind: &str, version: &str) -> Option<Vendor> {
    versions(kind).into_iter().find(|v| v.version == version)
}
/// 版本化包目录：data/managed-drivers/<kind>/jdbc/<version>。
pub fn package_dir_for(v: &Vendor) -> PathBuf {
    crate::storage::data_dir()
        .join("managed-drivers")
        .join(v.kind)
        .join("jdbc")
        .join(v.version)
}
/// 解析该版本的实际包目录：优先版本化目录；旧布局（无版本段）若 manifest
/// 与该产品/版本一致则继续使用（向后兼容已下载的旧包）；都没有则返回版本化目录（下载目标）。
pub fn resolve_package_dir(v: &Vendor) -> PathBuf {
    let versioned = package_dir_for(v);
    if versioned.exists() { return versioned; }
    let legacy = crate::storage::data_dir()
        .join("managed-drivers")
        .join(v.kind)
        .join("jdbc");
    if legacy.exists() {
        if let Ok(m) = serde_json::from_slice::<Manifest>(&fs::read(legacy.join("manifest.json")).unwrap_or_default()) {
            if m.format == 1 && m.kind == v.kind && m.version == v.version {
                return legacy;
            }
        }
    }
    versioned
}
#[derive(Serialize, Deserialize)]
struct Manifest {
    format: u32,
    kind: String,
    version: String,
    class: String,
}
fn check_asset(path: &Path, spec: &DownloadSpec) -> Result<()> {
    if fs::symlink_metadata(path)?.file_type().is_symlink()
        || !path.is_file()
        || fs::metadata(path)?.len() != spec.size
        || digest_file(path)? != spec.sha256
    {
        bail!(
            "固定原件不完整、被替换或 SHA-256 不符：{}",
            path.display()
        )
    }
    Ok(())
}
fn verify_jar(path: &Path, v: &Vendor) -> Result<()> {
    check_asset(path, &v.jar)?;
    let mut jar = zip::ZipArchive::new(fs::File::open(path)?)?;
    if jar.len() != v.entries {
        bail!("JDBC 原 JAR 条目数不符")
    }
    let mut names = HashSet::new();
    let mut expanded = 0u64;
    let mut manifest = None;
    let mut spi = None;
    let mut class = None;
    let driver_path = format!("{}.class", v.class.replace('.', "/"));
    for i in 0..jar.len() {
        let mut entry = jar.by_index(i)?;
        let name = entry.name().to_string();
        if entry.enclosed_name().is_none()
            || !safe_relative(name.trim_end_matches('/'))
            || !names.insert(name.clone())
            || entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
        {
            bail!("JDBC JAR 包含不安全、重复或符号链接条目")
        }
        if entry.is_dir() {
            continue;
        }
        expanded = expanded
            .checked_add(entry.size())
            .ok_or_else(|| anyhow!("JAR 大小溢出"))?;
        if expanded > v.expanded || expanded > v.max_expanded {
            bail!("JDBC JAR 展开大小超过固定资产记录")
        }
        let size = entry.size();
        if name == "META-INF/MANIFEST.MF"
            || name == "META-INF/services/java.sql.Driver"
            || name == driver_path
        {
            if size > 1024 * 1024 {
                bail!("JDBC class/SPI/Manifest 异常")
            }
            let mut bytes = Vec::new();
            entry.by_ref().take(size + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 != size {
                bail!("JDBC ZIP 条目被截断")
            }
            if name == driver_path {
                class = Some(bytes)
            } else if name == "META-INF/services/java.sql.Driver" {
                spi = Some(bytes)
            } else {
                manifest = Some(bytes)
            }
        } else if std::io::copy(&mut entry.by_ref().take(size + 1), &mut std::io::sink())? != size {
            bail!("JDBC ZIP 条目被截断")
        }
    }
    if expanded != v.expanded {
        bail!("JDBC 原 JAR 全展开大小不符")
    }
    let manifest = String::from_utf8(manifest.ok_or_else(|| anyhow!("缺少 JAR Manifest"))?)?;
    // 前缀匹配：部分官方 JAR 的版本行带构建元数据（SAP 哈希、达梦 Production、
    // Jaybird build 串）。JAR 的整体 SHA-256/条目数/展开大小已经固定产品身份，
    // 这里只做防御性的版本一致性检查；空 manifest_version 表示该产品无版本行。
    if !v.manifest_version.is_empty()
        && !manifest.lines().any(|line| {
            line.trim().starts_with(&format!("Implementation-Version: {}", v.manifest_version))
        })
    {
        bail!("JDBC 产品正式版本不符")
    }
    if v.kind == "xugu" && !manifest.lines().any(|line| line.trim() == "Bundle-Version: 12.3.9") {
        bail!("虚谷官方 Bundle-Version 不符")
    }
    let spi = String::from_utf8(spi.ok_or_else(|| anyhow!("缺少 JDBC SPI"))?)?;
    let drivers: Vec<_> = spi
        .lines()
        .map(|s| s.split('#').next().unwrap_or("").trim())
        .filter(|s| !s.is_empty())
        .collect();
    let expected_drivers: &[&str] = if v.kind == "informix" {
        &["com.informix.jdbc.IfxDriver", "com.informix.jdbc.InformixDriver"]
    } else if v.kind == "ignite" {
        &["org.apache.ignite.IgniteJdbcDriver", "org.apache.ignite.IgniteJdbcThinDriver"]
    } else if v.kind == "access" {
        &["net.ucanaccess.jdbc.UcanaccessDriver", "shaded.org.hsqldb.jdbc.JDBCDriver"]
    } else { std::slice::from_ref(&v.class) };
    if drivers != expected_drivers {
        bail!("JDBC SPI 不属于这个产品")
    }
    let class = class.ok_or_else(|| anyhow!("缺少具体产品 Driver.class"))?;
    if class.len() < 10
        || class[..4] != [0xca, 0xfe, 0xba, 0xbe]
        || u16::from_be_bytes([class[6], class[7]]) != v.class_major
    {
        bail!("JDBC 主类字节码版本不符，不能静默降级 Java")
    }
    Ok(())
}
fn verify_package(dir: &Path, v: &Vendor) -> Result<PathBuf> {
    let expected: HashSet<String> = ["driver.jar", "manifest.json"]
        .into_iter()
        .chain(v.pom.as_ref().map(|_| "official.pom"))
        .chain(v.license.as_ref().map(|_| "LICENSE"))
        .map(String::from)
        .chain(dependencies::specs(v.kind).iter().flat_map(|d| [d.name.to_string(), d.pom_name.to_string()]))
        .chain(vendor_assets::extra_files(v.kind).iter().map(|name| name.to_string()))
        .collect();
    if fs::symlink_metadata(dir)?.file_type().is_symlink() {
        bail!("托管 JDBC 包目录不得为链接")
    }
    let actual: HashSet<_> = fs::read_dir(dir)?
        .map(|entry| entry.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<std::io::Result<_>>()?;
    if expected != actual {
        bail!("托管 JDBC 包缺组件或有额外未验证依赖")
    }
    let manifest_path = dir.join("manifest.json");
    if fs::metadata(&manifest_path)?.len() > 4096
        || fs::symlink_metadata(&manifest_path)?
            .file_type()
            .is_symlink()
    {
        bail!("托管 JDBC manifest 异常")
    }
    let m: Manifest = serde_json::from_slice(&fs::read(manifest_path)?)?;
    if m.format != 1 || m.kind != v.kind || m.version != v.version || m.class != v.class {
        bail!("托管 JDBC 产品清单被替换")
    }
    if let Some(pom) = &v.pom { check_asset(&dir.join("official.pom"), pom)?; }
    if let Some(license) = &v.license {
        check_asset(&dir.join("LICENSE"), license)?
    }
    let jar = dir.join("driver.jar");
    verify_jar(&jar, v)?;
    vendor_assets::verify(dir, v)?;
    // Verify mandatory runtime dependencies as well as the original complete official JAR.
    // Original legal resources/NOTICE remain inside each untouched artifact.
    dependencies::verify(dir, v.kind)?;
    Ok(jar)
}
fn runtime(jar: &Path, v: &Vendor) -> Result<String> {
    let dir = jar.parent().ok_or_else(|| anyhow!("JDBC 驱动目录无效"))?;
    let dependencies = dependencies::verify(dir, v.kind)?;
    crate::jdbc_db::driver_runtime_status_with_dependencies(jar, v.class, v.probe_url, &dependencies)
}
/// A managed connection only loads the dependencies of its verified product package.
/// User-supplied JARs never load arbitrary neighbouring files.
pub(crate) fn classpath_for(jar: &Path) -> Result<std::ffi::OsString> {
    for kind in ["trino", "prestosql", "databricks", "db2", "informix", "saphana", "teradata", "vertica", "exasol", "dameng", "firebird", "oracle", "hive", "kylin", "snowflake", "dremio", "ignite", "access", "yashandb", "xugu", "iris", "bigquery", "gbase", "inceptor", "spanner", "cache", "sundb", "oscar", "argodb"] {
        for v in versions(kind) {
            let dir = resolve_package_dir(&v);
            if jar == dir.join("driver.jar") {
                verify_package(&dir, &v)?;
                let paths = std::iter::once(jar.to_path_buf()).chain(dependencies::verify(&dir, v.kind)?);
                return std::env::join_paths(paths).context("JDBC classpath 路径无效");
            }
        }
    }
    std::env::join_paths([jar]).context("JDBC JAR 路径无效")
}
fn status_at(v: &Vendor, dir: &Path, kind: &str) -> DriverStatus {
    let mut s = DriverStatus {
        kind: kind.into(),
        version: v.version.into(),
        status: "missing".into(),
        architecture: "JDBC / Java 11+".into(),
        message: v.limits.into(),
        can_download: true,
        library: None,
        download_path: None,
    };
    match verify_package(dir, v) {
        Err(e) => {
            s.message = format!(
                "缺少完整托管 JDBC 包或校验失败：{e:#}。可直接下载固定原版本。{}",
                v.limits
            )
        }
        Ok(jar) => {
            s.download_path = Some(dir.to_string_lossy().into_owned());
            match runtime(&jar, v) {
                Ok(probe) => {
                    s.status = "ready".into();
                    s.can_download = false;
                    s.library = Some(jar.to_string_lossy().into_owned());
                    s.message = format!("完整原 JDBC 包、固定 SHA-256、全 ZIP CRC、版本/class/SPI、真实 source launcher/驱动实例与 URL 认可验证通过（{probe}）。标准连接默认走该 JDBC，凭据仅传 stdin；数据库服务仍 UNVERIFIED，需实际连接。{}", v.limits);
                }
                Err(e) => {
                    s.status = "manual".into();
                    s.can_download = false;
                    s.message = format!("原 JDBC 包完整但实际 Java/驱动探针失败：{e:#}。不会标记 ready，不安装 Java、不降级 SSL。{}", v.limits);
                }
            }
        }
    }
    s
}
/// Parent calls only AFTER checking a genuinely usable installed/native ODBC library.
pub fn status(kind: &str) -> Option<DriverStatus> {
    vendor(kind).map(|v| status_at(&v, &resolve_package_dir(&v), kind))
}
/// 按版本展开的全部状态（推荐版本在最前）；驱动管理「官方驱动」页逐版本展示。
pub fn statuses(kind: &str) -> Vec<DriverStatus> {
    versions(kind).iter().map(|v| status_at(v, &resolve_package_dir(v), kind)).collect()
}

// Pinned textual assets remain small, but BigQuery's original POM is 97,953 bytes.
const MAX_TEXT_ASSET_BYTES: u64 = 128 * 1024;
fn text_asset_url(spec: &DownloadSpec) -> Result<reqwest::Url> {
    if spec.size == 0 || spec.size > MAX_TEXT_ASSET_BYTES {
        bail!("JDBC 许可/清单大小超出限制")
    }
    let url = reqwest::Url::parse(spec.url)?;
    if !super::trusted_url(&url) {
        bail!("不允许的 JDBC 许可/清单下载源")
    }
    Ok(url)
}
// Binary JARs use the separate bounded range downloader. Every text asset retains its exact size/hash.
async fn download_text(spec: &DownloadSpec, path: &Path) -> Result<()> {
    let url = text_asset_url(spec)?;
    let mut response = super::client()?.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|size| size != spec.size)
    {
        bail!("JDBC 许可/清单大小不符")
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() as u64 + chunk.len() as u64 > spec.size {
            bail!("JDBC 许可/清单超出固定大小")
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() as u64 != spec.size {
        bail!("JDBC 许可/清单不完整")
    }
    fs::write(path, bytes)?;
    check_asset(path, spec)
}
fn publish(staging: &Path, destination: &Path) -> Result<Option<PathBuf>> {
    super::publication::replace(&staging.join("package"), destination)
}
pub async fn download(kind: &str, version: Option<&str>) -> Result<DriverStatus, String> {
    let v = match version {
        // 明确请求某版本：只下载该已核验版本，不做静默替换。
        Some(requested) => vendor_version(kind, requested)
            .ok_or(format!("数据源 {kind} 没有已核验的托管 JDBC 版本 {requested}"))?,
        None => vendor(kind).ok_or("这个产品没有已核验的托管 JDBC 自动下载链")?,
    };
    let permit = DOWNLOAD_GATE
        .get_or_init(|| tokio::sync::Semaphore::new(1))
        .try_acquire()
        .map_err(|_| "已有驱动正在下载，请稍后重试")?;
    let kind = kind.to_string(); // spawn_blocking 需要 'static
    let destination = package_dir_for(&v);
    let parent = destination.parent().ok_or("JDBC 目录无效")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let staging = Staging(parent.join(format!(".jdbc-download-{}", uuid::Uuid::new_v4())));
    fs::create_dir(&staging.0).map_err(|e| e.to_string())?;
    let package = staging.0.join("package");
    fs::create_dir(&package).map_err(|e| e.to_string())?;
    tokio::time::timeout(Duration::from_secs(180), async {
        if !vendor_assets::download(&v, &package).await? {
            super::download_file(&v.jar, &package.join("driver.jar")).await?;
        }
        if let Some(pom) = &v.pom { download_text(pom, &package.join("official.pom")).await?; }
        for dep in dependencies::specs(v.kind) {
            super::download_file(&dep.jar, &package.join(dep.name)).await?;
            download_text(&dep.pom, &package.join(dep.pom_name)).await?;
        }
        if let Some(license) = &v.license {
            download_text(license, &package.join("LICENSE")).await?;
        }
        Ok::<(), anyhow::Error>(())
    })
    .await
    .map_err(|_| "JDBC 全包下载 180 秒超时，partial 已清理")?
    .map_err(|e| format!("JDBC 下载失败：{e:#}"))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit; // keep global permit/staging alive even if an IPC is cancelled
        fs::write(
            package.join("manifest.json"),
            serde_json::to_vec_pretty(&Manifest {
                format: 1,
                kind: v.kind.into(),
                version: v.version.into(),
                class: v.class.into(),
            })
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        verify_package(&package, &v).map_err(|e| format!("JDBC 全包校验失败：{e:#}"))?;
        // A missing/incompatible Java still leaves the verified package available, but only manual.
        // Status after publication MUST run the real probe; never infer readiness from a download.
        let retained_backup = publish(&staging.0, &destination).map_err(|e| format!("JDBC 原子发布失败：{e:#}"))?;
        let mut status = status_at(&v, &destination, &kind);
        if let Some(path) = retained_backup { status.message.push_str(&format!(" 原包备份暂被占用，保留于 {}。", path.display())); }
        Ok(status)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn installed_odbc_ready(kind: &str, inventory: Option<&OdbcInventory>) -> bool {
    let Some(meta) = crate::catalog::meta(kind) else {
        return false;
    };
    inventory.is_some_and(|i| {
        i.drivers.iter().any(|d| {
            !meta.driver.is_empty()
                && d.name.eq_ignore_ascii_case(meta.driver)
                && super::driver_path(&d.library).is_some_and(|p| {
                    crate::driver_manager::validate_driver_file(&p.to_string_lossy()).is_ok()
                })
        })
    })
}
/// TLS 设置统一为一个四值模型：Plain（显式明文）/ Require / VerifyCa / VerifyFull。
/// ssl_verify=false 一律拒绝；互相矛盾的参数一律拒绝。None 表示用户未表达。
enum TlsMode { Plain, Require, VerifyCa, VerifyFull }
fn tls_mode(pairs: &[(String, String)]) -> Result<Option<TlsMode>> {
    use TlsMode::*;
    let mut ssl = None;
    for (key, value) in pairs {
        let setting = match key.as_str() {
            "ssl" | "tls" => Some(match bool_value(value)? {
                true => Require,
                false => Plain,
            }),
            "sslmode" | "ssl-mode" => {
                Some(match value.trim().to_ascii_lowercase().as_str() {
                    "disable" | "disabled" => Plain,
                    "require" | "required" => Require,
                    "verify-ca" => VerifyCa,
                    "verify-full" => VerifyFull,
                    _ => bail!(
                        "托管 JDBC 不支持可能降级明文的 sslmode，请明确 disable 或 require/verify-full"
                    ),
                })
            }
            "ssl_verify" => {
                if !bool_value(value)? {
                    bail!("托管 JDBC 不关闭 TLS 证书验证")
                }
                None
            }
            _ => None,
        };
        if let Some(setting) = setting {
            if ssl.is_some_and(|old: TlsMode| std::mem::discriminant(&old) != std::mem::discriminant(&setting)) {
                bail!("托管 JDBC TLS 参数互相矛盾")
            }
            ssl = Some(setting);
        }
    }
    Ok(ssl)
}
fn params(v: &Vendor, req: &ConnectionRequest) -> Result<Option<Vec<(String, String)>>> {
    let text = req.params.trim();
    if text.starts_with(PARAMS_PREFIX) {
        return Ok(None);
    }
    let pairs = if text.starts_with('{') || text.starts_with('[') {
        let value: serde_json::Value =
            serde_json::from_str(text).context("托管 JDBC 参数 JSON 损坏")?;
        let object = value
            .as_object()
            .ok_or_else(|| anyhow!("托管 JDBC 参数必须为 JSON 对象"))?;
        object
            .iter()
            .map(|(k, v)| {
                (
                    k.to_ascii_lowercase(),
                    v.as_str()
                        .map(String::from)
                        .unwrap_or_else(|| v.to_string()),
                )
            })
            .collect::<Vec<_>>()
    } else {
        crate::db::extra_params(text)
            .into_iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v))
            .collect()
    };
    let mut allowed = vec![
        "catalog",
        "schema",
        "ssl",
        "tls",
        "sslmode",
        "ssl-mode",
        "ssl_verify",
    ];
    match v.kind {
        "databricks" => allowed.extend(["httppath", "http_path", "authmech"]),
        "informix" => allowed.extend(["informixserver", "informix_server"]),
        "dameng" => allowed.extend(["schema"]),
        "access" => allowed.extend(["newdatabaseversion"]),
        "oracle" => allowed.extend(["service", "sid", "oracle_connect_as"]),
        _ => {}
    }
    // Preserve explicit ODBC/DSN/connection strings and unsupported options instead of discarding them.
    if (!text.is_empty() && pairs.is_empty())
        || pairs.iter().any(|(k, _)| !allowed.contains(&k.as_str()))
    {
        return Ok(None);
    }
    Ok(Some(pairs))
}
fn bool_value(value: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => bail!("托管 JDBC TLS 布尔参数无效"),
    }
}
fn percent(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn validated_host(req: &ConnectionRequest) -> Result<String> {
    let host = req.host.trim();
    if host.is_empty()
        || host.chars().any(|c| {
            c.is_whitespace() || matches!(c, '@' | '/' | '\\' | '?' | '#' | '%' | ';' | '\0')
        })
    {
        bail!("托管 JDBC host 必须为主机名或 IP，不能包含凭据/URL/连接串")
    }
    if host.starts_with('[') && host.ends_with(']') {
        let _: std::net::Ipv6Addr = host[1..host.len() - 1]
            .parse()
            .context("JDBC IPv6 host 无效")?;
        return Ok(host.to_string());
    }
    if host.contains(':') {
        let _: std::net::Ipv6Addr = host.parse().context("JDBC IPv6 host 无效")?;
        return Ok(format!("[{host}]"));
    }
    Ok(host.to_string())
}
fn server_authority(req: &ConnectionRequest, default_port: u16) -> Result<String> {
    let port = if req.port == 0 { default_port } else { req.port };
    Ok(format!("{}:{port}", validated_host(req)?))
}
/// 简单标识符（数据库名 / schema / project / server 名）的公共白名单：
/// 允许字母数字与 _ - . 及任意非 ASCII（支持中文库名），拒绝 URL/驱动分隔符。
fn plain_name<'a>(value: &'a str, label: &str) -> Result<&'a str> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().any(|c| {
            matches!(c, '/' | '\\' | '?' | '#' | ';' | '&' | '=' | '%' | '@' | ':' | '\0')
                || c.is_whitespace()
        })
    {
        bail!("{label} 不能为空且不能包含 URL/连接串分隔符或空白")
    }
    Ok(value)
}
fn connection_url(
    req: &ConnectionRequest,
    v: &Vendor,
    pairs: &[(String, String)],
) -> Result<String> {
    match v.kind {
        "databricks" => databricks_url(req, pairs),
        "db2" => db2_url(req, pairs),
        "informix" => informix_url(req, pairs),
        "saphana" => saphana_url(req, pairs),
        "teradata" => teradata_url(req, pairs),
        "vertica" => vertica_url(req, pairs),
        "exasol" => exasol_url(req, pairs),
        "dameng" => dameng_url(req, pairs),
        "firebird" => firebird_url(req, pairs),
        "oracle" => oracle_thin_url(req, pairs),
        "hive" => hive_url(req, pairs),
        "kylin" => kylin_url(req, pairs),
        "snowflake" => snowflake_url(req, pairs),
        "dremio" | "ignite" => remaining_url(req, v.kind, pairs),
        "access" => access_url(req, pairs),
        "yashandb" => yashan_url(req, pairs),
        "xugu" | "iris" => xugu_iris_url(req, v.kind, pairs),
        "bigquery" => bigquery_url(req, pairs),
        "spanner" => spanner::connection_url(req, pairs),
        "gbase" | "inceptor" | "argodb" => vendor_assets::connection_url(req, v.kind, pairs),
        "cache" | "sundb" | "oscar" => dbx::connection_url(req, v.kind, pairs),
        _ => trino_presto_url(req, v, pairs),
    }
}
// A deliberately bounded PAT-only contract. Credentials remain stdin properties,
// never URL options; no OAuth flow, arbitrary semicolon option or TLS bypass is inferred.
fn databricks_url(req: &ConnectionRequest, pairs: &[(String, String)]) -> Result<String> {
    let authority = server_authority(req, 443)?;
    if matches!(tls_mode(pairs)?, Some(TlsMode::Plain)) {
        bail!("Databricks 托管 JDBC 必须启用 TLS，不接受 SSL=0/disable")
    }
    let get = |keys: &[&str]| -> Result<Option<&str>> {
        let values: Vec<_> = pairs.iter().filter(|(key, _)| keys.contains(&key.as_str())).collect();
        if values.len() > 1 { bail!("Databricks JDBC 参数重复或别名冲突") }
        Ok(values.first().map(|(_, value)| value.as_str()))
    };
    if get(&["authmech"])?.is_some_and(|value| value != "3") {
        bail!("Databricks 托管 JDBC 仅支持 AuthMech=3 PAT；OAuth 请使用显式自定义 JDBC")
    }
    if req.user != "token" || req.password.is_empty() {
        bail!("Databricks PAT 连接要求用户名 token、密码填写 PAT；凭据不会写入 JDBC URL")
    }
    let http_path = get(&["httppath", "http_path"])?.ok_or_else(|| anyhow!("Databricks JDBC 必须填写 httpPath（计算资源连接详情中的 HTTP 路径）"))?;
    if !http_path.starts_with('/') || http_path.len() > 2048
        || http_path.split('/').skip(1).any(|part| part.is_empty() || part == "." || part == "..")
        || !http_path.bytes().all(|b| b.is_ascii_alphanumeric() || b"/_.-".contains(&b)) {
        bail!("Databricks httpPath 必须为安全的绝对 HTTP 路径，不能包含连接串/凭据/URL 参数")
    }
    let database = req.database.trim();
    let parts: Vec<_> = database.split('/').collect();
    if parts.len() > 2 { bail!("Databricks database 格式为 catalog 或 catalog/schema") }
    let catalog = get(&["catalog"])?.unwrap_or(parts[0]);
    let schema = get(&["schema"])?.or_else(|| parts.get(1).copied()).unwrap_or("default");
    // The driver's semicolon property parser is not Trino's percent-decoded path.
    // Reject reserved delimiters rather than silently changing names or options.
    for value in [catalog, schema] {
        if value.len() > 255 || (!value.is_empty() && !value.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')) {
            bail!("Databricks catalog/schema 仅支持字母、数字、下划线和连字符；复杂名称请显式配置自定义 JDBC")
        }
    }
    if schema.is_empty() { bail!("Databricks schema 不能为空") }
    let mut url = format!("jdbc:databricks://{authority}/{schema};httpPath={http_path};AuthMech=3;SSL=1");
    if !catalog.is_empty() { url.push_str(&format!(";ConnCatalog={catalog}")); }
    Ok(url)
}
fn trino_presto_url(
    req: &ConnectionRequest,
    v: &Vendor,
    pairs: &[(String, String)],
) -> Result<String> {    let authority = server_authority(req, 8080)?;
    let ssl = tls_mode(pairs)?;
    let database = req.database.trim();
    let db_parts: Vec<_> = database.split('/').collect();
    if db_parts.len() > 2 {
        bail!("JDBC database 格式为 catalog 或 catalog/schema")
    }
    let get = |key: &str| {
        pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    let catalog = get("catalog").unwrap_or(db_parts[0]);
    let schema = get("schema")
        .or_else(|| db_parts.get(1).copied())
        .unwrap_or("");
    if !schema.is_empty() && catalog.is_empty() {
        bail!("JDBC schema 必须同时指定 catalog")
    }
    let mut url = format!("jdbc:{}://{authority}", v.prefix);
    if !catalog.is_empty() {
        url.push('/');
        url.push_str(&percent(catalog));
        if !schema.is_empty() {
            url.push('/');
            url.push_str(&percent(schema));
        }
    }
    url.push_str(if matches!(ssl, Some(TlsMode::Require | TlsMode::VerifyCa | TlsMode::VerifyFull)) {
        "?SSL=true"
    } else {
        "?SSL=false"
    });
    Ok(url)
}
fn db2_url(req: &ConnectionRequest, pairs: &[(String, String)]) -> Result<String> {
    let authority = server_authority(req, 50000)?;
    let ssl = tls_mode(pairs)?;
    let db = plain_name(req.database.trim(), "DB2 数据库名")?;
    let mut url = format!("jdbc:db2://{authority}/{db}");
    // 实测接受的 TLS 形态只有 URL 属性 :sslConnection=true;；明文是驱动默认，无需写入。
    if matches!(ssl, Some(TlsMode::Require | TlsMode::VerifyCa | TlsMode::VerifyFull)) {
        url.push_str(":sslConnection=true;");
    }
    Ok(url)
}
fn informix_url(req: &ConnectionRequest, pairs: &[(String, String)]) -> Result<String> {
    let authority = server_authority(req, 9088)?;
    if tls_mode(pairs)?.is_some() {
        bail!("Informix 托管 JDBC 没有经核验的 URL TLS 参数；TLS 由服务器 sqlhosts/环境决定，需要 TLS 请使用自定义 JDBC")
    }
    let get = |keys: &[&str]| -> Option<&str> {
        pairs.iter().find(|(k, _)| keys.contains(&k.as_str())).map(|(_, v)| v.as_str())
    };
    let server = get(&["informixserver", "informix_server"])
        .ok_or_else(|| anyhow!("Informix 需要参数 informixserver（sqlhosts 服务器名）"))?;
    let server = plain_name(server, "informixserver")?;
    let db = req.database.trim();
    let mut url = format!("jdbc:informix-sqli://{authority}");
    if !db.is_empty() {
        url.push('/');
        url.push_str(plain_name(db, "Informix 数据库名")?);
    }
    url.push_str(&format!(":INFORMIXSERVER={server}"));
    Ok(url)
}
fn saphana_url(req: &ConnectionRequest, pairs: &[(String, String)]) -> Result<String> {
    let authority = server_authority(req, 30015)?;
    let ssl = tls_mode(pairs)?;
    let db = req.database.trim();
    if !db.is_empty() { plain_name(db, "HANA databaseName")?; }
    let mut url = format!("jdbc:sap://{authority}/?");
    if !db.is_empty() {
        url.push_str(&format!("databaseName={}&", percent(db)));
    }
    // 实测：encrypt=true 必须伴随 validateCertificate=true 才是可验证的严格 TLS。
    url.push_str(if matches!(ssl, Some(TlsMode::Require | TlsMode::VerifyCa | TlsMode::VerifyFull)) {
        "encrypt=true&validateCertificate=true"
    } else {
        "encrypt=false"
    });
    Ok(url)
}
fn teradata_url(req: &ConnectionRequest, pairs: &[(String, String)]) -> Result<String> {
    let host = validated_host(req)?;
    let port = if req.port == 0 { 1025 } else { req.port };
    let db = req.database.trim();
    if !db.is_empty() { plain_name(db, "Teradata DATABASE")?; }
    let mut parts: Vec<String> = match tls_mode(pairs)? {
        Some(TlsMode::Plain) => vec![format!("DBS_PORT={port}"), "SSLMode=DISABLE".into()],
        Some(TlsMode::Require) => vec!["SSLMode=REQUIRE".into(), format!("https_port={port}")],
        Some(TlsMode::VerifyCa) => vec!["SSLMode=VERIFY-CA".into(), format!("https_port={port}")],
        Some(TlsMode::VerifyFull) => vec!["SSLMode=VERIFY-FULL".into(), format!("https_port={port}")],
        None => vec![format!("DBS_PORT={port}")],
    };
    if !db.is_empty() { parts.push(format!("DATABASE={db}")); }
    Ok(format!("jdbc:teradata://{host}/{}", parts.join(",")))
}
fn vertica_url(req: &ConnectionRequest, pairs: &[(String, String)]) -> Result<String> {
    let authority = server_authority(req, 5433)?;
    let db = req.database.trim();
    if !db.is_empty() { plain_name(db, "Vertica 数据库名")?; }
    let mut url = format!("jdbc:vertica://{authority}");
    if !db.is_empty() {
        url.push('/');
        url.push_str(&percent(db));
    }
    // 驱动公布的 tlsmode 取值：prefer|disable|verify-ca|require|verify-full。
    url.push_str(match tls_mode(pairs)? {
        Some(TlsMode::Plain) => "?tlsmode=disable",
        Some(TlsMode::Require) => "?tlsmode=require",
        Some(TlsMode::VerifyCa) => "?tlsmode=verify-ca",
        Some(TlsMode::VerifyFull) => "?tlsmode=verify-full",
        None => "",
    });
    Ok(url)
}
fn exasol_url(req: &ConnectionRequest, pairs: &[(String, String)]) -> Result<String> {
    let host = validated_host(req)?;
    let port = if req.port == 0 { 8563 } else { req.port };
    if !req.database.trim().is_empty() {
        bail!("Exasol 连接 URL 不包含数据库名，请留空数据库；固定 schema 请使用自定义 JDBC")
    }
    match tls_mode(pairs)? {
        Some(TlsMode::VerifyCa | TlsMode::VerifyFull) => {
            bail!("Exasol 托管 JDBC 不支持证书校验模式；需要严格校验请使用自定义 JDBC")
        }
        Some(TlsMode::Plain) => Ok(format!("jdbc:exa:{host}:{port};encryption=false")),
        Some(TlsMode::Require) => Ok(format!("jdbc:exa:{host}:{port};encryption=true")),
        None => Ok(format!("jdbc:exa:{host}:{port}")),
    }
}
fn dameng_url(req: &ConnectionRequest, pairs: &[(String, String)]) -> Result<String> {
    let authority = server_authority(req, 5236)?;
    if tls_mode(pairs)?.is_some() {
        bail!("DM 托管 JDBC 的 TLS 需要 sslFilesPath 等客户端证书参数；需要 TLS 请使用自定义 JDBC")
    }
    let get = |key: &str| {
        pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    };
    // 目录页的「数据库」字段承载 DM 的 schema 语义；params.schema 优先。
    let schema: Option<String> = if let Some(s) = get("schema") {
        Some(plain_name(s, "DM schema")?.to_string())
    } else if !req.database.trim().is_empty() {
        Some(plain_name(req.database.trim(), "DM schema")?.to_string())
    } else {
        None
    };
    let mut url = format!("jdbc:dm://{authority}");
    if let Some(schema) = schema {
        url.push_str(&format!("?schema={schema}"));
    }
    Ok(url)
}
fn firebird_url(req: &ConnectionRequest, pairs: &[(String, String)]) -> Result<String> {
    let authority = server_authority(req, 3050)?;
    let db = req.database.trim();
    if db.is_empty() || db.chars().any(|c| matches!(c, '?' | '&' | ';' | '#' | '\0') || c.is_whitespace()) {
        bail!("Firebird 需要服务器端数据库路径或别名（不能包含 URL 分隔符/空白）")
    }
    let mut url = format!("jdbc:firebirdsql://{authority}/{db}");
    url.push_str(match tls_mode(pairs)? {
        Some(TlsMode::Plain) => "?wireCrypt=DISABLED",
        Some(TlsMode::Require) => "?wireCrypt=REQUIRED",
        Some(TlsMode::VerifyCa | TlsMode::VerifyFull) => {
            bail!("Firebird wireCrypt 不提供证书校验模式；需要严格校验请使用自定义 JDBC")
        }
        None => "",
    });
    Ok(url)
}
fn oracle_thin_url(req: &ConnectionRequest, pairs: &[(String, String)]) -> Result<String> {
    let host = validated_host(req)?;
    let port = if req.port == 0 { 1521 } else { req.port };
    if tls_mode(pairs)?.is_some() {
        bail!("Oracle thin JDBC 的 TLS (TCPS) 需要 wallet 配置；需要 TCPS 请使用自定义 JDBC 或托管 ODBC 客户端")
    }
    let get = |key: &str| {
        pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    };
    let mode = get("oracle_connect_as").unwrap_or("service");
    if mode != "service" && mode != "sid" {
        bail!("oracle_connect_as 只能是 service 或 sid")
    }
    let sid = get("sid").map(str::trim).filter(|s| !s.is_empty());
    if mode == "sid" || sid.is_some() {
        let sid = sid.ok_or_else(|| anyhow!("SID 连接需要显式填写 params.sid，不会猜测数据库名为 SID"))?;
        let sid = plain_name(sid, "Oracle SID")?;
        Ok(format!("jdbc:oracle:thin:@{host}:{port}:{sid}"))
    } else {
        let service = get("service").map(str::trim).filter(|s| !s.is_empty())
            .unwrap_or_else(|| req.database.trim());
        let service = plain_name(service, "Oracle service name")?;
        Ok(format!("jdbc:oracle:thin:@//{host}:{port}/{service}"))
    }
}
fn hive_url(req: &ConnectionRequest, pairs: &[(String, String)]) -> Result<String> {
    let authority = server_authority(req, 10000)?;
    let db = req.database.trim();
    if !db.is_empty() { plain_name(db, "Hive 数据库名")?; }
    let mut url = format!("jdbc:hive2://{authority}");
    if !db.is_empty() {
        url.push('/');
        url.push_str(&percent(db));
    }
    match tls_mode(pairs)? {
        Some(TlsMode::Plain) => {}
        Some(TlsMode::Require) => url.push_str(";ssl=true"),
        Some(TlsMode::VerifyCa | TlsMode::VerifyFull) => {
            bail!("Hive JDBC 仅支持 ssl=true；证书校验（sslTrustStore）请使用自定义 JDBC")
        }
        None => {}
    }
    Ok(url)
}
fn kylin_url(req: &ConnectionRequest, pairs: &[(String, String)]) -> Result<String> {
    let authority = server_authority(req, 7070)?;
    if tls_mode(pairs)?.is_some() {
        bail!("Kylin 托管 JDBC 未核验 HTTPS/TLS URL 参数；TLS 部署请使用自定义 JDBC")
    }
    let project = plain_name(req.database.trim(), "Kylin project")?;
    Ok(format!("jdbc:kylin://{authority}/{project}"))
}
fn access_url(req: &ConnectionRequest, pairs: &[(String, String)]) -> Result<String> {
    if tls_mode(pairs)?.is_some() { bail!("Access 是本地文件连接，不接受 SSL/TLS 参数") }
    if pairs.iter().any(|(key, _)| key != "newdatabaseversion") { bail!("Access 托管连接只支持明确的 newDatabaseVersion=V2010；其它属性请使用自定义 JDBC") }
    if !req.database.trim().is_empty() { bail!("Access 数据库字段应留空，请填写数据库文件路径") }
    let file = req.file.trim();
    if file.is_empty() || !Path::new(file).is_absolute() || file.chars().any(|c| matches!(c, ';' | '\0' | '\r' | '\n')) {
        bail!("Access 需要数据库文件绝对路径，且路径不能含分号、NUL 或换行")
    }
    let suffix = Path::new(file).extension().and_then(|s| s.to_str()).unwrap_or("");
    if !suffix.eq_ignore_ascii_case("mdb") && !suffix.eq_ignore_ascii_case("accdb") { bail!("Access 文件必须为 .mdb 或 .accdb") }
    let mut url = format!("jdbc:ucanaccess://{};immediatelyReleaseResources=true", file.replace('\\', "/"));
    let versions: Vec<_> = pairs.iter().filter(|(key, _)| key == "newdatabaseversion").collect();
    if versions.len() > 1 { bail!("Access newDatabaseVersion 不能重复") }
    if let Some((_, version)) = versions.first() {
        if version != "V2010" { bail!("Access 托管新建文件只核验了 newDatabaseVersion=V2010") }
        url.push_str(";newDatabaseVersion=V2010");
    } else if !Path::new(file).is_file() { bail!("Access 文件不存在；新建文件请明确 newDatabaseVersion=V2010") }
    Ok(url)
}
fn yashan_url(req: &ConnectionRequest, pairs: &[(String, String)]) -> Result<String> {
    if matches!(tls_mode(pairs)?, Some(TlsMode::Require | TlsMode::VerifyCa | TlsMode::VerifyFull)) { bail!("YashanDB 托管 JDBC 未核验 TLS/TLCP 参数；不会降级明文，请使用自定义 JDBC") }
    if pairs.iter().any(|(key, _)| !["ssl", "tls", "sslmode", "ssl-mode", "ssl_verify"].contains(&key.as_str())) { bail!("YashanDB 扩展属性请显式配置自定义 JDBC") }
    let database = req.database.trim();
    if !database.is_empty() { plain_name(database, "YashanDB 数据库名")?; }
    Ok(format!("jdbc:yasdb://{}/{}", server_authority(req, 1688)?, database))
}
fn xugu_iris_url(req: &ConnectionRequest, kind: &str, pairs: &[(String, String)]) -> Result<String> {
    if matches!(tls_mode(pairs)?, Some(TlsMode::Require | TlsMode::VerifyCa | TlsMode::VerifyFull)) {
        bail!("虚谷/IRIS 托管连接尚未核验 TLS 参数，不会静默降级；请使用自定义 JDBC")
    }
    if pairs.iter().any(|(key, _)| !["ssl", "tls", "sslmode", "ssl-mode", "ssl_verify"].contains(&key.as_str())) {
        bail!("虚谷/IRIS 扩展参数需要显式自定义 JDBC，不能静默忽略")
    }
    let database = if req.database.trim().is_empty() { if kind == "iris" { "USER" } else { "SYSTEM" } }
        else { plain_name(req.database.trim(), "数据库或 namespace")? };
    let (prefix, port) = if kind == "iris" { ("IRIS", 1972) } else { ("xugu", 5138) };
    Ok(format!("jdbc:{prefix}://{}/{database}", server_authority(req, port)?))
}
fn bigquery_url(req: &ConnectionRequest, pairs: &[(String, String)]) -> Result<String> {
    if pairs.iter().any(|(key, _)| !["ssl", "tls", "sslmode", "ssl-mode", "ssl_verify"].contains(&key.as_str())) {
        bail!("BigQuery 托管连接仅支持预生成访问令牌；OAuth/ADC/任意端点参数请用自定义 JDBC")
    }
    if req.user != "token" || req.password.is_empty() || !req.password.bytes().all(|b| b.is_ascii_alphanumeric() || b"._~+/=-".contains(&b)) {
        bail!("BigQuery 用户名必须为 token，密码填写预生成 OAuth 访问令牌；令牌不会进入 URL")
    }
    let parts: Vec<_> = req.database.trim().split('/').collect();
    if parts.len() > 2 || parts.iter().any(|part| part.is_empty() || !part.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))) {
        bail!("BigQuery 数据库格式为 project 或 project/dataset，仅支持字母、数字、下划线和连字符")
    }
    let host = validated_host(req)?;
    let loopback = host == "127.0.0.1" || host == "[::1]";
    let tls = tls_mode(pairs)?;
    let mut url = if loopback {
        if !matches!(tls, Some(TlsMode::Plain)) { bail!("BigQuery 本地端点必须使用字面回环地址并显式 ssl=false；默认不重定向云令牌") }
        let authority = server_authority(req, 443)?;
        let base = format!("http://{authority}");
        format!("jdbc:bigquery://{authority};EndpointOverrides=BIGQUERY={base},OAUTH2={base}/token,READ_API={authority},STS={base}/sts")
    } else {
        if host != "bigquery.googleapis.com" || ![0,443].contains(&req.port) || matches!(tls, Some(TlsMode::Plain)) {
            bail!("BigQuery 托管云连接固定 bigquery.googleapis.com:443 和正常 HTTPS，不接受任意主机或 TLS 降级")
        }
        "jdbc:bigquery://bigquery.googleapis.com:443".into()
    };
    url.push_str(&format!(";ProjectId={};OAuthType=2;OAuthAccessTokenReadonly=0", parts[0]));
    if let Some(dataset) = parts.get(1) { url.push_str(&format!(";DefaultDataset={dataset}")); }
    let secs = req.timeout_secs.clamp(1,300);
    url.push_str(&format!(";LogLevel=0;EnableDiagnosticTelemetry=0;EnableHighThroughputAPI=0;EnableWriteAPI=0;EnableProjectDiscovery=0;enableGcpTraceExporter=false;enableGcpLogExporter=false;useGlobalOpenTelemetry=false;HttpConnectTimeout={};HttpReadTimeout={};Timeout={secs};RetryInitialDelay=1;RetryMaxDelay=1;UseStatelessQueryMode=1", secs*1000, secs*1000));
    Ok(url)
}
fn remaining_url(req: &ConnectionRequest, kind: &str, pairs: &[(String, String)]) -> Result<String> {
    if matches!(tls_mode(pairs)?, Some(TlsMode::Require | TlsMode::VerifyCa | TlsMode::VerifyFull)) {
        bail!("此托管 JDBC 尚未核验 TLS 参数；不会降级明文，需要 TLS 请使用自定义 JDBC")
    }
    if pairs.iter().any(|(key, _)| !["ssl", "tls", "sslmode", "ssl-mode", "ssl_verify"].contains(&key.as_str())) {
        bail!("Dremio/Ignite 的扩展参数需要显式自定义 JDBC，不能静默忽略")
    }
    if kind == "ignite" {
        let server = server_authority(req, 10800)?;
        let schema = if req.database.trim().is_empty() { "PUBLIC" } else { plain_name(req.database.trim(), "Ignite schema")? };
        Ok(format!("jdbc:ignite:thin://{server}/{schema}"))
    } else {
        if !req.database.trim().is_empty() { bail!("Dremio legacy 默认连接的数据库字段请留空；schema/扩展属性请使用自定义 JDBC") }
        Ok(format!("jdbc:dremio:direct={}", server_authority(req, 31010)?))
    }
}
fn snowflake_url(req: &ConnectionRequest, pairs: &[(String, String)]) -> Result<String> {
    let host = validated_host(req)?;
    let port = if req.port == 0 { 443 } else { req.port };
    if matches!(tls_mode(pairs)?, Some(TlsMode::Plain)) {
        bail!("Snowflake 强制 TLS，不接受 ssl=false/sslmode=disable")
    }
    let database = req.database.trim();
    let parts: Vec<_> = database.split('/').collect();
    if parts.len() > 2 {
        bail!("Snowflake 数据库格式为 db 或 db/schema")
    }
    let db = parts[0].trim();
    let schema = parts.get(1).copied().unwrap_or("").trim();
    let db = (!db.is_empty()).then(|| plain_name(db, "Snowflake db")).transpose()?;
    let schema = (!schema.is_empty()).then(|| plain_name(schema, "Snowflake schema")).transpose()?;
    let mut url = format!("jdbc:snowflake://{host}");
    if port != 443 {
        url.push_str(&format!(":{port}"));
    }
    url.push_str("/?");
    if let Some(db) = db {
        url.push_str(&format!("db={}&", percent(db)));
    }
    if let Some(schema) = schema {
        url.push_str(&format!("schema={}&", percent(schema)));
    }
    Ok(url)
}
/// Return None for explicit custom/raw ODBC settings and genuinely usable installed ODBC.
/// Once the product package exists, corruption/Java failures are surfaced; no silent fallback.
pub fn resolve_default(req: &ConnectionRequest) -> Result<Option<DriverResolution>> {
    let Some(v) = vendor(req.kind.as_str()) else {
        return Ok(None);
    };
    // Oracle：已核验的托管 ODBC 客户端继续走既有 ODBC 通道；损坏/缺失目录才考虑 thin JDBC。
    if v.kind == "oracle" && super::managed_library("oracle").is_ok_and(|l| l.is_some()) {
        return Ok(None);
    }
    let Some(pairs) = params(&v, req)? else {
        return Ok(None);
    };
    let inventory = crate::driver_manager::list_odbc_inventory().ok();
    if installed_odbc_ready(v.kind, inventory.as_ref()) {
        return Ok(None);
    }
    let dir = resolve_package_dir(&v);
    if !dir.exists() {
        return Ok(None);
    }
    let jar = verify_package(&dir, &v)?;
    let url = connection_url(req, &v, &pairs)?;
    runtime(&jar, &v)?;
    Ok(Some(DriverResolution {
        target: jar.to_string_lossy().into_owned(),
        url,
        driver_class: v.class.into(),
    }))
}

/// 连接明确选择了托管驱动版本时的解析：版本必须已核验且已下载，缺一都如实报错，
/// 不做静默回退（否则用户以为在用 A 版本实际跑的是 B 版本）。
pub fn resolve_versioned(req: &ConnectionRequest, version: &str) -> Result<Option<DriverResolution>> {
    let Some(v) = vendor_version(req.kind.as_str(), version) else {
        bail!("数据源 {} 没有已核验的托管驱动版本 {}；请在驱动管理中查看可用版本", req.kind.as_str(), version);
    };
    if v.kind == "oracle" && super::managed_library("oracle").is_ok_and(|l| l.is_some()) {
        bail!("Oracle 已有可用的托管 ODBC 客户端；如需 thin JDBC 版本请先移除托管 ODBC 客户端或改用自定义 JDBC");
    }
    let Some(pairs) = params(&v, req)? else {
        bail!("托管驱动版本选择不能与自定义驱动配置同时使用");
    };
    let dir = resolve_package_dir(&v);
    if !dir.exists() {
        bail!("托管驱动版本 {} 未下载，请先在驱动管理中下载后再连接", version);
    }
    let jar = verify_package(&dir, &v)?;
    let url = connection_url(req, &v, &pairs)?;
    runtime(&jar, &v)?;
    Ok(Some(DriverResolution {
        target: jar.to_string_lossy().into_owned(),
        url,
        driver_class: v.class.into(),
    }))
}

#[cfg(test)]
#[path = "managed_jdbc_package_tests.rs"]
mod package_tests;
#[cfg(test)]
#[path = "managed_jdbc_remaining_tests.rs"]
mod remaining_tests;
#[cfg(test)]
#[path = "managed_jdbc_access_yashan_tests.rs"]
mod access_yashan_tests;
#[cfg(test)]
#[path = "managed_jdbc_xugu_iris_tests.rs"]
mod xugu_iris_tests;
#[cfg(test)]
#[path = "managed_jdbc_bigquery_tests.rs"]
mod bigquery_tests;
#[cfg(test)]
#[path = "managed_gbase_inceptor_tests.rs"]
mod gbase_inceptor_tests;
#[cfg(test)]
#[path = "managed_spanner_tests.rs"]
mod spanner_tests;
#[cfg(test)]
#[path = "managed_dbx_tests.rs"]
mod dbx_tests;
#[cfg(test)]
#[path = "managed_argodb_tests.rs"]
mod argodb_tests;

#[cfg(test)]
mod tests {
    use super::*;
    fn request(kind: &str) -> ConnectionRequest {
        ConnectionRequest {
            kind: crate::models::DbKind::of(kind),
            host: "db.example".into(),
            port: 8080,
            user: "u@x;".into(),
            password: "p@:/#;secret".into(),
            database: "iceberg/default".into(),
            file: String::new(),
            params: String::new(),
            timeout_secs: 5,
        }
    }
    #[test]
    fn every_registered_text_asset_fits_the_real_download_contract() {
        let bigquery = vendor("bigquery").unwrap();
        let pom = bigquery.pom.as_ref().unwrap();
        assert_eq!(pom.size, 97_953);
        assert!(pom.size > 64 * 1024);
        assert!(text_asset_url(pom).is_ok());
        let mut checked = 0;
        for meta in crate::catalog::CATALOG {
            for release in versions(meta.id) {
                for asset in release.pom.iter().chain(release.license.iter())
                    .chain(dependencies::specs(release.kind).iter().map(|dep| &dep.pom)) {
                    text_asset_url(asset).unwrap_or_else(|error| panic!("{} {}: {error:#}", meta.id, release.version));
                    checked += 1;
                }
            }
        }
        assert!(checked >= 24, "The actual registry and runtime dependency POMs must be checked");
        for size in [0, MAX_TEXT_ASSET_BYTES + 1] {
            assert!(text_asset_url(&spec(pom.url, size, pom.sha256)).is_err());
        }
        assert!(text_asset_url(&spec("https://untrusted.example/driver.pom", 10, pom.sha256)).is_err());
        assert!(text_asset_url(&spec("http://repo.maven.apache.org/driver.pom", 10, pom.sha256)).is_err());
    }
    #[test]
    fn only_exact_registered_original_products() {
        for k in ["trino", "prestosql", "databricks", "db2", "informix", "saphana", "teradata",
                  "vertica", "exasol", "dameng", "firebird", "oracle", "hive", "kyuubi", "impala",
                  "spark", "kylin", "snowflake", "dremio", "ignite", "access", "yashandb", "xugu", "iris", "bigquery", "gbase", "inceptor", "spanner", "cache", "sundb", "oscar", "argodb"] {
            assert!(supports(k), "{k} 应有托管 JDBC");
        }
        for k in ["saphana2", "presto", "kafka", "cassandra", "duckdb",
                  "hbase", "salesforce"] {
            assert!(!supports(k), "{k} 不应有托管 JDBC");
        }
    }
    #[test]
    fn hive_family_aliases_share_one_pinned_package() {
        for alias in ["kyuubi", "impala", "spark"] {
            let v = vendor(alias).unwrap();
            assert_eq!(v.kind, "hive");
            assert!(v.kinds.contains(&alias));
            assert_eq!(package_dir(alias), package_dir("hive"));
        }
    }
    #[test]
    fn urls_do_not_contain_credentials_and_keep_product_identity() {
        for (k, p) in [("trino", "trino"), ("prestosql", "presto")] {
            let r = request(k);
            let u = connection_url(&r, &vendor(k).unwrap(), &[]).unwrap();
            assert_eq!(
                u,
                format!("jdbc:{p}://db.example:8080/iceberg/default?SSL=false")
            );
            assert!(!u.contains(&r.user) && !u.contains(&r.password));
        }
    }
    #[test]
    fn per_product_url_contracts_match_verified_driver_behavior() {
        // 每种拼写都与真实驱动的 acceptsURL/死端口连接验证一致（见交付说明）。
        let mut r = request("db2");
        r.database = "navkit".into();
        assert_eq!(db2_url(&r, &[]).unwrap(), "jdbc:db2://db.example:8080/navkit");
        r.params = "ssl=true".into();
        assert_eq!(db2_url(&r, &params(&vendor("db2").unwrap(), &r).unwrap().unwrap()).unwrap(), "jdbc:db2://db.example:8080/navkit:sslConnection=true;");

        let mut r = request("informix");
        r.database = "stores".into();
        let v = vendor("informix").unwrap();
        assert!(informix_url(&r, &params(&v, &r).unwrap().unwrap()).is_err(), "缺少 informixserver 必须失败");
        r.params = "informixserver=ol_navkit".into();
        assert_eq!(informix_url(&r, &params(&v, &r).unwrap().unwrap()).unwrap(), "jdbc:informix-sqli://db.example:8080/stores:INFORMIXSERVER=ol_navkit");
        r.params = "informixserver=ol_navkit;ssl=true".into();
        assert!(informix_url(&r, &params(&v, &r).unwrap().unwrap()).is_err(), "Informix 不提供经核验的 TLS 参数");

        let mut r = request("saphana");
        r.database = "HXE".into();
        let v = vendor("saphana").unwrap();
        assert_eq!(saphana_url(&r, &params(&v, &r).unwrap().unwrap()).unwrap(), "jdbc:sap://db.example:8080/?databaseName=HXE&encrypt=false");
        r.params = "ssl=true".into();
        assert_eq!(saphana_url(&r, &params(&v, &r).unwrap().unwrap()).unwrap(), "jdbc:sap://db.example:8080/?databaseName=HXE&encrypt=true&validateCertificate=true");

        let mut r = request("teradata");
        r.port = 1025;
        r.database = "navkit".into();
        let v = vendor("teradata").unwrap();
        assert_eq!(teradata_url(&r, &params(&v, &r).unwrap().unwrap()).unwrap(), "jdbc:teradata://db.example/DBS_PORT=1025,DATABASE=navkit");
        r.params = "sslmode=verify-full".into();
        assert_eq!(teradata_url(&r, &params(&v, &r).unwrap().unwrap()).unwrap(), "jdbc:teradata://db.example/SSLMode=VERIFY-FULL,https_port=1025,DATABASE=navkit");
        r.params = "ssl=false".into();
        assert_eq!(teradata_url(&r, &params(&v, &r).unwrap().unwrap()).unwrap(), "jdbc:teradata://db.example/DBS_PORT=1025,SSLMode=DISABLE,DATABASE=navkit");

        let mut r = request("vertica");
        r.port = 5433;
        r.database = "navkit".into();
        let v = vendor("vertica").unwrap();
        assert_eq!(vertica_url(&r, &[]).unwrap(), "jdbc:vertica://db.example:5433/navkit");
        r.params = "sslmode=verify-ca".into();
        assert_eq!(vertica_url(&r, &params(&v, &r).unwrap().unwrap()).unwrap(), "jdbc:vertica://db.example:5433/navkit?tlsmode=verify-ca");

        let mut r = request("exasol");
        let v = vendor("exasol").unwrap();
        r.port = 8563;
        r.database.clear();
        assert_eq!(exasol_url(&r, &[]).unwrap(), "jdbc:exa:db.example:8563");
        r.params = "ssl=true".into();
        assert_eq!(exasol_url(&r, &params(&v, &r).unwrap().unwrap()).unwrap(), "jdbc:exa:db.example:8563;encryption=true");
        r.database = "nope".into();
        assert!(exasol_url(&r, &[]).is_err(), "Exasol URL 不含数据库名");

        let mut r = request("dameng");
        r.port = 5236;
        r.database = "".into();
        r.params = "schema=SYSDBA".into();
        let v = vendor("dameng").unwrap();
        assert_eq!(dameng_url(&r, &params(&v, &r).unwrap().unwrap()).unwrap(), "jdbc:dm://db.example:5236?schema=SYSDBA");
        r.params = "".into();
        r.database = "SYSDBA".into();
        assert_eq!(dameng_url(&r, &[]).unwrap(), "jdbc:dm://db.example:5236?schema=SYSDBA");

        let mut r = request("firebird");
        r.port = 3050;
        r.database = "/data/navkit.fdb".into();
        let v = vendor("firebird").unwrap();
        assert_eq!(firebird_url(&r, &[]).unwrap(), "jdbc:firebirdsql://db.example:3050//data/navkit.fdb");
        r.params = "ssl=true".into();
        assert_eq!(firebird_url(&r, &params(&v, &r).unwrap().unwrap()).unwrap(), "jdbc:firebirdsql://db.example:3050//data/navkit.fdb?wireCrypt=REQUIRED");

        let mut r = request("oracle");
        r.port = 1521;
        r.database = "ORCLPDB".into();
        r.params = "".into();
        let v = vendor("oracle").unwrap();
        assert_eq!(oracle_thin_url(&r, &[]).unwrap(), "jdbc:oracle:thin:@//db.example:1521/ORCLPDB");
        r.params = r#"{"oracle_connect_as":"sid","sid":"ORCL"}"#.into();
        assert_eq!(oracle_thin_url(&r, &params(&v, &r).unwrap().unwrap()).unwrap(), "jdbc:oracle:thin:@db.example:1521:ORCL");
        r.params = "ssl=true".into();
        assert!(oracle_thin_url(&r, &params(&v, &r).unwrap().unwrap()).is_err(), "TCPS 需要 wallet，不允许静默明文或假 TLS");

        let mut r = request("hive");
        r.port = 10000;
        r.database = "navkit".into();
        let v = vendor("hive").unwrap();
        assert_eq!(hive_url(&r, &[]).unwrap(), "jdbc:hive2://db.example:10000/navkit");
        r.params = "ssl=true".into();
        assert_eq!(hive_url(&r, &params(&v, &r).unwrap().unwrap()).unwrap(), "jdbc:hive2://db.example:10000/navkit;ssl=true");
        r.params = "sslmode=verify-full".into();
        assert!(hive_url(&r, &params(&v, &r).unwrap().unwrap()).is_err());

        let mut r = request("kylin");
        r.port = 7070;
        r.database = "navkit_project".into();
        assert_eq!(kylin_url(&r, &[]).unwrap(), "jdbc:kylin://db.example:7070/navkit_project");

        let mut r = request("snowflake");
        r.port = 0;
        r.host = "org-account.snowflakecomputing.com".into();
        r.database = "navkit/public".into();
        r.user = "u".into();
        assert_eq!(snowflake_url(&r, &[]).unwrap(), "jdbc:snowflake://org-account.snowflakecomputing.com/?db=navkit&schema=public&");
        r.params = "ssl=false".into();
        assert!(snowflake_url(&r, &params(&vendor("snowflake").unwrap(), &r).unwrap().unwrap()).is_err(), "Snowflake 强制 TLS");
    }
    #[test]
    fn tls_never_downgrades_or_silently_ignores_invalid_json() {
        let mut r = request("trino");
        for text in ["ssl=true", r#"{"tls":true}"#, "sslmode=verify-full"] {
            r.params = text.into();
            assert!(
                connection_url(&r, &vendor("trino").unwrap(), &params(&vendor("trino").unwrap(), &r).unwrap().unwrap())
                    .unwrap()
                    .ends_with("SSL=true")
            );
        }
        for text in [
            "ssl=true;tls=false",
            "sslmode=prefer",
            "ssl_verify=false",
            "tls=maybe",
        ] {
            r.params = text.into();
            assert!(
                connection_url(&r, &vendor("trino").unwrap(), &params(&vendor("trino").unwrap(), &r).unwrap().unwrap())
                    .is_err()
            );
        }
        for text in ["[]", "{bad"] {
            r.params = text.into();
            assert!(params(&vendor("trino").unwrap(), &r).is_err());
        }
    }
    #[test]
    fn explicit_odbc_and_custom_bindings_are_not_overridden() {
        let mut r = request("trino");
        for text in [
            "Driver={ODBC};SSL=true",
            "DSN=existing",
            "@navkit-driver:{\"version\":1}",
            "AuthenticationType=token",
        ] {
            r.params = text.into();
            assert!(params(&vendor("trino").unwrap(), &r).unwrap().is_none());
        }
    }
    #[test]
    fn product_specific_params_do_not_leak_across_products() {
        let mut r = request("databricks");
        r.params = "httpPath=/sql/1.0/warehouses/abc_123".into();
        assert!(params(&vendor("databricks").unwrap(), &r).unwrap().is_some());
        assert!(params(&vendor("trino").unwrap(), &r).unwrap().is_none(), "Databricks options must not leak into Trino");
        let mut r = request("informix");
        r.params = "informixserver=ol_a".into();
        assert!(params(&vendor("informix").unwrap(), &r).unwrap().is_some());
        assert!(params(&vendor("db2").unwrap(), &r).unwrap().is_none());
    }
    #[test]
    fn url_injection_rejected_and_catalog_encoded() {
        let mut r = request("trino");
        for h in [
            "user:password@db",
            "https://db",
            "db?SSL=false",
            "db;Driver=x",
        ] {
            r.host = h.into();
            assert!(connection_url(&r, &vendor("trino").unwrap(), &[]).is_err());
        }
        r.host = "::1".into();
        r.database = "iceberg/中文?SSL=false".into();
        let u = connection_url(
            &r,
            &vendor("trino").unwrap(),
            &[("ssl".into(), "true".into())],
        )
        .unwrap();
        assert!(u.contains("[::1]"));
        assert!(u.contains("%3FSSL%3Dfalse?SSL=true"));
        // 其它产品同样拒绝注入。
        for kind in ["db2", "hive", "teradata", "snowflake"] {
            let mut r = request(kind);
            r.host = "db;Driver=x".into();
            let v = vendor(kind).unwrap();
            let pairs = params(&v, &r).unwrap().unwrap_or_default();
            assert!(connection_url(&r, &v, &pairs).is_err(), "{kind}");
        }
    }
    #[test]
    fn databricks_pat_http_path_and_tls_are_product_specific() {
        let mut r = request("databricks");
        r.port = 0;
        r.user = "token".into();
        r.params = "httpPath=/sql/1.0/warehouses/abc_123;AuthMech=3;ssl=true".into();
        let v = vendor("databricks").unwrap();
        let url = connection_url(&r, &v, &params(&v, &r).unwrap().unwrap()).unwrap();
        assert_eq!(url, "jdbc:databricks://db.example:443/default;httpPath=/sql/1.0/warehouses/abc_123;AuthMech=3;SSL=1;ConnCatalog=iceberg");
        assert!(!url.contains(&r.password));
        for bad in ["", "httpPath=/sql/ok;ssl=false", "httpPath=/sql/ok;ssl_verify=false",
                    "httpPath=/sql/ok;sslmode=prefer", "httpPath=/sql/ok;AuthMech=11",
                    "httpPath=/sql/ok;http_path=/sql/other",
                    "httpPath=/sql/ok?SSL=0", "httpPath=https://example/sql",
                    "httpPath=/sql/../x", "httpPath=/sql//x"] {
            r.params = bad.into();
            assert!(connection_url(&r, &v, &params(&v, &r).unwrap().unwrap()).is_err(), "{bad}");
        }
        r.params = r#"{"httpPath":"/sql/x;SSL=0"}"#.into();
        assert!(connection_url(&r, &v, &params(&v, &r).unwrap().unwrap()).is_err());
        r.params = "httpPath=/sql/ok;PWD=secret".into();
        assert!(params(&v, &r).unwrap().is_none(), "Never absorb credentials from arbitrary URL options");
        r.params = "httpPath=/sql/ok".into();
        r.user = "not-token".into();
        assert!(connection_url(&r, &v, &params(&v, &r).unwrap().unwrap()).is_err());
        r.user = "token".into(); r.password.clear();
        assert!(connection_url(&r, &v, &params(&v, &r).unwrap().unwrap()).is_err());
        r.password = "secret".into(); r.database = "cat/schema;SSL=0".into();
        assert!(connection_url(&r, &v, &params(&v, &r).unwrap().unwrap()).is_err());
        r.database.clear(); r.host = "::1".into();
        assert!(connection_url(&r, &v, &params(&v, &r).unwrap().unwrap()).unwrap().starts_with("jdbc:databricks://[::1]:443/default;"));
    }
    #[test]
    fn fake_and_html_jars_are_not_ready() {
        let dir = std::env::temp_dir().join(format!(
            "navkit-managed-jdbc-negative-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&dir).unwrap();
        let guard = Staging(dir);
        fs::write(guard.0.join("driver.jar"), b"PK<html>fake</html>").unwrap();
        assert!(verify_jar(&guard.0.join("driver.jar"), &vendor("trino").unwrap()).is_err());
    }
    #[tokio::test]
    #[ignore = "real official packages + Java source launcher; no server account implies UNVERIFIED"]
    async fn real_official_managed_jdbc_packages() {
        let root = std::env::var_os("NAVKIT_JDBC_EVIDENCE_ROOT")
            .expect("set evidence root to downloaded official packages");
        for (kind, folder, name, pom) in [
            ("trino", "trino", "trino-jdbc-435.jar", "official.pom"),
            ("prestosql", "prestosql", "presto-jdbc-350.jar", "official.pom"),
            ("databricks", "databricks-new", "databricks-jdbc-3.4.4.jar", "databricks-jdbc-3.4.4.pom"),
            ("db2", "db2", "jcc-12.1.5.0.jar", "jcc-12.1.5.0.pom"),
            ("informix", "informix", "jdbc-15.0.1.4.jar", "jdbc-15.0.1.4.pom"),
            ("saphana", "saphana", "ngdbc-2.30.11.jar", "ngdbc-2.30.11.pom"),
            ("teradata", "teradata", "terajdbc-20.00.00.62.jar", "terajdbc-20.00.00.62.pom"),
            ("vertica", "vertica", "vertica-jdbc-25.3.0-0.jar", "vertica-jdbc-25.3.0-0.pom"),
            ("exasol", "exasol", "exasol-jdbc-26.2.10.jar", "exasol-jdbc-26.2.10.pom"),
            ("dameng", "dameng", "DmJdbcDriver18-8.1.3.140.jar", "DmJdbcDriver18-8.1.3.140.pom"),
            ("firebird", "firebird", "jaybird-5.0.13.java11.jar", "jaybird-5.0.13.java11.pom"),
            ("oracle", "oracle-thin", "ojdbc11-23.26.4.0.0.jar", "ojdbc11-23.26.4.0.0.pom"),
            ("hive", "hive", "hive-jdbc-3.1.3-standalone.jar", "hive-jdbc-3.1.3.pom"),
            ("kylin", "kylin", "kylin-jdbc-5.0.5.jar", "kylin-jdbc-5.0.5.pom"),
            ("snowflake", "snowflake", "snowflake-jdbc-4.4.1.jar", "snowflake-jdbc-4.4.1.pom"),
        ] {
            let v = vendor(kind).unwrap();
            let dir = PathBuf::from(&root).join(folder);
            verify_jar(&dir.join(name), &v).unwrap();
            check_asset(&dir.join(pom), v.pom.as_ref().unwrap()).unwrap();
            if let Some(license) = &v.license {
                check_asset(&dir.join("LICENSE"), license).unwrap();
            }
            let probe = runtime(&dir.join(name), &v).unwrap();
            println!("{kind}: {probe}; SERVER UNVERIFIED");
        }
    }
    #[tokio::test]
    #[ignore = "copy test executable into navkit-jdbc-command-tests-* temp dir, set official evidence root and H2 jar"]
    async fn real_managed_default_routes_and_generic_h2_connection() {
        use crate::driver_manager::{
            DriverConnectionParams, DriverDefinition, DriverProtocol, DriverSource,
        };
        let portable = crate::storage::portable_root();
        assert!(
            portable.starts_with(std::env::temp_dir())
                && portable
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("navkit-jdbc-command-tests-"),
            "must run only a copied, isolated test executable"
        );
        let root = PathBuf::from(
            std::env::var_os("NAVKIT_JDBC_EVIDENCE_ROOT").expect("official evidence root"),
        );
        for (kind, folder, name, pom) in [
            ("trino", "trino", "trino-jdbc-435.jar", "official.pom"),
            ("prestosql", "prestosql", "presto-jdbc-350.jar", "official.pom"),
            ("databricks", "databricks-new", "databricks-jdbc-3.4.4.jar", "databricks-jdbc-3.4.4.pom"),
        ] {
            let v = vendor(kind).unwrap();
            let dir = package_dir(kind);
            fs::create_dir_all(&dir).unwrap();
            fs::copy(root.join(folder).join(name), dir.join("driver.jar")).unwrap();
            fs::copy(
                root.join(folder).join(pom),
                dir.join("official.pom"),
            )
            .unwrap();
            if v.license.is_some() {
                fs::copy(root.join(kind).join("LICENSE"), dir.join("LICENSE")).unwrap();
            }
            fs::write(
                dir.join("manifest.json"),
                serde_json::to_vec(&Manifest {
                    format: 1,
                    kind: kind.into(),
                    version: v.version.into(),
                    class: v.class.into(),
                })
                .unwrap(),
            )
            .unwrap();
            assert_eq!(status_at(&v, &dir, kind).status, "ready");
            let mut req = request(kind);
            req.host = "127.0.0.1".into();
            req.port = 9;
            req.params = "ssl=true".into();
            if kind == "databricks" {
                req.params = "ssl=true;httpPath=/sql/1.0/warehouses/navkit-probe".into();
                req.user = "token".into();
            }
            req.timeout_secs = 5;
            let resolved = crate::driver_manager::resolve_jdbc_driver(&req)
                .unwrap()
                .expect("standard product selects verified managed JDBC without a custom id");
            assert_eq!(resolved.driver_class, v.class);
            assert!(if kind == "databricks" { resolved.url.contains(";SSL=1") } else { resolved.url.ends_with("SSL=true") });
            let e = match crate::db::open(&req).await {
                Ok(_) => panic!("closed port must not connect"),
                Err(e) => format!("{e:#}"),
            };
            assert!(!e.contains(&req.password));
            assert!(
                e.contains("JDBC") || e.contains("sidecar"),
                "expected real JDBC negative, not ODBC: {e}"
            );
            fs::write(dir.join("untrusted-extra.jar"), b"fake dependency").unwrap();
            assert!(verify_package(&dir, &v).is_err());
        }
        let jar = PathBuf::from(std::env::var_os("NAVKIT_H2_JAR").expect("real official H2 jar"));
        crate::jdbc_db::driver_runtime_status(&jar, "org.h2.Driver", "jdbc:h2:mem:probe").unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let url = format!("jdbc:h2:mem:navkit_jdbc_{}", id.replace('-', ""));
        let driver = DriverDefinition {
            id: id.clone(),
            name: format!("Isolated JDBC {id}"),
            protocol: DriverProtocol::Jdbc,
            source: DriverSource::File,
            target: jar.to_string_lossy().into_owned(),
            template: url,
            driver_class: "org.h2.Driver".into(),
            test_sql: "SELECT 1".into(),
        };
        crate::driver_manager::save_custom_driver_for_test(driver).unwrap();
        let mut req = request("odbc");
        req.user = "sa".into();
        req.password = String::new();
        req.params = format!(
            "{}{}",
            PARAMS_PREFIX,
            serde_json::to_string(&DriverConnectionParams {
                version: 1,
                driver_id: id,
                template: String::new()
            })
            .unwrap()
        );
        req.timeout_secs = 10;
        let db = crate::db::open(&req).await.unwrap();
        let crate::db::Db::Http(client) = &db else {
            panic!("explicit JDBC route must not use ODBC")
        };
        client.run("","","CREATE TABLE PUBLIC.NAVKIT_JDBC_PROBE(id INT PRIMARY KEY, payload VARCHAR(100), nil VARCHAR(10))").await.unwrap();
        client
            .run(
                "",
                "",
                "INSERT INTO PUBLIC.NAVKIT_JDBC_PROBE VALUES(1,'中文;O''Reilly\\尾',NULL)",
            )
            .await
            .unwrap();
        let rows = client
            .run("", "", "SELECT payload,nil FROM PUBLIC.NAVKIT_JDBC_PROBE")
            .await
            .unwrap();
        assert_eq!(rows.rows[0][0].as_deref(), Some("中文;O'Reilly\\尾"));
        assert_eq!(rows.rows[0][1], None);
        assert!(client
            .containers()
            .await
            .unwrap()
            .iter()
            .any(|(n, _)| n == "PUBLIC"));
        assert!(client
            .objects("PUBLIC")
            .await
            .unwrap()
            .iter()
            .any(|n| n == "NAVKIT_JDBC_PROBE"));
        let preview = client
            .rows("PUBLIC", "NAVKIT_JDBC_PROBE", 0, 2)
            .await
            .unwrap();
        assert_eq!(preview.rows.len(), 1);
        assert!(client
            .table_columns("PUBLIC", "NAVKIT_JDBC_PROBE")
            .await
            .unwrap()
            .unwrap()
            .iter()
            .any(|c| c.name == "ID" && c.primary_key));
        assert!(client
            .run("", "", "SELECT * FROM PUBLIC.NONEXISTENT_NAVKIT_JDBC")
            .await
            .is_err());
        let overflow = client
            .run("", "", "SELECT X FROM SYSTEM_RANGE(1,12001)")
            .await
            .err()
            .expect("oversize query must fail rather than silently return 10000 rows");
        assert!(format!("{overflow:#}").contains("Query exceeds 10000 rows"));
        client
            .run("", "", "DROP TABLE PUBLIC.NAVKIT_JDBC_PROBE")
            .await
            .unwrap();
        println!("H2 generic JDBC/custom db::open/schema/preview/unicode/null/negative PASS; managed product servers UNVERIFIED");
    }
}
