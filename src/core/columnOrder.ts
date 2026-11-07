/** 物理字段排序：只使用服务端完整列定义，不重建表或拼凑丢失属性的类型。 */
export function columnOrderCapability(kind: string, params = "", readOnly = false): string | null {
  if (readOnly) return "此连接只读，不能调整字段顺序";
  if (params.startsWith("@navkit-driver:")) return "自定义驱动未提供安全字段排序能力";
  if (kind !== "mysql" && kind !== "mariadb") {
    return "物理字段排序仅支持原生 MySQL / MariaDB；当前数据库不支持安全原地调整，不会重建表";
  }
  return null;
}

const quote = (name: string): string => `\`${name.replace(/`/g, "``")}\``;

/** 切分 SHOW CREATE TABLE 的列/约束，保留引号、默认表达式和括号中的逗号。 */
function definitionParts(ddl: string): string[] {
  if (!/^\s*CREATE\s+TABLE\b/i.test(ddl)) throw new Error("未读取到完整 CREATE TABLE 定义，不能安全调整字段顺序");
  const parts: string[] = [];
  let depth = 0, start = -1, delimiter = "", blockComment = false, lineComment = false;
  for (let i = 0; i < ddl.length; i++) {
    const char = ddl[i], next = ddl[i + 1];
    if (blockComment) { if (char === "*" && next === "/") { blockComment = false; i++; } continue; }
    if (lineComment) { if (char === "\n" || char === "\r") lineComment = false; continue; }
    if (delimiter) {
      if (char === "\\" && delimiter !== "`") { i++; continue; }
      if (char === delimiter) { if (next === delimiter) i++; else delimiter = ""; }
      continue;
    }
    if (char === "'" || char === '"' || char === "`") { delimiter = char; continue; }
    if (char === "/" && next === "*") { blockComment = true; i++; continue; }
    if (char === "#" || (char === "-" && next === "-" && /\s/.test(ddl[i + 2] ?? ""))) { lineComment = true; continue; }
    if (char === "(") { depth++; if (depth === 1) start = i + 1; continue; }
    if (char === ")") {
      depth--;
      if (depth === 0 && start >= 0) { parts.push(ddl.slice(start, i).trim()); return parts; }
      if (depth < 0) break;
    }
    if (char === "," && depth === 1) { parts.push(ddl.slice(start, i).trim()); start = i + 1; }
  }
  throw new Error("CREATE TABLE 定义不完整，不能安全调整字段顺序");
}

export function mysqlColumnDefinitions(ddl: string): Map<string, string> {
  const definitions = new Map<string, string>();
  for (const part of definitionParts(ddl)) {
    // SHOW CREATE TABLE 的字段名会引用；约束/索引行不以引用标识符开头。
    const match = /^`((?:``|[^`])+)`\s+\S/.exec(part);
    if (!match) continue;
    const name = match[1].replace(/``/g, "`");
    if (definitions.has(name)) throw new Error(`字段定义重复：${name}`);
    definitions.set(name, part);
  }
  if (!definitions.size) throw new Error("CREATE TABLE 未包含可安全复用的字段定义");
  return definitions;
}

/** 乐观检查旧顺序，单条 ALTER 内只移动必要字段；字段属性直接复用服务端定义。 */
export function mysqlColumnOrderSql(ddl: string, schema: string, table: string, originalOrder: string[], desiredOrder: string[]): string | null {
  const definitions = mysqlColumnDefinitions(ddl);
  const actual = [...definitions.keys()];
  if (actual.length !== originalOrder.length || actual.some((name, i) => name !== originalOrder[i])) {
    throw new Error("数据库字段结构或顺序已发生变化，请重新打开表结构后再调整");
  }
  if (desiredOrder.length !== actual.length || new Set(desiredOrder).size !== actual.length || desiredOrder.some(name => !definitions.has(name))) {
    throw new Error("排序草稿必须完整保留原有字段，不能混合新增、删除或重命名");
  }
  const clauses: string[] = [];
  const current = [...actual];
  desiredOrder.forEach((name, index) => {
    const at = current.indexOf(name);
    if (at === index) return;
    clauses.push(`MODIFY COLUMN ${definitions.get(name)} ${index === 0 ? "FIRST" : `AFTER ${quote(desiredOrder[index - 1])}`}`);
    current.splice(at, 1); current.splice(index, 0, name);
  });
  const full = schema ? `${quote(schema)}.${quote(table)}` : quote(table);
  return clauses.length ? `ALTER TABLE ${full}\n  ${clauses.join(",\n  ")};` : null;
}
