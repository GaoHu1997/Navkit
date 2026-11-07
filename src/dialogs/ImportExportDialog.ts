/** 导入 / 导出向导：步骤条界面 + 真实执行。 */

import { api, transferTasks } from "../core/api";
import { accessibleDialog, taskFeedback, openSyncDialog } from "./SyncDialog";
import type { ConnectionConfig, ExportOptions, ImportOptions } from "../core/types";
import { Dialog, field, select, textInput } from "../ui/Dialog";

/** 步骤条控件。 */
function stepBar(titles: string[]): {
  el: HTMLElement;
  go: (index: number) => void;
} {
  const el = document.createElement("div");
  el.className = "wizard-steps";
  const nodes: HTMLElement[] = [];
  titles.forEach((t, i) => {
    if (i > 0) {
      const line = document.createElement("div");
      line.className = "wizard-step-line";
      el.appendChild(line);
    }
    const step = document.createElement("div");
    step.className = "wizard-step";
    const num = document.createElement("div");
    num.className = "wizard-step-num";
    num.textContent = String(i + 1);
    const label = document.createElement("div");
    label.textContent = t;
    step.appendChild(num);
    step.appendChild(label);
    el.appendChild(step);
    nodes.push(step);
  });
  return {
    el,
    go: (index: number) => {
      nodes.forEach((n, i) => {
        n.classList.toggle("active", i === index);
        n.classList.toggle("done", i < index);
      });
    },
  };
}

const ENCODINGS = [
  { value: "utf-8", label: "UTF-8" },
  { value: "gbk", label: "GBK" },
  { value: "big5", label: "Big5" },
  { value: "latin1", label: "ISO-8859-1" },
];

/** 选择文件对话框（使用 Tauri 原生对话框）。 */
async function pickFile(
  title: string,
  filters: { name: string; extensions: string[] }[],
): Promise<string | null> {
  try {
    const { open, save } = await import("@tauri-apps/plugin-dialog");
    const res = title.startsWith("保存") ? await save({ title, filters }) : await open({ title, multiple: false, filters });
    if (typeof res === "string") return res;
    return null;
  } catch {
    return null;
  }
}

/** 导入向导。 */
export function openImportWizard(
  connection: ConnectionConfig,
  schema: string,
  table: string,
  onDone: (count: number) => void,
): Dialog {
  const steps = ["源", "目标", "映射", "执行"];
  const bar = stepBar(steps);
  let step = 0;
  let running = false;
  let feedback: ReturnType<typeof taskFeedback>;

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

  const preview = document.createElement("div");
  preview.style.maxHeight = "180px";
  preview.style.overflow = "auto";
  preview.style.border = "1px solid var(--line)";
  preview.style.background = "#fff";

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
    const f = await pickFile("选择导入文件", [
      { name: "CSV 文件", extensions: ["csv"] },
      { name: "JSON 文件", extensions: ["json"] },
      { name: "SQL 文件", extensions: ["sql"] },
      { name: "所有文件", extensions: ["*"] },
    ]);
    if (f) {
      fileInput.value = f;
      fileInput.dispatchEvent(new Event("change"));
      await refreshPreview();
    }
  });
  const fileWrap = document.createElement("div");
  fileWrap.style.display = "flex";
  fileWrap.style.gap = "6px";
  fileWrap.appendChild(fileInput);
  fileWrap.appendChild(browseBtn);
  p1.appendChild(field("文件名:", fileWrap));
  p1.appendChild(field("格式:", formatSelect));
  p1.appendChild(field("分隔符:", delimInput));
  p1.appendChild(field("编码:", encodingSelect));
  const headerLabel = document.createElement("label");
  headerLabel.className = "checkbox";
  headerLabel.appendChild(headerInput);
  const headerText = document.createElement("span");
  headerText.textContent = "第一行包含列名";
  headerLabel.appendChild(headerText);
  p1.appendChild(headerLabel);
  panels.push(p1);

  async function refreshPreview(): Promise<void> {
    const f = fileInput.value.trim();
    preview.innerHTML = "";
    if (!f) return;
    if (formatSelect.value !== "csv") {
      const t = document.createElement("div");
      t.style.padding = "8px";
      t.style.color = "var(--fg-muted)";
      t.textContent = "仅 CSV 格式支持预览。";
      preview.appendChild(t);
      return;
    }
    try {
      const [names, rows] = await api.previewCsv(
        f,
        delimInput.value || ",",
        encodingSelect.value,
        10,
      );
      const table = document.createElement("table");
      table.className = "mini-table";
      const thead = document.createElement("thead");
      const tr = document.createElement("tr");
      for (const n of names) {
        const th = document.createElement("th");
        th.textContent = n;
        tr.appendChild(th);
      }
      thead.appendChild(tr);
      table.appendChild(thead);
      const tbody = document.createElement("tbody");
      for (const r of rows) {
        const rtr = document.createElement("tr");
        for (const c of r) {
          const td = document.createElement("td");
          td.textContent = c;
          rtr.appendChild(td);
        }
        tbody.appendChild(rtr);
      }
      table.appendChild(tbody);
      preview.appendChild(table);
    } catch (e) {
      const t = document.createElement("div");
      t.style.padding = "8px";
      t.style.color = "#c62828";
      t.textContent = `预览失败：${String(e)}`;
      preview.appendChild(t);
    }
  }
  fileInput.addEventListener("change", () => void refreshPreview());
  delimInput.addEventListener("change", () => void refreshPreview());
  formatSelect.addEventListener("change", () => void refreshPreview());

  // 第 2 步：目标
  const p2 = document.createElement("div");
  p2.appendChild(field("连接:", textInput(connection.name, 240)));
  p2.appendChild(field("模式:", textInput(schema, 240)));
  const targetTable = textInput(table, 240);
  p2.appendChild(field("表:", targetTable));
  panels.push(p2);

  // 第 3 步：映射
  const p3 = document.createElement("div");
  const mapNote = document.createElement("div");
  mapNote.style.marginBottom = "8px";
  mapNote.style.color = "var(--fg-muted)";
  mapNote.textContent =
    "列按名称自动匹配。若 CSV 第一行包含列名，则以表结构中的列名进行匹配；未匹配的列将写入 NULL。";
  p3.appendChild(mapNote);
  const mapTable = document.createElement("table");
  mapTable.className = "mini-table";
  let mappingInputs: { source: string; target: HTMLSelectElement }[] = [];
  const mapRefresh = document.createElement("button");
  mapRefresh.className = "btn"; mapRefresh.textContent = "刷新列映射";
  mapRefresh.addEventListener("click", () => void refreshMapping());
  p3.append(mapRefresh);
  for (const control of [fileInput, delimInput, formatSelect, encodingSelect, headerInput]) control.addEventListener("change", () => { mappingInputs = []; mapTable.replaceChildren(); });
  targetTable.addEventListener("change", () => { mappingInputs = []; mapTable.replaceChildren(); });
  async function refreshMapping() {
    mappingInputs = []; mapTable.replaceChildren();
    if (formatSelect.value !== "csv") { mapNote.textContent = formatSelect.value === "sql" ? "SQL 按脚本原始目标执行，不使用本向导列映射。" : "JSON 按对象键名自动匹配，未指定的目标列保留默认值。"; return; }
    try {
      const [headers] = await api.previewCsv(fileInput.value.trim(), delimInput.value, encodingSelect.value, 10);
      const names = headerInput.checked ? headers : headers.map((_, i) => `column${i + 1}`);
      let targets = names;
      try { const meta = await api.getTableSchema(connection.id, schema, targetTable.value.trim()); if (meta.columns.length) targets = meta.columns.map(c => c.name); } catch { if (!createInput.checked) throw new Error("目标表不可读取；新表请勾选目标不存在时创建"); }
      const head = document.createElement("tr"); for (const title of ["源列", "目标列"]) { const th = document.createElement("th"); th.textContent = title; head.append(th); } mapTable.append(head);
      names.forEach((name, i) => {
        const target = select([{ value: "", label: "（跳过）" }, ...targets.map(n => ({ value: n, label: n }))], headerInput.checked ? targets.find(n => n.toLowerCase() === name.toLowerCase()) ?? "" : targets[i] ?? "", 220);
        target.setAttribute("aria-label", `${name} 对应目标列`);
        const tr = document.createElement("tr"), a = document.createElement("td"), b = document.createElement("td"); a.textContent = name; b.append(target); tr.append(a, b); mapTable.append(tr); mappingInputs.push({ source: name, target });
      });
      mapNote.textContent = "可按列映射或跳过源列。未写入的目标列使用数据库默认值。CSV 的 \\N 表示 NULL，\\\\N 表示字面量 \\N，空字段保持空字符串。";
    } catch (e) { mapNote.textContent = `映射读取失败：${String(e)}`; }
  }
  p3.appendChild(mapTable);
  panels.push(p3);

  // 第 4 步：执行
  const p4 = document.createElement("div");
  p4.appendChild(field("目标表:", textInput(table, 240)));
  const truncateLabel = document.createElement("label");
  truncateLabel.className = "checkbox";
  truncateLabel.appendChild(truncateInput);
  const truncateText = document.createElement("span");
  truncateText.textContent = "导入前清空目标表";
  truncateLabel.appendChild(truncateText);
  p4.appendChild(truncateLabel);
  const createLabel = document.createElement("label");
  createLabel.className = "checkbox";
  createLabel.appendChild(createInput);
  const createText = document.createElement("span");
  createText.textContent = "目标表不存在时创建";
  createLabel.appendChild(createText);
  p4.appendChild(createLabel);
  p4.appendChild(status);
  panels.push(p4);

  function showStep(i: number): void {
    step = i;
    if (i === 2) void refreshMapping();
    bar.go(i);
    panels.forEach((p, idx) => {
      p.style.display = idx === i ? "" : "none";
    });
  }

  const dlg = new Dialog({
    title: "导入向导",
    width: 620,
    render: (body) => {
      body.appendChild(bar.el);
      const host = document.createElement("div");
      host.style.paddingTop = "10px";
      for (const p of panels) host.appendChild(p);
      body.appendChild(host);
      body.appendChild(document.createElement("div"));
      body.appendChild(field("预览:", preview));
      showStep(0);
    },
    buttons: [
      {
        label: "上一步",
        onClick: () => {
          if (step > 0) showStep(step - 1);
          return false;
        },
      },
      {
        label: "下一步",
        onClick: () => {
          if (step < 3) {
            showStep(step + 1);
            return false;
          }
          return false;
        },
      },
      {
        label: "开始导入",
        primary: true,
        onClick: async () => {
          if (running) return false;
          const file = fileInput.value.trim();
          if (!file) {
            dlg.showError("请选择要导入的文件");
            return false;
          }
          if (formatSelect.value === "csv" && mappingInputs.length && mappingInputs.every(m => !m.target.value)) { dlg.showError("至少映射一列，不能全部跳过"); return false; }
          if (connection.read_only) { dlg.showError("目标连接为只读"); return false; }
          if (!targetTable.value.trim()) { dlg.showError("请填写目标表"); return false; }
          if (formatSelect.value === "sql" && (truncateInput.checked || createInput.checked)) { dlg.showError("SQL脚本不能使用清空/自动建表选项"); return false; }
          if (truncateInput.checked && !window.confirm(`清空目标表 ${targetTable.value} 后导入？`)) return false;
          showStep(3);
          running = true;
          const taskId = feedback.begin();
          status.textContent = "正在导入...";
          const opts: ImportOptions = {
            connection_id: connection.id,
            schema,
            table: targetTable.value.trim() || table,
            file,
            format: formatSelect.value,
            has_header: headerInput.checked,
            delimiter: delimInput.value || ",",
            encoding: encodingSelect.value,
            create_table: createInput.checked,
            truncate: truncateInput.checked,
            sheet: null,
          };
          try {
            const n = await transferTasks.import({ ...opts, task_id: taskId, column_mapping: formatSelect.value === "csv" ? mappingInputs.filter(m => m.target.value).map(m => ({ source: m.source, target: m.target.value })) : [] });
            feedback.finish();
            status.textContent = `导入完成，共 ${n} 行。`;
            onDone(n);
          } catch (e) {
            feedback.finish();
            status.textContent = `导入失败：${String(e)}`;
            dlg.showError(String(e));
          } finally { running = false; }
          return false;
        },
      },
      { label: "关闭" },
    ],
  });

  accessibleDialog(dlg, "导入向导");
  feedback = taskFeedback(dlg, status);
  return dlg;
}

/** 导出向导。 */
export function openExportWizard(
  connection: ConnectionConfig,
  schema: string,
  table: string,
  onDone: (count: number) => void,
): Dialog {
  const steps = ["源", "目标", "格式", "执行"];
  const bar = stepBar(steps);
  let step = 0;
  let running = false;
  let feedback: ReturnType<typeof taskFeedback>;

  const fileInput = textInput("", 320);
  const formatSelect = select(
    [
      { value: "csv", label: "CSV" },
      { value: "json", label: "JSON" },
      { value: "sql", label: "SQL INSERT" },
    ],
    "csv",
    120,
  );
  const delimInput = textInput(",", 60);
  const encodingSelect = select(ENCODINGS, "utf-8", 120);
  const headerInput = document.createElement("input");
  headerInput.type = "checkbox";
  headerInput.checked = true;
  const whereInput = textInput("", 320);
  const limitInput = textInput("", 80);
  const status = document.createElement("div");
  status.style.marginTop = "6px";
  status.style.minHeight = "18px";
  status.style.color = "var(--fg-muted)";

  const panels: HTMLElement[] = [];

  const p1 = document.createElement("div");
  p1.appendChild(field("连接:", textInput(connection.name, 240)));
  p1.appendChild(field("模式:", textInput(schema, 240)));
  p1.appendChild(field("表:", textInput(table, 240)));
  panels.push(p1);

  const p2 = document.createElement("div");
  const browseBtn = document.createElement("button");
  browseBtn.className = "btn";
  browseBtn.style.minWidth = "0";
  browseBtn.textContent = "浏览...";
  browseBtn.addEventListener("click", async () => {
    const f = await pickFile("保存导出文件", [
      { name: "CSV 文件", extensions: ["csv"] },
      { name: "JSON 文件", extensions: ["json"] },
      { name: "SQL 文件", extensions: ["sql"] },
    ]);
    if (f) fileInput.value = f;
  });
  const fileWrap = document.createElement("div");
  fileWrap.style.display = "flex";
  fileWrap.style.gap = "6px";
  fileWrap.appendChild(fileInput);
  fileWrap.appendChild(browseBtn);
  p2.appendChild(field("文件名:", fileWrap));
  panels.push(p2);

  const p3 = document.createElement("div");
  p3.appendChild(field("格式:", formatSelect));
  p3.appendChild(field("分隔符:", delimInput));
  p3.appendChild(field("编码:", encodingSelect));
  const headerLabel = document.createElement("label");
  headerLabel.className = "checkbox";
  headerLabel.appendChild(headerInput);
  const headerText = document.createElement("span");
  headerText.textContent = "包含列名行";
  headerLabel.appendChild(headerText);
  p3.appendChild(headerLabel);
  panels.push(p3);

  const p4 = document.createElement("div");
  p4.appendChild(field("筛选条件 (WHERE):", whereInput));
  p4.appendChild(field("限制行数:", limitInput));
  p4.appendChild(status);
  panels.push(p4);

  function showStep(i: number): void {
    step = i;
    bar.go(i);
    panels.forEach((p, idx) => {
      p.style.display = idx === i ? "" : "none";
    });
  }

  const dlg = new Dialog({
    title: "导出向导",
    width: 620,
    render: (body) => {
      body.appendChild(bar.el);
      const host = document.createElement("div");
      host.style.paddingTop = "10px";
      for (const p of panels) host.appendChild(p);
      body.appendChild(host);
      showStep(0);
    },
    buttons: [
      {
        label: "上一步",
        onClick: () => {
          if (step > 0) showStep(step - 1);
          return false;
        },
      },
      {
        label: "下一步",
        onClick: () => {
          if (step < 3) {
            showStep(step + 1);
            return false;
          }
          return false;
        },
      },
      {
        label: "开始导出",
        primary: true,
        onClick: async () => {
          if (running) return false;
          const file = fileInput.value.trim();
          if (!file) {
            dlg.showError("请指定导出文件");
            return false;
          }
          const limitValue = limitInput.value.trim();
          if (limitValue && (!/^\d+$/.test(limitValue) || !Number.isSafeInteger(Number(limitValue)))) { dlg.showError("限制行数必须是非负整数"); return false; }
          if (/[;\0]|--|\/\*/.test(whereInput.value)) { dlg.showError("WHERE不能包含分号或注释"); return false; }
          showStep(3); running = true;
          const taskId = feedback.begin();
          status.textContent = "正在导出...";
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
            status.textContent = `导出完成，共 ${n} 行。`;
            onDone(n);
          } catch (e) {
            feedback.finish();
            status.textContent = `导出失败：${String(e)}`;
            dlg.showError(String(e));
          } finally { running = false; }
          return false;
        },
      },
      { label: "关闭" },
    ],
  });

  accessibleDialog(dlg, "导出向导");
  feedback = taskFeedback(dlg, status);
  return dlg;
}

/** 查询结果导出向导（把当前结果集写出为 CSV / JSON / SQL）。 */
export function openResultExportDialog(
  connection: ConnectionConfig,
  onRun: (opts: ExportOptions) => Promise<number>,
): Dialog {
  let running = false;
  const fileInput = textInput("", 320);
  const formatSelect = select(
    [
      { value: "csv", label: "CSV" },
      { value: "json", label: "JSON" },
      { value: "sql", label: "SQL INSERT" },
    ],
    "csv",
    120,
  );
  const delimInput = textInput(",", 60);
  const encodingSelect = select(ENCODINGS, "utf-8", 120);
  const headerInput = document.createElement("input");
  headerInput.type = "checkbox";
  headerInput.checked = true;
  const status = document.createElement("div");
  status.style.marginTop = "6px";
  status.style.minHeight = "18px";
  status.style.color = "var(--fg-muted)";

  const dlg = new Dialog({
    title: "导出结果",
    width: 560,
    render: (body) => {
      const browseBtn = document.createElement("button");
      browseBtn.className = "btn";
      browseBtn.style.minWidth = "0";
      browseBtn.textContent = "浏览...";
      browseBtn.addEventListener("click", async () => {
        const f = await pickFile("保存导出文件", [
          { name: "CSV 文件", extensions: ["csv"] },
          { name: "JSON 文件", extensions: ["json"] },
          { name: "SQL 文件", extensions: ["sql"] },
        ]);
        if (f) fileInput.value = f;
      });
      const fileWrap = document.createElement("div");
      fileWrap.style.display = "flex";
      fileWrap.style.gap = "6px";
      fileWrap.appendChild(fileInput);
      fileWrap.appendChild(browseBtn);
      body.appendChild(field("文件名:", fileWrap));
      body.appendChild(field("格式:", formatSelect));
      body.appendChild(field("分隔符:", delimInput));
      body.appendChild(field("编码:", encodingSelect));
      const headerLabel = document.createElement("label");
      headerLabel.className = "checkbox";
      headerLabel.appendChild(headerInput);
      const headerText = document.createElement("span");
      headerText.textContent = "包含列名行";
      headerLabel.appendChild(headerText);
      body.appendChild(headerLabel);
      body.appendChild(status);
    },
    buttons: [
      {
        label: "开始导出",
        primary: true,
        onClick: async () => {
          if (running) return false;
          const file = fileInput.value.trim();
          if (!file) {
            dlg.showError("请指定导出文件");
            return false;
          }
          running = true;
          status.textContent = "正在导出...";
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
          } finally { running = false; }
          return false;
        },
      },
      { label: "关闭" },
    ],
  });

  accessibleDialog(dlg, "导出结果");
  status.setAttribute("role", "status");
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

/** SQL 转储对话框。 */
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
  const fileInput = textInput("", 320);
  const structureOnly = document.createElement("input");
  structureOnly.type = "checkbox";
  const dataOnly = document.createElement("input");
  dataOnly.type = "checkbox";
  const includeDrop = document.createElement("input");
  includeDrop.type = "checkbox";
  includeDrop.checked = true;
  structureOnly.addEventListener("change", () => { if (structureOnly.checked) dataOnly.checked = false; includeDrop.disabled = false; });
  dataOnly.addEventListener("change", () => { if (dataOnly.checked) { structureOnly.checked = false; includeDrop.checked = false; } includeDrop.disabled = dataOnly.checked; });
  const status = document.createElement("div");
  status.style.marginTop = "8px";
  status.style.minHeight = "18px";
  status.style.color = "var(--fg-muted)";

  const browse = document.createElement("button");
  browse.className = "btn";
  browse.style.minWidth = "0";
  browse.textContent = "浏览...";
  browse.addEventListener("click", async () => {
    const f = await pickFile("保存 SQL 文件", [
      { name: "SQL 文件", extensions: ["sql"] },
    ]);
    if (f) fileInput.value = f;
  });

  const dlg = new Dialog({
    title: "SQL 转储",
    width: 520,
    render: (body) => {
      body.appendChild(field("连接:", textInput(connection.name, 220)));
      body.appendChild(field("模式:", textInput(schema, 220)));
      body.appendChild(field("表:", textInput(table, 220)));
      const wrap = document.createElement("div");
      wrap.style.display = "flex";
      wrap.style.gap = "6px";
      wrap.appendChild(fileInput);
      wrap.appendChild(browse);
      body.appendChild(field("输出文件:", wrap));

      const l1 = document.createElement("label");
      l1.className = "checkbox";
      l1.appendChild(structureOnly);
      const t1 = document.createElement("span");
      t1.textContent = "仅结构";
      l1.appendChild(t1);
      body.appendChild(l1);

      const l2 = document.createElement("label");
      l2.className = "checkbox";
      l2.appendChild(dataOnly);
      const t2 = document.createElement("span");
      t2.textContent = "仅数据";
      l2.appendChild(t2);
      body.appendChild(l2);

      const l3 = document.createElement("label");
      l3.className = "checkbox";
      l3.appendChild(includeDrop);
      const t3 = document.createElement("span");
      t3.textContent = "包含 DROP TABLE 语句";
      l3.appendChild(t3);
      body.appendChild(l3);
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
          running = true;
          status.textContent = "正在生成转储...";
          try {
            const n = await onRun({
              connection_id: connection.id,
              schema,
              table,
              file,
              structure_only: structureOnly.checked,
              data_only: dataOnly.checked,
              include_drop: includeDrop.checked,
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

  accessibleDialog(dlg, "SQL 转储");
  status.setAttribute("role", "status");
  return dlg;
}
