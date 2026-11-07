import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";

const bundle = await build({ entryPoints: ["src/core/manualTransaction.ts"], bundle: true, platform: "node", format: "esm", write: false, logLevel: "silent" });
const { manualTransactionUnavailableReason: reason, manualTransactionStatusText: text, MANUAL_TRANSACTION_WRITE_COMMANDS: writes } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`);
const cfg = (kind, extra = {}) => ({ id: "conn", name: "测试", kind, params: "", read_only: false, ...extra });

test("手动事务仅对真实原生SQLite/MySQL/PostgreSQL协议及实现分支启用", () => {
  for (const kind of ["sqlite", "mysql", "mariadb", "postgresql"]) assert.equal(reason(cfg(kind)), null, kind);
  for (const kind of ["redis", "mongodb", "sqlserver", "unknown-kind"]) assert.match(reason(cfg(kind)), /仅支持/);
  assert.match(reason(undefined), /选择/);
});
test("只读连接和自定义ODBC不借标称数据库kind获得事务能力", () => {
  assert.match(reason(cfg("sqlite", { read_only: true })), /只读/);
  assert.match(reason(cfg("mysql", { params: '@navkit-driver:{"version":1,"driver_id":"custom-odbc","template":""}' })), /仅支持/);
});
test("事务状态区分自动提交、所有者、未提交和失败待回滚", () => {
  assert.equal(text(undefined, "query-a"), "自动提交");
  assert.equal(text({ active: false, owner: "", schema: "", failed: false }, "query-a"), "自动提交");
  const active = { active: true, schema: "main", owner: "query-a", failed: false };
  assert.match(text(active, "query-a"), /未提交/);
  assert.match(text(active, "query-b"), /另一个查询页/);
  assert.match(text({ ...active, failed: true }, "query-a"), /必须回滚/);
});
test("手动事务禁止其它自动提交写入入口，但不混淆保存SQL/复制/导出", () => {
  for (const command of ["data-sync", "data-generation", "import", "tree-import", "grid-commit", "restore-backup", "delete-field", "connection-properties"]) assert(writes.has(command), command);
  for (const command of ["save", "save-as", "export", "copy", "run"]) assert(!writes.has(command), command);
});
