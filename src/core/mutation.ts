/** 图形化写入值适配：整数与精确小数始终保留十进制文字。 */
import type { DbValue } from "./api";

const INTEGER_TYPES = /^(tinyint|smallint|mediumint|int|integer|bigint|serial|bigserial|smallserial|int2|int4|int8)( unsigned)?$/i;
const DECIMAL_TYPES = /^(numeric|decimal|number|money|smallmoney)$/i;
const FLOAT_TYPES = /^(float|double|double precision|real|float4|float8)$/i;
const DECIMAL_TEXT = /^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?$/i;
const BINARY_TYPES = /^(binary|varbinary|longvarbinary|blob|bytea|image)$/i;

/** 类型不明时保留文本；已知数值类型的非法值必须拒绝，不能默默当作文本或零。 */
export function toDbValue(raw: string | null | undefined, dataType: string): DbValue {
  if (raw === null || raw === undefined) return { kind: "null" };
  const type = dataType.replace(/\([^)]*\)/g, "").trim().replace(/\s+/g, " ").toLowerCase();
  const value = raw.trim();
  if (INTEGER_TYPES.test(type)) {
    if (!/^[+-]?\d+$/.test(value)) throw new Error("整数必须是完整的十进制文字，空字符串不是 0");
    const n = BigInt(value);
    const min = type.endsWith(" unsigned") ? 0n : -(1n << 63n);
    const max = type.endsWith(" unsigned") ? (1n << 64n) - 1n : (1n << 63n) - 1n;
    if (n < min || n > max) throw new Error("整数超出目标 64 位范围");
    return { kind: "integer", value };
  }
  if (DECIMAL_TYPES.test(type)) {
    if (!DECIMAL_TEXT.test(value) || value.length > 1024) throw new Error("精确小数必须是完整十进制文字");
    const exponent = /e([+-]?\d+)$/i.exec(value)?.[1];
    if (exponent && Math.abs(Number(exponent)) > 1000) throw new Error("十进制指数超过安全范围");
    return { kind: "decimal", value };
  }
  if (FLOAT_TYPES.test(type)) {
    if (!DECIMAL_TEXT.test(value)) throw new Error("浮点值必须是有限的十进制数字");
    const n = Number(value);
    if (!Number.isFinite(n)) throw new Error("浮点值超出范围");
    return { kind: "float", value: n };
  }
  if (/^(bool|boolean)$/i.test(type)) {
    if (/^(1|true|t)$/i.test(value)) return { kind: "bool", value: true };
    if (/^(0|false|f)$/i.test(value)) return { kind: "bool", value: false };
    throw new Error("布尔值必须是 true/false 或 1/0");
  }
  if (BINARY_TYPES.test(type)) {
    if (!/^0x(?:[0-9a-f]{2})*$/i.test(raw)) throw new Error("二进制编辑必须使用明确的 0x 十六进制，不能回写显示文本");
    const bytes = new Uint8Array((raw.length - 2) / 2);
    for (let i = 0; i < bytes.length; i++) bytes[i] = parseInt(raw.slice(2 + i * 2, 4 + i * 2), 16);
    // 分块，避免大值展开超出 JS 参数/栈上限。
    let binary = "";
    for (let offset = 0; offset < bytes.length; offset += 8192) binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
    return { kind: "binary", value: btoa(binary) };
  }
  if (/^date$/i.test(type)) return { kind: "date", value };
  if (/^time(?: without time zone)?$/i.test(type)) return { kind: "time", value };
  if (/^(timestamp|datetime|datetime2|timestamp without time zone)$/i.test(type)) return { kind: "timestamp", value };
  if (/^(timestamptz|timestamp with time zone|datetimeoffset)$/i.test(type)) {
    if (!/T.*(?:Z|[+-]\d{2}:\d{2})$/i.test(value)) throw new Error("带时区时间戳必须保留明确的 RFC 3339 时区");
    return { kind: "timestamptz", value };
  }
  if (/^uuid$/i.test(type)) return { kind: "uuid", value };
  if (/^(json|jsonb)$/i.test(type)) { JSON.parse(raw); return { kind: "json", value: raw }; }
  return { kind: "text", value: raw };
}
