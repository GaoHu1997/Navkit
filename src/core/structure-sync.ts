/** 结构同步的固定预览与受控执行；页面不生成或执行 SQL。 */
import type { StructureDiffReport, StructureSyncRequest, StructureSyncReceipt, StructurePreview, StructurePhysicalIdentity } from "./api";
import { ExecutionController, type ExecutionOutcome, type ExecutionRequest, type ExecutionTransport } from "./execution";

export interface StructureSyncPort extends Omit<ExecutionTransport, "executeSqlV2"> {
  issueGrant(request: ExecutionRequest): Promise<ExecutionRequest>;
  preview(request: StructureSyncRequest, ownerId: string): Promise<StructurePreview>;
  execute(request: ExecutionRequest, syncRequest: StructureSyncRequest): Promise<{ outcome: ExecutionOutcome; receipt: StructureSyncReceipt | null }>;
  current(): StructureSyncRequest;
}
const revision = (value: unknown): value is string => typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
function inputOf(request: StructureSyncRequest): StructureSyncRequest {
  return { source_connection: request.source_connection, source_schema: request.source_schema, source_table: request.source_table,
    target_connection: request.target_connection, target_schema: request.target_schema, target_table: request.target_table,
    columns: request.columns.map(([from, to]) => [from, to]), allow_drop: request.allow_drop,
    source_revision: "", target_revision: "", review_digest: "" };
}
function key(request: StructureSyncRequest): string { return JSON.stringify(inputOf(request)); }
function boundReport(report: StructureDiffReport, request: StructureSyncRequest, owner: string): boolean {
  const binding = report.review_binding;
  return !!binding && binding.owner === owner && !!binding.principal && revision(binding.source_fingerprint) && revision(binding.target_fingerprint)
    && binding.source_connection_id === request.source_connection && binding.source_schema === request.source_schema && binding.source_table === request.source_table
    && binding.target_connection_id === request.target_connection && binding.target_schema === request.target_schema && binding.target_table === request.target_table
    && revision(report.source_revision) && revision(report.target_revision) && revision(report.review_digest);
}
function boundObservation(report: StructurePreview, request: StructureSyncRequest, owner: string): boolean {
  const binding = report.observation_binding;
  const identity = (value: StructurePhysicalIdentity | null, schema: string, table: string): boolean => !!value
    && value.schema === schema && value.table === table && Number.isSafeInteger(value.rootpage) && value.rootpage > 0
    && ["sqlite-file", "sqlite-memory"].includes(value.database?.kind) && typeof value.database.identity === "string"
    && value.database.identity.length > 0 && value.database.identity.length <= 4096
    && (value.database.canonical_path === null || typeof value.database.canonical_path === "string");
  return revision(report.observation_digest) && revision(report.source_revision)
    && boundReport({ ...report, review_binding: binding, review_digest: report.observation_digest,
      target_revision: report.target_exists ? report.target_revision : report.source_revision }, request, owner)
    && identity(report.source_identity, request.source_schema, request.source_table)
    && (report.target_exists ? revision(report.target_revision) && identity(report.target_identity, request.target_schema, request.target_table) : report.target_identity === null)
    && typeof report.same_database === "boolean"
    && (!report.target_identity || report.same_database === (report.source_identity.database.kind === report.target_identity.database.kind && report.source_identity.database.identity === report.target_identity.database.identity))
    && report.source_consistency === (report.same_database ? "same_owned_read_snapshot" : "two_owned_read_snapshots_not_cross_database_atomic")
    && typeof report.observation_only === "boolean" && report.observation_only === (report.request_sql === "");
}
function sameSource(before: StructurePreview, after: StructurePreview): boolean {
  const a = before.observation_binding, b = after.observation_binding;
  const sameIdentity = (x: StructurePhysicalIdentity | null, y: StructurePhysicalIdentity | null): boolean => !!x && !!y
    && x.schema === y.schema && x.table === y.table && x.rootpage === y.rootpage
    && x.database.kind === y.database.kind && x.database.identity === y.database.identity;
  return !!a && !!b && a.principal === b.principal && a.owner === b.owner
    && a.source_connection_id === b.source_connection_id && a.source_fingerprint === b.source_fingerprint
    && a.source_schema === b.source_schema && a.source_table === b.source_table
    && a.target_connection_id === b.target_connection_id && a.target_fingerprint === b.target_fingerprint
    && a.target_schema === b.target_schema && a.target_table === b.target_table
    && sameIdentity(before.source_identity, after.source_identity) && sameIdentity(before.target_identity, after.target_identity);
}
interface Review { input: string; request: StructureSyncRequest; report: StructurePreview }
export class StructureSyncSession {
  readonly execution: ExecutionController;
  private review: Review | null = null;
  private active: Review | null = null;
  private receipt: StructureSyncReceipt | null = null;
  private reading = false;
  private generation = 0;
  private disposed = false;
  private unknown = false;
  private reconciliation: StructurePreview | null = null;
  message = "";
  constructor(private readonly ownerId: string, private readonly port: StructureSyncPort, private readonly changed: () => void = () => {}) {
    if (!ownerId || ownerId.length > 128 || ownerId.includes("\0")) throw new Error("STRUCTURE_OWNER_REQUIRED: 结构同步必须归属有效页面");
    this.execution = new ExecutionController({
      issueGrant: async request => {
        const grant = await port.issueGrant(request);
        if (this.disposed || !this.active || key(port.current()) !== this.active.input) throw new Error("STRUCTURE_REVIEW_CHANGED: 授权期间映射或目标改变，没有发送 SQL");
        return grant;
      },
      executeSqlV2: async request => {
        const active = this.active;
        if (!active || key(port.current()) !== active.input) throw new Error("STRUCTURE_REVIEW_CHANGED: 执行前上下文改变，没有发送 SQL");
        const response = await port.execute(request, structuredClone(active.request));
        if (!response || response.outcome?.execution_id !== request.execution_id) throw new Error("STRUCTURE_RECEIPT_INVALID: 执行回执身份不一致");
        this.receipt = response.receipt; return response.outcome;
      },
      cancelExecution: context => port.cancelExecution(context), executionStatus: context => port.executionStatus(context),
    }, () => changed());
  }
  get busy(): boolean { return this.reading || this.execution.busy; }
  get effectsUnknown(): boolean { return this.unknown; }
  get canApply(): boolean { return !this.busy && !this.unknown && this.review !== null && key(this.port.current()) === this.review.input; }
  invalidate(): void { this.review = null; this.generation++; this.changed(); }
  async preview(input = this.port.current()): Promise<StructurePreview> {
    if (this.disposed || this.busy) throw new Error("结构工作或资源排空尚未结束");
    const request = inputOf(input), inputKey = key(request), generation = ++this.generation;
    this.review = null; this.reading = true; this.message = "正在读取双端真实结构（没有发送 DDL）"; this.changed();
    try {
      const report = await this.port.preview(structuredClone(request), this.ownerId);
      if (this.disposed || generation !== this.generation || key(this.port.current()) !== inputKey) throw new Error("STRUCTURE_REVIEW_CHANGED: 读取期间上下文改变，未保留旧预览");
      if (!report || typeof report.alterable !== "boolean" || typeof report.target_exists !== "boolean" || !Array.isArray(report.changes) || !Array.isArray(report.steps) || !Array.isArray(report.unsupported) || !Array.isArray(report.warnings) || report.steps.some(step => typeof step?.sql !== "string" || !step.sql.trim() || typeof step.label !== "string")) throw new Error("STRUCTURE_PREVIEW_INVALID: 预览协议不完整");
      if (!boundObservation(report, request, this.ownerId)) throw new Error("STRUCTURE_PREVIEW_UNBOUND: 缺少完整双端物理快照与独立核对证据");
      if (report.alterable && report.target_exists && !report.unsupported.length && report.steps.length) {
        if (report.observation_only || !boundReport(report, request, this.ownerId) || !sameSource(report, { ...report, observation_binding: report.review_binding! }) || report.final_design?.schema !== request.target_schema || report.final_design.table !== request.target_table || !/^STRUCTURE_SYNC_REVIEW [0-9a-f]{64}$/.test(report.request_sql)) throw new Error("STRUCTURE_PREVIEW_UNBOUND: 缺少双端真实来源/固定版本/受控工作标识");
        this.review = { input: inputKey, request: { ...request, source_revision: report.source_revision, target_revision: report.target_revision, review_digest: report.review_digest }, report: structuredClone(report) };
      } else if (!report.observation_only || report.request_sql || report.review_digest || report.review_binding) {
        throw new Error("STRUCTURE_PREVIEW_INVALID: 只读核对不能携带可执行结构计划");
      }
      if (this.unknown && (!report.target_exists || report.unsupported.length || !this.reconciliation || !sameSource(this.reconciliation, report))) {
        this.review = null; throw new Error("STRUCTURE_RECONCILIATION_INCOMPLETE: 来源或完整物理快照未核对，不能解除未知写入锁");
      }
      const reconciled = this.unknown;
      this.unknown = false; this.reconciliation = null;
      this.message = reconciled ? `${report.summary}；已核对当前双端结构，此前执行的未知终态保持不变` : report.summary; return report;
    } finally { this.reading = false; this.changed(); }
  }
  async apply(confirmDestructive = false): Promise<StructureSyncReceipt> {
    if (this.disposed || this.busy || this.unknown) throw new Error("结构执行尚未结束或回执未知；必须明确读取真实双端核对");
    const review = this.review;
    if (!review || key(this.port.current()) !== review.input) { this.invalidate(); throw new Error("STRUCTURE_REVIEW_CHANGED: 请按当前映射重新预览，未签发执行授权"); }
    if ((review.request.allow_drop || review.report.changes.some(change => change.risk === "destructive")) && !confirmDestructive) throw new Error("STRUCTURE_DESTRUCTIVE_CONFIRMATION_REQUIRED: 尚未明确确认破坏性结构变更");
    this.review = null; this.active = structuredClone(review); this.reconciliation = structuredClone(review.report); this.receipt = null; this.message = "正在应用已确认的结构计划"; this.changed();
    try {
      const outcome = await this.execution.run({ execution_id: globalThis.crypto.randomUUID(), connection_id: review.request.target_connection,
        namespace: review.request.target_schema, owner_id: this.ownerId, sql: review.report.request_sql, timeout_ms: 60_000, origin: "structure-sync" });
      if (!outcome || outcome.state === "unknown" || outcome.resources_pending) { this.unknown = true; throw new Error("结构回执未知或资源仍在排空；禁止重放，请明确重新预览核对真实目标"); }
      const receipt = this.currentReceipt();
      if (outcome.state !== "succeeded") {
        if (outcome.effects !== "no_effect" && outcome.transaction_state !== "rolled_back") this.unknown = true;
        throw new Error(outcome.error?.message || "结构计划未成功提交；请核对后重新预览");
      }
      if (!receipt || receipt.report?.review_digest !== review.request.review_digest || !boundReport(receipt.report, review.request, this.ownerId)
        || receipt.report.source_revision !== review.request.source_revision || receipt.report.target_revision !== review.request.target_revision
        || receipt.report.review_binding?.principal !== review.report.review_binding?.principal || receipt.report.review_binding?.source_fingerprint !== review.report.review_binding?.source_fingerprint || receipt.report.review_binding?.target_fingerprint !== review.report.review_binding?.target_fingerprint
        || outcome.transaction_state !== "committed"
        || receipt.receipt?.verified !== true || receipt.receipt.unverified !== false || receipt.receipt.transaction !== "committed"
        || !Array.isArray(receipt.receipt.mismatches) || receipt.receipt.mismatches.length || !Array.isArray(receipt.receipt.steps)
        || receipt.receipt.steps.length !== review.report.steps.length || receipt.receipt.steps.some((step, index) => step.status !== "applied" || step.sql !== review.report.steps[index]?.sql)
        || receipt.receipt.schema?.table !== review.request.target_table || receipt.receipt.schema.database !== review.request.target_schema) {
        this.unknown = true; throw new Error("STRUCTURE_RECEIPT_INVALID: 缺少匹配的提交及完整结构读回证据，禁止重放");
      }
      this.message = "结构已提交并完成读回校验"; return receipt;
    } catch (error) {
      if (this.execution.state === "unknown" || this.execution.busy) this.unknown = true;
      this.message = String(error); throw error;
    } finally { this.active = null; this.changed(); }
  }
  private currentReceipt(): StructureSyncReceipt | null { return this.receipt; }
  async stop(): Promise<void> { if (this.reading) this.invalidate(); await this.execution.cancel(); }
  dispose(): void { this.disposed = true; this.invalidate(); this.execution.dispose(); }
}
