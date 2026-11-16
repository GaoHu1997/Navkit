import { api, type TableDesignSnapshot, type TableDesignPayload, type TableDesignPreview, type TableDesignResponse, type TableOptionChoices, type IndexDesign } from "../core/api";
import { openPopover, closePopover, isPopoverOpenFor, POPOVER_CHEVRON } from "./popover";
import { highlightSql } from "./sql-highlight";
import type { ConnectionConfig, TabState, TableOptionsDesign } from "../core/types";
import type { DraftSnapshot } from "../core/editing-state";
import { ExecutionController } from "../core/execution";
import { assertDesignSnapshot, designClone, draftFromSnapshot, draftSignature, payloadFromDraft, verifiedDesignResponse, type TableDesignerDraft, type DesignerColumn } from "../core/table-design-draft";
import { reviewTablePlan } from "../dialogs/TablePlanDialog";
import "../styles/table-designer.css";

export interface TableDesignerHost {
  connection(): ConnectionConfig | undefined;
  writeReason(): string | null;
  changed(): void;
  status(): void;
  toast(kind: "error" | "info" | "success", text: string): void;
}
/** 可见页签：检查 / 触发器 / 选项三页暂时收起（实现与草稿载荷仍保留，恢复时加回本数组即可）。 */
const TABS = ["字段", "索引", "外键", "注释", "SQL 预览"];
/** 索引页里主键是固定首行，用这个哨兵值参与选中态。 */
const PRIMARY_ROW = "PRIMARY";
/** 各数据库方言的真实类型清单：类型选择只出方言支持的项，杜绝随意拼类型导致数据库拒绝。 */
const DIALECT_TYPES: Record<string, string[]> = {
  mysql: ["tinyint", "smallint", "mediumint", "int", "bigint", "decimal", "numeric", "float", "double", "bit", "boolean", "bool",
    "char", "varchar", "tinytext", "text", "mediumtext", "longtext", "binary", "varbinary", "tinyblob", "blob", "mediumblob", "longblob",
    "json", "enum", "set", "date", "datetime", "timestamp", "time", "year", "geometry", "point", "linestring", "polygon"],
  mariadb: ["tinyint", "smallint", "mediumint", "int", "bigint", "decimal", "numeric", "float", "double", "bit", "boolean", "bool",
    "char", "varchar", "tinytext", "text", "mediumtext", "longtext", "binary", "varbinary", "tinyblob", "blob", "mediumblob", "longblob",
    "json", "enum", "set", "date", "datetime", "timestamp", "time", "year", "geometry", "point", "linestring", "polygon"],
  postgresql: ["smallint", "integer", "bigint", "decimal", "numeric", "real", "double precision", "money", "serial", "bigserial",
    "char", "varchar", "text", "bytea", "timestamp", "timestamptz", "date", "time", "timetz", "interval", "boolean",
    "uuid", "json", "jsonb", "xml", "inet", "cidr", "macaddr", "point", "line", "box", "path", "polygon", "circle"],
  sqlite: ["integer", "real", "text", "blob", "numeric", "boolean", "varchar", "date", "datetime"],
  sqlserver: ["tinyint", "smallint", "int", "bigint", "bit", "decimal", "numeric", "money", "smallmoney", "float", "real",
    "date", "datetime2", "datetime", "smalldatetime", "time", "datetimeoffset",
    "char", "varchar", "text", "nchar", "nvarchar", "ntext", "binary", "varbinary", "image",
    "uniqueidentifier", "xml", "geometry", "geography"],
  oracle: ["number", "integer", "float", "binary_float", "binary_double", "char", "nchar", "varchar2", "nvarchar2",
    "clob", "nclob", "blob", "bfile", "raw", "long", "long raw", "date", "timestamp", "timestamp with time zone",
    "interval year to month", "interval day to second", "rowid", "urowid", "json"],
};
/** 未知方言（ODBC / JDBC / 自定义驱动）兜底：SQL 标准常用类型。既有列的真实类型始终原样保留显示。 */
const GENERIC_TYPES = ["integer", "bigint", "smallint", "decimal", "numeric", "float", "double", "real",
  "char", "varchar", "text", "blob", "boolean", "date", "time", "timestamp", "datetime", "json", "uuid"];
/** 后端允许自增的类型集合（与 src-tauri/src/design.rs 的校验保持一致）。 */
const AUTO_INCREMENT_TYPES = ["tinyint", "smallint", "mediumint", "int", "integer", "bigint", "float", "double", "real"];
/** 可带 UNSIGNED / ZEROFILL 修饰的数值类型。 */
const NUMERIC_TYPES = [...AUTO_INCREMENT_TYPES, "decimal", "numeric", "bit", "boolean", "bool", "money", "smallmoney", "number", "year"];
/** 基础类型名：去掉长度/精度参数并转小写，用于判断默认值候选与修饰可用性。 */
function baseType(dataType: string): string {
  return (dataType || "").toLowerCase().replace(/\(.*$/, "").trim();
}
/** 按字段类型给出默认值候选（参考 通用数据库客户端：' ' / NULL / 0 / 1 / CURRENT_*），仍允许手动输入任意表达式。 */
function defaultChoices(dataType: string): string[] {
  const type = baseType(dataType);
  const numeric = NUMERIC_TYPES.includes(type);
  const temporal = ["date", "datetime", "datetime2", "datetimeoffset", "timestamp", "timestamptz", "time", "timetz", "smalldatetime"].includes(type);
  const textual = ["char", "varchar", "tinytext", "text", "mediumtext", "longtext", "nchar", "nvarchar", "ntext", "varchar2", "nvarchar2", "clob", "nclob", "string", "xml", "json", "jsonb", "uuid", "enum", "set", "uniqueidentifier", "image"].includes(type);
  const choices: string[] = [];
  if (textual) choices.push("' '", "''");
  if (numeric) choices.push("0", "1");
  if (temporal) choices.push("CURRENT_TIMESTAMP", "CURRENT_DATE", "CURRENT_TIME");
  choices.push("NULL");
  return choices;
}
let designerId = 0;
export class TableDesigner {
  readonly el: HTMLElement;
  private readonly id = `table-designer-${++designerId}`;
  private snapshot: TableDesignSnapshot | null = null;
  private draft: TableDesignerDraft | null = null;
  private baseline = "";
  private baselineDraft: TableDesignerDraft | null = null;
  private currentTab = "字段";
  private selected = "";
  /** 各页签当前选中的条目（对象引用），供工具栏「删除X」定位。 */
  private selectedIndex: unknown = null;
  private selectedForeignKey: unknown = null;
  private selectedCheck: unknown = null;
  private selectedTrigger: unknown = null;
  private loading = false;
  private saving = false;
  private disposed = false;
  private locked = false;
  private message = "";
  private preview: TableDesignPreview | null = null;
  private previewSignature = "";
  private previewState: "idle" | "loading" | "ok" | "error" = "idle";
  private previewError = "";
  private previewTimer: ReturnType<typeof setTimeout> | null = null;
  private previewToken = 0;
  /** 「选项」页的真实候选项（引擎/字符集/排序规则/行格式），只在需要时读取一次。 */
  private optionChoices: { values: TableOptionChoices | null; error: string; loading: boolean } = { values: null, error: "", loading: false };
  private metadataVersion = 0;
  private execution: ExecutionController | null = null;
  constructor(private readonly tab: TabState, private readonly host: TableDesignerHost, el: HTMLElement) {
    this.el = el;
    this.el.classList.add("table-designer");
    this.render();
  }
  activate(): void { if (!this.snapshot && !this.loading) void this.reload().catch(e => this.fail(e)); }
  flushDraft(): void { const active = document.activeElement; if (active instanceof HTMLElement && this.el.contains(active)) active.blur(); }
  draftDirty(): boolean { return !!this.draft && draftSignature(this.draft) !== this.baseline; }
  draftBusy(): boolean { return this.loading || this.saving || !!this.execution?.busy; }
  isSavingStructure(): boolean { return this.draftBusy(); }
  captureDraft(): Pick<DraftSnapshot, "kind" | "baseline" | "payload"> {
    this.flushDraft();
    return { kind: "table_structure", baseline: this.snapshot ? JSON.parse(JSON.stringify(this.snapshot)) : null,
      payload: { draft: this.draft ? JSON.parse(JSON.stringify(this.draft)) : null, preserve_undefined: ["indexes", "foreign_keys", "checks", "triggers", "options", "comment"].filter(key => this.draft?.[key as keyof TableDesignerDraft] === undefined),
        outcome_requires_reconciliation: this.locked, requires_validation: true } };
  }
  discardDraft(): void {
    if (this.draftBusy()) throw new Error("表设计操作仍在执行，暂时不能丢弃草稿。");
    this.flushDraft();
    if (this.locked) throw new Error("执行结果尚未核对，原始草稿和执行证据需要保留；请重新载入真实结构核对。");
    if (this.baselineDraft) this.draft = designClone(this.baselineDraft);
    this.tab.dirty = false;
    this.preview = null;
    this.message = this.locked ? "执行结果尚未完整证实；请重新载入真实结构核对后再编辑。" : "已恢复全部草稿基线。";
    this.render(); this.host.changed();
  }
  async reloadAfterLeave(): Promise<void> { await this.load(true); }
  async reload(): Promise<void> { await this.load(false); }
  private async load(force: boolean): Promise<void> {
    if (this.loading || this.disposed) return;
    if (this.execution?.busy) throw new Error("执行尚未停止或资源尚未排空，请等待后再载入结构。");
    this.flushDraft();
    if (!force && this.draftDirty()) return;
    this.loading = true; this.render(); this.host.changed();
    try {
      const s = await api.readTableDesignSnapshot(this.tab.connectionId, this.tab.schema, this.tab.object, this.tab.id);
      if (this.disposed) return;
      assertDesignSnapshot(s);
      if (s.owner_id !== this.tab.id || s.design.table !== this.tab.object || s.design.schema !== (this.tab.schema || s.schema.database)) throw new Error("表设计快照身份不匹配，原草稿已保留。");
      this.accept(s); this.message = ""; this.locked = false;
    } catch (error) { this.message = `读取表设计失败：${String(error)}。当前草稿已保留。`; throw error; }
    finally { this.loading = false; this.render(); this.host.changed(); this.host.status(); }
  }
  private accept(snapshot: TableDesignSnapshot): void {
    const draft = draftFromSnapshot(snapshot);
    this.metadataVersion++; this.optionChoices = { values: null, error: "", loading: false };
    this.snapshot = designClone(snapshot); this.draft = draft; this.baselineDraft = designClone(draft); this.baseline = draftSignature(draft);
    this.tab.schemaInfo = designClone(snapshot.schema); this.tab.dirty = false;
    this.preview = null; this.previewSignature = "";
    if (!draft.columns.some(c => c.key === this.selected)) this.selected = draft.columns[0]?.key ?? "";
  }
  private changed(): void {
    this.tab.dirty = this.draftDirty();
    const status = this.el.querySelector<HTMLElement>(".designer-status");
    if (status) status.textContent = this.tab.dirty ? "有未保存的表结构草稿。" : "表结构与读取基线一致。";
    this.host.changed();
    this.queuePreview();
  }
  private fail(error: unknown): void { this.message = String(error); this.host.toast("error", this.message); this.render(); }
  private editReason(): string | null { return this.host.writeReason() || (this.loading || this.saving ? "正在读取或保存表结构。" : this.locked ? "上次执行结果需要核对；请重新载入真实结构，不能自动重放。" : null); }
  private mutate(action: () => void, focus?: string): void {
    this.flushDraft(); const reason = this.editReason(); if (reason) { this.host.toast("info", reason); return; }
    action(); this.changed(); this.render();
    if (focus) this.el.querySelector<HTMLElement>(focus)?.focus();
  }
  addField(): void { this.insert(false); }
  insertField(): void { this.insert(true); }
  private insert(before: boolean): void {
    if (!this.draft) return;
    const draft = this.draft;
    this.mutate(() => {
      const column: DesignerColumn = { key: `${this.id}-new-${crypto.randomUUID()}`, deleted: false, name: "", orig_name: null, data_type: "varchar", length: "100", scale: null, nullable: true, default_value: null, auto_increment: false, comment: null, unsigned: false, zerofill: false };
      const index = before ? draft.columns.findIndex(c => c.key === this.selected) : -1;
      draft.columns.splice(index < 0 ? draft.columns.length : index, 0, column); this.selected = column.key; this.currentTab = "字段";
    }, `[data-field-key="${this.id}-unused"]`);
    this.el.querySelector<HTMLInputElement>(`[data-field-key="${this.selected}"] input`)?.focus();
  }
  deleteField(): void {
    const column = this.draft?.columns.find(c => c.key === this.selected); if (!column || !this.draft) { this.host.toast("info", "请先选择字段。"); return; }
    const draft = this.draft;
    this.mutate(() => { if (!column.orig_name) { draft.columns.splice(draft.columns.indexOf(column), 1); this.selected = draft.columns[0]?.key ?? ""; } else column.deleted = !column.deleted; });
  }
  togglePrimaryKey(): void {
    const column = this.draft?.columns.find(c => c.key === this.selected); if (!column || !this.draft) { this.host.toast("info", "请先选择字段。"); return; }
    const draft = this.draft;
    this.mutate(() => { const at = draft.primary_key.indexOf(column.name); if (at >= 0) draft.primary_key.splice(at, 1); else draft.primary_key.push(column.name); });
  }
  /** 当前页签名（工具栏据此切换按钮）。 */
  get activeTabName(): string { return this.currentTab; }
  addIndex(): void { const list = this.draft?.indexes; if (!list) return; this.mutate(() => { const item = { name: "", columns: [] as string[], unique: false, method: "BTREE", descending: [] as string[] }; list.push(item); this.selectedIndex = item; }); }
  deleteIndex(): void {
    const list = this.draft?.indexes; if (!list) return;
    if (this.selectedIndex === PRIMARY_ROW) { this.host.toast("info", "主键不能直接删除；请把它的索引类型改成 NORMAL 或 UNIQUE 降级后再删除。"); return; }
    const item = list.find(x => (x as unknown) === this.selectedIndex);
    if (!item) { this.host.toast("info", "请先在索引列表中选择要删除的行。"); return; }
    this.mutate(() => { const at = list.indexOf(item); if (at >= 0) list.splice(at, 1); this.selectedIndex = (this.draft?.indexes ?? []).length ? (this.draft!.indexes![0]) : PRIMARY_ROW; });
  }
  addForeignKey(): void { const list = this.draft?.foreign_keys; if (!list) return; this.mutate(() => { const item = { name: "", columns: [], ref_schema: this.snapshot!.design.schema, ref_table: "", ref_columns: [], on_update: null, on_delete: null }; list.push(item); this.selectedForeignKey = item; }); }
  deleteForeignKey(): void { const list = this.draft?.foreign_keys; if (!list) return; const item = list.find(x => (x as unknown) === this.selectedForeignKey); if (!item) { this.host.toast("info", "请先选择外键。"); return; } this.mutate(() => { const at = list.indexOf(item); if (at >= 0) list.splice(at, 1); this.selectedForeignKey = list[0] ?? null; }); }
  addCheck(): void { const list = this.draft?.checks; if (!list) return; this.mutate(() => { const item = { name: "", expression: "", orig_name: null }; list.push(item); this.selectedCheck = item; }); }
  deleteCheck(): void { const list = this.draft?.checks; if (!list) return; const item = list.find(x => (x as unknown) === this.selectedCheck); if (!item) { this.host.toast("info", "请先选择检查约束。"); return; } this.mutate(() => { const at = list.indexOf(item); if (at >= 0) list.splice(at, 1); this.selectedCheck = list[0] ?? null; }); }
  addTrigger(): void { const list = this.draft?.triggers; if (!list) return; this.mutate(() => { const item = { name: "", timing: "BEFORE", event: "INSERT", statement: "", orig_name: null }; list.push(item); this.selectedTrigger = item; }); }
  deleteTrigger(): void { const list = this.draft?.triggers; if (!list) return; const item = list.find(x => (x as unknown) === this.selectedTrigger); if (!item) { this.host.toast("info", "请先选择触发器。"); return; } this.mutate(() => { const at = list.indexOf(item); if (at >= 0) list.splice(at, 1); this.selectedTrigger = list[0] ?? null; }); }
  fieldMoveReason(direction: -1 | 1): string | null {
    const reason = this.editReason(); if (reason) return reason;
    if (!this.draft || !this.snapshot) return "请先等待完整结构加载。";
    const orderReason = this.columnOrderReason(); if (orderReason) return orderReason;
    if (this.currentTab !== "字段") return "请先切换到字段页。";
    const at = this.draft.columns.findIndex(c => c.key === this.selected);
    if (at < 0) return "请选择字段。";
    if (at + direction < 0 || at + direction >= this.draft.columns.length) return direction < 0 ? "已是第一个字段。" : "已是最后一个字段。";
    const before = this.baselineDraft!;
    if (this.draft.columns.length !== before.columns.length || this.draft.columns.some(c => !before.columns.some(b => draftSignature({ ...before, columns: [b] }) === draftSignature({ ...before, columns: [c] }))) ||
      draftSignature({ ...this.draft, columns: [] }) !== draftSignature({ ...before, columns: [] })) return "字段排序不能与其它结构修改混合；请先保存或丢弃其它修改。";
    return null;
  }
  moveField(direction: -1 | 1): void {
    this.flushDraft(); const reason = this.fieldMoveReason(direction); if (reason) { this.host.toast("info", reason); this.host.changed(); return; }
    const draft = this.draft!;
    this.mutate(() => { const at = draft.columns.findIndex(c => c.key === this.selected); [draft.columns[at], draft.columns[at + direction]] = [draft.columns[at + direction], draft.columns[at]]; });
    this.el.querySelector<HTMLInputElement>(`[data-field-key="${this.selected}"] input`)?.focus();
  }
  private columnOrderReason(): string | null {
    const cfg = this.host.connection();
    if (!this.snapshot?.design.schema) return "未知命名空间，不能安全调整物理字段顺序。";
    if (!cfg || !["mysql", "mariadb"].includes(cfg.kind) || cfg.params.startsWith("@navkit-driver:")) return "当前协议不支持安全原地调整物理字段顺序。";
    const before = this.baselineDraft, draft = this.draft;
    if (!before || !draft) return "请先等待完整结构加载。";
    if (draft.columns.length !== before.columns.length || draft.columns.some(c => !before.columns.some(b => draftSignature({ ...before, columns: [b] }) === draftSignature({ ...before, columns: [c] }))) || draftSignature({ ...draft, columns: [] }) !== draftSignature({ ...before, columns: [] })) return "字段排序不能与其它结构修改混合；请先保存或丢弃其它修改。";
    return null;
  }
  private payload(): TableDesignPayload {
    if (!this.snapshot || !this.draft || !this.baselineDraft) throw new Error("尚未读取完整表设计基线。");
    const current = this.draft.columns.filter(c => !c.deleted && c.orig_name).map(c => c.orig_name!);
    const original = this.baselineDraft.columns.filter(c => current.includes(c.orig_name!)).map(c => c.orig_name!);
    if (JSON.stringify(current) !== JSON.stringify(original)) { const reason = this.columnOrderReason(); if (reason) throw new Error(reason); }
    // 外键的引用库 / 引用表 / 引用字段按草稿里填写的值原样提交，不再要求引用元数据先被读取证实。
    return payloadFromDraft(this.snapshot, this.draft);
  }
  private async makePreview(whileSaving = false): Promise<{ design: TableDesignPayload; preview: TableDesignPreview }> {
    this.flushDraft(); const reason = whileSaving ? this.host.writeReason() || (this.loading || this.locked ? "结构基线需要重新读取核对。" : null) : this.editReason(); if (reason) throw new Error(reason);
    const design = this.payload(); const signature = draftSignature(this.draft!);
    const preview = await api.previewTableDesign(this.tab.connectionId, design, this.tab.id);
    if (!preview.request_sql || preview.revision !== this.snapshot!.revision) throw new Error("后端预览缺少绑定载荷或基线版本不一致。");
    if (this.disposed || signature !== draftSignature(this.draft!)) throw new Error("预览期间草稿发生变化，请重新预览。");
    this.preview = preview; this.previewSignature = signature;
    return { design, preview };
  }
  async saveChanges(): Promise<void> {
    this.flushDraft(); if (!this.draftDirty()) { this.host.toast("info", "没有待保存的结构修改。"); return; }
    if (this.draftBusy()) return;
    const reason = this.editReason(); if (reason) { this.host.toast("error", reason); return; }
    this.saving = true; this.render(); this.host.changed();
    let sent = false;
    try {
      const prepared = await this.makePreview(true);
      const signature = this.previewSignature;

      const confirmed = await reviewTablePlan("保存表结构", prepared.preview, `目标：${this.tab.schema || this.snapshot!.schema.database}.${this.tab.object}`, prepared.preview.destructive ? "此计划包含删除或重建操作，请确认已查看实际 SQL。" : "确认后执行本次后端计划。");
      if (!confirmed || this.disposed) return;
      if (signature !== draftSignature(this.draft!)) throw new Error("确认期间草稿发生变化，未发送修改。");
      const blocked = this.host.writeReason(); if (blocked) throw new Error(blocked);
      let response: TableDesignResponse | null = null;
      this.execution = new ExecutionController({ ...api, executeSqlV2: async request => { sent = true; response = await api.executeTableDesign(request, prepared.design); return response.outcome; } }, () => this.host.changed());
      await this.execution.run({ execution_id: crypto.randomUUID(), connection_id: this.tab.connectionId, namespace: prepared.design.schema, owner_id: this.tab.id,
        sql: prepared.preview.request_sql, origin: "table-design" });
      if (!response) throw new Error("缺少表结构执行回执。");
      const snapshot = verifiedDesignResponse(response);
      if (snapshot.owner_id !== this.tab.id || snapshot.design.table !== this.tab.object || snapshot.design.schema !== prepared.design.schema) throw new Error("成功回执的对象身份不匹配，草稿已保留。");
      this.accept(snapshot); this.message = "表结构已保存，完整读回验证通过。"; this.host.toast("success", this.message);
    } catch (error) { if (sent) this.locked = true; this.message = `${String(error)}${sent ? " 原草稿保留；请核对真实状态后重新载入，不能重试重放。" : " 草稿已保留。"}`; this.host.toast("error", this.message); }
    finally { this.saving = false; this.tab.dirty = this.draftDirty(); this.render(); this.host.changed(); this.host.status(); }
  }
  private input(value: string | null | undefined, label: string, set: (value: string) => void, disabled = false): HTMLInputElement {
    const el = document.createElement("input"); el.type = "text"; el.value = value ?? ""; el.setAttribute("aria-label", label); el.disabled = disabled || !!this.editReason();
    el.addEventListener("input", () => { set(el.value); this.changed(); }); return el;
  }
  private select(value: string, choices: string[], label: string, set: (value: string) => void, disabled = false): HTMLSelectElement {
    const el = document.createElement("select"); el.setAttribute("aria-label", label); el.disabled = disabled || !!this.editReason();
    for (const choice of [...new Set([value, ...choices])]) { const option = document.createElement("option"); option.value = choice; option.textContent = choice || "（未设置）"; el.appendChild(option); }
    el.value = value; el.addEventListener("change", () => { set(el.value); this.changed(); }); return el;
  }
  /** 默认值单元格：一个框同时承担回显与手动输入，右侧箭头只负责给候选。
   *  选中候选后直接写回该框，不再拼第二个输入框。 */
  private defaultCell(column: DesignerColumn, label: string): HTMLElement {
    const wrap = document.createElement("div"); wrap.className = "designer-default-cell";
    const field = this.input(column.default_value ?? "", label + " 默认值", value => { column.default_value = value; }, column.deleted);
    field.className = "designer-default-input";
    field.placeholder = "NULL（空）";
    field.title = "可直接输入任意默认值表达式，例如 CURRENT_TIMESTAMP、' ' 或 0。";
    const pick = document.createElement("button"); pick.type = "button"; pick.className = "designer-default-pick";
    pick.setAttribute("aria-label", label + " 默认值候选");
    pick.title = "选择候选默认值";
    pick.disabled = field.disabled;
    pick.innerHTML = POPOVER_CHEVRON;
    pick.addEventListener("click", event => {
      event.preventDefault();
      if (pick.disabled) return;
      if (isPopoverOpenFor(pick)) { closePopover(); return; }
      this.openDefaultChoices(pick, column, field, label);
    });
    wrap.append(field, pick);
    return wrap;
  }
  /** 默认值候选浮层：候选随类型变化（整数 0/1、时间 CURRENT_*、字符 ' '），选中即回显并关闭。 */
  private openDefaultChoices(anchor: HTMLButtonElement, column: DesignerColumn, field: HTMLInputElement, label: string): void {
    const choices = defaultChoices(column.data_type);
    anchor.classList.add("open");
    openPopover({
      anchor,
      className: "choices",
      width: 232,
      ariaLabel: label + " 默认值候选",
      onClose: () => anchor.classList.remove("open"),
      render: body => {
        for (const choice of choices) {
          const option = document.createElement("button"); option.type = "button"; option.className = "popover-option";
          if ((column.default_value ?? "") === choice) option.classList.add("current");
          const value = document.createElement("span"); value.className = "popover-option-value"; value.textContent = choice;
          option.appendChild(value);
          option.addEventListener("click", () => {
            column.default_value = choice; field.value = choice;
            this.changed(); this.render(); closePopover();
          });
          body.appendChild(option);
        }
        const hint = document.createElement("p"); hint.className = "popover-hint";
        hint.textContent = "也可以直接在默认值框里键入任意表达式。";
        body.appendChild(hint);
      },
    });
  }
  /** 扩展列：自增 / 无符号 / 填充零不再各占一列，收进这一格的浮层里。
   *  只有 MySQL 家族提供这些属性；类型不支持时对应项在浮层里置灰。 */
  private extensionCell(column: DesignerColumn, label: string): HTMLElement {
    const btn = document.createElement("button"); btn.type = "button"; btn.className = "designer-ext";
    btn.setAttribute("aria-label", label + " 扩展");
    btn.title = "扩展属性：自增 / 无符号 / 填充零";
    btn.disabled = column.deleted || !!this.editReason();
    const text = document.createElement("span"); text.className = "designer-ext-text";
    this.fillExtensionSummary(text, column);
    const chevron = document.createElement("span"); chevron.className = "designer-ext-chevron"; chevron.innerHTML = POPOVER_CHEVRON;
    btn.append(text, chevron);
    btn.addEventListener("click", () => {
      if (isPopoverOpenFor(btn)) { closePopover(); return; }
      btn.classList.add("open");
      openPopover({
        anchor: btn,
        className: "ext",
        width: 248,
        ariaLabel: label + " 扩展属性",
        onClose: () => btn.classList.remove("open"),
        render: body => {
          const type = baseType(column.data_type);
          const autoAllowed = AUTO_INCREMENT_TYPES.includes(type);
          const numericAllowed = NUMERIC_TYPES.includes(type);
          const entries: Array<{ key: "auto_increment" | "unsigned" | "zerofill"; title: string; tag: string; allowed: boolean }> = [
            { key: "auto_increment", title: "自增", tag: "AUTO_INCREMENT", allowed: autoAllowed },
            { key: "unsigned", title: "无符号", tag: "UNSIGNED", allowed: numericAllowed },
            { key: "zerofill", title: "填充零", tag: "ZEROFILL", allowed: numericAllowed },
          ];
          for (const entry of entries) {
            const row = document.createElement("label"); row.className = "popover-check" + (entry.allowed ? "" : " disabled");
            const box = document.createElement("input"); box.type = "checkbox"; box.checked = !!column[entry.key];
            box.disabled = !entry.allowed; box.setAttribute("aria-label", label + " " + entry.title);
            box.addEventListener("change", () => {
              column[entry.key] = box.checked;
              if (entry.key === "auto_increment" && box.checked) { column.nullable = false; column.default_value = null; }
              this.changed();
              // 浮层保持打开，只刷新单元格摘要与本页其它单元格
              this.fillExtensionSummary(text, column);
              const nullableBox = this.el.querySelector<HTMLInputElement>(`input[type="checkbox"][aria-label="${label} 允许 NULL"]`);
              if (nullableBox) nullableBox.checked = column.nullable;
            });
            const title = document.createElement("span"); title.className = "popover-check-title"; title.textContent = entry.title;
            const tag = document.createElement("span"); tag.className = "popover-check-tag"; tag.textContent = entry.tag;
            row.append(box, title, tag); body.appendChild(row);
          }
          const hint = document.createElement("p"); hint.className = "popover-hint";
          hint.textContent = numericAllowed ? "整数类型可自增；数值类型可无符号 / 填充零。" : `${type || "该类型"} 不是数值类型，三项都不可选。`;
          body.appendChild(hint);
        },
      });
    });
    return btn;
  }
  private fillExtensionSummary(text: HTMLElement, column: DesignerColumn): void {
    text.textContent = "";
    const active = [column.auto_increment ? "自增" : "", column.unsigned ? "无符号" : "", column.zerofill ? "填充零" : ""].filter(Boolean);
    if (!active.length) { text.classList.add("empty"); text.textContent = "—"; return; }
    text.classList.remove("empty");
    for (const name of active) { const tag = document.createElement("span"); tag.className = "designer-ext-tag"; tag.textContent = name; text.appendChild(tag); }
  }
  /** 只有复选框、没有可见文字（表头已说明语义），用 aria-label 保留可访问名。 */
  private checkBare(value: boolean, ariaLabel: string, set: (value: boolean) => void, disabled = false): HTMLLabelElement {
    const wrapper = document.createElement("label"); wrapper.className = "designer-check designer-check-bare";
    const el = document.createElement("input"); el.type = "checkbox"; el.checked = value; el.setAttribute("aria-label", ariaLabel); el.disabled = disabled || !!this.editReason();
    el.addEventListener("change", () => { set(el.checked); this.changed(); }); wrapper.appendChild(el); return wrapper;
  }
  /** 主键钥匙：只有已在主键中的字段才显示图标；空白格同样可点，点一下即加入主键并显示图标。 */
  private keyToggle(active: boolean, ariaLabel: string, action: () => void, disabled = false): HTMLButtonElement {
    const el = document.createElement("button"); el.type = "button"; el.className = "designer-key" + (active ? " active" : " empty");
    if (active) el.innerHTML = '<svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" aria-hidden="true"><circle cx="4.6" cy="8" r="2.4"/><path d="M7 8h6.4M11 8v3.2"/></svg>';
    el.setAttribute("aria-label", ariaLabel); el.setAttribute("aria-pressed", String(active));
    el.title = active ? "已在主键中，点击移出" : "点击加入主键"; el.disabled = disabled || !!this.editReason();
    el.addEventListener("click", action); return el;
  }
  private button(text: string, action: () => void, reason: string | null = this.editReason()): HTMLButtonElement {
    const el = document.createElement("button"); el.type = "button"; el.className = "btn"; el.textContent = text; el.disabled = !!reason; if (reason) el.title = reason;
    el.addEventListener("click", action); return el;
  }
  private labeled(label: string, control: HTMLElement, reason?: string): HTMLElement {
    const wrapper = document.createElement("label"); wrapper.className = "designer-control"; const text = document.createElement("span"); text.textContent = label; wrapper.appendChild(text); wrapper.appendChild(control);
    if (reason) { const hint = document.createElement("small"); hint.textContent = reason; wrapper.appendChild(hint); } return wrapper;
  }
  private textarea(value: string, label: string, set: (value: string) => void, disabled = false): HTMLTextAreaElement {
    const el = document.createElement("textarea"); el.value = value; el.setAttribute("aria-label", label); el.disabled = disabled || !!this.editReason();
    el.addEventListener("input", () => { set(el.value); this.changed(); }); return el;
  }
  private render(): void {
    if (this.disposed) return;
    this.el.innerHTML = ""; this.el.setAttribute("aria-busy", String(this.loading || this.saving));
    const tabs = document.createElement("div"); tabs.className = "designer-tabs"; tabs.setAttribute("role", "tablist"); tabs.setAttribute("aria-label", "表设计分类");
    TABS.forEach((name, i) => {
      const tab = this.button(name, () => this.showTab(name), null); tab.className = `designer-tab${name === this.currentTab ? " active" : ""}`;
      tab.id = `${this.id}-tab-${i}`; tab.setAttribute("role", "tab"); tab.setAttribute("aria-selected", String(name === this.currentTab)); tab.setAttribute("aria-controls", `${this.id}-panel`); tab.tabIndex = name === this.currentTab ? 0 : -1;
      tab.addEventListener("keydown", event => { const to = event.key === "Home" ? 0 : event.key === "End" ? TABS.length - 1 : event.key === "ArrowRight" ? (i + 1) % TABS.length : event.key === "ArrowLeft" ? (i + TABS.length - 1) % TABS.length : -1; if (to >= 0) { event.preventDefault(); this.showTab(TABS[to]); } }); tabs.appendChild(tab);
    });
    this.el.appendChild(tabs);
    const body = document.createElement("div"); body.className = "designer-body"; body.id = `${this.id}-panel`; body.setAttribute("role", "tabpanel"); body.setAttribute("aria-labelledby", `${this.id}-tab-${TABS.indexOf(this.currentTab)}`); body.tabIndex = 0; this.el.appendChild(body);
    if (this.message) { const notice = document.createElement("div"); notice.className = "designer-notice"; notice.setAttribute("role", "status"); notice.textContent = this.message; body.appendChild(notice); }
    if (!this.snapshot || !this.draft) { const text = document.createElement("p"); text.textContent = this.loading ? "正在读取完整表设计…" : "表设计元数据尚未完整读取。"; body.appendChild(text); return; }
    if (this.currentTab === "字段") this.renderFields(body);
    else if (this.currentTab === "索引") this.renderIndexes(body);
    else if (this.currentTab === "外键") this.renderForeignKeys(body);
    else if (this.currentTab === "检查") this.renderChecks(body);
    else if (this.currentTab === "触发器") this.renderTriggers(body);
    else if (this.currentTab === "选项") this.renderOptions(body);
    else if (this.currentTab === "注释") { const unsupported = this.host.connection()?.kind === "sqlite"; body.appendChild(this.labeled("表注释", this.textarea(this.draft.comment ?? "", "表注释", value => { this.draft!.comment = value; }, unsupported), unsupported ? "SQLite 没有表注释 DDL，当前协议不能保存表注释。" : undefined)); }
    else this.renderPreview(body);
    const status = document.createElement("div"); status.className = "designer-status"; status.setAttribute("role", "status"); status.setAttribute("aria-live", "polite"); status.textContent = this.locked ? "执行结果需要核对；已禁止重放。" : this.tab.dirty ? "有未保存的表结构草稿。" : "表结构与读取基线一致。"; this.el.appendChild(status);
  }
  private showTab(name: string): void { this.flushDraft(); this.currentTab = name; this.render(); this.host.changed(); this.el.querySelector<HTMLElement>(`#${this.id}-tab-${TABS.indexOf(name)}`)?.focus(); if (name === "SQL 预览") this.queuePreview(0); }
  /** 字段表格的列定义：类型与默认值都在列内编辑，不再有底部「字段属性」面板。
   *  自增 / 无符号 / 填充零只有 MySQL 家族支持，非 MySQL 连接不显示这三列。
   *  固定列宽：表格是 table-layout: fixed，列宽由 colgroup 决定，输入框才有稳定宽度。 */
  private fieldColumnDefs(): Array<{ title: string; width: string; align?: "center" }> {
    const mysql = ["mysql", "mariadb"].includes(this.host.connection()?.kind ?? "");
    const columns: Array<{ title: string; width: string; align?: "center" }> = [
      { title: "名称", width: "150px" },
      { title: "类型", width: "150px" },
      { title: "长度", width: "68px" },
      { title: "小数位", width: "68px" },
      { title: "默认值", width: "250px" },
      { title: "主键", width: "54px", align: "center" },
      { title: "允许 NULL", width: "80px", align: "center" },
    ];
    if (mysql) columns.push({ title: "扩展", width: "120px" });
    columns.push({ title: "注释", width: "auto" });
    return columns;
  }
  private renderFields(body: HTMLElement): void {
    const draft = this.draft!;
    const mysql = ["mysql", "mariadb"].includes(this.host.connection()?.kind ?? "");
    const table = document.createElement("table"); table.className = "designer-grid designer-fields wide mini-table";
    const defs = this.fieldColumnDefs();
    const colgroup = document.createElement("colgroup");
    for (const def of defs) { const col = document.createElement("col"); if (def.width !== "auto") col.style.width = def.width; colgroup.appendChild(col); }
    table.appendChild(colgroup);
    const head = document.createElement("thead"); const row = document.createElement("tr");
    for (const def of defs) { const th = document.createElement("th"); th.scope = "col"; th.textContent = def.title; if (def.align) th.style.textAlign = def.align; row.appendChild(th); } head.appendChild(row); table.appendChild(head);
    const rows = document.createElement("tbody");
    const dialectTypes = DIALECT_TYPES[this.host.connection()?.kind ?? ""] ?? GENERIC_TYPES;
    const tableTypes = [...new Set(this.snapshot?.design.columns.map(c => c.data_type).filter(t => t && !dialectTypes.includes(t)) ?? [])];
    const typeChoices = [...dialectTypes, ...tableTypes];
    for (const column of draft.columns) {
      const tr = document.createElement("tr"); tr.dataset.fieldKey = column.key; tr.className = (column.key === this.selected ? "selected " : "") + (column.deleted ? "designer-deleted" : "");
      tr.addEventListener("focusin", () => { this.selected = column.key; for (const other of rows.querySelectorAll("tr")) other.classList.toggle("selected", other === tr); this.host.changed(); });
      const cell = (el: HTMLElement) => { const td = document.createElement("td"); td.appendChild(el); tr.appendChild(td); };
      const label = () => "字段 " + (column.name || "未命名");
      cell(this.input(column.name, label() + " 名称", value => { const at = draft.primary_key.indexOf(column.name); if (at >= 0) draft.primary_key[at] = value; column.name = value; }, column.deleted));
      cell(this.select(column.data_type, typeChoices, label() + " 类型", value => { column.data_type = value; this.render(); }, column.deleted));
      cell(this.input(column.length, label() + " 长度", value => { column.length = value || null; }, column.deleted));
      cell(this.input(column.scale, label() + " 小数位", value => { column.scale = value || null; }, column.deleted));
      cell(this.defaultCell(column, label()));
      // 主键列每行都有钥匙：点一下加入主键，再点一下移出（不再只对已设主键的字段显示）
      const inPrimary = draft.primary_key.includes(column.name);
      cell(this.keyToggle(inPrimary, label() + " 主键", () => {
        const at = draft.primary_key.indexOf(column.name);
        if (at >= 0) draft.primary_key.splice(at, 1); else draft.primary_key.push(column.name);
        this.changed(); this.render();
      }, column.deleted));
      // 允许 NULL 紧跟在主键之后；自增 / 无符号 / 填充零收在扩展列里（列序：… 主键 / 允许 NULL / 扩展 / 注释）
      cell(this.checkBare(column.nullable, label() + " 允许 NULL", value => { column.nullable = value; }, column.deleted || (mysql && !!column.auto_increment)));
      if (mysql) cell(this.extensionCell(column, label()));
      const comment = this.input(column.comment, label() + " 注释", value => { column.comment = value; }, column.deleted || this.host.connection()?.kind === "sqlite"); if (this.host.connection()?.kind === "sqlite") comment.title = "SQLite 不支持列注释 DDL。"; cell(comment);
      rows.appendChild(tr);
    }
    table.appendChild(rows); body.appendChild(table);
  }
  private indexMethodChoices(): string[] {
    const kind = this.host.connection()?.kind ?? "";
    if (kind === "mysql" || kind === "mariadb" || kind === "postgresql") return ["BTREE", "HASH"];
    return ["BTREE"];
  }
  /** 索引/主键字段编辑器（浮层内）：每行「字段下拉 + 排序下拉 + 删除」，底部 ↑ ↓ 调整顺序、＋ 添加字段。 */
  private indexColumnsEditor(columns: string[], descending: string[], columnNames: string[], label: string, onChange: (columns: string[], descending: string[]) => void): HTMLElement {
    const box = document.createElement("div"); box.className = "designer-index-cols";
    let cursor = 0;
    const render = () => {
      box.innerHTML = "";
      const head = document.createElement("div"); head.className = "designer-index-head";
      for (const title of ["字段", "排序", ""]) { const span = document.createElement("span"); span.textContent = title; head.appendChild(span); }
      box.appendChild(head);
      const list = document.createElement("div"); list.className = "designer-index-list";
      columns.forEach((name, i) => {
        const rowEl = document.createElement("div"); rowEl.className = "designer-index-col-row";
        if (i === cursor) rowEl.classList.add("current");
        const pos = label + " 字段 " + (i + 1);
        const colSel = this.select(name, columnNames, pos, value => {
          const old = columns[i]; columns[i] = value;
          const di = descending.indexOf(old); if (di >= 0) { descending = [...descending]; descending[di] = value; }
          onChange(columns, descending); render();
        });
        colSel.classList.add("designer-index-field");
        colSel.addEventListener("focus", () => { cursor = i; });
        const dirSel = this.select(descending.includes(name) ? "DESC" : "ASC", ["ASC", "DESC"], pos + " 排序", value => {
          const set = new Set(descending); if (value === "DESC") set.add(name); else set.delete(name); descending = [...set];
          onChange(columns, descending); render();
        });
        dirSel.classList.add("designer-index-order");
        const del = this.button("×", () => { const removed = columns.splice(i, 1)[0]; descending = descending.filter(n => n !== removed); cursor = Math.max(0, i - 1); onChange(columns, descending); render(); }, this.editReason());
        del.className = "designer-index-remove"; del.setAttribute("aria-label", "删除" + pos);
        rowEl.append(colSel, dirSel, del); list.appendChild(rowEl);
      });
      box.appendChild(list);
      const tools = document.createElement("div"); tools.className = "designer-index-tools";
      const up = this.button("↑", () => { const at = Math.min(cursor, columns.length - 1); if (at > 0) { [columns[at - 1], columns[at]] = [columns[at], columns[at - 1]]; cursor = at - 1; onChange(columns, descending); render(); } }, this.editReason());
      up.className = "designer-index-tool"; up.setAttribute("aria-label", "上移" + label + "字段");
      const down = this.button("↓", () => { const at = Math.min(cursor, columns.length - 1); if (at < columns.length - 1) { [columns[at + 1], columns[at]] = [columns[at], columns[at + 1]]; cursor = at + 1; onChange(columns, descending); render(); } }, this.editReason());
      down.className = "designer-index-tool"; down.setAttribute("aria-label", "下移" + label + "字段");
      const add = this.button("＋字段", () => { columns.push(columnNames.find(n => !columns.includes(n)) ?? ""); cursor = columns.length - 1; onChange(columns, descending); render(); }, this.editReason());
      add.className = "designer-index-tool designer-add-col"; add.setAttribute("aria-label", "为" + label + "添加字段");
      tools.append(up, down, add);
      box.appendChild(tools);
    };
    render();
    return box;
  }
  /** 单元格内的索引字段摘要 + 右侧「…」按钮：点任意位置都打开字段选择浮层。 */
  private indexColumnsCell(columns: string[], descending: string[], columnNames: string[], label: string, apply: (columns: string[], descending: string[]) => void): HTMLElement {
    const summary = columns.length ? columns.map(name => `${name} ${descending.includes(name) ? "DESC" : "ASC"}`).join("，") : "选择字段…";
    const wrap = document.createElement("div"); wrap.className = "designer-picker";
    const text = document.createElement("span"); text.className = "designer-picker-text"; text.textContent = summary;
    if (!columns.length) text.classList.add("empty");
    const more = document.createElement("button"); more.type = "button"; more.className = "designer-picker-more";
    more.setAttribute("aria-label", label + " 字段选择"); more.title = "编辑索引字段与排序";
    more.innerHTML = '<svg viewBox="0 0 14 14" width="14" height="14" fill="currentColor" aria-hidden="true"><circle cx="3" cy="7" r="1.25"/><circle cx="7" cy="7" r="1.25"/><circle cx="11" cy="7" r="1.25"/></svg>';
    more.disabled = !!this.editReason(); if (more.disabled) more.title = this.editReason() ?? "";
    more.classList.toggle("open", this.pickerOpenedFrom === more);
    more.addEventListener("click", event => { event.stopPropagation(); this.openIndexColumnsPopover(more, columns, descending, columnNames, label, apply); });
    wrap.addEventListener("click", () => { if (!more.disabled) this.openIndexColumnsPopover(more, columns, descending, columnNames, label, apply); });
    wrap.append(text, more);
    return wrap;
  }
  /** 索引字段浮层：锚定「…」按钮，字段与排序都是下拉，底部可增删与上下移动。 */
  private pickerOpenedFrom: HTMLButtonElement | null = null;
  private openIndexColumnsPopover(anchor: HTMLButtonElement, columns: string[], descending: string[], columnNames: string[], label: string, apply: (columns: string[], descending: string[]) => void): void {
    const reason = this.editReason(); if (reason) { this.host.toast("info", reason); return; }
    if (isPopoverOpenFor(anchor)) { closePopover(); return; }
    let nextColumns = [...columns];
    let nextDescending = [...descending];
    this.pickerOpenedFrom = anchor;
    anchor.classList.add("open");
    openPopover({
      anchor, className: "index-cols", width: 336, align: "end", ariaLabel: label + " 字段选择",
      onClose: () => { anchor.classList.remove("open"); if (this.pickerOpenedFrom === anchor) this.pickerOpenedFrom = null; },
      render: (body, close) => {
        const list = document.createElement("div"); list.className = "popover-index-cols";
        list.appendChild(this.indexColumnsEditor(nextColumns, nextDescending, columnNames, label, (cols, desc) => { nextColumns = cols; nextDescending = desc; }));
        body.appendChild(list);
        const foot = document.createElement("div"); foot.className = "popover-footer";
        const cancel = document.createElement("button"); cancel.type = "button"; cancel.className = "btn"; cancel.textContent = "取消";
        cancel.addEventListener("click", close);
        const confirm = document.createElement("button"); confirm.type = "button"; confirm.className = "btn primary"; confirm.textContent = "确定";
        confirm.addEventListener("click", () => { apply([...nextColumns], [...nextDescending]); this.changed(); close(); this.render(); });
        foot.append(cancel, confirm); body.appendChild(foot);
      },
    });
  }
  /** 行内多字段单元格：逗号分隔输入，书写顺序即列顺序；datalist 提供真实列名候选。 */
  private columnsInput(value: string[], label: string, set: (next: string[]) => void, datalistId?: string, disabled = false): HTMLInputElement {
    const el = document.createElement("input"); el.type = "text"; el.value = value.join(", "); el.setAttribute("aria-label", label); el.disabled = disabled || !!this.editReason();
    if (datalistId) el.setAttribute("list", datalistId);
    el.setAttribute("data-search-token", ",");
    el.addEventListener("input", () => { set(el.value.split(",").map(name => name.trim()).filter(Boolean)); this.changed(); });
    return el;
  }
  /** 真实列名的共享 datalist。 */
  private columnsDatalist(body: HTMLElement): string {
    const list = document.createElement("datalist"); list.id = `${this.id}-cols`;
    for (const column of this.draft!.columns.filter(c => !c.deleted)) { const option = document.createElement("option"); option.value = column.name; list.appendChild(option); }
    body.appendChild(list); return list.id;
  }
  private unknown(body: HTMLElement, label: string): void { const p = document.createElement("p"); p.className = "designer-notice"; p.textContent = `${label}元数据未提供可编辑全集；保持 undefined 原样保留，不能当作空集合提交。`; body.appendChild(p); }
  /** 索引页：主键就是列表里的第一行，与普通索引同一套单元格、同一种编辑方式。
   *  主键行可改字段与顺序、可把类型改成 NORMAL/UNIQUE（降级为普通索引）；
   *  普通索引行把类型改成 PRIMARY 即升级为主键。名称与方法的可改范围由数据库决定，UI 不再额外禁用。
   *  行内不再放删除按钮：点行选中，删除统一走顶部操作栏的「删除索引」。 */
  private renderIndexes(body: HTMLElement): void {
    const draft = this.draft!; const indexes = draft.indexes;
    if (indexes === undefined) { this.unknown(body, "索引"); return; }
    const columnNames = draft.columns.filter(c => !c.deleted).map(c => c.name);
    const methods = this.indexMethodChoices();
    const kinds = ["NORMAL", "UNIQUE", "PRIMARY"];
    // 选中态兜底：草稿变化（新增 / 删除 / 重载）后总有一个确定的删除目标。
    if (this.selectedIndex !== PRIMARY_ROW && !indexes.includes(this.selectedIndex as IndexDesign)) this.selectedIndex = indexes[0] ?? PRIMARY_ROW;
    const table = document.createElement("table"); table.className = "designer-grid index-grid mini-table"; table.style.minWidth = "0"; table.style.width = "100%";
    const colgroup = document.createElement("colgroup");
    for (const width of ["24%", "auto", "18%", "16%"]) { const col = document.createElement("col"); col.style.width = width; colgroup.appendChild(col); }
    table.appendChild(colgroup);
    const head = document.createElement("thead"); const hr = document.createElement("tr");
    for (const title of ["名", "字段（可多选，可排序）", "索引类型", "索引方法"]) { const th = document.createElement("th"); th.scope = "col"; th.textContent = title; hr.appendChild(th); }
    head.appendChild(hr); table.appendChild(head);
    const tbody = document.createElement("tbody");
    const selectRow = (row: HTMLElement, entry: unknown) => {
      this.selectedIndex = entry;
      for (const other of tbody.querySelectorAll("tr")) other.classList.toggle("selected", other === row);
      this.host.changed();
    };

    // 主键行
    const pkRow = document.createElement("tr"); pkRow.dataset.indexRow = "primary"; pkRow.classList.toggle("selected", this.selectedIndex === PRIMARY_ROW);
    pkRow.addEventListener("focusin", () => selectRow(pkRow, PRIMARY_ROW));
    pkRow.addEventListener("click", () => selectRow(pkRow, PRIMARY_ROW));
    const pkNameCell = document.createElement("td");
    const pkName = document.createElement("input"); pkName.type = "text"; pkName.value = "PRIMARY"; pkName.readOnly = true; pkName.disabled = !!this.editReason();
    pkName.setAttribute("aria-label", "主键 名称"); pkName.title = "主键名由数据库固定为 PRIMARY；要改的是字段、顺序与类型。";
    pkNameCell.appendChild(pkName); pkRow.appendChild(pkNameCell);
    const pkColsCell = document.createElement("td");
    pkColsCell.appendChild(this.indexColumnsCell(draft.primary_key, [], columnNames, "主键", cols => { draft.primary_key = cols; }));
    pkRow.appendChild(pkColsCell);
    const pkKindCell = document.createElement("td");
    pkKindCell.appendChild(this.select("PRIMARY", kinds, "主键 索引类型", value => this.changeIndexKind({ primary: true }, value), false));
    pkRow.appendChild(pkKindCell);
    const pkMethodCell = document.createElement("td");
    const pkMethod = this.select("BTREE", ["BTREE"], "主键 索引方法", () => {}, false);
    pkMethod.title = "主键索引方法由数据库按方言决定。"; pkMethodCell.appendChild(pkMethod); pkRow.appendChild(pkMethodCell);
    tbody.appendChild(pkRow);

    // 普通索引行
    indexes.forEach((index, i) => {
      const rowLabel = "索引 " + (i + 1);
      const tr = document.createElement("tr"); tr.classList.toggle("selected", this.selectedIndex === index);
      tr.addEventListener("focusin", () => selectRow(tr, index));
      tr.addEventListener("click", () => selectRow(tr, index));
      const nameCell = document.createElement("td");
      nameCell.appendChild(this.input(index.name, rowLabel + " 名称", value => { index.name = value; }));
      tr.appendChild(nameCell);
      const colsCell = document.createElement("td");
      colsCell.appendChild(this.indexColumnsCell(index.columns, index.descending ?? [], columnNames, rowLabel, (cols, desc) => { index.columns = cols; index.descending = desc; }));
      tr.appendChild(colsCell);
      const kindCell = document.createElement("td");
      kindCell.appendChild(this.select(index.unique ? "UNIQUE" : "NORMAL", kinds, rowLabel + " 索引类型", value => this.changeIndexKind({ primary: false, index }, value)));
      tr.appendChild(kindCell);
      const methodCell = document.createElement("td");
      methodCell.appendChild(this.select(index.method ?? "BTREE", methods, rowLabel + " 索引方法", value => { index.method = value; }));
      tr.appendChild(methodCell);
      tbody.appendChild(tr);
    });
    table.appendChild(tbody); body.appendChild(table);
  }
  /** 索引类型切换：PRIMARY ↔ NORMAL/UNIQUE 是同一个数据模型的两端，切换即升降级，不做二次确认。 */
  private changeIndexKind(entry: { primary: true } | { primary: false; index: IndexDesign }, value: string): void {
    const draft = this.draft!; const indexes = draft.indexes;
    if (!indexes) return;
    if (entry.primary) {
      if (value === "PRIMARY") return;
      const columns = [...draft.primary_key];
      if (!columns.length) { this.host.toast("info", "主键还没有字段，无法改成普通索引。"); this.render(); return; }
      draft.primary_key = [];
      indexes.push({ name: this.uniqueIndexName(`idx_${columns[0]}`), columns, unique: value === "UNIQUE", method: "BTREE", descending: [] });
    } else if (value === "PRIMARY") {
      const index = entry.index;
      if (!index.columns.length) { this.host.toast("info", `索引「${index.name || "未命名"}」还没有字段，不能升级为主键。`); this.render(); return; }
      draft.primary_key = [...index.columns];
      const at = indexes.indexOf(index as never);
      if (at >= 0) indexes.splice(at, 1);
    } else {
      entry.index.unique = value === "UNIQUE";
    }
    this.changed(); this.render();
  }
  private uniqueIndexName(base: string): string {
    const taken = new Set((this.draft?.indexes ?? []).map(index => index.name));
    if (!taken.has(base)) return base;
    let n = 2; while (taken.has(`${base}_${n}`)) n++;
    return `${base}_${n}`;
  }
  /** 读取「选项」页候选（引擎/字符集/排序规则/行格式）；非 MySQL 后端返回空候选。
   *  该页签暂时收起，候选读取保留给恢复后的页面。 */
  private async loadOptionChoices(): Promise<void> {
    if (this.optionChoices.loading) return;
    const version = this.metadataVersion; this.optionChoices = { values: null, error: "", loading: true };
    try { const values = await api.tableOptionChoices(this.tab.connectionId); if (version !== this.metadataVersion || this.disposed) return; this.optionChoices = { values, error: "", loading: false }; }
    catch (error) { if (version !== this.metadataVersion || this.disposed) return; this.optionChoices = { values: null, error: `表选项候选项读取失败：${String(error)}`, loading: false }; }
    if (this.currentTab === "选项") this.render();
  }
  /** 外键页：直接渲染草稿里已有的外键配置，不为了显示去读取引用元数据。
   *  引用库 / 引用表 / 引用字段都是可直接编辑的文本（顺序即对应关系）；没有外键时整页保持空白提示。 */
  private renderForeignKeys(body: HTMLElement): void {
    const fks = this.draft!.foreign_keys; if (fks === undefined) { this.unknown(body, "外键"); return; }
    if (!fks.length) { const empty = document.createElement("p"); empty.className = "designer-empty"; empty.textContent = "当前表没有外键约束。如需创建，请使用顶部操作栏的「添加外键」。"; body.appendChild(empty); return; }
    const colsId = this.columnsDatalist(body);
    const fkTable = document.createElement("table"); fkTable.className = "designer-grid mini-table fk-grid"; fkTable.style.minWidth = "0"; fkTable.style.width = "100%";
    const fkHead = document.createElement("thead"); const fkHr = document.createElement("tr");
    for (const title of ["名", "本表字段（逗号分隔）", "引用库", "引用表（父）", "引用字段（按顺序对应）", "删除时", "更新时"]) { const th = document.createElement("th"); th.scope = "col"; th.textContent = title; fkHr.appendChild(th); }
    fkHead.appendChild(fkHr); fkTable.appendChild(fkHead);
    const fkTbody = document.createElement("tbody");
    fks.forEach((fk, i) => {
      const rowLabel = `外键 ${i + 1}`;
      const tr = document.createElement("tr"); tr.classList.toggle("selected", fk === this.selectedForeignKey);
      tr.addEventListener("focusin", () => { this.selectedForeignKey = fk; for (const other of fkTbody.querySelectorAll("tr")) other.classList.toggle("selected", other === tr); this.host.changed(); });
      const td = (el: HTMLElement) => { const cell = document.createElement("td"); cell.appendChild(el); tr.appendChild(cell); };
      td(this.input(fk.name, `${rowLabel} 名称`, value => { fk.name = value; }));
      td(this.columnsInput(fk.columns, `${rowLabel} 本表字段`, next => { fk.columns = next; }, colsId));
      const namespace = this.input(fk.ref_schema ?? this.snapshot!.design.schema, `${rowLabel} 引用库`, value => { fk.ref_schema = value; });
      namespace.placeholder = this.snapshot!.design.schema;
      td(namespace);
      const target = this.input(fk.ref_table, `${rowLabel} 引用表`, value => { fk.ref_table = value; });
      target.placeholder = "被引用的表名";
      td(target);
      const columns = this.columnsInput(fk.ref_columns, `${rowLabel} 引用字段`, next => { fk.ref_columns = next; });
      columns.placeholder = "与左侧字段逐个对应的列名";
      td(columns);
      td(this.select(fk.on_delete ?? "", ["", "NO ACTION", "RESTRICT", "CASCADE", "SET NULL", "SET DEFAULT"], `${rowLabel} 删除时`, value => { fk.on_delete = value || null; }));
      td(this.select(fk.on_update ?? "", ["", "NO ACTION", "RESTRICT", "CASCADE", "SET NULL", "SET DEFAULT"], `${rowLabel} 更新时`, value => { fk.on_update = value || null; }));
      fkTbody.appendChild(tr);
    });
    fkTable.appendChild(fkTbody); body.appendChild(fkTable);
  }
  private renderChecks(body: HTMLElement): void {
    const checks = this.draft!.checks; if (checks === undefined) { this.unknown(body, "检查约束"); return; }
    const capability = this.snapshot!.capabilities.checks; if (!capability.supported) { const note = document.createElement("p"); note.textContent = capability.reason || "后端不支持修改检查约束。"; body.appendChild(note); }
    if (!checks.some(item => (item as unknown) === this.selectedCheck)) this.selectedCheck = checks[0] ?? null;
    const table = document.createElement("table"); table.className = "designer-grid mini-table"; table.style.minWidth = "0"; table.style.width = "100%";
    const head = document.createElement("thead"); const hr = document.createElement("tr");
    for (const title of ["名", "CHECK 表达式"]) { const th = document.createElement("th"); th.scope = "col"; th.textContent = title; hr.appendChild(th); }
    head.appendChild(hr); table.appendChild(head);
    const tbody = document.createElement("tbody");
    checks.forEach((check, i) => {
      const rowLabel = `检查约束 ${i + 1}`;
      const tr = document.createElement("tr"); tr.classList.toggle("selected", check === this.selectedCheck);
      tr.addEventListener("focusin", () => { this.selectedCheck = check; for (const other of tbody.querySelectorAll("tr")) other.classList.toggle("selected", other === tr); this.host.changed(); });
      const td = (el: HTMLElement) => { const cell = document.createElement("td"); cell.appendChild(el); tr.appendChild(cell); };
      const name = this.input(check.name, `${rowLabel} 名称`, value => { check.name = value; }, !capability.supported || check.generated_name);
      if (check.generated_name) name.title = "此名称是匿名 CHECK 的展示身份；后端决定真实 DDL 可支持范围。";
      td(name);
      td(this.input(check.expression, `${rowLabel} CHECK 表达式`, value => { check.expression = value; }, !capability.supported));
      tbody.appendChild(tr);
    });
    table.appendChild(tbody); body.appendChild(table);
  }
  private renderTriggers(body: HTMLElement): void {
    const triggers = this.draft!.triggers; if (triggers === undefined) { this.unknown(body, "触发器"); return; }
    const capability = this.snapshot!.capabilities.triggers; if (!capability.supported) { const note = document.createElement("p"); note.textContent = capability.reason || "后端不支持修改触发器。"; body.appendChild(note); }
    if (!triggers.some(item => (item as unknown) === this.selectedTrigger)) this.selectedTrigger = triggers[0] ?? null;
    const table = document.createElement("table"); table.className = "designer-grid mini-table"; table.style.minWidth = "0"; table.style.width = "100%";
    const head = document.createElement("thead"); const hr = document.createElement("tr");
    for (const title of ["名", "时机", "事件", "触发语句 / 函数调用"]) { const th = document.createElement("th"); th.scope = "col"; th.textContent = title; hr.appendChild(th); }
    head.appendChild(hr); table.appendChild(head);
    const tbody = document.createElement("tbody");
    triggers.forEach((trigger, i) => {
      const rowLabel = `触发器 ${i + 1}`;
      const tr = document.createElement("tr"); tr.classList.toggle("selected", trigger === this.selectedTrigger);
      tr.addEventListener("focusin", () => { this.selectedTrigger = trigger; for (const other of tbody.querySelectorAll("tr")) other.classList.toggle("selected", other === tr); this.host.changed(); });
      const td = (el: HTMLElement) => { const cell = document.createElement("td"); cell.appendChild(el); tr.appendChild(cell); };
      td(this.input(trigger.name, `${rowLabel} 名称`, value => { trigger.name = value; }, !capability.supported));
      td(this.select(trigger.timing, ["BEFORE", "AFTER", "INSTEAD OF"], `${rowLabel} 时机`, value => { trigger.timing = value; }, !capability.supported));
      td(this.select(trigger.event, ["INSERT", "UPDATE", "DELETE"], `${rowLabel} 事件`, value => { trigger.event = value; }, !capability.supported));
      const statement = this.input(trigger.statement, `${rowLabel} 触发语句`, value => { trigger.statement = value; }, !capability.supported);
      if (trigger.definition) statement.title = `原始完整定义（只读）：\n${trigger.definition}`;
      td(statement);
      tbody.appendChild(tr);
    });
    table.appendChild(tbody); body.appendChild(table);
  }
  private renderOptions(body: HTMLElement): void {
    body.classList.add("designer-options");
    const draft = this.draft!; const capability = this.snapshot!.capabilities; const options = draft.options;
    if (options === undefined) { this.unknown(body, "表选项"); return; }
    const labels: [keyof TableOptionsDesign, string][] = [["engine", "存储引擎"], ["charset", "字符集"], ["collation", "排序规则"], ["row_format", "行格式"], ["strict", "STRICT"], ["without_rowid", "WITHOUT ROWID"], ["fillfactor", "填充因子"]];
    // engine/charset/collation/row_format 用可搜索下拉：候选来自当前数据库的真实信息模式。
    const choiceKeys: (keyof TableOptionsDesign)[] = ["engine", "charset", "collation", "row_format"];
    if (labels.some(([key]) => choiceKeys.includes(key) && capability.options.supported && capability.supported_options.includes(key)) && !this.optionChoices.values && !this.optionChoices.loading && !this.optionChoices.error) void this.loadOptionChoices();
    const choices = this.optionChoices.values;
    const choiceMap: Record<string, string[]> = { engine: choices?.engines ?? [], charset: choices?.charsets ?? [], collation: choices?.collations ?? [], row_format: choices?.row_formats ?? [] };
    for (const [key, label] of labels) {
      const supported = capability.options.supported && capability.supported_options.includes(key);
      let reason = supported ? undefined : this.host.connection()?.kind === "sqlite" && ["engine", "charset", "collation"].includes(key) ? "SQLite 没有此表级选项。" : capability.options.reason || `后端未声明支持 ${key}。`;
      let control: HTMLElement;
      if (key === "strict" || key === "without_rowid") control = this.select(options[key] == null ? "未设置" : options[key] ? "是" : "否", ["未设置", "是", "否"], label, value => { options[key] = value === "未设置" ? null : value === "是"; }, !supported);
      else if (key === "fillfactor") { const input = this.input(options[key] == null ? "" : String(options[key]), label, value => { options[key] = value === "" ? null : /^\d+$/.test(value) ? Number(value) : value; }, !supported); input.inputMode = "numeric"; input.setAttribute("aria-description", "10 到 100 之间的整数；未完成的输入会保留并在保存前报错。"); control = input; }
      else if (choiceKeys.includes(key)) {
        control = this.select((options[key] as string | null) ?? "", choiceMap[key as string] ?? [], label, value => { options[key] = value || null; }, !supported || !choices);
        if (supported && !choices) reason = this.optionChoices.error || "正在读取该数据库的真实候选项…";
      }
      else control = this.input(options[key], label, value => { options[key] = value || null; }, !supported);
      body.appendChild(this.labeled(label, control, reason));
    }
    for (const limitation of capability.limitations) { const note = document.createElement("p"); note.textContent = limitation; body.appendChild(note); }
  }
  /** SQL 预览页：只显示 SQL 本体——没有生成按钮、没有状态条、没有附加说明。
   *  切到本页或继续编辑都会自动重算；高亮沿用 DDL 同一套着色。 */
  private renderPreview(body: HTMLElement): void {
    const synced = !!this.preview && this.previewSignature === draftSignature(this.draft!);
    const code = document.createElement("pre"); code.className = "designer-sql"; code.tabIndex = 0;
    code.setAttribute("aria-label", "后端实际 SQL 计划");
    if (this.previewState === "error") { code.textContent = this.previewError; code.classList.add("error"); }
    else if (synced && this.preview) code.innerHTML = highlightSql(this.previewSql());
    else if (!this.draftDirty()) code.textContent = "-- 没有待执行的结构变更";
    else code.textContent = "-- 正在生成…";
    body.appendChild(code);
  }
  private previewSql(): string {
    if (!this.preview) return "";
    return this.preview.steps.map(step => step.sql).join("\n\n");
  }
  /** 编辑后（或切到预览页）自动重算；防抖把连续输入合并成一次后端调用。 */
  private queuePreview(delay = 260): void {
    if (this.previewTimer) { clearTimeout(this.previewTimer); this.previewTimer = null; }
    if (this.currentTab !== "SQL 预览") return;
    this.previewTimer = setTimeout(() => { this.previewTimer = null; void this.refreshPreview(); }, delay);
  }
  private async refreshPreview(): Promise<void> {
    if (this.disposed || this.currentTab !== "SQL 预览" || this.saving || this.loading || this.locked) return;
    if (!this.draft || !this.draftDirty()) {
      this.preview = null; this.previewSignature = ""; this.previewState = "idle"; this.previewError = "";
      this.render(); return;
    }
    const token = ++this.previewToken;
    const signature = draftSignature(this.draft);
    this.previewState = "loading"; this.previewError = ""; this.render();
    try {
      const design = this.payload();
      const preview = await api.previewTableDesign(this.tab.connectionId, design, this.tab.id);
      if (this.disposed || token !== this.previewToken || this.currentTab !== "SQL 预览") return;
      if (!this.draft || draftSignature(this.draft) !== signature) { this.queuePreview(120); return; }
      this.preview = preview; this.previewSignature = signature;
      this.previewState = "ok";
      this.render();
    } catch (error) {
      if (this.disposed || token !== this.previewToken) return;
      this.previewState = "error"; this.previewError = String(error);
      if (this.currentTab === "SQL 预览") this.render();
    }
  }
  destroy(): void {
    this.disposed = true;
    if (this.previewTimer) { clearTimeout(this.previewTimer); this.previewTimer = null; }
    closePopover();
    this.execution?.dispose();
  }
}
