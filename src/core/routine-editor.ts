/** 例程编辑的页面级状态：固定加载时CAS基线，所有写入复用唯一C1控制器。 */
import { ExecutionController, type ExecutionOutcome, type ExecutionRequest, type ExecutionState, type ExecutionTransport } from "./execution";
import { RoutineEditorBaseline, requireRoutineSourceFingerprint, type RoutineSnapshot, type RoutineReplacementDraft, type RoutineReplacementPreview } from "./object-catalog";

type BoundDraft = RoutineReplacementDraft & { source_fingerprint: string };
type Preview = RoutineReplacementPreview & { request_sql: string };
export interface RoutineEditorPort extends Omit<ExecutionTransport, "executeSqlV2"> {
  issueGrant(request: ExecutionRequest): Promise<ExecutionRequest>;
  preview(draft: BoundDraft): Promise<Preview>;
  execute(draft: BoundDraft, request: ExecutionRequest): Promise<{ outcome: ExecutionOutcome; receipt: unknown }>;
}
interface VerifiedReplacement {
  verified: true; status: "succeeded"; transaction: "committed"; original_revision: string;
  revision: string; definition: string; mismatches: string[];
}
function fingerprint(snapshot: RoutineSnapshot): string {
  const value = (snapshot as RoutineSnapshot & { source_fingerprint?: unknown }).source_fingerprint;
  if (typeof value !== "string" || !/^[0-9a-f]{64}$/.test(value)) throw new Error("ROUTINE_BASELINE_UNBOUND: 缺少加载时端点指纹，请重新读取完整例程，不能保存时补读基线");
  return value;
}
function sameObject(a: RoutineReplacementDraft["object"], b: RoutineReplacementDraft["object"]): boolean {
  return a.connection_id === b.connection_id && a.schema === b.schema && a.name === b.name && a.kind === b.kind && a.identity_arguments === b.identity_arguments;
}
function verifiedReceipt(value: unknown, draft: BoundDraft): value is VerifiedReplacement {
  if (!value || typeof value !== "object") return false;
  const receipt = value as Partial<VerifiedReplacement>;
  return receipt.verified === true && receipt.status === "succeeded" && receipt.transaction === "committed"
    && receipt.original_revision === draft.base_revision && typeof receipt.revision === "string" && !!receipt.revision
    && typeof receipt.definition === "string" && !!receipt.definition && Array.isArray(receipt.mismatches)
    && receipt.mismatches.length === 0;
}

export class RoutineEditorSession {
  private baseline: RoutineEditorBaseline;
  private loaded: RoutineSnapshot;
  private submitting = false;
  private activeDraft: BoundDraft | null = null;
  private response: unknown = null;
  private unknown = false;
  private disposed = false;
  private stopRequested = false;
  readonly execution: ExecutionController;
  message = "";
  constructor(snapshot: RoutineSnapshot, private readonly ownerId: string, private readonly port: RoutineEditorPort, private readonly changed: () => void = () => {}) {
    fingerprint(snapshot);
    if (!ownerId.trim() || typeof port.issueGrant !== "function") throw new Error("例程编辑必须归属有效页面并接入C1授权");
    this.loaded = structuredClone(snapshot); this.baseline = new RoutineEditorBaseline(snapshot);
    this.execution = new ExecutionController({
      issueGrant: request => port.issueGrant(request),
      executeSqlV2: async request => {
        const draft = this.activeDraft;
        if (!draft) throw new Error("ROUTINE_CONTEXT_CHANGED: 编辑请求已不存在");
        const response = await port.execute(draft, request);
        if (!response || response.outcome?.execution_id !== request.execution_id) throw new Error("ROUTINE_RESPONSE_INVALID: 执行回执身份不一致");
        this.response = response.receipt; return response.outcome;
      },
      cancelExecution: context => port.cancelExecution(context), executionStatus: context => port.executionStatus(context),
    }, () => this.changed());
  }
  get busy(): boolean { return this.submitting || this.execution.busy; }
  get effectsUnknown(): boolean { return this.unknown; }
  get state(): ExecutionState { return this.execution.state; }
  get loadedSnapshot(): RoutineSnapshot { return structuredClone(this.loaded); }
  private currentResponse(): unknown { return this.response; }
  async save(definition: string): Promise<boolean> {
    if (this.disposed || this.busy) throw new Error("例程执行或资源排空尚未结束");
    if (this.unknown) throw new Error("例程写入回执未知；禁止重放，请明确重新读取真实目标核对");
    if (!definition.trim()) throw new Error("例程定义不能为空");
    const draft: BoundDraft = { ...this.baseline.draft(definition, false), source_fingerprint: fingerprint(this.loaded) };
    this.submitting = true; this.stopRequested = false; this.activeDraft = draft; this.response = null; this.message = "正在预览（尚未发送SQL）"; this.changed();
    try {
      const preview = await this.port.preview(draft);
      if (this.disposed || this.stopRequested) throw new Error("例程准备已停止，没有发送SQL");
      if (!sameObject(preview.object, draft.object) || preview.base_revision !== draft.base_revision || typeof preview.request_sql !== "string" || !preview.request_sql.startsWith("ROUTINE_REPLACEMENT ")) throw new Error("ROUTINE_PLAN_STALE: 预览对象或基线不一致，没有发送SQL");
      if (requireRoutineSourceFingerprint(preview.source_fingerprint) !== draft.source_fingerprint) throw new Error("ROUTINE_SOURCE_CHANGED: 预览端点与加载时不同，没有发送SQL");
      if (!preview.executable) throw new Error(preview.warnings.join("；") || "此替换尚无已验证的安全恢复通道，仅保留SQL供复制 / 导出");
      const outcome = await this.execution.run({ execution_id: globalThis.crypto.randomUUID(), owner_id: this.ownerId, connection_id: draft.object.connection_id, namespace: draft.object.schema, sql: preview.request_sql, timeout_ms: 60_000, origin: "routine-replacement" });
      if (!outcome && this.execution.state === "failed" && !this.execution.busy) throw new Error("例程授权或前置校验失败，没有发送SQL");
      if (!outcome || outcome.state === "unknown" || outcome.resources_pending) { this.unknown = true; throw new Error("例程写入回执未知或资源未排空；保留当前文本，禁止重放，请读取真实目标核对"); }
      const receipt = this.currentResponse();
      if (outcome.state !== "succeeded" || !verifiedReceipt(receipt, draft)) {
        const provedNoEffect = outcome.state !== "succeeded" && ["none", "no_effect"].includes(outcome.effects);
        if (!provedNoEffect) this.unknown = true;
        throw new Error(outcome.error?.message || "例程替换未取得完整提交与读回回执，没有将其当作成功");
      }
      this.loaded = { ...this.loaded, revision: receipt.revision, definition: receipt.definition };
      this.baseline = new RoutineEditorBaseline(this.loaded);
      this.message = "例程已提交并读回验证；保存期间的新编辑仍保留"; return true;
    } catch (error) {
      if (this.execution.state === "unknown" || this.execution.busy) this.unknown = true;
      this.message = error instanceof Error ? error.message : String(error); throw error;
    } finally { this.submitting = false; if (!this.unknown) this.activeDraft = null; this.changed(); }
  }
  async stop(): Promise<void> {
    this.stopRequested = true;
    if (this.submitting && !this.execution.busy) { this.message = "准备已请求停止，没有发送SQL"; this.changed(); return; }
    await this.execution.cancel();
  }
  replaceLoaded(snapshot: RoutineSnapshot): void {
    if (this.busy) throw new Error("尚在执行或排空，不能替换编辑基线");
    fingerprint(snapshot);
    if (!sameObject(snapshot.object, this.loaded.object)) throw new Error("不能用另一例程的快照替换当前编辑页");
    this.loaded = structuredClone(snapshot); this.baseline = new RoutineEditorBaseline(snapshot);
    this.unknown = false; this.activeDraft = null; this.response = null; this.message = "已明确重新读取实际目标并重建编辑基线"; this.changed();
  }
  capture(): { snapshot: RoutineSnapshot; execution_state: ExecutionState; execution_effects_unknown: boolean; pending_draft: BoundDraft | null } {
    return { snapshot: this.loadedSnapshot, execution_state: this.state, execution_effects_unknown: this.unknown, pending_draft: this.activeDraft ? structuredClone(this.activeDraft) : null };
  }
  dispose(): void { this.disposed = true; this.execution.dispose(); }
}
