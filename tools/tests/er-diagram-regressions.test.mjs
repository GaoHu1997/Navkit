import test from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";

async function load(entry) {
  const bundle = await build({ entryPoints: [entry], bundle: true, write: false, platform: "node", format: "esm", logLevel: "silent" });
  return import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`);
}
const er = await load("src/core/erDiagram.ts");
const cache = await load("src/core/erDiagramMetadata.ts");
function column(name, pk = false) {
  return { name, type: "int", primary_key: pk, auto_increment: false, nullable: false, comment: null, length: null };
}
function table(name, schema = "public", columns = [column("id", true)]) {
  return { name, schema, comment: "", columns, foreignKeys: [] };
}
function deferred() {
  let resolve, reject;
  const promise = new Promise((a, b) => { resolve = a; reject = b; });
  return { promise, resolve, reject };
}
let nextId = 0;
function connection() {
  return { id: `diagram-regression-${++nextId}`, name: "test", kind: "postgresql", host: "localhost", port: 5432,
    user: "test", password: "encrypted", database: "db", file: "", params: "", group: "", read_only: false, keep_alive: false };
}
function metadata(name = "items", failures = []) {
  return { tables: [{ table: name, database: "public", columns: [column("id", true)], indexes: [], foreign_keys: [],
    comment: null, engine: null, collation: null, row_count: null }], failures, limitations: [] };
}

test("wide cards use the existing clipped CSS geometry, not all columns", () => {
  const wide = table("wide", "public", Array.from({ length: 100 }, (_, i) => column(`c${i}`)));
  assert.equal(er.cardHeight(wide), 276);
  assert.equal(er.cardHeight(table("empty", "public", [])), 36);
  assert.equal(er.cardHeight(table("short")), 58);
});

test("FK matching honors ref_schema and preserves dotted identifiers", () => {
  const parent = table("parent");
  const child = table("child");
  child.foreignKeys.push({ name: "fk", columns: ["id"], ref_schema: "other", ref_table: "parent", ref_columns: ["id"] });
  assert.equal(er.buildRelationships([parent, child]).length, 0, "must not bind other.parent to public.parent");
  const dotted = table("parent.name", "schema.with.dots");
  child.foreignKeys[0].ref_schema = dotted.schema;
  child.foreignKeys[0].ref_table = dotted.name;
  assert.equal(er.buildRelationships([dotted, child])[0].targetTable, dotted.name);
  child.foreignKeys[0].ref_schema = "PUBLIC";
  child.foreignKeys[0].ref_table = "parent";
  assert.equal(er.buildRelationships([parent, child]).length, 0, "quoted namespace identity is case sensitive");
});

test("a subset of composite PK is N:1; FK covering a unique key is 1:1", () => {
  const parent = table("parent");
  const child = table("child", "public", [column("id", true), column("tenant", true)]);
  child.foreignKeys.push({ name: "fk", columns: ["id"], ref_table: "parent", ref_columns: ["id"] });
  assert.equal(er.buildRelationships([parent, child])[0].sourceCardinality, "N");
  child.foreignKeys[0].columns = ["id", "tenant"];
  assert.equal(er.buildRelationships([parent, child])[0].sourceCardinality, "1");
  child.foreignKeys[0].columns = ["id"];
  child.indexes = [{ unique: true, columns: ["id"] }];
  assert.equal(er.buildRelationships([parent, child])[0].sourceCardinality, "1");
});

test("engineering FK beyond the first 12 columns falls back to the entity", () => {
  const parent = table("parent");
  const child = table("child", "public", Array.from({ length: 13 }, (_, i) => column(`c${i}`)));
  child.foreignKeys.push({ name: "fk_late", columns: ["c12"], ref_table: "parent", ref_columns: ["id"] });
  const graph = er.computeEngineeringLayout([parent, child], er.buildRelationships([parent, child]));
  assert.equal(graph.attributes.filter(a => a.table === "child").length, 12, "preserve existing attribute display limit");
  assert.equal(graph.diamonds.length, 1);
  assert.equal(graph.links.filter(l => l.kind === "relation").length, 2);
});

test("empty engineering graph has finite usable bounds", () => {
  const graph = er.computeEngineeringLayout([], []);
  assert.ok(Number.isFinite(graph.width) && graph.width > 0);
  assert.ok(Number.isFinite(graph.height) && graph.height > 0);
});

test("cache coalesces same-scope requests and deep-clones for each consumer", async () => {
  const conn = connection();
  const pending = deferred();
  let calls = 0;
  const loader = () => { calls++; return pending.promise; };
  const a = cache.loadDiagramMetadataCached(conn, "public", loader);
  const b = cache.loadDiagramMetadataCached(conn, "public", loader);
  await Promise.resolve();
  assert.equal(calls, 1);
  pending.resolve(metadata());
  const [first, second] = await Promise.all([a, b]);
  first.tables[0].columns[0].name = "dirty-draft";
  assert.equal(second.tables[0].columns[0].name, "id");
  const hit = await cache.loadDiagramMetadataCached(conn, "public", () => { throw new Error("should hit cache"); });
  assert.equal(hit.tables[0].columns[0].name, "id");
});

test("forced refresh supersedes an old load and accepts new callers", async () => {
  const conn = connection(), old = deferred(), fresh = deferred();
  const a = cache.loadDiagramMetadataCached(conn, "public", () => old.promise);
  const b = cache.loadDiagramMetadataCached(conn, "public", () => fresh.promise, true);
  const joined = cache.loadDiagramMetadataCached(conn, "public", () => { throw new Error("must join refresh"); });
  fresh.resolve(metadata("fresh"));
  assert.equal((await b).tables[0].table, "fresh");
  assert.equal((await joined).tables[0].table, "fresh");
  old.resolve(metadata("stale"));
  await a;
  assert.equal((await cache.loadDiagramMetadataCached(conn, "public", () => { throw new Error("cache expected"); })).tables[0].table, "fresh");
});

test("invalidation prevents old in-flight results from resurrecting the snapshot", async () => {
  const conn = connection(), old = deferred();
  const a = cache.loadDiagramMetadataCached(conn, "public", () => old.promise);
  cache.invalidateDiagramMetadata(conn, "public");
  old.resolve(metadata("stale"));
  await a;
  let calls = 0;
  const result = await cache.loadDiagramMetadataCached(conn, "public", () => { calls++; return Promise.resolve(metadata("new")); });
  assert.equal(calls, 1);
  assert.equal(result.tables[0].table, "new");
});

test("refresh failure or partial metadata does not replace the complete snapshot", async () => {
  const conn = connection();
  await cache.loadDiagramMetadataCached(conn, "public", () => Promise.resolve(metadata("complete")));
  await assert.rejects(cache.loadDiagramMetadataCached(conn, "public", () => Promise.reject(new Error("denied")), true), /denied/);
  const partial = await cache.loadDiagramMetadataCached(conn, "public", () => Promise.resolve(metadata("partial", [{ table: "partial", error: "fields denied" }])), true);
  assert.equal(partial.failures.length, 1);
  const good = await cache.loadDiagramMetadataCached(conn, "public", () => { throw new Error("snapshot should survive"); });
  assert.equal(good.tables[0].table, "complete");
});

test("partial reads are not cached as a complete empty diagram", async () => {
  const conn = connection();
  const partial = { tables: [], failures: [{ table: "items", error: "denied" }], limitations: [] };
  await cache.loadDiagramMetadataCached(conn, "public", () => Promise.resolve(partial));
  let calls = 0;
  await cache.loadDiagramMetadataCached(conn, "public", () => { calls++; return Promise.resolve(metadata()); });
  assert.equal(calls, 1);
});

test("cache expires after DBX's five-minute TTL", async () => {
  const originalNow = Date.now, conn = connection();
  let now = 1000;
  Date.now = () => now;
  try {
    await cache.loadDiagramMetadataCached(conn, "public", () => Promise.resolve(metadata("old")));
    now += cache.DIAGRAM_METADATA_CACHE_TTL_MS;
    assert.equal((await cache.loadDiagramMetadataCached(conn, "public", () => Promise.resolve(metadata("new")))).tables[0].table, "new");
  } finally { Date.now = originalNow; }
});

test("cache isolates namespaces and connection configuration changes", async () => {
  const conn = connection();
  await cache.loadDiagramMetadataCached(conn, "public", () => Promise.resolve(metadata("old")));
  assert.equal((await cache.loadDiagramMetadataCached(conn, "other", () => Promise.resolve(metadata("other")))).tables[0].table, "other");
  assert.equal((await cache.loadDiagramMetadataCached({ ...conn, host: "new-endpoint" }, "public", () => Promise.resolve(metadata("new-host")))).tables[0].table, "new-host");
  assert.equal((await cache.loadDiagramMetadataCached(conn, "public", () => { throw new Error("old endpoint cache expected"); })).tables[0].table, "old");
});

test("cache bounds retained namespaces and refreshes the least-recently-used entry", async () => {
  const conn = connection();
  for (let i = 0; i < 16; i++) await cache.loadDiagramMetadataCached(conn, `ns_${i}`, () => Promise.resolve(metadata(`table_${i}`)));
  await cache.loadDiagramMetadataCached(conn, "ns_0", () => { throw new Error("expected hit"); });
  await cache.loadDiagramMetadataCached(conn, "ns_16", () => Promise.resolve(metadata("table_16")));
  let calls = 0;
  await cache.loadDiagramMetadataCached(conn, "ns_1", () => { calls++; return Promise.resolve(metadata("reloaded")); });
  assert.equal(calls, 1, "the least-recently-used namespace must have been evicted");
  assert.equal((await cache.loadDiagramMetadataCached(conn, "ns_0", () => { throw new Error("recent hit retained"); })).tables[0].table, "table_0");
});

test("multiple forced callers coalesce and propagate the loader's failure", async () => {
  const conn = connection(), pending = deferred();
  let calls = 0;
  const loader = () => { calls++; return pending.promise; };
  const a = cache.loadDiagramMetadataCached(conn, "public", loader, true);
  const b = cache.loadDiagramMetadataCached(conn, "public", loader, true);
  const assertions = [assert.rejects(a, /catalog denied/), assert.rejects(b, /catalog denied/)];
  await Promise.resolve();
  assert.equal(calls, 1);
  pending.reject(new Error("catalog denied"));
  await Promise.all(assertions);
});

test("1,000-table model stays complete with finite geometry", t => {
  const tables = Array.from({ length: 1000 }, (_, i) => table(`table_${i}`, "public", Array.from({ length: 20 }, (_, c) => column(`c${c}`, c === 0))));
  for (let i = 1; i < tables.length; i++) tables[i].foreignKeys.push({ name: "fk", columns: ["c1"], ref_table: tables[i - 1].name, ref_columns: ["c0"] });
  const started = performance.now();
  const relationships = er.buildRelationships(tables), nodes = er.computeLayout(tables, relationships);
  const engineering = er.computeEngineeringLayout(tables, relationships);
  assert.equal(nodes.length, 1000);
  assert.equal(relationships.length, 999);
  assert.equal(engineering.diamonds.length, 999);
  assert.ok(nodes.every(n => Number.isFinite(n.x) && Number.isFinite(n.y) && n.height <= 276));
  t.diagnostic(`1000 tables / 20000 columns / 999 FK: model ${Math.round(performance.now() - started)}ms`);
});
