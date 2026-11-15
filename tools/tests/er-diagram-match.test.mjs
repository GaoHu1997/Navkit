// 关系图智能匹配 / 边避障路由 / ELK 布局：纯逻辑单测（无 DOM、无数据库）。
// 覆盖移植自 dbx 的匹配策略、路由避障与布局兜底，并锁定相对 dbx 的修正点（四元组去重）。
import test from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

// 产物必须落在仓库内，裸模块名（elkjs）才能解析到 node_modules。
const root = path.resolve(import.meta.dirname, "../..");
const outDir = path.join(root, ".tmp", "er-diagram-match");
await mkdir(outDir, { recursive: true });
let seq = 0;
async function load(entry, { external = [] } = {}) {
  const bundle = await build({
    entryPoints: [entry], bundle: true, write: false, platform: "node",
    format: "esm", logLevel: "silent", external,
  });
  const file = path.join(outDir, `mod-${seq++}.mjs`);
  await writeFile(file, bundle.outputFiles[0].text, "utf8");
  return import(pathToFileURL(file).href);
}

const match = await load("src/core/erDiagramMatch.ts");
const route = await load("src/core/erDiagramRouting.ts");
const layout = await load("src/core/erDiagramLayout.ts", { external: ["elkjs/lib/elk.bundled.js"] });
const er = await load("src/core/erDiagram.ts");

function col(name, type = "bigint", pk = false) {
  return { name, type, primary_key: pk, auto_increment: false, nullable: !pk, comment: null, length: null };
}
function table(name, columns, foreignKeys = []) {
  return { name, schema: "public", comment: "", columns, foreignKeys, indexes: [] };
}

// ---------------------------------------------------------------- 类型兼容

test("isTypeCompatible：同族兼容、跨族拒绝、去参数与别名归一", () => {
  assert.equal(match.isTypeCompatible("bigint", "bigint"), true);
  assert.equal(match.isTypeCompatible("bigint", "int"), true);
  assert.equal(match.isTypeCompatible("VARCHAR(50)", "varchar"), true);
  assert.equal(match.isTypeCompatible("uuid", "varchar"), true);
  // 跨族不算兼容
  assert.equal(match.isTypeCompatible("bigint", "varchar"), false);
  assert.equal(match.isTypeCompatible("int", "text"), false);
  // serial 别名收敛到整数族
  assert.equal(match.isTypeCompatible("serial", "int"), true);
  assert.equal(match.isTypeCompatible("bigserial", "bigint"), true);
  // 空类型不猜
  assert.equal(match.isTypeCompatible("", "int"), false);
});

test("normalizeColumnType：去参数/小写/serial 别名", () => {
  assert.equal(match.normalizeColumnType("VARCHAR(255)"), "varchar");
  assert.equal(match.normalizeColumnType("Decimal(10,2)"), "decimal");
  assert.equal(match.normalizeColumnType("SERIAL"), "int");
  assert.equal(match.normalizeColumnType("bigserial"), "bigint");
});

// ---------------------------------------------------------------- 命名约定

test("extractTableNameCandidates：snake/camel/Pascal/粘连四种写法", () => {
  assert.deepEqual(match.extractTableNameCandidates("user_id"), ["user"]);
  assert.deepEqual(match.extractTableNameCandidates("userId"), ["user"]);
  assert.deepEqual(match.extractTableNameCandidates("user_uuid"), ["user"]);
  assert.deepEqual(match.extractTableNameCandidates("order_pk"), ["order"]);
  assert.deepEqual(match.extractTableNameCandidates("userid"), ["user"]);
  // 不像外键的列不产生候选
  assert.deepEqual(match.extractTableNameCandidates("created_at"), []);
  assert.deepEqual(match.extractTableNameCandidates("id"), []);
  // 残留分隔符必须清掉（`user_id` 的 camel 分支曾产出 `user_`）
  assert.equal(match.extractTableNameCandidates("user_id").some((c) => c.endsWith("_")), false);
});

test("toSnakeCase：camel / Pascal → snake", () => {
  assert.equal(match.toSnakeCase("UserProfile"), "user_profile");
  assert.equal(match.toSnakeCase("userProfile"), "user_profile");
  assert.equal(match.toSnakeCase("HTTPServer"), "http_server");
});

// ---------------------------------------------------------------- 推断主流程

test("inferRelationships：命名 + 类型签名命中，置信度高", () => {
  const tables = [
    table("users", [col("id", "bigint", true), col("name", "varchar")]),
    table("orders", [col("id", "bigint", true), col("user_id", "bigint")]),
  ];
  const inferred = match.inferRelationships(tables, []);
  assert.equal(inferred.length, 1);
  const rel = inferred[0];
  assert.equal(rel.sourceTable, "orders");
  assert.equal(rel.sourceColumn, "user_id");
  assert.equal(rel.targetTable, "users");
  assert.equal(rel.targetColumn, "id");
  assert.equal(rel.confidence, "high");
  assert.equal(rel.strategy, "type_signature");
});

test("inferRelationships：类型不兼容时降级为中置信（仍可人工确认）", () => {
  const tables = [
    table("users", [col("id", "bigint", true)]),
    table("orders", [col("id", "bigint", true), col("user_id", "varchar")]),
  ];
  const inferred = match.inferRelationships(tables, []);
  assert.equal(inferred.length, 1);
  assert.equal(inferred[0].confidence, "medium");
  assert.equal(inferred[0].strategy, "naming_convention");
});

test("inferRelationships：单复数容错 user_id → users", () => {
  const tables = [
    table("users", [col("id", "int", true)]),
    table("orders", [col("id", "int", true), col("user_id", "int")]),
  ];
  const inferred = match.inferRelationships(tables, []);
  assert.equal(inferred.length, 1);
  assert.equal(inferred[0].targetTable, "users");
});

test("inferRelationships：目标表不存在则不猜", () => {
  const tables = [table("orders", [col("id", "bigint", true), col("ghost_id", "bigint")])];
  assert.equal(match.inferRelationships(tables, []).length, 0);
});

test("inferRelationships：自引用不推断", () => {
  const tables = [table("nodes", [col("id", "bigint", true), col("node_id", "bigint")])];
  assert.equal(match.inferRelationships(tables, []).length, 0);
});

test("inferRelationships：主键列本身不参与推断", () => {
  const tables = [
    table("users", [col("id", "bigint", true)]),
    // user_id 同时是主键（1:1 主键共享），不应被推断成虚拟外键
    table("profiles", [col("user_id", "bigint", true)]),
  ];
  assert.equal(match.inferRelationships(tables, []).length, 0);
});

test("inferRelationships：已有物理外键的源列不再推断（避免重复出边）", () => {
  const tables = [
    table("users", [col("id", "bigint", true)]),
    table("orders", [col("id", "bigint", true), col("user_id", "bigint")], [
      { name: "fk_orders_user", columns: ["user_id"], ref_table: "users", ref_columns: ["id"] },
    ]),
  ];
  const existing = er.buildRelationships(tables);
  assert.equal(existing.length, 1);
  // dbx 原实现此处会产出重复边；本实现必须为空
  assert.equal(match.inferRelationships(tables, existing).length, 0);
});

test("mergeRelationships：四元组去重，物理外键优先于推断", () => {
  const fk = { id: "fk1", sourceTable: "orders", sourceColumn: "user_id", targetTable: "users", targetColumn: "id", fkName: "fk_orders_user", sourceCardinality: "N", targetCardinality: "1", kind: "foreign-key" };
  const inferred = [{ id: "inferred:orders:user_id:users:id", sourceTable: "orders", sourceColumn: "user_id", targetTable: "users", targetColumn: "id", confidence: "high", strategy: "type_signature" }];
  const merged = match.mergeRelationships([fk], inferred);
  assert.equal(merged.length, 1);
  assert.equal(merged[0].kind, "foreign-key");
});

test("toErRelationship：推断关系标记为 inferred，基数为 N:1", () => {
  const rel = match.toErRelationship({ id: "x", sourceTable: "orders", sourceColumn: "user_id", targetTable: "users", targetColumn: "id", confidence: "high", strategy: "type_signature" });
  assert.equal(rel.kind, "inferred");
  assert.equal(rel.sourceCardinality, "N");
  assert.equal(rel.targetCardinality, "1");
  assert.equal(rel.fkName, "");
});

// ---------------------------------------------------------------- 确认 / 忽略 / 冲突

test("filterByStorage：确认项保留、忽略项剔除", () => {
  const inferred = [
    { id: "a", sourceTable: "orders", sourceColumn: "user_id", targetTable: "users", targetColumn: "id", confidence: "high", strategy: "type_signature" },
    { id: "b", sourceTable: "orders", sourceColumn: "item_id", targetTable: "items", targetColumn: "id", confidence: "high", strategy: "type_signature" },
  ];
  const result = match.filterByStorage(inferred, ["a"], ["b"]);
  assert.deepEqual(result.relationships.map((r) => r.id), ["a"]);
  assert.equal(result.stats.total, 2);
});

test("findConflicts：同一源列指向多表判冲突，且冲突项不上图", () => {
  const inferred = [
    { id: "a", sourceTable: "orders", sourceColumn: "type_id", targetTable: "types", targetColumn: "id", confidence: "high", strategy: "type_signature" },
    { id: "b", sourceTable: "orders", sourceColumn: "type_id", targetTable: "user_types", targetColumn: "id", confidence: "high", strategy: "type_signature" },
  ];
  const result = match.filterByStorage(inferred, [], []);
  assert.equal(result.conflicts.length, 2);
  assert.equal(result.relationships.length, 0);
});

// ---------------------------------------------------------------- 边避障路由

function rect(id, x, y, width = 260, height = 120) {
  return { id, x, y, width, height };
}

test("routeOrthogonalAroundObstacles：无障碍时走最短正交折线", () => {
  // 端点卡片不得覆盖起点/终点，否则会被当成遮挡
  const points = route.routeOrthogonalAroundObstacles({
    source: { x: 100, y: 100 }, target: { x: 600, y: 100 },
    obstacles: [rect("a", 40, 80, 60, 40), rect("b", 600, 80, 60, 40)],
    endpointIds: ["a", "b"],
  });
  assert.ok(points);
  // 两端同高且无遮挡：最短解就是一条直线
  assert.equal(points.length, 2);
  assert.deepEqual(points[0], { x: 100, y: 100 });
  assert.deepEqual(points[points.length - 1], { x: 600, y: 100 });
});

test("routeOrthogonalAroundObstacles：中间有卡片时绕开，不穿卡片", () => {
  const blocker = rect("mid", 300, 40, 260, 160);
  const points = route.routeOrthogonalAroundObstacles({
    source: { x: 100, y: 120 }, target: { x: 700, y: 120 },
    obstacles: [rect("a", 40, 80, 60, 40), rect("b", 660, 80, 60, 40), blocker],
    endpointIds: ["a", "b"],
  });
  assert.ok(points, "应能找出绕行通道");
  // 绕行路径必须避开 blocker 本体（收缩 12px 后仍不相交）
  const inner = { ...blocker, x: blocker.x + 6, y: blocker.y + 6, width: blocker.width - 12, height: blocker.height - 12 };
  assert.equal(route.pathHitsObstacles(points, [inner]), false);
  // 且全程正交
  assert.equal(route.isOrthogonalPolyline(points), true);
});

test("routeOrthogonalAroundObstacles：四周被围死时返回 null（由调用方回退）", () => {
  // 目标卡片被四面高墙完全封死（墙与端点卡片边缘重叠，无任何缝隙）
  const points = route.routeOrthogonalAroundObstacles({
    source: { x: 100, y: 100 }, target: { x: 200, y: 100 },
    obstacles: [
      rect("a", 80, 90, 20, 20),
      rect("b", 200, 90, 20, 20),
      rect("top", -2000, -600, 6000, 690),
      rect("bottom", -2000, 110, 6000, 600),
      rect("left", -600, -2000, 680, 6000),
      rect("right", 200, -2000, 600, 6000),
    ],
    endpointIds: ["a", "b"],
  });
  assert.equal(points, null);
});

test("routeEdge：路由失败时仍返回可渲染的正交折线", () => {
  const points = route.routeEdge({
    source: { x: 100, y: 100 }, target: { x: 200, y: 100 },
    obstacles: [rect("a", 60, 60, 40, 80), rect("b", 200, 60, 40, 80), rect("wall", 120, -500, 10, 2000)],
    endpointIds: ["a", "b"],
  });
  assert.ok(points.length >= 2);
  assert.equal(route.isOrthogonalPolyline(points), true);
});

test("pointsToSvgPath / pointAlongPolyline：路径与中点计算", () => {
  const points = [{ x: 0, y: 0 }, { x: 100, y: 0 }, { x: 100, y: 100 }];
  assert.equal(route.pointsToSvgPath(points), "M 0 0 L 100 0 L 100 100");
  const mid = route.midpointAlongPolyline(points);
  // 总长 200，中点落在 (100,0)
  assert.equal(Math.round(mid.x), 100);
  assert.equal(Math.round(mid.y), 0);
});

test("collapseColinearPoints：去掉同轴冗余点", () => {
  const collapsed = route.collapseColinearPoints([{ x: 0, y: 0 }, { x: 50, y: 0 }, { x: 100, y: 0 }, { x: 100, y: 50 }]);
  assert.deepEqual(collapsed, [{ x: 0, y: 0 }, { x: 100, y: 0 }, { x: 100, y: 50 }]);
});

// ---------------------------------------------------------------- 布局

test("computeGridLayout：无重叠、按连通分量聚簇", () => {
  const nodes = [
    { table: table("users", [col("id", "bigint", true)]), width: 260, height: 60 },
    { table: table("orders", [col("id", "bigint", true)]), width: 260, height: 60 },
    { table: table("lonely", [col("id", "bigint", true)]), width: 260, height: 60 },
  ];
  const positions = layout.computeGridLayout(nodes, [{ sourceTable: "orders", targetTable: "users" }], 1200);
  assert.equal(Object.keys(positions).length, 3);
  // 无重叠
  const entries = Object.entries(positions);
  for (let i = 0; i < entries.length; i++) {
    for (let j = i + 1; j < entries.length; j++) {
      const [, a] = entries[i];
      const [, b] = entries[j];
      const overlap = a.x < b.x + 260 && b.x < a.x + 260 && a.y < b.y + 60 && b.y < a.y + 60;
      assert.equal(overlap, false, `${entries[i][0]} 与 ${entries[j][0]} 重叠`);
    }
  }
});

test("orderByConnectivity：有关系的表相邻，孤立表排最后", () => {
  const nodes = ["alpha", "beta", "lonely", "gamma"].map((name) => ({ table: table(name, [col("id", "bigint", true)]), width: 260, height: 60 }));
  const ordered = layout.orderByConnectivity(nodes, [{ sourceTable: "gamma", targetTable: "beta" }]);
  assert.deepEqual(ordered.map((n) => n.table.name), ["beta", "gamma", "alpha", "lonely"]);
});

test("computeElkLayout：空输入不执行布局", async () => {
  const result = await layout.computeElkLayout([], [], { direction: "LR" });
  assert.equal(result.applied, false);
  assert.deepEqual(result.positions, {});
});

test("buildElkOptions：方向映射与交叉最小化参数", () => {
  assert.equal(layout.buildElkOptions("LR")["elk.direction"], "RIGHT");
  assert.equal(layout.buildElkOptions("TB")["elk.direction"], "DOWN");
  assert.equal(layout.buildElkOptions("RL")["elk.direction"], "LEFT");
  assert.equal(layout.buildElkOptions("BT")["elk.direction"], "UP");
  const opts = layout.buildElkOptions("LR");
  assert.equal(opts["elk.algorithm"], "layered");
  assert.equal(opts["elk.layered.crossingMinimization"], "LAYER_SWEEP");
  assert.equal(opts["elk.layered.edgeRouting"], "ORTHOGONAL");
});
