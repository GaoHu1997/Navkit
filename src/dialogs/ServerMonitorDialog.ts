/** 数据库监控：选择连接后仅采样当前会话，SQL 随会话行展示，不保留已完成的语句。 */
import { api } from "../core/api";
import { DB_KINDS } from "../core/dbcatalog";
import type { ConnectionConfig, ProcessInfo } from "../core/types";
import { Dialog, numberInput, select } from "../ui/Dialog";
import { ICON_REFRESH, iconSvgByName, svg } from "../ui/icons";
import { accessibleToolDialog, toolButton } from "./toolDialog";
import "../styles/server-monitor-dialog.css";

export type MonitorApi = Pick<typeof api, "listDbProcesses" | "killDbProcess"> &
  Partial<Pick<typeof api, "ensureConnection">>;
export interface MonitorSnapshot {
  connectionId: string;
  capturedAt: string;
  elapsedMs: number;
  processes: ProcessInfo[] | null;
  processError: string | null;
}

export function supportsProcessList(kind: string): boolean {
  const protocol = DB_KINDS.find(k => k.id === kind)?.protocol;
  return ["mysql", "postgres", "mssql", "clickhouse", "odbc"].includes(protocol ?? "");
}

/** PostgreSQL 会返回空闲连接的上一条 query；等待锁的活动查询则不能误判成空闲。 */
export function currentProcessSql(process: ProcessInfo): string {
  const inactive = /^(?:sleep(?:ing)?|idle(?:\b|[（(])|dormant|awaiting\s+command|空闲|事务中空闲)/i;
  if (!process.sql?.trim() || inactive.test(process.command.trim()) || inactive.test(process.state.trim())) return "";
  return process.sql;
}

/** 不请求服务器基本信息、协议指标或 executeSql，也不依赖已选中的数据库。 */
export async function sampleServerMonitor(connection: ConnectionConfig, source: MonitorApi = api): Promise<MonitorSnapshot> {
  const started = performance.now();
  const snapshot: MonitorSnapshot = {
    connectionId: connection.id, capturedAt: new Date().toISOString(), elapsedMs: 0,
    processes: null, processError: null,
  };
  if (!supportsProcessList(connection.kind)) {
    snapshot.processError = "该数据源不支持服务器会话监控，请选择其他连接。";
  } else {
    try {
      const processes = await source.listDbProcesses(connection.id);
      snapshot.processes = processes.map(process => {
        const sql = currentProcessSql(process);
        return { ...process, sql, elapsed_secs: sql ? process.elapsed_secs : null };
      });
    } catch (error) {
      snapshot.processError = error instanceof Error ? error.message : String(error);
    }
  }
  snapshot.elapsedMs = Math.round(performance.now() - started);
  snapshot.capturedAt = new Date().toISOString();
  return snapshot;
}

/** 非重叠轮询：停止/切换/关闭使旧响应失效，不宣称取消已提交的服务器查询。 */
export class ServerMonitorController {
  private timer: ReturnType<typeof setTimeout> | null = null;
  private version = 0;
  private busy = false;
  private queued = false;
  private disposed = false;
  private interval = 0;
  constructor(private readonly sample: () => Promise<MonitorSnapshot>, private readonly receive: (snapshot: MonitorSnapshot) => void) {}
  async refresh(): Promise<void> {
    if (this.disposed) return;
    if (this.busy) { this.queued = true; return; }
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    this.busy = true; const version = this.version;
    try {
      const result = await this.sample();
      if (!this.disposed && version === this.version) this.receive(result);
    } catch (error) {
      if (!this.disposed && version === this.version) this.receive({
        connectionId: "", capturedAt: new Date().toISOString(), elapsedMs: 0, processes: null,
        processError: error instanceof Error ? error.message : String(error),
      });
    } finally {
      this.busy = false;
      if (!this.disposed && this.queued) { this.queued = false; void this.refresh(); }
      else if (!this.disposed && this.interval > 0) this.schedule();
    }
  }
  start(seconds: number): void {
    this.stop();
    this.interval = Math.max(2, Math.min(300, Number.isFinite(seconds) ? seconds : 5)) * 1000;
    void this.refresh();
  }
  stop(): void {
    this.interval = 0; this.version++; this.queued = false;
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
  }
  dispose(): void { this.stop(); this.disposed = true; }
  private schedule(): void {
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = setTimeout(() => { this.timer = null; void this.refresh(); }, this.interval);
  }
}

export function openServerMonitorDialog(
  connections: readonly ConnectionConfig[],
  initialConnection?: ConnectionConfig,
  source: MonitorApi = api,
  ensureConnection?: (id: string) => Promise<void>,
): Dialog {
  const returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const prefix = `server-monitor-${crypto.randomUUID()}`;
  const connectionSelect = select([
    { value: "", label: "请选择连接…" },
    ...connections.map(connection => ({
      value: connection.id,
      label: `${connection.name} · ${DB_KINDS.find(kind => kind.id === connection.kind)?.label ?? connection.kind}${connection.read_only ? " · 只读" : ""}`,
    })),
  ], connections.some(connection => connection.id === initialConnection?.id) ? initialConnection!.id : "");
  connectionSelect.className = "monitor-connection";
  const interval = numberInput(5); interval.min = "2"; interval.max = "300"; interval.step = "1";
  interval.className = "monitor-interval";
  const pause = toolButton("暂停"), refresh = toolButton("立即刷新");
  pause.classList.add("monitor-pause"); refresh.classList.add("monitor-refresh");
  const refreshIcon = document.createElement("span"); refreshIcon.setAttribute("aria-hidden", "true");
  refreshIcon.innerHTML = svg(ICON_REFRESH); refresh.prepend(refreshIcon);
  const status = document.createElement("div"); status.className = "monitor-status"; status.id = `${prefix}-status`;
  status.setAttribute("role", "status"); status.setAttribute("aria-live", "polite"); status.setAttribute("aria-atomic", "true");
  interval.setAttribute("aria-describedby", status.id);
  const count = document.createElement("span"); count.className = "monitor-count";
  const sampledAt = document.createElement("span"); sampledAt.className = "monitor-sampled-at";
  const content = document.createElement("div"); content.className = "monitor-table-wrap";
  content.setAttribute("role", "region"); content.setAttribute("aria-label", "当前连接及 SQL");
  const table = document.createElement("table"); table.className = "monitor-table";
  const columns = document.createElement("colgroup"); table.appendChild(columns);
  const head = table.createTHead().insertRow(), rows = table.createTBody();
  let closed = false, automatic = true, loading = false, generation = 0;
  let processes: ProcessInfo[] = [];
  const children = new Set<Dialog>();
  let detail: { dialog: Dialog; id: string; sql: string } | null = null;
  const selectedConnection = () => connections.find(connection => connection.id === connectionSelect.value);
  const setStatus = (message: string, error = false) => {
    if (closed) return;
    status.textContent = message;
    status.setAttribute("role", error ? "alert" : "status");
    status.setAttribute("aria-live", error ? "assertive" : "polite");
  };
  const updateControls = () => {
    const supported = supportsProcessList(selectedConnection()?.kind ?? "");
    refresh.disabled = !supported || loading;
    pause.disabled = !supported;
    pause.textContent = automatic ? "暂停" : "继续";
    content.setAttribute("aria-busy", String(loading));
  };
  const rebuildHead = () => {
    head.replaceChildren(); columns.replaceChildren();
    const names = ["会话 ID", "用户", "来源", "数据库", "状态", "耗时（秒）", "当前 SQL"];
    if (selectedConnection() && !selectedConnection()!.read_only) names.push("操作");
    names.forEach((name, index) => {
      const cell = document.createElement("th"); cell.scope = "col"; cell.textContent = name;
      const column = document.createElement("col"); column.className = `monitor-column-${index}`; columns.appendChild(column);
      cell.className = `monitor-column-${index}`; head.appendChild(cell);
    });
  };
  const renderEmpty = (message: string) => {
    rows.replaceChildren();
    const cell = rows.insertRow().insertCell(); cell.colSpan = head.cells.length;
    cell.className = "monitor-empty"; cell.textContent = message;
  };
  const restoreRowFocus = (trigger: HTMLElement) => {
    if (closed || children.size) return;
    const target = [...rows.querySelectorAll<HTMLElement>("[data-monitor-action]")].find(element =>
      element.dataset.monitorAction === trigger.dataset.monitorAction && element.dataset.monitorProcess === trigger.dataset.monitorProcess);
    (target ?? connectionSelect).focus();
  };
  const attachChild = (child: Dialog, title: string, trigger: HTMLElement, onClose?: () => void) => {
    children.add(child); dlg.el.inert = true;
    accessibleToolDialog(child, title, () => {
      children.delete(child); onClose?.();
      if (!closed) { dlg.el.inert = children.size > 0; restoreRowFocus(trigger); }
    });
  };
  const showSql = (process: ProcessInfo, trigger: HTMLElement) => {
    const sql = currentProcessSql(process);
    if (closed || !sql || children.size) return;
    const copyStatus = document.createElement("div"); copyStatus.className = "monitor-detail-status";
    copyStatus.setAttribute("role", "status"); copyStatus.setAttribute("aria-live", "polite");
    const child = new Dialog({ title: `会话 ${process.id} · 当前 SQL`, width: 880, render: (body, dialog) => {
      dialog.el.classList.add("server-monitor-sql-dialog");
      const text = document.createElement("pre"); text.className = "monitor-sql-full"; text.textContent = sql;
      text.tabIndex = 0; text.setAttribute("aria-label", "当前 SQL 原文"); body.append(text, copyStatus);
    }, buttons: [
      { label: "复制 SQL", onClick: async (dialog) => {
        const button = dialog.footer.querySelector<HTMLButtonElement>("button"); if (button?.disabled) return false;
        if (button) button.disabled = true;
        try {
          await navigator.clipboard.writeText(sql);
          if (dialog.el.isConnected) copyStatus.textContent = "已复制完整 SQL 原文。";
        } catch (error) {
          if (dialog.el.isConnected) { copyStatus.setAttribute("role", "alert"); copyStatus.textContent = `复制失败：${error instanceof Error ? error.message : String(error)}。可选择正文后按 Ctrl+C。`; }
        } finally { if (button && dialog.el.isConnected) button.disabled = false; }
        return false;
      } },
      { label: "关闭" },
    ] });
    detail = { dialog: child, id: process.id, sql };
    attachChild(child, `会话 ${process.id} · 当前 SQL`, trigger, () => { if (detail?.dialog === child) detail = null; });
  };
  const confirmKill = (process: ProcessInfo, trigger: HTMLElement) => {
    const connection = selectedConnection();
    if (closed || children.size || !connection || connection.read_only) return;
    const requestGeneration = generation;
    const label = process.is_self ? "断开本会话" : "结束会话";
    const notice = document.createElement("div"); notice.className = "monitor-confirm-status";
    notice.setAttribute("role", "status"); notice.setAttribute("aria-live", "polite");
    const prompt = document.createElement("p"); prompt.id = `${prefix}-kill-prompt`;
    prompt.textContent = `确定${label} ${process.id}（${process.user}${process.host ? ` @ ${process.host}` : ""}）？正在执行的 SQL 会被数据库终止，未提交事务可能回滚；已提交的数据不会回滚。${process.is_self ? "这是本工具的会话，监控可能暂时中断。" : ""}`;
    let confirmClosed = false, killing = false;
    const child = new Dialog({ title: label, width: 480, render: (body, dialog) => {
      dialog.el.classList.add("server-monitor-confirm"); body.append(prompt, notice);
    }, buttons: [
      { label: "取消" },
      { label, onClick: async (dialog) => {
        if (killing || closed || confirmClosed) return false;
        if (generation !== requestGeneration || selectedConnection()?.id !== connection.id) {
          notice.setAttribute("role", "alert"); notice.textContent = "监控连接已变化，请重新选择目标会话。"; return false;
        }
        killing = true;
        const buttons = [...dialog.footer.querySelectorAll<HTMLButtonElement>("button")]; buttons.forEach(button => { button.disabled = true; });
        notice.textContent = `正在${label} ${process.id}…`;
        try {
          const message = await source.killDbProcess(connection.id, process.id);
          if (!closed && generation === requestGeneration) { setStatus(message); void controller.refresh(); }
          return !closed && !confirmClosed;
        } catch (error) {
          if (!confirmClosed && !closed) { notice.setAttribute("role", "alert"); notice.textContent = `${label}失败：${error instanceof Error ? error.message : String(error)}`; }
          return false;
        } finally {
          killing = false;
          if (!confirmClosed && !closed) buttons.forEach(button => { button.disabled = false; });
        }
      } },
    ] });
    attachChild(child, label, trigger, () => { confirmClosed = true; });
    child.el.setAttribute("role", "alertdialog"); child.el.setAttribute("aria-describedby", prompt.id);
    child.footer.lastElementChild?.classList.add("danger");
    queueMicrotask(() => { if (!closed && !confirmClosed) child.footer.querySelector<HTMLButtonElement>("button")?.focus(); });
  };
  const renderSnapshot = (snapshot: MonitorSnapshot) => {
    if (closed || (snapshot.connectionId && snapshot.connectionId !== connectionSelect.value)) return;
    loading = false; updateControls();
    sampledAt.textContent = `采样 ${new Date(snapshot.capturedAt).toLocaleTimeString()} · ${snapshot.elapsedMs} ms`;
    processes = snapshot.processes ?? [];
    if (detail && !processes.some(process => process.id === detail!.id && currentProcessSql(process) === detail!.sql)) detail.dialog.close();
    const focused = document.activeElement instanceof HTMLElement && document.activeElement.dataset.monitorAction ? document.activeElement : null;
    if (snapshot.processError) {
      count.textContent = "当前连接 · 无法读取";
      renderEmpty("当前会话读取失败，请检查连接和权限后立即刷新重试。");
      setStatus(`读取失败：${snapshot.processError}`, true);
      if (focused) restoreRowFocus(focused);
      return;
    }
    count.textContent = `当前连接 ${processes.length}${selectedConnection()?.read_only ? " · 只读" : ""}`;
    rows.replaceChildren();
    for (const process of processes) {
      const row = rows.insertRow(); row.dataset.sessionId = process.id;
      const cell = (value: string, className = "") => {
        const td = row.insertCell(); td.textContent = value; if (value) td.title = value; td.className = className; return td;
      };
      cell(process.is_self ? `${process.id} · 本工具` : process.id);
      // 空值一律留白（不用「—」占位），悬停 title 也仅在确有内容时设置
      cell(process.user || ""); cell(process.host || ""); cell(process.db || "");
      const sql = currentProcessSql(process);
      cell(sql ? "执行中" : process.command || "").title = [process.command, process.state].filter(Boolean).join(" · ");
      cell(sql && process.elapsed_secs !== null ? String(process.elapsed_secs) : "", "monitor-elapsed");
      const sqlCell = cell("", "monitor-sql-cell");
      if (sql) {
        const preview = document.createElement("span"); preview.className = "monitor-sql-preview"; preview.textContent = sql.replace(/\s+/g, " ").trim();
        const view = toolButton("查看全文"); view.classList.add("monitor-view-sql");
        view.dataset.viewSql = process.id; view.dataset.monitorAction = "view-sql"; view.dataset.monitorProcess = process.id;
        view.setAttribute("aria-label", `查看会话 ${process.id} 的 SQL 全文`);
        view.addEventListener("click", () => showSql(process, view)); sqlCell.append(preview, view);
      } else sqlCell.textContent = "";
      if (!selectedConnection()?.read_only) {
        const action = cell("", "monitor-session-action");
        // 电源（关机）图标：终止/断开会话；外层 span 置 lineHeight 0 保证垂直居中。
        const kill = toolButton(""); kill.classList.add("monitor-kill");
        const killIcon = document.createElement("span"); killIcon.setAttribute("aria-hidden", "true");
        killIcon.style.lineHeight = "0"; killIcon.innerHTML = iconSvgByName("power");
        kill.append(killIcon);
        kill.dataset.killSession = process.id; kill.dataset.monitorAction = "kill-session"; kill.dataset.monitorProcess = process.id;
        const killLabel = `${process.is_self ? "断开本会话" : "结束会话"} ${process.id}`;
        kill.setAttribute("aria-label", killLabel);
        kill.title = killLabel;
        kill.addEventListener("click", () => confirmKill(process, kill)); action.appendChild(kill);
      }
    }
    if (!processes.length) renderEmpty("当前没有可见会话；账号权限可能限制可查看的范围。");
    setStatus(automatic ? `每 ${interval.value} 秒采样 · 已完成 SQL 不保留` : "已暂停自动采样 · 显示最后一次采样结果");
    if (focused) restoreRowFocus(focused);
  };
  const controller = new ServerMonitorController(async () => {
    const connection = selectedConnection();
    if (!connection || closed) throw new Error("请先选择连接");
    loading = true; updateControls(); setStatus("正在读取当前会话…");
    if (ensureConnection) await ensureConnection(connection.id);
    else if (source.ensureConnection) {
      const result = await source.ensureConnection(connection.id);
      if (!result.ok) throw new Error(result.message);
    }
    return sampleServerMonitor(connection, source);
  }, renderSnapshot);
  const labelled = (text: string, control: HTMLElement, key: string, suffix?: string) => {
    const label = document.createElement("label"); label.className = "monitor-control"; label.htmlFor = `${prefix}-${key}`; control.id = label.htmlFor;
    const name = document.createElement("span"); name.textContent = text; label.append(name, control);
    if (suffix) { const unit = document.createElement("span"); unit.textContent = suffix; label.appendChild(unit); }
    return label;
  };
  // 固定窗口尺寸：会话行数随采样变化，尺寸跟着变会让工具栏和关闭按钮移位。
  const dlg = new Dialog({ title: "数据库监控", width: 1240, height: 640, render: (body, dialog) => {
    dialog.el.classList.add("server-monitor-dialog");
    const toolbar = document.createElement("div"); toolbar.className = "monitor-toolbar";
    toolbar.append(labelled("连接", connectionSelect, "connection"), labelled("间隔", interval, "interval", "秒"), pause, refresh);
    const caption = document.createElement("div"); caption.className = "monitor-caption"; caption.append(count, sampledAt);
    content.appendChild(table); body.append(toolbar, caption, content);
  }, buttons: [{ label: "关闭" }] });
  dlg.footer.prepend(status);
  accessibleToolDialog(dlg, "数据库监控", () => {
    closed = true; generation++; controller.dispose();
    for (const child of children) child.close();
  }, returnFocus);
  const validInterval = () => {
    const seconds = interval.valueAsNumber;
    const valid = Number.isInteger(seconds) && seconds >= 2 && seconds <= 300;
    if (valid) interval.removeAttribute("aria-invalid");
    else { interval.setAttribute("aria-invalid", "true"); setStatus("采样间隔必须为 2–300 的整数秒。", true); }
    return valid;
  };
  const chooseConnection = () => {
    generation++; controller.stop();
    for (const child of children) child.close();
    processes = []; loading = false; sampledAt.textContent = ""; rebuildHead(); updateControls();
    const connection = selectedConnection();
    if (!connection) {
      count.textContent = "当前连接";
      renderEmpty(connections.length ? "请选择连接，随后查看其当前会话与 SQL；无需先打开数据库。" : "暂无已保存连接，请先在主界面新建连接，再打开监控。");
      setStatus("选择连接后开始监控。不会打开数据库标签。");
    } else if (!supportsProcessList(connection.kind)) {
      count.textContent = "当前连接 · 不支持监控";
      renderEmpty("该数据源没有服务器会话监控接口，请选择其他连接。");
      setStatus("当前连接不支持会话监控，不会回退显示服务器基本信息。");
    } else {
      count.textContent = "当前连接 · 读取中"; renderEmpty("正在读取当前会话…");
      if (automatic && validInterval()) controller.start(interval.valueAsNumber);
      else void controller.refresh();
    }
  };
  connectionSelect.addEventListener("change", chooseConnection);
  refresh.addEventListener("click", () => { if (!refresh.disabled) void controller.refresh(); });
  pause.addEventListener("click", () => {
    if (pause.disabled) return;
    if (automatic) {
      automatic = false; controller.stop(); loading = false; updateControls();
      setStatus("已暂停自动采样 · 显示最后一次采样结果；不会取消服务器正在执行的 SQL。");
    } else if (validInterval()) { automatic = true; updateControls(); controller.start(interval.valueAsNumber); }
  });
  interval.addEventListener("change", () => {
    const valid = validInterval();
    if (!valid) { automatic = false; controller.stop(); loading = false; updateControls(); }
    else if (automatic && supportsProcessList(selectedConnection()?.kind ?? "")) controller.start(interval.valueAsNumber);
  });
  chooseConnection(); return dlg;
}
