/**
 * 代码段数据源：内置模板 + 用户自定义（localStorage 持久化）。
 *
 * 查询页右侧信息面板与「代码段」对话框共用同一份数据，保证两处对应一致；
 * 自定义代码段可随时在两处新增 / 删除。
 */

export interface QuerySnippet {
  name: string;
  tag: string;
  desc: string;
  sql: string;
  /** 是否为用户自定义（自定义项可删除）。 */
  custom?: boolean;
}

/** 内置代码段（只读，参考截图 5 的清单）。 */
const BUILT_IN: QuerySnippet[] = [
  { name: "SELECT 查询", tag: "查询", desc: "检索表中的记录", sql: "SELECT *\nFROM table_name\nWHERE condition;" },
  { name: "INSERT 插入", tag: "DML", desc: "插入一条记录", sql: "INSERT INTO table_name (column_name)\nVALUES ('value');" },
  { name: "UPDATE 更新", tag: "DML", desc: "按条件更新，执行前检查 WHERE", sql: "UPDATE table_name\nSET column_name = 'value'\nWHERE condition;" },
  { name: "DELETE 删除", tag: "DML", desc: "按条件删除，执行前检查 WHERE", sql: "DELETE FROM table_name\nWHERE condition;" },
  { name: "多表连接", tag: "查询", desc: "INNER JOIN 连接两张表", sql: "SELECT a.*, b.*\nFROM table_a a\nINNER JOIN table_b b ON b.a_id = a.id\nLIMIT 100;" },
  { name: "分组统计", tag: "查询", desc: "GROUP BY 聚合计数", sql: "SELECT column, COUNT(*) AS cnt\nFROM table_name\nGROUP BY column\nORDER BY cnt DESC;" },
  { name: "CASE 分支", tag: "流程控制", desc: "在查询中使用条件表达式", sql: "CASE\n  WHEN condition THEN 'yes'\n  ELSE 'no'\nEND" },
  { name: "SQL 注释", tag: "注释", desc: "插入说明注释", sql: "-- 在这里填写说明\n" },
  { name: "多字段排序", tag: "查询", desc: "依次使用多个字段排序", sql: "SELECT * FROM table_name\nORDER BY first_column ASC, second_column DESC;" },
  { name: "建表", tag: "DDL", desc: "创建一张新表", sql: "CREATE TABLE table_name (\n  id BIGINT PRIMARY KEY AUTO_INCREMENT,\n  name VARCHAR(100) NOT NULL,\n  created_at DATETIME\n);" },
];

const STORAGE_KEY = "navkit.custom-snippets.v1";

/** 读取自定义代码段（损坏数据按空处理）。 */
function loadCustom(): QuerySnippet[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(
      (s): s is QuerySnippet =>
        !!s && typeof s === "object" &&
        typeof (s as QuerySnippet).name === "string" && typeof (s as QuerySnippet).sql === "string",
    ).map((s) => ({ name: s.name, tag: s.tag || "自定义", desc: s.desc || "自定义代码段", sql: s.sql, custom: true }));
  } catch {
    return [];
  }
}

function saveCustom(snippets: QuerySnippet[]): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(snippets));
  } catch {
    throw new Error("代码段保存失败：本地存储不可用，输入内容已保留，请重试。");
  }
}

/** 全部代码段：内置在前，自定义在后。 */
export function querySnippets(): QuerySnippet[] {
  return [...BUILT_IN, ...loadCustom()];
}

/** 新增自定义代码段；重名（含内置）时抛错。 */
export function addSnippet(snippet: { name: string; tag: string; desc: string; sql: string }): QuerySnippet {
  const name = snippet.name.trim();
  if (!name) throw new Error("请填写代码段名称");
  if (!snippet.sql.trim()) throw new Error("请填写 SQL 内容");
  if (querySnippets().some((s) => s.name === name)) throw new Error(`代码段「${name}」已存在`);
  const item: QuerySnippet = { name, tag: snippet.tag.trim() || "自定义", desc: snippet.desc.trim() || "自定义代码段", sql: snippet.sql, custom: true };
  const custom = loadCustom();
  custom.push(item);
  saveCustom(custom);
  return item;
}

/** 删除自定义代码段；内置项不可删除。 */
export function removeSnippet(name: string): void {
  saveCustom(loadCustom().filter((s) => s.name !== name));
}
