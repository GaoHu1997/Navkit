/**
 * SQL 语句类别（历史日志的类型筛选）。
 *
 * 按词法扫描得到语句顶层的关键字：跳过行/块注释与字符串字面量（'…' "…" `…` $tag$…$tag$），
 * 并统计括号深度，避免把 CTE 子查询或字符串里的 SELECT/DELETE 当成整条语句的类型。
 * WITH 语句取 CTE 列表之后真正生效的动词；多语句各自归类后去重。
 */

export type SqlKind = "query" | "insert" | "update" | "delete" | "ddl" | "dcl" | "tcl" | "call" | "other";

export const SQL_KIND_LABELS: Record<SqlKind, string> = {
  query: "查询（DQL）",
  insert: "新增（DML）",
  update: "修改（DML）",
  delete: "删除（DML）",
  ddl: "结构变更（DDL）",
  dcl: "权限（DCL）",
  tcl: "事务（TCL）",
  call: "例程调用（CALL）",
  other: "其它",
};

/** 类型筛选下拉选项（"all" 表示不筛选）。 */
export const SQL_KIND_OPTIONS: { value: SqlKind | "all"; label: string }[] = [
  { value: "all", label: "全部类型" },
  { value: "query", label: SQL_KIND_LABELS.query },
  { value: "insert", label: SQL_KIND_LABELS.insert },
  { value: "update", label: SQL_KIND_LABELS.update },
  { value: "delete", label: SQL_KIND_LABELS.delete },
  { value: "ddl", label: SQL_KIND_LABELS.ddl },
  { value: "dcl", label: SQL_KIND_LABELS.dcl },
  { value: "tcl", label: SQL_KIND_LABELS.tcl },
  { value: "call", label: SQL_KIND_LABELS.call },
  { value: "other", label: SQL_KIND_LABELS.other },
];

const KEYWORD_KINDS: Record<string, SqlKind> = {
  SELECT: "query", SHOW: "query", DESCRIBE: "query", DESC: "query", EXPLAIN: "query",
  ANALYZE: "query", PRAGMA: "query", TABLE: "query", VALUES: "query", USE: "query", SET: "query",
  INSERT: "insert", REPLACE: "insert", UPDATE: "update", MERGE: "update", UPSERT: "update",
  DELETE: "delete",
  CREATE: "ddl", ALTER: "ddl", DROP: "ddl", TRUNCATE: "ddl", RENAME: "ddl", COMMENT: "ddl",
  VACUUM: "ddl", REINDEX: "ddl", ATTACH: "ddl", DETACH: "ddl",
  GRANT: "dcl", REVOKE: "dcl",
  BEGIN: "tcl", START: "tcl", COMMIT: "tcl", ROLLBACK: "tcl", SAVEPOINT: "tcl", RELEASE: "tcl",
  CALL: "call", EXEC: "call", EXECUTE: "call", DO: "call",
};

/**
 * 词法清理：把注释替换为空格、原样保留字符串字面量与代码。
 * 字符串内的 --、/*、关键字因此不会被误判。未闭合的引号按剩余文本处理。
 */
export function stripComments(sql: string): string {
  let out = "";
  let i = 0;
  const n = sql.length;
  while (i < n) {
    const ch = sql[i];
    const next = sql[i + 1];
    if (ch === "-" && next === "-") {
      const end = sql.indexOf("\n", i + 2); i = end < 0 ? n : end + 1; out += " "; continue;
    }
    if (ch === "/" && next === "*") {
      const end = sql.indexOf("*/", i + 2); i = end < 0 ? n : end + 2; out += " "; continue;
    }
    if (ch === "'" || ch === '"' || ch === "`") {
      const quote = ch; let j = i + 1;
      while (j < n) {
        if (sql[j] === "\\") { j += 2; continue; }
        if (sql[j] === quote) { if (sql[j + 1] === quote) { j += 2; continue; } break; }
        j++;
      }
      out += sql.slice(i, Math.min(n, j + 1)); i = j + 1; continue;
    }
    if (ch === "$") {
      const tag = /^\$(?:[A-Za-z_]\w*)?\$/.exec(sql.slice(i));
      if (tag) {
        const close = sql.indexOf(tag[0], i + tag[0].length);
        const end = close < 0 ? n : close + tag[0].length;
        out += sql.slice(i, end); i = end; continue;
      }
    }
    out += ch; i++;
  }
  return out;
}

/** 首个有效关键字（大写）；空串或纯注释返回空串。 */
function firstKeyword(sql: string): string {
  const match = /^[A-Za-z_]+/.exec(stripComments(sql).trimStart());
  return (match?.[0] ?? "").toUpperCase();
}

/** 扫描器：在已清理文本上按字符前进，提供读词 / 跳空白 / 跳括号。 */
function scanner(text: string) {
  let i = 0;
  const n = text.length;
  const isWordChar = (c: string) => /[A-Za-z0-9_$]/.test(c);
  const skipWs = () => { while (i < n && /\s/.test(text[i])) i++; };
  const at = () => i;
  const to = (j: number) => { i = j; };
  const readWord = () => { skipWs(); const st = i; while (i < n && isWordChar(text[i])) i++; return text.slice(st, i); };
  const peekWord = () => { const st = i; const w = readWord(); i = st; return w; };
  const skipParen = () => {
    skipWs(); if (text[i] !== "(") return false;
    let depth = 0;
    for (; i < n; i++) {
      const c = text[i];
      if (c === "(") depth++;
      else if (c === ")") { depth--; if (depth === 0) { i++; return true; } }
    }
    return true;
  };
  return { skipWs, readWord, peekWord, skipParen, at, to };
}

/** WITH … 取 CTE 列表之后的顶层动词。 */
function withVerb(text: string): string {
  const s = scanner(text);
  s.readWord(); // WITH
  if (s.peekWord().toUpperCase() === "RECURSIVE") s.readWord();
  for (;;) {
    s.skipWs();
    if (s.at() >= text.length) return "SELECT";
    const name = s.readWord();
    if (!name) return "SELECT";
    s.skipWs();
    if (text[s.at()] === "(") { s.skipParen(); s.skipWs(); } // 可选列名列表
    const as = s.readWord().toUpperCase();
    if (as !== "AS") return as || "SELECT";
    s.skipWs();
    if (s.peekWord().toUpperCase() === "NOT") { s.readWord(); s.readWord(); }
    else if (s.peekWord().toUpperCase() === "MATERIALIZED") { s.readWord(); }
    s.skipParen();
    s.skipWs();
    if (text[s.at()] === ",") { s.to(s.at() + 1); continue; }
    break;
  }
  s.skipWs();
  return s.readWord().toUpperCase() || "SELECT";
}

/**
 * 归类单条 SQL。多语句请用 classifySqlKinds（本函数只按整段首个顶层动词归类）。
 */
export function classifySql(sql: string): SqlKind {
  const text = stripComments(sql).trimStart();
  const head = firstKeyword(sql);
  if (!head) return "other";
  if (head === "WITH") return KEYWORD_KINDS[withVerb(text)] ?? "other";
  return KEYWORD_KINDS[head] ?? "other";
}

/** 按顶层分号拆分语句（忽略字符串、注释与括号内的分号）。 */
export function splitStatements(sql: string): string[] {
  const text = stripComments(sql);
  const n = text.length;
  const result: string[] = [];
  let depth = 0;
  let start = 0;
  let i = 0;
  while (i < n) {
    const c = text[i];
    if (c === "'" || c === '"' || c === "`") {
      const quote = c; i++;
      while (i < n) {
        if (text[i] === "\\") { i += 2; continue; }
        if (text[i] === quote) { if (text[i + 1] === quote) { i += 2; continue; } i++; break; }
        i++;
      }
      continue;
    }
    if (c === "$") {
      const tag = /^\$(?:[A-Za-z_]\w*)?\$/.exec(text.slice(i));
      if (tag) { const close = text.indexOf(tag[0], i + tag[0].length); i = close < 0 ? n : close + tag[0].length; continue; }
    }
    if (c === "(") depth++;
    else if (c === ")") { if (depth > 0) depth--; }
    else if (c === ";" && depth === 0) {
      const part = text.slice(start, i);
      if (part.trim()) result.push(part);
      start = i + 1;
    }
    i++;
  }
  const tail = text.slice(start);
  if (tail.trim()) result.push(tail);
  return result;
}

/** 一条记录可能包含多条语句：返回去重后的类型集合（顺序按出现先后）。 */
export function classifySqlKinds(sql: string): SqlKind[] {
  const parts = splitStatements(sql);
  const kinds: SqlKind[] = [];
  for (const part of parts.length ? parts : [sql]) {
    const kind = classifySql(part);
    if (!kinds.includes(kind)) kinds.push(kind);
  }
  return kinds.length ? kinds : ["other"];
}
