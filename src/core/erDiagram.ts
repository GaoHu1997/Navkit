/**
 * 关系图（ER Diagram）核心模型 —— 纯逻辑，无 DOM 依赖（需求 1.4，参考实现）。
 *
 *  · 表卡片图（表结构图）：表为卡片、外键为折线边、乌鸦脚基数标记
 *  · 工程 ER 图：表为矩形、列为椭圆、关系为菱形的列级图
 *  · 分层自动布局：按 FK 方向的最长路径分层（LR），同层纵向排布
 *  · 搜索过滤：匹配表名 / 注释 / 列名
 *  · JOIN SQL 生成：以第一条关系为根的贪心 BFS LEFT JOIN
 */

export interface ErColumn {
  name: string;
  type: string;
  primary_key: boolean;
  auto_increment: boolean;
  nullable: boolean;
  comment: string | null;
  length: number | null;
}

export interface ErForeignKey {
  name: string;
  columns: string[];
  ref_table: string;
  ref_schema?: string | null;
  ref_columns: string[];
}

export interface ErTable {
  name: string;
  schema?: string;
  comment: string;
  columns: ErColumn[];
  foreignKeys: ErForeignKey[];
  /** Only fully represented, unfiltered unique keys may prove a 1:1 FK. */
  indexes?: { columns: string[]; unique: boolean; primary?: boolean }[];
}

export type Cardinality = "1" | "N";

export interface ErRelationship {
  id: string;
  /** FK 所在表。 */
  sourceTable: string;
  sourceColumn: string;
  /** 被引用表。 */
  targetTable: string;
  targetColumn: string;
  fkName: string;
  sourceCardinality: Cardinality;
  targetCardinality: Cardinality;
  /** foreign-key：物理外键（实线）；custom：用户建模；inferred：自动匹配的虚拟外键（虚线）。 */
  kind: "foreign-key" | "custom" | "inferred";
}

/** 表卡片节点（布局结果）。 */
export interface DiagramNode {
  table: ErTable;
  x: number;
  y: number;
  width: number;
  height: number;
}

// ---------------------------------------------------------------- 尺寸常量（渲染与布局共用）

export const CARD_WIDTH = 260;
export const CARD_HEADER_H = 34;
export const CARD_ROW_H = 22;
export const CARD_GAP_X = 120;
export const CARD_GAP_Y = 60;
export const CARD_BODY_MAX_H = 240;
// Existing .er-card has a 1px border on both sides, not an 8px bottom pad.
export const CARD_PAD_BOTTOM = 2;

/** Use the existing CSS card geometry, including its clipped column body. */
export function cardHeight(table: ErTable): number {
  return CARD_HEADER_H + Math.min(table.columns.length * CARD_ROW_H, CARD_BODY_MAX_H) + CARD_PAD_BOTTOM;
}

// ---------------------------------------------------------------- 关系构建

/** 从表外键构建关系（目标端恒为 1；源端为 PK/唯一列时判 1，否则 N）。 */
export function buildRelationships(tables: ErTable[]): ErRelationship[] {
  // JSON tuples preserve dotted identifiers and exact namespace identity.
  const identity = (schema: string, name: string): string => JSON.stringify([schema, name]);
  const byIdentity = new Map(tables.map((t) => [identity(t.schema ?? "", t.name), t]));
  const out: ErRelationship[] = [];
  for (const table of tables) {
    const pkColumns = table.columns.filter((c) => c.primary_key).map((c) => c.name);
    const uniqueKeys = [pkColumns, ...(table.indexes ?? []).filter((i) => i.unique || i.primary).map((i) => i.columns)];
    for (const fk of table.foreignKeys) {
      const target = byIdentity.get(identity(fk.ref_schema || table.schema || "", fk.ref_table));
      if (!target) continue;
      // 参考实现: unique key ⊆ FK; a subset of a composite PK is not itself unique.
      const sourceColumns = new Set(fk.columns);
      const uniqueSource = uniqueKeys.some((key) => key.length > 0 && key.every((c) => sourceColumns.has(c)));
      const sourceColumn = fk.columns[0] ?? "";
      const targetColumn = fk.ref_columns[0] ?? target.columns.find((c) => c.primary_key)?.name ?? "id";
      out.push({
        id: JSON.stringify([table.schema ?? "", table.name, fk.name || "foreign_key", fk.columns, target.schema ?? "", target.name, fk.ref_columns]),
        sourceTable: table.name,
        sourceColumn,
        targetTable: fk.ref_table,
        targetColumn,
        fkName: fk.name || "",
        sourceCardinality: uniqueSource ? "1" : "N",
        targetCardinality: "1",
        kind: "foreign-key",
      });
    }
  }
  return out;
}

/** 关系是否两端都在给定表集合内。 */
export function relationshipVisible(rel: ErRelationship, names: Set<string>): boolean {
  return names.has(rel.sourceTable) && names.has(rel.targetTable);
}

// ---------------------------------------------------------------- 搜索过滤

/** 按关键词过滤表（匹配表名 / 注释 / 列名 / 列类型，大小写不敏感）。 */
export function filterTables(tables: ErTable[], query: string): ErTable[] {
  const kw = query.trim().toLowerCase();
  if (!kw) return tables;
  return tables.filter(
    (t) =>
      t.name.toLowerCase().includes(kw) ||
      (t.comment ?? "").toLowerCase().includes(kw) ||
      t.columns.some((c) => c.name.toLowerCase().includes(kw) || c.type.toLowerCase().includes(kw)),
  );
}

/** 过滤后保留的表集合（含直接关联表名，便于展示上下文）。 */
export function filterTableNames(tables: ErTable[], query: string): Set<string> {
  return new Set(filterTables(tables, query).map((t) => t.name));
}

// ---------------------------------------------------------------- 自动布局（分层 LR）

/**
 * 分层布局：FK 方向 target→source 的最长路径分层（无环处理：回边不计入层深）。
 * 无关系的表排到最后。返回节点位置。
 */
export function computeLayout(tables: ErTable[], relationships: ErRelationship[]): DiagramNode[] {
  const byName = new Map(tables.map((t) => [t.name, t]));
  const names = tables.map((t) => t.name);
  const edges = relationships
    .filter((r) => r.sourceTable !== r.targetTable && byName.has(r.sourceTable) && byName.has(r.targetTable))
    .map((r) => ({ from: r.targetTable, to: r.sourceTable })); // 被引用表在左

  // 入边计数与邻接
  const layerOf = new Map<string, number>();
  const incoming = new Map<string, Set<string>>();
  const outgoing = new Map<string, Set<string>>();
  for (const name of names) {
    incoming.set(name, new Set());
    outgoing.set(name, new Set());
    layerOf.set(name, 0);
  }
  for (const e of edges) {
    incoming.get(e.to)?.add(e.from);
    outgoing.get(e.from)?.add(e.to);
  }

  // Kahn 拓扑分层（处理环：跳过已访问的）。注意会改 incoming/outgoing，度数要用原始边另算。
  const remaining = new Set(names);
  const queue = names.filter((n) => incoming.get(n)!.size === 0);
  for (const n of queue) remaining.delete(n);
  let processed = 0;
  while (processed < queue.length) {
    const current = queue[processed++];
    const layer = layerOf.get(current) ?? 0;
    for (const next of outgoing.get(current) ?? []) {
      if (!remaining.has(next)) continue;
      layerOf.set(next, Math.max(layerOf.get(next) ?? 0, layer + 1));
      incoming.get(next)!.delete(current);
      if (incoming.get(next)!.size === 0) {
        remaining.delete(next);
        queue.push(next);
      }
    }
  }
  // 有环残留：排到已有最大层之后
  if (remaining.size) {
    let maxLayer = 0;
    for (const layer of layerOf.values()) maxLayer = Math.max(maxLayer, layer);
    for (const n of remaining) layerOf.set(n, maxLayer + 1);
  }

  // 度数按原始边统计（Kahn 过程会消耗边，不能复用 incoming/outgoing）
  const degree = new Map<string, number>();
  for (const name of names) degree.set(name, 0);
  for (const e of edges) {
    degree.set(e.from, (degree.get(e.from) ?? 0) + 1);
    degree.set(e.to, (degree.get(e.to) ?? 0) + 1);
  }
  let maxLayer = 0;
  for (const layer of layerOf.values()) maxLayer = Math.max(maxLayer, layer);
  const layers: string[][] = Array.from({ length: maxLayer + 1 }, () => []);
  const isolated: string[] = [];
  for (const name of names) {
    if ((degree.get(name) ?? 0) === 0) isolated.push(name);
    else layers[layerOf.get(name) ?? maxLayer].push(name);
  }
  for (const layer of layers) {
    layer.sort((a, b) => (degree.get(b) ?? 0) - (degree.get(a) ?? 0) || a.localeCompare(b));
  }
  const orderedLayers = [...layers, isolated];

  // 摆放
  const nodes: DiagramNode[] = [];
  let x = 40;
  for (const layer of orderedLayers) {
    let y = 40;
    let layerWidth = 0;
    for (const name of layer) {
      const table = byName.get(name);
      if (!table) continue;
      const height = cardHeight(table);
      nodes.push({ table, x, y, width: CARD_WIDTH, height });
      y += height + CARD_GAP_Y;
      layerWidth = Math.max(layerWidth, CARD_WIDTH);
    }
    x += layerWidth + CARD_GAP_X;
  }
  return nodes;
}

// ---------------------------------------------------------------- 边路由

/** 边端点：从卡片边缘引出（左右侧中点）。 */
export function edgeEndpoints(
  source: DiagramNode,
  target: DiagramNode,
): { x1: number; y1: number; x2: number; y2: number; side1: "left" | "right" | "top" | "bottom" } {
  const sRight = source.x + source.width;
  const tRight = target.x + target.width;
  const sy = source.y + source.height / 2;
  const ty = target.y + target.height / 2;
  if (sRight <= target.x) {
    return { x1: sRight, y1: sy, x2: target.x, y2: ty, side1: "right" };
  }
  if (tRight <= source.x) {
    return { x1: source.x, y1: sy, x2: tRight, y2: ty, side1: "left" };
  }
  // 上下重叠：走顶部/底部
  if (source.y >= target.y + target.height) {
    return { x1: source.x + source.width / 2, y1: source.y, x2: target.x + target.width / 2, y2: target.y + target.height, side1: "top" };
  }
  if (target.y >= source.y + source.height) {
    return { x1: source.x + source.width / 2, y1: source.y + source.height, x2: target.x + target.width / 2, y2: target.y, side1: "bottom" };
  }
  // 完全重叠：右侧兜底
  return { x1: sRight, y1: sy, x2: target.x, y2: ty, side1: "right" };
}

/** 正交折线路径（带一点中段偏移），返回 SVG path d。 */
export function orthogonalPath(
  e: ReturnType<typeof edgeEndpoints>,
  stub = 24,
): string {
  const { x1, y1, x2, y2, side1 } = e;
  const dir = side1 === "left" ? -1 : side1 === "right" ? 1 : 0;
  if (dir !== 0) {
    const mx1 = x1 + dir * stub;
    const mx2 = x2 + (x2 > x1 ? -stub : stub);
    if (Math.abs(y1 - y2) < 1) {
      return `M ${x1} ${y1} L ${x2} ${y2}`;
    }
    if ((dir === 1 && mx1 <= mx2) || (dir === -1 && mx1 >= mx2)) {
      // H 型三段
      return `M ${x1} ${y1} L ${mx1} ${y1} L ${mx1} ${y2} L ${mx2} ${y2} L ${x2} ${y2}`;
    }
    // 回折：走中间竖线
    const midX = (x1 + x2) / 2;
    return `M ${x1} ${y1} L ${midX} ${y1} L ${midX} ${y2} L ${x2} ${y2}`;
  }
  // 竖直边：中点横移
  const midY = (y1 + y2) / 2;
  return `M ${x1} ${y1} L ${x1} ${midY} L ${x2} ${midY} L ${x2} ${y2}`;
}

// ---------------------------------------------------------------- JOIN SQL

/** 生成 JOIN SQL（贪心 BFS 连接；不连通关系以注释跳过）。 */
export function buildJoinSql(tables: ErTable[], relationships: ErRelationship[]): string {
  if (!relationships.length) return "-- 没有可用的外键关系";
  const byName = new Map(tables.map((t) => [t.name, t]));
  const joined = new Set<string>();
  const lines: string[] = [];
  const root = relationships[0];
  const aliases = new Map<string, string>();
  const aliasOf = (table: string): string => {
    if (!aliases.has(table)) aliases.set(table, `t${aliases.size + 1}`);
    return aliases.get(table)!;
  };
  aliasOf(root.sourceTable);
  aliasOf(root.targetTable);
  joined.add(root.sourceTable);
  joined.add(root.targetTable);
  lines.push(
    `LEFT JOIN ${root.targetTable} ${aliasOf(root.targetTable)} ON ${aliasOf(root.sourceTable)}.${root.sourceColumn} = ${aliasOf(root.targetTable)}.${root.targetColumn}`,
  );
  const pending = relationships.slice(1);
  const disconnected: ErRelationship[] = [];
  for (const rel of pending) {
    const srcIn = joined.has(rel.sourceTable);
    const dstIn = joined.has(rel.targetTable);
    if (srcIn && dstIn) continue; // 已连通（多条件合并为单 JOIN 即可表达）
    if (srcIn) {
      aliasOf(rel.targetTable);
      joined.add(rel.targetTable);
      lines.push(
        `LEFT JOIN ${rel.targetTable} ${aliasOf(rel.targetTable)} ON ${aliasOf(rel.sourceTable)}.${rel.sourceColumn} = ${aliasOf(rel.targetTable)}.${rel.targetColumn}`,
      );
    } else if (dstIn) {
      aliasOf(rel.sourceTable);
      joined.add(rel.sourceTable);
      lines.push(
        `LEFT JOIN ${rel.sourceTable} ${aliasOf(rel.sourceTable)} ON ${aliasOf(rel.sourceTable)}.${rel.sourceColumn} = ${aliasOf(rel.targetTable)}.${rel.targetColumn}`,
      );
    } else {
      disconnected.push(rel);
    }
  }
  const allTables = [...aliases.keys()].filter((t) => byName.has(t));
  const selectList = allTables.map((t) => `${aliasOf(t)}.*`).join(", ");
  const rootTable = [...aliases.keys()][0];
  const disconnectedNotes = disconnected.length
    ? `\n-- 另有 ${disconnected.length} 条与当前连接集不连通的关系已跳过`
    : "";
  return `SELECT ${selectList}\nFROM ${rootTable} ${aliasOf(rootTable)}\n${lines.join("\n")}${disconnectedNotes}`;
}

// ---------------------------------------------------------------- 工程 ER 图（列级图）

export interface EngineeringEntity {
  table: ErTable;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface EngineeringAttribute {
  table: string;
  column: string;
  isPk: boolean;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface EngineeringLink {
  /** attribute 中心连线 或 关系两段线。 */
  x1: number;
  y1: number;
  x2: number;
  y2: number;
  kind: "attribute" | "relation";
}

export interface EngineeringDiagram {
  entities: EngineeringEntity[];
  attributes: EngineeringAttribute[];
  links: EngineeringLink[];
  /** 关系菱形中心（供渲染菱形）。 */
  diamonds: { x: number; y: number; label: string }[];
  width: number;
  height: number;
}

const ENG_ENTITY_W = 150;
const ENG_ENTITY_H = 52;
const ENG_ATTR_H = 30;
const ENG_GAP = 44;
const ENG_ROW_GAP = 120;
const ENG_MAX_PER_SIDE = 3;

/** 工程 ER 图布局：列环绕表（上下左右四侧），关系线经菱形。 */
export function computeEngineeringLayout(tables: ErTable[], relationships: ErRelationship[]): EngineeringDiagram {
  const entities: EngineeringEntity[] = [];
  const attributes: EngineeringAttribute[] = [];
  const links: EngineeringLink[] = [];
  const diamonds: EngineeringDiagram["diamonds"] = [];
  const attrCenter = new Map<string, { x: number; y: number }>();
  const entityCenter = new Map<string, { x: number; y: number }>();
  const attributeKey = (table: string, column: string): string => JSON.stringify([table, column]);

  const baseX = 80;
  let cursorY = 80;

  for (const table of tables) {
    const cols = table.columns.slice(0, 12); // 防爆量：最多 12 列环绕
    const ex = baseX + ENG_ENTITY_W + ENG_GAP;
    const entity: EngineeringEntity = {
      table,
      x: ex,
      y: cursorY,
      width: ENG_ENTITY_W,
      height: ENG_ENTITY_H,
    };
    entities.push(entity);
    const center = { x: ex + ENG_ENTITY_W / 2, y: cursorY + ENG_ENTITY_H / 2 };
    entityCenter.set(table.name, center);

    // 列分四侧：上 / 右 / 下 / 左
    const sides: { top: ErColumn[]; right: ErColumn[]; bottom: ErColumn[]; left: ErColumn[] } = {
      top: [], right: [], bottom: [], left: [],
    };
    cols.forEach((col, i) => {
      const slot = i % 4;
      if (slot === 0) sides.top.push(col);
      else if (slot === 1) sides.right.push(col);
      else if (slot === 2) sides.bottom.push(col);
      else sides.left.push(col);
    });

    const attrWidth = (col: ErColumn): number => Math.min(156, Math.max(96, col.name.length * 9 + 34));
    const place = (col: ErColumn, x: number, y: number): void => {
      const w = attrWidth(col);
      const attr: EngineeringAttribute = {
        table: table.name,
        column: col.name,
        isPk: col.primary_key,
        x,
        y,
        width: w,
        height: ENG_ATTR_H,
      };
      attributes.push(attr);
      attrCenter.set(attributeKey(table.name, col.name), { x: x + w / 2, y: y + ENG_ATTR_H / 2 });
      links.push({ x1: center.x, y1: center.y, x2: x + w / 2, y2: y + ENG_ATTR_H / 2, kind: "attribute" });
    };

    // 上侧
    let topWidth = 0;
    sides.top.forEach((col) => {
      const w = attrWidth(col);
      place(col, entity.x + ENG_ENTITY_W / 2 - (sides.top.reduce((s, c) => s + attrWidth(c) + 16, -16) / 2) + topWidth, entity.y - ENG_GAP - ENG_ATTR_H);
      topWidth += w + 16;
    });
    // 下侧
    let bottomWidth = 0;
    sides.bottom.forEach((col) => {
      const w = attrWidth(col);
      place(col, entity.x + ENG_ENTITY_W / 2 - (sides.bottom.reduce((s, c) => s + attrWidth(c) + 16, -16) / 2) + bottomWidth, entity.y + entity.height + ENG_GAP);
      bottomWidth += w + 16;
    });
    // 右侧（纵向）
    sides.right.forEach((col, i) => {
      if (i >= ENG_MAX_PER_SIDE) return;
      place(col, entity.x + entity.width + ENG_GAP, entity.y + i * (ENG_ATTR_H + 14) - (ENG_ATTR_H + 14));
    });
    // 左侧
    sides.left.forEach((col, i) => {
      if (i >= ENG_MAX_PER_SIDE) return;
      const w = attrWidth(col);
      place(col, entity.x - ENG_GAP - w, entity.y + i * (ENG_ATTR_H + 14) - (ENG_ATTR_H + 14));
    });

    // 该表（含其注释高度）占位：列数最多的侧决定纵向高度
    const vertical = Math.max(
      sides.top.length + sides.bottom.length > 0 ? 0 : 0,
      (sides.right.length ? sides.right.length * (ENG_ATTR_H + 14) : 0),
      (sides.left.length ? sides.left.length * (ENG_ATTR_H + 14) : 0),
      ENG_ENTITY_H + (sides.bottom.length ? ENG_GAP + ENG_ATTR_H : 0) + (sides.top.length ? ENG_GAP + ENG_ATTR_H : 0),
    );
    cursorY += Math.max(ENG_ENTITY_H, vertical) + ENG_ROW_GAP;
  }

  // 关系：源属性中心 → 菱形 → 目标属性中心
  for (const rel of relationships) {
    // 参考实现 fallback: clipped attributes must not silently delete real FK links.
    const s = attrCenter.get(attributeKey(rel.sourceTable, rel.sourceColumn)) ?? entityCenter.get(rel.sourceTable);
    const t = attrCenter.get(attributeKey(rel.targetTable, rel.targetColumn)) ?? entityCenter.get(rel.targetTable);
    if (!s || !t) continue;
    const mx = (s.x + t.x) / 2;
    const my = (s.y + t.y) / 2;
    diamonds.push({ x: mx, y: my, label: rel.sourceColumn || rel.fkName });
    links.push({ x1: s.x, y1: s.y, x2: mx, y2: my, kind: "relation" });
    links.push({ x1: mx, y1: my, x2: t.x, y2: t.y, kind: "relation" });
  }

  // 整体归一化
  let minX = 80;
  let minY = 80;
  for (const item of [...entities, ...attributes]) {
    minX = Math.min(minX, item.x);
    minY = Math.min(minY, item.y);
  }
  const shiftX = 80 - minX;
  const shiftY = 80 - minY;
  for (const e of entities) {
    e.x += shiftX;
    e.y += shiftY;
  }
  for (const a of attributes) {
    a.x += shiftX;
    a.y += shiftY;
  }
  for (const l of links) {
    l.x1 += shiftX;
    l.y1 += shiftY;
    l.x2 += shiftX;
    l.y2 += shiftY;
  }
  for (const d of diamonds) {
    d.x += shiftX;
    d.y += shiftY;
  }
  let width = 400;
  let height = 300;
  for (const item of [...entities, ...attributes]) {
    width = Math.max(width, item.x + item.width + 80);
    height = Math.max(height, item.y + item.height + 80);
  }
  return { entities, attributes, links, diamonds, width, height };
}

// ---------------------------------------------------------------- 基数选择（建模关系面板）

/** 基数选项（参考实现：1:1 / 1:N / N:1 / N:N）。 */
export const CARDINALITY_CHOICES = [
  { value: "N:1", label: "源 N · 目标 1" },
  { value: "1:1", label: "源 1 · 目标 1" },
  { value: "1:N", label: "源 1 · 目标 N" },
  { value: "N:N", label: "源 N · 目标 N" },
] as const;

/** 把 "N:1" 等选择写回关系的两端基数。 */
export function applyCardinalityChoice(rel: ErRelationship, choice: string): ErRelationship {
  const [source, target] = choice.split(":");
  return {
    ...rel,
    sourceCardinality: source === "1" ? "1" : "N",
    targetCardinality: target === "1" ? "1" : "N",
  };
}

export function cardinalityChoiceOf(rel: ErRelationship): string {
  return `${rel.sourceCardinality}:${rel.targetCardinality}`;
}

// ---------------------------------------------------------------- 位置持久化

const POSITIONS_KEY = "navkit:diagram:positions:v1";

/** 读某个连接库的位置记录。 */
export function loadPositions(connId: string, database: string): Record<string, { x: number; y: number }> {
  try {
    const raw = localStorage.getItem(`${POSITIONS_KEY}:${connId}:${database}`);
    return raw ? (JSON.parse(raw) as Record<string, { x: number; y: number }>) : {};
  } catch {
    return {};
  }
}

export function savePositions(
  connId: string,
  database: string,
  positions: Record<string, { x: number; y: number }>,
): void {
  try {
    localStorage.setItem(`${POSITIONS_KEY}:${connId}:${database}`, JSON.stringify(positions));
  } catch {
    /* localStorage 不可用时静默 */
  }
}
