/** 与 Rust 后端的 IPC 桥接。 */

import { invoke } from "@tauri-apps/api/core";
import { DB_KINDS } from "./dbcatalog";
import type {
  AppSettings,
  ConnectionConfig,
  ConnectionRequest,
  DumpOptions,
  ExportOptions,
  Favorite,
  ImportOptions,
  MultiResult,
  ObjectType,
  PageRequest,
  PageResult,
  RecentItem,
  RowInsert,
  RowUpdate,
  ServerInfo,
  TestResult,
  TransferOptions,
} from "./types";

/** 调用后端命令；错误统一转成字符串。 */
async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  // 开发预览（preview.html / preview-bootstrap.js）注入的 mock 优先，
  // 未预置的命令回落真实后端；生产构建不存在 __PREVIEW_MOCK__。
  const mock = (window as unknown as { __PREVIEW_MOCK__?: Record<string, (a?: unknown) => unknown> })
    .__PREVIEW_MOCK__;
  if (mock && typeof mock[cmd] === "function") {
    return mock[cmd](args) as T;
  }
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw new Error(typeof e === "string" ? e : String(e));
  }
}

import type { DatasourceSyncSource, DatasourceSyncConfig, DatasourceSyncPreview, DatasourceSyncResult } from "./datasourceSync";

export const datasourceSyncApi: DatasourceSyncSource = {
  getConfig: () => call<DatasourceSyncConfig>("datasource_sync_get_config"),
  setConfig: config => call<DatasourceSyncConfig>("datasource_sync_set_config", { config }),
  test: () => call<string>("datasource_sync_test"),
  preview: () => call<DatasourceSyncPreview>("datasource_sync_preview"),
  commit: (planId, choices, automatic) => call<DatasourceSyncResult>("datasource_sync_commit", { planId, choices, automatic }),
  cancel: planId => call<void>("datasource_sync_cancel", { planId }),
};

export const api = {
  // 设置
  getSettings: () => call<AppSettings>("get_settings"),
  setSettings: (settings: AppSettings) => call<void>("set_settings", { settings }),

  // 连接
  listConnections: () => call<ConnectionConfig[]>("list_connections"),
  listGroups: () => call<string[]>("list_groups"),
  addGroup: (name: string) => call<string[]>("add_group", { name }),
  moveConnection: (id: string, group: string, targetId?: string, position?: import("./types").ConnectionMovePosition) =>
    call<ConnectionConfig[]>("move_connection", { id, group, targetId: targetId ?? null, position: position ?? null }),
  insertGeneratedRows: (req: import("./dataGenerator").GeneratedRowsRequest) => call<number>("insert_generated_rows", { req }),
  saveConnection: (config: ConnectionConfig, password?: string) =>
    call<ConnectionConfig[]>("save_connection", { config, password }),
  deleteConnection: (id: string) => call<ConnectionConfig[]>("delete_connection", { id }),
  renameGroup: (oldName: string, newName: string) =>
    call<ConnectionConfig[]>("rename_group", { oldName, newName }),
  testConnection: (req: ConnectionRequest) => call<TestResult>("test_connection", { req }),
  openConnection: (id: string) => call<TestResult>("open_connection", { id }),
  closeConnection: (id: string) => call<void>("close_connection", { id }),
  closeAllConnections: () => call<void>("close_all_connections"),

  // 元数据
  listDatabases: (id: string) => call<[string, string][]>("list_databases", { id }),
  listTables: (id: string, schemaName: string) =>
    call<[string, ObjectType][]>("list_tables", { id, schemaName }),
  listRoutines: (id: string, schemaName: string) =>
    call<[ObjectType, string][]>("list_routines", { id, schemaName }),
  listObjects: (id: string, schemaName: string) =>
    call<import("./types").ObjectInfo[]>("list_objects", { id, schemaName }),
  getTableSchema: (id: string, schemaName: string, table: string) =>
    call<import("./types").TableSchema>("get_table_schema", { id, schemaName, table }),

  // 数据
  pageTable: (req: PageRequest) => call<PageResult>("page_table", { req }),
  executeSql: (id: string, sql: string, database?: string, transactionOwner?: string, origin?: string) =>
    call<MultiResult>("execute_sql", { id, sql, database: database || null, ...(transactionOwner ? { transactionOwner } : {}), ...(origin ? { origin } : {}) }),
  startManualTransaction: (id: string, schema: string, owner: string) =>
    call<import("./manualTransaction").ManualTransactionStatus>("manual_transaction_start", { id, schema, owner }),
  manualTransactionStatus: (id: string) =>
    call<import("./manualTransaction").ManualTransactionStatus>("manual_transaction_status", { id }),
  finishManualTransaction: (id: string, owner: string, commit: boolean) =>
    call<import("./manualTransaction").ManualTransactionStatus>("manual_transaction_finish", { id, owner, commit }),
  updateRow: (req: RowUpdate) => call<number>("update_row", { req }),
  insertRow: (req: RowInsert) => call<number>("insert_row", { req }),
  deleteRow: (
    id: string,
    schemaName: string,
    table: string,
    primaryKeys: [string, string | null][],
  ) => call<number>("delete_row", { id, schemaName, table, primaryKeys }),
  executeDdl: (id: string, sql: string) => call<number>("execute_ddl", { id, sql }),

  // 收藏 / 最近
  listFavorites: () => call<Favorite[]>("list_favorites"),
  addFavorite: (favorite: Favorite) => call<Favorite[]>("add_favorite", { favorite }),
  removeFavorite: (id: string) => call<Favorite[]>("remove_favorite", { id }),
  listRecent: () => call<RecentItem[]>("list_recent"),
  pushRecent: (item: RecentItem) => call<RecentItem[]>("push_recent", { item }),
  clearRecent: () => call<RecentItem[]>("clear_recent"),

  // 导入导出
  runImport: (opts: ImportOptions) => call<number>("run_import", { opts }),
  runExport: (opts: ExportOptions) => call<number>("run_export", { opts }),
  exportQueryResult: (
    opts: ExportOptions,
    names: string[],
    rows: (string | null)[][],
  ) => call<number>("export_query_result", { opts, names, rows }),
  previewCsv: (path: string, delimiter: string, encoding: string, limit: number) =>
    call<[string[], string[][]]>("preview_csv", { path, delimiter, encoding, limit }),
  runTransfer: (opts: TransferOptions) => call<number>("run_transfer", { opts }),
  runDump: (opts: DumpOptions) => call<number>("run_dump", { opts }),

  // 备份 / 还原
  listBackups: (connectionId: string, schemaName: string) =>
    call<import("./types").BackupInfo[]>("list_backups", { connectionId, schemaName }),
  createBackup: (opts: import("./types").BackupOptions) =>
    call<[number, number]>("create_backup", { opts }),
  restoreBackup: (connectionId: string, schemaName: string, file: string) =>
    call<number>("restore_backup", { connectionId, schemaName, file }),
  /** 还原备份（详细版）：返回逐条语句的执行结果，供进度对话框展示。 */
  restoreBackupDetailed: (connectionId: string, schemaName: string, file: string) =>
    call<import("./types").RestoreReport>("restore_backup_detailed", {
      connectionId,
      schemaName,
      file,
    }),
  deleteBackup: (connectionId: string, schemaName: string, file: string) =>
    call<void>("delete_backup", { connectionId, schemaName, file }),
  openBackupFolder: (connectionId: string, schemaName: string) =>
    call<string>("open_backup_folder", { connectionId, schemaName }),
  listDbUsers: (id: string) => call<string[][]>("list_db_users", { id }),
  createDbUser: (id: string, user: string, host: string, password: string) =>
    call<void>("create_db_user", { id, user, host, password }),
  dropDbUser: (id: string, user: string, host: string) =>
    call<void>("drop_db_user", { id, user, host }),
  alterDbUserPassword: (id: string, user: string, host: string, password: string) =>
    call<void>("alter_db_user_password", { id, user, host, password }),
  getViewDefinition: (id: string, schemaName: string, view: string) =>
    call<string>("get_view_definition", { id, schemaName, view }),
  getRoutineDefinition: (id: string, schemaName: string, name: string, routineType: string) =>
    call<string>("get_routine_definition", { id, schemaName, name, routineType }),
  tableStatus: (id: string, schemaName: string, table: string) =>
    call<[string, string][]>("table_status", { id, schemaName, table }),
  getTableDdl: (id: string, schemaName: string, table: string) =>
    call<string>("get_table_ddl", { id, schemaName, table }),
  listJobs: () => call<import("./types").ScheduleJob[]>("list_jobs"),
  saveJob: (job: import("./types").ScheduleJob) =>
    call<import("./types").ScheduleJob[]>("save_job", { job }),
  deleteJob: (id: string) => call<import("./types").ScheduleJob[]>("delete_job", { id }),
  listJobHistory: () => call<import("./types").JobHistoryEntry[]>("list_job_history"),
  runJobNow: (id: string) => call<[number, string]>("run_job_now", { id }),

  // 工具
  serverInfo: (id: string) => call<ServerInfo>("server_info", { id }),
  completionData: (id: string, schemaName: string) =>
    call<Record<string, string[]>>("completion_data", { id, schemaName }),
  readLog: () => call<string[]>("read_log"),
  listSqlLog: (limit?: number) => call<SqlLogEntry[]>("list_sql_log", { limit: limit ?? null }),
  clearSqlLog: () => call<void>("clear_sql_log"),
  listDbProcesses: (id: string, database?: string) =>
    call<import("./types").ProcessInfo[]>("list_db_processes", { id, database: database || null }),
  killDbProcess: (id: string, processId: string, database?: string) =>
    call<string>("kill_db_process", { id, processId, database: database || null }),
  readSqlFile: (path: string) => call<string>("read_sql_file", { path }),
  ensureConnection: (id: string) => call<TestResult>("ensure_connection", { id }),
  openDatabase: (id: string, database: string) => call<TestResult>("open_database", { id, database }),
  aboutInfo: () => call<Record<string, string>>("about_info"),
};

/** 每种数据库的默认端口与图标（由 dbcatalog.ts 目录派生）。 */
export const DB_META: Record<string, { label: string; port: number; icon: string }> =
  Object.fromEntries(
    DB_KINDS.map((k) => [
      k.id,
      { label: k.label, port: k.port, icon: `info_pane_conn_${k.id}` },
    ]),
  );

/** User-defined drivers: native ODBC or JDBC (.jar + Java 11+). Templates never persist literal passwords. */
export type DriverSource = "installed" | "file" | "dsn";
/** 驱动协议：ODBC（原生库/DSN）或 JDBC（本地 .jar）。 */
export type DriverProtocol = "odbc" | "jdbc";
export interface DriverDefinition {
  id: string;
  name: string;
  protocol: DriverProtocol;
  source: DriverSource;
  target: string;
  template: string;
  driver_class: string;
  test_sql: string;
}
export interface DriverConnectionParams {
  version: 1;
  driver_id: string;
  template: string;
}
export interface OdbcInventory {
  drivers: { name: string; library: string }[];
  dsns: { name: string; driver: string }[];
  architecture: string;
}
export const DRIVER_PARAMS_PREFIX = "@navkit-driver:";
export function parseDriverParams(params: string): DriverConnectionParams | null {
  if (!params.startsWith(DRIVER_PARAMS_PREFIX)) return null;
  try {
    const value = JSON.parse(params.slice(DRIVER_PARAMS_PREFIX.length));
    return value.version === 1 && typeof value.driver_id === "string" ? value : null;
  } catch { return null; }
}
export function encodeDriverParams(driverId: string, template = ""): string {
  return DRIVER_PARAMS_PREFIX + JSON.stringify({ version: 1, driver_id: driverId, template });
}
export type DatabaseDriverState = "builtin" | "ready" | "missing" | "manual" | "unsupported";
export interface DatabaseDriverStatus {
  kind: string;
  /** 托管驱动版本；空串表示该状态行没有版本维度（内置协议 / ODBC 安装检测）。 */
  version: string;
  status: DatabaseDriverState;
  architecture: string;
  message: string;
  can_download: boolean;
  library: string | null;
  download_path: string | null;
}
/** 托管驱动版本选择（连接参数前缀 @navkit-driver-version: 的 JSON）。 */
export interface DriverVersionSelection { v: number; kind: string; version: string; extra: string }
export const DRIVER_VERSION_PARAMS_PREFIX = "@navkit-driver-version:";
export function parseVersionSelection(params: string): DriverVersionSelection | null {
  if (!params.startsWith(DRIVER_VERSION_PARAMS_PREFIX)) return null;
  try {
    const value = JSON.parse(params.slice(DRIVER_VERSION_PARAMS_PREFIX.length));
    return value.v === 1 && typeof value.kind === "string" && typeof value.version === "string" ? { ...value, extra: typeof value.extra === "string" ? value.extra : "" } : null;
  } catch { return null; }
}
export function encodeVersionParams(kind: string, version: string, extra = ""): string {
  return DRIVER_VERSION_PARAMS_PREFIX + JSON.stringify({ v: 1, kind, version, extra });
}
export const driverApi = {
  statuses: () => call<DatabaseDriverStatus[]>("list_driver_statuses"),
  download: (kind: string, version?: string) => call<DatabaseDriverStatus>("download_database_driver", { kind, version: version ?? null }),
  deleteManaged: (kind: string, version: string) => call<void>("delete_managed_driver", { kind, version }),
  list: () => call<DriverDefinition[]>("list_custom_drivers"),
  save: (driver: DriverDefinition) => call<DriverDefinition[]>("save_custom_driver", { driver }),
  remove: (id: string) => call<DriverDefinition[]>("delete_custom_driver", { id }),
  inventory: () => call<OdbcInventory>("list_odbc_inventory"),
  test: (driver: DriverDefinition, req: ConnectionRequest) => call<TestResult>("test_custom_driver", { driver, req }),
  testConnection: (req: ConnectionRequest, savedConnectionId?: string, useSavedPassword = false) =>
    call<TestResult>("test_connection_with_saved_password", { req, savedConnectionId: savedConnectionId ?? null, useSavedPassword }),
};

/** Focused synchronization and cancellable transfer APIs (append-only). */
export interface SyncColumnMapping { source: string; target: string; target_type: string | null }
export interface SyncOptions {
  task_id: string | null;
  source_connection: string; source_schema: string; source_table: string;
  target_connection: string; target_schema: string; target_table: string;
  mode: "structure" | "data" | "both";
  write_mode: "append" | "replace" | "recreate";
  /** full（默认）| incremental（按主键只插入目标缺失的行） */
  sync_mode: "full" | "incremental";
  /** 增量时是否同时更新主键相同但内容不同的行（归一化近似比较） */
  incremental_update: boolean;
  create_target: boolean; columns: SyncColumnMapping[]; where_clause: string | null;
}
export interface SyncReport { rows: number; created: boolean; warnings: string[] }
/** 整库同步请求：逐表复用表对表同步逻辑。 */
export interface DbSyncOptions {
  task_id: string | null;
  source_connection: string; source_schema: string;
  target_connection: string; target_schema: string;
  mode: "structure" | "both";
  /** 目标已有同名表的处理：跳过 / 追加 / 清空后写入 / 重建 */
  write_mode: "skip" | "append" | "replace" | "recreate";
  /** full（默认）| incremental（逐表按主键只插入缺失行） */
  sync_mode: "full" | "incremental";
  incremental_update: boolean;
  tables: { source: string; target: string; view?: boolean }[];
  where_clause: string | null;
}
export interface SyncTableReport {
  source_table: string; target_table: string;
  rows: number; created: boolean; skipped: boolean;
  error: string | null; warnings: string[];
}
/** 一条本工具执行的 SQL 记录（SQL 执行日志）。 */
export interface SqlLogEntry {
  ts: string; connection_id: string; connection_name: string; kind: string;
  database: string; sql: string; ok: boolean; elapsed_ms: number; rows: number;
  message: string; origin: string;
}
export interface TaskProgress {
  task_id: string; status: "running" | "completed" | "failed" | "cancelled";
  phase: string; processed: number; total: number; message: string;
}
export const transferTasks = {
  import: (opts: ImportOptions & { task_id: string; column_mapping: { source: string; target: string }[] }) => call<number>("run_import_task", { opts }),
  export: (opts: ExportOptions & { task_id: string }) => call<number>("run_export_task", { opts }),
  progress: (taskId: string) => call<TaskProgress>("transfer_task_progress", { taskId }),
  cancel: (taskId: string) => call<void>("cancel_transfer_task", { taskId }),
};
/** 源表与目标表的差异报告（只读比较）。 */
export interface SyncDiffReport {
  source_rows: number;
  target_rows: number;
  /** 是否按主键进行了行级比较（源表无主键时为 false）。 */
  keyed: boolean;
  missing_in_target: number;
  extra_in_target: number;
  /** 主键相同但内容不同的行数（归一化近似比较）。 */
  changed: number;
  changed_samples: string[];
  target_missing_columns: string[];
  target_extra_columns: string[];
  message: string | null;
}
export const syncApi = {
  run: (opts: SyncOptions) => call<SyncReport>("run_sync", { opts }),
  runDatabase: (req: DbSyncOptions) => call<SyncTableReport[]>("run_sync_database", { req }),
  diff: (req: { source_connection: string; source_schema: string; source_table: string; target_connection: string; target_schema: string; target_table: string; where_clause: string | null }) =>
    call<SyncDiffReport>("sync_diff", req),
  typeMapping: (sourceConnection: string, sourceSchema: string, sourceTable: string, targetConnection: string) =>
    call<{ source: string; source_type: string; target_type: string }[]>("sync_type_mapping", { sourceConnection, sourceSchema, sourceTable, targetConnection }),
};
