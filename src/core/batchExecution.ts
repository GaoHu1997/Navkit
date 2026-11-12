/** 受控批任务的窗口内持久宿主。视图只订阅/脱离，不持有或重放执行票据。 */
import { api, type BatchExecutionRequest, type BatchExecutionTicket, type BatchExecutionSnapshot, type BatchExecutionLog, type BatchTaskKind } from "./api";
import { isExecutionTerminal, type ExecutionContext, type ExecutionState, type CancelAcknowledgement } from "./execution";

export type BatchExecutionTransport = Pick<typeof api, "registerBatchExecution" | "runBatchExecution" | "batchExecutionStatus" | "cancelExecution">;
export interface BatchExecutionRecord {
  readonly id: string;
  readonly kind: BatchTaskKind;
  readonly connectionName: string;
  /** 数据库驱动名（MySQL/PostgreSQL…），仅用于展开态的元信息行。 */
  readonly driver: string;
  readonly namespace: string;
  readonly createdAt: number;
  readonly context: ExecutionContext | null;
  readonly summary: string;
  readonly state: ExecutionState;
  readonly registering: boolean;
  /** 仅由真实终态且资源排空释放；停止 ack 和关闭视图都不能释放。 */
  readonly busy: boolean;
  readonly unknown: boolean;
  readonly cancelRequested: boolean;
  readonly notice: string;
  readonly snapshot: BatchExecutionSnapshot | null;
  readonly logs: readonly BatchExecutionLog[];
  readonly droppedBeforeSequence: number;
  readonly locallyDropped: number;
  readonly totalRows: number | null;
  readonly sourceBytes: number | null;
}
interface Entry {
  record: BatchExecutionRecord;
  context: ExecutionContext | null;
  runInvoked: boolean;
  cursor: number;
  timer: ReturnType<typeof setTimeout> | null;
  cancelling: Promise<CancelAcknowledgement | null> | null;
}
export interface BatchExecutionServiceOptions {
  pollIntervalMs?: number;
  logLimit?: number;
  logByteLimit?: number;
  historyLimit?: number;
  schedule?: (callback: () => void, delay: number) => ReturnType<typeof setTimeout>;
  clearSchedule?: (timer: ReturnType<typeof setTimeout>) => void;
}
const stateNames: Record<ExecutionState, string> = {
  queued: "等待执行", running: "执行中", cancel_requested: "停止请求处理中", succeeded: "已完成",
  failed: "执行失败", cancelled: "已停止", timed_out: "已超时", unknown: "结果未知",
};
export function batchStateLabel(state: ExecutionState): string { return stateNames[state]; }
function plainText(value: string, limit = 240): string { return value.replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/g, "").slice(0, limit); }
function sameContext(a: ExecutionContext, b: ExecutionContext): boolean {
  return a.execution_id === b.execution_id && a.connection_id === b.connection_id && a.namespace === b.namespace && a.owner_id === b.owner_id;
}
function errorCode(error: unknown): string {
  const code = typeof error === "object" && error !== null ? (error as { code?: unknown }).code : null;
  return typeof code === "string" && /^[A-Z0-9_]{1,80}$/.test(code) ? `（${code}）` : "";
}
function validateTicket(request: BatchExecutionRequest, ticket: BatchExecutionTicket): void {
  if (!ticket || ticket.connection_id !== request.connection_id || ticket.namespace !== request.namespace || ticket.owner_id !== request.owner_id
      || ticket.task_kind !== request.task.kind || !/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(ticket.execution_id)
      || ticket.execution_id === "00000000-0000-0000-0000-000000000000" || !ticket.execution_grant
      || !Number.isFinite(ticket.expires_in_ms) || ticket.expires_in_ms <= 0 || typeof ticket.summary !== "string"
      || ticket.total_rows !== null && !validCount(ticket.total_rows)
      || ticket.source_bytes !== null && !validCount(ticket.source_bytes)) throw new Error("批任务授权上下文不匹配，未发送执行");
}
function validCount(value: number): boolean { return Number.isSafeInteger(value) && value >= 0; }
function validateSnapshot(context: ExecutionContext, kind: BatchTaskKind, snapshot: BatchExecutionSnapshot): void {
  if (!snapshot?.context || !sameContext(context, snapshot.context) || snapshot.kind !== kind
      || snapshot.status?.execution_id !== context.execution_id || !(snapshot.status.state in stateNames)
      || snapshot.outcome && snapshot.outcome.execution_id !== context.execution_id
      || !snapshot.metrics || !Object.values(snapshot.metrics).every(validCount)
      || ![snapshot.metrics.scanned, snapshot.metrics.executed, snapshot.metrics.errors, snapshot.metrics.elapsed_ms, snapshot.affected_rows].every(validCount)
      || snapshot.total_rows !== null && !validCount(snapshot.total_rows)
      || !["none", "active", "committed", "rolled_back", "unknown"].includes(snapshot.transaction_state)
      || typeof snapshot.scan_complete !== "boolean"
      || !validCount(snapshot.next_sequence) || !validCount(snapshot.dropped_before_sequence)
      || !Array.isArray(snapshot.logs)) throw new Error("批任务状态上下文不匹配；不能确认本次结果");
}
/** 错误码只接受短 ASCII 标识：数据库原生码（1062/SQLITE_CONSTRAINT）与后端分类码都符合，其余一律丢弃。 */
function safeErrorCode(value: unknown): string | null {
  return typeof value === "string" && /^[A-Za-z0-9_]{1,32}$/.test(value) ? value : null;
}
function safeDuration(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= Number.MAX_SAFE_INTEGER ? Math.floor(value) : undefined;
}
/** 展示数据库/扫描器的错误原文，同时说明执行与事务状态。错误始终作为文本渲染。 */
export function failureDiagnosis(snapshot: BatchExecutionSnapshot): string {
  const { state, deadline } = snapshot.status, metrics = snapshot.metrics;
  const error = snapshot.outcome?.error;
  const rawError = typeof error?.message === "string" ? error.message : "";
  const reason = rawError ? "\n原因：" + rawError : "";
  const applied: string[] = [];
  if (validCount(metrics.scanned)) applied.push("已扫描 " + metrics.scanned + " 条");
  if (validCount(metrics.executed)) applied.push("已执行 " + metrics.executed + " 条");
  if (validCount(metrics.errors) && metrics.errors > 0) applied.push("失败 " + metrics.errors + " 条");
  const counts = applied.length ? applied.join("，") : "没有完成的执行回执";
  const tail = snapshot.scan_complete ? "" : "；脚本尾部未执行";
  const settled = ({ none: "无整体事务", active: "事务进行中", committed: "提交已确认", rolled_back: "回滚已确认", unknown: "事务结果未知" } as Record<string, string>)[snapshot.transaction_state] ?? snapshot.transaction_state;
  if (state === "unknown" || snapshot.transaction_state === "unknown") return "结果未知；" + counts + "，" + settled + "。请核对目标数据，不提供重放。" + reason;
  if (state === "timed_out" || deadline?.deadline_reached) {
    const budget = deadline?.budget_ms;
    const limit = typeof budget === "number" && budget > 0 ? (budget / 1000).toFixed(1) + " 秒" : "配置";
    return "执行超时：超过 " + limit + "的执行时限" + (deadline?.stop_confirmed ? "，后台停止已确认" : "，停止尚未确认") + "；" + counts + tail + "；" + settled + "。" + reason;
  }
  if (state === "cancelled") return "已停止；" + counts + tail + "；" + settled + "。" + reason;
  if (state === "failed") {
    const code = safeErrorCode(error?.code);
    return "执行失败" + (code ? "（代码 " + code + "）" : "") + "；" + counts + tail + "；" + settled + "。" + reason;
  }
  return "";
}
/** 校验日志结构，但不改写数据库错误原文。SQL 模板保留现有参数化显示。 */
function safeLog(log: BatchExecutionLog): BatchExecutionLog | null {
  if (!validCount(log.sequence) || !["info", "success", "error"].includes(log.level)
      || !["queued", "scan", "execute", "commit", "rollback", "finish"].includes(log.phase)) return null;
  const messages = {
    queued: "任务已登记，等待后台执行", scan: "扫描到完整可执行语句", execute: "语句执行回执",
    commit: "事务提交回执", rollback: "事务回滚回执", finish: "任务结束回执",
  };
  let template: string | null = null;
  if (typeof log.sql_template === "string") {
    template = plainText(log.sql_template
      .replace(/\/\*[\s\S]*?\*\//g, " ").replace(/--[^\r\n]*/g, " ")
      .replace(/\$(\w*)\$[\s\S]*?\$\1\$/g, "?").replace(/'(?:''|\\.|[^'\\])*'/g, "?")
      .replace(/\$\d+|\b\d+(?:\.\d+)?\b/g, value => value.startsWith("$") ? value : "?"), 2048);
  }
  const code = safeErrorCode(log.error_code);
  const message = typeof log.message === "string" && log.message.length > 0
    ? log.message : messages[log.phase] + (log.level === "error" ? "失败" : "");
  return { sequence: log.sequence, level: log.level, phase: log.phase,
    statement: log.statement !== null && validCount(log.statement) ? log.statement : null,
    row: log.row !== null && validCount(log.row) ? log.row : null,
    message, sql_template: template,
    ts_ms: safeDuration(log.ts_ms), elapsed_ms: safeDuration(log.elapsed_ms),
    affected_rows: safeDuration(log.affected_rows), error_code: code };
}
function safeSnapshot(snapshot: BatchExecutionSnapshot): BatchExecutionSnapshot {
  // 结果集不用于批任务视图；错误消息与解析详情必须原样保留。
  return { ...snapshot, logs: [],
    parse_error: typeof snapshot.parse_error === "string" ? snapshot.parse_error : null,
    outcome: snapshot.outcome ? { ...snapshot.outcome, result: null,
      error: snapshot.outcome.error ? { ...snapshot.outcome.error } : null } : null };
}

export class BatchExecutionService {
  private readonly entries = new Map<string, Entry>();
  private readonly listeners = new Set<() => void>();
  private readonly interval: number;
  private readonly logLimit: number;
  private readonly byteLimit: number;
  private readonly historyLimit: number;
  private readonly schedule: NonNullable<BatchExecutionServiceOptions["schedule"]>;
  private readonly clearSchedule: NonNullable<BatchExecutionServiceOptions["clearSchedule"]>;
  constructor(private readonly source: BatchExecutionTransport, options: BatchExecutionServiceOptions = {}) {
    this.interval = Math.min(500, Math.max(300, options.pollIntervalMs ?? 400));
    this.logLimit = Math.min(300, Math.max(1, options.logLimit ?? 300));
    this.byteLimit = Math.min(96 * 1024, Math.max(256, options.logByteLimit ?? 96 * 1024));
    this.historyLimit = Math.max(1, options.historyLimit ?? 30);
    this.schedule = options.schedule ?? ((callback, delay) => setTimeout(callback, delay));
    this.clearSchedule = options.clearSchedule ?? (timer => clearTimeout(timer));
  }
  list(): readonly BatchExecutionRecord[] { return [...this.entries.values()].map(entry => entry.record).reverse(); }
  get(id: string): BatchExecutionRecord | null { return this.entries.get(id)?.record ?? null; }
  subscribe(listener: () => void): () => void { this.listeners.add(listener); return () => this.listeners.delete(listener); }
  private publish(entry: Entry, patch: Partial<BatchExecutionRecord>): void {
    entry.record = { ...entry.record, ...patch };
    for (const listener of this.listeners) { try { listener(); } catch (error) { console.error("批任务视图更新失败", error); } }
  }
  /** 同步返回记录，注册期间也可回看/请求停止；真实 payload 只发送给 register，绝不留给重试按钮。 */
  start(request: BatchExecutionRequest, connectionName = request.connection_id, driver = ""): BatchExecutionRecord {
    const captured = structuredClone(request);
    const record: BatchExecutionRecord = {
      id: crypto.randomUUID(), kind: request.task.kind, connectionName: plainText(connectionName), driver: plainText(driver, 24), namespace: request.namespace, createdAt: Date.now(),
      context: null, summary: request.task.kind === "generated_rows" ? "生成数据写入" : "运行 SQL 文件", state: "queued",
      registering: true, busy: true, unknown: false, cancelRequested: false, notice: "正在登记真实任务，尚未发送执行…",
      snapshot: null, logs: [], droppedBeforeSequence: 0, locallyDropped: 0, totalRows: null, sourceBytes: null,
    };
    const entry: Entry = { record, context: null, runInvoked: false, cursor: 0, timer: null, cancelling: null };
    this.entries.set(record.id, entry); this.trimHistory(); this.publish(entry, {});
    // 捕获点击时的 scope 和数据；后续改配置、后台脱离都不能改变这次任务。
    void this.registerAndRun(entry, captured);
    return entry.record;
  }
  private async registerAndRun(entry: Entry, request: BatchExecutionRequest): Promise<void> {
    let ticket: BatchExecutionTicket;
    try {
      ticket = await this.source.registerBatchExecution(request);
      validateTicket(request, ticket);
    } catch (error) {
      this.publish(entry, { registering: false, busy: false, state: "failed", notice: `任务登记失败；没有发送执行${errorCode(error)}。重新执行必须重新预览/确认。` });
      return;
    }
    entry.context = Object.freeze({ execution_id: ticket.execution_id, connection_id: ticket.connection_id, namespace: ticket.namespace, owner_id: ticket.owner_id });
    this.publish(entry, { context: entry.context, registering: false, summary: plainText(ticket.summary), totalRows: ticket.total_rows, sourceBytes: ticket.source_bytes, notice: "任务已登记，正在启动后台执行…" });
    if (entry.record.cancelRequested) {
      // 登记期间的停止先作用于已登记 context；绝不消费票据启动已撤销任务。
      void this.cancel(entry.record.id); this.schedulePoll(entry); return;
    }
    // 每张票据唯一的一次 run：传输失败只能查询同一 context，绝不能消费/注册另一张票据重放。
    if (entry.runInvoked) return;
    entry.runInvoked = true;
    try {
      const started = this.source.runBatchExecution(ticket);
      this.schedulePoll(entry);
      if (entry.record.cancelRequested) void this.cancel(entry.record.id);
      this.merge(entry, await started);
    } catch (error) {
      // 若轮询已经给出真实终态，不用迟到的 start 传输异常覆盖该回执。
      if (!entry.record.snapshot || !isExecutionTerminal(entry.record.snapshot.status.state)) {
        this.publish(entry, { registering: false, state: "unknown", unknown: true, busy: true,
          notice: `启动回执未确认，结果未知${errorCode(error)}；正在回看同一任务，不会自动重放。` });
      }
    } finally { this.schedulePoll(entry); }
  }
  private merge(entry: Entry, input: BatchExecutionSnapshot): void {
    if (!entry.context) return;
    validateSnapshot(entry.context, entry.record.kind, input);
    const old = entry.record.snapshot;
    const stale = old && (input.next_sequence < old.next_sequence || input.metrics.elapsed_ms < old.metrics.elapsed_ms
      || isExecutionTerminal(old.status.state) && !isExecutionTerminal(input.status.state)
      || old.status.state === "unknown" && input.status.state !== "unknown"
      || !entry.record.busy && (input.status.resources_pending !== false || input.outcome?.resources_pending === true));
    const snapshot = stale ? old : safeSnapshot(input);
    const diagnosis = failureDiagnosis(snapshot);
    const bySequence = new Map(entry.record.logs.map(log => [log.sequence, log]));
    for (const raw of input.logs) {
      if (raw.sequence <= entry.cursor) continue;
      const log = safeLog(raw);
      if (log) bySequence.set(log.sequence, log);
    }
    entry.cursor = Math.max(entry.cursor, input.next_sequence);
    const logs = [...bySequence.values()].sort((a, b) => a.sequence - b.sequence);
    let bytes = logs.reduce((total, log) => total + (log.message.length + (log.sql_template?.length ?? 0) + 96) * 3, 0);
    let removed = 0;
    while (logs.length > this.logLimit || bytes > this.byteLimit) {
      const log = logs.shift(); if (!log) break;
      bytes -= (log.message.length + (log.sql_template?.length ?? 0) + 96) * 3; removed++;
    }
    const unknown = snapshot.status.state === "unknown" || snapshot.transaction_state === "unknown" || snapshot.outcome?.state === "unknown";
    const busy = !isExecutionTerminal(snapshot.status.state) || snapshot.status.resources_pending !== false || snapshot.outcome?.resources_pending === true;
    const terminal = isExecutionTerminal(snapshot.status.state);
    const notice = [
      unknown || terminal && snapshot.status.state !== "succeeded" ? diagnosis : "",
      terminal && busy ? "已收到终态，正在等待事务确认与资源排空…" : "",
      snapshot.parse_error && !diagnosis.includes(snapshot.parse_error) ? "SQL 文件解析失败：" + snapshot.parse_error : "",
    ].filter(Boolean).join("\n");
    this.publish(entry, { snapshot, state: unknown ? "unknown" : snapshot.status.state, busy, unknown, notice, logs,
      totalRows: snapshot.total_rows, locallyDropped: entry.record.locallyDropped + removed,
      droppedBeforeSequence: Math.max(entry.record.droppedBeforeSequence, input.dropped_before_sequence) });
    if (!busy) { this.clearTimer(entry); this.trimHistory(); }
  }
  private schedulePoll(entry: Entry): void {
    if (!entry.context || !entry.record.busy || entry.timer !== null) return;
    entry.timer = this.schedule(() => { entry.timer = null; void this.poll(entry); }, this.interval);
  }
  private async poll(entry: Entry): Promise<void> {
    if (!entry.context || !entry.record.busy) return;
    try { this.merge(entry, await this.source.batchExecutionStatus(entry.context, entry.cursor)); }
    catch { if (entry.record.busy) this.publish(entry, { notice: "暂时无法读取可信状态；保留同一任务并继续等待，不会重新运行。" }); }
    finally { this.schedulePoll(entry); }
  }
  async cancel(id: string): Promise<CancelAcknowledgement | null> {
    const entry = this.entries.get(id);
    if (!entry || !entry.record.busy) return null;
    if (entry.cancelling) return entry.cancelling;
    this.publish(entry, { cancelRequested: true, notice: "已请求停止；仍需等待后台停止、事务回执和资源排空。" });
    if (!entry.context) return null;
    const cancellation = Promise.resolve().then(() => this.source.cancelExecution(entry.context!)).then(ack => {
      if (ack.execution_id !== entry.context?.execution_id) throw new Error("停止回执身份不匹配");
      // ack.state 即使是终态也不能替代 snapshot/outcome；继续原任务的增量轮询。
      this.publish(entry, { cancelRequested: ack.accepted,
        notice: ack.accepted ? "停止请求已接收，继续等待真实停止/事务确认。" : "后台尚未接受停止请求；任务状态以执行回执为准。" });
      return ack;
    }).catch(error => {
      this.publish(entry, { cancelRequested: false, notice: `停止请求未确认${errorCode(error)}；继续回看同一任务，可再次请求停止。` });
      return null;
    }).finally(() => { entry.cancelling = null; this.schedulePoll(entry); });
    entry.cancelling = cancellation;
    return cancellation;
  }
  private clearTimer(entry: Entry): void { if (entry.timer !== null) this.clearSchedule(entry.timer); entry.timer = null; }
  private trimHistory(): void {
    const settled = [...this.entries.values()].filter(entry => !entry.record.busy && !entry.record.unknown);
    for (const entry of settled.slice(0, Math.max(0, settled.length - this.historyLimit))) this.entries.delete(entry.record.id);
  }
}

export const batchExecutions = new BatchExecutionService(api);
const injectedServices = new WeakMap<BatchExecutionTransport, BatchExecutionService>();
/** 测试可注入完整 batch source，但没有旧 insertGeneratedRows 或 runSqlFile 的伪后台回落。 */
export function batchExecutionServiceFor(source: BatchExecutionTransport = api): BatchExecutionService {
  if (source === api) return batchExecutions;
  let service = injectedServices.get(source);
  if (!service) { service = new BatchExecutionService(source); injectedServices.set(source, service); }
  return service;
}