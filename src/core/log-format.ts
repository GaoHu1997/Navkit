/**
 * 执行记录文本格式的唯一实现：历史日志窗口与执行日志台共用同一份格式化。
 * 纯函数、零运行时依赖，便于在 vm/测试中单独执行。
 */

export type HistoryLineKind = "meta" | "sql" | "time" | "error";

/** 结构化执行记录的最小形状；SqlLogEntry 与批执行条目都满足。 */
export interface LogEntryLike {
  ts: string;
  connection_name: string;
  database?: string | null;
  kind?: string | null;
  sql: string;
  ok: boolean;
  elapsed_ms: number;
  message?: string | null;
}

function pad(value: number, size = 2): string { return String(value).padStart(size, "0"); }

/** 统一时间戳：本地 "YYYY-MM-DD HH:mm:ss.SSS"（记录原文为 ISO 时按本地时区展开）。 */
export function formatLogTimestamp(ts: string): string {
  const trimmed = ts.trim();
  const iso = /^\d{4}-\d{2}-\d{2}T/.test(trimmed) ? trimmed : trimmed.replace(" ", "T");
  const date = new Date(iso);
  if (!Number.isFinite(date.getTime())) return trimmed;
  return date.getFullYear() + "-" + pad(date.getMonth() + 1) + "-" + pad(date.getDate()) + " " +
    pad(date.getHours()) + ":" + pad(date.getMinutes()) + ":" + pad(date.getSeconds()) + "." + pad(date.getMilliseconds(), 3);
}

/** epoch 毫秒 → 同款本地时间戳；无有效值时返回空串，由调用方决定占位符。 */
export function formatEpoch(ts_ms: number | null | undefined): string {
  if (typeof ts_ms !== "number" || !Number.isFinite(ts_ms) || ts_ms <= 0) return "";
  const date = new Date(ts_ms);
  return date.getFullYear() + "-" + pad(date.getMonth() + 1) + "-" + pad(date.getDate()) + " " +
    pad(date.getHours()) + ":" + pad(date.getMinutes()) + ":" + pad(date.getSeconds()) + "." + pad(date.getMilliseconds(), 3);
}

/** 单条耗时文本；与历史窗口的 `Time: 0.000s` 逐字节一致。 */
export function formatLogTime(elapsed_ms: number): string { return "Time: " + (elapsed_ms / 1000).toFixed(3) + "s"; }

/**
 * 统一执行记录格式（所有执行入口一致）：
 *   [时间][连接][会话/库][驱动]
 *   <SQL 原文>
 *   Time: 0.000s
 *   Error: ...（仅失败）
 */
export function sqlHistoryParts(entry: LogEntryLike): { kind: HistoryLineKind; text: string }[] {
  const parts: { kind: HistoryLineKind; text: string }[] = [
    { kind: "meta", text: "[" + formatLogTimestamp(entry.ts) + "][" + entry.connection_name + "][" + (entry.database || "—") + "][" + ((entry.kind || "").toUpperCase()) + "]" },
    { kind: "sql", text: entry.sql },
    { kind: "time", text: formatLogTime(entry.elapsed_ms) },
  ];
  if (!entry.ok) parts.push({ kind: "error", text: "Error: " + (entry.message || "未提供错误信息") });
  return parts;
}
