/** 新例程保留真实不存在基线；提交仍使用唯一 C1 授权和停止控制器。 */
import { ExecutionController, type ExecutionOutcome, type ExecutionRequest, type ExecutionState, type ExecutionTransport } from "./execution";
import { requireRoutineSourceFingerprint, type RoutineObjectId, type RoutineSnapshot } from "./object-catalog";

export interface RoutineCreationBaseline {
  baseline_id: string; object: RoutineObjectId; source_fingerprint: string | null;
  base_revision: string; completeness: string; absent: boolean; expires_in_ms: number;
  strategy: string; warnings: string[];
}
export interface RoutineCreationDraft {
  baseline_id: string; object: RoutineObjectId; source_fingerprint: string; base_revision: string; definition: string;
}
export interface RoutineCreationPreview {
  baseline_id: string; object: RoutineObjectId; source_fingerprint: string | null; base_revision: string;
  steps: { label: string; sql: string }[]; request_sql: string; strategy: string; executable: boolean;
  expires_in_ms: number; warnings: string[];
}
export interface RoutineCreationReceipt {
  object: RoutineObjectId; source_fingerprint: string | null; baseline_id: string; status: string;
  verified: boolean; steps: { label: string; sql: string; status: string; error: string | null }[];
  transaction: string; snapshot: RoutineSnapshot; warnings: string[];
}
export interface RoutineCreationResponse { outcome: ExecutionOutcome; receipt: RoutineCreationReceipt | null }
export interface RoutineCreationPort extends Omit<ExecutionTransport, "executeSqlV2"> {
  issueGrant(request: ExecutionRequest): Promise<ExecutionRequest>;
  preview(draft: RoutineCreationDraft): Promise<RoutineCreationPreview>;
  execute(draft: RoutineCreationDraft, request: ExecutionRequest): Promise<RoutineCreationResponse>;
}
function sameObject(a: RoutineObjectId | undefined, b: RoutineObjectId): boolean {
  return !!a && a.connection_id === b.connection_id && a.schema === b.schema && a.name === b.name
    && a.kind === b.kind && a.identity_arguments === b.identity_arguments;
}
function validateBaseline(value: RoutineCreationBaseline): void {
  requireRoutineSourceFingerprint(value.source_fingerprint);
  if (!/^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/.test(value.baseline_id) || /^0{8}-0{4}-0{4}-0{4}-0{12}$/.test(value.baseline_id)
    || !value.base_revision || value.completeness !== "complete" || value.absent !== true
    || !Number.isSafeInteger(value.expires_in_ms) || value.expires_in_ms <= 0 || value.expires_in_ms > 120_000
    || !["create_transactional", "create_implicit_commit"].includes(value.strategy)
    || !value.object?.connection_id || !value.object.schema || !value.object.name
    || !["function", "procedure"].includes(value.object.kind) || value.object.identity_arguments !== "") {
    throw new Error("ROUTINE_CREATION_BASELINE_REQUIRED: 必须取得完整且未过期的零参数新名字不存在基线");
  }
}
function verifiedReceipt(value: RoutineCreationReceipt | null, draft: RoutineCreationDraft, strategy: string): value is RoutineCreationReceipt {
  const expectedTransaction = strategy === "create_transactional" ? "committed" : "implicit_commit_acknowledged";
  return !!value && value.status === "applied" && value.verified === true && value.transaction === expectedTransaction
    && value.baseline_id === draft.baseline_id && value.source_fingerprint === draft.source_fingerprint
    && sameObject(value.object, draft.object) && sameObject(value.snapshot?.object, draft.object)
    && value.snapshot?.source_fingerprint === draft.source_fingerprint && value.snapshot.completeness === "complete"
    && value.snapshot.overloads === 1 && typeof value.snapshot.revision === "string" && !!value.snapshot.revision
    && typeof value.snapshot.definition === "string" && !!value.snapshot.definition.trim()
    && Array.isArray(value.steps) && value.steps.length === 1 && value.steps.every(step => step.status === "applied" && !step.error && typeof step.sql === "string" && !!step.sql.trim())
    && Array.isArray(value.warnings);
}

export class RoutineCreationSession {
  private baseline: RoutineCreationBaseline;
  private expiresAt: number;
  private submitting = false;
  private disposed = false;
  private stopped = false;
  private unknown = false;
  private recheckRequired = false;
  private activeDraft: RoutineCreationDraft | null = null;
  private response: RoutineCreationReceipt | null = null;
  private created: RoutineSnapshot | null = null;
  readonly execution: ExecutionController;
  message = "";
  constructor(baseline: RoutineCreationBaseline, private readonly ownerId: string, private readonly port: RoutineCreationPort, private readonly changed: () => void = () => {}) {
    validateBaseline(baseline);
    if (!ownerId.trim() || typeof port.issueGrant !== "function") throw new Error("新建例程必须归属有效页面并接入C1授权");
    this.baseline = structuredClone(baseline); this.expiresAt = Date.now() + baseline.expires_in_ms;
    this.execution = new ExecutionController({
      issueGrant: request => port.issueGrant(request),
      executeSqlV2: async request => {
        const draft = this.activeDraft;
        if (!draft) throw new Error("ROUTINE_CREATION_CONTEXT_CHANGED: 创建草稿已不存在");
        // 进入执行 IPC 后一次性基线可能已被消费；只允许用户明确重新核对，不能暗中续签重放。
        this.recheckRequired = true;
        const response = await port.execute(draft, request);
        if (!response || response.outcome?.execution_id !== request.execution_id) throw new Error("ROUTINE_CREATION_RESPONSE_INVALID: 创建回执身份不一致");
        this.response = response.receipt; return response.outcome;
      },
      cancelExecution: context => port.cancelExecution(context), executionStatus: context => port.executionStatus(context),
    }, () => this.changed());
  }
  get busy(): boolean { return this.submitting || this.execution.busy; }
  get effectsUnknown(): boolean { return this.unknown; }
  get state(): ExecutionState { return this.execution.state; }
  get needsRecheck(): boolean { return this.recheckRequired || Date.now() >= this.expiresAt; }
  get loadedBaseline(): RoutineCreationBaseline { return structuredClone(this.baseline); }
  get createdSnapshot(): RoutineSnapshot | null { return this.created ? structuredClone(this.created) : null; }
  private currentResponse(): RoutineCreationReceipt | null { return this.response; }
  async save(definition: string): Promise<boolean> {
    if (this.disposed) throw new Error("例程页面已关闭");
    if (this.unknown) throw new Error("创建结果未知，禁止重放；请明确读取真实目标核对");
    if (this.busy) throw new Error("例程执行或资源排空尚未结束");
    if (this.created) throw new Error("例程已创建，不能重复执行CREATE");
    if (this.needsRecheck) throw new Error("创建基线已过期或可能已消费，请点击重新核对目标后再保存");
    if (!definition.trim() || new TextEncoder().encode(definition).byteLength > 2 * 1024 * 1024) throw new Error("新例程定义不能为空或超过2MiB");
    const draft: RoutineCreationDraft = { baseline_id: this.baseline.baseline_id, object: structuredClone(this.baseline.object), source_fingerprint: requireRoutineSourceFingerprint(this.baseline.source_fingerprint), base_revision: this.baseline.base_revision, definition };
    this.submitting = true; this.stopped = false; this.activeDraft = draft; this.response = null;
    this.message = "正在检查固定不存在基线（尚未发送CREATE）"; this.changed();
    try {
      const preview = await this.port.preview(draft);
      if (this.disposed || this.stopped) throw new Error("创建准备已停止，没有发送SQL");
      if (preview.baseline_id !== draft.baseline_id || !sameObject(preview.object, draft.object) || preview.base_revision !== draft.base_revision
        || requireRoutineSourceFingerprint(preview.source_fingerprint) !== draft.source_fingerprint || preview.strategy !== this.baseline.strategy
        || typeof preview.request_sql !== "string" || !/^ROUTINE_CREATION [0-9a-f]{64}$/.test(preview.request_sql)
        || !Number.isSafeInteger(preview.expires_in_ms) || preview.expires_in_ms <= 0) throw new Error("ROUTINE_CREATION_PLAN_STALE: 预览来源、对象或不存在基线不匹配，没有发送SQL");
      if (!preview.executable) throw new Error(preview.warnings.join("；") || "当前创建策略尚未认证，没有发送SQL");
      const outcome = await this.execution.run({ execution_id: globalThis.crypto.randomUUID(), owner_id: this.ownerId, connection_id: draft.object.connection_id,
        namespace: draft.object.schema, sql: preview.request_sql, timeout_ms: 60_000, origin: "routine-creation" });
      if (!outcome && this.execution.state === "failed" && !this.execution.busy) throw new Error("创建授权或前置校验失败，没有发送SQL");
      if (!outcome || outcome.state === "unknown" || outcome.resources_pending) { this.unknown = true; throw new Error("创建结果未知或资源未排空；保留文本并禁止重放，请读取真实目标核对"); }
      const receipt = this.currentResponse();
      if (outcome.state !== "succeeded" || !verifiedReceipt(receipt, draft, this.baseline.strategy)) {
        if (outcome.state === "succeeded" || !["none", "no_effect"].includes(outcome.effects)) this.unknown = true;
        throw new Error(outcome.error?.message || "创建未取得完整提交与读回回执，没有将其当作成功");
      }
      this.created = structuredClone(receipt.snapshot); this.message = "新例程已创建并读回验证；保存期间的新编辑仍保留"; return true;
    } catch (error) {
      if (this.execution.state === "unknown" || this.execution.busy) this.unknown = true;
      this.message = error instanceof Error ? error.message : String(error); throw error;
    } finally { this.submitting = false; if (!this.unknown) this.activeDraft = null; this.changed(); }
  }
  async stop(): Promise<void> {
    this.stopped = true;
    if (this.submitting && !this.execution.busy) { this.message = "创建准备已请求停止，没有发送SQL"; this.changed(); return; }
    await this.execution.cancel();
  }
  replaceBaseline(baseline: RoutineCreationBaseline): void {
    if (this.busy || this.created || this.disposed) throw new Error("尚在执行、排空或已创建，不能替换不存在基线");
    validateBaseline(baseline);
    if (!sameObject(baseline.object, this.baseline.object) || baseline.source_fingerprint !== this.baseline.source_fingerprint) throw new Error("不能用另一例程或端点的基线替换创建页面");
    this.baseline = structuredClone(baseline); this.expiresAt = Date.now() + baseline.expires_in_ms;
    this.unknown = false; this.recheckRequired = false; this.activeDraft = null; this.response = null;
    this.message = "已明确重新核对真实目标不存在状态；当前创建文本保留"; this.changed();
  }
  capture(): { creation_baseline: RoutineCreationBaseline; execution_state: ExecutionState; execution_effects_unknown: boolean; requires_recheck: boolean; pending_draft: RoutineCreationDraft | null } {
    return { creation_baseline: this.loadedBaseline, execution_state: this.state, execution_effects_unknown: this.unknown, requires_recheck: this.needsRecheck, pending_draft: this.activeDraft ? structuredClone(this.activeDraft) : null };
  }
  dispose(): void { this.disposed = true; this.execution.dispose(); }
}
