import { api, type TableDesignSnapshot, type TableDesignResponse, type ForeignKeyDesign } from "../core/api";
import type { ConnectionConfig } from "../core/types";
import { ExecutionController } from "../core/execution";
import { assertDesignSnapshot, designClone, draftFromSnapshot, draftSignature, payloadFromDraft, verifiedDesignResponse, type TableDesignerDraft, type DesignerColumn } from "../core/table-design-draft";
import { reviewErSyncPlan } from "../dialogs/ErSyncDialog";
import "../styles/er-table-editor.css";

export interface ErTableEditorOptions { connectionId: string; schema: string; table: string; ownerId: string; }
export interface ErTableEditorHost {
  connection(): ConnectionConfig | undefined;
  writeReason(): string | null;
  changed(): void;
  toast(kind: "error" | "info" | "success", text: string): void;
}

const DIALECT_TYPES: Record<string, string[]> = {
  mysql: ["tinyint", "smallint", "mediumint", "int", "bigint", "decimal", "numeric", "float", "double", "bit", "boolean", "varchar", "char", "text", "mediumtext", "longtext", "json", "enum", "set", "date", "datetime", "timestamp", "time", "year", "blob"],
  mariadb: ["tinyint", "smallint", "int", "bigint", "decimal", "numeric", "float", "double", "varchar", "char", "text", "longtext", "json", "date", "datetime", "timestamp", "time", "blob"],
  postgresql: ["smallint", "integer", "bigint", "decimal", "numeric", "real", "double precision", "serial", "bigserial", "varchar", "char", "text", "bytea", "timestamp", "timestamptz", "date", "time", "boolean", "uuid", "json", "jsonb"],
  sqlite: ["integer", "real", "text", "blob", "numeric", "boolean", "varchar", "date", "datetime"],
  sqlserver: ["tinyint", "smallint", "int", "bigint", "bit", "decimal", "numeric", "money", "float", "real", "varchar", "nvarchar", "char", "nchar", "text", "datetime2", "date", "time", "uniqueidentifier"],
  oracle: ["number", "integer", "float", "varchar2", "nvarchar2", "char", "nchar", "clob", "nclob", "blob", "date", "timestamp", "raw"],
};
const GENERIC_TYPES = ["integer", "bigint", "smallint", "decimal", "numeric", "float", "double", "real", "char", "varchar", "text", "blob", "boolean", "date", "time", "timestamp", "datetime", "json", "uuid"];

function elem<K extends keyof HTMLElementTagNameMap>(tag: K, cls?: string, text?: string): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

/** 关系图右侧的紧凑表设计器：字段 / 索引 / 外键 三个页签，按设计稿视觉，内部复用受控表设计协议。 */
export class ErTableEditor {
  readonly el: HTMLElement;
  private snapshot: TableDesignSnapshot | null = null;
  private draft: TableDesignerDraft | null = null;
  private baseline = "";
  private tab: "字段" | "索引" | "外键" = "字段";
  private loading = false;
  private saving = false;
  private disposed = false;
  private message = "";
  private execution: ExecutionController | null = null;
  private readonly fkCardinality = new Map<object, string>();
  private refTables: { values: string[] | null; loading: boolean; error: string } = { values: null, loading: false, error: "" };
  private readonly refColumns = new Map<string, { values: string[] | null; loading: boolean; error: string }>();
  private form = { local: "", refTable: "", refCol: "", cardinality: "N:1", onDelete: "RESTRICT", onUpdate: "CASCADE" };
  private fieldSeq = 0;

  constructor(private readonly opts: ErTableEditorOptions, private readonly host: ErTableEditorHost, el: HTMLElement) {
    this.el = el;
    this.el.classList.add("er-te");
    this.render();
  }

  activate(): void { if (!this.snapshot && !this.loading) void this.load(); }
  draftDirty(): boolean { return !!this.draft && draftSignature(this.draft) !== this.baseline; }
  dispose(): void { this.disposed = true; this.execution?.dispose(); }
  destroy(): void { this.dispose(); }

  private async load(): Promise<void> {
    if (this.loading || this.disposed) return;
    this.loading = true; this.render(); this.host.changed();
    try {
      const s = await api.readTableDesignSnapshot(this.opts.connectionId, this.opts.schema, this.opts.table, this.opts.ownerId);
      if (this.disposed) return;
      assertDesignSnapshot(s);
      this.accept(s);
      this.message = "";
    } catch (e) {
      this.message = "读取表设计失败：" + String(e);
    } finally {
      this.loading = false; this.render(); this.host.changed();
    }
  }

  private accept(s: TableDesignSnapshot): void {
    this.snapshot = designClone(s);
    this.draft = draftFromSnapshot(s);
    this.baseline = draftSignature(this.draft);
    this.fkCardinality.clear();
    for (const fk of this.draft.foreign_keys ?? []) this.fkCardinality.set(fk, this.deriveCardinality(fk));
    const first = this.draft.columns.find(c => !c.deleted);
    this.form.local = first?.name ?? "";
  }

  private deriveCardinality(fk: ForeignKeyDesign): string {
    const pk = new Set(this.draft?.primary_key ?? []);
    const one = fk.columns.length > 0 && fk.columns.every(c => pk.has(c));
    return one ? "1:1" : "N:1";
  }

  private editReason(): string | null {
    return this.host.writeReason() || (this.loading ? "正在读取表结构。" : this.saving ? "正在保存表结构。" : null);
  }

  private changed(): void {
    const status = this.el.querySelector<HTMLElement>(".er-te-status");
    if (status) status.textContent = this.draftDirty() ? "有未保存的表结构草稿。" : "表结构与读取基线一致。";
    this.host.changed();
  }

  private render(): void {
    if (this.disposed) return;
    this.el.replaceChildren();
    const tabs = elem("div", "er-te-tabs");
    tabs.setAttribute("role", "tablist");
    (["字段", "索引", "外键"] as const).forEach((name) => {
      const t = elem("button", "er-te-tab" + (this.tab === name ? " active" : ""), name);
      t.type = "button"; t.setAttribute("role", "tab"); t.setAttribute("aria-selected", String(this.tab === name));
      t.addEventListener("click", () => { this.tab = name; this.render(); });
      tabs.appendChild(t);
    });
    this.el.appendChild(tabs);
    const body = elem("div", "er-te-body");
    this.el.appendChild(body);
    if (!this.snapshot || !this.draft) {
      body.appendChild(elem("p", "er-te-empty", this.loading ? "正在读取完整表设计…" : "表设计元数据尚未完整读取。"));
      return;
    }
    if (this.message) body.appendChild(elem("div", "er-te-notice", this.message));
    const nameField = elem("div", "er-te-namefield");
    nameField.appendChild(elem("p", "er-te-lbl", "表名"));
    const ni = elem("input", "er-te-input");
    ni.value = this.opts.table; ni.disabled = true; ni.title = "表名修改请使用表结构编辑器";
    nameField.appendChild(ni);
    body.appendChild(nameField);
    if (this.tab === "字段") this.renderFields(body);
    else if (this.tab === "索引") this.renderIndexes(body);
    else this.renderForeignKeys(body);
    const status = elem("div", "er-te-status", this.draftDirty() ? "有未保存的表结构草稿。" : "表结构与读取基线一致。");
    this.el.appendChild(status);
  }

  private fieldInput(value: string | null | undefined, aria: string, set: (v: string) => void, disabled = false): HTMLInputElement {
    const i = elem("input", "er-te-mini");
    i.value = value ?? ""; i.setAttribute("aria-label", aria); i.disabled = disabled || !!this.editReason();
    i.addEventListener("input", () => { set(i.value); this.changed(); });
    return i;
  }

  private fieldSelect(value: string, choices: string[], aria: string, set: (v: string) => void, disabled = false): HTMLSelectElement {
    const s = elem("select", "er-te-mini");
    s.setAttribute("aria-label", aria); s.disabled = disabled || !!this.editReason();
    for (const c of [...new Set([value, ...choices])]) { const o = elem("option"); o.value = c; o.textContent = c || "（未设置）"; s.appendChild(o); }
    s.value = value; s.addEventListener("change", () => { set(s.value); this.changed(); });
    return s;
  }

  private check(value: boolean, label: string, aria: string, set: (v: boolean) => void, disabled = false): HTMLLabelElement {
    const w = elem("label", "er-te-check");
    const c = elem("input"); c.type = "checkbox"; c.checked = value; c.disabled = disabled || !!this.editReason(); c.setAttribute("aria-label", aria);
    c.addEventListener("change", () => { set(c.checked); this.changed(); });
    w.append(c, elem("span", undefined, label));
    return w;
  }

  private typeChoices(): string[] {
    const dialect = DIALECT_TYPES[this.host.connection()?.kind ?? ""] ?? GENERIC_TYPES;
    const existing = [...new Set(this.snapshot?.design.columns.map(c => c.data_type).filter(t => t && !dialect.includes(t)) ?? [])];
    return [...dialect, ...existing];
  }

  private renderFields(body: HTMLElement): void {
    const head = elem("div", "er-te-subhead");
    head.appendChild(elem("h4", undefined, "字段"));
    const add = elem("button", "er-te-add", "＋ 添加字段"); add.type = "button"; add.disabled = !!this.editReason();
    add.addEventListener("click", () => this.addField());
    head.appendChild(add); body.appendChild(head);
    for (const col of this.draft!.columns) body.appendChild(this.renderFieldCard(col));
  }

  private renderFieldCard(col: DesignerColumn): HTMLElement {
    const card = elem("div", "er-fedit" + (col.deleted ? " deleted" : ""));
    const isPk = this.draft!.primary_key.includes(col.name);
    const isFk = (this.draft!.foreign_keys ?? []).some(fk => fk.columns.includes(col.name));
    const l1 = elem("div", "er-fedit-l1");
    l1.appendChild(elem("span", "er-fedit-key " + (isFk ? "fk" : "pk"), isFk ? "🔗" : "🔑"));
    const nameI = this.fieldInput(col.name, "字段名称", v => {
      const at = this.draft!.primary_key.indexOf(col.name);
      if (at >= 0) this.draft!.primary_key[at] = v;
      col.name = v;
    }, col.deleted);
    nameI.classList.add("er-fedit-name");
    const del = elem("button", "er-fedit-del", "🗑"); del.type = "button"; del.title = "删除字段"; del.disabled = !!this.editReason();
    del.addEventListener("click", () => { col.deleted = !col.deleted; this.render(); this.changed(); });
    l1.append(nameI, del);
    const l2 = elem("div", "er-fedit-l2");
    const typeS = this.fieldSelect(col.data_type, this.typeChoices(), "字段类型", v => { col.data_type = v; }, col.deleted);
    typeS.classList.add("er-fedit-type");
    const lenI = this.fieldInput(col.length, "字段长度", v => { col.length = v || null; }, col.deleted);
    lenI.classList.add("er-fedit-len"); lenI.placeholder = "长度";
    l2.append(typeS, lenI,
      this.check(isPk, "PK", "主键", v => { const arr = this.draft!.primary_key; const at = arr.indexOf(col.name); if (v && at < 0) arr.push(col.name); else if (!v && at >= 0) arr.splice(at, 1); }, col.deleted),
      this.check(col.nullable, "NULL", "允许 NULL", v => { col.nullable = v; }, col.deleted));
    if (isFk) l2.appendChild(elem("span", "er-fkchip", "🔗 外键"));
    const l3 = elem("div", "er-fedit-l3");
    const noteI = this.fieldInput(col.comment, "字段注释", v => { col.comment = v; }, col.deleted || this.host.connection()?.kind === "sqlite");
    noteI.placeholder = "注释"; noteI.classList.add("er-fedit-note");
    l3.appendChild(noteI);
    card.append(l1, l2, l3);
    return card;
  }

  private addField(): void {
    const reason = this.editReason(); if (reason) { this.host.toast("info", reason); return; }
    const types = this.typeChoices();
    this.draft!.columns.push({ key: "new-" + (++this.fieldSeq), name: "", orig_name: null, data_type: types[0] ?? "text", length: null, scale: null, nullable: true, default_value: null, auto_increment: false, comment: "", unsigned: false, zerofill: false, deleted: false });
    this.render(); this.changed();
  }

  private renderIndexes(body: HTMLElement): void {
    const draft = this.draft!;
    const head = elem("div", "er-te-subhead");
    head.appendChild(elem("h4", undefined, "索引"));
    const add = elem("button", "er-te-add", "＋ 添加索引"); add.type = "button"; add.disabled = !!this.editReason();
    add.addEventListener("click", () => { (draft.indexes ??= []).push({ name: "", columns: [], unique: false }); this.render(); this.changed(); });
    head.appendChild(add); body.appendChild(head);
    if (!draft.indexes) { body.appendChild(elem("p", "er-te-empty", "索引元数据不可编辑。")); return; }
    const pkRow = elem("div", "er-te-pkrow");
    pkRow.append(elem("span", "er-te-pklabel", "PRIMARY（主键）"), elem("span", "er-te-pkcols", draft.primary_key.join(", ") || "—"));
    body.appendChild(pkRow);
    // 索引字段候选：当前未删除的字段名；datalist 只提供建议，逗号分隔的自由输入保持不变。
    const columnNames = [...new Set(draft.columns.filter(c => !c.deleted && c.name.trim()).map(c => c.name.trim()))];
    const columnList = elem("datalist");
    if (columnNames.length) {
      columnList.id = `er-te-idx-cols-${crypto.randomUUID()}`;
      columnList.append(...columnNames.map(name => new Option(name, name)));
      body.appendChild(columnList);
    }
    draft.indexes.forEach((idx, i) => {
      const row = elem("div", "er-te-idxrow");
      const nameI = this.fieldInput(idx.name, "索引名称", v => { idx.name = v; });
      const colI = this.fieldInput(idx.columns.join(", "), "索引字段（逗号分隔）", v => { idx.columns = v.split(",").map(s => s.trim()).filter(Boolean); });
      if (columnNames.length) {
        colI.setAttribute("list", columnList.id);
        colI.setAttribute("data-search-token", ",");
      }
      const uniq = this.fieldSelect(idx.unique ? "UNIQUE" : "NORMAL", ["NORMAL", "UNIQUE"], "索引类型", v => { idx.unique = v === "UNIQUE"; });
      const del = elem("button", "er-fedit-del", "🗑"); del.type = "button"; del.disabled = !!this.editReason();
      del.addEventListener("click", () => { draft.indexes!.splice(i, 1); this.render(); this.changed(); });
      row.append(nameI, colI, uniq, del);
      body.appendChild(row);
    });
  }

  private renderForeignKeys(body: HTMLElement): void {
    if (!this.refTables.values && !this.refTables.loading) void this.ensureRefTables();
    const head = elem("div", "er-te-subhead");
    head.appendChild(elem("h4", undefined, "外键关系"));
    body.appendChild(head);
    const fks = this.draft!.foreign_keys;
    if (!fks) body.appendChild(elem("p", "er-te-empty", "外键元数据不可编辑。"));
    else for (const fk of fks) body.appendChild(this.renderFkCard(fk));
    body.appendChild(this.renderFkForm());
  }

  private renderFkCard(fk: ForeignKeyDesign): HTMLElement {
    const card = elem("div", "er-fkitem");
    const r1 = elem("div", "er-fkitem-r1");
    r1.appendChild(elem("span", "er-fkitem-name", "🔗 " + (fk.name || "（未命名外键）")));
    const del = elem("button", "er-fedit-del", "🗑"); del.type = "button"; del.title = "删除外键"; del.disabled = !!this.editReason();
    del.addEventListener("click", () => { const arr = this.draft!.foreign_keys!; const at = arr.indexOf(fk); if (at >= 0) arr.splice(at, 1); this.render(); this.changed(); });
    r1.appendChild(del);
    const r2 = elem("div", "er-fkitem-r2");
    r2.append(elem("code", undefined, fk.columns.join(", ")), elem("span", "er-fk-arrow", " → "), elem("code", undefined, fk.ref_table + "." + fk.ref_columns.join(", ")));
    const chips = elem("div", "er-fkchips");
    chips.appendChild(elem("span", "er-tag b", "基数 " + (this.fkCardinality.get(fk) ?? this.deriveCardinality(fk))));
    if (fk.on_delete) chips.appendChild(elem("span", "er-tag", "ON DELETE " + fk.on_delete));
    if (fk.on_update) chips.appendChild(elem("span", "er-tag", "ON UPDATE " + fk.on_update));
    card.append(r1, r2, chips);
    return card;
  }

  private renderFkForm(): HTMLElement {
    const form = elem("div", "er-fkform");
    const grid = elem("div", "er-fkform-grid");
    const cols = this.draft!.columns.filter(c => !c.deleted).map(c => c.name);
    const field = (labelText: string, control: HTMLElement) => { const w = elem("div", "er-fkform-field"); w.appendChild(elem("p", "er-te-lbl", labelText)); w.appendChild(control); return w; };
    const local = this.fieldSelect(this.form.local, cols, "本表字段", v => { this.form.local = v; });
    const refTable = this.fieldSelect(this.form.refTable, this.refTables.values ?? [], "引用表", v => { this.form.refTable = v; this.form.refCol = ""; void this.ensureRefColumns(v); this.render(); }, !this.refTables.values);
    const refCols = this.refColumns.get(this.form.refTable)?.values ?? [];
    const refCol = this.fieldSelect(this.form.refCol, refCols, "引用字段", v => { this.form.refCol = v; }, !refCols.length);
    const card = this.fieldSelect(this.form.cardinality, ["N:1", "1:1", "1:N", "N:N"], "关系基数", v => { this.form.cardinality = v; });
    const onDel = this.fieldSelect(this.form.onDelete, ["RESTRICT", "CASCADE", "SET NULL", "NO ACTION", ""], "删除时 ON DELETE", v => { this.form.onDelete = v; });
    const onUpd = this.fieldSelect(this.form.onUpdate, ["CASCADE", "RESTRICT", "SET NULL", "NO ACTION", ""], "更新时 ON UPDATE", v => { this.form.onUpdate = v; });
    grid.append(field("本表字段", local), field("引用表", refTable), field("引用字段", refCol), field("关系基数", card), field("删除时 (ON DELETE)", onDel), field("更新时 (ON UPDATE)", onUpd));
    form.appendChild(grid);
    const btns = elem("div", "er-fkform-btns");
    const cancel = elem("button", "er-te-btn", "取消"); cancel.type = "button"; cancel.addEventListener("click", () => { this.resetForm(); });
    const ok = elem("button", "er-te-btn pri", "确定并连线"); ok.type = "button"; ok.disabled = !!this.editReason();
    ok.addEventListener("click", () => this.commitFk());
    btns.append(cancel, ok); form.appendChild(btns);
    form.appendChild(elem("p", "er-fkform-hint", "确定后画布会生成 / 更新连线，端点显示基数 N / 1。"));
    return form;
  }

  private resetForm(): void { this.form = { local: this.form.local, refTable: "", refCol: "", cardinality: "N:1", onDelete: "RESTRICT", onUpdate: "CASCADE" }; this.render(); }

  private commitFk(): void {
    const reason = this.editReason(); if (reason) { this.host.toast("info", reason); return; }
    const local = (this.form.local || "").trim();
    const refTable = (this.form.refTable || "").trim();
    const refCol = (this.form.refCol || "").trim();
    if (!local) { this.host.toast("error", "请选择本表字段"); return; }
    if (!refTable || !refCol) { this.host.toast("error", "请选择引用表与引用字段"); return; }
    const draft = this.draft!;
    const list = draft.foreign_keys ?? (draft.foreign_keys = []);
    const name = "fk_" + this.opts.table + "_" + local + "_" + (list.length + 1);
    const fk: ForeignKeyDesign = { name, columns: [local], ref_table: refTable, ref_schema: this.opts.schema, ref_columns: [refCol], on_update: this.form.onUpdate || null, on_delete: this.form.onDelete || null };
    list.push(fk);
    this.fkCardinality.set(fk, this.form.cardinality || "N:1");
    this.form.refCol = "";
    this.render(); this.changed();
  }

  private async ensureRefTables(): Promise<void> {
    if (this.refTables.loading || this.refTables.values) return;
    this.refTables = { values: null, loading: true, error: "" };
    try {
      const list = await api.listTables(this.opts.connectionId, this.opts.schema);
      if (this.disposed) return;
      this.refTables = { values: list.filter(([, k]) => k === "table").map(([n]) => n), loading: false, error: "" };
    } catch (e) {
      if (this.disposed) return;
      this.refTables = { values: null, loading: false, error: String(e) };
    }
    if (this.tab === "外键") this.render();
  }

  private async ensureRefColumns(table: string): Promise<void> {
    if (!table) return;
    const cur = this.refColumns.get(table);
    if (cur?.loading || cur?.values) return;
    this.refColumns.set(table, { values: null, loading: true, error: "" });
    try {
      const s = await api.getTableSchema(this.opts.connectionId, this.opts.schema, table);
      if (this.disposed) return;
      this.refColumns.set(table, { values: s.columns.map(c => c.name), loading: false, error: "" });
    } catch (e) {
      if (this.disposed) return;
      this.refColumns.set(table, { values: null, loading: false, error: String(e) });
    }
    if (this.tab === "外键" && this.form.refTable === table) this.render();
  }

  /** 生成后端计划、按设计稿弹窗确认、受控执行并读回校验。 */
  async save(): Promise<void> {
    if (!this.snapshot || !this.draft) return;
    if (!this.draftDirty()) { this.host.toast("info", "没有待保存的结构修改。"); return; }
    if (this.saving) return;
    const reason = this.editReason(); if (reason) { this.host.toast("error", reason); return; }
    this.saving = true; this.render(); this.host.changed();
    let sent = false;
    try {
      const design = payloadFromDraft(this.snapshot, this.draft);
      const preview = await api.previewTableDesign(this.opts.connectionId, design, this.opts.ownerId);
      if (!preview.request_sql || preview.revision !== this.snapshot.revision) throw new Error("后端预览缺少绑定载荷或基线版本不一致。");
      const signature = draftSignature(this.draft);
      const confirmed = await reviewErSyncPlan({ connectionName: this.host.connection()?.name ?? "", schema: this.opts.schema, table: this.opts.table, preview });
      if (!confirmed || this.disposed) return;
      if (signature !== draftSignature(this.draft)) throw new Error("确认期间草稿发生变化，未发送修改。");
      const blocked = this.host.writeReason(); if (blocked) throw new Error(blocked);
      let response: TableDesignResponse | null = null;
      this.execution = new ExecutionController({ ...api, executeSqlV2: async (request) => { sent = true; response = await api.executeTableDesign(request, design); return response.outcome; } }, () => this.host.changed());
      await this.execution.run({ execution_id: crypto.randomUUID(), connection_id: this.opts.connectionId, namespace: design.schema, owner_id: this.opts.ownerId, sql: preview.request_sql, origin: "table-design" });
      if (!response) throw new Error("缺少表结构执行回执。");
      const snapshot = verifiedDesignResponse(response);
      this.accept(snapshot);
      this.message = "表结构已保存，完整读回验证通过。";
      this.host.toast("success", this.message);
    } catch (e) {
      if (sent) this.message = String(e) + " 请核对真实状态后重新载入。";
      else this.message = String(e);
      this.host.toast("error", this.message);
    } finally {
      this.saving = false; this.render(); this.host.changed();
    }
  }
}
