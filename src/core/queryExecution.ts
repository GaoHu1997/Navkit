/**
 * 查询页「解释」按钮的 SQL 重写。
 *
 * 这里只做文本规范化，不接触执行通道。安全约束只有一条：
 * 绝不把多条语句拼成 `EXPLAIN <first>; <rest>` —— `<rest>` 会被数据库原样执行。
 * 因此遇到多条有效语句、无法安全切分的引用或未闭合的引号/注释时，直接抛错，
 * 由调用方提示用户选中一条 SQL。
 *
 * 方言限制（与后端 src-tauri/src/db.rs:split_statements 的实际能力对齐）：
 * - 后端只识别 '' / "" / `` 三种引号（双写转义），不处理反斜杠转义；
 *   本模块同样不把反斜杠当转义，避免前端认为一条、后端切成两条。
 * - 后端把任意 `--` 当行注释（到 \n），不识别 MySQL `#` 注释；
 *   本模块识别 `#` 仅用于检测注释内分号并在存在时拒绝。
 * - 后端不识别 PG 美元引号、SQL Server 方括号；
 *   本模块识别它们，仅当字面量内含分号（会导致后端切开）时拒绝。
 * - 以 `/*!` 开头的块注释是 MySQL 可执行注释，服务器会执行其内容，可能绕过 EXPLAIN；
 *   一律拒绝。
 * - 后端块注释不嵌套，本模块同样不嵌套；未闭合直接拒绝。
 */

const UNTERMINATED = 'SQL 中存在未闭合的引号、注释或括号，无法安全解释；请检查后再试';
const HASH_COMMENT_FORBIDDEN = 'MySQL # 注释内含分号；后端 SQL 拆分器不认识 # 注释，会在分号处切开，请改用 -- 注释或拆成单条语句';
const MYSQL_EXECUTABLE_COMMENT = '检测到 MySQL 可执行注释 /*! ... */；该注释内的 SQL 会被服务器执行，无法安全地解释，请改为普通 SQL';
const DOLLAR_QUOTE_SEMICOLON = 'PG 美元引号字符串内含分号；后端 SQL 拆分器不认识美元引号，会在分号处切开，请改用普通字符串或拆成单条语句';
const BRACKET_SEMICOLON = 'SQL Server 方括号标识符内含分号；后端 SQL 拆分器不认识方括号，会在分号处切开，请改用双引号或拆成单条语句';

/**
 * 把用户 SQL 规范成一条可解释语句。
 *
 * - 单条有效语句：返回 `EXPLAIN <原样语句>`，不重排、不合并。
 * - 空/仅注释/多条有效语句：抛错。
 * - 已带 EXPLAIN（含 EXPLAIN ANALYZE）：抛错，避免把会真正执行写入的语句
 *   误当成普通解释，或生成 `EXPLAIN EXPLAIN ...`。
 */
export function explainQuerySql(sql: string): string {
  const statements = splitStatements(sql).filter(hasExecutableContent);
  if (statements.length === 0) {
    throw new Error('SQL 为空或只有注释，没有可解释的语句；请选中一条 SQL 再解释');
  }
  if (statements.length > 1) {
    throw new Error(`解释只支持单条语句，当前有 ${statements.length} 条；请选中其中一条再解释`);
  }
  const statement = statements[0].trim();
  const keyword = leadingKeyword(statement);
  if (keyword !== null && keyword.toUpperCase() === 'EXPLAIN') {
    throw new Error('SQL 已包含 EXPLAIN；为避免 EXPLAIN ANALYZE 执行写操作，请去掉 EXPLAIN 后再解释，或直接运行');
  }
  return `EXPLAIN ${statement}`;
}

function splitStatements(sql: string): string[] {
  const statements: string[] = [];
  let start = 0;
  let i = 0;
  const n = sql.length;
  while (i < n) {
    const ch = sql[i];
    if (ch === "'" || ch === '"' || ch === '`') {
      i = skipQuoted(sql, i, ch);
    } else if (ch === '[') {
      i = skipBracket(sql, i);
    } else if (ch === '-' && sql[i + 1] === '-') {
      i = skipLineComment(sql, i);
    } else if (ch === '/' && sql[i + 1] === '*') {
      i = skipBlockComment(sql, i);
    } else if (ch === '#') {
      i = skipHashComment(sql, i);
    } else if (ch === '$') {
      const end = skipDollarQuoted(sql, i);
      i = end > i ? end : i + 1;
    } else if (ch === ';') {
      statements.push(sql.slice(start, i));
      start = i + 1;
      i += 1;
    } else {
      i += 1;
    }
  }
  statements.push(sql.slice(start));
  return statements;
}

/** 引号字面量：只处理双写转义，不处理反斜杠（与后端 split_statements 一致）。 */
function skipQuoted(sql: string, start: number, quote: string): number {
  const n = sql.length;
  let i = start + 1;
  while (i < n) {
    if (sql[i] === quote) {
      if (sql[i + 1] === quote) {
        i += 2;
        continue;
      }
      return i + 1;
    }
    i += 1;
  }
  throw new Error(UNTERMINATED);
}

/** SQL Server 方括号标识符；后端不认识，字面量内分号会让后端切开，直接拒绝。 */
function skipBracket(sql: string, start: number): number {
  const n = sql.length;
  let i = start + 1;
  while (i < n) {
    if (sql[i] === ']') {
      if (sql[i + 1] === ']') {
        i += 2;
        continue;
      }
      if (sql.slice(start + 1, i).includes(';')) throw new Error(BRACKET_SEMICOLON);
      return i + 1;
    }
    i += 1;
  }
  throw new Error(UNTERMINATED);
}

/** 行注释：任意 `--` 到 \n（与后端一致）。 */
function skipLineComment(sql: string, start: number): number {
  const n = sql.length;
  let i = start + 2;
  while (i < n && sql[i] !== '\n') i += 1;
  return i;
}

/** MySQL `#` 注释：后端不认识它，注释内分号会被后端切开，因此拒绝。 */
function skipHashComment(sql: string, start: number): number {
  const n = sql.length;
  let i = start + 1;
  while (i < n && sql[i] !== '\n') i += 1;
  if (sql.slice(start, i).includes(';')) throw new Error(HASH_COMMENT_FORBIDDEN);
  return i;
}

/** 块注释：不嵌套（与后端一致）；`/*!` 可执行注释一律拒绝。 */
function skipBlockComment(sql: string, start: number): number {
  if (sql[start + 2] === '!') throw new Error(MYSQL_EXECUTABLE_COMMENT);
  const end = sql.indexOf('*/', start + 2);
  if (end === -1) throw new Error(UNTERMINATED);
  return end + 2;
}

/** PG 美元引号；后端不认识，字面量内分号会让后端切开，直接拒绝。 */
function skipDollarQuoted(sql: string, start: number): number {
  if (start > 0 && isIdentifierPart(sql[start - 1])) return start;
  const n = sql.length;
  let i = start + 1;
  if (i < n && (isLetter(sql[i]) || sql[i] === '_')) {
    i += 1;
    while (i < n && isIdentifierPart(sql[i])) i += 1;
  }
  if (i >= n || sql[i] !== '$') return start;
  const delimiter = sql.slice(start, i + 1);
  const bodyStart = i + 1;
  const end = sql.indexOf(delimiter, bodyStart);
  if (end === -1) throw new Error(UNTERMINATED);
  if (sql.slice(bodyStart, end).includes(';')) throw new Error(DOLLAR_QUOTE_SEMICOLON);
  return end + delimiter.length;
}

function leadingKeyword(statement: string): string | null {
  const n = statement.length;
  let i = 0;
  while (i < n) {
    const ch = statement[i];
    if (isWhitespace(ch)) {
      i += 1;
    } else if (ch === '-' && statement[i + 1] === '-') {
      i = skipLineComment(statement, i);
    } else if (ch === '/' && statement[i + 1] === '*') {
      i = skipBlockComment(statement, i);
    } else if (ch === '#') {
      i = skipHashComment(statement, i);
    } else {
      break;
    }
  }
  if (i >= n || !(isLetter(statement[i]) || statement[i] === '_')) return null;
  let j = i + 1;
  while (j < n && isIdentifierPart(statement[j])) j += 1;
  return statement.slice(i, j);
}

function hasExecutableContent(statement: string): boolean {
  const n = statement.length;
  let i = 0;
  while (i < n) {
    const ch = statement[i];
    if (isWhitespace(ch)) {
      i += 1;
    } else if (ch === '-' && statement[i + 1] === '-') {
      i = skipLineComment(statement, i);
    } else if (ch === '/' && statement[i + 1] === '*') {
      i = skipBlockComment(statement, i);
    } else if (ch === '#') {
      i = skipHashComment(statement, i);
    } else {
      return true;
    }
  }
  return false;
}

function isWhitespace(ch: string): boolean {
  return ch === ' ' || ch === '\t' || ch === '\n' || ch === '\r' || ch === '\f' || ch === '\v';
}

function isLetter(ch: string): boolean {
  return (ch >= 'A' && ch <= 'Z') || (ch >= 'a' && ch <= 'z');
}

function isIdentifierPart(ch: string): boolean {
  return isLetter(ch) || (ch >= '0' && ch <= '9') || ch === '_';
}
