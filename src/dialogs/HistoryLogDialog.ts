/** 历史日志查看器：主来源为本工具的 SQL 执行日志；另有运行日志/自动运行历史。只读展示，局部搜索/筛选。 */
import { api } from "../core/api";
import type { JobHistoryEntry } from "../core/types";
import type { SqlLogEntry } from "../core/api";
import { Dialog, field, select, textInput } from "../ui/Dialog";
import { accessibleToolDialog, toolButton, toolTable } from "./toolDialog";
export type HistoryLogApi = Pick<typeof api, "readLog" | "listJobHistory" | "listSqlLog" | "clearSqlLog">;
export interface HistoryLogRow {
  timestamp: string;
  category: string;
  outcome: "success" | "error" | "unknown";
  message: string;
  raw: string;
  /** SQL 执行日志的完整记录（点击行展开详情用）。 */
  detail?: SqlLogEntry;
}
export function parseHistoryLog(lines: string[]): HistoryLogRow[] {
  return lines.map(raw => {
    const match = /^\[([^\]]+)\]\s+(\S+)\s*(.*)$/.exec(raw);
    const message = match?.[3] ?? raw;
    return { timestamp: match?.[1] ?? "", category: match?.[2] ?? "未分类", outcome: "unknown", message, raw };
  });
}
export function jobHistoryRows(entries: JobHistoryEntry[]): HistoryLogRow[] {
  return entries.map(entry => ({ timestamp: entry.started_at, category: "自动运行", outcome: entry.ok ? "success" : "error",
    message: `${entry.job_name} · ${entry.elapsed_ms} ms · ${entry.detail}`,
    raw: `[${entry.started_at}] ${entry.job_name} (${entry.job_id}) ${entry.ok ? "成功" : "失败"} ${entry.elapsed_ms} ms ${entry.detail}` }));
}
export function sqlLogRows(entries: SqlLogEntry[]): HistoryLogRow[] {
  return entries.map(entry => {
    const summary = entry.sql.replace(/\s+/g, " ").trim();
    const detail = [`${entry.connection_name}（${entry.kind}）${entry.database ? ` · ${entry.database}` : ""}`,
      `${entry.ok ? "成功" : `失败：${entry.message}`}`,
      `耗时 ${entry.elapsed_ms} ms · ${entry.rows >= 0 ? `${entry.rows} 行` : "无行数信息"} · 来源 ${entry.origin}`,
      entry.sql].join("\n");
    return { timestamp: entry.ts, category: "SQL", outcome: entry.ok ? "success" : "error",
      message: `${entry.connection_name}${entry.database ? ` · ${entry.database}` : ""} · ${entry.elapsed_ms} ms · ${summary.slice(0, 160)}`,
      raw: detail, detail: entry };
  });
}
export function filterHistoryLog(rows: HistoryLogRow[], search: string, category = "all", outcome = "all"): HistoryLogRow[] {
  const needle = search.trim().toLocaleLowerCase();
  return rows.filter(row => (category === "all" || category === row.category) && (outcome === "all" || outcome === row.outcome) && (!needle || `${row.timestamp}\n${row.category}\n${row.message}\n${row.raw}`.toLocaleLowerCase().includes(needle)));
}

/** App cmdShowLog 可直接调用 openHistoryLogDialog()；不需要连接上下文。 */
export function openHistoryLogDialog(source: HistoryLogApi = api): Dialog {
  const returnFocus = document.activeElement as HTMLElement | null;
  const mode = select([
    { value: "sql", label: "SQL 执行日志（最近 2000 条 · 30 天内）" },
    { value: "application", label: "运行日志（最近 500 条）" },
    { value: "jobs", label: "自动运行历史（最近 200 条）" },
  ], "sql", 280); mode.setAttribute("aria-label", "日志来源");
  const outcome = select([{ value: "all", label: "全部结果" }, { value: "success", label: "成功" }, { value: "error", label: "失败" }], "all", 120); outcome.setAttribute("aria-label", "结果");
  const category = select([{ value: "all", label: "全部分类" }], "all", 180); category.setAttribute("aria-label", "日志分类");
  const search = textInput("", 300); search.setAttribute("aria-label", "搜索日志"); search.placeholder = "时间、连接或 SQL 文本";
  const refresh = toolButton("刷新"), copy = toolButton("复制筛选结果"), clear = toolButton("清空 SQL 日志"); copy.disabled = true;
  const status = document.createElement("div"); status.setAttribute("role", "status"); status.setAttribute("aria-live", "polite"); status.style.cssText = "margin:8px 0;min-height:24px;white-space:pre-wrap;";
  const content = document.createElement("div"); content.style.cssText = "height:300px;overflow:auto;border:1px solid var(--border);";
  const detail = document.createElement("div");
  detail.style.cssText = "display:none;margin-top:8px;border:1px solid var(--border);border-radius:4px;padding:8px;background:var(--bg-subtle,transparent);";
  const detailText = document.createElement("pre");
  detailText.style.cssText = "margin:0;white-space:pre-wrap;word-break:break-all;font-family:var(--font-mono,monospace);font-size:12px;max-height:140px;overflow:auto;";
  const detailTitle = document.createElement("div"); detailTitle.style.cssText = "font-weight:600;margin-bottom:6px";
  detail.append(detailTitle, detailText);
  let sql: HistoryLogRow[] = [], application: HistoryLogRow[] = [], jobs: HistoryLogRow[] = [], visible: HistoryLogRow[] = [], closed = false, busy = false, version = 0, sampledAt = "";
  let errors: string[] = [];
  const isSql = () => mode.value === "sql";
  const selectedRows = () => isSql() ? sql : mode.value === "jobs" ? jobs : application;
  const showDetail = (row: HistoryLogRow | null) => {
    if (!row?.detail) { detail.style.display = "none"; return; }
    detailTitle.textContent = `${row.timestamp} · ${row.detail.connection_name} · ${row.detail.ok ? "成功" : "失败"}`;
    detailText.textContent = row.raw;
    detail.style.display = "block";
  };
  const render = () => {
    const rows = selectedRows();
    visible = filterHistoryLog(rows, search.value, "all", outcome.value);
    content.replaceChildren();
    if (visible.length) {
      const table = isSql()
        ? toolTable(["时间", "连接", "库", "状态", "耗时", "SQL"], visible.map(row => {
            const d = row.detail!;
            return [row.timestamp, d.connection_name, d.database || "-", row.outcome === "success" ? "成功" : "失败",
              `${d.elapsed_ms} ms`, d.sql.replace(/\s+/g, " ").trim().slice(0, 120) || "-"];
          }))
        : toolTable(["时间", "分类", "结果", "消息"], visible.map(row => [row.timestamp || "未提供", row.category, row.outcome === "success" ? "成功" : row.outcome === "error" ? "失败" : "未提供", row.message]));
      if (isSql()) {
        table.querySelectorAll<HTMLElement>("tbody tr").forEach((tr, i) => {
          const row = visible[i];
          if (!row?.detail) return;
          tr.style.cursor = "pointer";
          tr.tabIndex = 0;
          tr.setAttribute("aria-label", "查看 SQL 详情");
          const open = () => showDetail(row);
          tr.addEventListener("click", open);
          tr.addEventListener("keydown", e => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); open(); } });
        });
      }
      content.appendChild(table);
    } else {
      content.textContent = rows.length ? "没有符合当前搜索/筛选的记录。" : errors.length ? "未读取到此来源的日志，请检查错误后刷新重试。" : "此来源还没有记录；执行 SQL 后会出现在这里。";
    }
    copy.disabled = !visible.length;
    clear.hidden = !isSql();
    status.textContent = `${sampledAt ? `读取 ${sampledAt} · ` : ""}显示 ${visible.length} / ${rows.length} 条${errors.length ? ` · ${errors.join("；")}` : ""}`;
    status.setAttribute("role", errors.length ? "alert" : "status");
  };
  const updateFilters = () => {
    categoryField.style.display = isSql() ? "none" : "";
    outcomeField.style.display = isSql() || mode.value === "jobs" ? "" : "none";
    outcome.disabled = mode.value === "application";
    const old = category.value; category.replaceChildren();
    for (const value of ["all", ...new Set(selectedRows().map(row => row.category))]) {
      const option = document.createElement("option"); option.value = value; option.textContent = value === "all" ? "全部分类" : value; category.appendChild(option);
    }
    category.value = [...category.options].some(o => o.value === old) ? old : "all";
    if (!isSql()) showDetail(null);
    render();
  };
  const load = async () => {
    if (busy || closed) return;
    const request = ++version; busy = true; refresh.disabled = true; content.setAttribute("aria-busy", "true"); status.textContent = "正在读取日志…";
    const result = await Promise.allSettled([source.listSqlLog(), source.readLog(), source.listJobHistory()]);
    if (closed || request !== version) return;
    errors = [];
    if (result[0].status === "fulfilled") sql = sqlLogRows(result[0].value);
    else { sql = []; errors.push(`SQL 执行日志读取失败：${String(result[0].reason)}`); }
    if (result[1].status === "fulfilled") application = parseHistoryLog(result[1].value);
    else { application = []; errors.push(`运行日志读取失败：${String(result[1].reason)}`); }
    if (result[2].status === "fulfilled") jobs = jobHistoryRows(result[2].value);
    else { jobs = []; errors.push(`任务历史读取失败：${String(result[2].reason)}`); }
    sampledAt = new Date().toLocaleTimeString(); busy = false; refresh.disabled = false; content.setAttribute("aria-busy", "false"); updateFilters();
  };
  const outcomeField = field("结果", outcome), categoryField = field("分类", category);
  const dlg = new Dialog({ title: "历史日志", width: 940, render: body => {
    body.append(field("日志来源", mode), outcomeField, categoryField, field("搜索", search));
    const actions = document.createElement("div"); actions.style.cssText = "display:flex;gap:8px;margin-top:8px;"; actions.append(refresh, copy, clear); body.append(actions, status, content, detail);
    const note = document.createElement("div"); note.textContent = "SQL 执行日志记录本工具实际执行的每条 SQL（查询、工具写入、监控采样等），保留最近 2000 条且不超过 30 天，超限自动清理；失败与成功都会记录，语句文本超长会截断。运行日志为后端保留的真实操作记录（最新优先）。";
    note.style.cssText = "margin-top:8px;line-height:1.5;color:var(--fg-muted);"; body.appendChild(note);
  }, buttons: [{ label: "关闭" }] });
  accessibleToolDialog(dlg, "历史日志", () => { closed = true; version++; }, returnFocus);
  mode.addEventListener("change", updateFilters); category.addEventListener("change", render); outcome.addEventListener("change", render); search.addEventListener("input", render);
  refresh.addEventListener("click", () => void load());
  copy.addEventListener("click", async () => {
    try { await navigator.clipboard.writeText(visible.map(row => row.raw).join("\n")); if (!closed) status.textContent = `已复制 ${visible.length} 条当前筛选记录。`; }
    catch (e) { if (!closed) { status.setAttribute("role", "alert"); status.textContent = `复制失败：${String(e)}。可直接选择表格文本复制。`; } }
  });
  clear.addEventListener("click", () => {
    if (busy) return;
    // Dialog 构造时自动挂载；确认后返回 true 关闭确认框。
    void new Dialog({ title: "清空 SQL 执行日志", width: 420, render: body => {
      body.textContent = `确定清空 SQL 执行日志？此操作不可恢复（运行日志与任务历史不受影响）。`;
    }, buttons: [
      { label: "清空", onClick: async () => {
          await source.clearSqlLog();
          sql = []; showDetail(null); sampledAt = new Date().toLocaleTimeString(); updateFilters();
          status.textContent = "SQL 执行日志已清空。";
          return true;
        } },
      { label: "取消" },
    ] });
    return false;
  });
  void load(); return dlg;
}
