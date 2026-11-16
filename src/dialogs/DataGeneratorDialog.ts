/** 数据生成对话框：设置 → 生成预览 → 用户明确写入，参数化事务由后端执行。 */
import { api } from "../core/api";
import { batchExecutionServiceFor, type BatchExecutionTransport } from "../core/batchExecution";
import { BatchExecutionPanel, openBatchExecutionDialog } from "../ui/BatchExecutionPanel";
import { toolConnectionError } from "../core/toolCapabilities";
import { namespaceLabel } from "../core/namespace-label";
import { defaultGeneratorFields, generateDataPreview, generatorValueKind, MAX_GENERATED_ROWS, MAX_RECIPE_ROWS, createGeneratedRecipe, validateRecipePreview } from "../core/dataGenerator";
import type { GeneratorField, GeneratorMode, GeneratorPlan, GeneratedRecipePreview, RecipeTransactionMode } from "../core/dataGenerator";
import { kindMeta } from "../core/dbcatalog";
import type { ConnectionConfig, TableSchema } from "../core/types";
import { Dialog, field, numberInput, select, textInput } from "../ui/Dialog";
import { accessibleToolDialog, toolButton, toolTable } from "./toolDialog";
import { Wizard } from "../ui/Wizard";
import { taskCenter } from "../core/backgroundTasks";

export interface DataGeneratorApi extends Pick<typeof api, "ensureConnection" | "listDatabases" | "listTables" | "getTableSchema">, BatchExecutionTransport { previewGeneratedRecipe?: typeof api.previewGeneratedRecipe }
const defaultApi: DataGeneratorApi = api;
export function openDataGeneratorDialog(
  connection: ConnectionConfig, schema: string, initialTable = "",
  onDone?: (rows: number, table: string) => void, source: DataGeneratorApi = defaultApi,
  /** 提供时在对话框内给出「连接」下拉（只列可写的原生协议连接），可换数据源。 */
  connections: ConnectionConfig[] = [],
): Dialog {
  const service = batchExecutionServiceFor(source);
  const ownerId = crypto.randomUUID();
  let taskId: string | null = null;
  let unknown = false;
  const recordHost = document.createElement("div"); recordHost.className = "batch-generator-record"; recordHost.hidden = true;
  const returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  // 连接可切换：App 原样传入当前选中的连接；它不受支持时下拉保持空选，
  // 由用户在可写的原生协议连接中自行挑选，绝不默认选中第一个。
  let conn = connection;
  const pickable = connections.filter((c) => toolConnectionError(c, true) === null);
  const connReady = () => !connSelect || pickable.some((c) => c.id === conn.id);
  const connOptions = pickable.map((c) => ({ value: c.id, label: `${c.name} (${c.kind})` }));
  const connSelect = pickable.length
    ? select(pickable.some((c) => c.id === conn.id) ? connOptions : [{ value: "", label: "请选择连接" }, ...connOptions], pickable.some((c) => c.id === conn.id) ? conn.id : "")
    : null;
  if (connSelect) connSelect.setAttribute("aria-label", "连接");
  // 第一步标题：随所选连接变化（换连后原地更新）。
  const targetTitle = document.createElement("div");
  targetTitle.className = "wizard-panel-title";
  targetTitle.textContent = conn.name;
  const databases = select([], undefined); databases.setAttribute("aria-label", `目标${namespaceLabel(conn)}`); databases.disabled = true;
  const namespaceField = field(namespaceLabel(conn), databases);
  const tables = select([], undefined); tables.setAttribute("aria-label", "目标表"); tables.disabled = true;
  const count = numberInput(10); count.min = "1"; count.max = String(MAX_RECIPE_ROWS); count.step = "1"; count.setAttribute("aria-label", "生成条数");
  const seed = textInput("sample"); seed.maxLength = 80; seed.setAttribute("aria-label", "文本种子");
  const transactionMode = select([{ value: "atomic", label: "原子（默认）：完整单事务" }, { value: "segmented", label: "分段：每 1000 行或 8 MiB 绑定预算提交（仅 SQLite）" }], "atomic");
  transactionMode.setAttribute("aria-label", "大总量事务模式");
  const transactionModeField = field("大总量事务模式", transactionMode); transactionModeField.hidden = true;
  const segmentConfirmation = document.createElement("label"); segmentConfirmation.className = "checkbox"; segmentConfirmation.hidden = true;
  const confirmSegments = document.createElement("input"); confirmSegments.type = "checkbox";
  const segmentWarning = document.createElement("span"); segmentWarning.textContent = "我确认：已提交前缀失败/取消不能撤销（仅回滚当前段，不自动重试）";
  segmentConfirmation.append(confirmSegments, segmentWarning);
  const segmented = () => count.valueAsNumber > MAX_GENERATED_ROWS && transactionMode.value === "segmented";
  const syncTransactionMode = () => {
    transactionModeField.hidden = !(count.valueAsNumber > MAX_GENERATED_ROWS);
    transactionMode.options[1].disabled = kindMeta(conn.kind)?.protocol !== "sqlite";
    if (transactionModeField.hidden || transactionMode.options[1].disabled) transactionMode.value = "atomic";
    segmentConfirmation.hidden = !segmented();
  };
  // 内容区只保留预览；真实写入只有 Wizard 页脚一个入口，停止由共享记录面板处理。
  const previewButton = toolButton("生成预览", true);
  previewButton.disabled = true;
  const status = document.createElement("div"); status.id = `generator-status-${crypto.randomUUID()}`;
  status.setAttribute("role", "status"); status.setAttribute("aria-live", "polite"); status.style.cssText = "margin:8px 0;min-height:24px;white-space:pre-wrap;";
  count.setAttribute("aria-describedby", status.id);
  const fieldsWrap = document.createElement("div"); fieldsWrap.style.cssText = "height:210px;overflow:auto;border:1px solid var(--border);margin:8px 0;";
  const previewWrap = document.createElement("div"); previewWrap.style.cssText = "height:170px;overflow:auto;border:1px solid var(--border);";
  const confirmation = document.createElement("label"); confirmation.className = "checkbox"; confirmation.style.cssText = "display:flex;gap:8px;margin-top:10px;";
  const confirm = document.createElement("input"); confirm.type = "checkbox";
  const confirmText = document.createElement("span"); confirmText.textContent = "我确认将预览中的数据真实插入目标表（不会覆盖旧行）"; confirmation.append(confirm, confirmText);
  let nativePreview: GeneratedRecipePreview | null = null;
  let structure: TableSchema | null = null, plan: GeneratorPlan | null = null, settings: GeneratorField[] = [], version = 0, metadataVersion = 0, closed = false, writing = false;
  const fieldControls: (HTMLSelectElement | HTMLInputElement)[] = [];
  let capabilityError = toolConnectionError(conn, true);
  let writable = capabilityError === null;
  const finishEligible = () => !closed && writable && toolConnectionError(conn, true) === null && connReady() && !!plan && confirm.checked && (!segmented() || confirmSegments.checked) && !writing && !unknown;
  const invalidate = () => { nativePreview = null; plan = null; confirm.checked = false; confirmSegments.checked = false; previewWrap.replaceChildren(); wizard.refreshControls(); };
  /** 预览所依赖的配置（条数/种子/字段规则）在已有预览后发生变化：状态行提示重新生成，不自动刷新，保持「手动生成预览」交互。 */
  const noteStale = () => {
    if (closed || writing || unknown || !plan) return;
    status.setAttribute("role", "status");
    status.textContent = "配置已变化，请重新生成预览。";
  };
  const setBusy = (busy: boolean) => {
    const locked = busy || unknown;
    if (connSelect) connSelect.disabled = locked;
    databases.disabled = locked || !databases.options.length; tables.disabled = locked || !tables.options.length;
    count.disabled = locked; seed.disabled = locked; transactionMode.disabled = locked; confirmSegments.disabled = locked; previewButton.disabled = locked || !structure || !writable; confirm.disabled = locked;
    for (const control of fieldControls) control.disabled = locked;
    fieldsWrap.setAttribute("aria-busy", String(locked));
    wizard.setBusy(busy || unknown); wizard.refreshControls();
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
      modes.push({ value: "omit", label: "省略（数据库默认；必填列由后端拒绝）" });
      const mode = select(c.auto_increment ? [{ value: "omit", label: "由数据库生成" }] : modes, config.mode, 150); mode.setAttribute("aria-label", `${c.name} 生成方式`); mode.setAttribute("data-no-search", "");
      const value = textInput(config.value, 160); value.setAttribute("aria-label", `${c.name} 固定值或顺序起始值`); value.maxLength = 4096;
      const sync = () => { version++; previewButton.disabled = !structure || !writable || writing || unknown; config.mode = mode.value as GeneratorMode; config.value = value.value; value.readOnly = !["constant", "sequence"].includes(mode.value); noteStale(); invalidate(); };
      value.readOnly = !["constant", "sequence"].includes(mode.value);
      mode.addEventListener("change", sync); value.addEventListener("input", sync);
      row.insertCell().appendChild(mode); row.insertCell().appendChild(value); fieldControls.push(mode, value);
      for (const cell of row.cells) cell.style.cssText = "padding:5px 8px;border-bottom:1px solid var(--border);vertical-align:top;";
    }
    fieldsWrap.appendChild(table);
  };
  const loadTable = async () => {
    if (writing || unknown) return;
    const request = ++metadataVersion; ++version; invalidate(); structure = null; fieldsWrap.replaceChildren(); previewButton.disabled = true;
    if (!tables.value) return;
    status.textContent = "正在读取真实表结构…"; fieldsWrap.setAttribute("aria-busy", "true");
    try {
      const info = await source.getTableSchema(conn.id, schema, tables.value);
      if (closed || request !== metadataVersion) return;
      structure = info; settings = defaultGeneratorFields(info); renderFields(); previewButton.disabled = !writable;
      status.textContent = writable ? "先设置字段与条数，再生成预览。外键请填写已有引用；主键起始值须避开已存在的键。" : capabilityError ?? "当前连接不可写入数据。";
      // 状态行只挂在第 3 步；只读/协议不支持导致按钮禁用时，用向导错误行把原因亮在任何一步
      if (!writable) showError(status.textContent);
    } catch (e) { if (!closed && request === metadataVersion) { status.textContent = `读取表结构失败：${String(e)}`; status.setAttribute("role", "alert"); } }
    finally { if (!closed && request === metadataVersion) fieldsWrap.setAttribute("aria-busy", "false"); }
  };
  const wizard = new Wizard({
      steps: [
        {
          label: "目标与行数",
          hint: "选择要写入的表、生成条数与随机种子。相同种子会得到相同的随机数据。",
          render: (host) => {
            const grid = document.createElement("div");
            grid.className = "form-grid";
            if (connSelect) {
              const connectionField = field("连接:", connSelect);
              connectionField.classList.add("form-span-all");
              grid.appendChild(connectionField);
            }
            grid.append(
              namespaceField,
              field("目标表", tables),
              field(`条数（1–${MAX_RECIPE_ROWS}）`, count),
              field("文本种子", seed),
              transactionModeField,
            );
            host.append(targetTitle, grid);
            const note = document.createElement("div");
            note.className = "wizard-panel-note";
            note.textContent = "超过 500 行使用原生 recipe，仅传规则与最多 20 行样本；冻结 seed/时间/结构。仅 SQLite 原生可显式选分段：每 1000 行或 8 MiB 绑定值及字段预算先到提交（非 WAL 大小），已提交前缀失败/取消不能撤销。PG/MySQL 分段尚不支持。完整单事务可能持有长时间锁并产生大量 WAL/undo，失败或取消回滚整批。";
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
          hint: "先预览再写入。默认原子失败全回滚；显式分段只回滚当前段，已提交前缀保留。",
          render: (host) => {
            const head = document.createElement("div");
            head.className = "wizard-panel-title";
            head.textContent = "预览（最多显示 20 行，未写入数据库）";
            previewWrap.style.height = "auto";
            previewWrap.style.flex = "1";
            previewWrap.style.minHeight = "0";
            const actions = document.createElement("div");
            actions.style.cssText = "display:flex;gap:8px;margin-bottom:8px;";
            actions.append(previewButton);
            host.append(head, actions, status, previewWrap, confirmation, segmentConfirmation);
            const note = document.createElement("div");
            note.className = "wizard-panel-note";
            note.textContent = "应用生成值与 Rust 样本一致；省略列/default/identity/trigger 最终值由数据库执行时决定。约束保留；默认完整单事务失败全部回滚。显式分段每段重新校验真实结构，失败只回滚当前段；已提交前缀不能撤销，不会自动重试或覆盖旧行。";
            host.appendChild(note);
          },
          validate: () => {
            if (!plan) return "请先生成预览，再勾选确认后执行写入。";
            if (!confirm.checked) return "请勾选确认，表示你同意将预览数据真实插入目标表。";
            if (segmented() && !confirmSegments.checked) return "分段写入必须明确确认：已提交前缀失败/取消不能撤销。";
            return null;
          },
          // 这一步就是完成步：页脚按钮是「执行写入」，提交后翻到「执行记录」页。
          finish: true,
        },
        {
          label: "执行记录",
          hint: "写入按批次在事务中执行：这里显示真实阶段、扫描/执行/错误计数与逐条回执；关闭窗口不会中断已提交的写入。",
          // 记录页不再提供「下一步/完成」：收尾统一走页脚「关闭」与记录面板的动作。
          hideNext: true,
          render: (host) => {
            recordHost.hidden = false;
            recordHost.style.cssText = "display:flex;flex-direction:column;min-height:0;height:100%;";
            host.appendChild(recordHost);
          },
        },
      ],
      onEnter: (index) => {
        // 进入「字段规则」步时若结构尚未就绪，先等待加载
        if (index === 1 && !structure && tables.value) void loadTable();
      },
      finishLabel: "执行写入",
      finishEligible,
      onFinish: () => {
        // 停在「执行记录」页时，完成按钮只关闭窗口；回到配置页再次执行才提交新批次。
        if (wizard.current === 3) { dlg.close(); return; }
        if (!finishEligible() || !plan) return;
        const submitted = plan;
        const submittedRecipe = nativePreview?.recipe;
        const submittedCount = submittedRecipe?.total ?? submitted.rows.length;
        const table = tables.value;
        const targetConnection = conn;
        const targetSchema = schema;
        writing = true; unknown = false; version++;
        invalidate(); wizard.clearError();
        // 先切到执行记录页再锁按钮：Wizard.go 在 busy 时会拒绝跳步。
        showRecords();
        setBusy(true);
        const record = service.start({ connection_id: targetConnection.id, namespace: targetSchema, owner_id: ownerId,
          task: submittedRecipe ? { kind: "generated_recipe", recipe: submittedRecipe } : { kind: "generated_rows", request: { connection_id: targetConnection.id, schema: targetSchema, table,
            columns: submitted.columns, rows: submitted.rows } } }, targetConnection.name, targetConnection.kind);
        taskId = record.id; recordPanel.setTask(record.id); syncTask();
        // 后台任务中心登记：转后台后仍可在任务面板查看
        const genHandle = taskCenter.register("datagen", `数据生成 · ${table}（${submittedCount} 行）`);
        // 后台任务点击后重新打开这次执行的任务框（批执行记录），不再另弹通用任务详情。
        genHandle.setOpener(() => { void openBatchExecutionDialog(service, record.id); });
        // 完成回调不属于视图；关闭后仍在确认提交/排空时通知调用方。
        const releaseReceipt = service.subscribe(() => {
          const result = service.get(record.id);
          if (!result || result.busy) {
            genHandle.update({ message: `计划写入 ${submittedCount} 行；确认提交 ${result?.snapshot?.metrics.committed ?? 0} 行` });
            return;
          }
          releaseReceipt();
          if (!result.unknown && result.state === "succeeded" && result.snapshot?.transaction_state === "committed") {
            genHandle.finish(`写入 ${result.snapshot.affected_rows} 行`);
            try { onDone?.(result.snapshot.affected_rows, table); } catch (error) { console.error("数据生成完成回调失败", error); }
          } else if (result.unknown) {
            genHandle.cancel("结果未知：请在执行记录中确认");
          } else {
            genHandle.fail(result.notice || "数据生成失败");
          }
        });
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
  dlg.el.classList.add("form-stacked");
  const wizardButtons = wizard.buttons();
  const showRecords = () => { recordHost.hidden = false; wizard.jump(3); };
  const recordPanel = new BatchExecutionPanel(service, null, { onDetach: () => dlg.close(), onConfigure: () => {
    if (writing || unknown) return;
    wizard.go(2); wizard.refreshControls();
  } });
  recordHost.append(recordPanel.el);
  const syncTask = () => { const task = taskId ? service.get(taskId) : null; if (!task) return;
    writing = task.busy; unknown = task.unknown; if (!closed) setBusy(writing);
  };
  const releaseView = service.subscribe(syncTask);
  const review = toolButton("批执行记录"); review.addEventListener("click", () => openBatchExecutionDialog(service, taskId));
  dlg.footer.append(...wizardButtons, review, (() => {
    const close = document.createElement("button");
    close.type = "button";
    close.className = "btn";
    close.textContent = "关闭";
    close.addEventListener("click", () => dlg.close());
    return close;
  })());
  // 关闭即转后台：任务已在事务中执行，关闭窗口不取消，但要让用户明确知道。
  const closeDialog = dlg.close.bind(dlg);
  dlg.close = () => {
    if (writing || unknown) {
      const message = unknown
        ? "上一批写入的结果尚未确认。关闭窗口不会撤销已提交的数据，可在「批执行记录」中核对。确定关闭？"
        : "数据写入仍在执行。关闭窗口不会中断任务，它会在后台继续，可在「批执行记录」和顶部任务中心查看结果。确定关闭？";
      if (!window.confirm(message)) return;
    }
    closeDialog();
  };
  accessibleToolDialog(dlg, "数据生成", () => { closed = true; version++; metadataVersion++; releaseView(); recordPanel.dispose(); }, returnFocus);
  tables.addEventListener("change", () => void loadTable());
  count.addEventListener("input", () => { version++; syncTransactionMode(); noteStale(); invalidate(); previewButton.disabled = !structure || !writable || writing || unknown; });
  seed.addEventListener("input", () => { version++; noteStale(); invalidate(); previewButton.disabled = !structure || !writable || writing || unknown; });
  confirm.addEventListener("change", () => wizard.refreshControls());
  confirmSegments.addEventListener("change", () => wizard.refreshControls());
  transactionMode.addEventListener("change", () => { version++; syncTransactionMode(); noteStale(); invalidate(); previewButton.disabled = !structure || !writable || writing || unknown; });
  syncTransactionMode();
  previewButton.addEventListener("click", async () => {
    if (previewButton.disabled) return;
    // 前置校验逐条给可见提示，绝不静默返回（状态行挂在第 3 步内，此处点击必然发生在第 3 步）
    if (!writable) {
      status.setAttribute("role", "alert");
      status.textContent = capabilityError ?? "当前连接不可写入数据，无法生成预览。";
      showError(status.textContent);
      return;
    }
    if (!tables.value || !schema) {
      status.setAttribute("role", "alert");
      status.textContent = "请先选择目标数据表，再生成预览。";
      return;
    }
    if (!structure) {
      status.setAttribute("role", "alert");
      status.textContent = "表结构尚未加载完成，请先选择表并等待字段加载。";
      return;
    }
    const rows = count.valueAsNumber;
    if (!Number.isInteger(rows) || rows < 1 || rows > MAX_RECIPE_ROWS) {
      status.setAttribute("role", "alert");
      status.textContent = `生成条数必须为 1–${MAX_RECIPE_ROWS} 的整数，请修正后再生成预览。`;
      count.setAttribute("aria-invalid", "true");
      return;
    }
    count.removeAttribute("aria-invalid");
    const ready = structure; // 已通过判空，闭包内用局部常量保持类型收窄
    invalidate();
    const previewVersion = ++version;
    previewButton.disabled = true; // 短暂禁用防重复点击，完成/失败后按现状条件恢复
    try {
      if (rows > MAX_GENERATED_ROWS) {
        if (!source.previewGeneratedRecipe) throw new Error("当前后端未提供原生 recipe 样本接口；不能发送大数组作为替代");
        status.textContent = "正在原生物理会话中冻结结构并生成最多 20 行样本…";
        const recipe = createGeneratedRecipe(conn.id, schema, tables.value, settings, rows, seed.value, undefined, transactionMode.value as RecipeTransactionMode);
        const sample = validateRecipePreview(recipe, await source.previewGeneratedRecipe(recipe, ownerId));
        if (closed || previewVersion !== version) return;
        nativePreview = sample; plan = sample;
        confirmText.textContent = `我确认按冻结 recipe 在${sample.recipe.transaction_mode === "segmented" ? "显式分段事务" : "完整单事务"}中生成 ${sample.recipe.total} 行（这里只显示最多 20 行样本；不会覆盖旧行）`;
      } else { plan = generateDataPreview(ready, settings, rows, seed.value); confirmText.textContent = "我确认将预览中的数据真实插入目标表（不会覆盖旧行）"; }
      // 单行省略 + 横向滚动：预览表保持紧凑行高，不在单元格里逐字符折行
      previewWrap.appendChild(toolTable(plan.columns.length ? plan.columns : ["数据库默认值"], plan.columns.length ? plan.rows.slice(0, 20) : plan.rows.slice(0, 20).map(() => ["整行使用数据库默认"] ), { nowrap: true }));
      status.setAttribute("role", "status"); status.textContent = nativePreview ? `原生 recipe 已冻结 ${nativePreview.recipe.total} 行计划，显示 ${nativePreview.rows.length} 行样本。时间：${nativePreview.recipe.frozen_time}；结构/计划：${nativePreview.plan_hash.slice(0, 16)}。数据库执行时决定：${nativePreview.database_decided.join("、") || "无省略列（trigger 仍可能改变最终值）"}。${nativePreview.recipe.transaction_mode === "segmented" ? "请确认分段写入：已提交前缀失败/取消不能撤销，仅回滚当前段。" : "请确认完整单事务写入，失败/取消整批回滚。"}` : `预览已生成 ${plan.rows.length} 行（显示前 ${Math.min(20, plan.rows.length)} 行）。尚未写入；请勾选确认后点击“执行写入”。`;
    } catch (e) {
      if (closed || previewVersion !== version) return;
      status.setAttribute("role", "alert"); status.textContent = e instanceof Error ? e.message : String(e);
    } finally {
      if (!closed && previewVersion === version) { previewButton.disabled = !structure || !writable || writing || unknown; wizard.refreshControls(); }
    }
  });
  const loadTables = async () => {
    if (writing || unknown) return;
    const request = ++metadataVersion; ++version;
    invalidate(); structure = null; fieldsWrap.replaceChildren(); tables.replaceChildren(); tables.disabled = true; previewButton.disabled = true;
    schema = databases.value;
    if (!schema) return;
    status.textContent = "正在加载表列表…";
    try {
      const list = await source.listTables(conn.id, schema);
      if (closed || request !== metadataVersion) return;
      for (const [name, type] of list) if (type === "table") { const option = document.createElement("option"); option.value = name; option.textContent = name; tables.appendChild(option); }
      if (!tables.options.length) throw new Error("该数据库没有可写入的表（视图不支持数据生成）。");
      // 预填表名只对最初传入的连接生效，换连后不借用别的连接的表。
      if (conn.id === connection.id && initialTable && [...tables.options].some(o => o.value === initialTable)) tables.value = initialTable;
      tables.disabled = false; wizard.clearError(); await loadTable();
    } catch (e) { if (!closed && request === metadataVersion) { status.textContent = `加载表列表失败：${String(e)}`; status.setAttribute("role", "alert"); showError(status.textContent); } }
  };
  databases.addEventListener("change", () => { if (!writing && !unknown) void loadTables(); });
  /** 连接就绪后加载库/表；run 计号让换连后仍在途的旧请求作废。 */
  const initConnection = async () => {
    if (capabilityError) { status.textContent = capabilityError; showError(capabilityError); return; }
    // 条数/种子/字段规则只作废预览，不得让正在加载的连接/结构永远失去回执。
    const run = ++metadataVersion; ++version;
    status.textContent = "正在连接并读取数据库列表…";
    try {
      const connected = await source.ensureConnection(conn.id);
      if (closed || run !== metadataVersion) return;
      if (!connected.ok) throw new Error(connected.message || "连接失败");
      const list = await source.listDatabases(conn.id);
      if (closed || run !== metadataVersion) return;
      databases.replaceChildren(...list.map(([name]) => new Option(name, name)));
      if (!databases.options.length) throw new Error("此连接没有可用数据库/模式");
      if (schema && !list.some(([name]) => name === schema)) throw new Error(`目标数据库/模式「${schema}」不存在，请重新选择`);
      if (schema) databases.value = schema;
      databases.disabled = false;
      await loadTables();
    } catch (e) { if (!closed && run === metadataVersion) { databases.disabled = !databases.options.length; status.textContent = `加载数据库失败：${String(e)}`; status.setAttribute("role", "alert"); showError(status.textContent); } }
  };
  // 当前数据源不支持时保持空选：不默认选中第一个连接，也不发任何请求，
  // 等用户在「连接」下拉里自己选定后再加载。
  if (connReady()) void initConnection();
  else status.textContent = "当前连接不支持数据生成，请先在「连接」中选择可写入的连接。";
  connSelect?.addEventListener("change", () => {
    const next = pickable.find((c) => c.id === connSelect.value);
    if (writing || unknown || !next || next.id === conn.id) return;
    conn = next;
    transactionMode.value = "atomic"; confirmSegments.checked = false; syncTransactionMode();
    const nextNamespaceLabel = namespaceLabel(conn);
    databases.setAttribute("aria-label", `目标${nextNamespaceLabel}`);
    const namespaceCaption = namespaceField.querySelector<HTMLElement>(".field-label");
    if (namespaceCaption) namespaceCaption.textContent = nextNamespaceLabel;
    capabilityError = toolConnectionError(conn, true);
    writable = capabilityError === null;
    schema = ""; structure = null; plan = null; settings = [];
    confirm.checked = false; previewButton.disabled = true; wizard.refreshControls();
    previewWrap.replaceChildren(); fieldsWrap.replaceChildren();
    databases.replaceChildren(); databases.disabled = true;
    tables.replaceChildren(); tables.disabled = true;
    wizard.clearError();
    targetTitle.textContent = conn.name;
    void initConnection();
  });
  return dlg;
}
