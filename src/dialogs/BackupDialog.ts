/** 新建备份向导：选择备份内容 → 备份选项 → 执行。 */

import { api } from "../core/api";
import type { BackupOptions, ConnectionConfig } from "../core/types";
import { Dialog, field, select, textInput } from "../ui/Dialog";
import { ObjectTree } from "../ui/ObjectTree";
import { accessibleDialog } from "./SyncDialog";
import { Wizard } from "../ui/Wizard";
import { taskCenter } from "../core/backgroundTasks";
import "../styles/backup-dialog.css";

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
  let completed = false;
  let loaded = false;
  let loading = false;
  let loadError = "";
  let closed = false;

  // 对象树：分组根行常显、可折叠、可全选；叶子默认全选（备份默认包含全部对象）。
  const tree = new ObjectTree({ ariaLabel: "选择备份对象" });
  const tablesGroup = tree.addGroup({ title: "表", icon: "info_pane_table" });
  const viewsGroup = tree.addGroup({ title: "视图", icon: "info_pane_view" });
  const routinesGroup = tree.addGroup({ title: "函数与存储过程", icon: "info_pane_function" });

  // 执行步状态：完成后填满原状态区域，而非一行易被忽略的小字。
  const status = document.createElement("div");
  status.className = "backup-status";
  status.setAttribute("role", "status");
  status.setAttribute("aria-live", "polite");
  status.setAttribute("aria-atomic", "true");

  // 步1 对象列表加载状态行（加载中 / 失败提示）
  const loadStatus = document.createElement("div");
  loadStatus.style.marginTop = "6px";
  loadStatus.style.color = "var(--fg-muted)";

  /** 加载表 / 视图 / 例程清单（只加载一次，失败可重试）。 */
  async function loadObjects(): Promise<void> {
    if (loaded || loading || closed) return;
    loading = true; loadError = "";
    loadStatus.textContent = "正在加载对象列表...";
    try {
      const [tables, routines] = await Promise.all([
        api.listTables(connection.id, schema),
        api.listRoutines(connection.id, schema),
      ]);
      if (closed) return;
      loaded = true;
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
      tree.filter(filterInput.value);
      // 分组根节点始终显示（没有对象时显示「（无）」），不在空组上折叠整棵树
      loadStatus.textContent = tables.length || routines.length
        ? ""
        : "（该数据库没有可备份的表、视图、函数或存储过程）";
    } catch (e) {
      if (closed) return;
      loaded = false; loadError = String(e);
      loadStatus.textContent = `对象列表加载失败：${loadError}；请点击重试，成功前不能备份。`;
      dlg.showError(loadError);
    } finally { loading = false; }
  }
  function objectsProblem(): string | null {
    if (loading || !loaded && !loadError) return "对象列表仍在加载，请等待完成后核对备份范围。";
    if (!loaded) return `对象列表加载失败，请先重试，不能备份：${loadError}`;
    // 真正空库也不把空选择当作‘全库’请求提交：没有可备份对象时明确停止。
    if (!tablesGroup.items.length && !viewsGroup.items.length && !routinesGroup.items.length) return "当前数据库没有可备份对象，不能创建空备份。";
    return null;
  }

  // 第 1 步：选择备份内容
  const p1 = document.createElement("div");
  p1.style.cssText = "display:flex;flex-direction:column;gap:6px;height:100%;min-height:0;";
  const note = document.createElement("div");
  note.style.color = "var(--fg-muted)";
  note.style.marginBottom = "4px";
  note.textContent = "不勾选任何对象 = 备份全部表与视图";
  // 本地过滤只隐藏不匹配的行，不改变任何勾选状态，也不改变分组全选的含义。
  const filterInput = textInput();
  filterInput.type = "search";
  filterInput.placeholder = "搜索表 / 视图 / 函数与存储过程";
  filterInput.setAttribute("aria-label", "搜索备份对象");
  filterInput.addEventListener("input", () => tree.filter(filterInput.value));
  const retryObjects = document.createElement("button");
  retryObjects.type = "button"; retryObjects.className = "btn"; retryObjects.textContent = "重试加载对象";
  retryObjects.addEventListener("click", () => void loadObjects());
  p1.append(note, field("搜索:", filterInput), tree.el, loadStatus, retryObjects);

  // 第 2 步：备份选项
  const incStructure = checkboxRow("包含结构", true);
  const incData = checkboxRow("包含数据", true);
  const skipIncompatible = checkboxRow("跳过无法无损读取的表（会逐表告警）", false);
  const skipNote = document.createElement("div");
  skipNote.style.cssText = "font-size:11px;color:var(--text-tertiary);margin-left:22px;";
  skipNote.textContent = "默认严格失败；开启后整表跳过并写入备份警告，不会静默漏列。";
  const consistency = select([
    { value: "compat", label: "兼容逻辑转储（默认）" },
    ...(connection.kind === "postgresql" ? [{ value: "pg_snapshot", label: "显式：PostgreSQL 任务级只读快照" }] : []),
    ...(connection.kind === "mysql" ? [{ value: "mysql_locked", label: "显式：MySQL 源读锁（可能阻塞关联表读写）" }] : []),
  ], "compat");
  consistency.setAttribute("aria-label", "源一致性");
  const consistencyNote = document.createElement("div");
  consistencyNote.id = `backup-consistency-${Math.random().toString(36).slice(2)}`;
  consistency.setAttribute("aria-describedby", consistencyNote.id);
  consistencyNote.style.cssText = "font-size:11px;color:var(--text-tertiary);";
  const refreshConsistencyNote = (): void => {
    consistencyNote.textContent = consistency.value === "mysql_locked"
      ? "仅已识别原生 MySQL 8.0/8.4 的普通 InnoDB 表；一次 LOCK TABLES READ 贯穿任务，需要 SELECT 与 LOCK TABLES 权限，会阻塞源写入/DDL；MySQL 可能隐式锁未选关联表并升级 WRITE、阻塞其读写。已知 trigger/FK 依赖会被拒绝，目录权限或并发DDL不足以保证锁范围仅限所选表。不支持视图、例程或兼容协议；生成列暂不支持数据写出（可选仅结构）。能力不满足直接失败，不使用全局 FLUSH。"
      : consistency.value === "pg_snapshot"
        ? "同一只读 REPEATABLE READ 事务覆盖所选普通表，ACCESS SHARE 锁可能阻塞 DDL，长快照会增加维护成本。视图、例程、分区、RLS、自定义类型、生成/identity 列等拒绝；显式结构模式要求 PG14+，不能保留的约束/索引/default/非默认 STORAGE/COMPRESSION/表访问方法等拒绝（可选仅数据）。"
        : connection.kind === "sqlite"
          ? "逻辑 DDL/数据转储；SQLite 文件库 main 使用已支持的同一只读快照，其他路径保持原行为。不是物理备份或 PITR。"
          : "保持原逻辑 DDL/数据转储路径，不承诺跨表同一快照；不是全实例、物理备份或 PITR。";
  };
  consistency.addEventListener("change", refreshConsistencyNote);
  refreshConsistencyNote();
  const commentInput = textInput();
  const fileNameInput = textInput();
  fileNameInput.placeholder = "留空则按时间自动命名，之后可在备份列表右键重命名";
  const p2 = document.createElement("div");
  p2.append(
    incStructure.label,
    incData.label,
    skipIncompatible.label,
    skipNote,
    field("源一致性:", consistency),
    consistencyNote,
    field("注释:", commentInput),
    field("备份文件名称:", fileNameInput),
  );

  /** 选项校验：文件名禁用字符 + 结构 / 数据至少一项。 */
  function optionsProblem(): string | null {
    const name = fileNameInput.value.trim();
    if (FILE_NAME_INVALID.test(name)) return "备份文件名称不能包含 / \\ : * ? \" < > | 字符";
    if (!incStructure.input.checked && !incData.input.checked) return "「包含结构」与「包含数据」至少选择一项";
    if (consistency.value !== "compat" && (viewsGroup.items.some(it => it.input.checked) || routinesGroup.items.some(it => it.input.checked))) {
      return "显式一致性模式尚不能证明视图/例程依赖，请取消勾选或使用兼容模式";
    }
    return null;
  }

  // 第 3 步：执行（只读汇总 + 成功结果）
  const sumObjects = roInput("");
  const sumContent = roInput("");
  const sumFile = roInput("");
  const sumConsistency = roInput("");
  const p3 = document.createElement("div");
  p3.className = "backup-execution";
  p3.append(
    field("备份对象:", sumObjects),
    field("包含内容:", sumContent),
    field("备份文件:", sumFile),
    field("源一致性:", sumConsistency),
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
    sumConsistency.value = consistency.selectedOptions[0]?.textContent || "兼容逻辑转储";
  }

  /** 成功结果使用本次命令回执的实际路径，不推测文件名或查找列表中的最新文件。 */
  function showSuccess(rows: number, bytes: number, savedPath: string, warnings: string[], restoreSubmitFocus: boolean): void {
    status.classList.add("is-success");
    const icon = document.createElement("div");
    icon.className = "backup-success-icon";
    icon.setAttribute("aria-hidden", "true");
    icon.innerHTML = '<svg viewBox="0 0 32 32"><path d="m8 16 5 5 11-11" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"/></svg>';
    const title = document.createElement("div");
    title.className = "backup-success-title";
    title.textContent = "备份成功";
    const result = document.createElement("div");
    result.className = "backup-success-result";
    result.textContent = `已备份 ${rows} 行数据，文件大小 ${(bytes / 1024).toFixed(1)} KB`;
    const location = document.createElement("div");
    location.className = "backup-success-location";
    location.textContent = `存储位置：${savedPath}`;
    location.title = savedPath;
    const hint = document.createElement("div");
    hint.className = "backup-success-hint";
    hint.textContent = warnings.length
      ? `备份文件已保存；${warnings.length} 张表被跳过，见下方警告`
      : "备份文件已保存，可关闭此窗口";
    const children: HTMLElement[] = [icon, title, result, location, hint];
    if (warnings.length) {
      const warning = document.createElement("div");
      warning.className = "backup-success-warning";
      warning.setAttribute("role", "alert");
      warning.textContent = warnings.join("\n");
      warning.style.cssText = "white-space:pre-wrap;color:var(--warn-600);max-height:120px;overflow:auto;";
      children.push(warning);
    }
    status.replaceChildren(...children);
    wizard.markComplete("备份已完成", "备份任务已完成。");
    closeBtn.classList.add("primary");
    // 提交按钮已禁用，焦点转到关闭；不抢走用户在后台任务期间移走的焦点。
    if (dlg.el.isConnected && (document.activeElement === wizard.nextBtn ||
      (restoreSubmitFocus && document.activeElement === document.body))) closeBtn.focus();
  }

  /** 执行备份：全不选 = 空数组表示全部表与视图；例程默认空数组。 */
  const runBackup = async (): Promise<void> => {
    if (closed || running || completed) return;
    const problem = objectsProblem() ?? optionsProblem();
    if (problem) {
      dlg.showError(problem);
      return;
    }
    const restoreSubmitFocus = document.activeElement === wizard.nextBtn;
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
      const backup = await api.createBackup({
        connection_id: connection.id,
        schema,
        comment: commentInput.value.trim(),
        tables,
        routines,
        file_name: fileNameInput.value.trim() || null,
        include_structure: incStructure.input.checked,
        include_data: incData.input.checked,
        skip_incompatible_tables: skipIncompatible.input.checked,
        source_consistency: consistency.value as BackupOptions["source_consistency"],
      });
      handle.finish(`备份 ${backup.rows} 行数据（${(backup.bytes / 1024).toFixed(1)} KB）`);
      completed = true;
      showSuccess(backup.rows, backup.bytes, backup.path, backup.warnings, restoreSubmitFocus);
      onDone(backup.rows, backup.bytes);
    } catch (e) {
      if (completed) {
        // 备份已落盘，列表刷新回调失败不能把成功结果改成失败或重新开放提交。
        dlg.showError(`备份已保存，但刷新备份列表失败：${String(e)}`);
        return;
      }
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
  const closeBtn = dlg.footer.querySelector<HTMLButtonElement>("button")!;
  wizard = new Wizard({
    steps: [
      { label: "选择备份内容", hint: "勾选要备份的表、视图、函数与存储过程。", render: host => host.appendChild(p1), validate: objectsProblem },
      { label: "备份选项", hint: "设置包含内容、注释与备份文件名称。", render: host => host.appendChild(p2), validate: optionsProblem },
      { label: "执行", hint: "核对配置后开始备份。", render: host => host.appendChild(p3), validate: optionsProblem },
    ],
    onEnter: index => {
      if (completed && index !== 2) {
        completed = false;
        status.classList.remove("is-success");
        status.replaceChildren();
        closeBtn.classList.remove("primary");
      }
      if (index === 0) void loadObjects();
      if (index === 2) refreshSummary();
    },
    finishLabel: "开始备份",
    finishEligible: () => !completed,
    onFinish: async () => { await runBackup(); },
  });
  // 与导入 / 导出向导一致的分步布局
  dlg.el.classList.add("form-stacked", "backup-dialog");
  dlg.body.style.padding = "0";
  dlg.body.style.display = "flex";
  dlg.body.style.flexDirection = "column";
  dlg.body.style.overflow = "hidden";
  dlg.body.append(wizard.stepsEl, wizard.bodyEl);
  dlg.footer.prepend(...wizard.buttons());
  dlg.body.querySelector<HTMLInputElement>("input")?.focus();
  accessibleDialog(dlg, "新建备份");
  dlg.onClose(() => { closed = true; });
  return dlg;
}
