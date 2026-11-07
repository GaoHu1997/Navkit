/** 数据生成对话框：设置 → 生成预览 → 用户明确写入，参数化事务由后端执行。 */
import { api } from "../core/api";
import { toolConnectionError } from "../core/toolCapabilities";
import { defaultGeneratorFields, generateDataPreview, generatorValueKind, MAX_GENERATED_ROWS } from "../core/dataGenerator";
import type { GeneratedRowsRequest, GeneratorField, GeneratorMode, GeneratorPlan } from "../core/dataGenerator";
import type { ConnectionConfig, TableSchema } from "../core/types";
import { Dialog, field, numberInput, select, textInput } from "../ui/Dialog";
import { accessibleToolDialog, toolButton, toolTable } from "./toolDialog";
import { Wizard } from "../ui/Wizard";

export interface DataGeneratorApi extends Pick<typeof api, "ensureConnection" | "listDatabases" | "listTables" | "getTableSchema"> {
  insertGeneratedRows(req: GeneratedRowsRequest): Promise<number>;
}
const defaultApi: DataGeneratorApi = {
  ensureConnection: api.ensureConnection,
  listDatabases: api.listDatabases,
  listTables: api.listTables,
  getTableSchema: api.getTableSchema,
  insertGeneratedRows: api.insertGeneratedRows,
};
export function openDataGeneratorDialog(
  connection: ConnectionConfig, schema: string, initialTable = "",
  onDone?: (rows: number, table: string) => void, source: DataGeneratorApi = defaultApi,
  /** 提供时在对话框内给出「连接」下拉（只列可写的原生协议连接），可换数据源。 */
  connections: ConnectionConfig[] = [],
): Dialog {
  const returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  // 连接可切换：App 原样传入当前选中的连接；它不受支持时下拉保持空选，
  // 由用户在可写的原生协议连接中自行挑选，绝不默认选中第一个。
  let conn = connection;
  const pickable = connections.filter((c) => toolConnectionError(c, true) === null);
  const connReady = () => !connSelect || pickable.some((c) => c.id === conn.id);
  const connOptions = pickable.map((c) => ({ value: c.id, label: `${c.name} (${c.kind})` }));
  const connSelect = pickable.length
    ? select(pickable.some((c) => c.id === conn.id) ? connOptions : [{ value: "", label: "请选择连接" }, ...connOptions], pickable.some((c) => c.id === conn.id) ? conn.id : "", 260)
    : null;
  if (connSelect) connSelect.setAttribute("aria-label", "连接");
  // 第一步标题：随所选连接变化（换连后原地更新）。
  const targetTitle = document.createElement("div");
  targetTitle.className = "wizard-panel-title";
  targetTitle.textContent = conn.name;
  const databases = select([], undefined, 260); databases.setAttribute("aria-label", "目标数据库 / 模式"); databases.disabled = true;
  const tables = select([], undefined, 260); tables.setAttribute("aria-label", "目标表"); tables.disabled = true;
  const count = numberInput(10, 90); count.min = "1"; count.max = String(MAX_GENERATED_ROWS); count.step = "1"; count.setAttribute("aria-label", "生成条数");
  const seed = textInput("sample", 180); seed.maxLength = 80; seed.setAttribute("aria-label", "文本种子");
  const previewButton = toolButton("生成预览"), executeButton = toolButton("执行写入", true), stop = toolButton("停止后续操作");
  executeButton.disabled = true; previewButton.disabled = true; stop.disabled = true;
  const status = document.createElement("div"); status.id = `generator-status-${crypto.randomUUID()}`;
  status.setAttribute("role", "status"); status.setAttribute("aria-live", "polite"); status.style.cssText = "margin:8px 0;min-height:24px;white-space:pre-wrap;";
  count.setAttribute("aria-describedby", status.id);
  const fieldsWrap = document.createElement("div"); fieldsWrap.style.cssText = "height:210px;overflow:auto;border:1px solid var(--border);margin:8px 0;";
  const previewWrap = document.createElement("div"); previewWrap.style.cssText = "height:170px;overflow:auto;border:1px solid var(--border);";
  const confirmation = document.createElement("label"); confirmation.className = "checkbox"; confirmation.style.cssText = "display:flex;gap:8px;margin-top:10px;";
  const confirm = document.createElement("input"); confirm.type = "checkbox";
  const confirmText = document.createElement("span"); confirmText.textContent = "我确认将预览中的数据真实插入目标表（不会覆盖旧行）"; confirmation.append(confirm, confirmText);
  let structure: TableSchema | null = null, plan: GeneratorPlan | null = null, settings: GeneratorField[] = [], version = 0, closed = false, writing = false;
  const fieldControls: (HTMLSelectElement | HTMLInputElement)[] = [];
  let capabilityError = toolConnectionError(conn, true);
  let writable = capabilityError === null;
  const invalidate = () => { plan = null; confirm.checked = false; executeButton.disabled = true; previewWrap.replaceChildren(); };
  const setBusy = (busy: boolean) => {
    if (connSelect) connSelect.disabled = busy;
    databases.disabled = busy; tables.disabled = busy; count.disabled = busy; seed.disabled = busy; previewButton.disabled = busy || !structure || !writable; confirm.disabled = busy;
    for (const control of fieldControls) control.disabled = busy;
    stop.disabled = !busy; fieldsWrap.setAttribute("aria-busy", String(busy));
  };
  const renderFields = () => {
    fieldsWrap.replaceChildren(); fieldControls.length = 0;
    if (!structure) return;
    const table = toolTable(["字段 / 类型", "约束", "生成方式", "固定值 / 起始值"], []);
    const body = table.tBodies[0];
    for (const c of structure.columns) {
      const config = settings.find(s => s.name === c.name)!;
      const row = body.insertRow(); row.insertCell().textContent = `${c.name} · ${c.type || "未声明类型"}`;
      row.insertCell().textContent = [c.primary_key ? "主键" : "", !c.nullable || c.primary_key ? "非空" : "", c.auto_increment ? "自增/标识" : "", structure.foreign_keys.some(f => f.columns.includes(c.name)) ? "外键" : "", c.default !== null ? `默认 ${c.default}` : ""].filter(Boolean).join(" · ") || "可为空";
      const modes: { value: string; label: string }[] = [{ value: "auto", label: "按类型生成" }, { value: "constant", label: "固定值" }];
      if (generatorValueKind(c) === "integer") modes.push({ value: "sequence", label: "顺序整数" });
      if (c.nullable && !c.primary_key) modes.push({ value: "null", label: "NULL" });
      if (c.auto_increment || c.default !== null || (c.nullable && !c.primary_key)) modes.push({ value: "omit", label: "省略（数据库默认）" });
      const mode = select(c.auto_increment ? [{ value: "omit", label: "由数据库生成" }] : modes, config.mode, 150); mode.setAttribute("aria-label", `${c.name} 生成方式`);
      const value = textInput(config.value, 160); value.setAttribute("aria-label", `${c.name} 固定值或顺序起始值`); value.maxLength = 4096;
      const sync = () => { config.mode = mode.value as GeneratorMode; config.value = value.value; value.readOnly = !["constant", "sequence"].includes(mode.value); invalidate(); };
      value.readOnly = !["constant", "sequence"].includes(mode.value);
      mode.addEventListener("change", sync); value.addEventListener("input", sync);
      row.insertCell().appendChild(mode); row.insertCell().appendChild(value); fieldControls.push(mode, value);
      for (const cell of row.cells) cell.style.cssText = "padding:5px 8px;border-bottom:1px solid var(--border);vertical-align:top;";
    }
    fieldsWrap.appendChild(table);
  };
  const loadTable = async () => {
    const request = ++version; invalidate(); structure = null; fieldsWrap.replaceChildren(); previewButton.disabled = true;
    if (!tables.value) return;
    status.textContent = "正在读取真实表结构…"; fieldsWrap.setAttribute("aria-busy", "true");
    try {
      const info = await source.getTableSchema(conn.id, schema, tables.value);
      if (closed || request !== version) return;
      structure = info; settings = defaultGeneratorFields(info); renderFields(); previewButton.disabled = !writable;
      status.textContent = writable ? "先设置字段与条数，再生成预览。外键请填写已有引用；主键起始值须避开已存在的键。" : conn.read_only ? "此连接只读，禁止生成写入。" : "此协议尚不支持安全事务批量写入（仅 SQLite / MySQL / PostgreSQL）。";
    } catch (e) { if (!closed && request === version) { status.textContent = `读取表结构失败：${String(e)}`; status.setAttribute("role", "alert"); } }
    finally { if (!closed && request === version) fieldsWrap.setAttribute("aria-busy", "false"); }
  };
  const wizard = new Wizard({
      steps: [
        {
          label: "目标与行数",
          hint: "选择要写入的表、生成条数与随机种子。相同种子会得到相同的随机数据。",
          render: (host) => {
            host.append(
              targetTitle,
              ...(connSelect ? [field("连接:", connSelect)] : []),
              field("数据库 / 模式", databases),
              field("目标表", tables),
              field(`条数（1–${MAX_GENERATED_ROWS}）`, count),
              field("文本种子", seed),
            );
            const note = document.createElement("div");
            note.className = "wizard-panel-note";
            note.textContent = "文本种子用于让生成的随机数据可复现；相同种子 + 相同表结构会得到相同结果。";
            host.appendChild(note);
          },
          validate: () => connReady()
            ? capabilityError ?? (!schema || !tables.value ? "请选择目标数据库与数据表。" : !structure ? "表结构尚未加载成功，请稍候或重新选择目标表。" : null)
            : "请先在「连接」中选择支持写入的连接。",
        },
        {
          label: "字段生成规则",
          hint: "可以逐列决定生成方式：按类型自动、固定值、顺序整数、NULL 或交给数据库默认值。",
          render: (host) => {
            const head = document.createElement("div");
            head.className = "wizard-panel-title";
            head.textContent = "为每个字段选择生成方式";
            fieldsWrap.style.height = "auto";
            fieldsWrap.style.flex = "1";
            fieldsWrap.style.minHeight = "0";
            host.append(head, fieldsWrap);
            const note = document.createElement("div");
            note.className = "wizard-panel-note";
            note.textContent = "自动值不保证满足任意 CHECK/外键约束；外键请填写已存在的引用，主键起始值需避开已有键。";
            host.appendChild(note);
          },
        },
        {
          label: "预览与写入",
          hint: "先预览再写入。预览不会碰数据库；确认后整批在事务中写入，失败会全部回滚。",
          render: (host) => {
            const head = document.createElement("div");
            head.className = "wizard-panel-title";
            head.textContent = "预览（最多显示 20 行，未写入数据库）";
            previewWrap.style.height = "auto";
            previewWrap.style.flex = "1";
            previewWrap.style.minHeight = "0";
            const actions = document.createElement("div");
            actions.style.cssText = "display:flex;gap:8px;margin-bottom:8px;";
            actions.append(previewButton, executeButton, stop);
            host.append(head, actions, status, previewWrap, confirmation);
            const note = document.createElement("div");
            note.className = "wizard-panel-note";
            note.textContent = "预览不写数据库；执行使用整个预览批次。写入使用参数绑定与事务，约束失败全部回滚。";
            host.appendChild(note);
          },
          validate: () => {
            if (!plan) return "请先生成预览，再勾选确认后执行写入。";
            if (!confirm.checked) return "请勾选确认，表示你同意将预览数据真实插入目标表。";
            return null;
          },
        },
      ],
      onEnter: (index) => {
        // 进入「字段规则」步时若结构尚未就绪，先等待加载
        if (index === 1 && !structure && tables.value) void loadTable();
      },
      finishLabel: "执行写入",
      onFinish: async () => {
        if (!writable || writing || !plan || !confirm.checked) return;
        const submitted = plan;
        const table = tables.value;
        writing = true;
        executeButton.disabled = true;
        setBusy(true);
        wizard.setBusy(true);
        status.textContent = `正在向 ${schema}.${table} 写入 ${submitted.rows.length} 行…`;
        try {
          const rows = await source.insertGeneratedRows({
            connection_id: conn.id,
            schema,
            table,
            columns: submitted.columns,
            rows: submitted.rows,
          });
          if (onDone) {
            try {
              onDone(rows, table);
            } catch (e) {
              console.error("数据生成完成回调失败", e);
            }
          }
          if (!closed) {
            invalidate();
            status.setAttribute("role", "status");
            status.textContent = `写入完成：实际影响 ${rows} 行。再次写入必须重新生成预览。`;
            wizard.clearError();
          }
        } catch (e) {
          if (!closed) {
            invalidate();
            status.setAttribute("role", "alert");
            const msg = `写入失败或未收到提交确认：${e instanceof Error ? e.message : String(e)}。数据库约束错误会整批回滚；通信异常时请先刷新表确认结果，勿直接重复提交。`;
            status.textContent = msg;
            wizard.showError(msg);
          }
        } finally {
          writing = false;
          if (!closed) {
            setBusy(false);
            wizard.setBusy(false);
          }
        }
      },
  });
  const showError = (message: string) => wizard.showError(message);
  const dlg = new Dialog({ title: "数据生成", width: 920, height: 620, render: body => {
    body.style.padding = "0";
    body.style.display = "flex";
    body.style.flexDirection = "column";
    body.style.overflow = "hidden";
    body.appendChild(wizard.stepsEl);
    body.appendChild(wizard.bodyEl);
  }, buttons: [] });
  dlg.footer.append(...wizard.buttons(), (() => {
    const close = document.createElement("button");
    close.type = "button";
    close.className = "btn";
    close.textContent = "关闭";
    close.addEventListener("click", () => dlg.close());
    return close;
  })());
  accessibleToolDialog(dlg, "数据生成", () => { closed = true; version++; }, returnFocus);
  tables.addEventListener("change", () => void loadTable()); count.addEventListener("input", invalidate); seed.addEventListener("input", invalidate);
  confirm.addEventListener("change", () => { executeButton.disabled = !confirm.checked || !plan || writing || !writable; });
  previewButton.addEventListener("click", () => {
    invalidate(); if (!structure) return;
    try {
      plan = generateDataPreview(structure, settings, count.valueAsNumber, seed.value);
      previewWrap.appendChild(toolTable(plan.columns.length ? plan.columns : ["数据库默认值"], plan.columns.length ? plan.rows.slice(0, 20) : plan.rows.slice(0, 20).map(() => ["整行使用数据库默认"] )));
      status.setAttribute("role", "status"); status.textContent = `预览已生成 ${plan.rows.length} 行（显示前 ${Math.min(20, plan.rows.length)} 行）。尚未写入；请勾选确认后点击“执行写入”。`;
      count.removeAttribute("aria-invalid");
    } catch (e) { status.setAttribute("role", "alert"); status.textContent = e instanceof Error ? e.message : String(e); count.setAttribute("aria-invalid", String(!count.checkValidity())); }
  });
  stop.addEventListener("click", () => { invalidate(); status.textContent = "已禁止再次提交。当前事务若已发送，将继续完成或回滚；关闭窗口也不会取消该事务。"; });
  executeButton.addEventListener("click", () => { void wizard.next(); });
  const loadTables = async () => {
    const request = ++version;
    invalidate(); structure = null; fieldsWrap.replaceChildren(); tables.replaceChildren(); tables.disabled = true; previewButton.disabled = true;
    schema = databases.value;
    if (!schema) return;
    status.textContent = "正在加载表列表…";
    try {
      const list = await source.listTables(conn.id, schema);
      if (closed || request !== version) return;
      for (const [name, type] of list) if (type === "table") { const option = document.createElement("option"); option.value = name; option.textContent = name; tables.appendChild(option); }
      if (!tables.options.length) throw new Error("该数据库没有可写入的表（视图不支持数据生成）。");
      // 预填表名只对最初传入的连接生效，换连后不借用别的连接的表。
      if (conn.id === connection.id && initialTable && [...tables.options].some(o => o.value === initialTable)) tables.value = initialTable;
      tables.disabled = false; wizard.clearError(); await loadTable();
    } catch (e) { if (!closed && request === version) { status.textContent = `加载表列表失败：${String(e)}`; status.setAttribute("role", "alert"); showError(status.textContent); } }
  };
  databases.addEventListener("change", () => { if (!writing) void loadTables(); });
  /** 连接就绪后加载库/表；run 计号让换连后仍在途的旧请求作废。 */
  const initConnection = async () => {
    if (capabilityError) { status.textContent = capabilityError; showError(capabilityError); return; }
    const run = ++version;
    status.textContent = "正在连接并读取数据库列表…";
    try {
      const connected = await source.ensureConnection(conn.id);
      if (closed || run !== version) return;
      if (!connected.ok) throw new Error(connected.message || "连接失败");
      const list = await source.listDatabases(conn.id);
      if (closed || run !== version) return;
      databases.replaceChildren(...list.map(([name]) => new Option(name, name)));
      if (!databases.options.length) throw new Error("此连接没有可用数据库/模式");
      if (schema && !list.some(([name]) => name === schema)) throw new Error(`目标数据库/模式「${schema}」不存在，请重新选择`);
      if (schema) databases.value = schema;
      databases.disabled = false;
      await loadTables();
    } catch (e) { if (!closed && run === version) { databases.disabled = !databases.options.length; status.textContent = `加载数据库失败：${String(e)}`; status.setAttribute("role", "alert"); showError(status.textContent); } }
  };
  // 当前数据源不支持时保持空选：不默认选中第一个连接，也不发任何请求，
  // 等用户在「连接」下拉里自己选定后再加载。
  if (connReady()) void initConnection();
  else status.textContent = "当前连接不支持数据生成，请先在「连接」中选择可写入的连接。";
  connSelect?.addEventListener("change", () => {
    const next = pickable.find((c) => c.id === connSelect.value);
    if (writing || !next || next.id === conn.id) return;
    conn = next;
    capabilityError = toolConnectionError(conn, true);
    writable = capabilityError === null;
    schema = ""; structure = null; plan = null; settings = [];
    confirm.checked = false; executeButton.disabled = true; previewButton.disabled = true;
    previewWrap.replaceChildren(); fieldsWrap.replaceChildren();
    databases.replaceChildren(); databases.disabled = true;
    tables.replaceChildren(); tables.disabled = true;
    wizard.clearError();
    targetTitle.textContent = conn.name;
    void initConnection();
  });
  return dlg;
}
