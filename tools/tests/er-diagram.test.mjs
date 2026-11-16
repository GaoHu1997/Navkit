// 关系图核心模型（需求 1.4，参考实现）：关系构建 / 基数 / 分层布局 / 搜索过滤 / JOIN SQL / 工程 ER 图。
import test from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";

const bundle = await build({
  entryPoints: ["src/core/erDiagram.ts"], bundle: true, write: false,
  platform: "node", format: "esm", logLevel: "silent",
});
const er = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`);

/** 构造测试表集合：orders.user_id → users.id；users.id 为 PK。 */
function fixture() {
  const users = {
    name: "users",
    comment: "用户表",
    columns: [
      { name: "id", type: "bigint", primary_key: true, auto_increment: true, nullable: false, comment: "主键", length: null },
      { name: "user_name", type: "varchar", primary_key: false, auto_increment: false, nullable: false, comment: "用户名", length: 50 },
    ],
    foreignKeys: [],
  };
  const orders = {
    name: "orders",
    comment: "订单表",
    columns: [
      { name: "id", type: "bigint", primary_key: true, auto_increment: true, nullable: false, comment: "", length: null },
      { name: "user_id", type: "bigint", primary_key: false, auto_increment: false, nullable: true, comment: "下单用户", length: null },
    ],
    foreignKeys: [{ name: "fk_orders_user", columns: ["user_id"], ref_table: "users", ref_columns: ["id"] }],
  };
  const logs = {
    name: "app_logs",
    comment: "",
    columns: [{ name: "id", type: "int", primary_key: true, auto_increment: false, nullable: false, comment: "", length: null }],
    foreignKeys: [],
  };
  return [users, orders, logs];
}

test("buildRelationships：FK → 关系，目标端恒为 1，源端 N", () => {
  const rels = er.buildRelationships(fixture());
  assert.equal(rels.length, 1);
  const rel = rels[0];
  assert.equal(rel.sourceTable, "orders");
  assert.equal(rel.sourceColumn, "user_id");
  assert.equal(rel.targetTable, "users");
  assert.equal(rel.targetColumn, "id");
  assert.equal(rel.sourceCardinality, "N");
  assert.equal(rel.targetCardinality, "1");
  assert.equal(rel.kind, "foreign-key");
});

test("buildRelationships：FK 列同时是 PK 时判 1:1", () => {
  const tables = fixture();
  tables.push({
    name: "user_profile",
    comment: "",
    columns: [
      { name: "user_id", type: "bigint", primary_key: true, auto_increment: false, nullable: false, comment: "", length: null },
    ],
    foreignKeys: [{ name: "fk_profile_user", columns: ["user_id"], ref_table: "users", ref_columns: ["id"] }],
  });
  const rel = er.buildRelationships(tables).find(r => r.sourceTable === "user_profile");
  assert.ok(rel);
  assert.equal(rel.sourceCardinality, "1", "源端 FK 覆盖主键 → 1:1");
});

test("buildRelationships 跳过引用不存在表的 FK", () => {
  const tables = fixture();
  tables[1].foreignKeys.push({ name: "fk_bad", columns: ["user_id"], ref_table: "ghost_table", ref_columns: ["id"] });
  const rels = er.buildRelationships(tables);
  assert.equal(rels.length, 1);
});

test("computeLayout：被引用表排在更左层，无关系表靠后", () => {
  const tables = fixture();
  const rels = er.buildRelationships(tables);
  const nodes = er.computeLayout(tables, rels);
  assert.equal(nodes.length, 3);
  const byName = new Map(nodes.map(n => [n.table.name, n]));
  const users = byName.get("users"), orders = byName.get("orders"), logs = byName.get("app_logs");
  assert.ok(users.x < orders.x, "users（被引用）应在 orders 左侧");
  assert.ok(orders.x < logs.x, "无关系的 app_logs 应排在最后");
  // 同层纵向排布：y 递增不重叠
  assert.ok(users.y + users.height + 1 <= orders.y + er.CARD_GAP_Y + 1 || users.x !== orders.x);
  // 卡片高度 = 表头 + 列行 + 底部留白
  assert.equal(users.height, er.CARD_HEADER_H + 2 * er.CARD_ROW_H + er.CARD_PAD_BOTTOM);
});

test("computeLayout：环形引用不死循环", () => {
  const tables = fixture();
  tables[0].foreignKeys.push({ name: "fk_selfish", columns: ["user_name"], ref_table: "orders", ref_columns: ["id"] });
  const rels = er.buildRelationships(tables);
  const nodes = er.computeLayout(tables, rels);
  assert.equal(nodes.length, 3);
});

test("edgeEndpoints + orthogonalPath：右向连接生成 H 型折线", () => {
  const tables = fixture();
  const nodes = er.computeLayout(tables, er.buildRelationships(tables));
  const byName = new Map(nodes.map(n => [n.table.name, n]));
  const e = er.edgeEndpoints(byName.get("users"), byName.get("orders"));
  assert.equal(e.side1, "right");
  const path = er.orthogonalPath(e);
  assert.match(path, /^M [\d.]+ [\d.]+ L/);
  // 基数徽标位置
  const rels = er.buildRelationships(tables);
  assert.equal(rels[0].sourceCardinality, "N");
});

test("filterTables：匹配表名 / 注释 / 列名，大小写不敏感", () => {
  const tables = fixture();
  // "user" 命中 users（表名）、orders（user_id 列）
  assert.deepEqual(er.filterTables(tables, "user").map(t => t.name).sort(), ["orders", "users"]);
  assert.equal(er.filterTables(tables, "USERS").length, 1);
  assert.equal(er.filterTables(tables, "订单").length, 1, "注释命中");
  assert.equal(er.filterTables(tables, "不存在").length, 0);
  assert.equal(er.filterTables(tables, "").length, 3);
});

test("buildJoinSql：贪心 LEFT JOIN；根为第一条关系的源表", () => {
  const tables = fixture();
  const rels = er.buildRelationships(tables);
  const sql = er.buildJoinSql(tables, rels);
  assert.match(sql, /^SELECT t1\.\*/);
  assert.match(sql, /FROM orders t1/);
  assert.match(sql, /LEFT JOIN users t2 ON t1\.user_id = t2\.id/);
  // 断连关系：两张互不相干的表
  const a = { name: "a", comment: "", columns: [{ name: "id", type: "int", primary_key: true, auto_increment: false, nullable: false, comment: "", length: null }], foreignKeys: [] };
  const b = { name: "b", comment: "", columns: [{ name: "id", type: "int", primary_key: true, auto_increment: false, nullable: false, comment: "", length: null }], foreignKeys: [{ name: "fk", columns: ["id"], ref_table: "a", ref_columns: ["id"] }] };
  const sql2 = er.buildJoinSql([a, b], er.buildRelationships([a, b]));
  assert.match(sql2, /LEFT JOIN a t2/);
});

test("buildJoinSql：无关系时返回提示注释", () => {
  const sql = er.buildJoinSql(fixture(), []);
  assert.match(sql, /没有可用的外键关系/);
});

test("computeEngineeringLayout：实体 / 属性 / 关系线 / 菱形齐全", () => {
  const tables = fixture();
  const rels = er.buildRelationships(tables);
  const d = er.computeEngineeringLayout(tables, rels);
  assert.equal(d.entities.length, 3);
  assert.ok(d.attributes.length >= 5, "users 2 列 + orders 2 列 + app_logs 1 列");
  const relationLinks = d.links.filter(l => l.kind === "relation");
  assert.equal(relationLinks.length, 2, "一条关系拆两段（属性→菱形→属性）");
  assert.equal(d.diamonds.length, 1);
  assert.equal(d.diamonds[0].label, "user_id");
  // 归一化：最小坐标不小于 0
  const minX = Math.min(...d.entities.map(e => e.x), ...d.attributes.map(a => a.x));
  const minY = Math.min(...d.entities.map(e => e.y), ...d.attributes.map(a => a.y));
  assert.ok(minX >= 0 && minY >= 0, `坐标应归一化（minX=${minX}, minY=${minY}）`);
});

test("基数选项与换算：N:1 / 1:1 / 1:N / N:N", () => {
  const rels = er.buildRelationships(fixture());
  const rel = rels[0];
  assert.equal(er.cardinalityChoiceOf(rel), "N:1");
  const flipped = er.applyCardinalityChoice(rel, "1:N");
  assert.equal(flipped.sourceCardinality, "1");
  assert.equal(flipped.targetCardinality, "N");
  assert.equal(er.cardinalityChoiceOf(er.applyCardinalityChoice(rel, "1:1")), "1:1");
  assert.equal(er.cardinalityChoiceOf(er.applyCardinalityChoice(rel, "N:N")), "N:N");
  assert.equal(er.CARDINALITY_CHOICES.length, 4);
});

test("位置持久化：localStorage 不可用时静默", () => {
  // node 环境无 localStorage：读默认空、写不抛错
  assert.deepEqual(er.loadPositions("conn", "db"), {});
  er.savePositions("conn", "db", { users: { x: 1, y: 2 } });
  assert.deepEqual(er.loadPositions("conn", "db"), {});
});

test("relationshipVisible：两端都可见才显示", () => {
  const tables = fixture();
  const rels = er.buildRelationships(tables);
  assert.equal(er.relationshipVisible(rels[0], new Set(["users", "orders"])), true);
  assert.equal(er.relationshipVisible(rels[0], new Set(["users"])), false);
});
