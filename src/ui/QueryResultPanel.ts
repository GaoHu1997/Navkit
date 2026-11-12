/** 受控查询结果：DataGrid 仅显示，类型化编辑和提交始终绑定服务端行令牌。 */
import type { DbValue, MutationPlan, MutationReceipt } from "../core/api";
import { QueryResultModel, type QueryModelSnapshot, type QueryFilterGroup, type QueryFilterOperator, type QueryLocalSort } from "../core/query-result-model";
import { queryActionReason, type QueryColumnOrigin, type QueryRowChange } from "../core/query-editability";
import { encodeGridCopy, type CopyMode } from "../core/grid-clipboard";
import { DataGrid } from "./DataGrid";
import { ColumnSidebar } from "./ColumnSidebar";
import { iconSvgByName, svg, ICON_PLUS_LINE, ICON_MINUS_LINE, ICON_CHECK, ICON_CROSS, type IconDef } from "./icons";
import "./query-result-panel.css";
import "./data-grid-controls.css";

export interface QueryResultPanelPort {
  preview(change: QueryRowChange): Promise<MutationPlan>;
  execute(planId: string): Promise<MutationReceipt>;
  onChanged?(): void;
  /** 编辑焦点变化时刷新宿主的单元格预览面板。 */
  onPreviewChange?(): void;
  toast?(kind: "success" | "error" | "info", message: string): void;
}
type Phase = "idle" | "review" | "executing" | "unknown" | "refresh_required";
type EditorKind = DbValue["kind"] | "default";
interface EditorDraft { target: string; column: string; kind: EditorKind; raw: string; touched: boolean }
interface FilterDraft { logic: "AND" | "OR"; conditions: { column: string; operator: QueryFilterOperator; kind: EditorKind; raw: string; caseSensitive: boolean }[] }
interface Review { plan: MutationPlan; change: QueryRowChange; key: string; serial: number; target: string; preparedAt: number }
export interface QueryResultPanelDraft {
  version: 1;
  context: { source_execution_id: string; result_index: number; generation: string };
  result_token: string | null;
  metadata_revision: string | null;
  changes: QueryRowChange[];
  new_row: [string, DbValue][] | null;
  editor: EditorDraft | null;
  filters: FilterDraft[];
  sorts: QueryLocalSort[];
  hidden_columns: string[];
  invalid_reason: string | null;
  write_evidence: { plan_id: string; state: "in_flight" | "unknown" | "confirmed" | "rolled_back"; message: string } | null;
  replay_allowed: false;
}
const clone = <T>(value: T): T => structuredClone(value);
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const KINDS: [EditorKind, string][] = [["default", "省略字段 / DEFAULT"], ["null", "NULL（不同于空字符串）"], ["text", "文本"], ["integer", "整数（十进制文字）"], ["decimal", "精确小数"], ["float", "浮点数"], ["bool", "布尔值 true / false"], ["binary", "二进制（Base64）"], ["date", "日期 YYYY-MM-DD"], ["time", "时间 HH:mm:ss"], ["timestamp", "本地时间戳"], ["timestamptz", "带时区时间戳"], ["uuid", "UUID"], ["json", "JSON 原文"]];
const OPS: [QueryFilterOperator, string][] = [["contains", "包含文本"], ["eq", "等于"], ["ne", "不等于"], ["lt", "小于"], ["lte", "小于等于"], ["gt", "大于"], ["gte", "大于等于"], ["starts_with", "文本开头"], ["ends_with", "文本结尾"], ["is_null", "是 NULL"], ["is_not_null", "不是 NULL"]];
function element<K extends keyof HTMLElementTagNameMap>(tag: K, className = "", text?: string): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag); el.className = className; if (text !== undefined) el.textContent = text; return el;
}
function button(text: string, action: () => void): HTMLButtonElement { const el = element("button", "btn", text); el.type = "button"; el.addEventListener("click", action); return el; }
function label(text: string, input: HTMLElement): HTMLLabelElement { const el = element("label", "qr-field"); el.append(element("span", "", text), input); return el; }
function select(items: readonly (readonly [string, string])[], value: string): HTMLSelectElement {
  const el = element("select"); for (const [id, text] of items) { const opt = element("option", "", text); opt.value = id; el.append(opt); } el.value = value; return el;
}
function message(error: unknown): string { return error instanceof Error ? error.message : typeof error === "string" ? error : "请求未得到可核验的回执"; }
type InsertOrigin = QueryColumnOrigin & { insertable?: boolean; auto_increment?: boolean; nullable?: boolean; default_value?: string | null };
function insertReason(column: QueryColumnOrigin | undefined): string | null {
  if (!column) return "此列缺少服务端来源证明"; const origin = column as InsertOrigin;
  if (origin.generated || origin.auto_increment === true) return "生成列或自增列由数据库生成，仅允许省略";
  if (origin.insertable === true) return null;
  if (origin.insertable !== undefined || !origin.writable || origin.key) return origin.reason || "此列没有明确的新增权限，仅允许省略";
  return null;
}
function text(value: DbValue): string | null { return value.kind === "null" ? null : value.kind === "binary" ? `base64:${value.value}` : String(value.value); }
function exact(value: unknown, fields: string[]): value is Record<string, unknown> { return !!value && typeof value === "object" && !Array.isArray(value) && Object.keys(value).sort().join("|") === [...fields].sort().join("|"); }
/** 复用纯模型的完整运行时值校验；不从网格显示文字恢复类型。 */
function typed(value: unknown): DbValue {
  new QueryResultModel({ source_execution_id: "typed-editor", result_index: 0, generation: "typed-editor", columns: [{ id: "v", name: "v", data_type: "" }], rows: [{ token: "v", values: [value as DbValue] }] });
  return clone(value as DbValue);
}
export function parseQueryPanelValue(kind: EditorKind, raw: string): DbValue | undefined {
  if (kind === "default") return undefined;
  if (kind === "null") return { kind: "null" };
  if (kind === "int") kind = "integer";
  if (kind === "bool") { if (raw !== "true" && raw !== "false") throw new Error("布尔值只能填写 true 或 false"); return { kind, value: raw === "true" }; }
  if (kind === "float") {
    if (!/^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?$/i.test(raw.trim())) throw new Error("浮点数必须是有限的完整数字；空字符串不是 0");
    return typed({ kind, value: Number(raw) });
  }
  return typed({ kind, value: kind === "text" || kind === "json" ? raw : raw.trim() });
}
function checkedPlan(value: unknown, revision: string): MutationPlan {
  if (!exact(value, ["plan_id", "sql", "binds", "base_revision", "expected_rows", "summary"]) || typeof value.plan_id !== "string" || !UUID.test(value.plan_id)
      || typeof value.sql !== "string" || !value.sql.trim() || typeof value.summary !== "string" || value.base_revision !== revision || value.expected_rows !== 1
      || !Array.isArray(value.binds) || value.binds.length > 16384 || new TextEncoder().encode(JSON.stringify(value)).byteLength > 32 * 1024 * 1024) throw new Error("写入预览回执不完整或结构版本不匹配，未发送写入");
  value.binds.forEach(typed); return clone(value) as unknown as MutationPlan;
}
function checkedReceipt(value: unknown, review: Review): MutationReceipt {
  if (!exact(value, ["plan_id", "status", "transaction", "affected", "verified", "mismatches", "sql"]) || value.plan_id !== review.plan.plan_id || value.sql !== review.plan.sql
      || !Number.isSafeInteger(value.affected) || typeof value.verified !== "boolean" || !Array.isArray(value.mismatches) || !value.mismatches.every(m => typeof m === "string")) throw new Error("写入回执身份或证据不完整");
  const success = value.status === "applied" && value.affected === 1 && value.verified === true && value.transaction === "committed" && value.mismatches.length === 0;
  const unchanged = value.status === "unchanged" && review.change.operation === "update" && value.affected === 0 && value.verified === true && ["committed", "rolled_back"].includes(String(value.transaction)) && value.mismatches.length === 0;
  const rolledBack = ["failed", "conflict"].includes(String(value.status)) && value.affected === 0 && value.verified === false && ["rolled_back", "none"].includes(String(value.transaction)) && value.mismatches.length > 0;
  if (!success && !unchanged && !rolledBack) throw new Error("写入结果未知或事务、影响行数与核验状态矛盾");
  return clone(value) as unknown as MutationReceipt;
}

export class QueryResultPanel {
  readonly el = element("section", "qr-panel");
  private readonly model: QueryResultModel;
  private readonly snapshot: QueryModelSnapshot;
  private readonly grid: DataGrid;
  private readonly status = element("p", "qr-status");
  private readonly error = element("p", "qr-error");
  private readonly note = element("p", "qr-note");
  private readonly editorHost = element("section", "qr-editor");
  private readonly reviewHost = element("section", "qr-review");
  /** 网格底部操作条：与表数据页共用同一套网格页脚按钮。 */
  private readonly footer = element("div", "grid-footer");
  private readonly filterHost = element("div", "qr-rules");
  private readonly sortHost = element("div", "qr-rules");
  private readonly rowSelect = select([["", "选择一行"]], "");
  private readonly copyMode = select([["rows_with_headers", "复制可见列（含表头）"], ["rows", "复制可见列（无表头）"], ["cell", "复制当前单元格"], ["spreadsheet_safe", "复制为电子表格安全文本"]], "rows_with_headers");
  private readonly newButton: HTMLButtonElement;
  private readonly deleteButton: HTMLButtonElement;
  private readonly previewButton: HTMLButtonElement;
  private readonly discardButton: HTMLButtonElement;
  private readonly uid = `query-panel-${crypto.randomUUID()}`;
  private phase: Phase = "idle";
  private disposed = false;
  private pendingPreviews = 0;
  private serial = 0;
  private review: Review | null = null;
  private editor: EditorDraft | null = null;
  private newRow: Map<string, DbValue> | null = null;
  private target = "";
  private focusedColumn = "";
  private visibleTokens: string[] = [];
  private hidden = new Set<string>();
  private columnSidebar!: ColumnSidebar;
  private filters: FilterDraft[] = [];
  private sorts: QueryLocalSort[] = [];
  private readonly drafts = new Map<string, QueryRowChange>();
  private readonly consumedPlans = new Set<string>();
  private invalidReason: string | null = null;
  private writeEvidence: QueryResultPanelDraft["write_evidence"] = null;

  constructor(snapshot: QueryModelSnapshot, private readonly port: QueryResultPanelPort) {
    this.snapshot = clone(snapshot); this.model = new QueryResultModel(snapshot);
    snapshot.columns.filter(c => c.hidden).forEach(c => this.hidden.add(c.id));
    if (snapshot.columns.length && snapshot.columns.every(c => this.hidden.has(c.id))) { this.hidden.delete(snapshot.columns[0].id); this.model.setHiddenColumns([...this.hidden]); }
    this.status.setAttribute("role", "status"); this.status.setAttribute("aria-live", "polite");
    this.error.id = `${this.uid}-error`; this.error.setAttribute("role", "alert"); this.error.hidden = true;
    this.note.id = `${this.uid}-scope`; this.el.setAttribute("aria-label", "查询结果与受控编辑");
    this.el.setAttribute("aria-describedby", this.note.id); this.rowSelect.setAttribute("aria-describedby", `${this.note.id} ${this.error.id}`);
    this.grid = new DataGrid({
      onSelectionChange: rows => this.attempt(() => this.selectTokens(rows.map(i => this.visibleTokens[i]).filter((v): v is string => !!v))),
      onCellFocus: (row, col) => this.attempt(() => this.focusCell(row, col)),
      onEditStart: (row, col) => this.attempt(() => this.beginGridEdit(row, col)),
      onEditEnd: (row, col, value) => this.attempt(() => this.commitGridEdit(row, col, value)),
      onEditBegin: () => { try { this.port.onPreviewChange?.(); } catch { /* 预览刷新失败不影响写入证据 */ } },
      onRowDoubleClick: row => this.attempt(() => { this.focusCell(row, Math.max(0, this.visibleColumns().findIndex(c => c.id === this.focusedColumn))); this.editorHost.querySelector<HTMLElement>("textarea,select")?.focus(); }),
      onColumnClick: (_name, index) => this.attempt(() => this.toggleSort(index)),
    });
    this.grid.el.setAttribute("aria-label", "已加载的类型化查询结果：双击单元格可直接修改，提交时仍走后端计划核验");
    const iconBtn = (icon: IconDef, title: string, action: () => void) => {
      const b = element("button", "grid-footer-btn");
      b.type = "button"; b.title = title; b.setAttribute("aria-label", title);
      b.innerHTML = svg(icon); b.addEventListener("click", action); return b;
    };
    this.newButton = iconBtn(ICON_PLUS_LINE, "插入记录", () => this.attempt(() => this.beginInsert()));
    this.deleteButton = iconBtn(ICON_MINUS_LINE, "删除记录", () => this.attempt(() => this.markSelectedDeleted()));
    this.discardButton = iconBtn(ICON_CROSS, "清除本机未提交草稿（不执行数据库回滚）", () => this.attempt(() => this.discardSelected()));
    this.previewButton = iconBtn(ICON_CHECK, "提交改动（先预览后端写入计划）", () => { void this.previewSelected(); });
    this.footer.append(this.newButton, this.deleteButton, this.previewButton, this.discardButton);
    const actions = element("div", "qr-toolbar");
    actions.append(label("当前行", this.rowSelect));
    this.rowSelect.addEventListener("change", () => this.attempt(() => this.selectTokens(this.rowSelect.value ? [this.rowSelect.value] : [])));
    const copy = element("div", "qr-toolbar"); copy.append(button("选择全部可见行", () => this.attempt(() => this.selectTokens([...this.visibleTokens]))), label("复制内容", this.copyMode), button("复制", () => { void this.copy(); }));
    const controls = element("div", "qr-tools");
    controls.append(this.ruleDetails("本地筛选（各组之间 AND）", this.filterHost), this.ruleDetails("多列排序（按优先级）", this.sortHost), this.columnDetails());
    const body = element("div", "qr-body"); body.append(this.grid.el, this.editorHost);
    this.reviewHost.hidden = true; this.reviewHost.setAttribute("aria-label", "后端写入预览与确认");
    const columnBody = element("div", "qr-column-body"); columnBody.append(body, this.columnSidebar.el);
    this.el.append(this.status, this.note, controls, actions, copy, this.error, columnBody, this.reviewHost, this.footer);
    this.renderFilters(); this.renderSorts(); this.render();
  }
  get dirty(): boolean { return this.model.dirty || this.newRow !== null || this.editor?.touched === true || this.writeEvidence?.state === "unknown" || this.writeEvidence?.state === "in_flight"; }
  get busy(): boolean { return this.pendingPreviews > 0 || this.phase === "review" || this.phase === "executing" || this.writeEvidence?.state === "unknown"; }
  captureDraft(): QueryResultPanelDraft {
    return clone({ version: 1, context: this.model.context, result_token: this.snapshot.editability?.result_token ?? null, metadata_revision: this.snapshot.editability?.metadata_revision ?? null,
      changes: [...this.drafts.values()], new_row: this.newRow === null ? null : [...this.newRow], editor: this.editor, filters: this.filters, sorts: this.sorts,
      hidden_columns: [...this.hidden], invalid_reason: this.invalidReason, write_evidence: this.writeEvidence, replay_allowed: false });
  }
  invalidate(reason: string): void {
    this.invalidReason = reason || "查询结果上下文已改变，请处理草稿后显式重新执行"; this.model.invalidate(this.invalidReason); this.revokeReview();
    if (this.phase !== "executing" && this.phase !== "unknown") this.phase = "refresh_required";
    if (!this.disposed) { this.report(this.invalidReason); this.render(); this.changed(); }
  }
  activate(): void { if (!this.disposed) { this.render(); this.grid.el.focus({ preventScroll: true }); } }
  dispose(): void {
    if (this.disposed) return;
    if (this.phase === "executing" && this.writeEvidence) { this.writeEvidence.state = "unknown"; this.writeEvidence.message = "组件关闭时写入回执尚未确认，禁止自动重放"; this.invalidReason = this.writeEvidence.message; }
    this.disposed = true; ++this.serial; this.review = null; this.grid.dispose();
  }
  selectTokens(tokens: string[]): void {
    this.guardBrowse(); if (this.editor?.touched) throw new Error("字段输入尚未应用；请先应用或取消该输入，再切换行");
    this.model.select(tokens); this.target = tokens.length === 1 ? tokens[0] : ""; this.editor = null; this.revokeReview(); this.render(); this.changed();
  }
  stageCell(token: string, columnId: string, value: DbValue): void {
    this.guardWrite(); this.model.stage(token, columnId, value); this.revokeReview(); this.rememberDrafts(); this.render(); this.changed();
  }
  beginInsert(): void {
    this.guardWrite(); const reason = queryActionReason(this.snapshot.editability ?? null, "insert"); if (reason) throw new Error(reason);
    if (this.editor?.touched) throw new Error("请先应用或取消当前字段输入");
    this.newRow ??= new Map(); this.target = "new"; this.editor = null; this.revokeReview(); this.render(); this.changed();
  }
  setInsertValue(columnId: string, value: DbValue | undefined): void {
    this.guardWrite(); if (this.newRow === null) throw new Error("请先新增一行");
    const origin = this.snapshot.editability?.columns.find(c => c.result_column_id === columnId);
    const denied = insertReason(origin); if (denied) throw new Error(denied);
    if (value?.kind === "null" && (origin as InsertOrigin).nullable === false) throw new Error("服务端元数据标明此列不允许 NULL；可省略字段保留默认值");
    if (value === undefined) this.newRow.delete(columnId); else this.newRow.set(columnId, typed(value));
    this.revokeReview(); this.render(); this.changed();
  }
  markSelectedDeleted(): void {
    this.guardWrite(); const token = this.singleSelected(); if (this.editor?.touched) throw new Error("请先处理当前字段输入");
    this.model.markDeleted(token); this.revokeReview(); this.rememberDrafts(); this.render(); this.changed();
  }
  discardSelected(): void {
    this.guardWrite(); if (this.target === "new") this.newRow = null;
    else { const token = this.singleSelected(); this.model.discard(token); this.drafts.delete(token); }
    this.editor = null; this.revokeReview(); this.render(); this.changed();
  }
  setLocalFilters(groups: QueryFilterGroup[]): void { this.guardBrowse(); this.model.setFilters(groups); this.revokeReview(); this.render(); this.changed(); }
  setLocalSorts(sorts: QueryLocalSort[]): void { this.guardBrowse(); this.model.setSorts(sorts); this.sorts = clone(sorts); this.revokeReview(); this.render(); this.changed(); }
  setHiddenColumns(ids: string[]): void {
    this.guardBrowse();
    if (this.snapshot.columns.length && this.snapshot.columns.every(column => ids.includes(column.id))) throw new Error("至少保留一个显示字段");
    if (this.editor?.touched && ids.includes(this.editor.column)) throw new Error("字段输入尚未应用；请先应用或取消当前输入，再隐藏此列");
    this.model.setHiddenColumns(ids); this.hidden = new Set(ids); this.revokeReview(); this.render(); this.columnSidebar.render(); this.changed();
  }
  copyText(mode: CopyMode = "rows_with_headers"): string {
    this.guardBrowse(); const cell = this.target && this.target !== "new" && this.focusedColumn ? { token: this.target, column_id: this.focusedColumn } : undefined;
    return encodeGridCopy(this.model.copySnapshot(undefined, mode === "cell" ? cell : undefined), mode);
  }
  async previewSelected(): Promise<boolean> {
    if (this.disposed || this.pendingPreviews || this.phase === "executing") return false;
    let change: QueryRowChange, target: string;
    try { this.guardWrite(); if (this.editor?.touched) throw new Error("字段输入尚未应用，未发送预览；请先应用或取消输入"); change = this.currentChange(); target = this.target; }
    catch (error) { this.report(message(error)); return false; }
    this.revokeReview(); const serial = this.serial; const key = JSON.stringify(change); this.pendingPreviews++; this.error.hidden = true; this.render(); this.changed();
    try {
      const response = await this.port.preview(clone(change));
      if (this.disposed || serial !== this.serial || this.invalidReason || target !== this.target || JSON.stringify(this.currentChange()) !== key || this.editor?.touched) return false;
      const plan = checkedPlan(response, this.snapshot.editability!.metadata_revision!);
      if (this.consumedPlans.has(plan.plan_id)) throw new Error("后端返回了已消费的计划，不能重复提交");
      this.review = { plan, change: clone(change), key, serial, target, preparedAt: Date.now() }; this.phase = "review"; this.renderReview(); this.changed(); return true;
    } catch (error) { if (!this.disposed && serial === this.serial) { this.revokeReview(); this.report(message(error)); } return false; }
    finally { this.pendingPreviews--; if (!this.disposed) { this.render(); this.changed(); } }
  }
  async confirmPreview(): Promise<boolean> {
    if (this.disposed || this.phase !== "review" || !this.review || this.pendingPreviews) return false;
    const review = this.review;
    try {
      this.guardWrite();
      if (review.serial !== this.serial || review.target !== this.target || this.editor?.touched || JSON.stringify(this.currentChange()) !== review.key || Date.now() - review.preparedAt > 110_000
          || this.consumedPlans.has(review.plan.plan_id)) throw new Error("预览后内容、行或有效期已改变，请重新预览；未发送写入");
      if (!this.reviewHost.querySelector<HTMLInputElement>("input[type='checkbox']")?.checked) throw new Error("请先核对后端 SQL、绑定值与仅一行的写入范围，并勾选确认");
    } catch (error) { this.revokeReview(); this.report(message(error)); this.render(); this.changed(); return false; }
    this.consumedPlans.add(review.plan.plan_id); this.review = null; this.phase = "executing";
    this.writeEvidence = { plan_id: review.plan.plan_id, state: "in_flight", message: "已发送一次，等待事务与影响行数核验；不会重试" }; this.render(); this.changed();
    try {
      const value = await this.port.execute(review.plan.plan_id);
      if (this.disposed) return false;
      const receipt = checkedReceipt(value, review);
      if (receipt.status === "conflict" || receipt.status === "failed") {
        this.writeEvidence = { plan_id: receipt.plan_id, state: "rolled_back", message: receipt.mismatches.join("；") };
        this.phase = "refresh_required"; this.invalidate(`后端已确认未应用：${receipt.mismatches.join("；")}。草稿已保留，请核对后显式重新执行查询`); return false;
      }
      if (review.change.operation === "insert") this.newRow = null;
      else { this.model.discard(review.target); this.drafts.delete(review.target); }
      this.editor = null; this.writeEvidence = { plan_id: receipt.plan_id, state: "confirmed", message: `${receipt.status} / ${receipt.transaction} / ${receipt.affected}` };
      this.model.requireRefreshAfterWrite(); this.phase = "refresh_required"; this.invalidReason = "写入已核验；旧结果令牌与筛选成员关系需要显式重新执行查询。其他行草稿已保留";
      this.report(this.invalidReason, false); try { this.port.toast?.("success", receipt.status === "unchanged" ? "后端核验为未变化" : "此行写入已提交并核验"); } catch { /* 通知失败不改变已核验的事务证据。 */ } this.render(); this.changed(); return true;
    } catch (error) {
      if (this.disposed) return false;
      this.phase = "unknown"; this.invalidReason = `写入结果未确认：${message(error)}。请在数据库中核对；禁止重放、刷新覆盖或删除此草稿`;
      this.writeEvidence = { plan_id: review.plan.plan_id, state: "unknown", message: this.invalidReason }; this.model.invalidate(this.invalidReason); this.report(this.invalidReason); this.render(); this.changed(); return false;
    }
  }
  cancelPreview(): void { if (this.phase === "executing" || this.disposed) return; this.revokeReview(); this.render(); this.previewButton.focus({ preventScroll: true }); this.changed(); }
  private currentChange(): QueryRowChange {
    if (this.target === "new") {
      if (this.newRow === null) throw new Error("新增草稿不存在"); const reason = queryActionReason(this.snapshot.editability ?? null, "insert"); if (reason) throw new Error(reason);
      return { result_token: this.snapshot.editability!.result_token!, row_token: null, operation: "insert", changes: [...this.newRow].map(([id, value]) => [id, clone(value)]) };
    }
    const token = this.singleSelected(); const row = this.snapshot.rows.find(r => r.token === token)!;
    const change = this.model.changes().find(c => c.row_token === row.row_token); if (!change) throw new Error("此行没有待提交改动；每次只预览并提交一行"); return clone(change);
  }
  private singleSelected(): string { const selected = this.model.selectedTokens(); if (selected.length !== 1 || selected[0] !== this.target) throw new Error("请只选择一行；批量提交不受支持"); return selected[0]; }
  private guardBrowse(): void { if (this.disposed) throw new Error("结果组件已关闭"); }
  private guardWrite(): void { this.guardBrowse(); if (this.phase === "executing") throw new Error("写入已发送，等待回执，不能修改草稿"); if (this.invalidReason || this.phase === "unknown" || this.phase === "refresh_required") throw new Error(this.invalidReason || "请重新执行查询取得新结果"); }
  private revokeReview(): void { ++this.serial; this.review = null; if (this.phase === "review") this.phase = "idle"; this.reviewHost.replaceChildren(); this.reviewHost.hidden = true; }
  private rememberDrafts(): void {
    this.drafts.clear(); for (const change of this.model.changes()) { const row = this.snapshot.rows.find(r => r.row_token === change.row_token); if (row) this.drafts.set(row.token, clone(change)); }
  }
  private changed(): void { if (!this.disposed) try { this.port.onChanged?.(); } catch { this.report("宿主未能保存状态通知；当前草稿与写入证据仍保留"); } }
  private attempt(action: () => void): void { try { action(); } catch (error) { if (!this.disposed) this.report(message(error)); } }
  private report(text: string, failure = true): void { if (this.disposed) return; this.error.textContent = text; this.error.hidden = !text; this.error.dataset.level = failure ? "error" : "info"; }
  private visibleColumns(): QueryModelSnapshot["columns"] { return this.snapshot.columns.filter(c => !this.hidden.has(c.id)); }

  /** 当前焦点单元格的显示文本（「文本▼」预览读取）。 */
  focusedCellValue(): string | null {
    const cell = this.grid.focusedCell;
    if (!cell) return null;
    return this.grid.rows[cell.row]?.[cell.col] ?? null;
  }

  /** 是否有焦点单元格：NULL 单元格的显示文本同样是 null，必须与「没有选中」区分。 */
  hasFocusedCell(): boolean {
    return !!this.grid.focusedCell;
  }

  /** 双击单元格进入内联编辑：可写权限先由服务端来源证明判定。 */
  private beginGridEdit(row: number, col: number): void {
    const token = this.visibleTokens[row], column = this.visibleColumns()[col];
    if (!token || !column) return;
    try {
      this.guardWrite();
      const denied = queryActionReason(this.snapshot.editability ?? null, "update", column.id);
      if (denied) throw new Error(denied);
    } catch (error) { this.report(message(error)); return; }
    this.target = token; this.focusedColumn = column.id; this.editor = null; this.revokeReview();
    this.rowSelect.value = token;
    this.grid.beginEdit(row, col, this.grid.rows[row]?.[col] ?? null);
  }

  /** 内联编辑确认：只登记本地类型化草稿，写入仍须预览并核对后端计划。 */
  private commitGridEdit(row: number, col: number, value: string | null): void {
    const token = this.visibleTokens[row], column = this.visibleColumns()[col];
    if (!token || !column) return;
    this.guardWrite();
    const denied = queryActionReason(this.snapshot.editability ?? null, "update", column.id);
    if (denied) throw new Error(denied);
    const index = this.snapshot.columns.findIndex(c => c.id === column.id);
    const current = this.model.values(token)[index];
    // 类型来自服务端原值，不从网格显示文字反推。
    const kind: EditorKind = current?.kind === "int" ? "integer" : current?.kind ?? "text";
    let next: DbValue;
    if (value === null) next = { kind: "null" };
    else if (kind === "null") next = { kind: "text", value };
    else {
      const parsed = parseQueryPanelValue(kind, value);
      if (parsed === undefined) throw new Error("更新不能省略值；请填写具体值或 NULL");
      next = parsed;
    }
    this.stageCell(token, column.id, next);
  }
  private focusCell(row: number, col: number): void {
    const token = this.visibleTokens[row], column = this.visibleColumns()[col]; if (!token || !column) return;
    if (this.editor?.touched && (this.editor.target !== token || this.editor.column !== column.id)) throw new Error("请先应用或取消当前字段输入");
    this.target = token; this.focusedColumn = column.id; this.editor = null; this.revokeReview(); this.renderEditor();
    // 选中单元格即刷新宿主预览面板：预览只跟随选中，不靠「文本▼」格式常驻。
    try { this.port.onPreviewChange?.(); } catch { /* 预览刷新失败不影响选中状态 */ }
    this.changed();
  }
  private toggleSort(index: number): void {
    const column = this.visibleColumns()[index]; if (!column) return; const sorts = clone(this.sorts), found = sorts.find(s => s.column_id === column.id);
    if (found) found.ascending = !found.ascending; else sorts.push({ column_id: column.id, ascending: true, nulls: "last" }); this.setLocalSorts(sorts); this.renderSorts();
  }
  private render(): void {
    if (this.disposed) return;
    const view = this.model.view(), columns = this.visibleColumns(), selected = new Set(this.model.selectedTokens()); this.visibleTokens = [...view.tokens];
    const rowFlags: Record<number, "delete" | "dirty"> = {}; view.tokens.forEach((token, index) => { const change = this.drafts.get(token); if (change) rowFlags[index] = change.operation === "delete" ? "delete" : "dirty"; });
    const locked = !!this.invalidReason || this.phase === "executing" || this.phase === "unknown" || this.phase === "refresh_required";
    this.grid.setData({ columns: columns.map(c => ({ name: c.name, type: c.data_type, nullable: true, primary_key: false, auto_increment: false })), rows: view.rows.map(row => columns.map(c => text(row[this.snapshot.columns.findIndex(original => original.id === c.id)]))),
      // 有行即可内联编辑；空结果、锁定态与未知回执仍保持只读。
      readOnly: locked || view.tokens.length === 0,
      selectedRows: new Set(view.tokens.flatMap((token, i) => selected.has(token) ? [i] : [])), rowFlags, lined: true, showRowNumbers: false });
    this.status.textContent = `${view.scope} · 完整结果 ${view.total} 行 · 已选 ${selected.size} 行 · 未提交 ${view.dirty + (this.newRow !== null ? 1 : 0)} 行${view.hidden_dirty ? `（${view.hidden_dirty} 行被筛选隐藏，草稿仍保留）` : ""}`;
    const hiddenColumnDirty = [...this.drafts.values()].some(change => change.changes.some(([id]) => this.hidden.has(id))) || (this.newRow !== null && [...this.newRow.keys()].some(id => this.hidden.has(id)));
    this.note.textContent = `${this.snapshot.rows.length < view.total ? `仅当前类型化前缀 ${view.loaded} 行可操作；前缀外 ${view.total - view.loaded} 行保持只读。` : "筛选、排序与复制仅作用于已加载行。"} ${hiddenColumnDirty ? "隐藏列内有未提交值，草稿仍保留。" : ""} ${this.snapshot.editability?.mode === "single_table" ? "编辑须通过后端逐行预览与核验。" : this.snapshot.editability?.reason || "缺少来源证明，保持只读。"} ${view.warnings.join("；")}`;
    this.rowSelect.replaceChildren(...[["", "选择一行"], ...view.tokens.map(token => [token, `已加载第 ${this.snapshot.rows.findIndex(row => row.token === token) + 1} 行${this.drafts.has(token) ? "（有草稿）" : ""}`])].map(([value, title]) => { const option = element("option", "", title); option.value = value; return option; }));
    this.rowSelect.value = this.target !== "new" && view.tokens.includes(this.target) ? this.target : "";
    this.el.setAttribute("aria-busy", String(this.pendingPreviews > 0 || this.phase === "executing"));
    const reason = (action: "insert" | "delete") => queryActionReason(this.snapshot.editability ?? null, action);
    this.newButton.disabled = locked || !!reason("insert"); this.newButton.title = this.invalidReason || reason("insert") || "新增行默认省略所有字段；仅显式设置值才发送该字段";
    this.deleteButton.disabled = locked || selected.size !== 1 || this.target === "new" || !!reason("delete"); this.deleteButton.title = this.invalidReason || reason("delete") || "按选中行的服务端令牌标记，预览后单独确认";
    this.previewButton.disabled = locked || !!this.pendingPreviews || this.target !== "new" && selected.size !== 1; this.previewButton.title = this.invalidReason || "仅预览当前一行的服务端写入计划";
    this.discardButton.disabled = locked || !this.target; this.discardButton.title = this.invalidReason || "撤销当前行尚未发送的草稿";
    this.renderEditor(); if (this.phase === "review" && this.review) this.renderReview(); else if (this.phase === "executing") { this.reviewHost.hidden = false; this.reviewHost.replaceChildren(element("p", "", this.writeEvidence!.message)); }
  }
  private renderEditor(): void {
    if (this.disposed) return; this.editorHost.replaceChildren(element("h3", "", this.target === "new" ? "新增行的类型化字段" : "类型化字段编辑"));
    if (!this.target) { this.editorHost.append(element("p", "qr-note", "选择一行与字段；网格显示文本不参与回写。可使用行选择框和 Tab 键操作。")); return; }
    const columns = this.visibleColumns(); if (!columns.length) { this.editorHost.append(element("p", "qr-note", "所有列均已隐藏；显示字段后才能编辑或复制。")); return; }
    if (!columns.some(c => c.id === this.focusedColumn)) this.focusedColumn = columns.find(c => this.snapshot.editability?.columns.find(o => o.result_column_id === c.id)?.writable)?.id ?? columns[0].id;
    const column = columns.find(c => c.id === this.focusedColumn)!, origin = this.snapshot.editability?.columns.find(c => c.result_column_id === column.id);
    const selector = select(columns.map(c => [c.id, `${c.name} · ${c.data_type}`]), column.id);
    selector.addEventListener("change", () => this.attempt(() => { if (this.editor?.touched) { selector.value = this.focusedColumn; throw new Error("请先应用或取消当前字段输入"); } this.focusedColumn = selector.value; this.editor = null; this.revokeReview(); this.renderEditor(); }));
    const inserting = this.target === "new", value = inserting ? this.newRow?.get(column.id) : this.model.values(this.target)[this.snapshot.columns.findIndex(c => c.id === column.id)];
    if (!this.editor || this.editor.target !== this.target || this.editor.column !== column.id) this.editor = { target: this.target, column: column.id, kind: value?.kind === "int" ? "integer" : value?.kind ?? "default", raw: value && value.kind !== "null" ? String(value.value) : "", touched: false };
    const draft = this.editor;
    const kind = select(KINDS.filter(([id]) => inserting || id !== "default"), draft.kind);
    const input = element("textarea"); input.rows = 5; input.value = draft.raw; input.spellcheck = false; input.setAttribute("aria-describedby", `${this.uid}-editor-help ${this.error.id}`);
    const update = () => { draft.kind = kind.value as EditorKind; draft.raw = input.value; draft.touched = true; input.disabled = kind.value === "null" || kind.value === "default"; input.removeAttribute("aria-invalid"); kind.removeAttribute("aria-invalid"); this.revokeReview(); this.changed(); };
    kind.addEventListener("change", update); input.addEventListener("input", update);
    const denied = this.invalidReason || (this.phase === "executing" ? "写入正在核验，字段已锁定" : inserting ? insertReason(origin) : queryActionReason(this.snapshot.editability ?? null, "update", column.id));
    selector.disabled = this.phase === "executing"; kind.disabled = !!denied; input.disabled = !!denied || kind.value === "null" || kind.value === "default";
    const help = element("p", "qr-note", denied || (inserting ? "省略字段会保留数据库默认值；NULL 与空文本分别选择。键列、生成列不推测可写权限。" : "输入来自类型化原值；应用只保存本地草稿。整数、小数和二进制保持原始精度。")); help.id = `${this.uid}-editor-help`;
    const apply = button(inserting ? "设置新增字段" : "应用到此行草稿", () => this.attempt(() => {
      if (denied) throw new Error(denied);
      try { const value = parseQueryPanelValue(draft.kind, draft.raw); draft.touched = false; if (inserting) this.setInsertValue(column.id, value); else { if (value === undefined) throw new Error("更新不能省略值；请选择 NULL 或明确类型"); this.stageCell(this.target, column.id, value); } }
      catch (error) { draft.touched = true; input.setAttribute("aria-invalid", "true"); kind.setAttribute("aria-invalid", "true"); input.focus({ preventScroll: true }); throw error; }
    })); apply.disabled = !!denied;
    const cancel = button("取消字段输入", () => { this.editor = null; this.revokeReview(); this.renderEditor(); this.changed(); }); cancel.disabled = this.phase === "executing" || this.phase === "unknown";
    this.editorHost.append(label("字段", selector), label("输入类型", kind), label("类型化值", input), help, apply, cancel);
    if (inserting) this.editorHost.append(element("p", "qr-note", `当前显式字段 ${this.newRow?.size ?? 0} 个，其余字段省略；新增行不借用旧行令牌。`));
  }
  private renderReview(): void {
    if (!this.review || this.disposed) return; const review = this.review; this.reviewHost.hidden = false;
    const consent = element("input"); consent.type = "checkbox"; consent.checked = false;
    const confirm = button("确认并仅提交此行一次", () => { void this.confirmPreview(); }); confirm.classList.add("primary"); confirm.disabled = true;
    consent.addEventListener("change", () => { confirm.disabled = !consent.checked || this.phase !== "review"; });
    this.reviewHost.replaceChildren(element("h3", "", "后端写入计划 · 预计影响 1 行"), element("p", "qr-note", `${review.plan.summary} · ${review.change.operation} · 结构版本 ${review.plan.base_revision}`),
      element("pre", "qr-sql", review.plan.sql), element("pre", "qr-binds", JSON.stringify(review.plan.binds, null, 2)), label("我已核对 SQL、全部类型化绑定值及此行范围", consent), confirm, button("取消预览", () => this.cancelPreview()));
    this.reviewHost.onkeydown = event => { if (event.key === "Escape") { event.preventDefault(); this.cancelPreview(); } };
    queueMicrotask(() => { if (!this.disposed && this.review === review) consent.focus({ preventScroll: true }); });
  }
  private ruleDetails(title: string, host: HTMLElement): HTMLDetailsElement { const details = element("details"); details.append(element("summary", "", title), host); return details; }
  private columnDetails(): HTMLButtonElement {
    this.columnSidebar = new ColumnSidebar(`${this.uid}-columns`, () => this.snapshot.columns, () => new Set(this.hidden), ids => this.attempt(() => this.setHiddenColumns(ids)));
    const trigger = button("", () => this.columnSidebar.toggle()); trigger.classList.add("qr-column-trigger");
    trigger.innerHTML = iconSvgByName("columns"); trigger.title = "选择显示字段"; trigger.setAttribute("aria-label", "选择显示字段");
    trigger.disabled = !this.snapshot.columns.length; this.columnSidebar.bind(trigger); this.columnSidebar.render(); return trigger;
  }
  private renderFilters(): void {
    this.filterHost.replaceChildren(element("p", "qr-note", "各组之间为 AND；组内可选 AND / OR。仅改变已加载结果，不向服务器发送 SQL。"));
    this.filters.forEach((group, groupIndex) => {
      const box = element("fieldset", "qr-rule-group"); box.append(element("legend", "", `条件组 ${groupIndex + 1}`));
      const logic = select([["AND", "满足全部条件（AND）"], ["OR", "满足任一条件（OR）"]], group.logic); logic.addEventListener("change", () => { group.logic = logic.value as "AND" | "OR"; this.revokeReview(); }); box.append(label("组内逻辑", logic));
      group.conditions.forEach((condition, conditionIndex) => {
        const row = element("div", "qr-rule"); const column = select(this.snapshot.columns.map(c => [c.id, c.name]), condition.column), op = select(OPS, condition.operator), kind = select(KINDS.filter(([k]) => k !== "default" && k !== "null"), condition.kind);
        const input = element("input"); input.value = condition.raw; input.disabled = condition.operator === "is_null" || condition.operator === "is_not_null";
        const sensitive = element("input"); sensitive.type = "checkbox"; sensitive.checked = condition.caseSensitive;
        const update = () => { condition.column = column.value; condition.operator = op.value as QueryFilterOperator; condition.kind = kind.value as EditorKind; condition.raw = input.value; condition.caseSensitive = sensitive.checked; input.disabled = op.value === "is_null" || op.value === "is_not_null"; this.revokeReview(); };
        for (const control of [column, op, kind, sensitive]) control.addEventListener("change", update); input.addEventListener("input", update);
        row.append(label("筛选列", column), label("比较", op), label("比较值类型", kind), label("比较值", input), label("区分大小写", sensitive), button(`移除条件 ${conditionIndex + 1}`, () => { group.conditions.splice(conditionIndex, 1); this.revokeReview(); this.renderFilters(); })); box.append(row);
      });
      box.append(button("增加条件", () => { if (!this.snapshot.columns.length) return; group.conditions.push({ column: this.snapshot.columns[0].id, operator: "contains", kind: "text", raw: "", caseSensitive: false }); this.revokeReview(); this.renderFilters(); }), button(`移除条件组 ${groupIndex + 1}`, () => { this.filters.splice(groupIndex, 1); this.revokeReview(); this.renderFilters(); })); this.filterHost.append(box);
    });
    this.filterHost.append(button("增加条件组", () => { this.filters.push({ logic: "AND", conditions: [] }); this.revokeReview(); this.renderFilters(); }), button("应用本地筛选", () => this.attempt(() => this.setLocalFilters(this.filters.map(group => ({ logic: group.logic, conditions: group.conditions.map(c => ({ column_id: c.column, operator: c.operator, case_sensitive: c.caseSensitive, ...(c.operator === "is_null" || c.operator === "is_not_null" ? {} : { value: parseQueryPanelValue(c.kind, c.raw)! }) })) }))))), button("清除本地筛选", () => this.attempt(() => { this.filters = []; this.setLocalFilters([]); this.renderFilters(); })));
  }
  private renderSorts(): void {
    this.sortHost.replaceChildren(); this.sorts.forEach((sort, index) => {
      const row = element("div", "qr-rule"), column = select(this.snapshot.columns.map(c => [c.id, c.name]), sort.column_id), direction = select([["asc", "升序"], ["desc", "降序"]], sort.ascending ? "asc" : "desc"), nulls = select([["first", "NULL 在前"], ["last", "NULL 在后"]], sort.nulls);
      const update = () => this.attempt(() => { const next = clone(this.sorts); next[index] = { column_id: column.value, ascending: direction.value === "asc", nulls: nulls.value as "first" | "last" }; this.setLocalSorts(next); this.renderSorts(); });
      [column, direction, nulls].forEach(control => control.addEventListener("change", update));
      const up = button(`提高排序 ${index + 1} 优先级`, () => this.attempt(() => { const next = clone(this.sorts); [next[index - 1], next[index]] = [next[index], next[index - 1]]; this.setLocalSorts(next); this.renderSorts(); })); up.disabled = index === 0;
      row.append(label(`排序 ${index + 1}`, column), label("方向", direction), label("NULL 位置", nulls), up, button(`移除排序 ${index + 1}`, () => this.attempt(() => { this.setLocalSorts(this.sorts.filter((_, i) => i !== index)); this.renderSorts(); }))); this.sortHost.append(row);
    });
    this.sortHost.append(button("增加排序列", () => this.attempt(() => { const column = this.snapshot.columns.find(c => !this.sorts.some(s => s.column_id === c.id)); if (!column) throw new Error("所有列均已在排序中"); this.setLocalSorts([...this.sorts, { column_id: column.id, ascending: true, nulls: "last" }]); this.renderSorts(); })), button("清除排序", () => this.attempt(() => { this.setLocalSorts([]); this.renderSorts(); })));
  }
  private async copy(): Promise<void> {
    try { const value = this.copyText(this.copyMode.value as CopyMode); await navigator.clipboard.writeText(value); if (!this.disposed) this.port.toast?.("success", "已复制选中的可见类型化列（仅已加载范围）"); }
    catch (error) { if (!this.disposed) this.report(`复制未完成：${message(error)}；数据未截断，请选择较小范围或导出`); }
  }
}
