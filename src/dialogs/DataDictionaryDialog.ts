/**
 * 数据字典导出向导（需求 1.1）：数据源 → 对象 → 格式 → 文件 → 导出。
 *
 * 模板列集：序号/列名/数据类型/长度/主键/自增/允许空/默认值/列说明；
 * 界面按 docs/design/data-dictionary-wizard.html 的向导结构 + NavKit 设计令牌实现：
 *  · Excel / Word / HTML 仅导出表；PDF / Markdown 支持视图与存储过程
 *  · Excel 可选「所有表一个 sheet」或「每张表一个 sheet」
 *  · 导出走后台任务中心（可「后台执行」，结束后在任务面板打开产物）
 */

import { api } from "../core/api";
import type { ConnectionConfig, DictionaryDatabase, DictionaryExportOptions } from "../core/types";
import { Dialog, field, textInput } from "../ui/Dialog";
import { iconSvgByName } from "../ui/icons";
import { ObjectTree } from "../ui/ObjectTree";
import { accessibleDialog } from "./SyncDialog";
import { Wizard } from "../ui/Wizard";
import { TransferProgress } from "../ui/TransferProgress";
import { taskCenter } from "../core/backgroundTasks";
import "./../styles/data-dictionary.css";

/** 文档名不允许出现的字符（Windows 保留字符）。 */
const FILE_NAME_INVALID = /[/\\:*?"<>|]/;

type DictFormat = "word" | "pdf" | "html" | "markdown" | "excel";

/** 格式卡片定义（icon 为完整内联 SVG，采用各格式官方品牌 logo 与品牌色）。 */
const FORMATS: {
  id: DictFormat;
  label: string;
  ext: string;
  scope: "表" | "全部";
  filter: { name: string; extensions: string[] };
  icon: string;
}[] = [
  {
    id: "word",
    label: "Word",
    ext: ".docx",
    scope: "表",
    filter: { name: "Word 文档", extensions: ["docx"] },
    icon: `<svg viewBox="0 0 24 24" fill="#2B579A" xmlns="http://www.w3.org/2000/svg" aria-hidden="true"><path d="M23.004 1.5q.41 0 .703.293t.293.703v19.008q0 .41-.293.703t-.703.293H6.996q-.41 0-.703-.293T6 21.504V18H.996q-.41 0-.703-.293T0 17.004V6.996q0-.41.293-.703T.996 6H6V2.496q0-.41.293-.703t.703-.293zM6.035 11.203l1.442 4.735h1.64l1.57-7.876H9.036l-.937 4.653-1.325-4.5H5.38l-1.406 4.523-.938-4.675H1.312l1.57 7.874h1.641zM22.5 21v-3h-15v3zm0-4.5v-3.75H12v3.75zm0-5.25V7.5H12v3.75zm0-5.25V3h-15v3Z"/></svg>`,
  },
  {
    id: "pdf",
    label: "PDF",
    ext: ".pdf",
    scope: "全部",
    filter: { name: "PDF 文档", extensions: ["pdf"] },
    icon: `<svg viewBox="0 0 24 24" fill="#EC1C24" xmlns="http://www.w3.org/2000/svg" aria-hidden="true"><path d="M23.63 15.3c-.71-.745-2.166-1.17-4.224-1.17-1.1 0-2.377.106-3.761.354a19.443 19.443 0 0 1-2.307-2.661c-.532-.71-.994-1.49-1.42-2.236.817-2.484 1.207-4.507 1.207-5.962 0-1.632-.603-3.336-2.342-3.336-.532 0-1.065.32-1.349.781-.78 1.384-.425 4.4.923 7.381a60.277 60.277 0 0 1-1.703 4.507c-.568 1.349-1.207 2.733-1.917 4.01C2.834 18.53.314 20.34.03 21.758c-.106.533.071 1.03.462 1.42.142.107.639.533 1.49.533 2.59 0 5.323-4.188 6.707-6.707 1.065-.355 2.13-.71 3.194-.994a34.963 34.963 0 0 1 3.407-.745c2.732 2.448 5.145 2.839 6.352 2.839 1.49 0 2.023-.604 2.2-1.1.32-.64.106-1.349-.213-1.704zm-1.42 1.03c-.107.532-.64.887-1.384.887-.213 0-.39-.036-.604-.071-1.348-.32-2.626-.994-3.903-2.059a17.717 17.717 0 0 1 2.98-.248c.746 0 1.385.035 1.81.142.497.106 1.278.426 1.1 1.348zm-7.524-1.668a38.01 38.01 0 0 0-2.945.674 39.68 39.68 0 0 0-2.52.745 40.05 40.05 0 0 0 1.207-2.555c.426-.994.78-2.023 1.136-2.981.354.603.745 1.207 1.135 1.739a50.127 50.127 0 0 0 1.987 2.378zM10.038 1.46a.768.768 0 0 1 .674-.425c.745 0 .887.851.887 1.526 0 1.135-.355 2.874-.958 4.861-1.03-2.768-1.1-5.074-.603-5.962zM6.134 17.997c-1.81 2.981-3.549 4.826-4.613 4.826a.872.872 0 0 1-.532-.177c-.213-.213-.32-.461-.249-.745.213-1.065 2.271-2.555 5.394-3.904Z"/></svg>`,
  },
  {
    id: "html",
    label: "HTML",
    ext: ".html",
    scope: "表",
    filter: { name: "HTML 文件", extensions: ["html"] },
    icon: `<svg viewBox="0 0 24 24" fill="#E34F26" xmlns="http://www.w3.org/2000/svg" aria-hidden="true"><path d="M1.5 0h21l-1.91 21.563L11.977 24l-8.564-2.438L1.5 0zm7.031 9.75l-.232-2.718 10.059.003.23-2.622L5.412 4.41l.698 8.01h9.126l-.326 3.426-2.91.804-2.955-.81-.188-2.11H6.248l.33 4.171L12 19.351l5.379-1.443.744-8.157H8.531z"/></svg>`,
  },
  {
    id: "markdown",
    label: "Markdown",
    ext: ".md",
    scope: "全部",
    filter: { name: "Markdown 文件", extensions: ["md"] },
    icon: `<svg viewBox="0 0 24 24" fill="#1B1F23" xmlns="http://www.w3.org/2000/svg" aria-hidden="true"><path d="M22.27 19.385H1.73A1.73 1.73 0 010 17.655V6.345a1.73 1.73 0 011.73-1.73h20.54A1.73 1.73 0 0124 6.345v11.308a1.73 1.73 0 01-1.73 1.731zM5.769 15.923v-4.5l2.308 2.885 2.307-2.885v4.5h2.308V8.078h-2.308l-2.307 2.885-2.308-2.885H3.46v7.847zM21.232 12h-2.309V8.077h-2.307V12h-2.308l3.461 4.039z"/></svg>`,
  },
  {
    id: "excel",
    label: "Excel",
    ext: ".xlsx",
    scope: "表",
    filter: { name: "Excel 工作簿", extensions: ["xlsx"] },
    icon: `<svg viewBox="0 0 24 24" fill="#217346" xmlns="http://www.w3.org/2000/svg" aria-hidden="true"><path d="M23 1.5q.41 0 .7.3.3.29.3.7v19q0 .41-.3.7-.29.3-.7.3H7q-.41 0-.7-.3-.3-.29-.3-.7V18H1q-.41 0-.7-.3-.3-.29-.3-.7V7q0-.41.3-.7Q.58 6 1 6h5V2.5q0-.41.3-.7.29-.3.7-.3zM6 13.28l1.42 2.66h2.14l-2.38-3.87 2.34-3.8H7.46l-1.3 2.4-.05.08-.04.09-.64-1.28-.66-1.29H2.59l2.27 3.82-2.48 3.85h2.16zM14.25 21v-3H7.5v3zm0-4.5v-3.75H12v3.75zm0-5.25V7.5H12v3.75zm0-5.25V3H7.5v3zm8.25 15v-3h-6.75v3zm0-4.5v-3.75h-6.75v3.75zm0-5.25V7.5h-6.75v3.75zm0-5.25V3h-6.75v3Z"/></svg>`,
  },
];

/** 某格式是否支持视图 / 存储过程（仅 PDF / Markdown）。 */
function formatSupportsObjects(format: DictFormat): boolean {
  return format === "pdf" || format === "markdown";
}

/** 勾选条目（对象步）。 */
interface PickItem {
  kind: "table" | "view" | "routine";
  typeLabel: "procedure" | "function";
  name: string;
  comment: string;
  input: HTMLInputElement;
  row: HTMLElement;
}

export function openDataDictionaryDialog(
  connections: ConnectionConfig[],
  current: ConnectionConfig | undefined,
  currentSchema: string,
  onDone: (path: string) => void,
): Dialog {
  let wizard!: Wizard;
  let running = false;
  let metadata: DictionaryDatabase | null = null;
  let metaLoaded = false;

  const usable = connections;

  // ---------------------------------------------------------------- 步骤 1：数据源
  const connSelect = document.createElement("select");
  connSelect.style.width = "100%";
  for (const c of usable) {
    const opt = document.createElement("option");
    opt.value = c.id;
    opt.textContent = `${c.name} · ${c.kind}`;
    connSelect.appendChild(opt);
  }
  if (current) connSelect.value = current.id;
  const dbSelect = document.createElement("select");
  dbSelect.style.width = "100%";
  const dbHint = document.createElement("div");
  dbHint.style.cssText = "font-size:11px;color:var(--text-tertiary);min-height:16px;";
  const infoRow = document.createElement("div");
  infoRow.style.cssText = "display:flex;flex-wrap:wrap;gap:6px;font-size:11px;color:var(--text-secondary);";

  const connById = (id: string): ConnectionConfig | undefined => usable.find((c) => c.id === id);

  let databasesVersion = 0;
  async function reloadDatabases(prefer?: string): Promise<void> {
    const version = ++databasesVersion;
    const cfg = connById(connSelect.value);
    dbSelect.disabled = true;
    dbSelect.setAttribute("aria-busy", "true");
    dbSelect.replaceChildren();
    infoRow.replaceChildren();
    metadata = null;
    metaLoaded = false;
    if (!cfg) {
      dbHint.textContent = "请先创建连接";
      dbSelect.removeAttribute("aria-busy");
      return;
    }
    dbHint.textContent = "正在读取数据库列表...";
    try {
      const dbs = await api.listDatabases(cfg.id);
      if (version !== databasesVersion) return;
      for (const [name] of dbs) {
        const opt = document.createElement("option");
        opt.value = name;
        opt.textContent = name;
        dbSelect.appendChild(opt);
      }
      if (prefer && dbs.some(([name]) => name === prefer)) dbSelect.value = prefer;
      dbSelect.disabled = dbs.length === 0;
      dbHint.textContent = dbs.length ? "" : "该连接没有可用的数据库";
    } catch (e) {
      if (version === databasesVersion) dbHint.textContent = `数据库列表读取失败：${String(e)}`;
    } finally {
      if (version === databasesVersion) dbSelect.removeAttribute("aria-busy");
    }
  }
  connSelect.addEventListener("change", () => void reloadDatabases());

  const p1 = document.createElement("div");
  p1.style.display = "flex";
  p1.style.flexDirection = "column";
  p1.style.gap = "10px";
  p1.append(
    field("连接:", connSelect),
    field("数据库:", dbSelect),
    dbHint,
    infoRow,
  );

  // ---------------------------------------------------------------- 步骤 2：对象
  const searchInput = textInput("");
  searchInput.placeholder = "搜索表 / 视图 / 存储过程";
  searchInput.style.width = "100%";
  // 对象选择（与「新建备份」一致的树形结构）：分组可折叠、可全选，行尾显示注释。
  const pickTree = new ObjectTree({ ariaLabel: "选择导出对象" });
  pickTree.el.style.flex = "1";
  pickTree.el.style.minHeight = "0";
  const pickHost = document.createElement("div");
  pickHost.style.cssText = "min-height:0;height:100%;display:flex;flex-direction:column;";
  pickHost.append(pickTree.el);
  const selList = document.createElement("div");
  selList.style.cssText =
    "flex:1;min-height:0;overflow:auto;border:1px solid var(--border-subtle);border-radius:6px;padding:4px;background:var(--bg-surface);";
  const selCount = document.createElement("div");
  selCount.style.cssText = "font-size:11px;color:var(--text-tertiary);margin-bottom:4px;";
  const loadHint = document.createElement("div");
  loadHint.style.cssText = "font-size:11px;color:var(--text-tertiary);";
  let items: PickItem[] = [];

  function isItemIncluded(it: PickItem, format: DictFormat): boolean {
    if (it.kind === "table") return true;
    return formatSupportsObjects(format) && (it.kind === "view" ? includeViewsInput.checked : includeRoutinesInput.checked);
  }

  function refreshSelection(): void {
    const selected = items.filter((it) => it.input.checked && isItemIncluded(it, selectedFormat));
    selCount.textContent = `导出顺序：${selected.length} 项`;
    selList.replaceChildren();
    for (const it of selected) {
      const row = document.createElement("div");
      row.style.cssText =
        "display:flex;align-items:center;gap:8px;height:26px;margin-bottom:2px;padding:0 8px;border-radius:4px;font-size:12px;";
      const ic = document.createElement("span");
      ic.style.display = "inline-flex";
      ic.innerHTML = iconSvgByName(it.kind === "table" ? "info_pane_table" : "info_pane_view");
      const name = document.createElement("span");
      name.textContent = it.name;
      const x = document.createElement("button");
      x.type = "button";
      x.textContent = "✕";
      x.setAttribute("aria-label", `移除 ${it.name}`);
      x.style.cssText =
        "margin-left:auto;border:0;background:transparent;color:var(--text-disabled);cursor:pointer;font-size:11px;";
      x.addEventListener("click", () => {
        it.input.checked = false;
        refreshSelection();
      });
      row.append(ic, name, x);
      selList.appendChild(row);
    }
    const tables = selected.filter((it) => it.kind === "table").length;
    const views = selected.filter((it) => it.kind === "view").length;
    const routines = selected.filter((it) => it.kind === "routine").length;
    selCount.textContent = `已选 ${tables} 表 · ${views} 视图 · ${routines} 存储过程/函数`;
  }

  function renderPickTree(): void {
    pickTree.clear();
    const groups: Record<PickItem["kind"], ReturnType<ObjectTree["addGroup"]>> = {
      table: pickTree.addGroup({ title: "表", icon: "info_pane_table" }),
      view: pickTree.addGroup({ title: "视图", icon: "info_pane_view" }),
      routine: pickTree.addGroup({ title: "存储过程 / 函数", icon: "info_pane_function" }),
    };
    for (const it of items) {
      const handle = pickTree.addItem(groups[it.kind], {
        name: it.name,
        icon: it.kind === "table" ? "info_pane_table" : it.kind === "view" ? "info_pane_view" : "info_pane_function",
        checked: it.kind === "table",
        meta: it.comment,
        onChange: refreshSelection,
      });
      it.input = handle.input;
      it.row = handle.row;
    }
    pickTree.filter(searchInput.value);
    refreshSelection();
  }

  async function loadMetadata(): Promise<void> {
    const cfg = connById(connSelect.value);
    if (!cfg || !dbSelect.value) return;
    if (metaLoaded && metadata) return;
    loadHint.textContent = "正在读取对象元数据...";
    try {
      metadata = await api.dictionaryMetadata(cfg.id, dbSelect.value);
      const mk = (
        kind: PickItem["kind"],
        name: string,
        comment: string,
        typeLabel: PickItem["typeLabel"] = "procedure",
      ): PickItem => {
        // 勾选框由统一对象树创建（renderPickTree），这里仅占位，稍后回填。
        return { kind, typeLabel, name, comment, input: document.createElement("input"), row: document.createElement("div") };
      };
      items = [
        ...metadata.tables.map((t) => mk("table", t.name, t.comment)),
        ...metadata.views.map((v) => mk("view", v.name, v.comment)),
        ...metadata.routines.map((r) => mk("routine", r.name, r.comment, r.name.toLowerCase().includes("func") ? "function" : "procedure")),
      ];
      const total = metadata.tables.length + metadata.views.length + metadata.routines.length;
      infoRow.replaceChildren();
      const pill = (text: string): HTMLElement => {
        const s = document.createElement("span");
        s.style.cssText =
          "padding:2px 8px;border-radius:999px;background:var(--bg-hover);border:1px solid var(--border-subtle);font-size:11px;";
        s.textContent = text;
        return s;
      };
      infoRow.append(
        pill(`${metadata.tables.length} 张表`),
        pill(`${metadata.views.length} 视图`),
        pill(`${metadata.routines.length} 存储过程/函数`),
      );
      loadHint.textContent = total ? "" : "该数据库没有可导出的对象";
      metaLoaded = true;
      renderPickTree();
    } catch (e) {
      loadHint.textContent = `对象元数据读取失败：${String(e)}`;
      dlg.showError(String(e));
    }
  }

  const p2 = document.createElement("div");
  p2.style.cssText = "display:flex;flex-direction:column;gap:8px;height:100%;min-height:0;";
  const searchWrap = field("搜索:", searchInput);
  const pickGrid = document.createElement("div");
  pickGrid.style.cssText = "flex:1;min-height:260px;display:grid;grid-template-columns:1fr 260px;gap:10px;";
  const selPane = document.createElement("div");
  selPane.style.cssText = "display:flex;flex-direction:column;min-height:0;";
  selPane.append(selCount, selList);
  pickGrid.append(pickHost, selPane);
  p2.append(searchWrap, pickGrid, loadHint);
  searchInput.addEventListener("input", () => pickTree.filter(searchInput.value));

  // ---------------------------------------------------------------- 步骤 3：格式
  let selectedFormat: DictFormat = "markdown";
  const fmtGrid = document.createElement("div");
  fmtGrid.style.cssText = "display:grid;grid-template-columns:repeat(5,1fr);gap:8px;";
  const fmtCards = new Map<DictFormat, HTMLButtonElement>();
  for (const f of FORMATS) {
    const card = document.createElement("button");
    card.type = "button";
    card.className = "dict-fmt-card";
    card.dataset.format = f.id;
    card.innerHTML = `
      <span class="dict-fmt-tick" aria-hidden="true">✓</span>
      <span class="dict-fmt-ic" aria-hidden="true">${f.icon}</span>
      <span class="dict-fmt-t">${f.label}</span>
      <span class="dict-fmt-x">${f.ext}</span>
      <span class="dict-fmt-cap">${f.scope}</span>`;
    card.addEventListener("click", () => {
      selectedFormat = f.id;
      for (const [id, el] of fmtCards) el.classList.toggle("on", id === selectedFormat);
      syncSectionsVisibility();
      refreshSelection();
      refreshGeneratedName();
    });
    fmtCards.set(f.id, card);
    fmtGrid.appendChild(card);
  }

  // Excel sheet 模式
  const excelOpt = document.createElement("div");
  excelOpt.style.cssText =
    "border-top:1px solid var(--border-subtle);padding-top:10px;margin-top:10px;display:flex;flex-direction:column;gap:6px;";
  const excelLabel = document.createElement("div");
  excelLabel.style.cssText = "font-size:11px;color:var(--text-tertiary);font-weight:600;";
  excelLabel.textContent = "Excel 工作表方式";
  const singleRadio = document.createElement("input");
  singleRadio.type = "radio";
  singleRadio.name = "dict-xl-mode";
  singleRadio.value = "single";
  singleRadio.checked = true;
  const perTableRadio = document.createElement("input");
  perTableRadio.type = "radio";
  perTableRadio.name = "dict-xl-mode";
  perTableRadio.value = "per_table";
  const xlItem = (radio: HTMLInputElement, title: string, desc: string): HTMLElement => {
    const label = document.createElement("label");
    label.style.cssText =
      "display:flex;gap:8px;align-items:flex-start;padding:8px 10px;border:1px solid var(--border-subtle);border-radius:4px;cursor:pointer;";
    radio.style.margin = "3px 0 0";
    const body = document.createElement("span");
    body.style.display = "flex";
    body.style.flexDirection = "column";
    const b = document.createElement("b");
    b.style.fontSize = "12px";
    b.textContent = title;
    const d = document.createElement("span");
    d.style.cssText = "font-size:11px;color:var(--text-tertiary);";
    d.textContent = desc;
    body.append(b, d);
    label.append(radio, body);
    return label;
  };
  excelOpt.append(
    excelLabel,
    xlItem(singleRadio, "所有表放在一个 sheet", "含修订日志 / 目录 / 表三个工作表，表结构汇总在「表」中"),
    xlItem(perTableRadio, "每张表单独一个 sheet", "修订日志 / 目录之外，每张表各占一个工作表"),
  );

  // 章节包含（表结构必选）
  const sectionBox = document.createElement("div");
  sectionBox.style.cssText = "border-top:1px solid var(--border-subtle);padding-top:10px;margin-top:10px;display:flex;flex-direction:column;gap:8px;";
  const sectionChecks = document.createElement("div");
  sectionChecks.style.cssText = "display:flex;gap:8px 12px;flex-wrap:wrap;align-items:flex-start;";
  // 返回 label（复选框 + 标题 + 说明），只返回 input 会让说明文字整块消失，界面上只剩两个孤立方框。
  const mkSectionCheck = (labelText: string, sub: string, checked: boolean, disabled: boolean): { label: HTMLLabelElement; input: HTMLInputElement } => {
    const label = document.createElement("label");
    label.style.cssText =
      "display:flex;gap:10px;align-items:flex-start;cursor:pointer;padding:8px 10px;border:1px solid var(--border-subtle);border-radius:4px;min-width:190px;background:var(--bg-surface);";
    const input = document.createElement("input");
    input.type = "checkbox";
    input.checked = checked;
    input.disabled = disabled;
    input.style.cssText = "margin:2px 0 0;flex:0 0 14px;";
    input.setAttribute("aria-label", labelText);
    const body = document.createElement("span");
    body.style.display = "flex";
    body.style.flexDirection = "column";
    body.style.gap = "2px";
    const b = document.createElement("span");
    b.style.fontSize = "12px";
    b.textContent = labelText;
    const s = document.createElement("span");
    s.style.cssText = "font-size:10.5px;line-height:1.5;color:var(--text-tertiary);";
    s.textContent = sub;
    body.append(b, s);
    label.append(input, body);
    return { label, input };
  };
  const structureSection = mkSectionCheck("表结构", "必选 · 所有格式均输出", true, true);
  const viewsSection = mkSectionCheck("视图", "仅 PDF / Markdown 支持导出视图", true, false);
  const routinesSection = mkSectionCheck("存储过程 / 函数", "仅 PDF / Markdown 支持导出存储过程与函数", true, false);
  const includeViewsInput = viewsSection.input;
  const includeRoutinesInput = routinesSection.input;
  const sectionCaption = document.createElement("div");
  sectionCaption.style.cssText = "font-size:11px;color:var(--text-tertiary);font-weight:600;";
  sectionCaption.textContent = "文档包含内容";
  sectionChecks.append(structureSection.label, viewsSection.label, routinesSection.label);
  sectionBox.append(sectionCaption, sectionChecks);

  function syncSectionsVisibility(): void {
    const supports = formatSupportsObjects(selectedFormat);
    includeViewsInput.disabled = !supports;
    includeRoutinesInput.disabled = !supports;
    // 保留用户的章节选择（切回支持格式时原选择仍在），但不支持的格式里这两个选项整块不出现。
    viewsSection.label.style.display = supports ? "flex" : "none";
    routinesSection.label.style.display = supports ? "flex" : "none";
    // Excel 工作表方式只在选择 Excel 时出现。必须改 style.display：excelOpt 内联了
    // display:flex，会盖过 [hidden] 的 UA 样式，导致直接设 hidden 无效（此前 Markdown 下仍露出的原因）。
    excelOpt.style.display = selectedFormat === "excel" ? "flex" : "none";
  }

  includeViewsInput.addEventListener("change", refreshSelection);
  includeRoutinesInput.addEventListener("change", refreshSelection);

  const p3 = document.createElement("div");
  p3.style.display = "flex";
  p3.style.flexDirection = "column";
  p3.append(fmtGrid, excelOpt, sectionBox);

  // ---------------------------------------------------------------- 步骤 4：文件
  const nameInput = textInput("", 320);
  const pathInput = textInput("", 320);
  const browseBtn = document.createElement("button");
  browseBtn.type = "button";
  browseBtn.className = "btn";
  browseBtn.style.minWidth = "0";
  browseBtn.textContent = "浏览...";
  browseBtn.addEventListener("click", async () => {
    const format = FORMATS.find((f) => f.id === selectedFormat)!;
    try {
      const { save } = await import("@tauri-apps/plugin-dialog");
      const res = await save({ title: "保存数据字典", filters: [format.filter], defaultPath: defaultFileName() });
      if (res) {
        // save() 返回的是完整文件路径：拆成「目录 + 文档名」，否则目录里会再拼一次文件名。
        const cut = Math.max(res.lastIndexOf("\\"), res.lastIndexOf("/"));
        if (cut > 0) {
          pathInput.value = res.slice(0, cut);
          const stem = res.slice(cut + 1).replace(/\.[^.\\/]+$/, "");
          if (stem.trim()) nameInput.value = stem.trim();
        } else {
          pathInput.value = res;
        }
        refreshGeneratedName();
      }
    } catch (e) {
      dlg.showError(`无法打开保存对话框：${String(e)}`);
    }
  });
  const generated = document.createElement("div");
  generated.style.cssText =
    "font-family:var(--font-mono);font-size:11.5px;background:var(--bg-subtle);border:1px solid var(--border-subtle);border-radius:4px;padding:6px 8px;overflow-wrap:anywhere;";

  function defaultFileName(): string {
    const db = dbSelect.value || "database";
    const stamp = new Date()
      .toISOString()
      .replace(/[-:TZ.]/g, "")
      .slice(0, 14);
    return `${db}数据库设计文档${stamp}`;
  }
  function refreshGeneratedName(): void {
    generated.textContent = targetFilePath();
  }
  /** 输出目录：去掉尾部斜杠。 */
  function outputDirectory(): string {
    return pathInput.value.trim().replace(/[\\/]+$/, "");
  }
  /** 目标文件的完整路径：目录 + 文档名 + 扩展名。全流程唯一的拼接点。 */
  function targetFilePath(): string {
    const name = nameInput.value.trim() || defaultFileName();
    const format = FORMATS.find((f) => f.id === selectedFormat)!;
    const dir = outputDirectory();
    const file = `${name}${format.ext}`;
    if (!dir) return file;
    // 沿用目录里已有的分隔符，避免 Windows 路径混用 / 与 \
    return `${dir}${dir.includes("\\") ? "\\" : "/"}${file}`;
  }
  nameInput.addEventListener("input", refreshGeneratedName);
  pathInput.addEventListener("input", refreshGeneratedName);
  // 进入本步时预填默认名
  let namePrefilled = false;
  const p4 = document.createElement("div");
  p4.style.display = "flex";
  p4.style.flexDirection = "column";
  p4.style.gap = "10px";
  const pathRow = field("输出路径:", pathInput);
  pathRow.querySelector<HTMLElement>(".field-control")!.appendChild(browseBtn);
  p4.append(field("文档名称:", nameInput), pathRow, field("将生成:", generated));
  const nameNote = document.createElement("div");
  nameNote.style.cssText = "font-size:11px;color:var(--warn-600);";
  nameNote.textContent = "文档名称不能包含 \\ / : * ? \" < > | 字符";
  p4.appendChild(nameNote);

  // ---------------------------------------------------------------- 步骤 5：导出
  const progress = new TransferProgress("数据字典导出进度");
  const summary = document.createElement("div");
  summary.style.cssText = "font-size:11.5px;color:var(--text-tertiary);margin-top:8px;line-height:1.7;";
  const p5 = document.createElement("div");
  p5.append(summary, progress.el);

  /** 汇总当前配置。 */
  function describeSelection(): string {
    if (!metadata) return "";
    const selected = items.filter((it) => it.input.checked && isItemIncluded(it, selectedFormat));
    const tables = selected.filter((it) => it.kind === "table");
    const views = selected.filter((it) => it.kind === "view");
    const routines = selected.filter((it) => it.kind === "routine");
    return `连接 ${connById(connSelect.value)?.name ?? ""} · 库 ${dbSelect.value} · 表 ${tables.length}、视图 ${views.length}、过程 ${routines.length} · 格式 ${selectedFormat}`;
  }

  /** 文件名 / 路径校验。 */
  function filesProblem(): string | null {
    const name = nameInput.value.trim() || defaultFileName();
    if (FILE_NAME_INVALID.test(name)) return "文档名称不能包含 \\ / : * ? \" < > | 字符";
    if (!outputDirectory()) return "请填写输出路径（或点击浏览选择位置）";
    // 输出路径是目录：用户误把完整文件名填进来时（浏览/手输），这里直接拦住，避免生成 x.pdf\x.pdf。
    if (/\.(pdf|md|markdown|docx|xlsx|html?)$/i.test(outputDirectory())) {
      return "「输出路径」请填写目录；文件名由「文档名称」决定。";
    }
    return null;
  }

  /** 执行导出（注册后台任务；关闭窗口不中断任务，会转入任务中心）。 */
  async function runExport(): Promise<void> {
    if (running) return;
    const problem = filesProblem();
    if (problem) {
      dlg.showError(problem);
      return;
    }
    const cfg = connById(connSelect.value);
    if (!cfg || !dbSelect.value) {
      dlg.showError("请先选择连接与数据库");
      return;
    }
    if (!metadata) {
      dlg.showError("对象元数据尚未加载完成");
      return;
    }
    const selected = items.filter((it) => it.input.checked && isItemIncluded(it, selectedFormat));
    const tables = metadata.tables.filter((t) => selected.some((it) => it.kind === "table" && it.name === t.name));
    const views = metadata.views.filter((v) => selected.some((it) => it.kind === "view" && it.name === v.name));
    const routines = metadata.routines.filter((r) => selected.some((it) => it.kind === "routine" && it.name === r.name));
    if (!formatSupportsObjects(selectedFormat) && !tables.length) {
      dlg.showError("Excel / Word / HTML 至少需要选择一张表");
      return;
    }
    if (!tables.length && !views.length && !routines.length) {
      dlg.showError("请至少选择一个当前格式与文档章节允许导出的对象");
      return;
    }
    const name = nameInput.value.trim() || defaultFileName();
    const format = FORMATS.find((f) => f.id === selectedFormat)!;
    const filePath = targetFilePath();
    const opts: DictionaryExportOptions = {
      format: selectedFormat,
      excel_sheet_mode: perTableRadio.checked ? "per_table" : "single",
      include_tables: true,
      include_views: views.length > 0,
      include_routines: routines.length > 0,
      created_at: new Date().toLocaleString("zh-CN", { hour12: false }),
    };
    running = true;
    wizard.setBusy(true);
    progress.begin("正在导出，无法预估剩余量…");
    summary.textContent = describeSelection();
    const handle = taskCenter.register("dictionary", `数据字典导出 · ${dbSelect.value}（${selectedFormat.toUpperCase()}）`);
    const exportPromise = api
      .dictionaryExport(
        { ...metadata, tables, views, routines },
        opts,
        filePath,
      )
      .then((bytes) => {
        handle.finish(`已生成 ${name}${format.ext}（${(bytes / 1024).toFixed(1)} KB）`, filePath);
        return bytes;
      })
      .catch((e) => {
        handle.fail(String(e));
        throw e;
      });
    try {
      const bytes = await exportPromise;
      progress.finish(`导出完成：${name}${format.ext}（${(bytes / 1024).toFixed(1)} KB）`);
      exportedPath = filePath;
      // 已导出后，完成按钮改成「已导出」并禁用：避免重复点击又生成一份同名文档。
      wizard.nextBtn.textContent = "已导出";
      wizard.nextBtn.classList.remove("primary");
      wizard.refreshControls();
      onDone(filePath);
    } catch (e) {
      progress.fail(`导出失败：${String(e)}`);
      dlg.showError(String(e));
    } finally {
      running = false;
      wizard.setBusy(false);
    }
  }

  // ---------------------------------------------------------------- 组装向导
  // 固定窗口尺寸：各步内容多少不同，尺寸随内容跳动会让「下一步」在指针下移位。
  const dlg = new Dialog({ title: "数据字典", width: 820, height: 640, render: () => {}, buttons: [{ label: "关闭" }] });
  /** 已成功导出的目标路径：非空时禁用「开始导出」，改配置后恢复。 */
  let exportedPath: string | null = null;
  const resetExportState = () => {
    if (!exportedPath) return;
    exportedPath = null;
    wizard.nextBtn.textContent = "开始导出";
    wizard.nextBtn.classList.add("primary");
    wizard.refreshControls();
  };
  wizard = new Wizard({
    steps: [
      { label: "数据源", hint: "数据字典按库生成：先选连接，再选库。", render: (host) => host.appendChild(p1) },
      {
        label: "对象",
        hint: "勾选要导出的表 / 视图 / 存储过程；右侧为文档章节顺序（表 → 视图 → 存储过程）。",
        validate: () => (items.some((it) => it.input.checked) ? null : "请至少勾选一个导出对象"),
        render: (host) => host.appendChild(p2),
      },
      {
        label: "格式",
        hint: "文档类型为单选。Excel、Word、HTML 仅支持导出表；PDF、Markdown 支持视图与存储过程。",
        render: (host) => host.appendChild(p3),
      },
      {
        label: "文件",
        hint: "名称即文档标题（Word / PDF / Excel 首页标题、HTML 顶部标题、Markdown 目录名）。",
        validate: filesProblem,
        render: (host) => host.appendChild(p4),
      },
      {
        label: "导出",
        hint: "核对配置后开始导出。导出期间关闭窗口不会中断任务：它会转入后台，可在顶部任务中心查看进度与结果。",
        render: (host) => host.appendChild(p5),
      },
    ],
    onEnter: (index) => {
      if (index === 0 && dbSelect.options.length === 0) void reloadDatabases(currentSchema);
      if (index === 1) void loadMetadata();
      if (index === 3) {
        if (!namePrefilled) {
          nameInput.value = defaultFileName();
          namePrefilled = true;
        }
        refreshGeneratedName();
      }
      // 已导出后回到本步，按钮保持「已导出」而不是又变成可点的「开始导出」。
      if (index === 4) wizard.nextBtn.textContent = exportedPath ? "已导出" : "开始导出";
    },
    finishLabel: "开始导出",
    finishEligible: () => exportedPath === null,
    onFinish: () => runExport(),
  });
  dlg.el.classList.add("form-stacked");
  dlg.body.style.padding = "0";
  dlg.body.style.display = "flex";
  dlg.body.style.flexDirection = "column";
  dlg.body.style.overflow = "hidden";
  dlg.body.append(wizard.stepsEl, wizard.bodyEl);
  dlg.footer.prepend(...wizard.buttons());
  // 任何配置改动都让上一份导出结果失效，「开始导出」重新可用。
  dlg.body.addEventListener("input", resetExportState);
  dlg.body.addEventListener("change", resetExportState);
  // 关闭即转后台：导出已在任务中心运行，不取消，但要让用户明确知道。
  const closeDialog = dlg.close.bind(dlg);
  dlg.close = () => {
    if (running) {
      if (!window.confirm("数据字典仍在导出。关闭窗口不会中断任务，它会在后台继续，可在顶部任务中心查看进度与结果。确定关闭？")) return;
    }
    closeDialog();
  };
  accessibleDialog(dlg, "数据字典");
  fmtCards.get("markdown")?.classList.add("on");
  syncSectionsVisibility();
  if (usable.length === 0) {
    dlg.showError("请先创建一个连接");
  }
  return dlg;
}
