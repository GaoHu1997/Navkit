/** PL-22参数对话框：沿用现有Dialog与原生表单，不在前端拼调用SQL。 */
import { Dialog } from "../ui/Dialog";
import { RoutineCallController, parameterArgument, type RoutineCallPort, type RoutineCallMetadata, type RoutineCallResponse, type ParameterInput } from "../core/routine-call";
import { objectLabel, type DatabaseObjectId } from "../core/object-catalog";

let nextDialogId = 0;
export class RoutineCallDialog {
  readonly dialog: Dialog;
  readonly controller: RoutineCallController;
  private readonly previousFocus: HTMLElement | null;
  private readonly prefix = `routine-call-${++nextDialogId}`;
  private status!: HTMLElement;
  private errors!: HTMLElement;
  private parameters!: HTMLElement;
  private plan!: HTMLTextAreaElement;
  private result!: HTMLElement;
  private budget!: HTMLInputElement;
  private prepareButton!: HTMLButtonElement;
  private executeButton!: HTMLButtonElement;
  private cancelButton!: HTMLButtonElement;
  private reloadButton!: HTMLButtonElement;
  private renderedMetadata: RoutineCallMetadata | null = null;
  private fields = new Map<number, { input: HTMLInputElement; mode: HTMLSelectElement; error: HTMLElement }>();
  private alive = true;
  private lastResponse: RoutineCallResponse | null = null;
  constructor(object: DatabaseObjectId, ownerId: string, port: RoutineCallPort, private readonly onResult: (response: RoutineCallResponse) => void = () => {}) {
    this.previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    this.controller = new RoutineCallController(object, ownerId, port, () => this.update());
    this.dialog = new Dialog({ title: `调用例程 — ${objectLabel(object)}`, width: 680, render: body => this.build(body), buttons: [], modal: true });
    this.dialog.el.style.maxWidth = "calc(100vw - 32px)";
    this.dialog.el.style.maxHeight = "calc(100dvh - 32px)";
    this.dialog.body.style.overflowY = "auto";
    this.dialog.el.setAttribute("role", "dialog"); this.dialog.el.setAttribute("aria-modal", "true");
    const title = this.dialog.el.querySelector<HTMLElement>(".dialog-title-text");
    if (title) { title.id = `${this.prefix}-title`; this.dialog.el.setAttribute("aria-labelledby", title.id); }
    this.dialog.el.setAttribute("aria-describedby", this.status.id);
    // 只修此对话框的旧div关闭控件，不改共享Dialog。
    const oldClose = this.dialog.el.querySelector<HTMLElement>(".dialog-title-close");
    if (oldClose) {
      const close = this.button("关闭参数调用窗口", () => this.close()); close.className = oldClose.className;
      close.textContent = "×"; close.setAttribute("aria-label", "关闭参数调用窗口；运行中只请求停止"); oldClose.replaceWith(close);
    }
    const footer = this.dialog.footer;
    this.reloadButton = this.button("重新读取参数", () => { void this.load(); });
    this.prepareButton = this.button("预览调用", () => { void this.prepare(); });
    this.executeButton = this.button("执行调用", () => { void this.execute(); }); this.executeButton.classList.add("primary");
    this.cancelButton = this.button("停止调用", () => { void this.cancel(); });
    footer.append(this.reloadButton, this.prepareButton, this.executeButton, this.cancelButton, this.button("关闭", () => this.close()));
    this.dialog.el.addEventListener("keydown", event => this.handleKeydown(event));
    this.dialog.onClose(() => { this.alive = false; this.controller.dispose(); if (this.previousFocus?.isConnected) this.previousFocus.focus(); });
    this.update(); this.renderParameters(null); void this.load();
  }
  private button(text: string, click: () => void): HTMLButtonElement {
    const button = document.createElement("button"); button.type = "button"; button.className = "btn"; button.textContent = text; button.addEventListener("click", click); return button;
  }
  private build(body: HTMLElement): void {
    body.style.padding = "16px"; body.style.fontSize = "12px";
    this.status = document.createElement("p"); this.status.id = `${this.prefix}-status`; this.status.setAttribute("role", "status"); this.status.setAttribute("aria-live", "polite");
    this.errors = document.createElement("div"); this.errors.id = `${this.prefix}-error`; this.errors.className = "dialog-inline-error"; this.errors.setAttribute("role", "alert"); this.errors.tabIndex = -1; this.errors.hidden = true;
    this.parameters = document.createElement("div");
    const fieldset = document.createElement("fieldset"); const legend = document.createElement("legend"); legend.textContent = "参数（OUT只显示输出，不接受输入）"; fieldset.append(legend, this.parameters);
    const budgetLabel = document.createElement("label"); budgetLabel.htmlFor = `${this.prefix}-budget`; budgetLabel.textContent = "执行预算（毫秒，0表示无限）";
    this.budget = document.createElement("input"); this.budget.id = budgetLabel.htmlFor; this.budget.type = "number"; this.budget.min = "0"; this.budget.max = "86400000"; this.budget.step = "1"; this.budget.value = "30000"; this.budget.style.margin = "12px 0"; this.budget.style.width = "100%";
    const planLabel = document.createElement("label"); planLabel.htmlFor = `${this.prefix}-plan`; planLabel.textContent = "后端类型化调用预览（不包含输入值）";
    this.plan = document.createElement("textarea"); this.plan.id = planLabel.htmlFor; this.plan.readOnly = true; this.plan.rows = 4; this.plan.style.width = "100%"; this.plan.style.margin = "8px 0";
    this.result = document.createElement("div"); this.result.id = `${this.prefix}-result`; this.result.setAttribute("aria-label", "类型化调用结果与OUT参数"); this.result.style.overflowX = "auto";
    body.append(this.status, this.errors, fieldset, budgetLabel, this.budget, planLabel, this.plan, this.result);
  }
  private renderParameters(metadata: RoutineCallMetadata | null): void {
    this.parameters.replaceChildren(); this.fields.clear();
    if (!metadata) { this.parameters.textContent = "尚未读取参数。权限失败或驱动不支持不会被显示为空参数。"; return; }
    if (!metadata.parameters.length) { this.parameters.textContent = metadata.completeness === "complete" ? "此精确例程没有参数。" : "参数metadata不完整，不能假设是零参数例程。"; return; }
    for (const parameter of metadata.parameters) {
      const row = document.createElement("div"); row.style.padding = "10px 0"; row.style.borderBottom = "1px solid var(--border-color, #d5d5d5)";
      const label = document.createElement("label"); const id = `${this.prefix}-p-${parameter.ordinal}`; label.htmlFor = id; label.textContent = `${parameter.ordinal}. ${parameter.name ?? "未命名"} · ${parameter.mode.toUpperCase()} · ${parameter.data_type}`; label.style.display = "block"; label.style.marginBottom = "6px";
      row.append(label);
      if (parameter.mode === "out") { const output = document.createElement("output"); output.id = id; output.textContent = "执行后读取输出"; row.append(output); this.parameters.append(row); continue; }
      const modeLabel = document.createElement("label"); modeLabel.htmlFor = `${id}-mode`; modeLabel.textContent = "输入方式"; modeLabel.style.marginRight = "8px";
      const mode = document.createElement("select"); mode.id = modeLabel.htmlFor;
      for (const [value, text] of [["value", "值（空字符串不是NULL）"], ["null", "SQL NULL"], ...(parameter.has_default === true && parameter.mode === "in" ? [["default", "省略并使用数据库默认值"]] : [])]) {
        const option = document.createElement("option"); option.value = value; option.textContent = text; mode.append(option);
      }
      const input = document.createElement("input"); input.id = id; input.type = "text"; input.autocomplete = "off"; input.style.width = "100%"; input.style.marginTop = "6px";
      if (/blob|bytea|binary/i.test(parameter.data_type)) input.placeholder = "十六进制：0x0041FF；空二进制输入0x";
      const error = document.createElement("div"); error.id = `${id}-error`; error.className = "dialog-inline-error"; error.hidden = true; input.setAttribute("aria-describedby", error.id);
      const change = () => {
        try { this.controller.setInput(parameter.ordinal, { mode: mode.value as ParameterInput["mode"], text: input.value }); error.hidden = true; input.removeAttribute("aria-invalid"); }
        catch (cause) { this.showError(cause); }
      };
      mode.addEventListener("change", change); input.addEventListener("input", change);
      row.append(modeLabel, mode, input, error); this.parameters.append(row); this.fields.set(parameter.ordinal, { input, mode, error });
    }
  }
  private update(): void {
    if (!this.alive || !this.status || !this.prepareButton) return;
    const controller = this.controller, busy = controller.busy;
    this.dialog.el.setAttribute("aria-busy", String(busy));
    if (controller.metadata !== this.renderedMetadata) { this.renderedMetadata = controller.metadata; this.renderParameters(controller.metadata); }
    const metadata = controller.metadata;
    this.status.textContent = controller.loading ? "正在读取精确对象与真实参数…" : controller.preparing ? "正在生成一次性类型化计划…" : controller.execution.busy ? `执行状态：${controller.execution.state}；等待服务端完成/停止确认和资源排空，不能重复执行。` : `${metadata?.call.reason ?? "尚未读取参数"}${metadata?.notes.length ? ` ${metadata.notes.join("；")}` : ""}`;
    this.prepareButton.disabled = busy || !controller.callable; this.executeButton.disabled = busy || !controller.preview; this.reloadButton.disabled = busy;
    this.cancelButton.disabled = !controller.execution.busy || ["succeeded", "failed", "cancelled", "timed_out", "unknown"].includes(controller.execution.state);
    this.budget.disabled = busy;
    for (const [ordinal, field] of this.fields) { field.mode.disabled = busy || !controller.callable; field.input.disabled = busy || !controller.callable || controller.input(ordinal).mode !== "value"; }
    this.plan.value = controller.preview ? `${controller.preview.sql_preview}\n${controller.preview.warnings.join("\n")}` : "";
    this.errors.textContent = controller.error ?? ""; this.errors.hidden = !controller.error;
    const response = controller.response;
    this.result.hidden = !response;
    if (response && response !== this.lastResponse) { this.lastResponse = response; this.renderResult(response); this.onResult(response); }
  }
  private renderResult(response: RoutineCallResponse): void {
    this.result.replaceChildren();
    const summary = document.createElement("p"); summary.textContent = `状态 ${response.outcome.state}；effects=${response.outcome.effects}；transaction=${response.outcome.transaction_state}`; this.result.append(summary);
    const details = response.call;
    if (!details) { const note = document.createElement("p"); note.textContent = "没有可验证的类型化结果详情。"; this.result.append(note); return; }
    for (const [index, set] of details.sets.entries()) {
      const table = document.createElement("table"); table.style.fontSize = "13px"; const caption = document.createElement("caption"); caption.textContent = `结果集 ${index + 1} · ${set.rows.length}行 · 影响${set.affected}行`;
      const head = document.createElement("thead"), header = document.createElement("tr");
      for (const column of set.columns) { const th = document.createElement("th"); th.scope = "col"; th.textContent = `${column.name} (${column.data_type})`; header.append(th); }
      head.append(header); const body = document.createElement("tbody");
      for (const row of set.rows) { const tr = document.createElement("tr"); for (const value of row) { const td = document.createElement("td"); td.textContent = value.kind === "null" ? "NULL" : typeof value.value === "string" ? value.value : JSON.stringify(value.value); tr.append(td); } body.append(tr); }
      table.append(caption, head, body); this.result.append(table);
    }
    for (const output of details.outputs) { const p = document.createElement("p"); p.textContent = `OUT ${output.ordinal}. ${output.name} (${output.data_type}) = ${output.value.kind === "null" ? "NULL" : JSON.stringify(output.value.value)}`; this.result.append(p); }
    for (const warning of details.warnings) { const p = document.createElement("p"); p.textContent = warning; this.result.append(p); }
  }
  private showError(cause: unknown): void {
    if (!this.alive) return;
    this.errors.textContent = cause instanceof Error ? cause.message : String(cause); this.errors.hidden = false; this.errors.focus();
  }
  async load(): Promise<void> {
    try { await this.controller.load(); if (this.alive) this.fields.values().next().value?.input.focus(); }
    catch (cause) { this.showError(cause); }
  }
  async prepare(): Promise<void> {
    let invalid: HTMLInputElement | null = null;
    for (const p of this.controller.metadata?.parameters ?? []) {
      const field = this.fields.get(p.ordinal); if (!field) continue;
      try { parameterArgument(p, this.controller.input(p.ordinal)); field.error.hidden = true; field.input.removeAttribute("aria-invalid"); }
      catch (cause) { field.error.textContent = cause instanceof Error ? cause.message : String(cause); field.error.hidden = false; field.input.setAttribute("aria-invalid", "true"); invalid ??= field.input; }
    }
    if (invalid) { invalid.focus(); return; }
    try { await this.controller.prepare(); if (this.alive && this.controller.preview) this.executeButton.focus(); }
    catch (cause) { this.showError(cause); }
  }
  async execute(): Promise<void> {
    try {
      if (!this.budget.value.trim()) throw new Error("执行预算必须填写整数，0明确表示无限");
      const budget = Number(this.budget.value);
      if (!Number.isInteger(budget) || budget < 0 || budget > 86400000) throw new Error("执行预算必须是0..86400000毫秒整数");
      const pending = this.controller.execute(budget); if (!this.cancelButton.disabled) this.cancelButton.focus(); await pending;
      if (this.alive && this.controller.error) this.errors.focus();
    } catch (cause) { this.showError(cause); }
  }
  async cancel(): Promise<void> { try { await this.controller.cancel(); } catch (cause) { this.showError(cause); } }
  handleKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape") { event.preventDefault(); this.close(); return; }
    if (event.key !== "Tab") return;
    const focusable = Array.from(this.dialog.el.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex='0']")).filter(el => !el.hidden && el.getAttribute("aria-hidden") !== "true");
    const first = focusable[0], last = focusable[focusable.length - 1];
    if (!first || !last) { event.preventDefault(); return; }
    if (event.shiftKey && (document.activeElement === first || !this.dialog.el.contains(document.activeElement))) { event.preventDefault(); last.focus(); }
    else if (!event.shiftKey && (document.activeElement === last || !this.dialog.el.contains(document.activeElement))) { event.preventDefault(); first.focus(); }
  }
  close(): void { this.dialog.close(); }
}
