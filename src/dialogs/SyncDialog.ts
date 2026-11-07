/** 表对表与整库（库到库）同步向导：映射、建表与进度反馈。 */
import { api, syncApi, transferTasks, type DbSyncOptions, type SyncOptions, type SyncReport } from "../core/api";
import { syncConnectionError } from "../core/toolCapabilities";
import type { ConnectionConfig } from "../core/types";
import { Dialog, field, select, textInput } from "../ui/Dialog";
import { Wizard } from "../ui/Wizard";

/** Task feedback reused by the import/export wizard. */
export function taskFeedback(dlg: Dialog, status: HTMLElement) {
  let taskId: string | null = null;
  let timer: ReturnType<typeof setInterval> | null = null;
  let polling = false;
  let generation = 0;
  const cancel = document.createElement("button");
  cancel.className = "btn";
  cancel.textContent = "取消任务";
  cancel.disabled = true;
  // 仅在任务运行中出现：平时隐藏，避免占位把「上一步」挤到右侧
  cancel.hidden = true;
  const progress = document.createElement("progress");
  progress.setAttribute("aria-label", "任务进度");
  progress.style.width = "100%";
  progress.hidden = true;
  status.setAttribute("role", "status");
  status.setAttribute("aria-live", "polite");
  status.style.whiteSpace = "pre-wrap";
  dlg.body.append(progress);
  // 页脚布局：取消任务独占最左（靠 margin-right:auto 推开其余按钮），默认隐藏。
  cancel.style.marginRight = "auto";
  dlg.footer.insertBefore(cancel, dlg.footer.firstChild);
  const stop = () => { if (timer) clearInterval(timer); timer = null; };
  let disabledControls: (HTMLButtonElement | HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement)[] = [];
  cancel.addEventListener("click", async () => {
    if (!taskId) return;
    cancel.disabled = true;
    status.textContent = "正在取消，等待当前数据库操作完成…";
    try { await transferTasks.cancel(taskId); } catch (e) { status.textContent = String(e); cancel.disabled = false; }
  });
  dlg.onClose(() => { generation++; stop(); if (taskId) void transferTasks.cancel(taskId).catch(() => undefined); });
  return {
    begin() {
      generation++;
      stop();
      taskId = crypto.randomUUID();
      disabledControls = [...dlg.body.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement | HTMLButtonElement>("input:not(:disabled),select:not(:disabled),textarea:not(:disabled),button:not(:disabled)")];
      disabledControls.forEach(c => c.disabled = true);
      status.textContent = "正在启动…";
      cancel.hidden = false;
      cancel.disabled = false;
      progress.hidden = false;
      progress.removeAttribute("value");
      dlg.body.setAttribute("aria-busy", "true");
      timer = setInterval(async () => {
        if (!taskId || polling) return;
        polling = true;
        const currentId = taskId;
        const currentGeneration = generation;
        try {
          const p = await transferTasks.progress(currentId);
          if (currentGeneration !== generation || currentId !== taskId) return;
          status.textContent = `${p.message}${p.total ? `\n${p.processed} / ${p.total} 行` : ""}`;
          if (p.total) { progress.max = p.total; progress.value = p.processed; }
          if (p.status !== "running") stop();
        } catch { /* First poll may race with task registration; execution reports final errors. */ }
        finally { polling = false; }
      }, 250);
      return taskId;
    },
    finish() { generation++; stop(); taskId = null; cancel.hidden = true; cancel.disabled = true; disabledControls.forEach(c => c.disabled = false); disabledControls = []; dlg.body.removeAttribute("aria-busy"); progress.hidden = true; },
  };
}

export function accessibleDialog(dlg: Dialog, title: string) {
  const previous = document.activeElement as HTMLElement | null;
  dlg.el.setAttribute("role", "dialog"); dlg.el.setAttribute("aria-modal", "true"); dlg.el.setAttribute("aria-label", title);
  dlg.el.style.maxWidth = "calc(100vw - 32px)";
  dlg.body.style.maxHeight = "70vh"; dlg.body.style.overflowY = "auto";
  dlg.body.querySelectorAll<HTMLElement>(".field").forEach(row => {
    const label = row.querySelector(".field-label")?.textContent?.replace(/[:：]$/, "") ?? "选项";
    row.querySelectorAll<HTMLElement>("input,select,textarea").forEach(control => { if (!control.hasAttribute("aria-label")) control.setAttribute("aria-label", label); });
  });
  const close = dlg.el.querySelector<HTMLElement>(".dialog-title-close");
  if (close) { close.tabIndex = 0; close.setAttribute("role", "button"); close.setAttribute("aria-label", "关闭"); close.addEventListener("keydown", e => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); dlg.close(); } }); }
  dlg.el.addEventListener("keydown", e => {
    if (e.key === "Escape") { e.preventDefault(); dlg.close(); previous?.focus(); }
    if (e.key !== "Tab") return;
    const nodes = [...dlg.el.querySelectorAll<HTMLElement>("button:not(:disabled),input:not(:disabled),select:not(:disabled),textarea:not(:disabled),[tabindex='0']")].filter(n => n.getClientRects().length > 0);
    if (!nodes.length) return;
    if (e.shiftKey && document.activeElement === nodes[0]) { e.preventDefault(); nodes[nodes.length - 1]?.focus(); }
    else if (!e.shiftKey && document.activeElement === nodes[nodes.length - 1]) { e.preventDefault(); nodes[0].focus(); }
  });
}

const DB_WRITE_MODES = [
  { value: "skip", label: "跳过已存在的同名表（推荐）" },
  { value: "append", label: "追加数据 / 保留已有结构" },
  { value: "replace", label: "清空同名表后写入（数据事务）" },
  { value: "recreate", label: "重建同名表（删除原表）" },
] as const;

export function openSyncDialog(
  connections: ConnectionConfig[],
  source: ConnectionConfig | undefined,
  prefill: { schema: string; table: string } = { schema: "", table: "" },
  onDone: (report: SyncReport) => void = () => undefined,
): Dialog {
  const supported = connections.filter(c => syncConnectionError(c) === null);
  if (!supported.length) throw new Error("没有支持数据同步的连接：请先添加 SQLite / MySQL / PostgreSQL / SQL Server / ClickHouse 或 ODBC 关系型连接");
  const targets = supported.filter(c => !c.read_only);
  if (!targets.length) throw new Error("没有可写的受支持目标连接，请先添加连接或取消目标只读设置");
  // 全局工具入口不再按当前选中库禁用：当前数据源不支持同步时对话框照常打开，
  // 但源/目标保持空选（不默认选中第一个连接），由用户在受支持范围内自行选择。
  const initialSource = source && supported.some(c => c.id === source.id) ? source : undefined;
  const sourceHint = source && !initialSource ? syncConnectionError(source) : null;
  const status = document.createElement("div");
  status.style.marginTop = "10px"; status.style.minHeight = "36px";
  if (sourceHint) status.textContent = `${sourceHint}。请直接在下方选择受支持的数据源。`;
  let busy = false;
  let dlg: Dialog;
  let wizard!: Wizard;
  // 同步方式：表对表 / 整库（库到库）
  const syncKind = select([{ value: "table", label: "表对表同步" }, { value: "database", label: "整库同步（库到库）" }], "table", 240);
  syncKind.setAttribute("aria-label", "同步方式");
  const mode = select([{ value: "both", label: "同步结构与数据" }, { value: "structure", label: "仅同步结构" }, { value: "data", label: "仅同步数据" }], "both", 240);
  const dbWriteMode = select(DB_WRITE_MODES.map(o => ({ value: o.value, label: o.label })), "skip", 280);
  const writeMode = select([{ value: "append", label: "追加数据 / 保留已有结构" }, { value: "replace", label: "清空后写入（数据事务）" }, { value: "recreate", label: "重建目标表（删除原表）" }], "append", 280);
  // 同步方式：全量（按写入模式处理全部数据）/ 增量（按主键只插入目标缺失的行）
  const syncStrategy = select([{ value: "full", label: "全量同步" }, { value: "incremental", label: "增量同步（按主键，仅插入目标缺失的行）" }], "full", 320);
  syncStrategy.setAttribute("aria-label", "同步方式");
  const incrUpdate = document.createElement("input"); incrUpdate.type = "checkbox";
  const incrUpdateLabel = document.createElement("label"); incrUpdateLabel.className = "checkbox";
  incrUpdateLabel.append(incrUpdate, "同时更新主键相同但内容不同的行（归一化近似比较）");
  const where = textInput("", 360);
  const create = document.createElement("input"); create.type = "checkbox"; create.checked = true;
  const createLabel = document.createElement("label"); createLabel.className = "checkbox"; createLabel.append(create, "目标表不存在时自动建表（含主键/索引）");
  const mapping = document.createElement("table"); mapping.className = "mini-table";
  let mappings: { source: string; target: HTMLInputElement; type: HTMLInputElement }[] = [];
  let mapVersion = 0;
  let mapLoading = false;
  // 整库模式第 3 步的表清单
  let tableSelection: { source: string; kind: "table" | "view"; target: HTMLInputElement; checked: HTMLInputElement }[] = [];
  let tablesLoading = false;
  let tablesVersion = 0;
  const error = (e: unknown) => { status.textContent = String(e); dlg?.showError(String(e)); };
  const isDbMode = () => syncKind.value === "database";
  function side(title: string, connectionId: string, initialSchema: string, initialTable: string, target: boolean) {
    const pool = (target ? targets : supported).map(c => ({ value: c.id, label: `${c.name} (${c.kind})` }));
    // 无预选（当前数据源不支持同步）时加占位项，保持空选而不是默认选中第一个连接。
    const conn = select(pool.some(o => o.value === connectionId) ? pool : [{ value: "", label: "请选择连接" }, ...pool], connectionId, 260);
    const schema = select([], "", 260);
    const table = target ? textInput(initialTable, 260) : select([], "", 260);
    const tableField = field("表:", table);
    let version = 0;
    const box = document.createElement("fieldset"); box.className = "groupbox sync-side";
    const legend = document.createElement("legend");
    legend.className = "sync-side-legend";
    legend.innerHTML = `<span class="sync-side-badge${target ? " target" : ""}">${target ? "目标" : "源"}</span><span class="sync-side-title">${target ? (isDbMode() ? "目标库" : title) : (isDbMode() ? "源库" : title)}</span>`;
    box.append(legend, field("连接:", conn), field("数据库 / 模式:", schema), tableField);
    const hint = document.createElement("div");
    hint.className = "wizard-panel-note";
    hint.style.margin = "4px 0 0";
    if (target) box.append(hint);
    if (target) {
      const list = document.createElement("datalist"); list.id = `sync-tables-${crypto.randomUUID()}`;
      (table as HTMLInputElement).setAttribute("list", list.id); box.append(list);
      table.setAttribute("aria-label", "目标表（可输入新表名，不存在时自动创建）");
    }
    async function tables(v = ++version) {
      try {
        const rows = await api.listTables(conn.value, schema.value);
        if (v !== version) return;
        if (target) {
          const list = box.querySelector("datalist")!; list.replaceChildren(...rows.filter(([, ty]) => ty === "table").map(([n]) => new Option(n, n)));
          updateTargetHint();
        } else {
          const sel = table as HTMLSelectElement; sel.replaceChildren(...rows.filter(([, ty]) => ty === "table").map(([n]) => new Option(n, n)));
          sel.value = rows.some(([n]) => n === initialTable) ? initialTable : sel.options[0]?.value ?? "";
        }
        await refreshMapping();
      } catch (e) { if (v === version) error(e); }
    }
    function updateTargetHint() {
      if (!target) return;
      const value = (table as HTMLInputElement).value.trim();
      if (!value) { hint.textContent = "填写目标表名；表不存在时将按源结构自动创建。"; return; }
      const options = [...box.querySelectorAll<HTMLOptionElement>("datalist option")].map(o => o.value);
      hint.textContent = options.some(n => n === value)
        ? "目标表已存在：将保留现有结构，按下一步所选写入模式处理。"
        : "目标表不存在：将按源结构自动创建（含主键/索引，不复制外键）。";
    }
    async function databases() {
      const v = ++version;
      // 尚未选择连接（空选占位）时不发起请求，等用户选定后再加载。
      if (!conn.value) { schema.replaceChildren(); if (!target) (table as HTMLSelectElement).replaceChildren(); return; }
      conn.disabled = schema.disabled = true;
      try {
        const connected = await api.ensureConnection(conn.value);
        if (!connected.ok) throw new Error(connected.message || "连接失败");
        const dbs = await api.listDatabases(conn.value);
        if (v !== version) return;
        schema.replaceChildren(...dbs.map(([n]) => new Option(n, n)));
        schema.value = dbs.some(([n]) => n === initialSchema) ? initialSchema : schema.options[0]?.value ?? "";
        if (!schema.value) throw new Error("该连接没有可用数据库/模式");
        legend.innerHTML = `<span class="sync-side-badge${target ? " target" : ""}">${target ? "目标" : "源"}</span><span class="sync-side-title">${target ? (isDbMode() ? "目标库" : title) : (isDbMode() ? "源库" : title)}</span>`;
        await tables(v);
      } catch (e) { if (v === version) error(e); }
      finally { if (v === version) conn.disabled = schema.disabled = false; }
    }
    conn.addEventListener("change", () => void databases()); schema.addEventListener("change", () => void tables());
    table.addEventListener("input", () => { updateTargetHint(); void refreshMapping(); });
    table.addEventListener("change", () => { updateTargetHint(); void refreshMapping(); });
    return { box, conn, schema, table, tableField, hint, load: databases };
  }
  const src = side("来源表", initialSource?.id ?? "", initialSource ? prefill.schema : "", initialSource ? prefill.table : "", false);
  const dst = side("写入表", initialSource ? (supported.find(c => c.id !== initialSource.id && !c.read_only)?.id ?? initialSource.id) : "", "", initialSource ? prefill.table : "", true);
  // 源表变化时，目标表默认跟随同名（用户一旦手动修改目标表则不再跟随）
  let dstTouched = false;
  (dst.table as HTMLInputElement).addEventListener("input", () => { dstTouched = true; });
  (src.table as HTMLSelectElement).addEventListener("change", () => {
    if (!dstTouched) (dst.table as HTMLInputElement).value = (src.table as HTMLSelectElement).value || "";
    dst.hint.textContent = "";
    (dst.table as HTMLInputElement).dispatchEvent(new Event("input"));
  });
  const modeField = field("同步内容:", mode);
  const dbWriteField = field("目标同名表处理:", dbWriteMode);
  const tableWriteField = field("写入模式:", writeMode);
  const strategyField = field("同步方式:", syncStrategy);
  /** 同步方式联动：仅结构模式强制全量；增量强制追加写入并约束整库的清空/重建组合。 */
  function syncStrategyUi() {
    if (syncStrategy.value === "incremental" && mode.value === "structure") syncStrategy.value = "full";
    const incr = syncStrategy.value === "incremental";
    incrUpdateLabel.style.display = incr && mode.value !== "structure" ? "" : "none";
    writeMode.disabled = incr;
    dbWriteMode.disabled = incr;
    if (incr) {
      if (writeMode.value !== "append") writeMode.value = "append";
      if (isDbMode() && (dbWriteMode.value === "replace" || dbWriteMode.value === "recreate")) dbWriteMode.value = "append";
    }
  }
  syncStrategy.addEventListener("change", syncStrategyUi);
  function updateKindUi() {
    const db = isDbMode();
    // 表对表 / 整库：表选择行与写入选项的显隐
    src.tableField.style.display = db ? "none" : "";
    dst.tableField.style.display = db ? "none" : "";
    createLabel.style.display = db ? "none" : "";
    dbWriteField.style.display = db ? "" : "none";
    tableWriteField.style.display = db ? "" : "none";
    // 同步内容选项：整库固定包含结构（目标表不存在时建表），不提供“仅数据”
    const modeValue = mode.value;
    mode.replaceChildren();
    for (const [value, label] of db ? [["both", "同步结构与数据"], ["structure", "仅同步结构"]] : [["both", "同步结构与数据"], ["structure", "仅同步结构"], ["data", "仅同步数据"]] as const) {
      mode.append(new Option(label, value));
    }
    mode.value = db ? (modeValue === "structure" ? "structure" : "both") : modeValue;
    create.disabled = mode.value === "data";
    mappings.forEach(m => m.type.disabled = mode.value === "data");
    if ((mode.value === "data" && writeMode.value === "recreate") || (mode.value === "structure" && writeMode.value === "replace")) writeMode.value = "append";
    if (db) dst.hint.textContent = "整库同步将按第 3 步所选的表逐表写入；目标同名表按下一步设置处理。";
    syncStrategyUi();
  }
  syncKind.addEventListener("change", updateKindUi);
  // 同步内容变化：联动建表勾选/建表类型输入与写入模式冲突处理
  mode.addEventListener("change", () => {
    create.disabled = mode.value === "data";
    mappings.forEach(m => m.type.disabled = mode.value === "data");
    if (isDbMode()) {
      if (mode.value === "structure" && dbWriteMode.value === "replace") dbWriteMode.value = "skip";
    } else if ((mode.value === "data" && writeMode.value === "recreate") || (mode.value === "structure" && writeMode.value === "replace")) {
      writeMode.value = "append";
    }
    syncStrategyUi();
  });
  dbWriteMode.addEventListener("change", () => {
    if (isDbMode() && mode.value === "structure" && dbWriteMode.value === "replace") dbWriteMode.value = "skip";
  });
  async function refreshMapping() {
    if (isDbMode()) return;
    if (!src.table.value || !dst.conn.value) return;
    const v = ++mapVersion; mapLoading = true;
    try {
      const types = await syncApi.typeMapping(src.conn.value, src.schema.value, src.table.value, dst.conn.value);
      if (v !== mapVersion) return;
      mappings = [];
      const head = document.createElement("tr"); for (const title of ["源列 / 类型", "目标列", "目标建表类型"]) { const th = document.createElement("th"); th.textContent = title; head.append(th); }
      const tbody = document.createElement("tbody"); mapping.replaceChildren(head, tbody);
      for (const t of types) {
        const row = document.createElement("tr"); const label = document.createElement("td"); label.textContent = `${t.source} · ${t.source_type}`;
        const target = textInput(t.source, 160); target.setAttribute("aria-label", `${t.source} 对应目标列（留空跳过）`);
        const type = textInput(t.target_type, 160); type.setAttribute("aria-label", `${t.source} 目标建表类型`); type.disabled = mode.value === "data";
        const a = document.createElement("td"); a.append(target); const b = document.createElement("td"); b.append(type); row.append(label, a, b); tbody.append(row); mappings.push({ source: t.source, target, type });
      }
    } catch (e) { if (v === mapVersion) { mapping.replaceChildren(); mappings = []; error(e); } }
    finally { if (v === mapVersion) mapLoading = false; }
  }
  /** 表对表：比较源表与目标表的差异（只读），结果写入状态区。 */
  async function runDiff() {
    const targetTable = (dst.table as HTMLInputElement).value.trim();
    if (!src.conn.value || !src.table.value || !dst.conn.value || !dst.schema.value || !targetTable) {
      status.textContent = "请先在第一步选择源表并填写目标表名，再比较差异。";
      return;
    }
    status.textContent = "正在比较源表与目标表差异…";
    try {
      const report = await syncApi.diff({
        source_connection: src.conn.value, source_schema: src.schema.value, source_table: src.table.value,
        target_connection: dst.conn.value, target_schema: dst.schema.value, target_table: targetTable,
        where_clause: where.value.trim() || null,
      });
      const parts = [`源 ${report.source_rows} 行 / 目标 ${report.target_rows} 行`];
      if (report.keyed) {
        parts.push(`目标缺失 ${report.missing_in_target} 行`, `目标多出 ${report.extra_in_target} 行`, `内容差异 ${report.changed} 行（归一化近似）`);
      }
      if (report.target_missing_columns.length) parts.push(`目标缺列：${report.target_missing_columns.join("、")}`);
      if (report.target_extra_columns.length) parts.push(`目标多列：${report.target_extra_columns.join("、")}`);
      if (report.changed_samples.length) parts.push(`差异示例主键：${report.changed_samples.join("；")}`);
      status.textContent = `差异比较完成：${parts.join("；")}${report.message ? `\n${report.message}` : ""}`;
    } catch (e) { error(e); }
  }
  async function loadTableSelection() {
    if (!isDbMode()) return;
    const v = ++tablesVersion; tablesLoading = true;
    try {
      const rows = await api.listTables(src.conn.value, src.schema.value);
      if (v !== tablesVersion) return;
      // 树形选择的数据：表默认全选，视图默认不选（勾选后才同步）。
      tableSelection = rows.map(([name, ty]) => {
        const kind: "table" | "view" = ty === "view" ? "view" : "table";
        const checked = document.createElement("input"); checked.type = "checkbox"; checked.checked = kind === "table";
        checked.setAttribute("aria-label", `同步${kind === "view" ? "视图" : "表"} ${name}`);
        const target = textInput(name, 200); target.setAttribute("aria-label", `${name} 的目标表名（默认同名，视图物化为普通表）`);
        return { source: name, kind, target, checked };
      });
      renderTableSelection();
    } catch (e) { if (v === tablesVersion) error(e); }
    finally { if (v === tablesVersion) tablesLoading = false; }
  }
  function renderTableSelection() {
    const host = dlg.body.querySelector<HTMLElement>("#sync-db-tables");
    if (!host) return;
    host.replaceChildren();
    if (!tableSelection.length) {
      host.textContent = "源库中没有可同步的表或视图。";
      return;
    }
    const wrapper = document.createElement("div"); wrapper.style.cssText = "display:flex;flex-direction:column;gap:2px;max-height:280px;overflow:auto;border:1px solid var(--border);border-radius:4px;padding:8px";
    const rootCheck = document.createElement("input"); rootCheck.type = "checkbox"; rootCheck.setAttribute("aria-label", "全选（表与视图）");
    const rootLabel = document.createElement("label"); rootLabel.style.cssText = "display:flex;align-items:center;gap:8px;font-weight:600";
    rootLabel.append(rootCheck, document.createTextNode(`源库 ${src.schema.value || "-"}`));
    wrapper.appendChild(rootLabel);
    const syncStates = () => {
      for (const kind of ["table", "view"] as const) {
        const group = tableSelection.filter(t => t.kind === kind);
        const box = groupChecks.get(kind);
        if (!box || !group.length) continue;
        const selected = group.filter(t => t.checked.checked).length;
        box.checked = selected === group.length;
        box.indeterminate = selected > 0 && selected < group.length;
      }
      const total = tableSelection.length;
      const selected = tableSelection.filter(t => t.checked.checked).length;
      rootCheck.checked = selected === total;
      rootCheck.indeterminate = selected > 0 && selected < total;
    };
    const groupChecks = new Map<string, HTMLInputElement>();
    for (const [kind, label] of [["table", "表"], ["view", "视图（物化为普通表）"]] as const) {
      const items = tableSelection.filter(t => t.kind === kind);
      if (!items.length) continue;
      const groupCheck = document.createElement("input"); groupCheck.type = "checkbox"; groupCheck.setAttribute("aria-label", `全选${label}`);
      groupCheck.addEventListener("change", () => { items.forEach(t => t.checked.checked = groupCheck.checked); syncStates(); });
      groupChecks.set(kind, groupCheck);
      const groupRow = document.createElement("label"); groupRow.style.cssText = "display:flex;align-items:center;gap:8px;margin-top:6px;font-weight:600";
      groupRow.append(groupCheck, document.createTextNode(`${label}（${items.length}）`));
      wrapper.appendChild(groupRow);
      for (const item of items) {
        const row = document.createElement("label"); row.style.cssText = "display:flex;align-items:center;gap:8px;flex-wrap:wrap;padding-left:28px";
        item.checked.addEventListener("change", syncStates);
        row.append(item.checked, document.createTextNode(`${item.source} →`), item.target);
        wrapper.appendChild(row);
      }
    }
    rootCheck.addEventListener("change", () => { tableSelection.forEach(t => t.checked.checked = rootCheck.checked); syncStates(); });
    syncStates();
    host.replaceChildren(wrapper);
  }
  dlg = new Dialog({ title: "数据库同步", width: 780, height: 640, render: body => {
    body.style.padding = "0";
    body.style.display = "flex";
    body.style.flexDirection = "column";
    body.style.overflow = "hidden";
    wizard = new Wizard({
      steps: [
        {
          label: "选择源与目标",
          hint: "表对表：左侧是数据来源，右侧是写入目标，目标表不存在会自动创建。整库：选择源库与目标库，第 3 步树形勾选要同步的表与视图（视图默认不勾选）。",
          render: (host) => {
            host.append(field("同步方式:", syncKind), src.box, dst.box);
          },
          validate: () => {
            if (!src.conn.value || !src.schema.value) return "请选择源连接与数据库。";
            if (!dst.conn.value || !dst.schema.value) return "请选择目标连接与数据库。";
            if (!isDbMode() && (!src.table.value || !(dst.table as HTMLInputElement).value.trim())) return "请选择源表并填写目标表名。";
            if (supported.find((c) => c.id === dst.conn.value)?.read_only) return "目标连接是只读连接，无法写入。";
            return null;
          },
        },
        {
          label: "同步内容与写入模式",
          hint: "决定同步什么（结构 / 数据 / 两者）、全量还是增量，以及目标已有同名表时怎么处理。增量按主键只插入目标缺失的行；清空与重建不可撤销。",
          render: (host) => {
            host.append(
              modeField,
              strategyField,
              incrUpdateLabel,
              dbWriteField,
              tableWriteField,
              createLabel,
              field("源筛选 WHERE:", where),
            );
            const note = document.createElement("div");
            note.className = "wizard-panel-note";
            note.textContent = "整库同步始终包含结构：目标对象不存在时按源结构自动创建。视图会物化为目标普通表（不复制视图 DDL）。增量同步逐表按主键比较目标已有行，只插入缺失的行；源表无主键的表将报错跳过（可用整库「跳过已存在的表」替代）。清空与重建不可撤销，源端不受影响。";
            host.appendChild(note);
          },
          validate: () => {
            if (isDbMode() && mode.value === "structure" && dbWriteMode.value === "replace") return "仅同步结构时不能选择「清空同名表后写入」。";
            return null;
          },
        },
        {
          label: "映射确认与执行",
          hint: "表对表：逐列核对源字段与目标字段的对应关系；目标列留空表示该列不写入。整库：树形勾选要同步的表与视图及目标表名；视图按当前查询结果物化为普通表。",
          render: (host) => {
            host.append(mapping, status);
            const dbTables = document.createElement("div"); dbTables.id = "sync-db-tables"; dbTables.style.marginTop = "8px";
            host.appendChild(dbTables);
            const tools = document.createElement("div");
            tools.className = "sync-map-tools";
            const refresh = document.createElement("button");
            refresh.type = "button";
            refresh.className = "btn";
            refresh.textContent = "刷新映射";
            refresh.addEventListener("click", () => {
              if (!busy) void refreshMapping();
            });
            // 表对表：执行前可先比较源表与目标表的差异（只读）。
            const diffBtn = document.createElement("button");
            diffBtn.type = "button";
            diffBtn.className = "btn";
            diffBtn.textContent = "比较差异";
            diffBtn.addEventListener("click", () => {
              if (busy || mapLoading) return;
              void runDiff();
            });
            const tip = document.createElement("div");
            tip.className = "wizard-panel-note";
            tip.style.marginTop = "0";
            tip.textContent = "目标列留空表示该列不写入。比较差异为只读操作，不会修改数据。";
            tools.append(refresh, diffBtn, tip);
            host.appendChild(tools);
          },
          validate: () => {
            if (isDbMode()) {
              if (tablesLoading) return "表清单仍在加载，请稍候。";
              if (!tableSelection.some(t => t.checked.checked)) return "请至少勾选一个要同步的表或视图。";
              const names = new Set<string>();
              for (const item of tableSelection.filter(t => t.checked.checked)) {
                const key = item.target.value.trim();
                if (names.has(key.toLocaleLowerCase())) return `目标表名重复：${key}。请修改目标表名或取消多余的表。`;
                names.add(key.toLocaleLowerCase());
              }
              return null;
            }
            if (mapLoading) return "列映射仍在加载，请稍候。";
            if (!mappings.some((m) => m.target.value.trim())) return "至少映射一列；可点「刷新映射」重试。";
            return null;
          },
        },
      ],
      onEnter: (index) => {
        if (index === 1) updateKindUi();
        if (index === 2) {
          if (isDbMode()) { mapping.style.display = "none"; void loadTableSelection(); }
          else { mapping.style.display = ""; void refreshMapping(); }
          const dbTables = dlg.body.querySelector<HTMLElement>("#sync-db-tables");
          if (dbTables) dbTables.style.display = isDbMode() ? "" : "none";
          // 「比较差异」仅表对表模式提供；整库的逐表比较由增量同步本身完成。
          const diffBtn = [...dlg.body.querySelectorAll<HTMLButtonElement>(".sync-map-tools button")].find(b => b.textContent === "比较差异");
          if (diffBtn) diffBtn.style.display = isDbMode() ? "none" : "";
        }
      },
      finishLabel: "开始同步",
      onFinish: async () => {
        if (busy) return;
        if (isDbMode()) {
          const tables = tableSelection.filter(t => t.checked.checked).map(t => ({ source: t.source, target: t.target.value.trim(), view: t.kind === "view" }));
          if (!tables.length) { wizard.showError("请至少勾选一个要同步的表或视图"); return; }
          if (dbWriteMode.value === "replace" || dbWriteMode.value === "recreate") {
            const action = dbWriteMode.value === "recreate" ? "删除并重建同名表" : "清空同名表数据";
            if (!window.confirm(`整库同步将对目标库 ${dst.schema.value} 中所有同名已存在表执行「${action}」。源表不受影响，确认继续？`)) return;
          }
          busy = true; wizard.setBusy(true);
          try {
            const taskId = feedback.begin();
            const reports = await syncApi.runDatabase({
              task_id: taskId,
              source_connection: src.conn.value,
              source_schema: src.schema.value,
              target_connection: dst.conn.value,
              target_schema: dst.schema.value,
              mode: mode.value === "structure" ? "structure" : "both",
              write_mode: dbWriteMode.value as DbSyncOptions["write_mode"],
              sync_mode: syncStrategy.value as DbSyncOptions["sync_mode"],
              incremental_update: incrUpdate.checked,
              tables,
              where_clause: where.value.trim() || null,
            });
            feedback.finish();
            const okCount = reports.filter(r => !r.error && !r.skipped).length;
            const skippedCount = reports.filter(r => r.skipped).length;
            const failedCount = reports.filter(r => r.error).length;
            const rowsTotal = reports.reduce((sum, r) => sum + (r.rows || 0), 0);
            const lines = reports.map(r => `${r.source_table} → ${r.target_table}${r.skipped ? "：目标已存在，已跳过" : r.error ? `：失败（${r.error}）` : `：${r.rows} 行${r.created ? "，已建表" : ""}`}${r.warnings.length ? ` ⚠ ${r.warnings.join("；")}` : ""}`);
            status.textContent = `整库同步完成：成功 ${okCount} · 跳过 ${skippedCount} · 失败 ${failedCount}，共写入 ${rowsTotal} 行${failedCount ? `\n${lines.filter(l => l.includes("失败")).join("\n")}` : ""}`;
            onDone({ rows: rowsTotal, created: reports.some(r => r.created), warnings: reports.flatMap(r => r.warnings) });
          } catch (e) {
            feedback.finish();
            wizard.showError(String(e));
          } finally {
            busy = false;
            wizard.setBusy(false);
          }
          return;
        }
        const columns = mappings
          .filter((m) => m.target.value.trim())
          .map((m) => ({
            source: m.source,
            target: m.target.value.trim(),
            target_type: mode.value === "data" ? null : m.type.value.trim() || null,
          }));
        if (!columns.length) {
          wizard.showError("至少映射一列；请先刷新映射");
          return;
        }
        if (writeMode.value !== "append" && !window.confirm(`${writeMode.value === "recreate" ? "删除并重建" : "清空"}目标 ${dst.table.value}？源表不受影响。`)) {
          return;
        }
        busy = true;
        wizard.setBusy(true);
        try {
          const taskId = feedback.begin();
          const report = await syncApi.run({
            task_id: taskId,
            source_connection: src.conn.value,
            source_schema: src.schema.value,
            source_table: src.table.value,
            target_connection: dst.conn.value,
            target_schema: dst.schema.value,
            target_table: (dst.table as HTMLInputElement).value.trim(),
            mode: mode.value as SyncOptions["mode"],
            write_mode: writeMode.value as SyncOptions["write_mode"],
            sync_mode: syncStrategy.value as SyncOptions["sync_mode"],
            incremental_update: incrUpdate.checked,
            create_target: mode.value !== "data" && create.checked,
            columns,
            where_clause: where.value.trim() || null,
          });
          feedback.finish();
          status.textContent = `同步完成：${report.rows} 行${report.created ? "，已建表" : ""}${report.warnings.length ? `\n${report.warnings.join("\n")}` : ""}`;
          onDone(report);
        } catch (e) {
          feedback.finish();
          wizard.showError(String(e));
        } finally {
          busy = false;
          wizard.setBusy(false);
        }
      },
    });
    body.appendChild(wizard.stepsEl);
    body.appendChild(wizard.bodyEl);
  }, buttons: [] });
  // 底部：向导自带的上一步/下一步（完成按钮文案为「开始同步」）+ 关闭
  dlg.footer.append(...wizard.buttons(), (() => {
    const close = document.createElement("button");
    close.type = "button";
    close.className = "btn";
    close.textContent = "关闭";
    close.addEventListener("click", () => dlg.close());
    return close;
  })());
  accessibleDialog(dlg, "数据库同步");
  const feedback = taskFeedback(dlg, status);
  updateKindUi();
  void Promise.all([src.load(), dst.load()]);
  return dlg;
}
