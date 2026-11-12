/** 调度 v2 DTO 与编辑器纯规则。浏览器只验证/展示；时间计算、领取与执行始终在后端。 */
export type ScheduleSpec =
  | { kind: "manual" }
  | { kind: "interval"; seconds: number; anchor_utc: string }
  | { kind: "calendar"; cron: string; timezone: string; dst_gap: "skip"; dst_fold: "first" };
export type SchedulerActionKind = "sql" | "backup" | "export" | "import" | "data_sync" | "structure_sync";
export interface ScheduledAction { id: string; kind: SchedulerActionKind; depends_on: string[]; template_ref: string; approved_scope_revision: string }
export interface ScheduledJobV2 {
  version: 2; id: string; revision: string; name: string; enabled: boolean;
  execution_mode: "app_online" | "offline_opt_in"; schedule: ScheduleSpec;
  misfire: "skip" | "latest_once"; overlap: "skip" | "queue_one"; timeout_ms: number; actions: ScheduledAction[];
}
export interface SchedulerTemplate {
  id: string; revision: string; name: string; kind: SchedulerActionKind; connection_id: string; namespace: string; read_only: boolean;
  configuration: { kind: "sql"; sql: string } | { kind: "tool"; options_json: string };
}
export interface SchedulerTemplateSummary { id: string; revision: string; name: string; kind: SchedulerActionKind; connection_id: string; namespace: string; read_only: boolean }
export interface ScheduledJobRecord {
  job: ScheduledJobV2; schedule_revision: string; next_run_utc: string | null;
  queued: { schedule_revision: string; scheduled_at_utc: string; request_id: string | null } | null;
  blocked_run_id: string | null; paused_reason: string | null;
}
export interface SchedulerNextRun { utc: string; local: string; timezone: string }
export type SchedulerRunStatus = "claimed" | "running" | "succeeded" | "failed" | "cancelled" | "timed_out" | "partial" | "unknown" | "awaiting_confirmation";
export interface SchedulerRunSummary {
  id: string; job_id: string; job_revision: string; schedule_revision: string; scheduled_at_utc: string;
  claimed_at_utc: string; started_at_utc: string | null; completed_at_utc: string | null; status: SchedulerRunStatus;
  resources_pending: boolean; cancel_requested: boolean;
  actions: { action_id: string; status: string; started_at_utc: string | null; completed_at_utc: string | null; receipt: { error_code: string | null; execution_id: string | null; affected_rows: string | null; verified: boolean; resources_pending: boolean } | null }[];
  journal: { at_utc: string; kind: string; action_id: string | null; code: string | null }[];
}
export interface SchedulerApprovalPreview {
  plan_id: string; review_digest: string; job_id: string; job_revision: string; expires_at_utc: string;
  actions: { action_id: string; template_name: string; connection_label: string; namespace: string; read_only: boolean; details: string[]; destructive: string[] }[];
}
export interface SchedulerLegacyItem {
  legacy_id: string; name: string; status: "ready" | "blocked" | "already_imported"; code: string | null;
  was_enabled: boolean | null; warnings: string[]; job: ScheduledJobV2 | null; template: SchedulerTemplate | null;
}
export interface SchedulerLegacyPreview {
  plan_id: string; review_digest: string; source_digest: string; target_revision: string; timezone: string;
  prepared_at_utc: string; expires_at_utc: string; items: SchedulerLegacyItem[];
}
export interface SchedulerLegacyReceipt {
  plan_id: string; review_digest: string; source_digest: string; jobs: ScheduledJobRecord[]; blocked_count: number;
}
export interface SchedulerOfflinePlan {
  id: string; job_id: string; job_revision: string; schedule_revision: string; executable: string; executable_digest: string;
  argv: string[]; task_name: string; account_label: string; wake_device: boolean; run_when_logged_out: boolean;
  prepared_at_utc: string; expires_at_utc: string; consent_digest: string; confirmed: boolean;
  installed_receipt: { task_name: string; installed_at_utc: string; executable_digest: string; configuration_digest: string } | null;
}
/** 宿主的 approve 只接收服务器一次性预览 ID/digest；不得由客户端传入 ActionScope。 */
export interface ScheduleV2Bindings {
  listJobs(): Promise<ScheduledJobRecord[]>;
  saveJob(job: ScheduledJobV2, expectedRevision: string | null): Promise<ScheduledJobRecord>;
  deleteJob(jobId: string, expectedRevision: string): Promise<void>;
  listTemplates(): Promise<SchedulerTemplateSummary[]>;
  loadTemplate(templateId: string): Promise<SchedulerTemplate>;
  saveTemplate(template: SchedulerTemplate, expectedRevision: string | null): Promise<SchedulerTemplate>;
  previewTimes(schedule: ScheduleSpec): Promise<SchedulerNextRun[]>;
  prepareApproval(jobId: string, revision: string): Promise<SchedulerApprovalPreview>;
  approveJob(planId: string, reviewDigest: string): Promise<ScheduledJobRecord>;
  runNow(jobId: string, revision: string, requestId: string): Promise<SchedulerRunSummary | null>;
  history(jobId: string): Promise<SchedulerRunSummary[]>;
  cancelRun(runId: string): Promise<SchedulerRunSummary>;
  editToolTemplate?(template: SchedulerTemplate): Promise<SchedulerTemplateSummary | null>;
  createToolTemplate?(kind: Exclude<SchedulerActionKind, "sql">): Promise<SchedulerTemplateSummary | null>;
  reconcileRun?(run: SchedulerRunSummary, job: ScheduledJobRecord): Promise<ScheduledJobRecord | null>;
  prepareOffline?(jobId: string, revision: string, options: { account_label: string; wake_device: boolean; run_when_logged_out: boolean }): Promise<SchedulerOfflinePlan>;
  confirmOffline?(jobId: string, planId: string, digest: string): Promise<SchedulerOfflinePlan>;
  installOffline?(jobId: string, planId: string): Promise<SchedulerOfflinePlan>;
  removeOffline?(jobId: string, planId: string): Promise<void>;
  prepareLegacyMigration?(timezone: string): Promise<SchedulerLegacyPreview>;
  confirmLegacyMigration?(planId: string, reviewDigest: string): Promise<SchedulerLegacyReceipt>;
  onChanged?(): void;
}
export const SCHEDULER_KIND_LABELS: Record<SchedulerActionKind, string> = { sql: "SQL", backup: "备份", export: "导出", import: "导入", data_sync: "数据同步", structure_sync: "结构同步" };
export const SCHEDULER_STATUS_LABELS: Record<SchedulerRunStatus, string> = { claimed: "已领取", running: "执行中", succeeded: "已核验成功", failed: "失败", cancelled: "已停止", timed_out: "已超时", partial: "部分已应用，待核对", unknown: "结果未知，禁止重放", awaiting_confirmation: "等待重新核对" };
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const DIGEST = /^[0-9a-f]{64}$/;
function check(ok: unknown, message: string): asserts ok { if (!ok) throw new Error(message); }
function text(value: unknown, max: number, empty = false): value is string { return typeof value === "string" && value.length <= max && (empty || value.trim().length > 0) && !/[\u0000-\u001f\u007f]/.test(value); }
export function schedulerId(): string { return crypto.randomUUID(); }
export function newScheduledJob(nowUtc = new Date().toISOString()): ScheduledJobV2 {
  return { version: 2, id: schedulerId(), revision: "", name: "新任务", enabled: false, execution_mode: "app_online", schedule: { kind: "interval", seconds: 300, anchor_utc: nowUtc }, misfire: "skip", overlap: "skip", timeout_ms: 300_000, actions: [] };
}
export function validateCron5(expression: string): void {
  check(typeof expression === "string" && expression.length <= 1024, "Cron 表达式过长");
  const fields = expression.trim().split(/\s+/); check(fields.length === 5, "Cron 必须是五字段：分 时 日 月 星期");
  const ranges = [[0, 59], [0, 23], [1, 31], [1, 12], [0, 7]];
  fields.forEach((field, index) => {
    const [min, max] = ranges[index]; check(field.length <= 256, "Cron 字段过长");
    for (const item of field.split(",")) {
      const match = /^(\*|\d{1,3}(?:-\d{1,3})?)(?:\/(\d{1,3}))?$/.exec(item); check(match, "Cron 仅支持整数、*、列表、范围和步长");
      const step = match[2] === undefined ? 1 : Number(match[2]); check(step > 0 && step <= max - min + 1, "Cron 步长超出范围");
      if (match[1] !== "*") { const parts = match[1].split("-").map(Number); const end = parts[1] ?? parts[0]; check(parts[0] >= min && end <= max && parts[0] <= end, "Cron 值或范围超界"); }
    }
  });
}
export function validateScheduleSpec(schedule: ScheduleSpec): void {
  check(schedule && typeof schedule === "object", "缺少调度配置");
  if (schedule.kind === "manual") return;
  if (schedule.kind === "interval") {
    check(Number.isSafeInteger(schedule.seconds) && schedule.seconds >= 1 && schedule.seconds <= 31_536_000, "间隔必须是 1–31536000 秒");
    check(typeof schedule.anchor_utc === "string" && /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d{1,9})?Z$/.test(schedule.anchor_utc) && Number.isFinite(Date.parse(schedule.anchor_utc)), "锚点必须是明确的 UTC 时间，例如 2026-01-01T00:00:00Z");
    check(new Date(schedule.anchor_utc).toISOString().slice(0, 19) === schedule.anchor_utc.slice(0, 19), "锚点包含不存在的日期或时间");
    const year = Number(schedule.anchor_utc.slice(0, 4)); check(year >= 1970 && year <= 9999, "锚点年份必须在 1970–9999 之间"); return;
  }
  check(schedule.kind === "calendar", "未支持的调度类型"); validateCron5(schedule.cron);
  check(text(schedule.timezone, 128) && /^(?:UTC|GMT|[A-Za-z_]+(?:\/[A-Za-z0-9_+\-]+)+)$/.test(schedule.timezone), "请选择明确的 IANA 时区");
  check(schedule.dst_gap === "skip" && schedule.dst_fold === "first", "DST 策略必须是跳过不存在时刻、重叠时取第一次");
  // 浏览器可能不含后端全部 tzdb；只验证格式，最终 IANA/DST 与未来五次由后端确认。
}
export function schedulerActionOrder(actions: readonly ScheduledAction[]): string[] {
  check(Array.isArray(actions) && actions.length >= 1 && actions.length <= 64, "任务链需要 1–64 个动作");
  const ids = new Set<string>();
  for (const action of actions) { check(UUID.test(action.id) && !ids.has(action.id), "动作身份无效或重复"); ids.add(action.id); check(UUID.test(action.template_ref) && Object.prototype.hasOwnProperty.call(SCHEDULER_KIND_LABELS, action.kind), "请选择有效的动作模板"); check(action.approved_scope_revision === "" || DIGEST.test(action.approved_scope_revision), "动作批准版本无效"); }
  const pending = new Map<string, Set<string>>();
  for (const action of actions) { check(Array.isArray(action.depends_on) && action.depends_on.length <= 64, "动作依赖无效"); const deps: Set<string> = new Set<string>(action.depends_on); check(deps.size === action.depends_on.length && !deps.has(action.id) && [...deps].every((id: string) => ids.has(id)), "依赖不能重复、指向自己或缺失的动作"); pending.set(action.id, deps); }
  const order: string[] = [];
  while (pending.size) { const ready = actions.filter(action => pending.get(action.id)?.size === 0); check(ready.length, "任务链包含环，请调整依赖"); for (const action of ready) { pending.delete(action.id); for (const deps of pending.values()) deps.delete(action.id); order.push(action.id); } }
  return order;
}
export function validateScheduledJob(job: ScheduledJobV2): void {
  check(job.version === 2 && UUID.test(job.id) && (job.revision === "" || UUID.test(job.revision)), "任务身份或版本无效");
  check(text(job.name, 256), "任务名不能为空、过长或包含控制字符");
  check(typeof job.enabled === "boolean" && ["app_online", "offline_opt_in"].includes(job.execution_mode), "任务启用状态或执行模式无效");
  check(["skip", "latest_once"].includes(job.misfire) && ["skip", "queue_one"].includes(job.overlap), "错过或重叠策略无效");
  check(Number.isSafeInteger(job.timeout_ms) && job.timeout_ms >= 1 && job.timeout_ms <= 86_400_000, "总执行预算必须是 1–86400000 毫秒");
  validateScheduleSpec(job.schedule); schedulerActionOrder(job.actions);
}
export function removeSchedulerAction(job: ScheduledJobV2, actionId: string): ScheduledJobV2 {
  const out = structuredClone(job); out.actions = out.actions.filter(a => a.id !== actionId); for (const a of out.actions) { if (a.depends_on.includes(actionId)) { a.depends_on = a.depends_on.filter(id => id !== actionId); a.approved_scope_revision = ""; } } return out;
}
export function scheduleDescription(schedule: ScheduleSpec): string {
  if (schedule.kind === "manual") return "仅手动执行";
  if (schedule.kind === "interval") return `每 ${schedule.seconds} 秒，从 ${schedule.anchor_utc} 锚点计算`;
  return `${schedule.cron} · ${schedule.timezone}（DST 缺口跳过，重叠只执行第一次）`;
}
export function schedulerJobCanRun(record: ScheduledJobRecord): boolean { return !record.paused_reason && !record.blocked_run_id && record.job.actions.every(a => DIGEST.test(a.approved_scope_revision)); }
export function schedulerRunActive(run: SchedulerRunSummary): boolean { return run.status === "claimed" || run.status === "running" || run.resources_pending; }
export function schedulerCanonicalJson(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(schedulerCanonicalJson).join(",")}]`;
  if (value !== null && typeof value === "object") return `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${schedulerCanonicalJson((value as Record<string, unknown>)[key])}`).join(",")}}`;
  return JSON.stringify(value);
}
export function assertSchedulerLegacyPreview(preview: SchedulerLegacyPreview, timezone: string): void {
  check(preview && UUID.test(preview.plan_id) && DIGEST.test(preview.review_digest) && DIGEST.test(preview.source_digest) && DIGEST.test(preview.target_revision), "旧任务迁移预览身份无效");
  check(preview.timezone === timezone && Number.isFinite(Date.parse(preview.prepared_at_utc)) && Number.isFinite(Date.parse(preview.expires_at_utc)) && Date.parse(preview.expires_at_utc) > Date.parse(preview.prepared_at_utc), "迁移时区或时间基线不一致");
  check(Array.isArray(preview.items) && preview.items.length <= 512, "旧任务迁移预览容量无效");
  const ids = new Set<string>(), jobs = new Set<string>(), templates = new Set<string>();
  for (const item of preview.items) {
    check(typeof item.legacy_id === "string" && typeof item.name === "string" && Array.isArray(item.warnings) && item.warnings.every(w => typeof w === "string"), "旧任务迁移项目无效");
    check(["ready", "blocked", "already_imported"].includes(item.status), "旧任务迁移状态无效");
    if (item.status !== "ready") { check(item.job == null && item.template == null, "阻塞或已迁移项目不能携带待执行定义"); continue; }
    check(!ids.has(item.legacy_id) && item.job != null && item.template != null, "迁移项目身份重复或定义缺失"); ids.add(item.legacy_id);
    const job = item.job, template = item.template; validateScheduledJob(job);
    check(!jobs.has(job.id) && !templates.has(template.id) && UUID.test(job.revision) && UUID.test(template.id) && UUID.test(template.revision), "迁移任务或模板身份重复/无效"); jobs.add(job.id); templates.add(template.id);
    check(job.enabled === false && job.execution_mode === "app_online" && job.actions.length === 1 && job.actions[0].approved_scope_revision === "", "迁移任务必须暂停且未批准");
    check(job.actions[0].template_ref === template.id && job.actions[0].kind === template.kind && typeof template.connection_id === "string" && template.connection_id.length > 0 && typeof template.namespace === "string", "迁移模板与实际动作绑定不一致");
    check((template.kind === "sql" && template.configuration.kind === "sql" && typeof template.configuration.sql === "string") || (template.kind !== "sql" && template.configuration.kind === "tool" && typeof template.configuration.options_json === "string"), "迁移模板内容与类别不一致");
  }
}
export function assertSchedulerLegacyReceipt(preview: SchedulerLegacyPreview, receipt: SchedulerLegacyReceipt): void {
  check(receipt?.plan_id === preview.plan_id && receipt.review_digest === preview.review_digest && receipt.source_digest === preview.source_digest, "迁移回执来源或计划身份不一致；请刷新核对，不可重复提交");
  const ready = preview.items.filter(item => item.status === "ready");
  check(Array.isArray(receipt.jobs) && receipt.jobs.length === ready.length && receipt.blocked_count === preview.items.filter(item => item.status === "blocked").length, "迁移回执项目数量不一致");
  const expected = new Map(ready.map(item => [item.job!.id, item.job!]));
  for (const record of receipt.jobs) {
    const job = expected.get(record.job?.id); check(job && schedulerCanonicalJson(record.job) === schedulerCanonicalJson(job), "迁移回执内容与已核对定义不一致"); expected.delete(record.job.id);
    check(record.job.enabled === false && record.paused_reason != null && !record.blocked_run_id && !record.queued && !record.next_run_utc, "迁移结果未保持暂停和待批准状态");
    const approvals = (record as ScheduledJobRecord & { approvals?: Record<string, unknown> }).approvals;
    check(!approvals || Object.keys(approvals).length === 0, "迁移不应创建端点批准");
  }
  check(expected.size === 0, "迁移回执有重复或遗漏任务");
}
export function assertSchedulerSaveReceipt(submitted: ScheduledJobV2, receipt: ScheduledJobRecord): void {
  check(receipt?.job?.id === submitted.id && UUID.test(receipt.job.revision) && DIGEST.test(receipt.schedule_revision), "保存回执身份无效；请重新读取任务状态后核对");
  validateScheduledJob(receipt.job);
  const expected = structuredClone(submitted), actual = structuredClone(receipt.job); expected.revision = actual.revision = ""; for (const a of [...expected.actions, ...actual.actions]) a.approved_scope_revision = "";
  check(schedulerCanonicalJson(expected) === schedulerCanonicalJson(actual), "保存回执内容与提交不一致；请重新读取任务后核对");
}
