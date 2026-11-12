/** 逐条执行记录：折叠摘要 + 展开三段格式；支持过滤、跟随最新与暂停缓冲。 */
import type { BatchExecutionLog } from "../core/api";
import { formatEpoch, sqlHistoryParts } from "../core/log-format";
import "../styles/execution-log.css";

export type LogFilter = "all" | "error" | "slow";

const SLOW_MS = 100;
const MAX_ROWS = 300;

/** 条目展开态需要的任务上下文；每次渲染时读取，跟随当前任务切换。 */
export interface LogContext {
  connectionName: string;
  database: string;
  driver: string;
  sessionId: string;
}

export interface LogListOptions {
  context: () => LogContext;
  /** 用户手动上滚导致不再跟随时回调，便于外部复选框同步。 */
  onFollowChange?: (follow: boolean) => void;
  /** 条目集合或过滤变化时回调，便于外部更新计数。 */
  onCountChange?: () => void;
}

function duration(value: number | null | undefined): string {
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) return "—";
  if (value >= 1000) return (value / 1000).toFixed(2) + "s";
  return (Math.round(value * 10) / 10) + "ms";
}
function slowBand(value: number | null | undefined): string {
  if (typeof value !== "number" || !Number.isFinite(value)) return "";
  if (value >= 1000) return "fail";
  if (value >= SLOW_MS) return "warn";
  return "";
}
function oneLine(text: string): string {
  return text.replace(/\s+/g, " ").trim();
}

interface RowEntry { log: BatchExecutionLog; row: HTMLDivElement; detail: HTMLDivElement | null; }

export class ExecutionLogList {
  readonly el = document.createElement("div");
  private readonly entries = new Map<number, RowEntry>();
  private filterKey: LogFilter = "all";
  private followKey = true;
  private paused = false;
  private autoExpandKey = true;
  private pending: BatchExecutionLog[] = [];

  constructor(private readonly options: LogListOptions) {
    this.el.className = "exec-log-list batch-execution-logs";
    this.el.setAttribute("role", "log");
    this.el.setAttribute("aria-label", "逐条执行记录");
    this.el.tabIndex = 0;
    this.el.addEventListener("scroll", () => {
      if (!this.followKey) return;
      if (this.el.scrollHeight - this.el.clientHeight - this.el.scrollTop > 40) {
        this.followKey = false;
        this.options.onFollowChange?.(false);
      }
    });
  }

  append(logs: readonly BatchExecutionLog[]): void {
    if (!logs.length) return;
    if (this.paused) { this.pending.push(...logs); return; }
    let inserted = false;
    for (const log of logs) inserted = this.insert(log) || inserted;
    if (inserted) { this.prune(); if (this.followKey) this.scrollToEnd(); this.options.onCountChange?.(); }
  }

  setPaused(paused: boolean): void {
    this.paused = paused;
    if (!paused) this.flush();
  }
  get pausedValue(): boolean { return this.paused; }

  /** 暂停期间积压的条目一次性补入，保留用户当前滚动位置。 */
  flush(): void {
    if (!this.pending.length) return;
    const batch = this.pending; this.pending = [];
    const top = this.el.scrollTop;
    for (const log of batch) this.insert(log);
    this.prune();
    this.el.scrollTop = this.followKey ? this.el.scrollHeight : top;
    this.options.onCountChange?.();
  }

  setFollow(follow: boolean): void {
    this.followKey = follow;
    if (follow) this.scrollToEnd();
  }
  get follow(): boolean { return this.followKey; }

  setFilter(filter: LogFilter): void {
    this.filterKey = filter;
    for (const entry of this.entries.values()) {
      const show = this.matches(entry.log);
      entry.row.hidden = !show;
      if (entry.detail) entry.detail.hidden = !(show && entry.row.getAttribute("aria-expanded") === "true");
    }
    this.options.onCountChange?.();
  }
  get filter(): LogFilter { return this.filterKey; }

  /** 带 SQL 的执行条目展开显示完整语句；阶段信息保持折叠。 */
  setAutoExpand(on: boolean): void {
    this.autoExpandKey = on;
    for (const entry of this.entries.values()) {
      const open = entry.row.getAttribute("aria-expanded") === "true";
      if (on && this.expandable(entry.log) && !open) this.toggle(entry.log.sequence);
      if (!on && open) this.toggle(entry.log.sequence);
    }
  }
  get autoExpand(): boolean { return this.autoExpandKey; }
  private expandable(log: BatchExecutionLog): boolean {
    return log.phase === "execute" && !!log.sql_template && this.matches(log);
  }

  visible(): BatchExecutionLog[] {
    return [...this.entries.values()].filter(entry => this.matches(entry.log)).map(entry => entry.log);
  }
  count(filter?: LogFilter): number {
    if (!filter || filter === "all") return this.entries.size;
    return [...this.entries.values()].filter(entry => filter === "error" ? entry.log.level === "error" : (entry.log.elapsed_ms ?? 0) >= SLOW_MS).length;
  }

  /** 复制/导出的纯文本：与历史日志窗口逐字节同格式，条目间空一行。 */
  toText(): string {
    const context = this.options.context();
    return this.visible().map(log => sqlHistoryParts(this.asEntry(log, context)).map(part => part.text).join("\n")).join("\n\n");
  }

  clear(): void {
    this.entries.clear();
    this.pending = [];
    this.el.replaceChildren();
    this.options.onCountChange?.();
  }
  dispose(): void { this.clear(); }

  private matches(log: BatchExecutionLog): boolean {
    if (this.filterKey === "error") return log.level === "error";
    if (this.filterKey === "slow") return (log.elapsed_ms ?? 0) >= SLOW_MS;
    return true;
  }

  private asEntry(log: BatchExecutionLog, context: LogContext) {
    return {
      ts: formatEpoch(log.ts_ms) || "—",
      connection_name: context.connectionName || "—",
      database: context.database || "—",
      kind: context.driver || "—",
      sql: log.sql_template ?? log.message,
      ok: log.level !== "error",
      elapsed_ms: log.elapsed_ms ?? 0,
      message: log.message,
    };
  }

  private summaryText(log: BatchExecutionLog): string {
    if (log.phase === "execute" && log.sql_template) return oneLine(log.sql_template);
    return oneLine(log.message) || "—";
  }
  private resultText(log: BatchExecutionLog): string {
    if (log.level === "error") return log.error_code ? "ERROR " + log.error_code : "失败";
    if (typeof log.affected_rows === "number" && Number.isFinite(log.affected_rows)) return Math.floor(log.affected_rows).toLocaleString("en-US") + " 行";
    return "";
  }
  private attrText(log: BatchExecutionLog, context: LogContext): string {
    const parts: string[] = [];
    if (log.statement !== null) parts.push("语句 #" + log.statement);
    if (typeof log.affected_rows === "number") parts.push("影响 " + Math.floor(log.affected_rows).toLocaleString("en-US") + " 行");
    if (context.sessionId) parts.push("会话 " + context.sessionId);
    if (context.driver) parts.push("驱动 " + context.driver.toUpperCase());
    return parts.join(" · ");
  }

  private insert(log: BatchExecutionLog): boolean {
    if (this.entries.has(log.sequence)) return false;
    const time = formatEpoch(log.ts_ms);
    const row = document.createElement("div");
    row.className = "exec-log-row";
    row.dataset.level = log.level;
    row.setAttribute("role", "button");
    row.tabIndex = 0;
    row.setAttribute("aria-expanded", "false");

    const caret = document.createElement("span"); caret.className = "exec-log-caret"; caret.setAttribute("aria-hidden", "true");
    const seq = document.createElement("span"); seq.className = "exec-log-seq";
    seq.textContent = log.statement !== null ? "语句 #" + log.statement : log.row !== null ? "行 #" + log.row : "—";
    const stamp = document.createElement("span"); stamp.className = "exec-log-time";
    stamp.textContent = time ? time.slice(11) : "—";
    const dot = document.createElement("span"); dot.className = "exec-log-dot"; dot.setAttribute("aria-hidden", "true");
    const elapsed = document.createElement("span"); elapsed.className = "exec-log-ms";
    elapsed.textContent = duration(log.elapsed_ms);
    const band = slowBand(log.elapsed_ms); if (band) elapsed.dataset.slow = band;
    const sql = document.createElement("span"); sql.className = "exec-log-sql"; sql.textContent = this.summaryText(log);
    const result = document.createElement("span"); result.className = "exec-log-result";
    result.textContent = this.resultText(log);
    if (log.level === "error") result.dataset.bad = "true";
    row.append(caret, seq, stamp, dot, elapsed, sql, result);
    row.hidden = !this.matches(log);
    row.addEventListener("click", () => this.toggle(log.sequence));
    row.addEventListener("keydown", event => {
      if (event.key === "Enter" || event.key === " ") { event.preventDefault(); this.toggle(log.sequence); }
      else if (event.key === "ArrowDown" || event.key === "ArrowUp") { event.preventDefault(); this.move(event.key === "ArrowDown" ? 1 : -1, row); }
    });
    this.el.append(row);
    this.entries.set(log.sequence, { log, row, detail: null });
    if (this.autoExpandKey && this.expandable(log)) this.toggle(log.sequence);
    return true;
  }

  private toggle(sequence: number): void {
    const entry = this.entries.get(sequence);
    if (!entry) return;
    if (entry.row.getAttribute("aria-expanded") === "true") {
      entry.row.setAttribute("aria-expanded", "false");
      if (entry.detail) entry.detail.hidden = true;
      return;
    }
    entry.row.setAttribute("aria-expanded", "true");
    if (!entry.detail) {
      entry.detail = this.buildDetail(entry.log);
      entry.row.after(entry.detail);
    }
    entry.detail.hidden = !this.matches(entry.log);
  }

  private buildDetail(log: BatchExecutionLog): HTMLDivElement {
    const context = this.options.context();
    const detail = document.createElement("div");
    detail.className = "exec-log-detail";
    detail.dataset.level = log.level;
    for (const part of sqlHistoryParts(this.asEntry(log, context))) {
      if (part.kind === "meta") {
        const meta = document.createElement("div"); meta.className = "exec-log-meta"; meta.textContent = part.text; detail.append(meta);
      } else if (part.kind === "sql") {
        const pre = document.createElement("pre"); pre.className = "exec-log-sqltext"; pre.textContent = part.text; detail.append(pre);
      } else if (part.kind === "time") {
        const foot = document.createElement("div"); foot.className = "exec-log-detail-foot";
        const time = document.createElement("span"); time.className = "exec-log-duration"; time.textContent = part.text;
        const attr = document.createElement("span"); attr.className = "exec-log-attr"; attr.textContent = this.attrText(log, context);
        foot.append(time, attr); detail.append(foot);
      } else {
        const error = document.createElement("div"); error.className = "exec-log-error";
        error.textContent = log.error_code ? part.text + " (" + log.error_code + ")" : part.text;
        detail.append(error);
      }
    }
    return detail;
  }

  private move(delta: number, current: HTMLElement): void {
    const rows = [...this.el.querySelectorAll<HTMLElement>(".exec-log-row")].filter(row => !row.hidden);
    const index = rows.indexOf(current);
    const next = rows[Math.min(rows.length - 1, Math.max(0, index + delta))];
    if (next && next !== current) next.focus();
  }

  private prune(): void {
    while (this.entries.size > MAX_ROWS) {
      const oldest = this.entries.keys().next().value as number | undefined;
      if (oldest === undefined) break;
      const entry = this.entries.get(oldest);
      if (!entry) { this.entries.delete(oldest); continue; }
      if (entry.detail) entry.detail.remove();
      entry.row.remove();
      this.entries.delete(oldest);
    }
  }

  private scrollToEnd(): void { this.el.scrollTop = this.el.scrollHeight; }
}