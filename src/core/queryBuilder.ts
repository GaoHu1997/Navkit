/** 单表 SELECT 构造：仅使用真实元数据白名单，不接受 SQL 表达式；支持 IN / NOT IN 列表过滤与多字段排序。 */
import { kindMeta } from "./dbcatalog";
import type { TableSchema } from "./types";

export type QueryDialect = "mysql" | "postgres" | "sqlite" | "mssql" | "oracle" | "db2" | "firebird" | "access" | "clickhouse" | "standard" | "ansi";
export const QUERY_DIALECTS: { value: QueryDialect; label: string }[] = [
  { value: "mysql", label: "MySQL / MariaDB" }, { value: "postgres", label: "PostgreSQL" },
  { value: "sqlite", label: "SQLite / libSQL" }, { value: "mssql", label: "SQL Server" },
  { value: "oracle", label: "Oracle 12c+" }, { value: "db2", label: "DB2" },
  { value: "firebird", label: "Firebird" }, { value: "access", label: "Access" },
  { value: "clickhouse", label: "ClickHouse" }, { value: "standard", label: "标准引用 + LIMIT（DuckDB / H2 / Trino）" },
  { value: "ansi", label: "ANSI（FETCH FIRST）" },
];
export function queryBuilderUnavailableReason(kind: string): string | null {
  const meta = kindMeta(kind);
  if (!meta || meta.planned) return "该数据源尚未提供可用的 SQL 表结构接口，不能使用查询创建工具。";
  if (["mysql", "postgres", "sqlite", "mssql", "clickhouse"].includes(meta.protocol) ||
      (meta.protocol === "odbc" && ["relational", "analytics", "domestic", "lightweight"].includes(meta.category)) ||
      ["h2", "rqlite", "turso", "d1", "druid", "tdengine"].includes(kind)) return null;
  return `${meta.label} 不使用本工具支持的关系 SQL 表结构 / SELECT 方言，请在查询编辑器使用该数据源的原生命令。`;
}
export function defaultQueryDialect(kind: string): QueryDialect | "" {
  const protocol = kindMeta(kind)?.protocol;
  if (["mysql", "postgres", "sqlite", "mssql", "clickhouse"].includes(protocol ?? "")) return protocol as QueryDialect;
  if (["rqlite", "turso", "d1"].includes(kind)) return "sqlite";
  if (["oracle", "yashandb", "dameng"].includes(kind)) return "oracle";
  if (["db2", "firebird", "access"].includes(kind)) return kind as QueryDialect;
  if (kind === "tdengine") return "mysql";
  if (["duckdb", "vertica", "snowflake", "trino", "prestosql", "druid", "h2"].includes(kind)) return "standard";
  // 通用 ODBC / JDBC 桥接不知道目标方言，必须由用户明确选择。
  return "";
}
export type QueryFilterOperator = "=" | "<>" | ">" | ">=" | "<" | "<=" | "LIKE" | "NOT LIKE" | "IN" | "NOT IN" | "IS NULL" | "IS NOT NULL";
export interface QueryFilter {
  column: string;
  operator: QueryFilterOperator;
  value: string;
  valueKind: "text" | "number" | "boolean";
  /** 与上一条条件的连接方式；首条忽略，缺省 AND。 */
  join?: "AND" | "OR";
}
export interface QueryFilterGroup { join?: "AND" | "OR"; filters: QueryFilter[] }
export interface QueryBuilderSpec {
  dialect: QueryDialect; schema: string; table: string; columns: string[]; filters: QueryFilter[];
  orderBy: string; descending: boolean; limit: number;
  /** 多字段排序（可选）；提供非空数组时优先于 orderBy / descending，旧字段保持兼容。 */
  orderByRules?: { column: string; descending: boolean }[];
  /** 与表筛选一致：行内括号条件组，组间 AND / OR。提供时优先于平铺 filters。 */
  filterGroups?: QueryFilterGroup[];
}
export const MAX_QUERY_LIMIT = 100000;

/** 有限数字字面量：十进制，可带符号 / 小数点 / 指数。 */
const NUMBER_PATTERN = /^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$/;

export function buildSelectQuery(structure: TableSchema, spec: QueryBuilderSpec): string {
  if (!QUERY_DIALECTS.some(d => d.value === spec.dialect)) throw new Error("请选择目标数据库的 SQL 方言。");
  if (!spec.table || spec.table !== structure.table) throw new Error("目标表结构已改变，请重新加载。");
  if (!Number.isSafeInteger(spec.limit) || spec.limit < 1 || spec.limit > MAX_QUERY_LIMIT) throw new Error(`最大行数必须是 1–${MAX_QUERY_LIMIT} 的整数。`);
  const names = new Set(structure.columns.map(c => c.name));
  const column = (name: string) => { if (!names.has(name)) throw new Error(`字段「${name}」不在当前表结构中。`); return identifier(name); };
  const identifier = (name: string) => {
    if (!name || /[\u0000-\u001f]/.test(name)) throw new Error("对象名称为空或包含控制字符。");
    if (["mysql", "clickhouse"].includes(spec.dialect)) return `\`${name.replace(/`/g, "``")}\``;
    if (["mssql", "access"].includes(spec.dialect)) return `[${name.replace(/\]/g, "]]")}]`;
    return `"${name.replace(/"/g, '""')}"`;
  };
  if (!spec.columns.length) throw new Error("请至少选择一个输出字段。");
  if (new Set(spec.columns).size !== spec.columns.length) throw new Error("输出字段不能重复。");
  const filters = spec.filterGroups ? spec.filterGroups.flatMap(group => group.filters) : spec.filters;
  if (filters.length > 20) throw new Error("最多支持 20 个过滤条件。");
  const join = (value: string | undefined): "AND" | "OR" => {
    if (value !== undefined && value !== "AND" && value !== "OR") throw new Error("不支持的条件连接方式。");
    return value ?? "AND";
  };
  for (const group of spec.filterGroups ?? []) {
    join(group.join);
    if (!group.filters.length) throw new Error("条件组不能为空。");
  }
  for (const filter of filters) join(filter.join);
  const textLiteral = (value: string) => {
    if (value.length > 8192 || value.includes("\0")) throw new Error("过滤值不能包含 NUL，且不能超过 8192 字符。");
    // MySQL 的 NO_BACKSLASH_ESCAPES 与 PostgreSQL 的 standard_conforming_strings
    // 可能不同：采用模式无关的 MySQL UTF-8 十六进制和 PostgreSQL 显式 E 字符串。
    if (["mysql", "clickhouse"].includes(spec.dialect)) {
      const hex = Array.from(new TextEncoder().encode(value), b => b.toString(16).padStart(2, "0")).join("");
      return spec.dialect === "mysql" ? `CONVERT(X'${hex}' USING utf8mb4)` : `unhex('${hex}')`;
    }
    if (["standard", "ansi"].includes(spec.dialect) && value.includes("\\")) throw new Error("标准驱动的反斜杠转义规则未知，请选择明确方言或去掉该字符。");
    const escaped = value.replace(/'/g, "''");
    if (spec.dialect === "postgres") return `E'${escaped.replace(/\\/g, "\\\\")}'`;
    return `${spec.dialect === "mssql" ? "N" : ""}'${escaped}'`;
  };
  const numberLiteral = (value: string, name: string): string => {
    const raw = value.trim();
    if (!NUMBER_PATTERN.test(raw) || !Number.isFinite(Number(raw))) throw new Error(`「${name}」的过滤值必须是有限数字。`);
    return raw;
  };
  const booleanLiteral = (value: string): string => {
    if (!["true", "false"].includes(value.toLowerCase())) throw new Error("布尔值请填写 true 或 false。");
    const isTrue = value.toLowerCase() === "true";
    return spec.dialect === "access" ? (isTrue ? "-1" : "0") : ["mssql", "oracle", "sqlite"].includes(spec.dialect) ? (isTrue ? "1" : "0") : (isTrue ? "TRUE" : "FALSE");
  };
  // IN / NOT IN：值为逗号分隔列表，逐项按 valueKind 生成字面量（文本加引号、数字 / 布尔不加）。
  const inList = (field: string, filter: QueryFilter): string => {
    const items = filter.value.split(",").map(item => item.trim()).filter(item => item.length > 0);
    if (!items.length) throw new Error(`「${filter.column}」的 ${filter.operator} 过滤请填写至少一个值（逗号分隔）。`);
    const literals = items.map(item => {
      if (filter.valueKind === "number") return numberLiteral(item, filter.column);
      if (filter.valueKind === "boolean") return booleanLiteral(item);
      return textLiteral(item);
    });
    return `${field} ${filter.operator} (${literals.join(", ")})`;
  };
  const expression = (filter: QueryFilter): string => {
    const field = column(filter.column);
    if (!["=", "<>", ">", ">=", "<", "<=", "LIKE", "NOT LIKE", "IN", "NOT IN", "IS NULL", "IS NOT NULL"].includes(filter.operator)) throw new Error("不支持的过滤运算符。");
    if (["IS NULL", "IS NOT NULL"].includes(filter.operator)) return `${field} ${filter.operator}`;
    if (["IN", "NOT IN"].includes(filter.operator)) return `${inList(field, filter)}`;
    let value: string;
    if (filter.valueKind === "number") value = numberLiteral(filter.value, filter.column);
    else if (filter.valueKind === "boolean") value = booleanLiteral(filter.value);
    else if (filter.valueKind === "text") value = textLiteral(filter.value);
    else throw new Error("不支持的过滤值类型。");
    if (["LIKE", "NOT LIKE"].includes(filter.operator) && filter.valueKind !== "text") throw new Error("LIKE 过滤请使用文本值类型（% / _ 为通配符）。");
    return `${field} ${filter.operator} ${value}`;
  };
  const flatConditions = (items: QueryFilter[]) => items.map((filter, index) => (index ? join(filter.join) + " " : "") + expression(filter));
  const whereParts = spec.filterGroups
    ? spec.filterGroups.map((group, index) => (index ? join(group.join) + " " : "") + "(" + flatConditions(group.filters).join(" ") + ")")
    : flatConditions(spec.filters);
  const top = ["mssql", "access"].includes(spec.dialect) ? `TOP ${spec.limit} ` : spec.dialect === "firebird" ? `FIRST ${spec.limit} ` : "";
  const owner = spec.schema && !["access", "firebird"].includes(spec.dialect) ? `${identifier(spec.schema)}.` : "";
  let sql = `SELECT ${top}${spec.columns.map(column).join(", ")}\nFROM ${owner}${identifier(spec.table)}`;
  if (whereParts.length) sql += `\nWHERE ${whereParts.join("\n  ")}`;
  // 多字段排序优先；未提供时回退到旧的单字段 orderBy / descending。
  const orderRules = spec.orderByRules?.length ? spec.orderByRules : spec.orderBy ? [{ column: spec.orderBy, descending: spec.descending }] : [];
  if (orderRules.length) {
    const seen = new Set<string>();
    const parts = orderRules.map(rule => {
      if (seen.has(rule.column)) throw new Error(`排序字段「${rule.column}」重复。`);
      seen.add(rule.column);
      return `${column(rule.column)} ${rule.descending ? "DESC" : "ASC"}`;
    });
    sql += `\nORDER BY ${parts.join(", ")}`;
  }
  if (["oracle", "db2", "ansi"].includes(spec.dialect)) sql += `\nFETCH FIRST ${spec.limit} ROWS ONLY`;
  else if (!["mssql", "access", "firebird"].includes(spec.dialect)) sql += `\nLIMIT ${spec.limit}`;
  return `${sql};`;
}
