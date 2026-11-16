/**
 * 关系图智能匹配（虚拟外键）—— 纯逻辑，无 DOM 依赖。
 *
 * 移植自 参考实现 `lib/diagram/match-strategies.ts` + `match-engine.ts`，按 NavKit 的类型与
 * 约束重写。解决的核心问题：大量数据库不建物理外键（或引擎不支持），ER 图上表全是孤岛。
 *
 * 匹配策略分层（优先级从高到低）：
 *   P0 物理外键   —— 由 erDiagram.buildRelationships 产出，本模块不重复推断
 *   P1 命名约定   —— {table}_id / {table}_uuid / {table}_pk → 目标表主键
 *   P2 类型签名   —— P1 + 源列与目标列类型兼容（bigint↔int 等）
 *
 * 与 参考实现 的差异（修正项）：
 *   · 参考实现 用 `inferred:...` 形式的 id 去重，物理外键与推断关系的 id 永不相同，会重复出边。
 *     这里改为按 (源表,源列,目标表,目标列) 四元组去重，物理外键优先。
 *   · 源列已被物理外键覆盖时不再推断，避免同一条边两种样式。
 *   · 自引用（表指向自己）不推断。
 */

import type { ErColumn, ErRelationship, ErTable } from "./erDiagram";

/** 推断出的关系（尚未合入 ErRelationship）。 */
export interface InferredRelationship {
  id: string;
  sourceTable: string;
  sourceColumn: string;
  targetTable: string;
  targetColumn: string;
  confidence: "high" | "medium";
  strategy: "naming_convention" | "type_signature";
}

export interface MatchStats {
  total: number;
  high: number;
  medium: number;
}

export interface MatchResult {
  /** 可以直接上图的推断关系（已排除冲突项）。 */
  relationships: InferredRelationship[];
  /** 同一源列指向多张表，需要用户裁决。 */
  conflicts: InferredRelationship[];
  pending: InferredRelationship[];
  stats: MatchStats;
}

// ---------------------------------------------------------------- 类型兼容

/** 只覆盖整数族与字符串/UUID 族：跨族（如 int↔varchar）不算兼容。 */
const TYPE_COMPATIBLE_MAP: Record<string, readonly string[]> = {
  bigint: ["bigint", "int", "smallint", "tinyint", "integer", "long", "int8", "int4", "int2"],
  int: ["int", "bigint", "smallint", "tinyint", "integer", "int4", "int8", "int2"],
  integer: ["integer", "int", "bigint", "smallint", "int4", "int8"],
  smallint: ["smallint", "int", "bigint", "tinyint", "int2", "int4"],
  tinyint: ["tinyint", "smallint", "int", "bigint", "int2"],
  long: ["long", "bigint", "int"],
  int8: ["int8", "bigint", "int", "int4", "int2"],
  int4: ["int4", "int", "integer", "bigint", "int8", "smallint", "int2"],
  int2: ["int2", "smallint", "int", "int4"],
  uuid: ["uuid", "char", "varchar", "nvarchar", "uniqueidentifier", "guid"],
  char: ["char", "varchar", "uuid", "uniqueidentifier", "nvarchar"],
  varchar: ["varchar", "char", "uuid", "uniqueidentifier", "nvarchar"],
  nvarchar: ["nvarchar", "varchar", "char", "uuid", "uniqueidentifier"],
  uniqueidentifier: ["uniqueidentifier", "uuid", "char", "varchar"],
  guid: ["guid", "uuid", "char", "varchar"],
};

/** 归一化数据库类型：去参数、去空白、小写（`VARCHAR(50)` → `varchar`）。 */
export function normalizeColumnType(type: string): string {
  const base = (type ?? "").split(/[(<]/)[0]?.trim().toLowerCase() ?? "";
  // 常见别名收敛，避免 int / integer / int4 被当成不同类型而漏匹配。
  if (base === "serial" || base === "bigserial") return base === "serial" ? "int" : "bigint";
  return base;
}

export function isTypeCompatible(sourceType: string, targetType: string): boolean {
  const source = normalizeColumnType(sourceType);
  const target = normalizeColumnType(targetType);
  if (!source || !target) return false;
  if (source === target) return true;
  if (TYPE_COMPATIBLE_MAP[source]?.includes(target)) return true;
  if (TYPE_COMPATIBLE_MAP[target]?.includes(source)) return true;
  return false;
}

// ---------------------------------------------------------------- 命名约定

/** snake_case / camelCase / PascalCase → snake_case。 */
export function toSnakeCase(value: string): string {
  return value
    .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
    .replace(/([A-Z]+)([A-Z][a-z])/g, "$1_$2")
    .toLowerCase();
}

/**
 * 从列名反推目标表名候选（按置信度顺序返回，去重）。
 * 例：`user_id` → `user`；`userId` → `user`；`order_uuid` → `order`。
 */
export function extractTableNameCandidates(columnName: string): string[] {
  const name = (columnName ?? "").trim();
  if (!name) return [];
  const out: string[] = [];
  const push = (value: string): void => {
    // 去掉残留分隔符：`user_id` 的 camel 分支会算出 `user_`，不能当表名。
    const trimmed = value.replace(/[_\-\s]+$/, "").trim();
    if (trimmed.length >= 2 && !out.includes(trimmed)) out.push(trimmed);
  };
  // 1) 后缀 _id / _uuid / _pk / _key（snake_case）
  const suffix = name.match(/^(.+?)_(?:id|uuid|pk|key)$/i);
  if (suffix) push(suffix[1]);
  // 2) camelCase：userId / userID / orderUid
  //    只在名称确实含大写字母时启用；否则 `user_uuid` 会被切成 `user_u`。
  if (/[A-Z]/.test(name)) {
    const camel = name.match(/^(.+[a-z0-9])(?:Id|ID|Uid|UID)$/);
    if (camel) push(camel[1]);
  }
  // 3) 全小写粘连：userid。只认全小写且无分隔符的名称，
  //    否则 `user_uuid` 会被切成 `user_u`（真正的目标是 `user`，已由分支 1 得到）。
  if (name === name.toLowerCase() && !/[_\-.]/.test(name)) {
    const glued = name.match(/^(.+?)(?:id|uid)$/);
    if (glued) push(glued[1]);
  }
  return out;
}

// ---------------------------------------------------------------- 表名索引

/** 每张表的主键列（取第一列，与 参考实现 一致）。 */
export interface PrimaryKeyInfo {
  name: string;
  data_type: string;
}

export function buildPrimaryKeyIndex(tables: readonly ErTable[]): Map<string, PrimaryKeyInfo> {
  const index = new Map<string, PrimaryKeyInfo>();
  for (const table of tables) {
    const pk = table.columns.find((column) => column.primary_key);
    if (pk) index.set(table.name, { name: pk.name, data_type: pk.type });
  }
  return index;
}

/** 表名 → 表（同时接受 `schema.table` 与裸表名，大小写不敏感）。 */
function buildTableLookup(tables: readonly ErTable[]): Map<string, ErTable> {
  const lookup = new Map<string, ErTable>();
  for (const table of tables) {
    const bare = table.name.toLowerCase();
    if (!lookup.has(bare)) lookup.set(bare, table);
    if (table.schema) {
      const qualified = `${table.schema}.${table.name}`.toLowerCase();
      if (!lookup.has(qualified)) lookup.set(qualified, table);
    }
  }
  return lookup;
}

/**
 * 把候选表名解析成真实表：
 * 先精确（snake_case 归一后），再单复数容错（user → users），最后唯一子串匹配。
 * 返回 null 表示无法确定，绝不猜。
 */
export function resolveTargetTable(candidate: string, lookup: Map<string, ErTable>, tables: readonly ErTable[]): ErTable | null {
  const snake = toSnakeCase(candidate);
  const direct = lookup.get(snake) ?? lookup.get(candidate.toLowerCase());
  if (direct) return direct;

  // 单复数容错：user ↔ users
  const plural = lookup.get(`${snake}s`);
  if (plural) return plural;
  const singular = snake.endsWith("s") ? lookup.get(snake.slice(0, -1)) : undefined;
  if (singular) return singular;

  // 唯一子串匹配（参考实现 的 partialMatch，但要求唯一，避免歧义）
  const matches = tables.filter((table) => table.name.toLowerCase().includes(snake));
  if (matches.length === 1) return matches[0];
  return null;
}

// ---------------------------------------------------------------- 匹配主流程

export function buildRelationshipId(sourceTable: string, sourceColumn: string, targetTable: string, targetColumn: string): string {
  return ["inferred", sourceTable, sourceColumn, targetTable, targetColumn].join(":");
}

/** 关系四元组键：物理外键与推断关系共用同一去重口径。 */
export function relationshipKey(sourceTable: string, sourceColumn: string, targetTable: string, targetColumn: string): string {
  return JSON.stringify([sourceTable, sourceColumn, targetTable, targetColumn]);
}

/**
 * 推断虚拟外键。
 * @param tables      当前可见的表集合
 * @param existing    已有关系（物理外键 / 自定义），用于排除重复
 */
export function inferRelationships(tables: readonly ErTable[], existing: readonly ErRelationship[] = []): InferredRelationship[] {
  if (tables.length === 0) return [];
  const primaryKeys = buildPrimaryKeyIndex(tables);
  const lookup = buildTableLookup(tables);

  // 已被物理外键/自定义关系覆盖的源列，不再推断（同一条边不该有两种样式）。
  const coveredSources = new Set(existing.map((rel) => relationshipKey(rel.sourceTable, rel.sourceColumn, rel.targetTable, rel.targetColumn)));
  const coveredColumns = new Set(existing.map((rel) => `${rel.sourceTable}\u0000${rel.sourceColumn}`));

  const results: InferredRelationship[] = [];
  const seen = new Set<string>();

  for (const table of tables) {
    for (const column of table.columns) {
      if (column.primary_key) continue;
      if (coveredColumns.has(`${table.name}\u0000${column.name}`)) continue;

      const candidates = extractTableNameCandidates(column.name);
      if (candidates.length === 0) continue;

      let target: ErTable | null = null;
      for (const candidate of candidates) {
        target = resolveTargetTable(candidate, lookup, tables);
        if (target) break;
      }
      if (!target) continue;
      if (target.name === table.name) continue; // 自引用不推断

      const targetPK = primaryKeys.get(target.name);
      if (!targetPK) continue;

      const compatible = isTypeCompatible(column.type, targetPK.data_type);
      const key = relationshipKey(table.name, column.name, target.name, targetPK.name);
      if (coveredSources.has(key) || seen.has(key)) continue;
      seen.add(key);

      results.push({
        id: buildRelationshipId(table.name, column.name, target.name, targetPK.name),
        sourceTable: table.name,
        sourceColumn: column.name,
        targetTable: target.name,
        targetColumn: targetPK.name,
        // 类型签名通过 = 高置信；仅命名命中 = 中置信（需用户确认后转实线）。
        confidence: compatible ? "high" : "medium",
        strategy: compatible ? "type_signature" : "naming_convention",
      });
    }
  }

  return results;
}

// ---------------------------------------------------------------- 确认 / 忽略 / 冲突

/**
 * 按用户的确认与忽略记录过滤推断结果。
 * 冲突（同一源列指向多张表）不自动上图，交由用户在面板裁决。
 */
export function filterByStorage(inferred: readonly InferredRelationship[], confirms: readonly string[], ignores: readonly string[]): MatchResult {
  const confirmSet = new Set(confirms);
  const ignoreSet = new Set(ignores);
  const confirmed = inferred.filter((rel) => confirmSet.has(rel.id));
  const candidates = inferred.filter((rel) => !confirmSet.has(rel.id) && !ignoreSet.has(rel.id));

  const conflictList = findConflicts(candidates);
  const conflictIds = new Set(conflictList.map((rel) => rel.id));
  const actionable = candidates.filter((rel) => !conflictIds.has(rel.id));

  return {
    relationships: [...confirmed, ...actionable],
    conflicts: conflictList,
    pending: actionable,
    stats: {
      total: inferred.length,
      high: inferred.filter((rel) => rel.confidence === "high").length,
      medium: inferred.filter((rel) => rel.confidence === "medium").length,
    },
  };
}

/** 同一源表+源列指向多个不同目标表即为冲突。 */
export function findConflicts(relationships: readonly InferredRelationship[]): InferredRelationship[] {
  const bySource = new Map<string, InferredRelationship[]>();
  for (const rel of relationships) {
    const key = `${rel.sourceTable}\u0000${rel.sourceColumn}`;
    const list = bySource.get(key);
    if (list) list.push(rel);
    else bySource.set(key, [rel]);
  }
  const conflicts: InferredRelationship[] = [];
  for (const list of bySource.values()) {
    if (list.length > 1) conflicts.push(...list);
  }
  return conflicts;
}

/** 把推断结果转成可渲染的关系（虚线样式由 kind === "inferred" 决定）。 */
export function toErRelationship(input: InferredRelationship): ErRelationship {
  return {
    id: input.id,
    sourceTable: input.sourceTable,
    sourceColumn: input.sourceColumn,
    targetTable: input.targetTable,
    targetColumn: input.targetColumn,
    fkName: "",
    sourceCardinality: "N",
    targetCardinality: "1",
    kind: "inferred",
  };
}

/** 合并物理外键/自定义关系与推断关系，四元组去重且已有关系优先。 */
export function mergeRelationships(existing: readonly ErRelationship[], inferred: readonly InferredRelationship[]): ErRelationship[] {
  const merged = [...existing];
  const seen = new Set(existing.map((rel) => relationshipKey(rel.sourceTable, rel.sourceColumn, rel.targetTable, rel.targetColumn)));
  for (const rel of inferred) {
    const key = relationshipKey(rel.sourceTable, rel.sourceColumn, rel.targetTable, rel.targetColumn);
    if (seen.has(key)) continue;
    seen.add(key);
    merged.push(toErRelationship(rel));
  }
  return merged;
}

/** 供 UI 展示：某列的匹配置信度说明。 */
export function describeStrategy(rel: InferredRelationship): string {
  return rel.strategy === "type_signature" ? "命名约定 + 类型签名" : "命名约定";
}

export type { ErColumn };
