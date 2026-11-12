/** Lightweight, dependency-free SQL completion analysis. Only real metadata is suggested. */
export interface SqlCompletionItem {
  label: string;
  type: string;
  /** 名称之后的行内灰字：表备注，或字段的“类型 · 备注”。没有就整段不出现。 */
  detail?: string;
  apply?: string;
  /** 应用后在插入文本里的光标位置；函数补全把光标放进括号。 */
  cursor?: number;
  boost?: number;
}

/** 补全附带的关系元数据；只包含真实读取到的值。 */
export interface SqlCompletionRelationMeta {
  kind: "table" | "view";
  comment: string | null;
  /** 列读取是否成功；失败时保留表候选但不显示字段。 */
  columns_known: boolean;
}

/** 补全附带的字段元数据。 */
export interface SqlCompletionColumnMeta {
  type: string | null;
  comment: string | null;
}

/** 备注与类型投影；以 __meta__ 键挂在原有“对象名 → 列名”映射上，旧数据仍可直接使用。 */
export interface SqlCompletionMeta {
  version: number;
  schema: string;
  relations: Record<string, SqlCompletionRelationMeta>;
  columns: Record<string, Record<string, SqlCompletionColumnMeta>>;
}

export type SqlCompletionData = Record<string, string[]> & { __meta__?: SqlCompletionMeta };
export interface SqlCompletionAnalysis {
  from: number;
  to: number;
  options: SqlCompletionItem[];
  validFor: RegExp;
}
interface Token { value: string; from: number; to: number; kind: "identifier" | "symbol" | "ignored"; quote?: string; closed?: boolean; depth: number }
const identifierChar = /[\p{L}\p{N}_$]/u;
const reserved = new Set("SELECT FROM JOIN ON USING WHERE GROUP ORDER BY HAVING LIMIT OFFSET UNION EXCEPT INTERSECT INNER LEFT RIGHT FULL CROSS OUTER NATURAL AS SET VALUES RETURNING AND OR WHEN THEN ELSE END FOR INTO UPDATE DELETE INSERT TABLE".split(" "));

/** Keep offsets and statement delimiters intact; never interpret strings/comments as relations. */
function tokens(sql: string): Token[] {
  const result: Token[] = [];
  let depth = 0;
  for (let i = 0; i < sql.length;) {
    const from = i;
    const ch = sql[i];
    if (/\s/.test(ch)) { i++; continue; }
    if (sql.startsWith("--", i) || sql.startsWith("/*", i) || ch === "'") {
      if (sql.startsWith("--", i)) { const end = sql.indexOf("\n", i); i = end < 0 ? sql.length : end; }
      else if (sql.startsWith("/*", i)) { const end = sql.indexOf("*/", i + 2); i = end < 0 ? sql.length : end + 2; }
      else {
        i++;
        while (i < sql.length) {
          if (sql[i] === "\\") { i += 2; continue; }
          if (sql[i++] === "'") { if (sql[i] === "'") i++; else break; }
        }
      }
      result.push({ value: "", from, to: Math.min(i, sql.length), kind: "ignored", depth });
      continue;
    }
    const dollar = sql.slice(i).match(/^\$(?:[A-Za-z_]\w*)?\$/)?.[0];
    if (dollar) {
      const end = sql.indexOf(dollar, i + dollar.length);
      i = end < 0 ? sql.length : end + dollar.length;
      result.push({ value: "", from, to: i, kind: "ignored", depth });
      continue;
    }
    if (ch === '"' || ch === "`" || ch === "[") {
      const close = ch === "[" ? "]" : ch;
      let value = "", closed = false;
      i++;
      while (i < sql.length) {
        if (sql[i] === close) {
          if (sql[i + 1] === close) { value += close; i += 2; }
          else { i++; closed = true; break; }
        } else value += sql[i++];
      }
      result.push({ value, from, to: i, quote: ch, closed, kind: "identifier", depth });
      continue;
    }
    if (identifierChar.test(ch)) {
      i++;
      while (i < sql.length && identifierChar.test(sql[i])) i++;
      result.push({ value: sql.slice(from, i), from, to: i, kind: "identifier", depth });
      continue;
    }
    if (ch === ")") depth = Math.max(0, depth - 1);
    result.push({ value: ch, from, to: ++i, kind: "symbol", depth });
    if (ch === "(") depth++;
  }
  return result;
}
function lastItem<T>(values: readonly T[]): T | undefined { return values[values.length - 1]; }
function normal(value: string): string { return value.toLocaleLowerCase(); }
/** 空白备注等同于没有备注；列表不显示占位文案。 */
function noteText(value: string | null | undefined): string | undefined {
  const trimmed = value?.trim();
  return trimmed ? trimmed : undefined;
}
function joinNote(parts: (string | undefined)[]): string | undefined {
  const kept = parts.filter((part): part is string => !!part);
  return kept.length ? kept.join(" · ") : undefined;
}
function metadataNames(data: SqlCompletionData): string[] {
  return [...new Set([...(data.__tables__ ?? []), ...(data.__views__ ?? []), ...Object.keys(data).filter(k => !k.startsWith("__"))])];
}
function resolveOwner(parts: string[], names: string[], schema: string): string | undefined {
  const key = normal(parts.join("."));
  const exact = names.find(name => normal(name) === key);
  if (exact) return exact;
  if (parts.length > 1 && schema && normal(parts.slice(0, -1).join(".")) !== normal(schema)) return undefined;
  const last = normal(parts[parts.length - 1] ?? "");
  const matches = names.filter(name => normal(lastItem(name.split(".")) ?? name) === last);
  return matches.length === 1 ? matches[0] : undefined;
}
function quoteIdentifier(name: string, quote: string): string {
  const close = quote === "[" ? "]" : quote;
  return quote + name.replaceAll(close, close + close) + close;
}

/** Selection existence, not trimmed text, determines execution: even a whitespace selection wins. */
export function sqlExecutionText(sql: string, from: number, to: number): string {
  return from === to ? sql : sql.slice(Math.min(from, to), Math.max(from, to));
}

export function analyzeSqlCompletion(sql: string, cursor: number, data: SqlCompletionData, keywords: readonly string[] = [], functions: readonly string[] = [], schema = "", quote = '"'): SqlCompletionAnalysis | null {
  const all = tokens(sql);
  if (all.some(t => t.kind === "ignored" && t.from < cursor && cursor <= t.to && !/\n$/.test(sql.slice(t.from, cursor)))) return null;
  // Whole statement (including FROM after a SELECT-list cursor), never aliases from earlier statements.
  const start = lastItem(all.filter(t => t.value === ";" && t.to <= cursor))?.to ?? 0;
  const end = all.find(t => t.value === ";" && t.from >= cursor)?.from ?? sql.length;
  const statement = all.filter(t => t.from >= start && t.from < end && t.kind !== "ignored");
  const tail = statement.find(t => t.kind === "identifier" && t.from < cursor && t.to >= cursor);
  const from = tail ? tail.from + (tail.quote ? 1 : 0) : cursor;
  const close = tail?.quote === "[" ? "]" : tail?.quote;
  const prefixEnd = tail?.quote && tail.closed && cursor === tail.to ? cursor - 1 : cursor;
  const rawPrefix = tail ? sql.slice(from, prefixEnd) : "";
  const prefix = close ? rawPrefix.replaceAll(close + close, close) : rawPrefix;
  const to = tail?.to ?? cursor;
  const before = statement.filter(t => t.to <= (tail?.from ?? cursor));
  const qualifier: string[] = [];
  for (let i = before.length - 1; i >= 1 && before[i].value === "." && before[i - 1].kind === "identifier"; i -= 2) qualifier.unshift(before[i - 1].value);
  const preceding = before.slice(0, before.length - qualifier.length * 2);
  const last = lastItem(preceding);
  // Function parentheses don't create SQL query scopes; subqueries do. Bound a
  // query block so aliases from sibling subqueries at the same depth never leak.
  let depth = 0, scopeFrom = start, scopeTo = end;
  const opens: Token[] = [];
  const scopes: { from: number; to: number; depth: number }[] = [];
  for (const token of statement) {
    if (token.value === "(") opens.push(token);
    else if (token.value === ")") {
      const open = opens.pop();
      if (open) scopes.push({ from: open.to, to: token.from, depth: open.depth + 1 });
    }
  }
  for (const open of opens) scopes.push({ from: open.to, to: end, depth: open.depth + 1 });
  for (const scope of scopes) {
    if (scope.depth <= depth || cursor < scope.from || cursor > scope.to) continue;
    if (!statement.some(t => t.from >= scope.from && t.to <= scope.to && t.depth === scope.depth && !t.quote && t.value.toUpperCase() === "SELECT")) continue;
    depth = scope.depth; scopeFrom = scope.from; scopeTo = scope.to;
  }
  const sameScope = statement.filter(t => t.depth === depth && t.from >= scopeFrom && t.to <= scopeTo);
  const isKeyword = (t: Token | undefined, word: string) => t?.kind === "identifier" && !t.quote && t.value.toUpperCase() === word;
  let tableContext = !!last && !last.quote && /^(FROM|JOIN|INTO|UPDATE|TABLE)$/i.test(last.value);
  if (last?.value === ",") {
    const clause = [...preceding].reverse().find(t => t.depth === depth && !t.quote && /^(FROM|JOIN|WHERE|SELECT|ON|GROUP|ORDER|HAVING|SET|VALUES)$/i.test(t.value));
    tableContext = !!clause && /^(FROM|JOIN)$/i.test(clause.value);
  }
  const names = metadataNames(data);
  const aliases = new Map<string, string>();
  const referenced = new Set<string>();
  let inFrom = false;
  for (let i = 0; i < sameScope.length; i++) {
    const token = sameScope[i];
    const upper = token.value.toUpperCase();
    if (!token.quote && /^(WHERE|ON|GROUP|ORDER|HAVING|LIMIT|UNION|SET|VALUES|RETURNING)$/.test(upper)) inFrom = false;
    const trigger = !token.quote && /^(FROM|JOIN|UPDATE|INTO)$/.test(upper);
    if (!(trigger || (inFrom && token.value === ","))) continue;
    inFrom = true;
    let j = i + 1;
    const parts: string[] = [];
    if (sameScope[j]?.kind !== "identifier") continue;
    parts.push(sameScope[j++].value);
    while (sameScope[j]?.value === "." && sameScope[j + 1]?.kind === "identifier") { parts.push(sameScope[j + 1].value); j += 2; }
    const owner = resolveOwner(parts, names, schema);
    if (!owner) continue;
    referenced.add(owner);
    aliases.set(normal(parts.join(".")), owner);
    aliases.set(normal(lastItem(parts)!), owner);
    if (isKeyword(sameScope[j], "AS")) j++;
    const alias = sameScope[j];
    if (alias?.kind === "identifier" && (alias.quote || !reserved.has(alias.value.toUpperCase()))) aliases.set(normal(alias.value), owner);
  }
  // 光标所在子句决定“下一步合法内容”：SELECT 列表给查询内容，表名后给子句关键字，
  // 表达式完整后给运算符或后续子句，而不是任何位置都塞字段。
  const CLAUSE_WORDS: Record<string, string> = {
    SELECT: "select", FROM: "from", JOIN: "join", ON: "on", WHERE: "where",
    HAVING: "having", LIMIT: "limit", OFFSET: "limit", SET: "set",
    VALUES: "values", INTO: "into", UPDATE: "update", DELETE: "delete",
    INSERT: "insert", WITH: "with", RETURNING: "returning",
    UNION: "union", EXCEPT: "union", INTERSECT: "union",
  };
  const openWord = /^(?:AND|OR|NOT|AS|DISTINCT|ALL|CASE|WHEN|THEN|ELSE|END|EXISTS|IS|IN|LIKE|BETWEEN|BY|GROUP|ORDER|USING)$/;
  let clause = "";
  for (let i = 0; i < preceding.length; i++) {
    const token = preceding[i];
    if (token.kind !== "identifier" || token.quote) continue;
    const word = token.value.toUpperCase();
    if ((word === "GROUP" || word === "ORDER") && preceding[i + 1]?.value?.toUpperCase() === "BY") {
      clause = word === "GROUP" ? "group" : "order";
      i++;
      continue;
    }
    clause = CLAUSE_WORDS[word] ?? clause;
  }
  // CASE 表达式内部的阶段：决定提示 WHEN/THEN/ELSE/END，而不是继续给字段。
  let caseStage = "";
  for (const token of preceding) {
    if (token.kind !== "identifier" || token.quote) continue;
    const word = token.value.toUpperCase();
    if (word === "CASE") caseStage = "case";
    else if (word === "WHEN") caseStage = "when";
    else if (word === "THEN") caseStage = "then";
    else if (word === "ELSE") caseStage = "else";
    else if (word === "END") caseStage = "";
  }
  const lastBefore = lastItem(preceding);
  const isOpenWord = !!lastBefore && lastBefore.kind === "identifier" && !lastBefore.quote &&
    (CLAUSE_WORDS[lastBefore.value.toUpperCase()] !== undefined || openWord.test(lastBefore.value));
  const endsExpression = !!lastBefore && !isOpenWord &&
    (lastBefore.kind === "identifier" || lastBefore.value === ")" || lastBefore.value === "*" || lastBefore.value === "]");

  const options: SqlCompletionItem[] = [];
  const seen = new Set<string>();
  const add = (label: string, type: string, detail?: string, boost = 0, identifier = true) => {
    if (!normal(label).startsWith(normal(prefix))) return;
    const key = normal(label);
    if (seen.has(key)) return;
    seen.add(key);
    let apply: string | undefined;
    if (identifier) {
      if (tail?.quote) {
        const close = tail.quote === "[" ? "]" : tail.quote;
        apply = label.replaceAll(close, close + close) + close;
      } else if (!/^[\p{L}_][\p{L}\p{N}_$]*$/u.test(label) || reserved.has(label.toUpperCase())) apply = label.split(".").map(p => quoteIdentifier(p, quote)).join(".");
    }
    options.push({ label, type, detail, boost, ...(apply === undefined ? {} : { apply }) });
  };
  // 表的灰字只有表备注，字段的灰字只有类型与备注。都不存在时不渲染该段。
  const relationDetail = (name: string): string | undefined =>
    noteText(data.__meta__?.relations?.[name]?.comment);
  const columnDetail = (owner: string, column: string): string | undefined => {
    const meta = data.__meta__?.columns?.[owner]?.[column];
    if (meta) return joinNote([noteText(meta.type), noteText(meta.comment)]);
    // 没有 __meta__ 的旧数据退回归属表名；有元数据但缺该列时宁可不显示。
    return data.__meta__ ? undefined : owner;
  };
  const addTables = (qualified = false) => {
    for (const name of names) {
      const label = qualified && name.includes(".") ? lastItem(name.split("."))! : name;
      add(label, "class", relationDetail(name), 10);
    }
  };
  const addColumns = (owner: string, boost: number) => {
    for (const column of data[owner] ?? []) add(column, "property", columnDetail(owner, column), boost);
  };
  const addAliases = (boost: number) => {
    for (const [alias, owner] of aliases) if (!alias.includes(".")) add(alias, "variable", owner, boost);
  };
  // 函数补全插入 "FN()" 并把光标放进括号；光标后已有左括号时不重复补括号。
  const nextIsParen = /^\s*\(/.test(sql.slice(to));
  const addFunctions = (boost: number) => {
    for (const fn of functions) {
      if (!normal(fn).startsWith(normal(prefix))) continue;
      const key = normal(fn);
      if (seen.has(key)) continue;
      seen.add(key);
      options.push(nextIsParen
        ? { label: fn, type: "function", boost }
        : { label: fn, type: "function", boost, apply: fn + "()", cursor: fn.length + 1 });
    }
  };
  const addWords = (words: readonly string[], boost: number, type = "keyword") => {
    for (const word of words) add(word, type, undefined, boost, false);
  };
  // CASE 不是普通关键字：一次插入完整骨架，光标停在第一个 WHEN 后面。
  const addCaseTemplate = (boost: number) => {
    const label = "CASE";
    if (!normal(label).startsWith(normal(prefix))) return;
    const key = normal(label);
    if (seen.has(key)) return;
    seen.add(key);
    options.push({ label, type: "keyword", detail: "CASE WHEN … THEN … ELSE … END", boost, apply: "CASE WHEN  THEN  ELSE  END", cursor: "CASE WHEN ".length });
  };
  if (["select", "where", "on", "having", "group", "order", "set", "returning"].includes(clause) && !tableContext && !qualifier.length) addCaseTemplate(18);
  // CASE 阶段：CASE 后先给 WHEN，WHEN 后给 THEN，THEN 后给 ELSE/END。
  const caseNext = caseStage === "case" ? ["WHEN"] : caseStage === "when" ? ["THEN"] : caseStage === "then" ? ["ELSE", "END"] : caseStage === "else" ? ["END"] : [];
  if (caseNext.length && !tableContext) addWords(caseNext, endsExpression || caseStage === "case" ? 40 : 10);
  if (qualifier.length) {
    const key = normal(qualifier.join("."));
    const owner = aliases.get(key) ?? resolveOwner(qualifier, names, schema);
    if (!tableContext && owner) addColumns(owner, 20);
    else if (normal(schema) === key || (data.__schemas__ ?? []).some(s => normal(s) === key)) addTables(true);
    else if (tableContext) for (const name of names) {
      if (normal(name).startsWith(key + ".")) add(name.slice(qualifier.join(".").length + 1), "class", relationDetail(name), 10);
    }
  } else if (tableContext) {
    addTables();
    for (const s of new Set([...(data.__schemas__ ?? []), ...(schema ? [schema] : [])])) add(s, "namespace", "命名空间", 5);
  } else if (!clause) {
    // 语句开头：只给能开启一条语句的关键字。
    addWords(["SELECT", "INSERT", "UPDATE", "DELETE", "WITH", "CREATE", "ALTER", "DROP", "TRUNCATE", "EXPLAIN", "BEGIN", "COMMIT", "ROLLBACK", "PRAGMA", "VACUUM", "ANALYZE"], 30);
    for (const owner of referenced) addColumns(owner, 5);
  } else if (clause === "select") {
    if (endsExpression) {
      // 表达式已完成：下一步是逗号续列或 FROM 等子句，而不是继续堆字段。
      addWords([",", "FROM", "UNION", "INTO"], 30);
      for (const owner of referenced) addColumns(owner, 5);
    } else {
      // SELECT 列表起点：字段、别名、函数、* 与少量列表关键字；不再以表名为主。
      for (const owner of referenced) addColumns(owner, 30);
      addAliases(25);
      add("*", "keyword", undefined, 15, false);
      addFunctions(20);
      addWords(["DISTINCT", "ALL", "CASE"], 12);
      for (const keyword of keywords) add(keyword, "keyword", undefined, 0, false);
    }
  } else if (clause === "from" || clause === "join") {
    if (!endsExpression) {
      addTables();
      addWords(["SELECT"], 5);
    } else {
      // 表名/别名已写完：先给下一个子句（WHERE/JOIN/GROUP BY…），字段只作低优先级兜底。
      addWords(clause === "join"
        ? ["ON", "USING", "JOIN", "LEFT JOIN", "RIGHT JOIN", "INNER JOIN", "FULL JOIN", "CROSS JOIN", "WHERE", "GROUP BY", "ORDER BY", "HAVING", "LIMIT", ","]
        : ["WHERE", "JOIN", "LEFT JOIN", "RIGHT JOIN", "INNER JOIN", "FULL JOIN", "CROSS JOIN", "GROUP BY", "ORDER BY", "HAVING", "LIMIT", ",", "UNION"], 30);
      for (const owner of referenced) addColumns(owner, 5);
      addAliases(4);
    }
  } else if (clause === "where" || clause === "having" || clause === "on") {
    if (endsExpression) {
      addWords(["=", "<>", "!=", ">=", "<=", ">", "<", "LIKE", "NOT LIKE", "IN", "NOT IN", "IS NULL", "IS NOT NULL", "BETWEEN"], 30);
      addWords(clause === "on"
        ? ["AND", "OR", "JOIN", "LEFT JOIN", "INNER JOIN", "WHERE", "GROUP BY", "ORDER BY", "HAVING", "LIMIT"]
        : clause === "having" ? ["AND", "OR", "ORDER BY", "LIMIT"] : ["AND", "OR", "GROUP BY", "ORDER BY", "HAVING", "LIMIT", "UNION"], 25);
      for (const owner of referenced) addColumns(owner, 3);
    } else {
      for (const owner of referenced) addColumns(owner, 30);
      addAliases(25);
      addFunctions(20);
      addWords(["NOT", "EXISTS", "CASE"], 8);
    }
  } else if (clause === "group" || clause === "order") {
    if (endsExpression) {
      addWords(clause === "group" ? [",", "HAVING", "ORDER BY", "LIMIT", "UNION"] : [",", "ASC", "DESC", "LIMIT", "OFFSET", "UNION"], 30);
    } else {
      for (const owner of referenced) addColumns(owner, 30);
      addAliases(25);
      addFunctions(15);
    }
  } else if (clause === "set") {
    if (endsExpression) addWords(["=", ",", "WHERE", "RETURNING"], 30);
    else { for (const owner of referenced) addColumns(owner, 30); addFunctions(15); }
  } else if (clause === "values") {
    addWords(["(", "DEFAULT", "NULL"], 20);
  } else if (clause === "insert" || clause === "into") {
    addWords(["INTO", "VALUES", "SELECT", "TABLE", "("], 25);
  } else if (clause === "update") {
    addWords(["SET"], 30);
  } else if (clause === "delete") {
    addWords(["FROM"], 30);
  } else if (clause === "union") {
    addWords(["SELECT", "ALL"], 30);
  } else if (clause === "with") {
    addWords(["RECURSIVE"], 10);
  } else if (clause === "returning") {
    for (const owner of referenced) addColumns(owner, 25);
    add("*", "keyword", undefined, 15, false);
  } else {
    for (const owner of referenced) addColumns(owner, 20);
    addAliases(8);
    if (!referenced.size) addTables();
    addFunctions(0);
    for (const keyword of keywords) add(keyword, "keyword", undefined, 0, false);
  }
  return { from, to, options, validFor: tail?.quote ? /^[^`"\]]*$/ : /^[\p{L}\p{N}_$]*$/u };
}
