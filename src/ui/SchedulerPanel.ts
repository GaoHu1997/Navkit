/** 持久任务链管理。浏览器仅编辑/核对，不领取到期任务，也不直接执行 SQL/工具。 */
import { Dialog, checkbox, numberInput, select, textInput, type DialogOptions } from "./Dialog";
import type { ConnectionConfig } from "../core/types";
import {
  SCHEDULER_KIND_LABELS, SCHEDULER_STATUS_LABELS, assertSchedulerSaveReceipt, newScheduledJob,
  removeSchedulerAction, scheduleDescription, schedulerId, schedulerJobCanRun, schedulerRunActive,
  validateScheduledJob, schedulerCanonicalJson, type ScheduleV2Bindings, type ScheduledJobRecord, type ScheduledJobV2,
  type SchedulerApprovalPreview, type SchedulerRunSummary, type SchedulerTemplate,
  assertSchedulerLegacyPreview, assertSchedulerLegacyReceipt, type SchedulerLegacyPreview,
  type SchedulerTemplateSummary,
} from "../core/scheduler";

function paragraph(text: string): HTMLParagraphElement { const el = document.createElement("p"); el.textContent = text; return el; }
function labelled(text: string, control: HTMLElement): HTMLLabelElement {
  const label = document.createElement("label"); label.style.display = "grid"; label.style.gap = "4px";
  const title = document.createElement("span"); title.textContent = text; label.append(title, control); return label;
}
function button(text: string, action: () => void | Promise<void>): HTMLButtonElement {
  const el = document.createElement("button"); el.type = "button"; el.className = "btn"; el.textContent = text;
  el.addEventListener("click", () => { void action(); }); return el;
}
function checked(label: HTMLLabelElement): boolean { return label.querySelector<HTMLInputElement>("input")?.checked === true; }
function errorText(error: unknown): string { return error instanceof Error ? error.message : String(error); }
function pre(text: string): HTMLPreElement { const el = document.createElement("pre"); el.textContent = text; el.style.whiteSpace = "pre-wrap"; el.style.overflowWrap = "anywhere"; return el; }
function accessibleModal(dlg: Dialog, busy: () => boolean, before: HTMLElement | null): void {
  dlg.el.setAttribute("role", "dialog"); dlg.el.setAttribute("aria-modal", "true");
  const title = dlg.el.querySelector<HTMLElement>(".dialog-title-text");
  if (title) { title.id = `scheduler-title-${schedulerId()}`; dlg.el.setAttribute("aria-labelledby", title.id); }
  dlg.el.tabIndex = -1;
  dlg.el.addEventListener("keydown", event => {
    if (event.key === "Escape") { event.preventDefault(); if (!busy()) dlg.close(); return; }
    if (event.key !== "Tab") return;
    const controls = [...dlg.el.querySelectorAll<HTMLElement>("button,input,select,textarea,[tabindex='0']")]
      .filter(el => !el.hasAttribute("disabled") && !el.hidden && !el.closest("[hidden]") && el.getClientRects().length > 0);
    if (!controls.length) { event.preventDefault(); dlg.el.focus(); return; }
    const current = document.activeElement; const first = controls[0], last = controls[controls.length - 1];
    if (event.shiftKey && (current === first || !dlg.el.contains(current))) { event.preventDefault(); last.focus(); }
    else if (!event.shiftKey && (current === last || !dlg.el.contains(current))) { event.preventDefault(); first.focus(); }
  });
  dlg.onClose(() => { if (before?.isConnected) before.focus(); });
  queueMicrotask(() => dlg.el.querySelector<HTMLElement>("input,select,textarea,button")?.focus());
}
export function schedulerModal(options: DialogOptions, busy: () => boolean): Dialog {
  // The base Dialog focuses while constructing; capture the trigger first.
  const before = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const dlg = new Dialog(options); accessibleModal(dlg, busy, before); return dlg;
}
export function showModalError(dlg: Dialog, message: string): void {
  const id = dlg.body.querySelector<HTMLElement>(".dialog-error")?.id || `scheduler-form-error-${schedulerId()}`;
  Dialog.prototype.showError.call(dlg, message);
  const error = dlg.body.querySelector<HTMLElement>(".dialog-error");
  if (!error) return; error.id = id; error.setAttribute("role", "alert"); error.setAttribute("aria-live", "assertive");
  for (const field of dlg.body.querySelectorAll<HTMLElement>("input,select,textarea")) {
    const descriptions = new Set((field.getAttribute("aria-describedby") ?? "").split(/\s+/).filter(Boolean));
    descriptions.add(id); field.setAttribute("aria-describedby", [...descriptions].join(" "));
  }
}
function setBusy(dlg: Dialog, busy: boolean): void {
  dlg.el.setAttribute("aria-busy", String(busy));
  for (const control of dlg.el.querySelectorAll<HTMLInputElement | HTMLButtonElement | HTMLSelectElement | HTMLTextAreaElement>("input,button,select,textarea")) control.disabled = busy;
}

export class SchedulerPanel {
  readonly dialog: Dialog;
  private jobs: ScheduledJobRecord[] = [];
  private templates: SchedulerTemplateSummary[] = [];
  private selectedId = "";
  private disposed = false;
  private busy = false;
  private stale = true;
  private request = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private readonly jobSelect = select([]);
  private readonly actionSelect = select([]);
  private readonly toolKind = select(["backup", "export", "import", "data_sync", "structure_sync"].map(value => ({ value, label: SCHEDULER_KIND_LABELS[value as keyof typeof SCHEDULER_KIND_LABELS] })));
  private readonly summary = document.createElement("section");
  private readonly historyBody = document.createElement("section");
  private readonly status = paragraph("正在读取本机任务…");
  private readonly alert = paragraph("");
  private readonly controls: HTMLButtonElement[] = [];
  private readonly children = new Set<Dialog>();
  private historyRequest = 0;

  constructor(private readonly bindings: ScheduleV2Bindings, private readonly connections: readonly ConnectionConfig[]) {
    this.status.setAttribute("role", "status"); this.status.setAttribute("aria-live", "polite");
    this.alert.setAttribute("role", "alert"); this.alert.id = `scheduler-error-${schedulerId()}`;
    this.jobSelect.setAttribute("aria-describedby", this.alert.id);
    this.jobSelect.addEventListener("change", () => { this.selectedId = this.jobSelect.value; this.renderSelection(); void this.loadHistory(); });
    const toolbar = document.createElement("div"); toolbar.style.display = "flex"; toolbar.style.flexWrap = "wrap"; toolbar.style.gap = "6px";
    this.controls.push(button("新建任务链", () => this.editJob()), button("编辑", () => this.editJob(this.selected()?.job)),
      button("核对并批准", () => this.review()), button("立即执行", () => this.runNow()),
      button("暂停", () => this.pause()), button("删除", () => this.deleteSelected()),
      button("新建 SQL 模板", () => this.editSqlTemplate()), button("编辑动作模板", () => this.editSelectedTemplate()),
      button("新建工具模板", () => this.createTool()));
    toolbar.append(...this.controls, button("刷新", () => this.refresh()), button("迁移旧任务", () => this.openLegacyMigration()));
    this.dialog = schedulerModal({ title: "持久任务链与调度", width: 900, height: 680, closable: false,
      render: body => { body.style.display = "flex"; body.style.flexDirection = "column"; body.style.gap = "8px"; body.style.overflow = "auto";
        body.append(paragraph("执行由唯一后端调度服务负责。未批准的动作不会自动运行；失联/结果未知的运行不会自动重放。"),
          labelled("任务", this.jobSelect), labelled("选择编辑的动作", this.actionSelect), labelled("新工具模板类型", this.toolKind), toolbar, this.alert, this.status, this.summary, this.historyBody); },
      buttons: [{ label: "关闭", onClick: () => !this.busy && !this.children.size }] }, () => this.busy || this.children.size > 0);
    this.dialog.onClose(() => this.dispose()); void this.refresh();
  }
  private modal(options: DialogOptions, busy: () => boolean): Dialog {
    const dlg = schedulerModal(options, busy); this.children.add(dlg);
    dlg.onClose(() => this.children.delete(dlg)); return dlg;
  }
  private selected(): ScheduledJobRecord | undefined { return this.jobs.find(record => record.job.id === this.selectedId); }
  private guard(): ScheduledJobRecord {
    const record = this.selected(); if (this.busy || this.stale || !record) throw new Error("请先刷新并选择当前任务；未确认的旧状态不能执行或修改"); return structuredClone(record);
  }
  private async operate(action: () => Promise<void>): Promise<void> {
    if (this.busy || this.disposed) return; this.busy = true; this.alert.textContent = ""; this.renderSelection();
    try { await action(); this.bindings.onChanged?.(); }
    catch (error) { if (!this.disposed) this.alert.textContent = errorText(error); }
    finally { this.busy = false; if (!this.disposed) this.renderSelection(); }
  }
  async refresh(): Promise<void> {
    if (this.disposed) return; const request = ++this.request; this.status.textContent = "正在读取本机任务…";
    try {
      const [jobs, templates] = await Promise.all([this.bindings.listJobs(), this.bindings.listTemplates()]);
      for (const record of jobs) validateScheduledJob(record.job);
      if (this.disposed || request !== this.request) return;
      this.jobs = structuredClone(jobs); this.templates = structuredClone(templates); this.stale = false;
      if (!jobs.some(record => record.job.id === this.selectedId)) this.selectedId = jobs[0]?.job.id ?? "";
      this.jobSelect.replaceChildren(...jobs.map(record => { const opt = document.createElement("option"); opt.value = record.job.id; opt.textContent = record.job.name; return opt; }));
      this.jobSelect.value = this.selectedId; this.alert.textContent = ""; this.renderSelection(); await this.loadHistory();
    } catch (error) { if (this.disposed || request !== this.request) return; this.stale = true; this.alert.textContent = `读取失败，旧列表仅供核对：${errorText(error)}`; this.renderSelection(); }
  }
  private renderSelection(): void {
    const record = this.selected(); this.summary.replaceChildren();
    this.dialog?.el.setAttribute("aria-busy", String(this.busy)); this.jobSelect.disabled = this.actionSelect.disabled = this.toolKind.disabled = this.busy;
    const actionId = this.actionSelect.value;
    this.actionSelect.replaceChildren(...(record?.job.actions ?? []).map(action => { const option = document.createElement("option"); option.value = action.id; option.textContent = `${SCHEDULER_KIND_LABELS[action.kind]} · ${this.templates.find(t => t.id === action.template_ref)?.name ?? "模板不可用"}`; return option; }));
    this.actionSelect.value = record?.job.actions.some(a => a.id === actionId) ? actionId : record?.job.actions[0]?.id ?? "";
    for (const control of this.controls) control.disabled = this.busy || this.stale;
    for (const index of [1, 2, 3, 4, 5, 7]) this.controls[index].disabled ||= !record;
    this.controls[3].disabled ||= !record || !schedulerJobCanRun(record);
    this.controls[8].disabled ||= !this.bindings.createToolTemplate;
    if (!record) { this.status.textContent = this.stale ? "列表尚未确认" : "没有持久任务；新建任务需先保存模板及明确核对范围"; return; }
    this.status.textContent = `${record.job.enabled ? "已启用" : "已暂停"} · ${record.paused_reason || (record.blocked_run_id ? "有待核对运行" : "当前批准有效")} · 下次 UTC：${record.next_run_utc ?? "无"}`;
    this.summary.append(paragraph(scheduleDescription(record.job.schedule)), paragraph(`错过：${record.job.misfire === "skip" ? "跳过" : "仅最近一次"}；重叠：${record.job.overlap === "skip" ? "跳过" : "合并排队一次"}；总预算：${record.job.timeout_ms} ms`));
    const list = document.createElement("ol");
    for (const action of record.job.actions) { const item = document.createElement("li"); const template = this.templates.find(t => t.id === action.template_ref);
      item.textContent = `${SCHEDULER_KIND_LABELS[action.kind]} · ${template?.name ?? "模板不可用"} · ${action.approved_scope_revision ? "已核对范围" : "待批准"} · 依赖 ${action.depends_on.length} 个动作`; list.append(item); }
    this.summary.append(list);
  }
  private async loadHistory(): Promise<void> {
    if (this.timer) clearTimeout(this.timer); this.timer = null;
    const request = ++this.historyRequest;
    const id = this.selectedId; if (!id || this.disposed) { this.historyBody.replaceChildren(); return; }
    try {
      const runs = await this.bindings.history(id); if (this.disposed || this.selectedId !== id || request !== this.historyRequest) return;
      this.renderHistory(runs);
      if (runs.some(schedulerRunActive)) this.timer = setTimeout(() => { void this.loadHistory(); }, 2000);
    } catch (error) { if (!this.disposed && this.selectedId === id && request === this.historyRequest) this.alert.textContent = `运行记录未确认：${errorText(error)}`; }
  }
  private renderHistory(runs: SchedulerRunSummary[]): void {
    this.historyBody.replaceChildren(); const heading = document.createElement("h3"); heading.textContent = "运行与逐步回执"; this.historyBody.append(heading);
    if (!runs.length) { this.historyBody.append(paragraph("没有运行记录")); return; }
    for (const run of runs) {
      const box = document.createElement("details"), title = document.createElement("summary");
      title.textContent = `${run.scheduled_at_utc} · ${SCHEDULER_STATUS_LABELS[run.status] ?? run.status}${run.resources_pending ? " · 资源尚未排空" : ""}`; box.append(title);
      for (const action of run.actions) box.append(paragraph(`${action.action_id} · ${action.status} · ${action.receipt?.error_code ?? ""} · 行数 ${action.receipt?.affected_rows ?? "未知/不适用"} · ${action.receipt?.verified ? "已核验" : "未核验"}`));
      box.append(pre(run.journal.map(event => `${event.at_utc} ${event.kind} ${event.action_id ?? ""} ${event.code ?? ""}`).join("\n")));
      if (schedulerRunActive(run)) box.append(button(run.cancel_requested ? "停止已请求，等待真实回执" : "请求停止", () => this.operate(async () => { await this.bindings.cancelRun(run.id); await this.loadHistory(); })));
      if (!run.resources_pending && this.selected()?.blocked_run_id === run.id && this.bindings.reconcileRun) box.append(button("核对此次未知/部分结果", () => this.operate(async () => { const record = this.selected(); if (!record) return; await this.bindings.reconcileRun?.(run, record); await this.refresh(); })));
      this.historyBody.append(box);
    }
  }
  private async pause(): Promise<void> { let record: ScheduledJobRecord; try { record = this.guard(); } catch (e) { this.alert.textContent = errorText(e); return; }
    await this.operate(async () => { const submitted = { ...record.job, enabled: false }; const saved = await this.bindings.saveJob(submitted, record.job.revision); assertSchedulerSaveReceipt(submitted, saved); await this.refresh(); }); }
  private async deleteSelected(): Promise<void> { let record: ScheduledJobRecord; try { record = this.guard(); } catch (e) { this.alert.textContent = errorText(e); return; }
    let saving = false; const dlg = this.modal({ title: "删除本机任务", width: 420, closable: false, render: body => body.append(paragraph(`删除「${record.job.name}」的调度定义？不会删除数据库对象或清除既有运行日志。活动/未排空任务不能删除。`)),
      buttons: [{ label: "删除", onClick: async () => { if (saving) return false; saving = true; setBusy(dlg, true); try { await this.bindings.deleteJob(record.job.id, record.job.revision); await this.refresh(); return true; } catch (e) { showModalError(dlg, errorText(e)); return false; } finally { saving = false; setBusy(dlg, false); } } }, { label: "取消", onClick: () => !saving }] }, () => saving); }
  private async runNow(): Promise<void> { let record: ScheduledJobRecord; try { record = this.guard(); } catch (e) { this.alert.textContent = errorText(e); return; }
    if (!schedulerJobCanRun(record)) { this.alert.textContent = "此任务尚未批准或有待核对运行，未发起执行"; return; }
    // Capture one request identity before the IPC. A lost claim acknowledgement must not cause an automatic retry.
    const requestId = schedulerId(); await this.operate(async () => {
      try { const run = await this.bindings.runNow(record.job.id, record.job.revision, requestId);
        if (run && (run.job_id !== record.job.id || !run.id)) throw new Error("运行领取回执身份不一致");
        this.status.textContent = run ? "后端已领取；只有逐步核验完成才算成功" : "没有新领取；可能已合并排队或请求已消费，请核对运行记录";
      } catch (e) { this.stale = true; throw new Error(`领取结果未确认，不会自动重试。请求 ${requestId}：${errorText(e)}`); }
      await this.loadHistory(); }); }
  private async review(): Promise<void> { let record: ScheduledJobRecord; try { record = this.guard(); } catch (e) { this.alert.textContent = errorText(e); return; }
    await this.operate(async () => { const preview = await this.bindings.prepareApproval(record.job.id, record.job.revision); this.openApproval(record, preview); }); }
  private openApproval(record: ScheduledJobRecord, preview: SchedulerApprovalPreview): void {
    if (this.disposed || preview.job_id !== record.job.id || preview.job_revision !== record.job.revision || !preview.plan_id || !/^[0-9a-f]{64}$/.test(preview.review_digest)
      || preview.actions.length !== record.job.actions.length || new Set(preview.actions.map(action => action.action_id)).size !== record.job.actions.length
      || preview.actions.some(action => !record.job.actions.some(a => a.id === action.action_id))) throw new Error("审批预览身份不一致，没有批准任何执行");
    const reviewedJob = schedulerCanonicalJson(record.job);
    const accept = checkbox("我已逐项核对精确模板、实际端点和目标/文件范围", false);
    const risk = checkbox("我明确接受上述写入/破坏性风险及没有可证明行数上限的 SQL 副作用", false);
    const hasRisk = preview.actions.some(action => action.destructive.length > 0); let saving = false; let consumed = false;
    const dlg = this.modal({ title: "核对真实范围并批准此任务版本", width: 820, height: 630, closable: false,
      render: body => { body.style.overflow = "auto"; body.append(paragraph(`预览过期 UTC：${preview.expires_at_utc}。批准后由后端领取；模板、文件、权限或端点变化会停止后续动作并要求重新核对。`));
        for (const action of preview.actions) { const section = document.createElement("section"); const h = document.createElement("h3"); h.textContent = `${action.template_name} · ${action.connection_label} / ${action.namespace} · ${action.read_only ? "只读" : "可能写入"}`;
          section.append(h, pre(action.details.join("\n"))); if (action.destructive.length) { const warnings = document.createElement("ul"); for (const warning of action.destructive) { const li = document.createElement("li"); li.textContent = warning; warnings.append(li); } section.append(warnings); } body.append(section); }
        body.append(accept); if (hasRisk) body.append(risk); },
      buttons: [{ label: "批准此范围", primary: true, onClick: async () => { if (this.disposed || saving || consumed) return false;
        const current = this.selected();
        if (!current || current.job.id !== record.job.id || schedulerCanonicalJson(current.job) !== reviewedJob) {
          consumed = true; accept.querySelector<HTMLInputElement>("input")!.checked = false; risk.querySelector<HTMLInputElement>("input")!.checked = false;
          const submit = dlg.footer.querySelector<HTMLButtonElement>("button.primary"); if (submit) { submit.disabled = true; submit.title = "任务版本已变更；请关闭后重新核对"; }
          showModalError(dlg, "任务已切换或修改，请关闭后重新核对当前版本；没有批准任何执行"); return false;
        }
        if (!checked(accept) || (hasRisk && !checked(risk))) { showModalError(dlg, "尚未明确确认范围和风险，没有批准任何执行"); return false; }
        if (!Number.isFinite(Date.parse(preview.expires_at_utc)) || Date.now() >= Date.parse(preview.expires_at_utc)) { showModalError(dlg, "预览已经过期，请关闭后重新核对"); return false; }
        consumed = true; saving = true; setBusy(dlg, true);
        try { const result = await this.bindings.approveJob(preview.plan_id, preview.review_digest);
          if (result.job.id !== record.job.id || result.paused_reason || result.blocked_run_id || !schedulerJobCanRun(result)) throw new Error("批准回执尚未确认，请刷新任务核对");
          await this.refresh(); return true;
        } catch (e) { showModalError(dlg, `批准未确认，不能重复消费此预览；请关闭并重新读取：${errorText(e)}`); return false; }
        finally { saving = false; setBusy(dlg, false); const submit = dlg.footer.querySelector<HTMLButtonElement>("button.primary");
          if (submit) { submit.disabled = consumed; submit.title = "此预览已经消费；请关闭后重新核对，不可重复批准"; } }
      } }, { label: "取消", onClick: () => !saving }] }, () => saving);
  }
  private openLegacyMigration(): void {
    if (this.disposed || this.busy || !this.bindings.prepareLegacyMigration || !this.bindings.confirmLegacyMigration) return;
    const timezone = textInput(Intl.DateTimeFormat().resolvedOptions().timeZone || "Asia/Shanghai");
    const output = pre("请选择旧时刻使用的 IANA 时区，然后预览原定义。预览只读取本机文件，不执行数据库工作。");
    const accept = checkbox("已核对所有可迁移定义；仅导入暂停任务，阻塞项保留在旧文件中", false);
    let preview: SchedulerLegacyPreview | null = null, consumed = false, saving = false, loading = false, closed = false, request = 0;
    const sync = () => {
      const submit = dlg.footer.querySelector<HTMLButtonElement>("button.primary");
      if (submit) { submit.disabled = saving || loading || consumed || !preview?.items.some(item => item.status === "ready") || !checked(accept);
        submit.title = consumed ? "此计划已消费；请关闭并刷新核对，不可重复提交" : "预览后明确核对；新任务保持暂停且没有端点批准"; }
    };
    const previewButton = button("预览旧任务", async () => {
      if (closed || this.disposed || saving || loading) return;
      const zone = timezone.value.trim(), token = ++request; preview = null; consumed = false; accept.querySelector<HTMLInputElement>("input")!.checked = false;
      loading = true; setBusy(dlg, true); output.textContent = "正在读取原文件并生成一次性迁移计划…";
      try {
        const result = await this.bindings.prepareLegacyMigration!(zone);
        if (closed || this.disposed || request !== token || timezone.value.trim() !== zone) return;
        assertSchedulerLegacyPreview(result, zone); preview = structuredClone(result);
        const lines = [`时区：${result.timezone}`, `源文件摘要：${result.source_digest}`, `计划有效至：${result.expires_at_utc}`, ""];
        for (const item of result.items) {
          lines.push(`${item.name}：${item.status === "ready" ? "可导入为暂停任务" : item.status === "already_imported" ? "已迁移，跳过" : `阻塞（${item.code ?? "无法安全转换"}）`}`);
          if (item.job && item.template) lines.push(scheduleDescription(item.job.schedule), JSON.stringify(item.template, null, 2));
          lines.push(...item.warnings, "");
        }
        if (!result.items.length) lines.push("旧任务文件为空或尚不存在，无需迁移。"); output.textContent = lines.join("\n");
      } catch (error) { if (!closed && !this.disposed && request === token) { preview = null; showModalError(dlg, `未生成可确认的迁移计划：${errorText(error)}`); } }
      finally { loading = false; if (!closed && !this.disposed) { setBusy(dlg, false); sync(); } }
    });
    const dlg = this.modal({ title: "核对并迁移旧任务", width: 790, height: 600, closable: false,
      render: body => body.append(paragraph("原任务定义和旧历史保持原样。新任务默认暂停；数据库端点、工具文件和写入风险需要在新任务中另行核对批准。"), labelled("旧时刻使用的 IANA 时区", timezone), previewButton, output, accept),
      buttons: [{ label: "导入为暂停任务", primary: true, onClick: async () => {
        if (closed || this.disposed || saving || loading || consumed || !preview || !preview.items.some(item => item.status === "ready") || !checked(accept)) return false;
        const reviewed = structuredClone(preview);
        if (timezone.value.trim() !== reviewed.timezone || Date.now() >= Date.parse(reviewed.expires_at_utc)) { preview = null; showModalError(dlg, "时区已修改或计划已过期，请重新预览；没有导入任务"); sync(); return false; }
        consumed = true; saving = true; setBusy(dlg, true);
        try {
          const result = await this.bindings.confirmLegacyMigration!(reviewed.plan_id, reviewed.review_digest);
          assertSchedulerLegacyReceipt(reviewed, result);
          if (!closed && !this.disposed) { await this.refresh(); this.status.textContent = `已导入 ${result.jobs.length} 个暂停任务；${result.blocked_count} 个阻塞项保留在旧文件，未执行任何数据库工作。`; }
          return true;
        } catch (error) {
          this.stale = true;
          if (!closed && !this.disposed) showModalError(dlg, `导入结果未确认，此计划不能重复提交；请关闭后刷新核对本机任务：${errorText(error)}`);
          return false;
        } finally { saving = false; if (!closed && !this.disposed) { setBusy(dlg, false); sync(); } }
      } }, { label: "关闭", onClick: () => !saving && !loading }] }, () => saving || loading);
    timezone.addEventListener("input", () => { ++request; preview = null; consumed = false; accept.querySelector<HTMLInputElement>("input")!.checked = false; output.textContent = "时区变更后请重新预览原定义。"; sync(); });
    accept.querySelector<HTMLInputElement>("input")!.addEventListener("change", sync);
    dlg.onClose(() => { closed = true; ++request; }); sync();
  }
  private async createTool(): Promise<void> {
    if (this.busy || this.stale || !this.bindings.createToolTemplate) return;
    const kind = this.toolKind.value as Exclude<SchedulerTemplate["kind"], "sql">;
    await this.operate(async () => { const template = await this.bindings.createToolTemplate?.(kind); if (template) await this.refresh(); });
  }
  private async editSelectedTemplate(): Promise<void> {
    const record = this.selected(); const id = record?.job.actions.find(a => a.id === this.actionSelect.value)?.template_ref; if (!id) return;
    await this.operate(async () => { const template = await this.bindings.loadTemplate(id);
      if (template.configuration.kind === "sql") this.editSqlTemplate(template);
      else if (this.bindings.editToolTemplate) { await this.bindings.editToolTemplate(template); await this.refresh(); }
      else this.showTemplate(template); });
  }
  private showTemplate(template: SchedulerTemplate): void { this.modal({ title: `动作模板：${template.name}`, width: 730, height: 460, closable: false,
    render: body => body.append(paragraph("此模板由既有工具配置器建立。修改模板会撤销所有引用任务的批准；不能用客户端构造的范围替代后端核对。"), pre(JSON.stringify(template, null, 2))), buttons: [{ label: "关闭" }] }, () => false); }
  private editSqlTemplate(initial?: SchedulerTemplate): void {
    const template: SchedulerTemplate = initial ? structuredClone(initial) : { id: schedulerId(), revision: "", name: "SQL 模板", kind: "sql", connection_id: this.connections[0]?.id ?? "", namespace: "", read_only: true, configuration: { kind: "sql", sql: "SELECT 1;" } };
    if (template.configuration.kind !== "sql") return;
    const name = textInput(template.name), connection = select(this.connections.map(c => ({ value: c.id, label: c.name })), template.connection_id), namespace = textInput(template.namespace);
    const readonly = checkbox("只读通道（不能通过模板取消实际连接只读）", template.read_only), sql = document.createElement("textarea"); sql.value = template.configuration.sql; sql.style.height = "230px"; sql.spellcheck = false; let saving = false;
    const dlg = this.modal({ title: initial ? "编辑 SQL 动作模板" : "新建 SQL 动作模板", width: 720, closable: false,
      render: body => body.append(labelled("模板名", name), labelled("实际连接", connection), labelled("命名空间", namespace), readonly, labelled("精确 SQL", sql), paragraph("保存模板只保存本机密文，不执行 SQL。更改模板版本会使旧审批失效。")),
      buttons: [{ label: "保存模板", primary: true, onClick: async () => { if (saving) return false; if (!name.value.trim() || !connection.value || !sql.value.trim()) { showModalError(dlg, "模板名、连接和 SQL 不能为空"); return false; }
        const submitted: SchedulerTemplate = { ...template, name: name.value.trim(), connection_id: connection.value, namespace: namespace.value, read_only: checked(readonly), configuration: { kind: "sql", sql: sql.value } };
        saving = true; setBusy(dlg, true); try { const saved = await this.bindings.saveTemplate(submitted, template.revision || null);
          if (saved.id !== template.id || !saved.revision || schedulerCanonicalJson({ ...saved, revision: submitted.revision }) !== schedulerCanonicalJson(submitted)) throw new Error("模板保存回执未确认；模板内容、只读状态或目标与提交不一致"); await this.refresh(); return true;
        } catch (e) { showModalError(dlg, errorText(e)); return false; } finally { saving = false; setBusy(dlg, false); }
      } }, { label: "取消", onClick: () => !saving }] }, () => saving);
  }
  private editJob(initial?: ScheduledJobV2): void {
    let draft = initial ? structuredClone(initial) : newScheduledJob(); let saving = false;
    const name = textInput(draft.name), enabled = checkbox("启用（仍须明确核对真实范围）", draft.enabled);
    const mode = select([{ value: "manual", label: "仅手动" }, { value: "interval", label: "固定间隔（单调时钟）" }, { value: "calendar", label: "Cron / IANA 时区" }], draft.schedule.kind);
    const seconds = numberInput(draft.schedule.kind === "interval" ? draft.schedule.seconds : 300); seconds.min = "1"; seconds.max = "31536000";
    const anchor = textInput(draft.schedule.kind === "interval" ? draft.schedule.anchor_utc : new Date().toISOString());
    const cron = textInput(draft.schedule.kind === "calendar" ? draft.schedule.cron : "0 9 * * *"), timezone = textInput(draft.schedule.kind === "calendar" ? draft.schedule.timezone : "Asia/Shanghai");
    const misfire = select([{ value: "skip", label: "跳过错过时刻" }, { value: "latest_once", label: "只执行最近一次" }], draft.misfire), overlap = select([{ value: "skip", label: "重叠时跳过" }, { value: "queue_one", label: "最多合并排队一次" }], draft.overlap);
    const timeout = numberInput(draft.timeout_ms); timeout.min = "1"; timeout.max = "86400000";
    const nextTimes = pre("尚未计算未来时刻"), actions = document.createElement("div"); let timeRequest = 0; let closed = false;
    const intervalRow = labelled("间隔秒数", seconds), anchorRow = labelled("明确 UTC 锚点", anchor), cronRow = labelled("五字段 Cron", cron), zoneRow = labelled("IANA 时区（DST 缺口跳过、重叠取第一次）", timezone);
    const capture = (): ScheduledJobV2 => {
      draft.name = name.value.trim(); draft.enabled = checked(enabled); draft.timeout_ms = Number(timeout.value);
      draft.execution_mode = "app_online"; draft.misfire = misfire.value as ScheduledJobV2["misfire"]; draft.overlap = overlap.value as ScheduledJobV2["overlap"];
      draft.schedule = mode.value === "manual" ? { kind: "manual" } : mode.value === "interval" ? { kind: "interval", seconds: Number(seconds.value), anchor_utc: anchor.value } : { kind: "calendar", cron: cron.value, timezone: timezone.value, dst_gap: "skip", dst_fold: "first" };
      return structuredClone(draft);
    };
    const visibility = () => { ++timeRequest; intervalRow.hidden = anchorRow.hidden = mode.value !== "interval"; cronRow.hidden = zoneRow.hidden = mode.value !== "calendar"; nextTimes.textContent = "时间配置变更后请重新计算未来五次（后端 tzdb）"; };
    mode.addEventListener("change", visibility);
    for (const input of [seconds, anchor, cron, timezone]) input.addEventListener("input", visibility);
    const renderActions = () => {
      actions.replaceChildren();
      for (const [index, action] of draft.actions.entries()) {
        const row = document.createElement("fieldset"), title = document.createElement("legend"); title.textContent = `动作 ${index + 1} · ${SCHEDULER_KIND_LABELS[action.kind]}`; row.append(title);
        const templates = select(this.templates.filter(t => t.kind === action.kind).map(t => ({ value: t.id, label: `${t.name} · ${t.connection_id}/${t.namespace}` })), action.template_ref);
        templates.addEventListener("change", () => { action.template_ref = templates.value; action.approved_scope_revision = ""; }); row.append(labelled("动作模板", templates));
        const dependencies = select(draft.actions.filter(a => a.id !== action.id).map(a => ({ value: a.id, label: `动作 ${draft.actions.indexOf(a) + 1} · ${SCHEDULER_KIND_LABELS[a.kind]}` }))); dependencies.multiple = true;
        for (const option of dependencies.options) option.selected = action.depends_on.includes(option.value);
        dependencies.addEventListener("change", () => { action.depends_on = [...dependencies.selectedOptions].map(o => o.value); action.approved_scope_revision = ""; });
        row.append(labelled("成功依赖（可多选；未知/失败会阻断下游）", dependencies), button("上移", () => { if (index > 0) { [draft.actions[index - 1], draft.actions[index]] = [draft.actions[index], draft.actions[index - 1]]; renderActions(); } }),
          button("下移", () => { if (index + 1 < draft.actions.length) { [draft.actions[index + 1], draft.actions[index]] = [draft.actions[index], draft.actions[index + 1]]; renderActions(); } }),
          button("移除动作", () => { draft = removeSchedulerAction(draft, action.id); renderActions(); })); actions.append(row);
      }
    };
    const templateSelect = select(this.templates.map(t => ({ value: t.id, label: `${SCHEDULER_KIND_LABELS[t.kind]} · ${t.name}` })));
    const dlg = this.modal({ title: initial ? "编辑持久任务链" : "新建持久任务链", width: 770, height: 650, closable: false,
      render: body => { body.style.overflow = "auto"; body.style.display = "grid"; body.style.gap = "8px";
        body.append(labelled("任务名", name), enabled, labelled("执行计划", mode), intervalRow, anchorRow, cronRow, zoneRow,
          labelled("错过策略", misfire), labelled("重叠策略", overlap), labelled("总预算（毫秒）", timeout),
          button("计算后端未来五次", async () => { if (saving || closed || this.disposed) return; const schedule = capture().schedule, request = ++timeRequest; try { const times = await this.bindings.previewTimes(schedule); if (closed || this.disposed || request !== timeRequest || schedulerCanonicalJson(schedule) !== schedulerCanonicalJson(capture().schedule)) return; nextTimes.textContent = times.length ? times.map(t => `${t.utc} · ${t.local} [${t.timezone}]`).join("\n") : "仅手动，不自动触发"; } catch (e) { if (!closed && !this.disposed && request === timeRequest) showModalError(dlg, errorText(e)); } }), nextTimes,
          paragraph("离线 OS runner 尚未启用，本页不会注册系统计划任务。保存/修改计划不会替代真实范围核对。"),
          labelled("添加已保存动作模板", templateSelect), button("添加动作", () => { const template = this.templates.find(t => t.id === templateSelect.value); if (!template || draft.actions.length >= 64) { showModalError(dlg, "先保存动作模板；最多 64 个动作"); return; }
            draft.actions.push({ id: schedulerId(), kind: template.kind, template_ref: template.id, depends_on: draft.actions.length ? [draft.actions[draft.actions.length - 1].id] : [], approved_scope_revision: "" }); renderActions(); }), actions); renderActions(); visibility(); },
      buttons: [{ label: "保存任务", primary: true, onClick: async () => { if (saving) return false; const submitted = capture();
        try { validateScheduledJob(submitted); } catch (e) { showModalError(dlg, errorText(e)); return false; }
        saving = true; setBusy(dlg, true); try { const result = await this.bindings.saveJob(submitted, initial?.revision || null); assertSchedulerSaveReceipt(submitted, result);
          this.selectedId = result.job.id; await this.refresh(); return true;
        } catch (e) { showModalError(dlg, `保存未确认，草稿保留；不要重复创建新身份：${errorText(e)}`); return false; } finally { saving = false; setBusy(dlg, false); }
      } }, { label: "取消", onClick: () => !saving }] }, () => saving);
    dlg.onClose(() => { closed = true; ++timeRequest; });
  }
  dispose(): void { if (this.disposed) return; this.disposed = true; ++this.request; ++this.historyRequest; if (this.timer) clearTimeout(this.timer); this.timer = null; for (const child of this.children) child.close(); this.children.clear(); }
}
