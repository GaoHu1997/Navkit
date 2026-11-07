/** 真实元数据驱动的单表 SELECT 创建；仅预览和插入，不执行数据库语句。 */
import { api, parseDriverParams } from "../core/api";
import { buildSelectQuery, defaultQueryDialect, MAX_QUERY_LIMIT, QUERY_DIALECTS, queryBuilderUnavailableReason } from "../core/queryBuilder";
import type { QueryDialect, QueryFilter, QueryFilterOperator } from "../core/queryBuilder";
import type { ConnectionConfig, TableSchema } from "../core/types";
import { Dialog, field, numberInput, select, textInput } from "../ui/Dialog";
import { accessibleToolDialog, toolButton } from "./toolDialog";

export type QueryBuilderApi = Pick<typeof api, "ensureConnection" | "listTables" | "getTableSchema">;
export function openQueryBuilderDialog(
  connection: ConnectionConfig, schema: string, onInsert: (sql: string) => void, source: QueryBuilderApi = api,
): Dialog {
  const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const table = select([], undefined, 280); table.setAttribute("aria-label", "查询表或视图"); table.disabled = true;
  const dialect = select([{ value: "", label: "请选择目标 SQL 方言" }, ...QUERY_DIALECTS], parseDriverParams(connection.params) ? "" : defaultQueryDialect(connection.kind), 280);
  dialect.setAttribute("aria-label", "SQL 方言");
  const limit = numberInput(100, 100); limit.min = "1"; limit.max = String(MAX_QUERY_LIMIT); limit.step = "1"; limit.setAttribute("aria-label", "最大查询行数");
  const sort = select([{ value: "", label: "不排序" }], "", 200); sort.setAttribute("aria-label", "排序字段");
  const direction = select([{ value: "asc", label: "升序 ASC" }, { value: "desc", label: "降序 DESC" }], "asc", 110); direction.setAttribute("aria-label", "排序方向");
  const fields = document.createElement("fieldset"); fields.className = "query-builder-fields";
  fields.style.cssText = "border:1px solid var(--border);margin:10px 0;max-height:160px;overflow:auto;";
  const filterWrap = document.createElement("div"); filterWrap.className = "query-builder-filters";
  const status = document.createElement("div"); status.setAttribute("role", "status"); status.setAttribute("aria-live", "polite"); status.style.cssText = "min-height:24px;margin:8px 0;white-space:pre-wrap;";
  const preview = document.createElement("textarea"); preview.className = "query-builder-preview"; preview.readOnly = true;
  preview.setAttribute("aria-label", "待插入 SQL 预览"); preview.style.cssText = "width:100%;height:140px;resize:vertical;font-family:var(--mono);box-sizing:border-box;";
  const retry = toolButton("重新加载元数据"), addFilter = toolButton("添加 AND 条件"), generate = toolButton("生成 SQL 预览"), insert = toolButton("插入查询编辑器", true), close = toolButton("取消");
  addFilter.disabled = true; generate.disabled = true; insert.disabled = true;
  let closed = false, version = 0, filterSequence = 0, structure: TableSchema | null = null, generated = "";
  const columns: { name: string; input: HTMLInputElement }[] = [];
  const filters: { column: HTMLSelectElement; operator: HTMLSelectElement; kind: HTMLSelectElement; value: HTMLInputElement }[] = [];
  const invalidate = () => { generated = ""; preview.value = ""; insert.disabled = true; };
  const announce = (message: string, error = false) => { status.setAttribute("role", error ? "alert" : "status"); status.textContent = message; };
  const reset = () => {
    invalidate(); structure = null; columns.length = 0; filters.length = 0; fields.replaceChildren(); filterWrap.replaceChildren();
    sort.replaceChildren(new Option("不排序", "")); generate.disabled = true; addFilter.disabled = true;
  };
  const renderFields = () => {
    const legend = document.createElement("legend"); legend.textContent = "输出字段（至少一个）"; fields.appendChild(legend);
    for (const col of structure!.columns) {
      const label = document.createElement("label"); label.className = "checkbox"; label.style.cssText = "display:inline-flex;margin:4px 12px 4px 0;gap:5px;";
      const check = document.createElement("input"); check.type = "checkbox"; check.checked = true;
      check.setAttribute("aria-label", `输出字段 ${col.name}`); check.addEventListener("change", invalidate);
      const text = document.createElement("span"); text.textContent = `${col.name} (${col.type || "未声明类型"})`;
      label.append(check, text); fields.appendChild(label); columns.push({ name: col.name, input: check });
      sort.appendChild(new Option(col.name, col.name));
    }
  };
  const loadTable = async () => {
    const request = ++version, name = table.value; reset();
    if (!name || closed) return;
    announce("正在加载真实字段元数据…"); fields.setAttribute("aria-busy", "true"); retry.disabled = true;
    try {
      const info = await source.getTableSchema(connection.id, schema, name);
      if (closed || request !== version || table.value !== name) return;
      if (info.table !== name || !info.columns.length) throw new Error("没有获得目标表的有效字段结构。");
      structure = info; renderFields(); generate.disabled = false; addFilter.disabled = false;
      announce("选择输出字段、AND 过滤和排序后生成预览；插入仅修改当前选区 / 光标，不自动执行。自定义驱动请确认 SQL 方言。");
    } catch (e) { if (!closed && request === version) announce(`读取字段失败：${String(e)}。可重新加载。`, true); }
    finally { if (!closed && request === version) { retry.disabled = false; fields.setAttribute("aria-busy", "false"); } }
  };
  const loadTables = async () => {
    const request = ++version; reset(); table.replaceChildren(); table.disabled = true; retry.disabled = true;
    announce("正在连接并加载真实表 / 视图列表…");
    try {
      const reason = queryBuilderUnavailableReason(connection.kind);
      if (reason) throw new Error(reason);
      if (!schema) throw new Error("请先在查询编辑器选择数据库 / 模式。");
      const connected = await source.ensureConnection(connection.id);
      if (!connected.ok) throw new Error(connected.message || "连接失败");
      if (closed || request !== version) return;
      const list = await source.listTables(connection.id, schema);
      if (closed || request !== version) return;
      for (const [name, type] of list) if (type === "table" || type === "view") table.appendChild(new Option(type === "view" ? `${name}（视图）` : name, name));
      if (!table.options.length) throw new Error("当前模式没有可查询的表或视图。");
      table.disabled = false; await loadTable();
    } catch (e) { if (!closed && request === version) announce(`元数据加载失败：${String(e)}。可重新加载。`, true); }
    finally { if (!closed && request === version) retry.disabled = false; }
  };
  addFilter.addEventListener("click", () => {
    if (!structure || filters.length >= 20) { announce("最多 20 个过滤条件。", true); return; }
    invalidate();
    const row = document.createElement("div"); row.style.cssText = "display:flex;flex-wrap:wrap;gap:6px;margin:6px 0;";
    const index = ++filterSequence;
    const col = select(structure.columns.map(c => ({ value: c.name, label: c.name })), undefined, 145); col.setAttribute("aria-label", `条件 ${index} 字段`);
    const op = select(["=", "<>", ">", ">=", "<", "<=", "LIKE", "IS NULL", "IS NOT NULL"].map(value => ({ value, label: value })), "=", 115); op.setAttribute("aria-label", `条件 ${index} 运算符`);
    const kind = select([{ value: "text", label: "文本" }, { value: "number", label: "数字" }, { value: "boolean", label: "布尔" }], "text", 75); kind.setAttribute("aria-label", `条件 ${index} 值类型`);
    const value = textInput("", 160); value.maxLength = 8192; value.setAttribute("aria-label", `条件 ${index} 值`);
    const remove = toolButton("删除条件"); remove.setAttribute("aria-label", `删除条件 ${index}`);
    const config = { column: col, operator: op, kind, value }; filters.push(config);
    const sync = () => { value.disabled = kind.disabled = op.value === "IS NULL" || op.value === "IS NOT NULL"; invalidate(); };
    for (const control of [col, op, kind]) control.addEventListener("change", sync);
    value.addEventListener("input", invalidate);
    remove.addEventListener("click", () => { filters.splice(filters.indexOf(config), 1); row.remove(); invalidate(); addFilter.focus(); });
    row.append(col, op, kind, value, remove); filterWrap.appendChild(row); col.focus();
  });
  for (const control of [dialect, sort, direction]) control.addEventListener("change", invalidate);
  limit.addEventListener("input", invalidate);
  table.addEventListener("change", () => void loadTable());
  retry.addEventListener("click", () => void (table.value ? loadTable() : loadTables()));
  generate.addEventListener("click", () => {
    invalidate(); if (!structure) return;
    try {
      generated = buildSelectQuery(structure, {
        dialect: dialect.value as QueryDialect, schema, table: table.value, columns: columns.filter(c => c.input.checked).map(c => c.name),
        filters: filters.map(f => ({ column: f.column.value, operator: f.operator.value as QueryFilterOperator, valueKind: f.kind.value as QueryFilter["valueKind"], value: f.value.value })),
        orderBy: sort.value, descending: direction.value === "desc", limit: limit.valueAsNumber,
      });
      preview.value = generated; insert.disabled = false; announce("SQL 已生成，尚未执行。确认预览后插入编辑器。");
      dialect.removeAttribute("aria-invalid"); limit.removeAttribute("aria-invalid");
    } catch (e) { announce(String(e), true); dialect.setAttribute("aria-invalid", String(!dialect.value)); limit.setAttribute("aria-invalid", String(!limit.checkValidity())); }
  });
  const dlg = new Dialog({ title: "查询创建工具", width: 800, render: body => {
    const target = document.createElement("div"); target.textContent = `当前目标：${connection.name} / ${schema || "未选择模式"}`; target.style.marginBottom = "10px";
    const actions = document.createElement("div"); actions.style.cssText = "display:flex;gap:8px;margin:8px 0;"; actions.append(addFilter, generate);
    const sorting = document.createElement("div"); sorting.style.cssText = "display:flex;gap:8px;"; sorting.append(sort, direction);
    body.append(target, field("SQL 方言", dialect), field("表 / 视图", table), field("最大行数", limit), fields, field("排序", sorting), filterWrap, actions, status, preview);
  }, buttons: [] });
  dlg.el.classList.add("query-builder-dialog"); dlg.footer.append(retry, insert, close);
  accessibleToolDialog(dlg, "查询创建工具", () => { closed = true; ++version; }, previous);
  insert.addEventListener("click", () => {
    if (!generated || insert.disabled) return;
    try { onInsert(generated); dlg.close(); } catch (e) { announce(String(e), true); }
  });
  close.addEventListener("click", () => dlg.close());
  void loadTables();
  return dlg;
}
