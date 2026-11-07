/** 单表 SELECT 构造：仅使用真实元数据白名单，不接受 SQL 表达式。 */
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
export type QueryFilterOperator = "=" | "<>" | ">" | ">=" | "<" | "<=" | "LIKE" | "IS NULL" | "IS NOT NULL";
export interface QueryFilter { column: string; operator: QueryFilterOperator; value: string; valueKind: "text" | "number" | "boolean"; }
export interface QueryBuilderSpec {
  dialect: QueryDialect; schema: string; table: string; columns: string[]; filters: QueryFilter[];
  orderBy: string; descending: boolean; limit: number;
}
export const MAX_QUERY_LIMIT = 100000;
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
  if (spec.filters.length > 20) throw new Error("最多支持 20 个 AND 过滤条件。");
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
  const filters = spec.filters.map(filter => {
    const field = column(filter.column);
    if (!["=", "<>", ">", ">=", "<", "<=", "LIKE", "IS NULL", "IS NOT NULL"].includes(filter.operator)) throw new Error("不支持的过滤运算符。");
    if (["IS NULL", "IS NOT NULL"].includes(filter.operator)) return `${field} ${filter.operator}`;
    let value: string;
    if (filter.valueKind === "number") {
      value = filter.value.trim();
      if (!/^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$/.test(value) || !Number.isFinite(Number(value))) throw new Error(`「${filter.column}」的过滤值必须是有限数字。`);
    } else if (filter.valueKind === "boolean") {
      if (!["true", "false"].includes(filter.value.toLowerCase())) throw new Error("布尔值请填写 true 或 false。");
      const isTrue = filter.value.toLowerCase() === "true";
      value = spec.dialect === "access" ? (isTrue ? "-1" : "0") : ["mssql", "oracle", "sqlite"].includes(spec.dialect) ? (isTrue ? "1" : "0") : (isTrue ? "TRUE" : "FALSE");
    } else if (filter.valueKind === "text") value = textLiteral(filter.value);
    else throw new Error("不支持的过滤值类型。");
    if (filter.operator === "LIKE" && filter.valueKind !== "text") throw new Error("LIKE 过滤请使用文本值类型（% / _ 为通配符）。");
    return `${field} ${filter.operator} ${value}`;
  });
  const top = ["mssql", "access"].includes(spec.dialect) ? `TOP ${spec.limit} ` : spec.dialect === "firebird" ? `FIRST ${spec.limit} ` : "";
  const owner = spec.schema && !["access", "firebird"].includes(spec.dialect) ? `${identifier(spec.schema)}.` : "";
  let sql = `SELECT ${top}${spec.columns.map(column).join(", ")}\nFROM ${owner}${identifier(spec.table)}`;
  if (filters.length) sql += `\nWHERE ${filters.join("\n  AND ")}`;
  if (spec.orderBy) sql += `\nORDER BY ${column(spec.orderBy)} ${spec.descending ? "DESC" : "ASC"}`;
  if (["oracle", "db2", "ansi"].includes(spec.dialect)) sql += `\nFETCH FIRST ${spec.limit} ROWS ONLY`;
  else if (!["mssql", "access", "firebird"].includes(spec.dialect)) sql += `\nLIMIT ${spec.limit}`;
  return `${sql};`;
}
