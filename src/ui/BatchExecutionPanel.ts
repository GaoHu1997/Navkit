/** SQL 文件与生成数据共用的真实执行记录。关闭面板只解除订阅，任务继续由 service 管理。 */
import { batchExecutions, batchStateLabel, type BatchExecutionService, type BatchExecutionRecord } from "../core/batchExecution";
import { isExecutionTerminal } from "../core/execution";
import { Dialog, field } from "./Dialog";
import type { BatchFileEncoding } from "../core/api";
import type { ConnectionConfig } from "../core/types";
import { accessibleToolDialog, toolButton } from "../dialogs/toolDialog";
import { taskCenter } from "../core/backgroundTasks";
import "../styles/batch-execution.css";
import "../styles/execution-log.css";
import { ExecutionProgress, type ProgressInput, type ProgressPhase } from "./ExecutionProgress";
import { ExecutionLogList, type LogFilter } from "./ExecutionLogList";

export interface BatchExecutionPanelOptions { onDetach?: () => void; onConfigure?: () => void }
const transactions = { none: "无整体事务", active: "事务进行中", committed: "提交已确认", rolled_back: "回滚已确认", unknown: "事务结果未知" };
const kindLabels: Record<string, string> = { generated_rows: "生成写入", generated_recipe: "原生 recipe", sql_file: "SQL 文件" };

export class BatchExecutionPanel {
  readonly el = document.createElement("section");
  private readonly picker = document.createElement("select");
  private readonly scope = document.createElement("div");
  private readonly state = document.createElement("div");
  private readonly metrics: HTMLElement[] = [];
  private readonly progress = new ExecutionProgress();
  private readonly list: ExecutionLogList;
  private readonly receipt = document.createElement("div");
  private readonly note = document.createElement("div");
  private readonly truncated = document.createElement("div");
  private readonly tabs: { filter: LogFilter; button: HTMLButtonElement; badge: HTMLElement }[] = [];
  private readonly pause: HTMLButtonElement = toolButton("暂停");
  private readonly copy: HTMLButtonElement = toolButton("复制筛选结果");
  private readonly stop = toolButton("停止");
  private readonly detach = toolButton("转入后台");
  private readonly configure = toolButton("回到配置");
  private readonly follow = document.createElement("input");
  private readonly expandSql = document.createElement("input");
  private readonly unsubscribe: () => void;
  private id: string | null;
  private current: BatchExecutionRecord | null = null;
  private pickerSignature = "";
  private shownTaskId: string | null = null;

  constructor(private readonly service: BatchExecutionService = batchExecutions, id: string | null = null, options: BatchExecutionPanelOptions = {}) {
    this.id = id;
    this.el.className = "batch-execution-panel"; this.el.setAttribute("aria-label", "批任务执行记录");
    const pickerRow = document.createElement("label"); pickerRow.className = "batch-task-picker";
    const pickerLabel = document.createElement("span"); pickerLabel.textContent = "回看任务";
    this.picker.setAttribute("aria-label", "回看批执行任务"); pickerRow.append(pickerLabel, this.picker);
    this.picker.addEventListener("change", () => { this.id = this.picker.value || null; this.render(); });
    this.scope.className = "batch-execution-scope";
    this.state.className = "batch-execution-state"; this.state.setAttribute("role", "status"); this.state.setAttribute("aria-live", "polite"); this.state.setAttribute("aria-atomic", "true");
    const counts = document.createElement("dl"); counts.className = "batch-execution-metrics";
    for (const [name, description] of [["扫描", "后端已扫描到的完整可执行语句；不是字符或分号计数"], ["执行", "已收到回执的执行尝试，包含失败尝试"], ["错误", "后端执行错误计数；解析错误另外展示"], ["耗时", "后端真实累计耗时"]]) {
      const metric = document.createElement("div"), label = document.createElement("dt"), value = document.createElement("dd");
      label.textContent = name; label.title = description; value.textContent = "—"; metric.append(label, value); counts.append(metric); this.metrics.push(value);
    }
    this.receipt.className = "batch-execution-receipt";
    this.note.className = "batch-execution-note";
    const logHeading = document.createElement("div"); logHeading.className = "batch-execution-log-heading";
    const title = document.createElement("strong"); title.textContent = "执行日志";
    const headingTools = document.createElement("div"); headingTools.className = "batch-execution-log-tools";
    const followLabel = document.createElement("label"); followLabel.className = "checkbox";
    this.follow.type = "checkbox"; this.follow.checked = true;
    const followText = document.createElement("span"); followText.textContent = "跟随最新";
    followLabel.append(this.follow, followText);
    const expandLabel = document.createElement("label"); expandLabel.className = "checkbox";
    this.expandSql.type = "checkbox"; this.expandSql.checked = true;
    const expandText = document.createElement("span"); expandText.textContent = "展开 SQL";
    expandLabel.append(this.expandSql, expandText);
    headingTools.append(followLabel, expandLabel, this.pause, this.copy);
    logHeading.append(title, headingTools);
    const filters = document.createElement("div"); filters.className = "batch-execution-filters"; filters.setAttribute("role", "tablist"); filters.setAttribute("aria-label", "按结果过滤执行记录");
    for (const [filter, label] of [["all", "全部"], ["error", "错误"], ["slow", "慢查询"]] as const) {
      const button = document.createElement("button"); button.type = "button"; button.setAttribute("role", "tab");
      button.setAttribute("aria-selected", String(filter === "all"));
      const text = document.createElement("span"); text.textContent = label;
      const badge = document.createElement("b"); badge.textContent = "0";
      button.append(text, badge);
      button.addEventListener("click", () => this.selectFilter(filter));
      filters.append(button); this.tabs.push({ filter, button, badge });
    }
    this.list = new ExecutionLogList({
      context: () => ({
        connectionName: this.current?.connectionName ?? "",
        database: this.current?.namespace ?? "",
        driver: this.current?.driver ?? "",
        sessionId: this.current?.context?.execution_id.slice(0, 6) ?? "",
      }),
      onFollowChange: follow => { this.follow.checked = follow; },
      onCountChange: () => this.refreshCounts(),
    });
    this.follow.addEventListener("change", () => this.list.setFollow(this.follow.checked));
    this.expandSql.addEventListener("change", () => this.list.setAutoExpand(this.expandSql.checked));
    this.pause.addEventListener("click", () => {
      const paused = this.list.pausedValue;
      this.list.setPaused(!paused);
      this.pause.textContent = paused ? "暂停" : "继续";
      this.pause.setAttribute("aria-pressed", String(!paused));
    });
    this.copy.addEventListener("click", () => {
      const text = this.list.toText();
      if (!text || !navigator.clipboard) return;
      void navigator.clipboard.writeText(text).catch(() => undefined);
    });
    this.truncated.className = "batch-execution-truncated";
    const actions = document.createElement("div"); actions.className = "batch-execution-actions";
    this.stop.addEventListener("click", () => { if (this.id) void this.service.cancel(this.id); });
    this.detach.hidden = !options.onDetach; this.detach.addEventListener("click", () => options.onDetach?.());
    this.configure.hidden = !options.onConfigure; this.configure.addEventListener("click", () => options.onConfigure?.());
    actions.append(this.stop, this.detach, this.configure);
    const topRow = document.createElement("div"); topRow.className = "batch-execution-top";
    topRow.append(pickerRow, actions);
    this.el.append(topRow, this.scope, this.state, this.progress.el, counts, this.receipt, this.note, logHeading, filters, this.list.el, this.truncated);
    this.unsubscribe = this.service.subscribe(() => this.render()); this.render();
  }
  setTask(id: string): void { this.id = id; this.render(); }

  private selectFilter(filter: LogFilter): void {
    for (const tab of this.tabs) tab.button.setAttribute("aria-selected", String(tab.filter === filter));
    this.list.setFilter(filter);
    this.refreshCounts();
  }
  private refreshCounts(): void {
    for (const tab of this.tabs) {
      const value = this.list.count(tab.filter);
      tab.badge.textContent = String(value);
      tab.badge.dataset.tone = tab.filter === "error" && value > 0 ? "danger" : tab.filter === "slow" && value > 0 ? "warn" : "";
    }
  }

  /** 状态 → 进度条阶段；扫描未完成且总数未知时保持不确定态。 */
  private progressInput(record: BatchExecutionRecord | null, snapshot: BatchExecutionRecord["snapshot"] | undefined): ProgressInput {
    const metrics = snapshot?.metrics;
    const scanned = metrics?.scanned ?? 0;
    const executed = metrics?.executed ?? 0;
    const elapsedMs = metrics?.elapsed_ms ?? 0;
    const knownTotal = record?.totalRows ?? snapshot?.total_rows ?? (snapshot?.scan_complete ? scanned : null);
    const state = record?.state ?? "queued";
    let phase: ProgressPhase;
    if (!record || state === "queued") phase = "indeterminate";
    else if (state === "cancel_requested") phase = "stopping";
    else if (state === "succeeded") phase = "done";
    else if (state === "cancelled") phase = "stopped";
    else if (state === "failed" || state === "timed_out") phase = "failed";
    else if (state === "unknown") phase = "unknown";
    else phase = snapshot && !snapshot.scan_complete && knownTotal === null ? "indeterminate" : "running";
    const unit = record?.kind !== "sql_file" ? "行" : "条语句";
    return { phase, scanned, executed, total: knownTotal, elapsedMs, unit };
  }

  private render(): void {
    const all = this.service.list();
    if (!this.id || !this.service.get(this.id)) this.id = all[0]?.id ?? null;
    const signature = all.map(record => record.id + ":" + record.state + ":" + record.busy).join("|");
    if (signature !== this.pickerSignature) {
      this.pickerSignature = signature;
      this.picker.replaceChildren(...all.map(record => {
        const option = document.createElement("option"); option.value = record.id;
        option.textContent = (kindLabels[record.kind] ?? "批任务") + " · " + record.connectionName + " / " + (record.namespace || "默认") + " · " + batchStateLabel(record.state) + " · " + (record.context?.execution_id.slice(0, 8) ?? "登记中");
        return option;
      }));
    }
    this.picker.value = this.id ?? ""; this.picker.disabled = all.length === 0;
    const record = this.id ? this.service.get(this.id) : null;
    this.current = record;
    this.stop.disabled = !record?.busy || record.cancelRequested;
    this.configure.disabled = !record || record.busy || record.unknown;
    this.detach.textContent = record?.busy ? "转入后台" : "关闭记录";
    this.el.dataset.state = record?.state ?? "empty";
    this.scope.textContent = record ? (record.summary || (kindLabels[record.kind] ?? "批任务")) + " · 固定目标：" + record.connectionName + " / " + (record.namespace || "（默认）") : "尚无批执行任务。执行记录仅回看原任务，不会再次运行。";
    const snapshot = record?.snapshot;
    const stateText = record ? batchStateLabel(record.state) + (record.busy && isExecutionTerminal(record.state) ? " · 等待资源排空" : "") + (snapshot && snapshot.metrics.errors > 0 ? " · " + snapshot.metrics.errors + " 条执行错误" : "") + (record.cancelRequested && record.busy ? " · 已请求停止" : "") : "等待登记任务";
    if (this.state.textContent !== stateText) this.state.textContent = stateText;
    this.metrics[0].textContent = snapshot ? String(snapshot.metrics.scanned) : "—";
    this.metrics[1].textContent = snapshot ? String(snapshot.metrics.executed) : "—";
    this.metrics[2].textContent = snapshot ? String(snapshot.metrics.errors) : "—";
    this.metrics[3].textContent = snapshot ? (snapshot.metrics.elapsed_ms / 1000).toFixed(1) + " s" : "—";
    const committedPrefix = record?.kind === "generated_recipe" && (snapshot?.metrics.committed ?? 0) > 0 && record.state !== "succeeded";
    const transactionLabel = snapshot ? committedPrefix && snapshot.transaction_state === "rolled_back" ? "当前段回滚已确认（已提交前缀保留）" : transactions[snapshot.transaction_state] : "";
    this.receipt.textContent = snapshot ? transactionLabel + (record?.kind === "generated_recipe" ? " · 已生成 " + (snapshot.metrics.generated ?? snapshot.metrics.scanned) + " / 已发送 " + (snapshot.metrics.sent ?? "—") + " / 回执 " + (snapshot.metrics.acknowledged ?? snapshot.metrics.executed) + " / 确认提交 " + (snapshot.metrics.committed ?? "—") + " 行" : "") + " · 后端累计影响 " + snapshot.affected_rows + " 行" + (record?.totalRows !== null && record?.totalRows !== undefined ? " · 批次 " + record.totalRows + " 行" : "") + (record?.sourceBytes !== null && record?.sourceBytes !== undefined ? " · 源文件 " + record.sourceBytes + " 字节" : "") : "尚未收到执行/事务回执。";
    const scanNote = snapshot && !snapshot.scan_complete ? (isExecutionTerminal(snapshot.status.state) ? "扫描未完成；未扫描尾部没有执行，不计为成功。" : "扫描进行中；未扫描尾部尚未执行。") : "";
    this.note.textContent = [record?.notice, committedPrefix ? `已有 ${snapshot!.metrics.committed} 行确认提交前缀；失败/取消不能撤销，未知段禁止重放。` : "", snapshot?.parse_error && !record?.notice.includes(snapshot.parse_error) ? "SQL 文件解析失败：" + snapshot.parse_error : "", scanNote].filter(Boolean).join(" ");
    this.progress.el.hidden = !record;
    if (record) {
      this.progress.setInput(this.progressInput(record, snapshot));
      if (this.shownTaskId !== record.id) { this.list.clear(); this.shownTaskId = record.id; }
      this.list.append(record.logs);
    } else { this.list.clear(); this.shownTaskId = null; }
    this.truncated.textContent = this.truncation(record);
    this.refreshCounts();
  }

  private truncation(record: BatchExecutionRecord | null): string {
    const notes = ["保留近期执行记录，直接显示数据库错误原文、错误码与耗时；点击记录可展开完整详情。"];
    if (record && record.droppedBeforeSequence > 0) notes.push("后端序号 " + record.droppedBeforeSequence + " 及之前的日志已截断。");
    if (record?.locallyDropped) notes.push("本地已截断 " + record.locallyDropped + " 条前序记录。");
    return notes.join(" ");
  }
  /** 仅释放视图订阅，不取消/处置 service，也不抹除在途 context。 */
  dispose(): void { this.unsubscribe(); }
  
}

export function openBatchExecutionDialog(service: BatchExecutionService = batchExecutions, id: string | null = null): Dialog {
  const returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  let panel: BatchExecutionPanel;
  const dlg = new Dialog({ title: "批执行记录", width: 840, height: 600,
    render: body => { body.classList.add("batch-execution-dialog-body"); panel = new BatchExecutionPanel(service, id, { onDetach: () => dlg.close() }); body.append(panel.el); },
    buttons: [{ label: "关闭" }],
  });
  accessibleToolDialog(dlg, "批执行记录", () => panel.dispose(), returnFocus);
  return dlg;
}

/** 文件、目标在打开时固定；关闭仅脱离视图，不能释放在途任务或重放票据。 */
export function openSqlFileExecutionDialog(
  connection: ConnectionConfig, namespace: string, path: string, service: BatchExecutionService = batchExecutions,
): Dialog {
  const target = Object.freeze({ connection_id: connection.id, namespace, path, name: connection.name });
  const ownerId = crypto.randomUUID();
  const returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  let taskId: string | null = null;
  let starting = false;
  const tabs = document.createElement("div"); tabs.className = "batch-sql-tabs";
  tabs.setAttribute("role", "tablist"); tabs.setAttribute("aria-label", "运行 SQL 文件");
  const generalTab = toolButton("常规"), recordsTab = toolButton("执行记录");
  const general = document.createElement("section"), records = document.createElement("section");
  const tabButtons = [generalTab, recordsTab], panels = [general, records];
  const selectTab = (index: number) => {
    tabButtons.forEach((tab, i) => { tab.setAttribute("aria-selected", String(i === index)); tab.tabIndex = i === index ? 0 : -1; panels[i].hidden = i !== index; });
  };
  tabButtons.forEach((tab, index) => {
    tab.id = `sql-file-${ownerId}-tab-${index}`; tab.setAttribute("role", "tab");
    panels[index].id = `sql-file-${ownerId}-panel-${index}`;
    panels[index].className = "batch-sql-tabpanel"; panels[index].setAttribute("role", "tabpanel");
    panels[index].setAttribute("aria-labelledby", tab.id); tab.setAttribute("aria-controls", panels[index].id);
    tab.addEventListener("click", () => selectTab(index));
    tab.addEventListener("keydown", event => {
      if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
      event.preventDefault(); const next = event.key === "Home" ? 0 : event.key === "End" ? 1 : 1 - index;
      selectTab(next); tabButtons[next].focus();
    });
    tabs.append(tab);
  });
  const options = document.createElement("div"); options.className = "batch-sql-options";
  for (const [label, value] of [["固定目标", `${target.name} / ${target.namespace || "（默认）"}`], ["源文件", target.path]]) {
    const input = document.createElement("input"); input.type = "text"; input.readOnly = true;
    input.value = value; input.title = value; input.setAttribute("aria-label", label); input.className = "batch-fixed-value";
    options.append(field(label, input));
  }
  const encoding = document.createElement("select"); encoding.setAttribute("aria-label", "文件编码");
  const encodings: [BatchFileEncoding, string][] = [["utf-8", "UTF-8"], ["gbk", "GBK"], ["gb18030", "GB18030"], ["big5", "BIG5"], ["latin1", "LATIN1"]];
  encoding.append(...encodings.map(([value, label]) => { const option = document.createElement("option"); option.value = value; option.textContent = label; return option; }));
  const timeout = document.createElement("input"); timeout.type = "number"; timeout.min = "0"; timeout.step = "1"; timeout.required = true; timeout.value = "0";
  timeout.setAttribute("aria-label", "文件总超时（秒）");
  const timeoutHint = document.createElement("div"); timeoutHint.id = `sql-file-${ownerId}-timeout-hint`; timeoutHint.setAttribute("role", "status");
  const timeoutHelp = "整个 SQL 文件的总执行时限；0 表示不限时。";
  timeoutHint.textContent = timeoutHelp; timeout.setAttribute("aria-describedby", timeoutHint.id);
  timeout.addEventListener("input", () => { timeout.removeAttribute("aria-invalid"); timeoutHint.textContent = timeoutHelp; });
  const stopLabel = document.createElement("label"); stopLabel.className = "checkbox";
  const stopOnError = document.createElement("input"); stopOnError.type = "checkbox"; stopOnError.checked = true;
  const stopText = document.createElement("span"); stopText.textContent = "遇到错误时停止"; stopLabel.append(stopOnError, stopText);
  const notice = document.createElement("div"); notice.setAttribute("role", "status");
  options.append(field("文件编码", encoding), field("文件总超时（秒）", timeout), timeoutHint, stopLabel, notice); general.append(options); selectTab(0);
  let panel: BatchExecutionPanel;
  const refresh = () => {
    const record = taskId ? service.get(taskId) : null;
    const locked = starting || !!record?.busy || !!record?.unknown;
    start.disabled = locked || connection.read_only || !target.path.trim();
    encoding.disabled = locked; timeout.disabled = locked; stopOnError.disabled = locked;
    notice.textContent = connection.read_only ? "此连接只读，不能运行 SQL 文件。" : record?.unknown ? "结果未知；继续回看同一任务，不能重新运行。"
      : locked ? "任务仍在执行或等待资源排空，可在执行记录中停止或转入后台。" : "按完整语句边扫描边执行；已执行部分以真实回执为准。";
  };
  const start = toolButton("开始执行", true);
  start.addEventListener("click", () => {
    refresh(); if (start.disabled) return;
    const timeoutMs = Number(timeout.value) * 1000;
    if (!/^\d+$/.test(timeout.value) || !Number.isSafeInteger(timeoutMs) || timeoutMs < 0) {
      timeout.setAttribute("aria-invalid", "true");
      timeoutHint.textContent = "文件总超时必须是非负整数秒，且换算为毫秒后不能超出安全整数范围；0 表示不限时。";
      selectTab(0); timeout.focus(); return;
    }
    starting = true; refresh(); selectTab(1);
    try {
      const record = service.start({ connection_id: target.connection_id, namespace: target.namespace, owner_id: ownerId, timeout_ms: timeoutMs,
        task: { kind: "sql_file", path: target.path, encoding: encoding.value as BatchFileEncoding, stop_on_error: stopOnError.checked } }, target.name, connection.kind);
      taskId = record.id; panel.setTask(record.id);
      // 后台任务中心登记：对话框关闭（转后台）后仍可在任务面板看到执行状态
      const fileName = target.path.split(/[\\/]/).pop() ?? target.path;
      const handle = taskCenter.register("sql-file", `运行 SQL 文件 · ${fileName}`);
      // 点击后台任务时回看这次执行的记录面板（只读回看，不会重放任务）。
      handle.setOpener(() => { void openBatchExecutionDialog(service, record.id); });
      const mirror = () => {
        const current = service.get(record.id);
        if (!current) return;
        if (current.busy) {
          handle.update({ message: batchStateLabel(current.state) });
          return;
        }
        if (current.unknown) handle.cancel("结果未知：请在执行记录中确认");
        else if (current.state === "failed") handle.fail(current.notice || "执行失败");
        else handle.finish(current.summary || "执行完成", target.path);
        mirrorUnsubscribe();
      };
      const mirrorUnsubscribe = service.subscribe(mirror);
    } finally { starting = false; refresh(); }
  });
  const dlg = new Dialog({ title: "运行 SQL 文件", width: 840, height: 660,
    render: body => {
      body.classList.add("batch-execution-dialog-body");
      panel = new BatchExecutionPanel(service, null, { onDetach: () => dlg.close(), onConfigure: () => selectTab(0) });
      records.append(panel.el); body.append(tabs, general, records);
    }, buttons: [{ label: "关闭" }],
  });
  dlg.footer.prepend(start);
  const unsubscribe = service.subscribe(refresh); refresh();
  accessibleToolDialog(dlg, "运行 SQL 文件", () => { unsubscribe(); panel.dispose(); }, returnFocus);
  return dlg;
}
