/** 只编码已加载原值；纯文本单格不能区分 NULL 与文本 NULL，TSV 用 \N 区分。 */
export type CopyMode = "default" | "rows" | "cell" | "rows_with_headers" | "spreadsheet_safe";
export type ClipboardValue = string | null | { kind: string; value?: string; encoding?: string };
export interface GridCopySnapshot {
  generation: number;
  columns: { name: string }[];
  rows: ClipboardValue[][];
  cell: ClipboardValue | undefined;
}
export const CLIPBOARD_LIMIT = 10 * 1024 * 1024;

function valueText(value: ClipboardValue): string | null {
  if (value === null) return null;
  if (typeof value === "string") return value;
  if (value.kind === "null") return null;
  if (value.kind === "binary") {
    if (value.encoding !== "base64") throw new Error("无法解码二进制值");
    return "0x" + Array.from(atob(value.value ?? ""), c => c.charCodeAt(0).toString(16).padStart(2, "0")).join("");
  }
  if (typeof value.value !== "string") throw new Error("结果值缺少完整文本，无法复制");
  return value.value;
}
function field(value: ClipboardValue, safe: boolean): string {
  let text = valueText(value);
  if (text === null) return "\\N";
  if (safe && (typeof value === "string" || (value !== null && value.kind === "text")) && /^[\s]*[=+@-]/.test(text)) text = "'" + text;
  return /[\t\r\n"]/.test(text) || text === "\\N" ? '"' + text.replace(/"/g, '""') + '"' : text;
}
export function encodeGridCopy(snapshot: GridCopySnapshot, mode: CopyMode): string {
  let text: string;
  if (mode === "cell") {
    if (snapshot.cell === undefined) throw new Error("请先选择一个单元格");
    text = valueText(snapshot.cell) ?? "NULL";
  } else {
    if (!snapshot.rows.length || !snapshot.columns.length) throw new Error("请先选择已加载的行");
    const lines = snapshot.rows.map(row => snapshot.columns.map((_, i) => {
      if (row[i] === undefined) throw new Error("结果值不完整，无法复制");
      return field(row[i], mode === "spreadsheet_safe");
    }).join("\t"));
    if (mode !== "rows") lines.unshift(snapshot.columns.map(c => field(c.name, false)).join("\t"));
    text = lines.join("\r\n");
  }
  if (new TextEncoder().encode(text).byteLength > CLIPBOARD_LIMIT) throw new Error("复制内容超过 10 MiB，请使用导出；内容未截断");
  return text;
}
