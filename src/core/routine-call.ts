/** PL-22前端只是既有ExecutionController的调用者；终态/预算/停止不另实现。 */
import type { DbValue } from "./api";
import { ExecutionController, type ExecutionRequest, type ExecutionTransport, type ExecutionOutcome, type Capability } from "./execution";
import { toDbValue } from "./mutation";
import { objectKey, requireRoutineSourceFingerprint, type DatabaseObjectId } from "./object-catalog";

export interface RoutineParameter {
  ordinal: number; name: string | null; mode: "in" | "out" | "inout"; data_type: string;
  type_schema: string | null; type_name: string; has_default: boolean | null; variadic: boolean;
}
export interface RoutineCallMetadata {
  object: DatabaseObjectId; backend_object_id: string | null; revision: string; source_fingerprint: string | null; parameters: RoutineParameter[];
  result_type: string | null; definition_revision: string; completeness: string; call: Capability; notes: string[];
}
export type RoutineArgumentValue = { mode: "value"; value: DbValue } | { mode: "default" };
export interface RoutineCallDraft {
  object: DatabaseObjectId; metadata_revision: string; source_fingerprint: string | null;
  arguments: { ordinal: number; argument: RoutineArgumentValue }[];
}
export interface RoutineCallPreview {
  plan_id: string; metadata_revision: string; source_fingerprint: string | null; object: DatabaseObjectId; sql_preview: string;
  parameter_count: number; expires_in_ms: number; warnings: string[];
}
export interface TypedResultSet { columns: { name: string; data_type: string }[]; rows: DbValue[][]; affected: number }
export interface RoutineCallDetails {
  sets: TypedResultSet[]; outputs: { ordinal: number; name: string; value: DbValue; data_type: string }[]; warnings: string[];
}
export interface RoutineCallResponse { outcome: ExecutionOutcome; call: RoutineCallDetails | null }
export interface RoutineCallPort extends Pick<ExecutionTransport, "cancelExecution" | "executionStatus"> {
  issueGrant(request: ExecutionRequest): Promise<ExecutionRequest>;
  metadata(object: DatabaseObjectId): Promise<RoutineCallMetadata>;
  prepare(draft: RoutineCallDraft, ownerId: string): Promise<RoutineCallPreview>;
  execute(planId: string, request: ExecutionRequest): Promise<RoutineCallResponse>;
}
export interface ParameterInput { mode: "value" | "null" | "default"; text: string }
function inputType(parameter: RoutineParameter): string {
  const type = parameter.type_name;
  if (/^(tinyblob|mediumblob|longblob|varbinary|binary|blob|bytea)$/i.test(type)) return "blob";
  if (/^(tinytext|mediumtext|longtext|bpchar)$/i.test(type)) return "text";
  return parameter.data_type;
}
export function parameterArgument(parameter: RoutineParameter, input: ParameterInput): RoutineArgumentValue {
  if (parameter.mode === "out") throw new Error("OUT参数不接受输入");
  if (input.mode === "default") {
    if (parameter.mode !== "in" || parameter.has_default !== true) throw new Error("参数默认值未知或不可省略，不能把未知当默认值");
    return { mode: "default" };
  }
  return { mode: "value", value: input.mode === "null" ? { kind: "null" } : toDbValue(input.text, inputType(parameter)) };
}
export class RoutineCallController {
  metadata: RoutineCallMetadata | null = null;
  preview: RoutineCallPreview | null = null;
  response: RoutineCallResponse | null = null;
  error: string | null = null;
  loading = false;
  preparing = false;
  private closed = false;
  private sequence = 0;
  private inputs = new Map<number, ParameterInput>();
  private sourceFingerprint: string | null = null;
  readonly execution: ExecutionController;
  private readonly target: DatabaseObjectId;
  constructor(object: DatabaseObjectId, readonly ownerId: string, private readonly port: RoutineCallPort, private readonly changed: () => void = () => {}) {
    this.target = structuredClone(object);
    if (!ownerId || typeof port.issueGrant !== "function") throw new Error("调用必须归属真实页面并接入C1授权");
    this.execution = new ExecutionController({
      issueGrant: port.issueGrant.bind(port), cancelExecution: port.cancelExecution.bind(port), executionStatus: port.executionStatus.bind(port),
      executeSqlV2: async request => {
        const plan = this.preview;
        if (!plan || request.sql !== `ROUTINE_CALL_PLAN ${plan.plan_id}`) throw new Error("例程计划身份已变化，没有发送SQL");
        if (requireRoutineSourceFingerprint(plan.source_fingerprint) !== this.sourceFingerprint) throw new Error("ROUTINE_SOURCE_CHANGED: 预览端点与参数加载时不同，没有发送SQL");
        const response = await port.execute(plan.plan_id, request);
        if (!this.closed && response.outcome.execution_id === request.execution_id && response.outcome.state === "succeeded" && response.outcome.resources_pending === false) this.response = response;
        return response.outcome;
      },
    }, () => { if (!this.closed) this.changed(); });
  }
  get busy(): boolean { return this.loading || this.preparing || this.execution.busy; }
  get object(): DatabaseObjectId { return structuredClone(this.target); }
  get callable(): boolean { return !!this.metadata && this.sourceFingerprint !== null && this.metadata.source_fingerprint === this.sourceFingerprint && this.metadata.completeness === "complete" && this.metadata.call.level !== "unsupported"; }
  input(ordinal: number): ParameterInput { return { ...(this.inputs.get(ordinal) ?? { mode: "value", text: "" }) }; }
  setInput(ordinal: number, input: ParameterInput): void {
    if (this.closed || this.busy) throw new Error("调用仍在进行或窗口已关闭");
    const parameter = this.metadata?.parameters.find(p => p.ordinal === ordinal);
    if (!parameter || parameter.mode === "out") throw new Error("输入不属于当前IN/INOUT参数");
    this.inputs.set(ordinal, { ...input }); this.preview = null; this.response = null; this.error = null; this.changed();
  }
  async load(): Promise<void> {
    if (this.closed || this.busy) throw new Error("调用仍在进行或窗口已关闭");
    const sequence = ++this.sequence; this.loading = true; this.sourceFingerprint = null; this.error = null; this.preview = null; this.response = null; this.changed();
    try {
      const metadata = await this.port.metadata(this.object);
      if (this.closed || sequence !== this.sequence) return;
      if (objectKey(metadata.object) !== objectKey(this.target) || !metadata.revision) throw new Error("例程元数据身份或revision不匹配");
      const sourceFingerprint = requireRoutineSourceFingerprint(metadata.source_fingerprint);
      const seen = new Set<number>();
      for (const p of metadata.parameters) { if (!Number.isInteger(p.ordinal) || p.ordinal <= 0 || seen.has(p.ordinal) || !["in", "out", "inout"].includes(p.mode)) throw new Error("参数顺序或方向metadata未知"); seen.add(p.ordinal); }
      this.sourceFingerprint = sourceFingerprint; this.metadata = structuredClone(metadata); this.inputs.clear();
      for (const p of metadata.parameters) if (p.mode !== "out") this.inputs.set(p.ordinal, { mode: "value", text: "" });
    } catch (error) { if (!this.closed && sequence === this.sequence) { this.metadata = null; this.sourceFingerprint = null; this.error = error instanceof Error ? error.message : String(error); } throw error; }
    finally { if (!this.closed && sequence === this.sequence) { this.loading = false; this.changed(); } }
  }
  draft(): RoutineCallDraft {
    if (!this.callable || !this.metadata) throw new Error(this.metadata?.call.reason ?? "尚未读取真实参数metadata");
    return { object: this.object, metadata_revision: this.metadata.revision, source_fingerprint: requireRoutineSourceFingerprint(this.sourceFingerprint),
      arguments: this.metadata.parameters.filter(p => p.mode !== "out").map(p => ({ ordinal: p.ordinal, argument: parameterArgument(p, this.input(p.ordinal)) })) };
  }
  async prepare(): Promise<RoutineCallPreview | null> {
    if (this.closed || this.busy) throw new Error("调用仍在进行或窗口已关闭");
    const draft = this.draft(), sequence = this.sequence; this.preparing = true; this.preview = null; this.error = null; this.response = null; this.changed();
    try {
      const plan = await this.port.prepare(draft, this.ownerId);
      if (this.closed || sequence !== this.sequence) return null;
      if (objectKey(plan.object) !== objectKey(this.target) || plan.metadata_revision !== draft.metadata_revision || !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(plan.plan_id)) throw new Error("例程预览身份/参数基线不匹配，未执行SQL");
      if (requireRoutineSourceFingerprint(plan.source_fingerprint) !== draft.source_fingerprint) throw new Error("ROUTINE_SOURCE_CHANGED: 预览端点与参数加载时不同，没有发送SQL");
      this.preview = structuredClone(plan); return this.preview;
    } catch (error) { if (!this.closed) this.error = error instanceof Error ? error.message : String(error); throw error; }
    finally { if (!this.closed && sequence === this.sequence) { this.preparing = false; this.changed(); } }
  }
  async execute(timeoutMs: number | null = null): Promise<ExecutionOutcome | null> {
    if (this.closed || this.busy || !this.preview) throw new Error("请先预览当前参数计划，且等待前次执行/资源清理结束");
    const plan = this.preview;
    if (requireRoutineSourceFingerprint(plan.source_fingerprint) !== requireRoutineSourceFingerprint(this.sourceFingerprint)) throw new Error("ROUTINE_SOURCE_CHANGED: 预览端点与参数加载时不同，没有发送SQL");
    this.error = null; this.response = null;
    // C1签发真实身份；此临时空ID不具有授权能力，不传入数据库。
    const request: ExecutionRequest = { execution_id: "", connection_id: this.target.connection_id, namespace: this.target.schema, owner_id: this.ownerId,
      sql: `ROUTINE_CALL_PLAN ${plan.plan_id}`, transaction_owner: null, timeout_ms: timeoutMs, origin: "routine", execution_grant: null, cancel_before_send: false };
    try {
      const outcome = await this.execution.run(request);
      if (!this.closed && outcome && outcome.state !== "succeeded") { this.response = null; this.error = `${outcome.error?.message ?? outcome.state}；状态 ${outcome.state}，effects=${outcome.effects}，不能推断已回滚`; }
      return outcome;
    } catch (error) { if (!this.closed) { this.response = null; this.error = error instanceof Error ? error.message : String(error); } throw error; }
    finally { if (!this.closed) { this.preview = null; this.changed(); } }
  }
  cancel(): ReturnType<ExecutionController["cancel"]> { return this.execution.cancel(); }
  dispose(): void {
    if (this.closed) return;
    // 已发送的执行只请求C1停止；不abort传输，也不把关闭窗口当作服务端取消成功。
    void this.execution.cancel().catch(() => {}); this.closed = true; ++this.sequence; this.execution.dispose();
  }
}
