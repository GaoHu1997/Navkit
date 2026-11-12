/** 可视化 SELECT 查询创建工具：真实元数据驱动、实时生成 SQL 预览；插入仅修改编辑器，不执行语句。 */
import { api, parseDriverParams } from "../core/api";
import { buildSelectQuery, defaultQueryDialect, MAX_QUERY_LIMIT, QUERY_DIALECTS, queryBuilderUnavailableReason } from "../core/queryBuilder";
import type { QueryBuilderSpec, QueryDialect, QueryFilterOperator } from "../core/queryBuilder";
import type { ConnectionConfig, TableSchema } from "../core/types";
import { Dialog, numberInput, select } from "../ui/Dialog";
import { namespaceName } from "../core/namespace-label";
import { accessibleToolDialog, toolButton } from "./toolDialog";
import { filterJoinControl, filterValueControl } from "../ui/TableFilterControls";
import "../ui/data-grid-controls.css";

export type QueryBuilderApi = Pick<typeof api, "ensureConnection" | "listTables" | "getTableSchema">;

/** 与表页一致：一行是一个括号组，条件值在点击后就地编辑。 */
interface FilterRowState {
  column: string; op: QueryFilterOperator; val: string; touched: boolean; join: "AND" | "OR"; seq: number;
}
interface FilterGroupState { join: "AND" | "OR"; conditions: FilterRowState[] }

/** 排序行控件状态。 */
interface SortRowState {
  column: HTMLSelectElement;
  direction: HTMLSelectElement;
}

/** 输出字段勾选状态。 */
interface FieldState {
  name: string;
  input: HTMLInputElement;
}

/** 支持的筛选运算符（与 core/queryBuilder 的 QueryFilterOperator 保持一致）。 */
const FILTER_OPERATORS: QueryFilterOperator[] = ["=", "<>", ">", ">=", "<", "<=", "LIKE", "NOT LIKE", "IN", "NOT IN", "IS NULL", "IS NOT NULL"];
/** 筛选条件行数上限（与 buildSelectQuery 的校验一致）。 */
const MAX_FILTERS = 20;

/** 对话框局部样式：选择器一律以 .query-builder-dialog 作用域，不修改 base.css。 */
const QUERY_BUILDER_STYLE = `
.query-builder-dialog .qb-target { display:flex; align-items:center; gap:8px; flex-wrap:wrap; margin-bottom:8px; }
.query-builder-dialog .qb-chip { min-width:0; max-width:240px; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; background:var(--bg-subtle); border:1px solid var(--border); border-radius:4px; padding:3px 10px; font-size:var(--fs-12); color:var(--fg-muted); }
.query-builder-dialog .qb-status { min-height:20px; margin-bottom:6px; font-size:var(--fs-12); color:var(--fg-muted); overflow-wrap:anywhere; }
.query-builder-dialog .qb-status.is-error { color:var(--danger-500); }
.query-builder-dialog .qb-workspace { display:flex; gap:10px; height:clamp(300px, calc(100vh - 320px), 460px); }
.query-builder-dialog .qb-builder { flex:1; min-width:0; overflow-y:auto; padding-right:2px; }
.query-builder-dialog .qb-preview-pane { width:300px; flex:none; display:flex; flex-direction:column; }
.query-builder-dialog .qb-card { background:var(--bg-surface); border:1px solid var(--border); padding:8px; margin-bottom:8px; }
.query-builder-dialog .qb-card-head { display:flex; align-items:center; gap:6px; margin-bottom:6px; }
.query-builder-dialog .qb-card-title { font-weight:600; margin-right:auto; font-size:var(--fs-13); }
.query-builder-dialog .qb-text-btn { background:none; border:none; padding:2px 4px; font-size:var(--fs-12); color:var(--accent-500); cursor:pointer; }
.query-builder-dialog .qb-text-btn:hover { text-decoration:underline; }
.query-builder-dialog .qb-text-btn:disabled { color:var(--fg-disabled); cursor:default; text-decoration:none; }
.query-builder-dialog .qb-field-grid { display:grid; grid-template-columns:minmax(0, 1fr); gap:2px; max-height:150px; overflow-y:auto; }
.query-builder-dialog .qb-field-grid .checkbox { margin:0; display:flex; align-items:center; gap:5px; min-width:0; }
.query-builder-dialog .qb-field-grid .checkbox span:not(.qb-col-type) { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
.query-builder-dialog .qb-col-type { margin-left:auto; flex:none; color:var(--fg-muted); font-size:var(--fs-12); }
.query-builder-dialog .qb-row { display:flex; align-items:center; gap:6px; flex-wrap:wrap; margin-bottom:6px; }
.query-builder-dialog .qb-join-toggle { min-width:52px; flex:none; }
.query-builder-dialog .qb-remove { min-width:28px; flex:none; padding:0 6px; }
.query-builder-dialog .qb-gen-status { min-height:18px; margin-bottom:4px; font-size:var(--fs-12); color:var(--fg-muted); overflow-wrap:anywhere; white-space:pre-wrap; }
.query-builder-dialog .qb-gen-status.is-error { color:var(--danger-500); }
.query-builder-dialog .qb-preview { flex:1; width:100%; box-sizing:border-box; resize:none; font-family:var(--font-mono); font-size:var(--fs-12); background:var(--bg-subtle); }
.query-builder-dialog .qb-hint { color:var(--fg-muted); font-size:var(--fs-12); }
.query-builder-dialog .qb-empty { color:var(--fg-muted); font-size:var(--fs-12); grid-column:1 / -1; }
`;

export function openQueryBuilderDialog(
  connection: ConnectionConfig, schema: string, onInsert: (sql: string) => void, source: QueryBuilderApi = api,
): Dialog {
  const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;

  // ---------- 顶部目标区：连接 / 库 chip + 表视图 + 方言 + 重新加载 ----------
  const chip = document.createElement("span"); chip.className = "qb-chip";
  chip.textContent = `${connection.name} / ${schema || `未选择${namespaceName(connection)}`}`;
  const table = select([], undefined, 190); table.setAttribute("aria-label", "查询表或视图"); table.disabled = true;
  const dialect = select([{ value: "", label: "请选择 SQL 方言" }, ...QUERY_DIALECTS], parseDriverParams(connection.params) ? "" : defaultQueryDialect(connection.kind), 250);
  dialect.setAttribute("aria-label", "SQL 方言");
  const reloadTop = toolButton("重新加载");
  const target = document.createElement("div"); target.className = "qb-target";
  target.append(chip, table, dialect, reloadTop);

  // ---------- 底部按钮与状态 / 预览 ----------
  const insert = toolButton("插入查询编辑器", true); insert.disabled = true;
  const cancel = toolButton("取消");
  const reloadFooter = toolButton("重新加载元数据");
  // 元数据加载状态条（连接 → 列表 → 字段，失败可重试）。
  const status = document.createElement("div"); status.setAttribute("role", "status"); status.setAttribute("aria-live", "polite");
  status.className = "qb-status";
  // SQL 生成状态行（位于预览上方，生成失败时提示原因）。
  const genStatus = document.createElement("div"); genStatus.setAttribute("role", "status"); genStatus.setAttribute("aria-live", "polite");
  genStatus.className = "qb-gen-status";
  const preview = document.createElement("textarea"); preview.className = "qb-preview"; preview.readOnly = true; preview.spellcheck = false;
  preview.setAttribute("aria-label", "待插入 SQL 预览");

  // ---------- 左栏构建区卡片 ----------
  const textButton = (label: string): HTMLButtonElement => {
    const button = document.createElement("button"); button.type = "button"; button.className = "qb-text-btn"; button.textContent = label;
    return button;
  };
  // 每个分区一个卡片：白底 1px 边框、8px 内边距，标题左侧加粗、右侧放分区操作按钮。
  const card = (title: string, actions: HTMLElement[] = []): { el: HTMLElement; content: HTMLElement } => {
    const el = document.createElement("section"); el.className = "qb-card";
    const head = document.createElement("div"); head.className = "qb-card-head";
    const heading = document.createElement("div"); heading.className = "qb-card-title"; heading.textContent = title;
    head.append(heading, ...actions);
    const content = document.createElement("div");
    el.append(head, content);
    return { el, content };
  };
  const emptyHint = (text: string): HTMLElement => { const el = document.createElement("div"); el.className = "qb-empty"; el.textContent = text; return el; };

  // 1. 输出字段：一个字段一行，默认全选，标题行右侧「全选 / 清空」。
  const allFields = textButton("全选"); allFields.disabled = true;
  const clearFields = textButton("清空"); clearFields.disabled = true;
  const fieldsCard = card("输出字段", [allFields, clearFields]);
  const fieldGrid = document.createElement("div"); fieldGrid.className = "qb-field-grid";
  const fieldHint = emptyHint("尚未加载字段，请选择表 / 视图或重新加载。");
  fieldsCard.content.append(fieldGrid);

  // 2. 筛选条件：沿用表页的行内条件 chip、括号组、AND / OR 与值编辑交互。
  const addFilter = toolButton("＋ 添加条件"); addFilter.disabled = true;
  const filtersCard = card("筛选条件", [addFilter]);
  const filterList = document.createElement("div"); filterList.className = "filter-rows data-grid-controls";
  const filterHint = emptyHint("不添加条件时返回全部行；＋ 增加行，()＋ 在行内追加条件，点击 AND / OR 切换。");
  filtersCard.content.append(filterList);

  // 3. 排序：字段 + 方向 + 删除，可多行。
  const addSort = toolButton("＋ 添加排序"); addSort.disabled = true;
  const sortCard = card("排序", [addSort]);
  const sortList = document.createElement("div");
  const sortHint = emptyHint("不添加排序时按数据库返回顺序。");
  sortCard.content.append(sortList);

  // 4. 行数限制：number 输入，1–MAX_QUERY_LIMIT。
  const limit = numberInput(100, 120); limit.min = "1"; limit.max = String(MAX_QUERY_LIMIT); limit.step = "1";
  limit.setAttribute("aria-label", "最大查询行数");
  const limitHint = document.createElement("span"); limitHint.className = "qb-hint"; limitHint.textContent = `1 – ${MAX_QUERY_LIMIT} 行`;
  const limitCard = card("行数限制");
  const limitRow = document.createElement("div"); limitRow.className = "qb-row";
  limitRow.append(limit, limitHint);
  limitCard.content.append(limitRow);

  // ---------- 左右两栏工作区 ----------
  const builder = document.createElement("div"); builder.className = "qb-builder";
  builder.append(fieldsCard.el, filtersCard.el, sortCard.el, limitCard.el);
  const previewPane = document.createElement("div"); previewPane.className = "qb-preview-pane";
  previewPane.append(genStatus, preview);
  const workspace = document.createElement("div"); workspace.className = "qb-workspace";
  workspace.append(builder, previewPane);

  // ---------- 内部状态 ----------
  let closed = false, version = 0, filterSequence = 0, sortSequence = 0;
  let structure: TableSchema | null = null;
  let generated = "", lastValid = "";
  const fields: FieldState[] = [];
  const filterGroups: FilterGroupState[] = [];
  const sortRows: SortRowState[] = [];

  const announce = (message: string, error = false): void => {
    status.setAttribute("role", error ? "alert" : "status");
    status.className = error ? "qb-status is-error" : "qb-status";
    status.textContent = message;
  };
  const announceGeneration = (message: string, error = false): void => {
    genStatus.setAttribute("role", error ? "alert" : "status");
    genStatus.className = error ? "qb-gen-status is-error" : "qb-gen-status";
    genStatus.textContent = message;
  };
  // 新设计不提供值类型下拉：按字段声明类型推断 text / number / boolean。
  const inferValueKind = (meta: TableSchema, name: string): "text" | "number" | "boolean" => {
    const type = (meta.columns.find(c => c.name === name)?.type ?? "").toLowerCase();
    if (/bool|bit/.test(type)) return "boolean";
    if (/int|num|dec|flo|dou|rea|seri|money|year/.test(type)) return "number";
    return "text";
  };

  /** 任何输入变化都立即重新生成 SQL；失败时保留上次有效预览并在状态行提示。 */
  const refresh = (): void => {
    insert.disabled = true;
    generated = "";
    const meta = structure;
    if (!meta || !table.value) {
      preview.value = lastValid;
      announceGeneration("请先加载表 / 视图元数据。");
      return;
    }
    if (filterGroups.some(group => group.conditions.some(row => !row.touched && !/NULL$/.test(row.op)))) {
      preview.value = lastValid; announceGeneration("请点击 <?> 填写筛选值；Enter 或失焦确认，Escape 取消。"); return;
    }
    const spec: QueryBuilderSpec = {
      dialect: dialect.value as QueryDialect,
      schema, table: table.value,
      columns: fields.filter(f => f.input.checked).map(f => f.name),
      filters: [],
      filterGroups: filterGroups.map(group => ({ join: group.join, filters: group.conditions.map(row => ({
        column: row.column, operator: row.op, value: row.val,
        valueKind: /LIKE$/.test(row.op) ? "text" : inferValueKind(meta, row.column), join: row.join,
      })) })),
      orderByRules: sortRows.map(row => ({ column: row.column.value, descending: row.direction.value === "desc" })),
      orderBy: "", descending: false,
      limit: limit.valueAsNumber,
    };
    try {
      generated = buildSelectQuery(meta, spec);
      preview.value = generated;
      lastValid = generated;
      announceGeneration("");
      insert.disabled = false;
    } catch (e) {
      preview.value = lastValid;
      announceGeneration(String(e), true);
    }
  };

  /** 清空构建区（表切换 / 重新加载时）。 */
  const resetBuilder = (): void => {
    structure = null;
    fields.length = 0; filterGroups.length = 0; sortRows.length = 0;
    fieldGrid.replaceChildren(fieldHint); fieldHint.hidden = false;
    filterList.replaceChildren(filterHint); filterHint.hidden = false;
    sortList.replaceChildren(sortHint); sortHint.hidden = false;
    addFilter.disabled = true; addSort.disabled = true;
    allFields.disabled = true; clearFields.disabled = true;
    lastValid = ""; generated = "";
    refresh();
  };

  const renderFields = (info: TableSchema): void => {
    fieldGrid.replaceChildren();
    fields.length = 0;
    for (const col of info.columns) {
      const label = document.createElement("label"); label.className = "checkbox";
      const check = document.createElement("input"); check.type = "checkbox"; check.checked = true;
      check.setAttribute("aria-label", `输出字段 ${col.name}`);
      check.addEventListener("change", refresh);
      const name = document.createElement("span"); name.textContent = col.name;
      const type = document.createElement("span"); type.className = "qb-col-type"; type.textContent = col.type || "未声明类型";
      label.append(check, name, type);
      fieldGrid.appendChild(label);
      fields.push({ name: col.name, input: check });
    }
    fieldHint.hidden = true;
    allFields.disabled = false; clearFields.disabled = false;
  };

  // ---------- 交互 ----------
  allFields.addEventListener("click", () => { for (const f of fields) f.input.checked = true; refresh(); });
  clearFields.addEventListener("click", () => { for (const f of fields) f.input.checked = false; refresh(); });
  const conditionCount = () => filterGroups.reduce((count, group) => count + group.conditions.length, 0);
  const newCondition = (): FilterRowState => ({ column: structure!.columns[0].name, op: "=", val: "", touched: false, join: "OR", seq: ++filterSequence });
  const action = (text: string, label: string, run: () => void): HTMLButtonElement => {
    const button = document.createElement("button"); button.type = "button"; button.className = "fb-btn fb-act";
    button.textContent = text; button.title = label; button.setAttribute("aria-label", label); button.addEventListener("click", run); return button;
  };
  const renderFilters = (): void => {
    filterList.replaceChildren();
    filterHint.hidden = filterGroups.length > 0; filterList.append(filterHint);
    filterGroups.forEach((group, index) => {
      const row = document.createElement("div"); row.className = "filter-line";
      if (index > 0) row.append(filterJoinControl(group.join, join => { group.join = join; refresh(); }, "条件组 " + (index + 1) + " 与上一组的连接方式"));
      const chips = document.createElement("div"); chips.className = "cond-group";
      const bracket = (text: string) => { const span = document.createElement("span"); span.className = "fb-bracket"; span.textContent = text; return span; };
      if (group.conditions.length > 1) chips.append(bracket("（"));
      group.conditions.forEach((state, offset) => {
        if (offset > 0) chips.append(filterJoinControl(state.join, join => { state.join = join; refresh(); }, "条件 " + state.seq + " 与上一条件的连接方式"));
        const chip = document.createElement("div"); chip.className = "cond-chip";
        const column = select(structure!.columns.map(col => ({ value: col.name, label: col.name })), state.column);
        column.style.removeProperty("width");
        column.className = "fb-txt"; column.setAttribute("aria-label", "条件 " + state.seq + " 字段"); column.title = "点击修改字段";
        const operator = select(FILTER_OPERATORS.map(value => ({ value, label: value })), state.op);
        operator.style.removeProperty("width");
        operator.className = "fb-txt fb-op-txt"; operator.setAttribute("aria-label", "条件 " + state.seq + " 运算符"); operator.title = "点击修改操作符";
        const valueControl = () => filterValueControl(state, "条件 " + state.seq + " 值", () => refresh());
        column.addEventListener("change", () => { state.column = column.value; refresh(); });
        operator.addEventListener("change", () => { state.op = operator.value as QueryFilterOperator; chip.querySelector(".fb-val")?.replaceWith(valueControl()); refresh(); });
        chip.append(column, operator, valueControl());
        const remove = action("×", "删除条件 " + state.seq, () => {
          group.conditions.splice(group.conditions.indexOf(state), 1);
          if (!group.conditions.length) filterGroups.splice(filterGroups.indexOf(group), 1);
          renderFilters(); refresh(); addFilter.focus();
        }); remove.classList.add("fb-cond-del"); chip.append(remove); chips.append(chip);
      });
      if (group.conditions.length > 1) chips.append(bracket("）"));
      row.append(chips, action("＋", "在条件组 " + (index + 1) + " 后增加下一行", () => addGroup(index)),
        action("()＋", "在条件组 " + (index + 1) + " 内追加条件", () => {
          if (conditionCount() >= MAX_FILTERS) { announce("最多 " + MAX_FILTERS + " 个过滤条件。", true); return; }
          const condition = newCondition(); group.conditions.push(condition); renderFilters(); refresh();
          filterList.querySelector<HTMLElement>('[aria-label="条件 ' + condition.seq + ' 字段"]')?.focus();
        }), action("✕", "删除条件组 " + (index + 1), () => { filterGroups.splice(index, 1); renderFilters(); refresh(); addFilter.focus(); }));
      filterList.append(row);
    });
  };
  const addGroup = (after = filterGroups.length - 1): void => {
    if (!structure) return;
    if (conditionCount() >= MAX_FILTERS) { announce("最多 " + MAX_FILTERS + " 个过滤条件。", true); return; }
    const condition = newCondition();
    filterGroups.splice(after + 1, 0, { join: filterGroups[after]?.join ?? "AND", conditions: [condition] });
    renderFilters(); refresh(); filterList.querySelector<HTMLElement>('[aria-label="条件 ' + condition.seq + ' 字段"]')?.focus();
  };
  addFilter.addEventListener("click", () => addGroup());

  addSort.addEventListener("click", () => {
    if (!structure) return;
    const seq = ++sortSequence;
    const columnSel = select(structure.columns.map(c => ({ value: c.name, label: c.name })), undefined, 170);
    columnSel.setAttribute("aria-label", `排序 ${seq} 字段`);
    const directionSel = select([{ value: "asc", label: "升序 ASC" }, { value: "desc", label: "降序 DESC" }], "asc", 120);
    directionSel.setAttribute("aria-label", `排序 ${seq} 方向`);
    const remove = toolButton("×"); remove.classList.add("qb-remove");
    remove.setAttribute("aria-label", `删除排序 ${seq}`);
    const row = document.createElement("div"); row.className = "qb-row";
    const state: SortRowState = { column: columnSel, direction: directionSel };
    columnSel.addEventListener("change", refresh);
    directionSel.addEventListener("change", refresh);
    remove.addEventListener("click", () => {
      const at = sortRows.indexOf(state);
      if (at >= 0) sortRows.splice(at, 1);
      row.remove();
      sortHint.hidden = sortRows.length > 0;
      refresh();
      addSort.focus();
    });
    row.append(columnSel, directionSel, remove);
    sortList.appendChild(row);
    sortRows.push(state);
    sortHint.hidden = true;
    refresh();
    columnSel.focus();
  });

  for (const control of [dialect]) control.addEventListener("change", refresh);
  limit.addEventListener("input", refresh);
  table.addEventListener("change", () => void loadTable());
  const doReload = (): void => { void (table.value ? loadTable() : loadTables()); };
  reloadTop.addEventListener("click", doReload);
  reloadFooter.addEventListener("click", doReload);

  // ---------- 元数据加载：连接 → 表列表 → 字段（保留可重试与状态条） ----------
  const loadTable = async (): Promise<void> => {
    const request = ++version, name = table.value;
    resetBuilder();
    if (!name || closed) return;
    announce("正在加载真实字段元数据…"); builder.setAttribute("aria-busy", "true");
    reloadTop.disabled = true; reloadFooter.disabled = true;
    try {
      const info = await source.getTableSchema(connection.id, schema, name);
      if (closed || request !== version || table.value !== name) return;
      if (info.table !== name || !info.columns.length) throw new Error("没有获得目标表的有效字段结构。");
      structure = info;
      renderFields(info);
      addFilter.disabled = false; addSort.disabled = false;
      refresh();
      announce("已加载字段。勾选输出、添加条件与排序将实时生成预览；插入仅修改当前选区 / 光标，不自动执行。自定义驱动请确认 SQL 方言。");
    } catch (e) { if (!closed && request === version) announce(`读取字段失败：${String(e)}。可重新加载。`, true); }
    finally { if (!closed && request === version) { reloadTop.disabled = false; reloadFooter.disabled = false; builder.removeAttribute("aria-busy"); } }
  };
  const loadTables = async (): Promise<void> => {
    const request = ++version;
    resetBuilder();
    table.replaceChildren(); table.disabled = true;
    reloadTop.disabled = true; reloadFooter.disabled = true;
    announce("正在连接并加载真实表 / 视图列表…");
    try {
      const reason = queryBuilderUnavailableReason(connection.kind);
      if (reason) throw new Error(reason);
      if (!schema) throw new Error(`请先在查询编辑器选择${namespaceName(connection)}。`);
      const connected = await source.ensureConnection(connection.id);
      if (!connected.ok) throw new Error(connected.message || "连接失败");
      if (closed || request !== version) return;
      const list = await source.listTables(connection.id, schema);
      if (closed || request !== version) return;
      for (const [name, type] of list) if (type === "table" || type === "view") table.appendChild(new Option(type === "view" ? `${name}（视图）` : name, name));
      if (!table.options.length) throw new Error(`当前${namespaceName(connection)}没有可查询的表或视图。`);
      table.disabled = false;
      await loadTable();
    } catch (e) { if (!closed && request === version) announce(`元数据加载失败：${String(e)}。可重新加载。`, true); }
    finally { if (!closed && request === version) { reloadTop.disabled = false; reloadFooter.disabled = false; } }
  };

  // ---------- 对话框组装 ----------
  const dlg = new Dialog({
    title: "查询创建工具", width: 820,
    render: body => { body.append(target, status, workspace); },
    buttons: [],
  });
  dlg.el.classList.add("query-builder-dialog");
  const style = document.createElement("style");
  style.textContent = QUERY_BUILDER_STYLE;
  dlg.el.appendChild(style); // 样式随对话框移除，且晚于 base.css 生效以覆盖按钮最小宽度。
  dlg.footer.append(reloadFooter, insert, cancel);
  reloadFooter.style.marginRight = "auto";
  accessibleToolDialog(dlg, "查询创建工具", () => { closed = true; ++version; }, previous);
  insert.addEventListener("click", () => {
    if (!generated || insert.disabled) return;
    try { onInsert(generated); dlg.close(); } catch (e) { announce(String(e), true); }
  });
  cancel.addEventListener("click", () => dlg.close());
  void loadTables();
  return dlg;
}
