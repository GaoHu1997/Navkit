//! 数据模型：连接、对象、结果集等前后端共用的结构定义。

use serde::{Deserialize, Serialize};

/// 连接协议族：决定使用哪种驱动与 SQL 方言。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Sqlite,
    Mysql,
    Postgres,
    /// ClickHouse 原生 HTTP 接口。
    Clickhouse,
    /// SQL Server（TDS 协议，tiberius 原生客户端）。
    Mssql,
    /// Redis（RESP 协议，redis-rs 原生客户端）。
    Redis,
    /// MongoDB（mongodb 原生客户端）。
    Mongo,
    /// HTTP REST 接口（检索 / 向量 / 时序 / MQ / 注册配置类，见 catalog.http 适配器）。
    Http,
    /// 原生SDK / JDBC / 专用服务API。
    Native,
    /// Windows ODBC 驱动（需安装厂商 ODBC 驱动）。
    Odbc,
}

/// 支持的数据库类型：目录 id（小写字符串），元信息统一查 catalog.rs。
///
/// 以字符串为载体是为了表驱动化：新增数据源类型只需在 catalog.rs 的
/// CATALOG 表中加一行，无需再改枚举与全部 match。serde 透明序列化保证
/// 与旧版本连接配置文件（"mysql" 等字符串）双向兼容。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DbKind(pub String);

impl DbKind {
    pub fn of(id: &str) -> Self {
        DbKind(id.trim().to_ascii_lowercase())
    }

    /// 目录 id。
    pub fn id(&self) -> &str {
        &self.0
    }

    /// 目录元信息；未知类型返回 None。
    pub fn meta(&self) -> Option<&'static crate::catalog::KindMeta> {
        crate::catalog::meta(&self.0)
    }

    /// 该数据库使用的连接协议（未知类型按 ODBC DSN 处理）。
    pub fn protocol(&self) -> Protocol {
        self.meta().map(|m| m.protocol).unwrap_or(Protocol::Odbc)
    }

    /// 是否为 MySQL 协议族（mysql.user、SHOW 语法可用）。
    pub fn is_mysql_family(&self) -> bool {
        self.protocol() == Protocol::Mysql
    }

    /// 是否为 PostgreSQL 协议族。
    pub fn is_pg_family(&self) -> bool {
        self.protocol() == Protocol::Postgres
    }

    /// 是否为 SQLite。
    pub fn is_sqlite(&self) -> bool {
        self.protocol() == Protocol::Sqlite
    }

    /// 默认端口。
    pub fn default_port(&self) -> u16 {
        self.meta().map(|m| m.port).unwrap_or(0)
    }

    /// 目录 id（规范形式）。
    pub fn as_str(&self) -> &'static str {
        self.meta().map(|m| m.id).unwrap_or("unknown")
    }

    /// 对象树中该类型连接使用的图标名（前端图标注册表按 id 生成）。
    pub fn icon(&self) -> String {
        format!("info_pane_conn_{}", self.0)
    }

    /// 连接对话框 / 提示中使用的显示名。
    pub fn label(&self) -> &'static str {
        self.meta().map(|m| m.label).unwrap_or("未知类型")
    }

    /// 适配器尚未提供（仅可浏览目录）。
    pub fn is_planned(&self) -> bool {
        self.meta().map(|m| m.planned).unwrap_or(true)
    }
}

/// 数据源类型目录项（db_kinds 命令返回，驱动新建连接选择器）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KindInfo {
    pub id: String,
    pub label: String,
    pub port: u16,
    pub category: String,
    pub protocol: String,
    pub driver: String,
    pub note: String,
    pub planned: bool,
}

/// 连接树中相对于另一连接的插入位置。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionMovePosition {
    Before,
    After,
}

/// 一个保存的连接配置（不含明文密码）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionConfig {
    pub id: String,
    pub name: String,
    pub kind: DbKind,
    pub host: String,
    pub port: u16,
    pub user: String,
    /// 加密后的密码（base64），空表示未设置。
    #[serde(default)]
    pub password: String,
    pub database: String,
    /// SQLite 专用：数据库文件路径。
    #[serde(default)]
    pub file: String,
    /// 额外参数，例如 MySQL 的 charset、PostgreSQL 的 sslmode。
    #[serde(default)]
    pub params: String,
    /// 所属分组名，空表示未分组。
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub keep_alive: bool,
}

impl ConnectionConfig {
    /// 展示名、分组、数组位置不影响连接池；其余连接字段改变后需重开。
    pub(crate) fn requires_reopen(&self, other: &Self) -> bool {
        self.kind != other.kind || self.host != other.host || self.port != other.port
            || self.user != other.user || self.password != other.password
            || self.database != other.database || self.file != other.file
            || self.params != other.params || self.read_only != other.read_only
            || self.keep_alive != other.keep_alive
    }
}

/// 前端发起连接测试时携带的临时配置（密码明文）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionRequest {
    pub kind: DbKind,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub database: String,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub params: String,
    #[serde(default)]
    pub timeout_secs: u64,
}

/// 测试连接结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub ok: bool,
    pub message: String,
    /// 服务器版本串，例如 "8.0.35" / "15.3"。
    #[serde(default)]
    pub version: String,
    /// 当前字符集 / 排序规则。
    #[serde(default)]
    pub charset: String,
}

/// 对象树节点类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObjectType {
    Database,
    Schema,
    Table,
    View,
    Column,
    Index,
    Procedure,
    Function,
    Trigger,
    Event,
    Sequence,
    Query,
    Field,
    Folder,
}

/// 对象树中的一个节点。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNode {
    /// 唯一标识，形如 "conn:xx/table:yy"。
    pub id: String,
    /// 显示文本。
    pub label: String,
    #[serde(rename = "type")]
    pub kind: ObjectType,
    /// 图标名（对应 public/icons 下的文件名）。
    pub icon: String,
    /// 该节点下的子节点。
    #[serde(default)]
    pub children: Vec<TreeNode>,
    /// 是否可以展开。
    #[serde(default)]
    pub expandable: bool,
    /// 加载子节点后是否仍然有子节点。
    #[serde(default)]
    pub has_children: bool,
    /// 是否已被展开。
    #[serde(default)]
    pub expanded: bool,
}

/// 字段（列）定义。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub data_type: String,
    pub nullable: bool,
    pub default: Option<String>,
    #[serde(default)]
    pub primary_key: bool,
    #[serde(default)]
    pub auto_increment: bool,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub length: Option<u32>,
    #[serde(default)]
    pub precision: Option<u32>,
    #[serde(default)]
    pub scale: Option<u32>,
}

/// 索引定义。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub method: Option<String>,
}

/// 外键定义。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForeignKeyInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub ref_table: String,
    /// 引用表的实际命名空间；缺省仅兼容旧同命名空间目录。
    #[serde(default)]
    pub ref_schema: Option<String>,
    pub ref_columns: Vec<String>,
    #[serde(default)]
    pub on_update: Option<String>,
    #[serde(default)]
    pub on_delete: Option<String>,
}

/// 表结构完整定义。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableSchema {
    pub table: String,
    pub database: String,
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<IndexInfo>,
    pub foreign_keys: Vec<ForeignKeyInfo>,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub engine: Option<String>,
    #[serde(default)]
    pub collation: Option<String>,
    #[serde(default)]
    pub row_count: Option<i64>,
}

/// 对象列表的一行（对象标签页网格）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub obj_type: ObjectType,
    #[serde(rename = "autoIncrement")]
    pub auto_increment: i64,
    #[serde(rename = "updateTime")]
    pub update_time: String,
    #[serde(rename = "dataLength")]
    pub data_length: i64,
    pub engine: String,
    pub rows: i64,
    pub comment: String,
}

/// 结果集的一列。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub data_type: String,
    #[serde(default)]
    pub nullable: bool,
    #[serde(default)]
    pub primary_key: bool,
    #[serde(default)]
    pub auto_increment: bool,
}

/// 结果集数据：列定义 + 行。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultSet {
    pub columns: Vec<ResultColumn>,
    pub rows: Vec<Vec<Option<String>>>,
    /// SQL 影响行数（INSERT/UPDATE/DELETE）。
    #[serde(default)]
    pub affected: u64,
    /// 执行耗时（毫秒）。
    #[serde(default)]
    pub elapsed_ms: u64,
    /// 查询是否成功。
    pub ok: bool,
    #[serde(default)]
    pub message: String,
    /// 是否是 SELECT 之类返回数据的结果。
    #[serde(default)]
    pub has_rows: bool,
    /// 表格主键列名（用于行内编辑）。
    #[serde(default)]
    pub primary_keys: Vec<String>,
    /// 结果集总数（用于多结果集标签）。
    #[serde(default)]
    pub total: u64,
}

/// 一次执行可能返回多个结果集。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiResult {
    pub sets: Vec<ResultSet>,
    /// 执行过程中产生的消息。
    #[serde(default)]
    pub messages: Vec<String>,
    /// 总耗时（毫秒）。
    #[serde(default)]
    pub elapsed_ms: u64,
    /// 本次执行完整结果的本地缓存标识；`sets[i]` 对应缓存里第 i 个结果集。
    /// 为空表示这次执行没有可用的完整快照（适配器不走原生流）。
    #[serde(default)]
    pub spool_id: Option<String>,
}

/// 数据分页请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageRequest {
    pub connection_id: String,
    pub schema: String,
    pub table: String,
    pub offset: u64,
    pub limit: u64,
    /// 排序列。
    #[serde(default)]
    pub order_by: Option<String>,
    #[serde(default)]
    pub ascending: bool,
    /// 筛选条件行（每行一个括号组）。
    #[serde(default)]
    pub filters: Vec<FilterLine>,
    /// 逐条 logic 缺省时的回退连接方式；旧请求（无逐条 logic）整组生效。
    #[serde(default)]
    pub filter_logic: FilterLogic,
    /// 数组顺序即排序优先级；Some([]) 清除排序，None 兼容旧字段。
    #[serde(default)]
    pub order_rules: Option<Vec<SortRule>>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum FilterLogic {
    #[default]
    And,
    Or,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SortRule {
    pub column: String,
    pub ascending: bool,
}

/// 筛选条件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterCondition {
    pub column: String,
    pub operator: String,
    pub value: String,
    /// 文本比较是否区分大小写（前端「大小写敏感」复选框）。
    #[serde(default)]
    pub case_sensitive: bool,
    /// 行内与上一条条件的连接方式（首条忽略）；缺省 AND（兼容旧请求）。
    #[serde(default)]
    pub logic: Option<FilterLogic>,
}

/// 一行筛选条件：行内多条时整行括起，行与行按 logic 并列连接，
/// 直接对应 SQL 书写形式 a = 1 AND (b = 2 OR c = 3) AND d = 4。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterLine {
    /// 与上一行的连接方式（首行忽略）；缺省 AND。
    #[serde(default)]
    pub logic: FilterLogic,
    /// 行内条件（至少一条）。
    #[serde(default)]
    pub conds: Vec<FilterCondition>,
}

/// 表格数据的分页响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageResult {
    pub columns: Vec<ResultColumn>,
    pub rows: Vec<Vec<Option<String>>>,
    pub total: u64,
    pub primary_keys: Vec<String>,
    #[serde(default)]
    pub elapsed_ms: u64,
}

/// 行修改请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowUpdate {
    pub connection_id: String,
    pub schema: String,
    pub table: String,
    /// 主键列 -> 主键值
    pub primary_keys: Vec<(String, Option<String>)>,
    /// 要更新的列 -> 新值（None 表示 NULL）
    pub changes: Vec<(String, Option<String>)>,
    /// 无主键表用于定位旧行的全列旧值（可选）
    #[serde(default)]
    pub old_values: Vec<(String, Option<String>)>,
}

/// 批量写入请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowInsert {
    pub connection_id: String,
    pub schema: String,
    pub table: String,
    pub columns: Vec<String>,
    pub values: Vec<Option<String>>,
}

/// 应用设置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub settings_version: u32,
    pub font_family: String,
    pub font_size: u32,
    pub grid_font_size: u32,
    pub rows_per_page: u32,
    pub query_timeout_secs: u64,
    pub connect_timeout_secs: u64,
    pub pool_acquire_timeout_secs: u64,
    pub sqlite_busy_timeout_secs: u64,
    pub theme: String,
    pub show_system_tray: bool,
    pub confirm_on_close: bool,
    pub auto_complete: bool,
    pub max_recent: u32,
    pub sql_font_family: String,
    pub sql_font_size: u32,
    #[serde(flatten)] pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            settings_version: 2,
            font_family: "Microsoft YaHei UI".into(),
            font_size: 12,
            grid_font_size: 12,
            rows_per_page: 1000,
            query_timeout_secs: 30,
            connect_timeout_secs: 30,
            pool_acquire_timeout_secs: 30,
            sqlite_busy_timeout_secs: 10,
            theme: "light".into(),
            show_system_tray: true,
            confirm_on_close: true,
            auto_complete: true,
            max_recent: 10,
            sql_font_family: "Consolas".into(),
            sql_font_size: 13,
            extra: Default::default(),
        }
    }
}

impl AppSettings {
    pub fn validate_execution_settings(&self) -> crate::error::CmdResult<()> {
        if self.settings_version > 2 { return Err(crate::error::AppError::coded("SETTINGS_VERSION_UNSUPPORTED", "设置来自更新版本，拒绝覆盖")); }
        if self.query_timeout_secs > 86400 || !(1..=86400).contains(&self.connect_timeout_secs)
            || !(1..=86400).contains(&self.pool_acquire_timeout_secs) || self.sqlite_busy_timeout_secs > 86400 {
            return Err(crate::error::AppError::coded("EXECUTION_BUDGET_INVALID", "查询/SQLite 锁预算必须为 0..86400 秒；连接与池等待必须为 1..86400 秒"));
        }
        Ok(())
    }
}

/// 一条“最近打开”记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentItem {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub timestamp: i64,
}

/// 一条收藏。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Favorite {
    pub id: String,
    pub name: String,
    /// 收藏的 SQL 内容。
    pub sql: String,
    #[serde(default)]
    pub connection_id: Option<String>,
    #[serde(default)]
    pub schema: Option<String>,
    pub timestamp: i64,
}

/// 导入向导选项。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportOptions {
    pub connection_id: String,
    pub schema: String,
    pub table: String,
    pub file: String,
    /// csv / sql / json
    pub format: String,
    /// 第一行是否包含列名。
    #[serde(default)]
    pub has_header: bool,
    #[serde(default)]
    pub delimiter: String,
    #[serde(default)]
    pub encoding: String,
    #[serde(default)]
    pub create_table: bool,
    #[serde(default)]
    pub truncate: bool,
    #[serde(default)]
    pub sheet: Option<String>,
    /// 记录冲突策略：error（默认，遇重复整批失败）| skip（跳过重复）| update（按键更新）。
    #[serde(default)]
    pub on_conflict: String,
    /// 冲突判定键（目标表列名）；为空时使用目标表主键。
    #[serde(default)]
    pub conflict_columns: Vec<String>,
}

/// 导出向导选项。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportOptions {
    pub connection_id: String,
    pub schema: String,
    pub table: String,
    pub file: String,
    /// csv / json / sql
    pub format: String,
    #[serde(default)]
    pub has_header: bool,
    #[serde(default)]
    pub delimiter: String,
    #[serde(default)]
    pub encoding: String,
    #[serde(default)]
    pub where_clause: Option<String>,
    #[serde(default)]
    pub limit: Option<u64>,
}

/// 数据传输（表复制）选项。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferOptions {
    pub source_connection: String,
    pub source_schema: String,
    pub source_table: String,
    pub target_connection: String,
    pub target_schema: String,
    pub target_table: String,
    #[serde(default)]
    pub create_target: bool,
    #[serde(default)]
    pub drop_existing: bool,
    #[serde(default)]
    pub copy_structure: bool,
    #[serde(default)]
    pub copy_data: bool,
    #[serde(default)]
    pub where_clause: Option<String>,
}

/// SQL 转储选项。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpOptions {
    pub connection_id: String,
    pub schema: String,
    pub table: String,
    pub file: String,
    #[serde(default)]
    pub structure_only: bool,
    #[serde(default)]
    pub data_only: bool,
    #[serde(default)]
    pub include_drop: bool,
}

/// 服务器信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub product: String,
    pub version: String,
    pub host: String,
    pub port: u16,
    pub charset: String,
    pub collation: String,
    pub uptime: Option<String>,
    pub extra: Vec<(String, String)>,
}

/// 备份文件信息（备份标签页网格行）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupInfo {
    /// 文件名（不含路径）。
    pub name: String,
    /// 修改时间（本地时区文本）。
    pub modified: String,
    /// 文件大小（字节）。
    pub size: u64,
    /// 备份注释（写在文件头）。
    pub comment: String,
}

/// 整库备份选项。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupOptions {
    pub connection_id: String,
    pub schema: String,
    #[serde(default)]
    pub comment: String,
    /// 要备份的表 / 视图名单；空表示全部。
    #[serde(default)]
    pub tables: Vec<String>,
    /// 要备份的函数 / 存储过程名单；空表示不包含例程。
    #[serde(default)]
    pub routines: Vec<String>,
    /// 备份文件名（不含扩展名 / 路径）；空由后端按时间戳生成。
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default = "default_true")]
    pub include_structure: bool,
    #[serde(default = "default_true")]
    pub include_data: bool,
}

fn default_true() -> bool {
    true
}

/// 自动运行任务。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleJob {
    pub id: String,
    pub name: String,
    pub connection_id: String,
    pub database: String,
    pub sql: String,
    /// 执行间隔（秒）；0 表示仅手动执行。
    pub interval_secs: u64,
    /// 每天的固定执行时刻（本地时间，HH:MM）；与 interval_secs 二选一。
    #[serde(default)]
    pub daily_at: Option<String>,
    /// 可视化 cron 定时（五字段：分 时 日 月 星期）；填写后优先于 interval_secs / daily_at。
    #[serde(default)]
    pub cron: Option<String>,
    /// 任务动作配置（备份 / 导出 / 导入 / 数据同步 / 数据生成）；
    /// 结构由前端定义，后端只透传持久化，执行由前端派发到对应工具接口。
    #[serde(default)]
    pub task: Option<serde_json::Value>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub last_run: Option<String>,
    #[serde(default)]
    pub last_status: Option<String>,
}

/// 一次自动运行记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobHistoryEntry {
    pub job_id: String,
    pub job_name: String,
    pub started_at: String,
    pub elapsed_ms: u64,
    pub ok: bool,
    pub detail: String,
}

/// 一条本工具执行的 SQL 记录（「历史日志」的 SQL 执行日志来源）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqlLogParameter {
    pub ordinal: usize,
    pub type_name: String,
    pub value: String,
    #[serde(default)] pub redacted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqlLogEntry {
    /// 本地时间 "YYYY-MM-DD HH:MM:SS"（按字符串字典序即可比较先后）。
    pub ts: String,
    pub connection_id: String,
    pub connection_name: String,
    /// 数据源类型（连接 kind）。
    pub kind: String,
    /// 库 / 模式；未指定时为空串。
    pub database: String,
    /// 执行时的完整 SQL 原文；保留换行、注释和字面量，供历史日志查看、搜索和复制。
    pub sql: String,
    /// Physical namespace, separate from the actual connected database/file.
    #[serde(default)] pub namespace: String,
    /// Bound parameters, not interpolated into SQL. Values use typed Debug representation.
    #[serde(default)] pub parameters: Vec<SqlLogParameter>,
    pub ok: bool,
    pub elapsed_ms: u64,
    /// 受影响行数或返回行数；-1 表示该语句没有行数信息。
    pub rows: i64,
    /// 失败原因（截断）；成功为空串。
    pub message: String,
    /// 调用来源：query（查询页）/ monitor（监控）/ tool（其它工具）。
    pub origin: String,
    #[serde(default)] pub operation_id: Option<String>,
    #[serde(default)] pub execution_state: Option<crate::execution_contracts::ExecutionState>,
}

/// 一条数据库会话 / 正在执行的查询（数据库监控会话页）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    /// 会话标识字符串（MySQL Id / PG pid / MSSQL session_id / ClickHouse query_id）。
    pub id: String,
    pub user: String,
    pub host: String,
    pub db: String,
    pub command: String,
    pub state: String,
    pub elapsed_secs: Option<i64>,
    pub sql: String,
    /// 是否为本工具自身连接的会话。
    pub is_self: bool,
}
