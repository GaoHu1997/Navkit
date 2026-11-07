//! 数据源类型目录：全部支持的数据源类型与其连接协议映射。
//!
//! 参考 dbx 的分层支持策略（https://github.com/t8y2/dbx）：
//! - 协议兼容：MySQL / PostgreSQL 协议族数据库直接复用对应原生驱动；
//! - 原生客户端：SQL Server、Redis、MongoDB、HTTP API 类（检索 / 向量 / 时序 / MQ）；
//! - ODBC 通用：提供 Windows ODBC 驱动的数据源（Oracle、DB2、达梦、Hive 等）。
//!
//! 新增一种数据源只需在 `CATALOG` 表中加一行；前端通过 `db_kinds` 命令读取。

use crate::models::Protocol;

/// 单个数据源类型的元信息。
pub struct KindMeta {
    /// 目录 id（连接配置中的 kind 字符串，小写）。
    pub id: &'static str,
    /// 显示名。
    pub label: &'static str,
    /// 默认端口。
    pub port: u16,
    /// 连接协议族。
    pub protocol: Protocol,
    /// 分类 id：relational / analytics / domestic / lightweight / nosql / graph /
    /// timeseries / mq / registry。
    pub category: &'static str,
    /// HTTP 适配器 id（仅 Protocol::Http 使用，其余为空）。
    pub http: &'static str,
    /// ODBC 驱动名提示（仅 Protocol::Odbc 使用，空表示需在参数中提供 DSN / 连接串）。
    pub driver: &'static str,
    /// 连接说明（驱动要求 / 额外参数提示）。
    pub note: &'static str,
    /// true 表示适配器尚未提供，仅可浏览目录，连接时会给出明确提示。
    pub planned: bool,
}

const fn k(
    id: &'static str,
    label: &'static str,
    port: u16,
    protocol: Protocol,
    category: &'static str,
) -> KindMeta {
    KindMeta {
        id,
        label,
        port,
        protocol,
        category,
        http: "",
        driver: "",
        note: "",
        planned: false,
    }
}

const fn odbc(
    id: &'static str,
    label: &'static str,
    port: u16,
    category: &'static str,
    driver: &'static str,
    note: &'static str,
) -> KindMeta {
    KindMeta {
        id,
        label,
        port,
        protocol: Protocol::Odbc,
        category,
        http: "",
        driver,
        note,
        planned: false,
    }
}

const fn http(
    id: &'static str,
    label: &'static str,
    port: u16,
    category: &'static str,
    adapter: &'static str,
    note: &'static str,
) -> KindMeta {
    KindMeta {
        id,
        label,
        port,
        protocol: Protocol::Http,
        category,
        http: adapter,
        driver: "",
        note,
        planned: false,
    }
}

const fn native(
    id: &'static str,
    label: &'static str,
    port: u16,
    category: &'static str,
    note: &'static str,
) -> KindMeta {
    KindMeta {
        id,
        label,
        port,
        protocol: Protocol::Native,
        category,
        http: "",
        driver: "",
        note,
        planned: false,
    }
}

/// 全部数据源类型。顺序即各分类内的展示顺序。
pub static CATALOG: &[KindMeta] = &[
    // ------------------------------------------------ 关系型数据库
    k("mysql", "MySQL", 3306, Protocol::Mysql, "relational"),
    k("postgresql", "PostgreSQL", 5432, Protocol::Postgres, "relational"),
    k("sqlserver", "SQL Server", 1433, Protocol::Mssql, "relational"),
    odbc(
        "oracle",
        "Oracle",
        1521,
        "relational",
        "Oracle ODBC Driver",
        "可直接下载官方 thin JDBC（ojdbc11 8.8MB，需 Java 11+）；也可安装 Oracle Instant Client ODBC（或下载完整客户端），驱动名不一致时在参数中填写完整 ODBC 连接串。",
    ),
    k("mariadb", "MariaDB", 3306, Protocol::Mysql, "relational"),
    odbc(
        "firebird",
        "Firebird",
        3050,
        "relational",
        "Firebird/InterBase(r) driver",
        "可直接下载官方 Jaybird JDBC（需 Java 11+，数据库名填服务器端路径/别名）；也可安装 Firebird ODBC 驱动。",
    ),
    k("cockroachdb", "CockroachDB", 26257, Protocol::Postgres, "relational"),
    odbc(
        "db2",
        "DB2",
        50000,
        "relational",
        "IBM DB2 ODBC DRIVER",
        "可直接下载官方 IBM JDBC（jcc，需 Java 11+）；也可安装 IBM Data Server Client（含 DB2 ODBC 驱动）。",
    ),
    odbc(
        "informix",
        "Informix",
        9088,
        "relational",
        "IBM INFORMIX ODBC DRIVER",
        "可直接下载官方 Informix JDBC（需 Java 11+，参数需 informixserver）；也可安装 IBM Informix Client（含 ODBC 驱动）。",
    ),
    odbc(
        "jdbcx",
        "JDBCX",
        0,
        "relational",
        "",
        "通用 JDBC 桥接数据源：通过 ODBC DSN / 连接串接入，请在参数中填写完整 ODBC 连接串。",
    ),
    odbc(
        "spanner",
        "Cloud Spanner",
        443,
        "relational",
        "",
        "可直接下载官方 Google Spanner 2.47.0 完整 JDBC 包（Java 11+）；默认 spanner.googleapis.com:443、数据库填 project/instance/database、用户名 token、密码填预生成 OAuth 令牌。无凭据本地模拟器须显式回环地址与 ssl=false；驱动就绪不代表 Google 云端 SQL/TLS 已验收。",
    ),
    odbc(
        "iris",
        "IRIS",
        1972,
        "relational",
        "InterSystems IRIS ODBC",
        "可直接下载官方 IRIS JDBC 3.11.0（原签名完整保留，需 Java 11+）；数据库名填 namespace，默认 USER；也保留已安装 IRIS ODBC。IRIS 驱动不代替 Caché。", 
    ),
    odbc(
        "cache",
        "InterSystems Caché",
        1972,
        "relational",
        "InterSystems ODBC",
        "可直接下载参考项目 dbx 固定提交的 Caché 原 JDBC 2010.2.3.702（第三方来源，非厂商官网；保留原包许可，需 Java 11+）。普通 TCP 路由已验，服务器 SQL/认证/TLS 未验；也保留已安装 Caché ODBC。",
    ),
    k("custom_mysql", "Custom (MySQL)", 3306, Protocol::Mysql, "relational"),
    k("dolt", "Dolt", 3307, Protocol::Mysql, "relational"),
    k("custom_pg", "Custom (PostgreSQL)", 5432, Protocol::Postgres, "relational"),
    odbc(
        "odbc",
        "自定义 ODBC",
        0,
        "relational",
        "",
        "通过系统 ODBC 数据源接入：数据库名填 DSN，或在参数中填写完整 ODBC 连接串。",
    ),
    // ------------------------------------------------ 数仓分析
    k("cloudberry", "Apache Cloudberry", 5432, Protocol::Postgres, "analytics"),
    k("clickhouse", "ClickHouse", 8123, Protocol::Clickhouse, "analytics"),
    k("databend", "Databend", 3307, Protocol::Mysql, "analytics"),
    odbc(
        "databricks",
        "Databricks SQL",
        443,
        "analytics",
        "Simba Spark ODBC Driver",
        "可下载官方 Databricks JDBC 3.4.4，需 Java 11+、httpPath、用户名 token 和 PAT 密码，严格 TLS；也保留已安装 Simba Spark / Databricks ODBC。数据库名可填 catalog/schema；OAuth 使用显式自定义 JDBC。",
    ),
    odbc(
        "saphana",
        "SAP HANA",
        30015,
        "analytics",
        "HDBODBC",
        "可直接下载官方 HANA JDBC（ngdbc，需 Java 11+）；也可安装 SAP HANA Client（HDBODBC 驱动）。",
    ),
    odbc(
        "teradata",
        "Teradata",
        1025,
        "analytics",
        "Teradata Database ODBC Driver",
        "可直接下载官方 Teradata JDBC（需 Java 11+；TLS 走 https_port）；也可安装 Teradata ODBC 驱动。",
    ),
    odbc(
        "vertica",
        "Vertica",
        5433,
        "analytics",
        "Vertica ODBC Driver",
        "可直接下载官方 Vertica JDBC（需 Java 11+）；也可安装 Vertica ODBC 驱动。",
    ),
    odbc(
        "exasol",
        "Exasol",
        8563,
        "analytics",
        "EXASolution",
        "可直接下载官方 Exasol JDBC（需 Java 11+）；也可安装 Exasol ODBC 驱动。",
    ),
    k("doris", "Doris", 9030, Protocol::Mysql, "analytics"),
    k("selectdb", "SelectDB", 9030, Protocol::Mysql, "analytics"),
    k("starrocks", "StarRocks", 9030, Protocol::Mysql, "analytics"),
    k("redshift", "Redshift", 5439, Protocol::Postgres, "analytics"),
    odbc(
        "snowflake",
        "Snowflake",
        443,
        "analytics",
        "SnowflakeDSIIDriver",
        "可直接下载官方 Snowflake JDBC 4.4.1（约 98MB，强制 TLS，需 Java 11+；host 填 org-account.snowflakecomputing.com）；也可安装 Snowflake ODBC 驱动。",
    ),
    odbc(
        "trino",
        "Trino",
        8080,
        "analytics",
        "Trino ODBC Driver",
        "支持已完整安装的 Trino ODBC；缺驱动可直接下载官方 JDBC 435（Java 11+），默认自动使用托管 JDBC。数据库填 catalog/schema；密码认证必须启用 TLS。服务器仍需实际验证。",
    ),
    odbc(
        "prestosql",
        "PrestoSQL",
        8080,
        "analytics",
        "Presto ODBC Driver",
        "支持已完整安装的 Presto ODBC；缺驱动可直接下载官方 PrestoSQL JDBC 350（Java 11+），默认自动使用托管 JDBC。并非 PrestoDB 通用驱动；数据库填 catalog/schema，密码认证必须 TLS。",
    ),
    odbc(
        "hive",
        "Hive",
        10000,
        "analytics",
        "Hive ODBC Driver",
        "可直接下载官方 Hive standalone JDBC（72MB，需 Java 11+）；也可安装 Cloudera/Apache Hive ODBC 驱动。",
    ),
    odbc(
        "kyuubi",
        "Apache Kyuubi",
        10009,
        "analytics",
        "Hive ODBC Driver",
        "Kyuubi 兼容 Hive Thrift 协议；可直接下载官方 Hive standalone JDBC（与 Hive 共用包，需 Java 11+），也可安装 Hive ODBC 驱动。",
    ),
    odbc(
        "impala",
        "Apache Impala",
        21050,
        "analytics",
        "Cloudera Impala ODBC Driver",
        "可直接下载官方 Hive standalone JDBC（Impala 兼容 Hive Thrift，与 Hive 共用包，需 Java 11+）；也可安装 Cloudera Impala ODBC 驱动。",
    ),
    odbc(
        "spark",
        "Apache Spark",
        10000,
        "analytics",
        "Simba Spark ODBC Driver",
        "Spark Thrift Server 兼容 Hive Thrift 协议；可直接下载官方 Hive standalone JDBC（与 Hive 共用包，需 Java 11+），也可安装 Simba Spark ODBC 驱动。",
    ),
    odbc(
        "dremio",
        "Dremio",
        31010,
        "analytics",
        "Dremio ODBC Driver",
        "可直接下载官方 Dremio legacy 完整 JDBC 包（需 Java 11 或 17，应用自动使用固定模块参数）；默认数据库字段留空。该包不是 Arrow Flight SQL；也保留 Dremio ODBC。", 
    ),
    odbc(
        "bigquery",
        "BigQuery",
        443,
        "analytics",
        "",
        "可直接下载官方 BigQuery 完整 JDBC all 包（1.9.0，需 Java 11+）。默认主机 bigquery.googleapis.com，数据库填 project[/dataset]，用户名 token，密码填 OAuth 访问令牌；令牌被拒绝或过期需替换后重连，不支持自动刷新。驱动就绪不代表云端账户或 TLS 已验收。",
    ),
    odbc(
        "kylin",
        "Kylin",
        7070,
        "analytics",
        "Kylin ODBC Driver",
        "可直接下载官方 Kylin JDBC（5.0.5，需 Java 11+，数据库名填 project）；也可安装 Apache Kylin ODBC 驱动。",
    ),
    odbc(
        "ignite",
        "Apache Ignite",
        10800,
        "analytics",
        "Apache Ignite",
        "可直接下载官方 Ignite 2.18.0 thin JDBC 及六个必需依赖（需 Java 11 或 17，应用使用对应官方模块参数）；数据库名填 schema，默认 PUBLIC。此驱动不用于 Ignite 3；也保留原 ODBC。", 
    ),
    native(
        "salesforce",
        "Salesforce",
        443,
        "analytics",
        "内置 Salesforce REST：主机填组织 instance 域名，密码填预生成 OAuth token，数据库填 API 版本（默认 v59.0）。支持对象目录/describe、SOQL 游标分页及 JSON create/update/delete；云端强制验证 HTTPS，不自动刷新 token。保留显式自定义 JDBC/ODBC。",
    ),
    native(
        "druid",
        "Apache Druid",
        8888,
        "analytics",
        "Druid真实SQL HTTP /druid/v2/sql；支持INFORMATION_SCHEMA目录、浏览和查询；url/ssl/ssl_verify参数。",
    ),
    // ------------------------------------------------ 国产数据库
    k("opententbase", "OpenTenBase", 5432, Protocol::Postgres, "domestic"),
    k("tidb", "TiDB", 4000, Protocol::Mysql, "domestic"),
    k("oceanbase", "OceanBase", 2881, Protocol::Mysql, "domestic"),
    k("goldendb", "金篆 GoldenDB", 3306, Protocol::Mysql, "domestic"),
    k("tdsql", "TDSQL", 3306, Protocol::Mysql, "domestic"),
    k("polardb", "PolarDB", 3306, Protocol::Mysql, "domestic"),
    k("greatsql", "GreatSQL", 3306, Protocol::Mysql, "domestic"),
    odbc(
        "gbase",
        "南大通用 GBase",
        5258,
        "domestic",
        "GBase 8a ODBC Driver",
        "可直接下载南大通用官网 GBase 8a JDBC 9.5.0.10-build1 原 ZIP/JAR/readme（Java 11+）；默认普通 TCP 用户名/密码连接，非 GBase 8s/8c。TLS/集群扩展请配置自定义 JDBC；也保留 GBase 8a ODBC。",
    ),
    k("opengauss", "openGauss", 5432, Protocol::Postgres, "domestic"),
    k("gaussdb", "GaussDB", 8000, Protocol::Postgres, "domestic"),
    k("kwdb", "KWDB", 26257, Protocol::Postgres, "domestic"),
    k("kingbase", "金仓 KingbaseES", 54321, Protocol::Postgres, "domestic"),
    k("highgo", "瀚高 HighGo", 5866, Protocol::Postgres, "domestic"),
    k("uxdb", "优炫 UXDB", 5432, Protocol::Postgres, "domestic"),
    odbc(
        "yashandb",
        "崖山 YashanDB",
        1688,
        "domestic",
        "YashanDB ODBC Driver",
        "可直接下载官方 YashanDB JDBC 1.10.7（需 Java 11+）；支持默认 JDBC 路由，也保留已安装崖山 ODBC。托管 TLS/TLCP 未核验，请用显式自定义 JDBC，不能静默降级。", 
    ),
    k("vastbase", "海量 Vastbase", 5432, Protocol::Postgres, "domestic"),
    odbc(
        "dameng",
        "达梦 Dameng",
        5236,
        "domestic",
        "DM8 ODBC DRIVER",
        "可直接下载官方达梦 JDBC（DmJdbcDriver18，需 Java 11+）；也可安装达梦自带的 DM8 ODBC DRIVER。",
    ),
    odbc(
        "inceptor",
        "星环 Inceptor",
        10000,
        "domestic",
        "Hive ODBC Driver",
        "可直接下载星环官方 Inceptor JDBC 8.37.3 原 JAR（Java 11+，官方公开导航来源）；普通 inceptor2/noSasl 路由已验，Kerberos/HTTP/TLS 等扩展请用自定义 JDBC。不是 Apache Hive standalone 包，也不以此冒充 ArgoDB 实库认证。",
    ),
    odbc(
        "argodb",
        "ArgoDB",
        10000,
        "domestic",
        "Hive ODBC Driver",
        "可直接下载星环官方 Quark JDBC 8.37.3 独立完整包（Java 11+）；ArgoDB 文档指定 Quark Gateway 为 SQL 入口。实际 io.transwarp.jdbc.QuarkDriver + jdbc:transwarp2，仅 auth=noSasl 且用户名/密码留空；LDAP/NONE/Kerberos/TLS 扩展请显式配置自定义驱动，厂商实库 SQL/认证未验。", 
    ),
    odbc(
        "sundb",
        "科蓝 SUNDB",
        40000,
        "domestic",
        "SUNDB ODBC Driver",
        "可直接下载参考项目 dbx 固定提交的 SUNDB 8-1.0.2 原 JDBC（第三方来源，非厂商官网，Java 11+）；普通 TCP 路由已验，服务器 SQL/认证/TLS 未验；也保留已安装科蓝 ODBC。",
    ),
    odbc(
        "oscar",
        "神通 OSCAR",
        2003,
        "domestic",
        "Shentong ODBC Driver",
        "可直接下载参考项目 dbx 固定提交的 OSCAR JDBC 原 JAR（第三方来源，非厂商官网，Java 11+）；保留原包许可/版本，普通 TCP 路由已验，服务器 SQL/认证/TLS 未验；也保留神通 ODBC。",
    ),
    odbc(
        "xugu",
        "虚谷 XuguDB",
        5138,
        "domestic",
        "Xugu ODBC Driver",
        "可直接下载官方虚谷 JDBC 12.3.9 与必需 checker-qual 依赖（需 Java 11+）；数据库名默认 SYSTEM，也保留已安装虚谷 ODBC。托管 TLS 参数尚未核验。", 
    ),
    // ------------------------------------------------ 轻量数据库
    k("sqlite", "SQLite", 0, Protocol::Sqlite, "lightweight"),
    native("rqlite", "RQLite", 4001, "lightweight", "rqlite原生HTTP SQL /db/request，支持查询与DML；url/ssl/ssl_verify参数"),
    http(
        "turso",
        "Turso",
        8080,
        "lightweight",
        "turso",
        "远程平台连接：参数中填写 url 与 token（sqld HTTP 接口）；本地 libSQL 文件请直接使用 SQLite 类型。",
    ),
    http(
        "d1",
        "Cloudflare D1",
        443,
        "lightweight",
        "d1",
        "参数中需填写 account_id、database_id、api_token。",
    ),
    odbc(
        "duckdb",
        "DuckDB",
        0,
        "lightweight",
        "DuckDB Driver",
        "Windows x64/ARM64 可直接下载官方完整免安装 ODBC 包，验证当前架构与依赖后自动使用；文件路径填入「文件」字段，内存库填 :memory:。", 
    ),
    odbc(
        "access",
        "Microsoft Access",
        0,
        "lightweight",
        "Microsoft Access Driver (*.mdb, *.accdb)",
        "可直接下载 UCanAccess 5.3.0 完整 uber JDBC 包（含 Jackcess/HSQL，需 Java 11+），打开本地 .mdb/.accdb；新建文件需明确 newDatabaseVersion=V2010。也保留已安装 ACE/Access ODBC，不假定系统自带。", 
    ),
    native(
        "h2",
        "H2",
        0,
        "lightweight",
        "H2真实JDBC：需Java 11+和受管h2.jar；java/h2_jar/jdbc_url参数，支持内存/文件/TCP、schema目录与SQL。",
    ),
    // ------------------------------------------------ 文档/缓存/检索
    k("redis", "Redis", 6379, Protocol::Redis, "nosql"),
    k("mongodb", "MongoDB", 27017, Protocol::Mongo, "nosql"),
    native(
        "dynamodb",
        "Amazon DynamoDB",
        443,
        "nosql",
        "DynamoDB原生签名API（SigV4）：region/access_key/secret_key/session_token/url，ListTables/Scan/PartiQL或JSON operation/payload。",
    ),
    http(
        "elasticsearch",
        "Elasticsearch",
        9200,
        "nosql",
        "elasticsearch",
        "HTTP REST 接口；开启认证时在用户名 / 密码中填写，参数可指定 ssl=true。",
    ),
    http(
        "easysearch",
        "Easysearch",
        9200,
        "nosql",
        "elasticsearch",
        "INFINI Easysearch 兼容 Elasticsearch REST 接口。",
    ),
    http(
        "solr",
        "Apache Solr",
        8983,
        "nosql",
        "solr",
        "列出 Core / Collection 并浏览文档；SQL 查询需目标端启用 Streaming Expression。",
    ),
    http(
        "couchdb",
        "Apache CouchDB",
        5984,
        "nosql",
        "couchdb",
        "HTTP REST 接口。",
    ),
    http(
        "meilisearch",
        "Meilisearch",
        7700,
        "nosql",
        "meilisearch",
        "HTTP REST 接口；含 API Key 时填入密码字段。",
    ),
    native(
        "hbase",
        "Apache HBase",
        8080,
        "nosql",
        "内置 HBase Stargate REST（不是 Thrift 9090/Phoenix）：namespace/表/列族、真实 scanner 分页及 JSON scan/get/put/delete，二进制保真。Basic 凭据仅 HTTPS 或字面回环；Kerberos/SPNEGO 可用显式自定义驱动。",
    ),
    k("manticore", "Manticore Search", 9306, Protocol::Mysql, "nosql"),
    native(
        "cassandra",
        "Cassandra",
        9042,
        "nosql",
        "Cassandra原生CQL：认证、keyspace/table目录、分页浏览与CQL；TLS请求明确拒绝，不回退ODBC。",
    ),
    // ------------------------------------------------ 图谱/向量
    http(
        "qdrant",
        "Qdrant",
        6333,
        "graph",
        "qdrant",
        "HTTP REST 接口；含 API Key 时填入密码字段。",
    ),
    http(
        "milvus",
        "Milvus",
        9091,
        "graph",
        "milvus",
        "需启用 Milvus HTTP 接口（默认 9091）。",
    ),
    http(
        "weaviate",
        "Weaviate",
        8080,
        "graph",
        "weaviate",
        "HTTP REST 接口。",
    ),
    http(
        "chromadb",
        "ChromaDB",
        8000,
        "graph",
        "chroma",
        "HTTP REST 接口。",
    ),
    http(
        "neo4j",
        "Neo4j",
        7474,
        "graph",
        "neo4j",
        "HTTP 事务接口（7474）；查询标签页直接执行 Cypher。",
    ),
    native(
        "nebulagraph",
        "NebulaGraph",
        9669,
        "graph",
        "NebulaGraph v3原生Thrift：认证、space/tag/edge目录与nGQL；需要图查询权限及浏览所需索引。",
    ),
    // ------------------------------------------------ 时序数据库
    k("questdb", "QuestDB", 8812, Protocol::Postgres, "timeseries"),
    http(
        "tdengine",
        "TDengine",
        6041,
        "timeseries",
        "tdengine",
        "REST 接口（taosAdapter）；查询标签页直接执行 SQL。",
    ),
    http(
        "iotdb",
        "Apache IoTDB",
        18080,
        "timeseries",
        "iotdb",
        "需启用 IoTDB REST API v2；查询标签页直接执行 SQL。",
    ),
    http(
        "influxdb",
        "InfluxDB",
        8086,
        "timeseries",
        "influxdb",
        "使用 InfluxQL /query（非 Flux）；v1 支持 Basic，v2 需预建 DBRP（db/rp→bucket）映射并提供有权限 Token；用户名留空、密码填 Token，可选参数 rp。",
    ),
    http(
        "victoriametrics",
        "VictoriaMetrics",
        8428,
        "timeseries",
        "victoriametrics",
        "MetricsQL 查询接口；树中按指标名浏览。",
    ),
    // ------------------------------------------------ 消息队列
    http(
        "pulsar",
        "Apache Pulsar",
        8080,
        "mq",
        "pulsar",
        "REST 管理接口（broker 的 8080），浏览租户 / 命名空间 / 主题。",
    ),
    native(
        "kafka",
        "Apache Kafka",
        9092,
        "mq",
        "Kafka原生协议：topic/partition目录、fetch/produce JSON命令；brokers/SASL PLAIN或SCRAM参数；暂不支持TLS。",
    ),
    native(
        "rocketmq",
        "Apache RocketMQ",
        9876,
        "mq",
        "RocketMQ原生Remoting：NameServer topic/route目录，broker pull/send JSON；nameserver参数、ACL AccessKey/SecretKey。",
    ),
    http(
        "rabbitmq",
        "RabbitMQ",
        15672,
        "mq",
        "rabbitmq",
        "HTTP 管理接口，需启用 management 插件；用户名 / 密码填管理账号。",
    ),
    native(
        "mqtt",
        "MQTT",
        1883,
        "mq",
        "MQTT 3.1.1：topic_filter订阅实际观察主题和消息；JSON subscribe/publish等待ACK；无标准全量topic目录API，暂不支持TLS。",
    ),
    // ------------------------------------------------ 注册与配置
    http(
        "nacos",
        "Nacos",
        8848,
        "registry",
        "nacos",
        "HTTP Open API，浏览服务列表与配置；开启鉴权时填用户名 / 密码。",
    ),
    http(
        "consul",
        "Consul",
        8500,
        "registry",
        "consul",
        "HTTP KV 接口；含 ACL Token 时填入密码字段。",
    ),
    native(
        "zookeeper",
        "ZooKeeper",
        2181,
        "registry",
        "ZooKeeper原生协议：znode目录、get/set/create/delete JSON；hosts/auth_scheme参数，用户名密码用于digest ACL。",
    ),
    native(
        "etcd",
        "etcd",
        2379,
        "registry",
        "etcd v3原生JSON gateway：token认证、key目录/get/range/put/delete；需启用v3 HTTP gateway，支持url/ssl/ssl_verify。",
    ),
];

/// 按 id 查找目录项（忽略大小写）。
pub fn meta(id: &str) -> Option<&'static KindMeta> {
    CATALOG.iter().find(|m| m.id.eq_ignore_ascii_case(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_ids_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for m in CATALOG {
            assert!(seen.insert(m.id), "重复的目录 id: {}", m.id);
        }
    }

    #[test]
    fn lookup_case_insensitive() {
        assert!(meta("MySQL").is_some());
        assert!(meta("dameng").is_some());
        assert!(meta("nope").is_none());
    }
}
