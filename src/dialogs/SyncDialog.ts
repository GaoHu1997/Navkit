/** 表对表与整库（库到库）同步向导：映射、建表与进度反馈。 */
import { api, syncApi, transferTasks, type DbSyncOptions, type DiffMemoryOptions, type DiffMemoryStats, type StructureDiffReport, type SyncOptions, type SyncReport, type TaskProgress } from "../core/api";
import { namespaceName } from "../core/namespace-label";
import { StructureSyncReview } from "../ui/StructureSyncReview";
import { StructureSyncSession } from "../core/structure-sync";
import { structureSyncConnectionError, syncConnectionError, syncDiffConnectionError } from "../core/toolCapabilities";
import type { ConnectionConfig } from "../core/types";
import { Dialog, field, numberInput, select, textInput } from "../ui/Dialog";
import { Wizard } from "../ui/Wizard";
import { taskCenter } from "../core/backgroundTasks";
import { ObjectTree } from "../ui/ObjectTree";

/** Task feedback reused by the import/export wizard（onProgress 供后台任务中心联动）。 */
export function taskFeedback(dlg: Dialog, status: HTMLElement, host?: HTMLElement, onProgress?: (p: TaskProgress) => void, options: { cancelOnClose?: boolean; unit?: string; cancelLabel?: string } = {}) {
  let taskId: string | null = null;
  let timer: ReturnType<typeof setInterval> | null = null;
  let polling = false;
  let generation = 0;
  let startedAt = Date.now();
  const cancel = document.createElement("button");
  cancel.className = "btn";
  cancel.type = "button";
  cancel.textContent = options.cancelLabel ?? "取消任务";
  cancel.disabled = true;
  // 仅在任务运行中出现：平时隐藏，避免占位把「上一步」挤到右侧
  cancel.hidden = true;
  const progress = document.createElement("progress");
  progress.className = "exec-log-native-progress";
  progress.setAttribute("aria-label", "任务进度");
  progress.style.width = "100%";
  progress.hidden = true;
  status.setAttribute("role", "status");
  status.setAttribute("aria-live", "polite");
  status.style.whiteSpace = "pre-wrap";
  (host ?? dlg.body).append(progress);
  // 进度条跟随执行区的宽度，避免被挤在对话框最底部。
  progress.style.height = "6px";
  progress.style.accentColor = "var(--accent-500, #3574f0)";
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
  dlg.onClose(() => { generation++; stop(); if (taskId && options.cancelOnClose !== false) void transferTasks.cancel(taskId).catch(() => undefined); });
  return {
    begin() {
      generation++;
      stop();
      taskId = crypto.randomUUID();
      disabledControls = [...dlg.body.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement | HTMLButtonElement>("input:not(:disabled),select:not(:disabled),textarea:not(:disabled),button:not(:disabled)")];
      disabledControls.forEach(c => c.disabled = true);
      status.textContent = "正在启动…";
      startedAt = Date.now();
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
          // 进度提示：完成量 / 总量、百分比和按当前速率估算的剩余时间。
          let detail = "";
          if (p.total > 0) {
            const percent = Math.min(100, Math.floor((p.processed / p.total) * 100));
            progress.max = p.total; progress.value = p.processed;
            const seconds = (Date.now() - startedAt) / 1000;
            const remain = p.processed > 0 && seconds > 0.4
              ? Math.max(0, Math.round((p.total - p.processed) / (p.processed / seconds))) : null;
            detail = `\n${p.processed} / ${p.total} ${options.unit ?? "行"} · ${percent}%${remain !== null ? ` · 预计剩余 ${remain} 秒` : ""}`;
          } else if (p.processed > 0) {
            detail = `\n已处理 ${p.processed}`;
          }
          status.textContent = `${p.message}${detail}`;
          onProgress?.(p);
          if (p.status !== "running") stop();
        } catch { /* First poll may race with task registration; execution reports final errors. */ }
        finally { polling = false; }
      }, 250);
      return taskId;
    },
    finish() { generation++; stop(); taskId = null; cancel.hidden = true; cancel.disabled = true; disabledControls.forEach(c => c.disabled = false); disabledControls = []; dlg.body.removeAttribute("aria-busy"); progress.hidden = true; },
    /** 关闭窗口转后台：停止轮询与进度刷新，但保留后端任务，不触发 onClose 里的取消。 */
    detach() {
      generation++; stop(); taskId = null;
      cancel.hidden = true; cancel.disabled = true; progress.hidden = true;
      disabledControls.forEach(c => c.disabled = false); disabledControls = [];
      dlg.body.removeAttribute("aria-busy");
    },
  };
}

export function accessibleDialog(dlg: Dialog, title: string, previous: HTMLElement | null = document.activeElement as HTMLElement | null) {
  dlg.el.setAttribute("role", "dialog"); dlg.el.setAttribute("aria-modal", "true"); dlg.el.setAttribute("aria-label", title);
  dlg.el.style.maxWidth = "calc(100vw - 32px)";
  dlg.el.style.maxHeight = "calc(100vh - 32px)";
  if (dlg.body.querySelector(".wizard-body")) {
    dlg.body.style.maxHeight = "none";
    dlg.body.style.overflow = "hidden";
  } else {
    dlg.body.style.overflowY = "auto";
  }
  dlg.body.querySelectorAll<HTMLElement>(".field").forEach(row => {
    const label = row.querySelector(".field-label")?.textContent?.replace(/[:：]$/, "") ?? "选项";
    row.querySelectorAll<HTMLElement>("input,select,textarea").forEach(control => { if (!control.hasAttribute("aria-label")) control.setAttribute("aria-label", label); });
  });
  const close = dlg.el.querySelector<HTMLElement>(".dialog-title-close");
  if (close) { close.tabIndex = 0; close.setAttribute("role", "button"); close.setAttribute("aria-label", "关闭"); close.addEventListener("keydown", e => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); dlg.close(); } }); }
  dlg.el.addEventListener("keydown", e => {
    if (e.key === "Escape") { e.preventDefault(); dlg.close(); }
    if (e.key !== "Tab") return;
    const nodes = [...dlg.el.querySelectorAll<HTMLElement>("button:not(:disabled),input:not(:disabled),select:not(:disabled),textarea:not(:disabled),[tabindex='0']")].filter(n => n.getClientRects().length > 0);
    if (!nodes.length) return;
    if (e.shiftKey && document.activeElement === nodes[0]) { e.preventDefault(); nodes[nodes.length - 1]?.focus(); }
    else if (!e.shiftKey && document.activeElement === nodes[nodes.length - 1]) { e.preventDefault(); nodes[0].focus(); }
  });
  dlg.onClose(() => { if (previous?.isConnected) previous.focus({ preventScroll: true }); });
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
  const previousFocus = document.activeElement as HTMLElement | null;
  const supported = connections.filter(c => syncConnectionError(c) === null);
  if (!supported.length) throw new Error("没有支持数据同步的连接：请先添加 SQLite / MySQL / PostgreSQL / SQL Server / ClickHouse 或 ODBC 关系型连接");
  const targets = supported.filter(c => !c.read_only);
  if (!targets.length) throw new Error("没有可写的受支持目标连接，请先添加连接或取消目标只读设置");
  // 全局工具入口不再按当前选中库禁用：当前数据源不支持同步时对话框照常打开，
  // 但源/目标保持空选（不默认选中第一个连接），由用户在受支持范围内自行选择。
  const initialSource = source && supported.some(c => c.id === source.id) ? source : undefined;
  const sourceHint = source && !initialSource ? syncConnectionError(source) : null;
  const status = document.createElement("div");
  status.setAttribute("role", "status");
  status.setAttribute("aria-live", "polite");
  status.style.marginTop = "10px"; status.style.minHeight = "36px"; status.style.whiteSpace = "pre-line";
  if (sourceHint) status.textContent = `${sourceHint}。请直接在下方选择受支持的数据源。`;
  // 执行日志：逐表结果（成功 / 跳过 / 失败、行数与错误）独占向导最后一页，不再挤在映射页里。
  const logStatus = document.createElement("div");
  logStatus.className = "sync-log-status";
  logStatus.setAttribute("role", "status");
  logStatus.setAttribute("aria-live", "polite");
  logStatus.style.whiteSpace = "pre-line";
  logStatus.textContent = "尚未执行同步。";
  const logSummary = document.createElement("div");
  logSummary.className = "sync-log-summary";
  // taskFeedback 的原生进度条挂在这里，跟随日志页而不是对话框底部。
  const logProgressHost = document.createElement("div");
  logProgressHost.className = "sync-log-progress";
  const logEmpty = document.createElement("div");
  logEmpty.className = "sync-runlog-empty";
  logEmpty.textContent = "尚未执行同步。返回上一步核对配置后点「开始同步」。";
  const runLogBody = document.createElement("div");
  runLogBody.className = "sync-runlog-body";
  runLogBody.append(logEmpty);
  const runLog = document.createElement("div");
  runLog.className = "sync-runlog sync-runlog-page";
  runLog.setAttribute("role", "log");
  runLog.setAttribute("aria-label", "同步执行日志");
  runLog.append(runLogBody);
  let logRows = 0;
  const logClear = () => { logRows = 0; runLogBody.replaceChildren(logEmpty); };
  const logAdd = (state: "ok" | "skip" | "error", name: string, detail: string) => {
    if (!logRows) runLogBody.replaceChildren();
    logRows++;
    const row = document.createElement("div");
    row.className = "sync-runlog-row";
    row.dataset.state = state;
    const dot = document.createElement("span"); dot.className = "sync-runlog-dot"; dot.setAttribute("aria-hidden", "true");
    const label = document.createElement("span"); label.className = "sync-runlog-name"; label.textContent = name;
    const info = document.createElement("span"); info.className = "sync-runlog-detail"; info.textContent = detail;
    row.append(dot, label, info);
    runLogBody.append(row);
  };
  let busy = false;
  let structureClosed = false;
  let dlg: Dialog;
  let wizard!: Wizard;
  // 同步方式：表对表 / 整库（库到库）
  const syncKind = select([{ value: "table", label: "表对表同步" }, { value: "database", label: "整库同步（库到库）" }], "table");
  syncKind.setAttribute("aria-label", "同步方式");
  const mode = select([{ value: "both", label: "同步结构与数据" }, { value: "structure", label: "仅同步结构" }, { value: "data", label: "仅同步数据" }], "both");
  const dbWriteMode = select(DB_WRITE_MODES.map(o => ({ value: o.value, label: o.label })), "skip");
  const writeMode = select([{ value: "append", label: "追加数据 / 保留已有结构" }, { value: "replace", label: "清空后写入（数据事务）" }, { value: "recreate", label: "重建目标表（删除原表）" }], "append");
  // 同步方式：全量（按写入模式处理全部数据）/ 增量（按主键只插入目标缺失的行）
  const syncStrategy = select([{ value: "full", label: "全量同步" }, { value: "incremental", label: "增量同步（按主键，仅插入目标缺失的行）" }], "full");
  syncStrategy.setAttribute("aria-label", "同步方式");
  const incrUpdate = document.createElement("input"); incrUpdate.type = "checkbox";
  const incrUpdateLabel = document.createElement("label"); incrUpdateLabel.className = "checkbox";
  const incrUpdateText = document.createElement("span");
  incrUpdateText.textContent = "同时更新主键相同但内容不同的行（归一化近似比较）";
  incrUpdateLabel.append(incrUpdate, incrUpdateText);
  const where = textInput("");
  const bizKey = textInput(""); bizKey.setAttribute("placeholder", "业务键列（逗号分隔，留空用主键）"); bizKey.setAttribute("aria-label", "增量同步业务键列");
  bizKey.id = `sync-biz-key-${crypto.randomUUID()}`;
  const deleteMissing = document.createElement("input"); deleteMissing.type = "checkbox";
  const deleteMissingLabel = document.createElement("label"); deleteMissingLabel.className = "checkbox sync-danger-option";
  deleteMissingLabel.append(deleteMissing, document.createTextNode("增量时删除目标中多余的行（不可恢复）"));
  const create = document.createElement("input"); create.type = "checkbox"; create.checked = true;
  const createLabel = document.createElement("label"); createLabel.className = "checkbox";
  const createText = document.createElement("span"); createText.textContent = "目标表不存在时自动建表（含主键/索引）";
  createLabel.append(create, createText);
  const mapping = document.createElement("table"); mapping.className = "mini-table";
  let mappings: { source: string; target: HTMLInputElement; type: HTMLInputElement }[] = [];
  let mapVersion = 0;
  let mapLoading = false;
  // 整库模式第 3 步的表清单（checked 由统一对象树创建）
  let tableSelection: { source: string; kind: "table" | "view"; target: HTMLInputElement; checked?: HTMLInputElement }[] = [];
  let tablesLoading = false;
  let tablesVersion = 0;
  let tableSelectionSource = "";
  let tableSelectionError = "";
  /** 整库表清单的本地搜索关键字；步骤重进/清单重载时保留。 */
  let tableFilter = "";
  const error = (e: unknown) => {
    if (structureClosed) return;
    const message = e instanceof Error ? e.message : String(e);
    // 错误全文只进悬浮提示；状态区留一行摘要，不把长错误铺满步骤内容。
    status.setAttribute("role", "alert");
    status.textContent = `操作失败：${message.split("\n")[0]}`;
    if (wizard) wizard.showError(message); else dlg?.showError(message);
  };
  const isDbMode = () => syncKind.value === "database";
  function side(title: string, connectionId: string, initialSchema: string, initialTable: string, target: boolean) {
    const pool = (target ? targets : supported).map(c => ({ value: c.id, label: `${c.name} (${c.kind})` }));
    // 无预选（当前数据源不支持同步）时加占位项，保持空选而不是默认选中第一个连接。
    const conn = select(pool.some(o => o.value === connectionId) ? pool : [{ value: "", label: "请选择连接" }, ...pool], connectionId);
    const schema = select([], "");
    const table = target ? textInput(initialTable) : select([], "");
    const tableField = field("表:", table);
    let version = 0;
    let knownTables: string[] = [];
    let autoSelected: ((name: string) => void) | null = null;
    const box = document.createElement("fieldset"); box.className = "groupbox sync-side";
    const legend = document.createElement("legend");
    legend.className = "sync-side-legend";
    legend.innerHTML = `<span class="sync-side-badge${target ? " target" : ""}">${target ? "目标" : "源"}</span><span class="sync-side-title">${target ? (isDbMode() ? "目标库" : title) : (isDbMode() ? "源库" : title)}</span>`;
    const schemaField = field("", schema);
    box.append(legend, field("连接:", conn), schemaField, tableField);
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
        knownTables = rows.filter(([, ty]) => ty === "table").map(([n]) => n);
        if (target) {
          const list = box.querySelector("datalist")!; list.replaceChildren(...rows.filter(([, ty]) => ty === "table").map(([n]) => new Option(n, n)));
          updateTargetHint();
        } else {
          const sel = table as HTMLSelectElement; sel.replaceChildren(...rows.filter(([, ty]) => ty === "table").map(([n]) => new Option(n, n)));
          sel.value = rows.some(([n]) => n === initialTable) ? initialTable : sel.options[0]?.value ?? "";
          autoSelected?.(sel.value);
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
      const namespace = namespaceName(connections.find(connection => connection.id === conn.value));
      schemaField.querySelector<HTMLElement>(".field-label")!.textContent = `${namespace}:`;
      schema.setAttribute("aria-label", `${target ? "目标" : "源"}${namespace}`);
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
        if (!schema.value) throw new Error(`该连接没有可用${namespace}`);
        legend.innerHTML = `<span class="sync-side-badge${target ? " target" : ""}">${target ? "目标" : "源"}</span><span class="sync-side-title">${target ? (isDbMode() ? "目标库" : title) : (isDbMode() ? "源库" : title)}</span>`;
        await tables(v);
      } catch (e) { if (v === version) error(e); }
      finally { if (v === version) conn.disabled = schema.disabled = false; }
    }
    conn.addEventListener("change", () => void databases()); schema.addEventListener("change", () => void tables());
    table.addEventListener("input", () => { updateTargetHint(); void refreshMapping(); });
    table.addEventListener("change", () => { updateTargetHint(); void refreshMapping(); });
    return { box, conn, schema, table, tableField, hint, load: databases, updateHint: updateTargetHint, setAutoSelect: (fn: (name: string) => void) => { autoSelected = fn; }, knownTables: () => knownTables };
  }
  const src = side("来源表", initialSource?.id ?? "", initialSource ? prefill.schema : "", initialSource ? prefill.table : "", false);
  const dst = side("写入表", initialSource ? (supported.find(c => c.id !== initialSource.id && !c.read_only)?.id ?? initialSource.id) : "", "", initialSource ? prefill.table : "", true);
  // 源表变化或表列表自动选中时，目标表默认跟随同名（用户手动改过目标表名后不再覆盖）。
  // 程序化回填不能派发 input 事件，否则会误置 dstTouched，导致之后切换源表不再跟随。
  let dstTouched = false;
  (dst.table as HTMLInputElement).addEventListener("input", () => { dstTouched = true; });
  /** 把目标表名默认同步为源表同名；用户手动编辑过目标表名后不再覆盖。 */
  const syncTargetName = () => {
    if (isDbMode() || dstTouched) return;
    const name = (src.table as HTMLSelectElement).value || "";
    if (!name) return;
    (dst.table as HTMLInputElement).value = name;
    dst.updateHint();
  };
  (src.table as HTMLSelectElement).addEventListener("change", () => { syncTargetName(); });
  src.setAutoSelect(() => syncTargetName());
  // 源/目标连接切换时，按新方言重新判定「比较差异 / 预览结构差异」是否可用。
  src.conn.addEventListener("change", updateActionAvailability);
  dst.conn.addEventListener("change", updateActionAvailability);
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
    updateActionAvailability();
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
    updateActionAvailability();
  });
  dbWriteMode.addEventListener("change", () => {
    if (isDbMode() && mode.value === "structure" && dbWriteMode.value === "replace") dbWriteMode.value = "skip";
  });
  /** 表头上方的列数摘要；文案随映射重建更新，整库模式整行隐藏。 */
  function updateMapTitle() {
    const el = dlg?.body.querySelector<HTMLElement>(".sync-map-title");
    if (el) el.textContent = mappings.length ? `列映射 · ${mappings.length} 列` : "列映射";
  }
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
      const sourceColumns = document.createElement("datalist");
      sourceColumns.id = `sync-source-cols-${crypto.randomUUID()}`;
      sourceColumns.append(...types.map(t => new Option(t.source, t.source)));
      mapping.append(sourceColumns);
      bizKey.setAttribute("list", sourceColumns.id);
      bizKey.setAttribute("data-search-token", ",");
      for (const t of types) {
        const row = document.createElement("tr"); const label = document.createElement("td"); label.textContent = `${t.source} · ${t.source_type}`;
        const target = textInput(t.source, 160); target.setAttribute("aria-label", `${t.source} 对应目标列（留空跳过）`);
        const type = textInput(t.target_type, 160); type.setAttribute("aria-label", `${t.source} 目标建表类型`); type.disabled = mode.value === "data";
        const a = document.createElement("td"); a.append(target); const b = document.createElement("td"); b.append(type); row.append(label, a, b); tbody.append(row); mappings.push({ source: t.source, target, type });
      }
      // 目标表已存在时提供其列名候选；新建表 / 元数据不可读时静默保持自由输入。
      void attachTargetColumnList(mappings.map(m => m.target), v);
    } catch (e) { if (v === mapVersion) { mapping.replaceChildren(); mappings = []; error(e); } }
    finally { if (v === mapVersion) { mapLoading = false; updateMapTitle(); } }
  }
  /** 只读读取目标表现有列名并附加到映射的目标列输入；失败不提示、不阻断、不触发任何写入。 */
  async function attachTargetColumnList(inputs: HTMLInputElement[], version: number): Promise<void> {
    const connectionId = dst.conn.value;
    const schemaName = dst.schema.value;
    const tableName = (dst.table as HTMLInputElement).value.trim();
    if (!connectionId || !schemaName || !tableName || !inputs.length) return;
    try {
      const schema = await api.getTableSchema(connectionId, schemaName, tableName);
      if (version !== mapVersion || structureClosed || isDbMode() ||
        connectionId !== dst.conn.value || schemaName !== dst.schema.value ||
        tableName !== (dst.table as HTMLInputElement).value.trim()) return;
      const names = [...new Set(schema.columns.map(c => c.name).filter(name => name.trim()))];
      if (!names.length) return;
      const list = document.createElement("datalist");
      list.id = `sync-target-cols-${crypto.randomUUID()}`;
      list.append(...names.map(name => new Option(name, name)));
      mapping.append(list);
      for (const input of inputs) input.setAttribute("list", list.id);
    } catch { /* 目标表不存在或元数据不可读：保留手填目标列即可。 */ }
  }
  const connectionById = (id: string) => connections.find((c) => c.id === id);
  /** 受控结构同步支持同方言原生 SQLite/MySQL/PostgreSQL，后端仍复核真实适配器与身份。 */
  function structureLanguageReason(): string | null {
    if (mode.value === "data") return "「仅同步数据」模式不涉及结构差异；请切换为「同步结构与数据」或「仅同步结构」后再预览。";
    const source = connectionById(src.conn.value);
    const target = connectionById(dst.conn.value);
    if (!source || !target) return "请先在第一步选择源连接与目标连接。";
    if (source.kind !== target.kind) return "结构同步暂不支持跨方言；请选择同为 SQLite、MySQL 8 或 PostgreSQL 的源与目标。";
    return structureSyncConnectionError(source) ?? structureSyncConnectionError(target);
  }
  /** 数据差异比较的方言白名单与后端 sync::supported 一致。 */
  function diffLanguageReason(): string | null {
    const source = connectionById(src.conn.value);
    const target = connectionById(dst.conn.value);
    if (!source || !target) return "请先在第一步选择源连接与目标连接。";
    return syncDiffConnectionError(source) ?? syncDiffConnectionError(target);
  }
  /**
   * 按当前源/目标方言与同步内容，把「比较差异」「预览结构差异」在不可用时置灰（并给出 title 说明），
   * 避免用户先点按钮、再看到后端方言不支持（如 STRUCTURE_CONTROLLED_DIALECT_UNSUPPORTED）的报错。
   */
  function updateActionAvailability() {
    if (structureClosed) return;
    const diffReason = isDbMode() ? null : diffLanguageReason();
    const structReason = isDbMode() ? null : structureLanguageReason();
    const buttons = [...(dlg?.body.querySelectorAll<HTMLButtonElement>("button") ?? [])];
    const diffBtnEl = buttons.find((b) => b.dataset.diff === "true");
    const budgetReason = diffMemoryReason();
    const invalid = !budgetReason && memoryControl.checked && memoryMode.value === "manual" ? manualMemoryError() : null;
    if (diffBtnEl) { diffBtnEl.disabled = diffRunning || !!diffReason || !!invalid; diffBtnEl.title = diffReason ?? invalid ?? "比较源表与目标表的差异（只读）"; }
    memoryControl.disabled = diffRunning || !!diffReason || !!budgetReason;
    memoryControl.title = diffReason ?? budgetReason ?? "仅用于比较差异，不改变同步执行策略";
    memoryMode.disabled = memoryControl.disabled || !memoryControl.checked;
    memoryLimit.disabled = memoryMode.disabled || memoryMode.value !== "manual";
    memoryLimit.hidden = memoryLimitLabel.hidden = memoryMode.value !== "manual";
    memoryLimit.setAttribute("aria-invalid", String(!!invalid));
    if (invalid) memoryInfo.textContent = invalid;
    if (!diffRunning) scheduleMemoryPreview();
    if (diffRunning && diffBtnEl) diffBtnEl.setAttribute("aria-busy", "true");
    else diffBtnEl?.removeAttribute("aria-busy");
    const previewBtnEl = buttons.find((b) => b.dataset.structurePreview === "true");
    if (previewBtnEl) { previewBtnEl.disabled = diffRunning || !!structReason; previewBtnEl.title = structReason ?? "只读预览目标表相对源表的结构差异"; }
    const applyBtnEl = buttons.find((b) => b.dataset.structureApply === "true");
    if (applyBtnEl && (structReason || diffRunning)) { applyBtnEl.disabled = true; applyBtnEl.title = structReason ?? "数据差异正在比较，请等待完成。"; }
  }
  const memoryControl = document.createElement("input");
  memoryControl.type = "checkbox";
  memoryControl.checked = false;
  memoryControl.setAttribute("aria-label", "内存预算控制");
  const memoryMode = select([{ value: "auto", label: "自动预算" }, { value: "manual", label: "手动预算" }], "auto");
  memoryMode.setAttribute("aria-label", "比较内存预算模式");
  const memoryLimit = numberInput(0, 112);
  memoryLimit.value = "";
  memoryLimit.min = "1";
  memoryLimit.step = "1";
  memoryLimit.max = String(Math.floor(Number.MAX_SAFE_INTEGER / 1048576));
  memoryLimit.placeholder = "MiB";
  memoryLimit.setAttribute("aria-label", "手动比较缓冲预算（MiB）");
  const memoryLimitLabel = document.createElement("label");
  memoryLimitLabel.className = "sync-diff-memory-option";
  memoryLimitLabel.append(memoryLimit, document.createTextNode("MiB"));
  const memoryInfo = document.createElement("div");
  memoryInfo.className = "wizard-panel-note";
  memoryInfo.id = `sync-diff-memory-${crypto.randomUUID()}`;
  memoryInfo.setAttribute("role", "status");
  memoryInfo.setAttribute("aria-live", "polite");
  for (const control of [memoryControl, memoryMode, memoryLimit]) control.setAttribute("aria-describedby", memoryInfo.id);
  let diffRunning = false;
  let diffGeneration = 0;
  let memoryPreviewGeneration = 0;
  let memoryPreviewTimer: ReturnType<typeof setTimeout> | null = null;
  let memoryPreviewKey = "";
  let reportMemoryShown = false;
  /** kind 仅决定显示候选控件；真实原生 SQLite 文件身份必须由后台判定。 */
  function diffMemoryReason(): string | null {
    if (isDbMode()) return "预算控制仅用于表对表数据差异比较。";
    const source = connectionById(src.conn.value);
    const target = connectionById(dst.conn.value);
    if (source?.kind !== "sqlite" || target?.kind !== "sqlite") return "预算控制暂未认证，使用兼容比较。第一期仅原生 SQLite 文件库，由后台认定真实文件身份。";
    return null;
  }
  function manualMemoryError(): string | null {
    const value = memoryLimit.value.trim();
    const mib = Number(value);
    return /^\d+$/.test(value) && Number.isSafeInteger(mib) && mib > 0 && Number.isSafeInteger(mib * 1048576)
      ? null : "手动预算须为正整数 MiB，换算字节不得超过安全整数范围；实际工作区最小值与授权上限由后台校验。";
  }
  function diffMemoryOptions(): DiffMemoryOptions {
    return !memoryControl.checked ? { mode: "speed" } : memoryMode.value === "manual"
      ? { mode: "manual", limit_bytes: Number(memoryLimit.value) * 1048576 } : { mode: "auto" };
  }
  function memoryBytes(bytes: number): string {
    if (!Number.isFinite(bytes) || bytes < 0) return "未知";
    if (bytes < 1048576) return `${Math.round(bytes)} B`;
    return `${(bytes / 1048576).toLocaleString("zh-CN", { maximumFractionDigits: 2 })} MiB`;
  }
  function renderMemoryReport(memory: DiffMemoryStats, requested?: DiffMemoryOptions) {
    reportMemoryShown = true;
    const backend = memory.backend === "memory" ? "内存（memory）" : memory.backend === "disk" ? "磁盘（disk）" : "仅行数（count_only）";
    memoryInfo.textContent = `实际比较：${requested?.mode === "manual" ? `请求上限 ${memoryBytes(requested.limit_bytes ?? 0)} · ` : ""}有效可用 ${memoryBytes(memory.available_bytes)} · 实际额度上限 ${memoryBytes(memory.limit_bytes)} · 结束核算占用 ${memoryBytes(memory.used_bytes)} · 核算峰值（含暂存预留）${memoryBytes(memory.peak_bytes)} · 路径 ${backend} · 溢写数据量 ${memoryBytes(memory.spilled_bytes)} · 压力事件 ${memory.pressure_events} · 模式 ${memory.mode}\n范围：${memory.scope}。核算峰值不是进程RSS，额度上限不是预分配。`;
  }
  function scheduleMemoryPreview() {
    const invalid = memoryControl.checked && memoryMode.value === "manual" && manualMemoryError();
    const options = diffMemoryOptions();
    const key = JSON.stringify([src.conn.value, dst.conn.value, diffMemoryReason(), invalid || options]);
    if (key === memoryPreviewKey) return;
    memoryPreviewKey = key;
    const generation = ++memoryPreviewGeneration;
    if (memoryPreviewTimer) clearTimeout(memoryPreviewTimer);
    memoryPreviewTimer = null;
    reportMemoryShown = false;
    if (diffMemoryReason()) { memoryInfo.textContent = ""; return; }
    // 预算控制未启用时不展示预估：默认零噪音，也避免每次进页空转一次 IPC。
    if (!memoryControl.checked) { memoryInfo.textContent = ""; return; }
    if (invalid) { memoryInfo.textContent = invalid; return; }
    memoryInfo.textContent = "正在读取原生预算预估…预估额度，不代表已分配。";
    memoryPreviewTimer = setTimeout(async () => {
      memoryPreviewTimer = null;
      try {
        const preview = await syncApi.previewMemory(options);
        if (structureClosed || generation !== memoryPreviewGeneration || diffRunning || reportMemoryShown) return;
        memoryInfo.textContent = `预估：有效可用 ${memoryBytes(preview.available_bytes)} · 建议额度上限 ${memoryBytes(preview.suggested_limit_bytes)} · 比较池上限 ${memoryBytes(preview.global_limit_bytes)}\n预估额度，不代表已分配；比较时后台重新采样校验。范围：${preview.scope}${preview.limitations.length ? `；${preview.limitations.join("；")}` : ""}`;
      } catch (e) {
        if (structureClosed || generation !== memoryPreviewGeneration || diffRunning || reportMemoryShown) return;
        memoryInfo.textContent = `预算预估不可用：${e instanceof Error ? e.message : String(e)}。不以此阻断正常比较，实际额度由后台校验。`;
      }
    }, 250);
  }
  for (const control of [memoryControl, memoryMode, memoryLimit]) {
    for (const event of ["input", "change"]) control.addEventListener(event, e => {
      // 比较预算不是同步/结构配置：不触发全局 edited 失效已确认的结构预览。
      e.stopPropagation();
      if (!diffRunning) updateActionAvailability();
    });
    // 预算输入回车不能误触发同步写入；只比较仍由显式按钮发起。
    control.addEventListener("keydown", e => { if ((e as KeyboardEvent).key === "Enter") e.preventDefault(); });
  }
  /** 表对表：冻结只读比较请求及预算；独立于同步 busy，不产生写任务关闭确认。 */
  async function runDiff() {
    if (diffRunning || busy || structureClosed || structureSession.busy || isDbMode() || diffLanguageReason()) return;
    const req = {
      source_connection: src.conn.value, source_schema: src.schema.value, source_table: src.table.value,
      target_connection: dst.conn.value, target_schema: dst.schema.value, target_table: (dst.table as HTMLInputElement).value.trim(),
      where_clause: where.value.trim() || null,
    };
    if (!req.source_connection || !req.source_table || !req.target_connection || !req.target_schema || !req.target_table) {
      status.textContent = "请先在第一步选择源表并填写目标表名，再比较差异。";
      return;
    }
    const memory = diffMemoryReason() ? undefined : diffMemoryOptions();
    if (memory?.mode === "manual" && manualMemoryError()) { updateActionAvailability(); return; }
    diffRunning = true;
    reportMemoryShown = false;
    const generation = ++diffGeneration;
    ++memoryPreviewGeneration;
    if (memoryPreviewTimer) clearTimeout(memoryPreviewTimer);
    memoryPreviewTimer = null;
    const locked = [src.conn, src.schema, src.table, dst.conn, dst.schema, dst.table, where, memoryControl, memoryMode, memoryLimit]
      .filter(control => !control.disabled);
    locked.forEach(control => control.disabled = true);
    wizard.setBusy(true);
    updateActionAvailability();
    status.style.maxHeight = "120px"; status.style.overflowY = "auto"; status.style.overflowWrap = "anywhere"; status.style.flexShrink = "0";
    // 结果区必须为不可收缩的状态行留空间；其余内容交给现有向导滚动，不能盖住预算控件。
    if (status.parentElement?.classList.contains("sync-map")) status.parentElement.style.minHeight = "136px";
    status.setAttribute("role", "status");
    status.textContent = "正在比较源表与目标表差异…";
    try {
      const report = memory ? await syncApi.diff(req, false, memory) : await syncApi.diff(req);
      if (structureClosed || generation !== diffGeneration) return;
      const parts = [`源 ${report.source_rows} 行 / 目标 ${report.target_rows} 行`];
      if (report.keyed) {
        parts.push(`目标缺失 ${report.missing_in_target} 行`, `目标多出 ${report.extra_in_target} 行`, `内容差异 ${report.changed} 行（归一化近似）`);
      }
      if (report.target_missing_columns.length) parts.push(`目标缺列：${report.target_missing_columns.join("、")}`);
      if (report.target_extra_columns.length) parts.push(`目标多列：${report.target_extra_columns.join("、")}`);
      if (report.changed_samples.length) parts.push(`差异示例主键：${report.changed_samples.join("；")}`);
      status.textContent = `差异比较完成：${parts.join("；")}${report.message ? `\n${report.message}` : ""}`;
      if (report.memory) renderMemoryReport(report.memory, memory);
    } catch (e) { if (!structureClosed && generation === diffGeneration) error(e); }
    finally {
      diffRunning = false;
      if (!structureClosed && generation === diffGeneration) {
        locked.forEach(control => control.disabled = false);
        wizard.setBusy(busy || structureSession.busy || structureSession.effectsUnknown);
        const apply = dlg.body.querySelector<HTMLButtonElement>("[data-structure-apply]");
        if (apply) apply.disabled = !structureSession.canApply;
        if (!reportMemoryShown) memoryPreviewKey = "";
        updateActionAvailability();
      }
    }
  }

  /** 表对表：只读预览目标表相对源表的结构差异。 */
  const structureLockedControls = new Set<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>();
  const structureStop = document.createElement("button"); structureStop.type = "button"; structureStop.className = "btn"; structureStop.textContent = "停止结构同步"; structureStop.disabled = true;
  const structureReview = new StructureSyncReview(() => syncStructureUi());
  let structurePreview: StructureDiffReport | null = null;
  const structureSession = new StructureSyncSession(crypto.randomUUID(), {
    issueGrant: request => api.issueGrant(request),
    preview: (request, ownerId) => syncApi.previewStructure(request, ownerId),
    execute: (request, syncRequest) => syncApi.executeStructure(request, syncRequest),
    current: () => structureSyncRequest(),
    cancelExecution: context => api.cancelExecution(context), executionStatus: context => api.executionStatus(context),
  }, syncStructureUi);
  function syncStructureUi() {
    if (structureClosed) return;
    const locked = structureSession.busy || structureSession.effectsUnknown;
    if (dlg) {
      if (locked) {
        dlg.body.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>("input:not(:disabled),select:not(:disabled),textarea:not(:disabled)").forEach(control => { structureLockedControls.add(control); control.disabled = true; });
      } else { structureLockedControls.forEach(control => control.disabled = false); structureLockedControls.clear(); }
      wizard?.setBusy(busy || locked);
      const apply = dlg.body.querySelector<HTMLButtonElement>("[data-structure-apply]");
      const ready = structureSession.canApply && structureReview.ready;
      structureReview.setLocked(locked || busy);
      if (apply) { apply.disabled = !ready; apply.title = ready ? "应用已确认的双端结构预览" : structureSession.effectsUnknown ? "写入回执未知；请明确重新预览核对真实结构" : structureSession.canApply ? "请先逐项确认结构变化中的扫描/破坏风险" : "请先按当前目标与映射预览结构差异"; }
    }
    structureStop.disabled = !structureSession.busy;
  }
  structureStop.addEventListener("click", () => { void structureSession.stop().catch(error); });
  const allowDrop = document.createElement("input"); allowDrop.type = "checkbox";
  const allowDropLabel = document.createElement("label"); allowDropLabel.className = "checkbox sync-danger-option";
  allowDropLabel.append(allowDrop, document.createTextNode("允许删除目标多余的列/索引（危险）"));
  const structureSyncRequest = () => ({
    source_connection: src.conn.value, source_schema: src.schema.value, source_table: src.table.value,
    target_connection: dst.conn.value, target_schema: dst.schema.value,
    target_table: (dst.table as HTMLInputElement).value.trim(),
    columns: mappings.filter(m => m.target.value.trim()).map(m => [m.source, m.target.value.trim()] as [string, string]),
    allow_drop: allowDrop.checked,
    target_revision: "", source_revision: "", review_digest: "",
  });
  function renderStructureReport(report: StructureDiffReport) {
    structureReview.render(report);
    status.textContent = `结构差异：${report.summary}`;
    syncStructureUi();
  }
  async function previewStructure() {
    if (busy || structureSession.busy || structureClosed || isDbMode()) return;
    const req = structureSyncRequest();
    if (!req.source_connection || !req.source_table || !req.target_connection || !req.target_schema || !req.target_table) {
      status.textContent = "请先在第一步选择源表并填写目标表名，再预览结构差异。";
      return;
    }
    structurePreview = null; structureReview.clear();
    status.textContent = "正在比较结构差异…";
    try {
      const report = await structureSession.preview(req);
      if (structureClosed) return;
      structurePreview = report; renderStructureReport(report);
    } catch (e) { structurePreview = null; error(e); }
  }
  async function applyStructure() {
    if (!structurePreview) { status.textContent = "请先预览结构差异，确认后再应用。"; return; }
    if (!structurePreview.target_exists) { status.textContent = "目标表不存在：请改用建表/重建同步流程。"; return; }
    if (!structurePreview.alterable) { status.textContent = `无法应用：${structurePreview.unsupported.join("；") || "当前方言不支持原地修改"}`; return; }
    if (!structurePreview.changes.length) { status.textContent = "结构与目标一致，无需应用。"; return; }
    const req = structureSyncRequest();
    if (!structureSession.canApply) { status.textContent = structureSession.effectsUnknown ? "上次写入结果未知；请先重新预览真实双端结构，禁止重复应用。" : "目标或映射已改变，请重新预览结构差异。"; return; }
    if (!structureReview.ready) { status.textContent = "请先逐项确认结构变化中的扫描/破坏风险。"; return; }
    const destructive = req.allow_drop || structurePreview.changes.some(c => c.risk === "destructive");
    if (!window.confirm(`将对目标表「${req.target_table}」应用已审核的 ${structurePreview.changes.length} 项结构修改${destructive ? "，其中包含删除或重建风险" : ""}。扫描/转换及依赖限制见上方冻结计划；确认继续？`)) return;
    if (busy) return;
    busy = true; wizard.setBusy(true);
    const controls = [...dlg.body.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>("input:not(:disabled),select:not(:disabled),textarea:not(:disabled)")];
    controls.forEach(control => control.disabled = true);
    dlg.body.setAttribute("aria-busy", "true");
    try {
      status.textContent = "正在应用已绑定的双端结构计划…";
      const result = await structureSession.apply(destructive, structureReview.approvedIds);
      if (structureClosed) return;
      const applied = result.receipt.steps.filter(s => s.status === "applied").length;
      if (result.receipt.verified) {
        status.textContent = `已应用 ${applied} 项结构修改（${result.receipt.transaction}），读回校验通过。`;
      } else {
        status.textContent = `已执行 ${applied} 项，但读回校验未通过：${result.receipt.mismatches.slice(0, 5).join("；") || "未知差异"}`;
      }
      structurePreview = null;
    } catch (e) { error(e); }
    finally { busy = false; controls.forEach(control => control.disabled = false); dlg.body.removeAttribute("aria-busy"); syncStructureUi(); }
  }
  async function loadTableSelection() {
    if (structureClosed || !isDbMode()) return;
    const sourceKey = JSON.stringify([src.conn.value, src.schema.value]);
    if (sourceKey !== tableSelectionSource) { tableSelection = []; tableSelectionSource = ""; renderTableSelection(); }
    tableSelectionError = "";
    const previous = sourceKey === tableSelectionSource
      ? new Map(tableSelection.map(item => [`${item.kind}:${item.source}`, item]))
      : new Map<string, typeof tableSelection[number]>();
    const v = ++tablesVersion; tablesLoading = true;
    try {
      const rows = await api.listTables(src.conn.value, src.schema.value);
      if (structureClosed || v !== tablesVersion || !isDbMode() || sourceKey !== JSON.stringify([src.conn.value, src.schema.value])) return;
      // 同一源库重载保留勾选与目标表名，源库切换或新增对象使用默认状态。
      tableSelection = rows.map(([name, ty]) => {
        const kind: "table" | "view" = ty === "view" ? "view" : "table";
        const saved = previous.get(`${kind}:${name}`);
        if (saved) return saved;
        const target = textInput(name, 200); target.setAttribute("aria-label", `${name} 的目标表名（默认同名，视图物化为普通表）`);
        return { source: name, kind, target };
      });
      tableSelectionSource = sourceKey;
      renderTableSelection();
    } catch (e) { if (!structureClosed && v === tablesVersion) { tableSelectionError = String(e); error(e); } }
    finally { if (v === tablesVersion) tablesLoading = false; }
  }
  const invalidateTableSelection = () => {
    ++tablesVersion; tablesLoading = false; tableSelectionSource = ""; tableSelectionError = ""; tableSelection = [];
    if (!structureClosed) renderTableSelection();
  };
  src.conn.addEventListener("change", invalidateTableSelection);
  src.schema.addEventListener("change", invalidateTableSelection);
  function renderTableSelection() {
    const host = dlg.body.querySelector<HTMLElement>("#sync-db-tables");
    if (!host) return;
    host.replaceChildren();
    if (!tableSelection.length) {
      host.textContent = "源库中没有可同步的表或视图。";
      return;
    }
    // 与「新建备份」一致的树形对象选择：分组可折叠、可全选，每行右侧为目标表名列。
    const tree = new ObjectTree({ ariaLabel: "选择要同步的表与视图" });
    tree.el.style.flex = "1";
    tree.el.style.minHeight = "0";
    const groups = {
      table: tree.addGroup({ title: "表", icon: "info_pane_table", emptyText: "（无表）" }),
      view: tree.addGroup({ title: "视图", icon: "info_pane_view", emptyText: "（无视图）" }),
    };
    for (const item of tableSelection) {
      const handle = tree.addItem(groups[item.kind], {
        name: item.source,
        icon: item.kind === "view" ? "info_pane_view" : "info_pane_table",
        checked: item.checked?.checked ?? item.kind === "table",
        ariaLabel: `同步${item.kind === "view" ? "视图" : "表"} ${item.source}`,
        extra: item.target,
      });
      item.checked = handle.input;
    }
    const wrap = document.createElement("div");
    wrap.style.cssText = "display:flex;flex-direction:column;min-height:0;height:100%;";
    const search = textInput(tableFilter);
    search.type = "search";
    search.placeholder = "搜索表 / 视图";
    search.setAttribute("aria-label", "搜索要同步的表与视图");
    search.addEventListener("input", () => { tableFilter = search.value; tree.filter(tableFilter); });
    wrap.append(field("搜索:", search), tree.el);
    host.replaceChildren(wrap);
    tree.filter(tableFilter);
  }
  /** 第 3 步（映射确认）的前置校验：进入执行日志页与「重新执行」共用。 */
  function validateMapping(): string | null {
    if (isDbMode()) {
      if (tablesLoading) return "表清单仍在加载，请稍候。";
      if (tableSelectionError) return `源库表清单加载失败，不能同步：${tableSelectionError}`;
      if (tableSelectionSource !== JSON.stringify([src.conn.value, src.schema.value])) return "源库已变化，请重新加载并核对当前源库的表清单。";
      if (!tableSelection.some(t => t.checked?.checked)) return "请至少勾选一个要同步的表或视图。";
      const names = new Set<string>();
      for (const item of tableSelection.filter(t => t.checked?.checked)) {
        const key = item.target.value.trim();
        if (names.has(key.toLocaleLowerCase())) return `目标表名重复：${key}。请修改目标表名或取消多余的表。`;
        names.add(key.toLocaleLowerCase());
      }
      return null;
    }
    if (mapLoading) return "列映射仍在加载，请稍候。";
    if (!mappings.some((m) => m.target.value.trim())) return "至少映射一列；可点「刷新映射」重试。";
    return null;
  }
  // 配置改动后才允许自动重跑：从执行日志页返回修改配置，再进入即按新配置执行。
  let pendingRun = true;

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
            const pair = document.createElement("div");
            pair.className = "form-source-pair";
            pair.append(src.box, dst.box);
            host.append(field("同步方式:", syncKind), pair);
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
            host.classList.add("sync-map-panel");
            const mapWrap = document.createElement("div");
            mapWrap.className = "sync-map";
            // 映射表自身滚动（flex:1 + min-height:0），工具条不再叠在溢出的表格行上
            const mapScroll = document.createElement("div");
            mapScroll.className = "sync-map-scroll";
            mapScroll.append(mapping);
            mapWrap.append(mapScroll, status);
            host.append(mapWrap);
            const dbTables = document.createElement("div"); dbTables.id = "sync-db-tables";
            dbTables.style.cssText = "display:flex;flex-direction:column;min-height:0;max-height:340px;margin-top:8px;";
            host.appendChild(dbTables);
            // 表头工具行在映射表上方：刷新只作用于上方的映射表，与写入选项无关。
            const mapHead = document.createElement("div");
            mapHead.className = "sync-table-only sync-map-head";
            const mapTitle = document.createElement("span");
            mapTitle.className = "sync-map-title";
            mapTitle.textContent = "列映射";
            const refresh = document.createElement("button");
            refresh.type = "button";
            refresh.className = "btn";
            refresh.textContent = "刷新映射";
            refresh.addEventListener("click", () => {
              if (!busy) void refreshMapping();
            });
            mapHead.append(mapTitle, refresh);
            host.insertBefore(mapHead, mapWrap);
            // 表对表：执行前可先比较源表与目标表的差异（只读）。
            const diffBtn = document.createElement("button");
            diffBtn.type = "button";
            diffBtn.className = "btn";
            diffBtn.textContent = "比较差异"; diffBtn.dataset.diff = "true";
            diffBtn.addEventListener("click", () => {
              if (busy || mapLoading) return;
              void runDiff();
            });
            const structPreviewBtn = document.createElement("button");
            structPreviewBtn.type = "button"; structPreviewBtn.className = "btn";
            structPreviewBtn.textContent = "预览结构差异";
            structPreviewBtn.dataset.structurePreview = "true";
            structPreviewBtn.addEventListener("click", () => { if (!busy && !mapLoading) void previewStructure(); });
            const structApplyBtn = document.createElement("button");
            structApplyBtn.type = "button"; structApplyBtn.className = "btn";
            structApplyBtn.textContent = "应用结构差异"; structApplyBtn.dataset.structureApply = "true"; structApplyBtn.disabled = !structureSession.canApply;
            structApplyBtn.title = "请先按当前目标与映射预览结构差异";
            structApplyBtn.addEventListener("click", () => { void applyStructure(); });
            const diffControls = document.createElement("div");
            diffControls.className = "sync-diff-controls sync-table-only";
            diffControls.setAttribute("role", "group");
            diffControls.setAttribute("aria-label", "数据差异比较");
            const memoryLabel = document.createElement("label");
            memoryLabel.className = "sync-diff-memory-option";
            memoryLabel.title = "仅影响“比较差异”，不改变同步写入；预算足够时优先内存快路径，必要时才溢写。默认关闭仍使用保底预算，不是无限内存。";
            memoryLabel.append(memoryControl, document.createTextNode("内存预算控制"));
            diffControls.append(diffBtn, memoryLabel, memoryMode, memoryLimitLabel, memoryInfo);
            // 数据差异比较（只读）独占一行：比较按钮、内存预算与状态同组，不随结构按钮换行漂移。
            const diffRow = document.createElement("div");
            diffRow.className = "sync-table-only sync-map-diff-row";
            diffRow.append(diffControls);
            host.appendChild(diffRow);
            // 结构差异一行：预览/应用/停止与「允许删除多余列/索引」同组，因为该选项就属于结构应用请求。
            const structureRow = document.createElement("div");
            structureRow.className = "sync-table-only sync-structure-row";
            structureRow.append(structPreviewBtn, structApplyBtn, structureStop, allowDropLabel);
            host.appendChild(structureRow);
            host.appendChild(structureReview.el);
            // 增量配置：标题后紧跟「删除多余行」选项，业务键输入框独占下一行。
            const incrementalRow = document.createElement("div");
            incrementalRow.className = "sync-table-only sync-incremental-row";
            const bizLabel = document.createElement("label");
            bizLabel.className = "sync-incremental-key";
            bizLabel.htmlFor = bizKey.id;
            bizLabel.textContent = "增量业务键:";
            incrementalRow.append(bizLabel, deleteMissingLabel, bizKey);
            host.appendChild(incrementalRow);
          },
          validate: () => { const problem = validateMapping(); if (!problem) pendingRun = true; return problem; },
        },
        {
          label: "执行日志",
          hint: "同步按上一步确认的配置执行；每张表的结果、行数与错误都记在这一页。失败时点「上一步」修改配置后可重新执行。",
          render: (host) => {
            const head = document.createElement("div");
            head.className = "sync-log-head";
            const title = document.createElement("strong");
            title.textContent = "同步执行记录";
            const actions = document.createElement("div");
            actions.className = "sync-log-actions";
            const rerun = document.createElement("button");
            rerun.type = "button"; rerun.className = "btn"; rerun.textContent = "重新执行";
            rerun.addEventListener("click", () => { if (busy) return; pendingRun = true; void wizard.next(); });
            const back = document.createElement("button");
            back.type = "button"; back.className = "btn"; back.textContent = "上一步（修改配置）";
            back.addEventListener("click", () => { if (!busy) wizard.go(2); });
            actions.append(rerun, back);
            head.append(title, actions);
            const page = document.createElement("div");
            page.className = "sync-log-page";
            page.append(head, logStatus, logProgressHost, logSummary, runLog);
            host.append(page);
          },
        },
      ],
      onEnter: (index) => {
        if (index === 1) updateKindUi();
        if (index === 2) {
          if (isDbMode()) { mapping.style.display = "none"; void loadTableSelection(); }
          else { mapping.style.display = ""; void refreshMapping(); }
          const dbTables = dlg.body.querySelector<HTMLElement>("#sync-db-tables");
          if (dbTables) dbTables.style.display = isDbMode() ? "flex" : "none";
          // 「比较差异」仅表对表模式提供；整库的逐表比较由增量同步本身完成，其所在分组按 .sync-table-only 整体隐藏。
          for (const el of dlg.body.querySelectorAll<HTMLElement>(".sync-table-only")) {
            el.style.display = isDbMode() ? "none" : "";
          }
          updateActionAvailability();
          // 这一页是提交入口：按钮文案直接写动作，避免「下一步」被当成单纯翻页。
          wizard.nextBtn.textContent = "开始同步";
        }
        // 进入执行日志页即按当前配置执行；执行过后只显示结果，由「重新执行」显式重跑。
        if (index === 3 && pendingRun && !busy) { void wizard.next(); }
      },
      finishLabel: "完成",
      onFinish: async () => {
        if (structureSession.busy || structureSession.effectsUnknown) { wizard.showError("结构执行或未知写入尚未核对；请先停止并明确重新预览双端结构，不能启动另一项同步。"); return; }
        if (busy) return;
        // 最后一页是执行日志：刚进入时按当前配置执行；已执行过则「完成」只关闭窗口。
        if (!pendingRun) { dlg.close(); return; }
        const mappingProblem = validateMapping();
        if (mappingProblem) { wizard.showError(mappingProblem); return; }
        pendingRun = false;
        if (isDbMode()) {
          const tables = tableSelection.filter(t => t.checked?.checked).map(t => ({ source: t.source, target: t.target.value.trim(), view: t.kind === "view" }));
          if (!tables.length) { wizard.showError("请至少勾选一个要同步的表或视图"); return; }
          if (dbWriteMode.value === "replace" || dbWriteMode.value === "recreate") {
            const action = dbWriteMode.value === "recreate" ? "删除并重建同名表" : "清空同名表数据";
            if (!window.confirm(`整库同步将对目标库 ${dst.schema.value} 中所有同名已存在表执行「${action}」。源表不受影响，确认继续？`)) return;
          }
          busy = true; wizard.setBusy(true);
          logClear();
          const dbSyncHandle = taskCenter.register("sync", `整库同步 · ${src.schema.value} → ${dst.schema.value}`);
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
            dbSyncHandle.finish(`成功 ${okCount} · 跳过 ${skippedCount} · 失败 ${failedCount} · 共 ${rowsTotal} 行`);
            logClear();
            for (const r of reports) {
              const label = `${r.source_table} → ${r.target_table}`;
              if (r.skipped) logAdd("skip", label, "目标已存在，已跳过");
              else if (r.error) logAdd("error", label, `失败：${r.error}`);
              else logAdd("ok", label, `${r.rows} 行${r.created ? "，已建表" : ""}${r.warnings.length ? ` ⚠ ${r.warnings.join("；")}` : ""}`);
            }
            const lines = reports.map(r => `${r.source_table} → ${r.target_table}${r.skipped ? "：目标已存在，已跳过" : r.error ? `：失败（${r.error}）` : `：${r.rows} 行${r.created ? "，已建表" : ""}`}${r.warnings.length ? ` ⚠ ${r.warnings.join("；")}` : ""}`);
            logSummary.textContent = `成功 ${okCount} · 跳过 ${skippedCount} · 失败 ${failedCount} · 共写入 ${rowsTotal} 行`;
            logStatus.textContent = `整库同步完成：成功 ${okCount} · 跳过 ${skippedCount} · 失败 ${failedCount}，共写入 ${rowsTotal} 行${failedCount ? `\n${lines.filter(l => l.includes("失败")).join("\n")}` : ""}`;
            status.textContent = "整库同步完成，逐表结果见执行日志页。";
            onDone({ rows: rowsTotal, created: reports.some(r => r.created), warnings: reports.flatMap(r => r.warnings) });
          } catch (e) {
            feedback.finish();
            dbSyncHandle.fail(String(e));
            logClear();
            logAdd("error", "整库同步失败", String(e));
            logSummary.textContent = "执行失败";
            logStatus.textContent = `整库同步失败：${String(e)}`;
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
        const targetName = (dst.table as HTMLInputElement).value.trim();
        const canCreateTarget = mode.value !== "data" && create.checked;
        if (!canCreateTarget && !dst.knownTables().includes(targetName)) {
          wizard.showError(`目标表「${targetName}」不存在；请从下拉选择已有表，或勾选「目标不存在时创建」。`);
          return;
        }
        if (writeMode.value !== "append" && !window.confirm(`${writeMode.value === "recreate" ? "删除并重建" : "清空"}目标 ${dst.table.value}？源表不受影响。`)) {
          return;
        }
        const incremental = syncStrategy.value === "incremental";
        const removeMissing = incremental && deleteMissing.checked;
        if (removeMissing && !window.confirm("增量同步已启用「删除目标多余行」：将删除目标中键不在源数据里的行，且不可恢复。确认继续？")) {
          return;
        }
        busy = true;
        wizard.setBusy(true);
        logClear();
        const tableSyncHandle = taskCenter.register("sync", `表同步 · ${src.table.value || src.schema.value}`);
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
            key_columns: bizKey.value.split(",").map((x) => x.trim()).filter(Boolean),
            delete_missing: removeMissing,
            create_target: mode.value !== "data" && create.checked,
            columns,
            where_clause: where.value.trim() || null,
          });
          feedback.finish();
          tableSyncHandle.finish(`同步 ${report.rows} 行${report.created ? "，已建表" : ""}`);
          logClear();
          logAdd("ok", `${src.table.value} → ${(dst.table as HTMLInputElement).value.trim()}`, `${report.rows} 行${report.created ? "，已建表" : ""}${report.warnings.length ? ` ⚠ ${report.warnings.join("；")}` : ""}`);
          logSummary.textContent = `成功 1 · 共写入 ${report.rows} 行${report.created ? " · 已建表" : ""}`;
          logStatus.textContent = `同步完成：${report.rows} 行${report.created ? "，已建表" : ""}${report.warnings.length ? `\n${report.warnings.join("\n")}` : ""}`;
          status.textContent = "同步完成，结果见执行日志页。";
          onDone(report);
        } catch (e) {
          feedback.finish();
          tableSyncHandle.fail(String(e));
          logClear();
          logAdd("error", `${src.table.value} → ${(dst.table as HTMLInputElement).value.trim()}`, String(e));
          logSummary.textContent = "执行失败";
          logStatus.textContent = `同步失败：${String(e)}`;
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
  dlg.el.classList.add("form-stacked");
  // 底部：向导自带的上一步/下一步（最后一页的执行日志用「完成」关闭）+ 关闭
  dlg.footer.append(...wizard.buttons(), (() => {
    const close = document.createElement("button");
    close.type = "button";
    close.className = "btn";
    close.textContent = "关闭";
    close.addEventListener("click", () => dlg.close());
    return close;
  })());
  const closeDialog = dlg.close.bind(dlg);
  dlg.close = () => {
    // 结构执行与「写入回执未知」必须留在窗口里核对证据，不能转后台。
    if (structureSession.busy || structureSession.effectsUnknown) { wizard.showError("结构任务尚未核对完成。请先停止任务；结构回执未知时请明确重新预览真实双端，保留当前证据。"); return; }
    // 数据同步进行中关闭窗口＝转后台：任务在任务中心继续，不取消、不丢失结果。
    if (busy) {
      if (!window.confirm("同步任务仍在执行。关闭窗口不会中断任务，它会转入后台继续，可在顶部任务中心查看进度与结果。确定关闭？")) return;
      feedback.detach();
    }
    closeDialog();
  };
  dlg.onClose(() => {
    structureClosed = true; ++tablesVersion; structureSession.dispose();
    ++diffGeneration; ++memoryPreviewGeneration;
    if (memoryPreviewTimer) clearTimeout(memoryPreviewTimer);
    memoryPreviewTimer = null;
  });
  const edited = () => { pendingRun = true; if (!structureSession.busy && !structureSession.effectsUnknown) { structurePreview = null; structureReview.clear(); structureSession.invalidate(); } };
  dlg.body.addEventListener("input", edited); dlg.body.addEventListener("change", edited);
  accessibleDialog(dlg, "数据库同步", previousFocus);
  const feedback = taskFeedback(dlg, logStatus, logProgressHost);
  updateKindUi();
  void Promise.all([src.load(), dst.load()]);
  return dlg;
}
