/** 自动任务：定时任务（备份 / 导出 / 导入 / 数据同步 / 数据生成 / 数据字典）编辑对话框 + 运行历史对话框。 */

import { api } from "../core/api";
import { formatError } from "../core/execution";
import { bindExportPath, type ExportPathBinding } from "../core/export-path";
import { createDatabasePicker } from "../core/database-picker";
import { createTableMultiPicker, createTablePicker } from "../core/table-picker";
import type { ConnectionConfig, JobHistoryEntry, ScheduleJob, ScheduleTaskKind, TransferOptions } from "../core/types";
import { validateCron5 } from "../core/scheduler";
import { DataGrid } from "../ui/DataGrid";
import { checkbox, Dialog, field, groupbox, numberInput, radioGroup, select, textInput } from "../ui/Dialog";

/** Tauri 原生文件对话框过滤器。 */
interface FileFilter {
  name: string;
  extensions: string[];
}

/** 单个动作类型配置区对外接口（显示切换 / 校验 / 组装 options）。 */
export interface TaskBox {
  el: HTMLElement;
  /** 当前选中的连接 id（写回 job.connection_id）。 */
  connId(): string;
  /** 数据库 / 模式输入值（写回 job.database）。 */
  db(): string;
  /** 校验必填项，返回错误文案或 null。 */
  validate(): string | null;
  /** 组装写入 job.task.options 的配置。 */
  options(): Record<string, unknown>;
}

/** Cron 可视化构建器对外接口。 */
interface CronBuilder {
  el: HTMLElement;
  /** 由当前控件生成五字段表达式；配置不完整时返回空串。 */
  build(): string;
  /** 校验构建配置与表达式，返回错误文案或 null。 */
  problem(): string | null;
  /** 从已有五字段表达式回填控件；匹配不上落「高级自定义」。 */
  restore(expr: string): void;
}

/** 动作类型单选。 */
const TASK_KINDS: { value: ScheduleTaskKind; label: string }[] = [
  { value: "backup", label: "备份" },
  { value: "export", label: "导出" },
  { value: "import", label: "导入" },
  { value: "data_sync", label: "数据同步" },
  { value: "data_generation", label: "数据生成" },
  { value: "data_dictionary", label: "数据字典" },
];

/** 定时模式单选。 */
const SCHEDULE_MODES = [
  { value: "manual", label: "仅手动" },
  { value: "interval", label: "固定间隔" },
  { value: "daily", label: "每天" },
  { value: "cron", label: "Cron 定时" },
];

/** 固定间隔下拉（单位秒）。 */
const INTERVALS: { value: number; label: string }[] = [
  { value: 60, label: "每 1 分钟" },
  { value: 300, label: "每 5 分钟" },
  { value: 1800, label: "每 30 分钟" },
  { value: 3600, label: "每 1 小时" },
  { value: 21600, label: "每 6 小时" },
  { value: 86400, label: "每 24 小时" },
];

/** 导出 / 导入通用格式下拉。 */
const FORMATS = [
  { value: "csv", label: "CSV" },
  { value: "json", label: "JSON" },
  { value: "sql", label: "SQL" },
];

const ENCODINGS = [
  { value: "utf-8", label: "UTF-8" },
  { value: "gbk", label: "GBK" },
  { value: "big5", label: "Big5" },
  { value: "latin1", label: "ISO-8859-1" },
];

/** 导出保存对话框的过滤器（随格式选择）。 */
const EXPORT_FILE_FILTERS: Record<string, FileFilter> = {
  csv: { name: "CSV 文件", extensions: ["csv"] },
  json: { name: "JSON 文件", extensions: ["json"] },
  sql: { name: "SQL 文件", extensions: ["sql"] },
};

/** 导入源文件过滤器。 */
const IMPORT_FILE_FILTERS: FileFilter[] = [
  { name: "CSV 文件", extensions: ["csv"] },
  { name: "JSON 文件", extensions: ["json"] },
  { name: "SQL 文件", extensions: ["sql"] },
  { name: "所有文件", extensions: ["*"] },
];

/** 数据字典输出格式下拉（value 与后端 DictExportOptions.format 一致）。 */
const DICT_FORMATS = [
  { value: "markdown", label: "Markdown" },
  { value: "word", label: "Word" },
  { value: "excel", label: "Excel" },
  { value: "pdf", label: "PDF" },
  { value: "html", label: "HTML" },
];

/** 数据字典格式 → 扩展名（格式名与扩展名不一致，需单独映射）。 */
const DICT_EXTENSIONS: Record<string, string> = {
  markdown: "md",
  word: "docx",
  excel: "xlsx",
  pdf: "pdf",
  html: "html",
};

/** 数据字典保存对话框过滤器（随格式选择）。 */
const DICT_FILE_FILTERS: Record<string, FileFilter> = {
  markdown: { name: "Markdown 文件", extensions: ["md"] },
  word: { name: "Word 文档", extensions: ["docx"] },
  excel: { name: "Excel 工作簿", extensions: ["xlsx"] },
  pdf: { name: "PDF 文档", extensions: ["pdf"] },
  html: { name: "HTML 文件", extensions: ["html"] },
};

/** Cron 星期字段 1–7 = 周一..周日。 */
const WEEKDAYS: { value: number; label: string }[] = [
  { value: 1, label: "周一" },
  { value: 2, label: "周二" },
  { value: 3, label: "周三" },
  { value: 4, label: "周四" },
  { value: 5, label: "周五" },
  { value: 6, label: "周六" },
  { value: 7, label: "周日" },
];

/** Cron 构建器模式（前五种由控件生成表达式，custom 直接手填）。 */
const CRON_MODES = [
  { value: "every_n", label: "每 N 分钟" },
  { value: "hourly", label: "每小时第 M 分" },
  { value: "daily", label: "每天 HH:MM" },
  { value: "weekly", label: "每周星期 X 的 HH:MM" },
  { value: "monthly", label: "每月第 D 天 HH:MM" },
  { value: "custom", label: "高级自定义" },
];

/** 读取 checkbox() 控件的勾选状态。 */
function isChecked(box: HTMLLabelElement): boolean {
  return box.querySelector<HTMLInputElement>("input")!.checked;
}

/** A missing persisted connection must not silently become another endpoint. */
function pickConn(connections: ConnectionConfig[], preferred: string): string {
  return preferred ? connections.some(c => c.id === preferred) ? preferred : "" : connections[0]?.id ?? "";
}

/** 连接下拉框（各任务配置区共用）。 */
function connSelect(connections: ConnectionConfig[], value: string): HTMLSelectElement {
  return select(
    [{ value: "", label: "请选择连接…" }, ...connections.map(c => ({ value: c.id, label: `${c.name}（${c.kind}）` }))],
    pickConn(connections, value),
  );
}

/** 从任务 options 读取字符串字段（缺失或类型不符时给默认值）。 */
function optString(options: Record<string, unknown>, key: string, fallback = ""): string {
  const v = options[key];
  return typeof v === "string" ? v : fallback;
}

/** 从任务 options 读取布尔字段。 */
function optBool(options: Record<string, unknown>, key: string, fallback: boolean): boolean {
  const v = options[key];
  return typeof v === "boolean" ? v : fallback;
}

/** 从任务 options 读取数字字段；缺失 / null 时返回 null。 */
function optNumber(options: Record<string, unknown>, key: string): number | null {
  const v = options[key];
  return typeof v === "number" && Number.isFinite(v) ? v : null;
}

/** 从任务 options 读取字符串数组字段。 */
function optStringList(options: Record<string, unknown>, key: string): string[] {
  const v = options[key];
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
}

/** 数字输入框：value 传 null 时留空（表示不限 / 默认）。 */
function optionalNumber(value: number | null, placeholder: string): HTMLInputElement {
  const el = document.createElement("input");
  el.type = "number";
  el.step = "1";
  if (value !== null) el.value = String(value);
  else el.placeholder = placeholder;
  return el;
}

/** 灰色说明文案。 */
function mutedNote(text: string): HTMLDivElement {
  const el = document.createElement("div");
  el.style.cssText = "font-size: var(--fs-11); color: var(--text-tertiary); margin: 2px 0 var(--sp-3);";
  el.textContent = text;
  return el;
}

/** 复选框行（横向排列，form-check-group 自带下边距）。 */
function checkRow(...boxes: HTMLLabelElement[]): HTMLElement {
  const row = document.createElement("div");
  row.className = "form-check-group";
  row.style.flexDirection = "row";
  row.style.gap = "18px";
  row.append(...boxes);
  return row;
}

/** 打开 Tauri 原生文件对话框；useSave 为 true 时是保存路径对话框。 */
async function pickFile(useSave: boolean, title: string, filters: FileFilter[], defaultPath?: string): Promise<string | null> {
  const { open, save } = await import("@tauri-apps/plugin-dialog");
  const res = useSave ? await save({ title, filters, defaultPath }) : await open({ title, multiple: false, filters });
  if (res === null || typeof res === "string") return res;
  throw new Error("文件对话框返回了无法识别的路径；原输入已保留");
}

let fileErrorSequence = 0;

/** 「文本框 + 浏览...按钮」一行（form-control-line 布局）；filters 惰性求值，可随其它控件变化。 */
function fileLine(input: HTMLInputElement, useSave: boolean, title: string, filters: () => FileFilter[], destination?: ExportPathBinding): HTMLElement {
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "btn";
  btn.style.minWidth = "0";
  btn.textContent = "浏览...";
  const errorBox = document.createElement("div"); errorBox.id = `schedule-file-error-${++fileErrorSequence}`;
  errorBox.setAttribute("role", "alert"); errorBox.hidden = true;
  input.setAttribute("aria-describedby", [input.getAttribute("aria-describedby"), errorBox.id].filter(Boolean).join(" "));
  btn.addEventListener("click", async () => {
    try {
      await destination?.ready;
      const f = await pickFile(useSave, title, filters(), useSave ? input.value.trim() : undefined);
      if (f) { input.value = f; destination?.refresh(); errorBox.hidden = true; }
    } catch (error) {
      console.error("选择定时任务文件失败", error); errorBox.textContent = formatError(error); errorBox.hidden = false;
    }
  });
  const wrap = document.createElement("div");
  wrap.className = "form-control-line";
  wrap.append(input, btn, errorBox);
  return wrap;
}

/** Cron 可视化构建器：各模式控件生成表达式、实时校验、从已有表达式反解析。 */
function buildCronBuilder(): CronBuilder {
  const modeSelect = select(CRON_MODES, "every_n", 220);
  modeSelect.setAttribute("aria-label", "Cron 构建模式");
  const mkNum = (value: number, min: number, max: number): HTMLInputElement => {
    const el = numberInput(value);
    el.min = String(min);
    el.max = String(max);
    return el;
  };
  const everyNInput = mkNum(30, 1, 59);
  const hourlyMinInput = mkNum(0, 0, 59);
  const dailyHourInput = mkNum(9, 0, 23);
  const dailyMinInput = mkNum(0, 0, 59);
  const weeklyHourInput = mkNum(9, 0, 23);
  const weeklyMinInput = mkNum(0, 0, 59);
  const monthlyDayInput = mkNum(1, 1, 28);
  const monthlyHourInput = mkNum(9, 0, 23);
  const monthlyMinInput = mkNum(0, 0, 59);
  const customCronInput = textInput("");
  customCronInput.placeholder = "*/30 * * * *";
  customCronInput.style.fontFamily = "var(--font-mono)";
  const weekdayBoxes = WEEKDAYS.map((d) => ({ value: d.value, box: checkbox(d.label) }));
  const weekdayRow = checkRow(...weekdayBoxes.map((d) => d.box));

  // 各模式的字段网格包在普通 div 里，便于整体 hidden 切换（form-grid 的 display:grid 会覆盖 hidden）
  const grid = (three: boolean, ...fields: HTMLElement[]): HTMLElement => {
    const el = document.createElement("div");
    el.className = three ? "form-grid form-grid--three" : "form-grid";
    el.append(...fields);
    return el;
  };
  const panel = (value: string, ...children: HTMLElement[]): { value: string; el: HTMLElement } => {
    const host = document.createElement("div");
    host.append(...children);
    return { value, el: host };
  };
  const panels: { value: string; el: HTMLElement }[] = [
    panel("every_n", field("间隔（分钟）:", everyNInput)),
    panel("hourly", field("分钟:", hourlyMinInput)),
    panel("daily", grid(false, field("时:", dailyHourInput), field("分:", dailyMinInput))),
    panel("weekly", weekdayRow, grid(false, field("时:", weeklyHourInput), field("分:", weeklyMinInput))),
    panel("monthly", grid(true, field("日:", monthlyDayInput), field("时:", monthlyHourInput), field("分:", monthlyMinInput))),
    panel("custom", field("Cron 表达式:", customCronInput)),
  ];

  const exprInput = textInput("");
  exprInput.readOnly = true;
  exprInput.style.fontFamily = "var(--font-mono)";
  const err = document.createElement("div");
  err.style.color = "var(--danger-600)";
  err.style.fontSize = "var(--fs-12)";
  err.hidden = true;

  /** 读取整数输入并校验范围；空或非法返回 null。 */
  function intInRange(el: HTMLInputElement, min: number, max: number): number | null {
    if (!/^\d+$/.test(el.value.trim())) return null;
    const n = Number(el.value);
    return n >= min && n <= max ? n : null;
  }

  /** 由当前控件生成五字段 cron 表达式；配置不完整时返回空串。 */
  function build(): string {
    switch (modeSelect.value) {
      case "every_n": {
        const n = intInRange(everyNInput, 1, 59);
        return n !== null ? `*/${n} * * * *` : "";
      }
      case "hourly": {
        const m = intInRange(hourlyMinInput, 0, 59);
        return m !== null ? `${m} * * * *` : "";
      }
      case "daily": {
        const h = intInRange(dailyHourInput, 0, 23);
        const m = intInRange(dailyMinInput, 0, 59);
        return h !== null && m !== null ? `${m} ${h} * * *` : "";
      }
      case "weekly": {
        const h = intInRange(weeklyHourInput, 0, 23);
        const m = intInRange(weeklyMinInput, 0, 59);
        const days = weekdayBoxes.filter((d) => isChecked(d.box)).map((d) => d.value).sort((a, b) => a - b);
        return h !== null && m !== null && days.length > 0 ? `${m} ${h} * * ${days.join(",")}` : "";
      }
      case "monthly": {
        const day = intInRange(monthlyDayInput, 1, 28);
        const h = intInRange(monthlyHourInput, 0, 23);
        const m = intInRange(monthlyMinInput, 0, 59);
        return day !== null && h !== null && m !== null ? `${m} ${h} ${day} * *` : "";
      }
      case "custom":
        return customCronInput.value.trim();
      default:
        return "";
    }
  }

  /** 校验构建配置与表达式，返回错误文案或 null。 */
  function problem(): string | null {
    if (modeSelect.value === "weekly" && !weekdayBoxes.some((d) => isChecked(d.box))) {
      return "每周模式请至少选择一个星期";
    }
    const expr = build();
    if (!expr) return "Cron 构建配置不完整";
    try {
      validateCron5(expr);
    } catch (e) {
      return e instanceof Error ? e.message : String(e);
    }
    return null;
  }

  /** 刷新只读表达式与红色错误提示。 */
  function refresh(): void {
    exprInput.value = build();
    const p = problem();
    err.hidden = p === null;
    err.textContent = p ?? "";
  }

  /** 按当前构建模式切换各控件面板显示。 */
  function refreshPanels(): void {
    for (const p of panels) p.el.hidden = p.value !== modeSelect.value;
  }

  /** 从已有五字段表达式反解析回可视化模式；启发式匹配不上落「高级自定义」。 */
  function restore(expr: string): void {
    const f = expr.trim().split(/\s+/);
    const valid = (s: string, min: number, max: number): boolean =>
      /^\d{1,3}$/.test(s) && Number(s) >= min && Number(s) <= max;
    const every = f.length === 5 ? /^\*\/(\d{1,2})$/.exec(f[0]) : null;
    if (every !== null && valid(every[1], 1, 59) && f[1] === "*" && f[2] === "*" && f[3] === "*" && f[4] === "*") {
      modeSelect.value = "every_n";
      everyNInput.value = every[1];
    } else if (f.length === 5 && f[1] === "*" && f[2] === "*" && f[3] === "*" && f[4] === "*" && valid(f[0], 0, 59)) {
      modeSelect.value = "hourly";
      hourlyMinInput.value = f[0];
    } else if (f.length === 5 && f[2] === "*" && f[3] === "*" && f[4] === "*" && valid(f[0], 0, 59) && valid(f[1], 0, 23)) {
      modeSelect.value = "daily";
      dailyMinInput.value = f[0];
      dailyHourInput.value = f[1];
    } else if (f.length === 5 && f[2] === "*" && f[3] === "*" && valid(f[0], 0, 59) && valid(f[1], 0, 23)
      && f[4].split(",").every((d) => valid(d, 1, 7))) {
      modeSelect.value = "weekly";
      weeklyMinInput.value = f[0];
      weeklyHourInput.value = f[1];
      const days = f[4].split(",").map(Number);
      for (const d of weekdayBoxes) d.box.querySelector<HTMLInputElement>("input")!.checked = days.includes(d.value);
    } else if (f.length === 5 && f[3] === "*" && f[4] === "*" && valid(f[0], 0, 59) && valid(f[1], 0, 23) && valid(f[2], 1, 28)) {
      modeSelect.value = "monthly";
      monthlyMinInput.value = f[0];
      monthlyHourInput.value = f[1];
      monthlyDayInput.value = f[2];
    } else {
      modeSelect.value = "custom";
      customCronInput.value = expr;
    }
    refreshPanels();
    refresh();
  }

  modeSelect.addEventListener("change", () => {
    refreshPanels();
    refresh();
  });
  for (const input of [everyNInput, hourlyMinInput, dailyHourInput, dailyMinInput, weeklyHourInput, weeklyMinInput, monthlyDayInput, monthlyHourInput, monthlyMinInput, customCronInput]) {
    input.addEventListener("input", refresh);
    input.addEventListener("change", refresh);
  }
  for (const d of weekdayBoxes) d.box.addEventListener("change", refresh);

  const el = document.createElement("div");
  el.append(
    field("构建模式:", modeSelect),
    ...panels.map((p) => p.el),
    field("表达式（分 时 日 月 星期）:", exprInput),
    err,
  );
  refreshPanels();
  refresh();
  return { el, build, problem, restore };
}

/** 备份任务配置区。 */
function buildBackupBox(connections: ConnectionConfig[], opts: Record<string, unknown>, initConn: string, initDb: string, safeScheduler = false, withTablePicker = false, requireTables = false): TaskBox {
  const conn = connSelect(connections, initConn);
  const dbPicker = createDatabasePicker(connections, conn, initDb), db = dbPicker.select;
  const structure = checkbox("包含结构", optBool(opts, "include_structure", true));
  const data = checkbox("包含数据", optBool(opts, "include_data", true));
  const comment = textInput(optString(opts, "comment"));
  comment.placeholder = "备份说明（可选）";
  const tablePicker = withTablePicker ? createTableMultiPicker(connections, conn, db, Array.isArray(opts.tables) ? opts.tables.filter((x): x is string => typeof x === "string") : [], "备份表:", requireTables) : null;
  const box = groupbox("备份配置");
  box.body.append(
    field("连接:", conn),
    dbPicker.el,
    checkRow(structure, data),
    field("注释:", comment),
    tablePicker ? tablePicker.el : mutedNote(safeScheduler ? "持久备份须明确选择表集合；不自动扩展到视图或例程。" : "不选择对象即备份全部表与视图。"),
  );
  return {
    el: box.el,
    connId: () => conn.value,
    db: () => db.value.trim(),
    validate: () => (conn.value ? (dbPicker.problem() || (tablePicker ? tablePicker.problem() : null)) : "请选择备份连接"),
    options: () => ({
      connection_id: conn.value,
      schema: db.value.trim(),
      include_structure: isChecked(structure),
      include_data: isChecked(data),
      comment: comment.value,
      tables: tablePicker ? tablePicker.values() : [],
    }),
  };
}

/** 导出任务配置区。 */
function buildExportBox(connections: ConnectionConfig[], opts: Record<string, unknown>, initConn: string, initDb: string): TaskBox {
  const conn = connSelect(connections, initConn);
  const dbPicker = createDatabasePicker(connections, conn, initDb), db = dbPicker.select;
  const tablePicker = createTablePicker(connections, conn, db, optString(opts, "table"));
  const table = tablePicker.select;
  const format = select(FORMATS, optString(opts, "format", "csv"), 140);
  const delim = textInput(optString(opts, "delimiter", ","));
  const encoding = select(ENCODINGS, optString(opts, "encoding", "utf-8"), 140);
  const file = textInput(optString(opts, "file"));
  file.setAttribute("aria-label", "计划导出完整保存路径");
  const destination = bindExportPath(file, () => `${db.value || initDb || "database"}_${tablePicker.value() || "export"}.${format.value}`, format);
  db.addEventListener("change", destination.autoName);
  table.addEventListener("change", destination.autoName);
  const fileRow = fileLine(file, true, "保存导出文件", () => [EXPORT_FILE_FILTERS[format.value] ?? EXPORT_FILE_FILTERS.csv], destination);
  const header = checkbox("包含列名", optBool(opts, "has_header", true));
  const where = textInput(optString(opts, "where_clause"));
  where.placeholder = "例如 created_at > '2024-01-01'（可空）";
  const limit = optionalNumber(optNumber(opts, "limit"), "全部行");
  const fmtGrid = document.createElement("div");
  fmtGrid.className = "form-grid form-grid--three";
  fmtGrid.append(field("格式:", format), field("分隔符:", delim), field("编码:", encoding));
  const box = groupbox("导出配置");
  box.body.append(
    field("连接:", conn),
    dbPicker.el,
    tablePicker.el,
    fmtGrid,
    field("输出文件:", fileRow),
    checkRow(header),
    field("筛选条件 (WHERE):", where),
    field("限制行数:", limit),
  );
  return {
    el: box.el,
    connId: () => conn.value,
    db: () => db.value.trim(),
    validate: () => {
      if (!conn.value) return "请选择导出连接";
      if (dbPicker.problem()) return dbPicker.problem();
      if (tablePicker.problem()) return tablePicker.problem();
      if (destination.problem()) return destination.problem();
      const limitRaw = limit.value.trim();
      if (limitRaw && !/^\d+$/.test(limitRaw)) return "限制行数必须是非负整数";
      return null;
    },
    options: () => ({
      connection_id: conn.value,
      schema: db.value.trim(),
      table: tablePicker.value(),
      file: file.value.trim(),
      format: format.value,
      has_header: isChecked(header),
      delimiter: delim.value || ",",
      encoding: encoding.value,
      where_clause: where.value.trim() || null,
      limit: limit.value.trim() ? Number(limit.value) : null,
    }),
  };
}

/** 导入任务配置区。 */
function buildImportBox(connections: ConnectionConfig[], opts: Record<string, unknown>, initConn: string, initDb: string): TaskBox {
  const conn = connSelect(connections, initConn);
  const dbPicker = createDatabasePicker(connections, conn, initDb), db = dbPicker.select;
  const tablePicker = createTablePicker(connections, conn, db, optString(opts, "table"));
  const file = textInput(optString(opts, "file"));
  const fileRow = fileLine(file, false, "选择导入文件", () => IMPORT_FILE_FILTERS);
  const format = select(FORMATS, optString(opts, "format", "csv"), 140);
  const delim = textInput(optString(opts, "delimiter", ","));
  const encoding = select(ENCODINGS, optString(opts, "encoding", "utf-8"), 140);
  const header = checkbox("第一行包含列名", optBool(opts, "has_header", true));
  const createTable = checkbox("目标表不存在时创建", optBool(opts, "create_table", false));
  const truncate = checkbox("导入前清空目标表", optBool(opts, "truncate", false));
  const newTable = textInput(optString(opts, "table"));
  newTable.placeholder = "新表名";
  const newTableField = field("新表名:", newTable);
  const targetTable = (): string => isChecked(createTable) ? newTable.value.trim() : tablePicker.value();
  const syncTableMode = (): void => {
    const creating = isChecked(createTable);
    tablePicker.el.style.display = creating ? "none" : "";
    newTableField.style.display = creating ? "" : "none";
    tablePicker.select.disabled = creating || tablePicker.select.dataset.metadataDisabled === "true";
  };
  createTable.querySelector<HTMLInputElement>("input")?.addEventListener("change", syncTableMode);
  void tablePicker.ready.then(syncTableMode);
  syncTableMode();
  const conflict = select(
    [
      { value: "", label: "报错（默认）" },
      { value: "skip", label: "跳过冲突记录" },
      { value: "update", label: "按主键更新冲突记录" },
    ],
    optString(opts, "on_conflict", ""),
    200,
  );
  const conflictCols = textInput(optStringList(opts, "conflict_columns").join(","));
  conflictCols.placeholder = "逗号分隔，留空使用主键";
  const fmtGrid = document.createElement("div");
  fmtGrid.className = "form-grid form-grid--three";
  fmtGrid.append(field("格式:", format), field("分隔符:", delim), field("编码:", encoding));
  const box = groupbox("导入配置");
  box.body.append(
    field("连接:", conn),
    dbPicker.el,
    tablePicker.el,
    newTableField,
    field("源文件:", fileRow),
    fmtGrid,
    checkRow(header),
    checkRow(createTable, truncate),
    field("记录冲突:", conflict),
    field("冲突键列:", conflictCols),
  );
  return {
    el: box.el,
    connId: () => conn.value,
    db: () => db.value.trim(),
    validate: () => {
      if (!conn.value) return "请选择导入连接";
      if (dbPicker.problem()) return dbPicker.problem();
      if (isChecked(createTable)) { if (!newTable.value.trim()) return "请填写目标表"; }
      else if (tablePicker.problem()) return tablePicker.problem();
      if (!file.value.trim()) return "请选择要导入的文件";
      return null;
    },
    options: () => ({
      connection_id: conn.value,
      schema: db.value.trim(),
      table: targetTable(),
      file: file.value.trim(),
      format: format.value,
      has_header: isChecked(header),
      delimiter: delim.value || ",",
      encoding: encoding.value,
      create_table: isChecked(createTable),
      truncate: isChecked(truncate),
      on_conflict: conflict.value,
      conflict_columns: conflictCols.value.split(",").map((s) => s.trim()).filter(Boolean),
      sheet: null,
    }),
  };
}

/** 数据同步任务配置区（job.connection_id 存源连接）。 */
function buildSyncBox(connections: ConnectionConfig[], opts: Record<string, unknown>, initSrcConn: string, initSrcDb: string, safeScheduler = false): TaskBox {
  const srcConn = connSelect(connections, initSrcConn);
  const srcPicker = createDatabasePicker(connections, srcConn, initSrcDb, "源"), srcDb = srcPicker.select;
  const srcTablePicker = createTablePicker(connections, srcConn, srcDb, optString(opts, "source_table"), "源");
  const srcTable = srcTablePicker.select;
  const dstConn = connSelect(connections, pickConn(connections, optString(opts, "target_connection")));
  const dstPicker = createDatabasePicker(connections, dstConn, optString(opts, "target_schema"), "目标"), dstDb = dstPicker.select;
  const dstTablePicker = createTablePicker(connections, dstConn, dstDb, optString(opts, "target_table"), "目标");
  const createTarget = checkbox("目标表不存在时新建", !safeScheduler && optBool(opts, "create_target", true));
  const newTargetName = textInput(optString(opts, "target_table"));
  newTargetName.placeholder = "新目标表名";
  const newTargetField = field("新目标表名:", newTargetName);
  const targetTableName = (): string => (!safeScheduler && isChecked(createTarget)) ? newTargetName.value.trim() : dstTablePicker.value();
  const syncTargetMode = (): void => {
    const creating = !safeScheduler && isChecked(createTarget);
    dstTablePicker.el.style.display = creating ? "none" : "";
    newTargetField.style.display = creating ? "" : "none";
    dstTablePicker.select.disabled = creating || dstTablePicker.select.dataset.metadataDisabled === "true";
  };
  createTarget.querySelector<HTMLInputElement>("input")?.addEventListener("change", syncTargetMode);
  void dstTablePicker.ready.then(syncTargetMode);
  syncTargetMode();
  const structure = checkbox("复制结构", optBool(opts, "copy_structure", !safeScheduler));
  const data = checkbox("复制数据", optBool(opts, "copy_data", true));
  const box = groupbox("数据同步配置");
  box.body.append(
    field("源连接:", srcConn),
    srcPicker.el,
    srcTablePicker.el,
    field("目标连接:", dstConn),
    dstPicker.el,
    dstTablePicker.el,
    ...(safeScheduler ? [] : [checkRow(createTarget)]),
    newTargetField,
    checkRow(structure, data),
  );
  if (safeScheduler) {
    box.body.append(mutedNote("持久同步仅追加已有目标表数据；不复制结构、建表或删除目标额外行。"));
    const check = structure.querySelector<HTMLInputElement>("input"); if (check) check.checked = false;
  }
  return {
    el: box.el,
    connId: () => srcConn.value,
    db: () => srcDb.value.trim(),
    validate: () => {
      if (!srcConn.value) return "请选择源连接";
      if (srcPicker.problem()) return srcPicker.problem();
      if (srcTablePicker.problem()) return srcTablePicker.problem();
      if (!dstConn.value) return "请选择目标连接";
      if (dstPicker.problem()) return dstPicker.problem();
      if (!safeScheduler && isChecked(createTarget)) { if (!newTargetName.value.trim()) return "请填写新目标表名"; }
      else if (dstTablePicker.problem()) return dstTablePicker.problem();
      if (safeScheduler && (isChecked(structure) || !isChecked(data))) return "持久同步必须选择复制数据，不得选择复制结构";
      return null;
    },
    options: () => {
      if (safeScheduler) return { ...opts, task_id: null, source_connection: srcConn.value, source_schema: srcDb.value.trim(), source_table: srcTable.value.trim(), target_connection: dstConn.value, target_schema: dstDb.value.trim(), target_table: targetTableName(), mode: "data", write_mode: "append", sync_mode: optString(opts, "sync_mode", "full"), incremental_update: optBool(opts, "incremental_update", false), create_target: false, delete_missing: false, key_columns: Array.isArray(opts.key_columns) ? opts.key_columns : [], columns: Array.isArray(opts.columns) ? opts.columns : [], where_clause: typeof opts.where_clause === "string" ? opts.where_clause : null };
      const transfer: TransferOptions = {
        source_connection: srcConn.value,
        source_schema: srcDb.value.trim(),
        source_table: srcTable.value.trim(),
        target_connection: dstConn.value,
        target_schema: dstDb.value.trim(),
        target_table: targetTableName(),
        // 定时同步默认在目标不存在时建表；用户可改为仅写入已有目标表
        create_target: !safeScheduler && isChecked(createTarget),
        drop_existing: false,
        copy_structure: isChecked(structure),
        copy_data: isChecked(data),
        where_clause: null,
      };
      return { ...transfer };
    },
  };
}

/** 数据生成任务配置区。 */
function buildGenBox(connections: ConnectionConfig[], opts: Record<string, unknown>, initConn: string, initDb: string): TaskBox {
  const conn = connSelect(connections, initConn);
  const dbPicker = createDatabasePicker(connections, conn, initDb), db = dbPicker.select;
  const tablePicker = createTablePicker(connections, conn, db, optString(opts, "table"));
  const count = numberInput(optNumber(opts, "count") ?? 100);
  count.min = "1";
  count.max = "50000";
  const seed = optionalNumber(optNumber(opts, "seed"), "留空每次随机");
  const box = groupbox("数据生成配置");
  box.body.append(
    field("连接:", conn),
    dbPicker.el,
    tablePicker.el,
    field("生成行数:", count),
    field("随机种子:", seed),
  );
  return {
    el: box.el,
    connId: () => conn.value,
    db: () => db.value.trim(),
    validate: () => {
      if (!conn.value) return "请选择连接";
      if (dbPicker.problem()) return dbPicker.problem();
      if (tablePicker.problem()) return tablePicker.problem();
      const countRaw = count.value.trim();
      if (!/^\d+$/.test(countRaw) || Number(countRaw) < 1 || Number(countRaw) > 50000) return "生成行数必须是 1–50000 的整数";
      const seedRaw = seed.value.trim();
      if (seedRaw && !/^\d+$/.test(seedRaw)) return "随机种子必须是非负整数";
      return null;
    },
    options: () => ({
      connection_id: conn.value,
      schema: db.value.trim(),
      table: tablePicker.value(),
      count: Number(count.value),
      seed: seed.value.trim() ? Number(seed.value) : null,
    }),
  };
}

/** 数据字典任务配置区（定时导出全库数据字典文档）。 */
function buildDictBox(connections: ConnectionConfig[], opts: Record<string, unknown>, initConn: string, initDb: string): TaskBox {
  const conn = connSelect(connections, initConn);
  const dbPicker = createDatabasePicker(connections, conn, initDb), db = dbPicker.select;
  const format = select(DICT_FORMATS, optString(opts, "format", "markdown"), 140);
  const sheetMode = select(
    [
      { value: "single", label: "所有表放在一个 sheet" },
      { value: "per_table", label: "每张表单独一个 sheet" },
    ],
    optString(opts, "excel_sheet_mode", "single"),
    200,
  );
  const sheetField = field("Excel 工作表:", sheetMode);
  const file = textInput(optString(opts, "file"));
  file.setAttribute("aria-label", "数据字典输出文件完整路径");
  const destination = bindExportPath(file, () => `${db.value || initDb || "database"}数据库设计文档.${DICT_EXTENSIONS[format.value] ?? "md"}`);
  db.addEventListener("change", destination.autoName);
  format.addEventListener("change", destination.autoName);
  const fileRow = fileLine(file, true, "保存数据字典", () => [DICT_FILE_FILTERS[format.value] ?? DICT_FILE_FILTERS.markdown], destination);
  const syncSheet = (): void => { sheetField.hidden = format.value !== "excel"; };
  format.addEventListener("change", syncSheet);
  syncSheet();
  const box = groupbox("数据字典配置");
  box.body.append(
    field("连接:", conn),
    dbPicker.el,
    field("文档格式:", format),
    sheetField,
    field("输出文件:", fileRow),
    mutedNote("定时导出全库表结构；PDF / Markdown 另含视图与存储过程。"),
  );
  return {
    el: box.el,
    connId: () => conn.value,
    db: () => db.value.trim(),
    validate: () => {
      if (!conn.value) return "请选择连接";
      if (dbPicker.problem()) return dbPicker.problem();
      if (destination.problem()) return destination.problem();
      return null;
    },
    options: () => ({
      connection_id: conn.value,
      schema: db.value.trim(),
      format: format.value,
      excel_sheet_mode: sheetMode.value,
      file: file.value.trim(),
    }),
  };
}

/** Reuses existing tool editors without saving or dispatching a legacy timed job. */
export function buildScheduleToolBox(connections: ConnectionConfig[], kind: "backup" | "export" | "import" | "data_sync", options: Record<string, unknown>, connection: string, namespace: string, withTablePicker = false): TaskBox {
  if (kind === "backup") return buildBackupBox(connections, options, connection, namespace, true, withTablePicker, withTablePicker);
  if (kind === "export") return buildExportBox(connections, options, connection, namespace);
  if (kind === "import") return buildImportBox(connections, options, connection, namespace);
  return buildSyncBox(connections, options, connection, namespace, true);
}

/** 打开定时任务编辑对话框（新建时 job 传 null）。 */
export function openScheduleJobDialog(
  connections: ConnectionConfig[],
  job: ScheduleJob | null,
  onSaved: () => void,
): Dialog {
  const isEdit = job !== null && job.id !== "";
  const taskOpts: Record<string, unknown> = job?.task?.options ?? {};
  // 旧版 SQL 任务（task 为空且 sql 非空）：仅支持修改名称与定时
  const legacyJob = job !== null && job.task == null && job.sql !== "" ? job : null;
  const initialKind: ScheduleTaskKind = job?.task?.kind ?? "backup";

  // 任务信息区
  const nameInput = textInput(job?.name ?? "");
  const enabledBox = checkbox("启用", job?.enabled ?? true);
  const kindRadios = radioGroup("schedule-task-kind", TASK_KINDS, initialKind);
  const kindField = field("动作类型:", kindRadios);

  // 任务配置区：每个动作类型一个 groupbox，切换显示
  const initConn = pickConn(connections, optString(taskOpts, "connection_id") || job?.connection_id || "");
  const initDb = optString(taskOpts, "schema") || job?.database || "";
  const taskBoxes: Record<ScheduleTaskKind, TaskBox> = {
    backup: buildBackupBox(connections, taskOpts, initConn, initDb, false, true),
    export: buildExportBox(connections, taskOpts, initConn, initDb),
    import: buildImportBox(connections, taskOpts, initConn, initDb),
    data_sync: buildSyncBox(
      connections,
      taskOpts,
      pickConn(connections, optString(taskOpts, "source_connection") || job?.connection_id || ""),
      optString(taskOpts, "source_schema") || job?.database || "",
    ),
    data_generation: buildGenBox(connections, taskOpts, initConn, initDb),
    data_dictionary: buildDictBox(connections, taskOpts, initConn, initDb),
  };

  // 旧版 SQL 任务区（只读展示）
  const legacyBox = groupbox("旧版 SQL 任务");
  const legacySqlArea = document.createElement("textarea");
  legacySqlArea.readOnly = true;
  legacySqlArea.value = legacyJob?.sql ?? "";
  // 高度内联固定（base.css 里的 schedule-job-dialog textarea 高度只在矮屏媒体查询下生效）
  legacySqlArea.style.height = "140px";
  legacyBox.body.append(
    field("SQL:", legacySqlArea),
    mutedNote("旧版 SQL 任务仅支持修改名称与定时，建议重建为任务型任务。"),
  );

  // 定时配置区
  const initialMode = job?.cron ? "cron" : job?.daily_at ? "daily" : job !== null && job.interval_secs > 0 ? "interval" : "manual";
  const scheduleModeRadios = radioGroup("schedule-mode", SCHEDULE_MODES, initialMode);
  const intervalSelect = select(
    INTERVALS.map((i) => ({ value: String(i.value), label: i.label })),
    String(job?.interval_secs ?? 300),
  );
  const intervalField = field("执行间隔:", intervalSelect);
  const dailyInput = textInput(job?.daily_at ?? "");
  dailyInput.placeholder = "HH:MM（24 小时制）";
  const dailyField = field("每天执行时刻:", dailyInput);
  const cronBuilder = buildCronBuilder();
  if (job?.cron) cronBuilder.restore(job.cron);
  const schedBox = groupbox("定时配置");
  schedBox.body.append(field("模式:", scheduleModeRadios), intervalField, dailyField, cronBuilder.el);

  const currentKind = (): ScheduleTaskKind => {
    const value = kindRadios.querySelector<HTMLInputElement>("input:checked")?.value;
    return (value ?? "backup") as ScheduleTaskKind;
  };
  const scheduleMode = (): string =>
    scheduleModeRadios.querySelector<HTMLInputElement>("input:checked")?.value ?? "manual";

  /** 按动作类型 / 定时模式切换各配置区显示。 */
  function refreshVisibility(): void {
    kindField.hidden = legacyJob !== null;
    legacyBox.el.hidden = legacyJob === null;
    const kind = currentKind();
    for (const key of Object.keys(taskBoxes) as ScheduleTaskKind[]) {
      taskBoxes[key].el.hidden = legacyJob !== null || key !== kind;
    }
    const mode = scheduleMode();
    intervalField.hidden = mode !== "interval";
    dailyField.hidden = mode !== "daily";
    cronBuilder.el.hidden = mode !== "cron";
  }
  kindRadios.addEventListener("change", refreshVisibility);
  scheduleModeRadios.addEventListener("change", refreshVisibility);

  /** 校验定时配置，返回错误文案或 null。 */
  function scheduleProblem(): string | null {
    const mode = scheduleMode();
    if (mode === "daily") {
      const at = dailyInput.value.trim();
      if (!/^\d{1,2}:\d{2}$/.test(at)) return "每天执行时刻必须是 HH:MM（24 小时制）";
      const parts = at.split(":").map(Number);
      if (parts[0] > 23 || parts[1] > 59) return "每天执行时刻必须是 HH:MM（24 小时制）";
      return null;
    }
    return mode === "cron" ? cronBuilder.problem() : null;
  }

  const dlg = new Dialog({
    title: isEdit ? "编辑任务" : "新建任务",
    width: 640,
    render: (body) => {
      body.append(
        field("任务名:", nameInput),
        checkRow(enabledBox),
        kindField,
        legacyBox.el,
        taskBoxes.backup.el,
        taskBoxes.export.el,
        taskBoxes.import.el,
        taskBoxes.data_sync.el,
        taskBoxes.data_generation.el,
        taskBoxes.data_dictionary.el,
        schedBox.el,
      );
      refreshVisibility();
    },
    buttons: [
      {
        label: "保存",
        primary: true,
        onClick: async () => {
          if (!nameInput.value.trim()) {
            dlg.showError("请填写任务名");
            return false;
          }
          if (legacyJob === null) {
            const taskErr = taskBoxes[currentKind()].validate();
            if (taskErr) {
              dlg.showError(taskErr);
              return false;
            }
          }
          const schedErr = scheduleProblem();
          if (schedErr) {
            dlg.showError(schedErr);
            return false;
          }
          // 三种定时互斥：cron 填写后优先，其次 daily_at，最后 interval_secs
          const mode = scheduleMode();
          let intervalSecs = 0;
          let dailyAt: string | null = null;
          let cronExpr: string | null = null;
          if (mode === "interval") intervalSecs = Number(intervalSelect.value);
          else if (mode === "daily") dailyAt = dailyInput.value.trim();
          else if (mode === "cron") cronExpr = cronBuilder.build();
          let connectionId = legacyJob?.connection_id ?? "";
          let database = legacyJob?.database ?? "";
          let task: ScheduleJob["task"] = null;
          if (legacyJob === null) {
            const box = taskBoxes[currentKind()];
            connectionId = box.connId();
            database = box.db();
            task = { kind: currentKind(), options: box.options() };
          }
          try {
            await api.saveJob({
              id: job?.id ?? "",
              name: nameInput.value.trim(),
              connection_id: connectionId,
              database,
              sql: legacyJob !== null ? legacyJob.sql : job?.sql ?? "",
              interval_secs: intervalSecs,
              daily_at: dailyAt,
              cron: cronExpr,
              task,
              enabled: isChecked(enabledBox),
              last_run: job?.last_run ?? null,
              last_status: job?.last_status ?? null,
            });
            onSaved();
          } catch (e) {
            dlg.showError(String(e));
            return false;
          }
          return true;
        },
      },
      { label: "取消" },
    ],
  });
  dlg.el.classList.add("form-stacked", "schedule-job-dialog");
  return dlg;
}

/** 运行历史对话框。 */
export function openJobHistoryDialog(history: JobHistoryEntry[], jobId?: string): Dialog {
  const grid = new DataGrid({ onRowDoubleClick: () => {} });
  const rows = (jobId ? history.filter((h) => h.job_id === jobId) : history).map((h) => [
    h.job_name,
    h.started_at,
    h.ok ? "成功" : "失败",
    `${(h.elapsed_ms / 1000).toFixed(2)} s`,
    h.detail,
  ]);
  const dlg = new Dialog({
    title: "运行历史",
    width: 760,
    height: 460,
    render: (body) => {
      body.style.display = "flex";
      body.style.flexDirection = "column";
      body.appendChild(grid.el);
      grid.setData({
        columns: [
          { name: "任务", type: "text", nullable: false, primary_key: false, auto_increment: false },
          { name: "开始时间", type: "text", nullable: true, primary_key: false, auto_increment: false },
          { name: "状态", type: "text", nullable: true, primary_key: false, auto_increment: false },
          { name: "耗时", type: "text", nullable: true, primary_key: false, auto_increment: false },
          { name: "详情", type: "text", nullable: true, primary_key: false, auto_increment: false },
        ],
        rows,
        rowIcons: rows.map(() => "info_pane_query"),
        showRowNumbers: false,
        fillWidth: true,
      });
    },
    buttons: [{ label: "关闭" }],
  });
  // 对话框关闭后释放网格：DataGrid 在 window 上挂了 mousemove/mouseup/blur 监听
  // 和 ResizeObserver，不 dispose 会随着每次打开运行历史持续累积。
  dlg.onClose(() => grid.dispose());
  return dlg;
}
