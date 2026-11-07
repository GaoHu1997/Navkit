/** 数据库监控：真实只读状态采样 + 会话/正在执行的 SQL 清单与结束会话；所有数据来自本次实际采样。 */
import { api } from "../core/api";
import { DB_KINDS } from "../core/dbcatalog";
import type { ConnectionConfig, ProcessInfo, ResultSet, ServerInfo } from "../core/types";
import { Dialog, numberInput, select, field } from "../ui/Dialog";
import { accessibleToolDialog, toolButton, toolTable } from "./toolDialog";

export type MonitorApi = Pick<typeof api, "serverInfo" | "executeSql" | "listDbProcesses" | "killDbProcess">;
export interface MonitorSnapshot {
  capturedAt: string;
  elapsedMs: number;
  server: ServerInfo | null;
  metrics: ResultSet[];
  errors: string[];
  /** 会话清单；不支持会话的数据源为 null（processError 说明原因）。 */
  processes: ProcessInfo[] | null;
  processError: string | null;
}
export function monitorQuery(kind: string): string | null {
  switch (DB_KINDS.find(k => k.id === kind)?.protocol) {
    case "mysql": return "SHOW GLOBAL STATUS WHERE Variable_name IN ('Uptime','Threads_connected','Threads_running','Questions','Slow_queries','Bytes_received','Bytes_sent','Connections','Aborted_connects')";
    case "postgres": return "SELECT datname, numbackends, xact_commit, xact_rollback, blks_read, blks_hit, tup_returned, tup_fetched, tup_inserted, tup_updated, tup_deleted, deadlocks FROM pg_stat_database WHERE datname = current_database()";
    case "sqlite": return "SELECT sqlite_version() AS version, (SELECT page_count FROM pragma_page_count) AS page_count, (SELECT page_size FROM pragma_page_size) AS page_size, (SELECT freelist_count FROM pragma_freelist_count) AS free_pages, (SELECT journal_mode FROM pragma_journal_mode) AS journal_mode, (SELECT encoding FROM pragma_encoding) AS encoding";
    case "mssql": return "SELECT @@VERSION AS version, @@CONNECTIONS AS total_connections, @@CPU_BUSY AS cpu_busy_ticks, @@IO_BUSY AS io_busy_ticks, @@IDLE AS idle_ticks";
    case "clickhouse": return "SELECT metric, value, description FROM system.metrics ORDER BY metric LIMIT 100";
    default: return null;
  }
}
/** 该数据源是否有服务器会话概念（数据库监控会话页）。 */
export function supportsProcessList(kind: string): boolean {
  const protocol = DB_KINDS.find(k => k.id === kind)?.protocol;
  return protocol === "mysql" || protocol === "postgres" || protocol === "mssql" || protocol === "clickhouse" || protocol === "odbc";
}
export async function sampleServerMonitor(connection: ConnectionConfig, protocolMetrics = true, source: MonitorApi = api): Promise<MonitorSnapshot> {
  const started = performance.now();
  const snapshot: MonitorSnapshot = { capturedAt: new Date().toISOString(), elapsedMs: 0, server: null, metrics: [], errors: [], processes: null, processError: null };
  const query = protocolMetrics ? monitorQuery(connection.kind) : null;
  const jobs: Promise<void>[] = [source.serverInfo(connection.id).then(info => {
    snapshot.server = info;
    if (!info.version) snapshot.errors.push("服务器未提供版本，基本信息不完整");
  }).catch(e => { snapshot.errors.push(`基本信息：${e instanceof Error ? e.message : String(e)}`); })];
  if (query) jobs.push(source.executeSql(connection.id, query, undefined, undefined, "monitor").then(result => {
    snapshot.metrics = result.sets.filter(s => s.ok && s.has_rows);
    for (const set of result.sets.filter(s => !s.ok)) snapshot.errors.push(set.message || "协议状态查询失败");
    if (!snapshot.metrics.length && !result.sets.some(s => !s.ok)) snapshot.errors.push("协议状态查询没有返回指标");
  }).catch(e => { snapshot.errors.push(`协议状态：${e instanceof Error ? e.message : String(e)}`); }));
  if (typeof source.listDbProcesses === "function") {
    if (supportsProcessList(connection.kind)) {
      jobs.push(source.listDbProcesses(connection.id).then(processes => { snapshot.processes = processes; })
        .catch(e => { snapshot.processError = e instanceof Error ? e.message : String(e); }));
    } else {
      snapshot.processError = "该数据源没有服务器会话概念（SQLite 为嵌入式数据库）";
    }
  } else {
    snapshot.processError = "当前环境未提供会话查询接口";
  }
  await Promise.all(jobs); snapshot.elapsedMs = Math.round(performance.now() - started);
  snapshot.capturedAt = new Date().toISOString();
  return snapshot;
}

/** 非重叠刷新控制器：停止/关闭会使已发出的响应失效；不宣称终止已提交的后端查询。 */
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
      if (!this.disposed && version === this.version) this.receive({ capturedAt: new Date().toISOString(), elapsedMs: 0, server: null, metrics: [], errors: [error instanceof Error ? error.message : String(error)], processes: null, processError: null });
    } finally {
      this.busy = false;
      if (!this.disposed && this.queued) { this.queued = false; void this.refresh(); }
      else if (!this.disposed && this.interval > 0) this.schedule();
    }
  }
  start(seconds: number): void {
    this.stop(); this.interval = Math.max(2, Math.min(300, seconds)) * 1000;
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

export function openServerMonitorDialog(connection: ConnectionConfig, source: MonitorApi = api): Dialog {
  const returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const protocol = select([{ value: "status", label: "基本信息 + 协议状态（只读）" }, { value: "basic", label: "仅基本信息" }], "status", 260);
  if (!monitorQuery(connection.kind)) protocol.value = "basic";
  protocol.setAttribute("aria-label", "监控内容");
  const interval = numberInput(5, 80); interval.min = "2"; interval.max = "300"; interval.step = "1";
  interval.setAttribute("aria-label", "自动刷新间隔（秒）");
  const refresh = toolButton("立即刷新"), start = toolButton("开始自动刷新"), stop = toolButton("停止"); stop.disabled = true;
  const status = document.createElement("div"); status.setAttribute("role", "status"); status.setAttribute("aria-live", "polite");
  status.style.cssText = "min-height:24px;margin:8px 0;color:var(--fg-muted);";
  const content = document.createElement("div"); content.style.cssText = "height:320px;overflow:auto;border:1px solid var(--border);";
  const sessionContent = document.createElement("div"); sessionContent.style.cssText = "height:360px;overflow:auto;border:1px solid var(--border);";
  const sessionStatus = document.createElement("div"); sessionStatus.setAttribute("role", "status"); sessionStatus.setAttribute("aria-live", "polite");
  sessionStatus.style.cssText = "min-height:24px;margin:8px 0;color:var(--fg-muted);white-space:pre-wrap;";
  let closed = false;
  const canKill = !connection.read_only;
  const renderOverview = (snap: MonitorSnapshot) => {
    refresh.disabled = false; content.setAttribute("aria-busy", "false"); content.replaceChildren();
    const rows: (string | null)[][] = [];
    const info = snap.server;
    if (info) {
      for (const [key, value] of [["产品", info.product], ["版本", info.version], ["主机", info.host], ["端口", info.port ? String(info.port) : ""], ["字符集", DB_KINDS.find(k => k.id === connection.kind)?.protocol === "sqlite" ? "" : info.charset], ["排序规则", info.collation], ["运行时间", info.uptime ?? ""]]) if (value) rows.push([key, value]);
      rows.push(...info.extra.filter(([,value]) => value));
    }
    if (rows.length) content.appendChild(toolTable(["服务器信息", "值"], rows));
    for (const set of snap.metrics) content.appendChild(toolTable(set.columns.map(c => c.name), set.rows));
    if (!rows.length && !snap.metrics.length) content.textContent = "未取得服务器状态；请检查连接与权限后重试。";
    status.textContent = `采样 ${new Date(snap.capturedAt).toLocaleTimeString()} · ${snap.elapsedMs} ms${snap.errors.length ? ` · ${snap.errors.join("；")}` : " · 成功"}`;
    status.setAttribute("role", snap.errors.length ? "alert" : "status");
  };
  const renderSessions = (snap: MonitorSnapshot) => {
    sessionContent.replaceChildren();
    if (snap.processError) {
      sessionContent.textContent = snap.processError;
      sessionStatus.textContent = `会话读取于 ${new Date(snap.capturedAt).toLocaleTimeString()} · 不可用`;
      return;
    }
    const processes = snap.processes ?? [];
    if (!processes.length) {
      sessionContent.textContent = "当前没有可见会话（或该账号无权限查看其它会话）。";
      sessionStatus.textContent = `会话读取于 ${new Date(snap.capturedAt).toLocaleTimeString()} · 成功`;
      return;
    }
    const table = document.createElement("table"); table.style.cssText = "width:100%;border-collapse:collapse;font-size:12px;";
    const head = table.createTHead().insertRow();
    for (const name of ["会话ID", "用户", "来源", "库", "命令", "状态", "耗时(秒)", "正在执行的 SQL", canKill ? "操作" : ""].filter(Boolean)) {
      const cell = document.createElement("th"); cell.scope = "col"; cell.textContent = name;
      cell.style.cssText = "text-align:left;padding:5px 8px;border-bottom:1px solid var(--border);background:var(--bg-panel);position:sticky;top:0;";
      head.appendChild(cell);
    }
    const body = table.createTBody();
    for (const proc of processes) {
      const tr = body.insertRow();
      const cell = (value: string) => {
        const td = tr.insertCell(); td.textContent = value;
        td.style.cssText = "padding:4px 8px;border-bottom:1px solid var(--border);white-space:pre-wrap;overflow-wrap:anywhere;max-width:320px;";
        return td;
      };
      cell(proc.is_self ? `${proc.id}（本工具）` : proc.id);
      cell(proc.user); cell(proc.host); cell(proc.db || "-"); cell(proc.command || "-"); cell(proc.state || "-");
      cell(proc.elapsed_secs === null || proc.elapsed_secs === undefined ? "-" : String(proc.elapsed_secs));
      const sqlCell = cell(proc.sql ? proc.sql.replace(/\s+/g, " ").trim().slice(0, 160) : "-");
      if (proc.sql && proc.sql.length > 160) sqlCell.title = proc.sql;
      if (canKill) {
        const td = tr.insertCell(); td.style.cssText = "padding:4px 8px;border-bottom:1px solid var(--border);";
        const kill = toolButton(proc.is_self ? "断开本会话" : "结束会话");
        kill.title = proc.is_self ? "结束本工具自身的连接（自动刷新将中断，可重新打开窗口）" : "向数据库发送 KILL 结束该会话";
        kill.addEventListener("click", () => void confirmKill(proc));
        td.appendChild(kill);
      }
    }
    sessionContent.appendChild(table);
    sessionStatus.textContent = `会话 ${processes.length} 个 · 读取于 ${new Date(snap.capturedAt).toLocaleTimeString()}${canKill ? "" : " · 只读连接不可结束会话"}`;
  };
  const confirmKill = (proc: ProcessInfo): Promise<void> => (new Promise(resolve => {
    const confirm = new Dialog({ title: "结束数据库会话", width: 460, render: body => {
      body.textContent = `确定结束会话 ${proc.id}（${proc.user}${proc.host ? ` @ ${proc.host}` : ""}）？该会话正在执行的 SQL 将被数据库终止，未提交事务会回滚。`;
    }, buttons: [
      { label: "结束会话", onClick: async () => {
          sessionStatus.textContent = `正在结束会话 ${proc.id}…`;
          try {
            const message = await source.killDbProcess(connection.id, proc.id);
            sessionStatus.textContent = message;
          } catch (e) {
            sessionStatus.setAttribute("role", "alert");
            sessionStatus.textContent = `结束会话失败：${String(e)}`;
          }
          resolve(); return true;
        } },
      { label: "取消", onClick: () => { resolve(); return true; } },
    ] });
    void confirm;
  }));
  const controller = new ServerMonitorController(async () => {
    if (!closed) { status.textContent = "正在读取数据库状态…"; refresh.disabled = true; content.setAttribute("aria-busy", "true"); }
    return sampleServerMonitor(connection, protocol.value === "status", source);
  }, snap => {
    renderOverview(snap);
    renderSessions(snap);
  });
  // 页签：概览 / 会话与执行中的 SQL
  let showTab = (_tab: string) => {};
  const dlg = new Dialog({ title: "数据库监控", width: 860, height: 640, render: body => {
    const connectionText = document.createElement("div"); connectionText.textContent = `${connection.name} · ${DB_KINDS.find(k => k.id === connection.kind)?.label ?? connection.kind}`;
    connectionText.style.marginBottom = "12px"; body.appendChild(connectionText);
    const tabs = document.createElement("div"); tabs.style.cssText = "flex:1;min-height:0;display:flex;flex-direction:column;overflow:hidden";
    const header = document.createElement("div"); header.setAttribute("role", "tablist"); header.style.cssText = "display:flex;gap:4px;border-bottom:1px solid var(--border);padding-bottom:6px;margin-bottom:12px";
    const overviewPanel = document.createElement("div"); overviewPanel.setAttribute("role", "tabpanel");
    const sessionsPanel = document.createElement("div"); sessionsPanel.setAttribute("role", "tabpanel"); sessionsPanel.hidden = true;
    for (const [label, panel] of [["概览", overviewPanel], ["会话与执行中的 SQL", sessionsPanel]] as const) {
      const button = document.createElement("button"); button.type = "button"; button.className = "btn"; button.textContent = label;
      button.setAttribute("role", "tab"); button.setAttribute("aria-selected", String(panel === overviewPanel));
      button.addEventListener("click", () => showTab(label));
      header.appendChild(button);
    }
    overviewPanel.append(field("监控内容", protocol), field("刷新间隔（秒）", interval));
    const actions = document.createElement("div"); actions.style.cssText = "display:flex;gap:8px;margin-top:8px;"; actions.append(refresh, start, stop);
    overviewPanel.append(actions, status, content);
    const note = document.createElement("div"); note.textContent = "累计计数为服务器真实原值，不是瞬时速率。SQLite 无服务器进程，仅显示数据库页/日志模式；结束会话向数据库发送 KILL，不保证已提交的数据可回滚。";
    note.style.cssText = "margin-top:8px;color:var(--fg-muted);line-height:1.5;"; overviewPanel.appendChild(note);
    const sessionHint = document.createElement("div");
    sessionHint.textContent = "列出数据库当前会话与正在执行的 SQL；刷新与概览共用同一间隔。「结束会话」不可撤销。";
    sessionHint.style.cssText = "margin-bottom:8px;color:var(--fg-muted);line-height:1.5;";
    sessionsPanel.append(sessionHint, sessionContent, sessionStatus);
    showTab = (label: string) => {
      const overview = label === "概览";
      overviewPanel.hidden = !overview; sessionsPanel.hidden = overview;
      const buttons = [...header.querySelectorAll("button")];
      if (buttons[0]) buttons[0].setAttribute("aria-selected", String(overview));
      if (buttons[1]) buttons[1].setAttribute("aria-selected", String(!overview));
    };
    tabs.append(header, overviewPanel, sessionsPanel); body.appendChild(tabs);
  }, buttons: [{ label: "关闭" }] });
  accessibleToolDialog(dlg, "数据库监控", () => { closed = true; controller.dispose(); }, returnFocus);
  refresh.addEventListener("click", () => void controller.refresh());
  start.addEventListener("click", () => {
    const seconds = interval.valueAsNumber;
    if (!Number.isInteger(seconds) || seconds < 2 || seconds > 300) { interval.setAttribute("aria-invalid", "true"); status.textContent = "刷新间隔必须为 2–300 的整数秒"; return; }
    interval.removeAttribute("aria-invalid"); start.disabled = true; stop.disabled = false; interval.disabled = true; protocol.disabled = true;
    controller.start(seconds);
  });
  stop.addEventListener("click", () => {
    controller.stop(); start.disabled = false; stop.disabled = true; interval.disabled = false; protocol.disabled = false; refresh.disabled = false;
    content.setAttribute("aria-busy", "false"); status.textContent = "已停止自动刷新；未完成的采样结果不会更新此窗口。";
  });
  protocol.addEventListener("change", () => { controller.stop(); void controller.refresh(); });
  void controller.refresh();
  return dlg;
}
