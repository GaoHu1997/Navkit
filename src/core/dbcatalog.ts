/**
 * 数据源类型目录 —— 由 src-tauri/src/catalog.rs 经 tools/gen_dbcatalog.py 生成，
 * 请勿手工编辑（改动请改 Rust 目录后重新生成）。「新建连接」选择器与
 * 连接属性对话框的数据源列表、分类、默认端口、驱动提示均来源于此。
 */

export type DbCategory =
  | "relational"
  | "analytics"
  | "domestic"
  | "lightweight"
  | "nosql"
  | "graph"
  | "timeseries"
  | "mq"
  | "registry";

export interface KindInfo {
  id: string;
  label: string;
  port: number;
  category: DbCategory;
  /** 连接协议族：mysql / postgres / sqlite / mssql / redis / mongo / clickhouse / http / native / odbc / planned。 */
  protocol: string;
  /** ODBC 驱动名提示（仅 ODBC 类数据源）。 */
  driver: string;
  /** 连接说明。 */
  note: string;
  /** 适配器尚未提供（仅可浏览目录）。 */
  planned: boolean;
}

/** 数据源分类（顺序即选择器展示顺序）。 */
export const DB_CATEGORIES: { id: DbCategory; label: string }[] = [
  { id: "relational", label: "关系型数据库" },
  { id: "analytics", label: "数仓分析" },
  { id: "domestic", label: "国产数据库" },
  { id: "lightweight", label: "轻量数据库" },
  { id: "nosql", label: "文档/缓存/检索" },
  { id: "graph", label: "图谱/向量" },
  { id: "timeseries", label: "时序数据库" },
  { id: "mq", label: "消息队列" },
  { id: "registry", label: "注册与配置" },
];

export const DB_KINDS: KindInfo[] = [
  { id: "mysql", label: "MySQL", port: 3306, category: "relational", protocol: "mysql", driver: "", note: "", planned: false },
  { id: "postgresql", label: "PostgreSQL", port: 5432, category: "relational", protocol: "postgres", driver: "", note: "", planned: false },
  { id: "sqlserver", label: "SQL Server", port: 1433, category: "relational", protocol: "mssql", driver: "", note: "", planned: false },
  { id: "oracle", label: "Oracle", port: 1521, category: "relational", protocol: "odbc", driver: "Oracle ODBC Driver", note: "可直接下载官方 thin JDBC（ojdbc11 8.8MB，需 Java 11+）；也可安装 Oracle Instant Client ODBC（或下载完整客户端），驱动名不一致时在参数中填写完整 ODBC 连接串。", planned: false },
  { id: "mariadb", label: "MariaDB", port: 3306, category: "relational", protocol: "mysql", driver: "", note: "", planned: false },
  { id: "firebird", label: "Firebird", port: 3050, category: "relational", protocol: "odbc", driver: "Firebird/InterBase(r) driver", note: "可直接下载官方 Jaybird JDBC（需 Java 11+，数据库名填服务器端路径/别名）；也可安装 Firebird ODBC 驱动。", planned: false },
  { id: "cockroachdb", label: "CockroachDB", port: 26257, category: "relational", protocol: "postgres", driver: "", note: "", planned: false },
  { id: "db2", label: "DB2", port: 50000, category: "relational", protocol: "odbc", driver: "IBM DB2 ODBC DRIVER", note: "可直接下载官方 IBM JDBC（jcc，需 Java 11+）；也可安装 IBM Data Server Client（含 DB2 ODBC 驱动）。", planned: false },
  { id: "informix", label: "Informix", port: 9088, category: "relational", protocol: "odbc", driver: "IBM INFORMIX ODBC DRIVER", note: "可直接下载官方 Informix JDBC（需 Java 11+，参数需 informixserver）；也可安装 IBM Informix Client（含 ODBC 驱动）。", planned: false },
  { id: "jdbcx", label: "JDBCX", port: 0, category: "relational", protocol: "odbc", driver: "", note: "通用 JDBC 桥接数据源：通过 ODBC DSN / 连接串接入，请在参数中填写完整 ODBC 连接串。", planned: false },
  { id: "spanner", label: "Cloud Spanner", port: 443, category: "relational", protocol: "odbc", driver: "", note: "可直接下载官方 Google Spanner 2.47.0 完整 JDBC 包（Java 11+）；默认 spanner.googleapis.com:443、数据库填 project/instance/database、用户名 token、密码填预生成 OAuth 令牌。无凭据本地模拟器须显式回环地址与 ssl=false；驱动就绪不代表 Google 云端 SQL/TLS 已验收。", planned: false },
  { id: "iris", label: "IRIS", port: 1972, category: "relational", protocol: "odbc", driver: "InterSystems IRIS ODBC", note: "可直接下载官方 IRIS JDBC 3.11.0（原签名完整保留，需 Java 11+）；数据库名填 namespace，默认 USER；也保留已安装 IRIS ODBC。IRIS 驱动不代替 Caché。", planned: false },
  { id: "cache", label: "InterSystems Caché", port: 1972, category: "relational", protocol: "odbc", driver: "InterSystems ODBC", note: "可直接下载自有镜像（Navkit-drivers）的 Caché 原 JDBC 2010.2.3.702（原件来自参考项目 dbx 固定提交，第三方来源，非厂商官网；保留原包许可，需 Java 11+）。普通 TCP 路由已验，服务器 SQL/认证/TLS 未验；也保留已安装 Caché ODBC。", planned: false },
  { id: "custom_mysql", label: "Custom (MySQL)", port: 3306, category: "relational", protocol: "mysql", driver: "", note: "", planned: false },
  { id: "dolt", label: "Dolt", port: 3307, category: "relational", protocol: "mysql", driver: "", note: "", planned: false },
  { id: "custom_pg", label: "Custom (PostgreSQL)", port: 5432, category: "relational", protocol: "postgres", driver: "", note: "", planned: false },
  { id: "odbc", label: "自定义 ODBC", port: 0, category: "relational", protocol: "odbc", driver: "", note: "通过系统 ODBC 数据源接入：数据库名填 DSN，或在参数中填写完整 ODBC 连接串。", planned: false },
  { id: "cloudberry", label: "Apache Cloudberry", port: 5432, category: "analytics", protocol: "postgres", driver: "", note: "", planned: false },
  { id: "clickhouse", label: "ClickHouse", port: 8123, category: "analytics", protocol: "clickhouse", driver: "", note: "", planned: false },
  { id: "databend", label: "Databend", port: 3307, category: "analytics", protocol: "mysql", driver: "", note: "", planned: false },
  { id: "databricks", label: "Databricks SQL", port: 443, category: "analytics", protocol: "odbc", driver: "Simba Spark ODBC Driver", note: "可下载官方 Databricks JDBC 3.4.4，需 Java 11+、httpPath、用户名 token 和 PAT 密码，严格 TLS；也保留已安装 Simba Spark / Databricks ODBC。数据库名可填 catalog/schema；OAuth 使用显式自定义 JDBC。", planned: false },
  { id: "saphana", label: "SAP HANA", port: 30015, category: "analytics", protocol: "odbc", driver: "HDBODBC", note: "可直接下载官方 HANA JDBC（ngdbc，需 Java 11+）；也可安装 SAP HANA Client（HDBODBC 驱动）。", planned: false },
  { id: "teradata", label: "Teradata", port: 1025, category: "analytics", protocol: "odbc", driver: "Teradata Database ODBC Driver", note: "可直接下载官方 Teradata JDBC（需 Java 11+；TLS 走 https_port）；也可安装 Teradata ODBC 驱动。", planned: false },
  { id: "vertica", label: "Vertica", port: 5433, category: "analytics", protocol: "odbc", driver: "Vertica ODBC Driver", note: "可直接下载官方 Vertica JDBC（需 Java 11+）；也可安装 Vertica ODBC 驱动。", planned: false },
  { id: "exasol", label: "Exasol", port: 8563, category: "analytics", protocol: "odbc", driver: "EXASolution", note: "可直接下载官方 Exasol JDBC（需 Java 11+）；也可安装 Exasol ODBC 驱动。", planned: false },
  { id: "doris", label: "Doris", port: 9030, category: "analytics", protocol: "mysql", driver: "", note: "", planned: false },
  { id: "selectdb", label: "SelectDB", port: 9030, category: "analytics", protocol: "mysql", driver: "", note: "", planned: false },
  { id: "starrocks", label: "StarRocks", port: 9030, category: "analytics", protocol: "mysql", driver: "", note: "", planned: false },
  { id: "redshift", label: "Redshift", port: 5439, category: "analytics", protocol: "postgres", driver: "", note: "", planned: false },
  { id: "snowflake", label: "Snowflake", port: 443, category: "analytics", protocol: "odbc", driver: "SnowflakeDSIIDriver", note: "可直接下载官方 Snowflake JDBC 4.4.1（约 98MB，强制 TLS，需 Java 11+；host 填 org-account.snowflakecomputing.com）；也可安装 Snowflake ODBC 驱动。", planned: false },
  { id: "trino", label: "Trino", port: 8080, category: "analytics", protocol: "odbc", driver: "Trino ODBC Driver", note: "支持已完整安装的 Trino ODBC；缺驱动可直接下载官方 JDBC 435（Java 11+），默认自动使用托管 JDBC。数据库填 catalog/schema；密码认证必须启用 TLS。服务器仍需实际验证。", planned: false },
  { id: "prestosql", label: "PrestoSQL", port: 8080, category: "analytics", protocol: "odbc", driver: "Presto ODBC Driver", note: "支持已完整安装的 Presto ODBC；缺驱动可直接下载官方 PrestoSQL JDBC 350（Java 11+），默认自动使用托管 JDBC。并非 PrestoDB 通用驱动；数据库填 catalog/schema，密码认证必须 TLS。", planned: false },
  { id: "hive", label: "Hive", port: 10000, category: "analytics", protocol: "odbc", driver: "Hive ODBC Driver", note: "可直接下载官方 Hive standalone JDBC（72MB，需 Java 11+）；也可安装 Cloudera/Apache Hive ODBC 驱动。", planned: false },
  { id: "kyuubi", label: "Apache Kyuubi", port: 10009, category: "analytics", protocol: "odbc", driver: "Hive ODBC Driver", note: "Kyuubi 兼容 Hive Thrift 协议；可直接下载官方 Hive standalone JDBC（与 Hive 共用包，需 Java 11+），也可安装 Hive ODBC 驱动。", planned: false },
  { id: "impala", label: "Apache Impala", port: 21050, category: "analytics", protocol: "odbc", driver: "Cloudera Impala ODBC Driver", note: "可直接下载官方 Hive standalone JDBC（Impala 兼容 Hive Thrift，与 Hive 共用包，需 Java 11+）；也可安装 Cloudera Impala ODBC 驱动。", planned: false },
  { id: "spark", label: "Apache Spark", port: 10000, category: "analytics", protocol: "odbc", driver: "Simba Spark ODBC Driver", note: "Spark Thrift Server 兼容 Hive Thrift 协议；可直接下载官方 Hive standalone JDBC（与 Hive 共用包，需 Java 11+），也可安装 Simba Spark ODBC 驱动。", planned: false },
  { id: "dremio", label: "Dremio", port: 31010, category: "analytics", protocol: "odbc", driver: "Dremio ODBC Driver", note: "可直接下载官方 Dremio legacy 完整 JDBC 包（需 Java 11 或 17，应用自动使用固定模块参数）；默认数据库字段留空。该包不是 Arrow Flight SQL；也保留 Dremio ODBC。", planned: false },
  { id: "bigquery", label: "BigQuery", port: 443, category: "analytics", protocol: "odbc", driver: "", note: "可直接下载官方 BigQuery 完整 JDBC all 包（1.9.0，需 Java 11+）。默认主机 bigquery.googleapis.com，数据库填 project[/dataset]，用户名 token，密码填 OAuth 访问令牌；令牌被拒绝或过期需替换后重连，不支持自动刷新。驱动就绪不代表云端账户或 TLS 已验收。", planned: false },
  { id: "kylin", label: "Kylin", port: 7070, category: "analytics", protocol: "odbc", driver: "Kylin ODBC Driver", note: "可直接下载官方 Kylin JDBC（5.0.5，需 Java 11+，数据库名填 project）；也可安装 Apache Kylin ODBC 驱动。", planned: false },
  { id: "ignite", label: "Apache Ignite", port: 10800, category: "analytics", protocol: "odbc", driver: "Apache Ignite", note: "可直接下载官方 Ignite 2.18.0 thin JDBC 及六个必需依赖（需 Java 11 或 17，应用使用对应官方模块参数）；数据库名填 schema，默认 PUBLIC。此驱动不用于 Ignite 3；也保留原 ODBC。", planned: false },
  { id: "salesforce", label: "Salesforce", port: 443, category: "analytics", protocol: "native", driver: "", note: "内置 Salesforce REST：主机填组织 instance 域名，密码填预生成 OAuth token，数据库填 API 版本（默认 v59.0）。支持对象目录/describe、SOQL 游标分页及 JSON create/update/delete；云端强制验证 HTTPS，不自动刷新 token。保留显式自定义 JDBC/ODBC。", planned: false },
  { id: "druid", label: "Apache Druid", port: 8888, category: "analytics", protocol: "native", driver: "", note: "Druid真实SQL HTTP /druid/v2/sql；支持INFORMATION_SCHEMA目录、浏览和查询；url/ssl/ssl_verify参数。", planned: false },
  { id: "opententbase", label: "OpenTenBase", port: 5432, category: "domestic", protocol: "postgres", driver: "", note: "", planned: false },
  { id: "tidb", label: "TiDB", port: 4000, category: "domestic", protocol: "mysql", driver: "", note: "", planned: false },
  { id: "oceanbase", label: "OceanBase", port: 2881, category: "domestic", protocol: "mysql", driver: "", note: "", planned: false },
  { id: "goldendb", label: "金篆 GoldenDB", port: 3306, category: "domestic", protocol: "mysql", driver: "", note: "", planned: false },
  { id: "tdsql", label: "TDSQL", port: 3306, category: "domestic", protocol: "mysql", driver: "", note: "", planned: false },
  { id: "polardb", label: "PolarDB", port: 3306, category: "domestic", protocol: "mysql", driver: "", note: "", planned: false },
  { id: "greatsql", label: "GreatSQL", port: 3306, category: "domestic", protocol: "mysql", driver: "", note: "", planned: false },
  { id: "gbase", label: "南大通用 GBase", port: 5258, category: "domestic", protocol: "odbc", driver: "GBase 8a ODBC Driver", note: "可直接下载南大通用官网 GBase 8a JDBC 9.5.0.10-build1 原 ZIP/JAR/readme（Java 11+）；默认普通 TCP 用户名/密码连接，非 GBase 8s/8c。TLS/集群扩展请配置自定义 JDBC；也保留 GBase 8a ODBC。", planned: false },
  { id: "opengauss", label: "openGauss", port: 5432, category: "domestic", protocol: "postgres", driver: "", note: "", planned: false },
  { id: "gaussdb", label: "GaussDB", port: 8000, category: "domestic", protocol: "postgres", driver: "", note: "", planned: false },
  { id: "kwdb", label: "KWDB", port: 26257, category: "domestic", protocol: "postgres", driver: "", note: "", planned: false },
  { id: "kingbase", label: "金仓 KingbaseES", port: 54321, category: "domestic", protocol: "postgres", driver: "", note: "", planned: false },
  { id: "highgo", label: "瀚高 HighGo", port: 5866, category: "domestic", protocol: "postgres", driver: "", note: "", planned: false },
  { id: "uxdb", label: "优炫 UXDB", port: 5432, category: "domestic", protocol: "postgres", driver: "", note: "", planned: false },
  { id: "yashandb", label: "崖山 YashanDB", port: 1688, category: "domestic", protocol: "odbc", driver: "YashanDB ODBC Driver", note: "可直接下载官方 YashanDB JDBC 1.10.7（需 Java 11+）；支持默认 JDBC 路由，也保留已安装崖山 ODBC。托管 TLS/TLCP 未核验，请用显式自定义 JDBC，不能静默降级。", planned: false },
  { id: "vastbase", label: "海量 Vastbase", port: 5432, category: "domestic", protocol: "postgres", driver: "", note: "", planned: false },
  { id: "dameng", label: "达梦 Dameng", port: 5236, category: "domestic", protocol: "odbc", driver: "DM8 ODBC DRIVER", note: "可直接下载官方达梦 JDBC（DmJdbcDriver18，需 Java 11+）；也可安装达梦自带的 DM8 ODBC DRIVER。", planned: false },
  { id: "inceptor", label: "星环 Inceptor", port: 10000, category: "domestic", protocol: "odbc", driver: "Hive ODBC Driver", note: "可直接下载星环官方 Inceptor JDBC 8.37.3 原 JAR（Java 11+，官方公开导航来源）；普通 inceptor2/noSasl 路由已验，Kerberos/HTTP/TLS 等扩展请用自定义 JDBC。不是 Apache Hive standalone 包，也不以此冒充 ArgoDB 实库认证。", planned: false },
  { id: "argodb", label: "ArgoDB", port: 10000, category: "domestic", protocol: "odbc", driver: "Hive ODBC Driver", note: "可直接下载星环官方 Quark JDBC 8.37.3 独立完整包（Java 11+）；ArgoDB 文档指定 Quark Gateway 为 SQL 入口。实际 io.transwarp.jdbc.QuarkDriver + jdbc:transwarp2，仅 auth=noSasl 且用户名/密码留空；LDAP/NONE/Kerberos/TLS 扩展请显式配置自定义驱动，厂商实库 SQL/认证未验。", planned: false },
  { id: "sundb", label: "科蓝 SUNDB", port: 40000, category: "domestic", protocol: "odbc", driver: "SUNDB ODBC Driver", note: "可直接下载自有镜像（Navkit-drivers）的 SUNDB 8-1.0.2 原 JDBC（原件来自参考项目 dbx 固定提交，第三方来源，非厂商官网，Java 11+）；普通 TCP 路由已验，服务器 SQL/认证/TLS 未验；也保留已安装科蓝 ODBC。", planned: false },
  { id: "oscar", label: "神通 OSCAR", port: 2003, category: "domestic", protocol: "odbc", driver: "Shentong ODBC Driver", note: "可直接下载自有镜像（Navkit-drivers）的 OSCAR JDBC 原 JAR（原件来自参考项目 dbx 固定提交，第三方来源，非厂商官网，Java 11+）；保留原包许可/版本，普通 TCP 路由已验，服务器 SQL/认证/TLS 未验；也保留神通 ODBC。", planned: false },
  { id: "xugu", label: "虚谷 XuguDB", port: 5138, category: "domestic", protocol: "odbc", driver: "Xugu ODBC Driver", note: "可直接下载官方虚谷 JDBC 12.3.9 与必需 checker-qual 依赖（需 Java 11+）；数据库名默认 SYSTEM，也保留已安装虚谷 ODBC。托管 TLS 参数尚未核验。", planned: false },
  { id: "sqlite", label: "SQLite", port: 0, category: "lightweight", protocol: "sqlite", driver: "", note: "", planned: false },
  { id: "rqlite", label: "RQLite", port: 4001, category: "lightweight", protocol: "native", driver: "", note: "rqlite原生HTTP SQL /db/request，支持查询与DML；url/ssl/ssl_verify参数", planned: false },
  { id: "turso", label: "Turso", port: 8080, category: "lightweight", protocol: "http", driver: "", note: "远程平台连接：参数中填写 url 与 token（sqld HTTP 接口）；本地 libSQL 文件请直接使用 SQLite 类型。", planned: false },
  { id: "d1", label: "Cloudflare D1", port: 443, category: "lightweight", protocol: "http", driver: "", note: "参数中需填写 account_id、database_id、api_token。", planned: false },
  { id: "duckdb", label: "DuckDB", port: 0, category: "lightweight", protocol: "odbc", driver: "DuckDB Driver", note: "Windows x64/ARM64 可直接下载官方完整免安装 ODBC 包，验证当前架构与依赖后自动使用；文件路径填入「文件」字段，内存库填 :memory:。", planned: false },
  { id: "access", label: "Microsoft Access", port: 0, category: "lightweight", protocol: "odbc", driver: "Microsoft Access Driver (*.mdb, *.accdb)", note: "可直接下载 UCanAccess 5.3.0 完整 uber JDBC 包（含 Jackcess/HSQL，需 Java 11+），打开本地 .mdb/.accdb；新建文件需明确 newDatabaseVersion=V2010。也保留已安装 ACE/Access ODBC，不假定系统自带。", planned: false },
  { id: "h2", label: "H2", port: 0, category: "lightweight", protocol: "native", driver: "", note: "H2真实JDBC：需Java 11+和受管h2.jar；java/h2_jar/jdbc_url参数，支持内存/文件/TCP、schema目录与SQL。", planned: false },
  { id: "redis", label: "Redis", port: 6379, category: "nosql", protocol: "redis", driver: "", note: "", planned: false },
  { id: "mongodb", label: "MongoDB", port: 27017, category: "nosql", protocol: "mongo", driver: "", note: "", planned: false },
  { id: "dynamodb", label: "Amazon DynamoDB", port: 443, category: "nosql", protocol: "native", driver: "", note: "DynamoDB原生签名API（SigV4）：region/access_key/secret_key/session_token/url，ListTables/Scan/PartiQL或JSON operation/payload。", planned: false },
  { id: "elasticsearch", label: "Elasticsearch", port: 9200, category: "nosql", protocol: "http", driver: "", note: "HTTP REST 接口；开启认证时在用户名 / 密码中填写，参数可指定 ssl=true。", planned: false },
  { id: "easysearch", label: "Easysearch", port: 9200, category: "nosql", protocol: "http", driver: "", note: "INFINI Easysearch 兼容 Elasticsearch REST 接口。", planned: false },
  { id: "solr", label: "Apache Solr", port: 8983, category: "nosql", protocol: "http", driver: "", note: "列出 Core / Collection 并浏览文档；SQL 查询需目标端启用 Streaming Expression。", planned: false },
  { id: "couchdb", label: "Apache CouchDB", port: 5984, category: "nosql", protocol: "http", driver: "", note: "HTTP REST 接口。", planned: false },
  { id: "meilisearch", label: "Meilisearch", port: 7700, category: "nosql", protocol: "http", driver: "", note: "HTTP REST 接口；含 API Key 时填入密码字段。", planned: false },
  { id: "hbase", label: "Apache HBase", port: 8080, category: "nosql", protocol: "native", driver: "", note: "内置 HBase Stargate REST（不是 Thrift 9090/Phoenix）：namespace/表/列族、真实 scanner 分页及 JSON scan/get/put/delete，二进制保真。Basic 凭据仅 HTTPS 或字面回环；Kerberos/SPNEGO 可用显式自定义驱动。", planned: false },
  { id: "manticore", label: "Manticore Search", port: 9306, category: "nosql", protocol: "mysql", driver: "", note: "", planned: false },
  { id: "cassandra", label: "Cassandra", port: 9042, category: "nosql", protocol: "native", driver: "", note: "Cassandra原生CQL：认证、keyspace/table目录、分页浏览与CQL；TLS请求明确拒绝，不回退ODBC。", planned: false },
  { id: "qdrant", label: "Qdrant", port: 6333, category: "graph", protocol: "http", driver: "", note: "HTTP REST 接口；含 API Key 时填入密码字段。", planned: false },
  { id: "milvus", label: "Milvus", port: 9091, category: "graph", protocol: "http", driver: "", note: "需启用 Milvus HTTP 接口（默认 9091）。", planned: false },
  { id: "weaviate", label: "Weaviate", port: 8080, category: "graph", protocol: "http", driver: "", note: "HTTP REST 接口。", planned: false },
  { id: "chromadb", label: "ChromaDB", port: 8000, category: "graph", protocol: "http", driver: "", note: "HTTP REST 接口。", planned: false },
  { id: "neo4j", label: "Neo4j", port: 7474, category: "graph", protocol: "http", driver: "", note: "HTTP 事务接口（7474）；查询标签页直接执行 Cypher。", planned: false },
  { id: "nebulagraph", label: "NebulaGraph", port: 9669, category: "graph", protocol: "native", driver: "", note: "NebulaGraph v3原生Thrift：认证、space/tag/edge目录与nGQL；需要图查询权限及浏览所需索引。", planned: false },
  { id: "questdb", label: "QuestDB", port: 8812, category: "timeseries", protocol: "postgres", driver: "", note: "", planned: false },
  { id: "tdengine", label: "TDengine", port: 6041, category: "timeseries", protocol: "http", driver: "", note: "REST 接口（taosAdapter）；查询标签页直接执行 SQL。", planned: false },
  { id: "iotdb", label: "Apache IoTDB", port: 18080, category: "timeseries", protocol: "http", driver: "", note: "需启用 IoTDB REST API v2；查询标签页直接执行 SQL。", planned: false },
  { id: "influxdb", label: "InfluxDB", port: 8086, category: "timeseries", protocol: "http", driver: "", note: "使用 InfluxQL /query（非 Flux）；v1 支持 Basic，v2 需预建 DBRP（db/rp→bucket）映射并提供有权限 Token；用户名留空、密码填 Token，可选参数 rp。", planned: false },
  { id: "victoriametrics", label: "VictoriaMetrics", port: 8428, category: "timeseries", protocol: "http", driver: "", note: "MetricsQL 查询接口；树中按指标名浏览。", planned: false },
  { id: "pulsar", label: "Apache Pulsar", port: 8080, category: "mq", protocol: "http", driver: "", note: "REST 管理接口（broker 的 8080），浏览租户 / 命名空间 / 主题。", planned: false },
  { id: "kafka", label: "Apache Kafka", port: 9092, category: "mq", protocol: "native", driver: "", note: "Kafka原生协议：topic/partition目录、fetch/produce JSON命令；brokers/SASL PLAIN或SCRAM参数；暂不支持TLS。", planned: false },
  { id: "rocketmq", label: "Apache RocketMQ", port: 9876, category: "mq", protocol: "native", driver: "", note: "RocketMQ原生Remoting：NameServer topic/route目录，broker pull/send JSON；nameserver参数、ACL AccessKey/SecretKey。", planned: false },
  { id: "rabbitmq", label: "RabbitMQ", port: 15672, category: "mq", protocol: "http", driver: "", note: "HTTP 管理接口，需启用 management 插件；用户名 / 密码填管理账号。", planned: false },
  { id: "mqtt", label: "MQTT", port: 1883, category: "mq", protocol: "native", driver: "", note: "MQTT 3.1.1：topic_filter订阅实际观察主题和消息；JSON subscribe/publish等待ACK；无标准全量topic目录API，暂不支持TLS。", planned: false },
  { id: "nacos", label: "Nacos", port: 8848, category: "registry", protocol: "http", driver: "", note: "HTTP Open API，浏览服务列表与配置；开启鉴权时填用户名 / 密码。", planned: false },
  { id: "consul", label: "Consul", port: 8500, category: "registry", protocol: "http", driver: "", note: "HTTP KV 接口；含 ACL Token 时填入密码字段。", planned: false },
  { id: "zookeeper", label: "ZooKeeper", port: 2181, category: "registry", protocol: "native", driver: "", note: "ZooKeeper原生协议：znode目录、get/set/create/delete JSON；hosts/auth_scheme参数，用户名密码用于digest ACL。", planned: false },
  { id: "etcd", label: "etcd", port: 2379, category: "registry", protocol: "native", driver: "", note: "etcd v3原生JSON gateway：token认证、key目录/get/range/put/delete；需启用v3 HTTP gateway，支持url/ssl/ssl_verify。", planned: false },
];

const KIND_MAP = new Map<string, KindInfo>(DB_KINDS.map((k) => [k.id, k]));

/** 按分类取数据源列表。 */
export function kindsByCategory(category: DbCategory): KindInfo[] {
  return DB_KINDS.filter((k) => k.category === category);
}

export function kindMeta(id: string): KindInfo | undefined {
  return KIND_MAP.get(id);
}

/** 显示名（未知类型原样返回）。 */
export function kindLabel(id: string): string {
  return KIND_MAP.get(id)?.label ?? id;
}

/** 对象树 / 卡片使用的图标注册名。 */
export function kindIcon(id: string): string {
  return `info_pane_conn_${id}`;
}
