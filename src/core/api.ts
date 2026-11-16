/** 与 Rust 后端的 IPC 桥接。 */

import { invoke } from "@tauri-apps/api/core";
import { DB_KINDS } from "./dbcatalog";
import type { DatabaseObjectId, ObjectCatalogScope, ObjectCatalogPage, ObjectDefinition, RoutineReplacementDraft, RoutineReplacementPreview, RoutineSnapshot as ExactRoutineSnapshot } from "./object-catalog";
import type { RoutineCreationBaseline, RoutineCreationDraft, RoutineCreationPreview, RoutineCreationResponse } from "./routine-create";
import type { RoutineCallDraft, RoutineCallMetadata, RoutineCallPreview, RoutineCallResponse } from "./routine-call";
import type { RuntimeProfileSubmission, RuntimeProfileView } from "./runtime-profile";
import type { DbUserApplyResult, DbUserDetail, DbUserDraft, DbUserPlan } from "./db-user";
import type { SqlCompletionData } from "./sqlCompletion";
import type { ScheduleSpec, ScheduleV2Bindings, ScheduledJobRecord, ScheduledJobV2, SchedulerNextRun, SchedulerRunSummary, SchedulerApprovalPreview, SchedulerTemplate, SchedulerTemplateSummary } from "./scheduler";
import { asIpcError, type ExecutionRequest, type ExecutionOutcome, type ExecutionContext, type ExecutionStatus, type CancelAcknowledgement, type Capabilities } from "./execution";
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

/** 调用后端命令；保留可诊断的结构化错误，同时兼容旧字符串错误。 */
async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  // 仅浏览器静态预览消费显式 fixture；桌面存在真实 Tauri bridge 时始终调用真实 IPC。
  // preview-bootstrap 注入的最小 internals 带 __previewShim 标记：这类 shim 不是真实 bridge，
  // mock 仍然生效；生产/dev exe 的 preview.html 使用真实 bridge（无标记）→ mock 禁用（BUG-R1-001）。
  const host = window as unknown as { __TAURI_INTERNALS__?: { __previewShim?: unknown }; __PREVIEW_MOCK__?: Record<string, (a?: unknown) => unknown> };
  const internals = host.__TAURI_INTERNALS__;
  const realBridge = Boolean(internals) && !internals?.__previewShim;
  const mock = realBridge ? undefined : host.__PREVIEW_MOCK__;
  if (mock && typeof mock[cmd] === "function") {
    try { return await mock[cmd](args) as T; }
    catch (error) { throw asIpcError(error, cmd); }
  }
  // 仅兼容已显式注入的旧静态预览场景。生产没有此 fixture，始终 invoke Rust 服务。
  if (mock && typeof mock.execute_sql === "function" && cmd === "execution_capabilities") {
    const unsupported = { level: "unsupported", reason: "静态预览没有真实数据库控制通道" };
    return { version: 1, protocol: "preview", read_only: false,
      arbitrary_sql: { level: "limited", reason: "复用显式预览数据" }, cancel: unsupported, deadline: unsupported, manual_transaction: unsupported, safe_browse: unsupported } as T;
  }
  if (mock && typeof mock.execute_sql === "function" && cmd === "issue_execution_grant") {
    // Fixture-only handshake: desktop bypasses this branch and must obtain a real signed grant.
    const request = args?.request as ExecutionRequest;
    return { ...request, execution_grant: "preview_fixture" } as T;
  }
  if (mock && typeof mock.execute_sql === "function" && cmd === "execute_sql_v2") {
    const request = args?.request as ExecutionRequest;
    const deadline = { execution_id: request.execution_id, budget_ms: null, elapsed_ms: 0, deadline_reached: false, stop_confirmed: false };
    try {
      const result = await mock.execute_sql({ id: request.connection_id, sql: request.sql, database: request.namespace || null, transactionOwner: request.transaction_owner || null, origin: request.origin || "query" });
      return { execution_id: request.execution_id, state: "succeeded", result, error: null, deadline, effects: "preview_fixture", transaction_state: "none" } as T;
    } catch (error) {
      const e = asIpcError(error, cmd);
      return { execution_id: request.execution_id, state: "failed", result: null, error: { message: e.message, code: e.code, operation_id: e.operation_id, retryable: e.retryable }, deadline, effects: "preview_fixture", transaction_state: "none" } as T;
    }
  }
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw asIpcError(e, cmd);
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

export type { CheckDesign, TriggerDesign, TableOptionsDesign, TableOperation, TableDesignCapabilities } from "./types";

export interface TableDesignColumn {
  name: string;
  orig_name?: string | null;
  data_type: string;
  length?: string | null;
  scale?: string | null;
  nullable: boolean;
  default_value?: string | null;
  auto_increment?: boolean;
  comment?: string | null;
  unsigned?: boolean;
  zerofill?: boolean;
}

/** 表结构保存的目标状态；后端负责方言编译与执行。 */
/** 目标索引定义（不含主键索引）。 */
export interface IndexDesign {
  name: string;
  columns: string[];
  unique: boolean;
  /** 索引方法（BTREE/HASH…）；缺省由后端按方言选择。 */
  method?: string | null;
  /** columns 中需要降序的列名；未列出的列按升序。 */
  descending?: string[];
}

/** 目标外键定义。 */
export interface ForeignKeyDesign {
  name: string;
  columns: string[];
  ref_table: string;
  ref_schema?: string | null;
  ref_columns: string[];
  on_update?: string | null;
  on_delete?: string | null;
}

export interface TableDesignPayload {
  schema: string;
  table: string;
  columns: TableDesignColumn[];
  primary_key: string[];
  /** 缺省表示不修改表注释。 */
  comment?: string | null;
  /** 缺省表示不修改索引。 */
  indexes?: IndexDesign[];
  /** 缺省表示不修改外键。 */
  foreign_keys?: ForeignKeyDesign[];
  /** undefined 保留；[] 显式清空。 */
  checks?: import("./types").CheckDesign[];
  triggers?: import("./types").TriggerDesign[];
  /** 只提交 capabilities.supported_options 中的真实选项；undefined 保留。 */
  options?: import("./types").TableOptionsDesign;
  base_revision?: string;
  source_fingerprint?: string;
}

/** 检查约束。 */
export interface ConstraintInfo {
  name: string;
  definition: string;
  generated_name?: boolean;
}

/** 触发器定义。 */
export interface TriggerInfo {
  name: string;
  timing: string | null;
  event: string | null;
  statement: string;
  definition?: string | null;
}

export interface ConstraintSet {
  checks: ConstraintInfo[];
  triggers: TriggerInfo[];
  /** 缺省/unknown 不得视为完整空集合。 */
  completeness?: "complete" | "partial" | "unknown";
  limitations?: string[];
  table_definition?: string | null;
}

/** 完整真实设计基线，design 已归一化可直接用作编辑器全页草稿。 */
export interface TableDesignSnapshot {
  schema: import("./types").TableSchema;
  objects: ConstraintSet;
  options: import("./types").TableOptionsDesign;
  design: TableDesignPayload;
  revision: string;
  source_fingerprint: string;
  owner_id: string;
  completeness: "complete" | "partial" | "unknown";
  capabilities: import("./types").TableDesignCapabilities;
}

/** 表设计器「选项」页的真实候选项，来自当前数据库的信息模式。 */
export interface TableOptionChoices {
  engines: string[];
  charsets: string[];
  collations: string[];
  row_formats: string[];
}

export interface TableDesignPreview {
  request_sql: string;
  steps: { label: string; sql: string }[];
  destructive: boolean;
  revision: string;
  capabilities: import("./types").TableDesignCapabilities;
  limitations: string[];
}

export interface TableDesignReceipt extends DesignReceipt {
  snapshot: TableDesignSnapshot | null;
}

/** 唯有 outcome succeeded + receipt verified && !unverified && 无 mismatch 才能展示成功。 */
export interface TableDesignResponse {
  outcome: ExecutionOutcome;
  receipt: TableDesignReceipt | null;
}

export interface TableOperationPreview extends TableDesignPreview {
  operation: import("./types").TableOperation;
}

export interface TableOperationReceipt {
  kind: import("./types").TableOperation["kind"];
  source_table: string;
  target_table: string | null;
  affected_rows: number;
  copied_rows: number;
  verified: boolean;
  mismatches: string[];
  transaction: "committed" | "rolled_back" | "implicit" | "none" | "unknown";
  steps: DesignStepResult[];
  snapshot: TableDesignSnapshot | null;
}

export interface TableOperationResponse {
  outcome: ExecutionOutcome;
  receipt: TableOperationReceipt | null;
}

export interface RoutineObjectId {
  connection_id: string;
  schema: string;
  name: string;
  kind: string;
  identity_arguments?: string;
}

export interface RoutineSnapshot {
  object: RoutineObjectId;
  definition: string;
  revision: string;
  overloads: number;
  completeness: string;
}

export interface RoutineStepResult {
  label: string;
  sql: string;
  status: "applied" | "failed" | "skipped";
  error?: string | null;
}

export interface RoutineReceipt {
  steps: RoutineStepResult[];
  verified: boolean;
  mismatches: string[];
  strategy: string;
  transaction: string;
  revision: string;
  definition: string;
}

export interface DesignStepResult {
  label: string;
  sql: string;
  status: "applied" | "failed" | "skipped";
  error?: string;
}

export interface DesignReceipt {
  steps: DesignStepResult[];
  verified: boolean;
  unverified: boolean;
  mismatches: string[];
  transaction: "committed" | "rolled_back" | "implicit" | "none" | "unknown";
  schema: import("./types").TableSchema | null;
}

/** 类型化绑定值；与后端 mutation::DbValue 对应。 */
export type DbValue =
  | { kind: "null" }
  | { kind: "text"; value: string }
  | { kind: "int"; value: number }
  | { kind: "integer" | "decimal" | "binary" | "date" | "time" | "timestamp" | "timestamptz" | "uuid" | "json"; value: string }
  | { kind: "float"; value: number }
  | { kind: "bool"; value: boolean };

export interface MutationDraft {
  connection_id: string;
  owner_id?: string;
  schema: string;
  table: string;
  kind: "insert" | "update" | "delete";
  changes?: [string, DbValue][];
  keys?: [string, DbValue][];
  original_values?: [string, DbValue][];
}

export interface MutationPlan {
  plan_id: string;
  sql: string;
  binds: DbValue[];
  base_revision: string;
  expected_rows: number;
  summary: string;
}

export interface MutationReceipt {
  plan_id: string;
  status: "applied" | "unchanged" | "conflict" | "failed";
  transaction: "committed" | "rolled_back" | "none" | "unknown";
  affected: number;
  verified: boolean;
  mismatches: string[];
  sql: string;
}

export interface ConnectionHealthView {
  connection_id: string;
  state: "unknown" | "healthy" | "suspect" | "offline";
  last_checked_at?: string | null;
  last_error?: string | null;
  probe_skipped_reason?: string | null;
  interval_secs: number;
  timeout_secs: number;
  enabled: boolean;
  generation: string | null;
  resources_pending: boolean;
  next_due_in_ms: number | null;
}

export const schedulerApi: ScheduleV2Bindings = {
  listJobs: () => call<ScheduledJobRecord[]>("scheduler_list_jobs"),
  saveJob: (job: ScheduledJobV2, expectedRevision: string | null) => call<ScheduledJobRecord>("scheduler_save_job", { job, expectedRevision }),
  deleteJob: (id: string, revision: string) => call<void>("scheduler_delete_job", { id, revision }),
  listTemplates: () => call<SchedulerTemplateSummary[]>("scheduler_list_templates"),
  loadTemplate: (id: string) => call<SchedulerTemplate>("scheduler_load_template", { id }),
  saveTemplate: (template: SchedulerTemplate, expectedRevision: string | null) => call<SchedulerTemplate>("scheduler_save_template", { template, expectedRevision }),
  previewTimes: (schedule: ScheduleSpec) => call<SchedulerNextRun[]>("scheduler_preview_times", { schedule }),
  prepareApproval: (id: string, revision: string) => call<SchedulerApprovalPreview>("scheduler_prepare_approval", { id, revision }),
  approveJob: (planId: string, reviewDigest: string) => call<ScheduledJobRecord>("scheduler_approve_job", { planId, reviewDigest }),
  runNow: (id: string, revision: string, requestId: string) => call<SchedulerRunSummary | null>("scheduler_run_now", { id, revision, requestId }),
  history: (jobId: string) => call<SchedulerRunSummary[]>("scheduler_history", { jobId }),
  prepareLegacyMigration: (timezone: string) => call<import("./scheduler").SchedulerLegacyPreview>("scheduler_prepare_legacy_migration", { timezone }),
  confirmLegacyMigration: (planId: string, reviewDigest: string) => call<import("./scheduler").SchedulerLegacyReceipt>("scheduler_confirm_legacy_migration", { planId, reviewDigest }),
  cancelRun: (runId: string) => call<SchedulerRunSummary>("scheduler_cancel_run", { runId }),
};

/** 受控长批任务契约：payload 仅在注册时提交，运行消费一次性票据。 */
export type BatchTaskKind = "generated_rows" | "generated_recipe" | "sql_file";
export type BatchFileEncoding = "utf-8" | "gbk" | "gb18030" | "big5" | "latin1";
export type BatchTask =
  | { kind: "generated_rows"; request: import("./dataGenerator").GeneratedRowsRequest }
  | { kind: "generated_recipe"; recipe: import("./dataGenerator").GeneratedRecipe }
  | { kind: "sql_file"; path: string; encoding?: BatchFileEncoding; stop_on_error: boolean };
export interface BatchExecutionRequest {
  connection_id: string; namespace: string; owner_id: string;
  timeout_ms?: number | null; task: BatchTask;
}
export interface BatchExecutionTicket extends ExecutionContext {
  task_kind: BatchTaskKind; execution_grant: string; expires_in_ms: number;
  summary: string; total_rows: number | null; source_bytes: number | null;
}
export interface BatchExecutionMetrics { scanned: number; executed: number; errors: number; elapsed_ms: number; generated?: number; sent?: number; acknowledged?: number; committed?: number }
export interface BatchExecutionLog {
  sequence: number; level: "info" | "success" | "error";
  phase: "queued" | "scan" | "execute" | "commit" | "rollback" | "finish";
  statement: number | null; row: number | null; message: string; sql_template: string | null;
  /** 后端回执时间（epoch ms）。旧后端缺省时视图按到达时间兜底，只影响显示。 */
  ts_ms?: number;
  /** 单条语句的真实执行耗时（ms）；非执行阶段缺省。 */
  elapsed_ms?: number;
  /** 该语句影响/返回的行数；非执行阶段缺省。 */
  affected_rows?: number;
  /** 数据库错误码（如 1061）；仅失败回执存在。 */
  error_code?: string | null;
}
export interface BatchExecutionSnapshot {
  context: ExecutionContext; kind: BatchTaskKind; status: ExecutionStatus;
  metrics: BatchExecutionMetrics; scan_complete: boolean; parse_error: string | null;
  affected_rows: number; total_rows: number | null;
  transaction_state: "none" | "active" | "committed" | "rolled_back" | "unknown";
  logs: BatchExecutionLog[]; next_sequence: number; dropped_before_sequence: number;
  outcome: ExecutionOutcome | null;
}

export interface DiagramMetadata {
  tables: import("./types").TableSchema[];
  failures: { table: string; error: string }[];
  limitations: string[];
}

export const api = {
  readObjectMutationBaseline: (object: DatabaseObjectId, ownerId: string) => call<import("./object-mutation").ObjectMutationBaseline>("read_object_mutation_baseline", { object, ownerId }),
  previewObjectMutation: (draft: import("./object-mutation").ObjectMutationDraft, ownerId: string) => call<import("./object-mutation").ObjectMutationPreview>("preview_object_mutation", { draft, ownerId }),
  executeObjectMutation: (request: ExecutionRequest, work: import("./object-mutation").ObjectMutationWork) => call<import("./object-mutation").ObjectMutationResponse>("execute_object_mutation", { request, work }),
  previewGeneratedRecipe: (recipe: import("./dataGenerator").GeneratedRecipe, ownerId: string) => call<import("./dataGenerator").GeneratedRecipePreview>("preview_generated_recipe", { recipe, ownerId }),
  // BEGIN controlled batch API — 与设计/导出接口块独立。
  registerBatchExecution: (request: BatchExecutionRequest) => call<BatchExecutionTicket>("register_batch_execution", { request }),
  runBatchExecution: (ticket: BatchExecutionTicket) => call<BatchExecutionSnapshot>("run_batch_execution", { ticket }),
  batchExecutionStatus: (context: ExecutionContext, afterSequence = 0) => call<BatchExecutionSnapshot>("batch_execution_status", { context, afterSequence }),
  // END controlled batch API — 停止复用 cancelExecution；后台保留 context，绝不重发。
  checkConnectionHealth: (id: string, force?: boolean) => call<ConnectionHealthView>("check_connection_health", { id, force: force ?? null }),
  connectionHealth: (id: string) => call<ConnectionHealthView>("get_connection_health", { id }),
  issueGrant: (request: ExecutionRequest) => call<ExecutionRequest>("issue_execution_grant", { request }),
  executeSqlV2: (request: ExecutionRequest) => call<ExecutionOutcome>("execute_sql_v2", { request }),
  cancelExecution: (context: ExecutionContext) => call<CancelAcknowledgement>("cancel_execution", { executionId: context.execution_id, connectionId: context.connection_id, ownerId: context.owner_id, namespace: context.namespace }),
  executionStatus: (context: ExecutionContext) => call<ExecutionStatus>("execution_status", { executionId: context.execution_id, connectionId: context.connection_id, ownerId: context.owner_id, namespace: context.namespace }),
  executionCapabilities: (connectionId: string) => call<Capabilities>("execution_capabilities", { connectionId }),
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
  runtimeProfileView: (id: string) => call<RuntimeProfileView | null>("runtime_profile_view", { id }),
  testRuntimeConnection: (req: ConnectionRequest, id: string, submission: RuntimeProfileSubmission, preserveDatabasePassword: boolean) =>
    call<TestResult>("test_runtime_connection", { req, id, submission, preserveDatabasePassword }),
  saveConnection: (config: ConnectionConfig, password?: string, runtimeProfile?: RuntimeProfileSubmission) =>
    call<ConnectionConfig[]>("save_connection", { config, password, runtimeProfile }),
  deleteConnection: (id: string) => call<ConnectionConfig[]>("delete_connection", { id }),
  renameGroup: (oldName: string, newName: string) =>
    call<ConnectionConfig[]>("rename_group", { oldName, newName }),
  testConnection: (req: ConnectionRequest) => call<TestResult>("test_connection", { req }),
  openConnection: (id: string) => call<TestResult>("open_connection", { id }),
  closeConnection: (id: string) => call<void>("close_connection", { id }),
  closeAllConnections: () => call<void>("close_all_connections"),

  // 完整对象身份入口；旧按名API保留供兼容，不承担重载选择与受控执行。
  listObjectCatalog: (scope: ObjectCatalogScope, cursor: string | null = null) => call<ObjectCatalogPage>("list_object_catalog", { id: scope.connection_id, catalog: scope.catalog, schema: scope.schema, kinds: scope.kinds, cursor }),
  objectDefinition: (object: DatabaseObjectId) => call<ObjectDefinition>("get_object_definition", { object }),
  routineCallMetadata: (object: DatabaseObjectId) => call<RoutineCallMetadata>("get_routine_call_metadata", { object }),
  previewRoutineCall: (draft: RoutineCallDraft, ownerId: string) => call<RoutineCallPreview>("preview_routine_call", { draft, ownerId }),
  executeRoutineCall: (planId: string, request: ExecutionRequest) => call<RoutineCallResponse>("execute_routine_call", { planId, request }),
  readRoutineDesignObject: (object: DatabaseObjectId) => {
    if (!["function", "procedure"].includes(object.kind) || object.parent) throw new Error("必须选择精确的单例程身份");
    if (object.identity_arguments === null || typeof object.identity_arguments !== "string") throw new Error("OBJECT_SIGNATURE_REQUIRED: 必须从真实目录选择已知签名");
    return call<ExactRoutineSnapshot>("read_exact_routine_design", { object });
  },
  previewRoutineReplacement: (draft: RoutineReplacementDraft) => call<RoutineReplacementPreview & { request_sql: string }>("preview_routine_replacement", { draft }),
  executeRoutineReplacement: (draft: RoutineReplacementDraft, request: ExecutionRequest) => call<{ outcome: ExecutionOutcome; receipt: RoutineReceipt | null }>("execute_routine_replacement", { draft, request }),
  readRoutineCreationBaseline: (object: import("./object-catalog").RoutineObjectId, ownerId: string) => call<RoutineCreationBaseline>("read_routine_creation_baseline", { object, ownerId }),
  previewRoutineCreation: (draft: RoutineCreationDraft, ownerId: string) => call<RoutineCreationPreview>("preview_routine_creation", { draft, ownerId }),
  executeRoutineCreation: (draft: RoutineCreationDraft, request: ExecutionRequest) => call<RoutineCreationResponse>("execute_routine_creation", { draft, request }),
  // 元数据
  listDatabases: (id: string) => call<[string, string][]>("list_databases", { id }),
  tableOptionChoices: (id: string) => call<TableOptionChoices>("table_option_choices", { id }),
  listTables: (id: string, schemaName: string) =>
    call<[string, ObjectType][]>("list_tables", { id, schemaName }),
  listRoutines: (id: string, schemaName: string) =>
    call<[ObjectType, string][]>("list_routines", { id, schemaName }),
  listObjects: (id: string, schemaName: string) =>
    call<import("./types").ObjectInfo[]>("list_objects", { id, schemaName }),
  getDiagramMetadata: (id: string, schemaName: string) =>
    call<DiagramMetadata>("get_diagram_metadata", { id, schemaName }),
  getTableSchema: (id: string, schemaName: string, table: string, includeRowCount?: boolean) =>
    call<import("./types").TableSchema>("get_table_schema", { id, schemaName, table, ...(includeRowCount == null ? {} : { includeRowCount }) }),

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
  executeDdl: (id: string, sql: string, schemaName?: string) => call<number>("execute_ddl", { id, sql, schemaName: schemaName ?? null }),
  // T1/T11：完整基线 → 预览payload → 对request_sql获取一次性grant → 受控执行。
  readTableDesignSnapshot: (id: string, schema: string, table: string, ownerId: string) =>
    call<TableDesignSnapshot>("read_table_design_snapshot", { id, schema, table, ownerId }),
  previewTableDesign: (id: string, design: TableDesignPayload, ownerId: string) =>
    call<TableDesignPreview>("preview_table_design", { id, design, ownerId }),
  executeTableDesign: (request: ExecutionRequest, design: TableDesignPayload) =>
    call<TableDesignResponse>("execute_table_design", { request, design }),
  previewTableOperation: (operation: import("./types").TableOperation, ownerId: string) =>
    call<TableOperationPreview>("preview_table_operation", { operation, ownerId }),
  executeTableOperation: (request: ExecutionRequest, operation: import("./types").TableOperation) =>
    call<TableOperationResponse>("execute_table_operation", { request, operation }),
  /** 仅旧兼容入口；新增设计器必须走完整基线与受控三接口。 */
  applyTableDesign: (id: string, design: TableDesignPayload) =>
    call<DesignReceipt>("apply_table_design", { id, design }),
  /** 预览图形化行写入计划：服务端保存 SQL 与绑定值，不执行。 */
  previewRowMutation: (draft: MutationDraft) =>
    call<MutationPlan>("preview_row_mutation", { draft }),
  /** 执行已预览的计划；执行前会重新校验表结构版本。 */
  executeRowMutation: (planId: string, ownerId: string) =>
    call<MutationReceipt>("execute_row_mutation", { planId, ownerId }),
  /** 读取表上的检查约束与触发器。 */
  listTableConstraints: (id: string, schema: string, table: string) =>
    call<ConstraintSet>("list_table_constraints", { id, schema, table }),
  /** 删除触发器。 */
  dropTableTrigger: (id: string, schema: string, table: string, name: string) =>
    call<number>("drop_table_trigger", { id, schema, table, name }),
  replaceSqliteView: (id: string, schemaName: string, view: string, sql: string) => call<number>("execute_ddl", { id, sql, schemaName, replaceView: view }),

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
  /** 分页读取本次执行快照，不再次执行 SQL。 */
  fetchQueryRows: (scope: import("./types").QuerySpoolScope, index: number, offset: number, limit: number) =>
    call<import("./types").QueryRowsPage>("fetch_query_rows", { ...scope, index, offset, limit }),
  exportQuerySpooled: (opts: ExportOptions, scope: import("./types").QuerySpoolScope, index: number) =>
    call<number>("export_query_spooled", { opts, spoolId: scope.spoolId, ownerId: scope.ownerId, namespace: scope.namespace, index }),
  dropQuerySpool: (scope: import("./types").QuerySpoolScope) => call<void>("drop_query_spool", { ...scope }),
  exportQueryResult: (
    opts: ExportOptions,
    names: string[],
    rows: (string | null)[][],
  ) => call<number>("export_query_result", { opts, names, rows }),
  /** 查询页服务器分页规划：只生成当前页 SQL，不执行、不发 SQL。 */
  planQueryPagination: (connectionId: string, sql: string, limit: number, offset: number) =>
    call<{ mode: "page" | "unsupported"; sql: string | null; reason: string | null }>(
      "plan_query_pagination",
      { connectionId, sql, limit, offset },
    ),
  /** 查询页服务器分页精确总数：后台执行一次 COUNT；不能安全统计时 supported=false。 */
  countServerQueryPage: (connectionId: string, sql: string, namespace: string) =>
    call<{ supported: boolean; total: number | null; reason: string | null }>(
      "count_query_page",
      { connectionId, sql, namespace: namespace || null },
    ),
  /** 查询页 Keyset 判定：SQL 形状命中唯一键时可用锚点续页，否则继续 OFFSET。 */
  planQueryKeyset: (connectionId: string, sql: string, namespace: string) =>
    call<{ supported: boolean; reason: string | null; column?: string; ascending?: boolean; numeric?: boolean }>(
      "plan_query_keyset",
      { connectionId, sql, namespace },
    ),
  /** 按 Keyset 锚点生成查询页 SQL：只生成、不执行。 */
  planQueryKeysetPage: (connectionId: string, sql: string, after: string, limit: number, namespace: string) =>
    call<{ mode: "page" | "unsupported"; sql: string | null; reason: string | null }>(
      "plan_query_keyset_page",
      { connectionId, sql, after, limit, namespace },
    ),
  /** 导出时重新查询全量（仅可安全重跑的单条只读语句；超配额明确拒绝）。 */
  exportQueryRequery: (opts: ExportOptions, sql: string, ownerId: string, namespace: string) =>
    call<number>("export_query_requery", { opts, sql, ownerId, namespace }),
  previewCsv: (path: string, delimiter: string, encoding: string, limit: number) =>
    call<[string[], string[][]]>("preview_csv", { path, delimiter, encoding, limit }),
  runTransfer: (opts: TransferOptions) => call<number>("run_transfer", { opts }),
  runDump: (opts: DumpOptions) => call<number>("run_dump", { opts }),

  // 备份 / 还原
  listBackups: (connectionId: string, schemaName: string) =>
    call<import("./types").BackupInfo[]>("list_backups", { connectionId, schemaName }),
  createBackup: (opts: import("./types").BackupOptions) =>
    call<import("./types").BackupResult>("create_backup", { opts }),
  restoreBackup: (connectionId: string, schemaName: string, file: string) =>
    call<number>("restore_backup", { connectionId, schemaName, file }),
  /** 还原备份（详细版）：返回逐条语句的执行结果，供进度对话框展示。 */
  restoreBackupDetailed: (connectionId: string, schemaName: string, file: string, taskId?: string) =>
    call<import("./types").RestoreReport>("restore_backup_detailed", {
      connectionId,
      schemaName,
      file,
      taskId: taskId ?? null,
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
  readDbUser: (id: string, user: string, host: string) =>
    call<DbUserDetail>("read_db_user", { id, user, host }),
  previewDbUserChanges: (id: string, draft: DbUserDraft) =>
    call<DbUserPlan>("preview_db_user_changes", { id, draft }),
  applyDbUserChanges: (id: string, draft: DbUserDraft) =>
    call<DbUserApplyResult>("apply_db_user_changes", { id, draft }),
  getViewDefinition: (id: string, schemaName: string, view: string) =>
    call<string>("get_view_definition", { id, schemaName, view }),
  getRoutineDefinition: (id: string, schemaName: string, name: string, routineType: string) =>
    call<string>("get_routine_definition", { id, schemaName, name, routineType }),
  readRoutineDesign: (id: string, schemaName: string, name: string, routineType: string, identityArguments?: string) =>
    call<RoutineSnapshot>("read_routine_design", { id, schemaName, name, routineType, identityArguments: identityArguments ?? null }),
  applyRoutineReplacement: (id: string, draft: {
    object: RoutineObjectId;
    base_revision: string;
    definition: string;
    allow_drop_recreate: boolean;
  }) => call<RoutineReceipt>("apply_routine_replacement", { id, draft }),
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
  /** 前端派发的任务（备份 / 同步 / 数据生成等）统一写入同一份运行历史。 */
  recordJobHistory: (jobId: string, jobName: string, ok: boolean, elapsedMs: number, detail: string) =>
    call<void>("record_job_history", { jobId, jobName, ok, elapsedMs, detail }),

  // SQL 文件
  /** 逐条执行 SQL 文件；stopOnError 控制遇错终止还是跳过继续。 */
  runSqlFile: (id: string, schemaName: string, path: string, stopOnError: boolean) =>
    call<import("./types").RestoreReport>("run_sql_file", { id, schemaName, path, stopOnError }),
  /** 把整个数据库（表 + 视图）转储为单个 SQL 文件，保存到用户指定位置。 */
  dumpDatabaseToFile: (id: string, schemaName: string, file: string, includeStructure: boolean, includeData: boolean) =>
    call<[number, number]>("dump_database_to_file", { id, schemaName, file, includeStructure, includeData }),
  /** 重命名备份文件（右键备份改名）。 */
  renameBackup: (connectionId: string, schemaName: string, file: string, newName: string) =>
    call<void>("rename_backup", { connectionId, schemaName, file, newName }),
  /** 在手动事务连接上执行图形化行变更（网格「开始事务」的可视化编辑）。 */
  manualTransactionMutate: (id: string, owner: string, schemaName: string, ops: import("./types").ManualMutationOp[]) =>
    call<MultiResult>("manual_transaction_mutate", { id, owner, schema: schemaName, ops }),

  // 工具
  serverInfo: (id: string) => call<ServerInfo>("server_info", { id }),
  completionData: (id: string, schemaName: string) =>
    call<SqlCompletionData>("completion_data", { id, schemaName }),
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

  // 软件更新
  updateCheck: () => call<import("./types").UpdateCheckResult>("update_check"),
  updateDownload: () => call<string>("update_download"),
  updateStatus: () => call<import("./types").UpdateStatus>("update_status"),
  updateApply: () => call<void>("update_apply"),

  // 数据字典导出
  dictionaryMetadata: (id: string, schemaName: string) =>
    call<import("./types").DictionaryDatabase>("dictionary_metadata", { id, schemaName }),
  dictionaryExport: (
    dto: import("./types").DictionaryDatabase,
    opts: import("./types").DictionaryExportOptions,
    filePath: string,
  ) => call<number>("dictionary_export", { dto, opts, filePath }),

  // 通用
  /** 用系统文件管理器打开本地路径（文件 → 选中，目录 → 进入）。 */
  openPathInExplorer: (path: string) => call<void>("open_path_in_explorer", { path }),
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
/** 连接高级参数的实际应用诊断（PL-14）。 */
export interface ConnectionOptionDiagnostic {
  key: string;
  status: "applied" | "unsupported" | "invalid" | "legacy_ignored";
  message: string;
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
  options: (id: string) => call<ConnectionOptionDiagnostic[]>("validate_connection_options", { id }),
};

/** 托管 Java 运行时状态（JDBC / H2 连接依赖 Java 11+ 完整 JDK）。 */
export interface JavaRuntimeStatus {
  installed: boolean;
  version: string | null;
  path: string | null;
  source: "managed" | "system" | "missing";
  message: string;
  can_install: boolean;
  download_size: number;
}
export const javaRuntimeApi = {
  status: () => call<JavaRuntimeStatus>("java_runtime_status"),
  install: () => call<JavaRuntimeStatus>("install_java_runtime"),
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
  /** 自定义业务键（源列名）；为空时使用主键。 */
  key_columns?: string[];
  /** 增量时删除目标中键不在源数据的多余行（默认关闭）。 */
  delete_missing?: boolean;
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
  operation_id?: string | null;
  execution_state?: import("./execution").ExecutionState | null;
  ts: string; connection_id: string; connection_name: string; kind: string;
  database: string; sql: string; ok: boolean; elapsed_ms: number; rows: number;
  message: string; origin: string;
  /** 后端真实捕获新增的可选字段；旧日志可能没有。 */
  namespace?: string | null;
  parameters?: { ordinal: number; type_name: string; value: string; redacted: boolean }[];
}
export interface TaskProgress {
  task_id: string; status: "running" | "completed" | "failed" | "cancelled";
  phase: string; processed: number; total: number; message: string;
}
/** Batch exports always name an explicit nonempty table/view collection. */
export interface BatchExportObject { name: string; object_type: "table" | "view" }
export interface BatchExportRequest {
  connection_id: string; namespace: string; objects: BatchExportObject[];
  format: "csv" | "json" | "sql";
  destination: { mode: "files"; base_file: string } | { mode: "sql_file"; file: string };
  encoding: string; delimiter: string; has_header: boolean; task_id: string;
}
export interface BatchExportItemResult {
  object: BatchExportObject; file: string | null; rows: number;
  status: "completed" | "failed" | "unknown" | "skipped"; error: string | null;
}
export interface BatchExportResult { items: BatchExportItemResult[]; combined_file: string | null; message: string }
export const batchExportApi = { run: (request: BatchExportRequest) => call<BatchExportResult>("run_batch_export", { request }) };

export const transferTasks = {
  import: (opts: ImportOptions & { task_id: string; column_mapping: { source: string; target: string }[]; write_strategy?: "row" | "pg_copy" }) => call<number>("run_import_task", { opts }),
  export: (opts: ExportOptions & { task_id: string }) => call<number>("run_export_task", { opts }),
  progress: (taskId: string) => call<TaskProgress>("transfer_task_progress", { taskId }),
  cancel: (taskId: string) => call<void>("cancel_transfer_task", { taskId }),
};
/** 比较缓冲/键/临时数据预算，不是整个程序 RSS 硬上限；后端重新采样并校验额度。 */
export interface DiffMemoryOptions {
  mode: "speed" | "auto" | "manual";
  limit_bytes?: number | null;
}
export interface DiffMemoryStats {
  mode: string;
  available_bytes: number;
  limit_bytes: number;
  used_bytes: number;
  peak_bytes: number;
  spilled_bytes: number;
  backend: "memory" | "disk" | "count_only";
  scope: string;
  pressure_events: number;
}
/** 原生 OS 只读预估；额度不代表预分配或内存驻留。 */
export interface DiffMemoryPreview {
  available_bytes: number;
  suggested_limit_bytes: number;
  global_limit_bytes: number;
  scope: string;
  limitations: string[];
}
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
  memory?: DiffMemoryStats;
}
/** 结构差异同步请求（对应后端 structure_sync::StructureSyncRequest）。 */
export interface StructureSyncRequest {
  source_connection: string; source_schema: string; source_table: string;
  target_connection: string; target_schema: string; target_table: string;
  /** (源列, 目标列)；为空表示同名映射全部源列。 */
  columns: [string, string][];
  allow_drop: boolean;
  target_revision: string;
  source_revision: string;
  review_digest: string;
}
export interface StructureChange {
  kind: "add_column" | "alter_column" | "drop_column" | "primary_key" | "index" | "table_option" | string;
  object: string;
  before: string | null;
  after: string | null;
  risk: "safe" | "scan_required" | "destructive" | string;
}
export interface StructureDiffReport {
  target_exists: boolean;
  changes: StructureChange[];
  warnings: string[];
  alterable: boolean;
  unsupported: string[];
  source_revision: string;
  target_revision: string;
  summary: string;
  final_design: TableDesignPayload | null;
  steps: { label: string; sql: string }[];
  review_digest: string;
  review_binding: {
    principal: string; owner: string;
    source_connection_id: string; source_fingerprint: string; source_schema: string; source_table: string;
    target_connection_id: string; target_fingerprint: string; target_schema: string; target_table: string;
  } | null;
}
export interface StructurePhysicalIdentity {
  database: { kind: string; identity: string; canonical_path: string | null };
  schema: string; table: string; rootpage: number;
}
export interface StructurePreview extends StructureDiffReport {
  request_sql: string;
  source_identity: StructurePhysicalIdentity;
  target_identity: StructurePhysicalIdentity | null;
  same_database: boolean;
  source_consistency: string;
  observation_digest: string;
  observation_binding: NonNullable<StructureDiffReport["review_binding"]>;
  observation_only: boolean;
}
export interface StructureExecutionResponse { outcome: ExecutionOutcome; receipt: StructureSyncReceipt | null }
export interface StructureSyncReceipt {
  report: StructureDiffReport;
  receipt: DesignReceipt;
}
export const syncApi = {
  run: (opts: SyncOptions) => call<SyncReport>("run_sync", { opts }),
  runDatabase: (req: DbSyncOptions) => call<SyncTableReport[]>("run_sync_database", { req }),
  diff: async (req: { source_connection: string; source_schema: string; source_table: string; target_connection: string; target_schema: string; target_table: string; where_clause: string | null }, bounded = false, memory?: DiffMemoryOptions) => {
    if (bounded && memory !== undefined) throw new Error("不能同时指定旧低内存比较 bounded 与内存预算 memory");
    return call<SyncDiffReport>("sync_diff", memory !== undefined ? { req, memory } : bounded ? { req, bounded: true } : { req });
  },
  previewMemory: (memory?: DiffMemoryOptions) => call<DiffMemoryPreview>("preview_diff_memory", { memory }),
  typeMapping: (sourceConnection: string, sourceSchema: string, sourceTable: string, targetConnection: string) =>
    call<{ source: string; source_type: string; target_type: string }[]>("sync_type_mapping", { sourceConnection, sourceSchema, sourceTable, targetConnection }),
  /** 只读预览目标表相对源表的结构差异。 */
  previewStructure: (request: StructureSyncRequest, ownerId: string) => call<StructurePreview>("preview_controlled_structure_sync", { request, ownerId }),
  executeStructure: (request: ExecutionRequest, syncRequest: StructureSyncRequest) => call<StructureExecutionResponse>("execute_structure_sync", { request, syncRequest }),
  /** 应用结构差异（生成 ALTER 并读回校验）。 */
  applyStructure: (request: StructureSyncRequest) => call<StructureSyncReceipt>("apply_structure_sync", { request }),
};
