/** 字节数的可读进位（B / KB / MB / GB / TB），供信息面板与执行记录共用。 */

/** 保留两位有效小数；1024 进制。 */
export function formatBytes(value: number): string {
  if (!Number.isFinite(value) || value < 0) return "—";
  const units = ["B", "KB", "MB", "GB", "TB", "PB"];
  let n = value;
  let unit = 0;
  while (n >= 1024 && unit < units.length - 1) {
    n /= 1024;
    unit += 1;
  }
  if (unit === 0) return `${Math.round(n)} B`;
  return `${n >= 100 ? n.toFixed(0) : n >= 10 ? n.toFixed(1) : n.toFixed(2)} ${units[unit]}`;
}

/** 可读大小 + 原始字节数，例如 "5.52 MB (5,783,552)"。 */
export function formatByteCount(value: number): string {
  if (!Number.isFinite(value) || value < 0) return "—";
  return `${formatBytes(value)} (${value.toLocaleString("en-US")})`;
}
