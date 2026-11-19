import { api, type TableDesignSnapshot, type TableDesignResponse, type ForeignKeyDesign } from "../core/api";
import type { ConnectionConfig } from "../core/types";
import type { ErTable } from "../core/erDiagram";
import { scheduleErDesignRead, ErDesignReadCancelled, type ErDesignReadTask } from "../core/erDesignReadQueue";
import { ExecutionController, formatError } from "../core/execution";
import { assertDesignSnapshot, designClone, draftFromSnapshot, draftSignature, payloadFromDraft, verifiedDesignResponse, type TableDesignerDraft, type DesignerColumn } from "../core/table-design-draft";
import { reviewErSyncPlan } from "../dialogs/ErSyncDialog";
import "../styles/er-table-editor.css";

export interface ErTableEditorOptions { connectionId: string; schema: string; table: string; ownerId: string; summary?: ErTable; }
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
  private loadState: "idle" | "loading" | "error" | "ready" = "idle";
  private loadError = "";
  private metadataVersion = 0;
  private saving = false;
  private disposed = false;
  private message = "";
  private messageTone: "error" | "success" = "error";
  private execution: ExecutionController | null = null;
  private readonly fkCardinality = new Map<object, string>();
  private refTables: { values: string[] | null; loading: boolean; error: string } = { values: null, loading: false, error: "" };
  private readonly refColumns = new Map<string, { values: string[] | null; loading: boolean; error: string }>();
  private form = { local: "", refTable: "", refCol: "", cardinality: "N:1", onDelete: "RESTRICT", onUpdate: "CASCADE" };
  private fieldSeq = 0;
  private summary: ErTable | null = null;
  private summarySource: ErTable | undefined;
  private active = false;
  private loadVersion = 0;
  private readTask: ErDesignReadTask | null = null;
  private reloadAfterRead = false;
  private slow = false;
  private slowTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(private readonly opts: ErTableEditorOptions, private readonly host: ErTableEditorHost, el: HTMLElement) {
    this.el = el;
    this.el.classList.add("er-te");
    this.setSummary(opts.summary);
    this.render();
  }

  activate(): void { this.active = true; if (this.loadState === "idle") void this.load(); }
  deactivate(): void {
    this.active = false;
    // 已发送的读取继续受后端预算保护，切回时复用它；未发送选择不占用数据库。
    if (this.readTask?.cancelQueued()) {
      ++this.loadVersion; this.readTask = null; this.loadState = "idle";
      this.clearSlowTimer(); this.render();
    }
  }
  setSummary(summary?: ErTable): void {
    if (!summary || summary === this.summarySource || summary.name !== this.opts.table || (summary.schema && summary.schema !== this.opts.schema)) return;
    this.summarySource = summary;
    this.summary = designClone(summary);
    // 摘要更新永远不改变完整基线或已有草稿。
    if (!this.disposed && !this.snapshot) this.render();
  }
  /** 刷新使所有干净基线失效；在途旧回执不能冒充刷新后的新基线。 */
  reload(): void {
    if (this.disposed || this.saving || this.draftDirty()) return;
    ++this.loadVersion; ++this.metadataVersion;
    this.reloadAfterRead = true;
    if (this.readTask && !this.readTask.cancelQueued()) {
      this.loadState = "loading"; this.render(); this.host.changed(); return;
    }
    this.readTask = null; this.loadState = "idle"; this.clearSlowTimer();
    if (this.active) void this.load(); else this.render();
  }
  draftDirty(): boolean { return !!this.draft && draftSignature(this.draft) !== this.baseline; }
  saveReason(): string | null {
    return this.editReason() || (!this.snapshot || !this.draft ? "完整表设计尚未载入，请先读取或重试。" : !this.draftDirty() ? "没有待保存的结构修改。" : null);
  }
  dispose(): void {
    this.disposed = true; this.active = false; ++this.metadataVersion; ++this.loadVersion;
    this.readTask?.cancelQueued(); this.readTask = null; this.clearSlowTimer(); this.execution?.dispose();
  }
  destroy(): void { this.dispose(); }

  private clearSlowTimer(): void {
    if (this.slowTimer !== null) clearTimeout(this.slowTimer);
    this.slowTimer = null; this.slow = false;
  }

  private async load(): Promise<void> {
    if (this.readTask || this.saving || this.disposed || this.draftDirty()) return;
    const version = ++this.loadVersion;
    ++this.metadataVersion; this.reloadAfterRead = false;
    this.refTables = { values: null, loading: false, error: "" };
    this.refColumns.clear(); this.clearSlowTimer();
    this.loadState = "loading"; this.loadError = ""; this.message = "";
    const c = this.host.connection();
    // 只在内存中用端点身份分组，不存储或打印配置，不跨 owner 复用快照。
    const key = JSON.stringify([this.opts.connectionId, this.opts.schema, c?.kind, c?.host, c?.port,
      c?.user, c?.password, c?.database, c?.file, c?.params, c?.read_only]);
    const task = scheduleErDesignRead(key,
      () => api.readTableDesignSnapshot(this.opts.connectionId, this.opts.schema, this.opts.table, this.opts.ownerId),
      () => {
        if (this.disposed || this.readTask !== task) return;
        this.slowTimer = setTimeout(() => {
          this.slowTimer = null;
          if (!this.disposed && this.readTask === task) { this.slow = true; this.render(); }
        }, 10_000);
        this.render();
      });
    this.readTask = task;
    this.render(); this.host.changed();
    try {
      const s = await task.promise;
      if (this.disposed || version !== this.loadVersion) return;
      assertDesignSnapshot(s);
      if (s.owner_id !== this.opts.ownerId || s.design.schema !== this.opts.schema || s.design.table !== this.opts.table) {
        throw new Error("表设计回执归属不一致，请重新读取。");
      }
      this.accept(s);
      this.loadState = "ready";
    } catch (e) {
      if (this.disposed || version !== this.loadVersion) return;
      this.loadState = e instanceof ErDesignReadCancelled ? "idle" : "error";
      this.loadError = e instanceof ErDesignReadCancelled ? "" : "读取表设计失败：" + formatError(e);
    } finally {
      if (this.readTask === task) {
        this.readTask = null; this.clearSlowTimer();
        if (!this.disposed) {
          if (this.reloadAfterRead) {
            this.loadState = "idle";
            if (this.active) void this.load();
          }
          this.render(); this.host.changed();
        }
      }
    }
  }

  private accept(s: TableDesignSnapshot): void {
    // 校验与草稿转换全部完成后再提交，异常不能留下半个基线。
    const draft = draftFromSnapshot(s);
    const baseline = draftSignature(draft);
    this.snapshot = designClone(s);
    this.draft = draft;
    this.baseline = baseline;
    ++this.metadataVersion;
    this.refTables = { values: null, loading: false, error: "" };
    this.refColumns.clear();
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
    return this.disposed ? "表设计器已关闭。" : this.host.writeReason() || (this.loadState === "loading" ? "正在读取表结构。" : this.loadState !== "ready" ? "完整表设计尚未载入，请先读取或重试。" : this.saving ? "正在保存表结构。" : null);
  }

  private changed(): void {
    const status = this.el.querySelector<HTMLElement>(".er-te-status");
    if (status) status.textContent = this.draftDirty() ? "有未保存的表结构草稿。" : "表结构与读取基线一致。";
    this.host.changed();
  }

  private loadingText(): string {
    if (this.readTask && !this.readTask.started) return "正在排队补齐完整表设计；切换表会撤销尚未发送的读取。";
    if (this.slow) return "完整表设计读取较慢，仍在等待数据库响应；已有内容可查看，失败后可重试。";
    return this.reloadAfterRead ? "刷新已使旧读取失效，正在等待旧请求结束后重新读取。" : "正在后台补齐完整表设计…";
  }

  private renderSummary(body: HTMLElement): void {
    const summary = this.summary!;
    const wrap = elem("section", "er-te-summary");
    wrap.setAttribute("aria-label", `${this.opts.table} 的只读结构摘要`);
    wrap.appendChild(elem("p", "er-te-summary-note", "已显示关系图结构摘要（只读），不是完整设计基线；默认值、完整约束与选项需等待读取。"));
    const list = elem("ul", "er-te-summary-list");
    if (this.tab === "字段") {
      for (const c of summary.columns) {
        const item = elem("li", "er-te-summary-item");
        const line = elem("div", "er-te-summary-line");
        line.append(elem("strong", undefined, c.name), elem("code", undefined, c.type));
        const flags = [c.primary_key ? "PK" : "", c.auto_increment ? "自增" : "", c.nullable ? "允许 NULL" : "NOT NULL"].filter(Boolean);
        item.append(line, elem("p", "er-te-summary-meta", flags.join(" · ")));
        if (c.comment) item.appendChild(elem("p", "er-te-summary-meta", c.comment));
        list.appendChild(item);
      }
    } else if (this.tab === "索引") {
      for (const index of summary.indexes ?? []) {
        const item = elem("li", "er-te-summary-item");
        item.append(elem("strong", undefined, index.name || (index.primary ? "PRIMARY" : "索引名称未载入")),
          elem("p", "er-te-summary-meta", `${index.primary ? "主键" : index.unique ? "唯一索引" : "普通索引"} · ${index.columns.join(", ")}`));
        list.appendChild(item);
      }
    } else {
      for (const fk of summary.foreignKeys) {
        const item = elem("li", "er-fkitem er-te-summary-item");
        item.append(elem("strong", undefined, fk.name || "外键名称未载入"),
          elem("p", "er-te-summary-meta", `${fk.columns.join(", ")} → ${fk.ref_schema || summary.schema || this.opts.schema}.${fk.ref_table} (${fk.ref_columns.join(", ")})`));
        list.appendChild(item);
      }
    }
    if (!list.children.length) wrap.appendChild(elem("p", "er-te-empty", `关系图摘要未列出${this.tab}；以完整设计读取结果为准。`));
    else wrap.appendChild(list);
    body.appendChild(wrap);
  }

  private render(): void {
    if (this.disposed) return;
    const focus = document.activeElement;
    const focusedTab = focus?.parentElement === this.el.querySelector(".er-te-tabs") ? focus?.textContent : null;
    this.el.dataset.loadState = this.loadState;
    // 已有摘要保持可访问，不让整个区域的 busy 阻塞读屏器读取已有内容。
    this.el.setAttribute("aria-busy", String((this.loadState === "loading" && !this.summary && !this.snapshot) || this.saving));
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
    // 渲染/异步补齐不丢掉当前页签的键盘焦点。
    if (focusedTab) (Array.from(tabs.children).find(t => t.textContent === focusedTab) as HTMLElement | undefined)?.focus();
    const body = elem("div", "er-te-body");
    this.el.appendChild(body);
    if (this.loadError) {
      const error = elem("div", "er-te-notice", this.loadError);
      error.setAttribute("role", "alert");
      body.appendChild(error);
    }
    if (this.loadState === "idle" || this.loadState === "error") {
      const retry = elem("button", "er-te-btn er-te-retry", this.loadState === "error" ? "重试读取表设计" : "读取表设计");
      retry.type = "button";
      retry.addEventListener("click", () => this.reload());
      body.appendChild(retry);
    }
    if (!this.snapshot || !this.draft) {
      const status = elem("p", "er-te-summary-note", this.loadState === "loading" ? this.loadingText() : this.loadState === "error" ? "完整设计尚未取得；已有结构摘要仍可查看，请重试读取。" : "选择表后后台读取完整表设计。");
      status.setAttribute("role", "status");
      body.appendChild(status);
      if (this.summary) this.renderSummary(body);
      return;
    }
    if (this.loadState === "loading") {
      const status = elem("p", "er-te-summary-note", this.loadingText());
      status.setAttribute("role", "status");
      body.appendChild(status);
    }
    if (this.message) {
      const notice = elem("div", "er-te-notice", this.message);
      notice.dataset.tone = this.messageTone;
      notice.setAttribute("role", this.messageTone === "error" ? "alert" : "status");
      body.appendChild(notice);
    }
    const reason = this.host.writeReason();
    if (reason) body.appendChild(elem("p", "er-te-empty", reason));
    const nameField = elem("div", "er-te-namefield");
    nameField.appendChild(elem("p", "er-te-lbl", "表名"));
    const ni = elem("input", "er-te-input");
    ni.value = this.opts.table; ni.disabled = true; ni.title = "表名修改请使用表结构编辑器"; ni.setAttribute("aria-label", "表名");
    nameField.appendChild(ni);
    body.appendChild(nameField);
    if (this.tab === "字段") this.renderFields(body);
    else if (this.tab === "索引") this.renderIndexes(body);
    else this.renderForeignKeys(body);
    const status = elem("div", "er-te-status", this.loadState === "error" ? "重新读取失败，已保留上次基线。" : this.loadState === "loading" ? "正在读取表结构。" : this.saving ? "正在保存表结构。" : this.draftDirty() ? "有未保存的表结构草稿。" : "完整表设计已读取，表结构与读取基线一致。");
    status.setAttribute("role", "status");
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
    const del = elem("button", "er-fedit-del", "🗑"); del.type = "button"; del.title = col.deleted ? "恢复字段" : "删除字段"; del.setAttribute("aria-label", del.title); del.disabled = !!this.editReason();
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
      const del = elem("button", "er-fedit-del", "🗑"); del.type = "button"; del.title = "删除索引"; del.setAttribute("aria-label", del.title); del.disabled = !!this.editReason();
      del.addEventListener("click", () => { draft.indexes!.splice(i, 1); this.render(); this.changed(); });
      row.append(nameI, colI, uniq, del);
      body.appendChild(row);
    });
  }

  private renderForeignKeys(body: HTMLElement): void {
    if (this.loadState === "ready" && !this.refTables.values && !this.refTables.loading && !this.refTables.error) void this.ensureRefTables();
    if (this.loadState === "ready" && this.refTables.values?.includes(this.form.refTable) && !this.refColumns.has(this.form.refTable)) void this.ensureRefColumns(this.form.refTable);
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
    const del = elem("button", "er-fedit-del", "🗑"); del.type = "button"; del.title = "删除外键"; del.setAttribute("aria-label", del.title); del.disabled = !!this.editReason();
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
    this.renderReferenceStatus(form, this.refTables, "引用表", () => void this.ensureRefTables());
    const reference = this.refColumns.get(this.form.refTable);
    if (reference) this.renderReferenceStatus(form, reference, "引用字段", () => void this.ensureRefColumns(this.form.refTable));
    const btns = elem("div", "er-fkform-btns");
    const cancel = elem("button", "er-te-btn", "取消"); cancel.type = "button"; cancel.addEventListener("click", () => { this.resetForm(); });
    const ok = elem("button", "er-te-btn pri", "确定并连线"); ok.type = "button"; ok.disabled = !!this.editReason();
    ok.addEventListener("click", () => this.commitFk());
    btns.append(cancel, ok); form.appendChild(btns);
    form.appendChild(elem("p", "er-fkform-hint", "确定后画布会生成 / 更新连线，端点显示基数 N / 1。"));
    return form;
  }

  private resetForm(): void { this.form = { local: this.form.local, refTable: "", refCol: "", cardinality: "N:1", onDelete: "RESTRICT", onUpdate: "CASCADE" }; this.render(); }

  private renderReferenceStatus(body: HTMLElement, state: { loading: boolean; error: string }, label: string, retry: () => void): void {
    if (state.loading) {
      const status = elem("p", "er-fkform-hint", `正在读取${label}…`);
      status.setAttribute("role", "status");
      body.appendChild(status);
    } else if (state.error) {
      const error = elem("div", "er-te-notice", `读取${label}失败：${state.error}`);
      error.setAttribute("role", "alert");
      const button = elem("button", "er-te-btn er-te-retry", `重试读取${label}`);
      button.type = "button";
      button.addEventListener("click", retry);
      body.append(error, button);
    }
  }

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
    if (this.disposed || this.refTables.loading || this.refTables.values) return;
    const version = this.metadataVersion;
    this.refTables = { values: null, loading: true, error: "" };
    // 在微任务中渲染加载状态，避免从 renderForeignKeys 递归重建 DOM。
    void Promise.resolve().then(() => { if (!this.disposed && version === this.metadataVersion && this.tab === "外键") this.render(); });
    try {
      const list = await api.listTables(this.opts.connectionId, this.opts.schema);
      if (this.disposed || version !== this.metadataVersion) return;
      this.refTables = { values: list.filter(([, k]) => k === "table").map(([n]) => n), loading: false, error: "" };
    } catch (e) {
      if (this.disposed || version !== this.metadataVersion) return;
      this.refTables = { values: null, loading: false, error: formatError(e) };
    }
    if (this.tab === "外键") this.render();
  }

  private async ensureRefColumns(table: string): Promise<void> {
    if (!table || this.disposed) return;
    const version = this.metadataVersion;
    const cur = this.refColumns.get(table);
    if (cur?.loading || cur?.values) return;
    this.refColumns.set(table, { values: null, loading: true, error: "" });
    void Promise.resolve().then(() => { if (!this.disposed && version === this.metadataVersion && this.tab === "外键" && this.form.refTable === table) this.render(); });
    try {
      const s = await api.getTableSchema(this.opts.connectionId, this.opts.schema, table);
      if (this.disposed || version !== this.metadataVersion) return;
      this.refColumns.set(table, { values: s.columns.map(c => c.name), loading: false, error: "" });
    } catch (e) {
      if (this.disposed || version !== this.metadataVersion) return;
      this.refColumns.set(table, { values: null, loading: false, error: formatError(e) });
    }
    if (this.tab === "外键" && this.form.refTable === table) this.render();
  }

  /** 生成后端计划、按设计稿弹窗确认、受控执行并读回校验。 */
  async save(): Promise<void> {
    if (this.disposed || this.saving) return;
    const reason = this.saveReason(); if (reason) { this.host.toast("info", reason); return; }
    if (!this.snapshot || !this.draft) return;
    this.saving = true; this.render(); this.host.changed();
    let sent = false;
    try {
      const signature = draftSignature(this.draft);
      const design = payloadFromDraft(this.snapshot, this.draft);
      const preview = await api.previewTableDesign(this.opts.connectionId, design, this.opts.ownerId);
      if (this.disposed) return;
      if (signature !== draftSignature(this.draft)) throw new Error("预览期间草稿发生变化，请重新预览，未发送修改。");
      if (!preview.request_sql || preview.revision !== this.snapshot.revision) throw new Error("后端预览缺少绑定载荷或基线版本不一致。");
      const confirmed = await reviewErSyncPlan({ connectionName: this.host.connection()?.name ?? "", schema: this.opts.schema, table: this.opts.table, preview });
      if (!confirmed || this.disposed) return;
      if (signature !== draftSignature(this.draft)) throw new Error("确认期间草稿发生变化，未发送修改。");
      const blocked = this.host.writeReason(); if (blocked) throw new Error(blocked);
      let response: TableDesignResponse | null = null;
      this.execution = new ExecutionController({ ...api, executeSqlV2: async (request) => { sent = true; response = await api.executeTableDesign(request, design); return response.outcome; } }, () => this.host.changed());
      await this.execution.run({ execution_id: crypto.randomUUID(), connection_id: this.opts.connectionId, namespace: design.schema, owner_id: this.opts.ownerId, sql: preview.request_sql, origin: "table-design" });
      if (this.disposed) return;
      if (!response) throw new Error("缺少表结构执行回执。");
      const snapshot = verifiedDesignResponse(response);
      this.accept(snapshot);
      this.loadState = "ready"; this.loadError = "";
      this.message = "表结构已保存，完整读回验证通过。";
      this.messageTone = "success";
      this.host.toast("success", this.message);
    } catch (e) {
      if (this.disposed) return;
      this.message = formatError(e) + (sent ? " 请核对真实状态后重新载入。" : "");
      this.messageTone = "error";
      this.host.toast("error", this.message);
    } finally {
      this.saving = false;
      if (!this.disposed) { this.render(); this.host.changed(); }
    }
  }
}
