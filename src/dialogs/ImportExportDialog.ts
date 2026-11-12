/** 导入 / 导出向导：步骤条界面 + 真实执行。 */

import { api, transferTasks } from "../core/api";
import { formatError } from "../core/execution";
import { bindExportPath } from "../core/export-path";
import { namespaceLabel } from "../core/database-picker";
import { accessibleDialog, taskFeedback, openSyncDialog } from "./SyncDialog";
import type { ConnectionConfig, ExportOptions, ImportOptions } from "../core/types";
import { Dialog, field as dialogField, radioGroup, select, textInput } from "../ui/Dialog";
import { Wizard } from "../ui/Wizard";
import { TransferProgress } from "../ui/TransferProgress";
import { taskCenter, type TaskHandle } from "../core/backgroundTasks";
import type { TaskProgress } from "../core/api";

/** 此文件显式使用上置标签表单，保留原控件与监听，仅清理旧固定宽度。 */
function field(label: string, control: HTMLElement): HTMLElement {
  control.style.removeProperty("width");
  control.querySelectorAll<HTMLElement>("input,select,textarea").forEach(child => child.style.removeProperty("width"));
  return dialogField(label, control);
}

function formatFields(format: HTMLSelectElement, delimiter: HTMLInputElement, encoding: HTMLSelectElement): HTMLElement {
  const grid = document.createElement("div");
  grid.className = "form-grid form-grid--three";
  grid.append(field("格式:", format), field("分隔符:", delimiter), field("编码:", encoding));
  return grid;
}

function checkGroup(...labels: HTMLLabelElement[]): HTMLElement {
  const group = document.createElement("div");
  group.className = "form-check-group";
  group.append(...labels);
  return group;
}

/** 只读展示输入框（来源 / 汇总等）。 */
function roInput(value: string): HTMLInputElement {
  const input = textInput(value);
  input.readOnly = true;
  input.title = value;
  return input;
}

function mountTransferWizard(dlg: Dialog, wizard: Wizard): void {
  dlg.el.classList.add("form-stacked");
  dlg.body.style.padding = "0";
  dlg.body.style.display = "flex";
  dlg.body.style.flexDirection = "column";
  // 内容区不出横向滚动条：超宽内容在步内收缩（步骤条本身也会压缩步名）。
  dlg.body.style.overflow = "hidden";
  dlg.body.append(wizard.stepsEl, wizard.bodyEl);
  // 固定内容区高度：切换步骤时对话框尺寸不跳动，放不下的内容在步内滚动。
  wizard.bodyEl.style.flex = "1";
  wizard.bodyEl.style.minHeight = "0";
  wizard.bodyEl.style.overflow = "hidden";
  dlg.footer.prepend(...wizard.buttons());
  dlg.body.querySelector<HTMLInputElement>("input")?.focus();
}

const ENCODINGS = [
  { value: "utf-8", label: "UTF-8" },
  { value: "gbk", label: "GBK" },
  { value: "big5", label: "Big5" },
  { value: "latin1", label: "ISO-8859-1" },
];

/** 导出保存对话框的文件过滤器（随格式选择）。 */
const EXPORT_FILE_FILTERS: Record<string, { name: string; extensions: string[] }> = {
  csv: { name: "CSV 文件", extensions: ["csv"] },
  json: { name: "JSON 文件", extensions: ["json"] },
  sql: { name: "SQL 文件", extensions: ["sql"] },
};

/** 选择文件对话框（使用 Tauri 原生对话框）。 */
async function pickFile(
  title: string,
  filters: { name: string; extensions: string[] }[],
  defaultPath?: string,
): Promise<string | null> {
  const { open, save } = await import("@tauri-apps/plugin-dialog");
  const res = title.startsWith("保存") ? await save({ title, filters, defaultPath }) : await open({ title, multiple: false, filters });
  if (res === null || typeof res === "string") return res;
  throw new Error("文件对话框返回了无法识别的路径；原输入已保留");
}

/** 导入向导。 */
export function openImportWizard(
  connection: ConnectionConfig,
  schema: string,
  table: string,
  onDone: (count: number) => void,
): Dialog {
  const steps = ["源", "目标", "映射", "检查", "执行进度"];
  let wizard!: Wizard;
  let running = false;
  let started = false;
  let feedback: ReturnType<typeof taskFeedback>;
  /** 后台任务句柄（runImport 中创建，taskFeedback 的进度回调联动刷新）。 */
  let taskHandle: TaskHandle | null = null;

  const fileInput = textInput("", 320);
  const formatSelect = select(
    [
      { value: "csv", label: "CSV" },
      { value: "json", label: "JSON" },
      { value: "sql", label: "SQL" },
    ],
    "csv",
    120,
  );
  const delimInput = textInput(",", 60);
  const encodingSelect = select(ENCODINGS, "utf-8", 120);
  const headerInput = document.createElement("input");
  headerInput.type = "checkbox";
  headerInput.checked = true;
  const truncateInput = document.createElement("input");
  truncateInput.type = "checkbox";
  const createInput = document.createElement("input");
  createInput.type = "checkbox";

  // 预览卡片：标题行（列数/行数 + 刷新）+ 固定表头表格；空态、加载态、错误态都在同一卡片内。
  const preview = document.createElement("div");
  preview.className = "form-preview import-preview";
  preview.setAttribute("aria-label", "CSV 数据预览");
  const previewBar = document.createElement("div");
  previewBar.className = "import-preview-bar";
  const previewTitle = document.createElement("span");
  previewTitle.className = "import-preview-title";
  previewTitle.textContent = "数据预览";
  const previewMeta = document.createElement("span");
  previewMeta.className = "import-preview-meta";
  previewMeta.setAttribute("role", "status");
  previewMeta.textContent = "未选择文件";
  const previewRefresh = document.createElement("button");
  previewRefresh.type = "button";
  previewRefresh.className = "import-preview-refresh";
  previewRefresh.textContent = "刷新";
  previewRefresh.title = "重新读取预览";
  previewRefresh.addEventListener("click", () => void refreshPreview());
  previewBar.append(previewTitle, previewMeta, previewRefresh);
  const previewBody = document.createElement("div");
  previewBody.className = "import-preview-body";
  preview.append(previewBar, previewBody);

  const status = document.createElement("div");
  status.style.marginTop = "6px";
  status.style.minHeight = "18px";
  status.style.color = "var(--fg-muted)";

  const panels: HTMLElement[] = [];

  // 第 1 步：源
  const p1 = document.createElement("div");
  const browseBtn = document.createElement("button");
  browseBtn.className = "btn";
  browseBtn.style.minWidth = "0";
  browseBtn.textContent = "浏览...";
  browseBtn.addEventListener("click", async () => {
    try {
      const f = await pickFile("选择导入文件", [
        { name: "CSV 文件", extensions: ["csv"] },
        { name: "JSON 文件", extensions: ["json"] },
        { name: "SQL 文件", extensions: ["sql"] },
        { name: "所有文件", extensions: ["*"] },
      ]);
      if (f) {
        // 按扩展名自动匹配导入格式（.csv → csv、.json → json、.sql → sql）
        const ext = f.slice(f.lastIndexOf(".") + 1).toLowerCase();
        if (ext === "csv" || ext === "json" || ext === "sql") formatSelect.value = ext;
        fileInput.value = f;
        fileInput.dispatchEvent(new Event("change"));
      }
    } catch (error) { console.error("选择导入文件失败", error); dlg.showError(formatError(error)); }
  });
  const fileWrap = document.createElement("div");
  fileWrap.className = "form-control-line";
  fileWrap.appendChild(fileInput);
  fileWrap.appendChild(browseBtn);
  p1.appendChild(field("文件名:", fileWrap));
  p1.appendChild(formatFields(formatSelect, delimInput, encodingSelect));
  const headerLabel = document.createElement("label");
  headerLabel.className = "checkbox";
  headerLabel.appendChild(headerInput);
  const headerText = document.createElement("span");
  headerText.textContent = "第一行包含列名";
  headerLabel.appendChild(headerText);
  p1.appendChild(checkGroup(headerLabel));
  panels.push(p1);

  let previewRequest = 0;
  function showPreviewMessage(message: string, error = false): void {
    const note = document.createElement("div");
    note.className = "import-preview-message";
    note.classList.toggle("is-error", error);
    note.textContent = message;
    previewBody.replaceChildren(note);
  }
  async function refreshPreview(): Promise<void> {
    const request = ++previewRequest, file = fileInput.value.trim();
    preview.setAttribute("aria-busy", "false");
    previewRefresh.disabled = !file || formatSelect.value !== "csv";
    if (!file) { previewMeta.textContent = "未选择文件"; showPreviewMessage("选择 CSV 文件后，在此预览前 10 行数据。"); return; }
    if (formatSelect.value !== "csv") { previewMeta.textContent = "仅支持 CSV"; showPreviewMessage("JSON 和 SQL 文件不提供表格预览，可继续下一步。"); return; }
    preview.setAttribute("aria-busy", "true");
    previewRefresh.disabled = true;
    previewMeta.textContent = "正在读取…";
    showPreviewMessage("正在读取 CSV 数据…");
    const hasHeader = headerInput.checked;
    try {
      const [headers, samples] = await api.previewCsv(file, delimInput.value || ",", encodingSelect.value, 10);
      // 文件或解析参数改变时，迟到响应不能覆盖最新预览。
      if (request !== previewRequest) return;
      const names = hasHeader ? headers : headers.map((_, i) => "column" + (i + 1));
      const rows = hasHeader ? samples : [headers, ...samples].slice(0, 10);
      previewMeta.textContent = names.length + " 列 · 前 " + rows.length + " 行";
      if (!names.length) { showPreviewMessage("文件为空，没有可预览的数据。"); return; }
      const grid = document.createElement("table");
      grid.className = "mini-table import-preview-table";
      grid.setAttribute("aria-label", "CSV 数据预览");
      grid.style.minWidth = (44 + names.length * 140) + "px";
      const columns = document.createElement("colgroup"), rowNumber = document.createElement("col");
      rowNumber.className = "import-preview-index";
      columns.append(rowNumber, ...names.map((name, index) => {
        const col = document.createElement("col");
        // 短字段留紧凑列宽，最后一列吸收剩余空间；不按内容无限撑宽。
        const longest = Math.max(name.length, ...rows.map(row => (row[index] ?? "").length));
        if (index < names.length - 1 && longest <= 12) col.style.width = "120px";
        return col;
      }));
      const head = document.createElement("thead"), headerRow = document.createElement("tr");
      for (const [i, name] of ["#", ...names].entries()) {
        const th = document.createElement("th");
        th.scope = "col"; th.textContent = name; th.title = name;
        if (i === 0) th.setAttribute("aria-label", "预览行号");
        headerRow.append(th);
      }
      head.append(headerRow);
      const body = document.createElement("tbody");
      rows.forEach((row, index) => {
        const tr = document.createElement("tr"), number = document.createElement("th");
        number.scope = "row"; number.textContent = String(index + 1); tr.append(number);
        names.forEach((_, column) => {
          const value = row[column] ?? "", td = document.createElement("td");
          td.textContent = value || "空字符串";
          td.classList.toggle("is-empty", value === "");
          td.title = value || "空字符串（非 NULL）";
          tr.append(td);
        });
        body.append(tr);
      });
      grid.append(columns, head, body);
      previewBody.replaceChildren(grid);
      if (!rows.length) {
        const empty = document.createElement("div"); empty.className = "import-preview-message";
        empty.textContent = "只有列名，没有数据行。"; previewBody.append(empty);
      }
    } catch (error) {
      if (request !== previewRequest) return;
      previewMeta.textContent = "读取失败";
      showPreviewMessage("预览失败：" + formatError(error), true);
    } finally {
      if (request === previewRequest) { preview.setAttribute("aria-busy", "false"); previewRefresh.disabled = false; }
    }
  }
  for (const control of [fileInput, delimInput, formatSelect, encodingSelect, headerInput]) control.addEventListener("change", () => void refreshPreview());
  void refreshPreview();

  // 第 2 步：目标（数据库与表都从真实元数据中选择；新建表需显式勾选）
  const targetSchema = select([{ value: schema, label: schema || "（默认）" }], schema, 220);
  targetSchema.setAttribute("aria-label", `目标${namespaceLabel(connection)}`);
  const targetTable = document.createElement("select");
  targetTable.append(new Option("正在读取表…", ""));
  let tableLoading = true, tableRequest = 0;
  targetTable.setAttribute("aria-label", "目标表");
  const tableStatus = document.createElement("div");
  tableStatus.className = "form-note";
  const tableControl = document.createElement("div");
  tableControl.append(targetTable, tableStatus);
  const newTableInput = textInput("", 240);
  newTableInput.placeholder = "新表名";
  const newTableField = field("新表名:", newTableInput);
  const createLabel = document.createElement("label");
  createLabel.className = "checkbox";
  createLabel.appendChild(createInput);
  const createText = document.createElement("span");
  createText.textContent = "目标表不存在时创建";
  createLabel.appendChild(createText);
  const p2 = document.createElement("div");
  p2.appendChild(field("连接:", roInput(connection.name)));
  p2.appendChild(field(`${namespaceLabel(connection)}:`, targetSchema));
  p2.appendChild(field("表:", tableControl));
  p2.appendChild(newTableField);
  p2.appendChild(createLabel);
  panels.push(p2);

  const targetTableName = (): string => createInput.checked ? newTableInput.value.trim() : targetTable.value;
  const targetProblem = (): string | null => {
    if (tableLoading && !createInput.checked) return "目标表仍在读取，请稍后再试";
    if (!targetTableName()) return "请选择目标表或勾选新建并填写表名";
    return null;
  };
  const syncTableMode = (): void => {
    targetTable.disabled = createInput.checked || tableLoading;
    tableControl.style.display = createInput.checked ? "none" : "";
    newTableField.style.display = createInput.checked ? "" : "none";
  };
  createInput.addEventListener("change", syncTableMode);
  syncTableMode();

  /** 读取目标连接的真实库与表清单，供下拉选择。 */
  const loadSchemaChoices = async (): Promise<void> => {
    targetSchema.disabled = true;
    try {
      const databases = (await api.listDatabases(connection.id)).map(([name]) => name);
      if (databases.length) {
        targetSchema.replaceChildren(...databases.map(name => new Option(name, name)));
        targetSchema.value = databases.includes(schema) ? schema : databases[0];
        tableStatus.textContent = "";
      } else tableStatus.textContent = "当前连接没有可用数据库或模式。";
    } catch (reason) { tableStatus.textContent = `数据库元数据读取失败：${String(reason)}`; }
    targetSchema.disabled = false;
    await loadTableChoices();
  };
  const loadTableChoices = async (): Promise<void> => {
    const request = ++tableRequest, selectedSchema = targetSchema.value;
    tableLoading = true;
    targetTable.replaceChildren(new Option("正在读取表…", ""));
    syncTableMode(); refreshSummary();
    tableStatus.textContent = "正在读取表…";
    try {
      const tables = (await api.listTables(connection.id, selectedSchema)).filter(([, type]) => type === "table").map(([name]) => name);
      if (request !== tableRequest) return;
      targetTable.replaceChildren(...(tables.length ? tables.map(name => new Option(name, name)) : [new Option("（无可用表）", "")]));
      targetTable.value = selectedSchema === schema && tables.includes(table) ? table : (tables[0] ?? "");
      tableStatus.textContent = tables.length ? "" : "当前数据库或模式没有可用表，可勾选新建表。";
    } catch (reason) {
      if (request !== tableRequest) return;
      targetTable.replaceChildren(new Option("读取失败", ""));
      tableStatus.textContent = "表元数据读取失败：" + formatError(reason);
    } finally {
      if (request === tableRequest) {
        tableLoading = false; syncTableMode();
        // 程序赋值不触发 change，异步完成后显式更新检查页。
        refreshSummary();
      }
    }
  };
  targetSchema.addEventListener("change", () => { void loadTableChoices(); });
  void loadSchemaChoices();

  // 第 3 步：映射
  const p3 = document.createElement("div");
  const mapNote = document.createElement("div");
  mapNote.style.marginBottom = "8px";
  mapNote.style.color = "var(--fg-muted)";
  mapNote.textContent =
    "按列名自动匹配；未匹配列写入 NULL。";
  p3.appendChild(mapNote);
  const mapTable = document.createElement("table");
  mapTable.className = "mini-table";
  let mappingInputs: { source: string; target: HTMLSelectElement }[] = [];
  let mappingError = "", mappingLoading = false;
  const mapRefresh = document.createElement("button");
  mapRefresh.className = "btn"; mapRefresh.textContent = "刷新列映射";
  mapRefresh.addEventListener("click", () => void refreshMapping());
  p3.append(mapRefresh);
  for (const control of [fileInput, delimInput, formatSelect, encodingSelect, headerInput]) control.addEventListener("change", () => { mappingInputs = []; mapTable.replaceChildren(); });
  targetTable.addEventListener("change", () => { mappingInputs = []; mapTable.replaceChildren(); tableStatus.textContent = ""; });
  async function refreshMapping() {
    if (mappingLoading) return;
    if (formatSelect.value !== "csv") { mapNote.textContent = formatSelect.value === "sql" ? "SQL 按脚本原始目标执行，不使用本向导列映射。" : "JSON 按对象键名自动匹配，未指定的目标列保留默认值。"; return; }
    mappingLoading = true;
    try {
      const [headers] = await api.previewCsv(fileInput.value.trim(), delimInput.value, encodingSelect.value, 10);
      const names = headerInput.checked ? headers : headers.map((_, i) => `column${i + 1}`);
      let targets = names;
      try {
        const meta = await api.getTableSchema(connection.id, targetSchema.value || schema, targetTableName());
        if (!meta.columns.length) throw new Error("目标表列元数据为空，不能推断为新表");
        targets = meta.columns.map(c => c.name);
      } catch (error) {
        // 创建开关不是表不存在的证据；权限、网络及未知读取失败仍保持失败。
        if (!createInput.checked || (error as { code?: unknown } | null)?.code !== "TABLE_NOT_FOUND") throw error;
      }
      mappingInputs = []; mapTable.replaceChildren();
      const head = document.createElement("tr"); for (const title of ["源列", "目标列"]) { const th = document.createElement("th"); th.textContent = title; head.append(th); } mapTable.append(head);
      names.forEach((name, i) => {
        const target = select([{ value: "", label: "（跳过）" }, ...targets.map(n => ({ value: n, label: n }))], headerInput.checked ? targets.find(n => n.toLowerCase() === name.toLowerCase()) ?? "" : targets[i] ?? "", 220);
        target.setAttribute("aria-label", `${name} 对应目标列`);
        const tr = document.createElement("tr"), a = document.createElement("td"), b = document.createElement("td"); a.textContent = name; b.append(target); tr.append(a, b); mapTable.append(tr); mappingInputs.push({ source: name, target });
      });
      mappingError = "";
      mapNote.textContent = "可逐列映射或跳过；未映射目标列取默认值。";
    } catch (error) {
      console.error("导入映射读取失败，保留原映射且禁止导入", error);
      mappingError = formatError(error);
      mapNote.textContent = `映射读取失败，原映射已保留，不能导入：${mappingError}`;
      mapNote.setAttribute("role", "alert");
    } finally { mappingLoading = false; }
  }
  p3.appendChild(mapTable);
  panels.push(p3);

  // 第 4 步：执行（目标只读回显，避免与第 2 步重复填写）
  const p4 = document.createElement("div");
  const targetSummary = roInput("");
  targetSummary.setAttribute("aria-label", "已选择的目标表");
  function refreshSummary(): void {
    targetSummary.value = targetTableName();
    targetSummary.title = targetSummary.value ? connection.name + " / " + targetSchema.value + " / " + targetSummary.value : "";
    targetSummary.placeholder = createInput.checked ? "请填写新表名" : tableLoading ? "正在读取目标表…" : "未选择目标表";
  }
  targetTable.addEventListener("change", refreshSummary);
  targetSchema.addEventListener("change", refreshSummary);
  newTableInput.addEventListener("input", refreshSummary);
  newTableInput.addEventListener("change", refreshSummary);
  createInput.addEventListener("change", refreshSummary);
  refreshSummary();
  p4.appendChild(field("目标表:", targetSummary));
  const truncateLabel = document.createElement("label");
  truncateLabel.className = "checkbox";
  truncateLabel.appendChild(truncateInput);
  const truncateText = document.createElement("span");
  truncateText.textContent = "导入前清空目标表";
  truncateLabel.appendChild(truncateText);
  const conflictSelect = select(
    [
      { value: "", label: "遇重复记录时报错（默认）" },
      { value: "skip", label: "跳过重复记录" },
      { value: "update", label: "按冲突键更新已有记录" },
    ],
    "",
    220,
  );
  conflictSelect.setAttribute("aria-label", "记录冲突策略");
  const conflictCols = textInput("");
  conflictCols.setAttribute("placeholder", "冲突键列（逗号分隔，留空用主键）");
  conflictCols.setAttribute("aria-label", "冲突键列");
  p4.append(
    checkGroup(truncateLabel),
    field("记录冲突:", conflictSelect),
    field("冲突键列:", conflictCols),
  );
  const p4Note = document.createElement("p");
  p4Note.className = "transfer-hint";
  p4Note.textContent = "下一步开始导入，执行进度在下一页。";
  p4.appendChild(p4Note);
  panels.push(p4);

  // 第 5 步：执行进度（一次数据库往返没有逐行日志，用进度条 + 结果状态呈现）
  const p5 = document.createElement("div");
  const progress = new TransferProgress("导入执行进度");
  status.className = "transfer-log-status";
  progress.el.append(status);
  p5.append(progress.el);
  panels.push(p5);

  function showStep(i: number): void {
    wizard.go(i);
  }

  const runImport = async (): Promise<boolean> => {
          if (running) return false;
          const file = fileInput.value.trim();
          if (!file) {
            dlg.showError("请选择要导入的文件");
            return false;
          }
          if (formatSelect.value === "csv" && (mappingLoading || mappingError)) { dlg.showError(mappingLoading ? "列映射仍在读取，不能导入" : `列映射读取失败，不能导入：${mappingError}`); return false; }
          if (formatSelect.value === "csv" && mappingInputs.length && mappingInputs.every(m => !m.target.value)) { dlg.showError("至少映射一列，不能全部跳过"); return false; }
          if (connection.read_only) { dlg.showError("目标连接为只读"); return false; }
          if (tableLoading && !createInput.checked) { dlg.showError("目标表仍在读取，请稍后再试"); return false; }
          if (!targetTableName()) { dlg.showError("请选择目标表或勾选新建并填写表名"); return false; }
          if (formatSelect.value === "sql" && (truncateInput.checked || createInput.checked)) { dlg.showError("SQL脚本不能使用清空/自动建表选项"); return false; }
          if (truncateInput.checked && !window.confirm(`清空目标表 ${targetTableName()} 后导入？`)) return false;
          started = true;
          showStep(4);
          running = true;
          wizard.setBusy(true);
          progress.begin("正在导入，等待数据库返回…");
          const taskId = feedback.begin();
          status.textContent = "正在导入...";
          const targetName = targetTableName() || table;
          taskHandle = taskCenter.register("import", `导入 · ${targetName}`);
          const opts: ImportOptions = {
            connection_id: connection.id,
            schema: targetSchema.value || schema,
            table: targetName,
            file,
            format: formatSelect.value,
            has_header: headerInput.checked,
            delimiter: delimInput.value || ",",
            encoding: encodingSelect.value,
            create_table: createInput.checked,
            truncate: truncateInput.checked,
            sheet: null,
            on_conflict: conflictSelect.value,
            conflict_columns: conflictCols.value.split(",").map((x) => x.trim()).filter(Boolean),
          };
          try {
            const n = await transferTasks.import({ ...opts, task_id: taskId, column_mapping: formatSelect.value === "csv" ? mappingInputs.filter(m => m.target.value).map(m => ({ source: m.source, target: m.target.value })) : [] });
            feedback.finish();
            taskHandle?.finish(`共导入 ${n} 行`, file);
            taskHandle = null;
            status.textContent = `导入完成，共 ${n} 行。`;
            progress.finish(status.textContent ?? "已完成");
            onDone(n);
          } catch (e) {
            feedback.finish();
            taskHandle?.fail(String(e));
            taskHandle = null;
            status.textContent = `导入失败：${String(e)}`;
            progress.fail(status.textContent ?? "执行失败");
            dlg.showError(String(e));
          } finally { running = false; wizard.setBusy(false); }
          return false;
  };
  const dlg = new Dialog({ title: "导入向导", width: 820, height: 620, render: () => {}, buttons: [{ label: "关闭" }] });
  wizard = new Wizard({
    // 不再逐步堆说明文案；必要提示留在各自表单旁。
    steps: steps.map((label, index) => ({ label, render: host => {
      host.appendChild(panels[index]);
      if (index === 0) { host.classList.add("import-source-step"); host.appendChild(preview); }
    }, validate: index === 1 ? targetProblem : undefined })),
    onEnter: index => {
      if (index === 2) void refreshMapping();
      if (index === 3) refreshSummary();
      // 进入「执行日志」页即开始导入，日志与配置分页显示。
      if (index === 4 && !started) void runImport();
    },
    finishLabel: "完成",
    onFinish: async () => {
      if (running) return;
      if (!started) { await runImport(); return; }
      dlg.close();
    },
  });
  mountTransferWizard(dlg, wizard);
  accessibleDialog(dlg, "导入向导");
  // 后台任务中心联动：进度经 taskFeedback 轮询转发给 runImport 里的任务句柄
  // 原生 <progress> 不再占用步骤内容区；自建进度条统一呈现执行状态。
  const progressSink = document.createElement("div"); progressSink.hidden = true;
  feedback = taskFeedback(dlg, status, progressSink, (p: TaskProgress) => {
    progress.update(p.processed);
    taskHandle?.update({ done: p.processed, total: p.total || undefined, message: p.message });
  });
  return dlg;
}

/** 导出向导：源与范围 → 输出格式 → 保存位置 → 执行。 */
export function openExportWizard(
  connection: ConnectionConfig,
  schema: string,
  table: string,
  onDone: (count: number) => void,
): Dialog {
  const steps = ["源与范围", "输出格式", "保存位置", "检查并执行", "执行进度"];
  let wizard!: Wizard;
  let running = false;
  let started = false;
  let feedback: ReturnType<typeof taskFeedback>;
  /** 后台任务句柄（runExport 中创建，进度回调联动）。 */
  let exportTaskHandle: TaskHandle | null = null;

  const fileInput = textInput("");
  const formatSelect = select(
    [
      { value: "csv", label: "CSV" },
      { value: "json", label: "JSON" },
      { value: "sql", label: "SQL INSERT" },
    ],
    "csv",
  );
  const exportPath = bindExportPath(fileInput, `${schema}_${table}.csv`, formatSelect);
  const delimInput = textInput(",");
  const encodingSelect = select(ENCODINGS, "utf-8");
  const headerInput = document.createElement("input");
  headerInput.type = "checkbox";
  headerInput.checked = true;
  const whereInput = textInput("");
  // 限制行数：数字输入，可留空表示不限
  const limitInput = numberInputField();
  const status = document.createElement("div");
  status.style.marginTop = "6px";
  status.style.minHeight = "18px";
  status.style.color = "var(--fg-muted)";

  const panels: HTMLElement[] = [];

  // 第 1 步：源与范围（来源只读展示 + 筛选与行数限制）
  const p1 = document.createElement("div");
  p1.appendChild(field("连接:", roInput(connection.name)));
  p1.appendChild(field(`${namespaceLabel(connection)}:`, roInput(schema)));
  p1.appendChild(field("表:", roInput(table)));
  p1.appendChild(field("筛选条件 (WHERE):", whereInput));
  p1.appendChild(field("限制行数:", limitInput));
  panels.push(p1);

  // 第 2 步：输出格式
  const p2 = document.createElement("div");
  p2.appendChild(formatFields(formatSelect, delimInput, encodingSelect));
  const headerLabel = document.createElement("label");
  headerLabel.className = "checkbox";
  headerLabel.appendChild(headerInput);
  const headerText = document.createElement("span");
  headerText.textContent = "包含列名行";
  headerLabel.appendChild(headerText);
  p2.appendChild(checkGroup(headerLabel));
  panels.push(p2);

  // 第 3 步：保存位置（进入本步时文件名为空则按当前格式预填默认名）
  const p3 = document.createElement("div");
  const browseBtn = document.createElement("button");
  browseBtn.className = "btn";
  browseBtn.style.minWidth = "0";
  browseBtn.textContent = "浏览...";
  browseBtn.addEventListener("click", async () => {
    try {
      const filter = EXPORT_FILE_FILTERS[formatSelect.value] ?? EXPORT_FILE_FILTERS.csv;
      await exportPath.ready;
      const f = await pickFile("保存导出文件", [filter], fileInput.value);
      if (f) { fileInput.value = f; exportPath.refresh(); }
    } catch (error) { console.error("选择导出路径失败", error); dlg.showError(formatError(error)); }
  });
  const fileWrap = document.createElement("div");
  fileWrap.className = "form-control-line";
  fileWrap.appendChild(fileInput);
  fileWrap.appendChild(browseBtn);
  p3.appendChild(field("文件名:", fileWrap));
  panels.push(p3);

  // 第 4 步：执行（只读汇总 + 开始导出）
  const p4 = document.createElement("div");
  const sumSource = roInput("");
  const sumFormat = roInput("");
  const sumRows = roInput("");
  const sumFile = roInput("");
  p4.append(
    field("来源:", sumSource),
    field("输出格式:", sumFormat),
    field("行数与筛选:", sumRows),
    field("保存到:", sumFile),
  );
  const p4Note = document.createElement("p");
  p4Note.className = "transfer-hint";
  p4Note.textContent = "下一步开始导出，执行进度在下一页。";
  p4.appendChild(p4Note);
  panels.push(p4);

  // 第 5 步：执行进度（一次数据库往返没有逐行日志，用进度条 + 结果状态呈现）
  const p5 = document.createElement("div");
  const progress = new TransferProgress("导出执行进度");
  status.className = "transfer-log-status";
  const retryBtn = document.createElement("button");
  retryBtn.type = "button"; retryBtn.className = "btn"; retryBtn.textContent = "重新执行";
  retryBtn.hidden = true;
  retryBtn.addEventListener("click", () => { retryBtn.hidden = true; void runExport(); });
  progress.el.append(status);
  p5.append(progress.el, retryBtn);
  panels.push(p5);

  /** 进入执行步时刷新只读汇总。 */
  function refreshSummary(): void {
    sumSource.value = `${connection.name} / ${schema} / ${table}`;
    const formatLabel = formatSelect.selectedOptions[0]?.textContent ?? formatSelect.value;
    const parts = [formatLabel];
    if (formatSelect.value === "csv") parts.push(`分隔符 ${delimInput.value || ","}`);
    parts.push(`编码 ${encodingSelect.value}`, headerInput.checked ? "包含列名行" : "不含列名行");
    sumFormat.value = parts.join("，");
    const limitValue = limitInput.value.trim();
    const where = whereInput.value.trim();
    sumRows.value = `${limitValue ? `前 ${limitValue} 行` : "全部行"}${where ? `；WHERE ${where}` : ""}`;
    sumFile.value = fileInput.value.trim();
    for (const input of [sumSource, sumFormat, sumRows, sumFile]) input.title = input.value;
  }

  /** 范围类校验：WHERE 禁分号 / 注释，限制行数必须是非负整数。 */
  function rangeProblem(): string | null {
    if (/[;\0]|--|\/\*/.test(whereInput.value)) return "WHERE不能包含分号或注释";
    const limitValue = limitInput.value.trim();
    if (limitValue && (!/^\d+$/.test(limitValue) || !Number.isSafeInteger(Number(limitValue)))) return "限制行数必须是非负整数";
    return null;
  }

  const runExport = async (): Promise<boolean> => {
          if (running) return false;
          const file = fileInput.value.trim();
          if (!file) {
            dlg.showError("请指定导出文件");
            return false;
          }
          const fileError = exportPath.problem();
          if (fileError) { dlg.showError(fileError); return false; }
          const rangeError = rangeProblem();
          if (rangeError) { dlg.showError(rangeError); return false; }
          started = true; retryBtn.hidden = true; running = true; wizard.setBusy(true);
          progress.begin("正在导出，等待数据库返回…");
          const taskId = feedback.begin();
          status.textContent = "正在导出...";
          exportTaskHandle = taskCenter.register("export", `导出 · ${table}`);
          const opts: ExportOptions = {
            connection_id: connection.id,
            schema,
            table,
            file,
            format: formatSelect.value,
            has_header: headerInput.checked,
            delimiter: delimInput.value || ",",
            encoding: encodingSelect.value,
            where_clause: whereInput.value.trim() || null,
            limit: limitInput.value.trim() ? Number(limitInput.value) : null,
          };
          try {
            const n = await transferTasks.export({ ...opts, task_id: taskId });
            feedback.finish();
            exportTaskHandle?.finish(`共导出 ${n} 行`, file);
            exportTaskHandle = null;
            status.textContent = `导出完成，共 ${n} 行。`;
            progress.finish(status.textContent ?? "已完成");
            onDone(n);
          } catch (e) {
            feedback.finish();
            exportTaskHandle?.fail(String(e));
            exportTaskHandle = null;
            status.textContent = `导出失败：${String(e)}`;
            progress.fail(status.textContent ?? "执行失败");
            retryBtn.hidden = false;
            dlg.showError(String(e));
          } finally { running = false; wizard.setBusy(false); }
          return false;
  };
  const dlg = new Dialog({ title: "导出向导", width: 820, height: 620, render: () => {}, buttons: [{ label: "关闭" }] });
  wizard = new Wizard({
    // 每步不再堆说明文案：标题 + 表单即可。
    steps: steps.map((label, index) => ({
      label,
      render: host => host.appendChild(panels[index]),
      validate: index === 0 ? rangeProblem : index === 2 ? exportPath.problem : undefined,
    })),
    onEnter: index => {
      if (index === 2) { void exportPath.ready; exportPath.refresh(); }
      if (index === 3) refreshSummary();
      // 进入「执行日志」页即开始导出，进度与日志独占一页。
      if (index === 4 && !started) void runExport();
    },
    finishLabel: "完成",
    // 最后一步仍是真实提交入口：进入日志页会自动执行；未执行过时点「完成」等同于执行。
    onFinish: async () => {
      if (running) return;
      if (!started) { await runExport(); return; }
      dlg.close();
    },
  });
  mountTransferWizard(dlg, wizard);
  accessibleDialog(dlg, "导出向导");
  // 原生 <progress> 不再占用步骤内容区；自建进度条统一呈现执行状态。
  const progressSink = document.createElement("div"); progressSink.hidden = true;
  feedback = taskFeedback(dlg, status, progressSink, (p: TaskProgress) => {
    progress.update(p.processed);
    exportTaskHandle?.update({ done: p.processed, total: p.total || undefined, message: p.message });
  });
  return dlg;
}

/** 构造「限制行数」数字输入框：可留空表示不限（numberInput 需要初值，这里手动置空）。 */
function numberInputField(): HTMLInputElement {
  const input = document.createElement("input");
  input.type = "number";
  input.min = "0";
  input.step = "1";
  input.placeholder = "全部行";
  return input;
}

/** 查询结果导出向导（把当前结果集写出为 CSV / JSON / SQL）。 */
export function openResultExportDialog(
  connection: ConnectionConfig,
  onRun: (opts: ExportOptions) => Promise<number>,
  noteText?: string,
): Dialog {
  let running = false;
  let wizard!: Wizard;
  const formatSelect = select(
    [
      { value: "csv", label: "CSV" },
      { value: "json", label: "JSON" },
      { value: "sql", label: "SQL INSERT" },
    ],
    "csv",
  );
  const delimInput = textInput(",");
  const encodingSelect = select(ENCODINGS, "utf-8");
  const headerInput = document.createElement("input");
  headerInput.type = "checkbox";
  headerInput.checked = true;
  const fileInput = textInput("");
  const status = document.createElement("div");
  status.style.marginTop = "6px";
  status.style.minHeight = "18px";
  status.style.color = "var(--fg-muted)";

  // Filename timestamp is captured once; format changes only replace its extension.
  function defaultFileName(): string {
    const d = new Date();
    const pad = (n: number): string => String(n).padStart(2, "0");
    const stamp = `${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}-${pad(d.getHours())}${pad(d.getMinutes())}${pad(d.getSeconds())}`;
    return `查询结果_${stamp}.${formatSelect.value}`;
  }
  const exportPath = bindExportPath(fileInput, defaultFileName(), formatSelect);

  // 第 1 步：导出格式（结果集行列数在执行前无法得知，只做说明）
  const p1 = document.createElement("div");
  const note = document.createElement("div");
  note.style.color = "var(--fg-muted)";
  note.style.marginBottom = "8px";
  note.textContent = `将当前查询结果集导出为所选格式的文件。${noteText ? ` ${noteText}` : ""}`;
  p1.append(note, field("格式:", formatSelect), field("编码:", encodingSelect));

  // 第 2 步：保存位置
  const p2 = document.createElement("div");
  const delimField = field("分隔符:", delimInput);
  const headerLabel = document.createElement("label");
  headerLabel.className = "checkbox";
  headerLabel.appendChild(headerInput);
  const headerText = document.createElement("span");
  headerText.textContent = "包含列名行";
  headerLabel.appendChild(headerText);
  const browseBtn = document.createElement("button");
  browseBtn.className = "btn";
  browseBtn.style.minWidth = "0";
  browseBtn.textContent = "浏览...";
  browseBtn.addEventListener("click", async () => {
    try {
      const filter = EXPORT_FILE_FILTERS[formatSelect.value] ?? EXPORT_FILE_FILTERS.csv;
      await exportPath.ready;
      const f = await pickFile("保存导出文件", [filter], fileInput.value);
      if (f) { fileInput.value = f; exportPath.refresh(); }
    } catch (error) { console.error("选择导出路径失败", error); dlg.showError(formatError(error)); }
  });
  const fileWrap = document.createElement("div");
  fileWrap.className = "form-control-line";
  fileWrap.appendChild(fileInput);
  fileWrap.appendChild(browseBtn);
  p2.append(delimField, checkGroup(headerLabel), field("文件名:", fileWrap), status);

  // CSV-only options are hidden; path binding preserves the chosen basename.
  formatSelect.addEventListener("change", () => {
    delimField.hidden = formatSelect.value !== "csv";
  });

  /** 文件必填校验。 */
  function fileProblem(): string | null {
    return exportPath.problem();
  }

  const runExport = async (): Promise<void> => {
    if (running) return;
    const file = fileInput.value.trim();
    if (!file) {
      dlg.showError("请指定导出文件");
      return;
    }
    const fileError = fileProblem();
    if (fileError) { dlg.showError(fileError); return; }
    running = true;
    wizard.setBusy(true);
    p2.setAttribute("aria-busy", "true");
    status.textContent = "正在导出所选执行快照…（不会重新执行 SQL）";
    try {
      const n = await onRun({
        connection_id: connection.id,
        schema: "",
        table: "result",
        file,
        format: formatSelect.value,
        has_header: headerInput.checked,
        delimiter: delimInput.value || ",",
        encoding: encodingSelect.value,
        where_clause: null,
        limit: null,
      });
      status.textContent = `导出完成，共 ${n} 行。`;
    } catch (e) {
      status.textContent = `导出失败：${String(e)}`;
      dlg.showError(String(e));
    } finally { running = false; wizard.setBusy(false); p2.setAttribute("aria-busy", "false"); }
  };

  const dlg = new Dialog({ title: "导出结果", width: 640, height: 540, render: () => {}, buttons: [{ label: "关闭" }] });
  wizard = new Wizard({
    steps: [
      { label: "导出格式", hint: "选择当前结果集的导出格式与编码。", render: host => host.appendChild(p1) },
      { label: "保存位置", hint: "选择保存位置并开始导出。", render: host => host.appendChild(p2), validate: fileProblem },
    ],
    finishLabel: "开始导出",
    onFinish: async () => { await runExport(); },
  });
  mountTransferWizard(dlg, wizard);
  accessibleDialog(dlg, "导出结果");
  status.setAttribute("role", "status");
  status.setAttribute("aria-live", "polite");
  status.setAttribute("aria-atomic", "true");
  return dlg;
}

/** 数据传输对话框：源/目标均为 连接 → 库 → 表 级联选择（预填当前上下文）。 */
export function openTransferDialog(
  connections: ConnectionConfig[],
  source: ConnectionConfig,
  prefill: { schema: string; table: string },
  onRun: (opts: {
    source_connection: string;
    source_schema: string;
    source_table: string;
    target_connection: string;
    target_schema: string;
    target_table: string;
    create_target: boolean;
    drop_existing: boolean;
    copy_structure: boolean;
    copy_data: boolean;
    where_clause: string | null;
  }) => Promise<number>,
): Dialog {
  // Legacy entry point now uses the same safe heterogeneous synchronization dialog.
  void onRun;
  return openSyncDialog(connections, source, prefill);
}

/** SQL 转储对话框：转储内容单选 + DROP 选项 + 输出文件。 */
export function openDumpDialog(
  connection: ConnectionConfig,
  schema: string,
  table: string,
  onRun: (opts: {
    connection_id: string;
    schema: string;
    table: string;
    file: string;
    structure_only: boolean;
    data_only: boolean;
    include_drop: boolean;
  }) => Promise<number>,
): Dialog {
  let running = false;
  const fileInput = textInput("");
  const exportPath = bindExportPath(fileInput, `${schema}_${table || "database"}.sql`);
  const includeDropInput = document.createElement("input");
  includeDropInput.type = "checkbox";
  includeDropInput.checked = true;
  const status = document.createElement("div");
  status.style.marginTop = "8px";
  status.style.minHeight = "18px";
  status.style.color = "var(--fg-muted)";

  // 转储内容单选：数据和结构（默认）/ 仅结构 / 仅数据
  const contentRadios = radioGroup(
    "dump-content",
    [
      { value: "both", label: "数据和结构" },
      { value: "structure", label: "仅结构" },
      { value: "data", label: "仅数据" },
    ],
    "both",
  );
  /** 当前选中的转储内容。 */
  function currentMode(): string {
    return contentRadios.querySelector<HTMLInputElement>("input:checked")?.value ?? "both";
  }
  // 「包含 DROP TABLE」仅在 数据和结构 / 仅结构 时可用（仅数据无结构可删）
  function refreshDropState(): void {
    const disabled = currentMode() === "data";
    includeDropInput.disabled = disabled;
    if (disabled) includeDropInput.checked = false;
  }
  contentRadios.addEventListener("change", refreshDropState);

  const dropLabel = document.createElement("label");
  dropLabel.className = "checkbox";
  dropLabel.appendChild(includeDropInput);
  const dropText = document.createElement("span");
  dropText.textContent = "包含 DROP TABLE 语句";
  dropLabel.appendChild(dropText);

  const browse = document.createElement("button");
  browse.className = "btn";
  browse.style.minWidth = "0";
  browse.textContent = "浏览...";
  browse.addEventListener("click", async () => {
    try {
      await exportPath.ready;
      const f = await pickFile("保存 SQL 文件", [
        { name: "SQL 文件", extensions: ["sql"] },
      ], fileInput.value);
      if (f) { fileInput.value = f; exportPath.refresh(); }
    } catch (error) { console.error("选择 SQL 保存路径失败", error); dlg.showError(formatError(error)); }
  });
  const fileWrap = document.createElement("div");
  fileWrap.className = "form-control-line";
  fileWrap.appendChild(fileInput);
  fileWrap.appendChild(browse);

  const dlg = new Dialog({
    title: "SQL 转储",
    width: 520,
    render: (body) => {
      body.appendChild(field("连接:", roInput(connection.name)));
      body.appendChild(field(`${namespaceLabel(connection)}:`, roInput(schema)));
      body.appendChild(field("表:", roInput(table)));
      body.appendChild(field("转储内容:", contentRadios));
      body.appendChild(dropLabel);
      body.appendChild(field("输出文件:", fileWrap));
      body.appendChild(status);
    },
    buttons: [
      {
        label: "开始",
        primary: true,
        onClick: async () => {
          if (running) return false;
          const file = fileInput.value.trim();
          if (!file) {
            dlg.showError("请指定输出文件");
            return false;
          }
          const fileError = exportPath.problem();
          if (fileError) { dlg.showError(fileError); return false; }
          const mode = currentMode();
          running = true;
          status.textContent = "正在生成转储...";
          try {
            const n = await onRun({
              connection_id: connection.id,
              schema,
              table,
              file,
              structure_only: mode === "structure",
              data_only: mode === "data",
              include_drop: includeDropInput.checked,
            });
            status.textContent = `转储完成，共 ${n} 行数据。`;
          } catch (e) {
            status.textContent = `转储失败：${String(e)}`;
            dlg.showError(String(e));
          } finally { running = false; }
          return false;
        },
      },
      { label: "关闭" },
    ],
  });

  dlg.el.classList.add("form-stacked");
  accessibleDialog(dlg, "SQL 转储");
  status.setAttribute("role", "status");
  return dlg;
}
