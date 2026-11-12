/** 新建备份向导：选择备份内容 → 备份选项 → 执行。 */

import { api } from "../core/api";
import type { ConnectionConfig } from "../core/types";
import { Dialog, field, textInput } from "../ui/Dialog";
import { ObjectTree } from "../ui/ObjectTree";
import { accessibleDialog } from "./SyncDialog";
import { Wizard } from "../ui/Wizard";
import { taskCenter } from "../core/backgroundTasks";

/** 备份文件名称不允许出现的字符（Windows 保留字符）。 */
const FILE_NAME_INVALID = /[/\\:*?"<>|]/;

/** 构造复选框行。 */
function checkboxRow(labelText: string, checked: boolean): { label: HTMLLabelElement; input: HTMLInputElement } {
  const label = document.createElement("label");
  label.className = "checkbox";
  const input = document.createElement("input");
  input.type = "checkbox";
  input.checked = checked;
  input.setAttribute("aria-label", labelText);
  const text = document.createElement("span");
  text.textContent = labelText;
  label.append(input, text);
  return { label, input };
}

/** 只读展示输入框（执行步汇总）。 */
function roInput(value: string): HTMLInputElement {
  const input = textInput(value);
  input.readOnly = true;
  return input;
}

/** 打开「新建备份」向导。
 *  onDone(rows, bytes) 在备份成功后回调（用于刷新备份列表与提示）。 */
export function openBackupDialog(
  connection: ConnectionConfig,
  schema: string,
  onDone: (rows: number, bytes: number) => void,
): Dialog {
  let wizard!: Wizard;
  let running = false;
  let loaded = false;

  // 对象树：分组根行常显、可折叠、可全选；叶子默认全选（备份默认包含全部对象）。
  const tree = new ObjectTree({ ariaLabel: "选择备份对象" });
  const tablesGroup = tree.addGroup({ title: "表", icon: "info_pane_table" });
  const viewsGroup = tree.addGroup({ title: "视图", icon: "info_pane_view" });
  const routinesGroup = tree.addGroup({ title: "函数与存储过程", icon: "info_pane_function" });

  // 执行步状态行
  const status = document.createElement("div");
  status.style.marginTop = "6px";
  status.style.minHeight = "18px";
  status.style.color = "var(--fg-muted)";

  // 步1 对象列表加载状态行（加载中 / 失败提示）
  const loadStatus = document.createElement("div");
  loadStatus.style.marginTop = "6px";
  loadStatus.style.color = "var(--fg-muted)";

  /** 加载表 / 视图 / 例程清单（只加载一次，失败可重试）。 */
  async function loadObjects(): Promise<void> {
    if (loaded) return;
    loaded = true;
    loadStatus.textContent = "正在加载对象列表...";
    try {
      const [tables, routines] = await Promise.all([
        api.listTables(connection.id, schema),
        api.listRoutines(connection.id, schema),
      ]);
      for (const [name, ty] of tables) {
        if (ty === "view") tree.addItem(viewsGroup, { name, icon: "info_pane_view", checked: true });
        else tree.addItem(tablesGroup, { name, icon: "info_pane_table", checked: true });
      }
      // 函数与存储过程：同一分组内按类型分行显示
      const caption = (text: string): HTMLElement => {
        const cap = document.createElement("div");
        cap.style.fontSize = "var(--fs-11)";
        cap.style.color = "var(--text-tertiary)";
        cap.style.margin = "2px 0";
        cap.textContent = text;
        return cap;
      };
      const functions = routines.filter((r) => r[0] === "function");
      const procedures = routines.filter((r) => r[0] === "procedure");
      if (functions.length) {
        routinesGroup.children.insertBefore(caption("函数"), routinesGroup.empty);
        for (const [, name] of functions) tree.addItem(routinesGroup, { name, icon: "info_pane_function", checked: true });
      }
      if (procedures.length) {
        routinesGroup.children.insertBefore(caption("存储过程"), routinesGroup.empty);
        for (const [, name] of procedures) tree.addItem(routinesGroup, { name, icon: "info_pane_function", checked: true });
      }
      // 分组根节点始终显示（没有对象时显示「（无）」），不在空组上折叠整棵树
      loadStatus.textContent = tables.length || routines.length
        ? ""
        : "（该数据库没有可备份的表、视图、函数或存储过程）";
    } catch (e) {
      loaded = false;
      loadStatus.textContent = `对象列表加载失败：${String(e)}`;
      dlg.showError(String(e));
    }
  }

  // 第 1 步：选择备份内容
  const p1 = document.createElement("div");
  p1.style.cssText = "display:flex;flex-direction:column;gap:6px;height:100%;min-height:0;";
  const note = document.createElement("div");
  note.style.color = "var(--fg-muted)";
  note.style.marginBottom = "4px";
  note.textContent = "不勾选任何对象 = 备份全部表与视图";
  p1.append(note, tree.el, loadStatus);

  // 第 2 步：备份选项
  const incStructure = checkboxRow("包含结构", true);
  const incData = checkboxRow("包含数据", true);
  const commentInput = textInput();
  const fileNameInput = textInput();
  fileNameInput.placeholder = "留空则按时间自动命名，之后可在备份列表右键重命名";
  const p2 = document.createElement("div");
  p2.append(
    incStructure.label,
    incData.label,
    field("注释:", commentInput),
    field("备份文件名称:", fileNameInput),
  );

  /** 选项校验：文件名禁用字符 + 结构 / 数据至少一项。 */
  function optionsProblem(): string | null {
    const name = fileNameInput.value.trim();
    if (FILE_NAME_INVALID.test(name)) return "备份文件名称不能包含 / \\ : * ? \" < > | 字符";
    if (!incStructure.input.checked && !incData.input.checked) return "「包含结构」与「包含数据」至少选择一项";
    return null;
  }

  // 第 3 步：执行（只读汇总 + 状态行）
  const sumObjects = roInput("");
  const sumContent = roInput("");
  const sumFile = roInput("");
  const p3 = document.createElement("div");
  p3.append(
    field("备份对象:", sumObjects),
    field("包含内容:", sumContent),
    field("备份文件:", sumFile),
    status,
  );

  /** 进入执行步时刷新只读汇总。 */
  function refreshSummary(): void {
    const t = tablesGroup.items.filter((it) => it.input.checked).length;
    const v = viewsGroup.items.filter((it) => it.input.checked).length;
    const r = routinesGroup.items.filter((it) => it.input.checked).length;
    sumObjects.value = t + v + r === 0
      ? "未勾选对象（备份全部表与视图）"
      : `表 ${t}、视图 ${v}、函数与存储过程 ${r}`;
    const s = incStructure.input.checked;
    const d = incData.input.checked;
    sumContent.value = s && d ? "结构 + 数据" : s ? "仅结构" : "仅数据";
    const name = fileNameInput.value.trim();
    sumFile.value = name || "按时间自动命名";
  }

  /** 执行备份：全不选 = 空数组表示全部表与视图；例程默认空数组。 */
  const runBackup = async (): Promise<void> => {
    if (running) return;
    const problem = optionsProblem();
    if (problem) {
      dlg.showError(problem);
      return;
    }
    running = true;
    wizard.setBusy(true);
    status.textContent = "正在备份，请稍候...";
    const tables = [...tablesGroup.items, ...viewsGroup.items]
      .filter((it) => it.input.checked)
      .map((it) => it.name);
    const routines = routinesGroup.items.filter((it) => it.input.checked).map((it) => it.name);
    // 后台任务中心登记（关闭对话框不影响执行）
    const handle = taskCenter.register("backup", `备份 · ${schema}`);
    try {
      const [rows, bytes] = await api.createBackup({
        connection_id: connection.id,
        schema,
        comment: commentInput.value.trim(),
        tables,
        routines,
        file_name: fileNameInput.value.trim() || null,
        include_structure: incStructure.input.checked,
        include_data: incData.input.checked,
      });
      handle.finish(`备份 ${rows} 行数据（${(bytes / 1024).toFixed(1)} KB）`);
      status.textContent = `备份完成：${rows} 行数据，文件 ${(bytes / 1024).toFixed(1)} KB。`;
      onDone(rows, bytes);
    } catch (e) {
      handle.fail(String(e));
      status.textContent = `备份失败：${String(e)}`;
      dlg.showError(String(e));
    } finally {
      running = false;
      wizard.setBusy(false);
    }
  };

  // 固定窗口尺寸：对象清单长短不一会让对话框在切步时跳动。
  const dlg = new Dialog({ title: "新建备份", width: 640, height: 620, render: () => {}, buttons: [{ label: "关闭" }] });
  wizard = new Wizard({
    steps: [
      { label: "选择备份内容", hint: "勾选要备份的表、视图、函数与存储过程。", render: host => host.appendChild(p1) },
      { label: "备份选项", hint: "设置包含内容、注释与备份文件名称。", render: host => host.appendChild(p2), validate: optionsProblem },
      { label: "执行", hint: "核对配置后开始备份。", render: host => host.appendChild(p3), validate: optionsProblem },
    ],
    onEnter: index => {
      if (index === 0) void loadObjects();
      if (index === 2) refreshSummary();
    },
    finishLabel: "开始备份",
    onFinish: async () => { await runBackup(); },
  });
  // 与导入 / 导出向导一致的分步布局
  dlg.el.classList.add("form-stacked");
  dlg.body.style.padding = "0";
  dlg.body.style.display = "flex";
  dlg.body.style.flexDirection = "column";
  dlg.body.style.overflow = "hidden";
  dlg.body.append(wizard.stepsEl, wizard.bodyEl);
  dlg.footer.prepend(...wizard.buttons());
  dlg.body.querySelector<HTMLInputElement>("input")?.focus();
  accessibleDialog(dlg, "新建备份");
  return dlg;
}
