/** 前后端共用的类型定义（与 Rust 侧 models.rs 对应）。 */

/**
 * 数据库类型 id（目录化字符串，如 "mysql" / "dameng"）。
 * 全部可用类型见 dbcatalog.ts 的 DB_KINDS（与 Rust catalog.rs 同源生成）。
 */
export type DbKind = string;

export type ObjectType =
  | "connection"
  | "database"
  | "schema"
  | "table"
  | "view"
  | "column"
  | "index"
  | "procedure"
  | "function"
  | "trigger"
  | "event"
  | "sequence"
  | "query"
  | "field"
  | "folder";

export type ConnectionMovePosition = "before" | "after";

export interface ConnectionConfig {
  id: string;
  name: string;
  kind: DbKind;
  host: string;
  port: number;
  user: string;
  /** 加密后的密码，前端不展示。 */
  password: string;
  database: string;
  file: string;
  params: string;
  group: string;
  read_only: boolean;
  keep_alive: boolean;
}

export interface ConnectionRequest {
  kind: DbKind;
  host: string;
  port: number;
  user: string;
  password: string;
  database: string;
  file: string;
  params: string;
  timeout_secs: number;
}

export interface TestResult {
  ok: boolean;
  message: string;
  version: string;
  charset: string;
}

export interface TreeNode {
  id: string;
  label: string;
  type: ObjectType;
  icon: string;
  children: TreeNode[];
  expandable: boolean;
  has_children: boolean;
  expanded: boolean;
}

export interface ColumnInfo {
  name: string;
  type: string;
  nullable: boolean;
  default: string | null;
  primary_key: boolean;
  auto_increment: boolean;
  comment: string | null;
  length: number | null;
  precision: number | null;
  scale: number | null;
}

export interface IndexInfo {
  name: string;
  columns: string[];
  unique: boolean;
  primary: boolean;
  method: string | null;
}

export interface ForeignKeyInfo {
  name: string;
  columns: string[];
  ref_table: string;
  ref_columns: string[];
  on_update: string | null;
  on_delete: string | null;
}

export interface TableSchema {
  table: string;
  database: string;
  columns: ColumnInfo[];
  indexes: IndexInfo[];
  foreign_keys: ForeignKeyInfo[];
  comment: string | null;
  engine: string | null;
  collation: string | null;
  row_count: number | null;
}

export interface ResultColumn {
  name: string;
  type: string;
  nullable: boolean;
  primary_key: boolean;
  auto_increment: boolean;
}

export interface ResultSet {
  columns: ResultColumn[];
  rows: (string | null)[][];
  affected: number;
  elapsed_ms: number;
  ok: boolean;
  message: string;
  has_rows: boolean;
  primary_keys: string[];
  total: number;
}

export interface MultiResult {
  sets: ResultSet[];
  messages: string[];
  elapsed_ms: number;
}

export interface FilterCondition {
  column: string;
  operator: string;
  value: string;
  /** 文本比较是否区分大小写。 */
  case_sensitive?: boolean;
  /** 行内与上一条条件的连接方式；首条忽略，省略时回退 AND。 */
  logic?: FilterLogic;
}

/** 一行筛选条件。行内多条时整行括起（如同 SQL 中的括号组），
 *  行与行之间按 logic 并列连接，如：a=1 AND (b=2 OR c=3) AND d=4
 *  → 三行：[a=1]、[b=2, c=3(OR)]、[d=4]。 */
export interface FilterLine {
  /** 与上一行的连接方式；首行忽略，省略时回退 AND。 */
  logic?: FilterLogic;
  /** 行内条件（至少一条）。 */
  conds: FilterCondition[];
}

/** 逐条 logic 缺省时的回退连接方式；旧请求（无逐条 logic）整组生效。 */
export type FilterLogic = "AND" | "OR";

/** 数组顺序就是排序优先级（第一项优先）。 */
export interface SortRule {
  column: string;
  ascending: boolean;
}

export interface PageRequest {
  connection_id: string;
  schema: string;
  table: string;
  offset: number;
  limit: number;
  order_by: string | null;
  ascending: boolean;
  filters: FilterLine[];
  filter_logic?: FilterLogic;
  /** 提供时覆盖旧 order_by / ascending；空数组表示清除排序。 */
  order_rules?: SortRule[];
}

export interface PageResult {
  columns: ResultColumn[];
  rows: (string | null)[][];
  total: number;
  primary_keys: string[];
  elapsed_ms: number;
}

export interface RowUpdate {
  connection_id: string;
  schema: string;
  table: string;
  primary_keys: [string, string | null][];
  changes: [string, string | null][];
  /** 无主键表用于定位旧行的全列旧值 */
  old_values?: [string, string | null][];
}

export interface RowInsert {
  connection_id: string;
  schema: string;
  table: string;
  columns: string[];
  values: (string | null)[];
}

export interface AppSettings {
  font_family: string;
  font_size: number;
  grid_font_size: number;
  rows_per_page: number;
  query_timeout_secs: number;
  theme: string;
  show_system_tray: boolean;
  confirm_on_close: boolean;
  auto_complete: boolean;
  max_recent: number;
  sql_font_family: string;
  sql_font_size: number;
}

export interface RecentItem {
  id: string;
  label: string;
  kind: string;
  timestamp: number;
}

export interface Favorite {
  id: string;
  name: string;
  sql: string;
  connection_id: string | null;
  timestamp: number;
}

export interface ImportOptions {
  connection_id: string;
  schema: string;
  table: string;
  file: string;
  format: string;
  has_header: boolean;
  delimiter: string;
  encoding: string;
  create_table: boolean;
  truncate: boolean;
  sheet: string | null;
}

export interface ExportOptions {
  connection_id: string;
  schema: string;
  table: string;
  file: string;
  format: string;
  has_header: boolean;
  delimiter: string;
  encoding: string;
  where_clause: string | null;
  limit: number | null;
}

export interface TransferOptions {
  source_connection: string;
  source_schema: string;
  source_table: string;
  target_connection: string;
  target_schema: string;
  target_table: string;
  create_target: boolean;
  drop_existing: boolean;
  copy_structure: boolean;
  copy_data: boolean;
  where_clause: string | null;
}

export interface DumpOptions {
  connection_id: string;
  schema: string;
  table: string;
  file: string;
  structure_only: boolean;
  data_only: boolean;
  include_drop: boolean;
}

/** 备份文件信息（备份标签页网格行）。 */
export interface BackupInfo {
  name: string;
  modified: string;
  size: number;
  comment: string;
}

/** 整库备份选项。 */
export interface BackupOptions {
  connection_id: string;
  schema: string;
  comment: string;
  /** 要备份的表 / 视图名单；空表示全部。 */
  tables: string[];
  include_structure: boolean;
  include_data: boolean;
}

export interface ServerInfo {
  product: string;
  version: string;
  host: string;
  port: number;
  charset: string;
  collation: string;
  uptime: string | null;
  extra: [string, string][];
}

/** 标签页类型。 */
export type TabKind =
  | "objects"
  | "table"
  | "query"
  | "structure"
  | "result"
  | "datasync"
  | "backup"
  | "users"
  | "schedule"
  | "model"
  | "chart"
  | "viewdesign"
  | "routinedesign";

/** 标签页状态。 */
export interface TabState {
  id: string;
  kind: TabKind;
  title: string;
  connectionId: string;
  connectionName: string;
  schema: string;
  object: string;
  objectType: ObjectType;
  /** 查询页的 SQL 文本。 */
  sql: string;
  /** 查询页的多结果集。 */
  results: MultiResult | null;
  /** 当前显示的结果集索引。 */
  activeResult: number;
  /** 表数据页的分页状态。 */
  page: PageResult | null;
  pageOffset: number;
  pageLimit: number;
  orderBy: string | null;
  ascending: boolean;
  filters: FilterLine[];
  filter_logic?: FilterLogic;
  /** 提供时覆盖旧 order_by / ascending；空数组表示清除排序。 */
  order_rules?: SortRule[];
  /** 结构页的表结构。 */
  schemaInfo: TableSchema | null;
  /** 是否处于修改状态。 */
  dirty: boolean;
  /** 数据源同步改变端点后，旧页面禁止沿用旧上下文操作新数据库。 */
  datasourceStale?: string;
  /** 选中的行索引。 */
  selectedRows: Set<number>;
  /** 图表页保留创建时的内存快照与展示选项；关闭后不保存。 */
  chart?: import("./chart").ChartState;
  /** 正在编辑的单元格。 */
  editing: { row: number; col: number } | null;
}

/** 对象列表的一行（对象标签页网格）。 */
export interface ObjectInfo {
  name: string;
  type: string;
  autoIncrement: number;
  updateTime: string;
  dataLength: number;
  engine: string;
  rows: number;
  comment: string;
}

/** 左侧树节点（含运行时展开状态）。 */
export interface TreeItem extends TreeNode {
  loading?: boolean;
  loaded?: boolean;
  /** 连接节点：是否已打开（行尾绿点 + 图标着色）。 */
  connected?: boolean;
}

/** 自动运行任务。 */
export interface ScheduleJob {
  id: string;
  name: string;
  connection_id: string;
  database: string;
  sql: string;
  interval_secs: number;
  enabled: boolean;
  last_run: string | null;
  last_status: string | null;
}

/** 一次自动运行记录。 */
export interface JobHistoryEntry {
  job_id: string;
  job_name: string;
  started_at: string;
  elapsed_ms: number;
  ok: boolean;
  detail: string;
}

/** 一条数据库会话 / 正在执行的查询（数据库监控会话页）。 */
export interface ProcessInfo {
  id: string;
  user: string;
  host: string;
  db: string;
  command: string;
  state: string;
  elapsed_secs: number | null;
  sql: string;
  is_self: boolean;
}

/** 还原备份：单条语句的执行结果。 */
export interface RestoreStep {
  index: number;
  sql: string;
  ok: boolean;
  affected: number | null;
  error: string | null;
  skipped: boolean;
}

/** 还原备份：整份脚本的执行汇总。 */
export interface RestoreReport {
  total: number;
  succeeded: number;
  failed: number;
  steps: RestoreStep[];
}
