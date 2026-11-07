/** Lightweight, dependency-free SQL completion analysis. Only real metadata is suggested. */
export interface SqlCompletionItem {
  label: string;
  type: string;
  detail?: string;
  apply?: string;
  boost?: number;
}
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
function metadataNames(data: Record<string, string[]>): string[] {
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

export function analyzeSqlCompletion(sql: string, cursor: number, data: Record<string, string[]>, keywords: readonly string[] = [], functions: readonly string[] = [], schema = "", quote = '"'): SqlCompletionAnalysis | null {
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
  const addTables = (qualified = false) => {
    for (const name of names) {
      const label = qualified && name.includes(".") ? lastItem(name.split("."))! : name;
      add(label, "class", (data.__views__ ?? []).some(v => normal(v) === normal(name)) ? "视图" : "表", 10);
    }
  };
  if (qualifier.length) {
    const key = normal(qualifier.join("."));
    const owner = aliases.get(key) ?? resolveOwner(qualifier, names, schema);
    if (!tableContext && owner) for (const column of data[owner] ?? []) add(column, "property", owner, 20);
    else if (normal(schema) === key || (data.__schemas__ ?? []).some(s => normal(s) === key)) addTables(true);
    else if (tableContext) for (const name of names) {
      if (normal(name).startsWith(key + ".")) add(name.slice(qualifier.join(".").length + 1), "class", "表", 10);
    }
  } else if (tableContext) {
    addTables();
    for (const s of new Set([...(data.__schemas__ ?? []), ...(schema ? [schema] : [])])) add(s, "namespace", "数据库 / 模式", 5);
  } else {
    for (const owner of referenced) {
      for (const column of data[owner] ?? []) add(column, "property", owner, 20);
    }
    for (const [alias, owner] of aliases) if (!alias.includes(".")) add(alias, "variable", owner, 8);
    // With no references, metadata tables are useful; don't flood SELECT with unrelated columns.
    if (!referenced.size) addTables();
    for (const keyword of keywords) add(keyword, "keyword", undefined, 0, false);
    for (const fn of functions) add(fn, "function", undefined, 0, false);
  }
  return { from, to, options, validFor: tail?.quote ? /^[^`"\]]*$/ : /^[\p{L}\p{N}_$]*$/u };
}
