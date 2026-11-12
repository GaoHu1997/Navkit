import { api, type TableDesignSnapshot, type TableDesignPayload, type TableDesignPreview, type TableDesignResponse, type ForeignKeyDesign } from "../core/api";
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
const TABS = ["字段", "索引", "外键", "检查", "触发器", "选项", "注释", "SQL 预览"];
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
  private readonly references = new Map<string, { columns: string[] | null; error: string; loading: boolean }>();
  private namespaces: { values: string[] | null; error: string; loading: boolean } = { values: null, error: "", loading: false };
  private readonly referenceTables = new Map<string, { values: string[] | null; error: string; loading: boolean }>();
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
    this.metadataVersion++; this.namespaces = { values: null, error: "", loading: false }; this.referenceTables.clear(); this.references.clear();
    this.snapshot = designClone(snapshot); this.draft = draft; this.baselineDraft = designClone(draft); this.baseline = draftSignature(draft);
    this.tab.schemaInfo = designClone(snapshot.schema); this.tab.dirty = false;
    this.preview = null; this.previewSignature = "";
    if (!draft.columns.some(c => c.key === this.selected)) this.selected = draft.columns[0]?.key ?? "";
  }
  private changed(): void {
    this.tab.dirty = this.draftDirty(); this.preview = null; this.previewSignature = "";
    const status = this.el.querySelector<HTMLElement>(".designer-status");
    if (status) status.textContent = this.tab.dirty ? "有未保存的表结构草稿。" : "表结构与读取基线一致。";
    this.host.changed();
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
  deleteIndex(): void { const list = this.draft?.indexes; if (!list) return; const item = list.find(x => (x as unknown) === this.selectedIndex); if (!item) { this.host.toast("info", "请先选择索引。"); return; } this.mutate(() => { const at = list.indexOf(item); if (at >= 0) list.splice(at, 1); this.selectedIndex = list[0] ?? null; }); }
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
    for (const fk of this.draft.foreign_keys ?? []) {
      const originalFk = this.snapshot.design.foreign_keys?.find(old => old.name === fk.name);
      if (JSON.stringify(originalFk) === JSON.stringify(fk)) continue;
      const namespace = fk.ref_schema ?? this.snapshot.design.schema;
      if (!this.namespaces.values?.includes(namespace)) throw new Error(`外键「${fk.name}」引用命名空间尚未由真实元数据证实，请重新读取并选择。`);
      if (!this.referenceTables.get(namespace)?.values?.includes(fk.ref_table)) throw new Error(`外键「${fk.name}」引用表尚未由真实元数据证实，请重新读取并选择。`);
      const columns = this.references.get(this.referenceKey(fk))?.columns;
      if (!columns || fk.ref_columns.some(column => !columns.includes(column))) throw new Error(`外键「${fk.name}」引用字段未知或已失效，请重新读取并选择。`);
    }
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
  private check(value: boolean, label: string, set: (value: boolean) => void, disabled = false): HTMLLabelElement {
    const wrapper = document.createElement("label"); wrapper.className = "designer-check";
    const el = document.createElement("input"); el.type = "checkbox"; el.checked = value; el.setAttribute("aria-label", label); el.disabled = disabled || !!this.editReason();
    el.addEventListener("change", () => { set(el.checked); this.changed(); }); wrapper.appendChild(el); const text = document.createElement("span"); text.textContent = label; wrapper.appendChild(text); return wrapper;
  }
  /** 只有复选框、没有可见文字（表头已说明语义），用 aria-label 保留可访问名。 */
  private checkBare(value: boolean, ariaLabel: string, set: (value: boolean) => void, disabled = false): HTMLLabelElement {
    const wrapper = document.createElement("label"); wrapper.className = "designer-check designer-check-bare";
    const el = document.createElement("input"); el.type = "checkbox"; el.checked = value; el.setAttribute("aria-label", ariaLabel); el.disabled = disabled || !!this.editReason();
    el.addEventListener("change", () => { set(el.checked); this.changed(); }); wrapper.appendChild(el); return wrapper;
  }
  /** 主键钥匙图标：点击切换是否属于主键（替代复选框），与参考图的键图标一致。 */
  private keyToggle(active: boolean, ariaLabel: string, action: () => void, disabled = false): HTMLButtonElement {
    const el = document.createElement("button"); el.type = "button"; el.className = "designer-key" + (active ? " active" : "");
    el.textContent = "🔑"; el.setAttribute("aria-label", ariaLabel); el.setAttribute("aria-pressed", String(active));
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
  private showTab(name: string): void { this.flushDraft(); this.currentTab = name; this.render(); this.host.changed(); this.el.querySelector<HTMLElement>(`#${this.id}-tab-${TABS.indexOf(name)}`)?.focus(); }
  private renderFields(body: HTMLElement): void {
    const draft = this.draft!;
    const table = document.createElement("table"); table.className = "designer-grid wide mini-table";
    const head = document.createElement("thead"); const row = document.createElement("tr");
    for (const title of ["名称", "类型", "长度", "小数位", "允许 NULL", "主键", "注释"]) { const th = document.createElement("th"); th.scope = "col"; th.textContent = title; row.appendChild(th); } head.appendChild(row); table.appendChild(head);
    const rows = document.createElement("tbody");
    const dialectTypes = DIALECT_TYPES[this.host.connection()?.kind ?? ""] ?? GENERIC_TYPES;
    const tableTypes = [...new Set(this.snapshot?.design.columns.map(c => c.data_type).filter(t => t && !dialectTypes.includes(t)) ?? [])];
    const typeChoices = [...dialectTypes, ...tableTypes];
    let propsHost: HTMLElement | null = null;
    for (const column of draft.columns) {
      const tr = document.createElement("tr"); tr.dataset.fieldKey = column.key; tr.className = (column.key === this.selected ? "selected " : "") + (column.deleted ? "designer-deleted" : "");
      tr.addEventListener("focusin", () => { this.selected = column.key; for (const other of rows.querySelectorAll("tr")) other.classList.toggle("selected", other === tr); if (propsHost) this.renderFieldProperties(propsHost); this.host.changed(); });
      const cell = (el: HTMLElement) => { const td = document.createElement("td"); td.appendChild(el); tr.appendChild(td); };
      const label = () => "字段 " + (column.name || "未命名");
      cell(this.input(column.name, label() + " 名称", value => { const at = draft.primary_key.indexOf(column.name); if (at >= 0) draft.primary_key[at] = value; column.name = value; }, column.deleted));
      cell(this.select(column.data_type, typeChoices, label() + " 类型", value => { column.data_type = value; }, column.deleted));
      cell(this.input(column.length, label() + " 长度", value => { column.length = value || null; }, column.deleted));
      cell(this.input(column.scale, label() + " 小数位", value => { column.scale = value || null; }, column.deleted));
      cell(this.checkBare(column.nullable, label() + " 允许 NULL", value => { column.nullable = value; }, column.deleted));
      cell(this.keyToggle(draft.primary_key.includes(column.name), label() + " 主键", () => { const at = draft.primary_key.indexOf(column.name); if (at >= 0) draft.primary_key.splice(at, 1); else draft.primary_key.push(column.name); this.changed(); this.render(); }, column.deleted));
      const comment = this.input(column.comment, label() + " 注释", value => { column.comment = value; }, column.deleted || this.host.connection()?.kind === "sqlite"); if (this.host.connection()?.kind === "sqlite") comment.title = "SQLite 不支持列注释 DDL。"; cell(comment);
      rows.appendChild(tr);
    }
    table.appendChild(rows); body.appendChild(table);
    propsHost = document.createElement("div"); propsHost.className = "designer-field-props"; body.appendChild(propsHost);
    this.renderFieldProperties(propsHost);
  }
  /** 字段页底部：编辑当前选中字段的默认值与 MySQL 专有属性（自增/无符号/填充零）。 */
  private renderFieldProperties(host: HTMLElement): void {
    if (this.disposed) return;
    host.innerHTML = "";
    const column = this.draft?.columns.find(c => c.key === this.selected);
    if (!column) return;
    const label = "字段 " + (column.name || "未命名");
    const title = document.createElement("div"); title.className = "designer-props-title"; title.textContent = "字段属性：" + (column.name || "未命名"); host.appendChild(title);
    const hasDefault = column.default_value != null;
    const defaultInput = this.input(column.default_value ?? "", label + " 默认值 SQL 表达式", value => { column.default_value = value; }, column.deleted);
    const mode = this.select(hasDefault ? "SQL 表达式" : "无默认值", ["无默认值", "SQL 表达式"], label + " 默认值类型", value => {
      column.default_value = value === "无默认值" ? null : defaultInput.value;
      defaultInput.style.display = value === "无默认值" ? "none" : "";
    }, column.deleted);
    defaultInput.style.display = hasDefault ? "" : "none";
    const defWrap = document.createElement("div"); defWrap.className = "designer-default"; defWrap.append(mode, defaultInput);
    host.appendChild(this.labeled("默认值", defWrap));
    const mysql = ["mysql", "mariadb"].includes(this.host.connection()?.kind ?? "");
    if (mysql) {
      const flags = document.createElement("div"); flags.className = "designer-props-flags";
      flags.append(
        this.check(!!column.auto_increment, label + " 自动递增", value => { column.auto_increment = value; }, column.deleted),
        this.check(!!column.unsigned, label + " 无符号", value => { column.unsigned = value; }, column.deleted),
        this.check(!!column.zerofill, label + " 填充零", value => { column.zerofill = value; }, column.deleted),
      );
      host.appendChild(flags);
    }
  }
  private indexMethodChoices(): string[] {
    const kind = this.host.connection()?.kind ?? "";
    if (kind === "mysql" || kind === "mariadb" || kind === "postgresql") return ["BTREE", "HASH"];
    return ["BTREE"];
  }
  /** 索引/主键字段顺序编辑器：可多字段，每字段可选升/降序，并可上移调整列顺序。 */
  private indexColumnsEditor(columns: string[], descending: string[], columnNames: string[], label: string, onChange: (columns: string[], descending: string[]) => void): HTMLElement {
    const box = document.createElement("div"); box.className = "designer-index-cols";
    const render = () => {
      box.innerHTML = "";
      columns.forEach((name, i) => {
        const rowEl = document.createElement("div"); rowEl.className = "designer-index-col-row";
        const pos = label + " 字段 " + (i + 1);
        const colSel = this.select(name, columnNames, pos, value => {
          const old = columns[i]; columns[i] = value;
          const di = descending.indexOf(old); if (di >= 0) { descending = [...descending]; descending[di] = value; }
          onChange(columns, descending); render();
        });
        const dirSel = this.select(descending.includes(name) ? "DESC" : "ASC", ["ASC", "DESC"], pos + " 排序", value => {
          const set = new Set(descending); if (value === "DESC") set.add(name); else set.delete(name); descending = [...set];
          onChange(columns, descending); render();
        });
        const up = this.button("↑", () => { if (i > 0) { const tmp = columns[i - 1]; columns[i - 1] = columns[i]; columns[i] = tmp; onChange(columns, descending); render(); } }, this.editReason());
        up.className = "btn designer-mini"; up.setAttribute("aria-label", "上移" + pos);
        const del = this.button("×", () => { const removed = columns.splice(i, 1)[0]; descending = descending.filter(n => n !== removed); onChange(columns, descending); render(); }, this.editReason());
        del.className = "btn designer-mini"; del.setAttribute("aria-label", "删除" + pos);
        rowEl.append(colSel, dirSel, up, del); box.appendChild(rowEl);
      });
      const add = this.button("＋字段", () => { columns.push(columnNames.find(n => !columns.includes(n)) ?? ""); onChange(columns, descending); render(); }, this.editReason());
      add.className = "btn designer-mini designer-add-col"; add.setAttribute("aria-label", "为" + label + "添加字段");
      box.appendChild(add);
    };
    render();
    return box;
  }
  /** 页签顶部的动作行（新增按钮等），替代旧的“列表 + 下方成堆表单”布局。 */
  private actionsRow(body: HTMLElement, ...buttons: HTMLButtonElement[]): void {
    const row = document.createElement("div"); row.className = "designer-actions"; row.append(...buttons); body.appendChild(row);
  }
  /** 行内多字段单元格：逗号分隔输入，书写顺序即列顺序；datalist 提供真实列名候选。 */
  private columnsInput(value: string[], label: string, set: (next: string[]) => void, datalistId?: string, disabled = false): HTMLInputElement {
    const el = document.createElement("input"); el.type = "text"; el.value = value.join(", "); el.setAttribute("aria-label", label); el.disabled = disabled || !!this.editReason();
    if (datalistId) el.setAttribute("list", datalistId);
    el.addEventListener("input", () => { set(el.value.split(",").map(name => name.trim()).filter(Boolean)); this.changed(); });
    return el;
  }
  /** 列表行的删除单元格。 */
  private opCell(tr: HTMLElement, label: string, action: () => void, disabled = false): void {
    const td = document.createElement("td"); td.className = "designer-op";
    const remove = this.button("删除", action, disabled ? "当前后端能力不支持修改。" : this.editReason());
    remove.setAttribute("aria-label", `删除${label}`);
    td.appendChild(remove); tr.appendChild(td);
  }
  /** 真实列名的共享 datalist。 */
  private columnsDatalist(body: HTMLElement): string {
    const list = document.createElement("datalist"); list.id = `${this.id}-cols`;
    for (const column of this.draft!.columns.filter(c => !c.deleted)) { const option = document.createElement("option"); option.value = column.name; list.appendChild(option); }
    body.appendChild(list); return list.id;
  }
  private unknown(body: HTMLElement, label: string): void { const p = document.createElement("p"); p.className = "designer-notice"; p.textContent = `${label}元数据未提供可编辑全集；保持 undefined 原样保留，不能当作空集合提交。`; body.appendChild(p); }
  private renderIndexes(body: HTMLElement): void {
    const indexes = this.draft!.indexes; if (indexes === undefined) { this.unknown(body, "索引"); return; }
    if (!indexes.some(item => (item as unknown) === this.selectedIndex)) this.selectedIndex = indexes[0] ?? null;
    this.actionsRow(body, this.button("新增索引", () => this.addIndex()));
    const columnNames = this.draft!.columns.filter(c => !c.deleted).map(c => c.name);
    const methods = this.indexMethodChoices();
    const table = document.createElement("table"); table.className = "designer-grid index-grid mini-table"; table.style.minWidth = "0"; table.style.width = "100%";
    const head = document.createElement("thead"); const hr = document.createElement("tr");
    for (const title of ["名", "字段（可多选，可排序）", "索引类型", "索引方法", ""]) { const th = document.createElement("th"); th.scope = "col"; th.textContent = title; hr.appendChild(th); }
    head.appendChild(hr); table.appendChild(head);
    const tbody = document.createElement("tbody");
    // 主键行：可编辑主键字段顺序，也可清除主键（不再禁止操作）。
    const pkRow = document.createElement("tr"); pkRow.className = "designer-pk";
    const pkName = document.createElement("td"); const strong = document.createElement("strong"); strong.textContent = "PRIMARY"; pkName.append(strong, document.createTextNode("（主键）")); pkRow.appendChild(pkName);
    const pkCols = document.createElement("td"); pkCols.appendChild(this.indexColumnsEditor(this.draft!.primary_key, [], columnNames, "主键", cols => { this.draft!.primary_key = cols; })); pkRow.appendChild(pkCols);
    const pkType = document.createElement("td"); pkType.textContent = "PRIMARY"; pkRow.appendChild(pkType);
    const pkMethod = document.createElement("td"); pkMethod.textContent = "BTREE"; pkMethod.title = "主键索引方法由后端按方言选择。"; pkRow.appendChild(pkMethod);
    const pkOp = document.createElement("td"); pkOp.className = "designer-op";
    const clearPk = this.button("清除", () => { this.draft!.primary_key = []; this.changed(); this.render(); }, this.editReason());
    clearPk.setAttribute("aria-label", "清除主键"); pkOp.appendChild(clearPk); pkRow.appendChild(pkOp);
    tbody.appendChild(pkRow);
    indexes.forEach((index, i) => {
      const rowLabel = "索引 " + (i + 1);
      const tr = document.createElement("tr"); tr.classList.toggle("selected", index === this.selectedIndex);
      tr.addEventListener("focusin", () => { this.selectedIndex = index; for (const other of tbody.querySelectorAll("tr")) other.classList.toggle("selected", other === tr); this.host.changed(); });
      const td = (el: HTMLElement) => { const cell = document.createElement("td"); cell.appendChild(el); tr.appendChild(cell); };
      td(this.input(index.name, rowLabel + " 名称", value => { index.name = value; }));
      td(this.indexColumnsEditor(index.columns, index.descending ?? [], columnNames, rowLabel, (cols, desc) => { index.columns = cols; index.descending = desc; }));
      td(this.select(index.unique ? "UNIQUE" : "NORMAL", ["NORMAL", "UNIQUE"], rowLabel + " 索引类型", value => { index.unique = value === "UNIQUE"; }));
      td(this.select(index.method ?? "BTREE", methods, rowLabel + " 索引方法", value => { index.method = value; }));
      this.opCell(tr, rowLabel, () => { const at = indexes.indexOf(index); if (at >= 0) indexes.splice(at, 1); if (this.selectedIndex === index) this.selectedIndex = indexes[0] ?? null; this.changed(); this.render(); });
      tbody.appendChild(tr);
    });
    table.appendChild(tbody); body.appendChild(table);
  }
  private restoreReferenceFocus(focus: { label: string; start: number | null; end: number | null } | null): void {
    this.render();
    if (!focus) return;
    const control = [...this.el.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>("[aria-label]")].find(el => el.getAttribute("aria-label") === focus.label);
    control?.focus(); if (control instanceof HTMLInputElement && focus.start !== null) control.setSelectionRange(focus.start, focus.end);
  }
  private referenceFocus(): { label: string; start: number | null; end: number | null } | null {
    const active = document.activeElement; if (!(active instanceof HTMLElement) || !this.el.contains(active)) return null;
    const label = active.getAttribute("aria-label"); if (!label) return null;
    return { label, start: active instanceof HTMLInputElement ? active.selectionStart : null, end: active instanceof HTMLInputElement ? active.selectionEnd : null };
  }
  private async loadNamespaces(): Promise<void> {
    if (this.namespaces.loading) return;
    const version = this.metadataVersion; this.namespaces = { values: null, error: "", loading: true };
    try { const values = await api.listDatabases(this.tab.connectionId); if (version !== this.metadataVersion || this.disposed) return; this.namespaces = { values: [...new Set(values.map(([name]) => name))], error: "", loading: false }; }
    catch (error) { if (version !== this.metadataVersion || this.disposed) return; this.namespaces = { values: null, error: `引用命名空间元数据未知：${String(error)}`, loading: false }; }
    if (this.currentTab === "外键") this.restoreReferenceFocus(this.referenceFocus());
  }
  private async loadReferenceTables(namespace: string): Promise<void> {
    if (!namespace || this.referenceTables.get(namespace)?.loading) return;
    const version = this.metadataVersion; this.referenceTables.set(namespace, { values: null, error: "", loading: true });
    try { const values = await api.listTables(this.tab.connectionId, namespace); if (version !== this.metadataVersion || this.disposed) return; this.referenceTables.set(namespace, { values: values.filter(([, kind]) => kind === "table").map(([name]) => name), error: "", loading: false }); }
    catch (error) { if (version !== this.metadataVersion || this.disposed) return; this.referenceTables.set(namespace, { values: null, error: `引用表元数据未知：${String(error)}`, loading: false }); }
    if (this.currentTab === "外键") this.restoreReferenceFocus(this.referenceFocus());
  }
  private referenceKey(fk: ForeignKeyDesign): string { return JSON.stringify([fk.ref_schema ?? this.snapshot!.design.schema, fk.ref_table]); }
  private async loadReference(fk: ForeignKeyDesign): Promise<void> {
    const key = this.referenceKey(fk), version = this.metadataVersion; if (!fk.ref_table.trim() || this.references.get(key)?.loading) return;
    this.references.set(key, { columns: null, loading: true, error: "" });
    try { const schema = await api.getTableSchema(this.tab.connectionId, fk.ref_schema ?? this.snapshot!.design.schema, fk.ref_table); if (version !== this.metadataVersion || this.disposed) return; this.references.set(key, { columns: schema.columns.map(c => c.name), loading: false, error: "" }); }
    catch (error) { if (version !== this.metadataVersion || this.disposed) return; this.references.set(key, { columns: null, loading: false, error: `引用字段元数据未知：${String(error)}` }); }
    if (!this.disposed && this.currentTab === "外键" && this.referenceKey(fk) === key) this.restoreReferenceFocus(this.referenceFocus());
  }
  private renderForeignKeys(body: HTMLElement): void {
    const fks = this.draft!.foreign_keys; if (fks === undefined) { this.unknown(body, "外键"); return; }
    const namespaces = this.namespaces;
    if (!namespaces.values && !namespaces.loading && !namespaces.error) void this.loadNamespaces();
    this.actionsRow(body,
      this.button("新增外键", () => this.addForeignKey()),
      this.button("读取引用元数据", () => { this.namespaces = { values: null, error: "", loading: false }; this.referenceTables.clear(); this.references.clear(); void this.loadNamespaces(); this.render(); }, namespaces.loading ? "正在读取" : this.editReason()));
    const colsId = this.columnsDatalist(body);
    const fkTable = document.createElement("table"); fkTable.className = "designer-grid mini-table"; fkTable.style.minWidth = "0"; fkTable.style.width = "100%";
    const fkHead = document.createElement("thead"); const fkHr = document.createElement("tr");
    for (const title of ["名", "本表字段（逗号分隔）", "引用库", "引用表（父）", "引用字段（按顺序对应）", "删除时", "更新时", ""]) { const th = document.createElement("th"); th.scope = "col"; th.textContent = title; fkHr.appendChild(th); }
    fkHead.appendChild(fkHr); fkTable.appendChild(fkHead);
    const fkTbody = document.createElement("tbody");
    const hints: string[] = [];
    fks.forEach((fk, i) => {
      const rowLabel = `外键 ${i + 1}`;
      const namespace = fk.ref_schema ?? this.snapshot!.design.schema;
      const tables = this.referenceTables.get(namespace);
      if (namespaces.values?.includes(namespace) && !tables) void this.loadReferenceTables(namespace);
      if (namespaces.values?.includes(namespace) && tables?.values?.includes(fk.ref_table) && !this.references.has(this.referenceKey(fk))) void this.loadReference(fk);
      const tr = document.createElement("tr"); tr.classList.toggle("selected", fk === this.selectedForeignKey);
      tr.addEventListener("focusin", () => { this.selectedForeignKey = fk; for (const other of fkTbody.querySelectorAll("tr")) other.classList.toggle("selected", other === tr); this.host.changed(); });
      const td = (el: HTMLElement) => { const cell = document.createElement("td"); cell.appendChild(el); tr.appendChild(cell); };
      td(this.input(fk.name, `${rowLabel} 名称`, value => { fk.name = value; }));
      td(this.columnsInput(fk.columns, `${rowLabel} 本表字段`, next => { fk.columns = next; }, colsId));
      const namespaceControl = this.select(namespace, ["", ...(namespaces.values ?? [])], `${rowLabel} 引用库`, value => { fk.ref_schema = value; this.preview = null; void this.loadReferenceTables(value); this.render(); }, !namespaces.values);
      if (namespaces.values && !namespaces.values.includes(namespace)) hints.push(`${rowLabel} 引用库「${namespace}」已失效；值已保留，请明确选择有效引用库。`);
      td(namespaceControl);
      const tableControl = this.select(fk.ref_table, ["", ...(tables?.values ?? [])], `${rowLabel} 引用表`, value => { fk.ref_table = value; if (value) void this.loadReference(fk); this.render(); }, !namespaces.values?.includes(namespace) || !tables?.values);
      if (tables?.error) hints.push(`${rowLabel}：${tables.error}`);
      else if (fk.ref_table.trim() && tables?.values && !tables.values.includes(fk.ref_table)) hints.push(`${rowLabel} 引用表「${fk.ref_table}」已失效；值已保留，请明确选择有效引用表。`);
      td(tableControl);
      const metadata = this.references.get(this.referenceKey(fk));
      if (metadata?.error) hints.push(`${rowLabel}：${metadata.error}`);
      const refList = document.createElement("datalist"); refList.id = `${this.id}-fkref-${i}`; for (const name of metadata?.columns ?? []) { const option = document.createElement("option"); option.value = name; refList.appendChild(option); } body.appendChild(refList);
      td(this.columnsInput(fk.ref_columns, `${rowLabel} 引用字段`, next => { fk.ref_columns = next; }, metadata?.columns ? refList.id : undefined));
      td(this.select(fk.on_delete ?? "", ["", "NO ACTION", "RESTRICT", "CASCADE", "SET NULL", "SET DEFAULT"], `${rowLabel} 删除时`, value => { fk.on_delete = value || null; }));
      td(this.select(fk.on_update ?? "", ["", "NO ACTION", "RESTRICT", "CASCADE", "SET NULL", "SET DEFAULT"], `${rowLabel} 更新时`, value => { fk.on_update = value || null; }));
      this.opCell(tr, rowLabel, () => { const at = fks.indexOf(fk); if (at >= 0) fks.splice(at, 1); if (this.selectedForeignKey === fk) this.selectedForeignKey = fks[0] ?? null; this.changed(); this.render(); });
      fkTbody.appendChild(tr);
    });
    fkTable.appendChild(fkTbody); body.appendChild(fkTable);
    if (namespaces.error) hints.unshift(namespaces.error);
    if (hints.length) { const notice = document.createElement("p"); notice.className = "designer-notice"; notice.textContent = hints.join("\n"); body.appendChild(notice); }
  }
  private renderChecks(body: HTMLElement): void {
    const checks = this.draft!.checks; if (checks === undefined) { this.unknown(body, "检查约束"); return; }
    const capability = this.snapshot!.capabilities.checks; if (!capability.supported) { const note = document.createElement("p"); note.textContent = capability.reason || "后端不支持修改检查约束。"; body.appendChild(note); }
    if (!checks.some(item => (item as unknown) === this.selectedCheck)) this.selectedCheck = checks[0] ?? null;
    this.actionsRow(body, this.button("新增检查约束", () => this.addCheck(), !capability.supported ? capability.reason || "不支持检查约束" : this.editReason()));
    const table = document.createElement("table"); table.className = "designer-grid mini-table"; table.style.minWidth = "0"; table.style.width = "100%";
    const head = document.createElement("thead"); const hr = document.createElement("tr");
    for (const title of ["名", "CHECK 表达式", ""]) { const th = document.createElement("th"); th.scope = "col"; th.textContent = title; hr.appendChild(th); }
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
      this.opCell(tr, rowLabel, () => { const at = checks.indexOf(check); if (at >= 0) checks.splice(at, 1); if (this.selectedCheck === check) this.selectedCheck = checks[0] ?? null; this.changed(); this.render(); }, !capability.supported);
      tbody.appendChild(tr);
    });
    table.appendChild(tbody); body.appendChild(table);
  }
  private renderTriggers(body: HTMLElement): void {
    const triggers = this.draft!.triggers; if (triggers === undefined) { this.unknown(body, "触发器"); return; }
    const capability = this.snapshot!.capabilities.triggers; if (!capability.supported) { const note = document.createElement("p"); note.textContent = capability.reason || "后端不支持修改触发器。"; body.appendChild(note); }
    if (!triggers.some(item => (item as unknown) === this.selectedTrigger)) this.selectedTrigger = triggers[0] ?? null;
    this.actionsRow(body, this.button("新增触发器", () => this.addTrigger(), !capability.supported ? capability.reason || "不支持触发器" : this.editReason()));
    const table = document.createElement("table"); table.className = "designer-grid mini-table"; table.style.minWidth = "0"; table.style.width = "100%";
    const head = document.createElement("thead"); const hr = document.createElement("tr");
    for (const title of ["名", "时机", "事件", "触发语句 / 函数调用", ""]) { const th = document.createElement("th"); th.scope = "col"; th.textContent = title; hr.appendChild(th); }
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
      this.opCell(tr, rowLabel, () => { const at = triggers.indexOf(trigger); if (at >= 0) triggers.splice(at, 1); if (this.selectedTrigger === trigger) this.selectedTrigger = triggers[0] ?? null; this.changed(); this.render(); }, !capability.supported);
      tbody.appendChild(tr);
    });
    table.appendChild(tbody); body.appendChild(table);
  }
  private renderOptions(body: HTMLElement): void {
    body.classList.add("designer-options");
    const draft = this.draft!; const capability = this.snapshot!.capabilities; const options = draft.options;
    if (options === undefined) { this.unknown(body, "表选项"); return; }
    const labels: [keyof TableOptionsDesign, string][] = [["engine", "存储引擎"], ["charset", "字符集"], ["collation", "排序规则"], ["row_format", "行格式"], ["strict", "STRICT"], ["without_rowid", "WITHOUT ROWID"], ["fillfactor", "填充因子"]];
    for (const [key, label] of labels) {
      const supported = capability.options.supported && capability.supported_options.includes(key);
      const reason = supported ? undefined : this.host.connection()?.kind === "sqlite" && ["engine", "charset", "collation"].includes(key) ? "SQLite 没有此表级选项。" : capability.options.reason || `后端未声明支持 ${key}。`;
      let control: HTMLElement;
      if (key === "strict" || key === "without_rowid") control = this.select(options[key] == null ? "未设置" : options[key] ? "是" : "否", ["未设置", "是", "否"], label, value => { options[key] = value === "未设置" ? null : value === "是"; }, !supported);
      else if (key === "fillfactor") { const input = this.input(options[key] == null ? "" : String(options[key]), label, value => { options[key] = value === "" ? null : /^\d+$/.test(value) ? Number(value) : value; }, !supported); input.inputMode = "numeric"; input.setAttribute("aria-description", "10 到 100 之间的整数；未完成的输入会保留并在保存前报错。"); control = input; }
      else control = this.input(options[key], label, value => { options[key] = value || null; }, !supported);
      body.appendChild(this.labeled(label, control, reason));
    }
    for (const limitation of capability.limitations) { const note = document.createElement("p"); note.textContent = limitation; body.appendChild(note); }
  }
  private renderPreview(body: HTMLElement): void {
    body.appendChild(this.button("生成后端实际计划", () => { void this.makePreview().then(() => this.render()).catch(error => this.fail(error)); }));
    const code = document.createElement("pre"); code.className = "designer-sql"; code.tabIndex = 0;
    code.textContent = this.preview && this.previewSignature === draftSignature(this.draft!) ? this.preview.steps.map((step, i) => `-- ${i + 1}. ${step.label}\n${step.sql}`).join("\n\n") || "后端计划没有待执行步骤。" : "尚未生成当前草稿的后端实际计划。请先补全草稿，再点击生成。"; body.appendChild(code);
    if (this.preview) for (const limitation of this.preview.limitations) { const note = document.createElement("p"); note.textContent = limitation; body.appendChild(note); }
  }
  destroy(): void { this.disposed = true; this.execution?.dispose(); }
}
