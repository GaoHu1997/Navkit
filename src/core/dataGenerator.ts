/** 数据生成器纯逻辑：预览就是将写入的值，不生成/拼接 SQL。 */
import type { ColumnInfo, TableSchema } from "./types";
export const MAX_GENERATED_ROWS = 500;
export type GeneratorMode = "auto" | "sequence" | "constant" | "null" | "omit";
export interface GeneratorField { name: string; mode: GeneratorMode; value: string; }
export interface GeneratedRowsRequest {
  connection_id: string;
  schema: string;
  table: string;
  columns: string[];
  rows: (string | null)[][];
}
export interface GeneratorPlan { columns: string[]; rows: (string | null)[][]; }
export type ValueKind = "integer" | "number" | "boolean" | "date" | "datetime" | "time" | "uuid" | "json" | "text" | "enum" | "unsupported";
export function generatorValueKind(column: ColumnInfo): ValueKind {
  const t = column.type.toLowerCase().trim();
  if (/^(bool|boolean|bit)(\b|\()/.test(t)) return "boolean";
  if (/\b(uuid|uniqueidentifier)\b/.test(t)) return "uuid";
  if (/^enum\s*\(/.test(t)) return "enum";
  if (/\b(json|jsonb)\b/.test(t)) return "json";
  if (/\b(datetime|timestamp|timestamptz)\b/.test(t)) return "datetime";
  if (/^date\b/.test(t)) return "date";
  if (/^time\b/.test(t)) return "time";
  if (/\b(tinyint|smallint|mediumint|int|integer|bigint|int2|int4|int8|serial|bigserial|smallserial)\b/.test(t)) return "integer";
  if (/\b(decimal|numeric|number|real|float|double|money)\b/.test(t)) return "number";
  if (!t || /\b(char|varchar|nvarchar|nchar|text|tinytext|mediumtext|longtext|clob|string|citext)\b/.test(t)) return "text";
  return "unsupported";
}
export function defaultGeneratorFields(schema: TableSchema): GeneratorField[] {
  const foreign = new Set(schema.foreign_keys.flatMap(f => f.columns));
  return schema.columns.map(c => ({ name: c.name,
    mode: c.auto_increment || c.default !== null ? "omit" : foreign.has(c.name) || generatorValueKind(c) === "unsupported" ? "constant" : c.primary_key && generatorValueKind(c) === "integer" ? "sequence" : "auto",
    value: c.primary_key && generatorValueKind(c) === "integer" ? "1" : "",
  }));
}
function numericBounds(c: ColumnInfo): [bigint, bigint] {
  const t = c.type.toLowerCase();
  const bits = /tinyint/.test(t) ? 8 : /smallint|int2|smallserial/.test(t) ? 16 : /mediumint/.test(t) ? 24 : /bigint|int8|bigserial/.test(t) ? 64 : 32;
  return /unsigned/.test(t) ? [0n, (1n << BigInt(bits)) - 1n] : [-(1n << BigInt(bits - 1)), (1n << BigInt(bits - 1)) - 1n];
}
function enumValues(c: ColumnInfo): string[] {
  return [...c.type.matchAll(/'((?:''|[^'])*)'/g)].map(m => m[1].replace(/''/g, "'"));
}
function columnLength(c: ColumnInfo): number | null {
  return c.length ?? (/\b(?:n?varchar|n?char)\s*\((\d+)\)/i.exec(c.type)?.[1] ? Number(/\((\d+)\)/.exec(c.type)![1]) : null);
}
function validateValue(c: ColumnInfo, value: string | null): void {
  if (value === null) { if (!c.nullable || c.primary_key) throw new Error(`${c.name} 不允许 NULL`); return; }
  const kind = generatorValueKind(c), max = columnLength(c);
  if (max !== null && [...value].length > max && ["text", "enum"].includes(kind)) throw new Error(`${c.name} 超出字段长度 ${max}`);
  if (kind === "integer") {
    if (!/^[+-]?\d+$/.test(value)) throw new Error(`${c.name} 必须是整数`);
    const integer = BigInt(value), [min, max] = numericBounds(c);
    if (integer < min || integer > max) throw new Error(`${c.name} 的整数超出类型范围`);
  }
  if (kind === "number" && (!/^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$/.test(value) || !Number.isFinite(Number(value)))) throw new Error(`${c.name} 必须是有限数值`);
  if (kind === "number" && c.precision !== null) {
    const [whole, decimal = ""] = value.replace(/^[+-]/, "").split(".");
    if ((whole.replace(/^0+/, "").length || 1) > c.precision - (c.scale ?? 0) || decimal.length > (c.scale ?? 0)) throw new Error(`${c.name} 超出精度 ${c.precision}/${c.scale ?? 0}`);
  }
  if (kind === "boolean" && !/^(0|1|true|false)$/i.test(value)) throw new Error(`${c.name} 必须为 0/1 或 true/false`);
  if (kind === "uuid" && !/^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(value)) throw new Error(`${c.name} 必须为 UUID`);
  if (kind === "json") { try { JSON.parse(value); } catch { throw new Error(`${c.name} 必须为有效 JSON`); } }
  if (kind === "enum" && !enumValues(c).includes(value)) throw new Error(`${c.name} 必须选用类型定义中的枚举值`);
  if (kind === "date") {
    const parsed = new Date(`${value}T00:00:00Z`);
    if (!/^\d{4}-\d{2}-\d{2}$/.test(value) || !Number.isFinite(parsed.getTime()) || parsed.toISOString().slice(0, 10) !== value) throw new Error(`${c.name} 必须为有效日期 YYYY-MM-DD`);
  }
  if (kind === "datetime") {
    const match = /^(\d{4}-\d{2}-\d{2})[ T]([01]\d|2[0-3]):[0-5]\d:[0-5]\d(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})?$/.exec(value);
    const parsed = new Date(value.replace(" ", "T"));
    const date = match ? new Date(`${match[1]}T00:00:00Z`) : null;
    if (!match || !Number.isFinite(parsed.getTime()) || !date || !Number.isFinite(date.getTime()) || date.toISOString().slice(0, 10) !== match[1]) throw new Error(`${c.name} 必须为有效日期时间`);
  }
  if (kind === "time" && !/^([01]\d|2[0-3]):[0-5]\d:[0-5]\d$/.test(value)) throw new Error(`${c.name} 必须为时间 HH:mm:ss`);
}
function autoValue(c: ColumnInfo, index: number, seed: string, now: Date): string {
  switch (generatorValueKind(c)) {
    case "integer": return String(index + 1);
    case "number": return c.scale && c.scale > 0 ? ((index + 1) / 10).toFixed(Math.min(c.scale, 8)) : String(index + 1);
    case "boolean": return index % 2 ? "1" : "0";
    case "date": return new Date(now.getTime() + index * 86400000).toISOString().slice(0, 10);
    case "datetime": return new Date(now.getTime() + index * 1000).toISOString().slice(0, 19).replace("T", " ");
    case "time": return new Date(now.getTime() + index * 1000).toISOString().slice(11, 19);
    case "uuid": return crypto.randomUUID();
    case "json": return JSON.stringify({ generated: true, index: index + 1 });
    case "enum": { const values = enumValues(c); if (!values.length) throw new Error(`${c.name} 无可解析的枚举值，请填写固定值`); return values[index % values.length]; }
    case "text": {
      const max = columnLength(c); const text = `${seed}_${index + 1}_${c.name}`;
      // 保留序号以避免窄 varchar 主键全部截断为同一前缀。
      return max !== null ? `${(index + 1).toString(36)}_${seed}_${c.name}`.slice(0, max) : text;
    }
    default: throw new Error(`${c.name} 类型 ${c.type} 不支持自动生成，请使用固定值、NULL 或数据库默认值`);
  }
}
export function generateDataPreview(schema: TableSchema, fields: GeneratorField[], count: number, seed = "sample", now = new Date()): GeneratorPlan {
  if (!Number.isInteger(count) || count < 1 || count > MAX_GENERATED_ROWS) throw new Error(`生成条数必须为 1–${MAX_GENERATED_ROWS} 的整数`);
  if (!schema.columns.length) throw new Error("未读取到表字段，不能生成数据");
  const byName = new Map(fields.map(f => [f.name, f]));
  if (byName.size !== fields.length || fields.some(f => !schema.columns.some(c => c.name === f.name))) throw new Error("生成字段重复或不属于该表");
  const chosen = schema.columns.filter(c => byName.get(c.name)?.mode !== "omit");
  for (const c of schema.columns) {
    const f = byName.get(c.name);
    if (!f) throw new Error(`缺少 ${c.name} 的生成设置`);
    if (c.auto_increment && f.mode !== "omit") throw new Error(`${c.name} 为自增/标识字段，应交给数据库生成`);
    if (f.mode === "omit" && !c.auto_increment && c.default === null && (!c.nullable || c.primary_key)) throw new Error(`${c.name} 为必填字段，不能省略`);
    if (f.mode === "constant" && f.value === "" && (!c.nullable || c.primary_key) && generatorValueKind(c) !== "text") throw new Error(`请填写 ${c.name} 的固定值（外键须使用真实已有引用）`);
  }
  const rows = Array.from({ length: count }, (_, index) => chosen.map(c => {
    const f = byName.get(c.name)!;
    let value: string | null;
    if (f.mode === "null") value = null;
    else if (f.mode === "constant") value = f.value;
    else if (f.mode === "sequence") {
      if (generatorValueKind(c) !== "integer" || !/^[+-]?\d+$/.test(f.value)) throw new Error(`${c.name} 的顺序生成起始值必须为整数`);
      value = (BigInt(f.value) + BigInt(index)).toString();
    } else value = autoValue(c, index, seed, now);
    validateValue(c, value); return value;
  }));
  // 在批次内验证全部已提供主键，不假定能避开表内既有键；后端约束/事务最终保障。
  const primary = schema.columns.filter(c => c.primary_key);
  if (primary.length && primary.every(c => chosen.some(col => col.name === c.name))) {
    const indexes = primary.map(c => chosen.findIndex(col => col.name === c.name)), seen = new Set<string>();
    for (const row of rows) {
      const key = JSON.stringify(indexes.map(i => row[i]));
      if (seen.has(key)) throw new Error("预览数据包含重复主键，请修改生成方式或起始值"); seen.add(key);
    }
  }
  return { columns: chosen.map(c => c.name), rows };
}
