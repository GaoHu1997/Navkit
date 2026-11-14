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
  | "materialized_view"
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
  ref_schema?: string | null;
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

export interface TableFeatureCapability {
  supported: boolean;
  /** 不支持或受限的真实后端原因，不得用空元数据推断支持。 */
  reason: string | null;
}

export interface TableDesignCapabilities {
  checks: TableFeatureCapability;
  triggers: TableFeatureCapability;
  options: TableFeatureCapability;
  rebuild: TableFeatureCapability;
  clear: TableFeatureCapability;
  copy_structure: TableFeatureCapability;
  copy_data: TableFeatureCapability;
  supported_options: string[];
  limitations: string[];
}

export interface CheckDesign {
  name: string;
  orig_name?: string | null;
  /** 仅 CHECK 内的表达式，不是顶层 DDL。 */
  expression: string;
  /** 匿名既有 CHECK 的合成展示身份；不得当作真实可 DROP 的约束名。 */
  generated_name?: boolean;
}

export interface TriggerDesign {
  name: string;
  orig_name?: string | null;
  timing: string;
  event: string;
  /** SQLite/MySQL 为触发器 body；PG 为已存在函数的 EXECUTE FUNCTION 调用。 */
  statement: string;
  /** 只读完整原定义。未修改时原样保留，修改时只能由后端受限编译。 */
  definition?: string | null;
}

export interface TableOptionsDesign {
  engine?: string | null;
  charset?: string | null;
  collation?: string | null;
  row_format?: string | null;
  strict?: boolean | null;
  without_rowid?: boolean | null;
  fillfactor?: number | null;
}

export interface TableOperation {
  connection_id: string;
  schema: string;
  table: string;
  kind: "clear" | "copy_structure" | "copy_data";
  target_table?: string | null;
  /** 来自真实完整 snapshot；执行在物理连接锁内再次核验。 */
  base_revision: string;
  source_fingerprint: string;
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
  /** 本次执行的完整落盘快照；旧后端/历史结果可能没有。 */
  spool_id?: string | null;
}

export interface QuerySpoolScope {
  spoolId: string;
  connectionId: string;
  ownerId: string;
  namespace: string;
}

export interface QueryRowsPage {
  rows: (string | null)[][];
  total: number;
  truncated: boolean;
  message: string;
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
  /** false 只读取当前页，不在首屏等待全表计数；省略兼容旧调用。 */
  include_total?: boolean;
}

export interface PageResult {
  columns: ResultColumn[];
  rows: (string | null)[][];
  /** total_exact=false 时是已知下界，不能当作总行数或末页。 */
  total: number;
  total_exact?: boolean;
  has_more?: boolean | null;
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
  settings_version?: number;
  font_family: string;
  font_size: number;
  grid_font_size: number;
  rows_per_page: number;
  query_timeout_secs: number;
  connect_timeout_secs?: number;
  pool_acquire_timeout_secs?: number;
  sqlite_busy_timeout_secs?: number;
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
  schema?: string | null;
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
  /** 记录冲突策略：""（报错）| skip | update */
  on_conflict?: string;
  /** 冲突键列（目标表列名）；留空使用主键。 */
  conflict_columns?: string[];
}

export interface ImportTaskOptions extends ImportOptions {
  task_id: string;
  column_mapping: { source: string; target: string }[];
  write_strategy?: "row" | "pg_copy";
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

export interface ExportTaskOptions extends ExportOptions {
  task_id: string;
  /** 显式多行 SQL 恢复语义；0/1 或省略保持兼容单行。 */
  sql_batch_rows?: number;
  /** 单语句同时受目标编码字节及 UTF-8 缓冲硬限约束。 */
  sql_batch_bytes?: number;
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
  /** 要备份的函数 / 存储过程名单；空表示不包含例程。 */
  routines?: string[];
  /** 备份文件名（不含扩展名 / 路径）；空由后端按时间戳生成。 */
  file_name?: string | null;
  include_structure: boolean;
  include_data: boolean;
  /** 显式允许跳过无法无损读取的整表；默认关闭，跳过会出现在备份回执警告中。 */
  skip_incompatible_tables?: boolean;
  /** 缺省/compat保留旧路径；显式模式不支持时拒绝，不按兼容协议降级。 */
  source_consistency?: "compat" | "pg_snapshot" | "mysql_locked" | null;
}

/** 备份回执：数据行数、文件字节数、实际存储路径与跳过的表警告。 */
export interface BackupResult {
  rows: number;
  bytes: number;
  path: string;
  warnings: string[];
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
  savedQueryId?: string;
  /** SQL 设计器创建模式：首次保存绝不能先删除已有对象。 */
  designCreating?: boolean;
  /** 完整例程身份与打开时快照；保存不能重新读取旧基线。 */
  routineObject?: import("./object-catalog").DatabaseObjectId;
  routineSnapshot?: import("./object-catalog").RoutineSnapshot;
  /** 新建例程的真实不存在基线，不冒充已存在对象快照。 */
  routineCreation?: import("./routine-create").RoutineCreationBaseline;
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
  /** 正在编辑的单元格。 */
  editing: { row: number; col: number } | null;
  /** 表数据页视图模式：网格（缺省）/ 表单（每屏一条记录）。 */
  viewMode?: "grid" | "form";
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
  /** 不能从显示名或路径猜重载 / 触发器父表。 */
  objectDescriptor?: import("./object-catalog").ObjectDescriptor;
  /** 部分目录不能显示为完整空列表。 */
  catalogNotice?: string;
  loading?: boolean;
  loaded?: boolean;
  /** 连接节点：是否已打开（行尾绿点 + 图标着色）。 */
  connected?: boolean;
}

/** 自动任务的动作类型（定时执行的是数据同步 / 数据生成 / 备份 / 数据字典等任务，不是 SQL）。 */
export type ScheduleTaskKind =
  | "backup"
  | "export"
  | "import"
  | "data_sync"
  | "data_generation"
  | "data_dictionary";

/** 自动任务的动作配置：与对应工具对话框的选项结构一致，按 kind 解释。 */
export interface ScheduleTaskPayload {
  kind: ScheduleTaskKind;
  /** 各任务类型的配置；键与对应工具对话框提交的选项一致。 */
  options: Record<string, unknown>;
}

/** 自动任务。 */
export interface ScheduleJob {
  id: string;
  name: string;
  connection_id: string;
  database: string;
  /** 旧版 SQL 任务文本；新任务一律使用 task。 */
  sql: string;
  interval_secs: number;
  /** 每天的固定执行时刻（本地时间 HH:MM）；填写后忽略 interval_secs。 */
  daily_at?: string | null;
  /** 可视化 cron 定时（五字段：分 时 日 月 星期）；填写后优先于 interval_secs / daily_at。 */
  cron?: string | null;
  /** 任务动作（备份 / 导出 / 导入 / 数据同步 / 数据生成 / 数据字典）；旧版 SQL 任务为 null。 */
  task?: ScheduleTaskPayload | null;
  enabled: boolean;
  last_run: string | null;
  last_status: string | null;
}

/** 一次自动任务运行记录。 */
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
  effect: "acked" | "committed" | "rolled_back" | "skipped" | "failed" | "unknown";
}

/** 还原备份：计数独立于至多2048条步骤，succeeded仅表示已确认提交。 */
export interface RestoreReport {
  total: number;
  succeeded: number;
  failed: number;
  steps: RestoreStep[];
  acked: number;
  committed: number;
  rolled_back: number;
  skipped: number;
  unknown: number;
  attempted: number;
  first_failed_index: number | null;
  omitted_steps: number;
  effects_unknown: boolean;
  error: string | null;
  cleanup_error: string | null;
  warnings: string[];
}

/** 手动事务内的图形化行变更（表数据页事务路径保留；当前事务入口在查询页）。 */
export type ManualMutationOp =
  | { kind: "update"; table: string; changes: [string, string | null][]; keys: [string, string | null][]; old_values: [string, string | null][] }
  | { kind: "insert"; table: string; columns: string[]; values: (string | null)[] }
  | { kind: "delete"; table: string; keys: [string, string | null][]; old_values: [string, string | null][] };

// ---------------------------------------------------------------- 数据字典导出

/** 数据字典：列。 */
export interface DictionaryColumn {
  order: number;
  name: string;
  type_name: string;
  length: string;
  primary_key: boolean;
  auto_increment: boolean;
  nullable: boolean;
  default_value: string;
  comment: string;
}

/** 数据字典：表。 */
export interface DictionaryTable {
  name: string;
  comment: string;
  db_type: string;
  columns: DictionaryColumn[];
}

/** 数据字典：视图 / 存储过程。 */
export interface DictionaryObject {
  name: string;
  comment: string;
  script: string;
  db_type: string;
}

/** 一份数据库字典元数据（对应后端 data_dictionary::DictDatabase）。 */
export interface DictionaryDatabase {
  name: string;
  doc_title: string;
  tables: DictionaryTable[];
  views: DictionaryObject[];
  routines: DictionaryObject[];
}

/** 数据字典导出选项（对应后端 DictExportOptions）。 */
export interface DictionaryExportOptions {
  format: "markdown" | "html" | "excel" | "word" | "pdf" | string;
  excel_sheet_mode: "single" | "per_table" | string;
  include_tables: boolean;
  include_views: boolean;
  include_routines: boolean;
  created_at: string;
}

// ---------------------------------------------------------------- 软件更新

/** 更新清单（后端 updater::UpdateManifest）。 */
export interface UpdateManifest {
  version: string;
  notes: string;
  pub_date: string;
  url: string;
  /** 备用下载地址（镜像）。主地址失败时后端按序尝试。 */
  mirrors?: string[];
  sha256: string;
  size: number;
}

/** 检查更新结果。 */
export interface UpdateCheckResult {
  current_version: string;
  update_available: boolean;
  manifest: UpdateManifest | null;
}

/** 下载进度事件载荷（update-progress 事件）。 */
export interface UpdateProgressEvent {
  phase: "checking" | "downloading" | "verifying" | "ready" | "error" | string;
  done: number;
  total: number;
  speed: number;
  message: string;
}

/** 当前更新状态（update_status 轮询兜底）。 */
export interface UpdateStatus {
  phase: string;
  done: number;
  total: number;
  speed: number;
  message: string;
  staged_file: string | null;
}
