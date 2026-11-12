/** C0/C1 IPC 契约。字段与 Rust execution_contracts.rs 对应；状态由后端裁定。 */
import type { MultiResult } from "./types";
export type ExecutionState = "queued" | "running" | "cancel_requested" | "succeeded" | "failed" | "cancelled" | "timed_out" | "unknown";
export interface ExecutionRequest {
  execution_id: string; connection_id: string; namespace: string; owner_id: string; sql: string;
  transaction_owner?: string | null; timeout_ms?: number | null; origin?: string | null;
  execution_grant?: string | null; cancel_before_send?: boolean;
}
export interface StructuredError { message: string; code: string; operation_id?: string | null; retryable: boolean }
export interface DeadlineInfo { execution_id: string; budget_ms: number | null; elapsed_ms: number; deadline_reached: boolean; stop_confirmed: boolean }
export interface ExecutionOutcome {
  execution_id: string; state: ExecutionState; result: MultiResult | null; error: StructuredError | null;
  deadline: DeadlineInfo; effects: string; transaction_state: string; resources_pending?: boolean;
}
export interface CancelAcknowledgement { execution_id: string; accepted: boolean; state: ExecutionState; message: string }
export interface ExecutionStatus { execution_id: string; state: ExecutionState; deadline: DeadlineInfo; resources_pending?: boolean }
export interface Capability { level: "supported" | "limited" | "unsupported"; reason: string }
export interface Capabilities { version: number; protocol: string; read_only: boolean; arbitrary_sql: Capability; cancel: Capability; deadline: Capability; manual_transaction: Capability; safe_browse: Capability }
export type ExecutionContext = Pick<ExecutionRequest, "execution_id" | "connection_id" | "namespace" | "owner_id">;
export interface ExecutionTransport {
  /** 返回后台签发并绑定窗口/页面/端点/工作内容的完整请求；静态预览可不实现。 */
  issueGrant?(request: ExecutionRequest): Promise<ExecutionRequest>;
  executeSqlV2(request: ExecutionRequest): Promise<ExecutionOutcome>;
  cancelExecution(context: ExecutionContext): Promise<CancelAcknowledgement>;
  executionStatus(context: ExecutionContext): Promise<ExecutionStatus>;
}
export function isExecutionTerminal(state: ExecutionState): boolean { return !["queued", "running", "cancel_requested"].includes(state); }
/** 只读取已知诊断字段，不把未知对象序列化到用户提示（可能包含凭据）。 */
function errorField(error: unknown, key: string): unknown {
  try { return error !== null && typeof error === "object" ? (error as Record<string, unknown>)[key] : undefined; }
  catch { return undefined; }
}
function errorValue(error: unknown): unknown {
  if (typeof error !== "string") return error;
  try {
    const parsed: unknown = JSON.parse(error);
    if (typeof errorField(parsed, "message") === "string" && typeof errorField(parsed, "code") === "string"
        && typeof errorField(parsed, "retryable") === "boolean") return parsed;
  } catch { /* 旧字符串错误保持原文；不是 JSON 并不代表成功。 */ }
  return error;
}
function errorMessage(error: unknown): string {
  const message = errorField(error, "message");
  return typeof message === "string" ? message : typeof error === "string" ? error : "后端返回了无法识别的错误";
}
/** 纯文本格式；调用方必须使用 textContent，不能把后端消息当 HTML。 */
export function formatError(error: unknown): string {
  const value = errorValue(error), details: string[] = [];
  const code = errorField(value, "code"), operation = errorField(value, "operation_id");
  const retryable = errorField(value, "retryable"), command = errorField(value, "command");
  if (typeof code === "string" && code) details.push(`code=${code}`);
  if (typeof operation === "string") details.push(`operation_id=${operation}`);
  if (typeof retryable === "boolean") details.push(`retryable=${retryable}`);
  if (typeof command === "string" && command) details.push(`command=${command}`);
  return errorMessage(value) + (details.length ? `\n[${details.join("; ")}]` : "");
}
export class IpcError extends Error {
  readonly code: string; readonly operation_id?: string | null; readonly retryable: boolean;
  readonly command?: string;
  declare readonly cause: unknown;
  constructor(error: unknown, command?: string) {
    const value = errorValue(error);
    super(errorMessage(value));
    const code = errorField(value, "code"), operation = errorField(value, "operation_id");
    this.name = "IpcError";
    this.code = typeof code === "string" && code ? code : "APPLICATION_ERROR";
    this.operation_id = typeof operation === "string" || operation === null ? operation : undefined;
    this.retryable = errorField(value, "retryable") === true;
    const originalCommand = errorField(value, "command");
    this.command = typeof originalCommand === "string" && originalCommand ? originalCommand : command;
    // 保留原始对象/原始 stack 供诊断，且不让 JSON.stringify 意外导出原始载荷。
    Object.defineProperty(this, "cause", { value: error, configurable: true });
  }
  override toString(): string { return formatError(this); }
}
export function asIpcError(error: unknown, command?: string): IpcError {
  return error instanceof IpcError && (!command || error.command) ? error : new IpcError(error, command);
}
function validateGrant(request: Readonly<ExecutionRequest>, granted: ExecutionRequest): void {
  const sameContext = granted.connection_id === request.connection_id && granted.namespace === request.namespace && granted.owner_id === request.owner_id && granted.sql === request.sql
    && (granted.transaction_owner ?? null) === (request.transaction_owner ?? null) && (granted.timeout_ms ?? null) === (request.timeout_ms ?? null) && (granted.origin ?? null) === (request.origin ?? null);
  const validId = typeof granted.execution_id === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(granted.execution_id) && granted.execution_id !== "00000000-0000-0000-0000-000000000000";
  if (!sameContext || !validId || typeof granted.execution_grant !== "string" || !granted.execution_grant) {
    throw new IpcError({ message: "执行授权上下文或令牌不匹配，没有发送 SQL", code: "EXECUTION_GRANT_CONTEXT_MISMATCH", retryable: false });
  }
}
function rejectedBeforeRegistration(error: unknown): boolean {
  const code = typeof error === "object" && error !== null ? (error as { code?: unknown }).code : null;
  // 这些固定 C1 错误只在注册/消费前返回；Duplicate/Forbidden/传输失败不提供此证明。
  return typeof code === "string" && ["EXECUTION_GRANT_INVALID", "EXECUTION_ID_INVALID", "EXECUTION_CONTEXT_INVALID", "EXECUTION_OWNER_MISMATCH", "EXECUTION_BUDGET_INVALID", "EXECUTION_BUSY", "EXECUTION_QUARANTINED"].includes(code);
}
export class ExecutionController {
  state: ExecutionState = "succeeded";
  /** 最近一次状态读取失败；只提供诊断，不改变执行事实或解除 busy。 */
  statusError: IpcError | null = null;
  private active: Readonly<ExecutionRequest> | null = null;
  private sequence = 0;
  private disposed = false;
  private granting = false;
  private awaitingSettlement = false;
  private pendingCancel = false;
  private cancelling: Promise<CancelAcknowledgement | null> | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;
  constructor(private readonly source: ExecutionTransport, private readonly changed: (state: ExecutionState) => void) {}
  get busy(): boolean { return this.active !== null; }
  private publish(state: ExecutionState): void { this.state = state; if (!this.disposed) this.changed(state); }
  private release(): void { this.clearTimer(); this.active = null; this.pendingCancel = false; this.awaitingSettlement = false; }
  async run(request: ExecutionRequest): Promise<ExecutionOutcome | null> {
    if (this.disposed) throw new Error("执行控制器已关闭");
    if (this.busy) throw new Error("查询仍在执行或等待停止确认/资源排空");
    if (request.timeout_ms != null && (!Number.isInteger(request.timeout_ms) || request.timeout_ms < 0 || request.timeout_ms > 86400000)) throw new Error("查询预算必须为 0..86400000 毫秒整数");
    const sequence = ++this.sequence;
    this.statusError = null;
    this.active = Object.freeze({ ...request }); this.pendingCancel = false; this.cancelling = null; this.awaitingSettlement = false; this.publish("queued");
    let context = this.active;
    let sent = false;
    try {
      if (!context.execution_grant && this.source.issueGrant) {
        this.granting = true;
        const granted = await this.source.issueGrant(context);
        if (this.disposed || sequence !== this.sequence) return null;
        validateGrant(context, granted);
        context = Object.freeze({ ...granted, cancel_before_send: Boolean(context.cancel_before_send || granted.cancel_before_send || this.pendingCancel) });
        this.active = context;
      }
      this.granting = false;
      if (this.disposed || sequence !== this.sequence) return null;
      this.schedulePoll(sequence, context);
      sent = true;
      const result = await this.source.executeSqlV2(context);
      if (this.disposed || sequence !== this.sequence) return null;
      if (result.execution_id !== context.execution_id) throw new IpcError({ message: "执行响应 ID 不匹配，结果未知", code: "EXECUTION_RESULT_MISMATCH", retryable: false });
      if (!isExecutionTerminal(result.state)) throw new Error("执行响应缺少真实终态");
      this.awaitingSettlement = result.resources_pending === true;
      this.publish(result.state); return result;
    } catch (error) {
      if (this.disposed || sequence !== this.sequence) return null;
      // 授权/注册前拒绝可证明未发送 SQL；无法证明未注册的发送后失败只能 Unknown。
      const unsent = !sent || rejectedBeforeRegistration(error);
      this.awaitingSettlement = !unsent;
      this.publish(unsent ? "failed" : "unknown"); throw error;
    } finally {
      if (sequence === this.sequence) { this.granting = false; this.pendingCancel = false; if (!this.awaitingSettlement) this.release(); }
    }
  }
  async cancel(): Promise<CancelAcknowledgement | null> {
    if (!this.active || !this.busy || this.disposed) return null;
    if (this.granting) { this.pendingCancel = true; this.publish("cancel_requested"); return null; }
    if (isExecutionTerminal(this.state)) return null;
    if (this.cancelling) return this.cancelling;
    this.pendingCancel = true;
    const context = this.active, sequence = this.sequence;
    const cancellation: Promise<CancelAcknowledgement | null> = this.source.cancelExecution(context).then(ack => {
      if (!this.disposed && sequence === this.sequence && this.busy && !this.awaitingSettlement) {
        if (ack.execution_id !== context.execution_id) throw new IpcError({ message: "停止回执 ID 不匹配，继续等待本次执行结果", code: "EXECUTION_CANCEL_ACK_MISMATCH", retryable: false });
        this.pendingCancel = false;
        // 停止回执即使携带终态也不是主执行的完整 outcome，不能解锁上下文。
        if (!isExecutionTerminal(ack.state)) this.publish(ack.state);
      }
      return ack;
    }).catch(error => {
      if (this.disposed || sequence !== this.sequence) return null;
      if ((error as { code?: string })?.code === "EXECUTION_NOT_FOUND") return null;
      this.pendingCancel = false; throw error;
    }).finally(() => { if (this.cancelling === cancellation) this.cancelling = null; });
    this.cancelling = cancellation;
    return cancellation;
  }
  private schedulePoll(sequence: number, context: Readonly<ExecutionRequest>): void {
    this.timer = setTimeout(async () => {
      if (sequence !== this.sequence || !this.busy || this.disposed) return;
      try {
        const status = await this.source.executionStatus(context);
        if (sequence === this.sequence && this.busy && !this.disposed && status.execution_id === context.execution_id) {
          this.statusError = null;
          if (this.awaitingSettlement && isExecutionTerminal(status.state) && status.resources_pending === false) {
            // 已收到 outcome 或主传输已断开；只确认资源排空，绝不由状态轮询伪造结果或成功。
            this.release(); this.publish(this.state); return;
          }
          if (!this.awaitingSettlement && !isExecutionTerminal(status.state)) this.publish(status.state);
          if (this.pendingCancel && !isExecutionTerminal(status.state)) await this.cancel();
        }
      } catch (error) {
        if (sequence !== this.sequence || !this.busy || this.disposed) return;
        const diagnostic = asIpcError(error, "execution_status");
        // 同一错误连续出现时只记一次，避免每 200ms 刷屏；错误对象始终保留。
        if (!this.statusError || formatError(this.statusError) !== formatError(diagnostic)) console.error("执行状态读取失败，保留执行上下文，不自动重放", diagnostic);
        this.statusError = diagnostic;
      }
      if (sequence === this.sequence && this.busy && !this.disposed) this.schedulePoll(sequence, context);
    }, 200);
  }
  private clearTimer(): void { if (this.timer !== null) clearTimeout(this.timer); this.timer = null; }
  dispose(): void { this.disposed = true; ++this.sequence; this.clearTimer(); this.active = null; this.pendingCancel = false; this.awaitingSettlement = false; }
}
