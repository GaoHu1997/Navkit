/** SQL 美化只调整词法单元之间的空白，不改写标识符、字面量、注释和运算符。 */
export interface SqlFormatOptions {
  dialect?: "mysql" | "postgres" | "sqlite";
}

interface Token {
  text: string;
  gap: string;
  kind: "word" | "quoted" | "comment" | "symbol";
}

function tokenize(sql: string, dialect: SqlFormatOptions["dialect"]): Token[] | null {
  const tokens: Token[] = [];
  let i = 0;
  while (i < sql.length) {
    const gapStart = i;
    while (i < sql.length && /\s/.test(sql[i])) i++;
    const gap = sql.slice(gapStart, i), start = i;
    if (i === sql.length) break;
    let kind: Token["kind"] = "symbol";
    if (sql.startsWith("--", i) || (dialect === "mysql" && sql[i] === "#")) {
      const end = sql.indexOf("\n", i);
      i = end < 0 ? sql.length : end;
      kind = "comment";
    } else if (sql.startsWith("/*", i)) {
      let nesting = 1;
      i += 2;
      while (i < sql.length && nesting) {
        if (sql.startsWith("/*", i) && dialect === "postgres") { nesting++; i += 2; }
        else if (sql.startsWith("*/", i)) { nesting--; i += 2; }
        else i++;
      }
      if (nesting) return null;
      kind = "comment";
    } else if (sql[i] === "'" || sql[i] === '"' || sql[i] === String.fromCharCode(96) || sql[i] === "[") {
      const quote = sql[i++], endQuote = quote === "[" ? "]" : quote;
      // MySQL 引号支持反斜杠；PostgreSQL 仅 E'…' 支持。
      const escaped = dialect === "mysql" || (dialect === "postgres" && quote === "'"
        && gap === "" && /^e$/i.test(tokens[tokens.length - 1]?.text ?? ""));
      let closed = false;
      while (i < sql.length) {
        if (escaped && sql[i] === "\\") { i += 2; continue; }
        if (sql[i++] !== endQuote) continue;
        if (sql[i] === endQuote) { i++; continue; }
        closed = true; break;
      }
      if (!closed) return null;
      kind = "quoted";
    } else {
      const dollar = dialect === "postgres" ? /^\$(?:[A-Za-z_][A-Za-z_0-9]*)?\$/.exec(sql.slice(i)) : null;
      if (dollar) {
        const end = sql.indexOf(dollar[0], i + dollar[0].length);
        if (end < 0) return null;
        i = end + dollar[0].length;
        kind = "quoted";
      } else {
        const word = /^[\p{L}\p{N}_$]+/u.exec(sql.slice(i));
        if (word) { i += word[0].length; kind = "word"; }
        else i++;
      }
    }
    tokens.push({ text: sql.slice(start, i), gap, kind });
  }
  return tokens;
}

const HEADS = new Set(["SELECT", "FROM", "WHERE", "HAVING", "LIMIT", "OFFSET", "UNION",
  "INTERSECT", "EXCEPT", "VALUES", "SET", "RETURNING", "JOIN", "ON"]);
const JOIN_PREFIXES = new Set(["LEFT", "RIGHT", "INNER", "FULL", "CROSS", "NATURAL"]);

/** 常见查询按子句、字段列表换行；括号内表达式与引用内容保持原样。 */
export function formatSql(sql: string, options: SqlFormatOptions = {}): string {
  const tokens = tokenize(sql, options.dialect ?? "mysql");
  // 未闭合的引号 / 注释不猜测边界，原样保留草稿。
  if (!tokens) return sql;
  let output = "", depth = 0, caseDepth = 0, clause = "", definition = false;
  let afterComma = false, afterStatement = false;
  const word = (index: number): string => tokens[index]?.kind === "word" ? tokens[index].text.toUpperCase() : "";
  for (let i = 0; i < tokens.length; i++) {
    const token = tokens[i], keyword = word(i), previous = tokens[i - 1];
    let gap = token.gap;
    const joinedIdentifier = previous?.text === "." || (previous?.text === ":" && token.gap === "");
    const next = word(i + 1), before = word(i - 1);
    const joinPrefix = JOIN_PREFIXES.has(keyword) && (next === "JOIN" || (next === "OUTER" && word(i + 2) === "JOIN"));
    const compoundTail = (keyword === "FROM" && before === "DELETE")
      || (keyword === "JOIN" && (JOIN_PREFIXES.has(before) || before === "OUTER"));
    const head = !joinedIdentifier && !compoundTail && depth === 0 && caseDepth === 0
      && (HEADS.has(keyword) || joinPrefix || ((keyword === "GROUP" || keyword === "ORDER") && next === "BY"));
    const indent = definition ? 1 : 0;
    const clauseIndent = keyword === "ON" ? indent + 1 : indent;
    // 相邻引用之间的换行在 PostgreSQL 中决定字符串拼接，不能压成空格。
    const adjacentQuotes = token.kind === "quoted" && previous?.kind === "quoted";
    if (output && !adjacentQuotes) {
      if (afterStatement) gap = "\n\n";
      else if (head) gap = "\n" + "  ".repeat(clauseIndent);
      else if (afterComma && token.kind !== "comment") gap = "\n" + "  ".repeat(indent + 1);
    }
    if (!output) gap = "";
    output += gap + token.text;
    afterComma = false; afterStatement = false;
    if (token.kind === "comment" || token.kind === "quoted") continue;
    if (keyword === "CREATE" || keyword === "ALTER") definition = true;
    if (head) clause = keyword;
    if (keyword === "CASE") caseDepth++;
    if (keyword === "END") caseDepth = Math.max(0, caseDepth - 1);
    if (token.text === "(") depth++;
    if (token.text === ")") depth = Math.max(0, depth - 1);
    if (token.text === "," && depth === 0 && caseDepth === 0 && clause) afterComma = true;
    if (token.text === ";") {
      afterStatement = true; clause = ""; definition = false;
    }
  }
  return output;
}
