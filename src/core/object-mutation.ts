/** PL-19: backend-owned SQLite/main trigger DROP; no client DDL fallback. */
import type { DatabaseObjectId } from "./object-catalog";
import { objectKey } from "./object-catalog";
import { ExecutionController, IpcError, type ExecutionTransport, type ExecutionRequest, type ExecutionOutcome } from "./execution";
export interface ObjectMutationBaseline {
  baseline_id: string; object: DatabaseObjectId; source_fingerprint: string; base_revision: string;
  backend_object_id: string; metadata_state: string; expires_in_ms: number;
}
export interface ObjectMutationDraft {
  baseline_id: string; object: DatabaseObjectId; source_fingerprint: string; base_revision: string; operation: "drop";
}
export interface ObjectMutationPreview {
  plan_id: string; baseline_id: string; object: DatabaseObjectId; source_fingerprint: string; base_revision: string;
  review_digest: string; request_sql: string; expires_in_ms: number; confirmation_required: boolean; transaction: string;
}
export interface ObjectMutationWork { plan_id: string; review_digest: string; draft: ObjectMutationDraft; confirm_drop: boolean }
export interface ObjectMutationReceipt {
  plan_id: string; baseline_id: string; object: DatabaseObjectId; backend_object_id: string;
  source_fingerprint: string; before_revision: string; after_revision: string; status: string;
  verified: boolean; transaction: string; mismatches: string[];
}
export interface ObjectMutationResponse { outcome: ExecutionOutcome; receipt: ObjectMutationReceipt | null }
export interface ObjectMutationPort extends Pick<ExecutionTransport, "issueGrant" | "cancelExecution" | "executionStatus"> {
  readObjectMutationBaseline(object: DatabaseObjectId, ownerId: string): Promise<ObjectMutationBaseline>;
  previewObjectMutation(draft: ObjectMutationDraft, ownerId: string): Promise<ObjectMutationPreview>;
  executeObjectMutation(request: ExecutionRequest, work: ObjectMutationWork): Promise<ObjectMutationResponse>;
}
const sha = (value: string): boolean => typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
const uuid = (value: string): boolean => typeof value === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value) && value !== "00000000-0000-0000-0000-000000000000";
const clone = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
export function objectDropReason(object: DatabaseObjectId, readOnly: boolean): string | null {
  if (readOnly) return "此连接只读，禁止删除对象";
  if (object.kind !== "trigger" || object.schema !== "main" || object.catalog !== null || object.identity_arguments !== null || object.parent?.schema !== "main" || object.parent?.kind !== "table") return "只认证原生 SQLite main 用户触发器及完整父表身份";
  if (!object.name || !object.parent.name || [object.name, object.parent.name].some(name => name.includes("\0") || name.toLowerCase().startsWith("sqlite_"))) return "内部或无效对象禁止删除";
  return null;
}
export class ObjectMutationSession {
  private loaded: ObjectMutationBaseline | null = null;
  private reviewed: ObjectMutationPreview | null = null;
  get baseline(): ObjectMutationBaseline | null { return this.loaded ? clone(this.loaded) : null; }
  get preview(): ObjectMutationPreview | null { return this.reviewed ? clone(this.reviewed) : null; }
  locked = false;
  private working = false;
  private disposed = false;
  private readonly target: DatabaseObjectId;
  private activeWork: ObjectMutationWork | null = null;
  private response: ObjectMutationResponse | null = null;
  private sent = false;
  readonly execution: ExecutionController;
  constructor(object: DatabaseObjectId, readonly ownerId: string, private readonly port: ObjectMutationPort, private readonly changed: () => void) {
    const reason = objectDropReason(object, false);
    if (reason || !ownerId.trim() || !port.issueGrant) throw new Error(reason || "必须有真实页面与 C1 授权接口");
    this.target = clone(object);
    this.execution = new ExecutionController({
      issueGrant: request => port.issueGrant!(request), cancelExecution: context => port.cancelExecution(context), executionStatus: context => port.executionStatus(context),
      executeSqlV2: async request => {
        if (!this.activeWork) throw new Error("对象删除必须携带后端计划");
        this.sent = true;
        try {
          this.response = await port.executeObjectMutation(request, clone(this.activeWork));
          return this.response.outcome;
        } catch (error) {
          if ((error as { code?: string })?.code === "OBJECT_MUTATION_NOT_REGISTERED") {
            this.sent = false;
            throw new IpcError({ code: "EXECUTION_CONTEXT_INVALID", message: String(error), retryable: false });
          }
          throw error; // Transport loss is not no-send evidence.
        }
      },
    }, changed);
  }
  get busy(): boolean { return this.working || this.execution.busy; }
  async load(): Promise<void> {
    if (this.busy || this.locked || this.disposed) throw new Error("操作仍在执行或结果需要核对，禁止替换基线");
    this.working = true; this.changed();
    try {
      const baseline = await this.port.readObjectMutationBaseline(clone(this.target), this.ownerId);
      if (this.disposed) return;
      if (objectDropReason(baseline.object, false) || objectKey(baseline.object) !== objectKey(this.target) || !uuid(baseline.baseline_id) || !sha(baseline.source_fingerprint) || !sha(baseline.base_revision) || !baseline.backend_object_id || baseline.metadata_state !== "known" || !(baseline.expires_in_ms > 0)) throw new Error("对象基线身份或目录证明不完整，没有发送删除 SQL");
      this.loaded = clone(baseline); this.reviewed = null;
    } finally { this.working = false; this.changed(); }
  }
  async prepare(): Promise<ObjectMutationPreview> {
    if (this.busy || this.locked || this.disposed || !this.baseline) throw new Error("请先读取完整对象基线；未知结果禁止重放");
    // An explicit re-preview invalidates the former authorization view even if the read fails.
    this.reviewed = null; this.working = true; this.changed();
    try {
      const draft = this.draft();
      const preview = await this.port.previewObjectMutation(clone(draft), this.ownerId);
      if (this.disposed) throw new Error("对象操作页面已关闭，没有发送 SQL");
      if (!uuid(preview.plan_id) || preview.baseline_id !== draft.baseline_id || objectKey(preview.object) !== objectKey(draft.object) || preview.source_fingerprint !== draft.source_fingerprint || preview.base_revision !== draft.base_revision || !sha(preview.review_digest) || preview.request_sql !== `OBJECT_MUTATION_DROP ${preview.plan_id} ${preview.review_digest}` || !(preview.expires_in_ms > 0) || preview.confirmation_required !== true || preview.transaction !== "transactional") throw new Error("对象预览与打开时的基线不匹配，没有发送删除 SQL");
      this.reviewed = clone(preview); return clone(preview);
    } finally { this.working = false; this.changed(); }
  }
  private draft(): ObjectMutationDraft {
    const b = this.baseline!;
    return { baseline_id: b.baseline_id, object: clone(b.object), source_fingerprint: b.source_fingerprint, base_revision: b.base_revision, operation: "drop" };
  }
  async submit(confirmed: boolean): Promise<ObjectMutationResponse> {
    if (!confirmed || !this.preview || this.busy || this.locked || this.disposed) throw new Error("必须确认固定预览；已执行或未知计划禁止重放");
    const p = clone(this.preview), b = clone(this.baseline!);
    const work: ObjectMutationWork = { plan_id: p.plan_id, review_digest: p.review_digest, draft: this.draft(), confirm_drop: true };
    this.activeWork = work; this.response = null; this.sent = false;
    // The controller owns grant/cancel/drain; only its typed adapter may send this plan.
    const controller = this.execution;
    try {
      const outcome = await controller.run({ execution_id: crypto.randomUUID(), connection_id: b.object.connection_id, namespace: b.object.schema, owner_id: this.ownerId, sql: p.request_sql, origin: "object_drop", transaction_owner: null });
      if (!outcome || !this.response) throw new Error("未取得完整执行回执");
      const actual = this.response as ObjectMutationResponse;
      if (outcome.state === "unknown" || outcome.resources_pending) throw new Error("提交或资源排空结果未知；禁止重放，请从真实目录核对目标");
      if (outcome.state !== "succeeded") {
        if (outcome.effects !== "no_effect" || !["none", "rolled_back"].includes(outcome.transaction_state)) this.locked = true;
        this.reviewed = null;
        throw new Error(outcome.error?.message || "对象删除未成功，原始基线保留");
      }
      const r = actual.receipt;
      if (!r || r.plan_id !== p.plan_id || r.baseline_id !== b.baseline_id || objectKey(r.object) !== objectKey(b.object) || r.backend_object_id !== b.backend_object_id || r.source_fingerprint !== b.source_fingerprint || r.before_revision !== b.base_revision || !sha(r.after_revision) || r.after_revision === r.before_revision || !r.verified || r.transaction !== "committed" || r.status !== "applied" || r.mismatches.length !== 0 || outcome.transaction_state !== "committed") throw new Error("删除成功回执缺少同一目标的读回证明；禁止重放");
      this.locked = true; return actual;
    } catch (error) {
      if (this.sent && (controller.state === "unknown" || controller.busy)) this.locked = true;
      throw error;
    } finally {
      if (this.sent) this.locked = true; // Every submitted one-use plan requires explicit real target review, even after rollback.
      this.activeWork = null;
    }
  }
  cancel(): ReturnType<ExecutionController["cancel"]> { return this.execution.cancel(); }
  dispose(): void { this.disposed = true; this.execution.dispose(); }
}
