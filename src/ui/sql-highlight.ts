/** SQL 文本高亮：注释 / 字符串 / 反引号标识符 / 关键字 / 类型。
 *  产物是一段 HTML 片段，颜色由 base.css 的 .ddl-kw / .ddl-type / .ddl-str / .ddl-comment / .ddl-ident 决定，
 *  信息面板的 DDL 页与表设计器的 SQL 预览共用同一套着色，避免两处 SQL 看起来不是同一个产品。 */

const KEYWORDS = new Set([
  "CREATE", "TABLE", "PRIMARY", "KEY", "NOT", "NULL", "DEFAULT", "ENGINE",
  "CHARSET", "COLLATE", "COMMENT", "AUTO_INCREMENT", "UNSIGNED", "ZEROFILL",
  "UNIQUE", "INDEX", "CONSTRAINT", "FOREIGN", "REFERENCES", "ON", "DELETE",
  "UPDATE", "CASCADE", "SET", "VIEW", "AS", "SELECT", "FROM", "WHERE", "AND",
  "OR", "IF", "EXISTS", "REPLACE", "ALTER", "ADD", "DROP", "COLUMN", "TYPE",
  "GENERATED", "ALWAYS", "STORED", "VIRTUAL", "PARTITION", "BY", "RANGE",
  "LIST", "HASH", "CHECK", "TEMPORARY", "INCREMENT", "START", "MODIFY",
  "CHANGE", "RENAME", "TO", "AFTER", "FIRST", "USING", "BTREE", "HASH",
  "ASC", "DESC", "CURRENT_TIMESTAMP", "CURRENT_DATE", "CURRENT_TIME",
]);

const TYPES = new Set([
  "INT", "INTEGER", "BIGINT", "SMALLINT", "TINYINT", "MEDIUMINT", "VARCHAR", "CHAR",
  "DECIMAL", "NUMERIC", "FLOAT", "DOUBLE", "REAL", "TEXT", "LONGTEXT",
  "MEDIUMTEXT", "TINYTEXT", "BLOB", "LONGBLOB", "DATETIME", "TIMESTAMP",
  "DATE", "TIME", "YEAR", "BIT", "BOOLEAN", "BOOL", "ENUM", "JSON", "UUID",
  "SERIAL", "BIGSERIAL", "TINYBLOB", "MEDIUMBLOB", "VARBINARY", "BINARY",
]);

function escapeHtml(text: string): string {
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/** 逐 token 扫描并包装：注释 / 字符串 / 反引号标识符 / 单词 / 数字，其余原样转义。 */
export function highlightSql(sql: string): string {
  const re = /(--[^\n]*|\/\*[\s\S]*?\*\/|'(?:[^'\\]|\\.|'')*'|"(?:[^"\\]|\\.)*"|`[^`]*`|[A-Za-z_][A-Za-z0-9_]*|\d+(?:\.\d+)?|\s+|.)/g;
  let out = "";
  for (const match of sql.matchAll(re)) {
    const token = match[0];
    const first = token[0];
    if (first === "-" || first === "/") out += `<span class="ddl-comment">${escapeHtml(token)}</span>`;
    else if (first === "'" || first === '"') out += `<span class="ddl-str">${escapeHtml(token)}</span>`;
    else if (first === "`") out += `<span class="ddl-ident">${escapeHtml(token)}</span>`;
    else if (/^[A-Za-z_]/.test(token)) {
      const upper = token.toUpperCase();
      if (KEYWORDS.has(upper)) out += `<span class="ddl-kw">${escapeHtml(token)}</span>`;
      else if (TYPES.has(upper)) out += `<span class="ddl-type">${escapeHtml(token)}</span>`;
      else out += escapeHtml(token);
    } else out += escapeHtml(token);
  }
  return out;
}
